//! Plain tooltip: `inverse-surface`, 4 dp corners, body-small text and an
//! optional keyboard shortcut. Replaces egui's `on_hover_text` styling.

use egui::{Color32, Frame, Label, Margin, Response, RichText, Stroke, TextWrapMode, Tooltip};

use crate::{Theme, TypeRole, tokens::shape};

/// Shows a plain tooltip when `response` is hovered. Returns the response so
/// calls chain: `tooltip(ui.add(button), "Next", Some("Ctrl+→"))`.
pub fn tooltip(response: Response, text: &str, shortcut: Option<&str>) -> Response {
    let s = Theme::get(&response.ctx).scheme;
    let frame = Frame {
        inner_margin: Margin::symmetric(8, 4),
        outer_margin: Margin::ZERO,
        corner_radius: egui::CornerRadius::same(shape::EXTRA_SMALL as u8),
        shadow: egui::epaint::Shadow::NONE,
        fill: s.inverse_surface,
        stroke: Stroke::NONE,
    };
    let style = TypeRole::BodySmall.style();
    let rich = |t: &str, c: Color32| -> RichText { style.rich(t, c) };
    let mut tip = Tooltip::for_enabled(&response);
    tip.popup = tip.popup.frame(frame);
    tip.show(|ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.add(Label::new(rich(text, s.inverse_on_surface)).wrap_mode(TextWrapMode::Extend));
            if let Some(sc) = shortcut {
                ui.add(
                    Label::new(rich(sc, s.inverse_on_surface.gamma_multiply(0.7)))
                        .wrap_mode(TextWrapMode::Extend),
                );
            }
        });
    });
    response
}

/// `response.md3_tooltip("Next", Some("Ctrl+→"))`.
pub trait TooltipExt: Sized {
    fn md3_tooltip(self, text: &str, shortcut: Option<&str>) -> Self;
}

impl TooltipExt for Response {
    fn md3_tooltip(self, text: &str, shortcut: Option<&str>) -> Self {
        tooltip(self, text, shortcut)
    }
}
