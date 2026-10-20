//! Библиотека (md3e-player.md §3.2): my playlists first, then releases,
//! then artists (grid or list); a virtualized track table on "Треки".

use egui::{Id, Rect, Sense, Stroke, Ui, UiBuilder, Widget as _, pos2, vec2};
use nikgob_view::{
    ArtistCard, Command, LibraryTab, PlaylistCard, ReleaseCard, ReleaseSort, ReleaseType, Route,
    TrackSort, ViewState,
};

use super::{PageState, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Card, ConnectedGroup, Equalizer, FilterChip, IconButton, Leading, ListItem, Menu,
        MenuItem, Radii, Size, anim, paint,
    },
    screens::common,
    tokens::{shape, state as state_layer},
};

const TABS: [(LibraryTab, &str); 6] = [
    (LibraryTab::All, "Всё"),
    (LibraryTab::Playlists, "Плейлисты"),
    (LibraryTab::Albums, "Альбомы"),
    (LibraryTab::Artists, "Исполнители"),
    (LibraryTab::Tracks, "Треки"),
    (LibraryTab::Trash, "Корзина"),
];

/// Grid (false) or list (true) for playlists, releases and artists.
fn list_view(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("lib-list-view"))).unwrap_or(false)
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let lib = &state.library;
    let mut nav = None;
    let tab = pages.library_tab;
    let is_list = list_view(ui.ctx());

    ui.add_space(8.0);
    // Fixed height so the title and the 40 dp controls share a centre.
    let row = vec2(ui.available_width(), 48.0);
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(TypeRole::HeadlineMedium.style().rich("Библиотека", s.on_surface));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Sort applies to the release grid only; the table sorts by header.
            if matches!(tab, LibraryTab::All | LibraryTab::Albums) {
                let sort = ui.add(IconButton::new(Icon::Sort).tooltip("Сортировка").id_salt("lib-sort"));
                let cur = lib.release_sort;
                let picked = Menu::new("lib-sort-menu")
                    .align_right(true)
                    .item(MenuItem::new("По исполнителю").selected(cur == ReleaseSort::Artist))
                    .item(MenuItem::new("По названию").selected(cur == ReleaseSort::Title))
                    .item(MenuItem::new("По году").selected(cur == ReleaseSort::Year))
                    .item(MenuItem::new("Недавно добавленные").selected(cur == ReleaseSort::Added))
                    .attach(&sort);
                if let Some(i) = picked {
                    let sorts = [ReleaseSort::Artist, ReleaseSort::Title, ReleaseSort::Year, ReleaseSort::Added];
                    out.push(Command::SortReleases(sorts[i]));
                }
            }
            if tab != LibraryTab::Tracks {
                // The group lays out left to right inside this right-to-left row.
                let picked = ui
                    .allocate_ui_with_layout(vec2(82.0, 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ConnectedGroup::new("lib-view", &["Сетка", "Список"])
                            .icons(&[Icon::GridView, Icon::ViewList])
                            .show(ui, usize::from(is_list))
                    })
                    .inner;
                if let Some(i) = picked {
                    ui.ctx().data_mut(|d| d.insert_temp(Id::new("lib-list-view"), i == 1));
                }
            }
        });
    });
    ui.add_space(8.0);
    let labels: Vec<&str> = TABS.iter().map(|t| t.1).collect();
    let idx = TABS.iter().position(|t| t.0 == pages.library_tab).unwrap_or(0);
    // The tabs, and at the far end of their row the inconspicuous
    // «только скачанное» (docs/stage3.md §0): a faint icon, nothing loud.
    let picked = ui
        .allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            let w = (ui.available_width() - 36.0).max(0.0);
            let picked = ui
                .allocate_ui_with_layout(vec2(w, 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ConnectedGroup::new("lib-tabs", &labels).show(ui, idx)
                })
                .inner;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                only_downloaded(ui, state, out);
            });
            picked
        })
        .inner;
    if let Some(i) = picked {
        pages.library_tab = TABS[i].0;
        if pages.library_tab == LibraryTab::Tracks {
            out.push(Command::LoadTracks { offset: 0, limit: 200, sort: lib.track_sort });
        }
        if pages.library_tab == LibraryTab::Trash {
            out.push(Command::LoadTrash);
        }
    }
    ui.add_space(16.0);

    if pages.library_tab == LibraryTab::Trash {
        return trash(ui, state, pages, out);
    }
    if lib.loaded && lib.releases.is_empty() && lib.playlists.len() <= 1 {
        if widgets::empty_state(
            ui,
            Icon::LibraryMusic,
            "Здесь пока пусто",
            "Лайкни трек в поиске — он появится здесь. Свои файлы: папка Inbox или «Добавить».",
            Some("Добавить папку"),
        ) && let Some(dir) = rfd::FileDialog::new().pick_folder()
        {
            out.push(Command::ImportFolder(dir));
        }
        return None;
    }

    let tab = pages.library_tab;
    if matches!(tab, LibraryTab::All | LibraryTab::Albums) {
        widgets::edition_offers(ui, state, None, out);
    }
    if matches!(tab, LibraryTab::All | LibraryTab::Playlists) {
        nav = playlists(ui, state, pages, out, tab == LibraryTab::All, is_list).or(nav);
        ui.add_space(widgets::SECTION_GAP);
    }
    if matches!(tab, LibraryTab::All | LibraryTab::Albums) {
        nav = releases(ui, state, pages, out, is_list).or(nav);
        ui.add_space(widgets::SECTION_GAP);
    }
    if matches!(tab, LibraryTab::All | LibraryTab::Artists) {
        widgets::section_header(ui, "Исполнители", false);
        let artists = if tab == LibraryTab::All {
            &lib.artists[..lib.artists.len().min(24)]
        } else {
            &lib.artists[..]
        };
        if is_list {
            for a in artists {
                nav = artist_row(ui, a, out).or(nav);
            }
        } else {
            nav = widgets::grid(ui, artists, 120.0, |ui, a, w| widgets::artist_avatar(ui, a, w, out))
                .or(nav);
        }
    }
    if tab == LibraryTab::Tracks {
        nav = tracks(ui, state, pages, out).or(nav);
    }
    nav
}

/// The faint «только скачанное» switch: a small outlined arrow that is
/// barely there while off and a little firmer while on.
fn only_downloaded(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let on = state.prefs.library_only_downloaded;
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if on {
        "Только скачанное: стрим скрыт. Нажми, чтобы показать всё"
    } else {
        "Только скачанное"
    });
    // Not the ⬇ of downloads: a check over a line, what is on disk.
    if resp.hovered() {
        ui.painter().circle_filled(rect.center(), 14.0, s.on_surface.gamma_multiply(0.06));
    }
    let tint = match (on, resp.hovered()) {
        (true, _) => s.on_surface_variant,
        (false, true) => s.outline,
        (false, false) => s.outline_variant,
    };
    Icon::DownloadDone.widget(on, tint, 18.0).paint_at(ui, Rect::from_center_size(rect.center(), vec2(18.0, 18.0)));
    if resp.clicked() {
        out.push(Command::SetPrefs(Box::new(nikgob_view::Prefs { library_only_downloaded: !on, ..state.prefs.clone() })));
    }
}

/// "Корзина": deleted tracks with "Восстановить" and "Удалить навсегда";
/// everything goes for good after 30 days.
fn trash(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let items = &state.library.trash;
    let row = vec2(ui.available_width(), 40.0);
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let n = items.len() as u64;
        ui.label(TypeRole::TitleLarge.style().rich(
            format!("{n} {} в корзине", common::plural(n, "трек", "трека", "треков")),
            s.on_surface,
        ));
        if !items.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(crate::components::Button::new("Очистить корзину").icon(Icon::Delete).style(crate::components::Style::Tonal).id_salt("trash-empty"))
                    .clicked()
                {
                    pages.confirm_empty_trash = true;
                }
                if ui
                    .add(crate::components::Button::new("Восстановить всё").style(crate::components::Style::Text).id_salt("trash-restore-all"))
                    .clicked()
                {
                    out.push(Command::RestoreTracks(items.iter().map(|i| i.track.id).collect()));
                }
            });
        }
    });
    ui.label(TypeRole::BodyMedium.style().rich_para(
        if state.prefs.use_trash {
            "Удалённое не видно в библиотеке и через 30 дней стирается навсегда вместе с файлами."
        } else {
            "Корзина выключена в настройках: удаление сразу стирает файлы. Здесь остаётся то, что удалили раньше."
        },
        s.on_surface_variant,
    ));
    ui.add_space(8.0);
    if items.is_empty() {
        widgets::empty_state(ui, Icon::Delete, "Корзина пуста", "Удалённые треки и альбомы появятся здесь.", None);
        return None;
    }
    for (i, item) in items.iter().enumerate() {
        let t = &item.track;
        let mut line = t.artist.clone();
        if let Some(a) = &t.album {
            line = format!("{line} · {a}");
        }
        line = format!("{line} · удалено {} · осталось {} дн.", item.deleted, item.days_left);
        let (mut restore, mut purge) = (false, false);
        let o = ListItem::new(&t.title)
            .supporting(line)
            .leading(Leading::Image(t.cover.as_ref().map(|c| c.uri()), Icon::MusicNote))
            .trailing(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    purge = ui
                        .add(IconButton::new(Icon::Delete).size(Size::S).tooltip("Удалить навсегда").id_salt(("trash-purge", i)))
                        .clicked();
                    restore = ui
                        .add(crate::components::Button::new("Восстановить").size(Size::S).style(crate::components::Style::Tonal).id_salt(("trash-restore", i)))
                        .clicked();
                });
            })
            .id_salt(("trash-row", t.id))
            .show(ui);
        if restore {
            out.push(Command::RestoreTracks(vec![t.id]));
        }
        if purge {
            pages.confirm_purge = Some((super::Doomed::Tracks(vec![t.id]), format!("«{}»", t.title)));
        }
        let picked = Menu::new(("trash-menu", t.id))
            .item(MenuItem::new("Восстановить"))
            .divider()
            .item(MenuItem::new("Удалить навсегда").icon(Icon::Delete))
            .context_menu(&o.response);
        match picked {
            Some(0) => out.push(Command::RestoreTracks(vec![t.id])),
            Some(1) => pages.confirm_purge = Some((super::Doomed::Tracks(vec![t.id]), format!("«{}»", t.title))),
            _ => {}
        }
    }
    None
}

fn playlists(
    ui: &mut Ui,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
    row: bool,
    is_list: bool,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    widgets::section_header(ui, "Мои плейлисты", false);
    let mut nav = None;
    let new_playlist = |ui: &mut Ui, pages: &mut PageState, w: f32| {
        // "+ Новый плейлист": outlined card.
        let o = Card::outlined()
            .clickable(true)
            .width(w)
            .min_height(w + 56.0)
            .label("Новый плейлист")
            .id_salt("new-playlist-card")
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(w * 0.3);
                    Icon::Add.widget(false, s.primary, 40.0).ui(ui);
                    ui.add_space(8.0);
                    ui.label(TypeRole::TitleSmall.style().rich("Новый плейлист", s.on_surface));
                });
            });
        if o.response.clicked() {
            pages.open_ask(super::Ask::NewPlaylist { then_add: Vec::new() });
        }
    };
    if is_list {
        if ListItem::new("Новый плейлист")
            .leading_icon(Icon::PlaylistAdd)
            .id_salt("new-playlist-row")
            .show(ui)
            .response
            .clicked()
        {
            pages.open_ask(super::Ask::NewPlaylist { then_add: Vec::new() });
        }
        for p in &state.library.playlists {
            nav = playlist_row(ui, p, pages, out).or(nav);
        }
        return nav;
    }
    if row {
        widgets::card_row(ui, "lib-playlists", |ui| {
            new_playlist(ui, pages, widgets::CARD_W);
            for p in &state.library.playlists {
                nav = widgets::playlist_card_with(ui, p, widgets::CARD_W, "lib", Some(pages), out).or(nav.take());
            }
        });
    } else {
        // Grid: rows of the same cards.
        let width = ui.available_width();
        let cols = widgets::columns(width, widgets::CARD_W);
        let w = ((width - widgets::CARD_GAP * (cols - 1) as f32) / cols as f32).floor();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(widgets::CARD_GAP, widgets::CARD_GAP);
            new_playlist(ui, pages, w);
            for p in &state.library.playlists {
                nav = widgets::playlist_card_with(ui, p, w, "lib", Some(pages), out).or(nav.take());
            }
        });
    }
    nav
}

fn releases(
    ui: &mut Ui,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
    is_list: bool,
) -> Option<Route> {
    ui.horizontal(|ui| {
        ui.label(TypeRole::TitleLarge.style().rich("Альбомы и релизы", Theme::get(ui.ctx()).scheme.on_surface));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Where the albums play from: files, or only a service (no use
            // while «только скачанное» hides the stream anyway).
            if !state.prefs.library_only_downloaded {
                const SOURCES: [&str; 3] = ["Все источники", "Скачанные", "Стрим"];
                let cur = pages.release_source.min(2);
                let chip = ui.add(FilterChip::new(SOURCES[cur], cur != 0).trailing_icon(Icon::ExpandMore).id_salt("rel-source"));
                let mut menu = Menu::new("rel-source-menu").align_right(true);
                for (i, l) in SOURCES.iter().enumerate() {
                    menu = menu.item(MenuItem::new(*l).selected(i == cur));
                }
                if let Some(i) = menu.attach(&chip) {
                    pages.release_source = i;
                }
                ui.add_space(8.0);
            }
            for (i, label) in ["EP", "Синглы", "Альбомы", "Всё"].iter().enumerate() {
                let idx = 3 - i;
                if ui
                    .add(FilterChip::new(*label, pages.release_filter == idx).id_salt(("rel-filter", idx)))
                    .clicked()
                {
                    pages.release_filter = idx;
                }
            }
        });
    });
    ui.add_space(8.0);
    let filtered: Vec<_> = state
        .library
        .releases
        .iter()
        .filter(|r| match pages.release_filter {
            1 => matches!(r.release_type, ReleaseType::Album | ReleaseType::Compilation),
            2 => r.release_type == ReleaseType::Single,
            3 => r.release_type == ReleaseType::Ep,
            _ => true,
        })
        .filter(|r| match pages.release_source {
            _ if state.prefs.library_only_downloaded => true,
            1 => r.local_tracks > 0,
            2 => r.total_tracks.map_or(r.local_tracks == 0, |t| r.local_tracks < t),
            _ => true,
        })
        .collect();
    if is_list {
        let mut nav = None;
        for r in filtered {
            nav = release_row(ui, r, out).or(nav);
        }
        return nav;
    }
    widgets::grid(ui, &filtered, widgets::CARD_W, |ui, r, w| widgets::release_card(ui, r, w, "grid", out))
}

/// List view: an album row (cover, title, artist · year, "3/12", ▶).
fn release_row(ui: &mut Ui, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let badge = widgets::card_badge(r);
    let mut play = false;
    let o = ListItem::new(&r.title)
        .supporting(widgets::release_subtitle(r))
        .leading(Leading::Image(r.cover.as_ref().map(|c| c.uri()), Icon::Album))
        .trailing(|ui| {
            play = ui
                .add(IconButton::new(Icon::PlayArrow).size(Size::Xs).tooltip("Слушать").id_salt(("rel-row-play", r.id)))
                .clicked();
            if let Some(b) = badge {
                ui.label(TypeRole::LabelMedium.style().rich(b, s.on_surface_variant));
            }
        })
        .id_salt(("rel-row", r.id))
        .show(ui);
    let menu = widgets::release_menu(&o.response, r, out);
    if play {
        out.push(Command::PlayRelease { id: r.id, start: None, shuffle: false });
        None
    } else if o.response.clicked() {
        Some(Route::Release(r.id))
    } else {
        menu
    }
}

/// List view: a playlist row.
fn playlist_row(ui: &mut Ui, p: &PlaylistCard, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let n = u64::from(p.track_count);
    let mut play = false;
    let o = ListItem::new(&p.name)
        .supporting(format!("{n} {}", common::plural(n, "трек", "трека", "треков")))
        .leading(Leading::Image(
            p.cover.as_ref().map(|c| c.uri()),
            if p.pinned { Icon::Favorite } else { Icon::QueueMusic },
        ))
        .trailing(|ui| {
            play = ui
                .add(IconButton::new(Icon::PlayArrow).size(Size::Xs).tooltip("Слушать").id_salt(("pl-row-play", p.id)))
                .clicked();
        })
        .id_salt(("pl-row", p.id))
        .show(ui);
    widgets::playlist_menu_at(&o.response, p, Some(pages), out);
    if play {
        out.push(Command::PlayPlaylist { id: p.id, start: None, shuffle: false });
        None
    } else if o.response.clicked() {
        Some(Route::Playlist(p.id))
    } else {
        None
    }
}

/// List view: an artist row.
fn artist_row(ui: &mut Ui, a: &ArtistCard, out: &mut Vec<Command>) -> Option<Route> {
    let mut play = false;
    let o = ListItem::new(&a.name)
        .leading(Leading::Avatar(a.image.as_ref().map(|c| c.uri()), Icon::Person))
        .trailing(|ui| {
            play = ui
                .add(IconButton::new(Icon::PlayArrow).size(Size::Xs).tooltip("Слушать микс").id_salt(("ar-row-play", a.id)))
                .clicked();
        })
        .id_salt(("ar-row", a.id))
        .show(ui);
    widgets::artist_menu(&o.response, a, out);
    if play {
        out.push(Command::PlayArtist { id: a.id, shuffle: false });
        None
    } else {
        o.response.clicked().then_some(Route::Artist(a.id))
    }
}

/// A single-line galley, truncated with an ellipsis at `max_width`.
fn line(
    painter: &egui::Painter,
    text: &str,
    style: crate::TypeStyle,
    color: egui::Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), style.text_format(color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_width.max(1.0));
    painter.layout_job(job)
}

fn plain(
    painter: &egui::Painter,
    text: &str,
    style: crate::TypeStyle,
    color: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    painter.layout_job(egui::text::LayoutJob::single_section(text.to_owned(), style.text_format(color)))
}

/// `c` fully transparent, keeping its hue (colour springs avoid black).
fn clear(c: egui::Color32) -> egui::Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    egui::Color32::from_rgba_unmultiplied(r, g, b, 0)
}

const ROW_H: f32 = 56.0;
const HEADER_H: f32 = 48.0;
const SIDE: f32 = 16.0;
const NUM_W: f32 = 32.0;
const DUR_W: f32 = 96.0;
const QUALITY_W: f32 = 120.0;
const MORE_W: f32 = 32.0;
const COL_GAP: f32 = 16.0;
const COVER_W: f32 = 40.0;

/// Column x ranges of the table for a given width.
struct Columns {
    num: (f32, f32),
    title: (f32, f32),
    album: Option<(f32, f32)>,
    quality: Option<(f32, f32)>,
    duration: (f32, f32),
    more: (f32, f32),
}

impl Columns {
    fn new(left: f32, width: f32) -> Self {
        let right = left + width;
        let more = (right - SIDE - MORE_W, right - SIDE);
        let duration = (more.0 - 8.0 - DUR_W, more.0 - 8.0);
        let mut edge = duration.0 - COL_GAP;
        let quality = (width >= 560.0).then(|| {
            let q = (edge - QUALITY_W, edge);
            edge = q.0 - COL_GAP;
            q
        });
        let num = (left + SIDE, left + SIDE + NUM_W);
        let title_left = num.1 + COL_GAP;
        let album = (width >= 720.0).then(|| {
            let w = ((edge - title_left) * 0.36).clamp(120.0, 280.0);
            let a = (edge - w, edge);
            edge = a.0 - COL_GAP;
            a
        });
        Self {
            num,
            title: (title_left, edge),
            album,
            quality,
            duration,
            more,
        }
    }
}

/// Virtualized table: header pinned to the top of the viewport, 56 dp rows
/// laid out only where they intersect it; windows of rows load on demand.
fn tracks(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let motion = Theme::get(ui.ctx()).motion;
    let lib = &state.library;
    let mut nav = None;
    let total = lib.track_count;
    ui.label(TypeRole::TitleLarge.style().rich(
        format!("{total} {}", common::plural(total as u64, "трек", "трека", "треков")),
        s.on_surface,
    ));
    ui.add_space(8.0);
    if lib.loaded && total == 0 {
        ui.label(TypeRole::BodyMedium.style().rich("Здесь пока нет треков.", s.on_surface_variant));
        return None;
    }

    let width = ui.available_width();
    // The page scroll area is outer; reserve the whole height and draw only
    // what intersects the clip rect.
    let (rect, _) = ui.allocate_exact_size(vec2(width, HEADER_H + ROW_H * total as f32), Sense::hover());
    let cols = Columns::new(rect.left(), width);
    let clip = ui.clip_rect();
    let body_top = rect.top() + HEADER_H;
    let first = (((clip.top() - body_top) / ROW_H).floor().max(0.0)) as usize;
    let last = ((((clip.bottom() - body_top) / ROW_H).ceil().max(0.0)) as usize).min(total);

    // Ask for the window if it is not loaded.
    let have = lib.tracks_offset..lib.tracks_offset + lib.tracks.len();
    if first < last && (first < have.start || last > have.end) && pages.track_window != (first, last) {
        pages.track_window = (first, last);
        let offset = first.saturating_sub(50);
        out.push(Command::LoadTracks { offset, limit: (last - offset) + 150, sort: lib.track_sort });
    }

    let ids: Vec<_> = lib.tracks.iter().map(|t| t.id).collect();
    let current = state.player.current.as_ref().map(|t| t.id);
    let playing = state.player.status == nikgob_view::Status::Playing;
    for i in first..last.max(first) {
        let y = body_top + i as f32 * ROW_H;
        let row = Rect::from_min_size(pos2(rect.left(), y), vec2(width, ROW_H));
        let id = ui.id().with(("trow", i));
        let resp = ui.interact(row, id, Sense::click());
        // The # cell is its own button on top of the row: the ▶ shown there
        // on hover plays with one click (the row itself still needs two).
        let num_c = pos2((cols.num.0 + cols.num.1) * 0.5, row.center().y);
        let cell = ui.interact(Rect::from_center_size(num_c, vec2(40.0, 40.0)), id.with("num"), Sense::click());
        // The pointer on an artist link is on the row too (last frame's answer).
        let link_hover_key = id.with("link-hover");
        let link_hover = ui.ctx().data(|d| d.get_temp::<bool>(link_hover_key)).unwrap_or(false);
        let hovered = resp.hovered() || cell.hovered() || link_hover;
        let track = i.checked_sub(lib.tracks_offset).and_then(|j| lib.tracks.get(j));
        let is_cur = track.is_some_and(|t| current == Some(t.id));
        let (fg, variant, layer_color) = if is_cur {
            (s.on_secondary_container, s.on_secondary_container, s.on_secondary_container)
        } else {
            (s.on_surface, s.on_surface_variant, s.on_surface)
        };
        let radii = Radii::all(shape::LARGE);
        let bg = anim::color(
            ui.ctx(),
            id.with("bg"),
            if is_cur { s.secondary_container } else { clear(s.secondary_container) },
            motion.fast_effects,
        );
        paint::container(ui.painter(), row, radii, bg, Stroke::NONE);
        let layer = anim::fraction(
            ui.ctx(),
            id.with("layer"),
            if hovered { state_layer::HOVER } else { 0.0 },
            motion.fast_effects,
        );
        paint::state_layer(ui.painter(), row, radii, layer_color, layer);
        let cy = row.center().y;

        let Some(t) = track else {
            // Not loaded yet: skeleton lines.
            for (w, dy) in [(0.5, -9.0), (0.3, 9.0)] {
                let bar = Rect::from_center_size(
                    pos2(cols.title.0 + (cols.title.1 - cols.title.0) * w * 0.5, cy + dy),
                    vec2((cols.title.1 - cols.title.0) * w, 10.0),
                );
                paint::container(ui.painter(), bar, Radii::all(5.0), s.surface_container_highest, Stroke::NONE);
            }
            continue;
        };

        // #: number, ▶ on hover, equalizer on the playing row.
        let cell_layer = if cell.is_pointer_button_down_on() {
            state_layer::PRESSED
        } else if cell.hovered() {
            state_layer::HOVER
        } else {
            0.0
        };
        paint::state_layer(
            ui.painter(),
            Rect::from_center_size(num_c, vec2(40.0, 40.0)),
            Radii::all(20.0),
            layer_color,
            cell_layer,
        );
        if is_cur {
            Equalizer::new(("tbl-eq", i), fg)
                .playing(playing)
                .paint_at(ui, Rect::from_center_size(num_c, vec2(24.0, 24.0)));
        } else if hovered {
            Icon::PlayArrow.widget(true, variant, 24.0).paint_at(ui, Rect::from_center_size(num_c, vec2(24.0, 24.0)));
        } else {
            let g = plain(ui.painter(), &(i + 1).to_string(), TypeRole::BodyLarge.style(), variant);
            ui.painter().galley(num_c - g.size() * 0.5, g, variant);
        }
        // Обложка, название / исполнитель.
        let thumb = Rect::from_min_size(pos2(cols.title.0, cy - COVER_W * 0.5), vec2(COVER_W, COVER_W));
        crate::components::paint_image(
            ui,
            thumb,
            t.cover.as_ref().map(|c| c.uri()).as_deref(),
            egui::CornerRadius::same(shape::SMALL as u8),
            Icon::MusicNote,
        );
        let tx = cols.title.0 + COVER_W + 12.0;
        let tw = cols.title.1 - tx;
        let g = line(ui.painter(), &t.title, TypeRole::BodyLarge.style(), fg, tw);
        ui.painter().galley(pos2(tx, cy - 22.0), g, fg);
        let artist = if t.clip { format!("{} · клип", t.artist) } else { t.artist.clone() };
        let g = line(ui.painter(), &artist, TypeRole::BodyMedium.style(), variant, tw);
        // Each artist is a link to its page.
        let (links, routes) = crate::screens::artist_links::library_spans(&t.artists, &artist);
        let (clicked, over) =
            crate::components::interact_links(ui, id.with("artists"), &g, pos2(tx, cy + 2.0), &links, variant);
        if over != link_hover {
            ui.ctx().data_mut(|d| d.insert_temp(link_hover_key, over));
        }
        if let Some(r) = clicked.and_then(|k| routes.get(k).cloned()) {
            nav = Some(r);
        }
        ui.painter().galley(pos2(tx, cy + 2.0), g, variant);
        // Альбом.
        if let (Some((x0, x1)), Some(a)) = (cols.album, &t.album) {
            let g = line(ui.painter(), a, TypeRole::BodyMedium.style(), variant, x1 - x0);
            ui.painter().galley(pos2(x0, cy - g.size().y * 0.5), g, variant);
        }
        // Качество; a track without a file plays from a service: "стрим".
        if let Some((x0, x1)) = cols.quality {
            let stream = match (t.has_local, t.source) {
                (false, Some(nikgob_view::SourceKind::Spotify)) => Some("стрим · Spotify"),
                (false, Some(nikgob_view::SourceKind::Youtube)) => Some("стрим · YouTube"),
                _ => None,
            };
            if let Some(m) = stream {
                let g = plain(ui.painter(), m, TypeRole::LabelSmall.style(), s.on_secondary_container);
                let r = Rect::from_min_size(pos2(x0, cy - 10.0), vec2((g.size().x + 12.0).min(x1 - x0), 20.0));
                paint::container(ui.painter(), r, Radii::all(shape::SMALL), s.secondary_container, Stroke::NONE);
                ui.painter().galley(pos2(r.left() + 6.0, cy - g.size().y * 0.5), g, s.on_secondary_container);
            } else if let Some(q) = &t.quality {
                let g = line(ui.painter(), q, TypeRole::BodyMedium.style(), variant, x1 - x0);
                ui.painter().galley(pos2(x0, cy - g.size().y * 0.5), g, variant);
            }
        }
        // Длительность, по правому краю.
        if let Some(d) = t.duration {
            let g = plain(ui.painter(), &common::fmt_time(d), TypeRole::BodyMedium.style(), variant);
            ui.painter().galley(pos2(cols.duration.1 - g.size().x, cy - g.size().y * 0.5), g, variant);
        }
        // ⋮.
        let slot = Rect::from_center_size(pos2((cols.more.0 + cols.more.1) * 0.5, cy), vec2(MORE_W, MORE_W));
        let mut mui = ui.new_child(UiBuilder::new().max_rect(slot).id_salt(("tbl-more", i)));
        let more = mui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip("Ещё").id_salt(("tbl-more-btn", i)));
        nav = pages.track_menu_attach(&more, t, state, out).or(nav);

        // A second tap of a double click on ▶ must not pause what the first started.
        if resp.double_clicked() || (cell.clicked() && !cell.double_clicked()) {
            out.push(if is_cur {
                Command::TogglePlay
            } else {
                Command::PlayTracks {
                    tracks: ids.clone(),
                    start: i - lib.tracks_offset,
                    context: "библиотеки".into(),
                }
            });
        }
        nav = pages.track_context_menu(&resp, t, state, out).or(nav);
    }

    // Header, pinned at the top of the viewport once scrolled past.
    let top = rect.top().max(clip.top());
    let header = Rect::from_min_size(pos2(rect.left(), top), vec2(width, HEADER_H));
    ui.painter().rect_filled(header, 0.0, s.surface);
    ui.interact(header, ui.id().with("tbl-header"), Sense::click());
    ui.painter().line_segment(
        [pos2(header.left(), header.bottom() - 0.5), pos2(header.right(), header.bottom() - 0.5)],
        Stroke::new(1.0, s.outline_variant),
    );
    let cy = header.center().y;
    let mut sort = None;
    let title_x = cols.title.0 + COVER_W + 12.0;
    // (label, x range, sort it triggers)
    let mut cells: Vec<(&str, (f32, f32), Option<TrackSort>, bool)> = vec![
        ("#", cols.num, Some(TrackSort::Added), false),
        ("Название", (title_x, title_x + 76.0), Some(TrackSort::Title), false),
        ("Исполнитель", (title_x + 92.0, title_x + 92.0 + 92.0), Some(TrackSort::Artist), false),
    ];
    if let Some(a) = cols.album {
        cells.push(("Альбом", a, Some(TrackSort::Album), false));
    }
    if let Some(q) = cols.quality {
        cells.push(("Качество", q, None, false));
    }
    cells.push(("Длительность", cols.duration, Some(TrackSort::Duration), true));
    for (label, (x0, x1), by, right) in cells {
        let active = by == Some(lib.track_sort);
        let color = if active { s.on_surface } else { s.on_surface_variant };
        let g = line(ui.painter(), label, TypeRole::LabelLarge.style(), color, x1 - x0);
        let w = g.size().x;
        let x = if right { x1 - w } else { x0 };
        let text = Rect::from_min_size(pos2(x, cy - g.size().y * 0.5), g.size());
        if let Some(by) = by {
            let hit = text.expand2(vec2(8.0, 12.0));
            let r = ui.interact(hit, ui.id().with(("tbl-sort", label)), Sense::click());
            let k = anim::fraction(
                ui.ctx(),
                ui.id().with(("tbl-sort-layer", label)),
                if r.hovered() { state_layer::HOVER } else { 0.0 },
                motion.fast_effects,
            );
            paint::state_layer(ui.painter(), hit, Radii::all(shape::SMALL), s.on_surface, k);
            let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.clicked() && !active {
                sort = Some(by);
            }
        }
        ui.painter().galley(text.min, g, color);
        if active {
            let ix = if right { text.left() - 12.0 } else { text.right() + 12.0 };
            Icon::ExpandMore
                .widget(false, color, 16.0)
                .paint_at(ui, Rect::from_center_size(pos2(ix, cy), vec2(16.0, 16.0)));
        }
    }
    if let Some(sort) = sort {
        // Reload the visible window in the new order.
        pages.track_window = (first, last);
        let offset = first.saturating_sub(50);
        out.push(Command::LoadTracks { offset, limit: last.saturating_sub(offset) + 150, sort });
    }
    nav
}
