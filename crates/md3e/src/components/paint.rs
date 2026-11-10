//! Container painting with fractional corner radii, state layers and focus
//! rings. egui's `CornerRadius` is whole points; springs morph radii in
//! fractions, so containers are drawn as convex paths instead.

use std::f32::consts::FRAC_PI_2;

use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, TextureId, Vec2, epaint::{PathShape, Vertex}, pos2, vec2};

use crate::tokens::{Elevation, focus};

/// Per-corner radii: north-west, north-east, south-east, south-west.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii(pub [f32; 4]);

impl Radii {
    pub const fn all(r: f32) -> Self {
        Self([r; 4])
    }

    /// Leading (left) corners `l`, trailing (right) corners `r`.
    pub const fn sides(l: f32, r: f32) -> Self {
        Self([l, r, r, l])
    }

    pub const fn top(r: f32) -> Self {
        Self([r, r, 0.0, 0.0])
    }

    /// Each radius clamped to half the shorter side.
    pub fn clamped(self, rect: Rect) -> Self {
        let max = rect.width().min(rect.height()) * 0.5;
        Self(self.0.map(|r| r.clamp(0.0, max)))
    }

    pub fn shrink(self, by: f32) -> Self {
        Self(self.0.map(|r| (r - by).max(0.0)))
    }

    pub fn expand(self, by: f32) -> Self {
        Self(self.0.map(|r| r + by))
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        let mut o = [0.0; 4];
        for (i, v) in o.iter_mut().enumerate() {
            *v = self.0[i] + (other.0[i] - self.0[i]) * t;
        }
        Self(o)
    }

    fn whole_points(self) -> egui::CornerRadius {
        let c = |r: f32| r.round().clamp(0.0, 255.0) as u8;
        egui::CornerRadius {
            nw: c(self.0[0]),
            ne: c(self.0[1]),
            se: c(self.0[2]),
            sw: c(self.0[3]),
        }
    }
}

impl From<f32> for Radii {
    fn from(r: f32) -> Self {
        Self::all(r)
    }
}

/// The outline of a rounded rectangle, clockwise from the top-left arc.
pub fn rounded_rect_points(rect: Rect, radii: Radii) -> Vec<Pos2> {
    let r = radii.clamped(rect).0;
    let centers = [
        (pos2(rect.left() + r[0], rect.top() + r[0]), r[0], 2.0 * FRAC_PI_2),
        (pos2(rect.right() - r[1], rect.top() + r[1]), r[1], 3.0 * FRAC_PI_2),
        (pos2(rect.right() - r[2], rect.bottom() - r[2]), r[2], 0.0),
        (pos2(rect.left() + r[3], rect.bottom() - r[3]), r[3], FRAC_PI_2),
    ];
    let mut pts = Vec::with_capacity(64);
    for (c, radius, start) in centers {
        if radius < 0.25 {
            pts.push(c);
            continue;
        }
        // About one segment per 2 px of arc, at least 3.
        let n = ((radius * FRAC_PI_2) / 2.0).ceil().clamp(3.0, 24.0) as usize;
        for i in 0..=n {
            let a = start + FRAC_PI_2 * i as f32 / n as f32;
            pts.push(c + radius * egui::vec2(a.cos(), a.sin()));
        }
    }
    // On a pill the arcs meet in one point; egui's tessellator turns
    // duplicate vertices into broken normals, so drop them.
    pts.dedup_by(|a, b| a.distance_sq(*b) < 1e-4);
    if pts.len() > 1 && pts[0].distance_sq(pts[pts.len() - 1]) < 1e-4 {
        pts.pop();
    }
    pts
}

/// A texture over the convex outline `points` (as [`rounded_rect_points`]
/// gives them): `uv` is the part of the texture that `rect` shows. An edge
/// `soft` says yes to fades over `feather` (half inside, half outside, as
/// the fringe of [`container`]), so a picture cut to a container's own
/// outline ends exactly where the container does; an edge it says no to is
/// hard (the seam between two pictures standing side by side).
pub fn textured_polygon(points: &[Pos2], rect: Rect, uv: Rect, texture: TextureId, feather: f32, soft: &dyn Fn(Pos2, Pos2) -> bool) -> Mesh {
    let mut mesh = Mesh::with_texture(texture);
    let n = points.len();
    if n < 3 {
        return mesh;
    }
    let uv_at = |p: Pos2| {
        let fx = ((p.x - rect.min.x) / rect.width().max(1e-6)).clamp(0.0, 1.0);
        let fy = ((p.y - rect.min.y) / rect.height().max(1e-6)).clamp(0.0, 1.0);
        pos2(uv.min.x + fx * uv.width(), uv.min.y + fy * uv.height())
    };
    // Which side is out.
    let area: f32 = (0..n).map(|i| points[i].x * points[(i + 1) % n].y - points[(i + 1) % n].x * points[i].y).sum();
    let out = if area >= 0.0 { 1.0 } else { -1.0 };
    let normal = |a: Pos2, b: Pos2| -> Vec2 {
        let d = b - a;
        let len = d.length();
        if len <= 0.0 || feather <= 0.0 || !soft(a, b) { Vec2::ZERO } else { vec2(d.y, -d.x) / len * out }
    };
    let half = feather.max(0.0) * 0.5;
    let mut outer: Vec<Pos2> = Vec::with_capacity(n);
    for i in 0..n {
        let (n0, n1) = (normal(points[(i + n - 1) % n], points[i]), normal(points[i], points[(i + 1) % n]));
        // Both edges soft: the miter that moves each by one unit; one: its
        // own normal, along the hard edge.
        let m = n0 + n1;
        let offset = if n0 == Vec2::ZERO || n1 == Vec2::ZERO { m } else { m * (2.0 / m.length_sq().max(0.25)) };
        let inner = points[i] - offset * half;
        mesh.vertices.push(Vertex { pos: inner, uv: uv_at(inner), color: Color32::WHITE });
        outer.push(points[i] + offset * half);
    }
    for p in outer {
        mesh.vertices.push(Vertex { pos: p, uv: uv_at(p), color: Color32::TRANSPARENT });
    }
    let n = n as u32;
    for i in 1..n - 1 {
        mesh.add_triangle(0, i, i + 1);
    }
    if half > 0.0 {
        for i in 0..n {
            let j = (i + 1) % n;
            mesh.add_triangle(i, j, n + j);
            mesh.add_triangle(i, n + j, n + i);
        }
    }
    mesh
}

/// A rounded container, optionally outlined.
pub fn container(painter: &Painter, rect: Rect, radii: Radii, fill: Color32, border: Stroke) {
    if fill.a() == 0 && border.is_empty() {
        return;
    }
    painter.add(Shape::Path(PathShape::convex_polygon(
        rounded_rect_points(rect, radii),
        fill,
        border,
    )));
}

/// The level's shadow under a container of this shape.
pub fn shadow(painter: &Painter, rect: Rect, radii: Radii, elevation: Elevation) {
    if elevation == Elevation::Level0 {
        return;
    }
    painter.add(
        elevation
            .shadow()
            .as_shape(rect, radii.clamped(rect).whole_points()),
    );
}

/// The state layer: the content colour over the container at `opacity`.
pub fn state_layer(painter: &Painter, rect: Rect, radii: Radii, content: Color32, opacity: f32) {
    if opacity <= 0.001 {
        return;
    }
    container(
        painter,
        rect,
        radii,
        content.gamma_multiply(opacity),
        Stroke::NONE,
    );
}

/// The 3 dp focus ring, 2 dp outside the container, in `color` (secondary).
pub fn focus_ring(painter: &Painter, rect: Rect, radii: Radii, color: Color32) {
    let offset = focus::RING_OFFSET + focus::RING_WIDTH * 0.5;
    let outer = rect.expand(offset);
    painter.add(Shape::Path(PathShape::closed_line(
        rounded_rect_points(outer, radii.expand(offset)),
        Stroke::new(focus::RING_WIDTH, color),
    )));
}
