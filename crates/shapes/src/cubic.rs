//! Port of `Cubic.kt`: a cubic Bezier segment stored as `[a0x, a0y, c0x, c0y, c1x, c1y, a1x, a1y]`.

use crate::point::{DISTANCE_EPSILON, Point, convex, direction_vector, distance, interpolate};

/// A cubic Bezier curve: two anchors and two control points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cubic {
    pub(crate) points: [f32; 8],
}

impl Cubic {
    /// Builds a cubic from raw coordinates.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub const fn new(
        anchor0_x: f32,
        anchor0_y: f32,
        control0_x: f32,
        control0_y: f32,
        control1_x: f32,
        control1_y: f32,
        anchor1_x: f32,
        anchor1_y: f32,
    ) -> Self {
        Self {
            points: [
                anchor0_x, anchor0_y, control0_x, control0_y, control1_x, control1_y, anchor1_x,
                anchor1_y,
            ],
        }
    }

    /// Builds a cubic from the eight raw coordinates.
    #[inline]
    pub const fn from_array(points: [f32; 8]) -> Self {
        Self { points }
    }

    /// Builds a cubic from anchor/control points.
    #[inline]
    pub fn from_points(a0: Point, c0: Point, c1: Point, a1: Point) -> Self {
        Self::new(a0.x, a0.y, c0.x, c0.y, c1.x, c1.y, a1.x, a1.y)
    }

    /// The raw coordinates `[a0x, a0y, c0x, c0y, c1x, c1y, a1x, a1y]`.
    #[inline]
    pub fn as_array(&self) -> &[f32; 8] {
        &self.points
    }

    #[inline]
    pub fn anchor0(&self) -> Point {
        Point::new(self.points[0], self.points[1])
    }
    #[inline]
    pub fn control0(&self) -> Point {
        Point::new(self.points[2], self.points[3])
    }
    #[inline]
    pub fn control1(&self) -> Point {
        Point::new(self.points[4], self.points[5])
    }
    #[inline]
    pub fn anchor1(&self) -> Point {
        Point::new(self.points[6], self.points[7])
    }

    /// A degenerate cubic sitting at `(x, y)`.
    pub fn empty(x: f32, y: f32) -> Self {
        Self::new(x, y, x, y, x, y, x, y)
    }

    /// A straight line from `(x0, y0)` to `(x1, y1)` as a cubic.
    pub fn straight_line(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self::new(
            x0,
            y0,
            interpolate(x0, x1, 1.0 / 3.0),
            interpolate(y0, y1, 1.0 / 3.0),
            interpolate(x0, x1, 2.0 / 3.0),
            interpolate(y0, y1, 2.0 / 3.0),
            x1,
            y1,
        )
    }

    /// A cubic approximating the circular arc from `p0` to `p1` around `center`.
    pub fn circular_arc(center: Point, p0: Point, p1: Point) -> Self {
        let p0d = direction_vector(p0.x - center.x, p0.y - center.y);
        let p1d = direction_vector(p1.x - center.x, p1.y - center.y);
        let rotated_p0 = p0d.rotate90();
        let rotated_p1 = p1d.rotate90();
        let clockwise = rotated_p0.x * (p1.x - center.x) + rotated_p0.y * (p1.y - center.y) >= 0.0;
        let cosa = p0d.dot(p1d);
        if cosa > 0.999 {
            return Self::straight_line(p0.x, p0.y, p1.x, p1.y);
        }
        let k = distance(p0.x - center.x, p0.y - center.y) * 4.0 / 3.0
            * ((2.0 * (1.0 - cosa)).sqrt() - (1.0 - cosa * cosa).sqrt())
            / (1.0 - cosa)
            * if clockwise { 1.0 } else { -1.0 };
        Self::new(
            p0.x,
            p0.y,
            p0.x + rotated_p0.x * k,
            p0.y + rotated_p0.y * k,
            p1.x - rotated_p1.x * k,
            p1.y - rotated_p1.y * k,
            p1.x,
            p1.y,
        )
    }

    /// Point on the curve at parameter `t` in `[0, 1]`.
    pub fn point_on_curve(&self, t: f32) -> Point {
        let u = 1.0 - t;
        let p = &self.points;
        Point::new(
            p[0] * (u * u * u)
                + p[2] * (3.0 * t * u * u)
                + p[4] * (3.0 * t * t * u)
                + p[6] * (t * t * t),
            p[1] * (u * u * u)
                + p[3] * (3.0 * t * u * u)
                + p[5] * (3.0 * t * t * u)
                + p[7] * (t * t * t),
        )
    }

    /// True when both anchors coincide (within `1e-4`).
    pub fn zero_length(&self) -> bool {
        (self.points[0] - self.points[6]).abs() < DISTANCE_EPSILON
            && (self.points[1] - self.points[7]).abs() < DISTANCE_EPSILON
    }

    /// Whether the turn at the end of this cubic towards `next` is convex.
    pub fn convex_to(&self, next: &Cubic) -> bool {
        convex(self.anchor0(), self.anchor1(), next.anchor1())
    }

    /// Bounding box `[min_x, min_y, max_x, max_y]`. With `approximate` the control
    /// points are included (a cheap superset); otherwise the tight curve extrema are used.
    pub fn calculate_bounds(&self, approximate: bool) -> [f32; 4] {
        let p = &self.points;
        if self.zero_length() {
            return [p[0], p[1], p[0], p[1]];
        }
        let mut min_x = p[0].min(p[6]);
        let mut min_y = p[1].min(p[7]);
        let mut max_x = p[0].max(p[6]);
        let mut max_y = p[1].max(p[7]);
        if approximate {
            return [
                min_x.min(p[2].min(p[4])),
                min_y.min(p[3].min(p[5])),
                max_x.max(p[2].max(p[4])),
                max_y.max(p[3].max(p[5])),
            ];
        }
        // Solve the derivative's quadratic for each axis.
        for axis in 0..2 {
            let (a0, c0, c1, a1) = (p[axis], p[2 + axis], p[4 + axis], p[6 + axis]);
            let qa = -a0 + 3.0 * c0 - 3.0 * c1 + a1;
            let qb = 2.0 * a0 - 4.0 * c0 + 2.0 * c1;
            let qc = -a0 + c0;
            let mut roots = [f32::NAN; 2];
            if qa.abs() < DISTANCE_EPSILON {
                if qb != 0.0 {
                    roots[0] = 2.0 * qc / (-2.0 * qb);
                }
            } else {
                let disc = qb * qb - 4.0 * qa * qc;
                if disc >= 0.0 {
                    roots[0] = (-qb + disc.sqrt()) / (2.0 * qa);
                    roots[1] = (-qb - disc.sqrt()) / (2.0 * qa);
                }
            }
            for t in roots {
                if (0.0..=1.0).contains(&t) {
                    let pt = self.point_on_curve(t);
                    let (v, lo, hi) = if axis == 0 {
                        (pt.x, &mut min_x, &mut max_x)
                    } else {
                        (pt.y, &mut min_y, &mut max_y)
                    };
                    if v < *lo {
                        *lo = v;
                    }
                    if v > *hi {
                        *hi = v;
                    }
                }
            }
        }
        [min_x, min_y, max_x, max_y]
    }

    /// Splits at parameter `t` into two cubics that join continuously.
    pub fn split(&self, t: f32) -> (Cubic, Cubic) {
        let u = 1.0 - t;
        let p = &self.points;
        let pt = self.point_on_curve(t);
        (
            Cubic::new(
                p[0],
                p[1],
                p[0] * u + p[2] * t,
                p[1] * u + p[3] * t,
                p[0] * (u * u) + p[2] * (2.0 * u * t) + p[4] * (t * t),
                p[1] * (u * u) + p[3] * (2.0 * u * t) + p[5] * (t * t),
                pt.x,
                pt.y,
            ),
            Cubic::new(
                pt.x,
                pt.y,
                p[2] * (u * u) + p[4] * (2.0 * u * t) + p[6] * (t * t),
                p[3] * (u * u) + p[5] * (2.0 * u * t) + p[7] * (t * t),
                p[4] * u + p[6] * t,
                p[5] * u + p[7] * t,
                p[6],
                p[7],
            ),
        )
    }

    /// The same curve traversed backwards.
    pub fn reverse(&self) -> Cubic {
        let p = &self.points;
        Cubic::new(p[6], p[7], p[4], p[5], p[2], p[3], p[0], p[1])
    }

    /// Applies `f` to all four points.
    pub fn transformed(&self, f: impl Fn(f32, f32) -> (f32, f32)) -> Cubic {
        let mut out = self.points;
        for i in (0..8).step_by(2) {
            let (x, y) = f(out[i], out[i + 1]);
            out[i] = x;
            out[i + 1] = y;
        }
        Cubic { points: out }
    }

    /// Component-wise interpolation of two cubics.
    pub fn interpolate(a: &Cubic, b: &Cubic, progress: f32) -> Cubic {
        let mut out = [0.0; 8];
        for (i, o) in out.iter_mut().enumerate() {
            *o = interpolate(a.points[i], b.points[i], progress);
        }
        Cubic { points: out }
    }
}

impl std::ops::Add for Cubic {
    type Output = Cubic;
    fn add(self, o: Cubic) -> Cubic {
        let mut r = self.points;
        for (v, w) in r.iter_mut().zip(o.points) {
            *v += w;
        }
        Cubic { points: r }
    }
}

impl std::ops::Mul<f32> for Cubic {
    type Output = Cubic;
    fn mul(self, k: f32) -> Cubic {
        Cubic {
            points: self.points.map(|v| v * k),
        }
    }
}

impl std::ops::Div<f32> for Cubic {
    type Output = Cubic;
    fn div(self, k: f32) -> Cubic {
        self * (1.0 / k)
    }
}
