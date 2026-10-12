//! The Expressive slider (16 dp track, 4×44 handle bar with 6 dp gaps) whose
//! active track can turn into a wave while music plays, and the wavy linear
//! progress indicator.

use std::f32::consts::TAU;

use egui::{
    Align2, Color32, Id, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, epaint::PathShape, pos2,
    vec2,
};

use super::{anim, paint, paint::Radii};
use crate::{Theme, TypeRole, tokens::shape};

/// Wave geometry (wavy progress: amplitude 3, wavelength 40).
#[derive(Clone, Copy, Debug)]
pub struct Wave {
    pub amplitude: f32,
    pub wavelength: f32,
    /// Phase speed in wavelengths per second.
    pub speed: f32,
}

impl Default for Wave {
    fn default() -> Self {
        Self {
            amplitude: 3.0,
            wavelength: 40.0,
            speed: 0.6,
        }
    }
}

/// Result of a slider frame.
pub struct SliderOutput {
    pub response: Response,
    /// The value under the pointer while dragging (for a live label).
    pub dragging: Option<f32>,
    /// Set once, when the user releases (or clicks) at a new value.
    pub committed: Option<f32>,
}

/// A horizontal slider over `0..=1`.
pub struct Slider {
    id: Id,
    value: f32,
    track: f32,
    width: f32,
    wavy: bool,
    wave: Wave,
    label: Option<Box<dyn Fn(f32) -> String>>,
    active: Option<Color32>,
    inactive: Option<Color32>,
}

impl Slider {
    pub fn new(id_salt: impl egui::AsId, value: f32) -> Self {
        Self {
            id: Id::new(id_salt),
            value: value.clamp(0.0, 1.0),
            track: 16.0,
            width: f32::INFINITY,
            wavy: false,
            wave: Wave::default(),
            label: None,
            active: None,
            inactive: None,
        }
    }

    /// Track thickness (16 for XS).
    pub fn track(mut self, t: f32) -> Self {
        self.track = t;
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }

    /// Turns the active part into a wave (animated in and out).
    pub fn wavy(mut self, wavy: bool) -> Self {
        self.wavy = wavy;
        self
    }

    pub fn wave(mut self, wave: Wave) -> Self {
        self.wave = wave;
        self
    }

    /// Text in the value indicator shown while dragging.
    pub fn value_label(mut self, f: impl Fn(f32) -> String + 'static) -> Self {
        self.label = Some(Box::new(f));
        self
    }

    pub fn colors(mut self, active: Color32, inactive: Color32) -> Self {
        self.active = Some(active);
        self.inactive = Some(inactive);
        self
    }

    pub fn show(self, ui: &mut Ui) -> SliderOutput {
        let theme = Theme::get(ui.ctx());
        let s = &theme.scheme;
        let ctx = ui.ctx().clone();
        let width = if self.width.is_finite() {
            self.width
        } else {
            ui.available_width()
        };
        let height = 44.0;
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let id = ui.id().with(self.id);
        let response = ui.interact(rect, id, Sense::click_and_drag());

        let to_value = |p: Pos2| ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let mut dragging = None;
        let mut committed = None;
        if let Some(p) = response.interact_pointer_pos()
            && (response.dragged() || response.is_pointer_button_down_on())
        {
            dragging = Some(to_value(p));
        }
        if (response.drag_stopped() || response.clicked())
            && let Some(p) = response.interact_pointer_pos()
        {
            committed = Some(to_value(p));
        }
        let value = dragging.or(committed).unwrap_or(self.value);

        if ui.is_rect_visible(rect) {
            let active = self.active.unwrap_or(s.primary);
            let inactive = self.inactive.unwrap_or(s.secondary_container);
            let pressed = dragging.is_some();
            let handle_w = anim::f32(
                &ctx,
                id.with("hw"),
                if pressed { 2.0 } else { 4.0 },
                theme.motion.fast_spatial,
            );
            let wave_k = anim::fraction(
                &ctx,
                id.with("wave"),
                if self.wavy && !theme.reduced_motion { 1.0 } else { 0.0 },
                theme.motion.default_spatial,
            )
            .max(0.0);
            let gap = 6.0;
            let cy = rect.center().y;
            let x = rect.left() + value * rect.width();
            let t = self.track;

            // Inactive track (right of the handle).
            let right = Rect::from_min_max(
                pos2((x + gap + handle_w * 0.5).min(rect.right()), cy - t * 0.5),
                pos2(rect.right(), cy + t * 0.5),
            );
            if right.width() > 0.5 {
                paint::container(
                    ui.painter(),
                    right,
                    Radii([2.0, shape::FULL, shape::FULL, 2.0]),
                    inactive,
                    Stroke::NONE,
                );
                // Stop indicator.
                if right.width() > 12.0 {
                    ui.painter().circle_filled(
                        pos2(rect.right() - t * 0.5, cy),
                        2.0,
                        active,
                    );
                }
            }

            // Active track (left of the handle): a bar that turns into a wave.
            let left_end = (x - gap - handle_w * 0.5).max(rect.left());
            if left_end - rect.left() > 0.5 {
                if wave_k > 0.01 {
                    let thickness = t + (6.0 - t) * wave_k.min(1.0);
                    let amp = self.wave.amplitude * wave_k;
                    let phase = (anim::now(&ctx) as f32 * self.wave.speed).fract() * TAU;
                    wave_line(
                        ui,
                        rect.left() + thickness * 0.5,
                        left_end - thickness * 0.5,
                        cy,
                        amp,
                        self.wave.wavelength,
                        phase,
                        Stroke::new(thickness, active),
                    );
                    if self.wavy {
                        ctx.request_repaint();
                    }
                } else {
                    let bar = Rect::from_min_max(
                        pos2(rect.left(), cy - t * 0.5),
                        pos2(left_end, cy + t * 0.5),
                    );
                    paint::container(
                        ui.painter(),
                        bar,
                        Radii([shape::FULL, 2.0, 2.0, shape::FULL]),
                        active,
                        Stroke::NONE,
                    );
                }
            }

            // Handle bar.
            let handle = Rect::from_center_size(pos2(x, cy), vec2(handle_w, height));
            paint::container(ui.painter(), handle, Radii::all(shape::FULL), active, Stroke::NONE);

            // Value indicator above the handle while dragging.
            if let (true, Some(label)) = (pressed, &self.label) {
                let text = label(value);
                let font = TypeRole::LabelLarge.font_id();
                let galley = ui.painter().layout_no_wrap(text, font, s.inverse_on_surface);
                let size = galley.size() + vec2(32.0, 20.0);
                let bubble = Rect::from_center_size(
                    pos2(x, rect.top() - 12.0 - size.y * 0.5),
                    vec2(size.x.max(48.0), size.y.max(44.0)),
                );
                let layer = ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    id.with("indicator"),
                ));
                paint::container(&layer, bubble, Radii::all(shape::FULL), s.inverse_surface, Stroke::NONE);
                layer.galley(
                    Align2::CENTER_CENTER
                        .anchor_size(bubble.center(), galley.size())
                        .min,
                    galley,
                    s.inverse_on_surface,
                );
            }
        }
        SliderOutput {
            response,
            dragging,
            committed,
        }
    }
}

/// A sine polyline from `x0` to `x1` around `cy`, with round caps. The wave
/// is anchored at `x1` (the handle) and runs away from it as `phase` grows,
/// so it seems to come out of the handle.
#[allow(clippy::too_many_arguments)]
pub fn wave_line(
    ui: &Ui,
    x0: f32,
    x1: f32,
    cy: f32,
    amplitude: f32,
    wavelength: f32,
    phase: f32,
    stroke: Stroke,
) {
    if x1 <= x0 {
        ui.painter().circle_filled(pos2(x0, cy), stroke.width * 0.5, stroke.color);
        return;
    }
    let step = 2.0;
    let n = ((x1 - x0) / step).ceil().max(1.0) as usize;
    let k = TAU / wavelength;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let x = x0 + (x1 - x0) * i as f32 / n as f32;
            pos2(x, cy + amplitude * (k * (x1 - x) - phase).sin())
        })
        .collect();
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    ui.painter().add(Shape::Path(PathShape::line(pts, stroke)));
    ui.painter().circle_filled(first, stroke.width * 0.5, stroke.color);
    ui.painter().circle_filled(last, stroke.width * 0.5, stroke.color);
}

/// The wavy linear progress indicator (4 dp, gap 4, stop dot). `fraction` in 0..1.
pub fn wavy_progress(ui: &mut Ui, id_salt: impl egui::AsId, fraction: f32, width: f32) -> Response {
    let theme = Theme::get(ui.ctx());
    let s = &theme.scheme;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 10.0), Sense::hover());
    if ui.is_rect_visible(rect) {
        let id = ui.id().with(id_salt);
        let f = anim::fraction(ui.ctx(), id, fraction.clamp(0.0, 1.0), theme.motion.default_effects);
        let cy = rect.center().y;
        let thick = 4.0;
        let x = rect.left() + f * rect.width();
        let amp = if theme.reduced_motion { 0.0 } else { 3.0 };
        let phase = (anim::now(ui.ctx()) as f32 * 0.6).fract() * TAU;
        wave_line(
            ui,
            rect.left() + thick * 0.5,
            (x - thick * 0.5).max(rect.left() + thick * 0.5),
            cy,
            amp,
            40.0,
            phase,
            Stroke::new(thick, s.primary),
        );
        let track_start = x + 4.0 + thick * 0.5;
        if track_start < rect.right() - thick * 0.5 {
            ui.painter().line_segment(
                [pos2(track_start, cy), pos2(rect.right() - thick * 0.5, cy)],
                Stroke::new(thick, s.secondary_container),
            );
            ui.painter().circle_filled(pos2(rect.right() - 2.0, cy), 2.0, s.primary);
        }
        if amp > 0.0 && f > 0.0 && f < 1.0 {
            ui.ctx().request_repaint();
        }
    }
    response
}
