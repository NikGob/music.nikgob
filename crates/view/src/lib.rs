//! The view model every design system draws from.
//!
//! The runtime (library, player, import, downloads) fills a [`ViewState`];
//! a design system reads it each frame and answers with [`Command`]s. Neither
//! side knows how the other works: MD3E, Nothing and Liquid Glass are
//! different layouts over the same state.

use std::path::PathBuf;
use std::time::{Duration, Instant};

pub use nikgob_core::model::{
    ArtistId, LikeTarget, PlaylistId, ReleaseId, ReleaseType, SourceKind, TrackId,
};

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
    /// The egui image URI for this file.
    pub fn uri(&self) -> String {
        file_uri(&self.path)
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
    pub artist_id: Option<ArtistId>,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseCard {
    pub id: ReleaseId,
    pub title: String,
    pub artist: String,
    pub artist_id: Option<ArtistId>,
    pub year: Option<i32>,
    pub release_type: ReleaseType,
    pub cover: Option<Cover>,
    pub local_tracks: u32,
    pub total_tracks: Option<u32>,
    pub liked: bool,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArtistDetail {
    pub card: ArtistCard,
    pub top_tracks: Vec<TrackRow>,
    pub albums: Vec<ReleaseCard>,
    pub singles_eps: Vec<ReleaseCard>,
    pub liked: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistCard {
    pub id: PlaylistId,
    pub name: String,
    pub cover: Option<Cover>,
    pub track_count: u32,
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistDetail {
    pub card: PlaylistCard,
    pub tracks: Vec<TrackRow>,
    pub total_duration: Duration,
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
        }
    }
}

impl PlayerView {
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
    pub current: Option<TrackRow>,
    /// Upcoming tracks in play order; `entry` ids are stable for drag & drop.
    pub next: Vec<QueueEntry>,
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
    pub lines: Vec<(Duration, String)>,
    /// Unsynced fallback text.
    pub plain: Option<String>,
    pub loading: bool,
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeView {
    pub greeting_hour: u8,
    pub mix: Vec<TrackRow>,
    pub continue_listening: Vec<ReleaseCard>,
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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SearchView {
    pub query: String,
    pub tracks: Vec<TrackRow>,
    pub releases: Vec<ReleaseCard>,
    pub artists: Vec<ArtistCard>,
    pub playlists: Vec<PlaylistCard>,
    pub searching: bool,
    /// Torrent search (Prowlarr) runs only on request: it is slow.
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
}

/// A transient message ("Добавлено в Любимые · Отменить").
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub id: u64,
    pub text: String,
    pub action: Option<(String, Command)>,
    pub at: Instant,
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
    pub tag_editor: TagEditor,
    pub toasts: Vec<Toast>,
    pub prefs: Prefs,
    /// Bumped on every change, so a UI can skip work when nothing changed.
    pub revision: u64,
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
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LibraryTab {
    #[default]
    All,
    Playlists,
    Albums,
    Artists,
    Tracks,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DownloadsTab {
    #[default]
    Active,
    Confirm,
    Seeding,
    History,
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
    MoveInQueue { entry: u64, to: usize },
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
    /// Move the track's files to Trash (30 days).
    TrashTrack(TrackId),
    /// Take the last trashed track back.
    UndoTrash(TrackId),

    // Loads
    LoadLibrary,
    LoadTracks { offset: usize, limit: usize, sort: TrackSort },
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

    // Settings
    SetPrefs(Box<Prefs>),
    /// Prowlarr address and API key (stored in settings.toml, never shown again).
    SetIndexer { url: String, api_key: String },
    /// MusicBrainz contact and AcoustID key (stored in settings.toml).
    SetMetadata { contact: String, acoustid_key: Option<String> },
    DismissToast(u64),
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
        }
    }
}
