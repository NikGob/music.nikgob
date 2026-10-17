//! Поиск (md3e-player.md §3.6, docs/stage3.md §3.2): a docked search view
//! (radius 28, 56 dp header with source filter chips) under the bar.
//!
//! «Всё» is one answer ([`nikgob_view::SearchAnswer`]): «Лучший результат»
//! (a large card with ▶) and the first tracks beside it on a wide window
//! (under it on a narrow one), then more tracks («Ещё»), albums, artists,
//! playlists — the library and every usable service merged, without a
//! header per service; empty parts are left out, a service that cannot
//! answer says nothing, one that waits says so in one quiet line. Torrents
//! run only on request (the button, or Enter once Prowlarr is set up).
//!
//! A chip shows one source the old way: Библиотека, Deezer, YouTube Music,
//! Spotify (only while it is connected) — each its own section — or
//! Торренты. A service that cannot answer says why quietly there.

use egui::{Align, Id, Layout, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_shapes::MaterialShape;
use nikgob_view::{
    Account, Command, Route, SearchAlbum, SearchAnswer, SearchArtist, SearchPick, SearchPlaylist, SearchSources,
    SearchTrack, Service, StreamPage, StreamSearchSection, TOP_TRACKS, TopResult, TorrentResult, ViewState,
};

use super::{PageState, fmt_size, streams, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, FilterChip, IconButton, ListItem, LoadingIndicator, Radii, Size, Style, anim, badge, paint},
    screens::common,
    tokens::shape,
};
use widgets::{Lead, PlayFrom};

/// Which sources the view shows (one chip at a time). The services asked
/// follow it ([`Command::SetSearchSources`]); torrents run on request only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Library,
    Deezer,
    Youtube,
    Spotify,
    Torrents,
}

impl Filter {
    /// The sources this filter asks.
    fn sources(self) -> SearchSources {
        let only = |library, service: Option<Service>| SearchSources {
            library,
            spotify: service == Some(Service::Spotify),
            youtube: service == Some(Service::Youtube),
            deezer: service == Some(Service::Deezer),
            torrents: false,
            mixed: false,
        };
        match self {
            Self::All => SearchSources::all(),
            Self::Library => only(true, None),
            Self::Deezer | Self::Youtube | Self::Spotify => only(false, self.service()),
            // The library stays for context; the indexer is asked by its button.
            Self::Torrents => only(true, None),
        }
    }

    fn service(self) -> Option<Service> {
        match self {
            Self::Spotify => Some(Service::Spotify),
            Self::Youtube => Some(Service::Youtube),
            Self::Deezer => Some(Service::Deezer),
            _ => None,
        }
    }
}

const FILTER_KEY: &str = "md3e-search-filter";

pub fn filter(ctx: &egui::Context) -> Filter {
    ctx.data(|d| d.get_temp(Id::new(FILTER_KEY))).unwrap_or_default()
}

pub fn set_filter(ctx: &egui::Context, f: Filter) {
    ctx.data_mut(|d| d.insert_temp(Id::new(FILTER_KEY), f));
}

/// How far the results are scrolled (kept every frame), and where to put
/// them once they are back (going back to the search from a page).
const SCROLL_KEY: &str = "md3e-search-scroll";
const RESTORE_KEY: &str = "md3e-search-scroll-restore";

/// How far the search results are scrolled now.
pub fn search_scroll(ctx: &egui::Context) -> f32 {
    ctx.data(|d| d.get_temp(Id::new(SCROLL_KEY))).unwrap_or(0.0)
}

/// Scrolls the results to `y` once they are there again.
pub fn restore_search_scroll(ctx: &egui::Context, y: f32) {
    ctx.data_mut(|d| d.insert_temp(Id::new(RESTORE_KEY), y));
}

const HEADER: f32 = 56.0;

/// A service is connected and can answer (its chip shows).
fn usable(state: &ViewState, service: Service) -> bool {
    state.account(service).is_some_and(|a| a.registered && a.usable)
}

pub fn search_results(
    ui: &mut Ui,
    query: &str,
    state: &ViewState,
    bottom_inset: f32,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    let area = ui.max_rect();
    let panel = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - 16.0));
    paint::container(ui.painter(), panel, Radii::all(shape::EXTRA_LARGE), s.surface_container_low, Stroke::NONE);

    // Header: source filters. Spotify has a chip only while connected; a
    // chip that went away falls back to «Всё».
    let header = Rect::from_min_size(panel.min, vec2(panel.width(), HEADER));
    let mut f = filter(&ctx);
    let spotify = usable(state, Service::Spotify);
    let deezer = state.account(Service::Deezer).is_some_and(|a| a.registered);
    if (f == Filter::Spotify && !spotify) || (f == Filter::Deezer && !deezer) {
        f = Filter::All;
    }
    let mut hui = ui.new_child(
        UiBuilder::new()
            .max_rect(header.shrink2(vec2(16.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center))
            .id_salt("search-chips"),
    );
    egui::ScrollArea::horizontal()
        .id_salt("search-chips-scroll")
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(&mut hui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let mut chips = vec![("Всё", Filter::All), ("Библиотека", Filter::Library)];
                if deezer {
                    chips.push(("Deezer", Filter::Deezer));
                }
                chips.push(("YouTube Music", Filter::Youtube));
                if spotify {
                    chips.push(("Spotify", Filter::Spotify));
                }
                chips.push(("Торренты", Filter::Torrents));
                for (name, chip) in chips {
                    if ui.add(FilterChip::new(name, f == chip).id_salt(("sf", name))).clicked() {
                        f = chip;
                    }
                }
            });
        });

    // Body.
    let body = Rect::from_min_max(
        pos2(panel.left() + 16.0, header.bottom() + 1.0),
        pos2(panel.right() - 16.0, panel.bottom()),
    );
    let mut bui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("search-body"));
    bui.set_clip_rect(body.intersect(ui.clip_rect()));
    let mut nav = None;
    let mut area = egui::ScrollArea::vertical().id_salt(("search-scroll", f as u8)).auto_shrink([false, false]);
    // Back from a page: where the results were, once every source has
    // answered again (until then they are shorter).
    let settled = state.search.query.trim() == query.trim()
        && !state.search.searching
        && !state.search.answer.searching
        && state.search.streams.iter().all(|x| !x.searching);
    if settled && let Some(y) = ctx.data_mut(|d| d.remove_temp::<f32>(Id::new(RESTORE_KEY))) {
        area = area.vertical_scroll_offset(y);
    }
    let scrolled = area
        .show(&mut bui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(8.0);
            match f {
                Filter::All => {
                    nav = all_page(ui, query, state, pages, out).or(nav.take());
                    nav = torrents(ui, query, state, pages, out).or(nav.take());
                }
                Filter::Library => {
                    nav = library(ui, query, state, pages, out).or(nav.take());
                }
                Filter::Deezer | Filter::Youtube | Filter::Spotify => {
                    if let Some(service) = f.service() {
                        nav = stream_section(ui, query, service, state, pages, out).or(nav.take());
                    }
                }
                Filter::Torrents => {
                    nav = library(ui, query, state, pages, out).or(nav.take());
                    nav = torrents(ui, query, state, pages, out).or(nav.take());
                }
            }
            ui.add_space(bottom_inset);
        })
        .state
        .offset
        .y;
    ctx.data_mut(|d| d.insert_temp(Id::new(SCROLL_KEY), scrolled));
    set_filter(&ctx, f);
    // The services asked follow the chip (the runtime searches again).
    let wanted = f.sources();
    if wanted != state.search.sources {
        let asked = ctx.data(|d| d.get_temp::<SearchSources>(Id::new("md3e-search-asked")));
        if asked != Some(wanted) {
            ctx.data_mut(|d| d.insert_temp(Id::new("md3e-search-asked"), wanted));
            out.push(Command::SetSearchSources(wanted));
        }
    }
    ui.painter().line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, s.outline_variant),
    );
    nav
}

/// A section's head: its name (title-large) with a small loading indicator
/// while its results are on their way (the previous ones stay below, so
/// nothing jumps while typing), and «Все →» to its chip under «Всё».
/// True when «Все» was clicked.
fn section_head(ui: &mut Ui, title: &str, loading: bool, all: bool) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let mut clicked = false;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
        ui.label(TypeRole::TitleLarge.style().rich(title, s.on_surface));
        ui.add_space(4.0);
        loading_mark(ui, loading);
        if all {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                clicked = ui
                    .add(Button::new("Все").style(Style::Text).icon(Icon::ArrowForward).id_salt(("search-all", title)))
                    .clicked();
            });
        }
    });
    clicked
}

/// A part of a section (Треки, Альбомы…), quieter than the section.
fn part_label(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(4.0);
    ui.label(TypeRole::TitleSmall.style().rich(text, s.on_surface_variant));
}

/// Height of the loading mark.
const LABEL_H: f32 = 20.0;

/// The section's loading indicator; the same room is kept when idle.
fn loading_mark(ui: &mut Ui, loading: bool) {
    if loading {
        ui.add(LoadingIndicator::new().size(LABEL_H));
    } else {
        ui.allocate_exact_size(vec2(LABEL_H, LABEL_H), egui::Sense::hover());
    }
}

/// A quiet line saying why a section is empty, with an optional text
/// button. True when the button was clicked.
fn quiet(ui: &mut Ui, text: &str, button: Option<(&str, Icon, &str)>) -> bool {
    widgets::quiet_line(ui, text, button.map(|(label, icon, salt)| (label, Some(icon), Style::Text, salt)))
}

/// The library under its chip (and the torrents' one).
fn library(
    ui: &mut Ui,
    query: &str,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let r = &state.search;
    let mut nav = None;
    let loading = r.query.trim() != query.trim() || r.searching;
    let empty = r.tracks.is_empty() && r.releases.is_empty() && r.artists.is_empty() && r.playlists.is_empty();
    section_head(ui, "Библиотека", loading, false);
    if empty && loading {
        return None;
    }
    if empty {
        widgets::empty_state(ui, Icon::Search, "Ничего не нашлось", "В библиотеке такого нет.", None);
        ui.add_space(widgets::SECTION_GAP);
        return None;
    }
    if !r.tracks.is_empty() {
        part_label(ui, "Треки");
        let ids = r.tracks.iter().map(|t| t.id).collect();
        nav = widgets::track_list(
            ui,
            "search",
            &r.tracks,
            Lead::Cover,
            &PlayFrom::Tracks(ids, format!("поиска: {}", r.query.trim())),
            state,
            pages,
            out,
        )
        .or(nav.take());
    }
    if !r.releases.is_empty() {
        part_label(ui, "Альбомы");
        widgets::card_row(ui, "search-releases", |ui| {
            for rel in &r.releases {
                nav = widgets::release_card(ui, rel, widgets::CARD_W, "search", out).or(nav.take());
            }
        });
    }
    if !r.artists.is_empty() {
        part_label(ui, "Исполнители");
        widgets::card_row(ui, "search-artists", |ui| {
            for a in &r.artists {
                nav = widgets::artist_avatar(ui, a, 120.0, out).or(nav.take());
            }
        });
    }
    if !r.playlists.is_empty() {
        part_label(ui, "Плейлисты");
        widgets::card_row(ui, "search-playlists", |ui| {
            for p in &r.playlists {
                nav = widgets::playlist_card_with(ui, p, widgets::CARD_W, "search", Some(&mut *pages), out)
                    .or(nav.take());
            }
        });
    }
    ui.add_space(widgets::SECTION_GAP);
    nav
}

fn torrents(ui: &mut Ui, query: &str, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let t = &state.search.torrents;
    let q = query.trim();
    let stale = t.query.trim() != q;
    section_head(ui, "Торренты", t.searching, false);

    if t.searching {
        ui.label(TypeRole::BodyMedium.style().rich_para(
            format!("Ищу раздачи «{}»… Prowlarr отвечает не сразу.", t.query),
            s.on_surface_variant,
        ));
        return None;
    }

    if !state.prefs.prowlarr_configured {
        if quiet(
            ui,
            "Для раздач нужен Prowlarr: адрес и ключ — в Настройках → Источники → Торренты.",
            Some(("Настроить", Icon::Settings, "tor-settings")),
        ) {
            pages.open_setting(super::SettingId::Prowlarr);
        }
        return None;
    }

    // Not searched for this text yet: offer the search.
    if stale && !q.is_empty() {
        ui.horizontal(|ui| {
            if ui
                .add(Button::new("Искать раздачи").style(Style::Tonal).icon(Icon::Search).id_salt("tor-search"))
                .clicked()
            {
                out.push(Command::SearchTorrents(q.to_owned()));
            }
            ui.label(TypeRole::BodySmall.style().rich("или Enter в строке поиска", s.on_surface_variant));
        });
    }

    if let Some(err) = &t.error {
        let retry = !stale && quiet(ui, err, Some(("Повторить", Icon::Refresh, "tor-retry")));
        if retry {
            out.push(Command::SearchTorrents(q.to_owned()));
        }
        return None;
    }

    if t.results.is_empty() {
        if !stale && !t.query.is_empty() {
            quiet(ui, "Раздач не нашлось.", None);
        }
        return None;
    }
    if stale {
        ui.label(TypeRole::BodySmall.style().rich(format!("Результаты для «{}»", t.query), s.on_surface_variant));
    }
    // Buttons fit beside the title only in a wide row.
    let wide = ui.available_width() >= 640.0;
    for r in &t.results {
        torrent_row(ui, r, wide, out);
    }
    None
}

fn torrent_row(ui: &mut Ui, r: &TorrentResult, wide: bool, out: &mut Vec<Command>) {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(r.format.clone());
    parts.extend(r.size_bytes.map(fmt_size));
    if let Some(n) = r.seeders {
        parts.push(format!("{n} {}", common::plural(u64::from(n), "сид", "сида", "сидов")));
    }
    parts.push(r.indexer.clone());
    let idx = r.index;
    let buttons = |ui: &mut Ui, out: &mut Vec<Command>| {
        if ui
            .add(Button::new("Выбрать файлы…").style(Style::Outlined).size(crate::components::Size::S).id_salt(("tor-files", idx)))
            .clicked()
        {
            out.push(Command::ListTorrentFiles(idx));
        }
        if ui
            .add(Button::new("Скачать").style(Style::Tonal).size(crate::components::Size::S).id_salt(("tor-dl", idx)))
            .clicked()
        {
            out.push(Command::DownloadTorrent { result: idx, files: None });
        }
    };
    ListItem::new(&r.title)
        .supporting(parts.join(" · "))
        .leading_icon(Icon::CloudDownload)
        .trailing(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if wide {
                    buttons(ui, out);
                }
                if r.recommended {
                    marker(ui, "рекоменд.");
                }
            });
        })
        .id_salt(("torrent", idx))
        .show(ui);
    if !wide {
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 40.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                buttons(ui, out);
            },
        );
        ui.add_space(8.0);
    }
}

/// A small tertiary badge ("рекоменд.").
fn marker(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), TypeRole::LabelSmall.font_id(), s.on_tertiary_container);
    let w = galley.size().x + 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(w.max(badge::LARGE_HEIGHT), badge::LARGE_HEIGHT), egui::Sense::hover());
    badge::text_badge(ui.painter(), rect.left_center(), text, s.tertiary_container, s.on_tertiary_container);
}

/// Why a service cannot answer now, said quietly, and what helps: off, not
/// in this build, not signed in (Spotify), paused by the service.
enum Why {
    Off,
    Missing,
    SignIn,
    SigningIn,
    Paused(String),
}

fn why_not(state: &ViewState, service: Service) -> Option<Why> {
    let acc = state.account(service);
    if !acc.is_some_and(|a| a.registered) {
        return Some(Why::Missing);
    }
    if !state.prefs.service_enabled(service) {
        return Some(Why::Off);
    }
    let a = acc?;
    if let Some(hm) = &a.paused_until_hm {
        return Some(Why::Paused(hm.clone()));
    }
    // Spotify answers nothing without a login (YouTube Music does).
    if service == Service::Spotify {
        match &a.account {
            Account::LoggingIn(_) => return Some(Why::SigningIn),
            Account::LoggedIn { .. } => {}
            Account::LoggedOut | Account::Failed(_) => return Some(Why::SignIn),
        }
    }
    None
}

/// The results of one service under its chip: tracks, albums, artists,
/// playlists.
fn stream_section(
    ui: &mut Ui,
    query: &str,
    service: Service,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let mut nav = None;
    let salt = service.as_str();
    let name = service.display_name();
    let sec: Option<&StreamSearchSection> = state.search.streams.iter().find(|x| x.service == service);
    let q = query.trim();
    let stale = state.search.query.trim() != q;
    // A section that waits out a rate limit by itself says so itself.
    let noted = sec.is_some_and(|x| x.note.is_some());
    let why = why_not(state, service).filter(|w| !(noted && matches!(w, Why::Paused(_))));
    let loading = !q.is_empty() && why.is_none() && (stale || sec.is_none_or(|x| x.searching));
    section_head(ui, name, loading, false);
    match why {
        Some(Why::Missing) => {
            quiet(ui, &format!("{name} нет в этой сборке."), None);
        }
        Some(Why::Off) => {
            if quiet(ui, &format!("{name} выключен."), Some(("Включить", Icon::Settings, &format!("sv-on-{salt}")))) {
                pages.open_setting(match service {
                    Service::Spotify => super::SettingId::SpotifyEnabled,
                    Service::Youtube => super::SettingId::YouTubeEnabled,
                    Service::Deezer => super::SettingId::DeezerEnabled,
                });
            }
        }
        // Under its own chip the sign-in is all there is to show.
        Some(Why::SignIn) => {
            if widgets::empty_state(
                ui,
                Icon::Login,
                &format!("Войди в {name}"),
                "Тогда поиск найдёт и там: треки, альбомы, исполнителей и плейлисты.",
                Some(&format!("Войти через {name}")),
            ) {
                pages.open_setting(super::SettingId::Spotify);
            }
        }
        Some(Why::SigningIn) => {
            quiet(ui, &format!("Вхожу в {name}…"), None);
        }
        Some(Why::Paused(hm)) => {
            quiet(ui, &format!("{name} просит подождать до {hm}."), None);
        }
        None => {
            nav = results(ui, q, service, sec, loading, state, pages, out);
        }
    }
    ui.add_space(widgets::SECTION_GAP);
    nav
}

#[allow(clippy::too_many_arguments)]
fn results(
    ui: &mut Ui,
    q: &str,
    service: Service,
    sec: Option<&StreamSearchSection>,
    loading: bool,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let salt = service.as_str();
    let mut nav = None;
    let empty = |x: &StreamSearchSection| {
        x.tracks.is_empty() && x.albums.is_empty() && x.artists.is_empty() && x.playlists.is_empty()
    };
    // What the section searched for (its rows are of that query while a
    // newer one runs).
    let shown_q = state.search.query.trim();
    // A rate limit: a quiet line, never a button (the runtime asks again).
    if !q.is_empty()
        && let Some(note) = sec.and_then(|x| x.note.as_deref())
    {
        quiet(ui, note, None);
    }
    match sec {
        _ if q.is_empty() => {}
        None => {}
        Some(x) if loading && empty(x) => {}
        Some(x) if x.note.is_some() && empty(x) => {}
        Some(x) if x.error.is_some() => {
            let e = x.error.as_deref().unwrap_or_default();
            if quiet(ui, e, Some(("Повторить", Icon::Refresh, &format!("sv-retry-{salt}")))) {
                out.push(Command::Search(q.to_owned()));
            }
        }
        Some(x) if empty(x) => {
            quiet(ui, &format!("В {} ничего не нашлось.", service.display_name()), None);
        }
        Some(x) => {
            if !x.tracks.is_empty() {
                part_label(ui, "Треки");
                let from = streams::StreamFrom::Rows(format!("поиска {}: {shown_q}", service.display_name()));
                nav = streams::track_rows(ui, &format!("sv-tracks-{salt}"), &x.tracks, false, &from, state, pages, out)
                    .or(nav.take());
            }
            if !x.albums.is_empty() {
                part_label(ui, "Альбомы");
                widgets::card_row(ui, &format!("sv-albums-{salt}"), |ui| {
                    for a in &x.albums {
                        nav = streams::album_card(ui, a, widgets::CARD_W, salt, &state.downloads.wishlist, out).or(nav.take());
                    }
                });
            }
            if !x.artists.is_empty() {
                part_label(ui, "Исполнители");
                widgets::card_row(ui, &format!("sv-artists-{salt}"), |ui| {
                    for a in &x.artists {
                        nav = streams::artist_card(ui, a, 120.0).or(nav.take());
                    }
                });
            }
            if !x.playlists.is_empty() {
                part_label(ui, "Плейлисты");
                widgets::card_row(ui, &format!("sv-playlists-{salt}"), |ui| {
                    for p in &x.playlists {
                        nav = streams::playlist_card(ui, p, widgets::CARD_W, salt, out).or(nav.take());
                    }
                });
            }
        }
    }
    nav
}

// ---------------------------------------------------------------- «Всё»

/// The top result beside the first tracks from this width on.
const WIDE: f32 = 820.0;
/// Tracks under the first ones before «Ещё».
const MORE: usize = 5;
const EXPANDED_KEY: &str = "md3e-search-more-tracks";
/// Height of the top card stacked above the tracks (beside them it is as
/// high as they are).
const TOP_H_NARROW: f32 = 136.0;

/// The merged answer of «Всё» (see the module docs).
fn all_page(ui: &mut Ui, query: &str, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let a = &state.search.answer;
    let q = query.trim();
    if q.is_empty() {
        return None;
    }
    // The previous answer stays while the new one is on its way.
    let loading = a.query.trim() != q || a.searching || state.search.query.trim() != q;
    let mut nav = None;
    if let Some(note) = &a.note {
        quiet(ui, note, None);
    }
    if a.is_empty() {
        if a.top_pending && loading {
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 32.0) * 0.5);
                ui.add(LoadingIndicator::new().size(32.0));
            });
        } else if let Some(e) = a.error.as_deref().filter(|_| !loading) {
            if quiet(ui, e, Some(("Повторить", Icon::Refresh, "sa-retry"))) {
                out.push(Command::Search(q.to_owned()));
            }
        } else if !loading {
            widgets::empty_state(
                ui,
                Icon::Search,
                "Ничего не нашлось",
                "Проверь, как написано, или поищи раздачи в торрентах.",
                None,
            );
        }
        ui.add_space(widgets::SECTION_GAP);
        return None;
    }

    // One queue for every track shown: the top one first.
    let mut queue: Vec<&SearchTrack> = Vec::with_capacity(a.tracks.len() + 1);
    if let Some(TopResult::Track(t)) = &a.top {
        queue.push(t);
    }
    queue.extend(a.tracks.iter());
    let picks: Vec<SearchPick> = queue.iter().map(|t| t.pick()).collect();
    let context = format!("поиска: {q}");
    let offset = usize::from(matches!(a.top, Some(TopResult::Track(_))));
    let first = a.tracks.len().min(TOP_TRACKS);
    let show_top = a.top.is_some() || a.top_pending;

    // Beside each other only when the tracks fill the card's height (four
    // or five rows); else stacked, the card one row high.
    let wide = ui.available_width() >= WIDE && first >= 4;
    let top_h = first as f32 * 72.0 + (first.max(1) - 1) as f32 * 8.0;
    if wide && show_top {
        let w = ui.available_width();
        let left = (w * 0.4).clamp(300.0, 440.0);
        let gap = 24.0;
        let top = ui.cursor().min;
        let lrect = Rect::from_min_size(top, vec2(left, 40.0 + top_h));
        let rrect = Rect::from_min_size(pos2(top.x + left + gap, top.y), vec2(w - left - gap, 40.0 + top_h));
        let mut lui = ui.new_child(UiBuilder::new().max_rect(lrect).id_salt("sa-top-col"));
        part_head(&mut lui, "Лучший результат", loading && a.top_pending);
        nav = top_card(&mut lui, a.top.as_ref(), top_h, true, &picks, &context, state, out).or(nav.take());
        let mut rui = ui.new_child(UiBuilder::new().max_rect(rrect).id_salt("sa-tracks-col"));
        rui.spacing_mut().item_spacing.y = 8.0;
        if first > 0 {
            part_head(&mut rui, "Треки", loading && !a.top_pending);
            nav = track_rows(&mut rui, a, 0..first, offset, &picks, &context, state, pages, out).or(nav.take());
        }
        let h = lui.min_rect().height().max(rui.min_rect().height());
        ui.allocate_exact_size(vec2(w, h), Sense::hover());
        ui.add_space(widgets::SECTION_GAP);
    } else {
        if show_top {
            part_head(ui, "Лучший результат", loading && a.top_pending);
            nav = top_card(ui, a.top.as_ref(), TOP_H_NARROW, false, &picks, &context, state, out).or(nav.take());
            ui.add_space(8.0);
        }
        if first > 0 {
            part_head(ui, "Треки", loading && !a.top_pending && !show_top);
            nav = track_rows(ui, a, 0..first, offset, &picks, &context, state, pages, out).or(nav.take());
        }
    }

    // More tracks: the next few, all with «Ещё».
    let rest = a.tracks.len() - first;
    if rest > 0 {
        let key = Id::new(EXPANDED_KEY);
        let expanded = ui.ctx().data(|d| d.get_temp::<String>(key)).is_some_and(|k| k == q);
        if wide && show_top {
            part_head(ui, "Ещё треки", false);
        }
        let n = if expanded { rest } else { rest.min(MORE) };
        nav = track_rows(ui, a, first..first + n, offset, &picks, &context, state, pages, out).or(nav.take());
        if n < rest
            && ui
                .add(Button::new("Ещё").style(Style::Text).icon(Icon::ExpandMore).id_salt("sa-more-tracks"))
                .clicked()
        {
            ui.ctx().data_mut(|d| d.insert_temp(key, q.to_owned()));
        }
    }
    if !(wide && show_top) || rest > 0 {
        ui.add_space(widgets::SECTION_GAP);
    }

    if !a.albums.is_empty() {
        part_head(ui, "Альбомы", false);
        widgets::card_row(ui, "sa-albums", |ui| {
            for x in &a.albums {
                nav = match x {
                    SearchAlbum::Library(r) => widgets::release_card(ui, r, widgets::CARD_W, "sa", out),
                    SearchAlbum::Stream(c) => streams::album_card(ui, c, widgets::CARD_W, "sa", &state.downloads.wishlist, out),
                }
                .or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }
    if !a.artists.is_empty() {
        part_head(ui, "Исполнители", false);
        widgets::card_row(ui, "sa-artists", |ui| {
            for x in &a.artists {
                nav = match x {
                    SearchArtist::Library(c) => widgets::artist_avatar(ui, c, 120.0, out),
                    SearchArtist::Stream(c) => streams::artist_card(ui, c, 120.0),
                }
                .or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }
    if !a.playlists.is_empty() {
        part_head(ui, "Плейлисты", false);
        widgets::card_row(ui, "sa-playlists", |ui| {
            for x in &a.playlists {
                nav = match x {
                    SearchPlaylist::Library(p) => {
                        widgets::playlist_card_with(ui, p, widgets::CARD_W, "sa", Some(&mut *pages), out)
                    }
                    SearchPlaylist::Stream(p) => streams::playlist_card(ui, p, widgets::CARD_W, "sa", out),
                }
                .or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }
    nav
}

/// A part's name (title-medium) with the loading mark while it may change.
fn part_head(ui: &mut Ui, title: &str, loading: bool) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::left_to_right(Align::Center), |ui| {
        ui.label(TypeRole::TitleMedium.style().rich(title, s.on_surface));
        ui.add_space(4.0);
        loading_mark(ui, loading);
    });
}

/// Rows `range` of the answer's tracks: library rows as the library draws
/// them (with «у тебя»), services' rows as theirs; every row plays the one
/// queue from its place (`offset`: the top track before them).
#[allow(clippy::too_many_arguments)]
fn track_rows(
    ui: &mut Ui,
    a: &SearchAnswer,
    range: std::ops::Range<usize>,
    offset: usize,
    picks: &[SearchPick],
    context: &str,
    state: &ViewState,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let mut nav = None;
    for i in range {
        let at = i + offset;
        let id = format!("sa-track-{i}");
        nav = match &a.tracks[i] {
            SearchTrack::Library(t) => {
                let from = PlayFrom::Search { picks: picks.to_vec(), at, context: context.to_owned() };
                let opts = widgets::TrackListOpts { mark_local: true, ..Default::default() };
                widgets::track_list_ext(ui, &id, std::slice::from_ref(t), Lead::Cover, &from, opts, state, pages, out)
            }
            SearchTrack::Stream(t) => {
                let from = streams::StreamFrom::Search { picks: picks.to_vec(), at, context: context.to_owned() };
                streams::track_rows(ui, &id, std::slice::from_ref(t), false, &from, state, pages, out)
            }
        }
        .or(nav.take());
    }
    nav
}

/// What the top card shows and does.
struct TopView {
    title: String,
    /// «Исполнитель», «Трек · Кино», «Альбом · Кино · 1988».
    line: String,
    image: Option<String>,
    shape: MaterialShape,
    /// The card opens this (an artist, an album); a track plays instead.
    open: Option<Route>,
    play: Option<Command>,
    salt: String,
}

fn top_view(top: &TopResult, picks: &[SearchPick], context: &str) -> TopView {
    let play_queue = || Command::PlaySearch { tracks: picks.to_vec(), start: 0, context: context.to_owned() };
    match top {
        TopResult::Artist(SearchArtist::Library(a)) => TopView {
            title: a.name.clone(),
            line: "Исполнитель · в библиотеке".into(),
            image: a.image.as_ref().map(|c| c.uri()),
            shape: MaterialShape::Circle,
            open: Some(Route::Artist(a.id)),
            play: Some(Command::PlayArtist { id: a.id, shuffle: false }),
            salt: format!("la-{}", a.id),
        },
        TopResult::Artist(SearchArtist::Stream(a)) => {
            // Their songs among the results play; a library artist plays
            // from the library.
            let play = match a.artist {
                Some(id) => Some(Command::PlayArtist { id, shuffle: false }),
                None => (!picks.is_empty()).then(play_queue),
            };
            TopView {
                title: a.name.clone(),
                line: "Исполнитель".into(),
                image: a.image_url.clone(),
                shape: if a.image_url.is_some() { MaterialShape::Circle } else { widgets::avatar_shape(&a.remote.id) },
                open: Some(match a.artist {
                    Some(id) => Route::Artist(id),
                    None => Route::Stream(StreamPage::Artist(a.remote.clone())),
                }),
                play,
                salt: format!("sa-{}", a.remote),
            }
        }
        TopResult::Track(t) => {
            let (title, artist, image, local, salt) = match t {
                SearchTrack::Library(r) => {
                    (r.title.clone(), r.artist.clone(), r.cover.as_ref().map(|c| c.uri()), r.has_local, format!("lt-{}", r.id))
                }
                SearchTrack::Stream(r) => (r.title.clone(), r.artist.clone(), r.cover_url.clone(), r.has_local, format!("st-{}", r.remote)),
            };
            let mut line = format!("Трек · {artist}");
            if local {
                line.push_str(" · у тебя");
            }
            TopView { title, line, image, shape: MaterialShape::Square, open: None, play: Some(play_queue()), salt }
        }
        TopResult::Album(SearchAlbum::Library(r)) => TopView {
            title: r.title.clone(),
            line: album_line(&r.artist, r.year),
            image: r.cover.as_ref().map(|c| c.uri()),
            shape: MaterialShape::Square,
            open: Some(Route::Release(r.id)),
            play: Some(Command::PlayRelease { id: r.id, start: None, shuffle: false }),
            salt: format!("lr-{}", r.id),
        },
        TopResult::Album(SearchAlbum::Stream(c)) => TopView {
            title: c.title.clone(),
            line: album_line(&c.artist, c.year),
            image: c.cover_url.clone(),
            shape: MaterialShape::Square,
            open: Some(Route::Stream(StreamPage::Album(c.remote.clone()))),
            play: Some(Command::PlayStreamAlbum { id: c.remote.clone(), start: None, shuffle: false }),
            salt: format!("sr-{}", c.remote),
        },
    }
}

fn album_line(artist: &str, year: Option<i32>) -> String {
    match year {
        Some(y) if !artist.is_empty() => format!("Альбом · {artist} · {y}"),
        Some(y) => format!("Альбом · {y}"),
        None if !artist.is_empty() => format!("Альбом · {artist}"),
        None => "Альбом".into(),
    }
}

/// «Лучший результат»: a large card (picture, name, what it is) with ▶;
/// `tall` beside the tracks, else a row-shaped card above them. While the
/// top is not settled yet its room is kept, empty.
#[allow(clippy::too_many_arguments)]
fn top_card(
    ui: &mut Ui,
    top: Option<&TopResult>,
    height: f32,
    tall: bool,
    picks: &[SearchPick],
    context: &str,
    state: &ViewState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let width = ui.available_width();
    let Some(top) = top else {
        // Kept room: nothing jumps when it comes.
        let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        paint::container(ui.painter(), rect, Radii::all(shape::EXTRA_LARGE), s.surface_container, Stroke::NONE);
        return None;
    };
    let v = top_view(top, picks, context);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let radii = Radii::all(shape::EXTRA_LARGE);
    paint::container(ui.painter(), rect, radii, s.surface_container_high, Stroke::NONE);
    let layer = anim::fraction(ui.ctx(), ui.id().with(("sa-top-hover", &v.salt)), if resp.hovered() { 0.08 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);
    let pad = if tall { 24.0 } else { 16.0 };
    let play_size = 56.0;
    // Beside the tracks: the cover as large as the card allows, the name
    // and what it is under it, ▶ at the end of that line — the card is
    // filled, top to bottom. Above the tracks: one row.
    const TEXT_H: f32 = 72.0;
    let img = if tall {
        (height - 2.0 * pad - 16.0 - TEXT_H).min(rect.width() - 2.0 * pad).max(96.0)
    } else {
        height - 2.0 * pad
    };
    let img_rect = if tall {
        // Centred when the card is wider than tall enough for it.
        let block = img + 16.0 + TEXT_H;
        Rect::from_min_size(pos2(rect.left() + pad, rect.top() + ((height - block) * 0.5).max(pad)), vec2(img, img))
    } else {
        Rect::from_min_size(rect.min + vec2(pad, pad), vec2(img, img))
    };
    widgets::shape_image(ui, v.image.as_deref(), &widgets::initials(&v.title), img_rect, v.shape);
    // The current track shows that it plays.
    let playing_now = match top {
        TopResult::Track(SearchTrack::Library(t)) => state.player.current.as_ref().is_some_and(|c| c.id == t.id),
        TopResult::Track(SearchTrack::Stream(t)) => t.track.is_some() && state.player.current.as_ref().map(|c| c.id) == t.track,
        _ => false,
    };
    let text_top = img_rect.bottom() + 16.0;
    let play_rect = if tall {
        Rect::from_center_size(pos2(rect.right() - pad - play_size * 0.5, text_top + TEXT_H * 0.5), vec2(play_size, play_size))
    } else {
        Rect::from_center_size(pos2(rect.right() - pad - play_size * 0.5, rect.center().y), vec2(play_size, play_size))
    };
    let text_rect = if tall {
        Rect::from_min_max(pos2(rect.left() + pad, text_top), pos2(play_rect.left() - 16.0, text_top + TEXT_H))
    } else {
        Rect::from_min_max(pos2(img_rect.right() + 16.0, rect.top() + pad), pos2(play_rect.left() - 12.0, rect.bottom() - pad))
    };
    let mut tui = ui.new_child(
        UiBuilder::new()
            .max_rect(text_rect)
            .layout(Layout::top_down(Align::Min).with_main_align(Align::Center))
            .id_salt(("sa-top-text", &v.salt)),
    );
    let role = if tall { TypeRole::HeadlineMedium } else { TypeRole::HeadlineSmall };
    tui.add(egui::Label::new(role.emphasized().rich(v.title.as_str(), s.on_surface)).truncate());
    tui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(v.line.as_str(), s.on_surface_variant)).truncate());
    let mut pui = ui.new_child(UiBuilder::new().max_rect(play_rect).id_salt(("sa-top-play", &v.salt)));
    let play = v.play.is_some()
        && pui
            .add(
                IconButton::new(if playing_now && state.player.status == nikgob_view::Status::Playing {
                    Icon::Pause
                } else {
                    Icon::PlayArrow
                })
                .size(Size::M)
                .style(Style::Filled)
                .filled_icon(true)
                .tooltip("Слушать")
                .id_salt(("sa-top-play-btn", &v.salt)),
            )
            .clicked();
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if play || (resp.clicked() && v.open.is_none()) {
        out.push(if playing_now { Command::TogglePlay } else { v.play.clone().unwrap_or(Command::TogglePlay) });
        return None;
    }
    if resp.clicked() { v.open } else { None }
}
