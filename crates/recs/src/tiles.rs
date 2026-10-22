//! The bento's smaller tiles (docs/stage4.md §12.2): selections that play
//! as they are, each only when it has enough songs.
//!
//! * [`Tile::OnRepeat`] «На повторе»: the most completed in the last 30
//!   days (at least [`ON_REPEAT_MIN_COMPLETES`] completes each).
//! * [`Tile::TimeCapsule`] «Год назад»: played within 14 days of this day a
//!   year ago, most played first (imported history counts).
//! * [`Tile::Rediscover`] «Забытое любимое»: affinity of at least
//!   [`FAVOURITE_AFFINITY`], not played for [`REDISCOVERY_MS`].
//! * [`Tile::DeepCuts`] «Глубже у любимых»: tracks of the [`TOP_ARTISTS`]
//!   most played artists played once or never.
//! * [`Tile::FreshLikes`] «Свежие лайки»: liked in the last 30 days, newest
//!   first.
//!
//! Up to [`TILE_LEN`] tracks; fewer than [`TILE_MIN`] ([`FRESH_MIN`] for
//! fresh likes) and there is no tile. Disliked tracks never come. All but
//! the fresh likes keep the diversity rules in the smooth order; the fresh
//! likes stay newest first.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::ArtistId;
use serde::{Deserialize, Serialize};

use crate::daily::REDISCOVERY_MS;
use crate::pick::{Diversity, Item, Pick, Reason, Recs, Slot, arrange, select_diverse};
use crate::rng::Rng;
use crate::taste::ON_REPEAT_MS;

pub const TILE_LEN: usize = 30;
/// Fewer tracks than this, no tile.
pub const TILE_MIN: usize = 8;
/// «Свежие лайки» need this many.
pub const FRESH_MIN: usize = 5;
/// «На повторе»: completes of a track in the 30 days.
pub const ON_REPEAT_MIN_COMPLETES: u32 = 2;
/// «Забытое любимое»: a favourite has at least this affinity.
pub const FAVOURITE_AFFINITY: f32 = 1.0;
/// «Глубже у любимых»: this many most played artists…
pub const TOP_ARTISTS: usize = 10;
/// …and their tracks played at most this many times.
pub const DEEP_MAX_PLAYS: u32 = 1;

/// A tile of the bento.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tile {
    OnRepeat,
    TimeCapsule,
    Rediscover,
    DeepCuts,
    FreshLikes,
}

impl Tile {
    /// In the order the bento places them.
    pub const ALL: [Self; 5] = [Self::OnRepeat, Self::TimeCapsule, Self::Rediscover, Self::DeepCuts, Self::FreshLikes];

    const fn min(self) -> usize {
        match self {
            Self::FreshLikes => FRESH_MIN,
            _ => TILE_MIN,
        }
    }
}

impl Recs<'_> {
    /// The tile's tracks, or `None` when it has too few. `seed` varies the
    /// deep cuts from day to day.
    pub fn tile(&self, tile: Tile, seed: u64) -> Option<Vec<Pick>> {
        let n_cat = self.catalog.len();
        let ok = |i: usize| !self.blocked(i);
        let aff = |i: usize| self.taste.affinity_at(i);
        let now = self.now();
        // Candidates: position, reason, score (best first after sorting).
        let mut cands: Vec<(usize, Reason, f32)> = Vec::new();
        match tile {
            Tile::OnRepeat => {
                for i in (0..n_cat).filter(|&i| ok(i)) {
                    let completes = self.taste.recent_completes_at(i);
                    if completes >= ON_REPEAT_MIN_COMPLETES {
                        let score = completes as f32 + 0.01 * aff(i);
                        cands.push((i, Reason::OnRepeat { completes }, score));
                    }
                }
            }
            Tile::TimeCapsule => {
                for i in (0..n_cat).filter(|&i| ok(i)) {
                    let plays = self.taste.year_ago_plays_at(i);
                    if plays > 0 {
                        cands.push((i, Reason::YearAgo { plays }, plays as f32 + 0.01 * aff(i)));
                    }
                }
            }
            Tile::Rediscover => {
                for i in (0..n_cat).filter(|&i| ok(i) && aff(i) >= FAVOURITE_AFFINITY) {
                    // A like never played since is not forgotten unless it is old.
                    let last = self.taste.last_played_at(i).or(self.taste.liked_time_at(i));
                    if last.is_some_and(|t| now - t >= REDISCOVERY_MS) {
                        cands.push((i, Reason::Rediscovery, aff(i)));
                    }
                }
            }
            Tile::DeepCuts => {
                let mut plays: HashMap<ArtistId, u32> = HashMap::new();
                for i in 0..n_cat {
                    let p = self.taste.plays_at(i);
                    if p > 0 {
                        for a in &self.info(i).artists {
                            *plays.entry(*a).or_default() += p;
                        }
                    }
                }
                let mut top: Vec<(ArtistId, u32)> = plays.into_iter().filter(|(a, _)| !self.taste.is_disliked_artist(*a)).collect();
                top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                top.truncate(TOP_ARTISTS);
                let mut rng = Rng::derive(seed, 0xDEE9);
                let mut seen: HashSet<usize> = HashSet::new();
                for (rank, (a, _)) in top.iter().enumerate() {
                    for &i in self.catalog.of_artist(*a) {
                        let i = i as usize;
                        if ok(i) && self.taste.plays_at(i) <= DEEP_MAX_PLAYS && aff(i) >= 0.0 && seen.insert(i) {
                            // The top artists first, mixed a little by the day.
                            let score = -(rank as f32) / TOP_ARTISTS as f32 + rng.f32();
                            cands.push((i, Reason::DeepCut(*a), score));
                        }
                    }
                }
            }
            Tile::FreshLikes => {
                let mut fresh: Vec<(usize, i64)> = (0..n_cat)
                    .filter(|&i| ok(i) && self.taste.liked_at(i))
                    .filter_map(|i| Some((i, self.taste.liked_time_at(i)?)))
                    .filter(|(_, t)| now - t < ON_REPEAT_MS)
                    .collect();
                fresh.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                fresh.truncate(TILE_LEN);
                if fresh.len() < tile.min() {
                    return None;
                }
                return Some(
                    fresh
                        .into_iter()
                        .map(|(i, _)| Pick {
                            item: Item::Track(self.info(i).id),
                            reason: Reason::FreshLike,
                        })
                        .collect(),
                );
            }
        }
        if cands.len() < tile.min() {
            return None;
        }
        cands.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
        let slots: Vec<Slot> = cands.into_iter().map(|(i, reason, score)| self.slot(i, reason, score)).collect();
        let rules = Diversity::default();
        let picked = select_diverse(slots, TILE_LEN, &rules, &mut HashSet::new());
        Some(arrange(self, picked, &[], None, &rules).into_iter().map(Slot::pick).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DIM;
    use crate::index::VectorIndex;
    use crate::pick::assert_diverse;
    use crate::taste::{Catalog, DAY_MS, Play, Taste, TasteInput, YEAR_MS};
    use crate::test_util::*;
    use nikgob_core::model::{EventType, TrackId};

    const NOW: i64 = 2_000 * DAY_MS;

    fn complete(i: u128, ts: i64) -> Play {
        Play {
            track: tid(i),
            ts,
            kind: EventType::Complete,
            position_ms: None,
        }
    }

    fn num(p: &Pick) -> u128 {
        match &p.item {
            Item::Track(id) => id.0.as_u128(),
            Item::Stream(_) => u128::MAX,
        }
    }

    /// 300 tracks by 60 artists (5 each, own releases), random vectors.
    fn catalog() -> (Catalog, VectorIndex<TrackId>) {
        let cat = Catalog::new((0..300u128).map(|i| info(i, &[(i % 60) as u32], Some(9_000 + i))));
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut rng = Rng::new(5);
        for i in 0..300u128 {
            idx.insert(tid(i), std::array::from_fn(|_| rng.f32()));
        }
        (cat, idx)
    }

    fn ids(picks: &[Pick]) -> Vec<u128> {
        picks.iter().map(num).collect()
    }

    #[test]
    fn on_repeat_takes_the_most_completed_of_the_month() {
        let (cat, idx) = catalog();
        let mut events = Vec::new();
        // 0..40: completed 2..5 times this month; 40..50 once; 50..60 twice
        // but two months ago.
        for i in 0..40u128 {
            for k in 0..(2 + i % 4) {
                events.push(complete(i, NOW - (1 + k as i64) * DAY_MS));
            }
        }
        for i in 40..50u128 {
            events.push(complete(i, NOW - DAY_MS));
        }
        for i in 50..60u128 {
            events.push(complete(i, NOW - 60 * DAY_MS));
            events.push(complete(i, NOW - 61 * DAY_MS));
        }
        let input = TasteInput {
            events,
            disliked_tracks: vec![tid(3)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let picks = recs.tile(Tile::OnRepeat, 1).unwrap();
        assert_eq!(picks.len(), 30);
        assert!(ids(&picks).iter().all(|&i| i < 40 && i != 3), "{:?}", ids(&picks));
        // The ones completed 5 times are all in (3 is disliked).
        for i in (0..40u128).filter(|i| i % 4 == 3 && *i != 3) {
            assert!(ids(&picks).contains(&i), "{i}");
        }
        assert_diverse(&recs, &picks);
        // Seven repeats are not a tile.
        let events = (0..7u128).flat_map(|i| [complete(i, NOW - DAY_MS), complete(i, NOW - 2 * DAY_MS)]).collect();
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, NOW);
        assert!(Recs::new(&cat, &taste, &idx).tile(Tile::OnRepeat, 1).is_none());
    }

    #[test]
    fn a_year_ago_is_two_weeks_around_this_day() {
        let (cat, idx) = catalog();
        let year_ago = NOW - YEAR_MS;
        let mut events: Vec<Play> = (0..12u128).map(|i| complete(i, year_ago + (i as i64 - 6) * 2 * DAY_MS)).collect();
        events.push(complete(0, year_ago));
        // Outside the window, and this month.
        events.extend((20..40u128).map(|i| complete(i, year_ago - 20 * DAY_MS)));
        events.extend((40..60u128).map(|i| complete(i, NOW - DAY_MS)));
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let picks = recs.tile(Tile::TimeCapsule, 1).unwrap();
        let mut got = ids(&picks);
        got.sort_unstable();
        assert_eq!(got, (0..12).collect::<Vec<_>>());
        assert!(picks.iter().any(|p| p.reason == Reason::YearAgo { plays: 2 }));
        // Seven songs are not a tile.
        let events = (0..7u128).map(|i| complete(i, year_ago)).collect();
        let taste = Taste::build(&cat, &TasteInput { events, ..Default::default() }, NOW);
        assert!(Recs::new(&cat, &taste, &idx).tile(Tile::TimeCapsule, 1).is_none());
    }

    #[test]
    fn forgotten_favourites_are_loved_and_long_unplayed() {
        let (cat, idx) = catalog();
        let mut events = Vec::new();
        for i in 0..20u128 {
            // Loved 70 days ago (four completes, ~2.3 affinity).
            for k in 0..4 {
                events.push(complete(i, NOW - (70 + k) * DAY_MS));
            }
        }
        // 0 played again yesterday; 20..30 once 70 days ago (too little).
        events.push(complete(0, NOW - DAY_MS));
        events.extend((20..30u128).map(|i| complete(i, NOW - 70 * DAY_MS)));
        let input = TasteInput {
            events,
            // 40 liked long ago, 41 liked yesterday: neither played.
            likes: vec![tid(40), tid(41)],
            like_times: vec![(tid(40), NOW - 100 * DAY_MS), (tid(41), NOW - DAY_MS)],
            disliked_artists: vec![aid(5)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let picks = recs.tile(Tile::Rediscover, 1).unwrap();
        let got = ids(&picks);
        assert_eq!(got.len(), 19, "{got:?}");
        assert!(got.contains(&40) && !got.contains(&41));
        assert!(!got.contains(&0) && !got.contains(&5));
        assert!(got.iter().all(|&i| i < 20 || i == 40));
    }

    #[test]
    fn deep_cuts_dig_into_the_top_artists() {
        let (cat, idx) = catalog();
        let mut events = Vec::new();
        // Artists 0..12 by plays: artist a's track a (and a + 60) played
        // 30 − a times; artists 10 and 11 are below the top ten.
        for a in 0..12u128 {
            for k in 0..(30 - a as i64) {
                events.push(complete(a, NOW - (k + 1) * DAY_MS));
                events.push(complete(a + 60, NOW - (k + 1) * DAY_MS));
            }
        }
        // Artist 1's third track was played once: still a deep cut; its
        // fourth twice: not.
        events.push(complete(121, NOW - DAY_MS));
        events.push(complete(181, NOW - DAY_MS));
        events.push(complete(181, NOW - 2 * DAY_MS));
        let input = TasteInput {
            events,
            disliked_tracks: vec![tid(122)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let picks = recs.tile(Tile::DeepCuts, 7).unwrap();
        let got = ids(&picks);
        assert_eq!(got.len(), 28, "ten artists × three tracks, one played twice, one disliked: {got:?}");
        for i in &got {
            assert!(i % 60 < 10, "a top artist: {i}");
            assert!(*i >= 120, "barely played: {i}");
        }
        assert!(got.contains(&121) && !got.contains(&181) && !got.contains(&122));
        assert_diverse(&recs, &picks);
    }

    #[test]
    fn fresh_likes_are_newest_first() {
        let (cat, idx) = catalog();
        let likes: Vec<TrackId> = (0..40u128).map(tid).collect();
        // i liked i days ago: 0..30 are fresh; 7 disliked.
        let like_times = (0..40u128).map(|i| (tid(i), NOW - i as i64 * DAY_MS - 1)).collect();
        let input = TasteInput {
            likes,
            like_times,
            disliked_tracks: vec![tid(7)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let picks = recs.tile(Tile::FreshLikes, 1).unwrap();
        assert_eq!(ids(&picks), (0..30).filter(|i| *i != 7).collect::<Vec<_>>());
        assert!(picks.iter().all(|p| p.reason == Reason::FreshLike));
        // Four are not a tile; five are.
        for (n, some) in [(4, false), (5, true)] {
            let input = TasteInput {
                likes: (0..n).map(tid).collect(),
                like_times: (0..n).map(|i| (tid(i), NOW - DAY_MS)).collect(),
                ..Default::default()
            };
            let taste = Taste::build(&cat, &input, NOW);
            assert_eq!(Recs::new(&cat, &taste, &idx).tile(Tile::FreshLikes, 1).is_some(), some);
        }
    }

    #[test]
    fn no_history_no_tiles() {
        let (cat, idx) = catalog();
        let taste = Taste::build(&cat, &TasteInput::default(), NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        for t in Tile::ALL {
            assert!(recs.tile(t, 1).is_none(), "{t:?}");
        }
    }
}
