//! Search: the 56 dp search bar (full radius, `surface-container-high`,
//! leading search icon, body-large input, trailing clear button) and the
//! docked search view (radius 28, 56 dp header) the bar expands into with a
//! container-transform: the container's height follows a slow spatial
//! spring while the results cross-fade in.
//!
//! Ctrl+K / Ctrl+F focus the search (see [`shortcut_pressed`] and
//! [`Search::request_open`]); Esc or a click outside collapses the view.

use egui::{
    Align, Area, Context, Frame, Id, Key, Margin, Modifiers, Order, Rect, Response, RichText,
    Sense, Stroke, TextEdit, Ui, UiBuilder, pos2, vec2,
};

use super::{anim, common, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape},
};

pub const HEIGHT: f32 = 56.0;
const RADIUS: f32 = 28.0;

/// True (and consumed) when Ctrl+K or Ctrl+F was pressed this frame.
pub fn shortcut_pressed(ctx: &Context) -> bool {
    ctx.input_mut(|i| {
        i.consume_key(Modifiers::COMMAND, Key::K) | i.consume_key(Modifiers::COMMAND, Key::F)
    })
}

struct HeaderOut {
    edit: Response,
    clear_clicked: bool,
    back_clicked: bool,
    submitted: bool,
}

/// The 56 dp header row: leading icon, input, clear button.
fn header(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    text: &mut String,
    placeholder: &str,
    back: bool,
    focus: bool,
) -> HeaderOut {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let edit_id = id.with("edit");

    // Leading icon (a back arrow inside the expanded view).
    let lead_c = pos2(rect.left() + 28.0, rect.center().y);
    let mut back_clicked = false;
    if back {
        let r = ui.interact(
            egui::Rect::from_center_size(lead_c, vec2(48.0, 48.0)),
            id.with("back"),
            Sense::click(),
        );
        back_clicked = r.clicked();
        let l = common::layer_opacity(ui.ctx(), id.with("back"), &r, true);
        paint::state_layer(
            ui.painter(),
            Rect::from_center_size(lead_c, vec2(40.0, 40.0)),
            Radii::all(shape::FULL),
            s.on_surface,
            l,
        );
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close search"));
    }
    let lead_icon = if back { Icon::ArrowBack } else { Icon::Search };
    lead_icon
        .widget(false, s.on_surface, 24.0)
        .paint_at(ui, Rect::from_center_size(lead_c, vec2(24.0, 24.0)));

    // Input.
    let has_text = !text.is_empty();
    let text_rect = Rect::from_min_max(
        pos2(rect.left() + 56.0, rect.center().y - 12.0),
        pos2(
            rect.right() - if has_text { 56.0 } else { 16.0 },
            rect.center().y + 12.0,
        ),
    );
    let edit = ui
        .scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
            TextEdit::singleline(text)
                .id(edit_id)
                .frame(Frame::NONE)
                .margin(Margin::ZERO)
                .font(TypeRole::BodyLarge.font_id())
                .text_color(s.on_surface)
                .desired_width(text_rect.width())
                .vertical_align(Align::Center)
                .hint_text(RichText::new(placeholder).color(s.on_surface_variant))
                .show(ui)
        })
        .inner
        .response
        .response;
    if focus {
        edit.request_focus();
    }
    let submitted = edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));

    // Clear button.
    let mut clear_clicked = false;
    if has_text {
        let c = pos2(rect.right() - 28.0, rect.center().y);
        let r = ui.interact(
            Rect::from_center_size(c, vec2(48.0, 48.0)),
            id.with("clear"),
            Sense::click(),
        );
        clear_clicked = r.clicked();
        let l = common::layer_opacity(ui.ctx(), id.with("clear"), &r, true);
        paint::state_layer(
            ui.painter(),
            Rect::from_center_size(c, vec2(40.0, 40.0)),
            Radii::all(shape::FULL),
            s.on_surface_variant,
            l,
        );
        Icon::Close
            .widget(false, s.on_surface_variant, 24.0)
            .paint_at(ui, Rect::from_center_size(c, vec2(24.0, 24.0)));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Clear"));
    }
    HeaderOut {
        edit,
        clear_clicked,
        back_clicked,
        submitted,
    }
}

/// What a [`SearchBar`] reported.
pub struct SearchBarOutput {
    pub response: Response,
    pub rect: Rect,
    pub focused: bool,
    pub submitted: bool,
    /// The clear button emptied the text.
    pub cleared: bool,
}

/// A stand-alone search bar (no expanding view).
#[must_use = "call .show(ui, &mut text)"]
pub struct SearchBar {
    id: Id,
    placeholder: String,
    width: Option<f32>,
    shortcut: bool,
}

impl SearchBar {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            placeholder: String::new(),
            width: None,
            shortcut: true,
        }
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    /// Ctrl+K / Ctrl+F focus the bar (default on).
    pub fn shortcut(mut self, on: bool) -> Self {
        self.shortcut = on;
        self
    }

    pub fn show(self, ui: &mut Ui, text: &mut String) -> SearchBarOutput {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let (rect, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
        let bg = ui.interact(rect, self.id.with("bg"), Sense::click());
        let edit_id = self.id.with("edit");
        let focus = self.shortcut && shortcut_pressed(&ctx);
        if bg.clicked() {
            ctx.memory_mut(|m| m.request_focus(edit_id));
        }
        let radii = Radii::all(shape::FULL);
        paint::container(
            ui.painter(),
            rect,
            radii,
            s.surface_container_high,
            Stroke::NONE,
        );
        let layer = anim::fraction(
            &ctx,
            self.id.with("layer"),
            if bg.contains_pointer() { 0.08 } else { 0.0 },
            motion.fast_effects,
        );
        paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);
        let out = header(ui, rect, self.id, text, &self.placeholder, false, focus);
        let mut cleared = false;
        if out.clear_clicked {
            text.clear();
            cleared = true;
            out.edit.request_focus();
        }
        SearchBarOutput {
            focused: out.edit.has_focus(),
            submitted: out.submitted,
            response: out.edit,
            rect,
            cleared,
        }
    }
}

/// What a [`Search`] reported.
pub struct SearchOutput {
    /// The view is open (or still animating open).
    pub expanded: bool,
    pub changed: bool,
    pub submitted: bool,
    /// The view was collapsed this frame.
    pub collapsed: bool,
    pub bar_rect: Rect,
}

/// A search bar that expands into a docked search view.
#[must_use = "call .show(ui, &mut text, |ui| results)"]
pub struct Search {
    id: Id,
    placeholder: String,
    width: Option<f32>,
    view_height: f32,
}

impl Search {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            placeholder: String::new(),
            width: None,
            view_height: 480.0,
        }
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    /// Height of the open view including the 56 dp header (default 480).
    pub fn view_height(mut self, h: f32) -> Self {
        self.view_height = h.max(HEIGHT + 56.0);
        self
    }

    /// Opens and focuses the search `id_salt` from anywhere (a menu item,
    /// a custom shortcut).
    pub fn request_open(ctx: &Context, id_salt: impl egui::AsId) {
        let id = Id::new(id_salt);
        ctx.data_mut(|d| d.insert_temp(id.with("req"), true));
        ctx.request_repaint();
    }

    pub fn is_expanded(ctx: &Context, id_salt: impl egui::AsId) -> bool {
        ctx.data(|d| d.get_temp::<bool>(Id::new(id_salt).with("exp")))
            .unwrap_or(false)
    }

    /// `results` draws the view's content under the header; it is shown
    /// (cross-faded) while the view is open.
    pub fn show(
        self,
        ui: &mut Ui,
        text: &mut String,
        results: impl FnOnce(&mut Ui),
    ) -> SearchOutput {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let id = self.id;
        let edit_id = id.with("edit");
        let exp_key = id.with("exp");
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let (bar, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());

        let was_expanded = ctx.data(|d| d.get_temp::<bool>(exp_key)).unwrap_or(false);
        let mut expanded = was_expanded;
        let mut focus = false;
        let requested = ctx
            .data_mut(|d| d.remove_temp::<bool>(id.with("req")))
            .unwrap_or(false);
        if requested || shortcut_pressed(&ctx) {
            expanded = true;
            focus = true;
        }
        if expanded && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            expanded = false;
        }

        let k = anim::fraction(
            &ctx,
            id.with("k"),
            f32::from(u8::from(expanded)),
            motion.slow_spatial,
        );
        let ke = anim::fraction(
            &ctx,
            id.with("ke"),
            f32::from(u8::from(expanded)),
            motion.default_effects,
        )
        .clamp(0.0, 1.0);
        let showing_view = expanded || k.abs() > 0.002 || ke > 0.002;

        let mut changed = false;
        let mut submitted = false;
        if !showing_view {
            // Collapsed: an ordinary bar in the layout.
            let bg = ui.interact(bar, id.with("bg"), Sense::click());
            if bg.clicked() {
                expanded = true;
            }
            let radii = Radii::all(shape::FULL);
            paint::container(
                ui.painter(),
                bar,
                radii,
                s.surface_container_high,
                Stroke::NONE,
            );
            let layer = anim::fraction(
                &ctx,
                id.with("layer"),
                if bg.contains_pointer() { 0.08 } else { 0.0 },
                motion.fast_effects,
            );
            paint::state_layer(ui.painter(), bar, radii, s.on_surface, layer);
            let before = text.clone();
            let out = header(ui, bar, id, text, &self.placeholder, false, false);
            changed |= *text != before;
            if out.clear_clicked {
                text.clear();
                changed = true;
                expanded = true;
            }
            if out.edit.gained_focus() {
                expanded = true;
            }
            submitted |= out.submitted;
        } else {
            let view_h = HEIGHT + (self.view_height - HEIGHT) * k.max(0.0);
            let view_rect = Rect::from_min_size(bar.min, vec2(width, view_h.max(HEIGHT)));
            let radii = Radii::all(RADIUS);
            let mut clicked_close = false;
            let area = Area::new(id.with("view"))
                .order(Order::Foreground)
                .fixed_pos(bar.min)
                .constrain(false)
                .interactable(true)
                .show(&ctx, |ui| {
                    let (rect, _) = ui.allocate_exact_size(view_rect.size(), Sense::hover());
                    let shadow = if ke > 0.05 {
                        Elevation::Level3
                    } else {
                        Elevation::Level0
                    };
                    paint::shadow(ui.painter(), rect, radii, shadow);
                    paint::container(
                        ui.painter(),
                        rect,
                        radii,
                        s.surface_container_high,
                        Stroke::NONE,
                    );
                    // Swallow clicks on the container.
                    ui.interact(rect, id.with("view-bg"), Sense::click());
                    let header_rect = Rect::from_min_size(rect.min, vec2(width, HEIGHT));
                    let before = text.clone();
                    let out = header(
                        ui,
                        header_rect,
                        id,
                        text,
                        &self.placeholder,
                        ke > 0.5,
                        focus,
                    );
                    changed |= *text != before;
                    submitted |= out.submitted;
                    if out.clear_clicked {
                        text.clear();
                        changed = true;
                        out.edit.request_focus();
                    }
                    clicked_close = out.back_clicked;
                    if ke > 0.01 {
                        let body = Rect::from_min_max(
                            pos2(rect.left(), rect.top() + HEIGHT),
                            pos2(rect.right(), rect.bottom() - 8.0),
                        );
                        if body.height() > 8.0 {
                            ui.painter().line_segment(
                                [body.left_top(), body.right_top()],
                                Stroke::new(1.0, common::fade(s.outline_variant, ke)),
                            );
                            let mut child = ui
                                .new_child(UiBuilder::new().max_rect(body.shrink2(vec2(0.0, 1.0))));
                            child.set_clip_rect(body.intersect(ui.clip_rect()));
                            child.set_opacity(ke);
                            results(&mut child);
                        }
                    }
                });
            let _ = area;
            if clicked_close {
                expanded = false;
            }
            // A press outside the view collapses it.
            if expanded
                && ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|p| !view_rect.contains(p))
                })
            {
                expanded = false;
            }
        }

        let collapsed = was_expanded && !expanded;
        if collapsed {
            ctx.memory_mut(|m| m.surrender_focus(edit_id));
        }
        if expanded && !was_expanded {
            ctx.memory_mut(|m| m.request_focus(edit_id));
        }
        ctx.data_mut(|d| d.insert_temp(exp_key, expanded));
        SearchOutput {
            expanded: showing_view,
            changed,
            submitted,
            collapsed,
            bar_rect: bar,
        }
    }
}
