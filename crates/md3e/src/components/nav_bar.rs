//! Navigation bar: the destinations of a phone in a row along the bottom,
//! each an icon over its label. The bar is 80 dp tall above the gesture
//! area: the 56×32 selection pill at +12, the 24 dp icon centred in it,
//! the 12 sp label at +48 with 16 dp under it. The selected pill grows
//! from its centre on the fast spatial spring; a press lays the state layer
//! over the pill's place. The bar runs down under the gesture area
//! (`bottom_inset`) in its own colour.

use egui::{Align2, Id, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::{anim, paint, paint::Radii, rail};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{shape, state},
};

/// The bar's height above the gesture area.
pub const HEIGHT: f32 = 80.0;
/// The selection pill.
const PILL: egui::Vec2 = egui::Vec2::new(56.0, 32.0);
/// Space over the pill.
const TOP_PAD: f32 = 12.0;
/// Space between the pill and the label.
const LABEL_GAP: f32 = 4.0;

pub struct NavBarItem<'a> {
    pub icon: Icon,
    pub label: &'a str,
    /// A count in the large badge over the icon (none at 0).
    pub badge: Option<u32>,
}

#[must_use = "call .show(ui, rect)"]
pub struct NavBar<'a> {
    id: Id,
    items: &'a [NavBarItem<'a>],
    selected: Option<usize>,
    bottom_inset: f32,
}

impl<'a> NavBar<'a> {
    pub fn new(id_salt: impl egui::AsId, items: &'a [NavBarItem<'a>]) -> Self {
        Self { id: Id::new(id_salt), items, selected: None, bottom_inset: 0.0 }
    }

    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// The gesture area under the bar, filled with the bar's colour.
    pub fn bottom_inset(mut self, inset: f32) -> Self {
        self.bottom_inset = inset.max(0.0);
        self
    }

    /// The bar's whole height with its inset.
    pub fn height(&self) -> f32 {
        HEIGHT + self.bottom_inset
    }

    /// Draws the bar at the top of `rect` (it takes [`Self::height`]) and
    /// returns the destination tapped.
    pub fn show(self, ui: &mut Ui, rect: Rect) -> Option<usize> {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let s = theme.scheme;
        let bar = Rect::from_min_size(rect.min, vec2(rect.width(), self.height()));
        paint::container(ui.painter(), bar, Radii::all(0.0), s.surface_container, Stroke::NONE);
        // Taps on the bar's empty parts never reach what lies under it.
        ui.interact(bar, self.id.with("bg"), Sense::click());
        let n = self.items.len().max(1);
        let cell_w = bar.width() / n as f32;
        let mut tapped = None;
        for (i, item) in self.items.iter().enumerate() {
            let cell = Rect::from_min_size(pos2(bar.left() + cell_w * i as f32, bar.top()), vec2(cell_w, HEIGHT));
            let id = self.id.with(i);
            let resp = ui.interact(cell, id, Sense::click());
            let selected = self.selected == Some(i);
            let pill = Rect::from_center_size(pos2(cell.center().x, cell.top() + TOP_PAD + PILL.y * 0.5), PILL);
            let k = anim::fraction(&ctx, id.with("sel"), if selected { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
            if k > 0.001 {
                let grown = Rect::from_center_size(pill.center(), vec2(PILL.x * k.clamp(0.0, 1.15), PILL.y));
                paint::container(
                    ui.painter(),
                    grown,
                    Radii::all(shape::FULL),
                    s.secondary_container.gamma_multiply(k.clamp(0.0, 1.0)),
                    Stroke::NONE,
                );
            }
            let content = if selected { s.on_secondary_container } else { s.on_surface_variant };
            let layer = anim::fraction(
                &ctx,
                id.with("layer"),
                if resp.is_pointer_button_down_on() { state::PRESSED } else { 0.0 },
                theme.motion.fast_effects,
            );
            paint::state_layer(ui.painter(), pill, Radii::all(shape::FULL), content, layer);
            let icon = Rect::from_center_size(pill.center(), vec2(24.0, 24.0));
            item.icon.widget(selected, content, 24.0).paint_at(ui, icon);
            if let Some(count) = item.badge.filter(|&c| c > 0) {
                rail::badge(ui, icon.center() + vec2(6.0, -12.0), count, &s);
            }
            let style = if selected { TypeRole::LabelMedium.emphasized() } else { TypeRole::LabelMedium.style() };
            ui.painter().text(
                pos2(cell.center().x, pill.bottom() + LABEL_GAP),
                Align2::CENTER_TOP,
                item.label,
                style.font_id(),
                if selected { s.on_surface } else { s.on_surface_variant },
            );
            if resp.clicked() {
                tapped = Some(i);
            }
        }
        tapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_label_sits_at_48_with_16_dp_under_it_within_the_bar() {
        let label = TypeRole::LabelMedium.style().line_height;
        assert_eq!(TOP_PAD + PILL.y + LABEL_GAP, 48.0);
        assert!(TOP_PAD + PILL.y + LABEL_GAP + label + 16.0 <= HEIGHT);
    }
}
