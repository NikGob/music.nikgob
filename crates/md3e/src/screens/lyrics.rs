//! Synced lyrics, animated like the big players do it: the current line is
//! larger and bright, the others smaller and dimmer the further they are,
//! and size and brightness flow between them instead of switching. Lines
//! glide into place in a wave (the further from the current one, the
//! lazier). The current line lights up as it is sung: a soft karaoke
//! sweep, a plain focus, or letters rising one by one. Lines only carry a
//! start time, so the sweep runs at a speaking pace for the line's length,
//! not across the whole gap to the next line. The view follows the song
//! unless the user scrolls away (it comes back three seconds later).

use std::f32::consts::PI;
use std::sync::Arc;
use std::time::Duration;

use egui::{Align2, Color32, Galley, Rect, Sense, Ui, pos2, text::LayoutJob, vec2};
use nikgob_motion::Spring;
use nikgob_view::{Command, Prefs, Status, ViewState};

use crate::{
    Theme, TypeStyle,
    components::{ConnectedGroup, anim},
};

/// Point sizes of the current line and of the others.
const BIG: f32 = 26.0;
const SMALL: f32 = 20.0;

fn style_at(size: f32) -> TypeStyle {
    TypeStyle::new(size, (size * 1.28).round(), 600, 0.0)
}

const STYLES: [&str; 3] = ["Караоке", "Фокус", "Волна"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Karaoke,
    Focus,
    Wave,
}

/// The line-by-line view with the style switch under it. `lines` must not
/// be empty.
pub fn show(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let style = match state.prefs.lyrics_style {
        1 => Style::Focus,
        2 => Style::Wave,
        _ => Style::Karaoke,
    };
    let switch_h = 56.0;
    let size = vec2(ui.available_width(), (ui.available_height() - switch_h).max(80.0));
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    paint(ui, rect, &resp, state, style, out);

    ui.add_space(12.0);
    let w = ((ui.available_width() - 4.0) / 3.0).floor();
    if let Some(i) = ConnectedGroup::new("lyrics-style", &STYLES)
        .equal_width(w)
        .show(ui, style as usize)
        && i as u8 != state.prefs.lyrics_style
    {
        out.push(Command::SetPrefs(Box::new(Prefs {
            lyrics_style: i as u8,
            ..state.prefs.clone()
        })));
    }
}

fn paint(
    ui: &Ui,
    rect: Rect,
    resp: &egui::Response,
    state: &ViewState,
    style: Style,
    out: &mut Vec<Command>,
) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let lines = &state.lyrics.lines;
    let painter = ui.painter_at(rect);
    let id = ui.id().with("lyrics-view");
    let now = anim::now(&ctx);
    let pos = state.player.position_now();
    let playing = state.player.status == Status::Playing;

    // The current line and how far into its singing the song is: a pace
    // of ~15 letters a second, never past the next line's start.
    let cur = lines.iter().rposition(|(t, _)| *t <= pos);
    let ci = cur.unwrap_or(0);
    let progress = cur.map_or(0.0, |i| {
        let start = lines[i].0;
        let gap = lines
            .get(i + 1)
            .map_or(Duration::from_secs(6), |(t, _)| t.saturating_sub(start))
            .as_secs_f32();
        let letters = lines[i].1.chars().filter(|c| !c.is_whitespace()).count() as f32;
        let sung = (0.4 + letters * 0.066).clamp(0.8, (gap * 0.92).max(0.3));
        (pos.saturating_sub(start).as_secs_f32() / sung).clamp(0.0, 1.0)
    });

    // Slots are laid out at the big size, so nothing moves when a line
    // grows; the drawn size flows inside its slot.
    let wrap = rect.width() - 24.0;
    let layout = |text: &str, size: f32| -> Arc<Galley> {
        let text = if text.trim().is_empty() { " " } else { text };
        let mut job = LayoutJob::single_section(text.to_owned(), style_at(size).text_format(Color32::WHITE));
        // Wrap in proportion, so a line breaks at the same words at any size.
        job.wrap.max_width = wrap * size / BIG;
        painter.layout_job(job)
    };
    let slots: Vec<f32> = lines.iter().map(|(_, t)| layout(t, BIG).size().y).collect();
    let gap = 18.0;
    let mut tops = Vec::with_capacity(slots.len());
    let mut y = 0.0;
    for h in &slots {
        tops.push(y);
        y += h + gap;
    }

    // Follow the song, unless the user scrolled in the last three seconds.
    let (mut manual, mut until): (f32, f64) = ctx.data(|d| d.get_temp(id)).unwrap_or((0.0, 0.0));
    if resp.hovered() {
        let dy = ctx.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            manual -= dy;
            until = now + 3.0;
            ctx.input_mut(|i| i.smooth_scroll_delta.y = 0.0);
        }
    }
    let manual_on = now < until;
    if !manual_on {
        manual = 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(id, (manual, until)));
    if manual_on {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    let anchor = rect.height() * 0.38;
    let scroll = tops[ci] + slots[ci] * 0.5 - anchor + manual;

    let hover = resp.hover_pos();
    let mut clicked = None;
    for i in 0..lines.len() {
        let dist = i as i32 - ci as i32;
        let far = dist.unsigned_abs().min(8) as f32;
        // A wave: lines further from the current one follow more lazily,
        // without wobbling past their place.
        let spring = if manual_on {
            Spring::new(1.0, 1600.0)
        } else {
            Spring::new(0.9, (300.0 - 26.0 * far).max(110.0))
        };
        let y = anim::f32(&ctx, id.with(("y", i)), rect.top() + tops[i] - scroll, spring);
        let slot_h = slots[i];
        if y + slot_h < rect.top() - 60.0 || y > rect.bottom() + 60.0 {
            continue;
        }
        let is_cur = cur == Some(i);
        // 0 → 1 as the line becomes the current one, and back.
        let focus = anim::f32(
            &ctx,
            id.with(("focus", i)),
            if is_cur { 1.0 } else { 0.0 },
            Spring::new(1.0, 170.0),
        )
        .clamp(0.0, 1.0);
        let size = SMALL + (BIG - SMALL) * focus;
        let g = layout(&lines[i].1, size);
        let line = Rect::from_min_size(
            pos2(rect.left() + 12.0, y + (slot_h - g.size().y) * 0.5),
            g.size(),
        );
        // Fade out towards the edges.
        let edge = ((y + slot_h - rect.top()) / 72.0).min((rect.bottom() - y) / 72.0).clamp(0.0, 1.0);
        let slot = Rect::from_min_size(pos2(rect.left() + 12.0, y), vec2(wrap, slot_h));
        let hovered = hover.is_some_and(|p| slot.expand2(vec2(8.0, gap * 0.5)).contains(p));
        if hovered {
            painter.rect_filled(slot.expand2(vec2(10.0, 6.0)), 14, s.on_surface.gamma_multiply(0.06 * edge));
            if resp.clicked() {
                clicked = Some(i);
            }
        }
        // Brightness of a line at rest: past lines a bit brighter than the
        // ones to come; focus mode pushes the rest far back.
        let rest = match style {
            Style::Focus => 0.62f32.powf(far).max(0.08) * 0.7,
            _ => (if dist < 0 { 0.5 } else { 0.42 }) * 0.86f32.powf((far - 1.0).max(0.0)),
        };
        if lines[i].1.trim().is_empty() {
            dots(&painter, line, if is_cur { progress } else { 0.0 }, focus, edge, s.on_surface);
            continue;
        }
        let dim = s.on_surface.gamma_multiply(edge * (rest + (1.0 - rest) * focus * 0.4));
        let lit = s.on_surface.gamma_multiply(edge);
        if !is_cur && focus < 0.01 {
            painter.galley_with_override_text_color(line.min, g, dim);
            continue;
        }
        match style {
            Style::Karaoke => {
                // A line that just stopped being current keeps its fill and
                // fades with its focus.
                let p = if is_cur { progress } else { 1.0 };
                karaoke(&painter, &g, line, p, dim, lit.gamma_multiply(focus.max(0.35)));
            }
            Style::Focus => {
                let c = crate::lerp_color(dim, lit, focus);
                painter.galley_with_override_text_color(line.min, g, c);
            }
            Style::Wave => {
                let p = if is_cur { progress } else { 1.0 };
                wave(&painter, &g, line, p, focus, size, dim, lit);
            }
        }
    }
    if let Some(i) = clicked {
        out.push(Command::Seek(lines[i].0));
    }
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if playing {
        ctx.request_repaint();
    }
}

/// The line lit left to right (row by row when it wraps) as it is sung,
/// the edge of the light a soft gradient.
fn karaoke(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, progress: f32, base: Color32, lit: Color32) {
    painter.galley_with_override_text_color(line.min, g.clone(), base);
    let feather = 36.0;
    let total: f32 = g.rows.iter().map(|r| r.rect().width()).sum::<f32>() + feather;
    let mut left = total * progress;
    for row in &g.rows {
        let r = row.rect().translate(line.min.to_vec2()).expand2(vec2(0.0, 6.0));
        let w = r.width();
        let head = left;
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

/// Letter by letter: each one rises a little and lights up as the song
/// reaches it, and stays up until the line is done.
#[allow(clippy::too_many_arguments)]
fn wave(
    painter: &egui::Painter,
    g: &Arc<Galley>,
    line: Rect,
    progress: f32,
    focus: f32,
    size: f32,
    base: Color32,
    lit: Color32,
) {
    let n: usize = g.rows.iter().map(|r| r.row.glyphs.len()).sum::<usize>().max(1);
    // The head runs a little past the end so the last letters settle too.
    let head = progress * (n as f32 + 2.0);
    let font = style_at(size).font_id();
    let mut k = 0usize;
    for row in &g.rows {
        for glyph in &row.row.glyphs {
            let t = ((head - k as f32) / 2.0).clamp(0.0, 1.0);
            k += 1;
            if glyph.chr.is_whitespace() {
                continue;
            }
            let on = t * t * (3.0 - 2.0 * t) * focus;
            let rise = on * 4.0 + (PI * t).sin() * 2.0 * focus;
            let color = crate::lerp_color(base, lit, on);
            let p = line.min + row.pos.to_vec2() + vec2(glyph.pos.x, glyph.pos.y - glyph.font_ascent - rise);
            painter.text(p, Align2::LEFT_TOP, glyph.chr, font.clone(), color);
        }
    }
}

/// An instrumental break: three dots breathing in turn, growing with the
/// gap's progress while it is the current line.
fn dots(painter: &egui::Painter, line: Rect, progress: f32, focus: f32, edge: f32, color: Color32) {
    let cy = line.center().y;
    let t = anim_time(painter);
    for i in 0..3 {
        let x = line.left() + 8.0 + i as f32 * 20.0;
        let phase = (progress * 3.0 - i as f32).clamp(0.0, 1.0);
        let breathe = 1.0 + 0.12 * focus * (t * 3.0 - i as f32 * 0.7).sin();
        let r = (3.5 + 2.5 * phase * focus) * breathe;
        let a = 0.3 + 0.7 * phase * focus;
        painter.circle_filled(pos2(x, cy), r, color.gamma_multiply(a * edge));
    }
}

fn anim_time(painter: &egui::Painter) -> f32 {
    (anim::now(painter.ctx()) % 1000.0) as f32
}
