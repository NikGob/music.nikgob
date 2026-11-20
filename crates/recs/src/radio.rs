//! Endless selections: the radio of a track or an
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
//! Repeats: a radio never serves a pick twice,
//! and leaves out what played in its session or in the last few hours
//! («Не повторять сыгранное», [`RadioState::set_no_repeat_ms`], three
//! hours by default) until nothing else is left; one played twice running
//! plays once more first; a mix leaves what played in the last three
//! hours out. Both score what played lately less, a mix enough to let it
//! rest about a day and a half ([`MIX_REST`]).
//!
//! A track's radio (and autoplay, which is one) picks by the user model
//! when the engine has it ([`crate::model`]): one track at a
//! time as if the ones before it played, among the candidates of the
//! session, by its value — the model's relevance, plus
//! `1.5·ln(1 − p_skip)`, plus [`RADIO_REST`] times how rested it is (1.5
//! days to half), minus the dislikes near it, a little seeded chance.
//! «Похоже на X» stays X's: its candidates are within X's sound radius
//! ([`SEED_RADIUS`] median distances, nearer in a big library) and of a
//! kind of song that goes with X's, the anchor of the sound is X's for
//! [`SEED_HOLD`] and the session's for the rest, so affinity only orders
//! what is near. The diversity rules always hold (artist, album, world);
//! the next one on the album goes on only right after the user's own start
//! from an album. After two rejections running the model weighs the
//! rejected sounds' push and the session's lane twice ([`SKIP_SHIFT`]) and
//! the radio leaves the taste profile they were of for another of a kind
//! of song that goes with X, while there is one: two skips change the
//! cluster. A seed with few neighbours of its own scene (its artists, what the
//! graph links to them, its language) keeps to that scene before anything
//! else: its artists' and their graph neighbours' tracks of any sound,
//! then its language of any sound, and only then the rest. A track seed
//! the catalog lacks (a stream played from its album's page) is known by
//! what the runtime tells of it ([`SeedFacts`]: its artists, sound and
//! language).
//! The session is the runtime's own, handed to the radio before it picks
//! ([`RadioState::attune`]), else the radio's tracks. Only what the session
//! approved leads the anchor ([`crate::session::Verdict`]): a skip in a
//! track's first half or soon after a scrub pushes; a fast skip of a loved
//! one counts once in a run of skips, as any other («not now» of one song
//! is no reason to leave its profile).
//!
//! Autoplay goes on by the session's trajectory: the last
//! [`TRAJECTORY_MAX`] tracks not skipped (played through, liked, or still
//! playing), back to the user's own last pick, captured when autoplay
//! starts. Its anchor drifts elastically:
//! [`TRAJECTORY_DRIFT`] the last [`TRAJECTORY_RECENT`] approved (its own
//! picks among them as they play), the rest the trajectory itself (the
//! context it came from); what the graph links to several of the
//! trajectory gains ([`TRAJECTORY_MEET`]), and after a mixed session it
//! keeps to the profiles the trajectory had. A trajectory of one track is
//! that track's radio.
//!
//! A list's radio («Радио по альбому», «Радио по плейлисту»,
//! [`RadioSeed::Tracks`]) goes the same way from the start: its tracks
//! ([`BLEND_MAX`] of them at most, spread over the list) are its
//! trajectory, so it blends their sounds and what the graph links to
//! several of them; their artists make its scene, one artist among the
//! rest, and the diversity rules hold as in any radio. It keeps to the
//! profile most of the list is of, or to the list's profiles.
//!
//! Every radio the runtime plays ranks by the user model once it has one:
//! a track's radio and autoplay pick by it alone; an artist's, a service's
//! song's, a mood's, a style's and the daylist's gather their candidates
//! by their seed and add the model's value of each ([`W_MODEL`]: the model
//! of the next start at the session's moment, the chance of an early skip),
//! so a skip turns them too. In every one what played to the end many
//! times in the last days cools a little ([`COOL`],
//! [`crate::Taste::saturation_at`]) — the user's own lists never — and the
//! next pick of a model radio minds what a DJ would ([`DJ_VALUE`]). A like
//! of a track while it plays makes the next extension start with the best
//! of its neighbours ([`RadioState::liked`]); a pick taken out of the queue
//! pushes as one that did not land ([`RadioState::removed`]); picks dropped
//! unplayed are forgotten ([`RadioState::forget`]).
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
//! It keeps to a taste profile: «Похоже на X» to
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
use nikgob_core::streaming::{RemoteId, StreamTrack};
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::index::euclid;
use crate::model::{F_COOC, F_GRAPH, F_NEXT_ALBUM, F_PGRAPH, F_SAME_LANE, F_SKIP_PUSH, F_SOUND, HAND, UserModel, Weights};
use crate::mood::Mood;
use crate::pick::{Diversity, Dj, Item, NewTrack, Pick, Placed, Reason, Recs, Seen, Slot, arrange, dj_cost, new_count, select_diverse, select_more, take, token_of_artist};
use crate::rng::Rng;
use crate::mood::Axes;
use crate::listening::Mode;
use crate::profiles::LaneKey;
use crate::open_world::{OpenCandidate, WaveContext};
use crate::session::{Ended, MISSED_PUSH, Scrub, Session, Verdict, judge};
use crate::songtype::{SongLane, SongType, Voice};

use crate::taste::{DayPart, HOUR_MS};

mod by_model;
mod library;

use by_model::{BG_NEW_AFTER_MS, BG_NEW_SHARE, FOCUS_NEW, SEED_RADIUS, SeedAnchor, trajectory_of};
use library::{Ranker, Sound, library_scale, make_way, sort_by_score, take_works, works_of};

/// Served items remembered (never repeated while remembered).
const SERVED_CAP: usize = 2_000;
/// The works of new songs a radio keeps known ([`RadioState::note_stream_works`]).
const STREAM_WORKS_KEEP: usize = 256;
/// When the library runs dry, only this many last served stay excluded.
const RELAXED_SERVED: usize = 50;
/// A mix leaves tracks played this recently for later (unless the pool is
/// dry).
const RECENT_MS: i64 = 3 * HOUR_MS;
/// A wave's picks queued ahead that its next picks go on from, at most.
const QUEUED_MAX: usize = 6;

/// What of `served` is still ahead: what follows the later of the
/// session's last song (`session_last`, a library song) and the last told
/// played (`told`, of either kind); all of it while nothing played; none
/// when what played is not among it (the queue is not known).
fn queued_after<'a>(served: &'a VecDeque<Item>, session_last: Option<TrackId>, told: Option<&Item>) -> Vec<&'a Item> {
    let by_session = session_last.and_then(|t| served.iter().rposition(|x| matches!(x, Item::Track(u) if *u == t)));
    let by_told = told.and_then(|it| served.iter().rposition(|x| x == it));
    match by_session.max(by_told) {
        Some(k) => served.iter().skip(k + 1).collect(),
        None if session_last.is_none() && told.is_none() => served.iter().collect(),
        None => Vec::new(),
    }
}
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
pub(crate) const SKIP_VALUE: f32 = 1.5;
/// …a rest this long halves the bonus of being rested…
pub(crate) const REST_HALF_MS: i64 = 36 * HOUR_MS;
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
pub(crate) const RADIO_REST: f32 = 0.3;
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
/// Autoplay's trajectory: the last this many tracks of the session it did
/// not reject, back to the user's own last pick…
const TRAJECTORY_MAX: usize = 8;
/// …at least this many (fewer: the last track alone is the seed)…
const TRAJECTORY_MIN: usize = 2;
/// …the last this many approved of the session lead the anchor…
const TRAJECTORY_RECENT: usize = 4;
/// …by this share, the trajectory (the context it came from) the rest.
const TRAJECTORY_DRIFT: f32 = 0.6;
/// A track the graph links to more than one track of the trajectory gains
/// this much of its link for each more.
const TRAJECTORY_MEET: f32 = 0.5;
/// A list's radio blends this many of its tracks at most.
pub const BLEND_MAX: usize = 24;
/// What played to the end many times in the last days scores this much
/// less in a stream, worn through ([`crate::Taste::saturation_at`]).
pub(crate) const COOL: f32 = 0.4;
/// A model pick costs this much for the worst step a DJ could take from
/// the track before ([`crate::pick::dj_cost`]).
const DJ_VALUE: f32 = 0.25;
/// A pick taken out of the queue by hand pushes the picks as hard as one
/// that did not land.
const REMOVED_PUSH: f32 = MISSED_PUSH;
/// The user model's part of the score of a radio picked by its seed (an
/// artist, a service's song, a mood, a style, the daylist).
const W_MODEL: f32 = 0.5;
/// A liked track's neighbours: within this share of the radius of a radio
/// of it.
const PULL_RADIUS: f32 = 0.5;

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
    /// A list's radio («Радио по альбому», «Радио по плейлисту»): catalog
    /// tracks of the list ([`RadioState::of_list`] takes a sample).
    Tracks(Vec<TrackId>),
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
    /// «Моя волна» and the cards that start it with a filter of their own:
    /// see [`crate::wave`].
    Wave(crate::wave::WaveSeed),
}

/// How a served track ended (session learning).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Complete,
    /// Skipped at `position_ms` of its `duration_ms` (when known), the bar
    /// dragged before (`scrub`).
    Skip { position_ms: u32, duration_ms: Option<u32>, scrub: Option<Scrub> },
}

impl Outcome {
    /// A skip at `position_ms`, nothing else known.
    pub fn skip(position_ms: u32) -> Self {
        Self::Skip { position_ms, duration_ms: None, scrub: None }
    }

    fn ended(self) -> Ended {
        match self {
            Self::Complete => Ended::Complete,
            Self::Skip { position_ms, duration_ms, .. } => crate::session::skip_ended(position_ms, duration_ms),
        }
    }

    /// What it says of the track ([`crate::session::judge`]).
    pub fn verdict(self) -> Verdict {
        match self {
            Self::Complete => Verdict::Approved,
            Self::Skip { position_ms, duration_ms, scrub } => judge(self.ended(), Some(position_ms), duration_ms, scrub),
        }
    }
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
    /// The last told played (its end, [`Self::feedback`], or its start,
    /// [`Self::note_played`]), a library song or a new one: where the wave's
    /// queue stands ([`queued_after`]).
    #[serde(default)]
    played_last: Option<Item>,
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
    /// or its own ran out.
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
    /// Autoplay's trajectory, captured at its first extension (empty: its
    /// seed alone).
    #[serde(default)]
    trajectory: Option<Vec<TrackId>>,
    /// Liked while it played (and its sound, raw, for one the catalog
    /// lacks): the next extension starts with the best of its neighbours.
    #[serde(default)]
    pull: Option<(Item, Option<Vector>)>,
    /// Taken out of the queue by hand (push).
    #[serde(default)]
    removed: Vec<Item>,
    /// The runtime handed it a session: it ranks by the user model.
    #[serde(skip)]
    attuned: bool,
    /// «Моя волна»: the bandit of the user's clusters ([`crate::wave`]).
    #[serde(default)]
    wave: Option<crate::wave::Bandit>,
    /// What the wave's last picks asked of the runtime.
    #[serde(skip)]
    wave_asks: crate::wave::WaveAsks,
    /// What waves learnt before ([`crate::wave::BanditMemory`]), handed
    /// to the bandit when it is made.
    #[serde(skip)]
    wave_memory: Option<crate::wave::BanditMemory>,
    /// The songs queued after the one playing, in the queue's order, as the
    /// runtime told them for the next extension ([`Self::set_upcoming`]).
    #[serde(skip)]
    upcoming: Option<Vec<Item>>,
    /// The works of the new songs a wave served and of the streams the
    /// session queued or played ([`crate::discover::stream_work_key`],
    /// [`STREAM_WORKS_KEEP`] at most): another version of them is not
    /// picked even once the pool no longer holds them.
    #[serde(default)]
    stream_works: VecDeque<String>,
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

/// The taste profile more than half of a list's tracks the model knows
/// are of.
fn list_profile(model: &UserModel, tracks: &[TrackId]) -> Option<usize> {
    let mut count: HashMap<usize, usize> = HashMap::new();
    let mut known = 0usize;
    for i in tracks.iter().filter_map(|t| model.position(*t)) {
        known += 1;
        if let Some(j) = model.profile_at(i) {
            *count.entry(j).or_default() += 1;
        }
    }
    count.into_iter().find(|(_, c)| 2 * c > known).map(|(j, _)| j)
}

/// Up to `max` of `items` (each once), spread evenly over them in their
/// order.
fn spread<T: PartialEq + Copy>(items: &[T], max: usize) -> Vec<T> {
    let mut once: Vec<T> = Vec::with_capacity(items.len().min(4 * max));
    for x in items {
        if !once.contains(x) {
            once.push(*x);
        }
    }
    if once.len() <= max {
        return once;
    }
    (0..max).map(|k| once[k * once.len() / max]).collect()
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
            played_last: None,
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
            trajectory: None,
            pull: None,
            removed: Vec::new(),
            attuned: false,
            wave: matches!(seed, RadioSeed::Wave(_)).then(crate::wave::Bandit::default),
            wave_asks: crate::wave::WaveAsks::default(),
            wave_memory: None,
            upcoming: None,
            stream_works: VecDeque::new(),
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
            // A list: up to [`BLEND_MAX`] of its tracks (each once), spread
            // evenly over it, are its trajectory from the start.
            RadioSeed::Tracks(tracks) => {
                let sample = spread(tracks, BLEND_MAX);
                s.trajectory = Some(sample.clone());
                s.seed = RadioSeed::Tracks(sample);
            }
            _ => {}
        }
        s
    }

    /// A list's radio ([`RadioSeed::Tracks`]).
    pub fn of_list(tracks: &[TrackId]) -> Self {
        Self::new(RadioSeed::Tracks(tracks.to_vec()))
    }

    /// The listening session to continue and the weights learnt since the
    /// engine's model was built (the runtime's, before each extension).
    pub fn attune(&mut self, session: Session, learned: Option<Weights>) {
        self.session = session;
        self.learned = learned;
        self.attuned = true;
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
        matches!(self.seed, RadioSeed::Track(_) | RadioSeed::Artist(_) | RadioSeed::Tracks(_))
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
        // One version of a song (as [`Self::extend`] keeps it).
        let by_work = self.new_taken_by_work(&served);
        let seed_vec = self.seed_vector(recs);
        let mut slots: Vec<Slot> = self
            .new_pool
            .iter()
            .enumerate()
            .filter(|(_, t)| !served.contains(&Item::Stream(t.id.clone())) && !by_work.contains(&Item::Stream(t.id.clone())))
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

    /// The new songs not to pick by their works ([`NewTrack::work`]):
    /// another version of a song `served`, and of two versions of one song
    /// in the pool the later.
    fn new_taken_by_work(&self, served: &HashSet<Item>) -> HashSet<Item> {
        let mut works: HashSet<&str> = self.new_pool.iter().filter(|t| !t.work.is_empty() && served.contains(&Item::Stream(t.id.clone()))).map(|t| t.work.as_str()).collect();
        let mut taken = HashSet::new();
        for t in self.new_pool.iter().filter(|t| !t.work.is_empty() && !served.contains(&Item::Stream(t.id.clone()))) {
            if !works.insert(t.work.as_str()) {
                taken.insert(Item::Stream(t.id.clone()));
            }
        }
        taken
    }

    /// `id` is among the new-music candidates kept.
    pub fn has_new(&self, id: &RemoteId) -> bool {
        self.new_pool.iter().any(|t| t.id == *id)
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
        matches!(self.seed, RadioSeed::Mood(_) | RadioSeed::Style { .. } | RadioSeed::Discover | RadioSeed::Daylist { .. } | RadioSeed::Wave(_))
    }

    /// How a served track ended: a rejection (a skip in its first half or
    /// after a scrub) pushes later picks away from its sound, an approval
    /// (to the end, or to its outro) pulls them towards it; a skip past the
    /// half teaches nothing. A radio's track played twice running plays
    /// once more next.
    /// `item` played (its end not told, or not known: a new song in a
    /// replay): the queue stands after it.
    pub fn note_played(&mut self, item: Item) {
        self.played_last = Some(item);
    }

    pub fn feedback(&mut self, item: Item, outcome: Outcome) {
        self.judged(item, outcome, outcome.verdict());
    }

    /// [`Self::feedback`] with what the user's session says of it
    /// ([`Session::verdict`]: a like, «not now», «did not land»), which the
    /// lessons and the wave's clusters take instead of the end's own.
    pub fn judged(&mut self, item: Item, outcome: Outcome, verdict: Verdict) {
        self.played_last = Some(item.clone());
        if !self.is_mix() && self.recent.last() == Some(&item) && outcome == Outcome::Complete {
            self.again = Some(item.clone());
        }
        if let Item::Track(t) = &item {
            let open = self.session.tracks().rev().any(|p| p.track == *t && p.ended == Ended::Open);
            if !open {
                let ts = self.session.last_ts().map_or(0, |l| l + SIM_MS);
                self.session.start(*t, ts, nikgob_core::model::EventContext::Radio, None);
            }
            match outcome {
                Outcome::Complete => self.session.end(*t, Ended::Complete),
                Outcome::Skip { position_ms, duration_ms, scrub } => {
                    self.session.end_at(*t, outcome.ended(), position_ms, duration_ms, scrub);
                }
            }
        }
        self.recent.push(item.clone());
        let over = self.recent.len().saturating_sub(REPEAT_AFTER);
        self.recent.drain(..over);
        if let Some(b) = &mut self.wave {
            b.feedback(&item, verdict);
        }
        match verdict {
            Verdict::Approved => {
                self.skipped.retain(|x| *x != item);
                push_capped(&mut self.liked, item, LESSONS);
            }
            v if v.rejects() => {
                self.liked.retain(|x| *x != item);
                push_capped(&mut self.skipped, item, LESSONS);
            }
            _ => {}
        }
    }

    /// `item` was liked while it played (`vector`: its raw sound, for a
    /// track the catalog lacks): it is approved, and the next extension
    /// starts with the best of its neighbours.
    pub fn liked(&mut self, item: Item, vector: Option<Vector>) {
        if let Item::Track(t) = &item {
            self.session.like(*t);
        }
        self.skipped.retain(|x| *x != item);
        self.removed.retain(|x| *x != item);
        if let Some(b) = &mut self.wave {
            b.liked(&item);
        }
        push_capped(&mut self.liked, item.clone(), LESSONS);
        self.pull = Some((item, vector));
    }

    /// `item` was taken out of the queue by hand: its sound pushes the
    /// picks away ([`REMOVED_PUSH`]); it is never picked again (served).
    pub fn removed(&mut self, item: Item) {
        if let Some(b) = &mut self.wave {
            b.removed(&item);
        }
        self.liked.retain(|x| *x != item);
        push_capped(&mut self.removed, item, LESSONS);
    }

    /// The picks `items` left the queue unplayed (its tail replaced): they
    /// are not served, and the radio goes on from what is still queued.
    pub fn forget(&mut self, items: &[Item]) {
        if items.is_empty() {
            return;
        }
        if let Some(b) = &mut self.wave {
            b.forget(items);
        }
        let before = self.served.len();
        self.served.retain(|x| !items.contains(x));
        let gone = before - self.served.len();
        let keep = self.tail.len().saturating_sub(gone);
        self.tail.truncate(keep);
        self.last = self.served.back().cloned();
    }

    /// The next `n` of a wave ([`RadioSeed::Wave`], see [`crate::wave`])
    /// over `ctx` and the open `pool`; the state moves on (served, the
    /// bandit). Nothing for another seed.
    pub fn extend_wave(&mut self, ctx: &WaveContext<'_>, pool: &[OpenCandidate], n: usize, seed: u64) -> Vec<Pick> {
        let RadioSeed::Wave(wave) = &self.seed else { return Vec::new() };
        let wave = wave.clone();
        // The library's songs ranked by the model as learnt since the
        // engine's was built, as the radio's are.
        let learned = self.learned;
        let ctx = &WaveContext { learned: learned.as_ref().or(ctx.learned), ..*ctx };
        let memory = self.wave_memory.take();
        let bandit = self.wave.get_or_insert_with(crate::wave::Bandit::default);
        if let Some(mem) = memory {
            bandit.memory = mem;
        }
        bandit.seed_once(seed);
        self.turns += 1;
        // Served: track ids, and the songs of the pool by their keys.
        let by_id: HashMap<&RemoteId, &str> = pool.iter().map(|c| (&c.track.id, c.key.as_str())).collect();
        let keyed: Vec<(String, bool)> = self
            .served
            .iter()
            .map(|item| match item {
                Item::Track(t) => (t.to_string(), false),
                Item::Stream(r) => (by_id.get(r).map_or_else(|| r.to_string(), |k| (*k).to_owned()), true),
            })
            .collect();
        let served: HashSet<String> = keyed.iter().map(|k| k.0.clone()).collect();
        let again = crate::wave::repeatable(&keyed);
        let cool = self.no_repeat_ms.max(NO_REPEAT_MS);
        // The songs queued after the one playing, the new ones by their
        // artists: they play first.
        // The queue as it stands when the runtime told it (a song waiting
        // for its service may have been passed over), else what of the
        // served is still ahead.
        let told = self.upcoming.take();
        let after: Vec<&Item> = match &told {
            Some(list) => list.iter().collect(),
            None => queued_after(&self.served, self.session.last().map(|p| p.track), self.played_last.as_ref()),
        };
        let queued: Vec<crate::open_world::Ahead> = after
            .iter()
            .map(|x| match x {
                Item::Track(t) => crate::open_world::Ahead::Track(*t),
                Item::Stream(r) => pool.iter().find(|c| c.track.id == *r).map_or(crate::open_world::Ahead::New(crate::open_world::ArtistKey::Unknown), |c| crate::open_world::Ahead::of_new(ctx, c)),
            })
            .collect();
        let queued = &queued[queued.len().saturating_sub(QUEUED_MAX)..];
        let skipped_new: Vec<RemoteId> = self.skipped.iter().chain(&self.removed).filter_map(|i| if let Item::Stream(r) = i { Some(r.clone()) } else { None }).collect();
        let works: Vec<String> = self.stream_works.iter().cloned().collect();
        let picked = crate::wave::pick(bandit, &wave, ctx, pool, &self.session, n, &served, &again, cool, queued, &skipped_new, &works);
        self.wave_asks = picked.asks;
        if picked.scene_let_go {
            // The chip shows what plays.
            let mut wave = wave;
            wave.filter.scene = crate::open_world::Scene::All;
            wave.only.clear();
            self.seed = RadioSeed::Wave(wave);
        }
        let out: Vec<Pick> = picked.picks.into_iter().map(|p| Pick { reason: if p.new { Reason::New } else { Reason::Favourite }, item: p.item }).collect();
        // The new songs' works stay known whatever the pool becomes.
        let picked_works: Vec<String> =
            out.iter().filter_map(|p| if let Item::Stream(r) = &p.item { pool.iter().find(|c| c.track.id == *r).map(|c| crate::discover::stream_work_key(&c.track)) } else { None }).collect();
        self.note_stream_works(picked_works);
        for p in &out {
            self.served.push_back(p.item.clone());
        }
        while self.served.len() > SERVED_CAP {
            self.served.pop_front();
        }
        if let Some(p) = out.last() {
            self.last = Some(p.item.clone());
        }
        out
    }

    /// What the wave's last picks asked of the runtime (see
    /// [`crate::wave::WaveAsks`]), taken once.
    pub fn take_wave_asks(&mut self) -> crate::wave::WaveAsks {
        std::mem::take(&mut self.wave_asks)
    }

    /// The wave's filter, when it is a wave.
    pub fn wave_seed(&self) -> Option<&crate::wave::WaveSeed> {
        match &self.seed {
            RadioSeed::Wave(w) => Some(w),
            _ => None,
        }
    }

    /// The wave's bandit, once it picked.
    pub fn wave_bandit(&self) -> Option<&crate::wave::Bandit> {
        self.wave.as_ref()
    }

    /// Whether the new song `c` it served, now that its preview is heard,
    /// still fits what it plays: a wave's filter with the scene of the
    /// cluster it was picked for ([`crate::open_world::still_fits`]); any
    /// other radio's as it picks a new song heard ([`Self::extend`]): not
    /// near the user's dislikes, quiet enough for a radio of the
    /// background, and near the seed when «Подмешивать случайное» keeps
    /// what does not sound like it to a share.
    pub fn still_fits(&self, ctx: &WaveContext<'_>, c: &OpenCandidate) -> bool {
        match &self.seed {
            RadioSeed::Wave(w) => {
                let item = Item::Stream(c.track.id.clone());
                let scene = self.wave.as_ref().and_then(|b| b.scene_of(&item)).unwrap_or(w.filter.scene);
                crate::open_world::still_fits(ctx, &crate::open_world::WaveFilter { scene, ..w.filter }, c)
            }
            _ => {
                let Some(v) = &c.vector else { return true };
                let m = ctx.model;
                let e = m.embed(v);
                if m.dislike_near_of(&e) >= crate::open_world::MISFIT_DISLIKE {
                    return false;
                }
                if matches!(self.seed, RadioSeed::Track(_) | RadioSeed::Tracks(_)) && self.plan(m).background && !self.plan(m).quiet_new(m, Some(&e)) {
                    return false;
                }
                if self.explore_cap().is_some()
                    && let Some((p, r)) = self.fit_zone_of(m)
                {
                    return euclid(&e, &p) <= r;
                }
                true
            }
        }
    }

    /// [`Self::fit_zone`] by the user model alone (a radio's new song heard
    /// later): the seed's point and radius, for a seed with a sound.
    fn fit_zone_of(&self, m: &UserModel) -> Option<(Vector, f32)> {
        let mean = |vs: Vec<Vector>| (!vs.is_empty()).then(|| std::array::from_fn(|k| vs.iter().map(|v| v[k]).sum::<f32>() / vs.len() as f32));
        let point: Option<Vector> = match &self.seed {
            RadioSeed::Track(t) => {
                let a = SeedAnchor::of(m, Some(*t), self.seed_facts.as_ref());
                return a.point.map(|p| (p, a.radius));
            }
            RadioSeed::Stream { vector, .. } => vector.map(|v| m.embed(&v)),
            RadioSeed::Artist(a) => mean(m.of_artist(*a).iter().filter_map(|&i| m.embedded_at(i as usize).copied()).collect()),
            RadioSeed::Tracks(ts) => mean(ts.iter().filter_map(|t| m.position(*t)).filter_map(|i| m.embedded_at(i).copied()).collect()),
            _ => None,
        };
        point.map(|p| (p, SEED_RADIUS * m.scale()))
    }

    /// The songs queued after the one playing, in the queue's order: the
    /// next extension of a wave goes on from them (not from the order it
    /// served them in: a song waiting for its service may have been passed
    /// over).
    pub fn set_upcoming(&mut self, items: Vec<Item>) {
        self.upcoming = Some(items);
    }

    /// The works of streams queued or played this session (the runtime
    /// tells them): a wave picks no other version of them.
    pub fn note_stream_works(&mut self, works: impl IntoIterator<Item = String>) {
        for w in works {
            if !w.is_empty() && !self.stream_works.contains(&w) {
                self.stream_works.push_back(w);
            }
        }
        while self.stream_works.len() > STREAM_WORKS_KEEP {
            self.stream_works.pop_front();
        }
    }

    /// What waves learnt before (the runtime keeps it): the wave's
    /// clusters start from it.
    pub fn set_wave_memory(&mut self, memory: crate::wave::BanditMemory) {
        match &mut self.wave {
            Some(b) => b.memory = memory,
            None => self.wave_memory = Some(memory),
        }
    }

    /// What the wave learnt so far, its memory before included.
    pub fn wave_memory(&self) -> Option<&crate::wave::BanditMemory> {
        self.wave.as_ref().map(|b| &b.memory).or(self.wave_memory.as_ref())
    }


    /// The wave plays `seed` from its next picks (a chip of the queue's
    /// header): its clusters are found again for it.
    pub fn set_wave(&mut self, seed: crate::wave::WaveSeed) {
        if matches!(self.seed, RadioSeed::Wave(_)) {
            self.seed = RadioSeed::Wave(seed);
        }
    }

    /// «↻ Другой вайб»: the wave moves from the cluster of the song
    /// `playing` to another from its next pick, the song counted for
    /// nothing. False for a radio that is no wave, or a wave of one
    /// cluster.
    pub fn other_vibe(&mut self, playing: Option<&Item>) -> bool {
        self.wave.as_mut().is_some_and(|b| b.other_vibe(playing))
    }

    /// Autoplay's trajectory as captured (empty: none yet, or its seed
    /// alone).
    pub fn trajectory(&self) -> &[TrackId] {
        self.trajectory.as_deref().unwrap_or(&[])
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

    /// The user model's ranker of a radio the runtime plays (none before
    /// there is a model).
    fn ranker<'a>(&self, recs: &Recs<'a>) -> Option<Ranker<'a>> {
        let model = recs.model.filter(|_| self.attuned)?;
        let mut m = model.moment(&self.session, recs.now());
        m.pushes.extend(self.removed.iter().filter_map(|x| self.model_vector(model, x)).map(|v| (v, REMOVED_PUSH)));
        Some(Ranker { model, m, weights: self.learned.unwrap_or(model.weights) })
    }

    /// The sound of `item` as the model embeds it.
    fn model_vector(&self, model: &UserModel, item: &Item) -> Option<Vector> {
        match item {
            Item::Track(id) => model.position(*id).and_then(|i| model.embedded_at(i).copied()),
            Item::Stream(id) => self.new_pool.iter().find(|t| t.id == *id).and_then(|t| t.vector).map(|v| model.embed(&v)),
        }
    }

    /// A liked track's sound, embedded: its own, else what the runtime told.
    fn pull_vector(&self, recs: &Recs<'_>, pull: &(Item, Option<Vector>)) -> Option<Vector> {
        self.vector_of(recs, &pull.0).or_else(|| pull.1.map(|v| recs.norm.embed(&v)))
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
            RadioSeed::Tracks(tracks) => {
                let vs: Vec<Vector> = tracks.iter().filter_map(|t| recs.embedded(*t)).collect();
                (!vs.is_empty()).then(|| std::array::from_fn(|k| vs.iter().map(|v| v[k]).sum::<f32>() / vs.len() as f32))
            }
            RadioSeed::Style { centroid, .. } => <[f32; DIM]>::try_from(centroid.as_slice()).ok(),
            RadioSeed::Daylist { centre, .. } => recs.daylist_point(Axes::from_array(*centre)),
            RadioSeed::Mood(_) | RadioSeed::Discover | RadioSeed::Wave(_) => None,
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
            // Every artist of the list, once.
            RadioSeed::Tracks(tracks) => {
                let mut out: Vec<ArtistId> = Vec::new();
                for a in tracks.iter().filter_map(|t| recs.catalog.get(*t)).flat_map(|t| t.artists.iter()) {
                    if !out.contains(a) {
                        out.push(*a);
                    }
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// The next `n` picks; the state moves on (served, tail, turn).
    pub fn extend(&mut self, recs: &Recs<'_>, n: usize, params: &RadioParams) -> Vec<Pick> {
        if n == 0 {
            return Vec::new();
        }
        // A wave without its open pool: the library's side of it.
        if matches!(self.seed, RadioSeed::Wave(_))
            && let Some(m) = recs.model
        {
            let ok = |i: usize| !recs.blocked(i);
            let background = |i: usize| recs.background.and_then(|b| b.get(i)).copied().unwrap_or(false);
            let artist_tracks = |_: &StreamTrack| Vec::new();
            let ctx = WaveContext { model: m, ok: &ok, genre: None, background: &background, artist_tracks: &artist_tracks, index: Some(recs.index), learned: None };
            return self.extend_wave(&ctx, &[], n, params.seed);
        }
        // Autoplay's trajectory, once it has the session it continues.
        if self.follow
            && self.trajectory.is_none()
            && !self.session.is_empty()
            && let Some(m) = recs.model
        {
            self.trajectory = Some(trajectory_of(&self.session, m));
        }
        let pull = self.pull.take();
        let pull_vec = pull.as_ref().and_then(|p| self.pull_vector(recs, p));
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
            (RadioSeed::Track(_) | RadioSeed::Tracks(_), Some(m)) => Some(self.plan(m)),
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
        let skipped: Vec<Vector> = self.skipped.iter().chain(&self.removed).filter_map(|x| self.vector_of(recs, x)).collect();
        let exempt = match self.seed {
            RadioSeed::Artist(a) => Some(token_of_artist(a)),
            _ => None,
        };
        let rules = Diversity {
            exempt,
            ..Diversity::default()
        };
        let served: HashSet<Item> = self.served.iter().cloned().collect();
        // What the session played of works: what it served and its seed.
        let mut played = served.clone();
        if let RadioSeed::Track(t) = &self.seed {
            played.insert(Item::Track(*t));
        }
        let mut taken: HashSet<Item> = HashSet::new();
        let works = take_works(recs, &played, &mut taken);
        // New songs too: another version of one served is not picked, nor
        // two versions of one song in an extension.
        taken.extend(self.new_taken_by_work(&served));
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
            .filter(|(_, t)| !served.contains(&Item::Stream(t.id.clone())) && !taken.contains(&Item::Stream(t.id.clone())))
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
            let (mut lib, wide) = self.by_model(recs, model, plan, want_lib, &served, &rules, &mut rng, &mut taken, again.as_ref(), pull.as_ref());
            self.wide |= wide;
            if lib.len() < want_lib {
                let more = self.library(recs, want_lib - lib.len(), &sound, &served, false, &rules, &mut rng, &mut taken, None);
                lib.extend(more);
            }
            if lib.len() < want_lib {
                let keep_out = self.relax(recs, &played, &served, &works, &mut taken, again.as_ref());
                let more = self.library(recs, want_lib - lib.len(), &sound, &keep_out, false, &rules, &mut rng, &mut taken, None);
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
        let mut lib = self.library(recs, want_lib, &sound, &served, true, &rules, &mut rng, &mut taken, pull_vec.as_ref());
        if lib.len() < want_lib {
            let keep_out = self.relax(recs, &played, &served, &works, &mut taken, again.as_ref());
            let more = self.library(recs, want_lib - lib.len(), &sound, &keep_out, false, &rules, &mut rng, &mut taken, pull_vec.as_ref());
            lib.extend(more);
        }
        make_way(recs, &mut lib, &mut new_slots, reserve, &rules, &mut taken);

        let mut all = new_slots;
        all.extend(lib);
        let mut tail = self.tail.clone();
        let mut out: Vec<Slot> = Vec::with_capacity(n + 1);
        let mut start = match again {
            Some(s) => {
                tail.push(Placed::of(&s));
                out.push(s.clone());
                Some(s)
            }
            None => self.last.as_ref().and_then(|x| self.slot_of(recs, x)),
        };
        // A liked track's nearest neighbour first.
        if let Some(pv) = &pull_vec
            && let Some(k) = all.iter().enumerate().filter_map(|(k, s)| Some((k, euclid(s.vec.as_ref()?, pv)))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
        {
            let s = all.remove(k);
            tail.push(Placed::of(&s));
            out.push(s.clone());
            start = Some(s);
        }
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
    /// A radio run dry lets what it served play again, but for the last
    /// [`RELAXED_SERVED`]: what it then keeps out (the recent, the whole
    /// works of theirs, and every version of a work the session played
    /// other than the recordings that played — the same recording may come
    /// back, another version of it never). The marks of the session's works
    /// leave `taken` (a served version must not stand in its own way by
    /// its song); the picks of this extension and `again` stay in it.
    fn relax(&self, recs: &Recs<'_>, played: &HashSet<Item>, served: &HashSet<Item>, works: &[Item], taken: &mut HashSet<Item>, again: Option<&Slot>) -> HashSet<Item> {
        let keep = RELAXED_SERVED.min(recs.catalog.len() / 3);
        let recent: HashSet<Item> = self.served.iter().rev().take(keep).cloned().collect();
        for item in works {
            taken.remove(item);
        }
        if let Some(s) = again {
            taken.insert(s.item.clone());
        }
        let mut out = works_of(recs, &recent);
        out.extend(recent);
        out.extend(works_of(recs, played).into_iter().filter(|x| !served.contains(x)));
        out
    }
}

#[cfg(test)]
pub(crate) mod tests;

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
