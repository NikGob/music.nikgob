//! The tone and the reverb of one item ([`ItemFx`]), on the decoder thread:
//! after the resampler, before the item's gain and the crossfade, with its
//! own state per item, so both sides of a crossfade keep their own sound.
//!
//! Everything here is linear, which the engine relies on at a gapless join:
//! the chain of the item that ended keeps running on silence and is added
//! to the next item (see `pipeline`), so shelves and the reverb tail go on
//! exactly as if the sound had never been cut.
//!
//! # The reverb
//!
//! A feedback delay network rather than Freeverb's eight parallel combs:
//! the combs do not exchange energy, so each keeps ringing at its own
//! pitches and a drum hit comes back as a metallic flutter. Here eight
//! delay lines (33 to 77 ms, lengths without common factors) feed each
//! other through a Hadamard matrix, which is orthogonal (the loop itself
//! neither adds nor loses energy) and spreads every line into all eight on
//! each pass, so echoes multiply eightfold per pass and the tail is dense
//! within a tenth of a second.
//!
//! * The decay is set per line, `gain = 10^(-3 * delay / decay)`, so every
//!   line falls 60 dB in the same time whatever its length.
//! * A one-pole lowpass in each line makes the highs die sooner than the
//!   lows (a dark room, not a tiled one): with the default damping its
//!   corner is 6 kHz, where the tail lasts about a third as long.
//! * Before the network, each channel passes four short allpasses (5 to
//!   13 ms): a single hit enters the network already smeared into a dense
//!   burst, so the first passes do not sound like separate echoes.
//! * The left input feeds four lines, the right the other four, and the
//!   two outputs tap different lines: the tail is wide and its two sides
//!   are decorrelated, yet folds down to mono without holes.
//! * The send is cut below 150 Hz, so a lifted bass does not turn the tail
//!   into mud, and delayed by the pre-delay, so the voice stays in front of
//!   its own reverberation.
//!
//! Cost: 8 delay reads, a 24-add Hadamard, 8 allpasses per frame: about
//! 150 flops per frame, 7 MFLOP/s at 48 kHz. Not in the callback anyway.

use crate::biquad::{Coefs, State};
use crate::types::{ItemFx, Reverb, gcd};

/// Corner of the low shelf, Hz.
const LOW_SHELF_HZ: f64 = 100.0;
/// Corner of the high shelf, Hz.
const HIGH_SHELF_HZ: f64 = 7000.0;
/// The network's delay lines, milliseconds.
const LINES_MS: [f32; 8] = [33.1, 39.7, 45.3, 50.9, 56.3, 62.9, 69.7, 77.3];
/// Input diffusion: delay (ms) and feedback of the four allpasses.
const DIFFUSE: [(f32, f32); 4] = [(4.77, 0.75), (3.60, 0.75), (12.73, 0.625), (9.31, 0.625)];
/// The right channel's allpasses are this much longer than the left's.
const RIGHT_SKEW: f32 = 1.09;
/// The reverb hears nothing below this, Hz.
const SEND_LOW_CUT_HZ: f32 = 150.0;
/// Keeps the recirculating values out of the denormal range on silence.
const ANTI_DENORMAL: f32 = 1e-20;
/// How long the shelves alone are let ring into what follows.
const TONE_TAIL_MS: u64 = 100;
/// The longest tail carried into what follows, seconds.
const MAX_TAIL_S: f32 = 4.0;

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn finite(v: f32, or: f32) -> f32 {
    if v.is_finite() { v } else { or }
}

/// The tone and reverb of one item at one device format.
pub(crate) struct Chain {
    channels: usize,
    rate: u32,
    gain: f32,
    low: Option<(Coefs, Vec<State>)>,
    high: Option<(Coefs, Vec<State>)>,
    reverb: Option<Tank>,
}

impl Chain {
    /// `None` when `fx` changes nothing here (the samples are not touched).
    pub(crate) fn new(fx: &ItemFx, rate: u32, channels: usize) -> Option<Self> {
        let low_db = finite(fx.low_shelf_db, 0.0).clamp(-12.0, 12.0);
        let high_db = finite(fx.high_shelf_db, 0.0).clamp(-12.0, 12.0);
        let pre_db = finite(fx.preamp_db, 0.0).clamp(-24.0, 6.0);
        let reverb = fx.reverb.filter(|r| r.wet.is_finite() && r.wet > 0.0);
        if low_db == 0.0 && high_db == 0.0 && pre_db == 0.0 && reverb.is_none() {
            return None;
        }
        if rate == 0 || channels == 0 {
            return None;
        }
        let r = f64::from(rate);
        let shelf = |c: Coefs| (c, vec![State::default(); channels]);
        Some(Self {
            channels,
            rate,
            gain: db_to_gain(pre_db),
            low: (low_db != 0.0).then(|| shelf(Coefs::low_shelf(r, LOW_SHELF_HZ, f64::from(low_db)))),
            high: (high_db != 0.0)
                .then(|| shelf(Coefs::high_shelf(r, HIGH_SHELF_HZ, f64::from(high_db)))),
            reverb: reverb.map(|p| Tank::new(&p, rate)),
        })
    }

    /// Processes interleaved frames in place.
    pub(crate) fn process(&mut self, buf: &mut [f32]) {
        let ch = self.channels;
        if self.gain != 1.0 {
            for s in buf.iter_mut() {
                *s *= self.gain;
            }
        }
        for (coefs, states) in self.low.iter_mut().chain(self.high.iter_mut()) {
            for frame in buf.chunks_exact_mut(ch) {
                for (s, state) in frame.iter_mut().zip(states.iter_mut()) {
                    *s = state.tick(coefs, f64::from(*s)) as f32;
                }
            }
            for state in states.iter_mut() {
                state.settle();
            }
        }
        if let Some(tank) = &mut self.reverb {
            if ch == 1 {
                for s in buf.iter_mut() {
                    let (l, r) = tank.tick(*s, *s);
                    *s += (l + r) * 0.5;
                }
            } else {
                for frame in buf.chunks_exact_mut(ch) {
                    let (l, r) = tank.tick(frame[0], frame[1]);
                    frame[0] += l;
                    frame[1] += r;
                }
            }
        }
    }

    /// Frames this chain still sounds for after its input ended.
    pub(crate) fn tail_frames(&self) -> u64 {
        match &self.reverb {
            Some(tank) => (f64::from(tank.decay_s.min(MAX_TAIL_S)) * f64::from(self.rate)) as u64,
            None => u64::from(self.rate) * TONE_TAIL_MS / 1000,
        }
    }

    /// Whether the tail is long enough to be worth playing out when nothing
    /// follows (a reverb; the shelves ring for milliseconds).
    pub(crate) fn has_long_tail(&self) -> bool {
        self.reverb.is_some()
    }
}

/// A delay line of a fixed length.
struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    fn new(frames: usize) -> Self {
        Self {
            buf: vec![0.0; frames.max(1)],
            pos: 0,
        }
    }

    #[inline]
    fn read(&self) -> f32 {
        self.buf[self.pos]
    }

    #[inline]
    fn write(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.pos += 1;
        if self.pos == self.buf.len() {
            self.pos = 0;
        }
    }

    /// A Schroeder allpass around this line.
    #[inline]
    fn allpass(&mut self, x: f32, g: f32) -> f32 {
        let y = self.read() - g * x;
        self.write(x + g * y);
        y
    }
}

/// The reverb proper: only the wet signal comes out.
struct Tank {
    wet: f32,
    decay_s: f32,
    /// One-pole state and coefficient of the send's low cut.
    cut: [f32; 2],
    cut_k: f32,
    pre: [Delay; 2],
    diffuse: [[Delay; 4]; 2],
    lines: [Delay; 8],
    line_gain: [f32; 8],
    damp: [f32; 8],
    damp_k: f32,
}

impl Tank {
    fn new(p: &Reverb, rate: u32) -> Self {
        let fs = rate as f32;
        let frames = |ms: f32| ((ms * fs / 1000.0).round() as usize).max(1);
        let decay_s = finite(p.decay_s, 2.5).clamp(0.2, 10.0);
        let damping = finite(p.damping, 0.5).clamp(0.0, 1.0);
        let pre_ms = finite(p.pre_delay_ms, 0.0).clamp(0.0, 200.0);

        // Line lengths without common factors: no two lines ever line up,
        // so no period stands out of the tail.
        let mut lengths = [0usize; 8];
        for (i, ms) in LINES_MS.iter().enumerate() {
            let mut n = frames(*ms) | 1;
            while lengths[..i].iter().any(|&m| gcd(m as u64, n as u64) != 1) {
                n += 2;
            }
            lengths[i] = n;
        }
        let line_gain = lengths.map(|n| 10f32.powf(-3.0 * n as f32 / (decay_s * fs)));
        // Steady sound in, steady reverberation out: the network holds
        // 1 / (1 - g^2) times the power it is fed, and each output taps
        // half the lines at half level. `wet` is then the level of the
        // reverberation against the music itself.
        let mean_g2 = line_gain.iter().map(|g| g * g).sum::<f32>() / 8.0;
        let norm = 2.0 * (1.0 - mean_g2).sqrt();

        let corner = (20_000.0 * (1.0 - damping) * (1.0 - damping)).clamp(500.0, fs * 0.45);
        let one_pole = |hz: f32| 1.0 - (-std::f32::consts::TAU * hz / fs).exp();
        Self {
            wet: p.wet.clamp(0.0, 2.0) * norm,
            decay_s,
            cut: [0.0; 2],
            cut_k: one_pole(SEND_LOW_CUT_HZ),
            pre: [Delay::new(frames(pre_ms)), Delay::new(frames(pre_ms))],
            diffuse: [1.0, RIGHT_SKEW].map(|skew| DIFFUSE.map(|(ms, _)| Delay::new(frames(ms * skew) | 1))),
            lines: lengths.map(Delay::new),
            line_gain,
            damp: [0.0; 8],
            damp_k: one_pole(corner),
        }
    }

    /// One stereo frame in, the reverberation of everything so far out.
    #[inline]
    fn tick(&mut self, left: f32, right: f32) -> (f32, f32) {
        let mut send = [left, right];
        for (c, x) in send.iter_mut().enumerate() {
            // Low cut: the input less its own lowpass.
            self.cut[c] += self.cut_k * (*x - self.cut[c]);
            let cut = *x - self.cut[c];
            let delayed = self.pre[c].read();
            self.pre[c].write(cut);
            let mut v = delayed + ANTI_DENORMAL;
            for (line, (_, g)) in self.diffuse[c].iter_mut().zip(DIFFUSE) {
                v = line.allpass(v, g);
            }
            *x = v;
        }

        let out: [f32; 8] = std::array::from_fn(|i| self.lines[i].read());
        let mut v = [0.0f32; 8];
        for i in 0..8 {
            self.damp[i] += self.damp_k * (out[i] - self.damp[i]);
            v[i] = self.damp[i] * self.line_gain[i];
        }
        hadamard8(&mut v);
        for i in 0..8 {
            let input = if i < 4 { send[0] } else { send[1] };
            // Alternating signs: the four lines of a channel do not start
            // in step.
            let sign = if i % 2 == 0 { 0.5 } else { -0.5 };
            self.lines[i].write(input * sign + v[i]);
        }
        // Tap signs that match no row of the matrix: a row's pattern here
        // would bring that line's whole input back as one clear echo.
        let l = out[0] + out[2] + out[4] - out[6];
        let r = out[1] + out[3] - out[5] + out[7];
        (l * 0.5 * self.wet, r * 0.5 * self.wet)
    }
}

/// The 8-point Hadamard transform, scaled to be orthonormal.
#[inline]
fn hadamard8(v: &mut [f32; 8]) {
    let mut h = 1;
    while h < 8 {
        let mut i = 0;
        while i < 8 {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += h * 2;
        }
        h *= 2;
    }
    const SCALE: f32 = 0.353_553_38;
    for x in v {
        *x *= SCALE;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn echo() -> Reverb {
        Reverb {
            wet: 0.4,
            decay_s: 2.8,
            damping: 0.45,
            pre_delay_ms: 25.0,
        }
    }

    fn slowed_with_echo() -> ItemFx {
        ItemFx {
            speed: 0.85,
            low_shelf_db: 4.0,
            high_shelf_db: -3.0,
            preamp_db: -3.0,
            reverb: Some(echo()),
            keep_formants: false,
        }
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len().max(1) as f64).sqrt()
    }

    /// Deterministic noise in -1..1.
    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut x = seed | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 40) as f32 / (1u64 << 23) as f32 - 1.0
            })
            .collect()
    }

    #[test]
    fn nothing_to_do_means_no_chain() {
        assert!(Chain::new(&ItemFx::default(), RATE, 2).is_none());
        let fast = ItemFx {
            speed: 1.2,
            ..ItemFx::default()
        };
        assert!(Chain::new(&fast, RATE, 2).is_none(), "speed alone is the resampler's");
        assert!(Chain::new(&slowed_with_echo(), RATE, 2).is_some());
    }

    #[test]
    fn hadamard_keeps_energy() {
        let mut v = [0.3, -0.1, 0.7, 0.2, -0.5, 0.05, 0.4, -0.9];
        let before: f32 = v.iter().map(|x| x * x).sum();
        hadamard8(&mut v);
        let after: f32 = v.iter().map(|x| x * x).sum();
        assert!((before - after).abs() < 1e-5);
    }

    #[test]
    fn an_impulse_rings_and_dies_at_the_set_pace() {
        let mut tank = Tank::new(&echo(), RATE);
        let n = RATE as usize;
        let mut wet = Vec::with_capacity(n * 5);
        for i in 0..n * 5 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let (l, r) = tank.tick(x, x);
            assert!(l.is_finite() && r.is_finite());
            wet.push(l);
            wet.push(r);
        }
        // Nothing before the pre-delay.
        assert!(wet[..2 * (RATE as usize * 24 / 1000)].iter().all(|s| s.abs() < 1e-12));
        // Level over 100 ms windows: it only falls after the build-up, and
        // 60 dB take about the decay time (sooner: the highs go first).
        let level = |at_s: f32| {
            let a = (at_s * RATE as f32) as usize * 2;
            rms(&wet[a..a + n / 5])
        };
        let early = level(0.2);
        assert!(early > 1e-5, "{early}");
        for t in [0.5f32, 1.0, 1.5, 2.0, 3.0, 4.0] {
            assert!(level(t) < level(t - 0.3), "still growing at {t} s");
        }
        let fall_db = 20.0 * (level(0.2) / level(3.0)).log10();
        assert!(fall_db > 55.0 && fall_db < 95.0, "{fall_db} dB in 2.8 s");
        assert!(level(4.8) < early * 1e-4);
    }

    #[test]
    fn the_tail_is_dense_and_has_no_ringing_pitch() {
        // A metallic tail has a few strong periods: its autocorrelation
        // peaks at them. A dense one is noise-like at every lag.
        let mut tank = Tank::new(&echo(), RATE);
        let n = RATE as usize;
        let mut left = Vec::with_capacity(n);
        for i in 0..n {
            let x = if i == 0 { 1.0 } else { 0.0 };
            left.push(tank.tick(x, x).0);
        }
        let tail = &left[n / 4..n / 4 + 16_384];
        let energy: f64 = tail.iter().map(|s| f64::from(*s).powi(2)).sum();
        let mut worst = 0.0f64;
        for lag in 48..4800 {
            let c: f64 = tail[..tail.len() - lag]
                .iter()
                .zip(&tail[lag..])
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum();
            worst = worst.max((c / energy).abs());
        }
        assert!(worst < 0.3, "a ringing period: {worst}");
        // Dense: no long silent stretches between echoes.
        let quiet = tail.chunks(96).filter(|w| rms(w) < rms(tail) * 0.05).count();
        assert!(quiet == 0, "{quiet} gaps of 2 ms in the tail");
    }

    #[test]
    fn the_two_sides_of_the_tail_differ_and_fold_to_mono() {
        let mut tank = Tank::new(&echo(), RATE);
        let input = noise(RATE as usize * 2, 7);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for x in &input {
            let (a, b) = tank.tick(*x, *x);
            l.push(a);
            r.push(b);
        }
        let (l, r) = (&l[RATE as usize..], &r[RATE as usize..]);
        let dot: f64 = l.iter().zip(r).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
        let corr = dot / (rms(l) * rms(r) * l.len() as f64);
        assert!(corr.abs() < 0.3, "the sides are alike: {corr}");
        let mono: Vec<f32> = l.iter().zip(r).map(|(a, b)| (a + b) * 0.5).collect();
        assert!(rms(&mono) > rms(l) * 0.5, "the mono fold loses the tail");
        let balance = rms(l) / rms(r);
        assert!(balance > 0.8 && balance < 1.25, "{balance}");
    }

    #[test]
    fn wet_is_the_level_of_the_tail_against_the_music() {
        let p = Reverb {
            wet: 1.0,
            damping: 0.0,
            ..echo()
        };
        let mut tank = Tank::new(&p, RATE);
        let (a, b) = (noise(RATE as usize * 8, 3), noise(RATE as usize * 8, 11));
        let mut wet = Vec::new();
        for (x, y) in a.iter().zip(&b) {
            wet.push(tank.tick(*x, *y).0);
        }
        // Steady by the end; the low cut and what damping is left take a
        // little off.
        let ratio = rms(&wet[RATE as usize * 6..]) / rms(&a);
        assert!(ratio > 0.6 && ratio < 1.2, "{ratio}");
    }

    #[test]
    fn the_echo_preset_leaves_headroom_on_a_full_scale_mix() {
        // Full-scale noise is as hot as a mix gets; the preset's preamp
        // has to keep its level about where it was, not push it into the
        // limiter for good.
        let mut chain = Chain::new(&slowed_with_echo(), RATE, 2).unwrap();
        let dry = noise(RATE as usize * 2 * 6, 5);
        let mut out = dry.clone();
        for block in out.chunks_mut(2048) {
            chain.process(block);
        }
        assert!(out.iter().all(|s| s.is_finite()));
        let half = out.len() / 2;
        let gain_db = 20.0 * (rms(&out[half..]) / rms(&dry[half..])).log10();
        assert!(gain_db < 0.5, "louder than the dry mix by {gain_db} dB");
        assert!(gain_db > -6.0, "{gain_db} dB");
        // Over the knee (0.89) only on rare peaks.
        let over = out[half..].iter().filter(|s| s.abs() > 0.89).count();
        assert!(over * 20 < half, "{over} of {half} samples in the limiter's knee");
    }

    #[test]
    fn shelves_tilt_a_sine_by_their_gain() {
        let fx = ItemFx {
            low_shelf_db: 4.0,
            high_shelf_db: -3.0,
            ..ItemFx::default()
        };
        let tone = |freq: f64| {
            let mut chain = Chain::new(&fx, RATE, 2).unwrap();
            let mut buf: Vec<f32> = (0..RATE as usize)
                .flat_map(|i| {
                    let s = (0.25 * (std::f64::consts::TAU * freq * i as f64 / f64::from(RATE)).sin()) as f32;
                    [s, s]
                })
                .collect();
            let dry = rms(&buf[RATE as usize..]);
            chain.process(&mut buf);
            20.0 * (rms(&buf[RATE as usize..]) / dry).log10()
        };
        assert!((tone(30.0) - 4.0).abs() < 0.3, "{}", tone(30.0));
        assert!(tone(1000.0).abs() < 0.2, "{}", tone(1000.0));
        assert!((tone(16_000.0) + 3.0).abs() < 0.3, "{}", tone(16_000.0));
    }

    #[test]
    fn a_tail_added_to_a_fresh_chain_is_the_uncut_sound() {
        // The gapless join: the old chain runs on silence, a new one takes
        // the music, and their sum is what one chain would have made.
        let fx = slowed_with_echo();
        let input = noise(RATE as usize * 2, 9);
        let mut whole = input.clone();
        Chain::new(&fx, RATE, 2).unwrap().process(&mut whole);

        let cut = RATE as usize; // samples: half a second of stereo
        let mut first = Chain::new(&fx, RATE, 2).unwrap();
        let mut a = input[..cut].to_vec();
        first.process(&mut a);
        let mut b = input[cut..].to_vec();
        Chain::new(&fx, RATE, 2).unwrap().process(&mut b);
        let mut tail = vec![0.0f32; b.len()];
        first.process(&mut tail);
        let joined: Vec<f32> = a.iter().copied().chain(b.iter().zip(&tail).map(|(x, y)| x + y)).collect();
        let worst = whole.iter().zip(&joined).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "{worst}");
    }
}
