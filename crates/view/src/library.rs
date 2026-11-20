//! The library's view models: tracks, releases, artists, playlists, the
//! disliked ones and the progress of the sound analysis.

use std::time::Duration;

use super::*;

/// One row of a track list.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrackRow {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub release_id: Option<ReleaseId>,
    /// Every credited artist (a link on each name of `artist`).
    pub artists: ArtistLinks,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration: Option<Duration>,
    /// e.g. "FLAC → Opus 160", "MP3 320".
    pub quality: Option<String>,
    /// Where it would play from right now; `None` = nothing playable.
    pub source: Option<SourceKind>,
    pub has_local: bool,
    pub liked: bool,
    pub cover: Option<Cover>,
    /// What plays is a YouTube music video, not the song («· клип»).
    pub clip: bool,
    /// A library track: a song heard only (the wave, a radio, a find of
    /// Главная) is not, its menu offers «В библиотеку».
    #[serde(default)]
    pub in_library: bool,
}

impl TrackRow {
    /// The first (main) credited artist.
    pub fn artist_id(&self) -> Option<ArtistId> {
        self.artists.main().copied()
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseCard {
    pub id: ReleaseId,
    pub title: String,
    pub artist: String,
    /// Every credited artist (a link on each name of `artist`).
    pub artists: ArtistLinks,
    pub year: Option<i32>,
    pub release_type: ReleaseType,
    pub cover: Option<Cover>,
    pub local_tracks: u32,
    pub total_tracks: Option<u32>,
    pub liked: bool,
    /// When it came into the library (Unix ms): «Недавно добавлено» and the
    /// group headers of that sort.
    pub added_at: i64,
    /// A file of it a genuine lossless copy would really be better than
    /// (an MP3, a fake FLAC): its menus offer «Найти качество лучше».
    #[serde(default)]
    pub better: bool,
    /// Only some of the album is the user's (a song or a few of it, not
    /// added whole): how many of its songs are theirs. The card counts them
    /// («2 песни»), never the songs it lacks; ▶ plays them (or the whole
    /// album, as the album's page is set to open).
    #[serde(default)]
    pub mine: Option<u32>,
    /// Of the user's songs, the ones with a file.
    #[serde(default)]
    pub mine_local: u32,
    /// The whole album was added to the library («Добавить в библиотеку»)
    /// or saved.
    #[serde(default)]
    pub whole: bool,
}

impl ReleaseCard {
    /// The first (main) credited artist.
    pub fn artist_id(&self) -> Option<ArtistId> {
        self.artists.main().copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseDetail {
    pub card: ReleaseCard,
    pub tracks: Vec<TrackRow>,
    pub total_duration: Duration,
    /// "FLAC → Opus" style summary chip.
    pub quality: Option<String>,
    pub liked: bool,
    /// The page's lines in order, one per place the album counts (the
    /// card's «13 из 14» is their count); empty: just the tracks.
    pub lines: Vec<ReleaseLine>,
    /// The day (or month) it came out, when known beyond the card's year:
    /// from its files' tags, else from a service.
    pub released: Option<ReleaseDay>,
    /// The record label, when its files' tags name one.
    pub label: Option<String>,
    /// What each of [`Self::tracks`] is to the user, in their order (empty:
    /// every row is theirs).
    pub marks: Vec<AlbumMark>,
    /// Only some of the album is the user's ([`ReleaseCard::mine`]): the
    /// page has «Мои треки» and «Весь альбом».
    pub partial: bool,
    /// The page opens on «Весь альбом» (the album's own choice, else the
    /// setting); «Слушать» on a card plays it whole.
    pub open_whole: bool,
    /// The whole album was added to the library («Добавить в библиотеку»):
    /// its ⋮ takes it back to the user's songs.
    pub added_whole: bool,
    /// Its other versions play with it (the switch of «Другие версии»).
    pub versions_on: bool,
}

impl ReleaseDetail {
    /// The lines of «Мои треки»: the user's songs, in the album's order.
    pub fn mine_lines(&self) -> Vec<ReleaseLine> {
        (0..self.tracks.len())
            .filter(|&i| self.marks.get(i).is_none_or(|m| *m == AlbumMark::Mine))
            .map(ReleaseLine::Track)
            .collect()
    }

    /// The lines of «Весь альбом»: every place of the album; other versions
    /// of its songs the user did not choose go last (`versions`, their
    /// first index), as a group of their own.
    pub fn all_lines(&self) -> (Vec<ReleaseLine>, Option<usize>) {
        let every: Vec<ReleaseLine> =
            if self.lines.is_empty() { (0..self.tracks.len()).map(ReleaseLine::Track).collect() } else { self.lines.clone() };
        let version = |l: &ReleaseLine| matches!(l, ReleaseLine::Track(i) if self.marks.get(*i) == Some(&AlbumMark::Version));
        let (versions, mut lines): (Vec<ReleaseLine>, Vec<ReleaseLine>) = every.into_iter().partition(|l| version(l));
        if versions.is_empty() {
            return (lines, None);
        }
        let at = lines.len();
        lines.extend(versions);
        (lines, Some(at))
    }
}

/// What a row of an album's page is to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlbumMark {
    /// One of the user's songs of the album.
    Mine,
    /// Another version of one of its songs («(Instrumental)», «(Off
    /// Vocal)», «(Remix)»), not chosen.
    Version,
    /// Left out of the user's songs of the album by hand.
    Left,
    /// Only in the album's tracklist.
    Catalog,
}

/// The day an album came out, or its month when the day is not known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReleaseDay {
    pub year: i32,
    /// 1–12.
    pub month: u8,
    /// 1–31.
    pub day: Option<u8>,
}

impl ReleaseDay {
    /// From a tag's or a service's date: `2019-05-17`, `2019-05`,
    /// `2019-05-17T00:00:00Z`, `2019/05/17`. `None` for a bare year,
    /// anything else, or a date in another year than `year` (when known: the
    /// date of a recording is not the release's).
    pub fn parse(date: &str, year: Option<i32>) -> Option<Self> {
        let date = date.trim();
        let date = date.split(['T', ' ']).next().unwrap_or(date);
        let mut parts = date.split(['-', '/', '.']);
        let y = parts.next().filter(|p| p.len() == 4)?.parse::<i32>().ok()?;
        let m = parts.next().filter(|p| (1..=2).contains(&p.len()))?.parse::<u8>().ok()?;
        let d = match parts.next() {
            Some(p) if (1..=2).contains(&p.len()) => Some(p.parse::<u8>().ok()?),
            Some(_) => return None,
            None => None,
        };
        if parts.next().is_some() || !(1000..=9999).contains(&y) || !(1..=12).contains(&m) {
            return None;
        }
        if d.is_some_and(|d| !(1..=31).contains(&d)) || year.is_some_and(|year| year != y) {
            return None;
        }
        Some(Self { year: y, month: m, day: d })
    }
}

/// One line of an album page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseLine {
    /// A track of [`ReleaseDetail::tracks`], by index.
    Track(usize),
    /// A place the album counts that has no track: «Трек 7 · нет файла»
    /// (`no` 0 when unknown).
    Hole { disc: u32, no: u32 },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ArtistCard {
    pub id: ArtistId,
    pub name: String,
    pub image: Option<Cover>,
    /// "Подписан" (liked).
    pub liked: bool,
    /// Their tracks in the library («12 треков» under the avatar); 0 when
    /// not counted.
    pub track_count: u32,
    /// Its name in Latin letters when its own is in a script the user may
    /// not read («Ayako Saso» for 佐宗綾子): shown beside the name.
    #[serde(default)]
    pub latin: Option<String>,
    /// A role (an anime character a voice actor sings as), not a person.
    #[serde(default)]
    pub character: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistDetail {
    pub card: ArtistCard,
    pub top_tracks: Vec<TrackRow>,
    pub albums: Vec<ReleaseCard>,
    pub singles_eps: Vec<ReleaseCard>,
    pub liked: bool,
    /// "Нет в библиотеке": the artist's albums in the services that the
    /// library has no file of ([`Command::LoadDiscography`]).
    pub discography: DiscographyView,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlaylistCard {
    pub id: PlaylistId,
    pub name: String,
    /// What to show: the custom picture, the 2x2 collage or the most common
    /// album cover, or a generated placeholder (see [`PlaylistCoverMode`]).
    pub cover: Option<Cover>,
    pub track_count: u32,
    pub pinned: bool,
    pub cover_mode: PlaylistCoverMode,
    /// The service whose playlist it follows (or is synced with), if any.
    pub service: Option<Service>,
}

impl PlaylistCard {
    /// Display name for the playlist in `loc`: localizes the virtual
    /// «Любимые» playlist while preserving user-defined playlist names.
    pub fn display_name_in(&self, loc: Locale) -> &str {
        if self.id == LIKED_PLAYLIST {
            nikgob_core::i18n::nav_in(loc).liked
        } else {
            &self.name
        }
    }

    /// Display name for the playlist in the active process locale.
    pub fn display_name(&self) -> &str {
        self.display_name_in(nikgob_core::i18n::locale())
    }
}

/// How a playlist's cover is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PlaylistCoverMode {
    /// A collage when the tracks have 4+ different album covers, else the most
    /// common cover, else a generated gradient.
    #[default]
    Smart,
    /// The first 4 different album covers, 2x2.
    Collage,
    /// A picture the user picked.
    Custom,
}

/// One day of «Скачано».
#[derive(Clone, Debug, PartialEq)]
pub struct DownloadedDay {
    /// «Сегодня», «Вчера», «28 октября».
    pub label: String,
    pub tracks: Vec<TrackRow>,
}

/// A deleted track in the "Корзина" view.
#[derive(Clone, Debug, PartialEq)]
pub struct TrashItem {
    pub track: TrackRow,
    /// "сегодня", "3 дн. назад".
    pub deleted: String,
    /// Days until it is removed for good.
    pub days_left: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistDetail {
    pub card: PlaylistCard,
    pub tracks: Vec<TrackRow>,
    pub total_duration: Duration,
    /// The link to a playlist of a service, if any (the mode for its menu,
    /// [`Command::SetPlaylistSync`]).
    pub sync: Option<PlaylistSyncView>,
}

/// The liked-tracks playlist ("Любимые") is virtual: this id stands for it.
pub const LIKED_PLAYLIST: PlaylistId = PlaylistId(nikgob_core::model::Uuid::nil());

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ReleaseSort {
    #[default]
    Artist,
    Title,
    Year,
    Added,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TrackSort {
    #[default]
    Artist,
    Title,
    Album,
    Added,
    Duration,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryView {
    pub playlists: Vec<PlaylistCard>,
    /// The albums liked as a whole («Любимые альбомы»), shown on a shelf
    /// of their own or among the playlists: each with the index in
    /// [`Self::playlists`] it goes before (its length: after them all), so
    /// both read latest first.
    pub liked_albums: Vec<(usize, ReleaseCard)>,
    pub releases: Vec<ReleaseCard>,
    pub artists: Vec<ArtistCard>,
    pub release_sort: ReleaseSort,
    pub track_count: usize,
    /// A window of the virtualized track list: rows `tracks_offset..`.
    pub tracks_offset: usize,
    pub tracks: Vec<TrackRow>,
    pub track_sort: TrackSort,
    pub loaded: bool,
    /// Deleted tracks ("Корзина"), most recent first; filled by [`Command::LoadTrash`].
    pub trash: Vec<TrashItem>,
    /// «Скачано»: tracks with a file by the day it came, the latest first;
    /// filled by [`Command::LoadDownloaded`] a page at a time
    /// ([`Command::LoadMoreDownloaded`]).
    pub downloaded: Vec<DownloadedDay>,
    /// How many tracks «Скачано» has in all (more than loaded while there
    /// are pages left).
    pub downloaded_total: usize,
    /// The «Всё» overview's rows: the latest albums in, and the most played.
    pub recently_added: Vec<ReleaseCard>,
    pub often_played: Vec<ReleaseCard>,
    /// The library filter on «Треки» ([`Command::FilterTracks`]): the query
    /// it answers and its tracks, best first.
    pub track_hits: Option<(String, Vec<TrackRow>)>,
}

/// Everything a design system needs to draw a frame.
/// Releases that are editions of one album (one MusicBrainz release
/// group), offered to fold into one; nothing merges without a yes.
#[derive(Clone, Debug, PartialEq)]
pub struct EditionOffer {
    pub id: u64,
    pub artist: String,
    /// The fullest edition: the others would join it.
    pub keep: String,
    pub others: Vec<String>,
    /// Songs both have (they would become one track each) and songs only
    /// the others have (they would join the album).
    pub same_songs: u32,
    pub new_songs: u32,
}

/// What the user does not want: tracks disliked and artists not to
/// recommend, newest first, with what the list in Settings shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DislikesView {
    pub tracks: Vec<DislikedTrack>,
    pub artists: Vec<(ArtistId, String)>,
}

impl DislikesView {
    pub fn track(&self, id: TrackId) -> bool {
        self.tracks.iter().any(|t| t.id == id)
    }

    pub fn artist(&self, id: ArtistId) -> bool {
        self.artists.iter().any(|a| a.0 == id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DislikedTrack {
    pub id: TrackId,
    pub title: String,
    pub artist: String,
}

/// How much of the library has its sound analysed (the quiet «Анализирую
/// звук библиотеки · 40%» lines): files, and stream-only tracks that are
/// liked or in a playlist, separately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnalysisProgress {
    pub files_done: u32,
    pub files_total: u32,
    pub streams_done: u32,
    pub streams_total: u32,
}

impl AnalysisProgress {
    /// Everything analysed (also true for an empty library).
    pub fn complete(&self) -> bool {
        self.files_done >= self.files_total && self.streams_done >= self.streams_total
    }

    /// Done share of files and streams together, 0..=1 (1 when there is
    /// nothing to analyse).
    pub fn fraction(&self) -> f32 {
        let total = self.files_total + self.streams_total;
        if total == 0 {
            1.0
        } else {
            ((self.files_done + self.streams_done) as f32 / total as f32).min(1.0)
        }
    }
}
