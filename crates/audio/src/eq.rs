//! The ten-band equalizer of the audio callback.
//!
//! Ten octave-wide bells (32 Hz to 16 kHz) computed for the device rate, in
//! series, per channel. It runs in the callback, next to the volume, so a
//! dragged point is heard on the next block rather than after the buffered
//! second.
//!
//! What is set is the curve, not the bells: neighbouring bells overlap, so
//! each one's own gain is fitted ([`eq_filter_gains`]) until all ten
//! together have, at every band's centre, the gain asked for that band.
//!
//! Real-time rules hold: fixed arrays, no locks, no allocation. The control
//! side writes the wanted gains to atomics and bumps a generation; the
//! callback picks them up, fits the bells to them for its own rate (a few
//! thousand multiplies, once a change) and *glides* there: every
//! [`STEP_FRAMES`] frames each band's gain moves a little towards its target and its coefficients
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

use crate::biquad::{Coefs, State, angle, point};
use crate::shared::Shared;

/// Number of bands.
pub const EQ_BANDS: usize = 10;
/// Centre frequency of each band, Hz.
pub const EQ_FREQS: [f32; EQ_BANDS] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0,
];
/// The farthest the curve goes either way at a band's centre, dB.
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
/// Rate the preamp and the drawn curve are worked out at (between the
/// centres the curve barely depends on it).
const NOMINAL_RATE: f64 = 48_000.0;
/// The farthest a bell is driven to hold the curve, dB. A band up between
/// two that are down needs more than its own gain; the steepest setting
/// asks for about 20.
const FIT_MAX_DB: f64 = 24.0;
/// Rounds a fit takes at most (it is done in 3 to 14).
const FIT_ROUNDS: usize = 30;
/// Share of what is missing that a round adds: the neighbours answer the
/// same miss, so the whole of it overshoots and takes longer to settle.
const FIT_STEP: f64 = 0.7;
/// Closer than this to the curve at every centre (dB), the fit is done.
const FIT_CLOSE_DB: f64 = 0.01;

fn clean(gains: &[f32; EQ_BANDS]) -> [f32; EQ_BANDS] {
    gains.map(|g| {
        if g.is_finite() {
            g.clamp(-EQ_MAX_DB, EQ_MAX_DB)
        } else {
            0.0
        }
    })
}

/// The ten bands laid out for one rate.
#[derive(Clone, Copy)]
struct Layout {
    /// `cos(w0)` and `sin(w0) / 2Q` of every band.
    band: [(f64, f64); EQ_BANDS],
    /// Every band's centre on the unit circle (see [`point`]).
    centre: [[f64; 4]; EQ_BANDS],
}

impl Layout {
    /// No rate yet.
    const NONE: Self = Self {
        band: [(0.0, 0.0); EQ_BANDS],
        centre: [[0.0; 4]; EQ_BANDS],
    };

    fn new(rate: f64) -> Self {
        let mut layout = Self::NONE;
        for (band, freq) in EQ_FREQS.iter().enumerate() {
            let (cos, sin) = angle(rate, f64::from(*freq));
            layout.band[band] = (cos, sin / (2.0 * Q));
            // Where `angle` keeps the band: under Nyquist.
            layout.centre[band] = point(rate, f64::from(*freq).min(rate * 0.45));
        }
        layout
    }

    fn coefs(&self, gain: &[f64; EQ_BANDS]) -> [Coefs; EQ_BANDS] {
        std::array::from_fn(|band| {
            let (cos, alpha) = self.band[band];
            Coefs::peaking_at(cos, alpha, gain[band])
        })
    }

    /// The gain of all ten bells together at a point of the unit circle, dB.
    fn db_on(coefs: &[Coefs; EQ_BANDS], at: &[f64; 4]) -> f64 {
        let power: f64 = coefs.iter().map(|c| c.power_on(at)).product();
        10.0 * power.log10()
    }

    /// The bells' gains that put the curve on `curve` at every centre.
    ///
    /// Each round reads the curve at the centres and gives every bell a
    /// share of what its own centre misses. Deterministic, no allocation;
    /// a round is ten `powf`, ten `log10` and a hundred small products.
    fn fit(&self, curve: &[f32; EQ_BANDS]) -> [f32; EQ_BANDS] {
        let curve = clean(curve);
        if curve.iter().all(|g| *g == 0.0) {
            return [0.0; EQ_BANDS];
        }
        let mut gain = curve.map(f64::from);
        for _ in 0..FIT_ROUNDS {
            let coefs = self.coefs(&gain);
            let mut next = gain;
            let mut worst = 0.0f64;
            for band in 0..EQ_BANDS {
                let miss = f64::from(curve[band]) - Self::db_on(&coefs, &self.centre[band]);
                worst = worst.max(miss.abs());
                next[band] = (gain[band] + FIT_STEP * miss).clamp(-FIT_MAX_DB, FIT_MAX_DB);
            }
            if worst < FIT_CLOSE_DB {
                break;
            }
            gain = next;
        }
        gain.map(|g| g as f32)
    }

    /// The gain at `freq` (Hz) of bells run at `gain`, dB.
    fn response_db(&self, rate: f64, gain: &[f32; EQ_BANDS], freq: f32) -> f32 {
        if gain.iter().all(|g| *g == 0.0) {
            return 0.0;
        }
        let coefs = self.coefs(&gain.map(f64::from));
        Self::db_on(&coefs, &point(rate, f64::from(freq))) as f32
    }
}

/// The gain each of the ten bells runs at (dB) so that all of them together
/// have, at every band's centre, the gain set for that band: the curve
/// passes through what was set. Flat stays exactly flat. Worked out for
/// 48 kHz; the callback does the same for the device's rate.
#[must_use]
pub fn eq_filter_gains(gains: &[f32; EQ_BANDS]) -> [f32; EQ_BANDS] {
    Layout::new(NOMINAL_RATE).fit(gains)
}

/// The equalizer's gain at `freq` (Hz) for these band gains, dB: the curve
/// the listener hears, for drawing it. At a band's centre it is that band's
/// gain.
#[must_use]
pub fn eq_response_db(gains: &[f32; EQ_BANDS], freq: f32) -> f32 {
    Layout::new(NOMINAL_RATE).response_db(NOMINAL_RATE, &eq_filter_gains(gains), freq)
}

/// The gain (dB, never positive) that goes before these band gains so the
/// curve's highest point sits at 0 dB: what was at full scale stays there.
/// Between two raised bands the curve can stand a little above both, so
/// this is the peak of the whole curve, not of the highest band.
#[must_use]
pub fn eq_preamp_db(gains: &[f32; EQ_BANDS]) -> f32 {
    let layout = Layout::new(NOMINAL_RATE);
    let fitted = eq_filter_gains(gains);
    let at = |freq: f32| layout.response_db(NOMINAL_RATE, &fitted, freq);
    // The centres, and twelve points an octave from 16 Hz to 22 kHz: the
    // curve is far smoother than that.
    let mut peak = EQ_FREQS.iter().fold(0.0f32, |peak, freq| peak.max(at(*freq)));
    let mut freq = 16.0f32;
    while freq < 22_000.0 {
        peak = peak.max(at(freq));
        freq *= 2f32.powf(1.0 / 12.0);
    }
    -peak
}

/// The equalizer's state inside the callback.
pub(crate) struct Eq {
    rate: f64,
    seen_gen: u32,
    /// The curve asked for, dB at every centre (all zero when switched off).
    curve: [f32; EQ_BANDS],
    /// Where the bells' gains are going, dB: fitted to the curve.
    target: [f32; EQ_BANDS],
    target_pre: f32,
    /// Where they are.
    gain: [f32; EQ_BANDS],
    pre_db: f32,
    pre: f64,
    glide: f32,
    /// The bands at this rate.
    layout: Layout,
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
            curve: [0.0; EQ_BANDS],
            target: [0.0; EQ_BANDS],
            target_pre: 0.0,
            gain: [0.0; EQ_BANDS],
            pre_db: 0.0,
            pre: 1.0,
            glide: 1.0,
            layout: Layout::NONE,
            coefs: [Coefs::IDENTITY; EQ_BANDS],
            state: [[State::default(); EQ_BANDS]; MAX_CHANNELS],
            active: false,
            settle: 0,
            ring_left: 0,
        }
    }

    /// The device rate changed: the bands are laid out anew, the bells
    /// fitted to the curve again and their gains taken over at once (the
    /// stream restarts with a fade anyway).
    pub(crate) fn configure(&mut self, rate: u32) {
        self.rate = f64::from(rate.max(1));
        self.layout = Layout::new(self.rate);
        self.target = self.layout.fit(&self.curve);
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
            let (cos, alpha) = self.layout.band[band];
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
                self.curve[band] = f32::from_bits(sh.eq_gains[band].load(Ordering::Relaxed));
            }
            self.target_pre = f32::from_bits(sh.eq_preamp.load(Ordering::Relaxed));
        } else {
            self.curve = [0.0; EQ_BANDS];
            self.target_pre = 0.0;
        }
        // Before the first rate there is nothing to fit to; `configure` does it.
        if self.rate > 0.0 {
            self.target = self.layout.fit(&self.curve);
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
    fn two_raised_neighbours_do_not_add_up_and_the_preamp_is_the_peak() {
        assert_eq!(eq_response_db(&[0.0; EQ_BANDS], 1000.0), 0.0);
        let two = [0.0, 6.0, 6.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        // Unfitted, the two bells stood at 7.7 dB between them.
        let between = eq_response_db(&two, 90.0);
        assert!(between > 4.8 && between < 6.2, "{between}");
        let pre = eq_preamp_db(&two);
        assert!(pre <= -5.98 && pre > -6.3, "{pre}");
        assert!((eq_preamp_db(&only(5, 6.0)) + 6.0).abs() < 0.02);
        assert_eq!(eq_preamp_db(&[-6.0; EQ_BANDS]), 0.0);
    }

    /// How far the curve of `layout`'s fit is from `curve` at the centres.
    fn worst_miss(layout: &Layout, curve: &[f32; EQ_BANDS]) -> f64 {
        let fitted = layout.fit(curve).map(f64::from);
        let coefs = layout.coefs(&fitted);
        (0..EQ_BANDS)
            .map(|band| (f64::from(curve[band]) - Layout::db_on(&coefs, &layout.centre[band])).abs())
            .fold(0.0, f64::max)
    }

    /// The editor's ready curves but the flat one.
    const READY: [[f32; EQ_BANDS]; 4] = [
        [0.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, -1.0, 0.0, 2.0, 2.0, 0.0, 0.0, 0.0],
        [0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0],
        [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -2.0, -3.0, 0.0],
    ];

    fn alternating(db: f32) -> [f32; EQ_BANDS] {
        std::array::from_fn(|band| if band % 2 == 0 { db } else { -db })
    }

    #[test]
    fn the_curve_passes_through_every_band_s_gain() {
        let mut curves = vec![
            only(0, 12.0),
            only(5, 6.0),
            only(5, -12.0),
            only(9, 12.0),
            alternating(6.0),
            alternating(-12.0),
            [6.0, 5.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            [12.0; EQ_BANDS],
            [-12.0; EQ_BANDS],
            [12.0, 12.0, 12.0, 0.0, -12.0, -12.0, 0.0, 12.0, 12.0, 12.0],
            [0.0, 0.0, 0.0, 12.0, -12.0, 12.0, 0.0, 0.0, 0.0, 0.0],
        ];
        curves.extend(READY);
        for rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let layout = Layout::new(rate);
            for curve in &curves {
                let miss = worst_miss(&layout, curve);
                assert!(miss < 0.1, "{curve:?} at {rate} Hz: {miss} dB off");
                // No setting within the range drives a bell to the stop.
                let fitted = layout.fit(curve);
                assert!(fitted.iter().all(|g| f64::from(g.abs()) < FIT_MAX_DB), "{curve:?}: {fitted:?}");
            }
        }
        // The public curve is the same thing at the nominal rate.
        for curve in &curves {
            for (band, freq) in EQ_FREQS.iter().enumerate() {
                let got = eq_response_db(curve, *freq);
                assert!((got - curve[band]).abs() < 0.1, "{curve:?} at {freq} Hz: {got}");
            }
        }
    }

    #[test]
    fn a_flat_curve_is_fitted_with_no_gain_at_all() {
        assert_eq!(eq_filter_gains(&[0.0; EQ_BANDS]), [0.0; EQ_BANDS]);
        assert_eq!(eq_filter_gains(&[f32::NAN; EQ_BANDS]), [0.0; EQ_BANDS]);
        assert_eq!(Layout::new(44_100.0).fit(&[0.0; EQ_BANDS]), [0.0; EQ_BANDS]);
    }

    #[test]
    fn a_fit_moves_the_neighbours_of_a_raised_band_the_other_way() {
        let fitted = eq_filter_gains(&only(5, 6.0));
        assert!(fitted[5] > 6.2 && fitted[5] < 6.7, "{fitted:?}");
        assert!(fitted[4] < -1.0 && fitted[4] > -1.4, "{fitted:?}");
        assert!(fitted[6] < -1.0 && fitted[6] > -1.4, "{fitted:?}");
        assert!(fitted[0].abs() < 0.01 && fitted[9].abs() < 0.01, "{fitted:?}");
        // The numbers the editor's own copy of the fit is held to as well.
        let bass = eq_filter_gains(&READY[0]);
        for (bell, expected) in bass.iter().zip([-0.76, 3.89, 1.33, -0.38, -0.01]) {
            assert!((bell - expected).abs() < 0.03, "{bass:?}");
        }
        // The steepest setting asks the most of the bells, and gets it.
        let steep = eq_filter_gains(&alternating(12.0));
        let most = steep.iter().fold(0.0f32, |m, g| m.max(g.abs()));
        assert!(most > 19.0 && most < 21.5, "{steep:?}");
    }

    #[test]
    fn the_ready_curves_are_fitted_the_same_every_time_and_stay_near_themselves() {
        for curve in READY {
            let fitted = eq_filter_gains(&curve);
            assert_eq!(fitted, eq_filter_gains(&curve));
            for (bell, set) in fitted.iter().zip(curve) {
                assert!((bell - set).abs() < 1.0, "{curve:?}: {fitted:?}");
            }
            assert!(worst_miss(&Layout::new(NOMINAL_RATE), &curve) < 0.02);
            // Nothing stands above the highest band by more than a hair.
            let top = curve.iter().fold(0.0f32, |m, g| m.max(*g));
            let pre = eq_preamp_db(&curve);
            assert!(pre <= -top + 0.02 && pre > -top - 0.1, "{curve:?}: {pre}");
        }
    }

    #[test]
    fn a_raised_band_leaves_a_sine_at_its_neighbour_s_centre_alone() {
        let sh = Shared::new();
        let mut eq = Eq::new();
        eq.configure(RATE);
        set(&sh, true, only(5, 6.0), 0.0);
        let out = run(&mut eq, &sh, 500.0, 0.25, 0, RATE as usize * 2);
        let dry = sine(500.0, 0.25, RATE as usize, RATE as usize);
        let got = 20.0 * (rms(&out[RATE as usize * 2..]) / rms(&dry)).log10();
        // The bell alone has 1.5 dB there.
        assert!(got.abs() < 0.1, "{got} dB");
    }

    #[test]
    fn a_new_rate_fits_the_curve_again() {
        let sh = Shared::new();
        let mut eq = Eq::new();
        // Set before the device has a rate: fitted once it has.
        let curve = [12.0; EQ_BANDS];
        set(&sh, true, curve, eq_preamp_db(&curve));
        eq.process(&sh, &mut [0.0f32; 64], 2);
        eq.configure(RATE);
        assert_eq!(eq.target, Layout::new(48_000.0).fit(&curve));
        assert_eq!(eq.gain, eq.target);
        eq.configure(96_000);
        assert_eq!(eq.target, Layout::new(96_000.0).fit(&curve));
        assert_ne!(eq.target, Layout::new(48_000.0).fit(&curve));
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
