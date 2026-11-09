//! The Expressive slider (16 dp track, 4×44 handle bar with 6 dp gaps) whose
//! active track can turn into a wave while music plays, and the wavy linear
//! progress indicator.

use std::f32::consts::TAU;

use egui::{
    Align2, Color32, Context, Id, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, epaint::PathShape, pos2,
    vec2,
};

use super::{anim, paint, paint::Radii};
use crate::{Theme, TypeRole, tokens::shape};

/// Thickness of the active track while it is a wave (a bar is the track's).
const WAVE_THICKNESS: f32 = 6.0;

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
            // No bounce: a spatial spring undershoots 0 on pause and would
            // have to be clamped there, which cuts the flattening short.
            // With less motion the wave stays, still: «Волна на полосе
            // прогресса» shows either way, only its travel stops.
            let running = self.wavy && !theme.reduced_motion;
            let wave_k = anim::fraction(
                &ctx,
                id.with("wave"),
                if self.wavy { 1.0 } else { 0.0 },
                theme.motion.slow_effects,
            )
            .clamp(0.0, 1.0);
            let phase = wave_phase(&ctx, id.with("phase"), running, self.wave.speed);
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
            // One shape for the whole transition: thickness, amplitude and
            // the corner by the handle all follow `wave_k`, so pausing flattens
            // it without a pop where a bar would replace the wave.
            let left_end = (x - gap - handle_w * 0.5).max(rect.left());
            if left_end - rect.left() > 0.5 {
                let thickness = t + (WAVE_THICKNESS.min(t) - t) * wave_k;
                let amp = self.wave.amplitude * wave_k;
                // 2 dp inner corner as a bar, a round cap as a wave.
                let end_r = 2.0 + (thickness * 0.5 - 2.0) * wave_k;
                if amp > 0.05 {
                    wave_path(
                        ui,
                        rect.left() + thickness * 0.5,
                        left_end - thickness * 0.5,
                        cy,
                        amp,
                        self.wave.wavelength,
                        phase,
                        Stroke::new(thickness, active),
                        end_r,
                    );
                } else {
                    let bar = Rect::from_min_max(
                        pos2(rect.left(), cy - thickness * 0.5),
                        pos2(left_end, cy + thickness * 0.5),
                    );
                    paint::container(
                        ui.painter(),
                        bar,
                        Radii([thickness * 0.5, end_r, end_r, thickness * 0.5]),
                        active,
                        Stroke::NONE,
                    );
                }
                if running {
                    ctx.request_repaint();
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
    wave_path(ui, x0, x1, cy, amplitude, wavelength, phase, stroke, stroke.width * 0.5);
}

/// [`wave_line`] whose end at `x1` has corners of radius `end_radius` (up to
/// half the stroke width, which is the round cap).
#[allow(clippy::too_many_arguments)]
fn wave_path(
    ui: &Ui,
    x0: f32,
    x1: f32,
    cy: f32,
    amplitude: f32,
    wavelength: f32,
    phase: f32,
    stroke: Stroke,
    end_radius: f32,
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
    let half = stroke.width * 0.5;
    ui.painter().add(Shape::Path(PathShape::line(pts, stroke)));
    ui.painter().circle_filled(first, half, stroke.color);
    if end_radius >= half - 0.01 {
        ui.painter().circle_filled(last, half, stroke.color);
    } else {
        let cap = Rect::from_min_max(pos2(last.x, last.y - half), pos2(last.x + half, last.y + half));
        let r = end_radius.max(0.0);
        paint::container(ui.painter(), cap, Radii([0.0, r, r, 0.0]), stroke.color, Stroke::NONE);
    }
}

/// The wave's phase (0..TAU) of the slider `id`. It advances while `running`
/// and holds its value otherwise, so a wave that is flattening by a pause
/// stays where it was instead of running on, and the next play continues
/// from the same crest.
fn wave_phase(ctx: &Context, id: Id, running: bool, speed: f32) -> f32 {
    let now = anim::now(ctx);
    ctx.data_mut(|d| {
        let (turns, last) = d.get_temp_mut_or_insert_with(id, || (0.0_f64, now));
        // A frame that comes late (a minimized window) must not spin the wave.
        let dt = (now - *last).clamp(0.0, 0.1);
        if running {
            *turns = (*turns + dt * f64::from(speed)).rem_euclid(1.0);
        }
        *last = now;
        *turns as f32 * TAU
    })
}

/// The wavy linear progress indicator (4 dp, gap 4, stop dot). `fraction` in 0..1.
pub fn wavy_progress(ui: &mut Ui, id_salt: impl egui::AsId, fraction: f32, width: f32) -> Response {
    let track = Theme::get(ui.ctx()).scheme.secondary_container;
    wavy_progress_on(ui, id_salt, fraction, width, track)
}

/// [`wavy_progress`] with its own track colour, for a container whose
/// colour is the usual track's (the card of Главная on secondary_container).
pub fn wavy_progress_on(ui: &mut Ui, id_salt: impl egui::AsId, fraction: f32, width: f32, track: Color32) -> Response {
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
                Stroke::new(thick, track),
            );
            ui.painter().circle_filled(pos2(rect.right() - 2.0, cy), 2.0, s.primary);
        }
        if amp > 0.0 && f > 0.0 && f < 1.0 {
            ui.ctx().request_repaint();
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> Context {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        ctx
    }

    /// One frame of a 400 px slider at `time`; returns the vertical extent
    /// of the wave (0 if the active track is drawn as a flat bar).
    fn frame(ctx: &Context, time: f64, value: f32, wavy: bool) -> f32 {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(420.0, 100.0))),
            time: Some(time),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            Slider::new("t", value).width(400.0).wavy(wavy).show(ui);
        });
        out.textures_delta.clear();
        out.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::Path(p) if !p.closed && p.points.len() > 5 => {
                    let (lo, hi) = p.points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), q| {
                        (lo.min(q.y), hi.max(q.y))
                    });
                    Some(hi - lo)
                }
                _ => None,
            })
            .fold(0.0, f32::max)
    }

    #[test]
    fn pausing_flattens_the_wave_smoothly_and_playing_brings_it_back() {
        let ctx = context();
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..90 {
            frame(&ctx, t, 0.5, true);
            t += dt;
        }
        let full = frame(&ctx, t, 0.5, true);
        assert!(full > 5.0, "a playing slider is wavy: {full}");

        // Pause: the amplitude only ever shrinks, never jumps, and ends flat.
        let mut last = full;
        let mut biggest_step: f32 = 0.0;
        for _ in 0..180 {
            t += dt;
            let a = frame(&ctx, t, 0.5, false);
            assert!(a <= last + 0.05, "the wave grew while pausing: {last} -> {a}");
            biggest_step = biggest_step.max(last - a);
            last = a;
        }
        assert!(last == 0.0, "the wave did not flatten: {last}");
        assert!(biggest_step < full * 0.25, "the wave popped: step {biggest_step} of {full}");

        // Play again: it grows from flat without a jump.
        let mut last = 0.0;
        for _ in 0..180 {
            t += dt;
            let a = frame(&ctx, t, 0.5, true);
            assert!(a <= full + 0.05, "overshoot: {a}");
            assert!(a + 0.05 >= last || last == 0.0, "the wave shrank while starting: {last} -> {a}");
            assert!(a - last < full * 0.25, "the wave popped in: {last} -> {a}");
            last = a;
        }
        assert!((last - full).abs() < 0.2, "{last} vs {full}");
    }

    #[test]
    fn phase_holds_while_paused_and_ignores_a_long_gap() {
        let ctx = context();
        let id = Id::new("phase");
        let at = |t: f64, running: bool| {
            let mut phase = 0.0;
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), ..Default::default() }, |_| {
                phase = wave_phase(&ctx, id, running, 0.6);
            });
            out.textures_delta.clear();
            phase
        };
        let a = at(0.0, true);
        let b = at(0.05, true);
        assert!(b > a, "the wave runs while playing");
        let held = at(0.1, false);
        assert_eq!(at(0.15, false), held, "paused: the phase holds");
        assert_eq!(at(5.0, false), held, "paused for long: still the same");
        let c = at(5.05, true);
        assert!((c - held).abs() < 0.5, "playing resumes where it was: {held} -> {c}");
        // A frame hours late (a minimized window) advances by at most 0.1 s.
        let late = at(5.05 + 3600.0, true);
        let max_step = 0.1 * 0.6 * TAU;
        assert!(((late - c).rem_euclid(TAU)) <= max_step + 1e-3, "{c} -> {late}");
    }
}
