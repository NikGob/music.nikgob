//! The control's own menu, in every look (docs/lyrics-control.md):
//! «Автоматически», every source with how its text goes (picking a source
//! never needs a window); how the shown text is seen («По словам», «По
//! строкам», «Без подсветки», as far as it has that timing); then where
//! it is kept (a group label: only online, made by the player and kept in
//! the library, or on the computer) with «Сохранить текст» and the timing;
//! and «Настройки текста…» until it was picked once. The `menu` look makes
//! the timing right here; the others have one «Сделать подсветку…» row
//! that opens their window or page. Where timing cannot be made (a clip
//! whose song is unclear) the menu says why. While timing is made the
//! control shows its progress and the menu offers «Отменить подсветку»;
//! the result replaces the text by itself.

use egui::{Id, Ui};
use nikgob_view::{Command, LyricsAvail, LyricsSync, ViewState};

use crate::Theme;
use crate::components::{Menu, MenuItem, anim};

/// What a menu row does.
enum Row {
    Auto,
    Source(nikgob_view::LyricsSource),
    /// See the text so.
    Show(LyricsSync),
    Make(LyricsSync),
    /// The window or page where timing is made.
    Craft,
    /// Stop the timing being made.
    Cancel,
    /// «Сохранить текст».
    Save,
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

/// The control and its menu, making timing as `look` does; true for
/// «Настройки текста…».
pub(super) fn control(ui: &mut Ui, state: &ViewState, look: super::Variant, out: &mut Vec<Command>) -> bool {
    let ctx = ui.ctx().clone();
    let motion = Theme::get(&ctx).motion;
    let id = ui.id().with("lyrics-control");
    let open = ctx.data(|d| d.get_temp::<bool>(id.with("md3e-menu-open"))).unwrap_or(false);
    let turn = anim::f32(&ctx, Id::new(("lyrics-menu-chevron", id)), if open { 1.0 } else { 0.0 }, motion.fast_spatial);
    let resp = super::chip(ui, state, turn, out);

    let l = &state.lyrics;
    let mut m = Rows { menu: Some(Menu::new(("lyrics-menu", l.track)).align_center(true)), rows: Vec::new(), open: false, ended: false };
    if l.track.is_some() && !l.sources.is_empty() {
        // Says which text the player chose.
        let mut auto = MenuItem::new("Автоматически").selected(!l.picked);
        if let Some(src) = l.source.filter(|_| !l.picked) {
            auto = auto.shortcut(super::name(src));
        }
        m.item(auto, Row::Auto);
        m.split();
        for o in &l.sources {
            let mut item = MenuItem::new(super::name(o.source))
                .enabled(matches!(o.state, LyricsAvail::Ready(_)))
                .selected(l.picked && l.source == Some(o.source));
            let state = super::source_state(l, o.source);
            if !state.is_empty() {
                item = item.shortcut(state);
            }
            m.item(item, Row::Source(o.source));
        }
        m.split();
    }
    // How the shown text is seen.
    let seen = super::seen(l);
    for (sync, label) in super::shows(state) {
        m.item(MenuItem::new(label).selected(sync == seen), Row::Show(sync));
    }
    m.split();
    // Where the shown text is kept, and what can be done with it.
    let busy = super::busy(state).is_some();
    if let Some(home) = super::home(l).filter(|_| l.track.is_some()) {
        m.label(home);
    }
    if l.savable && !busy {
        m.item(MenuItem::new("Сохранить текст"), Row::Save);
    }
    if look == super::Variant::Menu {
        // Making timing shows on the control; the actions come back after.
        let actions = if busy { Vec::new() } else { super::actions(state) };
        for (target, label) in &actions {
            m.item(MenuItem::new(*label), Row::Make(*target));
        }
    } else if l.track.is_some() && (busy || !super::actions(state).is_empty()) {
        // The window shows the timing being made too.
        m.item(MenuItem::new(if busy { "Подсветка делается…" } else { "Сделать подсветку…" }), Row::Craft);
    }
    if busy {
        m.item(MenuItem::new("Отменить подсветку"), Row::Cancel);
    }
    if let Some(why) = super::no_craft(state) {
        m.label(why);
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
        Some(Row::Show(sync)) => {
            super::show_as(state, *sync, out);
            false
        }
        Some(Row::Make(target)) => {
            super::make(state, *target, out);
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
        Some(Row::Settings) => {
            out.push(Command::SetPref(nikgob_view::PrefPatch::LyricsSettingsSeen));
            true
        }
        None => false,
    }
}
