//! List items (1-line 56, 2-line 72, 3-line 88) with an icon / avatar /
//! image / number on the leading side, trailing text or widgets, a selected
//! state (`secondary-container`, radius 16), an optional drag handle, and the
//! expressive segmented list (2 dp gaps, 4 dp item corners in a 16 dp group).

use egui::{
    Color32, CornerRadius, Id, Rect, Response, Sense, Ui, UiBuilder, WidgetInfo, WidgetType, pos2,
    text::{LayoutJob, TextWrapping},
    vec2,
};

use super::{anim, common, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{shape, state},
};

pub const ONE_LINE: f32 = 56.0;
pub const TWO_LINE: f32 = 72.0;
pub const THREE_LINE: f32 = 88.0;
const SIDE: f32 = 16.0;
const GAP: f32 = 12.0;
const HANDLE_SLOT: f32 = 40.0;

/// What sits at the start of the row.
#[derive(Clone, Debug, Default)]
pub enum Leading {
    #[default]
    None,
    /// A 24 dp icon.
    Icon(Icon),
    /// A 40 dp circular image (URI), or a tonal circle with the icon.
    Avatar(Option<String>, Icon),
    /// A 56x56 image with radius 8.
    Image(Option<String>, Icon),
    /// A fixed-width column with text (track number).
    Number(String),
}

impl Leading {
    fn width(&self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Icon(_) => 24.0,
            Self::Avatar(..) => 40.0,
            Self::Image(..) => 56.0,
            Self::Number(_) => 28.0,
        }
    }
}

/// Where an item sits in a segmented list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Position {
    Only,
    First,
    Middle,
    Last,
}

impl Position {
    pub fn of(index: usize, count: usize) -> Self {
        match (index, count) {
            (_, 0 | 1) => Self::Only,
            (0, _) => Self::First,
            (i, n) if i + 1 == n => Self::Last,
            _ => Self::Middle,
        }
    }

    /// Resting corner radii: 16 on the outer corners of the group, 4 inside.
    pub fn radii(self) -> Radii {
        let (outer, inner) = (shape::LARGE, shape::EXTRA_SMALL);
        match self {
            Self::Only => Radii::all(outer),
            Self::First => Radii([outer, outer, inner, inner]),
            Self::Middle => Radii::all(inner),
            Self::Last => Radii([inner, inner, outer, outer]),
        }
    }
}

/// What a list item reported.
pub struct ListItemOutput {
    /// The row (`clicked()`, `secondary_clicked()`, `hovered()`).
    pub response: Response,
    /// The drag handle's response (drag it to reorder), if the item has one.
    pub handle: Option<Response>,
    pub rect: Rect,
}

type TrailingFn<'a> = Box<dyn FnOnce(&mut Ui) + 'a>;

/// One row.
#[must_use = "call .show(ui)"]
pub struct ListItem<'a> {
    headline: String,
    supporting: Option<String>,
    supporting_lines: usize,
    leading: Leading,
    trailing_text: Option<String>,
    trailing: Option<TrailingFn<'a>>,
    selected: bool,
    drag_handle: bool,
    dragging: bool,
    enabled: bool,
    muted: bool,
    fill: Option<Color32>,
    radii: Option<Radii>,
    min_height: Option<f32>,
    id_salt: Option<Id>,
}

impl<'a> ListItem<'a> {
    pub fn new(headline: impl Into<String>) -> Self {
        Self {
            headline: headline.into(),
            supporting: None,
            supporting_lines: 1,
            leading: Leading::None,
            trailing_text: None,
            trailing: None,
            selected: false,
            drag_handle: false,
            dragging: false,
            enabled: true,
            muted: false,
            fill: None,
            radii: None,
            min_height: None,
            id_salt: None,
        }
    }

    /// A second line (72 dp row).
    pub fn supporting(mut self, text: impl Into<String>) -> Self {
        self.supporting = Some(text.into());
        self
    }

    /// Lets the supporting text take two lines (88 dp row).
    pub fn supporting_lines(mut self, lines: usize) -> Self {
        self.supporting_lines = lines.clamp(1, 2);
        self
    }

    pub fn leading(mut self, leading: Leading) -> Self {
        self.leading = leading;
        self
    }

    pub fn leading_icon(self, icon: Icon) -> Self {
        self.leading(Leading::Icon(icon))
    }

    pub fn leading_number(self, n: impl ToString) -> Self {
        self.leading(Leading::Number(n.to_string()))
    }

    /// Trailing supporting text (label-small), for example a duration.
    pub fn trailing_text(mut self, text: impl Into<String>) -> Self {
        self.trailing_text = Some(text.into());
        self
    }

    /// Trailing widgets, laid out right to left in the row's end.
    pub fn trailing(mut self, add: impl FnOnce(&mut Ui) + 'a) -> Self {
        self.trailing = Some(Box::new(add));
        self
    }

    /// `secondary-container` with radius 16.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Shows a drag handle at the end; its response is in the output.
    pub fn drag_handle(mut self, handle: bool) -> Self {
        self.drag_handle = handle;
        self
    }

    /// Marks the row as being dragged (16 % state layer).
    pub fn dragging(mut self, dragging: bool) -> Self {
        self.dragging = dragging;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A resting container colour (segmented lists use one).
    /// Headline in `on-surface-variant` (e.g. a track with no local file).
    pub fn muted(mut self, muted: bool) -> Self {
        self.muted = muted;
        self
    }

    pub fn fill(mut self, color: Color32) -> Self {
        self.fill = Some(color);
        self
    }

    /// Resting corner radii (selected rows always animate to 16).
    pub fn radii(mut self, radii: Radii) -> Self {
        self.radii = Some(radii);
        self
    }

    pub fn position(self, pos: Position) -> Self {
        self.radii(pos.radii())
    }

    pub fn min_height(mut self, h: f32) -> Self {
        self.min_height = Some(h);
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }

    fn height(&self) -> f32 {
        let base = match (&self.supporting, self.supporting_lines) {
            (None, _) => ONE_LINE,
            (Some(_), 1) => TWO_LINE,
            (Some(_), _) => THREE_LINE,
        };
        let base = if matches!(self.leading, Leading::Image(..)) {
            base.max(TWO_LINE)
        } else {
            base
        };
        self.min_height.map_or(base, |m| base.max(m))
    }

    pub fn show(self, ui: &mut Ui) -> ListItemOutput {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |salt| ui.id().with(salt));
        let height = self.height();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
        let sense = if self.enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let response = ui.interact(rect, id, sense);

        // Drag handle (registered right after the row so it wins).
        let handle_rect = Rect::from_min_size(
            pos2(
                rect.right() - SIDE + 4.0 - HANDLE_SLOT,
                rect.center().y - 24.0,
            ),
            vec2(HANDLE_SLOT, 48.0),
        );
        let handle = (self.drag_handle && self.enabled)
            .then(|| ui.interact(handle_rect, id.with("handle"), Sense::drag()));
        let handle_active = handle.as_ref().is_some_and(|h| h.dragged());

        // Trailing widgets: measured last frame, laid out at the end.
        let handle_w = if self.drag_handle { HANDLE_SLOT } else { 0.0 };
        let trailing_right =
            rect.right() - SIDE - handle_w + if self.drag_handle { 4.0 } else { 0.0 };
        let last_trailing_w: f32 = ctx.data(|d| d.get_temp(id.with("tw"))).unwrap_or(0.0);
        let trailing_w = if last_trailing_w > 0.0 {
            last_trailing_w + GAP
        } else {
            0.0
        };

        let selected = self.selected;
        let content = if !self.enabled {
            common::fade(s.on_surface, state::DISABLED_CONTENT)
        } else if selected {
            s.on_secondary_container
        } else if self.muted {
            s.on_surface_variant
        } else {
            s.on_surface
        };
        let variant = if !self.enabled {
            common::fade(s.on_surface, state::DISABLED_CONTENT)
        } else if selected {
            s.on_secondary_container
        } else {
            s.on_surface_variant
        };

        if ui.is_rect_visible(rect) {
            // Container.
            let rest_fill = self
                .fill
                .unwrap_or_else(|| common::clear(s.secondary_container));
            let fill = anim::color(
                &ctx,
                id.with("bg"),
                if selected {
                    s.secondary_container
                } else {
                    rest_fill
                },
                motion.fast_effects,
            );
            let rest = self.radii.unwrap_or(Radii::all(shape::LARGE));
            let target = if selected {
                Radii::all(shape::LARGE)
            } else {
                rest
            };
            let radii = Radii(
                [0, 1, 2, 3]
                    .map(|i| anim::f32(&ctx, id.with(("r", i)), target.0[i], motion.fast_spatial)),
            )
            .clamped(rect);
            paint::container(ui.painter(), rect, radii, fill, egui::Stroke::NONE);
            let layer = anim::fraction(
                &ctx,
                id.with("layer"),
                common::state_target(&response, self.enabled, self.dragging || handle_active),
                motion.fast_effects,
            );
            let layer_color = if selected {
                s.on_secondary_container
            } else {
                s.on_surface
            };
            paint::state_layer(ui.painter(), rect, radii, layer_color, layer);

            // Leading.
            let lines3 = height >= THREE_LINE;
            let lead_cy = if lines3 {
                rect.top() + 16.0 + 12.0
            } else {
                rect.center().y
            };
            let mut x = rect.left() + SIDE;
            let lw = self.leading.width();
            match &self.leading {
                Leading::None => {}
                Leading::Icon(icon) => {
                    icon.widget(selected, variant, 24.0).paint_at(
                        ui,
                        Rect::from_center_size(pos2(x + 12.0, lead_cy), vec2(24.0, 24.0)),
                    );
                }
                Leading::Avatar(uri, icon) => {
                    common::paint_image(
                        ui,
                        Rect::from_center_size(pos2(x + 20.0, lead_cy), vec2(40.0, 40.0)),
                        uri.as_deref(),
                        CornerRadius::same(20),
                        *icon,
                    );
                }
                Leading::Image(uri, icon) => {
                    common::paint_image(
                        ui,
                        Rect::from_center_size(pos2(x + 28.0, rect.center().y), vec2(56.0, 56.0)),
                        uri.as_deref(),
                        CornerRadius::same(shape::SMALL as u8),
                        *icon,
                    );
                }
                Leading::Number(n) => {
                    let g = common::line(ui.painter(), n, TypeRole::BodyLarge.style(), variant, lw);
                    ui.painter().galley(
                        pos2(
                            x + (lw - g.size().x).max(0.0) * 0.5,
                            rect.center().y - g.size().y * 0.5,
                        ),
                        g,
                        variant,
                    );
                }
            }
            if lw > 0.0 {
                x += lw + GAP;
            }

            // Trailing text.
            let mut right = trailing_right - trailing_w;
            if let Some(t) = &self.trailing_text {
                let g = common::plain(ui.painter(), t, TypeRole::LabelSmall.style(), variant);
                let tx = right - g.size().x;
                ui.painter()
                    .galley(pos2(tx, rect.center().y - g.size().y * 0.5), g, variant);
                right = tx - GAP;
            }
            if self.drag_handle {
                Icon::DragIndicator.widget(false, variant, 24.0).paint_at(
                    ui,
                    Rect::from_center_size(handle_rect.center(), vec2(24.0, 24.0)),
                );
            }

            // Text.
            let text_w = (right - x).max(8.0);
            let n_support = if self.supporting.is_some() {
                self.supporting_lines
            } else {
                0
            };
            let block_h = 24.0 + 20.0 * n_support as f32;
            let mut y = rect.center().y - block_h * 0.5;
            let head = common::line(
                ui.painter(),
                &self.headline,
                TypeRole::BodyLarge.style(),
                content,
                text_w,
            );
            ui.painter().galley(pos2(x, y), head, content);
            y += 24.0;
            if let Some(sup) = &self.supporting {
                let mut job = LayoutJob::single_section(
                    sup.clone(),
                    TypeRole::BodyMedium.style().text_format(variant),
                );
                job.wrap = TextWrapping {
                    max_width: text_w,
                    max_rows: n_support.max(1),
                    break_anywhere: n_support <= 1,
                    ..TextWrapping::default()
                };
                let g = ui.painter().layout_job(job);
                ui.painter().galley(pos2(x, y), g, variant);
            }

            if self.enabled && common::focused(&response) {
                paint::focus_ring(ui.painter(), rect, radii, s.secondary);
            }
        }

        // Trailing widgets go on top of the container.
        if let Some(add) = self.trailing {
            let area = Rect::from_min_max(
                pos2(rect.left() + 64.0, rect.top()),
                pos2(trailing_right, rect.bottom()),
            );
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(area)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            child.spacing_mut().item_spacing.x = 4.0;
            add(&mut child);
            let w = child.min_rect().width();
            if (w - last_trailing_w).abs() > 0.5 {
                ctx.data_mut(|d| d.insert_temp(id.with("tw"), w));
                ctx.request_repaint();
            }
        }

        let label = self.headline.clone();
        let (enabled, sel) = (self.enabled, selected);
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, sel, &label));
        ListItemOutput {
            response,
            handle,
            rect,
        }
    }
}

/// Draws `count` items as an expressive segmented list: 2 dp gaps, 4 dp inner
/// corners, 16 dp outer corners. `item(i)` builds the i-th item; the rows are
/// returned in order.
pub fn segmented_list<'a>(
    ui: &mut Ui,
    count: usize,
    mut item: impl FnMut(usize) -> ListItem<'a>,
) -> Vec<ListItemOutput> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut out = Vec::with_capacity(count);
    let old = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;
    for i in 0..count {
        let it = item(i)
            .position(Position::of(i, count))
            .fill(s.surface_container_low);
        out.push(it.show(ui));
    }
    ui.spacing_mut().item_spacing.y = old;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_by_index() {
        assert_eq!(Position::of(0, 1), Position::Only);
        assert_eq!(Position::of(0, 3), Position::First);
        assert_eq!(Position::of(1, 3), Position::Middle);
        assert_eq!(Position::of(2, 3), Position::Last);
        assert_eq!(Position::First.radii().0, [16.0, 16.0, 4.0, 4.0]);
        assert_eq!(Position::Middle.radii(), Radii::all(4.0));
    }

    #[test]
    fn heights_follow_line_count() {
        assert_eq!(ListItem::new("a").height(), 56.0);
        assert_eq!(ListItem::new("a").supporting("b").height(), 72.0);
        assert_eq!(
            ListItem::new("a")
                .supporting("b")
                .supporting_lines(2)
                .height(),
            88.0
        );
        assert_eq!(
            ListItem::new("a")
                .leading(Leading::Image(None, Icon::Album))
                .height(),
            72.0
        );
    }
}
