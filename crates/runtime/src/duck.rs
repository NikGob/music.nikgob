//! The duck: the sound lowered for a while (a voice assistant listens or
//! speaks) and restored by itself `ttl` after the last request, so a
//! caller that died does not leave the music quiet.
//!
//! The engine glides the gain ([`crate::Playback::set_duck`]); the worker
//! keeps the request and its end, and its loop wakes at that end
//! ([`DuckState::deadline`]), not at the next poll.

use std::time::{Duration, Instant};

use nikgob_view::{Command, DuckView};

use crate::Worker;

/// How long the sound takes to come back when the duck ends by itself
/// (and a clear that names no fade, from a caller).
pub const DUCK_RESTORE_FADE: Duration = Duration::from_millis(450);
/// The longest fade a request may ask for.
const MAX_FADE_MS: u32 = 5000;
/// What the ttl of a request is kept within.
const TTL_MS: (u32, u32) = (100, 60_000);

/// A request's values within their limits: the gain 0..=1 (not a number:
/// no duck), the fade at most 5 s, the ttl 0.1..=60 s.
pub(crate) fn limits(gain: f32, fade_ms: u32, ttl_ms: u32) -> (f32, Duration, Duration) {
    let gain = if gain.is_finite() { gain.clamp(0.0, 1.0) } else { 1.0 };
    (gain, fade(fade_ms), Duration::from_millis(u64::from(ttl_ms.clamp(TTL_MS.0, TTL_MS.1))))
}

/// A fade within its limit.
pub(crate) fn fade(fade_ms: u32) -> Duration {
    Duration::from_millis(u64::from(fade_ms.min(MAX_FADE_MS)))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Active {
    gain: f32,
    until: Instant,
    until_unix_ms: i64,
}

/// The duck in force, if any.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DuckState {
    active: Option<Active>,
}

impl DuckState {
    /// A request at `now` (`now_unix_ms` the same moment on the wall
    /// clock): `gain` from now on, until `ttl` from now, whatever was
    /// before (a new request extends an old one).
    pub(crate) fn set(&mut self, gain: f32, ttl: Duration, now: Instant, now_unix_ms: i64) {
        let ttl_ms = i64::try_from(ttl.as_millis()).unwrap_or(i64::MAX);
        self.active = Some(Active {
            gain,
            until: now + ttl,
            until_unix_ms: now_unix_ms.saturating_add(ttl_ms),
        });
    }

    /// Ends the duck; true when there was one.
    pub(crate) fn clear(&mut self) -> bool {
        self.active.take().is_some()
    }

    /// When the duck ends by itself.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.active.map(|a| a.until)
    }

    /// The duck is over by `now`.
    pub(crate) fn due(&self, now: Instant) -> bool {
        self.active.is_some_and(|a| now >= a.until)
    }

    /// Its gain, while one is in force.
    pub(crate) fn gain(&self) -> Option<f32> {
        self.active.map(|a| a.gain)
    }

    pub(crate) fn view(&self) -> Option<DuckView> {
        self.active.map(|a| DuckView { gain: a.gain, until_unix_ms: a.until_unix_ms })
    }
}

impl Worker {
    /// `Command::SetDuck` / `Command::ClearDuck`.
    pub(crate) fn duck_command(&mut self, c: Command) {
        match c {
            Command::SetDuck { gain, fade_ms, ttl_ms } => {
                let (gain, fade, ttl) = limits(gain, fade_ms, ttl_ms);
                self.duck.set(gain, ttl, Instant::now(), nikgob_library::now_ms());
                self.playback.set_duck(gain, fade);
                self.publish_duck();
            }
            Command::ClearDuck { fade_ms } => self.clear_duck(fade(fade_ms)),
            _ => {}
        }
    }

    /// Restores the sound over `fade`; nothing when it is not lowered.
    fn clear_duck(&mut self, fade: Duration) {
        if self.duck.clear() {
            self.playback.set_duck(1.0, fade);
            self.publish_duck();
        }
    }

    /// The loop woke at the duck's end: the sound comes back.
    pub(crate) fn duck_due(&mut self) {
        if self.duck.due(Instant::now()) {
            log::debug!("the duck ended by itself");
            self.clear_duck(DUCK_RESTORE_FADE);
        }
    }

    fn publish_duck(&mut self) {
        let duck = self.duck.view();
        self.publish(|s| s.player.duck = duck);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_kept_within_its_limits() {
        assert_eq!(limits(0.18, 120, 8000), (0.18, Duration::from_millis(120), Duration::from_millis(8000)));
        assert_eq!(limits(1.7, 9000, 5).0, 1.0);
        assert_eq!(limits(1.7, 9000, 5).1, Duration::from_millis(5000));
        assert_eq!(limits(1.7, 9000, 5).2, Duration::from_millis(100));
        assert_eq!(limits(-0.2, 0, 600_000), (0.0, Duration::ZERO, Duration::from_millis(60_000)));
        assert_eq!(limits(f32::NAN, 80, 15_000).0, 1.0, "not a number lowers nothing");
        assert_eq!(fade(450), Duration::from_millis(450));
        assert_eq!(fade(u32::MAX), Duration::from_millis(5000));
    }

    #[test]
    fn a_new_request_before_the_end_extends_it() {
        let t0 = Instant::now();
        let mut d = DuckState::default();
        d.set(0.18, Duration::from_millis(8000), t0, 1_000_000);
        assert_eq!(d.deadline(), Some(t0 + Duration::from_millis(8000)));
        assert_eq!(d.view(), Some(DuckView { gain: 0.18, until_unix_ms: 1_008_000 }));
        // Five seconds on, the speaker asks again: eight more from then.
        let t5 = t0 + Duration::from_secs(5);
        assert!(!d.due(t5));
        d.set(0.18, Duration::from_millis(8000), t5, 1_005_000);
        assert!(!d.due(t0 + Duration::from_millis(8000)), "the first end has passed, the duck holds");
        assert!(!d.due(t5 + Duration::from_millis(7999)));
        assert!(d.due(t5 + Duration::from_millis(8000)));
        assert_eq!(d.view().map(|v| v.until_unix_ms), Some(1_013_000));
        // Listening again: the gain changes, the end moves.
        d.set(0.03, Duration::from_millis(15_000), t5, 1_005_000);
        assert_eq!(d.gain(), Some(0.03));
        assert_eq!(d.deadline(), Some(t5 + Duration::from_millis(15_000)));
    }

    #[test]
    fn a_clear_without_a_duck_is_harmless() {
        let mut d = DuckState::default();
        assert!(!d.clear());
        assert!(!d.clear());
        assert_eq!(d.view(), None);
        assert_eq!(d.deadline(), None);
        assert!(!d.due(Instant::now()));
        d.set(0.5, Duration::from_millis(100), Instant::now(), 0);
        assert!(d.clear());
        assert!(!d.clear(), "cleared once");
        assert_eq!(d.gain(), None);
    }
}
