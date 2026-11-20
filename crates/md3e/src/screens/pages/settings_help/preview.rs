use std::collections::HashMap;

use egui::{Id, Pos2, Rect, Stroke, Ui, pos2, vec2};
use nikgob_core::i18n::settings::settings_misc;
use nikgob_view::{self as v, Prefs};

use crate::{
    ColorScheme, Contrast, Icon, SchemeVariant, Theme, TypeRole,
    components::{anim, slider::wave_line},
    tokens::shape,
};
use crate::screens::{bg_shapes, lyrics, now_playing};

/// The live sample a card draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Preview {
    Palette,
    Ambient,
    Wave,
    Cover,
    Ring,
    Background,
    Lyrics,
    Gap,
    Motion,
    Volume,
    /// Which way the cover outline (false) or the ring (true) turns.
    Turn,
    /// Lines following a scroll, with or without the cascade.
    Cascade,
    /// Lines rolling through the lyrics' soft edges (or cut, when off).
    Edges,
    /// The end of a pause: digits 3-2-1, or the ring simply fading away.
    Countdown,
}

/// A preview in its own box: `salt` marks an option's sample (an option
/// that turns the effect off then says so instead of showing a stand-in).
fn sample(ui: &mut Ui, rect: Rect, kind: Preview, prefs: &Prefs, salt: Option<Id>) {
    let palette = (kind == Preview::Palette && salt.is_some()).then(|| scheme_for(ui.ctx(), prefs));
    let bg = palette.map_or_else(|| Theme::get(ui.ctx()).scheme.surface, |s| s.surface);
    ui.painter().rect_filled(rect, shape::SMALL, bg);
    let mut pui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    pui.set_clip_rect(rect.intersect(ui.clip_rect()));
    paint_preview(&mut pui, rect, kind, prefs, salt, palette);
}

/// The scheme `prefs` would give the current cover (for the palette samples
/// of other themes, contrasts and variants), cached.
fn scheme_for(ctx: &egui::Context, prefs: &Prefs) -> ColorScheme {
    type Key = (u32, bool, SchemeVariant, Contrast);
    let seed = ctx.data(|d| d.get_temp::<u32>(crate::screens::scheme_seed_id())).unwrap_or(prefs.fallback_seed);
    let (dark, variant, contrast) = crate::screens::theme_key(ctx, prefs);
    let key: Key = (seed, dark, variant, contrast);
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<HashMap<Key, ColorScheme>>(Id::new("settings-help-schemes"));
        *cache.entry(key).or_insert_with(|| ColorScheme::from_seed(seed, variant, dark, contrast))
    })
}

fn off_label(ui: &Ui, rect: Rect) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, settings_misc().off, TypeRole::LabelLarge.style().font_id(), s.on_surface_variant);
}

fn paint_preview(ui: &mut Ui, rect: Rect, kind: Preview, prefs: &Prefs, salt: Option<Id>, palette: Option<ColorScheme>) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let exact = salt.is_some();
    match kind {
        Preview::Palette => {
            // The scheme the interface paints with (or would, for an option).
            let s = palette.unwrap_or(s);
            let swatches = [s.primary, s.secondary, s.tertiary, s.primary_container, s.surface_container_highest];
            let small = rect.height() < 100.0;
            let pad = if small { 6.0 } else { 8.0 };
            let w = (rect.width() - 2.0 * pad) / swatches.len() as f32;
            let sh = if small { rect.height() * 0.42 } else { 64.0 };
            for (i, c) in swatches.iter().enumerate() {
                let r = Rect::from_min_size(pos2(rect.left() + pad + i as f32 * w, rect.top() + pad), vec2(w - 4.0, sh));
                ui.painter().rect_filled(r, if small { 8.0 } else { shape::MEDIUM }, *c);
            }
            let bh = if small { rect.height() - sh - 3.0 * pad } else { 32.0 };
            let text = Rect::from_min_size(pos2(rect.left() + pad + 4.0, rect.bottom() - pad - bh), vec2(rect.width() - 2.0 * pad - 8.0, bh));
            ui.painter().rect_filled(text, bh * 0.5, s.primary);
            let role = if small { TypeRole::LabelMedium } else { TypeRole::LabelLarge };
            ui.painter().text(text.center(), egui::Align2::CENTER_CENTER, settings_misc().button_sample, role.style().font_id(), s.on_primary);
        }
        Preview::Ambient => {
            if exact && prefs.ambient == v::Ambient::Off {
                off_label(ui, rect);
            } else {
                // Off (for the row): a sample of the calm glow.
                let t = anim::now(ui.ctx()) as f32;
                let bass = if prefs.ambient == v::Ambient::Reactive { beat(t) } else { 0.0 };
                crate::screens::ambient::paint(ui.ctx(), ui.painter(), rect, bass, true, prefs.ambient_strength);
            }
        }
        Preview::Wave => {
            let t = anim::now(ui.ctx()) as f32;
            let cy = rect.center().y;
            let (x0, x1) = (rect.left() + 20.0, rect.right() - 20.0);
            let head = x0 + (x1 - x0) * (0.25 + 0.5 * ((t * 0.1).fract()));
            let wavy = prefs.progress_wave && !theme.reduced_motion && !prefs.reduced_motion;
            let amp = if wavy { 3.0 } else { 0.0 };
            let phase = (t * 0.6).fract() * std::f32::consts::TAU;
            wave_line(ui, x0, head, cy, amp, 40.0, phase, Stroke::new(4.0, s.primary));
            ui.painter().line_segment([pos2(head + 8.0, cy), pos2(x1, cy)], Stroke::new(4.0, s.secondary_container));
            ui.painter().circle_filled(pos2(x1, cy), 2.0, s.primary);
        }
        Preview::Cover => now_playing::preview(ui, rect, prefs, false),
        Preview::Ring => now_playing::preview(ui, rect, prefs, prefs.ring),
        Preview::Background => {
            if exact && prefs.bg_shapes == v::BgShapes::Off {
                off_label(ui, rect);
            } else {
                // Off (for the row and its fields): a sample of how they
                // would float.
                let sample;
                let prefs = if prefs.bg_shapes == v::BgShapes::Off {
                    sample = Prefs { bg_shapes: v::BgShapes::Float, ..prefs.clone() };
                    &sample
                } else {
                    prefs
                };
                // A slower beat than the others: one a second, so bouncing
                // shapes land between the hops.
                let t = anim::now(ui.ctx()) as f32;
                let kick = beat(t * 0.5);
                let transients: Vec<f32> = (0..8).map(|i| kick * (0.6 + 0.4 * (i as f32 * 1.7 + t).sin())).collect();
                bg_shapes::preview(ui, rect, prefs, kick, &transients, salt.unwrap_or(Id::new("row")));
            }
        }
        Preview::Lyrics => lyrics::preview(ui, rect, prefs),
        // The ring's look: no countdown over it.
        Preview::Gap => lyrics::gap_preview(ui, rect, &Prefs { lyrics_countdown: false, ..prefs.clone() }),
        // What turning it on gives, whatever the switch says now.
        Preview::Countdown => lyrics::countdown_preview(ui, rect, &Prefs { lyrics_countdown: true, ..prefs.clone() }),
        Preview::Motion => motion_preview(ui, rect, prefs),
        Preview::Volume => volume_preview(ui, rect, prefs),
        Preview::Turn => turn_preview(ui, rect, prefs),
        Preview::Cascade => cascade_preview(ui, rect, prefs),
        Preview::Edges => lyrics::edges_preview(ui, rect, prefs),
    }
    ui.ctx().request_repaint();
}

/// Now Playing opening from the player bar and closing back, on loop, with
/// the spring of `prefs.np_motion`.
fn motion_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let spring = anim::effective(ui.ctx(), crate::screens::np_spring(prefs.np_motion));
    // Opens, stays open a while, closes, rests a little.
    const OPEN: f32 = 1.8;
    const LOOP: f32 = 2.8;
    let t = (anim::now(ui.ctx()) as f32).rem_euclid(LOOP);
    let opening = t < OPEN;
    let (x, _) = spring.solve(1.0, 0.0, if opening { t } else { t - OPEN });
    let k = if opening { 1.0 - x } else { x };
    let pad = (rect.height() * 0.1).min(12.0);
    let bar_h = (rect.height() * 0.22).clamp(14.0, 28.0);
    let bar = Rect::from_min_max(pos2(rect.left() + pad, rect.bottom() - pad - bar_h), pos2(rect.right() - pad, rect.bottom() - pad));
    let full = rect.shrink(pad * 0.5);
    let lerp = |a: f32, b: f32| a + (b - a) * k;
    let open = Rect::from_min_max(pos2(lerp(bar.left(), full.left()), lerp(bar.top(), full.top())), pos2(lerp(bar.right(), full.right()), lerp(bar.bottom(), full.bottom())));
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    painter.rect_filled(bar, bar_h * 0.5, s.surface_container_highest);
    painter.rect_filled(open, lerp(bar_h * 0.5, 10.0), s.surface_container_high);
    // The cover grows from the bar's thumbnail to the middle of the screen.
    let side = lerp(bar_h - 8.0, open.height() * 0.55).max(4.0);
    let c = pos2(lerp(open.left() + 4.0 + side * 0.5, open.center().x), lerp(open.center().y, open.top() + open.height() * 0.42));
    painter.rect_filled(Rect::from_center_size(c, Vec2::splat(side)), lerp(4.0, 8.0), s.primary);
}

/// The player's volume control under `prefs.np_volume` /
/// `np_volume_pop`: a slider beside the sound button, a pill rising at the
/// bottom while the volume changes, or the button alone.
fn volume_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    // A second of changing the volume, then the pill's hold, then a rest.
    let hold = prefs.np_volume_pop_hold.clamp(0.3, 5.0);
    let cycle = 1.0 + hold + 0.7;
    let t = (anim::now(ui.ctx()) as f32).rem_euclid(cycle);
    let changing = t.min(1.0);
    let level = 0.35 + 0.3 * (changing * 2.1).sin().abs();
    let pad = (rect.height() * 0.14).clamp(6.0, 14.0);
    let r = (rect.height() * 0.12).clamp(7.0, 12.0);
    let button = pos2(rect.left() + pad + r, rect.top() + pad + r);
    painter.circle_filled(button, r, s.surface_container_highest);
    Icon::VolumeUp.widget(false, s.on_surface_variant, r * 1.3).paint_at(ui, Rect::from_center_size(button, Vec2::splat(r * 1.3)));
    let bar = |painter: &Painter, a: Pos2, len: f32, alpha: f32| {
        let b = a + vec2(len, 0.0);
        painter.line_segment([a, b], Stroke::new(4.0, s.secondary_container.gamma_multiply(alpha)));
        painter.line_segment([a, a + vec2(len * level, 0.0)], Stroke::new(4.0, s.primary.gamma_multiply(alpha)));
        painter.circle_filled(a + vec2(len * level, 0.0), 5.0, s.primary.gamma_multiply(alpha));
    };
    if prefs.np_volume {
        bar(&painter, button + vec2(r + 8.0, 0.0), (rect.right() - pad - button.x - r - 8.0).max(10.0), 1.0);
    } else if prefs.np_volume_pop {
        // Rises while the volume changes, stays for the hold, then hides.
        let k = ((t / 0.2).min((1.0 + hold + 0.25 - t) / 0.25)).clamp(0.0, 1.0);
        let k = k * k * (3.0 - 2.0 * k);
        let w = (rect.width() * 0.6).min(160.0);
        let h = 20.0;
        let pill = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - pad - h * 0.5 + (1.0 - k) * 10.0), vec2(w, h));
        // Where it appears, faintly, while it is hidden.
        painter.rect_stroke(pill, h * 0.5, Stroke::new(1.0, s.outline_variant.gamma_multiply(0.6 * (1.0 - k))), egui::StrokeKind::Inside);
        painter.rect_filled(pill, h * 0.5, s.surface_container_highest.gamma_multiply(k));
        bar(&painter, pos2(pill.left() + 12.0, pill.center().y), w - 24.0, k);
    } else {
        // The wheel: a small mouse beside the button.
        let m = Rect::from_center_size(button + vec2(r + 16.0, 0.0), vec2(12.0, 18.0));
        painter.rect_stroke(m, 6.0, Stroke::new(1.5, s.on_surface_variant), egui::StrokeKind::Inside);
        let wy = m.top() + 4.0 + 3.0 * (anim::now(ui.ctx()) as f32 * 4.0).sin().abs();
        painter.line_segment([pos2(m.center().x, wy), pos2(m.center().x, wy + 3.0)], Stroke::new(2.0, s.primary));
    }
}

/// Which way the cover's outline turns: a clock face, the
/// shape turning in it with a bright marker, and an arrow running ahead of
/// the marker the same way. Clockwise goes 12 → 3 → 6.
fn turn_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    use std::f32::consts::TAU;
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    let ccw = prefs.cover_ccw;
    let sign = if ccw { -1.0 } else { 1.0 };
    // Not turning at all: no marker run, no arrow.
    let turning = prefs.cover_spin;
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5 - 18.0;
    let a = if turning { sign * (anim::now(ui.ctx()) as f32 * TAU / 6.0) } else { 0.0 };
    let dir = |th: f32| vec2(th.sin(), -th.cos());
    // Clock hints: 12, 3, 6, 9.
    let font = TypeRole::LabelSmall.style().font_id();
    for (i, n) in ["12", "3", "6", "9"].iter().enumerate() {
        let th = i as f32 * TAU / 4.0;
        painter.text(c + dir(th) * (r + 11.0), egui::Align2::CENTER_CENTER, *n, font.clone(), s.on_surface_variant.gamma_multiply(0.7));
    }
    let shape = match prefs.cover_shape {
        v::CoverShape::Original | v::CoverShape::Random => v::CoverShape::Cookie12,
        other => other,
    };
    if let Some(m) = now_playing::material(shape) {
        let box_ = Rect::from_center_size(c, Vec2::splat((r - 6.0) * 2.0));
        nikgob_shapes::paint::fill_shape(&painter, now_playing::shape_cubics(m), box_, a.rem_euclid(TAU), s.secondary_container);
    }
    // The marker on the turning part, and an arrow ahead of it.
    painter.circle_filled(c + dir(a) * (r - 12.0), 4.5, s.primary);
    if !turning {
        return;
    }
    let ar = r + 1.0;
    let span = 0.9;
    let pts: Vec<Pos2> = (0..=16).map(|k| c + dir(a + sign * (0.25 + span * k as f32 / 16.0)) * ar).collect();
    let stroke = Stroke::new(2.0, s.tertiary);
    painter.add(Shape::line(pts.clone(), stroke));
    // Arrowhead at the end, along the way of the turn.
    let tip = *pts.last().unwrap_or(&c);
    let th = a + sign * (0.25 + span);
    let along = vec2(th.cos(), th.sin()) * sign;
    let out = dir(th);
    painter.add(Shape::convex_polygon(
        vec![tip + along * 6.0, tip - along * 2.0 + out * 4.5, tip - along * 2.0 - out * 4.5],
        s.tertiary,
        Stroke::NONE,
    ));
}

/// Lines of text scrolling up one line at a time; with the cascade the
/// lines further from the sung one follow later, like a wave.
fn cascade_preview(ui: &Ui, rect: Rect, prefs: &Prefs) {
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    const STEP: f32 = 1.4;
    let t = anim::now(ui.ctx()) as f32;
    let (n, f) = ((t / STEP).floor(), (t / STEP).fract() * STEP);
    let line_h = (rect.height() / 5.0).clamp(14.0, 26.0);
    let cur_y = rect.center().y;
    let widths = [0.8, 0.55, 0.7, 0.45, 0.65, 0.75, 0.5, 0.6];
    for k in -3i32..=4 {
        // Line `idx` sits `k` lines below the current one after this step.
        let idx = (n as i32 + k).rem_euclid(widths.len() as i32) as usize;
        let delay = if prefs.lyrics_cascade { (k.unsigned_abs() as f32) * 0.12 } else { 0.0 };
        let x = ((f - delay) / 0.45).clamp(0.0, 1.0);
        let ease = 1.0 - (1.0 - x).powi(3);
        let y = cur_y + (k as f32 + 1.0 - ease) * line_h;
        let current = k == 0;
        let w = (rect.width() - 24.0) * widths[idx];
        let bar = Rect::from_min_size(pos2(rect.left() + 12.0, y - line_h * 0.22), vec2(w, line_h * 0.44));
        let color = if current { s.on_surface } else { s.on_surface.gamma_multiply(0.35) };
        painter.rect_filled(bar, line_h * 0.22, color);
    }
}

/// A made-up 120 BPM beat (1 on the beat, decaying).
pub(crate) fn beat(t: f32) -> f32 {
    (-(t * 2.0).fract() * 6.0).exp()
}
