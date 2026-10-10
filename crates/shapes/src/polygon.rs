//! Port of `RoundedPolygon.kt`, including the private `RoundedCorner` helper.

use crate::corner_rounding::CornerRounding;
use crate::cubic::Cubic;
use crate::error::{Result, ShapeError};
use crate::feature::{Feature, detect_features};
use crate::point::{
    DISTANCE_EPSILON, PI, Point, convex, direction_vector, interpolate, radial_to_cartesian, square,
};

/// A closed outline made of cubic Beziers, with its corner/edge features.
///
/// Coordinates are in "shape space" (unit scale); use [`RoundedPolygon::normalized`] to
/// fit one into the unit square.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundedPolygon {
    features: Vec<Feature>,
    center: Point,
    cubics: Vec<Cubic>,
}

impl RoundedPolygon {
    /// Builds a regular polygon with `num_vertices` corners on a circle of `radius`
    /// around `center` (first vertex at angle 0).
    pub fn from_num_vertices(
        num_vertices: usize,
        radius: f32,
        center: Point,
        rounding: CornerRounding,
        per_vertex_rounding: Option<&[CornerRounding]>,
    ) -> Result<Self> {
        Self::from_vertices(
            &vertices_from_num_verts(num_vertices, radius, center),
            rounding,
            per_vertex_rounding,
            Some(center),
        )
    }

    /// Builds a polygon from flat `[x0, y0, x1, y1, ...]` vertices. `center` defaults to the
    /// vertex average. `per_vertex_rounding` overrides `rounding` for each vertex.
    pub fn from_vertices(
        vertices: &[f32],
        rounding: CornerRounding,
        per_vertex_rounding: Option<&[CornerRounding]>,
        center: Option<Point>,
    ) -> Result<Self> {
        if vertices.len() < 6 {
            return Err(ShapeError::InvalidArgument(
                "polygons must have at least 3 vertices",
            ));
        }
        if vertices.len() % 2 == 1 {
            return Err(ShapeError::InvalidArgument(
                "the vertices array should have even size",
            ));
        }
        if let Some(pv) = per_vertex_rounding
            && pv.len() * 2 != vertices.len()
        {
            return Err(ShapeError::InvalidArgument(
                "per_vertex_rounding must be None or have one entry per vertex",
            ));
        }
        let n = vertices.len() / 2;
        let vtx = |i: usize| Point::new(vertices[i * 2], vertices[i * 2 + 1]);

        let rounded_corners: Vec<RoundedCorner> = (0..n)
            .map(|i| {
                let r = per_vertex_rounding.map_or(rounding, |pv| pv[i]);
                RoundedCorner::new(vtx((i + n - 1) % n), vtx(i), vtx((i + 1) % n), r)
            })
            .collect();

        // For each side, how much of the (round) cut can actually be used.
        let cut_adjusts: Vec<(f32, f32)> = (0..n)
            .map(|ix| {
                let a = &rounded_corners[ix];
                let b = &rounded_corners[(ix + 1) % n];
                let expected_round_cut = a.expected_round_cut + b.expected_round_cut;
                let expected_cut = a.expected_cut() + b.expected_cut();
                let side = vtx(ix) - vtx((ix + 1) % n);
                let side_size = side.distance();
                if expected_round_cut > side_size {
                    (side_size / expected_round_cut, 0.0)
                } else if expected_cut > side_size {
                    (
                        1.0,
                        (side_size - expected_round_cut) / (expected_cut - expected_round_cut),
                    )
                } else {
                    (1.0, 1.0)
                }
            })
            .collect();

        let corners: Vec<Vec<Cubic>> = (0..n)
            .map(|i| {
                let c = &rounded_corners[i];
                let mut allowed = [0.0f32; 2];
                for (delta, slot) in allowed.iter_mut().enumerate() {
                    let (round_cut_ratio, cut_ratio) = cut_adjusts[(i + n - 1 + delta) % n];
                    *slot = c.expected_round_cut * round_cut_ratio
                        + (c.expected_cut() - c.expected_round_cut) * cut_ratio;
                }
                c.cubics(allowed[0], allowed[1])
            })
            .collect();

        let mut features = Vec::with_capacity(n * 2);
        for i in 0..n {
            let is_convex = convex(vtx((i + n - 1) % n), vtx(i), vtx((i + 1) % n));
            features.push(Feature::Corner {
                cubics: corners[i].clone(),
                convex: is_convex,
            });
            let end = corners[i].last().map(Cubic::anchor1).unwrap_or_default();
            let start = corners[(i + 1) % n]
                .first()
                .map(Cubic::anchor0)
                .unwrap_or_default();
            features.push(Feature::Edge {
                cubics: vec![Cubic::straight_line(end.x, end.y, start.x, start.y)],
            });
        }

        let center = center.unwrap_or_else(|| calculate_center(vertices));
        Self::from_features(features, Some(center))
    }

    /// Builds a polygon from explicit features. `center` defaults to the average of the
    /// features' anchor points. The cubics must be contiguous.
    pub fn from_features(features: Vec<Feature>, center: Option<Point>) -> Result<Self> {
        if features.len() < 2 {
            return Err(ShapeError::InvalidArgument(
                "polygons must have at least 2 features",
            ));
        }
        let center = center.unwrap_or_else(|| {
            let mut verts = Vec::new();
            for f in &features {
                for c in f.cubics() {
                    verts.push(c.points[0]);
                    verts.push(c.points[1]);
                }
            }
            calculate_center(&verts)
        });
        Self::new_checked(features, center)
    }

    /// Builds a polygon from a closed list of cubics, detecting edge/corner features.
    pub fn from_cubics(cubics: &[Cubic]) -> Result<Self> {
        Self::from_features(detect_features(cubics), None)
    }

    fn new_checked(features: Vec<Feature>, center: Point) -> Result<Self> {
        let cubics = build_cubics(&features, center);
        let poly = Self {
            features,
            center,
            cubics,
        };
        let mut prev = poly.cubics[poly.cubics.len() - 1];
        for cubic in &poly.cubics {
            if (cubic.points[0] - prev.points[6]).abs() > DISTANCE_EPSILON
                || (cubic.points[1] - prev.points[7]).abs() > DISTANCE_EPSILON
            {
                return Err(ShapeError::NotContiguous);
            }
            prev = *cubic;
        }
        Ok(poly)
    }

    /// Circle approximated by `num_vertices` (>= 3) fully rounded vertices.
    pub fn circle(num_vertices: usize, radius: f32, center: Point) -> Result<Self> {
        if num_vertices < 3 {
            return Err(ShapeError::InvalidArgument(
                "circle must have at least three vertices",
            ));
        }
        let theta = PI / num_vertices as f32;
        let polygon_radius = radius / theta.cos();
        Self::from_num_vertices(
            num_vertices,
            polygon_radius,
            center,
            CornerRounding::radius(radius),
            None,
        )
    }

    /// Axis-aligned rectangle (vertices go BR, BL, TL, TR) with optional rounding.
    pub fn rectangle(
        width: f32,
        height: f32,
        rounding: CornerRounding,
        per_vertex_rounding: Option<&[CornerRounding]>,
        center: Point,
    ) -> Result<Self> {
        let left = center.x - width / 2.0;
        let top = center.y - height / 2.0;
        let right = center.x + width / 2.0;
        let bottom = center.y + height / 2.0;
        Self::from_vertices(
            &[right, bottom, left, bottom, left, top, right, top],
            rounding,
            per_vertex_rounding,
            Some(center),
        )
    }

    /// Star with `opts.num_vertices_per_radius` outer and as many inner vertices.
    pub fn star(opts: &StarOptions) -> Result<Self> {
        if opts.radius <= 0.0 || opts.inner_radius <= 0.0 {
            return Err(ShapeError::InvalidArgument(
                "star radii must both be greater than 0",
            ));
        }
        if opts.inner_radius >= opts.radius {
            return Err(ShapeError::InvalidArgument(
                "inner_radius must be less than radius",
            ));
        }
        let pv = per_vertex_from_inner(
            opts.num_vertices_per_radius,
            opts.rounding,
            opts.inner_rounding,
            opts.per_vertex_rounding.as_deref(),
        );
        Self::from_vertices(
            &star_vertices(
                opts.num_vertices_per_radius,
                opts.radius,
                opts.inner_radius,
                opts.center,
            ),
            opts.rounding,
            pv.as_deref(),
            Some(opts.center),
        )
    }

    /// Stadium / pill shape of `width` x `height`.
    pub fn pill(width: f32, height: f32, smoothing: f32, center: Point) -> Result<Self> {
        if width <= 0.0 || height <= 0.0 {
            return Err(ShapeError::InvalidArgument(
                "pill shapes must have positive width and height",
            ));
        }
        let (w, h) = (width / 2.0, height / 2.0);
        Self::from_vertices(
            &[
                w + center.x,
                h + center.y,
                -w + center.x,
                h + center.y,
                -w + center.x,
                -h + center.y,
                w + center.x,
                -h + center.y,
            ],
            CornerRounding::new(w.min(h), smoothing),
            None,
            Some(center),
        )
    }

    /// A star-like shape stretched along a pill outline.
    pub fn pill_star(opts: &PillStarOptions) -> Result<Self> {
        if opts.width <= 0.0 || opts.height <= 0.0 {
            return Err(ShapeError::InvalidArgument(
                "pill shapes must have positive width and height",
            ));
        }
        if !(opts.inner_radius_ratio > 0.0 && opts.inner_radius_ratio <= 1.0) {
            return Err(ShapeError::InvalidArgument(
                "inner_radius_ratio must be in (0, 1]",
            ));
        }
        let pv = per_vertex_from_inner(
            opts.num_vertices_per_radius,
            opts.rounding,
            opts.inner_rounding,
            opts.per_vertex_rounding.as_deref(),
        );
        Self::from_vertices(
            &pill_star_vertices(opts),
            opts.rounding,
            pv.as_deref(),
            Some(opts.center),
        )
    }

    /// Features in outline order.
    pub fn features(&self) -> &[Feature] {
        &self.features
    }

    /// Outline cubics, contiguous and closed.
    pub fn cubics(&self) -> &[Cubic] {
        &self.cubics
    }

    pub fn center(&self) -> Point {
        self.center
    }

    /// Applies `f` to every point of the outline (and the center).
    pub fn transformed(&self, f: impl Fn(f32, f32) -> (f32, f32)) -> RoundedPolygon {
        let f: &dyn Fn(f32, f32) -> (f32, f32) = &f;
        let features: Vec<Feature> = self.features.iter().map(|ft| ft.transformed(f)).collect();
        let (cx, cy) = f(self.center.x, self.center.y);
        let center = Point::new(cx, cy);
        // Transforming is pointwise, so contiguity is preserved; skip re-validation.
        let cubics = build_cubics(&features, center);
        RoundedPolygon {
            features,
            center,
            cubics,
        }
    }

    /// Scales and translates so the (approximate) bounds fit centered in the unit square,
    /// preserving aspect ratio.
    pub fn normalized(&self) -> RoundedPolygon {
        let b = self.calculate_bounds(true);
        let width = b[2] - b[0];
        let height = b[3] - b[1];
        let side = width.max(height);
        let offset_x = (side - width) / 2.0 - b[0];
        let offset_y = (side - height) / 2.0 - b[1];
        self.transformed(|x, y| ((x + offset_x) / side, (y + offset_y) / side))
    }

    /// Bounds `[min_x, min_y, max_x, max_y]`; the default (approximate) uses control points.
    pub fn bounds(&self) -> [f32; 4] {
        self.calculate_bounds(true)
    }

    /// Bounds as an `egui::Rect`.
    pub fn bounds_rect(&self) -> egui::Rect {
        let b = self.bounds();
        egui::Rect::from_min_max(egui::pos2(b[0], b[1]), egui::pos2(b[2], b[3]))
    }

    /// Bounds, either approximate (control points included) or tight.
    pub fn calculate_bounds(&self, approximate: bool) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::NEG_INFINITY, f32::NEG_INFINITY];
        for c in &self.cubics {
            let cb = c.calculate_bounds(approximate);
            b = [
                b[0].min(cb[0]),
                b[1].min(cb[1]),
                b[2].max(cb[2]),
                b[3].max(cb[3]),
            ];
        }
        b
    }

    /// Square bounds around the center that contain the shape under any rotation.
    pub fn calculate_max_bounds(&self) -> [f32; 4] {
        let mut max_dist_sq = 0.0f32;
        for c in &self.cubics {
            let a = c.anchor0() - self.center;
            let m = c.point_on_curve(0.5) - self.center;
            max_dist_sq = max_dist_sq.max(a.distance_squared().max(m.distance_squared()));
        }
        let d = max_dist_sq.sqrt();
        [
            self.center.x - d,
            self.center.y - d,
            self.center.x + d,
            self.center.y + d,
        ]
    }
}

/// Options for [`RoundedPolygon::star`].
#[derive(Debug, Clone)]
pub struct StarOptions {
    pub num_vertices_per_radius: usize,
    pub radius: f32,
    pub inner_radius: f32,
    pub rounding: CornerRounding,
    pub inner_rounding: Option<CornerRounding>,
    pub per_vertex_rounding: Option<Vec<CornerRounding>>,
    pub center: Point,
}

impl Default for StarOptions {
    fn default() -> Self {
        Self {
            num_vertices_per_radius: 5,
            radius: 1.0,
            inner_radius: 0.5,
            rounding: CornerRounding::UNROUNDED,
            inner_rounding: None,
            per_vertex_rounding: None,
            center: Point::ZERO,
        }
    }
}

/// Options for [`RoundedPolygon::pill_star`].
#[derive(Debug, Clone)]
pub struct PillStarOptions {
    pub width: f32,
    pub height: f32,
    pub num_vertices_per_radius: usize,
    pub inner_radius_ratio: f32,
    pub rounding: CornerRounding,
    pub inner_rounding: Option<CornerRounding>,
    pub per_vertex_rounding: Option<Vec<CornerRounding>>,
    pub vertex_spacing: f32,
    pub start_location: f32,
    pub center: Point,
}

impl Default for PillStarOptions {
    fn default() -> Self {
        Self {
            width: 2.0,
            height: 1.0,
            num_vertices_per_radius: 8,
            inner_radius_ratio: 0.5,
            rounding: CornerRounding::UNROUNDED,
            inner_rounding: None,
            per_vertex_rounding: None,
            vertex_spacing: 0.5,
            start_location: 0.0,
            center: Point::ZERO,
        }
    }
}

fn per_vertex_from_inner(
    n: usize,
    rounding: CornerRounding,
    inner: Option<CornerRounding>,
    explicit: Option<&[CornerRounding]>,
) -> Option<Vec<CornerRounding>> {
    match (explicit, inner) {
        (Some(e), _) => Some(e.to_vec()),
        (None, Some(inner)) => Some((0..n).flat_map(|_| [rounding, inner]).collect()),
        (None, None) => None,
    }
}

fn calculate_center(vertices: &[f32]) -> Point {
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    for v in vertices.as_chunks::<2>().0 {
        cx += v[0];
        cy += v[1];
    }
    let n = (vertices.len() / 2) as f32;
    Point::new(cx / n, cy / n)
}

fn build_cubics(features: &[Feature], center: Point) -> Vec<Cubic> {
    let mut out: Vec<Cubic> = Vec::new();
    let mut first_cubic: Option<Cubic> = None;
    let mut last_cubic: Option<Cubic> = None;
    let mut split_start: Option<[Cubic; 2]> = None;
    let mut split_end: Option<[Cubic; 2]> = None;

    // A leading corner of three cubics is split in the middle so the outline starts and ends
    // mid-corner.
    if let Some(first) = features.first()
        && first.cubics().len() == 3
    {
        let c = first.cubics();
        let (start, end) = c[1].split(0.5);
        split_start = Some([c[0], start]);
        split_end = Some([end, c[2]]);
    }

    for i in 0..=features.len() {
        let feature_cubics: &[Cubic] = if i == 0 && split_end.is_some() {
            split_end.as_ref().map_or(&[][..], |s| &s[..])
        } else if i == features.len() {
            match &split_start {
                Some(s) => &s[..],
                None => break,
            }
        } else {
            features[i].cubics()
        };
        for cubic in feature_cubics {
            if !cubic.zero_length() {
                if let Some(l) = last_cubic {
                    out.push(l);
                }
                last_cubic = Some(*cubic);
                if first_cubic.is_none() {
                    first_cubic = Some(*cubic);
                }
            } else if let Some(l) = last_cubic.as_mut() {
                l.points[6] = cubic.points[6];
                l.points[7] = cubic.points[7];
            }
        }
    }

    if let (Some(l), Some(f)) = (last_cubic, first_cubic) {
        out.push(Cubic::new(
            l.points[0],
            l.points[1],
            l.points[2],
            l.points[3],
            l.points[4],
            l.points[5],
            f.points[0],
            f.points[1],
        ));
    } else {
        out.push(Cubic::empty(center.x, center.y));
    }
    out
}

fn vertices_from_num_verts(num_vertices: usize, radius: f32, center: Point) -> Vec<f32> {
    let mut result = Vec::with_capacity(num_vertices * 2);
    for i in 0..num_vertices {
        let v = radial_to_cartesian(
            radius,
            PI / num_vertices as f32 * 2.0 * i as f32,
            Point::ZERO,
        ) + center;
        result.push(v.x);
        result.push(v.y);
    }
    result
}

fn star_vertices(n: usize, radius: f32, inner_radius: f32, center: Point) -> Vec<f32> {
    let mut result = Vec::with_capacity(n * 4);
    for i in 0..n {
        let v = radial_to_cartesian(radius, PI / n as f32 * 2.0 * i as f32, Point::ZERO);
        result.push(v.x + center.x);
        result.push(v.y + center.y);
        let v = radial_to_cartesian(
            inner_radius,
            PI / n as f32 * (2 * i + 1) as f32,
            Point::ZERO,
        );
        result.push(v.x + center.x);
        result.push(v.y + center.y);
    }
    result
}

fn pill_star_vertices(o: &PillStarOptions) -> Vec<f32> {
    let n = o.num_vertices_per_radius;
    let endcap_radius = o.width.min(o.height);
    let v_seg_len = (o.height - o.width).max(0.0);
    let h_seg_len = (o.width - o.height).max(0.0);
    let v_seg_half = v_seg_len / 2.0;
    let h_seg_half = h_seg_len / 2.0;

    let circle_perimeter = crate::point::TWO_PI
        * endcap_radius
        * interpolate(o.inner_radius_ratio, 1.0, o.vertex_spacing);
    let perimeter = 2.0 * h_seg_len + 2.0 * v_seg_len + circle_perimeter;

    let mut sections = [0.0f32; 11];
    sections[1] = v_seg_len / 2.0;
    sections[2] = sections[1] + circle_perimeter / 4.0;
    sections[3] = sections[2] + h_seg_len;
    sections[4] = sections[3] + circle_perimeter / 4.0;
    sections[5] = sections[4] + v_seg_len;
    sections[6] = sections[5] + circle_perimeter / 4.0;
    sections[7] = sections[6] + h_seg_len;
    sections[8] = sections[7] + circle_perimeter / 4.0;
    sections[9] = sections[8] + v_seg_len / 2.0;
    sections[10] = perimeter;

    let t_per_vertex = perimeter / (2 * n) as f32;
    let mut inner = false;
    let mut curr_sec = 0usize;
    let mut sec_start = 0.0f32;
    let mut sec_end = sections[1];
    let mut t = o.start_location * perimeter;
    let mut result = Vec::with_capacity(n * 4);

    let rect_br = Point::new(h_seg_half, v_seg_half);
    let rect_bl = Point::new(-h_seg_half, v_seg_half);
    let rect_tl = Point::new(-h_seg_half, -v_seg_half);
    let rect_tr = Point::new(h_seg_half, -v_seg_half);

    for _ in 0..n * 2 {
        let bounded_t = t % perimeter;
        if bounded_t < sec_start {
            curr_sec = 0;
        }
        // Guard against runaway looping on degenerate input.
        let mut guard = 0;
        while bounded_t >= sections[(curr_sec + 1) % sections.len()] && guard < 32 {
            curr_sec = (curr_sec + 1) % sections.len();
            sec_start = sections[curr_sec];
            sec_end = sections[(curr_sec + 1) % sections.len()];
            guard += 1;
        }
        let t_in_section = bounded_t - sec_start;
        let t_prop = t_in_section / (sec_end - sec_start);
        let r = if inner {
            endcap_radius * o.inner_radius_ratio
        } else {
            endcap_radius
        };
        let vertex = match curr_sec {
            0 => Point::new(r, t_prop * v_seg_half),
            1 => radial_to_cartesian(r, t_prop * PI / 2.0, Point::ZERO) + rect_br,
            2 => Point::new(h_seg_half - t_prop * h_seg_len, r),
            3 => radial_to_cartesian(r, PI / 2.0 + t_prop * PI / 2.0, Point::ZERO) + rect_bl,
            4 => Point::new(-r, v_seg_half - t_prop * v_seg_len),
            5 => radial_to_cartesian(r, PI + t_prop * PI / 2.0, Point::ZERO) + rect_tl,
            6 => Point::new(-h_seg_half + t_prop * h_seg_len, -r),
            7 => radial_to_cartesian(r, PI * 1.5 + t_prop * PI / 2.0, Point::ZERO) + rect_tr,
            _ => Point::new(r, -v_seg_half + t_prop * v_seg_half),
        };
        result.push(vertex.x + o.center.x);
        result.push(vertex.y + o.center.y);
        t += t_per_vertex;
        inner = !inner;
    }
    result
}

/// One polygon corner, rounded with a circular arc plus optional smoothing flanks.
struct RoundedCorner {
    p0: Point,
    p1: Point,
    p2: Point,
    d1: Point,
    d2: Point,
    corner_radius: f32,
    smoothing: f32,
    expected_round_cut: f32,
}

impl RoundedCorner {
    fn new(p0: Point, p1: Point, p2: Point, rounding: CornerRounding) -> Self {
        let v01 = p0 - p1;
        let v21 = p2 - p1;
        let d01 = v01.distance();
        let d21 = v21.distance();
        if d01 > 0.0 && d21 > 0.0 {
            let d1 = v01 / d01;
            let d2 = v21 / d21;
            let cos_angle = d1.dot(d2);
            let sin_angle = (1.0 - square(cos_angle)).max(0.0).sqrt();
            let expected_round_cut = if sin_angle > 1e-3 {
                rounding.radius * (cos_angle + 1.0) / sin_angle
            } else {
                0.0
            };
            Self {
                p0,
                p1,
                p2,
                d1,
                d2,
                corner_radius: rounding.radius,
                smoothing: rounding.smoothing,
                expected_round_cut,
            }
        } else {
            Self {
                p0,
                p1,
                p2,
                d1: Point::ZERO,
                d2: Point::ZERO,
                corner_radius: 0.0,
                smoothing: 0.0,
                expected_round_cut: 0.0,
            }
        }
    }

    fn expected_cut(&self) -> f32 {
        (1.0 + self.smoothing) * self.expected_round_cut
    }

    fn cubics(&self, allowed_cut0: f32, allowed_cut1: f32) -> Vec<Cubic> {
        let allowed_cut = allowed_cut0.min(allowed_cut1);
        if self.expected_round_cut < DISTANCE_EPSILON
            || allowed_cut < DISTANCE_EPSILON
            || self.corner_radius < DISTANCE_EPSILON
        {
            return vec![Cubic::straight_line(
                self.p1.x, self.p1.y, self.p1.x, self.p1.y,
            )];
        }
        let actual_round_cut = allowed_cut.min(self.expected_round_cut);
        let actual_smoothing0 = self.actual_smoothing(allowed_cut0);
        let actual_smoothing1 = self.actual_smoothing(allowed_cut1);
        let actual_r = self.corner_radius * actual_round_cut / self.expected_round_cut;
        let center_distance = (square(actual_r) + square(actual_round_cut)).sqrt();
        let center = self.p1 + ((self.d1 + self.d2) / 2.0).direction() * center_distance;
        let circle_intersection0 = self.p1 + self.d1 * actual_round_cut;
        let circle_intersection2 = self.p1 + self.d2 * actual_round_cut;
        let flanking0 = self.flanking_curve(
            actual_round_cut,
            actual_smoothing0,
            self.p0,
            circle_intersection0,
            circle_intersection2,
            center,
            actual_r,
        );
        let flanking2 = self
            .flanking_curve(
                actual_round_cut,
                actual_smoothing1,
                self.p2,
                circle_intersection2,
                circle_intersection0,
                center,
                actual_r,
            )
            .reverse();
        vec![
            flanking0,
            Cubic::circular_arc(center, flanking0.anchor1(), flanking2.anchor0()),
            flanking2,
        ]
    }

    fn actual_smoothing(&self, allowed_cut: f32) -> f32 {
        let expected_cut = self.expected_cut();
        if allowed_cut > expected_cut {
            self.smoothing
        } else if allowed_cut > self.expected_round_cut {
            self.smoothing * (allowed_cut - self.expected_round_cut)
                / (expected_cut - self.expected_round_cut)
        } else {
            0.0
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn flanking_curve(
        &self,
        actual_round_cut: f32,
        actual_smoothing: f32,
        side_start: Point,
        circle_segment_intersection: Point,
        other_circle_segment_intersection: Point,
        circle_center: Point,
        actual_r: f32,
    ) -> Cubic {
        let corner = self.p1;
        let side_direction = (side_start - corner).direction();
        let curve_start = corner + side_direction * actual_round_cut * (1.0 + actual_smoothing);
        let p = circle_segment_intersection.lerp(
            (circle_segment_intersection + other_circle_segment_intersection) / 2.0,
            actual_smoothing,
        );
        let curve_end = circle_center
            + direction_vector(p.x - circle_center.x, p.y - circle_center.y) * actual_r;
        let circle_tangent = (curve_end - circle_center).rotate90();
        let anchor_end = line_intersection(side_start, side_direction, curve_end, circle_tangent)
            .unwrap_or(circle_segment_intersection);
        let anchor_start = (curve_start + anchor_end * 2.0) / 3.0;
        Cubic::from_points(curve_start, anchor_start, anchor_end, curve_end)
    }
}

fn line_intersection(p0: Point, d0: Point, p1: Point, d1: Point) -> Option<Point> {
    let rotated_d1 = d1.rotate90();
    let den = d0.dot(rotated_d1);
    if den.abs() < DISTANCE_EPSILON {
        return None;
    }
    let num = (p1 - p0).dot(rotated_d1);
    if den.abs() < DISTANCE_EPSILON * num.abs() {
        return None;
    }
    let k = num / den;
    Some(p0 + d0 * k)
}
