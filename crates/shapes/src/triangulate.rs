//! Ear-clipping triangulation for simple (possibly concave) polygons.

/// A 2D point in `f64`, used internally for robust predicates.
type P = [f64; 2];

#[inline]
fn cross(a: P, b: P, c: P) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

#[inline]
fn same(a: P, b: P) -> bool {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    dx * dx + dy * dy < 1e-18
}

/// Twice the signed area (positive for counter-clockwise in a y-up frame).
pub fn signed_area2(points: &[[f32; 2]]) -> f64 {
    let n = points.len();
    let mut s = 0.0f64;
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        s += f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]);
    }
    s
}

/// Triangulates the simple polygon `points` (either winding), returning triangles as
/// index triples into `points`.
///
/// Consecutive duplicate points are skipped. If the input is not simple, the clipper falls
/// back to cutting the most convex vertex, so it always terminates with `n - 2` triangles
/// (but self-intersecting input may overlap).
pub fn triangulate(points: &[[f32; 2]]) -> Vec<[u32; 3]> {
    let pts: Vec<P> = points
        .iter()
        .map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect();

    // Drop consecutive duplicates (including closing duplicate).
    let mut idx: Vec<usize> = Vec::with_capacity(pts.len());
    for i in 0..pts.len() {
        if idx.last().is_none_or(|&l| !same(pts[l], pts[i])) {
            idx.push(i);
        }
    }
    while idx.len() > 1 && same(pts[idx[0]], pts[idx[idx.len() - 1]]) {
        idx.pop();
    }
    if idx.len() < 3 {
        return Vec::new();
    }

    // Make counter-clockwise (positive area) for the convexity test.
    let area: f64 = (0..idx.len())
        .map(|i| {
            let a = pts[idx[i]];
            let b = pts[idx[(i + 1) % idx.len()]];
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area < 0.0 {
        idx.reverse();
    }

    let mut tris: Vec<[u32; 3]> = Vec::with_capacity(idx.len().saturating_sub(2));
    let mut start = 0usize;
    while idx.len() > 3 {
        let n = idx.len();
        let mut found = None;
        for k in 0..n {
            let i = (start + k) % n;
            if is_ear(&pts, &idx, i) {
                found = Some(i);
                break;
            }
        }
        let i = found.unwrap_or_else(|| most_convex(&pts, &idx));
        let prev = (i + n - 1) % n;
        let next = (i + 1) % n;
        tris.push([idx[prev] as u32, idx[i] as u32, idx[next] as u32]);
        idx.remove(i);
        start = if prev > i { prev - 1 } else { prev };
    }
    tris.push([idx[0] as u32, idx[1] as u32, idx[2] as u32]);
    tris
}

fn is_ear(pts: &[P], idx: &[usize], i: usize) -> bool {
    let n = idx.len();
    let (ia, ib, ic) = (idx[(i + n - 1) % n], idx[i], idx[(i + 1) % n]);
    let (a, b, c) = (pts[ia], pts[ib], pts[ic]);
    if cross(a, b, c) <= 1e-14 {
        return false;
    }
    for &j in idx {
        if j == ia || j == ib || j == ic {
            continue;
        }
        let p = pts[j];
        if same(p, a) || same(p, b) || same(p, c) {
            continue;
        }
        if cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0 {
            return false;
        }
    }
    true
}

fn most_convex(pts: &[P], idx: &[usize]) -> usize {
    let n = idx.len();
    let mut best = 0;
    let mut best_cross = f64::NEG_INFINITY;
    for i in 0..n {
        let c = cross(
            pts[idx[(i + n - 1) % n]],
            pts[idx[i]],
            pts[idx[(i + 1) % n]],
        );
        if c > best_cross {
            best_cross = c;
            best = i;
        }
    }
    best
}
