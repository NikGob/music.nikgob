//! Floating toolbar container (64 tall, full shape, level 3, 8 inner padding,
//! 4 between items). Vibrant = primary-container, standard = surface-container.

use egui::{Color32, Rect, Stroke, Ui, UiBuilder};

use super::{
    button::Colors,
    paint::{self, Radii},
};
use crate::{
    Theme,
    tokens::{Elevation, shape},
};

pub const HEIGHT: f32 = 64.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToolbarColor {
    Standard,
    #[default]
    Vibrant,
}

/// Container and content colours of a toolbar.
pub fn colors(color: ToolbarColor, s: &crate::ColorScheme) -> (Color32, Color32) {
    match color {
        ToolbarColor::Vibrant => (s.primary_container, s.on_primary_container),
        ToolbarColor::Standard => (s.surface_container, s.on_surface_variant),
    }
}

/// Colours for an icon button sitting on the toolbar without its own container.
pub fn item_colors(color: ToolbarColor, s: &crate::ColorScheme) -> Colors {
    let (_, content) = colors(color, s);
    Colors {
        container: Color32::TRANSPARENT,
        content,
        border: Color32::TRANSPARENT,
    }
}

/// Paints the toolbar pill at `rect` and lays out `add_contents` inside it.
/// Also returns the pill's own response (clicks between the items).
pub fn floating_toolbar<R>(
    ui: &mut Ui,
    rect: Rect,
    color: ToolbarColor,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> (egui::Response, R) {
    let theme = Theme::get(ui.ctx());
    let (bg, _) = colors(color, &theme.scheme);
    let radii = Radii::all(shape::FULL);
    paint::shadow(ui.painter(), rect, radii, Elevation::Level3);
    paint::container(ui.painter(), rect, radii, bg, Stroke::NONE);
    // Swallow clicks on the pill itself so content beneath does not get them.
    let bg = ui.interact(rect, ui.id().with("toolbar-bg"), egui::Sense::click());
    let inner = rect.shrink2(egui::vec2(8.0, 4.0));
    let r = ui.scope_builder(
        UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            add_contents(ui)
        },
    )
    .inner;
    (bg, r)
}
