//! The control's own menu, in every look (docs/lyrics-control.md):
//! «Автоматически», every source with how its text is timed (picking a
//! source never needs a window), «Искать текст заново» (every source asked
//! again, whatever was found before), and «Настройки текста…» until it was
//! picked once. The window and page looks have just one row more, «Подсветка…»,
//! that opens them: everything else about the text is there. The `menu`
//! look does it all right here: how the shown text is seen («По словам»,
//! «По строкам», «Без подсветки», as far as it has that timing),
//! «Сохранить текст», the timing, and why timing cannot be made (a clip
//! whose song is unclear). While timing is made the control shows its
//! progress and the menu offers «Отменить подсветку» (the window and the
//! page their own «Отменить»); the result replaces the text by itself.
//! In every look «Изменить текст…» opens the editor (in a window, on the
//! page in the «Экран» look), and «Вернуть найденный» takes the user's
//! own text away (to the Trash, with undo).

use egui::{Id, Ui};
use nikgob_view::{Command, LyricsAvail, LyricsSync, ViewState};

use crate::Theme;
use crate::components::{Menu, MenuItem, anim};

/// What a menu row does.
enum Row {
    Auto,
    Source(nikgob_view::LyricsSource),
    /// «Искать текст заново».
    Search,
    /// See the text so.
    Show(LyricsSync),
    /// Make timing so, deep or the normal way.
    Make(LyricsSync, bool),
    /// The window or page where timing is made.
    Craft,
    /// Stop the timing being made.
    Cancel,
    /// «Сохранить текст».
    Save,
    /// «Изменить текст…»: the editor.
    Edit,
    /// «Вернуть найденный»: the user's own text goes.
    Found,
    Settings,
}

/// The menu as it is built: a divider goes between two groups of rows
/// (never first or last).
struct Rows {
    menu: Option<Menu>,
    rows: Vec<Row>,
    /// The group under way has something in it.
    open: bool,
    /// A group ended: the next row starts a new one.
    ended: bool,
}

impl Rows {
    fn start(&mut self) {
        if std::mem::take(&mut self.ended) {
            self.menu = self.menu.take().map(Menu::divider);
        }
        self.open = true;
    }

    fn item(&mut self, item: MenuItem, row: Row) {
        self.start();
        self.menu = self.menu.take().map(|m| m.item(item));
        self.rows.push(row);
    }

    fn label(&mut self, text: &str) {
        self.start();
        self.menu = self.menu.take().map(|m| m.group_label(text));
    }

    /// Ends the group under way (if it has anything).
    fn split(&mut self) {
        if std::mem::take(&mut self.open) {
            self.ended = true;
        }
    }
}

/// «Изменить текст…» (not while timing is made) and, with the user's own
/// text there, «Вернуть найденный».
fn edit_rows(m: &mut Rows, state: &ViewState, busy: bool) {
    if super::editor::editable(state) && !busy {
        m.item(MenuItem::new("Изменить текст…"), Row::Edit);
    }
    if super::has_own(&state.lyrics) {
        m.item(MenuItem::new("Вернуть найденный"), Row::Found);
    }
}

/// The control and its menu, making timing as `look` does; true for
/// «Настройки текста…».
pub(super) fn control(ui: &mut Ui, state: &ViewState, look: super::Variant, left: Option<f32>, out: &mut Vec<Command>) -> bool {
    let ctx = ui.ctx().clone();
    let motion = Theme::get(&ctx).motion;
    let id = ui.id().with("lyrics-control");
    let open = ctx.data(|d| d.get_temp::<bool>(id.with("md3e-menu-open"))).unwrap_or(false);
    let turn = anim::f32(&ctx, Id::new(("lyrics-menu-chevron", id)), if open { 1.0 } else { 0.0 }, motion.fast_spatial);
    let resp = super::chip(ui, state, turn, left, out);

    let l = &state.lyrics;
    let mut m = Rows { menu: Some(Menu::new(("lyrics-menu", l.track)).align_center(left.is_none())), rows: Vec::new(), open: false, ended: false };
    if l.track.is_some() && !l.sources.is_empty() {
        // Says which text the player chose.
        let mut auto = MenuItem::new("Автоматически").selected(!l.picked);
        if let Some(src) = l.source.filter(|_| !l.picked) {
            auto = auto.shortcut(super::source_name(l, src));
        }
        m.item(auto, Row::Auto);
        m.split();
        for o in &l.sources {
            let mut item = MenuItem::new(super::source_name(l, o.source))
                .enabled(matches!(o.state, LyricsAvail::Ready(_)))
                .selected(l.picked && l.source == Some(o.source));
            let state = super::source_state(l, o.source);
            if !state.is_empty() {
                item = item.shortcut(state);
            }
            m.item(item, Row::Source(o.source));
        }
        // Every source asked again (not while one still answers).
        let searching = l.sources.iter().any(|o| o.state == LyricsAvail::Loading);
        m.item(MenuItem::new("Искать текст заново").enabled(!searching), Row::Search);
        m.split();
    }
    let busy = super::busy(state).is_some();
    if look == super::Variant::Menu {
        // How the shown text is seen.
        let seen = super::seen(l);
        for (sync, label) in super::shows(state) {
            m.item(MenuItem::new(label).selected(sync == seen), Row::Show(sync));
        }
        m.split();
        // What can be done with the shown text. Making timing shows on the
        // control; the actions come back after.
        if l.savable && !busy {
            m.item(MenuItem::new(super::save_label(state)), Row::Save);
        }
        edit_rows(&mut m, state, busy);
        let actions = if busy { Vec::new() } else { super::actions(state) };
        // Each with its deep variant right after it (word timing).
        for a in &actions {
            m.item(MenuItem::new(a.label), Row::Make(a.target, false));
            if let Some(deep) = a.deep {
                m.item(MenuItem::new(deep), Row::Make(a.target, true));
            }
        }
        if busy {
            m.item(MenuItem::new("Отменить подсветку"), Row::Cancel);
        }
        if let Some(why) = super::no_craft(state) {
            m.label(why);
        }
    } else if l.track.is_some() && (busy || l.source.is_some()) {
        // Everything else about the text is in the window or on the page,
        // which shows the timing being made too.
        let label = match (busy, l.savable) {
            (true, _) => "Подсветка делается…",
            (false, true) => "Подсветка и сохранение…",
            (false, false) => "Подсветка…",
        };
        m.item(MenuItem::new(label), Row::Craft);
        edit_rows(&mut m, state, busy);
    }
    m.split();
    // Offered until it was picked once (the settings remember).
    if !state.prefs.lyrics_settings_seen {
        m.item(MenuItem::new("Настройки текста…"), Row::Settings);
    }
    let Rows { menu, rows, .. } = m;
    // Nothing to offer (no text yet, settings seen): no empty menu.
    let Some(menu) = menu.filter(|_| !rows.is_empty()) else {
        ctx.data_mut(|d| d.insert_temp(resp.id.with("md3e-menu-open"), false));
        return false;
    };

    match menu.attach(&resp).and_then(|i| rows.get(i)) {
        Some(Row::Auto) => {
            if let Some(track) = l.track {
                out.push(Command::PickLyricsSource(track, None));
            }
            false
        }
        Some(Row::Source(source)) => {
            if let Some(track) = l.track {
                out.push(Command::PickLyricsSource(track, Some(*source)));
            }
            false
        }
        Some(Row::Search) => {
            if let Some(track) = l.track {
                out.push(Command::SearchLyricsAgain(track));
            }
            false
        }
        Some(Row::Show(sync)) => {
            super::show_as(state, *sync, out);
            false
        }
        Some(Row::Make(target, deep)) => {
            super::make(state, *target, *deep, out);
            false
        }
        Some(Row::Craft) => {
            match look {
                super::Variant::Dialog => super::dialog::open(&ctx, resp.rect),
                super::Variant::Page => super::request(&ctx, super::Request::Page),
                super::Variant::Menu => {}
            }
            false
        }
        Some(Row::Cancel) => {
            out.push(Command::CancelCraft);
            false
        }
        Some(Row::Save) => {
            super::save(state, out);
            false
        }
        Some(Row::Edit) => {
            match look {
                super::Variant::Page => super::editor::open_on_page(&ctx, state),
                _ => super::editor::open(&ctx, state, resp.rect),
            }
            false
        }
        Some(Row::Found) => {
            if let Some(track) = l.track {
                out.push(Command::DropOwnLyrics(track));
            }
            false
        }
        Some(Row::Settings) => {
            out.push(Command::SetPref(nikgob_view::PrefPatch::LyricsSettingsSeen));
            true
        }
        None => false,
    }
}
