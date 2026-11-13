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

/// `lines` drawn as the lyrics view draws the current text, smaller (the
/// whole text behind the lyrics control), following the playing position;
/// a click seeks.
pub(crate) fn made_preview(ui: &mut Ui, size: egui::Vec2, state: &ViewState, lines: &[LyricLine], out: &mut Vec<Command>) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let look = style_for(&state.prefs, lines);
    // «Размер текста» too, as in the views.
    let scale = state.prefs.lyrics_scale.clamp(0.8, 1.4);
    let mut sizes = PANE;
    sizes.big = 22.0 * scale;
    sizes.small = 17.0 * scale;
    sizes.sub = 13.0 * scale;
    sizes.gap = 14.0 * scale;
    paint(ui, rect, &resp, state, lines, look, 0.0, sizes, false, &Stage::REAL, out);
}

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

/// The soft edges' settings: how deep each band reaches (a share of the
/// view's height, «Глубина»), how early a line fades in it («Затухание»)
/// and how much it blurs right at the edge («Размытие у края»). With the
/// text it is for ([`Soft::text`]) the band never takes less than about a
/// line and the blur goes with the size of the letters, so a smaller copy
/// of the view (the settings' scene) looks the same.
#[derive(Clone, Copy, Debug)]
struct Soft {
    depth: f32,
    fade: f32,
    /// «Размытие у края», 0..1.5.
    strength: f32,
    /// From one line to the next, px: the least band.
    pitch: f32,
    /// The text's size, px: what the blur is measured by.
    size: f32,
}

/// A band takes at least this many lines, so one always rolls through it.
const BAND_LINES: f32 = 1.25;
/// The blur's reach right at the edge at full strength, in text sizes.
const EDGE_BLUR: f32 = 0.16;
/// Below this reach (px) a line is not blurred at all.
const BLUR_MIN: f32 = 0.35;

impl Soft {
    fn of(p: &Prefs) -> Self {
        Self {
            depth: p.lyrics_soft_depth.clamp(0.08, 0.4),
            fade: p.lyrics_soft_fade.clamp(0.0, 1.0),
            strength: p.lyrics_soft_strength.clamp(0.0, 1.5),
            pitch: 0.0,
            size: 0.0,
        }
    }

    /// For text `size` px big, its lines `pitch` px apart.
    fn text(self, pitch: f32, size: f32) -> Self {
        Self { pitch: pitch.max(0.0), size: size.max(0.0), ..self }
    }

    /// How deep the band at each edge of `rect` reaches: its share of the
    /// height, at least about a line, never more than 0.42 of it.
    fn band(self, rect: Rect) -> f32 {
        let h = rect.height().max(0.0);
        (h * self.depth).max(self.pitch * BAND_LINES).max(12.0).min(h * 0.42)
    }
}

/// Where the current line's middle rests, down from `rect`'s top: `share`
/// of its height; with the soft edges (`soft`) never so high that the line
/// (`half` its height, its ink `ink` beyond it) reaches into the top band,
/// though never below the middle.
fn rest_at(rect: Rect, share: f32, soft: Option<Soft>, half: f32, ink: f32) -> f32 {
    let at = rect.height() * share;
    match soft {
        Some(soft) => at.max((soft.band(rect) + ink + half).min(rect.height() * 0.5)),
        None => at,
    }
}

/// The soft edges of a view from `top` to `bottom` (on screen, where it is
/// seen): the text is whole and sharp in the middle and fades over a
/// `band` at either edge, to nothing at the edge itself, so lines come in
/// and go out through it with no line where they are cut. A mask over the
/// view by height, on each glyph's own vertices: no picture of the text,
/// nothing drawn into a texture. Close to the edge the letters also blur a
/// little ([`Fade::blur_at`]); the middle never does.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fade {
    top: f32,
    bottom: f32,
    band: f32,
    /// How the opacity rises over the band ([`Fade::at`]): less keeps the
    /// text brighter until nearer the edge, more fades it from further in.
    gamma: f32,
    /// How far (px) the letters spread right at the edge.
    blur: f32,
}

impl Fade {
    fn of(rect: Rect, soft: Soft) -> Self {
        Fade {
            top: rect.top(),
            bottom: rect.bottom(),
            band: soft.band(rect),
            gamma: 0.6 + 1.4 * soft.fade,
            blur: EDGE_BLUR * soft.size * soft.strength,
        }
    }

    /// How far into the band (0 at the edge .. 1 at its inner end and
    /// beyond) height `y` is.
    fn depth_at(self, y: f32) -> f32 {
        let room = (y - self.top).min(self.bottom - y);
        (room / self.band.max(1.0)).clamp(0.0, 1.0)
    }

    /// How opaque (0..1) the text is at height `y` on screen: an S-curve
    /// over the band (no step at either end of it), shaped by «Затухание».
    fn at(self, y: f32) -> f32 {
        let f = self.depth_at(y);
        if f >= 1.0 {
            return 1.0;
        }
        smoothstep(f).powf(self.gamma)
    }

    /// How far (px) the letters spread at height `y`: nothing in the inner
    /// half of the band and in the middle, growing towards the edge.
    fn blur_at(self, y: f32) -> f32 {
        let near = (1.0 - self.depth_at(y) * 2.0).max(0.0);
        self.blur * near * near
    }

    /// Whether everything from `y0` down to `y1` is clear of both bands.
    fn clear(self, y0: f32, y1: f32) -> bool {
        y0 - self.top >= self.band && self.bottom - y1 >= self.band
    }
}

/// The directions of a blurred glyph's copies around it.
const BLUR_TAPS: [egui::Vec2; 8] = [
    vec2(1.0, 0.0),
    vec2(0.7071, 0.7071),
    vec2(0.0, 1.0),
    vec2(-0.7071, 0.7071),
    vec2(-1.0, 0.0),
    vec2(-0.7071, -0.7071),
    vec2(0.0, -1.0),
    vec2(0.7071, -0.7071),
];
/// How much of a glyph's opacity goes to its copies where the blur is
/// full (at the very edge); the rest stays on the glyph.
const BLUR_SPREAD: f32 = 0.75;

/// `g` at `pos` with each glyph vertex in `color` (its own colour when
/// `None`), thinned by `fade` at its height on screen. The glyphs are the
/// galley's own, so the text is as sharp as ever; a row near the edge also
/// gets copies of its glyphs spread around them by [`Fade::blur_at`] (on
/// the same mesh, the opacity shared between the glyph and its copies, so
/// the row is about as opaque as it would be sharp).
fn faded(g: &Galley, pos: egui::Pos2, color: Option<Color32>, fade: Fade) -> Arc<Galley> {
    let mut out = g.clone();
    let mut reach_all = 0.0_f32;
    for placed in &mut out.rows {
        let top = pos.y + placed.pos.y;
        let mut row = (*placed.row).clone();
        let glyphs = row.visuals.glyph_vertex_range.clone();
        let mesh = &mut row.visuals.mesh;
        let Some(vertices) = mesh.vertices.get(glyphs.clone()) else {
            placed.row = Arc::new(row);
            continue;
        };
        let reach = vertices.iter().map(|v| fade.blur_at(top + v.pos.y)).fold(0.0, f32::max);
        if reach < BLUR_MIN {
            for v in &mut mesh.vertices[glyphs] {
                v.color = color.unwrap_or(v.color).gamma_multiply(fade.at(top + v.pos.y));
            }
            placed.row = Arc::new(row);
            continue;
        }
        reach_all = reach_all.max(reach);
        // The glyphs' triangles, to copy with their vertices.
        let tris: Vec<u32> = mesh
            .indices
            .chunks_exact(3)
            .filter(|t| t.iter().all(|&i| glyphs.contains(&(i as usize))))
            .flatten()
            .copied()
            .collect();
        let base: Vec<egui::epaint::Vertex> = mesh.vertices[glyphs.clone()].to_vec();
        // The glyph's own share of its opacity and each copy's.
        let share = |y: f32| {
            let spread = BLUR_SPREAD * (fade.blur_at(y) / fade.blur.max(1e-3)).clamp(0.0, 1.0);
            (1.0 - spread, spread / BLUR_TAPS.len() as f32)
        };
        for v in &mut mesh.vertices[glyphs.clone()] {
            let y = top + v.pos.y;
            v.color = color.unwrap_or(v.color).gamma_multiply(fade.at(y) * share(y).0);
        }
        mesh.vertices.reserve(base.len() * BLUR_TAPS.len());
        mesh.indices.reserve(tris.len() * BLUR_TAPS.len());
        for dir in BLUR_TAPS {
            let first = mesh.vertices.len() as u32;
            for v in &base {
                let y = top + v.pos.y;
                let mut c = *v;
                c.pos += dir * fade.blur_at(y);
                c.color = color.unwrap_or(v.color).gamma_multiply(fade.at(y) * share(y).1);
                mesh.vertices.push(c);
            }
            mesh.indices.extend(tris.iter().map(|&i| first + (i - glyphs.start as u32)));
        }
        row.visuals.mesh_bounds = row.visuals.mesh_bounds.expand(reach);
        placed.row = Arc::new(row);
    }
    if reach_all > 0.0 {
        out.mesh_bounds = out.mesh_bounds.expand(reach_all);
    }
    Arc::new(out)
}

/// [`faded`] when `g` at `pos` reaches into one of `fade`'s bands; `None`
/// when it lies clear of them (or there is no fade) and is drawn as it is.
fn through(g: &Galley, pos: egui::Pos2, color: Option<Color32>, fade: Option<Fade>) -> Option<Arc<Galley>> {
    let ink = g.mesh_bounds.translate(pos.to_vec2());
    fade.filter(|f| !f.clear(ink.top(), ink.bottom())).map(|f| faded(g, pos, color, f))
}

/// `g` at `pos` in `color`, thinned towards the view's edges by `fade`.
fn fade_galley(painter: &egui::Painter, pos: egui::Pos2, g: &Arc<Galley>, color: Color32, fade: Option<Fade>) {
    if color.a() == 0 {
        return;
    }
    let (pos, g) = afloat(painter, pos, g);
    match through(&g, pos, Some(color), fade) {
        Some(g) => {
            painter.galley(pos, g, color);
        }
        None => {
            painter.galley_with_override_text_color(pos, g, color);
        }
    }
}

/// `g` to be drawn at the height of `pos` exactly, between whole pixels
/// too: the renderer puts a text's origin on whole pixels, so a line that
/// moves steps from one to the next (seen as a shake while it slows to its
/// place, and as lines trembling against each other when each moves on its
/// own). Returns the origin on whole pixels and the galley with its rows
/// moved by the rest; what rests on whole pixels is drawn as it is, as
/// sharp as ever. Across, the origin is left to the renderer.
fn afloat(painter: &egui::Painter, pos: egui::Pos2, g: &Arc<Galley>) -> (egui::Pos2, Arc<Galley>) {
    let ppp = painter.pixels_per_point();
    let whole = pos2(pos.x, (pos.y * ppp).round() / ppp);
    let rest = pos.y - whole.y;
    if !rest.is_finite() || (rest * ppp).abs() < 0.02 {
        return (whole, g.clone());
    }
    let mut out = (**g).clone();
    for placed in &mut out.rows {
        placed.pos.y += rest;
    }
    out.mesh_bounds = out.mesh_bounds.translate(vec2(0.0, rest));
    (whole, Arc::new(out))
}

/// The item under the pointer at height `y`, from each item's drawn
/// `(top, height, room)` (`None` when it is not drawn). Hit areas reach
/// `reach` past a slot, so neighbours (and background voices hugging
/// their line) can share a point: the slot the pointer is really in wins,
/// else the nearest one; a wait without room never takes it.
fn hit_target(items: &[Option<(f32, f32, f32)>], y: f32, reach: f32) -> Option<usize> {
    let off = |top: f32, h: f32| if y < top { top - y } else { (y - top - h).max(0.0) };
    items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let (top, h, room) = (*it)?;
            let d = off(top, h);
            (room >= ROOMLESS && d <= reach).then_some((i, d, (y - top - h * 0.5).abs()))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(i, ..)| i)
}

/// The position the lyrics are painted at: `lead` seconds ahead of `pos`
/// (behind when negative), within ±0.5 s and never before the start.
fn led(pos: Duration, lead: f32) -> Duration {
    let lead = lead.clamp(-0.5, 0.5);
    if lead >= 0.0 {
        pos + Duration::from_secs_f32(lead)
    } else {
        pos.saturating_sub(Duration::from_secs_f32(-lead))
    }
}

/// The view's own clock: where the song is, as the frames see it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Clock {
    /// The time of the frame it was last read in.
    at: f64,
    /// The song's position then, seconds.
    pos: f64,
}

/// The clock comes to the player's position over about this long, seconds.
const CLOCK_BLEND: f64 = 0.15;
/// Further than this from the player's position (a seek, another track, a
/// loop) the clock goes there at once.
const CLOCK_SEEK: f64 = 0.25;
/// While it waits for the player's position to catch up, the clock still
/// runs at this share of the pace: it slows down, never stops or goes back.
const CLOCK_SLOWEST: f64 = 0.5;

impl Clock {
    /// The clock a frame on: at `now` (the frame's time) the player says
    /// `heard` seconds and moves `rate` seconds a second. While it plays
    /// the clock runs by the frames' own time, the one the springs go by,
    /// and comes to the player's position little by little: that one is
    /// read some way into each frame and is put right now and then, which
    /// made every front and jump tremble or hop. `end`: the track's length.
    fn step(last: Option<Clock>, heard: f64, now: f64, rate: f64, end: Option<f64>, playing: bool) -> Clock {
        let there = Clock { at: now, pos: heard };
        let Some(last) = last.filter(|_| playing) else { return there };
        let dt = now - last.at;
        if dt.is_nan() || dt < 0.0 {
            return there;
        }
        let run = dt * rate.max(0.0);
        let off = heard - (last.pos + run);
        if off.is_nan() || off.abs() > CLOCK_SEEK {
            return there;
        }
        let step = (run + off * (1.0 - (-dt / CLOCK_BLEND).exp())).max(run * CLOCK_SLOWEST);
        let pos = last.pos + step;
        Clock { at: now, pos: end.map_or(pos, |end| pos.min(end.max(last.pos))) }
    }
}

/// The song's position for this frame of the view under `id` ([`Clock`]).
fn clock(ctx: &egui::Context, id: egui::Id, player: &nikgob_view::PlayerView, now: f64, playing: bool) -> Duration {
    let heard = player.position_now().as_secs_f64();
    let end = player.duration.map(|d| d.as_secs_f64());
    let c = Clock::step(ctx.data(|d| d.get_temp(id)), heard, now, f64::from(player.rate), end, playing);
    ctx.data_mut(|d| d.insert_temp(id, c));
    Duration::from_secs_f64(c.pos.max(0.0))
}

/// The painted position, kept from stepping back by a hair while playing:
/// the clock may snap back a little as it follows what is heard, which
/// read as a front flickering backwards; a seek goes further.
fn steady(ctx: &egui::Context, id: egui::Id, pos: Duration, playing: bool) -> Duration {
    let pos = steady_pos(ctx.data(|d| d.get_temp(id)), pos, playing);
    ctx.data_mut(|d| d.insert_temp(id, pos));
    pos
}

fn steady_pos(last: Option<Duration>, pos: Duration, playing: bool) -> Duration {
    match last {
        Some(last) if playing && pos < last && last - pos <= STEP_BACK => last,
        _ => pos,
    }
}

/// Further back than this the position is a seek, and is followed.
const STEP_BACK: Duration = Duration::from_millis(350);

/// Where a line that is no longer current is sung up to, from where it was
/// sung the frame before (`last`): on with the song as it plays, but a
/// seek either way (further than [`STEP_BACK`]) leaves it as it was while
/// it fades.
fn sung_pos(last: Option<Duration>, pos: Duration) -> Duration {
    match last {
        Some(last) if pos < last || pos - last > STEP_BACK => last,
        _ => pos,
    }
}

fn returning_offset(offset: f32, until: f64, now: f64) -> f32 {
    let t = ((now - until) / 0.5).clamp(0.0, 1.0) as f32;
    offset * (1.0 - t * t * (3.0 - 2.0 * t))
}

/// The user's own scroll of the lyrics: `offset` from following the current
/// line, held until `until` (then eased back over half a second), and the
/// follow position (`from`) it was last measured against.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Manual {
    offset: f32,
    until: f64,
    from: f32,
}

impl Manual {
    /// One frame. `follow`: where following the current line scrolls to
    /// now; `dy`: the user's scroll this frame; `hold`: how long a scroll
    /// holds, seconds; `range`: the offsets that keep the text's first and
    /// last lines reachable. While it holds, the text stays where the user
    /// left it as the song moves on (a new current line moves `follow`, the
    /// offset takes the move back), so the line under the pointer is the
    /// line that gets clicked.
    fn step(self, follow: f32, dy: f32, now: f64, hold: f64, range: (f32, f32)) -> Self {
        let mut m = self;
        if m.on(now) {
            m.offset += m.from - follow;
        }
        if dy != 0.0 {
            m.offset = m.shift(now) - dy;
            m.until = now + hold;
        }
        // Not `clamp`: it panics on an empty range.
        m.offset = m.offset.max(range.0).min(range.1);
        m.from = follow;
        m
    }

    fn on(&self, now: f64) -> bool {
        now < self.until
    }

    /// The offset as drawn (easing back once the hold is over).
    fn shift(&self, now: f64) -> f32 {
        returning_offset(self.offset, self.until, now)
    }
}

/// Where a click on an item seeks: a line's start; a wait's end less a
/// second, to hear the singing come.
fn seek_to(lines: &[LyricLine], item: DisplayLine) -> Duration {
    match item {
        DisplayLine::Line(j) => lines[j].start,
        DisplayLine::Interlude { end, .. } => end.saturating_sub(Duration::from_secs(1)),
    }
}

/// A clicked item, current from the click until its seek has come back.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Jump {
    item: DisplayLine,
    /// Where it seeks ([`seek_to`]).
    to: Duration,
    /// When it was clicked (the frame's time).
    at: f64,
}

/// A click holds its item at most this long while the seek does not come
/// back (nothing plays).
const JUMP_HOLD: f64 = 3.0;

/// Past the seek's point by up to this, the painted position has reached
/// the clicked item; further on it is still the position before the seek.
const JUMP_REACH: Duration = Duration::from_millis(1500);

/// The current item with a click's `jump`: the clicked item, from the
/// click until the painted position `pos` has reached the seek's point (the
/// seek comes back a frame or more after the click; with a lead behind the
/// sound, `back`, the painted position comes up to it a little later, and
/// waits there while paused). At most [`JUMP_HOLD`] while the seek has not
/// come back; dropped when the text changed. Returns the current item and
/// the jump while it holds; without one, `by_pos`.
fn with_jump(
    jump: Option<Jump>,
    lines: &[LyricLine],
    items: &[DisplayLine],
    by_pos: Option<usize>,
    pos: Duration,
    back: Duration,
    now: f64,
) -> (Option<usize>, Option<Jump>) {
    let Some(j) = jump else { return (by_pos, None) };
    let Some(i) = items.iter().position(|it| *it == j.item).filter(|i| seek_to(lines, items[*i]) == j.to) else {
        return (by_pos, None);
    };
    if pos >= j.to && pos <= j.to + JUMP_REACH {
        return (by_pos, None);
    }
    let lagging = pos < j.to && pos + back >= j.to;
    if !lagging && !(j.at..=j.at + JUMP_HOLD).contains(&now) {
        return (by_pos, None);
    }
    (Some(i), Some(j))
}

/// The painted position while a click's `jump` holds: where its seek will
/// land, with the `lead`, until the painted position `pos` is coming up to
/// the seek's point from behind (`back`, a lead behind the sound), which is
/// then followed; `pos` without a jump.
fn jump_pos(jump: Option<Jump>, pos: Duration, back: Duration, lead: f32) -> Duration {
    match jump {
        Some(j) if !(pos < j.to && pos + back >= j.to) => led(j.to, lead),
        _ => pos,
    }
}

/// How long a line without an end time takes to sing: ~15 letters a second,
/// at least 0.8 s, but never past 92 % of the `gap` (seconds) to the next line.
/// Word timing for a line that only has a start: its words share a
/// speaking-pace span (see [`speaking_secs`]) in proportion to their letters.
fn paced_words(l: &LyricLine, next: Option<Duration>) -> LyricLine {
    let gap = l
        .end
        .or(next)
        .map_or(6.0, |t| t.saturating_sub(l.start).as_secs_f32());
    let span = if l.end.is_some() { gap.max(0.3) } else { speaking_secs(&l.text, gap) };
    let letters = |w: &str| w.chars().filter(|c| !c.is_whitespace()).count() as f32;
    let total = letters(&l.text).max(1.0);
    let mut out = l.clone();
    let mut at = 0.0;
    for w in l.text.split_inclusive(char::is_whitespace) {
        let n = letters(w);
        let (a, b) = (at / total * span, (at + n) / total * span);
        at += n;
        out.words.push(nikgob_view::LyricWord {
            start: l.start + Duration::from_secs_f32(a),
            end: l.start + Duration::from_secs_f32(b),
            text: w.to_owned(),
        });
    }
    out
}

fn speaking_secs(text: &str, gap: f32) -> f32 {
    let letters = text.chars().filter(|c| !c.is_whitespace()).count() as f32;
    // Not `clamp`: with lines closer than 0.87 s the cap is the smaller
    // bound, and `clamp` panics on that.
    (0.4 + letters * 0.066).max(0.8).min((gap * 0.92).max(0.3))
}

/// A key for each item that is the same for the same line in another
/// copy of the text: its words (and whether it is a background voice) and
/// how many times they came before; a wait by the line it waits for.
fn item_keys(lines: &[LyricLine], items: &[DisplayLine]) -> Vec<u64> {
    use std::hash::{Hash, Hasher};
    let hash = |v: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        v(&mut h);
        h.finish()
    };
    let mut seen = std::collections::HashMap::<(&str, bool), u32>::new();
    let mut keys: Vec<u64> = items
        .iter()
        .map(|item| match *item {
            DisplayLine::Line(j) => {
                let l = &lines[j];
                let n = seen.entry((l.text.trim(), l.background)).or_default();
                *n += 1;
                hash(&|h| ("line", l.text.trim(), l.background, *n).hash(h))
            }
            DisplayLine::Interlude { .. } => 0,
        })
        .collect();
    let mut next = hash(&|h| "end".hash(h));
    for (k, item) in keys.iter_mut().zip(items).rev() {
        match item {
            DisplayLine::Line(_) => next = *k,
            DisplayLine::Interlude { .. } => *k = hash(&|h| ("wait", next).hash(h)),
        }
    }
    keys
}

/// The items' slots, kept under `id` from frame to frame.
#[derive(Clone, Default)]
struct Slots {
    keys: Vec<u64>,
    slots: Vec<u64>,
    next: u64,
}

/// The slot of each item with `keys`: when most of them were there the
/// frame before (the same song's text, swapped), each keeps its slot and a
/// new one gets a slot of its own; otherwise (another song) the slots go
/// by place, as the items do. With whether the items changed since the
/// frame before (`None`: the view's first frame).
fn slots(ctx: &egui::Context, id: egui::Id, keys: &[u64]) -> (Vec<u64>, Option<bool>) {
    let was = ctx.data(|d| d.get_temp::<Slots>(id));
    if let Some(was) = &was
        && was.keys == keys
    {
        return (was.slots.clone(), Some(false));
    }
    let changed = was.is_some().then_some(true);
    let now = carry_slots(&was.unwrap_or_default(), keys);
    let out = now.slots.clone();
    ctx.data_mut(|d| d.insert_temp(id, now));
    (out, changed)
}

fn carry_slots(was: &Slots, keys: &[u64]) -> Slots {
    let mut next = was.next.max(was.slots.iter().max().map_or(0, |m| m + 1));
    let mut fresh = || {
        next += 1;
        next - 1
    };
    let old: std::collections::HashMap<u64, u64> = was.keys.iter().copied().zip(was.slots.iter().copied()).collect();
    let kept = keys.iter().filter(|k| old.contains_key(k)).count();
    let slots = if !was.keys.is_empty() && kept * 2 >= keys.len() {
        keys.iter().map(|k| old.get(k).copied().unwrap_or_else(&mut fresh)).collect()
    } else {
        (0..keys.len()).map(|i| was.slots.get(i).copied().unwrap_or_else(&mut fresh)).collect()
    };
    Slots { keys: keys.to_vec(), slots, next }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DisplayLine {
    Line(usize),
    Interlude { start: Duration, end: Duration, duet: bool },
}

fn current_item(lines: &[LyricLine], items: &[DisplayLine], pos: Duration) -> Option<usize> {
    items.iter().position(|item| matches!(item, DisplayLine::Interlude { start, end, .. } if *start <= pos && pos < *end))
        .or_else(|| items.iter().rposition(|item| matches!(item, DisplayLine::Line(i) if !lines[*i].background && lines[*i].start <= pos)))
}

/// Empty stamps own their gap, so computed waiting slots cannot double them.
/// Any singing voice overlapping a break makes it unsuitable for waiting.
fn display_lines(lines: &[LyricLine]) -> Vec<DisplayLine> {
    let mut items = Vec::new();
    let mut prev: Option<usize> = None;
    for (i, next) in lines.iter().enumerate().filter(|(_, l)| !l.background && !l.text.trim().is_empty()) {
        let from = prev.map_or(0, |j| j + 1);
        let explicit = lines[from..i].iter().find(|l| l.text.trim().is_empty());
        let start = if let Some(empty) = explicit {
            Some(empty.start)
        } else if let Some(j) = prev {
            let l = &lines[j];
            let gap = next.start.saturating_sub(l.start).as_secs_f32();
            let end = l.end.unwrap_or(l.start + Duration::from_secs_f32(speaking_secs(&l.text, gap)));
            (next.start.saturating_sub(end) >= Duration::from_secs(4)).then_some(end)
        } else {
            (next.start >= Duration::from_millis(2500)).then_some(Duration::ZERO)
        };
        if let Some(start) = start {
            let singing = lines.iter().any(|l| {
                if !l.background || l.text.trim().is_empty() {
                    return false;
                }
                let end = l.end.unwrap_or(l.start + Duration::from_secs_f32(speaking_secs(
                    &l.text, next.start.saturating_sub(l.start).as_secs_f32(),
                )));
                l.start < next.start && end > start
            });
            if !singing && next.start.saturating_sub(start) >= Duration::from_millis(1500) {
                items.push(DisplayLine::Interlude { start, end: next.start, duet: next.duet });
            }
        }
        prev = Some(i);
    }
    items.extend(lines.iter().enumerate().filter(|(_, l)| !l.text.trim().is_empty()).map(|(i, _)| DisplayLine::Line(i)));
    items.sort_by_key(|item| match *item {
        DisplayLine::Line(i) => lines[i].start,
        DisplayLine::Interlude { start, .. } => start,
    });
    items
}

#[derive(Clone)]
struct Breaks {
    text: String,
    wrap: f32,
    big: f32,
    forced: String,
    height: f32,
    /// Width of the widest row at BIG.
    width: f32,
}

/// Glyph advances round differently at different sizes. Freeze the BIG
/// rows as paragraphs: newlines are row separators, never extra glyphs.
///
/// Kept shared, so taking it every frame for every line copies no text.
fn fixed_breaks(painter: &egui::Painter, id: egui::Id, text: &str, wrap: f32, big: f32) -> Arc<Breaks> {
    let ctx = painter.ctx();
    if let Some(b) = ctx.data(|d| d.get_temp::<Arc<Breaks>>(id))
        && b.text == text && b.wrap == wrap && b.big == big
    {
        return b;
    }
    let mut job = LayoutJob::single_section(text.to_owned(), style_at(big).text_format(Color32::WHITE));
    job.wrap.max_width = wrap;
    let g = crate::typography::layout(painter, job);
    let forced = g.rows.iter().map(|r| r.row.glyphs.iter().map(|gl| gl.chr).collect::<String>()).collect::<Vec<_>>().join("\n");
    let size = fixed_layout(painter, &forced, big).size();
    let b = Arc::new(Breaks { text: text.to_owned(), wrap, big, forced, height: size.y, width: size.x });
    ctx.data_mut(|d| d.insert_temp(id, b.clone()));
    b
}

fn fixed_layout(painter: &egui::Painter, text: &str, size: f32) -> Arc<Galley> {
    crate::typography::layout(painter, LayoutJob::single_section(text.to_owned(), style_at(size).text_format(Color32::WHITE)))
}

/// [`fixed_layout`] for a line whose size moves between its resting sizes
/// `rest` (small, big), `t` (0..1) of the way. It is the big resting
/// layout scaled down to the size (the small one scaled up within half a
/// point of it): one layout all the way, so each letter keeps its place in
/// the line as it grows. Laid out anew at every whole size on the way, each
/// with its letters on its own whole pixels, the letters shook against
/// each other a dozen times a move, and each size drew its glyphs into the
/// font atlas the first time. Its rows and height go evenly between the two
/// resting layouts', so the line grows without the whole-pixel steps of
/// each size's own rounded row height (which moved it in its slot by half
/// a pixel at a time) and meets either resting layout where it ends.
fn moving_layout(painter: &egui::Painter, text: &str, rest: (f32, f32), t: f32) -> Arc<Galley> {
    let size = rest.0 + (rest.1 - rest.0) * t;
    let (from, to) = (fixed_layout(painter, text, rest.0), fixed_layout(painter, text, rest.1));
    let (base, at) = if (size - rest.0).abs() <= 0.5 && (size - rest.1).abs() > 0.5 { (&from, rest.0) } else { (&to, rest.1) };
    let g = crate::typography::scale_galley(base, size / at.max(f32::EPSILON));
    between(g, &from, &to, t)
}

/// `g` with its rows placed and its height taken `t` (0..1) of the way
/// from `from`'s to `to`'s (the same rows at two sizes); as it is when the
/// rows differ.
fn between(g: Arc<Galley>, from: &Galley, to: &Galley, t: f32) -> Arc<Galley> {
    if g.rows.len() != from.rows.len() || g.rows.len() != to.rows.len() {
        return g;
    }
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    let mut out = Arc::unwrap_or_clone(g);
    let mut bounds = Rect::NOTHING;
    for (r, placed) in out.rows.iter_mut().enumerate() {
        placed.pos.y = lerp(from.rows[r].pos.y, to.rows[r].pos.y);
        bounds = bounds.union(placed.row.visuals.mesh_bounds.translate(placed.pos.to_vec2()));
    }
    out.mesh_bounds = bounds;
    out.rect = Rect::from_min_size(out.rect.min, vec2(out.rect.width(), lerp(from.rect.height(), to.rect.height())));
    Arc::new(out)
}

/// A looping sample for the settings' hover card: the line being sung
/// between two resting ones, painted by the real line painter under `prefs`
/// (animation, glow, dimming, size, letter jump).
pub(crate) fn preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    preview_at(ui, rect, prefs, None);
}

/// The text of a sample: the resting lines above and below the sung one,
/// and the sung one by its syllables.
struct SampleText {
    above: &'static str,
    below: &'static str,
    sung: &'static [&'static str],
}

/// The ⓘ card's sample.
const CARD_TEXT: SampleText = SampleText { above: "Где-то рядом", below: "Он ведёт меня", sung: &["Слы", "шишь ", "э", "тот ", "ритм"] };
/// A preset tile's: short lines, so the sung one can be large in a small
/// tile.
const TILE_TEXT: SampleText = SampleText { above: "Где-то рядом", below: "Он ведёт", sung: &["Слы", "шишь ", "ритм"] };

/// [`preview`] standing at `still` seconds into its loop, or running with
/// the clock.
pub(crate) fn preview_at(ui: &Ui, rect: Rect, prefs: &Prefs, still: Option<f64>) {
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let scale = prefs.lyrics_scale.clamp(0.8, 1.4);
    let (big, small) = (PANE.big * 0.8 * scale, PANE.small * 0.8 * scale);
    // Fit the three lines into the sample (a small card shrinks the text;
    // the size setting still shows, relative to the others): fitted at the
    // largest size.
    let (w, h) = sample_extent(&painter, &CARD_TEXT, big / scale * 1.4, small / scale * 1.4);
    let fit = ((rect.width() - 2.0 * 10.0) / w).min((rect.height() - 2.0 * 6.0) / h).min(1.0);
    sample(ui.ctx(), &painter, rect, prefs, still, &CARD_TEXT, big * fit, small * fit, 6.0 * fit, 10.0);
}

/// A preset's tile (`rect`, small): the sample drawn as the lyrics view
/// draws it, at the view's own size, and the whole scene scaled down to the
/// tile — the jump, the lift, the glow and the gaps shrink with the text,
/// so the motion reads as it does on the real screen. Standing at `still`
/// seconds into its loop (a tile at rest), or running with the clock. The
/// size setting is not a preset's and is left out.
pub(crate) fn tile_preview(ui: &Ui, rect: Rect, prefs: &Prefs, still: Option<f64>) {
    let ctx = ui.ctx();
    let probe = ui.painter();
    let (big, small, gap) = (PANE.big, PANE.small, 8.0);
    let (w, h) = sample_extent(probe, &TILE_TEXT, big, small);
    // The sung line spans the tile but for a margin; the three rows fit.
    const MARGIN: f32 = 10.0;
    let k = ((rect.width() - 2.0 * MARGIN) / w).min((rect.height() - 8.0) / (h + 2.0 * gap)).clamp(0.05, 1.0);
    let t = egui::emath::TSTransform::new(rect.center().to_vec2() * (1.0 - k), k);
    let virt = t.inverse().mul_rect(rect);
    let clip = t.inverse().mul_rect(ui.clip_rect().intersect(rect));
    let layer = ui.layer_id();
    let first = ctx.graphics(|g| g.get(layer).map_or(egui::layers::ShapeIdx(0), |l| l.next_idx()));
    // Set, not intersected with the tile's clip: that one is on screen,
    // this one in the scene's points (scaled down with it).
    let mut painter = ui.painter().clone();
    painter.set_clip_rect(clip);
    sample(ctx, &painter, virt, prefs, still, &TILE_TEXT, big, small, gap, MARGIN / k);
    ctx.graphics_mut(|g| {
        let list = g.entry(layer);
        let end = list.next_idx();
        list.transform_range(first, end, t);
    });
}

/// How wide the widest line of `text` is and how tall its three rows
/// stand (with the wave's jump above), at `big` for the sung line and
/// `small` for the others.
fn sample_extent(painter: &egui::Painter, text: &SampleText, big: f32, small: f32) -> (f32, f32) {
    let sung: String = text.sung.concat();
    let rows = [(text.above, small), (sung.as_str(), big), (text.below, small)];
    let w = rows.iter().map(|(t, z)| fixed_layout(painter, t, *z).size().x).fold(0.0, f32::max);
    let h: f32 = rows.iter().map(|(t, z)| fixed_layout(painter, t, *z).size().y).sum();
    (w, h + 2.0 * 6.0 + big * 0.25)
}

/// The looping sample over `rect`: `text`'s sung line at `big` between its
/// resting lines at `small`, `gap` apart, `pad` in from the left, painted by
/// the real line painter under `prefs`.
#[allow(clippy::too_many_arguments)]
fn sample(
    ctx: &egui::Context,
    painter: &egui::Painter,
    rect: Rect,
    prefs: &Prefs,
    still: Option<f64>,
    text: &SampleText,
    big: f32,
    small: f32,
    gap: f32,
    pad: f32,
) {
    let s = Theme::get(ctx).scheme;
    let look = Look::of(prefs);
    // Sung by ~2.9 s, held lit, then eased back to unsung before the loop
    // starts over (a snap back read as a glitch).
    const LOOP: f64 = 3.7;
    const BACK: f32 = 3.2;
    let pos = Duration::from_secs_f64(still.unwrap_or_else(|| anim::now(ctx)).rem_euclid(LOOP));
    let mut at = 0.3;
    let words = text
        .sung
        .iter()
        .map(|w| {
            let d = 0.16 * w.trim().chars().count() as f32;
            let word = nikgob_view::LyricWord {
                start: Duration::from_secs_f32(at),
                end: Duration::from_secs_f32(at + d),
                text: (*w).to_owned(),
            };
            at += d + 0.05;
            word
        })
        .collect::<Vec<_>>();
    let line = LyricLine { text: words.iter().map(|w| w.text.as_str()).collect(), words, ..Default::default() };
    let rest = 1.0 - (1.0 - 0.42) * prefs.lyrics_dim.clamp(0.0, 1.0);
    let dim = s.on_surface.gamma_multiply(rest);
    let lit = s.on_surface;
    let g = fixed_layout(painter, &line.text, big);
    // On whole pixels, as a line at rest in the view is.
    let ppp = painter.pixels_per_point();
    let top = ((rect.center().y - g.size().y * 0.5 + big * 0.1) * ppp).round() / ppp;
    let cur = Rect::from_min_size(pos2(rect.left() + pad, top), g.size());
    for (t, y) in [(text.above, cur.top() - gap), (text.below, cur.bottom() + gap)] {
        let sg = fixed_layout(painter, t, small);
        let top = if y < cur.top() { y - sg.size().y } else { y };
        painter.galley_with_override_text_color(pos2(cur.left(), top), sg, dim);
    }
    let base = s.on_surface.gamma_multiply(rest.min(UNSUNG));
    if let Some(timing) = timing_of(ctx, &line, &g, look, Pace::of(prefs), big) {
        // Past the end the line is drawn as no longer current, its light
        // (and the wave's lift) easing out with `focus`.
        let back = ((pos.as_secs_f32() - BACK) / 0.4).clamp(0.0, 1.0);
        let back = back * back * (3.0 - 2.0 * back);
        let (current, focus) = if back > 0.0 { (false, 1.0 - back) } else { (true, 1.0) };
        let color = crate::lerp_color(base, lit, focus);
        timed_line(painter, &g, cur, &timing, pos, focus, if current { 1.0 } else { focus }, big, look, base, color, prefs, None);
    }
}

/// The pause indicator between lines for the settings' cards: a gap on
/// loop, with the countdown at its end.
pub(crate) fn gap_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    // The ring (with its wave and the countdown's pop) fits the sample with
    // ~10 px around it, centred; fitted at the largest size, so the size
    // setting still shows.
    let base = if prefs.lyrics_gap_style >= 2 { 16.0 } else { 13.0 };
    let reach = base + 8.0;
    let k = ((rect.width().min(rect.height()) - 20.0) / (2.0 * reach * 1.5)).clamp(0.4, 1.4) * prefs.lyrics_gap_size.clamp(0.8, 1.5);
    let left = rect.center().x - 2.0 * k - base * k;
    let line = Rect::from_min_size(pos2(left, rect.center().y - 21.0 * k), vec2(56.0 * k, 42.0 * k));
    // A seven-second gap: four of the ring (its style is what is compared),
    // then the countdown when it is on.
    let pos = Duration::from_secs_f64(anim::now(ui.ctx()).rem_euclid(7.6));
    let bass = super::pages::settings_help::beat(anim::now(ui.ctx()) as f32) * 0.5;
    waiting(&painter, line, k, Duration::ZERO, Duration::from_secs(7), pos, ring_in(pos.as_secs_f32()), true, bass, 1.0, prefs);
}

/// The countdown in context for its setting's card: a sung line, the wait
/// under it (ring, then 3-2-1 when on) and the next line, which lights up
/// right when the count ends.
pub(crate) fn countdown_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    const PAD: f32 = 12.0;
    // A 1.4 s ring, the count, then the next line sung for a moment.
    const GAP: f32 = 4.4;
    const LOOP: f64 = 6.2;
    let t = anim::now(ui.ctx()).rem_euclid(LOOP) as f32;
    let rest = 1.0 - (1.0 - 0.42) * prefs.lyrics_dim.clamp(0.0, 1.0);
    let dim = s.on_surface.gamma_multiply(rest);
    let small = PANE.small * 0.75;
    let k = 0.8 * prefs.lyrics_gap_size.clamp(0.8, 1.5);
    let row_h = 42.0 * k;
    let prev = fixed_layout(&painter, "Слышишь этот ритм", small);
    let next = fixed_layout(&painter, "Он ведёт меня", small);
    let total = prev.size().y + row_h + next.size().y + 8.0;
    let top = rect.center().y - total * 0.5;
    painter.galley_with_override_text_color(pos2(rect.left() + PAD, top), prev.clone(), dim);
    let line = Rect::from_min_size(pos2(rect.left() + PAD, top + prev.size().y + 4.0), vec2(56.0 * k, row_h));
    let bass = super::pages::settings_help::beat(anim::now(ui.ctx()) as f32) * 0.5;
    let pos = Duration::from_secs_f32(t);
    waiting(&painter, line, k, Duration::ZERO, Duration::from_secs_f32(GAP), pos, ring_in(t), true, bass, 1.0, prefs);
    // The next line: dim while waiting, sung once the wait is over.
    let on = ((t - GAP) / 0.18).clamp(0.0, 1.0);
    let color = crate::lerp_color(dim, s.on_surface, on);
    painter.galley_with_override_text_color(pos2(rect.left() + PAD, line.bottom() + 4.0), next, color);
}

/// The soft edges for their settings' card: lines rolling up through the
/// sample as the lyrics scroll, thinning over its top and bottom bands as
/// in the view (when off, cut by the card as before).
pub(crate) fn edges_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    const TEXTS: [&str; 6] = ["Где-то рядом", "Слышишь этот ритм", "Он ведёт меня", "Сквозь огни домов", "До самого утра", "И дальше вперёд"];
    // A line every 1.6 s, each move eased out over half a second.
    const STEP: f64 = 1.6;
    let size = (rect.height() / 6.0).clamp(12.0, 20.0);
    // The lines rest on whole pixels, a whole number of them apart, and
    // roll between them ([`afloat`]).
    let ppp = painter.pixels_per_point();
    let whole = |v: f32| (v * ppp).round() / ppp;
    let pitch = whole(size * 1.8);
    let t = anim::now(ui.ctx()) / STEP;
    let n = t.floor() as i64;
    let x = ((t.fract() * STEP) / 0.5).min(1.0) as f32;
    let ease = 1.0 - (1.0 - x).powi(3);
    // Over what is seen of the card, as in the view.
    let seen = painter.clip_rect();
    let seen = if seen.is_positive() { seen } else { rect };
    let fade = prefs.lyrics_soft_edges.then(|| Fade::of(seen, Soft::of(prefs).text(pitch, size)));
    for k in -4i64..=5 {
        let g = fixed_layout(&painter, TEXTS[(n + k).rem_euclid(TEXTS.len() as i64) as usize], size);
        let top = whole(rect.center().y - g.size().y * 0.5) + (k as f32 + 1.0 - ease) * pitch;
        let alpha = if fade.is_some() {
            1.0
        } else {
            let band = rect.height() * 0.12;
            ((top + g.size().y - rect.top()) / band).min((rect.bottom() - top) / band).clamp(0.0, 1.0)
        };
        let rest = if k == 0 { 1.0 } else { 0.45 };
        fade_galley(&painter, pos2(rect.left() + 12.0, top), &g, s.on_surface.gamma_multiply(rest * alpha), fade);
    }
}

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
        let text = if state.lyrics.loading { "Ищу текст…" } else { "Текста нет" };
        child.painter().text(rect.center(), Align2::CENTER_CENTER, text, style_at(MEGA.small).font_id(), s.on_surface_variant);
    }
    let text = state.lyrics.plain.as_deref().is_some_and(|t| !t.trim().is_empty());
    text.then_some(column.left())
}

/// One timed piece spans its own glyph advances, even across frozen rows.
struct Syllable {
    start: f32,
    end: f32,
    from: f32,
    to: f32,
    letters: usize,
    /// Its whitespace-delimited word ([`Timing::words`]).
    word: usize,
    /// Its first and one past its last glyph.
    glyphs: (usize, usize),
}

/// A written unit for «По словам»: a word, or a part of a hyphenated word
/// as it is written («do-», «dodge»), from its first letter's time to its
/// last one's.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Unit {
    start: f32,
    end: f32,
}

struct Timing {
    syllables: Vec<Syllable>,
    /// Syllable index for each glyph (spaces have none).
    glyphs: Vec<Option<usize>>,
    units: Vec<Unit>,
    /// Unit index for each glyph (spaces have none).
    glyph_units: Vec<Option<usize>>,
    /// Each letter's own part of its syllable's time, weighted by
    /// [`letter_weight`] (spaces have none).
    #[cfg(test)]
    letter_times: Vec<Option<(f32, f32)>>,
    /// When each letter starts to light (and to jump) under the look the
    /// timing is for: as the front's way comes to it in the wave, or to its
    /// syllable or written word where those light whole; never before a
    /// letter written ahead of it. None for spaces, and for every letter
    /// where nothing lights by itself: karaoke follows the front, Focus by
    /// letters lights the whole line.
    lit: Vec<Option<f32>>,
    /// Each glyph's span on the line's run of advances.
    #[cfg(test)]
    xs: Vec<(f32, f32)>,
    /// First syllable of each whitespace-delimited word.
    words: Vec<usize>,
    /// The way the light goes along the line.
    front: Front,
}

/// The light's way along a line under a look: a [`Run`] for the pieces sung
/// one after another without a pause (a whole line, when it is sung in one
/// breath), each with a front of its own that crosses it in one motion: no
/// stop and no step in its speed at a letter, a syllable or a word, and
/// never back. It waits only where the singing does, at the end of a run,
/// with the run lit whole and the next one not begun.
///
/// What it keeps time by is the look's unit: by syllables the timed
/// pieces, each crossed evenly; by letters those with the ends of their
/// letters as the letters' weights guess them (a held vowel draws out); by
/// words each written word, crossed evenly from its first sound to its
/// last.
struct Front {
    runs: Vec<Run>,
    /// Run index for each glyph (spaces have none).
    of: Vec<Option<usize>>,
    /// The width of the light's soft edge, px: the front is its middle.
    edge: f32,
}

impl Front {
    /// The front for `look` over a line's pieces, and when each letter
    /// starts to light under it ([`Timing::lit`]).
    #[allow(clippy::too_many_arguments)]
    fn over(
        look: Look,
        syllables: &[Syllable],
        glyphs: &[Option<usize>],
        units: &[Unit],
        glyph_units: &[Option<usize>],
        times: &[Option<(f32, f32)>],
        xs: &[(f32, f32)],
        edge: f32,
        ease: Ease,
    ) -> (Self, Vec<Option<f32>>) {
        let n = glyphs.len();
        // Focus by letters lights the line whole: nothing goes along it.
        if look.style == Style::Focus && look.by == By::Letters {
            return (Front { runs: Vec::new(), of: vec![None; n], edge: 0.0 }, vec![None; n]);
        }
        let sweeping = look.style == Style::Karaoke;
        // Only karaoke draws the front's edge; the other looks take its
        // times.
        let edge = if sweeping { edge.max(0.0) } else { 0.0 };
        let owners = if look.words() { glyph_units } else { glyphs };
        let mut pieces: Vec<Piece> = Vec::new();
        let mut marks: Vec<(f32, f32)> = Vec::new();
        let mut piece_of: Vec<Option<usize>> = vec![None; n];
        let mut filling = None;
        for k in 0..n {
            let Some(o) = owners[k] else { continue };
            if filling != Some(o) {
                let (start, end) = if look.words() {
                    units.get(o).map_or((0.0, 0.0), |u| (u.start, u.end))
                } else {
                    (syllables[o].start, syllables[o].end)
                };
                pieces.push(Piece { start, end, from: xs[k].0, to: xs[k].1, marks: marks.len()..marks.len() });
                filling = Some(o);
            } else if let Some(piece) = pieces.last_mut() {
                // By letters: where each letter ends inside its piece.
                if look.by == By::Letters
                    && let Some((at, _)) = times[k]
                {
                    marks.push((xs[k].0, at));
                    piece.marks.end = marks.len();
                }
                piece.to = xs[k].1;
            }
            piece_of[k] = Some(pieces.len() - 1);
        }
        let (runs, run_of) = lyrics_front::runs(&pieces, &marks, edge, ease);
        let of = piece_of.iter().map(|p| p.map(|i| run_of[i])).collect();
        let lit = if sweeping {
            vec![None; n]
        } else {
            // The wave lights letter by letter (by words the word whole).
            let by_letter = look.style == Style::Wave && !look.words();
            (0..n)
                .map(|k| piece_of[k].map(|i| runs[run_of[i]].left(if by_letter { xs[k].0 } else { pieces[i].from })))
                .collect()
        };
        (Front { runs, of, edge }, lit)
    }

    /// The unlit end of run `run`'s soft edge at `p` seconds, px along the
    /// line: the light is whole an edge's width behind it. −∞ before the run
    /// is sung (nothing of it lit), +∞ once its front has come to rest (all
    /// of it lit); the edge is past either end of the run by then, so
    /// neither is a step.
    fn head(&self, run: usize, p: f64) -> f32 {
        let Some(run) = self.runs.get(run) else { return f32::INFINITY };
        let (start, rest) = run.times();
        if p <= start {
            f32::NEG_INFINITY
        } else if p >= rest {
            f32::INFINITY
        } else {
            run.at(p) + 0.5 * self.edge
        }
    }
}

/// The timings of the lines being sung, kept from frame to frame: a line's
/// timing and its front's way are worked out once for its layout, look and
/// settings, not every frame.
#[derive(Clone, Default)]
struct Timings(Vec<(u64, Arc<Timing>)>);

/// How many timings are kept: the lines sung or fading at once, twice over
/// for a change of look.
const TIMINGS: usize = 12;

/// [`Timing::of`], kept ([`Timings`]).
fn timing_of(ctx: &egui::Context, l: &LyricLine, g: &Galley, look: Look, pace: Pace, size: f32) -> Option<Arc<Timing>> {
    use std::hash::{Hash, Hasher};
    if l.words.is_empty() {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (look.style as u8, look.by as u8, pace.edge(size).to_bits(), pace.smooth.to_bits()).hash(&mut h);
    for w in &l.words {
        (w.start, w.end, &w.text).hash(&mut h);
    }
    for gl in g.rows.iter().flat_map(|r| &r.row.glyphs) {
        (gl.chr, gl.advance_width.to_bits()).hash(&mut h);
    }
    let key = h.finish();
    let id = egui::Id::new("lyrics-timings");
    let kept = ctx.data_mut(|d| d.get_temp_mut_or_default::<Timings>(id).0.iter().find(|(k, _)| *k == key).map(|(_, t)| t.clone()));
    if kept.is_some() {
        return kept;
    }
    let timing = Arc::new(Timing::of(l, g, look, pace, size)?);
    ctx.data_mut(|d| {
        let kept = &mut d.get_temp_mut_or_default::<Timings>(id).0;
        if kept.len() >= TIMINGS {
            kept.remove(0);
        }
        kept.push((key, timing.clone()));
    });
    Some(timing)
}

impl Timing {
    /// The timing of `l` laid out as `g`, by letters and on its times
    /// exactly.
    #[cfg(test)]
    fn new(l: &LyricLine, g: &Galley) -> Option<Self> {
        Self::build(l, g, Look { style: Style::Wave, by: By::Letters }, 0.0, Ease::EXACT)
    }

    /// The timing of `l` laid out as `g` at text `size`, for `look` at
    /// `pace`: «Плавность» is how closely the front keeps to the times,
    /// «Скорость подсветки» how wide the karaoke edge is.
    fn of(l: &LyricLine, g: &Galley, look: Look, pace: Pace, size: f32) -> Option<Self> {
        Self::build(l, g, look, pace.edge(size), Ease::of(pace.smooth))
    }

    fn build(l: &LyricLine, g: &Galley, look: Look, edge: f32, ease: Ease) -> Option<Self> {
        // By letters a letter's part of its syllable's time goes by its
        // weight; else evenly by its advance.
        let even = look.by != By::Letters;
        if l.words.is_empty() {
            return None;
        }
        let mut syllables = Vec::new();
        let mut letters = Vec::new();
        let mut words = Vec::new();
        for (i, w) in l.words.iter().enumerate() {
            if i == 0 || l.words[i - 1].text.ends_with(char::is_whitespace)
                || w.text.starts_with(char::is_whitespace)
            {
                words.push(i);
            }
            let n = w.text.chars().filter(|c| !c.is_whitespace()).count();
            syllables.push(Syllable {
                start: w.start.as_secs_f32(), end: w.end.as_secs_f32().max(w.start.as_secs_f32()),
                from: 0.0, to: 0.0, letters: n, word: words.len() - 1, glyphs: (0, 0),
            });
            letters.extend(std::iter::repeat_n(i, n));
        }
        let mut letters = letters.into_iter();
        let mut x = 0.0;
        let mut glyphs = Vec::new();
        let mut xs = Vec::new();
        let mut seen = vec![false; syllables.len()];
        let mut weights = vec![0.0f32; syllables.len()];
        for (k, gl) in g.rows.iter().flat_map(|r| &r.row.glyphs).enumerate() {
            let syllable = if gl.chr.is_whitespace() { None } else { letters.next() };
            if let Some(i) = syllable {
                if !seen[i] {
                    syllables[i].from = x;
                    syllables[i].glyphs.0 = k;
                    seen[i] = true;
                }
                syllables[i].to = x + gl.advance_width;
                syllables[i].glyphs.1 = k + 1;
                weights[i] += letter_weight(gl.chr);
            }
            glyphs.push(syllable);
            xs.push((x, x + gl.advance_width));
            x += gl.advance_width;
        }
        // Each letter's own part of its syllable's time: vowels, which
        // carry a held sound, take more of it than consonants (evenly by
        // its advance, `even`).
        let mut done = vec![0.0f32; syllables.len()];
        let times: Vec<Option<(f32, f32)>> = g.rows.iter().flat_map(|r| &r.row.glyphs).zip(&glyphs).zip(&xs)
            .map(|((gl, syllable), &(x0, x1))| {
                let i = (*syllable)?;
                let s = &syllables[i];
                if even {
                    let at = |x: f32| s.start + (s.end - s.start) * ((x - s.from) / (s.to - s.from).max(f32::EPSILON)).clamp(0.0, 1.0);
                    // The last letter ends exactly with its syllable.
                    return Some((at(x0), if x1 >= s.to - 1e-3 { s.end } else { at(x1) }));
                }
                let per = (s.end - s.start) / weights[i].max(f32::EPSILON);
                let w = letter_weight(gl.chr);
                let at = s.start + per * done[i];
                done[i] += w;
                // The last letter ends exactly with its syllable.
                let to = if done[i] >= weights[i] - 1e-4 { s.end } else { (s.start + per * done[i]).min(s.end) };
                Some((at.min(s.end), to))
            })
            .collect();
        let glyph_units = written_units(g.rows.iter().flat_map(|r| &r.row.glyphs).map(|gl| gl.chr));
        let mut spans: Vec<Option<Unit>> = vec![None; glyph_units.iter().flatten().max().map_or(0, |n| n + 1)];
        for (u, t) in glyph_units.iter().zip(&times) {
            if let (Some(u), Some((a, b))) = (*u, *t) {
                let span = spans[u].get_or_insert(Unit { start: a, end: b });
                span.start = span.start.min(a);
                span.end = span.end.max(b);
            }
        }
        // A unit none of whose letters is timed (a text that differs from
        // its timing) lights with the one before it.
        let first = syllables.first().map_or(0.0, |s| s.start);
        let mut last = Unit { start: first, end: first };
        // Letters and units light in the order they are written: a piece
        // timed to start before one written ahead of it (a timing slip)
        // waits for it, so the light never runs back along the line.
        let mut latest = f32::NEG_INFINITY;
        let units: Vec<Unit> = spans
            .into_iter()
            .map(|u| {
                last = u.unwrap_or(last);
                latest = latest.max(last.start);
                Unit { start: latest, end: last.end.max(latest) }
            })
            .collect();
        let (front, lit) = Front::over(look, &syllables, &glyphs, &units, &glyph_units, &times, &xs, edge, ease);
        Some(Self {
            syllables,
            glyphs,
            units,
            glyph_units,
            #[cfg(test)]
            letter_times: times,
            lit,
            #[cfg(test)]
            xs,
            words,
            front,
        })
    }

    /// How high (in rest heights) the written word of syllable `s` stands
    /// at `p` by syllables: it goes up with each of its syllables, a share
    /// at a time and each with its own jump, so it is up in full once the
    /// last is sung. With the seconds since that last one's onset
    /// (negative before it), for its settling onto whole pixels.
    fn word_jump(&self, s: usize, p: f32, speed: f32, window: f32) -> (f32, f32) {
        let w = self.syllables[s].word;
        let first = self.words[w];
        let end = self.words.get(w + 1).copied().unwrap_or(self.syllables.len()).max(first + 1);
        let onset = |i: usize| {
            let sy = &self.syllables[i];
            if sy.letters == 0 { sy.start } else { self.lit.get(sy.glyphs.0).copied().flatten().unwrap_or(sy.start) }
        };
        let height = (first..end).map(|i| jump_over(p - onset(i), speed, window)).sum::<f32>() / (end - first) as f32;
        (height, p - onset(end - 1))
    }
}

/// How much of its syllable's (or word's) time a letter takes: vowels,
/// which carry a held sound, more than consonants; signs and marks little.
/// Letters that are syllables already (kana, hangul, hanzi) share evenly.
fn letter_weight(c: char) -> f32 {
    const VOWELS: &str = "aeiouyàáâãäåæèéêëìíîïòóôõöøùúûüýÿœāăąēĕėęěīĭįıōŏőūŭůűųаеёиоуыэюяіїєўαεηιουωάέήίόύώϊϋ";
    if !c.is_alphanumeric() {
        return 0.25;
    }
    let lower = c.to_lowercase().next().unwrap_or(c);
    if VOWELS.contains(lower) {
        2.5
    } else if matches!(lower, 'ь' | 'ъ') {
        0.25
    } else {
        1.0
    }
}

/// The written unit of each char (whitespace has none): words, and the
/// parts of a hyphenated word as written, the hyphen ending its part
/// («do-dodge» is «do-» and «dodge»). A hyphen with no letter before it
/// («-то») does not split.
fn written_units(chars: impl IntoIterator<Item = char>) -> Vec<Option<usize>> {
    let mut out = Vec::new();
    let mut count = 0;
    // The last char of the unit being read, and whether it has a letter.
    let mut prev: Option<char> = None;
    let mut lettered = false;
    for c in chars {
        if c.is_whitespace() {
            out.push(None);
            prev = None;
            continue;
        }
        let split = prev.is_some_and(|p| matches!(p, '-' | '\u{2010}' | '\u{2011}') && lettered && c.is_alphanumeric());
        if prev.is_none() || split {
            count += 1;
            lettered = false;
        }
        lettered |= c.is_alphanumeric();
        out.push(Some(count - 1));
        prev = Some(c);
    }
    out
}

/// How a letter or word moves and lights up as it is sung: «Скорость
/// прыжка» and «Скорость подсветки».
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pace {
    /// How fast a jump goes up and settles (1: Material's expressive fast
    /// spatial spring).
    jump: f32,
    /// 0 lights up smoothly .. 1 at once.
    light: f32,
    /// «Плавность»: 0 by the times exactly .. 1 smooth.
    smooth: f32,
}

impl Pace {
    fn of(prefs: &Prefs) -> Self {
        Pace {
            jump: prefs.lyrics_jump_speed.clamp(0.5, 2.0),
            light: prefs.lyrics_light_speed.clamp(0.0, 1.0),
            smooth: prefs.lyrics_smooth.clamp(0.0, 1.0),
        }
    }

    /// The seconds every motion is averaged over («Плавность»): a letter's
    /// pop or step spreads over it, so neighbouring letters move together.
    fn window(self) -> f32 {
        self.smooth * SMOOTH_WINDOW
    }

    /// Seconds a letter or word takes to light up (90 % of the way), never
    /// less than [`LIGHT_FLOOR`]; focus brightens [`FOCUS_CALM`] times slower.
    fn rise(self, style: Style) -> f32 {
        let rise = (0.5 * (1.0 - self.light) * (1.0 - self.light)).max(LIGHT_FLOOR);
        if style == Style::Focus { rise * FOCUS_CALM } else { rise }
    }

    /// The soft edge of the karaoke front at text `size`, which is how long
    /// a letter it crosses takes to light up: about a letter wide at the
    /// default «Скорость подсветки» (a letter is some 0.55 of the size), a
    /// fifth of one at once (still no hard cut), two and a half at the
    /// smoothest.
    fn edge(self, size: f32) -> f32 {
        size * (0.12 + 1.3 * (1.0 - self.light))
    }
}

/// The shortest a letter or word takes to light up, at the fastest
/// «Скорость подсветки»: three frames or so, a quick ease where lighting at
/// once was a switch.
const LIGHT_FLOOR: f32 = 0.05;
/// Focus brightens words this many times slower than the others light them.
const FOCUS_CALM: f32 = 2.2;
/// How lit (0..1 from the unsung tone to the lit one) the whole current
/// line is under Focus by syllables or words: readable, and plainly dimmer
/// than the piece being sung, which alone is lit in full.
const FOCUS_LINE: f32 = 0.3;
/// Seconds a piece takes to go back to the line's tone once it is sung.
const FOCUS_FALL: f32 = 0.25;
/// A piece stands out for at least this long, however short it is sung.
const FOCUS_HOLD: f32 = 0.25;

/// How much (0..1) a piece sung from `start` to `end` stands out at `p`
/// under Focus: up as it starts (see [`light_over`]), back down over
/// [`FOCUS_FALL`] once it is sung. Nothing stays lit behind it: Focus has
/// no fill.
fn focus_pulse(p: f32, start: f32, end: f32, rise: f32, window: f32) -> f32 {
    let end = end.max(start + FOCUS_HOLD) + 0.5 * window;
    let down = ((p - end) / FOCUS_FALL).clamp(0.0, 1.0);
    light_over(p - start, rise, window) * (1.0 - down * down * (3.0 - 2.0 * down))
}

/// The averaging window at full «Плавность», in seconds: two or three sung
/// letters.
const SMOOTH_WINDOW: f32 = 0.4;

/// «Плавность» for the lights and jumps of the wave and of Focus: the
/// window each is spread over. Karaoke has none: its front takes
/// «Плавность» as how closely it keeps to the times ([`Ease`]).
fn smooth_window(look: Look, pace: Pace) -> f32 {
    if look.style == Style::Karaoke { 0.0 } else { pace.window() }
}

/// A letter or word starts lighting up (and jumping) this long before its
/// time, so the motion lands on the voice.
const ONSET_LEAD: f32 = 0.03;

/// How lit (0..1) a letter or word is `t` seconds after its onset: from just
/// before it, up on a critically damped spring that is 90 % there after
/// `rise` seconds (at once for 0). Only ever rises.
fn light_up(t: f32, rise: f32) -> f32 {
    let t = t + ONSET_LEAD;
    if t <= 0.0 {
        return 0.0;
    }
    if rise < 0.005 {
        return 1.0;
    }
    // (1 + ωt)·e^(−ωt) falls to 0.1 at ωt ≈ 3.89.
    let w = 3.89 / rise;
    (1.0 - Spring::new(1.0, w * w).solve(1.0, 0.0, t).0).clamp(0.0, 1.0)
}

/// [`light_up`] averaged over `window` seconds around `t` («Плавность»),
/// exactly (by its integral): it starts half a window early and takes a
/// window longer, still rising only and continuous even when lighting at once.
fn light_over(t: f32, rise: f32, window: f32) -> f32 {
    if window <= 0.0 {
        return light_up(t, rise);
    }
    // ∫ light_up from its start: s + (2 + ωs)·e^(−ωs)/ω − 2/ω, or s at once.
    let area = |t: f32| {
        let s = t + ONSET_LEAD;
        if s <= 0.0 {
            0.0
        } else if rise < 0.005 {
            s
        } else {
            let w = 3.89 / rise;
            s + ((2.0 + w * s) * (-w * s).exp() - 2.0) / w
        }
    };
    ((area(t + window * 0.5) - area(t - window * 0.5)) / window).clamp(0.0, 1.0)
}

/// [`jump_height`] averaged over `window` seconds around `t`: the pop
/// spreads into a gentle rise, so the letters around the one being sung
/// rise and settle with it, a bump travelling along the line.
fn jump_over(t: f32, speed: f32, window: f32) -> f32 {
    const SAMPLES: usize = 9;
    if window <= 0.0 {
        return jump_height(t, speed);
    }
    (0..SAMPLES).map(|i| jump_height(t - window * 0.5 + window * i as f32 / (SAMPLES - 1) as f32, speed)).sum::<f32>()
        / SAMPLES as f32
}

/// How high a jump is `t` seconds after its onset, in rest heights: off the
/// line at once with a kick, over the top (about 1.6) some 70 ms later, then
/// down onto its rest height (1) on Material's expressive fast spatial
/// spring; `speed` times faster or slower.
fn jump_height(t: f32, speed: f32) -> f32 {
    let t = t + ONSET_LEAD;
    if t <= 0.0 {
        return 0.0;
    }
    let k = speed.clamp(0.25, 4.0);
    let base = nikgob_motion::tokens::EXPRESSIVE_FAST_SPATIAL;
    let spring = Spring::new(base.damping_ratio, base.stiffness * k * k);
    1.0 + spring.solve(-1.0, JUMP_KICK * k, t).0
}

/// The jump's upward speed at its onset, in rest heights a second.
const JUMP_KICK: f32 = 60.0;
/// A jump is averaged over at least this many seconds, whatever «Плавность»
/// is: its kick comes in over a few frames instead of in one.
const JUMP_EASE: f32 = 0.05;
/// A jump at speed 1 is at rest (within a hundredth of a pixel) after this.
const JUMP_SETTLE: f32 = 0.4;
/// A sung wave letter (or word) rests this high, in text sizes, at jump 1.
const RISE: f32 = 0.15;

/// How far (0..1) a wave lift is drawn on whole pixels: from none while it
/// moves to all within [`CRISP`] seconds of coming to rest (`rested`
/// seconds ago, negative while it still moves), and back to none as its
/// line's `hold` leaves 1 (it moves again then, going down).
fn crisp_lift(rested: f32, hold: f32) -> f32 {
    let smooth = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    smooth(rested / CRISP) * smooth((hold - 0.9) / 0.1)
}

/// Seconds a lift at rest takes to settle onto whole pixels.
const CRISP: f32 = 0.15;

/// How long a held note's extra glow takes to settle after it ends.
const SETTLE: f32 = 0.3;
/// Seconds a held note takes to come up to what sets it apart.
const HELD_IN: f32 = 0.35;
/// How far (0..1 from the unsung tone to the lit one) the letters of a held
/// note brighten ahead of the karaoke front while the note is held: the
/// front goes through it slowly, and the note reads as held, not as stuck.
const HELD_WARM: f32 = 0.25;

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// How much of a held note (0..1) a piece sung over `secs` seconds with
/// `letters` letters is: none at a speaking pace, whole from 1.2 s on with
/// a quarter of a second or more a letter (a long word sung at its usual
/// pace is none). No threshold: a note a little shorter is set apart a
/// little less.
fn held_note(secs: f32, letters: usize) -> f32 {
    smoothstep((secs - 0.6) / 0.6) * smoothstep((secs / letters.max(1) as f32 - 0.12) / 0.13)
}

/// How far (0..1) a note held from `start` has come up at `pos`.
fn held_in(start: f32, pos: f32) -> f32 {
    smoothstep((pos - start) / HELD_IN)
}

/// 1 while the note is held, then down to 0 over [`SETTLE`].
fn held_envelope(end: f32, pos: f32) -> f32 {
    smoothstep(1.0 - (pos - end) / SETTLE)
}

#[allow(clippy::too_many_arguments)]
/// `pos` is where the line is sung up to: the position while it is
/// current, and after that on as it fades ([`sung_pos`]), so nothing in it
/// jumps when it stops being current; it only goes back with `hold`: how
/// far (0..1) its sung pieces are lit (and its wave pieces up), 1 while it
/// is current and down with its focus after that. `timing` is the line's
/// for `look` ([`Timing::of`]).
fn timed_line(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, timing: &Timing, pos: Duration,
    focus: f32, hold: f32, size: f32, look: Look, base: Color32, lit: Color32, prefs: &Prefs, fade: Option<Fade>)
{
    let style = look.style;
    let pace = Pace::of(prefs);
    // A piece under Focus lights as fast as any other (the calm rise is the
    // whole line's, by letters).
    let rise = pace.rise(if style == Style::Focus { Style::Wave } else { style });
    let window = smooth_window(look, pace);
    let p = pos.as_secs_f32();
    // Karaoke: each run of pieces is lit up to its own soft front, whatever
    // the step ([`Front`]); a piece never lights at once.
    let sweeping = style == Style::Karaoke;
    let front = &timing.front;
    let at = pos.as_secs_f64();
    let theme = Theme::get(painter.ctx());
    let glow_color = crate::lerp_color(lit, theme.scheme.primary, 0.3);
    let ppp = super::stage::ppp(painter.ctx());
    let hold = hold.clamp(0.0, 1.0);
    // Karaoke: the sung words float up as the light comes to them
    // («Подъём спетого»), a whole number of pixels once lit, and go down
    // with the line's hold. Reduced motion keeps them on the line.
    let float_up = if sweeping && !theme.reduced_motion {
        (KARAOKE_RISE * size * prefs.lyrics_lift.clamp(0.0, 1.0) * ppp).round() / ppp * hold
    } else {
        0.0
    };
    let n = timing.glyphs.len();
    let mut inks = Vec::with_capacity(n);
    let mut glows = Vec::with_capacity(n);
    // The run of the last glyph that had one, and where its front is: a
    // glyph with no run of its own (a space, a letter past the timed text)
    // goes with the run before it.
    let mut run = (usize::MAX, f32::NEG_INFINITY);
    for (k, glyph) in timing.glyphs.iter().enumerate() {
        let syllable = glyph.map(|s| &timing.syllables[s]);
        // A note held long stands out while it is held: under karaoke, and
        // in every look by syllables. It goes with the line's hold.
        let held = syllable
            .filter(|s| (sweeping || look.by == By::Syllables) && p > s.start)
            .map_or(0.0, |s| held_note(s.end - s.start, s.letters) * held_in(s.start, p) * hold);
        let swell = syllable.map_or(0.0, |s| held * held_envelope(s.end, p));
        // When the piece this glyph goes with starts to light.
        let onset = timing.lit.get(k).copied().flatten();
        // Focus has no fill: the line is bright as a whole and the piece
        // being sung a little brighter, only while it is sung.
        let on = onset.map(|at| match piece_end(timing, k, look).filter(|_| style == Style::Focus) {
            Some(end) => FOCUS_LINE + (1.0 - FOCUS_LINE) * focus_pulse(p, at, end, rise, window),
            None => light_over(p - at, rise, window),
        });
        // Karaoke and Focus keep one baseline: sung and unsung words never
        // drift apart (that read as broken text), a word with a held note
        // too (it glows instead). Reduced motion keeps the wave down as
        // well (it only lights up).
        // How much (0..1) of the wave's crest this glyph is on.
        let mut crest = 0.0;
        // How much bigger the glyph is drawn: a wave piece swells a little
        // on its pop.
        let mut grow = 0.0;
        let lift = match onset {
            Some(at) if style == Style::Wave && !theme.reduced_motion => {
                // The jump's speed scales its smoothing window too; with
                // no window it still eases off the line ([`JUMP_EASE`]).
                let spread = (window / pace.jump).max(JUMP_EASE);
                // By syllables the whole written word goes up with them.
                let (height, t) = match glyph.filter(|_| look.by == By::Syllables) {
                    Some(s) => timing.word_jump(s, p, pace.jump, spread),
                    None => (jump_over(p - at, pace.jump, spread), p - at),
                };
                // Up while it is sung, then down to where sung text rests:
                // the crest goes along the line with the voice, and a held
                // note stays up for as long as it is held.
                // It comes down on a spring that lands a touch below where
                // sung text rests and bounces back ([`land`]).
                let sinks = wave_end(timing, k, look).map_or(f32::INFINITY, |end| end + WAVE_STAY);
                let sunk = land(p - sinks, pace.jump);
                crest = (1.0 - sunk).clamp(0.0, 1.0);
                let lift = height * (1.0 - (1.0 - WAVE_REST) * sunk) * RISE * size * prefs.lyrics_wave_jump.max(0.0) * hold;
                // The piece's own pop over the top swells it a little.
                let pop = (jump_over(p - at, pace.jump, spread) - 1.0).max(0.0);
                grow = (JUMP_SWELL * pop).min(SWELL_MAX) * crest * hold * prefs.lyrics_wave_jump.clamp(0.0, 1.0);
                // At rest on whole pixels, so a raised word is as crisp as
                // the rest; in motion to the fraction, so it never steps.
                // It eases between the two, so it never steps onto or off
                // the pixel either: off them before it sinks, onto them
                // again once it is down.
                let up = crisp_lift(t + ONSET_LEAD - (JUMP_SETTLE / pace.jump + spread), hold) * smoothstep((sinks - p) / WAVE_STAY);
                let crisp = up.max(crisp_lift(p - (sinks + WAVE_FALL / pace.jump), hold));
                lift + ((lift * ppp).round() / ppp - lift) * crisp
            }
            _ => 0.0,
        };
        if sweeping
            && let Some(r) = front.of.get(k).copied().flatten()
            && r != run.0
        {
            run = (r, front.head(r, at));
        }
        // A glyph of the line the front goes along: in `ahead` until its
        // run's front comes to it and in `behind` once the front is past.
        let head = run.1;
        // Lit, it has floated up by the whole `float_up`; under the front's
        // soft edge by as much as the light at its middle.
        let swept = |ahead: Color32, behind: Color32| {
            if head == f32::INFINITY {
                Ink::solid(behind, lift + float_up)
            } else if head == f32::NEG_INFINITY {
                Ink::solid(ahead, lift)
            } else {
                Ink { lift, base: ahead, lit: behind, head, feather: front.edge, rise: float_up, grow: 0.0 }
            }
        };
        let ink = match on {
            Some(on) => Ink::solid(crate::lerp_color(base, lit, on * hold), lift),
            // A held note warms up ahead of the front, and only ever gets
            // brighter: the front lights it whole as it goes through.
            None if sweeping => swept(crate::lerp_color(base, lit, HELD_WARM * held), lit),
            None => Ink::solid(lit, lift),
        };
        inks.push(Ink { grow, ..ink });
        let strength = focus * (0.1 + 0.06 * swell);
        let glow_ink = |strength: f32| glow_color.gamma_multiply(strength.clamp(0.0, 1.0));
        let glow_of = match (on, style) {
            // The wave's light is strongest on its crest.
            (Some(on), Style::Wave) => Ink::solid(glow_ink(strength * on * (WAVE_GLOW_REST + (WAVE_GLOW_CREST - WAVE_GLOW_REST) * crest)), lift),
            (Some(on), _) => Ink::solid(glow_ink(strength * on), lift),
            (None, Style::Karaoke) => swept(Color32::TRANSPARENT, glow_ink(strength)),
            (None, Style::Focus) if !look.words() => Ink::solid(glow_ink(strength), lift),
            _ => Ink::solid(Color32::TRANSPARENT, lift),
        };
        glows.push(Ink { grow, ..glow_of });
    }
    // Every glyph is drawn once, whole, in its own place and colour: no clip
    // edge between pieces, so no seam, cut overhang or second copy shows.
    // In a soft band at the view's edge the colours thin by height too.
    let put = |pos: egui::Pos2, g: Arc<Galley>, fallback: Color32| {
        let (pos, g) = afloat(painter, pos, &g);
        let g = through(&g, pos, None, fade).unwrap_or(g);
        painter.galley(pos, g, fallback);
    };
    if prefs.lyrics_glow && glows.iter().any(|i| i.lit.a() > 0) {
        let g = inked(g, |k| glows[k]);
        for offset in GLOW {
            put(line.min + offset, g.clone(), glow_color);
        }
    }
    put(line.min, inked(g, |k| inks[k]), lit);
}

/// When the piece glyph `k` lights with under `look` ([`Timing::lit`]) is
/// sung: its written unit by words, else its syllable.
fn piece_end(timing: &Timing, k: usize, look: Look) -> Option<f32> {
    if look.words() {
        timing.glyph_units.get(k).copied().flatten().map(|u| timing.units[u].end)
    } else {
        timing.glyphs.get(k).copied().flatten().map(|s| timing.syllables[s].end)
    }
}

/// When the piece glyph `k` rises with in the wave is sung: its letter's
/// syllable by letters, its whole written word by syllables (the word goes
/// up with them) and by words.
fn wave_end(timing: &Timing, k: usize, look: Look) -> Option<f32> {
    let s = timing.glyphs.get(k).copied().flatten();
    match look.by {
        By::Letters => s.map(|s| timing.syllables[s].end),
        By::Words => piece_end(timing, k, look).or_else(|| s.map(|s| timing.syllables[s].end)),
        By::Syllables => s.map(|s| {
            let w = timing.syllables[s].word;
            let first = timing.words[w];
            let end = timing.words.get(w + 1).copied().unwrap_or(timing.syllables.len()).max(first + 1);
            timing.syllables[end - 1].end
        }),
    }
}

/// A sung wave piece stays on the crest this long after it is sung.
const WAVE_STAY: f32 = 0.08;
/// Seconds it takes to come down from the crest to where sung text rests.
const WAVE_FALL: f32 = 0.45;
/// Where sung text rests, as a share of the crest's height.
const WAVE_REST: f32 = 0.35;
/// The glow of sung text at rest and on the crest, against the other looks'.
const WAVE_GLOW_REST: f32 = 0.6;
const WAVE_GLOW_CREST: f32 = 1.8;
/// How much a wave piece swells for each rest height its pop goes over
/// the top (the pop goes some 0.6 over), and at most.
const JUMP_SWELL: f32 = 0.12;
const SWELL_MAX: f32 = 0.08;
/// How high sung karaoke text floats at «Подъём спетого» 100 %, in text
/// sizes: two or three pixels.
const KARAOKE_RISE: f32 = 0.06;

/// How far (0..1, a little past 1 on its bounce) a wave piece has come
/// down from its crest `t` seconds after it starts down: on a spring with
/// the expressive spatial springs' bounce (fast spatial damping, default
/// spatial stiffness), which lands a touch below where sung text rests and
/// settles back within [`WAVE_FALL`]; `speed` times faster or slower.
fn land(t: f32, speed: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    let k = speed.clamp(0.25, 4.0);
    let spring = Spring::new(
        nikgob_motion::tokens::EXPRESSIVE_FAST_SPATIAL.damping_ratio,
        nikgob_motion::tokens::EXPRESSIVE_DEFAULT_SPATIAL.stiffness * k * k,
    );
    1.0 + spring.solve(-1.0, 0.0, t).0
}

/// Where the glow's copies of a line go.
const GLOW: [egui::Vec2; 3] = [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0)];

/// A soft light around the whole line `g`: copies of it in `color` at
/// `strength`, a pixel to either side and above.
fn glow(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, color: Color32, strength: f32, fade: Option<Fade>) {
    if strength <= 0.001 {
        return;
    }
    for offset in GLOW {
        fade_galley(painter, line.min + offset, g, color.gamma_multiply(strength), fade);
    }
}

/// The small line under a lyric line: its translation, else romanization.
fn sub(l: &LyricLine) -> Option<&str> {
    l.translation.as_deref().or(l.romanization.as_deref())
}

#[cfg(test)]
fn glyph_count(g: &Galley) -> usize {
    g.rows.iter().map(|r| r.row.glyphs.len()).sum::<usize>().max(1)
}

/// How one glyph is painted: raised by `lift` px and lit along the line: in
/// `lit` up to `head - feather` px of its advances (rows flattened), fading
/// to `base` at `head` (a hard edge when `feather` is 0). It floats up
/// `rise` px more as lit as its middle is, and is drawn `grow` bigger
/// about the middle of its foot.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ink {
    lift: f32,
    base: Color32,
    lit: Color32,
    head: f32,
    feather: f32,
    rise: f32,
    grow: f32,
}

impl Ink {
    fn solid(color: Color32, lift: f32) -> Self {
        Ink { lift, base: color, lit: color, head: f32::INFINITY, feather: 0.0, rise: 0.0, grow: 0.0 }
    }

    fn uniform(self) -> bool {
        self.base == self.lit || self.head == f32::INFINITY
    }

    /// How lit (0..1) the line is `x` px along its advances.
    fn light(self, x: f32) -> f32 {
        if self.head == f32::INFINITY {
            1.0
        } else if self.feather <= 0.0 {
            f32::from(x < self.head)
        } else {
            ((self.head - x) / self.feather).clamp(0.0, 1.0)
        }
    }

    fn at(self, x: f32) -> Color32 {
        if self.uniform() { self.lit } else { crate::lerp_color(self.base, self.lit, self.light(x)) }
    }
}

/// `g` with each glyph (numbered over all rows) painted as `ink` says, as
/// one galley for one draw: vertex colours instead of clip rects, which the
/// renderer rounds to whole pixels (a front that stepped, slivers of a
/// neighbour's ink in the wrong colour or height) and which cut glyphs that
/// reach past their advance.
fn inked(g: &Galley, ink: impl Fn(usize) -> Ink) -> Arc<Galley> {
    let mut out = g.clone();
    let (mut k, mut x) = (0, 0.0);
    let mut bounds = Rect::NOTHING;
    let (mut vertices, mut indices) = (0, 0);
    for placed in &mut out.rows {
        let row = &placed.row;
        let src = &row.visuals.mesh;
        let mut mesh = egui::Mesh::with_texture(src.texture_id);
        let mut firsts = Vec::with_capacity(row.glyphs.len());
        for gl in &row.glyphs {
            firsts.push(mesh.vertices.len() as u32);
            // The line's run of advances at this glyph's x is `along` + x.
            let along = x - gl.pos.x;
            x += gl.advance_width;
            let i = k;
            k += 1;
            if gl.uv_rect.is_nothing() {
                continue;
            }
            let first = gl.first_vertex as usize;
            if let Some(q) = src.vertices.get(first..first + 4) {
                ink_quad(&mut mesh, [q[0], q[1], q[2], q[3]], along, ink(i));
            }
        }
        let mut new = (**row).clone();
        // Each glyph's first vertex is that of its first piece now.
        for (gl, first) in new.glyphs.iter_mut().zip(firsts) {
            gl.first_vertex = first;
        }
        new.visuals.mesh_bounds = mesh.calc_bounds();
        new.visuals.glyph_index_start = 0;
        new.visuals.glyph_vertex_range = 0..mesh.vertices.len();
        bounds = bounds.union(new.visuals.mesh_bounds.translate(placed.pos.to_vec2()));
        vertices += mesh.vertices.len();
        indices += mesh.indices.len();
        new.visuals.mesh = mesh;
        placed.row = Arc::new(new);
    }
    out.mesh_bounds = bounds;
    out.num_vertices = vertices;
    out.num_indices = indices;
    Arc::new(out)
}

/// One glyph's quad (left top, right top, left bottom, right bottom, as
/// egui lays them out) into `mesh`, raised by `ink.lift`; where the light's
/// gradient bends inside it, it is split there, so the linearly blended
/// vertex colours draw the gradient exactly.
fn ink_quad(mesh: &mut egui::Mesh, q: [egui::epaint::Vertex; 4], along: f32, ink: Ink) {
    let (left, right) = (q[2].pos.x, q[3].pos.x);
    // The whole glyph goes up together: its line's lift and its rise by the
    // light at its middle.
    let lift = if ink.rise == 0.0 { ink.lift } else { ink.lift + ink.rise * ink.light(along + 0.5 * (left + right)) };
    let foot = q[2].pos.lerp(q[3].pos, 0.5);
    let mut cuts = [0.0f32; 4];
    let mut n = 0;
    if !ink.uniform() && right > left {
        for edge in [ink.head - ink.feather, ink.head] {
            let t = (edge - along - left) / (right - left);
            if t > cuts[n] && t < 1.0 {
                n += 1;
                cuts[n] = t;
            }
        }
    }
    n += 1;
    cuts[n] = 1.0;
    // Each piece's colour is taken just inside it, so a hard edge stays hard.
    let point = |t: f32, top: bool, inward: f32| {
        let (a, b) = if top { (q[0], q[1]) } else { (q[2], q[3]) };
        let at = a.pos.lerp(b.pos, t);
        let at = if ink.grow == 0.0 { at } else { foot + (at - foot) * (1.0 + ink.grow) };
        egui::epaint::Vertex {
            pos: at - vec2(0.0, lift),
            uv: a.uv.lerp(b.uv, t),
            color: ink.at(along + left + (right - left) * t + inward),
        }
    };
    for w in cuts[..=n].windows(2) {
        let idx = mesh.vertices.len() as u32;
        mesh.vertices.extend([point(w[0], true, 1e-3), point(w[1], true, -1e-3), point(w[0], false, 1e-3), point(w[1], false, -1e-3)]);
        mesh.indices.extend([idx, idx + 1, idx + 2, idx + 2, idx + 1, idx + 3]);
    }
}

/// The line lit left to right (row by row when it wraps) up to `head`
/// pixels of its glyphs (rows flattened), the edge of the light a soft
/// gradient.
#[cfg(test)]
fn karaoke(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, head: f32, feather: f32, base: Color32, lit: Color32) {
    if head.is_infinite() {
        painter.galley_with_override_text_color(line.min, g.clone(), lit);
        return;
    }
    painter.galley(line.min, inked(g, |_| Ink { lift: 0.0, base, lit, head, feather, rise: 0.0, grow: 0.0 }), lit);
}

/// The room a wait opens between lines, at the pane's text size: taller for
/// the thick form, a larger ring and higher waves.
fn wait_height(prefs: &Prefs) -> f32 {
    let base = if prefs.lyrics_gap_style >= 2 { 42.0 } else { 34.0 };
    let waves = if prefs.lyrics_gap_style % 2 == 1 {
        1.0 + 0.15 * (prefs.lyrics_gap_height.clamp(0.5, 2.0) - 1.0).max(0.0)
    } else {
        1.0
    };
    base * prefs.lyrics_gap_size.clamp(0.8, 1.5) * waves
}

/// The spring a wait's room opens and closes on (the lines part with it).
const WAIT_ROOM: Spring = nikgob_motion::tokens::EXPRESSIVE_SLOW_SPATIAL;
/// The spring its ring grows in on, once the room has begun to open.
const WAIT_RING: Spring = nikgob_motion::tokens::EXPRESSIVE_DEFAULT_SPATIAL;
/// How long the ring takes to leave: the first half it closes into one
/// ring, the second half that ring shrinks and fades.
const WAIT_LEAVE: f32 = 0.5;
/// With the countdown the ring is gone this long before the next line, as
/// the 3 comes in.
const WAIT_COUNT_GONE: f32 = 2.9;

/// How far (0..1) a ring has grown in `t` seconds after it starts to come,
/// for the settings' samples (the lyrics drive it by [`WAIT_RING`] itself).
fn ring_in(t: f32) -> f32 {
    if t <= 0.0 { 0.0 } else { 1.0 - WAIT_RING.solve(1.0, 0.0, t).0 }
}

/// The wait between lines: a determinate circular wavy progress indicator
/// (Material 3 Expressive). The wavy arc grows clockwise from the top as the
/// gap passes, so it reads as "how much is left"; the rest of the circle is
/// the flat track. While playing the wave travels and swells with the bass.
/// It grows in with `appear` (0..1, on its spring); before the next line
/// (or the countdown) it closes into one ring, which shrinks and fades as
/// one shape, so no end or joint ever shows. Song time drives it, so
/// pausing freezes it. `k` scales it with the text (bigger in mega mode).
#[allow(clippy::too_many_arguments)]
fn waiting(painter: &egui::Painter, line: Rect, k: f32, start: Duration, end: Duration, pos: Duration, appear: f32, playing: bool, bass: f32, edge: f32, prefs: &Prefs) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let theme = Theme::get(painter.ctx());
    let s = theme.scheme;
    let motion = !theme.reduced_motion;
    let span = end.saturating_sub(start).as_secs_f32().max(0.001);
    let progress = (pos.saturating_sub(start).as_secs_f32() / span).clamp(0.0, 1.0);
    let remaining = end.saturating_sub(pos).as_secs_f32();
    let fade = (remaining / 0.4).clamp(0.0, 1.0);
    let fade = fade * fade * (3.0 - 2.0 * fade);
    let shown = appear.clamp(0.0, 1.0);
    if edge * shown <= 0.01 {
        return;
    }
    let countdown = prefs.lyrics_countdown && remaining <= 3.0;
    let digit = remaining.ceil().clamp(1.0, 3.0);
    let age = digit - remaining;
    let reveal = if countdown { ((3.0 - remaining) / 0.25).clamp(0.0, 1.0) } else { 0.0 };
    let reveal = 1.0 - (1.0 - reveal).powi(3);
    let thick_form = prefs.lyrics_gap_style >= 2;
    let base_radius = if thick_form { 16.0 } else { 13.0 } * k;
    let c = pos2(line.left() + 2.0 * k + base_radius, line.center().y);
    if countdown {
        let ease = 1.0 - (1.0 - (age / 0.22).clamp(0.0, 1.0)).powi(3);
        let pop = if theme.reduced_motion { 1.0 } else { 1.0 + 0.22 * (1.0 - ease) };
        let alpha = edge * shown * fade * reveal * (age / 0.12).clamp(0.0, 1.0);
        let color = s.primary.gamma_multiply(alpha);
        let g = crate::typography::layout_sized(painter, style_at(28.0 * k * pop), |style, _| {
            LayoutJob::simple_singleline(format!("{digit:.0}"), style.font_id(), color)
        });
        painter.galley(Align2::CENTER_CENTER.anchor_size(c, g.size()).min, g, color);
        // The digit before fades out (shrinking a little) while this one
        // fades in, so the count never blinks between them.
        if digit < 3.0 && age < 0.12 {
            let out = 1.0 - age / 0.12;
            let prev = edge * shown * fade * reveal * out * out;
            let shrink = if theme.reduced_motion { 1.0 } else { 1.0 - 0.12 * (1.0 - out) };
            let color = s.primary.gamma_multiply(prev);
            let g = crate::typography::layout_sized(painter, style_at(28.0 * k * shrink), |style, _| {
                LayoutJob::simple_singleline(format!("{:.0}", digit + 1.0), style.font_id(), color)
            });
            painter.galley(Align2::CENTER_CENTER.anchor_size(c, g.size()).min, g, color);
        }
    }
    // Leaving: the gaps shut, the wave smooths out and the track takes the
    // indicator's colour (one closed ring), then that ring goes.
    let gone_at = if prefs.lyrics_countdown { WAIT_COUNT_GONE } else { 0.0 };
    let out = ((gone_at + WAIT_LEAVE - remaining) / WAIT_LEAVE).clamp(0.0, 1.0);
    let close = (out * 2.0).min(1.0);
    let close = close * close * (3.0 - 2.0 * close);
    let vanish = (out * 2.0 - 1.0).clamp(0.0, 1.0);
    let vanish = vanish * vanish;
    let alpha = edge * shown * (1.0 - vanish);
    if alpha <= 0.01 {
        return;
    }
    // From 60 % as it comes (the spring's small overshoot a soft pop), to
    // 60 % as it goes; reduced motion only fades it.
    let shrink = if motion { (0.6 + 0.4 * appear) * (1.0 - 0.4 * vanish) } else { 1.0 };
    let radius = base_radius * shrink;
    let thick = if thick_form { 6.0 } else { 3.0 } * k * shrink;
    // As in Compose's CircularWavyProgressIndicator: flat near 0 % and
    // 100 %, wavy in between; the crests travel along the arc (one
    // wavelengths a second) and the ring itself stays put. Song time, so
    // it is still when paused and goes on from there.
    let ramp = (progress / 0.1).min((1.0 - progress) / 0.05).clamp(0.0, 1.0);
    let height = prefs.lyrics_gap_height.clamp(0.5, 2.0);
    // About half the stroke: anything less reads as a lumpy polygon, and
    // the tip (where the wave comes out) looks straight half the time.
    // Fewer crests stand taller, more lie lower: «Количество волн» shows as
    // a few big waves or a fine ripple, not the same ring.
    let count = (8.0 / prefs.lyrics_gap_waves.clamp(3.0, 16.0)).sqrt().clamp(0.7, 1.6);
    let requested_amp = if prefs.lyrics_gap_style % 2 == 1 {
        ramp * k * shrink * height * count * (1.4 + 0.5 * if motion && playing { bass.clamp(0.0, 1.0) } else { 0.0 })
    } else { 0.0 };
    let (waves, amp) = wave_geometry(prefs.lyrics_gap_waves, radius, thick, requested_amp, height);
    let amp = amp * (1.0 - close);
    // The song's own time: it stops where it is on a pause and goes on from
    // there (the wall clock while playing and 0 when paused made the wave
    // jump at both).
    let clock = if motion { pos.as_secs_f32() } else { 0.0 };
    // One wavelength a second: a calm ripple. The ring itself stays put.
    let phase = clock * TAU;
    // Chord clearance includes both round caps and radial wave excursions.
    let clearance = thick + 2.0 * amp + 3.0 * k * shrink;
    let gap = 2.0 * (clearance / (2.0 * radius)).clamp(0.0, 1.0).asin() * (1.0 - close);
    let sweep = progress * TAU;
    let feather = 1.0 / super::stage::ppp(painter.ctx());
    let points = |from: f32, to: f32, amp: f32| -> Vec<egui::Pos2> {
        let n = ((to - from) * radius / (k * shrink * 0.6)).ceil().max(2.0) as usize;
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                // Running back from the head, like the linear wavy
                // progress: the wave comes out of the tip. The crests keep
                // their own pace on the ring; tied to the growing head they
                // stood still in a gap as many seconds long as there are
                // crests, and crawled or ran backwards in others.
                let r = radius + amp * (-waves * a - phase).sin();
                let a = a - FRAC_PI_2;
                pos2(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect()
    };
    // Opaque scheme colours (as in Material: secondary-container track,
    // primary indicator); transparency only while it fades, and each piece
    // is one mesh, so nothing in it shows darker.
    let indicator = s.primary.gamma_multiply(alpha);
    if close >= 1.0 {
        let mut ring = points(0.0, TAU, 0.0);
        ring.pop();
        painter.add(ribbon(&ring, thick, true, indicator, feather));
        return;
    }
    let arc = |from: f32, to: f32, amp: f32, color: Color32| {
        if to - from <= 0.0 {
            return;
        }
        // An arc shorter than its stroke thins away instead of ending as a
        // dot that pops (unless it is closing into the ring).
        let t = ((to - from) * radius / thick.max(0.01)).clamp(0.0, 1.0);
        let taper = t * t * (3.0 - 2.0 * t);
        let width = thick * (taper + (1.0 - taper) * close);
        painter.add(ribbon(&points(from, to, amp), width, false, color, feather));
    };
    // The flat track: the part still to wait.
    let track = crate::lerp_color(s.secondary_container, s.primary, close).gamma_multiply(alpha);
    arc(sweep + gap, TAU - gap, 0.0, track);
    // The wavy arc: the part already waited.
    arc(0.0, sweep, amp, indicator);
}

/// A stroke `width` wide along `pts`, with round ends (a ring when
/// `closed`), as one mesh with a soft rim `feather` wide. Nothing in it is
/// drawn twice, so in a see-through colour no cap, join or seam shows
/// darker (cap discs laid over a stroke did, as two dots where the ring
/// closes).
fn ribbon(pts: &[egui::Pos2], width: f32, closed: bool, color: Color32, feather: f32) -> egui::Shape {
    use std::f32::consts::PI;
    let n = pts.len();
    if n < 2 || width <= 0.0 || color.a() == 0 {
        return egui::Shape::Noop;
    }
    // Thinner than the rim: fainter instead.
    let (core, color) = if width < feather {
        (0.0, color.gamma_multiply(width / feather))
    } else {
        (width * 0.5 - feather * 0.5, color)
    };
    let rim = core + feather;
    let left = |i: usize| {
        let (a, b) = if closed {
            (pts[(i + n - 1) % n], pts[(i + 1) % n])
        } else {
            (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)])
        };
        let d = (b - a).normalized();
        vec2(-d.y, d.x)
    };
    let mut mesh = egui::Mesh::default();
    // Per point: outer left, core left, core right, outer right.
    for (i, p) in pts.iter().enumerate() {
        let l = left(i);
        mesh.colored_vertex(*p + l * rim, Color32::TRANSPARENT);
        mesh.colored_vertex(*p + l * core, color);
        mesh.colored_vertex(*p - l * core, color);
        mesh.colored_vertex(*p - l * rim, Color32::TRANSPARENT);
    }
    let segments = if closed { n } else { n - 1 };
    for i in 0..segments {
        let (a, b) = (4 * i as u32, 4 * ((i + 1) % n) as u32);
        for (x, y) in [(0, 1), (1, 2), (2, 3)] {
            mesh.add_triangle(a + x, a + y, b + x);
            mesh.add_triangle(a + y, b + y, b + x);
        }
    }
    if !closed {
        // Half discs from the left edge round the back to the right one,
        // sharing the end's own vertices.
        for (i, back) in [(0, -(pts[1] - pts[0]).normalized()), (n - 1, (pts[n - 1] - pts[n - 2]).normalized())] {
            let (p, l, at) = (pts[i], left(i), 4 * i as u32);
            let centre = mesh.vertices.len() as u32;
            mesh.colored_vertex(p, color);
            let steps = ((rim * PI * 0.5).ceil() as usize).clamp(4, 24);
            let (mut core_was, mut rim_was) = (at + 1, at);
            for j in 1..=steps {
                let (c, r) = if j == steps {
                    (at + 2, at + 3)
                } else {
                    let a = PI * j as f32 / steps as f32;
                    let d = l * a.cos() + back * a.sin();
                    let c = mesh.vertices.len() as u32;
                    mesh.colored_vertex(p + d * core, color);
                    mesh.colored_vertex(p + d * rim, Color32::TRANSPARENT);
                    (c, c + 1)
                };
                mesh.add_triangle(centre, core_was, c);
                mesh.add_triangle(core_was, c, r);
                mesh.add_triangle(core_was, r, rim_was);
                (core_was, rim_was) = (c, r);
            }
        }
    }
    egui::Shape::mesh(mesh)
}

/// The crests and their height for a ring of `radius` and `stroke`: at
/// most `height` × 12 % of a wavelength, and never so high that the thick
/// stroke folds over itself.
fn wave_geometry(waves: f32, radius: f32, stroke: f32, requested_amp: f32, height: f32) -> (f32, f32) {
    let waves = waves.clamp(3.0, 16.0).round();
    let wavelength = std::f32::consts::TAU * radius / waves;
    // Also bound the curvature so a thick stroke never folds over itself:
    // for r = R + A·sin(nθ) the crest/trough curvature is ≈ (R ± A·n²)/R²,
    // which must stay under 2/stroke.
    let fold = (2.0 * radius * radius / stroke - radius).max(0.0) / (waves * waves + 1.0);
    let amp = requested_amp.max(0.0).min(wavelength * 0.12 * height.clamp(0.5, 2.0)).min(fold);
    (waves, amp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: f32, end: Option<f32>, text: &str) -> LyricLine {
        LyricLine { start: Duration::from_secs_f32(start), end: end.map(Duration::from_secs_f32), text: text.into(), ..Default::default() }
    }

    fn interludes(lines: &[LyricLine]) -> Vec<(f32, f32, bool)> {
        display_lines(lines).iter().filter_map(|item| match *item {
            DisplayLine::Interlude { start, end, duet } => Some((start.as_secs_f32(), end.as_secs_f32(), duet)),
            _ => None,
        }).collect()
    }

    /// A preset's tile, still or moving, paints its sample inside the tile:
    /// every shape and its clip are on the tile after the scene is scaled
    /// down to it, and the sung line spans most of it.
    #[test]
    fn a_tile_sample_is_scaled_into_its_tile() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        Theme::default().install(&ctx);
        let tile = Rect::from_min_size(pos2(200.0, 100.0), vec2(112.0, 72.0));
        for (k, prefs) in [Prefs::first_start(), Prefs { lyrics_style: 2, ..Prefs::first_start() }].into_iter().enumerate() {
            for still in [Some(1.25), None] {
                let mut clips = Vec::new();
                let mut bounds = Rect::NOTHING;
                for f in 0..3 {
                    let input = egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                        time: Some(1.0 + f64::from(f) * 0.3),
                        ..Default::default()
                    };
                    let out = ctx.run_ui(input, |ui| {
                        let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(tile));
                        tui.set_clip_rect(ui.clip_rect().intersect(tile));
                        tile_preview(&tui, tile, &prefs, still);
                    });
                    let mut out = out;
                    out.textures_delta.clear();
                    clips = out.shapes.iter().filter(|c| !matches!(c.shape, egui::Shape::Noop)).map(|c| c.clip_rect).collect();
                    bounds = out.shapes.iter().filter(|c| matches!(c.shape, egui::Shape::Text(_))).fold(Rect::NOTHING, |b, c| b.union(c.shape.visual_bounding_rect()));
                }
                assert!(!clips.is_empty(), "{k} {still:?}: nothing painted");
                for c in &clips {
                    assert!((c.min - tile.min).length() < 1.0 && (c.max - tile.max).length() < 1.0, "{k} {still:?}: clip {c:?} is not the tile {tile:?}");
                }
                assert!(bounds.width() > tile.width() * 0.6 && bounds.width() < tile.width() * 1.05, "{k} {still:?}: text {bounds:?}");
            }
        }
    }

    /// «Волна» with the calm fallback showed a word-timed text as karaoke:
    /// word times are real, so the letters wave over them; only a text with
    /// line times alone becomes bright.
    #[test]
    fn the_wave_goes_over_word_timing_and_only_line_timing_falls_back() {
        let p = Prefs { lyrics_style: 2, wave_syllables_only: true, ..Prefs::default() };
        let mut worded = line(1.0, Some(2.0), "Calling through");
        worded.words = vec![
            nikgob_view::LyricWord { start: Duration::from_secs(1), end: Duration::from_millis(1500), text: "Calling ".into() },
            nikgob_view::LyricWord { start: Duration::from_millis(1500), end: Duration::from_secs(2), text: "through".into() },
        ];
        let lines_only = line(1.0, Some(2.0), "Calling through");
        assert!(style_for(&p, std::slice::from_ref(&worded)).style == Style::Wave);
        assert!(style_for(&p, std::slice::from_ref(&lines_only)).style == Style::Focus);
        let mut syllables = worded.clone();
        syllables.words[0].text = "Call".into();
        assert!(style_for(&p, &[syllables]).style == Style::Wave);
        let p = Prefs { wave_syllables_only: false, ..p };
        assert!(style_for(&p, std::slice::from_ref(&worded)).style == Style::Wave);
        assert!(style_for(&p, std::slice::from_ref(&lines_only)).style == Style::Wave);
        let p = Prefs { lyrics_style: 1, ..p };
        assert!(style_for(&p, std::slice::from_ref(&worded)).style == Style::Focus);
        let p = Prefs { lyrics_style: 0, ..p };
        assert!(style_for(&p, &[worded]).style == Style::Karaoke);
    }

    /// KOMA, an empty stamp (a wait), then a line wrapping in two rows: the
    /// folded wait sat on the long line's top and took its pointer too.
    #[test]
    fn only_one_item_takes_the_pointer_and_a_folded_wait_none() {
        let reach = 9.0;
        // Folded: the wait has the long line's top and no room.
        let folded = [Some((0.0, 34.0, 1.0)), Some((52.0, 42.0, 0.0)), Some((52.0, 68.0, 1.0))];
        for y in [44.0, 52.0, 60.0, 80.0, 128.0] {
            assert_eq!(hit_target(&folded, y, reach), Some(2), "y={y}");
        }
        assert_eq!(hit_target(&folded, 20.0, reach), Some(0));
        assert_eq!(hit_target(&folded, 140.0, reach), None);
        // Half unfolded: the wait owns its half slot, the line moved down
        // by half of the wait's room (slot and gap).
        let half = [Some((0.0, 34.0, 1.0)), Some((52.0, 21.0, 0.5)), Some((82.0, 68.0, 1.0))];
        assert_eq!(hit_target(&half, 60.0, reach), Some(1));
        assert_eq!(hit_target(&half, 90.0, reach), Some(2));
        // Between them both reach: the nearer slot wins, never both.
        assert_eq!(hit_target(&half, 76.0, reach), Some(1));
        assert_eq!(hit_target(&half, 79.0, reach), Some(2));
        // A background voice hugging its line: the one the pointer is in.
        let hug = [Some((0.0, 40.0, 1.0)), Some((30.0, 30.0, 1.0))];
        assert_eq!(hit_target(&hug, 20.0, reach), Some(0));
        assert_eq!(hit_target(&hug, 50.0, reach), Some(1));
        assert_eq!(hit_target(&hug, 32.0, reach), Some(0), "both contain it: the nearer centre");
        assert_eq!(hit_target(&hug, 36.0, reach), Some(1));
        // Items not drawn take nothing.
        assert_eq!(hit_target(&[None, Some((100.0, 20.0, 1.0))], 10.0, reach), None);
    }

    fn word(start: f32, end: f32, text: &str) -> nikgob_view::LyricWord {
        nikgob_view::LyricWord { start: Duration::from_secs_f32(start), end: Duration::from_secs_f32(end), text: text.into() }
    }

    /// The timing of `l` laid out as `g` for `look`, at the default settings
    /// and 26 px.
    fn timing(l: &LyricLine, g: &Galley, look: Look) -> Timing {
        Timing::of(l, g, look, Pace::of(&Prefs::default()), 26.0).unwrap()
    }

    /// The units of a timed line as written, with their times.
    fn units_of(painter: &egui::Painter, l: &LyricLine) -> Vec<(String, f32, f32)> {
        let g = fixed_layout(painter, &l.text, 26.0);
        let t = Timing::new(l, &g).unwrap();
        let mut out: Vec<(String, f32, f32)> = t.units.iter().map(|u| (String::new(), u.start, u.end)).collect();
        for (gl, u) in g.rows.iter().flat_map(|r| &r.row.glyphs).zip(&t.glyph_units) {
            if let Some(u) = u {
                out[*u].0.push(gl.chr);
            }
        }
        out
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn written_units_split_hyphenated_words_as_written() {
        let text = "Я делаю do-dodge back — уклоняюсь от атаки";
        let units = written_units(text.chars());
        let mut words: Vec<String> = Vec::new();
        for (c, u) in text.chars().zip(&units) {
            if let Some(u) = *u {
                if words.len() <= u {
                    words.push(String::new());
                }
                words[u].push(c);
            }
        }
        assert_eq!(words, ["Я", "делаю", "do-", "dodge", "back", "—", "уклоняюсь", "от", "атаки"]);
        // A leading or doubled hyphen, or a dash between words, stays put.
        let parts = |t: &str| written_units(t.chars()).into_iter().flatten().max().map_or(0, |n| n + 1);
        assert_eq!(parts("-то"), 1);
        assert_eq!(parts("кто-то"), 2);
        assert_eq!(parts("a--"), 1);
        assert_eq!(parts("1-2"), 2);
    }

    /// Word timing keeps «do-dodge» one timed word: its parts share the
    /// word's time by their letters, weighted as everywhere: «do-» is
    /// d 1 + o 2.5 + «-» 0.25, «dodge» 1 + 2.5 + 1 + 1 + 2.5.
    #[test]
    fn words_split_at_the_hyphen_share_the_word_time_by_letters() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "Я делаю do-dodge back");
            l.words = vec![word(1.0, 1.2, "Я "), word(1.2, 2.0, "делаю "), word(2.0, 2.8, "do-dodge "), word(3.0, 3.5, "back")];
            let u = units_of(painter, &l);
            assert_eq!(u.iter().map(|u| u.0.as_str()).collect::<Vec<_>>(), ["Я", "делаю", "do-", "dodge", "back"]);
            let cut = 2.0 + 0.8 * 3.75 / 11.75;
            assert!(close(u[2].1, 2.0) && close(u[2].2, cut), "{:?}", u[2]);
            assert!(close(u[3].1, cut) && close(u[3].2, 2.8), "{:?}", u[3]);
            assert!(close(u[4].1, 3.0) && close(u[4].2, 3.5));
        });
    }

    /// Syllables light with their unit: «dodge» at its first syllable,
    /// never «dod» then «ge».
    #[test]
    fn syllables_of_one_unit_light_together() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "do-dodge back");
            l.words = vec![word(1.0, 1.4, "do-"), word(1.5, 1.8, "dod"), word(1.9, 2.4, "ge "), word(2.5, 3.0, "back")];
            let u = units_of(painter, &l);
            let want = [("do-", 1.0, 1.4), ("dodge", 1.5, 2.4), ("back", 2.5, 3.0)];
            assert_eq!(u.len(), want.len());
            for (got, want) in u.iter().zip(want) {
                assert!(got.0 == want.0 && close(got.1, want.1) && close(got.2, want.2), "{got:?}");
            }
            // Lit as one: the whole unit's light comes from its start.
            let pace = Pace::of(&Prefs::default());
            let (karaoke, focus) = (pace.rise(Style::Karaoke), pace.rise(Style::Focus));
            let lit = |rise: f32, pos: f32| light_up(pos - 1.5, rise);
            assert_eq!(lit(focus, 1.4), 0.0);
            assert!(lit(focus, 1.6) > 0.5);
            assert!(lit(focus, 1.9) > 0.99);
            // Karaoke fills a word quicker than focus brightens it.
            assert!(lit(karaoke, 1.55) > lit(focus, 1.55));
            assert!(lit(karaoke, 1.7) > 0.99);
            for rise in [karaoke, focus, 0.0, 0.5] {
                let mut was = 0.0;
                for ms in 1450..1900 {
                    let v = lit(rise, ms as f32 / 1000.0);
                    assert!(v >= was && v <= 1.0);
                    was = v;
                }
            }
        });
    }

    /// Line times only: «Спокойный Фокус» makes the line bright as a
    /// whole under every animation and every unit; otherwise the words are
    /// paced from the line, split as written. Real times keep the look.
    #[test]
    fn every_look_and_unit_follows_the_line_only_fallback() {
        use nikgob_view::{LYRICS_BY_LETTERS, LYRICS_BY_SYLLABLES, LYRICS_BY_WORDS};
        let lines_only = [line(1.0, Some(3.0), "do-dodge back")];
        let mut worded = lines_only[0].clone();
        worded.words = vec![word(1.0, 2.0, "do-dodge "), word(2.0, 3.0, "back")];
        let mut syllabled = lines_only[0].clone();
        syllabled.words = vec![word(1.0, 1.4, "do-"), word(1.4, 1.7, "dod"), word(1.7, 2.0, "ge "), word(2.0, 3.0, "back")];
        assert_eq!(LyricsSync::of(std::slice::from_ref(&syllabled)), Some(LyricsSync::Syllable));
        for (anim, style) in [(0, Style::Karaoke), (1, Style::Focus), (2, Style::Wave)] {
            for (unit, by) in [(LYRICS_BY_LETTERS, By::Letters), (LYRICS_BY_SYLLABLES, By::Syllables), (LYRICS_BY_WORDS, By::Words)] {
                let calm = Prefs { lyrics_style: anim, lyrics_unit: unit, wave_syllables_only: true, ..Prefs::default() };
                let case = format!("style {anim}, unit {unit}");
                assert_eq!(style_for(&calm, &lines_only), CALM, "{case}");
                let paced = Prefs { wave_syllables_only: false, ..calm.clone() };
                assert_eq!(style_for(&paced, &lines_only), Look { style, by }, "{case}");
                for timed in [&worded, &syllabled] {
                    assert_eq!(style_for(&calm, std::slice::from_ref(timed)), Look { style, by }, "{case}");
                    assert_eq!(style_for(&paced, std::slice::from_ref(timed)), Look { style, by }, "{case}");
                }
            }
        }
        // The old fourth animation reads as the wave by words here too.
        let old = Prefs { lyrics_style: 3, wave_syllables_only: true, ..Prefs::default() };
        assert_eq!(style_for(&old, &lines_only), CALM);
        assert_eq!(style_for(&old, std::slice::from_ref(&worded)), Look { style: Style::Wave, by: By::Words });
        // No text is not a guessed one.
        assert_eq!(style_for(&Prefs::default(), &[]), Look::of(&Prefs::default()));
        with_painter(|painter| {
            let u = units_of(painter, &paced_words(&lines_only[0], None));
            assert_eq!(u.iter().map(|u| u.0.as_str()).collect::<Vec<_>>(), ["do-", "dodge", "back"]);
            // 8 + 4 letters over the line's two seconds; inside «do-dodge»
            // its parts by their weighted letters (3.75 and 8).
            assert!(close(u[1].1, 1.0 + 2.0 * 8.0 / 12.0 * 3.75 / 11.75), "{:?}", u[1]);
            assert!(close(u[2].1, 1.0 + 2.0 * 8.0 / 12.0), "{:?}", u[2]);
        });
    }

    /// The unit applies to each animation (by syllables unless chosen
    /// otherwise); the old fourth animation (`lyrics_style` 3, «По
    /// словам») reads as the wave by words.
    #[test]
    fn the_unit_goes_with_every_animation_and_the_old_words_style_is_the_wave_by_words() {
        use nikgob_view::{LYRICS_BY_LETTERS, LYRICS_BY_SYLLABLES, LYRICS_BY_WORDS};
        for (anim, style) in [(0, Style::Karaoke), (1, Style::Focus), (2, Style::Wave)] {
            for (unit, by) in [(LYRICS_BY_LETTERS, By::Letters), (LYRICS_BY_SYLLABLES, By::Syllables), (LYRICS_BY_WORDS, By::Words)] {
                let p = Prefs { lyrics_style: anim, lyrics_unit: unit, ..Prefs::default() };
                assert_eq!(Look::of(&p), Look { style, by });
            }
        }
        assert_eq!(Look::of(&Prefs::default()).by, By::Syllables);
        let old = Prefs { lyrics_style: 3, lyrics_by_words: false, lyrics_unit: LYRICS_BY_LETTERS, ..Prefs::default() };
        assert_eq!(Look::of(&old), Look { style: Style::Wave, by: By::Words });
        let mut moved = old.clone();
        moved.upgrade_lyrics_style();
        assert_eq!((moved.lyrics_style, moved.lyrics_by_words, moved.lyrics_unit), (2, true, LYRICS_BY_WORDS));
        assert_eq!(Look::of(&moved), Look::of(&old));
        // Prefs from before syllables: letters (the old default) move to
        // syllables, words chosen stay.
        for (by_words, unit) in [(false, LYRICS_BY_SYLLABLES), (true, LYRICS_BY_WORDS)] {
            let mut p = Prefs { lyrics_by_words: by_words, lyrics_unit: LYRICS_BY_LETTERS, ..Prefs::default() };
            p.upgrade_lyrics(3);
            assert_eq!(p.lyrics_unit, unit);
            // Chosen since, it stays.
            let mut q = Prefs { lyrics_unit: LYRICS_BY_LETTERS, ..p };
            q.upgrade_lyrics(nikgob_view::LYRICS_REV);
            assert_eq!(q.lyrics_unit, LYRICS_BY_LETTERS);
        }
    }

    /// A wave letter or word jumps at its onset: still before it, off the
    /// line at once, over the top within a tenth of a second, then down onto
    /// its rest height on the spring; the speed setting scales the time.
    #[test]
    fn a_wave_piece_jumps_on_its_onset_and_settles() {
        assert_eq!(jump_height(-0.1, 1.0), 0.0);
        assert_eq!(jump_height(-ONSET_LEAD, 1.0), 0.0);
        // Up at once: a fifth of the way within 10 ms of the onset lead.
        assert!(jump_height(-ONSET_LEAD + 0.01, 1.0) > 0.2);
        let peak = |speed: f32| {
            (0..400).map(|ms| (ms, jump_height(ms as f32 / 1000.0 - ONSET_LEAD, speed)))
                .fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a })
        };
        let (at, top) = peak(1.0);
        assert!(at <= 100 && top > 1.4 && top < 1.8, "{at} ms, {top}");
        // At rest after its settle time, whatever the speed.
        for speed in [0.5, 1.0, 2.0] {
            let rest = jump_height(JUMP_SETTLE / speed - ONSET_LEAD, speed);
            assert!((rest - 1.0).abs() < 0.005, "{speed}: {rest}");
            assert!((jump_height(5.0, speed) - 1.0).abs() < 1e-4);
        }
        // Twice the speed: the same jump in half the time.
        assert!((peak(2.0).1 - top).abs() < 0.02 && peak(2.0).0 * 2 <= at + 4);
        assert!(peak(0.5).0 > at);
    }

    /// The speed settings: defaults light a word in about 60 ms and the
    /// karaoke edge is about a letter wide; the extremes are a quick ease
    /// (never a hard cut) and slow.
    #[test]
    fn the_light_speed_runs_from_smooth_to_at_once() {
        let p = Prefs::default();
        assert!((p.lyrics_jump_speed - 1.0).abs() < 1e-6);
        let pace = Pace::of(&p);
        assert!(pace.rise(Style::Wave) > 0.04 && pace.rise(Style::Wave) < 0.08);
        assert!(pace.rise(Style::Focus) > pace.rise(Style::Karaoke));
        assert!((pace.edge(26.0) - 26.0 * 0.575).abs() < 0.5);
        // The fastest is a quick ease, never a switch: no frame at 240 a
        // second takes it more than a fifth of the way.
        let at_once = Pace::of(&Prefs { lyrics_light_speed: 1.0, ..p.clone() });
        assert_eq!(at_once.rise(Style::Karaoke), LIGHT_FLOOR);
        assert!(at_once.edge(26.0) > 2.0 && at_once.edge(26.0) < pace.edge(26.0));
        assert!(at_once.rise(Style::Wave) <= pace.rise(Style::Wave));
        let mut was = 0.0;
        for i in 0..=120 {
            let v = light_up(-ONSET_LEAD + i as f32 / 240.0, at_once.rise(Style::Wave));
            assert!(v >= was && v - was < 0.2, "{i}: {was} → {v}");
            was = v;
        }
        assert!(light_up(-ONSET_LEAD + LIGHT_FLOOR, at_once.rise(Style::Wave)) > 0.85 && was > 0.999);
        let smooth = Pace::of(&Prefs { lyrics_light_speed: 0.0, ..p.clone() });
        assert!(smooth.rise(Style::Wave) >= 0.5 && smooth.edge(26.0) > 2.0 * pace.edge(26.0));
        assert!(light_up(0.1, smooth.rise(Style::Wave)) < light_up(0.1, pace.rise(Style::Wave)));
        // Out-of-range values are held to the slider's range.
        let wild = Pace::of(&Prefs { lyrics_jump_speed: 9.0, lyrics_light_speed: -1.0, ..p });
        assert_eq!((wild.jump, wild.light), (2.0, 0.0));
    }

    /// Every look paints a word-timed line (current, fading out, and not
    /// current) without panicking, by letters and by words, also wrapped.
    #[test]
    fn every_look_paints_a_timed_line() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "Я делаю do-dodge back");
            l.words = vec![word(1.0, 1.2, "Я "), word(1.2, 2.0, "делаю "), word(2.0, 2.4, "do-"), word(2.4, 2.8, "dod"), word(2.8, 3.0, "ge "), word(3.0, 3.5, "back")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let rect = Rect::from_min_size(pos2(0.0, 0.0), g.size());
            for style in [Style::Karaoke, Style::Focus, Style::Wave] {
                for by in [By::Letters, By::Syllables, By::Words] {
                    let look = Look { style, by };
                    let t = timing(&l, &g, look);
                    let prefs = Prefs { lyrics_glow: true, ..Prefs::default() };
                    for (at, current, focus) in [(0.5, true, 1.0), (2.5, true, 1.0), (2.9, false, 0.4), (5.0, false, 0.0)] {
                        timed_line(painter, &g, rect, &t, Duration::from_secs_f32(at), focus, if current { 1.0 } else { focus }, 26.0,
                            look, Color32::GRAY, Color32::WHITE, &prefs, None);
                    }
                }
            }
        });
    }

    /// Soft edges: the text is whole in the middle of the view and thins
    /// over a band at either edge, to nothing at the edge itself, by height
    /// alone and with no step.
    #[test]
    fn soft_edges_fade_over_the_band_only() {
        let rect = Rect::from_min_size(pos2(0.0, 100.0), vec2(600.0, 500.0));
        let size = 26.0;
        let soft = Soft::of(&Prefs::default());
        let fade = Fade::of(rect, soft);
        let band = soft.band(rect);
        assert!((40.0..=200.0).contains(&band), "{band}");
        // Whole from the bands' inner ends on.
        assert_eq!(fade.at(rect.top() + band), 1.0);
        assert_eq!(fade.at(rect.center().y), 1.0);
        assert_eq!(fade.at(rect.bottom() - band), 1.0);
        assert!(fade.clear(rect.top() + band + 0.5, rect.bottom() - band - 0.5));
        assert!(!fade.clear(rect.top() + band - 1.0, rect.center().y));
        assert!(!fade.clear(rect.center().y, rect.bottom() - band + 1.0));
        // Nothing at the edges and past them.
        for y in [rect.top(), rect.bottom(), rect.top() - 30.0, rect.bottom() + 30.0] {
            assert_eq!(fade.at(y), 0.0, "{y}");
        }
        // Rising all the way in from an edge with no step, the same from
        // the top and from the bottom.
        let mut last = 0.0;
        for i in 0..=(band as i32 * 4) {
            let d = i as f32 * 0.25;
            let a = fade.at(rect.top() + d);
            assert!(a >= last - 1e-6 && a - last < 0.02, "{d}: {last} to {a}");
            assert!((a - fade.at(rect.bottom() - d)).abs() < 1e-3, "{d}");
            last = a;
        }
        // A line comes in gradually over the whole band: faint a quarter
        // of the way in, about half there halfway, mostly there at three
        // quarters (not whole until just before the edge and then gone).
        assert!(fade.at(rect.top() + band * 0.25) < 0.3);
        let half_in = fade.at(rect.top() + band * 0.5);
        assert!((0.3..0.7).contains(&half_in), "{half_in}");
        assert!(fade.at(rect.top() + band * 0.75) > 0.7);
        // The blur: none in the middle and in the inner half of the band,
        // growing to the edge, by the letters' size and the strength.
        let sized = Fade::of(rect, soft.text(40.0, 30.0));
        assert_eq!(sized.blur_at(rect.center().y), 0.0);
        assert_eq!(sized.blur_at(rect.top() + band * 0.5), 0.0);
        assert!(sized.blur_at(rect.top() + band * 0.1) > sized.blur_at(rect.top() + band * 0.3));
        assert!(sized.blur_at(rect.top()) > 2.0 && sized.blur_at(rect.top()) < 8.0, "{}", sized.blur_at(rect.top()));
        assert_eq!(Fade::of(rect, Soft { strength: 0.0, ..soft.text(40.0, 30.0) }).blur_at(rect.top()), 0.0);
        // The band takes at least about a line, whatever the depth.
        assert!(Soft { depth: 0.08, ..soft }.text(80.0, 30.0).band(rect) >= 100.0);
        // A deeper band reaches further in; a gentler fade keeps the text
        // brighter at the same place.
        let near = rect.top() + 45.0;
        let of = |soft: Soft| Fade::of(rect, soft);
        assert!(of(Soft { depth: 0.3, ..soft }).at(near) < of(Soft { depth: 0.1, ..soft }).at(near));
        let mid = rect.top() + band * 0.5;
        assert!(of(Soft { fade: 0.0, ..soft }).at(mid) > of(Soft { fade: 1.0, ..soft }).at(mid));
        // A short view keeps its middle whole.
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 160.0));
        let short = Fade::of(small, Soft { depth: 0.1, ..soft });
        assert_eq!(short.at(80.0), 1.0);
        assert!(short.clear(60.0, 90.0));
        // The current line rests at 40 % of the whole view, clear of the
        // top band even when the band is deep, never below the middle.
        let (half, ink) = (17.0, size * 0.4);
        assert_eq!(rest_at(rect, 0.4, None, half, ink), 200.0);
        assert_eq!(rest_at(rect, 0.4, Some(soft), half, ink), 200.0);
        let deepest = Soft { depth: 0.4, ..soft };
        let rest = rest_at(rect, 0.4, Some(deepest), half, ink);
        assert!(rest > 200.0 && rest <= 250.0);
        assert!(of(deepest).clear(rect.top() + rest - half, rect.top() + rest + half));
    }

    /// A line near the edge gets copies of its glyphs spread around them
    /// (its blur); one in the middle and one in the inner half of a band
    /// keep their glyphs alone, and a strength of nothing blurs nothing.
    #[test]
    fn soft_edges_blur_only_right_at_the_edge() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let g = fixed_layout(ui.painter(), "Строка номер", 30.0);
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 600.0));
            let soft = Soft::of(&Prefs::default()).text(60.0, 30.0);
            let fade = Fade::of(rect, soft);
            let count = |g: &Galley| g.rows.iter().map(|r| r.row.visuals.mesh.vertices.len()).sum::<usize>();
            let base = count(&g);
            assert!(base > 0);
            let at = |y: f32, fade: Fade| count(&faded(&g, pos2(0.0, y), Some(Color32::WHITE), fade));
            assert_eq!(at(285.0, fade), base, "the middle");
            assert_eq!(at(fade.band * 0.6, fade), base, "the inner half of the band");
            assert_eq!(at(2.0, fade), base * (1 + BLUR_TAPS.len()), "at the edge");
            assert_eq!(at(2.0, Fade::of(rect, Soft { strength: 0.0, ..soft })), base);
        });
    }

    /// With the soft edges on every line in the view is still drawn, with
    /// its own glyphs (sharp, no picture of it), and a line in the top
    /// band thins towards the edge.
    #[test]
    fn soft_edges_keep_every_line_in_the_view() {
        let view = Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0));
        // Each line drawn whole inside the view: its text, its top, and
        // the least and the most opaque of its glyph vertices.
        let drawn = |soft: bool| {
            let ctx = egui::Context::default();
            let mut fonts = egui::FontDefinitions::default();
            crate::typography::install_fonts(&mut fonts);
            ctx.set_fonts(fonts);
            let mut state = ViewState::default();
            state.prefs = Prefs { lyrics_soft_edges: soft, lyrics_soft_depth: 0.3, lyrics_glow: false, ..Prefs::default() };
            state.player.status = Status::Paused;
            state.player.position = Duration::from_secs(40);
            state.lyrics.lines = (0..20).map(|i| line(i as f32 * 4.0, None, &format!("Строка номер {i}"))).collect();
            let mut lines: Vec<(String, f32, u8, u8)> = Vec::new();
            for frame in 0..16 {
                let mut output = ctx.run_ui(egui::RawInput {
                    screen_rect: Some(view),
                    time: Some(frame as f64 * 0.25), ..Default::default()
                }, |ui| {
                    show(ui, &state, 0.0, false, &mut Vec::new());
                });
                output.textures_delta.clear();
                lines.clear();
                for shape in &output.shapes {
                    let egui::Shape::Text(text) = &shape.shape else { continue };
                    if !text.galley.job.text.starts_with("Строка номер") {
                        continue;
                    }
                    let ink = text.galley.mesh_bounds.translate(text.pos.to_vec2());
                    if ink.top() < view.top() + 2.0 || ink.bottom() > view.bottom() - 2.0 {
                        continue;
                    }
                    let mut alphas = Vec::new();
                    for row in &text.galley.rows {
                        let v = &row.row.visuals;
                        assert_eq!(v.mesh.texture_id, egui::TextureId::default(), "drawn from the font's own glyphs");
                        alphas.extend(v.mesh.vertices[v.glyph_vertex_range.clone()].iter().map(|v| text.override_text_color.unwrap_or(v.color).a()));
                    }
                    let (least, most) = (alphas.iter().copied().min().unwrap_or(0), alphas.iter().copied().max().unwrap_or(0));
                    lines.push((text.galley.job.text.clone(), ink.top(), least, most));
                }
            }
            lines.sort_by(|a, b| a.1.total_cmp(&b.1));
            lines
        };
        let (plain, soft) = (drawn(false), drawn(true));
        assert!(plain.len() >= 8, "{plain:?}");
        // The same lines are there with the soft edges as without.
        let texts = |lines: &[(String, f32, u8, u8)]| lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&soft), texts(&plain));
        assert!(soft.iter().all(|l| l.3 > 0), "{soft:?}");
        // The topmost one lies in the band: thinner at its top than at its
        // bottom. One in the middle is whole.
        let top = &soft[0];
        assert!(top.2 < top.3, "{top:?}");
        let most = soft.iter().map(|l| l.3).max().unwrap();
        assert!(top.3 < most, "{top:?} against {most}");
    }

    /// The mega view's control has its own row under the lyrics, with air
    /// between: no line or hover lies under it.
    #[test]
    fn the_control_has_a_row_of_its_own_under_the_lyrics() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 500.0));
        let (lyrics, row) = control_row(area);
        let row = row.unwrap();
        assert_eq!(row.bottom(), area.bottom());
        assert_eq!(row.height(), Size::Xs.height());
        assert!(lyrics.bottom() + CONTROL_GAP <= row.top() + 1e-3);
        assert!(!lyrics.intersects(row));
    }


    /// The lead moves what is painted, within ±0.5 s and never before 0;
    /// the default lights a little before the voice.
    #[test]
    fn the_lead_moves_the_painted_position_only_within_its_range() {
        let at = |s: f32| Duration::from_secs_f32(s);
        let led_s = |pos: f32, lead: f32| led(at(pos), lead).as_secs_f32();
        assert!(close(led_s(10.0, 0.1), 10.1));
        assert!(close(led_s(10.0, -0.25), 9.75));
        assert!(close(led_s(10.0, 3.0), 10.5));
        assert!(close(led_s(10.0, -3.0), 9.5));
        assert_eq!(led(at(0.2), -0.5), Duration::ZERO);
        assert!((Prefs::default().lyrics_lead - nikgob_view::LYRICS_LEAD).abs() < 1e-6);
        // A line starting at 10 s is current 0.1 s before the voice.
        let lines = [line(5.0, Some(9.0), "раз"), line(10.0, Some(12.0), "два")];
        let items = display_lines(&lines);
        let cur = |pos| current_item(&lines, &items, pos).map(|i| items[i]);
        assert_eq!(cur(led(at(9.95), 0.1)), Some(DisplayLine::Line(1)));
        assert_ne!(cur(led(at(9.95), 0.0)), Some(DisplayLine::Line(1)));
    }

    #[test]
    fn intro_waits_for_the_first_main_voice_and_uses_its_side() {
        let mut first = line(2.5, None, "первый");
        first.duet = true;
        assert_eq!(interludes(&[first]), [(0.0, 2.5, true)]);
        assert!(interludes(&[line(2.49, None, "первый")]).is_empty());
    }

    #[test]
    fn gaps_use_known_end_or_speaking_pace() {
        assert_eq!(interludes(&[line(0.0, Some(2.0), "a"), line(6.0, None, "b")]), [(2.0, 6.0, false)]);
        assert_eq!(interludes(&[line(0.0, None, "a"), line(6.0, None, "b")]), [(0.8, 6.0, false)]);
        assert!(interludes(&[line(0.0, Some(2.0), "a"), line(5.99, None, "b")]).is_empty());
    }

    #[test]
    fn explicit_empty_stamps_replace_computed_gaps_and_intros() {
        let lines = [line(0.0, Some(1.0), "a"), line(2.0, None, ""), line(3.0, None, " "), line(10.0, None, "b")];
        assert_eq!(interludes(&lines), [(2.0, 10.0, false)]);
        assert_eq!(display_lines(&lines).len(), 3);
        assert_eq!(interludes(&[line(0.0, None, ""), line(3.0, None, "a")]), [(0.0, 3.0, false)]);
    }

    #[test]
    fn background_singing_cancels_waiting_even_if_it_started_before_the_gap() {
        let mut bg = line(1.0, Some(4.0), "эхо");
        bg.background = true;
        let lines = [line(0.0, Some(2.0), "a"), bg, line(10.0, None, "b")];
        assert!(interludes(&lines).is_empty());
        assert_eq!(display_lines(&lines), [DisplayLine::Line(0), DisplayLine::Line(1), DisplayLine::Line(2)]);
        let mut lines = lines;
        lines[1].start = Duration::from_secs(3);
        lines[1].end = None;
        assert!(interludes(&lines).is_empty());
    }

    #[test]
    fn short_explicit_breaks_and_trailing_empty_stamps_have_no_slot() {
        assert!(interludes(&[line(0.0, None, "a"), line(1.0, None, ""), line(2.49, None, "b"), line(3.0, None, "")]).is_empty());
        assert!(display_lines(&[]).is_empty());
        assert!(display_lines(&[line(0.0, None, "")]).is_empty());
    }

    #[test]
    fn interludes_own_the_scroll_anchor_until_singing_starts() {
        let lines = [line(3.0, Some(4.0), "a"), line(10.0, None, "b")];
        let items = display_lines(&lines);
        assert_eq!(current_item(&lines, &items, Duration::ZERO), Some(0));
        assert_eq!(current_item(&lines, &items, Duration::from_secs(3)), Some(1));
        assert_eq!(current_item(&lines, &items, Duration::from_secs(4)), Some(2));
        assert_eq!(current_item(&lines, &items, Duration::from_secs(10)), Some(3));
    }

    #[test]
    fn clicking_waiting_skips_to_one_second_before_singing() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs(1);
        state.lyrics.lines = vec![line(3.0, None, "Свет из окна")];
        // Without soft edges the line rests at the anchor of the part above
        // the control's row.
        state.prefs.lyrics_soft_edges = false;
        let mut target = pos2(0.0, 0.0);
        let mut commands = Vec::new();
        for frame in 0..3 {
            let events = if frame == 0 { Vec::new() } else {
                vec![egui::Event::PointerMoved(target), egui::Event::PointerButton {
                    pos: target, button: egui::PointerButton::Primary, pressed: frame == 1, modifiers: egui::Modifiers::NONE,
                }]
            };
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0))),
                time: Some(frame as f64 * 0.05), events, ..Default::default()
            }, |ui| {
                // Short lines are centred in mega mode: aim at the middle.
                target = pos2(ui.cursor().left() + ui.available_width() * 0.5,
                    ui.cursor().top() + (ui.available_height() - Size::Xs.height() - CONTROL_GAP) * MEGA.anchor);
                show(ui, &state, 0.0, true, &mut commands);
            });
            output.textures_delta.clear();
        }
        assert!(commands.iter().any(|c| matches!(c, Command::Seek(t) if *t == Duration::from_secs(2))));
    }

    /// A tap on a touch screen: the pointer is gone in the frame the press
    /// ends, so nothing is hovered then. The tap still seeks (it was lost:
    /// the view stayed where it was and went back to the current line).
    #[test]
    fn a_tap_seeks_though_the_pointer_is_gone_when_it_ends() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs(1);
        state.lyrics.lines = vec![line(3.0, None, "Свет из окна")];
        state.prefs.lyrics_soft_edges = false;
        let mut target = pos2(0.0, 0.0);
        let mut commands = Vec::new();
        for frame in 0..3 {
            let button = |pressed| egui::Event::PointerButton {
                pos: target, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE,
            };
            let events = match frame {
                0 => Vec::new(),
                1 => vec![egui::Event::PointerMoved(target), button(true)],
                _ => vec![button(false), egui::Event::PointerGone],
            };
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0))),
                time: Some(frame as f64 * 0.05), events, ..Default::default()
            }, |ui| {
                target = pos2(ui.cursor().left() + ui.available_width() * 0.5,
                    ui.cursor().top() + (ui.available_height() - Size::Xs.height() - CONTROL_GAP) * MEGA.anchor);
                show(ui, &state, 0.0, true, &mut commands);
            });
            output.textures_delta.clear();
        }
        assert!(commands.iter().any(|c| matches!(c, Command::Seek(t) if *t == Duration::from_secs(2))), "{commands:?}");
    }

    /// Where a click seeks: a line's start; a wait's end less a second.
    #[test]
    fn a_click_seeks_to_its_line_or_just_before_the_singing() {
        let lines = [line(3.0, Some(4.0), "a"), line(10.0, None, "b")];
        let items = display_lines(&lines);
        let at = |item: DisplayLine| seek_to(&lines, item);
        assert_eq!(at(DisplayLine::Line(1)), Duration::from_secs(10));
        assert_eq!(at(items[0]), Duration::from_secs(2), "the intro's wait");
        assert_eq!(at(DisplayLine::Interlude { start: Duration::ZERO, end: Duration::from_millis(500), duet: false }), Duration::ZERO);
    }

    /// Scrolled down by hand to a line far below and clicked: the line is
    /// the current one from the click on, while the seek has not come back
    /// (the position still the old one, above or below), while a lead behind
    /// the sound paints a little before it (as long as it takes, paused
    /// too), and at most 3 s when nothing plays. Reaching the line lets go;
    /// another text drops it.
    #[test]
    fn a_clicked_line_is_current_until_its_seek_comes_back() {
        let lines = [line(3.0, Some(4.0), "a"), line(10.0, Some(12.0), "b"), line(60.0, None, "c")];
        let items = display_lines(&lines);
        let of = |item: DisplayLine| items.iter().position(|it| *it == item).unwrap();
        let (a, b, c) = (of(DisplayLine::Line(0)), of(DisplayLine::Line(1)), of(DisplayLine::Line(2)));
        let by = |secs: f32| current_item(&lines, &items, Duration::from_secs_f32(secs));
        let at = |secs: f32| Duration::from_secs_f32(secs);
        let none = Duration::ZERO;
        let jump = Jump { item: DisplayLine::Line(2), to: at(60.0), at: 100.0 };
        // Before the seek comes back the position is still at «b».
        assert_eq!(by(10.5), Some(b));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(10.5), at(10.5), none, 100.02), (Some(c), Some(jump)));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(10.6), at(10.6), none, 102.9), (Some(c), Some(jump)));
        // Back: the position says it, the jump is done.
        assert_eq!(with_jump(Some(jump), &lines, &items, by(60.0), at(60.0), none, 100.1), (Some(c), None));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(60.3), at(60.3), none, 100.1), (Some(c), None));
        // Nothing plays: let go after 3 s.
        assert_eq!(with_jump(Some(jump), &lines, &items, by(10.6), at(10.6), none, 103.1), (Some(b), None));
        // A lead 0.4 s behind the sound paints at 59.7 s: still «c», paused
        // for as long as it takes; without such a lead 59.7 is not «c».
        let back = Duration::from_millis(400);
        assert_eq!(with_jump(Some(jump), &lines, &items, by(59.7), at(59.7), back, 200.0), (Some(c), Some(jump)));
        assert_eq!(with_jump(Some(jump), &lines, &items, by(59.7), at(59.7), none, 200.0), (by(59.7), None));
        // Up: clicked «a» while «c» plays, far past it.
        let up = Jump { item: DisplayLine::Line(0), to: at(3.0), at: 50.0 };
        assert_eq!(with_jump(Some(up), &lines, &items, by(61.0), at(61.0), none, 50.05), (Some(a), Some(up)));
        assert_eq!(with_jump(Some(up), &lines, &items, by(3.0), at(3.0), none, 50.2), (Some(a), None));
        // Another text (the song changed): dropped at once.
        let other = [line(1.0, None, "x"), line(2.0, None, "y"), line(5.0, None, "z")];
        let other_items = display_lines(&other);
        let by_other = current_item(&other, &other_items, at(0.5));
        assert_eq!(with_jump(Some(jump), &other, &other_items, by_other, at(0.5), none, 100.05), (by_other, None));
        assert_eq!(with_jump(None, &lines, &items, by(10.5), at(10.5), none, 1.0), (Some(b), None));
    }

    /// Scrolled by hand, the text stays where the user left it as the song
    /// moves to its next lines (it jumped a line at a time under the
    /// pointer, and a click took the line that had moved there); the hold
    /// ends and it eases back; a click ends it at once.
    #[test]
    fn a_scrolled_text_stays_put_while_the_song_moves_on() {
        let range = (-5000.0, 5000.0);
        // The wheel down 300: the text moves up, 300 past the current line.
        let m = Manual::default().step(100.0, -300.0, 10.0, 4.0, range);
        assert_eq!((m.offset, m.until, m.from), (300.0, 14.0, 100.0));
        assert!(m.on(10.0));
        // The next line becomes current (follow moves by 60): the text
        // stays at 400 all the same.
        let m = m.step(160.0, 0.0, 11.0, 4.0, range);
        assert_eq!(160.0 + m.shift(11.0), 400.0);
        let m = m.step(230.0, 0.0, 12.0, 4.0, range);
        assert_eq!(230.0 + m.shift(12.0), 400.0);
        // Scrolled on from there.
        let m = m.step(230.0, -50.0, 12.5, 4.0, range);
        assert_eq!(230.0 + m.shift(12.5), 450.0);
        assert_eq!(m.until, 16.5);
        // The hold is over: back to following, eased over half a second.
        let m = m.step(300.0, 0.0, 16.75, 4.0, range);
        assert!(!m.on(16.75));
        assert!(m.shift(16.75) > 0.0 && m.shift(16.75) < m.offset);
        assert_eq!(m.shift(17.0), 0.0);
        // The text's ends hold it; an empty range never panics.
        assert_eq!(Manual::default().step(0.0, -9000.0, 1.0, 4.0, (-50.0, 80.0)).offset, 80.0);
        assert_eq!(Manual::default().step(0.0, 9000.0, 1.0, 4.0, (-50.0, 80.0)).offset, -50.0);
        assert_eq!(Manual::default().step(0.0, 0.0, 1.0, 4.0, (30.0, 0.0)).offset, 0.0);
        // A click puts it back to following at once.
        let clicked = Manual::default();
        assert!(!clicked.on(12.0));
        assert_eq!(clicked.shift(12.0), 0.0);
    }

    fn with_painter(test: impl FnOnce(&egui::Painter)) {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut test = Some(test);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            test.take().unwrap()(ui.painter());
        });
        output.textures_delta.clear();
    }

    fn three_rows(painter: &egui::Painter, text: &str) -> Arc<Breaks> {
        (200..900).step_by(10).find_map(|width| {
            let b = fixed_breaks(painter, egui::Id::new("test-breaks"), text, width as f32, 44.0);
            (b.forced.split('\n').count() == 3).then_some(b)
        }).expect("fixture must wrap into three rows")
    }

    /// A swapped text with two more lines above the one being sung, while
    /// the view is moving to it: moved with that line, the text stays where
    /// it is drawn and keeps its speed, and only the rest of the way is
    /// animated.
    #[test]
    fn a_swapped_text_stays_where_it_is_drawn_and_keeps_its_speed() {
        use nikgob_motion::Animated;
        let old_tops = [0.0, 60.0, 120.0, 180.0];
        let old_slots = [10, 11, 12, 13];
        // The view moves from line 1 to line 2 (scroll = top − anchor 100).
        let mut scroll = Animated::new(60.0_f32 - 100.0, Spring::new(1.0, 300.0));
        scroll.set_target(120.0 - 100.0, 0.0);
        let now = 0.08;
        let drawn = old_tops[2] - scroll.value(now);
        let speed = scroll.velocity(now);
        // Two new lines come in before it; its slot stays with it.
        let new_tops = [0.0, 60.0, 120.0, 180.0, 240.0, 300.0];
        let new_slots = [20, 21, 10, 11, 12, 13];
        let moved = held_moved(Some((old_slots[2], old_tops[2])), &new_slots, &new_tops).unwrap();
        assert_eq!(moved, 120.0);
        scroll.shift(moved);
        assert!((new_tops[4] - scroll.value(now) - drawn).abs() < 1e-3);
        assert!((scroll.velocity(now) - speed).abs() < 1e-2);
        // It goes on to the line's place in the new text, on the same spring.
        scroll.set_target(new_tops[4] - 100.0, now);
        assert!((new_tops[4] - scroll.value(now) - drawn).abs() < 1e-3);
        assert!((scroll.velocity(now) - speed).abs() < 1e-2);
        assert_eq!(scroll.value(5.0), new_tops[4] - 100.0);
        // A line not in the new text moves nothing.
        assert_eq!(held_moved(Some((99, 0.0)), &new_slots, &new_tops), None);
        assert_eq!(held_moved(None, &new_slots, &new_tops), None);
    }

    /// At whole distances a line rests as bright as it always did; between
    /// them (the current line changing) its brightness moves without a step.
    #[test]
    fn resting_brightness_moves_without_steps_as_the_current_line_changes() {
        for style in [Style::Karaoke, Style::Focus, Style::Wave] {
            for dist in -12i32..=12 {
                let far = dist.unsigned_abs().min(8) as f32;
                let old = match style {
                    Style::Focus => 0.62f32.powf(far).max(0.08) * 0.7,
                    _ => (if dist < 0 { 0.5 } else { 0.42 }) * 0.86f32.powf((far - 1.0).max(0.0)),
                };
                assert!((rest_brightness(style, dist as f32) - old).abs() < 1e-6, "{style:?} {dist}");
            }
            let mut was = rest_brightness(style, -10.0);
            for k in 1..=2000 {
                let now = rest_brightness(style, -10.0 + k as f32 * 0.01);
                assert!((now - was).abs() < 0.006, "{style:?} at {}", -10.0 + k as f32 * 0.01);
                was = now;
            }
        }
    }

    /// A line growing from one resting size to the other: its height and
    /// rows move a little every frame (never by a rounded row height's
    /// whole pixel at once), and it starts and ends on the resting layouts.
    #[test]
    fn a_growing_line_moves_evenly_and_meets_its_resting_layouts() {
        with_painter(|painter| {
            let b = three_rows(painter, "и слова, что не сказаны вслух, звучат громче всего остального, пока город считает шаги до утра");
            let rest = (32.0 * 1.1, 44.0 * 1.1);
            let (from, to) = (fixed_layout(painter, &b.forced, rest.0), fixed_layout(painter, &b.forced, rest.1));
            let per_step = (to.rect.height() - from.rect.height()) / 200.0;
            let mut was = from.clone();
            for step in 1..200 {
                let t = step as f32 / 200.0;
                let g = moving_layout(painter, &b.forced, rest, t);
                assert_eq!(g.rows.len(), 3);
                assert!((g.rect.height() - was.rect.height() - per_step).abs() < 1e-3, "t={t}");
                for r in 0..3 {
                    let moved = g.rows[r].pos.y - was.rows[r].pos.y;
                    let even = (to.rows[r].pos.y - from.rows[r].pos.y) / 200.0;
                    assert!((moved - even).abs() < 1e-3, "t={t} row {r}");
                }
                was = g;
            }
            let near = |t: f32| moving_layout(painter, &b.forced, rest, t);
            for (g, rest) in [(near(1e-4), &from), (near(1.0 - 1e-4), &to)] {
                assert!((g.rect.height() - rest.rect.height()).abs() < 0.01);
                assert!((g.rect.width() - rest.rect.width()).abs() < 0.05);
            }
        });
    }

    #[test]
    fn forced_rows_stay_identical_across_all_animated_sizes_and_invalidate_on_width() {
        with_painter(|painter| {
            for text in [
                "и слова, что не сказаны вслух, звучат громче всего остального, пока город считает шаги до утра",
                "The words we never spoke are louder than the city and the light that waits outside our window tonight",
            ] {
                let b = three_rows(painter, text);
                let rows: Vec<_> = b.forced.split('\n').map(str::to_owned).collect();
                for step in 0..=120 {
                    let g = fixed_layout(painter, &b.forced, 32.0 + step as f32 * 0.1);
                    assert_eq!(g.rows.len(), 3);
                    assert_eq!(g.rows.iter().map(|r| r.row.glyphs.iter().map(|gl| gl.chr).collect::<String>()).collect::<Vec<_>>(), rows);
                    assert_eq!(glyph_count(&g), text.chars().count());
                }
                let wide = fixed_breaks(painter, egui::Id::new("test-breaks"), text, 4000.0, 44.0);
                assert_eq!(wide.forced, text);
                let changed = fixed_breaks(painter, egui::Id::new("test-breaks"), "Другой текст", 4000.0, 44.0);
                assert_eq!(changed.forced, "Другой текст");
                let scaled = fixed_breaks(painter, egui::Id::new("test-breaks"), "Другой текст", 4000.0, 44.0 * 1.4);
                assert!(scaled.height > changed.height);
            }
        });
    }

    #[test]
    fn word_synced_three_row_line_maps_every_glyph_to_the_same_letter() {
        with_painter(|painter| {
            let text = "Свет из окна ложится на пол пока город считает наши шаги до утра и слова звучат громче всего";
            let mut l = line(0.0, Some(20.0), text);
            l.words = text.split_whitespace().enumerate().map(|(i, word)| nikgob_view::LyricWord {
                start: Duration::from_secs(i as u64), end: Duration::from_secs(i as u64 + 1), text: format!("{word} "),
            }).collect();
            let b = three_rows(painter, text);
            let mut expected = Vec::new();
            for (i, word) in l.words.iter().enumerate() {
                let n = word.text.chars().filter(|c| !c.is_whitespace()).count();
                for _ in 0..n {
                    expected.push(Some(i));
                }
            }
            for size in [32.0, 38.0, 44.0] {
                let g = fixed_layout(painter, &b.forced, size);
                let timing = Timing::new(&l, &g).unwrap();
                assert_eq!(g.rows.len(), 3);
                assert_eq!(timing.glyphs.len(), glyph_count(&g));
                let mut letters = expected.iter();
                for (gl, time) in g.rows.iter().flat_map(|r| &r.row.glyphs).zip(&timing.glyphs) {
                    if gl.chr.is_whitespace() {
                        assert_eq!(*time, None);
                    } else {
                        assert_eq!(Some(time), letters.next());
                    }
                }
                assert!(letters.next().is_none());
                let line = Rect::from_min_size(pos2(0.0, 0.0), g.size());
                let pace = Pace::of(&Prefs::default());
                karaoke(painter, &g, line, 80.0, pace.edge(size), Color32::GRAY, Color32::WHITE);
                // Words a second each, one after another: one front
                // through the three rows.
                let look = Look { style: Style::Karaoke, by: By::Letters };
                let sung = Timing::of(&l, &g, look, pace, size).unwrap();
                assert_eq!(sung.front.runs.len(), 1);
                assert!(sung.front.of.iter().zip(&sung.glyphs).all(|(run, s)| run.is_some() == s.is_some()));
                timed_line(painter, &g, line, &sung, Duration::from_secs(4), 1.0, 1.0, size, look, Color32::GRAY, Color32::WHITE, &Prefs::default(), None);
            }
        });
    }

    #[test]
    fn vowels_weigh_more_than_consonants_and_kana_evenly() {
        for v in ['а', 'Я', 'ё', 'o', 'É', 'y', 'ω'] {
            assert_eq!(letter_weight(v), 2.5, "{v}");
        }
        for c in ['д', 'Ж', 'й', 'k', 'S', '7'] {
            assert_eq!(letter_weight(c), 1.0, "{c}");
        }
        for m in ['-', '\'', ',', 'ь', 'Ъ'] {
            assert!(letter_weight(m) < 1.0, "{m}");
        }
        // Kana, hangul and hanzi are syllables already: even shares.
        assert!(['か', 'ナ', '한', '中'].iter().all(|c| letter_weight(*c) == 1.0));
    }

    /// A word with a held last vowel: the consonants go by quickly and the
    /// vowels take the time, the word still starting and ending on its own
    /// times, each letter right after the one before.
    #[test]
    fn letters_share_their_words_time_by_weight() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "здесь домой");
            l.words = vec![word(1.0, 1.6, "здесь "), word(2.0, 3.0, "домой")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let times: Vec<(f32, f32)> = t.letter_times.iter().flatten().copied().collect();
            assert_eq!(times.len(), 10);
            let (a, b) = times.split_at(5);
            for (word, (start, end)) in [(a, (1.0, 1.6)), (b, (2.0, 3.0))] {
                assert_eq!(word[0].0, start);
                assert_eq!(word[4].1, end);
                for pair in word.windows(2) {
                    assert!(close(pair[0].1, pair[1].0), "{pair:?}");
                }
            }
            // «домой»: 1 + 2.5 + 1 + 2.5 + 1 = 8 shares of a second.
            let len = |(a, b): (f32, f32)| b - a;
            assert!(close(len(b[0]), 1.0 / 8.0) && close(len(b[1]), 2.5 / 8.0) && close(len(b[4]), 1.0 / 8.0));
            assert!(len(b[3]) > 2.0 * len(b[2]));
            // The karaoke front by letters, on the times exactly, crosses
            // the «д» by its share, not by a fifth.
            let k = Timing::build(&l, &g, Look { style: Style::Karaoke, by: By::Letters }, 0.0, Ease::EXACT).unwrap();
            assert_eq!(k.front.runs.len(), 2, "a pause parts the words");
            let at = |s: f32| k.front.runs[1].way(f64::from(s));
            let x = |i: usize| t.xs[i];
            let d = 6; // «д» after «здесь» and the space
            assert!((at(2.0 + 0.5 / 8.0) - (x(d).0 + x(d).1) / 2.0).abs() < 0.5);
            assert!((at(2.0 + 1.0 / 8.0) - x(d).1).abs() < 0.5);
            assert!((at(2.0 + 2.25 / 8.0) - (x(d + 1).0 + x(d + 1).1) / 2.0).abs() < 0.5);
            let mut was = 0.0;
            for ms in 1000..3000 {
                let h = at(ms as f32 / 1000.0);
                assert!(h >= was - 1e-3, "{ms}");
                was = h;
            }
        });
    }

    #[test]
    fn glued_syllables_share_a_word_and_the_front_waits_in_gaps() {
        with_painter(|painter| {
            let mut l = line(2.0, Some(8.0), "Hello world");
            l.words = [(2.0, 3.0, "Hel"), (4.0, 6.0, "lo "), (7.0, 8.0, "world")].map(|(s, e, text)| nikgob_view::LyricWord {
                start: Duration::from_secs_f32(s), end: Duration::from_secs_f32(e), text: text.into(),
            }).to_vec();
            let g = fixed_layout(painter, &l.text, 44.0);
            let t = Timing::of(&l, &g, Look { style: Style::Karaoke, by: By::Letters }, Pace::of(&Prefs::default()), 44.0).unwrap();
            assert_eq!(t.words, [0, 2]);
            assert_eq!(t.syllables.iter().map(|s| s.word).collect::<Vec<_>>(), [0, 0, 1]);
            // A second of silence parts the pieces, each with a front of
            // its own: in a gap the piece sung is lit whole and the next
            // not begun, no letter left half lit (also inside a word).
            let front = &t.front;
            assert_eq!(front.runs.len(), 3);
            let heads = |p: f64| [front.head(0, p), front.head(1, p), front.head(2, p)];
            let (none, all) = (f32::NEG_INFINITY, f32::INFINITY);
            assert_eq!(heads(1.0), [none; 3]);
            assert_eq!(heads(3.6), [all, none, none]);
            assert_eq!(heads(6.6), [all, all, none]);
            assert_eq!(heads(9.0), [all; 3]);
            // Inside «lo» its front is in it.
            let h = front.head(1, 5.0);
            assert!(h > t.syllables[1].from && h < t.syllables[1].to + front.edge, "{h}");
            // It comes in and goes out with no pop: just sung, the edge has
            // only begun on the first letter; about to rest, it is past the
            // last.
            let first = front.head(0, 2.02);
            assert!(first > t.syllables[0].from && first < t.syllables[0].from + 0.3 * front.edge, "{first}");
            let (_, rest) = front.runs[0].times();
            let last = front.head(0, rest - 0.004);
            assert!(last > t.syllables[0].to + 0.95 * front.edge, "{last}");
        });
    }

    #[test]
    fn line_synced_lines_are_paced_by_letters_inside_the_gap() {
        // No end: a speaking pace, never past the next line.
        let l = line(2.0, None, "ab cdef");
        let p = paced_words(&l, Some(Duration::from_secs(10)));
        assert_eq!(p.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["ab ", "cdef"]);
        let span = speaking_secs("ab cdef", 8.0);
        assert_eq!(p.words[0].start, Duration::from_secs(2));
        assert!((p.words[0].end.as_secs_f32() - (2.0 + span / 3.0)).abs() < 1e-3, "2 of 6 letters");
        assert!((p.words[1].end.as_secs_f32() - (2.0 + span)).abs() < 1e-3);
        // With an end the whole line is used.
        let p = paced_words(&line(2.0, Some(8.0), "ab cdef"), None);
        assert!((p.words[1].end.as_secs_f32() - 8.0).abs() < 1e-3);
    }

    #[test]
    fn paused_waiting_does_not_poll_after_its_springs_settle() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs(1);
        state.lyrics.lines = vec![line(3.0, None, "Свет из окна")];
        let mut delay = Duration::ZERO;
        for i in 0..20 {
            let mut output = ctx.run_ui(egui::RawInput { time: Some(i as f64 * 0.5), ..Default::default() }, |ui| {
                show(ui, &state, 0.8, true, &mut Vec::new());

            });
            delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            output.textures_delta.clear();
        }
        assert!(delay > Duration::from_secs(1), "paused waiting kept requesting frames: {delay:?}");
    }

    #[test]
    fn close_lines_do_not_panic_and_keep_the_pace_inside_the_gap() {
        // Lines 0.5 s apart (and duplicate stamps) used to panic in `clamp`.
        assert!((speaking_secs("вторая быстро", 0.5) - 0.46).abs() < 1e-4);
        assert_eq!(speaking_secs("a", 0.0), 0.3);
        assert_eq!(speaking_secs("a", 6.0), 0.8);
        assert!((speaking_secs("слова звучат громче всего", 6.0) - (0.4 + 22.0 * 0.066)).abs() < 1e-4);
    }

    #[test]
    fn manual_scroll_holds_then_returns_smoothly_and_can_restart_mid_return() {
        assert_eq!(returning_offset(100.0, 4.0, 3.0), 100.0);
        assert_eq!(returning_offset(100.0, 4.0, 4.0), 100.0);
        assert_eq!(returning_offset(100.0, 4.0, 4.25), 50.0);
        assert_eq!(returning_offset(100.0, 4.0, 4.5), 0.0);
        let scrolled = returning_offset(100.0, 4.0, 4.25) + 10.0;
        assert_eq!(returning_offset(scrolled, 8.25, 4.25), 60.0);
    }

    #[test]
    fn wave_crests_round_and_clamp_without_folding_thick_strokes() {
        assert_eq!(wave_geometry(2.0, 13.0, 3.0, 0.0, 1.0), (3.0, 0.0));
        assert_eq!(wave_geometry(6.49, 13.0, 3.0, 0.0, 1.0).0, 6.0);
        assert_eq!(wave_geometry(6.5, 13.0, 3.0, 0.0, 1.0).0, 7.0);
        assert_eq!(wave_geometry(20.0, 13.0, 3.0, 0.0, 1.0).0, 16.0);
        // The default thin ring (6 crests) gets a wave about half its stroke.
        assert!(wave_geometry(6.0, 13.0, 3.0, 100.0, 1.0).1 >= 1.5);
        // «Высота волн» raises the cap with it (up to folding).
        assert!(wave_geometry(6.0, 13.0, 3.0, 100.0, 2.0).1 > wave_geometry(6.0, 13.0, 3.0, 100.0, 1.0).1 * 1.5);
        assert!(wave_geometry(6.0, 13.0, 3.0, 100.0, 0.5).1 < wave_geometry(6.0, 13.0, 3.0, 100.0, 1.0).1);
        for radius in [13.0, 16.0, 24.0] {
            for stroke in [3.0, 6.0, 9.0] {
                for count in 3..=16 {
                    let (waves, amp) = wave_geometry(count as f32, radius, stroke, 100.0, 1.0);
                    assert!(amp <= std::f32::consts::TAU * radius / waves * 0.12);
                    assert!(wave_geometry(count as f32, radius, stroke, 100.0, 2.0).1 <= std::f32::consts::TAU * radius / waves * 0.24);
                    // Exact polar curvature at a crest and a trough stays
                    // under the stroke's half-width limit (no folding).
                    for (r, d2) in [(radius + amp, -amp * waves * waves), (radius - amp, amp * waves * waves)] {
                        let curvature: f32 = (r * r - r * d2) / (r * r * r);
                        assert!(curvature.abs() <= 2.0 / stroke + 1e-4, "R={radius} s={stroke} n={waves} A={amp}");
                    }
                    assert!(((waves * std::f32::consts::TAU).sin()).abs() < 1e-4);
                }
            }
        }
    }

    /// The meshes a wait paints at `pos` of a ten-second wait, and its texts.
    fn wait_shapes(ctx: &egui::Context, prefs: &Prefs, pos: f32, appear: f32) -> (Vec<Arc<egui::Mesh>>, usize) {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            waiting(ui.painter(), Rect::from_min_size(pos2(20.0, 20.0), vec2(56.0, 42.0)), 1.0,
                Duration::ZERO, Duration::from_secs(10), Duration::from_secs_f32(pos), appear, false, 1.0, 1.0, prefs);
        });
        output.textures_delta.clear();
        let meshes = output.shapes.iter().filter_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) => Some(mesh.clone()), _ => None,
        }).collect();
        let texts = output.shapes.iter().filter(|shape| matches!(shape.shape, egui::Shape::Text(_))).count();
        (meshes, texts)
    }

    #[test]
    fn waiting_strokes_are_opaque_and_countdown_respects_reduced_motion() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        for reduced_motion in [false, true] {
            let mut theme = Theme::get(&ctx);
            theme.reduced_motion = reduced_motion;
            theme.install(&ctx);
            for form in 0..4 {
                let prefs = Prefs { lyrics_gap_style: form, lyrics_countdown: false, ..Prefs::default() };
                // Mid-wait: the track and the indicator, each one opaque
                // mesh (a solid core inside a clear rim), as wide as the form.
                let (meshes, _) = wait_shapes(&ctx, &prefs, 5.0, 1.0);
                assert_eq!(meshes.len(), 2);
                for mesh in &meshes {
                    assert!(mesh.vertices.iter().all(|v| v.color.a() == 255 || v.color.a() == 0));
                    let core = mesh.vertices[1].pos.distance(mesh.vertices[2].pos);
                    assert!((core - if form >= 2 { 5.0 } else { 2.0 }).abs() < 0.05, "{form}: {core}");
                }
                // Leaving: one closed ring (no ends) that fades as one.
                let (meshes, _) = wait_shapes(&ctx, &prefs, 9.8, 1.0);
                assert_eq!(meshes.len(), 1);
                assert!(meshes[0].vertices.iter().all(|v| v.color.a() < 255));
                assert_eq!(meshes[0].indices.len(), meshes[0].vertices.len() / 4 * 18, "a ring has no caps");
                assert!(wait_shapes(&ctx, &prefs, 10.0, 1.0).0.is_empty());
                // Before it has come, nothing.
                assert!(wait_shapes(&ctx, &prefs, 5.0, 0.0).0.is_empty());
            }
            let prefs = Prefs { lyrics_countdown: true, ..Prefs::default() };
            for remaining in [2.95, 2.5, 1.5, 0.5] {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    waiting(ui.painter(), Rect::from_min_size(pos2(20.0, 20.0), vec2(56.0, 42.0)), 1.0,
                        Duration::ZERO, Duration::from_secs(10), Duration::from_secs_f32(10.0 - remaining),
                        1.0, false, 0.0, 1.0, &prefs);
                });
                output.textures_delta.clear();
                let texts: Vec<_> = output.shapes.iter().filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text), _ => None,
                }).collect();
                assert_eq!(texts.len(), 1);
                let text = texts[0];
                assert_eq!(text.galley.job.text, format!("{:.0}", remaining.ceil()));
                let size = text.galley.job.sections[0].format.font_id.size;
                if reduced_motion || remaining != 2.95 { assert_eq!(size, 28.0); }
                else { assert!(size > 28.0); }
                if remaining <= 2.5 {
                    assert!(!output.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::Mesh(_))));
                }
            }
        }
    }

    /// A stroke is one mesh: an open one has a half disc on each end built
    /// on the end's own vertices, a ring none; no triangle is drawn twice.
    #[test]
    fn ribbons_are_one_mesh_with_round_ends_or_closed() {
        let pts: Vec<egui::Pos2> = (0..10).map(|i| pos2(i as f32 * 3.0, (i as f32 * 0.5).sin() * 4.0)).collect();
        let egui::Shape::Mesh(open) = ribbon(&pts, 6.0, false, Color32::WHITE, 1.0) else { panic!() };
        let steps = ((3.5f32 * std::f32::consts::PI * 0.5).ceil() as usize).clamp(4, 24);
        assert_eq!(open.vertices.len(), 4 * pts.len() + 2 * (1 + 2 * (steps - 1)));
        assert_eq!(open.indices.len(), 18 * (pts.len() - 1) + 2 * 9 * steps);
        let mut seen = std::collections::HashSet::new();
        for t in open.indices.chunks(3) {
            let mut t = [t[0], t[1], t[2]];
            t.sort_unstable();
            assert!(seen.insert(t), "{t:?} twice");
        }
        // The caps reach half the width past the ends.
        let reach = open.vertices.iter().filter(|v| v.color.a() == 255).map(|v| v.pos.x).fold(f32::MIN, f32::max);
        assert!((reach - (27.0 + 2.5)).abs() < 0.6, "{reach}");
        let ring: Vec<egui::Pos2> = (0..24).map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 24.0;
            pos2(a.cos() * 20.0, a.sin() * 20.0)
        }).collect();
        let egui::Shape::Mesh(closed) = ribbon(&ring, 4.0, true, Color32::WHITE, 1.0) else { panic!() };
        assert_eq!((closed.vertices.len(), closed.indices.len()), (4 * 24, 18 * 24));
        // Too thin for its rim: fainter, not thinner than a pixel.
        let egui::Shape::Mesh(thin) = ribbon(&pts, 0.5, false, Color32::WHITE, 1.0) else { panic!() };
        assert!(thin.vertices.iter().all(|v| v.color.a() < 255));
        assert!(matches!(ribbon(&pts[..1], 4.0, false, Color32::WHITE, 1.0), egui::Shape::Noop));
    }

    /// The wait's room follows the indicator's size and its waves' height.
    #[test]
    fn the_wait_room_grows_with_the_ring() {
        let p = Prefs::default();
        assert!((p.lyrics_gap_size - 1.0).abs() < 1e-6 && (p.lyrics_gap_height - 1.0).abs() < 1e-6);
        assert_eq!(wait_height(&p), 34.0);
        assert_eq!(wait_height(&Prefs { lyrics_gap_style: 2, ..p.clone() }), 42.0);
        assert!(wait_height(&Prefs { lyrics_gap_size: 1.5, ..p.clone() }) > 50.0);
        assert!(wait_height(&Prefs { lyrics_gap_height: 2.0, ..p.clone() }) > 34.0);
        // A flat ring has no waves to make room for.
        assert_eq!(wait_height(&Prefs { lyrics_gap_style: 0, lyrics_gap_height: 2.0, ..p.clone() }), 34.0);
        // It grows in on its spring, from nothing.
        assert_eq!(ring_in(0.0), 0.0);
        assert!(ring_in(0.1) > 0.3 && ring_in(1.0) > 0.99);
    }

    /// Every glyph is drawn once and whole: the pieces a front splits it
    /// into tile its quad and its texture exactly, a uniform glyph stays
    /// one quad, the light only falls along the line, and a lift moves the
    /// whole glyph.
    #[test]
    fn inked_glyphs_are_whole_and_lit_along_the_line() {
        with_painter(|painter| {
            let g = fixed_layout(painter, "Слышишь этот ритм", 26.0);
            let quads: usize = g.rows.iter().flat_map(|r| &r.row.glyphs).filter(|gl| !gl.uv_rect.is_nothing()).count();
            let plain = inked(&g, |_| Ink::solid(Color32::WHITE, 0.0));
            assert_eq!(plain.rows[0].row.visuals.mesh.vertices.len(), 4 * quads);
            let lifted = inked(&g, |_| Ink::solid(Color32::WHITE, 3.0));
            for (a, b) in plain.rows[0].row.visuals.mesh.vertices.iter().zip(&lifted.rows[0].row.visuals.mesh.vertices) {
                assert!((a.pos.y - b.pos.y - 3.0).abs() < 1e-4 && a.pos.x == b.pos.x && a.uv == b.uv);
            }
            let front = Ink { lift: 0.0, base: Color32::BLACK, lit: Color32::WHITE, head: 70.0, feather: 9.0, rise: 0.0, grow: 0.0 };
            let lit = inked(&g, |_| front);
            let v = &lit.rows[0].row.visuals.mesh.vertices;
            assert!(v.len() > 4 * quads, "the front splits the glyphs it crosses");
            let src = &g.rows[0].row.visuals.mesh.vertices;
            let mut at = 0;
            for gl in g.rows[0].row.glyphs.iter().filter(|gl| !gl.uv_rect.is_nothing()) {
                let q = &src[gl.first_vertex as usize..gl.first_vertex as usize + 4];
                // This glyph's pieces: from its left edge to its right edge,
                // each starting where the one before ended.
                assert_eq!(v[at].pos, q[0].pos);
                let mut x = q[0].pos.x;
                loop {
                    assert_eq!(v[at].pos.x, x);
                    x = v[at + 1].pos.x;
                    assert!(x > v[at].pos.x);
                    // Brightness never rises to the right.
                    assert!(v[at + 1].color.r() <= v[at].color.r());
                    at += 4;
                    if x >= q[1].pos.x {
                        break;
                    }
                }
                assert_eq!(v[at - 3].pos, q[1].pos);
                assert_eq!(v[at - 3].uv, q[1].uv);
            }
            assert_eq!(at, v.len());
            // Lit before the front, unlit past it.
            assert_eq!(v[0].color, Color32::WHITE);
            assert_eq!(v[v.len() - 1].color, Color32::BLACK);
            // A hard front (no feather) stays hard inside the glyph it cuts.
            let hard = inked(&g, |_| Ink { feather: 0.0, ..front });
            assert!(hard.rows[0].row.visuals.mesh.vertices.iter().all(|v| v.color == Color32::WHITE || v.color == Color32::BLACK));
        });
    }

    /// Karaoke's sung words float up whole by the light at their middle (a
    /// lit glyph by all of its rise, one ahead of the front not at all, none
    /// leaning), and a swelling glyph grows about the middle of its foot.
    #[test]
    fn sung_glyphs_float_up_whole_and_a_swell_grows_from_the_foot() {
        with_painter(|painter| {
            let g = fixed_layout(painter, "Слышишь этот ритм", 26.0);
            let plain = inked(&g, |_| Ink::solid(Color32::WHITE, 0.0));
            let front = Ink { lift: 0.0, base: Color32::BLACK, lit: Color32::WHITE, head: 70.0, feather: 9.0, rise: 3.0, grow: 0.0 };
            let floated = inked(&g, |_| front);
            let pv = &plain.rows[0].row.visuals.mesh.vertices;
            let fv = &floated.rows[0].row.visuals.mesh.vertices;
            let drawn: Vec<_> = g.rows[0]
                .row
                .glyphs
                .iter()
                .zip(&floated.rows[0].row.glyphs)
                .zip(&plain.rows[0].row.glyphs)
                .filter(|((gl, _), _)| !gl.uv_rect.is_nothing())
                .map(|((_, f), p)| (f, p))
                .collect();
            let (mut lit, mut ahead) = (0, 0);
            for (i, (f, p)) in drawn.iter().enumerate() {
                let start = f.first_vertex as usize;
                let end = drawn.get(i + 1).map_or(fv.len(), |(n, _)| n.first_vertex as usize);
                let up = pv[p.first_vertex as usize].pos.y - fv[start].pos.y;
                // Every piece of the glyph at one height.
                for v in &fv[start..end] {
                    let top = (v.pos.y - fv[start].pos.y).abs() < 1e-3;
                    let bottom = (v.pos.y - fv[start + 2].pos.y).abs() < 1e-3;
                    assert!(top || bottom, "glyph {i} leans");
                }
                if p.pos.x + p.advance_width < front.head - front.feather - 2.0 {
                    assert!((up - 3.0).abs() < 1e-3, "glyph {i} lit: {up}");
                    lit += 1;
                } else if p.pos.x > front.head + 2.0 {
                    assert!(up.abs() < 1e-3, "glyph {i} ahead: {up}");
                    ahead += 1;
                } else {
                    assert!((-1e-3..=3.001).contains(&up), "glyph {i} under the front: {up}");
                }
            }
            assert!(lit > 0 && ahead > 0);
            // Grown by half: the foot's middle stays, the glyph is half as
            // wide and tall again.
            let grown = inked(&g, |_| Ink { grow: 0.5, ..Ink::solid(Color32::WHITE, 0.0) });
            let gv = &grown.rows[0].row.visuals.mesh.vertices;
            let (a, b) = (&pv[0..4], &gv[0..4]);
            assert!(((a[2].pos.x + a[3].pos.x) - (b[2].pos.x + b[3].pos.x)).abs() < 1e-3);
            assert!((a[2].pos.y - b[2].pos.y).abs() < 1e-3);
            assert!(((b[3].pos.x - b[2].pos.x) - 1.5 * (a[3].pos.x - a[2].pos.x)).abs() < 1e-3);
            assert!(((b[2].pos.y - b[0].pos.y) - 1.5 * (a[2].pos.y - a[0].pos.y)).abs() < 1e-3);
        });
    }

    /// A wave piece comes down from its crest on a spring: no step in a
    /// frame, a little past where it rests and back, settled within its
    /// fall at any jump speed.
    #[test]
    fn a_wave_piece_lands_on_a_bounce_and_settles() {
        assert_eq!(land(-0.1, 1.0), 0.0);
        assert_eq!(land(0.0, 1.0), 0.0);
        let (mut top, mut was) = (0.0f32, 0.0f32);
        for f in 1..=240 {
            let v = land(f as f32 / 240.0, 1.0);
            assert!((v - was).abs() < 0.08, "frame {f}: {was} → {v}");
            top = top.max(v);
            was = v;
        }
        assert!(top > 1.03 && top < 1.15, "a little past its rest: {top}");
        for speed in [0.5, 1.0, 2.0] {
            assert!((land(WAVE_FALL / speed, speed) - 1.0).abs() < 0.01, "{speed}");
        }
    }

    /// A timing slip (a piece timed before one written ahead of it) waits
    /// for that one: letters and words light in the order they are written.
    #[test]
    fn onsets_follow_the_written_order() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "ab cd ef");
            l.words = vec![word(1.0, 1.5, "ab "), word(0.8, 1.2, "cd "), word(2.0, 2.5, "ef")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let onsets: Vec<f32> = t.lit.iter().flatten().copied().collect();
            assert_eq!(onsets.len(), 6);
            assert!(onsets.windows(2).all(|w| w[0] <= w[1]), "{onsets:?}");
            // «cd», timed before «ab» is over, waits for it.
            assert!(close(onsets[2], 1.5) && close(onsets[3], 1.5) && close(onsets[4], 2.0), "{onsets:?}");
            assert!(t.units.windows(2).all(|w| w[0].start <= w[1].start && w[1].start <= w[1].end));
            // In order, they are the letters' own starts.
            let mut l = l;
            l.words[1] = word(1.5, 2.0, "cd ");
            let t = Timing::new(&l, &fixed_layout(painter, &l.text, 26.0)).unwrap();
            let own: Vec<f32> = t.letter_times.iter().flatten().map(|(a, _)| *a).collect();
            let lit: Vec<f32> = t.lit.iter().flatten().copied().collect();
            assert!(lit.len() == own.len() && lit.iter().zip(&own).all(|(a, b)| close(*a, *b)), "{lit:?} against {own:?}");
        });
    }

    /// The timed spans the front of `t` keeps time by under `by`: each
    /// written word by words, else each timed piece, as (start, end, from,
    /// to, its first glyph).
    fn spans(t: &Timing, by: By) -> Vec<(f32, f32, f32, f32, usize)> {
        if by != By::Words {
            return t.syllables.iter().filter(|s| s.letters > 0).map(|s| (s.start, s.end, s.from, s.to, s.glyphs.0)).collect();
        }
        let mut out: Vec<(f32, f32, f32, f32, usize)> = Vec::new();
        for (k, u) in t.glyph_units.iter().enumerate() {
            let Some(u) = *u else { continue };
            if out.len() <= u {
                out.push((t.units[u].start, t.units[u].end, t.xs[k].0, t.xs[k].1, k));
            }
            out[u].3 = t.xs[k].1;
        }
        out
    }

    /// The report: karaoke went ragged, a long syllable dragging and a short
    /// one flying by, every piece at its own speed. At every step, over
    /// Cyrillic, Latin and Japanese, and at any «Плавность»: each run's
    /// front crosses it in one motion (never back, no jump from one frame
    /// to the next at any frame rate), is in no piece before that is sung
    /// and through each within the tolerance, comes in on the first letter,
    /// and leaves the run lit whole.
    #[test]
    fn the_karaoke_front_crosses_a_run_in_one_motion_at_every_step() {
        let timed = |text: &str, pieces: &[(&str, f32, f32)]| {
            let mut l = line(pieces[0].1, Some(pieces[pieces.len() - 1].2 + 1.0), text);
            l.words = pieces.iter().map(|(t, a, b)| word(*a, *b, t)).collect();
            assert_eq!(l.words.iter().map(|w| w.text.as_str()).collect::<String>(), text);
            l
        };
        let lines = [
            timed("космоцветок домой", &[("ко", 1.0, 1.2), ("смо", 1.2, 1.5), ("цве", 1.55, 2.6), ("ток ", 2.6, 2.9), ("до", 4.0, 4.3), ("мой", 4.3, 5.0)]),
            timed("Somewhere close to me", &[("Some", 1.0, 1.18), ("where ", 1.18, 1.5), ("close ", 1.5, 2.4), ("to ", 2.45, 2.6), ("me", 2.6, 3.4)]),
            timed("夜空に光る星", &[("夜", 1.0, 1.3), ("空", 1.3, 1.5), ("に", 1.5, 1.6), ("光", 1.6, 2.4), ("る", 2.4, 2.55), ("星", 2.55, 3.6)]),
        ];
        with_painter(|painter| {
            for l in &lines {
                let g = fixed_layout(painter, &l.text, 26.0);
                for by in [By::Letters, By::Syllables, By::Words] {
                    for smooth in [0.0, 0.5, 1.0] {
                        let case = format!("«{}», {by:?}, smooth {smooth}", l.text);
                        let look = Look { style: Style::Karaoke, by };
                        let pace = Pace::of(&Prefs { lyrics_smooth: smooth, ..Prefs::default() });
                        let t = Timing::of(l, &g, look, pace, 26.0).unwrap();
                        let (front, ease) = (&t.front, Ease::of(smooth));
                        let edge = front.edge;
                        assert!((edge - pace.edge(26.0)).abs() < 1e-4 && edge > 2.0);
                        // Every letter goes with a run, the runs in order.
                        let runs: Vec<usize> = t.glyphs.iter().zip(&front.of).filter(|(s, _)| s.is_some()).map(|(_, r)| r.expect("a run")).collect();
                        assert!(runs.windows(2).all(|w| w[0] <= w[1]) && runs.last() == Some(&(front.runs.len() - 1)), "{case}");
                        for (r, run) in front.runs.iter().enumerate() {
                            let own = || t.xs.iter().zip(&front.of).filter(move |(_, o)| **o == Some(r)).map(|(x, _)| *x);
                            let (from, to) = (own().map(|x| x.0).fold(f32::MAX, f32::min), own().map(|x| x.1).fold(0.0, f32::max));
                            let (start, rest) = run.times();
                            assert_eq!((front.head(r, start), front.head(r, rest)), (f32::NEG_INFINITY, f32::INFINITY), "{case}");
                            for fps in [60.0, 142.0] {
                                let (mut was, mut seen) = (f32::NEG_INFINITY, 0);
                                let mut frame = 0;
                                loop {
                                    let p = start - 0.1 + f64::from(frame) / fps;
                                    frame += 1;
                                    let h = front.head(r, p);
                                    if h.is_finite() {
                                        if was.is_finite() {
                                            assert!(h >= was - 1e-3, "{case}, run {r} at {p:.3}: runs back");
                                            assert!(h - was < 900.0 / fps as f32, "{case}, run {r} at {p:.3}: jumps by {}", h - was);
                                        } else {
                                            assert!(seen == 0 && h < from + 900.0 / fps as f32, "{case}, run {r}: comes in at {h}, not {from}");
                                        }
                                        seen += 1;
                                    } else if was.is_finite() {
                                        assert!(h == f32::INFINITY && was > to + edge - 900.0 / fps as f32, "{case}, run {r}: leaves at {was}, not {}", to + edge);
                                    }
                                    was = h;
                                    if p > rest + 0.1 {
                                        break;
                                    }
                                }
                                assert!(seen > 5 && was == f32::INFINITY, "{case}, run {r}: {seen} frames");
                            }
                        }
                        // The middle of the edge is the front: in no piece
                        // before it is sung, through it within the
                        // tolerance after.
                        for (start, end, from, to, first) in spans(&t, by) {
                            let run = &front.runs[front.of[first].expect("a run")];
                            let before = run.at(f64::from(start));
                            assert!(before <= from + 0.05, "{case}: at {before} when the piece at {from} starts");
                            let after = run.at(f64::from(end + ease.behind()));
                            assert!(after >= to - 0.05, "{case}: at {after}, the piece to {to} ended {} s ago", ease.behind());
                        }
                    }
                }
            }
            // A pause parts the runs: in it the words sung are lit whole,
            // their space too, and the next has not begun, at any step.
            let l = &lines[0];
            let g = fixed_layout(painter, &l.text, 26.0);
            for by in [By::Letters, By::Syllables, By::Words] {
                let t = timing(l, &g, Look { style: Style::Karaoke, by });
                assert_eq!(t.front.runs.len(), 2, "{by:?}");
                for p in [3.4, 3.7, 4.0] {
                    assert_eq!((t.front.head(0, p), t.front.head(1, p)), (f32::INFINITY, f32::NEG_INFINITY), "{by:?} at {p}");
                }
                assert!(t.front.of[..11].iter().all(|r| *r == Some(0)) && t.front.of[11].is_none() && t.front.of[12..].iter().all(|r| *r == Some(1)));
            }
        });
    }

    /// «Плавность» evens the front out: the quick syllables of a line are
    /// crossed at paces closer to each other the smoother it is, a short
    /// piece between calmer ones no longer flying by; the note held long
    /// still takes its own long time, and warms up while it is held.
    #[test]
    fn smoothness_evens_the_front_out_and_a_held_note_stays_long() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(5.0), "тататам там тарарам");
            l.words = [("та", 1.0, 1.1), ("та", 1.1, 1.36), ("там ", 1.36, 1.46), ("там ", 1.5, 3.1), ("та", 3.1, 3.18), ("ра", 3.18, 3.42), ("рам", 3.42, 3.6)]
                .map(|(t, a, b)| word(a, b, t))
                .to_vec();
            let g = fixed_layout(painter, &l.text, 26.0);
            let front = |smooth: f32| {
                let pace = Pace::of(&Prefs { lyrics_smooth: smooth, ..Prefs::default() });
                Timing::of(&l, &g, Look { style: Style::Karaoke, by: By::Syllables }, pace, 26.0).unwrap()
            };
            // The fastest the front goes from frame to frame, and how long
            // it takes over the held «там».
            let measure = |t: &Timing| {
                let run = &t.front.runs[0];
                let fastest = (0..800).map(|f| (run.at(0.9 + f64::from(f + 1) / 240.0) - run.at(0.9 + f64::from(f) / 240.0)) * 240.0).fold(0.0f32, f32::max);
                let held = &t.syllables[3];
                let cross = |x: f32| (0..4000).map(|ms| 0.9 + f64::from(ms) / 1000.0).find(|p| run.at(*p) >= x).expect("crossed");
                (fastest, (cross(held.to - 1.0) - cross(held.from + 1.0)) as f32)
            };
            let (exact, calm, calmest) = (measure(&front(0.0)), measure(&front(0.5)), measure(&front(1.0)));
            assert_eq!(front(0.5).front.runs.len(), 1);
            assert!(calm.0 < 0.8 * exact.0 && calmest.0 < calm.0, "{exact:?} {calm:?} {calmest:?}");
            for held in [exact.1, calm.1, calmest.1] {
                assert!(held > 1.2 && held < 1.75, "the held note takes {held} s");
            }
            // 1.6 s over three letters is a held note; «рам» is not.
            assert_eq!(held_note(1.6, 3), 1.0);
            assert_eq!(held_note(0.18, 3), 0.0);
            assert!(held_note(0.9, 3) > 0.0 && held_note(0.9, 3) < 1.0 && held_note(2.0, 12) < 0.35);
            let mut was = 0.0;
            for ms in 1400..3600 {
                let p = ms as f32 / 1000.0;
                let (up, glow) = (held_in(1.5, p), held_in(1.5, p) * held_envelope(3.1, p));
                assert!(up >= was && glow <= up && (up - was) < 0.01, "{ms}");
                was = up;
            }
            assert!(held_in(1.5, 1.5) == 0.0 && held_in(1.5, 1.51 + HELD_IN) == 1.0 && held_envelope(3.1, 3.11 + SETTLE) == 0.0);
        });
    }

    /// Smoothed light and jumps: the light still only rises, starts half a
    /// window early and is whole a window after; at once becomes a short
    /// even ramp, not a step; a jump spreads into a lower, longer rise.
    #[test]
    fn smoothing_spreads_the_light_and_the_jump_evenly() {
        let w = 0.2;
        for rise in [0.0, 0.06, 0.3] {
            let mut was = 0.0;
            for ms in -400..900 {
                let t = ms as f32 / 1000.0;
                let v = light_over(t, rise, w);
                assert!(v >= was - 1e-5 && v <= 1.0, "{rise} {ms}");
                assert!(v - was < 0.02, "{rise} {ms}: a step");
                // The integral against a fine numeric average.
                let n = 400;
                let avg: f32 = (0..n).map(|i| light_up(t - w * 0.5 + w * (i as f32 + 0.5) / n as f32, rise)).sum::<f32>() / n as f32;
                assert!((v - avg).abs() < 0.01, "{rise} {ms}: {v} vs {avg}");
                was = v;
            }
            assert_eq!(light_over(-ONSET_LEAD - w * 0.5 - 0.001, rise, w), 0.0);
            assert!(light_over(w * 0.5 + rise * 3.0 + 0.1, rise, w) > 0.99);
        }
        assert_eq!(light_over(0.1, 0.06, 0.0), light_up(0.1, 0.06));
        // The jump: smooth, lower peak, the same rest.
        let mut was = 0.0;
        let mut top = 0.0f32;
        for ms in -300..800 {
            let v = jump_over(ms as f32 / 1000.0, 1.0, w);
            assert!((v - was).abs() < 0.08, "{ms}");
            top = top.max(v);
            was = v;
        }
        assert!(top > 1.05 && top < (0..400).map(|ms| jump_height(ms as f32 / 1000.0, 1.0)).fold(0.0, f32::max));
        assert!((jump_over(1.0, 1.0, w) - 1.0).abs() < 1e-3);
        assert_eq!(jump_over(0.05, 1.0, 0.0), jump_height(0.05, 1.0));
        assert_eq!(jump_over(-w, 1.0, w), 0.0);
    }

    /// Each glyph's colour as the lyrics view paints line `l` (alone, paused
    /// at `at` s) under `prefs`, once its springs have settled, and the lit
    /// colour.
    fn painted_glyphs(prefs: &Prefs, l: &LyricLine, at: f32) -> (Vec<Option<Color32>>, Color32) {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.prefs = Prefs { lyrics_soft_edges: false, ..prefs.clone() };
        state.player.status = Status::Paused;
        state.player.position = Duration::from_secs_f32(at);
        state.lyrics.lines = vec![l.clone()];
        let mut colours = Vec::new();
        for frame in 0..12 {
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(640.0, 700.0))),
                time: Some(frame as f64 * 0.25), ..Default::default()
            }, |ui| {
                show(ui, &state, 0.0, false, &mut Vec::new());
            });
            output.textures_delta.clear();
            colours.clear();
            for shape in &output.shapes {
                let egui::Shape::Text(text) = &shape.shape else { continue };
                if text.galley.job.text != l.text {
                    continue;
                }
                for row in &text.galley.rows {
                    for gl in &row.row.glyphs {
                        colours.push((!gl.uv_rect.is_nothing()).then(|| {
                            text.override_text_color.unwrap_or(row.row.visuals.mesh.vertices[gl.first_vertex as usize].color)
                        }));
                    }
                }
            }
        }
        (colours, Theme::get(&ctx).scheme.on_surface)
    }

    /// The report: «Подсвечивать: По словам» lit nothing under karaoke and
    /// only weakly under the wave. Every look, by letters and by words, over
    /// word, syllable and line timing (guessed or the calm fallback): the
    /// words already sung are fully lit, the ones to come are not (unless
    /// the look lights the line whole).
    #[test]
    fn sung_words_are_fully_lit_in_every_look() {
        let text = "Свет из окна ложится";
        let worded = {
            let mut l = line(1.0, Some(5.0), text);
            l.words = vec![word(1.0, 1.6, "Свет "), word(1.6, 2.0, "из "), word(2.0, 3.0, "окна "), word(3.0, 5.0, "ложится")];
            l
        };
        let syllables = {
            let mut l = line(1.0, Some(5.0), text);
            l.words = [("Св", 1.0, 1.3), ("ет ", 1.3, 1.6), ("из ", 1.6, 2.0), ("ок", 2.0, 2.5), ("на ", 2.5, 3.0), ("ло", 3.0, 3.8), ("жит", 3.8, 4.4), ("ся", 4.4, 5.0)]
                .map(|(t, a, b)| word(a, b, t)).to_vec();
            l
        };
        let lines_only = line(1.0, Some(5.0), text);
        let close = |a: Color32, b: Color32| a.to_array().iter().zip(b.to_array()).all(|(x, y)| x.abs_diff(y) <= 3);
        for (timing, l, calm) in [("word", &worded, true), ("syllable", &syllables, true), ("guessed", &lines_only, false), ("calm", &lines_only, true)] {
            for style in 0..3u8 {
                for unit in [nikgob_view::LYRICS_BY_LETTERS, nikgob_view::LYRICS_BY_SYLLABLES, nikgob_view::LYRICS_BY_WORDS] {
                    let letters = unit == nikgob_view::LYRICS_BY_LETTERS;
                    for smooth in [0.0, 0.5, 1.0] {
                        let prefs = Prefs { lyrics_style: style, lyrics_unit: unit, wave_syllables_only: calm, lyrics_smooth: smooth, ..Prefs::default() };
                        let case = format!("{timing}, style {style}, unit {unit}, smooth {smooth}");
                        let (glyphs, lit) = painted_glyphs(&prefs, l, 2.4);
                        assert_eq!(glyphs.len(), text.chars().count(), "{case}: the line was not painted with its timing");
                        // Focus by syllables or words has no fill: the word
                        // sung and the word to come are one tone, the
                        // line's, and not the lit one.
                        if style == 1 && !letters && timing != "calm" {
                            // (A guessed «Свет» has only just been sung.)
                            if timing != "guessed" {
                                assert_eq!(glyphs[0], glyphs[13], "{case}: «Свет» and «ложится» differ");
                            }
                            assert!(!close(glyphs[0].unwrap(), lit), "{case}: the line is lit whole");
                            continue;
                        }
                        // «Свет» was sung over a second ago.
                        for (k, c) in glyphs[..4].iter().enumerate() {
                            assert!(close(c.unwrap(), lit), "{case}: letter {k} is {:?}, lit is {lit:?}", c.unwrap());
                        }
                        // «ложится» is still to come.
                        let whole = (style == 1 && letters) || timing == "calm";
                        for (k, c) in glyphs[13..].iter().enumerate() {
                            assert_eq!(close(c.unwrap(), lit), whole, "{case}: letter {} is {:?}", 13 + k, c.unwrap());
                        }
                    }
                }
            }
        }
    }

    /// The painted position holds against a small step back while playing
    /// (the clock correcting itself), and follows seeks and pauses.
    #[test]
    fn the_painted_position_never_steps_back_by_a_hair() {
        let ms = Duration::from_millis;
        assert_eq!(steady_pos(Some(ms(10_000)), ms(9_800), true), ms(10_000));
        assert_eq!(steady_pos(Some(ms(10_000)), ms(10_020), true), ms(10_020));
        assert_eq!(steady_pos(Some(ms(10_000)), ms(9_000), true), ms(9_000), "a seek");
        assert_eq!(steady_pos(Some(ms(10_000)), ms(9_800), false), ms(9_800), "paused");
        assert_eq!(steady_pos(None, ms(5), true), ms(5));
        let mut last = None;
        let mut was = Duration::ZERO;
        for t in [100, 200, 180, 300, 290, 400] {
            let p = steady_pos(last, ms(t), true);
            assert!(p >= was);
            (last, was) = (Some(p), p);
        }
    }

    /// A raised letter goes onto whole pixels only after its jump is over,
    /// and off them as its line goes, each without a step.
    #[test]
    fn a_lift_eases_onto_and_off_whole_pixels() {
        assert_eq!(crisp_lift(-0.1, 1.0), 0.0, "still moving");
        assert_eq!(crisp_lift(0.0, 1.0), 0.0);
        assert_eq!(crisp_lift(CRISP, 1.0), 1.0);
        assert_eq!(crisp_lift(5.0, 0.9), 0.0, "going down with its line");
        assert_eq!(crisp_lift(5.0, 0.5), 0.0);
        let mut was = 0.0;
        for i in 0..=100 {
            let c = crisp_lift(i as f32 * CRISP / 100.0, 1.0);
            assert!(c >= was && c - was < 0.05, "{i}: {was} → {c}");
            was = c;
        }
        let mut was = 1.0;
        for i in 0..=100 {
            let c = crisp_lift(5.0, 1.0 - i as f32 * 0.001);
            assert!(c <= was && was - c < 0.05, "{i}: {was} → {c}");
            was = c;
        }
    }

    /// The view's clock at 240 frames a second: the player's position read
    /// a little early or late in each frame and put right by 80 ms mid-way
    /// never makes it hop, stop or go back; it is with the player again
    /// within a second; a seek, a pause and the track's end are followed
    /// at once.
    #[test]
    fn the_clock_runs_evenly_and_comes_to_the_player_without_a_hop() {
        let dt = 1.0 / 240.0;
        // The player: 10 s at frame 0, running at the pace of the frames,
        // read up to 3 ms off, and 80 ms further on from frame 240.
        let heard = |f: usize| {
            let jitter = [0.003, -0.002, 0.001, -0.003, 0.002][f % 5];
            10.0 + f as f64 * dt + jitter + if f >= 240 { 0.08 } else { 0.0 }
        };
        let mut c = Clock::step(None, heard(0), 0.0, 1.0, None, true);
        assert_eq!(c.pos, heard(0), "a first frame is where the player is");
        let (mut least, mut most) = (f64::INFINITY, 0.0f64);
        for f in 1..720 {
            let next = Clock::step(Some(c), heard(f), f as f64 * dt, 1.0, None, true);
            let step = next.pos - c.pos;
            least = least.min(step);
            most = most.max(step);
            // Before the correction it runs at the frames' pace, the
            // reading's own unevenness all but gone.
            if f < 240 {
                assert!((step - dt).abs() < dt * 0.1, "frame {f}: {step}");
            }
            c = next;
        }
        assert!(least >= dt * CLOCK_SLOWEST - 1e-9, "never stops or goes back: {least}");
        assert!(most < dt * 2.0, "the correction is run up to, not hopped: {most}");
        assert!((c.pos - (10.0 + 719.0 * dt + 0.08)).abs() < 0.005, "with the player again: {}", c.pos);
        // The player behind by a little: the clock slows down for it.
        let slow = Clock::step(Some(c), c.pos + dt - 0.1, c.at + dt, 1.0, None, true);
        assert!(slow.pos > c.pos && slow.pos < c.pos + dt);
        // A seek either way, and a pause, at once.
        assert_eq!(Clock::step(Some(c), 95.0, c.at + dt, 1.0, None, true).pos, 95.0);
        assert_eq!(Clock::step(Some(c), 2.0, c.at + dt, 1.0, None, true).pos, 2.0);
        assert_eq!(Clock::step(Some(c), c.pos - 0.01, c.at + dt, 1.0, None, false).pos, c.pos - 0.01);
        // A stalled sound stands still; the track's end is not run past.
        assert_eq!(Clock::step(Some(c), c.pos, c.at + dt, 0.0, None, true).pos, c.pos);
        let end = c.pos + 0.001;
        assert_eq!(Clock::step(Some(c), end, c.at + dt, 1.0, Some(end), true).pos, end);
    }

    /// A text between whole pixels is handed to the painter on them with its
    /// rows moved by the rest, so it is drawn where it is, not on the pixel
    /// nearest; one on whole pixels is drawn as it is.
    #[test]
    fn a_moving_text_is_drawn_between_whole_pixels() {
        with_painter(|painter| {
            let g = fixed_layout(painter, "Свет из окна\nложится на пол", 26.0);
            let ppp = painter.pixels_per_point();
            let (at, same) = afloat(painter, pos2(40.0, 120.0), &g);
            assert_eq!(at, pos2(40.0, 120.0));
            assert!(Arc::ptr_eq(&same, &g), "at rest: the layout itself");
            // Gliding down a pixel in 240 steps: every step moves it.
            let mut was = at.y + g.rows[1].pos.y;
            for step in 1..=240 {
                let y = 120.0 + step as f32 / 240.0 / ppp;
                let (at, moved) = afloat(painter, pos2(40.0, y), &g);
                assert!(((at.y * ppp).round() - at.y * ppp).abs() < 1e-3, "the origin is on whole pixels");
                assert_eq!(moved.rows.len(), g.rows.len());
                // (Up to the hair within which it counts as at rest.)
                for (m, r) in moved.rows.iter().zip(&g.rows) {
                    assert!((at.y + m.pos.y - (y + r.pos.y)).abs() < 0.021 / ppp, "step {step}");
                    assert_eq!(m.pos.x, r.pos.x);
                }
                let drawn = at.y + moved.rows[1].pos.y;
                assert!(drawn >= was - 1e-4 && drawn - was < 0.05 / ppp, "step {step}: {was} → {drawn}");
                was = drawn;
            }
        });
    }

    /// A growing line is one layout scaled: each letter moves along the
    /// line evenly, a little every frame, with one small shift at most (off
    /// the small resting layout), where a layout at every whole size on the
    /// way shook the letters against each other.
    #[test]
    fn a_growing_lines_letters_keep_their_places() {
        with_painter(|painter| {
            let text = "Слышишь этот ритм, он ведёт меня";
            let rest = (20.0, 26.0);
            let xs = |g: &Galley| g.rows[0].row.glyphs.iter().map(|gl| gl.pos.x).collect::<Vec<_>>();
            let big = xs(&*fixed_layout(painter, text, rest.1));
            let mut was = xs(&*moving_layout(painter, text, rest, 1.0 / 240.0));
            let mut shifts = 0;
            for step in 2..240 {
                let t = step as f32 / 240.0;
                let now = xs(&*moving_layout(painter, text, rest, t));
                assert_eq!(now.len(), big.len());
                // What a letter's place grows by in one step, at most.
                let even = big.last().unwrap() * (rest.1 - rest.0) / rest.1 / 240.0 + 0.01;
                let off = now.iter().zip(&was).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
                if off > even {
                    shifts += 1;
                    assert!(off < 1.5, "step {step}: {off}");
                }
                was = now;
            }
            assert!(shifts <= 1, "{shifts} shifts");
            // Above the small resting size it is the big layout, scaled.
            let size = rest.0 + (rest.1 - rest.0) * 0.5;
            for (x, b) in xs(&*moving_layout(painter, text, rest, 0.5)).iter().zip(&big) {
                assert!((x - b * size / rest.1).abs() < 1e-3);
            }
        });
    }

    /// The text follows the current line without swinging past its place,
    /// lines further away later; a wave piece's jump comes off the line
    /// over a few frames whatever «Плавность» is.
    #[test]
    fn the_scroll_settles_without_overshoot_and_a_jump_eases_off_the_line() {
        for far in 0..=8 {
            let spring = follow_spring(far as f32);
            let (mut was, mut fastest) = (1.0f32, 0.0f32);
            for f in 1..=480 {
                // What is left of the way, from all of it at rest.
                let (left, _) = spring.solve(1.0, 0.0, f as f32 / 240.0);
                assert!(left >= -1e-4 && left <= was + 1e-5, "far {far}, frame {f}: {was} → {left}");
                fastest = fastest.max(was - left);
                was = left;
            }
            assert!(fastest < 0.03, "far {far}: {fastest} of the way in a frame");
            assert!(was < 0.01, "far {far}: there within two seconds");
            if far > 0 {
                assert!(spring.stiffness <= follow_spring(far as f32 - 1.0).stiffness);
            }
        }
        assert_eq!(follow_spring(0.0), FOLLOW);
        let fastest = |window: f32| {
            (-60..240).map(|f| jump_over((f + 1) as f32 / 240.0, 1.0, window) - jump_over(f as f32 / 240.0, 1.0, window))
                .fold(0.0f32, f32::max)
        };
        assert!(fastest(JUMP_EASE) < fastest(0.0) * 0.7, "{} against {}", fastest(JUMP_EASE), fastest(0.0));
        assert!(fastest(JUMP_EASE) < 0.16);
    }

    /// The same song's text swapped (timing made, another source) keeps each
    /// line's slot, so its place, focus and wait go on; another song's text
    /// takes the slots by place.
    #[test]
    fn a_swapped_text_keeps_each_line_in_its_slot() {
        let plain = vec![line(1.0, Some(2.0), "Раз"), line(2.0, Some(3.0), "Два"), line(6.0, Some(7.0), "Раз"), line(7.0, Some(8.0), "Три")];
        let items = display_lines(&plain);
        let keys = item_keys(&plain, &items);
        assert_eq!(keys.len(), items.len());
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len(), "a repeated line is told apart");
        let first = carry_slots(&Slots::default(), &keys);
        assert_eq!(first.slots, (0..keys.len() as u64).collect::<Vec<_>>());
        // Made timing ends the second line sooner: a wait comes in before
        // the third line, the lines keep their slots and the wait gets one.
        let mut made = plain.clone();
        made[1].end = Some(Duration::from_secs_f32(2.0));
        let made_items = display_lines(&made);
        assert!(made_items.len() > items.len(), "{made_items:?}");
        let swapped = carry_slots(&first, &item_keys(&made, &made_items));
        for (i, item) in made_items.iter().enumerate() {
            if let DisplayLine::Line(j) = item {
                let was = items.iter().position(|it| *it == DisplayLine::Line(*j)).unwrap();
                assert_eq!(swapped.slots[i], first.slots[was], "line {j}");
            } else {
                assert!(!first.slots.contains(&swapped.slots[i]), "the new wait has a slot of its own");
            }
        }
        // Another song: by place.
        let other = vec![line(1.0, Some(2.0), "Иное"), line(2.0, Some(3.0), "Совсем")];
        let other_items = display_lines(&other);
        let next = carry_slots(&swapped, &item_keys(&other, &other_items));
        assert_eq!(next.slots[..], swapped.slots[..other_items.len()]);
    }

    /// While a click holds its line, the lyrics are painted where its seek
    /// will land, so the painted position meets the seek's without a jump.
    #[test]
    fn a_held_click_is_painted_where_its_seek_lands() {
        let ms = Duration::from_millis;
        let j = Jump { item: DisplayLine::Line(3), to: ms(20_000), at: 1.0 };
        assert_eq!(jump_pos(None, ms(5_000), Duration::ZERO, 0.2), ms(5_000));
        // A lead ahead: where the seek lands is its point plus the lead.
        assert_eq!(jump_pos(Some(j), ms(5_000), Duration::ZERO, 0.2), led(ms(20_000), 0.2));
        assert_eq!(jump_pos(Some(j), ms(40_000), Duration::ZERO, 0.0), ms(20_000));
        // A lead behind: the seek lands behind its point and is followed.
        let back = Duration::from_secs_f32(0.3);
        let landed = led(ms(20_000), -0.3);
        assert_eq!(jump_pos(Some(j), ms(5_000), back, -0.3), landed);
        assert_eq!(jump_pos(Some(j), landed, back, -0.3), landed);
        assert_eq!(jump_pos(Some(j), ms(19_850), back, -0.3), ms(19_850));
    }

    /// A line that is no longer current goes on being sung from where it
    /// was (no jump to the end when the next line comes in mid-word), and
    /// stays as it was when the song went back before it.
    #[test]
    fn a_line_left_goes_on_from_where_it_was_sung() {
        let ms = Duration::from_millis;
        assert_eq!(sung_pos(Some(ms(3_000)), ms(3_016)), ms(3_016), "on with the song");
        assert_eq!(sung_pos(Some(ms(3_000)), ms(1_000)), ms(3_000), "a seek back holds it");
        assert_eq!(sung_pos(Some(ms(3_000)), ms(9_000)), ms(3_000), "so does a seek on");
        assert_eq!(sung_pos(None, ms(1_000)), ms(1_000));
        // Frame by frame it keeps up with the song for as long as it fades.
        let mut last = Some(ms(3_000));
        for f in 1..60 {
            let p = sung_pos(last, ms(3_000 + 16 * f));
            assert_eq!(p, ms(3_000 + 16 * f));
            last = Some(p);
        }
        // Karaoke: the front where the line was left is where it goes on
        // from, instead of the whole line lit at once.
        let mut l = line(1.0, Some(4.0), "Свет из окна");
        l.words = vec![word(1.0, 2.0, "Свет "), word(2.0, 2.5, "из "), word(2.5, 4.0, "окна")];
        with_painter(|painter| {
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = timing(&l, &g, Look { style: Style::Karaoke, by: By::Syllables });
            let left = ms(3_000);
            let head = |at: Duration| t.front.head(0, at.as_secs_f64());
            let (at, next) = (head(left), head(sung_pos(Some(left), left + ms(16))));
            assert!(at.is_finite() && next >= at && next - at < 5.0, "{at} → {next}");
        });
    }

    /// The unit is what the karaoke front keeps time by. By syllables each
    /// piece fills evenly across its letters over its own time: a fast «ко»
    /// and «смо», a held «цве», then «ток». By letters its vowel takes
    /// longer; by words the whole word is crossed evenly.
    #[test]
    fn a_syllable_fills_evenly_over_its_own_time() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "космоцветок");
            l.words = vec![word(1.0, 1.1, "ко"), word(1.1, 1.25, "смо"), word(1.25, 2.1, "цве"), word(2.1, 2.3, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            // On the times exactly.
            let exact = |by| Timing::build(&l, &g, Look { style: Style::Karaoke, by }, 0.0, Ease::EXACT).unwrap();
            let t = exact(By::Syllables);
            let at = |p: f32| t.front.runs[0].way(f64::from(p));
            let s = &t.syllables[2];
            // Halfway through its time the front is halfway across it.
            let mid = at((s.start + s.end) * 0.5);
            assert!((mid - (s.from + s.to) * 0.5).abs() < 0.5, "{mid} in {}..{}", s.from, s.to);
            assert!((at(s.start) - s.from).abs() < 0.05 && (at(s.end) - s.to).abs() < 0.05);
            // The held one runs slowest.
            let pace = |i: usize| (t.syllables[i].to - t.syllables[i].from) / (t.syllables[i].end - t.syllables[i].start);
            assert!(pace(2) < pace(0) && pace(2) < pace(1));
            // By letters the same piece is weighted (its vowel takes longer).
            let letters = exact(By::Letters);
            assert!((letters.front.runs[0].way(f64::from((s.start + s.end) * 0.5)) - mid).abs() > 0.5);
            // By words the front is halfway across the word halfway through
            // it, whatever its syllables do (they are well past that).
            let words = exact(By::Words);
            let (from, to) = (t.syllables[0].from, t.syllables[3].to);
            let half = words.front.runs[0].way(1.65);
            assert!((half - (from + to) * 0.5).abs() < 0.5 && (half - at(1.65)).abs() > 5.0, "{half} against {}", at(1.65));
        });
    }

    /// «Шаг подсветки»: karaoke follows its front at every step; under
    /// Focus by syllables each syllable lights whole as it starts, by words
    /// each word at its first syllable; the wave lights letter by letter,
    /// by words the word whole.
    #[test]
    fn the_unit_sets_how_the_light_steps() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "космо цветок");
            l.words = vec![word(1.0, 1.4, "кос"), word(1.4, 1.8, "мо "), word(1.8, 2.4, "цве"), word(2.4, 2.8, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let of = |style, by| timing(&l, &g, Look { style, by });
            // Glyphs: к о с м о ␠ ц в е т о к.
            for by in [By::Letters, By::Syllables, By::Words] {
                let t = of(Style::Karaoke, by);
                assert!(t.lit.iter().all(Option::is_none), "the front");
                assert_eq!(t.front.runs.len(), 1);
            }
            let s = of(Style::Focus, By::Syllables).lit;
            assert_eq!(s[0], s[2], "«кос» lights whole");
            assert_eq!(s[3], s[4], "«мо» lights whole");
            assert!(s[0] == Some(1.0) && s[3] > s[2], "a syllable after the other");
            assert_eq!(s[5], None, "a space has none");
            let w = of(Style::Focus, By::Words).lit;
            assert_eq!(w[0], w[4], "«космо» lights whole");
            assert!(w[6] > w[4] && w[6] == w[11]);
            // Focus by letters lights the whole line: nothing goes along it.
            let calm = of(Style::Focus, By::Letters);
            assert!(calm.lit.iter().all(Option::is_none) && calm.front.runs.is_empty());
            for by in [By::Letters, By::Syllables] {
                let wave = of(Style::Wave, by).lit;
                assert!(wave[1] > wave[0] && wave[2] > wave[1] && wave[3] > wave[2], "{by:?}: letter by letter");
                // No letter before its piece is sung, none much after it.
                assert!(wave[0] == Some(1.0) && wave[6] >= Some(1.8) && wave[11] < Some(2.8 + 0.2), "{by:?}: {wave:?}");
            }
            let wave = of(Style::Wave, By::Words).lit;
            assert_eq!(wave[0], wave[4], "the word jumps whole");
        });
    }

    /// A line's timing is worked out once for its layout, look and settings
    /// and kept: the same one comes back frame after frame, another for
    /// another unit, «Плавность» or timing, and only a few are kept.
    #[test]
    fn a_lines_timing_is_kept_from_frame_to_frame() {
        with_painter(|painter| {
            let ctx = painter.ctx();
            let mut l = line(1.0, Some(3.0), "космо цветок");
            l.words = vec![word(1.0, 1.4, "кос"), word(1.4, 1.8, "мо "), word(1.8, 2.4, "цве"), word(2.4, 2.8, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let pace = Pace::of(&Prefs::default());
            let look = Look { style: Style::Karaoke, by: By::Syllables };
            let kept = timing_of(ctx, &l, &g, look, pace, 26.0).unwrap();
            assert!(Arc::ptr_eq(&kept, &timing_of(ctx, &l, &g, look, pace, 26.0).unwrap()));
            let other = |l: &LyricLine, look: Look, pace: Pace| !Arc::ptr_eq(&kept, &timing_of(ctx, l, &g, look, pace, 26.0).unwrap());
            assert!(other(&l, Look { by: By::Words, ..look }, pace));
            assert!(other(&l, look, Pace { smooth: 1.0, ..pace }));
            let mut retimed = l.clone();
            retimed.words[3].end = Duration::from_secs_f32(2.9);
            assert!(other(&retimed, look, pace));
            // A line that grows is laid out anew every frame: its timings
            // push the oldest out.
            for size in 10..40 {
                timing_of(ctx, &l, &fixed_layout(painter, &l.text, size as f32), look, pace, size as f32);
            }
            let count = ctx.data_mut(|d| d.get_temp_mut_or_default::<Timings>(egui::Id::new("lyrics-timings")).0.len());
            assert_eq!(count, TIMINGS);
            assert!(timing_of(ctx, &line(1.0, None, "no words"), &g, look, pace, 26.0).is_none());
        });
    }

    /// «Буквы» must not swallow a short word, at full «Плавность» either:
    /// its letters light one after another as the front goes through it.
    /// The view paints [`LYRICS_LEAD`] ahead of the player, so with the lead
    /// off 1.15 is early in «Свет » (1.0 to 1.35) and 1.33 near its end.
    #[test]
    fn letters_of_a_word_light_one_by_one() {
        use nikgob_view::{LYRICS_BY_LETTERS, LYRICS_BY_WORDS};
        let mut l = line(1.0, Some(3.0), "Свет окна");
        l.words = vec![word(1.0, 1.35, "Свет "), word(2.0, 3.0, "окна")];
        let close = |a: Color32, b: Color32| a.to_array().iter().zip(b.to_array()).all(|(x, y)| x.abs_diff(y) <= 3);
        let bright = |c: Color32| c.to_array().iter().map(|v| u32::from(*v)).sum::<u32>();
        let letters = Prefs {
            lyrics_style: 0,
            lyrics_unit: LYRICS_BY_LETTERS,
            lyrics_smooth: 1.0,
            lyrics_lead: 0.0,
            ..Prefs::default()
        };
        // By words the front crosses the word too: it never lights at once.
        let words = Prefs { lyrics_unit: LYRICS_BY_WORDS, ..letters.clone() };
        for (unit, prefs) in [("letters", &letters), ("words", &words)] {
            let (glyphs, lit) = painted_glyphs(prefs, &l, 1.15);
            assert!(glyphs.len() >= 5, "{:?}", glyphs.len());
            let (first, last, next) = (glyphs[0].expect("«С» is painted"), glyphs[3].expect("«т» is painted"), glyphs[5].expect("«о» is painted"));
            assert!(bright(first) > bright(last), "{unit}: «С» lights before «т»: {first:?} {last:?}");
            assert!(!close(last, lit) && last == next, "{unit}: «т» is not sung yet: {last:?}");
            let (glyphs, lit) = painted_glyphs(prefs, &l, 1.33);
            let (first, last) = (glyphs[0].unwrap(), glyphs[3].unwrap());
            assert!(close(first, lit), "{unit}: «С» is sung: {first:?}");
            assert!(!close(last, lit), "{unit}: «т» is still being sung: {last:?}");
            // Sung and in the pause: the word is lit whole, the next not.
            let (glyphs, lit) = painted_glyphs(prefs, &l, 1.9);
            assert!(glyphs[..4].iter().all(|c| close(c.unwrap(), lit)) && !close(glyphs[5].unwrap(), lit), "{unit}: in the pause");
        }
    }

    /// Focus by syllables or words: a piece stands out while it is sung and
    /// goes back to the line's tone after it, evenly; nothing fills.
    #[test]
    fn focus_lifts_the_sung_piece_and_lets_it_go() {
        let at = |p: f32| focus_pulse(p, 2.0, 3.0, 0.1, 0.0);
        assert_eq!(at(1.5), 0.0);
        assert!(at(2.5) > 0.99 && at(3.0) > 0.99);
        assert!(at(3.0 + FOCUS_FALL * 0.5) > 0.3 && at(3.0 + FOCUS_FALL * 0.5) < 0.7);
        assert!(at(3.0 + FOCUS_FALL + 0.01) < 1e-3);
        let mut was = 0.0;
        for ms in 1500..4000 {
            let v = at(ms as f32 / 1000.0);
            assert!((v - was).abs() < 0.05, "{ms}: a step");
            was = v;
        }
        // A piece sung in a blink still shows.
        assert!(focus_pulse(2.2, 2.0, 2.05, 0.1, 0.0) > 0.9);
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "космо цветок");
            l.words = vec![word(1.0, 1.4, "кос"), word(1.4, 1.8, "мо "), word(1.8, 2.4, "цве"), word(2.4, 2.8, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let look = |by| Look { style: Style::Focus, by };
            // Glyphs: к о с м о ␠ ц в е т о к.
            assert_eq!(piece_end(&t, 0, look(By::Syllables)), Some(t.syllables[0].end));
            assert_eq!(piece_end(&t, 0, look(By::Words)), Some(t.syllables[1].end));
            assert_eq!(piece_end(&t, 5, look(By::Words)), None);
        });
    }

    /// By syllables under the wave a written word rises with each of its
    /// syllables, all its letters together, and is up in full once the
    /// last is sung.
    #[test]
    fn by_syllables_a_word_rises_with_its_syllables() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "нон-стоп ok");
            l.words = vec![word(1.0, 1.5, "нон-"), word(1.5, 2.5, "стоп "), word(3.0, 3.5, "ok")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::build(&l, &g, Look { style: Style::Wave, by: By::Syllables }, 0.0, Ease::EXACT).unwrap();
            assert_eq!(t.words, [0, 2]);
            let at = |p: f32| t.word_jump(1, p, 1.0, 0.0);
            assert_eq!(at(0.5).0, 0.0);
            // Half up (at rest) between its syllables, then whole.
            assert!((at(1.45).0 - 0.5).abs() < 0.01, "{:?}", at(1.45));
            assert!((at(2.4).0 - 1.0).abs() < 0.01);
            assert_eq!(t.word_jump(0, 1.45, 1.0, 0.0), at(1.45));
            // Seconds since its last syllable, negative before it.
            assert!(at(1.45).1 < 0.0 && (at(2.0).1 - 0.5).abs() < 0.05);
            // The next word has not moved.
            assert_eq!(t.word_jump(2, 2.4, 1.0, 0.0).0, 0.0);
        });
    }

    /// Switching between the calm Focus and a timed look fades from the old
    /// one to the new one, the line never thinning much between them.
    #[test]
    fn a_change_of_look_fades_and_the_line_stays_whole() {
        let calm = Look { style: Style::Focus, by: By::Letters };
        let wave = Look { style: Style::Wave, by: By::Syllables };
        assert_eq!(look_mix(calm, calm, 0.0), 1.0);
        assert_eq!(look_mix(calm, wave, f32::INFINITY), 1.0);
        assert_eq!(look_mix(calm, wave, 0.0), 0.0);
        let mut last = -1.0;
        for ms in 0..400 {
            let m = look_mix(calm, wave, ms as f32 / 1000.0);
            assert!(m >= last && (0.0..=1.0).contains(&m));
            last = m;
        }
        assert_eq!(last, 1.0, "done within 0.4 s");
        assert_eq!(look_fades(0.0), (1.0, 0.0));
        assert_eq!(look_fades(1.0), (0.0, 1.0));
        for k in 0..=20 {
            let (out, inn) = look_fades(k as f32 / 20.0);
            assert!(inn + out - inn * out > 0.85, "{k}");
        }
        // In the view: a first frame shows its look, a change starts a fade.
        let ctx = egui::Context::default();
        let id = egui::Id::new("look");
        assert_eq!(look_change(&ctx, id, calm, 10.0), (None, 1.0));
        assert_eq!(look_change(&ctx, id, wave, 10.0), (Some(calm), 0.0));
        assert_eq!(look_change(&ctx, id, wave, 11.0), (None, 1.0));
    }

    /// A wait new to a swapped text opens from folded: its room's spring,
    /// started at 0, moves to its first target instead of starting there.
    #[test]
    fn a_new_wait_opens_from_folded() {
        let ctx = egui::Context::default();
        let (seeded, plain) = (egui::Id::new("seeded"), egui::Id::new("plain"));
        anim::fraction_starts_at(&ctx, seeded, 0.0);
        assert!(anim::fraction(&ctx, seeded, 1.0, WAIT_ROOM) < 0.01);
        assert_eq!(anim::fraction(&ctx, plain, 1.0, WAIT_ROOM), 1.0);
        // One that moves already is left as it is.
        anim::fraction_starts_at(&ctx, plain, 0.0);
        assert_eq!(anim::fraction(&ctx, plain, 1.0, WAIT_ROOM), 1.0);
    }

    /// Mega mode: another text moves the block of lines to its new middle
    /// on a spring instead of jumping there; a resize moves it at once.
    #[test]
    fn the_mega_block_glides_to_a_new_text_and_follows_a_resize() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        let mut state = ViewState::default();
        state.player.status = Status::Paused;
        state.prefs.lyrics_soft_edges = false;
        let left_at = |state: &ViewState, time: f64, width: f32| -> f32 {
            let mut left = 0.0;
            let mut output = ctx.run_ui(egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 700.0))),
                time: Some(time), ..Default::default()
            }, |ui| {
                show(ui, state, 0.0, true, &mut Vec::new());
                left = ui.ctx().data(|d| d.get_temp::<f32>(ui.id().with("lyrics-view").with("text-left"))).unwrap_or(f32::NAN);
            });
            output.textures_delta.clear();
            left
        };
        state.lyrics.lines = vec![line(1.0, None, "Да"), line(2.0, None, "Нет")];
        let a = left_at(&state, 0.0, 1000.0);
        assert_eq!(left_at(&state, 0.5, 1000.0), a);
        state.lyrics.lines = vec![line(1.0, None, "Строка подлиннее, чем те"), line(2.0, None, "И ещё одна длинная строка")];
        let b0 = left_at(&state, 1.0, 1000.0);
        let b1 = left_at(&state, 3.0, 1000.0);
        assert!((b0 - a).abs() < 1.0, "a new text starts where the block was: {a} → {b0}");
        assert!(b1 < a - 20.0, "then it moves to its own middle: {a} → {b1}");
        let c = left_at(&state, 3.1, 800.0);
        assert!((left_at(&state, 5.0, 800.0) - c).abs() < 0.5, "a resize at once");
    }
}
