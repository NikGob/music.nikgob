//! Regions of taste in sound (recs-v2 §4.5): k-means over the embedded
//! bliss vectors of the tracks with long affinity, every artist free to
//! fall in several regions, small ones merged into their nearest, and a
//! soft membership of any vector (`softmax(−d² / τ)`, τ the median squared
//! distance of the members to their centre).
//!
//! **Taste profiles.** The user's model builds the regions within each
//! kind of song apart ([`Regions::by_lane`]: instrumental, each sung
//! language, unknown), so a profile is a sound of one kind — rap in
//! Russian, calm soundtracks, Japanese pop — and a big one never swallows a
//! small one. A profile's share is the share of the user's active choices
//! in it, not of the plays (see [`crate::model`]): what plays on in the
//! background for hours counts fully inside its profile but little in the
//! balance. Shelves draw from the profiles by their shares, each active
//! one at least once ([`spread`]). The seeds of a discovery pass come from
//! every profile ([`crate::Recs::discovery_seeds`]), a new track's fit is
//! its best membership ([`Regions::fit`]) and the appetite for new music is
//! kept per profile ([`crate::novelty`]).

use crate::analysis::Vector;
use crate::index::euclid;
use crate::songtype::SongLane;

/// A profile is active on a shelf with at least this share of the user's
/// active choices (a generic prior: one choice in twenty-five)…
pub const ACTIVE_SHARE: f32 = 0.04;
/// …and gets at least this many places of it.
pub const MIN_PLACES: usize = 1;

/// `n` of `items` (best first, each with its profile) spread over the
/// profiles by `shares` (the user's, summing to about 1): each active
/// profile ([`ACTIVE_SHARE`]) that has items gets [`MIN_PLACES`] at least,
/// the rest of the places by share, the best of each profile first; the
/// order is the items' own. Items of no profile fill what is left.
pub fn spread<T: Clone>(items: &[(T, Option<usize>)], shares: &[f32], n: usize) -> Vec<T> {
    let mut quota = vec![0usize; shares.len()];
    let has = |j: usize| items.iter().any(|(_, p)| *p == Some(j));
    let active: Vec<usize> = (0..shares.len()).filter(|&j| shares[j] >= ACTIVE_SHARE && has(j)).collect();
    let total: f32 = active.iter().map(|&j| shares[j]).sum::<f32>().max(1e-6);
    // By the largest remainders of the shares…
    let mut left = n;
    let mut rem: Vec<(f32, usize)> = Vec::new();
    for &j in &active {
        let exact = n as f32 * shares[j] / total;
        let whole = (exact.floor() as usize).min(left);
        quota[j] = whole;
        left -= whole;
        rem.push((exact - exact.floor(), j));
    }
    rem.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, j) in rem.into_iter().take(left) {
        quota[j] += 1;
    }
    // …each active profile at least its places, from the largest.
    for &j in &active {
        while quota[j] < MIN_PLACES.min(n) {
            let Some(big) = (0..quota.len()).filter(|&b| quota[b] > MIN_PLACES).max_by_key(|&b| (quota[b], std::cmp::Reverse(b))) else { break };
            quota[big] -= 1;
            quota[j] += 1;
        }
    }
    let mut taken = vec![false; items.len()];
    let mut used = vec![0usize; shares.len()];
    let mut count = 0;
    for (k, (_, p)) in items.iter().enumerate() {
        if count == n {
            break;
        }
        if let Some(j) = *p
            && j < quota.len()
            && used[j] < quota[j]
        {
            used[j] += 1;
            taken[k] = true;
            count += 1;
        }
    }
    // A profile short of items leaves its places to the best of the rest.
    for t in &mut taken {
        if count == n {
            break;
        }
        if !*t {
            *t = true;
            count += 1;
        }
    }
    items.iter().zip(&taken).filter(|(_, t)| **t).map(|((x, _), _)| x.clone()).collect()
}

/// `k = clamp(3 + n / PER_REGION, MIN_K, MAX_K)` over `n` tracks with
/// affinity…
const PER_REGION: usize = 150;
pub const MIN_K: usize = 4;
pub const MAX_K: usize = 10;
/// …and a region smaller than this joins its nearest.
pub const MIN_MEMBERS: usize = 15;
const ITERATIONS: usize = 25;

/// The regions of one build.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Regions {
    pub centres: Vec<Vector>,
    /// The kind of song of each region (built [`Regions::by_lane`]).
    pub lanes: Vec<Option<SongLane>>,
    /// The temperature of the membership.
    tau: f32,
}

fn sq(a: &Vector, b: &Vector) -> f32 {
    let d = euclid(a, b);
    d * d
}

fn nearest(centres: &[Vector], p: &Vector) -> usize {
    let mut best = (f32::INFINITY, 0);
    for (j, c) in centres.iter().enumerate() {
        let d = sq(c, p);
        if d < best.0 {
            best = (d, j);
        }
    }
    best.1
}

fn mean(points: &[&Vector], members: &[usize]) -> Option<Vector> {
    if members.is_empty() {
        return None;
    }
    let mut m = [0.0f32; crate::DIM];
    for &i in members {
        for (a, b) in m.iter_mut().zip(points[i]) {
            *a += b;
        }
    }
    let n = members.len() as f32;
    Some(m.map(|x| x / n))
}

impl Regions {
    /// The regions of `points` (embedded vectors of tracks with affinity):
    /// none below [`MIN_MEMBERS`] points. Deterministic: the first centre is
    /// the point nearest the mean, each next the farthest from those taken.
    pub fn build(points: &[&Vector]) -> Self {
        let n = points.len();
        if n < MIN_MEMBERS {
            return Self::default();
        }
        let k = (3 + n / PER_REGION).clamp(MIN_K, MAX_K).min(n / MIN_MEMBERS).max(1);
        let all: Vec<usize> = (0..n).collect();
        let centre = mean(points, &all).expect("points");
        let first = (0..n).min_by(|&a, &b| sq(points[a], &centre).total_cmp(&sq(points[b], &centre)).then(a.cmp(&b))).expect("points");
        let mut centres: Vec<Vector> = vec![*points[first]];
        while centres.len() < k {
            let far = (0..n)
                .max_by(|&a, &b| {
                    let da = centres.iter().map(|c| sq(c, points[a])).fold(f32::INFINITY, f32::min);
                    let db = centres.iter().map(|c| sq(c, points[b])).fold(f32::INFINITY, f32::min);
                    da.total_cmp(&db).then(b.cmp(&a))
                })
                .expect("points");
            centres.push(*points[far]);
        }
        let mut assign = vec![0usize; n];
        for _ in 0..ITERATIONS {
            let mut moved = false;
            for (i, p) in points.iter().enumerate() {
                let j = nearest(&centres, p);
                moved |= assign[i] != j;
                assign[i] = j;
            }
            let mut next = Vec::with_capacity(centres.len());
            for j in 0..centres.len() {
                let members: Vec<usize> = (0..n).filter(|&i| assign[i] == j).collect();
                next.push(mean(points, &members).unwrap_or(centres[j]));
            }
            centres = next;
            if !moved {
                break;
            }
        }
        // Small regions join their nearest, the smallest first.
        loop {
            let sizes: Vec<usize> = (0..centres.len()).map(|j| assign.iter().filter(|a| **a == j).count()).collect();
            let Some(small) = (0..centres.len()).filter(|&j| sizes[j] < MIN_MEMBERS).min_by_key(|&j| (sizes[j], j)) else { break };
            if centres.len() == 1 {
                break;
            }
            centres.remove(small);
            for (i, p) in points.iter().enumerate() {
                if assign[i] == small || assign[i] > small {
                    assign[i] = nearest(&centres, p);
                }
            }
            for j in 0..centres.len() {
                let members: Vec<usize> = (0..n).filter(|&i| assign[i] == j).collect();
                if let Some(m) = mean(points, &members) {
                    centres[j] = m;
                }
            }
        }
        let mut d: Vec<f32> = points.iter().map(|p| sq(&centres[nearest(&centres, p)], p)).collect();
        d.sort_by(f32::total_cmp);
        let tau = d.get(d.len() / 2).copied().filter(|t| *t > 1e-4).unwrap_or(1.0);
        let lanes = vec![None; centres.len()];
        Self { centres, lanes, tau }
    }

    /// The regions of each kind of song apart: `groups` are the points of
    /// each lane; a lane with too few points joins the others' leftovers,
    /// built together at the end (of no lane).
    pub fn by_lane(groups: &[(SongLane, Vec<&Vector>)]) -> Self {
        let mut out = Self::default();
        let mut left: Vec<&Vector> = Vec::new();
        let mut all: Vec<&Vector> = Vec::new();
        for (lane, pts) in groups {
            all.extend(pts.iter().copied());
            let r = if *lane == SongLane::Unknown { Self::default() } else { Self::build(pts) };
            if r.is_empty() {
                left.extend(pts.iter().copied());
                continue;
            }
            out.lanes.extend(std::iter::repeat_n(Some(*lane), r.len()));
            out.centres.extend(r.centres);
        }
        let rest = Self::build(&left);
        out.lanes.extend(std::iter::repeat_n(None, rest.len()));
        out.centres.extend(rest.centres);
        if out.is_empty() {
            return out;
        }
        let mut d: Vec<f32> = all.iter().map(|p| sq(&out.centres[nearest(&out.centres, p)], p)).collect();
        d.sort_by(f32::total_cmp);
        out.tau = d.get(d.len() / 2).copied().filter(|t| *t > 1e-4).unwrap_or(1.0);
        out
    }

    /// The region of a track of `lane` sounding `v`: the nearest of its
    /// lane's, else of all.
    pub fn of(&self, v: &Vector, lane: Option<SongLane>) -> Option<usize> {
        let own: Vec<usize> = (0..self.len()).filter(|&j| lane.is_some() && self.lanes[j] == lane).collect();
        let pool: Vec<usize> = if own.is_empty() { (0..self.len()).collect() } else { own };
        pool.into_iter().min_by(|&a, &b| sq(&self.centres[a], v).total_cmp(&sq(&self.centres[b], v)).then(a.cmp(&b)))
    }

    pub fn len(&self) -> usize {
        self.centres.len()
    }

    pub fn is_empty(&self) -> bool {
        self.centres.is_empty()
    }

    /// The soft membership of `v` in each region (summing to 1).
    pub fn membership(&self, v: &Vector) -> Vec<f32> {
        let logits: Vec<f32> = self.centres.iter().map(|c| -sq(c, v) / self.tau).collect();
        let top = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let e: Vec<f32> = logits.iter().map(|l| (l - top).exp()).collect();
        let z: f32 = e.iter().sum();
        e.into_iter().map(|x| x / z).collect()
    }

    /// The region `v` belongs to most, and how much.
    pub fn best(&self, v: &Vector) -> Option<(usize, f32)> {
        self.membership(v).into_iter().enumerate().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)))
    }

    /// How well `v` fits the taste: how close it is to the nearest centre,
    /// 1 at the centre, ½ at the typical distance of a member, falling to
    /// 0 far out.
    pub fn fit(&self, v: &Vector) -> f32 {
        if self.is_empty() {
            return 0.0;
        }
        let d = self.centres.iter().map(|c| sq(c, v)).fold(f32::INFINITY, f32::min);
        (-(d / self.tau) * std::f32::consts::LN_2).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;
    use crate::test_util::point;

    #[test]
    fn three_clouds_make_three_regions() {
        let mut rng = Rng::new(5);
        let mut pts: Vec<Vector> = Vec::new();
        for (cx, cy, n) in [(0.0, 0.0, 60), (10.0, 0.0, 40), (0.0, 10.0, 30), (30.0, 30.0, 5)] {
            for _ in 0..n {
                pts.push(point(cx + rng.f32(), cy + rng.f32()));
            }
        }
        let refs: Vec<&Vector> = pts.iter().collect();
        let r = Regions::build(&refs);
        assert!((3..=4).contains(&r.len()), "{}", r.len());
        let a = r.best(&point(0.5, 0.5)).unwrap().0;
        let b = r.best(&point(10.5, 0.5)).unwrap().0;
        assert_ne!(a, b);
        assert!(r.fit(&point(0.5, 0.5)) > 0.4);
        assert!(r.fit(&point(50.0, -50.0)) < 0.01);
        let m = r.membership(&point(0.5, 0.5));
        assert!((m.iter().sum::<f32>() - 1.0).abs() < 1e-4);
        assert!(Regions::build(&refs[..10]).is_empty(), "too few");
    }

    /// One sound of two kinds of song makes two regions; a lane of too few
    /// joins the rest.
    #[test]
    fn profiles_split_by_kind() {
        let mut rng = Rng::new(2);
        let a: Vec<Vector> = (0..40).map(|_| point(rng.f32(), rng.f32())).collect();
        let b: Vec<Vector> = (0..40).map(|_| point(rng.f32(), rng.f32())).collect();
        let c: Vec<Vector> = (0..5).map(|_| point(5.0 + rng.f32(), rng.f32())).collect();
        let r = Regions::by_lane(&[
            (SongLane::Russian, a.iter().collect()),
            (SongLane::Instrumental, b.iter().collect()),
            (SongLane::Japanese, c.iter().collect()),
        ]);
        assert!(r.lanes.contains(&Some(SongLane::Russian)) && r.lanes.contains(&Some(SongLane::Instrumental)), "{:?}", r.lanes);
        let ru = r.of(&point(0.5, 0.5), Some(SongLane::Russian)).unwrap();
        let ost = r.of(&point(0.5, 0.5), Some(SongLane::Instrumental)).unwrap();
        assert_ne!(ru, ost, "the same sound, two kinds");
        assert_eq!(r.lanes[ru], Some(SongLane::Russian));
    }

    /// A shelf from a big profile and a small one shows the small one at
    /// about its share, never none of it.
    #[test]
    fn a_shelf_spreads_over_the_profiles() {
        // 90 items of profile 0, then 10 of profile 1, best first.
        let items: Vec<(u32, Option<usize>)> = (0..100).map(|k| (k, Some(usize::from(k >= 90)))).collect();
        let shelf = spread(&items, &[0.8, 0.2], 10);
        assert_eq!(shelf.len(), 10);
        assert_eq!(shelf.iter().filter(|k| **k >= 90).count(), 2, "{shelf:?}");
        let tiny = spread(&items, &[0.97, 0.03, 0.0], 10);
        assert_eq!(tiny.iter().filter(|k| **k >= 90).count(), 0, "below the active share: {tiny:?}");
        let small = spread(&items, &[0.95, 0.05], 10);
        assert_eq!(small.iter().filter(|k| **k >= 90).count(), 1, "at least one: {small:?}");
        // Short of items: the rest fills.
        let few: Vec<(u32, Option<usize>)> = (0..12).map(|k| (k, Some(usize::from(k >= 11)))).collect();
        assert_eq!(spread(&few, &[0.5, 0.5], 10).len(), 10);
    }

}
