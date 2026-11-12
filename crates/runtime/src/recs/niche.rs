//! The top-up of the niches (docs/recommendations.md §1.2): a mix of «Миксы
//! для тебя» the library has few songs or artists for
//! ([`nikgob_recs::open_world::is_niche`]) plays mostly new music, and its
//! songs seed it.
//!
//! * **What is asked**: YouTube Music's radio of the niche's
//!   [`RADIO_SEEDS`] best loved songs (Deezer's related list without
//!   YouTube Music; a song the services do not know by id is looked up by
//!   a song search first), Deezer's similar artists of its
//!   [`ARTIST_SEEDS`] commonest artists and the tops of the first
//!   [`TOPS_PER_LIST`] of each (an artist looked up by name first). The
//!   graph's own songs beside the niche's need no request: they are among
//!   the candidates already.
//! * **Pace**: one niche at a time, [`BUDGET`] requests at most, a day's
//!   [`DAY_BUDGET`]; one request at a time,
//!   [`crate::StreamSchedule::discover_gap`] apart, never while a search
//!   is typed or answered, while the window is idle, while a game is in
//!   front or while the discovery's pass waits for an answer. A rate limit
//!   waits its pause. Nothing is downloaded.
//! * **Kept**: each niche's songs (new indeed, no junk, [`KEEP`] at most)
//!   with the seeds whose lists held them, in the library's cache
//!   ([`cached`]) for [`TTL_MS`]; the mixes filter them by the niche's
//!   sound and kinds of song ([`nikgob_recs::open_world::NicheSound`]).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{ArtistId, TrackId, UnixMs};
use nikgob_core::streaming::{EntityKind, SearchKind, Service, StreamArtist, StreamError, StreamService, StreamTrack};
use nikgob_library::{Library, now_ms};
use nikgob_recs::{artist_key, song_key, stream_song_key};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{Msg, local_day};
use crate::Worker;
use crate::import::Internal;
use crate::streams::StreamSchedule;

/// A niche's songs stand this long.
const TTL_MS: i64 = 3 * 24 * 3600 * 1000;
/// Requests of one niche at most…
const BUDGET: u32 = 10;
/// …and of all of them in a day.
const DAY_BUDGET: u32 = 40;
/// The niche's songs whose radios are asked…
const RADIO_SEEDS: usize = 3;
/// …the artists whose similar artists are…
const ARTIST_SEEDS: usize = 2;
/// …and the similar artists whose tops are read, per list.
const TOPS_PER_LIST: usize = 2;
/// Songs a niche keeps.
const KEEP: usize = 150;
/// Library cache keys.
const KEY: &str = "niche.";
const DAY_KEY: &str = "niche.day";

/// What a niche's top-up found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct NicheFound {
    pub at: UnixMs,
    /// Each song and the niche's songs whose lists held it.
    pub tracks: Vec<(StreamTrack, Vec<TrackId>)>,
}

/// The cache key of the niche of the mix `key`.
fn key_of(key: v::MixKey) -> Option<String> {
    super::lanes::engine_key(key).map(|k| format!("{KEY}{}", k.token()))
}

/// What the last top-up of the niche of `key` found, if it still stands.
pub(crate) fn cached(lib: &Library, key: v::MixKey) -> Option<NicheFound> {
    let found: NicheFound = super::cache_read(lib, &key_of(key)?)?;
    (now_ms() - found.at < TTL_MS).then_some(found)
}

/// One request of a top-up.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Step {
    /// The songs around a seed: YouTube Music's radio, Deezer's related.
    Radio { seed: TrackId, service: Service, id: String },
    /// A seed the service does not know by id: a song search.
    FindSong { seed: TrackId, service: Service, query: String, key: String },
    /// Deezer's similar artists of a seed artist…
    Similar { seeds: Vec<TrackId>, id: String },
    /// …looked up by name first…
    FindArtist { artist: ArtistId, seeds: Vec<TrackId>, name: String },
    /// …and the top of one of them.
    Top { seeds: Vec<TrackId>, id: String },
}

impl Step {
    fn service(&self) -> Service {
        match self {
            Self::Radio { service, .. } | Self::FindSong { service, .. } => *service,
            _ => Service::Deezer,
        }
    }
}

pub(crate) enum Answer {
    Tracks(Vec<StreamTrack>),
    Artists(Vec<StreamArtist>),
}

pub(crate) enum NicheMsg {
    Answered { step: Step, result: Result<Answer, StreamError> },
    Wake,
}

/// The niche being topped up.
struct Open {
    mix: v::MixKey,
    steps: VecDeque<Step>,
    found: Vec<(StreamTrack, Vec<TrackId>)>,
    requests: u32,
}

#[derive(Default)]
pub(crate) struct NicheState {
    open: Option<Open>,
    busy: bool,
    next_at: Option<Instant>,
    warmed: bool,
    /// The day and its requests (kept in the library's cache).
    day: String,
    day_requests: u32,
    /// Niches left for this run: nothing to ask of them.
    spent: Vec<v::MixKey>,
}

async fn ask(svc: Arc<dyn StreamService>, step: &Step) -> Result<Answer, StreamError> {
    match step {
        Step::Radio { service: Service::Youtube, id, .. } => svc.radio(id).await.map(Answer::Tracks),
        Step::Radio { id, .. } => svc.related(id).await.map(Answer::Tracks),
        Step::FindSong { query, .. } => svc.search_kind(query, SearchKind::Songs).await.map(|s| Answer::Tracks(s.tracks)),
        Step::Similar { id, .. } => svc.similar_artists(id).await.map(Answer::Artists),
        Step::FindArtist { name, .. } => svc.search_kind(name, SearchKind::Artists).await.map(|s| Answer::Artists(s.artists)),
        Step::Top { id, .. } => svc.artist_top(id).await.map(Answer::Tracks),
    }
}

/// Adds `tracks` found by `seeds` to `found`, each song once (with every
/// seed that found it).
fn keep(found: &mut Vec<(StreamTrack, Vec<TrackId>)>, tracks: Vec<StreamTrack>, seeds: &[TrackId]) {
    let mut at: HashMap<String, usize> = found.iter().enumerate().map(|(k, (t, _))| (stream_song_key(t), k)).collect();
    for t in tracks {
        let key = stream_song_key(&t);
        match at.get(&key) {
            Some(&k) => {
                for s in seeds {
                    if !found[k].1.contains(s) {
                        found[k].1.push(*s);
                    }
                }
            }
            None => {
                at.insert(key, found.len());
                found.push((t, seeds.to_vec()));
            }
        }
    }
}

impl Worker {
    /// Every tick: the next niche to top up when none is under way, its
    /// next request when it is time.
    pub(crate) fn niche_tick(&mut self) {
        if crate::quiet::busy() || self.state.read().prefs.mixes_downloaded_only {
            return;
        }
        let (day, _) = local_day();
        if self.recs.niche.day != day {
            let kept: Option<(String, u32)> = super::cache_read(&self.lib, DAY_KEY);
            let n = &mut self.recs.niche;
            (n.day, n.day_requests) = kept.filter(|k| k.0 == day).unwrap_or((day, 0));
        }
        if self.recs.niche.open.is_none() && !self.niche_plan() {
            return;
        }
        self.niche_step();
    }

    /// Plans the top-up of the first niche of the home whose songs are
    /// stale; false when there is none.
    fn niche_plan(&mut self) -> bool {
        if self.recs.niche.day_requests >= DAY_BUDGET {
            return false;
        }
        let (yt, dz) = (self.service(Service::Youtube).is_some(), self.service(Service::Deezer).is_some());
        if !yt && !dz {
            return false;
        }
        let Some(snap) = self.snapshot() else { return false };
        let Some(home) = self.recs.home.as_ref() else { return false };
        let spent = &self.recs.niche.spent;
        let Some((mix, seeds)) = home
            .lanes
            .iter()
            .filter(|l| !l.niche.is_empty() && !spent.contains(&l.key))
            .find(|l| cached(&self.lib, l.key).is_none())
            .map(|l| (l.key, l.niche.clone()))
        else {
            return false;
        };
        // The best loved seeds first; their commonest artists.
        let model = &snap.model;
        let now = model.now();
        let mut loved: Vec<(f32, TrackId)> = seeds.iter().map(|t| (model.position(*t).map_or(0.0, |i| model.tracks[i].aff_long(now)), *t)).collect();
        loved.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut steps = VecDeque::new();
        for &(_, t) in loved.iter().take(RADIO_SEEDS) {
            let refs = self.lib.stream_refs(t).unwrap_or_default();
            let id_on = |s: Service| refs.iter().find(|r| r.id.service == s).map(|r| r.id.id.clone());
            let step = match (id_on(Service::Youtube), id_on(Service::Deezer)) {
                (Some(id), _) if yt => Some(Step::Radio { seed: t, service: Service::Youtube, id }),
                (_, Some(id)) if dz => Some(Step::Radio { seed: t, service: Service::Deezer, id }),
                _ => self.lib.get_track(t).ok().flatten().map(|row| {
                    let artist = row.artist_credit.display();
                    Step::FindSong {
                        seed: t,
                        service: if yt { Service::Youtube } else { Service::Deezer },
                        query: format!("{artist} {}", row.title),
                        key: song_key(&row.title, &artist),
                    }
                }),
            };
            steps.extend(step);
        }
        if dz {
            let mut by_artist: Vec<(ArtistId, Vec<TrackId>)> = Vec::new();
            for &(_, t) in &loved {
                let Some(a) = model.position(t).and_then(|i| model.main_artist_at(i)) else { continue };
                match by_artist.iter_mut().find(|x| x.0 == a) {
                    Some(x) => x.1.push(t),
                    None => by_artist.push((a, vec![t])),
                }
            }
            by_artist.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
            for (a, seeds) in by_artist.into_iter().take(ARTIST_SEEDS) {
                let known = self.lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default();
                match known.iter().find(|r| r.service == Service::Deezer) {
                    Some(r) => steps.push_back(Step::Similar { seeds, id: r.id.clone() }),
                    None => {
                        if let Some(name) = self.lib.get_artist(a).ok().flatten().map(|x| x.name)
                            && !nikgob_core::streaming::more_artists_note(&name)
                        {
                            steps.push_back(Step::FindArtist { artist: a, seeds, name });
                        }
                    }
                }
            }
        }
        if steps.is_empty() {
            self.recs.niche.spent.push(mix);
            return false;
        }
        log::info!("niche: topping up {mix:?} from {} seeds, {} requests planned", seeds.len(), steps.len());
        self.recs.niche.open = Some(Open { mix, steps, found: Vec::new(), requests: 0 });
        true
    }

    fn niche_wait(&mut self, d: Duration) {
        self.recs.niche.next_at = Some(Instant::now() + d);
        self.streaming.spawn_internal(async move {
            tokio::time::sleep(d).await;
            Internal::Recs(Msg::Niche(NicheMsg::Wake))
        });
    }

    /// Sends the next request of the niche under way when everything
    /// allows it.
    fn niche_step(&mut self) {
        let n = &self.recs.niche;
        if n.busy || n.next_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        if self.streaming.search.active() || self.streams_idle() || self.recs.discover_busy() {
            return;
        }
        if !std::mem::replace(&mut self.recs.niche.warmed, true) {
            let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
            self.niche_wait(gap);
            return;
        }
        loop {
            let over = self.recs.niche.day_requests >= DAY_BUDGET;
            let Some(open) = self.recs.niche.open.as_mut() else { return };
            let next = if open.requests >= BUDGET || over { None } else { open.steps.pop_front() };
            let Some(step) = next else {
                self.niche_finish();
                return;
            };
            let service = step.service();
            let Some(svc) = self.service(service) else { continue };
            if let Some(left) = self.pause_left(service) {
                if let Some(open) = self.recs.niche.open.as_mut() {
                    open.steps.push_front(step);
                }
                self.niche_wait(left + crate::streams::wake_jitter());
                return;
            }
            if let Some(open) = self.recs.niche.open.as_mut() {
                open.requests += 1;
            }
            let n = &mut self.recs.niche;
            n.day_requests += 1;
            n.busy = true;
            super::cache_write(&self.lib, DAY_KEY, now_ms(), &(n.day.clone(), n.day_requests));
            self.streaming.spawn_internal(async move {
                let result = ask(svc, &step).await;
                Internal::Recs(Msg::Niche(NicheMsg::Answered { step, result }))
            });
            return;
        }
    }

    pub(crate) fn niche_msg(&mut self, m: NicheMsg) {
        let (step, result) = match m {
            NicheMsg::Answered { step, result } => (step, result),
            NicheMsg::Wake => {
                self.niche_tick();
                return;
            }
        };
        self.recs.niche.busy = false;
        let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
        match result {
            Err(StreamError::RateLimited { retry_after }) => {
                let wait = retry_after.or_else(|| self.pause_left(step.service())).unwrap_or(self.streaming.schedule.rate_limit_unknown);
                log::info!("niche: rate limited by {}, again in {} s", step.service(), wait.as_secs());
                if let Some(open) = self.recs.niche.open.as_mut() {
                    open.requests = open.requests.saturating_sub(1);
                    open.steps.push_front(step);
                }
                self.niche_wait(wait + crate::streams::wake_jitter());
                return;
            }
            Err(e) => log::info!("niche: {step:?}: {e}"),
            Ok(answer) => self.niche_answer(step, answer),
        }
        self.niche_wait(gap);
    }

    fn niche_answer(&mut self, step: Step, answer: Answer) {
        let known: Vec<String> = self.snapshot().map(|s| s.filter.known_artists.iter().cloned().collect()).unwrap_or_default();
        let Some(open) = self.recs.niche.open.as_mut() else { return };
        match (step, answer) {
            (Step::Radio { seed, .. }, Answer::Tracks(t)) => keep(&mut open.found, t, &[seed]),
            (Step::FindSong { seed, service, key, .. }, Answer::Tracks(hits)) => {
                if let Some(hit) = hits.iter().find(|h| stream_song_key(h) == key) {
                    open.steps.push_front(Step::Radio { seed, service, id: hit.id.id.clone() });
                }
            }
            (Step::FindArtist { artist, seeds, name }, Answer::Artists(hits)) => {
                let key = artist_key(&name);
                if let Some(hit) = hits.iter().find(|h| artist_key(&h.name) == key) {
                    open.steps.push_front(Step::Similar { seeds, id: hit.id.id.clone() });
                    let _ = self.lib.set_external_id(EntityKind::Artist, &hit.id, *artist.as_uuid());
                }
            }
            (Step::Similar { seeds, .. }, Answer::Artists(similar)) => {
                // Artists new to the user first: theirs is the new music.
                let mut similar: Vec<&StreamArtist> = similar.iter().collect();
                similar.sort_by_key(|a| known.contains(&artist_key(&a.name)));
                for a in similar.into_iter().take(TOPS_PER_LIST) {
                    open.steps.push_front(Step::Top { seeds: seeds.clone(), id: a.id.id.clone() });
                }
            }
            (Step::Top { seeds, .. }, Answer::Tracks(t)) => keep(&mut open.found, t, &seeds),
            (step, _) => log::warn!("niche: an unexpected answer to {step:?}"),
        }
    }

    /// The niche under way is done: what is new indeed is kept, and the
    /// mixes take it at the next build.
    fn niche_finish(&mut self) {
        let Some(open) = self.recs.niche.open.take() else { return };
        self.recs.niche.spent.push(open.mix);
        let Some(snap) = self.snapshot() else { return };
        let mut tracks: Vec<(StreamTrack, Vec<TrackId>)> = open.found.into_iter().filter(|(t, _)| snap.is_new(t)).collect();
        // The songs most seeds found first.
        tracks.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        tracks.truncate(KEEP);
        log::info!("niche: {:?} topped up with {} songs in {} requests", open.mix, tracks.len(), open.requests);
        let Some(key) = key_of(open.mix) else { return };
        let found = NicheFound { at: now_ms(), tracks };
        self.streaming.learn(&found.tracks.iter().map(|t| t.0.clone()).collect::<Vec<_>>());
        super::cache_write(&self.lib, &key, found.at, &found);
        if !found.tracks.is_empty() {
            let after = self.streaming.schedule.recs_debounce;
            self.recs_dirty(after);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{ArtistRef, RemoteId};

    fn st(id: &str, title: &str, artist: &str) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists.push(ArtistRef::new(None, artist));
        t
    }

    /// A song two seeds' lists hold is kept once, with both seeds.
    #[test]
    fn a_song_of_two_lists_is_kept_once() {
        let (a, b) = (TrackId::new(), TrackId::new());
        let mut found = Vec::new();
        keep(&mut found, vec![st("1", "Calm", "Someone"), st("2", "Other", "Else")], &[a]);
        keep(&mut found, vec![st("3", "Calm (Remastered)", "Someone")], &[b]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].1, vec![a, b]);
    }
}
