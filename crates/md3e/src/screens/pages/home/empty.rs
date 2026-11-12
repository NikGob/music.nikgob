//! Главная without music (a new install, the first run skipped) or with
//! nothing to show of it: the ways in as tiles — the services by their own
//! marks (a signed-in one checked; a click opens its account in Настройки →
//! «Сервисы»), a folder to add, the search that finds and downloads.

use egui::{Id, Rect, Sense, Ui, pos2, vec2};
use nikgob_view::{Account, Command, Service, ViewState};

use super::super::first_run::{Art, tile};
use super::super::{PageState, SettingId};
use super::{Grid, entering};
use crate::{Icon, Theme, TypeRole};

pub const EMPTY_LINE: &str = "Музыки пока нет";

/// A tile's side and the space between tiles.
const TILE_W: f32 = 152.0;
const TILE_MIN_W: f32 = 120.0;
const TILE_H: f32 = 128.0;
const GAP: f32 = 12.0;

/// What a tile leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    Service(Service),
    Folder,
    Search,
}

const WAYS: [(Way, &str); 5] = [
    (Way::Service(Service::Spotify), "Spotify"),
    (Way::Service(Service::Youtube), "YouTube Music"),
    (Way::Service(Service::Soundcloud), "SoundCloud"),
    (Way::Folder, "Добавить папку"),
    (Way::Search, "Найти музыку"),
];

/// The tiles in rows from the page's left edge, as its sections stand (all
/// five in one where they fit, else 3 + 2); `line`: «Музыки пока нет» over
/// them, as a section's title, only where the library has nothing at all
/// (under the header — nothing downloaded under «Только скачанное», too
/// little music for the rows — they stand without it).
pub(super) fn show(ui: &mut Ui, grid: Grid, state: &ViewState, pages: &mut PageState, line: bool, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let per = ((grid.width + GAP) / (TILE_MIN_W + GAP)).floor().clamp(1.0, 5.0) as usize;
    let rows = WAYS.len().div_ceil(per);
    let per = WAYS.len().div_ceil(rows);
    let w = ((grid.width - GAP * (per - 1) as f32) / per as f32).min(TILE_W);
    let title_h = if line { TypeRole::HeadlineSmall.style().line_height + 16.0 } else { 0.0 };
    let h = title_h + rows as f32 * (TILE_H + GAP) - GAP;
    ui.add_space(24.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let area = Rect::from_min_size(pos2(grid.left, rect.top()), vec2(grid.width, h));
    if line {
        let title = Rect::from_min_size(area.min, vec2(area.width(), title_h));
        entering(ui, title, 0, "empty-line", |ui| {
            let g = crate::typography::layout(ui.painter(), TypeRole::HeadlineSmall.style().job(EMPTY_LINE, s.on_surface));
            ui.painter().galley(title.min, g, s.on_surface);
        });
    }
    for (i, (way, label)) in WAYS.iter().enumerate() {
        let (row, k) = (i / per, i % per);
        let x = area.left() + k as f32 * (w + GAP);
        let cell = Rect::from_min_size(pos2(x, area.top() + title_h + row as f32 * (TILE_H + GAP)), vec2(w, TILE_H));
        let (art, signed) = match way {
            Way::Service(service) => (Art::Logo(*service), state.account(*service).is_some_and(|a| matches!(a.account, Account::LoggedIn { .. }))),
            Way::Folder => (Art::Icon(Icon::Folder), false),
            Way::Search => (Art::Icon(Icon::Search), false),
        };
        let clicked = entering(ui, cell, 1 + i.min(3), ("empty-way", i), |ui| tile(ui, cell, Id::new(("home-empty-way", i)), art, label, signed));
        if clicked {
            go(*way, state, pages, out);
        }
    }
}

fn go(way: Way, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    match way {
        // Spotify not connected: its steps (the user's own key) first.
        Way::Service(Service::Spotify) if !state.account(Service::Spotify).is_some_and(|a| a.account.is_logged_in()) => {
            pages.open_setting(SettingId::SpotifyPersonal);
        }
        Way::Service(Service::Spotify) => pages.open_setting(SettingId::Spotify),
        Way::Service(Service::Youtube) => pages.open_setting(SettingId::YouTube),
        Way::Service(_) => pages.open_setting(SettingId::SoundCloud),
        Way::Folder => super::super::dialogs::folder(|d| d.set_title("Добавить папку"), Command::ImportFolder, out),
        Way::Search => pages.focus_search = true,
    }
}
