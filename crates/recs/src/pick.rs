//! What every selection shares (docs/stage4.md §4 «Общее для всех»): the
//! engine context [`Recs`], picks with their reasons, the exclusion of
//! disliked tracks and artists, the diversity rules (at most one track of
//! an artist in any window of three, at most two of an album in any window
//! of ten) and the smooth order: a greedy nearest-neighbour path through
//! the picks in the normalised, weighted bliss space, where a track without
//! a vector stands as close to another as their shared artist, album or
//! co-listening says.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, StreamTrack};
use serde::{Deserialize, Serialize};

use crate::analysis::Vector;
use crate::discover::{album_key, artist_key};
use crate::index::{Normalizer, VectorIndex, euclid};
use crate::mood::Mood;
use crate::taste::{Catalog, DayPart, Taste, TrackInfo};
use crate::tiles::TileKind;

/// What a pick plays.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Item {
    /// A catalog track (a library track or a liked stream).
    Track(TrackId),
    /// New music from a service, not in the library.
    Stream(RemoteId),
}

/// Why a track was picked (for an explaining line in the UI).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reason {
    /// One of your favourites.
    Favourite,
    /// Sounds like this track.
    SimilarTo(TrackId),
    /// A favourite not played for a long time.
    Rediscovery,
    /// Anything from the library (no history yet).
    Random,
    /// Fits this mood.
    Mood(Mood),
    /// Belongs to this style mix (its index).
    Style(usize),
    /// By the radio's artist, or the seed track's artist.
    SameArtist(ArtistId),
    /// Its artist is played in the same sessions as this one.
    CoListened(ArtistId),
    /// Sounds like the radio's seed.
    SoundsLikeSeed,
    /// New music for you.
    New,
    /// Found by discovery: so many of your seeds pointed at it.
    Discovered { votes: u32 },
    /// Sounds like what you play in this part of the day (the daylist).
    DayPart(DayPart),
    /// Belongs to a tile of the bento («На повторе», «Исполнитель дня»…).
    Tile(TileKind),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pick {
    pub item: Item,
    pub reason: Reason,
}

/// A new-music candidate the runtime found in a service (discovery, a
/// service's radio of the seed): enough to place it in a mix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewTrack {
    pub id: RemoteId,
    /// [`artist_key`] of its main artist.
    pub artist: String,
    /// [`album_key`] of its album.
    pub album: Option<String>,
    /// Its sound, when it was analysed.
    pub vector: Option<Vector>,
}

impl NewTrack {
    pub fn from_stream(t: &StreamTrack, vector: Option<Vector>) -> Self {
        let artist = t.artists.first().map(|a| artist_key(&a.name)).unwrap_or_default();
        Self {
            id: t.id.clone(),
            album: t.album.as_ref().map(|a| album_key(&a.title, &artist)),
            artist,
            vector,
        }
    }
}

/// The engine over one moment of the library: the catalog, the taste built
/// for it and the sound vectors (keys are catalog track ids; tracks without
/// a vector are fine).
pub struct Recs<'a> {
    pub catalog: &'a Catalog,
    pub taste: &'a Taste,
    pub index: &'a VectorIndex<TrackId>,
    pub(crate) norm: Normalizer,
}

impl<'a> Recs<'a> {
    pub fn new(catalog: &'a Catalog, taste: &'a Taste, index: &'a VectorIndex<TrackId>) -> Self {
        Self {
            catalog,
            taste,
            index,
            norm: index.normalizer(),
        }
    }

    pub fn now(&self) -> UnixMs {
        self.taste.now()
    }

    pub(crate) fn info(&self, i: usize) -> &TrackInfo {
        &self.catalog.tracks()[i]
    }

    /// Disliked, or by a disliked artist.
    pub(crate) fn blocked(&self, i: usize) -> bool {
        self.taste.blocks(self.info(i))
    }

    /// The track's vector in the weighted normalised space.
    pub(crate) fn embedded(&self, id: TrackId) -> Option<Vector> {
        self.index.get(id).map(|v| self.norm.embed(v))
    }

    /// Played within `ms` before now.
    pub(crate) fn played_within(&self, i: usize, ms: i64) -> bool {
        self.taste.last_played_at(i).is_some_and(|t| self.now() - t < ms)
    }

    /// A slot for the catalog track at `i`.
    pub(crate) fn slot(&self, i: usize, reason: Reason, score: f32) -> Slot {
        let t = self.info(i);
        Slot {
            item: Item::Track(t.id),
            reason,
            artists: t.artists.iter().map(|a| token_of_artist(*a)).collect(),
            album: t.release.map(|r| token(r.as_uuid().as_bytes())),
            lib_artists: t.artists.clone(),
            vec: self.embedded(t.id),
            score,
        }
    }

    /// A slot for a new track.
    pub(crate) fn new_slot(&self, t: &NewTrack, score: f32) -> Slot {
        Slot {
            item: Item::Stream(t.id.clone()),
            reason: Reason::New,
            artists: if t.artist.is_empty() { Vec::new() } else { vec![token(t.artist.as_bytes())] },
            album: t.album.as_ref().map(|a| token(a.as_bytes())),
            lib_artists: Vec::new(),
            vec: t.vector.map(|v| self.norm.embed(&v)),
            score,
        }
    }
}

/// FNV-1a: tokens that stay the same across builds (radio state keeps them).
pub(crate) fn token(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub(crate) fn token_of_artist(a: ArtistId) -> u64 {
    token(a.as_uuid().as_bytes())
}

/// A pick being placed.
#[derive(Clone, Debug)]
pub(crate) struct Slot {
    pub item: Item,
    pub reason: Reason,
    /// Artist tokens (library artists by id, new tracks by folded name).
    pub artists: Vec<u64>,
    pub album: Option<u64>,
    pub lib_artists: Vec<ArtistId>,
    /// Embedded vector.
    pub vec: Option<Vector>,
    pub score: f32,
}

impl Slot {
    pub fn pick(self) -> Pick {
        Pick {
            item: self.item,
            reason: self.reason,
        }
    }
}

/// What came right before the slots being placed (a radio's tail).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    pub artists: Vec<u64>,
    pub album: Option<u64>,
}

/// The diversity rules.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Diversity {
    /// No artist twice within this many consecutive tracks.
    pub artist_window: usize,
    /// At most `album_max` of one album within `album_window` tracks.
    pub album_window: usize,
    pub album_max: usize,
    /// An artist the artist rule does not apply to (an artist's radio).
    pub exempt: Option<u64>,
}

impl Default for Diversity {
    fn default() -> Self {
        Self {
            artist_window: 3,
            album_window: 10,
            album_max: 2,
            exempt: None,
        }
    }
}

impl Diversity {
    /// How many rules `s` breaks after `seq` (a sequence of placed tracks).
    fn violations(&self, seq: &[(&[u64], Option<u64>)], s: (&[u64], Option<u64>)) -> usize {
        let mut v = 0;
        let n = seq.len();
        let from = n.saturating_sub(self.artist_window - 1);
        for (artists, _) in &seq[from..] {
            if s.0.iter().any(|a| Some(*a) != self.exempt && artists.contains(a)) {
                v += 1;
            }
        }
        if let Some(album) = s.1 {
            let from = n.saturating_sub(self.album_window - 1);
            let same = seq[from..].iter().filter(|(_, b)| *b == Some(album)).count();
            if same >= self.album_max {
                v += 1;
            }
        }
        v
    }

    /// Caps on whole counts that keep an order of `n` tracks possible:
    /// per artist `ceil(n / artist_window)`, per album
    /// `album_max * ceil(n / album_window)`.
    pub fn caps(&self, n: usize) -> (usize, usize) {
        (n.div_ceil(self.artist_window).max(1), self.album_max * n.div_ceil(self.album_window).max(1))
    }
}

/// The best `n` of `cands` (sorted by score, best first) whose artist and
/// album counts let them be ordered under `rules`. Short when `cands` are.
pub(crate) fn select_diverse(cands: Vec<Slot>, n: usize, rules: &Diversity, taken: &mut HashSet<Item>) -> Vec<Slot> {
    let (artist_cap, album_cap) = rules.caps(n);
    let mut per_artist: HashMap<u64, usize> = HashMap::new();
    let mut per_album: HashMap<u64, usize> = HashMap::new();
    let mut out = Vec::with_capacity(n);
    let mut spare = Vec::new();
    for s in cands {
        if out.len() == n {
            break;
        }
        if taken.contains(&s.item) {
            continue;
        }
        let artist_full = s
            .artists
            .iter()
            .any(|a| Some(*a) != rules.exempt && per_artist.get(a).is_some_and(|c| *c >= artist_cap));
        let album_full = s.album.is_some_and(|b| per_album.get(&b).is_some_and(|c| *c >= album_cap));
        if artist_full || album_full {
            spare.push(s);
            continue;
        }
        for a in &s.artists {
            *per_artist.entry(*a).or_default() += 1;
        }
        if let Some(b) = s.album {
            *per_album.entry(b).or_default() += 1;
        }
        taken.insert(s.item.clone());
        out.push(s);
    }
    // Too few that fit: the rest by score anyway (a small library).
    for s in spare {
        if out.len() == n {
            break;
        }
        if taken.insert(s.item.clone()) {
            out.push(s);
        }
    }
    out
}

/// A rough "how far apart" for two slots when one has no vector, in units
/// of the typical distance `m` between the slots.
fn relation_distance(recs: &Recs<'_>, a: &Slot, b: &Slot, m: f32) -> f32 {
    if a.album.is_some() && a.album == b.album {
        return 0.5 * m;
    }
    if a.artists.iter().any(|x| b.artists.contains(x)) {
        return 0.6 * m;
    }
    let co = a
        .lib_artists
        .iter()
        .any(|x| b.lib_artists.iter().any(|y| recs.taste.co_count(*x, *y) > 0.0));
    if co { 0.9 * m } else { 1.5 * m }
}

fn distance(recs: &Recs<'_>, a: &Slot, b: &Slot, m: f32) -> f32 {
    match (&a.vec, &b.vec) {
        (Some(x), Some(y)) => euclid(x, y),
        _ => relation_distance(recs, a, b, m),
    }
}

/// The median distance between consecutive vectored slots (1.0 without).
fn typical_distance(slots: &[Slot]) -> f32 {
    let vs: Vec<&Vector> = slots.iter().filter_map(|s| s.vec.as_ref()).collect();
    let mut d: Vec<f32> = vs.windows(2).take(256).map(|w| euclid(w[0], w[1])).collect();
    if d.is_empty() {
        return 1.0;
    }
    d.sort_by(f32::total_cmp);
    d[d.len() / 2].max(1e-3)
}

/// `slots` in a smooth order under the diversity rules. The path starts
/// after `before` (what already plays, last = most recent; `start` its
/// last slot's vector and relations) or, without it, at the best-scored
/// slot; each next one is the nearest that breaks no rule (the nearest that
/// breaks the fewest when none fits).
pub(crate) fn arrange(recs: &Recs<'_>, mut slots: Vec<Slot>, before: &[Placed], start: Option<&Slot>, rules: &Diversity) -> Vec<Slot> {
    if slots.is_empty() {
        return slots;
    }
    let m = typical_distance(&slots);
    let mut out: Vec<Slot> = Vec::with_capacity(slots.len());
    let mut prev: Option<Slot> = start.cloned();
    while !slots.is_empty() {
        let seq: Vec<(&[u64], Option<u64>)> = before
            .iter()
            .map(|p| (p.artists.as_slice(), p.album))
            .chain(out.iter().map(|s| (s.artists.as_slice(), s.album)))
            .collect();
        // Counts left, so an artist with many slots is not cornered at the end.
        let mut artist_left: HashMap<u64, usize> = HashMap::new();
        let mut album_left: HashMap<u64, usize> = HashMap::new();
        for s in &slots {
            for a in &s.artists {
                if Some(*a) != rules.exempt {
                    *artist_left.entry(*a).or_default() += 1;
                }
            }
            if let Some(b) = s.album {
                *album_left.entry(b).or_default() += 1;
            }
        }
        let (artist_cap, album_cap) = rules.caps(slots.len() - 1);
        let crowded = |s: &Slot| {
            let over_artist = artist_left.iter().any(|(a, c)| c - usize::from(s.artists.contains(a)) > artist_cap);
            let over_album = album_left.iter().any(|(b, c)| c - usize::from(s.album == Some(*b)) > album_cap);
            usize::from(over_artist || over_album)
        };
        let mut best: Option<((usize, usize), f32, usize)> = None;
        for (i, s) in slots.iter().enumerate() {
            let v = (rules.violations(&seq, (&s.artists, s.album)), crowded(s));
            let d = match &prev {
                Some(p) => distance(recs, p, s, m),
                // The first one: the best score.
                None => -s.score,
            };
            let better = match best {
                None => true,
                Some((bv, bd, _)) => v < bv || (v == bv && d < bd),
            };
            if better {
                best = Some((v, d, i));
            }
        }
        let (_, _, i) = best.expect("slots is not empty");
        let s = slots.swap_remove(i);
        prev = Some(s.clone());
        out.push(s);
    }
    out
}

/// Fills `(count)` slots for parts in proportion: how many of `n` each
/// share gets, rounding so they sum to `n`.
pub(crate) fn split(n: usize, shares: &[f32]) -> Vec<usize> {
    let total: f32 = shares.iter().sum();
    if total <= 0.0 {
        return vec![0; shares.len()];
    }
    let exact: Vec<f32> = shares.iter().map(|s| s / total * n as f32).collect();
    let mut out: Vec<usize> = exact.iter().map(|x| x.floor() as usize).collect();
    let mut left = n - out.iter().sum::<usize>();
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by(|a, b| (exact[*b] - exact[*b].floor()).total_cmp(&(exact[*a] - exact[*a].floor())).then(a.cmp(b)));
    for i in order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

/// The number of new tracks among `n` for a share (0…1).
pub(crate) fn new_count(n: usize, share: f32) -> usize {
    ((n as f32) * share.clamp(0.0, 1.0)).round() as usize
}

/// Every window of the picks keeps the rules (tests).
#[cfg(test)]
pub(crate) fn assert_diverse(recs: &Recs<'_>, picks: &[Pick]) {
    let mut seq: Vec<(Vec<ArtistId>, Option<nikgob_core::model::ReleaseId>)> = Vec::new();
    for p in picks {
        // A new track holds its place (its artists are not the library's).
        let Item::Track(id) = &p.item else {
            seq.push((Vec::new(), None));
            continue;
        };
        let t = recs.catalog.get(*id).unwrap();
        seq.push((t.artists.clone(), t.release));
    }
    for (i, (artists, _)) in seq.iter().enumerate() {
        for prev in seq[i.saturating_sub(2)..i].iter() {
            assert!(!artists.iter().any(|a| prev.0.contains(a)), "an artist twice within three at {i}");
        }
    }
    for w in seq.windows(10.min(seq.len()).max(1)) {
        for (_, album) in w {
            if let Some(album) = album {
                assert!(w.iter().filter(|(_, b)| b.as_ref() == Some(album)).count() <= 2, "an album thrice within ten");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::taste::TasteInput;
    use crate::test_util::*;

    #[test]
    fn split_rounds_to_the_total() {
        assert_eq!(split(30, &[0.5, 0.3, 0.2]), vec![15, 9, 6]);
        assert_eq!(split(7, &[0.5, 0.3, 0.2]).iter().sum::<usize>(), 7);
        assert_eq!(split(0, &[1.0]), vec![0]);
        assert_eq!(new_count(30, 0.2), 6);
        assert_eq!(new_count(30, 0.0), 0);
    }

    #[test]
    fn arrange_keeps_the_rules_and_follows_the_sound() {
        // 12 artists × 4 tracks, two albums per artist; vectors on a line by
        // track number, so a smooth path walks the line.
        let tracks: Vec<TrackInfo> = (0..48u128).map(|i| info(i, &[(i % 12) as u32], Some(1000 + i % 24))).collect();
        let cat = Catalog::new(tracks);
        let mut idx = VectorIndex::with_weights([1.0; crate::DIM]);
        for i in 0..48u128 {
            idx.insert(tid(i), line(i as f32));
        }
        let taste = Taste::build(&cat, &TasteInput::default(), 0);
        let recs = Recs::new(&cat, &taste, &idx);
        let slots: Vec<Slot> = (0..48).rev().map(|i| recs.slot(i, Reason::Random, i as f32)).collect();
        let out = arrange(&recs, slots, &[], None, &Diversity::default());
        assert_eq!(out.len(), 48);
        let picks: Vec<Pick> = out.into_iter().map(Slot::pick).collect();
        assert_diverse(&recs, &picks);
        // Starts at the best score (47) and moves in small steps.
        assert_eq!(picks[0].item, Item::Track(tid(47)));
        let pos: Vec<i64> = picks
            .iter()
            .map(|p| match &p.item {
                Item::Track(id) => (id.0.as_u128()) as i64,
                Item::Stream(_) => unreachable!(),
            })
            .collect();
        let jumps: i64 = pos.windows(2).map(|w| (w[0] - w[1]).abs()).sum();
        assert!(jumps < 48 * 4, "a smooth path, not a shuffle: {jumps}");
    }
}
