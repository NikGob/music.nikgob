//! Now Playing (md3e-player.md §3.5): morphing cover inside a dotted ring
//! that turns and follows the spectrum, wavy progress, the hero play button
//! in a standard button group, one content slot, and the ambient
//! background (soft colour clouds following the bass).
//!
//! Wide windows put the cover on the left and the text and controls beside
//! it; narrow ones stack them. The content is laid out for the full window
//! even while the container grows out of the floating player, so nothing
//! reflows during the transition (the shell scales and fades it instead).

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use egui::{Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_shapes::{CornerRounding, Cubic, MaterialShape, Morph, Point, RoundedPolygon, paint as shape_paint};
use nikgob_motion::Animated;
use nikgob_view::{Command, CoverShape, LikeTarget, NpSlot, Repeat, RingStyle, Status, TrackRow, ViewState};

use super::{
    chrome::{self, Act, Chrome, WheelZone, Where},
    common,
    pages::{PageState, sound},
    pane,
    stage::{Part, Stage},
};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        ButtonGroup, IconButton, Item, Size, Slider, Style, anim,
        button::{Colors, Shape as BtnShape},
        paint::{self, Radii},
    },
    size_class,
};

/// The shape a [`CoverShape`] stands for; `None` keeps the square cover.
pub(crate) fn material(shape: CoverShape) -> Option<MaterialShape> {
    Some(match shape {
        CoverShape::Original | CoverShape::Random => return None,
        CoverShape::Cookie12 => MaterialShape::Cookie12Sided,
        CoverShape::Cookie9 => MaterialShape::Cookie9Sided,
        CoverShape::Cookie7 => MaterialShape::Cookie7Sided,
        CoverShape::Cookie6 => MaterialShape::Cookie6Sided,
        CoverShape::Cookie4 => MaterialShape::Cookie4Sided,
        CoverShape::Sunny => MaterialShape::Sunny,
        CoverShape::VerySunny => MaterialShape::VerySunny,
        CoverShape::Flower => MaterialShape::Flower,
        CoverShape::Clover4 => MaterialShape::Clover4Leaf,
        CoverShape::Clover8 => MaterialShape::Clover8Leaf,
        CoverShape::SoftBurst => MaterialShape::SoftBurst,
        CoverShape::SoftBoom => MaterialShape::SoftBoom,
        CoverShape::Puffy => MaterialShape::Puffy,
        CoverShape::PuffyDiamond => MaterialShape::PuffyDiamond,
        CoverShape::Circle => MaterialShape::Circle,
        CoverShape::Pentagon => MaterialShape::Pentagon,
        CoverShape::Gem => MaterialShape::Gem,
        CoverShape::Heart => MaterialShape::Heart,
    })
}

/// The shapes the random mode picks from: the user's range, or every
/// shape when it is empty.
pub(crate) fn random_pool(chosen: &[CoverShape]) -> Vec<CoverShape> {
    let pool: Vec<CoverShape> = CoverShape::shapes().iter().copied().filter(|c| chosen.contains(c)).collect();
    if pool.is_empty() { CoverShape::shapes().to_vec() } else { pool }
}

fn random_id() -> egui::Id {
    egui::Id::new("np-random-shape")
}

/// The random mode draws another shape on the next frame (the double
/// click gesture).
pub(crate) fn reroll_shape(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(random_id().with("reroll"), true));
}

/// The outline for `track` under the user's choice: a fixed shape, none,
/// or a random one from `range` drawn whenever another track starts
/// (nothing is remembered per track, and never the same shape twice in a
/// row).
pub fn cover_shape(
    ctx: &egui::Context,
    choice: CoverShape,
    range: &[CoverShape],
    track: Option<&TrackRow>,
) -> Option<MaterialShape> {
    cover_shape_in(ctx, random_id(), choice, range, track)
}

/// [`cover_shape`] with the random pick kept under `id` (the settings'
/// stage keeps its own, [`Stage::id`]).
fn cover_shape_in(
    ctx: &egui::Context,
    id: egui::Id,
    choice: CoverShape,
    range: &[CoverShape],
    track: Option<&TrackRow>,
) -> Option<MaterialShape> {
    if choice != CoverShape::Random {
        return material(choice);
    }
    let pool = random_pool(range);
    let key = track.map(|t| t.id);
    let reroll = ctx.data_mut(|d| d.remove_temp::<bool>(id.with("reroll"))).unwrap_or(false);
    let (seen, last): (Option<nikgob_view::TrackId>, Option<CoverShape>) =
        ctx.data(|d| d.get_temp(id)).unwrap_or((None, None));
    let pick = match last {
        Some(last) if seen == key && !reroll && pool.contains(&last) => last,
        _ => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::time::SystemTime::now().hash(&mut h);
            key.hash(&mut h);
            let mut next = (h.finish() % pool.len() as u64) as usize;
            if pool.len() > 1 && Some(pool[next]) == last {
                next = (next + 1) % pool.len();
            }
            let pick = pool[next];
            ctx.data_mut(|d| d.insert_temp(id, (key, Some(pick))));
            pick
        }
    };
    material(pick)
}

/// The playing outline of `shape`, centred (see [`centred`]), built once
/// per shape.
pub(crate) fn shape_cubics(shape: MaterialShape) -> &'static [Cubic] {
    static CACHE: OnceLock<Mutex<HashMap<MaterialShape, &'static [Cubic]>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(shape)
        .or_insert_with(|| Box::leak(centred(shape.polygon()).cubics().to_vec().into_boxed_slice()))
}

/// `from` → `to`, both playing outlines, built once per pair.
pub(crate) fn pair_morph(from: MaterialShape, to: MaterialShape) -> &'static Morph {
    static CACHE: OnceLock<Mutex<HashMap<(MaterialShape, MaterialShape), &'static Morph>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.entry((from, to)).or_insert_with(|| {
        Box::leak(Box::new(Morph::new(&centred(from.polygon()), &centred(to.polygon()))))
    })
}

/// `shape` (playing) ↔ rounded square with radius 48/480 (paused), built
/// once per shape.
fn cover_morph(shape: MaterialShape) -> &'static Morph {
    static CACHE: OnceLock<Mutex<HashMap<MaterialShape, &'static Morph>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache.entry(shape).or_insert_with(|| {
        let square = RoundedPolygon::rectangle(
            1.0,
            1.0,
            CornerRounding::new(0.1, 0.0),
            None,
            Point::new(0.5, 0.5),
        )
        .expect("unit square")
        .normalized();
        Box::leak(Box::new(Morph::new(&square, &centred(shape.polygon()))))
    })
}

/// `polygon` moved so its centre of area sits at the middle of the unit
/// square, and scaled to fit the circle there. Shapes come normalised by
/// their bounds, which puts a pentagon or a heart off centre: it would sit
/// lopsided in the ring and wobble as it turns.
pub(crate) fn centred(polygon: &RoundedPolygon) -> RoundedPolygon {
    let pts: Vec<Point> = polygon
        .cubics()
        .iter()
        .flat_map(|c| (0..16).map(move |k| c.point_on_curve(k as f32 / 16.0)))
        .collect();
    let (mut area, mut cx, mut cy) = (0.0f32, 0.0f32, 0.0f32);
    for (i, a) in pts.iter().enumerate() {
        let b = pts[(i + 1) % pts.len()];
        let cross = a.x * b.y - b.x * a.y;
        area += cross;
        cx += (a.x + b.x) * cross;
        cy += (a.y + b.y) * cross;
    }
    if area.abs() < 1e-6 {
        return polygon.clone();
    }
    let (cx, cy) = (cx / (3.0 * area), cy / (3.0 * area));
    let reach = pts
        .iter()
        .map(|p| ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt())
        .fold(0.0f32, f32::max)
        .max(1e-3);
    let k = 0.5 / reach;
    polygon.transformed(|x, y| (0.5 + (x - cx) * k, 0.5 + (y - cy) * k))
}

/// The cover's outline this frame: the morph towards `shape` at
/// `progress` (`None`: the square cover) placed in `rect`, turned by
/// `rotation`. The ring is built around it, so dots and bars follow the
/// shape as it turns and swells.
pub struct Outline {
    cubics: Vec<Cubic>,
    rect: Rect,
    rotation: f32,
}

/// Bins of [`Outline::radii`]: one per degree.
const BINS: usize = 360;

impl Outline {
    pub fn new(shape: Option<MaterialShape>, progress: f32, rect: Rect, rotation: f32) -> Self {
        let (shape, progress) = match shape {
            Some(shape) => (shape, progress.clamp(0.0, 1.0)),
            None => (MaterialShape::Cookie12Sided, 0.0),
        };
        Self {
            cubics: cover_morph(shape).as_cubics(progress),
            rect,
            // Unwinds to upright as it turns back into the square. The
            // fraction is clamped above, so a bouncing morph spring adds no
            // extra turn.
            rotation: rotation * progress,
        }
    }

    /// The playing outline turning from `from` into `to` (`t` 0 → 1, may
    /// overshoot a little) in `rect`, turned by `rotation`.
    pub fn swap(from: MaterialShape, to: MaterialShape, t: f32, rect: Rect, rotation: f32) -> Self {
        Self { cubics: pair_morph(from, to).as_cubics(t.clamp(0.0, 1.1)), rect, rotation }
    }

    /// Distance from the centre to the outline for [`BINS`] directions,
    /// clockwise from the top. The shapes are star-shaped around their
    /// centre, so each direction meets the outline once; each segment of
    /// the flattened path fills the bins its two ends span.
    fn radii(&self) -> Vec<f32> {
        let c = self.rect.center();
        let path = shape_paint::shape_path(&self.cubics, self.rect, self.rotation, 0.5);
        let mut out = vec![0.0f32; BINS];
        let step = TAU / BINS as f32;
        let polar = |v: egui::Vec2| v.x.atan2(-v.y).rem_euclid(TAU);
        let cross = |a: egui::Vec2, b: egui::Vec2| a.x * b.y - a.y * b.x;
        for (k, a) in path.iter().enumerate() {
            let (a, b) = (*a - c, path[(k + 1) % path.len()] - c);
            let (ta, tb) = (polar(a), polar(b));
            let span = (tb - ta + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
            let (from, len) = if span >= 0.0 { (ta, span) } else { (tb, -span) };
            let first = (from / step).ceil() as i64;
            let last = ((from + len) / step).floor() as i64;
            let e = b - a;
            for bin in first..=last {
                let th = bin as f32 * step;
                let d = vec2(th.sin(), -th.cos());
                let den = cross(d, e);
                if den.abs() < 1e-6 {
                    continue;
                }
                let t = cross(a, e) / den;
                let i = bin.rem_euclid(BINS as i64) as usize;
                out[i] = out[i].max(t);
            }
        }
        // Bins a segment skipped (path points exactly on a bin edge).
        let fallback = self.rect.width() * 0.5;
        for i in 0..BINS {
            if out[i] <= 0.0 {
                out[i] = [out[(i + BINS - 1) % BINS], out[(i + 1) % BINS]]
                    .into_iter()
                    .fold(0.0, f32::max)
                    .max(if out.iter().all(|r| *r <= 0.0) { fallback } else { 0.0 });
            }
        }
        out
    }

    /// Whether `p` falls inside the outline itself (not just its square):
    /// even-odd test on the flattened path.
    pub fn contains(&self, p: Pos2) -> bool {
        if !self.rect.expand(self.rect.width() * 0.1).contains(p) {
            return false;
        }
        let path = shape_paint::shape_path(&self.cubics, self.rect, self.rotation, 0.5);
        let mut inside = false;
        for (k, a) in path.iter().enumerate() {
            let b = path[(k + 1) % path.len()];
            if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y) {
                inside = !inside;
            }
        }
        inside
    }

    /// The cover texture cropped to the outline. The picture itself never
    /// turns, only its outline.
    pub fn paint(&self, ui: &Ui, uri: Option<&str>) {
        let s = Theme::get(ui.ctx()).scheme;
        let rect = self.rect;
        let texture = uri
            .and_then(|u| crate::covers::texture(ui.ctx(), u, rect.size(), 1.0))
            .map(|t| t.id);
        let opts = shape_paint::FillOptions::for_ppp(super::stage::ppp(ui.ctx()));
        let mesh: Mesh = match texture {
            Some(tex) => shape_paint::texture_mesh(
                &self.cubics,
                rect,
                self.rotation,
                tex,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
                &opts,
            ),
            None => shape_paint::fill_mesh(&self.cubics, rect, self.rotation, s.secondary_container, &opts),
        };
        if !mesh.is_empty() {
            ui.painter().add(Shape::mesh(mesh));
        }
        if texture.is_none() {
            let icon = rect.width() * 0.3;
            Icon::MusicNote
                .widget(false, s.on_secondary_container, icon)
                .paint_at(ui, Rect::from_center_size(rect.center(), vec2(icon, icon)));
        }
    }
}

/// Room the ring takes around the cover on each side.
const RING_PAD: f32 = 28.0;

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
struct RingInput<'a> {
    style: RingStyle,
    /// Turn of the ring (radians, either sign).
    angle: f32,
    /// Per-band transients (0..1); empty when the ring ignores the music.
    transients: &'a [f32],
    /// Bar heights (0..1) for [`RingStyle::Bars`], one per bar; empty = rest.
    bars: &'a [f32],
    /// Room for the bars outside the outline.
    room: f32,
    kick: f32,
    alpha: f32,
    /// Gaps, wobble and dot spacing relative to the player's (1); a small
    /// sample shrinks them so each style still reads as itself.
    scale: f32,
    /// Where the «Круг» ring keeps its dots' place ([`dot_phase`]).
    key: egui::Id,
}

/// The ring around the cover: dots on a soft 12-lobed circle (around the
/// square, its rounded offset), dots riding
/// the outline at a fixed gap, or rounded bars growing out of it. Dots and
/// bars jump out on each beat (`kick`) and where the spectrum spikes.
fn ring(ui: &Ui, outline: &Outline, input: &RingInput<'_>) {
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
            let gap = 14.0 * sc;
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
                // The soft 12 lobes ride with the dots.
                let wobble = 3.0 * sc * (12.0 * TAU * along / n as f32).cos();
                let out = dir(theta) * (wobble + (level * 16.0 + kick * 8.0) * sc);
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
            let gap = 14.0 * sc;
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
            let spacing = 11.0 * kd;
            let n = ((total / spacing).round() as usize).max(12);
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
            let gap = 8.0 * sc;
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
fn circle_path(radii: &[f32], circle: f32, gap: f32) -> Vec<f32> {
    radii.iter().map(|r| (r + gap).max(circle)).collect()
}

/// Where the «Круг» ring's dots stand, as a fraction of their spacing: the
/// dots are alike, so moving by whole ones shows nothing. It goes on by the
/// ring's turn since the last frame times `per_radian` (dots per radian on
/// the circle), so a dot runs as fast as it would on the circle along a
/// longer path too, and a path that changes length never makes them jump.
fn dot_phase(ctx: &egui::Context, key: egui::Id, angle: f32, per_radian: f32) -> f32 {
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
fn turn(ui: &Ui, id: egui::Id, playing: bool, bass: f32, period: f32, ccw: bool) -> f32 {
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

/// How many times the playing `shape` maps onto itself in one revolution
/// (its lobes), read off the definitions in `nikgob_shapes`: the cookies and
/// clovers are named by it, `repeat`ed custom shapes by their `reps`,
/// the lopsided ones (pentagon, gem, heart) have none. `None`, the square
/// cover, never turns.
fn symmetry(shape: Option<MaterialShape>) -> u32 {
    match shape {
        None => 1,
        Some(shape) => match shape {
            MaterialShape::Cookie12Sided => 12,
            MaterialShape::Cookie9Sided => 9,
            MaterialShape::Cookie7Sided => 7,
            MaterialShape::Cookie6Sided => 6,
            MaterialShape::Cookie4Sided | MaterialShape::Clover4Leaf | MaterialShape::PuffyDiamond => 4,
            MaterialShape::Sunny | MaterialShape::VerySunny | MaterialShape::Flower | MaterialShape::Clover8Leaf => 8,
            MaterialShape::SoftBurst | MaterialShape::Circle => 10,
            MaterialShape::SoftBoom => 16,
            MaterialShape::Puffy => 2,
            _ => 1,
        },
    }
}

/// `spin` moved by whole lobes (`n`-fold symmetry) into `[-lobe/2, lobe/2)`.
/// The shape looks the same there, but turning back to upright from it is
/// at most half a lobe instead of up to a whole revolution.
fn unwind(spin: f32, n: u32) -> f32 {
    let lobe = TAU / n.max(1) as f32;
    (spin + lobe * 0.5).rem_euclid(lobe) - lobe * 0.5
}

/// The angle the cover outline is turned by. While the shape is fully the
/// playing one it is [`unwind`] of `spin`; while the morph is under way (a
/// pause or resume) it follows the spin's steps from the last angle, so it
/// never meets a wrap that would jump the half-way shape: not the spin
/// crossing `TAU`, nor a lopsided shape's angle crossing a half turn.
fn cover_angle(ui: &Ui, id: egui::Id, shape: Option<MaterialShape>, spin: f32, morph: f32) -> f32 {
    let ctx = ui.ctx();
    let n = symmetry(shape);
    let last: Option<(u32, f32, f32)> = ctx.data(|d| d.get_temp(id));
    let angle = match last {
        Some((seen, last_spin, last_angle)) if seen == n && morph < 0.999 => {
            let step = (spin - last_spin + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
            last_angle + step
        }
        _ => unwind(spin, n),
    };
    ctx.data_mut(|d| d.insert_temp(id, (n, spin, angle)));
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
/// and per-band transients (0..1).
fn pulse(ui: &Ui, id: egui::Id, bass: f32, bands: &[f32]) -> (f32, Vec<f32>) {
    let ctx = ui.ctx();
    let now = anim::now(ctx);
    let mut p: Pulse = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let dt = if p.last == 0.0 { 1.0 / 60.0 } else { (now - p.last).clamp(0.0, 0.1) as f32 };
    p.last = now;
    let ease = |tau: f32| 1.0 - (-dt / tau).exp();

    p.bass_avg += (bass - p.bass_avg) * ease(0.6);
    let hit = ((bass - p.bass_avg - 0.03) * 5.0).clamp(0.0, 1.0);
    p.kick = hit.max(p.kick * (-dt / 0.15).exp());

    if p.band_avg.len() != bands.len() {
        p.band_avg = bands.to_vec();
        p.bands = vec![0.0; bands.len()];
    }
    for ((avg, out), &b) in p.band_avg.iter_mut().zip(&mut p.bands).zip(bands) {
        *avg += (b - *avg) * ease(0.45);
        let target = ((b - *avg) * 4.0 + b * 0.2).clamp(0.0, 1.0);
        *out = target.max(*out * (-dt / 0.12).exp());
    }
    let out = (p.kick, p.bands.clone());
    ctx.data_mut(|d| d.insert_temp(id, p));
    out
}

/// Bars around the cover in [`RingStyle::Bars`].
const BAR_COUNT: usize = 48;

/// cava-style processing of the analyzer's bands for the bars: one pass
/// around the circle (no mirror), automatic sensitivity (normalised between
/// a slow floor and a peak that falls back slowly), neighbour smoothing
/// ("monstercat") and gravity on the way down.
#[derive(Clone, Default)]
struct Cava {
    last: f64,
    peak: f32,
    floor: f32,
    shown: Vec<f32>,
    fall: Vec<f32>,
}

/// One step of the [`Cava`]: [`BAR_COUNT`] heights (0..1). `active` false
/// lets the bars fall to rest.
fn cava(ui: &Ui, stage: &Stage, bands: &[f32], active: bool) -> Vec<f32> {
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

/// The bars' targets this frame: `bands` resampled to [`BAR_COUNT`] in one
/// pass, auto-scaled and smoothed. `peak_k` / `floor_k` are this frame's
/// easing fractions for the sensitivity envelopes.
fn cava_targets(c: &mut Cava, bands: &[f32], active: bool, peak_k: f32, floor_k: f32) -> Vec<f32> {
    let raw: Vec<f32> = (0..BAR_COUNT)
        .map(|i| {
            if !active || bands.is_empty() {
                return 0.0;
            }
            let x = i as f32 / (BAR_COUNT - 1) as f32 * (bands.len() - 1) as f32;
            let (i0, f) = (x.floor() as usize, x.fract());
            let i1 = (i0 + 1).min(bands.len() - 1);
            bands[i0] * (1.0 - f) + bands[i1] * f
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

/// The cover outline turning into another shape (a new random shape, a
/// double click): `(from, to, t)` while it lasts. Only while the outline is
/// fully the playing shape (`live`): a change during a pause or its morph
/// just shows the new shape, and a pause drops a swap under way, so a
/// resume never jumps back to a half-way blend. Remembers the last shape
/// shown, so call it every frame.
fn shape_swap(
    ctx: &egui::Context,
    stage: &Stage,
    shape: Option<MaterialShape>,
    live: bool,
    reduced: bool,
) -> Option<(MaterialShape, MaterialShape, f32)> {
    const DURATION: f64 = 0.65;
    let id = stage.id("np-shape-swap");
    let now = anim::now(ctx);
    let (last, from, start): (Option<MaterialShape>, Option<MaterialShape>, f64) =
        ctx.data(|d| d.get_temp(id)).unwrap_or((shape, None, 0.0));
    let (from, start) = if last != shape { (last, now) } else { (from, start) };
    let from = from.filter(|_| live && !reduced);
    ctx.data_mut(|d| d.insert_temp(id, (shape, from, start)));
    let (Some(from), Some(to)) = (from, shape) else { return None };
    let x = ((now - start) / DURATION) as f32;
    if reduced || x >= 1.0 {
        return None;
    }
    stage.repaint(ctx);
    // A springy settle: quick, with a small overshoot.
    let t = 1.0 - (-6.0 * x).exp() * (7.0 * x).cos();
    Some((from, to, t))
}

/// The double click gesture: another outline for the cover. The random
/// mode draws a new one; a fixed shape steps to the next one in the range.
fn change_shape(ctx: &egui::Context, prefs: &nikgob_view::Prefs, out: &mut Vec<Command>) {
    if prefs.cover_shape == CoverShape::Random {
        reroll_shape(ctx);
        return;
    }
    let pool = random_pool(&prefs.random_shapes);
    let next = match pool.iter().position(|c| *c == prefs.cover_shape) {
        Some(i) => pool[(i + 1) % pool.len()],
        None => pool[0],
    };
    if next != prefs.cover_shape {
        out.push(Command::SetPref(nikgob_view::PrefPatch::CoverShape(next)));
    }
}

/// Which single tap is waiting out the double click delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tap {
    Pause,
}

/// How visible the ring is `played` of the way from paused (0) to playing
/// (1): dimmed on a pause, or with «Кольцо на паузе» off faded out.
pub(crate) fn ring_alpha(on_pause: bool, played: f32) -> f32 {
    let paused = if on_pause { 0.35 } else { 0.0 };
    paused + (1.0 - paused) * played.clamp(0.0, 1.0)
}

/// The container under Now Playing: the floating pill's colour and radius
/// turning into the full surface as `k` goes 0 → 1.
pub fn container(ui: &Ui, rect: Rect, k: f32) {
    let s = Theme::get(ui.ctx()).scheme;
    let kc = k.clamp(0.0, 1.0);
    // The pill's own radius going to square corners; a full radius on the
    // growing rect would leave the content's rectangular clip poking out
    // of big round ends.
    let radius = (crate::components::toolbar::HEIGHT * 0.5).min(rect.height() * 0.5) * (1.0 - kc);
    let bg = crate::lerp_color(s.primary_container, s.surface, kc);
    paint::container(ui.painter(), rect, Radii::all(radius), bg, Stroke::NONE);
}

/// `r` with its corner on the pixel grid (crisp text, no "Unaligned").
fn snap(ui: &Ui, r: Rect) -> Rect {
    let ppp = ui.ctx().pixels_per_point();
    let min = pos2((r.min.x * ppp).round() / ppp, (r.min.y * ppp).round() / ppp);
    Rect::from_min_size(min, r.size())
}

/// Draws Now Playing over `rect` (the whole window). `embedded` = mini mode,
/// with no close button. Returns true when the user closes it (the close
/// button, a click on empty space, a swipe down, or a navigation chosen
/// from a menu).
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    state: &ViewState,
    embedded: bool,
    pages: &mut PageState,
    chrome: &mut Chrome,
    out: &mut Vec<Command>,
) -> bool {
    show_on(ui, rect, state, embedded, pages, chrome, &Stage::REAL, out)
}

/// [`show`] for the real screen ([`Stage::REAL`]) or the settings' stage:
/// the copy keeps springs of its own, reads no pointer, claims neither the
/// wheel nor the snackbars' spot, plays on the levels it is given and
/// notes where its parts are. (Its commands are the caller's to drop.)
#[allow(clippy::too_many_arguments)]
pub fn show_on(
    ui: &mut Ui,
    rect: Rect,
    state: &ViewState,
    embedded: bool,
    pages: &mut PageState,
    chrome: &mut Chrome,
    stage: &Stage,
    out: &mut Vec<Command>,
) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let p = &state.player;
    let playing = p.status == Status::Playing;
    let mut closed = false;
    let levels_bass = stage.bass(pages.levels_bass);
    let bands = stage.bands(&chrome.bands);

    // The layer must know its size, or the wheel scrolls the page under it.
    ui.expand_to_include_rect(rect);
    // Swallow input under the overlay; a click on empty space or a swipe
    // down closes it.
    let bg = ui.interact(rect, ui.id().with("np-bg"), Sense::click_and_drag());
    if !embedded
        && stage.live
        && bg.drag_stopped()
        && let Some(d) = bg.total_drag_delta()
        && d.y > 96.0
        && d.y > d.x.abs() * 1.5
    {
        closed = true;
    }
    {
        let _p = crate::prof::scope("np.ambient");
        ambient(ui, rect, state, levels_bass, stage);
    }
    let motion = !theme.reduced_motion;
    let bass = if playing { levels_bass } else { 0.0 };
    let (kick, transients) = if playing && motion {
        pulse(ui, stage.id("np-pulse"), bass, &bands)
    } else {
        (0.0, Vec::new())
    };
    {
        let _p = crate::prof::scope("np.bg_shapes");
        super::bg_shapes::paint(ui, stage.id("np-bg-shapes"), rect, &state.prefs, playing && stage.moving, kick, &transients, bass);
    }
    stage.part(&ctx, Part::Background, rect);

    let class = size_class(rect.width());
    let margin = class.margin();
    let inner = rect.shrink2(vec2(margin, 16.0));
    // Content scrolls; the wheel elsewhere changes volume.
    if stage.live {
        chrome.volume_zone(rect, WheelZone::Np);
    }

    // Header.
    let header = snap(ui, Rect::from_min_size(inner.min, vec2(inner.width(), 48.0)));
    let mut hui = ui.new_child(
        UiBuilder::new()
            .max_rect(header)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    if !embedded
        && hui
            .add(IconButton::new(Icon::ExpandMore).tooltip("Свернуть (Esc)").id_salt("np-close"))
            .clicked()
    {
        closed = true;
    }
    let ctx_text = p.context_line();
    let left_used = hui.min_rect().right() - header.left();
    let right_used = hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let more = ui.add(IconButton::new(Icon::MoreVert).tooltip("Ещё").id_salt("np-more"));
        if let Some(t) = p.current.as_ref() {
            // The track's one list, the visualizer before it, the delete last.
            let rows = Chrome::player_rows(t, state, &[Act::Visualizer], true, true);
            chrome.track_menu(&more, "np-more", rows, t, state, Where::Np, out);
        }
        if ui.add(
            IconButton::new(Icon::QueueMusic)
                .style(Style::Tonal)
                .selected(chrome.np_slot == NpSlot::Queue)
                .tooltip("Очередь (Q)")
                .id_salt("np-queue-btn"),
        ).clicked() {
            chrome.np_slot.toggle(NpSlot::Queue);
        }
        if ui.add(
            IconButton::new(Icon::Lyrics)
                .style(Style::Tonal)
                .selected(chrome.np_slot == NpSlot::Lyrics)
                .tooltip("Текст (L)")
                .id_salt("np-lyrics-btn"),
        ).clicked() {
            chrome.np_slot.toggle(NpSlot::Lyrics);
        }
        header.right() - ui.min_rect().left()
    }).inner;
    // Centred on the whole screen, not in the gap between the buttons.
    let side = left_used.max(right_used).max(0.0) + 8.0;
    let title = Rect::from_min_max(pos2(header.left() + side, header.top()), pos2(header.right() - side, header.bottom()));
    if title.width() > 40.0 {
        let mut tui = ui.new_child(
            UiBuilder::new()
                .max_rect(title)
                .layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)),
        );
        // The album, playlist or artist it plays from: its name is a link to
        // its page, like the album under the title.
        match (p.context_link(&ctx_text), p.context_page.clone()) {
            (Some(name), Some(page)) => {
                let format = TypeRole::TitleSmall.style().text_format(s.on_surface_variant);
                if crate::components::link_label(&mut tui, "np-context", &ctx_text, &[name], format).clicked.is_some() {
                    chrome.nav = Some(page);
                }
            }
            _ => {
                common::label(&mut tui, &ctx_text, TypeRole::TitleSmall, s.on_surface_variant);
            }
        }
    }

    let body = Rect::from_min_max(pos2(inner.left(), header.bottom() + 16.0), inner.max);
    let slot_open = chrome.np_slot != NpSlot::None;
    let mega_k = anim::fraction(&ctx, stage.id("np-mega"), if slot_open { 1.0 } else { 0.0 }, theme.motion.default_spatial).clamp(0.0, 1.0);
    let content_alpha = slot_opacities(&ctx, chrome.np_slot, stage);
    let main = body;

    // Layout: cover beside the text when there is room, above it otherwise.
    let info_h = INFO_H;
    let wide = main.width() >= 720.0 && main.height() >= 320.0;
    let (cover_box, info_rect) = if wide {
        let info_w = (main.width() * 0.42).clamp(360.0, 520.0);
        let gap = 40.0;
        let cover = (main.width() - info_w - gap)
            .min(main.height() - 16.0)
            .clamp(160.0, 600.0);
        let total = cover + gap + info_w;
        let left = main.center().x - total * 0.5;
        let cy = main.center().y;
        (
            Rect::from_center_size(pos2(left + cover * 0.5, cy), vec2(cover, cover)),
            // A touch below centre: the big title makes the block top-heavy.
            Rect::from_min_size(pos2(left + cover + gap, cy - info_h * 0.5 + 20.0), vec2(info_w, info_h)),
        )
    } else {
        let col_w = main.width().min(560.0);
        let cover = (main.height() - info_h - 24.0)
            .min(col_w)
            .clamp(120.0, 560.0);
        let block = cover + 24.0 + info_h;
        let top = main.top() + ((main.height() - block) * 0.5).max(0.0);
        let cx = main.center().x;
        (
            Rect::from_center_size(pos2(cx, top + cover * 0.5), vec2(cover, cover)),
            Rect::from_min_size(pos2(cx - col_w * 0.5, top + cover + 24.0), vec2(col_w, info_h)),
        )
    };

    if stage.live {
        snack_spot(&ctx, rect, wide, slot_open, cover_box, info_rect);
    }

    // Cover: playing → its shape, paused → rounded square (with «Менять
    // форму на паузе» off it keeps the shape, only still); the ring follows
    // the outline through the morph.
    let prefs = &state.prefs;
    let morph = anim::fraction(
        &ctx,
        stage.id("np-cover-morph"),
        if playing || !prefs.cover_pause_morph { 1.0 } else { 0.0 },
        theme.motion.default_spatial,
    );
    // Playing or paused, apart from the shape (the rings dim on a pause).
    let played = anim::fraction(
        &ctx,
        stage.id("np-cover-play"),
        if playing { 1.0 } else { 0.0 },
        theme.motion.default_spatial,
    )
    .clamp(0.0, 1.0);
    let mega = mega_layout(body);
    let bars = prefs.ring && prefs.ring_style == RingStyle::Bars;
    let current = p.current.as_ref();
    let shape = cover_shape_in(&ctx, stage.id("np-random-shape"), prefs.cover_shape, &prefs.random_shapes, current);
    // The bars want room to grow: the cover gives way to them.
    let pad = if bars { (cover_box.width() * 0.13).clamp(RING_PAD, 80.0) } else { RING_PAD };
    let normal_cover = cover_box.shrink(pad);
    let cover_rect = if theme.reduced_motion { normal_cover } else { lerp_rect(normal_cover, mega.cover, mega_k) };
    let swap = shape_swap(&ctx, stage, shape, morph >= 0.999, theme.reduced_motion);
    let uri = current.and_then(|t| t.cover.as_ref()).map(|c| c.uri());
    // «Обложка → Скорость» and «Пульсация → Сила».
    let period = if prefs.cover_spin && motion { prefs.cover_spin_s.max(1.0) } else { 0.0 };
    let spin = turn(ui, stage.id("np-cover-angle"), playing, bass, period, prefs.cover_ccw);
    let swell = if prefs.cover_pulse { 1.0 + prefs.cover_pulse_strength * kick } else { 1.0 };
    // Instant feedback: the cover sinks a little while pressed, so a tap
    // feels taken even while it waits to see whether a second one comes.
    // (The stage's copy is pressed by its scene, not the pointer.)
    let pressing = if stage.live {
        (prefs.cover_click_pause || prefs.dbl_click_shape > 0)
            && ctx.input(|i| {
                let inside = |p: Pos2| p.distance(cover_rect.center()) < cover_rect.width() * 0.45;
                i.pointer.primary_down()
                    && i.pointer.press_origin().is_some_and(inside)
                    && i.pointer.interact_pos().is_some_and(inside)
            })
    } else {
        stage.pressed
    };
    let press = anim::fraction(&ctx, stage.id("np-cover-press"), if pressing { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
    let swell = swell * (1.0 - 0.04 * press);
    let shaped = Rect::from_center_size(cover_rect.center(), cover_rect.size() * swell);
    // The outline unwinds to upright by the shortest way, not by every
    // turn it has made.
    let angle = cover_angle(ui, stage.id("np-cover-lobes"), shape, spin, morph);
    let outline = match swap {
        // Turned by the raw spin: a full turn or whole lobes of either
        // shape look the same, so neither end of the swap jumps.
        Some((from, to, t)) => Outline::swap(from, to, t, shaped, spin),
        _ => Outline::new(shape, morph, shaped, angle),
    };
    let mega_outline = Outline::new(shape, morph, mega.cover, 0.0);
    let ring_scope = crate::prof::scope("np.ring");
    if prefs.ring {
        let period = if prefs.spin_ring && motion { prefs.ring_spin_s.max(1.0) } else { 0.0 };
        let angle = turn(ui, stage.id("np-ring-angle"), playing, bass, period, prefs.ring_ccw);
        let beat = prefs.ring_beat && playing;
        let heights = if bars { cava(ui, stage, &bands, beat && motion) } else { Vec::new() };
        let mut input = RingInput {
                style: prefs.ring_style,
                angle,
                transients: if beat { &transients } else { &[] },
                bars: &heights,
                room: pad,
                kick: if beat { kick } else { 0.0 },
                // Dimmed on a pause, or gone with «Кольцо на паузе» off;
                // every style follows the outline as it morphs.
                alpha: ring_alpha(prefs.ring_on_pause, played),
                scale: 1.0,
                key: egui::Id::new("np-ring-dots"),
        };
        let alpha = input.alpha;
        input.alpha *= if theme.reduced_motion || !mega.wide { 1.0 - mega_k } else { 1.0 };
        ring(ui, &outline, &input);
        if theme.reduced_motion && mega.wide {
            input.alpha = alpha * mega_k;
            ring(ui, &mega_outline, &input);
        }
    }
    drop(ring_scope);
    let cover_scope = crate::prof::scope("np.cover");
    if theme.reduced_motion {
        let mut normal_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("np-cover-fade"));
        normal_ui.multiply_opacity(1.0 - mega_k);
        outline.paint(&normal_ui, uri.as_deref());
        let mut mega_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("mega-cover-fade"));
        mega_ui.multiply_opacity(mega_k);
        mega_outline.paint(&mega_ui, uri.as_deref());
    } else {
        outline.paint(ui, uri.as_deref());
    }
    drop(cover_scope);
    stage.part(&ctx, Part::Cover, cover_rect);
    stage.part(&ctx, Part::Ring, lerp_rect(cover_box, mega.cover.expand(RING_PAD), mega_k));
    let mut cover_resp = ui.interact(cover_rect, ui.id().with("np-cover"), Sense::click());
    // Only the shape itself is the cover: its square's corners count as
    // the background.
    let on_shape = |pos: Option<Pos2>| pos.is_some_and(|p| outline.contains(p));
    let hover_shape = cover_resp.hovered() && on_shape(cover_resp.hover_pos());
    if prefs.cover_click_pause && hover_shape {
        cover_resp = cover_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    let shape_clicked = cover_resp.clicked() && on_shape(cover_resp.interact_pointer_pos());
    let corner_clicked = cover_resp.clicked() && !shape_clicked;
    if playing && !theme.reduced_motion {
        stage.repaint(&ctx);
    }

    let info_rect = snap(ui, info_rect);
    // Swallows clicks on the info column so they don't close the player.
    ui.interact(info_rect, ui.id().with("np-info-bg"), Sense::click());

    // Taps: a click on empty space closes at once, one on the cover shape
    // pauses; a second tap on the shape soon after changes the outline
    // instead. While that gesture is on, a tap on the shape waits only our
    // own double-tap window (`Prefs::np_tap_wait`, not egui's 0.3 s);
    // closing never waits. With no wait at all the tap pauses at once and
    // a second one undoes the pause as it changes the shape.
    let wait = f64::from(prefs.np_tap_wait.clamp(0.0, 0.4));
    let instant = wait < 0.05;
    let double_tap = if instant { 0.3 } else { wait };
    let gesture = match prefs.dbl_click_shape {
        0 => false,
        1 => prefs.cover_shape == CoverShape::Random,
        _ => prefs.cover_shape != CoverShape::Original,
    };
    let tap_id = stage.id("np-tap");
    if !embedded && stage.live && (bg.clicked() || corner_clicked) {
        ctx.data_mut(|d| d.remove::<(Option<Tap>, f64)>(tap_id));
        closed = true;
    }
    let single = (prefs.cover_click_pause && shape_clicked).then_some(Tap::Pause);
    let fire = if !stage.live {
        None
    } else if gesture {
        let now = ctx.input(|i| i.time);
        // (what the first tap will do, when it landed).
        let pending = ctx.data(|d| d.get_temp::<(Option<Tap>, f64)>(tap_id));
        let mut now_fire = None;
        if shape_clicked {
            match pending {
                Some((done, at)) if now - at <= double_tap => {
                    ctx.data_mut(|d| d.remove::<(Option<Tap>, f64)>(tap_id));
                    change_shape(&ctx, prefs, out);
                    if instant {
                        // The first tap already paused: take it back.
                        now_fire = done;
                    }
                }
                _ => {
                    ctx.data_mut(|d| d.insert_temp(tap_id, (single, now)));
                    if instant {
                        now_fire = single;
                    }
                }
            }
        }
        match ctx.data(|d| d.get_temp::<(Option<Tap>, f64)>(tap_id)) {
            Some((tap, at)) if now - at > double_tap => {
                ctx.data_mut(|d| d.remove::<(Option<Tap>, f64)>(tap_id));
                // Left from before the player was closed: drop it.
                if instant { None } else { tap.filter(|_| now - at < double_tap + 0.25) }
            }
            Some((_, at)) => {
                stage.repaint_after(&ctx, std::time::Duration::from_secs_f64((at + double_tap - now).max(0.0) + 0.01));
                now_fire
            }
            None => now_fire,
        }
    } else {
        single
    };
    match fire {
        Some(Tap::Pause) => out.push(Command::TogglePlay),
        None => {}
    }
    let info_scope = crate::prof::scope("np.info");
    if mega_k < 0.999 {
        let mut normal_ui = ui.new_child(UiBuilder::new().max_rect(info_rect).id_salt("np-normal-info"));
        normal_ui.multiply_opacity(1.0 - mega_k);
        if slot_open { normal_ui.disable(); }
        info(&mut normal_ui, info_rect, wide, InfoMode::Normal, state, pages, chrome, stage, out);
    }
    if mega_k > 0.001 {
        let mut mega_ui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("np-mega-body"));
        mega_ui.multiply_opacity(mega_k);
        if !slot_open { mega_ui.disable(); }
        if mega.wide {
            info(&mut mega_ui, mega.info, false, InfoMode::Compact, state, pages, chrome, stage, out);
        } else {
            thumbnail_info(&mut mega_ui, mega.cover, state, chrome);
            info(&mut mega_ui, mega.info, false, InfoMode::Transport, state, pages, chrome, stage, out);
        }
        let slide = if theme.reduced_motion { 0.0 } else { 32.0 * (1.0 - mega_k) };
        let content = mega.content.translate(vec2(slide, 0.0));
        mega_ui.interact(content, mega_ui.id().with("np-content-bg"), Sense::click());
        if stage.live {
            chrome.wheel_block(content);
        }
        for (slot, alpha) in [NpSlot::Lyrics, NpSlot::Queue, NpSlot::Info].into_iter().zip(content_alpha) {
            if alpha <= 0.001 { continue; }
            let width = if slot == NpSlot::Lyrics { content.width() } else { content.width().min(720.0) };
            // The lyrics take the air above the content too, up to just
            // under the header (or the small cover's row).
            let lift = if slot == NpSlot::Lyrics { mega.lyrics_lift } else { 0.0 };
            let area = Rect::from_min_max(pos2(content.left(), content.top() - lift), pos2(content.left() + width, content.bottom()));
            let mut cui = mega_ui.new_child(UiBuilder::new().max_rect(area).id_salt(("np-content", slot as u8)));
            cui.set_clip_rect(area.intersect(ui.clip_rect()));
            cui.multiply_opacity(alpha);
            if chrome.np_slot != slot { cui.disable(); }
            match slot {
                NpSlot::Lyrics => {
                    let _p = crate::prof::scope("np.lyrics");
                    if super::lyrics::show_on(&mut cui, state, levels_bass, true, stage, out) && stage.live {
                        pages.open_setting_group(super::lyrics_ui::SETTINGS);
                        closed = true;
                    }
                }
                NpSlot::Queue => pane::queue(&mut cui, state, chrome, out),
                NpSlot::Info => pane::info(&mut cui, state, out),
                NpSlot::None => unreachable!(),
            }
        }
    }
    drop(info_scope);
    if state.prefs.np_volume_pop && !state.prefs.np_volume {
        volume_pop(ui, rect, state, chrome, stage, out);
    }

    closed && stage.live
}

/// Where snackbars go while Now Playing is open: `(area, bottom inset)`,
/// written each frame and read by the shell the next ([`snack_spot`]).
pub const SNACK_SPOT: &str = "np-snack-spot";

/// Picks the snackbars' place so they cover neither the transport nor the
/// cover: under the controls column (the cover beside it) or below
/// everything (the cover above), and where there is no room for that over
/// the cover's lower edge, on top of the picture, never of a control; with
/// a slot open, at the bottom.
fn snack_spot(ctx: &egui::Context, rect: Rect, wide: bool, slot_open: bool, cover: Rect, info: Rect) {
    // A two-line snackbar and its margins.
    const ROOM: f32 = 72.0 + 24.0 + 8.0;
    const INSET: f32 = 24.0;
    let room = rect.bottom() - info.bottom() >= ROOM;
    let over_cover = (rect.bottom() - cover.bottom() + 12.0).max(INSET);
    let spot = match (slot_open, wide, room) {
        (true, ..) => (rect, INSET),
        (false, true, true) => (Rect::from_x_y_ranges(info.x_range(), rect.y_range()), INSET),
        (false, true, false) => (Rect::from_x_y_ranges(cover.x_range(), rect.y_range()), over_cover),
        (false, false, true) => (rect, INSET),
        (false, false, false) => (rect, over_cover),
    };
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(SNACK_SPOT), spot));
}

/// «Громкость — при изменении»: a floating pill at the bottom centre that
/// shows the volume while it changes (wheel, keys, media keys, the menu)
/// and slips away `Prefs::np_volume_pop_hold` after the last change; it
/// stays while the pointer is on it, so it can be dragged too.
fn volume_pop(ui: &mut Ui, area: Rect, state: &ViewState, chrome: &mut Chrome, stage: &Stage, out: &mut Vec<Command>) {
    let hold = f64::from(state.prefs.np_volume_pop_hold.clamp(0.3, 5.0));
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let p = &state.player;
    let volume = if p.muted { 0.0 } else { chrome.volume_now(&ctx, state) };
    let now = ctx.input(|i| i.time);
    let id = stage.id("np-vol-pop");
    // (last volume, muted, when it last changed, pointer on the pill).
    let (last, muted, at, held) = ctx
        .data(|d| d.get_temp::<(f32, bool, f64, bool)>(id))
        .unwrap_or((volume, p.muted, f64::NEG_INFINITY, false));
    let at = if (last - volume).abs() > 1e-4 || muted != p.muted { now } else { at };
    let shown = held || now - at < hold;
    let k = anim::fraction(&ctx, id.with("k"), if shown { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
    if shown && !held {
        stage.repaint_after(&ctx, std::time::Duration::from_secs_f64((at + hold - now).max(0.0) + 0.02));
    }
    let mut hovered = false;
    if k > 0.001 {
        let size = vec2(area.width().min(360.0) - 32.0, 64.0);
        let rise = if theme.reduced_motion { 0.0 } else { 28.0 * (1.0 - k) };
        let pill = Rect::from_center_size(
            pos2(area.center().x, area.bottom() - 24.0 - size.y * 0.5 + rise),
            size * (0.92 + 0.08 * k),
        );
        let radii = Radii::all(pill.height() * 0.5);
        let mut pui = ui.new_child(UiBuilder::new().max_rect(pill).id_salt("np-vol-pop-ui"));
        pui.multiply_opacity(k.clamp(0.0, 1.0));
        // Clicks on the pill must not close the player under it.
        let back = pui.interact(pill, id.with("bg"), Sense::click());
        paint::shadow(pui.painter(), pill, radii, crate::tokens::Elevation::Level3);
        paint::container(pui.painter(), pill, radii, s.surface_container_high, Stroke::NONE);
        let inner = pill.shrink2(vec2(12.0, 8.0));
        let mut row = pui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::left_to_right(egui::Align::Center))
                .id_salt("np-vol-pop-row"),
        );
        if row
            .add(IconButton::new(Icon::volume(volume, p.muted)).tooltip("Звук").id_salt("np-vol-pop-mute"))
            .clicked()
        {
            out.push(Command::ToggleMute);
        }
        let label_w = 44.0;
        let w = (row.available_width() - label_w - 8.0).max(64.0);
        let vo = Slider::new("np-vol-pop-slider", volume).track(12.0).width(w).show(&mut row);
        if let Some(v) = vo.dragging.or(vo.committed) {
            chrome.push_volume(state, v, out);
        }
        common::label(&mut row, &format!("{:.0}%", volume * 100.0), TypeRole::LabelLarge, s.on_surface);
        hovered = stage.live && (back.contains_pointer() || vo.dragging.is_some());
        if stage.live {
            chrome.volume_zone(pill, WheelZone::Np);
        }
        stage.part(&ctx, Part::Volume, pill);
    }
    ctx.data_mut(|d| d.insert_temp(id, (volume, p.muted, if hovered { now } else { at }, hovered)));
}

/// A small live sample of the cover under `prefs` for the settings' hover
/// card: its shape (going through the pool for «Случайная»), spin, beat
/// pulse and, with `with_ring`, the ring, all on a made-up beat. It keeps
/// its own clock, so the real player's angles are untouched.
pub(crate) fn preview(ui: &Ui, rect: Rect, prefs: &nikgob_view::Prefs, with_ring: bool) {
    let motion = !Theme::get(ui.ctx()).reduced_motion;
    let t = anim::now(ui.ctx()) as f32;
    let kick = if motion { super::pages::settings_help::beat(t) } else { 0.0 };
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
    if with_ring {
        let angle = if prefs.spin_ring && motion && prefs.ring_style != RingStyle::Bars {
            t * TAU / (prefs.ring_spin_s / 3.6).max(2.0) * sign(prefs.ring_ccw)
        } else {
            0.0
        };
        let beat = prefs.ring_beat && motion;
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
            // The player's cover is ~300 px: at sample size the gaps and
            // the circle's wobble would swamp the shape.
            scale: (side / 300.0).clamp(0.3, 1.0),
            key: egui::Id::new("np-ring-dots-sample"),
        };
        ring(ui, &outline, &input);
    }
}

/// Retain the last content on close; only switching open slots crossfades it.
fn slot_opacities(ctx: &egui::Context, slot: NpSlot, stage: &Stage) -> [f32; 4] {
    let theme = Theme::get(ctx);
    let spring = anim::effective(ctx, theme.motion.fast_effects);
    let now = anim::now(ctx);
    let (alpha, settled) = ctx.data_mut(|d| {
        let target = match slot {
            NpSlot::None | NpSlot::Lyrics => [1.0, 0.0, 0.0, 0.0],
            NpSlot::Queue => [0.0, 1.0, 0.0, 0.0],
            NpSlot::Info => [0.0, 0.0, 1.0, 0.0],
        };
        let (last, a) = d.get_temp_mut_or_insert_with(stage.id("np-content-fade"), || (slot, Animated::new(target, spring)));
        if slot != NpSlot::None {
            if *last == NpSlot::None { a.snap(target); }
            if a.spring() != spring {
                a.animate_to(target, spring, now);
            } else {
                a.set_target(target, now);
            }
        }
        *last = slot;
        (a.value(now), a.is_settled(now))
    });
    // A crossfade finishes even on a still stage.
    if !settled { ctx.request_repaint(); }
    alpha.map(|a| a.clamp(0.0, 1.0))
}

/// Height of the text and controls block.
const INFO_H: f32 = TITLE_H + 4.0 + 24.0 + 24.0 + 64.0 + PROGRESS_GAP + CONTROLS_H + 16.0 + 48.0;
/// Air between the progress bar and the transport row.
const PROGRESS_GAP: f32 = 28.0;
const TITLE_H: f32 = 44.0;
const CONTROLS_H: f32 = 96.0;

const COMPACT_INFO_H: f32 = 32.0 + 4.0 + 24.0 + 16.0 + 64.0 + 16.0 + 56.0;
const TRANSPORT_H: f32 = 64.0 + 16.0 + 56.0;
/// How far the lyrics beside the cover end above the cover column's bottom
/// air: the control on their bottom edge (32 px) is then centred on the
/// transport row (56 px) where the column fills the height.
const LYRICS_CONTROL_LIFT: f32 = 12.0;
/// Air between the header (or the small cover's row) and the lyrics' top.
const LYRICS_TOP_GAP: f32 = 8.0;

struct MegaLayout {
    wide: bool,
    cover: Rect,
    info: Rect,
    content: Rect,
    /// How far above `content` the lyrics start: they span from just under
    /// the header (beside the cover) or the small cover's row (narrow) down
    /// to the control's row, their soft bands inside that span.
    lyrics_lift: f32,
}

/// Wide: the cover column and the content form one group centred in the
/// window, so a big screen has no empty half on the right.
fn mega_layout(body: Rect) -> MegaLayout {
    if body.width() >= 900.0 {
        // Apple Music style: the window splits in halves. The cover column
        // sits in the middle of the left half; the lyrics / queue start just
        // right of the centre line. Room at the window edges throughout.
        let pad = (body.width() * 0.05).clamp(24.0, 96.0);
        let mid = body.center().x;
        let half = mid - body.left() - pad;
        let col = (half - 24.0).clamp(300.0, 500.0).min(half);
        let left = body.left() + pad + (half - col) * 0.5;
        let gap = (body.width() * 0.03).clamp(24.0, 64.0);
        let content_left = mid + gap * 0.5;
        let lyrics_w = (body.right() - pad - content_left).min(860.0);
        // Air above and below, so the transport row never hugs the
        // window's bottom edge.
        let vpad = (body.height() * 0.06).clamp(24.0, 56.0);
        let cover = (col - 2.0 * RING_PAD)
            .min(body.height() - 2.0 * vpad - 2.0 * RING_PAD - 16.0 - COMPACT_INFO_H)
            .clamp(120.0, 440.0);
        let height = cover + 2.0 * RING_PAD + 16.0 + COMPACT_INFO_H;
        let top = body.center().y - height * 0.5;
        let info = Rect::from_min_size(pos2(left, top + cover + 2.0 * RING_PAD + 16.0), vec2(col, COMPACT_INFO_H));
        MegaLayout {
            wide: true,
            cover: Rect::from_center_size(pos2(left + col * 0.5, top + RING_PAD + cover * 0.5), vec2(cover, cover)),
            info,
            // The lyrics reach down to the cover column's bottom air, the
            // control on their bottom edge (they scroll on behind it):
            // level with the transport row where the column fills the
            // height, at that same bottom margin where it is centred
            // higher in a tall window.
            content: Rect::from_min_max(
                pos2(content_left, body.top()),
                pos2(content_left + lyrics_w, body.bottom() - vpad - LYRICS_CONTROL_LIFT),
            ),
            // `content` starts 16 px under the header.
            lyrics_lift: 16.0 - LYRICS_TOP_GAP,
        }
    } else {
        MegaLayout {
            wide: false,
            cover: Rect::from_min_size(body.min, vec2(56.0, 56.0)),
            info: Rect::from_min_size(pos2(body.left(), body.bottom() - TRANSPORT_H), vec2(body.width(), TRANSPORT_H)),
            content: Rect::from_min_max(pos2(body.left(), body.top() + 72.0), pos2(body.right(), body.bottom() - TRANSPORT_H - 16.0)),
            // `content` starts 16 px under the small cover's row.
            lyrics_lift: 16.0 - LYRICS_TOP_GAP,
        }
    }
}

fn lerp_rect(from: Rect, to: Rect, k: f32) -> Rect {
    Rect::from_min_max(from.min.lerp(to.min, k), from.max.lerp(to.max, k))
}

fn thumbnail_info(ui: &mut Ui, cover: Rect, state: &ViewState, chrome: &mut Chrome) {
    let Some(t) = &state.player.current else { return };
    let s = Theme::get(ui.ctx()).scheme;
    let rect = Rect::from_min_max(pos2(cover.right() + 16.0, cover.top()), pos2(ui.max_rect().right(), cover.bottom()));
    let mut tui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("mega-title"));
    tui.add(egui::Label::new(TypeRole::TitleLarge.emphasized().rich(&t.title, s.on_surface)).truncate());
    let format = TypeRole::TitleMedium.style().text_format(s.on_surface_variant);
    if let Some(r) = super::artist_links::label(&mut tui, "mega-artists", &t.artist, &t.artists, format) {
        chrome.nav = Some(r);
    }
}

/// The natural widths of «Artist · Album»: all the artists, the first
/// one's name, « · » and the album (0 without one).
#[derive(Clone, Copy)]
struct LineWidths {
    artist: f32,
    first: f32,
    sep: f32,
    album: f32,
}

/// The least of the album worth showing after « · »: a few letters and «…».
const ALBUM_MIN: f32 = 48.0;

/// How `width` is shared by the artists and « · album»: both whole when
/// they fit; else the album truncates down to [`ALBUM_MIN`], then the
/// artists down to the first one's name, then the album goes.
fn split_line(width: f32, w: LineWidths) -> (f32, Option<f32>) {
    if w.album <= 0.0 {
        return (w.artist.min(width), None);
    }
    let album = w.sep + w.album;
    if w.artist + album <= width {
        return (w.artist, Some(album));
    }
    let least = w.sep + ALBUM_MIN.min(w.album);
    if w.artist + least <= width {
        return (w.artist, Some(width - w.artist));
    }
    let artist = (width - least).max(w.first.min(w.artist)).min(width);
    let rest = width - artist;
    (artist, (rest >= least).then_some(rest))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InfoMode {
    Normal,
    Compact,
    Transport,
}

/// Title, artist · album, progress, transport and the action row (like,
/// add to playlist, volume, dislike).
#[allow(clippy::too_many_arguments)]
fn info(
    ui: &mut Ui,
    rect: Rect,
    wide: bool,
    mode: InfoMode,
    state: &ViewState,
    pages: &mut PageState,
    chrome: &mut Chrome,
    stage: &Stage,
    out: &mut Vec<Command>,
) {
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    let p = &state.player;
    let playing = p.status == Status::Playing;
    let current = p.current.as_ref();
    let col = rect;
    let mut y = col.top();
    // Of the two layouts crossfading, the one mostly shown has the parts.
    let mostly_shown = ui.opacity() > 0.5;
    let part = |part: Part, r: Rect| {
        if mostly_shown {
            stage.part(&ctx, part, r);
        }
    };

    let compact = mode != InfoMode::Normal;
    let title_h = if compact { 32.0 } else { TITLE_H };
    // Title and links.
    if mode != InfoMode::Transport {
        let text_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), title_h + 4.0 + 24.0));
        part(Part::Title, text_rect);
        let mut tui = ui.new_child(UiBuilder::new().max_rect(text_rect).id_salt("np-text"));
        tui.spacing_mut().item_spacing.y = 4.0;
        let title_role = if compact { TypeRole::HeadlineSmall } else if wide { TypeRole::DisplaySmall } else { TypeRole::HeadlineLarge };
        match current {
            Some(t) => {
                tui.add(egui::Label::new(title_role.emphasized().rich(&t.title, s.on_surface)).truncate());
                // The artists, « · » and the album are laid out alike, so
                // they share one baseline.
                tui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    // «Artist · Album» in one line: the album gives way
                    // first, the artists keep at least the first one's name.
                    let format = TypeRole::TitleMedium.style().text_format(s.on_surface_variant);
                    let width_of = |text: &str| {
                        ui.painter()
                            .layout_job(egui::text::LayoutJob::single_section(text.to_owned(), format.clone()))
                            .size()
                            .x
                            .ceil()
                            + 1.0
                    };
                    let first = t.artists.0.first().map_or(t.artist.as_str(), |c| c.name.as_str());
                    let widths = LineWidths {
                        artist: width_of(&t.artist),
                        first: width_of(first),
                        sep: width_of(" · "),
                        album: t.album.as_deref().map_or(0.0, width_of),
                    };
                    // A track not played as recorded says so, quietly, at
                    // the end of the line (its room comes off the album).
                    let mark = speed_mark_of(state, t.id);
                    let mark = mark.as_deref();
                    let mark_w = mark.map_or(0.0, |m| speed_mark_width(ui, m) + SPEED_MARK_GAP);
                    let (artist_w, album_w) = split_line((ui.available_width() - mark_w).max(0.0), widths);
                    let h = ui.available_height();
                    // Each artist is a link of its own.
                    ui.allocate_ui(vec2(artist_w, h), |ui| {
                        if let Some(r) = super::artist_links::label(ui, "np-artists", &t.artist, &t.artists, format.clone()) {
                            chrome.nav = Some(r);
                        }
                    });
                    if let (Some(a), Some(w)) = (&t.album, album_w) {
                        crate::components::link_label(ui, "np-sep", " · ", &[], format.clone());
                        ui.allocate_ui(vec2(w - widths.sep, h), |ui| {
                            let album = crate::components::link_label(ui, "np-album", a, &[0..a.len()], format.clone());
                            if album.clicked.is_some() {
                                chrome.nav = t.release_id.map(nikgob_view::Route::Release);
                            }
                        });
                    }
                    if let Some(m) = mark {
                        ui.add_space(SPEED_MARK_GAP);
                        // The mark is the way to what it says.
                        if speed_mark(ui, m).clicked() {
                            sound::open(ui.ctx(), sound::Tab::Speed, None);
                        }
                    }
                });
            }
            None => {
                tui.label(title_role.style().rich("Ничего не играет", s.on_surface));
            }
        }
        y += title_h + 4.0 + 24.0 + if compact { 16.0 } else { 24.0 };
    }

    // Progress.
    let dur = p.duration.unwrap_or_default();
    let speed = p.speed;
    let prog_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), 64.0));
    part(Part::Progress, prog_rect);
    let mut pui = ui.new_child(UiBuilder::new().max_rect(prog_rect).id_salt("np-progress"));
    let so = Slider::new("np-slider", p.fraction_now())
        .width(col.width())
        .wavy(playing && state.prefs.progress_wave)
        .value_label(move |v| common::fmt_time(nikgob_view::heard_time(dur.mul_f32(v), speed)))
        .show(&mut pui);
    if let Some(v) = so.committed {
        out.push(Command::Seek(dur.mul_f32(v)));
    }
    let shown = so.dragging.map_or_else(|| p.position_now(), |v| dur.mul_f32(v));
    pui.horizontal(|ui| {
        let shown = nikgob_view::heard_time(shown, speed);
        ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(shown), s.on_surface_variant));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let heard = nikgob_view::heard_time(dur, speed);
            ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(heard), s.on_surface_variant));
            // What really plays, in one quiet line: where a stream's sound
            // comes from (a Spotify track may play from YouTube Music) and
            // its format, or a file's own format.
            if let Some(line) = common::sound_line(state) {
                ui.add_space(8.0);
                ui.label(TypeRole::LabelMedium.style().rich(line, s.on_surface_variant))
                    .on_hover_text("Откуда идёт звук и в каком качестве");
            }
        });
    });
    y += 64.0 + if compact { 16.0 } else { PROGRESS_GAP };

    // Controls: standard button group, hero play in the middle.
    // The largest sizes whose row fits the column (a narrow window steps
    // the side buttons down first, so play stays the biggest).
    let row_w = |button: Size, hero: Size| {
        4.0 * IconButton::new(Icon::Shuffle).size(button).natural_width()
            + IconButton::new(Icon::PlayArrow).size(hero).natural_width()
            + 4.0 * button.group_gap()
    };
    let tiers: &[(Size, Size)] = if compact {
        &[(Size::S, Size::M), (Size::Xs, Size::S)]
    } else {
        &[(Size::M, Size::L), (Size::S, Size::L), (Size::S, Size::M)]
    };
    // The compact layouts keep like and dislike at the transport row's
    // ends (under the progress bar's ends): the group leaves them room.
    let ends = if compact { 2.0 * (IconButton::new(Icon::Favorite).natural_width() + 8.0) } else { 0.0 };
    let (button_size, hero_size) = tiers
        .iter()
        .copied()
        .find(|&(b, h)| row_w(b, h) + ends <= col.width())
        .unwrap_or(tiers[tiers.len() - 1]);
    let group_w = row_w(button_size, hero_size);
    let controls = snap(
        ui,
        Rect::from_min_size(
            pos2(col.left() + ((col.width() - group_w) * 0.5).max(0.0), y),
            vec2(group_w.min(col.width()), hero_size.height()),
        ),
    );
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(controls)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("np-controls"),
    );
    let tonal = |icon: Icon| IconButton::new(icon).size(button_size).style(Style::Tonal);
    let repeat_icon = if p.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat };
    let hero = IconButton::new(Icon::PlayArrow)
        .selected_icon(Icon::Pause)
        .size(hero_size)
        .style(Style::Filled)
        .shape(BtnShape::Round)
        .selected(playing || p.status == Status::Buffering)
        .filled_icon(true)
        .colors(Colors {
            container: s.primary,
            content: s.on_primary,
            border: Color32::TRANSPARENT,
        })
        .tooltip("Играть / пауза (Space)");
    let r = ButtonGroup::new("np-group", button_size).show(
        &mut cui,
        vec![
            Item::from(tonal(Icon::Shuffle).selected(p.shuffle).tooltip("Перемешать")),
            Item::from(tonal(Icon::SkipPrevious).tooltip("Назад (Ctrl+←)")),
            Item::from(hero),
            Item::from(tonal(Icon::SkipNext).tooltip("Далее (Ctrl+→)")),
            Item::from(
                tonal(repeat_icon)
                    .selected(p.repeat != Repeat::Off)
                    .tooltip(super::player_bar::repeat_tip(p.repeat)),
            ),
        ],
    );
    if r[0].clicked() {
        out.push(Command::ToggleShuffle);
    }
    if r[1].clicked() {
        chrome::press_previous(&ctx, state, out);
    }
    if p.status == Status::Buffering {
        common::buffering_overlay(ui, r[2].rect, s.primary, s.on_primary);
    }
    if r[2].clicked() {
        out.push(Command::TogglePlay);
    }
    if r[3].clicked() {
        out.push(Command::Next);
    }
    if r[4].clicked() {
        out.push(Command::CycleRepeat);
    }
    if compact {
        // Like at the left end, dislike at the right, as in the full
        // layout's row under the transport; here level with the transport
        // (Now Playing with the lyrics, the queue or the details open).
        let Some(t) = current else { return };
        let ends = Rect::from_min_size(pos2(col.left(), controls.top()), vec2(col.width(), controls.height()));
        let mut lui = ui.new_child(
            UiBuilder::new().max_rect(ends).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt("np-compact-like"),
        );
        if lui.add(IconButton::new(Icon::Favorite).selected(t.liked).tooltip("Нравится (Ctrl+L)").id_salt("np-like-compact")).clicked() {
            out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
        }
        if state.prefs.dislike_button {
            let disliked = state.dislikes.track(t.id);
            let mut dui = ui.new_child(
                UiBuilder::new().max_rect(ends).layout(egui::Layout::right_to_left(egui::Align::Center)).id_salt("np-compact-dislike"),
            );
            if dui.add(IconButton::new(Icon::ThumbDown).selected(disliked).tooltip("Не нравится").id_salt("np-dislike-compact")).clicked() {
                out.push(Command::Dislike { track: t.id, dislike: !disliked, sure: false, relike: false });
            }
        }
        return;
    }
    y += CONTROLS_H + 16.0;


    // Actions: like and add on the left; volume and dislike on the right,
    // so like and dislike sit at opposite ends of the row.
    let row = snap(ui, Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), 48.0)));
    let mut aui = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("np-actions"),
    );
    if let Some(t) = current {
        if aui
            .add(
                IconButton::new(Icon::Favorite)
                    .selected(t.liked)
                    .tooltip("Нравится (Ctrl+L)")
                    .id_salt("np-like"),
            )
            .clicked()
        {
            out.push(Command::Like(LikeTarget::Track(t.id), !t.liked));
        }
        let add = aui.add(IconButton::new(Icon::PlaylistAdd).tooltip("В плейлист").id_salt("np-add"));

        pages.playlist_menu(&mut aui, &add, vec![t.id], state, out);
        // The sound of the track: its speed and the equalizer. Lit while
        // either changes what is heard.
        let changed = state.prefs.eq_on || state.speeds.of(t.id) != nikgob_view::SpeedVariant::Normal;
        let tab = sound::first_tab(state);
        let tip = if tab == sound::Tab::Speed { "Скорость и эквалайзер (S, E)" } else { "Эквалайзер (E)" };
        if aui.add(IconButton::new(Icon::Equalizer).selected(changed).tooltip(tip).id_salt("np-sound")).clicked() {
            sound::open(aui.ctx(), tab, None);
        }
    }
    let dislike = current.filter(|_| state.prefs.dislike_button).map(|t| (t.id, state.dislikes.track(t.id)));
    let volume_row = aui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let volume = if p.muted { 0.0 } else { chrome.volume_now(&ctx, state) };
        // The slider is optional; the wheel sets the volume either way.
        if state.prefs.np_volume {
            let buttons = if dislike.is_some() { 96.0 } else { 48.0 };
            let w = (ui.available_width() - buttons).clamp(96.0, 220.0);
            let vo = Slider::new("np-vol", volume)
                .track(8.0)
                .width(w)
                .value_label(|v| format!("{:.0}%", v * 100.0))
                .show(ui);
            if let Some(v) = vo.dragging.or(vo.committed) {
                chrome.push_volume(state, v, out);
            }
        }
        if ui
            .add(
                IconButton::new(Icon::volume(volume, p.muted))
                    .tooltip("Звук (колесо мыши — громкость)")
                    .id_salt("np-mute"),
            )
            .clicked()
        {
            out.push(Command::ToggleMute);
        }
        if let Some((track, disliked)) = dislike
            && ui
                .add(IconButton::new(Icon::ThumbDown).selected(disliked).tooltip("Не нравится").id_salt("np-dislike"))
                .clicked()
        {
            out.push(Command::Dislike { track, dislike: !disliked, sure: false, relike: false });
        }
    });
    part(Part::Volume, volume_row.response.rect);
}

/// Between the artist line and the speed mark.
const SPEED_MARK_GAP: f32 = 8.0;
/// Height of the speed mark and of the line it sits on.
const SPEED_MARK_H: f32 = 18.0;
const SPEED_LINE_H: f32 = 24.0;

/// The mark of the playing track's version («×1.2», «замедленно»), when it
/// is not played as recorded.
fn speed_mark_of(state: &ViewState, track: nikgob_view::TrackId) -> Option<std::borrow::Cow<'static, str>> {
    // A live track of another player plays as recorded whatever was chosen.
    if state.player.speed == 1.0 {
        return None;
    }
    state.speeds.of(track).chip()
}

fn speed_mark_width(ui: &Ui, text: &str) -> f32 {
    let galley = ui.painter().layout_no_wrap(text.to_owned(), TypeRole::LabelSmall.font_id(), Color32::WHITE);
    (galley.size().x + 14.0).ceil()
}

/// A quiet outlined mark: no fill, the colour of the line it ends. A press
/// on it opens the track's speed, so it takes the states of a button.
fn speed_mark(ui: &mut Ui, text: &str) -> egui::Response {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let galley = ui.painter().layout_no_wrap(text.to_owned(), TypeRole::LabelSmall.font_id(), s.on_surface_variant);
    let size = vec2((galley.size().x + 14.0).ceil(), SPEED_LINE_H);
    let (line, response) = ui.allocate_exact_size(size, Sense::click());
    let rect = Rect::from_center_size(line.center(), vec2(size.x, SPEED_MARK_H));
    let layer = if response.is_pointer_button_down_on() {
        crate::tokens::state::PRESSED
    } else if response.hovered() {
        crate::tokens::state::HOVER
    } else {
        0.0
    };
    let layer = anim::fraction(ui.ctx(), response.id.with("layer"), layer, theme.motion.fast_effects);
    let p = ui.painter();
    paint::state_layer(p, rect, Radii::all(SPEED_MARK_H * 0.5), s.on_surface_variant, layer);
    p.rect_stroke(rect, SPEED_MARK_H * 0.5, Stroke::new(1.0, s.outline_variant), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() * 0.5, galley, s.on_surface_variant);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Скорость: {text}")));
    crate::components::tooltip(response.on_hover_cursor(egui::CursorIcon::PointingHand), "Скорость", Some("S"))
}

/// Background: soft colour clouds drifting over the window, breathing with

/// the bass in reactive mode (effects spring, no overshoot).
fn ambient(ui: &Ui, rect: Rect, state: &ViewState, levels_bass: f32, stage: &Stage) {
    let theme = Theme::get(ui.ctx());
    let mode = state.prefs.ambient;
    if mode == nikgob_view::Ambient::Off {
        return;
    }
    let playing = state.player.status == Status::Playing;
    let bass = if mode == nikgob_view::Ambient::Reactive && playing {
        levels_bass
    } else {
        0.0
    };
    let level = anim::fraction(ui.ctx(), stage.id("np-glow"), bass, theme.motion.fast_effects);
    // (The drift is shared with the real screen: one clock for both.)
    super::ambient::paint(ui.ctx(), ui.painter(), rect, level.clamp(0.0, 1.0), playing && stage.moving, state.prefs.ambient_strength);
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_album_gives_way_before_the_artists() {
        let w = super::LineWidths { artist: 120.0, first: 60.0, sep: 12.0, album: 200.0 };
        assert_eq!(super::split_line(400.0, w), (120.0, Some(212.0)), "both whole");
        assert_eq!(super::split_line(200.0, w), (120.0, Some(80.0)), "the album truncates");
        assert_eq!(super::split_line(150.0, w), (90.0, Some(60.0)), "then the artists");
        assert_eq!(super::split_line(80.0, w), (60.0, None), "the first artist stays, the album goes");
        assert_eq!(super::split_line(40.0, w), (40.0, None));
        let alone = super::LineWidths { album: 0.0, ..w };
        assert_eq!(super::split_line(100.0, alone), (100.0, None));
    }
    use super::*;

    #[test]
    fn the_circle_ring_rounds_the_square_inside_the_circles_box() {
        let rect = Rect::from_center_size(pos2(300.0, 300.0), vec2(400.0, 400.0));
        let (gap, circle) = (14.0, 214.0);
        let dir = |i: usize| vec2((i as f32 * TAU / BINS as f32).sin(), -(i as f32 * TAU / BINS as f32).cos());
        // Around a shape: the circle itself.
        let shape = Outline::new(Some(MaterialShape::Cookie12Sided), 1.0, rect, 0.4);
        assert!(circle_path(&shape.radii(), circle, gap).iter().all(|r| (r - circle).abs() < 1.0));
        // Around the square (and half-way to it): a gap off every corner, and
        // never farther out at the sides than the circle.
        for k in [0.0, 0.5] {
            let o = Outline::new(Some(MaterialShape::Cookie12Sided), k, rect, 0.0);
            let radii = o.radii();
            let path = circle_path(&radii, circle, gap);
            for i in 0..BINS {
                assert!(path[i] >= radii[i] + gap - 0.01);
                let p = dir(i) * path[i];
                assert!(p.x.abs() <= circle + 1.0 && p.y.abs() <= circle + 1.0, "{k} {i}: {p:?}");
            }
        }
        let square = circle_path(&Outline::new(None, 1.0, rect, 0.0).radii(), circle, gap);
        assert!(square[45] > circle + 50.0, "the corners go round, not under, the dots");
        // The dots keep the circle's speed: a quarter turn moves them a
        // quarter of the circle's dots, however long the path.
        let ctx = egui::Context::default();
        let key = egui::Id::new("t");
        assert_eq!(dot_phase(&ctx, key, 6.0, 72.0 / TAU), 0.0);
        let u = dot_phase(&ctx, key, 6.0 + 0.25 * TAU / 72.0, 72.0 / TAU);
        assert!((u - 0.25).abs() < 1e-3);
        // Across the wrap of the angle, no jump.
        let u = dot_phase(&ctx, key, 0.01, 72.0 / TAU);
        let expect = (0.25 + (0.01 + TAU - 6.0 - 0.25 * TAU / 72.0) * 72.0 / TAU).rem_euclid(1.0);
        assert!((u - expect).abs() < 1e-3, "{u} vs {expect}");
    }

    #[test]
    fn only_the_shape_itself_is_the_cover() {
        let rect = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 200.0));
        for shape in [MaterialShape::Cookie9Sided, MaterialShape::Clover4Leaf, MaterialShape::Sunny] {
            let o = Outline::new(Some(shape), 1.0, rect, 0.0);
            assert!(o.contains(rect.center()), "{shape:?} centre");
            // The square's corners are outside every rounded shape.
            for c in [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom()] {
                let inward = c + (rect.center() - c) * 0.06;
                assert!(!o.contains(inward), "{shape:?} corner {inward:?}");
            }
            assert!(!o.contains(pos2(400.0, 200.0)));
        }
    }

    #[test]
    fn unwinding_takes_at_most_half_a_lobe() {
        for n in [1u32, 2, 4, 7, 8, 12, 16] {
            let lobe = TAU / n as f32;
            for i in 0..720 {
                let spin = i as f32 / 720.0 * TAU;
                let r = unwind(spin, n);
                assert!(r.abs() <= lobe * 0.5 + 1e-4, "n={n} spin={spin} r={r}");
                // Same pose: they differ by whole lobes only.
                let lobes = (spin - r) / lobe;
                assert!((lobes - lobes.round()).abs() < 1e-3, "n={n} spin={spin} r={r}");
            }
        }
        // A spin just short of a revolution is a small step back, not 340°.
        assert!(unwind(TAU - 0.05, 12).abs() < 0.06);
        assert!(unwind(TAU - 0.05, 1).abs() < 0.06);
        assert!(unwind(0.0, 12).abs() < 1e-6);
    }

    #[test]
    fn a_pause_never_jumps_the_half_way_shape() {
        let ctx = egui::Context::default();
        let lopsided = Some(MaterialShape::Heart);
        assert_eq!(symmetry(lopsided), 1);
        let mut last = None;
        // Playing just short of a half turn, then the pause morph starts and
        // the braking spin crosses it, and then crosses TAU too.
        for (i, (spin, morph)) in [(3.10, 1.0), (3.12, 0.95), (3.16, 0.9), (3.20, 0.8), (6.27, 0.5), (0.02, 0.4)]
            .into_iter()
            .enumerate()
        {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(i as f64 / 60.0), ..Default::default() }, |ui| {
                let a = cover_angle(ui, Stage::REAL.id("np-cover-lobes"), lopsided, spin, morph);
                if let Some(prev) = last {
                    let step: f32 = a - prev;
                    let expected = (spin - [3.10f32, 3.12, 3.16, 3.20, 6.27, 0.02][i.saturating_sub(1)] + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
                    if i > 0 && morph < 0.999 {
                        assert!((step - expected).abs() < 1e-4, "frame {i}: {prev} -> {a}");
                    }
                }
                last = Some(a);
            });
            out.textures_delta.clear();
        }
    }

    #[test]
    fn bars_follow_a_loud_track_and_rest_in_silence() {
        let mut c = Cava::default();
        // A loud track: every band high, one peak in the bass. The bars
        // still tell the peak apart instead of all sitting at the top.
        let mut bands = vec![0.8; 64];
        bands[4] = 1.0;
        let mut t = Vec::new();
        for _ in 0..120 {
            t = cava_targets(&mut c, &bands, true, 0.05, 0.05);
        }
        assert!(t[3] > 0.9, "{t:?}");
        assert!(t[BAR_COUNT - 1] < 0.5, "{t:?}");
        // One pass around the circle: no mirror, the highs end it.
        let mut high = vec![0.2; 64];
        high[63] = 1.0;
        let mut c = Cava::default();
        let t = cava_targets(&mut c, &high, true, 0.05, 0.05);
        assert!(t[BAR_COUNT - 1] > 0.9 && t[0] < 0.2, "{t:?}");
        // Silence and pause: nothing.
        assert!(cava_targets(&mut c, &[0.01; 64], true, 0.05, 0.05).iter().all(|v| *v == 0.0));
        assert!(cava_targets(&mut c, &bands, false, 0.05, 0.05).iter().all(|v| *v == 0.0));
    }

    #[test]
    fn every_pair_of_shapes_morphs() {
        // `Morph::new` panics on polygons it cannot match; the cover swaps
        // between any two cover shapes, the background between its pool.
        let mut shapes: Vec<MaterialShape> = CoverShape::shapes().iter().filter_map(|c| material(*c)).collect();
        shapes.extend(super::super::bg_shapes::pool());
        shapes.sort();
        shapes.dedup();
        for &a in &shapes {
            for &b in &shapes {
                if a != b {
                    assert!(!pair_morph(a, b).as_cubics(0.5).is_empty(), "{a:?} -> {b:?}");
                }
            }
        }
    }

    #[test]
    fn random_range_and_reroll() {
        let ctx = egui::Context::default();
        let range = [CoverShape::Heart, CoverShape::Gem];
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let shape = cover_shape(&ctx, CoverShape::Random, &range, None).expect("a shape");
            assert!(matches!(shape, MaterialShape::Heart | MaterialShape::Gem));
            // Stays put until a reroll, then never the same twice.
            assert_eq!(cover_shape(&ctx, CoverShape::Random, &range, None), Some(shape));
            reroll_shape(&ctx);
            let next = cover_shape(&ctx, CoverShape::Random, &range, None).expect("a shape");
            assert_ne!(next, shape);
            seen.insert(next);
        }
        assert_eq!(seen.len(), 2);
        assert_eq!(random_pool(&[]).len(), CoverShape::shapes().len());
    }

    #[test]
    fn shapes_look_the_same_after_a_lobe() {
        // The orders in `symmetry` against the real outlines: turning the
        // playing shape by one lobe must not change it.
        let rect = Rect::from_center_size(pos2(0.0, 0.0), vec2(200.0, 200.0));
        for choice in CoverShape::shapes() {
            let shape = material(*choice);
            let lobe = TAU / symmetry(shape) as f32;
            let (a, b) = (Outline::new(shape, 1.0, rect, 0.0).radii(), Outline::new(shape, 1.0, rect, lobe).radii());
            let worst = a.iter().zip(&b).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(worst < 3.0, "{shape:?}: radii differ by {worst}");
        }
    }

    #[test]
    fn switching_slots_crossfades_without_moving_the_cover() {
        let ctx = egui::Context::default();
        for reduced in [false, true] {
            Theme::new(crate::ColorScheme::baseline(true), true).with_reduced_motion(reduced).install(&ctx);
            for (time, slot, expected) in [
                (0.0, NpSlot::Lyrics, Some([1.0, 0.0, 0.0, 0.0])),
                (1.0, NpSlot::Queue, Some([1.0, 0.0, 0.0, 0.0])),
                (1.05, NpSlot::Queue, None),
                (2.0, NpSlot::Queue, Some([0.0, 1.0, 0.0, 0.0])),
                (3.0, NpSlot::None, Some([0.0, 1.0, 0.0, 0.0])),
                (4.0, NpSlot::Info, Some([0.0, 0.0, 1.0, 0.0])),
            ] {
                let mut output = ctx.run_ui(egui::RawInput { time: Some(time + if reduced { 10.0 } else { 0.0 }), ..Default::default() }, |ui| {
                    let alpha = slot_opacities(ui.ctx(), slot, &Stage::REAL);
                    if let Some(expected) = expected { assert_eq!(alpha, expected); }
                    else { assert!(alpha[0] > 0.0 && alpha[1] > 0.0); }
                    assert!((alpha.iter().sum::<f32>() - 1.0).abs() < 0.001);
                    if slot != NpSlot::None {
                        let k = anim::fraction(ui.ctx(), egui::Id::new("np-mega"), 1.0, Theme::get(ui.ctx()).motion.default_spatial);
                        assert_eq!(k, 1.0);
                    }
                });
                output.textures_delta.clear();
            }
            ctx.data_mut(|d| d.remove::<(NpSlot, Animated<[f32; 4]>)>(egui::Id::new("np-content-fade")));
        }
    }

    #[test]
    fn mega_columns_and_thumbnail_leave_lyrics_and_controls_room() {
        for width in [360.0, 800.0, 899.0, 900.0, 1280.0, 1800.0] {
            let body = Rect::from_min_size(pos2(24.0, 80.0), vec2(width, 780.0));
            let layout = mega_layout(body);
            assert!(body.contains_rect(layout.cover));
            assert!(body.contains_rect(layout.info));
            assert!(body.contains_rect(layout.content));
            assert!(layout.content.is_positive());
            assert!(!layout.info.intersects(layout.content));
            if layout.wide {
                // Halves: the cover column is centred in the left half, the
                // content starts right of the centre line.
                let mid = body.center().x;
                assert!(layout.info.right() < mid && layout.content.left() > mid);
                let col_mid = layout.info.center().x;
                assert!((col_mid - (body.left() + mid) * 0.5).abs() < (body.width() * 0.05).clamp(24.0, 96.0), "{width}");
                assert!(layout.cover.width() <= 440.0);
            } else {
                assert_eq!(layout.cover.size(), vec2(56.0, 56.0));
                assert_eq!(layout.info.bottom(), body.bottom());
            }
            assert_eq!(lerp_rect(body, layout.cover, 0.0), body);
            assert_eq!(lerp_rect(body, layout.cover, 1.0), layout.cover);
            // The lyrics start just under the header (16 px above the body)
            // or the small cover's row, never touching either.
            let lyrics_top = layout.content.top() - layout.lyrics_lift;
            let above = if layout.wide { body.top() - 16.0 } else { layout.cover.bottom() };
            assert_eq!(lyrics_top, above + LYRICS_TOP_GAP, "{width}");
        }
    }
}
