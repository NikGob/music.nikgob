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
    if !numbered && let Some(a) = &t.album {
        supporting = format!("{supporting} · {a}");
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
    // Each artist of the line is a link to its page in the service.
    let (links, artist_routes) = artist_links::stream_spans(&t.artists, &supporting);
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
                            .tooltip(if t.liked { "Убрать из Любимых" } else { "В Любимые" })
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
        let picked = track_menu(egui::Id::new(("st-menu", id, i)), t, wished).attach(m);
        nav = track_pick(&m.ctx, picked, t, Some(m.rect), pages, out);
    }
    let picked = track_menu(egui::Id::new(("st-menu-ctx", id, i)), t, wished).context_menu(&o.response);
    let nav = track_pick(&o.response.ctx, picked, t, None, pages, out).or(nav);
    // An artist name of the line: its page.
    o.link_clicked.and_then(|k| artist_routes.get(k).cloned()).or(nav)
}

fn track_menu(key: egui::Id, t: &StreamTrackRow, wished: Option<WishlistStatus>) -> Menu {
    let (download, can) = match (t.has_local, wished) {
        (true, _) => ("Уже скачан", false),
        (false, Some(WishlistStatus::Waiting | WishlistStatus::NoTorrent | WishlistStatus::Failed)) => ("Скачать сейчас", true),
        (false, Some(_)) => ("Уже в «Хочу скачать»", false),
        (false, None) => ("Скачать", true),
    };
    Menu::new(key)
        .align_right(true)
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .item(MenuItem::new(super::RADIO_BY_TRACK).icon(Icon::Radio))
        .divider()
        .item(MenuItem::new("К альбому").icon(Icon::Album).enabled(t.album_id.is_some()))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(!t.artists.linked().is_empty()))
        .item(MenuItem::new("Поделиться").icon(Icon::Share))
        .divider()
        .item(MenuItem::new(download).icon(Icon::Download).enabled(can))
        .item(MenuItem::new("Скачать альбом").icon(Icon::Download).enabled(t.album_id.is_some()))
}

fn track_pick(
    ctx: &egui::Context,
    picked: Option<usize>,
    t: &StreamTrackRow,
    anchor: Option<Rect>,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    match picked? {
        0 => out.push(Command::PlayStreamNext(vec![t.remote.clone()])),
        1 => out.push(Command::EnqueueStream(vec![t.remote.clone()])),
        2 => pages.add_stream_to_playlist(vec![t.remote.clone()], anchor),
        3 => out.push(radio_by_stream(t)),
        4 => return t.album_id.clone().map(|a| Route::Stream(StreamPage::Album(a))),
        // Several artists: a chooser.
        5 => return artist_links::go_to(ctx, artist_links::stream(&t.artists)),
        6 => out.push(Command::Share(ShareTarget::StreamTrack(t.remote.clone()))),
        // Download without a like: the like stays the ♡'s.
        7 => out.push(Command::DownloadStream { tracks: vec![t.remote.clone()] }),
        // The whole album the track is from, without a like.
        8 => {
            if let Some(a) = t.album_id.clone() {
                out.push(Command::DownloadStreamAlbum(a));
            }
        }
        _ => {}
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
/// and ⋮; right click or ⋮: Слушать · Скачать · В Любимые · Поделиться ·
/// Открыть. The cover corner shows its download status ([`album_status`]).
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
    let mut nav = None;
    if let Some(m) = &o.menu {
        nav = album_pick(album_menu(("st-album-more", m.id), a, can_download).align_right(true).attach(m), a, out);
    }
    nav = album_pick(album_menu(("st-album-menu", o.response.id), a, can_download).context_menu(&o.response), a, out)
        .or(nav);
    if o.play_clicked {
        out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false });
        None
    } else if o.card_clicked {
        Some(Route::Stream(StreamPage::Album(a.remote.clone())))
    } else {
        nav
    }
}

fn album_menu(key: impl egui::AsId, a: &StreamAlbumCard, can_download: bool) -> Menu {
    Menu::new(key)
        .item(MenuItem::new("Слушать").icon(Icon::PlayArrow))
        .item(MenuItem::new("Скачать").icon(Icon::Download).enabled(can_download))
        .item(MenuItem::new("В Любимые").icon(Icon::Favorite).enabled(!a.have.complete()))
        .divider()
        .item(MenuItem::new("Поделиться").icon(Icon::Share))
        .item(MenuItem::new("Открыть").icon(Icon::Album))
}

fn album_pick(picked: Option<usize>, a: &StreamAlbumCard, out: &mut Vec<Command>) -> Option<Route> {
    match picked? {
        0 => out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false }),
        // «Скачать» downloads without a like; «В Любимые» is the like (which
        // also downloads, as the settings say).
        1 => out.push(Command::DownloadStreamAlbum(a.remote.clone())),
        2 => out.push(Command::LikeStreamAlbum { album: a.remote.clone(), on: true }),
        3 => out.push(Command::Share(ShareTarget::StreamAlbum(a.remote.clone()))),
        4 => return Some(Route::Stream(StreamPage::Album(a.remote.clone()))),
        _ => {}
    }
    None
}

/// An artist of a service: the photo (or initials) in a shape, the name.
/// A library artist opens the library page, anyone else the service's.
pub fn artist_card(ui: &mut Ui, a: &StreamArtistCard, size: f32) -> Option<Route> {
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
    if resp.hovered() {
        ui.painter().circle_filled(img.center(), size * 0.5, s.on_surface.gamma_multiply(0.06));
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.clicked().then(|| match a.artist {
        Some(id) => Route::Artist(id),
        None => Route::Stream(StreamPage::Artist(a.remote.clone())),
    })
}

fn common_line(ui: &Ui, text: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), TypeRole::TitleSmall.style().text_format(s.on_surface));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    job.halign = egui::Align::LEFT;
    ui.painter().layout_job(job)
}

/// A playlist of a service as a card: opens its page, ▶ plays it.
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
        .width(width)
        .id_salt(("st-playlist", row, &p.remote))
        .show(ui);
    if o.play_clicked {
        out.push(Command::PlayStreamPlaylist { id: p.remote.clone(), start: None, shuffle: false });
        None
    } else {
        o.card_clicked.then(|| Route::Stream(StreamPage::Playlist(p.remote.clone())))
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
        None | Some(WishlistStatus::Waiting | WishlistStatus::Dismissed | WishlistStatus::Done) => ("Скачать", true),
        Some(WishlistStatus::Downloading) => ("Качается", false),
        Some(WishlistStatus::NoTorrent | WishlistStatus::Failed) => ("Нет раздачи", false),
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
        let year = v.year.map(|y| y.to_string()).unwrap_or_default();
        overline(ui, &[type_label(v.release_type), &year, service_upper(id.service)]);
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
            .tooltip(if v.liked { "Убрать альбом из Любимых" } else { "Альбом в Любимые" })
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
    let picked = Menu::new("st-album-more")
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .divider()
        .item(MenuItem::new("Поделиться").icon(Icon::Share))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(!artists.is_empty()))
        .item(MenuItem::new("Открыть в библиотеке").icon(Icon::LibraryMusic).enabled(v.release.is_some()))
        .attach(more);
    match picked {
        Some(0) => out.push(Command::PlayStreamNext(remotes.clone())),
        Some(1) => out.push(Command::EnqueueStream(remotes.clone())),
        Some(2) => pages.add_stream_to_playlist(remotes.clone(), Some(more.rect)),
        Some(3) => out.push(Command::Share(ShareTarget::StreamAlbum(id.clone()))),
        Some(4) => nav = artist_links::go_to(ui.ctx(), artists.clone()),
        Some(5) => nav = v.release.map(Route::Release),
        _ => {}
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
    let picked = Menu::new("st-pl-more")
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
        .item(MenuItem::new("Треки в свой плейлист…").icon(Icon::PlaylistAdd))
        .attach(&r[3]);
    match picked {
        Some(0) => out.push(Command::PlayStreamNext(remotes.clone())),
        Some(1) => out.push(Command::EnqueueStream(remotes.clone())),
        Some(2) => pages.add_stream_to_playlist(remotes.clone(), Some(r[3].rect)),
        _ => {}
    }
    ui.add_space(16.0);
    if v.tracks.is_empty() {
        widgets::empty_state(ui, Icon::QueueMusic, "Плейлист пуст", "В сервисе в нём пока ничего нет.", None);
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
        widgets::empty_state(ui, Icon::Album, "Релизов нет", "Сервис не знает релизов этого исполнителя.", None);
    }
    nav
}
