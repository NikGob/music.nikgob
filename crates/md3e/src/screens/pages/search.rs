//! Поиск (md3e-player.md §3.6, docs/stage3.md §3.2): a docked search view
//! (radius 28, 56 dp header with source filter chips) under the bar.
//! Sections: Библиотека → Spotify → YouTube Music (as each answers) →
//! Торренты, which run only on request (the button, or Enter once Prowlarr
//! is set up). Every section reads the same way: its name, then Треки →
//! Альбомы → Исполнители → Плейлисты; under «Всё» five tracks and «Все →»
//! to that source's chip. A service that cannot answer says why quietly
//! (sign in, it asked to wait, no connection), never in red.

use egui::{Align, Id, Layout, Rect, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{Account, Command, Route, SearchSources, Service, StreamSearchSection, TorrentResult, ViewState};

use super::{PageState, fmt_size, streams, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, FilterChip, ListItem, LoadingIndicator, Radii, Style, badge, paint},
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
    Spotify,
    Youtube,
    Torrents,
}

impl Filter {
    /// The sources this filter asks.
    fn sources(self) -> SearchSources {
        let only = |library, spotify, youtube| SearchSources { library, spotify, youtube, torrents: false };
        match self {
            Self::All => only(true, true, true),
            Self::Library => only(true, false, false),
            Self::Spotify => only(false, true, false),
            Self::Youtube => only(false, false, true),
            // The library stays for context; the indexer is asked by its button.
            Self::Torrents => only(true, false, false),
        }
    }

    fn service(self) -> Option<Service> {
        match self {
            Self::Spotify => Some(Service::Spotify),
            Self::Youtube => Some(Service::Youtube),
            _ => None,
        }
    }

    fn of(service: Service) -> Self {
        match service {
            Service::Spotify => Self::Spotify,
            Service::Youtube => Self::Youtube,
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

const HEADER: f32 = 56.0;
/// Tracks a section shows under «Всё» (its chip shows them all).
const FEW: usize = 5;

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

    // Header: source filters.
    let header = Rect::from_min_size(panel.min, vec2(panel.width(), HEADER));
    let mut f = filter(&ctx);
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
                if ui.add(FilterChip::new("Всё", f == Filter::All).id_salt("sf-all")).clicked() {
                    f = Filter::All;
                }
                if ui.add(FilterChip::new("Библиотека", f == Filter::Library).id_salt("sf-lib")).clicked() {
                    f = Filter::Library;
                }
                for (name, chip) in [("Spotify", Filter::Spotify), ("YouTube Music", Filter::Youtube)] {
                    if ui.add(FilterChip::new(name, f == chip).id_salt(("sf", name))).clicked() {
                        f = chip;
                    }
                }
                if ui.add(FilterChip::new("Торренты", f == Filter::Torrents).id_salt("sf-tor")).clicked() {
                    f = Filter::Torrents;
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
    let mut go = None;
    egui::ScrollArea::vertical()
        .id_salt(("search-scroll", f as u8))
        .auto_shrink([false, false])
        .show(&mut bui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(8.0);
            let all = f == Filter::All;
            if matches!(f, Filter::All | Filter::Library) {
                let (n, more) = library(ui, query, state, all, pages, out);
                nav = n.or(nav.take());
                if more {
                    go = Some(Filter::Library);
                }
            }
            for service in [Service::Spotify, Service::Youtube] {
                if all || f.service() == Some(service) {
                    let (n, more) = stream_section(ui, query, service, state, all, pages, out);
                    nav = n.or(nav.take());
                    if more {
                        go = Some(Filter::of(service));
                    }
                }
            }
            if matches!(f, Filter::All | Filter::Torrents) {
                nav = torrents(ui, query, state, pages, out).or(nav.take());
            }
            ui.add_space(bottom_inset);
        });
    if let Some(g) = go {
        f = g;
    }
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
    let s = Theme::get(ui.ctx()).scheme;
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(TypeRole::BodyMedium.style().rich(text, s.on_surface_variant));
        if let Some((label, icon, salt)) = button {
            clicked = ui.add(Button::new(label).style(Style::Text).icon(icon).id_salt(salt)).clicked();
        }
    });
    clicked
}

/// The library part. Returns a route, and whether «Все» was clicked.
fn library(
    ui: &mut Ui,
    query: &str,
    state: &ViewState,
    all: bool,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> (Option<Route>, bool) {
    let r = &state.search;
    let mut nav = None;
    let loading = r.query.trim() != query.trim() || r.searching;
    let empty = r.tracks.is_empty() && r.releases.is_empty() && r.artists.is_empty() && r.playlists.is_empty();
    let more = section_head(ui, "Библиотека", loading, all && !empty);
    if empty && loading {
        return (None, more);
    }
    if empty {
        if all {
            quiet(ui, "В библиотеке такого нет.", None);
        } else {
            widgets::empty_state(ui, Icon::Search, "Ничего не нашлось", "В библиотеке такого нет.", None);
        }
        ui.add_space(widgets::SECTION_GAP);
        return (None, more);
    }
    if !r.tracks.is_empty() {
        part_label(ui, "Треки");
        let n = if all { r.tracks.len().min(FEW) } else { r.tracks.len() };
        let ids = r.tracks.iter().map(|t| t.id).collect();
        nav = widgets::track_list(
            ui,
            "search",
            &r.tracks[..n],
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
    (nav, more)
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

/// The results of one service: tracks, albums, artists, playlists. Returns
/// a route, and whether «Все» was clicked.
fn stream_section(
    ui: &mut Ui,
    query: &str,
    service: Service,
    state: &ViewState,
    all: bool,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> (Option<Route>, bool) {
    let mut nav = None;
    let salt = service.as_str();
    let name = service.display_name();
    let sec: Option<&StreamSearchSection> = state.search.streams.iter().find(|x| x.service == service);
    let q = query.trim();
    let stale = state.search.query.trim() != q;
    let why = why_not(state, service);
    let loading = !q.is_empty() && why.is_none() && (stale || sec.is_none_or(|x| x.searching));
    let empty = |x: &StreamSearchSection| {
        x.tracks.is_empty() && x.albums.is_empty() && x.artists.is_empty() && x.playlists.is_empty()
    };
    let has_rows = sec.is_some_and(|x| !empty(x) && x.error.is_none());
    let more = section_head(ui, name, loading, all && has_rows && why.is_none());
    match why {
        Some(Why::Missing) => {
            quiet(ui, &format!("{name} нет в этой сборке."), None);
        }
        Some(Why::Off) => {
            if quiet(ui, &format!("{name} выключен."), Some(("Включить", Icon::Settings, &format!("sv-on-{salt}")))) {
                pages.open_setting(match service {
                    Service::Spotify => super::SettingId::SpotifyEnabled,
                    Service::Youtube => super::SettingId::YouTubeEnabled,
                });
            }
        }
        Some(Why::SignIn) => {
            if quiet(ui, &format!("Войди в {name}, чтобы искать и там."), Some(("Войти", Icon::Login, &format!("sv-login-{salt}")))) {
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
            nav = results(ui, q, service, sec, loading, all, state, pages, out);
        }
    }
    ui.add_space(widgets::SECTION_GAP);
    (nav, more)
}

#[allow(clippy::too_many_arguments)]
fn results(
    ui: &mut Ui,
    q: &str,
    service: Service,
    sec: Option<&StreamSearchSection>,
    loading: bool,
    all: bool,
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
    match sec {
        _ if q.is_empty() => {}
        None => {}
        Some(x) if loading && empty(x) => {}
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
                let n = if all { x.tracks.len().min(FEW) } else { x.tracks.len() };
                let from = streams::StreamFrom::Rows(format!("поиска {}: {shown_q}", service.display_name()));
                nav = streams::track_rows(ui, &format!("sv-tracks-{salt}"), &x.tracks[..n], false, &from, state, pages, out)
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
