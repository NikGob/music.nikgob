//! Port of `Morph.kt`: matches two polygons cubic-by-cubic and interpolates between them.

use crate::cubic::Cubic;
use crate::error::{Result, ShapeError};
use crate::mapping::feature_mapper;
use crate::measure::MeasuredPolygon;
use crate::point::{ANGLE_EPSILON, positive_modulo};
use crate::polygon::RoundedPolygon;

/// A morph between two [`RoundedPolygon`]s.
///
/// The polygons are split so that both outlines consist of the same number of cubics, aligned
/// at their matching corner features; [`Morph::as_cubics`] interpolates them.
#[derive(Debug, Clone)]
pub struct Morph {
    start_bounds: [f32; 4],
    end_bounds: [f32; 4],
    start_max_bounds: [f32; 4],
    end_max_bounds: [f32; 4],
    morph_match: Vec<(Cubic, Cubic)>,
}

impl Morph {
    /// Builds a morph.
    ///
    /// # Panics
    /// If the two polygons cannot be matched; use [`Morph::try_new`] to handle that case.
    pub fn new(start: &RoundedPolygon, end: &RoundedPolygon) -> Self {
        match Self::try_new(start, end) {
            Ok(m) => m,
            Err(e) => panic!("cannot morph polygons: {e}"),
        }
    }

    /// Fallible variant of [`Morph::new`].
    pub fn try_new(start: &RoundedPolygon, end: &RoundedPolygon) -> Result<Self> {
        Ok(Self {
            start_bounds: start.calculate_bounds(true),
            end_bounds: end.calculate_bounds(true),
            start_max_bounds: start.calculate_max_bounds(),
            end_max_bounds: end.calculate_max_bounds(),
            morph_match: match_polygons(start, end)?,
        })
    }

    /// The matched cubic pairs `(from start, from end)`.
    pub fn morph_match(&self) -> &[(Cubic, Cubic)] {
        &self.morph_match
    }

    /// Union of the (approximate) bounds of both polygons.
    pub fn bounds(&self) -> [f32; 4] {
        union(self.start_bounds, self.end_bounds)
    }

    /// Union of the max (rotation-safe) bounds of both polygons.
    pub fn max_bounds(&self) -> [f32; 4] {
        union(self.start_max_bounds, self.end_max_bounds)
    }

    /// The interpolated outline at `progress` (0 = start, 1 = end; values outside overshoot).
    pub fn as_cubics(&self, progress: f32) -> Vec<Cubic> {
        let mut out: Vec<Cubic> = Vec::with_capacity(self.morph_match.len());
        let mut first: Option<Cubic> = None;
        let mut last: Option<Cubic> = None;
        for (a, b) in &self.morph_match {
            let cubic = Cubic::interpolate(a, b, progress);
            if first.is_none() {
                first = Some(cubic);
            }
            if let Some(l) = last {
                out.push(l);
            }
            last = Some(cubic);
        }
        // Close the outline exactly on the first anchor.
        if let (Some(l), Some(f)) = (last, first) {
            out.push(Cubic::new(
                l.points[0],
                l.points[1],
                l.points[2],
                l.points[3],
                l.points[4],
                l.points[5],
                f.points[0],
                f.points[1],
            ));
        }
        out
    }
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

fn match_polygons(p1: &RoundedPolygon, p2: &RoundedPolygon) -> Result<Vec<(Cubic, Cubic)>> {
    let measured1 = MeasuredPolygon::measure(p1)?;
    let measured2 = MeasuredPolygon::measure(p2)?;
    let mapper = feature_mapper(&measured1.features, &measured2.features)?;
    let polygon2_cut_point = mapper.map(0.0)?;

    let bs1 = measured1;
    let bs2 = measured2.cut_and_shift(polygon2_cut_point)?;

    let mut ret = Vec::new();
    let mut i1 = 0usize;
    let mut i2 = 0usize;
    let mut b1 = bs1.cubics.get(i1).cloned();
    i1 += 1;
    let mut b2 = bs2.cubics.get(i2).cloned();
    i2 += 1;

    while let (Some(c1), Some(c2)) = (b1.as_ref(), b2.as_ref()) {
        let b1a = if i1 == bs1.cubics.len() {
            1.0
        } else {
            c1.end_outline_progress
        };
        let b2a = if i2 == bs2.cubics.len() {
            1.0
        } else {
            mapper.map_back(positive_modulo(
                c2.end_outline_progress + polygon2_cut_point,
                1.0,
            ))?
        };
        let minb = b1a.min(b2a);

        let (seg1, newb1) = if b1a > minb + ANGLE_EPSILON {
            let (a, b) = c1.cut_at_progress(minb)?;
            (a, Some(b))
        } else {
            let next = bs1.cubics.get(i1).cloned();
            i1 += 1;
            (c1.clone(), next)
        };
        let (seg2, newb2) = if b2a > minb + ANGLE_EPSILON {
            let (a, b) =
                c2.cut_at_progress(positive_modulo(mapper.map(minb)? - polygon2_cut_point, 1.0))?;
            (a, Some(b))
        } else {
            let next = bs2.cubics.get(i2).cloned();
            i2 += 1;
            (c2.clone(), next)
        };
        ret.push((seg1.cubic, seg2.cubic));
        b1 = newb1;
        b2 = newb2;
    }
    if b1.is_some() || b2.is_some() {
        return Err(ShapeError::Mapping(
            "expected both polygons' cubics to be fully matched",
        ));
    }
    Ok(ret)
}
