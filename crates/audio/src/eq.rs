//! The ten-band equalizer of the audio callback.
//!
//! Ten octave-wide bells (32 Hz to 16 kHz) computed for the device rate, in
//! series, per channel. It runs in the callback, next to the volume, so a
//! dragged point is heard on the next block rather than after the buffered
//! second.
//!
//! Real-time rules hold: fixed arrays, no locks, no allocation. The control
//! side writes the wanted gains to atomics and bumps a generation; the
//! callback picks them up and *glides* there: every [`STEP_FRAMES`] frames
//! each band's gain moves a little towards its target and its coefficients
//! are recomputed, and the preamp is interpolated per sample, so a preset
//! switch or a dragged point makes no zipper noise and no click.
//!
//! Off or flat, the equalizer is not in the path at all (the samples are
//! not touched). Turning it off glides to flat first and lets the filters
//! ring out before it steps aside.
//!
//! Cost: ten sections of five multiplies per sample and channel, about
//! 10 MFLOP/s for 48 kHz stereo, well under a percent of one core.

use std::sync::atomic::Ordering;

use crate::biquad::{Coefs, State, angle};
use crate::shared::Shared;

/// Number of bands.
pub const EQ_BANDS: usize = 10;
/// Centre frequency of each band, Hz.
pub const EQ_FREQS: [f32; EQ_BANDS] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0,
];
/// The farthest a band goes either way, dB.
pub const EQ_MAX_DB: f32 = 12.0;
/// Quality of every band: about an octave wide, so neighbours add up to a
/// smooth curve.
const Q: f64 = 1.41;
/// Channels the equalizer keeps state for; a wider device plays unequalized.
const MAX_CHANNELS: usize = 8;
/// Frames between two coefficient updates while the gains glide.
const STEP_FRAMES: usize = 32;
/// Time constant of the glide.
const GLIDE_MS: f64 = 30.0;
/// How long the filters ring out into silence, milliseconds.
const RING_MS: f64 = 250.0;
/// Closer than this to its target (dB), a gain is set to it.
const SNAP_DB: f32 = 0.005;
/// Rate the preamp is worked out at (the curve barely depends on it).
const NOMINAL_RATE: f64 = 48_000.0;

fn clean(gains: &[f32; EQ_BANDS]) -> [f32; EQ_BANDS] {
    gains.map(|g| {
        if g.is_finite() {
            g.clamp(-EQ_MAX_DB, EQ_MAX_DB)
        } else {
            0.0
        }
    })
}

/// The equalizer's gain at `freq` (Hz) for these band gains, dB: the curve
/// the listener hears, for drawing it.
#[must_use]
pub fn eq_response_db(gains: &[f32; EQ_BANDS], freq: f32) -> f32 {
    let gains = clean(gains);
    let mut power = 1.0f64;
    for (band, gain) in gains.iter().enumerate() {
        if *gain != 0.0 {
            let c = Coefs::peaking(NOMINAL_RATE, f64::from(EQ_FREQS[band]), Q, f64::from(*gain));
            power *= c.power_at(NOMINAL_RATE, f64::from(freq));
        }
    }
    (10.0 * power.log10()) as f32
}

/// The gain (dB, never positive) that goes before these band gains so the
/// curve's highest point sits at 0 dB: what was at full scale stays there.
/// Neighbouring boosts add up, so this is the peak of the whole curve, not
/// of the highest band.
#[must_use]
pub fn eq_preamp_db(gains: &[f32; EQ_BANDS]) -> f32 {
    // Twelve points an octave from 16 Hz to 22 kHz: the curve is far
    // smoother than that.
    let mut peak = 0.0f32;
    let mut freq = 16.0f32;
    while freq < 22_000.0 {
        peak = peak.max(eq_response_db(gains, freq));
        freq *= 2f32.powf(1.0 / 12.0);
    }
    -peak
}

/// The equalizer's state inside the callback.
pub(crate) struct Eq {
    rate: f64,
    seen_gen: u32,
    /// Where the gains are going, dB (all zero when switched off).
    target: [f32; EQ_BANDS],
    target_pre: f32,
    /// Where they are.
    gain: [f32; EQ_BANDS],
    pre_db: f32,
    pre: f64,
    glide: f32,
    /// `cos(w0)` and `sin(w0) / 2Q` of every band at this rate.
    band: [(f64, f64); EQ_BANDS],
    coefs: [Coefs; EQ_BANDS],
    state: [[State; EQ_BANDS]; MAX_CHANNELS],
    /// In the path. False: the samples pass untouched.
    active: bool,
    /// Frames the flat filters still run before stepping aside.
    settle: u64,
    /// Frames of silence the filters may still ring into.
    ring_left: u64,
}

impl Eq {
    pub(crate) fn new() -> Self {
        Self {
            rate: 0.0,
            seen_gen: 0,
            target: [0.0; EQ_BANDS],
            target_pre: 0.0,
            gain: [0.0; EQ_BANDS],
            pre_db: 0.0,
            pre: 1.0,
            glide: 1.0,
            band: [(0.0, 0.0); EQ_BANDS],
            coefs: [Coefs::IDENTITY; EQ_BANDS],
            state: [[State::default(); EQ_BANDS]; MAX_CHANNELS],
            active: false,
            settle: 0,
            ring_left: 0,
        }
    }

    /// The device rate changed: the bands are laid out anew and the gains
    /// taken over at once (the stream restarts with a fade anyway).
    pub(crate) fn configure(&mut self, rate: u32) {
        self.rate = f64::from(rate.max(1));
        for (band, freq) in EQ_FREQS.iter().enumerate() {
            let (cos, sin) = angle(self.rate, f64::from(*freq));
            self.band[band] = (cos, sin / (2.0 * Q));
        }
        let step_ms = STEP_FRAMES as f64 * 1000.0 / self.rate;
        self.glide = (1.0 - (-step_ms / GLIDE_MS).exp()) as f32;
        self.gain = self.target;
        self.pre_db = self.target_pre;
        self.pre = db_to_gain(self.pre_db);
        self.update_coefs();
        self.reset();
        self.active = !self.is_flat();
    }

    /// The filters hold sound that has not rung out yet (see
    /// [`Self::ring_out`]); always so while music plays through them.
    pub(crate) fn ringing(&self) -> bool {
        self.active && self.ring_left > 0
    }


    fn ring_total(&self) -> u64 {
        (self.rate * RING_MS / 1000.0) as u64
    }

    /// Forgets what the filters hold (the output was silent meanwhile).
    pub(crate) fn reset(&mut self) {

        self.state = [[State::default(); EQ_BANDS]; MAX_CHANNELS];
    }

    fn is_flat(&self) -> bool {
        self.pre_db == 0.0 && self.gain.iter().all(|g| *g == 0.0)
    }

    fn at_target(&self) -> bool {
        self.pre_db == self.target_pre && self.gain == self.target
    }

    fn update_coefs(&mut self) {
        for band in 0..EQ_BANDS {
            let (cos, alpha) = self.band[band];
            self.coefs[band] = Coefs::peaking_at(cos, alpha, f64::from(self.gain[band]));
        }
    }

    /// Takes the newest settings, if they changed.
    fn load(&mut self, sh: &Shared) {
        let generation = sh.eq_gen.load(Ordering::Acquire);
        if generation == self.seen_gen {
            return;
        }
        self.seen_gen = generation;
        if sh.eq_on.load(Ordering::Relaxed) {
            for band in 0..EQ_BANDS {
                self.target[band] = f32::from_bits(sh.eq_gains[band].load(Ordering::Relaxed));
            }
            self.target_pre = f32::from_bits(sh.eq_preamp.load(Ordering::Relaxed));
        } else {
            self.target = [0.0; EQ_BANDS];
            self.target_pre = 0.0;
        }
    }

    /// One glide step: every gain a little closer to its target.
    fn step(&mut self) {
        let k = self.glide;
        let toward = |now: &mut f32, target: f32| {
            let d = target - *now;
            *now = if d.abs() <= SNAP_DB {
                target
            } else {
                *now + d * k
            };
        };
        for band in 0..EQ_BANDS {
            toward(&mut self.gain[band], self.target[band]);
        }
        toward(&mut self.pre_db, self.target_pre);
        self.update_coefs();
    }

    /// Equalizes interleaved frames of music in place.
    pub(crate) fn process(&mut self, sh: &Shared, seg: &mut [f32], channels: usize) {
        self.ring_left = self.ring_total();
        self.run(sh, seg, channels);
    }

    fn run(&mut self, sh: &Shared, seg: &mut [f32], channels: usize) {
        self.load(sh);
        if channels == 0 || channels > MAX_CHANNELS || self.rate == 0.0 {
            return;
        }
        if !self.active {
            if self.at_target() && self.is_flat() {
                return;
            }
            // Coming in from flat with silent filters: no step in the sound.
            self.reset();
            self.active = true;
        }
        for block in seg.chunks_mut(STEP_FRAMES * channels) {
            let pre_from = self.pre;
            if !self.at_target() {
                self.step();
                self.pre = db_to_gain(self.pre_db);
            }
            let frames = block.len() / channels;
            let pre_step = (self.pre - pre_from) / frames.max(1) as f64;
            let coefs = &self.coefs;
            for (i, frame) in block.chunks_exact_mut(channels).enumerate() {
                let pre = pre_from + pre_step * (i + 1) as f64;
                for (sample, state) in frame.iter_mut().zip(self.state.iter_mut()) {
                    let mut x = f64::from(*sample) * pre;
                    for (c, s) in coefs.iter().zip(state.iter_mut()) {
                        x = s.tick(c, x);
                    }
                    *sample = x as f32;
                }
            }
        }
        for state in self.state.iter_mut().take(channels) {
            for s in state {
                s.settle();
            }
        }
        if self.at_target() && self.is_flat() {
            // Flat now, but the filters still hold the sound that was
            // shaped: let that ring out, then step aside.
            let frames = (seg.len() / channels) as u64;
            if self.settle <= frames {
                self.settle = 0;
                self.active = false;
            } else {
                self.settle -= frames;
            }
        } else {
            self.settle = (self.rate / 2.0) as u64;
        }
    }

    /// The music stopped inside a block (a pause, the end, a dropout): what
    /// the filters still hold rings out into the silence of `seg` instead
    /// of being cut off, eased so that it is over after [`RING_MS`] (the
    /// lowest band at full boost has all but died by then). Returns false
    /// when there was nothing to add.
    pub(crate) fn ring_out(&mut self, sh: &Shared, seg: &mut [f32], channels: usize) -> bool {
        if !self.active || self.ring_left == 0 || channels == 0 {
            return false;
        }
        self.run(sh, seg, channels);
        let total = self.ring_total().max(1) as f32;
        for frame in seg.chunks_exact_mut(channels) {
            self.ring_left = self.ring_left.saturating_sub(1);
            let ease = self.ring_left as f32 / total;
            for s in frame {
                *s *= ease;
            }
        }
        true
    }
}

fn db_to_gain(db: f32) -> f64 {
    10f64.powf(f64::from(db) / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const RATE: u32 = 48_000;

    fn set(sh: &Shared, on: bool, gains: [f32; EQ_BANDS], preamp: f32) {
        for (slot, g) in sh.eq_gains.iter().zip(gains) {
            slot.store(g.to_bits(), Ordering::Relaxed);
        }
        sh.eq_preamp.store(preamp.to_bits(), Ordering::Relaxed);
        sh.eq_on.store(on, Ordering::Relaxed);
        sh.eq_gen.fetch_add(1, Ordering::AcqRel);
    }

    fn sine(freq: f64, amp: f64, start: usize, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for i in start..start + frames {
            let s = (amp * (TAU * freq * i as f64 / f64::from(RATE)).sin()) as f32;
            out.extend_from_slice(&[s, s]);
        }
        out
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |m, s| m.max(s.abs()))
    }

    fn only(band: usize, db: f32) -> [f32; EQ_BANDS] {
        let mut g = [0.0; EQ_BANDS];
        g[band] = db;
        g
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
    }

    /// Runs a sine through the equalizer in callback-sized blocks; returns
    /// everything.
    fn run(eq: &mut Eq, sh: &Shared, freq: f64, amp: f64, start: usize, frames: usize) -> Vec<f32> {
        let mut all = sine(freq, amp, start, frames);
        for block in all.chunks_mut(480 * 2) {
            eq.process(sh, block, 2);
        }
        all
    }

    #[test]
    fn a_band_lifts_a_sine_at_its_centre_by_its_gain() {
        for (band, freq) in EQ_FREQS.iter().enumerate().take(9) {
            let sh = Shared::new();
            let mut eq = Eq::new();
            eq.configure(RATE);
            set(&sh, true, only(band, 6.0), 0.0);
            let out = run(&mut eq, &sh, f64::from(*freq), 0.25, 0, RATE as usize * 2);
            let dry = sine(f64::from(*freq), 0.25, RATE as usize, RATE as usize);
            let got = 20.0 * (rms(&out[RATE as usize * 2..]) / rms(&dry)).log10();
            assert!((got - 6.0).abs() < 0.1, "{freq} Hz: {got} dB");
        }
    }

    #[test]
    fn off_or_flat_does_not_touch_the_samples() {
        let sh = Shared::new();
        let mut eq = Eq::new();
        eq.configure(RATE);
        let dry = sine(441.0, 0.7, 0, 4800);
        let mut out = dry.clone();
        eq.process(&sh, &mut out, 2);
        assert_eq!(out, dry, "never switched on");
        set(&sh, true, [0.0; EQ_BANDS], 0.0);
        eq.process(&sh, &mut out, 2);
        assert_eq!(out, dry, "on and flat");
        set(&sh, false, only(1, 9.0), -9.0);
        eq.process(&sh, &mut out, 2);
        assert_eq!(out, dry, "gains set but switched off");
    }

    #[test]
    fn switching_off_glides_to_flat_and_then_steps_aside() {
        let sh = Shared::new();
        let mut eq = Eq::new();
        eq.configure(RATE);
        set(&sh, true, only(1, 6.0), 0.0);
        run(&mut eq, &sh, 64.0, 0.25, 0, RATE as usize);
        set(&sh, false, only(1, 6.0), 0.0);
        let n = RATE as usize;
        let out = run(&mut eq, &sh, 64.0, 0.25, n, n * 2);
        assert!(!eq.active, "out of the path once it rang out");
        let dry = sine(64.0, 0.25, n * 2, n);
        assert_eq!(&out[n * 2..], &dry[..], "bit-exact afterwards");
    }

    /// The largest step between two samples of one channel.
    fn largest_step(samples: &[f32]) -> f32 {
        samples
            .windows(3)
            .map(|w| (w[2] - w[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_preset_switch_makes_no_click() {
        // A 100 Hz sine at 0.5 moves 0.0065 per sample at most; a click is
        // a step many times that.
        let sh = Shared::new();
        let mut eq = Eq::new();
        eq.configure(RATE);
        let n = RATE as usize;
        let mut heard = run(&mut eq, &sh, 100.0, 0.5, 0, n);
        let bass = [6.0, 6.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        set(&sh, true, bass, eq_preamp_db(&bass));
        heard.extend(run(&mut eq, &sh, 100.0, 0.5, n, n));
        let treble = [-6.0, -6.0, 0.0, 0.0, 0.0, 0.0, 3.0, 6.0, 9.0, 12.0];
        set(&sh, true, treble, eq_preamp_db(&treble));
        heard.extend(run(&mut eq, &sh, 100.0, 0.5, n * 2, n));
        set(&sh, false, treble, 0.0);
        heard.extend(run(&mut eq, &sh, 100.0, 0.5, n * 3, n));
        let step = largest_step(&heard);
        assert!(step < 0.012, "a click: {step}");
    }

    #[test]
    fn the_preamp_keeps_a_full_scale_sine_at_full_scale() {
        let presets: [[f32; EQ_BANDS]; 3] = [
            [0.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [12.0; EQ_BANDS],
            [12.0, 12.0, 12.0, 0.0, -12.0, -12.0, 0.0, 12.0, 12.0, 12.0],
        ];
        for gains in presets {
            let pre = eq_preamp_db(&gains);
            assert!(pre <= 0.0);
            for freq in [40.0, 64.0, 90.0, 125.0, 1000.0, 8000.0, 14_000.0] {
                let sh = Shared::new();
                let mut eq = Eq::new();
                eq.configure(RATE);
                set(&sh, true, gains, pre);
                let out = run(&mut eq, &sh, freq, 1.0, 0, RATE as usize * 2);
                let top = peak(&out[RATE as usize * 2..]);
                assert!(top <= 1.02, "{gains:?} at {freq} Hz: {top}");
            }
        }
    }

    #[test]
    fn the_curve_is_the_bands_added_up() {
        assert_eq!(eq_response_db(&[0.0; EQ_BANDS], 1000.0), 0.0);
        let two = [0.0, 6.0, 6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let between = eq_response_db(&two, 90.0);
        assert!(between > 5.0 && between < 8.5, "{between}");
        let pre = eq_preamp_db(&two);
        assert!(pre < -6.5 && pre > -9.0, "{pre}");
        assert_eq!(eq_preamp_db(&[-6.0; EQ_BANDS]), 0.0);
    }

    #[test]
    fn what_the_filters_hold_rings_out_into_a_pause() {
        let sh = Shared::new();
        let mut eq = Eq::new();
        eq.configure(RATE);
        set(&sh, true, only(0, 12.0), 0.0);
        // A whole number of periods: the dry sine ends at a zero crossing.
        let played = run(&mut eq, &sh, 32.0, 0.5, 0, RATE as usize);
        let mut silence = vec![0.0f32; 4800 * 2];
        assert!(eq.ring_out(&sh, &mut silence, 2));
        let last = played[played.len() - 2];
        assert!((silence[0] - last).abs() < 0.02, "{last} then {}", silence[0]);
        assert!(largest_step(&silence) < 0.02);
        assert!(eq.ringing());
        let mut later = vec![0.0f32; RATE as usize * 2];
        eq.ring_out(&sh, &mut later, 2);
        assert!(peak(&later[RATE as usize..]) < 1e-6, "it is over");
        assert!(!eq.ringing());
        assert!(!eq.ring_out(&sh, &mut later, 2));

    }
}
