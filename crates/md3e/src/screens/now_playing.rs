//! Now Playing (md3e-player.md §3.5): morphing cover inside a dotted ring
//! that turns and follows the spectrum, wavy progress, the hero play button
//! in a standard button group, the supporting pane, and the ambient
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
use nikgob_view::{Command, CoverShape, LikeTarget, Prefs, Repeat, RingStyle, Status, TrackRow, ViewState};

use super::{
    chrome::{self, Act, Chrome, WheelZone, Where},
    common,
    pages::PageState,
    pane,
};
use crate::{
    Icon, SizeClass, Theme, TypeRole,
    components::{
        BottomSheet, ButtonGroup, IconButton, Item, Size, Slider, Style, anim,
        button::{Colors, Shape as BtnShape},
        paint::{self, Radii},
    },
    size_class,
    tokens::shape,
};

/// The shape a [`CoverShape`] stands for; `None` keeps the square cover.
fn material(shape: CoverShape) -> Option<MaterialShape> {
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

/// The outline for `track` under the user's choice: a fixed shape, none,
/// or a random one drawn whenever another track starts (nothing is
/// remembered per track, and never the same shape twice in a row).
pub fn cover_shape(ctx: &egui::Context, choice: CoverShape, track: Option<&TrackRow>) -> Option<MaterialShape> {
    if choice != CoverShape::Random {
        return material(choice);
    }
    let id = egui::Id::new("np-random-shape");
    let all = CoverShape::shapes();
    let key = track.map(|t| t.id);
    let (seen, mut pick): (Option<nikgob_view::TrackId>, usize) =
        ctx.data(|d| d.get_temp(id)).unwrap_or((None, usize::MAX));
    if seen != key || pick >= all.len() {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::time::SystemTime::now().hash(&mut h);
        key.hash(&mut h);
        let mut next = (h.finish() % all.len() as u64) as usize;
        if next == pick {
            next = (next + 1) % all.len();
        }
        pick = next;
        ctx.data_mut(|d| d.insert_temp(id, (key, pick)));
    }
    material(all[pick])
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
fn centred(polygon: &RoundedPolygon) -> RoundedPolygon {
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
            // Unwinds to upright as it turns back into the square.
            rotation: rotation * progress,
        }
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

    /// The cover texture cropped to the outline. The picture itself never
    /// turns, only its outline.
    pub fn paint(&self, ui: &Ui, uri: Option<&str>) {
        let s = Theme::get(ui.ctx()).scheme;
        let rect = self.rect;
        let texture = uri.and_then(|u| {
            egui::Image::new(u.to_owned())
                .texture_options(crate::components::PHOTO_TEXTURE)
                .load_for_size(ui.ctx(), rect.size())
                .ok()
                .and_then(|p| p.texture_id())
        });
        let opts = shape_paint::FillOptions::for_ppp(ui.ctx().pixels_per_point());
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
    /// Smoothed band levels, for the bars' resting length.
    levels: &'a [f32],
    kick: f32,
    alpha: f32,
}

/// The ring around the cover: dots on a soft 12-lobed circle, dots riding
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
    match input.style {
        RingStyle::Circle => {
            let n = 72;
            let radius = outline.rect.width() * 0.5 + 14.0;
            for i in 0..n {
                let u = i as f32 / n as f32;
                let theta = input.angle + u * TAU;
                let level = band_at(input.transients, theta);
                let r = radius + 3.0 * (12.0 * u * TAU).cos() + level * 16.0 + kick * 8.0;
                let dot = 2.0 + level * 2.0 + kick * 0.8;
                let a = input.alpha * (0.45 + 0.55 * level.max(kick * 0.6));
                painter.circle_filled(center + dir(theta) * r, dot, s.primary.gamma_multiply(a));
            }
        }
        RingStyle::Contour => {
            // The outline pushed out by a gap, then dots spaced evenly along
            // its length (not by angle: a square's corners would thin out),
            // sliding along it as the ring turns.
            let radii = outline.radii();
            let gap = 14.0;
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
            let spacing = 11.0;
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
                let out = dir(theta) * (level * 14.0 + kick * 7.0);
                let dot = 2.0 + level * 1.8 + kick * 0.8;
                let alpha = input.alpha * (0.5 + 0.5 * level.max(kick * 0.6));
                painter.circle_filled(p + out, dot, s.primary.gamma_multiply(alpha));
            }
        }
        RingStyle::Bars => {
            // As in caelestia's cover visualiser: bars along the outline's
            // normal-ish (radial) direction, starting a gap away from it.
            let radii = outline.radii();
            let n = 64;
            let gap = 10.0;
            let r_mid = outline.rect.width() * 0.5 + gap;
            let width = (TAU * r_mid / n as f32 * 0.42).clamp(2.5, 6.0);
            let stroke = Stroke::new(width, s.primary.gamma_multiply(input.alpha * 0.9));
            for i in 0..n {
                let theta = input.angle + i as f32 / n as f32 * TAU;
                let bin = ((theta / TAU).rem_euclid(1.0) * BINS as f32) as usize % BINS;
                let rest = ((band_at(input.levels, theta) - 0.45) / 0.55).clamp(0.0, 1.0).powi(2);
                let level = band_at(input.transients, theta).max(rest * 0.7);
                let len = 2.0 + level * 20.0 + kick * 5.0;
                let d = dir(theta);
                let a = center + d * (radii[bin] + gap + width * 0.5);
                let b = a + d * len;
                painter.line_segment([a, b], stroke);
                painter.circle_filled(a, width * 0.5, stroke.color);
                painter.circle_filled(b, width * 0.5, stroke.color);
            }
        }
    }
}

/// An angle that turns one revolution per `period` seconds (faster with
/// the bass, backwards when `ccw`) only while playing: a pause brakes it
/// in a fraction of a second where it is, and play goes on from there.
/// `period` 0 = still.
fn turn(ui: &Ui, key: &str, playing: bool, bass: f32, period: f32, ccw: bool) -> f32 {
    let ctx = ui.ctx();
    let id = egui::Id::new(key);
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
/// and per-band transients (0..1).
fn pulse(ui: &Ui, bass: f32, bands: &[f32]) -> (f32, Vec<f32>) {
    let ctx = ui.ctx();
    let id = egui::Id::new("np-pulse");
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
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let p = &state.player;
    let playing = p.status == Status::Playing;
    let mut closed = false;

    // The layer must know its size, or the wheel scrolls the page under it.
    ui.expand_to_include_rect(rect);
    // Swallow input under the overlay; a click on empty space or a swipe
    // down closes it.
    let bg = ui.interact(rect, ui.id().with("np-bg"), Sense::click_and_drag());
    if !embedded {
        if bg.clicked() {
            closed = true;
        }
        if bg.drag_stopped()
            && let Some(d) = bg.total_drag_delta()
            && d.y > 96.0
            && d.y > d.x.abs() * 1.5
        {
            closed = true;
        }
    }
    ambient(ui, rect, state, pages);

    let class = size_class(rect.width());
    let margin = class.margin();
    let inner = rect.shrink2(vec2(margin, 16.0));
    let two_col = class >= SizeClass::Expanded;
    if !chrome.np_loaded {
        chrome.np_loaded = true;
        chrome.np_sheet = state.prefs.np_pane;
        chrome.np_saved = Some(state.prefs.np_pane);
    }
    if two_col {
        chrome.np_modal = false;
    }
    // The wheel turns the volume anywhere but the pane (blocked below) and
    // the bottom sheet, which scrolls.
    if !chrome.np_modal {
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
    let ctx_text = p
        .context
        .as_deref()
        .map_or_else(|| "Сейчас играет".to_owned(), |c| format!("Играет из «{c}»"));
    let sheet_open = if two_col { chrome.np_sheet } else { chrome.np_modal };
    hui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let more = ui.add(IconButton::new(Icon::MoreVert).tooltip("Ещё").id_salt("np-more"));
        if let Some(t) = p.current.as_ref() {
            let acts = [
                Act::Playlist,
                Act::Artist,
                Act::Album,
                Act::Info,
                Act::Tags,
                Act::Replace,
                Act::Visualizer,
                Act::Sep,
                Act::Trash,
            ];
            let wh = Where::Np { tab: &mut pages.np_tab };
            chrome.track_menu(&more, "np-more", &acts, t, state, wh, out);
        }
        // Queue, lyrics and info: a side column when it fits, a bottom
        // sheet otherwise.
        if ui
            .add(
                IconButton::new(Icon::QueueMusic)
                    .style(Style::Tonal)
                    .selected(sheet_open)
                    .tooltip("Очередь, текст, инфо")
                    .id_salt("np-sheet-btn"),
            )
            .clicked()
        {
            if two_col {
                chrome.np_sheet = !chrome.np_sheet;
            } else {
                chrome.np_modal = !chrome.np_modal;
            }
        }
        ui.with_layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
            common::label(ui, &ctx_text, TypeRole::TitleSmall, s.on_surface_variant);
        });
    });

    // The pane slides in from the right edge (no bounce: it moves the
    // whole layout).
    let body = Rect::from_min_max(pos2(inner.left(), header.bottom() + 16.0), inner.max);
    let pane_full = if class >= SizeClass::ExtraLarge { 412.0 } else { 360.0 };
    let pane_k = anim::fraction(
        &ctx,
        egui::Id::new("np-pane-open"),
        if two_col && chrome.np_sheet { 1.0 } else { 0.0 },
        theme.motion.default_spatial.without_bounce(),
    )
    .clamp(0.0, 1.0);
    let pane_w = (pane_full + 24.0) * pane_k;
    let main = Rect::from_min_max(body.min, pos2(body.right() - pane_w, body.bottom()));
    if two_col && pane_k > 0.01 {
        let pr = snap(
            ui,
            Rect::from_min_size(
                pos2(body.right() - pane_w + 24.0 * pane_k, body.top()),
                vec2(pane_full, body.height()),
            ),
        );
        let clip = Rect::from_min_max(pr.min, pos2(rect.right(), pr.bottom()));
        // Clicks on the pane's empty parts must not close Now Playing, and
        // the wheel scrolls it.
        ui.interact(clip.intersect(pr), ui.id().with("np-pane-bg"), Sense::click());
        chrome.wheel_block(clip.intersect(pr));
        let painter = ui.painter().with_clip_rect(clip);
        painter.rect_filled(pr, shape::EXTRA_LARGE as u8, s.surface_container_low.gamma_multiply(0.85 * pane_k));
        let mut pui = ui.new_child(UiBuilder::new().max_rect(pr.shrink(16.0)).id_salt("np-pane"));
        pui.set_clip_rect(clip.intersect(pr).intersect(ui.clip_rect()));
        pui.multiply_opacity(pane_k);
        pane::show(&mut pui, state, &mut pages.np_tab, chrome, out);
    }

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

    // Cover: playing → Cookie12Sided, paused → rounded square; the ring
    // around it fades in with the cookie.
    let morph = anim::fraction(
        &ctx,
        egui::Id::new("np-cover-morph"),
        if playing { 1.0 } else { 0.0 },
        theme.motion.default_spatial,
    );
    let prefs = &state.prefs;
    let motion = !theme.reduced_motion;
    let cover_rect = cover_box.shrink(RING_PAD);
    let bass = if playing { pages.levels_bass } else { 0.0 };
    let (kick, transients) = if playing && motion {
        pulse(ui, bass, &chrome.bands)
    } else {
        (0.0, Vec::new())
    };
    let shown = morph.clamp(0.0, 1.0);
    let current = p.current.as_ref();
    let shape = cover_shape(&ctx, prefs.cover_shape, current);
    let uri = current.and_then(|t| t.cover.as_ref()).map(|c| c.uri());
    let period = if prefs.cover_spin && motion { 48.0 } else { 0.0 };
    let spin = turn(ui, "np-cover-angle", playing, bass, period, prefs.cover_ccw);
    let swell = if prefs.cover_pulse { 1.0 + 0.035 * kick } else { 1.0 };
    let shaped = Rect::from_center_size(cover_rect.center(), cover_rect.size() * swell);
    let outline = Outline::new(shape, morph, shaped, spin);
    if prefs.ring {
        let period = if prefs.spin_ring && motion { 36.0 } else { 0.0 };
        let angle = turn(ui, "np-ring-angle", playing, bass, period, prefs.ring_ccw);
        let beat = prefs.ring_beat && playing;
        ring(
            ui,
            &outline,
            &RingInput {
                style: prefs.ring_style,
                angle,
                transients: if beat { &transients } else { &[] },
                levels: if beat { &chrome.bands } else { &[] },
                kick: if beat { kick } else { 0.0 },
                // The contour ring stays around the square too; the
                // circle one only belongs to a shape.
                alpha: if prefs.ring_style == RingStyle::Circle { shown } else { 0.35 + 0.65 * shown },
            },
        );
    }
    outline.paint(ui, uri.as_deref());
    ui.interact(cover_rect, ui.id().with("np-cover"), Sense::click());
    if playing && !theme.reduced_motion {
        ctx.request_repaint();
    }

    let info_rect = snap(ui, info_rect);
    ui.interact(info_rect, ui.id().with("np-info-bg"), Sense::click());
    info(ui, info_rect, wide, state, pages, chrome, out);

    // Single column: the pane lives in a bottom sheet.
    if !two_col {
        let mut open = chrome.np_modal;
        BottomSheet::new("np-sheet").show(&ctx, &mut open, |ui| {
            pane::show(ui, state, &mut pages.np_tab, chrome, out);
        });
        chrome.np_modal = open;
    }

    // Remember whether the column was left open.
    if two_col && chrome.np_saved != Some(chrome.np_sheet) {
        chrome.np_saved = Some(chrome.np_sheet);
        out.push(Command::SetPrefs(Box::new(Prefs {
            np_pane: chrome.np_sheet,
            ..state.prefs.clone()
        })));
    }

    closed
}

/// Height of the text and controls block.
const INFO_H: f32 = TITLE_H + 4.0 + 24.0 + 24.0 + 64.0 + PROGRESS_GAP + CONTROLS_H + 16.0 + 48.0;
/// Air between the progress bar and the transport row.
const PROGRESS_GAP: f32 = 28.0;
const TITLE_H: f32 = 44.0;
const CONTROLS_H: f32 = 96.0;

/// Title, artist · album, progress, transport and the action row (like,
/// add to playlist, volume).
fn info(
    ui: &mut Ui,
    rect: Rect,
    wide: bool,
    state: &ViewState,
    pages: &mut PageState,
    chrome: &mut Chrome,
    out: &mut Vec<Command>,
) {
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    let p = &state.player;
    let playing = p.status == Status::Playing;
    let current = p.current.as_ref();
    let col = rect;
    let mut y = col.top();

    // Title and links.
    let text_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), TITLE_H + 4.0 + 24.0));
    let mut tui = ui.new_child(UiBuilder::new().max_rect(text_rect).id_salt("np-text"));
    tui.spacing_mut().item_spacing.y = 4.0;
    let title_role = if wide { TypeRole::DisplaySmall } else { TypeRole::HeadlineLarge };
    match current {
        Some(t) => {
            tui.add(egui::Label::new(title_role.emphasized().rich(&t.title, s.on_surface)).truncate());
            tui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let link = |ui: &mut Ui, text: &str| {
                    let r = ui.add(
                        egui::Label::new(TypeRole::TitleMedium.style().rich(text, s.on_surface_variant))
                            .truncate()
                            .sense(Sense::click()),
                    );
                    r.on_hover_cursor(egui::CursorIcon::PointingHand)
                };
                if link(ui, &t.artist).clicked() {
                    chrome.nav = t.artist_id.map(nikgob_view::Route::Artist);
                }
                if let Some(a) = &t.album {
                    ui.label(TypeRole::TitleMedium.style().rich(" · ", s.on_surface_variant));
                    if link(ui, a).clicked() {
                        chrome.nav = t.release_id.map(nikgob_view::Route::Release);
                    }
                }
            });
        }
        None => {
            tui.label(title_role.style().rich("Ничего не играет", s.on_surface));
        }
    }
    y += TITLE_H + 4.0 + 24.0 + 24.0;

    // Progress.
    let dur = p.duration.unwrap_or_default();
    let prog_rect = Rect::from_min_size(pos2(col.left(), y), vec2(col.width(), 64.0));
    let mut pui = ui.new_child(UiBuilder::new().max_rect(prog_rect).id_salt("np-progress"));
    let so = Slider::new("np-slider", p.fraction_now())
        .width(col.width())
        .wavy(playing)
        .value_label(move |v| common::fmt_time(dur.mul_f32(v)))
        .show(&mut pui);
    if let Some(v) = so.committed {
        out.push(Command::Seek(dur.mul_f32(v)));
    }
    let shown = so.dragging.map_or_else(|| p.position_now(), |v| dur.mul_f32(v));
    pui.horizontal(|ui| {
        ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(shown), s.on_surface_variant));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(TypeRole::LabelMedium.style().rich(common::fmt_time(dur), s.on_surface_variant));
            if let Some(q) = current.and_then(|t| t.quality.as_deref()) {
                ui.add_space(8.0);
                ui.label(TypeRole::LabelMedium.style().rich(q, s.primary));
            }
        });
    });
    y += 64.0 + PROGRESS_GAP;

    // Controls: standard button group, hero play in the middle.
    let group_w = 4.0 * 56.0 + 96.0 + 4.0 * 8.0 + 2.0 * 16.0;
    let controls = snap(
        ui,
        Rect::from_min_size(
            pos2(col.left() + ((col.width() - group_w) * 0.5).max(0.0), y),
            vec2(group_w.min(col.width()), CONTROLS_H),
        ),
    );
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(controls)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("np-controls"),
    );
    let tonal = |icon: Icon| IconButton::new(icon).size(Size::M).style(Style::Tonal);
    let repeat_icon = if p.repeat == Repeat::One { Icon::RepeatOne } else { Icon::Repeat };
    let hero = IconButton::new(Icon::PlayArrow)
        .selected_icon(Icon::Pause)
        .size(Size::L)
        .style(Style::Filled)
        .shape(BtnShape::Round)
        .selected(playing)
        .filled_icon(true)
        .colors(Colors {
            container: s.primary,
            content: s.on_primary,
            border: Color32::TRANSPARENT,
        })
        .tooltip("Играть / пауза (Space)");
    let r = ButtonGroup::new("np-group", Size::M).show(
        &mut cui,
        vec![
            Item::from(tonal(Icon::Shuffle).selected(p.shuffle).tooltip("Перемешать")),
            Item::from(tonal(Icon::SkipPrevious).tooltip("Назад (Ctrl+←)")),
            Item::from(hero),
            Item::from(tonal(Icon::SkipNext).tooltip("Далее (Ctrl+→)")),
            Item::from(
                tonal(repeat_icon)
                    .selected(p.repeat != Repeat::Off)
                    .tooltip("Повтор: выкл → всё → один"),
            ),
        ],
    );
    if r[0].clicked() {
        out.push(Command::ToggleShuffle);
    }
    if r[1].clicked() {
        chrome::press_previous(&ctx, state, out);
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
    y += CONTROLS_H + 16.0;

    // Actions: like and add on the left, volume on the right.
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
    }
    aui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let volume = if p.muted { 0.0 } else { chrome.volume_now(&ctx, state) };
        // The slider is optional; the wheel sets the volume either way.
        if state.prefs.np_volume {
            let w = (ui.available_width() - 48.0).clamp(96.0, 220.0);
            let vo = Slider::new("np-vol", volume)
                .track(8.0)
                .width(w)
                .value_label(|v| format!("{:.0}%", v * 100.0))
                .show(ui);
            if let Some(v) = vo.dragging.or(vo.committed) {
                out.push(Command::SetVolume(v));
            }
        }
        if ui
            .add(
                IconButton::new(if p.muted || volume == 0.0 { Icon::VolumeOff } else { Icon::VolumeUp })
                    .tooltip("Звук (колесо мыши — громкость)")
                    .id_salt("np-mute"),
            )
            .clicked()
        {
            out.push(Command::ToggleMute);
        }
    });
}

/// Background: soft colour clouds drifting over the window, breathing with
/// the bass in reactive mode (effects spring, no overshoot).
fn ambient(ui: &Ui, rect: Rect, state: &ViewState, pages: &PageState) {
    let theme = Theme::get(ui.ctx());
    let mode = state.prefs.ambient;
    if mode == nikgob_view::Ambient::Off {
        return;
    }
    let playing = state.player.status == Status::Playing;
    let bass = if mode == nikgob_view::Ambient::Reactive && playing {
        pages.levels_bass
    } else {
        0.0
    };
    let level = anim::fraction(ui.ctx(), egui::Id::new("np-glow"), bass, theme.motion.fast_effects);
    super::ambient::paint(ui.ctx(), ui.painter(), rect, level.clamp(0.0, 1.0), playing);
}
