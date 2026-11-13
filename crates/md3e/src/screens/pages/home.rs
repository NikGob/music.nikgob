//! Главная (home-v2 spec): the card at the top («Миры», [`worlds`]), the
//! greeting with «Только скачанное», the shortcuts, «Миксы для тебя»
//! («Любимые», «Микс дня», the named mixes, the moments), «Слушать
//! снова», «Подборки дня», «Из твоей библиотеки», «Новое для тебя» and
//! «Новые релизы» — on one [`Grid`]: a wider window shows more cards in a
//! row, not larger ones ([`CARD_MAX`]). Every row under a header
//! (the mixes, the songs in columns of two, the tiles of the day, the
//! albums) with more than fit goes on sideways ([`row`]): Shift + the
//! wheel or a sideways wheel, a drag, the arrows at its ends under the
//! pointer; the plain wheel always scrolls the page.
//!
//! One rule for clicks (§4): what the app put together (a mix, a tile of
//! the day, a song row) plays; a real thing (an album, a playlist, an
//! artist, a shortcut) opens; ▶ under the pointer always plays; ⋮ and a
//! right click open the same menu.

use egui::{Id, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{
    Command, HomeRow, HomeView, LaneCard, MixCard, MixId, PlaylistCard, Route, ShelfAlbum, Shortcut, Status,
    StreamAlbumCard, StreamPage, StreamTrackRow, TileKind, TrackRow, ViewState,
};

use super::{PageState, cards, widgets, worlds};
use crate::screens::stage::{Part, Stage};

mod empty;
mod refresh;
mod row;
#[cfg(test)]
pub use empty::EMPTY_LINE;
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, FilterChip, Size, Style, anim},
    fit::Fit,
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

/// The widest the page gets; wider windows centre it. The cards keep their
/// size on a wide page ([`CARD_MAX`]): it runs as wide as the search bar
/// over it on any usual screen.
pub const MAX_W: f32 = 2560.0;
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
/// «Слушать снова» and «Новое для тебя» stand in columns of this many
/// songs; the columns that do not fit go on sideways.
pub const SONG_LINES: usize = 2;
/// A tile of the day.
pub const OF_DAY_H: f32 = 112.0;
/// The shortcuts and the song rows go four to a row from this width.
pub const FOUR_FROM: f32 = 1488.0;
/// Below this width the rows go two to a row and the tiles of the day two.
pub const NARROW_BELOW: f32 = 1000.0;
/// Below this width (the page of a window under M3's expanded class: 840
/// less the rail, 96, and the margins, 2 × 24) a row has three mix or
/// album cards, as wide as they come out.
pub const CARDS_CAPPED_FROM: f32 = 696.0;
/// From [`CARDS_CAPPED_FROM`] a mix or an album card is at most this wide:
/// a wider page shows more of them.
pub const CARD_MAX: f32 = 200.0;
/// From [`FOUR_FROM`] a column of songs is at most this wide: a wider page
/// shows more columns.
pub const ROW_MAX: f32 = 440.0;
/// A tile of the day is at most this wide in a full row…
pub const OF_DAY_MAX: f32 = 440.0;
/// …and at most this wide when a few tiles share a row.
pub const OF_DAY_STRETCH: f32 = 560.0;
/// The shortcuts go at most this many to a row (the runtime makes eight:
/// two rows of four).
pub const SHORTCUTS_PER_LINE: usize = 4;

/// How many shortcuts the page shows: six in rows of three, eight in rows
/// of four on a wide window (home v2 §12: one constant to change).
pub const fn shortcuts_shown(per_row: usize) -> usize {
    if per_row >= 4 { 8 } else { 6 }
}

/// A click on a song of «Слушать снова» starts a radio from it (true) or
/// plays the shelf in order from it (home v2 §12, recommended: the radio).
pub const LISTEN_AGAIN_RADIO: bool = true;

/// The page's column: where it is and how many of each kind of item stand
/// in a row of it, [`GUTTER`] apart (each row its own count, the same
/// left edge).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub left: f32,
    pub width: f32,
}

impl Grid {
    pub fn new(left: f32, available: f32) -> Self {
        let width = available.min(MAX_W);
        let left = left + ((available - width) / 2.0).max(0.0);
        Self { left, width }
    }

    /// Song columns in a row: two on a narrow page, three, four from
    /// [`FOUR_FROM`], and more columns of at most [`ROW_MAX`] wider still.
    pub fn rows_per_line(&self) -> usize {
        if self.width >= FOUR_FROM {
            Fit::at_most(self.width, GUTTER, ROW_MAX, 4).count
        } else if self.width < NARROW_BELOW {
            2
        } else {
            3
        }
    }

    /// Shortcuts in a row: as the songs, at most [`SHORTCUTS_PER_LINE`].
    pub fn shortcuts_per_line(&self) -> usize {
        self.rows_per_line().min(SHORTCUTS_PER_LINE)
    }

    /// Mixes and albums in a row: three on a narrow page, else as many as
    /// keep each at most [`CARD_MAX`] wide.
    pub fn cards_per_line(&self) -> usize {
        if self.width < CARDS_CAPPED_FROM { 3 } else { Fit::at_most(self.width, GUTTER, CARD_MAX, 3).count }
    }

    /// Tiles of the day in a row: two on a narrow page, four, and more of at
    /// most [`OF_DAY_MAX`] wider still.
    pub fn of_day_per_line(&self) -> usize {
        if self.width < NARROW_BELOW { 2 } else { Fit::at_most(self.width, GUTTER, OF_DAY_MAX, 4).count }
    }

    /// The fewest tiles of the day a row is split into, so that a few that
    /// share it are at most [`OF_DAY_STRETCH`] wide (a narrow page: one).
    pub fn of_day_least(&self) -> usize {
        if self.width < NARROW_BELOW { 1 } else { Fit::at_most(self.width, GUTTER, OF_DAY_STRETCH, 1).count }
    }

    /// The width of an item of `per_line` sharing the row.
    pub fn tile(&self, per_line: usize) -> f32 {
        Fit::even(self.width, GUTTER, per_line).size
    }

    /// The rect of item `i` of `per_line` on a row of `h` at `top`, `gap`
    /// between rows.
    pub fn cell(&self, i: usize, per_line: usize, top: f32, h: f32, gap: f32) -> Rect {
        let per_line = per_line.max(1);
        let w = self.tile(per_line);
        let (row, k) = (i / per_line, i % per_line);
        Rect::from_min_size(pos2(self.left + k as f32 * (w + GUTTER), top + row as f32 * (h + gap)), vec2(w, h))
    }
}

/// The rows a list of `n` items takes, `per_line` to a row.
fn lines(n: usize, per_line: usize) -> usize {
    n.div_ceil(per_line.max(1))
}

/// `n` songs in columns of [`SONG_LINES`]: the columns and the lines the
/// tallest of them has.
pub fn song_columns(n: usize) -> (usize, usize) {
    (n.div_ceil(SONG_LINES), n.min(SONG_LINES))
}

/// The place of song `k` of a column in the column's `cell`.
fn song_cell(cell: Rect, k: usize) -> Rect {
    Rect::from_min_size(pos2(cell.left(), cell.top() + k as f32 * ROW_H), vec2(cell.width(), ROW_H))
}

/// The sections of the page, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Shortcuts,
    Mixes,
    ListenAgain,
    OfDay,
    FromLibrary,
    New,
    Releases,
}

// The settings name the sections from `nikgob_view::HomeSection`; these
// are for the tests.
#[cfg_attr(not(test), allow(dead_code))]
impl Section {
    /// Every section, in the page's own order.
    pub const ALL: [Self; 7] = [
        Self::Shortcuts,
        Self::Mixes,
        Self::ListenAgain,
        Self::OfDay,
        Self::FromLibrary,
        Self::New,
        Self::Releases,
    ];

    /// Its name in «Разделы главной».
    pub fn title(self) -> &'static str {
        match self {
            Self::Shortcuts => "Быстрый доступ",
            Self::Mixes => "Миксы для тебя",
            Self::ListenAgain => "Слушать снова",
            Self::OfDay => "Плитки дня",
            Self::FromLibrary => "Из библиотеки",
            Self::New => "Новое",
            Self::Releases => "Релизы",
        }
    }
}

/// The page's section for a line of «Разделы главной» (the same ones, in
/// the same order).
pub fn section_of(key: nikgob_view::HomeSection) -> Section {
    use nikgob_view::HomeSection as H;
    match key {
        H::Shortcuts => Section::Shortcuts,
        // The day's mixes stand in «Миксы для тебя» now.
        H::Daily | H::Mixes => Section::Mixes,
        H::ListenAgain => Section::ListenAgain,
        H::OfDay => Section::OfDay,
        H::FromLibrary => Section::FromLibrary,
        H::New => Section::New,
        H::Releases => Section::Releases,
    }
}

/// The order and the sections turned on, as «Разделы главной» sets them;
/// empty: all of them in the page's own order.
pub fn order_of(prefs: &nikgob_view::Prefs) -> Vec<(Section, bool)> {
    if prefs.home_sections.is_empty() {
        return Vec::new();
    }
    prefs.home_order().into_iter().map(|s| (section_of(s.key), s.on)).collect()
}

/// `shown` in `order`: the sections it names, turned on, in its order;
/// those it does not name (a section new since it was set) after them, in
/// the page's own order. An empty `order` changes nothing.
pub fn arrange(shown: Vec<Section>, order: &[(Section, bool)]) -> Vec<Section> {
    if order.is_empty() {
        return shown;
    }
    let mut out: Vec<Section> = Vec::with_capacity(shown.len());
    for (s, on) in order {
        if *on && shown.contains(s) && !out.contains(s) {
            out.push(*s);
        }
    }
    out.extend(shown.iter().filter(|s| !order.iter().any(|(o, _)| o == *s)));
    out
}

/// «Любимые» for «Миксы для тебя»: the playlist of the liked songs, while
/// anything is liked.
pub fn liked_playlist(state: &ViewState) -> Option<&PlaylistCard> {
    state.library.playlists.iter().find(|p| p.id == nikgob_view::LIKED_PLAYLIST && p.track_count > 0)
}

/// What the page shows of `h`: «Только скачанное» (`local_only`) takes the
/// new music away, sections and all (the chip in the header brings it back).
/// `liked`: there is «Любимые» to stand among the mixes.
pub fn sections(h: &HomeView, liked: bool, local_only: bool) -> Vec<Section> {
    let mut out = Vec::new();
    if !h.shortcuts.is_empty() {
        out.push(Section::Shortcuts);
    }
    if liked || !h.lanes.is_empty() {
        out.push(Section::Mixes);
    }
    if !h.listen_again.is_empty() {
        out.push(Section::ListenAgain);
    }
    if !h.tiles.is_empty() || h.daylist.is_some() {
        out.push(Section::OfDay);
    }
    if !h.from_library.is_empty() {
        out.push(Section::FromLibrary);
    }
    if !local_only && !h.discover.is_empty() {
        out.push(Section::New);
    }
    if !local_only && !h.new_releases.is_empty() {
        out.push(Section::Releases);
    }
    out
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
    show_on(ui, state, pages, &Stage::REAL, out)
}

/// [`show`] for the page or the settings' stage, which notes where each
/// section is and opens no sheet of its own.
pub fn show_on(ui: &mut Ui, state: &ViewState, pages: &mut PageState, stage: &Stage, out: &mut Vec<Command>) -> Option<Route> {
    let h = &state.home;
    let mut nav = None;
    ui.spacing_mut().item_spacing.y = 0.0;
    let grid = Grid::new(ui.cursor().left(), ui.available_width());
    let local_only = state.prefs.mixes_downloaded_only;

    if h.loading {
        super::brand::loading(ui);
        return None;
    }
    ui.add_space(8.0);
    // No music at all, as the library counts it: the ways in under one
    // line. With «Только скачанное» on the page keeps its header, so the
    // chip that emptied it can be turned off again.
    if h.library_empty && !local_only {
        if asks_name(&state.prefs) {
            name_prompt(ui, grid, pages, out);
        }
        empty::show(ui, grid, state, pages, true, out);
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

    let available = sections(h, liked_playlist(state).is_some(), local_only);
    // Nothing to show of what there is (nothing downloaded under «Только
    // скачанное», too little music for the rows yet): the ways in under
    // the header, without the line (its chip stays to turn it off).
    if available.is_empty() && stage.live {
        empty::show(ui, grid, state, pages, false, out);
        return nav;
    }
    let order = order_of(&state.prefs);
    let drawn: Vec<(Section, f32)> = if stage.live {
        arrange(available.clone(), &order).into_iter().map(|s| (s, 1.0)).collect()
    } else {
        fading_sections(ui, stage, &available, &order)
    };
    for (k, (section, opacity)) in drawn.iter().enumerate() {
        ui.add_space(if k == 0 { 16.0 } else { SECTION_GAP });
        let i = n + k;
        let top = ui.cursor().top();
        let r = ui.scope(|ui| {
            if *opacity < 0.999 {
                ui.multiply_opacity(*opacity);
            }
            match section {
                Section::Shortcuts => shortcuts(ui, grid, i, stage.live, state, out),
                Section::Mixes => mixes(ui, grid, i, stage.live, state, out),
                Section::ListenAgain => listen_again(ui, grid, i, stage.live, state, pages, out),
                Section::OfDay => of_day(ui, grid, i, stage.live, state, out),
                Section::FromLibrary => from_library(ui, grid, i, stage.live, state, out),
                Section::New => new_music(ui, grid, i, stage.live, state, pages, out),
                Section::Releases => releases(ui, grid, i, stage.live, state, out),
            }
        }).inner;
        let bottom = ui.min_rect().bottom().max(top);
        stage.part(ui.ctx(), Part::Home(*section), Rect::from_x_y_ranges(grid.left..=grid.left + grid.width, top..=bottom));
        nav = r.or(nav);
    }
    if stage.live {
        blend_sheet(ui.ctx(), state, out);
    }
    nav
}

/// On the stage, a section that was just turned on or off fades instead of
/// popping. The real page keeps the instant cut.
fn fading_sections(ui: &Ui, stage: &Stage, available: &[Section], order: &[(Section, bool)]) -> Vec<(Section, f32)> {
    if order.is_empty() {
        return available.iter().copied().map(|s| (s, 1.0)).collect();
    }
    let ctx = ui.ctx();
    let motion = Theme::get(ctx).motion.fast_spatial.without_bounce();
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for (s, on) in order {
        if !available.contains(s) {
            continue;
        }
        seen.push(*s);
        let k = anim::fraction(ctx, stage.id("home-sec").with(*s), if *on { 1.0 } else { 0.0 }, motion);
        if k > 0.02 {
            out.push((*s, k));
        }
    }
    for s in available {
        if !seen.contains(s) {
            out.push((*s, 1.0));
        }
    }
    out
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
    right.spacing_mut().item_spacing.x = 8.0;
    let chip = right.add(chip);
    let downloaded_clicked = chip.clicked();
    let mut job = TypeRole::HeadlineMedium
        .style()
        .job(greeting_line(state.home.greeting_hour, &state.prefs.user_name), s.on_surface);
    job.wrap = egui::text::TextWrapping::truncate_at_width((chip.rect.left() - row.left() - 16.0).max(48.0));
    let g = crate::typography::layout(ui.painter(), job);
    ui.painter().galley(pos2(row.left(), row.center().y - g.size().y / 2.0), g, s.on_surface);
    if downloaded_clicked {
        out.push(downloaded_only(!on));
    }
}

/// Whether the queue plays «Моя волна» started from `preset` (`None`: the
/// plain one), and whether it plays now.
pub(super) fn wave_playing(state: &ViewState, preset: Option<nikgob_view::MixKey>) -> (bool, bool) {
    let ours = state.queue.radio.as_ref().and_then(|r| r.wave.as_ref()).is_some_and(|w| w.preset == preset);
    (ours, ours && state.player.status == Status::Playing)
}

/// What a click on the wave's chip asks: pause it while it plays, play it
/// on when it is the queue's, else start it.
fn wave_command(ours: bool, playing: bool, preset: Option<nikgob_view::MixKey>) -> Command {
    if playing {
        Command::Pause
    } else if ours {
        Command::TogglePlay
    } else {
        Command::PlayWave { preset }
    }
}

/// The name card's height: 16 above, the question (24), 4 + 12 under it,
/// the field's row (56), 16 below — the old 164 less the line that
/// explained where the name goes.
const NAME_PROMPT_H: f32 = 140.0;

/// The first start's question under the greeting: how to call the user,
/// «Готово» or «Пропустить» (asked no more either way).
fn name_prompt(ui: &mut Ui, grid: Grid, pages: &mut PageState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let w = grid.width.min(560.0);
    let h = NAME_PROMPT_H;
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

// ---- Shortcuts.

fn shortcuts(ui: &mut Ui, grid: Grid, n: usize, live: bool, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let per = grid.shortcuts_per_line();
    let count = state.home.shortcuts.len().min(shortcuts_shown(per));
    let items = &state.home.shortcuts[..count];
    let h = lines(count, per) as f32 * (SHORTCUT_H + 8.0) - 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let mut nav = None;
    // Every shortcut the page has, not only the ones that fit: a page
    // that narrows and widens again shows the same cards, not new ones.
    let all = &state.home.shortcuts;
    let keys = all.iter().map(shortcut_key).collect();
    let changes = refresh::track(ui.ctx(), Id::new("home-shortcuts"), live, keys, || all.to_vec());
    if live {
        // Back from another page, the tiles are there at once.
        let uris: Vec<String> = items.iter().flat_map(shortcut_pictures).collect();
        crate::covers::hold(ui.ctx(), uris.iter().map(String::as_str));
    }
    entering(ui, rect, n, "shortcuts", |ui| {
        for (i, sc) in items.iter().enumerate() {
            let cell = grid.cell(i, per, rect.top(), SHORTCUT_H, 8.0);
            changes.card(ui, i, cell, i, |ui, old| match old {
                Some(old) => {
                    let _ = cards::shortcut(ui, cell, old, state, i, &mut Vec::new());
                }
                None => nav = cards::shortcut(ui, cell, sc, state, i, out).or(nav.take()),
            });
        }
    });
    nav
}

/// The pictures a shortcut draws (as [`cards::shortcut`] picks them).
pub(super) fn shortcut_pictures(sc: &Shortcut) -> Vec<String> {
    match sc {
        Shortcut::Release(a) => a.cover.iter().map(|c| c.uri()).collect(),
        Shortcut::Playlist(_, covers) if covers.len() >= 4 => covers.iter().take(4).map(|c| c.uri()).collect(),
        Shortcut::Playlist(p, covers) => p.cover.as_ref().or(covers.first()).map(|c| c.uri()).into_iter().collect(),
        Shortcut::Artist(a) => a.image.iter().map(|c| c.uri()).collect(),
    }
}

/// What a shortcut shows ([`refresh`]): its name and the pictures it
/// draws, not what else its cover carries (its colour seed, its ambient
/// picture), which a page built again may have found meanwhile.
pub(super) fn shortcut_key(sc: &Shortcut) -> u64 {
    let uri = |c: &Option<nikgob_view::Cover>| c.as_ref().map(|c| c.uri());
    match sc {
        Shortcut::Release(a) => refresh::key(("release", a.id, &a.title, &a.artist, uri(&a.cover))),
        Shortcut::Playlist(p, covers) => {
            refresh::key(("playlist", p.id, &p.name, uri(&p.cover), covers.iter().map(|c| c.uri()).collect::<Vec<_>>()))
        }
        Shortcut::Artist(a) => refresh::key(("artist", a.id, &a.name, uri(&a.image))),
    }
}

// ---- «Миксы для тебя».

/// The lane playing now: its queue was started from it and plays.
pub(super) fn lane_playing(state: &ViewState, l: &LaneCard) -> bool {
    state.player.status == Status::Playing && state.player.context.as_deref() == Some(l.context().as_str())
}

/// The queue plays from «Любимые» (its page is the queue's), as a mix's
/// card says of its own ([`lane_playing`]).
fn liked_playing(state: &ViewState, p: &PlaylistCard) -> bool {
    state.player.status == Status::Playing && state.player.context_page == Some(Route::Playlist(p.id))
}

/// Where the playing song is, in seconds (the heart of «Любимые» beats
/// with it).
fn song_s(state: &ViewState) -> f32 {
    state.player.position_now().as_secs_f32()
}

/// A mix card's height on `grid`: its cover and the lines under it.
fn mix_card_h(grid: Grid) -> f32 {
    grid.tile(grid.cards_per_line()) + cards::MIX_TEXT_H
}

/// The row `name` of «Любимые» (`liked`, first) and the mixes `lanes`
/// under the section's header in `rect`: as many as fit, the rest
/// sideways.
#[allow(clippy::too_many_arguments)]
fn mix_row(
    ui: &mut Ui,
    grid: Grid,
    rect: Rect,
    name: &str,
    liked: Option<&PlaylistCard>,
    lanes: &[&LaneCard],
    live: bool,
    state: &ViewState,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let h = &state.home;
    let per = grid.cards_per_line();
    let with_new = state.prefs.mixes_with_new && !state.prefs.mixes_downloaded_only;
    let mut nav = None;
    // «Любимые», then «Моя волна», then the mixes.
    let first = usize::from(liked.is_some()) + 1;
    let wave_at = first - 1;
    let keys = liked
        .iter()
        .map(|p| refresh::key(("liked", p.id, &p.name)))
        .chain(std::iter::once(refresh::key("wave")))
        .chain(lanes.iter().map(|l| lane_key(l)))
        .collect();
    let changes = refresh::track(ui.ctx(), Id::new(("home-row", name)), live, keys, || {
        liked
            .iter()
            .map(|p| MixItem::Liked((*p).clone()))
            .chain(std::iter::once(MixItem::Wave))
            .chain(lanes.iter().map(|l| MixItem::Lane((*l).clone())))
            .collect()
    });
    let pitch = grid.tile(per) + GUTTER;
    let at = row::Row {
        name,
        top: rect.top() + HEADER_H + HEADER_GAP,
        card_h: mix_card_h(grid),
        arrows_at: grid.tile(per) / 2.0,
        per,
        n: first + lanes.len(),
        live,
        bleed: 0.0,
    };
    let near = row::show(ui, grid, at, |ui, i, cell| {
        changes.card(ui, i, cell, refresh::place(cell, grid.left, pitch), |ui, old| {
            match old {
                Some(MixItem::Liked(p)) => {
                    let _ = cards::liked_playlist(ui, cell, p, false, state.player.status, 0.0, &mut Vec::new());
                    return;
                }
                Some(MixItem::Lane(l)) => {
                    let click = cards::MixClick::of(&state.prefs, l.key);
                    let _ = cards::mix(ui, cell, l, with_new, false, false, state.player.status, click, None, &mut Vec::new());
                    return;
                }
                Some(MixItem::Wave) => {
                    cards::wave(ui, cell, false, false, state.player.status, &mut Vec::new());
                    return;
                }
                None => {}
            }
            if let Some(p) = liked.filter(|_| i == 0) {
                nav = cards::liked_playlist(ui, cell, p, liked_playing(state, p), state.player.status, song_s(state), out).or(nav.take());
                return;
            }
            if i == wave_at {
                let (ours, playing) = wave_playing(state, None);
                cards::wave(ui, cell, playing, ours, state.player.status, out);
                return;
            }
            let l = lanes[i - first];
            // «Смешать с…»: another profile to mix it with.
            let can_blend = l.blend_base.is_some_and(|b| h.blend_profiles.iter().any(|p| p.key != b));
            let click = cards::MixClick::of(&state.prefs, l.key);
            // A card plays as long as its wave does or its list (started
            // from its page or its menu, whatever the click does).
            let playing = wave_playing(state, Some(l.key)).1 || lane_playing(state, l);
            nav = cards::mix(ui, cell, l, with_new, can_blend, playing, state.player.status, click, None, out).or(nav.take());
        });
    });
    if near {
        ask_more(ui.ctx(), HomeRow::Mixes, first + lanes.len(), out);
    }
    nav
}

/// A row scrolled near its end showing `n` cards asks for more of it,
/// once for each length: a row whose source has no more stays as it is
/// and asks no more.
fn ask_more(ctx: &egui::Context, row: HomeRow, n: usize, out: &mut Vec<Command>) {
    let id = Id::new(("home-row-more", row));
    if ctx.data(|d| d.get_temp::<usize>(id)) != Some(n) {
        ctx.data_mut(|d| d.insert_temp(id, n));
        out.push(Command::HomeMore(row));
    }
}

/// A card of the row of mixes as it was ([`refresh`]).
#[derive(Clone)]
enum MixItem {
    Liked(PlaylistCard),
    Wave,
    Lane(LaneCard),
}

/// What a mix's card shows ([`refresh`]).
fn lane_key(l: &LaneCard) -> u64 {
    refresh::key((l.key, &l.title, &l.artists, l.covers.first(), &l.new_cover))
}

fn mixes(ui: &mut Ui, grid: Grid, n: usize, live: bool, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let lanes: Vec<&LaneCard> = state.home.lanes.iter().collect();
    let local_only = state.prefs.mixes_downloaded_only;
    let total = HEADER_H + HEADER_GAP + mix_card_h(grid);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), total), Sense::hover());
    let mut nav = None;
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
        let (picked, _) = super::connected(&mut gui, Id::new("home-mixes-new"), &labels, usize::from(with_new), disabled);
        if let Some(p) = picked {
            out.push(Command::SetPref(nikgob_view::PrefPatch::MixesWithNew(p == 1)));
        }
        nav = mix_row(ui, grid, rect, "mixes", liked_playlist(state), &lanes, live, state, out);
    });
    nav
}

/// The profiles picked in «Смешать с…»: the card's first, then up to two.
#[derive(Clone, Debug, Default)]
struct Blend {
    base: u32,
    with: Vec<u32>,
    open: bool,
}

/// At most this many profiles join the card's in a blend.
const BLEND_WITH: usize = 2;

/// «Смешать с…» of a mix: the bottom sheet of the other taste profiles,
/// one or two of them picked, then «Слушать».
pub(super) fn blend_sheet(ctx: &egui::Context, state: &ViewState, out: &mut Vec<Command>) {
    let key = Id::new("home-blend");
    let Some(mut b) = ctx.data(|d| d.get_temp::<Blend>(key)) else { return };
    let profiles = &state.home.blend_profiles;
    let base_title = profiles.iter().find(|p| p.key == b.base).map(|p| p.title.clone());
    let others: Vec<&nikgob_view::BlendProfile> = profiles.iter().filter(|p| p.key != b.base).collect();
    if base_title.is_none() || others.is_empty() {
        b.open = false;
    }
    b.with.retain(|k| others.iter().any(|p| p.key == *k));
    let mut open = b.open;
    let mut play = false;
    // Its height: the title, the chips in rows of what fits, the button.
    let rows = others.len().div_ceil(3).max(1) as f32;
    let height = 36.0 + 48.0 + rows * 40.0 + 24.0 + 40.0 + 24.0;
    crate::components::BottomSheet::new("home-blend-sheet").height(height).max_width(640.0).show(ctx, &mut open, |ui| {
        let s = Theme::get(ui.ctx()).scheme;
        let title = format!("Смешать «{}» с", base_title.as_deref().unwrap_or_default());
        ui.add(egui::Label::new(TypeRole::TitleLarge.style().rich(title, s.on_surface)).truncate());
        ui.add_space(16.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for p in &others {
                let on = b.with.contains(&p.key);
                let full = b.with.len() >= BLEND_WITH;
                let chip = FilterChip::new(p.title.as_str(), on).enabled(on || !full).id_salt(("home-blend-chip", p.key));
                if ui.add(chip).clicked() {
                    if on {
                        b.with.retain(|k| *k != p.key);
                    } else if !full {
                        b.with.push(p.key);
                    }
                }
            }
        });
        ui.add_space(24.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let listen = Button::new("Слушать").icon(Icon::PlayArrow).style(Style::Filled).enabled(!b.with.is_empty()).id_salt("home-blend-play");
            play = ui.add(listen).clicked();
        });
    });
    if play {
        let mut profiles = vec![b.base];
        profiles.extend(b.with.iter().copied());
        out.push(Command::PlayBlend { profiles });
        open = false;
    }
    // Closed, it stays drawn while it slides away.
    b.open = open;
    ctx.data_mut(|d| d.insert_temp(key, b));
}

/// Opens «Смешать с…» of the profile `base`.
pub(super) fn open_blend(ctx: &egui::Context, base: u32) {
    ctx.data_mut(|d| d.insert_temp(Id::new("home-blend"), Blend { base, with: Vec::new(), open: true }));
}

// ---- «Слушать снова».

fn listen_again(ui: &mut Ui, grid: Grid, n: usize, live: bool, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let rows = &state.home.listen_again;
    let per = grid.rows_per_line();
    let (columns, tall) = song_columns(rows.len());
    let body = tall as f32 * ROW_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + body), Sense::hover());
    let mut nav = None;
    entering(ui, rect, n, "again", |ui| {
        if section_header(ui, grid, rect, "Слушать снова", Some(("Слушать всё", Some(Icon::PlayArrow)))) {
            out.push(Command::PlayAll { tracks: rows.iter().map(|t| t.id).collect(), context: "Слушать снова".into() });
        }
        let at = row::Row { name: "again", top: rect.top() + HEADER_H + HEADER_GAP, card_h: body, arrows_at: body / 2.0, per, n: columns, live, bleed: 8.0 };
        let keys = rows.chunks(SONG_LINES).map(|c| refresh::key(c.iter().map(|t| (t.id, &t.title, &t.cover)).collect::<Vec<_>>())).collect();
        let changes = refresh::track(ui.ctx(), Id::new(("home-row", "again")), live, keys, || rows.chunks(SONG_LINES).map(<[TrackRow]>::to_vec).collect());
        let pitch = grid.tile(per) + GUTTER;
        let near = row::show(ui, grid, at, |ui, column, cell| {
            changes.card(ui, column, cell, refresh::place(cell, grid.left, pitch), |ui, old: Option<&Vec<TrackRow>>| {
                if let Some(old) = old {
                    for (k, t) in old.iter().enumerate() {
                        let _ = cards::song_row(ui, song_cell(cell, k), &t.title, &t.artist, t.cover.as_ref().map(|c| c.uri()), false, None, ("again", column * SONG_LINES + k));
                    }
                    return;
                }
                for k in 0..SONG_LINES {
                    let i = column * SONG_LINES + k;
                    let Some(t) = rows.get(i) else { break };
                    let playing = state.player.current.as_ref().is_some_and(|c| c.id == t.id);
                    let (links, routes) = crate::screens::artist_links::library_spans(&t.artists, &t.artist);
                    let o = cards::song_row_linked(ui, song_cell(cell, k), &t.title, &t.artist, &links, t.cover.as_ref().map(|c| c.uri()), playing, None, ("again", i));
                    if let Some(r) = o.link.and_then(|k| routes.get(k).cloned()) {
                        nav = Some(r);
                    }
                    if o.clicked {
                        out.push(play_again(rows, i));
                    }
                    nav = pages.track_context_menu(&o.response, t, state, out).or(nav.take());
                }
            });
        });
        if near {
            ask_more(ui.ctx(), HomeRow::ListenAgain, rows.len(), out);
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

fn of_day(ui: &mut Ui, grid: Grid, n: usize, live: bool, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let tiles = day_tiles(&state.home);
    // Tiles that fit share the row (three in thirds rather than leave a
    // hole) as long as none grows past `OF_DAY_STRETCH`; more of them go
    // on sideways.
    let per = of_day_per(grid.of_day_per_line(), tiles.len()).max(grid.of_day_least());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + OF_DAY_H), Sense::hover());
    entering(ui, rect, n, "of-day", |ui| {
        section_header(ui, grid, rect, "Подборки дня", None);
        let at = row::Row {
            name: "of-day",
            top: rect.top() + HEADER_H + HEADER_GAP,
            card_h: OF_DAY_H,
            arrows_at: OF_DAY_H / 2.0,
            per,
            n: tiles.len(),
            live,
            bleed: 0.0,
        };
        let keys = tiles.iter().map(|m| refresh::key((m.id, &m.title, &m.subtitle, m.covers.first()))).collect();
        let changes = refresh::track(ui.ctx(), Id::new(("home-row", "of-day")), live, keys, || tiles.iter().map(|m| (*m).clone()).collect());
        let pitch = grid.tile(per) + GUTTER;
        row::show(ui, grid, at, |ui, i, cell| {
            changes.card(ui, i, cell, refresh::place(cell, grid.left, pitch), |ui, old: Option<&MixCard>| {
                let m = old.unwrap_or(tiles[i]);
                let artist = matches!(m.id, MixId::Tile(TileKind::ArtistOfDay | TileKind::Comeback));
                let photo = artist.then(|| artist_photo(state, &m.title)).flatten();
                if old.is_some() {
                    cards::of_day(ui, cell, m, photo.as_deref(), i, false, &mut Vec::new());
                } else {
                    cards::of_day(ui, cell, m, photo.as_deref(), i, tile_playing(state, m), out);
                }
            });
        });
    });
    None
}

/// The cards of «Подборки дня»: the daylist (the playlist of this part of
/// the day) first, then the tiles of the day. It is a selection of the day
/// as they are: one tile like them, played on a click like them.
pub fn day_tiles(h: &HomeView) -> Vec<&MixCard> {
    h.daylist.iter().chain(h.tiles.iter()).collect()
}

/// The queue plays the tile of the day `m` (started from it: the queue
/// says its name, «микса «…»» or «радио «…»»).
pub(super) fn tile_playing(state: &ViewState, m: &MixCard) -> bool {
    state.player.status == Status::Playing
        && state.player.context.as_deref().is_some_and(|c| c.ends_with(&format!("«{}»", m.title)) && !m.title.is_empty())
}

/// Tiles of the day in view of `n`, `fit` to a row: those that fit share
/// the row, more keep the row's own count.
pub fn of_day_per(fit: usize, n: usize) -> usize {
    if n <= fit { n.max(1) } else { fit.max(1) }
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

fn from_library(ui: &mut Ui, grid: Grid, n: usize, live: bool, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let h = &state.home;
    let per = grid.cards_per_line();
    let albums: &[ShelfAlbum] = &h.from_library;
    let w = grid.tile(per);
    let card_h = w + cards::ALBUM_TEXT_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + card_h), Sense::hover());
    let mut nav = None;
    let title = if h.history { "Из твоей библиотеки" } else { "С чего начать" };
    entering(ui, rect, n, "shelf", |ui| {
        section_header(ui, grid, rect, title, None);
        let at = row::Row { name: "shelf", top: rect.top() + HEADER_H + HEADER_GAP, card_h, arrows_at: w / 2.0, per, n: albums.len(), live, bleed: 0.0 };
        let keys = albums.iter().map(|a| refresh::key((a.card.id, &a.card.title, &a.card.cover, a.heard, a.total))).collect();
        let changes = refresh::track(ui.ctx(), Id::new(("home-row", "shelf")), live, keys, || albums.to_vec());
        row::show(ui, grid, at, |ui, i, cell| {
            changes.card(ui, i, cell, refresh::place(cell, grid.left, w + GUTTER), |ui, old: Option<&ShelfAlbum>| {
                let a = old.unwrap_or(&albums[i]);
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
                if old.is_some() {
                    return;
                }
                if o.play {
                    out.push(Command::PlayRelease { id: c.id, start: None, shuffle: false });
                } else if o.clicked {
                    nav = Some(Route::Release(c.id));
                }
                nav = widgets::release_menus(&o.response, o.more.as_ref(), c, out).or(nav.take());
            });
        });
    });
    nav
}

// ---- «Новое для тебя».

fn new_music(ui: &mut Ui, grid: Grid, n: usize, live: bool, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let h = &state.home;
    let per = grid.rows_per_line();
    let rows = &h.discover;
    let (columns, tall) = song_columns(rows.len());
    let body = tall as f32 * ROW_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + body), Sense::hover());
    let mut nav = None;
    entering(ui, rect, n, "new", |ui| {
        if section_header(ui, grid, rect, "Новое для тебя", Some(("Слушать всё", Some(Icon::PlayArrow)))) {
            out.push(Command::PlayMix { id: MixId::Discover });
        }
        let at = row::Row { name: "new", top: rect.top() + HEADER_H + HEADER_GAP, card_h: body, arrows_at: body / 2.0, per, n: columns, live, bleed: 8.0 };
        let keys = rows.chunks(SONG_LINES).map(|c| refresh::key(c.iter().map(|t| (&t.remote, &t.title, &t.cover_url)).collect::<Vec<_>>())).collect();
        let changes = refresh::track(ui.ctx(), Id::new(("home-row", "new")), live, keys, || rows.chunks(SONG_LINES).map(<[StreamTrackRow]>::to_vec).collect());
        let pitch = grid.tile(per) + GUTTER;
        let near = row::show(ui, grid, at, |ui, column, cell| {
            changes.card(ui, column, cell, refresh::place(cell, grid.left, pitch), |ui, old: Option<&Vec<StreamTrackRow>>| {
                if let Some(old) = old {
                    for (k, t) in old.iter().enumerate() {
                        let _ = cards::song_row(ui, song_cell(cell, k), &t.title, &t.artist, t.cover_url.clone(), false, Some(t.liked), ("new", column * SONG_LINES + k));
                    }
                    return;
                }
                for k in 0..SONG_LINES {
                    let i = column * SONG_LINES + k;
                    let Some(t) = rows.get(i) else { break };
                    let playing = state.player.current.as_ref().is_some_and(|c| t.track == Some(c.id));
                    let (links, routes) = crate::screens::artist_links::stream_spans(&t.artists, &t.artist);
                    let o = cards::song_row_linked(ui, song_cell(cell, k), &t.title, &t.artist, &links, t.cover_url.clone(), playing, Some(t.liked), ("new", i));
                    if let Some(r) = o.link.and_then(|k| routes.get(k).cloned()) {
                        nav = Some(r);
                    }
                    if o.liked {
                        out.push(Command::LikeStream { track: t.remote.clone(), on: !t.liked });
                    } else if o.clicked && t.playable {
                        out.push(play_discover(&h.discover, i));
                    }
                    // A service song's one list (its ♥ is on the row).
                    if !crate::components::Menu::context_idle(&o.response, ("discover-menu", o.response.id)) {
                        let wish = widgets::WishIndex::of(ui.ctx(), &state.downloads.wishlist);
                        let wished = (!t.has_local).then(|| wish.of_track(t.track, None)).flatten();
                        let rows = super::placed(super::streams::stream_track_rows(t, wished, &state.dislikes), &state.prefs.track_menu, |a| a.menu_item(false));
                        let (menu, acts) = widgets::menu_of(("discover-menu", o.response.id), rows);
                        let act = menu.context_menu(&o.response).and_then(|k| acts.get(k).copied());
                        nav = super::streams::track_pick(&o.response.ctx, act, t, None, pages, out).or(nav.take());
                    }
                }
            });
        });
        if near {
            ask_more(ui.ctx(), HomeRow::New, rows.len(), out);
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

// ---- «Новые релизы».

/// «ZUTOMAYO · 2025»: the artist and the year of a new release.
fn release_line(a: &StreamAlbumCard) -> (String, String) {
    (a.artist.clone(), a.year.map(|y| y.to_string()).unwrap_or_default())
}

/// «Новые релизы» shows this many albums in its row; «Все» has the rest.
pub const RELEASES_ROW: usize = 24;

fn releases(ui: &mut Ui, grid: Grid, n: usize, live: bool, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    let per = grid.cards_per_line();
    let all = &state.home.new_releases;
    let albums: &[StreamAlbumCard] = &all[..all.len().min(RELEASES_ROW)];
    let w = grid.tile(per);
    let card_h = w + cards::ALBUM_TEXT_H;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H + HEADER_GAP + card_h), Sense::hover());
    let mut nav = None;
    entering(ui, rect, n, "releases", |ui| {
        if section_header(ui, grid, rect, "Новые релизы", Some(("Все", Some(Icon::ArrowForward)))) {
            nav = Some(Route::NewReleases);
        }
        let at = row::Row { name: "releases", top: rect.top() + HEADER_H + HEADER_GAP, card_h, arrows_at: w / 2.0, per, n: albums.len(), live, bleed: 0.0 };
        let keys = albums.iter().map(|a| refresh::key((&a.remote, &a.title, &a.cover_url))).collect();
        let changes = refresh::track(ui.ctx(), Id::new(("home-row", "releases")), live, keys, || albums.to_vec());
        row::show(ui, grid, at, |ui, i, cell| {
            changes.card(ui, i, cell, refresh::place(cell, grid.left, w + GUTTER), |ui, old: Option<&StreamAlbumCard>| {
                let a = old.unwrap_or(&albums[i]);
                let (artist, tail) = release_line(a);
                let o = cards::album(ui, cell, &a.title, &artist, &tail, a.cover_url.clone(), ("release", i));
                if old.is_some() {
                    return;
                }
                if o.play {
                    out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false });
                } else if o.clicked {
                    nav = Some(Route::Stream(StreamPage::Album(a.remote.clone())));
                }
                nav = super::streams::album_menus(&o.response, o.more.as_ref(), a, &state.downloads.wishlist, out).or(nav.take());
            });
        });
    });
    nav
}

/// «Новые релизы» → «Все» ([`Route::NewReleases`]): every new album the
/// row has, in its order, as cards of the same grid; a click opens one, ▶
/// plays it, ⋮ and the right click: a service album's menu.
pub fn new_releases(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    super::detail::with_bar(ui, "Новые релизы", pages, |ui, _| {
        let albums = &state.home.new_releases;
        if albums.is_empty() {
            widgets::empty_state(ui, Icon::Album, "Новых релизов пока нет", "", None);
            return None;
        }
        ui.spacing_mut().item_spacing.y = 0.0;
        let grid = Grid::new(ui.cursor().left(), ui.available_width());
        ui.add_space(8.0);
        let per = grid.cards_per_line();
        let card_h = grid.tile(per) + cards::ALBUM_TEXT_H;
        let (block, _) = ui.allocate_exact_size(vec2(ui.available_width(), lines(albums.len(), per) as f32 * (card_h + 24.0) - 24.0), Sense::hover());
        let mut nav = None;
        for (i, a) in albums.iter().enumerate() {
            let cell = grid.cell(i, per, block.top(), card_h, 24.0);
            let (artist, tail) = release_line(a);
            let o = cards::album(ui, cell, &a.title, &artist, &tail, a.cover_url.clone(), ("new-releases", i));
            if o.play {
                out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false });
            } else if o.clicked {
                nav = Some(Route::Stream(StreamPage::Album(a.remote.clone())));
            }
            nav = super::streams::album_menus(&o.response, o.more.as_ref(), a, &state.downloads.wishlist, out).or(nav.take());
        }
        ui.add_space(widgets::SECTION_GAP);
        nav
    })
}

// ---- Empty.

#[cfg(test)]
mod tests;
