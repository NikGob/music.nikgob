//! [`ScWidget`]: the widget host's life (started on the first track, kept
//! between tracks, closed after a few idle minutes), the state its events
//! build, and the router of its sound.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use nikgob_soloist::route::{Captured, Router, Sessions};

use crate::protocol::{Command, Event, FLAG};

/// The host closes after this long without a track.
const IDLE_QUIT: Duration = Duration::from_secs(300);
/// The control thread looks around this often anyway.
const TICK: Duration = Duration::from_secs(15);
/// How long the host gets to close by itself before it is killed.
const CLOSE_GRACE: Duration = Duration::from_secs(3);
/// Host runs that end without ever being ready, in a row, before the route
/// is given up for this session (no WebView2, a broken install).
const MAX_BAD_RUNS: u32 = 3;
/// How long a track's start may wait for the host's capture to be made.
const CAPTURE_WAIT: Duration = Duration::from_millis(600);
/// A position report this old no longer moves the position on (the widget
/// went quiet: it buffers, or it hangs).
const STALE: Duration = Duration::from_secs(2);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Config {
    /// This program (it runs itself with `--sc-widget`).
    pub exe: PathBuf,
    /// The webview's profile: its own folder, never the sign-in window's,
    /// so the widget plays signed out and its WebView2 browser process is
    /// not shared with any other window.
    pub profile: PathBuf,
}

/// What the widget does, as of its last events.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Playing,
    Paused,
}

/// The host's state, as tracks look at it.
#[derive(Clone, Debug, Default)]
pub struct Live {
    /// A host process runs.
    pub alive: bool,
    /// Its page is up (it took commands).
    pub host_ready: bool,
    /// The load the widget has ready (its events are about it).
    pub seq: u64,
    pub status: Status,
    pub duration_ms: Option<u64>,
    /// The last position report of `seq` and when it came.
    pub position: Option<(u64, Instant)>,
    /// The load that played to its end.
    pub finished: u64,
    /// A load's error.
    pub error: Option<(u64, String)>,
    /// Loads up to this one failed with the host (it ended, or cannot play).
    pub lost: Option<(u64, String)>,
}

impl Live {
    /// Why load `seq` failed, if it did.
    pub fn failure(&self, seq: u64) -> Option<&str> {
        if let Some((g, why)) = &self.error
            && *g == seq
        {
            return Some(why);
        }
        match &self.lost {
            Some((upto, why)) if seq <= *upto => Some(why),
            _ => None,
        }
    }

    /// The widget's position of load `seq` now, in ms: the last report,
    /// moved on while it plays (for a short while only).
    pub fn position_ms(&self, seq: u64) -> Option<u64> {
        if self.seq != seq {
            return None;
        }
        let (ms, at) = self.position?;
        if self.status != Status::Playing {
            return Some(ms);
        }
        Some(ms + at.elapsed().min(STALE).as_millis() as u64)
    }

    /// The widget makes sound: it plays and its position moved lately.
    pub fn sounding(&self) -> bool {
        self.status == Status::Playing && self.position.is_some_and(|(ms, at)| ms > 0 && at.elapsed() < STALE)
    }
}

enum Ctl {
    /// Starts the host if none runs; answers once it was started (or not).
    Start(Option<SyncSender<bool>>),
    Send(Command),
    /// The host's stdout ended (run number).
    Exited(u64),
    /// The host cannot play anything: it is closed.
    Close(String),
    /// The app quits.
    Quit,
}

struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    run: u64,
    /// When it started.
    started: Instant,
}

/// SoundCloud's official player, for tracks only streamed with DRM.
pub struct ScWidget {
    me: Weak<ScWidget>,
    config: Config,
    router: Arc<Router>,
    live: Arc<Mutex<Live>>,
    ctl: Mutex<Option<Sender<Ctl>>>,
    /// The last load number given out.
    seq: AtomicU64,
    /// The engine paused the widget.
    user_paused: AtomicBool,
    /// Given up on for this session.
    broken: AtomicBool,
}

impl ScWidget {
    /// Creates the manager; nothing runs until the first track.
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new_cyclic(|me: &Weak<Self>| {
            let live = Arc::new(Mutex::new(Live::default()));
            let sounding = {
                let live = live.clone();
                Box::new(move || lock(&live).sounding()) as Box<dyn Fn() -> bool + Send + Sync>
            };
            Self {
                me: me.clone(),
                config,
                router: Router::new("sc widget", Sessions::Tree, sounding),
                live,
                ctl: Mutex::new(None),
                seq: AtomicU64::new(0),
                user_paused: AtomicBool::new(false),
                broken: AtomicBool::new(false),
            }
        })
    }

    /// Whether DRM tracks can go to the widget now.
    pub fn usable(&self) -> bool {
        !self.broken.load(Ordering::Acquire)
    }

    pub fn view(&self) -> Live {
        lock(&self.live).clone()
    }

    /// The control thread's channel (started on first use).
    fn control(&self) -> Option<Sender<Ctl>> {
        let mut slot = lock(&self.ctl);
        if let Some(tx) = slot.as_ref() {
            return Some(tx.clone());
        }
        let me = self.me.clone();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new().name("sc-widget".into()).spawn({
            let tx = tx.clone();
            move || control_loop(&me, &rx, &tx)
        });
        if let Err(e) = spawned {
            log::warn!("sc widget: no control thread: {e}");
            return None;
        }
        *slot = Some(tx.clone());
        Some(tx)
    }

    fn post(&self, c: Ctl) {
        if let Some(tx) = self.control() {
            let _ = tx.send(c);
        }
    }

    pub(crate) fn send(&self, c: Command) {
        self.post(Ctl::Send(c));
    }

    /// Starts the host in the background (a DRM track was just found, so
    /// its play is about to come).
    pub fn warm(&self) {
        if self.usable() {
            self.post(Ctl::Start(None));
        }
    }

    /// Before a track starts (on the runtime's thread): the host runs and
    /// its capture had a moment to be made, so [`ScWidget::routed`] can
    /// answer for this track. Waits a second at most, only when the host
    /// was not running.
    pub fn prepare(&self) {
        if !self.usable() || lock(&self.live).alive {
            return;
        }
        let Some(tx) = self.control() else { return };
        let (ack, started) = mpsc::sync_channel(1);
        if tx.send(Ctl::Start(Some(ack))).is_err() {
            return;
        }
        if started.recv_timeout(Duration::from_secs(1)) != Ok(true) {
            return;
        }
        let until = Instant::now() + CAPTURE_WAIT;
        while self.router.capture_starting() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The widget's sound goes through our engine (see
    /// [`Router::routed`]): its own output is turned down. Otherwise it is
    /// heard itself and the engine plays its track at volume 0.
    pub fn routed(&self) -> bool {
        self.router.routed()
    }

    /// The user's volume (0–1): applied by the engine while routed, else
    /// the level of the widget's audio sessions.
    pub fn set_volume(&self, v: f32) {
        self.router.set_volume(v);
    }

    pub fn pause(&self) {
        self.user_paused.store(true, Ordering::Release);
        self.send(Command::Pause { seq: self.seq.load(Ordering::Acquire) });
    }

    pub fn resume(&self) {
        self.user_paused.store(false, Ordering::Release);
        self.send(Command::Play { seq: self.seq.load(Ordering::Acquire) });
    }

    /// A load of SoundCloud track `track`; returns its number and whether
    /// the host had to start for it (it takes a few seconds more).
    pub(crate) fn load(&self, track: u64) -> (u64, bool) {
        let seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
        let cold = !lock(&self.live).host_ready;
        self.user_paused.store(false, Ordering::Release);
        self.send(Command::Load { seq, track });
        (seq, cold)
    }

    pub(crate) fn paused_by_us(&self) -> bool {
        self.user_paused.load(Ordering::Acquire)
    }

    pub(crate) fn playing(&self, delta: i32) {
        self.router.playing(delta);
    }

    pub(crate) fn prepare_output(&self) {
        self.router.prepare_output();
    }

    /// A SoundCloud track (`id`: its numeric id) for the engine.
    pub fn track(self: &Arc<Self>, id: &str, duration: Option<Duration>) -> crate::track::WidgetTrack {
        crate::track::WidgetTrack::new(self.clone(), id.trim().parse().ok(), duration)
    }

    /// Closes the host (the app is quitting).
    pub fn shutdown(&self) {
        if let Some(tx) = lock(&self.ctl).take() {
            let _ = tx.send(Ctl::Quit);
        }
    }

    fn on_event(&self, e: Event) {
        if let Event::Fail(why) = &e {
            self.post(Ctl::Close(why.clone()));
        }
        let mut l = lock(&self.live);
        match e {
            Event::Host => {
                l.host_ready = true;
                log::info!("sc widget: page up");
            }
            Event::Fail(why) => {
                log::warn!("sc widget: the host cannot play: {why}");
                l.lost = Some((self.seq.load(Ordering::Acquire), why));
            }
            Event::Ready { seq, duration_ms } => {
                log::info!("sc widget: load {seq} ready ({duration_ms:?} ms)");
                l.seq = seq;
                l.status = Status::Idle;
                l.duration_ms = duration_ms;
                l.position = None;
            }
            Event::Play { seq } if seq == l.seq => l.status = Status::Playing,
            Event::Pause { seq } if seq == l.seq => {
                l.status = Status::Paused;
                if let Some((ms, at)) = l.position {
                    // Where it stopped, not where it would have got to.
                    l.position = Some((ms + at.elapsed().min(STALE).as_millis() as u64, Instant::now()));
                }
            }
            Event::Progress { seq, ms } if seq == l.seq => {
                if l.status == Status::Idle {
                    l.status = Status::Playing;
                }
                l.position = Some((ms, Instant::now()));
            }
            Event::Finish { seq } if seq == l.seq => {
                log::info!("sc widget: load {seq} finished");
                l.finished = seq;
                l.status = Status::Idle;
            }
            Event::Error { seq, text } => {
                log::info!("sc widget: load {seq}: {text}");
                if seq > 0 {
                    l.error = Some((seq, text));
                }
            }
            _ => {}
        }
    }

    /// Starts the host process (the control thread only).
    fn spawn(&self, run: u64, tx: &Sender<Ctl>) -> Result<Host, String> {
        if let Err(e) = std::fs::create_dir_all(&self.config.profile) {
            return Err(format!("{}: {e}", self.config.profile.display()));
        }
        let mut child = Process::new(&self.config.exe)
            .arg(FLAG)
            .arg(&self.config.profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        let pid = child.id();
        log::info!("sc widget: host started (pid {pid})");
        // Into the job first: however the player ends, the host ends too.
        #[cfg(windows)]
        nikgob_soloist::job::adopt(pid);
        if let Some(out) = child.stdout.take() {
            let me = self.me.clone();
            let tx = tx.clone();
            let _ = std::thread::Builder::new().name("sc-widget-events".into()).spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let Some(me) = me.upgrade() else { return };
                    match Event::parse(&line) {
                        Some(e) => me.on_event(e),
                        None => log::debug!("sc widget: {line}"),
                    }
                }
                let _ = tx.send(Ctl::Exited(run));
            });
        }
        if let Some(err) = child.stderr.take() {
            let _ = std::thread::Builder::new().name("sc-widget-log".into()).spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    log::info!("[sc widget] {line}");
                }
            });
        }
        {
            let mut l = lock(&self.live);
            l.alive = true;
            l.host_ready = false;
            l.status = Status::Idle;
            l.position = None;
        }
        self.router.set_process(Some(pid));
        let stdin = child.stdin.take();
        Ok(Host { child, stdin, run, started: Instant::now() })
    }

    /// The host is gone (or is being closed): its loads fail, its sound is
    /// no longer routed.
    fn host_gone(&self, why: &str) {
        {
            let mut l = lock(&self.live);
            l.alive = false;
            l.host_ready = false;
            l.status = Status::Idle;
            l.position = None;
            l.lost = Some((self.seq.load(Ordering::Acquire), why.to_owned()));
        }
        self.router.set_process(None);
    }
}

/// Writes a command line to the host; false when its stdin is gone.
fn write_line(host: &mut Host, c: &Command) -> bool {
    let Some(stdin) = host.stdin.as_mut() else { return false };
    stdin.write_all(format!("{}\n", c.line()).as_bytes()).and_then(|()| stdin.flush()).is_ok()
}

/// Closes the host: its sessions back at full level, its stdin closed (it
/// exits by itself), killed if it does not.
fn close(me: &ScWidget, mut host: Host, why: &str) {
    me.router.restore_sessions();
    host.stdin = None;
    let until = Instant::now() + CLOSE_GRACE;
    loop {
        match host.child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) if Instant::now() >= until => {
                let _ = host.child.kill();
                let _ = host.child.wait();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    log::info!("sc widget: host closed ({why})");
    me.host_gone(why);
}

/// The control thread: starts the host, writes its commands, notices it
/// ending, closes it when idle or when the manager is dropped.
fn control_loop(me: &Weak<ScWidget>, rx: &Receiver<Ctl>, tx: &Sender<Ctl>) {
    let mut host: Option<Host> = None;
    let mut runs = 0u64;
    let mut bad_runs = 0u32;
    let mut last_busy = Instant::now();
    loop {
        let msg = rx.recv_timeout(TICK);
        let Some(w) = me.upgrade() else { break };
        if w.router.playing_count() > 0 {
            last_busy = Instant::now();
        }
        // Did it end by itself?
        if let Some(h) = host.as_mut()
            && matches!(h.child.try_wait(), Ok(Some(_)) | Err(_))
        {
            let h = host.take().expect("checked");
            note_end(&w, &h, &mut bad_runs);
            w.host_gone("the player host ended");
        }
        match msg {
            Ok(Ctl::Start(ack)) => {
                let ok = ensure(&w, &mut host, &mut runs, tx);
                last_busy = Instant::now();
                if let Some(a) = ack {
                    let _ = a.try_send(ok);
                }
            }
            Ok(Ctl::Send(c)) => {
                if matches!(c, Command::Load { .. }) {
                    last_busy = Instant::now();
                    if !ensure(&w, &mut host, &mut runs, tx) {
                        continue;
                    }
                }
                if let Some(h) = host.as_mut()
                    && !write_line(h, &c)
                {
                    let h = host.take().expect("checked");
                    note_end(&w, &h, &mut bad_runs);
                    close(&w, h, "its input closed");
                }
            }
            Ok(Ctl::Exited(run)) => {
                if host.as_ref().is_some_and(|h| h.run == run) {
                    let h = host.take().expect("checked");
                    note_end(&w, &h, &mut bad_runs);
                    close(&w, h, "it stopped answering");
                }
            }
            Ok(Ctl::Close(why)) => {
                if let Some(h) = host.take() {
                    note_end(&w, &h, &mut bad_runs);
                    close(&w, h, &why);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Ok(Ctl::Quit) | Err(RecvTimeoutError::Disconnected) => break,
        }
        if bad_runs >= MAX_BAD_RUNS && !w.broken.swap(true, Ordering::AcqRel) {
            log::warn!("sc widget: the player host never came up {bad_runs} times: off for this session");
        }
        if let Some(h) = host.take() {
            if w.router.playing_count() == 0 && last_busy.elapsed() >= IDLE_QUIT {
                close(&w, h, "idle");
            } else {
                host = Some(h);
            }
        }
    }
    // The manager is gone or quits.
    if let Some(h) = host.take() {
        match me.upgrade() {
            Some(w) => close(&w, h, "the player quits"),
            None => {
                let mut h = h;
                h.stdin = None;
                let _ = h.child.kill();
                let _ = h.child.wait();
            }
        }
    }
}

/// Counts a host run that ended without ever being ready.
fn note_end(me: &ScWidget, host: &Host, bad_runs: &mut u32) {
    if lock(&me.live).host_ready {
        *bad_runs = 0;
    } else if host.started.elapsed() < Duration::from_secs(60) {
        *bad_runs += 1;
    }
}

/// A host runs (started now if none did); false when it cannot start.
fn ensure(me: &ScWidget, host: &mut Option<Host>, runs: &mut u64, tx: &Sender<Ctl>) -> bool {
    if host.is_some() {
        return true;
    }
    if !me.usable() {
        return false;
    }
    *runs += 1;
    match me.spawn(*runs, tx) {
        Ok(h) => {
            *host = Some(h);
            true
        }
        Err(e) => {
            log::warn!("sc widget: the player host did not start: {e}");
            me.broken.store(true, Ordering::Release);
            me.host_gone("the player host did not start");
            false
        }
    }
}

impl Captured for ScWidget {
    fn take_captured(&self, out: &mut [f32]) -> Option<usize> {
        self.router.take_captured(out)
    }

    fn buffered_frames(&self) -> Option<usize> {
        self.router.buffered_frames()
    }

    fn clear_captured(&self) {
        self.router.clear_captured();
    }

    fn keep_captured(&self, frames: usize) {
        self.router.keep_captured(frames);
    }

    fn capture_gain(&self) -> f32 {
        self.router.capture_gain()
    }

    fn session_wrong(&self) {
        self.router.session_wrong();
    }
}
