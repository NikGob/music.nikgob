//! The order of a wave's batch: the steps from the song before it, the
//! energy kept steady, no artist twice running (see [`sequence`]).

use nikgob_core::model::TrackId;

use super::*;

/// The library picks of a batch at most this many: orders of more are not
/// all weighed ([`sequence`]).
const SEQUENCE_MAX: usize = 6;
/// Two songs of one artist next to each other cost this much in an order.
const SAME_ARTIST_COST: f32 = 10.0;
/// A song put a place later than the pull put it costs this much: the
/// order of the pull stands unless a step saves more.
const DELAY_COST: f32 = 0.15;
/// A key change of the circle of fifths' far side costs this much, a step
/// to a neighbour a sixth of it…
const KEY_COST: f32 = 0.5;
/// …told only when both keys are this sure (the correlation of their
/// pitch classes with the key's profile).
const KEY_SURE: f32 = 0.5;
/// A head this much louder than the tail before it is free; louder still,
/// a sudden attack, and it costs up to [`ATTACK_COST`] at [`ATTACK_FULL_DB`]
/// more.
const ATTACK_FREE_DB: f32 = 6.0;
const ATTACK_FULL_DB: f32 = 12.0;
const ATTACK_COST: f32 = 0.5;

/// Orders the library's songs of a calm or background wave's batch of
/// `picks` among their own places (new songs keep theirs: their sound is
/// mostly unknown) by the cost of the steps from the song before (the last
/// queued, else the session's): the edges' when both songs have them
/// ([`crate::sketch::Edge`]: the keys on the circle of fifths, a loud head
/// straight after a quiet tail), else a DJ's step of tempo and harmony
/// ([`crate::pick::dj_cost`]), and the energy kept steady. No artist twice
/// running; each place a song is put later than the pull put it costs
/// ([`DELAY_COST`]). The order given wins a tie. A wave of any other
/// character keeps the pull's order at a personal device: reordered, the
/// owner's replay heard fewer of the songs it played next and more skipped
/// sounds back. In a room ([`crate::context::PlayDevice::Ambient`]) a wave
/// of any character is ordered so.
pub fn sequence(ctx: &WaveContext<'_>, character: Character, session: &Session, queued: &[Ahead], picks: &mut [WavePick]) {
    if !crate::open_world::part_on(crate::open_world::Part::Sequence) {
        return;
    }
    let m = ctx.model;
    let steady = kept_steady(character, m.device());
    if !steady {
        return;
    }
    let Some(ix) = ctx.index else { return };
    let slots: Vec<usize> = picks.iter().enumerate().filter(|(_, p)| !p.new && matches!(p.item, Item::Track(_))).map(|(k, _)| k).collect();
    if slots.len() < 2 || slots.len() > SEQUENCE_MAX {
        return;
    }
    let prev: Option<TrackId> = queued.iter().rev().find_map(|q| if let Ahead::Track(t) = q { Some(*t) } else { None }).or_else(|| session.last().map(|p| p.track));
    let prev_artist = prev.and_then(|t| m.position(t)).map(|i| crate::open_world::ArtistKey::of_track(m, i).label(|| format!("track:{i}")));
    let id_of = |p: &WavePick| if let Item::Track(t) = p.item { Some(t) } else { None };
    let step = |a: Option<TrackId>, b: Option<TrackId>| match (a, b) {
        (Some(a), Some(b)) => transition(m, ix, a, b, steady),
        _ => 0.0,
    };
    let cost = |order: &[WavePick]| {
        let mut c = 0.0;
        let (mut before, mut before_artist) = (prev, prev_artist.clone());
        for p in order {
            c += step(before, id_of(p));
            if before_artist.as_deref() == Some(p.artist.as_str()) {
                c += SAME_ARTIST_COST;
            }
            before = id_of(p);
            before_artist = Some(p.artist.clone());
        }
        c
    };
    let lib: Vec<WavePick> = slots.iter().map(|&k| picks[k].clone()).collect();
    let mut best: (f32, Vec<usize>) = (f32::INFINITY, Vec::new());
    let mut perm: Vec<usize> = (0..lib.len()).collect();
    loop {
        let mut order = picks.to_vec();
        for (s, &j) in slots.iter().zip(&perm) {
            order[*s] = lib[j].clone();
        }
        let delay: usize = perm.iter().enumerate().map(|(at, &j)| at.saturating_sub(j)).sum();
        let c = cost(&order) + DELAY_COST * delay as f32;
        if c < best.0 - 1e-6 {
            best = (c, perm.clone());
        }
        if !next_permutation(&mut perm) {
            break;
        }
    }
    for (s, &j) in slots.iter().zip(&best.1) {
        picks[*s] = lib[j].clone();
    }
}

/// Whether a wave of `character` built for `device` is ordered for a
/// steady energy ([`sequence`]): a calm or a background one anywhere, and
/// in a room (a speaker) any wave, no loud head after a quiet tail; a
/// lively one never.
pub(crate) fn kept_steady(character: Character, device: crate::context::PlayDevice) -> bool {
    matches!(character, Character::Calm | Character::Background)
        || (character == Character::Any && device == crate::context::PlayDevice::Ambient)
}

/// The next permutation in lexicographic order; false after the last.
pub(super) fn next_permutation(p: &mut [usize]) -> bool {
    let Some(i) = (1..p.len()).rev().find(|&i| p[i - 1] < p[i]) else { return false };
    let j = (i..p.len()).rev().find(|&j| p[j] > p[i - 1]).unwrap_or(i);
    p.swap(i - 1, j);
    p[i..].reverse();
    true
}

/// The place of a key on the circle of fifths (a minor key at its relative
/// major's).
pub(super) fn fifths(k: crate::sketch::Key) -> i32 {
    let root = i32::from(k.root) + if k.minor { 3 } else { 0 };
    (root * 7).rem_euclid(12)
}

/// What the step from the library's song `a` to `b` costs (see
/// [`sequence`]): 0 a seamless one.
pub(crate) fn transition(m: &UserModel, ix: &crate::index::VectorIndex<TrackId>, a: TrackId, b: TrackId, steady: bool) -> f32 {
    let edges = ix.sketch(a).and_then(|s| s.tail()).zip(ix.sketch(b).and_then(|s| s.head()));
    let mut c = match edges {
        Some((tail, head)) => edge_cost(&tail, &head),
        None => match (ix.get(a), ix.get(b)) {
            (Some(x), Some(y)) => crate::pick::dj_cost(&crate::pick::Dj::of(x), &crate::pick::Dj::of(y)),
            _ => 0.0,
        },
    };
    if steady
        && let (Some(x), Some(y)) = (m.position(a).and_then(|i| m.energy_at(i)), m.position(b).and_then(|i| m.energy_at(i)))
    {
        c += (x - y).abs();
    }
    c
}

/// The step from a song's `tail` to the next one's `head`: their keys'
/// distance on the circle of fifths (both sure), and a head far louder
/// than the tail it follows coming in at full level.
pub(crate) fn edge_cost(tail: &crate::sketch::Edge, head: &crate::sketch::Edge) -> f32 {
    let key = match (tail.key(), head.key()) {
        (Some((a, ra)), Some((b, rb))) if ra >= KEY_SURE && rb >= KEY_SURE => {
            let d = (fifths(a) - fifths(b)).abs();
            KEY_COST * d.min(12 - d) as f32 / 6.0
        }
        _ => 0.0,
    };
    let attack = if head.shape == crate::sketch::EdgeShape::Cold {
        ATTACK_COST * ((head.loudness - tail.loudness - ATTACK_FREE_DB) / (ATTACK_FULL_DB - ATTACK_FREE_DB)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    key + attack
}
