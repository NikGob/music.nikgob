//! Mini equalizer: four bars that dance while a track plays and settle into
//! a static glyph when paused. Driven by the clock, not a spring, so it never
//! restarts; under reduced motion (a looping animation) the bars stay still.

use egui::{Color32, Id, Rect, Ui, pos2, vec2};

use super::{anim, paint, paint::Radii};
use crate::Theme;

/// Bar heights (fraction of the glyph) at rest.
const REST: [f32; 4] = [0.45, 0.8, 0.6, 0.35];
/// (rad/s, phase) pairs of the two sines that drive each bar.
const WAVES: [[(f32, f32); 2]; 4] = [
    [(7.1, 0.0), (11.3, 1.3)],
    [(8.9, 2.1), (13.7, 0.4)],
    [(6.3, 4.0), (10.1, 2.7)],
    [(9.7, 5.2), (12.9, 3.6)],
];

/// A 24 dp equalizer glyph.
#[must_use = "call .paint_at(ui, rect)"]
pub struct Equalizer {
    id: Id,
    color: Color32,
    size: f32,
    playing: bool,
}

impl Equalizer {
    pub fn new(id_salt: impl egui::AsId, color: Color32) -> Self {
        Self {
            id: Id::new(id_salt),
            color,
            size: 24.0,
            playing: true,
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Dances while true; settles to the static glyph when false.
    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = playing;
        self
    }

    /// Paints the glyph centred in `rect`.
    pub fn paint_at(self, ui: &Ui, rect: Rect) {
        let ctx = ui.ctx();
        let theme = Theme::get(ctx);
        let live = self.playing && !theme.reduced_motion;
        let k = anim::fraction(
            ctx,
            ui.id().with(self.id),
            if live { 1.0 } else { 0.0 },
            theme.motion.fast_effects,
        )
        .clamp(0.0, 1.0);
        // Out of sight (a row below the window) it asks for no frames.
        if live && ui.is_rect_visible(rect) {
            ctx.request_repaint();
        }
        let t = anim::now(ctx) as f32;
        let n = REST.len() as f32;
        let (w, gap) = (self.size / 7.0, self.size / 10.0);
        let total = n * w + (n - 1.0) * gap;
        let max_h = self.size * 0.78;
        let min_h = self.size * 0.18;
        let base = rect.center().y + max_h * 0.5;
        let left = rect.center().x - total * 0.5;
        for (i, rest) in REST.iter().enumerate() {
            let [(f1, p1), (f2, p2)] = WAVES[i];
            let wave = 0.5 + 0.5 * (0.6 * (t * f1 + p1).sin() + 0.4 * (t * f2 + p2).sin());
            let frac = rest + (wave - rest) * k;
            let h = min_h + (max_h - min_h) * frac;
            let x = left + i as f32 * (w + gap);
            let bar = Rect::from_min_max(pos2(x, base - h), pos2(x, base) + vec2(w, 0.0));
            paint::container(
                ui.painter(),
                bar,
                Radii::all(w * 0.5),
                self.color,
                egui::Stroke::NONE,
            );
        }
    }
}
