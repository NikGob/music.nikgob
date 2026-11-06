//! [`Soloist`]: the setup steps, the child's life, and the handle tracks
//! play through.
//!
//! Nothing is downloaded unless the user asks ([`SoloistAction::Download`]);
//! Soloist runs only while Spotify is switched on ([`Soloist::set_wanted`]),
//! it is set up (a build and the user's key) and not switched off. A run
//! that fails is started again after 2, 10 and 30 s, then waits for «Повторить».
//! An expired build is replaced by a fresh download once (Spotify publishes
//! builds well before they expire); a renewal in the last 30 days is
//! downloaded in the background and applied by the supervisor once nothing
//! has played for half a minute (or before Soloist starts anyway).
//!
//! On Windows a thread of its own keeps Soloist's audio session at its
//! level (COM calls never run on the engine's decoder thread), watches the
//! capture, and starts it again after a failure.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
#[cfg(windows)]
use std::sync::OnceLock;
#[cfg(windows)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use nikgob_core::streaming::{SoloistAction, SoloistPhase, SoloistStatus};
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;
use tokio::sync::{Notify, mpsc, watch};

use crate::client::{self, Live, Shared};
use crate::daemon::{self, Launch, Outcome};
use crate::install::{self, Build, CURRENT, PENDING};
use crate::protocol::{Command, Status};


/// Restarts after a failed run, then it waits for the user.
const BACKOFF: [Duration; 3] = [Duration::from_secs(2), Duration::from_secs(10), Duration::from_secs(30)];
/// The supervisor looks around this often anyway.
const TICK: Duration = Duration::from_secs(2);
/// Soloist's playback cache.
const CACHE_MB: u32 = 512;
/// The level of Soloist's own audio session while our engine plays its
/// captured sound: -120 dB, inaudible, yet process loopback (which takes the
/// session level as a plain float factor before it captures) gives a copy
/// that, scaled back by the same factor, is as clean as at full level.
/// Muting the session or a level of 0 would silence the capture as well
/// (`tests/loopback_mute.rs`).
pub const ROUTED_LEVEL: f32 = 1e-6;
/// A staged renewal goes in once nothing has played for this long.
const RENEW_IDLE: Duration = Duration::from_secs(30);
/// A run this long counts as stable: the restarts begin again from the
/// first delay.
const STABLE_RUN: Duration = Duration::from_secs(300);
/// While a track plays, the session level is set again this often (the
/// volume mixer, a new session on another output device).
#[cfg(windows)]
const REAPPLY: Duration = Duration::from_secs(2);
/// Soloist plays and the capture's frames stand still this long: it
/// stalled.
#[cfg_attr(not(windows), allow(dead_code))]
const STALL: Duration = Duration::from_millis(500);
/// ... or this long before the capture ever heard anything (Soloist may
/// take a moment to open its output).
#[cfg_attr(not(windows), allow(dead_code))]
const FIRST_SOUND: Duration = Duration::from_secs(2);
/// Waits before starting a capture again after it failed, then the last
/// one over and over.
#[cfg(windows)]
const CAPTURE_RETRY: [Duration; 4] =
    [Duration::from_secs(3), Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(60)];

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Where the user's key is kept (the Spotify grants' credential store).
pub trait KeyStore: Send + Sync + 'static {
    fn load(&self) -> BoxFuture<'_, Option<String>>;
    fn save(&self, key: String) -> BoxFuture<'_, Result<(), String>>;
    fn clear(&self);
}

pub struct Config {
    /// Soloist's own folder in the app's data dir: `builds/`, `data/`,
    /// `cache/`, `state.json`.
    pub root: PathBuf,
    /// The name Spotify's device list shows.
    pub device_name: String,
    pub user_agent: String,
}

/// Non-secret choices kept in `state.json`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// The user set it up and has not switched it off.
    #[serde(default)]
    enabled: bool,
    /// The one-time offer was answered.
    #[serde(default)]
    offer_answered: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Run {
    Idle,
    Starting,
    Running,
    /// Failed; started again at the instant, or by the user (`None`).
    Failed { why: String, retry_at: Option<Instant> },
}

struct State {
    saved: Saved,
    build: Option<Build>,
    key: Option<String>,
    key_loaded: bool,
    key_refused: bool,
    wanted: bool,
    downloading: Option<(u64, Option<u64>)>,
    download_error: Option<String>,
    size: Option<u64>,
    run: Run,
    failures: usize,
    /// When the current run started.
    running_since: Option<Instant>,
    /// Restart once the player is idle (a renewal is staged).
    restart_when_idle: bool,
    /// The user asked to stop the current run.
    stop: Option<watch::Sender<bool>>,
    expired_retry_done: bool,
    /// The app is quitting.
    closing: bool,
}

pub struct Soloist {
    me: Weak<Soloist>,
    rt: Handle,
    http: reqwest::Client,
    config: Config,
    keys: Arc<dyn KeyStore>,
    state: Mutex<State>,
    wake: Notify,
    live: Shared,
    commands: Mutex<Option<mpsc::UnboundedSender<Command>>>,
    pid: Mutex<Option<u32>>,
    #[cfg(windows)]
    route: Route,
    /// The user's volume, 0–1 (f32 bits).
    volume: AtomicU32,
    /// Bumped by every change, for the settings to notice.
    revision: AtomicU64,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// A track plays through Soloist (renewals wait).
    pub(crate) playing: AtomicU32,
    /// When a track last started or ended.
    busy_at: Mutex<Instant>,
    /// Bumped whenever Soloist's audio session should get its level again.
    session_want: AtomicU32,
    /// The `session_want` the level was last set (and read back) for.
    session_done: AtomicU32,
    /// The level Soloist's sessions were read back at after it was last
    /// set (f32 bits); NaN while not known.
    session_level: AtomicU32,
    /// The engine paused Soloist.
    user_paused: std::sync::atomic::AtomicBool,
    /// The CDN was asked for the download's size (once a run).
    size_asked: AtomicU32,
}

/// Soloist's capture and the thread that keeps its session (Windows).
#[cfg(windows)]
#[derive(Default)]
struct Route {
    /// The capture whose copy is heard.
    capture: Mutex<Option<Arc<crate::capture::ProcessCapture>>>,
    /// A capture started since (of that process id): it goes in as the next
    /// track is about to play ([`Soloist::routed`]), so the engine's volume,
    /// set for a track as it starts, matches where its sound comes from.
    spare: Mutex<Option<(u32, Arc<crate::capture::ProcessCapture>)>>,
    /// A capture is being started.
    starting: AtomicBool,
    retry: Mutex<Retry>,
    /// The session thread, woken when there is work.
    thread: OnceLock<std::thread::Thread>,
}

/// Starting the capture again after it failed.
#[cfg(windows)]
#[derive(Default)]
struct Retry {
    attempts: usize,
    /// Not before then (`None`: now).
    next_at: Option<Instant>,
}

#[cfg(windows)]
fn retry_delay(attempts: usize) -> Duration {
    CAPTURE_RETRY[attempts.min(CAPTURE_RETRY.len() - 1)]
}

/// A level read back is the level set (float rounding aside).
fn level_matches(got: f32, want: f32) -> bool {
    got.is_finite() && want.is_finite() && (got - want).abs() <= want.abs() * 0.01 + 1e-9
}

/// Watches a capture's frame count while Soloist plays.
#[derive(Debug, Default)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Watch {
    /// Which capture (its address).
    key: usize,
    frames: u64,
    /// Since when sound is expected and no new frames came.
    since: Option<Instant>,
    /// It captured anything at all.
    heard: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Watch {
    /// True once the frames stood still too long while sound was expected
    /// (Soloist plays a track of ours).
    fn stalled(&mut self, key: usize, frames: u64, expected: bool, now: Instant) -> bool {
        if key != self.key {
            *self = Self { key, frames, since: None, heard: frames > 0 };
        }
        if frames != self.frames {
            self.frames = frames;
            self.heard = true;
            self.since = Some(now);
            return false;
        }
        if !expected {
            self.since = None;
            return false;
        }
        let since = *self.since.get_or_insert(now);
        now.duration_since(since) >= if self.heard { STALL } else { FIRST_SOUND }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `music.nikgob (PC-NAME)`: what Spotify's device list shows.
pub fn default_device_name() -> String {
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|h| !h.trim().is_empty());
    match host {
        Some(h) => format!("music.nikgob ({})", h.trim()),
        None => "music.nikgob".to_owned(),
    }
}

impl Soloist {
    /// Creates the manager and its supervisor on `rt`. Reads the saved
    /// choices and the installed build; touches no network.
    pub fn new(config: Config, keys: Arc<dyn KeyStore>, rt: Handle) -> Arc<Self> {
        let saved: Saved = std::fs::read(config.root.join("state.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let builds = config.root.join("builds");
        let build = install::read_pointer(&builds, CURRENT);
        // A renewal staged by an earlier run goes in before Soloist starts.
        let staged = install::read_pointer_raw(&builds, PENDING).is_some();
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .https_only(true)
            .build()
            .unwrap_or_default();
        let me = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            rt: rt.clone(),
            http,
            config,
            keys,
            state: Mutex::new(State {
                saved,
                build,
                key: None,
                key_loaded: false,
                key_refused: false,
                wanted: false,
                downloading: None,
                download_error: None,
                size: None,
                run: Run::Idle,
                failures: 0,
                running_since: None,
                restart_when_idle: staged,
                stop: None,
                expired_retry_done: false,
                closing: false,
            }),
            wake: Notify::new(),
            live: Shared::default(),
            commands: Mutex::new(None),
            pid: Mutex::new(None),
            #[cfg(windows)]
            route: Route::default(),
            volume: AtomicU32::new(1.0f32.to_bits()),
            revision: AtomicU64::new(0),
            on_change: Mutex::new(None),
            playing: AtomicU32::new(0),
            busy_at: Mutex::new(Instant::now()),
            session_want: AtomicU32::new(1),
            session_done: AtomicU32::new(0),
            session_level: AtomicU32::new(f32::NAN.to_bits()),
            user_paused: std::sync::atomic::AtomicBool::new(false),
            size_asked: AtomicU32::new(0),
        });
        let s = me.clone();
        rt.spawn(async move { s.supervise().await });
        #[cfg(windows)]
        {
            let weak = Arc::downgrade(&me);
            match std::thread::Builder::new()
                .name("soloist-session".into())
                .spawn(move || session_loop(&weak))
            {
                Ok(t) => {
                    let _ = me.route.thread.set(t.thread().clone());
                }
                Err(e) => log::warn!("soloist: no session thread: {e}"),
            }
        }
        me
    }

    /// Called (from any thread) whenever the status may have changed.
    pub fn on_change(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.on_change) = Some(f);
    }

    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        let f = lock(&self.on_change).clone();
        if let Some(f) = f {
            f();
        }
    }

    fn builds(&self) -> PathBuf {
        self.config.root.join("builds")
    }

    fn save(&self, saved: &Saved) {
        let path = self.config.root.join("state.json");
        let _ = std::fs::create_dir_all(&self.config.root);
        if let Ok(b) = serde_json::to_vec_pretty(saved) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    pub fn status(&self) -> SoloistStatus {
        let st = lock(&self.state);
        let live = client::lock(&self.live);
        let phase = if install::artifact().is_none() {
            SoloistPhase::Unsupported
        } else if let Some((done, total)) = st.downloading {
            SoloistPhase::Downloading { done, total }
        } else if let Some(e) = &st.download_error {
            SoloistPhase::Failed(e.clone())
        } else if st.build.is_none() {
            SoloistPhase::Absent
        } else if st.key_loaded && (st.key.is_none() || st.key_refused) {
            SoloistPhase::NeedsKey
        } else if !st.saved.enabled || !st.wanted {
            SoloistPhase::Off
        } else {
            match &st.run {
                Run::Failed { why, .. } => SoloistPhase::Failed(why.clone()),
                Run::Running if live.connected && live.logged_in => SoloistPhase::Ready,
                Run::Running if live.connected => SoloistPhase::Pairing,
                _ => SoloistPhase::Starting,
            }
        };
        SoloistStatus {
            phase,
            device_name: self.config.device_name.clone(),
            size: st.size,
            expires_at: st.build.as_ref().map(Build::expires),
            has_key: st.key.is_some(),
            offer: !st.saved.offer_answered && st.build.is_none() && st.downloading.is_none(),
        }
    }

    /// Spotify's sound can come from Soloist right now.
    pub fn ready(&self) -> bool {
        self.status().ready()
    }

    /// Spotify is switched on in the settings (or not).
    pub fn set_wanted(&self, on: bool) {
        let mut st = lock(&self.state);
        if st.wanted != on && !st.closing {
            st.wanted = on;
            drop(st);
            self.wake.notify_one();
            self.changed();
        }
    }

    pub fn act(&self, action: SoloistAction) {
        log::info!("soloist: {action:?}");
        let mut st = lock(&self.state);
        match action {
            SoloistAction::Download => {
                st.saved.offer_answered = true;
                st.saved.enabled = true;
                st.download_error = None;
                let saved = st.saved.clone();
                let start = st.downloading.is_none() && st.build.is_none();
                drop(st);
                self.save(&saved);
                if start && let Some(me) = self.me.upgrade() {
                    self.rt.spawn(async move { me.download_now().await });
                }
            }
            SoloistAction::SetKey(key) => {
                let key = key.trim().to_owned();
                if key.is_empty() || key.len() > 4096 || !key.bytes().all(|b| b.is_ascii_graphic()) {
                    return;
                }
                st.key = Some(key.clone());
                st.key_loaded = true;
                st.key_refused = false;
                st.saved.enabled = true;
                st.failures = 0;
                if matches!(st.run, Run::Failed { .. }) {
                    st.run = Run::Idle;
                }
                let saved = st.saved.clone();
                drop(st);
                self.save(&saved);
                if let Some(me) = self.me.upgrade() {
                    self.rt.spawn(async move {
                        if let Err(e) = me.keys.save(key).await {
                            log::warn!("soloist: the key was not saved: {e}");
                        }
                    });
                }
            }
            SoloistAction::Enable | SoloistAction::Retry => {
                st.saved.enabled = true;
                st.failures = 0;
                // A download that failed is tried again (it was asked for).
                let download = st.download_error.take().is_some() && st.build.is_none() && st.downloading.is_none();
                st.expired_retry_done = false;
                if matches!(st.run, Run::Failed { .. }) {
                    st.run = Run::Idle;
                }
                let saved = st.saved.clone();
                drop(st);
                self.save(&saved);
                if download && let Some(me) = self.me.upgrade() {
                    self.rt.spawn(async move { me.download_now().await });
                }
            }
            SoloistAction::Disable => {
                st.saved.enabled = false;
                let saved = st.saved.clone();
                if let Some(stop) = st.stop.take() {
                    let _ = stop.send(true);
                }
                drop(st);
                self.save(&saved);
            }
            SoloistAction::AnswerOffer => {
                st.saved.offer_answered = true;
                let saved = st.saved.clone();
                drop(st);
                self.save(&saved);
            }
        }
        self.wake.notify_one();
        self.changed();
    }

    /// Forgets the key (Spotify signed out).
    pub fn forget_key(&self) {
        {
            let mut st = lock(&self.state);
            st.key = None;
            st.key_loaded = true;
            if let Some(stop) = st.stop.take() {
                let _ = stop.send(true);
            }
        }
        self.keys.clear();
        self.wake.notify_one();
        self.changed();
    }

    /// Stops the child (the app is quitting); waits a moment for it.
    pub fn shutdown(&self) {
        let stop = {
            let mut st = lock(&self.state);
            // Nothing starts it again.
            st.wanted = false;
            st.closing = true;
            st.stop.take()
        };
        if let Some(stop) = stop {
            let _ = stop.send(true);
            let until = Instant::now() + Duration::from_secs(2);
            while lock(&self.pid).is_some() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// The size of the download, asked of the CDN once (for the dialog).
    pub fn ask_size(&self) {
        if self.size_asked.swap(1, Ordering::AcqRel) != 0 || lock(&self.state).size.is_some() {
            return;
        }
        let Some(me) = self.me.upgrade() else { return };
        self.rt.spawn(async move {
            if let Some(n) = install::archive_size(&me.http).await {
                lock(&me.state).size = Some(n);
                me.changed();
            }
        });
    }

    async fn download_now(self: Arc<Self>) {
        lock(&self.state).downloading = Some((0, None));
        self.changed();
        let me = self.clone();
        let progress = move |done: u64, total: Option<u64>| {
            let mut st = lock(&me.state);
            st.downloading = Some((done, total));
            if total.is_some() {
                st.size = total;
            }
            drop(st);
            me.changed();
        };
        let result = install::download(&self.builds(), &self.http, &progress, "download").await;
        let mut st = lock(&self.state);
        st.downloading = None;
        match result {
            Ok(build) => {
                log::info!("soloist: downloaded {} (built {})", build.version, build.built);
                if let Err(e) = install::write_pointer(&self.builds(), CURRENT, &build) {
                    st.download_error = Some(format!("Не сохранилось: {e}"));
                } else {
                    // A staged renewal keeps its folder.
                    let pending = install::read_pointer_raw(&self.builds(), PENDING).map(|b| b.digest);
                    let mut keep = vec![build.digest.as_str()];
                    keep.extend(pending.as_deref());
                    install::collect(&self.builds(), &keep);
                    st.build = Some(build);
                    st.expired_retry_done = false;
                    if matches!(st.run, Run::Failed { .. }) {
                        st.run = Run::Idle;
                    }
                }
            }
            Err(e) => {
                log::warn!("soloist: download failed: {e}");
                st.download_error = Some(capitalised(&e.to_string()));
            }
        }
        drop(st);
        self.wake.notify_one();
        self.changed();
    }

    /// Supervises: starts and stops the child, restarts it after failures,
    /// renews the build. Runs for the life of the app.
    async fn supervise(self: Arc<Self>) {
        // The key from the credential store, once.
        let key = self.keys.load().await;
        {
            let mut st = lock(&self.state);
            if st.key.is_none() {
                st.key = key;
            }
            st.key_loaded = true;
        }
        self.changed();
        let mut renewal: Option<tokio::task::JoinHandle<()>> = None;
        loop {
            // A staged renewal: before Soloist starts, or once nothing has
            // played through it for a while (never between two tracks).
            let (staged, running) = {
                let st = lock(&self.state);
                (st.restart_when_idle, st.stop.is_some())
            };
            if staged && (!running || self.quiet_for().is_some_and(|d| d >= RENEW_IDLE)) {
                self.apply_pending(running).await;
            }
            {
                let mut st = lock(&self.state);
                if st.failures > 0 && st.running_since.is_some_and(|t| t.elapsed() >= STABLE_RUN) {
                    log::info!("soloist: running steadily again");
                    st.failures = 0;
                }
            }
            let launch = {
                let mut st = lock(&self.state);
                let set_up = st.build.is_some() && st.key.is_some() && !st.key_refused;
                let should = set_up && st.saved.enabled && st.wanted && st.downloading.is_none();
                let due = match &st.run {
                    Run::Idle => true,
                    Run::Failed { retry_at: Some(at), .. } => Instant::now() >= *at,
                    _ => false,
                };
                if should && due && st.stop.is_none() {
                    let (tx, rx) = watch::channel(false);
                    st.stop = Some(tx);
                    st.run = Run::Starting;
                    let build = st.build.clone().expect("checked");
                    Some((self.launch(&build, st.key.clone().unwrap_or_default()), rx, build))
                } else {
                    if !should && let Some(stop) = st.stop.take() {
                        let _ = stop.send(true);
                    }
                    None
                }
            };
            if let Some((launch, stop, build)) = launch {
                self.changed();
                let me = self.clone();
                self.rt.spawn(async move { me.run_once(launch, stop, build).await });
            }
            // A renewal in the last 30 days, once a day.
            let due = {
                let st = lock(&self.state);
                st.build.as_ref().is_some_and(|b| b.renewal_due(install::now())) && st.downloading.is_none()
            };
            if due && renewal.as_ref().is_none_or(tokio::task::JoinHandle::is_finished) {
                let me = self.clone();
                renewal = Some(self.rt.spawn(async move { me.renew().await }));
            }
            tokio::select! {
                () = self.wake.notified() => {}
                () = tokio::time::sleep(TICK) => {}
            }
        }
    }

    fn launch(&self, build: &Build, key: String) -> Launch {
        let volume = f32::from_bits(self.volume.load(Ordering::Relaxed));
        Launch {
            exe: build.exe(&self.builds()),
            builds: self.builds(),
            data_dir: self.config.root.join("data"),
            cache_dir: self.config.root.join("cache"),
            device_name: self.config.device_name.clone(),
            key,
            // Our volume is applied to its audio session (Windows) or by our
            // engine; Soloist itself plays at full level, unless there is no
            // other control.
            initial_volume: if cfg!(windows) { 100 } else { (volume * 100.0).round() as u8 },
            cache_mb: CACHE_MB,
            pipewire_device: None,
        }
    }

    async fn run_once(self: Arc<Self>, launch: Launch, stop: watch::Receiver<bool>, build: Build) {
        let me = self.clone();
        let hooks = daemon::Hooks {
            process: Box::new({
                let me = self.clone();
                move |pid| me.set_process(pid)
            }),
            commands: Box::new({
                let me = self.clone();
                move |tx| {
                    let up = tx.is_some();
                    *lock(&me.commands) = tx;
                    if up {
                        me.after_connect();
                    }
                }
            }),
            changed: Arc::new(move || me.on_live_change()),
        };
        {
            let mut st = lock(&self.state);
            if st.stop.is_some() {
                st.run = Run::Running;
                st.running_since = Some(Instant::now());
            }
        }
        self.changed();
        let outcome = daemon::run(&launch, &self.live, &hooks, stop).await;
        let refresh = {
            let mut st = lock(&self.state);
            st.stop = None;
            st.running_since = None;
            // A staged renewal stays staged: it goes in before the next start.
            match outcome {
                Outcome::Stopped => {
                    st.run = Run::Idle;
                    false
                }
                Outcome::BadKey => {
                    st.key_refused = true;
                    st.run = Run::Idle;
                    false
                }
                Outcome::Expired if !st.expired_retry_done => {
                    st.expired_retry_done = true;
                    st.run = Run::Idle;
                    st.build = None;
                    install::remove_pointer(&self.builds(), CURRENT);
                    true
                }
                Outcome::Expired => {
                    st.run = Run::Failed { why: "Сборка Soloist устарела, новой пока нет".into(), retry_at: None };
                    false
                }
                Outcome::Failed(why) => {
                    log::warn!("soloist: {why}");
                    let retry_at = BACKOFF.get(st.failures).map(|d| Instant::now() + *d);
                    st.failures += 1;
                    st.run = Run::Failed { why: "Soloist остановился".into(), retry_at };
                    false
                }
            }
        };
        if refresh {
            log::info!("soloist: build {} expired, downloading a fresh one", build.version);
            self.clone().download_now().await;
        }
        self.wake.notify_one();
        self.changed();
    }

    /// Downloads a newer build and stages it; the supervisor switches to it
    /// when nothing plays.
    async fn renew(self: Arc<Self>) {
        let builds = self.builds();
        let result = install::download(&builds, &self.http, &|_, _| {}, "renew").await;
        let mut st = lock(&self.state);
        let Some(current) = st.build.clone() else { return };
        let mut checked = current.clone();
        checked.checked = install::now();
        match result {
            Ok(next) if next.digest != current.digest && next.built > current.built => {
                log::info!("soloist: renewal {} staged", next.version);
                let _ = install::write_pointer(&builds, PENDING, &next);
                st.restart_when_idle = true;
                let _ = install::write_pointer(&builds, CURRENT, &checked);
                st.build = Some(checked);
            }
            Ok(_) => {
                let _ = install::write_pointer(&builds, CURRENT, &checked);
                st.build = Some(checked);
            }
            Err(e) => log::info!("soloist: no renewal yet: {e}"),
        }
        drop(st);
        self.wake.notify_one();
    }

    /// How long nothing has played through Soloist (`None`: something
    /// plays, a track of ours or one the user started from another app).
    fn quiet_for(&self) -> Option<Duration> {
        if self.playing.load(Ordering::Acquire) > 0 || client::lock(&self.live).status == Status::Playing {
            return None;
        }
        Some(lock(&self.busy_at).elapsed())
    }

    /// Switches to the staged renewal (the supervisor only); a `running`
    /// Soloist is stopped for it, so only if it is still quiet.
    async fn apply_pending(&self, running: bool) {
        let builds = self.builds();
        let dir = builds.clone();
        // Reading the pointer hashes the executable: not on a worker's time.
        let next = tokio::task::spawn_blocking(move || install::read_pointer(&dir, PENDING))
            .await
            .ok()
            .flatten();
        let keep = {
            let mut st = lock(&self.state);
            let Some(next) = next else {
                log::info!("soloist: the staged renewal is gone or not usable");
                install::remove_pointer(&builds, PENDING);
                st.restart_when_idle = false;
                return;
            };
            if running && self.quiet_for().is_none_or(|d| d < RENEW_IDLE) {
                return;
            }
            if let Err(e) = install::write_pointer(&builds, CURRENT, &next) {
                // Tried again at the next start of the player.
                log::warn!("soloist: the renewal was not switched to: {e}");
                st.restart_when_idle = false;
                return;
            }
            install::remove_pointer(&builds, PENDING);
            log::info!("soloist: switching to {} (built {})", next.version, next.built);
            let keep = next.digest.clone();
            st.build = Some(next);
            st.restart_when_idle = false;
            if let Some(stop) = st.stop.take() {
                let _ = stop.send(true);
            }
            keep
        };
        let _ = tokio::task::spawn_blocking(move || install::collect(&builds, &[&keep])).await;
        self.wake.notify_one();
        self.changed();
    }

    fn set_process(&self, pid: Option<u32>) {
        *lock(&self.pid) = pid;
        #[cfg(windows)]
        {
            // Into the job first: however the player ends, Soloist ends too.
            if let Some(p) = pid {
                crate::job::adopt(p);
            }
            let old = (lock(&self.route.capture).take(), lock(&self.route.spare).take());
            *lock(&self.route.retry) = Retry::default();
            if let Some(p) = pid {
                self.start_capture(p);
            }
            if old.0.is_some() || old.1.is_some() {
                // A capture stops by joining its thread.
                self.rt.spawn_blocking(move || drop(old));
            }
        }
        self.session_invalidate();
    }

    fn after_connect(&self) {
        // Our queue decides what follows: Soloist must not repeat or
        // shuffle on its own.
        self.send(Command::SetRepeatTrack(false));
        self.send(Command::SetRepeatContext(false));
        self.send(Command::SetShuffle(false));
    }

    fn on_live_change(&self) {
        let (logged_in, volume) = {
            let l = client::lock(&self.live);
            (l.logged_in, l.volume)
        };
        // Windows: the level is set on its audio session; Soloist's own
        // stays at 100 so the session's is the only one.
        if cfg!(windows) && logged_in && volume.is_some_and(|v| v < 100) && !self.volume_reset_recently() {
            self.send(Command::SetVolume(100));
        }
        self.changed();
    }

    fn volume_reset_recently(&self) -> bool {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        let mut last = lock(&LAST);
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(2)) {
            return true;
        }
        *last = Some(Instant::now());
        false
    }

    /// Queues a command on the current connection.
    pub(crate) fn send(&self, c: Command) -> bool {
        lock(&self.commands).as_ref().is_some_and(|tx| tx.send(c).is_ok())
    }

    /// The running capture; one that failed is dropped, and Soloist's own
    /// output is made audible again.
    #[cfg(windows)]
    pub(crate) fn capture(&self) -> Option<Arc<crate::capture::ProcessCapture>> {
        let mut slot = lock(&self.route.capture);
        if let Some(e) = slot.as_ref().and_then(|c| c.failure()) {
            log::warn!("soloist: capture lost ({e}): it plays out loud itself");
            let old = slot.take();
            drop(slot);
            self.capture_lost();
            // Its thread has already ended: joining it is quick.
            drop(old);
            return None;
        }
        slot.clone()
    }

    /// Soloist's sound reaches the listener through our engine: its capture
    /// runs, and its own output is turned down to [`ROUTED_LEVEL`]. Without
    /// a capture (other systems, an older Windows, a failure) Soloist is
    /// heard itself and the engine plays its track at volume 0.
    ///
    /// Asked as a track is about to play (the engine sets its volume for
    /// the answer): a capture started since the last track goes in now, and
    /// only now, so the answer holds for the whole track (a capture can
    /// still be lost during it; its track then gives silence and Soloist is
    /// heard itself).
    pub fn routed(&self) -> bool {
        #[cfg(windows)]
        {
            let (promoted, discard) = {
                let mut slot = lock(&self.route.capture);
                if slot.is_some() {
                    (false, None)
                } else {
                    let spare = lock(&self.route.spare).take();
                    let pid = *lock(&self.pid);
                    match spare {
                        Some((p, c)) if Some(p) == pid && c.failure().is_none() => {
                            *slot = Some(c);
                            (true, None)
                        }
                        other => (false, other),
                    }
                }
            };
            if discard.is_some() {
                self.capture_lost();
                drop(discard);
            }
            if promoted {
                log::info!("soloist: its sound goes through the player");
                self.session_invalidate();
            }
            self.capture().is_some()
        }
        #[cfg(not(windows))]
        false
    }

    /// Whether the capture runs now (no capture goes in).
    fn routed_now(&self) -> bool {
        #[cfg(windows)]
        return self.capture().is_some();
        #[cfg(not(windows))]
        false
    }

    /// Starts capturing `pid` off the async workers (it may take seconds);
    /// the capture waits as the spare until a track is about to play.
    #[cfg(windows)]
    fn start_capture(&self, pid: u32) {
        let Some(me) = self.me.upgrade() else { return };
        if self.route.starting.swap(true, Ordering::AcqRel) {
            return;
        }
        self.rt.spawn_blocking(move || {
            let result = crate::capture::ProcessCapture::start(pid);
            let current = *lock(&me.pid) == Some(pid);
            match result {
                Ok(c) if current => *lock(&me.route.spare) = Some((pid, Arc::new(c))),
                Ok(c) => drop(c),
                Err(e) => {
                    log::info!("soloist: no capture of its sound: {e}");
                    if current {
                        me.retry_later();
                    }
                }
            }
            me.route.starting.store(false, Ordering::Release);
            me.kick();
        });
    }

    /// The next start of the capture waits a while.
    #[cfg(windows)]
    fn retry_later(&self) {
        let mut r = lock(&self.route.retry);
        r.next_at = Some(Instant::now() + retry_delay(r.attempts));
    }

    /// The capture is gone: Soloist is heard itself until a new one runs.
    #[cfg(windows)]
    fn capture_lost(&self) {
        self.retry_later();
        self.session_invalidate();
    }

    /// Starts the capture again when there is none and its wait is over.
    #[cfg(windows)]
    fn restart_capture_if_due(&self) {
        let Some(pid) = *lock(&self.pid) else { return };
        if self.route.starting.load(Ordering::Acquire) || lock(&self.route.capture).is_some() {
            return;
        }
        let failed = {
            let mut spare = lock(&self.route.spare);
            match spare.as_ref() {
                Some((p, c)) if *p == pid && c.failure().is_none() => return,
                Some(_) => spare.take(),
                None => None,
            }
        };
        if failed.is_some() {
            drop(failed);
            self.retry_later();
            return;
        }
        {
            let mut r = lock(&self.route.retry);
            if r.next_at.is_some_and(|t| Instant::now() < t) {
                return;
            }
            r.attempts += 1;
            r.next_at = None;
        }
        log::info!("soloist: starting its capture again");
        self.start_capture(pid);
    }

    /// Soloist plays a track of ours and the capture's frames stand still:
    /// the capture is dropped, Soloist is heard itself.
    #[cfg(windows)]
    fn watch_capture(&self, w: &mut Watch) {
        let Some(c) = lock(&self.route.capture).clone() else {
            *w = Watch::default();
            return;
        };
        let expected = self.playing.load(Ordering::Acquire) > 0 && client::lock(&self.live).status == Status::Playing;
        if w.stalled(Arc::as_ptr(&c) as usize, c.frames_captured(), expected, Instant::now()) {
            log::warn!("soloist: its capture stalled: it plays out loud itself");
            let old = {
                let mut slot = lock(&self.route.capture);
                if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, &c)) { slot.take() } else { None }
            };
            drop(c);
            if old.is_some() {
                self.capture_lost();
            }
            drop(old);
            *w = Watch::default();
        }
    }

    /// One look of the session thread; returns how long to wait for the
    /// next.
    #[cfg(windows)]
    fn session_pass(&self, watch: &mut Watch, applied_at: &mut Option<Instant>) -> Duration {
        let playing = self.playing.load(Ordering::Acquire) > 0;
        self.watch_capture(watch);
        self.restart_capture_if_due();
        if playing && self.session_clean() && applied_at.is_none_or(|t| t.elapsed() >= REAPPLY) {
            // The user may move Soloist in the volume mixer, or it may open
            // a new session (another output device).
            self.session_dirty();
        }
        if !self.session_clean() && self.apply_session_volume() {
            *applied_at = Some(Instant::now());
        }
        match (playing, self.session_clean()) {
            (true, false) => Duration::from_millis(50),
            (true, true) => Duration::from_millis(100),
            (false, false) => Duration::from_secs(1),
            (false, true) => Duration::from_secs(2),
        }
    }

    /// Wakes the session thread.
    fn kick(&self) {
        #[cfg(windows)]
        if let Some(t) = self.route.thread.get() {
            t.unpark();
        }
    }

    /// The user's volume (0–1, the engine's slider). Routed, the engine
    /// applies it; otherwise it becomes the level of Soloist's audio
    /// session through the engine's cubic curve (Windows), or Soloist's own
    /// volume.
    pub fn set_volume(&self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
        let old = f32::from_bits(self.volume.swap(v.to_bits(), Ordering::AcqRel));
        if (old - v).abs() < f32::EPSILON {
            return;
        }
        if cfg!(windows) {
            self.session_dirty();
            self.kick();
        } else {
            self.send(Command::SetVolume((v * 100.0).round() as u8));
        }
    }

    /// The level Soloist's audio session should be at: [`ROUTED_LEVEL`]
    /// while routed, else the user's volume through the engine's curve.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn target_level(&self) -> f32 {
        if self.routed_now() {
            return ROUTED_LEVEL;
        }
        let v = f32::from_bits(self.volume.load(Ordering::Relaxed));
        v * v * v
    }

    /// The level of Soloist's audio session, which the capture went through.
    /// Routed, it is [`ROUTED_LEVEL`] only once the session was read back at
    /// it; 0 (unknown: the track gives silence) until then.
    pub(crate) fn capture_gain(&self) -> f32 {
        if self.routed_now() {
            let level = f32::from_bits(self.session_level.load(Ordering::Acquire));
            return if level_matches(level, ROUTED_LEVEL) { ROUTED_LEVEL } else { 0.0 };
        }
        let v = f32::from_bits(self.volume.load(Ordering::Relaxed));
        if cfg!(windows) { v * v * v } else { 1.0 }
    }

    /// While a track starts: its session gets our level as soon as it
    /// appears (the session thread looks every 50 ms then; this only wakes
    /// it, the decoder thread never waits for COM).
    pub(crate) fn prepare_output(&self) {
        if !self.session_clean() {
            self.kick();
        }
    }

    /// The captured sound is louder than the level allows: the session is at
    /// another level (a new session, the volume mixer). The track gives
    /// silence until the level is set and read back again.
    pub(crate) fn session_wrong(&self) {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        {
            let mut last = lock(&LAST);
            if last.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                log::info!("soloist: its session is not at the routed level: set again");
                *last = Some(Instant::now());
            }
        }
        self.session_invalidate();
    }

    pub fn pause(&self) {
        self.user_paused.store(true, Ordering::Release);
        self.send(Command::Pause);
    }

    pub fn resume(&self) {
        self.user_paused.store(false, Ordering::Release);
        self.send(Command::Play(None));
    }

    /// A Spotify track (`id`: base62 or a URI) to give the engine.
    pub fn track(self: &Arc<Self>, id: &str, duration: Option<Duration>) -> crate::track::SoloistTrack {
        crate::track::SoloistTrack::new(self.clone(), crate::protocol::track_uri(id), duration)
    }

    /// The session should get its level again (what it had stays known).
    fn session_dirty(&self) {
        self.session_want.fetch_add(1, Ordering::AcqRel);
    }

    /// The session's level is not known any more (another route, another
    /// process, a level that does not fit the sound): set it again.
    fn session_invalidate(&self) {
        self.session_level.store(f32::NAN.to_bits(), Ordering::Release);
        self.session_dirty();
        self.kick();
    }

    fn session_clean(&self) -> bool {
        self.session_done.load(Ordering::Acquire) == self.session_want.load(Ordering::Acquire)
    }

    /// Sets Soloist's audio sessions to their level and reads it back (the
    /// session thread only); true once every session is at it. A session
    /// appears with Soloist's first sound: until then there is none to set.
    #[cfg(windows)]
    fn apply_session_volume(&self) -> bool {
        let Some(pid) = *lock(&self.pid) else { return false };
        let generation = self.session_want.load(Ordering::Acquire);
        let want = self.target_level();
        match crate::capture::set_sessions_read_back(&[pid], Some(false), Some(want)) {
            Ok(levels) if !levels.is_empty() => {
                let all = levels.iter().all(|l| level_matches(*l, want));
                self.session_level.store(if all { want } else { f32::NAN }.to_bits(), Ordering::Release);
                if all {
                    self.session_done.store(generation, Ordering::Release);
                } else {
                    log::debug!("soloist: session levels {levels:?} after setting {want}");
                }
                all
            }
            Ok(_) => false,
            Err(e) => {
                self.session_level.store(f32::NAN.to_bits(), Ordering::Release);
                log::debug!("soloist: session level: {e}");
                false
            }
        }
    }
}

/// The session thread: keeps Soloist's session at its level, watches the
/// capture and starts it again; ends with the manager.
#[cfg(windows)]
fn session_loop(me: &Weak<Soloist>) {
    let mut watch = Watch::default();
    let mut applied_at = None;
    loop {
        let Some(s) = me.upgrade() else { return };
        let wait = s.session_pass(&mut watch, &mut applied_at);
        drop(s);
        std::thread::park_timeout(wait);
    }
}

impl crate::track::Link for Soloist {
    fn snapshot(&self) -> LiveView {
        LiveView::of(&client::lock(&self.live))
    }

    fn send(&self, c: Command) -> bool {
        if matches!(c, Command::Play(Some(_))) {
            self.user_paused.store(false, Ordering::Release);
        }
        Soloist::send(self, c)
    }

    fn take_captured(&self, out: &mut [f32]) -> Option<usize> {
        #[cfg(windows)]
        return self.capture().map(|c| c.take(out));
        #[cfg(not(windows))]
        {
            let _ = out;
            None
        }
    }

    fn buffered_frames(&self) -> Option<usize> {
        #[cfg(windows)]
        return self.capture().map(|c| c.buffered_frames());
        #[cfg(not(windows))]
        None
    }

    fn clear_captured(&self) {
        #[cfg(windows)]
        if let Some(c) = self.capture() {
            c.clear();
        }
    }

    fn keep_captured(&self, frames: usize) {
        #[cfg(windows)]
        if let Some(c) = self.capture() {
            c.keep_newest(frames);
        }
        #[cfg(not(windows))]
        let _ = frames;
    }

    fn capture_gain(&self) -> f32 {
        Soloist::capture_gain(self)
    }

    fn paused_by_us(&self) -> bool {
        self.user_paused.load(Ordering::Acquire)
    }

    fn prepare_output(&self) {
        Soloist::prepare_output(self);
    }

    fn session_wrong(&self) {
        Soloist::session_wrong(self);
    }

    /// Only counts and wakes: a staged renewal waits for the supervisor.
    fn playing(&self, delta: i32) {
        *lock(&self.busy_at) = Instant::now();
        if delta > 0 {
            self.playing.fetch_add(1, Ordering::AcqRel);
            self.session_dirty();
            self.kick();
        } else {
            let _ = self.playing.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
        }
    }
}

/// A copy of what tracks look at: Soloist's state at one moment.
#[derive(Clone, Debug, Default)]
pub struct LiveView {
    pub ready: bool,
    pub active: bool,
    pub status: Status,
    pub uri: Option<String>,
    pub duration_ms: Option<u64>,
    pub anchor: Option<crate::protocol::Anchor>,
    /// When the position last came.
    pub anchor_at: Option<Instant>,
    pub track_changes: u64,
    pub error: Option<(String, Instant)>,
    /// The command the last refusal answered, when known.
    pub error_command: Option<&'static str>,
    /// When the last `play` was accepted.
    pub play_accepted: Option<Instant>,
}

impl LiveView {
    fn of(l: &Live) -> Self {
        Self {
            ready: l.connected && l.logged_in,
            active: l.active,
            status: l.status,
            uri: l.uri().map(str::to_owned),
            duration_ms: l.item.as_ref().and_then(|i| i.duration_ms),
            anchor: l.anchor,
            anchor_at: l.anchor_at,
            track_changes: l.track_changes,
            error: l.error.clone(),
            error_command: l.error_command,
            play_accepted: l.play_accepted,
        }
    }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_read_back_matches_only_the_level_set() {
        assert!(level_matches(ROUTED_LEVEL, ROUTED_LEVEL));
        assert!(level_matches(1.000_001e-6, ROUTED_LEVEL), "float rounding");
        assert!(!level_matches(1e-5, ROUTED_LEVEL));
        assert!(!level_matches(1.0, ROUTED_LEVEL), "the volume mixer moved it");
        assert!(!level_matches(f32::NAN, ROUTED_LEVEL), "not known");
        assert!(level_matches(0.0, 0.0));
        assert!(level_matches(0.125, 0.125));
    }

    #[test]
    fn a_capture_stalls_only_while_soloist_plays() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut w = Watch::default();
        // A new capture that heard nothing yet gets longer.
        assert!(!w.stalled(1, 0, true, ms(0)));
        assert!(!w.stalled(1, 0, true, ms(1500)));
        assert!(w.stalled(1, 0, true, ms(2000)));
        // Frames come: the clock starts again, and 500 ms is enough now.
        assert!(!w.stalled(1, 480, true, ms(2010)));
        assert!(!w.stalled(1, 960, true, ms(2020)));
        assert!(!w.stalled(1, 960, true, ms(2300)));
        assert!(w.stalled(1, 960, true, ms(2520)));
        // Paused: no sound is expected, and the wait starts over on playing.
        assert!(!w.stalled(1, 960, false, ms(5000)));
        assert!(!w.stalled(1, 960, true, ms(5100)));
        assert!(!w.stalled(1, 960, true, ms(5500)));
        assert!(w.stalled(1, 960, true, ms(5600)));
        // Another capture starts afresh.
        assert!(!w.stalled(2, 0, true, ms(5700)));
        assert!(!w.stalled(2, 0, true, ms(6200)));
    }
}
