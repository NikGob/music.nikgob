//! Content pages and the menus and dialogs they open.

mod add_device;
pub mod brand;
pub(super) mod detail;
mod dialogs;
mod downloads;
mod equalizer;
mod cards;
pub(crate) mod first_run;
pub(crate) mod home;
mod library;
mod logos;
mod marks;
pub(crate) mod mesh_ui;
mod network;
mod overlays;
mod search;
mod shelf;
mod settings;
pub(crate) mod sound;
pub(crate) mod settings_help;
mod sources;
mod streams;
mod track_menu;
mod wishlist;
mod world;
mod worlds;
pub mod widgets;

#[cfg(test)]
mod speed_tests;

pub use search::{restore_search_scroll, search_results, search_scroll};
pub use settings_help::SettingId;
pub use track_menu::{TrackAct, TrackMenuOf, track_rows};
pub(crate) use track_menu::{
    LibraryRowsOf, can_trash, download_item, is_playing, library_item, library_track_rows, pick_release_command, placed,
    radio_by_track, sound_at, with_speed_row,
};
#[cfg(test)]
use track_menu::has_info;
pub(crate) use worlds::choose as choose_world;
pub(crate) use worlds::{hide_stage as hide_world_stage, play_primary as play_world};

use egui::{Context, Rect, Response, Ui, UiBuilder, vec2};
use nikgob_core::i18n;
use nikgob_view::{Command, LibraryTab, ReleaseId, Route, TrackId, TrackMenuSlot, TrackRow, ViewState};
use track_menu::push_deletes;

use super::PaneTab;
use crate::{
    Icon, Theme,
    components::{Button, FabMenu, FabMenuItem, Menu, MenuItem, Radii, SearchBar, Size, Style},
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

// For egui's temp data (taken out with `remove_temp`).
impl Default for Doomed {
    fn default() -> Self {
        Self::Tracks(Vec::new())
    }
}

fn delete_request_key() -> egui::Id {
    egui::Id::new("delete-request")
}

/// Asks [`PageState::overlays`] to confirm deleting tracks or an album
/// (`title` names it in the dialog, «…» quoted); set from menus that have
/// no `PageState` at hand (cards, the player's).
pub(crate) fn request_delete(ctx: &Context, what: Doomed, title: String, forever: bool) {
    ctx.data_mut(|d| d.insert_temp(delete_request_key(), (what, title, forever)));
}

/// [`request_delete`] of an album. `(album, title, for good)`.
pub(crate) fn request_release_delete(ctx: &Context, id: nikgob_view::ReleaseId, title: &str, forever: bool) {
    request_delete(ctx, Doomed::Release(id), i18n::menus::fmt_album_quoted(title), forever);
}

fn apart_request_key() -> egui::Id {
    egui::Id::new("apart-request")
}

/// Asks [`PageState::overlays`] to confirm «Разные песни» of `track`
/// (`title`) and the same song on the album `album`; it cannot be undone.
pub(crate) fn request_apart(ctx: &Context, track: TrackId, title: &str, album: &str) {
    ctx.data_mut(|d| d.insert_temp(apart_request_key(), (track, title.to_owned(), album.to_owned())));
}

fn picker_request_key() -> egui::Id {
    egui::Id::new("tracks-playlist-request")
}

/// Opens the «В плейлист…» picker for library tracks at `anchor`; set from
/// menus that have no `PageState` at hand (the player's, the queue's).
pub(crate) fn request_tracks_to_playlist(ctx: &Context, tracks: Vec<TrackId>, anchor: Option<Rect>) {
    ctx.data_mut(|d| d.insert_temp(picker_request_key(), (tracks, anchor)));
}

/// The like of a track or an album as a menu item or a tooltip says it.
pub(crate) fn like_label(liked: bool) -> &'static str {
    if liked { i18n::menus().unlike } else { i18n::menus().like }
}

/// «Не нравится» of a track, or its undo when it is disliked.
pub(crate) fn dislike_item(disliked: bool) -> MenuItem {
    MenuItem::new(if disliked { i18n::menus().undislike } else { i18n::menus().dislike }).icon(Icon::ThumbDown).selected(disliked)
}

pub(crate) fn dislike_command(track: nikgob_view::TrackId, marks: &nikgob_view::DislikesView) -> Command {
    Command::Dislike { track, dislike: !marks.track(track), sure: false, relike: false }
}

/// «Не рекомендовать исполнителя» of `artist`, or its undo.
pub(crate) fn ban_item(artist: nikgob_view::ArtistId, marks: &nikgob_view::DislikesView) -> MenuItem {
    let banned = marks.artist(artist);
    MenuItem::new(if banned { i18n::menus().unban_artist } else { i18n::menus().ban_artist }).icon(Icon::Block)
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

fn track_menu_key() -> egui::Id {
    egui::Id::new("md3e-track-menu-slots")
}

/// Keeps «Меню трека» (`Prefs::track_menu`) for the menus drawn without
/// the view state.
pub(crate) fn set_track_menu(ctx: &Context, slots: &[TrackMenuSlot]) {
    let same = ctx.data(|d| d.get_temp::<std::sync::Arc<Vec<TrackMenuSlot>>>(track_menu_key())).is_some_and(|s| s.as_slice() == slots);
    if !same {
        ctx.data_mut(|d| d.insert_temp(track_menu_key(), std::sync::Arc::new(slots.to_vec())));
    }
}

/// What [`set_track_menu`] kept.
pub(crate) fn track_menu_slots(ctx: &Context) -> std::sync::Arc<Vec<TrackMenuSlot>> {
    ctx.data(|d| d.get_temp(track_menu_key())).unwrap_or_default()
}

fn can_hide_key() -> egui::Id {
    egui::Id::new("md3e-window-can-hide")
}

/// Whether the window can hide to the tray and come back (not on Wayland):
/// «Быстрое открытие из трея» does something only then.
pub(crate) fn set_window_can_hide(ctx: &Context, can: bool) {
    if window_can_hide(ctx) != can {
        ctx.data_mut(|d| d.insert_temp(can_hide_key(), can));
    }
}

/// What [`set_window_can_hide`] kept (true until told otherwise).
pub(crate) fn window_can_hide(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp(can_hide_key())).unwrap_or(true)
}

/// The page on screen, as the track menus see it: an item that would only
/// lead back to it is left out (no «К альбому» of the album whose page it
/// is, no «К исполнителю» of the artist whose page it is).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Here {
    pub album: Option<ReleaseId>,
    pub artist: Option<nikgob_view::ArtistId>,
    pub stream_album: Option<nikgob_view::RemoteId>,
    pub stream_artist: Option<nikgob_view::RemoteId>,
}

impl Here {
    /// The page of `route`.
    pub(crate) fn of(route: &Route) -> Self {
        use nikgob_view::StreamPage;
        match route {
            Route::Release(id) => Self { album: Some(*id), ..Self::default() },
            Route::Artist(id) => Self { artist: Some(*id), ..Self::default() },
            Route::Stream(StreamPage::Album(id)) => Self { stream_album: Some(id.clone()), ..Self::default() },
            Route::Stream(StreamPage::Artist(id)) => Self { stream_artist: Some(id.clone()), ..Self::default() },
            _ => Self::default(),
        }
    }

    /// The artists «К исполнителю» of a library track goes to from here:
    /// its own, less the one whose page this is.
    pub(crate) fn artists(&self, links: &nikgob_view::ArtistLinks) -> crate::screens::artist_links::Choices {
        let mut choices = crate::screens::artist_links::library(links);
        if let Some(a) = self.artist {
            choices.retain(|(_, r)| *r != Route::Artist(a));
        }
        choices
    }

    /// [`Self::artists`] of a service's song.
    pub(crate) fn stream_artists(&self, links: &nikgob_view::StreamArtistLinks) -> crate::screens::artist_links::Choices {
        let mut choices = crate::screens::artist_links::stream(links);
        if let Some(a) = &self.stream_artist {
            choices.retain(|(_, r)| *r != Route::Stream(nikgob_view::StreamPage::Artist(a.clone())));
        }
        choices
    }

    /// Whether `act` of a library track's menu leads anywhere from here;
    /// `also`: the other album of the song («Есть и на альбоме»).
    pub(crate) fn keeps(&self, act: TrackAct, track: &TrackRow, also: Option<ReleaseId>) -> bool {
        match act {
            TrackAct::Album => self.album.is_none() || track.release_id != self.album,
            TrackAct::AlsoOn => self.album.is_none() || also != self.album,
            TrackAct::Artist => !self.artists(&track.artists).is_empty(),
            _ => true,
        }
    }

    /// The same of a service's song.
    pub(crate) fn keeps_stream(&self, act: TrackAct, t: &nikgob_view::StreamTrackRow) -> bool {
        match act {
            TrackAct::Album => self.stream_album.is_none() || t.album_id != self.stream_album,
            TrackAct::Artist => !self.stream_artists(&t.artists).is_empty(),
            _ => true,
        }
    }

    /// `rows` of a library track's menu without what leads back here;
    /// `act`: the track's action a row stands for, if any.
    pub(crate) fn trim<A: Copy>(
        &self,
        rows: Vec<widgets::Row<A>>,
        track: &TrackRow,
        also: Option<ReleaseId>,
        act: impl Fn(A) -> Option<TrackAct>,
    ) -> Vec<widgets::Row<A>> {
        if *self == Self::default() {
            return rows;
        }
        retain_rows(rows, &|a: A| act(a).is_none_or(|t| self.keeps(t, track, also)))
    }
}

/// `rows` with the items `keep` says no to left out (in a submenu too).
pub(crate) fn retain_rows<A: Copy>(rows: Vec<widgets::Row<A>>, keep: &dyn Fn(A) -> bool) -> Vec<widgets::Row<A>> {
    use widgets::Row;
    rows.into_iter()
        .filter_map(|r| match r {
            Row::Item(a, _) if !keep(a) => None,
            Row::Sub(id, m, inner) => Some(Row::Sub(id, m, retain_rows(inner, keep))),
            r => Some(r),
        })
        .collect()
}

fn here_key() -> egui::Id {
    egui::Id::new("md3e-here")
}

/// Keeps the page drawn this frame for the track menus ([`here`]).
pub(crate) fn set_here(ctx: &Context, here: Here) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(here_key(), (pass, here)));
}

/// The page on screen: drawn this frame or the one before (the bar and the
/// pane are drawn around it); none while no page is (Now Playing covers
/// them).
pub(crate) fn here(ctx: &Context) -> Here {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, Here)>(here_key()))
        .filter(|(at, _)| at + 1 >= pass)
        .map(|(_, h)| h)
        .unwrap_or_default()
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
    /// Settings tab: [`settings_help::TABS`] (0 «Музыка» … 6 «Система»).
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
    /// The library filter's text asked to be looked for everywhere
    /// («Искать везде»): the search bar takes it over.
    pub(crate) search_everywhere: Option<String>,
    release_filter: usize,
    /// Library albums by source: 0 all, 1 with files, 2 stream.
    release_source: usize,
    downloads_tab: usize,
    ask: Option<Ask>,
    ask_text: String,
    confirm_delete: Option<nikgob_view::PlaylistId>,
    /// «Связать с сервисом…» of this playlist, waiting for its yes.
    confirm_link: Option<nikgob_view::PlaylistId>,
    /// Tracks or an album waiting for the "Удалить" confirmation (into the
    /// Trash, or for good when the Trash is off).
    confirm_trash: Option<(Doomed, String)>,
    /// ... and for "Удалить навсегда".
    confirm_purge: Option<(Doomed, String)>,
    /// «Разные песни»: (track, its title, the other album).
    confirm_apart: Option<(TrackId, String, String)>,
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
    /// «Межсеть»: the token dialog, what is typed into «Порт», the tokens'
    /// last uses seen (their rows pulse on a new one).
    net: settings::NetState,
    /// The owner's devices: the dialogs and the window they share, «Карта
    /// сети».
    pub(crate) mesh: mesh_ui::MeshUi,
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
    /// (playlist name, tracks) to add once the new playlist appears.
    add_after_create: Option<(String, Vec<TrackId>)>,
    /// Stream tracks waiting for the "В плейлист" picker, and for a new
    /// playlist to appear.
    pending_add_stream: Option<StreamAdd>,
    stream_after_create: Option<(String, StreamAdd)>,
    /// The first run, while it is on screen.
    pub(crate) first_run: first_run::State,
}

/// Opens «Добавить устройство» on «Подключить», or on
/// «Установить» (scripted screenshots; Settings → Межсеть and «Карта
/// сети» draw it).
pub(crate) fn open_add_device(ctx: &Context, state: &ViewState, pages: &mut PageState, install: bool, out: &mut Vec<Command>) {
    let tab = if install { add_device::AddTab::Install } else { add_device::AddTab::Connect };
    add_device::open(ctx, &state.mesh, &mut pages.mesh, None, tab, out);
}

/// Opens «Добавить устройство» out of `from` (the output popover's
/// «Добавить»), on «Подключить».
pub(crate) fn open_add_device_from(ctx: &Context, state: &ViewState, pages: &mut PageState, from: Rect, out: &mut Vec<Command>) {
    add_device::open(ctx, &state.mesh, &mut pages.mesh, Some(from), add_device::AddTab::Connect, out);
}

/// Opens the code window of the first nearby device (scripted
/// screenshots).
pub(crate) fn open_pair_code(ctx: &Context, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    if let Some(n) = state.mesh.nearby.first() {
        add_device::open_pair(ctx, &state.mesh, &mut pages.mesh, None, &n.id, out);
    }
}

/// The devices' window and dialogs when no page of the devices drew them
/// this frame (opened from the player's output popover over another page,
/// or with Now Playing over the pages).
pub(crate) fn mesh_overlays_elsewhere(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    if !mesh_ui::overlays_drawn(ui.ctx()) {
        mesh_ui::overlays(ui, state, &mut pages.mesh, out);
    }
}

/// The library as a list (true) or a grid.
pub(crate) fn set_library_list(ctx: &Context, list: bool) {
    library::set_list_view(ctx, list);
}

impl PageState {
    pub(crate) fn set_library_filter(&mut self, text: &str) {
        self.library_filter = text.to_owned();
    }

    /// On Библиотека (but its «Скачано» and «Корзина», which have no
    /// filter) the search bar is the library's filter.
    pub(crate) fn bar_filters_library(&self, route: &Route) -> bool {
        matches!(route, Route::Library(_)) && !matches!(self.library_tab, LibraryTab::Downloaded | LibraryTab::Trash)
    }

    /// Esc in the bar while it filters the library: empties the filter;
    /// false when there was nothing to empty.
    pub(crate) fn clear_library_filter(&mut self) -> bool {
        let had = !self.library_filter.is_empty();
        self.library_filter.clear();
        had
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

    /// The "Добавить" FAB menu: the rail's FAB turns into a close button
    /// with pills below it. The rail already drew a plain FAB at `fab`; the
    /// menu takes its place so it can morph.
    pub fn add_menu(&mut self, ui: &mut Ui, fab: &Response, out: &mut Vec<Command>) {
        let s = Theme::get(ui.ctx()).scheme;
        ui.painter()
            .rect_filled(fab.rect.expand2(vec2(20.0, 20.0)), 0, s.surface);
        let extended = fab.rect.width() > 76.0;
        let items = [
            FabMenuItem::new(Icon::UploadFile, i18n::pages().fab_files),
            FabMenuItem::new(Icon::Folder, i18n::pages().fab_folder),
            FabMenuItem::new(Icon::AttachFile, i18n::pages().fab_torrent),
            FabMenuItem::new(Icon::Link, i18n::pages().fab_magnet),
            FabMenuItem::new(Icon::PlaylistAdd, i18n::pages().fab_new_playlist),
        ];
        let mut menu = FabMenu::new("add-fab-menu", Icon::Add, &items);
        if extended {
            menu = menu.label(i18n::common().add);
        }
        let slot = Rect::from_min_size(fab.rect.min, vec2(260.0, 56.0));
        let mut cui = ui.new_child(UiBuilder::new().max_rect(slot).id_salt("add-fab-slot"));
        let picked = menu.show(&mut cui).clicked;
        match picked {
            Some(0) => dialogs::files(
                |d| {
                    d.set_title(i18n::pages().files_dialog_title)
                        .add_filter(i18n::pages().files_filter, &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "alac", "aiff"])
                },
                Command::ImportFiles,
                out,
            ),
            Some(1) => dialogs::folder(|d| d.set_title(i18n::pages().add_folder), Command::ImportFolder, out),
            Some(2) => dialogs::file(
                |d| d.set_title(i18n::pages().open_torrent_title).add_filter(i18n::pages().torrent_filter, &["torrent"]),
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
        state: &ViewState,
        wish: &widgets::WishIndex,
        here: &Here,
    ) -> (Menu, Vec<TrackAct>) {
        let rows = library_track_rows(track, LibraryRowsOf::of(state, track, wish));
        let mut rows = with_speed_row(rows, &state.speeds, is_playing(state, track.id));
        push_deletes(&mut rows, track);
        let also = state.also_on.get(&track.id).map(|(r, _)| *r);
        let rows = here.trim(rows, track, also, Some);
        let rows = placed(rows, &state.prefs.track_menu, |a| a.menu_item(track.has_local));
        let (menu, acts) = widgets::menu_of(("track-menu", anchor), rows);
        (menu.align_right(true), acts)
    }

    /// Does a library track's action (all but a playlist's `Remove`).
    pub(crate) fn track_apply(
        &mut self,
        ctx: &Context,
        act: TrackAct,
        track: &TrackRow,
        state: &ViewState,
        anchor: Option<Rect>,
        out: &mut Vec<Command>,
    ) -> Option<Route> {
        use TrackAct as A;
        let marks = &state.dislikes;
        let also = state.also_on.get(&track.id);
        match act {
            A::Next => out.push(Command::PlayNext(vec![track.id])),
            A::Queue => out.push(Command::Enqueue(vec![track.id])),
            A::Playlist => {
                self.pending_add = Some(vec![track.id]);
                self.pending_add_anchor = anchor;
            }
            A::ToLibrary => out.push(Command::TrackInLibrary { track: track.id, on: true }),
            A::OutOfLibrary => out.push(Command::TrackInLibrary { track: track.id, on: false }),
            A::Radio => out.push(radio_by_track(track.id)),
            A::Album => return track.release_id.map(Route::Release),
            A::AlsoOn => return also.map(|(release, _)| Route::Release(*release)),
            // For good: asked first.
            A::Apart => {
                if let Some((_, album)) = also {
                    request_apart(ctx, track.id, &track.title, album);
                }
            }
            // Several artists: a chooser (not of the artist whose page
            // this is).
            A::Artist => return crate::screens::artist_links::go_to(ctx, here(ctx).artists(&track.artists)),
            // The playing track's in the side pane when it is on; any
            // other's in a window of its own.
            A::Info if is_playing(state, track.id) && state.prefs.pane_info => self.open_pane = Some(PaneTab::Info),
            A::Info => crate::screens::info_window::open(ctx, track.id, Some(track.clone()), out),
            A::Share => out.push(Command::Share(nikgob_view::ShareTarget::Track(track.id))),
            // Of this track, which need not be the one playing.
            A::Speed => sound::open(ctx, sound::Tab::Speed, Some((track.id, track.title.clone()))),
            A::Equalizer => sound::open(ctx, sound::Tab::Equalizer, None),
            A::Fetch if track.has_local => out.push(Command::OpenTagEditor(track.id)),
            A::Fetch => out.push(Command::DownloadTracks(vec![track.id])),
            // The whole album the track is from, the files it lacks.
            A::AlbumDownload => {
                if let Some(r) = track.release_id {
                    out.push(Command::DownloadRelease(r));
                }
            }
            // A better copy through the download sources, as the wishlist
            // looks.
            A::Better => out.push(Command::FindBetter(track.id)),
            A::PickRelease => out.push(pick_release_command(track)),
            A::ToMp3 => out.push(Command::FakeToMp3(track.id)),
            A::FindSong => out.push(Command::FindSong { track: track.id, query: None }),
            A::WrongFile => out.push(Command::WrongFile(track.id)),
            A::RefreshCover => out.extend(track.release_id.map(Command::RefreshReleaseCover)),
            A::Dislike => out.push(dislike_command(track.id, marks)),
            A::Ban => out.extend(track.artist_id().map(|a| ban_command(a, marks))),
            A::Trash => self.confirm_trash = Some((Doomed::Tracks(vec![track.id]), i18n::menus::fmt_quoted(&track.title))),
            A::Purge => self.confirm_purge = Some((Doomed::Tracks(vec![track.id]), i18n::menus::fmt_quoted(&track.title))),
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
        let wish = widgets::WishIndex::of(&anchor.ctx, &state.downloads.wishlist);
        let (menu, acts) = Self::track_menu_items(anchor.id, track, state, &wish, &here(&anchor.ctx));
        let act = menu.attach(anchor).and_then(|i| acts.get(i).copied())?;
        self.track_apply(&anchor.ctx, act, track, state, Some(anchor.rect), out)
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
        let wish = widgets::WishIndex::of(&row.ctx, &state.downloads.wishlist);
        let (menu, acts) = Self::track_menu_items(row.id.with("ctx"), track, state, &wish, &here(&row.ctx));
        let act = menu.context_menu(row).and_then(|i| acts.get(i).copied())?;
        self.track_apply(&row.ctx, act, track, state, None, out)
    }

    /// "В плейлист" picker anchored to a button.
    pub fn playlist_menu(&mut self, anchor: &Response, tracks: Vec<TrackId>) {
        if anchor.clicked() {
            self.pending_add = Some(tracks);
            self.pending_add_anchor = Some(anchor.rect);
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
) -> SearchBarOutput {
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("search-bar"));
    let before = query.clone();
    let o = SearchBar::new("main-search")
        .placeholder(i18n::common().search)
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
    SearchBarOutput { edited: *query != before, submitted: o.submitted }
}

/// The search bar on Библиотека: the library's instant filter of the tab
/// shown (placeholder «Найти в библиотеке», …), live as it is typed; Enter
/// looks for the text everywhere (`pages.search_everywhere`).
pub fn library_bar(ui: &mut Ui, rect: Rect, pages: &mut PageState, focus: &mut bool) {
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt("search-bar"));
    let o = SearchBar::new("main-search")
        .placeholder(library::filter_hint(pages.library_tab))
        .width(rect.width())
        .shortcut(true)
        .show(&mut cui, &mut pages.library_filter);
    if std::mem::take(focus) {
        o.response.request_focus();
    }
    let q = pages.library_filter.trim();
    if o.submitted && !q.is_empty() {
        pages.search_everywhere = Some(q.to_owned());
    }
}

/// What the search bar did this frame: the text changed (sent once typing
/// pauses), Enter (sent at once).
pub struct SearchBarOutput {
    pub edited: bool,
    pub submitted: bool,
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
    // The track menus of the page leave out what leads back to it.
    set_here(ui.ctx(), Here::of(route));
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
    // «Карта сети» is a field of its own, not a page that scrolls.
    if *route == Route::Devices {
        pages.scroll_now = 0.0;
        pages.scroll_restore = None;
        return network::show(ui, state, bottom_inset, pages, out);
    }
    let mut area = egui::ScrollArea::vertical().id_salt(("page-scroll", id)).auto_shrink([false, false]);
    if let Some(y) = restore {
        area = area.vertical_scroll_offset(y);
    }
    // The bar's gutter is kept while nothing scrolls too, so what sits at
    // the right edge does not move when the page grows long enough to scroll.
    let full = ui.available_width();
    let o = area
        .show(ui, |ui| {
            let room = full - crate::components::scrollbars::GUTTER;
            if ui.available_width() > room {
                ui.set_max_width(room);
            }
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
                Route::Marks => marks::show(ui, state, pages, out),
                Route::Devices => None,
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
    i18n::pages::fmt_size(bytes)
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

