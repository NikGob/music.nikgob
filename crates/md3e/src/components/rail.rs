//! Navigation rail: collapsed (96, indicator 56×32 with the label under the
//! icon) and expanded (icon and label in a row), morphing between the two
//! on a default spatial spring. A FAB / extended FAB sits on top.

use egui::{Align2, Color32, Id, Rect, Response, Sense, Stroke, Ui, pos2, vec2};

use super::{anim, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape, state},
};

pub const COLLAPSED_WIDTH: f32 = 96.0;
pub const EXPANDED_WIDTH: f32 = 256.0;

pub struct RailItem<'a> {
    pub icon: Icon,
    pub label: &'a str,
    pub badge: Option<u32>,
}

#[derive(Default)]
pub struct RailOutput {
    pub clicked: Option<usize>,
    pub bottom_clicked: Option<usize>,
    pub fab: Option<Response>,
}

pub struct NavRail<'a> {
    pub id: Id,
    /// 0 = collapsed, 1 = expanded (already animated by the caller).
    pub expansion: f32,
    pub fab: Option<(Icon, &'a str)>,
    pub items: &'a [RailItem<'a>],
    pub bottom: &'a [RailItem<'a>],
    pub selected: Option<usize>,
    pub bottom_selected: Option<usize>,
}

/// The rail width at an expansion fraction.
pub fn width(expansion: f32) -> f32 {
    COLLAPSED_WIDTH + (EXPANDED_WIDTH - COLLAPSED_WIDTH) * expansion
}

impl NavRail<'_> {
    pub fn show(self, ui: &mut Ui) -> RailOutput {
        let theme = Theme::get(ui.ctx());
        let s = theme.scheme;
        let k = self.expansion.clamp(0.0, 1.0);
        let full = ui.max_rect();
        let mut out = RailOutput::default();
        let mut y = full.top() + 44.0;

        if let Some((icon, label)) = self.fab {
            let w_c = 56.0;
            let label_galley = ui.painter().layout_no_wrap(
                label.to_owned(),
                TypeRole::LabelLarge.font_id(),
                s.on_primary_container,
            );
            let w_e = 16.0 + 24.0 + 8.0 + label_galley.size().x + 16.0;
            let w = w_c + (w_e - w_c) * k;
            // Expanded, its left edge and icon line up with the indicators'.
            let x = full.left() + 20.0 + (12.0 - 20.0) * k;
            let rect = Rect::from_min_size(pos2(x, y), vec2(w, 56.0));
            let id = self.id.with("fab");
            let r = ui.interact(rect, id, Sense::click());
            let hovered = r.hovered();
            let pressed = r.is_pointer_button_down_on();
            let elevation = if hovered { Elevation::Level4 } else { Elevation::Level3 };
            let radii = Radii::all(shape::LARGE);
            paint::shadow(ui.painter(), rect, radii, elevation);
            paint::container(ui.painter(), rect, radii, s.primary_container, Stroke::NONE);
            let layer = anim::fraction(
                ui.ctx(),
                id.with("l"),
                if pressed {
                    state::PRESSED
                } else if hovered {
                    state::HOVER
                } else {
                    0.0
                },
                theme.motion.fast_effects,
            );
            paint::state_layer(ui.painter(), rect, radii, s.on_primary_container, layer);
            let icon_x = rect.left() + 16.0 + (rect.width() - 32.0 - 24.0) * 0.5 * (1.0 - k);
            icon.widget(false, s.on_primary_container, 24.0).paint_at(
                ui,
                Rect::from_min_size(pos2(icon_x, rect.center().y - 12.0), vec2(24.0, 24.0)),
            );
            if k > 0.05 {
                let painter = ui.painter().with_clip_rect(rect);
                painter.galley(
                    pos2(rect.left() + 48.0, rect.center().y - label_galley.size().y * 0.5),
                    label_galley,
                    s.on_primary_container.gamma_multiply(k),
                );
            }
            y = rect.bottom() + 40.0;
            out.fab = Some(r.on_hover_text(label));
        }

        let draw = |items: &[RailItem<'_>], selected: Option<usize>, y: &mut f32, group: &str| {
            let mut clicked = None;
            for (i, item) in items.iter().enumerate() {
                let item_h = 56.0 + 8.0 * (1.0 - k);
                let row = Rect::from_min_size(pos2(full.left(), *y), vec2(full.width(), item_h));
                // Indicator: 56×32 centred at the top (collapsed) → a pill 56
                // tall hugging icon and label (expanded).
                let ind_c = Rect::from_center_size(
                    pos2(full.left() + COLLAPSED_WIDTH * 0.5, row.top() + 16.0),
                    vec2(56.0, 32.0),
                );
                let label_w = ui
                    .painter()
                    .layout_no_wrap(item.label.to_owned(), TypeRole::LabelLarge.font_id(), Color32::WHITE)
                    .size()
                    .x;
                let ind_e = Rect::from_min_size(
                    pos2(full.left() + 12.0, row.top()),
                    vec2(16.0 + 24.0 + 12.0 + label_w + 20.0, 56.0),
                );
                let ind = lerp_rect(ind_c, ind_e, k);
                let id = self.id.with((group, i));
                let resp = ui.interact(ind.union(row.shrink2(vec2(8.0, 0.0))), id, Sense::click());
                let is_sel = selected == Some(i);
                let sel_k = anim::fraction(
                    ui.ctx(),
                    id.with("sel"),
                    if is_sel { 1.0 } else { 0.0 },
                    theme.motion.fast_spatial,
                );
                if sel_k > 0.001 {
                    // The pill grows from its centre.
                    let grown = Rect::from_center_size(
                        ind.center(),
                        vec2(ind.width() * sel_k.clamp(0.0, 1.2), ind.height()),
                    );
                    paint::container(
                        ui.painter(),
                        grown,
                        Radii::all(shape::FULL),
                        s.secondary_container.gamma_multiply(sel_k.clamp(0.0, 1.0)),
                        Stroke::NONE,
                    );
                }
                let content = if is_sel {
                    s.on_secondary_container
                } else {
                    s.on_surface_variant
                };
                let layer = anim::fraction(
                    ui.ctx(),
                    id.with("l"),
                    if resp.is_pointer_button_down_on() {
                        state::PRESSED
                    } else if resp.hovered() {
                        state::HOVER
                    } else {
                        0.0
                    },
                    theme.motion.fast_effects,
                );
                paint::state_layer(ui.painter(), ind, Radii::all(shape::FULL), content, layer);

                let icon_c = ind_c.center();
                let icon_e = pos2(ind_e.left() + 16.0 + 12.0, ind_e.center().y);
                let icon_pos = icon_c + (icon_e - icon_c) * k;
                item.icon.widget(is_sel, content, 24.0).paint_at(
                    ui,
                    Rect::from_center_size(icon_pos, vec2(24.0, 24.0)),
                );
                if let Some(n) = item.badge.filter(|&n| n > 0) {
                    badge(ui, icon_pos + vec2(6.0, -12.0), n, &s);
                }
                let label_color = if is_sel { s.secondary } else { s.on_surface_variant };
                if k < 0.95 {
                    ui.painter().text(
                        pos2(ind_c.center().x, ind_c.bottom() + 4.0),
                        Align2::CENTER_TOP,
                        item.label,
                        TypeRole::LabelMedium.font_id(),
                        label_color.gamma_multiply(1.0 - k),
                    );
                }
                if k > 0.05 {
                    let painter = ui.painter().with_clip_rect(ind_e);
                    painter.text(
                        pos2(ind_e.left() + 16.0 + 24.0 + 12.0, ind_e.center().y),
                        Align2::LEFT_CENTER,
                        item.label,
                        TypeRole::LabelLarge.font_id(),
                        content.gamma_multiply(k),
                    );
                }
                if resp.clicked() {
                    clicked = Some(i);
                }
                let resp = if k < 0.5 { resp.on_hover_text(item.label) } else { resp };
                let _ = resp;
                *y = row.bottom() + 4.0 + 8.0 * (1.0 - k);
            }
            clicked
        };

        out.clicked = draw(self.items, self.selected, &mut y, "item");
        let bottom_h = self.bottom.len() as f32 * 68.0 + 16.0;
        let mut by = full.bottom() - bottom_h;
        out.bottom_clicked = draw(self.bottom, self.bottom_selected, &mut by, "bottom");
        out
    }
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(a.min + (b.min - a.min) * t, a.max + (b.max - a.max) * t)
}

/// The large badge (16 tall, error colours) with a count.
pub fn badge(ui: &Ui, left_center: egui::Pos2, n: u32, s: &crate::ColorScheme) {
    let text = if n > 999 { "999+".to_owned() } else { n.to_string() };
    let galley = ui
        .painter()
        .layout_no_wrap(text, TypeRole::LabelSmall.font_id(), s.on_error);
    let w = (galley.size().x + 8.0).max(16.0);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - 8.0), vec2(w, 16.0));
    paint::container(ui.painter(), rect, Radii::all(shape::FULL), s.error, Stroke::NONE);
    ui.painter().galley(
        rect.center() - galley.size() * 0.5,
        galley,
        Color32::PLACEHOLDER,
    );
}
