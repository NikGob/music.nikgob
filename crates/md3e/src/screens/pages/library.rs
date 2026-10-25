//! Библиотека (md3e-player.md §3.2, docs/stage4.md §12.6): a header with
//! the instant filter, «только скачанное», the view and the sort; «Всё» as
//! an overview (rows of playlists, the latest and the most played albums,
//! the first rows of the album grid, artists), each section leading on to
//! its tab; albums under headers that follow the sort; a virtualized track
//! table on «Треки».

use egui::{Align, Frame, Id, Key, Margin, Rect, RichText, Sense, Stroke, TextEdit, Ui, UiBuilder, Widget as _, pos2, vec2};
use nikgob_view::{
    ArtistCard, Command, LibraryTab, PlaylistCard, ReleaseCard, ReleaseSort, ReleaseType, Route, TrackRow,
    TrackSort, ViewState,
};

use super::{PageState, shelf, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Card, ConnectedGroup, Equalizer, FilterChip, IconButton, Leading, ListItem, LoadingIndicator, Menu,
        MenuItem, Radii, Size, Style, anim, paint,
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

const SORTS: [(ReleaseSort, &str); 4] = [
    (ReleaseSort::Artist, "По исполнителю"),
    (ReleaseSort::Title, "По названию"),
    (ReleaseSort::Year, "По году"),
    (ReleaseSort::Added, "Недавно добавленные"),
];

/// Artist avatars in grids and rows.
const AVATAR: f32 = 136.0;

/// Grid (false) or list (true) for playlists, releases and artists.
fn list_view(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("lib-list-view"))).unwrap_or(false)
}

pub(super) fn set_list_view(ctx: &egui::Context, list: bool) {
    ctx.data_mut(|d| d.insert_temp(Id::new("lib-list-view"), list));
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let lib = &state.library;
    header(ui, state, pages, out);
    ui.add_space(8.0);
    tabs(ui, pages, lib.track_sort, out);
    if std::mem::take(&mut pages.reopen_library_tab) {
        let tab = pages.library_tab;
        open_tab(pages, tab, lib.track_sort, out);
    }
    ui.add_space(16.0);

    let tab = pages.library_tab;
    if tab == LibraryTab::Trash {
        return trash(ui, state, pages, out);
    }
    if !lib.loaded {
        // A short wait (the library is read at start): the indicator alone.
        ui.add_space(48.0);
        ui.vertical_centered(|ui| ui.add(LoadingIndicator::new()));
        return None;
    }
    if lib.releases.is_empty() && lib.playlists.len() <= 1 && lib.artists.is_empty() && lib.track_count == 0 {
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
    if tab == LibraryTab::Tracks {
        filter_tracks(pages, out);
        return tracks(ui, state, pages, out);
    }
    let words = shelf::words(&pages.library_filter);
    let is_list = list_view(ui.ctx());
    match tab {
        LibraryTab::All if words.is_empty() => overview(ui, state, pages, out, is_list),
        LibraryTab::All => found(ui, state, pages, out, &words, is_list),
        LibraryTab::Playlists => playlists_tab(ui, state, pages, out, &words, is_list),
        LibraryTab::Albums => albums_tab(ui, state, pages, out, &words, is_list),
        LibraryTab::Artists => artists_tab(ui, state, pages, out, &words, is_list),
        LibraryTab::Tracks | LibraryTab::Trash => None,
    }
}

/// «Треки» asks the runtime for what the filter matches, once per change.
fn filter_tracks(pages: &mut PageState, out: &mut Vec<Command>) {
    let q = pages.library_filter.trim();
    if q != pages.track_filter_sent {
        pages.track_filter_sent = q.to_owned();
        out.push(Command::FilterTracks(q.to_owned()));
    }
}

/// The title, then at the end of its row the tools of the tab: the
/// filter, «только скачанное», the view and the sort, close together.
fn header(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let tab = pages.library_tab;
    let lib = &state.library;
    ui.add_space(8.0);
    let row = vec2(ui.available_width(), 56.0);
    let mut filter_below = false;
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(Align::Center), |ui| {
        ui.label(TypeRole::HeadlineMedium.style().rich("Библиотека", s.on_surface));
        ui.add_space(24.0);
        if tab == LibraryTab::Trash {
            return;
        }
        // The filter stays where it is from tab to tab, right after the
        // title; the tools of the tab come and go at the end of the row.
        let narrow = ui.available_width() < 640.0;
        let sorts = matches!(tab, LibraryTab::All | LibraryTab::Albums);
        let views = tab != LibraryTab::Tracks;
        let sort_w = match (sorts, narrow) {
            (false, _) => 0.0,
            (true, true) => 48.0,
            (true, false) => {
                let label = SORTS.iter().find(|s| s.0 == lib.release_sort).map_or("", |s| s.1);
                let text = ui.painter().layout_no_wrap(label.to_owned(), TypeRole::LabelLarge.font_id(), s.primary);
                text.size().x + 72.0
            }
        };
        let tools = 40.0 + if views { 90.0 } else { 0.0 } + sort_w;
        let w = (ui.available_width() - tools - 16.0).min(480.0);
        if w >= 200.0 {
            filter_field(ui, pages, w);
        } else {
            filter_below = true;
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // The sort orders the album grid (the table sorts by its header).
            if sorts {
                sort_button(ui, lib.release_sort, narrow, out);
            }
            if views {
                let is_list = list_view(ui.ctx());
                // The group lays out left to right inside this right-to-left row.
                let picked = ui
                    .allocate_ui_with_layout(vec2(82.0, 40.0), egui::Layout::left_to_right(Align::Center), |ui| {
                        ConnectedGroup::new("lib-view", &["Сетка", "Список"])
                            .icons(&[Icon::GridView, Icon::ViewList])
                            .show(ui, usize::from(is_list))
                    })
                    .inner;
                if let Some(i) = picked {
                    set_list_view(ui.ctx(), i == 1);
                }
            }
            only_downloaded(ui, state, out);
        });
    });
    if filter_below {
        ui.add_space(8.0);
        let w = ui.available_width();
        ui.allocate_ui_with_layout(vec2(w, 40.0), egui::Layout::left_to_right(Align::Center), |ui| {
            filter_field(ui, pages, w);
        });
    }
}

fn sort_button(ui: &mut Ui, cur: ReleaseSort, narrow: bool, out: &mut Vec<Command>) {
    let label = SORTS.iter().find(|s| s.0 == cur).map_or("Сортировка", |s| s.1);
    let resp = if narrow {
        ui.add(IconButton::new(Icon::Sort).tooltip(format!("Сортировка: {}", label.to_lowercase())).id_salt("lib-sort"))
    } else {
        ui.add(crate::components::Button::new(label).icon(Icon::Sort).style(Style::Text).id_salt("lib-sort"))
    };
    let mut menu = Menu::new("lib-sort-menu").align_right(true);
    for (sort, label) in SORTS {
        menu = menu.item(MenuItem::new(label).selected(cur == sort));
    }
    if let Some(i) = menu.attach(&resp)
        && SORTS[i].0 != cur
    {
        out.push(Command::SortReleases(SORTS[i].0));
    }
}

/// «Только скачанное» (docs/stage3.md §0): a filled toggle icon button,
/// its tooltip saying what it does in either state.
fn only_downloaded(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let on = state.prefs.library_only_downloaded;
    let resp = ui.add(
        IconButton::new(Icon::DownloadDone)
            .style(Style::Filled)
            .selected(on)
            .tooltip(if on {
                "Только скачанное: стрим скрыт. Нажми, чтобы показать всё"
            } else {
                "Только скачанное: скрыть то, что играет из сервисов"
            })
            .id_salt("lib-only-downloaded"),
    );
    if resp.clicked() {
        out.push(Command::SetPref(nikgob_view::PrefPatch::LibraryOnlyDownloaded(!on)));
    }
}

/// What the filter looks through on each tab.
fn filter_hint(tab: LibraryTab) -> &'static str {
    match tab {
        LibraryTab::Playlists => "Найти плейлист",
        LibraryTab::Albums => "Найти альбом или исполнителя",
        LibraryTab::Artists => "Найти исполнителя",
        LibraryTab::Tracks => "Найти трек, альбом, исполнителя",
        LibraryTab::All | LibraryTab::Trash => "Найти в библиотеке",
    }
}

/// The instant filter: a 40 dp pill (`surface-container-high`, a search
/// icon, the text, ✕ while there is some). Esc clears it.
fn filter_field(ui: &mut Ui, pages: &mut PageState, width: f32) {
    let theme = Theme::get(ui.ctx());
    let (s, motion) = (theme.scheme, theme.motion);
    let id = Id::new("lib-filter");
    let edit_id = id.with("edit");
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    let bg = ui.interact(rect, id.with("bg"), Sense::click()).on_hover_cursor(egui::CursorIcon::Text);
    if bg.clicked() {
        ui.memory_mut(|m| m.request_focus(edit_id));
    }
    let radii = Radii::all(shape::FULL);
    paint::container(ui.painter(), rect, radii, s.surface_container_high, Stroke::NONE);
    let layer = anim::fraction(
        ui.ctx(),
        id.with("layer"),
        if bg.contains_pointer() { state_layer::HOVER } else { 0.0 },
        motion.fast_effects,
    );
    paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);
    Icon::Search
        .widget(false, s.on_surface_variant, 20.0)
        .paint_at(ui, Rect::from_center_size(pos2(rect.left() + 22.0, rect.center().y), vec2(20.0, 20.0)));

    let text = &mut pages.library_filter;
    let has_text = !text.is_empty();
    let row_h = ui.ctx().fonts_mut(|f| f.row_height(&TypeRole::BodyLarge.font_id()));
    let text_rect = Rect::from_min_max(
        pos2(rect.left() + 44.0, rect.center().y - row_h * 0.5),
        pos2(rect.right() - if has_text { 44.0 } else { 16.0 }, rect.center().y + row_h * 0.5),
    );
    let edit = ui
        .scope_builder(UiBuilder::new().max_rect(text_rect), |ui| {
            TextEdit::singleline(text)
                .id(edit_id)
                .frame(Frame::NONE)
                .margin(Margin::ZERO)
                .font(TypeRole::BodyLarge.font_id())
                .text_color(s.on_surface)
                .desired_width(text_rect.width())
                .vertical_align(Align::Center)
                .hint_text(
                    RichText::new(if width < 300.0 { "Найти" } else { filter_hint(pages.library_tab) })
                        .color(s.on_surface_variant),
                )
                .show(ui)
        })
        .inner
        .response
        .response;
    // Esc takes the focus away from the field; it also empties it.
    if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
        text.clear();
    }
    if has_text {
        let slot = Rect::from_center_size(pos2(rect.right() - 22.0, rect.center().y), vec2(32.0, 32.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt("lib-filter-clear"));
        if child
            .add(IconButton::new(Icon::Close).size(Size::Xs).tooltip("Сбросить фильтр (Esc)").id_salt("lib-filter-x"))
            .clicked()
        {
            text.clear();
            edit.request_focus();
        }
    }
}

fn tabs(ui: &mut Ui, pages: &mut PageState, track_sort: TrackSort, out: &mut Vec<Command>) {
    let labels: Vec<&str> = TABS.iter().map(|t| t.1).collect();
    let idx = TABS.iter().position(|t| t.0 == pages.library_tab).unwrap_or(0);
    let picked = ui
        .allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(Align::Center), |ui| {
            ConnectedGroup::new("lib-tabs", &labels).show(ui, idx)
        })
        .inner;
    if let Some(i) = picked {
        open_tab(pages, TABS[i].0, track_sort, out);
    }
}

fn open_tab(pages: &mut PageState, tab: LibraryTab, track_sort: TrackSort, out: &mut Vec<Command>) {
    pages.library_tab = tab;
    if tab == LibraryTab::Tracks {
        out.push(Command::LoadTracks { offset: 0, limit: 200, sort: track_sort });
    }
    if tab == LibraryTab::Trash {
        out.push(Command::LoadTrash);
    }
}

/// A quiet line at the top of a tab: what is there, and the tab's own
/// controls at its end.
fn count_line(ui: &mut Ui, text: &str, add: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(Align::Center), |ui| {
        ui.label(TypeRole::LabelLarge.style().rich(text, s.on_surface_variant));
        ui.with_layout(egui::Layout::right_to_left(Align::Center), add);
    });
    ui.add_space(8.0);
}

fn count(n: usize, one: &str, few: &str, many: &str) -> String {
    format!("{n} {}", common::plural(n as u64, one, few, many))
}

/// «Ничего не найдено» for the filter, with «Сбросить» (also the chips).
fn no_results(ui: &mut Ui, pages: &mut PageState) {
    let q = pages.library_filter.trim().to_owned();
    let among = match pages.library_tab {
        LibraryTab::Playlists => "Среди плейлистов",
        LibraryTab::Albums => "Среди альбомов",
        LibraryTab::Artists => "Среди исполнителей",
        LibraryTab::Tracks => "Среди треков",
        LibraryTab::All | LibraryTab::Trash => "В библиотеке",
    };
    let body = if q.is_empty() {
        format!("{among} с этими фильтрами пусто.")
    } else {
        format!("{among} нет «{q}». Проверь, как написано, или поищи везде — в строке поиска сверху.")
    };
    if widgets::empty_state(ui, Icon::Search, "Ничего не найдено", &body, Some("Сбросить")) {
        pages.library_filter.clear();
        pages.release_filter = 0;
        pages.release_source = 0;
    }
}

/// «Всё»: the library at a glance, every section leading on to its tab.
fn overview(
    ui: &mut Ui,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
    is_list: bool,
) -> Option<Route> {
    let lib = &state.library;
    let mut nav = None;
    widgets::edition_offers(ui, state, None, out);
    let cols = widgets::columns(ui.available_width(), widgets::CARD_W);
    let sections = shelf::overview(shelf::Counts {
        recent: lib.recently_added.len(),
        often: lib.often_played.len(),
        albums: lib.releases.len(),
        artists: lib.artists.len(),
    });
    for (n, section) in sections.into_iter().enumerate() {
        if n > 0 {
            ui.add_space(widgets::SECTION_GAP);
        }
        match section {
            shelf::Overview::Playlists => {
                if widgets::section_header(ui, "Плейлисты", true) {
                    open_tab(pages, LibraryTab::Playlists, lib.track_sort, out);
                }
                // «Новый плейлист» first, then every playlist.
                widgets::card_row_virtual(ui, "lib-playlists", lib.playlists.len() + 1, widgets::CARD_W, |ui, i| match i {
                    0 => new_playlist_card(ui, pages, widgets::CARD_W),
                    i => {
                        let p = &lib.playlists[i - 1];
                        nav = widgets::playlist_card_with(ui, p, widgets::CARD_W, "lib", Some(&mut *pages), out).or(nav.take());
                    }
                });
            }
            shelf::Overview::RecentlyAdded => {
                if widgets::section_header(ui, "Недавно добавлено", true) {
                    // The whole list in the same order.
                    if lib.release_sort != ReleaseSort::Added {
                        out.push(Command::SortReleases(ReleaseSort::Added));
                    }
                    open_tab(pages, LibraryTab::Albums, lib.track_sort, out);
                }
                widgets::card_row(ui, "lib-recent", |ui| {
                    for r in &lib.recently_added {
                        nav = widgets::release_card(ui, r, widgets::CARD_W, "recent", out).or(nav.take());
                    }
                });
            }
            shelf::Overview::OftenPlayed => {
                // A top of its own: no tab holds this order.
                widgets::section_header(ui, "Часто слушаешь", false);
                widgets::card_row(ui, "lib-often", |ui| {
                    for r in &lib.often_played {
                        nav = widgets::release_card(ui, r, widgets::CARD_W, "often", out).or(nav.take());
                    }
                });
            }
            shelf::Overview::Albums => {
                let total = lib.releases.len();
                let shown = shelf::overview_albums(total, if is_list { 4 } else { cols });
                let all = format!("Все {total}");
                let action = (shown < total).then_some((all.as_str(), Icon::ArrowForward));
                if widgets::section_header_with(ui, "Альбомы", action) {
                    open_tab(pages, LibraryTab::Albums, lib.track_sort, out);
                }
                let first: Vec<&ReleaseCard> = lib.releases.iter().take(shown).collect();
                nav = release_items(ui, &first, is_list, "ov", out).or(nav);
            }
            shelf::Overview::Artists => {
                if widgets::section_header(ui, "Исполнители", true) {
                    open_tab(pages, LibraryTab::Artists, lib.track_sort, out);
                }
                widgets::card_row_virtual(ui, "lib-artists", lib.artists.len(), AVATAR, |ui, i| {
                    nav = widgets::artist_avatar(ui, &lib.artists[i], AVATAR, out).or(nav.take());
                });
            }
        }
    }
    nav
}

/// «Всё» with the filter: what matches, by kind; the tracks are one click
/// away on their tab.
fn found(
    ui: &mut Ui,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
    words: &[String],
    is_list: bool,
) -> Option<Route> {
    let lib = &state.library;
    let playlists: Vec<&PlaylistCard> = lib.playlists.iter().filter(|p| shelf::playlist_matches(words, p)).collect();
    let releases: Vec<&ReleaseCard> = lib.releases.iter().filter(|r| shelf::release_matches(words, r)).collect();
    let artists: Vec<&ArtistCard> = lib.artists.iter().filter(|a| shelf::artist_matches(words, a)).collect();
    let mut nav = None;
    let nothing = playlists.is_empty() && releases.is_empty() && artists.is_empty();
    if !nothing {
        if !artists.is_empty() {
            widgets::section_header(ui, &format!("Исполнители · {}", artists.len()), false);
            nav = artist_items(ui, &artists, is_list, out).or(nav);
            ui.add_space(widgets::SECTION_GAP);
        }
        if !releases.is_empty() {
            widgets::section_header(ui, &format!("Альбомы · {}", releases.len()), false);
            nav = release_items(ui, &releases, is_list, "found", out).or(nav);
            ui.add_space(widgets::SECTION_GAP);
        }
        if !playlists.is_empty() {
            widgets::section_header(ui, &format!("Плейлисты · {}", playlists.len()), false);
            nav = playlist_items(ui, &playlists, pages, is_list, out).or(nav);
            ui.add_space(widgets::SECTION_GAP);
        }
    } else {
        no_results(ui, pages);
        ui.add_space(16.0);
    }
    // Tracks are searched by the runtime: their own tab, the filter kept.
    let open = ui
        .vertical_centered(|ui| {
            ui.add(
                crate::components::Button::new("Искать среди треков")
                    .icon(Icon::MusicNote)
                    .style(Style::Tonal)
                    .id_salt("lib-found-tracks"),
            )
            .clicked()
        })
        .inner;
    if open {
        open_tab(pages, LibraryTab::Tracks, lib.track_sort, out);
    }
    nav
}

fn playlists_tab(
    ui: &mut Ui,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
    words: &[String],
    is_list: bool,
) -> Option<Route> {
    let lib = &state.library;
    let shown: Vec<&PlaylistCard> = lib.playlists.iter().filter(|p| shelf::playlist_matches(words, p)).collect();
    if !words.is_empty() && shown.is_empty() {
        no_results(ui, pages);
        return None;
    }
    count_line(ui, &count(shown.len(), "плейлист", "плейлиста", "плейлистов"), |_| {});
    // «Новый плейлист» first, unless the list is filtered.
    if words.is_empty() {
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
        } else {
            return playlist_grid(ui, &shown, pages, true, out);
        }
    }
    playlist_items(ui, &shown, pages, is_list, out)
}

fn albums_tab(
    ui: &mut Ui,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
    words: &[String],
    is_list: bool,
) -> Option<Route> {
    let lib = &state.library;
    widgets::edition_offers(ui, state, None, out);
    let shown: Vec<&ReleaseCard> = lib
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
        .filter(|r| shelf::release_matches(words, r))
        .collect();
    album_chips(ui, state, pages, shown.len());
    if shown.is_empty() {
        no_results(ui, pages);
        return None;
    }
    // Headers that follow the sort: letters joined to fill a row, decades,
    // or how long ago the albums came in.
    let sort = lib.release_sort;
    let cols = if is_list { 4 } else { widgets::columns(ui.available_width(), widgets::CARD_W) };
    let now = now_ms();
    let keys: Vec<String> = shown.iter().map(|r| shelf::group_key(sort, r, now)).collect();
    // Letters join into spans of two rows or more.
    let groups = shelf::groups(sort, &keys, cols * 2);
    let mut nav = None;
    if !shelf::worth_headers(groups.len(), shown.len(), is_list, cols) {
        return release_items(ui, &shown, is_list, "grid", out);
    }
    for (n, g) in groups.iter().enumerate() {
        group_label(ui, &g.label, n == 0);
        nav = release_items(ui, &shown[g.range.clone()], is_list, &g.label, out).or(nav);
    }
    nav
}

/// The kind and source chips of «Альбомы», the count at the end.
fn album_chips(ui: &mut Ui, state: &ViewState, pages: &mut PageState, n: usize) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (idx, label) in ["Всё", "Альбомы", "Синглы", "EP"].iter().enumerate() {
            if ui.add(FilterChip::new(*label, pages.release_filter == idx).id_salt(("rel-filter", idx))).clicked() {
                pages.release_filter = idx;
            }
        }
        // Where the albums play from: files, or only a service (no use
        // while «только скачанное» hides the stream anyway).
        if !state.prefs.library_only_downloaded {
            const SOURCES: [&str; 3] = ["Все источники", "Скачанные", "Стрим"];
            let cur = pages.release_source.min(2);
            let chip = ui.add(FilterChip::new(SOURCES[cur], cur != 0).trailing_icon(Icon::ExpandMore).id_salt("rel-source"));
            let mut menu = Menu::new("rel-source-menu");
            for (i, l) in SOURCES.iter().enumerate() {
                menu = menu.item(MenuItem::new(*l).selected(i == cur));
            }
            if let Some(i) = menu.attach(&chip) {
                pages.release_source = i;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.label(TypeRole::LabelLarge.style().rich(count(n, "альбом", "альбома", "альбомов"), s.on_surface_variant));
        });
    });
    ui.add_space(8.0);
}

/// A header over a run of the album grid («A – D», «1990-е», «За неделю»).
fn group_label(ui: &mut Ui, label: &str, first: bool) {
    let s = Theme::get(ui.ctx()).scheme;
    if !first {
        ui.add_space(12.0);
    }
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(Align::Center), |ui| {
        ui.label(TypeRole::TitleMedium.style().rich(label, s.on_surface));
    });
    ui.add_space(4.0);
}

fn artists_tab(
    ui: &mut Ui,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
    words: &[String],
    is_list: bool,
) -> Option<Route> {
    let shown: Vec<&ArtistCard> = state.library.artists.iter().filter(|a| shelf::artist_matches(words, a)).collect();
    if shown.is_empty() {
        if words.is_empty() {
            widgets::empty_state(ui, Icon::Person, "Исполнителей пока нет", "Они появятся вместе с первыми треками.", None);
        } else {
            no_results(ui, pages);
        }
        return None;
    }
    count_line(ui, &count(shown.len(), "исполнитель", "исполнителя", "исполнителей"), |_| {});
    artist_items(ui, &shown, is_list, out)
}

/// «+ Новый плейлист»: an outlined card the size of the others.
fn new_playlist_card(ui: &mut Ui, pages: &mut PageState, w: f32) {
    let s = Theme::get(ui.ctx()).scheme;
    let o = Card::outlined()
        .clickable(true)
        .width(w)
        .min_height(w + 56.0)
        .label("Новый плейлист")
        .id_salt(("new-playlist-card", w as u32))
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
}

/// Playlists as a grid (with «Новый плейлист» first when `new`).
fn playlist_grid(
    ui: &mut Ui,
    items: &[&PlaylistCard],
    pages: &mut PageState,
    new: bool,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let width = ui.available_width();
    let cols = widgets::columns(width, widgets::CARD_W);
    let w = ((width - widgets::CARD_GAP * (cols - 1) as f32) / cols as f32).floor();
    let mut nav = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(widgets::CARD_GAP, widgets::CARD_GAP);
        if new {
            new_playlist_card(ui, pages, w);
        }
        for p in items {
            nav = widgets::playlist_card_with(ui, p, w, "lib", Some(pages), out).or(nav.take());
        }
    });
    nav
}

fn playlist_items(
    ui: &mut Ui,
    items: &[&PlaylistCard],
    pages: &mut PageState,
    is_list: bool,
    out: &mut Vec<Command>,
) -> Option<Route> {
    if !is_list {
        return playlist_grid(ui, items, pages, false, out);
    }
    let mut nav = None;
    for p in items {
        nav = playlist_row(ui, p, pages, out).or(nav);
    }
    nav
}

fn release_items(ui: &mut Ui, items: &[&ReleaseCard], is_list: bool, row: &str, out: &mut Vec<Command>) -> Option<Route> {
    if is_list {
        let mut nav = None;
        for r in items {
            nav = release_row(ui, r, out).or(nav);
        }
        return nav;
    }
    widgets::grid(ui, items, widgets::CARD_W, |_, w| crate::components::MediaCard::height(w), |ui, r, w| widgets::release_card(ui, r, w, row, out))
}

fn artist_items(ui: &mut Ui, items: &[&ArtistCard], is_list: bool, out: &mut Vec<Command>) -> Option<Route> {
    if is_list {
        let mut nav = None;
        for a in items {
            nav = artist_row(ui, a, out).or(nav);
        }
        return nav;
    }
    widgets::grid(ui, items, AVATAR, |a, w| widgets::artist_avatar_height(a, w), |ui, a, w| widgets::artist_avatar(ui, a, w, out))
}

/// A small tonal label for list rows («3 из 12», «стрим»).
fn meta_badge(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    let g = plain(ui.painter(), text, TypeRole::LabelMedium.style(), s.on_secondary_container);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 16.0, 24.0), Sense::hover());
    paint::container(ui.painter(), rect, Radii::all(shape::SMALL), s.secondary_container, Stroke::NONE);
    ui.painter().galley(rect.center() - g.size() * 0.5, g, s.on_secondary_container);
}

/// A fixed-width cell of the list rows' meta, right-aligned so the columns
/// line up from row to row.
fn meta_cell(ui: &mut Ui, w: f32, add: impl FnOnce(&mut Ui)) {
    let (rect, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    let mut cell = ui.new_child(UiBuilder::new().max_rect(rect).layout(egui::Layout::right_to_left(Align::Center)));
    add(&mut cell);
}

/// List view: an album row (cover with ▶ on hover, title, artist; at the
/// end the badge, the number of tracks and the year).
fn release_row(ui: &mut Ui, r: &ReleaseCard, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let badge = widgets::card_badge(r);
    let wide = ui.available_width() >= 640.0;
    let tracks = r.total_tracks.unwrap_or(r.local_tracks);
    let o = ListItem::new(&r.title)
        .supporting(&r.artist)
        .leading(Leading::Image(r.cover.as_ref().map(|c| c.uri()), Icon::Album))
        .hover_play(true)
        .trailing(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            meta_cell(ui, 80.0, |ui| {
                if let Some(b) = &badge {
                    meta_badge(ui, b);
                }
            });
            if wide {
                meta_cell(ui, 88.0, |ui| {
                    if tracks > 0 {
                        ui.label(TypeRole::BodyMedium.style().rich(count(tracks as usize, "трек", "трека", "треков"), s.on_surface_variant));
                    }
                });
            }
            meta_cell(ui, 40.0, |ui| {
                if let Some(y) = r.year {
                    ui.label(TypeRole::BodyMedium.style().rich(y.to_string(), s.on_surface_variant));
                }
            });
        })
        .id_salt(("rel-row", r.id))
        .show(ui);
    let menu = widgets::release_menu(&o.response, r, out);
    if o.play_clicked {
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
    let mut line = count(p.track_count as usize, "трек", "трека", "треков");
    if let Some(service) = p.service {
        line = format!("{} · {line}", service.display_name());
    }
    let o = ListItem::new(&p.name)
        .supporting(line)
        .leading(Leading::Image(
            p.cover.as_ref().map(|c| c.uri()),
            if p.pinned { Icon::Favorite } else { Icon::QueueMusic },
        ))
        .hover_play(true)
        .id_salt(("pl-row", p.id))
        .show(ui);
    widgets::playlist_menu_at(&o.response, p, Some(pages), out);
    if o.play_clicked {
        out.push(Command::PlayPlaylist { id: p.id, start: None, shuffle: false });
        None
    } else if o.response.clicked() {
        Some(Route::Playlist(p.id))
    } else {
        None
    }
}

/// List view: an artist row (the avatar as in the grid, ▶ over it).
fn artist_row(ui: &mut Ui, a: &ArtistCard, out: &mut Vec<Command>) -> Option<Route> {
    let mut item = ListItem::new(&a.name).leading(widgets::artist_leading(ui, a)).hover_play(true);
    let tracks = widgets::artist_tracks(a);
    if !tracks.is_empty() {
        item = item.supporting(tracks);
    }
    let o = item.id_salt(("ar-row", a.id)).show(ui);
    widgets::artist_menu(&o.response, a, out);
    if o.play_clicked {
        out.push(Command::PlayArtist { id: a.id, shuffle: false });
        None
    } else {
        o.response.clicked().then_some(Route::Artist(a.id))
    }
}

/// "Корзина": deleted tracks with "Восстановить" and "Удалить навсегда";
/// everything goes for good after 30 days.
fn trash(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let items = &state.library.trash;
    let n = items.len();
    count_line(ui, &format!("{} в корзине", count(n, "трек", "трека", "треков")), |ui| {
        if n == 0 {
            return;
        }
        if ui
            .add(crate::components::Button::new("Очистить корзину").icon(Icon::Delete).style(Style::Tonal).id_salt("trash-empty"))
            .clicked()
        {
            pages.confirm_empty_trash = true;
        }
        if ui.add(crate::components::Button::new("Восстановить всё").style(Style::Text).id_salt("trash-restore-all")).clicked() {
            out.push(Command::RestoreTracks(items.iter().map(|i| i.track.id).collect()));
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
                        .add(crate::components::Button::new("Восстановить").size(Size::S).style(Style::Tonal).id_salt(("trash-restore", i)))
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
    // With the filter on, its hits (best first, no windows to load).
    let query = pages.library_filter.trim().to_owned();
    let hits: Option<&[TrackRow]> = (!query.is_empty()).then(|| lib.track_hits.as_ref().map_or(&[][..], |(_, rows)| &rows[..]));
    let waiting = hits.is_some() && lib.track_hits.as_ref().is_none_or(|(q, _)| *q != query);
    let (total, offset, rows) = match hits {
        Some(rows) => (rows.len(), 0, rows),
        None => (lib.track_count, lib.tracks_offset, &lib.tracks[..]),
    };
    if hits.is_some() && total == 0 {
        if waiting {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| ui.add(LoadingIndicator::new()));
        } else {
            no_results(ui, pages);
        }
        return None;
    }
    let summary = match hits {
        Some(_) => format!("Нашлось: {}", count(total, "трек", "трека", "треков")),
        None => count(total, "трек", "трека", "треков"),
    };
    count_line(ui, &summary, |_| {});
    if total == 0 {
        widgets::empty_state(ui, Icon::MusicNote, "Треков пока нет", "Лайкни трек в поиске или добавь папку с музыкой.", None);
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
    let have = offset..offset + rows.len();
    if hits.is_none() && first < last && (first < have.start || last > have.end) && pages.track_window != (first, last) {
        pages.track_window = (first, last);
        let offset = first.saturating_sub(50);
        out.push(Command::LoadTracks { offset, limit: (last - offset) + 150, sort: lib.track_sort });
    }

    let ids: Vec<_> = rows.iter().map(|t| t.id).collect();
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
        let track = i.checked_sub(offset).and_then(|j| rows.get(j));
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
                    start: i - offset,
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
        let by = by.filter(|_| hits.is_none());
        let active = by.is_some() && by == Some(lib.track_sort);
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
