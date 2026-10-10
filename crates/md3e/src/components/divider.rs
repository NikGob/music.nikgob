//! Divider: 1 dp `outline-variant` line, full width or inset.

use egui::{Rect, Response, Sense, Stroke, Ui, Widget, pos2, vec2};

use crate::Theme;

/// A horizontal (default) or vertical divider.
#[must_use = "add it with ui.add(Divider::new())"]
pub struct Divider {
    vertical: bool,
    inset_start: f32,
    inset_end: f32,
    margin: f32,
}

impl Divider {
    /// A horizontal divider with 8 dp of space above and below.
    pub fn new() -> Self {
        Self {
            vertical: false,
            inset_start: 0.0,
            inset_end: 0.0,
            margin: 8.0,
        }
    }

    /// A vertical divider (as tall as the available height).
    pub fn vertical() -> Self {
        Self {
            vertical: true,
            ..Self::new()
        }
    }

    /// Inset both ends (16 for an inset divider in a list).
    pub fn inset(mut self, inset: f32) -> Self {
        self.inset_start = inset;
        self.inset_end = inset;
        self
    }

    /// Inset only the leading end.
    pub fn inset_start(mut self, inset: f32) -> Self {
        self.inset_start = inset;
        self
    }

    /// Space on the two sides of the line.
    pub fn margin(mut self, margin: f32) -> Self {
        self.margin = margin;
        self
    }
}

impl Default for Divider {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Divider {
    fn ui(self, ui: &mut Ui) -> Response {
        let color = Theme::get(ui.ctx()).scheme.outline_variant;
        let stroke = Stroke::new(1.0, color);
        if self.vertical {
            let h = ui.available_height();
            let (rect, r) =
                ui.allocate_exact_size(vec2(1.0 + 2.0 * self.margin, h), Sense::hover());
            let x = rect.center().x;
            ui.painter().line_segment(
                [
                    pos2(x, rect.top() + self.inset_start),
                    pos2(x, rect.bottom() - self.inset_end),
                ],
                stroke,
            );
            r
        } else {
            let w = ui.available_width();
            let (rect, r) =
                ui.allocate_exact_size(vec2(w, 1.0 + 2.0 * self.margin), Sense::hover());
            let line = Rect::from_min_max(
                pos2(rect.left() + self.inset_start, rect.center().y),
                pos2(rect.right() - self.inset_end, rect.center().y),
            );
            ui.painter()
                .line_segment([line.left_center(), line.right_center()], stroke);
            r
        }
    }
}
