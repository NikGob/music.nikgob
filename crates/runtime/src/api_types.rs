//! The runtime's public API: what is played, the events back, the handles.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::streaming::{Service as StreamKind, StreamService};
use nikgob_library::Library;
use nikgob_view::{Command, ViewState};
use parking_lot::{RwLock, RwLockReadGuard};

use super::{ImportSetup, Msg, StreamSchedule, WebLogin, Worker};
use crate::streams;

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

/// What a playing item really is, as data: the words are made when the
/// player is published, in the UI's language of that moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QualityLabel {
    /// What the stream plays: its codec, bitrate and whether it varies.
    Stream(nikgob_core::streaming::StreamQuality),
    /// A name of its own, not a codec («Soloist»).
    Text(String),
    /// SoundCloud's own player.
    SoundcloudPlayer,
}

impl QualityLabel {
    /// The label in `loc`.
    pub fn text_in(&self, loc: nikgob_core::i18n::Locale) -> String {
        match self {
            Self::Stream(q) => q.words_in(loc),
            Self::Text(text) => text.clone(),
            Self::SoundcloudPlayer => nikgob_core::i18n::backend_in(loc).soundcloud_player.to_owned(),
        }
    }
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
    /// Item `id` was paused from outside the player (another device took
    /// Spotify's one stream): the player pauses as if the user had, keeping
    /// the position, and nothing moves on.
    PausedElsewhere { id: u64 },
    Ended { id: u64 },
    Error { id: u64, message: String },
    /// What a stream item actually plays ("Opus ~160 кбит/с"), once opened.
    Quality { id: u64, label: QualityLabel },
    /// How long the item is, as the engine found it (the container's
    /// length, else the stream's): the length of a track that has none.
    Duration { id: u64, duration: Duration },
    /// The item plays at this speed after all, not at its request's (a
    /// live track of another player cannot be sped up).
    Speed { id: u64, speed: f32 },
    /// The output device of engine `generation` did not open (the one
    /// heard, or one a device switch started: that one is dropped and the
    /// one heard plays on).
    DeviceError { generation: u64, message: String },
    /// Engine `generation`, started by a device switch, has its device
    /// open: it can take over ([`Playback::adopt_device`]).
    DeviceReady { generation: u64 },
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
    /// The sleep timer's fade: the sound glides to `level` (0..1, on the
    /// volume's perceptual curve) over `over` (zero: at once), whatever
    /// plays; the volume stays as the user set it.
    fn set_sleep_fade(&mut self, _level: f32, _over: Duration) {}
    /// Switches the output device (None = system default). Returns true if
    /// the engine was restarted at once and lost what it was playing; a
    /// backend whose device opens later says so with
    /// [`PlaybackEvent::DeviceReady`] and returns false.
    fn set_device(&mut self, _device: Option<String>) -> bool {
        false
    }
    /// Engine `generation`, started by [`Self::set_device`], has opened its
    /// device: it replaces the one heard. True when it did (what played is
    /// lost and played again on it).
    fn adopt_device(&mut self, _generation: u64) -> bool {
        false
    }
    /// Engine `generation` did not open its device: dropped if it was
    /// waiting to take over.
    fn drop_device(&mut self, _generation: u64) {}
    /// The duck: the sound glides to the linear `gain` (0..1, a factor on
    /// what the volume and the sleep fade make it) over `fade` (zero: at
    /// once); 1 restores it. The volume stays as the user set it; the duck
    /// holds whatever plays next, across gapless joins and device switches,
    /// until it is changed.
    fn set_duck(&mut self, _gain: f32, _fade: Duration) {}
    /// Whether sound can come out at all (false: a stand-in for an engine
    /// that did not start).
    fn available(&self) -> bool {
        true
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

/// What the host plays through when the audio engine did not start: it
/// plays nothing and says so ([`Playback::available`]), so the view shows
/// [`ViewState::no_audio`].
#[derive(Default)]
pub struct NoAudio;

impl Playback for NoAudio {
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
    fn available(&self) -> bool {
        false
    }
}

/// Services added after start-up (import, downloads) receive these.
pub trait Service: Send {
    /// Returns true if the command was handled.
    fn handle(&mut self, command: &Command, state: &mut ViewState) -> bool;
}

/// What the runtime does besides playing, chosen by its host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// The heavy passes over the music in the background: the sound
    /// analysis (bliss, the neural sound model, the previews of new songs),
    /// the lyrics alignment, the spectrum check of the files (fake
    /// lossless) and the check for files that hold another song. On by
    /// default; the headless player turns them off (a Raspberry Pi gets
    /// their results from another device).
    pub background_compute: bool,
    /// Playlists, likes and history written to `.musiclib/userdata` next to
    /// the library (the app's). The headless player turns it off: its
    /// library folder may be the one the app exports to, and its user data
    /// goes between devices another way.
    pub userdata_export: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self { background_compute: true, userdata_export: true }
    }
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

/// Handle to the running worker.
pub struct Runtime {
    tx: Sender<Msg>,
    pb_tx: Sender<PlaybackEvent>,
    state: Arc<RwLock<ViewState>>,
    /// The worker, joined by [`Runtime::shutdown`] (or the drop) ...
    worker: parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>,
    /// ... which sets this once it has ended, for other callers to wait on.
    stopped: Arc<(parking_lot::Mutex<bool>, parking_lot::Condvar)>,
    /// The worker's thread (a shutdown from it cannot wait for itself).
    worker_id: std::thread::ThreadId,
    /// The streaming services the worker got, for the local API's lookups.
    streams: Vec<Arc<dyn StreamService>>,
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
        Self::start_with(config, playback, wake, schedule, RuntimeOptions::default())
    }

    /// [`Self::start_scheduled`] with the host's [`RuntimeOptions`] (the
    /// headless player's background passes off).
    pub fn start_with(
        config: RuntimeConfig,
        playback: Box<dyn Playback>,
        wake: Arc<dyn Fn() + Send + Sync>,
        schedule: StreamSchedule,
        options: RuntimeOptions,
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
        let streams = config.streams.clone();
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
                    w.compute = options.background_compute;
                    w.userdata_export = options.userdata_export;
                    w.run(rx, pb_rx, in_rx);
                })
                .expect("spawn runtime thread")
        };
        let worker_id = worker.thread().id();
        let rt = Self {
            tx,
            pb_tx,
            state,
            worker: parking_lot::Mutex::new(Some(worker)),
            stopped: Arc::default(),
            worker_id,
            streams,
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

    /// Asks this server for share pages instead of MusicLink, song.link
    /// and Deezer (tests).
    pub fn set_share_base(&self, base: impl Into<String>) {
        let _ = self.tx.send(Msg::ShareBase(base.into()));
    }

    /// This runs on a `kind` of device (a computer until told): a speaker's
    /// (a Pi, a TV, a server) recommendations are for a room, its own plays
    /// a room's.
    pub fn set_device_kind(&self, kind: nikgob_view::MeshKind) {
        let _ = self.tx.send(Msg::DeviceKind(kind));
    }

    /// The graph of similar music asks this online metadata (tests: its
    /// clients pointed at a mock server) instead of the import's.
    pub fn set_graph_metadata(&self, m: nikgob_metadata::Metadata) {
        let _ = self.tx.send(Msg::GraphMetadata(Box::new(m)));
    }
}

impl Runtime {
    /// Stops the worker as the drop does (the session, the prefs and the
    /// rest saved) and waits until it has ended, through a shared handle: a
    /// host whose other parts (an API server) still hold the runtime stops
    /// it at a signal. Every caller returns once the worker is gone (one
    /// joins it, the others wait for that); commands sent after it go
    /// nowhere. Called on the worker's own thread (from `wake`), it only
    /// asks the worker to stop and returns: the loop ends after the turn
    /// it is in, and a later call elsewhere (or the drop) waits for it.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
        if std::thread::current().id() == self.worker_id {
            return;
        }
        let worker = self.worker.lock().take();
        let (done, ended) = &*self.stopped;
        match worker {
            Some(w) => {
                let _ = w.join();
                *done.lock() = true;
                ended.notify_all();
            }
            None => {
                let mut d = done.lock();
                while !*d {
                    ended.wait(&mut d);
                }
            }
        }
    }
}

/// A handle another thread drives the player with, as the UI does (the
/// local API's server): it sends commands and reads the state, and does not
/// keep the worker alive.
#[derive(Clone)]
pub struct RuntimeHandle {
    tx: Sender<Msg>,
    state: Arc<RwLock<ViewState>>,
    streams: Vec<Arc<dyn StreamService>>,
}

impl RuntimeHandle {
    /// The service of `kind` as the player would ask it now: switched on,
    /// and (Spotify) signed in; `None` otherwise.
    pub fn service(&self, kind: StreamKind) -> Option<Arc<dyn StreamService>> {
        if !self.state.read().prefs.service_enabled(kind) {
            return None;
        }
        self.streams.iter().find(|s| s.service() == kind && streams::usable(kind, &s.account())).cloned()
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(Msg::Cmd(command));
    }

    pub fn send_all(&self, commands: impl IntoIterator<Item = Command>) {
        for c in commands {
            self.send(c);
        }
    }

    /// The state (a short read lock: copy out what is needed).
    pub fn state(&self) -> RwLockReadGuard<'_, ViewState> {
        self.state.read()
    }

    /// The mesh service takes the commands that need the network from now
    /// on (pairing, invites, forgetting); `f` must not block.
    pub fn link_mesh(&self, f: impl Fn(Command) + Send + Sync + 'static) {
        let _ = self.tx.send(Msg::MeshLink(Arc::new(f)));
    }

    /// The mesh service takes the network part of playing on another
    /// device from now on (following a zone, forwarding, transfers, the
    /// files fetched); `f` must not block, and says whether it took the job.
    pub fn link_connect(&self, f: impl Fn(nikgob_view::ConnectJob) -> bool + Send + Sync + 'static) {
        let _ = self.tx.send(Msg::ConnectLink(Arc::new(f)));
    }

    /// Before the mesh stops: the devices still being forgotten are passed
    /// to it now. Waits at most `wait` for the loop to do it; whether it did.
    pub fn flush_mesh(&self, wait: Duration) -> bool {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.tx.send(Msg::MeshFlush(tx)).is_err() {
            return false;
        }
        rx.recv_timeout(wait).is_ok()
    }
}

impl Runtime {
    /// A [`RuntimeHandle`] for another thread.
    pub fn handle(&self) -> RuntimeHandle {
        RuntimeHandle { tx: self.tx.clone(), state: self.state.clone(), streams: self.streams.clone() }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // The last handle went on the worker's own thread: it is asked to
        // stop and its thread ends by itself, unjoined.
        if std::thread::current().id() == self.worker_id {
            let _ = self.tx.send(Msg::Shutdown);
            return;
        }
        self.shutdown();
    }
}
