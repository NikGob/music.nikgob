//! «Миксы для тебя» (recs-v2 §5.4, §6; profiles-v1 §6 and the user's
//! decisions of 2026-10-26 and 2026-10-29): five moments of the user's
//! taste, each a mix of about 28 songs fixed for the home's epoch — a run
//! of the app within one part of the day (the runtime's seed): a start of
//! the app or a new part of the day draws another lineup and other songs,
//! a build within the epoch keeps them («На повторе» beside them is the
//! runtime's).
//!
//! * **What a moment is** ([`MixKey`]): a taste profile's own mix; «Всё
//!   вперемешку», every active profile by its share, in blocks of three or
//!   four; the calm and the lively side of everything loved, across the
//!   profiles and their languages (a side of one language at
//!   [`LANG_HOLD`] keeps to it); «Фон», the tracks of a background profile
//!   that suit the background ([`crate::background`]), up to three cards
//!   split by energy, only for a user who listens so; «Смесь», two or
//!   three profiles the user chose, in pairs taking turns. With no profiles
//!   yet (nothing heard, liked or kept) the library's kinds of songs and
//!   its two sides stand in.
//! * **The lineup** ([`Builder::lineup`]): every moment there is — «Фон»
//!   cards by the balance of the profiles (profiles-v1 §5.3), «Всё
//!   вперемешку» with two active profiles, each active profile, the lively
//!   and the calm side — drawn by the epoch's lottery
//!   ([`Builder::drawn`]): by how well the moment fits now, a profile by
//!   its share too, a moment of the epoch before far less likely and one of
//!   the epoch before it less likely; a language is a limit inside a
//!   moment, not the axis of the cards. A moment that shares [`OVERLAP`] of
//!   its songs with one before it is left out; each kind can be switched
//!   off ([`Settings`]), each moment left out («Не предлагать этот микс»,
//!   [`Settings::hidden`]). The lineup of an epoch stays the epoch's, in
//!   its order ([`Kept`]).
//! * **The songs** ([`Builder::compose`]): by value — the long affinity
//!   (with no plays, the likes' and playlists'), a little of what plays on
//!   repeat, a rest of about a day and a half after a play
//!   ([`crate::radio::MIX_REST`]), `1.2·ln(1 − p_skip)`, the dislikes near;
//!   songs never played, near the loved, [`UNHEARD_SHARE`] at most. Half of
//!   a mix is its core, drawn by value from the best [`CORE_POOL`] times as
//!   many (the epoch before's less likely); the other half rotates by the
//!   epoch (seeded chance, the epoch before's songs of the mix left out),
//!   so two epochs share a third or so; «Другой состав» draws both again.
//!   No interludes, nothing over nine minutes, a song once, an artist once
//!   in three, an album twice in ten, a world twice in three and a quarter
//!   of the mix. The songs and their order stay the epoch's.
//! * **The order** ([`Builder::sequence`]): an arc of energy in the mix's
//!   own percentiles — the day rises to 0.9 at 70 % and eases to 0.6, the
//!   night settles from 0.65 to 0.1, «Фон» keeps 0.4 with small steps; a
//!   loved familiar song first, a calm one often heard out last, a song
//!   never played only between two loved familiar ones and never in the
//!   first two places (one that finds no such place makes room for
//!   another drawn, so a mix keeps its length); each next the nearest in sound on the arc, not of
//!   the world before, of the language before in a mix of many; the rules
//!   mended where the path cornered itself ([`crate::pick::repair`]).
//!   Blocks of a profile join by sound and the arc, never the same profile
//!   twice running while another is left, each ending on its track nearest
//!   the next block.
//! * **The names** ([`Builder::title`], recs-v2 §6.3): what is in it — the
//!   profile's name, «Всё вперемешку», «Спокойное» or «Погромче» (of songs,
//!   without words, or of one language when [`KIND_SHARE`] or
//!   [`LANG_HOLD`] of it is), «Фон», a blend's profiles joined by « + ».
//! * **The order of the cards** ([`Builder::context`]): how much more the
//!   mix's songs play at this part of the day than the user plays then at
//!   all, how hot its worlds are, the profile of the session going on, and
//!   for «Фон» how much of a background the session is.
//! * **What is listened to now leads** ([`Builder::fresh_at`]): a song's
//!   love counts whole while it was heard within [`FRESH_GRACE_MS`] of the
//!   user's latest listening (a play of any origin by its own time, a like
//!   set by hand), then fades by half every [`FRESH_HALF_MS`] down to
//!   [`STALE_FLOOR`]: a like is a state, and a library of likes brought
//!   from a service all at once ([`BULK_LIKES`] within an hour) says
//!   nothing of when they were loved. A loved song not heard for
//!   [`STALE_MS`] (or never, loved by such a state alone) is an old one: a
//!   mix takes [`STALE_SHARE`] of them at most while it has others
//!   («Любимые» half), the rest of them are «Забытое любимое»'s.
//! * **The mixes of the day** ([`Builder::daily`], «Микс дня 1…6»): what the
//!   user listens to now, by taste profile, the heavier profiles split by
//!   sound into two or three (a weighted k-means from the farthest
//!   points), one mix for each cluster, the heaviest first, at most
//!   [`DAILY_MIXES`]. A cluster's mix draws from the songs of its profile
//!   nearest its centre: every song is one cluster's, so no two mixes of
//!   the day share a song. Mostly loved and familiar, [`DAILY_UNHEARD`] of
//!   songs never played that stand near them; seeded by the day, and what
//!   a day drew stays the day's ([`Kept`]) whatever is played meanwhile.
//! * **The mixes that are always there** ([`Builder::pinned`]): «Любимые»,
//!   the liked songs with those heard and liked lately first, and the mix
//!   of this part of the day (the loved songs this part hears more than
//!   the others, the liked ones before the rest), each while it has songs
//!   enough; in the epoch's rotation like any moment.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EventContext, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::index::euclid;
use crate::model::{Live, Moment, UserModel};
use crate::pick::{Diversity, Reason, Recs, Seen, Slot, repair, repair_within};
use crate::profiles::{ACTIVE_SHARE, Kind, LaneKey, NameStyle, background_cards, lang_adjective, quota};
use crate::radio::MIX_REST;
use crate::rng::Rng;
use crate::session::Session;
use crate::songtype::Lang;
use crate::taste::{DAY_MS, DayPart, HOUR_MS};

/// Moments of «Миксы для тебя».
pub const MOMENTS: usize = 5;
/// Mixes of the day at most.
pub const DAILY_MIXES: usize = 6;
/// A cluster of the day needs this many songs listened to now…
const DAILY_CORES: usize = 8;
/// …and a profile gets one more mix for each this many of them (while its
/// songs are enough for as many mixes).
const DAILY_CORES_EACH: usize = 16;
/// Songs never played take at most this share of a mix of the day, and
/// one that fits (near the loved by sound or in the graph) stands this
/// much higher there.
pub const DAILY_UNHEARD: f32 = 0.2;
const W_DAILY_NEW: f32 = 0.5;
/// Songs of «Любимые» (about two hours and a half).
pub const LIKED_LEN: usize = 40;
const LIKED_MS: u64 = 150 * 60_000;
/// The mix of a part of the day draws from the best this many times its
/// length by how much the part hears them; what the part hears more than
/// the others stands this much higher, a liked song this much.
const PART_POOL: usize = 4;
const PART_BONUS: f32 = 1.5;
const PART_LIKED: f32 = 0.3;
/// A love counts whole within this of the user's latest listening…
pub const FRESH_GRACE_MS: i64 = 30 * DAY_MS;
/// …then fades by half every this long…
pub const FRESH_HALF_MS: i64 = 60 * DAY_MS;
/// …down to this.
pub const STALE_FLOOR: f32 = 0.35;
/// What is loved by a state alone (a like, a playlist) and never heard
/// counts this much, once anything was heard.
const UNHEARD_LOVE: f32 = 0.5;
/// A loved song not heard for this long is an old one…
pub const STALE_MS: i64 = 90 * DAY_MS;
/// …and a mix takes this share of them at most while it has others
/// («Любимые» [`LIKED_STALE_SHARE`]).
pub const STALE_SHARE: f32 = 0.25;
pub const LIKED_STALE_SHARE: f32 = 0.5;
/// This many likes within [`BULK_WINDOW_MS`] of one another came from a
/// service's library at once: they say nothing of when.
pub const BULK_LIKES: usize = 20;
const BULK_WINDOW_MS: i64 = HOUR_MS;
/// The blends made today shown at most (the latest).
pub const BLENDS_SHOWN: usize = 2;
/// Songs of a mix (about 90–100 minutes)…
pub const MIX_LEN: usize = 28;
const MIX_MS: u64 = 105 * 60_000;
/// …of «Фон» (about 80 minutes; a radio of the same mode goes on after)…
pub const BG_LEN: usize = 36;
const BG_MS: u64 = 120 * 60_000;
/// …and of a blend.
pub const BLEND_LEN: usize = 24;
/// A mix needs this many songs.
pub const MIN_SONGS: usize = 12;
/// A mix is drawn again at most this many times in place of the songs
/// never played that find no place in its order.
const REDRAWS: usize = 3;
/// No song shorter (an interlude)…
const INTERLUDE_MS: u32 = 75_000;
/// …or longer (skipped early a third of the time, recs-v2 §2).
pub const LONG_MS: u32 = 9 * 60_000;
/// The first and the last song are not longer than this.
const EDGE_MS: u32 = 6 * 60_000;
/// One world takes at most this share of a mix.
const WORLD_SHARE: f32 = 0.25;
/// Songs never played take at most this share of a mix…
pub const UNHEARD_SHARE: f32 = 0.15;
/// …of «Фон».
const BG_UNHEARD_SHARE: f32 = 0.1;
/// The calm side is the loved songs below this percentile of their energy,
/// the lively one above `1 −` it.
const SIDE: f32 = 0.45;
/// A side with this share of one sung language keeps to it (and to songs
/// of no known language that are no soundtracks).
pub const LANG_HOLD: f32 = 0.7;
/// A kind of song names a side at this share of it.
pub const KIND_SHARE: f32 = 0.6;
/// A moment sharing this share of its songs with one before it is the same.
pub const OVERLAP: f32 = 0.65;
/// Value: what plays on repeat, the early skip, the unheard near the loved.
const W_HOT: f32 = 0.4;
const W_SKIP: f32 = 1.2;
const W_EXPLORE: f32 = 0.9;
/// The chance of the rotating half.
const ROTATE: f32 = 0.6;
/// What the graph links to the songs taken stands this much higher in a
/// mix of one kind (times the tanh of the links).
const W_COHERE: f32 = 0.8;
/// The core is drawn from the best this many times its size, by value over
/// a third of their spread (the best of them some twenty times likelier
/// than the last); a song of the epoch before is `e` times less likely.
pub const CORE_POOL: usize = 3;
const CORE_SPREAD: f32 = 3.0;
const CORE_BEFORE: f32 = 1.0;
/// The lottery of the lineup: how much the fit to now weighs (its log
/// odds), the least weight of a profile by its share against the largest,
/// and a moment shown in the epoch before (and the one before it).
const LOT_FIT: f32 = 0.8;
const LOT_SHARE: f32 = 0.3;
const LOT_SHOWN: [f32; 2] = [0.12, 0.45];
/// The order's costs: off the arc, the world before, the one before it,
/// another language, a jump in «Фон», the same profile's block again, a
/// block off the arc.
const C_ARC: f32 = 1.5;
const C_WORLD: f32 = 0.8;
const C_WORLD2: f32 = 0.3;
const C_LANE: f32 = 0.6;
const C_STEADY: f32 = 2.0;
/// A block of a profile left before its length, or kept past it.
const C_BLOCK: f32 = 2.0;
/// A block longer than a block may be weighs this against the rules of
/// diversity (a world thrice running weighs more).
const W_BLOCK: usize = 3;
/// An album a third time within ten, and what the places left could no
/// longer hold, only cost along the way: [`repair`] mends what is left.
const C_ALBUM: f32 = 2.0;
const C_CROWD: f32 = 1.0;
/// A song never played out of its place weighs this against the rules of
/// diversity (a world thrice running weighs more).
const W_UNHEARD: usize = 3;
/// «Фон» keeps to this band of its profile's energy (profiles-v1 §6.1),
/// cards of it split this one.
const BG_BAND: (f32, f32) = (0.15, 0.7);
const BG_CARDS_BAND: (f32, f32) = (0.1, 0.9);

/// What a mix of «Миксы для тебя» is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MixKey {
    /// One taste profile, by its key ([`crate::profiles::Profile::key`]).
    Profile(u32),
    /// «Всё вперемешку».
    Everything,
    /// The calm side of everything loved.
    Calm,
    /// The lively side.
    Lively,
    /// «Фон»: card `card` (the calmest first) of the `cards` of the
    /// background profile `profile`.
    Background { profile: u32, card: u8, cards: u8 },
    /// «Смесь» of two or three profiles (a missing third is 0).
    Blend([u32; 3]),
    /// No profiles yet: the library's songs of one kind ([`lane_code`]).
    Kind(u8),
    /// «Любимые»: the liked songs.
    Liked,
    /// The mix of a part of the day, by [`DayPart::index`].
    Part(u8),
    /// «Микс дня» number this (from 1), the heaviest cluster first.
    Daily(u8),
}

impl MixKey {
    /// A stable number of the key (seeds).
    pub fn token(self) -> u64 {
        let (a, b, c, d): (u32, u32, u32, u32) = match self {
            Self::Profile(k) => (1, k, 0, 0),
            Self::Everything => (2, 0, 0, 0),
            Self::Calm => (3, 0, 0, 0),
            Self::Lively => (4, 0, 0, 0),
            Self::Background { profile, card, cards } => (5, profile, u32::from(card), u32::from(cards)),
            Self::Blend([x, y, z]) => (6, x, y, z),
            Self::Kind(k) => (7, u32::from(k), 0, 0),
            Self::Liked => (8, 0, 0, 0),
            Self::Part(p) => (9, u32::from(p), 0, 0),
            Self::Daily(n) => (10, u32::from(n), 0, 0),
        };
        crate::pick::token(&[a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes(), d.to_le_bytes()].concat())
    }

    /// The profile «Смешать с…» starts from.
    pub fn blend_base(self) -> Option<u32> {
        match self {
            Self::Profile(k) | Self::Background { profile: k, .. } => Some(k),
            _ => None,
        }
    }

    /// A blend of `profiles` (two or three, in the order chosen).
    pub fn blend(profiles: &[u32]) -> Option<Self> {
        let mut keys = [0u32; 3];
        let mut n = 0;
        for &k in profiles {
            if k != 0 && !keys[..n].contains(&k) && n < 3 {
                keys[n] = k;
                n += 1;
            }
        }
        (n >= 2).then_some(Self::Blend(keys))
    }
}

/// The number of a kind of song in [`MixKey::Kind`].
pub fn lane_code(l: LaneKey) -> u8 {
    match l {
        LaneKey::Instrumental => 0,
        LaneKey::Unknown => 1,
        LaneKey::Sung(lang) => 2 + LANGS.iter().position(|x| *x == lang).unwrap_or(LANGS.len() - 1) as u8,
    }
}

fn lane_of_code(c: u8) -> LaneKey {
    match c {
        0 => LaneKey::Instrumental,
        1 => LaneKey::Unknown,
        c => LaneKey::Sung(LANGS.get(usize::from(c - 2)).copied().unwrap_or(Lang::Other)),
    }
}

const LANGS: [Lang; 8] = [Lang::Russian, Lang::Japanese, Lang::Korean, Lang::English, Lang::Spanish, Lang::French, Lang::German, Lang::Other];

/// The arc of energy along a mix, in the mix's own percentiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arc {
    /// Rises from 0.45 to 0.9 at 70 % and eases to 0.6.
    Day,
    /// Settles from 0.65 to 0.1.
    Night,
    /// Stays at 0.4 («Фон»).
    Focus,
}

impl Arc {
    /// The arc of a part of the day.
    pub fn of(part: DayPart) -> Self {
        if part == DayPart::Night { Self::Night } else { Self::Day }
    }

    /// The energy wanted at `x` (0…1) along the mix.
    pub fn at(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Self::Night => 0.65 - 0.55 * x,
            Self::Day => {
                let rise = 0.45 + 0.45 * (std::f32::consts::FRAC_PI_2 * (x / 0.7).min(1.0)).sin();
                if x > 0.7 { rise - 0.3 * (x - 0.7) / 0.3 } else { rise }
            }
            Self::Focus => 0.4,
        }
    }

    /// The energy wanted at each of `n` places, as a percentile of the
    /// mix's own: the arc's values ranked among themselves, so every song
    /// has its place on it (a night mix opens on its liveliest and closes
    /// on its calmest, a day's peaks at 70 %). «Фон» wants no arc, only
    /// small steps (`None`).
    pub fn targets(self, n: usize) -> Option<Vec<f32>> {
        if self == Self::Focus {
            return None;
        }
        let at: Vec<f32> = (0..n).map(|k| self.at(k as f32 / n.saturating_sub(1).max(1) as f32)).collect();
        Some(crate::listening::pct_rank(&at))
    }
}

/// Which moments the user wants (Settings → «Рекомендации»).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// The profiles' own mixes.
    pub profiles: bool,
    /// «Всё вперемешку».
    pub everything: bool,
    /// The calm and the lively side.
    pub moods: bool,
    /// «Фон» cards at most (0: none).
    pub background_cards: usize,
    pub names: NameStyle,
    /// «Не предлагать этот микс»: the moments left out.
    pub hidden: Vec<MixKey>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { profiles: true, everything: true, moods: true, background_cards: crate::profiles::BG_CARDS, names: NameStyle::Kind, hidden: Vec::new() }
    }
}

/// What an epoch keeps between builds (the runtime's cache): the lineup in
/// its order, each mix's songs in theirs with the salt they were drawn
/// with, the songs of each in the epoch before, the lineups of the two
/// epochs before, the salts of «Другой состав» and the blends made today.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Kept {
    pub lineup: Vec<MixKey>,
    #[serde(alias = "today")]
    pub songs: Vec<(MixKey, u64, Vec<TrackId>)>,
    #[serde(alias = "yesterday")]
    pub before: Vec<(MixKey, Vec<TrackId>)>,
    /// The latest first.
    #[serde(default)]
    pub shown: Vec<Vec<MixKey>>,
    pub salts: Vec<(MixKey, u64)>,
    pub blends: Vec<MixKey>,
}

impl Kept {
    fn salt(&self, key: MixKey) -> u64 {
        self.salts.iter().find(|s| s.0 == key).map_or(0, |s| s.1)
    }

    fn songs(&self, key: MixKey) -> Option<(u64, &[TrackId])> {
        self.songs.iter().find(|s| s.0 == key).map(|s| (s.1, s.2.as_slice()))
    }

    fn before(&self, key: MixKey) -> &[TrackId] {
        self.before.iter().find(|s| s.0 == key).map_or(&[], |s| s.1.as_slice())
    }

    /// The next epoch: this one's songs become the songs before, its
    /// lineup the latest shown; the lineup and the salts start over, the
    /// blends too on another day.
    pub fn next_epoch(self, same_day: bool) -> Self {
        let mut shown = self.shown;
        if !self.lineup.is_empty() {
            shown.insert(0, self.lineup);
        }
        shown.truncate(LOT_SHOWN.len());
        Self {
            before: self.songs.into_iter().map(|(k, _, t)| (k, t)).collect(),
            shown,
            blends: if same_day { self.blends } else { Vec::new() },
            ..Self::default()
        }
    }

    /// The songs of every mix of `mixes` with their salts, for the epoch.
    pub fn keep(&mut self, mixes: &[Mix], model: &UserModel) {
        for m in mixes {
            let ids: Vec<TrackId> = m.tracks.iter().map(|&i| model.id(i)).collect();
            let salt = self.salt(m.key);
            match self.songs.iter_mut().find(|s| s.0 == m.key) {
                Some(s) => *s = (m.key, salt, ids),
                None => self.songs.push((m.key, salt, ids)),
            }
        }
    }
}

/// One mix of «Миксы для тебя».
#[derive(Clone, Debug, PartialEq)]
pub struct Mix {
    pub key: MixKey,
    pub title: String,
    /// Shorter forms of the title, the longest first (a blend's: a card
    /// too narrow for the whole name says one of these).
    pub short: Vec<String>,
    /// Catalog positions, in the order they play.
    pub tracks: Vec<usize>,
    /// Familiar and loved: a new song of «С новым» may stand between two
    /// of these.
    pub anchors: Vec<bool>,
    /// Its artists, most of its songs first.
    pub artists: Vec<ArtistId>,
    /// The profiles it draws from.
    pub profiles: Vec<usize>,
    /// How well it fits now ([`Builder::context`]).
    pub score: f32,
}

/// The pool of a moment and how it is made.
#[derive(Clone)]
struct Pool {
    tracks: Vec<usize>,
    len: usize,
    max_ms: u64,
    unheard: f32,
    /// Songs of each profile («Всё вперемешку», a blend).
    quota: Option<HashMap<usize, usize>>,
    /// Joined in blocks of this many of a profile (0: one per profile's
    /// count, three or four).
    blocks: Option<usize>,
    arc: Arc,
    /// Songs of many languages: keep the language before.
    cross: bool,
    /// The share of old songs it takes at most.
    stale: f32,
}

/// The engine of «Миксы для тебя» over one snapshot.
pub struct Builder<'a> {
    recs: &'a Recs<'a>,
    model: &'a UserModel,
    suits: &'a dyn Fn(usize) -> bool,
    /// The home's epoch: the seed of the lineup's lottery and the songs.
    epoch: u64,
    part: DayPart,
    live: Live,
    settings: Settings,
    /// Something was played: what never was is held to its share (likes
    /// alone, imported, make a library to mix as it is).
    history: bool,
    /// The affinity of every pickable track as it counts now (what was
    /// heard lately whole, an old love less: [`Self::fresh_at`]), its love
    /// whenever it was, and its value (`None`: not pickable).
    a: Vec<f32>,
    love: Vec<f32>,
    value: Vec<Option<f32>>,
    /// How much of each track's love counts now, and whether it is an old
    /// song.
    fresh: Vec<f32>,
    stale: Vec<bool>,
    /// The pools of the mixes of the day, once asked for.
    daily: std::cell::OnceCell<Vec<Vec<usize>>>,
}

/// Fills `n` places by `pick`: the counts each kind of rule allows.
struct Caps {
    artist: usize,
    album: usize,
    world: usize,
    unheard: usize,
    stale: usize,
}

/// When each of `n` tracks was last listened to (`None`: never): its last
/// play of any origin by the play's own time, or its like when that was
/// set by hand (not one of [`BULK_LIKES`] within an hour).
fn last_heard(recs: &Recs<'_>, model: &UserModel, n: usize) -> Vec<Option<UnixMs>> {
    let mut likes: Vec<UnixMs> = (0..n).filter_map(|i| recs.taste.liked_time_at(i)).collect();
    likes.sort_unstable();
    let by_hand = |t: UnixMs| {
        let from = likes.partition_point(|x| *x < t - BULK_WINDOW_MS);
        let to = likes.partition_point(|x| *x <= t + BULK_WINDOW_MS);
        to - from < BULK_LIKES
    };
    (0..n)
        .map(|i| {
            let s = &model.tracks[i];
            let liked = recs.taste.liked_time_at(i).filter(|t| by_hand(*t));
            [recs.taste.last_played_at(i), s.last_start, s.last_complete, liked].into_iter().flatten().max()
        })
        .collect()
}

/// How much of a love last heard `age` before the user's latest listening
/// counts now.
fn freshness(age: i64) -> f32 {
    if age <= FRESH_GRACE_MS {
        return 1.0;
    }
    let halves = (age - FRESH_GRACE_MS) as f64 / FRESH_HALF_MS as f64;
    STALE_FLOOR + (1.0 - STALE_FLOOR) * (-halves).exp2() as f32
}

impl<'a> Builder<'a> {
    /// The engine over `recs` (whose model is `model`): `ok` the tracks the
    /// runtime lets a mix take, `suits` those that suit the background,
    /// `epoch` the seed of the home's epoch and `part` the part of the day,
    /// `live` the session going on.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        recs: &'a Recs<'a>,
        model: &'a UserModel,
        ok: &dyn Fn(usize) -> bool,
        suits: &'a dyn Fn(usize) -> bool,
        epoch: u64,
        part: DayPart,
        live: Live,
        settings: Settings,
    ) -> Self {
        let now = model.now();
        let m0: Moment = model.moment(&Session::default(), now);
        let n = model.len().min(recs.catalog.len());
        let mut a = vec![0.0f32; n];
        let mut love = vec![0.0f32; n];
        let mut value = vec![None; n];
        // What was heard and when, against the latest of it (a user back
        // after a year has last year's weeks, not none).
        let heard = last_heard(recs, model, n);
        let latest = heard.iter().flatten().copied().max();
        let mut fresh = vec![1.0f32; n];
        let mut stale = vec![false; n];
        for i in 0..n {
            let t = recs.info(i);
            if recs.blocked(i) || !ok(i) || t.duration_ms.is_some_and(|d| !(INTERLUDE_MS..=LONG_MS).contains(&d)) {
                continue;
            }
            let s = &model.tracks[i];
            love[i] = s.aff_long(now).max(0.0).max((recs.taste.affinity_at(i).max(0.0) / 3.0).tanh()).max(model.prior_mass(i));
            // What only the library's prior stands for is no love to fade.
            let state = love[i] > model.prior_mass(i);
            match (latest, heard[i]) {
                (Some(l), Some(h)) => {
                    fresh[i] = freshness(l - h);
                    stale[i] = state && l - h >= STALE_MS;
                }
                (Some(_), None) if state => {
                    fresh[i] = UNHEARD_LOVE;
                    stale[i] = true;
                }
                _ => {}
            }
            a[i] = love[i] * fresh[i];
            let p = model.p_skip(&m0, i, EventContext::HomeSection).min(0.95);
            value[i] = Some(
                a[i] + W_HOT * s.aff_hot(now).max(0.0) - MIX_REST * recs.taste.lately_at(i) + W_SKIP * (1.0 - p).ln()
                    - recs.taste.dislike_near_at(i),
            );
        }
        let history = model.tracks.iter().any(|t| t.starts > 0);
        Self { recs, model, suits, epoch, part, live, settings, history, a, love, value, fresh, stale, daily: std::cell::OnceCell::new() }
    }

    /// How much of the love of the track at `i` counts now (see the module
    /// docs): 1 while it is heard, down to [`STALE_FLOOR`].
    pub fn fresh_at(&self, i: usize) -> f32 {
        self.fresh.get(i).copied().unwrap_or(1.0)
    }

    /// A loved song not heard for [`STALE_MS`], or never.
    pub fn stale_at(&self, i: usize) -> bool {
        self.stale.get(i).copied().unwrap_or(false)
    }

    fn liked(&self, i: usize) -> bool {
        self.model.tracks[i].liked(self.model.now()) || self.recs.taste.liked_at(i)
    }

    fn pickable(&self, i: usize) -> bool {
        self.value.get(i).is_some_and(Option::is_some)
    }

    /// Never played and not loved otherwise (a like, a playlist): what a
    /// mix takes little of.
    fn unheard(&self, i: usize) -> bool {
        self.history && self.model.tracks[i].starts == 0 && self.a[i] <= 0.0
    }

    fn profile_index(&self, key: u32) -> Option<usize> {
        self.model.profiles.list.iter().position(|p| p.key == key)
    }

    fn profile_tracks(&self, j: usize) -> Vec<usize> {
        (0..self.value.len()).filter(|&i| self.pickable(i) && self.model.profile_at(i) == Some(j)).collect()
    }

    /// The profiles shown, active and with songs enough, the largest share
    /// first.
    fn active(&self) -> Vec<usize> {
        let p = &self.model.profiles;
        let mut out: Vec<usize> = (0..p.len())
            .filter(|&j| p.list[j].visible && self.model.profile_share.get(j).is_some_and(|s| *s >= ACTIVE_SHARE))
            .filter(|&j| self.profile_tracks(j).len() >= MIN_SONGS)
            .collect();
        out.sort_by(|a, b| self.model.profile_share[*b].total_cmp(&self.model.profile_share[*a]).then(a.cmp(b)));
        out
    }

    /// The «Фон» cards of the background profiles: the balance's own count
    /// of them (at most the user's), each a key.
    fn background_keys(&self) -> Vec<MixKey> {
        if self.settings.background_cards == 0 {
            return Vec::new();
        }
        let suits = |i: usize| self.pickable(i) && (self.suits)(i);
        let home = self.model.home_profiles(MOMENTS, self.settings.background_cards, suits);
        let mut out = Vec::new();
        for (j, cards) in home.background.iter().enumerate() {
            let (Some(p), n) = (self.model.profiles.list.get(j), cards.len()) else { continue };
            if !p.visible || n == 0 {
                continue;
            }
            for c in 0..n {
                out.push(MixKey::Background { profile: p.key, card: c as u8, cards: n as u8 });
            }
        }
        out
    }

    /// The epoch's moments: the lineup kept for it in its order, else those
    /// the lottery draws ([`Self::drawn`]) in the order of how they fit now;
    /// the blends made today first, in the places of the moments that fit
    /// now least. And the lineup to keep.
    pub fn lineup(&self, kept: &Kept) -> (Vec<Mix>, Vec<MixKey>) {
        let mut chosen: Vec<Mix> = Vec::new();
        // The epoch's lineup first, as it was; new moments only for what
        // of it is gone.
        let hidden = |k: &MixKey| self.settings.hidden.contains(k);
        let before: Vec<MixKey> = kept.lineup.iter().copied().filter(|k| !hidden(k)).collect();
        let fresh = self.drawn(kept).into_iter().filter(|k| !kept.lineup.contains(k) && !hidden(k));
        for (n, key) in before.iter().copied().chain(fresh).enumerate() {
            if chosen.len() >= MOMENTS {
                break;
            }
            let Some(mix) = self.mix(key, kept) else { continue };
            // «Всё вперемешку» takes the best of every profile: it is no
            // other moment's double, nor they its.
            let set: HashSet<usize> = mix.tracks.iter().copied().collect();
            let same = key != MixKey::Everything
                && chosen.iter().filter(|c| c.key != MixKey::Everything).any(|c| {
                    let shared = c.tracks.iter().filter(|i| set.contains(i)).count();
                    shared as f32 >= OVERLAP * set.len().min(c.tracks.len()) as f32
                });
            if n >= before.len() && same {
                continue;
            }
            chosen.push(mix);
        }
        if before.is_empty() {
            chosen.sort_by(|a, b| b.score.total_cmp(&a.score));
        }
        let keys: Vec<MixKey> = chosen.iter().map(|m| m.key).collect();
        // The blends stand in the places of the moments that fit now least
        // (the two latest; those stay in the day's lineup).
        let mut blends: Vec<Mix> = kept.blends.iter().rev().filter_map(|k| self.mix(*k, kept)).take(BLENDS_SHOWN).collect();
        chosen.truncate(MOMENTS.saturating_sub(blends.len()));
        blends.extend(chosen);
        (blends, keys)
    }

    /// Every moment there is, in the order the epoch's lottery draws them
    /// (Efraimidis–Spirakis): each by how well its songs fit now
    /// ([`Self::context`]), a profile's own by its share too (at least
    /// [`LOT_SHARE`] of the largest's chances), one shown in the epochs
    /// before far less ([`LOT_SHOWN`]); every moment has its chances.
    pub fn drawn(&self, kept: &Kept) -> Vec<MixKey> {
        let all = self.candidates();
        let top = self.model.profile_share.iter().copied().fold(0.0f32, f32::max).max(1e-6);
        let mut lots: Vec<(f32, MixKey)> = all
            .into_iter()
            .filter_map(|key| {
                let pool = self.pool(key)?;
                let profiles: Vec<usize> = {
                    let mut p: Vec<usize> = pool.tracks.iter().filter_map(|&i| self.model.profile_at(i)).collect();
                    p.sort_unstable();
                    p.dedup();
                    p
                };
                let probe = Mix { key, title: String::new(), short: Vec::new(), tracks: pool.tracks, anchors: Vec::new(), artists: Vec::new(), profiles, score: 0.0 };
                let mut w = (LOT_FIT * self.context(&probe).clamp(-2.0, 2.0)).exp();
                if let MixKey::Profile(k) = key
                    && let Some(j) = self.profile_index(k)
                {
                    w *= (self.model.profile_share.get(j).copied().unwrap_or(0.0) / top).sqrt().max(LOT_SHARE);
                }
                for (shown, f) in kept.shown.iter().zip(LOT_SHOWN) {
                    if shown.contains(&key) {
                        w *= f;
                    }
                }
                let u = Rng::derive(self.epoch ^ 0x6d69_7865, key.token()).f32().max(1e-6);
                Some((-u.ln() / w.max(1e-6), key))
            })
            .collect();
        lots.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        lots.into_iter().map(|(_, k)| k).collect()
    }

    /// The moments to choose from.
    fn candidates(&self) -> Vec<MixKey> {
        let s = &self.settings;
        let active = self.active();
        if active.is_empty() {
            // Nothing heard yet: the library's kinds.
            let mut kinds: HashMap<LaneKey, usize> = HashMap::new();
            for i in (0..self.value.len()).filter(|&i| self.pickable(i)) {
                *kinds.entry(LaneKey::of(self.model.song_type_at(i))).or_default() += 1;
            }
            let mut kinds: Vec<(LaneKey, usize)> = kinds.into_iter().filter(|k| k.1 >= MIN_SONGS).collect();
            kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let mut out: Vec<MixKey> = kinds.into_iter().map(|k| MixKey::Kind(lane_code(k.0))).collect();
            if s.moods {
                out.extend([MixKey::Lively, MixKey::Calm]);
            }
            return out;
        }
        let bg = self.background_keys();
        let with_bg: HashSet<u32> = bg.iter().filter_map(|k| k.blend_base()).collect();
        let mut out = bg;
        if s.everything && active.len() >= 2 {
            out.push(MixKey::Everything);
        }
        if s.profiles {
            let plain = active.iter().map(|&j| self.model.profiles.list[j].key).filter(|k| !with_bg.contains(k));
            out.extend(plain.map(MixKey::Profile));
        }
        if s.moods {
            out.extend([MixKey::Lively, MixKey::Calm]);
        }
        out
    }

    /// The mix of `key` in the epoch: its songs kept for it in their order,
    /// else drawn (`None` when it has too few).
    pub fn mix(&self, key: MixKey, kept: &Kept) -> Option<Mix> {
        let salt = kept.salt(key);
        let steady = matches!(key, MixKey::Background { .. });
        // A mix of the day and of a part of it stays what it was drawn as,
        // whatever its clusters or its part's best became since; the
        // others keep the songs their pool still has.
        let whole = matches!(key, MixKey::Daily(_) | MixKey::Part(_));
        let pool = self.pool(key);
        let kept_songs = match kept.songs(key) {
            Some((s, ids)) if s == salt && (whole || pool.is_some()) => {
                let have: Option<HashSet<usize>> = if whole { None } else { pool.as_ref().map(|p| p.tracks.iter().copied().collect()) };
                let t: Vec<usize> = ids
                    .iter()
                    .filter_map(|id| self.model.position(*id))
                    .filter(|&i| self.pickable(i) && have.as_ref().is_none_or(|h| h.contains(&i)))
                    .collect();
                (t.len() >= MIN_SONGS).then_some(t)
            }
            _ => None,
        };
        let tracks = match kept_songs {
            Some(t) => t,
            None => {
                let pool = pool?;
                let before = kept.songs(key).map_or(&[][..], |t| t.1);
                // What never played and finds no place between loved ones
                // in the order of the day leaves whatever the part, and
                // others are drawn in its place (the second time no more
                // songs never played than found a place).
                let arc = if pool.arc == Arc::Focus { Arc::Focus } else { Arc::Day };
                let mut left = pool.clone();
                let mut order = Vec::new();
                for round in 0..REDRAWS {
                    let drawn = self.compose(key, &left, salt, kept, before);
                    order = self.order(&drawn, &left, arc, steady, true);
                    let placed: HashSet<usize> = order.iter().copied().collect();
                    let dropped: Vec<usize> = drawn.iter().copied().filter(|i| !placed.contains(i)).collect();
                    if dropped.is_empty() || order.len() >= pool.len {
                        break;
                    }
                    left.tracks.retain(|i| !dropped.contains(i));
                    if round > 0 {
                        let fit = order.iter().filter(|&&i| self.unheard(i)).count();
                        left.unheard = fit as f32 / pool.len.max(1) as f32;
                    }
                }
                if order.len() < MIN_SONGS {
                    return None;
                }
                self.order(&order, &pool, pool.arc, steady, false)
            }
        };
        if tracks.len() < MIN_SONGS {
            return None;
        }
        let mut profiles: Vec<usize> = tracks.iter().filter_map(|&i| self.model.profile_at(i)).collect();
        profiles.sort_unstable();
        profiles.dedup();
        let anchors = self.anchors(&tracks);
        let mut mix = Mix { key, title: String::new(), short: Vec::new(), artists: self.artists(&tracks), anchors, tracks, profiles, score: 0.0 };
        mix.title = self.title(key, &mix.tracks);
        mix.short = self.short_titles(key);
        mix.score = self.context(&mix);
        Some(mix)
    }

    /// What `key` draws from and how.
    fn pool(&self, key: MixKey) -> Option<Pool> {
        let plain =
            |tracks: Vec<usize>| Pool { tracks, len: MIX_LEN, max_ms: MIX_MS, unheard: UNHEARD_SHARE, quota: None, blocks: None, arc: Arc::of(self.part), cross: false, stale: STALE_SHARE };
        let pool = match key {
            MixKey::Profile(k) => plain(self.profile_tracks(self.profile_index(k)?)),
            MixKey::Liked => {
                let liked: Vec<usize> = (0..self.value.len()).filter(|&i| self.pickable(i) && self.liked(i)).collect();
                Pool { len: LIKED_LEN, max_ms: LIKED_MS, cross: true, stale: LIKED_STALE_SHARE, ..plain(liked) }
            }
            MixKey::Part(p) => {
                let part = DayPart::ALL.get(usize::from(p)).copied()?;
                Pool { arc: Arc::of(part), cross: true, ..plain(self.part_tracks(part)) }
            }
            MixKey::Daily(n) => {
                let tracks = self.daily_pools().get(usize::from(n).checked_sub(1)?)?.clone();
                // The day's order, whatever the part.
                Pool { unheard: DAILY_UNHEARD, arc: Arc::Day, ..plain(tracks) }
            }
            MixKey::Kind(c) => {
                let lane = lane_of_code(c);
                plain((0..self.value.len()).filter(|&i| self.pickable(i) && LaneKey::of(self.model.song_type_at(i)) == lane).collect())
            }
            MixKey::Everything => {
                let active = self.active();
                if active.len() < 2 {
                    return None;
                }
                let shares: Vec<f32> = active.iter().map(|&j| self.model.profile_share[j]).collect();
                let counts = quota(&shares, MIX_LEN, &vec![false; active.len()], MIX_LEN);
                let tracks = active.iter().flat_map(|&j| self.profile_tracks(j)).collect();
                Pool { quota: Some(active.iter().copied().zip(counts).collect()), blocks: Some(0), cross: true, ..plain(tracks) }
            }
            MixKey::Calm | MixKey::Lively => Pool { cross: true, ..plain(self.side(key == MixKey::Calm)) },
            MixKey::Background { profile, card, cards } => {
                let j = self.profile_index(profile)?;
                let mut suited: Vec<(usize, f32)> =
                    self.profile_tracks(j).into_iter().filter(|&i| (self.suits)(i)).map(|i| (i, self.model.energy_at(i).unwrap_or(0.0))).collect();
                // The steady middle of the profile's energy, split by the cards.
                let mut e: Vec<f32> = suited.iter().map(|x| x.1).collect();
                e.sort_by(f32::total_cmp);
                if e.len() >= 2 * MIN_SONGS * usize::from(cards) {
                    let band = if cards == 1 { BG_BAND } else { BG_CARDS_BAND };
                    let at = |q: f32| e[((e.len() - 1) as f32 * q) as usize];
                    let (lo, hi) = (at(band.0), at(band.1));
                    suited.retain(|x| (lo..=hi).contains(&x.1));
                }
                let tracks: Vec<usize> = background_cards(&suited, usize::from(cards)).get(usize::from(card))?.clone();
                Pool { tracks, len: BG_LEN, max_ms: BG_MS, unheard: BG_UNHEARD_SHARE, quota: None, blocks: None, arc: Arc::Focus, cross: false, stale: STALE_SHARE }
            }
            MixKey::Blend(keys) => {
                let sides: Vec<usize> = keys.iter().filter(|k| **k != 0).filter_map(|k| self.profile_index(*k)).collect();
                if sides.len() < 2 {
                    return None;
                }
                // Each side's third nearest the others' sound (the bridges).
                let mut tracks = Vec::new();
                for &j in &sides {
                    let others: Vec<&crate::analysis::Vector> =
                        sides.iter().filter(|&&o| o != j).map(|&o| &self.model.profiles.list[o].sound).collect();
                    let mut own = self.profile_tracks(j);
                    let d = |i: usize| self.model.embedded_at(i).map_or(f32::INFINITY, |e| others.iter().map(|o| euclid(e, *o)).sum::<f32>());
                    own.sort_by(|a, b| d(*a).total_cmp(&d(*b)).then(a.cmp(b)));
                    own.truncate((own.len() / 3).max(2 * MIN_SONGS));
                    tracks.extend(own);
                }
                let each = BLEND_LEN / sides.len();
                let quota = sides.iter().map(|&j| (j, each)).collect();
                Pool { tracks, len: each * sides.len(), quota: Some(quota), blocks: Some(2), cross: true, ..plain(Vec::new()) }
            }
        };
        (pool.tracks.len() >= MIN_SONGS).then_some(pool)
    }

    /// The loved songs the part of the day `part` hears more than the
    /// others, the liked ones before the rest: the best [`PART_POOL`] times
    /// a mix's length.
    fn part_tracks(&self, part: DayPart) -> Vec<usize> {
        let mut scored: Vec<(f32, usize)> = (0..self.value.len())
            .filter(|&i| self.pickable(i) && self.a[i] > 0.0)
            .map(|i| {
                let more = self.recs.taste.daypart_bonus_at(i, part);
                (self.a[i] * (1.0 + PART_BONUS * more) + if self.liked(i) { PART_LIKED } else { 0.0 }, i)
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.truncate(PART_POOL * MIX_LEN);
        scored.into_iter().map(|x| x.1).collect()
    }

    /// The mixes that are always there, before the lottery's: «Любимые»
    /// while the liked songs make a mix, and the mix of this part of the
    /// day once anything was played (unless it is «Любимые» again); each
    /// can be left out («Не предлагать этот микс»). Their songs are the
    /// epoch's, as any moment's (`kept`).
    pub fn pinned(&self, kept: &Kept) -> Vec<Mix> {
        let hidden = |k: &MixKey| self.settings.hidden.contains(k);
        let mut out: Vec<Mix> = Vec::new();
        if !hidden(&MixKey::Liked)
            && let Some(m) = self.mix(MixKey::Liked, kept)
        {
            out.push(m);
        }
        let part = MixKey::Part(self.part.index() as u8);
        if self.history
            && !hidden(&part)
            && let Some(m) = self.mix(part, kept)
        {
            let liked: HashSet<usize> = out.iter().flat_map(|x| x.tracks.iter().copied()).collect();
            let shared = m.tracks.iter().filter(|i| liked.contains(i)).count();
            let least = out.first().map_or(0, |x| x.tracks.len().min(m.tracks.len()));
            if out.is_empty() || (shared as f32) < OVERLAP * least as f32 {
                out.push(m);
            }
        }
        out
    }

    /// The mixes of the day (see the module docs): the day's own as they
    /// were drawn (`kept`: its lineup, each mix's songs), else one for each
    /// cluster of what the user listens to now, the heaviest first; named
    /// by their places. And the lineup to keep.
    pub fn daily(&self, kept: &Kept) -> (Vec<Mix>, Vec<MixKey>) {
        let keys: Vec<MixKey> = if kept.lineup.is_empty() {
            (1..=self.daily_pools().len().min(DAILY_MIXES)).map(|n| MixKey::Daily(n as u8)).collect()
        } else {
            kept.lineup.clone()
        };
        let mut mixes: Vec<Mix> = keys.into_iter().filter_map(|k| self.mix(k, kept)).collect();
        for (k, m) in mixes.iter_mut().enumerate() {
            m.title = format!("Микс дня {}", k + 1);
        }
        let keys = mixes.iter().map(|m| m.key).collect();
        (mixes, keys)
    }

    /// The pools of the mixes of the day, the heaviest cluster first.
    fn daily_pools(&self) -> &[Vec<usize>] {
        self.daily.get_or_init(|| self.cluster_daily())
    }

    /// What the user listens to now in clusters, and the songs each draws
    /// from (see the module docs).
    fn cluster_daily(&self) -> Vec<Vec<usize>> {
        let n = self.value.len();
        // What is listened to now; with too little of it (a library just
        // brought in, a long silence) everything loved.
        let weigh = |now: bool| -> Vec<f32> {
            (0..n).map(|i| if self.pickable(i) && !(now && self.stale[i]) { self.a[i].max(0.0) } else { 0.0 }).collect()
        };
        let mut w = weigh(true);
        if w.iter().filter(|x| **x > 0.0).count() < 2 * MIN_SONGS {
            w = weigh(false);
        }
        // By taste profile (one group before there are profiles): the
        // songs listened to now, all its songs, the weight.
        struct Group {
            profile: Option<usize>,
            cores: Vec<usize>,
            pool: Vec<usize>,
            weight: f32,
            mixes: usize,
        }
        let mut by_profile: HashMap<Option<usize>, Group> = HashMap::new();
        for i in (0..n).filter(|&i| self.pickable(i)) {
            let profile = self.model.profile_at(i);
            let g = by_profile.entry(profile).or_insert_with(|| Group { profile, cores: Vec::new(), pool: Vec::new(), weight: 0.0, mixes: 0 });
            g.pool.push(i);
            if w[i] > 0.0 {
                g.cores.push(i);
                g.weight += w[i];
            }
        }
        let mut groups: Vec<Group> = by_profile.into_values().filter(|g| g.cores.len() >= DAILY_CORES && g.pool.len() >= MIN_SONGS).collect();
        groups.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(a.profile.cmp(&b.profile)));
        groups.truncate(DAILY_MIXES);
        // One mix for each profile, then the rest to the heaviest by what
        // each has for a mix more (D'Hondt), while its songs are enough.
        let room = |g: &Group| (g.cores.len() / DAILY_CORES_EACH).min(g.pool.len() / MIX_LEN).max(1);
        for g in &mut groups {
            g.mixes = 1;
        }
        for _ in groups.len()..DAILY_MIXES {
            let share = |g: &Group| g.weight / (g.mixes + 1) as f32;
            let next = (0..groups.len())
                .filter(|&k| groups[k].mixes < room(&groups[k]))
                .max_by(|&x, &y| share(&groups[x]).total_cmp(&share(&groups[y])).then(y.cmp(&x)));
            let Some(k) = next else { break };
            groups[k].mixes += 1;
        }
        let mut clusters: Vec<(f32, Vec<usize>)> = groups.iter().flat_map(|g| self.split(&g.cores, &g.pool, &w, g.mixes)).collect();
        clusters.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.first().cmp(&b.1.first())));
        clusters.truncate(DAILY_MIXES);
        clusters.into_iter().map(|c| c.1).collect()
    }

    /// `pool` in `k` clusters by the sound of its `cores` (weighed by `w`),
    /// in fewer when one would have too few of them or too few songs: the
    /// weight of each cluster and its songs. A song without a sound goes
    /// where most of its artist is, else by its place.
    fn split(&self, cores: &[usize], pool: &[usize], w: &[f32], k: usize) -> Vec<(f32, Vec<usize>)> {
        let sounded: Vec<(&Vector, f32)> = cores.iter().filter_map(|&i| self.model.embedded_at(i).map(|e| (e, w[i]))).collect();
        for k in (2..=k).rev() {
            if sounded.len() < k * DAILY_CORES {
                continue;
            }
            let centres = kmeans(&sounded, k);
            if centres.len() < k {
                continue;
            }
            let nearest = |e: &Vector| (0..k).min_by(|&a, &b| euclid(e, &centres[a]).total_cmp(&euclid(e, &centres[b])).then(a.cmp(&b))).unwrap_or(0);
            // Each cluster's weight, songs and count of cores.
            let mut out: Vec<(f32, Vec<usize>, usize)> = vec![(0.0, Vec::new(), 0); k];
            let mut of_artist: HashMap<(ArtistId, usize), f32> = HashMap::new();
            let mut unsounded: Vec<usize> = Vec::new();
            for &i in pool {
                let Some(e) = self.model.embedded_at(i) else {
                    unsounded.push(i);
                    continue;
                };
                let c = nearest(e);
                out[c].1.push(i);
                if w[i] > 0.0 {
                    out[c].0 += w[i];
                    out[c].2 += 1;
                    if let Some(a) = self.model.main_artist_at(i) {
                        *of_artist.entry((a, c)).or_default() += w[i];
                    }
                }
            }
            for i in unsounded {
                let most = self.model.main_artist_at(i).and_then(|a| {
                    let weight = |c: usize| of_artist.get(&(a, c)).copied().unwrap_or(0.0);
                    (0..k).filter(|&c| weight(c) > 0.0).max_by(|&x, &y| weight(x).total_cmp(&weight(y)).then(y.cmp(&x)))
                });
                let c = most.unwrap_or(i % k);
                out[c].1.push(i);
                if w[i] > 0.0 {
                    out[c].0 += w[i];
                    out[c].2 += 1;
                }
            }
            if out.iter().all(|c| c.2 >= DAILY_CORES && c.1.len() >= MIN_SONGS) {
                return out
                    .into_iter()
                    .map(|(weight, mut songs, _)| {
                        songs.sort_unstable();
                        (weight, songs)
                    })
                    .collect();
            }
        }
        vec![(cores.iter().map(|&i| w[i]).sum(), pool.to_vec())]
    }

    /// The calm (or the lively) side of what is loved, by the energy of
    /// the loved songs with a sound (all with a sound before anything is
    /// loved); a side of one language keeps to it.
    fn side(&self, calm: bool) -> Vec<usize> {
        let with_sound: Vec<usize> = (0..self.value.len()).filter(|&i| self.pickable(i) && self.model.embedded_at(i).is_some()).collect();
        let loved: Vec<usize> = with_sound.iter().copied().filter(|&i| self.a[i] > 0.0).collect();
        let base = if loved.len() >= 2 * MIN_SONGS { loved } else { with_sound };
        let mut e: Vec<f32> = base.iter().filter_map(|&i| self.model.energy_at(i)).collect();
        if e.len() < MIN_SONGS {
            return Vec::new();
        }
        e.sort_by(f32::total_cmp);
        let at = |q: f32| e[((e.len() - 1) as f32 * q) as usize];
        let (lo, hi) = (at(SIDE), at(1.0 - SIDE));
        let mut side: Vec<usize> =
            base.into_iter().filter(|&i| self.model.energy_at(i).is_some_and(|x| if calm { x <= lo } else { x >= hi })).collect();
        let mut langs: HashMap<Lang, usize> = HashMap::new();
        for &i in &side {
            if let LaneKey::Sung(l) = LaneKey::of(self.model.song_type_at(i)) {
                *langs.entry(l).or_default() += 1;
            }
        }
        if let Some((&l, &n)) = langs.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
            && n as f32 >= LANG_HOLD * side.len() as f32
        {
            side.retain(|&i| {
                let t = self.model.song_type_at(i);
                match LaneKey::of(t) {
                    LaneKey::Sung(x) => x == l,
                    LaneKey::Unknown => !t.ost,
                    LaneKey::Instrumental => false,
                }
            });
        }
        side
    }

    /// The songs of `key` from `pool` (see the module docs): the core drawn
    /// by value, the rest by the epoch's chance; `before` the songs it had
    /// before «Другой состав» (left out of the rotation, as the epoch
    /// before's).
    fn compose(&self, key: MixKey, pool: &Pool, salt: u64, kept: &Kept, before: &[TrackId]) -> Vec<usize> {
        let seed = self.epoch.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ key.token() ^ salt.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let chance = |i: usize| Rng::derive(seed, i as u64).f32();
        let mut away: HashSet<usize> = kept.before(key).iter().filter_map(|t| self.model.position(*t)).collect();
        if salt != 0 {
            away.extend(before.iter().filter_map(|t| self.model.position(*t)));
        }
        // What is never played but near the loved: the loved of now, or of
        // any time when nothing of the pool is heard now.
        let now_loved = pool.tracks.iter().any(|&i| self.love[i] >= 0.5 && !self.stale[i]);
        let is_loved = |i: usize| self.love[i] >= 0.5 && !(now_loved && self.stale[i]);
        let loved: Vec<&crate::analysis::Vector> = pool.tracks.iter().filter(|&&i| is_loved(i)).filter_map(|&i| self.model.embedded_at(i)).collect();
        let scale = self.model.scale().max(1e-3);
        // …or beside the loved in the graphs (docs/recs-graph.md).
        let mut linked: HashMap<usize, f32> = HashMap::new();
        for &l in pool.tracks.iter().filter(|&&i| is_loved(i)) {
            for &(c, w) in self.model.graph.near(l).iter().chain(self.model.personal_near(l)) {
                let e = linked.entry(c as usize).or_default();
                *e = e.max(w);
            }
        }
        let value = |i: usize| {
            let mut v = self.value[i].unwrap_or(f32::NEG_INFINITY);
            if self.unheard(i) {
                let sound = self.model.embedded_at(i).map_or(0.0, |e| {
                    let near = loved.iter().map(|l| euclid(e, *l)).fold(f32::INFINITY, f32::min);
                    (1.0 - near / scale).max(0.0)
                });
                let near = sound.max(linked.get(&i).copied().unwrap_or(0.0));
                v += W_EXPLORE * near;
                // A mix of the day brings its share of what fits and was
                // never played.
                if near > 0.0 && matches!(key, MixKey::Daily(_)) {
                    v += W_DAILY_NEW;
                }
            }
            v
        };
        let n = pool.len;
        let caps = Caps {
            artist: n.div_ceil(3).max(1),
            // Two in ten would allow six of 28, too tight a weave for an order.
            album: (n / 7).max(2),
            // One world (a franchise) is a quarter at most, also of a pool
            // of it alone: that makes no mix.
            world: ((n as f32 * WORLD_SHARE).ceil() as usize).max(2),
            unheard: if self.history { ((n as f32 * pool.unheard).ceil() as usize).max(1) } else { usize::MAX },
            // Old songs: their share, or what the songs heard now leave
            // empty.
            stale: {
                let heard_now = pool.tracks.iter().filter(|&&i| !self.stale[i] && !self.unheard(i)).count();
                ((n as f32 * pool.stale).ceil() as usize).max(n.saturating_sub(heard_now))
            },
        };
        let mut pick = Picking::new(self, caps, pool.quota.clone(), pool.max_ms);
        // The core: drawn from the best by value (Gumbel keys over a third
        // of their spread), the epoch before's less likely; then the rest of
        // the pool by value, for what the rules held back.
        let half = n.div_ceil(2);
        let mut core: Vec<(f32, usize)> = pool.tracks.iter().map(|&i| (value(i), i)).collect();
        core.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let k = (CORE_POOL * half).min(core.len());
        let mut tau = 1.0;
        if k > 0 {
            tau = ((core[0].0 - core[k - 1].0) / CORE_SPREAD).max(0.05);
            let gumbel = |i: usize| {
                let u = Rng::derive(seed ^ 0x636f_7265, i as u64).f64().clamp(1e-12, 1.0 - 1e-12);
                -(-u.ln()).ln() as f32
            };
            let mut top: Vec<(f32, usize)> =
                core[..k].iter().map(|&(v, i)| (v / tau + gumbel(i) - if away.contains(&i) { CORE_BEFORE } else { 0.0 }, i)).collect();
            top.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            core.splice(..k, top);
        }
        // A mix of one kind keeps to the scene its first songs are of: what
        // the graph links to the songs taken stands higher
        // (docs/recs-graph.md); without a graph nothing changes.
        let cohere = matches!(key, MixKey::Profile(_) | MixKey::Kind(_) | MixKey::Daily(_)) && !self.model.graph.is_empty();
        if cohere {
            let mut top: Vec<(f32, usize)> = core[..k].to_vec();
            self.take_cohering(&mut pick, &mut top, 1.0 / tau, &|p: &Picking<'_, '_>| p.len() >= half || p.full(n), &|_| false);
        }
        for &(_, i) in &core[if cohere { k } else { 0 }..] {
            if pick.len() >= half || pick.full(n) {
                break;
            }
            pick.take(i);
        }
        // The rest by the epoch's chance, the epoch before's left out while
        // others last.
        let mut rest: Vec<(f32, usize)> = pool.tracks.iter().map(|&i| (value(i) + ROTATE * chance(i), i)).collect();
        rest.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        if cohere {
            let mut left = rest.clone();
            self.take_cohering(&mut pick, &mut left, 1.0, &|p: &Picking<'_, '_>| p.full(n), &|i| away.contains(&i));
        }
        for strict in [true, false] {
            for &(_, i) in &rest {
                if pick.full(n) {
                    break;
                }
                if strict && away.contains(&i) {
                    continue;
                }
                pick.take(i);
            }
        }
        // Too few by the quotas (a small profile): the others fill in.
        if pick.quota.is_some() && !pick.full(n) {
            pick.quota = None;
            for &(_, i) in &rest {
                if pick.full(n) {
                    break;
                }
                pick.take(i);
            }
        }
        // Still short (the rules held back what is heard now): old songs
        // fill in rather than leave the mix short.
        if !pick.full(n) && pick.caps.stale != usize::MAX {
            pick.caps.stale = usize::MAX;
            for &(_, i) in &rest {
                if pick.full(n) {
                    break;
                }
                pick.take(i);
            }
        }
        // Short of its length (the rules held it back), a world and an
        // artist keep their share of what is there: the least worth of
        // whichever is over goes, until none is.
        let mut out = pick.out;
        loop {
            let world_cap = ((out.len() as f32 * WORLD_SHARE).ceil() as usize).max(2);
            let artist_cap = out.len().div_ceil(3).max(1);
            let mut per_world: HashMap<u64, usize> = HashMap::new();
            let mut per_artist: HashMap<ArtistId, usize> = HashMap::new();
            for &i in &out {
                let t = self.recs.info(i);
                if let Some(w) = t.world {
                    *per_world.entry(w).or_default() += 1;
                }
                if let Some(a) = t.artists.first() {
                    *per_artist.entry(*a).or_default() += 1;
                }
            }
            let over = |i: usize| {
                let t = self.recs.info(i);
                t.world.is_some_and(|w| per_world[&w] > world_cap) || t.artists.first().is_some_and(|a| per_artist[a] > artist_cap)
            };
            let Some(k) = (0..out.len()).filter(|&k| over(out[k])).min_by(|a, b| value(out[*a]).total_cmp(&value(out[*b])).then(a.cmp(b))) else {
                break;
            };
            out.remove(k);
        }
        out
    }

    /// Takes from `list` (keys, the best first) while `done` says no, each
    /// time the best by its key plus [`W_COHERE`] (× `unit`) of how much the
    /// graph links it to what was taken, `skip` left for later.
    fn take_cohering(
        &self,
        pick: &mut Picking<'_, '_>,
        list: &mut Vec<(f32, usize)>,
        unit: f32,
        done: &dyn Fn(&Picking<'_, '_>) -> bool,
        skip: &dyn Fn(usize) -> bool,
    ) {
        let g = &self.model.graph;
        let mut link: HashMap<usize, f32> = HashMap::new();
        for &i in &pick.out {
            self.linked(&mut link, i, g);
        }
        while !done(pick) {
            let best = list
                .iter()
                .enumerate()
                .filter(|(_, (_, i))| !skip(*i))
                .map(|(k, (key, i))| (k, key + unit * W_COHERE * link.get(i).copied().unwrap_or(0.0).tanh()))
                .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
            let Some((k, _)) = best else { break };
            let (_, i) = list.remove(k);
            let before = pick.len();
            pick.take(i);
            if pick.len() > before {
                self.linked(&mut link, i, g);
            }
        }
    }

    /// Adds what the graph links to the track at `i` to `link`.
    fn linked(&self, link: &mut HashMap<usize, f32>, i: usize, g: &crate::graph::SimGraph) {
        for &(c, w) in g.near(i) {
            *link.entry(c as usize).or_default() += w;
        }
        if let Some(a) = self.model.main_artist_at(i) {
            for &(b, w) in g.similar_artists(a) {
                for &j in self.model.of_artist(b) {
                    *link.entry(j as usize).or_default() += 0.5 * w;
                }
            }
        }
    }

    /// Familiar and loved: above the lowest quarter of the mix's familiar
    /// ones.
    fn anchors(&self, tracks: &[usize]) -> Vec<bool> {
        let mut a: Vec<f32> = tracks.iter().filter(|&&i| !self.unheard(i)).map(|&i| self.a[i]).collect();
        a.sort_by(f32::total_cmp);
        let low = a.get(a.len() / 4).copied().unwrap_or(0.0);
        tracks.iter().map(|&i| !self.unheard(i) && self.a[i] >= low && self.a[i] > 0.0).collect()
    }

    /// The energy percentile of each of `tracks` among them (ties
    /// averaged, a track without a sound in the middle).
    fn percentiles(&self, tracks: &[usize]) -> HashMap<usize, f32> {
        let raw: Vec<Option<f32>> = tracks.iter().map(|&i| self.model.energy_at(i)).collect();
        let known: Vec<f32> = raw.iter().flatten().copied().collect();
        let ranks = crate::listening::pct_rank(&known);
        let mut k = 0;
        let mut out = HashMap::new();
        for (t, r) in tracks.iter().zip(&raw) {
            let p = match r {
                Some(_) => {
                    k += 1;
                    ranks[k - 1]
                }
                None => 0.5,
            };
            out.insert(*t, p);
        }
        out
    }

    fn slot(&self, i: usize) -> Slot {
        self.recs.slot(i, Reason::Favourite, self.value[i].unwrap_or(0.0))
    }

    fn distance(&self, a: usize, b: usize, m: f32) -> f32 {
        match (self.model.embedded_at(a), self.model.embedded_at(b)) {
            (Some(x), Some(y)) => euclid(x, y) / m,
            _ => {
                let (x, y) = (self.recs.info(a), self.recs.info(b));
                if x.release.is_some() && x.release == y.release {
                    0.5
                } else if x.artists.iter().any(|z| y.artists.contains(z)) {
                    0.6
                } else {
                    1.5
                }
            }
        }
    }

    /// The typical distance between `tracks` (1 without sounds).
    fn typical(&self, tracks: &[usize]) -> f32 {
        let vs: Vec<&crate::analysis::Vector> = tracks.iter().filter_map(|&i| self.model.embedded_at(i)).collect();
        let half = vs.len() / 2;
        let mut d: Vec<f32> = (0..half).map(|k| euclid(vs[k], vs[k + half])).collect();
        d.sort_by(f32::total_cmp);
        d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(1.0)
    }

    /// `tracks` in the order they play (see the module docs): on `arc`,
    /// `steady` steps («Фон»), the language before kept in a mix of many
    /// (`cross`).
    pub fn sequence(&self, tracks: &[usize], arc: Arc, steady: bool, cross: bool) -> Vec<usize> {
        let (order, m) = self.path(tracks, arc, steady, cross, None);
        self.mended(order, m, None, true)
    }

    /// `tracks` of `pool` in the order they play on `arc`: one by one, or
    /// in blocks. With `settle` a song never played that finds no place
    /// between loved ones leaves; without, it stays where it breaks least.
    fn order(&self, tracks: &[usize], pool: &Pool, arc: Arc, steady: bool, settle: bool) -> Vec<usize> {
        match pool.blocks {
            Some(size) => {
                let (order, m) = self.path(tracks, arc, false, true, Some(size));
                self.mended(order, m, Some(if size > 0 { size + 1 } else { 4 }), settle)
            }
            None => {
                let (order, m) = self.path(tracks, arc, steady, pool.cross, None);
                self.mended(order, m, None, settle)
            }
        }
    }

    /// [`Builder::sequence`] before it is mended, and the typical distance.
    /// With `blocks` (`Some(size)`, 0: three or four by the profile's
    /// count) the songs of a profile come in blocks of that many.
    fn path(&self, tracks: &[usize], arc: Arc, steady: bool, cross: bool, blocks: Option<usize>) -> (Vec<Slot>, f32) {
        if tracks.len() < 3 {
            return (tracks.iter().map(|&i| self.slot(i)).collect(), 1.0);
        }
        let e = self.percentiles(tracks);
        let anchors: HashMap<usize, bool> = tracks.iter().copied().zip(self.anchors(tracks)).collect();
        let m = self.typical(tracks);
        let long = |i: usize| self.recs.info(i).duration_ms.is_some_and(|d| d > EDGE_MS);
        let mut left: Vec<usize> = tracks.to_vec();
        let total = tracks.len();
        let targets = arc.targets(total);
        let want = |k: usize| targets.as_ref().map_or(0.4, |t| t[k.min(total - 1)]);
        // The last: calmer than the middle, often heard out, familiar.
        let end = want(total - 1).min(0.45);
        let closer = left
            .iter()
            .copied()
            .filter(|&i| !self.unheard(i) && !long(i))
            .min_by(|&a, &b| {
                let c = |i: usize| {
                    let s = &self.model.tracks[i];
                    4.0 * (e[&i] - end).powi(2) - s.completes as f32 / s.starts.max(1) as f32
                };
                c(a).total_cmp(&c(b)).then(a.cmp(&b))
            });
        if let Some(c) = closer {
            left.retain(|&i| i != c);
        }
        // The first: loved, familiar, near the arc's start.
        let first = left
            .iter()
            .copied()
            .filter(|&i| !self.unheard(i) && !long(i))
            .min_by(|&a, &b| {
                let c = |i: usize| (e[&i] - want(0)).abs() - 0.5 * self.a[i];
                c(a).total_cmp(&c(b)).then(a.cmp(&b))
            })
            .unwrap_or(left[0]);
        left.retain(|&i| i != first);
        // The blocks: each profile's length of one and what is left of it.
        let mut count: HashMap<Option<usize>, usize> = HashMap::new();
        for &i in tracks {
            *count.entry(self.model.profile_at(i)).or_default() += 1;
        }
        let size = |p: Option<usize>| match blocks {
            Some(n) if n > 0 => n,
            _ if count.get(&p).copied().unwrap_or(0) >= 8 => 4,
            _ => 3,
        };
        let most = blocks.map(|n| if n > 0 { n + 1 } else { 4 });
        let mut rest = count.clone();
        *rest.entry(self.model.profile_at(first)).or_default() -= 1;
        if let Some(c) = closer {
            *rest.entry(self.model.profile_at(c)).or_default() -= 1;
        }
        let mut run = 1;
        let rules = Diversity { world_share: WORLD_SHARE, ..Diversity::default() };
        let mut out: Vec<Slot> = vec![self.slot(first)];
        let mut order: Vec<usize> = vec![first];
        while !left.is_empty() {
            let k = order.len();
            let seq: Vec<Seen<'_>> = out.iter().map(|s| (s.artists.as_slice(), s.album, s.world)).collect();
            let prev = order[k - 1];
            let prev2 = k.checked_sub(2).map(|p| order[p]);
            let room = left.len() - 1 + usize::from(closer.is_some());
            let crowd = Crowd::of(self, &left, room, &rules);
            let mut best: Option<((usize, f32), usize)> = None;
            for (n, &c) in left.iter().enumerate() {
                let s = self.slot(c);
                let (artist, album, world) = rules.broken(&seq, (&s.artists, s.album, s.world));
                let mut v = 2 * artist + 4 * usize::from(world);
                // The last two places before the closer: the closer too.
                if let Some(cl) = closer
                    && left.len() <= 2
                {
                    let rest: Vec<Slot> = left.iter().filter(|&&o| o != c).map(|&o| self.slot(o)).collect();
                    let mut then = seq.clone();
                    then.push((&s.artists, s.album, s.world));
                    then.extend(rest.iter().map(|r| (r.artists.as_slice(), r.album, r.world)));
                    let last = self.slot(cl);
                    let (artist, _, world) = rules.broken(&then, (&last.artists, last.album, last.world));
                    v += 2 * artist + 4 * usize::from(world);
                }
                if self.unheard(c) && (k < 2 || self.unheard(prev) || !anchors[&prev]) {
                    v += W_UNHEARD;
                }
                if self.unheard(prev) && !anchors[&c] {
                    v += W_UNHEARD;
                }
                let world = self.recs.info(c).world;
                let mut cost = self.distance(prev, c, m) * if steady { C_STEADY } else { 1.0 }
                    + if targets.is_some() { C_ARC * (e[&c] - want(k)).abs() } else { 0.0 }
                    + if album { C_ALBUM } else { 0.0 }
                    + C_CROWD * crowd.over(&s) as f32;
                if world.is_some() && world == self.recs.info(prev).world {
                    cost += C_WORLD;
                }
                if world.is_some() && prev2.is_some_and(|p| self.recs.info(p).world == world) {
                    cost += C_WORLD2;
                }
                if cross && self.model.song_type_at(c).lane() != self.model.song_type_at(prev).lane() {
                    cost += C_LANE;
                }
                if let Some(most) = most {
                    // Blocks are mended inside a profile only: an album thrice
                    // is better not made at all.
                    v += usize::from(album);
                    let (p, was) = (self.model.profile_at(c), self.model.profile_at(prev));
                    if p == was {
                        if run >= most {
                            v += W_BLOCK;
                        } else if run >= size(was) {
                            cost += C_BLOCK;
                        }
                    } else if run < size(was) && rest.get(&was).is_some_and(|n| *n > 0) {
                        cost += C_BLOCK;
                    }
                }
                if best.as_ref().is_none_or(|(b, _)| v < b.0 || (v == b.0 && cost < b.1)) {
                    best = Some(((v, cost), n));
                }
            }
            let (_, n) = best.expect("left is not empty");
            let c = left.swap_remove(n);
            run = if self.model.profile_at(c) == self.model.profile_at(prev) { run + 1 } else { 1 };
            if let Some(r) = rest.get_mut(&self.model.profile_at(c)) {
                *r = r.saturating_sub(1);
            }
            out.push(self.slot(c));
            order.push(c);
        }
        if let Some(c) = closer {
            out.push(self.slot(c));
        }
        (out, m)
    }

    /// The order of `slots` mended by [`repair`], then each song never
    /// played put in its place ([`Builder::sandwich`]), as catalog
    /// positions.
    fn mended(&self, mut out: Vec<Slot>, m: f32, blocks: Option<usize>, settle: bool) -> Vec<usize> {
        let rules = Diversity { world_share: WORLD_SHARE, ..Diversity::default() };
        if blocks.is_some() {
            // Inside one profile's songs only: the blocks stay.
            let profile = |s: &Slot| self.position(s).and_then(|i| self.model.profile_at(i));
            repair_within(self.recs, &mut out, &[], None, &rules, m, &|a, b| profile(a) == profile(b));
            // What the blocks could not mend (a world thrice, an artist
            // twice in three, an album thrice in ten): the rule weighs more
            // than a block, any swap may.
            let seq: Vec<Seen<'_>> = out.iter().map(|s| (s.artists.as_slice(), s.album, s.world)).collect();
            let stuck = (0..seq.len()).any(|k| {
                let (artist, album, world) = rules.broken(&seq[..k], seq[k]);
                world || album || artist > 0
            });
            if let Some(most) = blocks
                && stuck
            {
                // Taken only while no block grows past its length.
                let mut freer = out.clone();
                repair(self.recs, &mut freer, &[], None, &rules, m);
                let profiles: Vec<Option<usize>> = freer.iter().map(profile).collect();
                let longest = profiles.chunk_by(|a, b| a == b).map(<[_]>::len).max().unwrap_or(0);
                if longest <= most {
                    out = freer;
                }
            }
        } else {
            repair(self.recs, &mut out, &[], None, &rules, m);
        }
        let order: Vec<usize> = out.iter().filter_map(|s| self.position(s)).collect();
        self.sandwich(order, &rules, blocks, m, settle)
    }

    fn position(&self, s: &Slot) -> Option<usize> {
        match s.item {
            crate::pick::Item::Track(id) => self.recs.catalog.position(id),
            crate::pick::Item::Stream(_) => None,
        }
    }

    /// Every song never played between two loved familiar ones and not in
    /// the first two places: each in turn is taken out (the others staying)
    /// and put back in the gap between two of them nearest to where it
    /// stood that breaks no rule of diversity more than it did (of
    /// `blocks`, next to its own profile's, no block past its length); with
    /// no such gap it leaves the mix (`settle`; else it stands where it
    /// breaks least), and what its leaving broke is mended by swaps of
    /// familiar songs that keep the gaps.
    fn sandwich(&self, order: Vec<usize>, rules: &Diversity, blocks: Option<usize>, m: f32, settle: bool) -> Vec<usize> {
        let anchors: HashMap<usize, bool> = order.iter().copied().zip(self.anchors(&order)).collect();
        let fits = |o: &[usize], k: usize| k >= 2 && k + 1 < o.len() && anchors[&o[k - 1]] && anchors[&o[k + 1]];
        if (0..order.len()).all(|k| !self.unheard(order[k]) || fits(&order, k)) {
            return order;
        }
        let weight = |o: &[usize]| {
            let slots: Vec<Slot> = o.iter().map(|&i| self.slot(i)).collect();
            let seq: Vec<Seen<'_>> = slots.iter().map(|s| (s.artists.as_slice(), s.album, s.world)).collect();
            rules.weight_of(&seq, 0)
        };
        let todo: Vec<usize> = order.iter().copied().filter(|&i| self.unheard(i)).collect();
        let mut out = order;
        let mut dropped = false;
        for u in todo {
            let Some(was) = out.iter().position(|&i| i == u) else { continue };
            if fits(&out, was) {
                continue;
            }
            let base = weight(&out);
            out.remove(was);
            let mut best: Option<(usize, usize, usize)> = None;
            let own = self.model.profile_at(u);
            for g in 2..out.len() {
                if self.unheard(out[g - 1]) || self.unheard(out[g]) || !(anchors[&out[g - 1]] && anchors[&out[g]]) {
                    continue;
                }
                // In a mix of blocks, inside a block of its own profile.
                if blocks.is_some() && self.model.profile_at(out[g - 1]) != own && self.model.profile_at(out[g]) != own {
                    continue;
                }
                out.insert(g, u);
                // Nor makes the block longer than a block may be.
                let long = blocks.is_some_and(|most| {
                    let same = |k: &usize| self.model.profile_at(out[*k]) == own;
                    1 + (0..g).rev().take_while(same).count() + (g + 1..out.len()).take_while(same).count() > most
                });
                let w = weight(&out);
                out.remove(g);
                if long || w > base {
                    continue;
                }
                let key = (w, g.abs_diff(was));
                if best.is_none_or(|b| key < (b.0, b.1)) {
                    best = Some((w, g.abs_diff(was), g));
                }
            }
            // The day's songs stay: between loved ones past a block's
            // length, else where it breaks least, nearest its place.
            if best.is_none() && !settle {
                for g in 2..out.len() {
                    if self.unheard(out[g - 1]) || self.unheard(out[g]) || !(anchors[&out[g - 1]] && anchors[&out[g]]) {
                        continue;
                    }
                    out.insert(g, u);
                    let w = weight(&out);
                    out.remove(g);
                    if w <= base && best.is_none_or(|b| g.abs_diff(was) < b.1) {
                        best = Some((w, g.abs_diff(was), g));
                    }
                }
            }
            if best.is_none() && !settle {
                for g in 2..=out.len() {
                    out.insert(g, u);
                    let key = (weight(&out), g.abs_diff(was));
                    out.remove(g);
                    if best.is_none_or(|b| key < (b.0, b.1)) {
                        best = Some((key.0, key.1, g));
                    }
                }
            }
            match best {
                Some((_, _, g)) => out.insert(g, u),
                None => dropped = true,
            }
        }
        if dropped && weight(&out) > 0 {
            let mut slots: Vec<Slot> = out.iter().map(|&i| self.slot(i)).collect();
            let of = |s: &Slot| self.position(s);
            let keeps = |a: &Slot, b: &Slot| match (of(a), of(b)) {
                (Some(x), Some(y)) => {
                    !self.unheard(x) && !self.unheard(y) && anchors[&x] == anchors[&y] && (blocks.is_none() || self.model.profile_at(x) == self.model.profile_at(y))
                }
                _ => false,
            };
            repair_within(self.recs, &mut slots, &[], None, rules, m, &keeps);
            out = slots.iter().filter_map(|s| self.position(s)).collect();
        }
        out
    }

    /// `tracks` of many profiles in blocks of one profile (`size` songs; 0:
    /// four of a profile with eight or more, else three), the blocks
    /// joined (see the module docs).
    pub fn sequence_blocks(&self, tracks: &[usize], size: usize, arc: Arc) -> Vec<usize> {
        let (order, m) = self.path(tracks, arc, false, true, Some(size));
        self.mended(order, m, Some(if size > 0 { size + 1 } else { 4 }), true)
    }

    /// The main artists of `tracks`, by how many songs they have there
    /// (then by their first place).
    fn artists(&self, tracks: &[usize]) -> Vec<ArtistId> {
        let mut count: HashMap<ArtistId, (usize, usize)> = HashMap::new();
        for (k, &i) in tracks.iter().enumerate() {
            if let Some(a) = self.recs.info(i).artists.first() {
                count.entry(*a).or_insert((0, k)).0 += 1;
            }
        }
        let mut ranked: Vec<(ArtistId, (usize, usize))> = count.into_iter().collect();
        ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
        ranked.into_iter().map(|x| x.0).take(12).collect()
    }

    /// What the mix of `key` with `tracks` is called (see the module docs);
    /// `artist` names an artist.
    pub fn title(&self, key: MixKey, tracks: &[usize]) -> String {
        let name = |k: u32| -> Option<String> {
            let j = self.profile_index(k)?;
            Some(self.model.profiles.name(j, self.settings.names, &|_| None).0)
        };
        match key {
            MixKey::Profile(k) => name(k).unwrap_or_default(),
            MixKey::Everything => "Всё вперемешку".into(),
            MixKey::Calm | MixKey::Lively => side_title(key == MixKey::Calm, tracks.iter().map(|&i| LaneKey::of(self.model.song_type_at(i)))),
            MixKey::Background { card, cards, .. } => match (cards, card) {
                (2, 0) | (3, 0) => "Фон потише".into(),
                (2, _) | (3, 2) => "Фон поживее".into(),
                _ => "Фон".into(),
            },
            MixKey::Blend(keys) => {
                let names: Vec<String> = keys.iter().filter(|k| **k != 0).filter_map(|k| name(*k)).collect();
                if self.settings.names == NameStyle::Kind { blend_title(names) } else { names.join(" + ") }
            }
            MixKey::Kind(c) => kind_title(lane_of_code(c)),
            MixKey::Liked => "Любимые".into(),
            MixKey::Part(p) => part_title(p).into(),
            MixKey::Daily(n) => format!("Микс дня {n}"),
        }
    }

    /// Shorter forms of a blend's name by its profiles' kinds (see
    /// [`blend_short`]); none for the others, or by the artists.
    pub fn short_titles(&self, key: MixKey) -> Vec<String> {
        let MixKey::Blend(keys) = key else { return Vec::new() };
        if self.settings.names != NameStyle::Kind {
            return Vec::new();
        }
        let kinds: Option<Vec<Kind>> = keys.iter().filter(|k| **k != 0).map(|k| Some(self.model.profiles.list[self.profile_index(*k)?].kind)).collect();
        kinds.map_or_else(Vec::new, |k| blend_short(&k))
    }

    /// The names that need the artists' (`artist` names one): a profile's
    /// by its artists, a blend's of them; then two cards of one name tell
    /// themselves apart by their main artist.
    pub fn named(&self, mixes: &mut [Mix], artist: &dyn Fn(ArtistId) -> Option<String>) {
        let name = |k: u32| -> Option<String> {
            let j = self.profile_index(k)?;
            Some(self.model.profiles.name(j, self.settings.names, artist).0)
        };
        for m in mixes.iter_mut() {
            match m.key {
                MixKey::Profile(k) => m.title = name(k).unwrap_or_default(),
                MixKey::Blend(keys) => {
                    let parts: Vec<String> = keys
                        .iter()
                        .filter(|k| **k != 0)
                        .filter_map(|k| match self.settings.names {
                            NameStyle::Kind => name(*k),
                            NameStyle::Artists => {
                                let j = self.profile_index(*k)?;
                                self.model.profiles.list[j].stats.artists.first().and_then(|a| artist(*a))
                            }
                        })
                        .collect();
                    m.title = if self.settings.names == NameStyle::Kind { blend_title(parts) } else { parts.join(" + ") };
                }
                _ => {}
            }
        }
        for k in 1..mixes.len() {
            if mixes[..k].iter().any(|m| m.title == mixes[k].title) {
                let who = mixes[k].artists.iter().filter_map(|a| artist(*a)).find(|n| !mixes[..k].iter().any(|m| m.title.ends_with(n.as_str())));
                if let Some(who) = who {
                    mixes[k].title = format!("{} · {who}", mixes[k].title);
                }
            }
        }
    }

    /// How well `mix` fits now (see the module docs).
    pub fn context(&self, mix: &Mix) -> f32 {
        let part = self.part.index();
        let (mut here, mut all) = (0u32, 0u32);
        for &i in &mix.tracks {
            let p = &self.model.tracks[i].part;
            here += u32::from(p[part]);
            all += p.iter().map(|x| u32::from(*x)).sum::<u32>();
        }
        let expect = all as f32 * self.model.part_share(part);
        let mut s = ((here as f32 + 1.0) / (expect + 1.0)).ln();
        let worlds: HashSet<u64> = mix.tracks.iter().filter_map(|&i| self.recs.info(i).world).collect();
        if !worlds.is_empty() {
            let now = self.model.now();
            let hot: f32 = worlds.iter().filter_map(|w| self.model.worlds.get(w)).map(|w| w.hot(now)).sum::<f32>() / worlds.len() as f32;
            s += 0.8 * hot.ln_1p();
        }
        if self.live.profile.is_some_and(|p| mix.profiles.contains(&p)) {
            s += 0.5;
        }
        if matches!(mix.key, MixKey::Background { .. }) {
            s += 0.8 * self.live.p_bg;
        }
        // Before anything is heard: at night the calm and the wordless first.
        if !self.history && self.part == DayPart::Night {
            let quiet = mix.key == MixKey::Calm || mix.key == MixKey::Kind(lane_code(LaneKey::Instrumental));
            s += if quiet { 0.5 } else { 0.0 };
        }
        s
    }
}

/// Rounds of the k-means of the mixes of the day at most.
const KMEANS_ROUNDS: usize = 12;

/// `k` centres of `pts` (each with its weight) by a weighted k-means that
/// needs no chance: it starts from the heaviest point, then each time from
/// the point farthest (by the root of its weight) from the centres there
/// are. Fewer than `k` when the points are fewer.
fn kmeans(pts: &[(&Vector, f32)], k: usize) -> Vec<Vector> {
    let mut centres: Vec<Vector> = Vec::with_capacity(k);
    let Some(first) = (0..pts.len()).max_by(|&a, &b| pts[a].1.total_cmp(&pts[b].1).then(b.cmp(&a))) else {
        return centres;
    };
    centres.push(*pts[first].0);
    let mut far: Vec<f32> = pts.iter().map(|p| euclid(p.0, &centres[0])).collect();
    while centres.len() < k.min(pts.len()) {
        let reach = |i: usize| pts[i].1.max(0.0).sqrt() * far[i];
        let Some(next) = (0..pts.len()).max_by(|&a, &b| reach(a).total_cmp(&reach(b)).then(b.cmp(&a))) else {
            break;
        };
        let centre = *pts[next].0;
        for (d, p) in far.iter_mut().zip(pts) {
            *d = d.min(euclid(p.0, &centre));
        }
        centres.push(centre);
    }
    for _ in 0..KMEANS_ROUNDS {
        let mut sum = vec![[0.0f32; DIM]; centres.len()];
        let mut mass = vec![0.0f32; centres.len()];
        for &(e, weight) in pts {
            let c = (0..centres.len()).min_by(|&a, &b| euclid(e, &centres[a]).total_cmp(&euclid(e, &centres[b])).then(a.cmp(&b))).unwrap_or(0);
            for (s, x) in sum[c].iter_mut().zip(e.iter()) {
                *s += weight * x;
            }
            mass[c] += weight;
        }
        let mut moved = false;
        for c in 0..centres.len() {
            if mass[c] <= 0.0 {
                continue;
            }
            let next: Vector = std::array::from_fn(|d| sum[c][d] / mass[c]);
            moved |= euclid(&next, &centres[c]) > 1e-4;
            centres[c] = next;
        }
        if !moved {
            break;
        }
    }
    centres
}

/// The name of the mix of the part of the day `p` ([`DayPart::index`]).
pub fn part_title(p: u8) -> &'static str {
    match DayPart::ALL.get(usize::from(p)) {
        Some(DayPart::Morning) => "Утренний микс",
        Some(DayPart::Day) => "Дневной микс",
        Some(DayPart::Evening) => "Вечерний микс",
        _ => "Ночной микс",
    }
}

/// A blend's name: its profiles' names joined by «+», the songs of one word
/// said once («Русские и японские песни + Саундтреки без слов»).
fn blend_title(names: Vec<String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut kinds: Vec<(String, Vec<String>)> = Vec::new();
    for n in &names {
        // «Русские песни»: one word before the last.
        let words: Vec<&str> = n.split(' ').collect();
        if words.len() == 2 && n.chars().next().is_some_and(char::is_uppercase) {
            match kinds.iter_mut().find(|k| k.0 == words[1]) {
                Some(k) => k.1.push(words[0].to_owned()),
                None => kinds.push((words[1].to_owned(), vec![words[0].to_owned()])),
            }
        }
    }
    let mut said: Vec<&str> = Vec::new();
    for n in &names {
        let words: Vec<&str> = n.split(' ').collect();
        let kind = kinds.iter().find(|k| words.len() == 2 && k.0 == words[1] && k.1.len() > 1);
        match kind {
            Some((noun, adjectives)) if !said.contains(&noun.as_str()) => {
                said.push(noun);
                let mut first = adjectives[0].clone();
                let rest: Vec<String> = adjectives[1..].iter().map(|a| a.to_lowercase()).collect();
                first.push_str(&format!(" и {}", rest.join(" и ")));
                parts.push(format!("{first} {noun}"));
            }
            Some(_) => {}
            None => parts.push(n.clone()),
        }
    }
    parts.join(" + ")
}

/// Shorter forms of a blend's name, the longest first, by its profiles'
/// kinds: «Русские и японские песни + без слов», then «Русские и японские
/// + без слов». None when two of its profiles are of one kind (their names
/// tell them apart by more than the kind).
fn blend_short(kinds: &[Kind]) -> Vec<String> {
    let mut seen: Vec<Kind> = Vec::new();
    for k in kinds {
        if seen.contains(k) {
            return Vec::new();
        }
        seen.push(*k);
    }
    let sung: Vec<&str> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Sung(l) => lang_adjective(*l),
            _ => None,
        })
        .collect();
    let rest: Vec<&str> = kinds
        .iter()
        .filter_map(|k| match k {
            Kind::Sung(l) if lang_adjective(*l).is_some() => None,
            Kind::Instrumental | Kind::InstrumentalSoundtracks => Some("без слов"),
            Kind::Soundtracks => Some("саундтреки"),
            Kind::Songs | Kind::Sung(_) => Some("песни"),
            Kind::Calm => Some("спокойное"),
            Kind::Lively => Some("бодрое"),
        })
        .collect();
    let adjectives = |sung: &[&str]| -> String {
        let mut out = sung[0].to_owned();
        for (k, a) in sung.iter().enumerate().skip(1) {
            out.push_str(if k + 1 == sung.len() { " и " } else { ", " });
            out.push_str(&a.to_lowercase());
        }
        out
    };
    let join = |head: String| -> String {
        let mut parts = vec![head];
        parts.extend(rest.iter().map(|r| (*r).to_owned()));
        parts.join(" + ")
    };
    let mut out = Vec::new();
    if sung.is_empty() {
        let mut first = rest.first().map(|r| (*r).to_owned()).unwrap_or_default();
        if let Some(c) = first.chars().next() {
            first = c.to_uppercase().collect::<String>() + &first[c.len_utf8()..];
        }
        out.push(std::iter::once(first).chain(rest.iter().skip(1).map(|r| (*r).to_owned())).collect::<Vec<_>>().join(" + "));
    } else {
        out.push(join(format!("{} песни", adjectives(&sung))));
        if !rest.is_empty() {
            out.push(join(adjectives(&sung)));
        }
    }
    out
}

/// The name of a side by what it holds: of one language at
/// [`LANG_HOLD`], without words or of songs at [`KIND_SHARE`].
fn side_title(calm: bool, lanes: impl Iterator<Item = LaneKey>) -> String {
    let lanes: Vec<LaneKey> = lanes.collect();
    let n = lanes.len().max(1) as f32;
    let inst = lanes.iter().filter(|l| **l == LaneKey::Instrumental).count() as f32 / n;
    let mut langs: HashMap<Lang, usize> = HashMap::new();
    for l in &lanes {
        if let LaneKey::Sung(x) = l {
            *langs.entry(*x).or_default() += 1;
        }
    }
    let sung = langs.values().sum::<usize>() as f32 / n;
    let top = langs.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
    let word = if calm { "потише" } else { "погромче" };
    if let Some((l, k)) = top
        && k as f32 >= LANG_HOLD * n
        && lang_adjective(l).is_some()
    {
        return format!("{} {word}", Kind::Sung(l).title());
    }
    if inst >= KIND_SHARE {
        return if calm { "Спокойное без слов".into() } else { "Бодрое без слов".into() };
    }
    if sung >= KIND_SHARE {
        return if calm { "Спокойные песни".into() } else { "Песни погромче".into() };
    }
    if calm { "Спокойное".into() } else { "Погромче".into() }
}

/// The name of a kind of song before there are profiles.
fn kind_title(l: LaneKey) -> String {
    match l {
        LaneKey::Instrumental => Kind::Instrumental.title(),
        LaneKey::Sung(x) => Kind::Sung(x).title(),
        LaneKey::Unknown => "Песни".into(),
    }
}

/// How many of each rule's kind the places left can still hold, and what
/// a candidate would leave over it (an artist with many songs is not
/// cornered at the end).
struct Crowd {
    artist: HashMap<u64, usize>,
    album: HashMap<u64, usize>,
    world: HashMap<u64, usize>,
    caps: (usize, usize, usize),
}

impl Crowd {
    fn of(b: &Builder<'_>, left: &[usize], room: usize, rules: &Diversity) -> Self {
        let mut c = Self { artist: HashMap::new(), album: HashMap::new(), world: HashMap::new(), caps: rules.caps(room) };
        for &i in left {
            let s = b.slot(i);
            for a in s.artists {
                *c.artist.entry(a).or_default() += 1;
            }
            if let Some(x) = s.album {
                *c.album.entry(x).or_default() += 1;
            }
            if let Some(w) = s.world {
                *c.world.entry(w).or_default() += 1;
            }
        }
        c
    }

    fn over(&self, s: &Slot) -> usize {
        let over = |c: usize, mine: bool, cap: usize| (c - usize::from(mine)).saturating_sub(cap);
        self.artist.iter().map(|(a, c)| over(*c, s.artists.contains(a), self.caps.0)).sum::<usize>()
            + self.album.iter().map(|(x, c)| over(*c, s.album == Some(*x), self.caps.1)).sum::<usize>()
            + self.world.iter().map(|(w, c)| over(*c, s.world == Some(*w), self.caps.2)).sum::<usize>()
    }
}

/// A selection being filled under the caps.
struct Picking<'b, 'a> {
    b: &'b Builder<'a>,
    caps: Caps,
    quota: Option<HashMap<usize, usize>>,
    max_ms: u64,
    out: Vec<usize>,
    ms: u64,
    songs: HashSet<TrackId>,
    per_artist: HashMap<ArtistId, usize>,
    per_album: HashMap<u64, usize>,
    per_world: HashMap<u64, usize>,
    per_profile: HashMap<usize, usize>,
    /// A mix of quotas: a world's and an artist's songs of each profile.
    per_profile_world: HashMap<(usize, u64), usize>,
    per_profile_artist: HashMap<(usize, ArtistId), usize>,
    per_profile_album: HashMap<(usize, u64), usize>,
    unheard: usize,
    stale: usize,
}

impl<'b, 'a> Picking<'b, 'a> {
    fn new(b: &'b Builder<'a>, caps: Caps, quota: Option<HashMap<usize, usize>>, max_ms: u64) -> Self {
        Self {
            b,
            caps,
            quota,
            max_ms,
            out: Vec::new(),
            ms: 0,
            songs: HashSet::new(),
            per_artist: HashMap::new(),
            per_album: HashMap::new(),
            per_world: HashMap::new(),
            per_profile: HashMap::new(),
            per_profile_world: HashMap::new(),
            per_profile_artist: HashMap::new(),
            per_profile_album: HashMap::new(),
            unheard: 0,
            stale: 0,
        }
    }

    fn len(&self) -> usize {
        self.out.len()
    }

    fn full(&self, n: usize) -> bool {
        self.out.len() >= n || self.ms >= self.max_ms
    }

    /// Takes `i` if every cap allows it.
    fn take(&mut self, i: usize) {
        let t = self.b.recs.info(i);
        let song = t.song.unwrap_or(t.id);
        if self.songs.contains(&song) || self.songs.contains(&t.id) {
            return;
        }
        let album = self.b.model.release_at(i);
        let profile = self.b.model.profile_at(i);
        let unheard = self.b.unheard(i);
        let stale = self.b.stale[i];
        if t.artists.iter().any(|a| self.per_artist.get(a).is_some_and(|c| *c >= self.caps.artist))
            || album.is_some_and(|x| self.per_album.get(&x).is_some_and(|c| *c >= self.caps.album))
            || t.world.is_some_and(|w| self.per_world.get(&w).is_some_and(|c| *c >= self.caps.world))
            || (unheard && self.unheard >= self.caps.unheard)
            || (stale && self.stale >= self.caps.stale)
        {
            return;
        }
        if let Some(q) = &self.quota {
            let room = profile.and_then(|p| q.get(&p)).copied().unwrap_or(0);
            if profile.is_none_or(|p| self.per_profile.get(&p).copied().unwrap_or(0) >= room) {
                return;
            }
            // Its profile's share in blocks: a world or an artist half of
            // it at most, so a block can take turns.
            let half = room.div_ceil(2).max(2);
            let j = profile.unwrap_or(usize::MAX);
            // An album a quarter of it: its blocks stand near each other.
            let quarter = room.div_ceil(4).max(1);
            if album.is_some_and(|x| self.per_profile_album.get(&(j, x)).is_some_and(|c| *c >= quarter))
                || t.world.is_some_and(|w| self.per_profile_world.get(&(j, w)).is_some_and(|c| *c >= half))
                || t.artists.first().is_some_and(|a| self.per_profile_artist.get(&(j, *a)).is_some_and(|c| *c >= half))
            {
                return;
            }
        }
        self.songs.insert(song);
        self.songs.insert(t.id);
        for a in &t.artists {
            *self.per_artist.entry(*a).or_default() += 1;
        }
        if let Some(x) = album {
            *self.per_album.entry(x).or_default() += 1;
        }
        if let Some(w) = t.world {
            *self.per_world.entry(w).or_default() += 1;
        }
        if let Some(p) = profile {
            *self.per_profile.entry(p).or_default() += 1;
            if let Some(w) = t.world {
                *self.per_profile_world.entry((p, w)).or_default() += 1;
            }
            if let Some(a) = t.artists.first() {
                *self.per_profile_artist.entry((p, *a)).or_default() += 1;
            }
            if let Some(x) = album {
                *self.per_profile_album.entry((p, x)).or_default() += 1;
            }
        }
        self.unheard += usize::from(unheard);
        self.stale += usize::from(stale);
        self.ms += u64::from(t.duration_ms.unwrap_or(210_000));
        self.out.push(i);
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{EventOrigin, EventType, ReleaseId, UnixMs};

    use super::*;
    use crate::VectorIndex;
    use crate::analysis::DIM;
    use crate::model::ModelInput;
    use crate::songtype::{SongType, SongTypes, Voice};
    use crate::taste::{Catalog, DAY_MS, HOUR_MS, Play, TasteInput, TrackInfo};
    use crate::test_util::*;

    const T0: UnixMs = 20_000 * DAY_MS;

    /// A listener of Russian and Japanese songs and of soundtracks without
    /// words, 110 tracks of each kind in eight worlds, a favourite 60 % of
    /// each kind played over `days` days (the rest never), each track's
    /// sound its kind's place plus an energy of its own; `extra` plays more.
    struct Lib {
        catalog: Catalog,
        index: VectorIndex<TrackId>,
        types: SongTypes,
        input: TasteInput,
    }

    fn lib(days: i64, extra: &[(UnixMs, usize)]) -> Lib {
        let kinds = [(Some(Lang::Russian), 0.5f32), (Some(Lang::Japanese), 0.3), (None, 0.2)];
        let per = 110usize;
        let mut rng = Rng::new(9);
        let (mut tracks, mut types) = (Vec::new(), SongTypes::default());
        let mut index = VectorIndex::new();
        for (k, (lang, _)) in kinds.iter().enumerate() {
            for t in 0..per {
                let i = (k * per + t) as u128;
                let world = format!("kind {k} world {}", t % 8);
                let artist = (k * 100 + t % 14) as u32;
                let release = ReleaseId(uuid::Uuid::from_u128(1_000 + (k * per + t / 9) as u128));
                let ms = 150_000 + (t as u32 % 7) * 20_000;
                tracks.push(TrackInfo::new(tid(i), vec![aid(artist)], Some(release)).with_world(&world).with_duration(Some(ms)));
                let mut v = [0.0f32; DIM];
                for (d, x) in v.iter_mut().enumerate() {
                    *x = rng.f32() * 0.4 + if d % 3 == k { 2.0 } else { 0.0 };
                }
                // An energy of its own on the axes the moods read.
                let e = rng.f32() * 3.0;
                for d in crate::mood::ENERGY_FEATURES {
                    v[d] += e;
                }
                index.insert(tid(i), v);
                let ty = match lang {
                    Some(l) => SongType { voice: Voice::Sung, lang: Some(*l), ..SongType::default() },
                    None => SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() },
                };
                types.by_track.insert(tid(i), ty);
            }
        }
        let mut events = Vec::new();
        let mut ts = T0 - days * DAY_MS;
        let fav = per * 3 / 5;
        for _ in 0..days {
            for s in 0..2 {
                let mut x = rng.f32();
                let k = kinds
                    .iter()
                    .position(|(_, w)| {
                        x -= w;
                        x <= 0.0
                    })
                    .unwrap_or(0);
                let mut at = ts + (9 + s * 8) * HOUR_MS;
                for _ in 0..10 {
                    let i = k * per + rng.below(fav);
                    events.push(Play { origin: Some(EventOrigin::Manual), ..Play::new(tid(i as u128), at, EventType::Start, Some(0)) });
                    at += 180_000;
                    events.push(Play::new(tid(i as u128), at, EventType::Complete, Some(180_000)));
                }
            }
            ts += DAY_MS;
        }
        for &(at, i) in extra {
            events.push(Play { origin: Some(EventOrigin::Manual), ..Play::new(tid(i as u128), at, EventType::Start, Some(0)) });
            events.push(Play::new(tid(i as u128), at + 180_000, EventType::Complete, Some(180_000)));
        }
        events.sort_by_key(|p| p.ts);
        Lib { catalog: Catalog::new(tracks), index, types, input: TasteInput { events, ..TasteInput::default() } }
    }

    fn model(l: &Lib, now: UnixMs) -> UserModel {
        UserModel::build(&l.catalog, &l.input, ModelInput { index: &l.index, types: Some(&l.types), weights: None, graph: None }, now)
    }

    fn with<T>(l: &Lib, m: &UserModel, part: DayPart, f: impl FnOnce(&Builder<'_>) -> T) -> T {
        in_epoch(l, m, part, m.now().div_euclid(DAY_MS) as u64, f)
    }

    fn in_epoch<T>(l: &Lib, m: &UserModel, part: DayPart, epoch: u64, f: impl FnOnce(&Builder<'_>) -> T) -> T {
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(m);
        let no = |_: usize| false;
        let b = Builder::new(&recs, m, &|_| true, &no, epoch, part, Live::default(), Settings::default());
        f(&b)
    }

    fn energy(m: &UserModel, tracks: &[usize]) -> Vec<f32> {
        let raw: Vec<f32> = tracks.iter().map(|&i| m.energy_at(i).unwrap()).collect();
        crate::listening::pct_rank(&raw)
    }

    fn mean(x: &[f32]) -> f32 {
        x.iter().sum::<f32>() / x.len() as f32
    }

    fn profile(m: &UserModel, lane: LaneKey) -> u32 {
        m.profiles.list.iter().find(|p| p.lane == lane).unwrap().key
    }

    /// At night a mix settles: its first third is livelier than its last;
    /// by day it rises to its second half. The songs are the same.
    #[test]
    fn arc_settles_at_night() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let ru = MixKey::Profile(profile(&m, LaneKey::Sung(Lang::Russian)));
        let night = with(&l, &m, DayPart::Night, |b| b.mix(ru, &Kept::default())).unwrap();
        let e = energy(&m, &night.tracks);
        let third = e.len() / 3;
        assert!(mean(&e[..third]) > mean(&e[e.len() - third..]) + 0.25, "{e:?}");
        let day = with(&l, &m, DayPart::Day, |b| b.mix(ru, &Kept::default())).unwrap();
        let e = energy(&m, &day.tracks);
        assert!(mean(&e[e.len() / 2..e.len() * 4 / 5]) > mean(&e[..third / 2 + 1]) + 0.15, "{e:?}");
        let set = |t: &[usize]| t.iter().copied().collect::<HashSet<usize>>();
        assert_eq!(set(&night.tracks), set(&day.tracks), "the same songs, another order");
    }

    /// A song never played stands only between two loved familiar ones,
    /// never in the first two places, never two running.
    #[test]
    fn new_is_sandwiched() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let (mixes, _) = with(&l, &m, DayPart::Day, |b| b.lineup(&Kept::default()));
        let mut seen = 0;
        for mix in &mixes {
            for (k, &i) in mix.tracks.iter().enumerate() {
                if m.tracks[i].starts > 0 {
                    continue;
                }
                seen += 1;
                assert!(k >= 2 && k + 1 < mix.tracks.len(), "{}: unheard at {k}", mix.title);
                assert!(mix.anchors[k - 1] && mix.anchors[k + 1], "{}: unheard at {k} not between loved familiar songs", mix.title);
            }
            let unheard = mix.tracks.iter().filter(|&&i| m.tracks[i].starts == 0).count();
            assert!(unheard as f32 <= (UNHEARD_SHARE * mix.tracks.len() as f32).ceil() + 1.0, "{}: {unheard}", mix.title);
        }
        assert!(seen > 0, "the mixes bring something never played");
    }

    /// Later in the epoch, after more plays, the lineup, its order and the
    /// songs of each mix in theirs are the same.
    #[test]
    fn stable_within_epoch() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let (first, lineup) = in_epoch(&l, &m, DayPart::Morning, 5, |b| b.lineup(&Kept::default()));
        let mut kept = Kept { lineup, ..Kept::default() };
        kept.keep(&first, &m);
        // Ten more songs, another session.
        let plays: Vec<(UnixMs, usize)> = (0..10).map(|k| (T0 + 9 * HOUR_MS + k * 200_000, 3 * k as usize)).collect();
        let l2 = lib(40, &plays);
        let m2 = model(&l2, T0 + 10 * HOUR_MS);
        let (again, keys) = in_epoch(&l2, &m2, DayPart::Morning, 5, |b| b.lineup(&kept));
        assert_eq!(keys, kept.lineup);
        let order = |x: &[Mix]| x.iter().map(|m| m.key).collect::<Vec<_>>();
        assert_eq!(order(&again), order(&first), "the cards in their order");
        for (a, b) in first.iter().zip(&again) {
            assert_eq!(a.tracks, b.tracks, "{}", a.title);
        }
    }

    /// The next epoch (a start of the app, another part of the day) draws
    /// another lineup and other songs: a mix that stays shares a third or
    /// so of its songs and shows other covers first.
    #[test]
    fn the_next_epoch_rotates() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let (now, lineup) = in_epoch(&l, &m, DayPart::Morning, 11, |b| b.lineup(&Kept::default()));
        let mut kept = Kept { lineup: lineup.clone(), ..Kept::default() };
        kept.keep(&now, &m);
        let next = kept.clone().next_epoch(true);
        assert_eq!(next.shown, std::slice::from_ref(&lineup));
        let (then, keys) = in_epoch(&l, &m, DayPart::Morning, 12, |b| b.lineup(&next));
        assert_ne!(keys, lineup, "another lineup");
        let mut compared = 0;
        for a in &now {
            let b = in_epoch(&l, &m, DayPart::Morning, 12, |b| b.mix(a.key, &next)).unwrap();
            let set: HashSet<usize> = a.tracks.iter().copied().collect();
            let shared = b.tracks.iter().filter(|i| set.contains(i)).count() as f32 / a.tracks.len().max(b.tracks.len()) as f32;
            assert!((0.05..=0.55).contains(&shared), "{}: {shared}", a.title);
            let firsts = |t: &[usize]| t.iter().take(6).copied().collect::<HashSet<usize>>();
            assert!(firsts(&a.tracks) != firsts(&b.tracks), "{}", a.title);
            compared += 1;
        }
        assert!(compared >= 3, "{compared}");
        let _ = then;
    }

    /// «Другой состав» draws another mix: much of it new, the same kind.
    #[test]
    fn another_draw() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let ru = MixKey::Profile(profile(&m, LaneKey::Sung(Lang::Russian)));
        let a = with(&l, &m, DayPart::Day, |b| b.mix(ru, &Kept::default())).unwrap();
        let mut kept = Kept::default();
        kept.keep(std::slice::from_ref(&a), &m);
        kept.salts.push((ru, 7));
        let b = with(&l, &m, DayPart::Day, |b| b.mix(ru, &kept)).unwrap();
        let set: HashSet<usize> = a.tracks.iter().copied().collect();
        let shared = b.tracks.iter().filter(|i| set.contains(i)).count() as f32 / b.tracks.len() as f32;
        assert!(shared < 0.7, "{shared}");
        assert!(b.tracks.iter().all(|&i| m.profile_at(i) == a.tracks.first().and_then(|&f| m.profile_at(f))));
    }

    /// Five moments of three profiles, drawn from «Всё вперемешку», the
    /// profiles and both sides; no two alike, none of one world, every rule
    /// kept, nothing too long or too short.
    #[test]
    fn the_lineup_of_three_profiles() {
        let l = lib(40, &[]);
        let m = model(&l, T0);
        let (mixes, keys) = with(&l, &m, DayPart::Day, |b| b.lineup(&Kept::default()));
        let titles: Vec<&str> = mixes.iter().map(|x| x.title.as_str()).collect();
        let shares: Vec<(LaneKey, f32, bool)> = m.profiles.list.iter().zip(&m.profile_share).map(|(p, s)| (p.lane, *s, p.visible)).collect();
        assert_eq!(mixes.len(), MOMENTS, "{titles:?} {shares:?}");
        assert_eq!(keys.len(), MOMENTS);
        let unique: HashSet<&str> = titles.iter().copied().collect();
        assert_eq!(unique.len(), titles.len(), "{titles:?}");
        let recs = Recs::new(&l.catalog, &m.taste, &l.index);
        for mix in &mixes {
            assert!((MIX_LEN - 4..=MIX_LEN).contains(&mix.tracks.len()), "{}: {}", mix.title, mix.tracks.len());
            let picks: Vec<crate::Pick> = mix.tracks.iter().map(|&i| crate::Pick { item: crate::Item::Track(m.id(i)), reason: Reason::Favourite }).collect();
            crate::pick::assert_diverse(&recs, &picks);
            let mut worlds: HashMap<u64, usize> = HashMap::new();
            for &i in &mix.tracks {
                *worlds.entry(l.catalog.tracks()[i].world.unwrap()).or_default() += 1;
            }
            let cap = (mix.tracks.len() as f32 * WORLD_SHARE).ceil() as usize;
            assert!(worlds.len() >= 4 && worlds.values().all(|c| *c <= cap), "{}: {worlds:?}", mix.title);
        }
        // «Не предлагать этот микс»: the epoch's lineup goes on without it,
        // another in its place.
        let gone = keys[1];
        let kept = Kept { lineup: keys.clone(), ..Kept::default() };
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(&m);
        let no = |_: usize| false;
        let settings = Settings { hidden: vec![gone], ..Settings::default() };
        let b = Builder::new(&recs, &m, &|_| true, &no, m.now().div_euclid(DAY_MS) as u64, DayPart::Day, Live::default(), settings);
        let (after, after_keys) = b.lineup(&kept);
        assert!(!after_keys.contains(&gone) && after.iter().all(|x| x.key != gone), "{after_keys:?}");
        assert_eq!(after.len(), mixes.len(), "{after_keys:?}");
        assert_eq!(after_keys[..3], [keys[0], keys[2], keys[3]], "the rest in their places");
    }

    /// The listener of [`lib`] with an old collection brought from a
    /// service: the 40 % of each kind never played here, each played five
    /// times three years ago there and all liked at once yesterday; and the
    /// first forty favourites of each kind liked by hand over the last
    /// weeks. The old songs, by their numbers.
    fn with_old_collection(mut l: Lib) -> (Lib, Vec<usize>) {
        let per = 110usize;
        let fav = per * 3 / 5;
        let old: Vec<usize> = (0..3).flat_map(|k| (fav..per).map(move |t| k * per + t)).collect();
        for &i in &old {
            for n in 0..5i64 {
                let at = T0 - (3 * 365 + n * 7) * DAY_MS + i as i64 * 1_000;
                l.input.events.push(Play { origin: Some(EventOrigin::Import), ..Play::new(tid(i as u128), at, EventType::Start, Some(0)) });
                l.input.events.push(Play { origin: Some(EventOrigin::Import), ..Play::new(tid(i as u128), at + 180_000, EventType::Complete, Some(180_000)) });
            }
            l.input.likes.push(tid(i as u128));
            l.input.like_times.push((tid(i as u128), T0 - DAY_MS + i as i64));
        }
        for k in 0..3 {
            for t in 0..40usize {
                let i = (k * per + t) as u128;
                l.input.likes.push(tid(i));
                l.input.like_times.push((tid(i), T0 - (k * 40 + t) as i64 * 4 * HOUR_MS - 2 * DAY_MS));
            }
        }
        l.input.events.sort_by_key(|p| p.ts);
        (l, old)
    }

    /// A collection brought from a service — liked all at once, last
    /// played years ago — does not take the mixes over: its songs count
    /// for a third of what is heard now, a mix takes a quarter of them at
    /// most («Любимые» half), the mixes of the day a third among them all.
    #[test]
    fn an_old_imported_collection_is_a_minority() {
        let (l, old) = with_old_collection(lib(40, &[]));
        let m = model(&l, T0);
        let old: HashSet<usize> = old.iter().filter_map(|&i| m.position(tid(i as u128))).collect();
        assert_eq!(old.len(), 132);
        with(&l, &m, DayPart::Day, |b| {
            for &i in &old {
                assert!(b.stale_at(i), "{i}: liked in a bulk and played years ago is an old song");
                assert!(b.fresh_at(i) <= STALE_FLOOR + 0.01, "{i}: {}", b.fresh_at(i));
            }
            let heard_now = (0..m.len()).filter(|i| !old.contains(i) && m.tracks[*i].starts > 0);
            for i in heard_now {
                assert!(!b.stale_at(i) && b.fresh_at(i) > 0.8, "{i}: {}", b.fresh_at(i));
            }
            let mut mixes = b.lineup(&Kept::default()).0;
            mixes.extend(b.pinned(&Kept::default()));
            assert!(mixes.iter().any(|x| x.key == MixKey::Liked), "the likes make their mix");
            for mix in &mixes {
                let n = mix.tracks.iter().filter(|i| old.contains(i)).count();
                let (share, len) = if mix.key == MixKey::Liked { (LIKED_STALE_SHARE, LIKED_LEN) } else { (STALE_SHARE, MIX_LEN) };
                assert!(n as f32 <= (share * len as f32).ceil(), "{}: {n} old songs of {}", mix.title, mix.tracks.len());
            }
            let daily = b.daily(&Kept::default()).0;
            assert!(daily.len() >= 3, "{}", daily.len());
            let all: usize = daily.iter().map(|x| x.tracks.len()).sum();
            let of_old: usize = daily.iter().map(|x| x.tracks.iter().filter(|i| old.contains(i)).count()).sum();
            assert!(of_old * 3 <= all, "the mixes of the day: {of_old} old songs of {all}");
        });
        // With nothing heard at all and no like set by hand, the likes are
        // what there is: none of them is old, each counts whole.
        let brought: HashSet<TrackId> = old.iter().map(|&i| m.id(i)).collect();
        let mut only_likes = l;
        only_likes.input.events.clear();
        only_likes.input.likes.retain(|t| brought.contains(t));
        only_likes.input.like_times.retain(|(t, _)| brought.contains(t));
        let m = model(&only_likes, T0);
        with(&only_likes, &m, DayPart::Day, |b| {
            let liked: Vec<usize> = (0..m.len()).filter(|&i| m.tracks[i].liked(T0)).collect();
            assert_eq!(liked.len(), 132);
            let bulk = liked.iter().filter(|&&i| old.contains(&i));
            for &i in bulk {
                assert!(!b.stale_at(i) && b.fresh_at(i) == 1.0, "{i}");
            }
        });
    }

    /// The mixes of the day: one at least for each taste profile, six at
    /// most, named by their places, no song in two of them, a share of
    /// songs never played; the same all day whatever is played meanwhile
    /// and whatever the part of the day, other songs the next day.
    #[test]
    fn the_mixes_of_the_day_are_distinct_and_the_days_own() {
        let l = lib(40, &[]);
        let m = model(&l, T0 + 7 * HOUR_MS);
        let day = 77u64;
        let (first, keys) = in_epoch(&l, &m, DayPart::Morning, day, |b| b.daily(&Kept::default()));
        assert!((3..=DAILY_MIXES).contains(&first.len()), "{}", first.len());
        let mut seen: HashSet<usize> = HashSet::new();
        let mut profiles: HashSet<usize> = HashSet::new();
        for (k, mix) in first.iter().enumerate() {
            assert_eq!(mix.title, format!("Микс дня {}", k + 1));
            assert_eq!(mix.key, keys[k]);
            assert!(matches!(mix.key, MixKey::Daily(_)));
            assert!((MIN_SONGS..=MIX_LEN).contains(&mix.tracks.len()), "{}: {}", mix.title, mix.tracks.len());
            assert!(mix.profiles.len() <= 1, "{}: of {:?}", mix.title, mix.profiles);
            profiles.extend(mix.profiles.iter().copied());
            for &i in &mix.tracks {
                assert!(seen.insert(i), "{}: a song of another mix of the day", mix.title);
            }
            let unheard = mix.tracks.iter().filter(|&&i| m.tracks[i].starts == 0).count();
            assert!(unheard as f32 <= (DAILY_UNHEARD * MIX_LEN as f32).ceil(), "{}: {unheard} never played", mix.title);
        }
        assert!(profiles.len() >= 3, "each profile has its mix of the day: {profiles:?}");
        // Another part of the same day, nothing kept: the same.
        let (evening, _) = in_epoch(&l, &m, DayPart::Evening, day, |b| b.daily(&Kept::default()));
        let songs = |x: &[Mix]| x.iter().map(|m| (m.key, m.tracks.clone())).collect::<Vec<_>>();
        assert_eq!(songs(&evening), songs(&first));
        // Ten more songs played, the model built again: the day's mixes as
        // they were kept.
        let mut kept = Kept { lineup: keys.clone(), ..Kept::default() };
        kept.keep(&first, &m);
        let plays: Vec<(UnixMs, usize)> = (0..10).map(|k| (T0 + 9 * HOUR_MS + k * 200_000, 3 * k as usize)).collect();
        let l2 = lib(40, &plays);
        let m2 = model(&l2, T0 + 10 * HOUR_MS);
        let (again, keys2) = in_epoch(&l2, &m2, DayPart::Day, day, |b| b.daily(&kept));
        assert_eq!(keys2, keys);
        let ids = |m: &UserModel, x: &[Mix]| x.iter().map(|mix| mix.tracks.iter().map(|&i| m.id(i)).collect::<Vec<_>>()).collect::<Vec<_>>();
        assert_eq!(ids(&m2, &again), ids(&m, &first));
        assert_eq!(again.iter().map(|x| x.title.clone()).collect::<Vec<_>>(), first.iter().map(|x| x.title.clone()).collect::<Vec<_>>());
        // The next day draws again.
        let (next, _) = in_epoch(&l, &m, DayPart::Morning, day + 1, |b| b.daily(&Kept::default()));
        assert_eq!(next.len(), first.len());
        assert!(next.iter().zip(&first).any(|(a, b)| a.tracks != b.tracks), "another day, other songs");
    }

    /// «Любимые» is there while there are likes enough, first, of liked
    /// songs alone, the epoch's; the mix of the part of the day beside it;
    /// either can be left out.
    #[test]
    fn the_liked_songs_make_their_mix() {
        let mut l = lib(40, &[]);
        let m = model(&l, T0);
        let morning = MixKey::Part(DayPart::Morning.index() as u8);
        let pinned = with(&l, &m, DayPart::Morning, |b| b.pinned(&Kept::default()));
        assert!(pinned.iter().all(|x| x.key != MixKey::Liked), "nothing liked, no «Любимые»");
        let part = pinned.iter().find(|x| x.key == morning).expect("the mix of the morning");
        assert_eq!(part.title, "Утренний микс");
        assert!(part.tracks.iter().all(|&i| m.tracks[i].starts > 0), "of what is loved");
        // Every third song liked by hand over the last weeks.
        for (n, i) in (0..330u128).step_by(3).enumerate() {
            l.input.likes.push(tid(i));
            l.input.like_times.push((tid(i), T0 - n as i64 * 5 * HOUR_MS));
        }
        let m = model(&l, T0);
        let pinned = with(&l, &m, DayPart::Morning, |b| b.pinned(&Kept::default()));
        assert_eq!(pinned[0].key, MixKey::Liked, "«Любимые» first");
        assert_eq!(pinned[0].title, "Любимые");
        assert!((MIN_SONGS..=LIKED_LEN).contains(&pinned[0].tracks.len()), "{}", pinned[0].tracks.len());
        assert!(pinned[0].tracks.iter().all(|&i| m.tracks[i].liked(T0)), "liked songs alone");
        // Kept for the epoch: the same songs in their order.
        let mut kept = Kept::default();
        kept.keep(&pinned, &m);
        let again = with(&l, &m, DayPart::Morning, |b| b.pinned(&kept));
        assert_eq!(again.iter().map(|x| (x.key, x.tracks.clone())).collect::<Vec<_>>(), pinned.iter().map(|x| (x.key, x.tracks.clone())).collect::<Vec<_>>());
        // «Не предлагать этот микс».
        let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(&m);
        let no = |_: usize| false;
        let settings = Settings { hidden: vec![MixKey::Liked, morning], ..Settings::default() };
        let b = Builder::new(&recs, &m, &|_| true, &no, 1, DayPart::Morning, Live::default(), settings);
        assert!(b.pinned(&Kept::default()).is_empty());
        assert_eq!(part_title(DayPart::Night.index() as u8), "Ночной микс");
        assert_ne!(MixKey::Liked.token(), MixKey::Daily(1).token());
    }

    #[test]
    fn keys_and_names() {
        assert_eq!(MixKey::blend(&[3, 3, 5]), Some(MixKey::Blend([3, 5, 0])));
        let names = |n: &[&str]| n.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        assert_eq!(blend_title(names(&["Русские песни", "Саундтреки без слов", "Японские песни"])), "Русские и японские песни + Саундтреки без слов");
        assert_eq!(blend_title(names(&["Японские песни", "Саундтреки без слов"])), "Японские песни + Саундтреки без слов");
        // Shorter forms for a narrow card, of the kinds alone.
        let (ru, ja) = (Kind::Sung(Lang::Russian), Kind::Sung(Lang::Japanese));
        assert_eq!(
            blend_short(&[ru, Kind::InstrumentalSoundtracks, ja]),
            ["Русские и японские песни + без слов", "Русские и японские + без слов"]
        );
        assert_eq!(blend_short(&[ja, Kind::InstrumentalSoundtracks]), ["Японские песни + без слов", "Японские + без слов"]);
        assert_eq!(blend_short(&[ru, ja]), ["Русские и японские песни"]);
        assert_eq!(blend_short(&[Kind::Instrumental, Kind::Soundtracks]), ["Без слов + саундтреки"]);
        assert!(blend_short(&[ru, ru]).is_empty(), "two of one kind: their names tell them apart");
        assert_eq!(MixKey::blend(&[3]), None);
        assert_ne!(MixKey::Calm.token(), MixKey::Lively.token());
        for l in [LaneKey::Instrumental, LaneKey::Unknown, LaneKey::Sung(Lang::Korean), LaneKey::Sung(Lang::Other)] {
            assert_eq!(lane_of_code(lane_code(l)), l);
        }
        let ru = || std::iter::repeat_n(LaneKey::Sung(Lang::Russian), 8).chain([LaneKey::Instrumental, LaneKey::Unknown]);
        assert_eq!(side_title(true, ru()), "Русские песни потише");
        let mixed = [LaneKey::Sung(Lang::Russian), LaneKey::Sung(Lang::Japanese), LaneKey::Sung(Lang::English), LaneKey::Instrumental];
        assert_eq!(side_title(false, mixed.into_iter().cycle().take(20)), "Песни погромче");
        assert_eq!(side_title(true, std::iter::repeat_n(LaneKey::Instrumental, 7).chain([LaneKey::Sung(Lang::Russian); 3])), "Спокойное без слов");
        assert_eq!(side_title(false, [LaneKey::Instrumental, LaneKey::Sung(Lang::Russian)].into_iter().cycle().take(10)), "Погромче");
    }
}
