use egui::{Color32, Rect, pos2};

use crate::paint::{FillOptions, fill_mesh, shape_path};
use crate::triangulate::{signed_area2, triangulate};
use crate::{
    CornerRounding, Cubic, MaterialShape, Morph, PillStarOptions, Point, RoundedPolygon,
    StarOptions, detect_features,
};

const EPS: f32 = 1e-4;

fn dist(a: Point, b: Point) -> f32 {
    (a - b).distance()
}

/// Smallest distance from `p` to a densely sampled outline.
fn dist_to_outline(p: Point, cubics: &[Cubic]) -> f32 {
    let mut best = f32::MAX;
    for c in cubics {
        for i in 0..=64 {
            best = best.min(dist(p, c.point_on_curve(i as f32 / 64.0)));
        }
    }
    best
}

fn assert_same_outline(a: &[Cubic], b: &[Cubic], tol: f32) {
    for c in a {
        for i in 0..=8 {
            let p = c.point_on_curve(i as f32 / 8.0);
            let d = dist_to_outline(p, b);
            assert!(d < tol, "point {p:?} is {d} away from the other outline");
        }
    }
}

fn is_contiguous(cubics: &[Cubic]) -> bool {
    (0..cubics.len()).all(|i| {
        let prev = &cubics[(i + cubics.len() - 1) % cubics.len()];
        dist(prev.anchor1(), cubics[i].anchor0()) <= 1e-4
    })
}

#[test]
fn unrounded_square_has_sharp_corners() {
    let p =
        RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::UNROUNDED, None, Point::ZERO).unwrap();
    assert!(is_contiguous(p.cubics()));
    let b = p.calculate_bounds(false);
    for (got, want) in b.iter().zip([-1.0, -1.0, 1.0, 1.0]) {
        assert!((got - want).abs() < EPS, "{b:?}");
    }
    // The corner (1, 1) is on the outline.
    assert!(dist_to_outline(Point::new(1.0, 1.0), p.cubics()) < 1e-3);
}

#[test]
fn rounding_cuts_corners_and_smoothing_extends_flanks() {
    let rounded =
        RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::radius(0.5), None, Point::ZERO)
            .unwrap();
    assert!(is_contiguous(rounded.cubics()));
    // The sharp corner is no longer on the outline; the arc passes at the expected distance
    // sqrt(2)*(1 - r) + r from the center along the diagonal.
    let corner = Point::new(1.0, 1.0);
    let d = dist_to_outline(corner, rounded.cubics());
    let expected = (2.0f32.sqrt() - 1.0) * 0.5;
    assert!(
        (d - expected).abs() < 0.01,
        "corner distance {d}, expected ~{expected}"
    );
    // Straight sides are shortened: the outline still touches x = 1 at y = 0.
    assert!(dist_to_outline(Point::new(1.0, 0.0), rounded.cubics()) < 1e-3);

    // Smoothing keeps the first flank anchors farther from the corner than the arc cut.
    let smooth =
        RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::new(0.3, 1.0), None, Point::ZERO)
            .unwrap();
    let plain = RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::radius(0.3), None, Point::ZERO)
        .unwrap();
    assert!(is_contiguous(smooth.cubics()));
    assert!(smooth.cubics().len() >= plain.cubics().len());
}

#[test]
fn rounding_is_clamped_by_short_sides() {
    // Radius far larger than the shape must still produce a valid, contiguous outline.
    let p = RoundedPolygon::rectangle(1.0, 1.0, CornerRounding::new(10.0, 1.0), None, Point::ZERO)
        .unwrap();
    assert!(is_contiguous(p.cubics()));
    let b = p.calculate_bounds(true);
    assert!(b[0] >= -0.5 - EPS && b[2] <= 0.5 + EPS);
}

#[test]
fn circle_is_round() {
    let c = RoundedPolygon::circle(8, 1.0, Point::ZERO).unwrap();
    for cubic in c.cubics() {
        for i in 0..=16 {
            let r = cubic.point_on_curve(i as f32 / 16.0).distance();
            assert!((r - 1.0).abs() < 0.01, "radius {r}");
        }
    }
}

#[test]
fn invalid_arguments_are_errors() {
    assert!(RoundedPolygon::circle(2, 1.0, Point::ZERO).is_err());
    assert!(
        RoundedPolygon::from_vertices(&[0.0, 0.0, 1.0, 1.0], CornerRounding::UNROUNDED, None, None)
            .is_err()
    );
    assert!(
        RoundedPolygon::star(&StarOptions {
            inner_radius: 2.0,
            ..StarOptions::default()
        })
        .is_err()
    );
    assert!(RoundedPolygon::pill(0.0, 1.0, 0.0, Point::ZERO).is_err());
}

#[test]
fn star_pill_and_pill_star_build() {
    let star = RoundedPolygon::star(&StarOptions {
        num_vertices_per_radius: 6,
        rounding: CornerRounding::radius(0.1),
        inner_rounding: Some(CornerRounding::radius(0.05)),
        ..StarOptions::default()
    })
    .unwrap();
    assert!(is_contiguous(star.cubics()));
    let pill = RoundedPolygon::pill(2.0, 1.0, 0.0, Point::ZERO).unwrap();
    let b = pill.calculate_bounds(false);
    assert!((b[2] - b[0] - 2.0).abs() < 1e-3 && (b[3] - b[1] - 1.0).abs() < 1e-3);
    let ps = RoundedPolygon::pill_star(&PillStarOptions {
        rounding: CornerRounding::radius(0.05),
        ..PillStarOptions::default()
    })
    .unwrap();
    assert!(is_contiguous(ps.cubics()));
}

#[test]
fn detect_features_roundtrip() {
    let square =
        RoundedPolygon::rectangle(2.0, 2.0, CornerRounding::radius(0.3), None, Point::ZERO)
            .unwrap();
    let features = detect_features(square.cubics());
    assert!(features.iter().any(|f| f.is_convex_corner()));
    let rebuilt = RoundedPolygon::from_cubics(square.cubics()).unwrap();
    assert_same_outline(square.cubics(), rebuilt.cubics(), 1e-3);
}

#[test]
fn every_material_shape_is_normalized() {
    for shape in MaterialShape::ALL {
        let p = shape.polygon();
        assert!(is_contiguous(p.cubics()), "{} not contiguous", shape.name());
        for approximate in [true, false] {
            let b = p.calculate_bounds(approximate);
            for v in b {
                assert!(
                    (-EPS..=1.0 + EPS).contains(&v),
                    "{} bounds {b:?} (approximate={approximate})",
                    shape.name()
                );
            }
        }
        // Aspect-preserving normalization: the larger side fills the unit square.
        let b = p.calculate_bounds(true);
        assert!(
            ((b[2] - b[0]).max(b[3] - b[1]) - 1.0).abs() < 1e-3,
            "{} {b:?}",
            shape.name()
        );
    }
    assert_eq!(MaterialShape::ALL.len(), 35);
    for (i, s) in MaterialShape::ALL.iter().enumerate() {
        assert_eq!(s.index(), i);
    }
}

#[test]
fn material_polygon_is_cached() {
    let a = MaterialShape::Heart.polygon() as *const RoundedPolygon;
    let b = MaterialShape::Heart.polygon() as *const RoundedPolygon;
    assert_eq!(a, b);
}

#[test]
fn spot_check_known_shapes() {
    // Triangle points up (apex at the top, y down), circle is centered.
    let tri = MaterialShape::Triangle.polygon();
    let top = tri
        .cubics()
        .iter()
        .map(|c| c.anchor0().y)
        .fold(f32::MAX, f32::min);
    let bottom = tri
        .cubics()
        .iter()
        .map(|c| c.anchor0().y)
        .fold(f32::MIN, f32::max);
    let apex_x = tri
        .cubics()
        .iter()
        .min_by(|a, b| a.anchor0().y.total_cmp(&b.anchor0().y))
        .map(|c| c.anchor0().x)
        .unwrap();
    assert!(top < 0.2 && bottom > 0.5);
    assert!((apex_x - 0.5).abs() < 0.05);
    let circle = MaterialShape::Circle.polygon();
    assert!((circle.center().x - 0.5).abs() < 0.01 && (circle.center().y - 0.5).abs() < 0.01);
}

#[test]
fn cubic_split_is_continuous() {
    let c = Cubic::new(0.0, 0.0, 1.0, 2.0, 3.0, -1.0, 4.0, 1.0);
    for t in [0.1f32, 0.37, 0.5, 0.9] {
        let (a, b) = c.split(t);
        let mid = c.point_on_curve(t);
        assert!(dist(a.anchor1(), mid) < 1e-5);
        assert!(dist(b.anchor0(), mid) < 1e-5);
        assert!(dist(a.anchor0(), c.anchor0()) < 1e-6);
        assert!(dist(b.anchor1(), c.anchor1()) < 1e-6);
        for i in 0..=10 {
            let s = i as f32 / 10.0;
            assert!(dist(a.point_on_curve(s), c.point_on_curve(s * t)) < 1e-4);
            assert!(dist(b.point_on_curve(s), c.point_on_curve(t + s * (1.0 - t))) < 1e-4);
        }
    }
}

#[test]
fn cubic_reverse_and_bounds() {
    let c = Cubic::new(0.0, 0.0, 0.0, 2.0, 2.0, 2.0, 2.0, 0.0);
    let r = c.reverse();
    for i in 0..=10 {
        let t = i as f32 / 10.0;
        assert!(dist(c.point_on_curve(t), r.point_on_curve(1.0 - t)) < 1e-5);
    }
    let tight = c.calculate_bounds(false);
    let loose = c.calculate_bounds(true);
    assert!((tight[3] - 1.5).abs() < 1e-4, "{tight:?}");
    assert!((loose[3] - 2.0).abs() < 1e-6);
}

#[test]
fn morph_endpoints_match_polygons() {
    let pairs = [
        (MaterialShape::Circle, MaterialShape::Square),
        (MaterialShape::Heart, MaterialShape::Cookie9Sided),
        (MaterialShape::Triangle, MaterialShape::Pill),
        (MaterialShape::Burst, MaterialShape::Flower),
    ];
    for (a, b) in pairs {
        let (pa, pb) = (a.polygon(), b.polygon());
        let morph = Morph::new(pa, pb);
        let start = morph.as_cubics(0.0);
        let end = morph.as_cubics(1.0);
        assert!(is_contiguous(&start) && is_contiguous(&end));
        assert_same_outline(&start, pa.cubics(), 0.01);
        assert_same_outline(pa.cubics(), &start, 0.01);
        assert_same_outline(&end, pb.cubics(), 0.01);
        assert_same_outline(pb.cubics(), &end, 0.01);
        // The midpoint is a valid closed outline too.
        assert!(is_contiguous(&morph.as_cubics(0.5)));
    }
}

#[test]
fn morph_between_all_shapes_succeeds() {
    for a in MaterialShape::ALL {
        for b in MaterialShape::ALL {
            let morph = Morph::try_new(a.polygon(), b.polygon())
                .unwrap_or_else(|e| panic!("{} -> {}: {e}", a.name(), b.name()));
            assert!(!morph.morph_match().is_empty());
        }
    }
}

#[test]
fn identical_morph_is_identity() {
    let p = MaterialShape::Sunny.polygon();
    let m = Morph::new(p, p);
    assert_same_outline(&m.as_cubics(0.5), p.cubics(), 1e-3);
}

fn unit_rect(size: f32) -> Rect {
    Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(size, size))
}

fn polygon_area(points: &[egui::Pos2]) -> f64 {
    let raw: Vec<[f32; 2]> = points.iter().map(|p| [p.x, p.y]).collect();
    signed_area2(&raw).abs() / 2.0
}

#[test]
fn every_shape_triangulates_to_polygon_area() {
    let opts = FillOptions {
        tolerance: 0.05,
        feather: 0.0,
    };
    for shape in MaterialShape::ALL {
        let rect = unit_rect(200.0);
        let cubics = shape.polygon().cubics();
        let path = shape_path(cubics, rect, 0.0, opts.tolerance);
        let want = polygon_area(&path);
        let mesh = fill_mesh(cubics, rect, 0.0, Color32::WHITE, &opts);
        assert!(mesh.is_valid(), "{} mesh invalid", shape.name());
        let mut got = 0.0f64;
        let mut signed = 0.0f64;
        for t in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize].pos);
            let cr = f64::from((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)) / 2.0;
            got += cr.abs();
            signed += cr;
        }
        assert!(want > 100.0, "{} has no area", shape.name());
        assert!(
            (got - want).abs() / want < 1e-3,
            "{}: triangles {got}, polygon {want}",
            shape.name()
        );
        // All triangles share the polygon's winding (a simple polygon has no flipped ears).
        assert!(
            (signed.abs() - got).abs() / want < 1e-3,
            "{} has flipped triangles",
            shape.name()
        );
    }
}

#[test]
fn rotated_and_feathered_meshes_are_valid() {
    for shape in [
        MaterialShape::Burst,
        MaterialShape::Heart,
        MaterialShape::Ghostish,
    ] {
        let mesh = fill_mesh(
            shape.polygon().cubics(),
            Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(120.0, 80.0)),
            0.7,
            Color32::from_rgb(10, 200, 30),
            &FillOptions::for_ppp(2.0),
        );
        assert!(mesh.is_valid());
        assert!(!mesh.is_empty());
    }
}

#[test]
fn rotation_keeps_center_fixed_and_rotates_path() {
    let rect = Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(100.0, 100.0));
    let cubics = MaterialShape::Square.polygon().cubics();
    let a = shape_path(cubics, rect, 0.0, 0.1);
    let b = shape_path(cubics, rect, std::f32::consts::FRAC_PI_2, 0.1);
    // A square is invariant under a quarter turn (as a set of vertices).
    assert_eq!(a.len(), b.len());
    for p in &b {
        assert!(a.iter().any(|q| q.distance(*p) < 0.5), "{p:?}");
    }
    let mean = |v: &[egui::Pos2]| {
        let n = v.len() as f32;
        pos2(
            v.iter().map(|p| p.x).sum::<f32>() / n,
            v.iter().map(|p| p.y).sum::<f32>() / n,
        )
    };
    assert!(mean(&b).distance(pos2(50.0, 50.0)) < 1.0);
}

#[test]
fn ear_clipping_handles_concave_polygons() {
    // L-shape (area 3), both windings.
    let l = [
        [0.0, 0.0],
        [2.0, 0.0],
        [2.0, 1.0],
        [1.0, 1.0],
        [1.0, 2.0],
        [0.0, 2.0],
    ];
    for pts in [l.to_vec(), l.iter().rev().copied().collect::<Vec<_>>()] {
        let tris = triangulate(&pts);
        assert_eq!(tris.len(), 4);
        let area: f64 = tris
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|i| pts[i as usize]);
                (f64::from((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0)
                    .abs()
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-9);
    }
    assert!(triangulate(&[[0.0, 0.0], [1.0, 1.0]]).is_empty());
}
