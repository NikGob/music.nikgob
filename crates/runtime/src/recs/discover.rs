//! «Новое для тебя» and «Новые релизы» (docs/stage4.md §5): a background
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
//!   ([`nikgob_recs::suggest::SUGGESTION_SEEDS`], docs/stage4.md §12.3)
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
//!   [`nikgob_recs::Recs::new_releases`]; both kept in the cache
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
use nikgob_recs::{
    DiscoverySeeds, SeedFinds, SeedRef, artist_key, rank_discoveries, song_key, stream_song_key, suggestion_finds,
};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{ARTIST_NAME, Msg, SONG, local_day};
use crate::Worker;
use crate::import::Internal;
use crate::streams::StreamSchedule;

/// Requests of the discovery part of a pass…
pub(crate) const DISCOVER_BUDGET: u32 = 60;
/// …and of the new releases.
pub(crate) const RELEASE_BUDGET: u32 = 40;
/// An artist's discography is asked at most this often.
pub(crate) const RELEASES_TTL_MS: i64 = 7 * 24 * 3600 * 1000;
/// Similar artists whose tops are read, per list.
const TOPS_PER_LIST: usize = 2;
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
    found.map_or((Vec::new(), 0), |f| (f.tracks, f.releases.len()))
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
/// suggest now.
fn ranked(snap: &super::Snapshot, seeds: &DiscoverySeeds, finds: &[(Seed, Vec<StreamTrack>)], filter: &nikgob_recs::DiscoveryFilter) -> Vec<StreamTrack> {
    let mut all: Vec<SeedFinds> = finds
        .iter()
        .map(|(seed, tracks)| SeedFinds { seed: seed.engine(), tracks: tracks.clone() })
        .collect();
    all.extend(suggestion_finds(&snap.suggested_new));
    rank_discoveries(seeds, &all, filter).into_iter().map(|d| d.track).collect()
}

fn releases_key(artist: ArtistId) -> String {
    format!("releases.{artist}")
}

impl Worker {
    fn discover_load(&mut self) {
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

    /// The discoveries as found (the home filters out the disliked).
    pub(crate) fn discoveries(&self) -> Vec<StreamTrack> {
        self.recs.discover.found.as_ref().map(|f| f.tracks.clone()).unwrap_or_default()
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
        let DiscoverySeeds { tracks, artists, profiles } = recs.discovery_seeds();
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
        // Tracks and artists in turn, so a short budget still sees both.
        let (mut ti, mut ai) = (track_steps.into_iter(), artist_steps.into_iter());
        loop {
            let (t, a) = (ti.next(), ai.next());
            if t.is_none() && a.is_none() {
                break;
            }
            pass.steps.extend(t);
            pass.steps.extend(a);
        }
        // New releases: Deezer dates albums to the day.
        if dz {
            let now = now_ms();
            for a in release_artists {
                let cached: Option<Vec<StreamAlbum>> = self
                    .lib
                    .cache_get(&releases_key(a))
                    .ok()
                    .flatten()
                    .filter(|(at, _)| now - at < RELEASES_TTL_MS)
                    .and_then(|(_, json)| serde_json::from_str(&json).ok());
                if let Some(albums) = cached {
                    pass.releases.push((a, albums));
                    continue;
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
            let paused = self.pause_left(service);
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
                pass.releases.push((artist, albums));
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
        let seeds = DiscoverySeeds {
            tracks: pass.seed_tracks.clone(),
            artists: pass.seed_artists.clone(),
            profiles: pass.seed_profiles.clone(),
        };
        let tracks = ranked(&snap, &seeds, &pass.finds, &filter);
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
            seed_profiles: pass.seed_profiles,
            finds: pass.finds,
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
        let tracks = ranked(&snap, &seeds, &found.finds, &filter);
        if tracks == found.tracks {
            return;
        }
        log::info!("discover: {} discoveries with the services' suggestions", tracks.len());
        found.tracks = tracks;
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
        self.discover_load();
        let Some(found) = self.recs.discover.found.clone() else { return (Vec::new(), Vec::new()) };
        let lib = &self.lib;
        let disliked = |kind: &str, id: &str| lib.is_rec_disliked(kind, id).unwrap_or(false);
        let tracks: Vec<&StreamTrack> = found
            .tracks
            .iter()
            .filter(|t| {
                !disliked(SONG, &stream_song_key(t))
                    && !t.artists.iter().any(|a| disliked(ARTIST_NAME, &artist_key(&a.name)))
            })
            .collect();
        let (day, _) = local_day();
        let mut counted = false;
        for t in &tracks {
            let e = self.recs.discover.shown.entry(stream_song_key(t)).or_insert((0, String::new()));
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
        let rows = tracks.into_iter().map(|t| self.stream_row_auto(t)).collect();
        let albums = found
            .releases
            .iter()
            .filter(|(a, _)| !self.lib.is_rec_disliked("artist", &a.to_string()).unwrap_or(false))
            .map(|(_, album)| self.album_card(album))
            .collect();
        (rows, albums)
    }
}
