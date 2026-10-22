//! The view model every design system draws from.
//!
//! The runtime (library, player, import, downloads) fills a [`ViewState`];
//! a design system reads it each frame and answers with [`Command`]s. Neither
//! side knows how the other works: MD3E, Nothing and Liquid Glass are
//! different layouts over the same state.

use std::path::PathBuf;
use std::time::{Duration, Instant};

mod credit;
mod recs;
mod streams;

pub use nikgob_core::lyrics::{LyricLine, LyricWord, LyricsSource, LyricsSync};
pub use nikgob_core::model::{
    ArtistId, LikeTarget, PlaylistId, ReleaseId, ReleaseType, SourceKind, TrackId,
};
pub use credit::*;
pub use recs::*;
pub use streams::*;

/// A cover image on disk plus the palette seed computed at import.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Cover {
    pub path: PathBuf,
    /// ARGB seed for the colour scheme, if the cover is colourful enough.
    pub seed: Option<u32>,
    /// A tiny (16×16) downscale of the cover; stretched with linear
    /// filtering it is the blurred ambient background.
    pub ambient: Option<PathBuf>,
}

impl Cover {
    /// A cover of a streaming service, drawn from its `https://` url (the
    /// app's web image loader fetches it; a placeholder while it cannot).
    pub fn web(url: impl Into<String>) -> Self {
        Self {
            path: PathBuf::from(url.into()),
            seed: None,
            ambient: None,
        }
    }

    /// The url of a [`Self::web`] cover.
    pub fn web_url(&self) -> Option<&str> {
        self.path
            .to_str()
            .filter(|p| p.starts_with("https://") || p.starts_with("http://"))
    }

    /// The cover file on disk (`None` for a web cover).
    pub fn file(&self) -> Option<&std::path::Path> {
        self.web_url().is_none().then_some(self.path.as_path())
    }

    /// The egui image URI for this file (a web cover: its url).
    pub fn uri(&self) -> String {
        match self.web_url() {
            Some(url) => url.to_owned(),
            None => file_uri(&self.path),
        }
    }

    pub fn ambient_uri(&self) -> Option<String> {
        self.ambient.as_deref().map(file_uri)
    }
}

/// `file://` URI egui's file loader understands (forward slashes on Windows).
pub fn file_uri(path: &std::path::Path) -> String {
    let p = path.display().to_string().replace('\\', "/");
    // egui_extras wants `file:///C:/...` on Windows (empty host).
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// Live audio levels for ambient effects, read by the UI only while visible.
pub trait Levels {
    /// Bass energy 20–150 Hz, smoothed, 0..1.
    fn bass(&self) -> f32;
    /// Log-spaced spectrum bands, 0..1 each; fills `out`.
    fn bands(&self, out: &mut [f32]);
}

/// No audio analysis available.
pub struct NoLevels;

impl Levels for NoLevels {
    fn bass(&self) -> f32 {
        0.0
    }
    fn bands(&self, out: &mut [f32]) {
        out.fill(0.0);
    }
}

/// One row of a track list.
#[derive(Clone, Debug, PartialEq)]
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
}

impl TrackRow {
    /// The first (main) credited artist.
    pub fn artist_id(&self) -> Option<ArtistId> {
        self.artists.main().copied()
    }
}

#[derive(Clone, Debug, PartialEq)]
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistCard {
    pub id: ArtistId,
    pub name: String,
    pub image: Option<Cover>,
    /// "Подписан" (liked).
    pub liked: bool,
    /// Their tracks in the library («12 треков» under the avatar); 0 when
    /// not counted.
    pub track_count: u32,
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

#[derive(Clone, Debug, PartialEq)]
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

/// How a playlist's cover is chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Stopped,
    Playing,
    Paused,
    /// Waiting for the decoder or a stream.
    Buffering,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    pub const fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

/// Playback as of `at`; the UI extrapolates while playing so progress moves
/// smoothly without a state update per frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerView {
    pub status: Status,
    pub current: Option<TrackRow>,
    pub position: Duration,
    pub at: Instant,
    pub duration: Option<Duration>,
    /// 0..1 perceptual.
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    /// What the current queue was started from ("Играет из «…»").
    pub context: Option<String>,
    /// The service the current track's sound comes from (`None` for a local
    /// file): the "звук: YouTube" label, e.g. for a Spotify track played
    /// from YouTube Music because Spotify has no sound for this account.
    pub audio_service: Option<Service>,
    /// What the sound of a stream actually is ("Opus ~160 кбит/с", "AAC
    /// ~128 кбит/с", "Vorbis 320 кбит/с"): the source that plays, not the
    /// track's best source. `None` for a local file (its row says).
    pub audio_quality: Option<String>,
}

impl Default for PlayerView {
    fn default() -> Self {
        Self {
            status: Status::Stopped,
            current: None,
            position: Duration::ZERO,
            at: Instant::now(),
            duration: None,
            volume: 0.8,
            muted: false,
            shuffle: false,
            repeat: Repeat::Off,
            context: None,
            audio_service: None,
            audio_quality: None,
        }
    }
}

impl PlayerView {
    /// The header line of Now Playing: "Играет из альбома «…»", "Играет
    /// из поиска Spotify: лёд", "Играет из «Библиотека»" — never quotes in
    /// quotes. A context that is a phrase ("альбома «…»", "поиска: …") is
    /// used as it is, a bare name is quoted; old saved contexts ("Поиск
    /// «…»", "Spotify · «…»") read the new way.
    pub fn context_line(&self) -> String {
        let Some(c) = self.context.as_deref().map(str::trim).filter(|c| !c.is_empty()) else {
            return "Сейчас играет".to_owned();
        };
        let unquote = |s: &str| s.trim().trim_start_matches('«').trim_end_matches('»').to_owned();
        const PHRASES: [&str; 5] = ["поиска", "альбома ", "плейлиста ", "исполнителя ", "микса "];
        if PHRASES.iter().any(|p| c.starts_with(p)) || c == "библиотеки" {
            return format!("Играет из {c}");
        }
        if let Some(q) = c.strip_prefix("Поиск ") {
            return format!("Играет из поиска: {}", unquote(q));
        }
        if let Some((service, q)) = c.split_once(" · ") {
            return format!("Играет из поиска {service}: {}", unquote(q));
        }
        if c.contains('«') || c.contains('»') {
            return format!("Играет из {c}");
        }
        format!("Играет из «{c}»")
    }

    /// Position now, extrapolated from the last update.
    pub fn position_now(&self) -> Duration {
        let p = if self.status == Status::Playing {
            self.position + self.at.elapsed()
        } else {
            self.position
        };
        match self.duration {
            Some(d) => p.min(d),
            None => p,
        }
    }

    pub fn fraction_now(&self) -> f32 {
        match self.duration {
            Some(d) if !d.is_zero() => {
                (self.position_now().as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueView {
    /// What played before the current track (the last few, oldest first);
    /// "previous" walks back through these.
    pub prev: Vec<QueueEntry>,
    pub current: Option<TrackRow>,
    /// The current track's queue entry id (a drop target in the queue).
    pub current_entry: Option<u64>,
    /// Upcoming tracks in play order; `entry` ids are stable for drag & drop.
    pub next: Vec<QueueEntry>,
    /// The queue is a radio or an endless mix: its title («Радио · Кино»)
    /// and whether it is looking for more right now. Any other playback
    /// ends it.
    pub radio: Option<RadioView>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub entry: u64,
    pub track: TrackRow,
}

/// Synced lyrics for the current track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LyricsView {
    pub track: Option<TrackId>,
    pub lines: Vec<LyricLine>,
    /// Unsynced fallback text.
    pub plain: Option<String>,
    pub loading: bool,
    /// Where the shown text comes from.
    pub source: Option<LyricsSource>,
    /// The source was picked by the user (not chosen automatically).
    pub picked: bool,
    /// Every source that has, or may still get, a text for this track.
    pub sources: Vec<LyricsOption>,
}

/// One source in the lyrics source picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsOption {
    pub source: LyricsSource,
    pub state: LyricsAvail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LyricsAvail {
    Loading,
    /// Looked up, nothing there.
    Missing,
    Ready(LyricsSync),
}

/// Technical details of the current track ("Инфо" tab).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackInfo {
    pub track: Option<TrackId>,
    pub quality: Option<String>,
    pub source: Option<String>,
    pub path: Option<PathBuf>,
    pub tags: Vec<(String, String)>,
}

/// One card of «Продолжить»: an album of the library, or an album of a
/// service that was streamed without joining the library (it opens the
/// service's page).
#[derive(Clone, Debug, PartialEq)]
pub enum ContinueCard {
    Release(ReleaseCard),
    Stream(StreamAlbumCard),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeView {
    pub greeting_hour: u8,
    /// The library tracks of «Твой микс» (the hero's tracks; it plays with
    /// [`Command::PlayMix`] [`MixId::Daily`]).
    pub mix: Vec<TrackRow>,
    /// «Твой микс»: «30 треков · обновлён сегодня» and four covers. `None`
    /// until the recommendations are built the first time (or with an
    /// empty library).
    pub hero: Option<MixCard>,
    /// «Дейлист» ([`MixId::Daylist`]), the bento's big tile: title «Тягучий
    /// вечер четверга», subtitle «Massive Attack, Kevin Penkin и другие ·
    /// до 23:00», four covers. `None` until the recommendations are built
    /// (or with an empty library).
    pub daylist: Option<MixCard>,
    /// The bento's smaller tiles in [`MixId::TILES`] order, each only when
    /// it has enough songs (title «На повторе», subtitle «25 треков · за
    /// месяц», «октябрь 2025 · 18 треков»).
    pub tiles: Vec<MixCard>,
    /// «Под настроение»: the six moods, in the order for the part of the
    /// day (night: Спокойное, Мрачное first). Empty without any sound
    /// analysis.
    pub moods: Vec<MixCard>,
    /// «Твои миксы»: mixes by style, named by their artists («A, B и
    /// другие»), with a cover collage. Empty while too little is listened.
    pub style_mixes: Vec<MixCard>,
    /// «Новое для тебя»: up to 30 songs of the services new to the user
    /// (they play from the stream; «Слушать всё» is [`MixId::Discover`]).
    pub discover: Vec<StreamTrackRow>,
    /// «Новые релизы»: albums of the last 60 days by artists you listen to.
    pub new_releases: Vec<StreamAlbumCard>,
    /// Albums played lately, most recent first: library albums and streamed
    /// ones.
    pub continue_listening: Vec<ContinueCard>,
    /// «Миксы исполнителей»: a click plays the artist's radio
    /// ([`MixId::Artist`]).
    pub artist_mixes: Vec<ArtistCard>,
    pub long_unplayed: Vec<ReleaseCard>,
    pub recently_added: Vec<ReleaseCard>,
}

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
    /// The «Всё» overview's rows: the latest albums in, and the most played.
    pub recently_added: Vec<ReleaseCard>,
    pub often_played: Vec<ReleaseCard>,
    /// The library filter on «Треки» ([`Command::FilterTracks`]): the query
    /// it answers and its tracks, best first.
    pub track_hits: Option<(String, Vec<TrackRow>)>,
}

/// The unified search ([`Command::Search`]): the library, Deezer, YouTube
/// Music, Spotify, torrents. The library part is filled at once, the
/// services' sections as they answer; on the «Всё» page
/// ([`SearchSources::mixed`]) also one merged answer ([`Self::answer`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchView {
    pub query: String,
    /// Library hits (and stream hits that turned out to be library tracks).
    pub tracks: Vec<TrackRow>,
    pub releases: Vec<ReleaseCard>,
    pub artists: Vec<ArtistCard>,
    pub playlists: Vec<PlaylistCard>,
    pub searching: bool,
    /// Which sources are asked (the chips).
    pub sources: SearchSources,
    /// One section per registered, enabled and chosen service, Spotify first.
    pub streams: Vec<StreamSearchSection>,
    /// The «Всё» page: the library and the services as one answer.
    pub answer: SearchAnswer,
    /// Torrent search (Prowlarr) runs only on request (or with the torrents
    /// chip on): it is slow.
    pub torrents: TorrentSearch,
}

/// Results of [`Command::SearchTorrents`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TorrentSearch {
    pub query: String,
    pub searching: bool,
    /// "Prowlarr не настроен", network errors…
    pub error: Option<String>,
    pub results: Vec<TorrentResult>,
}

/// One release found by the indexer.
#[derive(Clone, Debug, PartialEq)]
pub struct TorrentResult {
    /// Index for [`Command::DownloadTorrent`] / [`Command::ListTorrentFiles`].
    pub index: usize,
    pub title: String,
    /// "FLAC", "FLAC 24/96", "MP3 320"… guessed from the title.
    pub format: Option<String>,
    pub size_bytes: Option<u64>,
    pub seeders: Option<u32>,
    pub indexer: String,
    /// Best pick for automatic download (format → release match → seeders).
    pub recommended: bool,
}

/// Files of a torrent, for "Выбрать файлы…".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TorrentFiles {
    /// Which search result (or `None` for a dropped .torrent / magnet,
    /// identified by `job`).
    pub result: Option<usize>,
    pub job: Option<u64>,
    pub name: String,
    pub loading: bool,
    pub error: Option<String>,
    /// (index in the torrent, path inside it, size).
    pub files: Vec<(usize, String, u64)>,
}

/// Editable tags of one track (tag editor dialog).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagEdit {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    pub track_no: String,
    pub disc_no: String,
    pub genre: String,
    /// Filled by MusicBrainz suggestions, written along (not shown).
    pub recording_mbid: String,
    pub release_mbid: String,
}

/// State of the tag editor for one track.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagEditor {
    pub track: Option<TrackId>,
    /// Current values read from the file.
    pub current: TagEdit,
    pub loading: bool,
    /// MusicBrainz suggestions: (label, values).
    pub suggestions: Vec<(String, TagEdit)>,
    pub looking_up: bool,
    pub error: Option<String>,
}

/// One torrent being seeded ("Раздача" tab).
#[derive(Clone, Debug, PartialEq)]
pub struct SeedRow {
    pub id: u64,
    pub title: String,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    /// When it will be cleaned up ("через 12 дн.").
    pub expires: Option<String>,
}

/// An import or download job shown in "Загрузки".
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    pub title: String,
    pub subtitle: String,
    /// 0..1, None = indeterminate.
    pub progress: Option<f32>,
    pub rate: Option<String>,
    pub eta: Option<String>,
    pub paused: bool,
    /// Pause / cancel make sense (torrents); a folder import runs to the end.
    pub controllable: bool,
    pub error: Option<String>,
}

/// An item waiting for the user to pick a match ("Подтверди").
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub id: u64,
    pub title: String,
    pub origin: String,
    pub options: Vec<ConfirmOption>,
    /// What `ConfirmChoice { option: None }` does ("Пропустить",
    /// "Оставить теги файла").
    pub skip_label: String,
    /// Button for the chosen option ("Скачать", "Импортировать").
    pub accept_label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmOption {
    pub label: String,
    pub detail: String,
    pub recommended: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DownloadsView {
    pub active: Vec<Job>,
    pub confirm: Vec<Confirm>,
    pub history: Vec<Job>,
    pub seeding: Vec<SeedRow>,
    /// Total uploaded / limit for the "Раздача" header.
    pub seeded_bytes: u64,
    /// Files of a torrent the user is choosing from.
    pub files: Option<TorrentFiles>,
    /// Liked stream tracks waiting for (or on their way to) a file.
    pub wishlist: WishlistView,
}

/// A transient message ("Добавлено в Любимые · Отменить").
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub action: Option<(String, Command)>,
    /// A place the toast's button takes the user to (used when there is no
    /// `action`): «Открыть» → «Хочу скачать», «Войти» → the service's row.
    pub open: Option<(String, Link)>,
    pub at: Instant,
    /// What the message is about, for its icon and accent.
    pub kind: ToastKind,
}

/// A place in the app a message can lead to; the design system navigates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Route(Route),
    /// A row of the settings (each design system knows where it is).
    Setting(SettingLink),
}

/// Settings rows other parts of the app point to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingLink {
    /// The Spotify account (login).
    Spotify,
    /// «Звук из Spotify» (the playback login).
    SpotifyPlayback,
    /// The YouTube Music account.
    Youtube,
    /// Prowlarr and the torrents.
    Torrents,
    /// «Что качать по лайку».
    LikeDownload,
}

/// What a toast reports: drawn with its own icon and accent colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToastKind {
    #[default]
    Info,
    /// Something got done ("Сохранено", "Добавлено в очередь").
    Success,
    /// Something started and goes on in the background ("Добавляю раздачу…").
    Working,
    /// Something failed or is not available.
    Error,
}

impl ToastKind {
    /// Guesses the kind from the (Russian) message text, so call sites only
    /// say what happened.
    pub fn guess(text: &str) -> Self {
        let t = text.to_lowercase();
        let any = |words: &[&str]| words.iter().any(|w| t.contains(w));
        if any(&[
            "ошибка", "не получилось", "не удалось", "недоступ", "не читается", "нельзя", "не запустил",
            "некуда", "нечего", "нет файла", "нет локального", "это не ", "устарел", "не скачать", "не всё",
            "не нашёл", "не нашел", "нужно войти", "не подключ", "нет связи",
        ]) {
            Self::Error
        } else if t.ends_with('…') || any(&["начался", "качается", "добавляю"]) {
            Self::Working
        } else if any(&["сохранен", "сохранён", "добавлено", "скачано", "восстановлено", "удалён", "удалено", "следующим", "в любимых", "убран", "в корзине", "объединено", "скопирован", "заменено", "вошёл", "вход выполнен", "подключён", "включён", "в «хочу скачать»", "в библиотеке"]) {
            Self::Success
        } else {
            Self::Info
        }
    }
}

/// User-facing settings the UI edits (the runtime persists them).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub design: Design,
    pub theme: ThemeMode,
    pub contrast: u8,
    /// Index into the design system's scheme variants (MD3E: Content first).
    pub scheme_variant: u8,
    pub ambient: Ambient,
    pub fps_limit: u32,
    pub reduced_motion: bool,
    pub gapless: bool,
    pub crossfade_s: f32,
    pub normalization: bool,
    pub opus_bitrate: u32,
    pub check_fake_lossless: bool,
    pub library_dir: PathBuf,
    pub inbox_dir: PathBuf,
    /// Where lossless masters (`.flac` originals and `.json` sidecars) are kept.
    /// Defaults to `<library_dir>/.musiclib/masters`, or another drive/folder chosen by the user.
    pub masters_dir: PathBuf,
    pub fallback_seed: u32,
    pub prowlarr_url: String,
    pub prowlarr_configured: bool,
    pub seed_limit_gb: u32,
    pub seed_days: u32,
    pub no_seed: bool,
    pub output_devices: Vec<String>,
    pub output_device: Option<String>,
    /// MusicBrainz contact (User-Agent); the AcoustID key lives in settings.toml.
    pub metadata_contact: String,
    pub acoustid_configured: bool,
    /// Stop seeding a torrent at this upload/download ratio (0 = no limit).
    pub seed_ratio: f32,
    /// KiB/s, 0 = unlimited.
    pub download_limit_kbps: u32,
    pub upload_limit_kbps: u32,
    /// Content beside the cover in Now Playing.
    pub np_slot: NpSlot,
    /// The dotted ring around the Now Playing cover turns while playing.
    pub spin_ring: bool,
    /// How synced lyrics animate: 0 karaoke, 1 focus, 2 wave (letter by letter).
    pub lyrics_style: u8,
    /// Wave only for syllable-synced lyrics; other texts use Focus.
    pub wave_syllables_only: bool,
    /// Wave letters jump up as they are sung (off: they only light up).
    pub lyrics_wave_jump: f32,
    /// How far lines other than the current one fade (0 none .. 1 strong).
    pub lyrics_dim: f32,
    /// Text size multiplier for synced lyrics (0.8..1.4).
    pub lyrics_scale: f32,
    /// Lines further from the current one follow the scroll more lazily.
    pub lyrics_cascade: bool,
    /// The sung part glows a little.
    pub lyrics_glow: bool,
    /// Seconds after a manual scroll before the view returns to the song.
    pub lyrics_return_s: f32,
    /// The pause indicator between lines: 0 flat thin, 1 wavy thin,
    /// 2 flat thick, 3 wavy thick (the four Material 3 Expressive forms).
    pub lyrics_gap_style: u8,
    /// Crests around the pause ring (wavelength = circumference / this).
    pub lyrics_gap_waves: f32,
    /// The last seconds of a pause count down in digits instead.
    pub lyrics_countdown: bool,
    /// Embed the selected online lyrics in the track tags.
    pub save_found_lyrics: bool,
    /// The outline the Now Playing cover takes while playing.
    pub cover_shape: CoverShape,
    /// The cover's outline turns while playing (the picture stays put).
    pub cover_spin: bool,
    /// The cover swells a little on each beat.
    pub cover_pulse: bool,
    /// The dotted ring around the cover is shown.
    pub ring: bool,
    /// The ring's dots jump with the music.
    pub ring_beat: bool,
    /// What the ring is made of and where it sits.
    pub ring_style: RingStyle,
    /// The ring turns counter-clockwise.
    pub ring_ccw: bool,
    /// The cover's outline turns counter-clockwise.
    pub cover_ccw: bool,
    /// Now Playing shows a volume slider (otherwise just the mute button;
    /// the wheel still sets the volume).
    pub np_volume: bool,
    /// Instead of a fixed slider: a volume pill pops up at the bottom
    /// centre of Now Playing only while the volume changes.
    pub np_volume_pop: bool,
    /// How long the volume pill stays after the last change, in seconds.
    pub np_volume_pop_hold: f32,
    /// How Now Playing opens and closes: 0 smooth, 1 lively, 2 fast.
    pub np_motion: u8,
    /// Progress bars turn into a wave while playing (off: always flat).
    pub progress_wave: bool,
    /// The shapes [`CoverShape::Random`] picks from; empty = all of them.
    pub random_shapes: Vec<CoverShape>,
    /// Windows starts the app at sign-in (the app reads the system's own
    /// state at start; this only mirrors and edits it).
    pub autostart: bool,
    /// A start at sign-in stays in the tray, with no window.
    pub autostart_hidden: bool,
    /// A click on the Now Playing cover toggles play / pause.
    pub cover_click_pause: bool,
    /// A double click in Now Playing changes the cover outline: 0 never,
    /// 1 only in the random mode, 2 always (a fixed shape steps on).
    pub dbl_click_shape: u8,
    /// How long a tap on the cover waits for a second one, in seconds;
    /// 0 pauses at once (a second tap then undoes it).
    pub np_tap_wait: f32,
    /// Material shapes behind Now Playing that follow the music.
    pub bg_shapes: BgShapes,
    /// How many of them (4..=32).
    pub bg_shapes_count: u32,
    /// Size multiplier (0.5..=2).
    pub bg_shapes_size: f32,
    /// Opacity multiplier (0.25..=2).
    pub bg_shapes_opacity: f32,
    /// Motion speed multiplier (0.25..=3).
    pub bg_shapes_speed: f32,
    /// How strongly they react to the music (0..=2).
    pub bg_shapes_react: f32,
    /// Colours: 0 theme containers, 1 accent only, 2 bright accents,
    /// 3 the user's hue ([`Self::bg_shapes_hue`]).
    pub bg_shapes_colors: u8,
    /// The hue for colour mode 3: an index into the UI's swatches (0..8).
    pub bg_shapes_hue: u8,
    /// How strong the ambient clouds are (0.25..=2).
    pub ambient_strength: f32,
    /// The navigation rail always stays narrow (icons only) and its expand
    /// button is gone.
    pub rail_compact: bool,
    /// Deleting moves tracks into the Trash for 30 days; off: they are
    /// deleted for good at once (after a confirmation).
    pub use_trash: bool,
    /// Release groups whose editions the user chose to keep apart.
    pub kept_editions: Vec<String>,

    // ---- Sources (stage 3, the "Источники" settings tab). No secrets
    // here: the Spotify Client ID and YouTube cookies live in settings.toml
    // ([`Command::SetStreamSecrets`]).
    /// Spotify is used (search, likes, sound when the account has it).
    pub spotify_enabled: bool,
    /// YouTube Music is used.
    pub youtube_enabled: bool,
    /// The Deezer catalogue is used (search, albums, discographies; no
    /// account, its tracks play from YouTube Music).
    pub deezer_enabled: bool,
    /// What a like of a stream track downloads: the track (from its
    /// album's torrent) or the whole album. An album like is always the album.
    pub like_download_scope: WishlistScope,
    /// Likes download at all («Ничего» turns it off: a like is only a like;
    /// «Скачать» still downloads).
    pub like_downloads: bool,
    /// Sync mode of a playlist newly linked to a service.
    pub playlist_sync_default: PlaylistSyncMode,
    /// Likes and unlikes here go to the services the track is in.
    pub push_likes: bool,
    /// The library lists only tracks with a file (the inconspicuous switch).
    pub library_only_downloaded: bool,
    /// How many wishlist items search / download at once on their own.
    pub auto_download_limit: u32,
    /// Toasts about downloads going on by themselves: a release not found, a
    /// torrent started or finished. Off: that is only in «Загрузки» and «Хочу
    /// скачать».
    pub download_toasts: bool,
    /// Size limit of the stream audio cache per service, in MB (0 = off):
    /// recently played YouTube files and Spotify's encrypted files.
    pub stream_cache_mb: u32,
    /// settings.toml has a personal Spotify Client ID (the id is not here).
    pub spotify_personal_app: bool,
    /// settings.toml has YouTube Music cookies.
    pub youtube_cookies_set: bool,
    /// «Найти песню…»: the last scope chosen, «И в YouTube Music» (true) or
    /// «Только здесь».
    pub replace_in_youtube: bool,

    // ---- Recommendations (stage 4, docs/stage4.md).
    /// «Новое в радио»: how much music new to the user radios and mixes mix
    /// in.
    pub radio_new_share: RadioNewShare,
    /// «Анализ звука стримов»: liked and playlist tracks without a file are
    /// analysed from YouTube Music in the background.
    pub stream_analysis: bool,
}

/// «Новое в радио»: the share of new music in radios and mixes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioNewShare {
    /// «Без нового».
    None,
    /// «Немного», about a fifth.
    #[default]
    Some,
    /// «Побольше», about two fifths.
    More,
}

impl RadioNewShare {
    /// The share of a queue that is new music.
    pub const fn fraction(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Some => 0.2,
            Self::More => 0.4,
        }
    }
}

/// Material shapes behind the open track.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BgShapes {
    #[default]
    Off,
    /// Lying on the floor and jumping on the beats.
    Bounce,
    /// Drifting and glowing with their band.
    Float,
    /// Drifting and turning into other shapes on the beats.
    Morph,
}

/// The single content slot beside the Now Playing cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NpSlot {
    #[default]
    None,
    Lyrics,
    Queue,
    Info,
}

impl NpSlot {
    pub fn toggle(&mut self, slot: Self) {
        *self = if *self == slot { Self::None } else { slot };
    }
}

/// The ring around the Now Playing cover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RingStyle {
    /// Dots on a soft circle.
    Circle,
    /// Dots following the cover's outline at a fixed gap.
    #[default]
    Contour,
    /// Dots around the outline that grow into bars with the spectrum
    /// (caelestia's cover visualiser).
    Bars,
}

/// The Now Playing cover's outline while playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CoverShape {
    /// No outline: the square cover as is.
    Original,
    /// A new random shape whenever a track starts.
    Random,
    #[default]
    Cookie12,
    Cookie9,
    Cookie7,
    Cookie6,
    Cookie4,
    Sunny,
    VerySunny,
    Flower,
    Clover4,
    Clover8,
    SoftBurst,
    SoftBoom,
    Puffy,
    PuffyDiamond,
    Circle,
    Pentagon,
    Gem,
    Heart,
}

impl CoverShape {
    pub const ALL: [Self; 20] = [
        Self::Original,
        Self::Random,
        Self::Cookie12,
        Self::Cookie9,
        Self::Cookie7,
        Self::Cookie6,
        Self::Cookie4,
        Self::Sunny,
        Self::VerySunny,
        Self::Flower,
        Self::Clover4,
        Self::Clover8,
        Self::SoftBurst,
        Self::SoftBoom,
        Self::Puffy,
        Self::PuffyDiamond,
        Self::Circle,
        Self::Pentagon,
        Self::Gem,
        Self::Heart,
    ];

    /// The shapes [`CoverShape::Random`] picks from.
    pub fn shapes() -> &'static [Self] {
        &Self::ALL[2..]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Original => "Оригинал (квадрат)",
            Self::Random => "Случайная",
            Self::Cookie12 => "Печенье, 12 волн",
            Self::Cookie9 => "Печенье, 9 волн",
            Self::Cookie7 => "Печенье, 7 волн",
            Self::Cookie6 => "Печенье, 6 волн",
            Self::Cookie4 => "Печенье, 4 волны",
            Self::Sunny => "Солнце",
            Self::VerySunny => "Яркое солнце",
            Self::Flower => "Цветок",
            Self::Clover4 => "Клевер, 4 листа",
            Self::Clover8 => "Клевер, 8 листьев",
            Self::SoftBurst => "Вспышка",
            Self::SoftBoom => "Взрыв",
            Self::Puffy => "Облако",
            Self::PuffyDiamond => "Пухлый ромб",
            Self::Circle => "Круг",
            Self::Pentagon => "Пятиугольник",
            Self::Gem => "Самоцвет",
            Self::Heart => "Сердце",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Design {
    #[default]
    Md3e,
    Nothing,
    Glass,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Ambient {
    Off,
    Glow,
    #[default]
    Reactive,
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

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub player: PlayerView,
    pub queue: QueueView,
    pub lyrics: LyricsView,
    pub info: TrackInfo,
    pub home: HomeView,
    pub library: LibraryView,
    pub release: Option<ReleaseDetail>,
    pub artist: Option<ArtistDetail>,
    pub playlist: Option<PlaylistDetail>,
    pub search: SearchView,
    pub downloads: DownloadsView,
    /// Albums that look like editions of one album, offered to merge.
    pub editions: Vec<EditionOffer>,
    pub tag_editor: TagEditor,
    pub toasts: Vec<Toast>,
    pub prefs: Prefs,
    /// The streaming services, Spotify first (registered or not).
    pub accounts: Vec<AccountView>,
    /// The open album / playlist / artist of a service ([`Route::Stream`]).
    pub stream_page: Option<StreamPageView>,
    /// The last [`Command::Share`].
    pub share: Option<ShareView>,
    /// What the last check of Prowlarr found ([`Command::CheckIndexer`],
    /// also after its settings are saved).
    pub indexer_check: Option<IndexerCheck>,
    /// Bytes of stream audio cached on disk, all services together.
    pub stream_cache_bytes: u64,
    /// Text the app should put on the clipboard (one-shot, see
    /// [`ClipboardRequest`]).
    pub clipboard: Option<ClipboardRequest>,
    /// The open «Найти песню…» dialog ([`Command::FindSong`]).
    pub find_song: Option<FindSongView>,
    /// How far the background sound analysis is.
    pub analysis: AnalysisProgress,
    /// Bumped on every change, so a UI can skip work when nothing changed.
    pub revision: u64,
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

impl ViewState {
    pub fn account(&self, service: Service) -> Option<&AccountView> {
        self.accounts.iter().find(|a| a.service == service)
    }
}

/// The answer of Prowlarr to a check, in words: «отвечает · 12 трекеров»,
/// «ключ не подходит».
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexerCheck {
    pub checking: bool,
    pub ok: bool,
    pub text: String,
}

/// Where the user is.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Home,
    Library(LibraryTab),
    Downloads(DownloadsTab),
    Settings,
    Release(ReleaseId),
    Artist(ArtistId),
    Playlist(PlaylistId),
    /// An album, playlist or artist of a service (load it with
    /// [`Command::LoadStream`]; shown from [`ViewState::stream_page`]).
    Stream(StreamPage),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LibraryTab {
    #[default]
    All,
    Playlists,
    Albums,
    Artists,
    Tracks,
    /// Deleted tracks.
    Trash,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DownloadsTab {
    #[default]
    Active,
    Confirm,
    Seeding,
    History,
    /// Liked stream tracks waiting for a file ([`DownloadsView::wishlist`]).
    Wishlist,
}

/// What the UI asks the runtime to do. Loads are requests too: the runtime
/// answers by filling the matching part of [`ViewState`].
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    // Playback
    TogglePlay,
    Play,
    Pause,
    Next,
    /// Restart the track if past 3 s, else go back.
    Previous,
    Seek(Duration),
    SeekBy(i64),
    /// Show this track's lyrics from another source; `None` = automatic.
    PickLyricsSource(TrackId, Option<LyricsSource>),
    SetVolume(f32),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    PlayRelease { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    PlayPlaylist { id: PlaylistId, start: Option<TrackId>, shuffle: bool },
    PlayArtist { id: ArtistId, shuffle: bool },
    PlayTracks { tracks: Vec<TrackId>, start: usize, context: String },
    PlayNext(Vec<TrackId>),
    Enqueue(Vec<TrackId>),
    /// All tracks of an album / an artist to the end of the queue.
    EnqueueRelease(ReleaseId),
    EnqueueArtist(ArtistId),
    PlayQueueEntry(u64),
    RemoveFromQueue(u64),
    /// Puts queue `entry` right after (or before) the `anchor` entry.
    MoveInQueue { entry: u64, anchor: u64, after: bool },
    ClearQueue,
    SaveQueueAsPlaylist(String),

    // Library
    Like(LikeTarget, bool),
    CreatePlaylist(String),
    RenamePlaylist(PlaylistId, String),
    DeletePlaylist(PlaylistId),
    AddToPlaylist(PlaylistId, Vec<TrackId>),
    RemoveFromPlaylist(PlaylistId, TrackId),
    MoveInPlaylist { playlist: PlaylistId, track: TrackId, to: usize },
    /// Delete a track: into the Trash (30 days), or for good when
    /// [`Prefs::use_trash`] is off.
    TrashTrack(TrackId),
    /// Take a deleted track back out of the Trash.
    UndoTrash(TrackId),
    /// Delete every track of an album (like [`Command::TrashTrack`]).
    TrashRelease(ReleaseId),
    /// Take deleted tracks back out of the Trash.
    RestoreTracks(Vec<TrackId>),
    /// Delete tracks for good: files, master and rows.
    DeleteForever(Vec<TrackId>),
    /// Delete every track of an album for good.
    DeleteReleaseForever(ReleaseId),
    /// Delete everything in the Trash for good.
    EmptyTrash,
    /// Fill [`LibraryView::trash`].
    LoadTrash,
    /// Collage / smart cover (a custom one is set by [`Command::SetPlaylistCover`]).
    SetPlaylistCoverMode(PlaylistId, PlaylistCoverMode),

    // Loads
    LoadLibrary,
    LoadTracks { offset: usize, limit: usize, sort: TrackSort },
    /// The library filter on «Треки»: library tracks matching the words
    /// into [`LibraryView::track_hits`]; an empty query clears it.
    FilterTracks(String),
    SortReleases(ReleaseSort),
    LoadRelease(ReleaseId),
    LoadArtist(ArtistId),
    LoadPlaylist(PlaylistId),
    LoadHome,
    Search(String),

    // Tags
    /// Read the file's tags into [`ViewState::tag_editor`].
    OpenTagEditor(TrackId),
    /// Ask MusicBrainz for matches of the current editor values.
    LookupTags { track: TrackId, query: TagEdit },
    /// Write tags to the file and the library.
    SaveTags { track: TrackId, tags: TagEdit },
    CloseTagEditor,
    SetPlaylistCover(PlaylistId, PathBuf),

    // Torrents
    /// Search the indexer (Prowlarr) for releases.
    SearchTorrents(String),
    /// Download a search result; `files` = indices inside the torrent
    /// (None = everything).
    DownloadTorrent { result: usize, files: Option<Vec<usize>> },
    /// Fill [`DownloadsView::files`] for a search result.
    ListTorrentFiles(usize),
    CloseTorrentFiles,
    /// Remove finished torrents from seeding now (their files go too).
    CleanSeeding,

    // Import & downloads
    ImportFiles(Vec<PathBuf>),
    ImportFolder(PathBuf),
    AddTorrentFile(PathBuf),
    AddMagnet(String),
    PauseJob(u64, bool),
    CancelJob(u64),
    ConfirmChoice { id: u64, option: Option<usize> },
    /// Folds the editions of [`EditionOffer::id`] into one album.
    MergeEditions(u64),
    /// Keeps them apart and never offers them again.
    KeepEditionsApart(u64),

    // Settings
    SetPrefs(Box<Prefs>),
    /// Move lossless FLAC/WAV masters to another drive or folder (copy -> hash verify -> delete old).
    MoveMastersDir(PathBuf),
    /// Prowlarr address and API key (stored in settings.toml, never shown again).
    SetIndexer { url: String, api_key: String },
    /// Asks Prowlarr whether it answers ([`ViewState::indexer_check`]) at the
    /// address and key typed in the settings (not saved yet); an empty address
    /// checks the saved one, an empty key uses the saved key.
    CheckIndexer { url: String, api_key: String },
    /// MusicBrainz contact and AcoustID key (stored in settings.toml).
    SetMetadata { contact: String, acoustid_key: Option<String> },
    DismissToast(u64),

    // Streaming: accounts (stage 3). Every network call runs in the
    // background; the result shows up in `ViewState::accounts` / a toast.
    /// Logs in ([`AccountView::busy`] meanwhile; the prompt to act on is in
    /// [`AccountView::account`]). A Spotify personal app's Client ID is
    /// saved to settings.toml on success.
    StreamLogin { service: Service, input: LoginInput },
    StreamLogout(Service),
    /// Secrets → settings.toml (never into prefs): `None` keeps the value,
    /// `Some("")` clears it.
    SetStreamSecrets {
        spotify_client_id: Option<String>,
        youtube_cookies: Option<String>,
    },

    // Streaming: search and pages
    /// The chips of the unified search; runs the current query again.
    SetSearchSources(SearchSources),
    /// Fill [`ViewState::stream_page`].
    LoadStream(StreamPage),
    /// Play stream tracks (rows of a search, page or section). They become
    /// "seen" tracks (a library row outside the library) as they play.
    PlayStreamTracks {
        tracks: Vec<RemoteId>,
        start: usize,
        context: String,
    },
    /// Play the merged tracks of the «Всё» search: library tracks and
    /// stream tracks in one queue ([`SearchAnswer::tracks`]).
    PlaySearch {
        tracks: Vec<SearchPick>,
        start: usize,
        context: String,
    },
    /// Like [`Command::PlayRelease`] for an album of a service.
    PlayStreamAlbum {
        id: RemoteId,
        start: Option<RemoteId>,
        shuffle: bool,
    },
    /// Like [`Command::PlayPlaylist`] for a playlist of a service.
    PlayStreamPlaylist {
        id: RemoteId,
        start: Option<RemoteId>,
        shuffle: bool,
    },
    /// Stream tracks after the current one / at the end of the queue.
    PlayStreamNext(Vec<RemoteId>),
    EnqueueStream(Vec<RemoteId>),
    /// Like a stream track: it joins the library (marked "стрим"), the like
    /// goes to the services ([`Prefs::push_likes`]) and, without a file, a
    /// wishlist item downloads it ([`Prefs::like_download_scope`]).
    LikeStream { track: RemoteId, on: bool },
    /// Like every track of an album of a service (the wishlist downloads
    /// the whole album).
    LikeStreamAlbum { album: RemoteId, on: bool },
    /// Download stream tracks without liking them: each goes to «Хочу
    /// скачать» and joins the library once its file is here (not as a like).
    DownloadStream { tracks: Vec<RemoteId> },
    /// The same for a whole album of a service (one album item).
    DownloadStreamAlbum(RemoteId),
    /// Library tracks without a file → «Хочу скачать» (no like).
    DownloadTracks(Vec<TrackId>),
    /// The missing files of a library album → one album item of «Хочу
    /// скачать».
    DownloadRelease(ReleaseId),
    /// Stream tracks into a library playlist (they join the library).
    AddStreamToPlaylist(PlaylistId, Vec<RemoteId>),
    /// A song.link / album.link for a track or album → [`ViewState::share`]
    /// and the clipboard, plus a toast.
    Share(ShareTarget),

    // Streaming: wishlist
    /// Fill [`DownloadsView::wishlist`].
    LoadWishlist,
    /// Start downloading these waiting items (queued under
    /// [`Prefs::auto_download_limit`]).
    WishlistDownload(WishlistSelect),
    /// Never offer these again (also not by a pull or a re-import).
    WishlistDismiss(Vec<WishlistId>),
    /// Take dismissed items back: they wait again (imported ones) or are
    /// searched for at once.
    WishlistRestore(Vec<WishlistId>),
    /// Search a torrent for this item now (instead of next week).
    WishlistRetry(WishlistId),
    /// Save the audio of the item's track from YouTube Music as it is (low
    /// quality, a torrent upgrades it later).
    WishlistSaveFromYoutube(WishlistId),
    /// The user's own .torrent / magnet / file / folder for this item: it is
    /// downloaded / imported as the item's track.
    WishlistAttach { item: WishlistId, what: WishlistAttach },

    // Streaming: cold start, sync, discography. Not done yet: answered with
    // a "Скоро" toast (the UI can be built against them).
    /// Likes, own playlists and recent plays from a service's API; progress
    /// as a job in "Загрузки"; likes and playlists → library (stream) +
    /// wishlist "ждёт", history → events only.
    ImportFromService(Service),
    /// Export files (Spotify Extended Streaming History / Account data,
    /// Google Takeout YouTube Music): a folder or a .zip.
    ImportExportFiles(PathBuf),
    /// Empties the stream audio cache of every service.
    ClearStreamCache,
    /// The app window was shown, hidden (tray, minimized) or (un)focused.
    /// While it is hidden and nothing plays, the runtime's periodic network
    /// work (like pulls, playlist checks, completing imports) waits; on
    /// focus a like pull older than 10 minutes runs.
    WindowState { visible: bool, focused: bool },
    /// How a library playlist linked to a service follows it.
    SetPlaylistSync { playlist: PlaylistId, mode: PlaylistSyncMode },
    /// A playlist of a service into the library: a library playlist that
    /// follows it ([`Prefs::playlist_sync_default`]; someone else's
    /// playlist is only followed).
    FollowStreamPlaylist(RemoteId),
    /// Fill [`ArtistDetail::discography`] ("Нет в библиотеке").
    LoadDiscography(ArtistId),

    // Streaming: a song by hand for a clip or a wrong match
    /// Opens [`ViewState::find_song`] for this stream track and searches
    /// songs for `query` (`None`: its cleaned title and artist).
    FindSong { track: TrackId, query: Option<String> },
    CloseFindSong,
    /// The track becomes the song `with` (a result of the dialog); with
    /// `youtube` the like moves there too: the liked clip is unliked on
    /// YouTube Music and the song liked. The choice is kept as
    /// [`Prefs::replace_in_youtube`].
    ReplaceTrack { track: TrackId, with: RemoteId, youtube: bool },
    /// Takes a replacement back (the toast's «Отменить»), on YouTube Music
    /// too when it went there.
    UndoReplace(u64),

    // Recommendations (stage 4, docs/stage4.md §6)
    /// Plays a mix card: «Твой микс» as it is today, a mood, a style mix,
    /// an artist's radio or the discoveries; everything but «Твой микс» is
    /// endless (the queue becomes a radio, [`QueueView::radio`]).
    PlayMix { id: MixId },
    /// «Радио по треку» / «Радио» of an artist: the seed first (a track),
    /// then a queue that extends itself and learns from skips.
    StartRadio { seed: RadioSeed },
    /// «Не рекомендовать трек / исполнителя» (`dislike`), or its undo (the
    /// toast's «Отменить» sends it with `dislike: false`). Kept out of
    /// every mix, radio and discovery from now on.
    RecFeedback { target: RecTarget, dislike: bool },
    /// «Обновить» in ⋮ of a mix: «Твой микс» is picked again (and kept for
    /// the rest of the day), «Новое для тебя» is looked for again.
    RefreshMix { id: MixId },
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            design: Design::Md3e,
            theme: ThemeMode::Dark,
            contrast: 0,
            scheme_variant: 0,
            ambient: Ambient::Reactive,
            fps_limit: 60,
            reduced_motion: false,
            gapless: true,
            crossfade_s: 0.0,
            normalization: true,
            opus_bitrate: 160,
            check_fake_lossless: true,
            library_dir: PathBuf::new(),
            inbox_dir: PathBuf::new(),
            masters_dir: PathBuf::new(),
            fallback_seed: 0xFF6750A4,
            prowlarr_url: String::new(),
            prowlarr_configured: false,
            seed_limit_gb: 50,
            seed_days: 30,
            no_seed: false,
            output_devices: Vec::new(),
            output_device: None,
            metadata_contact: String::new(),
            acoustid_configured: false,
            seed_ratio: 2.0,
            download_limit_kbps: 0,
            upload_limit_kbps: 0,
            np_slot: NpSlot::None,
            spin_ring: true,
            lyrics_style: 0,
            wave_syllables_only: true,
            lyrics_wave_jump: 1.0,
            lyrics_dim: 0.5,
            lyrics_scale: 1.0,
            lyrics_cascade: true,
            lyrics_glow: false,
            lyrics_return_s: 4.0,
            lyrics_gap_style: 1,
            lyrics_gap_waves: 6.0,
            lyrics_countdown: true,
            save_found_lyrics: false,
            cover_shape: CoverShape::default(),
            cover_spin: true,
            cover_pulse: true,
            ring: true,
            ring_beat: true,
            ring_style: RingStyle::default(),
            ring_ccw: false,
            cover_ccw: true,
            np_volume: false,
            np_volume_pop: false,
            np_volume_pop_hold: 0.8,
            np_motion: 1,
            progress_wave: true,
            random_shapes: Vec::new(),
            autostart: false,
            autostart_hidden: true,
            cover_click_pause: true,
            dbl_click_shape: 1,
            np_tap_wait: 0.22,
            bg_shapes: BgShapes::Off,
            bg_shapes_count: 14,
            bg_shapes_size: 1.0,
            bg_shapes_opacity: 1.0,
            bg_shapes_speed: 1.0,
            bg_shapes_react: 1.0,
            bg_shapes_colors: 0,
            bg_shapes_hue: 5,
            ambient_strength: 1.0,
            rail_compact: false,
            use_trash: true,
            kept_editions: Vec::new(),
            spotify_enabled: true,
            youtube_enabled: true,
            deezer_enabled: true,
            like_download_scope: WishlistScope::Track,
            like_downloads: true,
            playlist_sync_default: PlaylistSyncMode::Follow,
            push_likes: true,
            library_only_downloaded: false,
            auto_download_limit: 2,
            download_toasts: false,
            stream_cache_mb: 500,
            spotify_personal_app: false,
            youtube_cookies_set: false,
            replace_in_youtube: false,
            radio_new_share: RadioNewShare::Some,
            stream_analysis: true,
        }
    }
}

impl Prefs {
    /// The service is switched on.
    pub const fn service_enabled(&self, s: Service) -> bool {
        match s {
            Service::Spotify => self.spotify_enabled,
            Service::Youtube => self.youtube_enabled,
            Service::Deezer => self.deezer_enabled,
        }
    }
}

#[cfg(test)]
mod toast_tests {
    use super::ToastKind;

    #[test]
    fn toast_kinds_are_guessed_from_the_text() {
        for (text, kind) in [
            ("Не получилось воспроизвести трек", ToastKind::Error),
            ("Торренты недоступны", ToastKind::Error),
            ("Это не magnet-ссылка", ToastKind::Error),
            ("Восстановить удалось не всё", ToastKind::Error),
            ("Добавляю раздачу…", ToastKind::Working),
            ("Импорт начался · смотри «Загрузки»", ToastKind::Working),
            ("Сохранено", ToastKind::Success),
            ("Теги сохранены", ToastKind::Success),
            ("Плейлист «Ночь» сохранён", ToastKind::Success),
            ("Добавлено в очередь", ToastKind::Success),
            ("Альбом в Любимых · 3 трека", ToastKind::Success),
            ("Альбом в корзине (30 дней)", ToastKind::Success),
            ("Удалено навсегда: 2", ToastKind::Success),
            ("Импорт: 12 треков", ToastKind::Info),
            ("Не нашёл раздачу: Кино — Война", ToastKind::Error),
            ("Ссылка скопирована", ToastKind::Success),
            ("Spotify подключён · звук пока с YouTube Music", ToastKind::Success),
            ("Spotify не подключён", ToastKind::Error),
            ("В «Хочу скачать»: «Война»", ToastKind::Success),
            ("Нет связи — проверь интернет", ToastKind::Error),
            ("Заменено на «AIYOU»", ToastKind::Success),
            ("Скоро", ToastKind::Info),
        ] {
            assert_eq!(ToastKind::guess(text), kind, "{text}");
        }
    }
}


#[cfg(test)]
mod context_tests {
    use super::PlayerView;

    fn line(c: Option<&str>) -> String {
        PlayerView { context: c.map(str::to_owned), ..PlayerView::default() }.context_line()
    }

    #[test]
    fn contexts_read_without_quotes_in_quotes() {
        assert_eq!(line(None), "Сейчас играет");
        assert_eq!(line(Some("Библиотека")), "Играет из «Библиотека»");
        assert_eq!(line(Some("библиотеки")), "Играет из библиотеки");
        assert_eq!(line(Some("альбома «Группа крови»")), "Играет из альбома «Группа крови»");
        assert_eq!(line(Some("плейлиста «Вечер»")), "Играет из плейлиста «Вечер»");
        assert_eq!(line(Some("поиска Spotify: лёд")), "Играет из поиска Spotify: лёд");
        assert_eq!(line(Some("поиска: лёд")), "Играет из поиска: лёд");
        // Saved by older versions.
        assert_eq!(line(Some("Spotify · «лёд»")), "Играет из поиска Spotify: лёд");
        assert_eq!(line(Some("Поиск «лёд»")), "Играет из поиска: лёд");
        for c in ["Spotify · «лёд»", "Поиск «лёд»", "альбома «X»", "Кино"] {
            assert!(line(Some(c)).matches('«').count() <= 1, "{c}");
        }
    }
}
