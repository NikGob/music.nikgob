//! Keeps a voice's timbre when an item plays faster or slower.
//!
//! Playing at another speed moves everything in frequency by that factor:
//! the notes (wanted: that is the sound of a sped-up or slowed song) and,
//! with them, the *formants*, the resonances of the mouth and throat that
//! make a voice sound like one person. Moved up, the singer's head sounds
//! smaller (the chipmunk); moved down, larger and hollow.
//!
//! The formants are the smooth outline of the spectrum, the pitch is its
//! fine comb. So the outline can be put back without touching the notes:
//!
//! 1. short-time spectra of the resampled sound (Hann window, 75 % overlap);
//! 2. the outline `E(f)` of each: the log spectrum smoothed by keeping only
//!    the low part of its cepstrum (below 1.5 ms, shorter than a pitch
//!    period of any voice below 660 Hz, so the comb of harmonics is
//!    smoothed away and the resonances stay);
//! 3. the outline the recording had at `f` now sits at `f * speed`, so each
//!    bin is scaled by `E(f * speed) / E(f)`;
//! 4. back to time and overlap-add.
//!
//! The gain is real and as smooth in frequency as the outline, which makes
//! it a zero-phase filter about 3 ms long that changes slowly: it cannot
//! smear a drum hit or pre-echo the way a phase vocoder does, and it leaves
//! pitch, tempo and phase alone. One gain is used for all channels (from
//! their summed power), so the stereo image does not move. It is limited to
//! +-12 dB, eased over time so it does not flutter, and scaled so each
//! frame keeps its energy: the timbre changes, the loudness does not.

//!
//! The delay of the overlap-add (three quarters of a window) is cut off at
//! the start and played out at the end, like the resampler's, so the output
//! is exactly as long as the input and nothing shifts in time.
//!
//! Cost per hop (512 frames at 48 kHz): two FFTs of 2048 per channel and
//! two for the cepstrum, about 0.6 ms of one core per second of stereo.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

/// The cepstrum is kept below this, seconds.
const LIFTER_S: f32 = 0.0015;
/// The correction never goes beyond this (natural log of 4: 12 dB).
const MAX_LOG_GAIN: f32 = 1.386_294_4;
/// How much of the way to a new gain each hop goes.
const EASE: f32 = 0.6;

pub(crate) struct Formant {
    channels: usize,
    n: usize,
    hop: usize,
    /// Speed: where the recording's outline went.
    shift: f32,
    window: Vec<f32>,
    lifter: Vec<f32>,
    fwd: Arc<dyn RealToComplex<f32>>,
    inv: Arc<dyn ComplexToReal<f32>>,
    fwd_scratch: Vec<Complex<f32>>,
    inv_scratch: Vec<Complex<f32>>,
    /// The last `n` input frames of each channel.
    input: Vec<Vec<f32>>,
    /// Overlap-add accumulators.
    acc: Vec<Vec<f32>>,
    spec: Vec<Vec<Complex<f32>>>,
    time: Vec<f32>,
    cep_spec: Vec<Complex<f32>>,
    cep: Vec<f32>,
    gain: Vec<f32>,
    /// Summed power of the channels per bin, and the level correction.
    power: Vec<f32>,
    level: f32,
    /// New frames since the last hop.
    fill: usize,
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl Formant {
    pub(crate) fn new(rate: u32, channels: usize, speed: f32) -> Self {
        let n = match rate {
            0..=50_000 => 2048,
            50_001..=100_000 => 4096,
            _ => 8192,
        };
        let hop = n / 4;
        let bins = n / 2 + 1;
        let mut planner = RealFftPlanner::<f32>::new();
        let fwd = planner.plan_fft_forward(n);
        let inv = planner.plan_fft_inverse(n);
        let window = (0..n)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos())
            .collect();
        // Keep the cepstrum's low part, easing out over its last third.
        let cutoff = (LIFTER_S * rate as f32).max(8.0);
        let lifter = (0..n)
            .map(|i| {
                let q = i.min(n - i) as f32;
                let t = ((q - cutoff * 0.67) / (cutoff * 0.33)).clamp(0.0, 1.0);
                // Also undoes the inverse transform's scale.
                (0.5 + 0.5 * (std::f32::consts::PI * t).cos()) / n as f32
            })
            .collect();
        Self {
            channels,
            n,
            hop,
            shift: speed,
            window,
            lifter,
            fwd_scratch: fwd.make_scratch_vec(),
            inv_scratch: inv.make_scratch_vec(),
            input: vec![vec![0.0; n]; channels],
            acc: vec![vec![0.0; n]; channels],
            spec: vec![vec![Complex::default(); bins]; channels],
            time: vec![0.0; n],
            cep_spec: vec![Complex::default(); bins],
            cep: vec![0.0; n],
            gain: vec![1.0; bins],
            power: vec![0.0; bins],
            level: 1.0,
            fwd,
            inv,
            fill: 0,
            delay_left: n - hop,
            in_total: 0,
            out_total: 0,
        }
    }

    /// Feeds interleaved frames; appends whatever is ready to `out`.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.in_total += (input.len() / self.channels) as u64;
        self.feed(input, u64::MAX, out);
    }

    /// Plays out what is still inside. Call once, after the last `process`.
    pub(crate) fn finish(&mut self, out: &mut Vec<f32>) {
        let limit = self.in_total;
        let zeros = vec![0.0f32; self.hop * self.channels];
        // The delay plus a hop to complete the last one, at most.
        for _ in 0..self.n / self.hop + 1 {
            if self.out_total >= limit {
                break;
            }
            self.feed(&zeros, limit, out);
        }
    }

    fn feed(&mut self, input: &[f32], limit: u64, out: &mut Vec<f32>) {
        let (ch, n, hop) = (self.channels, self.n, self.hop);
        for frame in input.chunks_exact(ch) {
            let at = n - hop + self.fill;
            for (c, s) in frame.iter().enumerate() {
                self.input[c][at] = *s;
            }
            self.fill += 1;
            if self.fill == hop {
                self.fill = 0;
                self.hop_done();
                self.emit(limit, out);
                for c in 0..ch {
                    self.input[c].copy_within(hop.., 0);
                    self.acc[c].copy_within(hop.., 0);
                    self.acc[c][n - hop..].fill(0.0);
                }
            }
        }
    }

    /// Appends the `hop` finished frames at the front of the accumulators,
    /// after dropping the start delay and without passing `limit` frames.
    fn emit(&mut self, limit: u64, out: &mut Vec<f32>) {
        let skip = self.delay_left.min(self.hop);
        self.delay_left -= skip;
        let room = usize::try_from(limit.saturating_sub(self.out_total)).unwrap_or(usize::MAX);
        let take = (self.hop - skip).min(room);
        for i in skip..skip + take {
            for acc in &self.acc {
                out.push(acc[i]);
            }
        }
        self.out_total += take as u64;
    }

    /// One analysis frame: spectra, the outline, the gain, and back.
    fn hop_done(&mut self) {
        let n = self.n;
        let bins = n / 2 + 1;
        for c in 0..self.channels {
            for i in 0..n {
                self.time[i] = self.input[c][i] * self.window[i];
            }
            let _ = self
                .fwd
                .process_with_scratch(&mut self.time, &mut self.spec[c], &mut self.fwd_scratch);
        }
        // Log of the summed power: half of it is the log magnitude.
        for k in 0..bins {
            let power: f32 = self.spec.iter().map(|s| s[k].norm_sqr()).sum();
            self.power[k] = power;
            self.cep_spec[k] = Complex::new(0.5 * (power + 1e-12).ln(), 0.0);
        }
        let _ = self
            .inv
            .process_with_scratch(&mut self.cep_spec, &mut self.cep, &mut self.inv_scratch);
        for (c, l) in self.cep.iter_mut().zip(&self.lifter) {
            *c *= *l;
        }
        let _ = self
            .fwd
            .process_with_scratch(&mut self.cep, &mut self.cep_spec, &mut self.fwd_scratch);
        // `cep_spec[k].re` is the outline now (natural log).
        let top = (bins - 1) as f32;
        for k in 0..bins {
            let from = (k as f32 * self.shift).min(top);
            let i = from as usize;
            let frac = from - i as f32;
            let a = self.cep_spec[i].re;
            let b = self.cep_spec[(i + 1).min(bins - 1)].re;
            let moved = a + (b - a) * frac;
            let log_gain = (moved - self.cep_spec[k].re).clamp(-MAX_LOG_GAIN, MAX_LOG_GAIN);
            self.gain[k] += EASE * (log_gain.exp() - self.gain[k]);
        }
        // The outline moves, the loudness stays: the frame keeps its
        // energy. Without this a lone flute note, whose outline is one
        // narrow hill, would be turned down as the hill moves off it.
        let (mut before, mut after) = (1e-12f32, 1e-12f32);
        for (p, g) in self.power.iter().zip(&self.gain) {
            before += p;
            after += p * g * g;
        }
        let level = (before / after).sqrt().clamp(0.5, 2.0);
        self.level += EASE * (level - self.level);
        let scale = 1.0 / (n as f32 * 1.5);
        for c in 0..self.channels {
            for (s, g) in self.spec[c].iter_mut().zip(&self.gain) {
                *s *= *g * self.level;
            }

            let _ = self
                .inv
                .process_with_scratch(&mut self.spec[c], &mut self.time, &mut self.inv_scratch);
            for i in 0..n {
                self.acc[c][i] += self.time[i] * self.window[i] * scale;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const RATE: u32 = 48_000;

    fn run(f: &mut Formant, input: &[f32], block: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            f.process(chunk, &mut out);
        }
        f.finish(&mut out);
        out
    }

    #[test]
    fn at_speed_one_it_gives_back_its_input_in_place() {
        let frames = RATE as usize + 777;
        let input: Vec<f32> = (0..frames)
            .flat_map(|i| {
                let t = i as f64 / f64::from(RATE);
                let l = 0.4 * (TAU * 330.0 * t).sin() + 0.2 * (TAU * 2500.0 * t).sin();
                let r = 0.3 * (TAU * 500.0 * t + 1.0).sin();
                [l as f32, r as f32]
            })
            .collect();
        let mut f = Formant::new(RATE, 2, 1.0);
        let out = run(&mut f, &input, 2 * 701);
        assert_eq!(out.len(), input.len(), "as long as the input");
        let worst = input.iter().zip(&out).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(worst < 2e-3, "moved or changed: {worst}");
    }

    /// A sung vowel played at `speed`: harmonics of a 160 Hz voice under
    /// one broad resonance at 1 kHz, all moved up by the speed.
    fn vowel(speed: f64, frames: usize) -> Vec<f32> {
        let f0 = 160.0 * speed;
        let centre = 1000.0 * speed;
        let width = 350.0 * speed;
        (0..frames)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                let mut s = 0.0;
                let mut h = 1.0;
                while f0 * h < 6000.0 {
                    let f = f0 * h;
                    let amp = 1.0 / (1.0 + ((f - centre) / width).powi(2));
                    s += amp * (TAU * f * t + h).sin();
                    h += 1.0;
                }
                (s * 0.1) as f32
            })
            .collect()
    }

    /// Amplitude of `freq` in `x` (a single-bin transform).
    fn amplitude(x: &[f32], freq: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in x.iter().enumerate() {
            let p = TAU * freq * i as f64 / f64::from(RATE);
            re += f64::from(*s) * p.cos();
            im += f64::from(*s) * p.sin();
        }
        2.0 * (re * re + im * im).sqrt() / x.len() as f64
    }

    /// Where the weight of the harmonics lies, Hz.
    fn centroid(x: &[f32], f0: f64) -> f64 {
        let (mut sum, mut weighted) = (0.0, 0.0);
        let mut h = 1.0;
        while f0 * h < 6000.0 {
            let a = amplitude(x, f0 * h).powi(2);
            sum += a;
            weighted += a * f0 * h;
            h += 1.0;
        }
        weighted / sum
    }

    #[test]
    fn a_sped_up_vowel_gets_its_resonance_back_and_keeps_its_pitch() {
        let speed = 1.25;
        let frames = RATE as usize;
        let fast = vowel(speed, frames);
        let mut f = Formant::new(RATE, 1, speed as f32);
        let out = run(&mut f, &fast, 1024);
        assert_eq!(out.len(), fast.len());
        // A whole number of periods of 200 Hz, away from the edges.
        let (a, b) = (9600, 9600 + 24_000);
        let f0 = 160.0 * speed;
        let before = centroid(&fast[a..b], f0);
        let after = centroid(&out[a..b], f0);
        assert!((before - 1250.0).abs() < 120.0, "{before}");
        assert!(after < before * 0.96, "the resonance stayed up: {before} -> {after}");
        assert!(after > 1000.0 * 0.85, "pushed too far down: {after}");
        // The notes did not move: the harmonics are where they were and
        // nothing appeared between them.
        let between = amplitude(&out[a..b], f0 * 5.5);
        let on = amplitude(&out[a..b], f0 * 5.0);
        assert!(between < on * 0.02, "{between} between harmonics of {on}");
    }

    #[test]
    fn a_slowed_vowel_gets_its_resonance_back_too() {
        let speed = 0.8;
        let fast = vowel(speed, RATE as usize);
        let mut f = Formant::new(RATE, 1, speed as f32);
        let out = run(&mut f, &fast, 1024);
        let (a, b) = (9600, 9600 + 24_000);
        let f0 = 160.0 * speed;
        let before = centroid(&fast[a..b], f0);
        let after = centroid(&out[a..b], f0);
        assert!(after > before * 1.04, "the resonance stayed down: {before} -> {after}");
        assert!(after < 1000.0 * 1.15, "pushed too far up: {after}");
    }

    #[test]
    fn a_click_stays_a_click() {
        // No pre-echo: before the click there is silence.
        let mut input = vec![0.0f32; 16_384];
        input[8000] = 0.8;
        let mut f = Formant::new(RATE, 1, 1.25);
        let out = run(&mut f, &input, 512);
        assert_eq!(out.len(), input.len());
        let before = out[..8000 - 200].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(before < 1e-3, "pre-echo {before}");
        let after = out[8000 + 400..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(after < 2e-2, "smeared: {after}");
        let peak = out[7990..8010].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 0.2, "the click is gone: {peak}");
    }
}
