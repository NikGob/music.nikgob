//! «Миксы для тебя»: five moments of the user's
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
//!   cards by the balance of the profiles, «Всё
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
//! * **The names** ([`Builder::title`]): what is in it — the
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
//! * **The mixes of the day** ([`Builder::daily`]): one «Микс дня» of
//!   everything the user listens to now, and the named mixes beside it —
//!   what the user listens to now by taste profile, the heavier profiles
//!   split by sound into two or three (a weighted k-means from the
//!   farthest points), one mix for each cluster, the heaviest first, at
//!   most [`DAILY_MIXES`], each named by what it is made of
//!   ([`Builder::name_daily`], [`crate::mix_names`]: its genre, its leading
//!   artist, its decade, its mood). A cluster's mix draws from the songs
//!   of its profile nearest its centre: every song is one cluster's, so no
//!   two named mixes share a song. Mostly loved and familiar,
//!   [`DAILY_UNHEARD`] of songs never played that stand near them; seeded
//!   by the day, and what a day drew stays the day's ([`Kept`]) whatever
//!   is played meanwhile.
//! * **The genre mixes** ([`MixKey::Genre`], [`Builder::with_genres`]):
//!   built by genre, not named after the fact. A song is of the family of
//!   genres its main artist is of ([`crate::mix_names::leading`]: the one
//!   leading the tags other listeners gave them); a family has a mix when
//!   enough of its songs are listened to now (as many as a cluster of the
//!   day needs) and its songs make a mix, the families the listener loves most now first (the weights
//!   the day's clusters go by), [`GENRE_MIXES`] at most. Its songs are
//!   drawn as a cluster's: mostly loved and familiar, [`DAILY_UNHEARD`] of
//!   those never played, seeded by the day and the day's own. A song has
//!   one family, so no two genre mixes share a song; the mix is called by
//!   its family («Хип-хоп микс»).
//! * **The mix of the part of the day** ([`Builder::pinned`]): «На утро»,
//!   «На день», «На вечер», «На ночь» — the loved songs this part hears
//!   more than the others, the liked ones before the rest, while it has
//!   songs enough; in the epoch's rotation like any moment. («Любимые» is
//!   the playlist's own tile; a mix of the liked songs
//!   ([`MixKey::Liked`]) can still be drawn, the row does not show it.)
//! * **The row** ([`row`]): «Микс дня», the genre mixes, the mix of the
//!   part of the day, a named mix or two to fill, the moments —
//!   [`ROW_MIXES`] at most, the first cards winning: a card that is
//!   another's look-alike (its name, [`OVERLAP`] of its songs, its genre,
//!   its kind of song) is left out.

use std::collections::HashMap;

use nikgob_core::i18n::Locale;
use nikgob_core::model::{ArtistId, TrackId};
use serde::{Deserialize, Serialize};

use crate::model::{Live, UserModel};
use crate::pick::Recs;
use crate::profiles::{Kind, LaneKey, NameStyle};
use crate::songtype::Lang;
use crate::taste::DayPart;

/// Moments of «Миксы для тебя».
pub const MOMENTS: usize = 5;
/// Named mixes of the day at most (beside the one «Микс дня»).
pub const DAILY_MIXES: usize = 5;
/// A cluster of the day needs this many songs listened to now…
const DAILY_CORES: usize = 8;
/// …and a profile gets one more mix for each this many of them (while its
/// songs are enough for as many mixes).
const DAILY_CORES_EACH: usize = 16;
/// Genre mixes of the day at most.
pub const GENRE_MIXES: usize = 4;
/// A card with this share of its songs of one family of genres is that
/// genre's mix again ([`row`]).
pub const GENRE_SAME: f32 = 0.75;
/// The mixes of the row of «Миксы для тебя» at most (beside the «Любимые»
/// tile and «На повторе»).
pub const ROW_MIXES: usize = 10;
/// The named mixes of the day only fill the row: this many at most, and
/// [`ROW_NAMED_FEW`] once [`GENRE_MANY`] genre mixes stand there.
pub const ROW_NAMED: usize = 2;
pub const ROW_NAMED_FEW: usize = 1;
pub const GENRE_MANY: usize = 3;
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
/// A moment of the row stands this much higher per unit of its songs' mean
/// fit to the part of this weekday ([`Builder::context`]).
const MOMENT_FIT: f32 = 0.5;
/// What is listened to now leads: the model's freshness of a love
/// ([`crate::model::freshness`], [`UserModel::freshness_at`]), which every
/// surface shares.
pub use crate::model::{BULK_LIKES, FRESH_GRACE_MS, FRESH_HALF_MS, STALE_FLOOR, STALE_MS};
/// …and a mix takes this share of them at most while it has others
/// («Любимые» [`LIKED_STALE_SHARE`]).
pub const STALE_SHARE: f32 = 0.25;
pub const LIKED_STALE_SHARE: f32 = 0.5;
/// The blends made today shown at most (the latest).
pub const BLENDS_SHOWN: usize = 2;
/// Songs of a mix (about 95–105 minutes)…
pub const MIX_LEN: usize = 30;
const MIX_MS: u64 = 110 * 60_000;
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
/// …or longer (skipped early a third of the time).
pub const LONG_MS: u32 = 9 * 60_000;
/// The first and the last song are not longer than this.
const EDGE_MS: u32 = 6 * 60_000;
/// One world takes at most this share of a mix.
const WORLD_SHARE: f32 = 0.25;
/// Songs never played take at most this share of a mix…
pub const UNHEARD_SHARE: f32 = 0.15;
/// …of «Фон».
const BG_UNHEARD_SHARE: f32 = 0.1;
/// «Преимущественно новые песни» ([`Settings::unheard`]): a mix takes at
/// least this share of songs never played while it has them — «Больше
/// новых», «Только новые» (each between two loved ones: not much more
/// fits).
pub const UNHEARD_MORE: f32 = 0.25;
pub const UNHEARD_MOST: f32 = 0.4;
/// The calm side is the loved songs below this percentile of their energy,
/// the lively one above `1 −` it.
const SIDE: f32 = 0.45;
/// A side with this share of one sung language keeps to it (and to songs
/// of no known language that are no soundtracks).
pub const LANG_HOLD: f32 = 0.7;
/// A kind of song names a side at this share of it.
pub const KIND_SHARE: f32 = 0.6;
/// A moment sharing this share of its songs with one before it is the
/// same; so is a card of the row sharing more with any card before it.
pub const OVERLAP: f32 = 0.65;
/// Value: what plays on repeat, the early skip, the unheard near the loved.
const W_HOT: f32 = 0.4;
const W_SKIP: f32 = 1.2;
const W_EXPLORE: f32 = 0.9;
/// A file just come to the library and never played: this much more value
/// among the unheard, fading as it waits
/// ([`crate::UserModel::arrival_at`]).
const W_ARRIVAL: f32 = 0.9;
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
/// Share of yesterday's familiar, non-worn tracks carried over as the daily
/// familiar core on automatic day rollover (manual refresh still rotates all).
const DAILY_CARRY_SHARE: f32 = 0.42;
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
/// «Фон» keeps to this band of its profile's energy,
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
    /// The day's mixes: 0 is «Микс дня», of everything listened to now;
    /// from 1 the named mix of the day's cluster of that number, the
    /// heaviest first.
    Daily(u8),
    /// The family of the mix of genres, by the family's number
    /// ([`crate::mix_names::family_title_in`]).
    Genre(u8),
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
            Self::Genre(f) => (11, u32::from(f), 0, 0),
        };
        crate::pick::token(&[a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes(), d.to_le_bytes()].concat())
    }

    /// A named mix of the day: one cluster's.
    pub const fn is_cluster(self) -> bool {
        matches!(self, Self::Daily(n) if n > 0)
    }

    /// A genre mix of the day.
    pub const fn is_genre(self) -> bool {
        matches!(self, Self::Genre(_))
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

pub fn lane_of_code(c: u8) -> LaneKey {
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
    /// «Преимущественно новые песни»: the share of songs never played a
    /// mix takes at least while it has them (0: only what their value
    /// brings, at most [`UNHEARD_SHARE`]). «Фон» and «Любимые» keep theirs.
    pub unheard: f32,
    /// Target locale for mix titles generated by [`Builder`].
    pub locale: Locale,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            profiles: true,
            everything: true,
            moods: true,
            background_cards: crate::profiles::BG_CARDS,
            names: NameStyle::Kind,
            hidden: Vec::new(),
            unheard: 0.0,
            locale: Locale::Ru,
        }
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
    /// The kind of song it is of, when it is of one (`Kind::Sung(Russian)`,
    /// a profile's kind, a kind of the library, the language a side keeps
    /// to): what tells a profile's mix and its louder or quieter side for
    /// one ([`row`]).
    pub kind: Option<Kind>,
    /// The family of genres it is of: a genre mix's own, another's when
    /// [`GENRE_SAME`] of its songs are of one.
    pub genre: Option<u8>,
    /// The size of the whole pool it is drawn from: its tracks and their
    /// main artists (whether the filter is a niche,
    /// [`crate::open_world::is_niche`], is told by these, not by the card).
    pub pool_size: (usize, usize),
    /// That pool when it is a niche (its songs seed the niche's new
    /// music); empty otherwise.
    pub pool: Vec<usize>,
}

/// The pool of a moment and how it is made.
#[derive(Clone)]
struct Pool {
    tracks: Vec<usize>,
    len: usize,
    max_ms: u64,
    unheard: f32,
    /// The share of songs never played it takes at least, while it has
    /// them ([`Settings::unheard`]).
    floor: f32,
    /// What the songs heard now and the old ones' share leave empty goes to
    /// more of the never played ([`Builder::unheard_and_stale`]); not once a
    /// draw showed how many of them find a place.
    widen: bool,
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
    /// The family of genres of each track ([`Self::with_genres`]; empty:
    /// none known), and the families that have a mix with their songs, the
    /// most loved now first, once asked for.
    genres: Vec<Option<u8>>,
    genre_pools: std::cell::OnceCell<Vec<(u8, Vec<usize>)>>,
}

/// Fills `n` places by `pick`: the counts each kind of rule allows.
struct Caps {
    artist: usize,
    album: usize,
    world: usize,
    unheard: usize,
    stale: usize,
}

/// The track at `i` was never played here and is not loved otherwise (a
/// like, a playlist, a play imported from a service, the library's prior):
/// what a mix counts as never played (its rule, the offline evaluation's
/// «unheard»).
pub fn never_heard(model: &UserModel, i: usize) -> bool {
    let s = &model.tracks[i];
    s.starts == 0 && s.aff_long(model.now()) <= 0.0 && model.taste.affinity_at(i) <= 0.0 && model.prior_mass(i) <= 0.0
}


mod compose;
mod naming;
mod pools;
mod sequence;

#[cfg(test)]
mod tests;

pub use naming::{
    SideFlavor, part_title_in, row, row_upto, side_flavor_of_types, side_title_for_flavor,
};
