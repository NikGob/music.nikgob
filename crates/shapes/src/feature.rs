//! Port of `Features.kt` and `FeatureDetector.kt`.

use crate::cubic::Cubic;
use crate::error::{Result, ShapeError};
use crate::point::{DISTANCE_EPSILON, RELAXED_DISTANCE_EPSILON, collinear_ish};

/// A feature of a polygon outline: a straight edge or a (convex/concave) corner.
///
/// Features drive the morph: only corners are matched between two shapes.
#[derive(Debug, Clone, PartialEq)]
pub enum Feature {
    Edge { cubics: Vec<Cubic> },
    Corner { cubics: Vec<Cubic>, convex: bool },
}

impl Feature {
    /// An edge feature without validation (a straight line made of cubics).
    pub fn edge(cubics: Vec<Cubic>) -> Feature {
        Feature::Edge { cubics }
    }

    /// A corner feature without validation.
    pub fn corner(cubics: Vec<Cubic>, convex: bool) -> Feature {
        Feature::Corner { cubics, convex }
    }

    /// Validated edge feature (`buildIgnorableFeature`).
    pub fn build_ignorable_feature(cubics: Vec<Cubic>) -> Result<Feature> {
        Self::validated(Feature::Edge { cubics })
    }

    /// Edge feature from a single cubic (`buildEdge`).
    pub fn build_edge(cubic: Cubic) -> Feature {
        Feature::Edge {
            cubics: vec![cubic],
        }
    }

    /// Validated convex corner.
    pub fn build_convex_corner(cubics: Vec<Cubic>) -> Result<Feature> {
        Self::validated(Feature::Corner {
            cubics,
            convex: true,
        })
    }

    /// Validated concave corner.
    pub fn build_concave_corner(cubics: Vec<Cubic>) -> Result<Feature> {
        Self::validated(Feature::Corner {
            cubics,
            convex: false,
        })
    }

    fn validated(feature: Feature) -> Result<Feature> {
        let cubics = feature.cubics();
        if cubics.is_empty() {
            return Err(ShapeError::InvalidArgument(
                "features need at least one cubic",
            ));
        }
        for w in cubics.windows(2) {
            let (a, b) = (w[0].anchor1(), w[1].anchor0());
            if (a.x - b.x).abs() > DISTANCE_EPSILON || (a.y - b.y).abs() > DISTANCE_EPSILON {
                return Err(ShapeError::NotContiguous);
            }
        }
        Ok(feature)
    }

    pub fn cubics(&self) -> &[Cubic] {
        match self {
            Feature::Edge { cubics } | Feature::Corner { cubics, .. } => cubics,
        }
    }

    pub fn is_edge(&self) -> bool {
        matches!(self, Feature::Edge { .. })
    }

    /// Edges are ignorable for morph matching.
    pub fn is_ignorable_feature(&self) -> bool {
        self.is_edge()
    }

    pub fn is_convex_corner(&self) -> bool {
        matches!(self, Feature::Corner { convex: true, .. })
    }

    pub fn is_concave_corner(&self) -> bool {
        matches!(self, Feature::Corner { convex: false, .. })
    }

    /// Applies `f` to every point.
    pub fn transformed(&self, f: &dyn Fn(f32, f32) -> (f32, f32)) -> Feature {
        let map = |cubics: &[Cubic]| cubics.iter().map(|c| c.transformed(f)).collect::<Vec<_>>();
        match self {
            Feature::Edge { cubics } => Feature::Edge {
                cubics: map(cubics),
            },
            Feature::Corner { cubics, convex } => Feature::Corner {
                cubics: map(cubics),
                convex: *convex,
            },
        }
    }

    /// The same feature traversed backwards (a corner flips convexity).
    pub fn reversed(&self) -> Feature {
        let rev = |cubics: &[Cubic]| cubics.iter().rev().map(Cubic::reverse).collect::<Vec<_>>();
        match self {
            Feature::Edge { cubics } => Feature::Edge {
                cubics: rev(cubics),
            },
            Feature::Corner { cubics, convex } => Feature::Corner {
                cubics: rev(cubics),
                convex: !*convex,
            },
        }
    }
}

/// Detects edge/corner features in a closed list of cubics (`detectFeatures`).
pub fn detect_features(cubics: &[Cubic]) -> Vec<Feature> {
    let mut out = Vec::new();
    let Some(&first) = cubics.first() else {
        return out;
    };
    let mut current = first;
    let n = cubics.len();
    for i in 0..n {
        let next = cubics[(i + 1) % n];
        if i < n - 1 && aligns_ish_with(&current, &next) {
            current = extend(&current, &next);
            continue;
        }
        out.push(as_feature(&current, &next));
        if !smoothes_into_ish(&current, &next) {
            let end = current.anchor1();
            out.push(as_feature(&Cubic::empty(end.x, end.y), &next));
        }
        current = next;
    }
    out
}

fn as_feature(c: &Cubic, next: &Cubic) -> Feature {
    if straight_ish(c) {
        Feature::Edge { cubics: vec![*c] }
    } else {
        Feature::Corner {
            cubics: vec![*c],
            convex: c.convex_to(next),
        }
    }
}

fn straight_ish(c: &Cubic) -> bool {
    !c.zero_length()
        && collinear_ish(
            c.anchor0(),
            c.anchor1(),
            c.control0(),
            RELAXED_DISTANCE_EPSILON,
        )
        && collinear_ish(
            c.anchor0(),
            c.anchor1(),
            c.control1(),
            RELAXED_DISTANCE_EPSILON,
        )
}

fn smoothes_into_ish(c: &Cubic, next: &Cubic) -> bool {
    collinear_ish(
        c.control1(),
        next.control0(),
        c.anchor1(),
        RELAXED_DISTANCE_EPSILON,
    )
}

fn aligns_ish_with(c: &Cubic, next: &Cubic) -> bool {
    (straight_ish(c) && straight_ish(next) && smoothes_into_ish(c, next))
        || c.zero_length()
        || next.zero_length()
}

fn extend(a: &Cubic, b: &Cubic) -> Cubic {
    if a.zero_length() {
        Cubic::new(
            a.points[0],
            a.points[1],
            b.points[2],
            b.points[3],
            b.points[4],
            b.points[5],
            b.points[6],
            b.points[7],
        )
    } else {
        Cubic::new(
            a.points[0],
            a.points[1],
            a.points[2],
            a.points[3],
            a.points[4],
            a.points[5],
            b.points[6],
            b.points[7],
        )
    }
}
