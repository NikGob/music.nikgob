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
use nikgob_view::{Command, LyricLine, LyricsAvail, LyricsSource, Prefs, Status, ViewState};

use crate::{
    Icon, Theme, TypeRole, TypeStyle,
    components::{ConnectedGroup, Menu, MenuItem, anim},
};

/// Point sizes of the current line and of the others.
const BIG: f32 = 26.0;
const SMALL: f32 = 20.0;
/// Translations under a line.
const SUB: f32 = 15.0;
/// Background vocals, relative to a main line.
const BG_SCALE: f32 = 0.72;

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

    // The current line is the last main line that has started; background
    // vocals light up on their own while they are sung.
    let cur = lines.iter().rposition(|l| !l.background && l.start <= pos);
    let ci = cur.unwrap_or(0);
    let next_start = |i: usize| lines[i + 1..].iter().find(|l| !l.background).map(|l| l.start);
    let active = |i: usize| {
        let l = &lines[i];
        if l.background {
            l.start <= pos && pos < l.end.unwrap_or(l.start + Duration::from_secs(5))
        } else {
            cur == Some(i)
        }
    };
    // How far into its singing a line is. Without an end time: a pace of
    // ~15 letters a second, never past the next line's start.
    let progress = |i: usize| {
        let l = &lines[i];
        let gap = l
            .end
            .or_else(|| next_start(i))
            .map_or(Duration::from_secs(6), |t| t.saturating_sub(l.start))
            .as_secs_f32();
        let sung = if l.end.is_some() { gap.max(0.3) } else { speaking_secs(&l.text, gap) };
        (pos.saturating_sub(l.start).as_secs_f32() / sung).clamp(0.0, 1.0)
    };

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
    let sub_layout = |text: &str| -> Arc<Galley> {
        let mut job = LayoutJob::single_section(text.to_owned(), style_at(SUB).text_format(Color32::WHITE));
        job.wrap.max_width = wrap;
        painter.layout_job(job)
    };
    let scale = |l: &LyricLine| if l.background { BG_SCALE } else { 1.0 };
    let slots: Vec<(f32, f32)> = lines
        .iter()
        .map(|l| {
            let main = layout(&l.text, BIG * scale(l)).size().y;
            let extra = sub(l).map_or(0.0, |t| sub_layout(t).size().y + 4.0);
            (main, extra)
        })
        .collect();
    let gap = 18.0;
    let mut tops = Vec::with_capacity(slots.len());
    let mut y = 0.0;
    for (i, (h, extra)) in slots.iter().enumerate() {
        // Background vocals hug the line they belong to.
        if lines[i].background && i > 0 {
            y -= gap * 0.6;
        }
        tops.push(y);
        y += h + extra + gap;
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
    let scroll = tops[ci] + slots[ci].0 * 0.5 - anchor + manual;

    let hover = resp.hover_pos();
    let mut clicked = None;
    for (i, l) in lines.iter().enumerate() {
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
        let (slot_h, extra) = slots[i];
        if y + slot_h + extra < rect.top() - 60.0 || y > rect.bottom() + 60.0 {
            continue;
        }
        let is_cur = active(i);
        // 0 → 1 as the line becomes the current one, and back.
        let focus = anim::f32(
            &ctx,
            id.with(("focus", i)),
            if is_cur { 1.0 } else { 0.0 },
            Spring::new(1.0, 170.0),
        )
        .clamp(0.0, 1.0);
        let size = (SMALL + (BIG - SMALL) * focus) * scale(l);
        let g = layout(&l.text, size);
        // The second voice of a duet sings from the other side.
        let side = |w: f32| {
            if l.duet {
                rect.left() + 12.0 + (wrap - w).max(0.0)
            } else {
                rect.left() + 12.0
            }
        };
        let line = Rect::from_min_size(pos2(side(g.size().x), y + (slot_h - g.size().y) * 0.5), g.size());
        // Fade out towards the edges.
        let edge = ((y + slot_h - rect.top()) / 72.0).min((rect.bottom() - y) / 72.0).clamp(0.0, 1.0);
        let slot = Rect::from_min_size(pos2(rect.left() + 12.0, y), vec2(wrap, slot_h + extra));
        let hovered = hover.is_some_and(|p| slot.expand2(vec2(8.0, gap * 0.5)).contains(p));
        if hovered {
            painter.rect_filled(slot.expand2(vec2(10.0, 6.0)), 14, s.on_surface.gamma_multiply(0.06 * edge));
            if resp.clicked() {
                clicked = Some(i);
            }
        }
        // Brightness of a line at rest: past lines a bit brighter than the
        // ones to come; focus mode pushes the rest far back.
        let mut rest = match style {
            Style::Focus => 0.62f32.powf(far).max(0.08) * 0.7,
            _ => (if dist < 0 { 0.5 } else { 0.42 }) * 0.86f32.powf((far - 1.0).max(0.0)),
        };
        if l.background {
            rest *= 0.8;
        }
        if l.text.trim().is_empty() {
            let p = if is_cur { progress(i) } else { 0.0 };
            dots(&painter, line, p, focus, edge, s.on_surface);
            continue;
        }
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
        // Per-letter singing: real timing for word-synced lines, else an
        // even pace over the line.
        let timing = Timing::new(l, &g);
        let line_p = progress(i);
        let n = glyph_count(&g) as f32;
        let sung = |k: usize| -> f32 {
            if !is_cur {
                return 1.0;
            }
            match &timing {
                Some(t) => t.lit(k, pos),
                None => ((line_p * (n + 2.0) - k as f32) / 2.0).clamp(0.0, 1.0),
            }
        };
        match style {
            Style::Karaoke => {
                // A line that just stopped being current keeps its fill and
                // fades with its focus.
                let widths = glyph_widths(&g);
                let total = widths.iter().sum::<f32>().max(1.0);
                let head = if !is_cur {
                    f32::INFINITY
                } else if timing.is_some() {
                    let at: f32 = widths.iter().enumerate().map(|(k, w)| w * sung(k)).sum();
                    if at >= total - 0.5 {
                        f32::INFINITY
                    } else {
                        at + FEATHER * (0.5 + 0.5 * at / total)
                    }
                } else {
                    (total + FEATHER) * line_p
                };
                karaoke(&painter, &g, line, head, dim, lit.gamma_multiply(focus.max(0.35)));
            }
            Style::Focus => {
                let c = crate::lerp_color(dim, lit, focus);
                painter.galley_with_override_text_color(line.min, g, c);
            }
            Style::Wave => wave(&painter, &g, line, &sung, focus, size, dim, lit),
        }
    }
    if let Some(i) = clicked {
        out.push(Command::Seek(lines[i].start));
    }
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if playing {
        ctx.request_repaint();
    }
}

/// How long a line without an end time takes to sing: ~15 letters a second,
/// at least 0.8 s, but never past 92 % of the `gap` (seconds) to the next line.
fn speaking_secs(text: &str, gap: f32) -> f32 {
    let letters = text.chars().filter(|c| !c.is_whitespace()).count() as f32;
    // Not `clamp`: with lines closer than 0.87 s the cap is the smaller
    // bound, and `clamp` panics on that.
    (0.4 + letters * 0.066).max(0.8).min((gap * 0.92).max(0.3))
}

/// Per-glyph singing times of a word-synced line.
struct Timing {
    /// (start, end) in seconds for each glyph of the galley, rows flattened.
    glyphs: Vec<(f32, f32)>,
}

impl Timing {
    /// `None` for line-synced lines. Letters of a word share its time
    /// evenly; spaces take the end of the letter before them.
    fn new(l: &LyricLine, g: &Galley) -> Option<Self> {
        if l.words.is_empty() {
            return None;
        }
        let mut letters = Vec::new();
        for w in &l.words {
            let n = w.text.chars().filter(|c| !c.is_whitespace()).count();
            let (s, e) = (w.start.as_secs_f32(), w.end.as_secs_f32());
            let e = e.max(s);
            for k in 0..n {
                let (a, b) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
                letters.push((s + (e - s) * a, s + (e - s) * b));
            }
        }
        let mut next = letters.iter().copied();
        let mut last = letters.first().copied().unwrap_or_default();
        let glyphs = g
            .rows
            .iter()
            .flat_map(|r| r.row.glyphs.iter())
            .map(|gl| {
                if !gl.chr.is_whitespace()
                    && let Some(t) = next.next()
                {
                    last = t;
                    t
                } else {
                    (last.1, last.1)
                }
            })
            .collect();
        Some(Self { glyphs })
    }

    /// 0 → 1 as glyph `k` is sung.
    fn lit(&self, k: usize, pos: Duration) -> f32 {
        let Some(&(s, e)) = self.glyphs.get(k) else { return 1.0 };
        let p = pos.as_secs_f32();
        ((p - s + 0.04) / (e - s + 0.08).max(0.1)).clamp(0.0, 1.0)
    }
}

/// The small line under a lyric line: its translation, else romanization.
fn sub(l: &LyricLine) -> Option<&str> {
    l.translation.as_deref().or(l.romanization.as_deref())
}

fn glyph_count(g: &Galley) -> usize {
    g.rows.iter().map(|r| r.row.glyphs.len()).sum::<usize>().max(1)
}

/// Advance of every glyph, rows flattened.
fn glyph_widths(g: &Galley) -> Vec<f32> {
    g.rows
        .iter()
        .flat_map(|r| r.row.glyphs.iter().map(|gl| gl.advance_width))
        .collect()
}

const FEATHER: f32 = 36.0;

/// The line lit left to right (row by row when it wraps) up to `head`
/// pixels of its glyphs (rows flattened), the edge of the light a soft
/// gradient.
fn karaoke(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, head: f32, base: Color32, lit: Color32) {
    if head.is_infinite() {
        painter.galley_with_override_text_color(line.min, g.clone(), lit);
        return;
    }
    painter.galley_with_override_text_color(line.min, g.clone(), base);
    let mut left = head;
    for row in &g.rows {
        let r = row.rect().translate(line.min.to_vec2()).expand2(vec2(0.0, 6.0));
        let w: f32 = row.row.glyphs.iter().map(|gl| gl.advance_width).sum();
        let head = left;
        left -= w;
        if head <= 0.0 {
            break;
        }
        // Solid up to the gradient, then slices fading out.
        let solid = (head - FEATHER).clamp(0.0, w);
        if solid > 0.0 {
            let clip = Rect::from_min_max(r.min, pos2(r.left() + solid, r.bottom()));
            painter
                .with_clip_rect(clip.intersect(painter.clip_rect()))
                .galley_with_override_text_color(line.min, g.clone(), lit);
        }
        const SLICES: usize = 8;
        for k in 0..SLICES {
            let x0 = head - FEATHER + FEATHER * k as f32 / SLICES as f32;
            let x1 = head - FEATHER + FEATHER * (k + 1) as f32 / SLICES as f32;
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

/// Letter by letter: each one rises a little and lights up as it is sung
/// (`sung(k)` goes 0 → 1), and stays up until the line is done.
#[allow(clippy::too_many_arguments)]
fn wave(
    painter: &egui::Painter,
    g: &Arc<Galley>,
    line: Rect,
    sung: &dyn Fn(usize) -> f32,
    focus: f32,
    size: f32,
    base: Color32,
    lit: Color32,
) {
    let font = style_at(size).font_id();
    let mut k = 0usize;
    for row in &g.rows {
        for glyph in &row.row.glyphs {
            let t = sung(k);
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

/// A small "where is this text from" line; clicking it picks another source.
pub fn source_picker(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let l = &state.lyrics;
    let (Some(track), false) = (l.track, l.sources.is_empty()) else { return };
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
    let more = l.sources.len() > 1;
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let r = ui.add(
                egui::Label::new(TypeRole::LabelSmall.style().rich(text, s.on_surface_variant.gamma_multiply(0.8)))
                    .selectable(false)
                    .sense(if more { Sense::click() } else { Sense::hover() }),
            );
            if more {
                Icon::ExpandMore
                    .widget(false, s.on_surface_variant.gamma_multiply(0.8), 14.0)
                    .paint_at(ui, Rect::from_min_size(pos2(r.rect.right() + 2.0, r.rect.center().y - 7.0), vec2(14.0, 14.0)));
            }
            r
        })
        .inner;
    if !more {
        return;
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let mut menu = Menu::new(("lyrics-source", track))
        .item(MenuItem::new("Автоматически").selected(!l.picked))
        .divider();
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
    if let Some(i) = menu.attach(&resp) {
        let pick = if i == 0 { None } else { l.sources.get(i - 1).map(|o| o.source) };
        out.push(Command::PickLyricsSource(track, pick));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_lines_do_not_panic_and_keep_the_pace_inside_the_gap() {
        // Lines 0.5 s apart (and duplicate stamps) used to panic in `clamp`.
        assert!((speaking_secs("вторая быстро", 0.5) - 0.46).abs() < 1e-4);
        assert_eq!(speaking_secs("a", 0.0), 0.3);
        assert_eq!(speaking_secs("a", 6.0), 0.8);
        assert!((speaking_secs("слова звучат громче всего", 6.0) - (0.4 + 22.0 * 0.066)).abs() < 1e-4);
    }
}
