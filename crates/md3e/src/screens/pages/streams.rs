//! What the streaming services show (docs/stage3.md §3.2–3.4): their
//! tracks, albums, artists and playlists as rows and cards, and the album /
//! playlist / artist page of a service ([`Route::Stream`]), laid out like
//! the library's release, playlist and artist pages. Where something comes
//! from is said in words (the page's overline, the search section), never
//! with brand colours or badges.

use egui::{CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_view::{
    Command, LikeTarget, ReleaseType, RemoteId, Route, Service, ShareTarget, Status, StreamAlbumCard,
    StreamArtistCard, StreamPage, StreamPageView, StreamPlaylistCard, StreamTrackRow, TrackKind, ViewState,
    WishlistStatus, WishlistView,
};

use super::{PageState, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        AssistChip, Button, ButtonRow, CoverStatus, Equalizer, IconButton, Leading, ListItem, LoadingIndicator, MediaCard,
        Member, Menu, MenuItem, Radii, Size, Style, paint,
    },
    screens::{artist_links, common},
    tokens::shape,
};

/// "Spotify", "YouTube Music", "Deezer" as a page's overline says it.
pub fn service_upper(service: Service) -> &'static str {
    match service {
        Service::Spotify => "SPOTIFY",
        Service::Youtube => "YOUTUBE MUSIC",
        Service::Deezer => "DEEZER",
        Service::Soundcloud => "SOUNDCLOUD",
    }
}

/// A page's overline in one style: «АЛЬБОМ · 1988 · SPOTIFY».
fn overline(ui: &mut Ui, parts: &[&str]) {
    let s = Theme::get(ui.ctx()).scheme;
    let text = parts.iter().filter(|p| !p.is_empty()).copied().collect::<Vec<_>>().join(" · ");
    ui.label(TypeRole::LabelLarge.style().rich(text, s.primary));
}

/// Where a list of stream tracks plays from.
#[derive(Clone, Debug)]
pub enum StreamFrom {
    /// The rows themselves, as a queue named `context`.
    Rows(String),
    Album(RemoteId),
    Playlist(RemoteId),
    /// The merged «Всё» search: a queue of library and stream tracks,
    /// started at `at` (the row's own place in it).
    Search {
        picks: Vec<nikgob_view::SearchPick>,
        at: usize,
        context: String,
    },
}

fn play_from(from: &StreamFrom, rows: &[StreamTrackRow], i: usize) -> Command {
    match from {
        StreamFrom::Rows(ctx) => Command::PlayStreamTracks {
            tracks: rows.iter().map(|r| r.remote.clone()).collect(),
            start: i,
            context: ctx.clone(),
        },
        StreamFrom::Album(id) => Command::PlayStreamAlbum { id: id.clone(), start: Some(rows[i].remote.clone()), shuffle: false },
        StreamFrom::Playlist(id) => {
            Command::PlayStreamPlaylist { id: id.clone(), start: Some(rows[i].remote.clone()), shuffle: false }
        }
        StreamFrom::Search { picks, at, context } => Command::PlaySearch {
            tracks: picks.clone(),
            start: *at,
            context: context.clone(),
        },
    }
}

/// How far a stream track is in «Хочу скачать» (its library track's item).
fn wish_of(t: &StreamTrackRow, wish: &widgets::WishIndex) -> Option<WishlistStatus> {
    (!t.has_local).then(|| wish.of_track(t.track, None)).flatten()
}

/// Rows of stream tracks: cover (or number, ▶ on hover), title, artist ·
/// album (· клип · у тебя), duration, like and ⋮. Double click (or the ▶)
/// plays from the row. A long list lays out only the rows on screen.
#[allow(clippy::too_many_arguments)]
pub fn track_rows(
    ui: &mut Ui,
    id: &str,
    rows: &[StreamTrackRow],
    numbered: bool,
    from: &StreamFrom,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let mut nav = None;
    // An album by one artist needs no artist line under every title.
    let two_line = !numbered || rows.first().is_some_and(|f| rows.iter().any(|t| t.artist != f.artist));
    let wish = widgets::WishIndex::of(ui.ctx(), &state.downloads.wishlist);
    let row_h = if two_line { crate::components::list::TWO_LINE } else { crate::components::list::ONE_LINE };
    widgets::virtual_rows(ui, (id, "st-rows"), rows.len(), row_h, |ui, i| {
        nav = track_row(ui, id, rows, i, numbered, two_line, from, &wish, state, pages, out).or(nav.take());
    });
    nav
}

#[allow(clippy::too_many_arguments)]
fn track_row(
    ui: &mut Ui,
    id: &str,
    rows: &[StreamTrackRow],
    i: usize,
    numbered: bool,
    two_line: bool,
    from: &StreamFrom,
    wish: &widgets::WishIndex,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let t = &rows[i];
    let current = state.player.current.as_ref().map(|t| t.id);
    let playing = state.player.status == Status::Playing;
    let is_cur = t.track.is_some() && t.track == current;
    let leading = if numbered {
        Leading::Number(if is_cur { String::new() } else if t.track_no > 0 { t.track_no.to_string() } else { (i + 1).to_string() })
    } else {
        Leading::Image(t.cover_url.clone(), Icon::MusicNote)
    };
    // What the row is, in words: a music video, a song already on disk.
    let mut supporting = t.artist.clone();
    // Each artist of the line is a link to its page in the service, and so
    // is the album.
    let (mut links, mut routes) = artist_links::stream_spans(&t.artists, &t.artist);
    if !numbered && let Some(a) = &t.album {
        let at = artist_links::append_source(&mut supporting, a);
        if let Some(id) = &t.album_id {
            links.push(at);
            routes.push(Route::Stream(StreamPage::Album(id.clone())));
        }
    }
    let mut extras: Vec<String> = Vec::new();
    if t.kind == TrackKind::Video {
        extras.push("клип".into());
    }
    // A song filed on another library album says which.
    match (&t.elsewhere, t.has_local) {
        (Some(a), true) => extras.push(format!("у тебя в «{a}»")),
        (None, true) => extras.push("у тебя".into()),
        _ => {}
    }
    if !t.playable {
        extras.push("недоступен".into());
    }
    // Two lines: after the artist; one line (an album page): quiet text by
    // the duration, so every row keeps its height.
    let trailing_extras = if two_line {
        for e in &extras {
            supporting = format!("{supporting} · {e}");
        }
        String::new()
    } else {
        extras.join(" · ")
    };
    let wished = wish_of(t, wish);
    let dur = t.duration.map(common::fmt_time).unwrap_or_default();
    let mut menu_resp = None;
    let mut item = ListItem::new(&t.title);
    if two_line {
        item = item.supporting(supporting).supporting_links(links);
    }
    let o = item
        .leading(leading)
        .selected(is_cur)
        .muted(!t.playable)
        .hover_play(!is_cur && t.playable)
        .trailing(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                menu_resp = Some(ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("st-more", id, i))));
                if let Some(st) = wished
                    && widgets::wish_mark(ui, st, state.prefs.prowlarr_configured, ("st-wish", id, i)).clicked()
                {
                    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(widgets::OPEN_WISHLIST), true));
                }
                if ui
                    .add(
                        IconButton::new(Icon::Favorite)
                            .size(Size::Xs)
                            .selected(t.liked)
                            .tooltip(super::like_label(t.liked))
                            .id_salt(("st-like", id, i)),
                    )
                    .clicked()
                {
                    out.push(Command::LikeStream { track: t.remote.clone(), on: !t.liked });
                }
                ui.label(TypeRole::BodyMedium.style().rich(dur, s.on_surface_variant));
                if !trailing_extras.is_empty() {
                    ui.label(TypeRole::LabelSmall.style().rich(trailing_extras.as_str(), s.on_surface_variant));
                }
            });
        })
        .id_salt(("st-row", id, i))
        .show(ui);
    if is_cur {
        let c = if numbered {
            pos2(o.rect.left() + 16.0 + 14.0, o.rect.center().y)
        } else {
            let c = pos2(o.rect.left() + 16.0 + 28.0, o.rect.center().y);
            paint::container(ui.painter(), Rect::from_center_size(c, vec2(56.0, 56.0)), Radii::all(shape::SMALL), s.secondary_container.gamma_multiply(0.85), Stroke::NONE);
            c
        };
        Equalizer::new(("st-eq", id, i), s.on_secondary_container)
            .playing(playing)
            .paint_at(ui, Rect::from_center_size(c, vec2(24.0, 24.0)));
    }
    if (o.response.double_clicked() || o.play_clicked) && t.playable {
        out.push(if is_cur { Command::TogglePlay } else { play_from(from, rows, i) });
    }
    let mut nav = None;
    if let Some(m) = &menu_resp {
        let (menu, acts) = track_menu(egui::Id::new(("st-menu", id, i)), t, wished, &super::marks(&m.ctx), &super::track_menu_slots(&m.ctx));
        let act = menu.attach(m).and_then(|k| acts.get(k).copied());
        nav = track_pick(&m.ctx, act, t, Some(m.rect), pages, out);
    }
    let (menu, acts) = track_menu(egui::Id::new(("st-menu-ctx", id, i)), t, wished, &super::marks(&o.response.ctx), &super::track_menu_slots(&o.response.ctx));
    let act = menu.context_menu(&o.response).and_then(|k| acts.get(k).copied());
    let nav = track_pick(&o.response.ctx, act, t, None, pages, out).or(nav);
    // An artist name or the album of the line: its page.
    o.link_clicked.and_then(|k| routes.get(k).cloned()).or(nav)
}

/// [`super::track_rows`] of a service's song: the common rows in the
/// library track's order, its download for the files (none once it has a
/// file or while it is on its way: the row's mark says so).
pub(crate) fn stream_track_rows(
    t: &StreamTrackRow,
    wished: Option<WishlistStatus>,
    marks: &nikgob_view::DislikesView,
) -> Vec<widgets::Row<super::TrackAct>> {
    super::track_rows(super::TrackMenuOf {
        playable: t.playable,
        has_album: t.album_id.is_some(),
        has_artists: !t.artists.linked().is_empty(),
        has_file: t.has_local,
        also_on: None,
        info: false,
        fetch: if t.has_local { None } else { super::download_item(wished) },
        better: false,
        pick_release: false,
        find_song: None,
        wrong_file: false,
        dislike: super::dislike_item(t.track.is_some_and(|id| marks.track(id))),
        ban: (!t.artist.is_empty()).then(|| MenuItem::new("Не рекомендовать исполнителя").icon(Icon::Block)),
    })
}

fn track_menu(
    key: egui::Id,
    t: &StreamTrackRow,
    wished: Option<WishlistStatus>,
    marks: &nikgob_view::DislikesView,
    slots: &[nikgob_view::TrackMenuSlot],
) -> (Menu, Vec<super::TrackAct>) {
    // Its «Скачать» is the download, never the tags.
    let rows = super::placed(stream_track_rows(t, wished, marks), slots, |a| a.menu_item(false));
    let (menu, acts) = widgets::menu_of(key, rows);
    (menu.align_right(true), acts)
}

/// A service song's menu on a right click on `resp` (a card that stands
/// for it, as search's top one).
pub(super) fn track_context_menu(
    resp: &egui::Response,
    t: &StreamTrackRow,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let key = egui::Id::new(("st-menu-card", resp.id));
    if Menu::context_idle(resp, key) {
        return None;
    }
    let wish = widgets::WishIndex::of(&resp.ctx, &state.downloads.wishlist);
    let (menu, acts) = track_menu(key, t, wish_of(t, &wish), &state.dislikes, &state.prefs.track_menu);
    let act = menu.context_menu(resp).and_then(|k| acts.get(k).copied());
    track_pick(&resp.ctx, act, t, None, pages, out)
}

/// «Не нравится» of a service's track: its library track's mark when it
/// has one, else the song's on every service.
fn stream_dislike(t: &StreamTrackRow, marks: &nikgob_view::DislikesView) -> Command {
    match t.track {
        Some(id) => super::dislike_command(id, marks),
        None => Command::RecFeedback { target: nikgob_view::RecTarget::StreamTrack(t.remote.clone()), dislike: true },
    }
}

pub(super) fn track_pick(
    ctx: &egui::Context,
    act: Option<super::TrackAct>,
    t: &StreamTrackRow,
    anchor: Option<Rect>,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    use super::TrackAct as A;
    match act? {
        A::Next => out.push(Command::PlayStreamNext(vec![t.remote.clone()])),
        A::Queue => out.push(Command::EnqueueStream(vec![t.remote.clone()])),
        A::Playlist => pages.add_stream_to_playlist(vec![t.remote.clone()], anchor),
        A::Radio => out.push(radio_by_stream(t)),
        A::Album => return t.album_id.clone().map(|a| Route::Stream(StreamPage::Album(a))),
        // Several artists: a chooser.
        A::Artist => return artist_links::go_to(ctx, artist_links::stream(&t.artists)),
        A::Share => out.push(Command::Share(ShareTarget::StreamTrack(t.remote.clone()))),
        // Download without a like: the like stays the ♡'s.
        A::Fetch => out.push(Command::DownloadStream { tracks: vec![t.remote.clone()] }),
        // The whole album the track is from, without a like.
        A::AlbumDownload => {
            if let Some(a) = t.album_id.clone() {
                out.push(Command::DownloadStreamAlbum(a));
            }
        }
        A::Dislike => out.push(stream_dislike(t, &super::marks(ctx))),
        A::Ban => out.push(Command::RecFeedback { target: nikgob_view::RecTarget::StreamArtist(t.artist.clone()), dislike: true }),
        // Only a library track has these.
        A::AlsoOn | A::Apart | A::Info | A::Speed | A::Equalizer | A::Better | A::PickRelease | A::ToMp3 | A::FindSong | A::WrongFile
        | A::Remove | A::Trash | A::Purge => {}
    }
    None
}

/// «Радио по треку» of a service's track: its library track when it has one
/// (the radio then knows its sound and history), else the service's track.
pub fn radio_by_stream(t: &StreamTrackRow) -> Command {
    match t.track {
        Some(id) => super::radio_by_track(id),
        None => Command::StartRadio { seed: nikgob_view::RadioSeed::Stream(t.remote.clone()) },
    }
}

/// "Артист · 1988".
fn album_subtitle(a: &StreamAlbumCard) -> String {
    match a.year {
        Some(y) if !a.artist.is_empty() => format!("{} · {y}", a.artist),
        Some(y) => y.to_string(),
        None => a.artist.clone(),
    }
}

/// What the cover of a service's album card says: «в очереди» or a ring
/// while its wishlist item is on its way (downloading: filled to the tracks
/// already on disk), a check when every track has a file, else the
/// [`AlbumHave`](nikgob_view::AlbumHave) label alone.
pub fn album_status(a: &StreamAlbumCard, wishlist: &WishlistView) -> CoverStatus {
    use nikgob_view::WishlistStatus as St;
    let same = |x: &str, y: &str| x.trim().to_lowercase() == y.trim().to_lowercase();
    let mine = |r: &&nikgob_view::WishlistRow| match (r.release, a.release) {
        (Some(x), Some(y)) => x == y,
        _ => {
            let title = r.album.as_deref().unwrap_or(&r.title);
            same(title, &a.title) && same(&r.artist, &a.artist)
        }
    };
    let rows: Vec<_> = wishlist.items.iter().filter(mine).collect();
    if rows.iter().any(|r| r.status == St::Downloading) {
        let h = a.have;
        return CoverStatus::Progress((h.total > 0 && h.local > 0).then(|| h.local as f32 / h.total as f32));
    }
    if rows.iter().any(|r| matches!(r.status, St::New | St::Searching | St::Confirm)) {
        return CoverStatus::Queued;
    }
    if a.have.complete() { CoverStatus::Done } else { CoverStatus::None }
}

/// An album of a service as a card, like a library album: opens its page,
/// ▶ plays it; on hover a small ⬇ (the album to «Хочу скачать», no like)
/// and ⋮; right click or ⋮: [`stream_album_rows`]. The cover corner shows
/// its download status ([`album_status`]).
pub fn album_card(
    ui: &mut Ui,
    a: &StreamAlbumCard,
    width: f32,
    row: &str,
    wishlist: &WishlistView,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let status = album_status(a, wishlist);
    let can_download = status == CoverStatus::None;
    let o = MediaCard::new(&a.title)
        .subtitle(album_subtitle(a))
        .cover_opt(a.cover_url.clone())
        .badge_opt(a.have.label())
        .status(status)
        .download_button(can_download.then_some("Скачать альбом"))
        .menu_button(true)
        .width(width)
        .id_salt(("st-album", row, &a.remote))
        .show(ui);
    if o.download_clicked {
        out.push(Command::DownloadStreamAlbum(a.remote.clone()));
    }
    let nav = album_menus(&o.response, o.menu.as_ref(), a, wishlist, out);
    if o.play_clicked {
        out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false });
        None
    } else if o.card_clicked {
        Some(Route::Stream(StreamPage::Album(a.remote.clone())))
    } else {
        nav
    }
}

/// The menu of a service's album card on a right click on `resp` and on
/// its ⋮ (`more`): [`stream_album_rows`]. For every card of such an album
/// (search, an artist, «Новые релизы», a world's «Новое»).
pub fn album_menus(resp: &egui::Response, more: Option<&egui::Response>, a: &StreamAlbumCard, wishlist: &WishlistView, out: &mut Vec<Command>) -> Option<Route> {
    let of = || album_of(a, album_status(a, wishlist) == CoverStatus::None);
    let mut act = None;
    if let Some(m) = more {
        let (menu, acts) = widgets::menu_of(("st-album-more", m.id), stream_album_rows(of()));
        act = menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied());
    }
    if !Menu::context_idle(resp, ("st-album-menu", resp.id)) {
        let (menu, acts) = widgets::menu_of(("st-album-menu", resp.id), stream_album_rows(of()));
        act = act.or(menu.context_menu(resp).and_then(|i| acts.get(i).copied()));
    }
    stream_album_apply(&resp.ctx, act?, &a.remote, &a.artists, a.release, out)
}

/// What the menu of a service's album does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamAlbumAct {
    Play,
    Shuffle,
    Next,
    Queue,
    Playlist,
    Like,
    Artist,
    Share,
    Download,
    Library,
}

/// What a service's album menu is built over.
#[derive(Clone, Copy, Debug, Default)]
pub struct StreamAlbumMenuOf {
    /// The ⋮ of its page (else the card's right click or ⋮).
    pub on_page: bool,
    pub has_artists: bool,
    /// Linked to a library album.
    pub in_library: bool,
    /// Nothing downloads it yet and the library lacks some of it.
    pub can_download: bool,
}

/// The one list of a service album's actions, in the order of the library
/// album's ([`widgets::release_rows`]): its card's right click and ⋮ and
/// the ⋮ of its page. Play │ the like, a playlist │ the artist, its
/// library copy, share │ the download. The page leaves out its buttons
/// (play, shuffle, the like, the download); on the card playing next, the
/// queue and «В плейлист…» go by the album's id (the runtime fetches its
/// tracks).
pub fn stream_album_rows(o: StreamAlbumMenuOf) -> Vec<widgets::Row<StreamAlbumAct>> {
    use StreamAlbumAct as A;
    use widgets::Row;
    let mut rows = Vec::new();
    if !o.on_page {
        rows.push(Row::Item(A::Play, MenuItem::new("Слушать").icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new("Перемешать").icon(Icon::Shuffle)));
    }
    rows.push(Row::Item(A::Next, MenuItem::new(super::PLAY_NEXT).icon(Icon::QueueMusic)));
    rows.push(Row::Item(A::Queue, MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic)));
    rows.push(Row::Divider);
    if !o.on_page {
        // Liking is not downloading: there whatever the library has of it.
        rows.push(Row::Item(A::Like, MenuItem::new(super::LIKE).icon(Icon::Favorite)));
    }
    rows.push(Row::Item(A::Playlist, MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd)));
    rows.push(Row::Divider);
    if o.has_artists {
        rows.push(Row::Item(A::Artist, MenuItem::new("К исполнителю").icon(Icon::Person)));
    }
    if o.in_library {
        rows.push(Row::Item(A::Library, MenuItem::new("Открыть в библиотеке").icon(Icon::LibraryMusic)));
    }
    rows.push(Row::Item(A::Share, MenuItem::new("Поделиться").icon(Icon::Share)));
    if !o.on_page && o.can_download {
        rows.push(Row::Divider);
        rows.push(Row::Item(A::Download, MenuItem::new("Скачать альбом").icon(Icon::Download)));
    }
    rows
}

/// Does a service album action (the page does its next / queue / «В
/// плейлист…» with the tracks it has).
fn stream_album_apply(
    ctx: &egui::Context,
    act: StreamAlbumAct,
    id: &RemoteId,
    artists: &nikgob_view::StreamArtistLinks,
    release: Option<nikgob_view::ReleaseId>,
    out: &mut Vec<Command>,
) -> Option<Route> {
    use StreamAlbumAct as A;
    match act {
        A::Play => out.push(Command::PlayStreamAlbum { id: id.clone(), start: None, shuffle: false }),
        A::Shuffle => out.push(Command::PlayStreamAlbum { id: id.clone(), start: None, shuffle: true }),
        // «Скачать альбом» downloads without a like; «Нравится» is the like.
        A::Download => out.push(Command::DownloadStreamAlbum(id.clone())),
        A::Like => out.push(Command::LikeStreamAlbum { album: id.clone(), on: true }),
        A::Share => out.push(Command::Share(ShareTarget::StreamAlbum(id.clone()))),
        A::Artist => return artist_links::go_to(ctx, artist_links::stream(artists)),
        A::Library => return release.map(Route::Release),
        A::Next => out.push(Command::PlayStreamAlbumNext(id.clone())),
        A::Queue => out.push(Command::EnqueueStreamAlbum(id.clone())),
        A::Playlist => {
            let anchor = ctx.input(|i| i.pointer.latest_pos()).map(|p| egui::Rect::from_center_size(p, egui::vec2(1.0, 1.0)));
            super::request_stream_album_to_playlist(ctx, id.clone(), anchor);
        }
    }
    None
}

fn album_of(a: &StreamAlbumCard, can_download: bool) -> StreamAlbumMenuOf {
    StreamAlbumMenuOf {
        on_page: false,
        has_artists: !a.artists.linked().is_empty(),
        in_library: a.release.is_some(),
        can_download,
    }
}

/// An artist of a service: the photo (or initials) in a shape, the name;
/// ⋮ under the pointer and the right click: [`stream_artist_rows`]. A
/// library artist opens the library page, anyone else the service's.
pub fn artist_card(ui: &mut Ui, a: &StreamArtistCard, size: f32, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size + 28.0), Sense::click());
    let img = Rect::from_min_size(rect.min, vec2(size, size));
    let shape = if a.image_url.is_some() {
        nikgob_shapes::MaterialShape::Circle
    } else {
        widgets::avatar_shape(&a.remote.id)
    };
    widgets::shape_image(ui, a.image_url.as_deref(), &widgets::initials(&a.name), img, shape);
    let g = common_line(ui, &a.name, size);
    ui.painter().galley(pos2(rect.center().x - g.size().x * 0.5, img.bottom() + 6.0), g, s.on_surface);
    let id = ui.id().with(("st-artist", &a.remote));
    let menu_open = ui.ctx().data(|d| d.get_temp::<bool>(id.with("more-open"))).unwrap_or(false);
    let hover = resp.hovered() || menu_open;
    if hover {
        ui.painter().circle_filled(img.center(), size * 0.5, s.on_surface.gamma_multiply(0.06));
    }
    let more = super::cards::more_button(ui, id, hover, widgets::card_more_slot(img));
    super::cards::keep_more_open(ui.ctx(), id, more.as_ref());
    artist_menus(&resp, more.as_ref(), a, out);
    let more_clicked = more.as_ref().is_some_and(egui::Response::clicked);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    (resp.clicked() && !more_clicked).then(|| match a.artist {
        Some(id) => Route::Artist(id),
        None => Route::Stream(StreamPage::Artist(a.remote.clone())),
    })
}

/// The menu of a service's artist on a right click on `resp` and on its
/// ⋮ (`more`): [`stream_artist_rows`].
pub fn artist_menus(resp: &egui::Response, more: Option<&egui::Response>, a: &StreamArtistCard, out: &mut Vec<Command>) {
    let marks = super::marks(&resp.ctx);
    let mut act = None;
    if let Some(m) = more {
        let (menu, acts) = widgets::menu_of(("st-artist-more", m.id), stream_artist_rows(a, &marks));
        act = menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied());
    }
    if !Menu::context_idle(resp, ("st-artist-menu", resp.id)) {
        let (menu, acts) = widgets::menu_of(("st-artist-menu", resp.id), stream_artist_rows(a, &marks));
        act = act.or(menu.context_menu(resp).and_then(|i| acts.get(i).copied()));
    }
    if let Some(act) = act {
        out.push(stream_artist_command(act, a, &marks));
    }
}

/// What the menu of a service's artist does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamArtistAct {
    Play,
    Shuffle,
    Radio,
    Queue,
    Ban,
}

/// The menu of a service's artist: a library artist's play, shuffle,
/// radio and queue (as [`widgets::artist_rows`]), and for anyone
/// «Не рекомендовать исполнителя».
pub fn stream_artist_rows(a: &StreamArtistCard, marks: &nikgob_view::DislikesView) -> Vec<widgets::Row<StreamArtistAct>> {
    use StreamArtistAct as A;
    use widgets::Row;
    let mut rows = Vec::new();
    if a.artist.is_some() {
        rows.push(Row::Item(A::Play, MenuItem::new("Слушать микс").icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new("Перемешать").icon(Icon::Shuffle)));
        rows.push(Row::Item(A::Radio, MenuItem::new("Радио исполнителя").icon(Icon::Radio)));
        rows.push(Row::Item(A::Queue, MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic)));
        rows.push(Row::Divider);
    }
    let ban = match a.artist {
        Some(id) => super::ban_item(id, marks),
        None => MenuItem::new("Не рекомендовать исполнителя").icon(Icon::Block),
    };
    rows.push(Row::Item(A::Ban, ban));
    rows
}

fn stream_artist_command(act: StreamArtistAct, a: &StreamArtistCard, marks: &nikgob_view::DislikesView) -> Command {
    use StreamArtistAct as A;
    match (act, a.artist) {
        (A::Play, Some(id)) => Command::PlayArtist { id, shuffle: false },
        (A::Shuffle, Some(id)) => Command::PlayArtist { id, shuffle: true },
        (A::Radio, Some(id)) => Command::StartRadio { seed: nikgob_view::RadioSeed::Artist(id) },
        (A::Queue, Some(id)) => Command::EnqueueArtist(id),
        (A::Ban, Some(id)) => super::ban_command(id, marks),
        // Not in the library: the name on every service.
        _ => Command::RecFeedback { target: nikgob_view::RecTarget::StreamArtist(a.name.clone()), dislike: true },
    }
}

fn common_line(ui: &Ui, text: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), TypeRole::TitleSmall.style().text_format(s.on_surface));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    job.halign = egui::Align::LEFT;
    ui.painter().layout_job(job)
}

/// What the menu of a service's playlist does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamPlaylistAct {
    Play,
    Shuffle,
    Next,
    Queue,
    Playlist,
    Follow,
    Library,
}

/// The one list of a service playlist's actions, in the order of the
/// library playlist's ([`widgets::playlist_rows`]): its card's right click
/// and ⋮ and the ⋮ of its page. The page leaves out its buttons (play,
/// shuffle, «В библиотеку»); playing next, the queue and «В плейлист…»
/// need the tracks only the page has.
pub fn stream_playlist_rows(on_page: bool, followed: bool) -> Vec<widgets::Row<StreamPlaylistAct>> {
    use StreamPlaylistAct as A;
    use widgets::Row;
    let mut rows = Vec::new();
    if on_page {
        rows.push(Row::Item(A::Next, MenuItem::new(super::PLAY_NEXT).icon(Icon::QueueMusic)));
        rows.push(Row::Item(A::Queue, MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic)));
        rows.push(Row::Divider);
        rows.push(Row::Item(A::Playlist, MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd)));
    } else {
        rows.push(Row::Item(A::Play, MenuItem::new("Слушать").icon(Icon::PlayArrow)));
        rows.push(Row::Item(A::Shuffle, MenuItem::new("Перемешать").icon(Icon::Shuffle)));
        rows.push(Row::Divider);
        rows.push(if followed {
            Row::Item(A::Library, MenuItem::new("Открыть в библиотеке").icon(Icon::LibraryMusic))
        } else {
            Row::Item(A::Follow, MenuItem::new("В библиотеку").icon(Icon::LibraryMusic))
        });
    }
    rows
}

/// A playlist of a service as a card: opens its page, ▶ plays it; right
/// click and ⋮: [`stream_playlist_rows`].
pub fn playlist_card(ui: &mut Ui, p: &StreamPlaylistCard, width: f32, row: &str, out: &mut Vec<Command>) -> Option<Route> {
    let mut sub = p.owner.clone().unwrap_or_default();
    if let Some(n) = p.total_tracks {
        let n = u64::from(n);
        let count = format!("{n} {}", common::plural(n, "трек", "трека", "треков"));
        sub = if sub.is_empty() { count } else { format!("{sub} · {count}") };
    }
    let o = MediaCard::new(&p.name)
        .subtitle(sub)
        .cover_opt(p.cover_url.clone())
        .placeholder_icon(Icon::QueueMusic)
        .menu_button(true)
        .width(width)
        .id_salt(("st-playlist", row, &p.remote))
        .show(ui);
    let mut nav = None;
    let mut act = None;
    if let Some(m) = &o.menu {
        let (menu, acts) = widgets::menu_of(("st-pl-more", m.id), stream_playlist_rows(false, p.playlist.is_some()));
        act = menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied());
    }
    if !Menu::context_idle(&o.response, ("st-pl-menu", o.response.id)) {
        let (menu, acts) = widgets::menu_of(("st-pl-menu", o.response.id), stream_playlist_rows(false, p.playlist.is_some()));
        act = act.or(menu.context_menu(&o.response).and_then(|i| acts.get(i).copied()));
    }
    match act {
        Some(StreamPlaylistAct::Play) => out.push(Command::PlayStreamPlaylist { id: p.remote.clone(), start: None, shuffle: false }),
        Some(StreamPlaylistAct::Shuffle) => out.push(Command::PlayStreamPlaylist { id: p.remote.clone(), start: None, shuffle: true }),
        Some(StreamPlaylistAct::Follow) => out.push(Command::FollowStreamPlaylist(p.remote.clone())),
        Some(StreamPlaylistAct::Library) => nav = p.playlist.map(Route::Playlist),
        Some(StreamPlaylistAct::Next | StreamPlaylistAct::Queue | StreamPlaylistAct::Playlist) | None => {}
    }
    if o.play_clicked {
        out.push(Command::PlayStreamPlaylist { id: p.remote.clone(), start: None, shuffle: false });
        None
    } else if o.card_clicked {
        Some(Route::Stream(StreamPage::Playlist(p.remote.clone())))
    } else {
        nav
    }
}

// ---------------------------------------------------------------- pages

/// The album / playlist / artist page of a service.
pub fn page(ui: &mut Ui, page: &StreamPage, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let view = state.stream_page.as_ref().filter(|p| &p.page == page);
    let title = view.map_or("", |v| v.title.as_str());
    super::detail::with_bar(ui, title, pages, |ui, pages| {
        let Some(v) = view.filter(|v| !v.loading) else {
            ui.add(LoadingIndicator::new());
            return None;
        };
        if let Some(e) = &v.error {
            widgets::empty_state(ui, Icon::Error, "Не открылось", e, None);
            ui.vertical_centered(|ui| {
                if ui.add(Button::new("Повторить").style(Style::Text).icon(Icon::Refresh).id_salt("st-page-retry")).clicked() {
                    out.push(Command::LoadStream(page.clone()));
                }
            });
            return None;
        }
        match page {
            StreamPage::Album(id) => album_page(ui, id, v, state, pages, out),
            StreamPage::Playlist(id) => playlist_page(ui, id, v, state, pages, out),
            StreamPage::Artist(id) => artist_page(ui, id, v, &state.downloads.wishlist, out),
        }
    })
}

fn type_label(t: Option<ReleaseType>) -> &'static str {
    match t {
        Some(ReleaseType::Single) => "СИНГЛ",
        Some(ReleaseType::Ep) => "EP",
        Some(ReleaseType::Compilation) => "СБОРНИК",
        _ => "АЛЬБОМ",
    }
}

/// The header: a big web cover and the text column beside it.
fn header(ui: &mut Ui, v: &StreamPageView, round: bool, add_text: impl FnOnce(&mut Ui)) {
    let size = if ui.available_width() > 720.0 { 232.0 } else { 160.0 };
    ui.add_space(8.0);
    ui.horizontal_top(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
        if round {
            let shape = if v.cover_url.is_some() {
                nikgob_shapes::MaterialShape::Circle
            } else {
                nikgob_shapes::MaterialShape::Cookie12Sided
            };
            widgets::shape_image(ui, v.cover_url.as_deref(), &widgets::initials(&v.title), rect, shape);
        } else {
            crate::components::paint_image(
                ui,
                rect,
                v.cover_url.as_deref(),
                CornerRadius::same(shape::EXTRA_LARGE as u8),
                if matches!(v.page, StreamPage::Playlist(_)) { Icon::QueueMusic } else { Icon::Album },
            );
        }
        ui.add_space(24.0);
        ui.vertical(|ui| {
            ui.set_min_height(size);
            add_text(ui);
        });
    });
}

/// "Кино · 8 треков · 37 мин".
fn count_line(lead: &str, n: usize, total: std::time::Duration) -> String {
    let n = n as u64;
    let mut parts = Vec::new();
    if !lead.is_empty() {
        parts.push(lead.to_owned());
    }
    parts.push(format!("{n} {}", common::plural(n, "трек", "трека", "треков")));
    if !total.is_zero() {
        parts.push(common::fmt_total(total));
    }
    parts.join(" · ")
}

/// The download button of a service's album page: what «Хочу скачать»
/// says about the album, and whether a click downloads it.
fn album_download(v: &StreamPageView, wish: &widgets::WishIndex, all_local: bool, torrents: bool) -> (&'static str, bool) {
    if all_local {
        return ("Скачан", false);
    }
    match v.release.and_then(|r| wish.of_release(r)) {
        None | Some(WishlistStatus::Waiting | WishlistStatus::Dismissed | WishlistStatus::Done) => ("Скачать альбом", true),
        Some(WishlistStatus::Downloading) => ("Качается", false),
        Some(WishlistStatus::NoTorrent | WishlistStatus::Failed) => ("Не нашлось", false),
        Some(_) if !torrents => ("Ждёт торренты", false),
        Some(_) => ("В очереди", false),
    }
}

fn album_page(
    ui: &mut Ui,
    id: &RemoteId,
    v: &StreamPageView,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut nav = None;
    let local = v.tracks.iter().filter(|t| t.has_local).count();
    let artists = artist_links::stream(&v.subtitle_artists);
    header(ui, v, false, |ui| {
        let when = crate::screens::common::fmt_album_date(v.released, v.year, state.prefs.album_date)
            .map(|w| w.to_uppercase())
            .unwrap_or_default();
        overline(ui, &[type_label(v.release_type), &when, service_upper(id.service)]);
        // How much of it is here, in the same line as its size
        // («8 из 10 у тебя»: counted over the page's own rows).
        let mut line = count_line(&v.subtitle, v.tracks.len(), v.total_duration);
        if let Some(have) = v.have.files_label() {
            line = format!("{line} · {have}");
        }
        // Each artist of the album is a link to its page.
        let (links, routes) = artist_links::stream_spans(&v.subtitle_artists, &line);
        let format = TypeRole::TitleMedium.style().text_format(s.on_surface_variant);
        if let Some(k) = crate::components::link_paragraph(ui, "st-album-artists", &line, &links, format).clicked {
            nav = routes.get(k).cloned();
        }
        // The linked library album, with its own count: «В библиотеке:
        // Ugly Is Beautiful · 13 из 14».
        if let Some(r) = v.release.filter(|_| v.have.in_library > 0 || v.library.as_ref().is_some_and(|l| l.1.in_library > 0)) {
            let label = match &v.library {
                Some((title, h)) if h.local > 0 && !h.complete() => format!("В библиотеке: {title} · {} из {}", h.local, h.total),
                Some((title, _)) => format!("В библиотеке: {title}"),
                None => "Открыть в библиотеке".to_owned(),
            };
            ui.add_space(8.0);
            if ui.add(AssistChip::new(label).icon(Icon::LibraryMusic).id_salt("st-chip-lib")).clicked() {
                nav = Some(Route::Release(r));
            }
        }
    });
    ui.add_space(16.0);
    let remotes: Vec<RemoteId> = v.tracks.iter().map(|t| t.remote.clone()).collect();
    let all_local = v.have.complete() || (!v.tracks.is_empty() && local == v.tracks.len());
    let wish = widgets::WishIndex::of(ui.ctx(), &state.downloads.wishlist);
    let (dl_label, dl_can) = album_download(v, &wish, all_local, state.prefs.prowlarr_configured);
    let members: Vec<Member> = vec![
        Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
        widgets::shuffle_member(ui),
        Button::new(dl_label).icon(Icon::Download).size(Size::M).style(Style::Tonal).enabled(!all_local).into(),
        IconButton::new(Icon::Favorite)
            .size(Size::M)
            .style(Style::Tonal)
            .selected(v.liked)
            .tooltip(super::like_label(v.liked))
            .into(),
        IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into(),
    ];
    let row = ButtonRow::new("st-album-actions", Size::M).show(ui, members);
    let r = &row.responses;
    if r[0].clicked() {
        out.push(Command::PlayStreamAlbum { id: id.clone(), start: None, shuffle: false });
    }
    if r[1].clicked() {
        out.push(Command::PlayStreamAlbum { id: id.clone(), start: None, shuffle: true });
    }
    // «Скачать» downloads without a like; once on its way, the button says
    // how far it is and opens «Хочу скачать».
    let dl = r[2].clone().on_hover_text(if dl_can {
        "Весь альбом — в «Хочу скачать» (без лайка)"
    } else {
        "Открыть «Хочу скачать»"
    });
    if dl.clicked() {
        if dl_can {
            out.push(Command::DownloadStreamAlbum(id.clone()));
        } else {
            nav = Some(Route::Downloads(nikgob_view::DownloadsTab::Wishlist));
        }
    }
    if r[3].clicked() {
        out.push(Command::LikeStreamAlbum { album: id.clone(), on: !v.liked });
    }
    let more = &r[4];
    let of = StreamAlbumMenuOf { on_page: true, has_artists: !artists.is_empty(), in_library: v.release.is_some(), can_download: dl_can };
    let (menu, acts) = widgets::menu_of("st-album-more", stream_album_rows(of));
    match menu.attach(more).and_then(|i| acts.get(i).copied()) {
        Some(StreamAlbumAct::Next) => out.push(Command::PlayStreamNext(remotes.clone())),
        Some(StreamAlbumAct::Queue) => out.push(Command::EnqueueStream(remotes.clone())),
        Some(StreamAlbumAct::Playlist) => pages.add_stream_to_playlist(remotes.clone(), Some(more.rect)),
        Some(act) => nav = stream_album_apply(ui.ctx(), act, id, &v.subtitle_artists, v.release, out).or(nav),
        None => {}
    }
    ui.add_space(16.0);
    let rows = track_rows(ui, "st-album", &v.tracks, true, &StreamFrom::Album(id.clone()), state, pages, out);
    rows.or(nav)
}

fn playlist_page(
    ui: &mut Ui,
    id: &RemoteId,
    v: &StreamPageView,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut nav = None;
    // (Once followed, «✓ В библиотеке» below opens the library playlist.)
    header(ui, v, false, |ui| {
        overline(ui, &["ПЛЕЙЛИСТ", service_upper(id.service)]);
        ui.label(TypeRole::TitleMedium.style().rich(count_line(&v.subtitle, v.tracks.len(), v.total_duration), s.on_surface_variant));
    });
    ui.add_space(16.0);
    let remotes: Vec<RemoteId> = v.tracks.iter().map(|t| t.remote.clone()).collect();
    let followed = v.playlist.is_some();
    let row = ButtonRow::new("st-pl-actions", Size::M).show(
        ui,
        vec![
            Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
            widgets::shuffle_member(ui),
            Button::new(if followed { "В библиотеке" } else { "В библиотеку" })
                .icon(if followed { Icon::Check } else { Icon::LibraryMusic })
                .size(Size::M)
                .style(Style::Outlined)
                .selected(followed)
                .into(),
            IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Tonal).tooltip("Ещё").into(),
        ],
    );
    let r = &row.responses;
    if r[0].clicked() {
        out.push(Command::PlayStreamPlaylist { id: id.clone(), start: None, shuffle: false });
    }
    if r[1].clicked() {
        out.push(Command::PlayStreamPlaylist { id: id.clone(), start: None, shuffle: true });
    }
    let follow = r[2].clone().on_hover_text(if followed {
        "Плейлист в библиотеке и следит за сервисом — открыть"
    } else {
        "Плейлист в библиотеку: он будет следить за сервисом"
    });
    if follow.clicked() {
        match v.playlist {
            Some(p) => nav = Some(Route::Playlist(p)),
            None => out.push(Command::FollowStreamPlaylist(id.clone())),
        }
    }
    let (menu, acts) = widgets::menu_of("st-pl-more", stream_playlist_rows(true, followed));
    match menu.attach(&r[3]).and_then(|i| acts.get(i).copied()) {
        Some(StreamPlaylistAct::Next) => out.push(Command::PlayStreamNext(remotes.clone())),
        Some(StreamPlaylistAct::Queue) => out.push(Command::EnqueueStream(remotes.clone())),
        Some(StreamPlaylistAct::Playlist) => pages.add_stream_to_playlist(remotes.clone(), Some(r[3].rect)),
        _ => {}
    }
    ui.add_space(16.0);
    if v.tracks.is_empty() {
        widgets::empty_state(ui, Icon::QueueMusic, "Плейлист пуст", "", None);
        return nav;
    }
    track_rows(ui, "st-playlist", &v.tracks, false, &StreamFrom::Playlist(id.clone()), state, pages, out).or(nav)
}

fn artist_page(
    ui: &mut Ui,
    id: &RemoteId,
    v: &StreamPageView,
    wishlist: &WishlistView,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut nav = None;
    header(ui, v, true, |ui| {
        overline(ui, &["ИСПОЛНИТЕЛЬ", service_upper(id.service)]);
        let n = v.albums.len() as u64;
        let mut line = format!("{n} {}", common::plural(n, "релиз", "релиза", "релизов"));
        let have: u32 = v.albums.iter().map(|a| a.have.local).sum();
        if have > 0 {
            line = format!("{line} · {have} {} у тебя", common::plural(u64::from(have), "трек", "трека", "треков"));
        }
        ui.label(TypeRole::TitleMedium.style().rich(line, s.on_surface_variant));
    });
    ui.add_space(16.0);
    let newest = v.albums.first().map(|a| a.remote.clone());
    let mut members: Vec<Member> = vec![
        Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).enabled(newest.is_some()).into(),
    ];
    if v.artist.is_some() {
        // The library knows the artist: a radio of its tracks and the
        // like (an artist only on the service has nothing to grow from).
        members.push(Button::new("Радио").icon(Icon::Radio).size(Size::M).style(Style::Tonal).into());
        members.push(
            Button::new(if v.liked { "В подписках" } else { "Подписаться" })
                .icon(Icon::Favorite)
                .size(Size::M)
                .style(Style::Outlined)
                .selected(v.liked)
                .into(),
        );
        members.push(Button::new("В библиотеке").icon(Icon::LibraryMusic).size(Size::M).style(Style::Tonal).into());
    }
    let row = ButtonRow::new("st-ar-actions", Size::M).show(ui, members);
    let r = &row.responses;
    if r[0].clone().on_hover_text("Самый новый релиз").clicked()
        && let Some(a) = newest
    {
        out.push(Command::PlayStreamAlbum { id: a, start: None, shuffle: false });
    }
    if let Some(a) = v.artist {
        if r[1].clone().on_hover_text("Его треки и похожее — очередь дописывается сама").clicked() {
            out.push(Command::StartRadio { seed: nikgob_view::RadioSeed::Artist(a) });
        }
        if r[2].clicked() {
            out.push(Command::Like(LikeTarget::Artist(a), !v.liked));
        }
        if r[3].clicked() {
            nav = Some(Route::Artist(a));
        }
    }
    ui.add_space(widgets::SECTION_GAP);
    let (albums, singles): (Vec<&StreamAlbumCard>, Vec<&StreamAlbumCard>) =
        v.albums.iter().partition(|a| matches!(a.release_type, ReleaseType::Album | ReleaseType::Compilation));
    for (title, list) in [("Альбомы", albums), ("Синглы и EP", singles)] {
        if list.is_empty() {
            continue;
        }
        widgets::section_header(ui, title, false);
        nav = widgets::grid(ui, &list, widgets::CARD_W, |_, w| MediaCard::height(w), |ui, a, w| album_card(ui, a, w, title, wishlist, out))
            .or(nav);
        ui.add_space(widgets::SECTION_GAP);
    }
    if v.albums.is_empty() {
        widgets::empty_state(ui, Icon::Album, "Релизов нет", "", None);
    }
    nav
}

#[cfg(test)]
mod menu_tests {
    use super::{StreamAlbumAct, StreamAlbumMenuOf, StreamPlaylistAct, stream_album_rows, stream_playlist_rows};
    use crate::screens::pages::widgets::{Row, row_acts};

    fn labels<A>(rows: &[Row<A>]) -> Vec<String> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Item(_, i) => Some(i.label.clone()),
                Row::Divider | Row::Label(_) | Row::Sub(..) => None,
            })
            .collect()
    }

    /// A service album's right click and its page's ⋮ are one list in the
    /// library album's order: the card adds play / shuffle / the like /
    /// «Скачать альбом»; both play next, queue and add to a playlist. What
    /// the card's click does («Открыть») is not in it.
    #[test]
    fn a_service_album_offers_the_same_actions_by_right_click_and_by_more() {
        use StreamAlbumAct as A;
        let of = StreamAlbumMenuOf { has_artists: true, in_library: true, can_download: true, ..Default::default() };
        let card = row_acts(&stream_album_rows(of));
        let page = row_acts(&stream_album_rows(StreamAlbumMenuOf { on_page: true, ..of }));
        let shared = |acts: &[A]| -> Vec<A> {
            acts.iter().copied().filter(|a| !matches!(a, A::Play | A::Shuffle | A::Like | A::Download)).collect()
        };
        assert_eq!(shared(&card), shared(&page));
        assert_eq!(shared(&card), [A::Next, A::Queue, A::Playlist, A::Artist, A::Library, A::Share]);
        assert_eq!(card, [A::Play, A::Shuffle, A::Next, A::Queue, A::Like, A::Playlist, A::Artist, A::Library, A::Share, A::Download]);
        assert_eq!(&page[..3], [A::Next, A::Queue, A::Playlist]);
        assert_eq!(&labels(&stream_album_rows(of))[2..6], ["Сыграть следующим", "В очередь", "Нравится", "В плейлист…"]);
        assert_eq!(labels(&stream_album_rows(of)).last().map(String::as_str), Some("Скачать альбом"));
        // Not linked, nothing to download, no artist known: none of those.
        let bare = row_acts(&stream_album_rows(StreamAlbumMenuOf::default()));
        assert!(!bare.iter().any(|a| matches!(a, A::Library | A::Download | A::Artist)));
        assert!(bare.contains(&A::Like), "liking is not downloading");
    }

    /// A service playlist's right click and its page's ⋮: one list, the
    /// same names as the library playlist's.
    #[test]
    fn a_service_playlist_offers_its_actions_by_right_click_and_by_more() {
        use StreamPlaylistAct as A;
        assert_eq!(row_acts(&stream_playlist_rows(false, false)), [A::Play, A::Shuffle, A::Follow]);
        assert_eq!(row_acts(&stream_playlist_rows(false, true)), [A::Play, A::Shuffle, A::Library]);
        assert_eq!(row_acts(&stream_playlist_rows(true, true)), [A::Next, A::Queue, A::Playlist]);
        assert_eq!(labels(&stream_playlist_rows(true, false)), ["Сыграть следующим", "В очередь", "В плейлист…"]);
    }
}
