//! Switch: 52x32 track, handle 16 (off) / 24 (on or with icon) / 28 (pressed),
//! position and size on a fast spatial spring, colours on fast effects.

use egui::{
    Color32, Id, Rect, Response, Sense, Stroke, Ui, Vec2, Widget, WidgetInfo, WidgetType, pos2,
    vec2,
};

use super::{anim, button::hit_rect, common, paint, paint::Radii};
use crate::{
    Icon, Theme,
    tokens::{shape, state},
};

pub const TRACK_WIDTH: f32 = 52.0;
pub const TRACK_HEIGHT: f32 = 32.0;
const ICON: f32 = 16.0;

/// A switch bound to a `bool`.
#[must_use = "add it with ui.add(Switch::new(&mut on))"]
pub struct Switch<'a> {
    on: &'a mut bool,
    icon: bool,
    enabled: bool,
    label: Option<String>,
    id_salt: Option<Id>,
}

impl<'a> Switch<'a> {
    pub fn new(on: &'a mut bool) -> Self {
        Self {
            on,
            icon: false,
            enabled: true,
            label: None,
            id_salt: None,
        }
    }

    /// Show a check (on) / cross (off) inside the handle.
    pub fn with_icon(mut self, icon: bool) -> Self {
        self.icon = icon;
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

impl Widget for Switch<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        // The id before the allocation: afterwards `next_auto_id` is the
        // next widget's, whose rect would replace this button's hit area.
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |s| ui.id().with(s));
        let (visual, _) = ui.allocate_exact_size(vec2(TRACK_WIDTH, TRACK_HEIGHT), Sense::hover());
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let mut response = ui.interact(hit_rect(visual), id, sense);
        if self.enabled && response.clicked() {
            *self.on = !*self.on;
            response.mark_changed();
        }
        let on = *self.on;

        if ui.is_rect_visible(visual) {
            paint_switch(ui, id, visual, &response, on, self.icon, self.enabled);
        }
        let label = self.label.unwrap_or_default();
        let enabled = self.enabled;
        response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, enabled, on, &label));
        response
    }
}

fn paint_switch(ui: &Ui, id: Id, rect: Rect, r: &Response, on: bool, icon: bool, enabled: bool) {
    let ctx = ui.ctx();
    let theme = Theme::get(ctx);
    let (s, motion) = (&theme.scheme, theme.motion);
    let pressed = enabled && r.is_pointer_button_down_on();
    let hovered = enabled && r.hovered();
    let focused = enabled && common::focused(r);

    let (track, border, handle, glyph) = match (enabled, on) {
        (true, true) => (
            s.primary,
            common::clear(s.outline),
            if pressed || hovered {
                s.primary_container
            } else {
                s.on_primary
            },
            s.on_primary_container,
        ),
        (true, false) => (
            s.surface_container_highest,
            s.outline,
            if pressed || hovered {
                s.on_surface_variant
            } else {
                s.outline
            },
            s.surface_container_highest,
        ),
        (false, true) => (
            common::fade(s.on_surface, state::DISABLED_CONTAINER),
            common::clear(s.on_surface),
            s.surface,
            common::fade(s.on_surface, state::DISABLED_CONTENT),
        ),
        (false, false) => (
            common::fade(s.surface_container_highest, state::DISABLED_CONTAINER),
            common::fade(s.on_surface, state::DISABLED_CONTAINER),
            common::fade(s.on_surface, state::DISABLED_CONTENT),
            s.surface_container_highest,
        ),
    };
    let track = anim::color(ctx, id.with("track"), track, motion.fast_effects);
    let border = anim::color(ctx, id.with("border"), border, motion.fast_effects);
    let handle = anim::color(ctx, id.with("handle"), handle, motion.fast_effects);
    let glyph = anim::color(ctx, id.with("glyph"), glyph, motion.fast_effects);

    let pos = anim::fraction(
        ctx,
        id.with("pos"),
        f32::from(u8::from(on)),
        motion.fast_spatial,
    );
    let target_size = if pressed {
        28.0
    } else if on || icon {
        24.0
    } else {
        16.0
    };
    let size = anim::f32(ctx, id.with("size"), target_size, motion.fast_spatial).max(0.0);
    let center = pos2(rect.left() + 16.0 + 20.0 * pos, rect.center().y);

    let painter = ui.painter();
    let radii = Radii::all(shape::FULL);
    paint::container(painter, rect, radii, track, Stroke::NONE);
    if border.a() > 0 {
        paint::container(
            painter,
            rect.shrink(1.0),
            radii,
            Color32::TRANSPARENT,
            Stroke::new(2.0, border),
        );
    }

    // State layer: a 40 dp circle behind the handle.
    let layer_target = common::state_target(r, enabled, false);
    let layer = anim::fraction(ctx, id.with("layer"), layer_target, motion.fast_effects);
    let layer_color = if on { s.primary } else { s.on_surface };
    paint::state_layer(
        painter,
        Rect::from_center_size(center, Vec2::splat(40.0)),
        radii,
        layer_color,
        layer,
    );
    paint::container(
        painter,
        Rect::from_center_size(center, Vec2::splat(size)),
        radii,
        handle,
        Stroke::NONE,
    );
    if icon && size >= 20.0 {
        let (ic, fill) = if on {
            (Icon::Check, true)
        } else {
            (Icon::Close, false)
        };
        ic.widget(fill, glyph, ICON)
            .paint_at(ui, Rect::from_center_size(center, Vec2::splat(ICON)));
    }
    if focused {
        paint::focus_ring(painter, rect, radii, s.secondary);
    }
}
