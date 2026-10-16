//! Honest syllable fill, word lift and sustained notes; line timing only focuses.
//! Both reading scales share frozen wrapping, scrolling and source selection.

use std::sync::Arc;
use std::time::Duration;

use egui::{Align2, Color32, Galley, Rect, Sense, Ui, pos2, text::LayoutJob, vec2};
use nikgob_motion::Spring;
use nikgob_view::{Command, LyricLine, LyricsAvail, LyricsSource, LyricsSync, Prefs, Status, ViewState};

use crate::{
    Icon, Theme, TypeRole, TypeStyle,
    components::{Menu, MenuItem, Size, anim},
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Karaoke,
    Focus,
    /// Letter by letter: each one rises and lights up as it is sung.
    Wave,
}

/// Both views share timing and scrolling; true opens the lyrics settings.
pub fn show(ui: &mut Ui, state: &ViewState, bass: f32, mega: bool, out: &mut Vec<Command>) -> bool {
    let style = match state.prefs.lyrics_style {
        1 => Style::Focus,
        // Letters jumping on a guessed pace looks wrong: optionally only
        // with real syllable timing, else the calm Focus.
        2 if state.prefs.wave_syllables_only
            && LyricsSync::of(&state.lyrics.lines) != Some(LyricsSync::Syllable) =>
        {
            Style::Focus
        }
        2 => Style::Wave,
        _ => Style::Karaoke,
    };
    let source_h = if mega { Size::Xs.height() + ui.spacing().item_spacing.y } else { 0.0 };
    let size = vec2(ui.available_width(), (ui.available_height() - source_h).max(80.0));
    if mega && state.lyrics.lines.is_empty() {
        plain(ui, size, state);
        return source_picker(ui, state, out);
    }
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let mut sizes = if mega { MEGA } else { PANE };
    let scale = state.prefs.lyrics_scale.clamp(0.8, 1.4);
    sizes.big *= scale;
    sizes.small *= scale;
    sizes.sub *= scale;
    sizes.gap *= scale;
    paint(ui, rect, &resp, state, style, bass, sizes, mega, out);
    mega && source_picker(ui, state, out)
}

#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &Ui,
    rect: Rect,
    resp: &egui::Response,
    state: &ViewState,
    style: Style,
    bass: f32,
    sizes: Sizes,
    mega: bool,
    out: &mut Vec<Command>,
) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let lines = &state.lyrics.lines;
    let items = display_lines(lines);
    if items.is_empty() {
        return;
    }
    let painter = ui.painter_at(rect);
    let id = ui.id().with("lyrics-view");
    let now = anim::now(&ctx);
    let pos = state.player.position_now();
    let playing = state.player.status == Status::Playing;

    // Waiting owns the anchor during a break; otherwise follow the last
    // main line. Background voices light up independently while sung.
    let cur = current_item(lines, &items, pos);
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
        painter.layout_job(job)
    };
    let scale = |l: &LyricLine| if l.background { BG_SCALE } else { 1.0 };
    let breaks: Vec<_> = lines.iter().enumerate().map(|(i, l)| {
        fixed_breaks(&painter, id.with(("breaks", i)), &l.text, wrap, sizes.big * scale(l))
    }).collect();
    let slots: Vec<(f32, f32)> = items
        .iter()
        .map(|item| {
            let DisplayLine::Line(i) = *item else {
                let height = if state.prefs.lyrics_gap_style >= 2 { 42.0 } else { 34.0 };
                return (height * sizes.big / PANE.big, 0.0);
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
        widest.min(wrap)
    } else {
        wrap
    };
    let left = rect.left() + 12.0 + (wrap - block) * 0.5;
    let wrap = block;
    let gap = sizes.gap;
    let mut tops = Vec::with_capacity(slots.len());
    let mut y = 0.0;
    for (i, (h, extra)) in slots.iter().enumerate() {
        // Background vocals hug the line they belong to.
        if matches!(items[i], DisplayLine::Line(j) if lines[j].background) && i > 0 {
            y -= gap * 0.6;
        }
        tops.push(y);
        // A wait only takes room while it is happening: it unfolds when it
        // starts and folds away when the singing comes back.
        let keep = match items[i] {
            DisplayLine::Interlude { .. } => anim::f32(
                &ctx,
                id.with(("wait-room", i)),
                if cur == Some(i) { 1.0 } else { 0.0 },
                Spring::new(1.0, 260.0),
            )
            .clamp(0.0, 1.0),
            DisplayLine::Line(_) => 1.0,
        };
        y += (h + extra + gap) * keep;
    }

    // Keep the user's offset, then ease it back over half a second.
    let (mut manual, mut until): (f32, f64) = ctx.data(|d| d.get_temp(id)).unwrap_or((0.0, 0.0));
    let mut dy = if resp.dragged() { ctx.input(|i| i.pointer.delta().y) } else { 0.0 };
    if resp.hovered() {
        dy += ctx.input(|i| i.smooth_scroll_delta.y);
        ctx.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
    }
    if dy != 0.0 {
        manual = returning_offset(manual, until, now) - dy;
        until = now + f64::from(state.prefs.lyrics_return_s.max(0.0));
    }
    // Scrolling stops at the text's ends: the first line or the last one
    // at the anchor, never into empty space.
    let centre = |i: usize| tops[i] + slots[i].0 * 0.5;
    let last = tops.len().saturating_sub(1);
    manual = manual.clamp(centre(0) - centre(ci), centre(last) - centre(ci));
    let manual_on = now < until;
    ctx.data_mut(|d| d.insert_temp(id, (manual, until)));
    if manual_on {
        ctx.request_repaint_after(Duration::from_secs_f64((until - now).max(0.0)));
    } else if manual != 0.0 && now < until + 0.5 {
        ctx.request_repaint();
    }
    let anchor = rect.height() * sizes.anchor;
    let scroll = tops[ci] + slots[ci].0 * 0.5 - anchor + returning_offset(manual, until, now);
    let together = if !state.prefs.lyrics_cascade {
        anim::f32(&ctx, id.with("scroll"), scroll, Spring::new(1.0, 300.0))
    } else { scroll };

    let hover = resp.hover_pos();
    let mut clicked = None;
    for (i, item) in items.iter().enumerate() {
        let dist = i as i32 - ci as i32;
        let far = dist.unsigned_abs().min(8) as f32;
        // A wave: lines further from the current one follow more lazily,
        // without wobbling past their place.
        let spring = if manual_on {
            Spring::new(1.0, 1600.0)
        } else {
            Spring::new(0.9, (300.0 - 26.0 * far).max(110.0))
        };
        let y = if state.prefs.lyrics_cascade {
            anim::f32(&ctx, id.with(("y", i)), rect.top() + tops[i] - scroll, spring)
        } else { rect.top() + tops[i] - together };
        let (slot_h, extra) = slots[i];
        if y + slot_h + extra < rect.top() - 60.0 || y > rect.bottom() + 60.0 {
            continue;
        }
        let is_cur = match *item {
            DisplayLine::Line(j) => active(j),
            DisplayLine::Interlude { .. } => cur == Some(i),
        };
        // 0 → 1 as the line becomes the current one, and back.
        let focus = anim::f32(
            &ctx,
            id.with(("focus", i)),
            if is_cur { 1.0 } else { 0.0 },
            Spring::new(1.0, 170.0),
        )
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
        // Fade out towards the edges.
        let edge = ((y + slot_h - rect.top()) / 72.0).min((rect.bottom() - y) / 72.0).clamp(0.0, 1.0);
        let slot = Rect::from_min_size(pos2(left, y), vec2(wrap, slot_h + extra));
        let hovered = hover.is_some_and(|p| slot.expand2(vec2(8.0, gap * 0.5)).contains(p));
        if hovered {
            painter.rect_filled(slot.expand2(vec2(10.0, 6.0)), 14, s.on_surface.gamma_multiply(0.06 * edge));
            if resp.clicked() {
                clicked = Some(match *item {
                    DisplayLine::Line(j) => lines[j].start,
                    DisplayLine::Interlude { end, .. } => end.saturating_sub(Duration::from_secs(1)),
                });
            }
        }
        let DisplayLine::Line(j) = *item else {
            if let DisplayLine::Interlude { start, end, .. } = *item {
                let k = sizes.big / PANE.big;
                let line = Rect::from_min_size(pos2(side(56.0 * k), y), vec2(56.0 * k, slot_h));
                waiting(&painter, line, k, start, end, pos, is_cur, playing, bass, edge, &state.prefs);
            }
            continue;
        };
        let l = &lines[j];
        let size = (sizes.small + (sizes.big - sizes.small) * focus) * scale(l);
        let g = fixed_layout(&painter, &breaks[j].forced, size);
        let line = Rect::from_min_size(pos2(side(g.size().x), y + (slot_h - g.size().y) * 0.5), g.size());
        // Brightness of a line at rest: past lines a bit brighter than the
        // ones to come; focus mode pushes the rest far back.
        let mut rest = match style {
            Style::Focus => 0.62f32.powf(far).max(0.08) * 0.7,
            _ => (if dist < 0 { 0.5 } else { 0.42 }) * 0.86f32.powf((far - 1.0).max(0.0)),
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
            painter.galley_with_override_text_color(pos2(side(sg.size().x), y + slot_h + 4.0), sg, c);
        }
        if !is_cur && focus < 0.01 {
            painter.galley_with_override_text_color(line.min, g, dim);
            continue;
        }
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
        let Some(timing) = Timing::new(timed, &g) else {
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
        let color = crate::lerp_color(dim, lit, focus);
        if theme.reduced_motion {
            if state.prefs.lyrics_glow {
                let (head, feather) = if style == Style::Karaoke { timing.head(pos, size * 0.35) } else { (f32::INFINITY, 0.0) };
                glow(&painter, &g, line, head, feather, lit, focus * 0.1);
            }
            if style == Style::Karaoke {
                let (head, feather) = timing.head(pos, size * 0.35);
                karaoke(&painter, &g, line, if is_cur { head } else { f32::INFINITY }, feather, base, color);
            } else {
                painter.galley_with_override_text_color(line.min, g, color);
            }
        } else {
            timed_line(&painter, &g, line, &timing, pos, is_cur, focus, size, style, base, color, &state.prefs);
        }
    }
    if let Some(pos) = clicked {
        out.push(Command::Seek(pos));
    }
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if playing {
        ctx.request_repaint();
    }
}

fn returning_offset(offset: f32, until: f64, now: f64) -> f32 {
    let t = ((now - until) / 0.5).clamp(0.0, 1.0) as f32;
    offset * (1.0 - t * t * (3.0 - 2.0 * t))
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
    let g = painter.layout_job(job);
    let forced = g.rows.iter().map(|r| r.row.glyphs.iter().map(|gl| gl.chr).collect::<String>()).collect::<Vec<_>>().join("\n");
    let size = fixed_layout(painter, &forced, big).size();
    let b = Breaks { text: text.to_owned(), wrap, big, forced, height: size.y, width: size.x };
    ctx.data_mut(|d| d.insert_temp(id, b.clone()));
    b
}

fn fixed_layout(painter: &egui::Painter, text: &str, size: f32) -> Arc<Galley> {
    painter.layout_job(LayoutJob::single_section(text.to_owned(), style_at(size).text_format(Color32::WHITE)))
}

/// A looping sample for the settings' hover card: the line being sung
/// between two resting ones, painted by the real line painter under `prefs`
/// (animation, glow, dimming, size, letter jump).
pub(crate) fn preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let style = match prefs.lyrics_style {
        1 => Style::Focus,
        2 => Style::Wave,
        _ => Style::Karaoke,
    };
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
    let words = ["Слы", "шишь ", "этот ", "ритм"]
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
    if let Some(timing) = Timing::new(&line, &g) {
        // Past the end the line is drawn as no longer current, its light
        // (and the wave's lift) easing out with `focus`.
        let back = ((pos.as_secs_f32() - BACK) / 0.4).clamp(0.0, 1.0);
        let back = back * back * (3.0 - 2.0 * back);
        let (current, focus) = if back > 0.0 { (false, 1.0 - back) } else { (true, 1.0) };
        let color = crate::lerp_color(base, lit, focus);
        timed_line(&painter, &g, cur, &timing, pos, current, focus, big, style, base, color, prefs);
    }
}

/// The pause indicator between lines for the settings' cards: a gap on
/// loop, with the countdown at its end.
pub(crate) fn gap_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    // The ring (with its wave and the countdown's pop) fits the sample with
    // ~10 px around it, centred.
    let base = if prefs.lyrics_gap_style >= 2 { 16.0 } else { 13.0 };
    let reach = base + 8.0;
    let k = ((rect.width().min(rect.height()) - 20.0) / (2.0 * reach)).clamp(0.6, 1.4);
    let left = rect.center().x - 2.0 * k - base * k;
    let line = Rect::from_min_size(pos2(left, rect.center().y - 21.0 * k), vec2(56.0 * k, 42.0 * k));
    // A seven-second gap: four of the ring (its style is what is compared),
    // then the countdown when it is on.
    let pos = Duration::from_secs_f64(anim::now(ui.ctx()).rem_euclid(7.6));
    let bass = super::pages::settings_help::beat(anim::now(ui.ctx()) as f32) * 0.5;
    waiting(&painter, line, k, Duration::ZERO, Duration::from_secs(7), pos, true, true, bass, 1.0, prefs);
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
    let k = 0.8;
    let row_h = 42.0 * k;
    let prev = fixed_layout(&painter, "Слышишь этот ритм", small);
    let next = fixed_layout(&painter, "Он ведёт меня", small);
    let total = prev.size().y + row_h + next.size().y + 8.0;
    let top = rect.center().y - total * 0.5;
    painter.galley_with_override_text_color(pos2(rect.left() + PAD, top), prev.clone(), dim);
    let line = Rect::from_min_size(pos2(rect.left() + PAD, top + prev.size().y + 4.0), vec2(56.0 * k, row_h));
    let bass = super::pages::settings_help::beat(anim::now(ui.ctx()) as f32) * 0.5;
    let pos = Duration::from_secs_f32(t);
    waiting(&painter, line, k, Duration::ZERO, Duration::from_secs_f32(GAP), pos, true, true, bass, 1.0, prefs);
    // The next line: dim while waiting, sung once the wait is over.
    let on = ((t - GAP) / 0.18).clamp(0.0, 1.0);
    let color = crate::lerp_color(dim, s.on_surface, on);
    painter.galley_with_override_text_color(pos2(rect.left() + PAD, line.bottom() + 4.0), next, color);
}

/// Unsynced text still gets a readable column, with its source kept outside
/// the scroll area so changing providers never requires scrolling to the end.
fn plain(ui: &mut Ui, size: egui::Vec2, state: &ViewState) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let column = Rect::from_center_size(rect.center(), vec2(rect.width().min(900.0), rect.height()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(column).id_salt("mega-plain"));
    if let Some(text) = state.lyrics.plain.as_deref().filter(|t| !t.trim().is_empty()) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            ui.label(style_at(MEGA.small * state.prefs.lyrics_scale.clamp(0.8, 1.4)).rich_para(text, s.on_surface));
        });
    } else {
        let text = if state.lyrics.loading { "Ищу текст…" } else { "Текста нет" };
        child.painter().text(rect.center(), Align2::CENTER_CENTER, text, style_at(MEGA.small).font_id(), s.on_surface_variant);
    }
}

/// One timed piece spans its own glyph advances, even across frozen rows.
struct Syllable {
    start: f32,
    end: f32,
    from: f32,
    to: f32,
    letters: usize,
    word: usize,
}

struct Timing {
    syllables: Vec<Syllable>,
    /// Syllable index for each glyph (spaces have none).
    glyphs: Vec<Option<usize>>,
    /// First syllable of each whitespace-delimited word.
    #[cfg_attr(not(test), allow(dead_code))]
    words: Vec<usize>,
}

impl Timing {
    fn new(l: &LyricLine, g: &Galley) -> Option<Self> {
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
                from: 0.0, to: 0.0, letters: n, word: words.len() - 1,
            });
            letters.extend(std::iter::repeat_n(i, n));
        }
        let mut letters = letters.into_iter();
        let mut x = 0.0;
        let mut glyphs = Vec::new();
        let mut seen = vec![false; syllables.len()];
        for gl in g.rows.iter().flat_map(|r| &r.row.glyphs) {
            let syllable = if gl.chr.is_whitespace() { None } else { letters.next() };
            if let Some(i) = syllable {
                if !seen[i] {
                    syllables[i].from = x;
                    seen[i] = true;
                }
                syllables[i].to = x + gl.advance_width;
            }
            glyphs.push(syllable);
            x += gl.advance_width;
        }
        Some(Self { syllables, glyphs, words })
    }

    /// The light front as (centre of its soft edge, edge width): solid up
    /// to the sung position, the edge just ahead of it. No estimated pace or
    /// movement between timed pieces, but the edge keeps its width at piece
    /// boundaries and in the pauses between them (a hard cut mid-word reads
    /// as a glitch); it only fades in as the line starts and out once the
    /// last piece is sung, so the front never jumps.
    fn head(&self, pos: Duration, feather: f32) -> (f32, f32) {
        const FADE: f32 = 0.15;
        let p = pos.as_secs_f32();
        let (Some(first), Some(last)) = (self.syllables.first(), self.syllables.last()) else {
            return (f32::INFINITY, 0.0);
        };
        let out = 1.0 - ((p - last.end) / FADE).clamp(0.0, 1.0);
        if out <= 0.0 {
            return (f32::INFINITY, 0.0);
        }
        let mut at = 0.0;
        for s in &self.syllables {
            if p < s.start {
                break;
            }
            if p < s.end {
                at = syllable_head(s.from, s.to, s.start, s.end, p);
                break;
            }
            at = s.to;
        }
        let f = feather * ((p - first.start) / FADE).clamp(0.0, 1.0).min(out);
        (at + 0.5 * f, f)
    }
}

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
fn timed_line(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, timing: &Timing, pos: Duration,
    current: bool, focus: f32, size: f32, style: Style, base: Color32, lit: Color32, prefs: &Prefs)
{
    let (head, feather) = if current { timing.head(pos, size * 0.35) } else { (f32::INFINITY, 0.0) };
    let p = pos.as_secs_f32();
    let glow_color = crate::lerp_color(lit, Theme::get(painter.ctx()).scheme.primary, 0.3);
    let mut k = 0;
    for (row_index, row) in g.rows.iter().enumerate() {
        let glyphs = &row.row.glyphs;
        let mut i = 0;
        while i < glyphs.len() {
            let syllable = timing.glyphs[k + i].map(|s| &timing.syllables[s]);
            let word = syllable.map(|s| s.word);
            let sustained = syllable.filter(|s| current && style == Style::Karaoke
                && held(s.start, s.end, s.letters, p));
            let mut end = i + 1;
            // Ordinary glyphs share one word run; held notes and the wave
            // move each letter on its own.
            if sustained.is_none() && style != Style::Wave {
                while end < glyphs.len() {
                    let next = timing.glyphs[k + end].map(|s| &timing.syllables[s]);
                    if next.map(|s| s.word) != word || next.is_some_and(|s| current && style == Style::Karaoke && held(s.start, s.end, s.letters, p)) { break; }
                    end += 1;
                }
            }
            // Spaces ride with the letter before them: a lifted glyph's
            // overhang (the arm of a "y") must not be left at the baseline.
            if syllable.is_some() {
                while end < glyphs.len() && timing.glyphs[k + end].is_none() {
                    end += 1;
                }
            }
            // Wave: the letter's own share of its syllable's time.
            let letter_on = syllable.filter(|_| style == Style::Wave).map(|s| {
                if !current {
                    return 1.0;
                }
                let letter = timing.glyphs[..k + i].iter().filter(|v| **v == timing.glyphs[k + i]).count();
                let n = s.letters.max(1) as f32;
                let from = s.start + (s.end - s.start) * letter as f32 / n;
                ((p - from + 0.04) / ((s.end - s.start) / n + 0.08).max(0.1)).clamp(0.0, 1.0)
            });
            let lift = if let Some(t) = letter_on {
                let on = t * t * (3.0 - 2.0 * t);
                (on * 0.15 + (std::f32::consts::PI * t).sin() * 0.08) * focus * size * prefs.lyrics_wave_jump.max(0.0)
            } else if style == Style::Wave {
                0.0
            } else {
                // Karaoke and Focus keep one baseline: sung and unsung words
                // never drift apart (that read as broken text).
                0.0
            };
            let shifted = line.translate(vec2(0.0, -lift));
            let r = outer_room(row_index, g.rows.len(), row.rect().translate(shifted.min.to_vec2()), size);
            let x0 = if i == 0 { r.left() - 2.0 } else { shifted.left() + row.pos.x + glyphs[i].pos.x };
            let x1 = if end == glyphs.len() { r.right() + 2.0 } else { shifted.left() + row.pos.x + glyphs[end].pos.x };
            let run = Rect::from_min_max(pos2(x0, r.top()), pos2(x1, r.bottom()));
            let clipped = painter.with_clip_rect(run.intersect(painter.clip_rect()));
            if prefs.lyrics_glow {
                let strength = focus * (0.1 + sustained.map_or(0.0, |s|
                    0.06 * ((p - s.start) / 0.3).clamp(0.0, 1.0) * held_envelope(s.end, p)));
                let (glow_head, glow_feather, strength) = match style {
                    Style::Karaoke => (head, feather, strength),
                    Style::Wave => (f32::INFINITY, 0.0, strength * letter_on.unwrap_or(0.0)),
                    Style::Focus => (f32::INFINITY, 0.0, strength),
                };
                glow(&clipped, g, shifted, glow_head, glow_feather, glow_color, strength);
            }
            if let Some(t) = letter_on {
                let on = t * t * (3.0 - 2.0 * t) * focus.max(if current { 1.0 } else { 0.0 });
                clipped.galley_with_override_text_color(shifted.min, g.clone(), crate::lerp_color(base, lit, on));
            } else if style == Style::Focus || style == Style::Wave {
                clipped.galley_with_override_text_color(shifted.min, g.clone(), lit);
            } else {
                karaoke(&clipped, g, shifted, head, feather, base, lit);
            }
            i = end;
        }
        k += glyphs.len();
    }
}

#[allow(clippy::too_many_arguments)]
fn glow(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, head: f32, feather: f32, color: Color32, strength: f32) {
    if strength <= 0.001 { return; }
    for offset in [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0)] {
        karaoke(painter, g, line.translate(offset), head, feather, Color32::TRANSPARENT, color.gamma_multiply(strength));
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

/// A row's clip area: rows meet edge to edge (so a wrapped line's rows
/// never paint into each other); only the outer edges get room for accents
/// above the first row and descenders below the last one.
fn outer_room(row_index: usize, rows: usize, r: Rect, size: f32) -> Rect {
    let top = if row_index == 0 { size * 0.4 } else { 0.0 };
    let bottom = if row_index + 1 == rows { size * 0.4 } else { 0.0 };
    Rect::from_min_max(pos2(r.left(), r.top() - top), pos2(r.right(), r.bottom() + bottom))
}

/// The line lit left to right (row by row when it wraps) up to `head`
/// pixels of its glyphs (rows flattened), the edge of the light a soft
/// gradient.
fn karaoke(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, head: f32, feather: f32, base: Color32, lit: Color32) {
    if head.is_infinite() {
        painter.galley_with_override_text_color(line.min, g.clone(), lit);
        return;
    }
    painter.galley_with_override_text_color(line.min, g.clone(), base);
    let mut left = head;
    for (row_index, row) in g.rows.iter().enumerate() {
        let r = outer_room(row_index, g.rows.len(), row.rect().translate(line.min.to_vec2()), row.rect().height());
        let w: f32 = row.row.glyphs.iter().map(|gl| gl.advance_width).sum();
        let head = left + feather * 0.5;
        left -= w;
        if head <= 0.0 {
            break;
        }
        // Solid up to the gradient, then slices fading out.
        let solid = (head - feather).clamp(0.0, w);
        if solid > 0.0 {
            let clip = Rect::from_min_max(r.min, pos2(r.left() + solid, r.bottom()));
            painter
                .with_clip_rect(clip.intersect(painter.clip_rect()))
                .galley_with_override_text_color(line.min, g.clone(), lit);
        }
        const SLICES: usize = 8;
        for k in 0..SLICES {
            let x0 = head - feather + feather * k as f32 / SLICES as f32;
            let x1 = head - feather + feather * (k + 1) as f32 / SLICES as f32;
            let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
            if x1 <= x0 {
                continue;
            }
            let a = 1.0 - (k as f32 + 0.5) / SLICES as f32;
            let clip = Rect::from_min_max(pos2(r.left() + x0, r.top()), pos2(r.left() + x1, r.bottom()));
            painter
                .with_clip_rect(clip.intersect(painter.clip_rect()))
                .galley_with_override_text_color(line.min, g.clone(), crate::lerp_color(base, lit, a));
        }
    }
}

/// The wait between lines: a determinate circular wavy progress indicator
/// (Material 3 Expressive). The wavy arc grows clockwise from the top as the
/// gap passes, so it reads as "how much is left"; the rest of the circle is
/// the flat track. While playing the wave travels and swells with the bass;
/// in the last moments the indicator shrinks away and the next line takes
/// over. Song time drives it, so pausing freezes it. `k` scales it with the
/// text (bigger in mega mode).
#[allow(clippy::too_many_arguments)]
fn waiting(painter: &egui::Painter, line: Rect, k: f32, start: Duration, end: Duration, pos: Duration, current: bool, playing: bool, bass: f32, edge: f32, prefs: &Prefs) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let theme = Theme::get(painter.ctx());
    let s = theme.scheme;
    let span = end.saturating_sub(start).as_secs_f32().max(0.001);
    let progress = if current { (pos.saturating_sub(start).as_secs_f32() / span).clamp(0.0, 1.0) } else { 0.0 };
    let remaining = end.saturating_sub(pos).as_secs_f32();
    let fade = if current { (remaining / 0.4).clamp(0.0, 1.0) } else { 1.0 };
    let fade = fade * fade * (3.0 - 2.0 * fade);
    // Only the wait that is happening is shown.
    if !current {
        return;
    }
    if edge * fade <= 0.01 {
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
        let alpha = edge * fade * reveal * (age / 0.12).clamp(0.0, 1.0);
        painter.text(c, Align2::CENTER_CENTER, format!("{digit:.0}"), style_at(28.0 * k * pop).font_id(), s.primary.gamma_multiply(alpha));
    }
    let alpha = edge * fade * (1.0 - reveal);
    if alpha <= 0.01 { return; }
    let shrink = (0.6 + 0.4 * fade) * (1.0 - 0.35 * reveal);
    let radius = base_radius * shrink;
    let thick = if thick_form { 6.0 } else { 3.0 } * k * shrink;
    let motion = !theme.reduced_motion;
    // As in Compose's CircularWavyProgressIndicator: flat near 0 % and
    // 100 %, wavy in between; the crests travel along the arc (one
    // wavelengths a second) and the ring itself stays put. Wall-clock
    // time while playing, so it never looks frozen; still when paused.
    let ramp = (progress / 0.1).min((1.0 - progress) / 0.05).clamp(0.0, 1.0);
    // About half the stroke: anything less reads as a lumpy polygon, and
    // the tip (where the wave comes out) looks straight half the time.
    let requested_amp = if prefs.lyrics_gap_style % 2 == 1 {
        ramp * k * shrink * (1.4 + 0.5 * if motion && playing { bass.clamp(0.0, 1.0) } else { 0.0 })
    } else { 0.0 };
    let (waves, amp) = wave_geometry(prefs.lyrics_gap_waves, radius, thick, requested_amp);
    let clock = if motion && playing { anim::now(painter.ctx()) as f32 } else { 0.0 };
    // One wavelength a second: a calm ripple. The ring itself stays put.
    let phase = clock * TAU;
    // Chord clearance includes both round caps and radial wave excursions.
    let clearance = thick + 2.0 * amp + 3.0 * k * shrink;
    let gap = 2.0 * (clearance / (2.0 * radius)).clamp(0.0, 1.0).asin();
    let sweep = progress * TAU;
    let arc = |from: f32, to: f32, amp: f32, color: Color32| {
        if to - from <= 0.0 {
            return;
        }
        let n = ((to - from) * radius / (k * shrink * 0.6)).ceil().max(2.0) as usize;
        let pts: Vec<egui::Pos2> = (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                // Anchored at the head and running back from it, like the
                // linear wavy progress: the wave comes out of the tip.
                let r = radius + amp * (waves * (sweep - a) - phase).sin();
                let a = a - FRAC_PI_2;
                pos2(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect();
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        painter.add(egui::Shape::line(pts, egui::Stroke::new(thick, color)));
        painter.circle_filled(first, thick * 0.5, color);
        painter.circle_filled(last, thick * 0.5, color);
    };
    // The flat track: the part still to wait.
    // Opaque scheme colours (as in Material: secondary-container track,
    // primary indicator) so caps and stroke joins never show darker
    // overlaps; transparency only appears while the ring fades out.
    arc(sweep + gap, TAU - gap, 0.0, s.secondary_container.gamma_multiply(alpha));
    // The wavy arc: the part already waited.
    arc(0.0, sweep, amp, s.primary.gamma_multiply(alpha));
}

fn wave_geometry(waves: f32, radius: f32, stroke: f32, requested_amp: f32) -> (f32, f32) {
    let waves = waves.clamp(3.0, 16.0).round();
    let wavelength = std::f32::consts::TAU * radius / waves;
    // Also bound the curvature so a thick stroke never folds over itself:
    // for r = R + A·sin(nθ) the crest/trough curvature is ≈ (R ± A·n²)/R²,
    // which must stay under 2/stroke.
    let fold = (2.0 * radius * radius / stroke - radius).max(0.0) / (waves * waves + 1.0);
    let amp = requested_amp.max(0.0).min(wavelength * 0.12).min(fold);
    (waves, amp)
}

/// A small "where is this text from" line; clicking it picks another source.
pub fn source_picker(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) -> bool {
    let l = &state.lyrics;
    let s = Theme::get(ui.ctx()).scheme;
    let sync = |src: LyricsSource| {
        l.sources.iter().find(|o| o.source == src).and_then(|o| match o.state {
            LyricsAvail::Ready(sync) => Some(sync),
            _ => None,
        })
    };
    let text = match l.source {
        Some(src) => match sync(src) {
            Some(sync) => format!("{} · {}", src.label(), sync.label()),
            None => src.label().to_owned(),
        },
        None if l.sources.iter().any(|o| o.state == LyricsAvail::Loading) => "Ищу текст…".to_owned(),
        None => "Источник текста".to_owned(),
    };
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let r = ui.add(
                egui::Label::new(TypeRole::LabelSmall.style().rich(text, s.on_surface_variant.gamma_multiply(0.8)))
                    .selectable(false)
                    .sense(Sense::click()),
            );
            Icon::ExpandMore
                .widget(false, s.on_surface_variant.gamma_multiply(0.8), 14.0)
                .paint_at(ui, Rect::from_min_size(pos2(r.rect.right() + 2.0, r.rect.center().y - 7.0), vec2(14.0, 14.0)));
            r
        })
        .inner;
    // The menu also leads to the animation settings, so it always opens.
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let track = l.track.filter(|_| !l.sources.is_empty());
    let mut menu = Menu::new(("lyrics-source", track));
    let n = if track.is_some() { l.sources.len() + 1 } else { 0 };
    if track.is_some() {
        menu = menu.item(MenuItem::new("Автоматически").selected(!l.picked)).divider();
        for o in &l.sources {
            let state = match o.state {
                LyricsAvail::Ready(sync) => sync.label(),
                LyricsAvail::Loading => "ищу…",
                LyricsAvail::Missing => "нет текста",
            };
            menu = menu.item(
                MenuItem::new(format!("{} · {state}", o.source.label()))
                    .enabled(matches!(o.state, LyricsAvail::Ready(_)))
                    .selected(l.picked && l.source == Some(o.source)),
            );
        }
        menu = menu.divider();
    }
    menu = menu.item(MenuItem::new("Настроить анимацию…"));
    match (menu.attach(&resp), track) {
        (Some(i), _) if i == n => true,
        (Some(i), Some(track)) => {
            let pick = if i == 0 { None } else { l.sources.get(i - 1).map(|o| o.source) };
            out.push(Command::PickLyricsSource(track, pick));
            false
        }
        _ => false,
    }
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
                    ui.cursor().top() + (ui.available_height() - Size::Xs.height() - ui.spacing().item_spacing.y) * MEGA.anchor);
                show(ui, &state, 0.0, true, &mut commands);
            });
            output.textures_delta.clear();
        }
        assert!(commands.iter().any(|c| matches!(c, Command::Seek(t) if *t == Duration::from_secs(2))));
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
                karaoke(painter, &g, line, 80.0, size * 0.35, Color32::GRAY, Color32::WHITE);
                timed_line(painter, &g, line, &timing, Duration::from_secs(4), true, 1.0, size, Style::Karaoke, Color32::GRAY, Color32::WHITE, &Prefs::default());
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
            assert!((at - (t.syllables[1].from + t.syllables[1].to) * 0.5).abs() < 0.001);
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
        assert_eq!(wave_geometry(2.0, 13.0, 3.0, 0.0), (3.0, 0.0));
        assert_eq!(wave_geometry(6.49, 13.0, 3.0, 0.0).0, 6.0);
        assert_eq!(wave_geometry(6.5, 13.0, 3.0, 0.0).0, 7.0);
        assert_eq!(wave_geometry(20.0, 13.0, 3.0, 0.0).0, 16.0);
        // The default thin ring (6 crests) gets a wave about half its stroke.
        assert!(wave_geometry(6.0, 13.0, 3.0, 100.0).1 >= 1.5);
        for radius in [13.0, 16.0, 24.0] {
            for stroke in [3.0, 6.0, 9.0] {
                for count in 3..=16 {
                    let (waves, amp) = wave_geometry(count as f32, radius, stroke, 100.0);
                    assert!(amp <= std::f32::consts::TAU * radius / waves * 0.12);
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
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    waiting(ui.painter(), Rect::from_min_size(pos2(20.0, 20.0), vec2(56.0, 42.0)), 1.0,
                        Duration::ZERO, Duration::from_secs(10), Duration::from_secs(5), true, false, 1.0, 1.0, &prefs);
                });
                output.textures_delta.clear();
                let mut paths = 0;
                for shape in output.shapes {
                    match shape.shape {
                        egui::Shape::Path(path) => {
                            let egui::epaint::ColorMode::Solid(color) = path.stroke.color else { panic!("ring must use a solid color"); };
                            assert_eq!(color.a(), 255);
                            assert_eq!(path.stroke.width, if form >= 2 { 6.0 } else { 3.0 });
                            paths += 1;
                        }
                        egui::Shape::Circle(circle) => assert_eq!(circle.fill.a(), 255),
                        _ => {}
                    }
                }
                assert_eq!(paths, 2);
            }
            let prefs = Prefs { lyrics_countdown: true, ..Prefs::default() };
            for remaining in [2.95, 2.5, 1.5, 0.5] {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    waiting(ui.painter(), Rect::from_min_size(pos2(20.0, 20.0), vec2(56.0, 42.0)), 1.0,
                        Duration::ZERO, Duration::from_secs(10), Duration::from_secs_f32(10.0 - remaining),
                        true, false, 0.0, 1.0, &prefs);
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
                    assert!(!output.shapes.iter().any(|shape| matches!(shape.shape, egui::Shape::Path(_))));
                }
            }
        }
    }
}
