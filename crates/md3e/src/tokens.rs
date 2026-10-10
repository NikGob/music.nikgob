//! Design tokens that are not colour or type: shape, state layers, elevation,
//! spacing, window size classes and motion springs.

use egui::{CornerRadius, Rect, epaint::Shadow};
use nikgob_motion::{Spring, tokens as springs};

/// The corner-radius scale in dp (tokens.md section 3.1).
pub mod shape {
    pub const NONE: f32 = 0.0;
    pub const EXTRA_SMALL: f32 = 4.0;
    pub const SMALL: f32 = 8.0;
    pub const MEDIUM: f32 = 12.0;
    pub const LARGE: f32 = 16.0;
    pub const LARGE_INCREASED: f32 = 20.0;
    pub const EXTRA_LARGE: f32 = 28.0;
    pub const EXTRA_LARGE_INCREASED: f32 = 32.0;
    pub const EXTRA_EXTRA_LARGE: f32 = 48.0;
    /// Fully round: clamp with [`corner`] or [`clamp_radius`].
    pub const FULL: f32 = f32::INFINITY;

    /// `radius` limited to half the shorter side, so [`FULL`] gives a pill
    /// or a circle.
    pub fn clamp_radius(radius: f32, rect: egui::Rect) -> f32 {
        radius.clamp(0.0, rect.width().min(rect.height()).max(0.0) / 2.0)
    }

    /// The corner for `rect`. egui stores whole-point radii (`u8`), so a
    /// radius animated by a spring moves in 1 pt steps.
    pub fn corner(radius: f32, rect: egui::Rect) -> egui::CornerRadius {
        egui::CornerRadius::same(clamp_radius(radius, rect).round().min(255.0) as u8)
    }

    /// Optical roundness: the radius of a shape inset by `padding` inside a
    /// container of radius `outer`.
    pub fn inner_radius(outer: f32, padding: f32) -> f32 {
        (outer - padding).max(0.0)
    }
}

/// [`shape::corner`] as a free function for call sites that read better so.
pub fn corner_radius(radius: f32, rect: Rect) -> CornerRadius {
    shape::corner(radius, rect)
}

/// State-layer and disabled opacities (tokens.md section 5). A state layer is
/// the content colour at this opacity over the container.
pub mod state {
    pub const HOVER: f32 = 0.08;
    pub const FOCUS: f32 = 0.10;
    pub const PRESSED: f32 = 0.10;
    pub const DRAGGED: f32 = 0.16;
    /// `on-surface` at this opacity is the disabled container (buttons, chips).
    pub const DISABLED_CONTAINER: f32 = 0.12;
    /// `on-surface` at this opacity is disabled content.
    pub const DISABLED_CONTENT: f32 = 0.38;
}

/// Focus indicator: 3 dp ring in `secondary`, 2 dp outside the component.
pub mod focus {
    pub const RING_WIDTH: f32 = 3.0;
    pub const RING_OFFSET: f32 = 2.0;
}

/// Elevation levels (dp: 0, 1, 3, 6, 8, 12).
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum Elevation {
    Level0,
    Level1,
    Level2,
    Level3,
    Level4,
    Level5,
}

impl Elevation {
    pub const fn dp(self) -> f32 {
        match self {
            Self::Level0 => 0.0,
            Self::Level1 => 1.0,
            Self::Level2 => 3.0,
            Self::Level3 => 6.0,
            Self::Level4 => 8.0,
            Self::Level5 => 12.0,
        }
    }

    /// One egui shadow standing in for M3's two-layer recipe.
    ///
    /// M3 draws a key shadow (30% black, small blur, y 1-4) plus an ambient
    /// one (15% black, wider blur and spread, y 1-8). egui paints one shadow
    /// per shape, so we take the ambient layer's blur, an offset between the
    /// two layers, half its spread, and an alpha between the two:
    ///
    /// | level | offset y | blur | spread | alpha |
    /// |---|---|---|---|---|
    /// | 1 | 1 | 3 | 0 | 56 |
    /// | 2 | 2 | 6 | 1 | 60 |
    /// | 3 | 3 | 9 | 1 | 66 |
    /// | 4 | 4 | 11 | 2 | 72 |
    /// | 5 | 6 | 14 | 3 | 78 |
    ///
    /// Depth is mostly tonal in M3; use this only where the spec asks for a
    /// shadow (FAB, menus, dialogs, floating toolbar).
    pub const fn shadow(self) -> Shadow {
        let (y, blur, spread, alpha) = match self {
            Self::Level0 => return Shadow::NONE,
            Self::Level1 => (1, 3, 0, 56),
            Self::Level2 => (2, 6, 1, 60),
            Self::Level3 => (3, 9, 1, 66),
            Self::Level4 => (4, 11, 2, 72),
            Self::Level5 => (6, 14, 3, 78),
        };
        Shadow {
            offset: [0, y],
            blur,
            spread,
            color: egui::Color32::from_black_alpha(alpha),
        }
    }
}

/// Spacing on the 4 dp grid.
pub mod spacing {
    pub const UNIT: f32 = 4.0;
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 12.0;
    pub const L: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    /// Space between panes.
    pub const PANE_GAP: f32 = 24.0;
    /// Minimum touch target.
    pub const TARGET: f32 = 48.0;

    /// `n` grid units.
    pub const fn dp(n: f32) -> f32 {
        n * UNIT
    }
}

/// Window width classes (dp).
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum SizeClass {
    /// Below 600.
    Compact,
    /// 600 to 839.
    Medium,
    /// 840 to 1199.
    Expanded,
    /// 1200 to 1599.
    Large,
    /// 1600 and up.
    ExtraLarge,
}

/// The size class for a window `width` in points.
pub fn size_class(width: f32) -> SizeClass {
    match width {
        w if w < 600.0 => SizeClass::Compact,
        w if w < 840.0 => SizeClass::Medium,
        w if w < 1200.0 => SizeClass::Expanded,
        w if w < 1600.0 => SizeClass::Large,
        _ => SizeClass::ExtraLarge,
    }
}

impl SizeClass {
    /// Window margin: 16 on compact, 24 above.
    pub const fn margin(self) -> f32 {
        match self {
            Self::Compact => 16.0,
            _ => 24.0,
        }
    }
}

/// The six spring tokens a component animates with. Spatial springs move
/// things and may overshoot; effects springs (colour, alpha) never do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub fast_spatial: Spring,
    pub default_spatial: Spring,
    pub slow_spatial: Spring,
    pub fast_effects: Spring,
    pub default_effects: Spring,
    pub slow_effects: Spring,
}

impl Motion {
    /// The Expressive scheme: bouncy spatial springs.
    pub const fn expressive() -> Self {
        Self {
            fast_spatial: springs::EXPRESSIVE_FAST_SPATIAL,
            default_spatial: springs::EXPRESSIVE_DEFAULT_SPATIAL,
            slow_spatial: springs::EXPRESSIVE_SLOW_SPATIAL,
            fast_effects: springs::FAST_EFFECTS,
            default_effects: springs::DEFAULT_EFFECTS,
            slow_effects: springs::SLOW_EFFECTS,
        }
    }

    /// The Standard scheme: near-critically damped spatial springs.
    pub const fn standard() -> Self {
        Self {
            fast_spatial: springs::STANDARD_FAST_SPATIAL,
            default_spatial: springs::STANDARD_DEFAULT_SPATIAL,
            slow_spatial: springs::STANDARD_SLOW_SPATIAL,
            ..Self::expressive()
        }
    }

    /// Expressive speeds with no overshoot, for reduced motion.
    pub const fn reduced() -> Self {
        let e = Self::expressive();
        Self {
            fast_spatial: e.fast_spatial.without_bounce(),
            default_spatial: e.default_spatial.without_bounce(),
            slow_spatial: e.slow_spatial.without_bounce(),
            fast_effects: e.fast_effects,
            default_effects: e.default_effects,
            slow_effects: e.slow_effects,
        }
    }
}

impl Default for Motion {
    fn default() -> Self {
        Self::expressive()
    }
}

#[cfg(test)]
mod tests {
    use egui::{Rect, pos2};

    use super::*;

    #[test]
    fn size_classes_follow_the_breakpoints() {
        assert_eq!(size_class(599.0), SizeClass::Compact);
        assert_eq!(size_class(600.0), SizeClass::Medium);
        assert_eq!(size_class(839.9), SizeClass::Medium);
        assert_eq!(size_class(840.0), SizeClass::Expanded);
        assert_eq!(size_class(1199.0), SizeClass::Expanded);
        assert_eq!(size_class(1200.0), SizeClass::Large);
        assert_eq!(size_class(1599.0), SizeClass::Large);
        assert_eq!(size_class(1600.0), SizeClass::ExtraLarge);
        assert_eq!(SizeClass::Compact.margin(), 16.0);
        assert_eq!(SizeClass::Large.margin(), 24.0);
    }

    #[test]
    fn full_radius_is_a_pill() {
        let rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 40.0));
        assert_eq!(shape::clamp_radius(shape::FULL, rect), 20.0);
        assert_eq!(shape::clamp_radius(shape::MEDIUM, rect), 12.0);
        assert_eq!(shape::corner(shape::FULL, rect), CornerRadius::same(20));
        assert_eq!(shape::inner_radius(48.0, 14.0), 34.0);
        assert_eq!(shape::inner_radius(8.0, 14.0), 0.0);
    }

    #[test]
    fn shadows_grow_with_level() {
        assert_eq!(Elevation::Level0.shadow(), Shadow::NONE);
        let levels = [
            Elevation::Level1,
            Elevation::Level2,
            Elevation::Level3,
            Elevation::Level4,
            Elevation::Level5,
        ];
        for pair in levels.windows(2) {
            let (a, b) = (pair[0].shadow(), pair[1].shadow());
            assert!(b.blur > a.blur && b.offset[1] > a.offset[1], "{pair:?}");
            assert!(b.color.a() > a.color.a());
        }
    }

    #[test]
    fn reduced_motion_never_overshoots() {
        let r = Motion::reduced();
        for s in [r.fast_spatial, r.default_spatial, r.slow_spatial] {
            assert!(s.damping_ratio >= 1.0);
        }
        // Same stiffness as expressive: only the bounce goes.
        assert_eq!(
            r.fast_spatial.stiffness,
            Motion::expressive().fast_spatial.stiffness
        );
        assert!(Motion::expressive().fast_spatial.damping_ratio < 1.0);
        assert_eq!(Motion::default(), Motion::expressive());
    }
}
