//! 2D point/vector helpers and small numeric utilities (port of `Point.kt` / `Utils.kt`).

use std::ops::{Add, Div, Mul, Neg, Sub};

/// A 2D point or vector in shape space.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn distance(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    #[inline]
    pub fn distance_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    #[inline]
    pub fn dot(self, o: Point) -> f32 {
        self.x * o.x + self.y * o.y
    }

    /// True when `other` is clockwise from `self` (y pointing down gives screen-clockwise).
    #[inline]
    pub fn clockwise(self, o: Point) -> bool {
        self.x * o.y - self.y * o.x > 0.0
    }

    /// Unit vector in the same direction (zero for a zero vector).
    #[inline]
    pub(crate) fn direction(self) -> Point {
        let d = self.distance();
        if d > 0.0 { self / d } else { Point::ZERO }
    }

    #[inline]
    pub(crate) fn rotate90(self) -> Point {
        Point::new(-self.y, self.x)
    }

    /// Linear interpolation between two points.
    #[inline]
    pub fn lerp(self, other: Point, t: f32) -> Point {
        Point::new(
            interpolate(self.x, other.x, t),
            interpolate(self.y, other.y, t),
        )
    }
}

impl Add for Point {
    type Output = Point;
    #[inline]
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Point {
    type Output = Point;
    #[inline]
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f32> for Point {
    type Output = Point;
    #[inline]
    fn mul(self, k: f32) -> Point {
        Point::new(self.x * k, self.y * k)
    }
}
impl Div<f32> for Point {
    type Output = Point;
    #[inline]
    fn div(self, k: f32) -> Point {
        Point::new(self.x / k, self.y / k)
    }
}
impl Neg for Point {
    type Output = Point;
    #[inline]
    fn neg(self) -> Point {
        Point::new(-self.x, -self.y)
    }
}

impl From<(f32, f32)> for Point {
    fn from((x, y): (f32, f32)) -> Self {
        Point::new(x, y)
    }
}
impl From<Point> for egui::Pos2 {
    fn from(p: Point) -> Self {
        egui::pos2(p.x, p.y)
    }
}
impl From<egui::Pos2> for Point {
    fn from(p: egui::Pos2) -> Self {
        Point::new(p.x, p.y)
    }
}

pub(crate) const DISTANCE_EPSILON: f32 = 1e-4;
pub(crate) const ANGLE_EPSILON: f32 = 1e-6;
pub(crate) const RELAXED_DISTANCE_EPSILON: f32 = 5e-3;
pub(crate) const PI: f32 = std::f32::consts::PI;
pub(crate) const TWO_PI: f32 = 2.0 * std::f32::consts::PI;

#[inline]
pub(crate) fn distance(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

#[inline]
pub(crate) fn square(x: f32) -> f32 {
    x * x
}

/// Unit vector for a direction `(x, y)`; zero vector yields zero.
#[inline]
pub(crate) fn direction_vector(x: f32, y: f32) -> Point {
    Point::new(x, y).direction()
}

#[inline]
pub(crate) fn radial_to_cartesian(radius: f32, angle_radians: f32, center: Point) -> Point {
    Point::new(angle_radians.cos(), angle_radians.sin()) * radius + center
}

#[inline]
pub(crate) fn interpolate(start: f32, stop: f32, fraction: f32) -> f32 {
    (1.0 - fraction) * start + fraction * stop
}

#[inline]
pub(crate) fn positive_modulo(num: f32, modulus: f32) -> f32 {
    (num % modulus + modulus) % modulus
}

pub(crate) fn collinear_ish(a: Point, b: Point, c: Point, tolerance: f32) -> bool {
    let ab = (b - a).rotate90();
    let ac = c - a;
    let dot = ab.dot(ac).abs();
    let relative = tolerance * ab.distance() * ac.distance();
    dot < tolerance || dot < relative
}

pub(crate) fn convex(previous: Point, current: Point, next: Point) -> bool {
    (current - previous).clockwise(next - current)
}
