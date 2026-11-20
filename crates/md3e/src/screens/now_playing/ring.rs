//! The ring around the cover: dots on a circle, dots riding the outline, or
//! bars growing out of it, driven by the beat and the spectrum, and the
//! settings' live sample of it.

use std::f32::consts::TAU;

use egui::{Pos2, Rect, Stroke, Ui, vec2};
use nikgob_view::{CoverShape, RingStyle};

use crate::{
    Theme,
    components::anim,
    screens::stage::Stage,
};

use super::shapes::{BINS, Outline, material, random_pool};

/// Room the ring takes around the cover on each side.
pub(super) const RING_PAD: f32 = 28.0;

/// The spectrum level for direction `theta`: low bands at the top, high at
/// the bottom, mirrored left and right.
fn band_at(bands: &[f32], theta: f32) -> f32 {
    if bands.is_empty() {
        return 0.0;
    }
    let fold = 1.0 - (2.0 * ((theta / TAU).rem_euclid(1.0)) - 1.0).abs();
    let x = fold * (bands.len() - 1) as f32;
    let (i0, f) = (x.floor() as usize, x.fract());
    let i1 = (i0 + 1).min(bands.len() - 1);
    (bands[i0] * (1.0 - f) + bands[i1] * f).clamp(0.0, 1.0)
}

/// What drives the ring this frame.
pub(super) struct RingInput<'a> {
    pub(super) style: RingStyle,
    /// Turn of the ring (radians, either sign).
    pub(super) angle: f32,
    /// Per-band transients (0..1); empty when the ring ignores the music.
    pub(super) transients: &'a [f32],
    /// Bar heights (0..1) for [`RingStyle::Bars`], one per bar; empty = rest.
    pub(super) bars: &'a [f32],
    /// Room for the bars outside the outline.
    pub(super) room: f32,
    pub(super) kick: f32,
    pub(super) alpha: f32,
    /// How far the ring has come in (1) or gone (0): it settles a few
    /// points inward as it fades on a pause.
    pub(super) shown: f32,
    /// The cover's side at rest (no beat's swell): the «Контур» ring's
    /// dot count follows it, never the outline's live length.
    pub(super) base: f32,
    /// Gaps and dot spacing relative to the player's (1); a small sample
    /// shrinks them so each style still reads as itself.
    pub(super) scale: f32,
    /// Where the «Круг» ring keeps its dots' place ([`dot_phase`]).
    pub(super) key: egui::Id,
}

/// The ring around the cover: dots on a circle (around the square, its
/// rounded offset), as many dots riding the outline at a fixed gap, or
/// rounded bars growing out of it. Dots and bars jump out on each beat
/// (`kick`) and where the spectrum spikes.
pub(super) fn ring(ui: &Ui, outline: &Outline, input: &RingInput<'_>) {
    if input.alpha <= 0.01 {
        return;
    }
    let s = Theme::get(ui.ctx()).scheme;
    let painter = ui.painter();
    let center = outline.rect.center();
    let dir = |th: f32| vec2(th.sin(), -th.cos());
    let kick = input.kick;
    let sc = input.scale;
    // Dots shrink less than the gaps, so a sample stays legible.
    let kd = sc.sqrt();
    match input.style {
        RingStyle::Circle => {
            // A circle around a shape; around the square (and every outline
            // on the way between them) the outline's own offset, so no
            // corner covers a dot and the path morphs with the cover. The
            // dots sit evenly along it and keep the circle's speed.
            let gap = (10.0 + 4.0 * input.shown) * sc;
            let circle = outline.rect.width() * 0.5 + gap;
            let path = circle_path(&outline.radii(), circle, gap);
            let pts: Vec<Pos2> = (0..BINS).map(|i| center + dir(i as f32 * TAU / BINS as f32) * path[i]).collect();
            let mut acc = Vec::with_capacity(BINS + 1);
            let mut total = 0.0;
            acc.push(0.0);
            for i in 0..BINS {
                total += pts[i].distance(pts[(i + 1) % BINS]);
                acc.push(total);
            }
            let n = ((72.0 * kd).round() as usize).max(24);
            let spacing = total / n as f32;
            let u = dot_phase(ui.ctx(), input.key, input.angle, circle / spacing.max(1e-3));
            let mut bin = 0;
            for k in 0..n {
                let along = k as f32 + u;
                let d = (along * spacing) % total;
                while bin + 1 < BINS && acc[bin + 1] < d {
                    bin += 1;
                }
                while bin > 0 && acc[bin] > d {
                    bin -= 1;
                }
                let seg = (acc[bin + 1] - acc[bin]).max(1e-3);
                let f = ((d - acc[bin]) / seg).clamp(0.0, 1.0);
                let (a, b) = (pts[bin], pts[(bin + 1) % BINS]);
                let theta = (bin as f32 + f) * TAU / BINS as f32;
                let level = band_at(input.transients, theta);
                let out = dir(theta) * (level * 16.0 + kick * 8.0) * sc;
                let dot = (2.0 + level * 2.0 + kick * 0.8) * kd;
                let alpha = input.alpha * (0.45 + 0.55 * level.max(kick * 0.6));
                painter.circle_filled(a + (b - a) * f + out, dot, s.primary.gamma_multiply(alpha));
            }
        }
        RingStyle::Contour => {
            // The outline pushed out by a gap, then dots spaced evenly along
            // its length (not by angle: a square's corners would thin out),
            // sliding along it as the ring turns.
            let radii = outline.radii();
            let gap = (10.0 + 4.0 * input.shown) * sc;
            let pts: Vec<Pos2> = (0..BINS)
                .map(|i| center + dir(i as f32 * TAU / BINS as f32) * (radii[i] + gap))
                .collect();
            let mut acc = Vec::with_capacity(BINS + 1);
            let mut total = 0.0;
            acc.push(0.0);
            for i in 0..BINS {
                total += pts[i].distance(pts[(i + 1) % BINS]);
                acc.push(total);
            }
            // As many dots as the cover's circle holds at rest: a count that
            // followed the outline's length (a beat's swell, the morph)
            // popped dots in and out.
            let spacing = 11.0 * kd;
            let n = ((TAU * (input.base * 0.5 + 14.0 * sc) / spacing).round() as usize).max(12);
            let phase = (input.angle / TAU).rem_euclid(1.0) * total;
            let mut bin = 0;
            for k in 0..n {
                let d = (phase + k as f32 * total / n as f32) % total;
                while bin + 1 < BINS && acc[bin + 1] < d {
                    bin += 1;
                }
                while bin > 0 && acc[bin] > d {
                    bin -= 1;
                }
                let seg = (acc[bin + 1] - acc[bin]).max(1e-3);
                let f = ((d - acc[bin]) / seg).clamp(0.0, 1.0);
                let (a, b) = (pts[bin], pts[(bin + 1) % BINS]);
                let p = a + (b - a) * f;
                let theta = (bin as f32 + f) * TAU / BINS as f32;
                let level = band_at(input.transients, theta);
                let out = dir(theta) * (level * 14.0 + kick * 7.0) * sc;
                let dot = (2.0 + level * 1.8 + kick * 0.8) * kd;
                let alpha = input.alpha * (0.5 + 0.5 * level.max(kick * 0.6));
                painter.circle_filled(p + out, dot, s.primary.gamma_multiply(alpha));
            }
        }
        RingStyle::Bars => {
            // caelestia's cover visualiser: bars on fixed angles (they do not
            // turn with the ring), each starting a gap off the turning
            // outline, so their base breathes with it. In silence they are
            // round dots; with the music they grow over all the room around
            // the cover.
            let radii = outline.radii();
            let n = BAR_COUNT;
            let gap = (6.0 + 2.0 * input.shown) * sc;
            let r_mid = outline.rect.width() * 0.5 + gap;
            let width = (TAU * r_mid / n as f32 - 3.0).clamp(2.5, 7.0);
            let room = (input.room - gap - width).max(4.0);
            let color = s.primary.gamma_multiply(input.alpha);
            let stroke = Stroke::new(width, color);
            for i in 0..n {
                let theta = i as f32 / n as f32 * TAU;
                let bin = (i * BINS / n) % BINS;
                let value = input.bars.get(i).copied().unwrap_or(0.0).clamp(0.01, 1.0);
                let d = dir(theta);
                let a = center + d * (radii[bin] + gap + width * 0.5);
                let b = a + d * value * room;
                if value * room > 0.5 {
                    painter.line_segment([a, b], stroke);
                    painter.circle_filled(b, width * 0.5, color);
                }
                painter.circle_filled(a, width * 0.5, color);
            }
        }
    }
}

/// The «Круг» ring's path, one radius per [`BINS`] direction: the outline
/// (`radii`) pushed out by `gap`, never inside `circle` (the ring around a
/// shape). Around a shape that is the circle itself, around the square the
/// rounded square's offset (no farther out at the sides than the circle, so
/// it fits where the circle does), and in a morph between them each
/// direction goes smoothly from one to the other.
pub(super) fn circle_path(radii: &[f32], circle: f32, gap: f32) -> Vec<f32> {
    radii.iter().map(|r| (r + gap).max(circle)).collect()
}

/// Where the «Круг» ring's dots stand, as a fraction of their spacing: the
/// dots are alike, so moving by whole ones shows nothing. It goes on by the
/// ring's turn since the last frame times `per_radian` (dots per radian on
/// the circle), so a dot runs as fast as it would on the circle along a
/// longer path too, and a path that changes length never makes them jump.
pub(super) fn dot_phase(ctx: &egui::Context, key: egui::Id, angle: f32, per_radian: f32) -> f32 {
    use std::f32::consts::PI;
    let (last, u): (f32, f32) = ctx.data(|d| d.get_temp(key)).unwrap_or((angle, 0.0));
    let step = (angle - last + PI).rem_euclid(TAU) - PI;
    let u = (u + step * per_radian).rem_euclid(1.0);
    ctx.data_mut(|d| d.insert_temp(key, (angle, u)));
    u
}

/// An angle that turns one revolution per `period` seconds (faster with
/// the bass, backwards when `ccw`) only while playing: a pause brakes it
/// in a fraction of a second where it is, and play goes on from there.
/// `period` 0 = still.
pub(super) fn turn(ui: &Ui, id: egui::Id, playing: bool, bass: f32, period: f32, ccw: bool) -> f32 {
    let ctx = ui.ctx();
    let now = anim::now(ctx);
    // Not the cover morph: that one takes its time, and the ring would
    // keep turning through it after a pause.
    let go = anim::fraction(ctx, id.with("go"), if playing { 1.0 } else { 0.0 }, Theme::get(ctx).motion.fast_effects);
    let (angle, last): (f32, f64) = ctx.data(|d| d.get_temp(id)).unwrap_or((0.0, now));
    let dt = (now - last).clamp(0.0, 0.1) as f32;
    let speed = if period > 0.0 { TAU / period * (1.0 + bass * 0.8) } else { 0.0 };
    let sign = if ccw { -1.0 } else { 1.0 };
    let angle = (angle + sign * dt * speed * go.clamp(0.0, 1.0)).rem_euclid(TAU);
    ctx.data_mut(|d| d.insert_temp(id, (angle, now)));
    angle
}

/// Beat follower over the analyzer's smoothed levels. Those sit high for
/// any loud track, so what moves is how far they jump above their own
/// recent average.
#[derive(Clone, Default)]
struct Pulse {
    last: f64,
    bass_avg: f32,
    kick: f32,
    band_avg: Vec<f32>,
    bands: Vec<f32>,
}

/// One step of the [`Pulse`]: the kick (0..1, sharp attack, ~0.15 s decay)
/// and per-band transients (0..1). Not `live` (a pause), nothing new
/// comes: what jumped falls back at its own pace.
pub(super) fn pulse(ui: &Ui, id: egui::Id, bass: f32, bands: &[f32], live: bool) -> (f32, Vec<f32>) {
    let ctx = ui.ctx();
    let now = anim::now(ctx);
    let mut p: Pulse = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let dt = if p.last == 0.0 { 1.0 / 60.0 } else { (now - p.last).clamp(0.0, 0.1) as f32 };
    p.last = now;
    let ease = |tau: f32| 1.0 - (-dt / tau).exp();

    let bass = if live { bass } else { 0.0 };
    p.bass_avg += (bass - p.bass_avg) * ease(0.6);
    let hit = ((bass - p.bass_avg - 0.03) * 5.0).clamp(0.0, 1.0);
    p.kick = hit.max(p.kick * (-dt / 0.15).exp());

    if live && p.band_avg.len() != bands.len() {
        p.band_avg = bands.to_vec();
        p.bands = vec![0.0; bands.len()];
    }
    for (i, (avg, out)) in p.band_avg.iter_mut().zip(&mut p.bands).enumerate() {
        let b = if live { bands.get(i).copied().unwrap_or(0.0) } else { 0.0 };
        *avg += (b - *avg) * ease(0.45);
        let target = ((b - *avg) * 4.0 + b * 0.2).clamp(0.0, 1.0);
        *out = target.max(*out * (-dt / 0.12).exp());
    }
    let out = (p.kick, p.bands.clone());
    ctx.data_mut(|d| d.insert_temp(id, p));
    out
}

/// Bars around the cover in [`RingStyle::Bars`].
pub(super) const BAR_COUNT: usize = 48;

/// cava-style processing of the analyzer's bands for the bars: mirrored
/// left and right (the lows at the top, the highs at the bottom, so no
/// seam where the circle closes), automatic sensitivity (normalised between
/// a slow floor and a peak that falls back slowly), neighbour smoothing
/// ("monstercat") and gravity on the way down.
#[derive(Clone, Default)]
pub(super) struct Cava {
    last: f64,
    peak: f32,
    floor: f32,
    shown: Vec<f32>,
    fall: Vec<f32>,
}

/// One step of the [`Cava`]: [`BAR_COUNT`] heights (0..1). `active` false
/// lets the bars fall to rest.
pub(super) fn cava(ui: &Ui, stage: &Stage, bands: &[f32], active: bool) -> Vec<f32> {
    const GRAVITY: f32 = 3.2;
    let ctx = ui.ctx();
    let id = stage.id("np-cava");
    let now = anim::now(ctx);
    let mut c: Cava = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let dt = if c.last == 0.0 { 1.0 / 60.0 } else { (now - c.last).clamp(0.0, 0.1) as f32 };
    c.last = now;
    let ease = |tau: f32| 1.0 - (-dt / tau).exp();
    if c.shown.len() != BAR_COUNT {
        c.shown = vec![0.0; BAR_COUNT];
        c.fall = vec![0.0; BAR_COUNT];
    }
    let target = cava_targets(&mut c, bands, active, ease(2.5), ease(1.5));
    for ((shown, fall), t) in c.shown.iter_mut().zip(&mut c.fall).zip(target) {
        if t >= *shown {
            *shown = t;
            *fall = 0.0;
        } else {
            *fall += GRAVITY * dt;
            *shown = (*shown - *fall * dt).max(t);
        }
    }
    let out = c.shown.clone();
    if out.iter().any(|v| *v > 0.011) {
        stage.repaint(ctx);
    }
    ctx.data_mut(|d| d.insert_temp(id, c));
    out
}

/// The bars' targets this frame: `bands` resampled to [`BAR_COUNT`],
/// mirrored (bar `i` and bar `BAR_COUNT - i` are one), each bar the
/// loudest band of its span, auto-scaled and smoothed. `peak_k` /
/// `floor_k` are this frame's easing fractions for the sensitivity
/// envelopes.
pub(super) fn cava_targets(c: &mut Cava, bands: &[f32], active: bool, peak_k: f32, floor_k: f32) -> Vec<f32> {
    const HALF: usize = BAR_COUNT / 2;
    let raw: Vec<f32> = (0..BAR_COUNT)
        .map(|i| {
            if !active || bands.is_empty() {
                return 0.0;
            }
            let k = if i <= HALF { i } else { BAR_COUNT - i } as f32;
            let last = (bands.len() - 1) as f32;
            let from = ((k - 0.5).max(0.0) / HALF as f32 * last).round() as usize;
            let to = ((k + 0.5).min(HALF as f32) / HALF as f32 * last).round() as usize;
            bands[from..=to.max(from)].iter().copied().fold(0.0, f32::max)
        })
        .collect();
    let max = raw.iter().copied().fold(0.0, f32::max);
    let mean = raw.iter().sum::<f32>() / BAR_COUNT as f32;
    c.peak = if max > c.peak { max } else { c.peak + (max - c.peak) * peak_k };
    c.floor += (mean - c.floor) * floor_k;
    let lo = c.floor * 0.9;
    let span = (c.peak - lo).max(0.12);
    let mut target: Vec<f32> = raw
        .iter()
        .map(|&v| if max < 0.05 { 0.0 } else { ((v - lo) / span).clamp(0.0, 1.0).powf(1.3) })
        .collect();
    // Monstercat: a bar lifts its neighbours, less the further they are.
    for i in 0..BAR_COUNT {
        let v = target[i];
        let mut k = v;
        for t in target.iter_mut().skip(i + 1) {
            k /= 1.5;
            if k < 0.01 {
                break;
            }
            *t = t.max(k);
        }
        let mut k = v;
        for t in target[..i].iter_mut().rev() {
            k /= 1.5;
            if k < 0.01 {
                break;
            }
            *t = t.max(k);
        }
    }
    target
}

/// The ring is there (`ring`, «Кольцо») only around a shaped cover: the
/// square one keeps its own frame.
pub(crate) fn ring_on(ring: bool, shape: CoverShape) -> bool {
    ring && shape != CoverShape::Original
}

/// The ring shows only while the music plays and only around a shaped
/// cover ([`ring_on`]): on a pause it fades out, and nothing of it stays.
pub(crate) fn ring_shown(ring: bool, shape: CoverShape, playing: bool) -> bool {
    playing && ring_on(ring, shape)
}

/// A small live sample of the cover under `prefs` for the settings' hover
/// card: its shape (going through the pool for «Случайная»), spin, beat
/// pulse and, with `with_ring`, the ring, all on a made-up beat. It keeps
/// its own clock, so the real player's angles are untouched.
pub(crate) fn preview(ui: &Ui, rect: Rect, prefs: &nikgob_view::Prefs, with_ring: bool) {
    let motion = !Theme::get(ui.ctx()).reduced_motion;
    let t = anim::now(ui.ctx()) as f32;
    let kick = if motion { crate::screens::pages::settings_help::beat(t) } else { 0.0 };
    let sign = |ccw: bool| if ccw { -1.0 } else { 1.0 };
    // Four times faster than in the player, so a small sample shows it.
    let period = (prefs.cover_spin_s / 4.0).max(2.0);
    let spin = if prefs.cover_spin && motion { t * TAU / period * sign(prefs.cover_ccw) } else { 0.0 };
    let swell = if prefs.cover_pulse { 1.0 + prefs.cover_pulse_strength * kick } else { 1.0 };
    let pad = if with_ring { RING_PAD * 0.8 } else { 8.0 };
    let side = (rect.width().min(rect.height()) - 2.0 * pad).max(16.0);
    let cover = Rect::from_center_size(rect.center(), vec2(side, side) * swell);
    let outline = match prefs.cover_shape {
        CoverShape::Random => {
            let pool = random_pool(&prefs.random_shapes);
            let step = (t / 1.6).floor() as usize;
            let (from, to) = (pool[step.wrapping_sub(1) % pool.len()], pool[step % pool.len()]);
            let k = ((t / 1.6).fract() / 0.35).clamp(0.0, 1.0);
            match (material(from), material(to)) {
                (Some(a), Some(b)) if k < 1.0 && a != b => Outline::swap(a, b, k * k * (3.0 - 2.0 * k), cover, spin),
                (_, b) => Outline::new(b, 1.0, cover, spin),
            }
        }
        c => Outline::new(material(c), 1.0, cover, spin),
    };
    outline.paint(ui, None);
    if with_ring && ring_on(true, prefs.cover_shape) {
        let angle = if motion && prefs.ring_style != RingStyle::Bars {
            t * TAU / (nikgob_view::RING_SPIN_S / 3.6).max(2.0) * sign(!prefs.cover_ccw)
        } else {
            0.0
        };
        let beat = motion;
        let transients: Vec<f32> = if beat {
            (0..12).map(|i| kick * (0.5 + 0.5 * (i as f32 * 1.3 + t * 3.0).sin()).powi(2)).collect()
        } else {
            Vec::new()
        };
        let bars: Vec<f32> = if beat {
            (0..BAR_COUNT)
                .map(|i| {
                    let x = i as f32 / BAR_COUNT as f32 * TAU;
                    (0.25 + 0.45 * kick) * (0.55 + 0.45 * (x * 3.0 + t * 2.0).sin()) * (0.6 + 0.4 * (x * 7.0 - t).cos())
                })
                .collect()
        } else {
            Vec::new()
        };
        let input = RingInput {
            style: prefs.ring_style,
            angle,
            transients: &transients,
            bars: &bars,
            room: pad,
            kick: if beat { kick } else { 0.0 },
            alpha: 1.0,
            shown: 1.0,
            base: side,
            // The player's cover is ~300 px: at sample size the gaps would
            // swamp the shape.
            scale: (side / 300.0).clamp(0.3, 1.0),
            key: egui::Id::new("np-ring-dots-sample"),
        };
        ring(ui, &outline, &input);
    }
}
