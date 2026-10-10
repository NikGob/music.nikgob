//! egui rendering helpers for [`Cubic`] outlines.
//!
//! Outlines are expected in the unit square (as produced by [`crate::MaterialShape::polygon`]
//! and [`crate::Morph::as_cubics`] of normalized polygons) and are scaled into a target
//! [`Rect`], optionally rotated around the rect center.
//!
//! egui's built-in filled paths only support convex polygons, so filling here is done with
//! an ear-clipped [`Mesh`] plus a one-pixel feathered edge for anti-aliasing.

use egui::epaint::{Mesh, PathShape, Vertex, WHITE_UV};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, TextureId, Vec2, pos2, vec2};

use crate::cubic::Cubic;
use crate::point::Point;
use crate::triangulate::triangulate;

/// Flattening tolerance used by the `fill_*` helpers, in physical pixels.
pub const DEFAULT_TOLERANCE_PX: f32 = 0.2;

/// Tessellation settings, in egui points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillOptions {
    /// Maximum distance between the flattened outline and the true curve.
    pub tolerance: f32,
    /// Width of the anti-aliasing fringe (0 disables it). One physical pixel is ideal.
    pub feather: f32,
}

impl FillOptions {
    /// Options tuned for `pixels_per_point`: sub-pixel tolerance and a one-pixel feather.
    pub fn for_ppp(pixels_per_point: f32) -> Self {
        let ppp = pixels_per_point.max(0.01);
        Self {
            tolerance: DEFAULT_TOLERANCE_PX / ppp,
            feather: 1.0 / ppp,
        }
    }
}

impl Default for FillOptions {
    fn default() -> Self {
        Self::for_ppp(1.0)
    }
}

/// Maps a unit-square point into `rect`, then rotates around the rect center.
#[derive(Clone, Copy)]
struct Placement {
    min: Vec2,
    size: Vec2,
    center: Vec2,
    sin: f32,
    cos: f32,
}

impl Placement {
    fn new(rect: Rect, rotation: f32) -> Self {
        let (sin, cos) = rotation.sin_cos();
        Self {
            min: rect.min.to_vec2(),
            size: rect.size(),
            center: rect.center().to_vec2(),
            sin,
            cos,
        }
    }

    #[inline]
    fn apply(&self, p: Point) -> Pos2 {
        let x = self.min.x + p.x * self.size.x - self.center.x;
        let y = self.min.y + p.y * self.size.y - self.center.y;
        pos2(
            self.center.x + x * self.cos - y * self.sin,
            self.center.y + x * self.sin + y * self.cos,
        )
    }
}

/// Flattens a cubic Bezier (screen space) by adaptive subdivision, appending every point
/// after the start point to `out`.
fn flatten(p: [Pos2; 4], tolerance: f32, depth: u32, out: &mut Vec<Pos2>) {
    let chord = p[3] - p[0];
    let len = chord.length();
    let dist = |q: Pos2| {
        if len > 1e-9 {
            ((q - p[0]).x * chord.y - (q - p[0]).y * chord.x).abs() / len
        } else {
            (q - p[0]).length()
        }
    };
    // The curve deviates from its chord by at most 3/4 of the control points' distance.
    let flat = 0.75 * dist(p[1]).max(dist(p[2])) <= tolerance;
    if flat || depth >= 12 {
        out.push(p[3]);
        return;
    }
    let mid = |a: Pos2, b: Pos2| pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5);
    let (p01, p12, p23) = (mid(p[0], p[1]), mid(p[1], p[2]), mid(p[2], p[3]));
    let (a, b) = (mid(p01, p12), mid(p12, p23));
    let m = mid(a, b);
    flatten([p[0], p01, a, m], tolerance, depth + 1, out);
    flatten([m, b, p23, p[3]], tolerance, depth + 1, out);
}

/// Flattens `cubics` into a closed polygon in screen space.
///
/// `rect` receives the unit square; `rotation` (radians, clockwise on screen) is applied
/// around the rect center; `tolerance` is the maximum deviation in points. The closing point
/// is not repeated.
pub fn shape_path(cubics: &[Cubic], rect: Rect, rotation: f32, tolerance: f32) -> Vec<Pos2> {
    let place = Placement::new(rect, rotation);
    let tolerance = tolerance.max(1e-3);
    let mut out = Vec::with_capacity(cubics.len() * 4);
    for (i, c) in cubics.iter().enumerate() {
        let p = [
            place.apply(c.anchor0()),
            place.apply(c.control0()),
            place.apply(c.control1()),
            place.apply(c.anchor1()),
        ];
        if i == 0 {
            out.push(p[0]);
        }
        flatten(p, tolerance, 0, &mut out);
    }
    if out.len() > 1 && out[0].distance(out[out.len() - 1]) < 1e-4 {
        out.pop();
    }
    out
}

/// A closed stroked outline as an egui [`Shape`].
pub fn outline_shape(
    cubics: &[Cubic],
    rect: Rect,
    rotation: f32,
    tolerance: f32,
    stroke: Stroke,
) -> Shape {
    Shape::Path(PathShape::closed_line(
        shape_path(cubics, rect, rotation, tolerance),
        stroke,
    ))
}

/// Strokes the outline.
pub fn stroke_shape(
    painter: &Painter,
    cubics: &[Cubic],
    rect: Rect,
    rotation: f32,
    stroke: Stroke,
) {
    let tol = FillOptions::for_ppp(painter.ctx().pixels_per_point()).tolerance;
    painter.add(outline_shape(cubics, rect, rotation, tol, stroke));
}

/// Where to sample a texture from.
#[derive(Clone, Copy)]
struct TexMap {
    rect: Rect,
    uv: Rect,
}

impl TexMap {
    fn uv_at(&self, p: Pos2) -> Pos2 {
        let size = self.rect.size();
        let fx = if size.x > 0.0 {
            (p.x - self.rect.min.x) / size.x
        } else {
            0.0
        };
        let fy = if size.y > 0.0 {
            (p.y - self.rect.min.y) / size.y
        } else {
            0.0
        };
        pos2(
            self.uv.min.x + fx * self.uv.width(),
            self.uv.min.y + fy * self.uv.height(),
        )
    }
}

/// Builds a (feathered) triangle mesh filling the closed polygon `path`.
fn build_mesh(
    path: &[Pos2],
    color: Color32,
    texture: Option<(TextureId, TexMap)>,
    feather: f32,
) -> Mesh {
    let mut mesh = match texture {
        Some((id, _)) => Mesh::with_texture(id),
        None => Mesh::default(),
    };

    // Dedupe consecutive points so edge normals are well defined.
    let mut pts: Vec<Pos2> = Vec::with_capacity(path.len());
    for &p in path {
        if pts.last().is_none_or(|l| l.distance_sq(p) > 1e-10) {
            pts.push(p);
        }
    }
    while pts.len() > 1 && pts[0].distance_sq(pts[pts.len() - 1]) <= 1e-10 {
        pts.pop();
    }
    let n = pts.len();
    if n < 3 {
        return mesh;
    }

    let raw: Vec<[f32; 2]> = pts.iter().map(|p| [p.x, p.y]).collect();
    let tris = triangulate(&raw);

    let uv_of = |p: Pos2| texture.map_or(WHITE_UV, |(_, t)| t.uv_at(p));
    let push = |mesh: &mut Mesh, pos: Pos2, color: Color32| {
        mesh.vertices.push(Vertex {
            pos,
            uv: uv_of(pos),
            color,
        });
    };

    if feather <= 0.0 {
        mesh.reserve_vertices(n);
        for &p in &pts {
            push(&mut mesh, p, color);
        }
    } else {
        // Orientation decides which side the outward normal is on.
        let area: f32 = (0..n)
            .map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                a.x * b.y - b.x * a.y
            })
            .sum();
        let outward = if area >= 0.0 { 1.0 } else { -1.0 };
        let edge_normal = |a: Pos2, b: Pos2| {
            let d = b - a;
            let len = d.length();
            if len > 0.0 {
                vec2(d.y, -d.x) / len * outward
            } else {
                Vec2::ZERO
            }
        };
        let half = feather * 0.5;
        mesh.reserve_vertices(n * 2);
        let mut outer = Vec::with_capacity(n);
        for i in 0..n {
            let n0 = edge_normal(pts[(i + n - 1) % n], pts[i]);
            let n1 = edge_normal(pts[i], pts[(i + 1) % n]);
            let m = n0 + n1;
            // Miter offset: moves both adjacent edges by 1 unit (length clamped for spikes).
            let offset = m * (2.0 / m.length_sq().max(0.25));
            push(&mut mesh, pts[i] - offset * half, color);
            outer.push(pts[i] + offset * half);
        }
        for p in outer {
            push(&mut mesh, p, Color32::TRANSPARENT);
        }
    }

    mesh.reserve_triangles(tris.len() + if feather > 0.0 { n * 2 } else { 0 });
    // Winding is irrelevant to egui's tessellator (no culling).
    for [a, b, c] in tris {
        mesh.add_triangle(a, b, c);
    }
    if feather > 0.0 {
        let n = n as u32;
        for i in 0..n {
            let j = (i + 1) % n;
            mesh.add_triangle(i, j, n + j);
            mesh.add_triangle(i, n + j, n + i);
        }
    }
    mesh
}

/// Mesh filling the outline with a flat `color`.
pub fn fill_mesh(
    cubics: &[Cubic],
    rect: Rect,
    rotation: f32,
    color: Color32,
    options: &FillOptions,
) -> Mesh {
    let path = shape_path(cubics, rect, rotation, options.tolerance);
    build_mesh(&path, color, None, options.feather)
}

/// Mesh filling the outline with a texture. The texture is mapped onto `rect` axis-aligned
/// (it does not rotate with `rotation`): `uv` is the texture region shown across `rect`.
pub fn texture_mesh(
    cubics: &[Cubic],
    rect: Rect,
    rotation: f32,
    texture_id: TextureId,
    uv: Rect,
    tint: Color32,
    options: &FillOptions,
) -> Mesh {
    let path = shape_path(cubics, rect, rotation, options.tolerance);
    build_mesh(
        &path,
        tint,
        Some((texture_id, TexMap { rect, uv })),
        options.feather,
    )
}

/// Fills the (possibly concave) outline with `color`, anti-aliased.
pub fn fill_shape(painter: &Painter, cubics: &[Cubic], rect: Rect, rotation: f32, color: Color32) {
    fill_shape_with(
        painter,
        cubics,
        rect,
        rotation,
        color,
        &FillOptions::for_ppp(painter.ctx().pixels_per_point()),
    );
}

/// [`fill_shape`] with explicit tessellation options.
pub fn fill_shape_with(
    painter: &Painter,
    cubics: &[Cubic],
    rect: Rect,
    rotation: f32,
    color: Color32,
    options: &FillOptions,
) {
    let mesh = fill_mesh(cubics, rect, rotation, color, options);
    if !mesh.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
}

/// Draws `texture_id` (region `uv`, usually `[0,0]..[1,1]`) cropped to the outline, e.g. to
/// shape an album cover. The image stays axis-aligned and stretched over `rect`; `tint`
/// multiplies the texture colors (use [`Color32::WHITE`] for none).
pub fn clip_texture(
    painter: &Painter,
    cubics: &[Cubic],
    rect: Rect,
    rotation: f32,
    texture_id: TextureId,
    uv: Rect,
    tint: Color32,
) {
    let mesh = texture_mesh(
        cubics,
        rect,
        rotation,
        texture_id,
        uv,
        tint,
        &FillOptions::for_ppp(painter.ctx().pixels_per_point()),
    );
    if !mesh.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
}
