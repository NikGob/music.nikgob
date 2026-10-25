//! Endless selections (docs/stage4.md §4): the radio of a track or an
//! artist, and the mood and style mixes, which play the same way. A
//! [`RadioState`] keeps the seed, what was served, the session's lessons
//! and the new-music candidates; [`RadioState::extend`] gives the next `n`
//! picks and updates it. The state is serde, so the runtime keeps it in
//! `session.json` and a radio survives a restart.
//!
//! Each extension:
//! 1. takes about `new_share` of `n` from the new-music pool the runtime
//!    feeds ([`RadioState::add_new`]: a service's radio of the seed, the
//!    radio of its artist, discoveries), nearest to the seed first;
//! 2. gathers library candidates by seed: sound neighbours of the seed (and
//!    of what completed in this session), the seed's artists, artists
//!    co-listened with them, a mood's top third, a style's artists, and the
//!    favourites as a fallback so the radio never runs dry;
//! 3. scores them: closeness to the seed and to completed tracks, minus
//!    closeness to early-skipped tracks ([`SKIP_PUSH`]), plus affinity, the
//!    relation to the seed, the day part and a seeded jitter;
//! 4. selects the best under the diversity caps and orders them smoothly
//!    from the last track served.
//!
//! Repeats (recs-v2 §17, decision 3): a radio never serves a pick twice,
//! but a track played today comes back once [`REPEAT_AFTER`] others played
//! since it ([`REPEAT_AFTER_MS`] for what played before the radio), and one
//! played twice running plays once more first; a mix leaves what played in
//! the last three hours out. Both score what played lately less, a mix
//! enough to let it rest about a day and a half ([`MIX_REST`]).
//!
//! An artist's radio is ~70% the artist's own tracks by affinity
//! ([`ARTIST_OWN_SHARE`]), the rest similar by sound and co-listening; the
//! artist rule of diversity does not apply to that artist. A daylist
//! ([`crate::daylist`]) draws from the tracks near its centre on the mood
//! axes and weighs the bonus of its own part of the day double.

use std::collections::{HashMap, HashSet, VecDeque};

use nikgob_core::model::{ArtistId, TrackId};
use nikgob_core::streaming::RemoteId;
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::index::euclid;
use crate::mood::Mood;
use crate::pick::{Diversity, Item, NewTrack, Pick, Placed, Reason, Recs, Slot, arrange, new_count, select_diverse, token_of_artist};
use crate::rng::Rng;
use crate::mood::Axes;
use crate::taste::{DayPart, HOUR_MS};

/// Served items remembered (never repeated while remembered).
const SERVED_CAP: usize = 2_000;
/// When the library runs dry, only this many last served stay excluded.
const RELAXED_SERVED: usize = 50;
/// A mix leaves tracks played this recently for later (unless the pool is
/// dry).
const RECENT_MS: i64 = 3 * HOUR_MS;
/// A radio may bring back a track played today (recs-v2 §17, decision 3):
/// once this many others played since it…
const REPEAT_AFTER: usize = 5;
/// …or, for what played before the radio, this long ago (about five
/// tracks); one played twice running comes back at once.
const REPEAT_AFTER_MS: i64 = 20 * 60 * 1000;
/// What played lately scores less by this much times
/// [`crate::Taste::lately_at`]: a mix lets a track rest about a day and a
/// half…
pub const MIX_REST: f32 = 0.8;
/// …a radio less so.
const RADIO_REST: f32 = 0.3;
/// Session lessons remembered.
const LESSONS: usize = 12;
/// New-music candidates kept.
const NEW_POOL_CAP: usize = 400;
/// Sound neighbours of the seed gathered.
const SEED_NEIGHBOURS: usize = 200;
const LIKED_NEIGHBOURS: usize = 40;
const CO_ARTISTS: usize = 6;
const CO_TRACKS: usize = 8;
const FALLBACK: usize = 200;
/// An artist's radio: this share is the artist's own tracks.
pub const ARTIST_OWN_SHARE: f32 = 0.7;
/// Weight of a completed track's pull against the seed's 1.0.
const LIKED_PULL: f32 = 0.5;
/// How hard an early-skipped track pushes picks near it away.
pub const SKIP_PUSH: f32 = 1.5;
const W_AFFINITY: f32 = 0.8;
const W_RELATION: f32 = 0.3;
const W_DAYPART: f32 = 0.3;
/// The day part's weight in a daylist.
const W_DAYLIST_DAYPART: f32 = 0.6;
const JITTER: f32 = 0.4;

/// What a radio or an endless mix grows from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RadioSeed {
    /// A catalog track («Радио по треку»); the runtime plays it first.
    Track(TrackId),
    /// A stream track outside the library: its sound when analysed and the
    /// library artists it matches (both may be empty). New music gets at
    /// least half of such a radio.
    Stream {
        id: RemoteId,
        vector: Option<Vector>,
        artists: Vec<ArtistId>,
    },
    /// An artist's radio («Миксы исполнителей», «Радио» on the artist page).
    Artist(ArtistId),
    /// A mood card.
    Mood(Mood),
    /// A style mix («Твои миксы»): see [`crate::style::StyleMix::seed`].
    Style {
        index: usize,
        centroid: Vec<f32>,
        artists: Vec<ArtistId>,
    },
    /// «Открытия»: only new music while the pool lasts, then favourites.
    Discover,
    /// «Дейлист» of a part of the day, around its centre on the mood axes
    /// (energy, brightness, steadiness): see [`crate::daylist`].
    Daylist { part: DayPart, centre: [f32; 3] },
}

/// How a served track ended (session learning).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    Skip { position_ms: u32 },
}

/// Settings of one extension.
#[derive(Clone, Copy, Debug)]
pub struct RadioParams {
    /// «Новое в радио»: 0, ~0.2 or ~0.4.
    pub new_share: f32,
    /// Seeds the jitter (with the extension's turn number).
    pub seed: u64,
}

/// A radio between extensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RadioState {
    pub seed: RadioSeed,
    served: VecDeque<Item>,
    tail: Vec<Placed>,
    last: Option<Item>,
    /// Completed in this session (pull).
    liked: Vec<Item>,
    /// Skipped early in this session (push).
    skipped: Vec<Item>,
    /// The last [`REPEAT_AFTER`] that played, oldest first.
    #[serde(default)]
    recent: Vec<Item>,
    /// Played twice running: it plays once more first.
    #[serde(default)]
    again: Option<Item>,
    new_pool: Vec<NewTrack>,
    turns: u64,
}

fn push_capped<T: PartialEq>(v: &mut Vec<T>, x: T, cap: usize) {
    v.retain(|y| *y != x);
    v.push(x);
    if v.len() > cap {
        v.remove(0);
    }
}

impl RadioState {
    /// A fresh radio. A track seed counts as served (it plays first).
    pub fn new(seed: RadioSeed) -> Self {
        let mut s = Self {
            served: VecDeque::new(),
            tail: Vec::new(),
            last: None,
            liked: Vec::new(),
            skipped: Vec::new(),
            recent: Vec::new(),
            again: None,
            new_pool: Vec::new(),
            turns: 0,
            seed,
        };
        match &s.seed {
            RadioSeed::Track(id) => {
                s.served.push_back(Item::Track(*id));
                s.last = Some(Item::Track(*id));
            }
            RadioSeed::Stream { id, .. } => {
                s.served.push_back(Item::Stream(id.clone()));
            }
            _ => {}
        }
        s
    }

    /// Counts `items` as served: a radio that continues a queue
    /// («Автовоспроизведение») does not pick what the queue already had.
    pub fn mark_served(&mut self, items: impl IntoIterator<Item = Item>) {
        for item in items {
            if !self.served.contains(&item) {
                self.served.push_back(item);
            }
        }
        while self.served.len() > SERVED_CAP {
            self.served.pop_front();
        }
    }

    /// New-music candidates for later extensions (repeats are ignored).
    pub fn add_new(&mut self, tracks: impl IntoIterator<Item = NewTrack>) {
        for t in tracks {
            if !self.new_pool.iter().any(|x| x.id == t.id) {
                self.new_pool.push(t);
            }
        }
        if self.new_pool.len() > NEW_POOL_CAP {
            let served: HashSet<&Item> = self.served.iter().collect();
            // Drop served ones first, then the oldest.
            let mut keep: Vec<NewTrack> = self
                .new_pool
                .drain(..)
                .filter(|t| !served.contains(&Item::Stream(t.id.clone())))
                .collect();
            let over = keep.len().saturating_sub(NEW_POOL_CAP);
            keep.drain(..over);
            self.new_pool = keep;
        }
    }

    /// New-music candidates not served yet.
    pub fn new_left(&self) -> usize {
        let served: HashSet<&Item> = self.served.iter().collect();
        self.new_pool.iter().filter(|t| !served.contains(&Item::Stream(t.id.clone()))).count()
    }

    /// Everything served so far (oldest first, the last [`SERVED_CAP`]).
    pub fn served(&self) -> impl Iterator<Item = &Item> {
        self.served.iter()
    }

    /// Whether the seed is a mix (a mood, a style, a daylist, «Новое для
    /// тебя»): a mix lets what played rest; a radio may bring it back.
    pub fn is_mix(&self) -> bool {
        matches!(self.seed, RadioSeed::Mood(_) | RadioSeed::Style { .. } | RadioSeed::Discover | RadioSeed::Daylist { .. })
    }

    /// How a served track ended: an early skip (< 30 s) pushes later picks
    /// away from its sound, a complete pulls them towards it; a late skip
    /// teaches nothing. A radio's track played twice running plays once
    /// more next.
    pub fn feedback(&mut self, item: Item, outcome: Outcome) {
        if !self.is_mix() && self.recent.last() == Some(&item) && outcome == Outcome::Complete {
            self.again = Some(item.clone());
        }
        self.recent.push(item.clone());
        let over = self.recent.len().saturating_sub(REPEAT_AFTER);
        self.recent.drain(..over);
        match outcome {
            Outcome::Complete => {
                self.skipped.retain(|x| *x != item);
                push_capped(&mut self.liked, item, LESSONS);
            }
            Outcome::Skip { position_ms } if position_ms < crate::taste::EARLY_SKIP_MS => {
                self.liked.retain(|x| *x != item);
                push_capped(&mut self.skipped, item, LESSONS);
            }
            Outcome::Skip { .. } => {}
        }
    }

    fn vector_of(&self, recs: &Recs<'_>, item: &Item) -> Option<Vector> {
        match item {
            Item::Track(id) => recs.embedded(*id),
            Item::Stream(id) => {
                let seed_vec = match &self.seed {
                    RadioSeed::Stream { id: s, vector, .. } if s == id => *vector,
                    _ => None,
                };
                seed_vec
                    .or_else(|| self.new_pool.iter().find(|t| t.id == *id).and_then(|t| t.vector))
                    .map(|v| recs.norm.embed(&v))
            }
        }
    }

    fn slot_of(&self, recs: &Recs<'_>, item: &Item) -> Option<Slot> {
        match item {
            Item::Track(id) => recs.catalog.position(*id).map(|i| recs.slot(i, Reason::SoundsLikeSeed, 0.0)),
            Item::Stream(id) => self.new_pool.iter().find(|t| t.id == *id).map(|t| recs.new_slot(t, 0.0)),
        }
    }

    /// The seed's own point in the weighted space.
    fn seed_vector(&self, recs: &Recs<'_>) -> Option<Vector> {
        match &self.seed {
            RadioSeed::Track(id) => recs.embedded(*id),
            RadioSeed::Stream { vector, .. } => vector.map(|v| recs.norm.embed(&v)),
            RadioSeed::Artist(a) => {
                let mut sum = [0.0f32; DIM];
                let mut w_total = 0.0;
                for &i in recs.catalog.of_artist(*a) {
                    let i = i as usize;
                    if let Some(v) = recs.embedded(recs.info(i).id) {
                        let w = 1.0 + recs.taste.affinity_at(i).max(0.0);
                        for (s, x) in sum.iter_mut().zip(v) {
                            *s += w * x;
                        }
                        w_total += w;
                    }
                }
                (w_total > 0.0).then(|| sum.map(|s| s / w_total))
            }
            RadioSeed::Style { centroid, .. } => <[f32; DIM]>::try_from(centroid.as_slice()).ok(),
            RadioSeed::Daylist { centre, .. } => recs.daylist_point(Axes::from_array(*centre)),
            RadioSeed::Mood(_) | RadioSeed::Discover => None,
        }
    }

    fn seed_artists(&self, recs: &Recs<'_>) -> Vec<ArtistId> {
        match &self.seed {
            RadioSeed::Track(id) => recs.catalog.get(*id).map(|t| t.artists.clone()).unwrap_or_default(),
            RadioSeed::Stream { artists, .. } => artists.clone(),
            RadioSeed::Artist(a) => vec![*a],
            _ => Vec::new(),
        }
    }

    /// The next `n` picks; the state moves on (served, tail, turn).
    pub fn extend(&mut self, recs: &Recs<'_>, n: usize, params: &RadioParams) -> Vec<Pick> {
        if n == 0 {
            return Vec::new();
        }
        let mut rng = Rng::derive(params.seed, self.turns);
        self.turns += 1;
        let share = match self.seed {
            // «Новое для тебя» is new music whatever the mixes pick.
            RadioSeed::Discover => 1.0,
            _ if recs.local_only => 0.0,
            RadioSeed::Stream { .. } => params.new_share.max(0.5),
            _ => params.new_share,
        };
        let seed_vec = self.seed_vector(recs);
        let liked: Vec<Vector> = self.liked.iter().filter_map(|x| self.vector_of(recs, x)).collect();
        let skipped: Vec<Vector> = self.skipped.iter().filter_map(|x| self.vector_of(recs, x)).collect();
        let exempt = match self.seed {
            RadioSeed::Artist(a) => Some(token_of_artist(a)),
            _ => None,
        };
        let rules = Diversity {
            exempt,
            ..Diversity::default()
        };
        let served: HashSet<Item> = self.served.iter().cloned().collect();
        let mut taken: HashSet<Item> = HashSet::new();
        let sound = Sound::new(seed_vec, liked, skipped, library_scale(recs));

        // 0. Played twice running: once more, first.
        let again: Option<Slot> = self.again.take().and_then(|item| match item {
            Item::Track(id) => recs.catalog.position(id).filter(|&i| !recs.blocked(i)).map(|i| recs.slot(i, Reason::SoundsLikeSeed, 0.0)),
            Item::Stream(_) => None,
        });
        if let Some(s) = &again {
            taken.insert(s.item.clone());
        }
        let n = n - usize::from(again.is_some());

        // 1. New music.
        let want_new = new_count(n, share);
        let mut new_slots: Vec<Slot> = self
            .new_pool
            .iter()
            .enumerate()
            .filter(|(_, t)| !served.contains(&Item::Stream(t.id.clone())))
            .map(|(k, t)| {
                let mut s = recs.new_slot(t, 0.0);
                // Nearest to the seed first, then in the order fed.
                let order = k as f32 * 1e-2;
                s.score = match &s.vec {
                    Some(v) => sound.score(v) + rng.f32() * JITTER - order,
                    None => -1.0 - order,
                };
                s
            })
            .collect();
        sort_by_score(&mut new_slots);
        let new_slots = select_diverse(new_slots, want_new, &rules, &mut taken);

        // 2-4. The library.
        let want_lib = n - new_slots.len();
        let mut lib = self.library(recs, want_lib, &sound, &served, true, &rules, &mut rng, &mut taken);
        if lib.len() < want_lib {
            let keep_out = RELAXED_SERVED.min(recs.catalog.len() / 3);
            let recent: HashSet<Item> = self.served.iter().rev().take(keep_out).cloned().collect();
            let more = self.library(recs, want_lib - lib.len(), &sound, &recent, false, &rules, &mut rng, &mut taken);
            lib.extend(more);
        }

        let mut all = new_slots;
        all.extend(lib);
        let mut tail = self.tail.clone();
        let mut out: Vec<Slot> = Vec::with_capacity(n + 1);
        let start = match again {
            Some(s) => {
                tail.push(Placed::of(&s));
                out.push(s.clone());
                Some(s)
            }
            None => self.last.as_ref().and_then(|x| self.slot_of(recs, x)),
        };
        out.extend(arrange(recs, all, &tail, start.as_ref(), &rules));
        for s in &out {
            self.served.push_back(s.item.clone());
            self.tail.push(Placed::of(s));
        }
        while self.served.len() > SERVED_CAP {
            self.served.pop_front();
        }
        let over = self.tail.len().saturating_sub(9);
        self.tail.drain(..over);
        if let Some(s) = out.last() {
            self.last = Some(s.item.clone());
        }
        out.into_iter().map(Slot::pick).collect()
    }

    /// Up to `n` library slots, selected under the diversity caps.
    #[allow(clippy::too_many_arguments)]
    fn library(
        &self,
        recs: &Recs<'_>,
        n: usize,
        sound: &Sound,
        exclude: &HashSet<Item>,
        strict: bool,
        rules: &Diversity,
        rng: &mut Rng,
        taken: &mut HashSet<Item>,
    ) -> Vec<Slot> {
        if n == 0 {
            return Vec::new();
        }
        // A mix leaves what played lately for later; a radio only what
        // played among the last few.
        let mix = self.is_mix();
        let recent: HashSet<&Item> = self.recent.iter().collect();
        let resting = |i: usize, id: TrackId| {
            if mix {
                recs.played_within(i, RECENT_MS)
            } else {
                recent.contains(&Item::Track(id)) || recs.played_within(i, REPEAT_AFTER_MS)
            }
        };
        let ok = |i: usize| {
            let id = recs.info(i).id;
            !recs.blocked(i)
                && !exclude.contains(&Item::Track(id))
                && !taken.contains(&Item::Track(id))
                && self.seed != RadioSeed::Track(id)
                && !(strict && resting(i, id))
        };
        // Candidates with the reason and the relation bonus they came by.
        let mut cands: HashMap<usize, (Reason, f32)> = HashMap::new();
        fn add(cands: &mut HashMap<usize, (Reason, f32)>, i: usize, reason: Reason, bonus: f32) {
            let e = cands.entry(i).or_insert((reason.clone(), bonus));
            if bonus > e.1 {
                *e = (reason, bonus);
            }
        }
        let seed_artists = self.seed_artists(recs);
        let seed_track = match &self.seed {
            RadioSeed::Track(id) => Some(*id),
            _ => None,
        };
        let raw_seed: Option<Vector> = match &self.seed {
            RadioSeed::Track(id) => recs.index.get(*id).copied(),
            RadioSeed::Stream { vector, .. } => *vector,
            _ => None,
        };
        // Sound neighbours of the seed.
        if let Some(q) = raw_seed {
            for (id, _) in recs.index.nearest(&q, SEED_NEIGHBOURS, |id| recs.catalog.position(id).is_some_and(ok)) {
                let i = recs.catalog.position(id).expect("filtered");
                add(&mut cands, i, seed_track.map_or(Reason::SoundsLikeSeed, Reason::SimilarTo), 0.0);
            }
        } else if let Some(v) = sound.seed {
            // An artist's or a style's point: scan by the embedded distance.
            for (i, _) in nearest_embedded(recs, &v, SEED_NEIGHBOURS, &ok) {
                let reason = match &self.seed {
                    RadioSeed::Style { index, .. } => Reason::Style(*index),
                    RadioSeed::Daylist { part, .. } => Reason::DayPart(*part),
                    _ => Reason::SoundsLikeSeed,
                };
                add(&mut cands, i, reason, 0.0);
            }
        }
        for item in &self.liked {
            if let Item::Track(id) = item {
                for (nid, _) in recs.index.neighbours(*id, LIKED_NEIGHBOURS, |k| recs.catalog.position(k).is_some_and(ok)) {
                    add(&mut cands, recs.catalog.position(nid).expect("filtered"), Reason::SoundsLikeSeed, 0.0);
                }
            }
        }
        // The seed's artists and the artists co-listened with them.
        for &a in &seed_artists {
            for &i in recs.catalog.of_artist(a) {
                if ok(i as usize) {
                    add(&mut cands, i as usize, Reason::SameArtist(a), 1.0);
                }
            }
            for &(b, count) in recs.taste.co_listened(a).iter().take(CO_ARTISTS) {
                let mut theirs: Vec<usize> = recs.catalog.of_artist(b).iter().map(|&i| i as usize).filter(|&i| ok(i)).collect();
                theirs.sort_by(|x, y| recs.taste.affinity_at(*y).total_cmp(&recs.taste.affinity_at(*x)).then(x.cmp(y)));
                for i in theirs.into_iter().take(CO_TRACKS) {
                    add(&mut cands, i, Reason::CoListened(a), (count / 3.0).min(1.0) * 0.7);
                }
            }
        }
        match &self.seed {
            RadioSeed::Mood(m) => {
                let pool = recs.mood_pool(*m);
                let (top, bottom) = (pool.first().map_or(0.0, |p| p.1), pool.last().map_or(0.0, |p| p.1));
                let span = (top - bottom).max(1e-3);
                for (id, s) in pool {
                    if let Some(i) = recs.catalog.position(id).filter(|&i| ok(i)) {
                        add(&mut cands, i, Reason::Mood(*m), (s - bottom) / span);
                    }
                }
            }
            RadioSeed::Style { index, artists, .. } => {
                for &a in artists {
                    for &i in recs.catalog.of_artist(a) {
                        if ok(i as usize) {
                            add(&mut cands, i as usize, Reason::Style(*index), 1.0);
                        }
                    }
                }
            }
            RadioSeed::Daylist { part, centre } => {
                for (id, closeness) in recs.daylist_pool(Axes::from_array(*centre)) {
                    if let Some(i) = recs.catalog.position(id).filter(|&i| ok(i)) {
                        add(&mut cands, i, Reason::DayPart(*part), closeness);
                    }
                }
                // Tracks without a sound join by being played in this part.
                for i in 0..recs.catalog.len() {
                    let plays = recs.taste.daypart_plays_at(i);
                    let here = f32::from(plays[part.index()]);
                    if here > 0.0 && ok(i) && !recs.index.contains(recs.info(i).id) {
                        let share = here / plays.iter().map(|p| f32::from(*p)).sum::<f32>();
                        add(&mut cands, i, Reason::DayPart(*part), share);
                    }
                }
            }
            _ => {}
        }
        // Never dry: favourites, then anything.
        if cands.len() < n {
            let mut fav: Vec<usize> = (0..recs.catalog.len()).filter(|&i| recs.taste.affinity_at(i) > 0.0 && ok(i)).collect();
            fav.sort_by(|x, y| recs.taste.affinity_at(*y).total_cmp(&recs.taste.affinity_at(*x)).then(x.cmp(y)));
            for i in fav.into_iter().take(FALLBACK) {
                add(&mut cands, i, Reason::Favourite, 0.0);
            }
        }
        if cands.len() < n && !recs.catalog.is_empty() {
            for _ in 0..FALLBACK {
                let i = rng.below(recs.catalog.len());
                if ok(i) {
                    add(&mut cands, i, Reason::Random, 0.0);
                }
            }
        }

        let (part, w_daypart) = match &self.seed {
            RadioSeed::Daylist { part, .. } => (*part, W_DAYLIST_DAYPART),
            _ => (recs.taste.day_part(), W_DAYPART),
        };
        let rest = if mix { MIX_REST } else { RADIO_REST };
        // Each track its own draw, whatever else is a candidate.
        let jitter = rng.next_u64();
        let mut keys: Vec<usize> = cands.keys().copied().collect();
        keys.sort_unstable();
        let mut slots: Vec<Slot> = keys
            .into_iter()
            .map(|i| {
                let (reason, bonus) = cands[&i].clone();
                let mut s = recs.slot(i, reason, 0.0);
                let snd = match &s.vec {
                    Some(v) => sound.score(v),
                    None => -1.0,
                };
                let aff = (recs.taste.affinity_at(i) / 3.0).tanh();
                let lately = recs.taste.lately_at(i);
                s.score = snd
                    + W_AFFINITY * aff
                    + W_RELATION * bonus
                    + w_daypart * recs.taste.daypart_bonus_at(i, part)
                    - rest * lately
                    + Rng::derive(jitter, i as u64).f32() * JITTER;
                s
            })
            .collect();

        if let RadioSeed::Artist(a) = self.seed {
            // The artist's own tracks by affinity, then the similar rest.
            let own_token = token_of_artist(a);
            let (mut own, mut rest): (Vec<Slot>, Vec<Slot>) = slots.into_iter().partition(|s| s.artists.contains(&own_token));
            for s in &mut own {
                let i = match s.item {
                    Item::Track(id) => recs.catalog.position(id).unwrap_or(0),
                    Item::Stream(_) => 0,
                };
                s.score = (recs.taste.affinity_at(i) / 3.0).tanh() + 0.3 * s.score;
            }
            sort_by_score(&mut own);
            sort_by_score(&mut rest);
            let want_own = ((n as f32) * ARTIST_OWN_SHARE).round() as usize;
            let mut out = select_diverse(own, want_own, rules, taken);
            let left = n - out.len();
            out.extend(select_diverse(rest, left, rules, taken));
            return out;
        }
        sort_by_score(&mut slots);
        select_diverse(slots, n, rules, taken)
    }
}

fn sort_by_score(v: &mut [Slot]) {
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
}

/// The `k` catalog tracks nearest to an embedded point (`ok` ones).
fn nearest_embedded(recs: &Recs<'_>, q: &Vector, k: usize, ok: &dyn Fn(usize) -> bool) -> Vec<(usize, f32)> {
    let mut all: Vec<(usize, f32)> = recs
        .index
        .iter()
        .filter_map(|(id, v)| {
            let i = recs.catalog.position(id)?;
            ok(i).then(|| (i, euclid(&recs.norm.embed(v), q)))
        })
        .collect();
    all.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    all.truncate(k);
    all
}

/// The median distance between tracks of the library, from a sample of
/// about 64 of them (2.0 without analysis).
fn library_scale(recs: &Recs<'_>) -> f32 {
    let step = (recs.index.len() / 64).max(1);
    let vs: Vec<Vector> = recs.index.iter().step_by(step).map(|(_, v)| recs.norm.embed(v)).collect();
    let mut d: Vec<f32> = vs.windows(2).map(|w| euclid(&w[0], &w[1])).collect();
    d.sort_by(f32::total_cmp);
    d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(2.0)
}

/// The sound part of a score: pulled to the seed and the completed tracks,
/// pushed from the early-skipped ones, in units of the typical distance.
struct Sound {
    seed: Option<Vector>,
    liked: Vec<Vector>,
    skipped: Vec<Vector>,
    scale: f32,
}

impl Sound {
    /// The scale is the median distance between the anchors (the seed and
    /// the lessons: an early skip pushes away everything about as close to
    /// it as it is to the seed), `fallback` with fewer than two anchors.
    fn new(seed: Option<Vector>, liked: Vec<Vector>, skipped: Vec<Vector>, fallback: f32) -> Self {
        let scale = {
            let anchors: Vec<&Vector> = seed.iter().chain(&liked).chain(&skipped).collect();
            let mut d: Vec<f32> = Vec::new();
            for (i, a) in anchors.iter().enumerate() {
                for b in &anchors[i + 1..] {
                    d.push(euclid(*a, *b));
                }
            }
            d.sort_by(f32::total_cmp);
            d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(fallback)
        };
        Self {
            seed,
            liked,
            skipped,
            scale,
        }
    }

    fn score(&self, v: &Vector) -> f32 {
        let mut pull = 0.0;
        let mut w = 0.0;
        if let Some(s) = &self.seed {
            pull += euclid(v, s);
            w += 1.0;
        }
        for l in &self.liked {
            pull += LIKED_PULL * euclid(v, l);
            w += LIKED_PULL;
        }
        let attract = if w > 0.0 { -pull / w / self.scale } else { 0.0 };
        let push: f32 = self.skipped.iter().map(|s| (1.0 - euclid(v, s) / self.scale).max(0.0)).sum();
        attract - SKIP_PUSH * push
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::VectorIndex;
    use crate::pick::assert_diverse;
    use crate::taste::{Catalog, Play, Taste, TasteInput, TrackInfo};
    use nikgob_core::model::EventType;
    use crate::test_util::*;
    use nikgob_core::streaming::Service;

    const NOW: i64 = 1_000 * crate::taste::DAY_MS;

    /// Region A around +x, region B around −x, the seed in the middle; 20
    /// artists per region, 3 tracks each.
    fn two_regions() -> (Catalog, VectorIndex<TrackId>) {
        let mut tracks = vec![info(0, &[999], None)];
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut rng = Rng::new(1);
        idx.insert(tid(0), point(0.0, 0.0));
        for i in 1..=120u128 {
            let region_a = i <= 60;
            let artist = ((i - 1) / 3) as u32;
            tracks.push(info(i, &[artist], Some(500 + i)));
            let x = if region_a { 4.0 } else { -4.0 };
            idx.insert(tid(i), point(x + rng.f32() - 0.5, rng.f32() * 2.0 - 1.0));
        }
        (Catalog::new(tracks), idx)
    }

    fn in_a(p: &Pick) -> bool {
        matches!(&p.item, Item::Track(id) if (1..=60).contains(&id.0.as_u128()))
    }

    #[test]
    fn early_skips_push_away_and_completes_pull() {
        let (cat, idx) = two_regions();
        let taste = Taste::build(&cat, &TasteInput::default(), NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let params = RadioParams { new_share: 0.0, seed: 3 };

        let mut free = RadioState::new(RadioSeed::Track(tid(0)));
        let picks = free.extend(&recs, 20, &params);
        let a = picks.iter().filter(|p| in_a(p)).count();
        assert!((5..=15).contains(&a), "both regions without lessons: {a}");

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(tid(5)), Outcome::Skip { position_ms: 10_000 });
        let picks = radio.extend(&recs, 10, &params);
        let a = picks.iter().filter(|p| in_a(p)).count();
        assert!(a <= 1, "an early skip in A moves away from A: {a}");

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(tid(5)), Outcome::Complete);
        let picks = radio.extend(&recs, 10, &params);
        let a = picks.iter().filter(|p| in_a(p)).count();
        assert!(a >= 8, "a complete in A pulls to A: {a}");

        // A late skip teaches nothing (it only counts as played).
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(tid(5)), Outcome::Skip { position_ms: 120_000 });
        let mut fresh = RadioState::new(RadioSeed::Track(tid(0)));
        fresh.recent.push(Item::Track(tid(5)));
        assert_eq!(radio, fresh);
    }

    /// Decision 3 of recs-v2: a radio brings back a favourite played an
    /// hour ago, but not one of the last five; twice running, it plays
    /// first. A mix lets the hour-old one rest.
    #[test]
    fn a_radio_repeats_after_five_and_a_mix_rests() {
        let now = NOW;
        let cat = Catalog::new((0..30u128).map(|i| info(i, &[i as u32], None)));
        let idx = VectorIndex::new();
        let fav = tid(3);
        let input = TasteInput {
            events: vec![
                Play::new(fav, now - 2 * HOUR_MS, EventType::Start, None),
                Play::new(fav, now - HOUR_MS, EventType::Complete, None),
            ],
            likes: vec![fav],
            ..TasteInput::default()
        };
        let taste = Taste::build(&cat, &input, now);
        let recs = Recs::new(&cat, &taste, &idx);
        let params = RadioParams { new_share: 0.0, seed: 1 };
        let has = |picks: &[Pick]| picks.iter().any(|p| p.item == Item::Track(fav));

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        assert!(has(&radio.extend(&recs, 5, &params)), "played an hour ago: it may come back");

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(fav), Outcome::Complete);
        assert!(!has(&radio.extend(&recs, 5, &params)), "one of the last five");
        for k in 20..25 {
            radio.feedback(Item::Track(tid(k)), Outcome::Complete);
        }
        assert!(has(&radio.extend(&recs, 5, &params)), "five others played since");

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(fav), Outcome::Complete);
        radio.feedback(Item::Track(fav), Outcome::Complete);
        assert_eq!(radio.extend(&recs, 5, &params)[0].item, Item::Track(fav), "twice running: once more, first");

        let mut mix = RadioState::new(RadioSeed::Daylist { part: crate::taste::DayPart::Evening, centre: [0.0; 3] });
        assert!(!has(&mix.extend(&recs, 5, &params)), "a mix lets it rest");
    }

    #[test]
    fn radio_never_repeats_survives_serde_and_is_deterministic() {
        let (cat, idx) = two_regions();
        let taste = Taste::build(&cat, &TasteInput::default(), NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let params = RadioParams { new_share: 0.2, seed: 9 };
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.add_new((0..30).map(|k| NewTrack {
            id: RemoteId::new(Service::Youtube, format!("v{k}")),
            artist: format!("new artist {k}"),
            album: None,
            vector: Some(point(0.5, 0.0)),
        }));
        let mut seen = HashSet::new();
        let mut all = Vec::new();
        for _ in 0..4 {
            let json = serde_json::to_string(&radio).unwrap();
            radio = serde_json::from_str(&json).unwrap();
            let picks = radio.extend(&recs, 10, &params);
            assert_eq!(picks.len(), 10);
            assert_eq!(picks.iter().filter(|p| matches!(p.item, Item::Stream(_))).count(), 2, "a fifth is new");
            for p in &picks {
                assert!(seen.insert(p.item.clone()), "{:?} twice", p.item);
                assert_ne!(p.item, Item::Track(tid(0)), "the seed is not repeated");
            }
            all.extend(picks);
        }
        assert_diverse(&recs, &all);
        // The same seed, the same radio.
        let mut again = RadioState::new(RadioSeed::Track(tid(0)));
        again.add_new((0..30).map(|k| NewTrack {
            id: RemoteId::new(Service::Youtube, format!("v{k}")),
            artist: format!("new artist {k}"),
            album: None,
            vector: Some(point(0.5, 0.0)),
        }));
        let first: Vec<Pick> = (0..4).flat_map(|_| again.extend(&recs, 10, &params)).collect();
        assert_eq!(first, all);
    }

    #[test]
    fn a_small_library_keeps_playing() {
        let cat = Catalog::new((0..8u128).map(|i| info(i, &[i as u32], None)));
        let idx = VectorIndex::new();
        let taste = Taste::build(&cat, &TasteInput::default(), NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        for _ in 0..20 {
            assert_eq!(radio.extend(&recs, 5, &RadioParams { new_share: 0.0, seed: 1 }).len(), 5);
        }
    }

    #[test]
    fn artist_radio_is_mostly_the_artist() {
        let (cat, idx) = {
            // Artist 7 has 40 tracks; 60 others by 30 artists around them.
            let mut tracks: Vec<TrackInfo> = (0..40u128).map(|i| info(i, &[7], Some(900 + i / 4))).collect();
            tracks.extend((40..100u128).map(|i| info(i, &[100 + (i as u32 % 30)], None)));
            let mut idx = VectorIndex::with_weights([1.0; DIM]);
            for i in 0..100u128 {
                idx.insert(tid(i), point((i % 10) as f32, (i / 10) as f32));
            }
            (Catalog::new(tracks), idx)
        };
        let taste = Taste::build(&cat, &TasteInput::default(), NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let mut radio = RadioState::new(RadioSeed::Artist(aid(7)));
        let picks = radio.extend(&recs, 20, &RadioParams { new_share: 0.0, seed: 5 });
        let own = picks
            .iter()
            .filter(|p| matches!(&p.item, Item::Track(id) if id.0.as_u128() < 40))
            .count();
        assert_eq!(own, 14, "70% of 20");
    }

    #[test]
    fn moods_take_their_tail_and_skip_dislikes() {
        // Energy grows with the track number (tempo, ZCR, centroid,
        // loudness together); half of the tracks liked.
        let n = 90u128;
        let cat = Catalog::new((0..n).map(|i| info(i, &[i as u32], None)));
        let mut idx = VectorIndex::new();
        let mut rng = Rng::new(4);
        for i in 0..n {
            let e = i as f32 / n as f32;
            let mut v = [0.0f32; DIM];
            for f in v.iter_mut() {
                *f = rng.f32() * 0.002;
            }
            for f in [0, 1, 2, 8] {
                v[f] += e;
            }
            idx.insert(tid(i), v);
        }
        let input = TasteInput {
            disliked_tracks: vec![tid(89)],
            ..Default::default()
        };
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let pool = recs.mood_pool(Mood::Energetic);
        assert_eq!(pool.len(), 30, "a third (the disliked one left out)");
        assert!(pool.iter().all(|(id, _)| id.0.as_u128() >= 59 && id.0.as_u128() != 89));
        let mut energetic = RadioState::new(RadioSeed::Mood(Mood::Energetic));
        let picks = energetic.extend(&recs, 15, &RadioParams { new_share: 0.0, seed: 2 });
        assert_eq!(picks.len(), 15);
        for p in &picks {
            let Item::Track(id) = &p.item else { panic!() };
            assert!(id.0.as_u128() >= 59, "energetic tail: {}", id.0.as_u128());
            assert_eq!(p.reason, Reason::Mood(Mood::Energetic));
        }
        let mut calm = RadioState::new(RadioSeed::Mood(Mood::Calm));
        for p in calm.extend(&recs, 15, &RadioParams { new_share: 0.0, seed: 2 }) {
            let Item::Track(id) = &p.item else { panic!() };
            assert!(id.0.as_u128() < 30, "calm tail: {}", id.0.as_u128());
        }
    }

    #[test]
    fn mood_scoring_prefers_affinity() {
        // A flat mood pool: the liked half wins.
        let n = 300u128;
        let cat = Catalog::new((0..n).map(|i| info(i, &[i as u32], None)));
        let mut idx = VectorIndex::new();
        let mut rng = Rng::new(8);
        for i in 0..n {
            idx.insert(tid(i), std::array::from_fn(|_| rng.f32()));
        }
        let likes: Vec<TrackId> = (0..n).filter(|i| i % 2 == 0).map(tid).collect();
        let taste = Taste::build(&cat, &TasteInput { likes, ..Default::default() }, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let mut radio = RadioState::new(RadioSeed::Mood(Mood::Focus));
        let picks = radio.extend(&recs, 20, &RadioParams { new_share: 0.0, seed: 6 });
        let liked = picks.iter().filter(|p| matches!(&p.item, Item::Track(id) if id.0.as_u128() % 2 == 0)).count();
        assert!(liked >= 15, "{liked}");
    }
}
