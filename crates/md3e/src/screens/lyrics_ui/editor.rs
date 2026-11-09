//! The lyrics editor («Изменить текст…»): the shown text's lines as a
//! list, each with its start, to fix by hand (a line's text inline; delete,
//! add, split at the cursor, merge with the next) and to time while the
//! song plays («Отметить», Space or Enter, times the selected line where
//! the song is and selects the next; ±0,1 с nudges; a click on a time
//! seeks there). «Сохранить» keeps it as the user's own text
//! (`Command::SaveOwnLyrics`), shown first from then on; «Отмена» gives it
//! up. Closed any other way, the draft stays for the session ([`Draft`]).
//!
//! It opens in a window like the «Окно» look's ([`super::dialog::window`],
//! also for the «Меню» look) or, in the «Экран» look, in place of the text
//! on the «Текст песни» page ([`page`]).

use std::time::Duration;

use egui::{Align, Context, CursorIcon, Frame, Id, Key, Layout, Margin, Modifiers, Rect, Sense, TextEdit, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{Command, Status, TrackId, ViewState};

use super::edit::Draft;
use crate::components::{Button, IconButton, Size, Style};
use crate::tokens::shape;
use crate::{Icon, Theme, TypeRole};

/// The editor's window ([`super::dialog::window`]).
const WINDOW: &str = "lyrics-edit-window";
/// The track whose draft the window shows.
const SHOWN: &str = "lyrics-edit-track";
/// The track whose draft the page shows in place of the text.
const PAGE: &str = "lyrics-edit-page";
/// The frame the editor was last drawn in (it takes the keys then).
const DRAWN: &str = "lyrics-edit-drawn";
const MAX_W: f32 = 720.0;
const MAX_H: f32 = 760.0;
/// A line's row.
const ROW: f32 = 40.0;
/// The start's column.
const TIME_W: f32 = 76.0;
/// A nudge of a line's start.
const NUDGE_MS: i64 = 100;

fn draft_id(track: TrackId) -> Id {
    Id::new(("lyrics-edit-draft", track))
}

fn draft(ctx: &Context, track: TrackId) -> Option<Draft> {
    ctx.data(|d| d.get_temp::<Draft>(draft_id(track)))
}

/// The shown text can be edited: a track with a text.
pub(crate) fn editable(state: &ViewState) -> bool {
    state.lyrics.track.is_some() && state.lyrics.source.is_some()
}

/// Starts editing the shown text: the draft left for it this session, else
/// one made of the text.
fn begin(ctx: &Context, state: &ViewState) -> Option<TrackId> {
    let track = state.lyrics.track?;
    if draft(ctx, track).is_none() {
        let title = state.player.current.as_ref().filter(|t| t.id == track).map(|t| format!("{} · {}", t.title, t.artist)).unwrap_or_default();
        let d = Draft::of(&state.lyrics, title)?;
        ctx.data_mut(|x| x.insert_temp(draft_id(track), d));
    }
    Some(track)
}

/// Opens the editor in its window growing out of `from` (the control).
pub(crate) fn open(ctx: &Context, state: &ViewState, from: Rect) {
    if let Some(track) = begin(ctx, state) {
        ctx.data_mut(|d| d.insert_temp(Id::new(SHOWN), track));
        super::dialog::grow(ctx, WINDOW, from);
    }
}

/// Opens the editor on the «Текст песни» page, in place of the text.
pub(crate) fn open_on_page(ctx: &Context, state: &ViewState) {
    if let Some(track) = begin(ctx, state) {
        ctx.data_mut(|d| d.insert_temp(Id::new(PAGE), Some(track)));
        super::request(ctx, super::Request::Page);
    }
}

/// Opens the editor where the look in use has it (screenshot runs).
pub fn open_any(ctx: &Context, state: &ViewState) {
    if super::variant(&state.prefs) == super::Variant::Page {
        open_on_page(ctx, state);
    } else {
        let from = ctx.data(|d| d.get_temp::<(Id, Rect)>(Id::new(super::LAST))).map_or(Rect::NOTHING, |(_, r)| r);
        open(ctx, state, from);
    }
}

/// The track the page edits in place of its text, if any.
pub(crate) fn on_page(ctx: &Context) -> Option<TrackId> {
    let track = ctx.data(|d| d.get_temp::<Option<TrackId>>(Id::new(PAGE))).flatten()?;
    draft(ctx, track).is_some().then_some(track)
}

/// The editor was drawn last frame: Space, Enter and the arrows are its
/// own while no line's text is being typed (the window's shortcuts leave
/// them alone).
pub(crate) fn takes_keys(ctx: &Context) -> bool {
    let drawn = ctx.data(|d| d.get_temp::<u64>(Id::new(DRAWN)));
    drawn.is_some_and(|f| f + 1 >= ctx.cumulative_frame_nr())
}

/// The editor's window (the «Окно» and «Меню» looks).
pub(crate) fn show(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    if super::variant(&state.prefs) == super::Variant::Page {
        return;
    }
    let Some(track) = ctx.data(|d| d.get_temp::<TrackId>(Id::new(SHOWN))) else { return };
    // Escape while typing a line only stops typing.
    let escape = !ctx.egui_wants_keyboard_input();
    let mut done = false;
    super::dialog::window(ctx, WINDOW, vec2(MAX_W, MAX_H), escape, |ui, inner| {
        done = body(ui, inner, track, true, state, out);
    });
    if done {
        super::dialog::shut(ctx, WINDOW);
    }
}

/// The editor on the page, in `rect` (in place of the text); it goes back
/// to the text once saved or given up.
pub(crate) fn page(ui: &mut Ui, rect: Rect, track: TrackId, state: &ViewState, out: &mut Vec<Command>) {
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)).id_salt("lyrics-edit-page"));
    if body(&mut child, rect, track, false, state, out) {
        ui.ctx().data_mut(|d| d.insert_temp(Id::new(PAGE), None::<TrackId>));
    }
}

/// The editor in `inner`: its head (`head`: the window's), what can be done
/// to the selected line, the lines, and «Отмена» / «Сохранить». True once
/// it is saved or given up (the draft is gone then).
fn body(ui: &mut Ui, inner: Rect, track: TrackId, head: bool, state: &ViewState, out: &mut Vec<Command>) -> bool {
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    let Some(mut d) = draft(&ctx, track) else { return true };
    let here = state.player.current.as_ref().is_some_and(|t| t.id == track);
    let pos = state.player.position_now();
    if ui.is_enabled() {
        ctx.data_mut(|x| x.insert_temp(Id::new(DRAWN), ctx.cumulative_frame_nr()));
        keys(&ctx, &mut d, here, pos);
    }
    if here && state.player.status == Status::Playing {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    ui.spacing_mut().item_spacing.y = 0.0;
    if head {
        ui.allocate_ui_with_layout(vec2(inner.width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
            ui.label(TypeRole::HeadlineSmall.style().rich("Изменить текст", s.on_surface));
        });
        if !d.title.is_empty() {
            ui.label(TypeRole::BodyMedium.style().rich(d.title.clone(), s.on_surface_variant));
        }
        ui.add_space(16.0);
    } else if !here && !d.title.is_empty() {
        // On the page, a song other than the one playing says which.
        ui.label(TypeRole::TitleSmall.style().rich(d.title.clone(), s.on_surface));
        ui.add_space(12.0);
    }
    tools(ui, &mut d, here, pos);
    ui.add_space(12.0);
    let foot = 40.0;
    let list_h = (inner.bottom() - ui.cursor().top() - 16.0 - foot).max(120.0);
    list(ui, vec2(inner.width(), list_h), &mut d, here, pos, out);
    ui.add_space(16.0);
    let mut closed = false;
    ui.allocate_ui_with_layout(vec2(inner.width(), foot), Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let worth = d.changed() && d.lines.iter().any(|l| !l.text.trim().is_empty());
        if ui.add(Button::new("Сохранить").style(Style::Filled).size(Size::S).enabled(worth).id_salt("lyrics-edit-save")).clicked() {
            out.push(Command::SaveOwnLyrics { track, lines: d.to_lines(), timed: d.timed() });
            closed = true;
        }
        if ui.add(Button::new("Отмена").style(Style::Text).size(Size::S).id_salt("lyrics-edit-cancel")).clicked() {
            closed = true;
        }
    });
    ctx.data_mut(|x| {
        if closed {
            x.remove::<Draft>(draft_id(track));
        } else {
            x.insert_temp(draft_id(track), d);
        }
    });
    closed
}

/// Space or Enter: «Отметить» (while this song plays); up and down: the
/// line above or below; left and right: the selected line's start ∓0,1 с.
/// Not while a line's text is being typed.
fn keys(ctx: &Context, d: &mut Draft, here: bool, pos: Duration) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    ctx.input_mut(|i| {
        let mark = i.consume_key(Modifiers::NONE, Key::Space) | i.consume_key(Modifiers::NONE, Key::Enter);
        if mark && here {
            d.mark(pos);
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            d.step(-1);
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            d.step(1);
        }
        for (key, ms) in [(Key::ArrowLeft, -NUDGE_MS), (Key::ArrowRight, NUDGE_MS)] {
            if i.consume_key(Modifiers::NONE, key)
                && let Some(at) = d.selected
            {
                d.nudge(at, ms);
            }
        }
    });
}

/// What can be done to the selected line: timing on the left, the text on
/// the right (wrapped under where the width is short).
fn tools(ui: &mut Ui, d: &mut Draft, here: bool, pos: Duration) {
    let sel = d.selected.filter(|i| *i < d.lines.len());
    let timed = sel.is_some_and(|i| d.lines[i].start.is_some());
    // The cursor inside the selected line's text, not at either end.
    let cut = d.cursor.filter(|(i, c)| Some(*i) == sel && *c > 0 && *c < d.lines[*i].text.chars().count());
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        if ui.add(Button::new("Отметить").style(Style::Filled).size(Size::S).enabled(here && sel.is_some()).id_salt("lyrics-edit-mark")).clicked() {
            d.mark(pos);
        }
        for (label, ms, salt) in [("−0,1 с", -NUDGE_MS, "lyrics-edit-earlier"), ("+0,1 с", NUDGE_MS, "lyrics-edit-later")] {
            if ui.add(Button::new(label).style(Style::Tonal).size(Size::S).enabled(timed).id_salt(salt)).clicked()
                && let Some(i) = sel
            {
                d.nudge(i, ms);
            }
        }
        ui.add_space(16.0);
        if ui.add(Button::new("Разделить").style(Style::Tonal).size(Size::S).enabled(cut.is_some()).id_salt("lyrics-edit-split")).clicked()
            && let Some((i, c)) = cut
        {
            d.split(i, c);
        }
        let next = sel.is_some_and(|i| i + 1 < d.lines.len());
        if ui.add(Button::new("Склеить").style(Style::Tonal).size(Size::S).enabled(next).id_salt("lyrics-edit-merge")).clicked()
            && let Some(i) = sel
        {
            d.merge(i);
        }
        if ui.add(Button::new("Строка ниже").icon(Icon::Add).style(Style::Tonal).size(Size::S).id_salt("lyrics-edit-add")).clicked() {
            d.add_below(sel);
        }
        if ui.add(IconButton::new(Icon::Delete).tooltip("Удалить строку").enabled(sel.is_some()).id_salt("lyrics-edit-delete")).clicked()
            && let Some(i) = sel
        {
            d.delete(i);
        }
    });
}

/// The lines in a card of `size`, each with its start.
fn list(ui: &mut Ui, size: egui::Vec2, d: &mut Draft, here: bool, pos: Duration, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, shape::LARGE, s.surface_container);
    let inner = rect.shrink2(vec2(8.0, 8.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)).id_salt("lyrics-edit-list"));
    child.set_clip_rect(rect.shrink(1.0).intersect(ui.clip_rect()));
    let now = if here { d.playing(pos) } else { None };
    let mut split = None;
    egui::ScrollArea::vertical().id_salt("lyrics-edit-scroll").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in 0..d.lines.len() {
            if let Some(at) = row(ui, d, i, now, here, out) {
                split = Some((i, at));
            }
        }
    });
    // Enter inside a line's text cuts it there.
    if let Some((i, at)) = split {
        d.split(i, at);
        d.focus = Some(i + 1);
    }
}

/// One line: its start (a click seeks there while the song plays) and its
/// text to edit. `Some(at)` when Enter was pressed inside the text at
/// character `at` (to cut the line there).
fn row(ui: &mut Ui, d: &mut Draft, i: usize, now: Option<usize>, here: bool, out: &mut Vec<Command>) -> Option<usize> {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let selected = d.selected == Some(i);
    if selected {
        ui.painter().rect_filled(rect, shape::MEDIUM, s.secondary_container);
        if std::mem::take(&mut d.scroll) {
            ui.scroll_to_rect(rect, Some(Align::Center));
        }
    }
    if resp.clicked() {
        d.select(i);
    }

    // The start: when it comes before a line above it, in the error colour.
    let start = d.lines[i].start;
    let time = Rect::from_min_size(rect.min, vec2(TIME_W, ROW));
    let seek = start.is_some() && here;
    let t = ui.interact(time, ui.id().with(("lyrics-edit-time", i)), if seek { Sense::click() } else { Sense::hover() });
    let t = if seek { t.on_hover_cursor(CursorIcon::PointingHand) } else { t };
    if t.clicked() {
        d.select(i);
        if let Some(at) = start {
            out.push(Command::Seek(at));
        }
    }
    let color = if d.early(i) {
        s.error
    } else if now == Some(i) {
        s.primary
    } else if start.is_some() {
        s.on_surface_variant
    } else {
        s.outline
    };
    let label = start.map_or_else(|| "—".to_owned(), clock);
    let galley = crate::typography::layout(ui.painter(), TypeRole::LabelLarge.style().job(label, color));
    ui.painter().galley(pos2(time.left() + 12.0, time.center().y - galley.size().y * 0.5), galley, color);

    // The text, edited in place; a background voice smaller.
    let cy = rect.center().y;
    let area = Rect::from_min_max(pos2(rect.left() + TIME_W, cy - 12.0), pos2((rect.right() - 12.0).max(rect.left() + TIME_W + 8.0), cy + 12.0));
    let (font, ink) =
        if d.lines[i].background() { (TypeRole::BodyMedium.font_id(), s.on_surface_variant) } else { (TypeRole::BodyLarge.font_id(), s.on_surface) };
    let edit_id = ui.id().with(("lyrics-edit-text", i));
    let mut child = ui.new_child(UiBuilder::new().max_rect(area));
    let edit = TextEdit::singleline(&mut d.lines[i].text)
        .id(edit_id)
        .frame(Frame::NONE)
        .margin(Margin::ZERO)
        .font(font)
        .text_color(ink)
        .desired_width(area.width())
        .vertical_align(Align::Center)
        .show(&mut child);
    let r = edit.response.response;
    if d.focus == Some(i) {
        r.request_focus();
        d.focus = None;
    }
    if r.gained_focus() || r.clicked() {
        d.select(i);
    }
    if r.has_focus()
        && let Some(c) = edit.cursor_range
    {
        d.cursor = Some((i, c.primary.index.0));
    }
    // Enter inside the text (not at either end) cuts the line there.
    let len = d.lines[i].text.chars().count();
    if r.lost_focus() && ui.input(|x| x.key_pressed(Key::Enter)) {
        return d.cursor.filter(|(at, c)| *at == i && *c > 0 && *c < len).map(|(_, c)| c);
    }
    None
}

/// «1:23,4»: a start to the tenth of a second.
fn clock(t: Duration) -> String {
    let ms = t.as_millis();
    format!("{}:{:02},{}", ms / 60_000, ms / 1000 % 60, ms % 1000 / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_read_to_the_tenth_of_a_second() {
        assert_eq!(clock(Duration::from_millis(83_456)), "1:23,4");
        assert_eq!(clock(Duration::from_millis(5_000)), "0:05,0");
    }
}
