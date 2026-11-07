//! Another process's sound routed through our engine: the parts Soloist and
//! SoundCloud's official player share.
//!
//! * [`Router`]: the capture of a process tree (Windows process loopback),
//!   its audio sessions kept at [`ROUTED_LEVEL`] while the copy is heard
//!   through the engine (or at the user's volume while it is not), the
//!   watchdog that drops a stalled capture so the process is heard itself,
//!   and the restarts of a capture that failed. A thread of its own does the
//!   COM calls; the engine's decoder thread never waits for them.
//! * [`Feed`]: a live track's sound for the engine, from the capture (the
//!   session level undone) or, without one, silence paced by the player's
//!   position.
//!
//! See the crate docs for why the level is 1e-6 and not a mute.

#[cfg(windows)]
use std::sync::OnceLock;
#[cfg(windows)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use nikgob_audio::PcmRead;

/// The format of the captured sound (Windows converts to it).
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;
/// The level of the routed process's audio sessions while our engine plays
/// its captured sound: -120 dB, inaudible, yet process loopback (which takes
/// the session level as a plain float factor before it captures) gives a
/// copy that, scaled back by the same factor, is as clean as at full level.
/// Muting the session or a level of 0 would silence the capture as well
/// (`tests/loopback_mute.rs`).
pub const ROUTED_LEVEL: f32 = 1e-6;
/// Captured sound held before the first frames go to the engine: enough
/// for the capture's 10 ms packets to arrive unevenly without the engine
/// running dry, little enough to keep the copy close to what is heard.
pub const PREBUFFER_MS: u64 = 40;
/// More than this much waiting means the clocks drifted (the process's
/// output and ours are different devices): it is cut back to the prebuffer.
pub const DRIFT_MS: u64 = 160;
/// A captured sample above this many times the session level cannot have
/// come through it (full scale is 1): the session is at another level.
const TOO_LOUD: f32 = 8.0;
/// While a track plays, the session level is set again this often (the
/// volume mixer, a new session on another output device).
#[cfg(windows)]
const REAPPLY: Duration = Duration::from_secs(2);
/// The process plays and the capture's frames stand still this long: it
/// stalled.
#[cfg_attr(not(windows), allow(dead_code))]
const STALL: Duration = Duration::from_millis(500);
/// ... or this long before the capture ever heard anything (the process may
/// take a moment to open its output).
#[cfg_attr(not(windows), allow(dead_code))]
const FIRST_SOUND: Duration = Duration::from_secs(2);
/// Waits before starting a capture again after it failed, then the last
/// one over and over.
#[cfg(windows)]
const CAPTURE_RETRY: [Duration; 4] =
    [Duration::from_secs(3), Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(60)];

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Which audio sessions carry the routed process's sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sessions {
    /// The process's own (Soloist renders itself).
    Own,
    /// Those of the process and every process it started (a WebView2's
    /// audio service is a grandchild of its host).
    Tree,
}

/// What a live track needs of the capture (a [`Router`]; a fake in tests).
pub trait Captured: Send + Sync {
    /// Moves captured samples into `out`; `None` when there is no capture.
    fn take_captured(&self, out: &mut [f32]) -> Option<usize>;
    fn buffered_frames(&self) -> Option<usize>;
    fn clear_captured(&self);
    /// Drops all but the newest `frames` captured frames.
    fn keep_captured(&self, frames: usize);
    /// The gain the capture went through after the process (its session
    /// level); 0 while that level is not known.
    fn capture_gain(&self) -> f32;
    /// The captured sound is louder than [`Captured::capture_gain`] allows:
    /// the session is not at that level, it is set again.
    fn session_wrong(&self) {}
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
pub(crate) fn level_matches(got: f32, want: f32) -> bool {
    got.is_finite() && want.is_finite() && (got - want).abs() <= want.abs() * 0.01 + 1e-9
}

/// Watches a capture's frame count while the process plays.
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
    /// (the process plays a track of ours).
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

/// The capture and the thread that keeps the sessions (Windows).
#[cfg(windows)]
#[derive(Default)]
struct Route {
    /// The capture whose copy is heard.
    capture: Mutex<Option<Arc<crate::capture::ProcessCapture>>>,
    /// A capture started since (of that process id): it goes in as the next
    /// track is about to play ([`Router::routed`]), so the engine's volume,
    /// set for a track as it starts, matches where its sound comes from.
    spare: Mutex<Option<(u32, Arc<crate::capture::ProcessCapture>)>>,
    /// A capture is being started.
    starting: AtomicBool,
    retry: Mutex<Retry>,
    /// The session thread, woken when there is work.
    thread: OnceLock<std::thread::Thread>,
}

/// One routed process: its capture, its sessions' level, the watchdog.
pub struct Router {
    #[cfg_attr(not(windows), allow(dead_code))]
    me: Weak<Router>,
    /// For the log ("soloist").
    name: &'static str,
    #[cfg_attr(not(windows), allow(dead_code))]
    sessions: Sessions,
    /// Whether the process plays a track of ours right now, as far as its
    /// owner knows (with [`Router::playing`] above 0, the capture should
    /// hear something).
    #[cfg_attr(not(windows), allow(dead_code))]
    sounding: Box<dyn Fn() -> bool + Send + Sync>,
    pid: Mutex<Option<u32>>,
    #[cfg(windows)]
    route: Route,
    /// The user's volume, 0–1 (f32 bits).
    volume: AtomicU32,
    /// Tracks playing through the process.
    playing: AtomicU32,
    /// Bumped whenever the sessions should get their level again.
    session_want: AtomicU32,
    /// The `session_want` the level was last set (and read back) for.
    session_done: AtomicU32,
    /// The level the sessions were read back at after it was last set (f32
    /// bits); NaN while not known.
    session_level: AtomicU32,
}

impl Router {
    /// A router with no process yet; `sounding` says whether the process
    /// plays (its owner's state). Starts the session thread (Windows).
    pub fn new(name: &'static str, sessions: Sessions, sounding: Box<dyn Fn() -> bool + Send + Sync>) -> Arc<Self> {
        let me = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            name,
            sessions,
            sounding,
            pid: Mutex::new(None),
            #[cfg(windows)]
            route: Route::default(),
            volume: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicU32::new(0),
            session_want: AtomicU32::new(1),
            session_done: AtomicU32::new(0),
            session_level: AtomicU32::new(f32::NAN.to_bits()),
        });
        #[cfg(windows)]
        {
            let weak = Arc::downgrade(&me);
            match std::thread::Builder::new()
                .name(format!("{name}-session"))
                .spawn(move || session_loop(&weak))
            {
                Ok(t) => {
                    let _ = me.route.thread.set(t.thread().clone());
                }
                Err(e) => log::warn!("{name}: no session thread: {e}"),
            }
        }
        me
    }

    /// The process routed now.
    pub fn pid(&self) -> Option<u32> {
        *lock(&self.pid)
    }

    /// The process whose sound is routed (`None`: it ended). Its capture
    /// starts in the background.
    pub fn set_process(&self, pid: Option<u32>) {
        *lock(&self.pid) = pid;
        #[cfg(windows)]
        {
            let old = (lock(&self.route.capture).take(), lock(&self.route.spare).take());
            *lock(&self.route.retry) = Retry::default();
            if let Some(p) = pid {
                self.start_capture(p);
            }
            if old.0.is_some() || old.1.is_some() {
                // A capture stops by joining its thread.
                let _ = std::thread::Builder::new()
                    .name(format!("{}-capture-drop", self.name))
                    .spawn(move || drop(old));
            }
        }
        self.session_invalidate();
    }

    /// Tracks playing through the process now.
    pub fn playing_count(&self) -> u32 {
        self.playing.load(Ordering::Acquire)
    }

    /// A track starts (+1) or is done (-1).
    pub fn playing(&self, delta: i32) {
        if delta > 0 {
            self.playing.fetch_add(1, Ordering::AcqRel);
            self.session_dirty();
            self.kick();
        } else {
            let _ = self.playing.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1));
        }
    }

    /// The running capture; one that failed is dropped, and the process's
    /// own output is made audible again.
    #[cfg(windows)]
    fn capture(&self) -> Option<Arc<crate::capture::ProcessCapture>> {
        let mut slot = lock(&self.route.capture);
        if let Some(e) = slot.as_ref().and_then(|c| c.failure()) {
            log::warn!("{}: capture lost ({e}): it plays out loud itself", self.name);
            let old = slot.take();
            drop(slot);
            self.capture_lost();
            // Its thread has already ended: joining it is quick.
            drop(old);
            return None;
        }
        slot.clone()
    }

    /// The process's sound reaches the listener through our engine: its
    /// capture runs, and its own output is turned down to [`ROUTED_LEVEL`].
    /// Without a capture (other systems, an older Windows, a failure) it is
    /// heard itself and the engine plays its track at volume 0.
    ///
    /// Asked as a track is about to play (the engine sets its volume for
    /// the answer): a capture started since the last track goes in now, and
    /// only now, so the answer holds for the whole track (a capture can
    /// still be lost during it; its track then gives silence and the
    /// process is heard itself).
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
                log::info!("{}: its sound goes through the player", self.name);
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

    /// Starts capturing `pid` off the caller's thread (it may take
    /// seconds); the capture waits as the spare until a track is about to
    /// play.
    #[cfg(windows)]
    fn start_capture(&self, pid: u32) {
        let Some(me) = self.me.upgrade() else { return };
        if self.route.starting.swap(true, Ordering::AcqRel) {
            return;
        }
        let spawned = std::thread::Builder::new().name(format!("{}-capture-start", self.name)).spawn({
            let me = me.clone();
            move || {
                let result = crate::capture::ProcessCapture::start(pid);
                let current = *lock(&me.pid) == Some(pid);
                match result {
                    Ok(c) if current => *lock(&me.route.spare) = Some((pid, Arc::new(c))),
                    Ok(c) => drop(c),
                    Err(e) => {
                        log::info!("{}: no capture of its sound: {e}", me.name);
                        if current {
                            me.retry_later();
                        }
                    }
                }
                me.route.starting.store(false, Ordering::Release);
                me.kick();
            }
        });
        if let Err(e) = spawned {
            log::warn!("{}: no thread to start its capture: {e}", self.name);
            me.route.starting.store(false, Ordering::Release);
            me.retry_later();
        }
    }

    /// A capture is being made right now (it takes milliseconds, rarely
    /// seconds).
    pub fn capture_starting(&self) -> bool {
        #[cfg(windows)]
        return self.route.starting.load(Ordering::Acquire);
        #[cfg(not(windows))]
        false
    }

    /// The next start of the capture waits a while.
    #[cfg(windows)]
    fn retry_later(&self) {
        let mut r = lock(&self.route.retry);
        r.next_at = Some(Instant::now() + retry_delay(r.attempts));
    }

    /// The capture is gone: the process is heard itself until a new one runs.
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
        log::info!("{}: starting its capture again", self.name);
        self.start_capture(pid);
    }

    /// The process plays a track of ours and the capture's frames stand
    /// still: the capture is dropped, the process is heard itself.
    #[cfg(windows)]
    fn watch_capture(&self, w: &mut Watch) {
        let Some(c) = lock(&self.route.capture).clone() else {
            *w = Watch::default();
            return;
        };
        let expected = self.playing.load(Ordering::Acquire) > 0 && (self.sounding)();
        if w.stalled(Arc::as_ptr(&c) as usize, c.frames_captured(), expected, Instant::now()) {
            log::warn!("{}: its capture stalled: it plays out loud itself", self.name);
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
            // The user may move the process in the volume mixer, or it may
            // open a new session (another output device).
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

    /// The user's volume (0–1, the engine's slider); returns whether it
    /// changed. Routed, the engine applies it; otherwise it becomes the level
    /// of the process's audio sessions through the engine's cubic curve
    /// (Windows). Elsewhere the owner passes it on to the process.
    pub fn set_volume(&self, v: f32) -> bool {
        let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 1.0 };
        let old = f32::from_bits(self.volume.swap(v.to_bits(), Ordering::AcqRel));
        if (old - v).abs() < f32::EPSILON {
            return false;
        }
        if cfg!(windows) {
            self.session_dirty();
            self.kick();
        }
        true
    }

    /// The user's volume, 0–1.
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    /// The level the sessions should be at: [`ROUTED_LEVEL`] while routed,
    /// else the user's volume through the engine's curve.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn target_level(&self) -> f32 {
        if self.routed_now() {
            return ROUTED_LEVEL;
        }
        let v = self.volume();
        v * v * v
    }

    /// The level of the process's audio sessions, which the capture went
    /// through. Routed, it is [`ROUTED_LEVEL`] only once the sessions were
    /// read back at it; 0 (unknown: the track gives silence) until then.
    pub fn capture_gain(&self) -> f32 {
        if self.routed_now() {
            let level = f32::from_bits(self.session_level.load(Ordering::Acquire));
            return if level_matches(level, ROUTED_LEVEL) { ROUTED_LEVEL } else { 0.0 };
        }
        let v = self.volume();
        if cfg!(windows) { v * v * v } else { 1.0 }
    }

    /// While a track starts: its session gets our level as soon as it
    /// appears (the session thread looks every 50 ms then; this only wakes
    /// it, the decoder thread never waits for COM).
    pub fn prepare_output(&self) {
        if !self.session_clean() {
            self.kick();
        }
    }

    /// The captured sound is louder than the level allows: the session is at
    /// another level (a new session, the volume mixer). The track gives
    /// silence until the level is set and read back again.
    pub fn session_wrong(&self) {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        {
            let mut last = lock(&LAST);
            if last.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                log::info!("{}: its session is not at the routed level: set again", self.name);
                *last = Some(Instant::now());
            }
        }
        self.session_invalidate();
    }

    /// The sessions should get their level again (what they had stays known).
    fn session_dirty(&self) {
        self.session_want.fetch_add(1, Ordering::AcqRel);
    }

    /// The sessions' level is not known any more (another route, another
    /// process, a level that does not fit the sound): set it again.
    fn session_invalidate(&self) {
        self.session_level.store(f32::NAN.to_bits(), Ordering::Release);
        self.session_dirty();
        self.kick();
    }

    fn session_clean(&self) -> bool {
        self.session_done.load(Ordering::Acquire) == self.session_want.load(Ordering::Acquire)
    }

    /// The processes whose sessions carry the sound.
    #[cfg(windows)]
    fn session_pids(&self, pid: u32) -> Vec<u32> {
        match self.sessions {
            Sessions::Own => vec![pid],
            Sessions::Tree => crate::capture::process_tree(pid),
        }
    }

    /// Sets the process's audio sessions to their level and reads it back
    /// (the session thread only); true once every session is at it. A
    /// session appears with the process's first sound: until then there is
    /// none to set.
    #[cfg(windows)]
    fn apply_session_volume(&self) -> bool {
        let Some(pid) = *lock(&self.pid) else { return false };
        let generation = self.session_want.load(Ordering::Acquire);
        let want = self.target_level();
        let pids = self.session_pids(pid);
        match crate::capture::set_sessions_read_back(&pids, Some(false), Some(want)) {
            Ok(levels) if !levels.is_empty() => {
                let all = levels.iter().all(|l| level_matches(*l, want));
                self.session_level.store(if all { want } else { f32::NAN }.to_bits(), Ordering::Release);
                if all {
                    self.session_done.store(generation, Ordering::Release);
                } else {
                    log::debug!("{}: session levels {levels:?} after setting {want}", self.name);
                }
                all
            }
            Ok(_) => false,
            Err(e) => {
                self.session_level.store(f32::NAN.to_bits(), Ordering::Release);
                log::debug!("{}: session level: {e}", self.name);
                false
            }
        }
    }

    /// Puts the process's sessions back at full level (before it goes away,
    /// so nothing is left near silent). Blocks for the COM calls.
    pub fn restore_sessions(&self) {
        #[cfg(windows)]
        if let Some(pid) = *lock(&self.pid) {
            let pids = self.session_pids(pid);
            if let Err(e) = crate::capture::set_sessions(&pids, Some(false), Some(1.0)) {
                log::debug!("{}: sessions not restored: {e}", self.name);
            }
        }
        self.session_invalidate();
    }
}

/// The session thread: keeps the sessions at their level, watches the
/// capture and starts it again; ends with the router.
#[cfg(windows)]
fn session_loop(me: &Weak<Router>) {
    let mut watch = Watch::default();
    let mut applied_at = None;
    loop {
        let Some(s) = me.upgrade() else { return };
        let wait = s.session_pass(&mut watch, &mut applied_at);
        drop(s);
        std::thread::park_timeout(wait);
    }
}

impl Captured for Router {
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
        Router::capture_gain(self)
    }

    fn session_wrong(&self) {
        Router::session_wrong(self);
    }
}

/// A live track's sound for the engine: from the capture while there is
/// one (after a short prebuffer, cut back when the clocks drift, the
/// session level undone), else silence as long as the player's position
/// moves on.
#[derive(Debug)]
pub struct Feed {
    /// For the log.
    name: &'static str,
    prebuffered: bool,
    /// Whether the last frames came from the capture (`None`: none yet).
    capturing: Option<bool>,
    /// Without a capture: the player's position where silence counting
    /// began, and the frames of silence given since.
    base_ms: u64,
    given: u64,
}

impl Feed {
    pub fn new(name: &'static str) -> Self {
        Self { name, prebuffered: false, capturing: None, base_ms: 0, given: 0 }
    }

    /// Counting starts again from the player's position `ms` (a start, a
    /// seek, a switch between the capture and silence).
    pub fn restart(&mut self, ms: u64) {
        self.prebuffered = false;
        self.base_ms = ms;
        self.given = 0;
    }

    /// Up to `out.len()` samples (interleaved stereo) for the engine; the
    /// player is at `position_ms` now.
    pub fn produce(&mut self, position_ms: u64, link: &dyn Captured, out: &mut [f32]) -> PcmRead {
        let ch = usize::from(CHANNELS);
        let want = out.len() / ch;
        let buffered = link.buffered_frames();
        if self.capturing != Some(buffered.is_some()) {
            if self.capturing.is_some() {
                log::info!("{}: its capture {} mid-track", self.name, if buffered.is_some() { "came back" } else { "is gone" });
            }
            self.capturing = Some(buffered.is_some());
            self.restart(position_ms);
        }
        match buffered {
            Some(buffered) => {
                let pre = (PREBUFFER_MS * u64::from(RATE) / 1000) as usize;
                if !self.prebuffered {
                    if buffered < pre {
                        return PcmRead::Pending;
                    }
                    self.prebuffered = true;
                }
                if buffered > (DRIFT_MS * u64::from(RATE) / 1000) as usize {
                    link.keep_captured(pre);
                }
                let n = link.take_captured(&mut out[..want * ch]).unwrap_or(0) / ch * ch;
                if n == 0 {
                    return PcmRead::Pending;
                }
                undo_level(&mut out[..n], link.capture_gain(), link);
                PcmRead::Frames(n / ch)
            }
            None => {
                // Silence as long as the player's position moved on.
                let moved = position_ms.saturating_sub(self.base_ms);
                let due = moved * u64::from(RATE) / 1000;
                let n = due.saturating_sub(self.given).min(want as u64) as usize;
                if n == 0 {
                    return PcmRead::Pending;
                }
                out[..n * ch].fill(0.0);
                self.given += n as u64;
                PcmRead::Frames(n)
            }
        }
    }
}

/// Undoes the session level `gain` the captured `block` went through. The
/// level is a plain float factor before process loopback (the capture at
/// 1e-6, scaled back, matches the one at 1: see examples/route_bench.rs).
/// An unknown level (0), or a block louder than the level allows, gives
/// silence: scaled up by the wrong factor it would be loud clipped noise.
pub fn undo_level(block: &mut [f32], gain: f32, link: &dyn Captured) {
    if !gain.is_finite() || gain <= 0.0 {
        block.fill(0.0);
        return;
    }
    if gain >= 0.999 {
        return;
    }
    let limit = gain * TOO_LOUD;
    if block.iter().any(|s| s.abs() > limit) {
        block.fill(0.0);
        link.session_wrong();
        return;
    }
    let k = 1.0 / gain;
    for s in block {
        *s = (*s * k).clamp(-1.0, 1.0);
    }
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
    fn a_capture_stalls_only_while_the_process_plays() {
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
