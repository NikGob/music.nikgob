//! Поиск (md3e-player.md §3.6): a docked search view (radius 28, 56 dp
//! header with source filter chips) under the bar. Sections: Библиотека,
//! then Торренты, which run only on request (Enter or the button).

use egui::{Align, Id, Layout, Rect, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{Command, Route, TorrentResult, ViewState};

use super::{PageState, fmt_size, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, FilterChip, ListItem, LoadingIndicator, Radii, Style, badge, paint,
    },
    screens::common,
    tokens::shape,
};
use widgets::{Lead, PlayFrom};

/// Which sources the view shows. Spotify and YouTube are chips but not yet
/// selectable (stage 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Library,
    Torrents,
}

const FILTER_KEY: &str = "md3e-search-filter";

pub fn filter(ctx: &egui::Context) -> Filter {
    ctx.data(|d| d.get_temp(Id::new(FILTER_KEY))).unwrap_or_default()
}

pub fn set_filter(ctx: &egui::Context, f: Filter) {
    ctx.data_mut(|d| d.insert_temp(Id::new(FILTER_KEY), f));
}

const HEADER: f32 = 56.0;

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
                for name in ["Spotify", "YouTube"] {
                    ui.add(FilterChip::new(name, false).enabled(false).id_salt(("sf", name)))
                        .on_disabled_hover_text("Этап 3");
                }
                if ui.add(FilterChip::new("Торренты", f == Filter::Torrents).id_salt("sf-tor")).clicked() {
                    f = Filter::Torrents;
                }
            });
        });
    set_filter(&ctx, f);
    ui.painter().line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, s.outline_variant),
    );

    // Body.
    let body = Rect::from_min_max(
        pos2(panel.left() + 16.0, header.bottom() + 1.0),
        pos2(panel.right() - 16.0, panel.bottom()),
    );
    let mut bui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("search-body"));
    bui.set_clip_rect(body.intersect(ui.clip_rect()));
    let mut nav = None;
    egui::ScrollArea::vertical()
        .id_salt("search-scroll")
        .auto_shrink([false, false])
        .show(&mut bui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add_space(8.0);
            if f != Filter::Torrents {
                nav = library(ui, query, state, f == Filter::Library, pages, out).or(nav.take());
            }
            if f != Filter::Library {
                nav = torrents(ui, query, state, out).or(nav.take());
            }
            if f == Filter::All {
                ui.add_space(widgets::SECTION_GAP);
                ui.label(TypeRole::BodySmall.style().rich(
                    "Spotify и YouTube Music появятся на этапе 3.",
                    s.on_surface_variant,
                ));
            }
            ui.add_space(bottom_inset);
        });
    nav
}

/// Section label ("Библиотека", "Торренты").
fn group_label(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.label(TypeRole::LabelLarge.style().rich(text, s.primary));
    ui.add_space(4.0);
}

fn library(
    ui: &mut Ui,
    query: &str,
    state: &ViewState,
    alone: bool,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let r = &state.search;
    let mut nav = None;
    group_label(ui, "Библиотека");
    if r.query.trim() != query.trim() || r.searching {
        ui.add(LoadingIndicator::new());
        return None;
    }
    let empty = r.tracks.is_empty() && r.releases.is_empty() && r.artists.is_empty() && r.playlists.is_empty();
    if empty {
        if alone {
            widgets::empty_state(ui, Icon::Search, "Ничего не нашлось", "В библиотеке такого нет.", None);
        } else {
            ui.label(TypeRole::BodyMedium.style().rich("В библиотеке ничего не нашлось.", s.on_surface_variant));
        }
        return None;
    }
    // Compact rows first, so a long track list does not bury them.
    if !r.artists.is_empty() {
        widgets::section_header(ui, "Исполнители", false);
        widgets::card_row(ui, "search-artists", |ui| {
            for a in &r.artists {
                nav = widgets::artist_avatar(ui, a, 120.0, out).or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }
    if !r.releases.is_empty() {
        widgets::section_header(ui, "Альбомы", false);
        widgets::card_row(ui, "search-releases", |ui| {
            for rel in &r.releases {
                nav = widgets::release_card(ui, rel, widgets::CARD_W, "search", out).or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }
    if !r.tracks.is_empty() {
        widgets::section_header(ui, "Треки", false);
        let ids = r.tracks.iter().map(|t| t.id).collect();
        nav = widgets::track_list(
            ui,
            "search",
            &r.tracks,
            Lead::Cover,
            &PlayFrom::Tracks(ids, format!("Поиск «{query}»")),
            state,
            pages,
            out,
        )
        .or(nav.take());
        ui.add_space(widgets::SECTION_GAP);
    }
    if !r.playlists.is_empty() {
        widgets::section_header(ui, "Плейлисты", false);
        widgets::card_row(ui, "search-playlists", |ui| {
            for p in &r.playlists {
                nav = widgets::playlist_card_with(ui, p, widgets::CARD_W, "search", Some(&mut *pages), out)
                    .or(nav.take());
            }
        });
    }
    nav
}

fn torrents(ui: &mut Ui, query: &str, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let t = &state.search.torrents;
    let q = query.trim();
    let stale = t.query.trim() != q;
    let mut nav = None;
    ui.add_space(8.0);
    group_label(ui, "Торренты");

    if t.searching {
        ui.horizontal(|ui| {
            ui.add(LoadingIndicator::new().size(32.0));
            ui.label(TypeRole::BodyMedium.style().rich_para(
                format!("Ищу раздачи «{}»… Prowlarr отвечает не сразу.", t.query),
                s.on_surface_variant,
            ));
        });
        return None;
    }

    if !state.prefs.prowlarr_configured {
        ui.label(TypeRole::BodyMedium.style().rich_para(
            "Prowlarr не настроен: адрес и ключ задаются в Настройках, раздел «Торренты».",
            s.on_surface_variant,
        ));
        if ui.add(Button::new("Открыть настройки").style(Style::Text).icon(Icon::Settings).id_salt("tor-settings")).clicked() {
            nav = Some(Route::Settings);
        }
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
        ui.label(TypeRole::BodyMedium.style().rich(err, s.error));
        if !stale
            && ui.add(Button::new("Повторить").style(Style::Text).id_salt("tor-retry")).clicked()
        {
            out.push(Command::SearchTorrents(q.to_owned()));
        }
        return nav;
    }

    if t.results.is_empty() {
        if !stale && !t.query.is_empty() {
            ui.label(TypeRole::BodyMedium.style().rich("Раздач не нашлось.", s.on_surface_variant));
        }
        return nav;
    }
    if stale {
        ui.label(TypeRole::BodySmall.style().rich(format!("Результаты для «{}»", t.query), s.on_surface_variant));
    }
    // Buttons fit beside the title only in a wide row.
    let wide = ui.available_width() >= 640.0;
    for r in &t.results {
        torrent_row(ui, r, wide, out);
    }
    nav
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
