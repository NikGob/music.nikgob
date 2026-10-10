//! Badges: the small dot (6 dp) and the large badge (16 dp, label-small).
//! Both are drawn with a painter at a position the caller computes, usually
//! with [`large_anchor`] / [`dot_anchor`] on the icon they decorate.

use egui::{Color32, Painter, Pos2, Rect, Stroke, pos2, vec2};

use super::{paint, paint::Radii};
use crate::{Theme, TypeRole, tokens::shape};

/// Diameter of the small badge.
pub const DOT_SIZE: f32 = 6.0;
/// Height of the large badge.
pub const LARGE_HEIGHT: f32 = 16.0;

/// Left-centre of a large badge that overlaps the top-right corner of `icon`.
pub fn large_anchor(icon: Rect) -> Pos2 {
    pos2(icon.right() - 6.0, icon.top())
}

/// Centre of a small badge on the top-right corner of `icon`.
pub fn dot_anchor(icon: Rect) -> Pos2 {
    pos2(icon.right() - 2.0, icon.top() + 2.0)
}

/// The small badge (error colour) centred at `center`.
pub fn dot(painter: &Painter, center: Pos2) -> Rect {
    let s = Theme::get(painter.ctx()).scheme;
    let rect = Rect::from_center_size(center, vec2(DOT_SIZE, DOT_SIZE));
    paint::container(
        painter,
        rect,
        Radii::all(shape::FULL),
        s.error,
        Stroke::NONE,
    );
    rect
}

/// The large badge with a count (`999+` above 999), error colours.
pub fn count(painter: &Painter, left_center: Pos2, n: u32) -> Rect {
    let s = Theme::get(painter.ctx()).scheme;
    let text = if n > 999 {
        "999+".to_owned()
    } else {
        n.to_string()
    };
    text_badge(painter, left_center, &text, s.error, s.on_error)
}

/// The large badge shape with arbitrary text and colours. Returns its rect.
pub fn text_badge(
    painter: &Painter,
    left_center: Pos2,
    text: &str,
    bg: Color32,
    fg: Color32,
) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), TypeRole::LabelSmall.font_id(), fg);
    let w = (galley.size().x + 8.0).max(LARGE_HEIGHT);
    let rect = Rect::from_min_size(
        pos2(left_center.x, left_center.y - LARGE_HEIGHT * 0.5),
        vec2(w, LARGE_HEIGHT),
    );
    paint::container(painter, rect, Radii::all(shape::FULL), bg, Stroke::NONE);
    painter.galley(
        pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        fg,
    );
    rect
}
