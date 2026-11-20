//! What the UI asks the runtime to do.

use std::path::PathBuf;
use std::time::Duration;

use super::*;

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
    /// «Таймер сна»: set, change or turn off.
    SleepTimer(SleepChoice),
    /// «Скорость» of a track: the version it plays as from now on
    /// (remembered with the track; the playing one changes at once).
    SetTrackSpeed { track: TrackId, variant: SpeedVariant },
    /// «Сбросить» of a track's speed on «Не нравится и скорость»: as
    /// recorded again, with «Отменить» in the snackbar.
    ResetTrackSpeed(TrackId),

    /// «Включить с YouTube» (the developer profile, [`dev::profile`]): the
    /// playing track goes on from where it is on its YouTube Music copy,
    /// looked up now when it has none (and kept as its source).
    PlayFromYoutube,
    /// Show this track's lyrics from another source; `None` = automatic.
    PickLyricsSource(TrackId, Option<LyricsSource>),
    /// Look this track's lyrics up online again, whatever was found or not
    /// found before (a text found earlier stays until a new one comes).
    SearchLyricsAgain(TrackId),
    /// Make line or word timing for a source's text from the recording
    /// ([`AlignView::craft`]); `deep`: with deep mode's parts for this one
    /// timing, whatever `Prefs::lyrics_deep` says (which decides only the
    /// timing the player makes by itself).
    CraftLyrics { track: TrackId, source: LyricsSource, target: LyricsSync, deep: bool },
    /// Forget a failed [`AlignView::craft`] (its message was seen).
    ClearCraft,
    /// Stop the timing being made ([`AlignView::craft`]); the text stays
    /// as it was.
    CancelCraft,
    /// See this track's lyrics by words, by lines or without timing
    /// ([`LyricsView::shows`]); remembered per track.
    ShowLyricsAs(TrackId, LyricsSync),
    /// Keep the shown text (with the timing the player made) on the
    /// computer ([`LyricsView::savable`]).
    SaveLyrics { track: TrackId, source: LyricsSource },
    /// Keep the text the user edited by hand as their own: `lines` with
    /// their timing (`timed`), or untimed, their texts one a line. A `.ttml`
    /// next to the file (for a song without one in its artist's folder),
    /// marked as theirs ([`LyricsSource::Own`]): shown first from then on,
    /// never refetched.
    SaveOwnLyrics { track: TrackId, lines: Vec<LyricLine>, timed: bool },
    /// «Вернуть найденный»: the user's own text goes to the Trash (with
    /// undo) and the found texts are shown again.
    DropOwnLyrics(TrackId),
    /// The undo of [`Command::DropOwnLyrics`].
    RestoreOwnLyrics(TrackId),
    /// Download (true) or delete a lyrics model by its [`AlignModel::key`].
    AlignModel { key: String, download: bool },
    SetVolume(f32),
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    /// An album as its page is set to open: of an album only some songs of
    /// which are the user's, those songs («Мои треки», a song picked
    /// outside them plays the whole album from it) or all of it.
    PlayRelease { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    /// «Слушать весь альбом»: every song of an album, once; nothing is
    /// added to the library or downloaded.
    PlayReleaseWhole { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    /// «Мои треки» of an album only some songs of which are the user's:
    /// those songs, whatever the album opens on (a song picked outside them
    /// plays the whole album from it).
    PlayReleaseMine { id: ReleaseId, start: Option<TrackId>, shuffle: bool },
    PlayPlaylist { id: PlaylistId, start: Option<TrackId>, shuffle: bool },
    PlayArtist { id: ArtistId, shuffle: bool },
    /// These tracks from the one at `start` (the user picked it).
    PlayTracks { tracks: Vec<TrackId>, start: usize, context: QueueOrigin },
    /// A whole list with no track picked («Слушать всё»): from the first,
    /// or from a random one under shuffle.
    PlayAll { tracks: Vec<TrackId>, context: QueueOrigin },
    PlayNext(Vec<TrackId>),
    Enqueue(Vec<TrackId>),
    /// All tracks of an album / an artist to the end of the queue.
    /// The whole album right after the current track, in its order (under
    /// «Только скачанное» as playing it); the queue goes on after it.
    PlayReleaseNext(ReleaseId),
    EnqueueRelease(ReleaseId),
    /// A whole playlist right after the current track / to the end of the
    /// queue (what «Слушать» would play of it).
    PlayPlaylistNext(PlaylistId),
    EnqueuePlaylist(PlaylistId),
    /// An artist's tracks right after the current track / to the end of
    /// the queue (what the queue gets of them).
    PlayArtistNext(ArtistId),
    EnqueueArtist(ArtistId),
    PlayQueueEntry(u64),
    RemoveFromQueue(u64),
    /// Puts queue `entry` right after (or before) the `anchor` entry.
    MoveInQueue { entry: u64, anchor: u64, after: bool },
    ClearQueue,
    /// The undo of [`Command::ClearQueue`] (its toast's «Отменить»): what
    /// it took out comes back around the track playing, if that still is
    /// the one that played then.
    UndoClearQueue,
    SaveQueueAsPlaylist(String),
    /// A new playlist named so with these tracks («Сохранить как плейлист»
    /// of a mix).
    SaveTracksAsPlaylist { name: String, tracks: Vec<TrackId> },

    // Library
    Like(LikeTarget, bool),
    CreatePlaylist(String),
    RenamePlaylist(PlaylistId, String),
    DeletePlaylist(PlaylistId),
    AddToPlaylist(PlaylistId, Vec<TrackId>),
    /// All tracks of a library album, in its disc / track order, into a
    /// playlist (as [`Command::AddToPlaylist`]; «Любимые» likes each).
    AddAlbumToPlaylist(PlaylistId, ReleaseId),
    RemoveFromPlaylist(PlaylistId, TrackId),
    /// The undo of [`Command::RemoveFromPlaylist`]: the track back at its
    /// place `at` in the playlist.
    RestoreToPlaylist { playlist: PlaylistId, track: TrackId, at: usize },
    MoveInPlaylist { playlist: PlaylistId, track: TrackId, to: usize },
    /// Delete a track: into the Trash (30 days), or for good when
    /// [`Prefs::use_trash`] is off.
    TrashTrack(TrackId),
    /// Take a deleted track back out of the Trash.
    UndoTrash(TrackId),
    /// Delete every track of an album (like [`Command::TrashTrack`]).
    TrashRelease(ReleaseId),
    /// «Добавить в библиотеку» (`on`): every song of the album
    /// becomes a library track (the ones that only stream too), nothing
    /// downloads; off, the songs chosen one by one stay.
    AddWholeAlbum { id: ReleaseId, on: bool },
    /// «Отменить» of «Добавить в библиотеку»: the album is the
    /// songs chosen one by one again, its like standing for them only
    /// (`mine`) as before, and the songs left out of it by hand before
    /// (`excluded`) out again.
    UndoWholeAlbum { id: ReleaseId, mine: bool, excluded: Vec<TrackId> },
    /// Whether the other versions of an album (instrumentals, karaoke,
    /// remixes the user did not choose) play with it: the switch of their
    /// group on its page.
    AlbumVersions { id: ReleaseId, on: bool },
    /// «В библиотеку» (`on`) of a song outside the library: a library track,
    /// no like, nothing downloads; «Убрать из библиотеки» of one without a
    /// file. With «Отменить».
    TrackInLibrary { track: TrackId, on: bool },
    /// «Отменить» of [`Command::TrackInLibrary`]: the song put into the
    /// library by hand (`put`) and its choice on its album (`choice`:
    /// `Some(true)` added, `Some(false)` left out) as they were.
    TrackLibraryRestore { track: TrackId, put: bool, choice: Option<bool> },
    /// A song of an album into the user's songs of it (`mine`: a library
    /// track from then on) or out of them (its file, like and playlists
    /// stay), with «Отменить».
    AlbumTrackMine { release: ReleaseId, track: TrackId, mine: bool },
    /// The stored choice of a song of an album put back as it was
    /// (`Some(true)` added, `Some(false)` left out, `None` none): the undo
    /// of [`Command::AlbumTrackMine`].
    AlbumTrackChoice { release: ReleaseId, track: TrackId, choice: Option<bool> },
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
    /// Fill [`LibraryView::downloaded`] with its first page.
    LoadDownloaded,
    /// One more page of [`LibraryView::downloaded`].
    LoadMoreDownloaded,
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
    /// Fills [`ViewState::world`] with the world of this key.
    LoadWorld(String),
    LoadPlaylist(PlaylistId),
    LoadHome,
    /// A row of Главная was scrolled near its end: more of it, where its
    /// source has more.
    HomeMore(HomeRow),
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
    /// Every fake FLAC turned into an MP3 comes back from the Trash and plays
    /// again; the MP3s go to the Trash.
    RestoreConvertedFlac,
    ConfirmChoice { id: u64, option: Option<usize> },
    /// The ticked songs of a [`Confirm::pick`] card: accepted (`accept`,
    /// the fakes become MP3s) or not (kept as they are, never offered
    /// again). Only these sources change, never the rest of the card.
    ConfirmPicked { id: u64, sources: Vec<nikgob_core::model::SourceId>, accept: bool },
    /// «В файле другая песня» on a local track: its file goes to the
    /// Trash, the track is looked for again, and that file and the place it
    /// came from are never taken for it again.
    WrongFile(nikgob_core::model::TrackId),
    /// Takes [`Self::WrongFile`] back: the file plays again.
    UndoWrongFile(nikgob_core::model::TrackId),
    /// «Найти качество лучше» of a track with a file: the download
    /// sources look for a copy better than it, which waits in «Подтверди»;
    /// a track with no file goes to «Хочу скачать».
    FindBetter(nikgob_core::model::TrackId),
    /// «Источник и качество» of any track: its info into
    /// [`ViewState::track_info`], kept up to date while it is asked for;
    /// `None`: no longer.
    TrackInfo(Option<nikgob_core::model::TrackId>),
    /// «Найти качество лучше» of an album: a better copy of the album is
    /// looked for and downloaded, and waits as one card in «Подтверди»
    /// (each track and what replaces its file) for «Заменить»; tracks with
    /// the best file there is are left alone.
    FindBetterAlbum(ReleaseId),
    /// Takes the files [`Self::FindBetterAlbum`] replaced back, the whole
    /// album at once: the old files from the Trash, the new ones into it.
    UndoBetterAlbum(ReleaseId),
    /// «Обновить обложку» of an album: its art is picked again from its
    /// files and looked up online, the best of it becomes its cover.
    RefreshReleaseCover(ReleaseId),
    /// «Обновить обложку» of a service's album: it is fetched again, and the
    /// open artist's discography that shows it.
    RefreshStreamAlbumCover(RemoteId),
    /// «Превратить в MP3» of a track whose lossless file surely holds an
    /// MP3: its MP3 with the tags, cover and lyrics takes its place, the
    /// original goes to the Trash.
    FakeToMp3(nikgob_core::model::TrackId),
    /// Takes [`Self::FakeToMp3`] back, or what one «Превратить N в MP3»
    /// of a fakes card made, all at once: each `(mp3, original)` pair of
    /// sources that conversion made, exactly — the original comes back from
    /// the Trash and plays again, that MP3 goes to the Trash.
    UndoFakeToMp3(Vec<(nikgob_core::model::SourceId, nikgob_core::model::SourceId)>),
    /// «Разные песни» on a track of one song on two albums
    /// ([`ViewState::also_on`]): it is another song than the tracks it was
    /// linked with, and never linked with them again.
    RecordingApart(nikgob_core::model::TrackId),
    /// Folds the editions of [`EditionOffer::id`] into one album.
    MergeEditions(u64),
    /// Keeps them apart and never offers them again.
    KeepEditionsApart(u64),

    // Settings
    /// Every preference at once (the Settings page edits its copy).
    SetPrefs(Box<Prefs>),
    /// One preference, applied to the preferences as they are then.
    SetPref(PrefPatch),
    /// Move lossless FLAC/WAV masters to another drive or folder (copy -> hash verify -> delete old).
    MoveMastersDir(PathBuf),
    /// Prowlarr address and API key (stored in settings.toml, never shown again).
    SetIndexer { url: String, api_key: String },
    /// The Soulseek account and where it runs (stored in `[soulseek]` of
    /// settings.toml); an empty password or token keeps the saved one.
    SetSoulseek(SoulseekForm),
    /// Soulseek's sharing, applied at once: share the library, and the
    /// upload limit in KiB/s (0 = none).
    SetSoulseekShare { share: bool, upload_limit_kbps: u32 },
    /// Soulseek uploads at once (`upload_slots` in settings.toml).
    SetSoulseekSlots(u32),
    /// Asks Prowlarr whether it answers ([`ViewState::indexer_check`]) at the
    /// address and key typed in the settings (not saved yet); an empty address
    /// checks the saved one, an empty key uses the saved key.
    CheckIndexer { url: String, api_key: String },
    /// MusicBrainz contact and AcoustID key (stored in settings.toml).
    SetMetadata { contact: String, acoustid_key: Option<String> },
    /// The Last.fm API key of the graph of similar music → settings.toml
    /// (never into prefs); empty clears it.
    SetLastfmKey(String),
    /// The ListenBrainz token of the graph's popularity → settings.toml
    /// (never into prefs); empty clears it.
    SetListenbrainzToken(String),
    /// Opens `sources.toml` (Qobuz and Tidal instance lists) beside
    /// settings.toml. The settings page does not draw the button yet.
    OpenSourcesConfig,
    /// Video thumbnails (`i.ytimg.com`) of stream tracks drawn on screen:
    /// their real album art is looked up ([`ViewState::real_covers`]).
    RealCovers(Vec<String>),
    DismissToast(u64),

    // Streaming: accounts (stage 3). Every network call runs in the
    // background; the result shows up in `ViewState::accounts` / a toast.
    /// Logs in ([`AccountView::busy`] meanwhile; the prompt to act on is in
    /// [`AccountView::account`]). A Spotify personal app's Client ID is
    /// saved to settings.toml on success.
    StreamLogin { service: Service, input: LoginInput },
    StreamLogout(Service),
    /// The service's sound is asked for again now instead of after its
    /// pause ([`AccountView::audio_refused_until_hm`]).
    RetryStreamAudio(Service),
    /// A step of Spotify Soloist's setup ([`AccountView::soloist`]).
    Soloist(SoloistAction),
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
        context: QueueOrigin,
    },
    /// Play the merged tracks of the «Всё» search: library tracks and
    /// stream tracks in one queue ([`SearchAnswer::tracks`]).
    PlaySearch {
        tracks: Vec<SearchPick>,
        start: usize,
        context: QueueOrigin,
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
    /// The tracks of an album of a service after the current one / at the
    /// end of the queue (fetched as playing it does), like
    /// [`Command::PlayReleaseNext`] / [`Command::EnqueueRelease`].
    PlayStreamAlbumNext(RemoteId),
    EnqueueStreamAlbum(RemoteId),
    /// The same for a playlist of a service.
    PlayStreamPlaylistNext(RemoteId),
    EnqueueStreamPlaylist(RemoteId),
    /// An artist of a service the library does not know: its newest
    /// release played, after the current track or at the end of the queue
    /// (as «Слушать» of its page plays it).
    PlayStreamArtist(RemoteId),
    PlayStreamArtistNext(RemoteId),
    EnqueueStreamArtist(RemoteId),
    /// «Не нравится» (`dislike`) or its undo for a track: out of every
    /// recommendation, skipped now when it plays ([`Prefs::dislike_skips`]),
    /// its like gone. A track liked or played to the end often is asked
    /// about first, unless `sure`; `relike` gives the like back with an
    /// undo.
    Dislike { track: TrackId, dislike: bool, sure: bool, relike: bool },
    /// Like a stream track: it joins the library (marked "стрим"), the like

    /// goes to the services ([`Prefs::push_likes`]) and, without a file, a
    /// wishlist item downloads it ([`Prefs::like_download_scope`]).
    LikeStream { track: RemoteId, on: bool },
    /// [`Command::TrackInLibrary`] of a song of a service (its track is
    /// made first).
    StreamInLibrary { track: RemoteId, on: bool },
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
    /// «Скачать недостающие» of a library album: of an album only some
    /// songs of which are the user's, their songs without a file (a track
    /// item each); of any other, its missing files (one album item of «Хочу
    /// скачать»).
    DownloadRelease(ReleaseId),
    /// «Скачать весь альбом»: the whole album (one album item); it is added
    /// whole to the library too, as its files will be.
    DownloadWholeRelease(ReleaseId),
    /// Stream tracks into a library playlist (they join the library).
    AddStreamToPlaylist(PlaylistId, Vec<RemoteId>),
    /// The tracks of an album of a service into a library playlist.
    AddStreamAlbumToPlaylist(PlaylistId, RemoteId),
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
    /// «Отменить» of a «Не качать» on items of several kinds: the turned
    /// down ones come back ([`Self::WishlistRestore`]), and the tracks and
    /// albums whose own «Скачать» items left are asked for again.
    WishlistUndoDismiss { restore: Vec<WishlistId>, tracks: Vec<TrackId>, albums: Vec<ReleaseId> },
    /// Search a torrent for this item now (instead of next week).
    WishlistRetry(WishlistId),
    /// Search again now for every item nothing was found for (a new source
    /// may have it).
    WishlistRetryAll,
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
    /// Настройки → «Для разработчика» → «Демо-профиль»: done by the app,
    /// never sent on to the runtime.
    DevProfile(dev::DevProfile),
    /// «Загрузки» is (not) the page shown. Off it, the runtime publishes a
    /// torrent's changes but not its bytes, rates and progress, so seeding
    /// does not wake the window every second.
    DownloadsShown(bool),
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

    // Recommendations (stage 4)
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
    /// the rest of the day), «Новое для тебя» is looked for again, a mix of
    /// «Миксы для тебя» gets «Другой состав».
    RefreshMix { id: MixId },
    /// «Смешать с…»: a blend of these taste profiles (two or three, the
    /// card's first) plays and stands first in «Миксы для тебя» until
    /// 06:00.
    PlayBlend { profiles: Vec<u32> },
    /// «Моя волна» starts: plain, or with the filter of a card of «Миксы
    /// для тебя» (`preset`).
    PlayWave { preset: Option<MixKey> },
    /// A chip of the wave's header in the queue: the wave plays `filter`
    /// from its next tracks (the one playing goes on).
    SetWave(WaveFilter),
    /// «↻ Другой вайб»: the wave moves to another of the user's clusters
    /// at once and plays its first song as soon as it is there, the track
    /// playing left without a skip or any word of it.
    OtherVibe,
    /// «Скрыть карточку»: switches «Карточка наверху главной» off and says
    /// how to bring it back (the snackbar's «Вернуть»).
    HideHomeCard,
    /// «Скачать всё» of a mix whose songs have no file yet («Ещё не
    /// скачано»): every stream song of it goes to «Хочу скачать».
    DownloadMix { id: MixId },
    /// «Сыграть следующим» / «В очередь» of a tile of the day or the
    /// daylist: its songs after the current track / at the end of the queue.
    PlayMixNext { id: MixId },
    EnqueueMix { id: MixId },
    /// Lowers the sound to `gain` (0..=1, a factor on the current volume) over
    /// `fade_ms`; restored by itself `ttl_ms` after the last SetDuck. A new
    /// SetDuck replaces the gain (fading from the current one) and restarts ttl.
    SetDuck { gain: f32, fade_ms: u32, ttl_ms: u32 },
    /// Restores the sound over `fade_ms`. Harmless when not ducked.
    ClearDuck { fade_ms: u32 },
    /// Switches the output device: `None` = the system default; `Some(name)` =
    /// a name from `Prefs::output_devices`, or on Linux also a PulseAudio sink
    /// name (`alsa_output.usb-…analog-stereo`). Persisted like the Settings pick.
    SetOutputDevice(Option<String>),

    // The local REST API («Межсеть → API и MCP»), kept in `[api]` of
    // settings.toml and applied live.
    /// Its switch.
    SetApiEnabled(bool),
    /// This computer only, or the home network too.
    SetApiAccess(ApiAccess),
    /// Its port (1024–65535).
    SetApiPort(u16),
    /// A new token: the runtime keeps its hash, the secret stays with the
    /// dialog that shows it once.
    ApiCreateToken(ApiNewToken),
    /// «Отозвать» (by id): refused from now on; it goes for good once its
    /// snackbar's «Отменить» is gone.
    ApiRevokeToken(String),
    /// «Отменить» of a revoke.
    ApiRestoreToken(String),
    /// From the API server: where it listens, or why it does not.
    ApiStatus { listening: Option<String>, error: Option<String> },
    /// From the API server: these tokens were used (id, unix ms), at most
    /// once a second each.
    ApiTokensUsed(Vec<(String, i64)>),
    /// From the API server: one request's commands, run one after another
    /// with nothing between them, then `ack` is shown as
    /// [`ApiView::synced`] (acks come in the order they were sent), so the
    /// answer has their state. `actor`: who the request's token plays for
    /// (kept by the loop while the commands run, for the history).
    ApiRun { commands: Vec<Command>, ack: u64, actor: Option<ApiActor> },
    /// Inside an [`Command::ApiRun`]: the next list it makes plays shuffled
    /// or in order as this says, whatever the settings of its kind.
    ApiShuffle(bool),
    /// From the API: a song.link / album.link page for `target`, answered
    /// to `reply` as `{"link", "text"}`; no toast, nothing copied.
    ApiShare { target: ShareTarget, reply: ApiReply },
    /// From the API: the tags in the files of these tracks, answered as
    /// `{"tags": {"<track id>": {"title", …}}, "outside": [ids]}` (a track
    /// without a file is left out; one whose file lies outside the library
    /// is never read, only listed).
    ApiReadTags { tracks: Vec<TrackId>, reply: ApiReply },
    /// From the API: these tags written into the files and the library,
    /// each track's fields not named kept as its file has them. The second
    /// patch is what the confirmed plan showed of those fields: a track
    /// whose file has other values now fails and is not written. Answered
    /// as `{"done": [ids], "failed": [{"id", "error"}]}`.
    ApiRetag { tracks: Vec<(TrackId, TagPatch, TagPatch)>, reply: ApiReply },
    /// From the API: these tracks into the Trash at once, one snackbar with
    /// «Отменить» for all of them. Never deletes for good: nothing happens
    /// while the Trash is off.
    ApiTrash { tracks: Vec<TrackId>, what: String },
    /// From the API: the download sources searched for a track or an album
    /// without downloading anything, answered as `{"offers": […]}`.
    ApiSourceSearch { track: Option<TrackId>, release: Option<ReleaseId>, reply: ApiReply },
    /// From the API: a download too large to start by itself waits in
    /// «Загрузки → Подтверди» as one card; «Скачать» runs `commands`. The
    /// card's words are made from `lines` and `caller` when it is
    /// published, in the UI's language of that moment.
    ApiConfirmDownload { lines: Vec<ApiConfirmLine>, caller: String, commands: Vec<Command> },

    // The owner's devices («Межсеть → Устройства»): `[mesh]` of
    // settings.toml and the node table kept by the runtime, the network
    // part done by the mesh service.
    /// The mesh on or off (`[mesh] enabled`).
    MeshSetEnabled(bool),
    /// «Подключить» of a nearby device with the code it shows.
    MeshPairNearby { nearby_id: String, code: String },
    /// «Добавить устройство»: a one-time ticket for `nikgobd pair` (and the
    /// QR, the install line); the device that joins with it becomes `scope`.
    MeshCreateInvite { scope: MeshScope },
    /// The invite's dialog closed: the ticket no longer works.
    MeshCancelInvite,
    /// Joins the ring of the device that made `ticket`.
    MeshJoin { ticket: String },
    /// A device's name here (`None`: this device, `[mesh] name`).
    MeshRename { node_id: Option<String>, name: String },
    /// «Забыть»: gone at once with «Отменить» in the snackbar; revoked in
    /// the ring once the snackbar is gone.
    MeshForget { node_id: String },
    /// What a device stores.
    MeshSetProfile { node_id: String, profile: MeshProfile },
    /// «Свой» or «Гость» (the ring holder signs it anew).
    MeshSetScope { node_id: String, scope: MeshScope },
    /// From the mesh service: what is live now.
    MeshUpdate(Box<MeshSnapshot>),
    /// «Отменить» of a forget.
    MeshRestore { node_id: String },
    /// The pairing's result was seen: back to idle.
    MeshClearPairing,
    /// From the mesh service: a forget could not be done (the device stays
    /// in the ring); `why` in words.
    MeshForgetFailed { node_id: String, why: String },
    /// From the API: service tracks it found, remembered as the window's
    /// search results are, so the commands after it can act on them.
    ApiKnow(Vec<nikgob_core::streaming::StreamTrack>),
    /// From the API: a new playlist with the id the API chose, with these
    /// tracks.
    ApiCreatePlaylist { id: PlaylistId, name: String, tracks: Vec<TrackId> },
    /// «Оригиналы FLAC на ПК» (`[mesh] keep_masters`); the replicas act on
    /// it, nothing is deleted by the choice itself.
    MeshSetKeepMasters(KeepMasters),
    /// «Избранное» of a device: «Любимые» and the playlists it keeps.
    MeshSetPins { node_id: String, pins: MeshPins },
    /// The pause on a transfer's line («Карта сети»): the files to that
    /// device wait (the replicas honour it).
    MeshPauseTransfer { node_id: String, paused: bool },
    /// The answer to [`MeshView::forget_ask`]: forget anyway, or not.
    MeshForgetAnswer { node_id: String, forget: bool },
    /// Turns the library's sync with a paired device on or off (the
    /// device's settings row).
    MeshSetSync { node_id: String, on: bool },
    /// From the mesh's sync loop to the worker: how the sync with a device
    /// goes.
    MeshSyncReport { node_id: String, sync: MeshSyncView },

    // Playing on another device (like Spotify Connect, [`RemoteTarget`]).
    /// Some: what plays here goes to that device and it becomes the target
    /// (this zone parks). None: the target's music comes back here (loaded
    /// into this zone, the remote paused) and the target is this device.
    OutputTransfer { node_id: Option<String> },
    /// That device becomes the target as it is (nothing moves; this zone
    /// parks).
    OutputAttach { node_id: String },
    /// The target is this device again, nothing moved (the parked queue is
    /// back, still paused).
    OutputDetach,
    /// After the link was lost: this zone plays the last mirrored state from
    /// a moment before where it was, and the target is this device.
    OutputContinueHere,
    /// «Отменить» of a transfer onto a device that was playing: its queue,
    /// place and pause come back, and the target is what it was.
    OutputUndoTransfer,
    /// From the API: this zone's transferable state, answered as a
    /// [`ZoneSnapshot`].
    ApiSnapshot { reply: ApiReply },
    /// From the API: this zone's queue replaced by `snapshot` (its tracks
    /// checked by the API already), playing unless it says paused; `from`
    /// is the device that sent it (missing files are fetched from it).
    /// Answered as `{"previous": ZoneSnapshot | null}`.
    ApiLoad { snapshot: Box<ZoneSnapshot>, from: Option<String>, reply: ApiReply },
    /// From the API: player commands a remote forwarded (`from`: its
    /// device), run here as they are.
    ConnectRun { from: Option<String>, commands: Vec<Command> },
    /// From the mesh service: what came of a [`ConnectJob`].
    ConnectReport(Box<ConnectReport>),
    /// From the API (`PUT /v1/player/output`): a transfer to `node_id`
    /// (None: back here), or with `attach` an attach (None: a detach),
    /// answered when it is done as `{"done": bool, "message"?: text}`.
    ApiTransfer { node_id: Option<String>, attach: bool, reply: ApiReply },

    // The copies of the library on the owner's devices.
    /// From the worker to the mesh: right before the masters of a
    /// «Подтверди» card (`card`) go to the Trash, each trusted device
    /// (none of `leaving`) is asked to read its copies of `hashes` again;
    /// answered with [`Command::ReplicaRechecked`].
    ReplicaRecheck { card: u64, hashes: Vec<String>, need: u32, leaving: Vec<String> },
    /// From the mesh: the masters of `card` that still have `need` verified
    /// copies (`ok`), and why none could be checked (`failed`).
    ReplicaRechecked { card: u64, ok: Vec<String>, failed: Option<String> },
    /// «Отменить» of masters moved to the Trash: they go back.
    ReplicaUndoTrash { ids: Vec<i64> },
    /// From the mesh: what the devices hold of this computer's masters
    /// changed (the masters that may go are looked at again).
    ReplicaCopiesChanged,
    /// From the mesh: a pulled file was put into this device's library
    /// (the music's size is counted again).
    ReplicaFileKept,
}
