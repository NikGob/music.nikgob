//! Spectrum and bass analysis for visualizers and the ambient glow.
//!
//! The analyzer is a plain struct owned by the UI. It does no work until
//! [`Analyzer::update`] is called, so a hidden visualizer costs nothing
//! except the audio callback's sample copy into the [`VisualizerTap`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::tap::{SNAPSHOT_LEN, VisualizerTap};

const DEFAULT_BANDS: usize = 64;
const LOW_HZ: f32 = 30.0;
const HIGH_HZ: f32 = 16_000.0;
const BASS_LOW_HZ: f32 = 20.0;
const BASS_HIGH_HZ: f32 = 150.0;
/// Band levels span this many dB below full scale.
const BAND_RANGE_DB: f32 = 70.0;
/// Bass RMS level mapped to 0.0 and 1.0.
const BASS_FLOOR_DB: f32 = -45.0;
const BASS_CEIL_DB: f32 = -6.0;

const BAND_ATTACK: f32 = 0.030;
const BAND_RELEASE: f32 = 0.180;
const BASS_ATTACK: f32 = 0.025;
const BASS_RELEASE: f32 = 0.200;

/// FFT-based analyzer over the last 4096 mono samples.
pub struct Analyzer {
    tap: Arc<VisualizerTap>,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    samples: Box<[f32; SNAPSHOT_LEN]>,
    input: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    amplitude: Vec<f32>,
    /// Inclusive FFT bin range of each band (for the rate in `edges_rate`).
    edges: Vec<(usize, usize)>,
    edges_rate: u32,
    bands: Vec<f32>,
    targets: Vec<f32>,
    bass: f32,
    last_written: u64,
    last_update: Option<Instant>,
}

impl Analyzer {
    /// An analyzer with 64 log-spaced bands.
    #[must_use]
    pub fn new(tap: Arc<VisualizerTap>) -> Self {
        Self::with_bands(tap, DEFAULT_BANDS)
    }

    /// An analyzer with `bands` log-spaced bands (clamped to 4..=256).
    #[must_use]
    pub fn with_bands(tap: Arc<VisualizerTap>, bands: usize) -> Self {
        let bands = bands.clamp(4, 256);
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(SNAPSHOT_LEN);
        let window = (0..SNAPSHOT_LEN)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / SNAPSHOT_LEN as f32).cos())
            .collect();
        let input = fft.make_input_vec();
        let spectrum = fft.make_output_vec();
        let scratch = fft.make_scratch_vec();
        let amplitude = vec![0.0; spectrum.len()];
        Self {
            tap,
            fft,
            window,
            samples: Box::new([0.0; SNAPSHOT_LEN]),
            input,
            spectrum,
            scratch,
            amplitude,
            edges: Vec::new(),
            edges_rate: 0,
            bands: vec![0.0; bands],
            targets: vec![0.0; bands],
            bass: 0.0,
            last_written: 0,
            last_update: None,
        }
    }

    /// Smoothed band levels, 0.0..=1.0, low frequency first.
    #[must_use]
    pub fn bands(&self) -> &[f32] {
        &self.bands
    }

    /// Bass (20-150 Hz) energy, 0.0..=1.0, attack/release smoothed and never
    /// above 1.0.
    #[must_use]
    pub fn bass_energy(&self) -> f32 {
        self.bass
    }

    /// Centre frequency of band `i`, in Hz.
    #[must_use]
    pub fn band_center_hz(&self, i: usize) -> f32 {
        let n = self.bands.len() as f32;
        let hi = HIGH_HZ.min(self.tap.sample_rate() as f32 * 0.5 * 0.98);
        LOW_HZ * (hi / LOW_HZ).powf((i as f32 + 0.5) / n)
    }

    /// Analyzes the newest samples, measuring the time since the previous call
    /// itself. Call once per UI frame while the visualizer is visible.
    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_update
            .map_or(Duration::from_millis(16), |t| now - t);
        self.last_update = Some(now);
        self.update_with_dt(dt);
    }

    /// Like [`update`](Self::update) with an explicit time step (tests,
    /// fixed-rate drivers).
    pub fn update_with_dt(&mut self, dt: Duration) {
        let dt = dt.as_secs_f32().clamp(0.001, 0.25);
        let rate = self.tap.sample_rate();
        let written = self.tap.written();
        let fresh = written != self.last_written && self.tap.snapshot(&mut self.samples);
        self.last_written = written;

        self.targets.fill(0.0);
        let mut bass_target = 0.0;
        if fresh {
            if self.edges_rate != rate {
                self.rebuild_edges(rate);
            }
            for ((x, s), w) in self
                .input
                .iter_mut()
                .zip(self.samples.iter())
                .zip(&self.window)
            {
                *x = s * w;
            }
            if self
                .fft
                .process_with_scratch(&mut self.input, &mut self.spectrum, &mut self.scratch)
                .is_ok()
            {
                // A full-scale sine under a Hann window peaks at N/4.
                let k = 4.0 / SNAPSHOT_LEN as f32;
                for (a, c) in self.amplitude.iter_mut().zip(&self.spectrum) {
                    *a = c.norm() * k;
                }
                for (target, &(lo, hi)) in self.targets.iter_mut().zip(&self.edges) {
                    let peak = self.amplitude[lo..=hi].iter().copied().fold(0.0, f32::max);
                    *target = ((db(peak) + BAND_RANGE_DB) / BAND_RANGE_DB).clamp(0.0, 1.0);
                }
                let bin_hz = rate as f32 / SNAPSHOT_LEN as f32;
                let lo = ((BASS_LOW_HZ / bin_hz).ceil() as usize).max(1);
                let hi = ((BASS_HIGH_HZ / bin_hz).floor() as usize).min(self.amplitude.len() - 1);
                let power: f32 = self.amplitude[lo..=hi].iter().map(|a| a * a * 0.5).sum();
                bass_target = ((db(power.sqrt()) - BASS_FLOOR_DB) / (BASS_CEIL_DB - BASS_FLOOR_DB))
                    .clamp(0.0, 1.0);
            }
        }
        for (v, &t) in self.bands.iter_mut().zip(&self.targets) {
            *v = smooth(*v, t, dt, BAND_ATTACK, BAND_RELEASE);
        }
        self.bass = smooth(self.bass, bass_target, dt, BASS_ATTACK, BASS_RELEASE);
    }

    fn rebuild_edges(&mut self, rate: u32) {
        let n = self.bands.len();
        let hi = HIGH_HZ.min(rate as f32 * 0.5 * 0.98);
        let bin_hz = rate as f32 / SNAPSHOT_LEN as f32;
        let max_bin = SNAPSHOT_LEN / 2;
        self.edges = (0..n)
            .map(|i| {
                let f0 = LOW_HZ * (hi / LOW_HZ).powf(i as f32 / n as f32);
                let f1 = LOW_HZ * (hi / LOW_HZ).powf((i + 1) as f32 / n as f32);
                let lo_bin = ((f0 / bin_hz).round() as usize).clamp(1, max_bin);
                let hi_bin = ((f1 / bin_hz).round() as usize).clamp(lo_bin, max_bin);
                (lo_bin, hi_bin)
            })
            .collect();
        self.edges_rate = rate;
    }
}

fn db(amplitude: f32) -> f32 {
    20.0 * amplitude.max(1e-9).log10()
}

/// Exponential approach: reaches the target without overshoot.
fn smooth(current: f32, target: f32, dt: f32, attack: f32, release: f32) -> f32 {
    let tau = if target > current { attack } else { release };
    let alpha = 1.0 - (-dt / tau).exp();
    (current + (target - current) * alpha).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_sine(tap: &VisualizerTap, hz: f32, amp: f32, start: usize, samples: usize) {
        let data: Vec<f32> = (start..start + samples)
            .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin())
            .collect();
        tap.write_mono(&data);
    }

    fn settle(a: &mut Analyzer, tap: &VisualizerTap, hz: f32, amp: f32) {
        let mut pos = 8192;
        for _ in 0..60 {
            feed_sine(tap, hz, amp, pos, 768);
            pos += 768;
            a.update_with_dt(Duration::from_millis(16));
        }
    }

    fn peak_band(a: &Analyzer) -> usize {
        a.bands()
            .iter()
            .enumerate()
            .fold((0, 0.0), |m, (i, &v)| if v > m.1 { (i, v) } else { m })
            .0
    }

    #[test]
    fn sixty_hz_sine_drives_bass_and_lights_the_right_band() {
        let tap = Arc::new(VisualizerTap::new());
        tap.set_sample_rate(48_000);
        feed_sine(&tap, 60.0, 0.5, 0, 8192);
        let mut a = Analyzer::new(Arc::clone(&tap));
        settle(&mut a, &tap, 60.0, 0.5);
        assert!(a.bass_energy() > 0.8, "bass {}", a.bass_energy());
        let hz = a.band_center_hz(peak_band(&a));
        assert!((45.0..85.0).contains(&hz), "peak band at {hz} Hz");
    }

    #[test]
    fn high_tone_does_not_move_the_bass_meter() {
        let tap = Arc::new(VisualizerTap::new());
        tap.set_sample_rate(48_000);
        feed_sine(&tap, 5000.0, 0.5, 0, 8192);
        let mut a = Analyzer::new(Arc::clone(&tap));
        settle(&mut a, &tap, 5000.0, 0.5);
        assert!(a.bass_energy() < 0.05, "bass {}", a.bass_energy());
        let hz = a.band_center_hz(peak_band(&a));
        assert!((4000.0..6300.0).contains(&hz), "peak band at {hz} Hz");
    }

    #[test]
    fn bass_never_overshoots_and_releases_to_zero() {
        let tap = Arc::new(VisualizerTap::new());
        tap.set_sample_rate(48_000);
        feed_sine(&tap, 50.0, 1.0, 0, 8192);
        let mut a = Analyzer::new(Arc::clone(&tap));
        let mut prev = 0.0;
        let mut pos = 8192;
        for i in 0..200 {
            feed_sine(&tap, 50.0, 1.0, pos, 800 + i % 3);
            pos += 800 + i % 3;
            a.update_with_dt(Duration::from_millis(16));
            let b = a.bass_energy();
            assert!((0.0..=1.0).contains(&b), "{b}");
            assert!(b + 1e-6 >= prev, "attack must be monotonic: {prev} -> {b}");
            prev = b;
        }
        for _ in 0..300 {
            a.update_with_dt(Duration::from_millis(16));
        }
        assert!(a.bass_energy() < 0.01);
        assert!(a.bands().iter().all(|&v| v < 0.01));
    }

    #[test]
    fn silence_reads_zero() {
        let tap = Arc::new(VisualizerTap::new());
        feed_sine(&tap, 100.0, 0.0, 0, 8192);
        let mut a = Analyzer::new(Arc::clone(&tap));
        a.update_with_dt(Duration::from_millis(16));
        assert_eq!(a.bass_energy(), 0.0);
    }
}
