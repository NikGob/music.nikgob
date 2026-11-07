//! The library's sound vectors in memory (docs/stage4.md §2 «Индекс»):
//! 50 000 tracks × 23 `f32` is about 5 MB, and a brute-force pass over all
//! of them takes a few milliseconds, so there is no tree to keep balanced.
//!
//! Distances are z-normalised over the stored vectors themselves (each
//! feature divided by its standard deviation in this library, so a feature
//! that barely varies here does not drown in one that varies a lot) and
//! weighted per feature with bliss's own weights for its features version
//! (the tempo counts a quarter, the 13 chroma features about three features
//! together), then euclidean. The running sums behind the statistics follow
//! every insert, update and removal.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;

use bliss_audio::FeaturesVersion;

use crate::analysis::{DIM, Vector};

/// A standard deviation below this counts as "does not vary" (scale 1).
const MIN_STD: f64 = 1e-4;

/// bliss's distance weights for the features version we analyse with.
fn bliss_weights() -> [f32; DIM] {
    let m = FeaturesVersion::LATEST.feature_weights();
    std::array::from_fn(|i| m[[i, i]])
}

/// Sound vectors by key (a track id in the runtime).
#[derive(Clone, Debug)]
pub struct VectorIndex<K> {
    keys: Vec<K>,
    /// `keys.len() * DIM` features, row `i` belongs to `keys[i]`.
    data: Vec<f32>,
    pos: HashMap<K, usize>,
    sum: [f64; DIM],
    sum_sq: [f64; DIM],
    weights: [f32; DIM],
}

impl<K: Copy + Eq + Hash> Default for VectorIndex<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// A candidate of a nearest-neighbour pass, ordered by distance.
struct Cand(f32, usize);

impl PartialEq for Cand {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Cand {}

impl PartialOrd for Cand {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Cand {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
    }
}

impl<K: Copy + Eq + Hash> VectorIndex<K> {
    /// An empty index with bliss's feature weights.
    pub fn new() -> Self {
        Self::with_weights(bliss_weights())
    }

    /// An empty index with these feature weights (1.0 everywhere = plain
    /// z-normalised euclidean distance).
    pub fn with_weights(weights: [f32; DIM]) -> Self {
        Self {
            keys: Vec::new(),
            data: Vec::new(),
            pos: HashMap::new(),
            sum: [0.0; DIM],
            sum_sq: [0.0; DIM],
            weights,
        }
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn contains(&self, key: K) -> bool {
        self.pos.contains_key(&key)
    }

    fn row(&self, i: usize) -> &Vector {
        self.data[i * DIM..(i + 1) * DIM].try_into().expect("rows are DIM long")
    }

    pub fn get(&self, key: K) -> Option<&Vector> {
        self.pos.get(&key).map(|&i| self.row(i))
    }

    /// Every key with its (raw, not normalised) vector.
    pub fn iter(&self) -> impl Iterator<Item = (K, &Vector)> + '_ {
        self.keys.iter().enumerate().map(|(i, k)| (*k, self.row(i)))
    }

    fn account(&mut self, v: &Vector, sign: f64) {
        for (i, x) in v.iter().enumerate() {
            let x = f64::from(*x);
            self.sum[i] += sign * x;
            self.sum_sq[i] += sign * x * x;
        }
    }

    /// Adds `key`, or replaces its vector. A vector with a non-finite
    /// feature is refused (returns false; an older vector of `key` stays).
    pub fn insert(&mut self, key: K, vector: Vector) -> bool {
        if vector.iter().any(|v| !v.is_finite()) {
            return false;
        }
        match self.pos.get(&key) {
            Some(&i) => {
                let old = *self.row(i);
                self.account(&old, -1.0);
                self.data[i * DIM..(i + 1) * DIM].copy_from_slice(&vector);
            }
            None => {
                self.pos.insert(key, self.keys.len());
                self.keys.push(key);
                self.data.extend_from_slice(&vector);
            }
        }
        self.account(&vector, 1.0);
        true
    }

    /// Removes `key`; returns its vector.
    pub fn remove(&mut self, key: K) -> Option<Vector> {
        let i = self.pos.remove(&key)?;
        let old = *self.row(i);
        self.account(&old, -1.0);
        let last = self.keys.len() - 1;
        self.keys.swap_remove(i);
        if i != last {
            let (head, tail) = self.data.split_at_mut(last * DIM);
            head[i * DIM..(i + 1) * DIM].copy_from_slice(&tail[..DIM]);
            self.pos.insert(self.keys[i], i);
        }
        self.data.truncate(last * DIM);
        if self.keys.is_empty() {
            // No drift left over from the running sums.
            self.sum = [0.0; DIM];
            self.sum_sq = [0.0; DIM];
        }
        Some(old)
    }

    /// Mean of every feature over the stored vectors.
    pub fn mean(&self) -> Vector {
        let n = self.keys.len().max(1) as f64;
        std::array::from_fn(|i| (self.sum[i] / n) as f32)
    }

    /// Standard deviation of every feature over the stored vectors.
    pub fn std_dev(&self) -> Vector {
        let n = self.keys.len().max(1) as f64;
        std::array::from_fn(|i| {
            let mean = self.sum[i] / n;
            (self.sum_sq[i] / n - mean * mean).max(0.0).sqrt() as f32
        })
    }

    /// The divisor of each feature: its deviation, 1 when it does not vary.
    fn scales(&self) -> [f64; DIM] {
        let std = self.std_dev();
        std::array::from_fn(|i| if f64::from(std[i]) < MIN_STD { 1.0 } else { f64::from(std[i]) })
    }

    /// `v` as z-scores over this library (mood axes and clusters work on
    /// these).
    pub fn normalized(&self, v: &Vector) -> Vector {
        let mean = self.mean();
        let scales = self.scales();
        std::array::from_fn(|i| ((f64::from(v[i]) - f64::from(mean[i])) / scales[i]) as f32)
    }

    /// Per-feature factors: the weighted, normalised squared distance is
    /// `sum((f[i] * (a[i] - b[i]))^2)`.
    fn factors(&self) -> [f32; DIM] {
        let scales = self.scales();
        std::array::from_fn(|i| (f64::from(self.weights[i]).sqrt() / scales[i]) as f32)
    }

    fn dist_sq(f: &[f32; DIM], a: &[f32], b: &[f32]) -> f32 {
        let mut acc = 0.0f32;
        for i in 0..DIM {
            let d = f[i] * (a[i] - b[i]);
            acc += d * d;
        }
        acc
    }

    /// The distance between two vectors under the current statistics.
    pub fn distance(&self, a: &Vector, b: &Vector) -> f32 {
        Self::dist_sq(&self.factors(), a, b).sqrt()
    }

    /// The `k` stored vectors closest to `query` that `keep` accepts,
    /// nearest first, with their distances.
    pub fn nearest(&self, query: &Vector, k: usize, mut keep: impl FnMut(K) -> bool) -> Vec<(K, f32)> {
        if k == 0 {
            return Vec::new();
        }
        let f = self.factors();
        let mut heap: BinaryHeap<Cand> = BinaryHeap::with_capacity(k + 1);
        for (i, row) in self.data.as_chunks::<DIM>().0.iter().enumerate() {
            let d = Self::dist_sq(&f, query, row);
            if heap.len() == k && heap.peek().is_some_and(|worst| d >= worst.0) {
                continue;
            }
            if !keep(self.keys[i]) {
                continue;
            }
            heap.push(Cand(d, i));
            if heap.len() > k {
                heap.pop();
            }
        }
        heap.into_sorted_vec()
            .into_iter()
            .map(|Cand(d, i)| (self.keys[i], d.sqrt()))
            .collect()
    }

    /// The current statistics frozen into a [`Normalizer`], for passes that
    /// normalise many vectors (moods, clusters, ordering).
    pub fn normalizer(&self) -> Normalizer {
        let mean = self.mean();
        let scales = self.scales();
        Normalizer {
            mean,
            inv_scale: std::array::from_fn(|i| (1.0 / scales[i]) as f32),
            sqrt_w: std::array::from_fn(|i| self.weights[i].sqrt()),
        }
    }

    /// The `k` neighbours of a stored `key` (itself excluded); empty when
    /// `key` has no vector.
    pub fn neighbours(&self, key: K, k: usize, mut keep: impl FnMut(K) -> bool) -> Vec<(K, f32)> {
        let Some(query) = self.get(key).copied() else {
            return Vec::new();
        };
        self.nearest(&query, k, |other| other != key && keep(other))
    }
}

/// An index's normalisation at one moment: z-scores
/// ([`Normalizer::z`]) and the weighted space ([`Normalizer::embed`]) in
/// which plain euclidean distance ([`euclid`]) equals
/// [`VectorIndex::distance`].
#[derive(Clone, Debug)]
pub struct Normalizer {
    mean: Vector,
    inv_scale: [f32; DIM],
    sqrt_w: [f32; DIM],
}

impl Normalizer {
    /// Each feature as a z-score over the library.
    pub fn z(&self, v: &Vector) -> Vector {
        std::array::from_fn(|i| (v[i] - self.mean[i]) * self.inv_scale[i])
    }

    /// z-scores scaled by the square roots of the feature weights.
    pub fn embed(&self, v: &Vector) -> Vector {
        std::array::from_fn(|i| (v[i] - self.mean[i]) * self.inv_scale[i] * self.sqrt_w[i])
    }

    /// The z-scores of an [`Self::embed`]ded vector (a feature weighed 0
    /// comes back 0).
    pub fn z_of_embedded(&self, e: &Vector) -> Vector {
        std::array::from_fn(|i| if self.sqrt_w[i] > 0.0 { e[i] / self.sqrt_w[i] } else { 0.0 })
    }
}

/// Plain euclidean distance (of two [`Normalizer::embed`]ded vectors).
pub fn euclid(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>().sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::tests::Rng;

    #[test]
    fn the_normalizer_agrees_with_the_index() {
        let mut rng = Rng(9);
        let mut idx = VectorIndex::new();
        for k in 0..100u32 {
            idx.insert(k, random(&mut rng));
        }
        let norm = idx.normalizer();
        let (a, b) = (*idx.get(3).unwrap(), *idx.get(4).unwrap());
        assert!((euclid(&norm.embed(&a), &norm.embed(&b)) - idx.distance(&a, &b)).abs() < 1e-4);
        assert!(close(&norm.z(&a), &idx.normalized(&a)));
    }

    fn v(first: &[f32]) -> Vector {
        let mut out = [0.0; DIM];
        out[..first.len()].copy_from_slice(first);
        out
    }

    fn random(rng: &mut Rng) -> Vector {
        std::array::from_fn(|_| rng.next_f32())
    }

    fn close(a: &Vector, b: &Vector) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn insert_update_remove_keep_the_statistics() {
        let mut rng = Rng(5);
        let mut idx = VectorIndex::new();
        let all: Vec<Vector> = (0..50).map(|_| random(&mut rng)).collect();
        for (k, x) in all.iter().enumerate() {
            assert!(idx.insert(k, *x));
        }
        // Update half of them, remove a few.
        let mut expect: HashMap<usize, Vector> = all.iter().copied().enumerate().collect();
        for k in (0..50).step_by(2) {
            let x = random(&mut rng);
            idx.insert(k, x);
            expect.insert(k, x);
        }
        for k in [0, 7, 49, 13] {
            assert_eq!(idx.remove(k), expect.remove(&k));
        }
        assert_eq!(idx.remove(7), None);
        assert_eq!(idx.len(), expect.len());
        for (k, x) in &expect {
            assert_eq!(idx.get(*k), Some(x));
        }
        assert_eq!(idx.iter().count(), expect.len());

        let n = expect.len() as f64;
        let mean: Vector = std::array::from_fn(|i| (expect.values().map(|x| f64::from(x[i])).sum::<f64>() / n) as f32);
        let std: Vector = std::array::from_fn(|i| {
            let m = f64::from(mean[i]);
            (expect.values().map(|x| (f64::from(x[i]) - m).powi(2)).sum::<f64>() / n).sqrt() as f32
        });
        assert!(close(&idx.mean(), &mean));
        assert!(close(&idx.std_dev(), &std));
        // z-scores of the stored vectors have mean 0 and deviation 1.
        let z: Vec<Vector> = expect.values().map(|x| idx.normalized(x)).collect();
        for i in 0..DIM {
            let m = z.iter().map(|x| f64::from(x[i])).sum::<f64>() / n;
            let s = (z.iter().map(|x| (f64::from(x[i]) - m).powi(2)).sum::<f64>() / n).sqrt();
            assert!(m.abs() < 1e-4 && (s - 1.0).abs() < 1e-3, "feature {i}: {m} {s}");
        }
        for k in expect.keys().copied().collect::<Vec<_>>() {
            idx.remove(k);
        }
        assert!(idx.is_empty());
        assert_eq!(idx.mean(), [0.0; DIM]);
    }

    #[test]
    fn refuses_vectors_that_are_not_numbers() {
        let mut idx = VectorIndex::new();
        assert!(idx.insert(1, v(&[0.5])));
        assert!(!idx.insert(1, v(&[f32::NAN])));
        assert!(!idx.insert(2, v(&[0.0, f32::INFINITY])));
        assert_eq!(idx.get(1), Some(&v(&[0.5])), "the old vector stays");
        assert_eq!(idx.len(), 1);
    }

    #[test]
    fn nearest_is_ordered_filtered_and_skips_the_seed() {
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        for k in 0..10u32 {
            idx.insert(k, v(&[k as f32, 0.0]));
        }
        let near: Vec<u32> = idx.nearest(&v(&[3.2]), 3, |_| true).into_iter().map(|(k, _)| k).collect();
        assert_eq!(near, [3, 4, 2]);
        let odd: Vec<u32> = idx.nearest(&v(&[3.2]), 3, |k| k % 2 == 1).into_iter().map(|(k, _)| k).collect();
        assert_eq!(odd, [3, 5, 1]);
        let around: Vec<(u32, f32)> = idx.neighbours(5, 2, |_| true);
        assert_eq!(around.len(), 2);
        assert!(around.iter().all(|(k, _)| [4, 6].contains(k)));
        assert!(around[0].1 <= around[1].1);
        assert_eq!(idx.nearest(&v(&[0.0]), 100, |_| true).len(), 10);
        assert!(idx.nearest(&v(&[0.0]), 0, |_| true).is_empty());
        assert!(idx.neighbours(42, 3, |_| true).is_empty());
    }

    #[test]
    fn normalisation_lets_a_quiet_feature_count() {
        // Feature 0 spreads over ±100, feature 1 over ±0.01: unnormalised,
        // feature 1 would not matter at all.
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut rng = Rng(11);
        for k in 0..200u32 {
            idx.insert(k, v(&[rng.next_f32() * 100.0, rng.next_f32() * 0.01]));
        }
        idx.insert(1000, v(&[10.0, 0.01]));
        idx.insert(1001, v(&[12.0, -0.01]));
        let q = v(&[11.0, 0.0099]);
        let d_a = idx.distance(&q, idx.get(1000).unwrap());
        let d_b = idx.distance(&q, idx.get(1001).unwrap());
        assert!(d_a < d_b, "the quiet feature decides: {d_a} vs {d_b}");
        // Weights scale a feature's say.
        let mut w = [1.0; DIM];
        w[1] = 0.0;
        let mut flat = VectorIndex::with_weights(w);
        for (k, x) in idx.iter() {
            flat.insert(k, *x);
        }
        let (a, b) = (flat.distance(&q, flat.get(1000).unwrap()), flat.distance(&q, flat.get(1001).unwrap()));
        assert!((a - b).abs() < 1e-4, "without its weight it does not: {a} vs {b}");
    }

    #[test]
    fn bliss_weights_damp_tempo_and_chroma() {
        let w = bliss_weights();
        assert_eq!(w[0], 0.25);
        assert_eq!(w[1], 1.0);
        assert!((w[DIM - 1] - 3.0 / 13.0).abs() < 1e-6);
    }

    #[test]
    fn fifty_thousand_tracks_brute_force() {
        let mut rng = Rng(3);
        let mut idx = VectorIndex::new();
        for k in 0..50_000u32 {
            idx.insert(k, random(&mut rng));
        }
        let q = random(&mut rng);
        let t = std::time::Instant::now();
        let near = idx.nearest(&q, 25, |k| k % 3 != 0);
        let took = t.elapsed();
        assert_eq!(near.len(), 25);
        assert!(near.windows(2).all(|w| w[0].1 <= w[1].1));
        assert!(near.iter().all(|(k, _)| k % 3 != 0));
        // The best one really is the best one.
        let best = idx
            .iter()
            .filter(|(k, _)| k % 3 != 0)
            .map(|(k, x)| (k, idx.distance(&q, x)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        assert_eq!(near[0].0, best.0);
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }
}
