//! Plain tooltip: `inverse-surface`, 4 dp corners, body-small text and an
//! optional keyboard shortcut. Replaces egui's `on_hover_text` styling.

use egui::{Color32, Frame, Label, Margin, Response, Stroke, TextWrapMode, Tooltip};

use crate::{Theme, TypeRole, tokens::shape};

/// Whether hints show on hover («Подсказки при наведении»), for every
/// tooltip of the app: plain ones and egui's `on_hover_text`.
pub fn set_enabled(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("md3e-tooltips"), on));
    // egui's own tooltips wait this long; off, they never come. Checked
    // every frame: a new theme brings a fresh style.
    let delay = if on { egui::style::Interaction::default().tooltip_delay } else { f32::INFINITY };
    if ctx.global_style().interaction.tooltip_delay != delay {
        ctx.all_styles_mut(|s| s.interaction.tooltip_delay = delay);
    }
}

fn enabled(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(egui::Id::new("md3e-tooltips"))).unwrap_or(true)
}

/// Shows a plain tooltip when `response` is hovered. Returns the response so
/// calls chain: `tooltip(ui.add(button), "Next", Some("Ctrl+→"))`.
pub fn tooltip(response: Response, text: &str, shortcut: Option<&str>) -> Response {
    if !enabled(&response.ctx) {
        return response;
    }
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
    let rich = |t: &str, c: Color32| -> egui::WidgetText { style.rich(t, c) };
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
