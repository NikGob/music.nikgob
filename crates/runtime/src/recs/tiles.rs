//! The bento's tiles in the runtime: what the
//! recipes read besides the snapshot, the day's board kept in the
//! library's cache (`recs.tiles`, so it does not change within the day),
//! the kinds and parameters shown on the last days (`recs.tiles.shown`, the
//! next day's choice avoids them), and the cards' lines.

use std::collections::HashMap;

use nikgob_core::model::{ArtistId, Event, TrackId};
use nikgob_library::{Library, LyricsLevel, RecTrack};
use nikgob_recs::tiles::civil;
use nikgob_recs::{
    Item, LyricsKind, Pick, PlaylistFacts, TILES_PER_DAY, TileHistory, TileKind, TileMix, TilesInput, TrackFacts,
    like_title,
};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{BuildInput, CoverRef, Snapshot, cache_read, cache_write, covers_of, still_fits, tracks_count};

/// The day's board.
const DAY_KEY: &str = "recs.tiles";
/// What the last days showed.
const SHOWN_KEY: &str = "recs.tiles.shown";

/// «Подборки дня» shows this many tiles a day at most: more than fit in a
/// row, which goes on sideways.
pub(crate) const HOME_TILES: usize = 8;
/// The day's board is drawn this large: the kinds the home leaves out
/// («На повторе», the lottery, the languages) leave [`HOME_TILES`].
const HOME_BOARD: usize = HOME_TILES + 3;

/// A tile of the home.
pub(crate) struct TileHome {
    pub kind: TileKind,
    pub title: String,
    /// «1 ч 4 мин»: how long its songs play.
    pub duration: String,
    /// «25 треков · за месяц», «Kevin Penkin, Pedro Silva и другие».
    pub subtitle: String,
    pub picks: Vec<Pick>,
    pub covers: Vec<CoverRef>,
}

impl TileHome {
    pub fn mix_id(&self) -> v::MixId {
        v::MixId::Tile(view_kind(self.kind))
    }

    pub fn kicker(&self) -> &'static str {
        self.kind.kicker()
    }
}

/// The view's name of a kind (the two enums list the kinds in one order).
pub(crate) fn view_kind(k: TileKind) -> v::TileKind {
    v::TileKind::ALL[k as usize]
}

/// The day's board as the library's cache keeps it.
#[derive(Serialize, Deserialize)]
struct TilesDay {
    day: String,
    /// Picked from a history: a board without one is picked again once
    /// there is one.
    history: bool,
    tiles: Vec<TileMix>,
    /// Picked under «Миксы только из скачанного»: picked again when it
    /// changes.
    #[serde(default)]
    local_only: bool,
    /// How many tiles it was drawn for: a board of another size (an older
    /// one, drawn for fewer) is drawn again.
    #[serde(default)]
    size: usize,
}

/// What the tiles read besides the snapshot: the facts of every track,
/// the playlists, the plays and the artists' names. A fact that cannot be
/// read leaves its tiles out.
pub(super) fn input(lib: &Library, rows: &[RecTrack], events: &[Event], artist_names: HashMap<ArtistId, String>) -> TilesInput {
    let read = lib.track_facts().unwrap_or_else(|e| {
        log::warn!("recs: the tiles' facts: {e}");
        Vec::new()
    });
    let mut facts: HashMap<TrackId, TrackFacts> = read
        .into_iter()
        .map(|f| {
            let lyrics = match f.lyrics {
                LyricsLevel::None => LyricsKind::None,
                LyricsLevel::Plain => LyricsKind::Plain,
                LyricsLevel::Lines => LyricsKind::Synced,
                LyricsLevel::Words => LyricsKind::Words,
            };
            let fact = TrackFacts {
                duration_ms: f.duration_ms,
                added_at: f.added_at,
                year: f.year,
                album: f.album,
                position: f.disc_no.saturating_mul(1000).saturating_add(f.track_no),
                lyrics,
                has_local: f.has_local,
                ..TrackFacts::default()
            };
            (f.id, fact)
        })
        .collect();
    for r in rows {
        if let Some(f) = facts.get_mut(&r.id) {
            f.title.clone_from(&r.title);
            f.artist.clone_from(&r.artist);
        }
    }
    let playlists = lib
        .playlist_tracks()
        .unwrap_or_else(|e| {
            log::warn!("recs: the tiles' playlists: {e}");
            Vec::new()
        })
        .into_iter()
        .map(|p| PlaylistFacts {
            key: p.id.to_string(),
            name: p.name,
            tracks: p.tracks,
        })
        .collect();
    // Each play on its device; a room's long passive runs are not what the
    // tiles count.
    let devices = super::Devices::read(lib);
    let plays: Vec<nikgob_recs::Play> = events.iter().filter_map(|e| devices.place(e, nikgob_recs::Play::from(e))).collect();
    let whole = super::whole(&plays);
    TilesInput {
        facts,
        events: plays.into_iter().zip(whole).filter_map(|(p, w)| w.then_some(p)).collect(),
        playlists,
        artist_names,
    }
}

/// The board of today: the one kept for the day, else picked now (and
/// the day recorded in the history).
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput) -> Vec<TileHome> {
    let lib = &input.lib;
    let history = snap.model.taste.has_history();
    let kept = cache_read::<TilesDay>(lib, DAY_KEY)
        .filter(|c| c.day == input.day && (c.history || !history) && c.local_only == snap.local_only && c.size == HOME_BOARD);
    let board = match kept {
        Some(c) => c.tiles,
        None => {
            let mut shown: TileHistory = cache_read(lib, SHOWN_KEY).unwrap_or_default();
            let board = snap.recs().tiles_of_day(tiles, &shown, HOME_BOARD);
            shown.record(snap.recs().local_day(), &board);
            cache_write(lib, SHOWN_KEY, input.now, &shown);
            let day = TilesDay {
                day: input.day.clone(),
                history,
                tiles: board.clone(),
                local_only: snap.local_only,
                size: HOME_BOARD,
            };
            cache_write(lib, DAY_KEY, input.now, &day);
            board
        }
    };
    let today = snap.recs().local_day();
    // «На повторе» and the languages are mixes of their own now and
    // «Наугад» is gone from the home: the day shows the
    // others, at most [`HOME_TILES`].
    board
        .into_iter()
        .filter(|t| !matches!(t.kind, TileKind::OnRepeat | TileKind::Lottery | TileKind::Language))
        .filter_map(|t| home_of(snap, tiles, today, t))
        .take(HOME_TILES)
        .collect()
}

/// «1 ч 4 мин», «2 ч», «45 мин» of `ms`.
pub(crate) fn hours_minutes(ms: u64) -> String {
    let m = ((ms + 30_000) / 60_000).max(1);
    match (m / 60, m % 60) {
        (0, m) => format!("{m} мин"),
        (h, 0) => format!("{h} ч"),
        (h, m) => format!("{h} ч {m} мин"),
    }
}

/// «каждый раз другое — жми и узнаешь»: the lottery's line, all it says.
pub(crate) const LOTTERY_SUBTITLE: &str = "каждый раз другое — жми и узнаешь";

/// A tile as the home shows it: what is still allowed of it, its lines and
/// covers. `None` when nothing is left. The lottery shows no covers: what
/// it plays is drawn at the click.
fn home_of(snap: &Snapshot, facts: &TilesInput, today: i64, t: TileMix) -> Option<TileHome> {
    let no_streams = HashMap::new();
    let picks: Vec<Pick> = t.picks.into_iter().filter(|p| still_fits(snap, p, &no_streams)).collect();
    if picks.is_empty() {
        return None;
    }
    let (title, subtitle) = lines(t.kind, t.title, picks.len(), today);
    let covers = if t.kind == TileKind::Lottery { Vec::new() } else { covers_of(snap, &picks, &no_streams) };
    let ms: u64 = picks
        .iter()
        .map(|p| match &p.item {
            Item::Track(id) => u64::from(facts.facts.get(id).and_then(|f| f.duration_ms).unwrap_or(210_000)),
            Item::Stream(_) => 210_000,
        })
        .sum();
    Some(TileHome {
        kind: t.kind,
        duration: hours_minutes(ms),
        title,
        subtitle,
        picks,
        covers,
    })
}

/// A tile's title and subtitle: what it is and how many songs, short
/// enough for a tile. `title` is the recipe's («Kevin Penkin», «Лето
/// 2025»), `n` the songs.
fn lines(kind: TileKind, title: String, n: usize, today: i64) -> (String, String) {
    // A board kept from before the titles changed reads as today's.
    let title = match kind {
        TileKind::OnRepeat => "На повторе".to_owned(),
        TileKind::Rediscover => "Забытое любимое".to_owned(),
        TileKind::FirstLikes => "Первые лайки".to_owned(),
        TileKind::FreshLikes => "Свежие лайки".to_owned(),
        TileKind::Unplayed => "Ждут своего часа".to_owned(),
        TileKind::DeepCuts => "Глубже у любимых".to_owned(),
        TileKind::Long => "Длинные треки".to_owned(),
        TileKind::Short => "Короткие треки".to_owned(),
        TileKind::SingAlong => "Песни с текстом".to_owned(),
        TileKind::Unfinished => "Недослушанные альбомы".to_owned(),
        TileKind::NotDownloaded => "Ещё не скачано".to_owned(),
        TileKind::Lottery => "Лотерея".to_owned(),
        TileKind::YearAgo => {
            let mut month = month_year(today - 365);
            if let Some(first) = month.get(..2) {
                month = first.to_uppercase() + &month[2..];
            }
            month
        }
        TileKind::BecauseYouLiked if !title.starts_with("Похоже на ") => like_title(&title),
        _ => title,
    };
    // The name says what it is; the line under it, how many songs.
    let subtitle = match kind {
        TileKind::Lottery => LOTTERY_SUBTITLE.to_owned(),
        _ => tracks_count(n),
    };
    (title, subtitle)
}

const MONTHS: [&str; 12] = [
    "январь", "февраль", "март", "апрель", "май", "июнь", "июль", "август", "сентябрь", "октябрь", "ноябрь", "декабрь",
];

/// «октябрь 2025» of a local day number.
fn month_year(day: i64) -> String {
    let (y, m, _) = civil(day);
    format!("{} {y}", MONTHS[(m as usize).clamp(1, 12) - 1])
}

/// One tile of a previewed day: its kind, title, subtitle and song count.
pub type PreviewTile = (v::TileKind, String, String, usize);

/// The boards of `days` days from today on, as the runtime would pick
/// them day after day from the history the library keeps (nothing is
/// written): checks of the rotation on a copy of a real library and on the
/// demo.
pub fn preview_tile_days(lib: &Library, days: usize) -> Result<Vec<Vec<PreviewTile>>, String> {
    let index = crate::analysis::SoundIndex::default();
    crate::analysis::load_offline(lib, &index, &nikgob_view::Prefs::default());
    let (_, utc_offset_min) = super::local_day();
    let mut shown: TileHistory = cache_read(lib, SHOWN_KEY).unwrap_or_default();
    let mut out = Vec::with_capacity(days);
    for d in 0..days {
        let now = super::recs_now() + d as i64 * nikgob_recs::taste::DAY_MS;
        let (snap, input, _) = super::read_snapshot(lib, &index, now, utc_offset_min, &[], false)?;
        let recs = snap.recs();
        let day = recs.local_day();
        let board = recs.tiles_of_day(&input, &shown, TILES_PER_DAY);
        shown.record(day, &board);
        let tiles = board
            .into_iter()
            .filter_map(|t| home_of(&snap, &input, day, t))
            .map(|t| (view_kind(t.kind), t.title, t.subtitle, t.picks.len()))
            .collect();
        out.push(tiles);
    }
    Ok(out)
}

/// Every kind that has enough songs today, as the runtime would see it
/// (its title and song count): what a library can show at all.
pub fn preview_ready_tiles(lib: &Library) -> Result<Vec<(v::TileKind, String, usize)>, String> {
    let index = crate::analysis::SoundIndex::default();
    crate::analysis::load_offline(lib, &index, &nikgob_view::Prefs::default());
    let (_, utc_offset_min) = super::local_day();
    let (snap, input, _) = super::read_snapshot(lib, &index, super::recs_now(), utc_offset_min, &[], false)?;
    let shown: TileHistory = cache_read(lib, SHOWN_KEY).unwrap_or_default();
    Ok(snap
        .recs()
        .ready_tiles(&input, &shown)
        .into_iter()
        .map(|t| (view_kind(t.kind), t.title, t.picks.len()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_map_to_the_view_in_order() {
        for k in TileKind::ALL {
            assert_eq!(format!("{k:?}"), format!("{:?}", view_kind(k)));
        }
        assert_eq!(TileKind::ALL.len(), v::TileKind::ALL.len());
        assert_eq!(month_year(20_748 - 365), "октябрь 2025");
    }

    /// Every tile says what it is in a title and a subtitle that fit a
    /// tile.
    #[test]
    fn every_tile_has_a_name_and_its_count() {
        for k in TileKind::ALL {
            let (title, subtitle) = lines(k, "Kevin Penkin".into(), 25, 20_748);
            assert!(!title.is_empty() && title.chars().count() <= 24, "{k:?}: {title}");
            assert!(subtitle.chars().count() <= 40, "{k:?}: {subtitle} ({})", subtitle.chars().count());
            assert_ne!(k.kicker().to_lowercase(), title.to_lowercase(), "{k:?}: the kicker repeats the title");
            assert!(k.kicker().chars().count() <= 16, "{k:?}: {}", k.kicker());
            if k != TileKind::Lottery {
                assert_eq!(subtitle, tracks_count(25), "{k:?}: the count only");
            }
        }
        let line = |k| lines(k, String::new(), 13, 20_748);
        assert_eq!(line(TileKind::YearAgo).0, "Октябрь 2025");
        assert_eq!(line(TileKind::Season).1, "13 треков");
        assert_eq!(line(TileKind::Lottery).1, LOTTERY_SUBTITLE);
        // A board kept from before the titles changed.
        let kept = |k, t: &str| lines(k, t.into(), 13, 20_748).0;
        assert_eq!(kept(TileKind::BecauseYouLiked, "Underground River (opening version)"), "Похоже на Underground River");
        assert_eq!(kept(TileKind::BecauseYouLiked, "Похоже на Разгром"), "Похоже на Разгром");
        assert_eq!(kept(TileKind::NotDownloaded, "Лайкнутое без файла"), "Ещё не скачано");
        assert_eq!(kept(TileKind::ArtistOfDay, "Eve"), "Eve");
    }
}
