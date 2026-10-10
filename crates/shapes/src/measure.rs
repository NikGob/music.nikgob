//! Port of `PolygonMeasure.kt` with the `LengthMeasurer` (3-segment arc length estimate).

use crate::cubic::Cubic;
use crate::error::{Result, ShapeError};
use crate::feature::Feature;
use crate::mapping::ProgressableFeature;
use crate::point::{DISTANCE_EPSILON, Point, positive_modulo};
use crate::polygon::RoundedPolygon;

const SEGMENTS: usize = 3;

/// Walks the curve in `SEGMENTS` chords; returns (t reached, measure) for `threshold`.
fn closest_progress_to(cubic: &Cubic, threshold: f32) -> (f32, f32) {
    let mut total = 0.0f32;
    let mut remainder = threshold;
    let mut prev: Point = cubic.anchor0();
    for i in 1..=SEGMENTS {
        let progress = i as f32 / SEGMENTS as f32;
        let point = cubic.point_on_curve(progress);
        let segment = (point - prev).distance();
        if segment >= remainder {
            return (
                progress - (1.0 - remainder / segment) / SEGMENTS as f32,
                threshold,
            );
        }
        remainder -= segment;
        total += segment;
        prev = point;
    }
    (1.0, total)
}

fn measure_cubic(c: &Cubic) -> f32 {
    closest_progress_to(c, f32::INFINITY).1
}

fn find_cubic_cut_point(c: &Cubic, m: f32) -> f32 {
    closest_progress_to(c, m).0
}

/// A cubic with its range along the polygon's normalized outline.
#[derive(Debug, Clone)]
pub(crate) struct MeasuredCubic {
    pub cubic: Cubic,
    pub start_outline_progress: f32,
    pub end_outline_progress: f32,
    measured_size: f32,
}

impl MeasuredCubic {
    fn new(cubic: Cubic, start: f32, end: f32) -> Result<Self> {
        if end < start {
            return Err(ShapeError::Mapping(
                "end progress must not be below start progress",
            ));
        }
        Ok(Self {
            cubic,
            start_outline_progress: start,
            end_outline_progress: end,
            measured_size: measure_cubic(&cubic),
        })
    }

    pub fn cut_at_progress(
        &self,
        cut_outline_progress: f32,
    ) -> Result<(MeasuredCubic, MeasuredCubic)> {
        let bounded = cut_outline_progress
            .max(self.start_outline_progress)
            .min(self.end_outline_progress);
        let outline_progress_size = self.end_outline_progress - self.start_outline_progress;
        let progress_from_start = bounded - self.start_outline_progress;
        let relative_progress = progress_from_start / outline_progress_size;
        let t = find_cubic_cut_point(&self.cubic, relative_progress * self.measured_size);
        if !(0.0..=1.0).contains(&t) {
            return Err(ShapeError::Mapping(
                "cubic cut point is expected to be between 0 and 1",
            ));
        }
        let (c1, c2) = self.cubic.split(t);
        Ok((
            MeasuredCubic::new(c1, self.start_outline_progress, bounded)?,
            MeasuredCubic::new(c2, bounded, self.end_outline_progress)?,
        ))
    }
}

/// A polygon's cubics annotated with outline progress, plus its corner features.
#[derive(Debug, Clone)]
pub(crate) struct MeasuredPolygon {
    pub cubics: Vec<MeasuredCubic>,
    pub features: Vec<ProgressableFeature>,
}

impl MeasuredPolygon {
    fn new(
        features: Vec<ProgressableFeature>,
        cubics: &[Cubic],
        outline_progress: &[f32],
    ) -> Result<Self> {
        if outline_progress.len() != cubics.len() + 1 {
            return Err(ShapeError::Mapping(
                "outline progress size must be cubics size + 1",
            ));
        }
        if outline_progress[0] != 0.0 || outline_progress[outline_progress.len() - 1] != 1.0 {
            return Err(ShapeError::Mapping(
                "outline progress must start at 0 and end at 1",
            ));
        }
        let mut measured: Vec<MeasuredCubic> = Vec::new();
        let mut start = 0.0f32;
        for (index, cubic) in cubics.iter().enumerate() {
            if outline_progress[index + 1] - outline_progress[index] > DISTANCE_EPSILON {
                measured.push(MeasuredCubic::new(
                    *cubic,
                    start,
                    outline_progress[index + 1],
                )?);
                start = outline_progress[index + 1];
            }
        }
        match measured.last_mut() {
            Some(last) => last.end_outline_progress = 1.0,
            None => return Err(ShapeError::Mapping("polygon has no measurable cubics")),
        }
        Ok(Self {
            cubics: measured,
            features,
        })
    }

    pub fn measure(polygon: &RoundedPolygon) -> Result<Self> {
        let mut cubics: Vec<Cubic> = Vec::new();
        let mut feature_to_cubic: Vec<(&Feature, usize)> = Vec::new();
        for feature in polygon.features() {
            for (cubic_index, cubic) in feature.cubics().iter().enumerate() {
                if matches!(feature, Feature::Corner { .. })
                    && cubic_index == feature.cubics().len() / 2
                {
                    feature_to_cubic.push((feature, cubics.len()));
                }
                cubics.push(*cubic);
            }
        }

        let mut measures = Vec::with_capacity(cubics.len() + 1);
        let mut acc = 0.0f32;
        measures.push(acc);
        for c in &cubics {
            let m = measure_cubic(c);
            if m < 0.0 {
                return Err(ShapeError::Mapping("measured cubic must be non-negative"));
            }
            acc += m;
            measures.push(acc);
        }
        let total = acc;
        let outline_progress: Vec<f32> = measures.iter().map(|m| m / total).collect();

        let features = feature_to_cubic
            .iter()
            .map(|(feature, ix)| ProgressableFeature {
                progress: positive_modulo(
                    (outline_progress[*ix] + outline_progress[ix + 1]) / 2.0,
                    1.0,
                ),
                feature: (*feature).clone(),
            })
            .collect();

        Self::new(features, &cubics, &outline_progress)
    }

    /// Rotates the outline so it starts at `cutting_point`, splitting the cubic there.
    pub fn cut_and_shift(&self, cutting_point: f32) -> Result<MeasuredPolygon> {
        if !(0.0..=1.0).contains(&cutting_point) {
            return Err(ShapeError::Mapping(
                "cutting point is expected to be between 0 and 1",
            ));
        }
        if cutting_point < DISTANCE_EPSILON {
            return Ok(self.clone());
        }
        let n = self.cubics.len();
        let target_index = self
            .cubics
            .iter()
            .position(|c| {
                (c.start_outline_progress..=c.end_outline_progress).contains(&cutting_point)
            })
            .ok_or(ShapeError::Mapping("cutting point not within any cubic"))?;
        let target = &self.cubics[target_index];
        let (b1, b2) = target.cut_at_progress(cutting_point)?;

        let mut ret_cubics = Vec::with_capacity(n + 1);
        ret_cubics.push(b2.cubic);
        for i in 1..n {
            ret_cubics.push(self.cubics[(i + target_index) % n].cubic);
        }
        ret_cubics.push(b1.cubic);

        let mut ret_progress = Vec::with_capacity(n + 2);
        for index in 0..n + 2 {
            ret_progress.push(if index == 0 {
                0.0
            } else if index == n + 1 {
                1.0
            } else {
                let cubic_index = (target_index + index - 1) % n;
                positive_modulo(
                    self.cubics[cubic_index].end_outline_progress - cutting_point,
                    1.0,
                )
            });
        }

        let new_features = self
            .features
            .iter()
            .map(|f| ProgressableFeature {
                progress: positive_modulo(f.progress - cutting_point, 1.0),
                feature: f.feature.clone(),
            })
            .collect();

        Self::new(new_features, &ret_cubics, &ret_progress)
    }
}
