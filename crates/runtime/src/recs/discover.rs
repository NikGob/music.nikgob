//! «Новое для тебя» and «Новые релизы»: a background
//! pass over the services once a day, and after the first sign-in to a
//! service.
//!
//! * **Seeds** ([`nikgob_recs::Recs::discovery_seeds`]): for a seed track,
//!   YouTube Music's radio of its song (Deezer's related list without
//!   YouTube Music); a track the services do not know by id is looked up
//!   by a song search first. For a seed artist, the similar artists of
//!   YouTube Music and of Deezer and the tops of the first two of each
//!   (an artist the library has not matched in a service yet is looked up
//!   by name and remembered, as the discography does). The strongest
//!   songs the services suggest that the library does not have
//!   ([`nikgob_recs::suggest::SUGGESTION_SEEDS`])
//!   seed the pass the same way as tracks. About [`DISCOVER_BUDGET`]
//!   requests a pass.
//! * **New releases**: the discographies of the artists with affinity
//!   ([`nikgob_recs::Recs::release_artists`], at most 30) from Deezer (it
//!   dates albums to the day), each artist at most weekly
//!   ([`RELEASES_TTL_MS`], kept in the library's cache), at most
//!   [`RELEASE_BUDGET`] requests a pass.
//! * **Pace**: one request at a time, [`StreamSchedule::discover_gap`]
//!   apart (2–4 s), in the services' background lanes, never while a
//!   search is typed or answered, never while the periodic network work is
//!   idle (window hidden, nothing playing). A rate limit stops the pass
//!   until the pause ends; it then goes on where it stopped (the pass is
//!   kept in the library's cache, a restart goes on too).
//! * **Results**: [`nikgob_recs::rank_discoveries`] with the library's
//!   songs, artists and dislikes and the «shown» counts (a discovery counts
//!   as shown once for each day it is on the home page; shown three times
//!   and never played, it is left out; the songs the services suggest
//!   vote as the services' own seeds, [`nikgob_recs::suggestion_finds`]),
//!   and
//!   [`nikgob_recs::Recs::new_releases`]. The pass keeps [`DISCOVER_POOL`]
//!   ranked discoveries (the open world's pool: new music that does not run out in a quarter of an hour), each
//!   with the taste profile it fits, spread over the profiles by their
//!   shares; the home's row and the radio take the first [`DISCOVERIES`].
//!   Both kept in the cache
//!   (`discover.found`, with the pass's seeds and finds, so new
//!   suggestions rank them again at once) until the next pass. With only
//!   YouTube Music, only
//!   Deezer, or neither, it does what it can (both catalogues answer
//!   without a login; with neither there is no request at all).
//!
//! [`StreamSchedule::discover_gap`]: crate::StreamSchedule::discover_gap

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{ArtistId, TrackId, UnixMs};
use nikgob_core::streaming::{EntityKind, RemoteId, SearchKind, Service, StreamAlbum, StreamArtist, StreamError, StreamService, StreamTrack};
use nikgob_library::{Library, now_ms};
use nikgob_recs::suggest::SUGGESTION_SEEDS;
use nikgob_recs::discover::{DISCOVERIES, NewScoring, rank_discoveries_upto};
use nikgob_recs::{DiscoverySeeds, NewLog, SeedFinds, SeedRef, Source, artist_key, song_key, stream_song_key, suggestion_finds};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{ARTIST_NAME, Msg, SONG, local_day};
use crate::Worker;
use crate::import::Internal;
use crate::streams::StreamSchedule;

/// The discoveries weighed for a preview ([`Worker::discovery_preview_next`]):
/// those shown and played and as many again past them, and the wave's.
const PREVIEW_LOOK: usize = 120;

/// What the library's songs that found a new song (`near`, catalog
/// positions) say of how sure its blind place is: the share of them of
/// their most common taste profile (`None` with fewer than two), and
/// whether most of them are calm or suit the background (a candidate of a
/// strict filter, where a wrong song costs more).
fn seeds_doubt(snap: &super::Snapshot, near: &[usize]) -> (Option<f32>, bool) {
    let m = &snap.model;
    let agree = (near.len() >= 2).then(|| {
        let mut by: HashMap<Option<usize>, usize> = HashMap::new();
        for &i in near {
            *by.entry(m.profile_at(i)).or_default() += 1;
        }
        by.values().copied().max().unwrap_or(0) as f32 / near.len() as f32
    });
    let quiet = near.iter().filter(|&&i| snap.background.get(i).copied().unwrap_or(false) || m.energy_at(i).is_some_and(|e| e <= nikgob_recs::open_world::CALM_Z)).count();
    (agree, !near.is_empty() && 2 * quiet > near.len())
}

/// Requests of the discovery part of a pass…
pub(crate) const DISCOVER_BUDGET: u32 = 60;
/// …and of the new releases.
pub(crate) const RELEASE_BUDGET: u32 = 60;
/// An artist's discography is asked at most this often.
pub(crate) const RELEASES_TTL_MS: i64 = 7 * 24 * 3600 * 1000;
/// Similar artists whose tops are read, per list.
const TOPS_PER_LIST: usize = 2;
/// The discoveries a pass keeps, best first (the open world's pool): the
/// radio takes the first [`DISCOVERIES`], «Новое для тебя» shows them and
/// more at its row's end ([`HOME_NEW_MORE`] at a time), the cards of «Миксы
/// для тебя» and the waves draw from them all.
pub(crate) const DISCOVER_POOL: usize = 400;
/// «Новое для тебя» gets this many more songs at its row's end.
pub(crate) const HOME_NEW_MORE: usize = 12;
/// New songs heard out lately that seed a pass.
const HEARD_SEEDS: usize = 5;
/// The songs the graph found beside the library's that a pass ranks with
/// its own finds, the strongest.
const GRAPH_DISCOVER: usize = super::graph::GRAPH_FINDS;
/// …each under this many of the library's tracks beside it at most.
const GRAPH_SEEDS: usize = 3;

/// Library cache keys.
const PASS_KEY: &str = "discover.pass";
const FOUND_KEY: &str = "discover.found";
const SHOWN_KEY: &str = "discover.shown";
/// Which services were ever signed in (the first sign-in starts a pass).
const SIGNED_KEY: &str = "discover.signed_in.";
/// «Shown» counts forgotten after this many days without a show.
const SHOWN_KEEP_DAYS: i64 = 90;
const DAY_MS: i64 = 24 * 3600 * 1000;

/// A seed as the pass keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Seed {
    Track(TrackId),
    Artist(ArtistId),
    /// A song the service suggests that the library does not have.
    Service(Service),
}

impl Seed {
    fn engine(self) -> SeedRef {
        match self {
            Self::Track(t) => SeedRef::Track(t),
            Self::Artist(a) => SeedRef::Artist(a),
            Self::Service(s) => SeedRef::Service(s),
        }
    }
}

/// What a name lookup of an artist leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Then {
    Similar,
    Releases,
}

/// One request of a pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum Step {
    /// The songs around a seed track (or a suggested song): YouTube
    /// Music's radio of it, Deezer's related list.
    TrackList { seed: Seed, service: Service, id: String },
    /// A seed track without an id in the service: a song search for it.
    FindTrack { seed: Seed, service: Service, query: String, key: String },
    /// A library artist without an id in the service: an artist search.
    FindArtist { artist: ArtistId, service: Service, name: String, then: Then },
    /// The similar artists of a seed artist.
    Similar { seed: ArtistId, service: Service, id: String },
    /// The top of an artist like a seed artist.
    Top { seed: ArtistId, service: Service, id: String },
    /// The discography of an artist with affinity (new releases).
    Releases { artist: ArtistId, service: Service, id: String },
}

impl Step {
    fn service(&self) -> Service {
        match self {
            Self::TrackList { service, .. }
            | Self::FindTrack { service, .. }
            | Self::FindArtist { service, .. }
            | Self::Similar { service, .. }
            | Self::Top { service, .. }
            | Self::Releases { service, .. } => *service,
        }
    }

    /// Counted against the new releases' budget.
    fn for_releases(&self) -> bool {
        matches!(self, Self::Releases { .. } | Self::FindArtist { then: Then::Releases, .. })
    }
}

/// A pass between its requests (kept in the library's cache).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Pass {
    day: String,
    seed_tracks: Vec<(TrackId, f32)>,
    seed_artists: Vec<(ArtistId, f32)>,
    /// The taste profile of each seed track.
    #[serde(default)]
    seed_profiles: Vec<(TrackId, usize)>,
    steps: VecDeque<Step>,
    finds: Vec<(Seed, Vec<StreamTrack>)>,
    releases: Vec<(ArtistId, Vec<StreamAlbum>)>,
    /// Artists whose tops are asked already (by [`artist_key`]).
    topped: HashSet<String>,
    discover_requests: u32,
    release_requests: u32,
}

/// What the last pass found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Found {
    day: String,
    at: UnixMs,
    tracks: Vec<StreamTrack>,
    releases: Vec<(ArtistId, StreamAlbum)>,
    /// The pass's seeds and finds: new suggestions rank them again.
    #[serde(default)]
    seed_tracks: Vec<(TrackId, f32)>,
    #[serde(default)]
    seed_artists: Vec<(ArtistId, f32)>,
    #[serde(default)]
    seed_profiles: Vec<(TrackId, usize)>,
    /// Where each song found came from (by [`stream_song_key`]).
    #[serde(default)]
    sources: Vec<(String, Source)>,
    /// The taste profile (its key) each of `tracks` fits, when one: the
    /// pool split by cluster.
    #[serde(default)]
    profiles: Vec<Option<u32>>,
    #[serde(default)]
    finds: Vec<(Seed, Vec<StreamTrack>)>,
}

/// The discoveries and new releases the last pass left in the library's
/// cache: how many of each.
pub(crate) fn cached_found(lib: &Library) -> (Vec<StreamTrack>, usize) {
    let found: Option<Found> = lib
        .cache_get(FOUND_KEY)
        .ok()
        .flatten()
        .and_then(|(_, j)| serde_json::from_str(&j).ok());
    found.map_or((Vec::new(), 0), |f| (f.tracks.into_iter().take(DISCOVERIES).collect(), f.releases.len()))
}

/// Every discovery the last pass kept, best first, and where each came
/// from (by [`stream_song_key`]): what the cards of «Миксы для тебя» draw
/// from (the offline evaluation's dataset).
pub(crate) fn cached_pool(lib: &Library) -> (Vec<StreamTrack>, HashMap<String, Source>) {
    let found: Option<Found> = lib.cache_get(FOUND_KEY).ok().flatten().and_then(|(_, j)| serde_json::from_str(&j).ok());
    found.map_or_else(Default::default, |f| (f.tracks, f.sources.into_iter().collect()))
}

/// Every song the last pass found, before ranking (the preview tools).
pub(crate) fn cached_finds(lib: &Library) -> Vec<StreamTrack> {
    let found: Option<Found> = lib.cache_get(FOUND_KEY).ok().flatten().and_then(|(_, j)| serde_json::from_str(&j).ok());
    found.map(|f| f.finds.into_iter().flat_map(|(_, t)| t).collect()).unwrap_or_default()
}

/// One answer of a step.
pub(crate) enum Answer {
    Tracks(Vec<StreamTrack>),
    Artists(Vec<StreamArtist>),
    Albums(Vec<StreamAlbum>),
}

pub(crate) enum DiscoverMsg {
    Answered { step: Step, result: Result<Answer, StreamError> },
    /// A gap or a rate-limit pause is over.
    Wake,
}

#[derive(Default)]
pub(crate) struct DiscoverState {
    loaded: bool,
    pass: Option<Pass>,
    found: Option<Found>,
    /// [`song_key`] → (days shown, last day shown).
    shown: HashMap<String, (u32, String)>,
    /// A request is on its way.
    busy: bool,
    next_at: Option<Instant>,
    /// A pass now, whatever the day («Обновить», a first sign-in).
    force: bool,
    /// The first request of this run waited its gap (no burst at start-up).
    warmed: bool,
    /// The discoveries ranked for previews ([`Worker::discovery_preview_next`]).
    preview_rank: Option<PreviewRank>,
}

/// The discoveries in the order their previews are worth, for the
/// discoveries and snapshot of `key`, with the library tracks of those
/// looked at ([`PREVIEW_LOOK`] at most).
pub(crate) struct PreviewRank {
    key: (usize, Option<RemoteId>, usize),
    order: Vec<StreamTrack>,
    ids: HashMap<RemoteId, Option<TrackId>>,
}

impl DiscoverState {
    /// A request is on its way (the graph's pass waits for it).
    pub(crate) fn is_busy(&self) -> bool {
        self.busy
    }
}

/// What a step asks.
async fn ask(svc: Arc<dyn StreamService>, step: &Step) -> Result<Answer, StreamError> {
    match step {
        Step::TrackList { service, id, .. } => match service {
            Service::Youtube => svc.radio(id).await.map(Answer::Tracks),
            _ => svc.related(id).await.map(Answer::Tracks),
        },
        Step::FindTrack { query, .. } => svc.search_kind(query, SearchKind::Songs).await.map(|s| Answer::Tracks(s.tracks)),
        Step::FindArtist { name, .. } => svc.search_kind(name, SearchKind::Artists).await.map(|s| Answer::Artists(s.artists)),
        Step::Similar { id, .. } => svc.similar_artists(id).await.map(Answer::Artists),
        Step::Top { id, .. } => svc.artist_top(id).await.map(Answer::Tracks),
        Step::Releases { id, .. } => svc.artist(id).await.map(|a| Answer::Albums(a.albums)),
    }
}

/// The discoveries of a pass's `finds` and of the songs the services
/// suggest now, each with the key of the taste profile it fits.
fn ranked(
    snap: &super::Snapshot,
    seeds: &DiscoverySeeds,
    finds: &[(Seed, Vec<StreamTrack>)],
    filter: &nikgob_recs::DiscoveryFilter,
    log: &NewLog,
) -> (Vec<StreamTrack>, Vec<Option<u32>>) {
    let mut all: Vec<SeedFinds> = finds
        .iter()
        .map(|(seed, tracks)| SeedFinds { seed: seed.engine(), tracks: tracks.clone() })
        .collect();
    all.extend(suggestion_finds(&snap.suggested_new));
    let scoring = NewScoring {
        model: &snap.model,
        vectors: &snap.new_vectors,
        log,
        artist_affinity: &snap.artist_affinity,
        now: super::recs_now(),
        types: Some(&snap.types),
    };
    let key = |j: Option<usize>| j.and_then(|j| snap.model.profiles.list.get(j)).map(|p| p.key);
    rank_discoveries_upto(seeds, &all, filter, Some(&scoring), DISCOVER_POOL).into_iter().map(|d| (d.track, key(d.profile))).unzip()
}

/// Where the songs of `finds` came from (by [`stream_song_key`]): a seed
/// track's YouTube Music radio, the similar artists and related lists, a
/// service's own suggestion.
fn sources_of(finds: &[(Seed, Vec<StreamTrack>)]) -> Vec<(String, Source)> {
    let mut out: Vec<(String, Source)> = Vec::new();
    for (seed, tracks) in finds {
        for t in tracks {
            let source = match seed {
                Seed::Service(_) => Source::Service,
                Seed::Track(_) if t.id.service == Service::Youtube => Source::YoutubeRadio,
                _ => Source::Similar,
            };
            let key = stream_song_key(t);
            if !out.iter().any(|(k, _)| *k == key) {
                out.push((key, source));
            }
        }
    }
    out
}

/// The source a step asks.
fn source_of(step: &Step) -> Source {
    match step {
        Step::TrackList { seed: Seed::Service(_), .. } | Step::FindTrack { seed: Seed::Service(_), .. } => Source::Service,
        Step::TrackList { service: Service::Youtube, .. } | Step::FindTrack { service: Service::Youtube, .. } => Source::YoutubeRadio,
        _ => Source::Similar,
    }
}

/// `steps` in the order a pass asks them: each source in turn by the share
/// of the requests its draw gives it ([`NewLog::source_shares`]), so a short
/// budget goes mostly where new music was heard out.
fn by_shares(steps: Vec<Step>, shares: &std::collections::BTreeMap<Source, f32>) -> Vec<Step> {
    let mut queues: Vec<(Source, VecDeque<Step>)> = Vec::new();
    for s in steps {
        let src = source_of(&s);
        match queues.iter_mut().find(|q| q.0 == src) {
            Some(q) => q.1.push_back(s),
            None => queues.push((src, VecDeque::from([s]))),
        }
    }
    let mut asked: std::collections::BTreeMap<Source, f32> = std::collections::BTreeMap::new();
    let mut out = Vec::new();
    loop {
        let total: f32 = asked.values().sum::<f32>() + 1.0;
        let next = queues
            .iter_mut()
            .filter(|q| !q.1.is_empty())
            .max_by(|a, b| {
                let gap = |s: Source| shares.get(&s).copied().unwrap_or(0.1) - asked.get(&s).copied().unwrap_or(0.0) / total;
                gap(a.0).total_cmp(&gap(b.0)).then(b.0.cmp(&a.0))
            });
        let Some(q) = next else { break };
        *asked.entry(q.0).or_default() += 1.0;
        out.extend(q.1.pop_front());
    }
    out
}

/// The songs the graph of similar music found beside the library's
/// ([`super::Snapshot::graph_new`], the best [`GRAPH_DISCOVER`]) as finds of
/// a pass: each under the library's tracks whose radios hold it most (up
/// to [`GRAPH_SEEDS`], so the ranking counts its independent seeds,
/// [`nikgob_recs::discover::multi_seed`]), and those tracks' taste profiles
/// (a find's profile when its preview is not analysed).
fn graph_seed_finds(snap: &super::Snapshot) -> (Vec<(Seed, Vec<StreamTrack>)>, Vec<(TrackId, usize)>) {
    let mut by_seed: Vec<(TrackId, Vec<StreamTrack>)> = Vec::new();
    for f in snap.graph_new.iter().take(GRAPH_DISCOVER) {
        let mut near = f.near.clone();
        near.sort_by(|a, b| b.1.total_cmp(&a.1));
        let mut from: Vec<TrackId> = Vec::new();
        for (t, _) in near {
            if from.len() < GRAPH_SEEDS && !from.contains(&t) {
                from.push(t);
            }
        }
        for t in from {
            match by_seed.iter_mut().find(|s| s.0 == t) {
                Some(s) => s.1.push(f.track.clone()),
                None => by_seed.push((t, vec![f.track.clone()])),
            }
        }
    }
    let profiles = by_seed.iter().filter_map(|(t, _)| Some((*t, snap.model.profile_at(snap.model.position(*t)?)?))).collect();
    (by_seed.into_iter().map(|(t, list)| (Seed::Track(t), list)).collect(), profiles)
}

/// The discography of `artist` as just answered, in place of what the
/// pass read of it before (an older answer kept in the cache).
fn keep_releases(releases: &mut Vec<(ArtistId, Vec<StreamAlbum>)>, artist: ArtistId, albums: Vec<StreamAlbum>) {
    match releases.iter_mut().find(|r| r.0 == artist) {
        Some(r) => r.1 = albums,
        None => releases.push((artist, albums)),
    }
}

fn releases_key(artist: ArtistId) -> String {
    format!("releases.{artist}")
}

impl Worker {
    /// The last pass's finds and the pass under way, from the library's
    /// cache, once a run (before the run's first build: its mixes take
    /// them).
    pub(super) fn discover_load(&mut self) {
        let d = &mut self.recs.discover;
        if d.loaded {
            return;
        }
        d.loaded = true;
        let read = |key: &str| self.lib.cache_get(key).ok().flatten().map(|(_, json)| json);
        let pass: Option<Pass> = read(PASS_KEY).and_then(|j| serde_json::from_str(&j).ok());
        let found: Option<Found> = read(FOUND_KEY).and_then(|j| serde_json::from_str(&j).ok());
        let shown: HashMap<String, (u32, String)> = read(SHOWN_KEY)
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        if let Some(f) = &found {
            self.streaming.learn(&f.tracks);
        }
        let d = &mut self.recs.discover;
        d.pass = pass;
        d.found = found;
        d.shown = shown;
    }

    /// The best discoveries as found (the home filters out the disliked;
    /// «Новое для тебя» shows more of them, [`DiscoverInput`]).
    pub(crate) fn discoveries(&self) -> Vec<StreamTrack> {
        self.recs.discover.found.as_ref().map(|f| f.tracks.iter().take(DISCOVERIES).cloned().collect()).unwrap_or_default()
    }

    /// Every discovery the last pass kept, best first: the cards of «Миксы
    /// для тебя» draw from them all.
    pub(crate) fn discoveries_all(&self) -> Vec<StreamTrack> {
        self.recs.discover.found.as_ref().map(|f| f.tracks.clone()).unwrap_or_default()
    }

    /// The discoveries of the taste profile of `key` (the pool by
    /// cluster), best first.
    #[allow(dead_code)]
    pub(crate) fn discoveries_of_profile(&self, key: u32) -> Vec<StreamTrack> {
        let Some(f) = self.recs.discover.found.as_ref() else { return Vec::new() };
        f.tracks.iter().zip(&f.profiles).filter(|(_, p)| **p == Some(key)).map(|(t, _)| t.clone()).collect()
    }

    /// Where each discovery came from, by [`stream_song_key`].
    pub(crate) fn discovery_sources(&self) -> HashMap<String, Source> {
        self.recs.discover.found.as_ref().map(|f| f.sources.iter().cloned().collect()).unwrap_or_default()
    }

    /// How many discoveries the last pass kept ([`DISCOVER_POOL`] at most).
    pub(crate) fn discoveries_kept(&self) -> usize {
        self.recs.discover.found.as_ref().map_or(0, |f| f.tracks.len())
    }

    /// Why discovery `t` is offered: the seed of the pass that found it
    /// (the most loved of them) and its service, «Новое: нашлось по Ghost
    /// Avenue в YouTube Music».
    pub(crate) fn discovery_why(&self, t: &StreamTrack) -> Option<String> {
        let found = self.recs.discover.found.as_ref()?;
        let key = stream_song_key(t);
        let weight = |s: &Seed| match s {
            Seed::Track(id) => found.seed_tracks.iter().find(|x| x.0 == *id).map_or(0.0, |x| x.1),
            Seed::Artist(a) => found.seed_artists.iter().find(|x| x.0 == *a).map_or(0.0, |x| x.1),
            Seed::Service(_) => -1.0,
        };
        let (seed, _) = found
            .finds
            .iter()
            .filter(|(_, tracks)| tracks.iter().any(|x| x.id == t.id || stream_song_key(x) == key))
            .max_by(|a, b| weight(&a.0).total_cmp(&weight(&b.0)))?;
        let service = t.id.service.display_name();
        Some(match seed {
            Seed::Track(id) => nikgob_recs::explain::new_found(&self.lib.get_track(*id).ok().flatten()?.title, service),
            Seed::Artist(a) => nikgob_recs::explain::new_found(&self.lib.get_artist(*a).ok().flatten()?.name, service),
            Seed::Service(s) => nikgob_recs::explain::suggested(s.display_name()),
        })
    }

    /// The library's songs and artists whose radios, related lists or
    /// tops found each discovery (by [`stream_song_key`]): its links to the
    /// library besides the graph's.
    pub(crate) fn discovery_seeds(&self) -> HashMap<String, (Vec<TrackId>, Vec<ArtistId>)> {
        let mut out: HashMap<String, (Vec<TrackId>, Vec<ArtistId>)> = HashMap::new();
        let Some(found) = self.recs.discover.found.as_ref() else { return out };
        for (seed, tracks) in &found.finds {
            for t in tracks {
                let e = out.entry(stream_song_key(t)).or_default();
                match seed {
                    Seed::Track(id) if !e.0.contains(id) => e.0.push(*id),
                    Seed::Artist(a) if !e.1.contains(a) => e.1.push(*a),
                    _ => {}
                }
            }
        }
        out
    }

    /// Where the discovery of song `key` came from.
    pub(crate) fn discovery_source(&self, key: &str) -> Option<Source> {
        self.recs.discover.found.as_ref()?.sources.iter().find(|(k, _)| k == key).map(|(_, s)| *s)
    }

    /// The next discovery to analyse a preview of (of YouTube Music's alone
    /// when `youtube_only`), none analysed or in `tried` yet, as its track
    /// and song: by what a preview of it is worth
    /// ([`nikgob_recs::previews`]) — near the cut-off of what is shown and
    /// played, found by songs of different taste profiles, by an artist new
    /// to the library, found by calm or background songs — not simply the
    /// first found. The first [`PREVIEW_LOOK`] are weighed. `only`: of
    /// these alone (the songs a free preview was looked for already).
    pub(crate) fn discovery_preview_next(&mut self, tried: &HashSet<TrackId>, youtube_only: bool, only: Option<&HashSet<TrackId>>) -> Option<(TrackId, StreamTrack)> {
        let key = self.preview_rank_key()?;
        // Ranked once for the discoveries and the snapshot (not on every
        // step of the analysis job), each song's library track looked up
        // once as it is reached.
        let mut rank = match self.recs.discover.preview_rank.take() {
            Some(r) if r.key == key => r,
            _ => PreviewRank { key, order: self.preview_ranking(), ids: HashMap::new() },
        };
        let index = self.analysis.index().clone();
        let mut chosen = None;
        for t in &rank.order {
            if youtube_only && t.id.service != Service::Youtube {
                continue;
            }
            let id = *rank.ids.entry(t.id.clone()).or_insert_with(|| self.lib.track_by_remote(&t.id).ok().flatten());
            match id {
                Some(id) => {
                    if tried.contains(&id) || only.is_some_and(|o| !o.contains(&id)) || index.read().contains(id) {
                        continue;
                    }
                    chosen = Some((Some(id), t.clone()));
                }
                // No track yet: never tried, never looked for free; a row
                // only for the one chosen.
                None if only.is_some() => continue,
                None => chosen = Some((None, t.clone())),
            }
            break;
        }
        let picked = match chosen {
            Some((Some(id), t)) => Some((id, t)),
            Some((None, t)) => {
                let id = self.upsert_streams(std::slice::from_ref(&t)).first().copied();
                if let Some(id) = id {
                    rank.ids.insert(t.id.clone(), Some(id));
                }
                id.map(|id| (id, t))
            }
            None => None,
        };
        self.recs.discover.preview_rank = Some(rank);
        picked
    }

    /// What [`PreviewRank`] is ranked for: the discoveries (their count and
    /// first song) and the snapshot.
    fn preview_rank_key(&self) -> Option<(usize, Option<RemoteId>, usize)> {
        let found = self.recs.discover.found.as_ref()?;
        let snap = self.snapshot().map_or(0, |s| Arc::as_ptr(&s) as usize);
        Some((found.tracks.len(), found.tracks.first().map(|t| t.id.clone()), snap))
    }

    /// The first [`PREVIEW_LOOK`] discoveries by what a preview of each is
    /// worth ([`nikgob_recs::previews`]).
    fn preview_ranking(&self) -> Vec<StreamTrack> {
        use nikgob_recs::previews::{PreviewCandidate, preview_order};
        let Some(found) = self.recs.discover.found.as_ref() else { return Vec::new() };
        let snap = self.snapshot();
        let seeds = self.discovery_seeds();
        let list: Vec<&StreamTrack> = found.tracks.iter().take(PREVIEW_LOOK).collect();
        let cands: Vec<PreviewCandidate> = list
            .iter()
            .enumerate()
            .map(|(rank, t)| {
                let (agree, strict) = snap.as_ref().map_or((None, false), |s| {
                    let near: Vec<usize> = seeds.get(&stream_song_key(t)).map(|(ts, _)| ts.iter().filter_map(|id| s.model.position(*id)).collect()).unwrap_or_default();
                    seeds_doubt(s, &near)
                });
                let known_artist = snap.as_ref().is_some_and(|s| t.artists.first().is_some_and(|a| s.artist_keys.contains_key(&artist_key(&a.name))));
                PreviewCandidate { rank, cut: DISCOVERIES, soon: false, agree, known_artist, strict }
            })
            .collect();
        preview_order(&cands).into_iter().map(|k| list[k].clone()).collect()
    }


    /// YouTube Music and Deezer, the ones that can be asked now.
    fn discover_services(&self) -> (bool, bool) {
        (self.service(Service::Youtube).is_some(), self.service(Service::Deezer).is_some())
    }

    /// `RefreshMix(Discover)`: a pass now.
    pub(crate) fn discover_now(&mut self) {
        self.recs.discover.force = true;
        self.discover_tick();
    }

    /// A service signed in for the first time: a pass now (its catalogue
    /// may know more).
    pub(crate) fn discover_signed_in(&mut self, service: Service) {
        let key = format!("{SIGNED_KEY}{}", service.as_str());
        if self.lib.cache_get(&key).ok().flatten().is_some() {
            return;
        }
        let _ = self.lib.cache_put(&key, now_ms(), "1");
        log::info!("discover: first sign-in to {service}, looking for new music");
        self.discover_now();
    }

    /// Every tick (and after each answer): a new pass when it is due, the
    /// next request when it is time.
    pub(crate) fn discover_tick(&mut self) {
        // Not while a game is in front (the tick goes on when it is over).
        if crate::quiet::busy() {
            return;
        }
        self.discover_load();
        let (yt, dz) = self.discover_services();
        if !yt && !dz {
            return;
        }
        if self.recs.discover.pass.is_none() {
            let (day, _) = local_day();
            let every = self.streaming.schedule.discover_every.as_millis() as i64;
            let d = &self.recs.discover;
            let due = d.force || d.found.as_ref().is_none_or(|f| f.day != day || now_ms() - f.at >= every);
            if !due {
                return;
            }
            if !self.discover_plan(day) {
                return;
            }
        }
        self.discover_step();
    }

    /// Plans a pass from the current snapshot; false without one.
    fn discover_plan(&mut self, day: String) -> bool {
        let Some(snap) = self.snapshot() else { return false };
        let (yt, dz) = self.discover_services();
        self.recs.discover.force = false;
        let recs = snap.recs();
        let DiscoverySeeds { mut tracks, artists, profiles } = recs.discovery_seeds();
        // A seed whose finds keep flopping seeds less (the log of new music).
        let trusts = self.recs.newlog.log.seed_trusts();
        for t in &mut tracks {
            t.1 *= trusts.get(&t.0).copied().unwrap_or(1.0);
        }
        tracks.sort_by(|a, b| b.1.total_cmp(&a.1));
        let release_artists = recs.release_artists();
        let mut pass = Pass {
            day,
            seed_tracks: tracks.clone(),
            seed_artists: artists.clone(),
            seed_profiles: profiles,
            ..Pass::default()
        };
        let mut track_steps = Vec::new();
        for (t, _) in &tracks {
            let refs = self.lib.stream_refs(*t).unwrap_or_default();
            let id_on = |s: Service| refs.iter().find(|r| r.id.service == s).map(|r| r.id.id.clone());
            let seed = Seed::Track(*t);
            let step = match (id_on(Service::Youtube), id_on(Service::Deezer)) {
                (Some(id), _) if yt => Some(Step::TrackList { seed, service: Service::Youtube, id }),
                (_, Some(id)) if dz => Some(Step::TrackList { seed, service: Service::Deezer, id }),
                _ => self.lib.get_track(*t).ok().flatten().map(|row| {
                    let artist = row.artist_credit.display();
                    Step::FindTrack {
                        seed,
                        service: if yt { Service::Youtube } else { Service::Deezer },
                        query: format!("{artist} {}", row.title),
                        key: song_key(&row.title, &artist),
                    }
                }),
            };
            track_steps.extend(step);
        }
        // The songs the services suggest that the library lacks: every
        // other track step, so a short budget still sees them.
        let suggested: Vec<Step> = snap
            .suggested_new
            .iter()
            .take(SUGGESTION_SEEDS)
            .filter_map(|(t, _)| {
                let seed = Seed::Service(t.id.service);
                match t.id.service {
                    Service::Youtube if yt => Some(Step::TrackList { seed, service: Service::Youtube, id: t.id.id.clone() }),
                    Service::Deezer if dz => Some(Step::TrackList { seed, service: Service::Deezer, id: t.id.id.clone() }),
                    _ if yt || dz => Some(Step::FindTrack {
                        seed,
                        service: if yt { Service::Youtube } else { Service::Deezer },
                        query: format!("{} {}", t.artist_line(), t.title),
                        key: stream_song_key(t),
                    }),
                    _ => None,
                }
            })
            .collect();
        let mut library_steps = std::mem::take(&mut track_steps).into_iter();
        for s in suggested {
            track_steps.push(s);
            track_steps.extend(library_steps.next());
        }
        track_steps.extend(library_steps);
        let mut artist_steps = Vec::new();
        for (a, _) in &artists {
            let Some(name) = self.lib.get_artist(*a).ok().flatten().map(|a| a.name) else { continue };
            // A cut list's tail an older import took for an artist.
            if nikgob_core::streaming::more_artists_note(&name) {
                continue;
            }
            pass.topped.insert(artist_key(&name));
            let known = self.lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default();
            for (on, service) in [(yt, Service::Youtube), (dz, Service::Deezer)] {
                if !on {
                    continue;
                }
                artist_steps.push(match known.iter().find(|r| r.service == service) {
                    Some(r) => Step::Similar { seed: *a, service, id: r.id.clone() },
                    None => Step::FindArtist { artist: *a, service, name: name.clone(), then: Then::Similar },
                });
            }
        }
        // New songs heard out lately seed the pass too.
        for t in self.recs.newlog.log.heard_out(HEARD_SEEDS) {
            if yt
                && let Some(r) = self.lib.stream_refs(t).unwrap_or_default().into_iter().find(|r| r.id.service == Service::Youtube)
            {
                track_steps.push(Step::TrackList { seed: Seed::Track(t), service: Service::Youtube, id: r.id.id });
            }
        }
        // Tracks and artists in turn, so a short budget still sees both…
        let (mut ti, mut ai) = (track_steps.into_iter(), artist_steps.into_iter());
        let mut steps = Vec::new();
        loop {
            let (t, a) = (ti.next(), ai.next());
            if t.is_none() && a.is_none() {
                break;
            }
            steps.extend(t);
            steps.extend(a);
        }
        // …and every source by the share its draw gives it.
        let shares = self.recs.newlog.log.source_shares(&mut nikgob_recs::rng::Rng::new(fastrand::u64(..)));
        log::info!("discover: the sources' shares {shares:?}");
        pass.steps.extend(by_shares(steps, &shares));
        // New releases: Deezer dates albums to the day.
        if dz {
            let now = now_ms();
            for a in release_artists {
                // «и ещё 1 исполнитель» (a cut list's tail an older import
                // took for an artist) would find somebody's single.
                if self.lib.get_artist(a).ok().flatten().is_some_and(|x| nikgob_core::streaming::more_artists_note(&x.name)) {
                    continue;
                }
                let cached: Option<(UnixMs, Vec<StreamAlbum>)> = self
                    .lib
                    .cache_get(&releases_key(a))
                    .ok()
                    .flatten()
                    .and_then(|(at, json)| Some((at, serde_json::from_str(&json).ok()?)));
                // What was read before stands until the answer replaces
                // it: an artist whose request the budget leaves for the
                // next pass, or whose request fails, keeps the releases
                // already known.
                if let Some((at, albums)) = cached {
                    pass.releases.push((a, albums));
                    if now - at < RELEASES_TTL_MS {
                        continue;
                    }
                }
                let known = self.lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default();
                match known.iter().find(|r| r.service == Service::Deezer) {
                    Some(r) => pass.steps.push_back(Step::Releases { artist: a, service: Service::Deezer, id: r.id.clone() }),
                    None => {
                        if let Some(name) = self.lib.get_artist(a).ok().flatten().map(|a| a.name) {
                            pass.steps.push_back(Step::FindArtist {
                                artist: a,
                                service: Service::Deezer,
                                name,
                                then: Then::Releases,
                            });
                        }
                    }
                }
            }
        }
        log::info!(
            "discover: a pass with {} seed tracks, {} seed artists, {} requests planned",
            tracks.len(),
            artists.len(),
            pass.steps.len()
        );
        self.recs.discover.pass = Some(pass);
        self.discover_save_pass();
        true
    }

    fn discover_save_pass(&self) {
        let json = match &self.recs.discover.pass {
            Some(p) => serde_json::to_string(p).unwrap_or_default(),
            None => String::new(),
        };
        if let Err(e) = self.lib.cache_put(PASS_KEY, now_ms(), &json) {
            log::warn!("discover: keeping the pass: {e}");
        }
    }

    /// Waits `d`, then looks again.
    fn discover_wait(&mut self, d: Duration) {
        self.recs.discover.next_at = Some(Instant::now() + d);
        self.streaming.spawn_internal(async move {
            tokio::time::sleep(d).await;
            Internal::Recs(Msg::Discover(DiscoverMsg::Wake))
        });
    }

    /// Sends the next request of the pass when everything allows it.
    fn discover_step(&mut self) {
        let d = &self.recs.discover;
        if d.busy || d.next_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        // The user's search and the window's idleness go first.
        if self.streaming.search.active() || self.streams_idle() {
            return;
        }
        if !std::mem::replace(&mut self.recs.discover.warmed, true) {
            let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
            self.discover_wait(gap);
            return;
        }
        loop {
            let Some(pass) = self.recs.discover.pass.as_ref() else { return };
            let Some(step) = pass.steps.front().cloned() else {
                self.discover_finish();
                return;
            };
            let over = if step.for_releases() {
                pass.release_requests >= RELEASE_BUDGET
            } else {
                pass.discover_requests >= DISCOVER_BUDGET
            };
            let service = step.service();
            let svc = self.service(service);
            // Its pause, or the listener's turn on YouTube Music.
            let paused = if svc.is_some() && !over { self.background_wait(service) } else { None };
            let Some(pass) = self.recs.discover.pass.as_mut() else { return };
            let Some(svc) = svc.filter(|_| !over) else {
                pass.steps.pop_front();
                continue;
            };
            if let Some(left) = paused {
                log::info!("discover: {service} asks to wait {} s", left.as_secs());
                self.discover_wait(left + crate::streams::wake_jitter());
                return;
            }
            pass.steps.pop_front();
            if step.for_releases() {
                pass.release_requests += 1;
            } else {
                pass.discover_requests += 1;
            }
            self.recs.discover.busy = true;
            self.streaming.spawn_internal(async move {
                let result = ask(svc, &step).await;
                Internal::Recs(Msg::Discover(DiscoverMsg::Answered { step, result }))
            });
            return;
        }
    }

    pub(crate) fn discover_msg(&mut self, m: DiscoverMsg) {
        let (step, result) = match m {
            DiscoverMsg::Answered { step, result } => (step, result),
            DiscoverMsg::Wake => {
                // `next_at` has passed (the sleep began after it was set).
                self.discover_tick();
                return;
            }
        };
        self.recs.discover.busy = false;
        let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
        let Some(pass) = self.recs.discover.pass.as_mut() else { return };
        match result {
            Err(StreamError::RateLimited { retry_after }) => {
                // Not a failure: the same request after the pause.
                if step.for_releases() {
                    pass.release_requests = pass.release_requests.saturating_sub(1);
                } else {
                    pass.discover_requests = pass.discover_requests.saturating_sub(1);
                }
                pass.steps.push_front(step.clone());
                let wait = retry_after
                    .or_else(|| self.pause_left(step.service()))
                    .unwrap_or(self.streaming.schedule.rate_limit_unknown);
                log::info!("discover: rate limited by {}, again in {} s", step.service(), wait.as_secs());
                self.discover_save_pass();
                self.discover_wait(wait + crate::streams::wake_jitter());
                return;
            }
            Err(e) => log::info!("discover: {step:?}: {e}"),
            Ok(answer) => self.discover_answer(step, answer),
        }
        self.discover_save_pass();
        self.discover_wait(gap);
    }

    fn discover_answer(&mut self, step: Step, answer: Answer) {
        // YouTube Music's radio of a library track is the graph's too.
        if let (Step::TrackList { seed: Seed::Track(t), service: Service::Youtube, id }, Answer::Tracks(tracks)) = (&step, &answer)
            && self.state.read().prefs.graph_services
        {
            self.graph_from_discovery(*t, id, tracks);
        }
        let mut learnt: Vec<(ArtistId, RemoteId)> = Vec::new();
        let Some(pass) = self.recs.discover.pass.as_mut() else { return };
        match (step, answer) {
            (Step::TrackList { seed, .. }, Answer::Tracks(t)) => pass.finds.push((seed, t)),
            (Step::FindTrack { seed, service, key, .. }, Answer::Tracks(hits)) => {
                if let Some(hit) = hits.iter().find(|h| stream_song_key(h) == key) {
                    pass.steps.push_front(Step::TrackList { seed, service, id: hit.id.id.clone() });
                }
            }
            (Step::FindArtist { artist, service, name, then }, Answer::Artists(hits)) => {
                let key = artist_key(&name);
                if let Some(hit) = hits.iter().find(|h| artist_key(&h.name) == key) {
                    learnt.push((artist, hit.id.clone()));
                    let id = hit.id.id.clone();
                    pass.steps.push_front(match then {
                        Then::Similar => Step::Similar { seed: artist, service, id },
                        Then::Releases => Step::Releases { artist, service, id },
                    });
                }
            }
            (Step::Similar { seed, service, .. }, Answer::Artists(similar)) => {
                // Unknown artists first: their songs are the new ones.
                let mut similar: Vec<&StreamArtist> = similar.iter().collect();
                let snap = self.recs.snap.clone();
                similar.sort_by_key(|a| snap.as_ref().is_some_and(|s| s.filter.known_artists.contains(&artist_key(&a.name))));
                let mut tops = 0;
                for a in similar {
                    if tops == TOPS_PER_LIST {
                        break;
                    }
                    if pass.topped.insert(artist_key(&a.name)) {
                        pass.steps.push_front(Step::Top { seed, service, id: a.id.id.clone() });
                        tops += 1;
                    }
                }
            }
            (Step::Top { seed, .. }, Answer::Tracks(t)) => pass.finds.push((Seed::Artist(seed), t)),
            (Step::Releases { artist, .. }, Answer::Albums(albums)) => {
                if let Ok(json) = serde_json::to_string(&albums) {
                    let _ = self.lib.cache_put(&releases_key(artist), now_ms(), &json);
                }
                keep_releases(&mut pass.releases, artist, albums);
            }
            (step, _) => log::warn!("discover: an unexpected answer to {step:?}"),
        }
        for (artist, remote) in learnt {
            log::info!("artist {artist} is {remote}");
            let _ = self.lib.set_external_id(EntityKind::Artist, &remote, *artist.as_uuid());
        }
    }

    /// The pass is done: ranked, kept and shown.
    fn discover_finish(&mut self) {
        let Some(pass) = self.recs.discover.pass.take() else { return };
        let Some(snap) = self.snapshot() else {
            self.recs.discover.pass = Some(pass);
            return;
        };
        let mut filter = snap.filter.clone();
        filter.shown = self
            .recs
            .discover
            .shown
            .iter()
            .map(|(k, (n, _))| (k.clone(), (*n, snap.played_songs.contains(k))))
            .collect();
        // The songs the graph found beside the library's join the pass's
        // finds, each under the tracks whose radios hold it most.
        let (graph_finds, graph_profiles) = graph_seed_finds(&snap);
        let mut finds = pass.finds;
        finds.extend(graph_finds);
        let mut seed_profiles = pass.seed_profiles;
        for p in graph_profiles {
            if !seed_profiles.iter().any(|s| s.0 == p.0) {
                seed_profiles.push(p);
            }
        }
        let seeds = DiscoverySeeds {
            tracks: pass.seed_tracks.clone(),
            artists: pass.seed_artists.clone(),
            profiles: seed_profiles.clone(),
        };
        self.new_log_load();
        let (tracks, profiles) = ranked(&snap, &seeds, &finds, &filter, &self.recs.newlog.log);
        let releases = snap
            .recs()
            .new_releases(&pass.releases, &snap.known_albums)
            .into_iter()
            .map(|r| (r.artist, r.album))
            .collect();
        log::info!(
            "discover: {} discoveries from {} + {} requests",
            tracks.len(),
            pass.discover_requests,
            pass.release_requests
        );
        let found = Found {
            day: pass.day,
            at: now_ms(),
            tracks,
            releases,
            seed_tracks: pass.seed_tracks,
            seed_artists: pass.seed_artists,
            seed_profiles,
            sources: sources_of(&finds),
            profiles,
            finds,
        };
        self.streaming.learn(&found.tracks);
        if let Ok(json) = serde_json::to_string(&found) {
            let _ = self.lib.cache_put(FOUND_KEY, found.at, &json);
        }
        self.recs.discover.found = Some(found);
        self.discover_save_pass();
        self.publish_recs_home();
        // The daily mix of a new day takes the new discoveries.
        self.radio_snapshot_ready();
        self.plus_ready();
        // The mixes take them now: «С новым» of the cards, «Твой микс» and
        // the daylist of the day (their day's caches know the finds were
        // older).
        self.recs_dirty(Duration::ZERO);
    }

    /// When the finds the mixes take were made (0: none yet).
    pub(super) fn discoveries_at(&self) -> UnixMs {
        self.recs.discover.found.as_ref().map_or(0, |f| f.at)
    }

    /// The services' suggestions changed: the last pass's finds are ranked
    /// again with their votes (the snapshot has the new ones by now).
    pub(crate) fn discover_rerank(&mut self) {
        self.discover_load();
        let Some(snap) = self.snapshot() else { return };
        let Some(found) = self.recs.discover.found.as_mut() else { return };
        if found.finds.is_empty() && snap.suggested_new.is_empty() {
            return;
        }
        let mut filter = snap.filter.clone();
        filter.shown = self
            .recs
            .discover
            .shown
            .iter()
            .map(|(k, (n, _))| (k.clone(), (*n, snap.played_songs.contains(k))))
            .collect();
        let seeds = DiscoverySeeds {
            tracks: found.seed_tracks.clone(),
            artists: found.seed_artists.clone(),
            profiles: found.seed_profiles.clone(),
        };
        let (tracks, profiles) = ranked(&snap, &seeds, &found.finds, &filter, &self.recs.newlog.log);
        if tracks == found.tracks {
            return;
        }
        log::info!("discover: {} discoveries with the services' suggestions", tracks.len());
        found.tracks = tracks;
        found.profiles = profiles;
        let found = found.clone();
        self.streaming.learn(&found.tracks);
        if let Ok(json) = serde_json::to_string(&found) {
            let _ = self.lib.cache_put(FOUND_KEY, found.at, &json);
        }
        self.publish_recs_home();
    }

    /// The home's «Новое для тебя» and «Новые релизы»: the last pass's,
    /// without what is disliked or in the library since; each discovery
    /// on the home counts as shown once a day.
    pub(crate) fn discover_rows(&mut self) -> (Vec<v::StreamTrackRow>, Vec<v::StreamAlbumCard>) {
        let (rows, albums, shown) = self.discover_input().rows(&self.lib);
        self.discover_shown(&shown);
        (rows, albums)
    }

    /// What [`Self::discover_rows`] makes its rows of, taken here so the
    /// rows can be made on another thread ([`DiscoverInput::rows`]).
    pub(crate) fn discover_input(&mut self) -> DiscoverInput {
        self.discover_load();
        let Some(found) = self.recs.discover.found.as_ref() else { return DiscoverInput::default() };
        let releases = found
            .releases
            .iter()
            .map(|(artist, album)| (*artist, album.clone(), self.album_theirs(album)))
            .collect();
        let limit = DISCOVERIES + self.home_build.more.new;
        DiscoverInput { tracks: found.tracks.clone(), releases, limit }
    }

    /// The discoveries of `keys` (by [`stream_song_key`]) were on the home:
    /// each counts as shown once a day.
    pub(crate) fn discover_shown(&mut self, keys: &[String]) {
        let (day, _) = local_day();
        let mut counted = false;
        for key in keys {
            let e = self.recs.discover.shown.entry(key.clone()).or_insert((0, String::new()));
            if e.1 != day {
                e.0 += 1;
                e.1.clone_from(&day);
                counted = true;
            }
        }
        if counted {
            let today = nikgob_core::streaming::date_ms(&day).unwrap_or(0);
            self.recs.discover.shown.retain(|_, (_, last)| {
                nikgob_core::streaming::date_ms(last).is_none_or(|d| today - d < SHOWN_KEEP_DAYS * DAY_MS)
            });
            if let Ok(json) = serde_json::to_string(&self.recs.discover.shown) {
                let _ = self.lib.cache_put(SHOWN_KEY, now_ms(), &json);
            }
        }
    }
}

/// The home's «Новое для тебя» and «Новые релизы» before their rows are
/// made: the last pass's discoveries, and its new releases with their
/// artist and the service's track count ([`Worker::album_theirs`]).
#[derive(Default)]
pub(crate) struct DiscoverInput {
    tracks: Vec<StreamTrack>,
    releases: Vec<(ArtistId, StreamAlbum, u32)>,
    /// The songs the row shows at most (more once it was scrolled to its
    /// end).
    limit: usize,
}

impl DiscoverInput {
    /// The rows, without what is disliked (it reads only the library), and
    /// the keys of the discoveries among them ([`Worker::discover_shown`]).
    pub(crate) fn rows(&self, lib: &Library) -> (Vec<v::StreamTrackRow>, Vec<v::StreamAlbumCard>, Vec<String>) {
        let disliked = |kind: &str, id: &str| lib.is_rec_disliked(kind, id).unwrap_or(false);
        let tracks: Vec<&StreamTrack> = self
            .tracks
            .iter()
            .filter(|t| {
                !disliked(SONG, &stream_song_key(t))
                    && !t.artists.iter().any(|a| disliked(ARTIST_NAME, &artist_key(&a.name)))
            })
            .take(self.limit)
            .collect();
        let shown = tracks.iter().map(|t| stream_song_key(t)).collect();
        let rows = tracks
            .into_iter()
            .map(|t| crate::streams::stream_row_of(lib, t, lib.track_for_stream(t).ok().flatten()))
            .collect();
        // Nor what was found for «и ещё 1 исполнитель», a cut list's tail an
        // older import took for an artist (somebody else's single).
        let note = |a: &ArtistId| lib.get_artist(*a).ok().flatten().is_some_and(|x| nikgob_core::streaming::more_artists_note(&x.name));
        let albums = self
            .releases
            .iter()
            .filter(|(a, _, _)| !disliked("artist", &a.to_string()) && !note(a))
            .map(|(_, album, theirs)| crate::streams::album_card_of(lib, album, *theirs))
            .collect();
        (rows, albums, shown)
    }
}

impl Worker {
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An artist's answer takes the place of what the pass read of them
    /// from the cache; another artist's is added.
    #[test]
    fn an_answer_replaces_the_older_releases() {
        let (a, b) = (ArtistId::new(), ArtistId::new());
        let mut releases: Vec<(ArtistId, Vec<StreamAlbum>)> = vec![(a, Vec::new())];
        let album = |id: &str| StreamAlbum {
            id: RemoteId::new(Service::Deezer, id),
            title: id.to_owned(),
            artists: Vec::new(),
            year: None,
            release_date: None,
            release_type: Default::default(),
            cover_url: None,
            total_tracks: None,
            tracks: Vec::new(),
        };
        keep_releases(&mut releases, a, vec![album("1"), album("2")]);
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].1.len(), 2);
        keep_releases(&mut releases, b, vec![album("3")]);
        assert_eq!(releases.iter().map(|r| (r.0, r.1.len())).collect::<Vec<_>>(), [(a, 2), (b, 1)]);
    }
}
