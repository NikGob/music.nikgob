//! The search, the torrents, the tag editor, the downloads and the sharing,
//! the toasts and the routes, and the whole [`ViewState`].

use std::time::Instant;

use super::*;

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
    /// Upload and download right now, bytes per second.
    pub up_bps: u64,
    pub down_bps: u64,
    /// Peers connected right now.
    pub peers: u32,
    /// What it takes on disk (the selected files).
    pub size_bytes: u64,
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
    /// It waits its turn at the sharer (a Soulseek peer's queue): no bar,
    /// the subtitle says the place.
    pub queued: bool,
    /// It can be cancelled (a torrent, an import: what it added goes back).
    pub controllable: bool,
    /// It can be paused too (a torrent).
    pub pausable: bool,
    pub error: Option<String>,
    /// When it ended (ms since the epoch); "История" is newest first by it.
    /// 0 while it runs.
    pub at: i64,
}

/// An item waiting for the user to pick a match ("Подтверди").
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub id: u64,
    pub title: String,
    pub origin: String,
    /// What the options answer ("Это какой трек?", "Что из этого скачать?").
    pub question: String,
    pub options: Vec<ConfirmOption>,
    /// What `ConfirmChoice { option: None }` does ("Пропустить",
    /// "Оставить теги файла").
    pub skip_label: String,
    /// Button for the chosen option ("Скачать", "Импортировать").
    pub accept_label: String,
    /// A card of songs each ticked on its own (the fake FLACs offered as
    /// MP3s): `options` are its songs, none is ticked at first, and both
    /// buttons act on the ticked ones only ([`Command::ConfirmPicked`]).
    /// `None`: one option is chosen ([`Command::ConfirmChoice`]).
    pub pick: Option<ConfirmPick>,
    /// A yes or no question about the options («Одна и та же песня?» with
    /// the two songs): they are shown without a choice, the accept button
    /// answers yes (`option: Some(0)`) and the other no (`None`).
    pub yes_no: bool,
}

impl Confirm {
    /// It has something to answer: options to choose from, songs to tick,
    /// or the songs a yes or no is about. A card without is never shown
    /// (its accept button would take nothing).
    pub fn answerable(&self) -> bool {
        !self.options.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmOption {
    pub label: String,
    pub detail: String,
    pub recommended: bool,
}

/// The songs of a [`Confirm::pick`] card and the words of its buttons.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmPick {
    /// The source of each option, in order: what
    /// [`Command::ConfirmPicked`] carries for the ticked ones.
    pub sources: Vec<nikgob_core::model::SourceId>,
    /// The accept button around the count of ticked songs: («Превратить»,
    /// «в MP3») reads «Превратить 3 в MP3».
    pub accept: (String, String),
    /// The other button, the same way: («Оставить», «FLAC») reads
    /// «Оставить 3 FLAC».
    pub skip: (String, String),
}

impl ConfirmPick {
    /// A button's words around `n` ticked songs; with none ticked, without
    /// a count («Превратить в MP3»).
    pub fn words(around: &(String, String), n: usize) -> String {
        let (before, after) = around;
        match (n, after.is_empty()) {
            (0, true) => before.clone(),
            (0, false) => format!("{before} {after}"),
            (n, true) => format!("{before} {n}"),
            (n, false) => format!("{before} {n} {after}"),
        }
    }
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
    /// The download sources, in the user's order (Settings → «Источники»).
    pub sources: Vec<SourceView>,
    /// `[soulseek]` of settings.toml as «Источники» shows it.
    pub soulseek: SoulseekView,
    /// What «Раздача» says about both kinds of sharing.
    pub sharing: SharingView,
    /// How much the music takes and what is left of its disks (the
    /// page's header).
    pub storage: StorageView,
}

/// «Загрузки → Раздача»: how much the network is used and how much was
/// given away, torrents and Soulseek apart.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharingView {
    /// Right now, bytes per second.
    pub torrent_up_bps: u64,
    pub torrent_down_bps: u64,
    pub slsk_up_bps: u64,
    pub slsk_down_bps: u64,
    /// Given away today (the local day) and over all time.
    pub torrent_today: u64,
    pub slsk_today: u64,
    pub torrent_total: u64,
    pub slsk_total: u64,
    /// What the torrents being seeded take on disk.
    pub seeding_disk_bytes: u64,
    /// Peers connected to all torrents now.
    pub torrent_peers: u32,
    pub slsk: SlskSharingView,
}

impl SharingView {
    pub fn up_bps(&self) -> u64 {
        self.torrent_up_bps + self.slsk_up_bps
    }

    pub fn down_bps(&self) -> u64 {
        self.torrent_down_bps + self.slsk_down_bps
    }

    pub fn today(&self) -> u64 {
        self.torrent_today + self.slsk_today
    }

    pub fn total(&self) -> u64 {
        self.torrent_total + self.slsk_total
    }
}

/// The Soulseek half of «Раздача».
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlskSharingView {
    /// An account (or a service) is saved.
    pub configured: bool,
    /// The client (or the link to the service) runs.
    pub running: bool,
    /// On a server or a small device (`nikgob-slsk`).
    pub remote: bool,
    /// Sharing is on in settings.toml.
    pub share: bool,
    /// What is shared, as the client counts it (0 before the first scan).
    pub files: u32,
    pub folders: u32,
    /// KiB/s for all uploads together, 0 = no limit.
    pub upload_limit_kbps: u32,
    /// Uploads at once.
    pub upload_slots: u32,
    /// Running, queued, then the last finished.
    pub uploads: Vec<UploadRow>,
}

/// One file someone takes from us over Soulseek.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadRow {
    pub id: u64,
    pub user: String,
    /// The file name, without folders.
    pub file: String,
    pub size: u64,
    pub sent: u64,
    pub bytes_per_sec: u64,
    pub state: UploadRowState,
    /// Ms since the epoch when it ended (0 while not ended).
    pub ended_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadRowState {
    Queued,
    Sending,
    Done,
    Failed,
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
    /// It stays twice as long (8 s): it says how to undo a step that
    /// matters («Карточка скрыта…» with «Вернуть»).
    pub long: bool,
    /// How many times the same message came again while it was up: each
    /// starts its time over.
    pub again: u32,
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

#[derive(Clone, Debug, Default)]
pub struct ViewState {
    pub player: PlayerView,
    pub queue: QueueView,
    pub lyrics: LyricsView,
    pub align: AlignView,
    pub info: TrackInfo,
    /// The info of a track that need not be the one playing, asked for by
    /// [`Command::TrackInfo`] (its own window).
    pub track_info: Option<TrackInfo>,
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
    /// The open world of Главная ([`Route::World`]).
    pub world: Option<WorldView>,
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
    /// The neural sound model: its files and how far it is.
    pub sound_model: SoundModelView,
    /// The names of the tracks with a speed of their own
    /// ([`TrackSpeeds::chosen`]), for «Не нравится и скорость».
    pub speed_names: std::sync::Arc<std::collections::HashMap<TrackId, DislikedTrack>>,
    /// Disliked tracks and artists not to recommend («Не нравится»,
    /// «Не рекомендовать исполнителя»).
    pub dislikes: DislikesView,
    /// The tracks played faster or slower («Скорость»).
    pub speeds: TrackSpeeds,
    /// The tracks of one song on two albums (one recording): another album
    /// it is on and that album's title, for the track's menu («Есть и на
    /// альбоме «…»»).
    pub also_on: std::sync::Arc<std::collections::HashMap<TrackId, (ReleaseId, String)>>,
    /// Bumped when cover files on disk may have been rewritten (an import,
    /// a retag), so a UI looks at the covers it holds again.
    pub cover_files: u64,
    /// Web covers to ask anew, whatever is kept of them (in memory, on
    /// disk, decoded): «Обновить обложку» of a service's album. Counted, so
    /// a UI does it once per change.
    pub refetch_covers: (u64, Vec<String>),
    /// Real album art found for the video thumbnails of stream tracks drawn
    /// on screen ([`Command::RealCovers`]): the thumbnail's url → the art's.
    pub real_covers: std::sync::Arc<std::collections::HashMap<String, String>>,
    /// Bumped on every change, so a UI can skip work when nothing changed.
    pub revision: u64,
    /// The local REST API («Межсеть → API и MCP»).
    pub api: ApiView,
    /// No sound can come out: the audio engine did not start, or the
    /// device heard failed and nothing has played since. The API answers
    /// its play commands with `503 no_audio`.
    pub no_audio: bool,
    /// The owner's devices («Межсеть → Устройства»).
    pub mesh: MeshView,
    /// None: this window shows this device's zone; Some: another device's,
    /// whose player and queue `player` and `queue` mirror (a remote).
    pub target: Option<RemoteTarget>,
    /// This device's own zone while the target is remote.
    pub parked: Option<ParkedView>,
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
    /// «Текст песни»: every text of the playing song and its timing.
    Lyrics,
    Release(ReleaseId),
    Artist(ArtistId),
    Playlist(PlaylistId),
    /// An album, playlist or artist of a service (load it with
    /// [`Command::LoadStream`]; shown from [`ViewState::stream_page`]).
    Stream(StreamPage),
    /// A world of Главная's card by its key ([`Command::LoadWorld`]).
    World(String),
    /// A mix's page: one of «Миксы для тебя» ([`HomeView::lanes`]) or,
    /// with `world`, of that world's page ([`WorldView::mixes`]).
    Mix { key: MixKey, world: Option<String> },
    /// «Новые релизы» of Главная, all of them ([`HomeView::new_releases`]).
    NewReleases,
    /// «Не нравится и скорость» (from Settings → Музыка): the disliked
    /// tracks, the artists not recommended and the tracks with a speed of
    /// their own, each taken back there.
    Marks,
    /// «Карта сети» (from Settings → Межсеть): the owner's devices as
    /// cards, what flows between them, each one's passport.
    Devices,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LibraryTab {
    #[default]
    All,
    Playlists,
    Albums,
    Artists,
    Tracks,
    /// Tracks by the day their file came (experimental).
    Downloaded,
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
