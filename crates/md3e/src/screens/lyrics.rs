//! Honest syllable fill, word lift and sustained notes; line timing only focuses.
//! Both reading scales share frozen wrapping, scrolling and source selection.

use std::sync::Arc;
use std::time::Duration;

use egui::{Align2, Color32, Galley, Rect, Sense, Ui, pos2, text::LayoutJob, vec2};
use nikgob_motion::Spring;
use nikgob_view::{Command, LyricLine, LyricsSync, Prefs, Status, ViewState};

use super::lyrics_front::{self, Ease, Piece, Run};
use super::stage::{Part, Stage};
use crate::{
    Theme, TypeStyle,
    components::{Size, anim},
};

mod fade;
mod interlude;
mod layout;
mod preview;
mod timing;
#[cfg(test)]
mod tests;

use fade::*;
use interlude::*;
use layout::*;
use timing::*;

pub(crate) use preview::{countdown_preview, edges_preview, gap_preview, made_preview, preview, preview_at, tile_preview};

/// Both views share timing and wrapping; only their reading scale differs.
#[derive(Clone, Copy)]
struct Sizes {
    big: f32,
    small: f32,
    sub: f32,
    gap: f32,
    anchor: f32,
}

const PANE: Sizes = Sizes { big: 26.0, small: 20.0, sub: 15.0, gap: 18.0, anchor: 0.38 };
const MEGA: Sizes = Sizes { big: 44.0, small: 32.0, sub: 20.0, gap: 28.0, anchor: 0.4 };
/// How bright the not-yet-sung part of the current line gets at most.
const UNSUNG: f32 = 0.45;
/// Background vocals, relative to a main line.
const BG_SCALE: f32 = 0.72;

fn style_at(size: f32) -> TypeStyle {
    TypeStyle::new(size, (size * 1.28).round(), 600, 0.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Style {
    Karaoke,
    Focus,
    /// Each letter (or word) rises and lights up as it is sung.
    Wave,
}

/// What an animation goes over («Шаг подсветки»): what the light's front
/// keeps time by ([`Front`]) and what lights or jumps together.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum By {
    /// The letters of each timed piece, by their weights: a held vowel
    /// draws out.
    Letters,
    /// The timed pieces: the syllables of a text timed by syllables (the
    /// words of one timed by words), each crossed evenly over its own time.
    /// Under Focus a piece lights whole as it starts; in the wave its
    /// letters light across it, and the word rises with its syllables.
    Syllables,
    /// Whole written words (each part of a hyphenated one), each crossed
    /// evenly from its first sound to its last; outside karaoke the
    /// syllables of one light (and jump) together.
    Words,
}

/// An animation and what it goes over.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Look {
    style: Style,
    by: By,
}

impl Look {
    /// The look the settings ask for, whatever timing the text has.
    fn of(prefs: &Prefs) -> Self {
        let style = match prefs.lyrics_style {
            1 => Style::Focus,
            // 3 is the old «По словам» (moved once at start).
            2 | 3 => Style::Wave,
            _ => Style::Karaoke,
        };
        let by = match prefs.lyrics_unit {
            _ if prefs.lyrics_style == 3 => By::Words,
            nikgob_view::LYRICS_BY_LETTERS => By::Letters,
            nikgob_view::LYRICS_BY_WORDS => By::Words,
            _ => By::Syllables,
        };
        Look { style, by }
    }

    fn words(self) -> bool {
        self.by == By::Words
    }
}

/// Both views share timing and scrolling; true opens the lyrics settings.
/// The lyrics take the whole area given (less the mega view's control row):
/// the soft bands, or the plain fade with them off, lie inside it.
pub fn show(ui: &mut Ui, state: &ViewState, bass: f32, mega: bool, out: &mut Vec<Command>) -> bool {
    show_on(ui, state, bass, mega, &Stage::REAL, out)
}

/// [`show`] for the real view or the settings' stage ([`Stage`]): the copy
/// reads no pointer (its scene's hand scrolls it), asks for frames only
/// while it moves and notes where its line, its pause indicator, its
/// column and its soft bands are.
pub fn show_on(ui: &mut Ui, state: &ViewState, bass: f32, mega: bool, stage: &Stage, out: &mut Vec<Command>) -> bool {
    let look = style_for(&state.prefs, &state.lyrics.lines);
    let size = vec2(ui.available_width(), ui.available_height().max(80.0));
    let full = Rect::from_min_size(ui.cursor().min, size);
    ui.allocate_rect(full, Sense::hover());
    // The mega view's control has a row of its own under the lyrics: no
    // line ever lies under it.
    let (live, row) = if mega { control_row(full) } else { (full, None) };
    if mega && state.lyrics.lines.is_empty() {
        let left = plain(ui, live, state);
        return stage.live && row.is_some_and(|r| control_in(ui, r, left, state, out));
    }
    let resp = ui.interact(live, ui.id().with("lyrics-live"), Sense::click_and_drag());
    let mut sizes = if mega { MEGA } else { PANE };
    let scale = state.prefs.lyrics_scale.clamp(0.8, 1.4);
    sizes.big *= scale;
    sizes.small *= scale;
    sizes.sub *= scale;
    sizes.gap *= scale;
    paint(ui, live, &resp, state, &state.lyrics.lines, look, bass, sizes, mega, stage, out);
    // On the stage the control keeps its row but is not drawn: it notes
    // itself app-wide (for its menu and the screenshot runs).
    let Some(row) = row.filter(|_| stage.live) else { return false };
    let left = ui.ctx().data(|d| d.get_temp::<f32>(ui.id().with("lyrics-view").with("text-left")));
    control_in(ui, row, Some(left.unwrap_or(live.left() + 12.0)), state, out)
}

/// Air between the lyrics and the control's row under them.
const CONTROL_GAP: f32 = 12.0;

/// `area` less the control's row at its bottom (and the air above it),
/// and that row.
fn control_row(area: Rect) -> (Rect, Option<Rect>) {
    let h = Size::Xs.height();
    let row = Rect::from_min_max(pos2(area.left(), area.bottom() - h), area.max);
    let lyrics = Rect::from_min_max(area.min, pos2(area.right(), (row.top() - CONTROL_GAP).max(area.top())));
    (lyrics, Some(row))
}

/// The lyrics control in its row under the lyrics, its left edge in line
/// with the text's (`left`, else centred). True opens the lyrics settings.
fn control_in(ui: &mut Ui, row: Rect, left: Option<f32>, state: &ViewState, out: &mut Vec<Command>) -> bool {
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(row).id_salt("lyrics-control-edge"));
    super::lyrics_ui::control(&mut cui, state, left, out)
}

/// How `lines` are drawn: the look the settings ask for over real times
/// (syllables or words). A text with line times only has its words paced
/// from the line, which looks wrong under any look and unit, so with
/// «Спокойный Фокус» it becomes bright instead (the calm Focus, the line as
/// a whole; Focus by letters is that already).
fn style_for(prefs: &Prefs, lines: &[LyricLine]) -> Look {
    let guessed = LyricsSync::of(lines) == Some(LyricsSync::Line);
    if prefs.wave_syllables_only && guessed {
        return CALM;
    }
    Look::of(prefs)
}

/// The calm Focus: the line being sung becomes bright as a whole.
const CALM: Look = Look { style: Style::Focus, by: By::Letters };


/// Paints `lines` over `rect` (the view, clipped there; the soft bands
/// reach its edges), which `resp` takes the pointer over.
#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &Ui,
    rect: Rect,
    resp: &egui::Response,
    state: &ViewState,
    lines: &[LyricLine],
    look: Look,
    bass: f32,
    sizes: Sizes,
    mega: bool,
    stage: &Stage,
    out: &mut Vec<Command>,
) {
    let _p = crate::prof::scope("lyrics");
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let items = display_lines(lines);
    if items.is_empty() {
        return;
    }
    let painter = ui.painter_at(rect);
    let id = ui.id().with("lyrics-view");
    let now = anim::now(&ctx);
    let playing = state.player.status == Status::Playing;
    // The stage's copy plays a scene's own time, which runs as the scene
    // says: it is taken as it is.
    let heard = if stage.is_copy() { state.player.position_now() } else { clock(&ctx, id.with("clock"), &state.player, now, playing) };
    let pos = steady(&ctx, id.with("steady"), led(heard, state.prefs.lyrics_lead), playing);
    // Text is put on whole pixels where it rests and between them while it
    // moves ([`afloat`]).
    let ppp = painter.pixels_per_point();
    let to_px = |v: f32| (v * ppp).round() / ppp;
    // Each item's motion (its place, focus, wait) is kept under its slot,
    // which stays with the same line when the text is swapped for the same
    // song's (timing made, another source), so nothing moves or dims then.
    // A wait new to a swapped text opens from folded instead of popping
    // open; the first frame of the view shows its state as it is.
    let (slot_of, swapped) = slots(&ctx, id.with("slots"), &item_keys(lines, &items));
    let first_frame = swapped.is_none();
    // When the text's timing turns from line times only to real times or
    // back (made timing arrives, «По строкам» picked), the calm Focus and
    // the chosen look fade into each other instead of swapping at once.
    let (look_was, look_mix) = look_change(&ctx, id.with("look"), look, now);

    // Waiting owns the anchor during a break; otherwise follow the last
    // main line. Background voices light up independently while sung. A
    // clicked line is the current one from the click on, while its seek
    // comes back.
    let back = Duration::from_secs_f32((-state.prefs.lyrics_lead.clamp(-0.5, 0.5)).max(0.0));
    let jump_id = id.with("jump");
    let by_pos = current_item(lines, &items, pos);
    let (cur, jump) = with_jump(ctx.data(|d| d.get_temp::<Jump>(jump_id)), lines, &items, by_pos, pos, back, now);
    match jump {
        Some(j) => {
            ctx.data_mut(|d| d.insert_temp(jump_id, j));
            // Let go of it at the latest when it times out.
            stage.repaint_after(&ctx, Duration::from_secs_f64((j.at + JUMP_HOLD - now).max(0.0) + 0.01));
        }
        None => ctx.data_mut(|d| d.remove::<Jump>(jump_id)),
    }
    // While a click holds its line, everything is painted where the seek
    // will land (its point, with the lead): the clicked line starts as it
    // will go on and the old one keeps its state as it fades, instead of a
    // frame or more of the old position's singing on the new line. Once
    // the seek is back and the painted position is only coming up to its
    // point (a lead behind the sound), it is followed.
    let pos = jump_pos(jump, pos, back, state.prefs.lyrics_lead);
    let ci = cur.unwrap_or(0);
    let active = |i: usize| {
        let l = &lines[i];
        if l.background {
            l.start <= pos && pos < l.end.unwrap_or(l.start + Duration::from_secs(5))
        } else {
            cur.is_some_and(|ci| items[ci] == DisplayLine::Line(i))
        }
    };
    // Slots are laid out at the big size, so nothing moves when a line
    // grows; the drawn size flows inside its slot.
    let wrap = (rect.width() - 24.0).clamp(1.0, 900.0);
    let sub_layout = |text: &str| -> Arc<Galley> {
        let mut job = LayoutJob::single_section(text.to_owned(), style_at(sizes.sub).text_format(Color32::WHITE));
        job.wrap.max_width = wrap;
        crate::typography::layout(&painter, job)
    };
    let scale = |l: &LyricLine| if l.background { BG_SCALE } else { 1.0 };
    let breaks: Vec<_> = lines.iter().enumerate().map(|(i, l)| {
        fixed_breaks(&painter, id.with(("breaks", i)), &l.text, wrap, sizes.big * scale(l))
    }).collect();
    let slots: Vec<(f32, f32)> = items
        .iter()
        .map(|item| {
            let DisplayLine::Line(i) = *item else {
                return (wait_height(&state.prefs) * sizes.big / PANE.big, 0.0);
            };
            let l = &lines[i];
            let main = breaks[i].height;
            let extra = sub(l).map_or(0.0, |t| sub_layout(t).size().y + 4.0);
            (main, extra)
        })
        .collect();
    // Mega mode: a song of short lines would hug the column's left edge
    // and leave most of it empty, so the block moves towards the middle.
    let block = if mega {
        let widest = lines
            .iter()
            .enumerate()
            .map(|(i, l)| breaks[i].width.max(sub(l).map_or(0.0, |t| sub_layout(t).size().x)))
            .fold(0.0, f32::max);
        // Another text moves the block to its new place on the spatial
        // spring instead of jumping there; a resize (another wrap) moves
        // it at once, with the view.
        let (widest_id, wrap_id) = (id.with("widest"), id.with("widest-wrap"));
        if ctx.data(|d| d.get_temp::<f32>(wrap_id)) != Some(wrap) {
            anim::snap_f32(&ctx, widest_id, widest);
            ctx.data_mut(|d| d.insert_temp(wrap_id, wrap));
        }
        // Its overshoot is kept, so it settles instead of stopping dead
        // at the full width.
        anim::f32(&ctx, widest_id, widest, BLOCK).clamp(0.0, wrap * 1.1)
    } else {
        wrap
    };
    let left = rect.left() + 12.0 + (wrap - block) * 0.5;
    // The control on the lyrics' bottom edge lines up with the text.
    ctx.data_mut(|d| d.insert_temp(id.with("text-left"), left));
    let wrap = block;
    let gap = sizes.gap;
    let mut tops = Vec::with_capacity(slots.len());
    let mut rooms = Vec::with_capacity(slots.len());
    let mut y = 0.0;
    for (i, (h, extra)) in slots.iter().enumerate() {
        // Background vocals hug the line they belong to.
        if matches!(items[i], DisplayLine::Line(j) if lines[j].background) && i > 0 {
            y -= to_px(gap * 0.6);
        }
        tops.push(y);
        // A wait only takes room while it is happening: it unfolds when it
        // starts and folds away when the singing comes back; one new to a
        // swapped text that is happening already unfolds too (its spring
        // would start where it goes).
        if !first_frame && matches!(items[i], DisplayLine::Interlude { .. }) {
            anim::fraction_starts_at(&ctx, id.with(("wait-room", slot_of[i])), 0.0);
        }
        let keep = match items[i] {
            // On Material's expressive spatial spring: the lines part (and
            // close again) in one soft move, its small overshoot and all
            // (cut off at open and at folded, the lines stopped dead there
            // instead of settling).
            DisplayLine::Interlude { .. } => anim::fraction(
                &ctx,
                id.with(("wait-room", slot_of[i])),
                if cur == Some(i) { 1.0 } else { 0.0 },
                if theme.reduced_motion { WAIT_ROOM.without_bounce() } else { WAIT_ROOM },
            )
            .clamp(-ROOM_OVERSHOOT, 1.0 + ROOM_OVERSHOOT),
            DisplayLine::Line(_) => 1.0,
        };
        // Whole pixels from one item to the next, so with the text at rest
        // every line lies on them; a wait's room moves between them.
        y += to_px(h + extra + gap) * keep;
        // The wait itself (its ring, highlight and pointer) takes the room
        // it has, within its slot.
        rooms.push(keep.clamp(0.0, 1.0));
    }

    // Where the current line rests: at its share of the whole view, with the
    // soft edges always clear of the top band.
    let soft = Soft::of(&state.prefs).text(sizes.small * 1.28 + sizes.gap, sizes.small);
    // The soft edges fade the text over a band at the top and the bottom
    // of what is seen of the view (a view taller than its clip, as in the
    // settings' scene, was cut off by the clip before its band came); the
    // middle stays sharp and no line inside the view is left out.
    let seen = rect.intersect(painter.clip_rect());
    let seen = if seen.is_positive() { seen } else { rect };
    let fade = state.prefs.lyrics_soft_edges.then(|| Fade::of(seen, soft));
    if stage.is_copy() {
        let band = if state.prefs.lyrics_soft_edges { soft.band(seen) } else { 72.0 };
        stage.part(&ctx, Part::LyricsColumn, Rect::from_x_y_ranges(left - 12.0..=left + wrap + 12.0, seen.y_range()));
        stage.part(&ctx, Part::LyricsEdges, Rect::from_min_max(seen.min, pos2(seen.right(), seen.top() + band)));
        stage.part(&ctx, Part::LyricsEdges, Rect::from_min_max(pos2(seen.left(), seen.bottom() - band), seen.max));
    }
    let anchor = rest_at(rect, sizes.anchor, state.prefs.lyrics_soft_edges.then_some(soft), slots[ci].0 * 0.5, sizes.big * 0.4);
    let follow = tops[ci] + slots[ci].0 * 0.5 - anchor;

    // Keep the user's offset, then ease it back over half a second.
    let manual_id = id.with("manual");
    let mut dy = if !stage.live {
        // The stage's scene scrolls it, not the pointer.
        stage.hand
    } else if resp.dragged() {
        ctx.input(|i| i.pointer.delta().y)
    } else {
        0.0
    };
    if stage.live && resp.hovered() {
        dy += ctx.input(|i| i.smooth_scroll_delta.y);
        ctx.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
    }
    // Scrolling stops at the text's ends: the first line (or wait) with
    // room or the last one at the anchor, never into empty space.
    let centre = |i: usize| tops[i] + slots[i].0 * 0.5;
    let first = rooms.iter().position(|r| *r >= ROOMLESS).unwrap_or(0);
    let last = tops.len().saturating_sub(1);
    let manual = ctx.data(|d| d.get_temp::<Manual>(manual_id)).unwrap_or_default().step(
        follow,
        dy,
        now,
        f64::from(state.prefs.lyrics_return_s.max(0.0)),
        (centre(first) - centre(ci), centre(last) - centre(ci)),
    );
    let manual_on = manual.on(now);
    ctx.data_mut(|d| d.insert_temp(manual_id, manual));
    if manual_on {
        stage.repaint_after(&ctx, Duration::from_secs_f64((manual.until - now).max(0.0)));
    } else if manual.offset != 0.0 && now < manual.until + 0.5 {
        stage.repaint(&ctx);
    }
    let scroll = follow + manual.shift(now);
    // A text swapped for another (the same song's from another source,
    // timing made, a wait more or less) is laid out anew at once: the
    // text's spring is moved by as much as the line it was on moved, so
    // that line stays where it is and only the rest of the way is
    // animated, instead of the whole text jumping there and drifting back.
    let held_id = id.with("held");
    if swapped == Some(true)
        && !state.prefs.lyrics_cascade
        && !manual_on
        && let Some(moved) = held_moved(ctx.data(|d| d.get_temp::<(u64, f32)>(held_id)), &slot_of, &tops)
    {
        anim::shift_f32(&ctx, id.with("scroll"), moved);
    }
    ctx.data_mut(|d| d.insert_temp(held_id, (slot_of[ci], tops[ci])));
    // When the view moves or resizes (the window, the layout's span, a card
    // that rises in), the text goes with it at once, kept at its share of
    // the height: the springs are moved with their frame instead of chasing
    // it, so only the text's own moves are animated.
    let frame = (rect.top(), rect.height() * sizes.anchor);
    let frame_id = id.with("frame");
    if let Some(was) = ctx.data(|d| d.get_temp::<(f32, f32)>(frame_id))
        && was != frame
    {
        let (top, anchor) = (frame.0 - was.0, frame.1 - was.1);
        if state.prefs.lyrics_cascade {
            for i in 0..items.len() {
                anim::shift_f32(&ctx, id.with(("y", slot_of[i])), top + anchor);
            }
        } else {
            anim::shift_f32(&ctx, id.with("scroll"), -anchor);
        }
    }
    ctx.data_mut(|d| d.insert_temp(frame_id, frame));

    // A scroll by hand follows the hand as closely in both modes; the
    // text's own moves take the calmer spring. The springs go to whole
    // pixels: the text comes to rest sharp without a last step onto them
    // (drawn to the pixel it stepped all the way, most visibly as it slowed).
    let together = if !state.prefs.lyrics_cascade {
        let rest = rect.top() - to_px(rect.top() - scroll);
        anim::f32(&ctx, id.with("scroll"), rest, if manual_on { HAND } else { FOLLOW })
    } else { scroll };

    // Where each item is drawn this frame.
    let ys: Vec<f32> = (0..items.len())
        .map(|i| {
            let far = (i as i32 - ci as i32).unsigned_abs().min(8) as f32;
            if state.prefs.lyrics_cascade {
                let spring = if manual_on { HAND } else { follow_spring(far) };
                anim::f32(&ctx, id.with(("y", slot_of[i])), to_px(rect.top() + tops[i] - scroll), spring)
            } else {
                rect.top() + tops[i] - together
            }
        })
        .collect();
    // How far each item is from the current one, eased with the focus as
    // the current line changes: the lines' resting brightness follows it,
    // where every other line stepped brighter or dimmer at once the frame
    // the next line came in, right as the lines began to move. Kept for
    // items out of view too, so one scrolled back in is where it should be.
    let aways: Vec<f32> = (0..items.len())
        .map(|i| anim::f32(&ctx, id.with(("away", slot_of[i])), (i as f32 - ci as f32).clamp(-9.0, 9.0), AWAY))
        .collect();
    let shown = |i: usize| {
        let (slot_h, extra) = slots[i];
        !(ys[i] + slot_h + extra < rect.top() - 60.0 || ys[i] > rect.bottom() + 60.0)
    };
    // One item at most is under the pointer: a folded wait has no room to
    // be hovered in, an unfolding one only the room it has.
    let item_at = |p: Option<egui::Pos2>| {
        p.filter(|p| (left - 8.0..=left + wrap + 8.0).contains(&p.x)).and_then(|p| {
            let hits: Vec<_> = (0..items.len())
                .map(|i| shown(i).then(|| (ys[i], (slots[i].0 + slots[i].1) * rooms[i], rooms[i])))
                .collect();
            hit_target(&hits, p.y, gap * 0.5)
        })
    };
    let target = item_at(resp.hover_pos());
    // A tap on a touch screen ends with the pointer gone, so nothing is
    // hovered in the frame of its click: the click is placed where it was.
    let clicked = if stage.live && resp.clicked() { item_at(resp.interact_pointer_pos().or(resp.hover_pos())) } else { None };
    for (i, item) in items.iter().enumerate() {
        let y = ys[i];
        let (slot_h, extra) = slots[i];
        let room = rooms[i];
        if !shown(i) || room < ROOMLESS {
            continue;
        }
        let is_cur = match *item {
            DisplayLine::Line(j) => active(j),
            DisplayLine::Interlude { .. } => cur == Some(i),
        };
        // 0 → 1 as the line becomes the current one, and back.
        let focus = anim::fraction(
            &ctx,
            id.with(("focus", slot_of[i])),
            if is_cur { 1.0 } else { 0.0 },
            Spring::new(1.0, 170.0),
        )
        .clamp(0.0, 1.0);
        // The sung part's colour comes in faster than the line's focus, so
        // the first word lights on its beat instead of easing in with it.
        let shine = anim::fraction(&ctx, id.with(("shine", slot_of[i])), if is_cur { 1.0 } else { 0.0 }, nikgob_motion::tokens::FAST_EFFECTS)
            .clamp(0.0, 1.0);
        let duet = match *item {
            DisplayLine::Line(j) => lines[j].duet,
            DisplayLine::Interlude { duet, .. } => duet,
        };
        // The second voice of a duet sings from the other side.
        let side = |w: f32| {
            if duet {
                left + (wrap - w).max(0.0)
            } else {
                left
            }
        };
        // Towards the edges the text fades out. With the soft edges its
        // colour thins by height where it is drawn ([`fade_galley`]), so
        // `edge` leaves the line whole; without them the line fades as one.
        let edge = match fade {
            Some(_) => 1.0,
            None => ((y + slot_h - seen.top()) / 72.0).min((seen.bottom() - y) / 72.0).clamp(0.0, 1.0),
        };
        // What has no height of its own to fade over (the hover's layer, a
        // wait's ring) takes the fade at its middle.
        let whole = fade.map_or(edge, |f| f.at(y + (slot_h + extra) * room * 0.5));
        // A wait that is unfolding or folding has only part of its slot:
        // its highlight and ring take that part and fade with it.
        let slot = Rect::from_min_size(pos2(left, y), vec2(wrap, (slot_h + extra) * room));
        // The hover's state layer fades in and out like any M3 one.
        let hover = anim::fraction(
            &ctx,
            id.with(("hover", slot_of[i])),
            if target == Some(i) { 1.0 } else { 0.0 },
            nikgob_motion::tokens::FAST_EFFECTS,
        )
        .clamp(0.0, 1.0);
        if hover > 0.001 {
            painter.rect_filled(slot.expand2(vec2(10.0, 6.0 * room)), 14, s.on_surface.gamma_multiply(0.06 * whole * room * hover));
        }
        let DisplayLine::Line(j) = *item else {
            if let DisplayLine::Interlude { start, end, .. } = *item {
                let k = sizes.big / PANE.big * state.prefs.lyrics_gap_size.clamp(0.8, 1.5) * room;
                let line = Rect::from_min_size(pos2(side(56.0 * k), y), vec2(56.0 * k, slot_h * room));
                if is_cur {
                    stage.part(&ctx, Part::LyricsPause, line);
                }
                // Never over its neighbours, wherever their springs are.
                let above = if i > 0 { ys[i - 1] + slots[i - 1].0 + slots[i - 1].1 } else { f32::NEG_INFINITY };
                let below = ys.get(i + 1).copied().unwrap_or(f32::INFINITY);
                let room_rect = Rect::from_x_y_ranges(
                    rect.x_range(),
                    (y - gap * 0.5 * room).max(above)..=(y + (slot_h + gap * 0.5) * room).min(below),
                );
                let clipped = painter.with_clip_rect(room_rect.intersect(painter.clip_rect()));
                // The ring comes once its room has begun to open, and goes
                // before the room closes (see [`waiting`]).
                let appear = anim::fraction(
                    &ctx,
                    id.with(("wait-ring", slot_of[i])),
                    if is_cur && room > 0.3 { 1.0 } else { 0.0 },
                    if theme.reduced_motion { WAIT_RING.without_bounce() } else { WAIT_RING },
                )
                .max(0.0);
                waiting(&clipped, line, k, start, end, pos, appear, playing, bass, whole, &state.prefs);
            }
            continue;
        };
        let l = &lines[j];
        let rest_sizes = (sizes.small * scale(l), sizes.big * scale(l));
        let size = rest_sizes.0 + (rest_sizes.1 - rest_sizes.0) * focus;
        let (small, big) = (fixed_layout(&painter, &breaks[j].forced, rest_sizes.0), fixed_layout(&painter, &breaks[j].forced, rest_sizes.1));
        // Between the two resting sizes the size moves every frame.
        let g = if focus <= 0.0 {
            small.clone()
        } else if focus >= 1.0 {
            big.clone()
        } else {
            moving_layout(&painter, &breaks[j].forced, rest_sizes, focus)
        };
        // In the middle of its slot: on whole pixels at either resting
        // size and evenly from one to the other, so the line is sharp where
        // it rests and never steps on its way there.
        let inset = |g: &Galley| to_px((slot_h - g.size().y) * 0.5);
        let inset = inset(&*small) + (inset(&*big) - inset(&*small)) * focus;
        let line = Rect::from_min_size(pos2(side(g.size().x), y + inset), g.size());
        if is_cur && !l.background {
            stage.part(&ctx, Part::LyricsLine, line);
        }
        // Brightness of a line at rest: past lines a bit brighter than the
        // ones to come; focus mode pushes the rest far back (from the look
        // before as the look changes), by how far the line is from the
        // current one as eased ([`AWAY`]).
        let rest_in = |style: Style| rest_brightness(style, aways[i]);
        let mut rest = match look_was {
            Some(was) => rest_in(was.style) + (rest_in(look.style) - rest_in(was.style)) * look_mix,
            None => rest_in(look.style),
        };
        if l.background {
            rest *= 0.8;
        }
        rest = 1.0 - (1.0 - rest) * state.prefs.lyrics_dim.clamp(0.0, 1.0);
        let dim = s.on_surface.gamma_multiply(edge * (rest + (1.0 - rest) * focus * 0.4));
        let lit = s.on_surface.gamma_multiply(edge * if l.background { 0.8 } else { 1.0 });

        if let Some(t) = sub(l) {
            let sg = sub_layout(t);
            let c = crate::lerp_color(dim, lit, focus * 0.6).gamma_multiply(0.8);
            fade_galley(&painter, pos2(side(sg.size().x), y + to_px(slot_h + 4.0)), &sg, c, fade);
        }
        if !is_cur && focus < 0.01 {
            fade_galley(&painter, line.min, &g, dim, fade);
            continue;
        }
        // Where the line is sung up to: the position while it is current;
        // once it is not, it goes on being sung as it fades (a front or a
        // jump mid-word when the next line comes in finishes instead of
        // jumping to the end), held where it was after a seek.
        let sung_id = id.with(("sung", slot_of[i]));
        let sung = if is_cur { pos } else { sung_pos(ctx.data(|d| d.get_temp::<Duration>(sung_id)), pos) };
        ctx.data_mut(|d| d.insert_temp(sung_id, sung));
        // A line-synced line is sung at a speaking pace (the source label
        // says «по строкам», so it's not passed off as real timing).
        let paced;
        let timed = if l.words.is_empty() {
            let next = lines[j + 1..].iter().find(|n| !n.background).map(|n| n.start);
            paced = paced_words(l, next);
            &paced
        } else {
            l
        };
        let pace = Pace::of(&state.prefs);
        let Some(timing) = timing_of(&ctx, timed, &g, look, pace, size) else {
            if state.prefs.lyrics_glow {
                glow(&painter, &g, line, lit, focus * 0.1, fade);
            }
            fade_galley(&painter, line.min, &g, crate::lerp_color(dim, lit, focus), fade);
            continue;
        };
        // The not-yet-sung part eases with focus from the line's resting
        // brightness to a darker tone (for contrast with the sung part): a
        // fixed tone here made a line dip darker the moment it became
        // current, and jump back when it stopped being so.
        let base = s.on_surface.gamma_multiply(edge * (rest + (rest.min(UNSUNG) - rest) * focus));
        let color = crate::lerp_color(dim, lit, focus.max(shine));
        // Focus draws the whole line in that colour, sung or not: there it
        // eases in with the line's focus (as fast as the sung part's, the
        // line went bright in a few frames, well before it had grown).
        let calm = crate::lerp_color(dim, lit, focus);
        // Sung pieces stay lit (and wave pieces up) in full while the line
        // is current and go back with its focus after it; a line left
        // before its focus was whole goes on from full all the same.
        let hold = if is_cur { 1.0 } else { focus.max(shine) };
        let draw = |painter: &egui::Painter, look: Look, timing: &Timing| {
            let color = if look.style == Style::Focus { calm } else { color };
            // Reduced motion keeps everything down: karaoke by letters
            // still fills along its front (no word rises), the wave lights
            // its letters one by one without rising, and focus lights the
            // whole line.
            if theme.reduced_motion && look.by == By::Letters && look.style == Style::Focus {
                if state.prefs.lyrics_glow {
                    glow(painter, &g, line, lit, focus * 0.1, fade);
                }
                fade_galley(painter, line.min, &g, color, fade);
            } else {
                timed_line(painter, &g, line, timing, sung, focus, hold, size, look, base, color, &state.prefs, fade);
            }
        };
        match look_was.and_then(|was| Some((was, timing_of(&ctx, timed, &g, was, pace, size)?))) {
            // The look before goes as the new one comes, the line about as
            // opaque as ever between them (see [`look_fades`]).
            Some((was, before)) => {
                let (out, inn) = look_fades(look_mix);
                let mut old = painter.clone();
                old.multiply_opacity(out);
                draw(&old, was, &before);
                let mut new = painter.clone();
                new.multiply_opacity(inn);
                draw(&new, look, &timing);
            }
            None => draw(&painter, look, &timing),
        }
    }
    // A click goes to its line (a wait: to just before the singing): the
    // song seeks there, the line becomes the current one at once and the
    // view settles on it as for any new line; the user's scroll ends.
    if let Some(i) = clicked {
        let to = seek_to(lines, items[i]);
        out.push(Command::Seek(to));
        ctx.data_mut(|d| {
            d.insert_temp(jump_id, Jump { item: items[i], to, at: now });
            d.insert_temp(manual_id, Manual::default());
        });
        ctx.request_repaint();
    }
    if stage.live && resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if playing {
        stage.repaint(&ctx);
    }
}

/// How far a text laid out anew moved the item the view was on (`held`:
/// its slot and top the frame before), by the items' slots and `tops` now;
/// `None` when it is not there any more.
fn held_moved(held: Option<(u64, f32)>, slot_of: &[u64], tops: &[f32]) -> Option<f32> {
    let (slot, top) = held?;
    let i = slot_of.iter().position(|s| *s == slot)?;
    Some(tops.get(i)? - top)
}

/// The spring the text follows a scroll by hand on.
const HAND: Spring = Spring::new(1.0, 1600.0);

/// The spring the text follows the current line on: that of a line's focus
/// ([`AWAY`]), so the line comes to its place as it grows and lights, in
/// one move and without swinging past it.
const FOLLOW: Spring = Spring::new(1.0, 170.0);

/// [`FOLLOW`] for a line `far` lines from the current one, when each line
/// moves on its own: further ones follow more lazily, none past its place.
fn follow_spring(far: f32) -> Spring {
    Spring::new(1.0, (FOLLOW.stiffness - 14.0 * far.max(0.0)).max(70.0))
}

/// The spring a line's distance from the current one eases on (that of
/// its focus, so its brightness moves with the rest of the change).
const AWAY: Spring = Spring::new(1.0, 170.0);

/// How bright a line at rest is under `style`, `away` lines from the
/// current one (before it when negative; fractions between whole lines
/// while the current line changes): lines sung already a bit brighter
/// than the ones to come; focus pushes the rest far back.
fn rest_brightness(style: Style, away: f32) -> f32 {
    let far = away.abs().min(8.0);
    match style {
        Style::Focus => 0.62f32.powf(far).max(0.08) * 0.7,
        _ => (0.42 + 0.08 * (-away).clamp(0.0, 1.0)) * 0.86f32.powf((far - 1.0).max(0.0)),
    }
}

/// The spring mega mode's block of text moves to its new middle on when
/// the text changes.
const BLOCK: Spring = nikgob_motion::tokens::EXPRESSIVE_DEFAULT_SPATIAL;

/// The effects spring one look fades into another on.
const LOOK_FADE: Spring = nikgob_motion::tokens::SLOW_EFFECTS;

/// A change of look ([`look_change`]): the look now, the one before and
/// when it changed.
#[derive(Clone, Copy, Debug)]
struct LookChange {
    now: Look,
    was: Look,
    at: f64,
}

/// The look before `look` while the view fades from it (with how far, 0..1,
/// the fade is), else `None` and 1.
fn look_change(ctx: &egui::Context, id: egui::Id, look: Look, now: f64) -> (Option<Look>, f32) {
    let c = match ctx.data(|d| d.get_temp::<LookChange>(id)) {
        Some(c) if c.now == look => c,
        // A first frame shows its look as it is.
        None => LookChange { now: look, was: look, at: f64::NEG_INFINITY },
        Some(c) => LookChange { now: look, was: c.now, at: now },
    };
    ctx.data_mut(|d| d.insert_temp(id, c));
    let mix = look_mix(c.was, c.now, (now - c.at) as f32);
    if mix < 1.0 {
        ctx.request_repaint();
        (Some(c.was), mix)
    } else {
        (None, 1.0)
    }
}

/// How far (0..1) a fade from look `was` to `now` is `t` seconds after the
/// change: on [`LOOK_FADE`], done once it is within a thousandth.
fn look_mix(was: Look, now: Look, t: f32) -> f32 {
    if was == now || !t.is_finite() {
        return 1.0;
    }
    let mix = (1.0 - LOOK_FADE.solve(1.0, 0.0, t.max(0.0)).0).clamp(0.0, 1.0);
    if mix > 0.999 { 1.0 } else { mix }
}

/// The opacity of the look going and of the one coming at `mix`: the one
/// going stays nearly whole until the new one has come in mostly, so the
/// line (one drawn over the other) never thins out between them.
fn look_fades(mix: f32) -> (f32, f32) {
    let m = mix.clamp(0.0, 1.0);
    (1.0 - m.powi(3), 1.0 - (1.0 - m).powi(3))
}

/// Below this share of its slot a folding wait counts as gone: it takes no
/// pointer and draws nothing.
const ROOMLESS: f32 = 0.01;

/// How far past open or folded a wait's room may swing (its spring's own
/// overshoot is a few hundredths); only a bound for safety.
const ROOM_OVERSHOOT: f32 = 0.1;
/// Unsynced text still gets a readable column in `rect`, with its source
/// kept outside the scroll area (the control in its row under it, in line
/// with the text) so changing providers never requires scrolling to the
/// end. Returns the text's left edge (none for the centred «Текста нет»).
fn plain(ui: &mut Ui, rect: Rect, state: &ViewState) -> Option<f32> {
    let s = Theme::get(ui.ctx()).scheme;
    let column = Rect::from_center_size(rect.center(), vec2(rect.width().min(900.0), rect.height()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(column).id_salt("mega-plain"));
    if let Some(text) = state.lyrics.plain.as_deref().filter(|t| !t.trim().is_empty()) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            ui.label(style_at(MEGA.small * state.prefs.lyrics_scale.clamp(0.8, 1.4)).rich_para(text, s.on_surface));
            ui.add_space(16.0);
        });
    } else {
        let text = if state.lyrics.loading { nikgob_core::i18n::lyrics().searching } else { nikgob_core::i18n::lyrics().no_text };
        child.painter().text(rect.center(), Align2::CENTER_CENTER, text, style_at(MEGA.small).font_id(), s.on_surface_variant);
    }
    let text = state.lyrics.plain.as_deref().is_some_and(|t| !t.trim().is_empty());
    text.then_some(column.left())
}
