//! Lock-free visualizer tap: the audio callback appends mono samples to a
//! power-of-two ring of atomics; any thread can snapshot the latest ones.
//!
//! The writer (audio thread) does one relaxed store per frame and one release
//! store per block. A reader (the UI, only while a visualizer is visible)
//! copies the newest [`SNAPSHOT_LEN`] samples and checks, seqlock style, that
//! the writer did not lap it during the copy. Nobody ever waits for anybody.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Samples in a snapshot (about 85 ms at 48 kHz).
pub const SNAPSHOT_LEN: usize = 4096;
const RING_LEN: usize = 16_384;
const MASK: usize = RING_LEN - 1;

/// The shared sample ring. Cheap to clone through an `Arc`.
#[derive(Debug)]
pub struct VisualizerTap {
    samples: Box<[AtomicU32]>,
    /// Total mono samples ever written.
    written: AtomicU64,
    sample_rate: AtomicU32,
}

impl Default for VisualizerTap {
    fn default() -> Self {
        Self::new()
    }
}

impl VisualizerTap {
    /// An empty tap (a 48 kHz rate is assumed until the engine says otherwise).
    #[must_use]
    pub fn new() -> Self {
        Self {
            samples: (0..RING_LEN).map(|_| AtomicU32::new(0)).collect(),
            written: AtomicU64::new(0),
            sample_rate: AtomicU32::new(48_000),
        }
    }

    /// Sample rate of the samples in the tap.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate.load(Ordering::Relaxed)
    }

    pub(crate) fn set_sample_rate(&self, rate: u32) {
        self.sample_rate.store(rate, Ordering::Relaxed);
    }

    /// Total samples ever written; lets a reader skip work when nothing new
    /// arrived (the music is paused).
    #[must_use]
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Appends interleaved frames as mono (channel average). Audio-thread safe:
    /// no locks, no allocation.
    pub fn write_interleaved(&self, frames: &[f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let mut pos = self.written.load(Ordering::Relaxed);
        let inv = 1.0 / channels as f32;
        for frame in frames.chunks_exact(channels) {
            let mono = if channels == 2 {
                (frame[0] + frame[1]) * 0.5
            } else {
                frame.iter().sum::<f32>() * inv
            };
            self.samples[pos as usize & MASK].store(mono.to_bits(), Ordering::Relaxed);
            pos += 1;
        }
        self.written.store(pos, Ordering::Release);
    }

    /// Appends mono samples (tests, and sources that are mono already).
    pub fn write_mono(&self, samples: &[f32]) {
        self.write_interleaved(samples, 1);
    }

    /// Copies the newest [`SNAPSHOT_LEN`] samples, oldest first, into `out`.
    /// Returns `false` (and leaves `out` unspecified) when fewer than
    /// `SNAPSHOT_LEN` samples were ever written or the writer lapped the copy.
    pub fn snapshot(&self, out: &mut [f32; SNAPSHOT_LEN]) -> bool {
        for _ in 0..4 {
            let end = self.written.load(Ordering::Acquire);
            if (end as usize) < SNAPSHOT_LEN {
                return false;
            }
            let start = end - SNAPSHOT_LEN as u64;
            for (i, slot) in out.iter_mut().enumerate() {
                *slot = f32::from_bits(
                    self.samples[(start as usize + i) & MASK].load(Ordering::Relaxed),
                );
            }
            std::sync::atomic::fence(Ordering::Acquire);
            let after = self.written.load(Ordering::Relaxed);
            // Samples older than `after - RING_LEN` may have been overwritten.
            if after - start <= RING_LEN as u64 {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_returns_the_newest_samples_in_order() {
        let tap = VisualizerTap::new();
        let mut out = [0.0; SNAPSHOT_LEN];
        assert!(!tap.snapshot(&mut out), "not enough samples yet");
        let data: Vec<f32> = (0..10_000).map(|i| i as f32).collect();
        tap.write_mono(&data);
        assert!(tap.snapshot(&mut out));
        assert_eq!(out[0], (10_000 - SNAPSHOT_LEN) as f32);
        assert_eq!(out[SNAPSHOT_LEN - 1], 9_999.0);
    }

    #[test]
    fn interleaved_stereo_is_averaged() {
        let tap = VisualizerTap::new();
        let mut stereo = Vec::new();
        for _ in 0..SNAPSHOT_LEN {
            stereo.extend_from_slice(&[1.0, 0.0]);
        }
        tap.write_interleaved(&stereo, 2);
        let mut out = [0.0; SNAPSHOT_LEN];
        assert!(tap.snapshot(&mut out));
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn ring_wraps_without_losing_order() {
        let tap = VisualizerTap::new();
        let data: Vec<f32> = (0..RING_LEN * 3 + 123).map(|i| i as f32).collect();
        tap.write_mono(&data);
        let mut out = [0.0; SNAPSHOT_LEN];
        assert!(tap.snapshot(&mut out));
        let last = (data.len() - 1) as f32;
        assert_eq!(out[SNAPSHOT_LEN - 1], last);
        assert_eq!(out[0], last - (SNAPSHOT_LEN - 1) as f32);
    }
}
