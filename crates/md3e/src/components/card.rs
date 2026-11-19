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

/// What a media card says about its album in the cover corner, next to
/// (or instead of) its badge text.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CoverStatus {
    /// Nothing to say: only the badge text, if any.
    #[default]
    None,
    /// Waiting for its download: «в очереди».
    Queued,
    /// Downloading: a small ring, filled to the fraction when known.
    Progress(Option<f32>),
    /// Fully on disk: a check before the badge text («у тебя»).
    Done,
}

/// Which part of a media card was clicked.
pub struct MediaCardOutput {
    /// The whole card.
    pub response: Response,
    pub card_clicked: bool,
    /// The play button over the cover.
    pub play_clicked: bool,
    /// The ⬇ button shown on hover ([`MediaCard::download_button`]).
    pub download_clicked: bool,
    /// The ⋮ button shown on hover ([`MediaCard::menu_button`]), for a
    /// menu attached to it.
    pub menu: Option<Response>,
    pub card_secondary_clicked: bool,
}

/// An album / playlist tile: a square cover, title and subtitle, an optional
/// badge on the cover and a play button that fades in on hover.
#[must_use = "call .show(ui)"]
pub struct MediaCard {
    title: String,
    subtitle: Option<String>,
    cover: Option<String>,
    /// Four covers drawn 2×2 instead of `cover` (a mix of many albums).
    collage: Vec<String>,
    /// The title may take two lines (only without a subtitle).
    title_two_lines: bool,
    placeholder: Icon,
    badge: Option<String>,
    width: f32,
    container: Option<CardKind>,
    play_button: bool,
    always_show_play: bool,
    status: CoverStatus,
    download: Option<String>,
    menu_button: bool,
    id_salt: Option<Id>,
}

impl MediaCard {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            cover: None,
            collage: Vec::new(),
            title_two_lines: false,
            placeholder: Icon::Album,
            badge: None,
            width: 160.0,
            container: Some(CardKind::Filled),
            play_button: true,
            always_show_play: false,
            status: CoverStatus::None,
            download: None,
            menu_button: false,
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

    /// Covers of a mix: four or more are drawn as a 2×2 collage (the first
    /// four), fewer as the first alone (the same as [`Self::cover`]).
    pub fn collage(mut self, uris: Vec<String>) -> Self {
        if uris.len() < 4 {
            self.cover = uris.into_iter().next();
        } else {
            self.collage = uris;
        }
        self
    }

    /// Lets a long title wrap onto the subtitle's line (a card without a
    /// subtitle, like a mix named by its artists).
    pub fn title_two_lines(mut self, on: bool) -> Self {
        self.title_two_lines = on;
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

    /// A status in the badge corner of the cover ([`CoverStatus`]).
    pub fn status(mut self, status: CoverStatus) -> Self {
        self.status = status;
        self
    }

    /// A small tonal ⬇ button next to ▶ on hover, with this tooltip.
    pub fn download_button(mut self, tooltip: Option<impl Into<String>>) -> Self {
        self.download = tooltip.map(Into::into);
        self
    }

    /// A ⋮ button at the end of the title lines on hover.
    pub fn menu_button(mut self, show: bool) -> Self {
        self.menu_button = show;
        self
    }

    /// How tall a card `width` wide is (its cover and two lines under it).
    pub fn height(width: f32) -> f32 {
        width + 56.0
    }

    pub fn show(self, ui: &mut Ui) -> MediaCardOutput {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |salt| ui.id().with(salt));
        let w = self.width;
        let (rect, _) = ui.allocate_exact_size(vec2(w, Self::height(w)), Sense::hover());
        let response = ui.interact(rect, id, Sense::click());
        let radii = Radii::all(shape::MEDIUM);
        // While the menu of its ⋮ is open (below the card, so the pointer
        // leaves it) the card stays as under the pointer and keeps its ⋮.
        let more_open = self.menu_button && ctx.data(|d| d.get_temp::<bool>(id.with("more-open"))).unwrap_or(false);
        let hover = response.contains_pointer() || common::focused(&response) || more_open;
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
            if self.collage.len() >= 4 {
                paint_collage(ui, cover, &self.collage[..4], corner, self.placeholder, zoom);
            } else {
                common::paint_image_zoomed(ui, cover, self.cover.as_deref(), corner, self.placeholder, zoom);
            }
            let corner_at = pos2(cover.left() + 8.0, cover.top() + 16.0);
            match (self.status, &self.badge) {
                (CoverStatus::None, Some(text)) => {
                    badge::text_badge(&painter, corner_at, text, s.secondary_container, s.on_secondary_container);
                }
                (CoverStatus::None, None) => {}
                (status, text) => status_chip(ui, &painter, corner_at, status, text.as_deref()),
            }

            // Text (shorter while the ⋮ shows at its end).
            let text_w = w - 2.0 * pad - if self.menu_button && hover { 28.0 } else { 0.0 };
            let title = if self.title_two_lines && self.subtitle.is_none() {
                let mut job = TypeRole::TitleSmall.style().job(&self.title, s.on_surface);
                job.wrap = egui::text::TextWrapping {
                    max_width: text_w.max(1.0),
                    max_rows: 2,
                    break_anywhere: false,
                    overflow_character: Some('…'),
                };
                crate::typography::layout(&painter, job)
            } else {
                common::line(&painter, &self.title, TypeRole::TitleSmall.style(), s.on_surface, text_w)
            };
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

        // ⬇ next to ▶, and ⋮ after the title lines, with the pointer.
        let mut download_clicked = false;
        let mut menu = None;
        let k = anim::fraction(&ctx, id.with("extras"), f32::from(u8::from(hover)), motion.fast_effects).clamp(0.0, 1.0);
        if k > 0.01 {
            if let Some(tip) = &self.download {
                let play = Size::M.height();
                let size = Size::Xs.height();
                let c = pos2(
                    rect.left() + w - 8.0 - play - 8.0 - size * 0.5,
                    rect.top() + w - 8.0 - play * 0.5,
                );
                let mut child = ui.new_child(
                    UiBuilder::new().max_rect(Rect::from_center_size(c, vec2(size, size))).id_salt((id, "dl-slot")),
                );
                child.set_opacity(k);
                download_clicked = child
                    .add_enabled(
                        k > 0.5,
                        IconButton::new(Icon::Download)
                            .size(Size::Xs)
                            .style(Style::Tonal)
                            .tooltip(tip.clone())
                            .id_salt((id, "dl-btn")),
                    )
                    .clicked();
            }
            if self.menu_button {
                let size = Size::Xs.height();
                let c = pos2(rect.right() - 4.0 - size * 0.5, rect.top() + w + 28.0);
                let mut child = ui.new_child(
                    UiBuilder::new().max_rect(Rect::from_center_size(c, vec2(size, size))).id_salt((id, "more-slot")),
                );
                child.set_opacity(k);
                menu = Some(child.add_enabled(
                    k > 0.5,
                    IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip(nikgob_core::i18n::common().more).id_salt((id, "more-btn")),
                ));
            }
        }
        if self.menu_button {
            let open = menu.as_ref().is_some_and(|m| ctx.data(|d| d.get_temp::<bool>(m.id.with("md3e-menu-open"))).unwrap_or(false));
            if open != more_open {
                ctx.data_mut(|d| d.insert_temp(id.with("more-open"), open));
            }
        }
        let extra_clicked = download_clicked || menu.as_ref().is_some_and(Response::clicked);

        let label = match &self.subtitle {
            Some(sub) => format!("{}, {sub}", self.title),
            None => self.title.clone(),
        };
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
        MediaCardOutput {
            card_clicked: response.clicked() && !play_clicked && !extra_clicked,
            card_secondary_clicked: response.secondary_clicked(),
            play_clicked,
            download_clicked,
            menu,
            response,
        }
    }
}

/// Four covers in the quarters of `rect`, each keeping the outer corner of
/// `corner` it sits in.
fn paint_collage(ui: &Ui, rect: Rect, uris: &[String], corner: CornerRadius, placeholder: Icon, zoom: f32) {
    let half = rect.size() * 0.5;
    for (i, uri) in uris.iter().take(4).enumerate() {
        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
        let r = Rect::from_min_size(rect.min + vec2(col * half.x, row * half.y), half);
        let c = CornerRadius {
            nw: if i == 0 { corner.nw } else { 0 },
            ne: if i == 1 { corner.ne } else { 0 },
            sw: if i == 2 { corner.sw } else { 0 },
            se: if i == 3 { corner.se } else { 0 },
        };
        common::paint_image_zoomed(ui, r, Some(uri), c, placeholder, zoom);
    }
}

/// Height of a status chip on a cover: the large badge's.
const STATUS_H: f32 = badge::LARGE_HEIGHT;

/// The cover-corner chip of a [`CoverStatus`]: an icon or a ring, then the
/// text («в очереди», «3 из 12», «у тебя»).
fn status_chip(ui: &Ui, painter: &egui::Painter, left_center: egui::Pos2, status: CoverStatus, text: Option<&str>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (bg, fg) = match status {
        CoverStatus::Done => (s.primary_container, s.on_primary_container),
        _ => (s.secondary_container, s.on_secondary_container),
    };
    let text = match status {
        CoverStatus::Queued => Some(nikgob_core::i18n::pages().in_queue),
        CoverStatus::Done => Some(text.unwrap_or(nikgob_core::i18n::pages().you_have)),
        _ => text,
    };
    let galley = text.map(|t| painter.layout_no_wrap(t.to_owned(), TypeRole::LabelSmall.font_id(), fg));
    let icon = 12.0;
    let w = 2.0 + icon + galley.as_ref().map_or(2.0, |g| 3.0 + g.size().x + 5.0);
    let rect = Rect::from_min_size(pos2(left_center.x, left_center.y - STATUS_H * 0.5), vec2(w, STATUS_H));
    paint::container(painter, rect, Radii::all(shape::FULL), bg, Stroke::NONE);
    let ic = Rect::from_center_size(pos2(rect.left() + 2.0 + icon * 0.5, rect.center().y), vec2(icon, icon));
    match status {
        CoverStatus::Queued => Icon::HourglassEmpty.widget(false, fg, icon).paint_at(ui, ic),
        CoverStatus::Done => Icon::Check.widget(false, fg, icon).paint_at(ui, ic),
        CoverStatus::Progress(p) => progress_ring(ui, painter, ic.shrink(0.5), p, fg),
        CoverStatus::None => {}
    }
    if let Some(g) = galley {
        painter.galley(pos2(ic.right() + 3.0, rect.center().y - g.size().y * 0.5), g, fg);
    }
}

/// A small circular progress ring in `rect`: filled to `p`, or a turning
/// arc while the fraction is unknown.
fn progress_ring(ui: &Ui, painter: &egui::Painter, rect: Rect, p: Option<f32>, color: Color32) {
    use std::f32::consts::TAU;
    let (c, r) = (rect.center(), rect.width() * 0.5 - 1.0);
    let stroke = Stroke::new(1.75, color);
    painter.circle_stroke(c, r, Stroke::new(1.75, color.gamma_multiply(0.3)));
    let (start, sweep) = match p {
        Some(p) => (-TAU / 4.0, TAU * p.clamp(0.0, 1.0)),
        None => {
            let t = ui.ctx().input(|i| i.time) as f32;
            ui.ctx().request_repaint();
            (t * TAU * 0.8, TAU * 0.3)
        }
    };
    if sweep <= 0.0 {
        return;
    }
    let n = ((sweep / TAU) * 32.0).ceil().max(2.0) as usize;
    let points: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let a = start + sweep * i as f32 / n as f32;
            c + r * vec2(a.cos(), a.sin())
        })
        .collect();
    painter.add(egui::Shape::line(points, stroke));
}
