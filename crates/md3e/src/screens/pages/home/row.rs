//! A row of Главная that goes on sideways: more cards than fit stand to
//! the right of the grid, and the row scrolls to them.
//!
//! * **The wheel**: a sideways wheel (two fingers on a touchpad) and
//!   Shift + the wheel scroll the row under the pointer; the plain wheel
//!   always scrolls the page, whatever is under the pointer.
//! * **A drag**: the pointer held on the row and moved [`DRAG_FROM`]
//!   sideways takes the row with it (the card under it is not clicked: a
//!   click moves less); let go, the row comes to rest on a card's edge.
//! * **The arrows**: a small round button at each end the row can go to,
//!   grown in over the cards while the pointer is on the row; a click
//!   scrolls by the cards that fit. They take no room: nothing moves when
//!   they come and go.
//!
//! The row's state ([`Scroll`]) is plain numbers, kept in egui's memory by
//! the row's name; the cards are painted clipped to the grid's width, and
//! only those in view.

use egui::{Color32, Id, Rect, Ui, UiBuilder, Vec2, pos2, vec2};

use super::{GUTTER, Grid};
use crate::{
    Icon, Theme,
    components::{IconButton, Size, Style, anim, button::Colors},
    screens::pages::cards,
};

/// The pointer moved this far sideways with the button down: a drag.
pub const DRAG_FROM: f32 = 8.0;
/// The arrows stand this far inside the row's ends.
const ARROW_INSET: f32 = 8.0;

/// How far a row is scrolled, and the drag going on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    /// Where the row is going: how far its first card is out at the left.
    pub offset: f32,
    /// The pointer went down on the row at this x with the row at this
    /// offset.
    press: Option<(f32, f32)>,
    dragging: bool,
}

impl Scroll {
    /// How far a row of `n` cards `pitch` apart (the gap `gap` between
    /// two) scrolls in a view `view` wide: 0 when they all fit.
    pub fn max(n: usize, pitch: f32, gap: f32, view: f32) -> f32 {
        (n as f32 * pitch - gap - view).max(0.0)
    }

    /// Back within `0..=max` (the window got wider, a card left).
    pub fn clamp(&mut self, max: f32) {
        self.offset = self.offset.clamp(0.0, max.max(0.0));
    }

    /// The wheel moved the content by `delta` (as egui's scroll delta:
    /// positive brings what is at the left).
    pub fn wheel(&mut self, delta: f32, max: f32) {
        self.offset = (self.offset - delta).clamp(0.0, max.max(0.0));
    }

    /// An arrow: `per` cards on (`forward`) or back, to a card's edge.
    pub fn page(&mut self, forward: bool, per: usize, pitch: f32, max: f32) {
        if pitch <= 0.0 {
            return;
        }
        let at = (self.offset / pitch).round() as i64;
        let to = at + if forward { per.max(1) as i64 } else { -(per.max(1) as i64) };
        self.offset = (to.max(0) as f32 * pitch).clamp(0.0, max.max(0.0));
    }

    pub fn can_back(&self) -> bool {
        self.offset > 0.5
    }

    pub fn can_forward(&self, max: f32) -> bool {
        self.offset < max - 0.5
    }

    /// The button went down on the row at `x`.
    pub fn press(&mut self, x: f32) {
        self.press = Some((x, self.offset));
        self.dragging = false;
    }

    /// The pointer is at `x` with the button down: true once it drags the
    /// row (it moved [`DRAG_FROM`] from where it went down).
    pub fn drag(&mut self, x: f32, max: f32) -> bool {
        let Some((x0, from)) = self.press else { return false };
        if !self.dragging && (x - x0).abs() >= DRAG_FROM {
            self.dragging = true;
        }
        if self.dragging {
            self.offset = (from - (x - x0)).clamp(0.0, max.max(0.0));
        }
        self.dragging
    }

    /// The button went up: a drag ends on the nearest card's edge. True
    /// when it was a drag.
    pub fn release(&mut self, pitch: f32, max: f32) -> bool {
        let was = self.dragging;
        if was && pitch > 0.0 {
            self.offset = ((self.offset / pitch).round() * pitch).clamp(0.0, max.max(0.0));
        }
        self.press = None;
        self.dragging = false;
        was
    }

    pub fn dragging(&self) -> bool {
        self.dragging
    }
}

/// What a row is laid out by.
#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    /// Its name (its state's key).
    pub name: &'a str,
    pub top: f32,
    /// A card's height, and how far down a card the arrows stand (the
    /// middle of its cover).
    pub card_h: f32,
    pub arrows_at: f32,
    /// Cards that fit (each `12 / per` columns).
    pub per: usize,
    /// Cards in all.
    pub n: usize,
    /// Takes the pointer (the real page; the settings' copy stands still
    /// at its start).
    pub live: bool,
}

/// Lays `row` out on `grid` and calls `card` with the place of each card
/// in view (see the module docs).
pub fn show(ui: &mut Ui, grid: Grid, row: Row<'_>, mut card: impl FnMut(&mut Ui, usize, Rect)) {
    let per = row.per.max(1);
    let card_w = grid.span(12 / per);
    let pitch = card_w + GUTTER;
    let view = Rect::from_min_size(pos2(grid.left, row.top), vec2(grid.width, row.card_h));
    let max = Scroll::max(row.n, pitch, GUTTER, grid.width);
    let ctx = ui.ctx().clone();
    let id = Id::new(("home-row", row.name));
    let mut state: Scroll = if row.live { ctx.data(|d| d.get_temp(id)).unwrap_or_default() } else { Scroll::default() };
    state.clamp(max);
    // What moved the row by hand this frame is followed at once; an arrow
    // and the end of a drag ride a spring.
    let mut by_hand = false;
    let hovered = row.live && ui.rect_contains_pointer(view);
    if row.live && max > 0.0 {
        if hovered {
            let delta = ctx.input_mut(|i| {
                let d = i.smooth_scroll_delta;
                if d.x != 0.0 {
                    i.smooth_scroll_delta.x = 0.0;
                    d.x
                } else if i.modifiers.shift && d.y != 0.0 {
                    i.smooth_scroll_delta.y = 0.0;
                    d.y
                } else {
                    0.0
                }
            });
            if delta != 0.0 {
                state.wheel(delta, max);
                by_hand = true;
            }
        }
        let (pressed, down, at) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos()));
        if pressed && hovered && let Some(p) = at {
            state.press(p.x);
        }
        if down {
            if let Some(p) = at
                && state.drag(p.x, max)
            {
                by_hand = true;
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        } else {
            state.release(pitch, max);
        }
    }
    let spring_id = id.with("x");
    let shown = if by_hand || !row.live {
        anim::snap_f32(&ctx, spring_id, state.offset);
        state.offset
    } else {
        anim::f32(&ctx, spring_id, state.offset, Theme::get(&ctx).motion.default_spatial.without_bounce()).clamp(0.0, max)
    };

    // The cards, cut at the grid's edges (a little room above and below
    // for what a card paints around itself).
    let mut child = ui.new_child(UiBuilder::new().max_rect(view).id_salt(("home-row-ui", row.name)));
    child.set_clip_rect(view.expand2(vec2(0.0, 8.0)).intersect(ui.clip_rect()));
    for i in 0..row.n {
        let cell = Rect::from_min_size(pos2(grid.left + i as f32 * pitch - shown, row.top), vec2(card_w, row.card_h));
        if cell.right() <= view.left() || cell.left() >= view.right() {
            continue;
        }
        card(&mut child, i, cell);
    }

    if row.live && max > 0.0 {
        let s = Theme::get(&ctx).scheme;
        let d = Size::S.height();
        let colors = Colors { container: s.surface_container_highest, content: s.on_surface, border: Color32::TRANSPARENT };
        let y = row.top + row.arrows_at - d / 2.0;
        let arrow = |icon: Icon, tip: &'static str, salt: &'static str| {
            IconButton::new(icon).size(Size::S).style(Style::Filled).colors(colors).tooltip(tip).id_salt((salt, id))
        };
        let on = hovered && !state.dragging();
        let back = Rect::from_min_size(pos2(view.left() + ARROW_INSET, y), Vec2::splat(d));
        let clicked = cards::grow_button(ui, id.with("back"), on && state.can_back(), back, arrow(Icon::ChevronLeft, "Назад", "home-row-back"));
        if clicked.is_some_and(|r| r.clicked()) {
            state.page(false, per, pitch, max);
        }
        let forward = Rect::from_min_size(pos2(view.right() - ARROW_INSET - d, y), Vec2::splat(d));
        let clicked = cards::grow_button(ui, id.with("forward"), on && state.can_forward(max), forward, arrow(Icon::ChevronRight, "Дальше", "home-row-forward"));
        if clicked.is_some_and(|r| r.clicked()) {
            state.page(true, per, pitch, max);
        }
    }
    if row.live {
        ctx.data_mut(|d| d.insert_temp(id, state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Eight cards 216 apart (a gap of 16), six of them in view.
    const PITCH: f32 = 216.0;
    const VIEW: f32 = 6.0 * PITCH - 16.0;

    #[test]
    fn a_row_scrolls_between_its_ends() {
        // All in view: nowhere to go.
        assert_eq!(Scroll::max(6, PITCH, 16.0, VIEW), 0.0);
        assert_eq!(Scroll::max(3, PITCH, 16.0, VIEW), 0.0);
        let max = Scroll::max(8, PITCH, 16.0, VIEW);
        assert_eq!(max, 2.0 * PITCH, "the last card ends on the grid's edge");
        let mut s = Scroll::default();
        assert!(!s.can_back() && s.can_forward(max));
        assert!(!Scroll::default().can_forward(0.0), "no arrows on a row that fits");
        // The wheel: as far as the row goes, no farther.
        s.wheel(-100.0, max);
        assert_eq!(s.offset, 100.0);
        assert!(s.can_back() && s.can_forward(max));
        s.wheel(-10_000.0, max);
        assert_eq!(s.offset, max);
        assert!(s.can_back() && !s.can_forward(max));
        s.wheel(10_000.0, max);
        assert_eq!(s.offset, 0.0);
        // A wider window: the row is back within its ends.
        s.offset = max;
        s.clamp(PITCH);
        assert_eq!(s.offset, PITCH);
        s.clamp(0.0);
        assert_eq!(s.offset, 0.0);
    }

    #[test]
    fn an_arrow_pages_to_a_cards_edge() {
        let max = Scroll::max(20, PITCH, 16.0, VIEW);
        assert_eq!(max, 14.0 * PITCH);
        let mut s = Scroll::default();
        s.page(true, 6, PITCH, max);
        assert_eq!(s.offset, 6.0 * PITCH);
        s.page(true, 6, PITCH, max);
        assert_eq!(s.offset, 12.0 * PITCH);
        s.page(true, 6, PITCH, max);
        assert_eq!(s.offset, max, "the last page ends where the row does");
        assert!(!s.can_forward(max));
        s.page(false, 6, PITCH, max);
        assert_eq!(s.offset, 8.0 * PITCH);
        // From between two cards: to the edge of the nearest, a page on.
        s.offset = 2.4 * PITCH;
        s.page(false, 6, PITCH, max);
        assert_eq!(s.offset, 0.0);
        s.offset = 2.6 * PITCH;
        s.page(true, 6, PITCH, max);
        assert_eq!(s.offset, 9.0 * PITCH);
    }

    #[test]
    fn a_drag_takes_the_row_and_rests_on_a_card() {
        let max = Scroll::max(12, PITCH, 16.0, VIEW);
        let mut s = Scroll { offset: PITCH, ..Scroll::default() };
        // Not pressed on the row: nothing.
        assert!(!s.drag(500.0, max));
        s.press(500.0);
        // A click moves less than a drag: the row stays.
        assert!(!s.drag(500.0 - DRAG_FROM + 1.0, max));
        assert_eq!(s.offset, PITCH);
        assert!(!s.release(PITCH, max), "a click, not a drag");
        assert_eq!(s.offset, PITCH);
        // Pulled left by 300: the row goes on by 300 with the pointer.
        s.press(500.0);
        assert!(s.drag(200.0, max) && s.dragging());
        assert_eq!(s.offset, PITCH + 300.0);
        // Back near where it went down: still a drag.
        assert!(s.drag(497.0, max));
        assert_eq!(s.offset, PITCH + 3.0);
        assert!(s.drag(200.0, max));
        assert!(s.release(PITCH, max));
        assert_eq!(s.offset, 2.0 * PITCH, "at rest on the nearest card's edge");
        assert!(!s.dragging());
        // Past the ends: held there.
        s.press(0.0);
        assert!(s.drag(-100_000.0, max));
        assert_eq!(s.offset, max);
        assert!(s.drag(100_000.0, max));
        assert_eq!(s.offset, 0.0);
        s.release(PITCH, max);
        assert_eq!(s.offset, 0.0);
    }
}
