//! The bento's tiles in the runtime (docs/stage4.md §12.9): what the
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
};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{BuildInput, CoverRef, Snapshot, artists_line, cache_read, cache_write, covers_of, still_fits, tracks_count};

/// The day's board.
const DAY_KEY: &str = "recs.tiles";
/// What the last days showed.
const SHOWN_KEY: &str = "recs.tiles.shown";

/// A tile of the home.
pub(crate) struct TileHome {
    pub kind: TileKind,
    pub title: String,
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
    TilesInput {
        facts,
        events: events.iter().map(nikgob_recs::Play::from).collect(),
        playlists,
        artist_names,
    }
}

/// The board of today: the one kept for the day, else picked now (and
/// the day recorded in the history).
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput) -> Vec<TileHome> {
    let lib = &input.lib;
    let history = snap.taste.has_history();
    let kept = cache_read::<TilesDay>(lib, DAY_KEY).filter(|c| c.day == input.day && (c.history || !history));
    let board = match kept {
        Some(c) => c.tiles,
        None => {
            let mut shown: TileHistory = cache_read(lib, SHOWN_KEY).unwrap_or_default();
            let board = snap.recs().tiles_of_day(tiles, &shown, TILES_PER_DAY);
            shown.record(snap.recs().local_day(), &board);
            cache_write(lib, SHOWN_KEY, input.now, &shown);
            let day = TilesDay {
                day: input.day.clone(),
                history,
                tiles: board.clone(),
            };
            cache_write(lib, DAY_KEY, input.now, &day);
            board
        }
    };
    let today = snap.recs().local_day();
    board.into_iter().filter_map(|t| home_of(snap, lib, today, t)).collect()
}

/// A tile as the home shows it: what is still allowed of it, its lines and
/// covers. `None` when nothing is left.
fn home_of(snap: &Snapshot, lib: &Library, today: i64, t: TileMix) -> Option<TileHome> {
    let no_streams = HashMap::new();
    let picks: Vec<Pick> = t.picks.into_iter().filter(|p| still_fits(snap, p, &no_streams)).collect();
    if picks.is_empty() {
        return None;
    }
    let n = tracks_count(picks.len());
    let artists = || match artists_line(lib, snap, &picks) {
        a if a.is_empty() => n.clone(),
        a => a,
    };
    let subtitle = match t.kind {
        TileKind::OnRepeat | TileKind::FreshLikes => format!("{n} · за месяц"),
        TileKind::YearAgo => format!("{} · {n}", month_year(today - 365)),
        TileKind::Rediscover | TileKind::Comeback => format!("{n} · давно не играли"),
        TileKind::FirstLikes => format!("{n} · первые лайки"),
        TileKind::Unplayed => format!("{n} · ни разу не играли"),
        TileKind::ArtistOfDay => format!("{n} · лучшее"),
        TileKind::BecauseYouLiked => format!("{n} · похожее по звучанию"),
        TileKind::Playlist => format!("{n} · вперемешку"),
        TileKind::Unfinished => {
            let albums: std::collections::HashSet<_> = picks
                .iter()
                .filter_map(|p| match p.item {
                    Item::Track(id) => snap.catalog.get(id).and_then(|t| t.release),
                    Item::Stream(_) => None,
                })
                .collect();
            let a = albums.len();
            format!("{n} · {a} {}", crate::streams::plural(a, "альбом", "альбома", "альбомов"))
        }
        TileKind::NotDownloaded => format!("{n} · играют из сервиса"),
        TileKind::Season
        | TileKind::DeepCuts
        | TileKind::Long
        | TileKind::Short
        | TileKind::SingAlong
        | TileKind::Language
        | TileKind::Decade
        | TileKind::Soundtrack
        | TileKind::Lottery => artists(),
    };
    let covers = covers_of(snap, &picks, &no_streams);
    Some(TileHome {
        kind: t.kind,
        title: t.title,
        subtitle,
        picks,
        covers,
    })
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
    crate::analysis::load_index(lib, &index);
    let (_, utc_offset_min) = super::local_day();
    let mut shown: TileHistory = cache_read(lib, SHOWN_KEY).unwrap_or_default();
    let mut out = Vec::with_capacity(days);
    for d in 0..days {
        let now = super::recs_now() + d as i64 * nikgob_recs::taste::DAY_MS;
        let (snap, input, _) = super::read_snapshot(lib, &index, now, utc_offset_min)?;
        let recs = snap.recs();
        let day = recs.local_day();
        let board = recs.tiles_of_day(&input, &shown, TILES_PER_DAY);
        shown.record(day, &board);
        let tiles = board
            .into_iter()
            .filter_map(|t| home_of(&snap, lib, day, t))
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
    crate::analysis::load_index(lib, &index);
    let (_, utc_offset_min) = super::local_day();
    let (snap, input, _) = super::read_snapshot(lib, &index, super::recs_now(), utc_offset_min)?;
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
}
