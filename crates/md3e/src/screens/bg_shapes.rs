//! Material shapes behind the open track (after caelestia's
//! `BackgroundShapes`), following the music in one of three moods:
//!
//! - **Bounce**: they lie on the floor and jump on the beats of their band,
//!   squashing a little as they land.
//! - **Float**: they drift and turn slowly, faster with the bass, and glow
//!   when their band is loud.
//! - **Morph**: they drift and turn into other shapes on the beats.
//!
//! The simulation is stepped by real time and kept in the egui context, so
//! reopening Now Playing continues where it was; nothing moves while paused
//! (bouncing shapes still land).

use std::borrow::Cow;
use std::f32::consts::TAU;

use egui::{Color32, Rect, Shape, Ui, pos2, vec2};
use nikgob_shapes::{MaterialShape, paint as shape_paint};
use nikgob_view::{BgShapes, Prefs};

use super::now_playing::{pair_morph, shape_cubics};
use crate::{Theme, components::anim};

/// The shapes they are drawn from (caelestia's pool plus a few softer ones).
const POOL: [MaterialShape; 21] = [
    MaterialShape::Circle,
    MaterialShape::Cookie4Sided,
    MaterialShape::Cookie6Sided,
    MaterialShape::Cookie7Sided,
    MaterialShape::Cookie9Sided,
    MaterialShape::Cookie12Sided,
    MaterialShape::Sunny,
    MaterialShape::VerySunny,
    MaterialShape::SoftBurst,
    MaterialShape::Pentagon,
    MaterialShape::Gem,
    MaterialShape::Arch,
    MaterialShape::Arrow,
    MaterialShape::Pill,
    MaterialShape::Triangle,
    MaterialShape::Fan,
    MaterialShape::Oval,
    MaterialShape::Clover4Leaf,
    MaterialShape::Flower,
    MaterialShape::Puffy,
    MaterialShape::Heart,
];

/// The shapes the background draws from.
#[cfg(test)]
pub(crate) fn pool() -> &'static [MaterialShape] {
    &POOL
}

/// Smallest and largest shape (px at a 900 px tall window, before the
/// user's size).
const MIN_SIZE: f32 = 36.0;
const MAX_SIZE: f32 = 124.0;
/// Seconds a shape takes to turn into another.
const MORPH_S: f32 = 0.55;

#[derive(Clone, Debug)]
struct Body {
    /// Centre, relative to the field's top-left.
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// Degrees and degrees per second.
    rot: f32,
    vr: f32,
    /// Size before the field's scale.
    size: f32,
    shape: usize,
    /// The shape it turns from while `morph` < 1.
    from: usize,
    morph: f32,
    color: usize,
    /// The spectrum band it listens to.
    band: f32,
    glow: f32,
    /// Landing squash (decays to 0).
    squash: f32,
    /// Seconds before it may react again.
    cooldown: f32,
}

#[derive(Clone, Debug, Default)]
struct Field {
    mode: Option<BgShapes>,
    count: usize,
    size: egui::Vec2,
    last: f64,
    rng: u64,
    bodies: Vec<Body>,
    /// Jump height factor (0 = as in the player).
    jump: f32,
}

impl Field {
    fn rand(&mut self) -> f32 {
        // xorshift64*: plenty for scattering shapes.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.rand() * (hi - lo)
    }

    fn signed(&mut self, lo: f32, hi: f32) -> f32 {
        let v = self.range(lo, hi);
        if self.rand() < 0.5 { -v } else { v }
    }

    fn pick(&mut self, n: usize) -> usize {
        ((self.rand() * n as f32) as usize).min(n - 1)
    }

    /// Fresh shapes scattered over `w` × `h`.
    fn reset(&mut self, mode: BgShapes, count: usize, w: f32, h: f32) {
        self.mode = Some(mode);
        self.count = count;
        self.bodies.clear();
        for i in 0..count {
            let size = MIN_SIZE + i as f32 / count.max(1) as f32 * (MAX_SIZE - MIN_SIZE);
            let shape = self.pick(POOL.len());
            let body = Body {
                x: self.range(0.0, w),
                y: if mode == BgShapes::Bounce { self.range(0.0, h * 0.6) } else { self.range(0.0, h) },
                vx: self.signed(4.0, 18.0),
                vy: self.signed(4.0, 18.0),
                rot: self.range(0.0, 360.0),
                vr: self.range(-12.0, 12.0),
                size,
                shape,
                from: shape,
                morph: 1.0,
                color: self.pick(4),
                // Spread over the lower two thirds of the spectrum, where
                // the beats are.
                band: self.range(0.0, 0.66),
                glow: 0.0,
                squash: 0.0,
                cooldown: self.range(0.0, 0.4),
            };
            self.bodies.push(body);
        }
    }
}

/// The level of `band` (0..1 across the spectrum) in `levels`.
fn level(levels: &[f32], band: f32) -> f32 {
    if levels.is_empty() {
        return 0.0;
    }
    let x = band.clamp(0.0, 1.0) * (levels.len() - 1) as f32;
    let (i0, f) = (x.floor() as usize, x.fract());
    let i1 = (i0 + 1).min(levels.len() - 1);
    (levels[i0] * (1.0 - f) + levels[i1] * f).clamp(0.0, 1.0)
}

/// Paints the shapes into `rect` under the current prefs, their field kept
/// under `id` (`"np-bg-shapes"` for the real screen, its own for the
/// settings' stage). `kick` (0..1) is the beat, `transients` the per-band
/// jumps, `bass` the smoothed bass.
#[allow(clippy::too_many_arguments)]
pub fn paint(ui: &Ui, id: egui::Id, rect: Rect, prefs: &Prefs, playing: bool, kick: f32, transients: &[f32], bass: f32) {
    let scale = (rect.height() / 900.0).clamp(0.6, 1.6);
    field(ui, id, rect, prefs, Look { scale, vis: 1.0, prewarm: false, jump: 1.0 }, playing, kick, transients, bass);
}

/// A small sample for the settings' cards, on its own field (one per
/// `salt`, so samples side by side keep their own shapes) so the player's
/// shapes stay where they are.
///
/// A sample is a few hundred px, not a window: fewer, larger and much more
/// visible shapes (the user's count, size and brightness still scale
/// them), and a fresh field starts settled, so bouncing ones lie on the
/// floor at once.
pub(crate) fn preview(ui: &Ui, rect: Rect, prefs: &Prefs, kick: f32, transients: &[f32], salt: egui::Id) {
    let prefs = Prefs { bg_shapes_count: (prefs.bg_shapes_count / 2).max(4), ..prefs.clone() };
    // Jumps sized to the sample, so they hop instead of leaving it.
    let look = Look { scale: rect.height() / 380.0, vis: 3.2, prewarm: true, jump: 0.55 };
    field(ui, egui::Id::new("settings-bg-shapes").with(salt), rect, &prefs, look, true, kick, transients, kick * 0.5);
}

/// How a field is drawn: shape size, an opacity multiplier and whether a
/// fresh field is run until it settles first.
#[derive(Clone, Copy, Debug)]
struct Look {
    scale: f32,
    vis: f32,
    prewarm: bool,
    jump: f32,
}

/// The hues the user can give the shapes (HCT degrees, with their names):
/// red, orange, yellow, green, teal, blue, violet, pink.
pub(crate) const HUES: [(f64, &str); 8] = [
    (25.0, "Красный"),
    (55.0, "Оранжевый"),
    (95.0, "Жёлтый"),
    (145.0, "Зелёный"),
    (195.0, "Бирюзовый"),
    (260.0, "Синий"),
    (300.0, "Фиолетовый"),
    (345.0, "Розовый"),
];

/// Swatch `i` of [`HUES`] in the tones of `s`: the primary's lightness, so
/// it reads like an accent of the current theme.
pub(crate) fn hue_color(s: &crate::ColorScheme, i: u8) -> Color32 {
    let (hue, _) = HUES[usize::from(i).min(HUES.len() - 1)];
    crate::color::hue_like(s.primary, hue, 40.0)
}

/// [`paint`] with the field kept under `id`, shapes sized by `scale`.
#[allow(clippy::too_many_arguments)]
fn field(ui: &Ui, id: egui::Id, rect: Rect, prefs: &Prefs, look: Look, playing: bool, kick: f32, transients: &[f32], bass: f32) {
    let scale = look.scale;
    let mode = prefs.bg_shapes;
    if mode == BgShapes::Off || !rect.is_positive() {
        return;
    }
    let ctx = ui.ctx();
    let theme = Theme::get(ctx);
    let count = prefs.bg_shapes_count.clamp(4, 32) as usize;
    let mut f: Field = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if f.rng == 0 {
        f.rng = 0x9E37_79B9_7F4A_7C15;
    }
    let (w, h) = (rect.width(), rect.height());
    f.jump = look.jump;
    if f.mode != Some(mode) || f.count != count || f.bodies.is_empty() {
        f.reset(mode, count, w, h);
        f.size = rect.size();
        if look.prewarm {
            // Two quiet seconds: bouncing shapes land, the others spread.
            let s = scale * prefs.bg_shapes_size.clamp(0.5, 2.0);
            for _ in 0..120 {
                step(&mut f, mode, 1.0 / 60.0, s, false, 0.0, &[], 0.0, 1.0, w, h);
            }
        }
    } else if f.size != rect.size() {
        // Keep the picture when the window resizes.
        let (sx, sy) = (w / f.size.x.max(1.0), h / f.size.y.max(1.0));
        for b in &mut f.bodies {
            b.x *= sx;
            b.y *= sy;
        }
        f.size = rect.size();
    }

    let now = anim::now(ctx);
    let dt = if f.last == 0.0 { 1.0 / 60.0 } else { (now - f.last).clamp(0.0, 0.05) as f32 };
    f.last = now;
    let scale = scale * prefs.bg_shapes_size.clamp(0.5, 2.0);
    let react = prefs.bg_shapes_react.clamp(0.0, 2.0);
    let speed = prefs.bg_shapes_speed.clamp(0.25, 3.0);
    let moving = !theme.reduced_motion && (playing || mode == BgShapes::Bounce);
    let mut busy = false;
    if moving {
        busy = step(&mut f, mode, dt * speed, scale, playing, kick, transients, bass, react, w, h);
    } else if mode == BgShapes::Bounce {
        // Nothing falls without motion: they lie where they would land.
        for b in &mut f.bodies {
            b.y = h - b.size * scale * 0.5;
        }
    }
    draw(ui, rect, &f, mode, prefs, scale, look.vis);
    ctx.data_mut(|d| d.insert_temp(id, f));
    if busy || (moving && playing) {
        ctx.request_repaint();
    }
}

/// Advances the shapes by `dt`; true while anything still moves.
#[allow(clippy::too_many_arguments)]
fn step(
    f: &mut Field,
    mode: BgShapes,
    dt: f32,
    scale: f32,
    playing: bool,
    kick: f32,
    transients: &[f32],
    bass: f32,
    react: f32,
    w: f32,
    h: f32,
) -> bool {
    const GRAVITY: f32 = 2400.0;
    let mut busy = false;
    for i in 0..f.bodies.len() {
        let mut b = f.bodies[i].clone();
        let r = b.size * scale * 0.5;
        let lvl = if playing { level(transients, b.band) } else { 0.0 };
        b.cooldown = (b.cooldown - dt).max(0.0);
        b.squash *= (-dt / 0.12).exp();
        match mode {
            BgShapes::Bounce => {
                let floor = h - r;
                let grounded = b.y >= floor - 0.5 && b.vy.abs() < 1.0;
                if grounded {
                    b.y = floor;
                    b.x += b.vx * dt;
                    b.vx *= (-dt / 0.15).exp();
                    b.vr *= (-dt / 0.1).exp();
                    // Lie flat-ish: settle the turn towards the nearest
                    // quarter.
                    let target = (b.rot / 90.0).round() * 90.0;
                    b.rot += (target - b.rot) * (1.0 - (-dt / 0.2).exp());
                    // The kick lifts the bass shapes most; the others
                    // mostly follow their own band.
                    let hit = lvl.max(kick * 0.7 * (1.0 - b.band)) * react;
                    if playing && b.cooldown <= 0.0 && hit > 0.35 {
                        // Small ones jump higher.
                        let lift = (MAX_SIZE / b.size).sqrt();
                        let jump = if f.jump > 0.0 { f.jump } else { 1.0 };
                        b.vy = -(380.0 + 900.0 * hit.min(1.5)) * lift * scale.sqrt() * jump;
                        let jitter = f.signed(20.0, 120.0);
                        b.vx += jitter;
                        b.vr = f.signed(60.0, 260.0);
                        b.cooldown = 0.18;
                    }
                } else {
                    b.vy += GRAVITY * scale.sqrt() * dt;
                    b.x += b.vx * dt;
                    b.y += b.vy * dt;
                    b.rot += b.vr * dt;
                    if b.y >= floor {
                        b.y = floor;
                        let impact = (b.vy / 900.0).clamp(0.0, 1.0);
                        b.squash = b.squash.max(impact);
                        // A small bounce, then rest.
                        b.vy = if b.vy > 260.0 { -b.vy * 0.28 } else { 0.0 };
                    }
                    busy = true;
                }
                if b.x < r {
                    b.x = r;
                    b.vx = b.vx.abs() * 0.6;
                } else if b.x > w - r {
                    b.x = w - r;
                    b.vx = -b.vx.abs() * 0.6;
                }
                b.glow = lvl;
                busy |= b.squash > 0.01 || b.vx.abs() > 1.0;
            }
            BgShapes::Float | BgShapes::Morph => {
                let calm = if mode == BgShapes::Morph { 0.6 } else { 1.0 };
                let push = calm * (1.0 + bass * react * 2.0);
                b.x += b.vx * push * dt * scale;
                b.y += b.vy * push * dt * scale;
                b.rot += b.vr * push * dt;
                // Wrap around the edges, like caelestia.
                if b.x < -r {
                    b.x = w + r;
                } else if b.x > w + r {
                    b.x = -r;
                }
                if b.y < -r {
                    b.y = h + r;
                } else if b.y > h + r {
                    b.y = -r;
                }
                let target = (lvl * react).min(1.0);
                b.glow = if target > b.glow { target } else { b.glow * (-dt / 0.35).exp() };
                if mode == BgShapes::Morph {
                    if b.morph < 1.0 {
                        b.morph = (b.morph + dt / MORPH_S).min(1.0);
                        busy = true;
                    } else if playing && b.cooldown <= 0.0 && (lvl.max(kick * 0.8) * react) > 0.55 {
                        // Not everyone at once: a third of the hits pass.
                        if f.rand() < 0.35 {
                            let mut next = f.pick(POOL.len());
                            if next == b.shape {
                                next = (next + 1) % POOL.len();
                            }
                            b.from = b.shape;
                            b.shape = next;
                            b.morph = 0.0;
                            b.vr += f.signed(20.0, 60.0);
                            b.cooldown = 0.9;
                        } else {
                            b.cooldown = 0.25;
                        }
                    }
                    b.vr *= (-dt / 3.0).exp();
                    if b.vr.abs() < 4.0 {
                        b.vr = 4.0f32.copysign(b.vr);
                    }
                }
            }
            BgShapes::Off => {}
        }
        f.bodies[i] = b;
    }
    busy
}

#[allow(clippy::too_many_arguments)]
fn draw(ui: &Ui, rect: Rect, f: &Field, mode: BgShapes, prefs: &Prefs, scale: f32, vis: f32) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    // On the settings' stage the feather follows its scale.
    let opts = shape_paint::FillOptions::for_ppp(super::stage::ppp(ui.ctx()));
    let opacity = prefs.bg_shapes_opacity.clamp(0.25, 2.0);
    let (colors, alphas): ([Color32; 4], [f32; 4]) = match prefs.bg_shapes_colors {
        1 => ([s.primary; 4], if theme.dark { [0.10, 0.07, 0.05, 0.12] } else { [0.16, 0.11, 0.08, 0.2] }),
        2 => (
            [s.primary, s.secondary, s.tertiary, s.primary],
            if theme.dark { [0.12, 0.10, 0.12, 0.08] } else { [0.18, 0.14, 0.18, 0.12] },
        ),
        3 => {
            // The user's hue: as an accent and as a container, plus its two
            // neighbours, so the field is not flat.
            let (hue, _) = HUES[usize::from(prefs.bg_shapes_hue).min(HUES.len() - 1)];
            let like = |c: Color32, h: f64| crate::color::hue_like(c, hue + h, 36.0);
            (
                [like(s.primary, 0.0), like(s.primary_container, 0.0), like(s.primary, 22.0), like(s.primary_container, -22.0)],
                if theme.dark { [0.13, 0.22, 0.10, 0.18] } else { [0.18, 0.32, 0.14, 0.26] },
            )
        }
        _ => (
            [s.primary_container, s.secondary_container, s.tertiary_container, s.outline_variant],
            if theme.dark { [0.20, 0.18, 0.16, 0.16] } else { [0.34, 0.30, 0.24, 0.2] },
        ),
    };
    for b in &f.bodies {
        let size = b.size * scale;
        let swell = match mode {
            BgShapes::Morph => 1.0 + 0.12 * b.glow,
            BgShapes::Float => 1.0 + 0.05 * b.glow,
            _ => 1.0,
        };
        // Squash: wider and lower on landing, from the floor up.
        let (sx, sy) = (1.0 + 0.22 * b.squash, 1.0 - 0.22 * b.squash);
        let (bw, bh) = (size * swell * sx, size * swell * sy);
        let center = pos2(rect.left() + b.x, rect.top() + b.y + (size - bh) * 0.5);
        let shape_rect = Rect::from_center_size(center, vec2(bw, bh));
        let cubics: Cow<'_, [_]> = if b.morph < 1.0 {
            // Ease out with a touch of overshoot.
            let x = b.morph;
            let t = 1.0 - (-6.0 * x).exp() * (7.0 * x).cos();
            Cow::Owned(pair_morph(POOL[b.from], POOL[b.shape]).as_cubics(t.clamp(0.0, 1.1)))
        } else {
            Cow::Borrowed(shape_cubics(POOL[b.shape]))
        };
        let rot = b.rot.to_radians().rem_euclid(TAU);
        let base = (alphas[b.color] * opacity * vis).min(1.0);
        let color = colors[b.color];
        if mode == BgShapes::Float && b.glow > 0.02 {
            // A soft halo: the same shape, larger and fainter, twice.
            for (grow, a) in [(1.55, 0.18), (1.25, 0.32)] {
                let halo = Rect::from_center_size(center, shape_rect.size() * (1.0 + (grow - 1.0) * b.glow));
                let mesh = shape_paint::fill_mesh(&cubics, halo, rot, color.gamma_multiply((a * b.glow * opacity.min(1.5) * vis).min(1.0)), &opts);
                if !mesh.is_empty() {
                    painter.add(Shape::mesh(mesh));
                }
            }
        }
        let lit = match mode {
            BgShapes::Float => base * (1.0 + 1.6 * b.glow),
            BgShapes::Bounce | BgShapes::Morph => base * (1.0 + 0.6 * b.glow),
            BgShapes::Off => base,
        };
        let mesh = shape_paint::fill_mesh(&cubics, shape_rect, rot, color.gamma_multiply(lit.min(1.0)), &opts);
        if !mesh.is_empty() {
            painter.add(Shape::mesh(mesh));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bouncing_shapes_land_and_rest() {
        let mut f = Field { rng: 7, ..Default::default() };
        f.reset(BgShapes::Bounce, 10, 800.0, 600.0);
        for _ in 0..600 {
            step(&mut f, BgShapes::Bounce, 1.0 / 60.0, 1.0, false, 0.0, &[], 0.0, 1.0, 800.0, 600.0);
        }
        for b in &f.bodies {
            let r = b.size * 0.5;
            assert!((b.y - (600.0 - r)).abs() < 1.0, "{b:?}");
            assert!(b.x >= r - 0.01 && b.x <= 800.0 - r + 0.01);
        }
        assert!(!step(&mut f, BgShapes::Bounce, 1.0 / 60.0, 1.0, false, 0.0, &[], 0.0, 1.0, 800.0, 600.0));
    }

    #[test]
    fn beats_make_them_jump_and_morph() {
        let mut f = Field { rng: 11, ..Default::default() };
        f.reset(BgShapes::Bounce, 8, 800.0, 600.0);
        for _ in 0..300 {
            step(&mut f, BgShapes::Bounce, 1.0 / 60.0, 1.0, false, 0.0, &[], 0.0, 1.0, 800.0, 600.0);
        }
        let loud = vec![1.0; 64];
        step(&mut f, BgShapes::Bounce, 1.0 / 60.0, 1.0, true, 1.0, &loud, 1.0, 1.0, 800.0, 600.0);
        assert!(f.bodies.iter().any(|b| b.vy < -100.0), "nobody jumped");

        let mut f = Field { rng: 3, ..Default::default() };
        f.reset(BgShapes::Morph, 12, 800.0, 600.0);
        let before: Vec<usize> = f.bodies.iter().map(|b| b.shape).collect();
        for _ in 0..240 {
            step(&mut f, BgShapes::Morph, 1.0 / 60.0, 1.0, true, 1.0, &loud, 1.0, 1.0, 800.0, 600.0);
        }
        let after: Vec<usize> = f.bodies.iter().map(|b| b.shape).collect();
        assert_ne!(before, after, "no shape changed");
    }
}
