//! Content pages and the menus and dialogs they open.

mod brand;
pub(super) mod detail;
mod dialogs;
mod downloads;
mod equalizer;
mod cards;
pub(crate) mod home;
mod library;
mod search;
mod shelf;
mod settings;
pub(crate) mod settings_help;
mod sources;
mod streams;
mod wishlist;
mod world;
mod worlds;
pub mod widgets;

pub use search::{restore_search_scroll, search_results, search_scroll};
pub use settings_help::SettingId;
pub(crate) use worlds::choose as choose_world;

use egui::{Context, Rect, Response, Ui, UiBuilder, vec2};
use nikgob_view::{
    Command, LIKED_PLAYLIST, LibraryTab, ReleaseId, Route, TorrentFiles, TrackId, TrackRow, ViewState,
};

use super::PaneTab;
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, DialogAction, FabMenu, FabMenuItem, LoadingIndicator, Menu, MenuItem, Radii,
        SearchBar, Size, SnackbarHost, SnackbarItem, Style, TextInputDialog,
        dialog::TextInputResult,
    },
    tokens::shape,
};

/// Which text-input dialog is open.
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    NewPlaylist { then_add: Vec<TrackId> },
    /// A new playlist for stream tracks or a service album.
    NewPlaylistStream(StreamAdd),
    /// A magnet link for a wishlist item without a torrent.
    WishMagnet(nikgob_view::WishlistId),
    RenamePlaylist(nikgob_view::PlaylistId),
    Magnet,
}

/// What the "В плейлист" picker of an album card or a service adds: stream
/// tracks, a service album by its id, or a library album by its id (the
/// runtime finds the tracks of either album).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum StreamAdd {
    Tracks(Vec<nikgob_view::RemoteId>),
    Album(nikgob_view::RemoteId),
    Release(nikgob_view::ReleaseId),
}

impl StreamAdd {
    fn into_playlist(self, playlist: nikgob_view::PlaylistId) -> Command {
        match self {
            Self::Tracks(tracks) => Command::AddStreamToPlaylist(playlist, tracks),
            Self::Album(album) => Command::AddStreamAlbumToPlaylist(playlist, album),
            Self::Release(album) => Command::AddAlbumToPlaylist(playlist, album),
        }
    }
}

/// Opens the "В плейлист" picker for a service album; set from its card's
/// menu, which has no `PageState` at hand.
pub(crate) fn request_stream_album_to_playlist(ctx: &Context, album: nikgob_view::RemoteId, anchor: Option<Rect>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("stream-album-playlist-request"), Some((album, anchor))));
}

/// Opens the "В плейлист" picker for a library album; set from its card's
/// menu, which has no `PageState` at hand.
pub(crate) fn request_release_to_playlist(ctx: &Context, album: nikgob_view::ReleaseId, anchor: Option<Rect>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("release-playlist-request"), Some((album, anchor))));
}

/// What a delete confirmation is about.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Doomed {
    Tracks(Vec<TrackId>),
    Release(nikgob_view::ReleaseId),
}

/// Asks [`PageState::overlays`] to confirm deleting an album; set from menus
/// that have no `PageState` at hand. `(album, title, for good)`.
pub(crate) fn request_release_delete(ctx: &Context, id: nikgob_view::ReleaseId, title: &str, forever: bool) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("release-delete-request"), (id, title.to_owned(), forever)));
}

/// «Радио по треку» in the track menus.
pub(crate) const RADIO_BY_TRACK: &str = "Радио по треку";
/// The menu item of a track whose file holds another song.
pub(crate) const WRONG_FILE: &str = "В файле другая песня";

/// «Не нравится» of a track, or its undo when it is disliked.
pub(crate) fn dislike_item(disliked: bool) -> MenuItem {
    MenuItem::new(if disliked { "Убрать «Не нравится»" } else { "Не нравится" }).icon(Icon::ThumbDown).selected(disliked)
}

pub(crate) fn dislike_command(track: nikgob_view::TrackId, marks: &nikgob_view::DislikesView) -> Command {
    Command::Dislike { track, dislike: !marks.track(track), sure: false, relike: false }
}

/// «Не рекомендовать исполнителя» of `artist`, or its undo.
pub(crate) fn ban_item(artist: Option<nikgob_view::ArtistId>, marks: &nikgob_view::DislikesView) -> MenuItem {
    let banned = artist.is_some_and(|a| marks.artist(a));
    MenuItem::new(if banned { "Снова рекомендовать исполнителя" } else { "Не рекомендовать исполнителя" })
        .icon(Icon::Block)
        .enabled(artist.is_some())
}

pub(crate) fn ban_command(artist: nikgob_view::ArtistId, marks: &nikgob_view::DislikesView) -> Command {
    Command::RecFeedback { target: nikgob_view::RecTarget::Artist(artist), dislike: !marks.artist(artist) }
}

fn marks_key() -> egui::Id {
    egui::Id::new("md3e-dislike-marks")
}

/// Keeps the marks for the menus drawn without the view state (cards).
pub(crate) fn set_marks(ctx: &Context, marks: &nikgob_view::DislikesView) {
    let same = ctx.data(|d| d.get_temp::<std::sync::Arc<nikgob_view::DislikesView>>(marks_key())).is_some_and(|m| *m == *marks);
    if !same {
        ctx.data_mut(|d| d.insert_temp(marks_key(), std::sync::Arc::new(marks.clone())));
    }
}

/// The marks [`set_marks`] kept.
pub(crate) fn marks(ctx: &Context) -> std::sync::Arc<nikgob_view::DislikesView> {
    ctx.data(|d| d.get_temp(marks_key())).unwrap_or_default()
}



/// What a track menu does. Every track menu (a library row, a playlist
/// row, a service's song) draws its rows from [`track_rows`] in one order;
/// the caller appends the rows of the place the track is in (`Remove`, the
/// deletes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackAct {
    Next,
    Queue,
    Playlist,
    Radio,
    Album,
    AlsoOn,
    Apart,
    Artist,
    Info,
    Share,
    /// «Скорость…»: the version the track plays as.
    Speed,
    /// A file: its tags; no file: download it (no like).
    Fetch,
    AlbumDownload,
    Torrents,
    FindSong,
    WrongFile,
    Dislike,
    Ban,
    Remove,
    Trash,
    Purge,
}

/// What a track menu is built over: the common rows are always there (some
/// greyed), the optional ones only for a track that has them.
pub struct TrackMenuOf {
    pub playable: bool,
    pub has_album: bool,
    pub has_artists: bool,
    /// The other album of the same song («Есть и на альбоме «…»»).
    pub also_on: Option<String>,
    /// «Источник и качество» (the side pane's part, off by default).
    pub info: bool,
    pub fetch: MenuItem,
    pub torrents: Option<&'static str>,
    pub find_song: Option<&'static str>,
    pub wrong_file: bool,
    pub dislike: MenuItem,
    pub ban: MenuItem,
}

/// The one list of a track's actions: play next / queue / to a playlist /
/// radio │ album (and the other album of the song) / artist / info / share
/// │ the files │ «Не нравится» / «Не рекомендовать исполнителя».
pub fn track_rows(o: TrackMenuOf) -> Vec<widgets::Row<TrackAct>> {
    use TrackAct as A;
    use widgets::Row;
    let mut rows = vec![
        Row::Item(A::Next, MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic).enabled(o.playable)),
        Row::Item(A::Queue, MenuItem::new("В конец очереди").icon(Icon::QueueMusic).enabled(o.playable)),
        Row::Item(A::Playlist, MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd)),
        Row::Item(A::Radio, MenuItem::new(RADIO_BY_TRACK).icon(Icon::Radio)),
        Row::Divider,
        Row::Item(A::Album, MenuItem::new("К альбому").icon(Icon::Album).enabled(o.has_album)),
    ];
    // One song on two albums: the other album, and the way out of it.
    if let Some(album) = o.also_on {
        rows.push(Row::Item(A::AlsoOn, MenuItem::new(format!("Есть и на альбоме «{album}»")).icon(Icon::Album)));
        rows.push(Row::Item(A::Apart, MenuItem::new("Разные песни").icon(Icon::Close)));
    }
    rows.push(Row::Item(A::Artist, MenuItem::new("К исполнителю").icon(Icon::Person).enabled(o.has_artists)));
    if o.info {
        rows.push(Row::Item(A::Info, MenuItem::new("Источник и качество").icon(Icon::Info)));
    }
    rows.push(Row::Item(A::Share, MenuItem::new("Поделиться").icon(Icon::Share)));
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Fetch, o.fetch));
    rows.push(Row::Item(A::AlbumDownload, MenuItem::new("Скачать альбом").icon(Icon::Download).enabled(o.has_album)));
    if let Some(t) = o.torrents {
        rows.push(Row::Item(A::Torrents, MenuItem::new(t).icon(Icon::Search)));
    }
    // A clip: its song by hand; a stream from YouTube: another version.
    if let Some(f) = o.find_song {
        rows.push(Row::Item(A::FindSong, MenuItem::new(f).icon(Icon::MusicNote)));
    }
    // A file that holds another song: to the Trash, looked for again.
    if o.wrong_file {
        rows.push(Row::Item(A::WrongFile, MenuItem::new(WRONG_FILE).icon(Icon::Delete)));
    }
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Dislike, o.dislike));
    rows.push(Row::Item(A::Ban, o.ban));
    rows
}

/// «Скорость…» of a library track's menu, after «Поделиться»: there while
/// the speed of tracks is on in the settings, marked while the track is not
/// played as recorded.
pub(crate) fn with_speed_row(
    mut rows: Vec<widgets::Row<TrackAct>>,
    speeds: &nikgob_view::TrackSpeeds,
    track: TrackId,
) -> Vec<widgets::Row<TrackAct>> {
    if !speeds.enabled {
        return rows;
    }
    let changed = speeds.of(track) != nikgob_view::SpeedVariant::Normal;
    let item = MenuItem::new("Скорость…").icon(Icon::GraphicEq).selected(changed);
    let at = rows
        .iter()
        .position(|r| matches!(r, widgets::Row::Item(TrackAct::Share, _)))
        .map_or(rows.len(), |i| i + 1);
    rows.insert(at, widgets::Row::Item(TrackAct::Speed, item));
    rows
}

/// The five versions of «Скорость», the one `track` plays as marked.
pub(crate) fn speed_menu(id_salt: impl egui::AsId, speeds: &nikgob_view::TrackSpeeds, track: TrackId) -> Menu {
    let now = speeds.of(track);
    nikgob_view::SpeedVariant::ALL
        .into_iter()
        .fold(Menu::new(id_salt), |menu, v| menu.item(MenuItem::new(v.label()).selected(v == now)))
}

/// The command of item `picked` of [`speed_menu`].
pub(crate) fn speed_command(track: TrackId, picked: usize) -> Option<Command> {
    let variant = *nikgob_view::SpeedVariant::ALL.get(picked)?;
    Some(Command::SetTrackSpeed { track, variant })
}

/// [`track_rows`] of a library track (in the library or in a playlist).
pub(crate) fn library_track_rows(
    track: &TrackRow,
    marks: &nikgob_view::DislikesView,
    info: bool,
    also: Option<&(ReleaseId, String)>,
) -> Vec<widgets::Row<TrackAct>> {
    // A file: its tags and a better copy; no file: download it (no like)
    // or look for a release by hand.
    let fetch = if track.has_local {
        MenuItem::new("Редактировать теги…").icon(Icon::Edit)
    } else {
        MenuItem::new("Скачать").icon(Icon::Download)
    };
    track_rows(TrackMenuOf {
        playable: true,
        has_album: track.release_id.is_some(),
        has_artists: !track.artists.linked().is_empty(),
        also_on: also.map(|(_, a)| a.clone()),
        info,
        fetch,
        torrents: Some(if track.has_local { "Найти качество лучше…" } else { "Искать раздачу…" }),
        find_song: super::find_song::label(track),
        wrong_file: track.has_local,
        dislike: dislike_item(marks.track(track.id)),
        ban: ban_item(track.artist_id(), marks),
    })
}

/// The library's deletes, after a library row's [`library_track_rows`].
fn push_deletes(rows: &mut Vec<widgets::Row<TrackAct>>, track: &TrackRow) {
    rows.push(widgets::Row::Divider);
    rows.push(widgets::Row::Item(TrackAct::Trash, MenuItem::new("Удалить из библиотеки").icon(Icon::Delete)));
    rows.push(widgets::Row::Item(
        TrackAct::Purge,
        MenuItem::new("Удалить навсегда").icon(Icon::Delete).enabled(track.has_local),
    ));
}

/// The command of «Радио по треку» for a library track.
pub(crate) fn radio_by_track(track: TrackId) -> Command {
    Command::StartRadio { seed: nikgob_view::RadioSeed::Track(track) }
}

/// The tabs and chips a page is on, beyond its route: what a step of the
/// history brings back with the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tabs {
    library: LibraryTab,
    downloads: usize,
    settings: usize,
}

/// A page scrolled to `y` once its content is tall enough (it may still be
/// loading), for at most `frames` frames.
#[derive(Clone, Debug, PartialEq)]
struct ScrollRestore {
    route: Route,
    y: f32,
    frames: u32,
}

/// Page-level presentation state that outlives a frame.
#[derive(Default)]
pub struct PageState {
    /// How far the page on screen is scrolled (last frame).
    pub(crate) scroll_now: f32,
    scroll_restore: Option<ScrollRestore>,
    /// The library tab came back from the history: it loads what it shows.
    reopen_library_tab: bool,
    /// A navigation requested from inside an overlay or menu.
    pub pending_nav: Option<Route>,
    /// A settings row a "настроить" link asked for: Settings opens on its
    /// tab, scrolls to it and makes it glow.
    pub focus_setting: Option<SettingId>,
    /// The whole group of [`Self::focus_setting`] is shown, not the row.
    pub focus_group: bool,
    /// Settings tab: [`settings_help::TABS`] (0 «Музыка» … 5 «Система»).
    settings_tab: usize,
    /// «Просто · Подробно» held for this session whatever the prefs say
    /// (the settings sweep, tests); `None`: `Prefs::settings_deep`.
    settings_deep: Option<bool>,
    /// A link to a row «Просто» does not show opened this visit of
    /// Settings in «Подробно» (the picked mode stays).
    settings_visit_deep: bool,
    /// The pass Settings was last drawn in (a new visit after a gap).
    settings_seen: u64,
    /// What Settings keeps for this visit: its stage, its Ctrl+Z.
    settings_memory: settings::Memory,
    /// Latest bass level for the ambient glow (set by the host each frame).
    pub levels_bass: f32,
    /// Ask the shell to open a pane tab (e.g. "Источник и качество").
    pub open_pane: Option<PaneTab>,
    /// A page asked to go back (its ← button).
    pub go_back: bool,
    /// A page asked for the search bar («Найти музыку» on an empty Home).
    pub focus_search: bool,
    /// A page asked for Now Playing («Сейчас играет» on the card of Главная).
    pub open_now_playing: bool,
    pub(crate) library_tab: LibraryTab,
    /// The instant filter of Библиотека (the current tab by title and
    /// artist), and what «Треки» last asked the runtime for.
    library_filter: String,
    track_filter_sent: String,
    release_filter: usize,
    /// Library albums by source: 0 all, 1 with files, 2 stream.
    release_source: usize,
    downloads_tab: usize,
    ask: Option<Ask>,
    ask_text: String,
    confirm_delete: Option<nikgob_view::PlaylistId>,
    /// Tracks or an album waiting for the "Удалить" confirmation (into the
    /// Trash, or for good when the Trash is off).
    confirm_trash: Option<(Doomed, String)>,
    /// ... and for "Удалить навсегда".
    confirm_purge: Option<(Doomed, String)>,
    /// "Очистить корзину".
    confirm_empty_trash: bool,
    /// "Очистить сейчас" confirmation on the seeding tab.
    confirm_clean: bool,
    /// Which torrent the files picker shows (result, job, file count) and the ticked files.
    files_key: Option<(Option<usize>, Option<u64>, usize)>,
    files_sel: std::collections::BTreeSet<usize>,
    /// The picker already answered; waiting for the runtime to close it.
    files_done: bool,
    indexer_url: String,
    indexer_key: String,
    /// «Soulseek» on «Загрузки»: what is typed (the password and the
    /// token are never shown back), and whether the fields were filled from
    /// the saved settings.
    slsk_remote: bool,
    slsk_user: String,
    slsk_pass: String,
    slsk_url: String,
    slsk_token: String,
    slsk_synced: bool,
    acoustid_key: String,
    /// What is typed into «Ключ Last.fm» before «Сохранить».
    lastfm_key: String,
    /// What is typed into «Токен ListenBrainz» before «Сохранить».
    listenbrainz_token: String,
    metadata_contact: String,
    metadata_synced: bool,
    /// What is typed into «Как к тебе обращаться» (Главная's question and
    /// the settings row) before it is kept.
    name_draft: String,
    name_synced: bool,
    /// «Сервисы»: what is typed into the Spotify Client ID and the
    /// YouTube cookies fields, and which folds are open.
    /// Spotify Soloist's setup: the open dialog, the key being typed, the
    /// flow (each step's dialog opens when the one before is done), the
    /// one-time offer shown this run, a step to send.
    soloist_dialog: sources::SoloistDialog,
    soloist_key: String,
    soloist_flow: bool,
    soloist_offered: bool,
    soloist_action: Option<nikgob_view::SoloistAction>,
    sp_client_id: String,
    yt_cookies: String,
    yt_form: bool,
    /// SoundCloud's pasted token and whether its field is open.
    sc_token: String,
    sc_form: bool,
    track_window: (usize, usize),
    /// Tracks waiting for the "В плейлист" picker.
    pending_add: Option<Vec<TrackId>>,
    pending_add_anchor: Option<Rect>,
    /// The track waiting for the «Скорость» picker.
    pending_speed: Option<TrackId>,
    /// (playlist name, tracks) to add once the new playlist appears.
    add_after_create: Option<(String, Vec<TrackId>)>,
    /// Stream tracks waiting for the "В плейлист" picker, and for a new
    /// playlist to appear.
    pending_add_stream: Option<StreamAdd>,
    stream_after_create: Option<(String, StreamAdd)>,
}

/// The library as a list (true) or a grid.
pub(crate) fn set_library_list(ctx: &Context, list: bool) {
    library::set_list_view(ctx, list);
}

impl PageState {
    pub(crate) fn set_library_filter(&mut self, text: &str) {
        self.library_filter = text.to_owned();
    }

    /// Opens the "В плейлист" picker for stream tracks.
    pub(crate) fn add_stream_to_playlist(&mut self, tracks: Vec<nikgob_view::RemoteId>, anchor: Option<Rect>) {
        self.pending_add_stream = Some(StreamAdd::Tracks(tracks));
        self.pending_add_anchor = anchor;
    }

    /// Shows `tab` of «Загрузки» (a route that names one).
    pub(crate) fn set_downloads_tab(&mut self, tab: nikgob_view::DownloadsTab) {
        self.downloads_tab = downloads::tab_index(tab);
    }

    pub(crate) fn unfold_sources(&mut self) {
        self.yt_form = true;
    }

    /// The tabs the pages are on.
    pub(crate) fn tabs(&self) -> Tabs {
        Tabs { library: self.library_tab, downloads: self.downloads_tab, settings: self.settings_tab }
    }

    /// Back on `tabs` (a step of the history).
    pub(crate) fn set_tabs(&mut self, tabs: Tabs) {
        if tabs.library != self.library_tab {
            self.reopen_library_tab = true;
        }
        self.library_tab = tabs.library;
        self.downloads_tab = tabs.downloads;
        self.settings_tab = tabs.settings;
    }

    /// The page of `route` shows from `y` down once it can.
    pub(crate) fn scroll_to(&mut self, route: Route, y: f32) {
        // About two seconds for its content to arrive.
        self.scroll_restore = Some(ScrollRestore { route, y, frames: 120 });
    }

    /// Where the page waits to be scrolled to (tests).
    #[cfg(test)]
    pub(crate) fn scroll_target(&self) -> Option<f32> {
        self.scroll_restore.as_ref().map(|r| r.y)
    }

    /// «Просто» (false) or «Подробно» (true) on Settings for this session,
    /// whatever the prefs say (the settings sweep shoots both).
    pub(crate) fn set_settings_deep(&mut self, deep: bool) {
        self.settings_deep = Some(deep);
    }

    /// Opens Settings at row `id` (its tab, scrolled into view, glowing).
    pub fn open_setting(&mut self, id: SettingId) {
        self.focus_setting = Some(id);
        self.focus_group = false;
        self.pending_nav = Some(Route::Settings);
    }

    /// The typed fields of Settings («Как к тебе обращаться», the
    /// MusicBrainz contact) read the prefs again when next shown (the
    /// settings sweep changes the prefs under them).
    pub(crate) fn forget_typed(&mut self) {
        self.name_synced = false;
        self.metadata_synced = false;
    }

    /// Opens Settings at the group that holds row `id`, scrolled into view
    /// and briefly tinted, so it is clear which settings these are.
    pub fn open_setting_group(&mut self, id: SettingId) {
        self.open_setting(id);
        self.focus_group = true;
    }

    /// Goes where a message points ([`nikgob_view::Link`]).
    pub fn follow(&mut self, link: nikgob_view::Link) {
        use nikgob_view::{Link, SettingLink};
        match link {
            Link::Route(r) => self.pending_nav = Some(r),
            Link::Setting(s) => self.open_setting(match s {
                SettingLink::Spotify => SettingId::Spotify,
                SettingLink::SpotifyPlayback => SettingId::SpotifyPlayback,
                SettingLink::Youtube => SettingId::YouTube,
                SettingLink::Torrents => SettingId::Prowlarr,
                SettingLink::LikeDownload => SettingId::LikeDownload,
            }),
        }
    }

    /// The "Добавить" FAB menu (§2.1): the rail's FAB turns into a close button
    /// with pills below it. The rail already drew a plain FAB at `fab`; the
    /// menu takes its place so it can morph.
    pub fn add_menu(&mut self, ui: &mut Ui, fab: &Response, out: &mut Vec<Command>) {
        let s = Theme::get(ui.ctx()).scheme;
        ui.painter()
            .rect_filled(fab.rect.expand2(vec2(20.0, 20.0)), 0, s.surface);
        let extended = fab.rect.width() > 76.0;
        let items = [
            FabMenuItem::new(Icon::UploadFile, "Файлы…"),
            FabMenuItem::new(Icon::Folder, "Папку…"),
            FabMenuItem::new(Icon::AttachFile, ".torrent…"),
            FabMenuItem::new(Icon::Link, "Magnet-ссылку"),
            FabMenuItem::new(Icon::PlaylistAdd, "Новый плейлист"),
        ];
        let mut menu = FabMenu::new("add-fab-menu", Icon::Add, &items);
        if extended {
            menu = menu.label("Добавить");
        }
        let slot = Rect::from_min_size(fab.rect.min, vec2(260.0, 56.0));
        let mut cui = ui.new_child(UiBuilder::new().max_rect(slot).id_salt("add-fab-slot"));
        let picked = menu.show(&mut cui).clicked;
        match picked {
            Some(0) => dialogs::files(
                |d| {
                    d.set_title("Добавить файлы")
                        .add_filter("Музыка", &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "alac", "aiff"])
                },
                Command::ImportFiles,
                out,
            ),
            Some(1) => dialogs::folder(|d| d.set_title("Добавить папку"), Command::ImportFolder, out),
            Some(2) => dialogs::file(
                |d| d.set_title("Открыть .torrent").add_filter("Торрент", &["torrent"]),
                Command::AddTorrentFile,
                out,
            ),
            Some(3) => self.open_ask(Ask::Magnet),
            Some(4) => self.open_ask(Ask::NewPlaylist { then_add: Vec::new() }),
            _ => {}
        }
    }

    fn open_ask(&mut self, ask: Ask) {
        self.ask_text.clear();
        self.ask = Some(ask);
    }

    /// The track menu, keyed by its anchor: the ⋮ menu and the right-click
    /// menu of one row (or the same track in two lists) must not share an id,
    /// or each closes the other and both draw the same widgets.
    fn track_menu_items(
        anchor: egui::Id,
        track: &TrackRow,
        marks: &nikgob_view::DislikesView,
        speeds: &nikgob_view::TrackSpeeds,
        info: bool,
        also: Option<&(ReleaseId, String)>,
    ) -> (Menu, Vec<TrackAct>) {
        let mut rows = with_speed_row(library_track_rows(track, marks, info, also), speeds, track.id);
        push_deletes(&mut rows, track);
        let (menu, acts) = widgets::menu_of(("track-menu", anchor), rows);
        (menu.align_right(true), acts)
    }

    /// Does a library track's action (all but a playlist's `Remove`).
    pub(crate) fn track_apply(
        &mut self,
        ctx: &Context,
        act: TrackAct,
        track: &TrackRow,
        marks: &nikgob_view::DislikesView,
        also: Option<&(ReleaseId, String)>,
        anchor: Option<Rect>,
        out: &mut Vec<Command>,
    ) -> Option<Route> {
        use TrackAct as A;
        match act {
            A::Next => out.push(Command::PlayNext(vec![track.id])),
            A::Queue => out.push(Command::Enqueue(vec![track.id])),
            A::Playlist => {
                self.pending_add = Some(vec![track.id]);
                self.pending_add_anchor = anchor;
            }
            A::Radio => out.push(radio_by_track(track.id)),
            A::Album => return track.release_id.map(Route::Release),
            A::AlsoOn => return also.map(|(release, _)| Route::Release(*release)),
            A::Apart => out.push(Command::RecordingApart(track.id)),
            // Several artists: a chooser.
            A::Artist => return crate::screens::artist_links::go_to(ctx, crate::screens::artist_links::library(&track.artists)),
            A::Info => self.open_pane = Some(PaneTab::Info),
            A::Share => out.push(Command::Share(nikgob_view::ShareTarget::Track(track.id))),
            A::Speed => {
                self.pending_speed = Some(track.id);
                self.pending_add_anchor = anchor;
            }
            A::Fetch if track.has_local => out.push(Command::OpenTagEditor(track.id)),
            A::Fetch => out.push(Command::DownloadTracks(vec![track.id])),
            // The whole album the track is from, the files it lacks.
            A::AlbumDownload => {
                if let Some(r) = track.release_id {
                    out.push(Command::DownloadRelease(r));
                }
            }
            A::Torrents => out.push(Command::SearchTorrents(format!(
                "{} {}{}",
                track.artist,
                track.album.as_deref().unwrap_or(&track.title),
                if track.has_local { " FLAC" } else { "" }
            ))),
            A::FindSong => out.push(Command::FindSong { track: track.id, query: None }),
            A::WrongFile => out.push(Command::WrongFile(track.id)),
            A::Dislike => out.push(dislike_command(track.id, marks)),
            A::Ban => out.extend(track.artist_id().map(|a| ban_command(a, marks))),
            A::Trash => self.confirm_trash = Some((Doomed::Tracks(vec![track.id]), format!("«{}»", track.title))),
            A::Purge => self.confirm_purge = Some((Doomed::Tracks(vec![track.id]), format!("«{}»", track.title))),
            A::Remove => {}
        }
        None
    }

    /// ⋮ on a track row.
    pub fn track_menu_attach(
        &mut self,
        anchor: &Response,
        track: &TrackRow,
        state: &ViewState,
        out: &mut Vec<Command>,
    ) -> Option<Route> {
        let info = state.prefs.pane_info;
        let also = state.also_on.get(&track.id);
        let (menu, acts) = Self::track_menu_items(anchor.id, track, &state.dislikes, &state.speeds, info, also);
        let act = menu.attach(anchor).and_then(|i| acts.get(i).copied())?;
        self.track_apply(&anchor.ctx, act, track, &state.dislikes, also, None, out)
    }

    /// Right click on a track row.
    pub fn track_context_menu(
        &mut self,
        row: &Response,
        track: &TrackRow,
        state: &ViewState,
        out: &mut Vec<Command>,
    ) -> Option<Route> {
        if Menu::context_idle(row, row.id.with("ctx")) {
            return None;
        }
        let info = state.prefs.pane_info;
        let also = state.also_on.get(&track.id);
        let (menu, acts) = Self::track_menu_items(row.id.with("ctx"), track, &state.dislikes, &state.speeds, info, also);
        let act = menu.context_menu(row).and_then(|i| acts.get(i).copied())?;
        self.track_apply(&row.ctx, act, track, &state.dislikes, also, None, out)
    }

    /// The ⋮ menu for the playing track (Now Playing header).
    pub fn track_menu(
        &mut self,
        _ui: &mut Ui,
        anchor: &Response,
        track: Option<&TrackRow>,
        marks: &nikgob_view::DislikesView,
        out: &mut Vec<Command>,
    ) {
        if let Some(t) = track {
            let (menu, acts) = Self::track_menu_items(anchor.id, t, marks, &nikgob_view::TrackSpeeds::default(), true, None);
            if let Some(act) = menu.attach(anchor).and_then(|i| acts.get(i).copied())
                && let Some(r) = self.track_apply(&anchor.ctx, act, t, marks, None, None, out)
            {
                self.pending_nav = Some(r);
            }
        }
    }

    /// "В плейлист" picker anchored to a button.
    pub fn playlist_menu(
        &mut self,
        _ui: &mut Ui,
        anchor: &Response,
        tracks: Vec<TrackId>,
        state: &ViewState,
        out: &mut Vec<Command>,
    ) {
        if anchor.clicked() {
            self.pending_add = Some(tracks);
        }
        self.pending_add_anchor = Some(anchor.rect);
        let _ = (state, out);
    }

    /// The file picker of a torrent: checkboxes with sizes, all/none.
    fn files_dialog(&mut self, ctx: &Context, f: &TorrentFiles, out: &mut Vec<Command>) {
        let s = Theme::get(ctx).scheme;
        let key = (f.result, f.job, f.files.len());
        if self.files_key != Some(key) {
            self.files_key = Some(key);
            self.files_sel = f.files.iter().map(|(i, ..)| *i).collect();
        }
        let ready = !f.loading && f.error.is_none() && !f.files.is_empty();
        let chosen = |sel: &std::collections::BTreeSet<usize>| -> (usize, u64) {
            f.files
                .iter()
                .filter(|(i, ..)| sel.contains(i))
                .fold((0, 0), |(n, b), (_, _, size)| (n + 1, b + size))
        };
        let (n_sel, _) = chosen(&self.files_sel);
        let can_download = f.result.is_some() && ready && n_sel > 0;
        let mut open = true;
        let sel = &mut self.files_sel;
        let r = crate::components::Dialog::new("torrent-files", "Выбрать файлы")
            .supporting(if f.name.is_empty() { "Раздача" } else { f.name.as_str() })
            .max_width(560.0)
            .action("Отмена")
            .action_with(DialogAction::new("Скачать").enabled(can_download))
            .show_with(ctx, &mut open, |ui| {
                ui.add_space(16.0);
                if f.loading {
                    ui.horizontal(|ui| {
                        ui.add(LoadingIndicator::new().size(32.0));
                        ui.label(TypeRole::BodyMedium.style().rich("Читаю список файлов…", s.on_surface_variant));
                    });
                } else if let Some(e) = &f.error {
                    ui.label(TypeRole::BodyMedium.style().rich(e, s.error));
                } else if f.files.is_empty() {
                    ui.label(TypeRole::BodyMedium.style().rich("В раздаче нет файлов.", s.on_surface_variant));
                } else {
                    let (n, bytes) = chosen(sel);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if ui.add(Button::new("Все").style(Style::Text).size(Size::Xs).id_salt("files-all")).clicked() {
                            *sel = f.files.iter().map(|(i, ..)| *i).collect();
                        }
                        if ui.add(Button::new("Ничего").style(Style::Text).size(Size::Xs).id_salt("files-none")).clicked() {
                            sel.clear();
                        }
                        ui.label(TypeRole::BodySmall.style().rich(
                            format!("Выбрано {n} из {} · {}", f.files.len(), fmt_size(bytes)),
                            s.on_surface_variant,
                        ));
                    });
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("files-scroll")
                        .max_height(280.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            for (i, path, size) in &f.files {
                                let mut on = sel.contains(i);
                                let text = format!("{path} · {}", fmt_size(*size));
                                if ui
                                    .checkbox(&mut on, TypeRole::BodyMedium.style().rich(text, s.on_surface))
                                    .changed()
                                {
                                    if on {
                                        sel.insert(*i);
                                    } else {
                                        sel.remove(i);
                                    }
                                }
                            }
                        });
                    if f.result.is_none() {
                        ui.add_space(8.0);
                        ui.label(TypeRole::BodySmall.style().rich_para(
                            "Выбор файлов для .torrent и magnet пока недоступен: список только для просмотра.",
                            s.on_surface_variant,
                        ));
                    }
                }
            });
        if r.action == Some(1)
            && let Some(result) = f.result
        {
            let files: Vec<usize> = self.files_sel.iter().copied().collect();
            out.push(Command::DownloadTorrent { result, files: Some(files) });
            out.push(Command::CloseTorrentFiles);
            self.files_done = true;
        } else if r.action.is_some() || r.dismissed || !open {
            out.push(Command::CloseTorrentFiles);
            self.files_done = true;
        }
    }

    /// Draws global overlays: the playlist picker, dialogs, snackbars.
    /// `snack`: where the snackbars sit, `(area, bottom inset)`.
    pub fn overlays(&mut self, ctx: &Context, state: &ViewState, area: Rect, snack: (Rect, f32), out: &mut Vec<Command>) {
        dialogs::drain(ctx, out);
        equalizer::overlay(ctx, state, out);
        // Playlist picker.
        if let Some(tracks) = self.pending_add.clone() {
            let mut open = true;
            let mut menu = Menu::new("playlist-picker")
                .item(MenuItem::new("Любимые").icon(Icon::Favorite));
            let user: Vec<_> = state.library.playlists.iter().filter(|p| !p.pinned).collect();
            for p in &user {
                menu = menu.item(MenuItem::new(&p.name).icon(Icon::QueueMusic));
            }
            menu = menu.divider().item(MenuItem::new("Новый плейлист…").icon(Icon::Add));
            let anchor = self
                .pending_add_anchor
                .unwrap_or_else(|| Rect::from_center_size(area.center(), egui::vec2(1.0, 1.0)));
            let picked = menu.popup(ctx, anchor, &mut open);
            match picked {
                Some(0) => out.push(Command::AddToPlaylist(LIKED_PLAYLIST, tracks)),
                Some(i) if i <= user.len() => {
                    out.push(Command::AddToPlaylist(user[i - 1].id, tracks));
                }
                Some(_) => self.open_ask(Ask::NewPlaylist { then_add: tracks }),
                None => {}
            }
            if picked.is_some() || !open {
                self.pending_add = None;
                self.pending_add_anchor = None;
            }
        }

        // «Скорость» of a track: the five versions.
        if let Some(track) = self.pending_speed {
            let mut open = true;
            let anchor = self
                .pending_add_anchor
                .unwrap_or_else(|| Rect::from_center_size(area.center(), egui::vec2(1.0, 1.0)));
            let picked = speed_menu("speed-picker", &state.speeds, track).popup(ctx, anchor, &mut open);
            out.extend(picked.and_then(|i| speed_command(track, i)));
            if picked.is_some() || !open {
                self.pending_speed = None;
                self.pending_add_anchor = None;
            }
        }


        // The same picker for stream tracks or an album. «Любимые» only for
        // one stream track (a like of each of many would also download
        // each) or a library album (its tracks are here already).
        if let Some((album, anchor)) =
            ctx.data_mut(|d| d.remove_temp::<Option<(nikgob_view::RemoteId, Option<Rect>)>>(egui::Id::new("stream-album-playlist-request"))).flatten()
        {
            self.pending_add_stream = Some(StreamAdd::Album(album));
            self.pending_add_anchor = anchor;
        }
        if let Some((album, anchor)) =
            ctx.data_mut(|d| d.remove_temp::<Option<(nikgob_view::ReleaseId, Option<Rect>)>>(egui::Id::new("release-playlist-request"))).flatten()
        {
            self.pending_add_stream = Some(StreamAdd::Release(album));
            self.pending_add_anchor = anchor;
        }
        if let Some(add) = self.pending_add_stream.clone() {
            let mut open = true;
            let liked = matches!(&add, StreamAdd::Tracks(t) if t.len() == 1) || matches!(add, StreamAdd::Release(_));
            let mut menu = Menu::new("playlist-picker-stream");
            if liked {
                menu = menu.item(MenuItem::new("Любимые").icon(Icon::Favorite));
            }
            let user: Vec<_> = state.library.playlists.iter().filter(|p| !p.pinned).collect();
            for p in &user {
                menu = menu.item(MenuItem::new(&p.name).icon(Icon::QueueMusic));
            }
            menu = menu.divider().item(MenuItem::new("Новый плейлист…").icon(Icon::Add));
            let anchor = self
                .pending_add_anchor
                .unwrap_or_else(|| Rect::from_center_size(area.center(), egui::vec2(1.0, 1.0)));
            let picked = menu.popup(ctx, anchor, &mut open).map(|i| if liked { i } else { i + 1 });
            match (picked, add) {
                (Some(0), StreamAdd::Tracks(tracks)) => {
                    out.extend(tracks.into_iter().map(|track| Command::LikeStream { track, on: true }))
                }
                (Some(0), StreamAdd::Release(album)) => out.push(Command::AddAlbumToPlaylist(LIKED_PLAYLIST, album)),
                (Some(0), StreamAdd::Album(_)) => {}
                (Some(i), add) if i <= user.len() => out.push(add.into_playlist(user[i - 1].id)),
                (Some(_), add) => self.open_ask(Ask::NewPlaylistStream(add)),
                (None, _) => {}
            }
            if picked.is_some() || !open {
                self.pending_add_stream = None;
                self.pending_add_anchor = None;
            }
        }

        // Text input dialogs.
        if let Some(ask) = self.ask.clone() {
            let mut open = true;
            let dialog = match &ask {
                Ask::NewPlaylist { .. } | Ask::NewPlaylistStream(_) => TextInputDialog::new("ask-new-playlist", "Новый плейлист", "Название")
                    .icon(Icon::PlaylistAdd)
                    .buttons("Создать", "Отмена")
                    .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned())),
                Ask::RenamePlaylist(_) => TextInputDialog::new("ask-rename", "Переименовать", "Название")
                    .buttons("Сохранить", "Отмена")
                    .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned())),
                Ask::WishMagnet(_) => TextInputDialog::new("ask-wish-magnet", "Magnet-ссылка", "magnet:?xt=…")
                    .icon(Icon::Link)
                    .supporting("Раздача скачается как этот трек из «Хочу скачать».")
                    .buttons("Скачать", "Отмена")
                    .validate(|t| {
                        (!t.trim().starts_with("magnet:")).then(|| "Ссылка должна начинаться с magnet:".to_owned())
                    }),
                Ask::Magnet => TextInputDialog::new("ask-magnet", "Magnet-ссылка", "magnet:?xt=…")
                    .icon(Icon::Link)
                    .supporting("Загрузка появится во вкладке «Загрузки».")
                    .buttons("Скачать", "Отмена")
                    .validate(|t| {
                        (!t.trim().starts_with("magnet:")).then(|| "Ссылка должна начинаться с magnet:".to_owned())
                    }),
            };
            match dialog.show(ctx, &mut open, &mut self.ask_text) {
                Some(TextInputResult::Confirmed) => {
                    let text = self.ask_text.trim().to_owned();
                    match ask {
                        Ask::NewPlaylist { then_add } => {
                            out.push(Command::CreatePlaylist(text.clone()));
                            if !then_add.is_empty() {
                                self.add_after_create = Some((text, then_add));
                            }
                        }
                        Ask::RenamePlaylist(id) => out.push(Command::RenamePlaylist(id, text)),
                        Ask::Magnet => out.push(Command::AddMagnet(text)),
                        Ask::NewPlaylistStream(add) => {
                            out.push(Command::CreatePlaylist(text.clone()));
                            self.stream_after_create = Some((text, add));
                        }
                        Ask::WishMagnet(item) => out.push(Command::WishlistAttach {
                            item,
                            what: nikgob_view::WishlistAttach::Magnet(text),
                        }),
                    }
                    self.ask = None;
                }
                Some(TextInputResult::Cancelled) => self.ask = None,
                None => {
                    if !open {
                        self.ask = None;
                    }
                }
            }
        }
        // Tracks waiting for a playlist we just created.
        if let Some((name, tracks)) = self.add_after_create.clone()
            && let Some(p) = state.library.playlists.iter().find(|p| p.name == name && !p.pinned)
        {
            out.push(Command::AddToPlaylist(p.id, tracks));
            self.add_after_create = None;
        }
        if let Some((name, add)) = self.stream_after_create.clone()
            && let Some(p) = state.library.playlists.iter().find(|p| p.name == name && !p.pinned)
        {
            out.push(add.into_playlist(p.id));
            self.stream_after_create = None;
        }

        // Delete confirmation.
        if let Some(id) = self.confirm_delete {
            let mut open = true;
            // A playlist that follows a service: that one stays as it is.
            let service = state
                .playlist
                .as_ref()
                .filter(|p| p.card.id == id)
                .and_then(|p| p.sync.as_ref())
                .map(|l| l.remote.service)
                // From a card's menu: what the card knows.
                .or_else(|| state.library.playlists.iter().find(|p| p.id == id).and_then(|p| p.service))
                .map(|s| s.display_name());
            let text = match service {
                Some(s) => format!("Треки останутся в библиотеке, а плейлист в {s} — как был."),
                None => "Треки останутся в библиотеке.".to_owned(),
            };
            let r = crate::components::Dialog::new("confirm-delete", "Удалить плейлист?")
                .icon(Icon::Delete)
                .supporting(text)
                .action("Отмена")
                .action("Удалить")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::DeletePlaylist(id));
                self.pending_nav = Some(Route::Library(LibraryTab::Playlists));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_delete = None;
            }
        }

        // Album deletion asked from a card's menu.
        if let Some((id, title, forever)) =
            ctx.data_mut(|d| d.remove_temp::<(nikgob_view::ReleaseId, String, bool)>(egui::Id::new("release-delete-request")))
        {
            let what = (Doomed::Release(id), format!("Альбом «{title}»"));
            if forever {
                self.confirm_purge = Some(what);
            } else {
                self.confirm_trash = Some(what);
            }
        }
        // Deletion: into the Trash (30 days), or for good when it is off.
        if let Some((what, title)) = self.confirm_trash.clone() {
            let mut open = true;
            let trash = state.prefs.use_trash;
            let r = crate::components::Dialog::new("confirm-trash", if trash { "Удалить из библиотеки?" } else { "Удалить навсегда?" })
                .icon(Icon::Delete)
                .supporting(if trash {
                    format!("{title} переедет в Корзину на 30 дней: из библиотеки пропадёт сразу, вернуть можно из Библиотеки → Корзина.")
                } else {
                    format!("{title}: файлы будут стёрты с диска. Корзина выключена в настройках, отменить нельзя.")
                })
                .action("Отмена")
                .action(if trash { "Удалить" } else { "Удалить навсегда" })
                .show(ctx, &mut open);
            if r.action == Some(1) {
                match what {
                    Doomed::Tracks(ids) => out.extend(ids.into_iter().map(Command::TrashTrack)),
                    Doomed::Release(id) => out.push(Command::TrashRelease(id)),
                }
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_trash = None;
            }
        }
        if let Some((what, title)) = self.confirm_purge.clone() {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-purge", "Удалить навсегда?")
                .icon(Icon::Delete)
                .supporting(format!("{title}: файлы будут стёрты с диска, треки уйдут из плейлистов и Любимых. Отменить нельзя."))
                .action("Отмена")
                .action("Удалить навсегда")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(match what {
                    Doomed::Tracks(ids) => Command::DeleteForever(ids),
                    Doomed::Release(id) => Command::DeleteReleaseForever(id),
                });
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_purge = None;
            }
        }
        if self.confirm_empty_trash {
            let mut open = true;
            let n = state.library.trash.len() as u64;
            let r = crate::components::Dialog::new("confirm-empty-trash", "Очистить корзину?")
                .icon(Icon::Delete)
                .supporting(format!(
                    "{n} {} и их файлы будут стёрты навсегда. Отменить нельзя.",
                    crate::screens::common::plural(n, "трек", "трека", "треков")
                ))
                .action("Отмена")
                .action("Очистить")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::EmptyTrash);
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_empty_trash = false;
            }
        }

        // Seeding cleanup.
        if self.confirm_clean {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-clean", "Очистить раздачу?")
                .icon(Icon::Delete)
                .supporting("Завершённые торренты перестанут раздаваться, их файлы будут удалены. Музыка в библиотеке останется.")
                .action("Отмена")
                .action("Очистить")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::CleanSeeding);
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_clean = false;
            }
        }

        // Files of a torrent ("Выбрать файлы…").
        match &state.downloads.files {
            Some(f) if !self.files_done => self.files_dialog(ctx, f, out),
            Some(_) => {}
            None => self.files_done = false,
        }

        // A row's «Хочу скачать» mark was clicked.
        if ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(widgets::OPEN_WISHLIST))).unwrap_or(false) {
            self.pending_nav = Some(Route::Downloads(nikgob_view::DownloadsTab::Wishlist));
        }

        // Snackbars: «Уведомления на экране» seconds (0 = until closed).
        let duration = match state.prefs.toast_s {
            0 => f64::INFINITY,
            s => f64::from(s),
        };
        let items: Vec<SnackbarItem<'_>> = state
            .toasts
            .iter()
            .map(|t| {
                let i = SnackbarItem::new(t.id, &t.text).kind(match t.kind {
                    nikgob_view::ToastKind::Info => crate::components::SnackbarKind::Info,
                    nikgob_view::ToastKind::Success => crate::components::SnackbarKind::Success,
                    nikgob_view::ToastKind::Working => crate::components::SnackbarKind::Working,
                    nikgob_view::ToastKind::Error => crate::components::SnackbarKind::Error,
                });
                let i = if t.long { i.longer(duration) } else { i }.again(t.again);
                match (&t.action, &t.open) {
                    (Some((label, _)), _) | (None, Some((label, _))) => i.action(label),
                    (None, None) => i,
                }
            })
            .collect();
        // One at a time (M3): a new one replaces the one up, an undo stays.
        let ev = SnackbarHost::new("toasts")
            .replacing(true)
            .max_visible(1)
            .show_close(true)
            .duration(duration)
            .show(ctx, snack.0, snack.1, &items);
        if let Some(id) = ev.action_clicked
            && let Some(t) = state.toasts.iter().find(|t| t.id == id)
        {
            match (&t.action, &t.open) {
                (Some((_, cmd)), _) => out.push(cmd.clone()),
                (None, Some((_, link))) => self.follow(link.clone()),
                (None, None) => {}
            }
        }
        for id in ev.dismissed {
            out.push(Command::DismissToast(id));
        }
    }
}

/// The search bar. Returns whether the text changed (the caller sends the
/// query once typing pauses, see `Md3eUi::flush_search`).
pub fn search_bar(
    ui: &mut Ui,
    rect: Rect,
    query: &mut String,
    focus: &mut bool,
    torrents: bool,
    out: &mut Vec<Command>,
) -> bool {
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("search-bar"));
    let before = query.clone();
    let o = SearchBar::new("main-search")
        .placeholder("Поиск")
        .width(rect.width())
        .shortcut(true)
        .show(&mut cui, query);
    if std::mem::take(focus) {
        o.response.request_focus();
    }
    // Enter looks for torrents (Prowlarr is slow, so never on typing) when
    // Prowlarr is set up; the chip stays, the results land in «Торренты».
    if o.submitted && torrents && !query.trim().is_empty() {
        out.push(Command::SearchTorrents(query.trim().to_owned()));
    }
    *query != before
}

/// The page for `route`, inside a vertical scroll area that leaves room
/// for the floating player. Returns a route to navigate to.
pub fn page(
    ui: &mut Ui,
    route: &Route,
    state: &ViewState,
    bottom_inset: f32,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let mut nav = None;
    let id = format!("{route:?}");
    // Back where it was: the offset is held until the page is tall enough
    // for it (a page that loads is short at first).
    let restore = pages.scroll_restore.as_ref().filter(|r| r.route == *route).map(|r| r.y);
    // Settings keeps its title and tabs and scrolls its rows (beside the
    // stage) itself.
    if *route == Route::Settings {
        let o = settings::page(ui, state, bottom_inset, restore, pages, out);
        settle_scroll(ui, pages, restore, o.offset, o.max);
        return None;
    }
    let mut area = egui::ScrollArea::vertical().id_salt(("page-scroll", id)).auto_shrink([false, false]);
    if let Some(y) = restore {
        area = area.vertical_scroll_offset(y);
    }
    let o = area
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let r = match route {
                Route::Home => home::show(ui, state, pages, out),
                Route::Library(_) => library::show(ui, state, pages, out),
                Route::Release(id) => detail::release(ui, *id, state, pages, out),
                Route::Artist(id) => detail::artist(ui, *id, state, pages, out),
                Route::Playlist(id) => detail::playlist(ui, *id, state, pages, out),
                Route::Downloads(_) => downloads::show(ui, state, pages, out),
                Route::Settings => None,
                Route::Lyrics => crate::screens::lyrics_ui::page::show(ui, state, bottom_inset, out),
                Route::Stream(page) => streams::page(ui, page, state, pages, out),
                Route::World(key) => world::show(ui, key, state, pages, out),
                Route::Mix { key, world } => detail::mix(ui, *key, world.as_deref(), state, pages, out),
                Route::NewReleases => home::new_releases(ui, state, pages, out),
            };
            nav = r.or(nav.take());
            ui.add_space(bottom_inset);
        });
    settle_scroll(ui, pages, restore, o.state.offset.y, (o.content_size.y - o.inner_rect.height()).max(0.0));
    nav
}

/// Keeps how far the page is scrolled (`offset`, of `max`), and lets go of
/// the history's offset once the page is tall enough for it (or its time
/// is up).
fn settle_scroll(ui: &Ui, pages: &mut PageState, restore: Option<f32>, offset: f32, max: f32) {
    pages.scroll_now = offset;
    if let (Some(y), Some(r)) = (restore, pages.scroll_restore.as_mut()) {
        let reachable = max + 0.5 >= y;
        r.frames = r.frames.saturating_sub(1);
        if reachable || r.frames == 0 {
            pages.scroll_restore = None;
        } else {
            ui.ctx().request_repaint();
        }
    }
}

/// Human size: "412 МБ", "1.1 ГБ".
pub(super) fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["Б", "КБ", "МБ", "ГБ", "ТБ"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    match i {
        0 => format!("{bytes} Б"),
        _ if v >= 100.0 => format!("{v:.0} {}", UNITS[i]),
        _ => format!("{v:.1} {}", UNITS[i]),
    }
}

/// A connected button group (single select) that can disable members and
/// reports every member's rect (for badges). Returns the newly picked index.
pub(super) fn connected(
    ui: &mut Ui,
    id: egui::Id,
    labels: &[&str],
    selected: usize,
    disabled: &[usize],
) -> (Option<usize>, Vec<Rect>) {
    connected_shown(ui, id, labels, selected, disabled, selected)
}

/// [`connected`] painting `shown` as the selected member (a hover preview)
/// while a click still commits against `selected`.
pub(super) fn connected_shown(
    ui: &mut Ui,
    id: egui::Id,
    labels: &[&str],
    selected: usize,
    disabled: &[usize],
    shown: usize,
) -> (Option<usize>, Vec<Rect>) {
    let n = labels.len();
    let inner = Size::S.connected_inner();
    let mut picked = None;
    let mut rects = Vec::with_capacity(n);
    // One button tall, so the group sits on its parent's centre line.
    let row = egui::vec2(ui.available_width(), Size::S.height());
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (i, label) in labels.iter().enumerate() {
            let sel = i == shown;
            let off = disabled.contains(&i);
            let l = if i == 0 || sel { shape::FULL } else { inner };
            let r = if i + 1 == n || sel { shape::FULL } else { inner };
            let b = Button::new(*label)
                .size(Size::S)
                .style(Style::Tonal)
                .selected(sel)
                .enabled(!off)
                .radii(
                    Radii::sides(l, r),
                    Radii::sides(
                        if i == 0 { shape::FULL } else { inner * 0.5 },
                        if i + 1 == n { shape::FULL } else { inner * 0.5 },
                    ),
                )
                .id_salt((id, i));
            let resp = ui.add(b);
            rects.push(resp.rect);
            if resp.clicked() && i != selected && !off {
                picked = Some(i);
            }
        }
    });
    (picked, rects)
}
