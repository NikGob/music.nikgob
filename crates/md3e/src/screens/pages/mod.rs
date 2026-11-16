//! Content pages and the menus and dialogs they open.

mod add_device;
mod brand;
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
mod search;
mod shelf;
mod settings;
pub(crate) mod sound;
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
    Command, LIKED_PLAYLIST, LibraryTab, ReleaseId, Route, TorrentFiles, TrackId, TrackMenuItem, TrackMenuPlace, TrackMenuSlot,
    TrackRow, ViewState,
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
    request_delete(ctx, Doomed::Release(id), format!("Альбом «{title}»"), forever);
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

/// «Радио по треку» in the track menus.
pub(crate) const RADIO_BY_TRACK: &str = "Радио по треку";
/// Right after the track playing.
pub(crate) const PLAY_NEXT: &str = "Сыграть следующим";
/// To the end of the queue.
pub(crate) const TO_QUEUE: &str = "В очередь";
/// A song outside the library into it (no like, nothing downloads).
pub(crate) const TO_LIBRARY: &str = "В библиотеку";
/// A song in the library without a file or a like out of it.
pub(crate) const OUT_OF_LIBRARY: &str = "Убрать из библиотеки";
/// The like of a track or an album, and its undo.
pub(crate) const LIKE: &str = "Нравится";
pub(crate) const UNLIKE: &str = "Убрать из любимого";
/// The menu item of a track whose file holds another song.
pub(crate) const WRONG_FILE: &str = "В файле другая песня";
/// «Файл ▸» of a track whose lossless file surely holds an MP3.
pub(crate) const TO_MP3: &str = "Превратить в MP3";
/// A better copy of a file, through the download sources.
pub(crate) const FIND_BETTER: &str = "Найти качество лучше";
/// The indexer's search for a release of a track or an album, by hand.
pub(crate) const PICK_RELEASE: &str = "Выбрать раздачу…";
/// A song of two albums is two songs: never linked again.
pub(crate) const APART: &str = "Разные песни";
/// The submenu of a track's menu ([`track_rows`]).
pub(crate) const FILE: &str = "Файл";

/// The like of a track or an album as a menu item or a tooltip says it.
pub(crate) fn like_label(liked: bool) -> &'static str {
    if liked { UNLIKE } else { LIKE }
}

/// «Не нравится» of a track, or its undo when it is disliked.
pub(crate) fn dislike_item(disliked: bool) -> MenuItem {
    MenuItem::new(if disliked { "Убрать «Не нравится»" } else { "Не нравится" }).icon(Icon::ThumbDown).selected(disliked)
}

pub(crate) fn dislike_command(track: nikgob_view::TrackId, marks: &nikgob_view::DislikesView) -> Command {
    Command::Dislike { track, dislike: !marks.track(track), sure: false, relike: false }
}

/// «Не рекомендовать исполнителя» of `artist`, or its undo.
pub(crate) fn ban_item(artist: nikgob_view::ArtistId, marks: &nikgob_view::DislikesView) -> MenuItem {
    let banned = marks.artist(artist);
    MenuItem::new(if banned { "Снова рекомендовать исполнителя" } else { "Не рекомендовать исполнителя" }).icon(Icon::Block)
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
            Row::Sub(m, inner) => Some(Row::Sub(m, retain_rows(inner, keep))),
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

/// What a track menu does. Every track menu (a library row, a playlist
/// row, a service's song, the queue, the player) draws its rows from
/// [`track_rows`] in one order; the caller adds the rows of the place the
/// track is in (`Remove`, the deletes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackAct {
    Next,
    Queue,
    Playlist,
    /// «В библиотеку» of a song outside the library: no like, nothing
    /// downloads.
    ToLibrary,
    /// «Убрать из библиотеки» of a song without a file, not liked.
    OutOfLibrary,
    Radio,
    Album,
    AlsoOn,
    Apart,
    Artist,
    Info,
    Share,
    /// «Скорость…»: the sound sheet on the track's speed.
    Speed,
    /// «Эквалайзер…» of the playing track's menu: the sound sheet on the
    /// equalizer.
    Equalizer,
    /// A file: its tags; no file: download it (no like).
    Fetch,
    AlbumDownload,
    /// A file: a better copy through the download sources.
    Better,
    /// No file: the indexer's search page, by hand.
    PickRelease,
    /// The lossless file surely holds an MP3: that MP3 takes its place.
    ToMp3,
    FindSong,
    WrongFile,
    /// «Обновить обложку» of the album of a song with a file.
    RefreshCover,
    Dislike,
    Ban,
    Remove,
    Trash,
    Purge,
}

impl TrackAct {
    /// The item of «Меню трека» the action is, if the settings place it.
    /// `tags`: the track has a file, so [`Self::Fetch`] edits its tags
    /// (else it downloads the track).
    pub(crate) fn menu_item(self, tags: bool) -> Option<TrackMenuItem> {
        use TrackMenuItem as I;
        Some(match self {
            Self::Radio => I::Radio,
            Self::Album => I::Album,
            Self::Artist => I::Artist,
            Self::Share => I::Share,
            Self::Info => I::Info,
            Self::Speed | Self::Equalizer => I::Sound,
            Self::Fetch if tags => I::Tags,
            Self::Fetch => I::Download,
            Self::Better => I::Torrents,
            Self::PickRelease => I::PickRelease,
            Self::WrongFile => I::WrongFile,
            Self::Dislike => I::Dislike,
            Self::Ban => I::Ban,
            Self::Next | Self::Queue | Self::Playlist | Self::ToLibrary | Self::OutOfLibrary | Self::AlsoOn | Self::Apart | Self::FindSong | Self::ToMp3
            | Self::AlbumDownload
            | Self::RefreshCover
            | Self::Remove | Self::Trash | Self::Purge => return None,
        })
    }
}

/// What a track menu is built over. An item that cannot work for this
/// track is left out, never greyed.
pub struct TrackMenuOf {
    pub playable: bool,
    pub has_album: bool,
    pub has_artists: bool,
    /// It has a file: «Файл» holds the file's actions, else its download.
    pub has_file: bool,
    /// The other album of the same song («Есть и на альбоме «…»»), and
    /// with it «Разные песни».
    pub also_on: Option<String>,
    /// «Источник и качество»: what the track's info shows (its file, or
    /// where it streams from).
    pub info: bool,
    /// A file: «Редактировать теги…»; no file: «Скачать» (none while it is
    /// on its way).
    pub fetch: Option<MenuItem>,
    /// «Найти качество лучше» of a file.
    pub better: bool,
    /// «Выбрать раздачу…» of a track with no file (a torrent source is set
    /// up).
    pub pick_release: bool,
    pub find_song: Option<&'static str>,
    pub wrong_file: bool,
    pub dislike: MenuItem,
    /// «Не рекомендовать исполнителя», for a track whose artist is known.
    pub ban: Option<MenuItem>,
    /// «В библиотеку» (`Some(true)`: a song outside it) or «Убрать из
    /// библиотеки» (`Some(false)`: in it without a file or a like); none
    /// where it gives nothing.
    pub library: Option<bool>,
}

/// The one list of a track's actions, in groups: play next / the queue /
/// radio │ to a playlist │ album (and the other album of the song) /
/// artist / share │ «Скачать» of a track with no file, the sound (the
/// speed, and what the player adds: see [`sound_at`]), «Файл ▸» (the
/// info, the file's actions or the other ways to get it, and the deletes
/// of the place) │ «Не нравится» / «Не рекомендовать
/// исполнителя» │ «Разные песни», after which the caller puts what takes
/// the track out of its list. A submenu is shown as [`widgets::shown_rows`]
/// has it: not at all with no item, its one item in its place.
pub fn track_rows(o: TrackMenuOf) -> Vec<widgets::Row<TrackAct>> {
    use TrackAct as A;
    use widgets::Row;
    let mut rows = Vec::new();
    if o.playable {
        rows.push(Row::Item(A::Next, MenuItem::new(PLAY_NEXT).icon(Icon::QueueMusic)));
        rows.push(Row::Item(A::Queue, MenuItem::new(TO_QUEUE).icon(Icon::QueueMusic)));
    }
    rows.push(Row::Item(A::Radio, MenuItem::new(RADIO_BY_TRACK).icon(Icon::Radio)));
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Playlist, MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd)));
    match o.library {
        Some(true) => rows.push(Row::Item(A::ToLibrary, MenuItem::new(TO_LIBRARY).icon(Icon::LibraryMusic))),
        Some(false) => rows.push(Row::Item(A::OutOfLibrary, MenuItem::new(OUT_OF_LIBRARY).icon(Icon::LibraryMusic))),
        None => {}
    }
    rows.push(Row::Divider);
    if o.has_album {
        rows.push(Row::Item(A::Album, MenuItem::new("К альбому").icon(Icon::Album)));
    }
    // One song on two albums: the other album (the way out of it is last).
    let apart = o.also_on.is_some();
    if let Some(album) = o.also_on {
        rows.push(Row::Item(A::AlsoOn, MenuItem::new(format!("Есть и на альбоме «{album}»")).icon(Icon::Album)));
    }
    if o.has_artists {
        rows.push(Row::Item(A::Artist, MenuItem::new("К исполнителю").icon(Icon::Person)));
    }
    rows.push(Row::Item(A::Share, MenuItem::new("Поделиться").icon(Icon::Share)));
    rows.push(Row::Divider);
    let mut file = Vec::new();
    if o.info {
        file.push(Row::Item(A::Info, MenuItem::new("Источник и качество").icon(Icon::Info)));
    }
    // «Скачать» stands in the menu itself; a file's «Редактировать теги…»
    // in «Файл».
    match o.fetch {
        Some(f) if o.has_file => file.push(Row::Item(A::Fetch, f)),
        Some(f) => rows.push(Row::Item(A::Fetch, f)),
        None => {}
    }
    if o.has_file {
        if o.better {
            file.push(Row::Item(A::Better, MenuItem::new(FIND_BETTER).icon(Icon::Search)));
        }
        // A file that holds another song: to the Trash, looked for again.
        if o.wrong_file {
            file.push(Row::Item(A::WrongFile, MenuItem::new(WRONG_FILE).icon(Icon::Delete)));
        }
    } else {
        if o.has_album {
            file.push(Row::Item(A::AlbumDownload, MenuItem::new("Скачать альбом").icon(Icon::Download)));
        }
        if o.pick_release {
            file.push(Row::Item(A::PickRelease, MenuItem::new(PICK_RELEASE).icon(Icon::Search)));
        }
        // A clip: its song by hand; a stream from YouTube: another version.
        if let Some(f) = o.find_song {
            file.push(Row::Item(A::FindSong, MenuItem::new(f).icon(Icon::MusicNote)));
        }
    }
    rows.push(Row::Sub(MenuItem::new(FILE).icon(Icon::Folder), file));
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Dislike, o.dislike));
    rows.extend(o.ban.map(|b| Row::Item(A::Ban, b)));
    if apart {
        rows.push(Row::Divider);
        rows.push(Row::Item(A::Apart, MenuItem::new(APART).icon(Icon::Close)));
    }
    rows
}

/// Where the rows of the sound go in a track's `rows`: right before «Файл».
pub(crate) fn sound_at<A>(rows: &[widgets::Row<A>]) -> usize {
    rows.iter()
        .position(|r| matches!(r, widgets::Row::Sub(item, _) if item.label == FILE))
        .unwrap_or(rows.len())
}

/// A track's `rows` as «Меню трека» (`slots`) has them: an item sent to
/// «Файл» joins the submenu, one taken out of it stands right before it,
/// a hidden one is gone; each keeps its place among the others of where it
/// went. `item` names the item an action is (`None`: the row stays).
pub(crate) fn placed<A: Copy>(
    rows: Vec<widgets::Row<A>>,
    slots: &[TrackMenuSlot],
    item: impl Fn(A) -> Option<TrackMenuItem>,
) -> Vec<widgets::Row<A>> {
    use TrackMenuPlace as P;
    use widgets::Row;
    let at = sound_at(&rows);
    if slots.is_empty() || at == rows.len() {
        return rows;
    }
    let place = |a: A| item(a).map(|i| TrackMenuSlot::place_of(slots, i));
    let (mut before, mut after) = (Vec::new(), Vec::new());
    // Sent to «Файл» from above it and from below it; taken out of it.
    let (mut head, mut tail, mut out) = (Vec::new(), Vec::new(), Vec::new());
    let mut file = Vec::new();
    let mut parent = None;
    for (i, r) in rows.into_iter().enumerate() {
        match r {
            Row::Sub(m, inner) if i == at => {
                parent = Some(m);
                for r in inner {
                    match r {
                        Row::Item(a, m) => match place(a) {
                            Some(P::Menu) => out.push(Row::Item(a, m)),
                            Some(P::Hidden) => {}
                            Some(P::File) | None => file.push(Row::Item(a, m)),
                        },
                        r => file.push(r),
                    }
                }
            }
            Row::Item(a, m) => {
                let (stay, moved) = if i < at { (&mut before, &mut head) } else { (&mut after, &mut tail) };
                match place(a) {
                    Some(P::File) => moved.push(Row::Item(a, m)),
                    Some(P::Hidden) => {}
                    Some(P::Menu) | None => stay.push(Row::Item(a, m)),
                }
            }
            r if i < at => before.push(r),
            r => after.push(r),
        }
    }
    // The deletes stay last, after their divider.
    let deletes = file.iter().position(|r| matches!(r, Row::Divider)).unwrap_or(file.len());
    drop(file.splice(deletes..deletes, tail));
    head.extend(file);
    before.extend(out);
    before.extend(parent.map(|m| Row::Sub(m, head)));
    before.extend(after);
    before
}

/// «Скорость…» of a library track's menu ([`sound_at`]): only for the
/// track `playing` (a list's row of it), while the speed of tracks is on in
/// the settings.
pub(crate) fn with_speed_row(
    mut rows: Vec<widgets::Row<TrackAct>>,
    speeds: &nikgob_view::TrackSpeeds,
    playing: bool,
) -> Vec<widgets::Row<TrackAct>> {
    if speeds.enabled && playing {
        let at = sound_at(&rows);
        rows.insert(at, widgets::Row::Item(TrackAct::Speed, MenuItem::new("Скорость…").icon(Icon::GraphicEq)));
    }
    rows
}

/// What a library track's menu knows besides the track.
#[derive(Clone, Copy)]
pub(crate) struct LibraryRowsOf<'a> {
    pub marks: &'a nikgob_view::DislikesView,
    /// «Источник и качество»: the track has something to show ([`has_info`]).
    pub info: bool,
    /// The other album of the same song.
    pub also: Option<&'a (ReleaseId, String)>,
    /// A torrent source is set up: «Выбрать раздачу…» can search.
    pub torrents: bool,
    /// How far the track is in «Хочу скачать».
    pub wished: Option<nikgob_view::WishlistStatus>,
}

impl<'a> LibraryRowsOf<'a> {
    /// What a list's row has: `state`'s marks and settings, the info of
    /// any track that has one, its place in «Хочу скачать» by `wish`.
    pub(crate) fn of(state: &'a ViewState, track: &TrackRow, wish: &widgets::WishIndex) -> Self {
        Self {
            marks: &state.dislikes,
            info: has_info(track),
            also: state.also_on.get(&track.id),
            torrents: state.prefs.prowlarr_configured,
            wished: (!track.has_local).then(|| wish.of_track(Some(track.id), track.release_id)).flatten(),
        }
    }
}

/// Whether «Источник и качество» of a library track shows something real:
/// its file, or the source it streams from.
pub(crate) fn has_info(track: &TrackRow) -> bool {
    track.has_local || track.source.is_some()
}

/// Whether `track` is the one playing.
pub(crate) fn is_playing(state: &ViewState, track: TrackId) -> bool {
    state.player.current.as_ref().is_some_and(|c| c.id == track)
}

/// «Скачать» of a track with no file, by how far it is in «Хочу скачать»:
/// there while nothing downloads it (not asked yet, waiting, not found),
/// none while it is on its way (its row's mark says so).
pub(crate) fn download_item(wished: Option<nikgob_view::WishlistStatus>) -> Option<MenuItem> {
    use nikgob_view::WishlistStatus as St;
    matches!(wished, None | Some(St::Waiting | St::NoTorrent | St::Failed)).then(|| MenuItem::new("Скачать").icon(Icon::Download))
}

/// «В библиотеку» of a track menu: what the item of a song does
/// ([`TrackMenuOf::library`]): put a song outside the library in it, take
/// one in it without a file or a like out (a like takes it out with the
/// heart); none for a file.
pub(crate) fn library_item(in_library: bool, has_local: bool, liked: bool) -> Option<bool> {
    if !in_library {
        Some(true)
    } else if !has_local && !liked {
        Some(false)
    } else {
        None
    }
}

/// [`track_rows`] of a library track (in the library or in a playlist).
pub(crate) fn library_track_rows(track: &TrackRow, o: LibraryRowsOf<'_>) -> Vec<widgets::Row<TrackAct>> {
    // A file: its tags and a better copy; no file: download it (no like)
    // or look for a release by hand.
    let fetch = if track.has_local { Some(MenuItem::new("Редактировать теги…").icon(Icon::Edit)) } else { download_item(o.wished) };
    let mut rows = track_rows(TrackMenuOf {
        playable: true,
        has_album: track.release_id.is_some(),
        has_artists: !track.artists.linked().is_empty(),
        has_file: track.has_local,
        also_on: o.also.map(|(_, a)| a.clone()),
        info: o.info,
        fetch,
        better: track.has_local,
        pick_release: !track.has_local && o.torrents,
        find_song: super::find_song::label(track),
        wrong_file: track.has_local,
        dislike: dislike_item(o.marks.track(track.id)),
        ban: track.artist_id().map(|a| ban_item(a, o.marks)),
        library: library_item(track.in_library, track.has_local, track.liked),
    });
    // A lossless file the check is sure holds an MP3 (its row says «FLAC
    // из MP3»): that MP3, right after the better copy. A doubt has none.
    let mp3 = track.has_local && track.quality.as_deref().is_some_and(nikgob_core::quality::row_label_mp3_inside);
    if mp3 && let Some(file) = widgets::sub_rows_mut(&mut rows, FILE) {
        let at = file
            .iter()
            .position(|r| matches!(r, widgets::Row::Item(TrackAct::Better, _)))
            .map_or(file.len(), |i| i + 1);
        file.insert(at, widgets::Row::Item(TrackAct::ToMp3, MenuItem::new(TO_MP3).icon(Icon::Sync)));
    }
    // The cover of the album of a file, last of the file's own items. A
    // song without an album has no cover of its own to refresh.
    if track.has_local
        && track.release_id.is_some()
        && let Some(file) = widgets::sub_rows_mut(&mut rows, FILE)
    {
        file.push(widgets::Row::Item(TrackAct::RefreshCover, MenuItem::new(widgets::REFRESH_COVER).icon(Icon::Refresh)));
    }
    rows
}

/// Whether the playing track's menu offers its delete: what is in the
/// library by a file or a like (a list's row is in the library anyway: a
/// stream track in a playlist is too, and its row always offers it).
pub(crate) fn can_trash(track: &TrackRow) -> bool {
    track.has_local || track.liked
}

/// The library's deletes, last in «Файл» of a library row's
/// [`library_track_rows`]: «Удалить навсегда» only for a file.
fn push_deletes(rows: &mut Vec<widgets::Row<TrackAct>>, track: &TrackRow) {
    let mut deletes = vec![
        widgets::Row::Divider,
        widgets::Row::Item(TrackAct::Trash, MenuItem::new("Удалить из библиотеки").icon(Icon::Delete)),
    ];
    if track.has_local {
        deletes.push(widgets::Row::Item(TrackAct::Purge, MenuItem::new("Удалить навсегда").icon(Icon::Delete)));
    }
    match widgets::sub_rows_mut(rows, FILE) {
        Some(file) => file.extend(deletes),
        None => rows.extend(deletes),
    }
}

/// «Выбрать раздачу…» of a library track: the indexer's search page for
/// its artist and album (its title without one).
pub(crate) fn pick_release_command(track: &TrackRow) -> Command {
    Command::SearchTorrents(format!("{} {}", track.artist, track.album.as_deref().unwrap_or(&track.title)))
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

/// Opens «Добавить устройство» at the first nearby device's code, or on
/// «Установить» (scripted screenshots; Settings → Межсеть and «Карта
/// сети» draw it).
pub(crate) fn open_add_device(ctx: &Context, state: &ViewState, pages: &mut PageState, install: bool, out: &mut Vec<Command>) {
    let tab = if install { add_device::AddTab::Install } else { add_device::AddTab::Connect };
    add_device::open(ctx, &state.mesh, &mut pages.mesh, None, tab, state.mesh.nearby.first().map(|n| n.id.clone()), out);
}

/// Opens «Добавить устройство» out of `from` (the output popover's
/// «Добавить»), at the first nearby device's code.
pub(crate) fn open_add_device_from(ctx: &Context, state: &ViewState, pages: &mut PageState, from: Rect, out: &mut Vec<Command>) {
    add_device::open(ctx, &state.mesh, &mut pages.mesh, Some(from), add_device::AddTab::Connect, state.mesh.nearby.first().map(|n| n.id.clone()), out);
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
            FabMenuItem::new(Icon::UploadFile, "Файлы…"),
            FabMenuItem::new(Icon::Folder, "Папку…"),
            FabMenuItem::new(Icon::AttachFile, ".torrent…"),
            FabMenuItem::new(Icon::Link, "Magnet-ссылку…"),
            FabMenuItem::new(Icon::PlaylistAdd, "Новый плейлист…"),
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
        sound::overlay(ctx, state, out);
        crate::screens::info_window::overlay(ctx, state, out);
        // Playlist picker (for the player's and the queue's menus too).
        if let Some((tracks, anchor)) = ctx.data_mut(|d| d.remove_temp::<(Vec<TrackId>, Option<Rect>)>(picker_request_key())) {
            self.pending_add = Some(tracks);
            self.pending_add_anchor = anchor;
        }
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

        // «Связать с сервисом…»: a playlist is made in the service at once.
        if let Some(id) = self.confirm_link {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-link", "Связать с сервисом?")
                .icon(Icon::Sync)
                .supporting("Плейлист появится в Spotify или YouTube Music, и изменения пойдут в обе стороны.")
                .action("Отмена")
                .action("Связать")
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(widgets::playlist_link_command(id));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_link = None;
            }
        }
        // A deletion asked from a menu without the pages at hand (a card's,
        // the player's).
        if let Some((what, title, forever)) = ctx.data_mut(|d| d.remove_temp::<(Doomed, String, bool)>(delete_request_key())) {
            if forever {
                self.confirm_purge = Some((what, title));
            } else {
                self.confirm_trash = Some((what, title));
            }
        }
        // «Разные песни»: for good, so asked.
        if let Some(apart) = ctx.data_mut(|d| d.remove_temp::<(TrackId, String, String)>(apart_request_key())) {
            self.confirm_apart = Some(apart);
        }
        if let Some((track, title, album)) = self.confirm_apart.clone() {
            let mut open = true;
            let r = crate::components::Dialog::new("confirm-apart", "Это разные песни?")
                .icon(Icon::Close)
                .supporting(format!("«{title}» и песня с альбома «{album}» больше не будут считаться одной. Отменить нельзя."))
                .action("Отмена")
                .action(APART)
                .show(ctx, &mut open);
            if r.action == Some(1) {
                out.push(Command::RecordingApart(track));
            }
            if r.action.is_some() || !open || r.dismissed {
                self.confirm_apart = None;
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
) -> SearchBarOutput {
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

#[cfg(test)]
mod speed_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use nikgob_view::{SpeedVariant, TrackSpeeds};

    use super::widgets::{Row, row_acts, row_items, shown_rows, sub_rows};
    use super::*;

    fn rows() -> Vec<Row<TrackAct>> {
        track_rows(TrackMenuOf {
            playable: true,
            has_album: true,
            has_artists: true,
            has_file: false,
            also_on: None,
            info: false,
            fetch: Some(MenuItem::new("Скачать")),
            better: false,
            pick_release: false,
            find_song: None,
            wrong_file: false,
            dislike: MenuItem::new("Не нравится"),
            ban: Some(MenuItem::new("Не рекомендовать исполнителя")),
            library: None,
        })
    }

    /// A library track with an album and an artist.
    fn track(has_local: bool) -> TrackRow {
        TrackRow {
            id: TrackId::new(), title: "Track".into(), artist: "Artist".into(),
            album: Some("Album".into()), release_id: Some(ReleaseId::new()),
            artists: nikgob_view::Credit::single("Artist", Some(nikgob_view::ArtistId::new())), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local, liked: false, cover: None, clip: false, in_library: true,
        }
    }

    /// A library row's menu as a list has it, with no torrent source and
    /// nothing in «Хочу скачать».
    fn of(marks: &nikgob_view::DislikesView, info: bool) -> LibraryRowsOf<'_> {
        LibraryRowsOf { marks, info, also: None, torrents: false, wished: None }
    }

    fn speeds(enabled: bool, chosen: &[(TrackId, SpeedVariant)]) -> TrackSpeeds {
        TrackSpeeds { enabled, chosen: Arc::new(chosen.iter().copied().collect::<HashMap<_, _>>()) }
    }

    fn labels(rows: &[Row<TrackAct>]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Item(_, item) | Row::Sub(item, _) => Some(item.label.clone()),
                Row::Divider | Row::Label(_) => None,
            })
            .collect()
    }

    /// «Скорость…» is in the menu of the track playing, right before
    /// «Файл», only while the speed of tracks is on in the settings; any
    /// other row has none.
    #[test]
    fn the_speed_row_is_there_only_while_the_speed_of_tracks_is_on() {
        let track = TrackId::new();
        let plain = row_acts(&rows());
        let off = with_speed_row(rows(), &speeds(false, &[(track, SpeedVariant::Fast)]), true);
        assert_eq!(row_acts(&off), plain, "switched off: the menu as it was");
        let other = with_speed_row(rows(), &speeds(true, &[(track, SpeedVariant::Fast)]), false);
        assert_eq!(row_acts(&other), plain, "a track that does not play: no speed");

        let on = with_speed_row(rows(), &speeds(true, &[]), true);
        let acts = row_acts(&on);
        assert_eq!(acts.len(), plain.len() + 1);
        // «Скачать» of a track with no file, then the speed.
        let share = acts.iter().position(|a| *a == TrackAct::Share).unwrap();
        assert_eq!(acts[share + 1..share + 3], [TrackAct::Fetch, TrackAct::Speed]);
        let at = on.iter().position(|r| matches!(r, Row::Item(TrackAct::Speed, _))).unwrap();
        assert!(matches!(on[at - 1], Row::Item(TrackAct::Fetch, _)));
        assert!(matches!(&on[at + 1], Row::Sub(item, _) if item.label == FILE));
    }

    /// The row is the same whatever the track's speed: nothing marks it.
    #[test]
    fn the_speed_row_is_not_marked() {
        let fast = TrackId::new();
        let s = speeds(true, &[(fast, SpeedVariant::Nightcore)]);
        let rows = with_speed_row(rows(), &s, true);
        let marked: Vec<bool> =
            row_items(&rows).into_iter().filter(|(a, _)| *a == TrackAct::Speed).map(|(_, item)| item.selected).collect();
        assert_eq!(marked, [false]);
    }

    /// The menu is in its groups: play next, the queue and the radio, then
    /// «В плейлист…», then where the track is from; «Скачать» of a track
    /// with no file in the menu itself, the rest of the files and the info
    /// behind «Файл».
    #[test]
    fn the_menu_is_in_its_groups_and_the_files_are_in_a_submenu() {
        let shown = shown_rows(rows());
        let top = labels(&shown);
        assert_eq!(&top[..4], [PLAY_NEXT, TO_QUEUE, RADIO_BY_TRACK, "В плейлист…"]);
        assert!(matches!(shown[3], Row::Divider) && matches!(shown[5], Row::Divider));
        // «Файл» stays a submenu even with «Скачать альбом» alone.
        assert_eq!(&top[top.len() - 5..], ["Поделиться", "Скачать", FILE, "Не нравится", "Не рекомендовать исполнителя"]);
        assert_eq!(row_acts(sub_rows(&shown, FILE).unwrap()), [TrackAct::AlbumDownload]);
        // The speed stands in the menu itself, right before «Файл».
        let top = labels(&with_speed_row(rows(), &speeds(true, &[]), true));
        let at = top.iter().position(|l| l == "Скорость…").unwrap();
        assert_eq!(top[at + 1], FILE);
        let shown = shown_rows(with_speed_row(rows(), &speeds(true, &[]), true));
        // What is shown stands for the same actions in the same order.
        assert_eq!(row_acts(&shown), row_acts(&with_speed_row(rows(), &speeds(true, &[]), true)));
        // A library row's deletes close «Файл».
        let t = track(true);
        let marks = nikgob_view::DislikesView::default();
        let mut library = library_track_rows(&t, of(&marks, true));
        push_deletes(&mut library, &t);
        let file = row_acts(sub_rows(&library, FILE).unwrap());
        assert_eq!(
            file,
            [TrackAct::Info, TrackAct::Fetch, TrackAct::Better, TrackAct::WrongFile, TrackAct::RefreshCover, TrackAct::Trash, TrackAct::Purge]
        );
        let top = labels(&library);
        let share = top.iter().position(|l| l == "Поделиться").unwrap();
        assert_eq!(top[share + 1], FILE);
        let acts = row_acts(&library);
        assert_eq!(&acts[acts.len() - 2..], [TrackAct::Dislike, TrackAct::Ban]);
    }

    /// What cannot work for a track is not in its menu at all: no album,
    /// no artist, a song that cannot play, «Удалить навсегда» without a
    /// file, «Скачать» of a track already on its way. A list's row always
    /// offers «Удалить из библиотеки» (a stream track in a playlist is in
    /// the library too).
    #[test]
    fn what_cannot_work_is_left_out() {
        let marks = nikgob_view::DislikesView::default();
        let bare = TrackRow { release_id: None, album: None, artists: Default::default(), ..track(false) };
        let acts = row_acts(&library_track_rows(&bare, of(&marks, false)));
        for a in [TrackAct::Album, TrackAct::Artist, TrackAct::AlbumDownload, TrackAct::Ban, TrackAct::Info] {
            assert!(!acts.contains(&a), "{a:?} in {acts:?}");
        }
        let mut rows = library_track_rows(&bare, of(&marks, false));
        push_deletes(&mut rows, &bare);
        let acts = row_acts(&rows);
        assert!(acts.contains(&TrackAct::Trash) && !acts.contains(&TrackAct::Purge), "no file: no delete for good");
        // «В библиотеку» of a song outside it, right after «В плейлист…»;
        // «Убрать из библиотеки» of one in it without a file or a like;
        // neither for a file or a liked song.
        let at = |acts: &[TrackAct], a| acts.iter().position(|x| *x == a);
        assert!(acts.contains(&TrackAct::OutOfLibrary) && !acts.contains(&TrackAct::ToLibrary));
        let heard = row_acts(&library_track_rows(&TrackRow { in_library: false, ..bare.clone() }, of(&marks, false)));
        assert_eq!(at(&heard, TrackAct::ToLibrary), at(&heard, TrackAct::Playlist).map(|p| p + 1));
        assert!(!heard.contains(&TrackAct::OutOfLibrary));
        for t in [track(true), TrackRow { liked: true, ..track(false) }] {
            let acts = row_acts(&library_track_rows(&t, of(&marks, false)));
            assert!(!acts.contains(&TrackAct::ToLibrary) && !acts.contains(&TrackAct::OutOfLibrary));
        }
        let unplayable = row_acts(&track_rows(TrackMenuOf { playable: false, ..of_stream() }));
        assert!(!unplayable.contains(&TrackAct::Next) && !unplayable.contains(&TrackAct::Queue));
        // On its way: no «Скачать»; waiting or not found: «Скачать».
        let t = track(false);
        let wished = |st| row_acts(&library_track_rows(&t, LibraryRowsOf { wished: Some(st), ..of(&marks, false) }));
        assert!(!wished(nikgob_view::WishlistStatus::Downloading).contains(&TrackAct::Fetch));
        assert!(wished(nikgob_view::WishlistStatus::Waiting).contains(&TrackAct::Fetch));
    }

    fn of_stream() -> TrackMenuOf {
        TrackMenuOf {
            playable: true,
            has_album: false,
            has_artists: false,
            has_file: false,
            also_on: None,
            info: false,
            fetch: None,
            better: false,
            pick_release: false,
            find_song: None,
            wrong_file: false,
            dislike: MenuItem::new("Не нравится"),
            ban: None,
            library: None,
        }
    }

    /// A track with a file: «Найти качество лучше»; without one and with a
    /// torrent source: «Выбрать раздачу…» (the album page's name), which a
    /// player with no torrent source does not offer.
    #[test]
    fn the_better_copy_and_the_release_by_hand_are_two_items() {
        let marks = nikgob_view::DislikesView::default();
        let file = |t: &TrackRow, torrents: bool| {
            let rows = library_track_rows(t, LibraryRowsOf { torrents, ..of(&marks, false) });
            row_acts(sub_rows(&rows, FILE).unwrap())
        };
        assert!(file(&track(true), true).contains(&TrackAct::Better));
        assert!(!file(&track(true), true).contains(&TrackAct::PickRelease));
        assert_eq!(file(&track(false), true), [TrackAct::AlbumDownload, TrackAct::PickRelease]);
        assert_eq!(file(&track(false), false), [TrackAct::AlbumDownload]);
        let rows = library_track_rows(&track(false), LibraryRowsOf { torrents: true, ..of(&marks, false) });
        assert!(row_items(&rows).iter().any(|(a, i)| *a == TrackAct::PickRelease && i.label == PICK_RELEASE));
        assert_eq!(TrackAct::PickRelease.menu_item(false), Some(TrackMenuItem::PickRelease));
        assert_eq!(TrackAct::Better.menu_item(true), Some(TrackMenuItem::Torrents));
    }

    /// «Разные песни» cannot be undone: it is last, after its divider.
    #[test]
    fn the_songs_apart_is_last() {
        let marks = nikgob_view::DislikesView::default();
        let also = (ReleaseId::new(), "Live".to_owned());
        let rows = library_track_rows(&track(true), LibraryRowsOf { also: Some(&also), ..of(&marks, false) });
        assert!(matches!(rows.last(), Some(Row::Item(TrackAct::Apart, _))));
        assert!(matches!(rows[rows.len() - 2], Row::Divider));
        let acts = row_acts(&rows);
        let album = acts.iter().position(|a| *a == TrackAct::Album).unwrap();
        assert_eq!(&acts[album..album + 3], [TrackAct::Album, TrackAct::AlsoOn, TrackAct::Artist]);
    }

    /// «Превратить в MP3» is in «Файл», right after «Найти качество
    /// лучше», only for a file whose row says it surely holds an MP3.
    #[test]
    fn only_a_sure_fake_offers_its_mp3() {
        let row = |quality: Option<&str>, has_local: bool| TrackRow { quality: quality.map(str::to_owned), ..track(has_local) };
        let marks = nikgob_view::DislikesView::default();
        let file = |t: &TrackRow| row_acts(sub_rows(&library_track_rows(t, of(&marks, true)), FILE).unwrap());
        let fake = file(&row(Some("FLAC из MP3 ~128"), true));
        let at = fake.iter().position(|a| *a == TrackAct::Better).unwrap();
        assert_eq!(fake[at + 1], TrackAct::ToMp3);
        for (q, local) in [(Some("FLAC"), true), (None, true), (Some("MP3 320 из поддельного FLAC"), true), (Some("FLAC из MP3 ~128"), false)] {
            assert!(!file(&row(q, local)).contains(&TrackAct::ToMp3), "{q:?} {local}");
        }
    }

    /// «Обновить обложку» of a file's album is the last of the file's own
    /// items in «Файл»; a song without an album or without a file has none.
    #[test]
    fn the_cover_of_a_files_album_is_refreshed_from_its_file_menu() {
        let marks = nikgob_view::DislikesView::default();
        let file = |t: &TrackRow| row_acts(sub_rows(&library_track_rows(t, of(&marks, true)), FILE).unwrap());
        assert_eq!(file(&track(true)).last(), Some(&TrackAct::RefreshCover));
        let loose = TrackRow { release_id: None, album: None, ..track(true) };
        assert!(!file(&loose).contains(&TrackAct::RefreshCover));
        assert!(!file(&track(false)).contains(&TrackAct::RefreshCover));
        assert_eq!(TrackAct::RefreshCover.menu_item(true), None);
        let full = track(true);
        let rows = library_track_rows(&full, of(&marks, true));
        let items = row_items(&rows);
        assert!(items.iter().any(|(a, i)| *a == TrackAct::RefreshCover && i.label == widgets::REFRESH_COVER));
    }

    /// «Меню трека»: an item sent to «Файл» joins the submenu in the menu's
    /// order, one taken out stands right before it, a hidden one is gone;
    /// nothing set, nothing changes.
    #[test]
    fn the_settings_place_the_items_of_the_menu() {
        use nikgob_view::{TrackMenuItem as I, TrackMenuPlace as P, TrackMenuSlot};
        use TrackAct as A;
        let t = track(true);
        let marks = nikgob_view::DislikesView::default();
        let library = || {
            let mut rows = with_speed_row(library_track_rows(&t, of(&marks, true)), &speeds(true, &[]), true);
            push_deletes(&mut rows, &t);
            rows
        };
        let with = |slots: &[(I, P)]| {
            let slots: Vec<TrackMenuSlot> = slots.iter().map(|&(item, place)| TrackMenuSlot { item, place }).collect();
            placed(library(), &slots, |a| a.menu_item(true))
        };
        let top = |rows: &[Row<A>]| -> Vec<A> {
            rows.iter()
                .filter_map(|r| match r {
                    Row::Item(a, _) => Some(*a),
                    _ => None,
                })
                .collect()
        };
        let file = |rows: &[Row<A>]| row_acts(sub_rows(rows, FILE).unwrap());
        assert_eq!(row_acts(&with(&[])), row_acts(&library()));
        let plain = (top(&library()), file(&library()));
        assert_eq!(plain.1[..3], [A::Info, A::Fetch, A::Better]);

        // Into «Файл»: before its own items, in the menu's order; from
        // below it: after them, before the deletes.
        let rows = with(&[(I::Share, P::File), (I::Radio, P::File), (I::Ban, P::File)]);
        assert!(!top(&rows).contains(&A::Share) && !top(&rows).contains(&A::Radio) && !top(&rows).contains(&A::Ban));
        let f = file(&rows);
        assert_eq!(f[..4], [A::Radio, A::Share, A::Info, A::Fetch]);
        assert_eq!(f[f.len() - 3..], [A::Ban, A::Trash, A::Purge]);
        assert!(matches!(sub_rows(&rows, FILE).unwrap()[f.len() - 2], Row::Divider));

        // Hidden: not built at all.
        let rows = with(&[(I::Info, P::Hidden), (I::Torrents, P::Hidden)]);
        let acts = row_acts(&rows);
        assert!(!acts.contains(&A::Info) && !acts.contains(&A::Better));
        assert_eq!(acts.len(), row_acts(&library()).len() - 2);

        // «Редактировать теги…» taken out: in the menu, right before «Файл».
        let rows = with(&[(I::Tags, P::Menu)]);
        let at = rows.iter().position(|r| matches!(r, Row::Sub(..))).unwrap();
        assert!(matches!(rows[at - 1], Row::Item(A::Fetch, _)) && matches!(rows[at - 2], Row::Item(A::Speed, _)));
        assert_eq!(file(&rows)[..2], [A::Info, A::Better]);

        // Everything it places taken out of «Файл»: the album's cover and
        // the deletes stay in it.
        let rows = with(&[(I::Info, P::Menu), (I::Tags, P::Menu), (I::Torrents, P::Menu), (I::WrongFile, P::Hidden)]);
        assert_eq!(file(&rows), [A::RefreshCover, A::Trash, A::Purge]);
        // No divider is doubled or left at an end once the menu is built.
        let (_, acts) = widgets::menu_of("placed-test", rows);
        assert!(acts.contains(&A::Fetch) && acts.contains(&A::Trash));
    }

    /// «Источник и качество» of a list's row: of any track with a file or
    /// a source to stream from, whichever plays.
    #[test]
    fn the_info_is_of_any_track_that_has_one() {
        let state = ViewState::default();
        let wish = widgets::WishIndex::default();
        let marks = nikgob_view::DislikesView::default();
        let file = track(true);
        let stream = TrackRow { source: Some(nikgob_view::SourceKind::Spotify), ..track(false) };
        let nothing = track(false);
        for (t, info) in [(&file, true), (&stream, true), (&nothing, false)] {
            assert_eq!(LibraryRowsOf::of(&state, t, &wish).info, info, "{:?}", t.source);
            let acts = row_acts(&library_track_rows(t, LibraryRowsOf { info: has_info(t), ..of(&marks, false) }));
            assert_eq!(acts.contains(&TrackAct::Info), info);
        }
        assert!(!is_playing(&state, file.id));
    }

    /// On a page, a track's menu leaves out what leads back to it: no «К
    /// альбому» on the page of its album, no «К исполнителю» on its
    /// artist's (one with another artist goes to that one), wherever
    /// «Меню трека» placed them; anywhere else both stay.
    #[test]
    fn a_menu_leaves_out_the_page_it_is_on() {
        use nikgob_view::{ArtistId, Credit, CreditName, StreamPage};
        let marks = nikgob_view::DislikesView::default();
        let t = track(true);
        let album = t.release_id.unwrap();
        let artist = *t.artists.main().unwrap();
        let acts = |here: &Here, t: &TrackRow| row_acts(&here.trim(library_track_rows(t, of(&marks, false)), t, None, Some));

        let anywhere = acts(&Here::default(), &t);
        assert!(anywhere.contains(&TrackAct::Album) && anywhere.contains(&TrackAct::Artist));
        for route in [Route::Home, Route::Library(LibraryTab::Tracks), Route::Playlist(nikgob_view::PlaylistId::new())] {
            assert_eq!(acts(&Here::of(&route), &t), anywhere, "{route:?}");
        }
        // Another album's or artist's page: both stay.
        assert_eq!(acts(&Here::of(&Route::Release(ReleaseId::new())), &t), anywhere);
        assert_eq!(acts(&Here::of(&Route::Artist(ArtistId::new())), &t), anywhere);

        let on_album = acts(&Here::of(&Route::Release(album)), &t);
        assert!(!on_album.contains(&TrackAct::Album) && on_album.contains(&TrackAct::Artist));
        // The album's download is no way to its page: it stays.
        let missing = track(false);
        let on_its_album = Here::of(&Route::Release(missing.release_id.unwrap()));
        let file = |rows: Vec<Row<TrackAct>>| row_acts(sub_rows(&rows, FILE).unwrap());
        assert!(file(on_its_album.trim(library_track_rows(&missing, of(&marks, false)), &missing, None, Some)).contains(&TrackAct::AlbumDownload));

        let on_artist = acts(&Here::of(&Route::Artist(artist)), &t);
        assert!(on_artist.contains(&TrackAct::Album) && !on_artist.contains(&TrackAct::Artist));
        // With a guest: «К исполнителю» goes to the guest.
        let guest = ArtistId::new();
        let duet = TrackRow {
            artists: Credit(vec![
                CreditName { name: "Artist".into(), id: Some(artist), join: " feat. ".into() },
                CreditName { name: "Guest".into(), id: Some(guest), join: String::new() },
            ]),
            ..track(true)
        };
        let here = Here::of(&Route::Artist(artist));
        assert!(acts(&here, &duet).contains(&TrackAct::Artist));
        assert_eq!(here.artists(&duet.artists), vec![("Guest".to_owned(), Route::Artist(guest))]);
        assert_eq!(Here::default().artists(&duet.artists).len(), 2);

        // Moved into «Файл» by «Меню трека»: left out there too.
        use nikgob_view::{TrackMenuItem as I, TrackMenuPlace as P, TrackMenuSlot};
        let slots = [TrackMenuSlot { item: I::Album, place: P::File }];
        let rows = placed(library_track_rows(&t, of(&marks, false)), &slots, |a| a.menu_item(true));
        assert!(row_acts(sub_rows(&rows, FILE).unwrap()).contains(&TrackAct::Album));
        let rows = Here::of(&Route::Release(album)).trim(rows, &t, None, Some);
        assert!(!row_acts(&rows).contains(&TrackAct::Album));

        // «Есть и на альбоме» of the album on screen: left out, «Разные
        // песни» stays.
        let other = ReleaseId::new();
        let also = (other, "Live".to_owned());
        let rows = library_track_rows(&t, LibraryRowsOf { also: Some(&also), ..of(&marks, false) });
        let rows = row_acts(&Here::of(&Route::Release(other)).trim(rows, &t, Some(other), Some));
        assert!(!rows.contains(&TrackAct::AlsoOn) && rows.contains(&TrackAct::Apart) && rows.contains(&TrackAct::Album));

        // A service's page: its album and its artist.
        let remote = |id: &str| nikgob_view::RemoteId::new(nikgob_view::Service::Spotify, id);
        let song = nikgob_view::StreamTrackRow {
            remote: remote("t"),
            title: "Song".into(),
            artist: "Band".into(),
            artists: Credit::single("Band", Some(remote("a"))),
            album: Some("LP".into()),
            album_id: Some(remote("l")),
            duration: None,
            disc_no: 1,
            track_no: 1,
            cover_url: None,
            kind: nikgob_view::TrackKind::Song,
            playable: true,
            track: None,
            liked: false,
            in_library: false,
            has_local: false,
            elsewhere: None,
            also_on: Vec::new(),
        };
        let keeps = |route: Route, act| Here::of(&route).keeps_stream(act, &song);
        assert!(!keeps(Route::Stream(StreamPage::Album(remote("l"))), TrackAct::Album));
        assert!(keeps(Route::Stream(StreamPage::Album(remote("l"))), TrackAct::Artist));
        assert!(!keeps(Route::Stream(StreamPage::Artist(remote("a"))), TrackAct::Artist));
        assert!(keeps(Route::Stream(StreamPage::Artist(remote("a"))), TrackAct::Album));
        assert!(keeps(Route::Stream(StreamPage::Album(remote("other"))), TrackAct::Album));
    }

    /// The page drawn this frame or the one before is where the menus are;
    /// once no page is drawn (Now Playing covers it), nowhere.
    #[test]
    fn the_page_on_screen_is_kept_for_a_frame() {
        let ctx = Context::default();
        let album = ReleaseId::new();
        let frame = |set: bool| {
            let mut seen = Here::default();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                if set {
                    set_here(ui.ctx(), Here::of(&Route::Release(album)));
                }
                seen = here(ui.ctx());
            });
            out.textures_delta.clear();
            seen
        };
        assert_eq!(frame(true).album, Some(album));
        assert_eq!(frame(false).album, Some(album), "the frame after it");
        frame(false);
        assert_eq!(frame(false), Here::default(), "no page for two frames: none");
    }
}
