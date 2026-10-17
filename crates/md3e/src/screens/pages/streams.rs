//! What the streaming services show (docs/stage3.md §3.2–3.4): their
//! tracks, albums, artists and playlists as rows and cards with a source
//! badge, and the album / playlist / artist page of a service
//! ([`Route::Stream`]), laid out like the library's release, playlist and
//! artist pages.

use egui::{Color32, CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_view::{
    Command, LikeTarget, ReleaseType, RemoteId, Route, Service, ShareTarget, Status, StreamAlbumCard,
    StreamArtistCard, StreamPage, StreamPageView, StreamPlaylistCard, StreamTrackRow, TrackKind, ViewState,
};

use super::{PageState, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        AssistChip, Button, ButtonRow, Equalizer, IconButton, Leading, ListItem, LoadingIndicator, MediaCard,
        Member, Menu, MenuItem, Radii, Size, Style, badge, paint,
    },
    screens::common,
    tokens::shape,
};

/// The service's own colour, for its badge (Spotify green, YouTube red).
pub fn service_color(service: Service) -> Color32 {
    match service {
        Service::Spotify => Color32::from_rgb(0x1E, 0xD7, 0x60),
        Service::Youtube => Color32::from_rgb(0xFF, 0x33, 0x33),
    }
}

/// "Spotify", "YouTube" (short, for chips and badges).
pub fn service_short(service: Service) -> &'static str {
    match service {
        Service::Spotify => "Spotify",
        Service::Youtube => "YouTube",
    }
}

/// A round badge with the service's mark on the top-right corner of a cover.
pub fn service_badge(ui: &Ui, cover: Rect, service: Service) {
    let s = Theme::get(ui.ctx()).scheme;
    let size = (cover.width() * 0.16).clamp(16.0, 24.0);
    let c = pos2(cover.right() - size * 0.5 - 6.0, cover.top() + size * 0.5 + 6.0);
    ui.painter().circle_filled(c, size * 0.5 + 1.5, s.surface_container_highest);
    let icon = match service {
        Service::Spotify => Icon::GraphicEq,
        Service::Youtube => Icon::PlayArrow,
    };
    icon.widget(true, service_color(service), size * 0.72)
        .paint_at(ui, Rect::from_center_size(c, vec2(size * 0.72, size * 0.72)));
}

/// A small pill with the service's dot and name, laid out inline.
pub fn service_pill(ui: &mut Ui, service: Service) -> egui::Response {
    let s = Theme::get(ui.ctx()).scheme;
    let text = service_short(service);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), TypeRole::LabelMedium.font_id(), s.on_surface_variant);
    let size = vec2(galley.size().x + 26.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint::container(ui.painter(), rect, Radii::all(shape::FULL), s.surface_container_highest, Stroke::NONE);
    ui.painter().circle_filled(pos2(rect.left() + 11.0, rect.center().y), 4.0, service_color(service));
    ui.painter().galley(pos2(rect.left() + 19.0, rect.center().y - galley.size().y * 0.5), galley, s.on_surface_variant);
    resp
}

/// A small tonal mark ("стрим", "клип", "у тебя").
pub fn mark(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    let g = ui.painter().layout_no_wrap(text.to_owned(), TypeRole::LabelSmall.font_id(), s.on_secondary_container);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 10.0, badge::LARGE_HEIGHT), Sense::hover());
    badge::text_badge(ui.painter(), rect.left_center(), text, s.secondary_container, s.on_secondary_container);
}

/// Where a list of stream tracks plays from.
#[derive(Clone, Debug)]
pub enum StreamFrom {
    /// The rows themselves, as a queue named `context`.
    Rows(String),
    Album(RemoteId),
    Playlist(RemoteId),
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
    }
}

/// Rows of stream tracks: cover (or number), title, artist · album, marks,
/// duration, like and ⋮. Double click plays from the row.
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
    let s = Theme::get(ui.ctx()).scheme;
    let mut nav = None;
    let current = state.player.current.as_ref().map(|t| t.id);
    let playing = state.player.status == Status::Playing;
    // An album by one artist needs no artist line under every title.
    let two_line = !numbered || rows.first().is_some_and(|f| rows.iter().any(|t| t.artist != f.artist));
    for (i, t) in rows.iter().enumerate() {
        let is_cur = t.track.is_some() && t.track == current;
        let leading = if numbered {
            Leading::Number(if is_cur { String::new() } else if t.track_no > 0 { t.track_no.to_string() } else { (i + 1).to_string() })
        } else {
            Leading::Image(t.cover_url.clone(), Icon::MusicNote)
        };
        let mut supporting = t.artist.clone();
        if !numbered && let Some(a) = &t.album {
            supporting = format!("{supporting} · {a}");
        }
        let dur = t.duration.map(common::fmt_time).unwrap_or_default();
        let mut menu_resp = None;
        let mut item = ListItem::new(&t.title);
        if two_line {
            item = item.supporting(supporting);
        }
        let o = item
            .leading(leading)
            .selected(is_cur)
            .muted(!t.playable)
            .trailing(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    menu_resp = Some(ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("st-more", id, i))));
                    if ui
                        .add(
                            IconButton::new(Icon::Favorite)
                                .size(Size::Xs)
                                .selected(t.liked)
                                .tooltip(if t.liked { "Убрать из Любимых" } else { "Нравится — и в «Хочу скачать»" })
                                .id_salt(("st-like", id, i)),
                        )
                        .clicked()
                    {
                        out.push(Command::LikeStream { track: t.remote.clone(), on: !t.liked });
                    }
                    ui.label(TypeRole::BodyMedium.style().rich(dur, s.on_surface_variant));
                    if t.has_local {
                        mark(ui, "у тебя");
                    }
                    if t.kind == TrackKind::Video {
                        mark(ui, "клип");
                    }
                    if !t.playable {
                        ui.label(TypeRole::LabelSmall.style().rich("недоступен", s.on_surface_variant));
                    }
                    for &other in &t.also_on {
                        ui.label(TypeRole::LabelSmall.style().rich(format!("+ {}", service_short(other)), s.on_surface_variant))
                            .on_hover_text(format!("Тот же трек есть в {}", other.display_name()));
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
        if o.response.double_clicked() && t.playable {
            out.push(if is_cur { Command::TogglePlay } else { play_from(from, rows, i) });
        }
        if let Some(m) = &menu_resp {
            let picked = track_menu(egui::Id::new(("st-menu", id, i)), t).attach(m);
            nav = track_pick(picked, t, Some(m.rect), pages, out).or(nav);
        }
        let picked = track_menu(egui::Id::new(("st-menu-ctx", id, i)), t).context_menu(&o.response);
        nav = track_pick(picked, t, None, pages, out).or(nav);
    }
    nav
}

fn track_menu(key: egui::Id, t: &StreamTrackRow) -> Menu {
    Menu::new(key)
        .align_right(true)
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .divider()
        .item(MenuItem::new("К альбому").icon(Icon::Album).enabled(t.album_id.is_some()))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(t.artist_id.is_some()))
        .item(MenuItem::new("Поделиться").icon(Icon::Share))
        .divider()
        .item(MenuItem::new(if t.has_local { "Уже скачан" } else { "Лайк и скачать" }).icon(Icon::Download).enabled(!t.has_local && !t.liked))
}

fn track_pick(picked: Option<usize>, t: &StreamTrackRow, anchor: Option<Rect>, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    match picked? {
        0 => out.push(Command::PlayStreamNext(vec![t.remote.clone()])),
        1 => out.push(Command::EnqueueStream(vec![t.remote.clone()])),
        2 => pages.add_stream_to_playlist(vec![t.remote.clone()], anchor),
        3 => return t.album_id.clone().map(|a| Route::Stream(StreamPage::Album(a))),
        4 => return t.artist_id.clone().map(|a| Route::Stream(StreamPage::Artist(a))),
        5 => out.push(Command::Share(ShareTarget::StreamTrack(t.remote.clone()))),
        6 => out.push(Command::LikeStream { track: t.remote.clone(), on: true }),
        _ => {}
    }
    None
}

/// "Артист · 1988".
fn album_subtitle(a: &StreamAlbumCard) -> String {
    match a.year {
        Some(y) if !a.artist.is_empty() => format!("{} · {y}", a.artist),
        Some(y) => y.to_string(),
        None => a.artist.clone(),
    }
}

/// An album of a service as a card: opens its page, ▶ plays it, right
/// click: Слушать · Перемешать · Скачать · Поделиться.
pub fn album_card(ui: &mut Ui, a: &StreamAlbumCard, width: f32, row: &str, out: &mut Vec<Command>) -> Option<Route> {
    let o = MediaCard::new(&a.title)
        .subtitle(album_subtitle(a))
        .cover_opt(a.cover_url.clone())
        .badge_opt(a.have.label())
        .width(width)
        .id_salt(("st-album", row, &a.remote))
        .show(ui);
    let cover = Rect::from_min_size(o.response.rect.min, vec2(width, width));
    service_badge(ui, cover, a.remote.service);
    let menu = album_menu(&o.response, a, out);
    if o.play_clicked {
        out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false });
        None
    } else if o.card_clicked {
        Some(Route::Stream(StreamPage::Album(a.remote.clone())))
    } else {
        menu
    }
}

fn album_menu(resp: &egui::Response, a: &StreamAlbumCard, out: &mut Vec<Command>) -> Option<Route> {
    let picked = Menu::new(("st-album-menu", resp.id))
        .item(MenuItem::new("Слушать").icon(Icon::PlayArrow))
        .item(MenuItem::new("Перемешать").icon(Icon::Shuffle))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(a.artist_id.is_some()))
        .divider()
        .item(MenuItem::new("Скачать альбом").icon(Icon::Download))
        .item(MenuItem::new("Поделиться").icon(Icon::Share))
        .context_menu(resp);
    match picked? {
        0 => out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false }),
        1 => out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: true }),
        2 => return a.artist_id.clone().map(|r| Route::Stream(StreamPage::Artist(r))),
        3 => out.push(Command::LikeStreamAlbum { album: a.remote.clone(), on: true }),
        4 => out.push(Command::Share(ShareTarget::StreamAlbum(a.remote.clone()))),
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
    service_badge(ui, img, a.remote.service);
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
    service_badge(ui, Rect::from_min_size(o.response.rect.min, vec2(width, width)), p.remote.service);
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
            StreamPage::Artist(id) => artist_page(ui, id, v, out),
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
    let artist_id = v.tracks.iter().find_map(|t| t.artist_id.clone());
    header(ui, v, false, |ui| {
        let line = match v.year {
            Some(y) => format!("{} · {y}", type_label(v.release_type)),
            None => type_label(v.release_type).to_owned(),
        };
        ui.horizontal(|ui| {
            ui.label(TypeRole::LabelLarge.style().rich(line, s.primary));
            ui.add_space(8.0);
            service_pill(ui, id.service);
        });
        let artist = ui.add(
            egui::Label::new(TypeRole::TitleMedium.style().rich(count_line(&v.subtitle, v.tracks.len(), v.total_duration), s.on_surface_variant))
                .sense(Sense::click()),
        );
        if artist.clicked() && let Some(a) = &artist_id {
            nav = Some(Route::Stream(StreamPage::Artist(a.clone())));
        }
        artist.on_hover_cursor(egui::CursorIcon::PointingHand);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add(AssistChip::new("стрим").icon(Icon::GraphicEq).id_salt("st-chip-stream"));
            if let Some(have) = v.have.files_label() {
                ui.add(AssistChip::new(have).icon(Icon::Download).id_salt("st-chip-have"));
            }
            if let Some(r) = v.release.filter(|_| v.have.in_library > 0)
                && ui.add(AssistChip::new("В библиотеке").icon(Icon::LibraryMusic).id_salt("st-chip-lib")).clicked()
            {
                nav = Some(Route::Release(r));
            }
        });
    });
    ui.add_space(16.0);
    let remotes: Vec<RemoteId> = v.tracks.iter().map(|t| t.remote.clone()).collect();
    let all_local = v.have.complete() || (!v.tracks.is_empty() && local == v.tracks.len());
    let members: Vec<Member> = vec![
        Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
        Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).style(Style::Tonal).into(),
        Button::new(if all_local { "Скачан" } else { "Скачать" }).icon(Icon::Download).size(Size::M).style(Style::Tonal).enabled(!all_local).into(),
        IconButton::new(Icon::Favorite)
            .size(Size::M)
            .style(Style::Tonal)
            .selected(v.liked)
            .tooltip(if v.liked { "Убрать альбом из Любимых" } else { "Добавить альбом в Любимые" })
            .into(),
        IconButton::new(Icon::Share).size(Size::M).style(Style::Tonal).tooltip("Поделиться").into(),
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
    // A like of the album puts the whole album on the wishlist.
    if r[2].clone().on_hover_text("Весь альбом — в Любимые и в «Хочу скачать»").clicked() {
        out.push(Command::LikeStreamAlbum { album: id.clone(), on: true });
    }
    if r[3].clicked() {
        out.push(Command::LikeStreamAlbum { album: id.clone(), on: !v.liked });
    }
    if r[4].clicked() {
        out.push(Command::Share(ShareTarget::StreamAlbum(id.clone())));
    }
    let more = &r[5];
    let picked = Menu::new("st-album-more")
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(artist_id.is_some()))
        .item(MenuItem::new("Открыть в библиотеке").icon(Icon::LibraryMusic).enabled(v.release.is_some()))
        .attach(more);
    match picked {
        Some(0) => out.push(Command::PlayStreamNext(remotes.clone())),
        Some(1) => out.push(Command::EnqueueStream(remotes.clone())),
        Some(2) => pages.add_stream_to_playlist(remotes.clone(), Some(more.rect)),
        Some(3) => nav = artist_id.clone().map(|a| Route::Stream(StreamPage::Artist(a))),
        Some(4) => nav = v.release.map(Route::Release),
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
    header(ui, v, false, |ui| {
        ui.horizontal(|ui| {
            ui.label(TypeRole::LabelLarge.style().rich("ПЛЕЙЛИСТ", s.primary));
            ui.add_space(8.0);
            service_pill(ui, id.service);
        });
        ui.label(TypeRole::TitleMedium.style().rich(count_line(&v.subtitle, v.tracks.len(), v.total_duration), s.on_surface_variant));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if v.editable {
                ui.add(AssistChip::new("твой").icon(Icon::Person).id_salt("st-pl-own"));
            }
            if let Some(p) = v.playlist
                && ui.add(AssistChip::new("В библиотеке").icon(Icon::LibraryMusic).id_salt("st-pl-lib")).clicked()
            {
                nav = Some(Route::Playlist(p));
            }
        });
    });
    ui.add_space(16.0);
    let remotes: Vec<RemoteId> = v.tracks.iter().map(|t| t.remote.clone()).collect();
    let row = ButtonRow::new("st-pl-actions", Size::M).show(
        ui,
        vec![
            Button::new("Слушать").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).into(),
            Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).style(Style::Tonal).into(),
            Button::new("В плейлист").icon(Icon::PlaylistAdd).size(Size::M).style(Style::Outlined).into(),
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
    if r[2].clone().on_hover_text("Все треки — в свой плейлист (и в библиотеку)").clicked() {
        pages.add_stream_to_playlist(remotes.clone(), Some(r[2].rect));
    }
    let picked = Menu::new("st-pl-more")
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic))
        .item(MenuItem::new("Открыть в библиотеке").icon(Icon::LibraryMusic).enabled(v.playlist.is_some()))
        .attach(&r[3]);
    match picked {
        Some(0) => out.push(Command::PlayStreamNext(remotes.clone())),
        Some(1) => out.push(Command::EnqueueStream(remotes.clone())),
        Some(2) => nav = v.playlist.map(Route::Playlist),
        _ => {}
    }
    ui.add_space(16.0);
    if v.tracks.is_empty() {
        widgets::empty_state(ui, Icon::QueueMusic, "Плейлист пуст", "В сервисе в нём пока ничего нет.", None);
        return nav;
    }
    track_rows(ui, "st-playlist", &v.tracks, false, &StreamFrom::Playlist(id.clone()), state, pages, out).or(nav)
}

fn artist_page(ui: &mut Ui, id: &RemoteId, v: &StreamPageView, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let mut nav = None;
    header(ui, v, true, |ui| {
        ui.horizontal(|ui| {
            ui.label(TypeRole::LabelLarge.style().rich("ИСПОЛНИТЕЛЬ", s.primary));
            ui.add_space(8.0);
            service_pill(ui, id.service);
        });
        let n = v.albums.len() as u64;
        ui.label(TypeRole::TitleMedium.style().rich(
            format!("{n} {} в {}", common::plural(n, "релиз", "релиза", "релизов"), id.service.display_name()),
            s.on_surface_variant,
        ));
        let have: u32 = v.albums.iter().map(|a| a.have.local).sum();
        if have > 0 {
            ui.add_space(8.0);
            ui.add(AssistChip::new(format!("{have} {} у тебя", common::plural(u64::from(have), "трек", "трека", "треков"))).icon(Icon::Download).id_salt("st-ar-have"));
        }
    });
    ui.add_space(16.0);
    let newest = v.albums.first().map(|a| a.remote.clone());
    let mut members: Vec<Member> = vec![
        Button::new("Слушать новое").icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).enabled(newest.is_some()).into(),
    ];
    if v.artist.is_some() {
        members.push(
            Button::new(if v.liked { "Вы подписаны" } else { "Подписаться" })
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
        if r[1].clicked() {
            out.push(Command::Like(LikeTarget::Artist(a), !v.liked));
        }
        if r[2].clicked() {
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
        nav = widgets::grid(ui, &list, widgets::CARD_W, |ui, a, w| album_card(ui, a, w, title, out)).or(nav);
        ui.add_space(widgets::SECTION_GAP);
    }
    if v.albums.is_empty() {
        widgets::empty_state(ui, Icon::Album, "Релизов нет", "Сервис не знает релизов этого исполнителя.", None);
    }
    nav
}
