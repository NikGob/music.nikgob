//! The frame limit («Частота кадров») on top of vsync.
//!
//! With vsync every frame waits for a vblank, so a limit below the
//! display's refresh made by sleeping a fixed time lands frames on two and
//! three vblanks in turn (13.9 and 20.8 ms for 60 on 144 Hz), and the
//! time-based springs judder. Knowing the refresh rate, the limit is
//! rounded to the nearest refresh / n and the sleep ends just after the
//! (n − 1)-th vblank, so every frame takes exactly n of them; a limit of
//! at least 95 % of the refresh does not sleep at all (vsync paces it).
//! Without the refresh rate, or when the swap turns out not to wait for
//! the vblank (frames come faster than n vblanks), the old fixed sleep
//! stays.

use std::time::{Duration, Instant};

/// How the frames are paced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pace {
    /// Vsync alone.
    Free,
    /// A frame every `n` vblanks of `period`.
    Vblanks { n: u32, period: Duration },
    /// No refresh rate known: frames this far apart.
    Every(Duration),
}

impl Pace {
    /// The pace for a limit of `fps` frames a second on a display of
    /// `refresh` Hz (`None`: unknown).
    pub fn of(fps: u32, refresh: Option<f64>) -> Self {
        let fps = f64::from(fps.max(1));
        let Some(hz) = refresh.filter(|hz| (20.0..=1000.0).contains(hz)) else {
            return Self::Every(Duration::from_secs_f64(1.0 / fps));
        };
        if fps >= 0.95 * hz {
            return Self::Free;
        }
        // The nearest frame time a whole number of vblanks gives.
        let n = (hz / fps).round().max(1.0) as u32;
        if n == 1 {
            Self::Free
        } else {
            Self::Vblanks { n, period: Duration::from_secs_f64(1.0 / hz) }
        }
    }

    /// How long to sleep before eframe paints, `began` after this frame
    /// started (right after the last vblank while something animates) and
    /// `spent` after the last frame's own code ended.
    pub fn wait(self, began: Duration, spent: Duration) -> Duration {
        match self {
            Self::Free => Duration::ZERO,
            Self::Every(budget) => budget.saturating_sub(spent),
            // Past the (n − 1)-th vblank by a quarter of one, with the
            // rest of it left for painting: the swap then waits for the
            // n-th.
            Self::Vblanks { n, period } => (period * (n - 1) + period / 4).saturating_sub(began),
        }
    }
}

/// Frames paced continuously this many times before the swap is judged.
const JUDGED_AFTER: usize = 24;

/// The pacing of the window's frames.
pub struct Pacer {
    hz: Option<f64>,
    read: Option<Instant>,
    /// When this frame began (its logic).
    began: Instant,
    /// The last frame slept to a vblank.
    slept: Option<Pace>,
    /// Frame to frame times while sleeping to vblanks.
    gaps: Vec<Duration>,
    /// The swap was seen not to wait for the vblank on this display.
    unsynced: bool,
}

impl Default for Pacer {
    fn default() -> Self {
        Self { hz: None, read: None, began: Instant::now(), slept: None, gaps: Vec::new(), unsynced: false }
    }
}

impl Pacer {
    /// At the start of a frame.
    pub fn begin(&mut self) {
        let now = Instant::now();
        if let Some(Pace::Vblanks { n, period }) = self.slept.take() {
            self.gaps.push(now - self.began);
            if self.gaps.len() >= JUDGED_AFTER {
                self.gaps.sort_unstable();
                // Waiting for vblanks, no frame is shorter than n of them
                // (bar timer noise); not waiting, they all are.
                if self.gaps[self.gaps.len() / 2] < period * n - period / 2 {
                    self.unsynced = true;
                    log::info!("frame limit: the swap does not wait for the vblank here; a fixed sleep paces it");
                }
                self.gaps.clear();
            }
        } else {
            self.gaps.clear();
        }
        self.began = now;
    }

    /// How long to sleep at the end of the frame for a limit of `fps`,
    /// `spent` after the last frame's own code ended.
    pub fn wait(&mut self, frame: &eframe::Frame, fps: u32, spent: Duration) -> Duration {
        if self.read.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
            self.read = Some(Instant::now());
            let hz = read(frame);
            if hz != self.hz {
                self.hz = hz;
                self.unsynced = false;
                self.gaps.clear();
            }
        }
        let pace = Pace::of(fps, if self.unsynced { None } else { self.hz });
        let wait = pace.wait(self.began.elapsed(), spent);
        self.slept = matches!(pace, Pace::Vblanks { .. }).then_some(pace);
        wait
    }

    /// The frame did not sleep after all (nothing asked for another).
    pub fn idle(&mut self) {
        self.slept = None;
    }
}

/// The refresh rate of the window's monitor.
#[cfg(windows)]
fn read(frame: &eframe::Frame) -> Option<f64> {
    let mhz = frame.winit_window()?.current_monitor()?.refresh_rate_millihertz()?;
    Some(f64::from(mhz) / 1000.0)
}

#[cfg(not(windows))]
fn read(_: &eframe::Frame) -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: f64) -> Duration {
        Duration::from_secs_f64(ms / 1000.0)
    }

    #[test]
    fn a_limit_below_the_refresh_is_a_whole_number_of_vblanks() {
        let period = Duration::from_secs_f64(1.0 / 144.0);
        // 60 on 144 Hz: every other vblank (72 fps), not 2 and 3 in turn.
        assert_eq!(Pace::of(60, Some(144.0)), Pace::Vblanks { n: 2, period });
        assert_eq!(Pace::of(50, Some(144.0)), Pace::Vblanks { n: 3, period });
        assert_eq!(Pace::of(30, Some(60.0)), Pace::Vblanks { n: 2, period: Duration::from_secs_f64(1.0 / 60.0) });
        // At or near the refresh, and above it: vsync alone.
        assert_eq!(Pace::of(142, Some(144.0)), Pace::Free);
        assert_eq!(Pace::of(142, Some(75.0)), Pace::Free);
        assert_eq!(Pace::of(60, Some(75.0)), Pace::Free);
        // Unknown: the fixed budget as before.
        assert_eq!(Pace::of(60, None), Pace::Every(Duration::from_secs_f64(1.0 / 60.0)));
        assert_eq!(Pace::of(60, Some(0.0)), Pace::Every(Duration::from_secs_f64(1.0 / 60.0)));
    }

    #[test]
    fn the_sleep_ends_just_past_the_vblank_before_the_one_to_show_on() {
        let p = Pace::of(60, Some(144.0));
        // 2 ms into the frame: until 6.94 + 1.74 ms.
        let w = p.wait(ms(2.0), ms(1.0));
        assert!((w.as_secs_f64() * 1000.0 - (6.944 + 1.736 - 2.0)).abs() < 0.01, "{w:?}");
        // A frame that took longer than that does not sleep.
        assert_eq!(p.wait(ms(12.0), ms(12.0)), Duration::ZERO);
        assert_eq!(Pace::Free.wait(ms(1.0), ms(1.0)), Duration::ZERO);
        assert_eq!(Pace::of(60, None).wait(ms(9.0), ms(10.0)), ms(1000.0 / 60.0) - ms(10.0));
    }
}
