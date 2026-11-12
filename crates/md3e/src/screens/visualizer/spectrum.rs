//! «Спектр»: the spectrum as a row of round-ended bars mirrored up and
//! down a centre line, lows in the middle and highs out to both sides. A
//! bar lifts its neighbours a little (so the row reads as one line, not
//! noise), falls back with gravity, and leaves a peak dot that holds a
//! moment and then drops. Silence is a row of dots.

use egui::{CornerRadius, Painter, Rect, pos2, vec2};

use super::signal::Signal;
use crate::ColorScheme;

/// Most bars on one side of the centre.
const MAX: usize = 48;
const GRAVITY: f32 = 3.0;
/// Seconds a peak dot stays up before it falls.
const HOLD: f32 = 0.35;

pub struct Spectrum {
    shown: [f32; MAX],
    fall: [f32; MAX],
    peak: [f32; MAX],
    peak_v: [f32; MAX],
    hold: [f32; MAX],
    /// Bars on one side, as last laid out.
    half: usize,
}

impl Default for Spectrum {
    fn default() -> Self {
        Self { shown: [0.0; MAX], fall: [0.0; MAX], peak: [0.0; MAX], peak_v: [0.0; MAX], hold: [0.0; MAX], half: 24 }
    }
}

/// Bars on one side for a picture `width` wide: one every ~18 points.
pub fn half_for(width: f32) -> usize {
    ((width * 0.42 / 18.0) as usize).clamp(12, MAX)
}

impl Spectrum {
    pub fn update(&mut self, sig: &Signal, width: f32, reduced: bool) {
        let dt = sig.dt;
        self.half = half_for(width);
        let n = self.half;
        let mut target = [0.0f32; MAX];
        for (i, t) in target.iter_mut().take(n).enumerate() {
            *t = if reduced { 0.0 } else { Signal::at(&sig.bands, i as f32 / (n - 1) as f32).powf(1.3) };
        }
        // A bar lifts its neighbours, less the further they are.
        let raw = target;
        for i in 0..n {
            let mut k = raw[i];
            for t in target.iter_mut().take(n).skip(i + 1) {
                k /= 1.6;
                if k < 0.01 {
                    break;
                }
                *t = t.max(k);
            }
            let mut k = raw[i];
            for t in target[..i].iter_mut().rev() {
                k /= 1.6;
                if k < 0.01 {
                    break;
                }
                *t = t.max(k);
            }
        }
        for (i, &t) in target.iter().enumerate().take(n) {
            if t >= self.shown[i] {
                self.shown[i] = t;
                self.fall[i] = 0.0;
            } else {
                self.fall[i] += GRAVITY * dt;
                self.shown[i] = (self.shown[i] - self.fall[i] * dt).max(t);
            }
            if self.shown[i] >= self.peak[i] {
                self.peak[i] = self.shown[i];
                self.peak_v[i] = 0.0;
                self.hold[i] = HOLD;
            } else if self.hold[i] > 0.0 {
                self.hold[i] -= dt;
            } else {
                self.peak_v[i] += GRAVITY * 0.6 * dt;
                self.peak[i] = (self.peak[i] - self.peak_v[i] * dt).max(self.shown[i]);
            }
        }
    }

    pub fn paint(&self, painter: &Painter, rect: Rect, s: &ColorScheme) {
        let n = self.half;
        let width = rect.width() * 0.84;
        let pitch = width / (2 * n) as f32;
        let bar = (pitch * 0.56).max(3.0);
        let cy = rect.center().y;
        let reach = rect.height() * 0.3;
        let radius = CornerRadius::same((bar * 0.5).round().min(255.0) as u8);
        for side in [-1.0f32, 1.0] {
            for i in 0..n {
                let x = rect.center().x + side * (i as f32 + 0.5) * pitch;
                let h = bar + 2.0 * reach * self.shown[i];
                painter.rect_filled(Rect::from_center_size(pos2(x, cy), vec2(bar, h)), radius, s.primary);
                // The peak, a dot clear of the bar's end, up and down.
                let lift = self.peak[i] - self.shown[i];
                if self.peak[i] > 0.02 && lift > 0.005 {
                    let off = bar * 0.5 + reach * self.peak[i] + bar * 0.9;
                    for dy in [-off, off] {
                        painter.circle_filled(pos2(x, cy + dy), bar * 0.4, s.tertiary);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_rise_at_once_and_fall_with_gravity_under_a_held_peak() {
        let mut sp = Spectrum::default();
        let mut sig = Signal::default();
        (sig.dt, sig.bands) = (1.0 / 60.0, vec![1.0; 64]);
        sp.update(&sig, 1200.0, false);
        assert!(sp.shown[..sp.half].iter().all(|v| (*v - 1.0).abs() < 1e-5));
        sig.bands.fill(0.0);
        sp.update(&sig, 1200.0, false);
        assert!(sp.shown[0] > 0.9, "gravity starts slow");
        for _ in 0..10 {
            sp.update(&sig, 1200.0, false);
        }
        assert!(sp.peak[0] > 0.99, "the peak holds");
        for _ in 0..120 {
            sp.update(&sig, 1200.0, false);
        }
        assert!(sp.shown[0] < 1e-3 && sp.peak[0] < 1e-3);
    }

    #[test]
    fn the_row_fits_the_width() {
        assert_eq!(half_for(300.0), 12);
        assert_eq!(half_for(1280.0), 29);
        assert_eq!(half_for(5000.0), MAX);
    }
}
