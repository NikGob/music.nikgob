//! Sheets: a modal side sheet (up to 400 wide, `surface-container-low`,
//! radius 28 on the leading edge, 32 % scrim), a modal bottom sheet (radius 28
//! on top, 32x4 drag handle, drag down to dismiss) and the `SupportingPane`
//! frame (no overlay, only the container).
//!
//! Both sheets open on a default spatial spring, close on Esc or a click on
//! the scrim, and live above `Order::Foreground` like dialogs.

use egui::{
    Area, Context, Id, Key, Modifiers, Order, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2,
};

use super::{
    anim,
    button::{IconButton, Size, Style},
    common, paint,
    paint::Radii,
};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape},
};

pub const SIDE_SHEET_MAX_WIDTH: f32 = 400.0;
pub const BOTTOM_SHEET_MAX_WIDTH: f32 = 640.0;

/// What a sheet reported.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SheetOutput {
    /// Closed this frame by Esc, the scrim, the close button or a drag.
    pub dismissed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Edge {
    Left,
    #[default]
    Right,
}

/// A modal side sheet.
#[must_use = "call .show(ctx, &mut open, |ui| ...)"]
pub struct SideSheet {
    id: Id,
    edge: Edge,
    title: Option<String>,
    width: f32,
}

impl SideSheet {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            edge: Edge::Right,
            title: None,
            width: SIDE_SHEET_MAX_WIDTH,
        }
    }

    pub fn edge(mut self, edge: Edge) -> Self {
        self.edge = edge;
        self
    }

    /// A headline-small title with a close button.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Maximum width (never more than 400, never wider than the window less 56).
    pub fn width(mut self, w: f32) -> Self {
        self.width = w.min(SIDE_SHEET_MAX_WIDTH);
        self
    }

    pub fn show(
        self,
        ctx: &Context,
        open: &mut bool,
        content: impl FnOnce(&mut Ui),
    ) -> SheetOutput {
        let theme = Theme::get(ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let was_open = *open;
        let pres = common::presence(
            ctx,
            self.id,
            was_open,
            motion.default_spatial,
            motion.default_effects,
        );
        if !pres.visible() {
            return SheetOutput::default();
        }
        let screen = ctx.content_rect();
        let mut out = SheetOutput::default();

        let scrim = common::scrim(ctx, self.id.with("scrim"), pres.alpha());
        if was_open
            && (scrim.clicked() || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)))
        {
            out.dismissed = true;
        }

        let w = self.width.min(screen.width() - 56.0).max(240.0);
        let t = pres.spatial.min(1.0);
        let hidden = (1.0 - t) * w;
        let (x, radii) = match self.edge {
            Edge::Right => (
                screen.right() - w + hidden,
                Radii::sides(shape::EXTRA_LARGE, 0.0),
            ),
            Edge::Left => (
                screen.left() - hidden,
                Radii::sides(0.0, shape::EXTRA_LARGE),
            ),
        };
        let rect = Rect::from_min_size(pos2(x, screen.top()), vec2(w, screen.height()));
        let id = self.id;
        let title = self.title;
        let mut close_clicked = false;
        Area::new(id.with("sheet"))
            .fade_in(false)
            .order(Order::Tooltip)
            .fixed_pos(rect.min)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
                paint::shadow(ui.painter(), r, radii, Elevation::Level1);
                paint::container(
                    ui.painter(),
                    r,
                    radii,
                    s.surface_container_low,
                    Stroke::NONE,
                );
                ui.interact(r, id.with("sheet-bg"), Sense::click());
                let mut inner = r.shrink2(vec2(24.0, 24.0));
                if let Some(title) = &title {
                    let g = common::line(
                        ui.painter(),
                        title,
                        TypeRole::HeadlineSmall.style(),
                        s.on_surface,
                        inner.width() - 56.0,
                    );
                    ui.painter()
                        .galley(pos2(inner.left(), inner.top() + 4.0), g, s.on_surface);
                    let btn = Rect::from_min_size(
                        pos2(inner.right() - 40.0, inner.top()),
                        vec2(40.0, 40.0),
                    );
                    let resp = ui
                        .scope_builder(UiBuilder::new().max_rect(btn), |ui| {
                            ui.add(
                                IconButton::new(Icon::Close)
                                    .size(Size::S)
                                    .style(Style::Standard)
                                    .tooltip("Close")
                                    .id_salt("sheet-close"),
                            )
                        })
                        .inner;
                    close_clicked = resp.clicked();
                    inner.min.y += 56.0;
                }
                let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
                child.set_clip_rect(r.intersect(ui.clip_rect()));
                content(&mut child);
            });
        if was_open && close_clicked {
            out.dismissed = true;
        }
        if out.dismissed {
            *open = false;
        }
        out
    }
}

/// A modal bottom sheet.
#[must_use = "call .show(ctx, &mut open, |ui| ...)"]
pub struct BottomSheet {
    id: Id,
    height: Option<f32>,
    max_width: f32,
}

impl BottomSheet {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            height: None,
            max_width: BOTTOM_SHEET_MAX_WIDTH,
        }
    }

    /// Sheet height (default: 60 % of the window, at most 560).
    pub fn height(mut self, h: f32) -> Self {
        self.height = Some(h);
        self
    }

    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = w;
        self
    }

    pub fn show(
        self,
        ctx: &Context,
        open: &mut bool,
        content: impl FnOnce(&mut Ui),
    ) -> SheetOutput {
        let theme = Theme::get(ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let was_open = *open;
        let pres = common::presence(
            ctx,
            self.id,
            was_open,
            motion.default_spatial,
            motion.default_effects,
        );
        let off_key = self.id.with("off");
        if !pres.visible() {
            ctx.data_mut(|d| d.remove::<f32>(off_key.with("last")));
            return SheetOutput::default();
        }
        let screen = ctx.content_rect();
        let mut out = SheetOutput::default();

        let scrim = common::scrim(ctx, self.id.with("scrim"), pres.alpha());
        if was_open
            && (scrim.clicked() || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)))
        {
            out.dismissed = true;
        }

        let h = self
            .height
            .unwrap_or_else(|| (screen.height() * 0.6).min(560.0))
            .min(screen.height() - 24.0)
            .max(120.0);
        let w = self.max_width.min(screen.width());
        let t = pres.spatial.min(1.0);
        // Drag offset: follows the pointer while dragging, springs back after.
        let last: f32 = ctx
            .data(|d| d.get_temp(off_key.with("last")))
            .unwrap_or(0.0);
        let off = if was_open {
            anim::f32(ctx, off_key, 0.0, motion.default_spatial).max(0.0)
        } else {
            last * pres.alpha()
        };
        let top = screen.bottom() - h * t + off;
        let rect = Rect::from_min_size(pos2(screen.center().x - w * 0.5, top), vec2(w, h));
        let radii = Radii::top(shape::EXTRA_LARGE);
        let id = self.id;
        let mut drag_value: Option<f32> = None;
        let mut released_at: Option<f32> = None;
        Area::new(id.with("sheet"))
            .fade_in(false)
            .order(Order::Tooltip)
            .fixed_pos(rect.min)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
                paint::shadow(ui.painter(), r, radii, Elevation::Level1);
                paint::container(
                    ui.painter(),
                    r,
                    radii,
                    s.surface_container_low,
                    Stroke::NONE,
                );
                ui.interact(r, id.with("sheet-bg"), Sense::click());

                // Drag handle + the strip around it.
                let strip = Rect::from_min_size(r.min, vec2(w, 40.0));
                let grab = ui.interact(strip, id.with("grab"), Sense::drag());
                let handle_c = pos2(r.center().x, r.top() + 16.0);
                let handle_alpha = if grab.hovered() || grab.dragged() {
                    1.0
                } else {
                    0.4
                };
                paint::container(
                    ui.painter(),
                    Rect::from_center_size(handle_c, vec2(32.0, 4.0)),
                    Radii::all(shape::FULL),
                    common::fade(s.on_surface_variant, handle_alpha),
                    Stroke::NONE,
                );
                grab.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Drag to dismiss")
                });
                if grab.dragged() {
                    let dy = grab.total_drag_delta().map_or(0.0, |d| d.y);
                    drag_value = Some(dy.max(0.0));
                }
                if grab.drag_stopped() {
                    released_at = Some(grab.total_drag_delta().map_or(0.0, |d| d.y).max(0.0));
                }

                let inner = Rect::from_min_max(
                    pos2(r.left() + 24.0, r.top() + 36.0),
                    pos2(r.right() - 24.0, r.bottom() - 16.0),
                );
                let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
                child.set_clip_rect(r.intersect(ui.clip_rect()));
                content(&mut child);
            });
        if was_open {
            if let Some(v) = drag_value {
                // Hold the sheet under the pointer (relative to where it is).
                anim::f32(ctx, off_key, v, motion.default_spatial);
                anim::snap_f32(ctx, off_key, v);
                ctx.data_mut(|d| d.insert_temp(off_key.with("last"), v));
            }
            if let Some(v) = released_at {
                if v > h * 0.3 {
                    out.dismissed = true;
                    ctx.data_mut(|d| d.insert_temp(off_key.with("last"), v));
                } else {
                    anim::f32(ctx, off_key, 0.0, motion.default_spatial);
                }
            }
        }
        if out.dismissed {
            *open = false;
        }
        out
    }
}

/// The container of a supporting pane (360 / 412 wide on large windows): a
/// `surface-container-low` rounded rectangle filling the available space,
/// with the content inset by `padding`. It draws no overlay.
#[must_use = "call .show(ui, |ui| ...)"]
pub struct SupportingPane {
    radius: f32,
    padding: f32,
}

impl SupportingPane {
    pub fn new() -> Self {
        Self {
            radius: shape::EXTRA_LARGE,
            padding: 16.0,
        }
    }

    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }

    pub fn padding(mut self, p: f32) -> Self {
        self.padding = p;
        self
    }

    pub fn show<R>(self, ui: &mut Ui, content: impl FnOnce(&mut Ui) -> R) -> R {
        let s = Theme::get(ui.ctx()).scheme;
        let rect = ui.available_rect_before_wrap();
        paint::container(
            ui.painter(),
            rect,
            Radii::all(self.radius),
            s.surface_container_low,
            Stroke::NONE,
        );
        let inner = rect.shrink(self.padding);
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let r = content(&mut child);
        ui.advance_cursor_after_rect(rect);
        r
    }
}

impl Default for SupportingPane {
    fn default() -> Self {
        Self::new()
    }
}
