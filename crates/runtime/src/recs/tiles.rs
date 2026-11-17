//! The bento's tiles in the runtime: what the
//! recipes read besides the snapshot, the day's board kept in the
//! library's cache (`recs.tiles`, so it does not change within the day),
//! the kinds and parameters shown on the last days (`recs.tiles.shown`, the
//! next day's choice avoids them), and the cards' lines.

use std::collections::HashMap;

use nikgob_core::i18n::{Locale, tracks_count_in};
use nikgob_core::model::{ArtistId, Event, TrackId};
use nikgob_library::{Library, LyricsLevel, RecTrack};
use nikgob_recs::{
    Item, LyricsKind, Pick, PlaylistFacts, TILES_PER_DAY, TileHistory, TileKind, TileMix, TilesInput, TrackFacts,
    tile_title_in, year_ago_title_in,
};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{BuildInput, CoverRef, Snapshot, cache_read, cache_write, covers_of, still_fits};

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
    /// Locale-neutral parameter (`2025-2`, `1990`, `ja`, track/artist id, …).
    pub param: String,
    /// Locale-neutral subject (artist name, cleaned song title, playlist/franchise name, …).
    pub subject: String,
    /// Local day the tile was built for (used for `YearAgo` relocalization).
    pub today: i64,
    /// Total duration of its picks in ms.
    pub ms: u64,
    pub title: String,
    /// «1 ч 4 мин» / «1 hr 4 min»: how long its songs play.
    pub duration: String,
    /// «25 треков» / «25 tracks», «каждый раз другое — жми и узнаешь».
    pub subtitle: String,
    pub picks: Vec<Pick>,
    pub covers: Vec<CoverRef>,
}

impl TileHome {
    pub fn mix_id(&self) -> v::MixId {
        v::MixId::Tile(view_kind(self.kind))
    }

    pub fn kicker(&self, loc: Locale) -> &'static str {
        self.kind.kicker_in(loc)
    }

    /// Invariant Russian title used as canonical analytics context key (`mix:{title}`).
    pub fn ru_title(&self) -> String {
        self.title_in(Locale::Ru)
    }

    /// Renders this tile's title in `loc`.
    pub fn title_in(&self, loc: Locale) -> String {
        lines(self.kind, &self.param, &self.subject, self.picks.len(), self.today, loc).0
    }

    /// Typed relocalizable title for this tile so shuffles, plays and restarts survive locale switches.
    pub fn mix_title(&self) -> v::MixTitle {
        use nikgob_core::i18n::{PerLocale, TileStatic};
        match view_kind(self.kind) {
            v::TileKind::OnRepeat => v::MixTitle::Tile(TileStatic::OnRepeat),
            v::TileKind::Rediscover => v::MixTitle::Tile(TileStatic::Rediscover),
            v::TileKind::FirstLikes => v::MixTitle::Tile(TileStatic::FirstLikes),
            v::TileKind::FreshLikes => v::MixTitle::Tile(TileStatic::FreshLikes),
            v::TileKind::Unplayed => v::MixTitle::Tile(TileStatic::Unplayed),
            v::TileKind::DeepCuts => v::MixTitle::Tile(TileStatic::DeepCuts),
            v::TileKind::Long => v::MixTitle::Tile(TileStatic::Long),
            v::TileKind::Short => v::MixTitle::Tile(TileStatic::Short),
            v::TileKind::SingAlong => v::MixTitle::Tile(TileStatic::SingAlong),
            v::TileKind::Unfinished => v::MixTitle::Tile(TileStatic::Unfinished),
            v::TileKind::NotDownloaded => v::MixTitle::Tile(TileStatic::NotDownloaded),
            v::TileKind::Lottery => v::MixTitle::Tile(TileStatic::Lottery),
            v::TileKind::BecauseYouLiked => match v::MixTitle::from_legacy(&self.ru_title()) {
                mt @ v::MixTitle::BecauseYouLiked(_) => mt,
                _ => v::MixTitle::Localized(PerLocale::init(|loc| self.title_in(loc))),
            },
            v::TileKind::YearAgo
            | v::TileKind::Season
            | v::TileKind::ArtistOfDay
            | v::TileKind::Comeback
            | v::TileKind::Language
            | v::TileKind::Decade
            | v::TileKind::Soundtrack
            | v::TileKind::Playlist => v::MixTitle::Localized(PerLocale::init(|loc| self.title_in(loc))),
        }
    }

    /// Recomputes all localized strings in place when switching `Locale`.
    pub fn relocalize(&mut self, loc: Locale) {
        let (title, subtitle) = lines(self.kind, &self.param, &self.subject, self.picks.len(), self.today, loc);
        self.title = title;
        self.subtitle = subtitle;
        self.duration = hours_minutes(self.ms, loc);
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
        .filter_map(|t| home_of(snap, tiles, today, t, input.locale))
        .take(HOME_TILES)
        .collect()
}

/// «1 ч 4 мин» / «1 hr 4 min», «2 ч» / «2 hr», «45 мин» / «45 min» of `ms`.
pub(crate) fn hours_minutes(ms: u64, loc: Locale) -> String {
    let m = ((ms + 30_000) / 60_000).max(1);
    match loc {
        Locale::Ru => match (m / 60, m % 60) {
            (0, m) => format!("{m} мин"),
            (h, 0) => format!("{h} ч"),
            (h, m) => format!("{h} ч {m} мин"),
        },
        Locale::En => match (m / 60, m % 60) {
            (0, m) => format!("{m} min"),
            (h, 0) => format!("{h} hr"),
            (h, m) => format!("{h} hr {m} min"),
        },
    }
}

/// «каждый раз другое — жми и узнаешь»: the lottery's line, all it says.
pub(crate) const fn lottery_subtitle_in(loc: Locale) -> &'static str {
    match loc {
        Locale::Ru => "каждый раз другое — жми и узнаешь",
        Locale::En => "different every time — tap to find out",
    }
}

/// A tile as the home shows it: what is still allowed of it, its lines and
/// covers. `None` when nothing is left. The lottery shows no covers: what
/// it plays is drawn at the click.
fn home_of(snap: &Snapshot, facts: &TilesInput, today: i64, t: TileMix, loc: Locale) -> Option<TileHome> {
    let no_streams = HashMap::new();
    let picks: Vec<Pick> = t.picks.into_iter().filter(|p| still_fits(snap, p, &no_streams)).collect();
    if picks.is_empty() {
        return None;
    }
    let (title, subtitle) = lines(t.kind, &t.param, &t.subject, picks.len(), today, loc);
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
        param: t.param,
        subject: t.subject,
        today,
        ms,
        duration: hours_minutes(ms, loc),
        title,
        subtitle,
        picks,
        covers,
    })
}

/// A tile's title and subtitle: what it is and how many songs, short
/// enough for a tile. `param` and `subject` come from [`TileMix`], `n` the songs.
fn lines(kind: TileKind, param: &str, subject: &str, n: usize, today: i64, loc: Locale) -> (String, String) {
    let title = match kind {
        TileKind::YearAgo => year_ago_title_in(today, loc),
        _ => tile_title_in(kind, param, subject, loc),
    };
    // The name says what it is; the line under it, how many songs.
    let subtitle = match kind {
        TileKind::Lottery => lottery_subtitle_in(loc).to_owned(),
        _ => tracks_count_in(loc, n as u64),
    };
    (title, subtitle)
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
    let loc = nikgob_core::i18n::locale();
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
            .filter_map(|t| home_of(&snap, &input, day, t, loc))
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
    let loc = nikgob_core::i18n::locale();
    Ok(snap
        .recs()
        .ready_tiles(&input, &shown)
        .into_iter()
        .map(|t| (view_kind(t.kind), t.title_in(loc), t.picks.len()))
        .collect())
}

#[cfg(test)]
mod tests {
    use nikgob_recs::month_year_in;

    use super::*;

    #[test]
    fn kinds_map_to_the_view_in_order() {
        for k in TileKind::ALL {
            assert_eq!(format!("{k:?}"), format!("{:?}", view_kind(k)));
        }
        assert_eq!(TileKind::ALL.len(), v::TileKind::ALL.len());
        assert_eq!(month_year_in(20_748 - 365, Locale::Ru), "Октябрь 2025");
        assert_eq!(month_year_in(20_748 - 365, Locale::En), "October 2025");
    }

    /// Every tile says what it is in a title and a subtitle that fit a
    /// tile.
    #[test]
    fn every_tile_has_a_name_and_its_count() {
        for loc in Locale::ALL {
            for k in TileKind::ALL {
                let param = match k {
                    TileKind::Season => "2025-2",
                    TileKind::Language => "ja",
                    TileKind::Decade => "1990",
                    _ => "",
                };
                let (title, subtitle) = lines(k, param, "Kevin Penkin", 25, 20_748, loc);
                assert!(!title.is_empty() && title.chars().count() <= 24, "{loc:?} {k:?}: {title}");
                assert!(subtitle.chars().count() <= 40, "{loc:?} {k:?}: {subtitle} ({})", subtitle.chars().count());
                assert_ne!(k.kicker_in(loc).to_lowercase(), title.to_lowercase(), "{loc:?} {k:?}: the kicker repeats the title");
                assert!(k.kicker_in(loc).chars().count() <= 16, "{loc:?} {k:?}: {}", k.kicker_in(loc));
                if k != TileKind::Lottery {
                    assert_eq!(subtitle, tracks_count_in(loc, 25), "{loc:?} {k:?}: the count only");
                }
            }
        }
        let line = |k| lines(k, "", "", 13, 20_748, Locale::Ru);
        assert_eq!(line(TileKind::YearAgo).0, "Октябрь 2025");
        assert_eq!(line(TileKind::Lottery).1, lottery_subtitle_in(Locale::Ru));
        // A board kept from before the titles changed.
        let kept = |k, t: &str, loc| lines(k, "", t, 13, 20_748, loc).0;
        assert_eq!(kept(TileKind::BecauseYouLiked, "Underground River (opening version)", Locale::Ru), "Похоже на Underground River");
        assert_eq!(kept(TileKind::BecauseYouLiked, "Underground River (opening version)", Locale::En), "Similar to Underground River");
        assert_eq!(kept(TileKind::BecauseYouLiked, "Похоже на Разгром", Locale::Ru), "Похоже на Разгром");
        assert_eq!(kept(TileKind::BecauseYouLiked, "Похоже на Разгром", Locale::En), "Similar to Разгром");
        assert_eq!(kept(TileKind::NotDownloaded, "Лайкнутое без файла", Locale::Ru), "Ещё не скачано");
        assert_eq!(kept(TileKind::ArtistOfDay, "Eve", Locale::Ru), "Eve");
    }

    #[test]
    fn dynamic_tiles_produce_relocalizable_mix_titles() {
        let make = |kind: TileKind, param: &str, subject: &str| TileHome {
            kind,
            param: param.into(),
            subject: subject.into(),
            today: 20_748,
            ms: 0,
            title: String::new(),
            duration: String::new(),
            subtitle: String::new(),
            picks: Vec::new(),
            covers: Vec::new(),
        };
        let year_ago = make(TileKind::YearAgo, "", "").mix_title();
        assert_eq!(year_ago.title_in(Locale::Ru), "Октябрь 2025");
        assert_eq!(year_ago.title_in(Locale::En), "October 2025");
        let decade = make(TileKind::Decade, "1990", "").mix_title();
        assert_eq!(decade.title_in(Locale::Ru), "Из 90-х");
        assert_eq!(decade.title_in(Locale::En), "From the 90s");
        let season = make(TileKind::Season, "2025-2", "").mix_title();
        assert_eq!(season.title_in(Locale::Ru), "Лето 2025");
        assert_eq!(season.title_in(Locale::En), "Summer 2025");
        let round: v::MixTitle = serde_json::from_str(&serde_json::to_string(&year_ago).unwrap()).unwrap();
        assert_eq!(round, year_ago);

        // Card titles and typed queue mix titles agree identically for every TileKind across all locales.
        for k in TileKind::ALL {
            let param = match k {
                TileKind::Season => "2025-2",
                TileKind::Language => "ja",
                TileKind::Decade => "1990",
                _ => "",
            };
            let tile = make(k, param, "Eve");
            let mt = tile.mix_title();
            for loc in Locale::ALL {
                assert_eq!(mt.title_in(loc), tile.title_in(loc), "{loc:?} {k:?}");
            }
        }
    }
}
