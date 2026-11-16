//! A gain that glides to where it is asked, frame by frame: the duck (a
//! voice assistant lowering the music while it listens or speaks).
//!
//! The glide runs on the volume's perceptual curve (the gain is the cube of
//! a level that moves at a steady pace), so a fade down sounds even instead
//! of dropping most of the way in its first moments. It never passes its
//! target and lands on it exactly after the frames asked for: the level is
//! worked out from where the glide began and how far along it is, never by
//! adding small steps, which a slow glide in `f32` would round away.

use crate::shared::volume_curve;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Glide {
    /// Where the level is now (0..1); the gain is its cube.
    level: f32,
    /// Where the glide began.
    start: f32,
    /// Where it goes.
    target: f32,
    /// The glide's length and the frames of it done.
    frames: u64,
    done: u64,
}

impl Default for Glide {
    fn default() -> Self {
        Self::new()
    }
}

impl Glide {
    /// At full gain, going nowhere.
    pub(crate) const fn new() -> Self {
        Self {
            level: 1.0,
            start: 1.0,
            target: 1.0,
            frames: 0,
            done: 0,
        }
    }

    /// Glides to the linear gain `gain` (0..1) over `frames` frames, from
    /// wherever it is now; zero frames jumps there.
    pub(crate) fn set(&mut self, gain: f32, frames: u64) {
        self.target = level_of(gain);
        self.start = self.level;
        self.frames = frames;
        self.done = 0;
        if frames == 0 {
            self.level = self.target;
        }
    }

    /// The gain still moves.
    pub(crate) fn moving(&self) -> bool {
        self.level != self.target
    }

    /// One frame on; the gain for that frame.
    pub(crate) fn advance(&mut self) -> f32 {
        if self.level != self.target {
            self.done = (self.done + 1).min(self.frames);
            self.level = if self.done >= self.frames {
                self.target
            } else {
                let along = self.done as f64 / self.frames as f64;
                let level = f64::from(self.start) + f64::from(self.target - self.start) * along;
                // Within the two ends, whatever the rounding.
                (level as f32).clamp(self.start.min(self.target), self.start.max(self.target))
            };
        }
        self.gain()
    }

    /// The gain now (linear).
    pub(crate) fn gain(&self) -> f32 {
        volume_curve(self.level)
    }
}

/// The level whose cube is the linear `gain` (clamped to 0..1; not a
/// number is full gain).
fn level_of(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, 1.0).cbrt()
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every gain of a glide until it stops, at most `limit` frames.
    fn run(g: &mut Glide, limit: usize) -> Vec<f32> {
        let mut out = Vec::new();
        while g.moving() && out.len() < limit {
            out.push(g.advance());
        }
        out
    }

    #[test]
    fn a_glide_down_is_monotonic_and_lands_on_its_target_in_time() {
        let mut g = Glide::new();
        g.set(0.18, 480);
        let gains = run(&mut g, 10_000);
        // The frames it was given (a few fewer where rounding lands early).
        assert!((470..=480).contains(&gains.len()), "{}", gains.len());
        assert!(gains.windows(2).all(|w| w[1] <= w[0]), "never goes back up");
        assert!(gains.iter().all(|&x| x >= 0.18 - 1e-6), "never passes the target");
        assert!((g.gain() - 0.18).abs() < 1e-5, "{}", g.gain());
        assert!(!g.moving());
        // Resting, it stays there.
        assert!((g.advance() - 0.18).abs() < 1e-5);
    }

    #[test]
    fn a_glide_up_from_halfway_is_monotonic_and_reaches_full_gain() {
        let mut g = Glide::new();
        g.set(0.03, 1000);
        let _ = run(&mut g, 300);
        let halfway = g.gain();
        assert!(halfway < 1.0 && halfway > 0.03);
        // Restored from where it got to, not from the bottom.
        g.set(1.0, 450);
        let gains = run(&mut g, 10_000);
        assert!(gains.len() <= 450);
        assert!(gains[0] >= halfway);
        assert!(gains.windows(2).all(|w| w[1] >= w[0]));
        assert!(gains.iter().all(|&x| x <= 1.0));
        assert_eq!(g.gain(), 1.0);
    }

    #[test]
    fn zero_frames_jumps_and_odd_gains_are_clamped() {
        let mut g = Glide::new();
        g.set(0.5, 0);
        assert!(!g.moving());
        assert!((g.gain() - 0.5).abs() < 1e-5);
        g.set(7.0, 0);
        assert_eq!(g.gain(), 1.0);
        g.set(-1.0, 0);
        assert_eq!(g.gain(), 0.0);
        g.set(f32::NAN, 0);
        assert_eq!(g.gain(), 1.0);
    }

    #[test]
    fn a_slow_shallow_glide_still_moves_and_lands_in_time() {
        // 0.99 over 5 s at 48 kHz: a step per frame far below an f32 ulp
        // near 1, which adding steps would round away for good.
        let frames = 5 * 48_000;
        let mut g = Glide::new();
        g.set(0.99, frames);
        let gains = run(&mut g, 2 * frames as usize);
        assert!(!g.moving(), "it stopped");
        assert!(gains.len() as u64 <= frames, "{} frames", gains.len());
        assert!(gains.len() as u64 > frames - 1000, "it took its time: {} frames", gains.len());
        assert!((g.gain() - 0.99).abs() < 1e-5, "{}", g.gain());
        assert!(gains.windows(2).all(|w| w[1] <= w[0]));
        assert!(gains.iter().all(|&x| x >= 0.99 - 1e-5 && x <= 1.0));
        // Halfway it is about halfway (on the level's scale).
        let mid = gains[gains.len() / 2];
        assert!(mid < 0.9999 && mid > 0.991, "{mid}");
    }

    #[test]
    fn a_new_target_mid_glide_goes_on_from_where_it_is() {
        let mut g = Glide::new();
        g.set(0.0, 100);
        let _ = run(&mut g, 50);
        let at = g.gain();
        g.set(0.18, 100);
        let next = g.advance();
        // A small step from where it was, not a jump.
        assert!((next - at).abs() < 0.05, "{at} -> {next}");
    }
}
