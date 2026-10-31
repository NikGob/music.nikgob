//! The graph of similar music in the runtime (docs/recs-graph.md): the
//! background pass that asks the sources about the library's artists and
//! tracks and keeps their answers in the library, and the resolution of
//! what was kept to the catalog at every build.
//!
//! * **The pass**: one request at a time, [`StreamSchedule::discover_gap`]
//!   apart, never while a search is typed or answered, while the window is
//!   idle, or while a discovery request is on its way; a rate limit waits
//!   its pause and asks the same again. At most [`GRAPH_BUDGET`] requests,
//!   a new pass [`GRAPH_EVERY`] after the last while anything is due.
//!   Artists first (one answer covers all their tracks: the most of a cold
//!   start), the loved and played first, then the tracks the same way.
//!   What each source said is kept for [`TTL_MS`] (MusicBrainz's tags
//!   [`TAGS_TTL_MS`]), an error is asked again after [`RETRY_MS`].
//! * **The sources**: YouTube Music's radio of a track (a song search first
//!   when the track has no id there) and its similar artists, Deezer's
//!   similar artists («Спрашивать YouTube Music и Deezer»); MusicBrainz's
//!   artist with its tags (the id ListenBrainz needs), ListenBrainz's
//!   similar artists, recordings and an artist's popular recordings
//!   («Спрашивать ListenBrainz и MusicBrainz», with the online metadata of
//!   the import); Last.fm's similar tracks and artists with the user's key.
//! * **Resolution** ([`resolve`]): an entry is the library's track or artist
//!   by the service's id, by its MusicBrainz id, else by its song or artist
//!   key; what the library lacks is left out of the graph, and the similar
//!   artists it lacks vote for the new music of their artists
//!   ([`Resolved::votes`]).
//!
//! [`StreamSchedule::discover_gap`]: crate::StreamSchedule::discover_gap

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

/// Requests of a pass.
pub(crate) const GRAPH_BUDGET: u32 = 120;
/// A pass this long after the last.
pub(crate) const GRAPH_EVERY: Duration = Duration::from_secs(2 * 3600);
/// What a source said stands this long…
pub(crate) const TTL_MS: i64 = 30 * 86_400_000;
/// …MusicBrainz's tags this long…
pub(crate) const TAGS_TTL_MS: i64 = 90 * 86_400_000;
/// …and an error is asked again after this.
pub(crate) const RETRY_MS: i64 = 86_400_000;
/// Entries kept since the last build that make a build due.
const FRESH_BUILD: usize = 60;

/// The sources as the library keeps them.
const S_MUSICBRAINZ: &str = "musicbrainz";
const S_POPULARITY: &str = "popularity";

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
    /// MusicBrainz's artist and its tags (by its id, else by name).
    Tags { artist: ArtistId, name: String, mbid: Option<String> },
    LbArtists { artist: ArtistId, mbid: String },
    LbTop { artist: ArtistId, mbid: String },
    LbRecordings { track: TrackId, mbid: String },
    FmTrack { track: TrackId, artist: String, title: String },
    FmArtist { artist: ArtistId, name: String },
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
            Self::Tags { artist, .. } => ("artist", a(artist), S_MUSICBRAINZ),
            Self::LbArtists { artist, .. } => ("artist", a(artist), GraphSource::ListenBrainz.as_str()),
            Self::LbTop { artist, .. } => ("artist", a(artist), S_POPULARITY),
            Self::LbRecordings { track, .. } => ("track", t(track), GraphSource::ListenBrainz.as_str()),
            Self::FmTrack { track, .. } => ("track", t(track), GraphSource::LastFm.as_str()),
            Self::FmArtist { artist, .. } => ("artist", a(artist), GraphSource::LastFm.as_str()),
        }
    }

    /// The service it asks, if one of the streaming services.
    fn service(&self) -> Option<Service> {
        match self {
            Self::Radio { .. } | Self::FindSong { .. } => Some(Service::Youtube),
            Self::Artists { service, .. } | Self::FindArtist { service, .. } => Some(*service),
            _ => None,
        }
    }
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

#[derive(Default)]
pub(crate) struct GraphState {
    plan: VecDeque<Ask>,
    requests: u32,
    busy: bool,
    next_at: Option<Instant>,
    /// When the last pass ended (none this run yet).
    last_pass: Option<Instant>,
    /// The first request of this run waited its gap.
    warmed: bool,
    /// Entries kept since the last build asked for.
    fresh: usize,
    /// Sources that wanted an account or a key this run ([`Failed::Closed`]).
    closed: Vec<&'static str>,
    /// The online metadata to ask (tests set it; else the import's).
    pub(crate) metadata: Option<Metadata>,
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
            "listenbrainz" | "lastfm" | S_POPULARITY => by_mbid(),
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
            ("artist", "track") if r.source == S_POPULARITY => {
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
        self.recs.graph.last_pass = None;
        self.graph_tick();
    }

    /// Every tick: a pass when one is due, its next request when it is time.
    pub(crate) fn graph_tick(&mut self) {
        let prefs = self.state.read().prefs.clone();
        if !prefs.graph_use || !(prefs.graph_services || prefs.graph_open_data) {
            self.recs.graph.plan.clear();
            return;
        }
        if self.recs.graph.plan.is_empty() {
            let due = self.recs.graph.last_pass.is_none_or(|t| t.elapsed() >= GRAPH_EVERY);
            if !due || self.recs.graph.busy || !self.graph_plan(&prefs) {
                return;
            }
        }
        self.graph_step();
    }

    /// Plans a pass from the snapshot: false when there is none or nothing
    /// is due.
    fn graph_plan(&mut self, prefs: &nikgob_view::Prefs) -> bool {
        let Some(snap) = self.snapshot() else { return false };
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
        let yt = prefs.graph_services && self.service(Service::Youtube).is_some();
        let dz = prefs.graph_services && self.service(Service::Deezer).is_some();
        let meta = if prefs.graph_open_data { self.graph_metadata() } else { None };
        let open = meta.is_some();
        let fm = self.graph_metadata().is_some_and(|m| m.lastfm.is_some());
        let model = &snap.model;
        let cat = &snap.catalog;
        let due = |kind: &str, id: &str, source: &str| !fresh(&fetches, kind, id, source, now);
        // Artists: the loved first, then the ones with most tracks.
        let mut artists: Vec<(ArtistId, f32, usize)> =
            cat.artists().map(|a| (a, model.taste.artist_affinity(a), cat.of_artist(a).len())).collect();
        artists.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        let mut plan: Vec<Ask> = Vec::new();
        let room = (2 * GRAPH_BUDGET) as usize;
        for (a, _, _) in artists {
            if plan.len() >= room {
                break;
            }
            let sid = a.to_string();
            let Some(name) = self.lib.get_artist(a).ok().flatten().map(|x| x.name) else { continue };
            let known = self.lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default();
            for (on, service, source) in [(yt, Service::Youtube, GraphSource::YoutubeArtists), (dz, Service::Deezer, GraphSource::DeezerArtists)] {
                if on && due("artist", &sid, source.as_str()) {
                    plan.push(match known.iter().find(|r| r.service == service) {
                        Some(r) => Ask::Artists { artist: a, service, id: r.id.clone() },
                        None => Ask::FindArtist { artist: a, service, name: name.clone() },
                    });
                }
            }
            if open {
                let mbid = mbids.get(&format!("artist:{sid}")).cloned();
                if due("artist", &sid, S_MUSICBRAINZ) {
                    plan.push(Ask::Tags { artist: a, name: name.clone(), mbid: mbid.clone() });
                } else if let Some(m) = mbid {
                    if due("artist", &sid, GraphSource::ListenBrainz.as_str()) {
                        plan.push(Ask::LbArtists { artist: a, mbid: m.clone() });
                    }
                    if due("artist", &sid, S_POPULARITY) {
                        plan.push(Ask::LbTop { artist: a, mbid: m });
                    }
                }
            }
            if fm && due("artist", &sid, GraphSource::LastFm.as_str()) {
                plan.push(Ask::FmArtist { artist: a, name: name.clone() });
            }
        }
        // Tracks: the loved first, then the played, then the rest.
        let mut tracks: Vec<(usize, f32, u32)> = (0..model.len()).map(|i| (i, model.tracks[i].aff_long(model.now()), model.tracks[i].starts)).collect();
        tracks.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
        for (i, _, _) in tracks {
            if plan.len() >= 2 * room {
                break;
            }
            let t = model.id(i);
            let sid = t.to_string();
            let need_yt = yt && due("track", &sid, GraphSource::YoutubeRadio.as_str());
            let mbid = mbids.get(&format!("track:{sid}")).cloned().filter(|_| open && due("track", &sid, GraphSource::ListenBrainz.as_str()));
            let need_fm = fm && due("track", &sid, GraphSource::LastFm.as_str());
            if !need_yt && mbid.is_none() && !need_fm {
                continue;
            }
            let Some(row) = self.lib.get_track(t).ok().flatten() else { continue };
            let artist = row.artist_credit.display();
            if need_yt {
                let refs = self.lib.stream_refs(t).unwrap_or_default();
                plan.push(match refs.iter().find(|r| r.id.service == Service::Youtube) {
                    Some(r) => Ask::Radio { track: t, id: r.id.id.clone() },
                    None => Ask::FindSong { track: t, query: format!("{artist} {}", row.title), key: song_key(&row.title, &artist) },
                });
            }
            if let Some(m) = mbid {
                plan.push(Ask::LbRecordings { track: t, mbid: m });
            }
            if need_fm {
                plan.push(Ask::FmTrack { track: t, artist: artist.clone(), title: row.title.clone() });
            }
        }
        plan.retain(|a| !self.recs.graph.closed.contains(&a.subject().2));
        if plan.is_empty() {
            self.recs.graph.last_pass = Some(Instant::now());
            return false;
        }
        log::info!("graph: a pass with {} requests planned", plan.len());
        let g = &mut self.recs.graph;
        g.plan = plan.into();
        g.requests = 0;
        true
    }

    fn graph_wait(&mut self, d: Duration) {
        self.recs.graph.next_at = Some(Instant::now() + d);
        self.streaming.spawn_internal(async move {
            tokio::time::sleep(d).await;
            Internal::Recs(Msg::Graph(GraphMsg::Wake))
        });
    }

    /// The pass ended: what it kept is built in.
    fn graph_done(&mut self) {
        let g = &mut self.recs.graph;
        g.plan.clear();
        g.last_pass = Some(Instant::now());
        log::info!("graph: the pass is over after {} requests", g.requests);
        if std::mem::take(&mut g.fresh) > 0 {
            let after = self.streaming.schedule.recs_debounce;
            self.recs_dirty(after);
        }
    }

    /// Sends the next request when everything allows it.
    fn graph_step(&mut self) {
        let g = &self.recs.graph;
        if g.busy || g.next_at.is_some_and(|t| Instant::now() < t) || self.recs.discover_busy() {
            return;
        }
        if self.streaming.search.active() || self.streams_idle() {
            return;
        }
        if !std::mem::replace(&mut self.recs.graph.warmed, true) {
            let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
            self.graph_wait(gap);
            return;
        }
        let meta = self.graph_metadata();
        loop {
            if self.recs.graph.requests >= GRAPH_BUDGET {
                self.graph_done();
                return;
            }
            let Some(next) = self.recs.graph.plan.pop_front() else {
                self.graph_done();
                return;
            };
            if self.recs.graph.closed.contains(&next.subject().2) {
                continue;
            }
            let svc = next.service().and_then(|s| self.service(s));
            if next.service().is_some() && svc.is_none() {
                continue;
            }
            if let Some(left) = next.service().and_then(|s| self.pause_left(s)) {
                self.recs.graph.plan.push_front(next);
                self.graph_wait(left + crate::streams::wake_jitter());
                return;
            }
            if next.service().is_none() && meta.is_none() {
                continue;
            }
            let g = &mut self.recs.graph;
            g.requests += 1;
            g.busy = true;
            let meta = meta.clone();
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
        self.recs.graph.busy = false;
        let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
        match result {
            Err(Failed::RateLimited(after)) => {
                self.recs.graph.requests = self.recs.graph.requests.saturating_sub(1);
                let wait = after.or_else(|| ask.service().and_then(|s| self.pause_left(s))).unwrap_or(self.streaming.schedule.rate_limit_unknown);
                log::info!("graph: rate limited, again in {} s", wait.as_secs());
                self.recs.graph.plan.push_front(ask);
                self.graph_wait(wait + crate::streams::wake_jitter());
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
        if self.recs.graph.fresh >= FRESH_BUILD {
            self.recs.graph.fresh = 0;
            let after = self.streaming.schedule.recs_debounce;
            self.recs_dirty(after);
        }
        self.graph_wait(gap);
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
            (Ask::Radio { id, .. }, Answer::Tracks(t)) => {
                let entries: Vec<GraphEntry> = t.iter().filter(|x| x.id.id != *id).map(track_entry).collect();
                self.graph_keep(&ask, status(entries.len()), Some(id), &entries, &[]);
            }
            (Ask::FindSong { track, key, .. }, Answer::Tracks(hits)) => match hits.iter().find(|h| stream_song_key(h) == *key) {
                Some(hit) => self.recs.graph.plan.push_front(Ask::Radio { track: *track, id: hit.id.id.clone() }),
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
                        self.recs.graph.plan.push_front(Ask::Artists { artist: *artist, service: *service, id: hit.id.id.clone() });
                    }
                    None => self.graph_keep(&ask, "none", None, &[], &[]),
                }
            }
            (Ask::Tags { artist, .. }, Answer::Tags(found)) => match found {
                Some((mbid, tags)) => {
                    self.graph_keep(&ask, "ok", Some(&mbid), &[], &tags);
                    // ListenBrainz knows the artist by this id.
                    self.recs.graph.plan.push_front(Ask::LbTop { artist: *artist, mbid: mbid.clone() });
                    self.recs.graph.plan.push_front(Ask::LbArtists { artist: *artist, mbid });
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
        ];
        let tags = vec![GraphTag { subject_kind: "artist".into(), subject_id: a1.clone(), tag: "rock".into(), weight: 3.0 }];
        let r = resolve(&rows, &tags, &names);
        let to: Vec<TrackId> = r.input.tracks.iter().map(|e| e.to).collect();
        assert_eq!(to, vec![t(2), t(4), t(3)]);
        let to: Vec<ArtistId> = r.input.artists.iter().map(|e| e.to).collect();
        assert_eq!(to, vec![a(3), a(2)]);
        assert_eq!(r.input.popular, vec![(t(4), 0, 10)]);
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
        ];
        let s = subjects(&plan);
        assert_eq!(s.len(), 4);
        assert!(s.contains(&("artist", a.to_string(), "deezer_artists")));
        assert!(s.contains(&("artist", a.to_string(), "popularity")));
    }
}
