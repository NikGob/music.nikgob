//! A row of Главная that goes on sideways: more cards than fit stand to
//! the right of the grid, and the row scrolls to them.
//!
//! * **The wheel**: a notch of a sideways wheel (a tilt wheel) or of
//!   Shift + the wheel moves the row under the pointer one card on, on the
//!   spatial spring; notches in a row add up, each a card. A touchpad's
//!   two fingers sideways (and a wheel that turns in parts of a notch) take
//!   the row with them, and once they stop for [`SETTLE_AFTER`] the row
//!   comes to rest on the nearest card's edge. The plain wheel always
//!   scrolls the page, whatever is under the pointer.
//! * **A drag**: the pointer held on the row and moved [`DRAG_FROM`]
//!   sideways takes the row with it (the card under it is not clicked: a
//!   click moves less); let go, the row goes on a little the way it was
//!   flung ([`FLING`], at most a view of cards) and comes to rest on a
//!   card's edge.
//! * **The arrows**: a filled tonal icon button (40, M3's standard) at
//!   each end the row can still go to, on the middle of the covers, faded
//!   in while the pointer is on the row and out when it leaves; a click
//!   sends the row a page of cards on, on the spatial spring, to rest on
//!   a card's edge. They take no room: nothing moves when they come and
//!   go.
//!
//! The row's state ([`Scroll`]) is plain numbers, kept in egui's memory by
//! the row's name; the cards are painted clipped to the grid's width, and
//! only those in view. [`show`] tells when the row is near its end, so the
//! page can ask for more cards.

use egui::{Event, Id, MouseWheelUnit, Rect, Response, Ui, UiBuilder, Vec2, pos2, vec2};

use super::{GUTTER, Grid};
use crate::{
    Icon, Theme,
    components::{IconButton, Size, Style, anim},
};

/// The pointer moved this far sideways with the button down: a drag.
pub const DRAG_FROM: f32 = 8.0;
/// The arrows stand this far inside the row's ends.
const ARROW_INSET: f32 = 8.0;
/// A touchpad that stopped this long: the row comes to rest on a card.
pub const SETTLE_AFTER: f64 = 0.15;
/// A drag let go goes on as far as it would in this long at its speed.
pub const FLING: f32 = 0.15;
/// A drag that stood still this long before the button went up is not
/// flung.
const FLING_STILL: f64 = 0.08;
/// The row is near its end when less than this many views of cards are
/// left to the right of the view.
const NEAR_END: f32 = 1.0;

/// How far a row is scrolled, and the drag or the touchpad going on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    /// Where the row is going: how far its first card is out at the left.
    pub offset: f32,
    /// The pointer went down on the row at this x with the row at this
    /// offset.
    press: Option<(f32, f32)>,
    dragging: bool,
    /// Moved by a touchpad, off a card's edge: it comes to rest on one
    /// once the touchpad stops ([`Self::settle`]).
    loose: bool,
    /// When the touchpad last moved it.
    glided: f64,
    /// The drag's speed (offset per second) and its last move (when, to
    /// what offset).
    speed: f32,
    moved: Option<(f64, f32)>,
}

/// What the wheel did over a row in a frame: whole notches (cards) and
/// smooth points, both as egui's scroll delta (positive brings what is at
/// the left).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Turn {
    pub cards: f32,
    pub points: f32,
}

impl Turn {
    /// The sideways wheel of `events`: a notch is a whole line (a tilt
    /// wheel, Shift + a wheel); parts of a line (a precise touchpad on
    /// Windows or X11, a wheel that turns freely) and points (a touchpad on
    /// Wayland) go as points, `line` points a line. Shift turns the plain
    /// wheel sideways.
    pub fn of(events: &[Event], line: f32) -> Self {
        let mut turn = Self::default();
        for e in events {
            let Event::MouseWheel { unit, delta, modifiers, .. } = e else { continue };
            let d = if modifiers.shift { delta.x + delta.y } else { delta.x };
            if d == 0.0 {
                continue;
            }
            match unit {
                MouseWheelUnit::Line if d.abs() >= 0.99 && (d - d.round()).abs() < 0.01 => turn.cards += d.round(),
                MouseWheelUnit::Line => turn.points += d * line,
                MouseWheelUnit::Page => turn.cards += d.round(),
                MouseWheelUnit::Point => turn.points += d,
            }
        }
        turn
    }
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

    /// A touchpad moved the content by `delta` at `now` (as egui's scroll
    /// delta: positive brings what is at the left); the row follows it.
    pub fn glide(&mut self, delta: f32, max: f32, now: f64) {
        self.offset = (self.offset - delta).clamp(0.0, max.max(0.0));
        self.loose = true;
        self.glided = now;
    }

    /// `cards` notches of the wheel (positive brings what is at the left):
    /// as many cards on, from the card's edge the row is at or passes.
    pub fn step(&mut self, cards: i64, pitch: f32, max: f32) {
        if pitch <= 0.0 || cards == 0 {
            return;
        }
        let at = self.offset / pitch;
        // From between two cards the first notch goes to the next edge
        // that way; from the row's end (not on an edge) back to the last.
        let from = if cards < 0 { (at + 0.01).floor() } else { (at - 0.01).ceil() };
        let to = from as i64 - cards;
        self.offset = (to.max(0) as f32 * pitch).clamp(0.0, max.max(0.0));
        self.loose = false;
    }

    /// The touchpad stopped [`SETTLE_AFTER`] ago: the row comes to rest on
    /// the nearest card's edge. True when it did now.
    pub fn settle(&mut self, now: f64, pitch: f32, max: f32) -> bool {
        if !self.loose || self.dragging || now - self.glided < SETTLE_AFTER {
            return false;
        }
        self.loose = false;
        if pitch > 0.0 {
            self.offset = ((self.offset / pitch).round() * pitch).clamp(0.0, max.max(0.0));
        }
        true
    }

    /// Moved by a touchpad and not at rest yet.
    pub fn loose(&self) -> bool {
        self.loose
    }

    /// An arrow: `per` cards on (`forward`) or back, to a card's edge.
    pub fn page(&mut self, forward: bool, per: usize, pitch: f32, max: f32) {
        if pitch <= 0.0 {
            return;
        }
        let at = (self.offset / pitch).round() as i64;
        let to = at + if forward { per.max(1) as i64 } else { -(per.max(1) as i64) };
        self.offset = (to.max(0) as f32 * pitch).clamp(0.0, max.max(0.0));
        self.loose = false;
    }

    pub fn can_back(&self) -> bool {
        self.offset > 0.5
    }

    pub fn can_forward(&self, max: f32) -> bool {
        self.offset < max - 0.5
    }

    /// Less than [`NEAR_END`] views of cards are left to the right of the
    /// view `view` wide (a row that does not scroll is never near it).
    pub fn near_end(&self, max: f32, view: f32) -> bool {
        max > 0.0 && max - self.offset < NEAR_END * view
    }

    /// The button went down on the row at `x`.
    pub fn press(&mut self, x: f32) {
        self.press = Some((x, self.offset));
        self.dragging = false;
        self.speed = 0.0;
        self.moved = None;
    }

    /// The pointer is at `x` with the button down at `now`: true once it
    /// drags the row (it moved [`DRAG_FROM`] from where it went down).
    pub fn drag(&mut self, x: f32, max: f32, now: f64) -> bool {
        let Some((x0, from)) = self.press else { return false };
        if !self.dragging && (x - x0).abs() >= DRAG_FROM {
            self.dragging = true;
        }
        if self.dragging {
            let offset = (from - (x - x0)).clamp(0.0, max.max(0.0));
            match self.moved {
                Some((t, o)) if offset != o => {
                    let dt = now - t;
                    if dt > 0.0 {
                        let v = (offset - o) / dt as f32;
                        // A move after a pause starts the speed afresh.
                        self.speed = if dt > FLING_STILL { v } else { 0.5 * self.speed + 0.5 * v };
                        self.moved = Some((now, offset));
                    }
                }
                Some(_) => {}
                None => self.moved = Some((now, offset)),
            }
            self.offset = offset;
            self.loose = false;
        }
        self.dragging
    }

    /// The button went up at `now`: a drag goes on the way it was flung
    /// (at most `per` cards) and ends on the nearest card's edge there.
    /// True when it was a drag.
    pub fn release(&mut self, pitch: f32, max: f32, per: usize, now: f64) -> bool {
        let was = self.dragging;
        if was && pitch > 0.0 {
            let still = self.moved.is_none_or(|(t, _)| now - t > FLING_STILL);
            let reach = per.max(1) as f32 * pitch;
            let on = if still { 0.0 } else { (self.speed * FLING).clamp(-reach, reach) };
            self.offset = (((self.offset + on) / pitch).round() * pitch).clamp(0.0, max.max(0.0));
        }
        self.press = None;
        self.dragging = false;
        self.speed = 0.0;
        self.moved = None;
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
    /// How far past the grid's edges its cards may paint (a song row's
    /// state layer reaches into the margin).
    pub bleed: f32,
}

/// An arrow of a row in `slot`: fading in while `shown` and out after (the
/// fast effects spring), taking clicks while it is there to see.
fn arrow(ui: &mut Ui, id: Id, shown: bool, slot: Rect, button: IconButton) -> Option<Response> {
    let theme = Theme::get(ui.ctx());
    let k = anim::fraction(ui.ctx(), id.with("fade"), f32::from(u8::from(shown)), theme.motion.fast_effects).clamp(0.0, 1.0);
    if k <= 0.01 {
        return None;
    }
    let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt(id.with("slot")));
    child.set_opacity(k * ui.opacity());
    Some(child.add_enabled(shown && k > 0.5, button))
}

/// Lays `row` out on `grid` and calls `card` with the place of each card
/// in view (see the module docs). True when the row is scrolled near its
/// end ([`Scroll::near_end`]).
pub fn show(ui: &mut Ui, grid: Grid, row: Row<'_>, mut card: impl FnMut(&mut Ui, usize, Rect)) -> bool {
    let per = row.per.max(1);
    let card_w = grid.span(12 / per);
    let pitch = card_w + GUTTER;
    let view = Rect::from_min_size(pos2(grid.left, row.top), vec2(grid.width, row.card_h));
    let max = Scroll::max(row.n, pitch, GUTTER, grid.width);
    let ctx = ui.ctx().clone();
    let id = Id::new(("home-row", row.name));
    let mut state: Scroll = if row.live { ctx.data(|d| d.get_temp(id)).unwrap_or_default() } else { Scroll::default() };
    state.clamp(max);
    let now = anim::now(&ctx);
    // What moves the row by hand this frame (a drag, a touchpad) is
    // followed at once; a notch, an arrow, the end of a drag and a
    // touchpad's rest ride a spring.
    let mut by_hand = false;
    let hovered = row.live && ui.rect_contains_pointer(view);
    if row.live && max > 0.0 {
        if hovered {
            let line = ctx.options(|o| o.input_options.line_scroll_speed);
            let turn = ctx.input_mut(|i| {
                let turn = Turn::of(&i.events, line);
                // The sideways scroll is the row's, all of it (egui hands
                // a notch out over a few frames); the page keeps the
                // plain wheel.
                i.smooth_scroll_delta.x = 0.0;
                if i.modifiers.shift {
                    i.smooth_scroll_delta.y = 0.0;
                }
                turn
            });
            if turn.cards != 0.0 {
                state.step(turn.cards as i64, pitch, max);
            }
            if turn.points != 0.0 {
                state.glide(turn.points, max, now);
                by_hand = true;
            }
        }
        let (pressed, down, at) = ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.interact_pos()));
        if pressed && hovered && let Some(p) = at {
            state.press(p.x);
        }
        if down {
            if let Some(p) = at
                && state.drag(p.x, max, now)
            {
                by_hand = true;
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            }
        } else {
            state.release(pitch, max, per, now);
        }
        if !by_hand && state.loose() && !state.settle(now, pitch, max) {
            // Comes back to rest it once the touchpad has stopped.
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE_AFTER));
        }
    }
    let spring_id = id.with("x");
    let shown = if !row.live {
        0.0
    } else if by_hand {
        anim::snap_f32(&ctx, spring_id, state.offset);
        state.offset
    } else {
        // The spring's overshoot shows between two cards; at the row's
        // ends it stops on the edge.
        anim::f32(&ctx, spring_id, state.offset, Theme::get(&ctx).motion.default_spatial).clamp(0.0, max)
    };

    // The cards, cut at the grid's edges (a little room above and below
    // for what a card paints around itself).
    let mut child = ui.new_child(UiBuilder::new().max_rect(view).id_salt(("home-row-ui", row.name)));
    child.set_clip_rect(view.expand2(vec2(row.bleed, 8.0)).intersect(ui.clip_rect()));
    for i in 0..row.n {
        let cell = Rect::from_min_size(pos2(grid.left + i as f32 * pitch - shown, row.top), vec2(card_w, row.card_h));
        if cell.right() <= view.left() || cell.left() >= view.right() {
            continue;
        }
        card(&mut child, i, cell);
    }

    if row.live && max > 0.0 {
        let d = Size::S.height();
        let y = row.top + row.arrows_at - d / 2.0;
        let button = |icon: Icon, tip: &'static str, salt: &'static str| IconButton::new(icon).size(Size::S).style(Style::Tonal).tooltip(tip).id_salt(id.with(salt));
        let on = hovered && !state.dragging();
        let back = Rect::from_min_size(pos2(view.left() + ARROW_INSET, y), Vec2::splat(d));
        let clicked = arrow(ui, id.with("back"), on && state.can_back(), back, button(Icon::ChevronLeft, "Назад", "home-row-back"));
        if clicked.is_some_and(|r| r.clicked()) {
            state.page(false, per, pitch, max);
        }
        let forward = Rect::from_min_size(pos2(view.right() - ARROW_INSET - d, y), Vec2::splat(d));
        let clicked = arrow(ui, id.with("forward"), on && state.can_forward(max), forward, button(Icon::ChevronRight, "Дальше", "home-row-forward"));
        if clicked.is_some_and(|r| r.clicked()) {
            state.page(true, per, pitch, max);
        }
    }
    if row.live {
        ctx.data_mut(|d| d.insert_temp(id, state));
    }
    row.live && state.near_end(max, grid.width)
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
        // A touchpad: as far as the row goes, no farther.
        s.glide(-100.0, max, 0.0);
        assert_eq!(s.offset, 100.0);
        assert!(s.can_back() && s.can_forward(max));
        s.glide(-10_000.0, max, 0.0);
        assert_eq!(s.offset, max);
        assert!(s.can_back() && !s.can_forward(max));
        s.glide(10_000.0, max, 0.0);
        assert_eq!(s.offset, 0.0);
        // A wider window: the row is back within its ends.
        s.offset = max;
        s.clamp(PITCH);
        assert_eq!(s.offset, PITCH);
        s.clamp(0.0);
        assert_eq!(s.offset, 0.0);
    }

    #[test]
    fn a_notch_moves_the_row_one_card() {
        let max = Scroll::max(20, PITCH, 16.0, VIEW);
        let mut s = Scroll::default();
        // A notch to the right (the content goes left): one card on.
        s.step(-1, PITCH, max);
        assert_eq!(s.offset, PITCH);
        s.step(-1, PITCH, max);
        assert_eq!(s.offset, 2.0 * PITCH);
        // Notches in a row add up, card by card.
        s.step(-3, PITCH, max);
        assert_eq!(s.offset, 5.0 * PITCH);
        s.step(2, PITCH, max);
        assert_eq!(s.offset, 3.0 * PITCH);
        // From between two cards: to the next edge that way.
        s.offset = 2.3 * PITCH;
        s.step(-1, PITCH, max);
        assert_eq!(s.offset, 3.0 * PITCH);
        s.offset = 2.7 * PITCH;
        s.step(1, PITCH, max);
        assert_eq!(s.offset, 2.0 * PITCH);
        // Not past the ends.
        s.offset = 0.0;
        s.step(1, PITCH, max);
        assert_eq!(s.offset, 0.0);
        s.step(-100, PITCH, max);
        assert_eq!(s.offset, max);
        // At the end of a row whose last card ends off an edge: back to
        // the last edge, not a card and a half.
        let max = Scroll::max(8, PITCH, 16.0, VIEW - 100.0);
        s.offset = max;
        s.step(1, PITCH, max);
        assert_eq!(s.offset, (max / PITCH).floor() * PITCH);
    }

    #[test]
    fn the_wheel_tells_notches_from_a_touchpad() {
        let wheel = |unit: MouseWheelUnit, delta: Vec2, shift: bool| Event::MouseWheel {
            unit,
            delta,
            phase: egui::TouchPhase::Move,
            modifiers: if shift { egui::Modifiers::SHIFT } else { egui::Modifiers::NONE },
        };
        // A tilt wheel's notch, Shift + the wheel's notch: a card each.
        let t = Turn::of(&[wheel(MouseWheelUnit::Line, vec2(-1.0, 0.0), false)], 40.0);
        assert_eq!(t, Turn { cards: -1.0, points: 0.0 });
        let t = Turn::of(&[wheel(MouseWheelUnit::Line, vec2(0.0, -1.0), true), wheel(MouseWheelUnit::Line, vec2(0.0, -1.0), true)], 40.0);
        assert_eq!(t, Turn { cards: -2.0, points: 0.0 });
        // The plain wheel is the page's.
        let t = Turn::of(&[wheel(MouseWheelUnit::Line, vec2(0.0, -1.0), false)], 40.0);
        assert_eq!(t, Turn::default());
        // A touchpad: points, or parts of a line.
        let t = Turn::of(&[wheel(MouseWheelUnit::Point, vec2(-12.0, 3.0), false)], 40.0);
        assert_eq!(t, Turn { cards: 0.0, points: -12.0 });
        let t = Turn::of(&[wheel(MouseWheelUnit::Line, vec2(0.25, 0.0), false)], 40.0);
        assert_eq!(t, Turn { cards: 0.0, points: 10.0 });
    }

    #[test]
    fn a_touchpad_takes_the_row_then_it_rests_on_a_card() {
        let max = Scroll::max(20, PITCH, 16.0, VIEW);
        let mut s = Scroll::default();
        s.glide(-300.0, max, 1.0);
        assert_eq!(s.offset, 300.0);
        assert!(s.loose());
        // Still moving: it stays where the fingers took it.
        assert!(!s.settle(1.0 + SETTLE_AFTER / 2.0, PITCH, max));
        assert_eq!(s.offset, 300.0);
        s.glide(-30.0, max, 1.1);
        assert!(!s.settle(1.1 + SETTLE_AFTER / 2.0, PITCH, max));
        // Stopped: on the nearest card's edge.
        assert!(s.settle(1.1 + SETTLE_AFTER + 0.01, PITCH, max));
        assert_eq!(s.offset, 2.0 * PITCH);
        assert!(!s.loose());
        assert!(!s.settle(5.0, PITCH, max), "once");
        // A notch while it glides puts it on an edge at once.
        s.glide(-50.0, max, 6.0);
        s.step(-1, PITCH, max);
        assert_eq!(s.offset, 3.0 * PITCH);
        assert!(!s.loose());
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
        assert!(!s.drag(500.0, max, 0.0));
        s.press(500.0);
        // A click moves less than a drag: the row stays.
        assert!(!s.drag(500.0 - DRAG_FROM + 1.0, max, 0.0));
        assert_eq!(s.offset, PITCH);
        assert!(!s.release(PITCH, max, 6, 0.0), "a click, not a drag");
        assert_eq!(s.offset, PITCH);
        // Pulled left by 300: the row goes on by 300 with the pointer.
        s.press(500.0);
        assert!(s.drag(200.0, max, 1.0) && s.dragging());
        assert_eq!(s.offset, PITCH + 300.0);
        // Back near where it went down: still a drag.
        assert!(s.drag(497.0, max, 1.5));
        assert_eq!(s.offset, PITCH + 3.0);
        assert!(s.drag(200.0, max, 2.0));
        // Held still before the button went up: no fling.
        assert!(s.release(PITCH, max, 6, 2.5));
        assert_eq!(s.offset, 2.0 * PITCH, "at rest on the nearest card's edge");
        assert!(!s.dragging());
        // Past the ends: held there.
        s.press(0.0);
        assert!(s.drag(-100_000.0, max, 3.0));
        assert_eq!(s.offset, max);
        assert!(s.drag(100_000.0, max, 3.1));
        assert_eq!(s.offset, 0.0);
        s.release(PITCH, max, 6, 3.5);
        assert_eq!(s.offset, 0.0);
    }

    #[test]
    fn a_flung_drag_goes_on_and_rests_on_a_card() {
        let max = Scroll::max(30, PITCH, 16.0, VIEW);
        let mut s = Scroll::default();
        s.press(1000.0);
        // 100 px a frame to the left at 60 frames a second: 6000 px/s.
        let mut x = 1000.0;
        let mut t = 0.0;
        for _ in 0..4 {
            x -= 100.0;
            t += 1.0 / 60.0;
            assert!(s.drag(x, max, t));
        }
        assert_eq!(s.offset, 400.0);
        // Let go at once: on by at most a view of cards, to an edge.
        assert!(s.release(PITCH, max, 6, t + 1.0 / 60.0));
        assert!(s.offset > 400.0 + PITCH, "flung: {}", s.offset);
        assert!(s.offset <= 400.0 + 6.0 * PITCH + PITCH / 2.0, "at most a view on: {}", s.offset);
        assert_eq!(s.offset % PITCH, 0.0, "on a card's edge");
    }

    #[test]
    fn a_row_is_near_its_end_within_a_view() {
        let max = Scroll::max(20, PITCH, 16.0, VIEW);
        let mut s = Scroll::default();
        assert!(!s.near_end(max, VIEW));
        s.offset = max - VIEW - 1.0;
        assert!(!s.near_end(max, VIEW));
        s.offset = max - VIEW + 1.0;
        assert!(s.near_end(max, VIEW));
        s.offset = max;
        assert!(s.near_end(max, VIEW));
        assert!(!Scroll::default().near_end(0.0, VIEW), "a row that fits asks for nothing");
    }
}
