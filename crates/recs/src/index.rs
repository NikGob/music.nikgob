//! The library's sound vectors in memory:
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
//!
//! Beside each bliss vector a track may have the neural sound model's
//! compact vector ([`crate::embedding::Compact`], 260 bytes). Where both
//! tracks of a pair have one, their distance is a blend
//! ([`SOUND_MODEL_SHARE`] of the model's, the rest bliss's, the model's
//! distance brought to bliss's scale over the library): the model hears the
//! genre and the voice, bliss the tempo and the loudness. A track without
//! one is compared by bliss alone, so new tracks and a library being
//! analysed mix freely. [`VectorIndex::neighbours`], [`VectorIndex::nearest_to`]
//! and [`VectorIndex::niche_fit`] use the blend, and for a track the index
//! does not hold (a preview of new music, with its own model vector)
//! [`VectorIndex::nearest_with`] and [`VectorIndex::probe_fit`];
//! [`VectorIndex::nearest`] and [`VectorIndex::distance`] of raw vectors
//! stay bliss's.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;

use bliss_audio::FeaturesVersion;

use crate::analysis::{DIM, Vector};
use crate::embedding::{COMPACT_DIM, Compact};

/// The model's share of the distance of two tracks that both have its vector.
pub const SOUND_MODEL_SHARE: f32 = 0.6;
/// With fewer tracks of the model's vector, distances are bliss's alone
/// (its scale cannot be matched to bliss's yet).
pub const MIN_EMBEDDED: usize = 16;
/// Pairs of tracks sampled to match the model's scale to bliss's.
const CALIBRATION_PAIRS: usize = 512;
/// A niche's members a candidate is measured against (its nearest ones).
const NICHE_NEAREST: usize = 5;
/// [`VectorIndex::fits_niche`]: a candidate this close to the niche, by
/// [`VectorIndex::niche_fit`], fits it.
pub const NICHE_FITS: f32 = 0.7;

/// A niche's members a [`NicheProbe`] measures (an even sample of a
/// larger niche).
const NICHE_SAMPLE: usize = 32;

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
    /// The model's vectors row by row beside `data` (`COMPACT_DIM` bytes a
    /// row; empty until the first one comes).
    emb: Vec<i8>,
    /// Each row's scale; 0 = the row has no model vector.
    emb_scale: Vec<f32>,
    emb_count: usize,
    /// Sum of the stored model vectors: their mean centres them.
    emb_sum: Vec<f64>,
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
            emb: Vec::new(),
            emb_scale: Vec::new(),
            emb_count: 0,
            emb_sum: Vec::new(),
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
                if !self.emb_scale.is_empty() {
                    self.emb.extend_from_slice(&[0; COMPACT_DIM]);
                    self.emb_scale.push(0.0);
                }
            }
        }
        self.account(&vector, 1.0);
        true
    }

    /// Removes `key`; returns its vector.
    pub fn remove(&mut self, key: K) -> Option<Vector> {
        let i = *self.pos.get(&key)?;
        self.remove_embedding(key);
        self.pos.remove(&key);
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
        if !self.emb_scale.is_empty() {
            self.emb_scale.swap_remove(i);
            if i != last {
                let (head, tail) = self.emb.split_at_mut(last * COMPACT_DIM);
                head[i * COMPACT_DIM..(i + 1) * COMPACT_DIM].copy_from_slice(&tail[..COMPACT_DIM]);
            }
            self.emb.truncate(last * COMPACT_DIM);
        }
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

    /// The `k` neighbours of a stored `key` (itself excluded), by the blend
    /// with the model's distance where both have its vector
    /// ([`Self::nearest_to`]); empty when `key` has no vector.
    pub fn neighbours(&self, key: K, k: usize, mut keep: impl FnMut(K) -> bool) -> Vec<(K, f32)> {
        let Some(query) = self.get(key).copied() else {
            return Vec::new();
        };
        self.nearest_to(Some(key), &query, k, |other| other != key && keep(other))
    }
}

impl<K: Copy + Eq + Hash> VectorIndex<K> {
    /// Tracks with the model's vector.
    pub fn embedded_len(&self) -> usize {
        self.emb_count
    }

    fn emb_row(&self, i: usize) -> Option<Compact> {
        let scale = *self.emb_scale.get(i)?;
        (scale > 0.0).then(|| Compact {
            scale,
            q: self.emb[i * COMPACT_DIM..(i + 1) * COMPACT_DIM].try_into().expect("rows are COMPACT_DIM long"),
        })
    }

    /// The model's vector of `key`.
    pub fn embedding(&self, key: K) -> Option<Compact> {
        self.pos.get(&key).and_then(|&i| self.emb_row(i))
    }

    fn account_emb(&mut self, c: &Compact, sign: f64) {
        if self.emb_sum.is_empty() {
            self.emb_sum = vec![0.0; COMPACT_DIM];
        }
        for (s, v) in self.emb_sum.iter_mut().zip(c.values()) {
            *s += sign * f64::from(v);
        }
    }

    /// Sets the model's vector of a track that has a bliss vector (false
    /// otherwise: a track is always compared by bliss at least).
    pub fn set_embedding(&mut self, key: K, c: Compact) -> bool {
        let Some(&i) = self.pos.get(&key) else { return false };
        if !(c.scale > 0.0 && c.scale.is_finite()) {
            return false;
        }
        if self.emb_scale.is_empty() {
            self.emb_scale = vec![0.0; self.keys.len()];
            self.emb = vec![0; self.keys.len() * COMPACT_DIM];
        }
        match self.emb_row(i) {
            Some(old) => self.account_emb(&old, -1.0),
            None => self.emb_count += 1,
        }
        self.account_emb(&c, 1.0);
        self.emb_scale[i] = c.scale;
        self.emb[i * COMPACT_DIM..(i + 1) * COMPACT_DIM].copy_from_slice(&c.q);
        true
    }

    /// Drops the model's vector of `key`; returns it.
    pub fn remove_embedding(&mut self, key: K) -> Option<Compact> {
        let i = *self.pos.get(&key)?;
        let old = self.emb_row(i)?;
        self.account_emb(&old, -1.0);
        self.emb_scale[i] = 0.0;
        self.emb_count -= 1;
        if self.emb_count == 0 {
            self.emb_sum.iter_mut().for_each(|s| *s = 0.0);
        }
        Some(old)
    }

    /// Drops every model vector (the neural analysis was switched off).
    pub fn clear_embeddings(&mut self) {
        self.emb = Vec::new();
        self.emb_scale = Vec::new();
        self.emb_sum = Vec::new();
        self.emb_count = 0;
    }

    /// The model's vectors take part in distances.
    fn blended(&self) -> bool {
        self.emb_count >= MIN_EMBEDDED
    }

    /// The library's mean model vector.
    fn emb_mean(&self) -> [f32; COMPACT_DIM] {
        let n = self.emb_count.max(1) as f64;
        std::array::from_fn(|i| self.emb_sum.get(i).map_or(0.0, |s| (s / n) as f32))
    }

    /// Row `i`'s model vector less the mean, made unit length.
    fn centred(&self, i: usize, mean: &[f32; COMPACT_DIM]) -> Option<[f32; COMPACT_DIM]> {
        Self::centred_of(&self.emb_row(i)?, mean)
    }

    /// A model vector less the mean, made unit length.
    fn centred_of(c: &Compact, mean: &[f32; COMPACT_DIM]) -> Option<[f32; COMPACT_DIM]> {
        let mut v: [f32; COMPACT_DIM] = std::array::from_fn(|k| f32::from(c.q[k]) * c.scale - mean[k]);
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !(n > 1e-9) {
            return None;
        }
        v.iter_mut().for_each(|x| *x /= n);
        Some(v)
    }

    /// The model's distance of a centred unit query to row `i` (0–2).
    fn emb_dist(&self, q: &[f32; COMPACT_DIM], i: usize, mean: &[f32; COMPACT_DIM]) -> Option<f32> {
        let scale = *self.emb_scale.get(i)?;
        if scale <= 0.0 {
            return None;
        }
        let row = &self.emb[i * COMPACT_DIM..(i + 1) * COMPACT_DIM];
        let (mut dot, mut nn) = (0f32, 0f32);
        for k in 0..COMPACT_DIM {
            let x = f32::from(row[k]) * scale - mean[k];
            dot += x * q[k];
            nn += x * x;
        }
        if !(nn > 1e-18) {
            return None;
        }
        let cos = (dot / nn.sqrt()).clamp(-1.0, 1.0);
        Some((2.0 - 2.0 * cos).max(0.0).sqrt())
    }

    /// Bliss's distance per unit of the model's, over pairs of the
    /// library's tracks that have both (pairs spread over the library the
    /// same way every time).
    fn calibration(&self, mean: &[f32; COMPACT_DIM]) -> f32 {
        let rows: Vec<usize> = (0..self.emb_scale.len()).filter(|&i| self.emb_scale[i] > 0.0).collect();
        let n = rows.len();
        if n < 2 {
            return 1.0;
        }
        let f = self.factors();
        let (mut sb, mut se) = (0f64, 0f64);
        for p in 0..CALIBRATION_PAIRS.min(n * (n - 1) / 2) {
            let a = rows[(p * 7919) % n];
            let b = rows[(p * 7919 + 1 + p % (n - 1)) % n];
            if a == b {
                continue;
            }
            let Some(de) = self.centred(a, mean).and_then(|q| self.emb_dist(&q, b, mean)) else { continue };
            sb += f64::from(Self::dist_sq(&f, self.row(a), self.row(b)).sqrt());
            se += f64::from(de);
        }
        if se > 1e-9 { (sb / se) as f32 } else { 1.0 }
    }

    /// The `k` stored tracks closest to a track whose bliss vector is
    /// `query`, nearest first; `key`, when it is stored with the model's
    /// vector, blends the model's distance in for the tracks that have it
    /// (once [`MIN_EMBEDDED`] tracks do). The track itself is not left out.
    pub fn nearest_to(&self, key: Option<K>, query: &Vector, k: usize, keep: impl FnMut(K) -> bool) -> Vec<(K, f32)> {
        let model = key.filter(|_| self.blended()).and_then(|key| self.pos.get(&key).copied()).and_then(|i| {
            let mean = self.emb_mean();
            let q = self.centred(i, &mean)?;
            let ratio = self.calibration(&mean);
            Some((q, mean, ratio))
        });
        match model {
            Some((q, mean, ratio)) => self.nearest_blended(query, &q, &mean, ratio, k, keep),
            None => self.nearest(query, k, keep),
        }
    }

    /// [`Self::nearest_to`] for a track the index does not hold: its bliss
    /// vector `query` and the model's vector `model` when it has one (a
    /// preview of new music the model heard).
    pub fn nearest_with(&self, query: &Vector, model: Option<&Compact>, k: usize, keep: impl FnMut(K) -> bool) -> Vec<(K, f32)> {
        let blend = model.filter(|_| self.blended()).and_then(|c| {
            let mean = self.emb_mean();
            let q = Self::centred_of(c, &mean)?;
            Some((q, mean))
        });
        match blend {
            Some((q, mean)) => {
                let ratio = self.calibration(&mean);
                self.nearest_blended(query, &q, &mean, ratio, k, keep)
            }
            None => self.nearest(query, k, keep),
        }
    }

    /// The `k` nearest to `query` with the model's centred query `q`
    /// blended in where a row has the model's vector.
    fn nearest_blended(
        &self,
        query: &Vector,
        q: &[f32; COMPACT_DIM],
        mean: &[f32; COMPACT_DIM],
        ratio: f32,
        k: usize,
        mut keep: impl FnMut(K) -> bool,
    ) -> Vec<(K, f32)> {
        if k == 0 {
            return Vec::new();
        }
        let f = self.factors();
        let mut heap: BinaryHeap<Cand> = BinaryHeap::with_capacity(k + 1);
        for (i, row) in self.data.as_chunks::<DIM>().0.iter().enumerate() {
            let bliss = Self::dist_sq(&f, query, row).sqrt();
            let d = match self.emb_dist(q, i, mean) {
                Some(e) => (1.0 - SOUND_MODEL_SHARE) * bliss + SOUND_MODEL_SHARE * ratio * e,
                None => bliss,
            };
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
        heap.into_sorted_vec().into_iter().map(|Cand(d, i)| (self.keys[i], d)).collect()
    }

    /// The distance of two stored tracks: the blend when both have the
    /// model's vector, else bliss's.
    pub fn key_distance(&self, a: K, b: K) -> Option<f32> {
        let (&i, &j) = (self.pos.get(&a)?, self.pos.get(&b)?);
        let bliss = Self::dist_sq(&self.factors(), self.row(i), self.row(j)).sqrt();
        if !self.blended() {
            return Some(bliss);
        }
        let mean = self.emb_mean();
        match self.centred(i, &mean).and_then(|q| self.emb_dist(&q, j, &mean)) {
            Some(e) => Some((1.0 - SOUND_MODEL_SHARE) * bliss + SOUND_MODEL_SHARE * self.calibration(&mean) * e),
            None => Some(bliss),
        }
    }

    /// How well `candidate` fits a niche of tracks (`niche`: e.g. the
    /// library's tracks of a mood, a filter or a profile) by sound: the
    /// niche expansion's check of new music. The mean distance from the candidate to its few nearest
    /// members, against the same of the members among themselves: 1.0 = as
    /// close as the niche's own tracks are to each other (or closer),
    /// towards 0 the further away. Uses the blend, so a candidate whose
    /// preview the model heard is judged by genre and voice too. `None`
    /// when the candidate has no vector or fewer than two members do.
    ///
    /// Cost: `O(n²)` distances in the niche's size `n` (small: a niche is
    /// expanded below 25 tracks; pass a sample of a large one).
    pub fn niche_fit(&self, candidate: K, niche: &[K]) -> Option<f32> {
        if !self.contains(candidate) {
            return None;
        }
        let members: Vec<K> = niche.iter().copied().filter(|k| *k != candidate && self.contains(*k)).collect();
        if members.len() < 2 {
            return None;
        }
        let m = NICHE_NEAREST.min(members.len() - 1);
        // One calibration for the whole check.
        let mean = self.emb_mean();
        let ratio = if self.blended() { self.calibration(&mean) } else { 1.0 };
        let f = self.factors();
        let dist = |a: K, b: K| -> Option<f32> {
            let (&i, &j) = (self.pos.get(&a)?, self.pos.get(&b)?);
            let bliss = Self::dist_sq(&f, self.row(i), self.row(j)).sqrt();
            let model = self.blended().then(|| self.centred(i, &mean).and_then(|q| self.emb_dist(&q, j, &mean))).flatten();
            Some(match model {
                Some(e) => (1.0 - SOUND_MODEL_SHARE) * bliss + SOUND_MODEL_SHARE * ratio * e,
                None => bliss,
            })
        };
        let near_mean = |from: K| -> Option<f32> {
            let mut d: Vec<f32> = members.iter().filter(|k| **k != from).filter_map(|k| dist(from, *k)).collect();
            if d.is_empty() {
                return None;
            }
            d.sort_by(f32::total_cmp);
            let take = m.clamp(1, d.len());
            Some(d[..take].iter().sum::<f32>() / take as f32)
        };
        let own = near_mean(candidate)?;
        let mut spread: Vec<f32> = members.iter().filter_map(|k| near_mean(*k)).collect();
        spread.sort_by(f32::total_cmp);
        let typical = *spread.get(spread.len() / 2)?;
        if own <= 1e-9 {
            return Some(1.0);
        }
        Some((typical / own).clamp(0.0, 1.0))
    }

    /// [`Self::niche_fit`] at or above [`NICHE_FITS`].
    pub fn fits_niche(&self, candidate: K, niche: &[K]) -> Option<bool> {
        self.niche_fit(candidate, niche).map(|f| f >= NICHE_FITS)
    }

    /// The niche of `niche` measured once for many candidates the index
    /// does not hold ([`Self::probe_fit`]): an even sample of
    /// [`NICHE_SAMPLE`] of its members at most, how close they are to each
    /// other. `None` when fewer than two members have a vector.
    pub fn niche_probe(&self, niche: &[K]) -> Option<NicheProbe<K>> {
        let held: Vec<usize> = niche.iter().filter_map(|k| self.pos.get(k).copied()).collect();
        let step = held.len().div_ceil(NICHE_SAMPLE).max(1);
        let rows: Vec<usize> = held.into_iter().step_by(step).collect();
        if rows.len() < 2 {
            return None;
        }
        let nearest = NICHE_NEAREST.min(rows.len() - 1);
        let mean = self.emb_mean();
        let ratio = if self.blended() { self.calibration(&mean) } else { 1.0 };
        let f = self.factors();
        let centred: Vec<Option<[f32; COMPACT_DIM]>> =
            rows.iter().map(|&i| if self.blended() { self.centred(i, &mean) } else { None }).collect();
        let mut spread: Vec<f32> = (0..rows.len())
            .map(|a| {
                let mut d: Vec<f32> = (0..rows.len())
                    .filter(|&b| b != a)
                    .map(|b| {
                        let bliss = Self::dist_sq(&f, self.row(rows[a]), self.row(rows[b])).sqrt();
                        match centred[a].as_ref().and_then(|q| self.emb_dist(q, rows[b], &mean)) {
                            Some(e) => (1.0 - SOUND_MODEL_SHARE) * bliss + SOUND_MODEL_SHARE * ratio * e,
                            None => bliss,
                        }
                    })
                    .collect();
                d.sort_by(f32::total_cmp);
                d[..nearest].iter().sum::<f32>() / nearest as f32
            })
            .collect();
        spread.sort_by(f32::total_cmp);
        let typical = spread[spread.len() / 2];
        Some(NicheProbe { members: rows.iter().map(|&i| self.keys[i]).collect(), nearest, typical, mean, ratio })
    }

    /// How well a track the index does not hold fits the niche of `probe`,
    /// as [`Self::niche_fit`] tells it: its bliss vector `vector` and the
    /// model's vector `model` when the model heard it (the blend with the
    /// members that have one). `None` when no member is held any more.
    pub fn probe_fit(&self, probe: &NicheProbe<K>, vector: &Vector, model: Option<&Compact>) -> Option<f32> {
        let f = self.factors();
        let q = model.filter(|_| self.blended()).and_then(|c| Self::centred_of(c, &probe.mean));
        let mut d: Vec<f32> = probe
            .members
            .iter()
            .filter_map(|k| self.pos.get(k).copied())
            .map(|j| {
                let bliss = Self::dist_sq(&f, vector, self.row(j)).sqrt();
                match q.as_ref().and_then(|q| self.emb_dist(q, j, &probe.mean)) {
                    Some(e) => (1.0 - SOUND_MODEL_SHARE) * bliss + SOUND_MODEL_SHARE * probe.ratio * e,
                    None => bliss,
                }
            })
            .collect();
        if d.is_empty() {
            return None;
        }
        d.sort_by(f32::total_cmp);
        let take = probe.nearest.clamp(1, d.len());
        let own = d[..take].iter().sum::<f32>() / take as f32;
        if own <= 1e-9 {
            return Some(1.0);
        }
        Some((probe.typical / own).clamp(0.0, 1.0))
    }
}

/// A niche measured once ([`VectorIndex::niche_probe`]) for the
/// candidates [`VectorIndex::probe_fit`] judges: previews of new music.
#[derive(Clone, Debug)]
pub struct NicheProbe<K> {
    /// The members measured.
    members: Vec<K>,
    /// How many nearest members a distance is the mean of.
    nearest: usize,
    /// The members' typical distance to their nearest fellow members.
    typical: f32,
    /// The library's mean model vector and the model's scale to bliss's
    /// when the probe was made.
    mean: [f32; COMPACT_DIM],
    ratio: f32,
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

    /// Feature `i` of an [`Self::embed`]ded vector as bliss gave it (none
    /// for a feature weighed 0 or flat over the library).
    pub fn raw_of_embedded(&self, e: &Vector, i: usize) -> Option<f32> {
        (self.sqrt_w[i] > 0.0 && self.inv_scale[i] > 0.0).then(|| e[i] / self.sqrt_w[i] / self.inv_scale[i] + self.mean[i])
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

    /// A compact vector near `centre` (a direction of the model's space).
    fn compact_near(rng: &mut Rng, centre: &[f32; COMPACT_DIM], noise: f32) -> Compact {
        let v: [f32; COMPACT_DIM] = std::array::from_fn(|i| centre[i] + rng.next_f32() * noise);
        Compact::of_unit(&v).unwrap()
    }

    fn direction(rng: &mut Rng) -> [f32; COMPACT_DIM] {
        std::array::from_fn(|_| rng.next_f32())
    }

    /// The model's vectors follow their rows through inserts and removals,
    /// and only tracks with a bliss vector take one.
    #[test]
    fn embeddings_follow_their_rows() {
        let mut rng = Rng(21);
        let mut idx = VectorIndex::new();
        for k in 0..10u32 {
            idx.insert(k, random(&mut rng));
        }
        let dir = direction(&mut rng);
        let c: Vec<Compact> = (0..10).map(|_| compact_near(&mut rng, &dir, 1.0)).collect();
        assert!(!idx.set_embedding(99, c[0]), "no bliss vector, no model vector");
        for k in [1u32, 3, 9] {
            assert!(idx.set_embedding(k, c[k as usize]));
        }
        assert_eq!(idx.embedded_len(), 3);
        // A new track after the first model vector gets an empty row.
        idx.insert(10, random(&mut rng));
        assert_eq!(idx.embedding(10), None);
        // Removing a row moves the last one into its place, with its vector.
        assert_eq!(idx.remove(1).is_some(), true);
        assert_eq!(idx.embedded_len(), 2);
        assert_eq!(idx.embedding(9), Some(c[9]));
        assert_eq!(idx.embedding(3), Some(c[3]));
        idx.remove(0);
        assert_eq!(idx.embedding(9), Some(c[9]));
        assert_eq!(idx.remove_embedding(9), Some(c[9]));
        assert_eq!(idx.embedding(9), None);
        assert_eq!(idx.embedded_len(), 1);
        // Replacing a vector keeps the count.
        assert!(idx.set_embedding(3, c[4]));
        assert_eq!(idx.embedded_len(), 1);
        assert_eq!(idx.embedding(3), Some(c[4]));
    }

    /// Two genres bliss cannot tell apart (their bliss vectors are drawn
    /// alike) but the model can: with the model's vectors a track's
    /// neighbours are of its genre; a track without one is still found by
    /// bliss, and below [`MIN_EMBEDDED`] nothing changes.
    #[test]
    fn the_model_blends_into_neighbours() {
        let mut rng = Rng(3);
        let mut idx = VectorIndex::new();
        let (rock, rap) = (direction(&mut rng), direction(&mut rng));
        for k in 0..60u32 {
            idx.insert(k, random(&mut rng));
        }
        let genre = |k: u32| k % 2;
        let bliss_only: Vec<u32> = idx.neighbours(0, 10, |_| true).into_iter().map(|(k, _)| k).collect();
        for k in 0..60u32 {
            let dir = if genre(k) == 0 { &rock } else { &rap };
            idx.set_embedding(k, compact_near(&mut rng, dir, 0.3));
        }
        let near: Vec<u32> = idx.neighbours(0, 10, |_| true).into_iter().map(|(k, _)| k).collect();
        let same = near.iter().filter(|k| genre(**k) == 0).count();
        assert!(same >= 9, "{near:?}");
        let before = bliss_only.iter().filter(|k| genre(**k) == 0).count();
        assert!(same > before, "{same} vs {before}");
        // Ordered, and the distances agree with key_distance.
        let with_d = idx.neighbours(0, 5, |_| true);
        for w in with_d.windows(2) {
            assert!(w[0].1 <= w[1].1);
        }
        assert!((idx.key_distance(0, with_d[0].0).unwrap() - with_d[0].1).abs() < 1e-3);
        // A track without the model's vector: bliss alone, still found.
        idx.insert(100, *idx.get(0).unwrap());
        let near: Vec<u32> = idx.neighbours(0, 1, |_| true).into_iter().map(|(k, _)| k).collect();
        assert_eq!(near, [100], "same bliss vector, no model vector: nearest by bliss");

        let mut small = VectorIndex::new();
        for k in 0..20u32 {
            small.insert(k, random(&mut rng));
        }
        let plain: Vec<(u32, f32)> = small.neighbours(0, 5, |_| true);
        for k in 0..(MIN_EMBEDDED as u32 - 1) {
            small.set_embedding(k, compact_near(&mut rng, &rock, 1.0));
        }
        assert_eq!(small.neighbours(0, 5, |_| true), plain, "too few model vectors to blend");
    }

    /// A candidate of the niche's genre fits it, one of another genre does
    /// not; no vector or too small a niche says nothing.
    #[test]
    fn niche_fit_tells_a_fitting_candidate() {
        let mut rng = Rng(8);
        let mut idx = VectorIndex::new();
        let (calm, loud) = (direction(&mut rng), direction(&mut rng));
        for k in 0..80u32 {
            idx.insert(k, random(&mut rng));
            let dir = if k < 40 { &calm } else { &loud };
            idx.set_embedding(k, compact_near(&mut rng, dir, 0.25));
        }
        let niche: Vec<u32> = (0..12).collect();
        let fit_in = idx.niche_fit(30, &niche).unwrap();
        let fit_out = idx.niche_fit(70, &niche).unwrap();
        assert!(fit_in > fit_out, "{fit_in} vs {fit_out}");
        assert_eq!(idx.fits_niche(30, &niche), Some(true), "{fit_in}");
        assert_eq!(idx.fits_niche(70, &niche), Some(false), "{fit_out}");
        assert_eq!(idx.niche_fit(500, &niche), None);
        assert_eq!(idx.niche_fit(30, &[1]), None);
        assert!((0.0..=1.0).contains(&fit_out));
    }

    /// A preview the index does not hold is judged by its own model
    /// vector: its neighbours and its niche are of its genre, as a held
    /// track's are; without the model's vector, by bliss alone.
    #[test]
    fn a_preview_outside_the_index_is_heard_by_the_model() {
        let mut rng = Rng(8);
        let mut idx = VectorIndex::new();
        let (calm, loud) = (direction(&mut rng), direction(&mut rng));
        for k in 0..80u32 {
            idx.insert(k, random(&mut rng));
            let dir = if k < 40 { &calm } else { &loud };
            idx.set_embedding(k, compact_near(&mut rng, dir, 0.25));
        }
        let bliss = random(&mut rng);
        let calm_preview = compact_near(&mut rng, &calm, 0.25);
        let near: Vec<u32> = idx.nearest_with(&bliss, Some(&calm_preview), 10, |_| true).into_iter().map(|(k, _)| k).collect();
        assert!(near.iter().filter(|k| **k < 40).count() >= 9, "{near:?}");
        assert_eq!(idx.nearest_with(&bliss, None, 10, |_| true), idx.nearest(&bliss, 10, |_| true));
        // The same as a held track with the same vectors.
        idx.insert(500, bliss);
        idx.set_embedding(500, calm_preview);
        let held: Vec<(u32, f32)> = idx.nearest_to(Some(500), &bliss, 6, |k| k != 500);
        let outside: Vec<(u32, f32)> = idx.nearest_with(&bliss, Some(&calm_preview), 6, |k| k != 500);
        assert_eq!(held.iter().map(|x| x.0).collect::<Vec<_>>(), outside.iter().map(|x| x.0).collect::<Vec<_>>());
        idx.remove(500);

        let niche: Vec<u32> = (0..12).collect();
        let probe = idx.niche_probe(&niche).unwrap();
        let fit_in = idx.probe_fit(&probe, &bliss, Some(&calm_preview)).unwrap();
        let fit_out = idx.probe_fit(&probe, &bliss, Some(&compact_near(&mut rng, &loud, 0.25))).unwrap();
        assert!(fit_in >= NICHE_FITS && fit_out < NICHE_FITS, "{fit_in} vs {fit_out}");
        assert!(idx.niche_probe(&[1]).is_none());
        // A large niche is sampled.
        let all: Vec<u32> = (0..80).collect();
        assert!(idx.niche_probe(&all).unwrap().members.len() <= NICHE_SAMPLE);
    }
}
