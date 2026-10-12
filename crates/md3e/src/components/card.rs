//! Cards: elevated / filled / outlined containers (radius 12), optionally
//! clickable with a state layer, and `MediaCard`, the album / playlist tile.

use egui::{
    Color32, CornerRadius, Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, WidgetInfo,
    WidgetType, pos2, vec2,
};

use super::{
    anim, badge,
    button::{IconButton, Size, Style},
    common, paint,
    paint::Radii,
};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum CardKind {
    Elevated,
    #[default]
    Filled,
    Outlined,
}

fn container_colors(kind: CardKind, s: &crate::ColorScheme) -> (Color32, Stroke, Elevation) {
    match kind {
        CardKind::Elevated => (s.surface_container_low, Stroke::NONE, Elevation::Level1),
        CardKind::Filled => (s.surface_container_highest, Stroke::NONE, Elevation::Level0),
        CardKind::Outlined => (
            s.surface,
            Stroke::new(1.0, s.outline_variant),
            Elevation::Level0,
        ),
    }
}

/// What a card frame produced.
pub struct CardOutput<R> {
    /// The card's response (`clicked()` when clickable).
    pub response: Response,
    /// What the content closure returned.
    pub inner: R,
}

/// A card container.
#[must_use = "call .show(ui, |ui| ...)"]
pub struct Card {
    kind: CardKind,
    clickable: bool,
    padding: f32,
    width: Option<f32>,
    min_height: f32,
    enabled: bool,
    label: Option<String>,
    id_salt: Option<Id>,
}

impl Card {
    pub fn new(kind: CardKind) -> Self {
        Self {
            kind,
            clickable: false,
            padding: 16.0,
            width: None,
            min_height: 0.0,
            enabled: true,
            label: None,
            id_salt: None,
        }
    }

    pub fn elevated() -> Self {
        Self::new(CardKind::Elevated)
    }

    pub fn filled() -> Self {
        Self::new(CardKind::Filled)
    }

    pub fn outlined() -> Self {
        Self::new(CardKind::Outlined)
    }

    /// Makes the whole card one tap target with a state layer.
    pub fn clickable(mut self, clickable: bool) -> Self {
        self.clickable = clickable;
        self
    }

    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    /// Fixed outer width (default: the available width).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn min_height(mut self, h: f32) -> Self {
        self.min_height = h;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Accessible name of a clickable card.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }

    /// The card's height follows its content from the previous frame, so a
    /// card whose content changes size lags by one frame.
    pub fn show<R>(self, ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> CardOutput<R> {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let s = theme.scheme;
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |salt| ui.id().with(salt));
        let width = self.width.unwrap_or_else(|| ui.available_width());
        let top_left = ui.cursor().min;
        let last_h: f32 = ctx
            .data(|d| d.get_temp(id.with("h")))
            .unwrap_or(self.min_height);
        let guess = Rect::from_min_size(top_left, vec2(width, last_h.max(self.min_height)));
        let sense = if self.clickable && self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(guess, id, sense);

        // Background first, from last frame's height.
        let radii = Radii::all(shape::MEDIUM);
        let (fill, border, rest) = container_colors(self.kind, &s);
        let interactive = self.clickable && self.enabled;
        let elevation = if interactive && response.hovered() && rest == Elevation::Level1 {
            Elevation::Level2
        } else {
            rest
        };
        if ui.is_rect_visible(guess) && last_h > 0.0 {
            let painter = ui.painter();
            paint::shadow(painter, guess, radii, elevation);
            paint::container(painter, guess, radii, fill, border);
        }

        let inner_rect = Rect::from_min_size(
            top_left + vec2(self.padding, self.padding),
            vec2(
                (width - 2.0 * self.padding).max(1.0),
                ui.available_size_before_wrap().y.max(1.0),
            ),
        );
        let mut content = ui.new_child(UiBuilder::new().max_rect(inner_rect));
        let inner = add_contents(&mut content);
        let used = content.min_rect();
        let height = (used.height() + 2.0 * self.padding).max(self.min_height);
        let total = Rect::from_min_size(top_left, vec2(width, height));
        ui.advance_cursor_after_rect(total);
        if (height - last_h).abs() > 0.5 {
            ctx.data_mut(|d| d.insert_temp(id.with("h"), height));
            ctx.request_repaint();
        }

        if interactive && last_h > 0.0 {
            let layer = common::layer_opacity(&ctx, id, &response, true);
            paint::state_layer(ui.painter(), guess, radii, s.on_surface, layer);
            if common::focused(&response) {
                paint::focus_ring(ui.painter(), guess, radii, s.secondary);
            }
        }
        if self.clickable {
            let label = self.label.unwrap_or_default();
            let enabled = self.enabled;
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, &label));
        }
        CardOutput { response, inner }
    }
}

/// Which part of a media card was clicked.
pub struct MediaCardOutput {
    /// The whole card.
    pub response: Response,
    pub card_clicked: bool,
    /// The play button over the cover.
    pub play_clicked: bool,
    pub card_secondary_clicked: bool,
}

/// An album / playlist tile: a square cover, title and subtitle, an optional
/// badge on the cover and a play button that fades in on hover.
#[must_use = "call .show(ui)"]
pub struct MediaCard {
    title: String,
    subtitle: Option<String>,
    cover: Option<String>,
    placeholder: Icon,
    badge: Option<String>,
    width: f32,
    container: Option<CardKind>,
    play_button: bool,
    always_show_play: bool,
    id_salt: Option<Id>,
}

impl MediaCard {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            cover: None,
            placeholder: Icon::Album,
            badge: None,
            width: 160.0,
            container: Some(CardKind::Filled),
            play_button: true,
            always_show_play: false,
            id_salt: None,
        }
    }

    pub fn subtitle(mut self, text: impl Into<String>) -> Self {
        self.subtitle = Some(text.into());
        self
    }

    /// An image URI (`file://`, `bytes://`, `http://`...).
    pub fn cover(mut self, uri: impl Into<String>) -> Self {
        self.cover = Some(uri.into());
        self
    }

    pub fn cover_opt(mut self, uri: Option<String>) -> Self {
        self.cover = uri;
        self
    }

    /// The icon on the tonal placeholder tile (default: album).
    pub fn placeholder_icon(mut self, icon: Icon) -> Self {
        self.placeholder = icon;
        self
    }

    /// A small chip on the cover, for example `3/12`.
    pub fn badge(mut self, text: impl Into<String>) -> Self {
        self.badge = Some(text.into());
        self
    }

    pub fn badge_opt(mut self, text: Option<String>) -> Self {
        self.badge = text;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// The container kind (default filled); `None` draws a flat tile.
    pub fn container(mut self, kind: Option<CardKind>) -> Self {
        self.container = kind;
        self
    }

    /// Hides the play button.
    pub fn play_button(mut self, show: bool) -> Self {
        self.play_button = show;
        self
    }

    /// Keeps the play button visible without hover (touch).
    pub fn always_show_play(mut self, always: bool) -> Self {
        self.always_show_play = always;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }

    pub fn show(self, ui: &mut Ui) -> MediaCardOutput {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |salt| ui.id().with(salt));
        let w = self.width;
        let (rect, _) = ui.allocate_exact_size(vec2(w, w + 56.0), Sense::hover());
        let response = ui.interact(rect, id, Sense::click());
        let radii = Radii::all(shape::MEDIUM);
        let hover = response.contains_pointer() || common::focused(&response);
        // Playful feedback: the picture leans in under the pointer and the
        // whole card squishes on press, both on springs.
        let squish = anim::f32(
            &ctx,
            id.with("squish"),
            if response.is_pointer_button_down_on() { 0.95 } else { 1.0 },
            motion.fast_spatial,
        );
        let zoom = 1.0 + 0.06 * anim::fraction(&ctx, id.with("zoom"), f32::from(u8::from(hover)), motion.default_effects);
        let layout_rect = rect;
        let rect = Rect::from_center_size(rect.center(), rect.size() * squish);
        let w = w * squish;

        if ui.is_rect_visible(layout_rect) {
            let painter = ui.painter().clone();
            let pad = if let Some(kind) = self.container {
                let (fill, border, elevation) = container_colors(kind, &s);
                let e = if response.hovered() && elevation == Elevation::Level1 {
                    Elevation::Level2
                } else {
                    elevation
                };
                paint::shadow(&painter, rect, radii, e);
                paint::container(&painter, rect, radii, fill, border);
                12.0
            } else {
                2.0
            };

            // Cover.
            let cover = Rect::from_min_size(rect.min, vec2(w, w));
            let r = shape::MEDIUM as u8;
            let corner = if self.container.is_some() {
                CornerRadius {
                    nw: r,
                    ne: r,
                    se: 0,
                    sw: 0,
                }
            } else {
                CornerRadius::same(r)
            };
            common::paint_image_zoomed(ui, cover, self.cover.as_deref(), corner, self.placeholder, zoom);
            if let Some(text) = &self.badge {
                badge::text_badge(
                    &painter,
                    pos2(cover.left() + 8.0, cover.top() + 16.0),
                    text,
                    s.secondary_container,
                    s.on_secondary_container,
                );
            }

            // Text.
            let text_w = w - 2.0 * pad;
            let title = common::line(
                &painter,
                &self.title,
                TypeRole::TitleSmall.style(),
                s.on_surface,
                text_w,
            );
            painter.galley(
                pos2(rect.left() + pad, cover.bottom() + 8.0),
                title,
                s.on_surface,
            );
            if let Some(sub) = &self.subtitle {
                let g = common::line(
                    &painter,
                    sub,
                    TypeRole::BodySmall.style(),
                    s.on_surface_variant,
                    text_w,
                );
                painter.galley(
                    pos2(rect.left() + pad, cover.bottom() + 28.0),
                    g,
                    s.on_surface_variant,
                );
            }

            let layer = common::layer_opacity(&ctx, id, &response, true);
            paint::state_layer(&painter, rect, radii, s.on_surface, layer);
            if common::focused(&response) {
                paint::focus_ring(&painter, rect, radii, s.secondary);
            }
        }

        // Play button over the cover, fading in with the pointer.
        let mut play_clicked = false;
        if self.play_button {
            let target = if hover || self.always_show_play {
                1.0
            } else {
                0.0
            };
            let k =
                anim::fraction(&ctx, id.with("play"), target, motion.fast_effects).clamp(0.0, 1.0);
            if k > 0.01 {
                let size = Size::M.height();
                let slot = Rect::from_min_size(
                    pos2(rect.left() + w - 8.0 - size, rect.top() + w - 8.0 - size),
                    vec2(size, size),
                );
                // A child, not a scope: the overlay must not move the
                // parent's cursor (the next card would slide under this one).
                let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt((id, "play-slot")));
                child.set_opacity(k);
                play_clicked = child
                    .add_enabled(
                        k > 0.5,
                        IconButton::new(Icon::PlayArrow)
                            .size(Size::M)
                            .style(Style::Filled)
                            .filled_icon(true)
                            .id_salt((id, "play-btn")),
                    )
                    .clicked();
            }
        }

        let label = match &self.subtitle {
            Some(sub) => format!("{}, {sub}", self.title),
            None => self.title.clone(),
        };
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
        MediaCardOutput {
            card_clicked: response.clicked() && !play_clicked,
            card_secondary_clicked: response.secondary_clicked(),
            play_clicked,
            response,
        }
    }
}
