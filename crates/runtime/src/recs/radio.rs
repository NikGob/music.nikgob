//! The radio queue: «Радио по треку», an artist's
//! radio and the endless mixes (moods, style mixes, «Новое для тебя», the
//! daylist) play as a queue that extends itself. The daylist starts with
//! the picks the home shows ([`Worker::start_radio_picked`]).
//!
//! * The queue holds the engine's [`Engine`] state and a short live buffer:
//!   when [`EXTEND_AT`] tracks or fewer are left, it is
//!   filled up to [`EXTEND_BY`] ahead, picked on the picks' thread from
//!   the current [`super::Snapshot`] (playback goes on meanwhile; two or
//!   three ahead are enough for the preload and a gapless step); what
//!   ended while they were picked is learnt after.
//! * How each track ended teaches the session: an approval (to the end or
//!   its outro, a like) pulls the next picks towards its sound, a rejection
//!   (a skip in its first half, a scrub-skip) pushes them away
//!   ([`Engine::feedback`]). The radio's tail follows at once: one
//!   rejection keeps the next track (it is preloaded) and picks the rest
//!   again with its push; two running («not now» of a loved track counts
//!   as two), «Не нравится» or a pick taken out of the queue by hand
//!   replace the radio's whole tail (playback waits for the new picks); a
//!   like of an unfamiliar track keeps the next and makes the best of its
//!   neighbours the one after, and asks the services around it. Only the
//!   radio's own entries ever go: what the user queued stays where it is.
//!   A replacement asked while picks are on their way waits for them, and
//!   they are forgotten unplayed ([`Engine::forget`]).
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
//! * «Моя волна» ([`RadioSeed::Wave`], [`super::wave`]) is a radio too:
//!   its picks come from the wave's bandit over the open pool, its novelty
//!   is its own (no splice of new music, no services asked around it), and
//!   a chip of its header or «↻ Другой вайб» replaces its whole tail at
//!   once while the track playing goes on, counted as no skip.
//! * Where its tracks come from is a hard rule: under «Миксы только из
//!   скачанного» nothing without a file is queued, without new music no
//!   stream pick is; switching either drops the radio's upcoming tracks
//!   that no longer fit and picks again.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::model::{ArtistId, EventContext, EventOrigin, TrackId};
use nikgob_core::streaming::{EntityKind, RemoteId, SearchKind, Service, StreamError, StreamService, StreamTrack};
use nikgob_recs::{Familiarity, Item, NewTrack, Outcome, Pick, RadioParams, RadioSeed, RadioState as Engine, Session, TrackLookup, artist_key};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::Msg;
use crate::Worker;
use crate::queue::Entry;
use crate::import::Internal;

/// The queue extends itself when this many tracks or fewer are left…
const EXTEND_AT: usize = 2;
/// …up to this many ahead (all of them for a radio's first picks).
const EXTEND_BY: usize = 3;
/// New-music answers of the services kept this long.
const NEW_TTL_MS: i64 = 24 * 3600 * 1000;
/// With fewer new candidates left than this the pool is refilled around
/// what the radio played to the end…
const REFILL_AT: usize = 6;
/// …from this many of its last tracks at a time.
const REFILL_FROM: usize = 2;
/// The radio's tracks heard out remembered (new music is asked around the
/// last few)…
const HEARD_KEEP: usize = 20;
/// …and the songs asked around already.
const ASKED_KEEP: usize = 200;

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
    /// Likes and removals while an extension ran: applied to its result.
    marks: Vec<Mark>,
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
    /// Its upcoming entries past the first this many are to be picked
    /// again ([`Worker::radio_redo`])…
    redo: Option<usize>,
    /// …and the extension on its way was asked before that: its picks go
    /// unplayed…
    stale: bool,
    /// …and its state forgets what left the queue meanwhile.
    forgotten: Vec<Item>,
    /// A wave whose filter has nothing left (not even to play again) in
    /// this snapshot: it is not asked again until the filter or the
    /// snapshot changes.
    wave_dry: Option<std::sync::Weak<super::Snapshot>>,
    /// A wave whose first picks found nothing waits for the top-ups of its
    /// filters instead of ending ([`Refill`]).
    refill: Option<Refill>,
}

/// A wave waiting for the top-ups of its filters (each pick of a wave of
/// every scene asks for the cluster it is in).
#[derive(Debug, Default)]
struct Refill {
    /// A top-up ended with songs: the next empty picks end the wave.
    done: bool,
    /// The filters whose top-ups ended with nothing: not asked again while
    /// it waits.
    empty: Vec<String>,
}

/// What the user did to a track of the radio besides playing it.
enum Mark {
    /// Liked while it played (its raw sound, when analysed).
    Liked(Item, Option<nikgob_recs::Vector>),
    /// Taken out of the queue, or «Не нравится».
    Removed(Item),
    /// The wave's filter changed.
    Wave(nikgob_recs::wave::WaveSeed),
    /// «↻ Другой вайб» while the item played.
    Vibe(Option<Item>),
}

impl Mark {
    fn apply(self, state: &mut Engine) {
        match self {
            Self::Liked(item, vector) => state.liked(item, vector),
            Self::Removed(item) => state.removed(item),
            Self::Wave(seed) => state.set_wave(seed),
            Self::Vibe(playing) => {
                state.other_vibe(playing.as_ref());
            }
        }
    }
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
    /// The queue places (play order) of the entries it added, and their
    /// tracks: entry ids are a run's own, places survive a restart.
    #[serde(default)]
    added_at: Vec<(usize, TrackId)>,
    /// The card of «Миксы для тебя» a wave was started from (none: the
    /// plain «Моя волна»).
    #[serde(default)]
    wave_preset: Option<v::MixKey>,
}

/// Every entry of the queue in play order (played, current, upcoming).
fn queue_order(q: &crate::queue::Queue) -> Vec<Entry> {
    let mut out = q.history().to_vec();
    out.extend(q.current());
    out.extend_from_slice(q.upcoming());
    out
}

/// The places in `order` of the entries `of` and their tracks, as
/// `session.json` keeps them (entry ids are a run's own).
pub(crate) fn places_of(order: &[Entry], of: &HashSet<u64>) -> Vec<(usize, TrackId)> {
    order.iter().enumerate().filter(|(_, e)| of.contains(&e.id)).map(|(k, e)| (k, e.track)).collect()
}

/// The entries of `order` at `places` that still hold their tracks (a
/// track that moved is the user's now).
pub(crate) fn entries_at(order: &[Entry], places: &[(usize, TrackId)]) -> HashSet<u64> {
    places.iter().filter_map(|(k, t)| order.get(*k).filter(|e| e.track == *t).map(|e| e.id)).collect()
}

#[derive(Default)]
pub(crate) struct RadioState {
    run: Option<RadioRun>,
    seq: u64,
    /// The queue generation autoplay found nothing for: not tried again
    /// until the queue is replaced.
    autoplay_dry: Option<u64>,
    /// The card «Моя волна» was started from, by the run it started.
    pub(crate) wave_preset: Option<(u64, Option<v::MixKey>)>,
    /// The user's scenes for the wave's header, of a snapshot.
    pub(crate) scenes: Option<(std::sync::Weak<super::Snapshot>, Vec<v::WaveSceneCard>)>,
    /// The wave's open pool, of a snapshot.
    pub(crate) wave_pool: Option<super::wave::WavePool>,
    /// The run whose extension on its way keeps to what plays while
    /// YouTube Music asks to wait.
    paused_pick: Option<u64>,
    /// Such an extension found nothing: no top-up before this.
    paused_dry_until: Option<std::time::Instant>,
}

impl RadioState {
    /// The number of the radio playing.
    pub(crate) fn run_id(&self) -> Option<u64> {
        self.run.as_ref().map(|r| r.id)
    }

    /// The engine of the radio playing.
    pub(crate) fn engine(&self) -> Option<&Engine> {
        self.run.as_ref().map(|r| &r.state)
    }
}

pub(crate) enum RadioMsg {
    Extended {
        id: u64,
        state: Box<Engine>,
        picks: Vec<Pick>,
        /// A wave's open pool, built or kept for its snapshot.
        wave_pool: Option<super::wave::WavePool>,
        /// The user's scenes for the wave's header, found for a snapshot
        /// on the way.
        scenes: Option<(std::sync::Weak<super::Snapshot>, Vec<v::WaveSceneCard>)>,
        /// What a wave's picks asked, ready for the worker.
        wave_asked: Option<super::wave::WaveAsked>,
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
        RadioSeed::Track(_) | RadioSeed::Stream { .. } | RadioSeed::Tracks(_) => v::RadioKind::Track,
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
    matches!(seed, RadioSeed::Discover | RadioSeed::Stream { .. } | RadioSeed::Wave(_))
}

/// A wave: its new music is its own.
fn is_wave(seed: &RadioSeed) -> bool {
    matches!(seed, RadioSeed::Wave(_))
}

/// What a radio of `seed` plays, as the history's `context_id`.
fn seed_context_id(seed: &RadioSeed, autoplay: bool) -> String {
    let what = match seed {
        RadioSeed::Track(t) => format!("track:{t}"),
        RadioSeed::Stream { id, .. } => format!("stream:{id}"),
        RadioSeed::Artist(a) => format!("artist:{a}"),
        RadioSeed::Tracks(tracks) => format!("list:{}", tracks.first().map(ToString::to_string).unwrap_or_default()),
        RadioSeed::Mood(m) => format!("mood:{m:?}"),
        RadioSeed::Style { index, .. } => format!("style:{index}"),
        RadioSeed::Discover => "discover".to_owned(),
        RadioSeed::Daylist { part, .. } => format!("daylist:{part:?}"),
        RadioSeed::Wave(_) => "wave".to_owned(),
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
/// «… радио по альбому «nyan.pop»» of a list's radio ([`list_title`]).
pub(super) fn context_of(title: &str) -> String {
    match (title.strip_prefix("Радио · "), title.strip_prefix("Радио по ")) {
        (Some(rest), _) => format!("радио «{rest}»"),
        (None, Some(rest)) => format!("радио по {rest}"),
        _ => format!("микса «{title}»"),
    }
}

/// The title of a list's radio: «Радио по альбому «nyan.pop»» (`of`
/// «альбому» or «плейлисту»).
fn list_title(of: &str, name: &str) -> String {
    format!("Радио по {of} «{name}»")
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

    /// «Автовоспроизведение» added the queue entry `entry` to the queue
    /// playing: the next albums of the world it continues, or its radio's
    /// picks.
    pub(crate) fn autoplay_owns(&self, entry: u64) -> bool {
        let generation = self.queue.generation();
        (self.autoplay_gen == Some(generation) && self.autoplay_entries.contains(&entry))
            || self.recs.radio.run.as_ref().is_some_and(|r| r.autoplay && r.queue_gen == Some(generation) && r.added.contains(&entry))
    }

    /// The radio of the queue playing is picking (or about to pick) more.
    pub(crate) fn radio_picking(&self) -> bool {
        let generation = self.queue.generation();
        self.recs
            .radio
            .run
            .as_ref()
            .is_some_and(|r| r.queue_gen.is_none_or(|g| g == generation) && (r.extending || r.want || r.redo.is_some()))
    }

    /// The world's next album that autoplay added, as `session.json` keeps it.
    pub(crate) fn autoplay_save(&self) -> Vec<(usize, TrackId)> {
        if self.autoplay_gen != Some(self.queue.generation()) {
            return Vec::new();
        }
        places_of(&queue_order(&self.queue), &self.autoplay_entries)
    }

    /// The world's next album that autoplay added, of the restored queue.
    pub(crate) fn autoplay_restore(&mut self, places: &[(usize, TrackId)]) {
        self.autoplay_entries = entries_at(&queue_order(&self.queue), places);
        self.autoplay_gen = Some(self.queue.generation());
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
                let seed = if in_catalog { RadioSeed::Track(id) } else { self.stream_seed(id, &t) };
                self.start_radio(seed, title, EventContext::Radio, Some(id));
            }
            v::RadioSeed::Album(r) => {
                let title = self.lib.get_release(r).ok().flatten().map_or_else(String::new, |r| r.title);
                let (ids, _) = self.release_tracks_for(r, None, None);
                self.start_list_radio(&ids, &[], "альбому", &title);
            }
            v::RadioSeed::Playlist(p) => {
                let (ids, name) = self.playlist_tracks(p, None);
                self.start_list_radio(&ids, &[], "плейлисту", &name);
            }
            v::RadioSeed::StreamAlbum(r) => self.stream_list_radio(r, true),
            v::RadioSeed::StreamPlaylist(r) => self.stream_list_radio(r, false),
        }
    }

    /// «Радио по альбому», «Радио по плейлисту»: a radio grown from the
    /// list `ids` ([`list_seed`]; `streams` are a service list's songs).
    /// The list is only the seed: the radio's picks, filtered as any
    /// radio's, play first.
    pub(crate) fn start_list_radio(&mut self, ids: &[TrackId], streams: &[StreamTrack], of: &str, name: &str) {
        let title = list_title(of, name);
        let snap = self.snapshot();
        let seed = {
            let index = self.analysis.index().read();
            list_seed(
                ids,
                streams,
                |t| snap.as_ref().is_none_or(|s| s.catalog.get(t).is_some()),
                |t| index.get(t).copied(),
                |name| snap.as_ref()?.artist_keys.get(&artist_key(name)).copied(),
            )
        };
        let Some(seed) = seed else {
            self.toast("Нечего играть", None);
            return;
        };
        self.start_radio(seed, title, EventContext::Radio, None);
    }

    /// The seed of a service's song `t` (the library's `id`) the catalog
    /// lacks: its sound when analysed, and the library artists it matches.
    fn stream_seed(&self, id: TrackId, t: &StreamTrack) -> RadioSeed {
        let snap = self.snapshot();
        let artists = t.artists.iter().filter_map(|a| snap.as_ref()?.artist_keys.get(&artist_key(&a.name)).copied()).collect();
        let vector = self.analysis.index().read().get(id).copied();
        RadioSeed::Stream { id: t.id.clone(), vector, artists }
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
            marks: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: false,
            added: HashSet::new(),
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
            redo: None,
            stale: false,
            forgotten: Vec::new(),
            wave_dry: None,
            refill: None,
        };
        log::info!("radio «{}» starts ({:?})", run.title, run.state.seed);
        let context = context_of(&run.title);
        r.run = Some(run);
        self.radio_add_discoveries();
        self.radio_fetch_new();
        match first {
            Some(t) => {
                // A radio keeps the shuffle button as it is.
                self.new_list(None);
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
            marks: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: false,
            added: HashSet::new(),
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
            redo: None,
            stale: false,
            forgotten: Vec::new(),
            wave_dry: None,
            refill: None,
        });
        log::info!("radio «{}» starts from its picks ({:?})", self.recs.radio.run.as_ref().map_or("", |r| &r.title), state.seed);
        self.radio_add_discoveries();
        self.radio_fetch_new();
        self.radio_extended(id, state, picks, None);
    }

    /// The radio still plays its queue (any other playback ended it).
    pub(super) fn radio_alive(&mut self) -> bool {
        let generation = self.queue.generation();
        let r = &mut self.recs.radio;
        match &r.run {
            Some(run) if run.queue_gen.is_none_or(|g| g == generation) => true,
            Some(run) => {
                log::info!("radio «{}» ends: the queue was replaced", run.title);
                r.run = None;
                // A wave's pool goes with it.
                r.wave_pool = None;
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
        // The short buffer is topped up after every track: only picks the
        // queue waits for (nothing ahead) show.
        let nothing_ahead = self.queue.upcoming().is_empty();
        let wave = self.wave_view();
        self.recs.radio.run.as_ref().map(|r| v::RadioView {
            title: r.title.clone(),
            extending: (r.extending || r.want) && nothing_ahead,
            autoplay: r.autoplay,
            kind: r.kind(),
            wave,
        })
    }

    /// Three tracks or fewer left: the next ones are picked.
    pub(crate) fn radio_top_up(&mut self) {
        if !self.radio_alive() {
            self.autoplay_start();
            return;
        }
        let left = self.queue.upcoming().len();
        let snap = self.snapshot();
        let Some(run) = &self.recs.radio.run else { return };
        if run.extending || (run.queue_gen.is_some() && left > EXTEND_AT) {
            return;
        }
        if self.recs.radio.paused_dry_until.is_some_and(|t| std::time::Instant::now() < t) {
            return;
        }
        if let (Some(dry), Some(snap)) = (&run.wave_dry, &snap)
            && std::sync::Weak::ptr_eq(dry, &Arc::downgrade(snap))
        {
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
                if self.autoplay_gen != Some(generation) {
                    self.autoplay_entries.clear();
                    self.autoplay_gen = Some(generation);
                }
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
            marks: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: true,
            added: HashSet::new(),
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
            redo: None,
            stale: false,
            forgotten: Vec::new(),
            wave_dry: None,
            refill: None,
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
        self.sleep_replacing(&drop);
        for id in drop {
            self.queue.remove(id);
        }
        self.radio_fetch_new();
        self.preload_next();
        self.publish_player();
    }

    /// Fills the queue up to [`EXTEND_BY`] ahead on the picks' thread
    /// ([`super::picker`]).
    fn radio_extend(&mut self) {
        let ahead = self.queue.upcoming().len();
        let prefs = self.state.read().prefs.clone();
        let Some(kind) = self.recs.radio.run.as_ref().map(RadioRun::kind) else { return };
        let new_share = super::new_share(&prefs, kind);
        // While YouTube Music asks to wait, a top-up of a playing radio
        // queues what plays now (its files); after the pause it picks as
        // before. (A radio just started picks as ever: its first entry
        // waits for the pause or gives way to one that plays.)
        let queued = self.recs.radio.run.as_ref().is_some_and(|r| r.queue_gen.is_some());
        let paused = queued && !prefs.mixes_downloaded_only && self.pause_left(Service::Youtube).is_some();
        let local_only = prefs.mixes_downloaded_only || paused;
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
        self.recs.radio.paused_pick = paused.then_some(id);
        state.attune(self.recs.session.clone(), self.recs.weights);
        state.set_no_repeat_ms(i64::from(prefs.radio_no_repeat_hours) * 3_600_000);
        state.set_explore(Some(prefs.radio_explore.fraction()));
        state.set_unheard(prefs.radio_unheard.fraction());
        let params = RadioParams { new_share, seed: run.seed };
        let n = if run.queue_gen.is_none() { EXTEND_BY } else { EXTEND_BY.saturating_sub(ahead).max(1) };
        let tx = self.recs.tx.clone();
        if is_wave(&state.seed) {
            self.wave_extend(id, state, snap, local_only, n, params.seed);
            return;
        }
        self.recs.radio.wave_pool = None;
        let spawned = self.recs.picker.send(super::picker::Lane::Radio, move || {
            // The prefs as they are now, even before the snapshot follows.
            let picks = state.extend(&snap.recs().local_only(local_only), n, &params);
            let msg = super::radio::RadioMsg::Extended {
                id,
                state: Box::new(state),
                picks,
                wave_pool: None,
                scenes: None,
                wave_asked: None,
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

    /// A wave's extension (see the module docs): its open pool kept for the
    /// snapshot, or the inputs to build it on the picks' thread.
    fn wave_extend(&mut self, id: u64, mut state: Engine, snap: Arc<super::Snapshot>, local_only: bool, n: usize, seed: u64) {
        let kept = self.discoveries_kept();
        self.new_log_load();
        let bans = self.recs.newlog.bans;
        // The previews the picks may ask for (none when they cannot run
        // today: no row is written for them).
        let room = if local_only { 0 } else { self.preview_room() };
        let topped_gen = self.recs.niche.topped_gen;
        let pool = self.recs.radio.wave_pool.clone().filter(|p| p.holds(&snap, kept, bans, topped_gen));
        // What a new pool is built from (only when it is needed).
        let inputs = match (&pool, local_only) {
            (None, false) => {
                // The niches' top-ups are read from the cache on the picks'
                // thread: the home's niches and the wave's own filters.
                let niches: Vec<(v::MixKey, Vec<TrackId>)> = match self.recs.home.as_ref() {
                    Some(h) if !snap.local_only => h.lanes.iter().filter(|l| !l.niche.is_empty()).map(|l| (l.key, l.niche.clone())).collect(),
                    _ => Vec::new(),
                };
                let waves = if snap.local_only { Vec::new() } else { self.recs.niche.wave_kept.clone() };
                Some((self.discoveries_all(), self.discovery_sources(), niches, waves, self.recs.newlog.log.clone(), self.analysis.index().clone()))
            }
            _ => None,
        };
        let lib = self.lib.clone();
        // The header's scenes, when this snapshot has none yet (they scan
        // the library: not on the worker).
        let has_scenes = self.recs.radio.scenes.as_ref().is_some_and(|(w, _)| std::sync::Weak::ptr_eq(w, &Arc::downgrade(&snap)));
        let profiles = (!has_scenes).then(|| self.recs.home.as_ref().map(|h| h.blend_profiles.clone()).unwrap_or_default());
        let tx = self.recs.tx.clone();
        let spawned = self.recs.picker.send(super::picker::Lane::Radio, move || {
            let t0 = std::time::Instant::now();
            let pool = match (pool, inputs) {
                (Some(p), _) => Some(p),
                (None, Some((found, sources, niches, waves, log, index))) => {
                    let mut topped: Vec<super::niche::NicheFound> = niches.iter().filter_map(|(k, seeds)| super::niche::cached(&lib, *k, seeds)).collect();
                    topped.extend(super::niche::wave_topped(&lib, &waves));
                    let heard = |t: &StreamTrack| super::open::heard_in(&lib, &index, t);
                    let cands = super::open::open_pool(&snap, &found, &sources, &topped, &log, &heard);
                    Some(super::wave::WavePool::new(&snap, kept, bans, topped_gen, cands))
                }
                (None, None) => None,
            };
            let cands: &[nikgob_recs::open_world::OpenCandidate] = pool.as_ref().map_or(&[], |p| p.cands.as_slice());
            let picks = super::wave::extend(&mut state, &snap, cands, local_only, n, seed);
            log::info!(
                "wave: {} picked in {:.1} ms from a pool of {} (cluster {:?})",
                picks.len(),
                t0.elapsed().as_secs_f32() * 1e3,
                cands.len(),
                state.wave_bandit().and_then(|b| b.current_scene())
            );
            let scenes = profiles.map(|p| (Arc::downgrade(&snap), super::wave::scenes(&snap, &p)));
            // What the picks ask, its library work done here.
            let asks = state.take_wave_asks();
            let asked = (!local_only).then(|| super::wave::prepare_asks(&lib, cands, asks, room));
            let msg = RadioMsg::Extended { id, state: Box::new(state), picks, wave_pool: pool, scenes, wave_asked: asked };
            let _ = tx.send(Internal::Recs(Msg::Radio(msg)));
        });
        if let Err(e) = spawned {
            log::warn!("wave: cannot pick: {e}");
            if let Some(run) = self.recs.radio.run.as_mut() {
                run.extending = false;
            }
        }
    }

    /// The playing wave's filter is now `seed` (`None`: «↻ Другой вайб»,
    /// another cluster): its whole tail is picked again, the track playing
    /// goes on and counts as no skip.
    pub(crate) fn radio_wave_changed(&mut self, seed: Option<nikgob_recs::wave::WaveSeed>) {
        if !self.radio_alive() {
            return;
        }
        let playing = self.playing.map(|e| e.track);
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let Some(old) = run.state.wave_seed().cloned() else { return };
        // The song playing, as the radio served it: «Другой вайб» leaves
        // its cluster, not that of the picks ahead of it.
        let playing = playing.map(|t| run.streams.get(&t).map_or(Item::Track(t), |r| Item::Stream(r.clone())));
        run.wave_dry = None;
        // Picks on their way for a queue not started yet: they are of
        // what the user just changed; the newer asks again.
        if run.extending && run.queue_gen.is_none() {
            run.stale = true;
        }
        let seed = match seed {
            Some(s) => Some(s),
            // One cluster only: «Другой вайб» leaves the scene chosen for
            // every scene (another cluster then), else it starts afresh.
            None if !run.state.other_vibe(playing.as_ref()) && old.filter.scene != nikgob_recs::open_world::Scene::All => {
                let mut s = old.clone();
                s.filter.scene = nikgob_recs::open_world::Scene::All;
                s.only.clear();
                Some(s)
            }
            None => {
                if run.extending {
                    run.marks.push(Mark::Vibe(playing));
                }
                None
            }
        };
        if let Some(s) = seed {
            run.state.set_wave(s.clone());
            if run.extending {
                run.marks.push(Mark::Wave(s));
            }
        }
        self.radio_repick(0);
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
            RadioMsg::Extended { id, state, picks, wave_pool, scenes, wave_asked } => {
                if scenes.is_some() {
                    self.recs.radio.scenes = scenes;
                }
                if let Some(pool) = wave_pool {
                    // The wave's new picks play from the pool's tracks.
                    if let Some(run) = self.recs.radio.run.as_mut().filter(|r| r.id == id) {
                        for p in &picks {
                            if let Item::Stream(r) = &p.item
                                && let Some(c) = pool.cands.iter().find(|c| c.track.id == *r)
                            {
                                run.pool.insert(r.clone(), c.track.clone());
                            }
                        }
                    }
                    self.recs.radio.wave_pool = Some(pool);
                }
                let refill = wave_asked.and_then(|a| self.wave_asked(id, a));
                self.radio_extended(id, *state, picks, refill);
            }
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

    fn radio_extended(&mut self, id: u64, mut state: Engine, picks: Vec<Pick>, refill: Option<String>) {
        let alive = self.radio_alive();
        let paused_pick = self.recs.radio.paused_pick.take_if(|r| *r == id).is_some();
        let Some(run) = self.recs.radio.run.as_mut().filter(|r| r.id == id) else { return };
        run.extending = false;
        for (item, outcome) in run.pending.drain(..) {
            state.feedback(item, outcome);
        }
        for mark in run.marks.drain(..) {
            mark.apply(&mut state);
        }
        state.add_new(run.pending_new.drain(..));
        run.state = state;
        if !alive {
            return;
        }
        // Asked to pick again while these were picked: they go unplayed,
        // and the tail dropped meanwhile is forgotten by their state too.
        let forgotten = std::mem::take(&mut run.forgotten);
        run.state.forget(&forgotten);
        // (Before the queue started only a wave's change makes them stale:
        // the picks for it come instead.)
        if std::mem::take(&mut run.stale) {
            let items: Vec<Item> = picks.into_iter().map(|p| p.item).collect();
            run.state.forget(&items);
            self.radio_extend();
            self.radio_redo();
            self.publish_player();
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
        if paused_pick && ids.is_empty() {
            // Nothing that plays now: the queue waits for the pause (see
            // `streams::held`), and the radio picks again after it.
            let left = self.pause_left(Service::Youtube).unwrap_or(Duration::from_secs(30));
            log::info!("radio: nothing that plays while YouTube Music asks to wait; picks again in {} s", left.as_secs());
            self.recs.radio.paused_dry_until = Some(std::time::Instant::now() + left);
            self.publish_player();
            return;
        }
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let mut went_dry = false;
        if is_wave(&run.state.seed) && run.queue_gen.is_some() {
            let dry = ids.is_empty();
            went_dry = dry && run.wave_dry.is_none();
            run.wave_dry = dry.then(|| snap.as_ref().map(Arc::downgrade)).flatten();
        }
        if went_dry {
            log::info!("wave: its filter has nothing left");
            self.toast("Нечего играть", None);
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
            self.radio_stops_waiting();
            self.publish_player();
            return;
        }
        // A wave with nothing yet waits while its filter is topped up
        // (once; it ends when the top-up brings nothing, see
        // [`Worker::wave_refill_ended`]).
        if run.queue_gen.is_none() && ids.is_empty() && is_wave(&run.state.seed) {
            if run.refill.is_none()
                && let Some(token) = refill
            {
                log::info!("wave: nothing new of its filter yet, it waits for the top-up of {token}");
                run.refill = Some(Refill::default());
            }
            if run.refill.as_ref().is_some_and(|r| !r.done) {
                run.want = true;
                self.publish_player();
                return;
            }
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
                // автовоспроизведение»); the shuffle button stays as it is.
                self.new_list(None);
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
                let waiting = self.playing.is_none() && matches!(self.status, v::Status::Stopped | v::Status::Buffering);
                if resume && waiting {
                    match self.queue.current() {
                        Some(e) => self.start(e),
                        None => self.radio_stops_waiting(),
                    }
                } else {
                    self.preload_next();
                    self.publish_player();
                }
            }
        }
        self.radio_splice_new();
        self.radio_refill();
        self.radio_prune();
    }

    /// New music the radio's queue lacks (it came after the queue was
    /// picked, or the short extensions rounded it away): what its share of
    /// all the radio queued so far wants more joins between the queued
    /// tracks at once, evenly.
    fn radio_splice_new(&mut self) {
        if !self.radio_alive() {
            return;
        }
        let prefs = self.state.read().prefs.clone();
        let Some(snap) = self.snapshot() else { return };
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let share = super::new_share(&prefs, run.kind());
        // New music waits for YouTube Music's pause too.
        if self.pause_left(Service::Youtube).is_some() {
            return;
        }
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if is_wave(&run.state.seed) || run.extending || run.queue_gen.is_none() || !streams_allowed(new_seed(&run.state.seed), &prefs, run.kind()) || share <= 0.0 {
            return;
        }
        let up: Vec<u64> = self.queue.upcoming().iter().filter(|e| run.added.contains(&e.id)).map(|e| e.id).collect();
        let have = self.queue.upcoming().iter().filter(|e| run.added.contains(&e.id) && run.streams.contains_key(&e.track)).count();
        // Of all queued so far (the next pick counted), its share is new.
        let owed = (((run.queued.0 + 1) as f32) * share).floor() as usize;
        let more = owed.saturating_sub(run.queued.1).min(up.len().saturating_sub(have));
        if up.is_empty() || more == 0 {
            return;
        }
        run.state.set_explore(Some(prefs.radio_explore.fraction()));
        let picks = run.state.take_new(&snap.recs().local_only(prefs.mixes_downloaded_only), more, up.len(), have);
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
        // The buffer holds three: new music takes the place of the
        // radio's farthest picks of the library (never the next one, it
        // is preloaded, never the user's), else waits for a free place.
        let room = EXTEND_BY.saturating_sub(self.queue.upcoming().len());
        let mut gone: Vec<Entry> = Vec::new();
        if let Some(run) = self.recs.radio.run.as_ref() {
            let movable: Vec<Entry> =
                self.queue.upcoming().iter().skip(1).filter(|e| run.added.contains(&e.id) && !run.streams.contains_key(&e.track)).copied().collect();
            gone = movable.into_iter().rev().take(ids.len().saturating_sub(room)).collect();
        }
        ids.truncate(room + gone.len());
        if ids.is_empty() {
            return;
        }
        if let Some(run) = self.recs.radio.run.as_mut() {
            let items: Vec<Item> = gone.iter().map(|e| Item::Track(e.track)).collect();
            run.state.forget(&items);
            for e in &gone {
                run.added.remove(&e.id);
                run.queued.0 = run.queued.0.saturating_sub(1);
            }
        }
        self.sleep_replacing(&gone.iter().map(|e| e.id).collect::<Vec<_>>());
        for e in &gone {
            self.queue.remove(e.id);
            self.recs.why.remove(&e.id);
        }
        let up: Vec<u64> = self.queue.upcoming().iter().filter(|e| self.recs.radio.run.as_ref().is_some_and(|r| r.added.contains(&e.id))).map(|e| e.id).collect();
        if up.is_empty() {
            // Nothing of the radio's ahead to go between: after the queue.
            for (tid, r) in &ids {
                let added = self.queue.enqueue(&[*tid]);
                if let Some(run) = self.recs.radio.run.as_mut() {
                    run.added.extend(added.iter().map(|e| e.id));
                    run.streams.insert(*tid, r.clone());
                    run.queued.1 += 1;
                    run.queued.0 += 1;
                }
            }
            self.preload_next();
            self.publish_player();
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
        if is_wave(&run.state.seed) || super::new_share(&prefs, run.kind()) <= 0.0 || prefs.mixes_downloaded_only {
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

    /// How a track of the radio ended (see [`Engine::feedback`]), after
    /// the session learnt it: a rejection picks the tail again (see the
    /// module docs).
    pub(crate) fn radio_feedback(&mut self, track: TrackId, complete: bool, position_ms: u32, duration_ms: Option<u32>) {
        if !self.radio_alive() {
            return;
        }
        // What the session says of it, and of the skips running.
        let song = self.recs.snap.as_ref().and_then(|s| s.catalog.get(track)).map_or(track, |t| t.id);
        let snap = self.snapshot();
        let played = self.recs.session.last_of(song).cloned();
        let scrubbed = played.as_ref().is_some_and(|p| p.scrubbed);
        let (rejected, streak) = match (&snap, &played) {
            (Some(s), Some(p)) => (Session::verdict(p, &s.model).rejects(), self.recs.session.skip_streak(&s.model)),
            _ => {
                let o = if complete { Outcome::Complete } else { Outcome::Skip { position_ms, duration_ms, scrubbed } };
                (o.verdict().rejects(), u8::from(o.verdict().rejects()))
            }
        };
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if run.queue_gen.is_none() {
            return;
        }
        let item = run.streams.get(&track).map_or(Item::Track(track), |r| Item::Stream(r.clone()));
        let outcome = if complete { Outcome::Complete } else { Outcome::Skip { position_ms, duration_ms, scrubbed } };
        if complete {
            run.heard.retain(|t| *t != track);
            run.heard.push(track);
            let over = run.heard.len().saturating_sub(HEARD_KEEP);
            run.heard.drain(..over);
        }
        run.state.feedback(item.clone(), outcome);
        if run.extending {
            run.pending.push((item, outcome));
        }
        if rejected {
            // Two running: the whole tail; one: all but the next (it is
            // preloaded already).
            let keep = if streak >= nikgob_recs::radio::SKIP_SHIFT { 0 } else { 1 };
            log::info!("radio «{}»: a skip ({streak} running), the tail past {keep} is picked again", run.title);
            run.redo = Some(run.redo.map_or(keep, |k| k.min(keep)));
        }
        // What a dislike of it asked waits for its end too.
        self.radio_redo();
    }

    /// The upcoming entries of the radio past the first `keep` of the
    /// queue are to be picked again (the least `keep` asked wins), now or
    /// once the picks on their way come.
    fn radio_repick(&mut self, keep: usize) {
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        run.redo = Some(run.redo.map_or(keep, |k| k.min(keep)));
        self.radio_redo();
    }

    /// Picks the radio's tail again ([`RadioRun::redo`]): its entries
    /// past the first `keep` upcoming leave the queue unplayed and the
    /// radio picks on from what is left; nothing left, playback waits for
    /// the picks. The user's own entries stay.
    pub(crate) fn radio_redo(&mut self) {
        if !self.radio_alive() {
            return;
        }
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let Some(keep) = run.redo.take() else { return };
        if run.queue_gen.is_none() {
            return;
        }
        let gone: Vec<Entry> = self.queue.upcoming().iter().skip(keep).filter(|e| run.added.contains(&e.id)).copied().collect();
        let items: Vec<Item> = gone.iter().map(|e| run.streams.get(&e.track).map_or(Item::Track(e.track), |r| Item::Stream(r.clone()))).collect();
        run.state.forget(&items);
        // Picks on their way were picked before this: they go unplayed
        // when they come, and what left the queue now is forgotten by the
        // state they come with too.
        if run.extending {
            run.stale = true;
            run.forgotten.extend(items);
        }
        for e in &gone {
            run.added.remove(&e.id);
        }
        self.sleep_replacing(&gone.iter().map(|e| e.id).collect::<Vec<_>>());
        for e in &gone {
            self.queue.remove(e.id);
            self.recs.why.remove(&e.id);
        }
        if self.queue.upcoming().is_empty()
            && let Some(run) = self.recs.radio.run.as_mut()
        {
            run.resume = true;
        }
        self.radio_extend();
        self.preload_next();
        self.publish_player();
    }

    /// The user's «Дальше» found nothing queued: true when the radio is
    /// picking what plays next (playback waits for it, buffering).
    pub(crate) fn radio_holds(&mut self) -> bool {
        self.radio_alive() && self.recs.radio.run.as_ref().is_some_and(|r| r.resume && (r.extending || r.want))
    }

    /// The top-up of the wave's filter `token` ended with `found` songs
    /// (found now, or kept from a top-up of it lately): a wave waiting for
    /// its filters' top-ups (nothing to play yet; each pick of a wave of
    /// every scene asks for the cluster it is in) picks again from its pool
    /// with them (the pool is built again:
    /// [`super::niche::NicheState::topped_gen`] moved), or ends when there
    /// are none and none of its filters is still to be topped up.
    pub(crate) fn wave_refill_ended(&mut self, token: &str, found: usize) {
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        let Some(refill) = run.refill.as_mut().filter(|r| !r.done) else { return };
        if found > 0 {
            refill.done = true;
            log::info!("wave: the top-up of {token} brought {found} songs");
            if run.want {
                self.radio_extend();
            }
            return;
        }
        refill.empty.push(token.to_owned());
        if self.niche_wave_pending() {
            log::info!("wave: the top-up of {token} brought nothing, another is under way");
            return;
        }
        log::info!("wave: its top-ups brought nothing");
        self.recs.radio.run = None;
        self.recs.radio.wave_pool = None;
        self.next_origin = None;
        self.toast("Нечего играть", None);
        self.radio_stops_waiting();
        self.publish_player();
    }

    /// Whether the wave plays nothing yet and waits for the top-ups of its
    /// filters.
    pub(crate) fn wave_waits(&self) -> bool {
        self.recs.radio.run.as_ref().and_then(|r| r.refill.as_ref()).is_some_and(|r| !r.done)
    }

    /// Whether the waiting wave's filter `token` was topped up with nothing
    /// while it waits (it is not asked again).
    pub(crate) fn wave_refill_empty(&self, token: &str) -> bool {
        self.recs.radio.run.as_ref().and_then(|r| r.refill.as_ref()).is_some_and(|r| !r.done && r.empty.iter().any(|t| t == token))
    }

    /// The radio found nothing to play after all: the player waiting for
    /// it stops.
    pub(super) fn radio_stops_waiting(&mut self) {
        if self.playing.is_none() && self.status == v::Status::Buffering {
            self.status = v::Status::Stopped;
        }
    }

    /// The user liked `track`: when the radio played it and it is new to
    /// them, the best of its neighbours follows the next track, and the
    /// services are asked around it.
    pub(crate) fn radio_liked(&mut self, track: TrackId) {
        if !self.radio_alive() || self.playing.map(|e| e.track) != Some(track) {
            return;
        }
        let snap = self.snapshot();
        let unfamiliar = snap.as_ref().is_none_or(|s| s.model.familiarity(track) == Familiarity::Unfamiliar);
        let vector = self.analysis.index().read().get(track).copied();
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if run.queue_gen.is_none() {
            return;
        }
        let item = run.streams.get(&track).map_or(Item::Track(track), |r| Item::Stream(r.clone()));
        // A wave learns every like (its cluster's α); the neighbours follow
        // a track new to the user only.
        if !unfamiliar {
            if is_wave(&run.state.seed) {
                run.state.liked(item.clone(), vector);
                if run.extending {
                    run.marks.push(Mark::Liked(item, vector));
                }
            }
            return;
        }
        run.state.liked(item.clone(), vector);
        if run.extending {
            run.marks.push(Mark::Liked(item.clone(), vector));
        }
        log::info!("radio «{}»: an unfamiliar track liked, its neighbours come next", run.title);
        let call = match run.streams.get(&track) {
            Some(r) if r.service == Service::Youtube => Some(Call::Radio(r.id.clone())),
            Some(r) if matches!(r.service, Service::Deezer | Service::Soundcloud) => Some(Call::Related(r.service, r.id.clone())),
            Some(_) => None,
            None => self.lib.stream_refs(track).unwrap_or_default().into_iter().find(|r| r.id.service == Service::Youtube).map(|r| Call::Radio(r.id.id)),
        };
        let wants_new = self.recs.radio.run.as_ref().is_some_and(|r| !is_wave(&r.state.seed) && super::new_share(&self.state.read().prefs, r.kind()) > 0.0);
        if let Some(call) = call.filter(|_| wants_new && !self.state.read().prefs.mixes_downloaded_only) {
            let key = match &call {
                Call::Radio(v) | Call::Related(_, v) => v.clone(),
                Call::ArtistRadio { name, .. } => name.clone(),
            };
            let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
            if let Some(run) = self.recs.radio.run.as_mut()
                && run.asked.insert(key)
            {
                let id = run.id;
                self.radio_ask(id, vec![(call, title)]);
            }
        }
        self.radio_repick(1);
    }

    /// The user marked `track` «Не нравится»: the radio's whole tail is
    /// picked again, its sound pushing. Its skip, when it plays and is
    /// skipped for it, does so as it ends; else now.
    pub(crate) fn radio_disliked(&mut self, track: TrackId, skips: bool) {
        if !self.radio_alive() {
            return;
        }
        let playing = self.playing.map(|e| e.track) == Some(track);
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if run.queue_gen.is_none() {
            return;
        }
        let item = run.streams.get(&track).map_or(Item::Track(track), |r| Item::Stream(r.clone()));
        if run.extending {
            run.marks.push(Mark::Removed(item.clone()));
        }
        run.state.removed(item);
        log::info!("radio «{}»: a dislike, its tail is picked again", run.title);
        if playing && skips {
            run.redo = Some(0);
        } else {
            self.radio_repick(0);
        }
    }

    /// The user took the queue entry `entry` (of `track`) out: one the
    /// radio added replaces the radio's whole tail, its sound pushing.
    pub(crate) fn radio_removed(&mut self, entry: u64, track: TrackId) {
        if !self.radio_alive() {
            return;
        }
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        if run.queue_gen.is_none() || !run.added.remove(&entry) {
            return;
        }
        let item = run.streams.get(&track).map_or(Item::Track(track), |r| Item::Stream(r.clone()));
        if run.extending {
            run.marks.push(Mark::Removed(item.clone()));
        }
        run.state.removed(item);
        log::info!("radio «{}»: a pick taken out, its tail is picked again", run.title);
        self.radio_repick(0);
    }

    /// New-music candidates for the radio (filtered: not in the library,
    /// not disliked, never junk, the snapshot there or not).
    fn radio_add_new(&mut self, tracks: Vec<StreamTrack>) {
        let snap = self.snapshot();
        let fresh: Vec<StreamTrack> = tracks
            .into_iter()
            .filter(|t| match snap.as_ref() {
                Some(s) => s.is_new(t),
                None => t.playable && nikgob_recs::junk(t, None).is_none(),
            })
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
        self.radio_prune();
    }

    /// The radio's bookkeeping stays bounded however long it plays: the
    /// candidates as the engine keeps them (its pool is capped) and the
    /// stream picks the queue still holds, where each was found, the songs
    /// asked around lately.
    fn radio_prune(&mut self) {
        let in_queue: HashSet<TrackId> = queue_order(&self.queue).iter().map(|e| e.track).collect();
        let Some(run) = self.recs.radio.run.as_mut() else { return };
        // Picks on their way may hold stream candidates not queued yet.
        if run.extending {
            return;
        }
        let heard: HashSet<TrackId> = run.heard.iter().copied().collect();
        run.streams.retain(|t, _| in_queue.contains(t) || heard.contains(t));
        let playing: HashSet<RemoteId> = run.streams.values().cloned().collect();
        let state = &run.state;
        run.pool.retain(|id, _| playing.contains(id) || state.has_new(id));
        let pool = &run.pool;
        run.via.retain(|id, _| pool.contains_key(id));
        if run.asked.len() > ASKED_KEEP {
            run.asked.clear();
        }
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
        if is_wave(&run.state.seed) || super::new_share(&self.state.read().prefs, run.kind()) <= 0.0 {
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
            // A list: the YouTube radio of its first song there, and its
            // commonest artist's on Deezer.
            RadioSeed::Tracks(tracks) => {
                let yt = tracks.iter().take(8).find_map(|t| {
                    let refs = self.lib.stream_refs(*t).unwrap_or_default();
                    let r = refs.into_iter().find(|r| r.id.service == Service::Youtube)?;
                    Some((r.id.id, self.lib.get_track(*t).ok().flatten().map(|t| t.title).unwrap_or_default()))
                });
                calls.extend(yt.map(|(id, title)| (Call::Radio(id), title)));
                // In the list's order: the first of the commonest.
                let mut count: Vec<(ArtistId, usize)> = Vec::new();
                for t in &tracks {
                    if let Some(a) = self.lib.get_track(*t).ok().flatten().and_then(|t| t.artist_credit.artist_ids().next()) {
                        match count.iter_mut().find(|(b, _)| *b == a) {
                            Some((_, n)) => *n += 1,
                            None => count.push((a, 1)),
                        }
                    }
                }
                let most = count.iter().map(|(_, n)| *n).max().unwrap_or(0);
                if let Some(&(a, _)) = count.iter().find(|(_, n)| *n == most) {
                    calls.extend(artist_call(self, a, &[Service::Deezer]));
                }
            }
            RadioSeed::Mood(_) | RadioSeed::Style { .. } | RadioSeed::Discover | RadioSeed::Daylist { .. } | RadioSeed::Wave(_) => {}
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
        let r = &self.recs.radio;
        let run = r.run.as_ref()?;
        run.queue_gen?;
        let wave_preset = r.wave_preset.filter(|(id, _)| *id == run.id).and_then(|(_, k)| k);
        Some(RadioSave {
            state: run.state.clone(),
            title: run.title.clone(),
            context: run.ctx,
            seed: run.seed,
            streams: run.streams.iter().map(|(t, r)| (*t, r.clone())).collect(),
            pool: run.pool.values().cloned().collect(),
            autoplay: run.autoplay,
            added_at: places_of(&queue_order(&self.queue), &run.added),
            wave_preset,
        })
    }

    /// The radio of the restored queue goes on.
    pub(crate) fn radio_restore(&mut self, save: RadioSave) {
        // Its own entries again, by their places (a track that moved is
        // the user's now).
        let added = entries_at(&queue_order(&self.queue), &save.added_at);
        let r = &mut self.recs.radio;
        r.seq += 1;
        // A wave keeps the card it was started from.
        r.wave_preset = matches!(save.state.seed, RadioSeed::Wave(_)).then_some((r.seq, save.wave_preset));
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
            marks: Vec::new(),
            pending_new: Vec::new(),
            resume: false,
            autoplay: save.autoplay,
            added,
            heard: Vec::new(),
            asked: HashSet::new(),
            queued: (0, 0),
            via: HashMap::new(),
            redo: None,
            stale: false,
            forgotten: Vec::new(),
            wave_dry: None,
            refill: None,
        });
        self.play_context = self.recs.radio.run.as_ref().map_or(self.play_context, |r| r.ctx);
    }
}

/// The seed of a list's radio: the catalog's tracks of a library list
/// ([`RadioSeed::Tracks`]); a service's list the catalog lacks songs of,
/// blended whole as a service song's seed ([`RadioSeed::Stream`] under its
/// first song): the mean sound of its analysed songs and every library
/// artist its songs match. None: nothing to grow from.
fn list_seed(
    ids: &[TrackId],
    streams: &[StreamTrack],
    in_catalog: impl Fn(TrackId) -> bool,
    vector: impl Fn(TrackId) -> Option<nikgob_recs::Vector>,
    artist: impl Fn(&str) -> Option<ArtistId>,
) -> Option<RadioSeed> {
    if streams.is_empty() || ids.iter().all(|t| in_catalog(*t)) {
        let known: Vec<TrackId> = ids.iter().copied().filter(|t| in_catalog(*t)).collect();
        return (!known.is_empty()).then_some(RadioSeed::Tracks(known));
    }
    let first = streams.first()?;
    let vs: Vec<nikgob_recs::Vector> = ids.iter().filter_map(|t| vector(*t)).collect();
    let vector = (!vs.is_empty()).then(|| std::array::from_fn(|k| vs.iter().map(|v| v[k]).sum::<f32>() / vs.len() as f32));
    let mut artists: Vec<ArtistId> = Vec::new();
    for a in streams.iter().flat_map(|t| &t.artists).filter_map(|a| artist(&a.name)) {
        if !artists.contains(&a) {
            artists.push(a);
        }
    }
    Some(RadioSeed::Stream { id: first.id.clone(), vector, artists })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64) -> Entry {
        Entry { id, track: TrackId::new() }
    }

    /// What autoplay and a radio added comes back by its places after a
    /// restart (new entry ids); a place that holds another track now does
    /// not.
    #[test]
    fn added_entries_come_back_by_their_places() {
        let before: Vec<Entry> = (1..=5).map(entry).collect();
        let places = places_of(&before, &HashSet::from([2, 4, 5]));
        assert_eq!(places, vec![(1, before[1].track), (3, before[3].track), (4, before[4].track)]);
        // The next run: the same tracks under new ids, the last one moved.
        let mut after: Vec<Entry> = before.iter().map(|e| Entry { id: e.id + 100, track: e.track }).collect();
        after.swap(3, 4);
        assert_eq!(entries_at(&after, &places), HashSet::from([102]));
        // An older session (no places): nothing.
        assert!(entries_at(&after, &[]).is_empty());
    }

    /// A wave's card is saved with it, and a session from before it was
    /// saved reads as the plain wave.
    #[test]
    fn a_wave_keeps_its_card_in_the_session() {
        let save = RadioSave {
            state: Engine::new(RadioSeed::Wave(nikgob_recs::wave::WaveSeed::plain(0.25))),
            title: "Моя волна".into(),
            context: EventContext::HomeSection,
            seed: 1,
            streams: Vec::new(),
            pool: Vec::new(),
            autoplay: false,
            added_at: Vec::new(),
            wave_preset: Some(v::MixKey::Calm),
        };
        let json = serde_json::to_value(&save).unwrap();
        let back: RadioSave = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back.wave_preset, Some(v::MixKey::Calm));
        let mut old = json;
        old.as_object_mut().unwrap().remove("wave_preset");
        let back: RadioSave = serde_json::from_value(old).unwrap();
        assert_eq!(back.wave_preset, None);
    }

    fn song(id: &str, artist: &str) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), id);
        t.artists = vec![nikgob_core::streaming::ArtistRef::new(None, artist)];
        t
    }

    fn sound(x: f32) -> nikgob_recs::Vector {
        let mut v = [0.0; nikgob_recs::DIM];
        v[0] = x;
        v
    }

    /// A service's list the catalog lacks is blended whole: the mean sound
    /// of every analysed song of it and every library artist it matches,
    /// not its first song alone; a library list (or a service list the
    /// catalog has whole) is its catalog tracks.
    #[test]
    fn a_service_list_radio_blends_the_whole_list() {
        let ids: Vec<TrackId> = (0..3).map(|_| TrackId::new()).collect();
        let streams = vec![song("a", "Кино"), song("b", "Аквариум"), song("c", "Кино")];
        let (kino, aquarium) = (ArtistId::new(), ArtistId::new());
        let artist = |name: &str| match name {
            "Кино" => Some(kino),
            "Аквариум" => Some(aquarium),
            _ => None,
        };
        let vector = |t: TrackId| (t != ids[0]).then(|| sound(if t == ids[1] { 1.0 } else { 3.0 }));
        let seed = list_seed(&ids, &streams, |_| false, vector, artist);
        let Some(RadioSeed::Stream { id, vector, artists }) = seed else { panic!("{seed:?}") };
        assert_eq!(id, streams[0].id);
        assert_eq!(vector.map(|v| v[0]), Some(2.0));
        assert_eq!(artists, vec![kino, aquarium]);
        // The catalog has some of it: still the whole list.
        let some = list_seed(&ids, &streams, |t| t == ids[1], |_| None, |_| None);
        assert!(matches!(some, Some(RadioSeed::Stream { .. })), "{some:?}");
        // The catalog has all of it, or it is the library's own.
        assert_eq!(list_seed(&ids, &streams, |_| true, |_| None, |_| None), Some(RadioSeed::Tracks(ids.clone())));
        assert_eq!(list_seed(&ids, &[], |t| t != ids[0], |_| None, |_| None), Some(RadioSeed::Tracks(ids[1..].to_vec())));
        assert_eq!(list_seed(&ids, &[], |_| false, |_| None, |_| None), None);
        assert_eq!(list_seed(&[], &[], |_| true, |_| None, |_| None), None);
    }

    /// A list's radio plays none of the list's own songs unasked: its
    /// seed holds no track to play first, and a service list's first song
    /// counts as served, so what plays is the radio's picks, which keep
    /// out the disliked, the banned and (under «Миксы только из
    /// скачанного») the not downloaded.
    #[test]
    fn a_list_radio_plays_the_picks_not_the_list() {
        let ids: Vec<TrackId> = (0..2).map(|_| TrackId::new()).collect();
        let streams = vec![song("a", "x"), song("b", "y")];
        let seed = list_seed(&ids, &streams, |_| false, |_| None, |_| None).expect("a seed");
        assert!(!matches!(seed, RadioSeed::Track(_)));
        let engine = Engine::new(seed);
        assert!(engine.served().any(|i| *i == Item::Stream(streams[0].id.clone())));
        assert!(!engine.served().any(|i| matches!(i, Item::Track(_))));
        let library = Engine::new(list_seed(&ids, &[], |_| true, |_| None, |_| None).expect("a seed"));
        assert_eq!(library.served().count(), 0);
    }

    #[test]
    fn contexts_read_as_phrases() {
        assert_eq!(context_of("Радио · Кино"), "радио «Кино»");
        assert_eq!(context_of("Спокойное"), "микса «Спокойное»");
        assert_eq!(context_of(&list_title("альбому", "nyan.pop")), "радио по альбому «nyan.pop»");
        assert_eq!(list_title("плейлисту", "P"), "Радио по плейлисту «P»");
    }
}
