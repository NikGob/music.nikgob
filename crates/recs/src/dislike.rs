//! Dislikes, calibrated so they never break the recommendations
//! (docs/TODO.md §10).
//!
//! * A disliked track is out of every pick ([`crate::Taste::blocks`]): that
//!   is where a dislike weighs strongly.
//! * Its [`NEIGHBOURS`] nearest sound neighbours within [`RADIUS`] median
//!   distances stand a little lower, the nearer the more
//!   (`NEAR_WEIGHT · (1 − d / r)`), all the dislikes of one track together
//!   at most [`CAP`]: one dislike can never empty a region the user
//!   listens to, a few in one place only lean it.
//! * Nothing else moves: not the artist, not the album, not the world, not
//!   a whole region. An artist is left out only by its own «Не
//!   рекомендовать исполнителя».

use crate::analysis::Vector;
use crate::index::euclid;

/// Neighbours of a disliked track that stand lower…
pub const NEIGHBOURS: usize = 8;
/// …within this many median distances between tracks…
pub const RADIUS: f32 = 0.6;
/// …by up to this much at distance 0…
pub const NEAR_WEIGHT: f32 = 0.5;
/// …and never more than this much in all.
pub const CAP: f32 = 0.6;

/// How much lower each track stands for the dislikes near it: `emb` the
/// embedded vectors by catalog position, `disliked` the positions of the
/// disliked tracks, `scale` the median distance between tracks.
pub fn near_dislikes(emb: &[Option<Vector>], disliked: &[usize], scale: f32) -> Vec<f32> {
    let mut out = vec![0.0f32; emb.len()];
    let r = RADIUS * scale.max(1e-3);
    for &d in disliked {
        let Some(dv) = emb.get(d).and_then(Option::as_ref) else { continue };
        let mut near: Vec<(f32, usize)> = emb
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != d && !disliked.contains(i))
            .filter_map(|(i, v)| Some((euclid(v.as_ref()?, dv), i)))
            .filter(|(dist, _)| *dist < r)
            .collect();
        near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        for (dist, i) in near.into_iter().take(NEIGHBOURS) {
            out[i] += NEAR_WEIGHT * (1.0 - dist / r);
        }
    }
    for x in &mut out {
        *x = x.min(CAP);
    }
    out
}

/// The same for a vector outside the catalog (a new track's preview):
/// the disliked vectors `disliked` near it.
pub fn near_dislikes_of(v: &Vector, disliked: &[Vector], scale: f32) -> f32 {
    let r = RADIUS * scale.max(1e-3);
    let mut push = 0.0;
    for d in disliked {
        let dist = euclid(v, d);
        if dist < r {
            push += NEAR_WEIGHT * (1.0 - dist / r);
        }
    }
    push.min(CAP)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::model::tests::{T0, album_listener, input};
    use crate::model::{UserModel, replay, replay_with};
    use crate::rng::Rng;
    use crate::test_util::*;

    /// The nearer a neighbour, the lower; past the radius, nothing; only
    /// the nearest few; never more than the cap.
    #[test]
    fn neighbours_decay_and_the_weight_is_capped() {
        let mut emb: Vec<Option<Vector>> = (0..20).map(|k| Some(line(k as f32 * 0.1))).collect();
        emb.push(None);
        let near = near_dislikes(&emb, &[0], 1.0);
        assert_eq!(near[0], 0.0, "the disliked one is out, not lowered");
        assert!(near[1] > near[2] && near[2] > near[3] && near[3] > 0.0, "{near:?}");
        assert_eq!(near[7], 0.0, "past {RADIUS} median distances: {near:?}");
        assert_eq!(near[20], 0.0);
        assert!(near.iter().filter(|x| **x > 0.0).count() <= NEIGHBOURS);
        // Many dislikes around one track: capped.
        let many: Vec<usize> = (0..20).filter(|k| *k != 10).collect();
        let crowded = near_dislikes(&emb, &many, 10.0);
        assert!(crowded[10] <= CAP + 1e-6 && crowded[10] > CAP - 0.01, "{}", crowded[10]);
    }

    /// A dislike lowers its sound neighbours a little and nothing of its
    /// artist, album or world: the artist's other tracks and affinity are
    /// as they were, and a region keeps most of its songs on top.
    #[test]
    fn a_dislike_never_spreads_to_the_artist() {
        let (cat, idx, mut history) = album_listener(15, 3);
        let before = UserModel::build(&cat, &history, input(&idx), T0);
        // A loved track of album 4 (world 1).
        let target = tid(43);
        history.disliked_tracks = vec![target];
        history.dislike_neighbours = true;
        let after = UserModel::build(&cat, &history, input(&idx), T0);
        let pos = after.position(target).unwrap();
        assert!(after.taste.blocks(&cat.tracks()[pos]));
        let artist = cat.tracks()[pos].artists[0];
        assert_eq!(before.taste.artist_affinity(artist), after.taste.artist_affinity(artist), "the artist's affinity is the same");
        let lowered: Vec<usize> = (0..after.len()).filter(|&i| after.taste.dislike_near_at(i) > 0.0).collect();
        assert!(!lowered.is_empty() && lowered.len() <= NEIGHBOURS, "{lowered:?}");
        let same_artist: Vec<usize> = (0..after.len()).filter(|&i| i != pos && cat.tracks()[i].artists.contains(&artist)).collect();
        let untouched = same_artist.iter().filter(|&&i| after.taste.dislike_near_at(i) == 0.0).count();
        assert!(untouched * 2 > same_artist.len(), "most of the artist is untouched: {untouched} of {}", same_artist.len());
        // Off: only the track itself.
        history.dislike_neighbours = false;
        let off = UserModel::build(&cat, &history, input(&idx), T0);
        assert!((0..off.len()).all(|i| off.taste.dislike_near_at(i) == 0.0));
    }

    /// The replay after ten random dislikes: the next start is found about
    /// as often, and as much of the catalog is ever recommended.
    #[test]
    fn replay_holds_after_random_dislikes() {
        let (cat, idx, history) = album_listener(15, 2);
        let mut rng = Rng::new(11);
        let dislikes: Vec<_> = (0..10).map(|_| tid(rng.below(cat.len()) as u128)).collect();
        let skip: HashSet<_> = dislikes.iter().copied().collect();
        let base = replay_with(&cat, &history, input(&idx), &[], &skip);
        let with = replay_with(&cat, &history, input(&idx), &dislikes, &skip);
        let (a, b) = (base.get("all", "learned"), with.get("all", "learned"));
        assert_eq!(a.n, b.n);
        assert!(b.hit10 >= a.hit10 - 0.03, "hit@10 {} → {}", a.hit10, b.hit10);
        assert!(with.coverage >= base.coverage - 0.03, "coverage {} → {}", base.coverage, with.coverage);
        let plain = replay(&cat, &history, input(&idx));
        assert!(plain.get("all", "learned").n >= a.n);
    }
}

