//! State shared between the control thread, the decoder thread and the audio
//! callback. Everything here is atomics or lock-free: the callback touches no
//! mutex and no allocator.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering, fence};
use std::time::{Duration, Instant};

use crate::tap::VisualizerTap;

/// A clock that tells how much audio has been *heard*.
#[derive(Clone, Debug)]
pub(crate) enum PlayClock {
    /// fastframe-audio's clock: frames rendered less the device latency.
    Device(fastframe_audio::Clock),
    /// Driven by hand (offline backend and tests).
    Manual(Arc<ManualClock>),
}

impl PlayClock {
    pub(crate) fn played(&self) -> Duration {
        match self {
            Self::Device(clock) => clock.played(),
            Self::Manual(clock) => Duration::from_nanos(clock.nanos.load(Ordering::Acquire)),
        }
    }

    pub(crate) fn played_ns(&self) -> u64 {
        u64::try_from(self.played().as_nanos()).unwrap_or(u64::MAX)
    }
}

/// A hand-driven clock for the offline backend.
#[derive(Debug, Default)]
pub struct ManualClock {
    nanos: AtomicU64,
}

impl ManualClock {
    pub(crate) fn set(&self, nanos: u64) {
        self.nanos.store(nanos, Ordering::Release);
    }
}

/// Where the audible item is on its timeline, published by the audio
/// callback as a seqlock so any thread can read a consistent snapshot.
#[derive(Debug, Default)]
pub(crate) struct Cursor {
    seq: AtomicU32,
    has_item: AtomicBool,
    moving: AtomicBool,
    id: AtomicU64,
    pos_ns: AtomicU64,
    clock_ns: AtomicU64,
    applied_gen: AtomicU64,
}

/// A consistent reading of the [`Cursor`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CursorSnapshot {
    pub has_item: bool,
    /// Audio is flowing: the position advances with the clock.
    pub moving: bool,
    pub id: u64,
    /// Position of the item at `clock_ns` on the played clock.
    pub pos_ns: u64,
    pub clock_ns: u64,
    /// The newest flush generation the callback has applied.
    pub applied_gen: u64,
}

impl Cursor {
    /// Single writer (the audio callback).
    pub(crate) fn write(&self, s: CursorSnapshot) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        self.has_item.store(s.has_item, Ordering::Relaxed);
        self.moving.store(s.moving, Ordering::Relaxed);
        self.id.store(s.id, Ordering::Relaxed);
        self.pos_ns.store(s.pos_ns, Ordering::Relaxed);
        self.clock_ns.store(s.clock_ns, Ordering::Relaxed);
        self.applied_gen.store(s.applied_gen, Ordering::Relaxed);
        self.seq.store(seq.wrapping_add(2), Ordering::Release);
    }

    pub(crate) fn read(&self) -> CursorSnapshot {
        loop {
            let before = self.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let snap = CursorSnapshot {
                has_item: self.has_item.load(Ordering::Relaxed),
                moving: self.moving.load(Ordering::Relaxed),
                id: self.id.load(Ordering::Relaxed),
                pos_ns: self.pos_ns.load(Ordering::Relaxed),
                clock_ns: self.clock_ns.load(Ordering::Relaxed),
                applied_gen: self.applied_gen.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == before {
                return snap;
            }
        }
    }
}

/// Everything the three threads agree on.
#[derive(Debug)]
pub(crate) struct Shared {
    // --- output configuration, written by `Render::configure` ---
    pub cfg_rate: AtomicU32,
    pub cfg_channels: AtomicU32,
    /// Bumped whenever the rate or channel count changes; the decoder thread
    /// answers every new value with a sync flush.
    pub cfg_gen: AtomicU32,
    // --- user settings ---
    /// Linear volume gain (already through the perceptual curve), f32 bits.
    pub volume: AtomicU32,
    pub normalization: AtomicBool,
    /// Crossfade length in milliseconds; 0 means gapless.
    pub crossfade_ms: AtomicU32,
    /// The listener asked for pause: the callback fades out and holds.
    pub pause_req: AtomicBool,
    // --- published by the callback ---
    pub cursor: Cursor,
    /// Item-relative position (ns) of the next sample the callback will
    /// consume, and the item it belongs to.
    pub consumed_pos_ns: AtomicU64,
    pub consumed_id: AtomicU64,
    /// Callback blocks that ran out of data while an item was playing.
    pub underruns: AtomicU64,
    // --- published by the decoder thread ---
    /// The decoder has nothing to produce (no current item).
    pub decoder_idle: AtomicBool,
    /// Commands sent to the decoder thread and not yet handled.
    pub inflight: AtomicU32,
    /// Absolute rendered-frame index at which the last item ended with
    /// nothing following (`u64::MAX` when none); lets the offline device
    /// cut its output exactly.
    pub end_frame: AtomicU64,
    pub tap: Arc<VisualizerTap>,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            cfg_rate: AtomicU32::new(0),
            cfg_channels: AtomicU32::new(0),
            cfg_gen: AtomicU32::new(0),
            volume: AtomicU32::new(volume_curve(1.0).to_bits()),
            normalization: AtomicBool::new(true),
            crossfade_ms: AtomicU32::new(0),
            pause_req: AtomicBool::new(false),
            cursor: Cursor::default(),
            consumed_pos_ns: AtomicU64::new(0),
            consumed_id: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            decoder_idle: AtomicBool::new(true),
            inflight: AtomicU32::new(0),
            end_frame: AtomicU64::new(u64::MAX),
            tap: Arc::new(VisualizerTap::new()),
        }
    }

    pub(crate) fn volume_gain(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
}

/// Counts occurrences of something worth a warning and says when to report
/// them: at most once per `every`, with everything counted since the last
/// report. Not for the audio callback (it is used by the threads that log).
#[derive(Debug)]
pub(crate) struct LogLimiter {
    every: Duration,
    last: Option<Instant>,
    pending: u64,
}

impl LogLimiter {
    pub(crate) const fn new(every: Duration) -> Self {
        Self {
            every,
            last: None,
            pending: 0,
        }
    }

    /// Adds `n` occurrences seen at `now` (0 just asks). Returns how many to
    /// report when a report is due, and starts a new period.
    pub(crate) fn record(&mut self, n: u64, now: Instant) -> Option<u64> {
        self.pending = self.pending.saturating_add(n);
        if self.pending == 0 {
            return None;
        }
        let due = self
            .last
            .is_none_or(|t| now.saturating_duration_since(t) >= self.every);
        if !due {
            return None;
        }
        self.last = Some(now);
        Some(std::mem::take(&mut self.pending))
    }
}

/// Perceptual volume curve: slider position 0..1 to linear gain.
pub(crate) fn volume_curve(v: f32) -> f32 {
    let v = if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        1.0
    };
    v * v * v
}

/// Nanoseconds for a number of frames at `rate`.
pub(crate) fn frames_to_ns(frames: u64, rate: u32) -> u64 {
    if rate == 0 {
        return 0;
    }
    (u128::from(frames) * 1_000_000_000 / u128::from(rate)) as u64
}

/// Frames (rounded) for a duration in nanoseconds at `rate`.
pub(crate) fn ns_to_frames(ns: u64, rate: u32) -> u64 {
    ((u128::from(ns) * u128::from(rate) + 500_000_000) / 1_000_000_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_roundtrip() {
        let c = Cursor::default();
        let s = CursorSnapshot {
            has_item: true,
            moving: true,
            id: 7,
            pos_ns: 123,
            clock_ns: 456,
            applied_gen: 9,
        };
        c.write(s);
        let r = c.read();
        assert!(r.has_item && r.moving);
        assert_eq!(
            (r.id, r.pos_ns, r.clock_ns, r.applied_gen),
            (7, 123, 456, 9)
        );
    }

    #[test]
    fn volume_curve_is_cubic_and_clamped() {
        assert_eq!(volume_curve(0.0), 0.0);
        assert!((volume_curve(0.5) - 0.125).abs() < 1e-6);
        assert_eq!(volume_curve(2.0), 1.0);
        assert_eq!(volume_curve(f32::NAN), 1.0);
    }

    #[test]
    fn log_limiter_reports_once_per_period_with_everything_counted() {
        let every = Duration::from_secs(10);
        let mut l = LogLimiter::new(every);
        let t0 = Instant::now();
        assert_eq!(l.record(0, t0), None, "nothing to report");
        assert_eq!(l.record(1, t0), Some(1), "the first one goes out at once");
        assert_eq!(l.record(2, t0 + Duration::from_secs(3)), None);
        assert_eq!(l.record(0, t0 + Duration::from_secs(9)), None);
        // The period is over: the held-back ones go out, even with no new one.
        assert_eq!(l.record(0, t0 + every), Some(2));
        assert_eq!(l.record(0, t0 + every * 3), None);
        assert_eq!(l.record(4, t0 + every * 3), Some(4));
    }

    #[test]
    fn frame_ns_conversions_roundtrip() {
        for rate in [44_100, 48_000, 96_000] {
            for frames in [0u64, 1, 1000, 44_100 * 3 + 17] {
                assert_eq!(ns_to_frames(frames_to_ns(frames, rate), rate), frames);
            }
        }
    }
}
