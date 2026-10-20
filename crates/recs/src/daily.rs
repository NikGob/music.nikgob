//! «Твой микс» (docs/stage4.md §4): the hero, once a day.
//!
//! [`DAILY_LEN`] tracks: about half favourites not played for
//! [`FAVOURITE_REST_MS`], about 30% sound neighbours of favourites that were
//! played little ([`FEW_PLAYS`] or fewer), about 20% rediscoveries:
//! favourites not played for [`REDISCOVERY_MS`]. «Новое в радио» first takes
//! its share of the mix from the new-music candidates. A part short of
//! tracks is made up by the others, then by any favourite, then by anything.
//! With no history at all it is random library tracks. Always in the smooth
//! order under the diversity rules.

use std::collections::HashSet;

use crate::pick::{Diversity, Item, NewTrack, Pick, Reason, Recs, Slot, arrange, new_count, select_diverse, split};
use crate::rng::Rng;
use crate::taste::DAY_MS;

pub const DAILY_LEN: usize = 30;
/// A favourite of the mix rests this long after it was played.
pub const FAVOURITE_REST_MS: i64 = 3 * DAY_MS;
/// A rediscovery has not been played for this long.
pub const REDISCOVERY_MS: i64 = 60 * DAY_MS;
/// A sound neighbour of the mix was played at most this many times.
pub const FEW_PLAYS: u32 = 2;
/// Shares of favourites, neighbours and rediscoveries.
const SHARES: [f32; 3] = [0.5, 0.3, 0.2];
/// Favourites drawn from the best this many times the needed count.
const DRAW_FROM: usize = 4;
/// Favourites whose neighbours are looked for.
const NEIGHBOUR_SEEDS: usize = 8;
const NEIGHBOURS_PER_SEED: usize = 12;

#[derive(Clone, Copy, Debug)]
pub struct DailyParams {
    pub len: usize,
    /// «Новое в радио».
    pub new_share: f32,
    /// A new seed («Обновить», or the day).
    pub seed: u64,
}

impl Default for DailyParams {
    fn default() -> Self {
        Self {
            len: DAILY_LEN,
            new_share: 0.2,
            seed: 0,
        }
    }
}

impl Recs<'_> {
    /// The daily mix; `new` are new-music candidates, best first.
    pub fn daily_mix(&self, new: &[NewTrack], p: &DailyParams) -> Vec<Pick> {
        let mut rng = Rng::derive(p.seed, 0xDA11);
        let rules = Diversity::default();
        let mut taken: HashSet<Item> = HashSet::new();
        let now = self.now();
        let n_cat = self.catalog.len();

        let mut slots: Vec<Slot> = Vec::with_capacity(p.len);
        let want_new = new_count(p.len, p.new_share).min(new.len());
        let new_slots: Vec<Slot> = new.iter().enumerate().map(|(k, t)| self.new_slot(t, -(k as f32))).collect();
        slots.extend(select_diverse(new_slots, want_new, &rules, &mut taken));
        let rest = p.len - slots.len();

        if !self.taste.has_history() {
            let mut all: Vec<usize> = (0..n_cat).filter(|&i| !self.blocked(i)).collect();
            rng.shuffle(&mut all);
            let cands = all.into_iter().take(rest * 3).map(|i| self.slot(i, Reason::Random, rng.f32())).collect();
            slots.extend(select_diverse(cands, rest, &rules, &mut taken));
            return arrange(self, slots, &[], None, &rules).into_iter().map(Slot::pick).collect();
        }

        let parts = split(rest, &SHARES);
        let (want_fav, want_sim, want_red) = (parts[0], parts[1], parts[2]);
        let aff = |i: usize| self.taste.affinity_at(i);
        let resting = |i: usize| self.played_within(i, FAVOURITE_REST_MS);
        let mut favourites: Vec<usize> = (0..n_cat).filter(|&i| aff(i) > 0.0 && !self.blocked(i)).collect();
        favourites.sort_by(|a, b| aff(*b).total_cmp(&aff(*a)).then(a.cmp(b)));
        let part = self.taste.day_part();
        let weight = |i: usize| f64::from(aff(i)) * f64::from(1.0 + 0.5 * self.taste.daypart_bonus_at(i, part));

        // Rediscoveries: favourites (played or liked) long unplayed.
        let red_pool: Vec<(usize, f64)> = favourites
            .iter()
            .copied()
            .filter(|&i| self.taste.last_played_at(i).is_none_or(|t| now - t >= REDISCOVERY_MS))
            .take(want_red * DRAW_FROM * 2)
            .map(|i| (i, weight(i)))
            .collect();
        let red: Vec<Slot> = rng
            .weighted_sample(&red_pool, red_pool.len())
            .into_iter()
            .map(|i| self.slot(i, Reason::Rediscovery, 0.0))
            .collect();
        slots.extend(select_diverse(red, want_red, &rules, &mut taken));

        // Favourites at rest for three days.
        let fav_pool: Vec<(usize, f64)> = favourites
            .iter()
            .copied()
            .filter(|&i| !resting(i) && !taken.contains(&Item::Track(self.info(i).id)))
            .take(want_fav * DRAW_FROM)
            .map(|i| (i, weight(i)))
            .collect();
        let fav: Vec<Slot> = rng
            .weighted_sample(&fav_pool, fav_pool.len())
            .into_iter()
            .map(|i| self.slot(i, Reason::Favourite, 0.0))
            .collect();
        slots.extend(select_diverse(fav, want_fav, &rules, &mut taken));

        // Sound neighbours of favourites, played little.
        let seed_pool: Vec<(usize, f64)> = favourites
            .iter()
            .copied()
            .filter(|&i| self.index.contains(self.info(i).id))
            .take(NEIGHBOUR_SEEDS * 3)
            .map(|i| (i, weight(i)))
            .collect();
        let mut sim: Vec<Slot> = Vec::new();
        for s in rng.weighted_sample(&seed_pool, NEIGHBOUR_SEEDS) {
            let seed_id = self.info(s).id;
            let near = self.index.neighbours(seed_id, NEIGHBOURS_PER_SEED, |id| {
                self.catalog.position(id).is_some_and(|i| {
                    !self.blocked(i) && self.taste.plays_at(i) <= FEW_PLAYS && !resting(i) && aff(i) >= 0.0
                })
            });
            for (rank, (id, _)) in near.into_iter().enumerate() {
                let i = self.catalog.position(id).expect("filtered");
                // Interleave the seeds' lists: rank first, a little jitter.
                let score = -(rank as f32) + rng.f32() * 0.5;
                sim.push(self.slot(i, Reason::SimilarTo(seed_id), score));
            }
        }
        sim.sort_by(|a, b| b.score.total_cmp(&a.score));
        slots.extend(select_diverse(sim, want_sim, &rules, &mut taken));

        // Short: any favourite not played today, then anything.
        if slots.len() < p.len {
            let more: Vec<Slot> = favourites
                .iter()
                .copied()
                .filter(|&i| !self.played_within(i, DAY_MS))
                .map(|i| self.slot(i, Reason::Favourite, 0.0))
                .collect();
            let left = p.len - slots.len();
            slots.extend(select_diverse(more, left, &rules, &mut taken));
        }
        if slots.len() < p.len {
            let mut all: Vec<usize> = (0..n_cat).filter(|&i| !self.blocked(i)).collect();
            rng.shuffle(&mut all);
            let more = all.into_iter().map(|i| self.slot(i, Reason::Random, 0.0)).collect();
            let left = p.len - slots.len();
            slots.extend(select_diverse(more, left, &rules, &mut taken));
        }
        // The path starts at the strongest favourite.
        for s in &mut slots {
            if let Item::Track(id) = s.item
                && let Some(i) = self.catalog.position(id)
            {
                s.score = aff(i);
            }
        }
        arrange(self, slots, &[], None, &rules).into_iter().map(Slot::pick).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DIM;
    use crate::index::VectorIndex;
    use crate::pick::assert_diverse;
    use crate::taste::{Catalog, Play, Taste, TasteInput};
    use crate::test_util::*;
    use nikgob_core::model::{EventType, TrackId};
    use nikgob_core::streaming::{RemoteId, Service};

    const NOW: i64 = 2_000 * DAY_MS;

    /// 400 tracks by 100 artists, 4 albums of one track... each track its
    /// own release; tracks 0..100 favourites (completed 5 times, 10..40 days
    /// ago), 100..130 favourites last played 100 days ago, 300 disliked.
    fn library() -> (Catalog, VectorIndex<TrackId>, TasteInput) {
        let cat = Catalog::new((0..400u128).map(|i| info(i, &[(i % 100) as u32], Some(10_000 + i))));
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut rng = Rng::new(3);
        for i in 0..400u128 {
            idx.insert(tid(i), std::array::from_fn(|_| rng.f32()));
        }
        let mut events = Vec::new();
        for i in 0..100u128 {
            for k in 0..5 {
                events.push(Play {
                    track: tid(i),
                    ts: NOW - (10 + (i as i64 % 30)) * DAY_MS - k * 2 * DAY_MS,
                    kind: EventType::Complete,
                    position_ms: None,
                });
            }
        }
        for i in 100..130u128 {
            for k in 0..4 {
                events.push(Play {
                    track: tid(i),
                    ts: NOW - 100 * DAY_MS - k * 2 * DAY_MS,
                    kind: EventType::Complete,
                    position_ms: None,
                });
            }
        }
        // Track 0 just played: resting.
        events.push(Play {
            track: tid(0),
            ts: NOW - DAY_MS,
            kind: EventType::Complete,
            position_ms: None,
        });
        let input = TasteInput {
            events,
            disliked_tracks: vec![tid(300)],
            ..Default::default()
        };
        (cat, idx, input)
    }

    fn num(p: &Pick) -> u128 {
        match &p.item {
            Item::Track(id) => id.0.as_u128(),
            Item::Stream(_) => u128::MAX,
        }
    }

    #[test]
    fn the_daily_mix_has_its_parts() {
        let (cat, idx, input) = library();
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let new: Vec<NewTrack> = (0..10)
            .map(|k| NewTrack {
                id: RemoteId::new(Service::Deezer, format!("{k}")),
                artist: format!("someone {k}"),
                album: None,
                vector: None,
            })
            .collect();
        let mix = recs.daily_mix(&new, &DailyParams { len: 30, new_share: 0.2, seed: 1 });
        assert_eq!(mix.len(), 30);
        let count = |r: fn(&Reason) -> bool| mix.iter().filter(|p| r(&p.reason)).count();
        assert_eq!(count(|r| *r == Reason::New), 6);
        assert_eq!(count(|r| *r == Reason::Favourite), 12);
        assert_eq!(count(|r| matches!(r, Reason::SimilarTo(_))), 7);
        assert_eq!(count(|r| *r == Reason::Rediscovery), 5);
        for p in &mix {
            match p.reason {
                Reason::Favourite => assert!(num(p) < 100 && num(p) != 0, "{}", num(p)),
                Reason::Rediscovery => assert!((100..130).contains(&num(p))),
                Reason::SimilarTo(_) => assert!(num(p) >= 130),
                _ => {}
            }
            assert_ne!(num(p), 300, "disliked");
        }
        assert_diverse(&recs, &mix);
        // Deterministic for a seed, different for another.
        assert_eq!(mix, recs.daily_mix(&new, &DailyParams { len: 30, new_share: 0.2, seed: 1 }));
        assert_ne!(mix, recs.daily_mix(&new, &DailyParams { len: 30, new_share: 0.2, seed: 2 }));
        // Without new music the library fills the mix.
        let plain = recs.daily_mix(&[], &DailyParams { len: 30, new_share: 0.2, seed: 1 });
        assert_eq!(plain.len(), 30);
        assert!(plain.iter().all(|p| matches!(p.item, Item::Track(_))));
    }

    #[test]
    fn an_empty_history_is_random_and_smooth() {
        let (cat, idx, _) = library();
        let input = TasteInput {
            disliked_artists: vec![aid(7)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let mix = recs.daily_mix(&[], &DailyParams { len: 30, new_share: 0.0, seed: 4 });
        assert_eq!(mix.len(), 30);
        assert!(mix.iter().all(|p| p.reason == Reason::Random));
        assert!(mix.iter().all(|p| num(p) % 100 != 7), "a disliked artist never comes");
        assert_diverse(&recs, &mix);
        let empty = Catalog::default();
        let taste = Taste::build(&empty, &TasteInput::default(), NOW);
        let idx = VectorIndex::new();
        assert!(Recs::new(&empty, &taste, &idx).daily_mix(&[], &DailyParams::default()).is_empty());
    }
}
