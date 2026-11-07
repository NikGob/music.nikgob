//! «Твои миксы» (docs/stage4.md §4): mixes by style, like Daily Mixes.
//!
//! k-means over the weighted, normalised vectors of what you listen to
//! (tracks with a positive affinity): k from the volume ([`k_for`]: 3…6,
//! never fewer than [`MIN_CLUSTER`] tracks a cluster), started from the
//! previous run's centroids so the mixes do not jump from day to day (the
//! rest k-means++ from the seed). Then every artist goes whole into the
//! cluster that holds most of their tracks, clusters smaller than
//! [`MIN_CLUSTER`] join the nearest one, and two clusters whose centres are
//! closer than [`MERGE_RATIO`] of their radii are one style split in two
//! and merge. Fewer than two clusters left: no style mixes.
//!
//! A mix is named by its two artists with the highest affinity («A, B и
//! другие» — the runtime formats) and covered by its releases with the
//! most affinity; it plays as a radio ([`StyleMix::seed`]).

use std::collections::HashMap;

use nikgob_core::model::{ArtistId, ReleaseId};

use crate::analysis::{DIM, Vector};
use crate::index::euclid;
use crate::mood::Axes;
use crate::pick::Recs;
use crate::radio::RadioSeed;
use crate::rng::Rng;

pub const MIN_CLUSTER: usize = 25;
pub const MIN_K: usize = 3;
pub const MAX_K: usize = 6;
/// Two centres closer than this share of the mean of their RMS radii are
/// one style.
pub const MERGE_RATIO: f32 = 0.5;
const ITERATIONS: usize = 30;
const COVERS: usize = 4;
/// A style mix's mean leans this far from the library's middle on an axis
/// (z-score units) to be named by it.
pub const WORD_LEAN: f32 = 0.2;

/// One style mix.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleMix {
    /// Its position (`MixId::Style`).
    pub index: usize,
    /// The centre in the weighted normalised space: kept by the runtime and
    /// passed back as the next run's start.
    pub centroid: Vec<f32>,
    /// The two artists with the highest affinity (the name).
    pub name_artists: Vec<ArtistId>,
    /// Up to four releases with the most affinity (the cover collage).
    pub covers: Vec<ReleaseId>,
    /// Every artist of the style.
    pub artists: Vec<ArtistId>,
    /// Listened tracks in it.
    pub size: usize,
    /// The mean sound of its tracks on the mood axes (its words,
    /// [`style_words`]).
    pub axes: Axes,
}

/// What a style sounds like in one or two words («спокойное и мрачное»):
/// the axes it leans on most, at least [`WORD_LEAN`] from the library's
/// middle. `None` for a style in the middle of everything.
pub fn style_words(a: Axes) -> Option<String> {
    let mut words: Vec<(f32, &str)> = [
        (a.energy, "энергичное", "спокойное"),
        (a.brightness, "светлое", "мрачное"),
        (a.steadiness, "ровное", "переменчивое"),
    ]
    .into_iter()
    .filter(|(x, _, _)| x.abs() >= WORD_LEAN)
    .map(|(x, up, down)| (x.abs(), if x > 0.0 { up } else { down }))
    .collect();
    words.sort_by(|x, y| y.0.total_cmp(&x.0));
    match words.as_slice() {
        [] => None,
        [(_, a)] => Some((*a).to_owned()),
        [(_, a), (_, b), ..] => Some(format!("{a} и {b}")),
    }
}

impl StyleMix {
    /// The radio seed that plays this mix.
    pub fn seed(&self) -> RadioSeed {
        RadioSeed::Style {
            index: self.index,
            centroid: self.centroid.clone(),
            artists: self.artists.clone(),
        }
    }
}

/// The k for `n` listened tracks: one more cluster per 400 tracks from 3,
/// at most 6, and no more than `n / MIN_CLUSTER`.
pub fn k_for(n: usize) -> usize {
    (MIN_K + n / 400).min(MAX_K).min(n / MIN_CLUSTER)
}

fn mean_of(points: &[Vector], members: impl Iterator<Item = usize>) -> Option<Vector> {
    let mut sum = [0.0f32; DIM];
    let mut c = 0usize;
    for i in members {
        for (s, x) in sum.iter_mut().zip(&points[i]) {
            *s += x;
        }
        c += 1;
    }
    (c > 0).then(|| sum.map(|s| s / c as f32))
}

fn nearest(centroids: &[Vector], p: &Vector) -> usize {
    let mut best = (f32::INFINITY, 0);
    for (k, c) in centroids.iter().enumerate() {
        let d = euclid(p, c);
        if d < best.0 {
            best = (d, k);
        }
    }
    best.1
}

impl Recs<'_> {
    /// The style mixes; `previous` are the centroids of the last run (any
    /// count, in [`StyleMix::index`] order), `seed` seeds the new ones.
    pub fn style_mixes(&self, previous: &[Vec<f32>], seed: u64) -> Vec<StyleMix> {
        // Listened, analysed, not disliked.
        let mut members: Vec<usize> = Vec::new();
        let mut points: Vec<Vector> = Vec::new();
        for (i, t) in self.catalog.tracks().iter().enumerate() {
            if self.taste.affinity_at(i) > 0.0
                && !self.blocked(i)
                && let Some(v) = self.embedded(t.id)
            {
                members.push(i);
                points.push(v);
            }
        }
        let n = points.len();
        let k = k_for(n);
        if k < 2 {
            return Vec::new();
        }

        // Start: the previous centres, then k-means++.
        let mut rng = Rng::derive(seed, 0x57E1E);
        let mut centroids: Vec<Vector> = previous
            .iter()
            .filter_map(|c| <[f32; DIM]>::try_from(c.as_slice()).ok())
            .take(k)
            .collect();
        if centroids.is_empty() {
            centroids.push(points[rng.below(n)]);
        }
        while centroids.len() < k {
            let d2: Vec<(usize, f64)> = points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let d = centroids.iter().map(|c| euclid(p, c)).fold(f32::INFINITY, f32::min);
                    (i, f64::from(d * d))
                })
                .collect();
            match rng.weighted_sample(&d2, 1).first() {
                Some(&i) => centroids.push(points[i]),
                None => break,
            }
        }

        // Lloyd.
        let mut label = vec![0usize; n];
        for _ in 0..ITERATIONS {
            let mut changed = false;
            for (i, p) in points.iter().enumerate() {
                let l = nearest(&centroids, p);
                if l != label[i] {
                    label[i] = l;
                    changed = true;
                }
            }
            for (c, centroid) in centroids.iter_mut().enumerate() {
                if let Some(m) = mean_of(&points, (0..n).filter(|&i| label[i] == c)) {
                    *centroid = m;
                }
            }
            if !changed {
                break;
            }
        }

        // Whole artists into their majority cluster (by main artist).
        let main = |p: usize| self.info(members[p]).main_artist();
        let mut votes: HashMap<ArtistId, Vec<usize>> = HashMap::new();
        for (p, &l) in label.iter().enumerate() {
            if let Some(a) = main(p) {
                votes.entry(a).or_insert_with(|| vec![0; centroids.len()])[l] += 1;
            }
        }
        let home: HashMap<ArtistId, usize> = votes
            .iter()
            .map(|(a, v)| {
                let best = (0..v.len()).max_by(|x, y| v[*x].cmp(&v[*y]).then(y.cmp(x))).unwrap_or(0);
                (*a, best)
            })
            .collect();
        for (p, l) in label.iter_mut().enumerate() {
            if let Some(a) = main(p) {
                *l = home[&a];
            }
        }

        // Merge small clusters and near twins.
        let mut alive: Vec<bool> = vec![true; centroids.len()];
        loop {
            let mut size = vec![0usize; centroids.len()];
            for &l in &label {
                size[l] += 1;
            }
            for (c, centroid) in centroids.iter_mut().enumerate() {
                alive[c] = size[c] > 0;
                if let Some(m) = mean_of(&points, (0..n).filter(|&i| label[i] == c)) {
                    *centroid = m;
                }
            }
            let live: Vec<usize> = (0..centroids.len()).filter(|&c| alive[c]).collect();
            if live.len() < 2 {
                break;
            }
            let radius: Vec<f32> = (0..centroids.len())
                .map(|c| {
                    let (s, k) = (0..n)
                        .filter(|&i| label[i] == c)
                        .fold((0.0f32, 0usize), |(s, k), i| {
                            let d = euclid(&points[i], &centroids[c]);
                            (s + d * d, k + 1)
                        });
                    if k == 0 { 0.0 } else { (s / k as f32).sqrt() }
                })
                .collect();
            let closest_to = |c: usize| {
                live.iter()
                    .copied()
                    .filter(|&o| o != c)
                    .min_by(|a, b| {
                        euclid(&centroids[c], &centroids[*a]).total_cmp(&euclid(&centroids[c], &centroids[*b])).then(a.cmp(b))
                    })
                    .expect("two live clusters")
            };
            let small = live.iter().copied().filter(|&c| size[c] < MIN_CLUSTER).min_by_key(|&c| (size[c], c));
            let pair = if let Some(c) = small {
                Some((c, closest_to(c)))
            } else {
                let mut best: Option<(f32, usize, usize)> = None;
                for (x, &a) in live.iter().enumerate() {
                    for &b in &live[x + 1..] {
                        let ratio = euclid(&centroids[a], &centroids[b]) / ((radius[a] + radius[b]) / 2.0).max(1e-6);
                        if ratio < MERGE_RATIO && best.is_none_or(|(r, _, _)| ratio < r) {
                            best = Some((ratio, b, a));
                        }
                    }
                }
                best.map(|(_, from, into)| (from, into))
            };
            let Some((from, into)) = pair else { break };
            for l in label.iter_mut() {
                if *l == from {
                    *l = into;
                }
            }
        }

        let live: Vec<usize> = (0..centroids.len()).filter(|&c| alive[c]).collect();
        if live.len() < 2 {
            return Vec::new();
        }
        live.iter()
            .enumerate()
            .map(|(index, &c)| {
                let ps: Vec<usize> = (0..n).filter(|&p| label[p] == c).collect();
                let mut artists: Vec<ArtistId> = Vec::new();
                let mut covers: HashMap<ReleaseId, f32> = HashMap::new();
                for &p in &ps {
                    let i = members[p];
                    for a in &self.info(i).artists {
                        if !artists.contains(a) {
                            artists.push(*a);
                        }
                    }
                    if let Some(r) = self.info(i).release {
                        *covers.entry(r).or_default() += self.taste.affinity_at(i);
                    }
                }
                artists.sort_by(|a, b| self.taste.artist_affinity(*b).total_cmp(&self.taste.artist_affinity(*a)).then(a.cmp(b)));
                let mut covers: Vec<(ReleaseId, f32)> = covers.into_iter().collect();
                covers.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                let sounds: Vec<Axes> = ps
                    .iter()
                    .filter_map(|&p| self.index.get(self.info(members[p]).id))
                    .map(|v| Axes::of(&self.norm.z(v)))
                    .collect();
                let k = sounds.len().max(1) as f32;
                let axes = Axes {
                    energy: sounds.iter().map(|x| x.energy).sum::<f32>() / k,
                    brightness: sounds.iter().map(|x| x.brightness).sum::<f32>() / k,
                    steadiness: sounds.iter().map(|x| x.steadiness).sum::<f32>() / k,
                };
                StyleMix {
                    index,
                    centroid: centroids[c].to_vec(),
                    name_artists: artists.iter().take(2).copied().collect(),
                    covers: covers.into_iter().take(COVERS).map(|(r, _)| r).collect(),
                    artists,
                    size: ps.len(),
                    axes,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::VectorIndex;
    use crate::pick::Item;
    use crate::radio::{RadioParams, RadioState};
    use crate::taste::{Catalog, Taste, TasteInput};
    use crate::test_util::*;
    use nikgob_core::model::TrackId;

    /// Two genres: artists 0..6 around +1 in every feature, artists 6..12
    /// around −1; 15 tracks each, every one liked. Artist 0 likes most.
    fn genres(extra_noise_artist: bool) -> (Catalog, VectorIndex<TrackId>, TasteInput) {
        let mut rng = Rng::new(12);
        let mut tracks = Vec::new();
        let mut idx = VectorIndex::new();
        let mut likes = Vec::new();
        for a in 0..12u32 {
            for k in 0..15u128 {
                let i = u128::from(a) * 100 + k;
                tracks.push(info(i, &[a], Some(5000 + u128::from(a) * 10 + k / 5)));
                let centre = if a < 6 { 1.0 } else { -1.0 };
                let mut v: Vector = std::array::from_fn(|_| centre + (rng.f32() - 0.5) * 0.3);
                // One artist of the first genre has two tracks that sound like
                // the second: the artist still goes whole.
                if extra_noise_artist && a == 1 && k < 2 {
                    v = std::array::from_fn(|_| -1.0 + (rng.f32() - 0.5) * 0.3);
                }
                idx.insert(tid(i), v);
                likes.push(tid(i));
                if a == 0 || a == 7 {
                    likes.push(tid(i));
                }
            }
        }
        // Artists 0 and 7 also have playlist entries: more affinity.
        let playlist_entries = tracks
            .iter()
            .filter(|t: &&crate::taste::TrackInfo| t.artists[0] == aid(0) || t.artists[0] == aid(7))
            .map(|t| (t.id, 0))
            .collect();
        (Catalog::new(tracks), idx, TasteInput { likes, playlist_entries, ..Default::default() })
    }

    #[test]
    fn two_genres_make_two_mixes() {
        let (cat, idx, input) = genres(true);
        let taste = Taste::build(&cat, &input, 0);
        let recs = Recs::new(&cat, &taste, &idx);
        let mixes = recs.style_mixes(&[], 1);
        assert_eq!(mixes.len(), 2, "{mixes:?}");
        for m in &mixes {
            assert_eq!(m.size, 90);
            let first_genre = m.artists.iter().filter(|a| a.0.as_u128() < 6).count();
            assert!(first_genre == 0 || first_genre == 6, "a style is one genre: {:?}", m.artists);
            assert_eq!(m.artists.len(), 6);
            assert_eq!(m.name_artists.len(), 2);
            assert!(m.name_artists[0] == aid(0) || m.name_artists[0] == aid(7));
            assert_eq!(m.covers.len(), 4);
        }
        // Warm start: the same mixes in the same order, whatever the seed.
        let prev: Vec<Vec<f32>> = mixes.iter().map(|m| m.centroid.clone()).collect();
        for seed in 2..6 {
            let again = recs.style_mixes(&prev, seed);
            assert_eq!(again.len(), 2);
            for (a, b) in again.iter().zip(&mixes) {
                assert_eq!(a.artists, b.artists);
            }
        }
        // A mix plays its style.
        let mut radio = RadioState::new(mixes[0].seed());
        let picks = radio.extend(&recs, 12, &RadioParams { new_share: 0.0, seed: 1 });
        assert_eq!(picks.len(), 12);
        for p in picks {
            let Item::Track(id) = p.item else { panic!() };
            let artist = cat.get(id).unwrap().artists[0];
            assert!(mixes[0].artists.contains(&artist));
        }
    }

    #[test]
    fn a_style_is_named_by_its_sound() {
        let ax = |energy, brightness, steadiness| Axes { energy, brightness, steadiness };
        assert_eq!(style_words(ax(0.05, -0.1, 0.0)), None);
        assert_eq!(style_words(ax(-0.6, -0.3, 0.1)).as_deref(), Some("спокойное и мрачное"));
        assert_eq!(style_words(ax(0.1, 0.5, 0.0)).as_deref(), Some("светлое"));
        assert_eq!(style_words(ax(0.3, 0.0, -0.9)).as_deref(), Some("переменчивое и энергичное"));
        // The two genres of the library sound opposite.
        let (cat, idx, input) = genres(false);
        let taste = Taste::build(&cat, &input, 0);
        let mixes = Recs::new(&cat, &taste, &idx).style_mixes(&[], 1);
        let words: Vec<Option<String>> = mixes.iter().map(|m| style_words(m.axes)).collect();
        assert_eq!(words.len(), 2);
        assert!(words.iter().all(Option::is_some) && words[0] != words[1], "{words:?}");
    }

    #[test]
    fn too_little_listening_makes_no_mixes() {
        assert_eq!(k_for(10), 0);
        assert_eq!(k_for(60), 2);
        assert_eq!(k_for(180), 3);
        assert_eq!(k_for(1_000), 5);
        assert_eq!(k_for(10_000), 6);
        let (cat, idx, _) = genres(false);
        let taste = Taste::build(&cat, &TasteInput::default(), 0);
        assert!(Recs::new(&cat, &taste, &idx).style_mixes(&[], 1).is_empty());
    }
}
