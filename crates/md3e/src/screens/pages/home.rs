//! Главная (md3e-player.md §3.1): greeting, filter chips, the "Твой микс"
//! hero card, then feed rows.

use egui::{Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_shapes::MaterialShape;
use nikgob_view::{Command, ContinueCard, LibraryTab, Route, ViewState};

use super::{PageState, widgets};
use crate::{
    Icon, Theme, TypeRole,
    components::{FilterChip, IconButton, Radii, Size, Style, anim, paint},
    tokens::shape,
};

fn greeting(hour: u8) -> &'static str {
    match hour {
        5..=11 => "Доброе утро",
        12..=16 => "Добрый день",
        17..=22 => "Добрый вечер",
        _ => "Доброй ночи",
    }
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let h = &state.home;
    let mut nav = None;

    ui.add_space(8.0);
    ui.label(TypeRole::HeadlineMedium.style().rich(greeting(h.greeting_hour), s.on_surface));
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (i, label) in ["Всё", "Моё", "Новое", "Под настроение"].iter().enumerate() {
            if ui
                .add(FilterChip::new(*label, pages.home_filter == i).id_salt(("home-chip", i)))
                .clicked()
            {
                pages.home_filter = i;
            }
        }
    });
    ui.add_space(16.0);

    let empty = h.mix.is_empty() && h.recently_added.is_empty() && h.continue_listening.is_empty();
    if empty {
        // Nothing here yet: the search already plays anything from YouTube
        // Music, without a login; own files come in from a folder.
        if widgets::empty_state(
            ui,
            Icon::Search,
            "Начни с поиска",
            "Поиск сразу ищет и в YouTube Music — без входа. Понравилось — лайк, и трек останется у тебя.",
            Some("Найти музыку"),
        ) {
            pages.focus_search = true;
        }
        ui.vertical_centered(|ui| {
            if ui
                .add(crate::components::Button::new("Добавить свои файлы…").style(Style::Text).id_salt("home-add-files"))
                .clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Добавить папку").pick_folder()
            {
                out.push(Command::ImportFolder(dir));
            }
            if !connected(state)
                && ui
                    .add(
                        crate::components::Button::new("Подключить Spotify или YouTube Music")
                            .style(Style::Text)
                            .id_salt("home-connect"),
                    )
                    .clicked()
            {
                pages.open_setting(super::SettingId::SpotifyEnabled);
            }
        });
        return None;
    }

    let show_mine = pages.home_filter != 2;
    let show_new = pages.home_filter != 1;

    // HERO: "Твой микс".
    if show_mine && !h.mix.is_empty() {
        hero_mix(ui, state, out);
        ui.add_space(widgets::SECTION_GAP);
    }

    if show_mine && !h.continue_listening.is_empty() {
        if widgets::section_header(ui, "Продолжить", true) {
            nav = Some(Route::Library(LibraryTab::Albums));
        }
        widgets::card_row(ui, "home-continue", |ui| {
            for c in &h.continue_listening {
                nav = match c {
                    ContinueCard::Release(r) => widgets::release_card(ui, r, widgets::CARD_W, "continue", out),
                    // Streamed, not in the library: its service's page.
                    ContinueCard::Stream(a) => {
                        super::streams::album_card(ui, a, widgets::CARD_W, "continue", &state.downloads.wishlist, out)
                    }
                }
                .or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }

    if show_mine && !h.artist_mixes.is_empty() {
        widgets::section_header(ui, "Миксы исполнителей", false);
        widgets::card_row(ui, "home-artists", |ui| {
            for a in &h.artist_mixes {
                nav = widgets::artist_avatar(ui, a, 120.0, out).or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }

    if show_mine && !h.long_unplayed.is_empty() {
        widgets::section_header(ui, "Давно не слушал", false);
        widgets::card_row(ui, "home-long", |ui| {
            for r in &h.long_unplayed {
                nav = widgets::release_card(ui, r, widgets::CARD_W, "long", out).or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }

    if show_new && !h.recently_added.is_empty() {
        if widgets::section_header(ui, "Недавно добавлено", true) {
            nav = Some(Route::Library(LibraryTab::Albums));
        }
        widgets::card_row(ui, "home-recent", |ui| {
            for r in &h.recently_added {
                nav = widgets::release_card(ui, r, widgets::CARD_W, "recent", out).or(nav.take());
            }
        });
        ui.add_space(widgets::SECTION_GAP);
    }

    // Picks from the services arrive in stage 4; until then the section
    // only shows the way to connect one (and stays away once connected).
    if show_new && !connected(state) {
        widgets::section_header(ui, "Новое для тебя", false);
        ui.horizontal_wrapped(|ui| {
            ui.label(TypeRole::BodyMedium.style().rich_para(
                "Подключи Spotify или YouTube Music — подтяну лайки и плейлисты, а здесь появятся подборки.",
                s.on_surface_variant,
            ));
            if ui
                .add(crate::components::Button::new("Подключить").style(Style::Tonal).id_salt("home-new-connect"))
                .clicked()
            {
                pages.open_setting(super::SettingId::SpotifyEnabled);
            }
        });
    }
    nav
}

/// Some service is signed in.
fn connected(state: &ViewState) -> bool {
    state.accounts.iter().any(|a| a.enabled && a.account.is_logged_in())
}

/// The hero card: covers cropped into Material shapes, title, play L.
fn hero_mix(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let mix = &state.home.mix;
    let width = ui.available_width();
    let height = 200.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let radii = Radii::all(shape::EXTRA_EXTRA_LARGE);
    paint::container(ui.painter(), rect, radii, s.surface_container_high, Stroke::NONE);
    let layer = anim::fraction(
        ui.ctx(),
        ui.id().with("hero-layer"),
        if resp.hovered() { 0.08 } else { 0.0 },
        theme.motion.fast_effects,
    );
    paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);

    // Entrance: covers spring up from below once.
    let enter = anim::fraction(ui.ctx(), ui.id().with("hero-enter"), 1.0, theme.motion.slow_spatial);
    let shapes = [
        MaterialShape::Cookie9Sided,
        MaterialShape::Clover4Leaf,
        MaterialShape::Sunny,
        MaterialShape::Pill,
    ];
    let mut covers: Vec<_> = Vec::new();
    for t in mix {
        if let Some(c) = &t.cover
            && !covers.iter().any(|x: &String| *x == c.uri())
        {
            covers.push(c.uri());
        }
        if covers.len() == 4 {
            break;
        }
    }
    let narrow = width < 760.0;
    let art_w = if narrow { (width * 0.34).min(240.0) } else { (width * 0.42).min(360.0) };
    let size = if narrow { 80.0 } else { 112.0 };
    for (i, uri) in covers.iter().enumerate() {
        let x = rect.left() + 24.0 + i as f32 * (art_w - size - 24.0) / 3.0_f32.max(1.0);
        let y = rect.center().y - size * 0.5 + if i % 2 == 0 { -14.0 } else { 14.0 };
        let r = Rect::from_min_size(pos2(x, y + (1.0 - enter) * 40.0), vec2(size, size));
        widgets::shape_image(ui, Some(uri), "", r, shapes[i % shapes.len()]);
    }

    let text_x = rect.left() + art_w + 48.0;
    let text_rect = Rect::from_min_max(pos2(text_x, rect.top() + 32.0), pos2(rect.right() - 128.0, rect.bottom() - 16.0));
    let mut tui = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    tui.label(TypeRole::LabelLarge.style().rich("ДЛЯ ТЕБЯ", s.primary));
    let title = if narrow { TypeRole::HeadlineMedium } else { TypeRole::DisplaySmall };
    tui.label(title.emphasized().rich("Твой микс", s.on_surface));
    let n = mix.len() as u64;
    tui.label(TypeRole::BodyLarge.style().rich(
        format!(
            "{n} {} · любимое и дослушанное",
            crate::screens::common::plural(n, "трек", "трека", "треков")
        ),
        s.on_surface_variant,
    ));

    let play_rect = Rect::from_center_size(pos2(rect.right() - 24.0 - 48.0, rect.center().y), vec2(96.0, 96.0));
    let mut pui = ui.new_child(egui::UiBuilder::new().max_rect(play_rect).id_salt("hero-play"));
    let play = pui.add(
        IconButton::new(Icon::PlayArrow)
            .size(Size::L)
            .style(Style::Filled)
            .filled_icon(true)
            .tooltip("Слушать микс")
            .id_salt("hero-play-btn"),
    );
    if play.clicked() || resp.clicked() {
        out.push(Command::PlayTracks {
            tracks: mix.iter().map(|t| t.id).collect(),
            start: 0,
            context: "Твой микс".into(),
        });
    }
}
