//! App bars: the small bar (64, title-large) and the large flexible bar
//! (120, or 152 with a subtitle) that collapses to 64 as the content scrolls:
//! the title shrinks from display-small to title-large and moves up beside
//! the leading icon, and the container goes from `surface` to
//! `surface-container`. The bar is driven by a scroll offset, not a spring,
//! so it follows the scroll exactly.

use egui::{Color32, Rect, Sense, Ui, UiBuilder, pos2, vec2};

use super::{
    anim,
    button::{IconButton, Size, Style},
    common,
};
use crate::{Icon, Theme, TypeRole, TypeStyle, lerp_color};

pub const SMALL_HEIGHT: f32 = 64.0;
pub const LARGE_HEIGHT: f32 = 120.0;
pub const LARGE_WITH_SUBTITLE_HEIGHT: f32 = 152.0;

/// Geometry of a flexible bar at a scroll offset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlexMetrics {
    /// Bar height: from the large height down to 64.
    pub height: f32,
    /// 0 = fully expanded, 1 = collapsed to the small bar.
    pub fraction: f32,
    /// Title font size and line height (display-small -> title-large).
    pub title_size: f32,
    pub title_line_height: f32,
    /// Title position relative to the bar's top-left corner.
    pub title_x: f32,
    pub title_y: f32,
}

/// The expanded height of a flexible bar.
pub fn large_height(has_subtitle: bool) -> f32 {
    if has_subtitle {
        LARGE_WITH_SUBTITLE_HEIGHT
    } else {
        LARGE_HEIGHT
    }
}

/// How collapsed the bar is at `scroll_offset` (0..=1). The bar finishes
/// collapsing after scrolling by the height it can lose.
pub fn collapse_fraction(scroll_offset: f32, has_subtitle: bool) -> f32 {
    ((scroll_offset.max(0.0)) / (large_height(has_subtitle) - SMALL_HEIGHT)).clamp(0.0, 1.0)
}

/// Interpolated geometry of a flexible bar.
pub fn flexible_metrics(scroll_offset: f32, has_subtitle: bool, has_leading: bool) -> FlexMetrics {
    let f = collapse_fraction(scroll_offset, has_subtitle);
    let lerp = |a: f32, b: f32| a + (b - a) * f;
    let big = TypeRole::DisplaySmall.style();
    let small = TypeRole::TitleLarge.style();
    let large_h = large_height(has_subtitle);
    // Expanded: the title sits in the bottom area under the icon row
    // (leaving room for the subtitle line when there is one).
    let title_bottom_area = if has_subtitle { 24.0 } else { 8.0 };
    let expanded_y =
        large_h - title_bottom_area - big.line_height - if has_subtitle { 20.0 } else { 0.0 };
    let collapsed_y =
        (SMALL_HEIGHT - small.line_height) * 0.5 - if has_subtitle { 8.0 } else { 0.0 };
    FlexMetrics {
        height: lerp(large_h, SMALL_HEIGHT),
        fraction: f,
        title_size: lerp(big.size, small.size),
        title_line_height: lerp(big.line_height, small.line_height),
        title_x: lerp(16.0, if has_leading { 64.0 } else { 16.0 }),
        title_y: lerp(expanded_y, collapsed_y),
    }
}

/// What an app bar reported.
pub struct AppBarOutput {
    /// The bar's rectangle (its height is the current one).
    pub rect: Rect,
    pub leading_clicked: bool,
    /// 0..1 collapse (always 0 or 1 for the small bar's scrolled flag).
    pub collapse: f32,
}

struct Content<'a> {
    title: &'a str,
    subtitle: Option<&'a str>,
    leading: Option<(Icon, &'a str)>,
}

/// Paints the bar into `rect`; `trailing` fills the right end of the top row.
#[allow(clippy::too_many_arguments)]
fn paint_bar(
    ui: &mut Ui,
    rect: Rect,
    id: egui::Id,
    fill: Color32,
    content: &Content<'_>,
    title_style: TypeStyle,
    subtitle_style: TypeStyle,
    title_pos: (f32, f32),
    trailing: impl FnOnce(&mut Ui),
) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    ui.painter().rect_filled(rect, 0.0, fill);

    // Leading icon.
    let mut leading_clicked = false;
    if let Some((icon, tip)) = content.leading {
        let slot =
            Rect::from_min_size(pos2(rect.left() + 8.0, rect.top() + 12.0), vec2(40.0, 40.0));
        let r = ui
            .scope_builder(UiBuilder::new().max_rect(slot), |ui| {
                ui.add(
                    IconButton::new(icon)
                        .size(Size::S)
                        .style(Style::Standard)
                        .colors(super::button::Colors {
                            container: Color32::TRANSPARENT,
                            content: s.on_surface,
                            border: Color32::TRANSPARENT,
                        })
                        .tooltip(tip)
                        .id_salt((id, "leading")),
                )
            })
            .inner;
        leading_clicked = r.clicked();
    }

    // Trailing widgets, measured last frame so the title can avoid them.
    let last_w: f32 = ui.ctx().data(|d| d.get_temp(id.with("tw"))).unwrap_or(0.0);
    let area = Rect::from_min_max(
        pos2(rect.left() + 64.0, rect.top()),
        pos2(rect.right() - 8.0, rect.top() + SMALL_HEIGHT),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(area)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 0.0;
    child.visuals_mut().override_text_color = Some(s.on_surface_variant);
    trailing(&mut child);
    let w = child.min_rect().width();
    if (w - last_w).abs() > 0.5 {
        ui.ctx().data_mut(|d| d.insert_temp(id.with("tw"), w));
        ui.ctx().request_repaint();
    }

    // Title (+ subtitle).
    let max_w = (rect.width() - title_pos.0 - 16.0 - last_w).max(40.0);
    let title = common::line(
        ui.painter(),
        content.title,
        title_style,
        s.on_surface,
        max_w,
    );
    let ty = rect.top() + title_pos.1;
    let th = title.size().y;
    ui.painter()
        .galley(pos2(rect.left() + title_pos.0, ty), title, s.on_surface);
    if let Some(sub) = content.subtitle {
        let g = common::line(
            ui.painter(),
            sub,
            subtitle_style,
            s.on_surface_variant,
            max_w,
        );
        ui.painter().galley(
            pos2(rect.left() + title_pos.0, ty + th),
            g,
            s.on_surface_variant,
        );
    }
    leading_clicked
}

/// The small app bar (64): `surface`, or `surface-container` once content
/// scrolls under it.
#[must_use = "call .show(ui, |ui| trailing)"]
pub struct SmallAppBar<'a> {
    title: &'a str,
    subtitle: Option<&'a str>,
    leading: Option<(Icon, &'a str)>,
    scrolled: bool,
    id_salt: egui::Id,
}

impl<'a> SmallAppBar<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            subtitle: None,
            leading: None,
            scrolled: false,
            id_salt: egui::Id::new("small-app-bar"),
        }
    }

    pub fn subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    /// A leading icon button with its tooltip (for example a back arrow).
    pub fn leading(mut self, icon: Icon, tooltip: &'a str) -> Self {
        self.leading = Some((icon, tooltip));
        self
    }

    /// Content is scrolled under the bar: the container turns `surface-container`.
    pub fn scrolled(mut self, scrolled: bool) -> Self {
        self.scrolled = scrolled;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = egui::Id::new(salt);
        self
    }

    pub fn show(self, ui: &mut Ui, trailing: impl FnOnce(&mut Ui)) -> AppBarOutput {
        let theme = Theme::get(ui.ctx());
        let s = theme.scheme;
        let id = ui.id().with(self.id_salt);
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), SMALL_HEIGHT), Sense::hover());
        let k = anim::fraction(
            ui.ctx(),
            id.with("scrolled"),
            f32::from(u8::from(self.scrolled)),
            theme.motion.default_effects,
        )
        .clamp(0.0, 1.0);
        let fill = lerp_color(s.surface, s.surface_container, k);
        let x = if self.leading.is_some() { 64.0 } else { 16.0 };
        let y = if self.subtitle.is_some() {
            10.0
        } else {
            (SMALL_HEIGHT - 28.0) * 0.5
        };
        let content = Content {
            title: self.title,
            subtitle: self.subtitle,
            leading: self.leading,
        };
        let leading_clicked = if ui.is_rect_visible(rect) {
            paint_bar(
                ui,
                rect,
                id,
                fill,
                &content,
                TypeRole::TitleLarge.style(),
                TypeRole::LabelMedium.style(),
                (x, y),
                trailing,
            )
        } else {
            false
        };
        AppBarOutput {
            rect,
            leading_clicked,
            collapse: k,
        }
    }
}

/// The large flexible app bar driven by the content's scroll offset.
#[must_use = "call .show(ui, scroll_offset, |ui| trailing)"]
pub struct FlexibleAppBar<'a> {
    title: &'a str,
    subtitle: Option<&'a str>,
    leading: Option<(Icon, &'a str)>,
    id_salt: egui::Id,
}

impl<'a> FlexibleAppBar<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            subtitle: None,
            leading: None,
            id_salt: egui::Id::new("flexible-app-bar"),
        }
    }

    pub fn subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }

    pub fn leading(mut self, icon: Icon, tooltip: &'a str) -> Self {
        self.leading = Some((icon, tooltip));
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = egui::Id::new(salt);
        self
    }

    /// `scroll_offset`: how far the content below has scrolled (>= 0). The
    /// allocated height shrinks from 120 (152 with a subtitle) to 64.
    pub fn show(
        self,
        ui: &mut Ui,
        scroll_offset: f32,
        trailing: impl FnOnce(&mut Ui),
    ) -> AppBarOutput {
        let s = Theme::get(ui.ctx()).scheme;
        let id = ui.id().with(self.id_salt);
        let m = flexible_metrics(
            scroll_offset,
            self.subtitle.is_some(),
            self.leading.is_some(),
        );
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), m.height), Sense::hover());
        let fill = lerp_color(s.surface, s.surface_container, m.fraction);
        let mut title_style = TypeRole::DisplaySmall.style();
        title_style.size = m.title_size;
        title_style.line_height = m.title_line_height;
        let sub_style = if m.fraction < 0.5 {
            TypeRole::TitleMedium.style()
        } else {
            TypeRole::LabelMedium.style()
        };
        let content = Content {
            title: self.title,
            subtitle: self.subtitle,
            leading: self.leading,
        };
        let leading_clicked = if ui.is_rect_visible(rect) {
            paint_bar(
                ui,
                rect,
                id,
                fill,
                &content,
                title_style,
                sub_style,
                (m.title_x, m.title_y),
                trailing,
            )
        } else {
            false
        };
        AppBarOutput {
            rect,
            leading_clicked,
            collapse: m.fraction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flexible_bar_interpolates_between_large_and_small() {
        let top = flexible_metrics(0.0, false, true);
        assert_eq!(top.height, 120.0);
        assert_eq!(top.fraction, 0.0);
        assert_eq!(top.title_size, 36.0);
        assert_eq!(top.title_x, 16.0);

        let mid = flexible_metrics(28.0, false, true);
        assert_eq!(mid.fraction, 0.5);
        assert_eq!(mid.height, 92.0);
        assert_eq!(mid.title_size, 29.0);
        assert!(mid.title_y < top.title_y);

        let end = flexible_metrics(56.0, false, true);
        assert_eq!(end.height, 64.0);
        assert_eq!(end.title_size, 22.0);
        assert_eq!(end.title_line_height, 28.0);
        assert_eq!(end.title_x, 64.0);
        // Scrolling further changes nothing.
        assert_eq!(flexible_metrics(500.0, false, true), end);
        // Overscroll (negative) stays expanded.
        assert_eq!(flexible_metrics(-30.0, false, true), top);
    }

    #[test]
    fn subtitle_makes_the_bar_taller_and_collapse_longer() {
        assert_eq!(flexible_metrics(0.0, true, false).height, 152.0);
        assert_eq!(collapse_fraction(44.0, true), 0.5);
        assert_eq!(flexible_metrics(88.0, true, false).height, 64.0);
    }
}
