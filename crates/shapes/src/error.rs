//! Error type for shape construction and morphing.

use std::fmt;

/// Errors raised when a shape or morph cannot be built.
///
/// The Kotlin original throws `IllegalArgumentException` in these places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeError {
    /// A constructor argument was out of range or inconsistent.
    InvalidArgument(&'static str),
    /// The cubics of a polygon or feature are not contiguous.
    NotContiguous,
    /// Progress mapping or measuring failed while matching two polygons.
    Mapping(&'static str),
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument(m) => write!(f, "invalid argument: {m}"),
            Self::NotContiguous => f.write_str(
                "RoundedPolygon must be contiguous, with the anchor points of all curves \
                 matching the anchor points of the preceding and succeeding cubics",
            ),
            Self::Mapping(m) => write!(f, "morph mapping failed: {m}"),
        }
    }
}

impl std::error::Error for ShapeError {}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, ShapeError>;
