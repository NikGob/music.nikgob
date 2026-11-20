use super::*;

/// The soft edges' settings: how deep each band reaches (a share of the
/// view's height, «Глубина»), how early a line fades in it («Затухание»)
/// and how much it blurs right at the edge («Размытие у края»). With the
/// text it is for ([`Soft::text`]) the band never takes less than about a
/// line and the blur goes with the size of the letters, so a smaller copy
/// of the view (the settings' scene) looks the same.
#[derive(Clone, Copy, Debug)]
pub(super) struct Soft {
    pub(super) depth: f32,
    pub(super) fade: f32,
    /// «Размытие у края», 0..1.5.
    pub(super) strength: f32,
    /// From one line to the next, px: the least band.
    pub(super) pitch: f32,
    /// The text's size, px: what the blur is measured by.
    pub(super) size: f32,
}

/// A band takes at least this many lines, so one always rolls through it.
pub(super) const BAND_LINES: f32 = 1.25;
/// The blur's reach right at the edge at full strength, in text sizes.
pub(super) const EDGE_BLUR: f32 = 0.16;
/// Below this reach (px) a line is not blurred at all.
pub(super) const BLUR_MIN: f32 = 0.35;

impl Soft {
    pub(super) fn of(p: &Prefs) -> Self {
        Self {
            depth: p.lyrics_soft_depth.clamp(0.08, 0.4),
            fade: p.lyrics_soft_fade.clamp(0.0, 1.0),
            strength: p.lyrics_soft_strength.clamp(0.0, 1.5),
            pitch: 0.0,
            size: 0.0,
        }
    }

    /// For text `size` px big, its lines `pitch` px apart.
    pub(super) fn text(self, pitch: f32, size: f32) -> Self {
        Self { pitch: pitch.max(0.0), size: size.max(0.0), ..self }
    }

    /// How deep the band at each edge of `rect` reaches: its share of the
    /// height, at least about a line, never more than 0.42 of it.
    pub(super) fn band(self, rect: Rect) -> f32 {
        let h = rect.height().max(0.0);
        (h * self.depth).max(self.pitch * BAND_LINES).max(12.0).min(h * 0.42)
    }
}

/// Where the current line's middle rests, down from `rect`'s top: `share`
/// of its height; with the soft edges (`soft`) never so high that the line
/// (`half` its height, its ink `ink` beyond it) reaches into the top band,
/// though never below the middle.
pub(super) fn rest_at(rect: Rect, share: f32, soft: Option<Soft>, half: f32, ink: f32) -> f32 {
    let at = rect.height() * share;
    match soft {
        Some(soft) => at.max((soft.band(rect) + ink + half).min(rect.height() * 0.5)),
        None => at,
    }
}

/// The soft edges of a view from `top` to `bottom` (on screen, where it is
/// seen): the text is whole and sharp in the middle and fades over a
/// `band` at either edge, to nothing at the edge itself, so lines come in
/// and go out through it with no line where they are cut. A mask over the
/// view by height, on each glyph's own vertices: no picture of the text,
/// nothing drawn into a texture. Close to the edge the letters also blur a
/// little ([`Fade::blur_at`]); the middle never does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Fade {
    pub(super) top: f32,
    pub(super) bottom: f32,
    pub(super) band: f32,
    /// How the opacity rises over the band ([`Fade::at`]): less keeps the
    /// text brighter until nearer the edge, more fades it from further in.
    pub(super) gamma: f32,
    /// How far (px) the letters spread right at the edge.
    pub(super) blur: f32,
}

impl Fade {
    pub(super) fn of(rect: Rect, soft: Soft) -> Self {
        Fade {
            top: rect.top(),
            bottom: rect.bottom(),
            band: soft.band(rect),
            gamma: 0.6 + 1.4 * soft.fade,
            blur: EDGE_BLUR * soft.size * soft.strength,
        }
    }

    /// How far into the band (0 at the edge .. 1 at its inner end and
    /// beyond) height `y` is.
    pub(super) fn depth_at(self, y: f32) -> f32 {
        let room = (y - self.top).min(self.bottom - y);
        (room / self.band.max(1.0)).clamp(0.0, 1.0)
    }

    /// How opaque (0..1) the text is at height `y` on screen: an S-curve
    /// over the band (no step at either end of it), shaped by «Затухание».
    pub(super) fn at(self, y: f32) -> f32 {
        let f = self.depth_at(y);
        if f >= 1.0 {
            return 1.0;
        }
        smoothstep(f).powf(self.gamma)
    }

    /// How far (px) the letters spread at height `y`: nothing in the inner
    /// half of the band and in the middle, growing towards the edge.
    pub(super) fn blur_at(self, y: f32) -> f32 {
        let near = (1.0 - self.depth_at(y) * 2.0).max(0.0);
        self.blur * near * near
    }

    /// Whether everything from `y0` down to `y1` is clear of both bands.
    pub(super) fn clear(self, y0: f32, y1: f32) -> bool {
        y0 - self.top >= self.band && self.bottom - y1 >= self.band
    }
}

/// The directions of a blurred glyph's copies around it.
pub(super) const BLUR_TAPS: [egui::Vec2; 8] = [
    vec2(1.0, 0.0),
    vec2(0.7071, 0.7071),
    vec2(0.0, 1.0),
    vec2(-0.7071, 0.7071),
    vec2(-1.0, 0.0),
    vec2(-0.7071, -0.7071),
    vec2(0.0, -1.0),
    vec2(0.7071, -0.7071),
];
/// How much of a glyph's opacity goes to its copies where the blur is
/// full (at the very edge); the rest stays on the glyph.
pub(super) const BLUR_SPREAD: f32 = 0.75;

/// `g` at `pos` with each glyph vertex in `color` (its own colour when
/// `None`), thinned by `fade` at its height on screen. The glyphs are the
/// galley's own, so the text is as sharp as ever; a row near the edge also
/// gets copies of its glyphs spread around them by [`Fade::blur_at`] (on
/// the same mesh, the opacity shared between the glyph and its copies, so
/// the row is about as opaque as it would be sharp). A row that crosses an
/// edge of the view has its triangles cut there ([`cut_at_edges`]): the
/// opacity is set at each vertex where it finally lies, and the GPU blends
/// it in a straight line between vertices, so without a vertex on the edge
/// a glyph reaching past it would be cut by the clip at what its inside
/// vertex blends to there, not at nothing.
pub(super) fn faded(g: &Galley, pos: egui::Pos2, color: Option<Color32>, fade: Fade) -> Arc<Galley> {
    let mut out = g.clone();
    let mut reach_all = 0.0_f32;
    for placed in &mut out.rows {
        let top = pos.y + placed.pos.y;
        let mut row = (*placed.row).clone();
        let glyphs = row.visuals.glyph_vertex_range.clone();
        // The mesh is glyphs only (no background or line under the text):
        // all of it may be cut and renumbered.
        let whole = glyphs.start == 0 && glyphs.end == row.visuals.mesh.vertices.len();
        let mesh = &mut row.visuals.mesh;
        let Some(vertices) = mesh.vertices.get(glyphs.clone()) else {
            placed.row = Arc::new(row);
            continue;
        };
        let reach = vertices.iter().map(|v| fade.blur_at(top + v.pos.y)).fold(0.0, f32::max);
        // Each glyph vertex in its colour, the copies of a blurred row
        // after them in theirs; the opacity of the fade comes last, where
        // each vertex finally lies.
        for v in &mut mesh.vertices[glyphs.clone()] {
            v.color = color.unwrap_or(v.color);
        }
        let blurred = reach >= BLUR_MIN;
        let before = mesh.vertices.len();
        if blurred {
            reach_all = reach_all.max(reach);
            blur_copies(mesh, glyphs.clone(), top, fade);
        }
        let (mut thin, mut copies) = (glyphs.clone(), before..mesh.vertices.len());
        let (lo, hi) = (fade.top - top, fade.bottom - top);
        if whole && mesh.vertices.iter().any(|v| v.pos.y < lo || v.pos.y > hi) {
            cut_at_edges(mesh, lo, hi);
            (thin, copies) = (0..mesh.vertices.len(), 0..0);
        }
        for range in [thin, copies] {
            for v in &mut mesh.vertices[range] {
                v.color = v.color.gamma_multiply(fade.at(top + v.pos.y));
            }
        }
        if whole {
            row.visuals.glyph_vertex_range = 0..row.visuals.mesh.vertices.len();
        }
        if blurred {
            row.visuals.mesh_bounds = row.visuals.mesh_bounds.expand(reach);
        }
        placed.row = Arc::new(row);
    }
    if reach_all > 0.0 {
        out.mesh_bounds = out.mesh_bounds.expand(reach_all);
    }
    Arc::new(out)
}

/// Copies of the glyph quads of `mesh` (its vertices `glyphs`, a row at
/// `top` on screen) spread around them by [`Fade::blur_at`], appended to
/// it; the glyphs' opacity is shared between each and its copies.
pub(super) fn blur_copies(mesh: &mut egui::epaint::Mesh, glyphs: std::ops::Range<usize>, top: f32, fade: Fade) {
    let tris: Vec<u32> = mesh
        .indices
        .chunks_exact(3)
        .filter(|t| t.iter().all(|&i| glyphs.contains(&(i as usize))))
        .flatten()
        .copied()
        .collect();
    let base: Vec<egui::epaint::Vertex> = mesh.vertices[glyphs.clone()].to_vec();
    // The glyph's own share of its opacity and each copy's.
    let share = |y: f32| {
        let spread = BLUR_SPREAD * (fade.blur_at(y) / fade.blur.max(1e-3)).clamp(0.0, 1.0);
        (1.0 - spread, spread / BLUR_TAPS.len() as f32)
    };
    for v in &mut mesh.vertices[glyphs.clone()] {
        v.color = v.color.gamma_multiply(share(top + v.pos.y).0);
    }
    mesh.vertices.reserve(base.len() * BLUR_TAPS.len());
    mesh.indices.reserve(tris.len() * BLUR_TAPS.len());
    for dir in BLUR_TAPS {
        let first = mesh.vertices.len() as u32;
        for v in &base {
            let y = top + v.pos.y;
            let mut c = *v;
            c.pos += dir * fade.blur_at(y);
            c.color = v.color.gamma_multiply(share(y).1);
            mesh.vertices.push(c);
        }
        mesh.indices.extend(tris.iter().map(|&i| first + (i - glyphs.start as u32)));
    }
}

/// `a` to `b` by `t`, every part of the vertex (place, texture, colour).
pub(super) fn vertex_between(a: &egui::epaint::Vertex, b: &egui::epaint::Vertex, t: f32) -> egui::epaint::Vertex {
    let c = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round().clamp(0.0, 255.0) as u8;
    let (ca, cb) = (a.color, b.color);
    egui::epaint::Vertex {
        pos: a.pos + (b.pos - a.pos) * t,
        uv: a.uv + (b.uv - a.uv) * t,
        color: Color32::from_rgba_premultiplied(c(ca.r(), cb.r()), c(ca.g(), cb.g()), c(ca.b(), cb.b()), c(ca.a(), cb.a())),
    }
}

/// `mesh` (its own coordinates) cut to the heights `lo..=hi`: every
/// triangle clipped against both lines, with new vertices where it crosses
/// one, so the edge of the view has a vertex wherever a glyph crosses it;
/// what lies wholly outside is dropped.
pub(super) fn cut_at_edges(mesh: &mut egui::epaint::Mesh, lo: f32, hi: f32) {
    let old = std::mem::take(&mut mesh.vertices);
    let indices = std::mem::take(&mut mesh.indices);
    // One side of a line at a time: keeps the part of `poly` with
    // `inside(y)`, `at` the line's height.
    fn clip(poly: &[egui::epaint::Vertex], at: f32, keep_below: bool) -> Vec<egui::epaint::Vertex> {
        let inside = |y: f32| if keep_below { y <= at } else { y >= at };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for (k, a) in poly.iter().enumerate() {
            let b = &poly[(k + 1) % poly.len()];
            let (ia, ib) = (inside(a.pos.y), inside(b.pos.y));
            if ia {
                out.push(*a);
            }
            if ia != ib {
                let t = (at - a.pos.y) / (b.pos.y - a.pos.y);
                let mut v = vertex_between(a, b, t);
                v.pos.y = at;
                out.push(v);
            }
        }
        out
    }
    mesh.vertices.reserve(old.len());
    mesh.indices.reserve(indices.len());
    for t in indices.chunks_exact(3) {
        let tri = [old[t[0] as usize], old[t[1] as usize], old[t[2] as usize]];
        let ys = tri.map(|v| v.pos.y);
        if ys.iter().all(|&y| y < lo) || ys.iter().all(|&y| y > hi) {
            continue;
        }
        if ys.iter().all(|&y| (lo..=hi).contains(&y)) {
            let first = mesh.vertices.len() as u32;
            mesh.vertices.extend(tri);
            mesh.indices.extend([first, first + 1, first + 2]);
            continue;
        }
        let poly = clip(&clip(&tri, lo, false), hi, true);
        if poly.len() < 3 {
            continue;
        }
        let first = mesh.vertices.len() as u32;
        mesh.vertices.extend(poly.iter().copied());
        for k in 1..poly.len() as u32 - 1 {
            mesh.indices.extend([first, first + k, first + k + 1]);
        }
    }
}

/// [`faded`] when `g` at `pos` reaches into one of `fade`'s bands; `None`
/// when it lies clear of them (or there is no fade) and is drawn as it is.
pub(super) fn through(g: &Galley, pos: egui::Pos2, color: Option<Color32>, fade: Option<Fade>) -> Option<Arc<Galley>> {
    let ink = g.mesh_bounds.translate(pos.to_vec2());
    fade.filter(|f| !f.clear(ink.top(), ink.bottom())).map(|f| faded(g, pos, color, f))
}

/// `g` at `pos` in `color`, thinned towards the view's edges by `fade`.
pub(super) fn fade_galley(painter: &egui::Painter, pos: egui::Pos2, g: &Arc<Galley>, color: Color32, fade: Option<Fade>) {
    if color.a() == 0 {
        return;
    }
    let (pos, g) = afloat(painter, pos, g);
    match through(&g, pos, Some(color), fade) {
        Some(g) => {
            painter.galley(pos, g, color);
        }
        None => {
            painter.galley_with_override_text_color(pos, g, color);
        }
    }
}

/// `g` to be drawn at the height of `pos` exactly, between whole pixels
/// too: the renderer puts a text's origin on whole pixels, so a line that
/// moves steps from one to the next (seen as a shake while it slows to its
/// place, and as lines trembling against each other when each moves on its
/// own). Returns the origin on whole pixels and the galley with its rows
/// moved by the rest; what rests on whole pixels is drawn as it is, as
/// sharp as ever. Across, the origin is left to the renderer.
pub(super) fn afloat(painter: &egui::Painter, pos: egui::Pos2, g: &Arc<Galley>) -> (egui::Pos2, Arc<Galley>) {
    let ppp = painter.pixels_per_point();
    let whole = pos2(pos.x, (pos.y * ppp).round() / ppp);
    let rest = pos.y - whole.y;
    if !rest.is_finite() || (rest * ppp).abs() < 0.02 {
        return (whole, g.clone());
    }
    let mut out = (**g).clone();
    for placed in &mut out.rows {
        placed.pos.y += rest;
    }
    out.mesh_bounds = out.mesh_bounds.translate(vec2(0.0, rest));
    (whole, Arc::new(out))
}
