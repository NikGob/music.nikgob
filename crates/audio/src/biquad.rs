//! Second-order filter sections (the Audio EQ Cookbook's formulas) for the
//! equalizer and the per-track tone.
//!
//! Coefficients and state are `f64` and the section runs in transposed
//! direct form II: a 32 Hz band at 48 kHz has its poles a hair inside the
//! unit circle, where `f32` state turns rounding into audible noise.

use std::f64::consts::TAU;

/// Normalized coefficients (`a0` is 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Coefs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coefs {
    /// Passes the signal through unchanged.
    pub(crate) const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalized(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }

    /// A bell of `gain_db` around `freq` with quality `q`.
    pub(crate) fn peaking(rate: f64, freq: f64, q: f64, gain_db: f64) -> Self {
        let (cos, sin) = angle(rate, freq);
        Self::peaking_at(cos, sin / (2.0 * q), gain_db)
    }

    /// [`Self::peaking`] from the band's precomputed `cos(w0)` and
    /// `sin(w0) / 2Q` (they do not change with the gain).
    pub(crate) fn peaking_at(cos: f64, alpha: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        Self::normalized(
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        )
    }

    /// A shelf lifting (or lowering) everything below `freq` by `gain_db`,
    /// with the widest slope that has no bump (S = 1).
    pub(crate) fn low_shelf(rate: f64, freq: f64, gain_db: f64) -> Self {
        let (cos, sin) = angle(rate, freq);
        let a = 10f64.powf(gain_db / 40.0);
        let k = 2.0 * a.sqrt() * (sin / 2.0 * std::f64::consts::SQRT_2);
        Self::normalized(
            [
                a * ((a + 1.0) - (a - 1.0) * cos + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - k),
            ],
            [
                (a + 1.0) + (a - 1.0) * cos + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - k,
            ],
        )
    }

    /// A shelf lifting (or lowering) everything above `freq` by `gain_db`.
    pub(crate) fn high_shelf(rate: f64, freq: f64, gain_db: f64) -> Self {
        let (cos, sin) = angle(rate, freq);
        let a = 10f64.powf(gain_db / 40.0);
        let k = 2.0 * a.sqrt() * (sin / 2.0 * std::f64::consts::SQRT_2);
        Self::normalized(
            [
                a * ((a + 1.0) + (a - 1.0) * cos + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - k),
            ],
            [
                (a + 1.0) - (a - 1.0) * cos + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - k,
            ],
        )
    }

    /// Gain at `freq`, as a power ratio.
    pub(crate) fn power_at(&self, rate: f64, freq: f64) -> f64 {
        let w = TAU * freq / rate;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        (num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)
    }
}

/// `cos(w0)` and `sin(w0)` of `freq` at `rate`, kept below Nyquist.
pub(crate) fn angle(rate: f64, freq: f64) -> (f64, f64) {
    let w = TAU * freq.min(rate * 0.45) / rate;
    (w.cos(), w.sin())
}

/// The two delay elements of one section for one channel.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct State {
    s1: f64,
    s2: f64,
}

impl State {
    #[inline]
    pub(crate) fn tick(&mut self, c: &Coefs, x: f64) -> f64 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Drops a state that has decayed to nothing, before it reaches the
    /// denormal range (where every operation costs a hundred times more).
    #[inline]
    pub(crate) fn settle(&mut self) {
        if self.s1.abs() < 1e-30 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-30 {
            self.s2 = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(power: f64) -> f64 {
        10.0 * power.log10()
    }

    #[test]
    fn a_bell_has_its_gain_at_the_centre_and_none_far_away() {
        let c = Coefs::peaking(48_000.0, 1000.0, 1.41, 6.0);
        assert!((db(c.power_at(48_000.0, 1000.0)) - 6.0).abs() < 0.01);
        assert!(db(c.power_at(48_000.0, 30.0)).abs() < 0.05);
        assert!(db(c.power_at(48_000.0, 20_000.0)).abs() < 0.1);
    }

    #[test]
    fn shelves_reach_their_gain_on_their_side_only() {
        let low = Coefs::low_shelf(48_000.0, 100.0, 4.0);
        assert!((db(low.power_at(48_000.0, 10.0)) - 4.0).abs() < 0.1);
        assert!(db(low.power_at(48_000.0, 5000.0)).abs() < 0.05);
        let high = Coefs::high_shelf(48_000.0, 7000.0, -3.0);
        assert!((db(high.power_at(48_000.0, 22_000.0)) + 3.0).abs() < 0.1);
        assert!(db(high.power_at(48_000.0, 200.0)).abs() < 0.05);
    }

    #[test]
    fn a_flat_bell_is_the_identity_bit_for_bit() {
        let c = Coefs::peaking(44_100.0, 32.0, 1.41, 0.0);
        let mut s = State::default();
        for i in 0..1000 {
            let x = f64::from((i as f32 * 0.37).sin());
            assert_eq!(s.tick(&c, x), x);
        }
    }

    #[test]
    fn the_lowest_band_stays_quiet_on_silence_after_a_loud_note() {
        // The noise of a marginal filter shows as a tail that never dies.
        let c = Coefs::peaking(96_000.0, 32.0, 1.41, 12.0);
        let mut s = State::default();
        for i in 0..96_000 {
            s.tick(&c, (TAU * 32.0 * f64::from(i) / 96_000.0).sin());
        }
        let mut last = 1.0f64;
        for _ in 0..96_000 * 2 {
            last = s.tick(&c, 0.0);
        }
        assert!(last.abs() < 1e-9, "{last}");
    }
}
