//! Ports of `FloatMapping.kt` (progress `DoubleMapper`) and `FeatureMapping.kt`.

use crate::error::{Result, ShapeError};
use crate::feature::Feature;
use crate::point::{DISTANCE_EPSILON, Point, positive_modulo};

/// A feature of a measured polygon at a position along its outline (0..1).
#[derive(Debug, Clone)]
pub(crate) struct ProgressableFeature {
    pub progress: f32,
    pub feature: Feature,
}

pub(crate) fn progress_distance(p1: f32, p2: f32) -> f32 {
    let d = (p1 - p2).abs();
    d.min(1.0 - d)
}

pub(crate) fn progress_in_range(progress: f32, from: f32, to: f32) -> bool {
    if to >= from {
        (from..=to).contains(&progress)
    } else {
        progress >= from || progress <= to
    }
}

fn linear_map(x_values: &[f32], y_values: &[f32], x: f32) -> Result<f32> {
    if !(0.0..=1.0).contains(&x) {
        return Err(ShapeError::Mapping("invalid progress"));
    }
    let n = x_values.len();
    let start = (0..n)
        .find(|&i| progress_in_range(x, x_values[i], x_values[(i + 1) % n]))
        .ok_or(ShapeError::Mapping("progress not found in mapping"))?;
    let end = (start + 1) % n;
    let size_x = positive_modulo(x_values[end] - x_values[start], 1.0);
    let size_y = positive_modulo(y_values[end] - y_values[start], 1.0);
    let position = if size_x < 0.001 {
        0.5
    } else {
        positive_modulo(x - x_values[start], 1.0) / size_x
    };
    Ok(positive_modulo(y_values[start] + size_y * position, 1.0))
}

fn validate_progress(p: &[f32]) -> Result<()> {
    let Some(&last) = p.last() else {
        return Err(ShapeError::Mapping("empty progress list"));
    };
    let mut prev = last;
    let mut wraps = 0;
    for &curr in p {
        if !(0.0..1.0).contains(&curr) {
            return Err(ShapeError::Mapping("progress outside of range"));
        }
        if progress_distance(curr, prev) <= DISTANCE_EPSILON {
            return Err(ShapeError::Mapping("progress repeats a value"));
        }
        if curr < prev {
            wraps += 1;
            if wraps > 1 {
                return Err(ShapeError::Mapping("progress wraps more than once"));
            }
        }
        prev = curr;
    }
    Ok(())
}

/// Maps progress values of one outline to another and back, piecewise linearly.
#[derive(Debug, Clone)]
pub(crate) struct DoubleMapper {
    source: Vec<f32>,
    target: Vec<f32>,
}

impl DoubleMapper {
    pub fn new(mappings: &[(f32, f32)]) -> Result<Self> {
        let source: Vec<f32> = mappings.iter().map(|m| m.0).collect();
        let target: Vec<f32> = mappings.iter().map(|m| m.1).collect();
        validate_progress(&source)?;
        validate_progress(&target)?;
        Ok(Self { source, target })
    }

    pub fn map(&self, x: f32) -> Result<f32> {
        linear_map(&self.source, &self.target, x)
    }

    pub fn map_back(&self, x: f32) -> Result<f32> {
        linear_map(&self.target, &self.source, x)
    }
}

/// Builds the progress mapping between the corner features of two polygons.
pub(crate) fn feature_mapper(
    features1: &[ProgressableFeature],
    features2: &[ProgressableFeature],
) -> Result<DoubleMapper> {
    let filtered1: Vec<&ProgressableFeature> = features1
        .iter()
        .filter(|f| matches!(f.feature, Feature::Corner { .. }))
        .collect();
    let filtered2: Vec<&ProgressableFeature> = features2
        .iter()
        .filter(|f| matches!(f.feature, Feature::Corner { .. }))
        .collect();
    let mapping = do_mapping(&filtered1, &filtered2)?;
    DoubleMapper::new(&mapping)
}

struct DistanceVertex {
    distance: f32,
    i1: usize,
    i2: usize,
}

fn do_mapping(
    features1: &[&ProgressableFeature],
    features2: &[&ProgressableFeature],
) -> Result<Vec<(f32, f32)>> {
    let mut vertices = Vec::new();
    for (i1, f1) in features1.iter().enumerate() {
        for (i2, f2) in features2.iter().enumerate() {
            let d = feature_dist_squared(&f1.feature, &f2.feature);
            if d != f32::MAX {
                vertices.push(DistanceVertex {
                    distance: d,
                    i1,
                    i2,
                });
            }
        }
    }
    vertices.sort_by(|a, b| a.distance.total_cmp(&b.distance));

    if vertices.is_empty() {
        return Ok(vec![(0.0, 0.0), (0.5, 0.5)]);
    }
    if vertices.len() == 1 {
        let f1 = features1[vertices[0].i1].progress;
        let f2 = features2[vertices[0].i2].progress;
        return Ok(vec![(f1, f2), ((f1 + 0.5) % 1.0, (f2 + 0.5) % 1.0)]);
    }

    let mut helper = MappingHelper::default();
    for v in &vertices {
        helper.add_mapping(
            v.i1,
            features1[v.i1].progress,
            v.i2,
            features2[v.i2].progress,
        )?;
    }
    Ok(helper.mapping)
}

#[derive(Default)]
struct MappingHelper {
    mapping: Vec<(f32, f32)>,
    used1: Vec<usize>,
    used2: Vec<usize>,
}

impl MappingHelper {
    fn add_mapping(&mut self, i1: usize, p1: f32, i2: usize, p2: f32) -> Result<()> {
        if self.used1.contains(&i1) || self.used2.contains(&i2) {
            return Ok(());
        }
        let insertion = match self.mapping.binary_search_by(|m| m.0.total_cmp(&p1)) {
            Ok(_) => {
                return Err(ShapeError::Mapping(
                    "there can't be two features with the same progress",
                ));
            }
            Err(i) => i,
        };
        let n = self.mapping.len();
        if n >= 1 {
            let (before1, before2) = self.mapping[(insertion + n - 1) % n];
            let (after1, after2) = self.mapping[insertion % n];
            if progress_distance(p1, before1) < DISTANCE_EPSILON
                || progress_distance(p1, after1) < DISTANCE_EPSILON
                || progress_distance(p2, before2) < DISTANCE_EPSILON
                || progress_distance(p2, after2) < DISTANCE_EPSILON
            {
                return Ok(());
            }
            if n > 1 && !progress_in_range(p2, before2, after2) {
                return Ok(());
            }
        }
        self.mapping.insert(insertion, (p1, p2));
        self.used1.push(i1);
        self.used2.push(i2);
        Ok(())
    }
}

fn feature_dist_squared(f1: &Feature, f2: &Feature) -> f32 {
    if let (Feature::Corner { convex: c1, .. }, Feature::Corner { convex: c2, .. }) = (f1, f2)
        && c1 != c2
    {
        return f32::MAX;
    }
    (representative_point(f1) - representative_point(f2)).distance_squared()
}

fn representative_point(f: &Feature) -> Point {
    let cubics = f.cubics();
    match (cubics.first(), cubics.last()) {
        (Some(a), Some(b)) => (a.anchor0() + b.anchor1()) / 2.0,
        _ => Point::ZERO,
    }
}
