//! The offline evaluation of the recommendations: a history replayed in
//! order of time, every recommendation made only of what came before it and
//! scored by what the user did after it.
//!
//! * [`Dataset`]: everything the engine reads of a library (the catalog,
//!   the sound, the song types, the history, the likes and playlists, the
//!   collective graph), plain and serde: a copy of a real library is read
//!   once by the runtime and evaluated here, in this crate alone.
//!   [`synthetic`] makes up listeners with a hidden taste (one kind of
//!   music, everything, background hours, a skipper, a cold start, a big
//!   history), so the engine is measured for any user, not one.
//! * [`evaluate`] measures, on the second half of the history:
//!   - **the next start** ([`crate::model::replay`]): where the model of
//!     the next start ranks the real next start among the whole catalog,
//!     hit@10, hit@50, MRR and NDCG@10, on every start, the free ones (not
//!     the next on the album) and the listened ones (not skipped early);
//!   - **autoplay**: at sampled moments the radio that goes on after a
//!     track (built of the history before it) gives ten; how many of them
//!     the user listened to in the rest of that session and within a week,
//!     whether the first was the real next, how many artists and worlds
//!     they are of, the largest world's share and run, how far apart they
//!     sound, how many were never heard, how often the user skips them
//!     early over the whole history;
//!   - **the mixes of the home** ([`crate::mixes::row`]: «Микс дня», the
//!     genre and cluster mixes, the part of the day, the moments) at
//!     sampled mornings: the share of their songs listened to within a week
//!     and the same spread, novelty (never played by the mixes' own rule,
//!     [`crate::mixes::never_heard`]) and skips; the share of the songs
//!     they play that are new songs of the services («С новым» over the
//!     discoveries and the graph's finds the dataset carries,
//!     [`Dataset::new`]);
//!   - what two early skips do to autoplay ([`two_skips`]): at the same
//!     moments its first two picks skipped early, autoplay picked again;
//!     how often its next pick is of another taste profile than both, and
//!     how often its next picks sound farther from the skipped two than the
//!     ones it had queued after them;
//!   - for both, the share of picks by an artist never played before;
//!   - the open world (docs/recommendations.md §1): the mixes that are
//!     niches ([`crate::open_world::is_niche`]), the share of new songs
//!     they play and how many artists they are of (a niche stuck on two
//!     artists is what the open world is for), the share of mixes mostly
//!     new; and the wave of a filter ([`crate::open_world::next`]: «Спокойное»
//!     and any music) at the same mornings — its share of new music, its
//!     artists, whether it was a niche;
//!   - a session of «Моя волна» ([`wave_session`], [`crate::wave`]) at
//!     the same mornings: the share of new music it plays at each novelty
//!     (heard out song by song, three at a time as the queue asks), and
//!     how often two early skips of a block's first songs move it to
//!     another cluster;
//!   - the share of the catalog all lists ever held, and for a synthetic
//!     listener how high the hidden taste rates what was picked (0.5 is a
//!     random pick);
//!   - how long a build of the model takes over the whole history, the
//!     graph of similar music alone, and a walk of it from a session.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use nikgob_core::model::{ArtistId, EndReason, EventContext, EventOrigin, EventType, ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::StreamTrack;
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::graph::{Edge, GraphInput, GraphSource};
use crate::index::{VectorIndex, euclid};
use crate::mix_new::{self, NewCandidate};
use crate::mixes::{Builder, Kept, Mix, MixKey, Settings as MixSettings, never_heard, row};
use crate::open_world::{Character, OpenCandidate, WaveContext, WaveFilter};
use crate::model::{Hits, Live, ModelInput, ReplayReport, UserModel, replay};
use crate::pick::{Item, Recs};
use crate::radio::{RadioParams, RadioSeed, RadioState};
use crate::session::{Ended, Session};
use crate::songtype::{SongTypes, Voice};
use crate::taste::{Catalog, DAY_MS, DayPart, EARLY_SKIP_MS, HOUR_MS, Play, SESSION_GAP_MS, TasteInput, TrackInfo};

pub mod synthetic;

/// One catalog track of a [`Dataset`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DTrack {
    pub id: TrackId,
    pub artists: Vec<ArtistId>,
    pub release: Option<ReleaseId>,
    pub local: bool,
    pub song: Option<TrackId>,
    pub world: Option<u64>,
    pub duration_ms: Option<u32>,
    pub position: u32,
}

/// One event of a [`Dataset`]'s history.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DPlay {
    pub track: TrackId,
    pub ts: UnixMs,
    pub kind: EventType,
    pub position_ms: Option<u32>,
    pub context: EventContext,
    pub origin: Option<EventOrigin>,
    pub end_reason: Option<EndReason>,
    pub attention: Option<f32>,
}

impl From<&Play> for DPlay {
    fn from(p: &Play) -> Self {
        Self {
            track: p.track,
            ts: p.ts,
            kind: p.kind,
            position_ms: p.position_ms,
            context: p.context,
            origin: p.origin,
            end_reason: p.end_reason,
            attention: p.attention,
        }
    }
}

impl From<&DPlay> for Play {
    fn from(p: &DPlay) -> Self {
        Self {
            track: p.track,
            ts: p.ts,
            kind: p.kind,
            position_ms: p.position_ms,
            context: p.context,
            origin: p.origin,
            end_reason: p.end_reason,
            attention: p.attention,
        }
    }
}

/// An edge of the collective graph: from, to, its place, of how many, the source.
pub type DEdge<T> = (T, T, u32, u32, GraphSource);

/// A new song of the services a [`Dataset`] carries: a discovery or a song
/// the graph found beside the library's, what «С новым» of the mixes draws
/// from ([`crate::mix_new`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DNew {
    pub track: StreamTrack,
    /// Its analysed preview (raw), if any.
    pub vector: Option<Vec<f32>>,
    /// The library's tracks the graph links it to.
    pub near: Vec<TrackId>,
    /// [`crate::mix_new::NewCandidate::weight`].
    pub weight: f32,
    /// Its main artist when the library has them.
    #[serde(default)]
    pub artist: Option<ArtistId>,
}

/// Everything the engine reads of a library (see the module docs).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Dataset {
    pub name: String,
    pub tracks: Vec<DTrack>,
    pub aliases: Vec<(TrackId, TrackId)>,
    pub vectors: Vec<(TrackId, Vec<f32>)>,
    pub types: SongTypes,
    pub events: Vec<DPlay>,
    pub likes: Vec<TrackId>,
    pub like_times: Vec<(TrackId, UnixMs)>,
    pub saved: Vec<TrackId>,
    pub saved_elsewhere: Vec<(TrackId, Vec<ArtistId>)>,
    pub playlist_entries: Vec<(TrackId, UnixMs)>,
    pub playlists: Vec<Vec<TrackId>>,
    pub disliked_tracks: Vec<TrackId>,
    pub disliked_artists: Vec<ArtistId>,
    pub utc_offset_min: i32,
    pub personal_graph: bool,
    pub library_prior: bool,
    pub dislike_neighbours: bool,
    pub graph_tracks: Vec<DEdge<TrackId>>,
    pub graph_artists: Vec<DEdge<ArtistId>>,
    pub tags: Vec<(ArtistId, String, f32)>,
    pub popular: Vec<(TrackId, u32, u32)>,
    /// The edges to songs and artists the library lacks (co-citation).
    #[serde(default)]
    pub graph_outside_tracks: Vec<(TrackId, u32, f32)>,
    #[serde(default)]
    pub graph_outside_artists: Vec<(ArtistId, u32, f32)>,
    /// A synthetic listener's hidden taste of each track (0…1).
    #[serde(default)]
    pub truth: Vec<(TrackId, f32)>,
    /// The new songs of the services the library's mixes could take.
    #[serde(default)]
    pub new: Vec<DNew>,
}

/// A [`Dataset`] as the engine takes it.
pub struct Parts {
    pub catalog: Catalog,
    pub index: VectorIndex<TrackId>,
    pub types: SongTypes,
    pub input: TasteInput,
    pub graph: GraphInput,
    pub truth: HashMap<TrackId, f32>,
    /// [`Dataset::new`].
    pub new: Vec<DNew>,
}

impl Dataset {
    /// The dataset of what a build reads: the catalog, its sound, the song
    /// types, the history and the graph.
    pub fn of(name: &str, catalog: &Catalog, index: &VectorIndex<TrackId>, types: &SongTypes, input: &TasteInput, graph: Option<&GraphInput>) -> Self {
        let edges = |list: &[Edge<TrackId>]| list.iter().map(|e| (e.from, e.to, e.rank, e.of, e.source)).collect();
        let artist_edges = |list: &[Edge<ArtistId>]| list.iter().map(|e| (e.from, e.to, e.rank, e.of, e.source)).collect();
        Self {
            name: name.to_owned(),
            tracks: catalog
                .tracks()
                .iter()
                .map(|t| DTrack {
                    id: t.id,
                    artists: t.artists.clone(),
                    release: t.release,
                    local: t.local,
                    song: t.song,
                    world: t.world,
                    duration_ms: t.duration_ms,
                    position: t.position,
                })
                .collect(),
            aliases: catalog.aliases().map(|(copy, i)| (copy, catalog.tracks()[i].id)).collect(),
            vectors: index.iter().map(|(id, v)| (id, v.to_vec())).collect(),
            types: types.clone(),
            events: input.events.iter().map(DPlay::from).collect(),
            likes: input.likes.clone(),
            like_times: input.like_times.clone(),
            saved: input.saved.clone(),
            saved_elsewhere: input.saved_elsewhere.clone(),
            playlist_entries: input.playlist_entries.clone(),
            playlists: input.playlists.clone(),
            disliked_tracks: input.disliked_tracks.clone(),
            disliked_artists: input.disliked_artists.clone(),
            utc_offset_min: input.utc_offset_min,
            personal_graph: input.personal_graph,
            library_prior: input.library_prior,
            dislike_neighbours: input.dislike_neighbours,
            graph_tracks: graph.map(|g| edges(&g.tracks)).unwrap_or_default(),
            graph_artists: graph.map(|g| artist_edges(&g.artists)).unwrap_or_default(),
            tags: graph.map(|g| g.tags.clone()).unwrap_or_default(),
            popular: graph.map(|g| g.popular.clone()).unwrap_or_default(),
            graph_outside_tracks: graph.map(|g| g.outside_tracks.clone()).unwrap_or_default(),
            graph_outside_artists: graph.map(|g| g.outside_artists.clone()).unwrap_or_default(),
            truth: Vec::new(),
            new: Vec::new(),
        }
    }

    /// The engine's inputs. The kept profiles of the builds before are left
    /// out: they would tell the past what came later.
    pub fn parts(&self) -> Parts {
        let catalog = Catalog::new(self.tracks.iter().map(|t| {
            let mut info = TrackInfo::new(t.id, t.artists.clone(), t.release).with_local(t.local).with_duration(t.duration_ms).with_position(t.position);
            info.song = t.song;
            info.world = t.world;
            info
        }))
        .with_aliases(self.aliases.iter().copied());
        let mut index = VectorIndex::new();
        for (id, v) in &self.vectors {
            if let Ok(v) = <[f32; DIM]>::try_from(v.as_slice()) {
                index.insert(*id, v);
            }
        }
        let edge = |e: &DEdge<TrackId>| Edge { from: e.0, to: e.1, rank: e.2, of: e.3, source: e.4 };
        let artist_edge = |e: &DEdge<ArtistId>| Edge { from: e.0, to: e.1, rank: e.2, of: e.3, source: e.4 };
        Parts {
            catalog,
            index,
            types: self.types.clone(),
            input: TasteInput {
                events: self.events.iter().map(Play::from).collect(),
                likes: self.likes.clone(),
                saved: self.saved.clone(),
                saved_elsewhere: self.saved_elsewhere.clone(),
                like_times: self.like_times.clone(),
                playlist_entries: self.playlist_entries.clone(),
                disliked_tracks: self.disliked_tracks.clone(),
                disliked_artists: self.disliked_artists.clone(),
                utc_offset_min: self.utc_offset_min,
                dislike_neighbours: self.dislike_neighbours,
                personal_graph: self.personal_graph,
                library_prior: self.library_prior,
                playlists: self.playlists.clone(),
                ..TasteInput::default()
            },
            graph: GraphInput {
                tracks: self.graph_tracks.iter().map(edge).collect(),
                artists: self.graph_artists.iter().map(artist_edge).collect(),
                tags: self.tags.clone(),
                popular: self.popular.clone(),
                outside_tracks: self.graph_outside_tracks.clone(),
                outside_artists: self.graph_outside_artists.clone(),
            },
            truth: self.truth.iter().copied().collect(),
            new: self.new.clone(),
        }
    }
}

impl Parts {
    /// What a build reads besides the catalog and the history.
    pub fn model_input(&self) -> ModelInput<'_> {
        ModelInput { index: &self.index, types: Some(&self.types), weights: None, graph: (!self.graph.is_empty()).then_some(&self.graph) }
    }

    /// The history as it stood just before `t`: its events, the likes set
    /// by then (one without a time is an old one), the playlist entries
    /// added by then.
    pub fn before(&self, t: UnixMs) -> TasteInput {
        let liked_later: HashSet<TrackId> = self.input.like_times.iter().filter(|(_, at)| *at >= t).map(|(id, _)| *id).collect();
        TasteInput {
            events: self.input.events.iter().filter(|p| p.ts < t).copied().collect(),
            likes: self.input.likes.iter().filter(|id| !liked_later.contains(id)).copied().collect(),
            like_times: self.input.like_times.iter().filter(|(_, at)| *at < t).copied().collect(),
            playlist_entries: self.input.playlist_entries.iter().filter(|(_, at)| *at < t).copied().collect(),
            suggested_tracks: Vec::new(),
            suggested_artists: Vec::new(),
            ..self.input.clone()
        }
    }

    /// The model at `t` of the history before it.
    pub fn model_at(&self, t: UnixMs) -> UserModel {
        UserModel::build(&self.catalog, &self.before(t), self.model_input(), t)
    }

    /// The tracks that suit the background, as far as the dataset tells
    /// (no names here): the instrumental ones.
    pub fn suits(&self) -> Vec<bool> {
        self.catalog.tracks().iter().map(|t| self.types.get(t.id).is_some_and(|s| s.voice == Voice::Instrumental)).collect()
    }
}

/// What [`evaluate`] does.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The replay of the next start over the whole catalog (slow over a big one).
    pub next: bool,
    /// Autoplay moments sampled at most.
    pub autoplay_points: usize,
    /// Mornings of mixes sampled at most.
    pub mix_days: usize,
    /// A pick counts as listened within this after the moment.
    pub window_ms: i64,
    /// Picks of autoplay at each moment.
    pub autoplay_len: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { next: true, autoplay_points: 40, mix_days: 8, window_ms: 7 * DAY_MS, autoplay_len: 10 }
    }
}

/// How a set of lists of recommendations did (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ListStats {
    pub lists: usize,
    pub len: f32,
    /// The share of picks listened to within the window after.
    pub precision: f32,
    /// The share of picks listened to in the rest of the session (autoplay).
    pub session: f32,
    /// The share of lists with a pick listened to within the window.
    pub hit: f32,
    /// The first pick was the real next start (autoplay).
    pub first: f32,
    /// Distinct main artists and worlds per pick.
    pub artists: f32,
    pub worlds: f32,
    /// The largest world's share of a list and its longest run.
    pub world_top: f32,
    pub world_run: f32,
    /// The mean distance between two picks, in median distances.
    pub spread: f32,
    /// The share of picks never heard before: for the mixes by their own
    /// rule ([`crate::mixes::never_heard`]: never started and not loved
    /// otherwise), for autoplay never started.
    pub unheard: f32,
    /// The share of picks by an artist never played before.
    pub unheard_artists: f32,
    /// The mixes: the share of what they play that is new songs of the
    /// services («С новым» placed between their songs).
    pub new_service: f32,
    /// The share of the picks heard before that were listened to within
    /// the window (what a list knows the user loves, without its novelty).
    pub known: f32,
    /// The mean share of early skips of the picks' plays over the whole
    /// history (of the picks with at least two plays).
    pub skips: f32,
    /// A synthetic listener: the mean percentile of the hidden taste of
    /// the picks (0.5 is random).
    pub truth: Option<f32>,
    /// The share of the catalog the lists ever held.
    pub coverage: f32,
    /// The mixes: the niches of a home, the share of new songs they play,
    /// their distinct artists (their songs' and the new songs'), the share
    /// of them of two artists or fewer; the share of the mixes that are
    /// mostly (60 % or more) new.
    pub niches: f32,
    pub niche_new: f32,
    pub niche_artists: f32,
    pub niche_stuck: f32,
    pub mostly_new: f32,
}

/// How the waves of a filter did ([`crate::open_world::next`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaveStats {
    pub lists: usize,
    /// The share of new songs of the pool, and of lists that were niches.
    pub new: f32,
    pub niche: f32,
    /// Distinct artists per list, and per pick.
    pub artists: f32,
    pub artists_per_pick: f32,
    /// The library tracks fitting the filter.
    pub fitting: f32,
}

/// How sessions of «Моя волна» did ([`wave_session`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaveSessionStats {
    pub sessions: usize,
    /// The share of new music played at «Знакомое», «Баланс», «Больше
    /// нового», «Только новое».
    pub new: [f32; 4],
    /// Clusters the wave weighed, on average.
    pub clusters: f32,
    /// Sessions of two clusters or more where two early skips were
    /// measured, and the share of them whose next block was of another.
    pub skips_n: usize,
    pub skips_moved: f32,
}

/// What [`evaluate`] measured.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub name: String,
    pub tracks: usize,
    pub events: usize,
    pub starts: usize,
    pub days: f32,
    pub next: Option<ReplayReport>,
    pub autoplay: ListStats,
    pub mixes: ListStats,
    /// The row's own cards alone (the niches' cards of their own left
    /// out): the mixes as they were before them.
    pub mixes_row: ListStats,
    /// The mixes of a home and how many songs each.
    pub mixes_per_home: f32,
    /// The waves of «Спокойное» and of any music (the user's share).
    pub wave_calm: WaveStats,
    pub wave_any: WaveStats,
    /// Sessions of «Моя волна».
    pub wave_sessions: WaveSessionStats,
    /// A build of the graph of similar music alone (ms) and a walk of it
    /// from three tracks (µs).
    pub graph_ms: f32,
    pub walk_us: f32,
    /// A build of the model over the whole history, learning the weights
    /// and with them cached; the home's mixes and an autoplay of ten over it.
    pub build_ms: f32,
    pub build_cached_ms: f32,
    pub mixes_ms: f32,
    pub autoplay_ms: f32,
    /// [`two_skips`]: the moments measured, the share whose next pick is of
    /// another profile (of those where all three have one) and the share
    /// whose next picks sound farther from the skipped two.
    pub skips2: (usize, f32, f32),
}

/// The outcome of every start of `events`, in their order of time.
struct Starts {
    /// (time, track, outcome, its session's number)
    list: Vec<(UnixMs, TrackId, Ended, usize)>,
}

impl Starts {
    fn of(events: &[Play]) -> Self {
        let mut sorted: Vec<&Play> = events.iter().filter(|p| p.origin != Some(EventOrigin::Import)).collect();
        sorted.sort_by_key(|p| (p.ts, p.kind == EventType::Start));
        let mut list: Vec<(UnixMs, TrackId, Ended, usize)> = Vec::new();
        let mut open: HashMap<TrackId, usize> = HashMap::new();
        let mut session = 0usize;
        let mut last: Option<UnixMs> = None;
        for p in sorted {
            if last.is_some_and(|l| p.ts - l >= SESSION_GAP_MS) {
                session += 1;
            }
            last = Some(p.ts);
            match p.kind {
                EventType::Start => {
                    open.insert(p.track, list.len());
                    list.push((p.ts, p.track, Ended::Open, session));
                }
                EventType::Complete | EventType::Skip => {
                    if let Some(k) = open.remove(&p.track) {
                        list[k].2 = ended_of(p);
                    }
                }
                _ => {}
            }
        }
        Self { list }
    }

    /// The tracks listened to (not skipped early) in `[from, to)`.
    fn listened(&self, from: UnixMs, to: UnixMs) -> HashSet<TrackId> {
        let a = self.list.partition_point(|s| s.0 < from);
        let b = self.list.partition_point(|s| s.0 < to);
        self.list[a..b].iter().filter(|s| s.2 != Ended::Early).map(|s| s.1).collect()
    }
}

fn ended_of(p: &Play) -> Ended {
    match p.kind {
        EventType::Complete => Ended::Complete,
        _ if p.end_reason == Some(EndReason::AppClosed) => Ended::Open,
        _ if p.position_ms.is_some_and(|x| x < EARLY_SKIP_MS) => Ended::Early,
        _ => Ended::Late,
    }
}

/// The session going on just before `t`, of the history's events (its
/// starts and how each ended).
fn session_before(events: &[Play], t: UnixMs) -> Session {
    let mut sorted: Vec<&Play> = events.iter().filter(|p| p.ts < t && p.origin != Some(EventOrigin::Import)).collect();
    sorted.sort_by_key(|p| (p.ts, p.kind == EventType::Start));
    // Back to the start of the session.
    let mut from = sorted.len();
    while from > 0 && (from == sorted.len() || sorted[from].ts - sorted[from - 1].ts < SESSION_GAP_MS) {
        from -= 1;
    }
    if sorted.last().is_some_and(|p| t - p.ts >= SESSION_GAP_MS) {
        return Session::default();
    }
    let mut s = Session::default();
    for p in &sorted[from..] {
        match (p.kind, p.position_ms) {
            (EventType::Start, _) => {
                s.start(p.track, p.ts, p.context, p.origin);
            }
            (EventType::Complete | EventType::Skip, Some(at)) => s.end_at(p.track, ended_of(p), at, None, false),
            (EventType::Complete | EventType::Skip, None) => s.end(p.track, ended_of(p)),
            (EventType::Seek, Some(to)) => s.seek(p.track, to),
            (EventType::Like, _) => s.like(p.track),
            _ => {}
        }
    }
    s
}

/// Accumulates [`ListStats`].
#[derive(Default)]
struct Lists {
    n: usize,
    len: f32,
    precision: f32,
    session: f32,
    hit: f32,
    first: f32,
    artists: f32,
    worlds: f32,
    world_top: f32,
    world_run: f32,
    spread: f32,
    spread_n: usize,
    unheard: f32,
    unheard_artists: f32,
    new_service: f32,
    new_n: usize,
    known: f32,
    known_n: usize,
    skips: f32,
    skips_n: usize,
    truth: f32,
    truth_n: usize,
    seen: HashSet<usize>,
    niches: usize,
    niche_new: f32,
    niche_artists: f32,
    niche_stuck: usize,
    mostly_new: usize,
}

/// What a list is scored against.
struct Truth<'a> {
    listened: &'a HashSet<TrackId>,
    session: Option<&'a HashSet<TrackId>>,
    next: Option<TrackId>,
    /// Heard before the moment (see [`ListStats::unheard`]).
    heard: &'a dyn Fn(usize) -> bool,
    /// Its main artist was played before the moment.
    artist_heard: &'a dyn Fn(usize) -> bool,
}

impl Lists {
    fn add(&mut self, ctx: &Ctx<'_>, list: &[usize], truth: &Truth<'_>) {
        if list.is_empty() {
            return;
        }
        let k = list.len() as f32;
        self.n += 1;
        self.len += k;
        let ids: Vec<TrackId> = list.iter().map(|&i| ctx.catalog.tracks()[i].id).collect();
        let got = ids.iter().filter(|id| truth.listened.contains(id)).count() as f32;
        self.precision += got / k;
        self.hit += f32::from(u8::from(got > 0.0));
        if let Some(s) = truth.session {
            self.session += ids.iter().filter(|id| s.contains(id)).count() as f32 / k;
        }
        if let Some(next) = truth.next {
            self.first += f32::from(u8::from(ids[0] == next));
        }
        let artists: HashSet<ArtistId> = list.iter().filter_map(|&i| ctx.catalog.tracks()[i].main_artist()).collect();
        self.artists += artists.len() as f32 / k;
        let worlds: Vec<Option<u64>> = list.iter().map(|&i| ctx.catalog.tracks()[i].world).collect();
        let mut count: HashMap<u64, usize> = HashMap::new();
        for w in worlds.iter().flatten() {
            *count.entry(*w).or_default() += 1;
        }
        let distinct = count.len() + worlds.iter().filter(|w| w.is_none()).count();
        self.worlds += distinct as f32 / k;
        self.world_top += count.values().copied().max().unwrap_or(1) as f32 / k;
        let (mut run, mut best) = (0usize, 0usize);
        for (x, w) in worlds.iter().enumerate() {
            run = if x > 0 && w.is_some() && *w == worlds[x - 1] { run + 1 } else { 1 };
            best = best.max(run);
        }
        self.world_run += best as f32;
        let vs: Vec<Vector> = list.iter().filter_map(|&i| ctx.emb[i]).collect();
        if vs.len() >= 2 {
            let mut d = 0.0f32;
            let mut pairs = 0usize;
            for a in 0..vs.len() {
                for b in a + 1..vs.len() {
                    d += euclid(&vs[a], &vs[b]);
                    pairs += 1;
                }
            }
            self.spread += d / pairs as f32 / ctx.scale;
            self.spread_n += 1;
        }
        self.unheard += list.iter().filter(|&&i| !(truth.heard)(i)).count() as f32 / k;
        self.unheard_artists += list.iter().filter(|&&i| !(truth.artist_heard)(i)).count() as f32 / k;
        let known: Vec<TrackId> = list.iter().filter(|&&i| (truth.heard)(i)).map(|&i| ctx.catalog.tracks()[i].id).collect();
        if !known.is_empty() {
            self.known += known.iter().filter(|id| truth.listened.contains(id)).count() as f32 / known.len() as f32;
            self.known_n += 1;
        }
        for &i in list {
            if let Some(r) = ctx.skip_rate[i] {
                self.skips += r;
                self.skips_n += 1;
            }
            if let Some(p) = ctx.truth_pct.get(i).copied().flatten() {
                self.truth += p;
                self.truth_n += 1;
            }
        }
        self.seen.extend(list.iter().copied());
    }

    /// A mix of `len` songs that plays `new.placed` new songs of the
    /// services between them.
    fn add_new(&mut self, len: usize, new: &HomeNew) {
        if len + new.placed == 0 {
            return;
        }
        let share = new.placed as f32 / (len + new.placed) as f32;
        self.new_service += share;
        self.new_n += 1;
        self.mostly_new += usize::from(share >= 0.6);
        if new.niche {
            self.niches += 1;
            self.niche_new += share;
            self.niche_artists += new.artists as f32;
            self.niche_stuck += usize::from(new.artists <= 2);
        }
    }

    fn stats(&self, catalog: usize) -> ListStats {
        let n = self.n.max(1) as f32;
        ListStats {
            lists: self.n,
            len: self.len / n,
            precision: self.precision / n,
            session: self.session / n,
            hit: self.hit / n,
            first: self.first / n,
            artists: self.artists / n,
            worlds: self.worlds / n,
            world_top: self.world_top / n,
            world_run: self.world_run / n,
            spread: self.spread / self.spread_n.max(1) as f32,
            unheard: self.unheard / n,
            unheard_artists: self.unheard_artists / n,
            new_service: self.new_service / self.new_n.max(1) as f32,
            known: self.known / self.known_n.max(1) as f32,
            skips: self.skips / self.skips_n.max(1) as f32,
            truth: (self.truth_n > 0).then(|| self.truth / self.truth_n as f32),
            coverage: self.seen.len() as f32 / catalog.max(1) as f32,
            niches: self.niches as f32,
            niche_new: self.niche_new / self.niches.max(1) as f32,
            niche_artists: self.niche_artists / self.niches.max(1) as f32,
            niche_stuck: self.niche_stuck as f32 / self.niches.max(1) as f32,
            mostly_new: self.mostly_new as f32 / self.new_n.max(1) as f32,
        }
    }
}

/// What every list is measured with.
struct Ctx<'a> {
    catalog: &'a Catalog,
    emb: Vec<Option<Vector>>,
    scale: f32,
    /// The share of early skips of each track's plays over the whole history.
    skip_rate: Vec<Option<f32>>,
    /// The percentile of the hidden taste of each track.
    truth_pct: Vec<Option<f32>>,
}

/// The positions of `picks` (catalog tracks only).
fn positions(catalog: &Catalog, picks: impl IntoIterator<Item = Item>) -> Vec<usize> {
    picks
        .into_iter()
        .filter_map(|p| match p {
            Item::Track(id) => catalog.position(id),
            Item::Stream(_) => None,
        })
        .collect()
}

/// The home's mixes of `model` (a build at `now`): the row of «Миксы для
/// тебя» as the runtime makes it, every track allowed.
pub fn home_mixes(parts: &Parts, model: &UserModel, suits: &[bool], epoch: u64) -> Vec<Mix> {
    let recs = Recs::new(&parts.catalog, &model.taste, &parts.index).with_model(model).with_background(suits);
    let fit = |i: usize| suits.get(i).copied().unwrap_or(false);
    let part = DayPart::at(model.now(), parts.input.utc_offset_min);
    let mut tags: HashMap<ArtistId, Vec<(String, f32)>> = HashMap::new();
    for (a, t, w) in &parts.graph.tags {
        tags.entry(*a).or_default().push((t.clone(), *w));
    }
    let ok = |_: usize| true;
    let mut b = Builder::new(&recs, model, &ok, &fit, epoch, part, Live::default(), MixSettings::default());
    if !tags.is_empty() {
        b = b.with_genres(&|a| tags.get(&a).cloned().unwrap_or_default());
    }
    let kept = Kept::default();
    let pinned = b.pinned(&kept);
    let (daily, _) = b.daily(&kept);
    let (moments, _) = b.lineup(&kept);
    row(daily, pinned.into_iter().next(), moments)
}

/// The family of genres of each catalog track by its main artist's tags
/// (what [`Builder::with_genres`] tells).
pub fn genres(parts: &Parts) -> Vec<Option<u8>> {
    let mut tags: HashMap<ArtistId, Vec<(String, f32)>> = HashMap::new();
    for (a, t, w) in &parts.graph.tags {
        tags.entry(*a).or_default().push((t.clone(), *w));
    }
    let mut of: HashMap<ArtistId, Option<u8>> = HashMap::new();
    parts
        .catalog
        .tracks()
        .iter()
        .map(|t| t.main_artist().and_then(|a| *of.entry(a).or_insert_with(|| tags.get(&a).and_then(|x| crate::mix_names::leading(x)))))
        .collect()
}

/// The home's mixes ([`home_mixes`]) with the niches it holds a card of
/// while new music plays ([`crate::mix_new::with_niches`]), and how many
/// of them are the row's own (the niches' own cards come after).
pub fn home_with_niches(parts: &Parts, model: &UserModel, suits: &[bool], epoch: u64) -> (Vec<Mix>, usize) {
    let mut home = home_mixes(parts, model, suits, epoch);
    let row = home.len();
    let genre_of = genres(parts);
    let genre = |i: usize| genre_of.get(i).copied().flatten();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let none = |_: &StreamTrack| Vec::new();
    let ctx = WaveContext { model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &none };
    let cards = crate::open_world::niches(&ctx).into_iter().map(|c| (c, String::new(), Vec::new())).collect();
    mix_new::with_niches(model, &mut home, cards);
    (home, row)
}

/// The user's share of new music the evaluation takes («Новое в радио и
/// миксах» as it comes, «Немного»).
pub const USER_NEW_SHARE: f32 = 0.2;

/// What a mix of the home plays of new music ([`home_new`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HomeNew {
    /// The new songs placed.
    pub placed: usize,
    /// It is a niche ([`crate::mix_new::NewMode::Niche`]).
    pub niche: bool,
    /// Its distinct artists, its songs' and the placed new songs'.
    pub artists: usize,
}

/// The new songs of the services a dataset carries as the mixes take them.
pub fn new_candidates(parts: &Parts, model: &UserModel) -> Vec<NewCandidate> {
    parts
        .new
        .iter()
        .map(|d| {
            let v: Option<Vector> = d.vector.as_ref().and_then(|v| <[f32; DIM]>::try_from(v.as_slice()).ok());
            let near: Vec<usize> = d.near.iter().filter_map(|t| model.position(*t)).collect();
            let voice = match v.as_ref() {
                Some(v) => mix_new::voice_near(model, &parts.index, v),
                None => mix_new::voice_of(model, &near),
            };
            NewCandidate {
                track: d.track.clone(),
                lane: mix_new::lane_of_new(&parts.types, voice, &d.track),
                soundtrack: d.track.album.as_ref().is_some_and(|a| crate::tiles::franchise(&a.title).is_some()),
                profile: v.map(|v| model.embed(&v)).and_then(|e| model.regions.of(&e, None)),
                near,
                weight: d.weight,
                vector: v,
                artist_tracks: d.artist.map(|a| model.of_artist(a).iter().map(|&i| i as usize).collect()).unwrap_or_default(),
            }
        })
        .collect()
}

/// How many new songs of the services each of `mixes` (a home's row of
/// `model`) plays: [`Parts::new`] through «С новым» as the runtime takes
/// them ([`crate::mix_new::take`]: each mix of the row in turn, a song in
/// one mix only, «Фон» and «Любимые» none, nothing new heard yet) and
/// places them; whether each is a niche and how many artists it is of.
pub fn home_new(parts: &Parts, model: &UserModel, mixes: &[Mix]) -> Vec<HomeNew> {
    let cands = new_candidates(parts, model);
    let mut used: HashSet<String> = HashSet::new();
    mixes
        .iter()
        .map(|m| {
            let own: HashSet<ArtistId> = m.tracks.iter().filter_map(|&i| model.main_artist_at(i)).collect();
            if matches!(m.key, MixKey::Background { .. } | MixKey::Liked) {
                return HomeNew { placed: 0, niche: false, artists: own.len() };
            }
            let (chosen, mode) = mix_new::take(model, m, &cands, None, USER_NEW_SHARE, &mut used);
            let placed = mix_new::placements(mode, &m.anchors, chosen.len()).len();
            let new_artists: HashSet<String> = chosen.iter().take(placed).filter_map(|c| c.track.artists.first()).map(|a| crate::artist_key(&a.name)).collect();
            HomeNew { placed, niche: mode == mix_new::NewMode::Niche, artists: own.len() + new_artists.len() }
        })
        .collect()
}

/// The open pool of a dataset ([`Parts::new`]) as a wave takes it.
pub fn open_pool(parts: &Parts, model: &UserModel) -> Vec<OpenCandidate> {
    new_candidates(parts, model)
        .into_iter()
        .map(|c| OpenCandidate { key: crate::stream_song_key(&c.track), track: c.track, vector: c.vector, lane: c.lane, near: c.near, weight: c.weight })
        .collect()
}

/// The next `n` of a wave of `filter` at `model`'s moment after `session`,
/// over the dataset's open pool.
pub fn wave(parts: &Parts, model: &UserModel, suits: &[bool], pool: &[OpenCandidate], filter: &WaveFilter, session: &Session, n: usize) -> crate::open_world::WaveNext {
    let artist_of: HashMap<String, ArtistId> = parts.new.iter().filter_map(|d| Some((crate::stream_song_key(&d.track), d.artist?))).collect();
    let artist_tracks = |t: &StreamTrack| artist_of.get(&crate::stream_song_key(t)).map(|a| model.of_artist(*a).iter().map(|&i| i as usize).collect()).unwrap_or_default();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let genre_of = genres(parts);
    let genre = |i: usize| genre_of.get(i).copied().flatten();
    let ctx = WaveContext { model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &artist_tracks };
    crate::open_world::next(&ctx, pool, filter, session, n, &HashSet::new())
}

/// The novelties of a wave session ([`crate::wave`]).
pub const WAVE_NOVELTIES: [f32; 4] = [crate::wave::NOVELTY_FAMILIAR, crate::wave::NOVELTY_BALANCE, crate::wave::NOVELTY_MORE, crate::wave::NOVELTY_ONLY_NEW];

/// What one session of «Моя волна» did ([`wave_session`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaveRun {
    /// The share of new music of each novelty of [`WAVE_NOVELTIES`]
    /// (`None`: nothing played).
    pub new: [Option<f32>; 4],
    /// The clusters it weighed.
    pub clusters: usize,
    /// Two early skips of a block's first songs: whether the next block
    /// was of another cluster (`None` with one cluster).
    pub moved: Option<bool>,
}

/// A session of «Моя волна» after `session` over the dataset's open
/// `pool`: [`WAVE_LEN`] songs at each novelty, three at a time, each heard
/// out; then at «Баланс» a fresh wave whose first two songs are skipped
/// early, and the cluster its next block is of.
pub fn wave_session(parts: &Parts, model: &UserModel, suits: &[bool], pool: &[OpenCandidate], session: &Session) -> WaveRun {
    use crate::radio::Outcome;
    use crate::wave::WaveSeed;
    let artist_of: HashMap<String, ArtistId> = parts.new.iter().filter_map(|d| Some((crate::stream_song_key(&d.track), d.artist?))).collect();
    let artist_tracks = |t: &StreamTrack| artist_of.get(&crate::stream_song_key(t)).map(|a| model.of_artist(*a).iter().map(|&i| i as usize).collect()).unwrap_or_default();
    let background = |i: usize| suits.get(i).copied().unwrap_or(false);
    let ok = |_: usize| true;
    let genre_of = genres(parts);
    let genre = |i: usize| genre_of.get(i).copied().flatten();
    let ctx = WaveContext { model, ok: &ok, genre: Some(&genre), background: &background, artist_tracks: &artist_tracks };
    let mut run = WaveRun::default();
    for (k, &novelty) in WAVE_NOVELTIES.iter().enumerate() {
        let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed::plain(novelty)));
        radio.attune(session.clone(), Some(model.weights));
        let (mut played, mut new) = (0usize, 0usize);
        while played < WAVE_LEN {
            let picks = radio.extend_wave(&ctx, pool, 3, 11);
            if picks.is_empty() {
                break;
            }
            for p in picks {
                played += 1;
                new += usize::from(matches!(p.item, Item::Stream(_)));
                radio.feedback(p.item, Outcome::Complete);
            }
        }
        run.new[k] = (played > 0).then(|| new as f32 / played as f32);
        if k == 1 {
            run.clusters = radio.wave_bandit().map_or(0, |b| b.arms.len());
        }
    }
    // Two early skips of a fresh block.
    let mut radio = RadioState::new(RadioSeed::Wave(WaveSeed::plain(crate::wave::NOVELTY_BALANCE)));
    radio.attune(session.clone(), Some(model.weights));
    let first = radio.extend_wave(&ctx, pool, 3, 13);
    let skipped = radio.wave_bandit().and_then(|b| b.current_scene());
    if first.len() >= 2 && radio.wave_bandit().is_some_and(|b| b.arms.len() >= 2) {
        for p in first.iter().take(2) {
            radio.feedback(p.item.clone(), Outcome::skip(8_000));
        }
        radio.forget(&first[2..].iter().map(|p| p.item.clone()).collect::<Vec<_>>());
        let again = radio.extend_wave(&ctx, pool, 3, 13);
        let now = radio.wave_bandit().and_then(|b| b.current_scene());
        if !again.is_empty() {
            run.moved = Some(now != skipped);
        }
    }
    run
}

/// Accumulates [`WaveSessionStats`].
#[derive(Default)]
struct WaveSessions {
    n: usize,
    new: [(f32, usize); 4],
    clusters: usize,
    skips: (usize, usize),
}

impl WaveSessions {
    fn add(&mut self, r: &WaveRun) {
        self.n += 1;
        for (acc, x) in self.new.iter_mut().zip(r.new) {
            if let Some(x) = x {
                acc.0 += x;
                acc.1 += 1;
            }
        }
        self.clusters += r.clusters;
        if let Some(m) = r.moved {
            self.skips.0 += 1;
            self.skips.1 += usize::from(m);
        }
    }

    fn stats(&self) -> WaveSessionStats {
        WaveSessionStats {
            sessions: self.n,
            new: self.new.map(|(s, n)| s / n.max(1) as f32),
            clusters: self.clusters as f32 / self.n.max(1) as f32,
            skips_n: self.skips.0,
            skips_moved: self.skips.1 as f32 / self.skips.0.max(1) as f32,
        }
    }
}

/// Accumulates [`WaveStats`].
#[derive(Default)]
struct Waves {
    n: usize,
    new: f32,
    niche: usize,
    artists: f32,
    per_pick: f32,
    fitting: f32,
}

impl Waves {
    fn add(&mut self, w: &crate::open_world::WaveNext) {
        if w.picks.is_empty() {
            return;
        }
        let k = w.picks.len() as f32;
        self.n += 1;
        self.new += w.picks.iter().filter(|p| p.new).count() as f32 / k;
        self.niche += usize::from(w.niche);
        let artists: HashSet<&str> = w.picks.iter().map(|p| p.artist.as_str()).collect();
        self.artists += artists.len() as f32;
        self.per_pick += artists.len() as f32 / k;
        self.fitting += w.fitting as f32;
    }

    fn stats(&self) -> WaveStats {
        let n = self.n.max(1) as f32;
        WaveStats { lists: self.n, new: self.new / n, niche: self.niche as f32 / n, artists: self.artists / n, artists_per_pick: self.per_pick / n, fitting: self.fitting / n }
    }
}

/// Picks of a wave the evaluation asks.
pub const WAVE_LEN: usize = 20;

/// The main artists of the tracks `model` saw started: whether the track at
/// `i` is by one of them.
fn artist_heard(catalog: &Catalog, model: &UserModel) -> impl Fn(usize) -> bool {
    let played: HashSet<ArtistId> = (0..catalog.len().min(model.tracks.len()))
        .filter(|&i| model.tracks[i].starts > 0)
        .filter_map(|i| catalog.tracks()[i].main_artist())
        .collect();
    let mains: Vec<Option<ArtistId>> = catalog.tracks().iter().map(|t| t.main_artist()).collect();
    move |i: usize| mains.get(i).copied().flatten().is_some_and(|a| played.contains(&a))
}

/// The autoplay after the session `session` (its last track the seed) of
/// `model`: the next `n` as the runtime's autoplay picks them.
pub fn autoplay(parts: &Parts, model: &UserModel, suits: &[bool], session: Session, n: usize) -> Vec<usize> {
    let Some(seed) = session.last().map(|p| p.track) else { return Vec::new() };
    let recs = Recs::new(&parts.catalog, &model.taste, &parts.index).with_model(model).with_background(suits);
    let mut radio = RadioState::new(RadioSeed::Track(seed)).continuing();
    radio.attune(session, Some(model.weights));
    radio.set_no_repeat_ms(3 * HOUR_MS);
    radio.set_explore(Some(0.1));
    radio.set_unheard(0.0);
    let picks = radio.extend(&recs, n, &RadioParams { new_share: 0.0, seed: 1 });
    positions(&parts.catalog, picks.into_iter().map(|p| p.item))
}

/// Picks after two skips are measured by this many.
const AFTER_SKIPS: usize = 3;

/// Two early skips of autoplay's first two `picks` after `session`:
/// autoplay picks [`AFTER_SKIPS`] again (its queue replaced, as the runtime
/// does). Whether the first of them is of another taste profile than both
/// skipped (`None` when one of the three has none), and whether they sound
/// farther from the skipped two (their middle) than the picks it had queued
/// after them. `None` without enough picks or sound.
pub fn two_skips(parts: &Parts, model: &UserModel, suits: &[bool], session: &Session, picks: &[usize]) -> Option<(Option<bool>, bool)> {
    if picks.len() < 2 + AFTER_SKIPS {
        return None;
    }
    let seed = session.last()?.track;
    let ids: Vec<TrackId> = picks.iter().map(|&i| parts.catalog.tracks()[i].id).collect();
    let mut s = session.clone();
    let mut ts = s.last_ts()?;
    for &t in &ids[..2] {
        ts += 20_000;
        s.start(t, ts, EventContext::Radio, Some(EventOrigin::Autoplay));
        s.end(t, Ended::Early);
    }
    let recs = Recs::new(&parts.catalog, &model.taste, &parts.index).with_model(model).with_background(suits);
    let mut radio = RadioState::new(RadioSeed::Track(seed)).continuing();
    radio.attune(s, Some(model.weights));
    radio.set_no_repeat_ms(3 * HOUR_MS);
    radio.set_explore(Some(0.1));
    radio.set_unheard(0.0);
    radio.mark_served(ids[..2].iter().map(|t| Item::Track(*t)));
    let again = positions(&parts.catalog, radio.extend(&recs, AFTER_SKIPS, &RadioParams { new_share: 0.0, seed: 1 }).into_iter().map(|p| p.item));
    let first = *again.first()?;
    let other = match (model.profile_at(first), model.profile_at(picks[0]), model.profile_at(picks[1])) {
        (Some(a), Some(b), Some(c)) => Some(a != b && a != c),
        _ => None,
    };
    let (a, b) = (model.embedded_at(picks[0])?, model.embedded_at(picks[1])?);
    let mid: Vector = std::array::from_fn(|k| (a[k] + b[k]) / 2.0);
    let far = |list: &[usize]| -> Option<f32> {
        let d: Vec<f32> = list.iter().filter_map(|&i| model.embedded_at(i)).map(|v| euclid(v, &mid)).collect();
        (!d.is_empty()).then(|| d.iter().sum::<f32>() / d.len() as f32)
    };
    let away = far(&again)? > far(&picks[2..2 + AFTER_SKIPS])?;
    Some((other, away))
}

/// Evaluates the engine on `parts` (see the module docs).
pub fn evaluate(name: &str, parts: &Parts, opts: &Options) -> Report {
    let events = &parts.input.events;
    let starts = Starts::of(events);
    let mut report = Report { name: name.to_owned(), tracks: parts.catalog.len(), events: events.len(), starts: starts.list.len(), ..Report::default() };
    let (Some(first), Some(last)) = (starts.list.first().map(|s| s.0), starts.list.last().map(|s| s.0)) else {
        return report;
    };
    report.days = (last - first) as f32 / DAY_MS as f32;
    let mid = first + (last - first) / 2;
    let suits = parts.suits();

    // What every list is measured with: the final model's sound, each
    // track's skips over the whole history, the hidden taste.
    let t0 = Instant::now();
    let full = UserModel::build(&parts.catalog, &parts.input, parts.model_input(), last + 1);
    report.build_ms = t0.elapsed().as_secs_f32() * 1e3;
    let t0 = Instant::now();
    let cached = UserModel::build(&parts.catalog, &parts.input, ModelInput { weights: Some(full.weights), ..parts.model_input() }, last + 1);
    report.build_cached_ms = t0.elapsed().as_secs_f32() * 1e3;
    let t0 = Instant::now();
    let homes = home_mixes(parts, &cached, &suits, 1);
    report.mixes_ms = t0.elapsed().as_secs_f32() * 1e3;
    let t0 = Instant::now();
    let _ = autoplay(parts, &cached, &suits, session_before(events, last + 1), opts.autoplay_len);
    report.autoplay_ms = t0.elapsed().as_secs_f32() * 1e3;
    let t0 = Instant::now();
    let graph = crate::graph::SimGraph::build(&parts.catalog, &parts.graph);
    report.graph_ms = t0.elapsed().as_secs_f32() * 1e3;
    // A walk from the three tracks of the graph with most neighbours.
    let mut busy: Vec<(usize, usize)> = (0..parts.catalog.len()).map(|i| (graph.near(i).len(), i)).collect();
    busy.sort_by(|a, b| b.cmp(a));
    let seeds: Vec<(usize, f32)> = busy.iter().take(3).enumerate().map(|(k, x)| (x.1, 0.6f32.powi(k as i32))).collect();
    let t0 = Instant::now();
    let rounds = 50;
    for _ in 0..rounds {
        std::hint::black_box(graph.walk(&seeds));
    }
    report.walk_us = t0.elapsed().as_secs_f32() * 1e6 / rounds as f32;
    drop(graph);
    drop(homes);
    let n = parts.catalog.len();
    let mut early = vec![0u32; n];
    let mut plays = vec![0u32; n];
    for s in &starts.list {
        if let Some(i) = parts.catalog.position(s.1)
            && s.2 != Ended::Open
        {
            plays[i] += 1;
            early[i] += u32::from(s.2 == Ended::Early);
        }
    }
    let truth_pct: Vec<Option<f32>> = if parts.truth.is_empty() {
        vec![None; n]
    } else {
        let mut order: Vec<(f32, usize)> = (0..n).map(|i| (parts.truth.get(&parts.catalog.tracks()[i].id).copied().unwrap_or(0.0), i)).collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut pct = vec![None; n];
        for (r, (_, i)) in order.iter().enumerate() {
            pct[*i] = Some(r as f32 / (n.max(2) - 1) as f32);
        }
        pct
    };
    let ctx = Ctx {
        catalog: &parts.catalog,
        emb: (0..n).map(|i| full.embedded_at(i).copied()).collect(),
        scale: full.scale(),
        skip_rate: (0..n).map(|i| (plays[i] >= 2).then(|| early[i] as f32 / plays[i] as f32)).collect(),
        truth_pct,
    };

    if opts.next {
        report.next = Some(replay(&parts.catalog, &parts.input, parts.model_input()));
    }

    // Autoplay: moments of the second half with a next start in their
    // session, evenly spread.
    let moments: Vec<usize> = (1..starts.list.len())
        .filter(|&k| starts.list[k].0 >= mid && starts.list[k].3 == starts.list[k - 1].3)
        .collect();
    let step = (moments.len() as f32 / opts.autoplay_points.max(1) as f32).max(1.0);
    let mut auto = Lists::default();
    let (mut skips_n, mut skips_other, mut skips_other_n, mut skips_away) = (0usize, 0usize, 0usize, 0usize);
    let mut x = 0.0f32;
    while (x as usize) < moments.len() && opts.autoplay_points > 0 {
        let k = moments[x as usize];
        x += step;
        let (t, next, _, sess) = starts.list[k];
        let model = parts.model_at(t);
        let session = session_before(events, t);
        if session.is_empty() {
            continue;
        }
        let picks = autoplay(parts, &model, &suits, session.clone(), opts.autoplay_len);
        if let Some((other, away)) = two_skips(parts, &model, &suits, &session, &picks) {
            skips_n += 1;
            skips_away += usize::from(away);
            if let Some(o) = other {
                skips_other_n += 1;
                skips_other += usize::from(o);
            }
        }
        let rest: HashSet<TrackId> = starts.list[k..].iter().take_while(|s| s.3 == sess).filter(|s| s.2 != Ended::Early).map(|s| s.1).collect();
        let listened = starts.listened(t, t + opts.window_ms);
        let heard = |i: usize| model.tracks[i].starts > 0;
        let artist = artist_heard(&parts.catalog, &model);
        auto.add(&ctx, &picks, &Truth { listened: &listened, session: Some(&rest), next: Some(next), heard: &heard, artist_heard: &artist });
    }
    report.autoplay = auto.stats(n);
    report.skips2 = (skips_n, skips_other as f32 / skips_other_n.max(1) as f32, skips_away as f32 / skips_n.max(1) as f32);

    // Mixes: mornings of the second half with a week after them.
    let offset = i64::from(parts.input.utc_offset_min) * 60_000;
    let first_day = (mid + offset).div_euclid(DAY_MS) + 1;
    let last_day = (last + offset).div_euclid(DAY_MS) - 1;
    let mut days: Vec<i64> = (first_day..=last_day).collect();
    // A history of a day or two (a cold start): the morning after it.
    if days.is_empty() {
        days.push((last + offset).div_euclid(DAY_MS) + 1);
    }
    let mut mixes = Lists::default();
    let mut mixes_row = Lists::default();
    let mut calm = Waves::default();
    let mut any = Waves::default();
    let mut sessions = WaveSessions::default();
    let mut per_home = 0usize;
    let mut homes_n = 0usize;
    if !days.is_empty() && opts.mix_days > 0 {
        let step = (days.len() as f32 / opts.mix_days as f32).max(1.0);
        let mut x = 0.0f32;
        while (x as usize) < days.len() {
            let day = days[x as usize];
            x += step;
            let t = day * DAY_MS + 9 * HOUR_MS - offset;
            let model = parts.model_at(t);
            let listened = starts.listened(t, t + opts.window_ms);
            // Never played by the mixes' own rule: never started and not
            // loved otherwise (a like, a playlist, an imported play).
            let heard = |i: usize| !never_heard(&model, i);
            let artist = artist_heard(&parts.catalog, &model);
            let (home, row) = home_with_niches(parts, &model, &suits, day as u64);
            let placed = home_new(parts, &model, &home);
            homes_n += 1;
            per_home += home.len();
            for (k, (m, new)) in home.iter().zip(placed).enumerate() {
                let truth = Truth { listened: &listened, session: None, next: None, heard: &heard, artist_heard: &artist };
                mixes.add(&ctx, &m.tracks, &truth);
                mixes.add_new(m.tracks.len(), &new);
                if k < row {
                    mixes_row.add(&ctx, &m.tracks, &truth);
                    mixes_row.add_new(m.tracks.len(), &new);
                }
            }
            let pool = open_pool(parts, &model);
            let session = Session::default();
            calm.add(&wave(parts, &model, &suits, &pool, &WaveFilter { novelty: USER_NEW_SHARE, character: Character::Calm, ..WaveFilter::default() }, &session, WAVE_LEN));
            any.add(&wave(parts, &model, &suits, &pool, &WaveFilter { novelty: USER_NEW_SHARE, ..WaveFilter::default() }, &session, WAVE_LEN));
            sessions.add(&wave_session(parts, &model, &suits, &pool, &session_before(events, t)));
        }
    }
    report.mixes = mixes.stats(n);
    report.mixes_row = mixes_row.stats(n);
    report.mixes_per_home = per_home as f32 / homes_n.max(1) as f32;
    report.mixes.niches /= homes_n.max(1) as f32;
    report.wave_calm = calm.stats();
    report.wave_any = any.stats();
    report.wave_sessions = sessions.stats();
    report
}

fn hits_row(h: &Hits) -> String {
    format!("n {:>5}  hit@10 {:.3}  hit@50 {:.3}  MRR {:.3}  NDCG@10 {:.3}", h.n, h.hit10, h.hit50, h.mrr, h.ndcg10)
}

fn list_row(s: &ListStats) -> String {
    let truth = s.truth.map_or_else(String::new, |t| format!("  truth {t:.3}"));
    format!(
        "lists {:>3} len {:>4.1}  prec@7d {:.3}  sess {:.3}  hit {:.3}  first {:.3}  artists {:.2}  worlds {:.2}  top-world {:.2}  run {:.2}  spread {:.2}  unheard {:.3}  unheard-artists {:.3}  new-service {:.3}  known {:.3}  skips {:.3}  coverage {:.3}{truth}",
        s.lists,
        s.len,
        s.precision,
        s.session,
        s.hit,
        s.first,
        s.artists,
        s.worlds,
        s.world_top,
        s.world_run,
        s.spread,
        s.unheard,
        s.unheard_artists,
        s.new_service,
        s.known,
        s.skips,
        s.coverage
    )
}

impl Report {
    /// The report as lines of text.
    pub fn table(&self) -> String {
        let mut out = format!(
            "== {}: {} tracks, {} events, {} starts over {:.0} days\n",
            self.name, self.tracks, self.events, self.starts, self.days
        );
        if let Some(r) = &self.next {
            for split in ["all", "free", "listened", "listened_free", "listened:second_half"] {
                for model in ["phase1_affinity", "hand", "learned"] {
                    let h = r.get(split, model);
                    if h.n > 0 {
                        out += &format!("  next {split:>22} {model:>16}: {}\n", hits_row(&h));
                    }
                }
            }
            out += &format!("  next: candidates hold the target {:.3}, first ten cover {:.3} of the catalog\n", r.cand_recall, r.coverage);
            let w: Vec<String> = crate::model::FEATURES.iter().zip(r.weights.next.w).map(|(n, w)| format!("{n} {w:.2}")).collect();
            out += &format!("  next: learnt {}\n", w.join(", "));
            if let (Some(a), Some(b)) = (r.skip_auc, r.skip_auc_history) {
                out += &format!("  early skip AUC: model {a:.3}, history {b:.3} ({} starts)\n", r.skip_test);
            }
        }
        out += &format!("  autoplay: {}\n", list_row(&self.autoplay));
        out += &format!(
            "  2 skips:  n {:>3}  next of another profile {:.3}  farther from the skipped {:.3}\n",
            self.skips2.0, self.skips2.1, self.skips2.2
        );
        out += &format!("  mixes:    {}  ({:.1} mixes a home)\n", list_row(&self.mixes), self.mixes_per_home);
        out += &format!("  mix row:  {}\n", list_row(&self.mixes_row));
        let m = &self.mixes;
        out += &format!(
            "  open world: niches {:.1} a home  niche-new {:.3}  niche-artists {:.1}  stuck-on-2 {:.3}  mostly-new {:.3}\n",
            m.niches, m.niche_new, m.niche_artists, m.niche_stuck, m.mostly_new
        );
        for (name, w) in [("calm", &self.wave_calm), ("any", &self.wave_any)] {
            out += &format!(
                "  wave {name:>4}: lists {:>3}  new {:.3}  niche {:.3}  artists {:.1} ({:.2} a pick)  fitting {:.0}\n",
                w.lists, w.new, w.niche, w.artists, w.artists_per_pick, w.fitting
            );
        }
        let w = &self.wave_sessions;
        out += &format!(
            "  wave session: n {:>3}  new familiar {:.3} balance {:.3} more {:.3} only-new {:.3}  clusters {:.1}  2 skips: n {:>3} moved {:.3}\n",
            w.sessions, w.new[0], w.new[1], w.new[2], w.new[3], w.clusters, w.skips_n, w.skips_moved
        );
        out += &format!(
            "  time: build {:.0} ms (learning) / {:.0} ms (cached weights), home mixes {:.0} ms, autoplay of ten {:.0} ms, graph {:.1} ms, walk {:.0} µs\n",
            self.build_ms, self.build_cached_ms, self.mixes_ms, self.autoplay_ms, self.graph_ms, self.walk_us
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic dataset goes through serde and back unchanged, and its
    /// parts rebuild the same catalog and history.
    #[test]
    fn a_dataset_round_trips() {
        let d = synthetic::user(synthetic::Kind::Cold, 1);
        let json = serde_json::to_string(&d).unwrap();
        let back: Dataset = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tracks, d.tracks);
        assert_eq!(back.events, d.events);
        let (a, b) = (d.parts(), back.parts());
        assert_eq!(a.catalog.len(), b.catalog.len());
        assert_eq!(a.index.len(), b.index.len());
        assert_eq!(a.input.events, b.input.events);
        let again = Dataset::of("again", &a.catalog, &a.index, &a.types, &a.input, Some(&a.graph));
        assert_eq!(again.tracks, d.tracks);
        assert_eq!(again.events, d.events);
    }

    /// The session before a moment is the one going on: its starts in
    /// order, how each ended; nothing after a pause.
    #[test]
    fn the_session_before_a_moment() {
        let d = synthetic::user(synthetic::Kind::OneGenre, 2);
        let p = d.parts();
        let starts = Starts::of(&p.input.events);
        let k = (2..starts.list.len()).find(|&k| starts.list[k].3 == starts.list[k - 2].3).unwrap();
        let s = session_before(&p.input.events, starts.list[k].0);
        assert!(s.len() >= 2);
        assert_eq!(s.last().map(|x| x.track), Some(starts.list[k - 1].1));
        let end = p.input.events.iter().map(|e| e.ts).max().unwrap();
        assert!(session_before(&p.input.events, end + 2 * SESSION_GAP_MS).is_empty());
    }

    /// The evaluation of a small synthetic listener runs through and its
    /// numbers are sane: the learnt model finds the next start far better
    /// than chance, lists are made and measured, the picks of a listener
    /// of one kind of music rate high by the hidden taste.
    #[test]
    fn the_evaluation_runs() {
        let d = synthetic::user(synthetic::Kind::OneGenre, 3);
        let p = d.parts();
        let r = evaluate(&d.name, &p, &Options { autoplay_points: 6, mix_days: 2, ..Options::default() });
        let next = r.next.as_ref().unwrap().get("listened", "learned");
        assert!(next.n > 100, "{next:?}");
        assert!(next.hit50 > 0.2, "{next:?}");
        assert!(r.autoplay.lists > 0 && r.mixes.lists > 0, "{}", r.table());
        assert!(r.mixes.truth.unwrap() > 0.6 && r.autoplay.truth.unwrap() > 0.6, "{}", r.table());
        eprintln!("{}", r.table());
    }

    /// A listener who brought a long history of an older taste from
    /// another service and listens to other music here: the home's mixes
    /// are not the import's old songs (a third of them at most over the
    /// row; a mix keeps its quarter, a short one fills to its least length)
    /// and they bring songs never heard.
    #[test]
    fn an_imported_history_does_not_take_the_mixes_over() {
        let d = synthetic::user(synthetic::Kind::Imported, 4);
        assert!(d.events.iter().filter(|e| e.origin == Some(EventOrigin::Import)).count() > 3_000, "a long imported history");
        let p = d.parts();
        let last = p.input.events.iter().filter(|e| e.origin != Some(EventOrigin::Import)).map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let suits = p.suits();
        let home = home_mixes(&p, &model, &suits, 1);
        assert!(!home.is_empty());
        let recs = Recs::new(&p.catalog, &model.taste, &p.index).with_model(&model).with_background(&suits);
        let fit = |i: usize| suits.get(i).copied().unwrap_or(false);
        let b = Builder::new(&recs, &model, &|_| true, &fit, 1, DayPart::Day, Live::default(), MixSettings::default());
        let (mut old, mut never, mut all) = (0usize, 0usize, 0usize);
        for m in home.iter().filter(|m| m.key != MixKey::Liked) {
            old += m.tracks.iter().filter(|&&i| b.stale_at(i)).count();
            never += m.tracks.iter().filter(|&&i| never_heard(&model, i)).count();
            all += m.tracks.len();
        }
        assert!(old * 3 <= all, "{old} old songs of {all}");
        assert!(never > 0, "nothing never heard in {all} songs");
    }

    /// A listener of rap with three calm songs of one artist: «Спокойное»
    /// is a niche, its wave plays mostly new calm songs of several artists
    /// — none of the rap, not the rap its calm songs' radios hold, not the
    /// songs of its own rapper — and the wave of any music stays the
    /// user's share of new.
    #[test]
    fn a_calm_niche_of_three_songs_opens_to_the_world() {
        let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let suits = p.suits();
        let pool = open_pool(&p, &model);
        let calm = wave(&p, &model, &suits, &pool, &WaveFilter { novelty: USER_NEW_SHARE, character: Character::Calm, ..WaveFilter::default() }, &Session::default(), WAVE_LEN);
        assert!(calm.niche, "fitting {} of {} artists", calm.fitting, calm.artists);
        let new: Vec<&crate::open_world::WavePick> = calm.picks.iter().filter(|p| p.new).collect();
        assert!(new.len() as f32 >= 0.7 * calm.picks.len() as f32, "{} new of {}", new.len(), calm.picks.len());
        let title = |p: &crate::open_world::WavePick| match &p.item {
            Item::Stream(r) => d.new.iter().find(|n| n.track.id == *r).map(|n| n.track.title.clone()).unwrap_or_default(),
            Item::Track(_) => String::new(),
        };
        assert!(new.iter().all(|p| title(p).starts_with("Drift")), "{:?}", new.iter().map(|p| title(p)).collect::<Vec<_>>());
        let artists: HashSet<&str> = calm.picks.iter().map(|p| p.artist.as_str()).collect();
        assert!(artists.len() >= 4, "{artists:?}");
        let any = wave(&p, &model, &suits, &pool, &WaveFilter::default(), &Session::default(), WAVE_LEN);
        assert!(!any.niche);
        let share = any.picks.iter().filter(|p| p.new).count() as f32 / any.picks.len() as f32;
        assert!(share <= USER_NEW_SHARE + 0.1, "{share}");
    }

    /// A session of «Моя волна» on a listener of several clusters: its new
    /// music follows the novelty (none at «Знакомое», all it can at «Только
    /// новое»), it weighs several clusters, and two early skips of a block
    /// move it to another.
    /// An open pool for a listener who has none: a new song of a new
    /// artist beside each of `every`-th library tracks, linked to it.
    fn linked_pool(model: &UserModel, every: usize) -> Vec<OpenCandidate> {
        use nikgob_core::streaming::{ArtistRef, RemoteId, Service};
        (0..model.len())
            .step_by(every)
            .enumerate()
            .map(|(k, i)| {
                let mut track = StreamTrack::new(RemoteId::new(Service::Youtube, format!("w{k}")), format!("Wave {k}"));
                track.artists.push(ArtistRef::new(None, format!("New {k}")));
                OpenCandidate { key: crate::stream_song_key(&track), track, vector: None, lane: None, near: vec![i], weight: 0.8 }
            })
            .collect()
    }

    #[test]
    fn a_wave_session_follows_novelty_and_skips() {
        let d = synthetic::user(synthetic::Kind::Omnivore, 5);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let suits = p.suits();
        let pool = linked_pool(&model, 2);
        assert!(pool.len() > 40);
        let r = wave_session(&p, &model, &suits, &pool, &Session::default());
        let new = r.new.map(Option::unwrap);
        assert_eq!(new[0], 0.0, "«Знакомое» plays no new song: {new:?}");
        assert!(new[1] > 0.0 && new[1] < new[2] + 0.05 && new[2] < new[3] + 0.05, "{new:?}");
        assert!(new[3] >= 0.9, "«Только новое»: {new:?}");
        assert!(r.clusters >= 2, "{}", r.clusters);
        assert_eq!(r.moved, Some(true), "two skips change the cluster");
    }

    /// Blocks of one cluster: a wave's picks come four to six of one
    /// cluster at a time, and each library pick is of its cluster and of
    /// the filter's character.
    #[test]
    fn a_wave_plays_blocks_of_its_filter() {
        use crate::radio::Outcome;
        use crate::wave::{BLOCK_MIN, WaveSeed};
        let d = synthetic::user(synthetic::Kind::Omnivore, 6);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let suits = p.suits();
        let background = |i: usize| suits.get(i).copied().unwrap_or(false);
        let ok = |_: usize| true;
        let none = |_: &StreamTrack| Vec::new();
        let ctx = WaveContext { model: &model, ok: &ok, genre: None, background: &background, artist_tracks: &none };
        for character in [Character::Any, Character::Calm] {
            let seed = WaveSeed { filter: WaveFilter { novelty: 0.0, character, scene: crate::open_world::Scene::All }, only: Vec::new(), part: None };
            let mut radio = RadioState::new(RadioSeed::Wave(seed.clone()));
            radio.attune(Session::default(), Some(model.weights));
            let mut runs: Vec<(Option<crate::open_world::Scene>, usize)> = Vec::new();
            let (mut fits, mut played) = (0usize, 0usize);
            for _ in 0..8 {
                let picks = radio.extend_wave(&ctx, &[], 3, 5);
                let scene = radio.wave_bandit().and_then(|b| b.current_scene());
                for pk in picks {
                    let Item::Track(t) = pk.item else { panic!("«Знакомое» is the library") };
                    let i = model.position(t).unwrap();
                    played += 1;
                    let fit = crate::open_world::fits_filter(&ctx, &seed.filter, i);
                    fits += usize::from(fit);
                    if let (true, Some(crate::open_world::Scene::Profile(k))) = (fit, scene) {
                        assert_eq!(model.profile_at(i).map(|j| model.profiles.list[j].key), Some(k), "a pick of its block's cluster");
                    }
                    match runs.last_mut() {
                        Some((s, n)) if *s == scene => *n += 1,
                        _ => runs.push((scene, 1)),
                    }
                    radio.feedback(pk.item, Outcome::Complete);
                }
            }
            // Every track of the filter played before any other (the filter
            // relaxes only when it runs dry).
            let fitting = (0..model.len()).filter(|&i| crate::open_world::fits_filter(&ctx, &seed.filter, i)).count();
            assert_eq!(fits, played.min(fitting), "{character:?}: {fits} of {played} fit, {fitting} fitting");
            // Every run of one cluster but the last is a whole block.
            // (A calm cluster of a few songs may run dry within its block.)
            if character == Character::Any && radio.wave_bandit().is_some_and(|b| b.arms.len() > 1) {
                for (_, n) in &runs[..runs.len() - 1] {
                    assert!(*n >= usize::from(BLOCK_MIN), "{runs:?}");
                }
            }
        }
    }

    /// The calm niche of three songs is a card of the home (below a mix's
    /// least length), in place of the calm side of everything: its three
    /// songs and mostly new calm songs of several artists; its pool is its
    /// three songs (the top-up's seeds).
    #[test]
    fn a_calm_niche_of_three_songs_is_a_card_of_the_home() {
        let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let suits = p.suits();
        let (home, _) = home_with_niches(&p, &model, &suits, 1);
        let calm = home.iter().find(|m| m.key == MixKey::Calm).expect("a calm card");
        assert_eq!(calm.tracks.len(), 3, "its own songs only");
        assert_eq!(calm.pool, calm.tracks);
        let at = home.iter().position(|m| m.key == MixKey::Calm).unwrap();
        let new = home_new(&p, &model, &home);
        assert!(new[at].niche);
        let share = new[at].placed as f32 / (new[at].placed + 3) as f32;
        assert!(share >= 0.7, "{share}");
        assert!(new[at].artists >= 4, "{:?}", new[at]);
        // A card drawn short from a large pool is no niche: the pool tells.
        let mut big = home.iter().find(|m| m.key != MixKey::Calm && m.pool_size.0 >= 25 && m.pool_size.1 >= 4).expect("a large pool").clone();
        big.tracks.truncate(12);
        big.artists.truncate(2);
        assert!(!home_new(&p, &model, &[big])[0].niche);
    }

    /// The ends of novelty hold: «Знакомое» plays no new song even when
    /// the library has few, «Только новое» no library song even when the
    /// pool runs short — fewer songs then.
    #[test]
    fn the_ends_of_novelty_hold() {
        let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let suits = p.suits();
        let pool = open_pool(&p, &model);
        let calm = |novelty: f32| WaveFilter { novelty, character: Character::Calm, ..WaveFilter::default() };
        let familiar = wave(&p, &model, &suits, &pool, &calm(0.0), &Session::default(), WAVE_LEN);
        assert!(!familiar.picks.is_empty() && familiar.picks.iter().all(|x| !x.new), "{:?}", familiar.picks);
        assert!(familiar.picks.len() < WAVE_LEN, "three calm songs only");
        let only = wave(&p, &model, &suits, &pool[..3], &calm(1.0), &Session::default(), WAVE_LEN);
        assert!(only.picks.iter().all(|x| x.new), "{:?}", only.picks);
        assert!(only.picks.len() <= 3);
    }

    /// A song without a preview a calm song's radio holds is no calm song
    /// on that word alone (a new rapper's loud song); the lists of two of
    /// the niche's songs, or its artist's own calm songs, are.
    #[test]
    fn one_radio_is_no_proof_of_a_calm_sound() {
        let d = synthetic::user(synthetic::Kind::NicheCalm, 5);
        let p = d.parts();
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let calm: Vec<usize> = d.new.iter().filter(|n| n.track.title.starts_with("Drift")).flat_map(|n| n.near.iter().filter_map(|t| model.position(*t))).collect::<HashSet<_>>().into_iter().collect();
        assert_eq!(calm.len(), 3);
        let strict = crate::open_world::NicheSound::of(&model, &calm, true);
        assert!(!strict.fits(&model, None, None, &[calm[0]], &[]), "one radio, a new artist");
        assert!(strict.fits(&model, None, None, &[calm[0], calm[1]], &[]), "two of the niche's lists");
        let calm_energy: Vec<f32> = calm.iter().filter_map(|&i| model.energy_at(i)).collect();
        assert!(strict.fits(&model, None, None, &[calm[0]], &calm_energy), "its artist's calm songs");
        let loose = crate::open_world::NicheSound::of(&model, &calm, false);
        assert!(loose.fits(&model, None, None, &[calm[0]], &[]), "a niche of a kind takes the link");
    }

    /// A scene of a family of genres works through the wave: library songs
    /// of the family, new songs linked to them or by its artists.
    #[test]
    fn a_wave_of_a_genre() {
        let mut d = synthetic::user(synthetic::Kind::NicheCalm, 5);
        // The rap's artists tagged hip hop.
        let rap_artists: HashSet<ArtistId> = d.tracks.iter().filter(|t| d.types.by_track.get(&t.id).is_some_and(|s| s.lang == Some(crate::songtype::Lang::Russian))).flat_map(|t| t.artists.clone()).collect();
        for a in &rap_artists {
            d.tags.push((*a, "hip hop".into(), 10.0));
        }
        let p = d.parts();
        let family = genres(&p).into_iter().flatten().next().expect("a family");
        let last = p.input.events.iter().map(|e| e.ts).max().unwrap();
        let model = p.model_at(last + 1);
        let suits = p.suits();
        let pool = open_pool(&p, &model);
        let w = wave(&p, &model, &suits, &pool, &WaveFilter { novelty: 0.5, scene: crate::open_world::Scene::Genre(family), ..WaveFilter::default() }, &Session::default(), WAVE_LEN);
        assert!(w.fitting > 50, "{}", w.fitting);
        assert!(w.picks.iter().any(|x| x.new) && w.picks.iter().any(|x| !x.new));
        let title = |r: &nikgob_core::streaming::RemoteId| d.new.iter().find(|n| n.track.id == *r).map(|n| n.track.title.clone()).unwrap_or_default();
        for x in &w.picks {
            match &x.item {
                Item::Track(t) => assert!(model.position(*t).and_then(|i| model.main_artist_at(i)).is_some_and(|a| rap_artists.contains(&a))),
                Item::Stream(r) => assert!(!title(r).starts_with("Drift"), "{}", title(r)),
            }
        }
    }

    /// Every synthetic listener, measured (`cargo test -p nikgob-recs
    /// --release eval::tests::synthetic_report -- --ignored --nocapture`).
    #[test]
    #[ignore = "a report, minutes"]
    fn synthetic_report() {
        // `NIKGOB_EVAL_QUICK`: the lists only (no replay, no big listener);
        // `NIKGOB_EVAL_NEXT`: the replay only.
        let quick = std::env::var_os("NIKGOB_EVAL_QUICK").is_some();
        let next_only = std::env::var_os("NIKGOB_EVAL_NEXT").is_some();
        for kind in synthetic::Kind::ALL {
            if (quick || next_only) && kind == synthetic::Kind::Big {
                continue;
            }
            let d = synthetic::user(kind, 7);
            let p = d.parts();
            let opts = if kind == synthetic::Kind::Big {
                Options { next: false, autoplay_points: 0, mix_days: 0, ..Options::default() }
            } else {
                Options { next: !quick, autoplay_points: if next_only { 0 } else { 40 }, mix_days: if next_only { 0 } else { 8 }, ..Options::default() }
            };
            println!("{}", evaluate(&d.name, &p, &opts).table());
        }
    }

    /// A real library's dataset (`NIKGOB_RECS_DATASET=path.json`, written
    /// by the runtime from a copy), measured.
    #[test]
    #[ignore = "needs NIKGOB_RECS_DATASET"]
    fn dataset_report() {
        let Some(path) = std::env::var_os("NIKGOB_RECS_DATASET") else {
            eprintln!("NIKGOB_RECS_DATASET is not set");
            return;
        };
        let d: Dataset = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let p = d.parts();
        println!("{}", evaluate(&d.name, &p, &Options::default()).table());
    }
}
