//! Главная (home-v2 spec): the card at the top («Миры», [`worlds`]), the
//! greeting with «Только скачанное», the shortcuts, «Миксы для тебя»,
//! «Слушать снова», «Подборки дня», «Из твоей библиотеки», «Новое для
//! тебя» and «Новые релизы» — on a grid of 12 columns, without a single
//! sideways row, so the wheel always scrolls the page.
//!
//! One rule for clicks (§4): what the app put together (a mix, a tile of
//! the day, a song row) plays; a real thing (an album, a playlist, an
//! artist, a shortcut) opens; ▶ under the pointer always plays; ⋮ and a
//! right click open the same menu.

use egui::{Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{
    AnalysisProgress, Command, HomeView, LaneCard, MixCard, MixId, RecTarget, Route, ShelfAlbum, Shortcut, Status,
    StreamAlbumCard, StreamPage, StreamTrackRow, TileKind, TrackRow, ViewState,
};

use super::{PageState, cards, widgets, worlds};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, FilterChip, Menu, MenuItem, Size, Style, TooltipExt, anim},
};

pub(super) fn greeting(hour: u8) -> &'static str {
    match hour {
        5..=11 => "Доброе утро",
        12..=16 => "Добрый день",
        17..=22 => "Добрый вечер",
        _ => "Доброй ночи",
    }
}

/// «Добрый вечер, Ник», or the greeting alone without a name.
pub(super) fn greeting_line(hour: u8, name: &str) -> String {
    let name = nikgob_view::user_name(name);
    if name.is_empty() { greeting(hour).to_owned() } else { format!("{}, {name}", greeting(hour)) }
}

/// Главная asks for the name: the first start, no name yet.
pub(super) fn asks_name(p: &nikgob_view::Prefs) -> bool {
    p.name_prompt && p.user_name.trim().is_empty()
}

/// The widest the page gets; wider windows centre it.
pub const MAX_W: f32 = 1600.0;
/// The space between columns.
pub const GUTTER: f32 = 16.0;
/// Sections apart.
pub const SECTION_GAP: f32 = 32.0;
/// A section's header row and the space under it.
pub const HEADER_H: f32 = 40.0;
pub const HEADER_GAP: f32 = 8.0;
/// A shortcut, the rows of songs.
pub const SHORTCUT_H: f32 = 72.0;
pub const ROW_H: f32 = 72.0;
/// A tile of the day.
pub const OF_DAY_H: f32 = 112.0;
/// The shortcuts and the song rows go four to a row from this width.
pub const FOUR_FROM: f32 = 1488.0;
/// Below this width the rows go two to a row and the cards three.
pub const NARROW_BELOW: f32 = 1000.0;

/// How many shortcuts the page shows: six in rows of three, eight in rows
/// of four on a wide window (home v2 §12: one constant to change).
pub const fn shortcuts_shown(per_row: usize) -> usize {
    if per_row >= 4 { 8 } else { 6 }
}

/// A click on a song of «Слушать снова» starts a radio from it (true) or
/// plays the shelf in order from it (home v2 §12, recommended: the radio).
pub const LISTEN_AGAIN_RADIO: bool = true;

/// The 12 columns of the page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub left: f32,
    pub width: f32,
    pub col: f32,
}

impl Grid {
    pub fn new(left: f32, available: f32) -> Self {
        let width = available.min(MAX_W);
        let left = left + ((available - width) / 2.0).max(0.0);
        Self { left, width, col: (width - 11.0 * GUTTER) / 12.0 }
    }

    /// The width of `n` columns.
    pub fn span(&self, n: usize) -> f32 {
        n as f32 * self.col + (n.saturating_sub(1)) as f32 * GUTTER
    }

    /// The left edge of column `i`.
    pub fn x(&self, i: usize) -> f32 {
        self.left + i as f32 * (self.col + GUTTER)
    }

    /// Shortcuts and song rows in a row.
    pub fn rows_per_line(&self) -> usize {
        if self.width >= FOUR_FROM {
            4
        } else if self.width < NARROW_BELOW {
            2
        } else {
            3
        }
    }

    /// Mixes and albums in a row (each two columns; four when narrow).
    pub fn cards_per_line(&self) -> usize {
        if self.width < NARROW_BELOW { 3 } else { 6 }
    }

    /// Tiles of the day in a row.
    pub fn of_day_per_line(&self) -> usize {
        if self.width < NARROW_BELOW { 2 } else { 4 }
    }

    /// The rect of item `i` of `per_line` on a row of `h` at `top`, `gap`
    /// between rows.
    pub fn cell(&self, i: usize, per_line: usize, top: f32, h: f32, gap: f32) -> Rect {
        let span = 12 / per_line;
        let (row, k) = (i / per_line, i % per_line);
        Rect::from_min_size(pos2(self.x(k * span), top + row as f32 * (h + gap)), vec2(self.span(span), h))
    }
}

/// The rows a list of `n` items takes, `per_line` to a row.
fn lines(n: usize, per_line: usize) -> usize {
    n.div_ceil(per_line.max(1))
}

/// The sections of the page, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Shortcuts,
    Mixes,
    ListenAgain,
    OfDay,
    FromLibrary,
    New,
    Releases,
}

/// What the page shows of `h`: «Только скачанное» (`local_only`) takes the
/// new music away (its section says so in a quiet line).
pub fn sections(h: &HomeView, local_only: bool) -> Vec<Section> {
    let mut out = Vec::new();
    if !h.shortcuts.is_empty() {
        out.push(Section::Shortcuts);
    }
    if !h.lanes.is_empty() {
        out.push(Section::Mixes);
    }
    if !h.listen_again.is_empty() {
        out.push(Section::ListenAgain);
    }
    if !h.tiles.is_empty() {
        out.push(Section::OfDay);
    }
    if !h.from_library.is_empty() {
        out.push(Section::FromLibrary);
    }
    if local_only || !h.discover.is_empty() {
        out.push(Section::New);
    }
    if !local_only && !h.new_releases.is_empty() {
        out.push(Section::Releases);
    }
    out
}

/// Nothing to show at all: no music in the library and nothing heard.
pub fn is_empty(h: &HomeView) -> bool {
    h.shortcuts.is_empty()
        && h.lanes.is_empty()
        && h.from_library.is_empty()
        && h.listen_again.is_empty()
        && h.recently_added.is_empty()
        && h.continue_listening.is_empty()
        && h.tiles.is_empty()
}

/// «Разбираю звук библиотеки, миксы станут точнее · 40 %», while the
/// analysis runs.
pub fn analysis_line(a: &AnalysisProgress) -> Option<String> {
    (!a.complete()).then(|| {
        let pct = (a.fraction() * 100.0).floor() as u32;
        format!("Разбираю звук библиотеки, миксы станут точнее · {pct}\u{a0}%")
    })
}

/// The section's entrance, once per run: `n` from the top, 30 ms after the
/// one above (up to four), rising 24 on the default spatial spring and
/// fading in; reduced motion: a fade only.
fn entrance(ui: &Ui, n: usize) -> (f32, f32) {
    let ctx = ui.ctx();
    let now = anim::now(ctx);
    let t0 = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(Id::new("home-v2-enter"), || now));
    let t = (now - t0) as f32 - 0.03 * n.min(4) as f32;
    if t >= 1.0 {
        return (0.0, 1.0);
    }
    ctx.request_repaint();
    if t <= 0.0 {
        return (24.0, 0.0);
    }
    let theme = Theme::get(ctx);
    let alpha = (1.0 - theme.motion.default_effects.solve(1.0, 0.0, t).0).clamp(0.0, 1.0);
    let rise = if theme.reduced_motion { 0.0 } else { 24.0 * theme.motion.default_spatial.solve(1.0, 0.0, t).0 };
    (rise, alpha)
}

/// Draws `add` over `rect` as entrance `n` (moved and faded; input keeps
/// its place).
pub fn entering<R>(ui: &mut Ui, rect: Rect, n: usize, salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (dy, alpha) = entrance(ui, n);
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("home-v2-section", salt)));
    if alpha < 1.0 {
        child.set_opacity(alpha);
    }
    cards::moved(&mut child, vec2(0.0, dy), add)
}

pub fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let h = &state.home;
    let mut nav = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    let grid = Grid::new(ui.cursor().left(), ui.available_width());
    let local_only = state.prefs.mixes_downloaded_only;

    ui.add_space(8.0);
    if is_empty(h) {
        if asks_name(&state.prefs) {
            name_prompt(ui, grid, pages, out);
        }
        empty(ui, grid, pages, out);
        return None;
    }

    // The card at the top, collapsing when hidden.
    let mut n = 0;
    if let Some(r) = worlds::show(ui, grid, state, n, pages, out) {
        nav = Some(r);
    }
    if worlds::shown(ui.ctx()) {
        n += 1;
    }

    // The greeting and «Только скачанное».
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let row = Rect::from_min_size(pos2(grid.left, row.top()), vec2(grid.width, HEADER_H));
    entering(ui, row, n, "greeting", |ui| header(ui, row, state, out));
    n += 1;
    if asks_name(&state.prefs) {
        ui.add_space(8.0);
        name_prompt(ui, grid, pages, out);
    }

    let shown = sections(h, local_only);
    for (k, section) in shown.iter().enumerate() {
        ui.add_space(if k == 0 { 16.0 } else { SECTION_GAP });
        let i = n + k;
        let r = match section {
            Section::Shortcuts => shortcuts(ui, grid, i, state, out),
            Section::Mixes => mixes(ui, grid, i, state, out),
            Section::ListenAgain => listen_again(ui, grid, i, state, pages, out),
            Section::OfDay => of_day(ui, grid, i, state, out),
            Section::FromLibrary => from_library(ui, grid, i, state, out),
            Section::New => new_music(ui, grid, i, state, pages, out),
            Section::Releases => releases(ui, grid, i, state, out),
        };
        nav = r.or(nav);
    }
    lane_list(ui.ctx(), state, out);
    nav
}

/// The header of the page: «Доброй ночи, Ник» and at the right «Только
/// скачанное»; a long name ends in «…» before the chip.
fn header(ui: &mut Ui, row: Rect, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let on = state.prefs.mixes_downloaded_only;
    let chip = FilterChip::new("Только скачанное", on).icon(Icon::DownloadDone).id_salt("home-downloaded-only");
    let mut right = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(egui::Layout::right_to_left(egui::Align::Center))
            .id_salt("home-header-right"),
    );
    let chip = right.add(chip);
    let mut job = TypeRole::HeadlineMedium
        .style()
        .job(greeting_line(state.home.greeting_hour, &state.prefs.user_name), s.on_surface);
    job.wrap = egui::text::TextWrapping::truncate_at_width((chip.rect.left() - row.left() - 16.0).max(48.0));
    let g = crate::typography::layout(ui.painter(), job);
    ui.painter().galley(pos2(row.left(), row.center().y - g.size().y / 2.0), g, s.on_surface);
    if chip.md3_tooltip("Миксы, «Слушать снова» и подборки — только из скачанных треков", None).clicked() {
        out.push(downloaded_only(!on));
    }
}

/// The first start's question under the greeting: how to call the user,
/// «Готово» or «Пропустить» (asked no more either way).
fn name_prompt(ui: &mut Ui, grid: Grid, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let w = grid.width.min(560.0);
    let h = 164.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let card = Rect::from_min_size(pos2(grid.left, rect.top()), vec2(w, h));
    crate::components::paint::container(
        ui.painter(),
        card,
        crate::components::Radii::all(crate::tokens::shape::LARGE),
        s.surface_container_high,
        egui::Stroke::NONE,
    );
    let mut c = ui.new_child(UiBuilder::new().max_rect(card.shrink2(vec2(20.0, 16.0))).id_salt("home-name-prompt"));
    c.spacing_mut().item_spacing.y = 4.0;
    c.label(TypeRole::TitleMedium.style().rich("Как к тебе обращаться?", s.on_surface));
    c.label(TypeRole::BodyMedium.style().rich("Имя будет в приветствии на главной. Сменить можно в Настройках.", s.on_surface_variant));
    c.add_space(12.0);
    c.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let buttons = Button::new("Пропустить").natural_width(ui) + Button::new("Готово").natural_width(ui) + 16.0;
        let field = crate::components::TextField::new(&mut pages.name_draft, "Имя")
            .width((ui.available_width() - buttons).max(120.0))
            .id_salt("home-name-field")
            .show(ui);
        let name = nikgob_view::user_name(&pages.name_draft);
        if ui.add(Button::new("Пропустить").style(Style::Text).id_salt("home-name-skip")).clicked() {
            out.push(Command::SetPref(nikgob_view::PrefPatch::NamePrompt(false)));
        }
        let done = ui.add(Button::new("Готово").style(Style::Filled).enabled(!name.is_empty()).id_salt("home-name-done")).clicked();
        if (done || field.submitted) && !name.is_empty() {
            out.push(Command::SetPref(nikgob_view::PrefPatch::UserName(name)));
            pages.name_synced = false;
        }
    });
}

/// «Только скачанное» switched to `on`.
fn downloaded_only(on: bool) -> Command {
    Command::SetPref(nikgob_view::PrefPatch::MixesDownloadedOnly(on))
}

/// A section header over the grid: the title at the left and, optionally,
/// a text button whose text ends on the right edge (by its label, not its
/// container). True when the button was clicked.
fn section_header(ui: &mut Ui, grid: Grid, rect: Rect, title: &str, action: Option<(&str, Option<Icon>)>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let row = Rect::from_min_size(pos2(grid.left, rect.top()), vec2(grid.width, HEADER_H));
    let g = crate::typography::layout(ui.painter(), TypeRole::TitleLarge.style().job(title, s.on_surface));
    ui.painter().galley(pos2(row.left(), row.center().y - g.size().y / 2.0), g, s.on_surface);
    let Some((text, icon)) = action else { return false };
    let mut b = Button::new(text).style(Style::Text).id_salt(("home-header", title));
    if let Some(i) = icon {
        b = b.icon(i);
    }
    let w = b.natural_width(ui);
    // The label ends on the column's edge: the button's padding hangs out.
    let pad = Size::S.side_padding();
    let at = Rect::from_min_size(pos2(row.right() + pad - w, row.center().y - 20.0), vec2(w, 40.0));
    ui.put(at, b).clicked()
}

/// A quiet line under a header (BodyMedium, on_surface_variant).
fn quiet_at(ui: &Ui, at: egui::Pos2, text: &str, width: f32) -> f32 {
    let s = Theme::get(ui.ctx()).scheme;
    let g = widgets::wrapped(ui, text, TypeRole::BodyMedium.style(), s.on_surface_variant, width, 2);
    let h = g.size().y;
    ui.painter().galley(at, g, s.on_surface_variant);
    h
}

// ---- Shortcuts.

fn shortcuts(ui: &mut Ui, grid: Grid, n: usize, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let per = grid.rows_per_line();
    let count = state.home.shortcuts.len().min(shortcuts_shown(per));
    let items = &state.home.shortcuts[..count];
    let h = lines(count, per) as f32 * (SHORTCUT_H + 8.0) - 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let mut nav = None;
    entering(ui, rect, n, "shortcuts", |ui| {
        for (i, sc) in items.iter().enumerate() {
            let cell = grid.cell(i, per, rect.top(), SHORTCUT_H, 8.0);
            nav = cards::shortcut(ui, cell, sc, state, i, out).or(nav.take());
        }
    });
    nav
}

// ---- «Миксы для тебя».

/// The lane playing now: its queue was started from it and plays.
fn lane_playing(state: &ViewState, l: &LaneCard) -> bool {
    state.player.status == Status::Playing
        && state.player.context.as_deref() == Some(format!("микса «{}»", l.lane.title()).as_str())
}

fn mixes(ui: &mut Ui, grid: Grid, n: usize, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let h = &state.home;
    let per = grid.cards_per_line();
    let card_w = grid.span(12 / per);
    let local_only = state.prefs.mixes_downloaded_only;
    let note = if local_only {
        Some("Включено «Только скачанное»: в миксах только треки с файлом.".to_owned())
    } else if !h.history {
        Some("Собраны из того, что у тебя есть. Станут точнее, когда послушаешь.".to_owned())
    } else {
        analysis_line(&state.analysis)
    };
    let note_h = if note.is_some() { 28.0 } else { 0.0 };
    let card_h = card_w + cards::MIX_TEXT_H;
    let body = lines(h.lanes.len(), per) as f32 * (card_h + 24.0) - 24.0;
    let total = HEADER_H + note_h + HEADER_GAP + body;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), total), Sense::hover());
    entering(ui, rect, n, "mixes", |ui| {
        let s = Theme::get(ui.ctx()).scheme;
        let g = crate::typography::layout(ui.painter(), TypeRole::TitleLarge.style().job("Миксы для тебя", s.on_surface));
        ui.painter().galley(pos2(grid.left, rect.top() + (HEADER_H - g.size().y) / 2.0), g, s.on_surface);
        // «Только моё | С новым»: a connected group at the right.
        let labels = ["Только моё", "С новым"];
        let gw: f32 = labels.iter().map(|l| Button::new(*l).size(Size::S).natural_width(ui)).sum::<f32>() + 2.0;
        let group = Rect::from_min_size(pos2(grid.left + grid.width - gw, rect.top()), vec2(gw, HEADER_H));
        let mut gui = ui.new_child(UiBuilder::new().max_rect(group).id_salt("home-mixes-group"));
        let with_new = state.prefs.mixes_with_new && !local_only;
        let disabled: &[usize] = if local_only { &[0, 1] } else { &[] };
        let (picked, rects) = super::connected(&mut gui, Id::new("home-mixes-new"), &labels, usize::from(with_new), disabled);
        if let Some(p) = picked {
            out.push(Command::SetPref(nikgob_view::PrefPatch::MixesWithNew(p == 1)));
        }
        let share = match state.prefs.radio_new_share {
            nikgob_view::RadioNewShare::More => "Примерно каждый третий трек новый: похожее из YouTube Music и Deezer.",
            _ => "Примерно каждый пятый трек новый: похожее из YouTube Music и Deezer.",
        };
        let tips = ["Скачанное, лайкнутое и треки из твоих плейлистов. Без нового.", share];
        for (k, r) in rects.iter().enumerate() {
            let _ = ui.interact(*r, Id::new(("home-mixes-tip", k)), Sense::hover()).md3_tooltip(tips[k], None);
        }
        let mut top = rect.top() + HEADER_H;
        if let Some(line) = &note {
            quiet_at(ui, pos2(grid.left, top + 4.0), line, grid.width);
            top += note_h;
        }
        top += HEADER_GAP;
        for (i, l) in h.lanes.iter().enumerate() {
            let cell = grid.cell(i, per, top, card_h, 24.0);
            cards::mix(ui, cell, l, with_new, lane_playing(state, l), state.player.status, out);
        }
    });
    None
}

/// «Список треков» of a mix: its songs in a dialog.
fn lane_list(ctx: &egui::Context, state: &ViewState, out: &mut Vec<Command>) {
    let key = Id::new("home-lane-list");
    let Some(lane) = ctx.data(|d| d.get_temp::<nikgob_view::Lane>(key)) else { return };
    let Some(l) = state.home.lanes.iter().find(|l| l.lane == lane) else {
        ctx.data_mut(|d| d.remove::<nikgob_view::Lane>(key));
        return;
    };
    let mut open = true;
    let n = l.tracks.len();
    let o = crate::components::Dialog::new("home-lane-list-dialog", l.lane.title())
        .supporting(format!("{n} {}", crate::screens::common::plural(n as u64, "трек", "трека", "треков")))
        .action("Закрыть")
        .action("Слушать")
        .max_width(560.0)
        .show_with(ctx, &mut open, |ui| {
            let s = Theme::get(ui.ctx()).scheme;
            egui::ScrollArea::vertical().max_height(420.0).id_salt("home-lane-list-scroll").show(ui, |ui| {
                for (i, t) in l.tracks.iter().enumerate() {
                    let line = format!("{}. {} · {}", i + 1, t.title, t.artist);
                    let r = ui.add(
                        egui::Label::new(TypeRole::BodyMedium.style().rich(line, s.on_surface))
                            .truncate()
                            .sense(Sense::click()),
                    );
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        out.push(Command::PlayTracks {
                            tracks: l.tracks.iter().map(|t| t.id).collect(),
                            start: i,
                            context: format!("микса «{}»", l.lane.title()),
                        });
                    }
                }
            });
        });
    if o.action == Some(1) {
        out.push(Command::PlayMix { id: l.id() });
    }
    if !open || o.action.is_some() {
        ctx.data_mut(|d| d.remove::<nikgob_view::Lane>(key));
    }
}

/// Opens «Список треков» of `lane`.
pub(super) fn open_lane_list(ctx: &egui::Context, lane: nikgob_view::Lane) {
    ctx.data_mut(|d| d.insert_temp(Id::new("home-lane-list"), lane));
}

// ---- «Слушать снова».

fn listen_again(ui: &mut Ui, grid: Grid, n: usize, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let rows = &state.home.listen_again;
    let per = grid.rows_per_line();
    let body = lines(rows.len(), per) as f32 * ROW_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + body), Sense::hover());
    let mut nav = None;
    entering(ui, rect, n, "again", |ui| {
        if section_header(ui, grid, rect, "Слушать снова", Some(("Слушать всё", Some(Icon::PlayArrow)))) {
            out.push(Command::PlayTracks { tracks: rows.iter().map(|t| t.id).collect(), start: 0, context: "Слушать снова".into() });
        }
        let top = rect.top() + HEADER_H + HEADER_GAP;
        for (i, t) in rows.iter().enumerate() {
            let cell = grid.cell(i, per, top, ROW_H, 0.0);
            let playing = state.player.current.as_ref().is_some_and(|c| c.id == t.id);
            let o = cards::song_row(ui, cell, &t.title, &t.artist, t.cover.as_ref().map(|c| c.uri()), playing, None, ("again", i));
            if o.clicked {
                out.push(play_again(rows, i));
            }
            nav = pages.track_context_menu(&o.response, t, state, out).or(nav.take());
        }
    });
    nav
}

/// A click on a song of «Слушать снова».
fn play_again(rows: &[TrackRow], i: usize) -> Command {
    if LISTEN_AGAIN_RADIO {
        super::radio_by_track(rows[i].id)
    } else {
        Command::PlayTracks { tracks: rows.iter().map(|t| t.id).collect(), start: i, context: "Слушать снова".into() }
    }
}

// ---- «Подборки дня».

fn of_day(ui: &mut Ui, grid: Grid, n: usize, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let tiles: Vec<&MixCard> = state.home.tiles.iter().take(4).collect();
    // Three tiles of a day share the row in thirds rather than leave a hole.
    let per = grid.of_day_per_line().min(tiles.len().max(1));
    let body = lines(tiles.len(), per) as f32 * (OF_DAY_H + 16.0) - 16.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + body), Sense::hover());
    entering(ui, rect, n, "of-day", |ui| {
        section_header(ui, grid, rect, "Подборки дня", None);
        let top = rect.top() + HEADER_H + HEADER_GAP;
        for (i, m) in tiles.iter().enumerate() {
            let cell = grid.cell(i, per, top, OF_DAY_H, 16.0);
            let artist = matches!(m.id, MixId::Tile(TileKind::ArtistOfDay | TileKind::Comeback));
            let photo = artist.then(|| artist_photo(state, &m.title)).flatten();
            if cards::of_day(ui, cell, m, photo.as_deref(), i) {
                out.push(Command::PlayMix { id: m.id });
            }
        }
    });
    None
}

/// The photo of the artist named `name`, from the library's artists.
pub(super) fn artist_photo(state: &ViewState, name: &str) -> Option<String> {
    let name = name.trim().to_lowercase();
    state
        .home
        .artist_mixes
        .iter()
        .chain(&state.library.artists)
        .chain(state.home.shortcuts.iter().filter_map(|s| match s {
            Shortcut::Artist(a) => Some(a),
            _ => None,
        }))
        .find(|a| a.image.is_some() && a.name.trim().to_lowercase() == name)
        .and_then(|a| a.image.as_ref().map(|c| c.uri()))
}

// ---- «Из твоей библиотеки» / «С чего начать».

/// «Kevin Penkin · слушал 5 из 52», «· не слушал»; without a history the
/// size: «OMORI · 179 треков».
pub fn shelf_tail(a: &ShelfAlbum, history: bool) -> String {
    if !history {
        let n = u64::from(a.total);
        return format!("{n} {}", crate::screens::common::plural(n, "трек", "трека", "треков"));
    }
    if a.heard == 0 { "не слушал".to_owned() } else { format!("слушал {} из {}", a.heard, a.total) }
}

fn from_library(ui: &mut Ui, grid: Grid, n: usize, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let h = &state.home;
    let per = grid.cards_per_line();
    let albums: Vec<&ShelfAlbum> = h.from_library.iter().take(per).collect();
    let w = grid.span(12 / per);
    let card_h = w + cards::ALBUM_TEXT_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + card_h), Sense::hover());
    let mut nav = None;
    let title = if h.history { "Из твоей библиотеки" } else { "С чего начать" };
    entering(ui, rect, n, "shelf", |ui| {
        section_header(ui, grid, rect, title, None);
        let top = rect.top() + HEADER_H + HEADER_GAP;
        for (i, a) in albums.iter().enumerate() {
            let cell = grid.cell(i, per, top, card_h, 24.0);
            let c = &a.card;
            let o = cards::album(
                ui,
                cell,
                &c.title,
                &c.artist,
                &shelf_tail(a, h.history),
                c.cover.as_ref().map(|c| c.uri()),
                ("shelf", i),
            );
            if o.play {
                out.push(Command::PlayRelease { id: c.id, start: None, shuffle: false });
            } else if o.clicked {
                nav = Some(Route::Release(c.id));
            }
            nav = widgets::release_menu(&o.response, c, out).or(nav.take());
        }
    });
    nav
}

// ---- «Новое для тебя».

fn new_music(ui: &mut Ui, grid: Grid, n: usize, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let h = &state.home;
    if state.prefs.mixes_downloaded_only {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + 40.0), Sense::hover());
        entering(ui, rect, n, "new-off", |ui| {
            section_header(ui, grid, rect, "Новое для тебя", None);
            let line = Rect::from_min_size(pos2(grid.left, rect.top() + HEADER_H + HEADER_GAP), vec2(grid.width, 40.0));
            let mut lui = ui.new_child(UiBuilder::new().max_rect(line).id_salt("home-new-off-line"));
            if widgets::quiet_line(
                &mut lui,
                "Новое скрыто: включено «Только скачанное», а у новой музыки ещё нет файлов.",
                Some(("Показать новое", None, Style::Text, "home-new-show")),
            ) {
                out.push(downloaded_only(false));
            }
        });
        return None;
    }
    let per = grid.rows_per_line();
    // Only whole rows: three to a row show six, four show four or eight.
    let mut count = h.discover.len().min(per * 2);
    if count > per {
        count -= count % per;
    }
    let rows = &h.discover[..count];
    let body = lines(count, per) as f32 * ROW_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + body), Sense::hover());
    let mut nav = None;
    entering(ui, rect, n, "new", |ui| {
        if section_header(ui, grid, rect, "Новое для тебя", Some(("Слушать всё", Some(Icon::PlayArrow)))) {
            out.push(Command::PlayMix { id: MixId::Discover });
        }
        let top = rect.top() + HEADER_H + HEADER_GAP;
        for (i, t) in rows.iter().enumerate() {
            let cell = grid.cell(i, per, top, ROW_H, 0.0);
            let playing = state.player.current.as_ref().is_some_and(|c| t.track == Some(c.id));
            let o = cards::song_row(ui, cell, &t.title, &t.artist, t.cover_url.clone(), playing, Some(t.liked), ("new", i));
            if o.liked {
                out.push(Command::LikeStream { track: t.remote.clone(), on: !t.liked });
            } else if o.clicked && t.playable {
                out.push(play_discover(&h.discover, i));
            }
            nav = discover_pick(discover_menu(("discover-menu", o.response.id), t).context_menu(&o.response), t, None, pages, out)
                .or(nav.take());
        }
    });
    nav
}

/// Plays the discoveries from the `i`-th one, as a queue named after the
/// section.
fn play_discover(rows: &[StreamTrackRow], i: usize) -> Command {
    Command::PlayStreamTracks {
        tracks: rows.iter().map(|r| r.remote.clone()).collect(),
        start: i,
        context: "Новое для тебя".into(),
    }
}

/// «Не нравится» / «Не рекомендовать исполнителя» of a discovery.
fn dislike(t: &StreamTrackRow, artist: bool) -> Command {
    let target = if artist {
        RecTarget::StreamArtist(t.artist.clone())
    } else {
        RecTarget::StreamTrack(t.remote.clone())
    };
    Command::RecFeedback { target, dislike: true }
}

fn discover_menu(key: impl egui::AsId, t: &StreamTrackRow) -> Menu {
    Menu::new(key)
        .item(MenuItem::new("Сыграть следующим").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new("В конец очереди").icon(Icon::QueueMusic).enabled(t.playable))
        .item(MenuItem::new(super::RADIO_BY_TRACK).icon(Icon::Radio))
        .item(MenuItem::new("В плейлист…").icon(Icon::PlaylistAdd))
        .item(MenuItem::new(if t.liked { "Убрать из любимого" } else { "В любимое" }).icon(Icon::Favorite).selected(t.liked))
        .item(MenuItem::new("К исполнителю").icon(Icon::Person).enabled(!t.artists.linked().is_empty()))
        .divider()
        .item(MenuItem::new("Не нравится").icon(Icon::ThumbDown))
        .item(MenuItem::new("Не рекомендовать исполнителя").icon(Icon::Block))
}

fn discover_pick(
    picked: Option<usize>,
    t: &StreamTrackRow,
    anchor: Option<Rect>,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    match picked? {
        0 => out.push(Command::PlayStreamNext(vec![t.remote.clone()])),
        1 => out.push(Command::EnqueueStream(vec![t.remote.clone()])),
        2 => out.push(super::streams::radio_by_stream(t)),
        3 => pages.add_stream_to_playlist(vec![t.remote.clone()], anchor),
        4 => out.push(Command::LikeStream { track: t.remote.clone(), on: !t.liked }),
        5 => return t.artist_id().cloned().map(|a| Route::Stream(StreamPage::Artist(a))),
        6 => out.push(dislike(t, false)),
        7 => out.push(dislike(t, true)),
        _ => {}
    }
    None
}

// ---- «Новые релизы».

/// «ZUTOMAYO · 2025»: the artist and the year of a new release.
fn release_line(a: &StreamAlbumCard) -> (String, String) {
    (a.artist.clone(), a.year.map(|y| y.to_string()).unwrap_or_default())
}

fn releases(ui: &mut Ui, grid: Grid, n: usize, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let per = grid.cards_per_line();
    let albums: Vec<&StreamAlbumCard> = state.home.new_releases.iter().take(per).collect();
    let w = grid.span(12 / per);
    let card_h = w + cards::ALBUM_TEXT_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + card_h), Sense::hover());
    let mut nav = None;
    entering(ui, rect, n, "releases", |ui| {
        if section_header(ui, grid, rect, "Новые релизы", Some(("Все", Some(Icon::ArrowForward)))) {
            nav = albums.first().and_then(|a| a.artist_id().cloned()).map(|a| Route::Stream(StreamPage::Artist(a)));
        }
        let top = rect.top() + HEADER_H + HEADER_GAP;
        for (i, a) in albums.iter().enumerate() {
            let cell = grid.cell(i, per, top, card_h, 24.0);
            let (artist, tail) = release_line(a);
            let o = cards::album(ui, cell, &a.title, &artist, &tail, a.cover_url.clone(), ("release", i));
            if o.play {
                out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false });
            } else if o.clicked {
                nav = Some(Route::Stream(StreamPage::Album(a.remote.clone())));
            }
        }
    });
    nav
}

// ---- Empty.

/// An empty library: what to do first.
fn empty(ui: &mut Ui, grid: Grid, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 420.0), Sense::hover());
    let mut c = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(grid.left, rect.top()), vec2(grid.width, 420.0))).id_salt("home-empty"));
    c.add_space(96.0);
    c.vertical_centered(|ui| {
        ui.label(TypeRole::HeadlineSmall.style().rich("Здесь соберётся твоя музыка", s.on_surface));
        ui.add_space(8.0);
        ui.set_max_width(480.0);
        ui.label(TypeRole::BodyMedium.style().rich_para(
            "Найди песню: поиск ищет и в YouTube Music, без входа. Понравилась — поставь лайк, и она останется у тебя.",
            s.on_surface_variant,
        ));
        ui.add_space(24.0);
        ui.horizontal(|ui| {
            let total = Button::new("Найти музыку").icon(Icon::Search).natural_width(ui)
                + Button::new("Добавить папку…").icon(Icon::Folder).natural_width(ui)
                + 8.0;
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new("Найти музыку").icon(Icon::Search).style(Style::Filled).id_salt("home-empty-search")).clicked() {
                pages.focus_search = true;
            }
            if ui.add(Button::new("Добавить папку…").icon(Icon::Folder).style(Style::Tonal).id_salt("home-empty-folder")).clicked()
                && let Some(dir) = rfd::FileDialog::new().set_title("Добавить папку").pick_folder()
            {
                out.push(Command::ImportFolder(dir));
            }
        });
        ui.add_space(8.0);
        if ui
            .add(Button::new("Перенести историю из Spotify или YouTube Music…").style(Style::Text).id_salt("home-empty-import"))
            .clicked()
            && let Some(path) = rfd::FileDialog::new().set_title("Выгрузка Spotify или Google Takeout").pick_file()
        {
            out.push(Command::ImportExportFiles(path));
        }
    });
}

#[cfg(test)]
mod tests;
