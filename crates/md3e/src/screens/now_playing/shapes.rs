//! Cover shape geometry: the shape a [`CoverShape`] stands for, the random
//! pick, the morph caches, the outline the ring is built around, and the
//! turn it takes as it morphs.

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use egui::{Color32, Mesh, Pos2, Rect, Shape, Ui, pos2, vec2};
use nikgob_shapes::{CornerRounding, Cubic, MaterialShape, Morph, Point, RoundedPolygon, paint as shape_paint};
use nikgob_view::{Command, CoverShape, TrackRow};

use crate::{
    Icon, Theme,
    components::anim,
    screens::stage::Stage,
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
pub(super) fn cover_shape_in(
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
pub(super) fn cover_morph(shape: MaterialShape) -> &'static Morph {
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
    pub(super) rect: Rect,
    rotation: f32,
}

/// Bins of [`Outline::radii`]: one per degree.
pub(super) const BINS: usize = 360;

impl Outline {
    pub fn new(shape: Option<MaterialShape>, progress: f32, rect: Rect, rotation: f32) -> Self {
        // The square cover never turns.
        let (shape, progress, rotation) = match shape {
            Some(shape) => (shape, progress.clamp(0.0, 1.0), rotation),
            None => (MaterialShape::Cookie12Sided, 0.0, 0.0),
        };
        Self {
            cubics: cover_morph(shape).as_cubics(progress),
            rect,
            rotation,
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
    pub(crate) fn radii(&self) -> Vec<f32> {
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
        let opts = shape_paint::FillOptions::for_ppp(crate::screens::stage::ppp(ui.ctx()));
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

/// How many times the playing `shape` maps onto itself in one revolution
/// (its lobes), read off the definitions in `nikgob_shapes`: the cookies and
/// clovers are named by it, `repeat`ed custom shapes by their `reps`,
/// the lopsided ones (pentagon, gem, heart) have none. `None`, the square
/// cover, never turns.
pub(super) fn symmetry(shape: Option<MaterialShape>) -> u32 {
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

/// The morph counts as done (the shape reached) from here.
const SHAPED: f32 = 0.999;

/// Where a pause's morph starts from: the playing angle `angle` of a
/// shape of `n` lobes as a quarter turn (`base`, which the square looks
/// the same at) and the `twist` left over, the smallest over the shape's
/// lobes: the outline turns by `twist` alone while it becomes the square.
pub(super) fn square_pose(angle: f32, n: u32) -> (f32, f32) {
    let quarter = TAU / 4.0;
    let lobe = TAU / n.max(1) as f32;
    let mut best = (0.0, f32::MAX);
    for k in 0..n.max(1) {
        let pose = angle + k as f32 * lobe;
        let q = (pose / quarter).round();
        let twist = pose - q * quarter;
        if twist.abs() < best.1.abs() {
            best = ((q * quarter).rem_euclid(TAU), twist);
        }
    }
    best
}

/// The turn of the cover outline. The shape turns only once the morph has
/// reached it, its speed growing from nothing (`ramp`); through a morph
/// (a pause or a resume) it does not spin: a pause turns it the least it
/// must to land on the upright square ([`square_pose`]), and a resume
/// grows the shape straight out of the square, upright.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CoverTurn {
    seen: bool,
    /// The turn's spin last frame.
    spin: f32,
    pub(super) angle: f32,
    /// The shape is reached and turns with the spin.
    shaped: bool,
    /// The morph's pose: `base + twist * morph`.
    base: f32,
    twist: f32,
}

impl CoverTurn {
    /// One frame: `spin` the turn's angle, `morph` the cover's morph (1:
    /// the shape, 0: the square), `toward` whether it heads to the shape,
    /// `ramp` (0..1) how much of the spin's step the shape takes, `n` the
    /// shape's lobes. The angle the outline is turned by.
    pub(super) fn step(&mut self, n: u32, spin: f32, morph: f32, toward: bool, ramp: f32) -> f32 {
        let step = if self.seen { (spin - self.spin + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI } else { 0.0 };
        self.seen = true;
        self.spin = spin;
        let m = morph.clamp(0.0, 1.0);
        if self.shaped && !toward {
            // A pause, before the morph moves: whole lobes of the shape
            // look the same, so its pose is taken where the square is
            // nearest.
            self.shaped = false;
            (self.base, self.twist) = square_pose(self.angle, n);
        } else if !self.shaped && toward && m >= SHAPED {
            self.shaped = true;
            self.angle = self.base + self.twist;
        }
        if self.shaped {
            // A whole turn looks the same: kept small.
            self.angle = (self.angle + step * ramp.clamp(0.0, 1.0)).rem_euclid(TAU);
        } else {
            if m <= 1.0 - SHAPED {
                // The square: any quarter turn of it is the upright one,
                // and the next shape grows from it upright.
                (self.base, self.twist) = (0.0, 0.0);
            }
            self.angle = self.base + self.twist * m;
        }
        self.angle
    }
}

/// The angle the cover outline is turned by ([`CoverTurn`]); `toward`:
/// the morph heads to the shape.
pub(super) fn cover_angle(ui: &Ui, id: egui::Id, shape: Option<MaterialShape>, spin: f32, morph: f32, toward: bool) -> f32 {
    let ctx = ui.ctx();
    let mut turn: CoverTurn = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    // The spin comes in only once the shape is reached, gently.
    let ramp = anim::fraction(
        ctx,
        id.with("ramp"),
        if toward && (turn.shaped || morph >= SHAPED) { 1.0 } else { 0.0 },
        Theme::get(ctx).motion.slow_effects,
    );
    let angle = turn.step(symmetry(shape), spin, morph, toward, ramp);
    ctx.data_mut(|d| d.insert_temp(id, turn));
    angle
}

/// The cover outline turning into another shape (a new random shape, a
/// double click): `(from, to, t)` while it lasts. Only while the outline is
/// fully the playing shape (`live`): a change during a pause or its morph
/// just shows the new shape, and a pause drops a swap under way, so a
/// resume never jumps back to a half-way blend. Remembers the last shape
/// shown, so call it every frame.
pub(super) fn shape_swap(
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
pub(super) fn change_shape(ctx: &egui::Context, prefs: &nikgob_view::Prefs, out: &mut Vec<Command>) {
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
