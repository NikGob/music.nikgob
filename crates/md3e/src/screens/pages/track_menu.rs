//! The one list of a track's actions ([`track_rows`]) and the menus built
//! from it: the rows of a library track ([`library_track_rows`]) and
//! «Меню трека»'s placement ([`placed`]).

use nikgob_core::i18n;
use nikgob_view::{Command, ReleaseId, TrackId, TrackMenuItem, TrackMenuPlace, TrackMenuSlot, TrackRow, ViewState};

use super::widgets;
use super::{ban_item, dislike_item};
use crate::{Icon, components::MenuItem, screens::find_song};

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
        rows.push(Row::Item(A::Next, MenuItem::new(i18n::menus().play_next).icon(Icon::QueueMusic)));
        rows.push(Row::Item(A::Queue, MenuItem::new(i18n::menus().to_queue).icon(Icon::QueueMusic)));
    }
    rows.push(Row::Item(A::Radio, MenuItem::new(i18n::menus().radio_by_track).icon(Icon::Radio)));
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Playlist, MenuItem::new(i18n::menus().to_playlist).icon(Icon::PlaylistAdd)));
    match o.library {
        Some(true) => rows.push(Row::Item(A::ToLibrary, MenuItem::new(i18n::menus().to_library).icon(Icon::LibraryMusic))),
        Some(false) => rows.push(Row::Item(A::OutOfLibrary, MenuItem::new(i18n::menus().out_of_library).icon(Icon::LibraryMusic))),
        None => {}
    }
    rows.push(Row::Divider);
    if o.has_album {
        rows.push(Row::Item(A::Album, MenuItem::new(i18n::menus().to_album).icon(Icon::Album)));
    }
    // One song on two albums: the other album (the way out of it is last).
    let apart = o.also_on.is_some();
    if let Some(album) = o.also_on {
        rows.push(Row::Item(A::AlsoOn, MenuItem::new(i18n::menus::fmt_also_on_album(&album)).icon(Icon::Album)));
    }
    if o.has_artists {
        rows.push(Row::Item(A::Artist, MenuItem::new(i18n::menus().to_artist).icon(Icon::Person)));
    }
    rows.push(Row::Item(A::Share, MenuItem::new(i18n::menus().share).icon(Icon::Share)));
    rows.push(Row::Divider);
    let mut file = Vec::new();
    if o.info {
        file.push(Row::Item(A::Info, MenuItem::new(i18n::menus().source_quality).icon(Icon::Info)));
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
            file.push(Row::Item(A::Better, MenuItem::new(i18n::menus().find_better).icon(Icon::Search)));
        }
        // A file that holds another song: to the Trash, looked for again.
        if o.wrong_file {
            file.push(Row::Item(A::WrongFile, MenuItem::new(i18n::menus().wrong_file).icon(Icon::Delete)));
        }
    } else {
        if o.has_album {
            file.push(Row::Item(A::AlbumDownload, MenuItem::new(i18n::menus().download_album).icon(Icon::Download)));
        }
        if o.pick_release {
            file.push(Row::Item(A::PickRelease, MenuItem::new(i18n::menus().pick_release).icon(Icon::Search)));
        }
        // A clip: its song by hand; a stream from YouTube: another version.
        if let Some(f) = o.find_song {
            file.push(Row::Item(A::FindSong, MenuItem::new(f).icon(Icon::MusicNote)));
        }
    }
    rows.push(Row::Sub(widgets::SubId::File, MenuItem::new(i18n::menus().file).icon(Icon::Folder), file));
    rows.push(Row::Divider);
    rows.push(Row::Item(A::Dislike, o.dislike));
    rows.extend(o.ban.map(|b| Row::Item(A::Ban, b)));
    if apart {
        rows.push(Row::Divider);
        rows.push(Row::Item(A::Apart, MenuItem::new(i18n::menus().apart).icon(Icon::Close)));
    }
    rows
}

/// Where the rows of the sound go in a track's `rows`: right before «Файл».
pub(crate) fn sound_at<A>(rows: &[widgets::Row<A>]) -> usize {
    rows.iter()
        .position(|r| matches!(r, widgets::Row::Sub(widgets::SubId::File, _, _)))
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
            Row::Sub(id, m, inner) if i == at => {
                parent = Some((id, m));
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
    before.extend(parent.map(|(id, m)| Row::Sub(id, m, head)));
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
        rows.insert(at, widgets::Row::Item(TrackAct::Speed, MenuItem::new(i18n::menus().speed).icon(Icon::GraphicEq)));
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
            wished: (!track.has_local || track.temporary).then(|| wish.of_track(Some(track.id), track.release_id)).flatten(),
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
    matches!(wished, None | Some(St::Waiting | St::NoTorrent | St::Failed)).then(|| MenuItem::new(i18n::menus().download).icon(Icon::Download))
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
    // or look for a release by hand. A temporary copy (saved from a stream
    // until a real one is found) is asked for again, like a track without
    // a file: «Скачать», not the tags of the file standing in.
    let fetch = if track.has_local && !track.temporary { Some(MenuItem::new(i18n::menus().edit_tags).icon(Icon::Edit)) } else { download_item(o.wished) };
    let mut rows = track_rows(TrackMenuOf {
        playable: true,
        has_album: track.release_id.is_some(),
        has_artists: !track.artists.linked().is_empty(),
        has_file: track.has_local && !track.temporary,
        also_on: o.also.map(|(_, a)| a.clone()),
        info: o.info,
        fetch,
        better: track.has_local,
        pick_release: (!track.has_local || track.temporary) && o.torrents,
        find_song: find_song::label(track),
        wrong_file: track.has_local,
        dislike: dislike_item(o.marks.track(track.id)),
        ban: track.artist_id().map(|a| ban_item(a, o.marks)),
        library: library_item(track.in_library, track.has_local, track.liked),
    });
    // A lossless file the check is sure holds an MP3 (its row says «FLAC
    // из MP3»): that MP3, right after the better copy. A doubt has none.
    let mp3 = track.has_local && track.quality.as_deref().is_some_and(nikgob_core::quality::row_label_mp3_inside);
    if mp3 && let Some(file) = widgets::sub_rows_mut(&mut rows, widgets::SubId::File) {
        let at = file
            .iter()
            .position(|r| matches!(r, widgets::Row::Item(TrackAct::Better, _)))
            .map_or(file.len(), |i| i + 1);
        file.insert(at, widgets::Row::Item(TrackAct::ToMp3, MenuItem::new(i18n::menus().to_mp3).icon(Icon::Sync)));
    }
    // The cover of the album of a file, last of the file's own items. A
    // song without an album has no cover of its own to refresh; a
    // temporary copy's cover comes from its file as it is.
    if track.has_local
        && !track.temporary
        && track.release_id.is_some()
        && let Some(file) = widgets::sub_rows_mut(&mut rows, widgets::SubId::File)
    {
        file.push(widgets::Row::Item(TrackAct::RefreshCover, MenuItem::new(i18n::menus().refresh_cover).icon(Icon::Refresh)));
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
pub(super) fn push_deletes(rows: &mut Vec<widgets::Row<TrackAct>>, track: &TrackRow) {
    let mut deletes = vec![
        widgets::Row::Divider,
        widgets::Row::Item(TrackAct::Trash, MenuItem::new(i18n::menus().delete_from_library).icon(Icon::Delete)),
    ];
    if track.has_local {
        deletes.push(widgets::Row::Item(TrackAct::Purge, MenuItem::new(i18n::menus().purge_forever).icon(Icon::Delete)));
    }
    match widgets::sub_rows_mut(rows, widgets::SubId::File) {
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
