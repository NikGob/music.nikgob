//! [`Soloist`]: the setup steps, the child's life, and the handle tracks
//! play through.
//!
//! Nothing is downloaded unless the user asks ([`SoloistAction::Download`]);
//! Soloist runs only while Spotify is switched on ([`Soloist::set_wanted`]),
//! it is set up (a build and the user's key) and not switched off. A run
//! that fails is started again after 2, 10 and 30 s, then waits for «Повторить».
//! An expired build is replaced by a fresh download once (Spotify publishes
//! builds well before they expire); a renewal in the last 30 days is
//! downloaded in the background and applied when nothing plays.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
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
    /// Restart once the player is idle (a renewal is staged).
    restart_when_idle: bool,
    /// The user asked to stop the current run.
    stop: Option<watch::Sender<bool>>,
    expired_retry_done: bool,
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
    capture: Mutex<Option<Arc<crate::capture::ProcessCapture>>>,
    /// The user's volume, 0–1 (f32 bits).
    volume: AtomicU32,
    /// Bumped by every change, for the settings to notice.
    revision: AtomicU64,
    on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// A track plays through Soloist (renewals wait).
    pub(crate) playing: AtomicU32,
    /// Soloist's audio session has our level.
    session_applied: AtomicU32,
    /// The engine paused Soloist.
    user_paused: std::sync::atomic::AtomicBool,
    /// The CDN was asked for the download's size (once a run).
    size_asked: AtomicU32,
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
                restart_when_idle: false,
                stop: None,
                expired_retry_done: false,
            }),
            wake: Notify::new(),
            live: Shared::default(),
            commands: Mutex::new(None),
            pid: Mutex::new(None),
            #[cfg(windows)]
            capture: Mutex::new(None),
            volume: AtomicU32::new(1.0f32.to_bits()),
            revision: AtomicU64::new(0),
            on_change: Mutex::new(None),
            playing: AtomicU32::new(0),
            session_applied: AtomicU32::new(0),
            user_paused: std::sync::atomic::AtomicBool::new(false),
            size_asked: AtomicU32::new(0),
        });
        let s = me.clone();
        rt.spawn(async move { s.supervise().await });
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
        if st.wanted != on {
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
                st.download_error = None;
                st.expired_retry_done = false;
                if matches!(st.run, Run::Failed { .. }) {
                    st.run = Run::Idle;
                }
                let saved = st.saved.clone();
                drop(st);
                self.save(&saved);
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
        let stop = lock(&self.state).stop.take();
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
        let result = install::download(&self.builds(), &self.http, &progress).await;
        let mut st = lock(&self.state);
        st.downloading = None;
        match result {
            Ok(build) => {
                log::info!("soloist: downloaded {} (built {})", build.version, build.built);
                if let Err(e) = install::write_pointer(&self.builds(), CURRENT, &build) {
                    st.download_error = Some(format!("Не сохранилось: {e}"));
                } else {
                    install::collect(&self.builds(), &[&build.digest]);
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
            self.apply_session_volume();
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
            }
        }
        self.changed();
        let outcome = daemon::run(&launch, &self.live, &hooks, stop).await;
        let refresh = {
            let mut st = lock(&self.state);
            st.stop = None;
            st.restart_when_idle = false;
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

    /// Downloads a newer build and switches to it when nothing plays.
    async fn renew(self: Arc<Self>) {
        let builds = self.builds();
        let result = install::download(&builds, &self.http, &|_, _| {}).await;
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
        self.apply_pending_when_idle();
    }

    /// Switches to a staged renewal if nothing plays through Soloist.
    fn apply_pending_when_idle(&self) {
        if self.playing.load(Ordering::Acquire) > 0 {
            return;
        }
        let builds = self.builds();
        let Some(next) = install::read_pointer(&builds, PENDING) else { return };
        let mut st = lock(&self.state);
        if install::write_pointer(&builds, CURRENT, &next).is_err() {
            return;
        }
        install::remove_pointer(&builds, PENDING);
        let keep = next.digest.clone();
        st.build = Some(next);
        st.restart_when_idle = false;
        if let Some(stop) = st.stop.take() {
            let _ = stop.send(true);
        }
        drop(st);
        install::collect(&builds, &[&keep]);
        self.wake.notify_one();
    }

    fn set_process(&self, pid: Option<u32>) {
        *lock(&self.pid) = pid;
        #[cfg(windows)]
        {
            let capture = pid.and_then(|p| match crate::capture::ProcessCapture::start(p) {
                Ok(c) => Some(Arc::new(c)),
                Err(e) => {
                    log::info!("soloist: no capture of its sound: {e}");
                    None
                }
            });
            *lock(&self.capture) = capture;
            if let Some(p) = pid {
                crate::job::adopt(p);
            }
        }
        self.session_dirty();
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
        if logged_in {
            let mut st = lock(&self.state);
            if st.failures > 0 {
                st.failures = 0;
            }
        }
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

    #[cfg(windows)]
    pub(crate) fn capture(&self) -> Option<Arc<crate::capture::ProcessCapture>> {
        lock(&self.capture).clone()
    }

    /// The user's volume (0–1, the engine's slider). On Windows it becomes
    /// the level of Soloist's audio session (the engine's cubic curve);
    /// elsewhere Soloist's own volume.
    pub fn set_volume(&self, v: f32) {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
        let old = f32::from_bits(self.volume.swap(v.to_bits(), Ordering::AcqRel));
        if (old - v).abs() < f32::EPSILON {
            return;
        }
        if cfg!(windows) {
            self.session_dirty();
            self.apply_session_volume();
        } else {
            self.send(Command::SetVolume((v * 100.0).round() as u8));
        }
    }

    /// The gain the captured sound went through (the session level).
    pub(crate) fn capture_gain(&self) -> f32 {
        let v = f32::from_bits(self.volume.load(Ordering::Relaxed));
        if cfg!(windows) { v * v * v } else { 1.0 }
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

    fn session_dirty(&self) {
        self.session_applied.store(0, Ordering::Release);
    }

    /// Sets Soloist's audio session level until it has one to set (the
    /// session appears with its first sound).
    fn apply_session_volume(&self) {
        #[cfg(windows)]
        {
            if self.session_applied.load(Ordering::Acquire) != 0 {
                return;
            }
            let Some(pid) = *lock(&self.pid) else { return };
            let gain = self.capture_gain();
            match crate::capture::set_sessions(&[pid], Some(false), Some(gain)) {
                Ok(n) if n > 0 => self.session_applied.store(1, Ordering::Release),
                Ok(_) => {}
                Err(e) => log::debug!("soloist: session level: {e}"),
            }
        }
    }

    /// A track ended or started: a staged renewal may go in.
    pub(crate) fn track_done(&self) {
        if self.playing.load(Ordering::Acquire) == 0 && lock(&self.state).restart_when_idle {
            self.apply_pending_when_idle();
        }
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

    fn playing(&self, delta: i32) {
        if delta > 0 {
            self.playing.fetch_add(1, Ordering::AcqRel);
            self.session_dirty();
        } else {
            let _ = self.playing.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
            self.track_done();
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
    pub track_changes: u64,
    pub error: Option<(String, Instant)>,
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
            track_changes: l.track_changes,
            error: l.error.clone(),
        }
    }
}

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}
