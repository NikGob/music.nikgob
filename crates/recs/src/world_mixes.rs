//! Mixes inside a world (recs-v2 §8): what a world's page offers beside its
//! albums, built when the page opens (a few milliseconds, the world's own
//! tracks only).
//!
//! * **«Лучшее»** ([`WorldMixKind::Best`]): the world's tracks with a
//!   history by `aff_long − 0.5·p_skip`, [`BEST_LEN`] of them; with too
//!   little history the first track of each album and the songs of the
//!   world (its themes) fill it, from [`BEST_HEARD`] tracks.
//! * **«Ещё не слышал»** ([`WorldMixKind::Unheard`]): the tracks never
//!   played, nearest the centre of what was liked or heard out in the world,
//!   less the chance of an early skip, [`UNHEARD_LEN`] of them; none before
//!   anything of the world is loved.
//! * **The world's mood** ([`WorldMixKind::Calm`], [`WorldMixKind::Moving`]):
//!   its calm and its lively side by the energy of its own tracks, each
//!   from [`MOOD_MIN`] tracks.
//! * **«В дорогу»** ([`WorldMixKind::Road`], §8.4): from the world's core
//!   (its liked tracks and the ones heard out, else any it has affinity for,
//!   else all) to what sounds like it in other worlds of the same taste
//!   profiles, one world a third at most.
//!
//! A mix of one world keeps an album to three running and an artist apart
//! where it can; its order follows the arc of the part of the day
//! ([`crate::mixes::Arc`]), the calm side settling, the lively one rising.
//! Interludes are left out, as everything blocked or disliked.

use std::collections::HashSet;

use nikgob_core::model::{EventContext, TrackId};

use crate::analysis::{DIM, Vector};
use crate::index::euclid;
use crate::mixes::Arc;
use crate::model::UserModel;
use crate::pick::Recs;
use crate::session::Session;
use crate::taste::DayPart;

/// «Лучшее»: this many tracks…
pub const BEST_LEN: usize = 15;
/// …from this many with a history (else the albums' first tracks join).
const BEST_HEARD: usize = 8;
/// «Ещё не слышал»: this many.
pub const UNHEARD_LEN: usize = 12;
/// A side of the world's mood: this many tracks at most, from this many.
const MOOD_LEN: usize = 15;
pub const MOOD_MIN: usize = 12;
/// The calm side is the world's tracks below this percentile of its energy,
/// the lively one above `1 −` it.
const MOOD_SIDE: f32 = 0.4;
/// «В дорогу»: this many, one world this share at most.
pub const ROAD_LEN: usize = 15;
const ROAD_WORLD_SHARE: f32 = 0.34;
/// The core of a world: liked, or with this much affinity.
const CORE_AFFINITY: f32 = 1.5;
/// Shorter than this is an interlude.
const INTERLUDE_MS: u32 = 75_000;
/// The order: an album at most this many running.
const ALBUM_RUN: usize = 3;

/// What a mix of a world is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldMixKind {
    Best,
    Unheard,
    Calm,
    Moving,
    Road,
}

/// One mix of a world (catalog positions, in the order they play).
#[derive(Clone, Debug, PartialEq)]
pub struct WorldMix {
    pub kind: WorldMixKind,
    pub tracks: Vec<usize>,
    /// «Ещё не слышал»: the loved track of the world nearest its first.
    pub near: Option<usize>,
}

impl Recs<'_> {
    /// The mixes of `world` ([`crate::world_token`]) at the part of the day
    /// `part`, in the order the page shows them (see the module docs).
    /// None without the user model.
    pub fn world_mixes(&self, world: u64, part: DayPart) -> Vec<WorldMix> {
        let Some(m) = self.model else { return Vec::new() };
        let now = m.now();
        let m0 = m.moment(&Session::default(), now);
        let pickable = |i: usize| {
            let t = self.info(i);
            !self.blocked(i) && t.duration_ms.is_none_or(|d| d >= INTERLUDE_MS) && self.taste.dislike_near_at(i) < 1.0
        };
        let own: Vec<usize> = (0..self.catalog.len()).filter(|&i| self.info(i).world == Some(world) && pickable(i)).collect();
        if own.is_empty() {
            return Vec::new();
        }
        let p_skip = |i: usize| m.p_skip(&m0, i, EventContext::HomeSection);
        let loved = |i: usize| m.tracks[i].liked(now) || m.tracks[i].completes > 0;
        let arc = Arc::of(part);
        let mut out = Vec::new();

        // «Лучшее».
        let mut heard: Vec<(f32, usize)> =
            own.iter().copied().filter(|&i| m.tracks[i].starts > 0).map(|i| (m.tracks[i].aff_long(now) - 0.5 * p_skip(i), i)).collect();
        heard.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut best: Vec<usize> = heard.iter().map(|x| x.1).take(BEST_LEN).collect();
        if best.len() < BEST_LEN {
            // Little history: the albums' first tracks and the world's songs.
            let mut firsts: Vec<usize> = own.clone();
            firsts.sort_by_key(|&i| (self.info(i).position, i));
            let mut seen_albums: HashSet<u64> = HashSet::new();
            let opening = firsts.into_iter().filter(|&i| m.release_at(i).is_none_or(|r| seen_albums.insert(r)));
            let songs = own.iter().copied().filter(|&i| m.song_type_at(i).vocal());
            for i in opening.chain(songs) {
                if best.len() >= BEST_LEN {
                    break;
                }
                if !best.contains(&i) {
                    best.push(i);
                }
            }
        }
        if best.len() >= BEST_HEARD.min(own.len()) && !best.is_empty() {
            out.push(WorldMix { kind: WorldMixKind::Best, tracks: order(self, m, &best, arc), near: None });
        }

        // «Ещё не слышал».
        let core: Vec<Vector> = own.iter().copied().filter(|&i| loved(i)).filter_map(|i| m.embedded_at(i).copied()).collect();
        if let Some(centre) = mean(&core) {
            let scale = m.scale().max(1e-3);
            let mut fresh: Vec<(f32, usize)> = own
                .iter()
                .copied()
                .filter(|&i| m.tracks[i].starts == 0)
                .filter_map(|i| Some((-euclid(m.embedded_at(i)?, &centre) / scale - p_skip(i), i)))
                .collect();
            fresh.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            let tracks: Vec<usize> = fresh.into_iter().take(UNHEARD_LEN).map(|x| x.1).collect();
            if tracks.len() >= UNHEARD_LEN.min(MOOD_MIN) / 2 {
                let first = m.embedded_at(tracks[0]).copied();
                let near = first.and_then(|f| {
                    own.iter()
                        .copied()
                        .filter(|&i| loved(i))
                        .filter_map(|i| Some((euclid(m.embedded_at(i)?, &f), i)))
                        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                        .map(|x| x.1)
                });
                out.push(WorldMix { kind: WorldMixKind::Unheard, tracks: keep_albums(self, &tracks), near });
            }
        }

        // The world's mood: its calm and its lively side.
        let mut energy: Vec<(f32, usize)> = own.iter().copied().filter_map(|i| Some((m.energy_at(i)?, i))).collect();
        energy.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let side = (energy.len() as f32 * MOOD_SIDE) as usize;
        if side >= MOOD_MIN {
            let value = |i: usize| m.tracks[i].aff_long(now) - 0.5 * p_skip(i);
            for (kind, part) in [(WorldMixKind::Calm, &energy[..side]), (WorldMixKind::Moving, &energy[energy.len() - side..])] {
                let mut by: Vec<usize> = part.iter().map(|x| x.1).collect();
                by.sort_by(|a, b| value(*b).total_cmp(&value(*a)).then(a.cmp(b)));
                by.truncate(MOOD_LEN);
                let arc = if kind == WorldMixKind::Calm { Arc::Night } else { Arc::Day };
                out.push(WorldMix { kind, tracks: order(self, m, &by, arc), near: None });
            }
        }

        // «В дорогу».
        if let Some(road) = road(self, m, world, &own) {
            out.push(WorldMix { kind: WorldMixKind::Road, tracks: road, near: None });
        }
        out
    }
}

/// The mean of `vs`.
fn mean(vs: &[Vector]) -> Option<Vector> {
    if vs.is_empty() {
        return None;
    }
    let mut c = [0.0f32; DIM];
    for v in vs {
        for (a, b) in c.iter_mut().zip(v) {
            *a += b / vs.len() as f32;
        }
    }
    Some(c)
}

/// «В дорогу» of `world` (its tracks `own`): its core's best loved track,
/// then the nearest to the core's sound of other worlds of the core's
/// profiles, at most a third of one world, in the smooth order.
fn road(recs: &Recs<'_>, m: &UserModel, world: u64, own: &[usize]) -> Option<Vec<usize>> {
    let now = m.now();
    let core_of = |keep: &dyn Fn(usize) -> bool| -> Vec<usize> { own.iter().copied().filter(|&i| keep(i) && m.embedded_at(i).is_some()).collect() };
    let mut core = core_of(&|i| m.tracks[i].liked(now) || m.tracks[i].aff_long(now) >= CORE_AFFINITY || recs.taste.affinity_at(i) >= CORE_AFFINITY);
    if core.is_empty() {
        core = core_of(&|i| recs.taste.affinity_at(i) > 0.0);
    }
    if core.is_empty() {
        core = core_of(&|_| true);
    }
    let centre = mean(&core.iter().filter_map(|&i| m.embedded_at(i).copied()).collect::<Vec<_>>())?;
    let profiles: HashSet<usize> = core.iter().filter_map(|&i| m.profile_at(i)).collect();
    let scale = m.scale().max(1e-3);
    let mut cands: Vec<(f32, usize)> = (0..recs.catalog.len())
        .filter(|&i| {
            let t = recs.info(i);
            t.world != Some(world)
                && !recs.blocked(i)
                && t.duration_ms.is_none_or(|d| d >= INTERLUDE_MS)
                && (profiles.is_empty() || m.profile_at(i).is_some_and(|j| profiles.contains(&j)))
        })
        .filter_map(|i| Some((-euclid(m.embedded_at(i)?, &centre) / scale + 0.3 * (recs.taste.affinity_at(i) / 3.0).tanh() - recs.taste.dislike_near_at(i), i)))
        .collect();
    cands.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    cands.truncate(4 * ROAD_LEN);
    let start = core.iter().copied().max_by(|a, b| m.tracks[*a].aff_long(now).total_cmp(&m.tracks[*b].aff_long(now)).then(b.cmp(a)))?;
    let ids: Vec<(TrackId, f32)> = cands.iter().map(|&(s, i)| (recs.info(i).id, s)).collect();
    let rest = recs.mix_of(&ids, ROAD_LEN - 1, ROAD_WORLD_SHARE);
    if rest.len() < MOOD_MIN - 1 {
        return None;
    }
    Some(std::iter::once(start).chain(rest.into_iter().filter_map(|t| recs.catalog.position(t))).collect())
}

/// `tracks` as they are, an album never more than [`ALBUM_RUN`] running
/// (a track that would make it so waits for the next place it may take).
fn keep_albums(recs: &Recs<'_>, tracks: &[usize]) -> Vec<usize> {
    let mut left: Vec<usize> = tracks.to_vec();
    let mut out: Vec<usize> = Vec::with_capacity(tracks.len());
    while !left.is_empty() {
        let k = (0..left.len()).find(|&k| !runs(recs, &out, left[k])).unwrap_or(0);
        out.push(left.remove(k));
    }
    out
}

/// `i` after `out` makes an album [`ALBUM_RUN`] + 1 running, or follows its
/// own artist.
fn runs(recs: &Recs<'_>, out: &[usize], i: usize) -> bool {
    let t = recs.info(i);
    let album = t.release.is_some() && out.len() >= ALBUM_RUN && out[out.len() - ALBUM_RUN..].iter().all(|&o| recs.info(o).release == t.release);
    let artist = out.last().is_some_and(|&o| recs.info(o).artists.first() == t.artists.first() && !t.artists.is_empty());
    album || artist
}

/// `tracks` of one world in the order of `arc` (in the energy percentiles
/// of the tracks themselves), each next the nearest in sound among those
/// near its place on the arc, the album and artist rules of
/// [`keep_albums`] kept where they can be.
fn order(recs: &Recs<'_>, m: &UserModel, tracks: &[usize], arc: Arc) -> Vec<usize> {
    let n = tracks.len();
    let Some(targets) = arc.targets(n) else { return keep_albums(recs, tracks) };
    let energies: Vec<f32> = tracks.iter().filter_map(|&i| m.energy_at(i)).collect();
    let pct = crate::listening::pct_rank(&energies);
    let mut k = 0;
    let at: Vec<f32> = tracks
        .iter()
        .map(|&i| {
            if m.energy_at(i).is_some() {
                k += 1;
                pct[k - 1]
            } else {
                0.5
            }
        })
        .collect();
    let scale = m.scale().max(1e-3);
    let mut left: Vec<usize> = (0..n).collect();
    let mut out: Vec<usize> = Vec::with_capacity(n);
    for target in targets {
        let cost = |x: usize| {
            let i = tracks[x];
            let jump = match (out.last().and_then(|&o| m.embedded_at(o)), m.embedded_at(i)) {
                (Some(a), Some(b)) => euclid(a, b) / scale,
                _ => 1.0,
            };
            2.0 * (at[x] - target).abs() + 0.5 * jump + if runs(recs, &out, i) { 10.0 } else { 0.0 }
        };
        let Some(best) = (0..left.len()).min_by(|a, b| cost(left[*a]).total_cmp(&cost(left[*b])).then(left[*a].cmp(&left[*b]))) else { break };
        out.push(tracks[left.remove(best)]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::VectorIndex;
    use crate::model::ModelInput;
    use crate::taste::{Catalog, Play, TasteInput};
    use crate::test_util::*;
    use nikgob_core::model::EventType;

    const T0: i64 = 20_000 * crate::taste::DAY_MS;

    /// One world of 60 tracks on five albums along a line of sound, the
    /// first ten liked and heard out, the rest never played; another world
    /// of 60 around it.
    fn world() -> (Catalog, VectorIndex<TrackId>, TasteInput) {
        let mut tracks = Vec::new();
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        for i in 0..120u128 {
            let w = if i < 60 { "the world" } else { "another" };
            tracks.push(info(i, &[(i % 7) as u32], Some(i % 5 + 10 * u128::from(i >= 60))).with_world(w).with_duration(Some(200_000)));
            idx.insert(tid(i), point(if i < 60 { i as f32 } else { (i - 60) as f32 + 0.5 }, if i < 60 { 0.0 } else { 3.0 }));
        }
        let mut events = Vec::new();
        for k in 0..30i64 {
            let t = tid((k % 10) as u128);
            let ts = T0 - 10 * crate::taste::DAY_MS + k * 3_600_000;
            events.push(Play::new(t, ts, EventType::Start, Some(0)));
            events.push(Play::new(t, ts + 200_000, EventType::Complete, Some(200_000)));
        }
        let likes = (0..10).map(tid).collect();
        (Catalog::new(tracks), idx, TasteInput { events, likes, ..TasteInput::default() })
    }

    /// «Ещё не слышал» are tracks never played, the nearest to what was
    /// loved first; «Лучшее» what was loved; «В дорогу» starts in the world
    /// and goes on in the other.
    #[test]
    fn unheard_close_to_liked() {
        let (cat, idx, input) = world();
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None }, T0);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let w = crate::world_token("the world");
        let mixes = recs.world_mixes(w, DayPart::Day);
        let of = |k: WorldMixKind| mixes.iter().find(|x| x.kind == k).unwrap_or_else(|| panic!("{k:?}: {mixes:?}"));
        let unheard: Vec<u128> = of(WorldMixKind::Unheard).tracks.iter().map(|&i| model.id(i).0.as_u128()).collect();
        assert_eq!(unheard.len(), UNHEARD_LEN);
        assert!(unheard.iter().all(|&t| (10..60).contains(&t)), "never played, of the world: {unheard:?}");
        let mut sorted = unheard.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (10..10 + UNHEARD_LEN as u128).collect::<Vec<_>>(), "the nearest to the loved: {unheard:?}");
        assert!(of(WorldMixKind::Unheard).near.is_some_and(|i| model.id(i).0.as_u128() < 10));
        let best: HashSet<u128> = of(WorldMixKind::Best).tracks.iter().map(|&i| model.id(i).0.as_u128()).collect();
        assert!((0..10).all(|t| best.contains(&t)), "{best:?}");
        let road: Vec<u128> = of(WorldMixKind::Road).tracks.iter().map(|&i| model.id(i).0.as_u128()).collect();
        assert!(road[0] < 10 && road[1..].iter().all(|&t| t >= 60), "{road:?}");
        for m in &mixes {
            let albums: Vec<Option<u128>> = m.tracks.iter().map(|&i| Some(model.id(i).0.as_u128() % 5)).collect();
            for run in albums.windows(ALBUM_RUN + 1) {
                assert!(!run.iter().all(|a| *a == run[0]), "{:?}: an album four running {albums:?}", m.kind);
            }
        }
        assert!(recs.world_mixes(crate::world_token("nowhere"), DayPart::Day).is_empty());
    }
}
