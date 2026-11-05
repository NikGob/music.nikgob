//! Honest syllable fill, word lift and sustained notes; line timing only focuses.
//! Both reading scales share frozen wrapping, scrolling and source selection.

use std::sync::Arc;
use std::time::Duration;

use egui::{Align2, Color32, Galley, Rect, Sense, Ui, pos2, text::LayoutJob, vec2};
use nikgob_motion::Spring;
use nikgob_view::{Command, LyricLine, LyricsSync, Prefs, Status, ViewState};

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

/// What an animation goes over («Подсвечивать»).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum By {
    Letters,
    /// The timed pieces: the syllables of a text timed by syllables (the
    /// words of one timed by words), each filled evenly across its letters
    /// over its own time; under the wave a word rises with its syllables.
    Syllables,
    /// Whole written words (each part of a hyphenated one; the syllables
    /// of one light together).
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
    let look = style_for(&state.prefs, &state.lyrics.lines);
    let size = vec2(ui.available_width(), ui.available_height().max(80.0));
    let full = Rect::from_min_size(ui.cursor().min, size);
    ui.allocate_rect(full, Sense::hover());
    // The mega view's control has a row of its own under the lyrics: no
    // line ever lies under it.
    let (live, row) = if mega { control_row(full) } else { (full, None) };
    if mega && state.lyrics.lines.is_empty() {
        let left = plain(ui, live, state);
        return row.is_some_and(|r| control_in(ui, r, left, state, out));
    }
    let resp = ui.interact(live, ui.id().with("lyrics-live"), Sense::click_and_drag());
    let mut sizes = if mega { MEGA } else { PANE };
    let scale = state.prefs.lyrics_scale.clamp(0.8, 1.4);
    sizes.big *= scale;
    sizes.small *= scale;
    sizes.sub *= scale;
    sizes.gap *= scale;
    paint(ui, live, &resp, state, &state.lyrics.lines, look, bass, sizes, mega, out);
    let Some(row) = row else { return false };
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
    let mut sizes = PANE;
    sizes.big = 22.0;
    sizes.small = 17.0;
    sizes.sub = 13.0;
    sizes.gap = 14.0;
    paint(ui, rect, &resp, state, lines, look, 0.0, sizes, false, out);
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
    let pos = steady(&ctx, id.with("steady"), led(state.player.position_now(), state.prefs.lyrics_lead), playing);
    // Each item's motion (its place, focus, wait) is kept under its slot,
    // which stays with the same line when the text is swapped for the same
    // song's (timing made, another source), so nothing moves or dims then.
    // A wait new to a swapped text opens from folded instead of popping
    // open; the first frame of the view shows its state as it is.
    let first_frame = ctx.data(|d| d.get_temp::<Slots>(id.with("slots"))).is_none();
    let slot_of = slots(&ctx, id.with("slots"), &item_keys(lines, &items));
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
            ctx.request_repaint_after(Duration::from_secs_f64((j.at + JUMP_HOLD - now).max(0.0) + 0.01));
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
        anim::f32(&ctx, widest_id, widest, BLOCK).clamp(0.0, wrap)
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
            y -= gap * 0.6;
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
            // close again) in one soft move.
            DisplayLine::Interlude { .. } => anim::fraction(
                &ctx,
                id.with(("wait-room", slot_of[i])),
                if cur == Some(i) { 1.0 } else { 0.0 },
                if theme.reduced_motion { WAIT_ROOM.without_bounce() } else { WAIT_ROOM },
            )
            .clamp(0.0, 1.0),
            DisplayLine::Line(_) => 1.0,
        };
        y += (h + extra + gap) * keep;
        rooms.push(keep);
    }

    // Where the current line rests: at its share of the whole view, with the
    // soft edges always clear of the top band (a lens: lines come into
    // focus as they arrive and go soft as they leave).
    let soft = Soft::of(&state.prefs);
    let anchor = rest_at(rect, sizes.anchor, state.prefs.lyrics_soft_edges.then_some(soft), slots[ci].0 * 0.5, sizes.big * 0.4);
    let follow = tops[ci] + slots[ci].0 * 0.5 - anchor;

    // Keep the user's offset, then ease it back over half a second.
    let manual_id = id.with("manual");
    let mut dy = if resp.dragged() { ctx.input(|i| i.pointer.delta().y) } else { 0.0 };
    if resp.hovered() {
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
        ctx.request_repaint_after(Duration::from_secs_f64((manual.until - now).max(0.0)));
    } else if manual.offset != 0.0 && now < manual.until + 0.5 {
        ctx.request_repaint();
    }
    let scroll = follow + manual.shift(now);
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
    // text's own moves take the calmer spring.
    let together = if !state.prefs.lyrics_cascade {
        anim::f32(&ctx, id.with("scroll"), scroll, if manual_on { HAND } else { Spring::new(1.0, 300.0) })
    } else { scroll };

    // Where each item is drawn this frame.
    let ys: Vec<f32> = (0..items.len())
        .map(|i| {
            let far = (i as i32 - ci as i32).unsigned_abs().min(8) as f32;
            // A wave: lines further from the current one follow more
            // lazily, without wobbling past their place.
            let spring = if manual_on {
                HAND
            } else {
                Spring::new(0.9, (300.0 - 26.0 * far).max(110.0))
            };
            if state.prefs.lyrics_cascade {
                anim::f32(&ctx, id.with(("y", slot_of[i])), rect.top() + tops[i] - scroll, spring)
            } else {
                rect.top() + tops[i] - together
            }
        })
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
    let clicked = if resp.clicked() { item_at(resp.interact_pointer_pos().or(resp.hover_pos())) } else { None };
    for (i, item) in items.iter().enumerate() {
        let dist = i as i32 - ci as i32;
        let far = dist.unsigned_abs().min(8) as f32;
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
        // Fade out towards the edges: softly, blurring, unless turned off.
        let (edge, blur) = if state.prefs.lyrics_soft_edges {
            let size = sizes.big * if let DisplayLine::Line(j) = *item { scale(&lines[j]) } else { 1.0 };
            soft_edge(y, slot_h + extra, rect, size, soft)
        } else {
            (((y + slot_h - rect.top()) / 72.0).min((rect.bottom() - y) / 72.0).clamp(0.0, 1.0), 0.0)
        };
        let blur_radius = sizes.big * BLUR * soft.strength;
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
            painter.rect_filled(slot.expand2(vec2(10.0, 6.0 * room)), 14, s.on_surface.gamma_multiply(0.06 * edge * room * hover));
        }
        let DisplayLine::Line(j) = *item else {
            if let DisplayLine::Interlude { start, end, .. } = *item {
                let k = sizes.big / PANE.big * state.prefs.lyrics_gap_size.clamp(0.8, 1.5) * room;
                let line = Rect::from_min_size(pos2(side(56.0 * k), y), vec2(56.0 * k, slot_h * room));
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
                waiting(&clipped, line, k, start, end, pos, appear, playing, bass, edge, &state.prefs);
            }
            continue;
        };
        let l = &lines[j];
        let size = (sizes.small + (sizes.big - sizes.small) * focus) * scale(l);
        // Between the two resting sizes the size moves every frame.
        let g = if focus > 0.0 && focus < 1.0 {
            moving_layout(&painter, &breaks[j].forced, size)
        } else {
            fixed_layout(&painter, &breaks[j].forced, size)
        };
        let line = Rect::from_min_size(pos2(side(g.size().x), y + (slot_h - g.size().y) * 0.5), g.size());
        // Brightness of a line at rest: past lines a bit brighter than the
        // ones to come; focus mode pushes the rest far back (from the look
        // before as the look changes).
        let rest_in = |style: Style| match style {
            Style::Focus => 0.62f32.powf(far).max(0.08) * 0.7,
            _ => (if dist < 0 { 0.5 } else { 0.42 }) * 0.86f32.powf((far - 1.0).max(0.0)),
        };
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
            soft_galley(&painter, pos2(side(sg.size().x), y + slot_h + 4.0), &sg, c, blur, blur_radius * sizes.sub / sizes.big);
        }
        if !is_cur && focus < 0.01 {
            soft_galley(&painter, line.min, &g, dim, blur, blur_radius);
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
        let Some(timing) = Timing::of(timed, &g, look) else {
            if state.prefs.lyrics_glow {
                glow(&painter, &g, line, f32::INFINITY, 0.0, lit, focus * 0.1);
            }
            painter.galley_with_override_text_color(line.min, g, crate::lerp_color(dim, lit, focus));
            continue;
        };
        // The not-yet-sung part eases with focus from the line's resting
        // brightness to a darker tone (for contrast with the sung part): a
        // fixed tone here made a line dip darker the moment it became
        // current, and jump back when it stopped being so.
        let base = s.on_surface.gamma_multiply(edge * (rest + (rest.min(UNSUNG) - rest) * focus));
        let color = crate::lerp_color(dim, lit, focus.max(shine));
        let pace = Pace::of(&state.prefs);
        let (feather, window) = (pace.feather(size), pace.window());
        let draw = |painter: &egui::Painter, look: Look, timing: &Timing| {
            // Words only change colour (their jump stays down), so they
            // still light up one by one.
            if theme.reduced_motion && !look.words() {
                if state.prefs.lyrics_glow {
                    let (head, feather) = if look.style == Style::Karaoke { timing.glide(sung, feather, window) } else { (f32::INFINITY, 0.0) };
                    glow(painter, &g, line, head, feather, lit, focus * 0.1);
                }
                if look.style == Style::Karaoke {
                    let (head, feather) = timing.glide(sung, feather, window);
                    karaoke(painter, &g, line, head, feather, base, color);
                } else {
                    painter.galley_with_override_text_color(line.min, g.clone(), color);
                }
            } else {
                timed_line(painter, &g, line, timing, sung, is_cur, focus, size, look, base, color, &state.prefs);
            }
        };
        match look_was.and_then(|was| Some((was, Timing::of(timed, &g, was)?))) {
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
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if playing {
        ctx.request_repaint();
    }
}

/// The spring the text follows a scroll by hand on.
const HAND: Spring = Spring::new(1.0, 1600.0);

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

/// The Gaussian blur (sigma) of a line at the very edge, relative to the
/// big text size (at «Сила размытия» 100 %).
const BLUR: f32 = 0.12;

/// The soft edges' settings: how deep each band reaches (a share of the
/// view's height, «Глубина»), how much a line at the edge is blurred
/// («Сила размытия») and how early it fades («Затухание»).
#[derive(Clone, Copy, Debug)]
struct Soft {
    depth: f32,
    strength: f32,
    fade: f32,
}

impl Soft {
    fn of(p: &Prefs) -> Self {
        Self {
            depth: p.lyrics_soft_depth.clamp(0.08, 0.4),
            strength: p.lyrics_soft_strength.clamp(0.0, 1.5),
            fade: p.lyrics_soft_fade.clamp(0.0, 1.0),
        }
    }

    /// How deep the band at each edge of `rect` reaches.
    fn band(self, rect: Rect) -> f32 {
        (rect.height() * self.depth).clamp(40.0, 360.0).min(rect.height() * 0.4)
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

/// The soft edges for an item `height` tall at `top`: (opacity, blur 0..1).
/// Over the band its blur grows from nothing, slowly at first, to full at
/// the edge, while it stays visible through most of the band, thinning
/// faster only near the edge (a gentler fade keeps it bright until nearer
/// the edge), so the band shows blurred lines instead of empty room; it
/// is gone before its ink (accents and descenders, `size` × 0.4 around
/// it) reaches the edge, so the view's clip never cuts a visible line.
fn soft_edge(top: f32, height: f32, rect: Rect, size: f32, soft: Soft) -> (f32, f32) {
    let band = soft.band(rect);
    let ink = size * 0.4;
    let room = (top - ink - rect.top()).min(rect.bottom() - (top + height + ink));
    let f = (room / band.max(1.0)).clamp(0.0, 1.0);
    let d = 1.0 - f;
    (1.0 - d.powf(3.0 - 1.8 * soft.fade), d * d * (3.0 - 2.0 * d))
}

/// `g` at `pos` in `color`, blurred by `blur` (0..1) of `radius` points
/// of Gaussian sigma ([`super::lyrics_blur`]): one draw, sharp or not.
fn soft_galley(painter: &egui::Painter, pos: egui::Pos2, g: &Arc<Galley>, color: Color32, blur: f32, radius: f32) {
    super::lyrics_blur::galley(painter, pos, g, color, radius * blur);
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
/// by place, as the items do.
fn slots(ctx: &egui::Context, id: egui::Id, keys: &[u64]) -> Vec<u64> {
    let was = ctx.data(|d| d.get_temp::<Slots>(id)).unwrap_or_default();
    if was.keys == keys {
        return was.slots;
    }
    let now = carry_slots(&was, keys);
    let out = now.slots.clone();
    ctx.data_mut(|d| d.insert_temp(id, now));
    out
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
fn fixed_breaks(painter: &egui::Painter, id: egui::Id, text: &str, wrap: f32, big: f32) -> Breaks {
    let ctx = painter.ctx();
    if let Some(b) = ctx.data(|d| d.get_temp::<Breaks>(id))
        && b.text == text && b.wrap == wrap && b.big == big
    {
        return b;
    }
    let mut job = LayoutJob::single_section(text.to_owned(), style_at(big).text_format(Color32::WHITE));
    job.wrap.max_width = wrap;
    let g = crate::typography::layout(painter, job);
    let forced = g.rows.iter().map(|r| r.row.glyphs.iter().map(|gl| gl.chr).collect::<String>()).collect::<Vec<_>>().join("\n");
    let size = fixed_layout(painter, &forced, big).size();
    let b = Breaks { text: text.to_owned(), wrap, big, forced, height: size.y, width: size.x };
    ctx.data_mut(|d| d.insert_temp(id, b.clone()));
    b
}

fn fixed_layout(painter: &egui::Painter, text: &str, size: f32) -> Arc<Galley> {
    crate::typography::layout(painter, LayoutJob::single_section(text.to_owned(), style_at(size).text_format(Color32::WHITE)))
}

/// [`fixed_layout`] for a size that is animating (see
/// [`crate::typography::layout_sized`]).
fn moving_layout(painter: &egui::Painter, text: &str, size: f32) -> Arc<Galley> {
    crate::typography::layout_sized(painter, style_at(size), |style, _| {
        LayoutJob::single_section(text.to_owned(), style.text_format(Color32::WHITE))
    })
}

/// A looping sample for the settings' hover card: the line being sung
/// between two resting ones, painted by the real line painter under `prefs`
/// (animation, glow, dimming, size, letter jump).
pub(crate) fn preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let look = Look::of(prefs);
    let scale = prefs.lyrics_scale.clamp(0.8, 1.4);
    let (mut big, mut small) = (PANE.big * 0.8 * scale, PANE.small * 0.8 * scale);
    // Sung by ~2.9 s, held lit, then eased back to unsung before the loop
    // starts over (a snap back read as a glitch).
    const LOOP: f64 = 3.7;
    const BACK: f32 = 3.2;
    const PAD: f32 = 10.0;
    const TEXTS: [&str; 3] = ["Где-то рядом", "Слышишь этот ритм", "Он ведёт меня"];
    let pos = Duration::from_secs_f64(anim::now(ui.ctx()).rem_euclid(LOOP));
    let mut at = 0.3;
    let words = ["Слы", "шишь ", "э", "тот ", "ритм"]
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
    // Fit the three lines into the sample (a small tile shrinks the text;
    // the size setting still shows, relative to the others): the widest
    // line with padding, and the three rows with the wave's jump above.
    let measure = |big: f32, small: f32| {
        let sizes = [small, big, small];
        let w = TEXTS.iter().zip(sizes).map(|(t, z)| fixed_layout(&painter, t, z).size().x).fold(0.0, f32::max);
        let h: f32 = TEXTS.iter().zip(sizes).map(|(t, z)| fixed_layout(&painter, t, z).size().y).sum();
        (w, h + 2.0 * 6.0 + big * 0.25)
    };
    // Fitted at the largest size, so the size setting still shows.
    let (w, h) = measure(big / scale * 1.4, small / scale * 1.4);
    let fit = ((rect.width() - 2.0 * PAD) / w).min((rect.height() - 2.0 * 6.0) / h).min(1.0);
    if fit < 1.0 {
        big *= fit;
        small *= fit;
    }
    let gap = 6.0 * fit.min(1.0);
    let g = fixed_layout(&painter, &line.text, big);
    let cur = Rect::from_min_size(pos2(rect.left() + PAD, rect.center().y - g.size().y * 0.5 + big * 0.1), g.size());
    for (text, y) in [(TEXTS[0], cur.top() - gap), (TEXTS[2], cur.bottom() + gap)] {
        let sg = fixed_layout(&painter, text, small);
        let top = if y < cur.top() { y - sg.size().y } else { y };
        painter.galley_with_override_text_color(pos2(cur.left(), top), sg, dim);
    }
    let base = s.on_surface.gamma_multiply(rest.min(UNSUNG));
    if let Some(timing) = Timing::of(&line, &g, look) {
        // Past the end the line is drawn as no longer current, its light
        // (and the wave's lift) easing out with `focus`.
        let back = ((pos.as_secs_f32() - BACK) / 0.4).clamp(0.0, 1.0);
        let back = back * back * (3.0 - 2.0 * back);
        let (current, focus) = if back > 0.0 { (false, 1.0 - back) } else { (true, 1.0) };
        let color = crate::lerp_color(base, lit, focus);
        timed_line(&painter, &g, cur, &timing, pos, current, focus, big, look, base, color, prefs);
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
/// sample as the lyrics scroll, blurring and fading at its top and bottom
/// (when off, cut by the card as before).
pub(crate) fn edges_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    const TEXTS: [&str; 6] = ["Где-то рядом", "Слышишь этот ритм", "Он ведёт меня", "Сквозь огни домов", "До самого утра", "И дальше вперёд"];
    // A line every 1.6 s, each move eased out over half a second.
    const STEP: f64 = 1.6;
    let size = (rect.height() / 6.0).clamp(12.0, 20.0);
    let pitch = size * 1.8;
    let t = anim::now(ui.ctx()) / STEP;
    let n = t.floor() as i64;
    let x = ((t.fract() * STEP) / 0.5).min(1.0) as f32;
    let ease = 1.0 - (1.0 - x).powi(3);
    let soft = Soft::of(prefs);
    for k in -4i64..=5 {
        let g = fixed_layout(&painter, TEXTS[(n + k).rem_euclid(TEXTS.len() as i64) as usize], size);
        let top = rect.center().y + (k as f32 + 1.0 - ease) * pitch - g.size().y * 0.5;
        let (alpha, blur) = if prefs.lyrics_soft_edges {
            soft_edge(top, g.size().y, rect, size, soft)
        } else {
            let band = rect.height() * 0.12;
            (((top + g.size().y - rect.top()) / band).min((rect.bottom() - top) / band).clamp(0.0, 1.0), 0.0)
        };
        let rest = if k == 0 { 1.0 } else { 0.45 };
        soft_galley(&painter, pos2(rect.left() + 12.0, top), &g, s.on_surface.gamma_multiply(rest * alpha), blur, size * BLUR * soft.strength);
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
    letter_times: Vec<Option<(f32, f32)>>,
    /// When each letter lights and jumps (spaces have none): its own
    /// start, but never before a letter written ahead of it.
    onsets: Vec<Option<f32>>,
    /// Each glyph's span on the line's run of advances.
    xs: Vec<(f32, f32)>,
    /// First syllable of each whitespace-delimited word.
    words: Vec<usize>,
}

impl Timing {
    /// The timing of `l` laid out as `g`, its letters' times by weight.
    #[cfg(test)]
    fn new(l: &LyricLine, g: &Galley) -> Option<Self> {
        Self::build(l, g, false)
    }

    /// [`Self::new`] for `look`: by syllables each piece fills evenly
    /// across its letters over its own time (the fill of a word runs piece
    /// by piece, each at its own pace).
    fn of(l: &LyricLine, g: &Galley, look: Look) -> Option<Self> {
        Self::build(l, g, look.by == By::Syllables)
    }

    fn build(l: &LyricLine, g: &Galley, even: bool) -> Option<Self> {
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
        let units = spans
            .into_iter()
            .map(|u| {
                last = u.unwrap_or(last);
                latest = latest.max(last.start);
                Unit { start: latest, end: last.end.max(latest) }
            })
            .collect();
        let mut latest = f32::NEG_INFINITY;
        let onsets = times.iter().map(|t| t.map(|(a, _)| {
            latest = latest.max(a);
            latest
        })).collect();
        Some(Self { syllables, glyphs, units, glyph_units, letter_times: times, onsets, xs, words })
    }

    /// The light front as (centre of its soft edge, edge width): solid up
    /// to the sung position, the edge just ahead of it. No estimated pace or
    /// movement between timed pieces, but the edge keeps its width at piece
    /// boundaries and in the pauses between them (a hard cut mid-word reads
    /// as a glitch); it only fades in as the line starts and out once the
    /// last piece is sung, so the front never jumps.
    #[cfg(test)]
    fn head(&self, pos: Duration, feather: f32) -> (f32, f32) {
        self.glide(pos, feather, 0.0)
    }

    /// The light front smoothed over `window` seconds («Плавность»): the sung
    /// position averaged over the window around `pos`, so the front glides
    /// through letters, pieces and short pauses at an even pace instead of
    /// changing speed at each letter. On average it keeps to the times, it
    /// never runs back, and it fades in and out half a window wider.
    fn glide(&self, pos: Duration, feather: f32, window: f32) -> (f32, f32) {
        const FADE: f32 = 0.15;
        const SAMPLES: usize = 17;
        let p = pos.as_secs_f32();
        let (Some(first), Some(last)) = (self.syllables.first(), self.syllables.last()) else {
            return (f32::INFINITY, 0.0);
        };
        let window = window.max(0.0);
        let half = window * 0.5;
        let out = 1.0 - ((p - half - last.end) / FADE).clamp(0.0, 1.0);
        if out <= 0.0 {
            return (f32::INFINITY, 0.0);
        }
        let at = if window <= 0.0 {
            self.sung_at(p)
        } else {
            (0..SAMPLES).map(|i| self.sung_at(p - half + window * i as f32 / (SAMPLES - 1) as f32)).sum::<f32>()
                / SAMPLES as f32
        };
        let f = feather * ((p + half - first.start) / FADE).clamp(0.0, 1.0).min(out);
        (at + 0.5 * f, f)
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
            if sy.letters == 0 { sy.start } else { self.onsets.get(sy.glyphs.0).copied().flatten().unwrap_or(sy.start) }
        };
        let height = (first..end).map(|i| jump_over(p - onset(i), speed, window)).sum::<f32>() / (end - first) as f32;
        (height, p - onset(end - 1))
    }

    /// How far along the line's advances it is sung at `p` seconds.
    fn sung_at(&self, p: f32) -> f32 {
        let mut at = 0.0;
        for s in &self.syllables {
            if p < s.start {
                break;
            }
            if p < s.end {
                // Over the letters at their weighted pace.
                at = s.from;
                for k in s.glyphs.0..s.glyphs.1 {
                    let (Some((a, b)), (x0, x1)) = (self.letter_times[k], self.xs[k]) else { continue };
                    if p < b {
                        at = syllable_head(x0, x1, a, b, p);
                        break;
                    }
                    at = x1;
                }
                break;
            }
            at = s.to;
        }
        at
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
    /// «Плавность»: 0 by the letters exactly .. 1 smooth.
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

    /// Seconds a letter or word takes to light up (90 % of the way), 0 at
    /// once; focus brightens [`FOCUS_CALM`] times slower.
    fn rise(self, style: Style) -> f32 {
        let rise = 0.5 * (1.0 - self.light) * (1.0 - self.light);
        if style == Style::Focus { rise * FOCUS_CALM } else { rise }
    }

    /// The karaoke front's soft edge at text `size`: how long a letter it
    /// crosses takes to light up.
    fn feather(self, size: f32) -> f32 {
        size * (0.06 + 0.8 * (1.0 - self.light))
    }
}

/// Focus brightens words this many times slower than the others light them.
const FOCUS_CALM: f32 = 2.2;

/// The averaging window at full «Плавность», in seconds: two or three sung
/// letters.
const SMOOTH_WINDOW: f32 = 0.4;

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

fn syllable_head(from: f32, to: f32, start: f32, end: f32, pos: f32) -> f32 {
    let progress = if end <= start { f32::from(pos >= start) } else { ((pos - start) / (end - start)).clamp(0.0, 1.0) };
    from + (to - from) * progress
}

/// How long a held note's extra rise takes to settle after it ends.
const SETTLE: f32 = 0.3;

/// A syllable sung out long enough to get the held-note rise and glow; it
/// stays "held" for [`SETTLE`] after its end so the letters come down smoothly.
fn held(start: f32, end: f32, letters: usize, pos: f32) -> bool {
    end - start >= 1.0 && end - start >= 0.25 * letters as f32 && start <= pos && pos < end + SETTLE
}

/// 1 while the note is held, then down to 0 over [`SETTLE`].
fn held_envelope(end: f32, pos: f32) -> f32 {
    let t = (1.0 - (pos - end) / SETTLE).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[allow(clippy::too_many_arguments)]
/// `pos` is where the line is sung up to: the position while it is
/// current, and after that on as it fades ([`sung_pos`]), so nothing in it
/// jumps when it stops being current; it only goes back with `focus`.
fn timed_line(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, timing: &Timing, pos: Duration,
    current: bool, focus: f32, size: f32, look: Look, base: Color32, lit: Color32, prefs: &Prefs)
{
    let style = look.style;
    let pace = Pace::of(prefs);
    let rise = pace.rise(style);
    let window = pace.window();
    let (head, feather) = timing.glide(pos, pace.feather(size), window);
    let p = pos.as_secs_f32();
    let theme = Theme::get(painter.ctx());
    let glow_color = crate::lerp_color(lit, theme.scheme.primary, 0.3);
    let ppp = painter.pixels_per_point();
    // Sung pieces are lit (and wave pieces up) in full on the current line,
    // and go back with its focus once it is over.
    let hold = focus.max(if current { 1.0 } else { 0.0 });
    let n = timing.glyphs.len();
    let mut inks = Vec::with_capacity(n);
    let mut glows = Vec::with_capacity(n);
    for (k, glyph) in timing.glyphs.iter().enumerate() {
        let syllable = glyph.map(|s| &timing.syllables[s]);
        // By words: the written unit this glyph lights (and jumps) with.
        let unit = timing.glyph_units[k].filter(|_| look.words());
        // A note held long glows while it is held: under karaoke, and in
        // every look by syllables.
        let sustained = syllable.filter(|s| current && (style == Style::Karaoke || look.by == By::Syllables) && held(s.start, s.end, s.letters, p));
        // When the piece this glyph goes with is sung: its unit by words,
        // its own letter in the wave (and in focus by syllables, which
        // fills each syllable across its letters). Karaoke by letters or
        // syllables follows the front instead; focus by letters lights the
        // whole line.
        let onset = if look.words() {
            unit.map(|u| timing.units[u].start)
        } else if style == Style::Wave || (style == Style::Focus && look.by == By::Syllables) {
            syllable.map(|s| timing.onsets[k].unwrap_or(s.start))
        } else {
            None
        };
        let on = onset.map(|at| light_over(p - at, rise, window));
        // Karaoke and Focus keep one baseline: sung and unsung words never
        // drift apart (that read as broken text). Reduced motion keeps the
        // wave down too (it only lights up).
        let lift = match onset {
            Some(at) if style == Style::Wave && !theme.reduced_motion => {
                // The jump's speed scales its smoothing window too.
                let spread = window / pace.jump;
                // By syllables the whole written word goes up with them.
                let (height, t) = match glyph.filter(|_| look.by == By::Syllables) {
                    Some(s) => timing.word_jump(s, p, pace.jump, spread),
                    None => (jump_over(p - at, pace.jump, spread), p - at),
                };
                let lift = height * RISE * size * prefs.lyrics_wave_jump.max(0.0) * hold;
                // At rest on whole pixels, so a raised word is as crisp as
                // the rest; in motion to the fraction, so it never steps.
                // It eases between the two, so it never steps onto or off
                // the pixel either.
                let crisp = crisp_lift(t + ONSET_LEAD - (JUMP_SETTLE / pace.jump + spread), hold);
                lift + ((lift * ppp).round() / ppp - lift) * crisp
            }
            _ => 0.0,
        };
        inks.push(match on {
            Some(on) => Ink::solid(crate::lerp_color(base, lit, on * hold), lift),
            None if style == Style::Karaoke && !look.words() => Ink { lift, base, lit, head, feather },
            None => Ink::solid(lit, lift),
        });
        let strength = focus * (0.1 + sustained.map_or(0.0, |s|
            0.06 * ((p - s.start) / 0.3).clamp(0.0, 1.0) * held_envelope(s.end, p)));
        let glow_ink = |strength: f32| glow_color.gamma_multiply(strength.clamp(0.0, 1.0));
        glows.push(match (on, style) {
            (Some(on), _) => Ink::solid(glow_ink(strength * on), lift),
            (None, Style::Karaoke) if !look.words() => Ink { lift, base: Color32::TRANSPARENT, lit: glow_ink(strength), head, feather },
            (None, Style::Focus) if !look.words() => Ink::solid(glow_ink(strength), lift),
            _ => Ink::solid(Color32::TRANSPARENT, lift),
        });
    }
    // Every glyph is drawn once, whole, in its own place and colour: no clip
    // edge between pieces, so no seam, cut overhang or second copy shows.
    if prefs.lyrics_glow && glows.iter().any(|i| i.lit.a() > 0) {
        let g = inked(g, |k| glows[k]);
        for offset in GLOW {
            painter.galley(line.min + offset, g.clone(), glow_color);
        }
    }
    painter.galley(line.min, inked(g, |k| inks[k]), lit);
}

/// Where the glow's copies of a line go.
const GLOW: [egui::Vec2; 3] = [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0)];

#[allow(clippy::too_many_arguments)]
fn glow(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, head: f32, feather: f32, color: Color32, strength: f32) {
    if strength <= 0.001 { return; }
    let c = color.gamma_multiply(strength);
    if head.is_infinite() {
        for offset in GLOW {
            painter.galley_with_override_text_color(line.min + offset, g.clone(), c);
        }
        return;
    }
    let g = inked(g, |_| Ink { lift: 0.0, base: Color32::TRANSPARENT, lit: c, head, feather });
    for offset in GLOW {
        painter.galley(line.min + offset, g.clone(), c);
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
/// to `base` at `head` (a hard edge when `feather` is 0).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ink {
    lift: f32,
    base: Color32,
    lit: Color32,
    head: f32,
    feather: f32,
}

impl Ink {
    fn solid(color: Color32, lift: f32) -> Self {
        Ink { lift, base: color, lit: color, head: f32::INFINITY, feather: 0.0 }
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
        egui::epaint::Vertex {
            pos: a.pos.lerp(b.pos, t) - vec2(0.0, ink.lift),
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
fn karaoke(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, head: f32, feather: f32, base: Color32, lit: Color32) {
    if head.is_infinite() {
        painter.galley_with_override_text_color(line.min, g.clone(), lit);
        return;
    }
    painter.galley(line.min, inked(g, |_| Ink { lift: 0.0, base, lit, head, feather }), lit);
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
    let requested_amp = if prefs.lyrics_gap_style % 2 == 1 {
        ramp * k * shrink * height * (1.4 + 0.5 * if motion && playing { bass.clamp(0.0, 1.0) } else { 0.0 })
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
    let feather = 1.0 / painter.pixels_per_point();
    let points = |from: f32, to: f32, amp: f32| -> Vec<egui::Pos2> {
        let n = ((to - from) * radius / (k * shrink * 0.6)).ceil().max(2.0) as usize;
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                // Anchored at the head and running back from it, like the
                // linear wavy progress: the wave comes out of the tip.
                let r = radius + amp * (waves * (sweep - a) - phase).sin();
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

    /// The speed settings: defaults light a word in about 60 ms and karaoke
    /// keeps its old soft edge; the extremes are at once and slow.
    #[test]
    fn the_light_speed_runs_from_smooth_to_at_once() {
        let p = Prefs::default();
        assert!((p.lyrics_jump_speed - 1.0).abs() < 1e-6);
        let pace = Pace::of(&p);
        assert!(pace.rise(Style::Wave) > 0.04 && pace.rise(Style::Wave) < 0.08);
        assert!(pace.rise(Style::Focus) > pace.rise(Style::Karaoke));
        assert!((pace.feather(26.0) - 26.0 * 0.34).abs() < 0.5);
        let at_once = Pace::of(&Prefs { lyrics_light_speed: 1.0, ..p.clone() });
        assert_eq!(at_once.rise(Style::Karaoke), 0.0);
        assert_eq!(light_up(-ONSET_LEAD + 0.001, at_once.rise(Style::Wave)), 1.0);
        let smooth = Pace::of(&Prefs { lyrics_light_speed: 0.0, ..p.clone() });
        assert!(smooth.rise(Style::Wave) >= 0.5 && smooth.feather(26.0) > pace.feather(26.0));
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
                    let t = Timing::of(&l, &g, look).unwrap();
                    let prefs = Prefs { lyrics_glow: true, ..Prefs::default() };
                    for (at, current, focus) in [(0.5, true, 1.0), (2.5, true, 1.0), (2.9, false, 0.4), (5.0, false, 0.0)] {
                        timed_line(painter, &g, rect, &t, Duration::from_secs_f32(at), current, focus, 26.0,
                            look, Color32::GRAY, Color32::WHITE, &prefs);
                    }
                }
            }
        });
    }

    /// Soft edges: a line deep inside is sharp and opaque; nearer an edge it
    /// blurs and thins, gently at first, and it is gone before its ink
    /// reaches the edge.
    #[test]
    fn soft_edges_blur_and_fade_before_the_edge() {
        let rect = Rect::from_min_size(pos2(0.0, 100.0), vec2(600.0, 500.0));
        let size = 26.0;
        let soft = Soft::of(&Prefs::default());
        let at = |top: f32| soft_edge(top, 34.0, rect, size, soft);
        assert_eq!(at(350.0), (1.0, 0.0));
        let (alpha, blur) = at(rect.top() + size * 0.4 + 40.0);
        assert!(alpha > 0.0 && alpha < 1.0 && blur > 0.0 && blur < 1.0, "{alpha} {blur}");
        // Touching the edge with its ink: invisible, both at the top and bottom.
        assert_eq!(at(rect.top() + size * 0.4).0, 0.0);
        assert_eq!(at(rect.bottom() - 34.0 - size * 0.4).0, 0.0);
        assert_eq!(at(rect.top() - 10.0), (0.0, 1.0));
        // Opacity falls and blur grows steadily towards the edge, with no
        // step at the band's inner end.
        let mut last = (1.0, 0.0);
        for top in (110..350).rev() {
            let (a, b) = at(top as f32);
            assert!(a <= last.0 + 1e-6 && b >= last.1 - 1e-6, "{top}");
            assert!(last.0 - a < 0.05 && b - last.1 < 0.05, "smooth at {top}");
            last = (a, b);
        }
        // A deeper band reaches further in; a gentler fade keeps a line
        // brighter at the same place.
        let near = rect.top() + size * 0.4 + 90.0;
        let deep = Soft { depth: 0.3, ..soft };
        assert!(soft_edge(near, 34.0, rect, size, deep).0 < soft_edge(near, 34.0, rect, size, Soft { depth: 0.1, ..soft }).0);
        let mid = rect.top() + size * 0.4 + soft.band(rect) * 0.5;
        assert!(soft_edge(mid, 34.0, rect, size, Soft { fade: 0.0, ..soft }).0 > soft_edge(mid, 34.0, rect, size, Soft { fade: 1.0, ..soft }).0);
        // A short view keeps its middle sharp.
        let small = Rect::from_min_size(pos2(0.0, 0.0), vec2(300.0, 160.0));
        assert_eq!(soft_edge(60.0, 30.0, small, 20.0, Soft { depth: 0.1, ..soft }), (1.0, 0.0));
        // The band shows blurred lines, not empty room: a quarter of the
        // way in, a line is still clearly there.
        let quarter = rect.top() + size * 0.4 + soft.band(rect) * 0.25;
        assert!(at(quarter).0 > 0.4 && at(quarter).1 > 0.5, "{:?}", at(quarter));
        // The current line rests at 40 % of the whole view, clear of the
        // top band even when the band is deep, never below the middle.
        let (half, ink) = (17.0, size * 0.4);
        assert_eq!(rest_at(rect, 0.4, None, half, ink), 200.0);
        assert_eq!(rest_at(rect, 0.4, Some(soft), half, ink), 200.0);
        let deepest = Soft { depth: 0.4, ..soft };
        let rest = rest_at(rect, 0.4, Some(deepest), half, ink);
        assert!(rest > 200.0 && rest <= 250.0);
        assert!(soft_edge(rect.top() + rest - half, half * 2.0, rect, size, deepest).0 > 0.999);
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

    fn three_rows(painter: &egui::Painter, text: &str) -> Breaks {
        (200..900).step_by(10).find_map(|width| {
            let b = fixed_breaks(painter, egui::Id::new("test-breaks"), text, width as f32, 44.0);
            (b.forced.split('\n').count() == 3).then_some(b)
        }).expect("fixture must wrap into three rows")
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
                karaoke(painter, &g, line, 80.0, Pace::of(&Prefs::default()).feather(size), Color32::GRAY, Color32::WHITE);
                timed_line(painter, &g, line, &timing, Duration::from_secs(4), true, 1.0, size, Look { style: Style::Karaoke, by: By::Letters }, Color32::GRAY, Color32::WHITE, &Prefs::default());
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
            // The karaoke front crosses the «д» by its share, not by a fifth.
            let at = |s: f32| t.head(Duration::from_secs_f32(s), 0.0).0;
            let x = |i: usize| t.xs[i];
            let d = 6; // «д» after «здесь» and the space
            assert!((at(2.0 + 0.5 / 8.0) - (x(d).0 + x(d).1) / 2.0).abs() < 0.5);
            assert!((at(2.0 + 1.0 / 8.0) - x(d).1).abs() < 0.5);
            assert!((at(2.0 + 2.25 / 8.0) - (x(d + 1).0 + x(d + 1).1) / 2.0).abs() < 0.5);
            let mut was = 0.0;
            for ms in 1000..3000 {
                let h = at(ms as f32 / 1000.0);
                if h.is_finite() {
                    assert!(h >= was - 1e-3, "{ms}");
                    was = h;
                }
            }
        });
    }

    #[test]
    fn syllable_head_is_continuous_and_clamped_to_its_own_span() {
        assert_eq!(syllable_head(20.0, 120.0, 2.0, 4.0, 1.0), 20.0);
        assert_eq!(syllable_head(20.0, 120.0, 2.0, 4.0, 3.0), 70.0);
        assert_eq!(syllable_head(20.0, 120.0, 2.0, 4.0, 5.0), 120.0);
        assert_eq!(syllable_head(20.0, 120.0, 2.0, 2.0, 2.0), 120.0);
    }

    #[test]
    fn glued_syllables_share_a_word_and_the_head_waits_in_gaps() {
        with_painter(|painter| {
            let mut l = line(2.0, Some(8.0), "Hello world");
            l.words = [(2.0, 3.0, "Hel"), (4.0, 6.0, "lo "), (7.0, 8.0, "world")].map(|(s, e, text)| nikgob_view::LyricWord {
                start: Duration::from_secs_f32(s), end: Duration::from_secs_f32(e), text: text.into(),
            }).to_vec();
            let g = fixed_layout(painter, &l.text, 44.0);
            let t = Timing::new(&l, &g).unwrap();
            assert_eq!(t.words, [0, 2]);
            assert_eq!(t.syllables.iter().map(|s| s.word).collect::<Vec<_>>(), [0, 0, 1]);
            // Solid up to the sung position; the soft edge sits ahead of it
            // and keeps its width in the pauses (no hard cut mid-word).
            let solid = |ms| {
                let (head, feather) = t.head(Duration::from_millis(ms), 15.0);
                (head - 0.5 * feather, feather)
            };
            assert_eq!(solid(1000), (0.0, 0.0));
            assert_eq!(solid(3500), (t.syllables[0].to, 15.0));
            assert_eq!(solid(6500), (t.syllables[1].to, 15.0));
            let (at, feather) = solid(5000);
            // Halfway through «lo»'s time: «l» takes 1 share, «o» 2.5, so
            // the front is 0.3 into the «o».
            let o = t.xs[t.syllables[1].glyphs.0 + 1];
            assert!((at - (o.0 + 0.3 * (o.1 - o.0))).abs() < 0.01, "{at} {o:?}");
            assert_eq!(feather, 15.0);
            // It fades in and out instead of popping.
            assert!(solid(2050).1 > 0.0 && solid(2050).1 < 15.0);
            assert!(solid(8050).1 > 0.0 && solid(8050).1 < 15.0);
            assert!(t.head(Duration::from_millis(8200), 15.0).0.is_infinite());
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
            let front = Ink { lift: 0.0, base: Color32::BLACK, lit: Color32::WHITE, head: 70.0, feather: 9.0 };
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

    /// A timing slip (a piece timed before one written ahead of it) waits
    /// for that one: letters and words light in the order they are written.
    #[test]
    fn onsets_follow_the_written_order() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "ab cd ef");
            l.words = vec![word(1.0, 1.5, "ab "), word(0.8, 1.2, "cd "), word(2.0, 2.5, "ef")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let onsets: Vec<f32> = t.onsets.iter().flatten().copied().collect();
            assert_eq!(onsets.len(), 6);
            assert!(onsets.windows(2).all(|w| w[0] <= w[1]), "{onsets:?}");
            assert!(close(onsets[2], onsets[1]) && close(onsets[4], 2.0));
            assert!(t.units.windows(2).all(|w| w[0].start <= w[1].start && w[1].start <= w[1].end));
            // In order, they are the letters' own starts.
            let mut l = l;
            l.words[1] = word(1.5, 2.0, "cd ");
            let t = Timing::new(&l, &fixed_layout(painter, &l.text, 26.0)).unwrap();
            let own: Vec<f32> = t.letter_times.iter().flatten().map(|(a, _)| *a).collect();
            assert_eq!(t.onsets.iter().flatten().copied().collect::<Vec<_>>(), own);
        });
    }

    /// «Плавность»: the front averaged over a window still follows the
    /// times (none at 0, the same at the end), never runs back and glides:
    /// no jump between frames, and an even pace through a word's letters.
    #[test]
    fn a_smooth_front_glides_and_keeps_to_the_times() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(4.0), "здесь домой");
            l.words = vec![word(1.0, 1.6, "здесь "), word(2.0, 3.0, "домой")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::new(&l, &g).unwrap();
            let at = |ms: u64, window: f32| t.glide(Duration::from_millis(ms), 8.0, window);
            for ms in (900..3300).step_by(7) {
                assert_eq!(at(ms, 0.0), t.head(Duration::from_millis(ms), 8.0));
            }
            let window = Pace::of(&Prefs::default()).window();
            assert!(window > 0.1 && window < 0.3, "the default is smoother than by letters");
            let (mut was, mut steps, mut exact_steps) = (0.0f32, Vec::new(), Vec::new());
            let mut exact_was = 0.0f32;
            for ms in 1000..3000 {
                let (h, f) = at(ms, window);
                let (e, ef) = at(ms, 0.0);
                if h.is_finite() && e.is_finite() {
                    let (h, e) = (h - 0.5 * f, e - 0.5 * ef);
                    assert!(h >= was - 1e-3, "{ms}: runs back");
                    steps.push(h - was);
                    exact_steps.push(e - exact_was);
                    (was, exact_was) = (h, e);
                }
            }
            // Smoother: its fastest frame-to-frame move is well under the
            // exact front's (which crosses a consonant in a few ms).
            let fastest = |v: &[f32]| v.iter().skip(1).copied().fold(0.0f32, f32::max);
            assert!(fastest(&steps) < fastest(&exact_steps) * 0.6, "{} vs {}", fastest(&steps), fastest(&exact_steps));
            // Done singing: it reaches the end and lights the line whole.
            assert!(at(3000 + (window * 500.0) as u64 + 1, window).0 >= t.syllables[1].to - 1e-3);
            assert!(at(3400, window).0.is_infinite());
            // Before the line: nothing lit.
            assert_eq!(at(800, window), (0.0, 0.0));
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
            let t = Timing::new(&l, &g).unwrap();
            let left = ms(3_000);
            let (at, _) = t.glide(left, 8.0, 0.2);
            let (next, _) = t.glide(sung_pos(Some(left), left + ms(16)), 8.0, 0.2);
            assert!(at.is_finite() && next >= at && next - at < 5.0, "{at} → {next}");
        });
    }

    /// By syllables each piece fills evenly across its letters over its
    /// own time: a fast «ко» and «смо», a held «цве», then «ток».
    #[test]
    fn a_syllable_fills_evenly_over_its_own_time() {
        with_painter(|painter| {
            let mut l = line(1.0, Some(3.0), "космоцветок");
            l.words = vec![word(1.0, 1.1, "ко"), word(1.1, 1.25, "смо"), word(1.25, 2.1, "цве"), word(2.1, 2.3, "ток")];
            let g = fixed_layout(painter, &l.text, 26.0);
            let t = Timing::of(&l, &g, Look { style: Style::Karaoke, by: By::Syllables }).unwrap();
            let s = &t.syllables[2];
            // Halfway through its time the front is halfway across it.
            let mid = t.sung_at((s.start + s.end) * 0.5);
            assert!((mid - (s.from + s.to) * 0.5).abs() < 0.5, "{mid} in {}..{}", s.from, s.to);
            assert!((t.sung_at(s.start) - s.from).abs() < 1e-3 && (t.sung_at(s.end - 1e-4) - s.to).abs() < 0.1);
            // The held one runs slowest.
            let pace = |i: usize| (t.syllables[i].to - t.syllables[i].from) / (t.syllables[i].end - t.syllables[i].start);
            assert!(pace(2) < pace(0) && pace(2) < pace(1));
            // By letters the same piece is weighted (its vowel takes longer).
            let w = Timing::new(&l, &g).unwrap();
            assert!((w.sung_at((s.start + s.end) * 0.5) - mid).abs() > 0.5);
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
            let t = Timing::of(&l, &g, Look { style: Style::Wave, by: By::Syllables }).unwrap();
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
