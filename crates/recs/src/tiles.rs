//! The bento's smaller tiles (docs/stage4.md §12.9): 21 kinds of selections
//! in four groups (memory, artists, sound, collection), each a recipe that
//! picks, orders and names its songs, and the daily choice of which of
//! them the board shows.
//!
//! * A tile exists only with [`TILE_MIN`] songs or more ([`FRESH_MIN`] for
//!   «Свежие лайки»), up to [`TILE_LEN`]; disliked songs never come. It
//!   plays as it is: the diversity rules in the smooth order, except the
//!   lists whose order is the point (likes by time, albums to finish, a
//!   playlist shuffled).
//! * Parameterised kinds (the artist of the day, a comeback, the like a
//!   tile grows from, a language, a decade, a season, a franchise, a
//!   playlist) pick their parameter by the day, not one shown within
//!   [`PARAM_REST_DAYS`] while another qualifies.
//! * The day's board ([`Recs::tiles_of_day`]): «На повторе» first when it
//!   qualifies, then the others, deterministic by the date: not a kind of
//!   yesterday or the day before while there is a replacement, groups not
//!   yet on the board first, and the kinds not shown for longest before
//!   the rest. What was shown is kept in a [`TileHistory`].
//!
//! The recipes read the catalog and the taste, plus what a tile needs
//! beyond them ([`TilesInput`]): titles, durations, years, albums, lyrics,
//! when songs were added, which have a file, the playlists and the plays.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EventType, ReleaseId, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::daily::REDISCOVERY_MS;
use crate::pick::{Diversity, Item, Pick, Reason, Recs, Slot, arrange, select_diverse, token_of_artist};
use crate::rng::Rng;
use crate::taste::{DAY_MS, ON_REPEAT_MS, Play};

pub const TILE_LEN: usize = 30;
/// Fewer songs than this, no tile.
pub const TILE_MIN: usize = 8;
/// «Свежие лайки» need this many.
pub const FRESH_MIN: usize = 5;
/// Tiles on the board a day (with «На повторе»).
pub const TILES_PER_DAY: usize = 5;
/// A parameter is not shown again for this many days while another fits.
pub const PARAM_REST_DAYS: i64 = 7;
/// Days of history kept.
const HISTORY_DAYS: i64 = 28;
/// «На повторе»: completes of a song in the 30 days.
pub const ON_REPEAT_MIN_COMPLETES: u32 = 2;
/// «Забытое любимое»: a favourite has at least this affinity.
pub const FAVOURITE_AFFINITY: f32 = 1.0;
/// «Глубже у любимых»: this many most played artists…
pub const TOP_ARTISTS: usize = 10;
/// …and their songs played at most this many times.
pub const DEEP_MAX_PLAYS: u32 = 1;
/// «Исполнитель дня» is one of this many favourite artists.
pub const DAY_ARTISTS: usize = 15;
/// «Давно не слышал»: an artist with this many plays…
pub const COMEBACK_PLAYS: u32 = 15;
/// …none for this long.
pub const COMEBACK_MS: i64 = 45 * DAY_MS;
/// «Ждут своего часа»: added this long ago at least.
pub const UNPLAYED_AFTER_MS: i64 = 7 * DAY_MS;
/// «С самого начала»: likes older than this.
pub const FIRST_LIKES_AFTER_MS: i64 = 30 * DAY_MS;
/// «От 7 минут».
pub const LONG_MS: u32 = 7 * 60_000;
/// «До 2:30» (but not a short interlude).
pub const SHORT_MS: u32 = 150_000;
const SHORT_FROM_MS: u32 = 45_000;
/// «Из плейлиста»: a playlist none of whose songs played for this long.
pub const PLAYLIST_REST_MS: i64 = 14 * DAY_MS;
/// «Дослушай»: an album of at least this many songs…
pub const UNFINISHED_MIN_TRACKS: usize = 4;
/// …left this long ago.
pub const UNFINISHED_AFTER_MS: i64 = 7 * DAY_MS;
/// A language or a decade that is more than this share of the catalog
/// says nothing new.
const COMMON_SHARE: f32 = 0.8;
/// How many recent likes «Потому что тебе понравилось» may grow from.
const LIKE_SEEDS: usize = 10;
const LIKE_NEIGHBOURS: usize = 40;

/// The four groups of tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileGroup {
    Memory,
    Artists,
    Sound,
    Collection,
}

/// The kinds of tiles (the view's `TileKind`, in the same order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TileKind {
    OnRepeat,
    YearAgo,
    Season,
    Rediscover,
    FirstLikes,
    FreshLikes,
    Unplayed,
    DeepCuts,
    ArtistOfDay,
    Comeback,
    BecauseYouLiked,
    Long,
    Short,
    SingAlong,
    Language,
    Decade,
    Soundtrack,
    Playlist,
    Unfinished,
    NotDownloaded,
    Lottery,
}

impl TileKind {
    pub const ALL: [Self; 21] = [
        Self::OnRepeat,
        Self::YearAgo,
        Self::Season,
        Self::Rediscover,
        Self::FirstLikes,
        Self::FreshLikes,
        Self::Unplayed,
        Self::DeepCuts,
        Self::ArtistOfDay,
        Self::Comeback,
        Self::BecauseYouLiked,
        Self::Long,
        Self::Short,
        Self::SingAlong,
        Self::Language,
        Self::Decade,
        Self::Soundtrack,
        Self::Playlist,
        Self::Unfinished,
        Self::NotDownloaded,
        Self::Lottery,
    ];

    pub const fn group(self) -> TileGroup {
        match self {
            Self::OnRepeat
            | Self::YearAgo
            | Self::Season
            | Self::Rediscover
            | Self::FirstLikes
            | Self::FreshLikes
            | Self::Unplayed => TileGroup::Memory,
            Self::DeepCuts | Self::ArtistOfDay | Self::Comeback => TileGroup::Artists,
            Self::BecauseYouLiked | Self::Long | Self::Short | Self::SingAlong => TileGroup::Sound,
            Self::Language
            | Self::Decade
            | Self::Soundtrack
            | Self::Playlist
            | Self::Unfinished
            | Self::NotDownloaded
            | Self::Lottery => TileGroup::Collection,
        }
    }

    /// The quiet label over the tile's title: why the tile is here, never
    /// the title again (docs/stage4.md §12.10).
    pub const fn kicker(self) -> &'static str {
        match self {
            Self::OnRepeat => "Чаще всего",
            Self::YearAgo => "Год назад",
            Self::Season => "Капсула времени",
            Self::Rediscover => "Давно не играло",
            Self::FirstLikes => "С самого начала",
            Self::FreshLikes => "Недавно",
            Self::Unplayed => "Не играло",
            Self::DeepCuts => "Твои исполнители",
            Self::ArtistOfDay => "Исполнитель дня",
            Self::Comeback => "Давно не слышал",
            Self::BecauseYouLiked => "Тебе понравилось",
            Self::Long => "Длинное",
            Self::Short => "Короткое",
            Self::SingAlong => "Подпевай",
            Self::Language => "По языку",
            Self::Decade => "По годам",
            Self::Soundtrack => "Саундтрек",
            Self::Playlist => "Из плейлиста",
            Self::Unfinished => "Не дослушал",
            Self::NotDownloaded => "Не скачано",
            Self::Lottery => "Наугад",
        }
    }

    const fn min(self) -> usize {
        match self {
            Self::FreshLikes => FRESH_MIN,
            _ => TILE_MIN,
        }
    }
}

/// How much lyrics a song has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum LyricsKind {
    #[default]
    None,
    Plain,
    /// Synced by lines.
    Synced,
    /// Synced by words.
    Words,
}

/// What a tile knows of a song beyond the catalog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackFacts {
    pub title: String,
    /// The credit as shown.
    pub artist: String,
    pub duration_ms: Option<u32>,
    pub added_at: UnixMs,
    /// The release's year and title.
    pub year: Option<i32>,
    pub album: Option<String>,
    /// Disc × 1000 + number, for the album order.
    pub position: u32,
    pub lyrics: LyricsKind,
    /// Has a file (a liked stream song without one is «Ещё не скачано»).
    pub has_local: bool,
}

/// A playlist of the user's.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistFacts {
    /// Its id, the parameter of «Из плейлиста».
    pub key: String,
    pub name: String,
    pub tracks: Vec<TrackId>,
}

/// What the tiles read beyond the catalog and the taste.
#[derive(Clone, Debug, Default)]
pub struct TilesInput {
    pub facts: HashMap<TrackId, TrackFacts>,
    /// The listening history (as for the taste).
    pub events: Vec<Play>,
    pub playlists: Vec<PlaylistFacts>,
    pub artist_names: HashMap<ArtistId, String>,
}

/// «Похоже на Underground River»: a song's title without what its brackets
/// add («(Opening Theme)», «[Remastered]»).
pub fn like_title(song: &str) -> String {
    let mut short = song.to_owned();
    for open in ['(', '[', '（', '【'] {
        if let Some(i) = short.find(open)
            && i > 0
        {
            short.truncate(i);
        }
    }
    let short = short.trim();
    format!("Похоже на {}", if short.is_empty() { song.trim() } else { short })
}

/// One tile of a day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TileMix {
    pub kind: TileKind,
    /// What it was picked for (an artist id, «ja», «2000»…; empty for the
    /// kinds without a parameter).
    pub param: String,
    /// «На повторе», «Kevin Penkin», «Лето 2025».
    pub title: String,
    pub picks: Vec<Pick>,
}

/// The tiles shown on the last days: kinds and parameters by local day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TileHistory {
    pub days: Vec<(i64, Vec<(TileKind, String)>)>,
}

impl TileHistory {
    /// Before `day` only (a day built again does not see itself).
    fn before(&self, day: i64) -> impl Iterator<Item = &(i64, Vec<(TileKind, String)>)> {
        self.days.iter().filter(move |(d, _)| *d < day)
    }

    /// The last day before `day` a kind was shown.
    pub fn last_shown(&self, kind: TileKind, day: i64) -> Option<i64> {
        self.before(day).filter(|(_, t)| t.iter().any(|(k, _)| *k == kind)).map(|(d, _)| *d).max()
    }

    /// The last day before `day` a kind was shown with this parameter.
    pub fn param_last_shown(&self, kind: TileKind, param: &str, day: i64) -> Option<i64> {
        self.before(day)
            .filter(|(_, t)| t.iter().any(|(k, p)| *k == kind && p == param))
            .map(|(d, _)| *d)
            .max()
    }

    /// Keeps what `day` shows (replacing an earlier record of that day) and
    /// forgets days older than four weeks.
    pub fn record(&mut self, day: i64, tiles: &[TileMix]) {
        self.days.retain(|(d, _)| *d != day && day - *d < HISTORY_DAYS);
        self.days.push((day, tiles.iter().map(|t| (t.kind, t.param.clone())).collect()));
        self.days.sort_by_key(|(d, _)| *d);
    }
}

/// The scripts a song's name may be written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Script {
    Japanese,
    Korean,
    Cyrillic,
    Latin,
}

impl Script {
    const fn key(self) -> &'static str {
        match self {
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::Cyrillic => "ru",
            Self::Latin => "latin",
        }
    }

    const fn title(self) -> &'static str {
        match self {
            Self::Japanese => "Японское",
            Self::Korean => "Корейское",
            Self::Cyrillic => "По-русски",
            Self::Latin => "Зарубежное",
        }
    }
}

/// The script of a song by its title and artist: kana or kanji make it
/// Japanese, hangul Korean, Cyrillic Russian; any of them wins over Latin.
pub fn script_of(text: &str) -> Option<Script> {
    let (mut cjk, mut hangul, mut cyrillic, mut latin) = (0, 0, 0, 0);
    for c in text.chars() {
        match c as u32 {
            // Kana, kanji, the Japanese punctuation («〜», «「」») and the
            // fullwidth forms («！», «～»).
            0x3000..=0x30FF | 0x31F0..=0x31FF | 0xFF01..=0xFF5E | 0xFF66..=0xFF9F | 0x3400..=0x4DBF | 0x4E00..=0x9FFF => cjk += 1,
            0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => hangul += 1,
            0x0400..=0x04FF => cyrillic += 1,
            _ if c.is_ascii_alphabetic() || ('\u{C0}'..='\u{24F}').contains(&c) => latin += 1,
            _ => {}
        }
    }
    if cjk > 0 && cjk >= hangul {
        Some(Script::Japanese)
    } else if hangul > 0 {
        Some(Script::Korean)
    } else if cyrillic > 0 {
        Some(Script::Cyrillic)
    } else if latin > 0 {
        Some(Script::Latin)
    } else {
        None
    }
}

/// «Из 90-х», «Из 2000-х».
fn decade_title(decade: i32) -> String {
    if (1900..2000).contains(&decade) {
        format!("Из {}-х", decade - 1900)
    } else {
        format!("Из {decade}-х")
    }
}

/// Soundtrack markers in an album title (lower case), longest first.
const MARKERS: [&str; 9] = [
    "original motion picture soundtrack",
    "オリジナル・サウンドトラック",
    "オリジナルサウンドトラック",
    "original soundtrack",
    "original score",
    "サウンドトラック",
    "soundtrack",
    "саундтрек",
    "o.s.t",
];

/// Where «OST» stands as a word in `lower`.
fn ost_at(lower: &str) -> Option<usize> {
    lower.match_indices("ost").map(|(i, _)| i).find(|&i| {
        let before = lower[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = lower[i + 3..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

/// A soundtrack album's franchise: its key (lower case) and its name as
/// shown, the title without the soundtrack markers, brackets and a
/// trailing season or volume («Made in Abyss Original Soundtrack 2» →
/// «Made in Abyss»). `None` for an album not marked a soundtrack.
pub fn franchise(album: &str) -> Option<(String, String)> {
    let lower = album.to_lowercase();
    // Byte offsets of `lower` match `album` only while lowering keeps the
    // lengths; cut on characters to stay safe.
    let marker = MARKERS
        .iter()
        .filter_map(|m| lower.find(m))
        .chain(ost_at(&lower))
        .min()?;
    let chars_before = lower[..marker].chars().count();
    let mut name: String = album.chars().take(chars_before).collect();
    for sep in ["(", "[", "（", "「", " - ", " – ", ":", "：", "~", "〜"] {
        if let Some(i) = name.find(sep) {
            name.truncate(i);
        }
    }
    // A trailing season, part or volume: «… Season 2», «… Vol. 3», «… 2».
    loop {
        let trimmed = name.trim_end_matches(|c: char| !c.is_alphanumeric()).to_owned();
        let mut words: Vec<&str> = trimmed.split_whitespace().collect();
        let Some(last) = words.last().map(|w| w.to_lowercase()) else {
            name = trimmed;
            break;
        };
        let roman = !last.is_empty() && last.chars().all(|c| "ivx".contains(c)) && last.len() <= 3;
        let number = last.trim_end_matches('.').chars().all(|c| c.is_ascii_digit());
        if words.len() > 1 && (number || roman || ["season", "vol", "volume", "part", "the", "movie"].contains(&last.trim_end_matches('.'))) {
            words.pop();
            name = words.join(" ");
        } else {
            name = trimmed;
            break;
        }
    }
    let name = name.trim().to_owned();
    if name.is_empty() {
        return None;
    }
    Some((name.to_lowercase(), name))
}

/// The civil date of a day number (days since 1970-01-01).
pub fn civil(day: i64) -> (i32, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// The season of a date: the year it is named by (a winter by its
/// January) and 0 winter, 1 spring, 2 summer, 3 autumn.
pub fn season_of(year: i32, month: u32) -> (i32, u32) {
    match month {
        12 => (year + 1, 0),
        1 | 2 => (year, 0),
        3..=5 => (year, 1),
        6..=8 => (year, 2),
        _ => (year, 3),
    }
}

fn season_title((year, s): (i32, u32)) -> String {
    let name = ["Зима", "Весна", "Лето", "Осень"][s as usize % 4];
    format!("{name} {year}")
}

/// A recipe's candidates: catalog position and score (higher first).
type Cands = Vec<(usize, f32)>;

/// Up to [`TILE_LEN`] of `cands` (best scored first) under the diversity
/// rules in the smooth order, from the best scored one.
fn smooth_picks(recs: &Recs<'_>, kind: TileKind, mut cands: Cands, exempt: Option<ArtistId>) -> Vec<Pick> {
    cands.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let slots: Vec<Slot> = cands.into_iter().map(|(i, s)| recs.slot(i, Reason::Tile(kind), s)).collect();
    let rules = Diversity {
        exempt: exempt.map(token_of_artist),
        ..Diversity::default()
    };
    let picked = select_diverse(slots, TILE_LEN, &rules, &mut HashSet::new());
    arrange(recs, picked, &[], None, &rules).into_iter().map(Slot::pick).collect()
}

/// One day's tiles over one snapshot.
struct Board<'r, 'a> {
    recs: &'r Recs<'a>,
    input: &'r TilesInput,
    history: &'r TileHistory,
    day: i64,
    now: UnixMs,
    utc_offset_min: i32,
    /// By catalog position.
    facts: Vec<Option<&'r TrackFacts>>,
    completes: Vec<u32>,
    /// Plays (a start, or a complete without one): position and time.
    plays: Vec<(usize, UnixMs)>,
}

impl<'r, 'a> Board<'r, 'a> {
    fn new(recs: &'r Recs<'a>, input: &'r TilesInput, history: &'r TileHistory) -> Self {
        let n = recs.catalog.len();
        let utc_offset_min = recs.taste.utc_offset_min();
        let facts = recs.catalog.tracks().iter().map(|t| input.facts.get(&t.id)).collect();
        let mut events: Vec<(UnixMs, usize, EventType)> = input
            .events
            .iter()
            .filter(|p| matches!(p.kind, EventType::Start | EventType::Complete))
            .filter_map(|p| Some((p.ts, recs.catalog.position(p.track)?, p.kind)))
            .collect();
        events.sort_unstable_by_key(|e| (e.0, e.1, e.2 == EventType::Complete));
        let mut completes = vec![0u32; n];
        let mut open = vec![false; n];
        let mut plays = Vec::with_capacity(events.len());
        for (ts, i, kind) in events {
            if kind == EventType::Start {
                open[i] = true;
                plays.push((i, ts));
            } else {
                completes[i] += 1;
                if !std::mem::take(&mut open[i]) {
                    plays.push((i, ts));
                }
            }
        }
        Self {
            recs,
            input,
            history,
            day: recs.local_day(),
            now: recs.now(),
            utc_offset_min,
            facts,
            completes,
            plays,
        }
    }

    fn ok(&self, i: usize) -> bool {
        !self.recs.blocked(i)
    }

    fn aff(&self, i: usize) -> f32 {
        self.recs.taste.affinity_at(i)
    }

    /// Affinity squeezed into 0…1 (a like is about 0.76).
    fn love(&self, i: usize) -> f32 {
        (self.aff(i) / 3.0).tanh()
    }

    fn fact(&self, i: usize) -> Option<&'r TrackFacts> {
        self.facts.get(i).copied().flatten()
    }

    fn rng(&self, kind: TileKind, salt: u64) -> Rng {
        Rng::derive(self.day as u64 ^ salt.wrapping_mul(0x9E37_79B9), 0x711E + kind as u64)
    }

    fn local_day(&self, ts: UnixMs) -> i64 {
        (ts + i64::from(self.utc_offset_min) * 60_000).div_euclid(DAY_MS)
    }

    /// Every catalog position that may be picked.
    fn all(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.recs.catalog.len()).filter(|&i| self.ok(i))
    }

    fn artist_name(&self, a: ArtistId) -> String {
        self.input.artist_names.get(&a).cloned().unwrap_or_default()
    }

    /// The tile of `cands` under the diversity rules in the smooth order
    /// (`exempt`: an artist the artist rule does not apply to).
    fn smooth(&self, kind: TileKind, param: String, title: String, cands: Cands, exempt: Option<ArtistId>) -> Option<TileMix> {
        if cands.len() < kind.min() {
            return None;
        }
        let picks = smooth_picks(self.recs, kind, cands, exempt);
        Some(TileMix { kind, param, title, picks })
    }

    /// The tile of `order` as it is.
    fn as_is(&self, kind: TileKind, param: String, title: String, order: Vec<usize>) -> Option<TileMix> {
        if order.len() < kind.min() {
            return None;
        }
        let picks = order
            .into_iter()
            .take(TILE_LEN)
            .map(|i| Pick {
                item: Item::Track(self.recs.info(i).id),
                reason: Reason::Tile(kind),
            })
            .collect();
        Some(TileMix { kind, param, title, picks })
    }

    /// The parameter of the day among `options` (best first, each enough
    /// for a tile): one not shown within [`PARAM_REST_DAYS`], by chance
    /// among the first three of those; else the one shown longest ago.
    fn choose<T>(&self, kind: TileKind, options: Vec<(String, T)>) -> Option<(String, T)> {
        let last = |p: &str| self.history.param_last_shown(kind, p, self.day);
        let (fresh, used): (Vec<_>, Vec<_>) =
            options.into_iter().partition(|(p, _)| last(p).is_none_or(|d| self.day - d >= PARAM_REST_DAYS));
        if !fresh.is_empty() {
            let k = self.rng(kind, 1).below(fresh.len().min(3));
            return fresh.into_iter().nth(k);
        }
        used.into_iter().min_by_key(|(p, _)| last(p))
    }

    fn make(&self, kind: TileKind) -> Option<TileMix> {
        let none = String::new;
        match kind {
            TileKind::OnRepeat => {
                let cands = self
                    .all()
                    .filter_map(|i| {
                        // What plays on repeat now first (the hot affinity).
                        let c = self.recs.taste.recent_completes_at(i);
                        (c >= ON_REPEAT_MIN_COMPLETES).then(|| (i, self.recs.taste.hot_at(i) + 0.01 * c as f32))
                    })
                    .collect();
                self.smooth(kind, none(), "На повторе".into(), cands, None)
            }
            TileKind::YearAgo => {
                let cands = self
                    .all()
                    .filter_map(|i| {
                        let p = self.recs.taste.year_ago_plays_at(i);
                        (p > 0).then(|| (i, p as f32 + 0.01 * self.aff(i)))
                    })
                    .collect();
                self.smooth(kind, none(), "Год назад".into(), cands, None)
            }
            TileKind::Season => self.season(),
            TileKind::Rediscover => {
                let cands = self
                    .all()
                    .filter(|&i| self.aff(i) >= FAVOURITE_AFFINITY)
                    .filter(|&i| {
                        // A like never played since is forgotten only when old.
                        let last = self.recs.taste.last_played_at(i).or(self.recs.taste.liked_time_at(i));
                        last.is_some_and(|t| self.now - t >= REDISCOVERY_MS)
                    })
                    .map(|i| (i, self.aff(i)))
                    .collect();
                self.smooth(kind, none(), "Забытое любимое".into(), cands, None)
            }
            TileKind::FirstLikes => {
                let mut liked: Vec<(usize, UnixMs)> = self
                    .all()
                    .filter_map(|i| Some((i, self.recs.taste.liked_time_at(i)?)))
                    .filter(|(_, t)| self.now - t >= FIRST_LIKES_AFTER_MS)
                    .collect();
                liked.sort_by_key(|(i, t)| (*t, *i));
                self.as_is(kind, none(), "Первые лайки".into(), liked.into_iter().map(|(i, _)| i).collect())
            }
            TileKind::FreshLikes => {
                let mut liked: Vec<(usize, UnixMs)> = self
                    .all()
                    .filter_map(|i| Some((i, self.recs.taste.liked_time_at(i)?)))
                    .filter(|(_, t)| self.now - t < ON_REPEAT_MS)
                    .collect();
                liked.sort_by_key(|(i, t)| (std::cmp::Reverse(*t), *i));
                self.as_is(kind, none(), "Свежие лайки".into(), liked.into_iter().map(|(i, _)| i).collect())
            }
            TileKind::Unplayed => {
                let mut rng = self.rng(kind, 0);
                let cands = self
                    .all()
                    .filter(|&i| self.recs.taste.plays_at(i) == 0 && self.completes[i] == 0)
                    .filter(|&i| self.fact(i).is_some_and(|f| self.now - f.added_at >= UNPLAYED_AFTER_MS))
                    .map(|i| (i, rng.f32()))
                    .collect();
                self.smooth(kind, none(), "Ждут своего часа".into(), cands, None)
            }
            TileKind::DeepCuts => self.deep_cuts(),
            TileKind::ArtistOfDay => self.artist_of_day(),
            TileKind::Comeback => self.comeback(),
            TileKind::BecauseYouLiked => self.because_you_liked(),
            TileKind::Long | TileKind::Short => {
                let fits = |ms: u32| match kind {
                    TileKind::Long => ms >= LONG_MS,
                    _ => (SHORT_FROM_MS..=SHORT_MS).contains(&ms),
                };
                let tracks = self.all().filter(|&i| self.fact(i).and_then(|f| f.duration_ms).is_some_and(fits)).collect();
                let title = if kind == TileKind::Long { "Длинные треки" } else { "Короткие треки" };
                self.smooth(kind, none(), title.into(), self.by_love(kind, tracks), None)
            }
            TileKind::SingAlong => {
                let mut rng = self.rng(kind, 0);
                let cands = self
                    .all()
                    .filter_map(|i| {
                        let words = match self.fact(i)?.lyrics {
                            LyricsKind::Words => 1.0,
                            LyricsKind::Synced => 0.0,
                            LyricsKind::Plain | LyricsKind::None => return None,
                        };
                        Some((i, words + self.love(i) + 0.5 * rng.f32()))
                    })
                    .collect();
                self.smooth(kind, none(), "Со словами".into(), cands, None)
            }
            TileKind::Language => self.language(),
            TileKind::Decade => self.decade(),
            TileKind::Soundtrack => self.soundtrack(),
            TileKind::Playlist => self.playlist(),
            TileKind::Unfinished => self.unfinished(),
            // Never while only songs with a file are picked.
            TileKind::NotDownloaded if self.recs.local_only => None,
            TileKind::NotDownloaded => {
                let mut liked: Vec<(usize, UnixMs)> = self
                    .all()
                    .filter(|&i| self.recs.taste.liked_at(i) && self.fact(i).is_some_and(|f| !f.has_local))
                    .map(|i| (i, self.recs.taste.liked_time_at(i).unwrap_or(0)))
                    .collect();
                liked.sort_by_key(|(i, t)| (std::cmp::Reverse(*t), *i));
                self.as_is(kind, none(), "Ещё не скачано".into(), liked.into_iter().map(|(i, _)| i).collect())
            }
            TileKind::Lottery => {
                // The board's draw only says the tile can play; a click
                // draws again ([`Recs::lottery`]).
                let picks = self.recs.lottery(self.day as u64 ^ 0x1077);
                (picks.len() >= kind.min()).then(|| TileMix {
                    kind,
                    param: none(),
                    title: "Лотерея".into(),
                    picks,
                })
            }
        }
    }

    /// Loved first, mixed a little by the day.
    fn by_love(&self, kind: TileKind, tracks: Vec<usize>) -> Cands {
        let mut rng = self.rng(kind, 3);
        tracks.into_iter().map(|i| (i, self.love(i) + 0.5 * rng.f32())).collect()
    }

    /// «Лето 2025»: the most played of a past season (not the current one).
    fn season(&self) -> Option<TileMix> {
        let season_at = |day: i64| {
            let (y, m, _) = civil(day);
            season_of(y, m)
        };
        let current = season_at(self.day);
        let mut by_season: HashMap<(i32, u32), HashMap<usize, u32>> = HashMap::new();
        for &(i, ts) in &self.plays {
            let s = season_at(self.local_day(ts));
            if s != current && self.ok(i) {
                *by_season.entry(s).or_default().entry(i).or_default() += 1;
            }
        }
        let mut seasons: Vec<((i32, u32), HashMap<usize, u32>)> =
            by_season.into_iter().filter(|(_, t)| t.len() >= TILE_MIN).collect();
        // The latest first.
        seasons.sort_by_key(|s| std::cmp::Reverse(s.0));
        let options = seasons.into_iter().map(|(s, t)| (format!("{}-{}", s.0, s.1), (s, t))).collect();
        let (param, (s, tracks)) = self.choose(TileKind::Season, options)?;
        let cands = tracks.into_iter().map(|(i, n)| (i, n as f32 + 0.01 * self.aff(i))).collect();
        self.smooth(TileKind::Season, param, season_title(s), cands, None)
    }

    /// Plays of each artist (any credit) over the catalog.
    fn artist_plays(&self) -> HashMap<ArtistId, u32> {
        let mut plays: HashMap<ArtistId, u32> = HashMap::new();
        for i in 0..self.recs.catalog.len() {
            let p = self.recs.taste.plays_at(i);
            if p > 0 {
                for a in &self.recs.info(i).artists {
                    *plays.entry(*a).or_default() += p;
                }
            }
        }
        plays
    }

    /// An artist's songs that may be picked.
    fn of_artist(&self, a: ArtistId) -> Vec<usize> {
        self.recs.catalog.of_artist(a).iter().map(|&i| i as usize).filter(|&i| self.ok(i)).collect()
    }

    /// An artist who may have a tile of their own.
    fn tile_artist(&self, a: ArtistId) -> bool {
        !self.recs.taste.is_disliked_artist(a) && self.of_artist(a).len() >= TILE_MIN && !self.artist_name(a).is_empty()
    }

    fn deep_cuts(&self) -> Option<TileMix> {
        let mut top: Vec<(ArtistId, u32)> = self
            .artist_plays()
            .into_iter()
            .filter(|(a, _)| !self.recs.taste.is_disliked_artist(*a))
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        top.truncate(TOP_ARTISTS);
        let mut rng = self.rng(TileKind::DeepCuts, 0);
        let mut seen: HashSet<usize> = HashSet::new();
        let mut cands = Vec::new();
        for (rank, (a, _)) in top.iter().enumerate() {
            for i in self.of_artist(*a) {
                if self.recs.taste.plays_at(i) <= DEEP_MAX_PLAYS && self.aff(i) >= 0.0 && seen.insert(i) {
                    // The top artists first, mixed a little by the day.
                    cands.push((i, -(rank as f32) / TOP_ARTISTS as f32 + rng.f32()));
                }
            }
        }
        self.smooth(TileKind::DeepCuts, String::new(), "Глубже у любимых".into(), cands, None)
    }

    /// An artist's tile: their songs by affinity.
    fn artist_tile(&self, kind: TileKind, a: ArtistId) -> Option<TileMix> {
        let mut rng = self.rng(kind, 2);
        let cands: Cands = self.of_artist(a).into_iter().map(|i| (i, self.love(i) + 0.1 * rng.f32())).collect();
        self.smooth(kind, a.to_string(), self.artist_name(a), cands, Some(a))
    }

    fn artist_of_day(&self) -> Option<TileMix> {
        let mut top: Vec<(ArtistId, f32)> = self
            .recs
            .taste
            .artist_affinities()
            .filter(|(a, s)| *s > 0.0 && self.tile_artist(*a))
            .collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        top.truncate(DAY_ARTISTS);
        let options = top.into_iter().map(|(a, _)| (a.to_string(), a)).collect();
        let (_, a) = self.choose(TileKind::ArtistOfDay, options)?;
        self.artist_tile(TileKind::ArtistOfDay, a)
    }

    fn comeback(&self) -> Option<TileMix> {
        let mut last: HashMap<ArtistId, UnixMs> = HashMap::new();
        for i in 0..self.recs.catalog.len() {
            if let Some(t) = self.recs.taste.last_played_at(i) {
                for a in &self.recs.info(i).artists {
                    let e = last.entry(*a).or_insert(t);
                    *e = (*e).max(t);
                }
            }
        }
        let mut cands: Vec<(ArtistId, u32)> = self
            .artist_plays()
            .into_iter()
            .filter(|(a, p)| *p >= COMEBACK_PLAYS && self.tile_artist(*a))
            .filter(|(a, _)| last.get(a).is_some_and(|t| self.now - t >= COMEBACK_MS))
            .collect();
        cands.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let options = cands.into_iter().map(|(a, _)| (a.to_string(), a)).collect();
        let (_, a) = self.choose(TileKind::Comeback, options)?;
        self.artist_tile(TileKind::Comeback, a)
    }

    /// What grows from a like: its sound neighbours, then the favourites
    /// of the artists played in the same sessions; the like first.
    fn grown_from(&self, seed: usize) -> Cands {
        let id = self.recs.info(seed).id;
        let mut out: Cands = vec![(seed, 10.0)];
        let mut seen: HashSet<usize> = HashSet::from([seed]);
        let near = self
            .recs
            .index
            .neighbours(id, LIKE_NEIGHBOURS, |k| self.recs.catalog.position(k).is_some_and(|i| self.ok(i)));
        for (rank, (k, _)) in near.into_iter().enumerate() {
            let i = self.recs.catalog.position(k).expect("filtered");
            if seen.insert(i) {
                out.push((i, 2.0 - rank as f32 / LIKE_NEIGHBOURS as f32 + 0.3 * self.love(i)));
            }
        }
        if let Some(a) = self.recs.info(seed).main_artist() {
            for &(b, _) in self.recs.taste.co_listened(a).iter().take(5) {
                let mut theirs = self.of_artist(b);
                theirs.sort_by(|x, y| self.aff(*y).total_cmp(&self.aff(*x)).then(x.cmp(y)));
                for i in theirs.into_iter().take(4) {
                    if seen.insert(i) {
                        out.push((i, 0.5 + 0.3 * self.love(i)));
                    }
                }
            }
        }
        out
    }

    fn because_you_liked(&self) -> Option<TileMix> {
        let mut liked: Vec<(usize, UnixMs)> = self.all().filter_map(|i| Some((i, self.recs.taste.liked_time_at(i)?))).collect();
        liked.sort_by_key(|(i, t)| (std::cmp::Reverse(*t), *i));
        let options: Vec<(String, (usize, Cands))> = liked
            .into_iter()
            .take(LIKE_SEEDS)
            .map(|(i, _)| (i, self.grown_from(i)))
            .filter(|(_, c)| c.len() >= TILE_MIN)
            .map(|(i, c)| (self.recs.info(i).id.to_string(), (i, c)))
            .collect();
        let (param, (seed, cands)) = self.choose(TileKind::BecauseYouLiked, options)?;
        let title = like_title(&self.fact(seed).map(|f| f.title.clone()).unwrap_or_default());
        self.smooth(TileKind::BecauseYouLiked, param, title, cands, None)
    }

    /// Songs grouped by a key: the groups of [`TILE_MIN`] or more that are
    /// not nearly the whole catalog, biggest first.
    fn groups<K: std::hash::Hash + Eq + Ord + Copy>(&self, key: impl Fn(usize) -> Option<K>) -> Vec<(K, Vec<usize>)> {
        let mut by: HashMap<K, Vec<usize>> = HashMap::new();
        let mut total = 0usize;
        for i in self.all() {
            if let Some(k) = key(i) {
                by.entry(k).or_default().push(i);
                total += 1;
            }
        }
        let mut groups: Vec<(K, Vec<usize>)> = by
            .into_iter()
            .filter(|(_, v)| v.len() >= TILE_MIN && (v.len() as f32) <= COMMON_SHARE * total as f32)
            .collect();
        groups.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        groups
    }

    fn language(&self) -> Option<TileMix> {
        let groups = self.groups(|i| self.fact(i).and_then(|f| script_of(&format!("{} {}", f.title, f.artist))));
        let options = groups.into_iter().map(|(s, v)| (s.key().to_owned(), (s, v))).collect();
        let (param, (script, tracks)) = self.choose(TileKind::Language, options)?;
        let cands = self.by_love(TileKind::Language, tracks);
        self.smooth(TileKind::Language, param, script.title().into(), cands, None)
    }

    fn decade(&self) -> Option<TileMix> {
        let groups = self.groups(|i| self.fact(i).and_then(|f| f.year).filter(|y| *y >= 1900).map(|y| y / 10 * 10));
        let options = groups.into_iter().map(|(d, v)| (d.to_string(), (d, v))).collect();
        let (param, (decade, tracks)) = self.choose(TileKind::Decade, options)?;
        let cands = self.by_love(TileKind::Decade, tracks);
        self.smooth(TileKind::Decade, param, decade_title(decade), cands, None)
    }

    fn soundtrack(&self) -> Option<TileMix> {
        // Franchises of the marked albums; a longer key that starts with a
        // shorter one joins it («made in abyss dawn…» → «made in abyss»).
        let mut names: HashMap<String, String> = HashMap::new();
        let mut of_track: Vec<(usize, String)> = Vec::new();
        let mut known: HashMap<&str, Option<(String, String)>> = HashMap::new();
        for i in self.all() {
            let Some(album) = self.fact(i).and_then(|f| f.album.as_deref()) else { continue };
            if let Some((key, name)) = known.entry(album).or_insert_with(|| franchise(album)).clone() {
                names.entry(key.clone()).or_insert(name);
                of_track.push((i, key));
            }
        }
        let mut keys: Vec<String> = names.keys().cloned().collect();
        keys.sort_by_key(|k| (k.len(), k.clone()));
        let root = |k: &str| -> String {
            keys.iter()
                .find(|r| k == r.as_str() || k.starts_with(&format!("{r} ")))
                .cloned()
                .unwrap_or_else(|| k.to_owned())
        };
        let mut by: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, k) in of_track {
            by.entry(root(&k)).or_default().push(i);
        }
        let mut groups: Vec<(String, Vec<usize>, f32)> = by
            .into_iter()
            .filter(|(_, v)| v.len() >= TILE_MIN)
            .map(|(k, v)| {
                let love = v.iter().map(|&i| self.aff(i).max(0.0)).sum();
                (k, v, love)
            })
            .collect();
        groups.sort_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));
        let options = groups.into_iter().map(|(k, v, _)| (k.clone(), (k, v))).collect();
        let (param, (key, tracks)) = self.choose(TileKind::Soundtrack, options)?;
        let title = names.get(&key).cloned().unwrap_or(key);
        let cands = self.by_love(TileKind::Soundtrack, tracks);
        self.smooth(TileKind::Soundtrack, param, title, cands, None)
    }

    /// The best of a playlist none of whose songs played for two weeks,
    /// shuffled.
    fn playlist(&self) -> Option<TileMix> {
        // A playlist, its songs and how much they are loved.
        type Candidate<'p> = (String, (&'p PlaylistFacts, Vec<usize>, f32));
        let mut options: Vec<Candidate<'_>> = Vec::new();
        for p in &self.input.playlists {
            let mut tracks: Vec<usize> = p.tracks.iter().filter_map(|t| self.recs.catalog.position(*t)).filter(|&i| self.ok(i)).collect();
            tracks.sort_unstable();
            tracks.dedup();
            let rested = tracks
                .iter()
                .all(|&i| self.recs.taste.last_played_at(i).is_none_or(|t| self.now - t >= PLAYLIST_REST_MS));
            if tracks.len() >= TILE_MIN && rested {
                let love = tracks.iter().map(|&i| self.aff(i).max(0.0)).sum();
                options.push((p.key.clone(), (p, tracks, love)));
            }
        }
        options.sort_by(|a, b| b.1.2.total_cmp(&a.1.2).then(a.0.cmp(&b.0)));
        let (param, (p, mut tracks, _)) = self.choose(TileKind::Playlist, options)?;
        tracks.sort_by(|a, b| self.aff(*b).total_cmp(&self.aff(*a)).then(a.cmp(b)));
        tracks.truncate(TILE_LEN);
        self.rng(TileKind::Playlist, 4).shuffle(&mut tracks);
        self.as_is(TileKind::Playlist, param, p.name.clone(), tracks)
    }

    /// Albums started and left with less than half completed: their songs
    /// not completed yet, album by album in album order, the one played
    /// last first.
    fn unfinished(&self) -> Option<TileMix> {
        let mut last: HashMap<usize, UnixMs> = HashMap::new();
        for &(i, ts) in &self.plays {
            let e = last.entry(i).or_insert(ts);
            *e = (*e).max(ts);
        }
        let mut albums: HashMap<ReleaseId, Vec<usize>> = HashMap::new();
        for (i, t) in self.recs.catalog.tracks().iter().enumerate() {
            if let Some(r) = t.release {
                albums.entry(r).or_default().push(i);
            }
        }
        let mut left: Vec<(UnixMs, Vec<usize>)> = Vec::new();
        for (_, mut tracks) in albums {
            if tracks.len() < UNFINISHED_MIN_TRACKS {
                continue;
            }
            let done = tracks.iter().filter(|&&i| self.completes[i] > 0).count();
            let Some(played) = tracks.iter().filter_map(|i| last.get(i)).max().copied() else { continue };
            if done * 2 >= tracks.len() || self.now - played < UNFINISHED_AFTER_MS {
                continue;
            }
            tracks.sort_by_key(|&i| (self.fact(i).map_or(0, |f| f.position), i));
            let rest: Vec<usize> = tracks.into_iter().filter(|&i| self.completes[i] == 0 && self.ok(i)).collect();
            left.push((played, rest));
        }
        left.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let order: Vec<usize> = left.into_iter().flat_map(|(_, t)| t).collect();
        self.as_is(TileKind::Unfinished, String::new(), "Недослушанные альбомы".into(), order)
    }
}

impl Recs<'_> {
    /// The local day number of now (days since 1970-01-01).
    pub fn local_day(&self) -> i64 {
        (self.now() + i64::from(self.taste.utc_offset_min()) * 60_000).div_euclid(DAY_MS)
    }

    /// «Наугад»: up to [`TILE_LEN`] songs drawn by chance, the rarely played
    /// likelier (weight 1 / (1 + plays)), under the diversity rules in the
    /// smooth order from the first one drawn. The runtime passes a fresh
    /// `seed` at every play, so neither the songs nor the first of them can
    /// be known before.
    pub fn lottery(&self, seed: u64) -> Vec<Pick> {
        let pool: Vec<(usize, f64)> = (0..self.catalog.len())
            .filter(|&i| !self.blocked(i))
            .map(|i| (i, 1.0 / (1.0 + f64::from(self.taste.plays_at(i)))))
            .collect();
        let drawn = Rng::derive(seed, 0x711E + TileKind::Lottery as u64).weighted_sample(&pool, TILE_LEN * 2);
        let n = drawn.len() as f32;
        let cands = drawn.into_iter().enumerate().map(|(k, i)| (i, n - k as f32)).collect();
        smooth_picks(self, TileKind::Lottery, cands, None)
    }

    /// A tile of a kind as it would be today (its parameter chosen against
    /// `history`), or `None` when it has too few songs.
    pub fn tile(&self, kind: TileKind, input: &TilesInput, history: &TileHistory) -> Option<TileMix> {
        Board::new(self, input, history).make(kind)
    }

    /// Every kind that has enough songs today.
    pub fn ready_tiles(&self, input: &TilesInput, history: &TileHistory) -> Vec<TileMix> {
        let board = Board::new(self, input, history);
        TileKind::ALL.into_iter().filter_map(|k| board.make(k)).collect()
    }

    /// The board's tiles today: «На повторе» first when it qualifies, then
    /// the others up to `n` in all (see the module docs).
    pub fn tiles_of_day(&self, input: &TilesInput, history: &TileHistory, n: usize) -> Vec<TileMix> {
        let day = self.local_day();
        let mut ready = self.ready_tiles(input, history);
        let mut out: Vec<TileMix> = Vec::new();
        if let Some(i) = ready.iter().position(|t| t.kind == TileKind::OnRepeat) {
            out.push(ready.remove(i));
        }
        let recent = |k: TileKind| history.last_shown(k, day).is_some_and(|d| day - d <= 2);
        let mut groups: HashSet<TileGroup> = HashSet::new();
        while out.len() < n && !ready.is_empty() {
            let fresh: Vec<usize> = (0..ready.len()).filter(|&i| !recent(ready[i].kind)).collect();
            let pool: Vec<usize> = if fresh.is_empty() { (0..ready.len()).collect() } else { fresh };
            let new_group: Vec<usize> = pool.iter().copied().filter(|&i| !groups.contains(&ready[i].kind.group())).collect();
            let pool = if new_group.is_empty() { pool } else { new_group };
            // The longest unseen first, by chance among equals.
            let score = |i: usize| {
                let k = ready[i].kind;
                let rested = history.last_shown(k, day).map_or(30, |d| (day - d).min(30)) as f32;
                rested + 3.0 * Rng::derive(day as u64, 0x5E1 + k as u64).f32()
            };
            let best = pool.into_iter().max_by(|a, b| score(*a).total_cmp(&score(*b))).expect("pool is not empty");
            let t = ready.remove(best);
            groups.insert(t.kind.group());
            out.push(t);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DIM;
    use crate::index::VectorIndex;
    use crate::pick::assert_diverse;
    use crate::taste::{Catalog, Taste, TasteInput, TrackInfo, YEAR_MS};
    use crate::test_util::*;

    /// 2026-10-22, a Thursday, noon UTC.
    const NOW: i64 = 20_748 * DAY_MS + 12 * 3_600_000;

    fn complete(i: u128, ts: i64) -> Play {
        Play {
            track: tid(i),
            ts,
            kind: EventType::Complete,
            position_ms: None,
            context: Default::default(),
        }
    }

    fn num(p: &Pick) -> u128 {
        match &p.item {
            Item::Track(id) => id.0.as_u128(),
            Item::Stream(_) => u128::MAX,
        }
    }

    fn ids(picks: &[Pick]) -> Vec<u128> {
        picks.iter().map(num).collect()
    }

    fn sorted(picks: &[Pick]) -> Vec<u128> {
        let mut v = ids(picks);
        v.sort_unstable();
        v
    }

    /// 300 tracks by `artists` artists (own releases), random vectors,
    /// facts that fit no tile in particular.
    fn catalog(artists: u32) -> (Catalog, VectorIndex<TrackId>, TilesInput) {
        let cat = Catalog::new((0..300u128).map(|i| info(i, &[(i % u128::from(artists)) as u32], Some(9_000 + i))));
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        let mut rng = Rng::new(5);
        for i in 0..300u128 {
            idx.insert(tid(i), std::array::from_fn(|_| rng.f32()));
        }
        let facts = (0..300u128)
            .map(|i| {
                let f = TrackFacts {
                    title: format!("Song {i}"),
                    artist: format!("Artist {}", i % u128::from(artists)),
                    duration_ms: Some(200_000),
                    added_at: NOW,
                    has_local: true,
                    ..TrackFacts::default()
                };
                (tid(i), f)
            })
            .collect();
        let artist_names = (0..artists).map(|a| (aid(a), format!("Artist {a}"))).collect();
        (cat, idx, TilesInput { facts, artist_names, ..TilesInput::default() })
    }

    /// A tile over `taste` (whose events the tiles read too).
    fn tile(cat: &Catalog, idx: &VectorIndex<TrackId>, taste: &TasteInput, input: &TilesInput, kind: TileKind) -> Option<TileMix> {
        let input = TilesInput { events: taste.events.clone(), ..input.clone() };
        let taste = Taste::build(cat, taste, NOW);
        Recs::new(cat, &taste, idx).tile(kind, &input, &TileHistory::default())
    }

    /// Adds tracks to a catalog (with default facts and a vector each).
    fn grow(cat: &Catalog, idx: &mut VectorIndex<TrackId>, input: &mut TilesInput, more: Vec<(TrackInfo, TrackFacts)>) -> Catalog {
        let mut tracks: Vec<TrackInfo> = cat.tracks().to_vec();
        let mut rng = Rng::new(tracks.len() as u64);
        for (t, f) in more {
            idx.insert(t.id, std::array::from_fn(|_| rng.f32()));
            input.facts.insert(t.id, f);
            tracks.push(t);
        }
        Catalog::new(tracks)
    }

    #[test]
    fn kinds_groups_and_dates() {
        assert_eq!(TileKind::ALL.len(), 21);
        let groups: HashSet<TileGroup> = TileKind::ALL.iter().map(|k| k.group()).collect();
        assert_eq!(groups.len(), 4);
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_748), (2026, 10, 22));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(season_of(2024, 12), (2025, 0));
        assert_eq!(season_title(season_of(2025, 7)), "Лето 2025");
        assert_eq!(season_title(season_of(2025, 1)), "Зима 2025");
        assert_eq!(decade_title(1990), "Из 90-х");
        assert_eq!(decade_title(2000), "Из 2000-х");
    }

    #[test]
    fn scripts_are_told_apart() {
        assert_eq!(script_of("すずめ RADWIMPS"), Some(Script::Japanese));
        assert_eq!(script_of("進撃の巨人 Hiroyuki Sawano"), Some(Script::Japanese));
        assert_eq!(script_of("봄날 BTS"), Some(Script::Korean));
        assert_eq!(script_of("Группа крови Кино"), Some(Script::Cyrillic));
        assert_eq!(script_of("Teardrop Massive Attack"), Some(Script::Latin));
        assert_eq!(script_of("Café Tacvba"), Some(Script::Latin));
        assert_eq!(script_of("1999 ..."), None);
        // Japanese punctuation and fullwidth forms in a Latin title.
        assert_eq!(script_of("A Horny Money World 〜legendary night〜"), Some(Script::Japanese));
        assert_eq!(script_of("Idol「Oshi no Ko」"), Some(Script::Japanese));
        assert_eq!(script_of("Ｂｒａｖｅ！"), Some(Script::Japanese));
    }

    #[test]
    fn franchises_come_from_soundtrack_titles() {
        let name = |t: &str| franchise(t).map(|(_, n)| n);
        assert_eq!(franchise("Made in Abyss Original Soundtrack"), Some(("made in abyss".into(), "Made in Abyss".into())));
        assert_eq!(franchise("Made in Abyss: The Golden City OST 2").unwrap().0, "made in abyss");
        assert_eq!(name("Attack on Titan Season 2 (Original Soundtrack)").unwrap(), "Attack on Titan");
        assert_eq!(name("メイドインアビス オリジナルサウンドトラック").unwrap(), "メイドインアビス");
        assert_eq!(name("Hollow Knight (OST)").unwrap(), "Hollow Knight");
        assert_eq!(name("Interstellar - Original Motion Picture Soundtrack").unwrap(), "Interstellar");
        assert_eq!(name("Celeste Original Soundtrack Vol. 2").unwrap(), "Celeste");
        assert_eq!(name("Ghost of Tsushima"), None, "not marked");
        assert_eq!(name("Hostage"), None, "«ost» inside a word is not a marker");
    }

    #[test]
    fn memory_tiles() {
        let (cat, idx, input) = catalog(60);
        let mut events = Vec::new();
        // On repeat: 0..12 completed three times this month.
        for i in 0..12u128 {
            for k in 0..3 {
                events.push(complete(i, NOW - (1 + k) * DAY_MS));
            }
        }
        // A year ago (autumn 2025): 20..30.
        events.extend((20..30u128).map(|i| complete(i, NOW - YEAR_MS)));
        // Summer 2025: 40..52, twice each.
        let summer = NOW - 450 * DAY_MS;
        for i in 40..52u128 {
            events.push(complete(i, summer));
            events.push(complete(i, summer + DAY_MS));
        }
        // 100..115 liked long ago (100 the earliest), 115..130 lately.
        let likes: Vec<TrackId> = (100..130u128).map(tid).collect();
        let like_times = (100..130u128)
            .map(|i| (tid(i), if i < 115 { NOW - (400 - i as i64) * DAY_MS } else { NOW - (i as i64 - 114) * DAY_MS }))
            .collect();
        let taste = TasteInput { events, likes, like_times, disliked_tracks: vec![tid(5)], ..Default::default() };
        let get = |k| tile(&cat, &idx, &taste, &input, k);
        assert_eq!(get(TileKind::OnRepeat).unwrap().picks.len(), 11, "the disliked one is out");
        assert_eq!(get(TileKind::YearAgo).unwrap().picks.len(), 10);
        let season = get(TileKind::Season).unwrap();
        match season.title.as_str() {
            "Лето 2025" => assert_eq!(sorted(&season.picks), (40..52).collect::<Vec<_>>()),
            "Осень 2025" => assert_eq!(sorted(&season.picks), (20..30).collect::<Vec<_>>()),
            other => panic!("{other}"),
        }
        assert_eq!(ids(&get(TileKind::FirstLikes).unwrap().picks), (100..115).collect::<Vec<_>>(), "earliest first");
        assert_eq!(ids(&get(TileKind::FreshLikes).unwrap().picks), (115..130).collect::<Vec<_>>(), "newest first");
        assert_eq!(sorted(&get(TileKind::Rediscover).unwrap().picks), (100..115).collect::<Vec<_>>(), "old likes never played");
        assert!(get(TileKind::Unplayed).is_none(), "everything was added today");
        let mut old = input.clone();
        for i in 200..215u128 {
            old.facts.get_mut(&tid(i)).unwrap().added_at = NOW - 10 * DAY_MS;
        }
        let unplayed = tile(&cat, &idx, &taste, &old, TileKind::Unplayed).unwrap();
        assert_eq!(sorted(&unplayed.picks), (200..215).collect::<Vec<_>>());
    }

    #[test]
    fn artist_tiles() {
        let (cat, mut idx, mut input) = catalog(60);
        // Artist 7 gets 40 more tracks, played a lot two months ago, then not.
        let more = (0..40u128)
            .map(|i| (info(1_000 + i, &[7], None), TrackFacts { title: format!("Seven {i}"), has_local: true, ..TrackFacts::default() }))
            .collect();
        let cat = grow(&cat, &mut idx, &mut input, more);
        let mut events: Vec<Play> = (0..40u128).map(|i| complete(1_000 + i, NOW - 60 * DAY_MS)).collect();
        // Artist 3's track 3 played lately: five songs are too few for a tile.
        events.extend((0..5).map(|k| complete(3, NOW - k * DAY_MS)));
        let taste = TasteInput { events, ..Default::default() };
        let comeback = tile(&cat, &idx, &taste, &input, TileKind::Comeback).unwrap();
        assert_eq!(comeback.title, "Artist 7");
        assert_eq!(comeback.param, aid(7).to_string());
        assert_eq!(comeback.picks.len(), 30);
        assert!(ids(&comeback.picks).iter().all(|i| *i >= 1_000), "the songs played most");
        assert_eq!(tile(&cat, &idx, &taste, &input, TileKind::ArtistOfDay).unwrap().title, "Artist 7");
        // Deep cuts: songs of the most played artists played once or never
        // (artist 3's track 3 was played five times).
        let deep = sorted(&tile(&cat, &idx, &taste, &input, TileKind::DeepCuts).unwrap().picks);
        assert_eq!(deep.len(), 30);
        assert!(!deep.contains(&3) && deep.iter().all(|i| i % 60 == 3 || i % 60 == 7 || *i >= 1_000), "{deep:?}");
    }

    #[test]
    fn a_parameter_rests_a_week() {
        // Twenty liked artists of ten songs each.
        let (cat, idx, input) = catalog(30);
        let likes: Vec<TrackId> = (0..300u128).filter(|i| i % 30 < 20).map(tid).collect();
        let taste = TasteInput { likes, ..Default::default() };
        let mut history = TileHistory::default();
        let mut seen = Vec::new();
        for d in 0..7 {
            let taste = Taste::build(&cat, &taste, NOW + d * DAY_MS);
            let recs = Recs::new(&cat, &taste, &idx);
            let t = recs.tile(TileKind::ArtistOfDay, &input, &history).unwrap();
            assert!(!seen.contains(&t.param), "{} again on day {d}", t.title);
            seen.push(t.param.clone());
            history.record(recs.local_day(), std::slice::from_ref(&t));
        }
    }

    #[test]
    fn sound_tiles() {
        let (cat, idx, mut input) = catalog(60);
        for i in 0..10u128 {
            input.facts.get_mut(&tid(i)).unwrap().duration_ms = Some(8 * 60_000);
        }
        for i in 10..30u128 {
            input.facts.get_mut(&tid(i)).unwrap().duration_ms = Some(120_000);
        }
        input.facts.get_mut(&tid(30)).unwrap().duration_ms = Some(20_000);
        for i in 40..50u128 {
            input.facts.get_mut(&tid(i)).unwrap().lyrics = if i < 43 { LyricsKind::Words } else { LyricsKind::Synced };
        }
        input.facts.get_mut(&tid(50)).unwrap().lyrics = LyricsKind::Plain;
        let taste = TasteInput {
            likes: vec![tid(200)],
            like_times: vec![(tid(200), NOW - DAY_MS)],
            ..Default::default()
        };
        let get = |k| tile(&cat, &idx, &taste, &input, k);
        assert_eq!(sorted(&get(TileKind::Long).unwrap().picks), (0..10).collect::<Vec<_>>());
        assert_eq!(sorted(&get(TileKind::Short).unwrap().picks), (10..30).collect::<Vec<_>>(), "an interlude is not a song");
        let sing = get(TileKind::SingAlong).unwrap();
        assert_eq!(sorted(&sing.picks), (40..50).collect::<Vec<_>>(), "plain lyrics do not count");
        let liked = get(TileKind::BecauseYouLiked).unwrap();
        assert_eq!(liked.title, "Похоже на Song 200");
        assert_eq!(like_title("Underground River (Opening Theme)"), "Похоже на Underground River");
        assert_eq!(like_title("(It's) Here"), "Похоже на (It's) Here");
        assert_eq!(liked.picks.len(), 30);
        assert!(ids(&liked.picks).contains(&200));
    }

    #[test]
    fn collection_tiles() {
        let (cat, mut idx, mut input) = catalog(60);
        for i in 0..20u128 {
            let f = input.facts.get_mut(&tid(i)).unwrap();
            f.title = format!("夜に駆ける {i}");
            f.year = Some(2019);
        }
        for i in 20..40u128 {
            let f = input.facts.get_mut(&tid(i)).unwrap();
            f.title = format!("Кукушка {i}");
            f.year = Some(1989);
        }
        for i in 40..50u128 {
            input.facts.get_mut(&tid(i)).unwrap().album = Some(format!("Made in Abyss OST {}", i % 2 + 1));
        }
        input.playlists = vec![PlaylistFacts {
            key: "p1".into(),
            name: "Вечером".into(),
            tracks: (60..75u128).map(tid).collect(),
        }];
        // 100..110 liked without files; 150..160 liked with.
        for i in 100..110u128 {
            input.facts.get_mut(&tid(i)).unwrap().has_local = false;
        }
        // Albums 77 and 78 of six songs each.
        let more = (0..12u128)
            .map(|k| (info(3_000 + k, &[1], Some(77 + k / 6)), TrackFacts { position: (k % 6) as u32 + 1, has_local: true, ..TrackFacts::default() }))
            .collect();
        let cat = grow(&cat, &mut idx, &mut input, more);
        // Album 77: two of six completed three weeks ago; album 78: all.
        let mut events = vec![complete(3_000, NOW - 21 * DAY_MS), complete(3_001, NOW - 21 * DAY_MS)];
        events.extend((6..12u128).map(|k| complete(3_000 + k, NOW - 21 * DAY_MS)));
        let taste = TasteInput {
            events,
            likes: (100..110u128).chain(150..160).map(tid).collect(),
            ..Default::default()
        };
        let get = |k| tile(&cat, &idx, &taste, &input, k);
        let lang = get(TileKind::Language).unwrap();
        assert!(["Японское", "По-русски"].contains(&lang.title.as_str()), "{}", lang.title);
        assert_eq!(lang.picks.len(), 20, "Latin is almost everything: no tile of its own");
        let decade = get(TileKind::Decade).unwrap();
        assert!(["Из 2010-х", "Из 80-х"].contains(&decade.title.as_str()), "{}", decade.title);
        let ost = get(TileKind::Soundtrack).unwrap();
        assert_eq!(ost.title, "Made in Abyss");
        assert_eq!(ost.picks.len(), 10, "both volumes");
        let playlist = get(TileKind::Playlist).unwrap();
        assert_eq!(playlist.title, "Вечером");
        assert_eq!(sorted(&playlist.picks), (60..75).collect::<Vec<_>>());
        assert_eq!(sorted(&get(TileKind::NotDownloaded).unwrap().picks), (100..110).collect::<Vec<_>>());
        assert!(get(TileKind::Unfinished).is_none(), "four songs left are fewer than eight");
        let lottery = get(TileKind::Lottery).unwrap();
        assert_eq!(lottery.picks.len(), 30);
        let built = Taste::build(&cat, &taste, NOW);
        assert_diverse(&Recs::new(&cat, &built, &idx), &lottery.picks);
        // A playlist played lately rests.
        let mut played = taste.clone();
        played.events.push(complete(61, NOW - DAY_MS));
        assert!(tile(&cat, &idx, &played, &input, TileKind::Playlist).is_none());
        // Two more albums left after their first song: fourteen songs to
        // finish, album by album in album order.
        let more = (12..24u128)
            .map(|k| (info(3_000 + k, &[2], Some(80 + k / 6)), TrackFacts { position: (k % 6) as u32 + 1, ..TrackFacts::default() }))
            .collect();
        let cat = grow(&cat, &mut idx, &mut input, more);
        let mut taste = taste;
        taste.events.extend([complete(3_012, NOW - 10 * DAY_MS), complete(3_018, NOW - 10 * DAY_MS)]);
        let got = ids(&tile(&cat, &idx, &taste, &input, TileKind::Unfinished).unwrap().picks);
        assert_eq!(got.len(), 14, "{got:?}");
        assert_eq!(&got[10..], &[3_002, 3_003, 3_004, 3_005], "the album left earlier comes last: {got:?}");
        assert!(got[..10].windows(2).filter(|w| w[1] == w[0] + 1).count() == 8, "album order: {got:?}");
    }

    #[test]
    fn a_week_of_boards_rotates() {
        let (cat, idx, mut input) = catalog(30);
        // Enough for many kinds at once.
        for i in 0..300u128 {
            let f = input.facts.get_mut(&tid(i)).unwrap();
            f.duration_ms = Some([8 * 60_000, 120_000, 200_000][i as usize % 3]);
            f.lyrics = if i % 4 == 0 { LyricsKind::Synced } else { LyricsKind::None };
            f.year = Some(if i % 2 == 0 { 1995 } else { 2015 });
            f.added_at = NOW - 30 * DAY_MS;
            if i % 5 == 0 {
                f.title = format!("夜 {i}");
            }
            if i % 7 == 0 {
                f.album = Some("Made in Abyss OST".into());
            }
        }
        let mut events = Vec::new();
        for i in 0..150u128 {
            for k in 0..3 {
                events.push(complete(i, NOW - (k * 9 + (i as i64 % 5)) * DAY_MS));
            }
            events.push(complete(i, NOW - YEAR_MS));
        }
        let likes: Vec<TrackId> = (0..60u128).map(tid).collect();
        let like_times = (0..60u128).map(|i| (tid(i), NOW - i as i64 * DAY_MS)).collect();
        input.events = events.clone();
        let base = TasteInput { events, likes, like_times, ..Default::default() };
        let mut history = TileHistory::default();
        let mut days: Vec<Vec<TileKind>> = Vec::new();
        for d in 0..7 {
            let taste = Taste::build(&cat, &base, NOW + d * DAY_MS);
            let recs = Recs::new(&cat, &taste, &idx);
            let tiles = recs.tiles_of_day(&input, &history, TILES_PER_DAY);
            assert_eq!(tiles.len(), TILES_PER_DAY);
            assert_eq!(tiles[0].kind, TileKind::OnRepeat, "the anchor first");
            let kinds: Vec<TileKind> = tiles.iter().map(|t| t.kind).collect();
            let groups: HashSet<TileGroup> = kinds[1..].iter().map(|k| k.group()).collect();
            assert!(groups.len() >= 3, "different groups: {kinds:?}");
            // The same day again, before or after it is recorded: the same board.
            assert_eq!(recs.tiles_of_day(&input, &history, TILES_PER_DAY), tiles);
            history.record(recs.local_day(), &tiles);
            assert_eq!(recs.tiles_of_day(&input, &history, TILES_PER_DAY), tiles);
            days.push(kinds);
        }
        let shown: HashSet<TileKind> = days.iter().flatten().copied().collect();
        assert!(shown.len() >= 15, "{} kinds in a week: {days:?}", shown.len());
        for w in days.windows(2) {
            for k in &w[1][1..] {
                assert!(!w[0].contains(k), "{k:?} two days in a row: {days:?}");
            }
        }
    }
}
