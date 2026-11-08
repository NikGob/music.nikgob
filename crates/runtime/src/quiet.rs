//! The one gate every unattended background pass goes through, so the
//! player never loads the PC while the user plays a game.
//!
//! * **A game or another fullscreen app** ([`busy`]): while one is in
//!   front, the passes wait and look again every [`POLL`]. Windows says so
//!   itself for exclusive fullscreen and presentation mode
//!   (`SHQueryUserNotificationState`); a borderless game is a window of
//!   another process that covers its whole monitor. Our own window in
//!   front is never busy. Elsewhere nothing is detected.
//! * **Duty cycle** ([`Pacer`]): after work that took `t`, a pass rests
//!   [`K`]`·t`, so it uses about a sixth of one core on average; a long
//!   unit (a decode) rests between its chunks.
//! * **Lowest priority** ([`lower_priority`]) for every thread of such a
//!   pass.
//!
//! Playback, and what the user asks for the playing song, never wait here.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// While busy, the passes look again this often.
pub(crate) const POLL: Duration = Duration::from_secs(5);
/// The system is asked at most this often; the last answer holds between.
const RECHECK: Duration = Duration::from_secs(2);
/// After work that took `t`, a pass rests `K·t`: 1/(1+K) ≈ 17 % of a core.
pub(crate) const K: f64 = 5.0;
/// Work within one unit piles up at most this long before its rest.
pub(crate) const SLICE: Duration = Duration::from_millis(100);
/// No single rest is longer (a unit that hung on the disk or the network).
const MAX_REST: Duration = Duration::from_secs(120);
/// A rest or a wait sees a stop within this.
const STEP: Duration = Duration::from_millis(200);

/// The rest after `worked` of work on `cores` cores.
pub(crate) fn rest_for(worked: Duration, cores: usize) -> Duration {
    worked.min(MAX_REST).mul_f64(K * cores.max(1) as f64).min(MAX_REST)
}

/// Lowers the calling thread to the lowest priority.
pub(crate) fn lower_priority() {
    if let Err(e) = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min) {
        log::debug!("thread priority: {e:?}");
    }
}

/// Sleeps `d` unless `stop` is set; false when it was.
fn sleep_unless(stop: &AtomicBool, d: Duration) -> bool {
    let until = Instant::now() + d;
    loop {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let now = Instant::now();
        if now >= until {
            return true;
        }
        std::thread::sleep((until - now).min(STEP));
    }
}

/// What is in front, as far as the busy state goes.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Front {
    /// No window, or the desktop.
    Nothing,
    /// A window of this process.
    Ours,
    /// A window of another process; `covers_monitor`: borderless over its
    /// whole monitor (not a plain maximized window).
    Other { covers_monitor: bool },
}

/// `QUNS_BUSY`: a fullscreen app runs (sometimes left stuck by the shell).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const QUNS_BUSY: i32 = 2;
/// `QUNS_RUNNING_D3D_FULL_SCREEN`: an exclusive fullscreen game.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const QUNS_RUNNING_D3D_FULL_SCREEN: i32 = 3;
/// `QUNS_PRESENTATION_MODE`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const QUNS_PRESENTATION_MODE: i32 = 4;

/// Whether the passes wait: `state` is what `SHQueryUserNotificationState`
/// said (None: it failed), `front` the window in front.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn busy_of(state: Option<i32>, front: Front) -> bool {
    match front {
        Front::Ours => false,
        Front::Other { covers_monitor: true } => true,
        Front::Nothing | Front::Other { covers_monitor: false } => match state {
            Some(QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE) => true,
            // Only with another app in front: a stuck state over the
            // desktop would hold the passes forever.
            Some(QUNS_BUSY) => matches!(front, Front::Other { .. }),
            _ => false,
        },
    }
}

/// The busy state with its probe, asked at most every `recheck`.
pub(crate) struct Gate {
    probe: Box<dyn Fn() -> bool + Send + Sync>,
    recheck: Duration,
    poll: Duration,
    last: Mutex<Option<(Instant, bool)>>,
}

impl Gate {
    pub(crate) fn new(probe: impl Fn() -> bool + Send + Sync + 'static, recheck: Duration, poll: Duration) -> Self {
        Self { probe: Box::new(probe), recheck, poll, last: Mutex::new(None) }
    }

    /// A game or another fullscreen app is in front.
    pub(crate) fn busy(&self) -> bool {
        let mut last = self.last.lock();
        if let Some((at, busy)) = *last
            && at.elapsed() < self.recheck
        {
            return busy;
        }
        let busy = (self.probe)();
        if last.map(|(_, b)| b) != Some(busy) && (busy || last.is_some()) {
            if busy {
                log::info!("quiet: a game or a fullscreen app is in front, background work waits");
            } else {
                log::info!("quiet: background work goes on");
            }
        }
        *last = Some((Instant::now(), busy));
        busy
    }

    /// Blocks while busy; false when `stop` was set meanwhile.
    pub(crate) fn wait_free(&self, stop: &AtomicBool) -> bool {
        while self.busy() {
            if !sleep_unless(stop, self.poll) {
                return false;
            }
        }
        !stop.load(Ordering::Relaxed)
    }
}

static GATE: LazyLock<Gate> = LazyLock::new(|| Gate::new(system::busy, RECHECK, POLL));

/// The app's gate.
pub(crate) fn gate() -> &'static Gate {
    &GATE
}

/// A game or another fullscreen app is in front: unattended work waits.
pub(crate) fn busy() -> bool {
    GATE.busy()
}

/// Waits (async) while [`busy`].
pub(crate) async fn free() {
    while busy() {
        tokio::time::sleep(POLL).await;
    }
}

/// The pace of one background thread: [`Self::begin`] before a unit of
/// work, [`Self::step`] between its chunks, [`Self::rest`] after it.
pub(crate) struct Pacer<'a> {
    /// None: never rests nor waits (tests, work the user asked for).
    gate: Option<&'a Gate>,
    stop: &'a AtomicBool,
    cores: usize,
    slice: Duration,
    /// When the work since the last rest began.
    since: Instant,
}

impl<'a> Pacer<'a> {
    /// Paced by the app's gate, for work on one core; `stop` cuts every
    /// rest and wait short.
    pub(crate) fn new(stop: &'a AtomicBool) -> Self {
        Self::with(Some(gate()), stop, 1, SLICE)
    }

    /// Never rests nor waits; only `stop` counts.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn unpaced(stop: &'a AtomicBool) -> Self {
        Self::with(None, stop, 1, SLICE)
    }

    pub(crate) fn with(gate: Option<&'a Gate>, stop: &'a AtomicBool, cores: usize, slice: Duration) -> Self {
        Self { gate, stop, cores, slice, since: Instant::now() }
    }

    /// Work on this many cores rests that much longer.
    pub(crate) fn cores(mut self, cores: usize) -> Self {
        self.cores = cores.max(1);
        self
    }

    pub(crate) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Before a unit: waits while busy, and the work is counted from now
    /// (what came before was no work of ours). False when stopped.
    pub(crate) fn begin(&mut self) -> bool {
        let ok = match self.gate {
            Some(g) => g.wait_free(self.stop),
            None => !self.stopped(),
        };
        self.since = Instant::now();
        ok
    }

    /// Between two chunks of a unit: once a slice of work piled up, its
    /// rest and the wait while busy. False when stopped.
    pub(crate) fn step(&mut self) -> bool {
        if self.gate.is_some() && self.since.elapsed() >= self.slice {
            return self.rest();
        }
        !self.stopped()
    }

    /// After a unit: [`K`] times the work since the last rest, then the
    /// wait while busy. False when stopped.
    pub(crate) fn rest(&mut self) -> bool {
        let Some(g) = self.gate else { return !self.stopped() };
        let d = rest_for(self.since.elapsed(), self.cores);
        let ok = sleep_unless(self.stop, d) && g.wait_free(self.stop);
        self.since = Instant::now();
        ok
    }
}

/// The system's answer. Tests and other systems: never busy.
#[cfg(any(not(windows), test))]
mod system {
    pub(super) fn busy() -> bool {
        false
    }
}

#[cfg(all(windows, not(test)))]
#[allow(unsafe_code)]
mod system {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONULL, MONITORINFO, MonitorFromWindow};
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowLongW, GetWindowRect,
        GetWindowThreadProcessId, IsZoomed, WS_CAPTION,
    };

    use super::{Front, busy_of};

    pub(super) fn busy() -> bool {
        let mut state = 0;
        // SAFETY: writes one i32 into `state`.
        let ok = unsafe { SHQueryUserNotificationState(&mut state) } >= 0;
        busy_of(ok.then_some(state), front())
    }

    fn front() -> Front {
        // SAFETY: queries of the foreground window; every out-parameter is
        // a local of the right type and size, and a window gone meanwhile
        // only makes a call fail.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() || hwnd == GetShellWindow() || hwnd == GetDesktopWindow() {
                return Front::Nothing;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == GetCurrentProcessId() {
                return Front::Ours;
            }
            let mut class = [0u16; 16];
            let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            let class = String::from_utf16_lossy(&class[..n.clamp(0, 16) as usize]);
            // The desktop behind the icons.
            if class == "Progman" || class == "WorkerW" {
                return Front::Nothing;
            }
            let other = |covers_monitor| Front::Other { covers_monitor };
            let mut r = RECT::default();
            if GetWindowRect(hwnd, &mut r) == 0 {
                return other(false);
            }
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
            if monitor.is_null() {
                return other(false);
            }
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(monitor, &mut mi) == 0 {
                return other(false);
            }
            let m = mi.rcMonitor;
            let covers = r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom;
            // A maximized window with a title bar (an auto-hidden taskbar
            // lets it cover the monitor) is no game.
            let captioned = (GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CAPTION) == WS_CAPTION;
            other(covers && !(IsZoomed(hwnd) != 0 && captioned))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    use super::*;

    /// A sixth of the time works: the rest is five times the work, more
    /// on more cores, never past the cap.
    #[test]
    fn the_rest_is_k_times_the_work() {
        assert_eq!(rest_for(Duration::from_millis(100), 1), Duration::from_millis(500));
        assert_eq!(rest_for(Duration::from_secs(2), 3), Duration::from_secs(30));
        assert_eq!(rest_for(Duration::ZERO, 1), Duration::ZERO);
        assert_eq!(rest_for(Duration::from_millis(100), 0), Duration::from_millis(500), "no cores counts as one");
        assert_eq!(rest_for(Duration::from_secs(3600), 1), MAX_REST);
        let work = Duration::from_secs(1);
        let share = work.as_secs_f64() / (work + rest_for(work, 1)).as_secs_f64();
        assert!(share <= 0.2, "{share}");
    }

    /// Exclusive fullscreen and presentation mode wait, a borderless window
    /// over its monitor waits, our own window in front never does, and a
    /// «busy» left over the desktop does not hold the passes.
    #[test]
    fn the_busy_state_maps_from_the_system() {
        let other = Front::Other { covers_monitor: false };
        let borderless = Front::Other { covers_monitor: true };
        assert!(busy_of(Some(QUNS_RUNNING_D3D_FULL_SCREEN), other));
        assert!(busy_of(Some(QUNS_RUNNING_D3D_FULL_SCREEN), Front::Nothing));
        assert!(busy_of(Some(QUNS_PRESENTATION_MODE), Front::Nothing));
        assert!(busy_of(Some(QUNS_BUSY), other));
        assert!(!busy_of(Some(QUNS_BUSY), Front::Nothing));
        assert!(busy_of(Some(5), borderless), "QUNS_ACCEPTS_NOTIFICATIONS with a borderless game");
        assert!(busy_of(None, borderless));
        assert!(!busy_of(Some(5), other));
        assert!(!busy_of(None, Front::Nothing));
        assert!(!busy_of(Some(6), other), "quiet time is not a game");
        for s in [None, Some(QUNS_BUSY), Some(QUNS_RUNNING_D3D_FULL_SCREEN), Some(QUNS_PRESENTATION_MODE)] {
            assert!(!busy_of(s, Front::Ours), "our window in front: {s:?}");
        }
    }

    /// The system is asked at most every `recheck`; the answer holds between.
    #[test]
    fn the_gate_asks_the_system_now_and_then() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let gate = Gate::new(move || c.fetch_add(1, Ordering::Relaxed) % 2 == 0, Duration::from_secs(3600), Duration::from_millis(1));
        assert!(gate.busy());
        assert!(gate.busy());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let gate = Gate::new(|| true, Duration::ZERO, Duration::from_millis(1));
        assert!(gate.busy());
    }

    /// A pass waits while busy and goes on when the game is over; a stop
    /// ends the wait.
    #[test]
    fn a_pass_waits_while_busy() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        // Busy for the first three looks.
        let gate = Gate::new(move || c.fetch_add(1, Ordering::Relaxed) < 3, Duration::ZERO, Duration::from_millis(1));
        let stop = AtomicBool::new(false);
        assert!(gate.wait_free(&stop));
        assert_eq!(calls.load(Ordering::Relaxed), 4);

        let always = Gate::new(|| true, Duration::ZERO, Duration::from_millis(1));
        let stop = AtomicBool::new(true);
        assert!(!always.wait_free(&stop));
        let mut p = Pacer::with(Some(&always), &stop, 1, SLICE);
        assert!(!p.begin());
        assert!(!p.rest());
    }

    /// After a unit, the rest is K times its work; between chunks only a
    /// whole slice rests; unpaced never sleeps.
    #[test]
    fn a_pacer_rests_k_times_the_work() {
        let gate = Gate::new(|| false, Duration::ZERO, Duration::from_millis(1));
        let stop = AtomicBool::new(false);
        let mut p = Pacer::with(Some(&gate), &stop, 1, Duration::from_secs(3600));
        assert!(p.begin());
        std::thread::sleep(Duration::from_millis(20));
        let t = Instant::now();
        assert!(p.step(), "less than a slice: no rest");
        assert!(t.elapsed() < Duration::from_millis(20));
        assert!(p.rest());
        assert!(t.elapsed() >= Duration::from_millis(100), "{:?}", t.elapsed());

        let mut p = Pacer::with(Some(&gate), &stop, 1, Duration::from_millis(5));
        p.begin();
        std::thread::sleep(Duration::from_millis(10));
        let t = Instant::now();
        assert!(p.step(), "a whole slice: its rest");
        assert!(t.elapsed() >= Duration::from_millis(50), "{:?}", t.elapsed());

        let mut p = Pacer::unpaced(&stop);
        p.begin();
        std::thread::sleep(Duration::from_millis(10));
        let t = Instant::now();
        assert!(p.step() && p.rest());
        assert!(t.elapsed() < Duration::from_millis(10));
        stop.store(true, Ordering::Relaxed);
        assert!(!p.step());
    }
}
