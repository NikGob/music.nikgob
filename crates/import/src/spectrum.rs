//! Fake-lossless detection: a "lossless" file whose
//! spectrum is cut like an MP3/AAC lowpass was transcoded from a lossy source.
//!
//! # Method
//!
//! 1. Up to [`MAX_WINDOWS`] Hann windows of [`WINDOW`] mono samples are taken on a
//!    regular grid over the whole file (every ~3 s, wider for long files). Windows
//!    quieter than [`SILENCE_RMS`] (-55 dBFS) are skipped; fewer than
//!    [`MIN_WINDOWS`] usable windows means "unknown".
//! 2. The power spectra are averaged and expressed in dB relative to a full-scale
//!    sine (0 dB), then smoothed over [`SMOOTH_BINS`] bins.
//! 3. `floor` is the 20th percentile of the level in the top 7 % of the band
//!    (below Nyquist), i.e. what the file contains where nothing lives.
//!    The spectrum must rise at least [`MIN_DYNAMIC_DB`] above the floor somewhere,
//!    otherwise the file is noise-like or near-silent and the verdict is `Unknown`.
//! 4. `cutoff` is the highest frequency whose level is still [`CUTOFF_MARGIN_DB`]
//!    above the floor. `steepness` is the mean level 1000..300 Hz below the cutoff
//!    minus the mean level 300..1000 Hz above it: a lossy encoder's lowpass is a
//!    cliff (tens of dB within a few hundred Hz), a natural roll-off is not.
//! 5. Verdict (for sample rates of 40 kHz and above):
//!
//! | cutoff            | steepness needed | typical source                  |
//! |-------------------|------------------|---------------------------------|
//! | below 17.5 kHz    | 15 dB            | MP3 up to ~192 kbps, 16 kHz cut |
//! | 17.5 .. 19.5 kHz  | 25 dB            | AAC / MP3 V2..V1                |
//! | 19.5 .. 20.3 kHz  | 35 dB            | MP3 V0 and the like             |
//! | 20.3 kHz and up   | never            | indistinguishable from a CD ADC |
//!
//! A cut below [`nikgob_core::quality::LOWEST_LOWPASS_HZ`] (11 kHz) is no
//! encoder's: the music ends there by itself, and the verdict is `Unknown`.
//!
//! Hi-res files (above 48 kHz) whose content stops below 24 kHz with a cliff are
//! reported as [`Verdict::Upsampled`]: not lossy, but not Hi-Res either.
//!
//! The thresholds are deliberately conservative where a genuine CD can look the
//! same (a steep 20 kHz anti-alias filter): a false "fake" costs the user a
//! better file, a missed fake is only a mislabel.

use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const WINDOW: usize = 4096;
pub const MAX_WINDOWS: usize = 96;
pub const MIN_WINDOWS: usize = 3;
/// Windows with RMS below this (linear, about -55 dBFS) carry no information.
pub const SILENCE_RMS: f32 = 0.001_78;
pub const SMOOTH_BINS: usize = 9;
pub const CUTOFF_MARGIN_DB: f32 = 15.0;
pub const MIN_DYNAMIC_DB: f32 = 40.0;
/// A body quieter than this (dB re full-scale sine, per bin) is treated as a noise floor.
pub const MIN_SIGNAL_DB: f32 = -75.0;
pub const MIN_RATE: u32 = 40_000;

/// The verdict is kept in the library, so it lives in the core.
pub use nikgob_core::quality::Verdict;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumReport {
    pub verdict: Verdict,
    /// Highest frequency with real content, Hz.
    pub cutoff_hz: f32,
    /// Level drop across the cutoff, dB.
    pub steepness_db: f32,
    /// Level of the empty top band, dB re full-scale sine.
    pub floor_db: f32,
    /// Level of the loudest part of the band, dB re full-scale sine.
    pub reference_db: f32,
    pub windows: usize,
}

impl SpectrumReport {
    fn unknown(windows: usize) -> Self {
        Self {
            verdict: Verdict::Unknown,
            cutoff_hz: 0.0,
            steepness_db: 0.0,
            floor_db: 0.0,
            reference_db: 0.0,
            windows,
        }
    }

    pub fn is_transcoded(&self) -> bool {
        self.verdict == Verdict::Transcoded
    }
}

/// Collects windows while the audio streams by.
pub struct SpectrumCollector {
    rate: u32,
    channels: usize,
    fft: Arc<dyn RealToComplex<f32>>,
    hann: Vec<f32>,
    hann_sum: f32,
    power: Vec<f64>,
    used: usize,
    /// Frames between window starts.
    stride: u64,
    /// Absolute frame index of the next window start.
    next_start: u64,
    frame: u64,
    cur: Vec<f32>,
    filling: bool,
    scratch_in: Vec<f32>,
    scratch_out: Vec<realfft::num_complex::Complex<f32>>,
}

impl SpectrumCollector {
    /// `duration_hint` (seconds) spreads the windows over the whole file.
    pub fn new(rate: u32, channels: usize, duration_hint_secs: f64) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(WINDOW);
        let hann: Vec<f32> = (0..WINDOW)
            .map(|i| {
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WINDOW as f32).cos()
            })
            .collect();
        let hann_sum = hann.iter().sum();
        let min_stride = u64::from(rate) * 3;
        let wanted = (duration_hint_secs.max(0.0) * f64::from(rate) / MAX_WINDOWS as f64) as u64;
        let scratch_in = fft.make_input_vec();
        let scratch_out = fft.make_output_vec();
        Self {
            rate,
            channels: channels.max(1),
            fft,
            hann,
            hann_sum,
            power: vec![0.0; WINDOW / 2 + 1],
            used: 0,
            stride: min_stride.max(wanted).max(WINDOW as u64),
            // Start a little in: the first second is often a fade-in.
            next_start: u64::from(rate) / 2,
            frame: 0,
            cur: Vec::with_capacity(WINDOW),
            filling: false,
            scratch_in,
            scratch_out,
        }
    }

    /// Feeds interleaved samples.
    pub fn push(&mut self, interleaved: &[f32]) {
        let ch = self.channels;
        for frame in interleaved.chunks_exact(ch) {
            if !self.filling && self.frame >= self.next_start && self.used < MAX_WINDOWS {
                self.filling = true;
                self.cur.clear();
            }
            if self.filling {
                self.cur.push(frame.iter().sum::<f32>() / ch as f32);
                if self.cur.len() == WINDOW {
                    self.filling = false;
                    self.next_start = self.frame + self.stride;
                    self.consume_window();
                }
            }
            self.frame += 1;
        }
    }

    fn consume_window(&mut self) {
        let rms = (self.cur.iter().map(|s| s * s).sum::<f32>() / WINDOW as f32).sqrt();
        if rms < SILENCE_RMS {
            return;
        }
        for ((dst, s), w) in self.scratch_in.iter_mut().zip(&self.cur).zip(&self.hann) {
            *dst = s * w;
        }
        if self
            .fft
            .process(&mut self.scratch_in, &mut self.scratch_out)
            .is_err()
        {
            return;
        }
        for (p, c) in self.power.iter_mut().zip(&self.scratch_out) {
            *p += f64::from(c.re * c.re + c.im * c.im);
        }
        self.used += 1;
    }

    /// Averaged spectrum in dB re full-scale sine, `None` if too few windows were usable.
    pub fn levels_db(&self) -> Option<Vec<f32>> {
        if self.used < MIN_WINDOWS {
            return None;
        }
        // A full-scale sine at a bin centre has |X| = A * sum(w) / 2.
        let norm = f64::from(self.hann_sum / 2.0).powi(2);
        Some(
            self.power
                .iter()
                .map(|p| (10.0 * (p / self.used as f64 / norm + 1e-20).log10()) as f32)
                .collect(),
        )
    }

    pub fn windows(&self) -> usize {
        self.used
    }

    pub fn finish(self) -> SpectrumReport {
        self.finish_with_levels().0
    }

    /// The report and the averaged spectrum it was judged on, in
    /// [`nikgob_core::quality::SPECTRUM_POINTS`] bands (for the quality
    /// panel's chart; `None` when too few windows were usable).
    pub fn finish_with_levels(self) -> (SpectrumReport, Option<Vec<f32>>) {
        match self.levels_db() {
            Some(levels) => (
                classify(&levels, self.rate, self.used),
                Some(nikgob_core::quality::downsample_db(&levels, nikgob_core::quality::SPECTRUM_POINTS)),
            ),
            None => (SpectrumReport::unknown(self.used), None),
        }
    }
}

fn db_mean(levels: &[f32]) -> f32 {
    if levels.is_empty() {
        return f32::NEG_INFINITY;
    }
    let p: f64 = levels
        .iter()
        .map(|l| 10f64.powf(f64::from(*l) / 10.0))
        .sum::<f64>()
        / levels.len() as f64;
    (10.0 * p.log10()) as f32
}

/// Smooths in the power domain with a centred moving average.
fn smooth(levels: &[f32], width: usize) -> Vec<f32> {
    let half = width / 2;
    (0..levels.len())
        .map(|i| {
            let lo = i.saturating_sub(half);
            let hi = (i + half + 1).min(levels.len());
            db_mean(&levels[lo..hi])
        })
        .collect()
}

/// Verdict from an averaged spectrum (`levels[k]` for bin `k`, 0..=WINDOW/2).
pub fn classify(levels_db: &[f32], sample_rate: u32, windows: usize) -> SpectrumReport {
    let bins = levels_db.len();
    if bins < 64 || sample_rate < MIN_RATE {
        return SpectrumReport::unknown(windows);
    }
    let nyquist = sample_rate as f32 / 2.0;
    let hz_per_bin = nyquist / (bins - 1) as f32;
    let bin_of = |hz: f32| ((hz / hz_per_bin).round().max(0.0) as usize).min(bins - 1);
    let smoothed = smooth(levels_db, SMOOTH_BINS);

    // Floor: what the top 7 % of the band holds.
    let top_from = bin_of(nyquist * 0.93);
    let mut top: Vec<f32> = smoothed[top_from..bins - 1].to_vec();
    if top.len() < 4 {
        return SpectrumReport::unknown(windows);
    }
    top.sort_by(f32::total_cmp);
    let floor_db = top[top.len() / 5];

    // Reference: loudest smoothed level between 100 Hz and 16 kHz.
    let ref_lo = bin_of(100.0);
    let ref_hi = bin_of(16_000.0_f32.min(nyquist * 0.8));
    let reference_db = smoothed[ref_lo..=ref_hi]
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    if reference_db - floor_db < MIN_DYNAMIC_DB {
        // The top band is about as loud as the body: nothing is cut. Unless the
        // whole thing is just a noise floor, in which case there is nothing to judge.
        let verdict = if reference_db >= MIN_SIGNAL_DB {
            Verdict::Genuine
        } else {
            Verdict::Unknown
        };
        return SpectrumReport {
            verdict,
            cutoff_hz: if verdict == Verdict::Genuine { nyquist } else { 0.0 },
            floor_db,
            reference_db,
            ..SpectrumReport::unknown(windows)
        };
    }

    // Cutoff: highest bin still CUTOFF_MARGIN_DB above the floor.
    let threshold = floor_db + CUTOFF_MARGIN_DB;
    let cutoff_bin = (ref_lo..bins - 1)
        .rev()
        .find(|&k| smoothed[k] >= threshold)
        .unwrap_or(ref_lo);
    let cutoff_hz = cutoff_bin as f32 * hz_per_bin;

    // Steepness across the cutoff.
    let below = &levels_db[bin_of(cutoff_hz - 1000.0)..=bin_of(cutoff_hz - 300.0)];
    let above_lo = bin_of(cutoff_hz + 300.0);
    let above_hi = bin_of(cutoff_hz + 1000.0);
    let steepness_db = if cutoff_hz + 300.0 >= nyquist || above_hi <= above_lo {
        0.0
    } else {
        db_mean(below) - db_mean(&levels_db[above_lo..=above_hi])
    };

    let transcoded = (cutoff_hz < 17_500.0 && steepness_db >= 15.0)
        || (cutoff_hz < 19_500.0 && steepness_db >= 25.0)
        || (cutoff_hz < 20_300.0 && steepness_db >= 35.0);
    let verdict = if transcoded && cutoff_hz < nikgob_core::quality::LOWEST_LOWPASS_HZ {
        // Nothing above a few kHz is the music's own (a quiet synth, a
        // piano), not an encoder's lowpass: no telling from the spectrum.
        Verdict::Unknown
    } else if transcoded {
        Verdict::Transcoded
    } else if sample_rate > 48_000 && cutoff_hz <= 24_000.0 && steepness_db >= 25.0 {
        Verdict::Upsampled
    } else {
        Verdict::Genuine
    };
    SpectrumReport {
        verdict,
        cutoff_hz,
        steepness_db,
        floor_db,
        reference_db,
        windows,
    }
}

/// Analyses a whole buffer (mono or interleaved), for tests and small inputs.
pub fn analyze_buffer(samples: &[f32], rate: u32, channels: usize) -> SpectrumReport {
    let secs = samples.len() as f64 / channels.max(1) as f64 / f64::from(rate);
    let mut c = SpectrumCollector::new(rate, channels, secs);
    c.push(samples);
    c.finish()
}

#[cfg(test)]
pub(crate) mod testsig {
    //! Deterministic synthetic signals shared by the tests.

    use realfft::RealFftPlanner;

    pub struct Rng(pub u64);

    impl Rng {
        /// Uniform in [-1, 1).
        pub fn next_f32(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        }
    }

    /// White noise of `secs` seconds, zeroed above `cutoff_hz` by an FFT
    /// brick wall (None = full band), plus 16-bit-sized dither like a real
    /// 16-bit decode would carry.
    pub fn noise(rate: u32, secs: f64, amp: f32, cutoff_hz: Option<f32>, seed: u64) -> Vec<f32> {
        let n = (f64::from(rate) * secs) as usize & !1;
        let mut rng = Rng(seed);
        let mut sig: Vec<f32> = (0..n).map(|_| rng.next_f32() * amp).collect();
        if let Some(cut) = cutoff_hz {
            let mut planner = RealFftPlanner::<f32>::new();
            let fwd = planner.plan_fft_forward(n);
            let inv = planner.plan_fft_inverse(n);
            let mut spec = fwd.make_output_vec();
            fwd.process(&mut sig, &mut spec).unwrap();
            let hz_per_bin = rate as f32 / n as f32;
            for (k, c) in spec.iter_mut().enumerate() {
                if k as f32 * hz_per_bin > cut {
                    *c = Default::default();
                }
            }
            inv.process(&mut spec, &mut sig).unwrap();
            for s in &mut sig {
                *s /= n as f32;
            }
        }
        let lsb = 1.0 / 32_768.0;
        for s in &mut sig {
            *s += rng.next_f32() * lsb * 0.5;
        }
        sig
    }
}

#[cfg(test)]
mod tests {
    use super::testsig::noise;
    use super::*;

    #[test]
    fn full_band_noise_is_genuine() {
        let sig = noise(44_100, 30.0, 0.2, None, 1);
        let r = analyze_buffer(&sig, 44_100, 1);
        assert_eq!(r.verdict, Verdict::Genuine, "{r:?}");
        assert!(r.cutoff_hz > 21_000.0, "{r:?}");
    }

    #[test]
    fn noise_lowpassed_at_16k_is_transcoded() {
        let sig = noise(44_100, 30.0, 0.2, Some(16_000.0), 2);
        let r = analyze_buffer(&sig, 44_100, 1);
        assert_eq!(r.verdict, Verdict::Transcoded, "{r:?}");
        assert!((r.cutoff_hz - 16_000.0).abs() < 400.0, "{r:?}");
        assert!(r.steepness_db > 40.0, "{r:?}");
    }

    #[test]
    fn noise_lowpassed_at_19k_is_transcoded() {
        let sig = noise(48_000, 30.0, 0.2, Some(19_000.0), 3);
        let r = analyze_buffer(&sig, 48_000, 1);
        assert_eq!(r.verdict, Verdict::Transcoded, "{r:?}");
        assert!((r.cutoff_hz - 19_000.0).abs() < 400.0, "{r:?}");
    }

    #[test]
    fn a_20k_cd_style_filter_with_a_clean_top_is_not_condemned_twice() {
        // 21 kHz cut: above every lossy lowpass, so genuine.
        let sig = noise(44_100, 30.0, 0.2, Some(21_000.0), 4);
        let r = analyze_buffer(&sig, 44_100, 1);
        assert_eq!(r.verdict, Verdict::Genuine, "{r:?}");
        assert!(r.cutoff_hz > 20_500.0, "{r:?}");
    }

    #[test]
    fn hires_container_with_cd_content_is_upsampled() {
        let sig = noise(96_000, 20.0, 0.2, Some(22_000.0), 5);
        let r = analyze_buffer(&sig, 96_000, 1);
        assert_eq!(r.verdict, Verdict::Upsampled, "{r:?}");
        let full = noise(96_000, 20.0, 0.2, None, 6);
        assert_eq!(analyze_buffer(&full, 96_000, 1).verdict, Verdict::Genuine);
    }

    #[test]
    fn silence_and_short_files_are_unknown() {
        let silent = vec![0f32; 44_100 * 20];
        assert_eq!(analyze_buffer(&silent, 44_100, 1).verdict, Verdict::Unknown);
        let short = noise(44_100, 1.0, 0.2, Some(16_000.0), 7);
        assert_eq!(analyze_buffer(&short, 44_100, 1).verdict, Verdict::Unknown);
        // Low sample rates cannot show a 16 kHz cut at all.
        let low = noise(22_050, 30.0, 0.2, Some(8_000.0), 8);
        assert_eq!(analyze_buffer(&low, 22_050, 1).verdict, Verdict::Unknown);
    }

    #[test]
    fn stereo_interleaved_input_is_mixed_down() {
        let mono = noise(44_100, 30.0, 0.2, Some(16_000.0), 9);
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        let r = analyze_buffer(&stereo, 44_100, 2);
        assert_eq!(r.verdict, Verdict::Transcoded, "{r:?}");
    }

    /// A quiet synth of an OST: nothing above 3 kHz, a cliff to the dither.
    /// No encoder cuts there, so it is no fake.
    #[test]
    fn music_that_ends_at_3_khz_is_not_called_a_fake() {
        let bins = 2049;
        let hz_per_bin = 22_050.0 / (bins - 1) as f32;
        let levels: Vec<f32> = (0..bins)
            .map(|k| if k as f32 * hz_per_bin < 2_800.0 { -50.0 } else { -115.0 })
            .collect();
        let r = classify(&levels, 44_100, 10);
        assert_eq!(r.verdict, Verdict::Unknown, "{r:?}");
        let sig = noise(44_100, 30.0, 0.2, Some(6_000.0), 10);
        assert_eq!(analyze_buffer(&sig, 44_100, 1).verdict, Verdict::Unknown);
    }

    #[test]
    fn classify_works_on_a_synthetic_spectrum() {
        // 2049 bins at 44.1 kHz: flat -50 dB up to 16 kHz, -110 dB above.
        let bins = 2049;
        let hz_per_bin = 22_050.0 / (bins - 1) as f32;
        let levels: Vec<f32> = (0..bins)
            .map(|k| if k as f32 * hz_per_bin < 16_000.0 { -50.0 } else { -110.0 })
            .collect();
        let r = classify(&levels, 44_100, 10);
        assert_eq!(r.verdict, Verdict::Transcoded);
        assert!((r.cutoff_hz - 16_000.0).abs() < 150.0, "{r:?}");
        let flat: Vec<f32> = vec![-100.0; bins];
        // Nothing but a noise floor -> unknown rather than genuine.
        assert_eq!(classify(&flat, 44_100, 10).verdict, Verdict::Unknown);
    }
}
