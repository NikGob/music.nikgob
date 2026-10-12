//! Dialogs: a basic dialog (`surface-container-high`, radius 28, level 3,
//! 32 % scrim over the window, scale + fade in on fast springs, Esc or a
//! click on the scrim closes) and a text-input variant for "new playlist" /
//! "magnet link".
//!
//! The dialog layer sits above `Order::Foreground`, so menus opened from
//! inside a dialog would appear below it; keep dialog content simple.

use egui::{
    Align, Align2, Area, Context, Frame, Id, Key, LayerId, Layout, Margin, Modifiers, Order,
    Stroke, Ui,
};

use super::{
    button::{Button, Size, Style},
    common,
    textfield::TextField,
};
use crate::{Icon, Theme, TypeRole, tokens::Elevation};

pub const MIN_WIDTH: f32 = 280.0;
pub const MAX_WIDTH: f32 = 560.0;

/// A text button at the bottom of a dialog.
#[derive(Clone, Debug)]
pub struct DialogAction {
    pub label: String,
    pub enabled: bool,
}

impl DialogAction {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// What the dialog reported this frame.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DialogOutput {
    /// Index of the action that was clicked (the dialog is closed).
    pub action: Option<usize>,
    /// Closed by Esc or a click on the scrim.
    pub dismissed: bool,
}

/// A basic dialog.
#[must_use = "call .show(ctx, &mut open)"]
pub struct Dialog {
    id: Id,
    icon: Option<Icon>,
    headline: String,
    supporting: Option<String>,
    actions: Vec<DialogAction>,
    max_width: f32,
    dismissible: bool,
}

impl Dialog {
    pub fn new(id_salt: impl egui::AsId, headline: impl Into<String>) -> Self {
        Self {
            id: Id::new(id_salt),
            icon: None,
            headline: headline.into(),
            supporting: None,
            actions: Vec::new(),
            max_width: MAX_WIDTH,
            dismissible: true,
        }
    }

    /// A 24 dp icon in `secondary` above the headline (both then centred).
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn supporting(mut self, text: impl Into<String>) -> Self {
        self.supporting = Some(text.into());
        self
    }

    /// Adds a text-button action; they are laid out right-aligned in order.
    pub fn action(mut self, action: impl Into<String>) -> Self {
        self.actions.push(DialogAction::new(action));
        self
    }

    pub fn action_with(mut self, action: DialogAction) -> Self {
        self.actions.push(action);
        self
    }

    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = w.max(MIN_WIDTH);
        self
    }

    /// Whether Esc and the scrim close the dialog (default true).
    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }

    pub fn show(self, ctx: &Context, open: &mut bool) -> DialogOutput {
        self.show_with(ctx, open, |_| {})
    }

    /// Like [`Self::show`] with extra content between the supporting text
    /// and the actions.
    pub fn show_with(
        self,
        ctx: &Context,
        open: &mut bool,
        content: impl FnOnce(&mut Ui),
    ) -> DialogOutput {
        let theme = Theme::get(ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let was_open = *open;
        let layer = LayerId::new(Order::Tooltip, self.id.with("dialog"));
        let pres = common::presence(
            ctx,
            self.id,
            was_open,
            motion.fast_spatial,
            motion.fast_effects,
        );
        let mut out = DialogOutput::default();
        if !pres.visible() {
            common::layer_scale(ctx, layer, egui::Pos2::ZERO, 1.0);
            return out;
        }
        let screen = ctx.content_rect();

        let scrim = common::scrim(ctx, self.id.with("scrim"), pres.alpha());
        if was_open && self.dismissible {
            let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
            if scrim.clicked() || esc {
                out.dismissed = true;
            }
        }

        let width = (screen.width() - 48.0).clamp(MIN_WIDTH, self.max_width);
        let frame = Frame {
            inner_margin: Margin::same(24),
            outer_margin: Margin::ZERO,
            corner_radius: egui::CornerRadius::same(28),
            shadow: Elevation::Level3.shadow(),
            fill: s.surface_container_high,
            stroke: Stroke::NONE,
        };
        let centered = self.icon.is_some();
        let alpha = pres.alpha();
        let mut clicked = None;
        let actions = self.actions;
        let area = Area::new(self.id.with("dialog"))
            .fade_in(false)
            .order(Order::Tooltip)
            .fixed_pos(screen.center())
            .pivot(Align2::CENTER_CENTER)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                frame.show(ui, |ui| {
                    ui.set_width(width - 48.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let header = if centered {
                        Layout::top_down(Align::Center)
                    } else {
                        Layout::top_down(Align::Min)
                    };
                    ui.with_layout(header, |ui| {
                        if let Some(icon) = self.icon {
                            ui.add(icon.widget(false, s.secondary, 24.0));
                            ui.add_space(16.0);
                        }
                        ui.add(
                            egui::Label::new(
                                TypeRole::HeadlineSmall
                                    .style()
                                    .rich_para(&self.headline, s.on_surface),
                            )
                            .wrap(),
                        );
                        if let Some(text) = &self.supporting {
                            ui.add_space(16.0);
                            ui.add(
                                egui::Label::new(
                                    TypeRole::BodyMedium
                                        .style()
                                        .rich_para(text, s.on_surface_variant),
                                )
                                .wrap(),
                            );
                        }
                    });
                    content(ui);
                    if !actions.is_empty() {
                        ui.add_space(24.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            for (i, a) in actions.iter().enumerate().rev() {
                                let r = ui.add(
                                    Button::new(&a.label)
                                        .style(Style::Text)
                                        .size(Size::S)
                                        .enabled(a.enabled)
                                        .id_salt(("dialog-action", i)),
                                );
                                if r.clicked() && a.enabled {
                                    clicked = Some(i);
                                }
                            }
                        });
                    }
                });
            });
        common::layer_scale(
            ctx,
            area.response.layer_id,
            screen.center(),
            0.85 + 0.15 * pres.spatial,
        );

        if was_open {
            if let Some(i) = clicked {
                out.action = Some(i);
            }
            if out.action.is_some() || out.dismissed {
                *open = false;
            }
        }
        out
    }
}

/// What a text-input dialog resolved to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextInputResult {
    Confirmed,
    Cancelled,
}

/// A dialog with one outlined text field ("Новый плейлист", "Magnet-ссылка").
#[must_use = "call .show(ctx, &mut open, &mut text)"]
pub struct TextInputDialog {
    id: Id,
    headline: String,
    supporting: Option<String>,
    label: String,
    placeholder: Option<String>,
    confirm: String,
    cancel: String,
    icon: Option<Icon>,
    #[allow(clippy::type_complexity)]
    validate: Option<Box<dyn Fn(&str) -> Option<String>>>,
}

impl TextInputDialog {
    pub fn new(
        id_salt: impl egui::AsId,
        headline: impl Into<String>,
        field_label: impl Into<String>,
    ) -> Self {
        Self {
            id: Id::new(id_salt),
            headline: headline.into(),
            supporting: None,
            label: field_label.into(),
            placeholder: None,
            confirm: "OK".to_owned(),
            cancel: "Cancel".to_owned(),
            icon: None,
            validate: None,
        }
    }

    pub fn supporting(mut self, text: impl Into<String>) -> Self {
        self.supporting = Some(text.into());
        self
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Button labels (confirm, cancel).
    pub fn buttons(mut self, confirm: impl Into<String>, cancel: impl Into<String>) -> Self {
        self.confirm = confirm.into();
        self.cancel = cancel.into();
        self
    }

    /// Returns an error message to show (and to block confirming) for an
    /// input, or `None` when it is fine. Empty input is always rejected
    /// without a message.
    pub fn validate(mut self, f: impl Fn(&str) -> Option<String> + 'static) -> Self {
        self.validate = Some(Box::new(f));
        self
    }

    pub fn show(
        self,
        ctx: &Context,
        open: &mut bool,
        text: &mut String,
    ) -> Option<TextInputResult> {
        let s = Theme::get(ctx).scheme;
        let focus_key = self.id.with("focused");
        let first_frame = *open && !ctx.data(|d| d.get_temp::<bool>(focus_key)).unwrap_or(false);
        if *open {
            ctx.data_mut(|d| d.insert_temp(focus_key, true));
        } else {
            ctx.data_mut(|d| d.remove::<bool>(focus_key));
        }

        let message = self.validate.as_ref().and_then(|v| {
            if text.trim().is_empty() {
                None
            } else {
                v(text)
            }
        });
        let valid = !text.trim().is_empty() && message.is_none();

        let mut dialog = Dialog::new(self.id, self.headline)
            .action_with(DialogAction::new(self.cancel))
            .action_with(DialogAction::new(self.confirm).enabled(valid));
        if let Some(sup) = self.supporting {
            dialog = dialog.supporting(sup);
        }
        if let Some(icon) = self.icon {
            dialog = dialog.icon(icon);
        }
        let mut submitted = false;
        let label = self.label;
        let placeholder = self.placeholder;
        let out = dialog.show_with(ctx, open, |ui| {
            ui.add_space(24.0);
            let mut field = TextField::new(text, label)
                .outlined()
                .background(s.surface_container_high)
                .error_opt(message)
                .request_focus(first_frame)
                .id_salt("text-input");
            if let Some(p) = placeholder {
                field = field.placeholder(p);
            }
            submitted = field.show(ui).submitted;
        });
        if submitted && valid && *open {
            *open = false;
            return Some(TextInputResult::Confirmed);
        }
        match out.action {
            Some(1) => Some(TextInputResult::Confirmed),
            Some(_) => Some(TextInputResult::Cancelled),
            None if out.dismissed => Some(TextInputResult::Cancelled),
            None => None,
        }
    }
}
