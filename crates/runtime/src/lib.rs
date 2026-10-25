//! The runtime between the core services and a design system.
//!
//! One worker thread executes [`Command`]s from the UI against the library,
//! the play queue and the playback backend, and publishes the result into a
//! shared [`ViewState`]. The UI reads that state each frame under a short
//! read lock and never calls the library or the audio engine itself.

mod analysis;
mod convert;
mod enrich;
mod hd_covers;
mod home;
mod home_v2;
mod import;
mod lyrics;
mod playlist_cover;
mod recs;
mod search;
mod tags;
mod torrents;
pub mod queue;
mod session;
mod editions;
mod streams;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, select, tick};
use nikgob_core::model::{
    EndReason, EventContext, EventMeta, EventOrigin, EventType, LikeTarget, PlaylistId, SourceId, SourceKind, TrackId,
};
use nikgob_core::streaming::{Service as StreamKind, StreamService};
use nikgob_library::{Library, ReleaseSortKey, Sort, TrackSortKey};
use nikgob_view::{Command, LIKED_PLAYLIST, Status, Toast, ViewState};
use parking_lot::{RwLock, RwLockReadGuard};

pub use convert::Resolver;
pub use import::ImportSetup;
pub use recs::{HomePreview, ModelPreview, PreviewTile, preview_home, preview_home_with, preview_ready_tiles, preview_replay, preview_tile_days};
pub use nikgob_import::ImportConfig;
pub use nikgob_metadata::MetadataConfig;
pub use streams::{StreamSchedule, WINDOW_HINT, WebLogin, WebLoginError};
use import::{IMPORT_ID, ImportService, Internal};
use queue::{Entry, Queue};

/// How many of the tracks before the current one the queue view lists.
const HISTORY_SHOWN: usize = 50;

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
}

/// Events the backend reports back (from any thread) through
/// [`Runtime::playback_sender`].
#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackEvent {
    Started { id: u64 },
    /// The preloaded next item took over at the sample boundary.
    TrackChanged { to: u64 },
    Position { id: u64, position: Duration },
    Paused,
    Resumed,
    Ended { id: u64 },
    Error { id: u64, message: String },
    /// What a stream item actually plays ("Opus ~160 кбит/с"), once opened.
    Quality { id: u64, label: String },
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
    OdesliBase(String),
    WebLogin(StreamKind, Arc<dyn WebLogin>),
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

    /// Logs `service` in through this sign-in window of the app for a
    /// [`nikgob_core::streaming::LoginInput::Browser`] login (YouTube Music's
    /// Google sign-in; see [`WebLogin`]). A logout also forgets its session.
    pub fn set_web_login(&self, service: StreamKind, login: Arc<dyn WebLogin>) {
        let _ = self.tx.send(Msg::WebLogin(service, login));
    }

    /// Asks this Odesli base url for share links instead of
    /// api.song.link (tests).
    pub fn set_odesli_base(&self, base: impl Into<String>) {
        let _ = self.tx.send(Msg::OdesliBase(base.into()));
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

/// How far the engine's position may differ from our extrapolation before
/// the view is told (normal jitter is a few tens of milliseconds).
const POSITION_DRIFT: Duration = Duration::from_millis(1500);
/// After we move the position ourselves (seek, start), a report the engine
/// sent before it heard about that can still be on its way: such a far-off
/// report is ignored for this long.
const POSITION_SETTLE: Duration = Duration::from_millis(500);
/// A toast no snackbar took away is dropped after this long.
const TOAST_KEPT: Duration = Duration::from_secs(60);

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
    /// `session.json` (volume and queue between runs) and what was last
    /// written to it.
    session_path: Option<PathBuf>,
    session_saved: String,
    /// Editions of one album found last time (see [`editions`]).
    editions: Vec<nikgob_import::EditionGroup>,
    services: Vec<Box<dyn Service>>,
    toast_id: u64,
    revision: u64,
    play_context: EventContext,
    /// What the queue plays (`release:<id>`, `playlist:<id>`, a mix, a
    /// radio's seed): the history's `context_id`.
    play_context_id: Option<String>,
    /// Who chose the next start, when the command that starts it knows.
    next_origin: Option<EventOrigin>,
    /// The queue generation of the last start: a start in another queue
    /// is the user's choice.
    origin_gen: Option<u64>,
    /// The playing entry's end is recorded already.
    ended: bool,
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
    /// Last torrent search, indexed like `SearchView::torrents.results`.
    candidates: Vec<nikgob_core::indexer::Candidate>,
    torrents: torrents::Torrents,
    /// Accounts, stream search and pages, likes sync, wishlist, sharing.
    streaming: streams::Streams,
    /// The background sound analysis and the vector index it fills.
    analysis: analysis::FileAnalysis,
    /// Mixes, radio, discovery (stage 4).
    recs: recs::RecsState,
    /// Albums whose larger cover was looked up in this run.
    hd_asked: hd_covers::Asked,
    /// The album the queue plays (and the queue's generation then): its
    /// tracks the services add later join the queue.
    album_queue: Option<(nikgob_core::model::ReleaseId, u64)>,
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
        let streaming = streams::Streams::new(internal.clone());
        let analysis = analysis::FileAnalysis::new(internal.clone());
        let recs = recs::RecsState::new(internal.clone());
        let import = config
            .import
            .and_then(|setup| ImportService::start(setup, config.library.clone(), internal));
        let mut playback = playback;
        let session_path = config.prefs_path.as_ref().map(|p| p.with_file_name("session.json"));
        let saved = session_path.as_deref().map(session::Session::load).unwrap_or_default();
        let volume = saved.volume.unwrap_or(0.8).clamp(0.0, 1.0);
        playback.set_volume(if saved.muted { 0.0 } else { volume });
        playback.configure(crossfade(&config.prefs), config.prefs.normalization);
        Self {
            res: Resolver::new(config.library.clone(), config.library_dir),
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
            jumped_at: Instant::now(),
            volume,
            muted: saved.muted,
            resume_at: None,
            failover: None,
            session_path,
            session_saved: String::new(),
            editions: Vec::new(),
            services: Vec::new(),
            toast_id: 0,
            revision: 0,
            play_context: EventContext::Other,
            play_context_id: None,
            next_origin: None,
            origin_gen: None,
            ended: true,
            autoplay_entries: Default::default(),
            prefs_path: config.prefs_path,
            settings_path: config.settings_path.clone(),
            exported_rev: 0,
            last_export: Instant::now(),
            exported_fp: None,
            indexer: search::indexer_from(config.settings_path.as_deref()),
            candidates: Vec::new(),
            torrents: torrents::Torrents::new(torrent_state, torrent_offline),
            import,
            streams: config.streams,
            tried: Vec::new(),
            playing_source: None,
            qualities: Vec::new(),
            streaming,
            analysis,
            hd_asked: Default::default(),
            album_queue: None,
            recs,
        }
    }

    fn run(&mut self, rx: Receiver<Msg>, pb: Receiver<PlaybackEvent>, internal: Receiver<Internal>) {
        let ticker = tick(self.streaming.schedule.tick);
        let fast = tick(Duration::from_secs(1));
        // Once each, after a backup: doubles of a song, an artist or an
        // album become one, and every album has a line per place it counts.
        self.lib.repair_catalog();
        self.load_torrent_history();
        self.publish_downloads();
        self.resume_torrents();
        self.start_enrich();
        self.restore_session();
        self.offer_editions();
        self.wish_restart();
        self.start_streams();
        self.start_analysis();
        loop {
            select! {
                recv(rx) -> m => match m {
                    Ok(Msg::Cmd(c)) => self.command(c),
                    Ok(Msg::Service(s)) => self.services.push(s),
                    Ok(Msg::Indexer(i)) => {
                        self.indexer = Some(i);
                        self.wish_tick();
                    }
                    Ok(Msg::OdesliBase(base)) => self.set_odesli_base(&base),
                    Ok(Msg::WebLogin(service, login)) => self.set_web_login(service, login),
                    Ok(Msg::Shutdown) | Err(_) => {
                        self.record_closed();
                        break;
                    }
                },
                recv(pb) -> e => if let Ok(e) = e { self.playback_event(e) },
                recv(internal) -> m => if let Ok(m) = m { self.internal(m) },
                recv(fast) -> _ => self.poll_torrents(),
                recv(ticker) -> _ => {
                    self.expire_toasts();
                    self.maybe_export(false);
                    self.save_session();
                    self.stream_tick();
                    self.recs_tick();
                }
            }
            self.analysis.set_playing(matches!(self.status, Status::Playing | Status::Buffering));
        }
        self.save_session();
        self.maybe_export(true);
        self.playback.stop();
        self.shutdown_torrents();
        self.streaming.analysis.shutdown();
        self.analysis.shutdown();
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
            s.toasts.iter().any(|t| t.at.elapsed() > TOAST_KEPT)
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
            Command::PickLyricsSource(t, src) => self.pick_lyrics_source(t, src),
            Command::SeekBy(ms) => {
                let p = self.position_now().as_millis() as i64 + ms;
                self.seek(Duration::from_millis(p.max(0) as u64));
            }
            Command::SetVolume(v) => {
                let v = v.clamp(0.0, 1.0);
                if v == self.volume && !self.muted {
                    return;
                }
                self.volume = v;
                self.muted = false;
                self.playback.set_volume(self.volume);
                self.publish_volume();
            }
            Command::ToggleMute => {
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
                let ids = self.release_tracks(id, start.is_none());
                let title = self
                    .lib
                    .get_release(id)
                    .ok()
                    .flatten()
                    .map(|r| format!("альбома «{}»", r.title));
                self.play_context_id = Some(format!("release:{id}"));
                self.play_list(ids, start, shuffle, title, EventContext::Album);
                // An album known in part plays at once; the rest of its
                // tracklist joins the queue when the services tell it.
                self.album_queue = Some((id, self.queue.generation()));
                self.complete_tracklist(id);
            }
            Command::PlayPlaylist { id, start, shuffle } => {
                let (ids, name) = self.playlist_tracks(id);
                let name = format!("плейлиста «{name}»");
                self.play_context_id = Some(format!("playlist:{id}"));
                self.play_list(ids, start, shuffle, Some(name), EventContext::Playlist);
            }
            Command::PlayArtist { id, shuffle } => {
                let rows = self
                    .log_err("исполнитель", self.lib.tracks_of_artist(id, 0, 500))
                    .unwrap_or_default();
                let name = self.lib.get_artist(id).ok().flatten().map(|a| a.name);
                let ids = rows.iter().map(|r| r.id).collect();
                self.play_context_id = Some(format!("artist:{id}"));
                self.play_list(ids, None, shuffle, name, EventContext::Artist);
            }
            Command::PlayTracks {
                tracks,
                start,
                context,
            } => {
                self.user_switch(EventOrigin::Manual);
                self.queue.set(&tracks, start, Some(context));
                self.play_context = EventContext::Library;
                self.play_context_id = None;
                if let Some(e) = self.queue.current() {
                    self.start(e);
                }
            }
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
                let ids = self.release_tracks(id, true);
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
                let ids: Vec<TrackId> = self
                    .log_err("альбом", self.lib.tracks_of_release(id))
                    .unwrap_or_default()
                    .iter()
                    .map(|r| r.id)
                    .collect();
                self.command(Command::Enqueue(ids));
            }
            Command::EnqueueArtist(id) => {
                let ids: Vec<TrackId> = self
                    .log_err("исполнитель", self.lib.tracks_of_artist(id, 0, 2000))
                    .unwrap_or_default()
                    .iter()
                    .map(|r| r.id)
                    .collect();
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
                self.queue.clear_upcoming();
                self.preload_next();
                self.publish_player();
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
                        (true, true) if self.indexer.is_some() => "В Любимых · скачаю в фоне",
                        (true, true) => "В Любимых · скачается, когда подключишь торренты",
                        (true, false) => "Добавлено в Любимые",
                        (false, _) => "Убрано из Любимых",
                    };
                    self.toast(text, Some(("Отменить".into(), Command::Like(target, !on))));
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
                if id == LIKED_PLAYLIST {
                    let _ = self.lib.unlike(track);
                } else if let Ok(entries) = self.lib.playlist_entries(id, 0, u32::MAX)
                    && let Some(e) = entries.iter().find(|e| e.track.id == track)
                {
                        let _ = self.log_err("плейлист", self.lib.playlist_remove(e.entry.id));
                }
                self.load_playlist(id);
                let releases = if id == LIKED_PLAYLIST { self.releases_of(&[track]) } else { Vec::new() };
                self.library_edited(before, releases);
                self.playlist_edited(id);
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
            Command::Search(q) => self.search(q),
            Command::SetPrefs(p) => self.set_prefs(p),
            Command::SetPref(patch) => {
                let mut p = self.state.read().prefs.clone();
                patch.apply(&mut p);
                self.set_prefs(Box::new(p));
            }
            Command::MoveMastersDir(dir) => self.move_masters_dir(dir),
            Command::SetIndexer { url, api_key } => self.set_indexer(url, api_key),
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
            | Command::SetStreamSecrets { .. }
            | Command::SetSearchSources(_)
            | Command::LoadStream(_)
            | Command::PlayStreamTracks { .. }
            | Command::PlaySearch { .. }
            | Command::PlayStreamAlbum { .. }
            | Command::PlayStreamPlaylist { .. }
            | Command::PlayStreamNext(_)
            | Command::EnqueueStream(_)
            | Command::LikeStream { .. }
            | Command::LikeStreamAlbum { .. }
            | Command::DownloadStream { .. }
            | Command::DownloadStreamAlbum(_)
            | Command::DownloadTracks(_)
            | Command::DownloadRelease(_)
            | Command::FollowStreamPlaylist(_)
            | Command::AddStreamToPlaylist(..)
            | Command::Share(_)
            | Command::LoadWishlist
            | Command::WishlistDownload(_)
            | Command::WishlistDismiss(_)
            | Command::WishlistRestore(_)
            | Command::WishlistRetry(_)
            | Command::WishlistSaveFromYoutube(_)
            | Command::WishlistAttach { .. }
            | Command::ImportFromService(_)
            | Command::ImportExportFiles(_)
            | Command::ClearStreamCache
            | Command::WindowState { .. }
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
        if self.status == Status::Playing {
            self.position + self.position_at.elapsed()
        } else {
            self.position
        }
    }

    /// What to hand the engine for `e`: its best source that is usable and
    /// not in `skip` (local > Spotify > YouTube, then quality). A stream
    /// source needs its service registered. A stream copy set aside earlier
    /// (its details or its video refused once) is tried again when nothing
    /// else plays, unless it was refused in the last hours (`REFUSED_TTL`).
    /// Returns the source too; says in the log why nothing plays.
    fn request_skipping(
        &mut self,
        e: Entry,
        skip: &[SourceId],
    ) -> Option<(PlayRequest, SourceId, SourceKind)> {
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
                format!("no audio from {} now ({} sources set aside)", services.join(", "), sources.iter().filter(|s| !s.available).count())
            };
            log::warn!("{}: nothing to play: {why}", e.track);
            return None;
        };
        if !src.available {
            log::info!("{}: trying its {} copy {} set aside earlier", e.track, src.kind.as_str(), src.locator);
        }
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
                let gain = import::file_gain_db(&path);
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
        let req = PlayRequest {
            id: e.id,
            source,
            gain_db,
            duration,
        };
        Some((req, src.id, src.kind))
    }

    /// [`Self::request_skipping`] with nothing skipped.
    fn request(&mut self, e: Entry) -> Option<PlayRequest> {
        self.request_skipping(e, &[]).map(|(req, ..)| req)
    }

    /// Hands `req` (for the playing entry `e`) to the engine. A stream reads
    /// Buffering until the engine reports its start.
    fn play_request(&mut self, e: Entry, req: PlayRequest, source: SourceId, kind: SourceKind) {
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
        self.jumped_at = Instant::now();
        self.publish_player();
    }

    fn play_list(
        &mut self,
        ids: Vec<TrackId>,
        start: Option<TrackId>,
        shuffle: bool,
        context: Option<String>,
        ctx: EventContext,
    ) {
        if ids.is_empty() {
            self.toast("Нечего играть", None);
            return;
        }
        let start_idx = start
            .and_then(|s| ids.iter().position(|&t| t == s))
            .unwrap_or_else(|| {
                if shuffle {
                    fastrand::usize(..ids.len())
                } else {
                    0
                }
            });
        self.user_switch(EventOrigin::Manual);
        self.queue.shuffle = false;
        self.queue.set(&ids, start_idx, context);
        if shuffle {
            self.queue.set_shuffle(true);
        }
        self.play_context = ctx;
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
                self.toast("Нет файла для этого трека", None);
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
        self.position = Duration::ZERO;
        self.position_at = Instant::now();
        self.jumped_at = Instant::now();
        self.record_start(e, (!kind.is_local()).then_some(kind));
        self.preload_next();
        // What only YouTube Music may have is looked up ahead, gapless or not.
        self.resolve_ahead();
        self.publish_now_playing();
        self.likes_if_stale();
    }

    fn preload_next(&mut self) {
        // Gapless off: nothing is prepared, the next track starts after `Ended`.
        if !self.state.read().prefs.gapless {
            self.playback.set_next(None);
            return;
        }
        let next = self.queue.peek_auto_next();
        let req = next.and_then(|n| self.request(n));
        self.playback.set_next(req);
    }

    fn pause(&mut self) {
        if matches!(self.status, Status::Playing | Status::Buffering) {
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
                self.position_at = Instant::now();
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
        let context_id = self.radio_context_id(e.id).or_else(|| self.play_context_id.clone());
        self.ended = false;
        let meta = EventMeta { origin: Some(origin), context_id, end_reason: None };
        let _ = self.lib.record_event_with(e.track, EventType::Start, kind, Some(0), self.play_context, meta);
        self.recs_started(e.track, origin);
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
        let meta = EventMeta { end_reason: Some(reason), ..EventMeta::default() };
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
        match ev {
            PlaybackEvent::Started { id } => {
                if self.restarted == Some(id) {
                    self.restarted = None;
                }
                if self.playing.map(|e| e.id) == Some(id) {
                    if self.status == Status::Buffering {
                        self.status = Status::Playing;
                    }
                    // A failover starts where the failed source stopped.
                    self.position = match self.failover.take() {
                        Some((failed, at)) if failed == id => at,
                        _ => Duration::ZERO,
                    };
                    self.position_at = Instant::now();
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
                    self.playing_source = self
                        .request_skipping(e, &[])
                        .map(|(_, source, kind)| (source, kind));
                    self.position = Duration::ZERO;
                    self.position_at = Instant::now();
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
                    self.status = Status::Playing;
                    self.position = position;
                    self.position_at = Instant::now();
                    self.publish_player();
                    return;
                }
                if self.playing.map(|e| e.id) == Some(id) {
                    // The view extrapolates from its last publish and stops at
                    // the track's end. When the engine reports somewhere else
                    // (a repeat-one loop or a seek we did not hear about), it
                    // would stay frozen there: publish the real position.
                    let drifted = self.status == Status::Playing
                        && self.position_now().abs_diff(position) > POSITION_DRIFT;
                    if drifted && self.jumped_at.elapsed() < POSITION_SETTLE {
                        return;
                    }
                    self.position = position;
                    self.position_at = Instant::now();
                    if drifted {
                        self.publish_player();
                    }
                }
            }
            PlaybackEvent::Paused | PlaybackEvent::Resumed => {}
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
        let position = self.position_now();
        let (volume, muted) = (self.volume, self.muted);
        let (shuffle, repeat) = (self.queue.shuffle, self.queue.repeat);
        let context = self.queue.context.clone();
        let upcoming: Vec<Entry> = self.queue.upcoming().iter().take(200).copied().collect();
        let history = self.queue.history();
        let history: Vec<Entry> = history[history.len().saturating_sub(HISTORY_SHOWN)..].to_vec();
        let current_entry = self.queue.current().map(|e| e.id);
        let audio_service = self.playing_source.and_then(|(_, k)| k.service());
        let audio_quality = self.audio_quality();
        // The rows of both lists in one query, before the lock.
        let ids: Vec<TrackId> = upcoming.iter().chain(&history).map(|e| e.track).collect();
        let found = self.lib.get_track_rows(&ids).unwrap_or_default();
        let res = &mut self.res;
        let mut rows = |list: &[Entry]| -> Vec<nikgob_view::QueueEntry> {
            list.iter()
                .filter_map(|e| {
                    found.get(&e.track).map(|r| nikgob_view::QueueEntry {
                        entry: e.id,
                        track: res.track(r),
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
            p.position = position;
            p.at = Instant::now();
            p.volume = volume;
            p.muted = muted;
            p.shuffle = shuffle;
            p.repeat = repeat;
            p.context = context;
            s.queue.next = next;
            s.queue.prev = prev;
            s.queue.current_entry = current_entry;
            s.queue.radio = radio;
        });
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
        }
        self.publish_player();
    }

    // ---------------------------------------------------------------- loads

    fn load_library(&mut self) {
        self.stream_analysis_changed();
        self.recs_library_changed();
        let sort = self.state.read().library.release_sort;
        let key = match sort {
            nikgob_view::ReleaseSort::Artist => Sort::asc(ReleaseSortKey::Artist),
            nikgob_view::ReleaseSort::Title => Sort::asc(ReleaseSortKey::Title),
            nikgob_view::ReleaseSort::Year => Sort::desc(ReleaseSortKey::Year),
            nikgob_view::ReleaseSort::Added => Sort::desc(ReleaseSortKey::Added),
        };
        let releases = self
            .log_err("библиотека", self.lib.releases(0, 20_000, key))
            .unwrap_or_default();
        let playlists = self
            .log_err("плейлисты", self.lib.list_playlists())
            .unwrap_or_default();
        let artists = self
            .log_err(
                "исполнители",
                self.lib.artists(0, 20_000, Sort::asc(nikgob_library::ArtistSortKey::Name)),
            )
            .unwrap_or_default();
        // The inconspicuous "только скачанное": what has a file.
        let only_local = self.state.read().prefs.library_only_downloaded;
        let (releases, artists) = if only_local {
            (
                releases.into_iter().filter(|r| r.local_tracks > 0).collect(),
                artists.into_iter().filter(|a| a.local_track_count > 0).collect(),
            )
        } else {
            (releases, artists)
        };
        let count = self
            .lib
            .count_tracks(track_filter(only_local))
            .unwrap_or(0) as usize;
        // The overview's rows: the latest albums in and the most played.
        let tracks = self.lib.rec_tracks().unwrap_or_default();
        let stats = self.lib.all_track_stats().unwrap_or_default();
        let per_artist = home::artist_track_counts(&tracks, only_local);
        let often = home::often_played(&tracks, &stats, 12);
        let mut recent: Vec<&nikgob_library::ReleaseRow> = releases.iter().collect();
        recent.sort_by_key(|r| std::cmp::Reverse(r.release.added_at));
        recent.truncate(12);
        let by_id: std::collections::HashMap<_, _> = releases.iter().map(|r| (r.release.id, r)).collect();
        let often: Vec<&nikgob_library::ReleaseRow> = often.iter().filter_map(|id| by_id.get(id).copied()).collect();
        let res = &mut self.res;
        let releases: Vec<_> = releases.iter().map(|r| res.release(r)).collect();
        let playlists: Vec<_> = playlists.iter().map(|p| res.playlist(p)).collect();
        let artists: Vec<_> = artists
            .iter()
            .map(|a| {
                let mut card = res.artist(a);
                card.track_count = per_artist.get(&a.artist.id).copied().unwrap_or(card.track_count);
                card
            })
            .collect();
        let recent: Vec<_> = recent.iter().map(|r| res.release(r)).collect();
        let often: Vec<_> = often.iter().map(|r| res.release(r)).collect();
        self.publish(|s| {
            s.library.releases = releases;
            s.library.playlists = playlists;
            s.library.artists = artists;
            s.library.recently_added = recent;
            s.library.often_played = often;
            s.library.track_count = count;
            s.library.loaded = true;
        });
        let sort = self.state.read().library.track_sort;
        self.load_tracks(0, 200, sort);
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
        let detail = self.res.release_detail(&release, &listing, liked);
        self.publish(|s| {
            s.release = Some(detail);
        });
        self.complete_tracklist(id);
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

    fn playlist_tracks(&mut self, id: PlaylistId) -> (Vec<TrackId>, String) {
        if id == LIKED_PLAYLIST {
            let ids = self.lib.liked_track_ids().unwrap_or_default();
            return (ids, "Любимые".into());
        }
        let entries = self.lib.playlist_entries(id, 0, u32::MAX).unwrap_or_default();
        let name = self
            .lib
            .get_playlist(id)
            .ok()
            .flatten()
            .map_or_else(String::new, |p| p.name);
        (entries.iter().map(|e| e.track.id).collect(), name)
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
        self.torrent_prefs(&p);
        self.publish(|s| s.prefs = *p.clone());
        self.save_prefs();
        self.import_prefs(&p);
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
        if (old.spotify_enabled, old.youtube_enabled, old.deezer_enabled)
            != (p.spotify_enabled, p.youtube_enabled, p.deezer_enabled)
        {
            self.publish_accounts();
            self.preload_next();
        }
        if old.library_only_downloaded != p.library_only_downloaded {
            self.load_library();
        }
        if old.auto_download_limit != p.auto_download_limit {
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
        if old.dislike_in_recs != p.dislike_in_recs {
            self.recs_prefs_changed();
        }
        if old.autoplay && !p.autoplay {
            self.autoplay_off();
        }
        if !old.autoplay && p.autoplay {
            self.publish_player();
        }
    }

    fn load_home(&mut self) {
        let visit = self.visit();
        let home = home::build_with(&self.lib, &visit);
        let parts = self.recs_home_parts();
        let mut view = self.res.home(&home);
        parts.apply(&mut view, &mut self.res);
        let stages: Vec<_> = view.worlds.iter().filter_map(|w| w.release).collect();
        self.publish(|s| {
            s.home = view;
        });
        self.kick_hd_covers(stages);
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
        let view = self.res.world_view(&w, new_album);
        let stage = view.world.release;
        self.publish(|s| s.world = Some(view));
        self.kick_hd_covers(stage);
    }

    fn search(&mut self, q: String) {
        let trimmed = q.trim().to_owned();
        let sources = self.streaming.search.sources;
        if trimmed.is_empty() {
            self.search_streams("", Vec::new());
            self.publish(|s| {
                s.search = nikgob_view::SearchView {
                    sources,
                    ..Default::default()
                };
            });
            return;
        }
        // Library first (at once), then the services as they answer.
        let started = Instant::now();
        let r = if sources.library {
            self.lib.search(&trimmed, 20).unwrap_or_default()
        } else {
            nikgob_library::SearchResults::default()
        };
        log::debug!(
            target: "nikgob_runtime::streams",
            "search «{trimmed}»: library {} tracks, {} albums in {} ms",
            r.tracks.len(),
            r.releases.len(),
            started.elapsed().as_millis()
        );
        let lib_tracks = r.tracks.clone();
        let res = &mut self.res;
        let tracks = r.tracks.iter().map(|t| res.track(t)).collect();
        let releases = r.releases.iter().map(|x| res.release(x)).collect();
        let artists = r.artists.iter().map(|a| res.artist(a)).collect();
        let playlists = r.playlists.iter().map(|p| res.playlist(p)).collect();
        self.publish(|s| {
            s.search = nikgob_view::SearchView {
                query: q,
                tracks,
                releases,
                artists,
                playlists,
                searching: false,
                sources,
                // The services' sections and the merged answer stay until
                // their new answers.
                streams: std::mem::take(&mut s.search.streams),
                answer: std::mem::take(&mut s.search.answer),
                torrents: std::mem::take(&mut s.search.torrents),
            };
        });
        self.search_streams(&trimmed, lib_tracks);
        if sources.torrents && self.state.read().search.torrents.query != trimmed {
            self.search_torrents(trimmed);
        }
    }

    /// Liking an album = liking all its tracks; unliking unlikes them all.
    fn like_release(&mut self, id: nikgob_core::model::ReleaseId, on: bool) {
        let before = self.library_tracks();
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
            (true, true) if self.indexer.is_some() => format!("Альбом в Любимых · {what} · скачаю в фоне"),
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
            for r in [&mut s.home.long_unplayed, &mut s.home.recently_added, &mut s.search.releases] {
                r.iter_mut().filter(|c| c.id == id).for_each(|c| c.liked = on);
            }
            for c in &mut s.home.continue_listening {
                if let nikgob_view::ContinueCard::Release(c) = c
                    && c.id == id
                {
                    c.liked = on;
                }
            }
        });
        self.after_like_change(before, vec![id]);
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

    /// The tracks an album plays, in its order: the whole album, its songs
    /// that only stream too; under «Только скачанное» (of the home or the
    /// library) only its songs with a file when it has some and `filtered`
    /// (no song of it was picked to start from).
    fn release_tracks(&mut self, id: nikgob_core::model::ReleaseId, filtered: bool) -> Vec<TrackId> {
        let mut rows = self.log_err("альбом", self.lib.tracks_of_release_all(id)).unwrap_or_default();
        let only_local = {
            let p = &self.state.read().prefs;
            p.mixes_downloaded_only || p.library_only_downloaded
        };
        if only_local && filtered && rows.iter().any(|r| r.has_local) {
            rows.retain(|r| r.has_local);
        }
        rows.iter().map(|r| r.id).collect()
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
        let same = match before {
            Held::Same => true,
            Held::Tracks(Some(n)) => self.library_tracks() == Held::Tracks(Some(n)),
            Held::Tracks(None) => false,
        };
        if !same || !self.state.read().library.loaded {
            self.load_library();
            return;
        }
        self.stream_analysis_changed();
        self.recs_library_changed();
        let playlists = self.log_err("плейлисты", self.lib.list_playlists()).unwrap_or_default();
        let rows: Vec<_> =
            releases.iter().filter_map(|id| self.lib.get_release_row(*id).ok().flatten()).collect();
        let res = &mut self.res;
        let playlists: Vec<_> = playlists.iter().map(|p| res.playlist(p)).collect();
        let cards: Vec<_> = rows.iter().map(|r| res.release(r)).collect();
        let liked = !cards.is_empty();
        self.publish(|s| {
            s.library.playlists = playlists;
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
        self.publish(|s| {
            let fix = |t: &mut nikgob_view::TrackRow| {
                if LikeTarget::Track(t.id) == target {
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
            for c in &mut s.home.continue_listening {
                if let nikgob_view::ContinueCard::Release(c) = c {
                    fix_release(c);
                }
            }
            s.home.long_unplayed.iter_mut().for_each(fix_release);
            s.home.recently_added.iter_mut().for_each(fix_release);
            if let Some(r) = s.release.as_mut() {
                fix_release(&mut r.card);
            }
            s.library.artists.iter_mut().for_each(fix_artist);
            s.search.artists.iter_mut().for_each(fix_artist);
            s.home.artist_mixes.iter_mut().for_each(fix_artist);
        });
        if let LikeTarget::Track(t) = target {
            let releases = self.releases_of(&[t]);
            self.after_like_change(before, releases);
        }
    }
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
                self.indexer = search::indexer_from(Some(&path));
                let usable = self.indexer.is_some();
                self.publish(|s| {
                    s.prefs.prowlarr_url = url;
                    s.prefs.prowlarr_configured = usable;
                });
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
        let mut cfg = MetadataConfig::new(settings.metadata.contact.clone());
        if let Some(k) = &settings.metadata.acoustid_key {
            cfg = cfg.with_acoustid_key(k.clone());
        }
        self.set_metadata_config(&cfg);
        let configured = settings.metadata.acoustid_key.is_some();
        self.publish(|s| {
            s.prefs.metadata_contact = settings.metadata.contact;
            s.prefs.acoustid_configured = configured;
        });
        self.toast("Сохранено", None);
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
