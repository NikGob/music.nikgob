//! Rounded polygons, shape morphing and the Material 3 Expressive shape library, rendered
//! with egui.
//!
//! This crate is a Rust port of Jetpack's `androidx.graphics.shapes` library
//! (`RoundedPolygon`, `CornerRounding`, `Cubic`, `Feature`, `Morph`, ...) and of Compose
//! Material3's `MaterialShapes`.
//!
//! Original sources: Copyright 2022-2025 The Android Open Source Project, licensed under the
//! Apache License, Version 2.0 (<https://www.apache.org/licenses/LICENSE-2.0>), from
//! <https://github.com/androidx/androidx> (`graphics/graphics-shapes` and
//! `compose/material3/material3`). This port keeps `f32` math and the original algorithms
//! (corner rounding with smoothing, feature detection, length-based feature mapping for
//! morphs) so results match Android's.
//!
//! ```
//! use nikgob_shapes::{MaterialShape, Morph};
//!
//! let morph = Morph::new(MaterialShape::Circle.polygon(), MaterialShape::Heart.polygon());
//! let cubics = morph.as_cubics(0.5);
//! assert!(!cubics.is_empty());
//! ```

mod corner_rounding;
mod cubic;
mod error;
mod feature;
mod mapping;
mod material;
mod measure;
mod morph;
pub mod paint;
mod point;
mod polygon;
pub mod triangulate;

pub use corner_rounding::CornerRounding;
pub use cubic::Cubic;
pub use error::{Result, ShapeError};
pub use feature::{Feature, detect_features};
pub use material::MaterialShape;
pub use morph::Morph;
pub use point::Point;
pub use polygon::{PillStarOptions, RoundedPolygon, StarOptions};

#[cfg(test)]
mod tests;
