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
//!    relation to the seed, the day part, minus the dislikes near it
//!    ([`crate::dislike`]), and a seeded jitter;
//! 4. selects the best under the diversity caps and orders them smoothly
//!    from the last track served.
//!
//! Repeats (recs-v2 §17, decision 3): a radio never serves a pick twice,
//! and leaves out what played in its session or in the last few hours
//! («Не повторять сыгранное», [`RadioState::set_no_repeat_ms`], three
//! hours by default) until nothing else is left; one played twice running
//! plays once more first; a mix leaves what played in the last three
//! hours out. Both score what played lately less, a mix enough to let it
//! rest about a day and a half ([`MIX_REST`]).
//!
//! A track's radio (and autoplay, which is one) picks by the user model
//! when the engine has it ([`crate::model`], recs-v2 §5): one track at a
//! time as if the ones before it played, among the candidates of the
//! session (§5.2), by its value (§5.3) — the model's relevance, plus
//! `1.5·ln(1 − p_skip)`, plus [`RADIO_REST`] times how rested it is (1.5
//! days to half), minus the dislikes near it, a little seeded chance.
//! «Похоже на X» stays X's: its candidates are within X's sound radius
//! ([`SEED_RADIUS`] median distances, nearer in a big library) and of a
//! kind of song that goes with X's, the anchor of the sound is X's for
//! [`SEED_HOLD`] and the session's for the rest, so affinity only orders
//! what is near. The diversity rules always hold (artist, album, world);
//! the next one on the album goes on only right after the user's own start
//! from an album. After two early skips running the model weighs the
//! skipped sounds' push and the session's lane twice ([`SKIP_SHIFT`]):
//! the radio turns to another part of X's neighbourhood, never out of it.
//! A seed with few neighbours of its own scene (its artists, what the
//! graph links to them, its language) keeps to that scene before anything
//! else: its artists' and their graph neighbours' tracks of any sound,
//! then its language of any sound, and only then the rest. A track seed
//! the catalog lacks (a stream played from its album's page) is known by
//! what the runtime tells of it ([`SeedFacts`]: its artists, sound and
//! language).
//! The session is the runtime's own, handed to the radio before it picks
//! ([`RadioState::attune`]), else the radio's tracks.
//!
//! Two of the user's knobs hold for a radio of a track or an artist (and
//! autoplay, which is one); neither ever stops a radio:
//! * «Подмешивать случайное» ([`RadioState::set_explore`]): new music whose
//!   sound is not known to be near the seed (within its sound radius) is a
//!   blind pick, at most that share of an extension (and of the queue new
//!   music joins later, [`RadioState::take_new`]); new music that sounds
//!   like the seed fills the share of new music first.
//! * «Преимущественно новые песни» ([`RadioState::set_unheard`]): that
//!   share of the library's picks at least is unheard by the user (never
//!   started nor played, not liked), each among what the radio would pick
//!   from anyway (the seed's scene, its sound radius, the profile), so its
//!   rules hold. With all of them unheard («Только новые») what had to be
//!   heard makes way for new music that sounds like the seed, while there
//!   is; with none left either, the usual picks.
//!
//! It keeps to a taste profile (profiles-v1 §4.3, §7): «Похоже на X» to
//! X's, autoplay ([`RadioState::continuing`]) to the one its session is
//! pure in if X belongs to it (else X's), to none after a mixed session (then in blocks of three or four
//! of a profile). A track of the profile is one with [`MEMBER`] of its
//! soft membership; the radio stays in it until fewer than
//! [`UNHEARD_MIN`] are left to play or two early skips run, then the
//! neighbouring profile (the nearest of a kind the user goes on to) joins.
//! In a background session it never widens, takes only what suits the
//! background ([`crate::background`]) from a profile without words, keeps
//! to the middle of the profile's energy, steps softly, brings new music
//! only after 20 minutes and one in six at most, of the profile's sound;
//! played through, what rested a day and a half comes back. Focused, it
//! steps more freely, brings half again as much new music and a world a
//! fifth at most.
//!
//! An artist's radio is ~70% the artist's own tracks by affinity
//! ([`ARTIST_OWN_SHARE`]), the rest similar by sound and co-listening; the
//! artist rule of diversity does not apply to that artist. A daylist
//! ([`crate::daylist`]) draws from the tracks near its centre on the mood
//! axes and weighs the bonus of its own part of the day double.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

use nikgob_core::model::{ArtistId, TrackId};
use nikgob_core::streaming::RemoteId;
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::index::euclid;
use crate::model::{F_COOC, F_GRAPH, F_NEXT_ALBUM, F_PGRAPH, F_SAME_LANE, F_SKIP_PUSH, F_SOUND, HAND, UserModel, Weights};
use crate::mood::Mood;
use crate::pick::{Diversity, Item, NewTrack, Pick, Placed, Reason, Recs, Seen, Slot, arrange, new_count, select_diverse, select_more, take, token_of_artist};
use crate::rng::Rng;
use crate::mood::Axes;
use crate::listening::Mode;
use crate::profiles::LaneKey;
use crate::session::{Ended, Session};
use crate::songtype::{SongLane, SongType, Voice};

use crate::taste::{DayPart, HOUR_MS};

/// Served items remembered (never repeated while remembered).
const SERVED_CAP: usize = 2_000;
/// When the library runs dry, only this many last served stay excluded.
const RELAXED_SERVED: usize = 50;
/// A mix leaves tracks played this recently for later (unless the pool is
/// dry).
const RECENT_MS: i64 = 3 * HOUR_MS;
/// Within a radio's own picks a track rests until this many others
/// played since it…
const REPEAT_AFTER: usize = 5;
/// …and what played this long ago or less is not picked again (the
/// default of «Не повторять сыгранное»), unless nothing else is left; one
/// played twice running comes back at once.
pub const NO_REPEAT_MS: i64 = 3 * HOUR_MS;
/// A seed with fewer unheard tracks of its own scene near it than this (or
/// whose sound's neighbours are mostly of other scenes) keeps to its scene
/// first.
const SPARSE_SCENE: usize = 10;
/// A model pick's value: the early skip's chance weighs this much…
const SKIP_VALUE: f32 = 1.5;
/// …a rest this long halves the bonus of being rested…
const REST_HALF_MS: i64 = 36 * HOUR_MS;
/// …and chance this much.
const MODEL_JITTER: f32 = 0.15;
/// After this many early skips running the radio leaves their sound: the
/// push of skipped sounds and the session's lane weigh twice.
pub const SKIP_SHIFT: u8 = 2;
/// A simulated track of the session lasts this long.
const SIM_MS: i64 = 200_000;

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
    /// A world it keeps out («После <мир>»: its sound from other worlds).
    #[serde(default)]
    not_world: Option<u64>,
    new_pool: Vec<NewTrack>,
    turns: u64,
    /// The listening session it continues ([`Self::attune`]).
    #[serde(default)]
    session: Session,
    /// «Автовоспроизведение»: the profile it keeps to is the session's
    /// ([`Self::continuing`]), not its seed's.
    #[serde(default)]
    follow: bool,
    /// It left its profile for the neighbouring one: two early skips ran,
    /// or its own ran out (profiles-v1 §7).
    #[serde(default)]
    wide: bool,
    /// The weights learnt since the engine's model was built.
    #[serde(skip)]
    learned: Option<Weights>,
    /// A track seed the catalog lacks, as the runtime knows it.
    #[serde(default)]
    seed_facts: Option<SeedFacts>,
    /// What played this long ago or less is not picked again.
    #[serde(default = "no_repeat_default")]
    no_repeat_ms: i64,
    /// «Подмешивать случайное»: the share of an extension at most that is
    /// new music not known to sound like the seed (`None`: as much as the
    /// share of new music).
    #[serde(default)]
    explore: Option<f32>,
    /// «Преимущественно новые песни»: the share of the library's picks at
    /// least that the user has not heard, while there are.
    #[serde(default)]
    unheard: f32,
}

fn no_repeat_default() -> i64 {
    NO_REPEAT_MS
}

/// What the runtime knows of a track seed the catalog lacks: its artists,
/// its sound (raw bliss) and the language its title shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SeedFacts {
    pub artists: Vec<ArtistId>,
    pub vector: Option<Vector>,
    pub lane: Option<SongLane>,
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
            not_world: None,
            new_pool: Vec::new(),
            turns: 0,
            session: Session::default(),
            follow: false,
            wide: false,
            learned: None,
            seed_facts: None,
            no_repeat_ms: NO_REPEAT_MS,
            explore: None,
            unheard: 0.0,
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

    /// The listening session to continue and the weights learnt since the
    /// engine's model was built (the runtime's, before each extension).
    pub fn attune(&mut self, session: Session, learned: Option<Weights>) {
        self.session = session;
        self.learned = learned;
    }

    /// The radio continues a queue («Автовоспроизведение»): it keeps to the
    /// profile the session is pure in, or to none in a mixed one.
    #[must_use]
    pub fn continuing(mut self) -> Self {
        self.follow = true;
        self
    }

    /// What the runtime knows of a track seed the catalog lacks.
    #[must_use]
    pub fn with_seed_facts(mut self, facts: SeedFacts) -> Self {
        self.seed_facts = Some(facts);
        self
    }

    /// What the runtime knows of a track seed the catalog lacks, told once
    /// it can (autoplay may start before the first snapshot).
    pub fn set_seed_facts(&mut self, facts: SeedFacts) {
        self.seed_facts = Some(facts);
    }

    /// The runtime has told what it knows of the seed.
    pub fn has_seed_facts(&self) -> bool {
        self.seed_facts.is_some()
    }

    /// «Не повторять сыгранное»: what played this long ago or less (or in
    /// the session) is not picked again while anything else is left.
    pub fn set_no_repeat_ms(&mut self, ms: i64) {
        self.no_repeat_ms = ms.max(0);
    }

    /// «Подмешивать случайное»: at most `share` of a radio of a track or an
    /// artist is new music not known to sound like the seed (`None`: no
    /// limit but the share of new music).
    pub fn set_explore(&mut self, share: Option<f32>) {
        self.explore = share.map(|s| s.clamp(0.0, 1.0));
    }

    /// «Преимущественно новые песни»: at least `share` of the library's
    /// picks of a radio of a track or an artist is unheard by the user,
    /// while there are (1: all, and new music that sounds like the seed
    /// before what was heard).
    pub fn set_unheard(&mut self, share: f32) {
        self.unheard = share.clamp(0.0, 1.0);
    }

    /// The knobs hold for a radio of a track or an artist: a mix, a
    /// service's song and «Новое для тебя» go by their own share.
    fn knobs_hold(&self) -> bool {
        matches!(self.seed, RadioSeed::Track(_) | RadioSeed::Artist(_))
    }

    /// The share of blind new music allowed, when limited.
    fn explore_cap(&self) -> Option<f32> {
        self.explore.filter(|_| self.knobs_hold())
    }

    /// The share of unheard library picks wanted.
    fn unheard_share(&self) -> f32 {
        if self.knobs_hold() { self.unheard } else { 0.0 }
    }

    /// Where new music sounds like the seed: the seed's point and its
    /// sound radius (none for a seed without a sound: nothing is known to).
    fn fit_zone(&self, recs: &Recs<'_>) -> Option<(Vector, f32)> {
        match (&self.seed, recs.model) {
            (RadioSeed::Track(t), Some(m)) => {
                let a = SeedAnchor::of(m, Some(*t), self.seed_facts.as_ref());
                a.point.map(|p| (p, a.radius))
            }
            _ => self.seed_vector(recs).map(|p| (p, SEED_RADIUS * library_scale(recs))),
        }
    }

    /// New music of `slots` that sounds like the seed, and the blind rest
    /// (all of it counts as sounding like the seed while blind picks are
    /// not limited). Both keep their order.
    fn split_fit(&self, recs: &Recs<'_>, slots: Vec<Slot>) -> (Vec<Slot>, Vec<Slot>) {
        if self.explore_cap().is_none() {
            return (slots, Vec::new());
        }
        let zone = self.fit_zone(recs);
        slots.into_iter().partition(|s| zone.as_ref().is_some_and(|(p, r)| s.vec.as_ref().is_some_and(|v| euclid(v, p) <= *r)))
    }

    /// How long the session it continues has played.
    fn session_ms(&self) -> i64 {
        match (self.session.tracks().next(), self.session.last_ts()) {
            (Some(first), Some(last)) => last - first.ts,
            _ => 0,
        }
    }

    /// Keeps the tracks of a world out ([`crate::world_token`]).
    pub fn without_world(mut self, world: u64) -> Self {
        self.not_world = Some(world);
        self
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

    /// Up to `k` new-music picks now, the nearest to the seed first, served
    /// at once: new music that came after the queue was picked joins it
    /// (the runtime puts them among the `of` picks queued, `have` of them
    /// new music already). Blind ones stay within «Подмешивать случайное»
    /// of `of`, the new music queued counted as blind.
    pub fn take_new(&mut self, recs: &Recs<'_>, k: usize, of: usize, have: usize) -> Vec<Pick> {
        let served: HashSet<Item> = self.served.iter().cloned().collect();
        let seed_vec = self.seed_vector(recs);
        let mut slots: Vec<Slot> = self
            .new_pool
            .iter()
            .enumerate()
            .filter(|(_, t)| !served.contains(&Item::Stream(t.id.clone())))
            .map(|(order, t)| {
                let mut s = recs.new_slot(t, 0.0);
                let near = match (&s.vec, &seed_vec) {
                    (Some(v), Some(q)) => -euclid(v, q),
                    _ => -1e3,
                };
                s.score = near - order as f32 * 1e-3;
                s
            })
            .collect();
        sort_by_score(&mut slots);
        let blind_max = self.explore_cap().map_or(usize::MAX, |e| new_count(of, e).saturating_sub(have));
        let (fit, blind) = self.split_fit(recs, slots);
        let mut taken = HashSet::new();
        let mut picked = select_diverse(fit, k, &Diversity::default(), &mut taken);
        let more = (k - picked.len()).min(blind_max);
        let blind = select_more(blind, more, k, &Diversity::default(), &mut taken, &picked);
        picked.extend(blind);
        for s in &picked {
            self.served.push_back(s.item.clone());
        }
        picked.into_iter().map(Slot::pick).collect()
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
        if let Item::Track(t) = &item {
            let open = self.session.tracks().rev().any(|p| p.track == *t && p.ended == Ended::Open);
            if !open {
                let ts = self.session.last_ts().map_or(0, |l| l + SIM_MS);
                self.session.start(*t, ts, nikgob_core::model::EventContext::Radio, None);
            }
            let ended = match outcome {
                Outcome::Complete => Ended::Complete,
                Outcome::Skip { position_ms } if position_ms < crate::taste::EARLY_SKIP_MS => Ended::Early,
                Outcome::Skip { .. } => Ended::Late,
            };
            self.session.end(*t, ended);
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
            RadioSeed::Track(id) => {
                recs.embedded(*id).or_else(|| self.seed_facts.as_ref().and_then(|f| f.vector).map(|v| recs.norm.embed(&v)))
            }
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
            RadioSeed::Track(id) => recs
                .catalog
                .get(*id)
                .map(|t| t.artists.clone())
                .or_else(|| self.seed_facts.as_ref().map(|f| f.artists.clone()))
                .unwrap_or_default(),
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
        // A track's radio by the model keeps to a profile in the mode of
        // its session: in the background new music comes late and seldom,
        // focused more of it.
        let plan = match (&self.seed, recs.model) {
            (RadioSeed::Track(_), Some(m)) => Some(self.plan(m)),
            _ => None,
        };
        let share = match &plan {
            Some(p) if p.background => {
                if self.session_ms() >= BG_NEW_AFTER_MS { share.min(BG_NEW_SHARE) } else { 0.0 }
            }
            Some(p) if p.focus => (share * FOCUS_NEW).min(1.0),
            _ => share,
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
            .filter(|(_, t)| match (&plan, recs.model) {
                (Some(p), Some(m)) if p.background => p.quiet_new(m, t.vector.map(|v| recs.norm.embed(&v)).as_ref()),
                _ => true,
            })
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
        // What sounds like the seed first, blind picks within «Подмешивать
        // случайное»; with all of the library unheard, the rest of what
        // sounds like the seed waits to stand in for what was heard (not in
        // the background, where new music stays one in six at most).
        let (fit, blind) = self.split_fit(recs, new_slots);
        let stand_in = self.unheard_share() >= 1.0 && share > 0.0 && plan.as_ref().is_none_or(|p| !p.background);
        let reserve: Vec<Slot> = if stand_in { fit.clone() } else { Vec::new() };
        let mut new_slots = select_diverse(fit, want_new, &rules, &mut taken);
        let blind_max = self.explore_cap().map_or(usize::MAX, |e| new_count(n, e));
        let more = (want_new - new_slots.len()).min(blind_max);
        let blind = select_more(blind, more, want_new, &rules, &mut taken, &new_slots);
        new_slots.extend(blind);

        // 2-4. The library: a track's radio by the user model, in its order.
        let want_lib = n - new_slots.len();
        if let (Some(plan), Some(model)) = (&plan, recs.model) {
            let (mut lib, wide) = self.by_model(recs, model, plan, want_lib, &served, &rules, &mut rng, &mut taken, again.as_ref());
            self.wide |= wide;
            if lib.len() < want_lib {
                let more = self.library(recs, want_lib - lib.len(), &sound, &served, false, &rules, &mut rng, &mut taken);
                lib.extend(more);
            }
            make_way(recs, &mut lib, &mut new_slots, reserve, &rules, &mut taken);
            // New music between, evenly.
            let mut out: Vec<Slot> = again.into_iter().collect();
            let every = (lib.len() + new_slots.len()) / (new_slots.len() + 1);
            let mut new = new_slots.into_iter();
            for (k, s) in lib.into_iter().enumerate() {
                out.push(s);
                if every > 0 && (k + 1) % every == 0 {
                    out.extend(new.next());
                }
            }
            out.extend(new);
            return self.serve(out);
        }
        let mut lib = self.library(recs, want_lib, &sound, &served, true, &rules, &mut rng, &mut taken);
        if lib.len() < want_lib {
            let keep_out = RELAXED_SERVED.min(recs.catalog.len() / 3);
            let recent: HashSet<Item> = self.served.iter().rev().take(keep_out).cloned().collect();
            let more = self.library(recs, want_lib - lib.len(), &sound, &recent, false, &rules, &mut rng, &mut taken);
            lib.extend(more);
        }
        make_way(recs, &mut lib, &mut new_slots, reserve, &rules, &mut taken);

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
        self.serve(out)
    }

    /// `out` is served: the state moves on.
    fn serve(&mut self, out: Vec<Slot>) -> Vec<Pick> {
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

    /// Up to `n` picks of a track's radio by the user model, in order (see
    /// the module docs): each the best by value among the candidates of
    /// the session as if the picks before it played to the end.
    #[allow(clippy::too_many_arguments)]
    fn by_model(
        &self,
        recs: &Recs<'_>,
        model: &UserModel,
        plan: &Plan,
        n: usize,
        served: &HashSet<Item>,
        rules: &Diversity,
        rng: &mut Rng,
        taken: &mut HashSet<Item>,
        again: Option<&Slot>,
    ) -> (Vec<Slot>, bool) {
        let weights = self.learned.unwrap_or(model.weights);
        // A focused session's radio: a world a fifth at most.
        let rules = &if plan.focus { Diversity { world_share: FOCUS_WORLD_SHARE, ..*rules } } else { *rules };
        let mut sim = self.session.clone();
        // A run of early skips turns the whole batch away (the simulated
        // plays after it would end the run at once).
        let shifted = sim.skip_streak() >= SKIP_SHIFT;
        // The session goes on from its last start (a session of the radio's
        // own tracks has no clock: it starts from 0).
        let mut ts = sim.last_ts().unwrap_or(recs.now());
        let play = |sim: &mut Session, ts: &mut i64, t: TrackId| {
            *ts += SIM_MS;
            sim.start(t, *ts, nikgob_core::model::EventContext::Radio, None);
            sim.end(t, Ended::Complete);
        };
        // What is queued after the track playing plays first.
        let order: Vec<TrackId> = self.served.iter().filter_map(|x| match x {
            Item::Track(t) => Some(*t),
            Item::Stream(_) => None,
        }).collect();
        let queued: Vec<TrackId> = match sim.last().and_then(|p| order.iter().rposition(|t| *t == p.track)) {
            Some(k) => order[k + 1..].to_vec(),
            None => match &self.last {
                Some(Item::Track(t)) => vec![*t],
                _ => Vec::new(),
            },
        };
        for t in queued.into_iter().chain(again.and_then(|s| match s.item {
            Item::Track(t) => Some(t),
            Item::Stream(_) => None,
        })) {
            play(&mut sim, &mut ts, t);
        }
        let recent: HashSet<&Item> = self.recent.iter().collect();
        let seed = match &self.seed {
            RadioSeed::Track(t) => Some(*t),
            _ => None,
        };
        // What the session played, and what played lately, rest while
        // anything else is left («Не повторять сыгранное»).
        let in_session: HashSet<TrackId> = self.session.tracks().map(|p| p.track).collect();
        let base = |i: usize| {
            let t = recs.info(i);
            let item = Item::Track(t.id);
            !recs.blocked(i)
                && !served.contains(&item)
                && seed != Some(t.id)
                && (self.not_world.is_none() || t.world != self.not_world)
                && !recent.contains(&item)
        };
        let ok = |i: usize| base(i) && !in_session.contains(&recs.info(i).id) && !recs.played_within(i, self.no_repeat_ms);
        // «Похоже на X» stays near X: within its sound radius and of a
        // kind of song that goes with it; the session only leans it.
        let anchor = SeedAnchor::of(model, seed, self.seed_facts.as_ref());
        // What the collective graph links to the seed goes too, beyond the
        // radius of its sound (a kind of song that goes with it).
        let linked = seed_links(model, seed);
        let near = |i: usize| anchor.holds(model, i) || (linked.get(&i).is_some_and(|w| *w >= LINKED) && anchor.goes(model, i));
        // The seed's scene: its artists' tracks and what the graph links to
        // them, and its language. A seed with few unheard tracks of it near
        // keeps to it first, of any sound, before it crosses to another.
        let kin = scene_of(model, seed, &self.seed_artists(recs), &linked);
        let lane = anchor.lane.filter(|l| *l != SongLane::Unknown);
        let same_lane = |i: usize| lane.is_some_and(|l| model.song_type_at(i).lane() == l);
        let in_scene = |i: usize| kin.contains_key(&i) || same_lane(i);
        let knows_scene = !kin.is_empty() || lane.is_some();
        let sparse = knows_scene && {
            let (mut of_scene, mut all) = (0usize, 0usize);
            for i in (0..model.len()).filter(|&i| ok(i) && near(i)) {
                all += 1;
                of_scene += usize::from(in_scene(i));
            }
            of_scene < SPARSE_SCENE || 2 * of_scene < all
        };
        // The profile it keeps to (profiles-v1 §7): its own tracks, and the
        // neighbouring profile's once two early skips ran or fewer than
        // [`UNHEARD_MIN`] of its own are left; a background radio never
        // widens, takes only what suits the background from a profile
        // without words, and keeps to its band of energy.
        let neighbour = plan.profile.filter(|_| !plan.background).and_then(|j| neighbour(model, j));
        let (m_own, m_next): (Vec<f32>, Vec<f32>) = match plan.profile {
            Some(j) => (0..model.len())
                .map(|i| {
                    let m = model.membership_at(i);
                    (m.get(j).copied().unwrap_or(0.0), neighbour.and_then(|o| m.get(o).copied()).unwrap_or(0.0))
                })
                .unzip(),
            None => (Vec::new(), Vec::new()),
        };
        let own = |i: usize| plan.profile.is_none() || m_own[i] >= MEMBER;
        let fresh = || (0..model.len()).filter(|&i| m_own[i] >= MEMBER && ok(i) && anchor.goes(model, i)).count();
        let wide = neighbour.is_some() && (self.wide || shifted || fresh() < UNHEARD_MIN);
        // Widened, the profile beyond the seed's neighbourhood joins too.
        let next = |i: usize| wide && (m_next[i] >= MEMBER || m_own[i] >= MEMBER) && anchor.goes(model, i);
        let quiet = |i: usize| {
            !plan.background
                || ((!plan.wordless || recs.suits_background(i))
                    && plan.band.is_none_or(|(lo, hi)| model.energy_at(i).is_none_or(|e| (lo..=hi).contains(&e))))
        };
        // A mixed session's radio goes in blocks of a profile.
        let blocks = plan.profile.is_none() && !plan.background;
        // The album goes on only from the user's own album, in its order
        // while each pick is the next on it; a world never over the rules.
        let mut continuing =
            self.session.last().is_some_and(|p| p.context == nikgob_core::model::EventContext::Album) && self.session.album_session(model);
        let mut tail: Vec<Placed> = self.tail.clone();
        let mut per_world: HashMap<u64, usize> = HashMap::new();
        let world_cap = rules.world_cap(n);
        let jitter = rng.next_u64();
        // «Преимущественно новые песни»: picks unheard so far.
        let unheard_share = self.unheard_share();
        let mut fresh = 0usize;
        let mut out: Vec<Slot> = Vec::with_capacity(n);
        for _ in 0..n {
            let mut m = model.moment(&sim, ts);
            m.album_session &= continuing;
            m.world_session = None;
            m.anchor = anchor.lean(m.anchor);
            // What the session played lately rests: back once
            // [`REPEAT_AFTER`] others played since it, at once when it
            // played twice running.
            let twice = sim.repeat_one().then(|| sim.last().map(|p| p.track)).flatten();
            let lately: HashSet<TrackId> = sim.tracks().rev().take(REPEAT_AFTER).map(|p| p.track).filter(|t| Some(*t) != twice).collect();
            let free = |i: usize| {
                let t = recs.info(i);
                !taken.contains(&Item::Track(t.id)) && !t.song.is_some_and(|s| taken.contains(&Item::Track(s))) && !lately.contains(&t.id)
            };
            // This pick is to be unheard: the set of candidates it is taken
            // from below keeps its unheard part apart (the nearest unheard,
            // not only the unheard of the nearest), so whatever may follow
            // stays as it is.
            let want_unheard = unheard_share > 0.0 && (fresh as f32) < unheard_share * (out.len() + 1) as f32 - 1e-3;
            let unheard: RefCell<Vec<usize>> = RefCell::new(Vec::new());
            let gather = |f: &dyn Fn(usize) -> bool| -> Vec<usize> {
                let c = model.candidates(&m, f);
                if want_unheard && !c.is_empty() {
                    *unheard.borrow_mut() = model.candidates(&m, &|i| f(i) && !recs.heard(i));
                }
                c
            };
            // A sparse scene first: what is near of it, then its artists'
            // and their neighbours' tracks of any sound, then its language.
            let mut cands = if sparse {
                let mut c = gather(&|i| ok(i) && free(i) && quiet(i) && in_scene(i) && near(i));
                if c.is_empty() {
                    c = kin.keys().copied().filter(|&i| ok(i) && free(i) && quiet(i)).collect();
                    c.sort_unstable();
                    if want_unheard {
                        *unheard.borrow_mut() = c.iter().copied().filter(|&i| !recs.heard(i)).collect();
                    }
                }
                if c.is_empty() {
                    c = gather(&|i| ok(i) && free(i) && quiet(i) && same_lane(i));
                }
                c
            } else {
                Vec::new()
            };
            if cands.is_empty() {
                cands = gather(&|i| ok(i) && free(i) && quiet(i) && ((own(i) && near(i)) || next(i)));
            }
            if cands.is_empty() && plan.profile.is_some() {
                // The profile beyond the seed's neighbourhood.
                cands = gather(&|i| ok(i) && free(i) && quiet(i) && own(i) && anchor.goes(model, i));
            }
            if cands.is_empty() && plan.background {
                // A background profile played through: what rested a day
                // and a half comes back.
                cands = gather(&|i| {
                    own(i) && quiet(i) && free(i) && !recs.blocked(i) && seed != Some(recs.info(i).id) && !recs.played_within(i, BG_REPEAT_MS)
                });
            }
            if cands.is_empty() {
                cands = gather(&|i| ok(i) && free(i) && near(i));
            }
            if cands.is_empty() && knows_scene {
                // Its scene of any sound before another scene.
                cands = gather(&|i| ok(i) && free(i) && in_scene(i));
            }
            if cands.is_empty() {
                cands = gather(&|i| ok(i) && free(i));
            }
            if cands.is_empty() {
                // Nothing unplayed is left: what played lately comes back.
                cands = gather(&|i| base(i) && free(i) && (!knows_scene || in_scene(i)));
            }
            if cands.is_empty() {
                cands = gather(&|i| base(i) && free(i));
            }
            if cands.is_empty() {
                break;
            }
            let mut w = weights.next;
            if !continuing {
                w.w[F_NEXT_ALBUM] = 0.0;
            }
            if shifted || m.skip_streak >= SKIP_SHIFT {
                w.w[F_SKIP_PUSH] = 2.0 * w.w[F_SKIP_PUSH].max(HAND[F_SKIP_PUSH]);
                w.w[F_SAME_LANE] = 2.0 * w.w[F_SAME_LANE].max(HAND[F_SAME_LANE]);
            }
            // A jump in sound costs twice in the background, half focused.
            if plan.background {
                w.w[F_SOUND] *= JUMP_BG;
            } else if plan.focus {
                w.w[F_SOUND] *= JUMP_FOCUS;
            }
            // The block going on: its profile and how long it ran.
            let (block, run) = if blocks {
                let ps: Vec<Option<usize>> =
                    sim.tracks().rev().filter(|p| p.ended != Ended::Early).map(|p| model.position(p.track).and_then(|i| model.profile_at(i))).collect();
                let first = ps.first().copied().flatten();
                (first, ps.iter().take_while(|p| first.is_some() && **p == first).count())
            } else {
                (None, 0)
            };
            let seq: Vec<Seen<'_>> = tail.iter().map(|p| (p.artists.as_slice(), p.album, p.world)).collect();
            let best_of = |cands: Vec<usize>| -> Option<((usize, f32), Slot, bool)> {
                let mut best: Option<((usize, f32), Slot, bool)> = None;
                for c in cands {
                    let f = model.features(&m, c);
                    let parts: Vec<f32> = w.w.iter().zip(&f).map(|(a, b)| a * b).collect();
                    let relevance: f32 = parts.iter().sum();
                    let mut x = model.skip_features(&m, c, nikgob_core::model::EventContext::Radio);
                    x[0] = 1.0;
                    let p_skip = weights.skip.p(&x).min(0.99);
                    let rested = model.tracks[c].last_start.map_or(1.0, |l| 1.0 - (-((m.now - l).max(0) as f64) / REST_HALF_MS as f64).exp2() as f32);
                    let joins = match (block, model.profile_at(c)) {
                        (Some(b), Some(p)) if b == p => match run {
                            r if r < BLOCK_LEN.0 => BLOCK,
                            r if r >= BLOCK_LEN.1 => -BLOCK,
                            _ => 0.0,
                        },
                        (Some(_), Some(_)) if run < BLOCK_LEN.0 => -BLOCK,
                        _ => 0.0,
                    };
                    let value = relevance + SKIP_VALUE * (1.0 - p_skip).ln() + RADIO_REST * rested + joins
                        - recs.taste.dislike_near_at(c)
                        + MODEL_JITTER * Rng::derive(jitter, c as u64).f32();
                    let reason = if parts[F_NEXT_ALBUM] > 0.0 && parts[F_NEXT_ALBUM] >= parts[F_COOC] {
                        Reason::NextOnAlbum
                    } else if parts[F_COOC].max(parts[F_PGRAPH]) > 0.5 && parts[F_COOC].max(parts[F_PGRAPH]) >= parts[F_SOUND] + 1.0 {
                        m.prev.map_or(Reason::SoundsLikeSeed, |p| Reason::FollowsOften(model.id(p)))
                    } else if parts[F_GRAPH] > 0.5 && parts[F_GRAPH] >= parts[F_SOUND] + 0.5 {
                        m.prev.or(seed.and_then(|t| model.position(t))).map_or(Reason::SoundsLikeSeed, |p| Reason::ListenedTogether(model.id(p)))
                    } else {
                        seed.map_or(Reason::SoundsLikeSeed, Reason::SimilarTo)
                    };
                    let slot = recs.slot(c, reason, value);
                    let free = (m.album_session && m.next_album == Some(c)) || (m.world_session.is_some() && slot.world == m.world_session);
                    let seen = (slot.artists.as_slice(), slot.album, slot.world);
                    let (violations, run) = if free {
                        (0, false)
                    } else {
                        let over = slot.world.is_some_and(|w| per_world.get(&w).is_some_and(|k| *k >= world_cap));
                        (rules.weighted(&seq, seen) + crate::pick::W_SHARE * usize::from(over), rules.world_run(&seq, seen))
                    };
                    let key = (violations, -value);
                    if best.as_ref().is_none_or(|(b, _, _)| key.0 < b.0 || (key.0 == b.0 && key.1 < b.1)) {
                        best = Some((key, slot, run));
                    }
                }
                best
            };
            let mut best = best_of(cands);
            // The best unheard one, unless it breaks more of the rules.
            let unheard = unheard.take();
            if !unheard.is_empty()
                && let Some(b) = best_of(unheard).filter(|b| best.as_ref().is_none_or(|a| b.0.0 <= a.0.0))
            {
                best = Some(b);
            }
            // Only one world left near the seed, and it ran twice: a track a
            // little farther, of a kind that goes with the seed, before a
            // world thrice running (the user's rule; the radius is ours).
            if best.as_ref().is_some_and(|b| b.2) {
                let wider = model.candidates(&m, &|i| {
                    ok(i) && free(i) && quiet(i) && !near(i) && anchor.goes(model, i) && (own(i) || next(i)) && (!sparse || in_scene(i))
                });
                if let Some(b) = best_of(wider).filter(|b| best.as_ref().is_some_and(|a| b.0.0 < a.0.0)) {
                    best = Some(b);
                }
            }
            let best = best.map(|(k, s, _)| (k, s));
            let Some((_, slot)) = best else { break };
            let Item::Track(id) = slot.item else { break };
            continuing &= m.next_album.is_some_and(|c| model.id(c) == id);
            fresh += usize::from(model.position(id).is_some_and(|i| !recs.heard(i)));

            take(taken, &slot);
            tail.push(Placed::of(&slot));
            if let Some(w) = slot.world {
                *per_world.entry(w).or_default() += 1;
            }
            play(&mut sim, &mut ts, id);
            out.push(slot);
        }
        (out, wide)
    }

    /// The profile and the mode a track's radio keeps to (profiles-v1 §4.3,
    /// §7): «Похоже на X» is pure in X's profile, autoplay in the profile
    /// its session is pure in (none in a mixed one); a background session
    /// keeps to its profile in the background, a focused one wanders more.
    fn plan(&self, model: &UserModel) -> Plan {
        let seed = match &self.seed {
            RadioSeed::Track(t) => model.position(*t),
            _ => None,
        };
        // A seed the catalog lacks: the profile most of its artists' tracks
        // are of.
        let own = seed.and_then(|i| model.profile_at(i)).or_else(|| {
            let f = self.seed_facts.as_ref()?;
            let mut count: HashMap<usize, usize> = HashMap::new();
            for a in &f.artists {
                for &i in model.of_artist(*a) {
                    if let Some(j) = model.profile_at(i as usize) {
                        *count.entry(j).or_default() += 1;
                    }
                }
            }
            count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(j, _)| j)
        });
        // The session's profile only if the seed is of it: a seed of
        // another kind (picked after a mix) is followed by its own.
        let fits = |j: usize| match (seed, own) {
            (Some(i), _) => model.membership_at(i).get(j).is_some_and(|m| *m >= MEMBER),
            (None, Some(o)) => o == j,
            (None, None) => true,
        };
        let live = model.live(&self.session);
        let mut profile = if self.follow {
            match live.mode {
                Some(Mode::Pure(j)) if fits(j) => Some(j),
                Some(Mode::Pure(_)) => own,
                Some(Mode::Mixed) => None,
                None => own,
            }
        } else {
            own
        };
        let background = live.p_bg >= 0.5;
        if background {
            profile = profile.or(live.profile);
        }
        let mut plan = Plan { profile, background, focus: !background && live.focus, ..Plan::default() };
        if background && let Some((j, p)) = profile.and_then(|j| Some((j, model.profiles.list.get(j)?))) {
            plan.wordless = p.stats.instrumental >= WORDLESS;
            plan.lane = Some(p.lane);
            let mut e: Vec<f32> = (0..model.len()).filter(|&i| model.profile_at(i) == Some(j)).filter_map(|i| model.energy_at(i)).collect();
            e.sort_by(f32::total_cmp);
            if e.len() >= BAND_FROM {
                let at = |q: f32| e[((e.len() - 1) as f32 * q) as usize];
                plan.band = Some((at(BG_BAND.0), at(BG_BAND.1)));
            }
        }
        plan
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
        let in_session: HashSet<TrackId> = self.session.tracks().map(|p| p.track).collect();
        let resting = |i: usize, id: TrackId| {
            if mix {
                recs.played_within(i, RECENT_MS)
            } else {
                recent.contains(&Item::Track(id)) || in_session.contains(&id) || recs.played_within(i, self.no_repeat_ms)
            }
        };
        let ok = |i: usize| {
            let id = recs.info(i).id;
            !recs.blocked(i)
                && !exclude.contains(&Item::Track(id))
                && !taken.contains(&Item::Track(id))
                && self.seed != RadioSeed::Track(id)
                && (self.not_world.is_none() || recs.info(i).world != self.not_world)
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
            // The artists other listeners hear with it (the collective
            // graph), their best known first.
            if let Some(m) = recs.model {
                for &(b, w) in m.graph.similar_artists(a).iter().take(CO_ARTISTS) {
                    let key = |i: usize| recs.taste.affinity_at(i) + m.graph.prior(i);
                    let mut theirs: Vec<usize> = recs.catalog.of_artist(b).iter().map(|&i| i as usize).filter(|&i| ok(i)).collect();
                    theirs.sort_by(|x, y| key(*y).total_cmp(&key(*x)).then(x.cmp(y)));
                    for i in theirs.into_iter().take(CO_TRACKS) {
                        add(&mut cands, i, Reason::CoListened(a), w * 0.7);
                    }
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
                    - recs.taste.dislike_near_at(i)
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
            let mut out = select_unheard(recs, own, want_own, self.unheard_share(), rules, taken);
            let left = n - out.len();
            out.extend(select_unheard(recs, rest, left, self.unheard_share(), rules, taken));
            return out;
        }
        sort_by_score(&mut slots);
        select_unheard(recs, slots, n, self.unheard_share(), rules, taken)
    }
}

/// The slot is new music or a track the user has not heard.
fn slot_unheard(recs: &Recs<'_>, s: &Slot) -> bool {
    match &s.item {
        Item::Track(id) => recs.catalog.position(*id).is_some_and(|i| !recs.heard(i)),
        Item::Stream(_) => true,
    }
}

/// The best `n` of `slots` (sorted) under the rules, at least `share` of
/// them unheard by the user while there are («Преимущественно новые
/// песни»).
fn select_unheard(recs: &Recs<'_>, slots: Vec<Slot>, n: usize, share: f32, rules: &Diversity, taken: &mut HashSet<Item>) -> Vec<Slot> {
    if share <= 0.0 {
        return select_diverse(slots, n, rules, taken);
    }
    let want = ((n as f32 * share).ceil() as usize).min(n);
    let fresh: Vec<Slot> = slots.iter().filter(|s| slot_unheard(recs, s)).cloned().collect();
    let mut out = select_more(fresh, want, n, rules, taken, &[]);
    let more = select_more(slots, n - out.len(), n, rules, taken, &out);
    out.extend(more);
    out
}

/// «Только новые»: the library's picks the user has heard (the last ones
/// first) make way for the new music of `reserve` that sounds like the
/// seed, while there is; `new` takes it.
fn make_way(recs: &Recs<'_>, lib: &mut Vec<Slot>, new: &mut Vec<Slot>, reserve: Vec<Slot>, rules: &Diversity, taken: &mut HashSet<Item>) {
    if reserve.is_empty() {
        return;
    }
    let heard: Vec<usize> = (0..lib.len()).filter(|&k| !slot_unheard(recs, &lib[k])).collect();
    if heard.is_empty() {
        return;
    }
    let more = select_more(reserve, heard.len(), new.len() + heard.len(), rules, taken, new);
    let gone: HashSet<usize> = heard.iter().rev().take(more.len()).copied().collect();
    let mut k = 0;
    lib.retain(|_| {
        k += 1;
        !gone.contains(&(k - 1))
    });
    new.extend(more);
}

/// A track is of a radio's profile with this membership in it
/// (profiles-v1 §7)…
const MEMBER: f32 = 0.35;
/// …and the radio widens to the neighbouring profile with fewer than this
/// many of its own left to play.
const UNHEARD_MIN: usize = 30;
/// A background profile this much without words: its radio takes only what
/// suits the background ([`crate::background`])…
const WORDLESS: f32 = 0.8;
/// …and only the middle of its energy (percentiles of the profile's own),
/// once it has this many tracks with a sound.
const BG_BAND: (f32, f32) = (0.15, 0.7);
const BAND_FROM: usize = 12;
/// A jump in sound costs this many times the model's own in the
/// background, and in a focused session.
const JUMP_BG: f32 = 2.0;
const JUMP_FOCUS: f32 = 0.5;
/// A focused session's radio: this many times the new music, a world a
/// fifth at most.
const FOCUS_NEW: f32 = 1.5;
const FOCUS_WORLD_SHARE: f32 = 0.2;
/// New music in the background: after this much of the session, one in
/// six at most…
const BG_NEW_AFTER_MS: i64 = 20 * 60_000;
const BG_NEW_SHARE: f32 = 1.0 / 6.0;
/// …and a background profile played through plays what rested this long.
const BG_REPEAT_MS: i64 = 36 * HOUR_MS;
/// A mixed session's radio keeps a profile for a block of three or four:
/// this much for the block going on before three, against it from four.
const BLOCK: f32 = 0.3;
const BLOCK_LEN: (usize, usize) = (3, 4);

/// What a track's radio keeps to ([`RadioState::plan`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Plan {
    /// The profile (none: a mixed session's radio takes all).
    profile: Option<usize>,
    background: bool,
    focus: bool,
    /// Background: the profile is mostly without words, and its kind.
    wordless: bool,
    lane: Option<LaneKey>,
    /// Background: the profile's band of energy.
    band: Option<(f32, f32)>,
}

impl Plan {
    /// New music sounding `v` (embedded) does not get in the way of a
    /// background radio (profiles-v1 §8): of its profile by its sound, in
    /// its band of energy.
    fn quiet_new(&self, model: &UserModel, v: Option<&Vector>) -> bool {
        let (Some(v), Some(j)) = (v, self.profile) else { return false };
        let kind = match self.lane {
            Some(LaneKey::Sung(l)) => SongType { voice: Voice::Sung, lang: Some(l), ..SongType::default() },
            Some(LaneKey::Instrumental) => SongType { voice: Voice::Instrumental, ..SongType::default() },
            _ => SongType::default(),
        };
        model.profiles.membership(v, &kind).get(j).is_some_and(|m| *m >= MEMBER)
            && self.band.is_none_or(|(lo, hi)| (lo..=hi).contains(&model.energy_of(v)))
    }
}

/// The profile nearest `j` of a kind of song the user goes on to from its
/// kind ([`UserModel::lanes_go`]).
fn neighbour(model: &UserModel, j: usize) -> Option<usize> {
    let lane = model.profiles.list.get(j)?.lane.song_lane();
    (0..model.profiles.len())
        .filter(|&o| o != j && model.lanes_go(lane, model.profiles.list[o].lane.song_lane()))
        .min_by(|a, b| model.profiles.distance(j, *a).total_cmp(&model.profiles.distance(j, *b)).then(a.cmp(b)))
}

/// A track the collective graph links to the seed this much goes in its
/// radio beyond the seed's sound radius.
const LINKED: f32 = 0.3;

/// What the collective graph links to the track `seed`: its neighbours,
/// and the tracks of the artists like its at [`crate::model`]'s weight of
/// an artist, by catalog position.
fn seed_links(model: &UserModel, seed: Option<TrackId>) -> HashMap<usize, f32> {
    let mut out: HashMap<usize, f32> = HashMap::new();
    let Some(s) = seed.and_then(|t| model.position(t)) else { return out };
    for &(c, w) in model.graph.near(s) {
        out.insert(c as usize, w);
    }
    if let Some(a) = model.main_artist_at(s) {
        for &(b, w) in model.graph.similar_artists(a) {
            for &i in model.of_artist(b) {
                let e = out.entry(i as usize).or_default();
                *e = e.max(0.5 * w);
            }
        }
    }
    out
}

/// The seed's scene: the tracks of its artists (1) and of the artists the
/// graph links to them (half the link), and the tracks `linked` to it.
fn scene_of(model: &UserModel, seed: Option<TrackId>, artists: &[ArtistId], linked: &HashMap<usize, f32>) -> HashMap<usize, f32> {
    let mut out: HashMap<usize, f32> = linked.iter().filter(|(_, w)| **w >= LINKED).map(|(i, w)| (*i, *w)).collect();
    for &a in artists {
        for &i in model.of_artist(a) {
            out.insert(i as usize, 1.0);
        }
        for &(b, w) in model.graph.similar_artists(a) {
            for &i in model.of_artist(b) {
                let e = out.entry(i as usize).or_default();
                *e = e.max(0.5 * w);
            }
        }
    }
    if let Some(p) = seed.and_then(|t| model.position(t)) {
        out.remove(&p);
    }
    out
}

/// The share of a model radio's anchor that is its seed: the session's
/// sound leans it, never leads it away.
const SEED_HOLD: f32 = 0.6;
/// A track of a radio is at most this many median distances from the seed…
const SEED_RADIUS: f32 = 1.0;
/// …or as far as the seed's nearest eighth of the library, if nearer…
const SEED_SHARE: usize = 8;
/// …but always as far as its nearest this many.
const SEED_AT_LEAST: usize = 30;

/// Where «Похоже на X» may go: near X in sound, songs of a kind the user
/// goes on to from X's ([`UserModel::lanes_go`]: no rap after a
/// soundtrack unless the user does so), a track without a vector only of
/// X's world or artist.

struct SeedAnchor {
    point: Option<Vector>,
    radius: f32,
    lane: Option<SongLane>,
    world: Option<u64>,
    artist: Option<ArtistId>,
}

impl SeedAnchor {
    fn of(model: &UserModel, seed: Option<TrackId>, facts: Option<&SeedFacts>) -> Self {
        let pos = seed.and_then(|t| model.position(t));
        // A seed the catalog lacks: its own sound if analysed, else its
        // artists' middle; their commonest kind of song, world and artist.
        let theirs: Vec<usize> = match (pos, facts) {
            (None, Some(f)) => f.artists.iter().flat_map(|a| model.of_artist(*a).iter().map(|&i| i as usize)).collect(),
            _ => Vec::new(),
        };
        let commonest = |v: Vec<u64>| {
            let mut count: HashMap<u64, usize> = HashMap::new();
            for x in v {
                *count.entry(x).or_default() += 1;
            }
            count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(x, _)| x)
        };
        let middle = || -> Option<Vector> {
            let vs: Vec<&Vector> = theirs.iter().filter_map(|&i| model.embedded_at(i)).collect();
            (!vs.is_empty()).then(|| std::array::from_fn(|k| vs.iter().map(|v| v[k]).sum::<f32>() / vs.len() as f32))
        };
        let their_lane = || {
            let lanes = [SongLane::Instrumental, SongLane::Russian, SongLane::Japanese, SongLane::Western];
            let v: Vec<u64> =
                theirs.iter().filter_map(|&i| lanes.iter().position(|l| *l == model.song_type_at(i).lane()).map(|k| k as u64)).collect();
            commonest(v).map(|k| lanes[k as usize]).or_else(|| facts.and_then(|f| f.lane))
        };
        let point = pos
            .and_then(|p| model.embedded_at(p).copied())
            .or_else(|| facts.and_then(|f| f.vector).map(|v| model.embed(&v)))
            .or_else(|| if pos.is_none() { middle() } else { None });
        let radius = point.map_or(f32::INFINITY, |sv| {
            let mut d: Vec<f32> = (0..model.len()).filter_map(|i| model.embedded_at(i)).map(|v| euclid(v, &sv)).collect();
            d.sort_by(f32::total_cmp);
            let last = d.len().saturating_sub(1);
            let share = d.get((d.len() / SEED_SHARE).min(last)).copied().unwrap_or(f32::INFINITY);
            let floor = d.get(SEED_AT_LEAST.min(last)).copied().unwrap_or(f32::INFINITY);
            share.min(SEED_RADIUS * model.scale()).max(floor)
        });
        let (lane, world, artist) = match pos {
            Some(p) => (Some(model.song_type_at(p).lane()), model.world_at(p), model.main_artist_at(p)),
            None => (
                their_lane(),
                commonest(theirs.iter().filter_map(|&i| model.world_at(i)).collect()),
                facts.and_then(|f| f.artists.iter().copied().find(|a| !model.of_artist(*a).is_empty())),
            ),
        };
        Self { point, radius, lane, world, artist }
    }

    /// Track `i` is of a kind of song that goes with the seed's.
    fn goes(&self, model: &UserModel, i: usize) -> bool {
        self.lane.is_none_or(|a| model.lanes_go(a, model.song_type_at(i).lane()))
    }

    /// Track `i` may follow.
    fn holds(&self, model: &UserModel, i: usize) -> bool {
        if !self.goes(model, i) {
            return false;
        }
        match (&self.point, model.embedded_at(i)) {
            (Some(p), Some(v)) => euclid(p, v) <= self.radius,
            (Some(_), None) => {
                (self.world.is_some() && model.world_at(i) == self.world) || (self.artist.is_some() && model.main_artist_at(i) == self.artist)
            }
            (None, _) => true,
        }
    }

    /// The anchor of the moment, held to the seed.
    fn lean(&self, session: Option<Vector>) -> Option<Vector> {
        match (self.point, session) {
            (Some(p), Some(s)) => Some(std::array::from_fn(|k| SEED_HOLD * p[k] + (1.0 - SEED_HOLD) * s[k])),
            (Some(p), None) => Some(p),
            (None, s) => s,
        }
    }
}


/// The tracks of a world's core that «После <мир>» starts from: liked or

/// with this much affinity (a couple of completes).
const CORE_AFFINITY: f32 = 1.5;

impl Recs<'_> {
    /// «После <мир>»: `n` songs from other worlds that sound like the core
    /// of `world` (its liked tracks and the ones played to the end more
    /// than once, else any it has affinity for, else all of it), at most
    /// `world_share` of one world, in the smooth order. Empty without
    /// analysis of the world.
    pub fn after_world(&self, world: u64, n: usize, world_share: f32, params: &RadioParams) -> Vec<TrackId> {
        let of_world: Vec<usize> = (0..self.catalog.len()).filter(|&i| self.info(i).world == Some(world)).collect();
        let core = |keep: &dyn Fn(usize) -> bool| -> Vec<Vector> {
            of_world.iter().copied().filter(|&i| keep(i)).filter_map(|i| self.embedded(self.info(i).id)).collect()
        };
        let mut vs = core(&|i| self.taste.liked_at(i) || self.taste.affinity_at(i) >= CORE_AFFINITY);
        if vs.is_empty() {
            vs = core(&|i| self.taste.affinity_at(i) > 0.0);
        }
        if vs.is_empty() {
            vs = core(&|_| true);
        }
        if vs.is_empty() {
            return Vec::new();
        }
        let mut centroid = vec![0.0f32; DIM];
        for v in &vs {
            for (c, x) in centroid.iter_mut().zip(v) {
                *c += x / vs.len() as f32;
            }
        }
        let seed = RadioSeed::Style { index: 0, centroid, artists: Vec::new() };
        let mut radio = RadioState::new(seed).without_world(world);
        // Twice as many as kept, so the world cap has room to choose.
        let picks = radio.extend(self, 2 * n, &RadioParams { new_share: 0.0, ..*params });
        let len = picks.len().max(1) as f32;
        let cands: Vec<(TrackId, f32)> = picks
            .iter()
            .enumerate()
            .filter_map(|(k, p)| match p.item {
                Item::Track(id) => Some((id, 1.0 - k as f32 / len)),
                Item::Stream(_) => None,
            })
            .collect();
        self.mix_of(&cands, n, world_share)
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
pub(crate) mod tests {
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

    /// The kinds of songs of [`listener`]: first id, count, x of the sound.
    const RAP: (u128, u128, f32) = (0, 120, 20.0);
    const OST: (u128, u128, f32) = (1_000, 60, 0.0);
    const JPOP: (u128, u128, f32) = (2_000, 60, 10.0);
    const ROCK: (u128, u128, f32) = (3_000, 60, -10.0);

    fn kind_of(id: u128) -> u128 {
        [RAP, OST, JPOP, ROCK].into_iter().find(|k| (k.0..k.0 + k.1).contains(&id)).expect("a kind").0
    }

    type Listener = (Catalog, VectorIndex<TrackId>, TasteInput, crate::songtype::SongTypes);

    /// A synthetic listener: a big rap region played most (twenty artists),
    /// a small region of soundtracks (three worlds, one composer each)
    /// played a little, and for an omnivore Japanese pop and rock too, in
    /// sessions of one kind each (the user's own choices).
    pub(crate) fn listener(omnivore: bool) -> Listener {
        use crate::songtype::{Lang, SongType, SongTypes, Voice};
        let mut kinds = vec![(RAP, Some(Lang::Russian), 40usize), (OST, None, 8)];
        if omnivore {
            kinds.push((JPOP, Some(Lang::Japanese), 10));
            kinds.push((ROCK, Some(Lang::English), 10));
        }
        let mut rng = Rng::new(17);
        let mut tracks = Vec::new();
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut types = SongTypes::default();
        for ((first, n, x), lang, _) in &kinds {
            for k in 0..*n {
                let id = first + k;
                let world = k / 20;
                let artist = if *first == OST.0 { (id / 20) as u32 } else { (id / 6) as u32 };
                tracks.push(info(id, &[artist], Some(id / 10)).with_world(&format!("w{first}.{world}")).with_duration(Some(200_000)));
                // Each world a little apart inside its kind.
                idx.insert(tid(id), point(x + world as f32 * 1.2 + rng.f32() * 0.6, rng.f32() * 0.6));
                let ty = match lang {
                    Some(l) => SongType { voice: Voice::Sung, lang: Some(*l), ..SongType::default() },
                    None => SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() },
                };
                types.by_track.insert(tid(id), ty);
            }
        }
        let mut events = Vec::new();
        let mut ts = crate::model::tests::T0 - 20 * crate::taste::DAY_MS;
        for ((first, n, _), _, sessions) in &kinds {
            for _ in 0..*sessions {
                for _ in 0..6 {
                    let id = tid(first + rng.below(*n as usize) as u128);
                    events.push(Play { context: nikgob_core::model::EventContext::Library, ..Play::new(id, ts, EventType::Start, Some(0)) });
                    ts += 200_000;
                    events.push(Play::new(id, ts, EventType::Complete, Some(200_000)));
                }
                ts += 4 * crate::taste::HOUR_MS;
            }
        }
        events.sort_by_key(|p| p.ts);
        (Catalog::new(tracks), idx, TasteInput { events, ..TasteInput::default() }, types)
    }

    /// Plays a model radio of `seed` for `batches` of ten, every pick heard
    /// out (the first `skip` of the first batch skipped early): the picks.
    fn play_model_radio(l: &Listener, seed: u128, batches: usize, skip: usize) -> Vec<u128> {
        play_model_radio_with(l, seed, batches, skip, |_| {})
    }

    /// [`play_model_radio`] with the radio's knobs set by `tune`.
    fn play_model_radio_with(l: &Listener, seed: u128, batches: usize, skip: usize, tune: fn(&mut RadioState)) -> Vec<u128> {
        use crate::model::{ModelInput, UserModel};
        let (cat, idx, input, types) = l;
        let now = crate::model::tests::T0;
        let model = UserModel::build(cat, input, ModelInput { index: idx, types: Some(types), weights: None, graph: None }, now);
        let recs = Recs::new(cat, &model.taste, idx).with_model(&model);
        let mut session = Session::default();
        let mut ts = now;
        session.start(tid(seed), ts, nikgob_core::model::EventContext::Radio, None);
        session.end(tid(seed), Ended::Complete);
        let mut radio = RadioState::new(RadioSeed::Track(tid(seed)));
        tune(&mut radio);
        let mut out = Vec::new();
        let mut skipped = 0;
        for b in 0..batches {
            radio.attune(session.clone(), None);
            for p in radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 5 }) {
                let Item::Track(t) = p.item else { continue };
                ts += 200_000;
                session.start(t, ts, nikgob_core::model::EventContext::Radio, None);
                let early = b == 0 && skipped < skip;
                session.end(t, if early { Ended::Early } else { Ended::Complete });
                radio.feedback(p.item.clone(), if early { Outcome::Skip { position_ms: 5_000 } } else { Outcome::Complete });
                skipped += usize::from(early);
                out.push(t.0.as_u128());
                if early && skipped == skip {
                    break;
                }
            }
        }
        out
    }

    /// «Похоже на» a soundtrack, for a listener of mostly rap and for an
    /// omnivore: thirty songs, all soundtracks near the seed, never three of
    /// one world running, never a rap song however much rap is played.
    #[test]
    fn a_soundtrack_radio_stays_a_soundtrack() {
        for omnivore in [false, true] {
            let l = listener(omnivore);
            let picks = play_model_radio(&l, OST.0 + 3, 3, 0);
            assert!(picks.len() >= 25, "{picks:?}");
            assert!(picks.iter().all(|t| kind_of(*t) == OST.0), "omnivore {omnivore}: {picks:?}");
            let worlds: Vec<u128> = picks.iter().map(|t| (t - OST.0) / 20).collect();
            for w in worlds.windows(3) {
                assert!(!(w[0] == w[1] && w[1] == w[2]), "a world thrice running: {worlds:?}");
            }
        }
    }

    /// Two early skips in a soundtrack radio turn it to the soundtracks of
    /// another world, not to the rap played most.
    #[test]
    fn skips_turn_a_radio_to_a_neighbouring_sound() {
        let l = listener(true);
        let picks = play_model_radio(&l, OST.0 + 3, 2, 2);
        let skipped_world = (picks[0] - OST.0) / 20;
        let after = &picks[2..];
        assert!(!after.is_empty());
        assert!(after.iter().all(|t| kind_of(*t) == OST.0), "{picks:?}");
        let other = after.iter().filter(|t| (**t - OST.0) / 20 != skipped_world).count();
        assert!(other * 2 >= after.len(), "mostly another world: {picks:?}");
    }

    /// Rap and rock of one language that sound apart (two profiles), and
    /// soundtracks without words, each played in sessions of its own: the
    /// first id, how many, x of the sound.
    const RU_RAP: (u128, u128, f32) = (0, 60, 20.0);
    const RU_ROCK: (u128, u128, f32) = (1_000, 120, -10.0);
    const QUIET: (u128, u128, f32) = (2_000, 60, 45.0);

    fn of_kind(id: u128) -> u128 {
        [RU_RAP, RU_ROCK, QUIET].into_iter().find(|k| (k.0..k.0 + k.1).contains(&id)).expect("a kind").0
    }

    fn one_language_two_sounds() -> Listener {
        use crate::songtype::{Lang, SongType, SongTypes, Voice};
        let kinds = [(RU_RAP, true, 15usize), (RU_ROCK, true, 20), (QUIET, false, 10)];
        let mut rng = Rng::new(23);
        let mut tracks = Vec::new();
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut types = SongTypes::default();
        for ((first, n, x), sung, _) in kinds {
            for k in 0..n {
                let id = first + k;
                // An artist (and their world) every six tracks.
                let artist = (id / 6) as u32;
                tracks.push(info(id, &[artist], Some(id / 6)).with_world(&format!("w{artist}")).with_duration(Some(200_000)));
                // Apart on the first feature only (each is normalised).
                idx.insert(tid(id), point(x + rng.f32() * 2.0, 0.0));
                let ty = if sung {
                    SongType { voice: Voice::Sung, lang: Some(Lang::Russian), ..SongType::default() }
                } else {
                    SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() }
                };
                types.by_track.insert(tid(id), ty);
            }
        }
        let mut events = Vec::new();
        let mut ts = crate::model::tests::T0 - 20 * crate::taste::DAY_MS;
        for ((first, n, _), _, sessions) in kinds {
            for _ in 0..sessions {
                for _ in 0..6 {
                    let id = tid(first + rng.below(n as usize) as u128);
                    events.push(Play { context: nikgob_core::model::EventContext::Library, ..Play::new(id, ts, EventType::Start, Some(0)) });
                    ts += 200_000;
                    events.push(Play::new(id, ts, EventType::Complete, Some(200_000)));
                }
                ts += 4 * crate::taste::HOUR_MS;
            }
        }
        events.sort_by_key(|p| p.ts);
        (Catalog::new(tracks), idx, TasteInput { events, ..TasteInput::default() }, types)
    }

    /// The fixture makes the profiles it is about: rap and rock of one
    /// language apart, soundtracks a third.
    fn profiles_of(l: &Listener) -> Vec<u128> {
        use crate::model::{ModelInput, UserModel};
        let (cat, idx, input, types) = l;
        let model = UserModel::build(cat, input, ModelInput { index: idx, types: Some(types), weights: None, graph: None }, crate::model::tests::T0);
        let mut kinds: Vec<u128> = (0..model.profiles.len())
            .map(|j| {
                let i = (0..model.len()).find(|&i| model.profile_at(i) == Some(j)).expect("a member");
                of_kind(model.id(i).0.as_u128())
            })
            .collect();
        kinds.sort_unstable();
        kinds
    }

    /// «Похоже на» a rap song stays rap while the rap profile has songs to
    /// play, then goes on with the rock of the same language (the
    /// neighbouring profile), never with the soundtracks it never follows.
    #[test]
    fn stays_in_profile_until_exhausted() {
        let l = one_language_two_sounds();
        assert_eq!(profiles_of(&l), [RU_RAP.0, RU_ROCK.0, QUIET.0]);
        let picks = play_model_radio(&l, RU_RAP.0 + 3, 9, 0);
        assert!(picks.len() >= 80, "it keeps playing: {}", picks.len());
        let kinds: Vec<u128> = picks.iter().map(|t| of_kind(*t)).collect();
        let first_other = kinds.iter().position(|k| *k != RU_RAP.0).expect("it goes on past the rap");
        assert!(first_other >= RU_RAP.1 as usize - 10, "rap first while it lasts: {first_other} {kinds:?}");
        let rap = kinds.iter().filter(|k| **k == RU_RAP.0).count();
        assert!(rap >= RU_RAP.1 as usize - 5, "the rap played through: {rap}");
        assert!(kinds[first_other..].iter().all(|k| *k == RU_ROCK.0), "then the rock: {kinds:?}");
        let worlds: Vec<u128> = picks.iter().map(|t| t / 6).collect();
        for w in worlds.windows(3) {
            assert!(!(w[0] == w[1] && w[1] == w[2]), "a world thrice running: {worlds:?}");
        }
    }

    /// Two early skips running take a rap radio out of the rap to the rock
    /// of its language; without them it stays.
    #[test]
    fn widens_after_two_skips() {
        let l = one_language_two_sounds();
        let calm = play_model_radio(&l, RU_RAP.0 + 3, 2, 0);
        assert!(calm.iter().all(|t| of_kind(*t) == RU_RAP.0), "{calm:?}");
        let picks = play_model_radio(&l, RU_RAP.0 + 3, 2, 2);
        let after = &picks[2..];
        assert!(after.len() >= 8, "{picks:?}");
        let rock = after.iter().filter(|t| of_kind(**t) == RU_ROCK.0).count();
        assert!(rock * 2 >= after.len(), "mostly the neighbouring profile: {picks:?}");
        assert!(after.iter().all(|t| of_kind(*t) != QUIET.0), "{picks:?}");
    }

    /// A background session's radio of soundtracks without words keeps to
    /// what suits the background and to the middle of the profile's energy.
    #[test]
    fn a_background_radio_stays_quiet() {
        use crate::model::{ModelInput, UserModel};
        let (cat, idx, input, types) = one_language_two_sounds();
        let now = crate::model::tests::T0;
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, now);
        // Half of the soundtracks suit the background.
        let suits: Vec<bool> = cat.tracks().iter().map(|t| of_kind(t.id.0.as_u128()) == QUIET.0 && t.id.0.as_u128() % 2 == 0).collect();
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model).with_background(&suits);
        let mut radio = RadioState::new(RadioSeed::Track(tid(QUIET.0 + 2)));
        let plain = radio.plan(&model);
        assert!(!plain.background);
        let picks = radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 4 });
        assert!(picks.iter().any(|p| matches!(p.item, Item::Track(t) if t.0.as_u128() % 2 == 1)), "not background: anything of the profile");
        let mut bg = plain;
        bg.background = true;
        bg.wordless = true;
        let mut taken = HashSet::new();
        let (picks, wide) = radio.by_model(&recs, &model, &bg, 10, &HashSet::new(), &Diversity::default(), &mut Rng::new(1), &mut taken, None);
        assert!(!wide, "a background radio never widens");
        assert_eq!(picks.len(), 10);
        for p in &picks {
            let Item::Track(t) = p.item else { panic!() };
            assert!(suits[cat.position(t).unwrap()], "{t:?} does not suit the background");
        }
    }

    /// «С новым» with candidates enough: a fifth of a model radio is new,
    /// never more than five of the library running, for a one-kind
    /// listener and an omnivore alike.
    #[test]
    fn a_radio_with_new_brings_its_share() {
        use crate::model::{ModelInput, UserModel};
        for omnivore in [false, true] {
            let (cat, idx, input, types) = listener(omnivore);
            let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, crate::model::tests::T0);
            let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
            let mut radio = RadioState::new(RadioSeed::Track(tid(OST.0 + 3)));
            radio.add_new((0..60).map(|k| NewTrack {
                id: RemoteId::new(Service::Youtube, format!("n{k}")),
                artist: format!("new artist {}", k % 20),
                album: None,
                vector: None,
            }));
            let mut picks = Vec::new();
            for _ in 0..4 {
                picks.extend(radio.extend(&recs, 10, &RadioParams { new_share: 0.2, seed: 2 }));
            }
            let new: Vec<bool> = picks.iter().map(|p| matches!(p.item, Item::Stream(_))).collect();
            let share = new.iter().filter(|n| **n).count() as f32 / new.len() as f32;
            assert!((0.15..=0.25).contains(&share), "omnivore {omnivore}: {share} {new:?}");
            let longest = new.split(|n| *n).map(<[bool]>::len).max().unwrap_or(0);
            assert!(longest <= 5, "{longest} of the library running: {new:?}");
            // New music that came late joins at once.
            let late = radio.take_new(&recs, 2, 10, 0);
            assert_eq!(late.len(), 2);
            assert!(late.iter().all(|p| !picks.contains(p)));
        }
    }

    /// A one-kind listener's radio of a rap song stays rap and spreads its
    /// artists.

    #[test]
    fn a_one_kind_radio_spreads_its_artists() {
        let l = listener(false);
        let picks = play_model_radio(&l, RAP.0 + 7, 3, 0);
        assert!(picks.len() >= 25);
        assert!(picks.iter().all(|t| kind_of(*t) == RAP.0), "{picks:?}");
        let artists: Vec<u128> = picks.iter().map(|t| t / 6).collect();
        for w in artists.windows(2) {
            assert_ne!(w[0], w[1], "an artist twice running: {artists:?}");
        }
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
        fresh.session = radio.session.clone();
        assert_eq!(radio, fresh);
    }

    /// A radio leaves out a favourite played an hour ago while «Не
    /// повторять сыгранное» (three hours) runs, brings it back under a
    /// shorter one, but never once it played in the radio's session; twice
    /// running, it plays first. A mix lets the hour-old one rest.
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
        assert!(!has(&radio.extend(&recs, 5, &params)), "played an hour ago: it rests three hours");

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.set_no_repeat_ms(HOUR_MS / 2);
        assert!(has(&radio.extend(&recs, 5, &params)), "played an hour ago: back after half an hour");

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.set_no_repeat_ms(HOUR_MS / 2);
        radio.feedback(Item::Track(fav), Outcome::Complete);
        assert!(!has(&radio.extend(&recs, 5, &params)), "one of the last five");
        for k in 20..25 {
            radio.feedback(Item::Track(tid(k)), Outcome::Complete);
        }
        assert!(!has(&radio.extend(&recs, 5, &params)), "played in the session: it rests even five others later");

        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(fav), Outcome::Complete);
        radio.feedback(Item::Track(fav), Outcome::Complete);
        assert_eq!(radio.extend(&recs, 5, &params)[0].item, Item::Track(fav), "twice running: once more, first");

        let mut mix = RadioState::new(RadioSeed::Daylist { part: crate::taste::DayPart::Evening, centre: [0.0; 3] });
        assert!(!has(&mix.extend(&recs, 5, &params)), "a mix lets it rest");
    }

    /// «После A»: the sound of A's liked core (region +x), none of A, and
    /// no other world over a quarter.
    #[test]
    fn after_a_world_comes_its_sound_from_other_worlds() {
        let (cat, idx) = two_regions();
        let worlds = ["a", "b", "c", "d"];
        let world = |i: u128| if i <= 30 { "a" } else if i <= 60 { worlds[1 + (i % 3) as usize] } else { "e" };
        let cat = Catalog::new(cat.tracks().iter().map(|t| {
            let i = t.id.0.as_u128();
            if i == 0 { t.clone() } else { t.clone().with_world(&format!("{}{}", world(i), if i > 60 { i % 8 } else { 0 })) }
        }));
        let likes: Vec<TrackId> = (1..=10).map(tid).collect();
        let taste = Taste::build(&cat, &TasteInput { likes, ..Default::default() }, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let a = crate::world_token("a0");
        let mix = recs.after_world(a, 20, 0.25, &RadioParams { new_share: 0.0, seed: 3 });
        assert_eq!(mix.len(), 20);
        let ids: Vec<u128> = mix.iter().map(|t| t.0.as_u128()).collect();
        assert!(ids.iter().all(|&i| i > 30 || i == 0), "none of A: {ids:?}");
        assert!(ids.iter().filter(|&&i| i <= 60).count() >= 12, "its sound first: {ids:?}");
        let mut per: HashMap<&str, usize> = HashMap::new();
        for &i in &ids {
            *per.entry(world(i)).or_default() += usize::from(i <= 60);
        }
        assert!(per.values().all(|&n| n <= 5), "a quarter at most: {per:?}");
    }

    /// A track's radio by the user model, on the listener of albums: the
    /// album being played goes on in its order first.
    #[test]
    fn a_model_radio_continues_the_album() {
        use crate::model::{ModelInput, UserModel};
        use nikgob_core::model::EventContext;
        let (cat, idx, history) = crate::model::tests::album_listener(15, 4);
        let now = crate::model::tests::T0 + crate::taste::DAY_MS;
        let model = UserModel::build(&cat, &history, ModelInput { index: &idx, types: None, weights: None, graph: None }, now);
        let taste = Taste::build(&cat, &history, now);
        let recs = Recs::new(&cat, &taste, &idx).with_model(&model);
        let mut session = crate::session::Session::default();
        session.start(tid(42), now - 400_000, EventContext::Album, None);
        session.end(tid(42), crate::session::Ended::Complete);
        session.start(tid(43), now - 200_000, EventContext::Album, None);
        let mut radio = RadioState::new(RadioSeed::Track(tid(43)));
        radio.attune(session, None);
        let picks = radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 1 });
        let ids: Vec<u128> = picks.iter().map(|p| match p.item { Item::Track(t) => t.0.as_u128(), Item::Stream(_) => 0 }).collect();
        assert_eq!(&ids[..3], &[44, 45, 46], "{ids:?}");
        assert_eq!(picks[0].reason, Reason::NextOnAlbum);
        assert_eq!(picks.len(), 10);
        let unique: HashSet<u128> = ids.iter().copied().collect();
        assert_eq!(unique.len(), 10);
    }

    /// Two early skips running in one sound: the model radio leaves it.
    #[test]
    fn two_early_skips_move_a_model_radio() {
        use crate::model::{ModelInput, UserModel};
        let (cat, idx) = two_regions();
        let input = TasteInput::default();
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, NOW);
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx).with_model(&model);
        let params = RadioParams { new_share: 0.0, seed: 3 };
        // A complete in A before the seed: the radio goes on in A…
        let mut liked = RadioState::new(RadioSeed::Track(tid(0)));
        liked.feedback(Item::Track(tid(9)), Outcome::Complete);
        let a = liked.extend(&recs, 10, &params).iter().filter(|p| in_a(p)).count();
        assert!(a >= 8, "a complete in A: {a}");
        // …unless two of A were skipped early since.
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.feedback(Item::Track(tid(9)), Outcome::Complete);
        radio.feedback(Item::Track(tid(5)), Outcome::Skip { position_ms: 8_000 });
        radio.feedback(Item::Track(tid(7)), Outcome::Skip { position_ms: 6_000 });
        let a = radio.extend(&recs, 10, &params).iter().filter(|p| in_a(p)).count();
        assert!(a <= 2, "two skips in A move away from A: {a}");
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

    /// The shape of a real autoplay that left its scene: a big cluster of
    /// Japanese songs played most (a calm mix of it was the session), and a
    /// Russian rap artist [`SEED_ARTIST`] of three tracks sounding like it,
    /// one more artist the graph links to it, and a little other Russian
    /// music far in sound. The user also goes on from Russian to Japanese.
    const JP: (u128, u128, f32) = (2_000, 150, 10.0);
    const SEED_ARTIST: u32 = 900;
    const LINKED_ARTIST: u32 = 901;
    const SEED_TRACKS: [u128; 3] = [100, 101, 102];
    const LINKED_TRACKS: [u128; 5] = [110, 111, 112, 113, 114];
    const OTHER_RU: (u128, u128) = (120, 10);

    fn russian(id: u128) -> bool {
        (100..130).contains(&id) || id == 103
    }

    fn sparse_seed() -> (Listener, crate::graph::GraphInput) {
        use crate::songtype::{Lang, SongType, SongTypes, Voice};
        let mut rng = Rng::new(31);
        let mut tracks = Vec::new();
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut types = SongTypes::default();
        let mut add = |id: u128, artist: u32, x: f32, lang: Lang, rng: &mut Rng| {
            tracks.push(info(id, &[artist], Some(id / 6)).with_world(&format!("w{artist}")).with_duration(Some(200_000)));
            idx.insert(tid(id), point(x + rng.f32() * 0.6, rng.f32() * 0.6));
            types.by_track.insert(tid(id), SongType { voice: Voice::Sung, lang: Some(lang), ..SongType::default() });
        };
        for k in 0..JP.1 {
            add(JP.0 + k, 2_000 + (k / 6) as u32, JP.2, Lang::Japanese, &mut rng);
        }
        for id in SEED_TRACKS {
            add(id, SEED_ARTIST, JP.2 + 0.3, Lang::Russian, &mut rng);
        }
        for id in LINKED_TRACKS {
            add(id, LINKED_ARTIST, -5.0, Lang::Russian, &mut rng);
        }
        for k in 0..OTHER_RU.1 {
            add(OTHER_RU.0 + k, 902 + (k / 5) as u32, -8.0, Lang::Russian, &mut rng);
        }
        let mut events = Vec::new();
        let mut ts = crate::model::tests::T0 - 20 * crate::taste::DAY_MS;
        let mut play = |id: u128, ts: &mut i64| {
            events.push(Play { context: nikgob_core::model::EventContext::Library, ..Play::new(tid(id), *ts, EventType::Start, Some(0)) });
            *ts += 200_000;
            events.push(Play::new(tid(id), *ts, EventType::Complete, Some(200_000)));
        };
        for _ in 0..30 {
            for _ in 0..6 {
                play(JP.0 + rng.below(JP.1 as usize) as u128, &mut ts);
            }
            ts += 4 * crate::taste::HOUR_MS;
        }
        // Russian, then on to Japanese: the lanes go for this user.
        for _ in 0..12 {
            for _ in 0..2 {
                play(OTHER_RU.0 + rng.below(OTHER_RU.1 as usize) as u128, &mut ts);
            }
            for _ in 0..3 {
                play(JP.0 + rng.below(JP.1 as usize) as u128, &mut ts);
            }
            ts += 4 * crate::taste::HOUR_MS;
        }
        events.sort_by_key(|p| p.ts);
        let graph = crate::graph::GraphInput {
            artists: vec![crate::graph::Edge {
                from: crate::test_util::aid(SEED_ARTIST),
                to: crate::test_util::aid(LINKED_ARTIST),
                rank: 0,
                of: 10,
                source: crate::graph::GraphSource::DeezerArtists,
            }],
            ..crate::graph::GraphInput::default()
        };
        ((Catalog::new(tracks), idx, TasteInput { events, ..TasteInput::default() }, types), graph)
    }

    /// Autoplay after a session of a calm Japanese mix and an album of the
    /// seed's artist: `facts` for a seed the catalog lacks. Thirty picks.
    fn sparse_autoplay(seed: u128, played: &[u128], facts: Option<SeedFacts>) -> Vec<u128> {
        sparse_autoplay_with(seed, played, facts, |_| {})
    }

    /// [`sparse_autoplay`] with the radio's knobs set by `tune`.
    fn sparse_autoplay_with(seed: u128, played: &[u128], facts: Option<SeedFacts>, tune: fn(&mut RadioState)) -> Vec<u128> {
        use crate::model::{ModelInput, UserModel};
        let ((cat, idx, input, types), graph) = sparse_seed();
        let now = crate::model::tests::T0;
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: Some(&graph) }, now);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let mut session = Session::default();
        let mut ts = now - 30 * 60_000;
        for k in 0..6 {
            session.start(tid(JP.0 + 7 * k), ts, nikgob_core::model::EventContext::HomeSection, None);
            ts += 200_000;
            session.end(tid(JP.0 + 7 * k), Ended::Complete);
        }
        for &t in played.iter().chain([&seed]) {
            session.start(tid(t), ts, nikgob_core::model::EventContext::Album, Some(nikgob_core::model::EventOrigin::Manual));
            ts += 120_000;
            session.end(tid(t), Ended::Complete);
        }
        let mut radio = RadioState::new(RadioSeed::Track(tid(seed))).continuing();
        if let Some(f) = facts {
            radio = radio.with_seed_facts(f);
        }
        radio.mark_served(played.iter().map(|t| Item::Track(tid(*t))));
        tune(&mut radio);
        let mut out = Vec::new();
        for _ in 0..3 {
            radio.attune(session.clone(), None);
            for p in radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 9 }) {
                let Item::Track(t) = p.item else { continue };
                ts += 200_000;
                session.start(t, ts, nikgob_core::model::EventContext::Radio, None);
                session.end(t, Ended::Complete);
                radio.feedback(p.item.clone(), Outcome::Complete);
                out.push(t.0.as_u128());
            }
        }
        out
    }

    /// The seed's artist has no tracks left and nothing of its scene sounds
    /// like it: autoplay goes on with the artist the graph links to it,
    /// then the rest of its language, not with the Japanese the session's
    /// mix was of; Japanese only once the Russian is played through.
    #[test]
    fn a_sparse_seed_keeps_to_its_scene() {
        let picks = sparse_autoplay(102, &[100, 101], None);
        let ru = LINKED_TRACKS.len() + OTHER_RU.1 as usize;
        assert!(picks.len() >= 25, "{picks:?}");
        assert!(picks[..ru].iter().all(|t| russian(*t)), "the Russian first: {picks:?}");
        assert!(LINKED_TRACKS.iter().all(|t| picks[..8].contains(t)), "the linked artist first: {picks:?}");
    }

    /// The same seed as a stream the catalog lacks (played from its album's
    /// page), known only by its artist and sound: its artist's own tracks
    /// and its scene come first.
    #[test]
    fn a_seed_the_catalog_lacks_keeps_to_its_artists_scene() {
        let facts = SeedFacts { artists: vec![crate::test_util::aid(SEED_ARTIST)], vector: Some(point(JP.2 + 0.3, 0.3)), lane: None };
        let picks = sparse_autoplay(103, &[], Some(facts));
        assert!(picks.len() >= 25, "{picks:?}");
        let ru = SEED_TRACKS.len() + LINKED_TRACKS.len() + OTHER_RU.1 as usize;
        assert!(picks[..ru].iter().all(|t| russian(*t)), "the Russian first: {picks:?}");
        assert!(SEED_TRACKS.iter().all(|t| picks[..8].contains(t)), "its artist first: {picks:?}");
    }

    /// What the session played (and what played within «Не повторять
    /// сыгранное») is not picked again while anything else is left; with
    /// nothing else left it comes back.
    #[test]
    fn autoplay_leaves_out_what_played_lately() {
        use crate::model::{ModelInput, UserModel};
        let ((cat, idx, mut input, types), graph) = sparse_seed();
        let now = crate::model::tests::T0;
        // A Japanese session of twenty-four, the seed last: the history
        // the model is built from has it, the session keeps its last twenty.
        let played: Vec<u128> = (0..24).map(|k| JP.0 + 3 * k).collect();
        let mut ts = now - 90 * 60_000;
        for &t in &played {
            input.events.push(Play::new(tid(t), ts, EventType::Start, Some(0)));
            input.events.push(Play::new(tid(t), ts + 200_000, EventType::Complete, Some(200_000)));
            ts += 200_000;
        }
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: Some(&graph) }, now);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let mut session = Session::default();
        let mut ts = now - 90 * 60_000;
        for &t in &played {
            session.start(tid(t), ts, nikgob_core::model::EventContext::Library, Some(nikgob_core::model::EventOrigin::Manual));
            ts += 200_000;
            session.end(tid(t), Ended::Complete);
        }
        let seed = *played.last().unwrap();
        let mut radio = RadioState::new(RadioSeed::Track(tid(seed))).continuing();
        let mut picks = Vec::new();
        for _ in 0..5 {
            radio.attune(session.clone(), None);
            for p in radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 4 }) {
                let Item::Track(t) = p.item else { continue };
                ts += 200_000;
                session.start(t, ts, nikgob_core::model::EventContext::Radio, None);
                session.end(t, Ended::Complete);
                radio.feedback(p.item.clone(), Outcome::Complete);
                picks.push(t.0.as_u128());
            }
        }
        assert!(picks.len() >= 45, "{picks:?}");
        assert!(picks.iter().all(|t| !played.contains(t)), "nothing of the session again: {picks:?}");

        // A library of four, all played in the session: they come back.
        let small = Catalog::new((0..4u128).map(|i| info(i, &[i as u32], None)));
        let taste = Taste::build(&small, &TasteInput::default(), NOW);
        let empty = VectorIndex::new();
        let recs = Recs::new(&small, &taste, &empty);
        let mut session = Session::default();
        for i in 0..4u128 {
            session.start(tid(i), NOW - 60_000 * (5 - i as i64), nikgob_core::model::EventContext::Library, None);
            session.end(tid(i), Ended::Complete);
        }
        let mut radio = RadioState::new(RadioSeed::Track(tid(3)));
        radio.attune(session, None);
        assert!(!radio.extend(&recs, 3, &RadioParams { new_share: 0.0, seed: 1 }).is_empty(), "a small library keeps playing");
    }

    fn stream(tag: &str, k: usize, vector: Option<Vector>) -> NewTrack {
        NewTrack {
            id: RemoteId::new(Service::Youtube, format!("{tag}{k}")),
            artist: format!("{tag} artist {k}"),
            album: None,
            vector,
        }
    }

    /// New music of `picks` whose id starts with `tag`.
    fn streams_of(picks: &[Pick], tag: &str) -> usize {
        picks.iter().filter(|p| matches!(&p.item, Item::Stream(r) if r.id.starts_with(tag))).count()
    }

    /// «Подмешивать случайное»: «Никогда» brings no new music that is not
    /// known to sound like the seed, neither in a batch nor joining later,
    /// while what sounds like it still comes; «Редко» one in ten; no limit,
    /// the share of new music as before.
    #[test]
    fn explore_limits_the_blind_new_music() {
        let (cat, idx) = two_regions();
        let taste = Taste::build(&cat, &TasteInput::default(), NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let params = RadioParams { new_share: 0.2, seed: 9 };
        let mut never = RadioState::new(RadioSeed::Track(tid(0)));
        never.set_explore(Some(0.0));
        never.add_new((0..30).map(|k| stream("b", k, None)));
        for _ in 0..4 {
            let picks = never.extend(&recs, 10, &params);
            assert_eq!(picks.len(), 10, "the library fills in");
            assert_eq!(streams_of(&picks, "b"), 0, "{picks:?}");
        }
        assert!(never.take_new(&recs, 2, 10, 0).is_empty(), "nor later");
        never.add_new((0..30).map(|k| stream("n", k, Some(point(0.0, 0.0)))));
        let picks = never.extend(&recs, 10, &params);
        assert_eq!(streams_of(&picks, "n"), 2, "what sounds like the seed comes: {picks:?}");
        assert_eq!(streams_of(&picks, "b"), 0);

        let mut rare = RadioState::new(RadioSeed::Track(tid(0)));
        rare.set_explore(Some(0.1));
        rare.add_new((0..30).map(|k| stream("b", k, None)));
        let mut plain = RadioState::new(RadioSeed::Track(tid(0)));
        plain.add_new((0..30).map(|k| stream("b", k, None)));
        for _ in 0..4 {
            assert_eq!(streams_of(&rare.extend(&recs, 10, &params), "b"), 1);
            assert_eq!(streams_of(&plain.extend(&recs, 10, &params), "b"), 2);
        }
        // Joining later, the new music queued counts as blind.
        assert!(rare.take_new(&recs, 2, 10, 1).is_empty());
        assert_eq!(rare.take_new(&recs, 2, 10, 0).len(), 1);
        // The knob survives a restart.
        let json = serde_json::to_string(&rare).unwrap();
        let back: RadioState = serde_json::from_str(&json).unwrap();
        assert_eq!((back.explore, back.unheard), (Some(0.1), 0.0));
    }

    /// The same in a radio by the user model: «Никогда» no blind new music
    /// at all, «Редко» one in ten at most, what sounds like the seed comes.
    #[test]
    fn explore_limits_a_model_radio_too() {
        use crate::model::{ModelInput, UserModel};
        let (cat, idx, input, types) = listener(true);
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, crate::model::tests::T0);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let seed = OST.0 + 3;
        let params = RadioParams { new_share: 0.2, seed: 2 };
        for (explore, most) in [(Some(0.0), 0), (Some(0.1), 1), (None, 2)] {
            let mut radio = RadioState::new(RadioSeed::Track(tid(seed)));
            radio.set_explore(explore);
            radio.add_new((0..40).map(|k| stream("b", k, None)));
            let mut total = 0;
            for _ in 0..4 {
                let picks = radio.extend(&recs, 10, &params);
                assert_eq!(picks.len(), 10);
                let blind = streams_of(&picks, "b");
                assert!(blind <= most, "{explore:?}: {blind} blind");
                total += blind;
            }
            assert_eq!(total == 0, most == 0, "{explore:?}: {total} blind");
        }
        let mut radio = RadioState::new(RadioSeed::Track(tid(seed)));
        radio.set_explore(Some(0.0));
        radio.add_new((0..40).map(|k| stream("b", k, None)));
        radio.add_new((0..40).map(|k| stream("n", k, idx.get(tid(seed)).copied())));
        let picks: Vec<Pick> = (0..4).flat_map(|_| radio.extend(&recs, 10, &params)).collect();
        assert!(streams_of(&picks, "n") > 0, "what sounds like the seed comes");
        assert_eq!(streams_of(&picks, "b"), 0);
    }

    /// «Только новые»: a radio takes only what the user has not heard (a
    /// dislike still kept out) while there is, then the usual picks, never
    /// stopping; with new music that sounds like the seed, that stands in
    /// for what was heard; with only what has a file, the usual picks.
    #[test]
    fn only_new_takes_the_unheard_then_falls_back() {
        let (cat, idx) = two_regions();
        let mut input = TasteInput { disliked_tracks: vec![tid(101)], ..TasteInput::default() };
        for i in 1..=100u128 {
            let ts = NOW - 10 * crate::taste::DAY_MS + i as i64 * 1_000_000;
            input.events.push(Play::new(tid(i), ts, EventType::Start, Some(0)));
            input.events.push(Play::new(tid(i), ts + 200_000, EventType::Complete, Some(200_000)));
        }
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let unheard = |p: &Pick| match &p.item {
            Item::Track(t) => t.0.as_u128() > 100,
            Item::Stream(_) => true,
        };
        let count = |picks: &[Pick]| picks.iter().filter(|p| unheard(p)).count();
        let disliked = |picks: &[Pick]| picks.iter().any(|p| p.item == Item::Track(tid(101)));

        // Nineteen unheard (one disliked), then the heard ones.
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.set_unheard(1.0);
        let params = RadioParams { new_share: 0.0, seed: 3 };
        let batches: Vec<Vec<Pick>> = (0..3).map(|_| radio.extend(&recs, 10, &params)).collect();
        assert!(batches.iter().all(|b| b.len() == 10), "it goes on: {batches:?}");
        assert_eq!(batches.iter().map(|b| count(b)).collect::<Vec<_>>(), vec![10, 9, 0], "{batches:?}");
        assert!(!batches.iter().any(|b| disliked(b)));

        // New music that sounds like the seed stands in for what was heard;
        // none of the blind.
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.set_unheard(1.0);
        radio.set_explore(Some(0.0));
        radio.add_new((0..30).map(|k| stream("n", k, Some(point(0.0, 0.0)))));
        radio.add_new((0..30).map(|k| stream("b", k, None)));
        let params = RadioParams { new_share: 0.2, seed: 3 };
        let picks: Vec<Pick> = (0..4).flat_map(|_| radio.extend(&recs, 10, &params)).collect();
        assert_eq!(picks.len(), 40);
        assert_eq!(count(&picks), 40, "{picks:?}");
        assert_eq!(streams_of(&picks, "b"), 0);
        assert!(!disliked(&picks));

        // Only what has a file, and the unheard have none: no new music,
        // the usual picks.
        let local = Catalog::new(cat.tracks().iter().cloned().map(|mut t| {
            t.local = t.id.0.as_u128() <= 100;
            t
        }));
        let taste = Taste::build(&local, &input, NOW);
        let recs = Recs::new(&local, &taste, &idx).local_only(true);
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.set_unheard(1.0);
        radio.add_new((0..30).map(|k| stream("n", k, Some(point(0.0, 0.0)))));
        for _ in 0..3 {
            let picks = radio.extend(&recs, 10, &params);
            assert_eq!(picks.len(), 10, "it goes on");
            assert!(picks.iter().all(|p| !unheard(p)), "{picks:?}");
        }
    }

    /// «Больше новых»: at least half of the library's picks unheard while
    /// there are.
    #[test]
    fn more_new_takes_half_unheard() {
        let (cat, idx) = two_regions();
        let mut input = TasteInput::default();
        for i in 1..=100u128 {
            let ts = NOW - 10 * crate::taste::DAY_MS + i as i64 * 1_000_000;
            input.events.push(Play::new(tid(i), ts, EventType::Start, Some(0)));
            input.events.push(Play::new(tid(i), ts + 200_000, EventType::Complete, Some(200_000)));
        }
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let mut radio = RadioState::new(RadioSeed::Track(tid(0)));
        radio.set_unheard(0.5);
        let picks = radio.extend(&recs, 10, &RadioParams { new_share: 0.0, seed: 3 });
        assert_eq!(picks.len(), 10);
        let fresh = picks.iter().filter(|p| matches!(&p.item, Item::Track(t) if t.0.as_u128() > 100)).count();
        assert!(fresh >= 5, "{fresh} unheard: {picks:?}");
    }

    /// The knobs keep a model radio's rules: with «Только новые» and
    /// «Никогда» a soundtrack's radio still brings only soundtracks, never a
    /// world thrice running, and no fewer unheard than usual.
    #[test]
    fn the_knobs_keep_a_model_radios_rules() {
        use crate::model::{ModelInput, UserModel};
        for omnivore in [false, true] {
            let l = listener(omnivore);
            let (cat, idx, input, types) = &l;
            let model = UserModel::build(cat, input, ModelInput { index: idx, types: Some(types), weights: None, graph: None }, crate::model::tests::T0);
            let recs = Recs::new(cat, &model.taste, idx).with_model(&model);
            let fresh = |picks: &[u128]| picks.iter().filter(|t| cat.position(tid(**t)).is_some_and(|i| !recs.heard(i))).count();
            let usual = play_model_radio(&l, OST.0 + 3, 3, 0);
            let only = play_model_radio_with(&l, OST.0 + 3, 3, 0, |r| {
                r.set_unheard(1.0);
                r.set_explore(Some(0.0));
            });
            assert!(only.len() >= 25, "{only:?}");
            assert!(only.iter().all(|t| kind_of(*t) == OST.0), "omnivore {omnivore}: {only:?}");
            let worlds: Vec<u128> = only.iter().map(|t| (t - OST.0) / 20).collect();
            for w in worlds.windows(3) {
                assert!(!(w[0] == w[1] && w[1] == w[2]), "a world thrice running: {worlds:?}");
            }
            assert!(fresh(&only) >= fresh(&usual), "omnivore {omnivore}: {} unheard, usually {}", fresh(&only), fresh(&usual));
        }
    }

    /// The knobs keep a sparse seed's scene: with «Только новые» and
    /// «Никогда» its language still comes before another.
    #[test]
    fn the_knobs_keep_a_sparse_seeds_scene() {
        let picks = sparse_autoplay_with(102, &[100, 101], None, |r| {
            r.set_unheard(1.0);
            r.set_explore(Some(0.0));
        });
        let ru = LINKED_TRACKS.len() + OTHER_RU.1 as usize;
        assert!(picks.len() >= 25, "{picks:?}");
        assert!(picks[..ru].iter().all(|t| russian(*t)), "the Russian first: {picks:?}");
    }
}
