//! Checkbox: an 18 dp box with 2 dp corners in a 40 dp state layer;
//! unticked a 2 dp outline, ticked or mixed filled with primary and the
//! check (or the dash) drawn in on a fast spatial spring, colours on fast
//! effects.

use egui::{Color32, Id, Rect, Response, Sense, Stroke, Ui, Vec2, Widget, WidgetInfo, WidgetType, pos2};

use super::{anim, button::hit_rect, common, paint, paint::Radii};
use crate::{Theme, tokens::state};

/// The box.
pub const BOX: f32 = 18.0;
/// The space it takes: its state layer.
pub const SIZE: f32 = 40.0;

/// A checkbox bound to a `bool`; `mixed` shows the dash of a partly
/// ticked group (a click then ticks it).
#[must_use = "add it with ui.add(Checkbox::new(&mut on))"]
pub struct Checkbox<'a> {
    on: &'a mut bool,
    mixed: bool,
    enabled: bool,
    label: Option<String>,
    id_salt: Option<Id>,
}

impl<'a> Checkbox<'a> {
    pub fn new(on: &'a mut bool) -> Self {
        Self { on, mixed: false, enabled: true, label: None, id_salt: None }
    }

    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }
}

impl Widget for Checkbox<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let id = self.id_salt.map_or_else(|| ui.next_auto_id(), |s| ui.id().with(s));
        let (visual, _) = ui.allocate_exact_size(Vec2::splat(SIZE), Sense::hover());
        let sense = if self.enabled { Sense::click() } else { Sense::hover() };
        let mut response = ui.interact(hit_rect(visual), id, sense);
        if self.enabled && response.clicked() {
            *self.on = if self.mixed { true } else { !*self.on };
            response.mark_changed();
        }
        let (on, mixed) = (*self.on, self.mixed && !response.changed());
        if ui.is_rect_visible(visual) {
            paint_checkbox(ui, id, visual, &response, on, mixed, self.enabled);
        }
        let label = self.label.unwrap_or_default();
        let enabled = self.enabled;
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, &label));
        response
    }
}

fn paint_checkbox(ui: &Ui, id: Id, rect: Rect, r: &Response, on: bool, mixed: bool, enabled: bool) {
    let ctx = ui.ctx();
    let theme = Theme::get(ctx);
    let (s, motion) = (&theme.scheme, theme.motion);
    let filled = on || mixed;
    let (fill, outline, mark) = match (enabled, filled) {
        (true, true) => (s.primary, common::clear(s.primary), s.on_primary),
        (true, false) => (common::clear(s.primary), s.on_surface_variant, common::clear(s.on_primary)),
        (false, true) => (common::fade(s.on_surface, state::DISABLED_CONTENT), common::clear(s.on_surface), s.surface),
        (false, false) => (
            common::clear(s.on_surface),
            common::fade(s.on_surface, state::DISABLED_CONTENT),
            common::clear(s.surface),
        ),
    };
    let fill = anim::color(ctx, id.with("fill"), fill, motion.fast_effects);
    let outline = anim::color(ctx, id.with("outline"), outline, motion.fast_effects);
    let mark = anim::color(ctx, id.with("mark"), mark, motion.fast_effects);
    // How far the check is drawn in, and how far it has become the dash.
    let drawn = anim::fraction(ctx, id.with("drawn"), f32::from(u8::from(filled)), motion.fast_spatial);
    let dash = anim::fraction(ctx, id.with("dash"), f32::from(u8::from(mixed)), motion.fast_spatial);

    let painter = ui.painter();
    let center = rect.center();
    let layer_target = common::state_target(r, enabled, false);
    let layer = anim::fraction(ctx, id.with("layer"), layer_target, motion.fast_effects);
    paint::state_layer(
        painter,
        Rect::from_center_size(center, Vec2::splat(SIZE)),
        Radii::all(SIZE / 2.0),
        if filled { s.primary } else { s.on_surface },
        layer,
    );
    let boxr = Rect::from_center_size(center, Vec2::splat(BOX));
    let radii = Radii::all(2.0);
    paint::container(painter, boxr, radii, fill, Stroke::NONE);
    if outline.a() > 0 {
        paint::container(painter, boxr.shrink(1.0), radii, Color32::TRANSPARENT, Stroke::new(2.0, outline));
    }
    if drawn > 0.01 && mark.a() > 0 {
        // The check's three points (24 dp icon grid, scaled to the box),
        // each moving onto the dash's line as `dash` grows.
        let p = |x: f32, y: f32| pos2(center.x + (x - 12.0) * BOX / 18.0, center.y + (y - 12.0) * BOX / 18.0);
        let check = [p(6.5, 12.3), p(10.0, 15.8), p(17.5, 8.3)];
        let line = [p(7.0, 12.0), p(12.0, 12.0), p(17.0, 12.0)];
        let pts: Vec<_> = check.iter().zip(line).map(|(a, b)| a.lerp(b, dash)).collect();
        let stroke = Stroke::new(2.0, mark);
        // Drawn in from the first point: the first leg, then the second.
        let first = (pts[1] - pts[0]).length();
        let second = (pts[2] - pts[1]).length();
        let along = drawn * (first + second);
        let a = along.min(first) / first.max(f32::EPSILON);
        painter.line_segment([pts[0], pts[0].lerp(pts[1], a)], stroke);
        if along > first {
            let b = ((along - first) / second.max(f32::EPSILON)).min(1.0);
            painter.line_segment([pts[1], pts[1].lerp(pts[2], b)], stroke);
        }
    }
    if common::focused(r) {
        paint::focus_ring(painter, boxr.expand(4.0), Radii::all(4.0), s.secondary);
    }
}
