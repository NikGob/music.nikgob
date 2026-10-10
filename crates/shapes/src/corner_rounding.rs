//! Port of `CornerRounding.kt`.

/// Describes how a polygon corner is rounded.
///
/// `radius` is the radius of the circular arc, `smoothing` (0..=1) is the extra
/// flanking-curve length as a fraction of the cut, giving a smoother transition
/// from the sides into the arc.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CornerRounding {
    pub radius: f32,
    pub smoothing: f32,
}

impl CornerRounding {
    /// No rounding.
    pub const UNROUNDED: CornerRounding = CornerRounding {
        radius: 0.0,
        smoothing: 0.0,
    };

    #[inline]
    pub const fn new(radius: f32, smoothing: f32) -> Self {
        Self { radius, smoothing }
    }

    /// Rounding with a radius and no smoothing.
    #[inline]
    pub const fn radius(radius: f32) -> Self {
        Self {
            radius,
            smoothing: 0.0,
        }
    }
}
