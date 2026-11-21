//! The top-up of the niches: a mix of «Миксы
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
//! * **Pace**: one niche at a time, [`BUDGET`] requests of a niche in
//!   [`TTL_MS`] at most (a rate limit's counts too; kept in the library's
//!   cache), a day's [`DAY_BUDGET`]; a niche with nothing to ask waits
//!   [`RETRY`]; one request at a time,
//!   [`crate::StreamSchedule::discover_gap`] apart, never while a search
//!   is typed or answered, while the window is idle, while a game is in
//!   front or while the discovery's pass waits for an answer. A rate limit
//!   waits its pause. Only while the mixes play new music («С новым» and
//!   a share of new music above none). Nothing is downloaded.
//! * **Kept**: each niche's songs (new indeed, no junk, [`KEEP`] at most)
//!   with the seeds whose lists held them, in the library's cache
//!   ([`cached`]) for [`TTL_MS`] — for the niche of those seeds only (a
//!   later draw of the mix of other songs asks anew); the mixes filter them
//!   by the niche's sound and kinds of song
//!   ([`nikgob_recs::open_world::NicheSound`]).
//! * **«Моя волна»** asks too: when few new songs of the pool pass the
//!   filter it plays ([`Worker::niche_want_wave`]), its songs of that
//!   filter are topped up the same way — first, within the same budgets,
//!   whether the mixes play new music or not — and its pool takes them
//!   ([`wave_topped`]); [`WAVE_WANTS`] filters at most, while a wave plays.
//!   A wave whose first picks found nothing waits for its filter's top-up
//!   ([`Worker::wave_refill_ended`]): it plays what the top-up brings (or
//!   what a top-up of that filter kept lately), or ends when it brings
//!   nothing, cannot be asked or is over its budget. It waits even while
//!   the window is hidden and nothing plays (the top-up's requests go on
//!   then for it alone, within the same budgets).
//! * **Off the worker**: the wave only records what it wants; the plan (the
//!   cache and the library read) is made on the picks' thread
//!   ([`super::picker`], its never-dropped lane), and so are the writes of
//!   the budgets and of what was found, in the order they were made.

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
/// Requests of one niche in [`TTL_MS`] at most…
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
/// A niche with nothing to ask is planned again after this.
const RETRY: Duration = Duration::from_secs(6 * 3600);
/// The share of its seeds a top-up shares with the niche it is for.
const SAME_SEEDS: f32 = 0.5;
/// Library cache keys.
const KEY: &str = "niche.";
const DAY_KEY: &str = "niche.day";
/// After a niche's key: its window of requests.
const REQ: &str = ".req";
/// The filters of the wave asked for at most (the latest kept).
const WAVE_WANTS: usize = 4;

/// What a niche's top-up found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct NicheFound {
    pub at: UnixMs,
    /// The niche's songs it was found for, sorted.
    #[serde(default)]
    pub seeds: Vec<TrackId>,
    /// Each song and the niche's songs whose lists held it.
    pub tracks: Vec<(StreamTrack, Vec<TrackId>)>,
}

/// The cache key of the niche of the mix `key`.
fn key_of(key: v::MixKey) -> Option<String> {
    super::lanes::engine_key(key).map(|k| format!("{KEY}{}", k.token()))
}

/// What the last top-up of the niche of `key` found, if it still stands
/// and was found for the niche of these `seeds` (most of them).
pub(crate) fn cached(lib: &Library, key: v::MixKey, seeds: &[TrackId]) -> Option<NicheFound> {
    cached_at(lib, &key_of(key)?, seeds)
}

/// [`cached`] by the niche's cache key.
fn cached_at(lib: &Library, key: &str, seeds: &[TrackId]) -> Option<NicheFound> {
    let found: NicheFound = super::cache_read(lib, key)?;
    (now_ms() - found.at < TTL_MS && same_seeds(&found.seeds, seeds)).then_some(found)
}

/// The cache key of the top-up of a wave's filter (its token).
fn wave_key(token: &str) -> String {
    format!("{KEY}wave.{token}")
}

/// What the top-ups of the wave's filters asked for (`wants`: each filter's
/// token and its songs) found and still stands.
pub(crate) fn wave_topped(lib: &Library, wants: &[(String, Vec<TrackId>)]) -> Vec<NicheFound> {
    wants.iter().filter_map(|(token, seeds)| cached_at(lib, &wave_key(token), seeds)).collect()
}

/// Whether two sets of seeds are one niche's: [`SAME_SEEDS`] of all of
/// them in both.
fn same_seeds(a: &[TrackId], b: &[TrackId]) -> bool {
    let a: std::collections::HashSet<&TrackId> = a.iter().collect();
    let b: std::collections::HashSet<&TrackId> = b.iter().collect();
    let all = a.union(&b).count();
    all > 0 && a.intersection(&b).count() as f32 >= SAME_SEEDS * all as f32
}

/// The requests asked for the niche of `key` since the window began
/// (the window lasts [`TTL_MS`]).
fn requests_of(lib: &Library, key: &str) -> (UnixMs, u32) {
    let now = now_ms();
    super::cache_read::<(UnixMs, u32)>(lib, &format!("{key}{REQ}")).filter(|w| now - w.0 < TTL_MS).unwrap_or((now, 0))
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
    /// The plan made on the picks' thread ([`plan`]).
    Planned(Box<Plan>),
    /// What the niche under way found is kept in the cache: `found` songs
    /// for the wave's `(run_id, token)`, if it was one.
    Kept { wave: Option<(u64, String)>, found: usize },
}

/// The niche being topped up.
pub(crate) struct Open {
    /// What it is, for the log.
    what: String,
    /// The wave run and filter token it is for, if it is.
    wave: Option<(u64, String)>,
    /// Its cache key's token and its seeds, sorted.
    token: String,
    seeds: Vec<TrackId>,
    /// The niche's window of requests ([`requests_of`]).
    window: (UnixMs, u32),
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
    /// Niches left for a while (by their cache keys): nothing to ask of
    /// them, or just topped up (planned again after [`RETRY`]).
    spent: Vec<(String, Instant)>,
    /// The wave's filters whose new songs ran short and are still to be
    /// planned: each one's `(run_id, token, seeds)` (sorted), the latest last.
    pub wave: Vec<(u64, String, Vec<TrackId>)>,
    /// The wave's filters asked for lately (planned or not): their top-ups
    /// join its pool.
    pub wave_kept: Vec<(String, Vec<TrackId>)>,
    /// Moves whenever the top-ups the wave's pool takes change (a filter
    /// joins `wave_kept`, a top-up of one is kept): its pool is built again
    /// ([`super::wave::WavePool`]).
    pub topped_gen: u64,
    /// A plan is being made on the picks' thread.
    planning: bool,
}

/// What a plan of the next top-up is made of: the worker's state, read
/// with the library's cache and catalog on the picks' thread ([`plan`]).
struct PlanInput {
    lib: Library,
    snap: Arc<super::Snapshot>,
    /// Today, when its requests are still to be read from the cache.
    day: Option<String>,
    day_requests: u32,
    yt: bool,
    dz: bool,
    /// The cache keys left for a while.
    spent: Vec<String>,
    /// The wave's filters to plan, the latest last.
    wave: Vec<(u64, String, Vec<TrackId>)>,
    /// The home's niches (what, cache key, seeds), when the mixes play new
    /// music.
    home: Vec<(String, String, Vec<TrackId>)>,
}

/// A plan ([`plan`]).
pub(crate) struct Plan {
    /// Today and its requests, when they were read.
    day: Option<(String, u32)>,
    /// The top-up to open.
    open: Option<Open>,
    /// Cache keys with nothing to ask (left for [`RETRY`]).
    spent: Vec<String>,
    /// The wave's filters let go, each with `(run_id, token, kept)`.
    ended: Vec<(u64, String, usize)>,
    /// The wave's filters not planned yet (one is opened first).
    rest: Vec<(u64, String, Vec<TrackId>)>,
}

/// Keeps the budgets: today's requests `day` and, for the top-up under way,
/// its window of requests (`window`: its cache key and the window). A count
/// is never lowered (a write queued before a later one, or before the one
/// at shutdown, keeps the larger).
fn keep_budgets(lib: &Library, day: &(String, u32), window: Option<(&str, (UnixMs, u32))>) {
    let kept: Option<(String, u32)> = super::cache_read(lib, DAY_KEY);
    if !kept.is_some_and(|k| k.0 == day.0 && k.1 >= day.1) {
        super::cache_write(lib, DAY_KEY, now_ms(), day);
    }
    if let Some((key, w)) = window {
        let key = format!("{key}{REQ}");
        let kept: Option<(UnixMs, u32)> = super::cache_read(lib, &key);
        if !kept.is_some_and(|k| k.0 == w.0 && k.1 >= w.1) {
            super::cache_write(lib, &key, now_ms(), &w);
        }
    }
}

/// Plans the next top-up (see [`Worker::niche_plan`]); the library is read,
/// never written.
fn plan(mut i: PlanInput) -> Plan {
    let mut out = Plan { day: None, open: None, spent: Vec::new(), ended: Vec::new(), rest: Vec::new() };
    let mut day_requests = i.day_requests;
    if let Some(day) = i.day.take() {
        let kept: Option<(String, u32)> = super::cache_read(&i.lib, DAY_KEY);
        day_requests = kept.filter(|k| k.0 == day).map_or(0, |k| k.1);
        out.day = Some((day, day_requests));
    }
    // Today's budget left, a service to ask.
    let can_ask = day_requests < DAY_BUDGET && (i.yt || i.dz);
    // The wave's filters (the latest first), each planned or let go once.
    while let Some((run_id, token, seeds)) = i.wave.pop() {
        let key = wave_key(&token);
        if let Some(found) = cached_at(&i.lib, &key, &seeds) {
            // Topped up lately: what it kept joins the wave's pool.
            out.ended.push((run_id, token, found.tracks.len()));
            continue;
        }
        let window = requests_of(&i.lib, &key);
        if !can_ask || i.spent.contains(&key) || window.1 >= BUDGET {
            out.ended.push((run_id, token, 0));
            continue;
        }
        match steps_of(&i.lib, &i.snap, &seeds, i.yt, i.dz) {
            Some(steps) => {
                out.open = Some(Open::new(format!("the wave's {token}"), Some((run_id, token)), key, seeds, window, steps));
                out.rest = i.wave;
                return out;
            }
            None => {
                out.spent.push(key);
                out.ended.push((run_id, token, 0));
            }
        }
    }
    if !can_ask {
        return out;
    }
    let spent = |key: &String| i.spent.contains(key);
    let first = i
        .home
        .into_iter()
        .filter(|(_, key, _)| !spent(key))
        .filter(|(_, key, seeds)| cached_at(&i.lib, key, seeds).is_none())
        .find_map(|(what, key, seeds)| {
            let window = requests_of(&i.lib, &key);
            (window.1 < BUDGET).then_some((what, key, seeds, window))
        });
    if let Some((what, key, seeds, window)) = first {
        match steps_of(&i.lib, &i.snap, &seeds, i.yt, i.dz) {
            Some(steps) => out.open = Some(Open::new(what, None, key, seeds, window, steps)),
            None => out.spent.push(key),
        }
    }
    out
}

/// The requests of a top-up of `seeds`: the radios of the best loved, the
/// similar artists of their commonest artists; `None` when there is
/// nothing to ask.
fn steps_of(lib: &Library, snap: &super::Snapshot, seeds: &[TrackId], yt: bool, dz: bool) -> Option<VecDeque<Step>> {
    // The best loved seeds first; their commonest artists.
    let model = &snap.model;
    let now = model.now();
    let mut loved: Vec<(f32, TrackId)> = seeds.iter().map(|t| (model.position(*t).map_or(0.0, |i| model.tracks[i].aff_long(now)), *t)).collect();
    loved.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut steps = VecDeque::new();
    for &(_, t) in loved.iter().take(RADIO_SEEDS) {
        let refs = lib.stream_refs(t).unwrap_or_default();
        let id_on = |s: Service| refs.iter().find(|r| r.id.service == s).map(|r| r.id.id.clone());
        let step = match (id_on(Service::Youtube), id_on(Service::Deezer)) {
            (Some(id), _) if yt => Some(Step::Radio { seed: t, service: Service::Youtube, id }),
            (_, Some(id)) if dz => Some(Step::Radio { seed: t, service: Service::Deezer, id }),
            _ => lib.get_track(t).ok().flatten().map(|row| {
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
            let known = lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default();
            match known.iter().find(|r| r.service == Service::Deezer) {
                Some(r) => steps.push_back(Step::Similar { seeds, id: r.id.clone() }),
                None => {
                    if let Some(name) = lib.get_artist(a).ok().flatten().map(|x| x.name)
                        && !nikgob_core::streaming::more_artists_note(&name)
                    {
                        steps.push_back(Step::FindArtist { artist: a, seeds, name });
                    }
                }
            }
        }
    }
    (!steps.is_empty()).then_some(steps)
}

impl Open {
    fn new(what: String, wave: Option<(u64, String)>, token: String, mut seeds: Vec<TrackId>, window: (UnixMs, u32), steps: VecDeque<Step>) -> Self {
        seeds.sort_unstable();
        Self { what, wave, token, seeds, window, steps, found: Vec::new(), requests: 0 }
    }
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
        // Only while the mixes play new music at all, or a wave asks.
        let mixes = {
            let p = &self.state.read().prefs;
            p.mixes_with_new && super::new_share(p, v::RadioKind::Mix) > 0.0
        };
        if self.recs.radio.engine().and_then(|e| e.wave_seed()).is_none() {
            self.recs.niche.wave.clear();
            if !self.recs.niche.wave_kept.is_empty() {
                self.recs.niche.wave_kept.clear();
                self.recs.niche.topped_gen += 1;
            }
        }
        if crate::quiet::busy() || self.recs.niche.planning || (!mixes && self.recs.niche.wave.is_empty() && self.recs.niche.open.is_none()) {
            return;
        }
        // A new day starts with none of its requests (today's, read from
        // the cache with the first plan after a start).
        let (day, _) = local_day();
        if !self.recs.niche.day.is_empty() && self.recs.niche.day != day {
            (self.recs.niche.day, self.recs.niche.day_requests) = (day, 0);
        }
        if self.recs.niche.open.is_none() {
            self.niche_plan(mixes);
            return;
        }
        self.niche_step();
    }

    /// The playing wave's filter of `token` has few new songs left: its
    /// library songs `seeds` (the best loved first) are to be topped up —
    /// planned on the next tick (see the module docs), which tells the wave
    /// what a top-up of it kept lately, or that none can come (no service,
    /// a budget spent). The token; `None` without songs to look around.
    pub(crate) fn niche_want_wave(&mut self, run_id: u64, token: String, mut seeds: Vec<TrackId>) -> Option<String> {
        if seeds.is_empty() {
            return None;
        }
        seeds.sort_unstable();
        let n = &mut self.recs.niche;
        // A filter its pool did not take yet (or of other songs): the pool
        // takes what a top-up of it kept at its next picks.
        if !n.wave_kept.iter().any(|x| x.0 == token && same_seeds(&x.1, &seeds)) {
            n.topped_gen += 1;
        }
        n.wave_kept.retain(|x| x.0 != token);
        n.wave_kept.push((token.clone(), seeds.clone()));
        while n.wave_kept.len() > WAVE_WANTS {
            n.wave_kept.remove(0);
        }
        if let Some(open) = n.open.as_mut()
            && open.wave.as_ref().is_some_and(|(_, t)| t == &token)
        {
            open.wave = Some((run_id, token.clone()));
            return Some(token);
        }
        if !n.wave.iter().any(|x| x.0 == run_id && x.1 == token && x.2 == seeds) {
            n.wave.retain(|x| x.1 != token);
            log::info!("niche: the wave asks for more new songs of {token}");
            n.wave.push((run_id, token.clone(), seeds));
            while n.wave.len() > WAVE_WANTS {
                n.wave.remove(0);
            }
        }
        Some(token)
    }

    /// Plans the top-up of the first niche whose songs are stale — the
    /// wave's filters first, then the home's niches (only when `mixes`
    /// play new music) — on the picks' thread ([`plan`]); its answer is
    /// [`NicheMsg::Planned`]. One plan at a time.
    fn niche_plan(&mut self, mixes: bool) {
        let Some(snap) = self.snapshot() else { return };
        let (yt, dz) = (self.service(Service::Youtube).is_some(), self.service(Service::Deezer).is_some());
        let home: Vec<(String, String, Vec<TrackId>)> = match self.recs.home.as_ref() {
            Some(h) if mixes => h.lanes.iter().filter(|l| !l.niche.is_empty()).filter_map(|l| Some((format!("{:?}", l.key), key_of(l.key)?, l.niche.clone()))).collect(),
            _ => Vec::new(),
        };
        let (today, _) = local_day();
        let n = &mut self.recs.niche;
        n.spent.retain(|s| s.1.elapsed() < RETRY);
        let input = PlanInput {
            lib: self.lib.clone(),
            snap,
            day: (n.day != today).then_some(today),
            day_requests: n.day_requests,
            yt,
            dz,
            spent: n.spent.iter().map(|s| s.0.clone()).collect(),
            wave: std::mem::take(&mut n.wave),
            home,
        };
        if input.wave.is_empty() && input.home.is_empty() && input.day.is_none() {
            return;
        }
        n.planning = true;
        let tx = self.recs.tx.clone();
        let wave: Vec<(u64, String)> = input.wave.iter().map(|w| (w.0, w.1.clone())).collect();
        let sent = self.recs.picker.send(super::picker::Lane::Niche, move || {
            let tokens: Vec<(u64, String)> = input.wave.iter().map(|w| (w.0, w.1.clone())).collect();
            // A plan that fails lets its wave's filters go (never a plan
            // waited for forever).
            let planned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| plan(input))).unwrap_or_else(|_| {
                log::error!("niche: the plan failed");
                Plan { day: None, open: None, spent: Vec::new(), ended: tokens.into_iter().map(|(id, t)| (id, t, 0)).collect(), rest: Vec::new() }
            });
            let _ = tx.send(Internal::Recs(Msg::Niche(NicheMsg::Planned(Box::new(planned)))));
        });
        if let Err(e) = sent {
            log::warn!("niche: cannot plan: {e}");
            self.recs.niche.planning = false;
            for (run_id, token) in wave {
                self.wave_refill_ended(run_id, &token, 0);
            }
        }
    }

    /// [`NicheMsg::Planned`]: the top-up planned opens and its first
    /// request goes; the wave's filters let go are told.
    fn niche_planned(&mut self, p: Plan) {
        let n = &mut self.recs.niche;
        n.planning = false;
        if let Some(day) = p.day {
            (n.day, n.day_requests) = day;
        }
        let now = Instant::now();
        n.spent.extend(p.spent.into_iter().map(|k| (k, now)));
        // Those not planned yet go before the wants since (older).
        if !p.rest.is_empty() {
            let since = std::mem::replace(&mut n.wave, p.rest);
            for w in since {
                n.wave.retain(|x| x.1 != w.1);
                n.wave.push(w);
            }
            while n.wave.len() > WAVE_WANTS {
                n.wave.remove(0);
            }
        }
        let opened = p.open.is_some();
        if let Some(open) = p.open {
            log::info!("niche: topping up {} from {} seeds, {} requests planned", open.what, open.seeds.len(), open.steps.len());
            self.recs.niche.open = Some(open);
        }
        for (run_id, token, kept) in p.ended {
            if kept > 0 {
                log::info!("niche: the wave's {token} was topped up lately, with {kept} songs");
            }
            self.wave_refill_ended(run_id, &token, kept);
        }
        if opened {
            self.niche_step();
        }
    }

    /// Whether a top-up of one of `run_id`'s filters is still to come:
    /// planned or wanted.
    pub(crate) fn niche_wave_pending(&self, run_id: u64) -> bool {
        let n = &self.recs.niche;
        n.planning || n.wave.iter().any(|w| w.0 == run_id) || n.open.as_ref().is_some_and(|o| o.wave.as_ref().is_some_and(|(id, _)| *id == run_id))
    }

    /// At shutdown: the budgets as they stand, written now (a write still
    /// waiting on the picks' thread may never run).
    pub(crate) fn niche_flush(&self) {
        let n = &self.recs.niche;
        if n.day.is_empty() {
            return;
        }
        let window = n.open.as_ref().map(|o| (o.token.as_str(), o.window));
        keep_budgets(&self.lib, &(n.day.clone(), n.day_requests), window);
    }

    /// Runs `work` (the library's writes of the top-ups) on the picks'
    /// thread after what was sent there before.
    fn niche_write(&self, work: impl FnOnce() + Send + 'static) {
        if let Err(e) = self.recs.picker.send(super::picker::Lane::Niche, work) {
            log::warn!("niche: cannot keep: {e}");
        }
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
        // Not while the window is idle — unless a wave waits for this
        // top-up to play at all.
        let for_wave = self.recs.niche.open.as_ref().is_some_and(|o| o.wave.is_some()) && self.wave_waits();
        if self.streaming.search.active() || (self.streams_idle() && !for_wave) || self.recs.discover_busy() {
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
            let next = if open.window.1 >= BUDGET || over { None } else { open.steps.pop_front() };
            let Some(step) = next else {
                self.niche_finish();
                return;
            };
            let service = step.service();
            let Some(svc) = self.service(service) else { continue };
            // Its pause, or the listener's turn on YouTube Music.
            if let Some(left) = self.background_wait(service) {
                if let Some(open) = self.recs.niche.open.as_mut() {
                    open.steps.push_front(step);
                }
                self.niche_wait(left + crate::streams::wake_jitter());
                return;
            }
            let mut window = None;
            if let Some(open) = self.recs.niche.open.as_mut() {
                open.requests += 1;
                open.window.1 += 1;
                window = Some((open.token.clone(), open.window));
            }
            let n = &mut self.recs.niche;
            n.day_requests += 1;
            n.busy = true;
            let (lib, day) = (self.lib.clone(), (n.day.clone(), n.day_requests));
            self.niche_write(move || keep_budgets(&lib, &day, window.as_ref().map(|(k, w)| (k.as_str(), *w))));
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
            NicheMsg::Planned(p) => {
                self.niche_planned(*p);
                return;
            }
            NicheMsg::Kept { wave, found } => {
                self.niche_kept(wave, found);
                return;
            }
        };
        self.recs.niche.busy = false;
        let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
        match result {
            Err(StreamError::RateLimited { retry_after }) => {
                let wait = retry_after.or_else(|| self.pause_left(step.service())).unwrap_or(self.streaming.schedule.rate_limit_unknown);
                log::info!("niche: rate limited by {}, again in {} s", step.service(), wait.as_secs());
                // The request counts: it was sent.
                if let Some(open) = self.recs.niche.open.as_mut() {
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
                    let (lib, remote) = (self.lib.clone(), hit.id.clone());
                    self.niche_write(move || {
                        let _ = lib.set_external_id(EntityKind::Artist, &remote, *artist.as_uuid());
                    });
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
        let Some(mut open) = self.recs.niche.open.take() else { return };
        self.recs.niche.spent.push((open.token.clone(), Instant::now()));
        let wave = open.wave.take();
        let Some(snap) = self.snapshot() else {
            if let Some((run_id, t)) = wave {
                self.wave_refill_ended(run_id, &t, 0);
            }
            return;
        };
        let mut tracks: Vec<(StreamTrack, Vec<TrackId>)> = open.found.into_iter().filter(|(t, _)| snap.is_new(t)).collect();
        // The songs most seeds found first.
        tracks.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
        tracks.truncate(KEEP);
        log::info!("niche: {} topped up with {} songs in {} requests", open.what, tracks.len(), open.requests);
        let found_n = tracks.len();
        let found = NicheFound { at: now_ms(), seeds: open.seeds, tracks };
        let key = open.token;
        self.streaming.learn(&found.tracks.iter().map(|t| t.0.clone()).collect::<Vec<_>>());
        // Kept on the picks' thread; the mixes and the wave take it once it
        // is ([`NicheMsg::Kept`]).
        let (lib, tx) = (self.lib.clone(), self.recs.tx.clone());
        let fallback = wave.clone();
        let sent = self.recs.picker.send(super::picker::Lane::Niche, move || {
            super::cache_write(&lib, &key, found.at, &found);
            let _ = tx.send(Internal::Recs(Msg::Niche(NicheMsg::Kept { wave, found: found_n })));
        });
        if let Err(e) = sent {
            log::warn!("niche: cannot keep: {e}");
            if let Some((run_id, t)) = fallback {
                self.wave_refill_ended(run_id, &t, 0);
            }
        }
    }

    /// [`NicheMsg::Kept`]: the songs a top-up found are in the cache — the
    /// mixes are built again with them, the wave's pool takes them.
    fn niche_kept(&mut self, wave: Option<(u64, String)>, found: usize) {
        if found > 0 {
            let after = self.streaming.schedule.recs_debounce;
            self.recs_dirty(after);
            if wave.is_some() {
                self.recs.niche.topped_gen += 1;
            }
        }
        if let Some((run_id, t)) = wave {
            self.wave_refill_ended(run_id, &t, found);
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

    /// What the wave's filters were topped up with is found by their
    /// tokens, for the songs they were asked for (most of them), while it
    /// stands; another filter's is not.
    #[test]
    fn the_waves_top_ups_are_kept_by_their_filters() {
        let lib = Library::open_in_memory().unwrap();
        let seeds: Vec<TrackId> = (0..4).map(|_| TrackId::new()).collect();
        let found = NicheFound { at: now_ms(), seeds: seeds.clone(), tracks: vec![(st("1", "Calm", "Someone"), vec![seeds[0]])] };
        super::super::cache_write(&lib, &wave_key("calm.all"), found.at, &found);
        let got = wave_topped(&lib, &[("calm.all".to_owned(), seeds[..3].to_vec()), ("lively.all".to_owned(), seeds.clone())]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tracks.len(), 1);
        let old = NicheFound { at: now_ms() - TTL_MS - 1, ..found };
        super::super::cache_write(&lib, &wave_key("calm.all"), old.at, &old);
        assert!(wave_topped(&lib, &[("calm.all".to_owned(), seeds)]).is_empty(), "stale");
    }

    /// The plan (made off the worker) tells a wave's filter topped up lately
    /// what it kept, whatever the budgets — not «nothing» — and lets go one
    /// that cannot be asked (left for a while, over today's budget).
    #[test]
    fn a_plan_tells_the_wave_what_its_filter_kept_lately() {
        let lib = Library::open_in_memory().unwrap();
        let seeds: Vec<TrackId> = (0..4).map(|_| TrackId::new()).collect();
        let found = NicheFound { at: now_ms(), seeds: seeds.clone(), tracks: vec![(st("1", "Calm", "Someone"), vec![seeds[0]]), (st("2", "Still", "Else"), vec![seeds[1]])] };
        super::super::cache_write(&lib, &wave_key("calm.all"), found.at, &found);
        let input = |day_requests: u32| PlanInput {
            lib: lib.clone(),
            snap: super::super::wave::empty_snapshot(),
            day: Some("2026-11-13".into()),
            day_requests,
            yt: true,
            dz: false,
            spent: vec![wave_key("lively.all")],
            wave: vec![(1, "lively.all".into(), seeds.clone()), (1, "calm.all".into(), seeds.clone())],
            home: Vec::new(),
        };
        let p = plan(input(0));
        assert_eq!(p.day, Some(("2026-11-13".into(), 0)), "the day's requests read with the plan");
        assert_eq!(p.ended, vec![(1, "calm.all".to_owned(), 2), (1, "lively.all".to_owned(), 0)]);
        assert!(p.open.is_none() && p.rest.is_empty());
        super::super::cache_write(&lib, DAY_KEY, now_ms(), &("2026-11-13".to_owned(), DAY_BUDGET));
        let p = plan(input(0));
        assert_eq!(p.day, Some(("2026-11-13".into(), DAY_BUDGET)));
        assert_eq!(p.ended, vec![(1, "calm.all".to_owned(), 2), (1, "lively.all".to_owned(), 0)], "kept songs count over the budget too");
    }

    /// The budgets kept are never lowered by a write that comes late (one
    /// queued before the one at shutdown); a new day or window starts anew.
    #[test]
    fn the_budgets_kept_never_go_down() {
        let lib = Library::open_in_memory().unwrap();
        let read_day = || super::super::cache_read::<(String, u32)>(&lib, DAY_KEY);
        let read_window = || super::super::cache_read::<(UnixMs, u32)>(&lib, &format!("niche.x{REQ}"));
        keep_budgets(&lib, &("d1".into(), 5), Some(("niche.x", (100, 4))));
        keep_budgets(&lib, &("d1".into(), 3), Some(("niche.x", (100, 2))));
        assert_eq!(read_day(), Some(("d1".into(), 5)));
        assert_eq!(read_window(), Some((100, 4)));
        keep_budgets(&lib, &("d1".into(), 6), None);
        assert_eq!(read_day(), Some(("d1".into(), 6)));
        keep_budgets(&lib, &("d2".into(), 1), Some(("niche.x", (200, 1))));
        assert_eq!(read_day(), Some(("d2".into(), 1)));
        assert_eq!(read_window(), Some((200, 1)));
    }

    /// A top-up is for its niche: most of the seeds the same, not a mix of
    /// the key drawn of other songs.
    #[test]
    fn a_top_up_is_for_its_seeds() {
        let t: Vec<TrackId> = (0..6).map(|_| TrackId::new()).collect();
        assert!(same_seeds(&t[..4], &t[..4]));
        assert!(same_seeds(&t[..4], &t[1..4]));
        assert!(!same_seeds(&t[..3], &t[3..6]));
        assert!(!same_seeds(&[], &[]));
    }
}
