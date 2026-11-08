//! The radio queue (docs/stage4.md §4): «Радио по треку», an artist's
//! radio and the endless mixes (moods, style mixes, «Новое для тебя», the
//! daylist) play as a queue that extends itself. The daylist starts with
//! the picks the home shows ([`Worker::start_radio_picked`]).
//!
//! * The queue holds the engine's [`Engine`] state. When three tracks or
//!   fewer are left, the next ten are picked on a thread of their own from
//!   the current [`super::Snapshot`] (playback goes on meanwhile); what
//!   ended while they were picked is learnt after.
//! * How each track ended teaches the session: a complete pulls the next
//!   picks towards its sound, a skip in the first 30 s pushes them away
//!   ([`Engine::feedback`]).
//! * New music by «Новое в радио» ([`v::Prefs::radio_new_share`]): for a
//!   track, YouTube Music's radio of it and Deezer's radio of its artist;
//!   for an artist, Deezer's radio of the artist (YouTube Music's without
//!   Deezer); for a service's track, its own service's radio or related
//!   list; the mixes take the discoveries. Background calls, each answer
//!   kept a day in the library's cache, nothing asked of a service that is
//!   paused by a rate limit. New music that comes after the queue was
//!   picked joins the queued tracks at once, its share of them evenly
//!   between ([`RadioState::take_new`](nikgob_recs::RadioState::take_new));
//!   with fewer than [`REFILL_AT`] candidates left, more is asked around
//!   the last tracks the radio played to the end. Each extension logs the
//!   share of new music queued so far. A new pick plays from the stream
//!   like any search result.
//! * «Подмешивать случайное» and «Преимущественно новые песни»
//!   ([`v::Prefs::radio_explore`], [`v::Prefs::radio_unheard`]) go to the
//!   engine before each extension and each splice of new music
//!   ([`Engine::set_explore`], [`Engine::set_unheard`]); a change holds from
//!   the next ones, what is queued stays.

//! * The radio is saved in `session.json` with the queue, so it goes on
//!   after a restart. Playing anything else (any command that replaces the
//!   queue) ends it; adding to the queue does not.
//! * «Автовоспроизведение» ([`v::Prefs::autoplay`]): when the last track of
//!   any other queue plays, a radio of that track takes the queue over and
//!   appends to it, never picking what the queue already had. After an
//!   album the album's world goes on first ([`v::Prefs::autoplay_continue_world`],
//!   [`crate::home_v2::world_next`]): its next album's tracks join the
//!   queue, album by album, and the radio comes past the world's end: a
//!   radio of the taste profile the session was pure in (any profile after
//!   a mixed one, [`Engine::continuing`]).
//! * Where its tracks come from is a hard rule: under «Миксы только из
//!   скачанного» nothing without a file is queued, without new music no
//!   stream pick is; switching either drops the radio's upcoming tracks
//!   that no longer fit and picks again.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::model::{ArtistId, EventContext, EventOrigin, TrackId};
use nikgob_core::streaming::{EntityKind, RemoteId, SearchKind, Service, StreamError, StreamService, StreamTrack};
use nikgob_recs::{Item, NewTrack, Outcome, Pick, RadioParams, RadioSeed, RadioState as Engine, artist_key};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::Msg;
use crate::Worker;
use crate::import::Internal;

/// The queue extends itself when this many tracks or fewer are left…
const EXTEND_AT: usize = 3;
/// …by this many.
const EXTEND_BY: usize = 10;
/// New-music answers of the services kept this long.
const NEW_TTL_MS: i64 = 24 * 3600 * 1000;
/// With fewer new candidates left than this the pool is refilled around
/// what the radio played to the end…
const REFILL_AT: usize = 6;
/// …from this many of its last tracks at a time.
const REFILL_FROM: usize = 2;

/// One radio playing.
pub(crate) struct RadioRun {
    id: u64,
    state: Engine,
    title: String,
    ctx: EventContext,
    /// Seeds the engine's jitter.
    seed: u64,
    /// The queue it plays in; `None` while its first tracks are picked.
    queue_gen: Option<u64>,
    /// Stream picks by the track they play as.
    streams: HashMap<TrackId, RemoteId>,
    /// New-music candidates by id (they play from the stream).
    pool: HashMap<RemoteId, StreamTrack>,
    extending: bool,
    /// An extension waits for the first snapshot.
    want: bool,
    /// Learnt while an extension ran: applied to its result.
    pending: Vec<(Item, Outcome)>,
    pending_new: Vec<NewTrack>,
    /// The queue ran out while an extension ran: play on when it comes.
    resume: bool,
    /// Started by «Автовоспроизведение» to continue another queue.
    autoplay: bool,
    /// The queue entries it added (a change of source drops the ones that
    /// no longer fit).
    added: HashSet<u64>,
    /// Its tracks heard out, the last last (new music is asked around
    /// them when the pool runs low).
    heard: Vec<TrackId>,
    /// The songs new music was asked around already.
    asked: HashSet<String>,
    /// Picks queued so far and how many of them new.
    queued: (usize, usize),
    /// Why each new candidate is offered: where it was found.
    via: HashMap<RemoteId, String>,
}

/// What `session.json` keeps of a radio.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct RadioSave {
    state: Engine,
    title: String,
    context: EventContext,
    seed: u64,
    streams: Vec<(TrackId, RemoteId)>,
    pool: Vec<StreamTrack>,
    #[serde(default)]
    autoplay: bool,
}

#[derive(Default)]
pub(crate) struct RadioState {
    run: Option<RadioRun>,
    seq: u64,
    /// The queue generation autoplay found nothing for: not tried again
    /// until the queue is replaced.
    autoplay_dry: Option<u64>,
}

pub(crate) enum RadioMsg {
    Extended {
        id: u64,
        state: Box<Engine>,
        picks: Vec<Pick>,
    },
    NewFound {
        id: u64,
        tracks: Vec<StreamTrack>,
        /// Artists matched in a service on the way.
        artists: Vec<(ArtistId, RemoteId)>,
        /// Answers to keep: cache key and tracks.
        answers: Vec<(String, Vec<StreamTrack>)>,
        /// Where each was found ([`nikgob_recs::explain::new_found`]).
        via: Vec<(RemoteId, String)>,
    },
}

/// One new-music call for a radio.
enum Call {
    /// YouTube Music's radio of a song.
    Radio(String),
    /// A service's related list of a track (Deezer: the radio of its artist).
    Related(Service, String),
    /// A service's radio of an artist; without its id there, looked up by
    /// name first.
    ArtistRadio {
        service: Service,
        artist: ArtistId,
        id: Option<String>,
        name: String,
    },
}

fn cache_key(service: Service, what: &str, id: &str) -> String {
    format!("radio_new.{}.{what}.{id}", service.as_str())
}

/// Whether a radio pick may be queued under `prefs`: nothing without a
/// file under «Миксы только из скачанного», and no stream pick without new
/// music, unless the radio is about new music itself (`new_seed`: «Новое
/// для тебя», a service's song). (The engine already picks so; this holds
/// while the snapshot it picked from is older than the prefs.)
pub(super) fn radio_allows(track: TrackId, stream: bool, new_seed: bool, prefs: &v::Prefs, snap: Option<&super::Snapshot>, kind: v::RadioKind) -> bool {
    if stream && !streams_allowed(new_seed, prefs, kind) {
        return false;
    }
    !prefs.mixes_downloaded_only || snap.and_then(|s| s.catalog.get(track)).is_some_and(|t| t.local)
}

/// New music from the services may be queued.
fn streams_allowed(new_seed: bool, prefs: &v::Prefs, kind: v::RadioKind) -> bool {
    !prefs.mixes_downloaded_only && (new_seed || super::new_share(prefs, kind) > 0.0)
}

/// The kind of a radio of `seed` (the share of new music it remembers).
pub(super) fn kind_of(seed: &RadioSeed, autoplay: bool) -> v::RadioKind {
    match seed {
        _ if autoplay => v::RadioKind::Autoplay,
        RadioSeed::Track(_) | RadioSeed::Stream { .. } => v::RadioKind::Track,
        RadioSeed::Artist(_) => v::RadioKind::Artist,
        _ => v::RadioKind::Mix,
    }
}

impl RadioRun {
    fn kind(&self) -> v::RadioKind {
        kind_of(&self.state.seed, self.autoplay)
    }
}

/// The radio is about new music whatever «Новое в радио» says.
fn new_seed(seed: &RadioSeed) -> bool {
    matches!(seed, RadioSeed::Discover | RadioSeed::Stream { .. })
}

/// What a radio of `seed` plays, as the history's `context_id`.
fn seed_context_id(seed: &RadioSeed, autoplay: bool) -> String {
    let what = match seed {
        RadioSeed::Track(t) => format!("track:{t}"),
        RadioSeed::Stream { id, .. } => format!("stream:{id}"),
        RadioSeed::Artist(a) => format!("artist:{a}"),
        RadioSeed::Mood(m) => format!("mood:{m:?}"),
        RadioSeed::Style { index, .. } => format!("style:{index}"),
        RadioSeed::Discover => "discover".to_owned(),
        RadioSeed::Daylist { part, .. } => format!("daylist:{part:?}"),
    };
    format!("{}:{what}", if autoplay { "autoplay" } else { "radio" })
}

/// Who chose what a radio of `seed` adds: autoplay, a mix or a radio.
fn origin_of(state: &Engine, autoplay: bool) -> EventOrigin {
    if autoplay {
        EventOrigin::Autoplay
    } else if state.is_mix() {
        EventOrigin::Mix
    } else {
        EventOrigin::Radio
    }
}

/// The queue's context line: «Играет из радио «Кино»» / «… микса «Спокойное»».
fn context_of(title: &str) -> String {
    match title.strip_prefix("Радио · ") {
        Some(rest) => format!("радио «{rest}»"),
        None => format!("микса «{title}»"),
    }
}

async fn fetch(svc: Arc<dyn StreamService>, call: &Call) -> (Result<Vec<StreamTrack>, StreamError>, Option<(ArtistId, RemoteId)>) {
    match call {
        Call::Radio(id) => (svc.radio(id).await, None),
        Call::Related(_, id) => (svc.related(id).await, None),
        Call::ArtistRadio { artist, id, name, .. } => {
            let (id, matched) = match id {
                Some(id) => (id.clone(), None),
                None => {
                    let key = artist_key(name);
                    let hits = match svc.search_kind(name, SearchKind::Artists).await {
                        Ok(s) => s.artists,
                        Err(e) => return (Err(e), None),
                    };
                    match hits.into_iter().find(|a| artist_key(&a.name) == key) {
                        Some(a) => (a.id.id.clone(), Some((*artist, a.id))),
                        None => return (Ok(Vec::new()), None),
                    }
                }
            };
            (svc.artist_radio(&id).await, matched)
        }
    }
}

impl Worker {
    /// Who chose the queue entry `entry`, when the playing radio added it.
    pub(crate) fn radio_origin(&mut self, entry: u64) -> Option<EventOrigin> {
        if !self.radio_alive() {
            return None;
        }
        let run = self.recs.radio.run.as_ref()?;
        run.added.contains(&entry).then(|| origin_of(&run.state, run.autoplay))
    }

    /// Where the queue entry `entry` plays and what plays it, when the
    /// playing radio (or autoplay) added it.
    pub(crate) fn radio_context(&mut self, entry: u64) -> Option<(EventContext, String)> {
        if !self.radio_alive() {
            return None;
        }
        let run = self.recs.radio.run.as_ref()?;
        run.added.contains(&entry).then(|| (run.ctx, seed_context_id(&run.state.seed, run.autoplay)))
    }

    /// `Command::StartRadio`.
    pub(crate) fn start_radio_cmd(&mut self, seed: v::RadioSeed) {
        match seed {
            v::RadioSeed::Track(t) => {
                let title = self.lib.get_track(t).ok().flatten().map_or_else(String::new, |t| t.title);
                self.start_radio(RadioSeed::Track(t), format!("Радио · {title}"), EventContext::Radio, Some(t));
            }
            v::RadioSeed::Artist(a) => {
                let name = self.lib.get_artist(a).ok().flatten().map_or_else(String::new, |a| a.name);
                self.start_radio(RadioSeed::Artist(a), format!("Радио · {name}"), EventContext::Radio, None);
            }
            v::RadioSeed::Stream(r) => {
                let Some(t) = self.stream_track(&r) else {
                    log::warn!("radio of an unknown stream track {r}");
                    self.toast("Нечего играть", None);
                    return;
                };
                let Some(id) = self.upsert_streams(std::slice::from_ref(&t)).first().copied() else { return };
                let title = format!("Радио · {}", t.title);
                // A track of the library (liked, in a playlist) is its own seed.
                let in_catalog = self.snapshot().is_some_and(|s| s.catalog.get(id).is_some());
                let seed = if in_catalog {
                    RadioSeed::Track(id)
                } else {
                    let snap = self.snapshot();
                    let artists = t
                        .artists
                        .iter()
                        .filter_map(|a| snap.as_ref()?.artist_keys.get(&artist_key(&a.name)).copied())
                        .collect();
                    let vector = self.analysis.index().read().get(id).copied();
                    RadioSeed::Stream { id: r, vector, artists }
                };
                self.start_radio(seed, title, EventContext::Radio, Some(id));
            }
        }
    }

    /// Starts a radio: `first` plays at once (a track's radio), the rest
    /// comes as it is picked.
    pub(crate) fn start_radio(&mut self, seed: RadioSeed, title: String, ctx: EventContext, first: Option<TrackId>) {
        // The seed track is the user's choice; the rest the radio's.
        let state = Engine::new(seed);
        self.user_switch(if first.is_some() { EventOrigin::Manual } else { origin_of(&state, false) });
        let r = &mut self.recs.radio;
        r.seq += 1;
        let run = RadioRun {
            id: r.seq,
            state,
            title,
            ctx,
            seed: fastrand::u64(..),
            queue_gen: None,
            streams: HashMap::new(),
            pool: HashMap::new(),
            extending: false,
            want: false,
            pending: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: false,
            added: HashSet::new(),
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
        };
        log::info!("radio «{}» starts ({:?})", run.title, run.state.seed);
        let context = context_of(&run.title);
        r.run = Some(run);
        self.radio_add_discoveries();
        self.radio_fetch_new();
        match first {
            Some(t) => {
                self.new_list();
                self.queue.set_ordered(&[t], 0, Some(context));
                self.play_context = ctx;
                self.play_context_id = self.recs.radio.run.as_ref().map(|r| seed_context_id(&r.state.seed, false));
                let generation = self.queue.generation();
                if let Some(run) = self.recs.radio.run.as_mut() {
                    run.queue_gen = Some(generation);
                }
                if let Some(e) = self.queue.current() {
                    self.start(e);
                }
            }
            None => {
                self.radio_extend();
                self.publish_player();
            }
        }
    }

    /// Plays a radio picked already (the home's daylist): `picks` first,
    /// then on from `state`, which served them; `streams` are the stream
    /// picks' tracks.
    pub(crate) fn start_radio_picked(
        &mut self,
        state: Engine,
        picks: Vec<Pick>,
        streams: Vec<StreamTrack>,
        title: String,
        ctx: EventContext,
    ) {
        self.user_switch(origin_of(&state, false));
        let r = &mut self.recs.radio;
        r.seq += 1;
        let id = r.seq;
        self.streaming.learn(&streams);
        r.run = Some(RadioRun {
            id,
            state: state.clone(),
            title,
            ctx,
            seed: fastrand::u64(..),
            queue_gen: None,
            streams: HashMap::new(),
            pool: streams.into_iter().map(|t| (t.id.clone(), t)).collect(),
            // As if its first picks were on their way: new music found
            // meanwhile joins the state they come with.
            extending: true,
            want: false,
            pending: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: false,
            added: HashSet::new(),
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
        });
        log::info!("radio «{}» starts from its picks ({:?})", self.recs.radio.run.as_ref().map_or("", |r| &r.title), state.seed);
        self.radio_add_discoveries();
        self.radio_fetch_new();
        self.radio_extended(id, state, picks);
    }

    /// The radio still plays its queue (any other playback ended it).
    fn radio_alive(&mut self) -> bool {
        let generation = self.queue.generation();
        let r = &mut self.recs.radio;
        match &r.run {
            Some(run) if run.queue_gen.is_none_or(|g| g == generation) => true,
            Some(run) => {
                log::info!("radio «{}» ends: the queue was replaced", run.title);
                r.run = None;
                false
            }
            None => false,
        }
    }

    /// What the queue view says about the radio.
    pub(crate) fn radio_view(&mut self) -> Option<v::RadioView> {
        if !self.radio_alive() {
            return None;
        }
        self.recs.radio.run.as_ref().map(|r| v::RadioView {
            title: r.title.clone(),
            extending: r.extending || r.want,
            autoplay: r.autoplay,
            kind: r.kind(),
        })
    }

    /// Three tracks or fewer left: the next ones are picked.
    pub(crate) fn radio_top_up(&mut self) {
        if !self.radio_alive() {
            self.autoplay_start();
            return;
        }
        let left = self.queue.upcoming().len();
        let Some(run) = &self.recs.radio.run else { return };
        if run.extending || (run.queue_gen.is_some() && left > EXTEND_AT) {
            return;
        }
        self.radio_extend();
    }

    /// «Автовоспроизведение»: the last track of a queue that is no radio
    /// plays (repeat off), so a radio of it continues the queue.
    fn autoplay_start(&mut self) {
        if !self.state.read().prefs.autoplay || self.queue.repeat != v::Repeat::Off || !self.queue.upcoming().is_empty() {
            return;
        }
        let Some(e) = self.playing else { return };
        let generation = self.queue.generation();
        if self.recs.radio.autoplay_dry == Some(generation) {
            return;
        }
        // After an album its world goes on first, album by album.
        let (continue_world, local_only) = {
            let p = &self.state.read().prefs;
            (p.autoplay_continue_world, p.mixes_downloaded_only)
        };
        if continue_world && self.play_context == EventContext::Album {
            let queued: HashSet<TrackId> = self.queue.tracks().into_iter().collect();
            let next = crate::home_v2::world_next(&self.lib, e.track, &queued, local_only);
            if !next.is_empty() {
                log::info!("autoplay: the world goes on with {} tracks of its next album", next.len());
                let shuffled = self.shuffle_radio();
                let added = self.queue.enqueue_path(&next, shuffled);
                self.autoplay_entries.extend(added.iter().map(|e| e.id));
                self.preload_next();
                self.publish_player();
                return;
            }
        }
        let title = self.lib.get_track(e.track).ok().flatten().map_or_else(String::new, |t| t.title);
        // It keeps to the profile the session is pure in (the album's,
        // after its world), or to none after a mixed queue.
        let mut state = Engine::new(RadioSeed::Track(e.track)).continuing();
        // A stream the catalog lacks (played from its album's page) is
        // known by its artists, sound and title.
        if let Some(facts) = self.seed_facts(e.track) {
            state = state.with_seed_facts(facts);
        }
        // Nothing the queue had comes back.
        state.mark_served(self.queue.tracks().into_iter().map(Item::Track));
        let r = &mut self.recs.radio;
        r.seq += 1;
        r.run = Some(RadioRun {
            id: r.seq,
            state,
            title: format!("Похожее на «{title}»"),
            ctx: EventContext::Radio,
            seed: fastrand::u64(..),
            queue_gen: Some(generation),
            streams: HashMap::new(),
            pool: HashMap::new(),
            extending: false,
            want: false,
            pending: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: true,
            added: HashSet::new(),
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
        });
        log::info!("autoplay: the queue goes on with a radio of «{title}»");
        self.radio_fetch_new();
        self.radio_extend();
    }

    /// What a track the snapshot's catalog lacks is to a radio of it: its
    /// library artists, its sound if analysed, the language its title
    /// shows. None for a track of the catalog.
    fn seed_facts(&self, track: TrackId) -> Option<nikgob_recs::SeedFacts> {
        self.seed_facts_in(&*self.snapshot()?, track)
    }

    fn seed_facts_in(&self, snap: &super::Snapshot, track: TrackId) -> Option<nikgob_recs::SeedFacts> {
        if snap.catalog.get(track).is_some() {
            return None;
        }
        let t = self.lib.get_track(track).ok().flatten()?;
        let mut artists: Vec<ArtistId> = t.artist_credit.artist_ids().collect();
        for a in &t.artist_credit.0 {
            if let Some(id) = snap.artist_keys.get(&artist_key(&a.name)).copied()
                && !artists.contains(&id)
            {
                artists.push(id);
            }
        }
        // The index of the sound may not have it yet: the library has.
        let vector = self.analysis.index().read().get(track).copied().or_else(|| {
            let a = self.lib.get_analysis(track).ok().flatten()?;
            (a.analyzed_with == nikgob_recs::ANALYZED_WITH).then(|| <[f32; nikgob_recs::DIM]>::try_from(a.bliss_vector.as_slice()).ok()).flatten()
        });
        let lane = nikgob_recs::songtype::title_language(&t.title).map(|l| match l {
            nikgob_recs::Lang::Russian => nikgob_recs::SongLane::Russian,
            nikgob_recs::Lang::Japanese => nikgob_recs::SongLane::Japanese,
            nikgob_recs::Lang::English | nikgob_recs::Lang::Spanish | nikgob_recs::Lang::French | nikgob_recs::Lang::German => {
                nikgob_recs::SongLane::Western
            }
            _ => nikgob_recs::SongLane::Unknown,
        });
        Some(nikgob_recs::SeedFacts { artists, vector, lane })
    }

    /// What the radio must not bring back as new music: the tracks of the
    /// queue, of the session, and what started within «Не повторять
    /// сыгранное».
    fn played_lately(&self) -> HashSet<TrackId> {
        let hours = i64::from(self.state.read().prefs.radio_no_repeat_hours);
        let since = nikgob_library::now_ms() - hours * 3_600_000;
        let mut out: HashSet<TrackId> = self.queue.tracks().into_iter().collect();
        out.extend(self.recs.session.tracks().map(|p| p.track));
        for e in self.lib.recent_events(500).unwrap_or_default() {
            if e.ts >= since && e.event_type == nikgob_core::model::EventType::Start {
                out.insert(e.track_id);
            }
        }
        out
    }

    /// «Автовоспроизведение» was switched off: its radio stops picking (what
    /// it queued stays).
    pub(crate) fn autoplay_off(&mut self) {
        if self.recs.radio.run.as_ref().is_some_and(|r| r.autoplay) {
            log::info!("autoplay switched off: its radio ends");
            self.recs.radio.run = None;
            self.publish_player();
        }
    }

    /// «Новое в радио» or «Миксы только из скачанного» changed: the radio's
    /// upcoming tracks that no longer fit go, and it picks again.
    pub(crate) fn radio_source_changed(&mut self) {
        if !self.radio_alive() {
            return;
        }
        let prefs = self.state.read().prefs.clone();
        let snap = self.snapshot();
        let Some(run) = self.recs.radio.run.as_ref() else { return };
        let drop: Vec<u64> = self
            .queue
            .upcoming()
            .iter()
            .filter(|e| run.added.contains(&e.id))
            .filter(|e| {
                !radio_allows(e.track, run.streams.contains_key(&e.track), new_seed(&run.state.seed), &prefs, snap.as_deref(), run.kind())
            })
            .map(|e| e.id)
            .collect();
        log::info!("radio «{}»: the source changed, {} upcoming go", run.title, drop.len());
        for id in drop {
            self.queue.remove(id);
        }
        self.radio_fetch_new();
        self.preload_next();
        self.publish_player();
    }

    /// Picks the next [`EXTEND_BY`] on a thread of its own.
    fn radio_extend(&mut self) {
        let prefs = self.state.read().prefs.clone();
        let Some(kind) = self.recs.radio.run.as_ref().map(RadioRun::kind) else { return };
        let new_share = super::new_share(&prefs, kind);
        let local_only = prefs.mixes_downloaded_only;
        let snap = self.snapshot();
        // What is known of a seed the catalog lacks, once there is a
        // snapshot to tell it by (autoplay may start before the first one).
        let facts = match (&snap, self.recs.radio.run.as_ref()) {
            (Some(s), Some(r)) if !r.state.has_seed_facts() => match r.state.seed {
                RadioSeed::Track(t) => self.seed_facts_in(s, t),
                _ => None,
            },
            _ => None,
        };
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if run.extending {
            return;
        }
        let Some(snap) = snap else {
            // The first build is on its way.
            run.want = true;
            self.recs_dirty(Duration::ZERO);
            return;
        };
        if let Some(f) = facts {
            log::info!("radio «{}»: its seed is beyond the catalog, by its {} artists", run.title, f.artists.len());
            run.state.set_seed_facts(f);
        }
        run.want = false;
        run.extending = true;
        let (id, mut state) = (run.id, run.state.clone());
        state.attune(self.recs.session.clone(), self.recs.weights);
        state.set_no_repeat_ms(i64::from(prefs.radio_no_repeat_hours) * 3_600_000);
        state.set_explore(Some(prefs.radio_explore.fraction()));
        state.set_unheard(prefs.radio_unheard.fraction());
        let params = RadioParams { new_share, seed: run.seed };
        let tx = self.recs.tx.clone();
        let spawned = std::thread::Builder::new().name("nikgob-radio".into()).spawn(move || {
            // The prefs as they are now, even before the snapshot follows.
            let picks = state.extend(&snap.recs().local_only(local_only), EXTEND_BY, &params);
            let msg = super::radio::RadioMsg::Extended {
                id,
                state: Box::new(state),
                picks,
            };
            let _ = tx.send(Internal::Recs(Msg::Radio(msg)));
        });
        if let Err(e) = spawned {
            log::warn!("radio: cannot pick: {e}");
            if let Some(run) = self.recs.radio.run.as_mut() {
                run.extending = false;
            }
        }
    }

    /// The first snapshot is here: a radio waiting for it goes on.
    pub(crate) fn radio_snapshot_ready(&mut self) {
        if self.recs.radio.run.as_ref().is_some_and(|r| r.want) {
            self.radio_add_discoveries();
            self.radio_extend();
        }
    }

    pub(crate) fn radio_msg(&mut self, m: RadioMsg) {
        match m {
            RadioMsg::Extended { id, state, picks } => self.radio_extended(id, *state, picks),
            RadioMsg::NewFound {
                id,
                tracks,
                artists,
                answers,
                via,
            } => {
                let now = nikgob_library::now_ms();
                for (key, tracks) in answers {
                    if let Ok(json) = serde_json::to_string(&tracks) {
                        let _ = self.lib.cache_put(&key, now, &json);
                    }
                }
                for (artist, remote) in artists {
                    let _ = self.lib.set_external_id(EntityKind::Artist, &remote, *artist.as_uuid());
                }
                if let Some(run) = self.recs.radio.run.as_mut().filter(|r| r.id == id) {
                    run.via.extend(via);
                    self.radio_add_new(tracks);
                    self.radio_splice_new();
                }
            }
        }
    }

    fn radio_extended(&mut self, id: u64, mut state: Engine, picks: Vec<Pick>) {
        let alive = self.radio_alive();
        let Some(run) = self.recs.radio.run.as_mut().filter(|r| r.id == id) else { return };
        run.extending = false;
        for (item, outcome) in run.pending.drain(..) {
            state.feedback(item, outcome);
        }
        state.add_new(run.pending_new.drain(..));
        run.state = state;
        if !alive {
            return;
        }
        // Stream picks play as "seen" tracks of their service.
        let prefs = self.state.read().prefs.clone();
        let snap = self.snapshot();
        let wants_new = self.recs.radio.run.as_ref().is_some_and(|r| new_seed(&r.state.seed));
        let kind = self.recs.radio.run.as_ref().map_or(v::RadioKind::Track, RadioRun::kind);
        // Only the first pick of a radio is explained (besides new music):
        // the first that is not its seed.
        let (first_batch, seed_track) = self.recs.radio.run.as_ref().map_or((false, None), |r| {
            (r.queued.0 == 0, if let RadioSeed::Track(t) = r.state.seed { Some(t) } else { None })
        });
        let mut explained = !first_batch;
        let lately = self.played_lately();
        let mut ids = Vec::with_capacity(picks.len());
        let mut whys: Vec<Option<String>> = Vec::with_capacity(picks.len());
        for p in &picks {
            match &p.item {
                Item::Track(t) => {
                    if radio_allows(*t, false, wants_new, &prefs, snap.as_deref(), kind) {
                        let why = if explained || Some(*t) == seed_track {
                            None
                        } else {
                            explained = true;
                            let before = ids.last().copied().or(seed_track);
                            let lib = &self.lib;
                            let title = |t: TrackId| lib.get_track(t).ok().flatten().map(|t| t.title);
                            snap.as_ref().and_then(|s| s.recs().why_pick(p, before, &title))
                        };
                        ids.push(*t);
                        whys.push(why);
                    }
                }
                // New music (never under «Миксы только из скачанного»).
                Item::Stream(_) if !streams_allowed(wants_new, &prefs, kind) => {}
                Item::Stream(r) => {
                    let pooled = self.recs.radio.run.as_ref().and_then(|run| run.pool.get(r).cloned());
                    let Some(t) = pooled.or_else(|| self.stream_track(r)) else { continue };
                    self.streaming.learn(std::slice::from_ref(&t));
                    if let Some(tid) = self.upsert_streams(std::slice::from_ref(&t)).first().copied() {
                        // New music that is a track played lately or queued.
                        if lately.contains(&tid) || ids.contains(&tid) {
                            continue;
                        }
                        explained |= first_batch;
                        let found = self.recs.radio.run.as_ref().and_then(|run| run.via.get(r).cloned());
                        ids.push(tid);
                        whys.push(found.or_else(|| self.discovery_why(&t)));
                        if let Some(run) = self.recs.radio.run.as_mut() {
                            run.streams.insert(tid, r.clone());
                        }
                    }
                }
            }
        }
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let new_now = ids.iter().filter(|t| run.streams.contains_key(t)).count();
        run.queued.0 += ids.len();
        run.queued.1 += new_now;
        log::info!(
            "radio «{}»: {} more, {new_now} new; {} of {} new so far ({:.0} %), {} new left",
            run.title,
            ids.len(),
            run.queued.1,
            run.queued.0,
            100.0 * run.queued.1 as f32 / run.queued.0.max(1) as f32,
            run.state.new_left()
        );
        if run.autoplay && ids.is_empty() {
            // Nothing fits (a small library under «только скачанное»): the
            // queue simply ends, and autoplay waits for another one.
            log::info!("autoplay: nothing to go on with");
            self.recs.radio.autoplay_dry = run.queue_gen;
            self.recs.radio.run = None;
            self.publish_player();
            return;
        }
        match run.queue_gen {
            None if ids.is_empty() => {
                self.recs.radio.run = None;
                self.next_origin = None;
                self.toast("Нечего играть", None);
                self.publish_player();
            }
            None => {
                let (context, ctx) = (context_of(&run.title), run.ctx);
                // A radio's picks are a path: they keep their order (or play
                // shuffled under shuffle with «Перемешивать радио и
                // автовоспроизведение»); shuffle stays or goes off as the
                // setting says.
                self.new_list();
                let shuffled = self.shuffle_radio();
                self.queue.set_path(&ids, Some(context), shuffled);
                self.play_context = ctx;
                self.play_context_id = None;
                let generation = self.queue.generation();
                // In the picks' own order, as `whys` are.
                let entries: Vec<u64> = self.queue.context_order().iter().map(|e| e.id).collect();
                self.recs.why.clear();
                self.explain_entries(&entries, whys);
                if let Some(run) = self.recs.radio.run.as_mut() {
                    run.queue_gen = Some(generation);
                    run.added.extend(entries);
                }
                if let Some(e) = self.queue.current() {
                    self.start(e);
                }
            }
            Some(_) => {
                let resume = std::mem::take(&mut run.resume);
                let shuffled = self.shuffle_radio();
                let entries: Vec<u64> = self.queue.enqueue_path(&ids, shuffled).iter().map(|e| e.id).collect();
                self.explain_entries(&entries, whys);
                if let Some(run) = self.recs.radio.run.as_mut() {
                    run.added.extend(entries);
                }
                if resume && self.playing.is_none() && self.status == v::Status::Stopped {
                    if let Some(e) = self.queue.current() {
                        self.start(e);
                    }
                } else {
                    self.preload_next();
                    self.publish_player();
                }
            }
        }
        self.radio_splice_new();
        self.radio_refill();
    }

    /// New music the radio's queue lacks (it came after the queue was
    /// picked): its share of what is queued joins between the queued
    /// tracks at once, evenly.
    fn radio_splice_new(&mut self) {
        if !self.radio_alive() {
            return;
        }
        let prefs = self.state.read().prefs.clone();
        let Some(snap) = self.snapshot() else { return };
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let share = super::new_share(&prefs, run.kind());
        if run.extending || run.queue_gen.is_none() || !streams_allowed(new_seed(&run.state.seed), &prefs, run.kind()) || share <= 0.0 {
            return;
        }
        let up: Vec<u64> = self.queue.upcoming().iter().filter(|e| run.added.contains(&e.id)).map(|e| e.id).collect();
        let have = self.queue.upcoming().iter().filter(|e| run.added.contains(&e.id) && run.streams.contains_key(&e.track)).count();
        let want = ((up.len() as f32) * share).round() as usize;
        if up.is_empty() || have >= want {
            return;
        }
        run.state.set_explore(Some(prefs.radio_explore.fraction()));
        let picks = run.state.take_new(&snap.recs().local_only(prefs.mixes_downloaded_only), want - have, up.len(), have);
        let lately = self.played_lately();
        let mut ids: Vec<(TrackId, RemoteId)> = Vec::new();
        for p in picks {
            let Item::Stream(r) = p.item else { continue };
            let pooled = self.recs.radio.run.as_ref().and_then(|run| run.pool.get(&r).cloned());
            let Some(t) = pooled else { continue };
            self.streaming.learn(std::slice::from_ref(&t));
            if let Some(tid) = self.upsert_streams(std::slice::from_ref(&t)).first().copied()
                && !lately.contains(&tid)
                && !ids.iter().any(|(x, _)| *x == tid)
            {
                ids.push((tid, r));
            }
        }
        if ids.is_empty() {
            return;
        }
        let every = (up.len() / (ids.len() + 1)).max(1);
        for (k, (tid, r)) in ids.iter().enumerate() {
            self.queue.enqueue(&[*tid]);
            let Some(entry) = self.queue.upcoming().last().map(|e| e.id) else { continue };
            if let Some(&anchor) = up.get(((k + 1) * every).min(up.len()) - 1) {
                self.queue.move_next_to(entry, anchor, true);
            }
            if let Some(run) = self.recs.radio.run.as_mut() {
                run.added.insert(entry);
                run.streams.insert(*tid, r.clone());
                run.queued.1 += 1;
                run.queued.0 += 1;
                if let Some(why) = run.via.get(r) {
                    self.recs.why.insert(entry, why.clone());
                }
            }
        }
        if let Some(run) = self.recs.radio.run.as_ref() {
            log::info!("radio «{}»: {} new music joined the queue ({} of {} new so far)", run.title, ids.len(), run.queued.1, run.queued.0);
        }
        self.preload_next();
        self.publish_player();
    }

    /// The new-music pool runs low: more is asked around the last tracks
    /// the radio played to the end (their service's radio or related list).
    fn radio_refill(&mut self) {
        let prefs = self.state.read().prefs.clone();
        let Some(run) = self.recs.radio.run.as_ref() else { return };
        if super::new_share(&prefs, run.kind()) <= 0.0 || prefs.mixes_downloaded_only {
            return;
        }
        if run.state.new_left() >= REFILL_AT {
            return;
        }
        let mut calls: Vec<(Call, String)> = Vec::new();
        let mut asked: Vec<String> = Vec::new();
        for t in run.heard.iter().rev() {
            if calls.len() == REFILL_FROM {
                break;
            }
            let call = match run.streams.get(t) {
                Some(r) if r.service == Service::Youtube => Some(Call::Radio(r.id.clone())),
                Some(r) if r.service == Service::Deezer => Some(Call::Related(Service::Deezer, r.id.clone())),
                Some(_) => None,
                None => self
                    .lib
                    .stream_refs(*t)
                    .unwrap_or_default()
                    .into_iter()
                    .find(|r| r.id.service == Service::Youtube)
                    .map(|r| Call::Radio(r.id.id)),
            };
            let Some(call) = call else { continue };
            let key = match &call {
                Call::Radio(v) | Call::Related(_, v) => v.clone(),
                Call::ArtistRadio { name, .. } => name.clone(),
            };
            if run.asked.contains(&key) {
                continue;
            }
            asked.push(key);
            let title = self.lib.get_track(*t).ok().flatten().map(|t| t.title).unwrap_or_default();
            calls.push((call, title));
        }
        if calls.is_empty() {
            return;
        }
        let id = run.id;
        log::info!("radio «{}»: {} new left, asking around {} of its tracks", run.title, run.state.new_left(), calls.len());
        if let Some(run) = self.recs.radio.run.as_mut() {
            run.asked.extend(asked);
        }
        self.radio_ask(id, calls);
    }

    /// The queue ran out: a radio picking more plays on when they come.
    pub(crate) fn radio_ran_out(&mut self) {
        if self.radio_alive()
            && let Some(run) = self.recs.radio.run.as_mut()
        {
            run.resume = true;
            if !run.extending {
                self.radio_extend();
            }
        }
    }

    /// How a track of the radio ended (see [`Engine::feedback`]).
    pub(crate) fn radio_feedback(&mut self, track: TrackId, complete: bool, position_ms: u32) {
        if !self.radio_alive() {
            return;
        }
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if run.queue_gen.is_none() {
            return;
        }
        let item = run.streams.get(&track).map_or(Item::Track(track), |r| Item::Stream(r.clone()));
        let outcome = if complete { Outcome::Complete } else { Outcome::Skip { position_ms } };
        if complete {
            run.heard.retain(|t| *t != track);
            run.heard.push(track);
        }
        run.state.feedback(item.clone(), outcome);
        if run.extending {
            run.pending.push((item, outcome));
        }
    }

    /// New-music candidates for the radio (filtered: not in the library,
    /// not disliked).
    fn radio_add_new(&mut self, tracks: Vec<StreamTrack>) {
        let snap = self.snapshot();
        let fresh: Vec<StreamTrack> = tracks
            .into_iter()
            .filter(|t| snap.as_ref().is_none_or(|s| s.is_new(t)))
            .collect();
        if fresh.is_empty() {
            return;
        }
        self.streaming.learn(&fresh);
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let new: Vec<NewTrack> = fresh
            .iter()
            .map(|t| snap.as_ref().map_or_else(|| NewTrack::from_stream(t, None), |s| s.new_track(t)))
            .collect();
        for t in fresh {
            run.pool.insert(t.id.clone(), t);
        }
        if run.extending {
            run.pending_new.extend(new.iter().cloned());
        }
        run.state.add_new(new);
    }

    /// The mixes (and «Новое для тебя») take the discoveries as new music.
    fn radio_add_discoveries(&mut self) {
        let Some(run) = &self.recs.radio.run else { return };
        if !matches!(
            run.state.seed,
            RadioSeed::Mood(_) | RadioSeed::Style { .. } | RadioSeed::Discover | RadioSeed::Daylist { .. }
        ) {
            return;
        }
        let found = self.discoveries();
        self.radio_add_new(found);
    }

    /// Asks the services for new music around the radio's seed (see the
    /// module docs); what the cache has is used at once.
    fn radio_fetch_new(&mut self) {
        let Some(run) = &self.recs.radio.run else { return };
        if super::new_share(&self.state.read().prefs, run.kind()) <= 0.0 {
            return;
        }
        let id = run.id;
        let seed_name = run.title.strip_prefix("Радио · ").unwrap_or(&run.title).to_owned();
        let mut calls: Vec<(Call, String)> = Vec::new();
        let artist_call = |w: &Worker, a: ArtistId, prefer: &[Service]| -> Option<(Call, String)> {
            let name = w.lib.get_artist(a).ok().flatten()?.name;
            let known = w.lib.external_ids(EntityKind::Artist, *a.as_uuid()).unwrap_or_default();
            prefer.iter().find(|s| w.service(**s).is_some()).map(|&service| {
                let call = Call::ArtistRadio {
                    service,
                    artist: a,
                    id: known.iter().find(|r| r.service == service).map(|r| r.id.clone()),
                    name: name.clone(),
                };
                (call, name.clone())
            })
        };
        match run.state.seed.clone() {
            RadioSeed::Track(t) => {
                let refs = self.lib.stream_refs(t).unwrap_or_default();
                if let Some(yt) = refs.iter().find(|r| r.id.service == Service::Youtube) {
                    let title = self.lib.get_track(t).ok().flatten().map(|t| t.title).unwrap_or_default();
                    calls.push((Call::Radio(yt.id.id.clone()), title));
                }
                let main = self
                    .lib
                    .get_track(t)
                    .ok()
                    .flatten()
                    .and_then(|t| t.artist_credit.artist_ids().next());
                if let Some(a) = main {
                    calls.extend(artist_call(self, a, &[Service::Deezer]));
                }
            }
            RadioSeed::Stream { id: r, .. } => match r.service {
                Service::Youtube => calls.push((Call::Radio(r.id), seed_name)),
                Service::Deezer => calls.push((Call::Related(Service::Deezer, r.id), seed_name)),
                Service::Soundcloud => calls.push((Call::Related(Service::Soundcloud, r.id), seed_name)),
                Service::Spotify => {}
            },
            RadioSeed::Artist(a) => calls.extend(artist_call(self, a, &[Service::Deezer, Service::Youtube])),
            RadioSeed::Mood(_) | RadioSeed::Style { .. } | RadioSeed::Discover | RadioSeed::Daylist { .. } => {}
        }
        for (c, _) in &calls {
            if let Call::Radio(v) | Call::Related(_, v) = c
                && let Some(run) = self.recs.radio.run.as_mut()
            {
                run.asked.insert(v.clone());
            }
        }
        self.radio_ask(id, calls);
    }

    /// Asks `calls` for new music for radio `id`: the cache first, the rest
    /// from services that can be asked now, in the background.
    /// Each call goes with the name of what it asks around (why its finds
    /// are offered).
    fn radio_ask(&mut self, id: u64, calls: Vec<(Call, String)>) {
        // The cache first; the rest from services that can be asked now.
        let now = nikgob_library::now_ms();
        let mut cached = Vec::new();
        let mut via: Vec<(RemoteId, String)> = Vec::new();
        let mut asks: Vec<(Arc<dyn StreamService>, Call, String, String)> = Vec::new();
        for (call, seed) in calls {
            let (service, key) = match &call {
                Call::Radio(v) => (Service::Youtube, cache_key(Service::Youtube, "radio", v)),
                Call::Related(s, t) => (*s, cache_key(*s, "related", t)),
                Call::ArtistRadio { service, artist, .. } => (*service, cache_key(*service, "artist", &artist.to_string())),
            };
            let hit: Option<Vec<StreamTrack>> = self
                .lib
                .cache_get(&key)
                .ok()
                .flatten()
                .filter(|(at, _)| now - at < NEW_TTL_MS)
                .and_then(|(_, json)| serde_json::from_str(&json).ok());
            let why = nikgob_recs::explain::new_found(&seed, service.display_name());
            if let Some(tracks) = hit {
                via.extend(tracks.iter().map(|t| (t.id.clone(), why.clone())));
                cached.extend(tracks);
                continue;
            }
            if self.pause_left(service).is_some() {
                continue;
            }
            if let Some(svc) = self.service(service) {
                asks.push((svc, call, key, why));
            }
        }
        if !cached.is_empty() {
            if let Some(run) = self.recs.radio.run.as_mut().filter(|r| r.id == id) {
                run.via.extend(via);
            }
            self.radio_add_new(cached);
            self.radio_splice_new();
        }
        if asks.is_empty() {
            return;
        }
        self.streaming.spawn_internal(async move {
            let mut tracks = Vec::new();
            let mut artists = Vec::new();
            let mut answers = Vec::new();
            let mut via = Vec::new();
            for (svc, call, key, why) in asks {
                let (result, matched) = fetch(svc, &call).await;
                artists.extend(matched);
                match result {
                    Ok(found) => {
                        answers.push((key, found.clone()));
                        via.extend(found.iter().map(|t| (t.id.clone(), why.clone())));
                        tracks.extend(found);
                    }
                    Err(e) => log::info!("radio: new music from {key}: {e}"),
                }
            }
            Internal::Recs(Msg::Radio(RadioMsg::NewFound {
                id,
                tracks,
                artists,
                answers,
                via,
            }))
        });
    }

    /// The radio as `session.json` keeps it (while it plays its queue).
    pub(crate) fn radio_save(&mut self) -> Option<RadioSave> {
        if !self.radio_alive() {
            return None;
        }
        let run = self.recs.radio.run.as_ref()?;
        run.queue_gen?;
        Some(RadioSave {
            state: run.state.clone(),
            title: run.title.clone(),
            context: run.ctx,
            seed: run.seed,
            streams: run.streams.iter().map(|(t, r)| (*t, r.clone())).collect(),
            pool: run.pool.values().cloned().collect(),
            autoplay: run.autoplay,
        })
    }

    /// The radio of the restored queue goes on.
    pub(crate) fn radio_restore(&mut self, save: RadioSave) {
        let r = &mut self.recs.radio;
        r.seq += 1;
        log::info!("radio «{}» goes on", save.title);
        self.streaming.learn(&save.pool);
        r.run = Some(RadioRun {
            id: r.seq,
            state: save.state,
            title: save.title,
            ctx: save.context,
            seed: save.seed,
            queue_gen: Some(self.queue.generation()),
            streams: save.streams.into_iter().collect(),
            pool: save.pool.into_iter().map(|t| (t.id.clone(), t)).collect(),
            extending: false,
            want: false,
            pending: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: save.autoplay,
            added: HashSet::new(),
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
        });
        self.play_context = self.recs.radio.run.as_ref().map_or(self.play_context, |r| r.ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contexts_read_as_phrases() {
        assert_eq!(context_of("Радио · Кино"), "радио «Кино»");
        assert_eq!(context_of("Спокойное"), "микса «Спокойное»");
    }
}
