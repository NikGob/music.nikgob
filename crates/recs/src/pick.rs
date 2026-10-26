//! What every selection shares (docs/stage4.md §4 «Общее для всех»): the
//! engine context [`Recs`], picks with their reasons, the exclusion of
//! disliked tracks and artists, the diversity rules (at most one track of
//! an artist in any window of three, at most two of an album in any window
//! of ten, at most two of a world — a soundtrack franchise or an artist's
//! discography — in any window of three and a quarter of the whole, recs-v2
//! §1.5) and the smooth order: a greedy nearest-neighbour path through
//! the picks in the normalised, weighted bliss space, where a track without
//! a vector stands as close to another as their shared artist, album or
//! co-listening says, mended by swaps wherever the path cornered itself
//! into breaking a rule ([`repair`]).

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, TrackId, UnixMs};
use nikgob_core::streaming::{RemoteId, StreamTrack};
use serde::{Deserialize, Serialize};

use crate::analysis::Vector;
use crate::discover::{album_key, artist_key};
use crate::index::{Normalizer, VectorIndex, euclid};
use crate::model::UserModel;
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
    /// The next one on the album of the track before.
    NextOnAlbum,
    /// Often follows this track in your sessions.
    FollowsOften(TrackId),
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
    /// «Миксы только из скачанного»: only tracks with a file are picked,
    /// and no new music.
    pub(crate) local_only: bool,
    /// The user model: a track's radio picks by it ([`crate::radio`]).
    pub model: Option<&'a UserModel>,
}

impl<'a> Recs<'a> {
    pub fn new(catalog: &'a Catalog, taste: &'a Taste, index: &'a VectorIndex<TrackId>) -> Self {
        Self {
            catalog,
            taste,
            index,
            norm: index.normalizer(),
            local_only: false,
            model: None,
        }
    }

    /// The engine whose radios of a track pick by `model` (built over the
    /// same catalog).
    #[must_use]
    pub fn with_model(mut self, model: &'a UserModel) -> Self {
        self.model = (model.len() == self.catalog.len()).then_some(model);
        self
    }

    /// The engine that picks only tracks with a file and no new music
    /// (`on`), as «Миксы только из скачанного» asks.
    #[must_use]
    pub fn local_only(mut self, on: bool) -> Self {
        self.local_only = on;
        self
    }

    /// Picks only tracks with a file ([`Recs::local_only`]).
    pub fn is_local_only(&self) -> bool {
        self.local_only
    }

    pub fn now(&self) -> UnixMs {
        self.taste.now()
    }

    pub(crate) fn info(&self, i: usize) -> &TrackInfo {
        &self.catalog.tracks()[i]
    }

    /// Disliked, or by a disliked artist, or without a file when only
    /// tracks with one are picked.
    pub(crate) fn blocked(&self, i: usize) -> bool {
        let t = self.info(i);
        self.taste.blocks(t) || (self.local_only && !t.local)
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
            song: t.song.map(Item::Track),
            reason,
            artists: t.artists.iter().map(|a| token_of_artist(*a)).collect(),
            album: t.release.map(|r| token(r.as_uuid().as_bytes())),
            world: t.world,
            lib_artists: t.artists.clone(),
            vec: self.embedded(t.id),
            score,
        }
    }

    /// `n` of `cands` (catalog tracks and their scores, any order) under
    /// the diversity rules with at most `world_share` of one world, in the
    /// smooth order from the best scored: a mix of fixed songs («Миксы для
    /// тебя»). Disliked tracks are the caller's to leave out.
    pub fn mix_of(&self, cands: &[(TrackId, f32)], n: usize, world_share: f32) -> Vec<TrackId> {
        let mut slots: Vec<Slot> = cands
            .iter()
            .filter_map(|&(id, score)| Some(self.slot(self.catalog.position(id)?, Reason::Favourite, score)))
            .collect();
        slots.sort_by(|a, b| b.score.total_cmp(&a.score));
        let rules = Diversity { world_share, ..Diversity::default() };
        let picked = select_diverse(slots, n, &rules, &mut HashSet::new());
        arrange(self, picked, &[], None, &rules)
            .into_iter()
            .filter_map(|s| match s.item {
                Item::Track(id) => Some(id),
                Item::Stream(_) => None,
            })
            .collect()
    }

    /// A slot for a new track.
    pub(crate) fn new_slot(&self, t: &NewTrack, score: f32) -> Slot {
        Slot {
            item: Item::Stream(t.id.clone()),
            song: None,
            reason: Reason::New,
            artists: if t.artist.is_empty() { Vec::new() } else { vec![token(t.artist.as_bytes())] },
            album: t.album.as_ref().map(|a| token(a.as_bytes())),
            world: None,
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
    /// The item standing for its song when another track is that song
    /// too: taken, it takes the song.
    pub song: Option<Item>,
    pub reason: Reason,
    /// Artist tokens (library artists by id, new tracks by folded name).
    pub artists: Vec<u64>,
    pub album: Option<u64>,
    /// [`TrackInfo::world`].
    pub world: Option<u64>,
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
    #[serde(default)]
    pub world: Option<u64>,
}

impl Placed {
    pub(crate) fn of(s: &Slot) -> Self {
        Self {
            artists: s.artists.clone(),
            album: s.album,
            world: s.world,
        }
    }
}

/// A placed track as the rules see it: artists, album, world.
pub(crate) type Seen<'a> = (&'a [u64], Option<u64>, Option<u64>);

/// What breaking the world rule weighs against an album's 1
/// ([`Diversity::weighted`])…
const W_WORLD: usize = 4;
/// …the artist rule…
const W_ARTIST: usize = 2;
/// …and a world over its share of a radio's batch (a run of three is
/// worse).
pub(crate) const W_SHARE: usize = 2;
/// [`repair`] swaps at most this many times.
const REPAIR_STEPS: usize = 64;

/// The diversity rules.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Diversity {
    /// No artist twice within this many consecutive tracks.
    pub artist_window: usize,
    /// At most `album_max` of one album within `album_window` tracks.
    pub album_window: usize,
    pub album_max: usize,
    /// At most `world_max` of one world within `world_window` tracks…
    pub world_window: usize,
    pub world_max: usize,
    /// …and at most this share of a selection (`world_max` at least).
    pub world_share: f32,
    /// An artist the artist rule does not apply to (an artist's radio);
    /// the world rules do not apply to such a selection either (an
    /// artist's tracks are their discography's world).
    pub exempt: Option<u64>,
}

impl Default for Diversity {
    fn default() -> Self {
        Self {
            artist_window: 3,
            album_window: 10,
            album_max: 2,
            world_window: 3,
            world_max: 2,
            world_share: 0.25,
            exempt: None,
        }
    }
}

impl Diversity {
    fn worlds(&self) -> bool {
        self.exempt.is_none()
    }

    /// The rules `s` breaks after `seq` (a sequence of placed tracks), each
    /// by what it weighs: a world
    /// thrice running is the user's own rule and weighs most ([`W_WORLD`]),
    /// an artist again within three [`W_ARTIST`], an album a third time
    /// within ten 1. When something must give, the album does.
    pub(crate) fn weighted(&self, seq: &[Seen<'_>], s: Seen<'_>) -> usize {
        let (artist, album, world) = self.broken(seq, s);
        W_ARTIST * artist + usize::from(album) + W_WORLD * usize::from(world)
    }

    /// `s` after `seq` makes one world thrice running.
    pub(crate) fn world_run(&self, seq: &[Seen<'_>], s: Seen<'_>) -> bool {
        self.broken(seq, s).2
    }

    /// The artists placed too near, and whether the album rule and the
    /// world rule break, for `s` after `seq`.
    fn broken(&self, seq: &[Seen<'_>], s: Seen<'_>) -> (usize, bool, bool) {
        let n = seq.len();
        let from = n.saturating_sub(self.artist_window - 1);
        let artist = seq[from..].iter().filter(|(artists, _, _)| s.0.iter().any(|a| Some(*a) != self.exempt && artists.contains(a))).count();
        let album = s.1.is_some_and(|album| {
            let from = n.saturating_sub(self.album_window - 1);
            seq[from..].iter().filter(|(_, b, _)| *b == Some(album)).count() >= self.album_max
        });
        let world = s.2.filter(|_| self.worlds()).is_some_and(|world| {
            let from = n.saturating_sub(self.world_window - 1);
            seq[from..].iter().filter(|(_, _, w)| *w == Some(world)).count() >= self.world_max
        });
        (artist, album, world)
    }

    /// The weight of every rule broken along `seq` from place `from` on
    /// (the places before it are what already plays).
    pub(crate) fn weight_of(&self, seq: &[Seen<'_>], from: usize) -> usize {
        (from..seq.len()).map(|k| self.weighted(&seq[..k], seq[k])).sum()
    }

    /// Caps on whole counts that keep an order of `n` tracks possible:
    /// per artist `ceil(n / artist_window)`, per album
    /// `album_max * ceil(n / album_window)`, per world exactly as many as
    /// `n` places hold (`world_max` of every window and of the part left).
    pub fn caps(&self, n: usize) -> (usize, usize, usize) {
        let world = self.world_max * (n / self.world_window) + self.world_max.min(n % self.world_window);
        (
            n.div_ceil(self.artist_window).max(1),
            self.album_max * n.div_ceil(self.album_window).max(1),
            if self.worlds() { world.max(1) } else { usize::MAX },
        )
    }

    /// How many of `n` one world may have: its share, `world_max` at least.
    pub fn world_cap(&self, n: usize) -> usize {
        if !self.worlds() {
            return usize::MAX;
        }
        ((n as f32 * self.world_share).ceil() as usize).max(self.world_max)
    }
}

/// The best `n` of `cands` (sorted by score, best first) whose artist and
/// album counts let them be ordered under `rules`, no world over its
/// share. Short when `cands` are; when too few fit, the rest by score
/// anyway, so a library of a single world gives as many as there are.
pub(crate) fn select_diverse(cands: Vec<Slot>, n: usize, rules: &Diversity, taken: &mut HashSet<Item>) -> Vec<Slot> {
    let (artist_cap, album_cap, _) = rules.caps(n);
    let worlds: HashSet<u64> = cands.iter().filter_map(|s| s.world).collect();
    let world_cap = if worlds.len() > 1 { rules.world_cap(n) } else { usize::MAX };
    let mut per_artist: HashMap<u64, usize> = HashMap::new();
    let mut per_album: HashMap<u64, usize> = HashMap::new();
    let mut per_world: HashMap<u64, usize> = HashMap::new();
    let mut out = Vec::with_capacity(n);
    let mut spare = Vec::new();
    for s in cands {
        if out.len() == n {
            break;
        }
        if taken.contains(&s.item) || s.song.as_ref().is_some_and(|x| taken.contains(x)) {
            continue;
        }
        let artist_full = s
            .artists
            .iter()
            .any(|a| Some(*a) != rules.exempt && per_artist.get(a).is_some_and(|c| *c >= artist_cap));
        let album_full = s.album.is_some_and(|b| per_album.get(&b).is_some_and(|c| *c >= album_cap));
        let world_full = s.world.is_some_and(|w| per_world.get(&w).is_some_and(|c| *c >= world_cap));
        if artist_full || album_full || world_full {
            spare.push(s);
            continue;
        }
        for a in &s.artists {
            *per_artist.entry(*a).or_default() += 1;
        }
        if let Some(b) = s.album {
            *per_album.entry(b).or_default() += 1;
        }
        if let Some(w) = s.world {
            *per_world.entry(w).or_default() += 1;
        }
        take(taken, &s);
        out.push(s);
    }
    // Too few that fit: the rest by score anyway (a small library).
    for s in spare {
        if out.len() == n {
            break;
        }
        if !taken.contains(&s.item) && !s.song.as_ref().is_some_and(|x| taken.contains(x)) {
            take(taken, &s);
            out.push(s);
        }
    }
    out
}

/// Marks the slot's item and its song taken: one song plays once.
pub(crate) fn take(taken: &mut HashSet<Item>, s: &Slot) {
    taken.insert(s.item.clone());
    if let Some(song) = &s.song {
        taken.insert(song.clone());
    }
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
        let seq: Vec<Seen<'_>> = before
            .iter()
            .map(|p| (p.artists.as_slice(), p.album, p.world))
            .chain(out.iter().map(|s| (s.artists.as_slice(), s.album, s.world)))
            .collect();
        // Counts left, so an artist with many slots is not cornered at the end.
        let mut artist_left: HashMap<u64, usize> = HashMap::new();
        let mut album_left: HashMap<u64, usize> = HashMap::new();
        let mut world_left: HashMap<u64, usize> = HashMap::new();
        for s in &slots {
            for a in &s.artists {
                if Some(*a) != rules.exempt {
                    *artist_left.entry(*a).or_default() += 1;
                }
            }
            if let Some(b) = s.album {
                *album_left.entry(b).or_default() += 1;
            }
            if let Some(w) = s.world {
                *world_left.entry(w).or_default() += 1;
            }
        }
        let (artist_cap, album_cap, world_cap) = rules.caps(slots.len() - 1);
        // How far over what the places left hold the counts would be: the
        // fewer, the better (one crowded album does not hide a crowded
        // world).
        let crowded = |s: &Slot| {
            let over = |c: usize, mine: bool, cap: usize| (c - usize::from(mine)).saturating_sub(cap);
            artist_left.iter().map(|(a, c)| over(*c, s.artists.contains(a), artist_cap)).sum::<usize>()
                + album_left.iter().map(|(b, c)| over(*c, s.album == Some(*b), album_cap)).sum::<usize>()
                + world_left.iter().map(|(w, c)| over(*c, s.world == Some(*w), world_cap)).sum::<usize>()
        };
        let mut best: Option<((usize, usize), f32, usize)> = None;
        for (i, s) in slots.iter().enumerate() {
            let v = (rules.weighted(&seq, (&s.artists, s.album, s.world)), crowded(s));
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
    repair(recs, &mut out, before, start, rules, m);
    out
}

/// What a greedy order cannot see coming: near its end the places left
/// may hold only tracks of one world, or of an album twice in the last
/// ten already, so a rule breaks there. The order is mended by swapping
/// two of its places, each time the swap that lowers the weight of the
/// rules broken most ([`Diversity::weighted`]; of equal ones the smoother
/// path), until none lowers it. What already plays (`before`) stays; `m`
/// is the typical distance of [`relation_distance`].
pub(crate) fn repair(recs: &Recs<'_>, out: &mut [Slot], before: &[Placed], start: Option<&Slot>, rules: &Diversity, m: f32) {
    let weight_of = |out: &[Slot]| -> usize {
        let seq: Vec<Seen<'_>> = before
            .iter()
            .map(|p| (p.artists.as_slice(), p.album, p.world))
            .chain(out.iter().map(|s| (s.artists.as_slice(), s.album, s.world)))
            .collect();
        rules.weight_of(&seq, before.len())
    };
    let path = |out: &[Slot]| -> f32 {
        let first = match (start, out.first()) {
            (Some(p), Some(s)) => distance(recs, p, s, m),
            _ => 0.0,
        };
        first + out.windows(2).map(|w| distance(recs, &w[0], &w[1], m)).sum::<f32>()
    };
    let mut weight = weight_of(out);
    for _ in 0..REPAIR_STEPS {
        if weight == 0 {
            return;
        }
        let mut best: Option<(usize, f32, usize, usize)> = None;
        for i in 0..out.len() {
            for j in i + 1..out.len() {
                out.swap(i, j);
                let w = weight_of(out);
                if w < weight {
                    let d = path(out);
                    if best.is_none_or(|b| w < b.0 || (w == b.0 && d < b.1)) {
                        best = Some((w, d, i, j));
                    }
                }
                out.swap(i, j);
            }
        }
        let Some((w, _, i, j)) = best else { return };
        out.swap(i, j);
        weight = w;
    }
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
    let mut seq: Vec<(Vec<ArtistId>, Option<nikgob_core::model::ReleaseId>, Option<u64>)> = Vec::new();
    for p in picks {
        // A new track holds its place (its artists are not the library's).
        let Item::Track(id) = &p.item else {
            seq.push((Vec::new(), None, None));
            continue;
        };
        let t = recs.catalog.get(*id).unwrap();
        seq.push((t.artists.clone(), t.release, t.world));
    }
    for (i, (artists, _, world)) in seq.iter().enumerate() {
        for prev in seq[i.saturating_sub(2)..i].iter() {
            assert!(!artists.iter().any(|a| prev.0.contains(a)), "an artist twice within three at {i}");
        }
        if i >= 2 && world.is_some() {
            assert!(!(seq[i - 1].2 == *world && seq[i - 2].2 == *world), "a world thrice running at {i}");
        }
    }
    for w in seq.windows(10.min(seq.len()).max(1)) {
        for (_, album, _) in w {
            if let Some(album) = album {
                assert!(w.iter().filter(|(_, b, _)| b.as_ref() == Some(album)).count() <= 2, "an album thrice within ten");
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
    fn a_mix_never_has_one_song_twice() {
        // Track 1 is track 0's song on another album (an OST and its special
        // edition); 2 is a song of its own.
        let tracks = vec![info(0, &[1], Some(10)), info(1, &[1], Some(11)).with_song(tid(0)), info(2, &[2], Some(10))];
        let cat = Catalog::new(tracks);
        let idx = VectorIndex::new();
        let taste = Taste::build(&cat, &TasteInput::default(), 0);
        let recs = Recs::new(&cat, &taste, &idx);
        let slots: Vec<Slot> = [1, 0, 2].into_iter().map(|i| recs.slot(i, Reason::Random, 1.0)).collect();
        let mut taken = HashSet::new();
        let out = select_diverse(slots, 3, &Diversity::default(), &mut taken);
        let ids: Vec<Item> = out.into_iter().map(|s| s.item).collect();
        assert_eq!(ids, [Item::Track(tid(1)), Item::Track(tid(2))], "one of the two tracks of the song");
        // A later part of the mix does not bring it back either.
        let again = select_diverse(vec![recs.slot(0, Reason::Random, 1.0)], 1, &Diversity::default(), &mut taken);
        assert!(again.is_empty());
    }

    /// 30 tracks of one world by 30 artists (a franchise's OSTs, each by
    /// its own credit) and 30 of others, the world's scored best: a quarter
    /// of a mix at most, never three of it running; a library of the world
    /// alone gives as many as there are.
    #[test]
    fn world_cap_holds() {
        let mut tracks: Vec<TrackInfo> = (0..30u128).map(|i| info(i, &[i as u32], Some(100 + i)).with_world("made in abyss")).collect();
        tracks.extend((30..60u128).map(|i| info(i, &[i as u32], Some(100 + i)).with_world(&format!("other {}", i % 6))));
        let cat = Catalog::new(tracks);
        let mut idx = VectorIndex::with_weights([1.0; crate::DIM]);
        for i in 0..60u128 {
            idx.insert(tid(i), line((i % 30) as f32));
        }
        let taste = Taste::build(&cat, &TasteInput::default(), 0);
        let recs = Recs::new(&cat, &taste, &idx);
        let mia = cat.tracks()[0].world;
        let slots = |range: std::ops::Range<usize>| -> Vec<Slot> {
            let mut v: Vec<Slot> = range.map(|i| recs.slot(i, Reason::Random, if i < 30 { 100.0 - i as f32 } else { 10.0 - i as f32 / 10.0 })).collect();
            v.sort_by(|a, b| b.score.total_cmp(&a.score));
            v
        };
        let rules = Diversity::default();
        let picked = select_diverse(slots(0..60), 20, &rules, &mut HashSet::new());
        assert_eq!(picked.len(), 20);
        let out = arrange(&recs, picked, &[], None, &rules);
        let worlds: Vec<Option<u64>> = out.iter().map(|s| s.world).collect();
        assert!(worlds.iter().filter(|w| **w == mia).count() <= 5, "a quarter of 20");
        for w in worlds.windows(3) {
            assert!(!(w[0] == mia && w[1] == mia && w[2] == mia), "{worlds:?}");
        }
        let picks: Vec<Pick> = out.into_iter().map(Slot::pick).collect();
        assert_diverse(&recs, &picks);
        // One world only: no cap.
        let alone = select_diverse(slots(0..30), 20, &rules, &mut HashSet::new());
        assert_eq!(alone.len(), 20);
    }

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
