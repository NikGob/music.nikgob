//! Loading indicator: a 48 dp container with a 38 dp shape that morphs
//! through a sequence of Material shapes while rotating. Evaluated from the
//! clock, so it never restarts. Use it for indeterminate waits under 5 s.
//! With reduced motion it shows one static shape with a slow opacity pulse.

use std::{f32::consts::TAU, sync::OnceLock};

use egui::{Rect, Response, Sense, Stroke, Ui, Vec2, Widget, WidgetInfo, WidgetType, vec2};
use nikgob_shapes::{MaterialShape, Morph, paint::fill_shape};

use super::{anim, paint, paint::Radii};
use crate::{Theme, tokens::shape};

pub const CONTAINER: f32 = 48.0;
pub const ACTIVE: f32 = 38.0;
/// Seconds each shape is shown (morph included).
const STEP: f32 = 0.65;

/// The shapes the indicator walks through.
pub const SEQUENCE: [MaterialShape; 7] = [
    MaterialShape::SoftBurst,
    MaterialShape::Cookie9Sided,
    MaterialShape::Pentagon,
    MaterialShape::Pill,
    MaterialShape::Sunny,
    MaterialShape::Cookie4Sided,
    MaterialShape::Oval,
];

fn morphs() -> &'static [Morph] {
    static MORPHS: OnceLock<Vec<Morph>> = OnceLock::new();
    MORPHS.get_or_init(|| {
        (0..SEQUENCE.len())
            .map(|i| {
                Morph::new(
                    SEQUENCE[i].polygon(),
                    SEQUENCE[(i + 1) % SEQUENCE.len()].polygon(),
                )
            })
            .collect()
    })
}

/// Which morph is running at time `t` (seconds) and how long it has run.
pub fn phase(t: f32) -> (usize, f32) {
    let steps = t.max(0.0) / STEP;
    let index = steps.floor() as usize % SEQUENCE.len();
    (index, steps.fract() * STEP)
}

/// A loading indicator.
#[must_use = "add it with ui.add(LoadingIndicator::new())"]
pub struct LoadingIndicator {
    contained: bool,
    size: f32,
}

impl LoadingIndicator {
    pub fn new() -> Self {
        Self {
            contained: false,
            size: CONTAINER,
        }
    }

    /// The contained variant: `primary-container` disc, `on-primary-container` shape.
    pub fn contained(mut self, contained: bool) -> Self {
        self.contained = contained;
        self
    }

    /// Overall size (the shape is 38/48 of it).
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

impl Default for LoadingIndicator {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for LoadingIndicator {
    fn ui(self, ui: &mut Ui) -> Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(self.size), Sense::hover());
        response
            .widget_info(|| WidgetInfo::labeled(WidgetType::ProgressIndicator, true, "Loading"));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let ctx = ui.ctx();
        let theme = Theme::get(ctx);
        let s = theme.scheme;
        let (bg, fg) = if self.contained {
            (Some(s.primary_container), s.on_primary_container)
        } else {
            (None, s.primary)
        };
        if let Some(bg) = bg {
            paint::container(
                ui.painter(),
                rect,
                Radii::all(shape::FULL),
                bg,
                Stroke::NONE,
            );
        }
        let t = anim::now(ctx) as f32;
        let active = self.size * ACTIVE / CONTAINER;

        if theme.reduced_motion {
            let pulse = 0.65 + 0.35 * (t * TAU * 0.5).sin();
            let cubics = SEQUENCE[0].polygon().cubics().to_vec();
            fill_shape(
                ui.painter(),
                &cubics,
                Rect::from_center_size(rect.center(), vec2(active, active)),
                0.0,
                fg.gamma_multiply(pulse),
            );
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return response;
        }

        let (index, local) = phase(t);
        let spring = anim::effective(ctx, theme.motion.default_spatial);
        // The morph progress is the spring's response to a step at the
        // start of each segment.
        let (x, _) = spring.solve(-1.0, 0.0, local);
        let progress = 1.0 + x;
        let morph = &morphs()[index];
        let cubics = morph.as_cubics(progress);
        let bounds = morph.max_bounds();
        let span = (bounds[2] - bounds[0]).max(bounds[3] - bounds[1]).max(1.0);
        let size = active / span.min(CONTAINER / ACTIVE);
        let rotation = ((index as f32 + progress) / SEQUENCE.len() as f32) * TAU;
        fill_shape(
            ui.painter(),
            &cubics,
            Rect::from_center_size(rect.center(), vec2(size, size)),
            rotation,
            fg,
        );
        ctx.request_repaint();
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_walks_the_sequence_and_wraps() {
        assert_eq!(phase(0.0).0, 0);
        assert_eq!(phase(STEP * 1.5).0, 1);
        assert_eq!(phase(STEP * 7.0 + 0.01).0, 0);
        assert!((phase(STEP * 2.5).1 - STEP * 0.5).abs() < 1e-4);
        assert_eq!(phase(-3.0).0, 0);
    }

    #[test]
    fn every_pair_morphs() {
        assert_eq!(morphs().len(), SEQUENCE.len());
        for m in morphs() {
            assert!(!m.as_cubics(0.5).is_empty());
        }
    }
}
