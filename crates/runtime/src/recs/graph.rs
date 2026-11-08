//! The graph of similar music in the runtime (docs/recs-graph.md): the
//! background pass that asks the sources about the library's artists and
//! tracks and keeps their answers in the library, and the resolution of
//! what was kept to the catalog at every build.
//!
//! * **The pass**: every source in a lane of its own ([`Lane`]), all lanes
//!   at once, one request at a time in each, its gap apart
//!   ([`StreamSchedule::graph_gap_youtube`] and the rest) on top of the
//!   clients' own limits; never while a search is typed or answered or
//!   while the window is idle, and the services' lanes not while a
//!   discovery request is on its way. A lane plans
//!   [`StreamSchedule::graph_chunk`] requests at a time and the next ones as
//!   soon as those are asked, so the pass goes through the whole library;
//!   once nothing is due it looks again [`StreamSchedule::graph_every`]
//!   later. A rate limit waits its pause (at least a second, doubled while
//!   it goes on) and asks the same again.
//!   Artists first (one answer covers all their tracks: the most of a cold
//!   start), the loved and played first, then the tracks the same way.
//!   What each source said is kept for [`TTL_MS`] (MusicBrainz's tags
//!   [`TAGS_TTL_MS`]), an error is asked again after [`RETRY_MS`].
//! * **The sources**: YouTube Music's radio of a track (a song search first
//!   when the track has no id there) and its similar artists, Deezer's
//!   similar artists and an artist's top tracks («Спрашивать YouTube Music
//!   и Deezer»); MusicBrainz's artist with its tags (the id ListenBrainz
//!   needs), ListenBrainz's similar artists, recordings and an artist's
//!   popular recordings («Спрашивать ListenBrainz и MusicBrainz», with the
//!   online metadata of the import; the popular recordings only with the
//!   user's token); Last.fm's similar tracks and artists with the user's
//!   key.
//! * **Popularity without ListenBrainz** (no token, or it was refused this
//!   run): Deezer's open top of an artist in ListenBrainz's lane when the
//!   Deezer lane does not ask its top already, and Last.fm's top tracks
//!   with the user's key; one log line a run says so.
//! * **Resolution** ([`resolve`]): an entry is the library's track or artist
//!   by the service's id, by its MusicBrainz id, else by its song or artist
//!   key; what the library lacks is left out of the graph, and the similar
//!   artists it lacks vote for the new music of their artists
//!   ([`Resolved::votes`]).
//!
//! [`StreamSchedule::graph_gap_youtube`]: crate::StreamSchedule::graph_gap_youtube
//! [`StreamSchedule::graph_chunk`]: crate::StreamSchedule::graph_chunk
//! [`StreamSchedule::graph_every`]: crate::StreamSchedule::graph_every

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{ArtistId, TrackId};
use nikgob_core::streaming::{EntityKind, SearchKind, Service, StreamArtist, StreamError, StreamService, StreamTrack};
use nikgob_library::{GraphEntry, GraphFetch, GraphRow, GraphTag, Library, now_ms};
use nikgob_metadata::{Metadata, MetadataError};
use nikgob_recs::graph::{Edge, GraphInput, GraphSource, rank_weight};
use nikgob_recs::{artist_key, song_key, stream_song_key};

use super::Msg;
use crate::Worker;
use crate::import::Internal;
use crate::streams::StreamSchedule;

/// What a source said stands this long…
pub(crate) const TTL_MS: i64 = 30 * 86_400_000;
/// …MusicBrainz's tags this long…
pub(crate) const TAGS_TTL_MS: i64 = 90 * 86_400_000;
/// …and an error is asked again after this.
pub(crate) const RETRY_MS: i64 = 86_400_000;
/// Entries kept since the last build that make a build due.
const FRESH_BUILD: usize = 60;

/// A rate limit that says to ask again at once waits this long at least.
const MIN_PAUSE: Duration = Duration::from_secs(1);

/// The sources as the library keeps them.
const S_MUSICBRAINZ: &str = "musicbrainz";
/// ListenBrainz's popular recordings of an artist…
const S_POPULARITY: &str = "popularity";
/// …Deezer's top tracks of an artist (no key)…
const S_DEEZER_TOP: &str = "deezer_top";
/// …and Last.fm's (with the user's key): all are the popularity of the
/// library's prior.
const S_LASTFM_TOP: &str = "lastfm_top";
/// Tracks asked of Deezer's open top of an artist.
const DZ_TOP_LIMIT: u32 = 25;

/// One request of the pass.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ask {
    /// YouTube Music's radio of a track by its id there.
    Radio { track: TrackId, id: String },
    /// A track without an id in YouTube Music: a song search first.
    FindSong { track: TrackId, query: String, key: String },
    /// A service's similar artists of an artist by its id there.
    Artists { artist: ArtistId, service: Service, id: String },
    /// An artist without an id in the service: an artist search first.
    FindArtist { artist: ArtistId, service: Service, name: String },
    /// Deezer's top tracks of an artist by its id there.
    DzTop { artist: ArtistId, id: String },
    /// Deezer's top tracks of an artist from its open API (no account; an
    /// artist search first without the id): the popularity when
    /// ListenBrainz gives none and the Deezer lane is off.
    DzTopOpen { artist: ArtistId, name: String, id: Option<u64> },
    /// MusicBrainz's artist and its tags (by its id, else by name).
    Tags { artist: ArtistId, name: String, mbid: Option<String> },
    LbArtists { artist: ArtistId, mbid: String },
    LbTop { artist: ArtistId, mbid: String },
    LbRecordings { track: TrackId, mbid: String },
    FmTrack { track: TrackId, artist: String, title: String },
    FmArtist { artist: ArtistId, name: String },
    /// Last.fm's top tracks of an artist (the popularity without
    /// ListenBrainz).
    FmTop { artist: ArtistId, name: String },
}

impl Ask {
    /// What it is about and which source it asks, as the library keeps them.
    pub(crate) fn subject(&self) -> (&'static str, String, &'static str) {
        let t = |id: &TrackId| id.to_string();
        let a = |id: &ArtistId| id.to_string();
        match self {
            Self::Radio { track, .. } | Self::FindSong { track, .. } => ("track", t(track), GraphSource::YoutubeRadio.as_str()),
            Self::Artists { artist, service, .. } | Self::FindArtist { artist, service, .. } => {
                ("artist", a(artist), if *service == Service::Youtube { GraphSource::YoutubeArtists.as_str() } else { GraphSource::DeezerArtists.as_str() })
            }
            Self::DzTop { artist, .. } | Self::DzTopOpen { artist, .. } => ("artist", a(artist), S_DEEZER_TOP),
            Self::Tags { artist, .. } => ("artist", a(artist), S_MUSICBRAINZ),
            Self::LbArtists { artist, .. } => ("artist", a(artist), GraphSource::ListenBrainz.as_str()),
            Self::LbTop { artist, .. } => ("artist", a(artist), S_POPULARITY),
            Self::LbRecordings { track, .. } => ("track", t(track), GraphSource::ListenBrainz.as_str()),
            Self::FmTrack { track, .. } => ("track", t(track), GraphSource::LastFm.as_str()),
            Self::FmArtist { artist, .. } => ("artist", a(artist), GraphSource::LastFm.as_str()),
            Self::FmTop { artist, .. } => ("artist", a(artist), S_LASTFM_TOP),
        }
    }

    /// The service it asks, if one of the streaming services.
    fn service(&self) -> Option<Service> {
        match self {
            Self::Radio { .. } | Self::FindSong { .. } => Some(Service::Youtube),
            Self::Artists { service, .. } | Self::FindArtist { service, .. } => Some(*service),
            Self::DzTop { .. } => Some(Service::Deezer),
            _ => None,
        }
    }

    /// The lane it waits in.
    pub(crate) fn lane(&self) -> Lane {
        match self {
            Self::Tags { .. } => Lane::MusicBrainz,
            // Deezer's open top stands in for ListenBrainz's popularity.
            Self::LbArtists { .. } | Self::LbTop { .. } | Self::LbRecordings { .. } | Self::DzTopOpen { .. } => Lane::ListenBrainz,
            Self::FmTrack { .. } | Self::FmArtist { .. } | Self::FmTop { .. } => Lane::LastFm,
            _ if self.service() == Some(Service::Deezer) => Lane::Deezer,
            _ => Lane::Youtube,
        }
    }
}

/// A source of the pass with its own pace: each asks one request at a
/// time, all of them at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    Youtube,
    Deezer,
    MusicBrainz,
    ListenBrainz,
    LastFm,
}

impl Lane {
    pub(crate) const ALL: [Lane; 5] = [Lane::Youtube, Lane::Deezer, Lane::MusicBrainz, Lane::ListenBrainz, Lane::LastFm];

    fn index(self) -> usize {
        self as usize
    }

    /// One of the streaming services (they also answer discovery).
    fn is_service(self) -> bool {
        matches!(self, Lane::Youtube | Lane::Deezer)
    }
}

/// How long a lane waits after its `n`-th rate limit in a row: what the
/// source said (else `unknown`), at least [`MIN_PAUSE`], doubled at each
/// one after the first and at most `cap` (unless the source itself said
/// longer).
pub(crate) fn backoff(said: Option<Duration>, unknown: Duration, cap: Duration, n: u32) -> Duration {
    let base = said.unwrap_or(unknown).max(MIN_PAUSE);
    base.saturating_mul(1 << n.saturating_sub(1).min(10)).min(cap.max(base))
}

/// An answer of a request.
pub(crate) enum Answer {
    Tracks(Vec<StreamTrack>),
    Artists(Vec<StreamArtist>),
    /// Entries of an open source.
    Entries(Vec<GraphEntry>),
    /// MusicBrainz's artist: its id and tags (none: not found).
    Tags(Option<(String, Vec<(String, f32)>)>),
}

/// Why a request gave nothing.
pub(crate) enum Failed {
    /// Ask again after the pause.
    RateLimited(Option<Duration>),
    /// The source wants an account or a key now (401, 403): it is not
    /// asked again this run.
    Closed(String),
    Other(String),
}

impl From<StreamError> for Failed {
    fn from(e: StreamError) -> Self {
        match e {
            StreamError::RateLimited { retry_after } => Self::RateLimited(retry_after),
            e => Self::Other(e.to_string()),
        }
    }
}

impl From<MetadataError> for Failed {
    fn from(e: MetadataError) -> Self {
        match e {
            MetadataError::RateLimited(d) => Self::RateLimited(d),
            e @ MetadataError::Status { status: 401 | 403, .. } => Self::Closed(e.to_string()),
            e => Self::Other(e.to_string()),
        }
    }
}

pub(crate) enum GraphMsg {
    Answered { ask: Ask, result: Result<Answer, Failed> },
    /// A gap or a pause is over.
    Wake,
}

/// A lane of the pass.
#[derive(Default)]
struct LaneState {
    queue: VecDeque<Ask>,
    busy: bool,
    next_at: Option<Instant>,
    /// When it last found nothing due (none this run yet).
    covered_at: Option<Instant>,
    /// Rate limits in a row.
    limited: u32,
}

#[derive(Default)]
pub(crate) struct GraphState {
    lanes: [LaneState; 5],
    /// Requests sent this run.
    requests: u32,
    /// A pass is under way (for its log line and its last build).
    running: bool,
    /// The first request of this run waited its gap.
    warmed: bool,
    /// Entries kept since the last build asked for…
    fresh: usize,
    /// …and when that was.
    built_at: Option<Instant>,
    /// Sources that wanted an account or a key this run ([`Failed::Closed`]).
    closed: Vec<&'static str>,
    /// The log said where the popularity comes from without ListenBrainz.
    popularity_told: bool,
    /// The online metadata to ask (tests set it; else the import's).
    pub(crate) metadata: Option<Metadata>,
}

impl GraphState {
    fn lane(&mut self, l: Lane) -> &mut LaneState {
        &mut self.lanes[l.index()]
    }
}

/// One entry of a stream track.
fn track_entry(t: &StreamTrack) -> GraphEntry {
    let artist = t.artists.first().map(|a| a.name.clone()).unwrap_or_default();
    GraphEntry { kind: "track".into(), name: t.title.clone(), artist, remote_id: Some(t.id.id.clone()) }
}

fn artist_entry(a: &StreamArtist) -> GraphEntry {
    GraphEntry { kind: "artist".into(), name: a.name.clone(), artist: String::new(), remote_id: Some(a.id.id.clone()) }
}

/// What a request asks of its source.
async fn ask(svc: Option<Arc<dyn StreamService>>, meta: Option<Metadata>, a: Ask) -> Result<Answer, Failed> {
    let svc = || svc.clone().ok_or_else(|| Failed::Other("no service".into()));
    let meta = || meta.clone().ok_or_else(|| Failed::Other("no online metadata".into()));
    let entries = |v: Vec<nikgob_metadata::Similar>, kind: &str| {
        Answer::Entries(v.into_iter().map(|s| GraphEntry { kind: kind.into(), name: s.name, artist: s.artist, remote_id: Some(s.mbid) }).collect())
    };
    let fm = |v: Vec<nikgob_metadata::LastFmSimilar>, kind: &str| {
        Answer::Entries(v.into_iter().map(|s| GraphEntry { kind: kind.into(), name: s.name, artist: s.artist, remote_id: s.mbid }).collect())
    };
    Ok(match a {
        Ask::Radio { id, .. } => Answer::Tracks(svc()?.radio(&id).await?),
        Ask::DzTop { id, .. } => Answer::Tracks(svc()?.artist_top(&id).await?),
        Ask::FindSong { query, .. } => Answer::Tracks(svc()?.search_kind(&query, SearchKind::Songs).await?.tracks),
        Ask::Artists { id, .. } => Answer::Artists(svc()?.similar_artists(&id).await?),
        Ask::FindArtist { name, .. } => Answer::Artists(svc()?.search_kind(&name, SearchKind::Artists).await?.artists),
        Ask::Tags { name, mbid, .. } => {
            let m = meta()?;
            let found = match mbid {
                Some(id) => Some(m.musicbrainz.artist_tags(&id).await?),
                None => m.musicbrainz.search_artist_tags(&name).await?,
            };
            Answer::Tags(found.map(|t| (t.id, t.tags.into_iter().map(|(n, c)| (n, c as f32)).collect())))
        }
        Ask::LbArtists { mbid, .. } => entries(meta()?.listenbrainz.similar_artists(&mbid).await?, "artist"),
        Ask::LbTop { mbid, .. } => entries(meta()?.listenbrainz.top_recordings(&mbid).await?, "track"),
        Ask::LbRecordings { mbid, .. } => entries(meta()?.listenbrainz.similar_recordings(&mbid).await?, "track"),
        Ask::FmTrack { artist, title, .. } => {
            let m = meta()?;
            let fm_client = m.lastfm.ok_or_else(|| Failed::Other("no Last.fm key".into()))?;
            fm(fm_client.similar_tracks(&artist, &title).await?, "track")
        }
        Ask::FmArtist { name, .. } => {
            let m = meta()?;
            let fm_client = m.lastfm.ok_or_else(|| Failed::Other("no Last.fm key".into()))?;
            fm(fm_client.similar_artists(&name).await?, "artist")
        }
        Ask::FmTop { name, .. } => {
            let m = meta()?;
            let fm_client = m.lastfm.ok_or_else(|| Failed::Other("no Last.fm key".into()))?;
            fm(fm_client.top_tracks(&name).await?, "track")
        }
        Ask::DzTopOpen { name, id, .. } => {
            let m = meta()?;
            let id = match id {
                Some(id) => id,
                None => {
                    // The artist of that name Deezer knows best.
                    let key = artist_key(&name);
                    let hits = m.deezer.search_artist(&name, 10).await?;
                    match hits.into_iter().filter(|h| artist_key(&h.name) == key).max_by_key(|h| h.nb_fan.unwrap_or(0)) {
                        Some(h) => h.id,
                        None => return Ok(Answer::Entries(Vec::new())),
                    }
                }
            };
            let top = m.deezer.artist_top(id, DZ_TOP_LIMIT).await?;
            Answer::Entries(
                top.into_iter()
                    .map(|t| GraphEntry { kind: "track".into(), artist: t.artist_name().to_owned(), name: t.title, remote_id: Some(t.id.to_string()) })
                    .collect(),
            )
        }
    })
}

/// Whether what `source` said about a subject still stands at `now`.
fn fresh(fetches: &HashMap<(String, String, String), GraphFetch>, kind: &str, id: &str, source: &str, now: i64) -> bool {
    fetches.get(&(kind.to_owned(), id.to_owned(), source.to_owned())).is_some_and(|f| {
        let ttl = match f.status.as_str() {
            "error" => RETRY_MS,
            _ if source == S_MUSICBRAINZ => TAGS_TTL_MS,
            _ => TTL_MS,
        };
        now - f.fetched_at < ttl
    })
}

/// What the library's graph resolves to over its tracks and artists.
#[derive(Debug, Default)]
pub(crate) struct Resolved {
    pub input: GraphInput,
    /// Similar artists the library lacks, by [`artist_key`]: the library's
    /// artists that point at them, by the edge's weight.
    pub votes: HashMap<String, Vec<(ArtistId, f32)>>,
}

/// The library's ways to name its own tracks and artists.
#[derive(Default)]
pub(crate) struct Names {
    /// (service, kind, remote id) → local id.
    pub remote: HashMap<(String, String, String), String>,
    /// A MusicBrainz id → (kind, local id).
    pub mbid: HashMap<String, (String, String)>,
    /// [`song_key`] → track.
    pub songs: HashMap<String, TrackId>,
    /// [`artist_key`] → artist.
    pub artists: HashMap<String, ArtistId>,
}

impl Names {
    fn track(&self, source: &str, e: &GraphEntry) -> Option<TrackId> {
        let by_remote = |service: &str| e.remote_id.as_ref().and_then(|r| self.remote.get(&(service.to_owned(), "track".to_owned(), r.clone())));
        let by_mbid = || e.remote_id.as_ref().and_then(|r| self.mbid.get(r)).filter(|(k, _)| k == "track").map(|(_, id)| id);
        let id = match source {
            "youtube_radio" => by_remote("youtube"),
            "listenbrainz" | "lastfm" | S_POPULARITY | S_LASTFM_TOP => by_mbid(),
            S_DEEZER_TOP => by_remote("deezer"),
            _ => None,
        };
        id.and_then(|s| s.parse().ok()).or_else(|| self.songs.get(&song_key(&e.name, &e.artist)).copied())
    }

    fn artist(&self, source: &str, e: &GraphEntry) -> Option<ArtistId> {
        let by_remote = |service: &str| e.remote_id.as_ref().and_then(|r| self.remote.get(&(service.to_owned(), "artist".to_owned(), r.clone())));
        let by_mbid = || e.remote_id.as_ref().and_then(|r| self.mbid.get(r)).filter(|(k, _)| k == "artist").map(|(_, id)| id);
        let id = match source {
            "youtube_artists" => by_remote("youtube"),
            "deezer_artists" => by_remote("deezer"),
            "listenbrainz" | "lastfm" => by_mbid(),
            _ => None,
        };
        id.and_then(|s| s.parse().ok()).or_else(|| self.artists.get(&artist_key(&e.name)).copied())
    }
}

/// The graph of `rows` and `tags` over the library `names` knows.
pub(crate) fn resolve(rows: &[GraphRow], tags: &[GraphTag], names: &Names) -> Resolved {
    let mut out = Resolved::default();
    for r in rows {
        let source = GraphSource::parse(&r.source);
        match (r.subject_kind.as_str(), r.entry.kind.as_str()) {
            ("track", "track") => {
                let (Some(source), Ok(from)) = (source, r.subject_id.parse::<TrackId>()) else { continue };
                if let Some(to) = names.track(&r.source, &r.entry) {
                    out.input.tracks.push(Edge { from, to, rank: r.rank, of: r.total, source });
                }
            }
            ("artist", "artist") => {
                let (Some(source), Ok(from)) = (source, r.subject_id.parse::<ArtistId>()) else { continue };
                match names.artist(&r.source, &r.entry) {
                    Some(to) => out.input.artists.push(Edge { from, to, rank: r.rank, of: r.total, source }),
                    None => out.votes.entry(artist_key(&r.entry.name)).or_default().push((from, rank_weight(r.rank, r.total) * source.trust())),
                }
            }
            ("artist", "track") if [S_POPULARITY, S_DEEZER_TOP, S_LASTFM_TOP].contains(&r.source.as_str()) => {
                if let Some(t) = names.track(&r.source, &r.entry) {
                    out.input.popular.push((t, r.rank, r.total));
                }
            }
            _ => {}
        }
    }
    for t in tags {
        if t.subject_kind == "artist"
            && let Ok(a) = t.subject_id.parse::<ArtistId>()
        {
            out.input.tags.push((a, t.tag.clone(), t.weight));
        }
    }
    out.votes.retain(|k, _| !k.is_empty());
    out
}

/// The library's graph resolved over its `rows` (the snapshot's).
pub(crate) fn read(lib: &Library, rows: &[nikgob_library::RecTrack], artist_keys: &HashMap<String, ArtistId>) -> Resolved {
    let edges = lib.graph_edges().unwrap_or_default();
    let tags = lib.graph_tags().unwrap_or_default();
    if edges.is_empty() && tags.is_empty() {
        return Resolved::default();
    }
    let mut names = Names { artists: artist_keys.clone(), ..Names::default() };
    for (service, kind, remote, local) in lib.graph_remote_ids().unwrap_or_default() {
        names.remote.insert((service, kind, remote), local);
    }
    for (kind, id, mbid) in lib.graph_mbids().unwrap_or_default() {
        names.mbid.insert(mbid, (kind, id));
    }
    for f in lib.graph_fetches().unwrap_or_default() {
        if f.source == S_MUSICBRAINZ
            && let Some(m) = f.remote_id
        {
            names.mbid.entry(m).or_insert((f.subject_kind, f.subject_id));
        }
    }
    for r in rows.iter().filter(|r| r.in_library) {
        names.songs.entry(song_key(&r.title, &r.artist)).or_insert(r.id);
    }
    let resolved = resolve(&edges, &tags, &names);
    log::info!(
        "graph: {} track edges, {} artist edges, {} tags, {} popular, {} artists beyond the library",
        resolved.input.tracks.len(),
        resolved.input.artists.len(),
        resolved.input.tags.len(),
        resolved.input.popular.len(),
        resolved.votes.len()
    );
    resolved
}

impl Worker {
    /// The online metadata the pass asks: the test's, else the import's.
    fn graph_metadata(&self) -> Option<Metadata> {
        self.recs.graph.metadata.clone().or_else(|| self.import.as_ref().and_then(|s| s.metadata()))
    }

    /// A pass now (a source was switched on).
    pub(crate) fn graph_now(&mut self) {
        for l in &mut self.recs.graph.lanes {
            l.covered_at = None;
        }
        self.graph_tick();
    }

    /// Whether `lane` may ask at all under `prefs`.
    fn graph_lane_on(&self, lane: Lane, prefs: &nikgob_view::Prefs, meta: Option<&Metadata>) -> bool {
        match lane {
            Lane::Youtube => prefs.graph_services && self.service(Service::Youtube).is_some(),
            Lane::Deezer => prefs.graph_services && self.service(Service::Deezer).is_some(),
            Lane::MusicBrainz | Lane::ListenBrainz => prefs.graph_open_data && meta.is_some(),
            Lane::LastFm => meta.is_some_and(|m| m.lastfm.is_some()),
        }
    }

    /// Whether ListenBrainz's popularity may be asked: with the user's token,
    /// unless it was refused this run.
    fn graph_popularity_open(&self, meta: Option<&Metadata>) -> bool {
        meta.is_some_and(|m| m.listenbrainz.has_token()) && !self.recs.graph.closed.contains(&S_POPULARITY)
    }

    /// The ListenBrainz token was set or taken away: its popularity may be
    /// asked again this run, and the lanes plan at once.
    pub(crate) fn graph_popularity_reopened(&mut self) {
        self.recs.graph.closed.retain(|s| *s != S_POPULARITY);
        self.recs.graph.popularity_told = false;
        self.graph_now();
    }

    /// Every tick: the next requests of the lanes that ran dry, the next
    /// request of each lane when it is time.
    pub(crate) fn graph_tick(&mut self) {
        let prefs = self.state.read().prefs.clone();
        let off = !prefs.graph_use || !(prefs.graph_services || prefs.graph_open_data);
        let meta = self.graph_metadata();
        let every = self.streaming.schedule.graph_every;
        let mut want = [false; 5];
        for lane in Lane::ALL {
            let on = !off && self.graph_lane_on(lane, &prefs, meta.as_ref());
            let l = self.recs.graph.lane(lane);
            if !on {
                l.queue.clear();
                continue;
            }
            want[lane.index()] = l.queue.is_empty() && !l.busy && l.covered_at.is_none_or(|t| t.elapsed() >= every);
        }
        if want.contains(&true) {
            let popularity = self.graph_popularity_open(meta.as_ref());
            let dz_lane = self.graph_lane_on(Lane::Deezer, &prefs, meta.as_ref());
            let lastfm = self.graph_lane_on(Lane::LastFm, &prefs, meta.as_ref());
            self.graph_plan(want, popularity, dz_lane, lastfm);
        }
        let g = &self.recs.graph;
        if g.running && g.lanes.iter().all(|l| l.queue.is_empty() && !l.busy) {
            self.graph_done();
            return;
        }
        if !off {
            self.graph_step(meta.as_ref());
        }
    }

    /// Plans the next requests of the lanes `want` names from the snapshot;
    /// a lane with nothing due is covered. Without ListenBrainz's
    /// `popularity` an artist's popularity comes from Deezer's open top
    /// (unless the Deezer lane asks its own top: `dz_lane`) and from
    /// Last.fm's (with its key: `lastfm`).
    fn graph_plan(&mut self, want: [bool; 5], popularity: bool, dz_lane: bool, lastfm: bool) {
        // Said once a run, when the lane that would ask it plans.
        if !popularity && want[Lane::ListenBrainz.index()] && !std::mem::replace(&mut self.recs.graph.popularity_told, true) {
            let why = if self.recs.graph.closed.contains(&S_POPULARITY) { "ListenBrainz refused its popularity" } else { "no ListenBrainz token" };
            let deezer = if dz_lane { "Deezer's top" } else { "Deezer's open top" };
            let from = if lastfm { format!("{deezer} and Last.fm's") } else { deezer.to_owned() };
            log::info!("graph: {why}, the popularity comes from {from} this run");
        }
        let Some(snap) = self.snapshot() else { return };
        let now = now_ms();
        let fetches: HashMap<(String, String, String), GraphFetch> = self
            .lib
            .graph_fetches()
            .unwrap_or_default()
            .into_iter()
            .map(|f| ((f.subject_kind.clone(), f.subject_id.clone(), f.source.clone()), f))
            .collect();
        let mut mbids: HashMap<String, String> = HashMap::new();
        for (kind, id, mbid) in self.lib.graph_mbids().unwrap_or_default() {
            mbids.insert(format!("{kind}:{id}"), mbid);
        }
        for f in fetches.values() {
            if f.source == S_MUSICBRAINZ
                && let Some(m) = &f.remote_id
            {
                mbids.entry(format!("artist:{}", f.subject_id)).or_insert_with(|| m.clone());
            }
        }
        let chunk = self.streaming.schedule.graph_chunk.max(1);
        let closed = self.recs.graph.closed.clone();
        let model = &snap.model;
        let cat = &snap.catalog;
        let stands = |kind: &str, id: &str, source: &str| fresh(&fetches, kind, id, source, now);
        let due = |kind: &str, id: &str, source: &'static str| !closed.contains(&source) && !stands(kind, id, source);
        let mut plans: [Vec<Ask>; 5] = Default::default();
        let room = |plans: &[Vec<Ask>; 5], l: Lane| want[l.index()] && plans[l.index()].len() < chunk;
        let full = |plans: &[Vec<Ask>; 5]| Lane::ALL.iter().all(|l| !room(plans, *l));
        // Artists: the loved first, then the ones with most tracks.
        let mut artists: Vec<(ArtistId, f32, usize)> =
            cat.artists().map(|a| (a, model.taste.artist_affinity(a), cat.of_artist(a).len())).collect();
        artists.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        for (a, _, _) in artists {
            if full(&plans) {
                break;
            }
            let sid = a.to_string();
            let yt = room(&plans, Lane::Youtube) && due("artist", &sid, GraphSource::YoutubeArtists.as_str());
            let dz = room(&plans, Lane::Deezer) && due("artist", &sid, GraphSource::DeezerArtists.as_str());
            let dz_top = room(&plans, Lane::Deezer) && due("artist", &sid, S_DEEZER_TOP);
            let mb = room(&plans, Lane::MusicBrainz) && due("artist", &sid, S_MUSICBRAINZ);
            // ListenBrainz knows the artist by the id MusicBrainz gave.
            let mbid = mbids.get(&format!("artist:{sid}")).filter(|_| room(&plans, Lane::ListenBrainz) && stands("artist", &sid, S_MUSICBRAINZ));
            let lb = mbid.is_some() && due("artist", &sid, GraphSource::ListenBrainz.as_str());
            let lb_top = popularity && mbid.is_some() && due("artist", &sid, S_POPULARITY);
            // Without it: Deezer's open top (unless the Deezer lane asks its
            // own) and Last.fm's.
            let dz_open = !popularity && !dz_lane && room(&plans, Lane::ListenBrainz) && due("artist", &sid, S_DEEZER_TOP);
            let fm_top = !popularity && room(&plans, Lane::LastFm) && due("artist", &sid, S_LASTFM_TOP);
            let fm = room(&plans, Lane::LastFm) && due("artist", &sid, GraphSource::LastFm.as_str());
            if !(yt || dz || dz_top || mb || lb || lb_top || dz_open || fm_top || fm) {
                continue;
            }
            let Some(name) = self.lib.get_artist(a).ok().flatten().map(|x| x.name) else { continue };
            let known = if yt || dz || dz_top || dz_open { self.lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default() } else { Vec::new() };
            let id_in = |service: Service| known.iter().find(|r| r.service == service).map(|r| r.id.clone());
            for (on, service, lane) in [(yt, Service::Youtube, Lane::Youtube), (dz, Service::Deezer, Lane::Deezer)] {
                if on {
                    plans[lane.index()].push(match id_in(service) {
                        Some(id) => Ask::Artists { artist: a, service, id },
                        None => Ask::FindArtist { artist: a, service, name: name.clone() },
                    });
                }
            }
            // Without the id yet, the artist search asks the top after it.
            if dz_top
                && let Some(id) = id_in(Service::Deezer)
            {
                plans[Lane::Deezer.index()].push(Ask::DzTop { artist: a, id });
            }
            if mb {
                plans[Lane::MusicBrainz.index()].push(Ask::Tags { artist: a, name: name.clone(), mbid: mbids.get(&format!("artist:{sid}")).cloned() });
            }
            if let Some(m) = mbid {
                if lb {
                    plans[Lane::ListenBrainz.index()].push(Ask::LbArtists { artist: a, mbid: m.clone() });
                }
                if lb_top {
                    plans[Lane::ListenBrainz.index()].push(Ask::LbTop { artist: a, mbid: m.clone() });
                }
            }
            if dz_open {
                let id = id_in(Service::Deezer).and_then(|s| s.parse().ok());
                plans[Lane::ListenBrainz.index()].push(Ask::DzTopOpen { artist: a, name: name.clone(), id });
            }
            if fm {
                plans[Lane::LastFm.index()].push(Ask::FmArtist { artist: a, name: name.clone() });
            }
            if fm_top {
                plans[Lane::LastFm.index()].push(Ask::FmTop { artist: a, name: name.clone() });
            }
        }
        // Tracks: the loved first, then the played, then the rest.
        let mut tracks: Vec<(usize, f32, u32)> = (0..model.len()).map(|i| (i, model.tracks[i].aff_long(model.now()), model.tracks[i].starts)).collect();
        tracks.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        for (i, _, _) in tracks {
            if full(&plans) {
                break;
            }
            let t = model.id(i);
            let sid = t.to_string();
            let need_yt = room(&plans, Lane::Youtube) && due("track", &sid, GraphSource::YoutubeRadio.as_str());
            let mbid = mbids
                .get(&format!("track:{sid}"))
                .cloned()
                .filter(|_| room(&plans, Lane::ListenBrainz) && due("track", &sid, GraphSource::ListenBrainz.as_str()));
            let need_fm = room(&plans, Lane::LastFm) && due("track", &sid, GraphSource::LastFm.as_str());
            if !need_yt && mbid.is_none() && !need_fm {
                continue;
            }
            let Some(row) = self.lib.get_track(t).ok().flatten() else { continue };
            let artist = row.artist_credit.display();
            if need_yt {
                let refs = self.lib.stream_refs(t).unwrap_or_default();
                plans[Lane::Youtube.index()].push(match refs.iter().find(|r| r.id.service == Service::Youtube) {
                    Some(r) => Ask::Radio { track: t, id: r.id.id.clone() },
                    None => Ask::FindSong { track: t, query: format!("{artist} {}", row.title), key: song_key(&row.title, &artist) },
                });
            }
            if let Some(m) = mbid {
                plans[Lane::ListenBrainz.index()].push(Ask::LbRecordings { track: t, mbid: m });
            }
            if need_fm {
                plans[Lane::LastFm.index()].push(Ask::FmTrack { track: t, artist: artist.clone(), title: row.title.clone() });
            }
        }
        for lane in Lane::ALL {
            if !want[lane.index()] {
                continue;
            }
            let plan = std::mem::take(&mut plans[lane.index()]);
            let l = self.recs.graph.lane(lane);
            if plan.is_empty() {
                l.covered_at = Some(Instant::now());
                continue;
            }
            log::info!("graph: {lane:?} plans {} requests", plan.len());
            l.queue = plan.into();
        }
    }

    /// Wakes the pass after `d`.
    fn graph_wake(&mut self, d: Duration) {
        self.streaming.spawn_internal(async move {
            tokio::time::sleep(d).await;
            Internal::Recs(Msg::Graph(GraphMsg::Wake))
        });
    }

    /// `lane` asks again after `d`.
    fn graph_wait(&mut self, lane: Lane, d: Duration) {
        self.recs.graph.lane(lane).next_at = Some(Instant::now() + d);
        self.graph_wake(d);
    }

    /// Asks `a` next in its lane.
    fn graph_push(&mut self, a: Ask) {
        self.recs.graph.lane(a.lane()).queue.push_front(a);
    }

    /// The pass ended (nothing is due in any lane): what it kept is built
    /// in.
    fn graph_done(&mut self) {
        let g = &mut self.recs.graph;
        g.running = false;
        log::info!("graph: the pass is over, {} requests this run", g.requests);
        if std::mem::take(&mut g.fresh) > 0 {
            g.built_at = Some(Instant::now());
            let after = self.streaming.schedule.recs_debounce;
            self.recs_dirty(after);
        }
    }

    /// Sends the next request of every lane that may ask now.
    fn graph_step(&mut self, meta: Option<&Metadata>) {
        // Not while a game is in front either (the tick goes on after it).
        if self.streaming.search.active() || self.streams_idle() || crate::quiet::busy() {
            return;
        }
        if !std::mem::replace(&mut self.recs.graph.warmed, true) {
            let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
            for lane in Lane::ALL {
                self.recs.graph.lane(lane).next_at = Some(Instant::now() + gap);
            }
            self.graph_wake(gap);
            return;
        }
        for lane in Lane::ALL {
            self.graph_lane_step(lane, meta);
        }
    }

    fn graph_lane_step(&mut self, lane: Lane, meta: Option<&Metadata>) {
        let l = &self.recs.graph.lanes[lane.index()];
        if l.busy || l.next_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        if lane.is_service() && self.recs.discover_busy() {
            return;
        }
        loop {
            let Some(next) = self.recs.graph.lane(lane).queue.pop_front() else { return };
            if self.recs.graph.closed.contains(&next.subject().2) {
                continue;
            }
            let svc = next.service().and_then(|s| self.service(s));
            if next.service().is_some() && svc.is_none() {
                continue;
            }
            if let Some(left) = next.service().and_then(|s| self.pause_left(s)) {
                self.recs.graph.lane(lane).queue.push_front(next);
                self.graph_wait(lane, left + crate::streams::wake_jitter());
                return;
            }
            if next.service().is_none() && meta.is_none() {
                continue;
            }
            let g = &mut self.recs.graph;
            g.requests += 1;
            g.running = true;
            g.lane(lane).busy = true;
            let meta = meta.cloned();
            self.streaming.spawn_internal(async move {
                let result = ask(svc, meta, next.clone()).await;
                Internal::Recs(Msg::Graph(GraphMsg::Answered { ask: next, result }))
            });
            return;
        }
    }

    pub(crate) fn graph_msg(&mut self, m: GraphMsg) {
        let (ask, result) = match m {
            GraphMsg::Answered { ask, result } => (ask, result),
            GraphMsg::Wake => {
                self.graph_tick();
                return;
            }
        };
        let lane = ask.lane();
        let s = &self.streaming.schedule;
        let gap = StreamSchedule::between(match lane {
            Lane::Youtube => s.graph_gap_youtube,
            Lane::Deezer => s.graph_gap_deezer,
            _ => s.graph_gap_open,
        });
        let (unknown, cap, debounce, build_every) = (s.rate_limit_unknown, s.rate_limit_cap, s.recs_debounce, s.graph_build_every);
        self.recs.graph.lane(lane).busy = false;
        match result {
            Err(Failed::RateLimited(after)) => {
                let l = self.recs.graph.lane(lane);
                l.limited += 1;
                let n = l.limited;
                let said = after.or_else(|| ask.service().and_then(|s| self.pause_left(s)));
                let wait = backoff(said, unknown, cap, n);
                log::info!("graph: {lane:?} rate limited, again in {} s", wait.as_secs());
                self.graph_push(ask);
                self.graph_wait(lane, wait + crate::streams::wake_jitter());
                return;
            }
            Err(Failed::Closed(e)) => {
                let source = ask.subject().2;
                log::info!("graph: {source} is closed this run: {e}");
                self.recs.graph.closed.push(source);
                self.graph_keep(&ask, "error", None, &[], &[]);
            }
            Err(Failed::Other(e)) => {
                log::info!("graph: {ask:?}: {e}");
                self.graph_keep(&ask, "error", None, &[], &[]);
            }
            Ok(answer) => self.graph_answer(ask, answer),
        }
        let g = &mut self.recs.graph;
        g.lane(lane).limited = 0;
        if g.fresh >= FRESH_BUILD && g.built_at.is_none_or(|t| t.elapsed() >= build_every) {
            g.fresh = 0;
            g.built_at = Some(Instant::now());
            self.recs_dirty(debounce);
        }
        self.graph_wait(lane, gap);
    }

    fn graph_keep(&mut self, ask: &Ask, status: &str, remote: Option<&str>, entries: &[GraphEntry], tags: &[(String, f32)]) {
        let (kind, id, source) = ask.subject();
        if let Err(e) = self.lib.graph_put(kind, &id, source, status, remote, entries, tags, now_ms()) {
            log::warn!("graph: keeping {kind} {id}: {e}");
            return;
        }
        self.recs.graph.fresh += entries.len() + tags.len();
    }

    fn graph_answer(&mut self, ask: Ask, answer: Answer) {
        let status = |n: usize| if n > 0 { "ok" } else { "none" };
        match (&ask, answer) {
            (Ask::Radio { id, .. } | Ask::DzTop { id, .. }, Answer::Tracks(t)) => {
                let entries: Vec<GraphEntry> = t.iter().filter(|x| x.id.id != *id).map(track_entry).collect();
                self.graph_keep(&ask, status(entries.len()), Some(id), &entries, &[]);
            }
            (Ask::FindSong { track, key, .. }, Answer::Tracks(hits)) => match hits.iter().find(|h| stream_song_key(h) == *key) {
                Some(hit) => self.graph_push(Ask::Radio { track: *track, id: hit.id.id.clone() }),
                None => self.graph_keep(&ask, "none", None, &[], &[]),
            },
            (Ask::Artists { id, .. }, Answer::Artists(a)) => {
                let entries: Vec<GraphEntry> = a.iter().map(artist_entry).collect();
                self.graph_keep(&ask, status(entries.len()), Some(id), &entries, &[]);
            }
            (Ask::FindArtist { artist, service, name }, Answer::Artists(hits)) => {
                let key = artist_key(name);
                match hits.iter().find(|h| artist_key(&h.name) == key) {
                    Some(hit) => {
                        log::info!("artist {artist} is {}", hit.id);
                        let _ = self.lib.set_external_id(EntityKind::Artist, &hit.id, *artist.as_uuid());
                        if *service == Service::Deezer {
                            self.graph_push(Ask::DzTop { artist: *artist, id: hit.id.id.clone() });
                        }
                        self.graph_push(Ask::Artists { artist: *artist, service: *service, id: hit.id.id.clone() });
                    }
                    None => self.graph_keep(&ask, "none", None, &[], &[]),
                }
            }
            (Ask::Tags { artist, .. }, Answer::Tags(found)) => match found {
                Some((mbid, tags)) => {
                    self.graph_keep(&ask, "ok", Some(&mbid), &[], &tags);
                    // ListenBrainz knows the artist by this id.
                    if self.graph_popularity_open(self.graph_metadata().as_ref()) {
                        self.graph_push(Ask::LbTop { artist: *artist, mbid: mbid.clone() });
                    }
                    self.graph_push(Ask::LbArtists { artist: *artist, mbid });
                }
                None => self.graph_keep(&ask, "none", None, &[], &[]),
            },
            (_, Answer::Entries(entries)) => self.graph_keep(&ask, status(entries.len()), None, &entries, &[]),
            (ask, _) => log::warn!("graph: an unexpected answer to {ask:?}"),
        }
    }

    /// Keeps a discovery's radio of a library track in the graph too (one
    /// request for both).
    pub(crate) fn graph_from_discovery(&mut self, track: TrackId, id: &str, tracks: &[StreamTrack]) {
        let entries: Vec<GraphEntry> = tracks.iter().filter(|x| x.id.id != id).map(track_entry).collect();
        if !entries.is_empty() {
            self.graph_keep(&Ask::Radio { track, id: id.to_owned() }, "ok", Some(id), &entries, &[]);
        }
    }
}

/// The library artists' affinities (0…1, by [`artist_key`]) and the
/// artists beyond the library the graph links to them: a new artist's
/// music counts as known by half the weight of the links of the loved
/// artists that point at it, [`VOTE_CAP`] at most.
pub(crate) fn with_votes(mut known: HashMap<String, f32>, votes: &HashMap<String, Vec<(ArtistId, f32)>>, affinity: &dyn Fn(ArtistId) -> f32) -> HashMap<String, f32> {
    for (key, from) in votes {
        let v = (0.5 * from.iter().map(|(a, w)| affinity(*a).clamp(0.0, 1.0) * w).sum::<f32>()).min(VOTE_CAP);
        if v > 0.0 {
            let e = known.entry(key.clone()).or_insert(0.0);
            *e = e.max(v);
        }
    }
    known
}

/// A new artist counts as known at most this much by the graph.
pub(crate) const VOTE_CAP: f32 = 0.6;

/// The set of a pass's requests as `(kind, id, source)` (tests).
#[cfg(test)]
pub(crate) fn subjects(plan: &[Ask]) -> std::collections::HashSet<(&'static str, String, &'static str)> {
    plan.iter().map(Ask::subject).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::Uuid;

    fn row(kind: &str, id: &str, source: &str, rank: u32, entry: GraphEntry) -> GraphRow {
        GraphRow { subject_kind: kind.into(), subject_id: id.into(), source: source.into(), rank, total: 10, entry }
    }

    /// Entries resolve by the service's id, the MusicBrainz id or the
    /// song's key; an artist the library lacks votes, a track it lacks is
    /// left out.
    #[test]
    fn entries_resolve_to_the_library() {
        let t = |i: u128| TrackId(Uuid::from_u128(i));
        let a = |i: u128| ArtistId(Uuid::from_u128(1000 + i));
        let mut names = Names::default();
        names.remote.insert(("youtube".into(), "track".into(), "vid2".into()), t(2).to_string());
        names.mbid.insert("mb-3".into(), ("track".into(), t(3).to_string()));
        names.songs.insert(song_key("Four", "Band"), t(4));
        names.artists.insert(artist_key("Other Band"), a(2));
        names.remote.insert(("deezer".into(), "artist".into(), "dz3".into()), a(3).to_string());
        names.remote.insert(("deezer".into(), "track".into(), "dz-t5".into()), t(5).to_string());
        let track = |name: &str, remote: Option<&str>| GraphEntry { kind: "track".into(), name: name.into(), artist: "Band".into(), remote_id: remote.map(Into::into) };
        let artist = |name: &str, remote: Option<&str>| GraphEntry { kind: "artist".into(), name: name.into(), artist: String::new(), remote_id: remote.map(Into::into) };
        let s1 = t(1).to_string();
        let a1 = a(1).to_string();
        let rows = vec![
            row("track", &s1, "youtube_radio", 0, track("Two", Some("vid2"))),
            row("track", &s1, "youtube_radio", 1, track("Four", Some("unknown"))),
            row("track", &s1, "youtube_radio", 2, track("Elsewhere", Some("vid9"))),
            row("track", &s1, "listenbrainz", 0, track("Three", Some("mb-3"))),
            row("artist", &a1, "deezer_artists", 0, artist("Whoever", Some("dz3"))),
            row("artist", &a1, "listenbrainz", 0, artist("Other Band", Some("mb-x"))),
            row("artist", &a1, "lastfm", 3, artist("New Band", None)),
            row("artist", &a1, "popularity", 0, track("Four", None)),
            row("artist", &a1, "deezer_top", 0, track("Five", Some("dz-t5"))),
            row("artist", &a1, "deezer_top", 1, track("Four", Some("dz-unknown"))),
            row("artist", &a1, "deezer_top", 2, track("Elsewhere", Some("dz-t9"))),
            row("artist", &a1, "lastfm_top", 0, track("Three", Some("mb-3"))),
            row("artist", &a1, "lastfm_top", 1, track("Nowhere", None)),
        ];
        let tags = vec![GraphTag { subject_kind: "artist".into(), subject_id: a1.clone(), tag: "rock".into(), weight: 3.0 }];
        let r = resolve(&rows, &tags, &names);
        let to: Vec<TrackId> = r.input.tracks.iter().map(|e| e.to).collect();
        assert_eq!(to, vec![t(2), t(4), t(3)]);
        let to: Vec<ArtistId> = r.input.artists.iter().map(|e| e.to).collect();
        assert_eq!(to, vec![a(3), a(2)]);
        // Deezer's top resolves by its track ids, else by the song's key;
        // Last.fm's by the MusicBrainz id.
        assert_eq!(r.input.popular, vec![(t(4), 0, 10), (t(5), 0, 10), (t(4), 1, 10), (t(3), 0, 10)]);
        assert_eq!(r.input.tags.len(), 1);
        assert_eq!(r.votes.keys().collect::<Vec<_>>(), vec![&artist_key("New Band")]);
        // The new band counts as known by its loved neighbour, capped.
        let known = with_votes(HashMap::new(), &r.votes, &|x| if x == a(1) { 1.0 } else { 0.0 });
        let v = known[&artist_key("New Band")];
        assert!(v > 0.0 && v <= VOTE_CAP);
    }

    #[test]
    fn each_request_names_its_subject_and_source() {
        let t = TrackId(Uuid::from_u128(5));
        let a = ArtistId(Uuid::from_u128(6));
        let plan = vec![
            Ask::FindSong { track: t, query: String::new(), key: String::new() },
            Ask::Radio { track: t, id: "v".into() },
            Ask::FindArtist { artist: a, service: Service::Deezer, name: String::new() },
            Ask::Tags { artist: a, name: String::new(), mbid: None },
            Ask::LbTop { artist: a, mbid: String::new() },
            Ask::DzTop { artist: a, id: "dz".into() },
            Ask::DzTopOpen { artist: a, name: String::new(), id: None },
            Ask::FmTop { artist: a, name: String::new() },
        ];
        let s = subjects(&plan);
        // Deezer's top is one source whichever way it is asked.
        assert_eq!(s.len(), 6);
        assert!(s.contains(&("artist", a.to_string(), "deezer_top")));
        assert!(s.contains(&("artist", a.to_string(), "lastfm_top")));
        let lanes: Vec<Lane> = plan.iter().map(Ask::lane).collect();
        assert_eq!(
            lanes,
            vec![Lane::Youtube, Lane::Youtube, Lane::Deezer, Lane::MusicBrainz, Lane::ListenBrainz, Lane::Deezer, Lane::ListenBrainz, Lane::LastFm]
        );
        assert!(s.contains(&("artist", a.to_string(), "deezer_artists")));
        assert!(s.contains(&("artist", a.to_string(), "popularity")));
    }

    /// MusicBrainz's 503 says `Retry-After: 0`: the lane waits a second,
    /// then twice as long each time, at most the cap; a longer pause the
    /// source names is waited out whole.
    #[test]
    fn a_rate_limit_backs_off_from_a_second() {
        let (unknown, cap) = (Duration::from_secs(30), Duration::from_secs(600));
        let secs = |said: Option<u64>, n| backoff(said.map(Duration::from_secs), unknown, cap, n).as_secs();
        assert_eq!((1..=5).map(|n| secs(Some(0), n)).collect::<Vec<_>>(), vec![1, 2, 4, 8, 16]);
        assert_eq!(secs(Some(0), 12), 600);
        assert_eq!(secs(Some(0), 40), 600);
        assert_eq!(secs(None, 1), 30);
        assert_eq!(secs(Some(3600), 1), 3600);
    }
}
