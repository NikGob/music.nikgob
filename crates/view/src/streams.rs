//! The streaming part of the view model (stage 3, docs/stage3.md): accounts,
//! the stream sections of the unified search, album / playlist / artist pages
//! of a service, the wishlist, sharing.
//!
//! Stream items are keyed by [`RemoteId`]: they need no library row to be
//! shown or played. A track gets a library row ([`StreamTrackRow::track`]) once
//! it is played, liked or put in a playlist; the runtime then reports which
//! library track it is, so the UI can mark it as liked / in the library.

use std::time::Duration;

pub use nikgob_core::model::{
    WishlistId, WishlistOrigin, WishlistScope, WishlistStatus,
};
pub use nikgob_core::source::SourceKey;
pub use nikgob_core::streaming::{
    Account, EntityKind, LoginInput, LoginPrompt, PlaylistSyncMode, RemoteId, Service, SoloistAction,
    SoloistPhase, SoloistStatus, TrackKind,
};

use crate::{ArtistId, PlaylistId, ReleaseDay, ReleaseId, ReleaseType, StreamArtistLinks, TrackId};

// ---------------------------------------------------------------- accounts

/// One streaming service as the settings show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountView {
    pub service: Service,
    /// A backend for it is running (the app registered one).
    pub registered: bool,
    /// Switched on in the settings ([`crate::Prefs::spotify_enabled`] /
    /// [`crate::Prefs::youtube_enabled`]).
    pub enabled: bool,
    /// What the service says about the login (cheap, polled while busy).
    pub account: Account,
    /// The service can be used now: registered, switched on, and (Spotify)
    /// connected. A Spotify that is not connected is asked nothing at all
    /// (it needs Premium, docs/stage3.md §0); the search can leave it out.
    pub usable: bool,
    /// A login is running ([`crate::Command::StreamLogin`]).
    pub busy: bool,
    /// The last login error, in Russian ("Порт 8989 занят", …).
    pub error: Option<String>,
    /// The account can play sound now (Spotify needs Premium and the
    /// playback login; otherwise its tracks play from YouTube Music).
    pub audio: bool,
    /// The service keeps refusing this account's sound (Spotify refused
    /// its audio keys): its tracks play from YouTube Music until this
    /// local time, «14:05», unless asked again
    /// ([`crate::Command::RetryStreamAudio`]). `None` when it plays.
    pub audio_refused_until_hm: Option<String>,
    /// The service asked us to slow down: its calls are paused until then
    /// (unix ms, rounded up to the minute), e.g. «YouTube Music просит
    /// подождать до 14:05». `None` when not paused.
    pub paused_until: Option<i64>,
    /// The same as local time, «14:05», for the UI to say.
    pub paused_until_hm: Option<String>,
    /// What the service's own suggestions give our mixes, a quiet line:
    /// «Подсказки: 48 треков · обновлено в 14:10», or why there are none
    /// (docs/stage4.md §12.3). `None`: the service has no suggestions.
    pub suggestions: Option<String>,
    /// A feed of suggestions needs a new sign-in: «Переподключи Spotify,
    /// чтобы брать твой топ».
    pub suggestions_note: Option<String>,
    /// Spotify Soloist, for a service that has one.
    pub soloist: Option<SoloistStatus>,
}

// ---------------------------------------------------------------- rows

/// A track of a service: a search hit, an album or playlist row.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StreamTrackRow {
    pub remote: RemoteId,
    pub title: String,
    pub artist: String,
    /// Every artist the service lists (a link on each name of `artist`).
    pub artists: StreamArtistLinks,
    pub album: Option<String>,
    pub album_id: Option<RemoteId>,
    pub duration: Option<Duration>,
    pub disc_no: u32,
    pub track_no: u32,
    /// Largest cover image (an `https://` url).
    pub cover_url: Option<String>,
    pub kind: TrackKind,
    /// The service lists it but will not play it (region, withdrawn).
    pub playable: bool,
    /// The library track it is, when it has a row (played, liked, in a
    /// playlist, or the same song as a library track).
    pub track: Option<TrackId>,
    /// That library track is liked.
    pub liked: bool,
    /// It is in the library (liked, in a playlist, or has a file).
    pub in_library: bool,
    /// The library has a file of it.
    pub has_local: bool,
    /// On an album page: the library album the track is on when it is not
    /// the page's own («у тебя в «… (Deluxe)»»).
    pub elsewhere: Option<String>,
    /// The other services the same song was found in (merged into this row).
    pub also_on: Vec<Service>,
}

impl StreamTrackRow {
    /// The first (main) artist.
    pub fn artist_id(&self) -> Option<&RemoteId> {
        self.artists.main()
    }
}

/// An album of a service (search hit, discography, artist page).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StreamAlbumCard {
    pub remote: RemoteId,
    pub title: String,
    pub artist: String,
    /// Every artist the service lists (a link on each name of `artist`).
    pub artists: StreamArtistLinks,
    pub year: Option<i32>,
    pub release_type: ReleaseType,
    pub cover_url: Option<String>,
    pub total_tracks: Option<u32>,
    /// The library album it is linked to, if any.
    pub release: Option<ReleaseId>,
    /// How much of it the library has ([`AlbumHave::label`]).
    pub have: AlbumHave,
}

impl StreamAlbumCard {
    /// The first (main) artist.
    pub fn artist_id(&self) -> Option<&RemoteId> {
        self.artists.main()
    }
}

/// How much of an album the library has, counted one way for every place
/// that says it (search cards, discography, the album page of a service,
/// library cards): over the tracks of the linked library album.
///
/// * `local` — tracks with a file: "у тебя" when all, else "3 из 12";
/// * `in_library` — tracks in the library, files or not (a stream-only
///   track is in it while liked or in a playlist): with no file at all the
///   album is "в библиотеке" on a service's card, "стрим" on a library card;
/// * `total` — the larger of the album's track count and the tracks known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AlbumHave {
    pub local: u32,
    pub in_library: u32,
    pub total: u32,
}

impl AlbumHave {
    /// Every track has a file.
    pub fn complete(&self) -> bool {
        self.total > 0 && self.local >= self.total
    }

    /// The badge of a service's album card: "у тебя", "3 из 12", "в
    /// библиотеке" or nothing.
    pub fn label(&self) -> Option<String> {
        if self.complete() {
            Some("у тебя".to_owned())
        } else if self.local > 0 {
            Some(format!("{} из {}", self.local, self.total))
        } else if self.in_library > 0 {
            Some("в библиотеке".to_owned())
        } else {
            None
        }
    }

    /// The chip of a service's album page: "у тебя", "3 из 12 у тебя", or
    /// nothing (files only; "В библиотеке" is a chip of its own).
    pub fn files_label(&self) -> Option<String> {
        if self.complete() {
            Some("у тебя".to_owned())
        } else if self.local > 0 {
            Some(format!("{} из {} у тебя", self.local, self.total))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamArtistCard {
    pub remote: RemoteId,
    pub name: String,
    pub image_url: Option<String>,
    /// The library artist it is linked to, if any.
    pub artist: Option<ArtistId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamPlaylistCard {
    pub remote: RemoteId,
    pub name: String,
    pub owner: Option<String>,
    pub cover_url: Option<String>,
    pub total_tracks: Option<u32>,
    /// The user may change it in the service.
    pub editable: bool,
    /// The library playlist that follows it, if any.
    pub playlist: Option<PlaylistId>,
    /// How that playlist follows it.
    pub sync_mode: Option<PlaylistSyncMode>,
}

// ---------------------------------------------------------------- search

/// Which sources the unified search asks ([`crate::Command::Search`]); the
/// chips of the search page ([`crate::Command::SetSearchSources`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SearchSources {
    pub library: bool,
    pub spotify: bool,
    pub youtube: bool,
    /// The Deezer catalogue.
    pub deezer: bool,
    pub soundcloud: bool,
    /// The indexer (Prowlarr): slow, so off unless asked for.
    pub torrents: bool,
    /// The «Всё» page: the runtime also builds one merged answer
    /// ([`crate::SearchView::answer`]), asks each service its quick mixed
    /// search ([`nikgob_core::streaming::StreamService::search_top`]) and
    /// leaves out the services that cannot answer (Spotify without a
    /// login). Off (the default, and a service's chip): every chosen
    /// service is asked its full search, section by section.
    pub mixed: bool,
}

impl Default for SearchSources {
    fn default() -> Self {
        Self {
            library: true,
            spotify: true,
            youtube: true,
            deezer: true,
            soundcloud: true,
            torrents: false,
            mixed: false,
        }
    }
}

impl SearchSources {
    pub const fn service(&self, s: Service) -> bool {
        match s {
            Service::Spotify => self.spotify,
            Service::Youtube => self.youtube,
            Service::Deezer => self.deezer,
            Service::Soundcloud => self.soundcloud,
        }
    }

    /// The «Всё» page: the library and every service, merged.
    pub const fn all() -> Self {
        Self {
            library: true,
            spotify: true,
            youtube: true,
            deezer: true,
            soundcloud: true,
            torrents: false,
            mixed: true,
        }
    }
}

/// A track of the merged «Всё» answer: a library track (it wins over the
/// same song in a service), or a service's track.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchTrack {
    Library(crate::TrackRow),
    Stream(StreamTrackRow),
}

impl SearchTrack {
    pub fn title(&self) -> &str {
        match self {
            Self::Library(t) => &t.title,
            Self::Stream(t) => &t.title,
        }
    }

    pub fn artist(&self) -> &str {
        match self {
            Self::Library(t) => &t.artist,
            Self::Stream(t) => &t.artist,
        }
    }

    /// What [`crate::Command::PlaySearch`] plays for it.
    pub fn pick(&self) -> SearchPick {
        match self {
            Self::Library(t) => SearchPick::Library(t.id),
            Self::Stream(t) => SearchPick::Stream(t.remote.clone()),
        }
    }
}

/// One track of a mixed queue ([`crate::Command::PlaySearch`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SearchPick {
    Library(TrackId),
    Stream(RemoteId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchAlbum {
    Library(crate::ReleaseCard),
    Stream(StreamAlbumCard),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchArtist {
    Library(crate::ArtistCard),
    Stream(StreamArtistCard),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaylist {
    Library(crate::PlaylistCard),
    Stream(StreamPlaylistCard),
}

/// «Лучший результат»: whatever matches the query best.
#[derive(Clone, Debug, PartialEq)]
pub enum TopResult {
    Artist(SearchArtist),
    Track(SearchTrack),
    Album(SearchAlbum),
}

/// The one answer of the «Всё» page ([`SearchSources::mixed`]): the
/// library and every usable service merged, each song once (same ISRC, else
/// the same artist and title folded and a length within 3 s), ranked by how
/// well it matches the query. A service that cannot answer is simply left
/// out; one that waits out a rate limit gives [`Self::note`].
///
/// Rows do not jump while answers arrive: once the top is settled (the
/// first service answered plus a short grace for Deezer) [`Self::top`] and
/// the first [`TOP_TRACKS`] tracks keep their places; slower answers go
/// below them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchAnswer {
    /// The query these rows are for (the previous one stays on screen
    /// until the new one has something).
    pub query: String,
    pub top: Option<TopResult>,
    /// The top may still change (it is settled a moment after the first
    /// answer): its room is kept.
    pub top_pending: bool,
    pub tracks: Vec<SearchTrack>,
    pub albums: Vec<SearchAlbum>,
    pub artists: Vec<SearchArtist>,
    pub playlists: Vec<SearchPlaylist>,
    /// Some source has not answered yet.
    pub searching: bool,
    /// One quiet line: a service waits out a rate limit («YouTube Music
    /// отвечает с задержкой — результаты появятся сами»).
    pub note: Option<String>,
    /// Nothing came and a service really failed (no connection): why.
    pub error: Option<String>,
}

/// Tracks of [`SearchAnswer::tracks`] that stay in place once shown (the
/// list beside the top result).
pub const TOP_TRACKS: usize = 5;

impl SearchAnswer {
    pub fn is_empty(&self) -> bool {
        self.top.is_none()
            && self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

/// The results of one service for the current query. Sections arrive one by
/// one (each service answers on its own time); a newer query replaces them.
///
/// Rows are merged: a track the library has is shown only in the library
/// section ([`crate::SearchView::tracks`]), a song found in Spotify and in
/// YouTube Music only in the first (with [`StreamTrackRow::also_on`]). Same
/// song = same ISRC, else the same artist and title (folded like the
/// importer folds them) and a length within 3 s.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamSearchSection {
    pub service: Service,
    pub searching: bool,
    /// "Spotify не отвечает", "Нужно войти", … (a real failure; shown
    /// with «Повторить»).
    pub error: Option<String>,
    /// A quiet line, never with a button: the service rate-limited the
    /// query and the runtime asks again by itself («Spotify отвечает с
    /// задержкой — результаты появятся сами»), or it is overloaded for
    /// longer («…сейчас перегружен — ищу в YouTube Music»).
    pub note: Option<String>,
    pub tracks: Vec<StreamTrackRow>,
    pub albums: Vec<StreamAlbumCard>,
    pub artists: Vec<StreamArtistCard>,
    pub playlists: Vec<StreamPlaylistCard>,
}

impl StreamSearchSection {
    pub fn new(service: Service) -> Self {
        Self {
            service,
            searching: false,
            error: None,
            note: None,
            tracks: Vec::new(),
            albums: Vec::new(),
            artists: Vec::new(),
            playlists: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------- pages

/// An album, playlist or artist of a service, opened as a page without
/// adding anything to the library ([`crate::Route::Stream`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StreamPage {
    Album(RemoteId),
    Playlist(RemoteId),
    Artist(RemoteId),
}

impl StreamPage {
    pub fn remote(&self) -> &RemoteId {
        match self {
            Self::Album(r) | Self::Playlist(r) | Self::Artist(r) => r,
        }
    }
}

/// The content of the open [`StreamPage`] ([`crate::Command::LoadStream`]).
#[derive(Clone, Debug, PartialEq)]
pub struct StreamPageView {
    pub page: StreamPage,
    pub loading: bool,
    pub error: Option<String>,
    /// Album title, playlist name or artist name.
    pub title: String,
    /// Album artist, playlist owner; empty for an artist.
    pub subtitle: String,
    /// Album: its artists (a link on each name of `subtitle`).
    pub subtitle_artists: StreamArtistLinks,
    pub cover_url: Option<String>,
    pub year: Option<i32>,
    /// Album: the day it came out, when the service tells it.
    pub released: Option<ReleaseDay>,
    pub release_type: Option<ReleaseType>,
    /// Album / playlist tracks in order (empty for an artist).
    pub tracks: Vec<StreamTrackRow>,
    /// An artist's discography, newest first (empty otherwise).
    pub albums: Vec<StreamAlbumCard>,
    pub total_duration: Duration,
    /// Album: how much of it the library has, counted over the page's own
    /// rows once they are known (else the same count as its card).
    pub have: AlbumHave,
    /// Album: the linked library album's title and its own count
    /// («В библиотеке: … · 13 из 14»).
    pub library: Option<(String, AlbumHave)>,
    /// Album: every track is liked. Artist: followed (library artist liked).
    pub liked: bool,
    /// The linked library album / artist / playlist.
    pub release: Option<ReleaseId>,
    pub artist: Option<ArtistId>,
    pub playlist: Option<PlaylistId>,
    /// Playlist: the user may edit it in the service.
    pub editable: bool,
}

impl StreamPageView {
    pub fn loading(page: StreamPage) -> Self {
        Self {
            page,
            loading: true,
            error: None,
            title: String::new(),
            subtitle: String::new(),
            subtitle_artists: StreamArtistLinks::default(),
            cover_url: None,
            year: None,
            released: None,
            release_type: None,
            tracks: Vec::new(),
            albums: Vec::new(),
            total_duration: Duration::ZERO,
            have: AlbumHave::default(),
            library: None,
            liked: false,
            release: None,
            artist: None,
            playlist: None,
            editable: false,
        }
    }
}

/// The "Нет в библиотеке" section of a library artist page: albums of the
/// artist in the services the library has no file of ([`crate::Command::LoadDiscography`],
/// filled by the discography part of stage 3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscographyView {
    pub loading: bool,
    pub error: Option<String>,
    pub albums: Vec<StreamAlbumCard>,
}

/// How a library playlist follows its playlist in a service
/// ([`crate::PlaylistDetail::sync`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistSyncView {
    pub remote: RemoteId,
    pub mode: PlaylistSyncMode,
    /// When it was last synced ("5 мин назад"); `None` = not yet.
    pub synced: Option<String>,
    /// A sync is running.
    pub syncing: bool,
    /// The last sync failed: why, in Russian.
    pub error: Option<String>,
}

// ---------------------------------------------------------------- wishlist

/// One wishlist item ("Загрузки → Wishlist").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WishlistRow {
    pub id: WishlistId,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub track: Option<TrackId>,
    pub release: Option<ReleaseId>,
    pub artist_id: Option<ArtistId>,
    pub status: WishlistStatus,
    pub scope: WishlistScope,
    pub origin: WishlistOrigin,
    /// Why it failed, what was chosen ("Не нашёл раздачу", "FLAC · 12 сидов").
    pub note: Option<String>,
    /// "поищу снова через 6 дн." for an item without a torrent.
    pub next_try: Option<String>,
    pub attempts: u32,
    /// "Сохранить с YouTube" makes sense (the track streams from YouTube or
    /// can be found there, and YouTube Music is on).
    pub can_save_from_youtube: bool,
    /// The album's cover (a file or a service's https url).
    pub cover: Option<crate::Cover>,
    /// While downloading: how much of its torrent is here, 0..=100.
    pub progress: Option<u8>,
    /// [`WishlistStatus::Confirm`] because downloaded files wait in
    /// «Подтверди» (not a torrent to choose).
    pub confirm_files: bool,
    /// A downloaded album: its tracks still without a file.
    pub missing: u32,
    /// The track has a copy saved from YouTube (low quality): the item
    /// stays to find a better one, it is not «Скачано» (docs/stage4.5.md).
    pub has_copy: bool,
    /// Where the file comes from while it downloads («Qobuz», «khinsider»,
    /// the tracker of a torrent).
    pub source: Option<String>,
    /// An item that looks for a better copy of a file the library has: its
    /// state in words («MP3 128 · ищу lossless»); `None` for the others.
    pub upgrade: Option<String>,
}

/// «Искать качество лучше»: how often a file below
/// [`crate::Prefs::upgrade_below`] is searched for a better copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UpgradeEvery {
    Day,
    #[default]
    Week,
    Month,
    Never,
}

impl UpgradeEvery {
    pub const ALL: [Self; 4] = [Self::Day, Self::Week, Self::Month, Self::Never];

    /// The time between two searches of one file; `None`: never.
    pub const fn period(self) -> Option<Duration> {
        const DAY: u64 = 24 * 3600;
        match self {
            Self::Day => Some(Duration::from_secs(DAY)),
            Self::Week => Some(Duration::from_secs(7 * DAY)),
            Self::Month => Some(Duration::from_secs(30 * DAY)),
            Self::Never => None,
        }
    }
}

/// What quality is good enough: a file below it is searched for again,
/// and only a copy at least this good replaces it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UpgradeBar {
    /// MP3 320 and what is as good (Opus 160, AAC 256) or better.
    Mp3_320,
    /// FLAC and the other lossless formats only.
    #[default]
    Lossless,
}

impl UpgradeBar {
    /// The lowest quality score that is good enough.
    pub const fn score(self) -> i32 {
        match self {
            Self::Mp3_320 => nikgob_core::model::score::MP3_320,
            Self::Lossless => nikgob_core::model::score::CD_LOSSLESS,
        }
    }

    /// What is looked for, in words: «lossless», «MP3 320 или лучше».
    pub const fn wanted(self) -> &'static str {
        match self {
            Self::Mp3_320 => "MP3 320 или лучше",
            Self::Lossless => "lossless",
        }
    }
}

/// Items of one album or one artist, for the grouped view and its bulk
/// buttons ("скачать альбом / артиста").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WishlistGroup {
    /// Album title or artist name.
    pub title: String,
    /// The album's artist; empty for an artist group.
    pub subtitle: String,
    pub release: Option<ReleaseId>,
    pub artist: Option<ArtistId>,
    pub items: Vec<WishlistId>,
    /// Items still waiting for "скачать".
    pub waiting: u32,
}

/// The wishlist ([`crate::Command::LoadWishlist`]; refreshed as items move).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WishlistView {
    pub loaded: bool,
    /// Every item that is not done, newest first; dismissed ones last.
    pub items: Vec<WishlistRow>,
    pub by_album: Vec<WishlistGroup>,
    pub by_artist: Vec<WishlistGroup>,
    /// How many items are in each status (statuses with none are left out).
    pub counts: Vec<(WishlistStatus, u32)>,
    /// Some download source is on and set up: items search by themselves.
    pub can_search: bool,
}

/// One download source as «Источники» lists it (docs/stage4.5.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceView {
    pub key: SourceKey,
    pub name: String,
    /// The app has it (Soulseek only once its client is in the build;
    /// torrents once Prowlarr is set up).
    pub available: bool,
    /// How it went lately, in words («Не отвечает», «Просит подождать до
    /// 14:20»); `None` when all is well.
    pub status: Option<String>,
}

/// `[soulseek]` as «Источники» shows it: never the password or the token,
/// only whether they are saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoulseekView {
    /// On a server or a small device (`nikgob-slsk`), not in the player.
    pub remote: bool,
    pub username: String,
    pub password_saved: bool,
    pub remote_url: String,
    pub token_saved: bool,
    pub share: bool,
    pub upload_limit_kbps: u32,
    /// The client (or the link to the service) runs.
    pub running: bool,
}

impl SoulseekView {
    /// Enough is saved to start it.
    pub fn configured(&self) -> bool {
        if self.remote { !self.remote_url.is_empty() } else { !self.username.is_empty() && self.password_saved }
    }
}

/// What «Сохранить» under Soulseek sends.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoulseekForm {
    pub remote: bool,
    pub username: String,
    /// Empty: keep the saved one.
    pub password: String,
    pub remote_url: String,
    /// Empty: keep the saved one.
    pub remote_token: String,
}

impl WishlistView {
    pub fn count(&self, status: WishlistStatus) -> u32 {
        self.counts
            .iter()
            .find(|(s, _)| *s == status)
            .map_or(0, |(_, n)| *n)
    }
}

/// Which wishlist items a bulk action takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WishlistSelect {
    Items(Vec<WishlistId>),
    /// Every item of this album.
    Album(ReleaseId),
    /// Every item of this artist.
    Artist(ArtistId),
    /// Every waiting item.
    AllWaiting,
}

/// What the user hands in for a wishlist item that has no torrent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WishlistAttach {
    TorrentFile(std::path::PathBuf),
    Magnet(String),
    /// Audio files (a single file is taken as the item's track whatever its tags say).
    Files(Vec<std::path::PathBuf>),
    Folder(std::path::PathBuf),
}

// ---------------------------------------------------------------- sharing

/// What to share ([`crate::Command::Share`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ShareTarget {
    Track(TrackId),
    Release(ReleaseId),
    StreamTrack(RemoteId),
    StreamAlbum(RemoteId),
}

/// The last share: a song.link / album.link page, or "Исполнитель —
/// Название" when no service has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareView {
    pub target: ShareTarget,
    /// Still looking it up.
    pub busy: bool,
    /// The aggregator page (or the service page when Odesli does not answer).
    pub link: Option<String>,
    /// What was copied (the link, or the text fallback).
    pub text: String,
}

/// The «Найти песню…» dialog of a stream track ([`crate::Command::FindSong`]):
/// songs of YouTube Music and Deezer (no clips) to replace the track with.
#[derive(Clone, Debug, PartialEq)]
pub struct FindSongView {
    pub track: TrackId,
    /// Opened for a clip («Найти песню…»), else for another version of a
    /// stream track («Другая версия…»).
    pub clip: bool,
    /// The track as it is shown (a clip by its cleaned title).
    pub title: String,
    pub artist: String,
    /// The track's own length, to compare the results with.
    pub duration: Option<Duration>,
    /// What the results are for.
    pub query: String,
    /// Bumped each time the dialog opens anew (the UI resets its field).
    pub opened: u64,
    pub loading: bool,
    pub results: Vec<StreamTrackRow>,
    /// Nothing could be asked (no connection, the services off).
    pub error: Option<String>,
    /// Why «И в YouTube Music» cannot be chosen now, if it cannot.
    pub youtube_off: Option<String>,
    /// A pick is being replaced (a Deezer song looks for its YouTube song).
    pub busy: bool,
}

/// Text for the system clipboard. The app copies it when `id` changes (the
/// runtime never touches the clipboard itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardRequest {
    pub id: u64,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::AlbumHave;

    fn have(local: u32, in_library: u32, total: u32) -> AlbumHave {
        AlbumHave { local, in_library, total }
    }

    #[test]
    fn one_wording_for_how_much_of_an_album_is_here() {
        assert_eq!(have(0, 0, 4).label(), None);
        assert_eq!(have(0, 2, 4).label().as_deref(), Some("в библиотеке"));
        assert_eq!(have(0, 2, 4).files_label(), None, "stream only: no files");
        assert_eq!(have(3, 3, 12).label().as_deref(), Some("3 из 12"));
        assert_eq!(have(3, 3, 12).files_label().as_deref(), Some("3 из 12 у тебя"));
        assert_eq!(have(4, 4, 4).label().as_deref(), Some("у тебя"));
        assert_eq!(have(4, 4, 4).files_label().as_deref(), Some("у тебя"));
        assert_eq!(have(0, 0, 0).label(), None);
    }
}
