//! The runtime between the core services and a design system.
//!
//! One worker thread executes [`Command`]s from the UI against the library,
//! the play queue and the playback backend, and publishes the result into a
//! shared [`ViewState`]. The UI reads that state each frame under a short
//! read lock and never calls the library or the audio engine itself.

mod align;
mod analysis;
mod artist_names;
mod quality;
mod same_audio;
mod better_covers;
mod stream_covers;
mod convert;
mod enrich;
mod gain;
mod hd_covers;
mod home;
mod home_build;
mod home_v2;
mod import;
mod library_load;
mod library_search;
mod lyrics;
mod lyrics_keep;
mod playlist_cover;
mod recs;
mod search;
mod soulseek;
mod quiet;
pub use quiet::set_enabled as set_background_quiet;
mod tags;
mod torrents;
pub mod queue;
mod session;
mod sharing;
mod editions;
mod streams;
#[cfg(test)]
mod perf;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crossbeam_channel::{Receiver, Sender, select, tick};
use nikgob_core::model::{
    EndReason, EventContext, EventMeta, EventOrigin, EventType, LikeTarget, PlaylistId, SourceId, SourceKind, TrackId,
};
use nikgob_core::streaming::{Service as StreamKind, StreamService};
use nikgob_library::{Library, Sort, TrackSortKey};
use nikgob_view::{Command, LIKED_PLAYLIST, Status, Toast, ViewState};
use parking_lot::{RwLock, RwLockReadGuard};

pub use convert::Resolver;
pub use import::ImportSetup;
pub use lyrics::memory as lyrics_memory;
pub use nikgob_sources::khinsider::PageBrowser;
pub use recs::{
    HomePreview, ModelPreview, PreviewTile, preview_home, preview_home_with, preview_ready_tiles, preview_replay, preview_replay_with,
    preview_tile_days,
};
pub use nikgob_import::ImportConfig;
pub use nikgob_metadata::{Metadata, MetadataConfig};
pub use streams::{SOUNDCLOUD_WINDOW_HINT, StreamSchedule, WINDOW_HINT, WebLogin, WebLoginError, window_hint};
use import::{IMPORT_ID, ImportService, Internal};
use queue::{Entry, Queue};

/// How many of the tracks before the current one the queue view lists.
const HISTORY_SHOWN: usize = 50;

/// In a [`PlaybackEvent::Error`] of a SoundCloud track: SoundCloud streams
/// it only with DRM, and the backend can play it through SoundCloud's own
/// player instead (`nikgob-sc-widget`). The same source is then played
/// again, once per entry; the backend knows the track now and takes that
/// route. If that fails too, its other sources are tried as for any error.
pub const OFFICIAL_PLAYER_ROUTE: &str = "[soundcloud official player]";

/// Where the audio of a [`PlayRequest`] comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaySource {
    /// A local file.
    File(PathBuf),
    /// A track of a streaming service, opened through the
    /// [`StreamService`] registered for `service` (`id` is the remote id,
    /// the locator of the library source).
    Stream { service: StreamKind, id: String },
}

/// What the playback backend should play: a queue entry id and its audio.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayRequest {
    pub id: u64,
    pub source: PlaySource,
    /// ReplayGain/R128 track gain to apply, in dB (0 for streams: their
    /// gain comes with the stream).
    pub gain_db: f32,
    pub duration: Option<Duration>,
    /// The version the track plays as («Скорость»; `Normal` when that is
    /// switched off).
    pub speed: nikgob_view::SpeedVariant,
    /// A version other than `Normal` keeps the timbre of its voices.
    pub keep_timbre: bool,
}

/// Events the backend reports back (from any thread) through
/// [`Runtime::playback_sender`].
#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackEvent {
    Started { id: u64 },
    /// The preloaded next item took over at the sample boundary.
    TrackChanged { to: u64 },
    Position { id: u64, position: Duration },
    /// [`Self::Position`] as read at `at`: it moved on while the event
    /// waited to be taken, which is made up for.
    PositionAt { id: u64, position: Duration, at: Instant },
    Paused,
    Resumed,
    Ended { id: u64 },
    Error { id: u64, message: String },
    /// What a stream item actually plays ("Opus ~160 кбит/с"), once opened.
    Quality { id: u64, label: String },
    /// How long the item is, as the engine found it (the container's
    /// length, else the stream's): the length of a track that has none.
    Duration { id: u64, duration: Duration },
    /// The item plays at this speed after all, not at its request's (a
    /// live track of another player cannot be sped up).
    Speed { id: u64, speed: f32 },
}

/// The audio engine as the runtime sees it.
pub trait Playback: Send {
    fn play(&mut self, item: PlayRequest);
    /// Preload what follows the current item (gapless); None clears it.
    fn set_next(&mut self, item: Option<PlayRequest>);
    fn pause(&mut self);
    fn resume(&mut self);
    fn stop(&mut self);
    fn seek(&mut self, position: Duration);
    /// 0..1 perceptual.
    fn set_volume(&mut self, volume: f32);
    fn position(&self) -> Duration;
    /// Crossfade (None = gapless) and loudness normalization.
    fn configure(&mut self, _crossfade: Option<Duration>, _normalization: bool) {}
    /// The equalizer: on or off, and each band's gain in dB.
    fn set_equalizer(&mut self, _on: bool, _gains: [f32; nikgob_view::EQ_BANDS]) {}
    /// Item `id` (the playing one, or the one preloaded) plays as `speed`
    /// from now on, going on from where it is. Positions stay the track's
    /// own time: at 1.2 the position moves 1.2 s a second.
    fn set_speed(&mut self, _id: u64, _speed: nikgob_view::SpeedVariant, _keep_timbre: bool) {}
    /// Item `id` (playing, preloaded or still opening) takes `gain_db` as
    /// its gain after all: its file's tags were read after it was handed
    /// over. One already sounding glides to it, never jumps.
    fn set_gain(&mut self, _id: u64, _gain_db: f32) {}
    /// Switches the output device (None = system default). Returns true if
    /// the engine was restarted and lost what it was playing.
    fn set_device(&mut self, _device: Option<String>) -> bool {
        false
    }
}

/// A backend that plays nothing (tests, or when no device is available).
#[derive(Default)]
pub struct NullPlayback;

impl Playback for NullPlayback {
    fn play(&mut self, _: PlayRequest) {}
    fn set_next(&mut self, _: Option<PlayRequest>) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
}

/// Services added after start-up (import, downloads) receive these.
pub trait Service: Send {
    /// Returns true if the command was handled.
    fn handle(&mut self, command: &Command, state: &mut ViewState) -> bool;
}

pub struct RuntimeConfig {
    pub library: Library,
    /// Root of the managed `Music/` folder; relative cover paths resolve here.
    pub library_dir: PathBuf,
    pub prefs: nikgob_view::Prefs,
    /// Where `Prefs` are saved (JSON); None = not persisted.
    pub prefs_path: Option<PathBuf>,
    /// `settings.toml` (secrets such as the Prowlarr key).
    pub settings_path: Option<PathBuf>,
    /// Import, Inbox, Trash and lyrics; None = playback only.
    pub import: Option<ImportSetup>,
    /// The streaming services (at most one per [`StreamKind`]). Tracks whose
    /// sources are streams play through them; without one, such a source is
    /// skipped.
    pub streams: Vec<Arc<dyn StreamService>>,
}

enum Msg {
    Cmd(Command),
    Service(Box<dyn Service>),
    Indexer(Arc<dyn nikgob_core::indexer::Indexer>),
    Source(Arc<dyn nikgob_core::source::Source>),
    OdesliBase(String),
    GraphMetadata(Box<nikgob_metadata::Metadata>),
    WebLogin(StreamKind, Arc<dyn WebLogin>),
    KhinsiderBrowser(Arc<dyn nikgob_sources::khinsider::PageBrowser>),
    Shutdown,
}

/// Handle to the running worker.
pub struct Runtime {
    tx: Sender<Msg>,
    pb_tx: Sender<PlaybackEvent>,
    state: Arc<RwLock<ViewState>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Runtime {
    /// Starts the worker. `wake` is called after every state change so the
    /// UI can repaint (it must be cheap and must not block).
    pub fn start(
        config: RuntimeConfig,
        playback: Box<dyn Playback>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::start_scheduled(config, playback, wake, StreamSchedule::default())
    }

    /// [`Self::start`] with other streaming intervals (tests make them
    /// short).
    pub fn start_scheduled(
        config: RuntimeConfig,
        playback: Box<dyn Playback>,
        wake: Arc<dyn Fn() + Send + Sync>,
        schedule: StreamSchedule,
    ) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (pb_tx, pb_rx) = crossbeam_channel::unbounded();
        let (in_tx, in_rx) = crossbeam_channel::unbounded();
        let mut initial_prefs = config.prefs.clone();
        if initial_prefs.masters_dir.as_os_str().is_empty() {
            if let Some(imp) = &config.import {
                initial_prefs.masters_dir = imp.config.masters_dir();
            } else if !initial_prefs.library_dir.as_os_str().is_empty() {
                initial_prefs.masters_dir = initial_prefs.library_dir.join(".musiclib").join("masters");
            }
        }
        let state = Arc::new(RwLock::new(ViewState {
            prefs: initial_prefs,
            home: nikgob_view::HomeView { loading: true, ..Default::default() },
            ..ViewState::default()
        }));
        let worker = {
            let state = state.clone();
            std::thread::Builder::new()
                .name("nikgob-runtime".into())
                .spawn(move || {
                    let mut w = Worker::new(config, playback, state, wake, in_tx);
                    w.streaming.schedule = schedule;
                    w.run(rx, pb_rx, in_rx);
                })
                .expect("spawn runtime thread")
        };
        let rt = Self {
            tx,
            pb_tx,
            state,
            worker: Some(worker),
        };
        rt.send(Command::LoadHome);
        rt.send(Command::LoadLibrary);
        rt
    }

    pub fn state(&self) -> RwLockReadGuard<'_, ViewState> {
        self.state.read()
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(Msg::Cmd(command));
    }

    pub fn send_all(&self, commands: impl IntoIterator<Item = Command>) {
        for c in commands {
            self.send(c);
        }
    }

    /// Where the playback backend reports its events.
    pub fn playback_sender(&self) -> Sender<PlaybackEvent> {
        self.pb_tx.clone()
    }

    pub fn add_service(&self, service: Box<dyn Service>) {
        let _ = self.tx.send(Msg::Service(service));
    }

    /// Uses this torrent indexer instead of the one from settings.toml
    /// (another backend, or a fake one in tests).
    pub fn set_indexer(&self, indexer: Arc<dyn nikgob_core::indexer::Indexer>) {
        let _ = self.tx.send(Msg::Indexer(indexer));
    }

    /// One more download source (Soulseek, or a fake one in tests); it
    /// replaces a source of the same kind. Whether it is used is its switch
    /// on «Источники».
    pub fn add_source(&self, source: Arc<dyn nikgob_core::source::Source>) {
        let _ = self.tx.send(Msg::Source(source));
    }

    /// Logs `service` in through this sign-in window of the app for a
    /// [`nikgob_core::streaming::LoginInput::Browser`] login (YouTube Music's
    /// Google sign-in; see [`WebLogin`]). A logout also forgets its session.
    pub fn set_web_login(&self, service: StreamKind, login: Arc<dyn WebLogin>) {
        let _ = self.tx.send(Msg::WebLogin(service, login));
    }

    /// The hidden webview khinsider asks when a page is still a bot check.
    /// None (Linux without WebKit) leaves the HTTP answer.
    pub fn set_khinsider_browser(&self, browser: Arc<dyn nikgob_sources::khinsider::PageBrowser>) {
        let _ = self.tx.send(Msg::KhinsiderBrowser(browser));
    }

    /// Asks this Odesli base url for share links instead of
    /// api.song.link (tests).
    pub fn set_odesli_base(&self, base: impl Into<String>) {
        let _ = self.tx.send(Msg::OdesliBase(base.into()));
    }

    /// The graph of similar music asks this online metadata (tests: its
    /// clients pointed at a mock server) instead of the import's.
    pub fn set_graph_metadata(&self, m: nikgob_metadata::Metadata) {
        let _ = self.tx.send(Msg::GraphMetadata(Box::new(m)));
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

/// The toast of «Добавить в плейлист»: what was added and what the
/// playlist had already.
fn added_to_playlist(name: &str, added: usize, already: usize) -> String {
    match (added, already) {
        (0, 0) => format!("Добавлено в «{name}»"),
        (0, _) => format!("Уже есть в «{name}»"),
        (_, 0) => format!("Добавлено в «{name}»"),
        (_, 1) => format!("Добавлено {added}, 1 уже был в «{name}»"),
        _ => format!("Добавлено {added}, {already} уже были в «{name}»"),
    }
}

/// Up to this far apart, our position and the heard one are the same.
const POSITION_JITTER: Duration = Duration::from_millis(4);
/// Further apart than this, our position jumps to the heard one (a seek, a
/// loop, a long stall); closer, it runs faster or slower until they meet.
const POSITION_SNAP: Duration = Duration::from_millis(300);
/// The pace is off by the difference over this (half of it is gone by
/// the next report, a quarter second later) ...
const SLEW_SECS: f64 = 0.5;
/// ... with the pace at most this much off the normal one.
const SLEW_MAX: f64 = 0.25;
/// After we move the position ourselves (seek, start), a report the engine
/// sent before it heard about that can still be on its way: such a far-off
/// report is ignored for this long.
const POSITION_SETTLE: Duration = Duration::from_millis(500);
/// What to do with our position when the engine reports the heard one.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Follow {
    Keep,
    /// Go on from where we are at this pace.
    Rate(f64),
    /// Jump to the heard position.
    Snap,
}

/// A turn of the worker's loop longer than this is logged: while it lasts
/// no command is answered and no playback event is seen.
const SLOW_TURN: Duration = Duration::from_millis(250);

/// One turn of the worker's loop, timed (see [`SLOW_TURN`]).
#[derive(Default)]
struct Turn {
    began: Option<(Instant, String)>,
    laps: Vec<(&'static str, Instant)>,
}

impl Turn {
    fn begin(&mut self, what: impl FnOnce() -> String) {
        self.began = Some((Instant::now(), what()));
    }

    /// A part of the turn ends here.
    fn lap(&mut self, part: &'static str) {
        self.laps.push((part, Instant::now()));
    }

    fn end(self) {
        let Some((at, what)) = self.began else { return };
        let took = at.elapsed();
        if took < SLOW_TURN {
            return;
        }
        let mut from = at;
        let parts: Vec<String> = self
            .laps
            .iter()
            .map(|(part, to)| {
                let ms = to.duration_since(from).as_millis();
                from = *to;
                format!("{part} {ms} ms")
            })
            .collect();
        let parts = if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) };
        log::warn!("the player's loop was held for {} ms by {what}{parts}", took.as_millis());
    }
}

/// The first word of a value's `Debug` (an enum's variant), without
/// writing out the rest.
fn head_of(value: &impl std::fmt::Debug) -> String {
    struct Head(String);
    impl std::fmt::Write for Head {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for c in s.chars() {
                if !(c.is_alphanumeric() || c == '_') || self.0.len() >= 40 {
                    return Err(std::fmt::Error);
                }
                self.0.push(c);
            }
            Ok(())
        }
    }
    let mut head = Head(String::new());
    let _ = std::fmt::write(&mut head, format_args!("{value:?}"));
    head.0
}

/// How our position `ours` follows the heard one (`stalled`: it has not
/// moved since the last report) while playing at `rate`. It never moves
/// backwards: ahead of a stalled sound it waits, ahead of a moving one it
/// slows down.
#[cfg(test)]
fn follow(ours: Duration, heard: Duration, stalled: bool, rate: f64) -> Follow {
    follow_at(ours, heard, stalled, rate, 1.0)
}

/// [`follow`] for a track whose position moves `speed` seconds a second
/// (a track played faster or slower: positions are its own time).
fn follow_at(ours: Duration, heard: Duration, stalled: bool, rate: f64, speed: f64) -> Follow {
    if ours.abs_diff(heard) > POSITION_SNAP {
        return Follow::Snap;
    }
    if stalled && ours >= heard {
        return Follow::Rate(0.0);
    }
    if ours.abs_diff(heard) <= POSITION_JITTER {
        return if rate == speed { Follow::Keep } else { Follow::Rate(speed) };
    }
    let error = heard.as_secs_f64() - ours.as_secs_f64();
    Follow::Rate((speed + error / SLEW_SECS).clamp(speed - SLEW_MAX, speed + SLEW_MAX))
}

/// A toast no snackbar took away is dropped after this long.
const TOAST_KEPT: Duration = Duration::from_secs(60);

/// How the playing track is heard (profiles-v1 §11): the time it played,
/// that time weighted by how far the window was from the user (0 focused,
/// 0.5 in view behind another window, 1 hidden) where that is known, and
/// by the volume. Counted between the moments anything of it changes.
#[derive(Default)]
struct Heard {
    since: Option<Instant>,
    played_ms: f64,
    known_ms: f64,
    away_ms: f64,
    volume_ms: f64,
}

impl Heard {
    /// Counting starts over (a new track starts).
    fn reset(&mut self) {
        *self = Self { since: Some(Instant::now()), ..Self::default() };
    }

    /// The time since the last tick played (or not) in this state.
    fn tick(&mut self, playing: bool, away: Option<f32>, volume: f32) {
        let now = Instant::now();
        let ms = self.since.map_or(0.0, |s| now.duration_since(s).as_secs_f64() * 1000.0);
        self.since = Some(now);
        self.add(ms, playing, away, volume);
    }

    fn add(&mut self, ms: f64, playing: bool, away: Option<f32>, volume: f32) {
        if !playing {
            return;
        }
        self.played_ms += ms;
        self.volume_ms += ms * f64::from(volume);
        if let Some(a) = away {
            self.known_ms += ms;
            self.away_ms += ms * f64::from(a);
        }
    }

    /// The attention (when the window's state covers half the time played
    /// at least) and the mean volume.
    fn take(&self) -> (Option<f32>, Option<f32>) {
        if self.played_ms < 1.0 {
            return (None, None);
        }
        let attention = (self.known_ms >= 0.5 * self.played_ms).then(|| (self.away_ms / self.known_ms) as f32);
        (attention, Some((self.volume_ms / self.played_ms) as f32))
    }
}

struct Worker {
    lib: Library,
    res: Resolver,
    playback: Box<dyn Playback>,
    state: Arc<RwLock<ViewState>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    queue: Queue,
    /// What the backend is playing (entry id) and since when.
    playing: Option<Entry>,
    /// The entry we just handed to `playback.play()`: the backend reports
    /// that start too (as `Started` or `TrackChanged`), and it must not be
    /// mistaken for a gapless repeat-one loop of the same entry.
    restarted: Option<u64>,
    status: Status,
    position: Duration,
    position_at: Instant,
    /// How fast `position` moves from `position_at` (1 normal; a little off
    /// while it meets the heard position, 0 while the sound stalls).
    rate: f64,
    /// How fast the position of the playing track moves when all is well:
    /// its speed (1 as recorded).
    speed: f64,
    /// The version the playing entry was handed to the engine as, and
    /// whether it keeps its timbre.
    playing_fx: (nikgob_view::SpeedVariant, bool),
    /// The engine's last position report, to tell a stall.
    reported: Option<(u64, Duration)>,
    /// When we last moved `position` ourselves (see [`POSITION_SETTLE`]).
    jumped_at: Instant,
    volume: f32,
    muted: bool,
    /// The queue came back from the last run, paused here; nothing is
    /// loaded in the backend until play (see [`session`]).
    resume_at: Option<Duration>,
    /// The source of the playing entry failed mid-track (entry id, where it
    /// got to): the next one starts there, not from the top.
    failover: Option<(u64, Duration)>,
    /// The entry whose YouTube stream was opened once more after its URLs
    /// kept being refused mid-track (once per entry).
    stream_retried: Option<u64>,
    /// The entry whose DRM-only SoundCloud source went to SoundCloud's own
    /// player (once per entry, see [`OFFICIAL_PLAYER_ROUTE`]).
    official_player_tried: Option<u64>,
    /// `session.json` (volume and queue between runs) and what was last
    /// written to it.
    session_path: Option<PathBuf>,
    session_saved: String,
    /// Editions of one album found last time (see [`editions`]).
    editions: Vec<nikgob_import::EditionGroup>,
    services: Vec<Box<dyn Service>>,
    toast_id: u64,
    /// What «Очистить» last took out of the queue, for its «Отменить»: the
    /// entry playing then, what was before it and what came after it.
    cleared_queue: Option<(Option<u64>, Vec<TrackId>, Vec<TrackId>)>,
    revision: u64,
    play_context: EventContext,
    /// What the queue plays (`release:<id>`, `playlist:<id>`, a mix, a
    /// radio's seed): the history's `context_id`.
    play_context_id: Option<String>,
    /// The page of the queue the last run left (`release:<id>`…, see
    /// [`context_page`]) and the queue generation it came back in: the
    /// header's link while that queue plays.
    restored_page: Option<(u64, String)>,
    /// Who chose the next start, when the command that starts it knows.
    next_origin: Option<EventOrigin>,
    /// The queue generation of the last start: a start in another queue
    /// is the user's choice.
    origin_gen: Option<u64>,
    /// The playing entry's end is recorded already.
    ended: bool,
    /// How the playing entry is heard so far (its end's attention and
    /// volume).
    heard: Heard,
    /// Entries «Автовоспроизведение» added to continue an album's world.
    autoplay_entries: std::collections::HashSet<u64>,
    prefs_path: Option<PathBuf>,
    settings_path: Option<PathBuf>,
    exported_rev: u64,
    last_export: Instant,
    /// Where the last export went and [`Library::userdata_fingerprint`] then.
    exported_fp: Option<(PathBuf, Vec<i64>)>,
    import: Option<ImportService>,
    /// Registered streaming services (see [`RuntimeConfig::streams`]).
    streams: Vec<Arc<dyn StreamService>>,
    /// Sources of the playing entry that failed to open, and the one asked
    /// for last (see [`Worker::request`]).
    tried: Vec<SourceId>,
    playing_source: Option<(SourceId, SourceKind)>,
    /// What recent stream items really are, by item id (see
    /// [`PlaybackEvent::Quality`]; the preloaded next one opens early).
    qualities: Vec<(u64, String)>,
    indexer: Option<Arc<dyn nikgob_core::indexer::Indexer>>,
    /// Download sources other than the indexer's torrents (khinsider, the
    /// free services, Soulseek), whether on or not (docs/stage4.5.md).
    sources: Vec<Arc<dyn nikgob_core::source::Source>>,
    /// khinsider, so a webview can be given to it after start.
    khinsider: Option<Arc<nikgob_sources::khinsider::Khinsider>>,
    /// Qobuz and Tidal, so `sources.toml` can change their lists.
    qobuz: Option<Arc<nikgob_sources::qobuz::Qobuz>>,
    tidal: Option<Arc<nikgob_sources::tidal::Tidal>>,
    /// `sources.toml`'s mtime when it was last read.
    sources_mtime: Option<SystemTime>,
    candidates: Vec<nikgob_core::indexer::Candidate>,
    torrents: torrents::Torrents,
    /// Accounts, stream search and pages, likes sync, wishlist, sharing.
    streaming: streams::Streams,
    /// The background sound analysis and the vector index it fills.
    analysis: analysis::FileAnalysis,
    /// The background spectrum check of the files (fake lossless).
    quality: quality::QualityJob,
    /// Files that hold another song's audio: the background check, the
    /// pairs in «Подтверди», «В файле другая песня».
    same_audio: same_audio::SameAudioJob,
    /// Mixes, radio, discovery (stage 4).
    recs: recs::RecsState,
    /// Albums whose larger cover was looked up in this run.
    hd_asked: hd_covers::Asked,
    /// Albums and clips whose real art was looked up in this run.
    better_asked: better_covers::Asked,
    /// Real art of the stream tracks' video thumbnails on screen.
    stream_covers: stream_covers::StreamCovers,
    /// The album the queue plays (and the queue's generation then): its
    /// tracks the services add later join the queue.
    album_queue: Option<(nikgob_core::model::ReleaseId, u64)>,
    /// The Soulseek source (settings.toml `[soulseek]`).
    soulseek: soulseek::SoulseekService,
    /// «Загрузки → Раздача»: speeds and what was given away.
    sharing: sharing::Sharing,
    /// The lyrics aligner's thread (docs/lyrics-align.md).
    align: align::AlignService,
    /// Where its models are downloaded.
    models_dir: Option<PathBuf>,
    align_was_playing: bool,
    /// New likes of a large import that wait for their wishlist items
    /// ([`Worker::wish_for_likes`]), and how many items they made so far.
    wishes_later: std::collections::VecDeque<(TrackId, nikgob_core::model::WishlistOrigin)>,
    wishes_made: usize,
    /// The worker's own inbox (for threads started later).
    internal_tx: Sender<Internal>,
    /// The turn of the loop going on, timed: its parts are marked with
    /// [`Turn::lap`] wherever they are ([`SLOW_TURN`]).
    turn: Turn,
    /// The home page's builder thread ([`home_build`]).
    home_build: home_build::HomeBuild,
    /// Work background threads hand back to the loop ([`Later`]).
    later_tx: Sender<Later>,
    later_rx: Receiver<Later>,
    /// The file of the next track whose tags are read off the loop
    /// ([`Worker::read_facts_ahead`]).
    reading_ahead: Option<PathBuf>,
    /// Files' tags read off the loop ([`gain`]).
    facts_reads: gain::FactsReads,
    /// The playing track whose details wait for its file's tags, and that
    /// file ([`Worker::details_after_read`]).
    details_waiting: Option<(PathBuf, TrackId)>,
    /// The library's part of the search, read off the loop ([`library_search`]).
    library_search: library_search::LibrarySearch,
    /// The library's pages, read off the loop ([`library_load`]).
    library_load: library_load::LibraryLoad,
}

/// Work a background thread hands back to the worker's loop: what it is
/// (a slow turn names it) and what it does there.
pub(crate) struct Later {
    what: &'static str,
    run: Box<dyn FnOnce(&mut Worker) + Send>,
}

impl Later {
    pub(crate) fn new(what: &'static str, run: impl FnOnce(&mut Worker) + Send + 'static) -> Self {
        Self { what, run: Box::new(run) }
    }
}

impl Worker {
    fn new(
        config: RuntimeConfig,
        playback: Box<dyn Playback>,
        state: Arc<RwLock<ViewState>>,
        wake: Arc<dyn Fn() + Send + Sync>,
        internal: Sender<Internal>,
    ) -> Self {
        let (torrent_state, torrent_offline) = config
            .import
            .as_ref()
            .map(|i| (i.torrent_state_dir.clone(), i.torrent_offline))
            .unwrap_or_default();
        let slsk_state = config.import.as_ref().map(|i| i.torrent_state_dir.with_file_name("soulseek"));
        let sharing = sharing::Sharing::new(config.import.as_ref().and_then(|i| i.torrent_state_dir.parent()));
        let soulseek = soulseek::SoulseekService::new(config.library_dir.clone(), slsk_state);
        let streaming = streams::Streams::new(internal.clone());
        let analysis = analysis::FileAnalysis::new(internal.clone());
        let quality = quality::QualityJob::new(internal.clone());
        let same_audio = same_audio::SameAudioJob::new(internal.clone());
        let recs = recs::RecsState::new(internal.clone());
        let models_dir = config.import.as_ref().and_then(|i| i.models_dir.clone());
        let internal_tx = internal.clone();
        let import = config
            .import
            .and_then(|setup| ImportService::start(setup, config.library.clone(), internal));
        let mut playback = playback;
        let session_path = config.prefs_path.as_ref().map(|p| p.with_file_name("session.json"));
        let saved = session_path.as_deref().map(session::Session::load).unwrap_or_default();
        let volume = saved.volume.unwrap_or(0.8).clamp(0.0, 1.0);
        playback.set_volume(if saved.muted { 0.0 } else { volume });
        playback.configure(crossfade(&config.prefs), config.prefs.normalization);
        playback.set_equalizer(config.prefs.eq_on, config.prefs.eq_gains);
        {
            // The versions chosen for tracks («Скорость»).
            let chosen: std::collections::HashMap<TrackId, nikgob_view::SpeedVariant> = config
                .library
                .track_fx()
                .unwrap_or_else(|e| {
                    log::warn!("the speeds of the tracks could not be read: {e}");
                    Vec::new()
                })
                .into_iter()
                .filter_map(|(track, key)| Some((track, nikgob_view::SpeedVariant::from_key(&key)?)))
                .collect();
            // Their names, for «Не нравится и скорость».
            let names: std::collections::HashMap<TrackId, nikgob_view::DislikedTrack> =
                chosen.keys().filter_map(|t| speed_name(&config.library, *t).map(|n| (*t, n))).collect();
            let mut st = state.write();
            let enabled = st.prefs.speed_enabled;
            st.speeds = nikgob_view::TrackSpeeds { enabled, chosen: Arc::new(chosen) };
            st.speed_names = Arc::new(names);

        }
        config.library.set_album_likes_tracks(config.prefs.album_like_tracks);
        let web = search::web_sources_from(config.settings_path.as_deref());
        let sources_mtime = config.settings_path.as_ref().and_then(|p| {
            std::fs::metadata(nikgob_sources::config::beside(p)).and_then(|m| m.modified()).ok()
        });
        let res = Resolver::new(config.library.clone(), config.library_dir);
        let home_build = home_build::HomeBuild::start(&res, config.prefs_path.as_deref());
        let (later_tx, later_rx) = crossbeam_channel::unbounded();
        let library_search = library_search::LibrarySearch::start(&res, later_tx.clone());
        let library_load = library_load::LibraryLoad::start(&res, later_tx.clone());
        Self {
            res,
            lib: config.library,
            playback,
            state,
            wake,
            queue: Queue::default(),
            playing: None,
            restarted: None,
            status: Status::Stopped,
            position: Duration::ZERO,
            position_at: Instant::now(),
            rate: 1.0,
            speed: 1.0,
            playing_fx: (nikgob_view::SpeedVariant::Normal, false),
            reported: None,
            jumped_at: Instant::now(),
            volume,
            muted: saved.muted,
            resume_at: None,
            failover: None,
            stream_retried: None,
            official_player_tried: None,
            session_path,
            session_saved: String::new(),
            editions: Vec::new(),
            services: Vec::new(),
            toast_id: 0,
            cleared_queue: None,
            revision: 0,
            play_context: EventContext::Other,
            play_context_id: None,
            restored_page: None,
            next_origin: None,
            origin_gen: None,
            ended: true,
            heard: Heard::default(),
            autoplay_entries: Default::default(),
            prefs_path: config.prefs_path,
            settings_path: config.settings_path.clone(),
            exported_rev: 0,
            last_export: Instant::now(),
            exported_fp: None,
            indexer: search::indexer_from(config.settings_path.as_deref()),
            sources: web.list,
            khinsider: web.khinsider,
            qobuz: web.qobuz,
            tidal: web.tidal,
            sources_mtime,
            candidates: Vec::new(),
            torrents: torrents::Torrents::new(torrent_state, torrent_offline),
            import,
            streams: config.streams,
            tried: Vec::new(),
            playing_source: None,
            qualities: Vec::new(),
            streaming,
            analysis,
            quality,
            same_audio,
            hd_asked: Default::default(),
            better_asked: Default::default(),
            stream_covers: Default::default(),
            album_queue: None,
            recs,
            soulseek,
            sharing,
            align: Default::default(),
            models_dir,
            align_was_playing: false,
            wishes_later: Default::default(),
            wishes_made: 0,
            internal_tx,
            turn: Turn::default(),
            home_build,
            later_tx,
            later_rx,
            reading_ahead: None,
            facts_reads: gain::FactsReads::default(),
            details_waiting: None,
            library_search,
            library_load,
        }
    }

    fn run(&mut self, rx: Receiver<Msg>, pb: Receiver<PlaybackEvent>, internal: Receiver<Internal>) {
        let ticker = tick(self.streaming.schedule.tick);
        let fast = tick(Duration::from_secs(1));
        let homes = self.home_build.done.clone();
        let later = self.later_rx.clone();
        // The last run's home page at once; this run's is built while the
        // rest below starts (the command the start sent builds it again
        // once the loop runs).
        self.show_kept_home();
        self.load_home();
        // Once each, after a backup: doubles of a song, an artist or an
        // album become one, and every album has a line per place it counts.
        self.lib.repair_catalog();
        // Once: the videos that are no music an old import left in its
        // playlists go from them (entries only).
        match self.lib.drop_imported_non_music(streams::music_video_title) {
            Ok(0) => {}
            Ok(n) => log::info!("library: repair of imported non-music entries: {n}"),
            Err(e) => log::warn!("library: repair of imported non-music entries: {e}"),
        }
        self.export_if_stale();
        self.load_torrent_history();
        self.publish_downloads();
        self.resume_torrents();
        self.soulseek_apply();
        self.start_enrich();
        self.restore_session();
        self.offer_editions();
        self.wish_restart();
        self.publish_sources();
        self.start_streams();
        self.start_analysis();
        self.start_quality();
        self.start_same_audio();
        self.start_align();
        self.dislikes_load(false);
        loop {
            // What this turn of the loop does, for [`SLOW_TURN`].
            self.turn = Turn::default();
            select! {
                recv(rx) -> m => match m {
                    Ok(Msg::Cmd(c)) => {
                        self.turn.begin(|| format!("command {}", head_of(&c)));
                        self.command(c);
                    }
                    Ok(Msg::Service(s)) => self.services.push(s),
                    Ok(Msg::Indexer(i)) => {
                        self.turn.begin(|| "the indexer".to_owned());
                        self.indexer = Some(i);
                        self.publish_sources();
                        self.wish_sources_ready();
                        self.wish_tick();
                    }
                    Ok(Msg::Source(s)) => {
                        self.turn.begin(|| "a source".to_owned());
                        // One per kind: a new one of a kind replaces the old.
                        self.sources.retain(|x| x.key() != s.key());
                        self.sources.push(s);
                        self.publish_sources();
                        self.wish_sources_ready();
                        self.wish_tick();
                    }
                    Ok(Msg::OdesliBase(base)) => self.set_odesli_base(&base),
                    Ok(Msg::GraphMetadata(m)) => self.set_graph_metadata(*m),
                    Ok(Msg::WebLogin(service, login)) => self.set_web_login(service, login),
                    Ok(Msg::KhinsiderBrowser(browser)) => {
                        if let Some(k) = &self.khinsider {
                            k.set_browser(browser);
                        }
                    }
                    Ok(Msg::Shutdown) | Err(_) => {
                        self.record_closed();
                        break;
                    }
                },
                recv(pb) -> e => if let Ok(e) = e {
                    self.turn.begin(|| format!("playback {}", head_of(&e)));
                    self.playback_event(e);
                },
                recv(internal) -> m => if let Ok(m) = m {
                    self.turn.begin(|| m.turn_name());
                    self.internal(m);
                },
                recv(homes) -> m => if let Ok(done) = m {
                    self.turn.begin(|| "a home page came".to_owned());
                    self.home_built(done);
                },
                recv(later) -> m => if let Ok(l) = m {
                    let what = l.what;
                    self.turn.begin(|| what.to_owned());
                    (l.run)(self);
                },
                recv(fast) -> _ => {
                    self.turn.begin(|| "the second's poll".to_owned());
                    self.soloist_poll();
                    self.turn.lap("soloist");
                    self.poll_torrents();
                    self.turn.lap("torrents");
                    self.poll_share();
                    self.turn.lap("sharing");
                    self.wish_poll_direct();
                    self.turn.lap("wishes");
                    self.reload_sources_file();
                    self.turn.lap("sources file");
                },
                recv(ticker) -> _ => {
                    self.turn.begin(|| "the tick".to_owned());
                    self.expire_toasts();
                    self.maybe_export(false);
                    self.turn.lap("export");
                    self.save_session();
                    self.keep_home_due();
                    self.turn.lap("session");
                    self.stream_tick();
                    self.turn.lap("streams");
                    self.recs_tick();
                    self.turn.lap("recs");
                }
            }
            std::mem::take(&mut self.turn).end();
            self.analysis.set_playing(matches!(self.status, Status::Playing | Status::Buffering));
            self.quality.set_playing(matches!(self.status, Status::Playing | Status::Buffering));
            self.same_audio.set_playing(matches!(self.status, Status::Playing | Status::Buffering));
            self.align_playing(matches!(self.status, Status::Playing | Status::Buffering));
        }
        self.save_session();
        self.recs_flush_weights(true);
        self.keep_home(true);
        self.maybe_export(true);
        self.playback.stop();
        self.shutdown_torrents();
        self.soulseek.shutdown();
        self.sharing.save(true);
        self.streaming.analysis.shutdown();
        self.analysis.shutdown();
        self.quality.shutdown();
        self.same_audio.shutdown();
        self.align.shutdown();
    }

    /// Applies `f` under the state's write lock. The UI waits for this lock
    /// every frame, so view values are built before it (the library, the
    /// resolver and files are never touched inside `f`).
    fn publish(&mut self, f: impl FnOnce(&mut ViewState)) {
        {
            let mut st = self.state.write();
            f(&mut st);
            self.revision += 1;
            st.revision = self.revision;
        }
        (self.wake)();
    }

    fn toast(&mut self, text: impl Into<String>, action: Option<(String, Command)>) {
        self.push_toast(text.into(), action, None);
    }

    /// A toast whose button takes the user somewhere ([`nikgob_view::Link`]).
    fn toast_open(&mut self, text: impl Into<String>, label: &str, link: nikgob_view::Link) {
        self.push_toast(text.into(), None, Some((label.to_owned(), link)));
    }

    fn push_toast(&mut self, text: String, action: Option<(String, Command)>, open: Option<(String, nikgob_view::Link)>) {
        // A like or unlike makes the undo of the one before on the same
        // thing wrong («Убрано из Любимых · Отменить» after a like again):
        // it is withdrawn, so the snackbar says what the heart shows.
        if let Some((_, Command::Like(target, _))) = &action {
            let stale = |t: &Toast| matches!(&t.action, Some((_, Command::Like(old, _))) if old == target);
            if self.state.read().toasts.iter().any(|t| stale(t) && t.text != text) {
                self.publish(|s| s.toasts.retain(|t| !stale(t) || t.text == text));
            }
        }
        // The same message while it is up: its time starts over.
        let same = self.state.read().toasts.iter().position(|t| t.text == text && t.action == action && t.open == open);
        if let Some(k) = same {
            self.publish(|s| {
                let t = &mut s.toasts[k];
                t.at = Instant::now();
                t.again += 1;
            });
            return;
        }
        self.toast_id += 1;
        let t = Toast {
            id: self.toast_id,
            kind: nikgob_view::ToastKind::guess(&text),
            text,
            action,
            open,
            at: Instant::now(),
            long: false,
            again: 0,
        };
        self.publish(|s| s.toasts.push(t));
    }

    /// Toasts no snackbar host took away (the window was hidden): the UI
    /// shows one at a time and dismisses each as it ends, a toast waiting
    /// behind an undo too, so this is only the last resort.
    fn expire_toasts(&mut self) {
        let stale = {
            let s = self.state.read();
            // «Пока не закрою»: they stay until the user closes them.
            s.prefs.toast_s != 0 && s.toasts.iter().any(|t| t.at.elapsed() > TOAST_KEPT)
        };
        if stale {
            self.publish(|s| s.toasts.retain(|t| t.at.elapsed() <= TOAST_KEPT));
        }
    }

    fn log_err<T>(&mut self, what: &str, r: Result<T, nikgob_library::LibraryError>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                log::error!("{what}: {e}");
                self.toast(format!("Ошибка: {what}"), None);
                None
            }
        }
    }

    // ---------------------------------------------------------------- commands

    fn command(&mut self, c: Command) {
        for i in 0..self.services.len() {
            let mut handled = false;
            let mut st = self.state.write();
            if self.services[i].handle(&c, &mut st) {
                handled = true;
            }
            drop(st);
            if handled {
                self.revision += 1;
                self.state.write().revision = self.revision;
                (self.wake)();
                return;
            }
        }
        match c {
            Command::TogglePlay => match self.status {
                Status::Playing | Status::Buffering => self.pause(),
                Status::Paused => self.resume(),
                _ => {
                    if let Some(e) = self.queue.current() {
                        self.start(e);
                    }
                }
            },
            Command::Play => self.resume(),
            Command::Pause => self.pause(),
            Command::Next => {
                self.record_end(true, EndReason::UserNext);
                self.turn.lap("end recorded");
                match self.queue.skip_forward() {
                    Some(e) => self.start(e),
                    None => self.stop(),
                }
            }
            Command::Previous => {
                if self.position_now() > Duration::from_secs(3) || self.queue.current().is_none() {
                    self.seek(Duration::ZERO);
                } else if let Some(e) = self.queue.previous() {
                    self.user_switch(EventOrigin::Manual);
                    self.start(e);
                }
            }
            Command::Seek(p) => self.seek(p),
            // The app opens and resets the developer profile itself.
            Command::DevProfile(_) => {}
            Command::PickLyricsSource(t, src) => self.pick_lyrics_source(t, src),
            Command::SearchLyricsAgain(t) => self.search_lyrics_again(t),
            Command::CraftLyrics { track, source, target, deep } => self.craft_lyrics(track, source, target, deep),
            Command::ClearCraft => self.clear_craft(),
            Command::CancelCraft => self.cancel_craft(),
            Command::ShowLyricsAs(t, sync) => self.show_lyrics_as(t, sync),
            Command::SaveLyrics { track, source } => self.save_lyrics(track, source),
            Command::SaveOwnLyrics { track, lines, timed } => self.save_own_lyrics(track, lines, timed),
            Command::DropOwnLyrics(track) => self.drop_own_lyrics(track),
            Command::RestoreOwnLyrics(track) => self.restore_own_lyrics(track),
            Command::AlignModel { key, download } => self.align_model(&key, download),
            Command::SeekBy(ms) => {
                // Seconds as the listener counts them: more of the track
                // when it plays faster.
                let ms = (ms as f64 * self.speed).round() as i64;
                let p = self.position_now().as_millis() as i64 + ms;
                self.seek(Duration::from_millis(p.max(0) as u64));
            }
            Command::SetTrackSpeed { track, variant } => self.set_track_speed(track, variant),
            Command::ResetTrackSpeed(track) => self.reset_track_speed(track),
            Command::SetVolume(v) => {
                let v = v.clamp(0.0, 1.0);
                if v == self.volume && !self.muted {
                    return;
                }
                self.heard_tick();
                self.volume = v;
                self.muted = false;
                self.playback.set_volume(self.volume);
                self.publish_volume();
            }
            Command::ToggleMute => {
                self.heard_tick();
                self.muted = !self.muted;
                self.playback
                    .set_volume(if self.muted { 0.0 } else { self.volume });
                self.publish_volume();
            }
            Command::ToggleShuffle => {
                let on = !self.queue.shuffle;
                self.queue.set_shuffle(on);
                self.preload_next();
                self.publish_player();
            }
            Command::CycleRepeat => {
                self.queue.repeat = self.queue.repeat.next();
                self.preload_next();
                self.publish_player();
            }
            Command::PlayRelease { id, start, shuffle } => {
                let ids = self.release_tracks(id);
                let title = self
                    .lib
                    .get_release(id)
                    .ok()
                    .flatten()
                    .map(|r| format!("альбома «{}»", r.title));
                self.turn.lap("album read");
                self.play_list(ids, start, shuffle, nikgob_view::ListKind::Album, title, EventContext::Album, Some(format!("release:{id}")));
                // An album known in part plays at once; the rest of its
                // tracklist joins the queue when the services tell it.
                self.album_queue = Some((id, self.queue.generation()));
                self.complete_tracklist(id);
                self.turn.lap("tracklist");
            }
            Command::PlayPlaylist { id, start, shuffle } => {
                let (ids, name) = self.playlist_tracks(id, start);
                let name = format!("плейлиста «{name}»");
                let kind = self.playlist_kind(id);
                self.play_list(ids, start, shuffle, kind, Some(name), EventContext::Playlist, Some(format!("playlist:{id}")));
            }
            Command::PlayArtist { id, shuffle } => {
                let rows = self
                    .log_err("исполнитель", self.lib.tracks_of_artist(id, 0, 500))
                    .unwrap_or_default();
                let name = self.lib.get_artist(id).ok().flatten().map(|a| a.name);
                let ids = self.playable(&rows, None);
                // Without «Перемешать» it is the artist's «Слушать микс».
                self.play_list(ids, None, shuffle, nikgob_view::ListKind::Mix, name, EventContext::Artist, Some(format!("artist:{id}")));
            }
            Command::PlayTracks {
                tracks,
                start,
                context,
            } => self.play_tracks(&tracks, Some(start), context),
            Command::PlayAll { tracks, context } => self.play_tracks(&tracks, None, context),
            Command::PlayNext(t) => {
                self.queue.play_next(&t);
                self.preload_next();
                self.publish_player();
                self.toast("Сыграет следующим", None);
            }
            Command::Enqueue(t) => {
                self.queue.enqueue(&t);
                self.preload_next();
                self.publish_player();
                self.toast("Добавлено в очередь", None);
            }
            Command::PlayReleaseNext(id) => {
                let ids = self.release_tracks(id);
                if ids.is_empty() {
                    return;
                }
                self.queue.play_next(&ids);
                self.preload_next();
                self.publish_player();
                let title = self.lib.get_release(id).ok().flatten().map(|r| r.title).unwrap_or_default();
                self.toast(format!("Следующим: «{title}»"), None);
            }
            Command::EnqueueRelease(id) => {
                let ids = self.release_tracks(id);
                self.command(Command::Enqueue(ids));
            }
            Command::AddAlbumToPlaylist(playlist, id) => {
                let ids = self.release_tracks(id);
                if ids.is_empty() {
                    return;
                }
                self.command(Command::AddToPlaylist(playlist, ids));
            }
            Command::PlayPlaylistNext(id) => {
                let (ids, name) = self.playlist_tracks(id, None);
                if ids.is_empty() {
                    return;
                }
                self.queue.play_next(&ids);
                self.preload_next();
                self.publish_player();
                self.toast(format!("Следующим: «{name}»"), None);
            }
            Command::EnqueuePlaylist(id) => {
                let (ids, _) = self.playlist_tracks(id, None);
                if ids.is_empty() {
                    return;
                }
                self.command(Command::Enqueue(ids));
            }
            Command::EnqueueArtist(id) => {
                let rows = self
                    .log_err("исполнитель", self.lib.tracks_of_artist(id, 0, 2000))
                    .unwrap_or_default();
                let ids = self.playable(&rows, None);
                self.command(Command::Enqueue(ids));
            }
            Command::PlayQueueEntry(entry) => {
                if let Some(e) = self.queue.jump(entry) {
                    self.user_switch(EventOrigin::Manual);
                    self.start(e);
                }
            }
            Command::RemoveFromQueue(entry) => {
                self.queue.remove(entry);
                self.preload_next();
                self.publish_player();
            }
            Command::MoveInQueue { entry, anchor, after } => {
                self.queue.move_next_to(entry, anchor, after);
                self.preload_next();
                self.publish_player();
            }
            Command::ClearQueue => {
                let before: Vec<TrackId> = self.queue.history().iter().map(|e| e.track).collect();
                let after: Vec<TrackId> = self.queue.upcoming().iter().map(|e| e.track).collect();
                let playing = self.queue.current().map(|e| e.id);
                self.queue.clear_upcoming();
                self.preload_next();
                self.publish_player();
                if !before.is_empty() || !after.is_empty() {
                    self.cleared_queue = Some((playing, before, after));
                    self.toast("Очередь очищена", Some(("Отменить".into(), Command::UndoClearQueue)));
                }
            }
            Command::UndoClearQueue => {
                // Only onto the queue it was taken from: another list since
                // then is not this one's to fill.
                if let Some((playing, before, after)) = self.cleared_queue.take()
                    && self.queue.current().map(|e| e.id) == playing
                {
                    self.queue.restore_around(&before, &after);
                    self.preload_next();
                    self.publish_player();
                }
            }
            Command::SaveTracksAsPlaylist { name, tracks } => {
                let before = self.library_tracks();
                if let Some(p) = self.log_err("плейлист", self.lib.create_playlist(&name)) {
                    let _ = self.log_err("плейлист", self.lib.playlist_add_many(p.id, &tracks));
                    self.toast(format!("Плейлист «{name}» сохранён"), None);
                    self.library_edited(before, Vec::new());
                }
            }
            Command::HideHomeCard => {
                self.publish(|s| s.prefs.home_card = false);
                self.save_prefs();
                self.load_home();
                self.toast(
                    "Карточка скрыта. Включить её снова можно в Настройках → Внешний вид → Главная.",
                    Some(("Вернуть".into(), Command::SetPref(nikgob_view::PrefPatch::HomeCard(true)))),
                );
                self.publish(|s| {
                    if let Some(t) = s.toasts.last_mut() {
                        t.long = true;
                    }
                });
            }
            Command::SaveQueueAsPlaylist(name) => {
                let tracks = self.queue.tracks();
                let before = self.library_tracks();
                if let Some(p) = self.log_err("плейлист", self.lib.create_playlist(&name)) {
                    let _ = self.log_err("плейлист", self.lib.playlist_add_many(p.id, &tracks));
                    self.toast(format!("Плейлист «{name}» сохранён"), None);
                    self.library_edited(before, Vec::new());
                }
            }
            Command::Like(LikeTarget::Release(id), on) => self.like_release(id, on),
            Command::Like(target, on) => {
                let before = self.library_tracks();
                let r = if on {
                    self.lib.like(target)
                } else {
                    self.lib.unlike(target)
                };
                if self.log_err("лайк", r).is_some() {
                    let mut wished = false;
                    if let LikeTarget::Track(t) = target {
                        if on {
                            self.liked_undislikes(t);
                            self.new_liked(t);
                        }
                        let _ = self.lib.record_event(
                            t,
                            if on { EventType::Like } else { EventType::Unlike },
                            None,
                            None,
                            EventContext::Other,
                        );
                        wished = self.track_liked(t, on);
                    }
                    // A like that downloads says so (a like of a track
                    // without a file puts it into «Хочу скачать»).
                    let text = match (on, wished) {
                        (true, true) if self.can_download() => "В Любимых · скачаю в фоне",
                        (true, true) => "В Любимых · скачается, когда подключишь торренты",
                        (true, false) => "Добавлено в Любимые",
                        (false, _) => "Убрано из Любимых",
                    };
                    self.toast(text, Some(("Отменить".into(), Command::Like(target, !on))));
                    // Its album's cards are read again below.
                    if let LikeTarget::Track(t) = target {
                        let _ = self.single_follows_like(t, on);
                    }
                    self.refresh_liked(target, on, before);
                }
            }
            Command::CreatePlaylist(name) => {
                if self.log_err("плейлист", self.lib.create_playlist(&name)).is_some() {
                    self.library_edited(Held::Same, Vec::new());
                }
            }
            Command::RenamePlaylist(id, name) => {
                if self.log_err("плейлист", self.lib.rename_playlist(id, &name)).is_some() {
                    self.library_edited(Held::Same, Vec::new());
                    self.load_playlist(id);
                }
            }
            Command::DeletePlaylist(id) => {
                let before = self.library_tracks();
                if self.log_err("плейлист", self.lib.delete_playlist(id)).is_some() {
                    self.toast("Плейлист удалён", None);
                    self.library_edited(before, Vec::new());
                }
            }
            Command::AddToPlaylist(id, tracks) => {
                let before = self.library_tracks();
                // Only what it does not have yet: never a track twice.
                let (mut added, mut already) = (0, 0);
                if id == LIKED_PLAYLIST {
                    let mut seen = std::collections::HashSet::new();
                    for t in tracks.iter().filter(|t| seen.insert(**t)) {
                        if self.lib.is_liked(*t).unwrap_or(false) {
                            already += 1;
                        } else if self.lib.like(*t).is_ok() {
                            added += 1;
                            self.track_liked(*t, true);
                        }
                    }
                } else if let Some(n) = self.log_err("плейлист", self.lib.playlist_add_missing(id, &tracks)) {
                    (added, already) = n;
                }
                let name = if id == LIKED_PLAYLIST {
                    "Любимые".to_owned()
                } else {
                    self.lib
                        .get_playlist(id)
                        .ok()
                        .flatten()
                        .map_or_else(String::new, |p| p.name)
                };
                self.toast(added_to_playlist(&name, added, already), None);
                let releases = if id == LIKED_PLAYLIST { self.releases_of(&tracks) } else { Vec::new() };
                self.library_edited(before, releases);
                self.playlist_edited(id);
            }
            Command::RemoveFromPlaylist(id, track) => {
                let before = self.library_tracks();
                // The way back, in its toast.
                let mut undo = None;
                if id == LIKED_PLAYLIST {
                    if self.log_err("лайк", self.lib.unlike(track)).is_some() {
                        undo = Some(("Убрано из Любимых".to_owned(), Command::Like(LikeTarget::Track(track), true)));
                    }
                } else if let Ok(entries) = self.lib.playlist_entries(id, 0, u32::MAX)
                    && let Some(at) = entries.iter().position(|e| e.track.id == track)
                    && self.log_err("плейлист", self.lib.playlist_remove(entries[at].entry.id)).is_some()
                {
                    let name = self.lib.get_playlist(id).ok().flatten().map_or_else(String::new, |p| p.name);
                    undo = Some((format!("Убрано из «{name}»"), Command::RestoreToPlaylist { playlist: id, track, at }));
                }
                self.load_playlist(id);
                let releases = if id == LIKED_PLAYLIST { self.releases_of(&[track]) } else { Vec::new() };
                self.library_edited(before, releases);
                self.playlist_edited(id);
                if let Some((text, cmd)) = undo {
                    self.toast(text, Some(("Отменить".into(), cmd)));
                }
            }
            Command::RestoreToPlaylist { playlist, track, at } => {
                let before = self.library_tracks();
                if self.log_err("плейлист", self.lib.playlist_insert(playlist, track, at)).is_some() {
                    self.load_playlist(playlist);
                    self.library_edited(before, Vec::new());
                    self.playlist_edited(playlist);
                }
            }
            Command::MoveInPlaylist {
                playlist,
                track,
                to,
            } => {
                if let Ok(entries) = self.lib.playlist_entries(playlist, 0, u32::MAX)
                    && let Some(e) = entries.iter().find(|e| e.track.id == track)
                {
                    let _ = self.log_err("плейлист", self.lib.playlist_move(e.entry.id, to));
                }
                self.load_playlist(playlist);
            }
            Command::TrashTrack(t) => self.delete_tracks(vec![t], "Трек"),
            Command::UndoTrash(t) => self.restore_tracks(vec![t]),
            Command::TrashRelease(id) => {
                let ids = self.release_track_ids(id);
                self.delete_tracks(ids, "Альбом");
            }
            Command::RestoreTracks(ids) => self.restore_tracks(ids),
            Command::DeleteForever(ids) => self.purge_tracks(ids),
            Command::DeleteReleaseForever(id) => {
                let ids = self.release_track_ids(id);
                self.purge_tracks(ids);
            }
            Command::EmptyTrash => self.empty_trash(),
            Command::LoadTrash => self.load_trash(),
            Command::LoadDownloaded => self.load_downloaded(),
            Command::SetPlaylistCoverMode(id, mode) => self.set_playlist_cover_mode(id, mode),
            Command::ImportFiles(paths) => self.import_paths(paths),
            Command::ImportFolder(dir) => self.import_paths(vec![dir]),
            Command::MergeEditions(id) => self.merge_editions(id),
            Command::KeepEditionsApart(id) => self.keep_editions_apart(id),
            Command::ConfirmChoice { id, option } if id & streams::WISH_ID != 0 => {
                self.wish_confirm(id, option);
            }
            Command::ConfirmChoice { id, option } if id & IMPORT_ID != 0 => {
                self.confirm_choice(id, option);
            }
            Command::ConfirmChoice { id, .. } if id & quality::QUALITY_ID != 0 => {
                // A fakes card is answered song by song (`ConfirmPicked`),
                // never as a whole.
                log::warn!("quality: a whole fakes card answered, ignored");
            }
            Command::ConfirmPicked { id, sources, accept } if id & quality::QUALITY_ID != 0 => {
                self.quality_picked(sources, accept);
            }
            Command::ConfirmChoice { id, option } if id & same_audio::SAME_AUDIO_ID != 0 => {
                self.same_audio_choice(id, option);
            }
            Command::ConfirmPicked { .. } => {
                log::warn!("picked songs of an unknown card");
            }
            Command::WrongFile(track) => self.wrong_file(track, None),
            Command::UndoWrongFile(track) => self.undo_wrong_file(track),
            Command::FindBetter(track) => self.find_better(track),
            Command::TrackInfo(track) => self.track_info(track),
            Command::FindBetterAlbum(release) => self.find_better_album(release),
            Command::UndoBetterAlbum(release) => self.undo_better_album(release),
            Command::FakeToMp3(track) => self.fake_to_mp3(track),
            Command::UndoFakeToMp3(track) => self.undo_fake_to_mp3(track),
            Command::RecordingApart(track) => self.recording_apart(track),
            Command::RestoreConvertedFlac => self.restore_converted_flac(),
            Command::LoadLibrary => self.load_library(),
            Command::DownloadsShown(shown) => self.torrents.shown = shown,
            Command::LoadTracks {
                offset,
                limit,
                sort,
            } => self.load_tracks(offset, limit, sort),
            Command::FilterTracks(q) => self.filter_tracks(&q),
            Command::SortReleases(sort) => {
                self.publish(|s| s.library.release_sort = sort);
                self.load_library();
            }
            Command::LoadRelease(id) => self.load_release(id),
            Command::LoadArtist(id) => self.load_artist(id),
            Command::LoadWorld(key) => self.load_world(&key),
            Command::LoadPlaylist(id) => {
                self.load_playlist(id);
                self.sync_on_open(id);
            }
            Command::LoadHome => self.load_home(),
            Command::HomeMore(row) => self.home_more(row),
            Command::Search(q) => self.search(q),
            Command::SetPrefs(p) => self.set_prefs(p),
            Command::SetPref(patch) => {
                let mut p = self.state.read().prefs.clone();
                patch.apply(&mut p);
                self.set_prefs(Box::new(p));
                if let nikgob_view::PrefPatch::HideMix(m) = patch {
                    self.mix_hidden(m);
                }
            }
            Command::MoveMastersDir(dir) => self.move_masters_dir(dir),
            Command::SetIndexer { url, api_key } => self.set_indexer(url, api_key),
            Command::SetSoulseek(form) => self.set_soulseek(form),
            Command::SetSoulseekShare { share, upload_limit_kbps } => self.set_soulseek_share(share, upload_limit_kbps),
            Command::SetSoulseekSlots(n) => self.set_soulseek_slots(n),
            Command::CheckIndexer { url, api_key } => self.check_indexer(url, api_key),
            Command::DismissToast(id) => {
                self.publish(|s| s.toasts.retain(|t| t.id != id));
            }
            Command::OpenTagEditor(t) => self.open_tag_editor(t),
            Command::LookupTags { track, query } => self.lookup_tags(track, query),
            Command::SaveTags { track, tags } => self.save_tags(track, tags),
            Command::CloseTagEditor => self.publish(|s| s.tag_editor = Default::default()),
            Command::SetPlaylistCover(id, path) => self.set_playlist_cover(id, path),
            Command::SetMetadata { contact, acoustid_key } => self.set_metadata(contact, acoustid_key),
            Command::SetLastfmKey(key) => self.set_lastfm_key(&key),
            Command::SetListenbrainzToken(token) => self.set_listenbrainz_token(&token),
            Command::OpenSourcesConfig => self.open_sources_config(),
            Command::RealCovers(urls) => self.real_covers_wanted(urls),
            Command::SearchTorrents(q) => self.search_torrents(q),
            c @ (Command::DownloadTorrent { .. }
            | Command::ListTorrentFiles(_)
            | Command::CloseTorrentFiles
            | Command::CleanSeeding
            | Command::AddTorrentFile(_)
            | Command::AddMagnet(_)
            | Command::PauseJob(..)
            | Command::CancelJob(_)) => self.torrent_command(c),
            Command::ConfirmChoice { .. } => {
                log::warn!("confirm choice for an unknown item");
            }
            c @ (Command::StreamLogin { .. }
            | Command::StreamLogout(_)
            | Command::RetryStreamAudio(_)
            | Command::Soloist(_)
            | Command::SetStreamSecrets { .. }
            | Command::SetSearchSources(_)
            | Command::LoadStream(_)
            | Command::PlayStreamTracks { .. }
            | Command::PlaySearch { .. }
            | Command::PlayStreamAlbum { .. }
            | Command::PlayStreamPlaylist { .. }
            | Command::PlayStreamNext(_)
            | Command::EnqueueStream(_)
            | Command::PlayStreamAlbumNext(_)
            | Command::EnqueueStreamAlbum(_)
            | Command::LikeStream { .. }
            | Command::LikeStreamAlbum { .. }
            | Command::DownloadStream { .. }
            | Command::DownloadStreamAlbum(_)
            | Command::DownloadTracks(_)
            | Command::DownloadRelease(_)
            | Command::FollowStreamPlaylist(_)
            | Command::AddStreamToPlaylist(..)
            | Command::AddStreamAlbumToPlaylist(..)
            | Command::Share(_)
            | Command::LoadWishlist
            | Command::WishlistDownload(_)
            | Command::WishlistDismiss(_)
            | Command::WishlistRestore(_)
            | Command::WishlistUndoDismiss { .. }
            | Command::WishlistRetry(_)
            | Command::WishlistRetryAll
            | Command::WishlistSaveFromYoutube(_)
            | Command::WishlistAttach { .. }
            | Command::ImportFromService(_)
            | Command::ImportExportFiles(_)
            | Command::ClearStreamCache
            | Command::WindowState { .. }
            | Command::PlayFromYoutube
            | Command::SetPlaylistSync { .. }
            | Command::LoadDiscography(_)
            | Command::FindSong { .. }
            | Command::CloseFindSong
            | Command::ReplaceTrack { .. }
            | Command::UndoReplace(_)) => {
                if let Some(c) = self.stream_command(c) {
                    log::warn!("unhandled {c:?}");
                }
            }
            c @ (Command::PlayMix { .. }
            | Command::StartRadio { .. }
            | Command::RecFeedback { .. }
            | Command::Dislike { .. }
            | Command::RefreshMix { .. }
            | Command::PlayBlend { .. }
            | Command::DownloadMix { .. }) => {
                if let Some(c) = self.recs_command(c) {
                    log::warn!("unhandled {c:?}");
                }
            }
        }
    }

    /// A track was liked / unliked here: the services hear about it, and a
    /// new like of a track without a file starts its download.
    /// A like / unlike of a track: to its services, and to «Хочу скачать».
    /// True when the like put the track there.
    fn track_liked(&mut self, track: TrackId, on: bool) -> bool {
        self.push_like(track, on);
        let wished = if on {
            self.wish_for_like(track, nikgob_core::model::WishlistOrigin::Like)
        } else {
            self.wish_drop_unstarted(track);
            false
        };
        self.publish_wishlist();
        self.wish_tick();
        wished
    }

    // ---------------------------------------------------------------- playback

    fn position_now(&self) -> Duration {
        self.position_at_instant(Instant::now())
    }

    /// Where we take playback to be at `now`, going from the last anchor at
    /// [`Self::rate`].
    fn position_at_instant(&self, now: Instant) -> Duration {
        if self.status == Status::Playing {
            self.position + now.saturating_duration_since(self.position_at).mul_f64(self.rate)
        } else {
            self.position
        }
    }

    /// Publishes only where playback is and how fast the view should move
    /// it (a position report, many a second: none of the rest changed).
    fn publish_clock(&mut self) {
        let now = Instant::now();
        let (position, rate) = (self.position_at_instant(now), self.rate);
        self.publish(|s| {
            s.player.position = position;
            s.player.at = now;
            s.player.rate = rate as f32;
        });
    }

    /// What to hand the engine for `e`: its best source that is usable and
    /// not in `skip` ([`Self::pick_source`]), with its gain and speed.
    /// Returns the source too; says in the log why nothing plays.
    fn request_skipping(
        &mut self,
        e: Entry,
        skip: &[SourceId],
    ) -> Option<(PlayRequest, SourceId, SourceKind)> {
        let src = self.pick_source(e, skip)?;
        self.turn.lap("source");
        let req = self.request_from(e, &src);
        self.turn.lap("gain");
        Some((req, src.id, src.kind))
    }

    /// The best source of `e` that is usable and not in `skip` (local >
    /// Spotify > YouTube, then quality). A stream source needs its service
    /// registered. A stream copy set aside earlier (its details or its
    /// video refused once) is tried again when nothing else plays, unless
    /// it was refused in the last hours (`REFUSED_TTL`). Says in the log
    /// why nothing plays.
    fn pick_source(&mut self, e: Entry, skip: &[SourceId]) -> Option<nikgob_core::model::Source> {
        let mut sources = match self.lib.sources_of_track(e.track) {
            Ok(s) => s,
            Err(err) => {
                log::warn!("{}: its sources could not be read: {err}", e.track);
                return None;
            }
        };
        let all = sources.len();
        sources.retain(|s| s.trashed_at.is_none() && !skip.contains(&s.id));
        sources.sort_by_key(|s| (!s.available, s.kind.priority(), std::cmp::Reverse(s.quality_score)));
        let refused = self.refused_videos();
        // A stream needs its service on and able to play sound for this
        // account (Spotify not connected, or before its playback login:
        // the library's Spotify tracks play from YouTube Music).
        let usable = |w: &mut Self, s: &nikgob_core::model::Source| match s.kind.service() {
            None => s.available,
            Some(service) => {
                (s.available || (s.kind == SourceKind::Youtube && !refused.contains(&s.locator))) && w.has_audio(service)
            }
        };
        let Some(src) = sources.iter().find(|s| usable(self, s)).cloned() else {
            let why = if all == 0 {
                "it has no source".to_owned()
            } else if sources.is_empty() {
                format!("all its {all} sources were tried or trashed")
            } else if sources.iter().all(|s| !s.available && s.kind.service().is_none()) {
                "its files are missing".to_owned()
            } else {
                let services: Vec<&str> = sources.iter().filter_map(|s| s.kind.service()).map(|s| s.as_str()).collect();
                let off = if self.service(StreamKind::Youtube).is_none() { ", YouTube Music switched off" } else { "" };
                format!(
                    "no audio from {} now ({} sources set aside{off})",
                    services.join(", "),
                    sources.iter().filter(|s| !s.available).count()
                )
            };
            log::warn!("{}: nothing to play: {why}", e.track);
            return None;
        };
        if !src.available {
            log::info!("{}: trying its {} copy {} set aside earlier", e.track, src.kind.as_str(), src.locator);
        }
        Some(src)
    }

    /// The request of `e` from `src`: a file's gain read from its tags
    /// (once per file, [`convert::FactsCache`]; off the loop when that is
    /// slow, [`Self::file_gain`]), the length, the speed.
    fn request_from(&mut self, e: Entry, src: &nikgob_core::model::Source) -> PlayRequest {
        let duration = self
            .lib
            .get_track(e.track)
            .ok()
            .flatten()
            .and_then(|t| t.duration_ms)
            .map(|ms| Duration::from_millis(u64::from(ms)));
        let (source, gain_db) = match src.kind.service() {
            None => {
                let path = self.res.path(&src.locator);
                let gain = self.file_gain(e.id, &path);
                (PlaySource::File(path), gain)
            }
            Some(service) => (
                PlaySource::Stream {
                    service,
                    id: src.locator.clone(),
                },
                0.0,
            ),
        };
        let (speed, keep_timbre) = self.fx_of(e.track);
        PlayRequest {
            id: e.id,
            source,
            gain_db,
            duration,
            speed,
            keep_timbre,
        }
    }

    /// The version `track` plays as now, and whether it keeps its timbre.
    fn fx_of(&self, track: TrackId) -> (nikgob_view::SpeedVariant, bool) {
        let st = self.state.read();
        let variant = st.speeds.of(track);
        (variant, st.prefs.keep_timbre && variant != nikgob_view::SpeedVariant::Normal)
    }

    /// `Command::SetTrackSpeed`: remembered with the track, heard at once
    /// if it plays.
    fn set_track_speed(&mut self, track: TrackId, variant: nikgob_view::SpeedVariant) {
        // A speed of the listener's own that a ready version has is that
        // version, and one out of range is the nearest the engine takes.
        let variant = match variant {
            nikgob_view::SpeedVariant::Custom(percent) => nikgob_view::SpeedVariant::at(percent),
            ready => ready,
        };
        // The slider says the same speed again while it is held: nothing
        // to save and nothing to restart.
        if self.state.read().speeds.chosen.get(&track).copied().unwrap_or_default() == variant {
            return;
        }
        let key = (variant != nikgob_view::SpeedVariant::Normal).then(|| variant.key());
        if let Err(e) = self.lib.set_track_fx(track, key.as_deref()) {
            log::warn!("{track}: its speed could not be saved: {e}");
            return;
        }
        let mut chosen = (*self.state.read().speeds.chosen).clone();
        let mut names = (*self.state.read().speed_names).clone();
        if key.is_some() {
            chosen.insert(track, variant);
            if !names.contains_key(&track)
                && let Some(n) = speed_name(&self.lib, track)
            {
                names.insert(track, n);
            }
        } else {
            chosen.remove(&track);
            names.remove(&track);
        }
        self.publish(|s| {
            s.speeds.chosen = Arc::new(chosen);
            s.speed_names = Arc::new(names);
        });
        self.apply_speed(Some(track));
    }

    /// `Command::ResetTrackSpeed`: as recorded again, and «Отменить»
    /// brings its speed back.
    fn reset_track_speed(&mut self, track: TrackId) {
        let Some(was) = self.state.read().speeds.chosen.get(&track).copied() else { return };
        let title = self.state.read().speed_names.get(&track).map(|n| n.title.clone());
        self.set_track_speed(track, nikgob_view::SpeedVariant::Normal);
        let text = match title {
            Some(t) => format!("«{t}» снова как записан"),
            None => "Скорость сброшена".to_owned(),
        };
        self.toast(text, Some(("Отменить".into(), Command::SetTrackSpeed { track, variant: was })));
    }

    /// The speed of `changed` (or of everything: the settings) changed: the
    /// playing track goes on as its version now is, from where it is, and
    /// the preloaded next track, if it is concerned, takes its version in
    /// the engine. It is not handed over anew: near the end of the playing
    /// track the engine is already producing it, and a second copy would
    /// play after the first.
    fn apply_speed(&mut self, changed: Option<TrackId>) {
        if let Some(e) = self.playing {
            let fx = self.fx_of(e.track);
            if fx != self.playing_fx {
                self.playback.set_speed(e.id, fx.0, fx.1);
                self.playing_fx = fx;
                // The position is the track's own time: it stays, and
                // moves at the new pace from here.
                let now = Instant::now();
                self.position = self.position_at_instant(now);
                self.position_at = now;
                self.speed = f64::from(fx.0.speed());
                self.rate = self.speed;
                self.publish_player();
            }
        }
        let playing = self.playing.map(|e| e.id);
        if let Some(n) = self.queue.peek_auto_next()
            && Some(n.id) != playing
            && changed.is_none_or(|t| n.track == t)
        {
            let fx = self.fx_of(n.track);
            self.playback.set_speed(n.id, fx.0, fx.1);
        }
    }

    /// [`Self::request_skipping`] with nothing skipped.
    fn request(&mut self, e: Entry) -> Option<PlayRequest> {
        self.request_skipping(e, &[]).map(|(req, ..)| req)
    }

    /// Hands `req` (for the playing entry `e`) to the engine. A stream reads
    /// Buffering until the engine reports its start.
    fn play_request(&mut self, e: Entry, req: PlayRequest, source: SourceId, kind: SourceKind) {
        self.playing_fx = (req.speed, req.keep_timbre);
        self.speed = f64::from(req.speed.speed());
        self.playback.play(req);
        self.restarted = Some(e.id);
        self.playing = Some(e);
        self.playing_source = Some((source, kind));
        self.status = if kind.is_local() {
            Status::Playing
        } else {
            Status::Buffering
        };
    }

    /// The source playing `e` did not open: tries its next source. Returns
    /// false when there is none left.
    fn try_next_source(&mut self, e: Entry) -> bool {
        if let Some((failed, _)) = self.playing_source.take() {
            self.tried.push(failed);
        }
        let skip = self.tried.clone();
        let Some((req, source, kind)) = self.request_skipping(e, &skip) else {
            // Last chance: the same song on YouTube Music.
            return self.wait_for_youtube(e);
        };
        log::info!("playing {} from its next source ({kind})", e.track);
        self.play_request(e, req, source, kind);
        self.position = Duration::ZERO;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        self.seek_to_failover(e);
        self.preload_next();
        self.publish_player();
        true
    }

    /// The entry `e` just started on another source after its last one
    /// failed mid-track: on from where that one stopped.
    pub(crate) fn seek_to_failover(&mut self, e: Entry) {
        let Some((_, at)) = self.failover.filter(|(id, _)| *id == e.id) else { return };
        log::info!("{}: going on at {} s", e.track, at.as_secs());
        self.playback.seek(at);
        self.position = at;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        self.publish_player();
    }

    /// A new list (an album, a playlist, a mix, a radio, a search result)
    /// is about to replace the queue: shuffle turns on or off as the
    /// setting of its `kind` says («Перемешивание»); a list of no kind (the
    /// library's tracks, a search, a radio) keeps the button as it is.
    pub(crate) fn new_list(&mut self, kind: Option<nikgob_view::ListKind>) {
        let rule = kind.map_or(nikgob_view::ListShuffle::Button, |k| self.state.read().prefs.list_shuffle.of(k));
        self.queue.new_list(rule);
    }

    /// The kind of the library playlist `id` for [`Self::new_list`]: one of
    /// a service (followed, synced or imported from it) is a ready one;
    /// «Любимые» and those made here are the user's own.
    fn playlist_kind(&self, id: PlaylistId) -> nikgob_view::ListKind {
        if id != LIKED_PLAYLIST && self.lib.playlist_remote(id).ok().flatten().is_some() {
            nikgob_view::ListKind::ReadyPlaylist
        } else {
            nikgob_view::ListKind::OwnPlaylist
        }
    }

    /// «Перемешивать радио и автовоспроизведение»: a radio's, autoplay's
    /// and the daylist's picks play shuffled while shuffle is on.
    pub(crate) fn shuffle_radio(&self) -> bool {
        self.state.read().prefs.shuffle_radio
    }

    /// A list of tracks of the library's pages (downloaded, «Слушать
    /// снова», worlds, cards) from `start`, or from the first (a random one
    /// under shuffle) when the user picked none; shuffle stays as the
    /// button is.
    fn play_tracks(&mut self, tracks: &[TrackId], start: Option<usize>, context: String) {
        self.user_switch(EventOrigin::Manual);
        self.new_list(None);
        self.queue.set_from(tracks, start, Some(context));
        self.play_context = EventContext::Library;
        self.play_context_id = None;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn play_list(
        &mut self,
        ids: Vec<TrackId>,
        start: Option<TrackId>,
        shuffle: bool,
        kind: nikgob_view::ListKind,
        context: Option<String>,
        ctx: EventContext,
        context_id: Option<String>,
    ) {
        if ids.is_empty() {
            self.toast("Нечего играть", None);
            return;
        }
        let start_idx = start.and_then(|s| ids.iter().position(|&t| t == s));
        self.user_switch(EventOrigin::Manual);
        self.turn.lap("switch");
        // «Перемешать» turns shuffle on; a plain play shuffles or not as
        // the setting of the list's kind says (shuffled: from a random
        // track unless one was picked).
        self.new_list(Some(kind));
        if shuffle {
            self.queue.shuffle = true;
        }
        self.queue.set_from(&ids, start_idx, context);
        self.turn.lap("queue");
        // The new queue's own context, never the one before it (a mix's,
        // a playlist's) carried over.
        self.play_context = ctx;
        self.play_context_id = context_id;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
    }

    fn start(&mut self, e: Entry) {
        self.resume_at = None;
        self.failover = None;
        let mut e = e;
        // Skip forward over unplayable items rather than stall. With repeat on,
        // `skip_forward` wraps around, so the walk is bounded by the queue
        // length (otherwise a queue without a single playable file recurses
        // until the stack overflows).
        for attempt in 0..=self.queue.len() {
            self.tried.clear();
            // Disliked, where the app picked it: the next one.
            if self.skips_disliked(e) {
                log::info!("skipping the disliked {}", e.track);
                match self.queue.skip_forward() {
                    Some(n) if n.id != e.id => {
                        e = n;
                        continue;
                    }
                    _ => break,
                }
            }
            if let Some((req, source, kind)) = self.request_skipping(e, &[]) {
                self.start_with(e, req, source, kind);
                return;
            }
            // Nothing plays now, but YouTube Music may have it: wait for it.
            if self.wait_for_youtube(e) {
                return;
            }
            log::warn!("no playable source for {}", e.track);
            if attempt == 0 {
                // A stream track's last way to the sound is YouTube Music:
                // when it is off, that is the reason, not a missing file.
                let streamed = self
                    .lib
                    .sources_of_track(e.track)
                    .unwrap_or_default()
                    .iter()
                    .any(|s| s.trashed_at.is_none() && s.kind.service().is_some());
                let text = if streamed && self.service(StreamKind::Youtube).is_none() {
                    self.off_text(StreamKind::Youtube)
                } else {
                    "Нет файла для этого трека".to_owned()
                };
                self.toast(text, None);
            }
            match self.queue.skip_forward() {
                Some(n) if n.id != e.id => e = n,
                _ => break,
            }
        }
        self.stop();
    }

    /// Plays `e` from `req` and records the start (with the service that
    /// actually plays, for a stream).
    fn start_with(&mut self, e: Entry, req: PlayRequest, source: SourceId, kind: SourceKind) {
        self.play_request(e, req, source, kind);
        self.turn.lap("play");
        self.position = Duration::ZERO;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        self.record_start(e, (!kind.is_local()).then_some(kind));
        self.preload_next();
        self.turn.lap("next");
        // From YouTube Music while Spotify plays: its Spotify copy is
        // looked up for the next time (before the lookups ahead: one at a time).
        self.spotify_copy_for(e, kind);
        // What only YouTube Music may have is looked up ahead, gapless or not.
        self.resolve_ahead();
        self.turn.lap("ahead");
        self.publish_now_playing();
        self.turn.lap("now playing");
        self.likes_if_stale();
        self.turn.lap("likes");
    }

    fn preload_next(&mut self) {
        self.prepare_next(true);
    }

    /// [`Self::preload_next`]; `ahead`: a file whose tags were not read yet
    /// is read on another thread first.
    fn prepare_next(&mut self, ahead: bool) {
        // Gapless off: nothing is prepared, the next track starts after `Ended`.
        let next = if self.state.read().prefs.gapless { self.queue.peek_auto_next() } else { None };
        let Some((next, src)) = next.and_then(|n| Some((n, self.pick_source(n, &[])?))) else {
            self.reading_ahead = None;
            self.playback.set_next(None);
            return;
        };
        // A file whose tags were not read yet is read off the loop; the
        // engine gets the next track when they are.
        if ahead && src.kind.service().is_none() {
            let path = self.res.path(&src.locator);
            if self.res.facts().get(&path).is_none() && self.read_facts_ahead(path) {
                self.playback.set_next(None);
                return;
            }
        }
        // Prepared here: a read still on its way prepares nothing again.
        self.reading_ahead = None;
        let req = self.request_from(next, &src);
        self.playback.set_next(Some(req));
    }

    /// Reads the tags of `path` (the next track's file) on a thread of its
    /// own, then prepares the next track (whatever it is by then, never
    /// reading ahead again). False when no thread could start (it is read
    /// here then).
    fn read_facts_ahead(&mut self, path: PathBuf) -> bool {
        if self.reading_ahead.as_ref() == Some(&path) {
            return true;
        }
        if !self.read_facts_off_loop(&path) {
            return false;
        }
        self.reading_ahead = Some(path);
        true
    }

    fn pause(&mut self) {
        if matches!(self.status, Status::Playing | Status::Buffering) {
            self.heard_tick();
            self.position = self.position_now();
            self.status = Status::Paused;
            self.playback.pause();
            self.publish_player();
        }
    }

    fn resume(&mut self) {
        match self.status {
            Status::Paused if self.playing.is_none() => self.start_restored(),
            Status::Paused => {
                self.heard_tick();
                self.position_at = Instant::now();
                self.rate = self.speed;
                self.status = Status::Playing;
                self.playback.resume();
                self.publish_player();
            }
            Status::Stopped => {
                if let Some(e) = self.queue.current() {
                    self.start(e);
                }
            }
            _ => {}
        }
    }

    fn stop(&mut self) {
        self.playback.stop();
        self.playing = None;
        self.playing_source = None;
        self.failover = None;
        self.status = Status::Stopped;
        self.position = Duration::ZERO;
        self.publish_now_playing();
    }

    fn seek(&mut self, p: Duration) {
        if self.playing.is_none() {
            // The restored track: play will start from here.
            if self.resume_at.is_some() {
                self.resume_at = Some(p);
                self.position = p;
                self.publish_player();
            }
            return;
        }
        self.playback.seek(p);
        self.position = p;
        self.position_at = Instant::now();
        self.rate = self.speed;
        self.jumped_at = Instant::now();
        if let Some(e) = self.playing {
            let _ = self.lib.record_event(
                e.track,
                EventType::Seek,
                None,
                Some(p.as_millis() as u32),
                self.play_context,
            );
        }
        self.publish_player();
    }

    /// Who chose `e`, starting now: what the command said, else the user
    /// when the queue is another one, else the radio or autoplay that
    /// added it, else the queue going on.
    fn origin_of(&mut self, e: Entry) -> EventOrigin {
        let generation = self.queue.generation();
        let new_queue = self.origin_gen != Some(generation);
        self.origin_gen = Some(generation);
        if new_queue {
            self.autoplay_entries.clear();
        }
        if let Some(o) = self.next_origin.take() {
            return o;
        }
        if let Some(o) = self.radio_origin(e.id) {
            return o;
        }
        if self.autoplay_entries.contains(&e.id) {
            return EventOrigin::Autoplay;
        }
        if new_queue { EventOrigin::Manual } else { EventOrigin::Queue }
    }

    /// Records the start of `e` with who chose it and what plays it.
    fn record_start(&mut self, e: Entry, kind: Option<SourceKind>) {
        let origin = self.origin_of(e);
        // What the radio or autoplay added plays in it, even in the queue
        // of an album it went on after.
        let (context, context_id) = match self.radio_context(e.id) {
            Some((ctx, id)) => (ctx, Some(id)),
            None => (self.play_context, self.play_context_id.clone()),
        };
        self.ended = false;
        self.heard.reset();
        let meta = EventMeta { origin: Some(origin), context_id, ..EventMeta::default() };
        let _ = self.lib.record_event_with(e.track, EventType::Start, kind, Some(0), context, meta);
        self.turn.lap("start recorded");
        self.recs_started(e.track, origin);
        self.turn.lap("recs session");
        self.home_input_changed();
    }

    /// The playing track's [`Heard`] counts the time since its last tick,
    /// in the state the player and the window are in.
    pub(crate) fn heard_tick(&mut self) {
        let playing = self.status == Status::Playing;
        let away = self.window_away();
        let volume = if self.muted { 0.0 } else { self.volume };
        self.heard.tick(playing, away, volume);
    }

    /// The user picks something else to play (`origin` of what comes): the
    /// playing track ends here.
    pub(crate) fn user_switch(&mut self, origin: EventOrigin) {
        if !self.ended {
            self.record_end(true, EndReason::UserPick);
        }
        self.next_origin = Some(origin);
    }

    /// The app closes: the playing (or paused) track ends as it is.
    fn record_closed(&mut self) {
        if !self.ended && self.playing.is_some() {
            self.record_end(true, EndReason::AppClosed);
        }
    }

    /// Records how the current track ended (`reason`): a skip before 90 %,
    /// else complete; once per start.
    fn record_end(&mut self, by_user: bool, reason: EndReason) {
        let Some(e) = self.playing else { return };
        if self.ended {
            return;
        }
        self.ended = true;
        if let Some((source, SourceKind::Youtube)) = self.playing_source {
            self.analyse_played(e.track, source);
        }
        let pos = self.position_now();
        let dur = self
            .lib
            .get_track(e.track)
            .ok()
            .flatten()
            .and_then(|t| t.duration_ms);
        let complete = !by_user
            || dur.is_some_and(|d| pos.as_millis() as f64 >= f64::from(d) * 0.9);
        let position_ms = pos.as_millis() as u32;
        if reason != EndReason::AppClosed {
            self.radio_feedback(e.track, complete, position_ms);
            self.recs_played();
            self.recs_ended(e.track, complete, position_ms);
        }
        self.heard_tick();
        let (attention, volume) = self.heard.take();
        let meta = EventMeta { end_reason: Some(reason), attention, volume, ..EventMeta::default() };
        let _ = self.lib.record_event_with(
            e.track,
            if complete {
                EventType::Complete
            } else {
                EventType::Skip
            },
            None,
            Some(position_ms),
            self.play_context,
            meta,
        );
    }

    fn playback_event(&mut self, ev: PlaybackEvent) {
        // How long a position report waited to be taken (a busy worker can
        // hold it for a while): the sound went on meanwhile.
        let (ev, late) = match ev {
            PlaybackEvent::PositionAt { id, position, at } => (PlaybackEvent::Position { id, position }, at.elapsed().min(POSITION_SNAP)),
            ev => (ev, Duration::ZERO),
        };
        match ev {
            PlaybackEvent::Started { id } => {
                if self.restarted == Some(id) {
                    self.restarted = None;
                }
                if self.playing.map(|e| e.id) == Some(id) {
                    if self.status == Status::Buffering {
                        self.heard_tick();
                        self.status = Status::Playing;
                    }
                    // A failover starts where the failed source stopped.
                    self.position = match self.failover.take() {
                        Some((failed, at)) if failed == id => at,
                        _ => Duration::ZERO,
                    };
                    self.position_at = Instant::now();
                    self.rate = self.speed;
                    self.publish_player();
                }
            }
            PlaybackEvent::TrackChanged { to } => {
                // `play()` over a playing item also reports a change: that one
                // we started ourselves. The same entry changing to itself
                // otherwise is repeat-one looping gaplessly: handled below
                // like any auto-advance, so the position starts over.
                if self.restarted.take() == Some(to) {
                    if self.status == Status::Buffering {
                        self.heard_tick();
                        self.status = Status::Playing;
                        self.publish_player();
                    }
                    return;
                }
                if self.queue.peek_auto_next().map(|e| e.id) != Some(to) {
                    log::debug!("backend changed to {to}, not the queued next");
                    return;
                }
                self.record_end(false, EndReason::Ended);
                if let Some(e) = self.queue.advance_auto() {
                    self.playing = Some(e);
                    self.tried.clear();
                    // The engine took over what `preload_next` asked for.
                    let taken = self.request_skipping(e, &[]);
                    self.playing_fx = taken
                        .as_ref()
                        .map_or((nikgob_view::SpeedVariant::Normal, false), |(req, ..)| (req.speed, req.keep_timbre));
                    self.speed = f64::from(self.playing_fx.0.speed());
                    self.playing_source = taken.map(|(_, source, kind)| (source, kind));
                    self.position = Duration::ZERO;
                    self.position_at = Instant::now();
                    self.rate = self.speed;
                    let kind = self.playing_source.map(|(_, k)| k).filter(|k| !k.is_local());
                    self.record_start(e, kind);
                    self.preload_next();
                    self.publish_now_playing();
                }
            }
            PlaybackEvent::Position { id, position } => {
                // The engine plays it: a start report that never came (or
                // was taken for another item) must not leave the player in
                // Buffering, where the view freezes the position and the
                // lyrics stop following the words.
                if self.playing.map(|e| e.id) == Some(id)
                    && self.status == Status::Buffering
                    && position > Duration::ZERO
                {
                    log::info!("item {id} plays without a start report; playing now");
                    self.restarted = None;
                    self.heard_tick();
                    self.status = Status::Playing;
                    self.position = position;
                    self.position_at = Instant::now();
                    self.rate = self.speed;
                    self.publish_player();
                    return;
                }
                if self.playing.map(|e| e.id) == Some(id) {
                    let stalled = self.reported == Some((id, position));
                    self.reported = Some((id, position));
                    if self.status != Status::Playing {
                        self.position = position;
                        self.position_at = Instant::now();
                        self.rate = self.speed;
                        return;
                    }
                    // The view extrapolates from its last publish and stops at
                    // the track's end, so it follows what is heard: a small
                    // difference is made up by running a little faster or
                    // slower (never backwards); a far one (a repeat-one loop,
                    // a seek we did not hear about, a long stall) is jumped.
                    let now = Instant::now();
                    let ours = self.position_at_instant(now);
                    // Where the sound is now (a stalled one stands still).
                    let heard = if stalled { position } else { position + late.mul_f64(self.speed) };
                    match follow_at(ours, heard, stalled, self.rate, self.speed) {
                        Follow::Keep => {}
                        Follow::Snap => {
                            if self.jumped_at.elapsed() < POSITION_SETTLE {
                                return;
                            }
                            self.position = heard;
                            self.position_at = now;
                            self.rate = if stalled { 0.0 } else { self.speed };
                            self.publish_player();
                        }
                        Follow::Rate(rate) => {
                            self.position = ours;
                            self.position_at = now;
                            self.rate = rate;
                            self.publish_clock();
                        }
                    }
                }
            }
            // Taken as a position above.
            PlaybackEvent::Paused | PlaybackEvent::Resumed | PlaybackEvent::PositionAt { .. } => {}
            PlaybackEvent::Speed { id, speed } => {
                let speed = f64::from(speed);
                if self.playing.map(|e| e.id) == Some(id) && speed.is_finite() && speed > 0.0 && speed != self.speed {
                    let now = Instant::now();
                    self.position = self.position_at_instant(now);
                    self.position_at = now;
                    self.speed = speed;
                    self.rate = speed;
                    self.publish_player();
                }
            }
            PlaybackEvent::Duration { id, duration } => {
                // The library's length wins; the engine's is for a track
                // without one (a stream whose service did not say).
                let Some(e) = self.playing.filter(|e| e.id == id) else { return };
                let ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
                if ms > 0 && self.lib.fill_duration(e.track, ms).unwrap_or(false) {
                    log::debug!(target: "nikgob_runtime::streams", "{}: its length {ms} ms is the heard one", e.track);
                    self.publish_now_playing();
                }
            }
            PlaybackEvent::Quality { id, label } => {
                log::debug!(target: "nikgob_runtime::streams", "item {id} plays as {label}");
                self.qualities.retain(|(i, _)| *i != id);
                if self.qualities.len() >= 4 {
                    self.qualities.remove(0);
                }
                self.qualities.push((id, label));
                if self.playing.map(|e| e.id) == Some(id) {
                    // A copy set aside earlier plays after all: it counts again.
                    if let Some((source, kind)) = self.playing_source.filter(|(_, k)| !k.is_local())
                        && self.lib.get_source(source).ok().flatten().is_some_and(|s| !s.available)
                    {
                        log::info!("{kind} copy {source} plays again");
                        if let Err(e) = self.lib.set_source_available(source, true) {
                            log::warn!("{source}: {e}");
                        }
                    }
                    self.publish_now_playing();
                }
            }
            PlaybackEvent::Ended { id } => {
                if self.playing.map(|e| e.id) == Some(id) {
                    self.record_end(false, EndReason::Ended);
                    // Repeat-one with no preload (or preload failed): restart.
                    match self.queue.advance_auto() {
                        Some(e) => self.start(e),
                        None => {
                            self.stop();
                            // A radio still picking its next tracks plays on.
                            self.radio_ran_out();
                        }
                    }
                }
            }
            PlaybackEvent::Error { id, message } => {
                log::error!("playback error on {id}: {message}");
                let playing = self.playing.filter(|e| e.id == id);
                if playing.is_none() && self.queue.peek_auto_next().is_some_and(|n| n.id == id) {
                    // The preloaded next track: it is tried again (with
                    // all its sources) when its turn comes.
                    return;
                }
                if let Some(e) = playing
                    && let Some((source, kind)) = self.playing_source
                    && kind == SourceKind::Soundcloud
                    && message.contains(OFFICIAL_PLAYER_ROUTE)
                    && self.official_player_tried != Some(e.id)
                {
                    // Only DRM streams: the backend plays the same source
                    // through SoundCloud's own player now.
                    self.official_player_tried = Some(e.id);
                    let skip = self.tried.clone();
                    if let Some((req, again, kind)) = self.request_skipping(e, &skip).filter(|(_, s, _)| *s == source) {
                        log::info!("{}: SoundCloud has it only with DRM, playing it through SoundCloud's player", e.track);
                        self.play_request(e, req, again, kind);
                        self.seek_to_failover(e);
                        return;
                    }
                }
                if let Some(e) = playing
                    && let Some((source, kind)) = self.playing_source
                    && kind == SourceKind::Youtube
                    && youtube_refused_midway(&message)
                    && !self.position.is_zero()
                    && self.stream_retried != Some(e.id)
                {
                    // YouTube refused the stream's URLs after it played a
                    // while (the ~15 s 403): a new stream asks the player
                    // afresh and goes on from here, once.
                    self.stream_retried = Some(e.id);
                    self.failover = Some((e.id, self.position));
                    let skip = self.tried.clone();
                    if let Some((req, again, kind)) = self.request_skipping(e, &skip).filter(|(_, s, _)| *s == source) {
                        log::info!("{}: YouTube refused the stream midway, opening it again", e.track);
                        self.play_request(e, req, again, kind);
                        self.seek_to_failover(e);
                        return;
                    }
                }
                if let Some(e) = playing
                    && let Some((source, kind)) = self.playing_source
                    && !self.stream_failed(e.track, source, &message)
                    && kind == SourceKind::Youtube
                {
                    // YouTube's failure, not the video's: its other copies
                    // would hang the same way.
                    let copies = self.lib.sources_of_track(e.track).unwrap_or_default();
                    self.tried
                        .extend(copies.iter().filter(|s| s.kind == SourceKind::Youtube).map(|s| s.id));
                }
                // Where the engine had got to (what it last reported, not
                // the extrapolation: a stream that runs dry stands still).
                if let Some(e) = playing
                    && !self.position.is_zero()
                {
                    self.failover = Some((e.id, self.position));
                }
                if let Some(e) = playing
                    && self.try_next_source(e)
                {
                    return;
                }
                let title = playing
                    .and_then(|e| self.lib.get_track(e.track).ok().flatten())
                    .map(|t| t.title);
                let why = playing.and_then(|e| self.streaming.failures.remove(&e.track));
                match (title, why) {
                    (Some(t), Some((why, Some(link)))) => {
                        self.toast_open(format!("{t}: {why}"), "Войти", nikgob_view::Link::Setting(link));
                    }
                    (Some(t), Some((why, None))) => self.toast(format!("{t}: {why}"), None),
                    _ => self.toast("Не получилось воспроизвести трек", None),
                }
                if playing.is_some() {
                    match self.queue.skip_forward() {
                        Some(e) if e.id != id => self.start(e),
                        _ => self.stop(),
                    }
                }
            }
        }
    }

    /// What the playing stream is: what it said once opened, else what its
    /// service usually streams. `None` for a local file.
    fn audio_quality(&self) -> Option<String> {
        let service = self.playing_source.and_then(|(_, k)| k.service())?;
        let id = self.playing?.id;
        let known = self.qualities.iter().find(|(i, _)| *i == id).map(|(_, q)| q.clone());
        Some(known.unwrap_or_else(|| service.nominal_quality()))
    }

    fn publish_player(&mut self) {
        // A radio picks more when its queue runs low.
        self.radio_top_up();
        let radio = self.radio_view();
        let status = self.status;
        let now = Instant::now();
        let (position, rate) = (self.position_at_instant(now), self.rate as f32);
        let speed = self.speed as f32;
        let (volume, muted) = (self.volume, self.muted);
        let (shuffle, repeat) = (self.queue.shuffle, self.queue.repeat);
        let context = self.queue.context.clone();
        let page = self.context_page_id().and_then(context_page);
        let upcoming: Vec<Entry> = self.queue.upcoming().iter().take(200).copied().collect();
        let history = self.queue.history();
        let history: Vec<Entry> = history[history.len().saturating_sub(HISTORY_SHOWN)..].to_vec();
        let current_entry = self.queue.current().map(|e| e.id);
        let audio_service = self.playing_source.and_then(|(_, k)| k.service());
        let audio_quality = self.audio_quality();
        let soloist = audio_service == Some(StreamKind::Spotify) && self.soloist_ready();
        // The rows of both lists in one query, before the lock.
        let ids: Vec<TrackId> = upcoming.iter().chain(&history).map(|e| e.track).collect();
        let found = self.lib.get_track_rows(&ids).unwrap_or_default();
        let why = &self.recs.why;
        let current_why = current_entry.and_then(|e| why.get(&e).cloned());
        let res = &mut self.res;
        let mut rows = |list: &[Entry]| -> Vec<nikgob_view::QueueEntry> {
            list.iter()
                .filter_map(|e| {
                    found.get(&e.track).map(|r| nikgob_view::QueueEntry {
                        entry: e.id,
                        track: res.track(r),
                        why: why.get(&e.id).cloned(),
                    })
                })
                .collect()
        };
        let next = rows(&upcoming);
        let prev = rows(&history);
        self.publish(|s| {
            let p = &mut s.player;
            p.status = status;
            p.audio_service = audio_service;
            p.audio_quality = audio_quality;
            p.soloist = soloist;
            p.position = position;
            p.at = now;
            p.rate = rate;
            p.speed = speed;
            p.volume = volume;
            p.muted = muted;
            p.shuffle = shuffle;
            p.repeat = repeat;
            p.context = context;
            p.context_page = page;
            s.queue.next = next;
            s.queue.prev = prev;
            s.queue.current_entry = current_entry;
            s.queue.current_why = current_why;
            s.queue.radio = radio;
        });
    }

    /// The id of what the queue was started from (see
    /// [`Self::play_context_id`]); after a restart, the one the last run
    /// left, while its queue plays.
    pub(crate) fn context_page_id(&self) -> Option<&str> {
        let restored = self.restored_page.as_ref().filter(|(g, _)| *g == self.queue.generation());
        self.play_context_id.as_deref().or(restored.map(|(_, id)| id.as_str()))
    }

    /// Volume and mute only: nothing else of the player or the queue changes.
    fn publish_volume(&mut self) {
        let (volume, muted) = (self.volume, self.muted);
        self.publish(|s| {
            s.player.volume = volume;
            s.player.muted = muted;
        });
    }

    fn publish_now_playing(&mut self) {
        let shown = self.playing.or_else(|| self.resume_at.and(self.queue.current()));
        let current = shown
            .and_then(|e| self.lib.get_track_row(e.track).ok().flatten());
        // The source that plays (a Spotify track may sound from YouTube),
        // else the track's best one.
        let playing = self
            .playing_source
            .filter(|_| self.playing.is_some() && self.playing == shown)
            .and_then(|(id, _)| self.lib.get_source(id).ok().flatten());
        let src = playing.or_else(|| shown.and_then(|e| self.lib.active_source(e.track).ok().flatten()));
        let audio_quality = self.audio_quality();
        let changed = self.state.read().player.current.as_ref().map(|r| r.id)
            != current.as_ref().map(|r| r.id);
        let row = current.as_ref().map(|r| self.res.track(r));
        let mut info = self.res.info(current.as_ref(), src.as_ref());
        if audio_quality.is_some() {
            // «Качество» of a stream: what it gives.
            if let (Some(d), Some(q)) = (info.sound.as_mut(), audio_quality.as_ref()) {
                d.facts.insert(0, ("Поток".into(), q.clone()));
            }
            info.quality = audio_quality;
        }
        self.publish(|s| {
            s.player.duration = row.as_ref().and_then(|r| r.duration);
            s.player.current = row.clone();
            s.queue.current = row;
            s.info = info;
        });
        if changed && let Some(t) = current.as_ref().map(|r| r.id) {
            self.track_details(t);
            self.kick_better_covers(Some(t));
            self.complete_playing(t);
        }
        self.publish_player();
        // The info window of another track follows the library too.
        self.refresh_track_info();
    }

    // ---------------------------------------------------------------- loads

    /// The library's playlists and the albums liked as a whole among them
    /// ([`nikgob_view::LibraryView::liked_albums`]), both latest first,
    /// «Любимые» staying first; under «только скачанное» the albums with a
    /// file.
    fn shelf(&mut self) -> (Vec<nikgob_view::PlaylistCard>, Vec<(usize, nikgob_view::ReleaseCard)>) {
        let only_local = self.state.read().prefs.library_only_downloaded;
        let mut errors = Vec::new();
        let shelf = library_load::shelf(&self.lib, &mut self.res, only_local, &mut errors);
        for what in errors {
            self.toast(format!("Ошибка: {what}"), None);
        }
        shelf
    }

    /// The library filter on «Треки»: what the library search finds,
    /// without the stream when «только скачанное» is on.
    fn filter_tracks(&mut self, q: &str) {
        let q = q.trim();
        if q.is_empty() {
            self.publish(|s| s.library.track_hits = None);
            return;
        }
        let only_local = self.state.read().prefs.library_only_downloaded;
        let rows: Vec<_> = self
            .log_err("фильтр треков", self.lib.search(q, 500))
            .map(|r| r.tracks)
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.has_local || !only_local)
            .collect();
        let q = q.to_owned();
        let hits = rows.iter().map(|t| self.res.track(t)).collect();
        self.publish(|s| {
            s.library.track_hits = Some((q, hits));
        });
    }

    fn load_tracks(&mut self, offset: usize, limit: usize, sort: nikgob_view::TrackSort) {
        let key = match sort {
            nikgob_view::TrackSort::Artist => Sort::asc(TrackSortKey::Artist),
            nikgob_view::TrackSort::Title => Sort::asc(TrackSortKey::Title),
            nikgob_view::TrackSort::Album => Sort::asc(TrackSortKey::Album),
            nikgob_view::TrackSort::Added => Sort::desc(TrackSortKey::Added),
            nikgob_view::TrackSort::Duration => Sort::asc(TrackSortKey::Duration),
        };
        // A library load on its way leaves these rows be.
        self.library_load.tracks_asked += 1;
        let filter = track_filter(self.state.read().prefs.library_only_downloaded);
        let rows = self
            .log_err("треки", self.lib.tracks_filtered(offset as u32, limit as u32, key, filter))
            .unwrap_or_default();
        let rows = rows.iter().map(|r| self.res.track(r)).collect();
        self.publish(|s| {
            s.library.tracks_offset = offset;
            s.library.tracks = rows;
            s.library.track_sort = sort;
        });
    }

    pub(crate) fn load_release(&mut self, id: nikgob_core::model::ReleaseId) {
        let Some(release) = self.lib.get_release(id).ok().flatten() else {
            return;
        };
        // Its songs outside the library too: the ones a service knows it
        // has and nothing here plays from a file (▶ stream, ↓ download).
        // A line per place the album counts: a hole where none has a row.
        let Some(listing) = self.lib.release_listing(id).ok().flatten() else {
            return;
        };
        let liked = self.lib.release_liked(id).unwrap_or(false);
        let mut detail = self.res.release_detail(&release, &listing, liked);
        (detail.released, detail.label) = self.release_facts(&release, &detail.tracks);
        self.publish(|s| {
            s.release = Some(detail);
        });
        self.complete_tracklist(id);
    }

    /// The day an album came out (beyond its year) and its label: from the
    /// tags of its first file that reads, the day else from a service.
    fn release_facts(
        &self,
        release: &nikgob_core::model::Release,
        tracks: &[nikgob_view::TrackRow],
    ) -> (Option<nikgob_view::ReleaseDay>, Option<String>) {
        let tags = tracks.iter().filter(|t| t.has_local).find_map(|t| {
            let src = self.lib.active_source(t.id).ok().flatten().filter(|s| s.kind.is_local())?;
            nikgob_import::tags::read_tags(&self.res.path(&src.locator)).ok().map(|(tags, _)| tags)
        });
        let label = tags
            .as_ref()
            .and_then(|t| t.label.as_deref())
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned);
        let day = |d: &str| nikgob_view::ReleaseDay::parse(d, release.year);
        let released = tags
            .as_ref()
            .and_then(|t| t.date.as_deref())
            .and_then(day)
            .or_else(|| self.kept_released(release.id).as_deref().and_then(day));
        (released, label)
    }

    fn load_artist(&mut self, id: nikgob_core::model::ArtistId) {
        let Some(artist) = self.lib.get_artist(id).ok().flatten() else {
            return;
        };
        let releases = self.lib.releases_of_artist(id).unwrap_or_default();
        let mut tracks = self.lib.tracks_of_artist(id, 0, 500).unwrap_or_default();
        // "Популярное у тебя": by plays, top 5.
        let mut plays: Vec<(u32, usize)> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (self.lib.track_stats(t.id).map(|s| s.plays).unwrap_or(0), i))
            .collect();
        plays.sort_by_key(|p| std::cmp::Reverse(p.0));
        let top: Vec<_> = plays.iter().take(5).map(|&(_, i)| tracks[i].clone()).collect();
        tracks.clear();
        let liked = self.lib.is_liked(id).unwrap_or(false);
        let detail = self.res.artist_detail(&artist, &top, &releases, liked);
        self.publish(|s| {
            s.artist = Some(detail);
        });
        self.kick_photos(Some(id));
        // "Нет в библиотеке": from the cache, or looked up once a day.
        self.load_discography(id, false);
    }

    /// What a playlist plays from `start` (see [`Self::playable`]).
    fn playlist_tracks(&mut self, id: PlaylistId, start: Option<TrackId>) -> (Vec<TrackId>, String) {
        if id == LIKED_PLAYLIST {
            if !self.only_downloaded() {
                let ids = self.lib.liked_track_ids().unwrap_or_default();
                return (ids, "Любимые".into());
            }
            let rows = self.lib.liked_tracks(0, u32::MAX).unwrap_or_default();
            return (self.playable(&rows, start), "Любимые".into());
        }
        let entries = self.lib.playlist_entries(id, 0, u32::MAX).unwrap_or_default();
        let name = self
            .lib
            .get_playlist(id)
            .ok()
            .flatten()
            .map_or_else(String::new, |p| p.name);
        let rows: Vec<nikgob_library::TrackRow> = entries.into_iter().map(|e| e.track).collect();
        (self.playable(&rows, start), name)
    }

    fn load_playlist(&mut self, id: PlaylistId) {
        let rows: Vec<nikgob_library::TrackRow> = if id == LIKED_PLAYLIST {
            self.lib.liked_tracks(0, u32::MAX).unwrap_or_default()
        } else {
            self.lib
                .playlist_entries(id, 0, u32::MAX)
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.track)
                .collect()
        };
        let summary = self
            .lib
            .list_playlists()
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.id.unwrap_or(LIKED_PLAYLIST) == id);
        let Some(summary) = summary else { return };
        let sync = self.playlist_sync_view(id);
        let mut detail = self.res.playlist_detail(&summary, &rows);
        detail.sync = sync;
        self.publish(|s| {
            s.playlist = Some(detail);
        });
    }

    /// What the home and a world's page are built for now.
    fn visit(&self) -> home_v2::Visit {
        let playing = self.queue.current().map(|e| e.track);
        let mut visit = home_v2::Visit::now(playing, &self.state.read().prefs);
        visit.sound = Some(self.analysis.index().clone());
        visit.profiles = self.snapshot().map(|s| s.profiles.clone());
        visit.epoch = self.home_epoch();
        visit
    }

    /// `Command::SetPrefs`: every preference at once, and what each change
    /// sets going.
    fn set_prefs(&mut self, mut p: Box<nikgob_view::Prefs>) {
        let old = self.state.read().prefs.clone();
        if p.masters_dir.as_os_str().is_empty() && !old.masters_dir.as_os_str().is_empty() {
            p.masters_dir = old.masters_dir.clone();
        }
        self.playback.configure(crossfade(&p), p.normalization);
        if (old.eq_on, old.eq_gains) != (p.eq_on, p.eq_gains) {
            self.playback.set_equalizer(p.eq_on, p.eq_gains);
        }
        let speed_changed = (old.speed_enabled, old.keep_timbre) != (p.speed_enabled, p.keep_timbre);
        let speed_enabled = p.speed_enabled;
        self.torrent_prefs(&p);
        self.publish(|s| {
            s.prefs = *p.clone();
            s.speeds.enabled = speed_enabled;
        });
        if speed_changed {
            self.apply_speed(None);
        }
        self.save_prefs();
        self.import_prefs(&p);
        if (old.check_fake_lossless, old.convert_fake_lossless, old.fake_flac_auto)
            != (p.check_fake_lossless, p.convert_fake_lossless, p.fake_flac_auto)
        {
            self.quality_prefs();
        }
        self.align_settings();
        if (old.lyrics_words_playing, old.lyrics_words_all, old.lyrics_lines_auto, old.lyrics_check, old.lyrics_recognize, old.lyrics_streams, old.lyrics_genius)
            != (p.lyrics_words_playing, p.lyrics_words_all, p.lyrics_lines_auto, p.lyrics_check, p.lyrics_recognize, p.lyrics_streams, p.lyrics_genius)
        {
            self.lyrics_prefs_changed();
        }
        let lyrics_track = self.state.read().lyrics.track;
        if !old.save_found_lyrics && p.save_found_lyrics
            && let Some(track) = lyrics_track
        {
            self.embed_lyrics(track);
        }
        if old.gapless != p.gapless {
            self.preload_next();
        }
        if old.output_device != p.output_device {
            self.switch_device(p.output_device.clone());
        }
        if (old.spotify_enabled, old.youtube_enabled, old.deezer_enabled, old.soundcloud_enabled)
            != (p.spotify_enabled, p.youtube_enabled, p.deezer_enabled, p.soundcloud_enabled)
        {
            self.soloist_wanted();
            self.publish_accounts();
            self.preload_next();
        }
        if old.album_like_tracks != p.album_like_tracks {
            // The album hearts read the likes the new way.
            self.lib.set_album_likes_tracks(p.album_like_tracks);
            self.load_library();
            let release = self.state.read().release.as_ref().map(|r| r.card.id);
            if let Some(r) = release {
                self.load_release(r);
            }
        }
        if old.library_only_downloaded != p.library_only_downloaded {
            self.load_library();
        }
        if old.auto_download_limit != p.auto_download_limit {
            self.wish_tick();
        }
        // A source turned on: what was not found is searched for again once
        // it can search.
        for key in nikgob_view::SourceKey::ALL {
            if !old.source_on(key) && p.source_on(key) {
                self.streaming.wish.enabled.insert(key);
            } else if !p.source_on(key) {
                self.streaming.wish.enabled.remove(&key);
            }
        }
        if old.source_on(nikgob_view::SourceKey::Soulseek) != p.source_on(nikgob_view::SourceKey::Soulseek) {
            self.soulseek_apply();
        }
        if old.source_order() != p.source_order() {
            self.publish_sources();
            self.publish_wishlist();
            self.wish_sources_ready();
            self.wish_tick();
        }
        if old.stream_cache_mb != p.stream_cache_mb {
            self.apply_stream_cache();
        }
        if old.stream_analysis != p.stream_analysis {
            self.stream_analysis_pref(p.stream_analysis);
        }
        if old.mixes_downloaded_only != p.mixes_downloaded_only {
            // The rows it hides go (or come back) at once; the
            // mixes follow when they are picked again.
            self.load_home();
            self.recs_prefs_changed();
        }
        if old.home_card != p.home_card || old.home_hidden_worlds != p.home_hidden_worlds {
            self.load_home();
        }
        if old.mixes_downloaded_only != p.mixes_downloaded_only || old.radio_new_share != p.radio_new_share || old.radio_new_share_by != p.radio_new_share_by {
            self.radio_source_changed();
        }
        if old.dislike_in_recs != p.dislike_in_recs
            || old.background_counts_less != p.background_counts_less
            || old.profiles_split != p.profiles_split
            || old.profile_names != p.profile_names
            || old.mixes_profiles != p.mixes_profiles
            || old.mixes_everything != p.mixes_everything
            || old.mixes_moods != p.mixes_moods
            || old.mixes_on_repeat != p.mixes_on_repeat
            || old.background_cards != p.background_cards
            || old.home_hidden_mixes != p.home_hidden_mixes
            || old.graph_use != p.graph_use
            || old.graph_personal != p.graph_personal
            || old.library_as_taste != p.library_as_taste
            || old.radio_unheard != p.radio_unheard
            || old.radio_new_share != p.radio_new_share
            || old.radio_new_share_by != p.radio_new_share_by
        {
            self.recs_prefs_changed();
        }
        if (!old.graph_services && p.graph_services) || (!old.graph_open_data && p.graph_open_data) {
            self.graph_now();
        }
        if old.autoplay && !p.autoplay {
            self.autoplay_off();
        }
        if !old.autoplay && p.autoplay {
            self.publish_player();
        }
    }

    /// A world's page ([`Command::LoadWorld`]).
    fn load_world(&mut self, key: &str) {
        let visit = self.visit();
        let Some(w) = home_v2::world(&self.lib, &visit, key) else {
            self.publish(|s| s.world = None);
            return;
        };
        let (_, releases) = if visit.local_only { (Vec::new(), Vec::new()) } else { self.discover_rows() };
        let new_album = home_v2::world_release(&w.world, &releases).map(|i| releases[i].clone());
        let mut view = self.res.world_view(&w, new_album);
        view.mixes = self.world_mix_cards(&view.world);
        let stage = view.world.release;
        self.publish(|s| s.world = Some(view));
        self.kick_hd_covers(stage);
    }

    /// Liking an album saves it; when album likes like the tracks too
    /// ([`nikgob_view::Prefs::album_like_tracks`]), it likes all its tracks,
    /// and unliking unlikes them all.
    fn like_release(&mut self, id: nikgob_core::model::ReleaseId, on: bool) {
        let before = self.library_tracks();
        if !self.state.read().prefs.album_like_tracks {
            self.save_release(id, on, before);
            return;
        }
        let Some(tracks) = self.log_err("лайк", self.lib.like_release(id, on)) else { return };
        for &t in &tracks {
            let _ = self.lib.record_event(
                t,
                if on { EventType::Like } else { EventType::Unlike },
                None,
                None,
                EventContext::Album,
            );
            self.push_like(t, on);
        }
        // An album like downloads the whole album (one wishlist item).
        let wished = on && self.wish_for_album(id, &tracks, nikgob_core::model::WishlistOrigin::Like);
        if !on {
            self.wish_drop_album(id);
            for &t in &tracks {
                self.wish_drop_unstarted(t);
            }
        }
        self.publish_wishlist();
        self.wish_tick();
        let n = tracks.len() as u64;
        let what = format!("{n} {}", if n % 10 == 1 && n % 100 != 11 {
            "трек"
        } else if (2..=4).contains(&(n % 10)) && !(12..=14).contains(&(n % 100)) {
            "трека"
        } else {
            "треков"
        });
        let text = match (on, wished) {
            (true, true) if self.can_download() => format!("Альбом в Любимых · {what} · скачаю в фоне"),
            (true, true) => format!("Альбом в Любимых · {what} · скачается, когда подключишь торренты"),
            (true, false) => format!("Альбом в Любимых · {what}"),
            (false, _) => format!("Альбом убран из Любимых · {what}"),
        };
        self.toast(text, Some(("Отменить".into(), Command::Like(LikeTarget::Release(id), !on))));
        self.publish(|s| {
            let fix = |t: &mut nikgob_view::TrackRow| {
                if tracks.contains(&t.id) {
                    t.liked = on;
                }
            };
            if let Some(c) = s.player.current.as_mut() {
                fix(c);
            }
            if let Some(c) = s.queue.current.as_mut() {
                fix(c);
            }
            s.queue.next.iter_mut().for_each(|e| fix(&mut e.track));
            s.library.tracks.iter_mut().for_each(fix);
            s.search.tracks.iter_mut().for_each(fix);
            s.search.releases.iter_mut().filter(|c| c.id == id).for_each(|c| c.liked = on);
        });
        self.patch_home(move |home| release_liked_on_home(home, id, on));
        self.after_like_change(before, vec![id]);
    }

    /// An album saved (or unsaved) alone: its tracks keep their likes; a
    /// saved album still downloads whole, as a like of it always did.
    fn save_release(&mut self, id: nikgob_core::model::ReleaseId, on: bool, before: Held) {
        if self.log_err("лайк", self.lib.like_release(id, on)).is_none() {
            return;
        }
        let wished = if on {
            let rows = self.lib.tracks_of_release_all(id).unwrap_or_default();
            let local: Vec<TrackId> = rows.iter().filter(|r| r.has_local).map(|r| r.id).collect();
            let tracks = if local.is_empty() { rows.iter().map(|r| r.id).collect() } else { local };
            self.wish_for_album(id, &tracks, nikgob_core::model::WishlistOrigin::Like)
        } else {
            self.wish_drop_album(id);
            false
        };
        self.publish_wishlist();
        self.wish_tick();
        let text = match (on, wished) {
            (true, true) if self.can_download() => "В любимых альбомах · скачаю в фоне",
            (true, true) => "В любимых альбомах · скачается, когда подключишь торренты",
            (true, false) => "Добавлено в любимые альбомы",
            (false, _) => "Убрано из любимых альбомов",
        };
        self.toast(text, Some(("Отменить".into(), Command::Like(LikeTarget::Release(id), !on))));
        self.publish(|s| {
            s.search.releases.iter_mut().filter(|c| c.id == id).for_each(|c| c.liked = on);
        });
        self.patch_home(move |home| release_liked_on_home(home, id, on));
        self.after_like_change(before, vec![id]);
    }

    /// A like of a single's song saves (or unsaves) the single as well,
    /// when [`nikgob_view::Prefs::single_like_saves`] asks for it.
    fn single_follows_like(&mut self, track: TrackId, on: bool) -> Option<()> {
        if !self.state.read().prefs.single_like_saves {
            return None;
        }
        let release = self.lib.get_track(track).ok().flatten()?.release_id?;
        let r = self.lib.get_release(release).ok().flatten()?;
        let rows = self.lib.tracks_of_release_all(release).unwrap_or_default();
        let single = r.release_type == nikgob_core::model::ReleaseType::Single || (rows.len() == 1 && r.total_tracks.unwrap_or(1) <= 1);
        if !single || self.lib.is_liked(LikeTarget::Release(release)).unwrap_or(false) == on {
            return None;
        }
        let r = if on { self.lib.like(LikeTarget::Release(release)) } else { self.lib.unlike(LikeTarget::Release(release)) };
        self.log_err("лайк", r)
    }

    /// Track likes changed: album hearts and "Любимые" follow.
    fn after_like_change(&mut self, before: Held, releases: Vec<nikgob_core::model::ReleaseId>) {
        self.refresh_stream_page();
        self.library_edited(before, releases);
        let (release, playlist) = {
            let s = self.state.read();
            (s.release.as_ref().map(|r| r.card.id), s.playlist.as_ref().map(|p| p.card.id))
        };
        if let Some(r) = release {
            self.load_release(r);
        }
        if playlist == Some(LIKED_PLAYLIST) {
            self.load_playlist(LIKED_PLAYLIST);
        }
    }

    fn release_track_ids(&self, id: nikgob_core::model::ReleaseId) -> Vec<TrackId> {
        self.lib
            .tracks_of_release(id)
            .unwrap_or_default()
            .iter()
            .map(|r| r.id)
            .collect()
    }

    fn set_playlist_cover_mode(&mut self, id: PlaylistId, mode: nikgob_view::PlaylistCoverMode) {
        let stored = match mode {
            nikgob_view::PlaylistCoverMode::Smart => None,
            nikgob_view::PlaylistCoverMode::Collage => Some(playlist_cover::COLLAGE_COVER),
            // A picture comes through `SetPlaylistCover`.
            nikgob_view::PlaylistCoverMode::Custom => return,
        };
        let previous = self.lib.get_playlist(id).ok().flatten().and_then(|p| p.cover);
        if self.log_err("обложка", self.lib.set_playlist_cover(id, stored)).is_some() {
            // The custom picture it leaves: ours to delete only when it is a
            // copy in `.musiclib/covers` (never a file of the user's).
            if let Some(old) = previous.filter(|c| playlist_cover::mode_of(Some(c)) == nikgob_view::PlaylistCoverMode::Custom) {
                let old = self.res.path(&old);
                let covers = self.res.path(".musiclib/covers");
                if old.parent() == Some(covers.as_path()) && old.is_file() {
                    let _ = std::fs::remove_file(&old);
                }
            }
            self.library_edited(Held::Same, Vec::new());
            self.load_playlist(id);
        }
    }

    /// How many tracks the library holds now, to tell whether an edit of
    /// likes or playlists changed what is in it ([`Self::library_edited`]).
    fn library_tracks(&self) -> Held {
        Held::Tracks(self.lib.count_tracks(nikgob_library::TrackFilter::All).ok())
    }

    /// The tracks an album plays, in its order: every song its page lists,
    /// the ones that only stream too, under «Только скачанное» as well (a
    /// song without a file is looked up when it comes up, and one not found
    /// is skipped with a notice).
    fn release_tracks(&mut self, id: nikgob_core::model::ReleaseId) -> Vec<TrackId> {
        let rows = self.log_err("альбом", self.lib.tracks_of_release_all(id)).unwrap_or_default();
        rows.iter().map(|r| r.id).collect()
    }

    /// «Только скачанное» is on, of the home or of the library.
    fn only_downloaded(&self) -> bool {
        let p = &self.state.read().prefs;
        p.mixes_downloaded_only || p.library_only_downloaded
    }

    /// What a list of tracks (a playlist, an artist's songs) plays: all of
    /// it; under «Только скачанное» only its songs with a file and `start`,
    /// the song picked to start from (played even when it only streams). A
    /// list without a single file plays whole. An album plays whole always
    /// ([`Self::release_tracks`]).
    fn playable(&self, rows: &[nikgob_library::TrackRow], start: Option<TrackId>) -> Vec<TrackId> {
        let only_local = self.only_downloaded() && rows.iter().any(|r| r.has_local);
        rows.iter().filter(|r| !only_local || r.has_local || Some(r.id) == start).map(|r| r.id).collect()
    }

    /// The releases of `tracks`, once each.

    fn releases_of(&self, tracks: &[TrackId]) -> Vec<nikgob_core::model::ReleaseId> {
        let mut ids: Vec<_> =
            tracks.iter().filter_map(|t| self.lib.get_track(*t).ok().flatten()?.release_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Likes or playlists were edited. When the library holds the same
    /// tracks as `before` (a local song liked, a playlist renamed…), only
    /// what such an edit changes there is read again and published: the
    /// playlist cards, the cards of `releases` and the page of «Треки»
    /// (their hearts). When it
    /// does not (a stream song liked or added to a playlist joins the
    /// library, one unliked leaves it), or the library was never loaded,
    /// all of it is read again.
    fn library_edited(&mut self, before: Held, releases: Vec<nikgob_core::model::ReleaseId>) {
        self.home_input_changed();
        let same = match before {
            Held::Same => true,
            Held::Tracks(Some(n)) => self.library_tracks() == Held::Tracks(Some(n)),
            Held::Tracks(None) => false,
        };
        // A load on its way would put back what is patched here.
        if !same || !self.state.read().library.loaded || self.library_load_pending() {
            self.load_library();
            return;
        }
        self.stream_analysis_changed();
        self.recs_library_changed();
        let (playlists, liked_albums) = self.shelf();
        let rows: Vec<_> =
            releases.iter().filter_map(|id| self.lib.get_release_row(*id).ok().flatten()).collect();
        let res = &mut self.res;
        let cards: Vec<_> = rows.iter().map(|r| res.release(r)).collect();
        let liked = !cards.is_empty();
        self.publish(|s| {
            s.library.playlists = playlists;
            s.library.liked_albums = liked_albums;
            let l = &mut s.library;
            for list in [&mut l.releases, &mut l.recently_added, &mut l.often_played] {
                for c in list.iter_mut() {
                    if let Some(new) = cards.iter().find(|n| n.id == c.id) {
                        *c = new.clone();
                    }
                }
            }
        });
        // Likes: the hearts of «Треки» too.
        if liked {
            let (offset, sort) = {
                let s = self.state.read();
                (s.library.tracks_offset, s.library.track_sort)
            };
            self.load_tracks(offset, 200, sort);
        }
    }

    /// Updates `liked` flags already shown without reloading everything.
    fn refresh_liked(&mut self, target: LikeTarget, on: bool, before: Held) {
        // The same song on other albums: one ♡ for the recording.
        let twins = match target {
            LikeTarget::Track(t) => self.lib.recording_twins(t).unwrap_or_default(),
            _ => Vec::new(),
        };
        self.publish(|s| {
            let fix = |t: &mut nikgob_view::TrackRow| {
                if LikeTarget::Track(t.id) == target || twins.contains(&t.id) {
                    t.liked = on;
                }
            };
            if let Some(c) = s.player.current.as_mut() {
                fix(c);
            }
            if let Some(c) = s.queue.current.as_mut() {
                fix(c);
            }
            s.queue.next.iter_mut().for_each(|e| fix(&mut e.track));
            s.queue.prev.iter_mut().for_each(|e| fix(&mut e.track));
            s.library.tracks.iter_mut().for_each(fix);
            s.search.tracks.iter_mut().for_each(fix);
            if let Some(r) = s.release.as_mut() {
                r.tracks.iter_mut().for_each(fix);
                if LikeTarget::Release(r.card.id) == target {
                    r.liked = on;
                }
            }
            if let Some(p) = s.playlist.as_mut() {
                p.tracks.iter_mut().for_each(fix);
            }
            if let Some(a) = s.artist.as_mut() {
                a.top_tracks.iter_mut().for_each(fix);
                if LikeTarget::Artist(a.card.id) == target {
                    a.liked = on;
                    a.card.liked = on;
                }
            }
            let fix_release = |r: &mut nikgob_view::ReleaseCard| {
                if LikeTarget::Release(r.id) == target {
                    r.liked = on;
                }
            };
            let fix_artist = |a: &mut nikgob_view::ArtistCard| {
                if LikeTarget::Artist(a.id) == target {
                    a.liked = on;
                }
            };
            s.library.releases.iter_mut().for_each(fix_release);
            s.search.releases.iter_mut().for_each(fix_release);
            if let Some(r) = s.release.as_mut() {
                fix_release(&mut r.card);
            }
            s.library.artists.iter_mut().for_each(fix_artist);
            s.search.artists.iter_mut().for_each(fix_artist);
        });
        // The home's cards: kept for a page on its way too.
        match target {
            LikeTarget::Release(id) => self.patch_home(move |home| release_liked_on_home(home, id, on)),
            LikeTarget::Artist(id) => self.patch_home(move |home| {
                home.artist_mixes.iter_mut().filter(|a| a.id == id).for_each(|a| a.liked = on);
            }),
            _ => self.home_input_changed(),
        }
        if let LikeTarget::Track(t) = target {
            let releases = self.releases_of(&[t]);
            self.after_like_change(before, releases);
        } else if self.library_load_pending() {
            // The load on its way may have read the hearts before.
            self.load_library();
        }
    }
}


impl Worker {
    fn sources_file(&self) -> Option<std::path::PathBuf> {
        self.settings_path.as_ref().map(|p| nikgob_sources::config::beside(p))
    }

    /// `sources.toml` changed: the new lists take over. A pause of an
    /// instance that stays is kept. A file that does not parse is left as
    /// it is, and the lists in use stay.
    fn reload_sources_file(&mut self) {
        let Some(path) = self.sources_file() else { return };
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if modified.is_none() || modified == self.sources_mtime {
            return;
        }
        self.sources_mtime = modified;
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("sources.toml: {e}");
                return;
            }
        };
        let defaults = source_defaults();
        let lists = match nikgob_sources::config::parse(&text, defaults) {
            Ok(l) => l,
            Err(e) => {
                log::warn!("sources.toml: {e}");
                return;
            }
        };
        if self.qobuz.as_ref().is_some_and(|q| q.instances_now() != (lists.qobuz.clone(), lists.qobuz_own)) {
            if let Some(q) = &self.qobuz {
                q.set_instances(&lists.qobuz, lists.qobuz_own);
            }
            log::info!("sources.toml: qobuz {:?}", lists.qobuz);
        }
        if self.tidal.as_ref().is_some_and(|t| t.instances_now() != (lists.tidal.clone(), lists.tidal_own)) {
            if let Some(t) = &self.tidal {
                t.set_instances(&lists.tidal, lists.tidal_own);
            }
            log::info!("sources.toml: tidal {:?}", lists.tidal);
        }
    }

    /// Creates `sources.toml` if it is missing and opens it for editing.
    fn open_sources_config(&mut self) {
        let Some(settings) = self.settings_path.clone() else {
            self.toast("Некуда сохранить настройки", None);
            return;
        };
        let path = nikgob_sources::config::beside(&settings);
        let saved = nikgob_core::settings::Settings::load(&settings).unwrap_or_default();
        if let Err(e) = nikgob_sources::config::ensure(&path, source_defaults(), &saved.sources) {
            log::error!("sources.toml: {e}");
            self.toast("Не получилось создать файл инстансов", None);
            return;
        }
        self.sources_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Err(e) = open_file(&path) {
            log::warn!("sources.toml: {e}");
            self.toast(format!("Файл инстансов: {}", path.display()), None);
        }
    }
}

fn source_defaults() -> nikgob_sources::config::Defaults<'static> {
    nikgob_sources::config::Defaults {
        qobuz: nikgob_sources::qobuz::DEFAULT_INSTANCES,
        tidal: nikgob_sources::tidal::DEFAULT_INSTANCES,
    }
}

/// Opens `path` with the system editor (a folder opener on Windows selects
/// the file when it has no editor of its own). A test never launches one.
fn open_file(path: &std::path::Path) -> std::io::Result<()> {
    if cfg!(test) {
        log::info!("sources.toml: not opened from a test ({})", path.display());
        return Ok(());
    }
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg("start").arg("").arg(path);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    };
    let child = cmd.spawn()?;
    #[cfg(unix)]
    let _ = std::thread::Builder::new().name("opener".into()).spawn(move || {
        let _ = child.wait();
    });
    #[cfg(not(unix))]
    drop(child);
    Ok(())
}

impl Worker {
    /// Stores the Prowlarr address and key in settings.toml (never in prefs).
    fn set_indexer(&mut self, url: String, api_key: String) {
        let Some(path) = self.settings_path.clone() else {
            self.toast("Некуда сохранить настройки", None);
            return;
        };
        // A file that cannot be read or parsed is not replaced by defaults: that
        // would silently drop the other secrets stored in it.
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast("settings.toml не читается — поправь файл вручную", None);
                return;
            }
        };
        settings.indexer.prowlarr = Some(nikgob_core::settings::ProwlarrSettings {
            url: url.clone(),
            api_key,
        });
        match settings.save(&path) {
            Ok(()) => {
                let had = self.indexer.is_some();
                self.indexer = search::indexer_from(Some(&path));
                let usable = self.indexer.is_some();
                self.publish(|s| {
                    s.prefs.prowlarr_url = url;
                    s.prefs.prowlarr_configured = usable;
                });
                if usable && !had {
                    self.streaming.wish.enabled.insert(nikgob_view::SourceKey::Torrents);
                }
                self.publish_sources();
                self.wish_sources_ready();
                self.toast(
                    if usable { "Prowlarr сохранён" } else { "Адрес или ключ Prowlarr не подходят" },
                    None,
                );
                // Whether it answers, right away (the settings show it).
                if usable {
                    self.check_indexer(String::new(), String::new());
                }
            }
            Err(e) => {
                log::error!("saving settings: {e}");
                self.toast("Не удалось сохранить настройки", None);
            }
        }
    }
}

impl Worker {
    /// Moves playback to another device, resuming where it was.
    fn switch_device(&mut self, device: Option<String>) {
        if !self.playback.set_device(device) {
            return;
        }
        let volume = if self.muted { 0.0 } else { self.volume };
        self.playback.set_volume(volume);
        let prefs = self.state.read().prefs.clone();
        self.playback.configure(crossfade(&prefs), prefs.normalization);
        self.playback.set_equalizer(prefs.eq_on, prefs.eq_gains);
        let Some(e) = self.playing else { return };

        let pos = self.position_now();
        let status = self.status;
        if let Some(req) = self.request(e) {
            self.playback.play(req);
            self.restarted = Some(e.id);
            self.playback.seek(pos);
            if status == Status::Paused {
                self.playback.pause();
            }
            self.position = pos;
            self.position_at = Instant::now();
            self.rate = self.speed;
            self.preload_next();
        }
    }

    /// Copies the picture next to the library data and points the playlist at it.
    fn set_playlist_cover(&mut self, id: PlaylistId, path: PathBuf) {
        const COVERS: &str = ".musiclib/covers";
        let dir = self.res.path(COVERS);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .filter(|e| !e.is_empty() && e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
            .unwrap_or_else(|| "jpg".into());
        // A fresh name per cover: the UI caches images by URI, so reusing one
        // file name would keep showing the previous picture.
        let name = format!("playlist-{id}-{}.{ext}", nikgob_library::now_ms());
        let dest = dir.join(&name);
        let copied = std::fs::create_dir_all(&dir).and_then(|()| std::fs::copy(&path, &dest));
        if let Err(e) = copied {
            log::warn!("playlist cover {}: {e}", path.display());
            self.toast("Не получилось взять картинку", None);
            return;
        }
        let previous = self.lib.get_playlist(id).ok().flatten().and_then(|p| p.cover);
        // Relative to the library folder, so the cover survives moving it and
        // the playlist export stays valid on other devices.
        let stored = format!("{COVERS}/{name}");
        if self.log_err("обложка", self.lib.set_playlist_cover(id, Some(&stored))).is_some() {
            if let Some(old) = previous {
                let old = self.res.path(&old);
                let ours = old
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with(&format!("playlist-{id}")));
                if ours && old.parent() == Some(dir.as_path()) && old != dest {
                    let _ = std::fs::remove_file(old);
                }
            }
            self.res.invalidate();
            self.load_library();
            self.load_playlist(id);
        }
    }

    /// MusicBrainz contact and AcoustID key → settings.toml, applied live.
    fn set_metadata(&mut self, contact: String, acoustid_key: Option<String>) {
        let Some(path) = self.settings_path.clone() else {
            self.toast("Некуда сохранить настройки", None);
            return;
        };
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast("settings.toml не читается — поправь файл вручную", None);
                return;
            }
        };
        settings.metadata.contact = contact.trim().to_owned();
        if let Some(k) = acoustid_key {
            settings.metadata.acoustid_key = Some(k.trim().to_owned()).filter(|k| !k.is_empty());
        }
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast("Не удалось сохранить настройки", None);
            return;
        }
        self.set_metadata_config(&metadata_config(&settings.metadata));
        let configured = settings.metadata.acoustid_key.is_some();
        self.publish(|s| {
            s.prefs.metadata_contact = settings.metadata.contact;
            s.prefs.acoustid_configured = configured;
        });
        self.toast("Сохранено", None);
    }

    /// The Last.fm key of the graph of similar music → settings.toml
    /// (empty: none), applied live.
    fn set_lastfm_key(&mut self, key: &str) {
        let Some(configured) = self.set_metadata_secret(|m| &mut m.lastfm_key, key) else { return };
        self.publish(|s| s.prefs.lastfm_configured = configured);
        self.toast(if configured { "Сохранено" } else { "Ключ убран" }, None);
    }

    /// The ListenBrainz token of the graph's popularity → settings.toml
    /// (empty: none), applied live: the popularity is asked again at once.
    fn set_listenbrainz_token(&mut self, token: &str) {
        let Some(configured) = self.set_metadata_secret(|m| &mut m.listenbrainz_token, token) else { return };
        self.publish(|s| s.prefs.listenbrainz_configured = configured);
        self.graph_popularity_reopened();
        self.toast(if configured { "Сохранено" } else { "Токен убран" }, None);
    }

    /// Keeps one secret of the online metadata in settings.toml (empty:
    /// none) and applies the config live; whether one is set now, none when
    /// it could not be kept (a toast says why).
    fn set_metadata_secret(&mut self, field: impl FnOnce(&mut nikgob_core::settings::MetadataSettings) -> &mut Option<String>, value: &str) -> Option<bool> {
        let Some(path) = self.settings_path.clone() else {
            self.toast("Некуда сохранить настройки", None);
            return None;
        };
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast("settings.toml не читается — поправь файл вручную", None);
                return None;
            }
        };
        let slot = field(&mut settings.metadata);
        *slot = Some(value.trim().to_owned()).filter(|k| !k.is_empty());
        let configured = slot.is_some();
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast("Не удалось сохранить настройки", None);
            return None;
        }
        self.set_metadata_config(&metadata_config(&settings.metadata));
        Some(configured)
    }
}

/// The online metadata's config of settings.toml's.
fn metadata_config(m: &nikgob_core::settings::MetadataSettings) -> MetadataConfig {
    let mut cfg = MetadataConfig::new(m.contact.clone());
    if let Some(k) = &m.acoustid_key {
        cfg = cfg.with_acoustid_key(k.clone());
    }
    if let Some(k) = &m.lastfm_key {
        cfg = cfg.with_lastfm_key(k.clone());
    }
    if let Some(t) = &m.listenbrainz_token {
        cfg = cfg.with_listenbrainz_token(t.clone());
    }
    cfg
}

/// The page a queue's context id opens (`release:<id>`, `playlist:<id>`,
/// `artist:<id>`); none for the rest (a mix, a radio, a search).
fn context_page(id: &str) -> Option<nikgob_view::Route> {
    use nikgob_view::Route;
    let (kind, id) = id.split_once(':')?;
    match kind {
        "release" => id.parse().ok().map(Route::Release),
        "playlist" => id.parse().ok().map(Route::Playlist),
        "artist" => id.parse().ok().map(Route::Artist),
        _ => None,
    }
}

/// The hearts of album `id` on the home page's cards say `on`.
fn release_liked_on_home(home: &mut nikgob_view::HomeView, id: nikgob_core::model::ReleaseId, on: bool) {
    for r in [&mut home.long_unplayed, &mut home.recently_added] {
        r.iter_mut().filter(|c| c.id == id).for_each(|c| c.liked = on);
    }
    for c in &mut home.continue_listening {
        if let nikgob_view::ContinueCard::Release(c) = c
            && c.id == id
        {
            c.liked = on;
        }
    }
}

/// Which tracks the library lists ([`nikgob_view::Prefs::library_only_downloaded`]).
fn track_filter(only_local: bool) -> nikgob_library::TrackFilter {
    if only_local {
        nikgob_library::TrackFilter::LocalOnly
    } else {
        nikgob_library::TrackFilter::All
    }
}

/// What the library held before an edit of likes or playlists
/// ([`Worker::library_edited`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Held {
    /// How many tracks (`None`: unknown).
    Tracks(Option<u32>),
    /// The edit cannot move a track in or out (a playlist created or
    /// renamed, its cover).
    Same,
}

/// Crossfade length from prefs; `None` means gapless back-to-back.
/// The name of `track` as «Не нравится и скорость» lists it.
fn speed_name(lib: &Library, track: TrackId) -> Option<nikgob_view::DislikedTrack> {
    let t = lib.get_track(track).ok().flatten()?;
    Some(nikgob_view::DislikedTrack { id: track, title: t.title, artist: t.artist_credit.display() })
}

fn crossfade(p: &nikgob_view::Prefs) -> Option<Duration> {
    (p.crossfade_s > 0.0).then(|| Duration::from_secs_f32(p.crossfade_s))
}

impl Worker {
    pub(crate) fn save_prefs(&self) {
        if let Some(path) = &self.prefs_path {
            let prefs = self.state.read().prefs.clone();
            if let Ok(json) = serde_json::to_vec_pretty(&prefs) {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, path);
                }
            }
        }
    }

    /// At the start: an export next to the library that this database no
    /// longer matches (a backup of the database put back since) is written
    /// again at once, not at the next change.
    fn export_if_stale(&mut self) {
        let dir = self.res.path(".musiclib/userdata");
        match self.lib.userdata_export_stale(&dir) {
            Ok(false) => {}
            Ok(true) => {
                log::info!("userdata export in {} is not this library's, writing it again", dir.display());
                let fp = self.lib.userdata_fingerprint().ok().map(|fp| (dir.clone(), fp));
                match self.lib.export_userdata(&dir) {
                    Ok(_) => self.exported_fp = fp,
                    Err(e) => log::warn!("userdata export to {}: {e}", dir.display()),
                }
            }
            Err(e) => log::warn!("userdata export in {}: {e}", dir.display()),
        }
    }

    /// Writes playlists, likes and history next to the library (PLAN §4:
    /// the DB can be rebuilt from files + this export). At most once a
    /// minute while things change, and once more on shutdown; only when the
    /// user data itself changed (most publishes are playback, downloads and
    /// pages, which leave it alone).
    fn maybe_export(&mut self, force: bool) {
        if self.revision == self.exported_rev {
            return;
        }
        if !force && self.last_export.elapsed() < Duration::from_secs(60) {
            return;
        }
        let dir = self.res.path(".musiclib/userdata");
        let fp = self.lib.userdata_fingerprint().ok().map(|fp| (dir.clone(), fp));
        if fp.is_some() && fp == self.exported_fp {
            self.exported_rev = self.revision;
            self.last_export = Instant::now();
            return;
        }
        match self.lib.export_userdata(&dir) {
            Ok(_) => {
                self.exported_rev = self.revision;
                self.last_export = Instant::now();
                self.exported_fp = fp;
            }
            Err(e) => log::warn!("userdata export to {}: {e}", dir.display()),
        }
    }
}

/// A playback error that is YouTube refusing a stream's URLs (every client
/// asked, HTTP 403), not the video's own refusal or a lost connection.
fn youtube_refused_midway(message: &str) -> bool {
    let m = message.to_lowercase();
    (m.contains("refused") || m.contains("http 403")) && !streams::resolve::video_refused(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn a_queue_started_from_an_album_a_playlist_or_an_artist_has_its_page() {
        use nikgob_view::Route;
        let (r, p, a) = (nikgob_core::model::ReleaseId::new(), PlaylistId::new(), nikgob_core::model::ArtistId::new());
        assert_eq!(context_page(&format!("release:{r}")), Some(Route::Release(r)));
        assert_eq!(context_page(&format!("playlist:{p}")), Some(Route::Playlist(p)));
        assert_eq!(context_page(&format!("playlist:{LIKED_PLAYLIST}")), Some(Route::Playlist(LIKED_PLAYLIST)));
        assert_eq!(context_page(&format!("artist:{a}")), Some(Route::Artist(a)));
        // A radio of an artist, a mix, a broken id: no page.
        assert_eq!(context_page(&format!("radio:artist:{a}")), None);
        assert_eq!(context_page("mix:Спокойное"), None);
        assert_eq!(context_page("release:nope"), None);
        assert_eq!(context_page("release"), None);
    }

    #[test]
    fn follow_slews_small_differences_and_jumps_far_ones() {
        assert_eq!(follow(ms(10_000), ms(10_002), false, 1.0), Follow::Keep);
        assert_eq!(follow(ms(10_000), ms(10_002), false, 1.1), Follow::Rate(1.0));
        // Behind: faster; ahead: slower, never backwards.
        let Follow::Rate(r) = follow(ms(10_000), ms(10_100), false, 1.0) else { panic!() };
        assert!((r - 1.2).abs() < 1e-9);
        let Follow::Rate(r) = follow(ms(10_100), ms(10_000), false, 1.0) else { panic!() };
        assert!((r - 0.8).abs() < 1e-9);
        let Follow::Rate(r) = follow(ms(10_290), ms(10_000), false, 1.0) else { panic!() };
        assert!((r - (1.0 - SLEW_MAX)).abs() < 1e-9);
        // A seek or a loop.
        assert_eq!(follow(ms(10_000), ms(500), false, 1.0), Follow::Snap);
        assert_eq!(follow(ms(10_000), ms(10_400), false, 1.0), Follow::Snap);
        // Stalled behind us: wait for it; stalled ahead: catch up.
        assert_eq!(follow(ms(10_200), ms(10_000), true, 1.0), Follow::Rate(0.0));
        assert!(matches!(follow(ms(10_000), ms(10_100), true, 1.0), Follow::Rate(r) if r > 1.0));
    }

    /// Our clock as the runtime keeps it, driven by simulated reports.
    struct Sim {
        position: f64,
        at: f64,
        rate: f64,
        heard: Option<Duration>,
        jumps: u32,
    }

    impl Sim {
        fn at(&self, t: f64) -> f64 {
            self.position + (t - self.at) * self.rate
        }

        fn report(&mut self, t: f64, heard: f64) {
            let h = Duration::from_secs_f64(heard);
            let stalled = self.heard == Some(h);
            self.heard = Some(h);
            let ours = self.at(t);
            match follow(Duration::from_secs_f64(ours), h, stalled, self.rate) {
                Follow::Keep => {}
                Follow::Snap => {
                    (self.position, self.at, self.rate) = (heard, t, if stalled { 0.0 } else { 1.0 });
                    self.jumps += 1;
                }
                Follow::Rate(r) => (self.position, self.at, self.rate) = (ours, t, r),
            }
        }
    }

    /// Plays `secs` of simulated time: the heard position from `heard(t)`,
    /// reports every 250 ms arriving 0–12 ms late, the view sampled every
    /// frame. Returns the largest |ours − heard| from `settle` on (s) and
    /// the jumps, and checks the view never went backwards without one.
    fn run(start: f64, secs: f64, settle: f64, heard: impl Fn(f64) -> f64) -> (f64, u32) {
        let mut sim = Sim { position: start, at: 0.0, rate: 1.0, heard: None, jumps: 0 };
        let (mut worst, mut last, mut next_report, mut k) = (0.0f64, f64::MIN, 0.25, 0u64);
        let mut jumps = 0;
        let frame = 1.0 / 120.0;
        let mut t = 0.0;
        while t < secs {
            if t >= next_report {
                // Pseudo-random delivery delay, the same every run.
                k = k.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                let late = (k >> 33) as f64 / (1u64 << 31) as f64 * 0.012;
                sim.report(t, heard(t - late));
                next_report += 0.25;
            }
            let ours = sim.at(t);
            // Only a jump may move it back.
            assert!(ours >= last - 1e-9 || sim.jumps != jumps, "went back at {t}");
            (last, jumps) = (ours, sim.jumps);
            if t >= settle {
                worst = worst.max((ours - heard(t)).abs());
            }
            t += frame;
        }
        (worst, sim.jumps)
    }

    /// The numbers behind the clock: how far the lyrics and the progress
    /// can be from what is heard (`cargo test -p nikgob-runtime clock --
    /// --nocapture` prints them).
    #[test]
    fn the_clock_follows_what_is_heard() {
        // The sound clock runs 0.3 % off the system clock.
        let (drift, jumps) = run(0.0, 120.0, 1.0, |t| t * 1.003);
        assert_eq!(jumps, 0);
        // 200 ms off at the start (latency the old clock did not know).
        let (offset, jumps) = run(0.2, 30.0, 2.0, |t| t);
        assert_eq!(jumps, 0);
        let (offset_first, _) = run(0.2, 30.0, 0.0, |t| t);
        // A stream stalls for 0.8 s at 10 s, then plays on: told by the
        // second report that stands still, by then up to half a second
        // ahead, it waits or jumps back once.
        let stall_at = |t: f64| if t < 10.0 { t } else if t < 10.8 { 10.0 } else { t - 0.8 };
        let (stall, jumps) = run(0.0, 30.0, 1.0, stall_at);
        assert!(jumps <= 1, "{jumps}");
        let (after_stall, _) = run(0.0, 30.0, 12.5, stall_at);
        println!(
            "clock error: drift 0.3 % {:.1} ms; 200 ms off {:.0} ms at first, {:.1} ms after 2 s; \
             0.8 s stall {:.0} ms at most, {:.1} ms 1.7 s after it",
            drift * 1e3,
            offset_first * 1e3,
            offset * 1e3,
            stall * 1e3,
            after_stall * 1e3,
        );
        assert!(drift < 0.02, "{drift}");
        assert!(offset < 0.02, "{offset}");
        assert!(stall < 0.55, "{stall}");
        assert!(after_stall < 0.02, "{after_stall}");
    }
}
