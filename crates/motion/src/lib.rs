//! Time-based spring animation shared by every design system.
//!
//! A spring is solved in closed form, so a value is a pure function of the
//! wall clock: nothing is stepped per frame. A hidden window that skips
//! frames shows the right state on the first frame back, and retargeting
//! mid-flight keeps position and velocity continuous.

/// A damped spring with unit mass, as Compose's `spring(dampingRatio, stiffness)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spring {
    pub damping_ratio: f32,
    pub stiffness: f32,
}

impl Spring {
    pub const fn new(damping_ratio: f32, stiffness: f32) -> Self {
        Self {
            damping_ratio,
            stiffness,
        }
    }

    /// The same speed without overshoot, for reduced motion.
    pub const fn without_bounce(self) -> Self {
        Self {
            damping_ratio: if self.damping_ratio < 1.0 {
                1.0
            } else {
                self.damping_ratio
            },
            stiffness: self.stiffness,
        }
    }

    /// Displacement from the target and velocity after `t` seconds, starting
    /// from displacement `x0` and velocity `v0`.
    pub fn solve(self, x0: f32, v0: f32, t: f32) -> (f32, f32) {
        let (x0, v0, t) = (f64::from(x0), f64::from(v0), f64::from(t.max(0.0)));
        let w0 = f64::from(self.stiffness).sqrt();
        let z = f64::from(self.damping_ratio);
        let (x, v) = if (z - 1.0).abs() < 1e-6 {
            // Critically damped.
            let c = v0 + w0 * x0;
            let e = (-w0 * t).exp();
            let x = e * (x0 + c * t);
            let v = e * (c - w0 * (x0 + c * t));
            (x, v)
        } else if z < 1.0 {
            let wd = w0 * (1.0 - z * z).sqrt();
            let a = x0;
            let b = (v0 + z * w0 * x0) / wd;
            let e = (-z * w0 * t).exp();
            let (s, c) = (wd * t).sin_cos();
            let x = e * (a * c + b * s);
            let v = e * ((b * wd - z * w0 * a) * c - (a * wd + z * w0 * b) * s);
            (x, v)
        } else {
            let root = (z * z - 1.0).sqrt();
            let r1 = -w0 * (z - root);
            let r2 = -w0 * (z + root);
            let c2 = (v0 - r1 * x0) / (r2 - r1);
            let c1 = x0 - c2;
            let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
            (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
        };
        (x as f32, v as f32)
    }
}

/// Material 3 motion-physics tokens (Compose `ExpressiveMotionTokens` /
/// `StandardMotionTokens`).
pub mod tokens {
    use super::Spring;

    pub const EXPRESSIVE_FAST_SPATIAL: Spring = Spring::new(0.6, 800.0);
    pub const EXPRESSIVE_DEFAULT_SPATIAL: Spring = Spring::new(0.8, 380.0);
    pub const EXPRESSIVE_SLOW_SPATIAL: Spring = Spring::new(0.8, 200.0);
    pub const STANDARD_FAST_SPATIAL: Spring = Spring::new(0.9, 1400.0);
    pub const STANDARD_DEFAULT_SPATIAL: Spring = Spring::new(0.9, 700.0);
    pub const STANDARD_SLOW_SPATIAL: Spring = Spring::new(0.9, 300.0);
    pub const FAST_EFFECTS: Spring = Spring::new(1.0, 3800.0);
    pub const DEFAULT_EFFECTS: Spring = Spring::new(1.0, 1600.0);
    pub const SLOW_EFFECTS: Spring = Spring::new(1.0, 800.0);
}

/// Up to four `f32` components a spring can drive independently.
pub trait Animatable: Copy {
    const LEN: usize;
    fn to_components(self) -> [f32; 4];
    fn from_components(c: [f32; 4]) -> Self;
}

impl Animatable for f32 {
    const LEN: usize = 1;
    fn to_components(self) -> [f32; 4] {
        [self, 0.0, 0.0, 0.0]
    }
    fn from_components(c: [f32; 4]) -> Self {
        c[0]
    }
}

impl Animatable for [f32; 2] {
    const LEN: usize = 2;
    fn to_components(self) -> [f32; 4] {
        [self[0], self[1], 0.0, 0.0]
    }
    fn from_components(c: [f32; 4]) -> Self {
        [c[0], c[1]]
    }
}

impl Animatable for [f32; 4] {
    const LEN: usize = 4;
    fn to_components(self) -> [f32; 4] {
        self
    }
    fn from_components(c: [f32; 4]) -> Self {
        c
    }
}

/// A value that follows its target along a spring, evaluated from a clock in
/// seconds (egui's `input.time`).
#[derive(Clone, Copy, Debug)]
pub struct Animated<T: Animatable> {
    from: [f32; 4],
    velocity: [f32; 4],
    target: [f32; 4],
    start: f64,
    spring: Spring,
    /// Below this distance and speed (per component) the value is at rest.
    threshold: f32,
    _t: std::marker::PhantomData<T>,
}

impl<T: Animatable> Animated<T> {
    /// A value at rest on `value`.
    pub fn new(value: T, spring: Spring) -> Self {
        let c = value.to_components();
        Self {
            from: c,
            velocity: [0.0; 4],
            target: c,
            start: 0.0,
            spring,
            threshold: 0.01,
            _t: std::marker::PhantomData,
        }
    }

    /// Rest threshold; 0.01 suits pixels, use ~0.001 for colours and 0..1 fractions.
    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.threshold = threshold;
        self
    }

    pub fn spring(&self) -> Spring {
        self.spring
    }

    pub fn target(&self) -> T {
        T::from_components(self.target)
    }

    fn state(&self, now: f64) -> ([f32; 4], [f32; 4]) {
        let t = (now - self.start) as f32;
        let mut x = [0.0; 4];
        let mut v = [0.0; 4];
        for i in 0..T::LEN {
            let (dx, dv) = self
                .spring
                .solve(self.from[i] - self.target[i], self.velocity[i], t);
            x[i] = self.target[i] + dx;
            v[i] = dv;
        }
        (x, v)
    }

    /// The value at `now`.
    pub fn value(&self, now: f64) -> T {
        if self.is_settled(now) {
            return T::from_components(self.target);
        }
        T::from_components(self.state(now).0)
    }

    pub fn velocity(&self, now: f64) -> T {
        T::from_components(self.state(now).1)
    }

    /// True once every component is within the threshold of its target and
    /// slower than the threshold per frame at 60 Hz.
    pub fn is_settled(&self, now: f64) -> bool {
        if self.from == self.target && self.velocity == [0.0; 4] {
            return true;
        }
        let (x, v) = self.state(now);
        (0..T::LEN).all(|i| {
            (x[i] - self.target[i]).abs() < self.threshold
                && (v[i] / 60.0).abs() < self.threshold
        })
    }

    /// Move toward `target`, keeping the current position and velocity.
    /// A no-op if the target is unchanged.
    pub fn set_target(&mut self, target: T, now: f64) {
        let target = target.to_components();
        if target == self.target {
            return;
        }
        let (x, v) = if self.is_settled(now) {
            (self.target, [0.0; 4])
        } else {
            self.state(now)
        };
        self.from = x;
        self.velocity = v;
        self.target = target;
        self.start = now;
    }

    /// Like [`Self::set_target`], switching to another spring for this move.
    pub fn animate_to(&mut self, target: T, spring: Spring, now: f64) {
        let (x, v) = if self.is_settled(now) {
            (self.target, [0.0; 4])
        } else {
            self.state(now)
        };
        self.from = x;
        self.velocity = v;
        self.target = target.to_components();
        self.start = now;
        self.spring = spring;
    }

    /// Jump to `value` with no animation.
    pub fn snap(&mut self, value: T) {
        let c = value.to_components();
        self.from = c;
        self.target = c;
        self.velocity = [0.0; 4];
    }
}

/// Seconds a spring needs to come to rest from a unit displacement.
pub fn settle_time(spring: Spring, threshold: f32) -> f32 {
    let mut t = 0.0;
    let mut last_moving = 0.0;
    while t < 10.0 {
        let (x, v) = spring.solve(1.0, 0.0, t);
        if x.abs() >= threshold || (v / 60.0).abs() >= threshold {
            last_moving = t;
        }
        t += 1.0 / 240.0;
    }
    last_moving
}

#[cfg(test)]
mod tests {
    use super::tokens::*;
    use super::*;

    #[test]
    fn starts_at_initial_and_ends_at_target() {
        for s in [EXPRESSIVE_FAST_SPATIAL, FAST_EFFECTS, Spring::new(1.5, 300.0)] {
            let (x, v) = s.solve(1.0, 0.0, 0.0);
            assert!((x - 1.0).abs() < 1e-5 && v.abs() < 1e-3);
            let (x, _) = s.solve(1.0, 0.0, 5.0);
            assert!(x.abs() < 1e-3, "{s:?} -> {x}");
        }
    }

    #[test]
    fn expressive_spatial_overshoots_effects_do_not() {
        let min = |s: Spring| {
            (0..400)
                .map(|i| s.solve(1.0, 0.0, i as f32 / 200.0).0)
                .fold(f32::MAX, f32::min)
        };
        assert!(min(EXPRESSIVE_FAST_SPATIAL) < -0.05);
        assert!(min(SLOW_EFFECTS) >= -1e-4);
    }

    #[test]
    fn velocity_matches_numerical_derivative() {
        for s in [EXPRESSIVE_DEFAULT_SPATIAL, DEFAULT_EFFECTS, Spring::new(2.0, 500.0)] {
            for i in 1..50 {
                let t = i as f32 / 100.0;
                let h = 1e-3;
                let num = (s.solve(1.0, 3.0, t + h).0 - s.solve(1.0, 3.0, t - h).0) / (2.0 * h);
                let (_, v) = s.solve(1.0, 3.0, t);
                assert!((num - v).abs() < 1e-2 * (1.0 + v.abs()), "{s:?} t={t} {num} vs {v}");
            }
        }
    }

    #[test]
    fn retarget_is_continuous() {
        let mut a = Animated::new(0.0_f32, EXPRESSIVE_FAST_SPATIAL);
        a.set_target(100.0, 0.0);
        let before = a.value(0.1);
        let vel = a.velocity(0.1);
        a.set_target(-50.0, 0.1);
        assert!((a.value(0.1) - before).abs() < 1e-3);
        assert!((a.velocity(0.1) - vel).abs() < 1e-2);
        assert!(a.is_settled(5.0));
        assert_eq!(a.value(5.0), -50.0);
    }

    #[test]
    fn settle_times_are_ordered() {
        let f = settle_time(EXPRESSIVE_FAST_SPATIAL, 0.002);
        let d = settle_time(EXPRESSIVE_DEFAULT_SPATIAL, 0.002);
        let s = settle_time(EXPRESSIVE_SLOW_SPATIAL, 0.002);
        assert!(f < d && d < s, "{f} {d} {s}");
    }
}
