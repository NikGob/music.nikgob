//! The queue as a sheet over the player (Metrolist's queue): two heights
//! (most of the screen, or all but its top), dragged by its handle between
//! them or down to close. Its head names what plays, with «Сохранить как
//! плейлист» and «Очистить»; the song playing first, then what comes, each
//! with its handle to move it and a swipe to the left to take it out.

use egui::{Area, Context, Id, Order, Rect, Sense, Stroke, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{IconButton, TextInputDialog, TextInputResult, anim};
use nikgob_md3e::tokens::Elevation;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Command, Route, TrackRow, ViewState};

use crate::gesture::Sprung;
use crate::rows::{self, Look, ROW_H};
use crate::text;

/// The sheet's heights, as parts of the screen.
const LOW: f32 = 0.62;
const HIGH: f32 = 0.92;
const HANDLE_H: f32 = 28.0;
const HEAD_H: f32 = 48.0;

#[derive(Clone, Copy, Debug)]
struct Reorder {
    entry: u64,
    from: usize,
    /// The pointer's height on screen.
    y: f32,
}

pub struct QueueSheet {
    height: Sprung,
    open: bool,
    drag_from: Option<f32>,
    reorder: Option<Reorder>,
    naming: bool,
    name: String,
}

impl Default for QueueSheet {
    fn default() -> Self {
        Self { height: Sprung::at(0.0), open: false, drag_from: None, reorder: None, naming: false, name: String::new() }
    }
}

/// The stop a sheet let go at `at` (part of the screen) moving at `v`
/// (parts per second, up positive) settles on: a fling goes one stop its
/// way (down from the lower one closes), else the nearest.
pub fn stop_for(at: f32, v: f32) -> f32 {
    const FLING: f32 = 0.9;
    if v > FLING {
        if at < LOW { LOW } else { HIGH }
    } else if v < -FLING {
        if at > LOW { LOW } else { 0.0 }
    } else {
        [0.0, LOW, HIGH].into_iter().min_by(|a, b| (a - at).abs().total_cmp(&(b - at).abs())).unwrap_or(0.0)
    }
}

/// Where a row moved by its handle lands: the row under `y` of a list at
/// `top` of `n` rows.
pub fn drop_index(y: f32, top: f32, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (((y - top) / ROW_H).floor().max(0.0) as usize).min(n - 1)
}

impl QueueSheet {
    pub fn is_open(&self) -> bool {
        self.open || self.naming
    }

    pub fn open(&mut self, ctx: &Context) {
        self.open = true;
        let now = anim::now(ctx);
        let spring = Theme::get(ctx).motion.default_spatial;
        self.height.go(LOW, spring, now);
    }

    pub fn close(&mut self, ctx: &Context) {
        let now = anim::now(ctx);
        let spring = Theme::get(ctx).motion.default_spatial;
        self.height.go(0.0, spring, now);
        self.naming = false;
    }

    /// Draws the sheet; a long press on a row asks for its menu.
    pub fn show(&mut self, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) -> Option<(TrackRow, Route)> {
        if self.naming {
            let mut open = true;
            let r = TextInputDialog::new("phone-save-queue", "Сохранить как плейлист", "Название")
                .icon(Icon::PlaylistAdd)
                .buttons("Сохранить", "Отмена")
                .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned()))
                .show(ctx, &mut open, &mut self.name);
            if matches!(r, Some(TextInputResult::Confirmed)) {
                out.push(Command::SaveQueueAsPlaylist(self.name.trim().to_owned()));
            }
            if r.is_some() || !open {
                self.naming = false;
            }
        }
        let now = anim::now(ctx);
        let theme = Theme::get(ctx);
        let s = theme.scheme;
        let k = self.height.value(now).max(0.0);
        if !self.height.settled(now) {
            ctx.request_repaint();
        }
        if self.height.target() == 0.0 && self.height.settled(now) {
            self.open = false;
            self.reorder = None;
            return None;
        }
        let screen = ctx.content_rect();
        let h = screen.height() * k;
        let rect = Rect::from_min_max(pos2(screen.left(), screen.bottom() - h), pos2(screen.right(), screen.bottom() + 40.0));
        let mut menu = None;
        let mut close = false;
        let shown = (k / LOW).clamp(0.0, 1.0);
        Area::new(Id::new("phone-queue-scrim")).order(Order::Foreground).fixed_pos(screen.min).fade_in(false).show(ctx, |ui| {
            ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha((0.32 * 255.0 * shown) as u8));
            if ui.interact(screen, Id::new("phone-queue-scrim-tap"), Sense::click()).clicked() {
                close = true;
            }
        });
        Area::new(Id::new("phone-queue")).order(Order::Foreground).fixed_pos(rect.min).fade_in(false).constrain(false).show(ctx, |ui| {
            let radii = Radii::top(28.0);
            paint::shadow(ui.painter(), rect, radii, Elevation::Level1);
            paint::container(ui.painter(), rect, radii, s.surface_container_low, Stroke::NONE);
            ui.interact(rect, Id::new("phone-queue-bg"), Sense::click());
            // The handle drags the sheet.
            let strip = Rect::from_min_size(rect.min, vec2(rect.width(), HANDLE_H));
            let grab = ui.interact(strip, Id::new("phone-queue-grab"), Sense::drag());
            paint::container(
                ui.painter(),
                Rect::from_center_size(pos2(strip.center().x, strip.top() + 14.0), vec2(32.0, 4.0)),
                Radii::all(2.0),
                s.on_surface_variant.gamma_multiply(if grab.dragged() { 1.0 } else { 0.4 }),
                Stroke::NONE,
            );
            if grab.drag_started() {
                self.drag_from = Some(k);
            }
            if grab.dragged()
                && let (Some(from), Some(d)) = (self.drag_from, grab.total_drag_delta())
            {
                self.height.hold((from - d.y / screen.height()).clamp(0.0, 1.0), now);
            }
            if grab.drag_stopped() {
                self.drag_from = None;
                let v = -ctx.input(|i| i.pointer.velocity().y) / screen.height().max(1.0);
                let at = self.height.value(now);
                self.height.release(at, v, stop_for(at, v), theme.motion.default_spatial, now);
            }
            // The head.
            let head = Rect::from_min_size(pos2(rect.left() + 16.0, strip.bottom()), vec2(rect.width() - 24.0, HEAD_H));
            let title = state.queue.radio.as_ref().map_or_else(|| state.player.context_line(), |r| r.title.clone());
            let clear = Rect::from_min_size(pos2(head.right() - 48.0, head.top()), vec2(48.0, 48.0));
            let save = clear.translate(vec2(-48.0, 0.0));
            let style = TypeRole::TitleMedium.style();
            text::paint_line(ui, pos2(head.left(), head.center().y - style.line_height * 0.5), title, style, s.on_surface, save.left() - head.left() - 8.0);
            let has_next = !state.queue.next.is_empty();
            if ui.put(save, IconButton::new(Icon::PlaylistAdd).enabled(state.queue.current.is_some()).id_salt("phone-queue-save")).clicked() {
                self.name.clear();
                self.naming = true;
            }
            if ui.put(clear, IconButton::new(Icon::Delete).enabled(has_next).id_salt("phone-queue-clear")).clicked() {
                out.push(Command::ClearQueue);
            }
            let list = Rect::from_min_max(pos2(rect.left(), head.bottom()), pos2(rect.right(), screen.bottom()));
            let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list));
            lui.set_clip_rect(list);
            crate::scroll("phone-queue-list", false).show(&mut lui, |ui| {
                if let Some(t) = &state.queue.current {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                    let look = Look { swipe_next: false, ..Look::list(t, state) };
                    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(r));
                    let o = rows::track(&mut cui, r, Id::new("phone-queue-current"), t, look, state);
                    if o.menu {
                        menu = Some((t.clone(), Route::Home));
                    }
                }
                menu = menu.take().or(self.next_rows(ui, state, out));
                ui.add_space(24.0);
            });
        });
        if close {
            self.close(ctx);
        }
        menu
    }

    /// The rows of what comes, movable by their handles.
    fn next_rows(&mut self, ui: &mut egui::Ui, state: &ViewState, out: &mut Vec<Command>) -> Option<(TrackRow, Route)> {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let next = &state.queue.next;
        let n = next.len();
        let width = ui.available_width();
        let (area, _) = ui.allocate_exact_size(vec2(width, n as f32 * ROW_H), Sense::hover());
        // A row moved by its handle stays under the finger; those between
        // its place and where it would land make room for it.
        let target = self.reorder.map(|r| drop_index(r.y, area.top(), n));
        let range = rows::in_sight(ui.clip_rect(), area.top(), ROW_H, n);
        let mut menu = None;
        let mut started: Option<Reorder> = None;
        let mut moving: Option<f32> = None;
        let mut dropped = false;
        let mut lifted: Option<usize> = None;
        let first = range.start.saturating_sub(1);
        let last = (range.end + 1).min(n);
        for i in first..last {
            let e = &next[i];
            let shift = match (self.reorder, target) {
                (Some(r), Some(j)) if r.from < j && i > r.from && i <= j => -ROW_H,
                (Some(r), Some(j)) if j < r.from && i >= j && i < r.from => ROW_H,
                _ => 0.0,
            };
            if self.reorder.is_some_and(|r| r.from == i) {
                lifted = Some(i);
                continue;
            }
            let shift = anim::f32(&ctx, Id::new(("phone-queue-shift", e.entry)), shift, theme.motion.fast_spatial);
            let r = Rect::from_min_size(pos2(area.left(), area.top() + i as f32 * ROW_H + shift), vec2(width, ROW_H));
            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt(("phone-queue-row", e.entry)));
            let look = Look { swipe_next: false, swipe_remove: true, more: false, handle: true, ..Look::list(&e.track, state) };
            let o = rows::track(&mut cui, r, Id::new(("phone-queue-row", e.entry)), &e.track, look, state);
            if o.tap {
                out.push(Command::PlayQueueEntry(e.entry));
            }
            if o.remove {
                out.push(Command::RemoveFromQueue(e.entry));
            }
            if o.menu {
                menu = Some((e.track.clone(), Route::Home));
            }
            if let Some(h) = o.handle
                && h.drag_started()
            {
                let y = ctx.input(|i| i.pointer.interact_pos()).map_or(r.center().y, |p| p.y);
                started = Some(Reorder { entry: e.entry, from: i, y });
            }
        }
        // The lifted row, over the others.
        if let (Some(i), Some(r)) = (lifted, self.reorder) {
            let e = &next[i];
            let y = r.y.clamp(area.top(), area.bottom().max(area.top() + ROW_H)) - ROW_H * 0.5;
            let rect = Rect::from_min_size(pos2(area.left(), y), vec2(width, ROW_H));
            let s = theme.scheme;
            paint::shadow(ui.painter(), rect.shrink2(vec2(8.0, 2.0)), Radii::all(16.0), Elevation::Level2);
            paint::container(ui.painter(), rect.shrink2(vec2(8.0, 2.0)), Radii::all(16.0), s.surface_container_highest, Stroke::NONE);
            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(("phone-queue-row", e.entry)));
            let look = Look { swipe_next: false, swipe_remove: false, more: false, handle: true, ..Look::list(&e.track, state) };
            let o = rows::track(&mut cui, rect, Id::new(("phone-queue-row", e.entry)), &e.track, look, state);
            if let Some(h) = o.handle {
                if h.dragged() {
                    moving = ctx.input(|i| i.pointer.interact_pos()).map(|p| p.y);
                }
                if h.drag_stopped() {
                    dropped = true;
                }
            }
            if !ctx.input(|i| i.pointer.primary_down()) {
                dropped = true;
            }
        }
        if let Some(r) = started {
            self.reorder = Some(r);
        }
        if let (Some(r), Some(y)) = (self.reorder.as_mut(), moving) {
            r.y = y;
        }
        if dropped && let Some(r) = self.reorder.take() {
            let j = drop_index(r.y, area.top(), n);
            if j != r.from
                && let Some(anchor) = next.get(j)
            {
                out.push(Command::MoveInQueue { entry: r.entry, anchor: anchor.entry, after: j > r.from });
            }
        }
        if self.reorder.is_some() {
            ctx.request_repaint();
        }
        menu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_let_go_sheet_goes_to_the_nearest_stop_or_the_way_it_was_flung() {
        assert_eq!(stop_for(0.6, 0.0), LOW);
        assert_eq!(stop_for(0.85, 0.0), HIGH);
        assert_eq!(stop_for(0.2, 0.0), 0.0);
        assert_eq!(stop_for(0.6, 2.0), HIGH);
        assert_eq!(stop_for(0.6, -2.0), 0.0);
        assert_eq!(stop_for(0.9, -2.0), LOW);
        assert_eq!(stop_for(0.3, 2.0), LOW);
    }

    #[test]
    fn a_moved_row_lands_on_the_row_under_the_finger() {
        assert_eq!(drop_index(100.0, 100.0, 5), 0);
        assert_eq!(drop_index(100.0 + ROW_H * 2.5, 100.0, 5), 2);
        assert_eq!(drop_index(5000.0, 100.0, 5), 4);
        assert_eq!(drop_index(0.0, 100.0, 5), 0);
        assert_eq!(drop_index(0.0, 100.0, 0), 0);
    }
}
