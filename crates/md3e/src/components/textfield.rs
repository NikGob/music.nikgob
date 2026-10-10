//! Text fields (outlined and filled): 56 dp, a label that floats to the top
//! when focused or filled (fast spatial), leading/trailing icons, supporting
//! and error text. Wraps `egui::TextEdit` with its frame disabled.

use egui::{
    Align, Color32, Frame, Id, Margin, Rect, Response, RichText, Sense, Stroke, TextEdit, Ui,
    UiBuilder, pos2, vec2,
};

use super::{anim, common, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{shape, state},
};

pub const HEIGHT: f32 = 56.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum FieldStyle {
    #[default]
    Outlined,
    Filled,
}

/// What a text field frame produced.
pub struct TextFieldOutput {
    /// The editor's response (`changed()`, `lost_focus()`, `has_focus()`).
    pub response: Response,
    /// The 56 dp container (without the supporting text row).
    pub rect: Rect,
    pub leading_clicked: bool,
    pub trailing_clicked: bool,
    /// Enter was pressed while the field had focus.
    pub submitted: bool,
}

/// An outlined or filled single-line text field.
#[must_use = "call .show(ui)"]
pub struct TextField<'a> {
    text: &'a mut String,
    label: String,
    style: FieldStyle,
    placeholder: Option<String>,
    supporting: Option<String>,
    error: Option<String>,
    leading: Option<Icon>,
    trailing: Option<Icon>,
    enabled: bool,
    password: bool,
    width: Option<f32>,
    background: Option<Color32>,
    focus: bool,
    id_salt: Option<Id>,
}

impl<'a> TextField<'a> {
    pub fn new(text: &'a mut String, label: impl Into<String>) -> Self {
        Self {
            text,
            label: label.into(),
            style: FieldStyle::Outlined,
            placeholder: None,
            supporting: None,
            error: None,
            leading: None,
            trailing: None,
            enabled: true,
            password: false,
            width: None,
            background: None,
            focus: false,
            id_salt: None,
        }
    }

    pub fn outlined(mut self) -> Self {
        self.style = FieldStyle::Outlined;
        self
    }

    pub fn filled(mut self) -> Self {
        self.style = FieldStyle::Filled;
        self
    }

    pub fn style(mut self, style: FieldStyle) -> Self {
        self.style = style;
        self
    }

    /// Hint shown inside the field while it is focused and empty.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    pub fn supporting(mut self, text: impl Into<String>) -> Self {
        self.supporting = Some(text.into());
        self
    }

    /// Puts the field in the error state; the text replaces the supporting text.
    pub fn error(mut self, text: impl Into<String>) -> Self {
        self.error = Some(text.into());
        self
    }

    /// Like [`Self::error`] but only when `text` is `Some`.
    pub fn error_opt(mut self, text: Option<String>) -> Self {
        self.error = text;
        self
    }

    pub fn leading_icon(mut self, icon: Icon) -> Self {
        self.leading = Some(icon);
        self
    }

    pub fn trailing_icon(mut self, icon: Icon) -> Self {
        self.trailing = Some(icon);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn password(mut self, password: bool) -> Self {
        self.password = password;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// The colour behind the field; the outlined label notch is cut with it
    /// (default `surface`; use `surface_container_high` inside dialogs).
    pub fn background(mut self, color: Color32) -> Self {
        self.background = Some(color);
        self
    }

    /// Grabs keyboard focus this frame (pass true once, when it appears).
    pub fn request_focus(mut self, focus: bool) -> Self {
        self.focus = focus;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }

    pub fn show(self, ui: &mut Ui) -> TextFieldOutput {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |salt| ui.id().with(salt));
        let edit_id = id.with("edit");

        let width = self.width.unwrap_or_else(|| ui.available_width());
        let support = self.error.as_deref().or(self.supporting.as_deref());
        let total_h = HEIGHT + if support.is_some() { 20.0 } else { 0.0 };
        let (outer, _) = ui.allocate_exact_size(vec2(width, total_h), Sense::hover());
        let rect = Rect::from_min_size(outer.min, vec2(width, HEIGHT));

        let container = ui.interact(
            rect,
            id,
            if self.enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        if container.clicked() {
            ctx.memory_mut(|m| m.request_focus(edit_id));
        }
        let focused = self.enabled && ctx.memory(|m| m.has_focus(edit_id));
        let hovered = self.enabled && container.contains_pointer();
        let invalid = self.error.is_some();
        let filled_style = self.style == FieldStyle::Filled;

        // Colours.
        let outline = if !self.enabled {
            common::fade(s.on_surface, state::DISABLED_CONTAINER)
        } else if invalid {
            if hovered && !focused {
                s.on_error_container
            } else {
                s.error
            }
        } else if focused {
            s.primary
        } else if hovered {
            s.on_surface
        } else if filled_style {
            s.on_surface_variant
        } else {
            s.outline
        };
        let label_color = if !self.enabled {
            common::fade(s.on_surface, state::DISABLED_CONTENT)
        } else if invalid {
            s.error
        } else if focused {
            s.primary
        } else {
            s.on_surface_variant
        };
        let text_color = if self.enabled {
            s.on_surface
        } else {
            common::fade(s.on_surface, state::DISABLED_CONTENT)
        };
        let icon_color = if self.enabled {
            s.on_surface_variant
        } else {
            common::fade(s.on_surface, state::DISABLED_CONTENT)
        };
        let outline = anim::color(&ctx, id.with("outline"), outline, motion.fast_effects);
        let label_color = anim::color(&ctx, id.with("label-c"), label_color, motion.fast_effects);
        let stroke_w = anim::fraction(
            &ctx,
            id.with("stroke"),
            if focused { 1.0 } else { 0.0 },
            motion.fast_effects,
        );
        let stroke_w = 1.0 + stroke_w.clamp(0.0, 1.0);

        let floated = focused || !self.text.is_empty();
        let float = anim::fraction(
            &ctx,
            id.with("float"),
            f32::from(u8::from(floated)),
            motion.fast_spatial,
        );

        let painter = ui.painter().clone();
        let radii = if filled_style {
            Radii([shape::EXTRA_SMALL, shape::EXTRA_SMALL, 0.0, 0.0])
        } else {
            Radii::all(shape::EXTRA_SMALL)
        };

        // Container.
        if filled_style {
            let fill = if self.enabled {
                s.surface_container_highest
            } else {
                common::fade(s.on_surface, state::DISABLED_CONTAINER)
            };
            paint::container(&painter, rect, radii, fill, Stroke::NONE);
            let layer = anim::fraction(
                &ctx,
                id.with("layer"),
                if hovered && !focused {
                    state::HOVER
                } else {
                    0.0
                },
                motion.fast_effects,
            );
            paint::state_layer(&painter, rect, radii, s.on_surface, layer);
            let y = rect.bottom() - stroke_w * 0.5;
            painter.line_segment(
                [pos2(rect.left(), y), pos2(rect.right(), y)],
                Stroke::new(stroke_w, outline),
            );
        } else {
            let inset = rect.shrink(stroke_w * 0.5);
            paint::container(
                &painter,
                inset,
                radii.shrink(stroke_w * 0.5),
                Color32::TRANSPARENT,
                Stroke::new(stroke_w, outline),
            );
        }

        // Label: body-large at rest, body-small floated.
        let text_left = rect.left() + if self.leading.is_some() { 52.0 } else { 16.0 };
        let trailing_icon = self
            .trailing
            .or(if invalid { Some(Icon::Error) } else { None });
        let text_right = rect.right() - if trailing_icon.is_some() { 52.0 } else { 16.0 };
        if !self.label.is_empty() {
            let k = float.clamp(0.0, 1.0);
            let size = 16.0 + (12.0 - 16.0) * k;
            let rest_style = TypeRole::BodyLarge.style();
            let mut style = rest_style;
            style.size = size;
            style.line_height = 24.0 + (16.0 - 24.0) * k;
            let galley = common::plain(&painter, &self.label, style, label_color);
            let float_y = if filled_style {
                rect.top() + 8.0 + galley.size().y * 0.5
            } else {
                rect.top()
            };
            let cy = rect.center().y + (float_y - rect.center().y) * float;
            let x = if filled_style || self.leading.is_none() {
                text_left
            } else {
                // An outlined label floats back above the leading icon.
                text_left + (rect.left() + 16.0 - text_left) * k
            };
            let pos = pos2(x, cy - galley.size().y * 0.5);
            if !filled_style && k > 0.01 {
                // Cut the border under the floated label.
                let bg = self.background.unwrap_or(s.surface);
                let notch = Rect::from_min_size(
                    pos2(pos.x - 4.0, rect.top() - stroke_w),
                    vec2(galley.size().x + 8.0, stroke_w * 2.0),
                );
                painter.rect_filled(notch, 0.0, bg.gamma_multiply(k));
            }
            painter.galley(pos, galley, label_color);
        }

        // Editor.
        let cy = if filled_style && !self.label.is_empty() {
            rect.center().y + 8.0
        } else {
            rect.center().y
        };
        let text_rect = Rect::from_min_max(
            pos2(text_left, cy - 12.0),
            pos2(text_right.max(text_left + 8.0), cy + 12.0),
        );
        let placeholder = self.placeholder.clone();
        let enabled = self.enabled;
        let password = self.password;
        let want_focus = self.focus;
        let text = self.text;
        // A child Ui: the field's space is already allocated, so the editor
        // must not move the parent's cursor again.
        let mut child = ui.new_child(UiBuilder::new().max_rect(text_rect));
        let edit = {
            let ui = &mut child;
            {
                let mut te = TextEdit::singleline(text)
                    .id(edit_id)
                    .frame(Frame::NONE)
                    .margin(Margin::ZERO)
                    .font(TypeRole::BodyLarge.font_id())
                    .text_color(text_color)
                    .desired_width(text_rect.width())
                    .vertical_align(Align::Center)
                    .interactive(enabled)
                    .password(password);
                if float > 0.5
                    && let Some(p) = placeholder
                {
                    te = te
                        .hint_text(RichText::new(p).color(common::fade(s.on_surface_variant, 0.8)));
                }
                te.show(ui)
            }
        };
        let response = edit.response.response;
        if want_focus {
            response.request_focus();
        }
        let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

        // Icons.
        let mut leading_clicked = false;
        let mut trailing_clicked = false;
        if let Some(icon) = self.leading {
            let r =
                Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), vec2(48.0, 48.0));
            let resp = ui.interact(
                r,
                id.with("lead"),
                if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            leading_clicked = resp.clicked();
            icon.widget(false, icon_color, 24.0).paint_at(
                ui,
                Rect::from_center_size(pos2(rect.left() + 28.0, rect.center().y), vec2(24.0, 24.0)),
            );
        }
        if let Some(icon) = trailing_icon {
            let c = pos2(rect.right() - 28.0, rect.center().y);
            let resp = ui.interact(
                Rect::from_center_size(c, vec2(48.0, 48.0)),
                id.with("trail"),
                if enabled && self.trailing.is_some() {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            trailing_clicked = resp.clicked();
            let color = if invalid && self.trailing.is_none() {
                s.error
            } else {
                icon_color
            };
            icon.widget(false, color, 24.0)
                .paint_at(ui, Rect::from_center_size(c, vec2(24.0, 24.0)));
        }

        // Supporting / error text.
        if let Some(t) = support {
            let color = if invalid {
                s.error
            } else {
                s.on_surface_variant
            };
            let galley = common::line(
                &painter,
                t,
                TypeRole::BodySmall.style(),
                color,
                rect.width() - 32.0,
            );
            painter.galley(pos2(rect.left() + 16.0, rect.bottom() + 4.0), galley, color);
        }

        // Focus is shown by the thicker primary outline; no extra ring.
        TextFieldOutput {
            response,
            rect,
            leading_clicked,
            trailing_clicked,
            submitted,
        }
    }
}
