//! The control's own menu, in every look: «Автоматически», every source
//! with how its text goes (picking a source never needs a window), then
//! where the shown text is kept (a group label: only online, made by the
//! player and kept in the library, or on the computer) with «Сохранить
//! текст» and the timing, and «Настройки текста…» until it was picked
//! once. The `menu` look makes the timing right here; the others have one
//! «Сделать подсветку…» row that opens their window or page. Where timing
//! cannot be made (a clip whose song is unclear) the menu says why. While
//! timing is made the control shows its progress; the result replaces the
//! text by itself.

use egui::{Id, Ui};
use nikgob_view::{Command, LyricsAvail, LyricsSync, ViewState};

use crate::Theme;
use crate::components::{Menu, MenuItem, anim};

/// What a menu row does.
enum Row {
    Auto,
    Source(nikgob_view::LyricsSource),
    Make(LyricsSync),
    /// The window or page where timing is made.
    Craft,
    /// «Сохранить текст».
    Save,
    Settings,
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
    let mut rows = Vec::new();
    let mut menu = Menu::new(("lyrics-menu", l.track)).align_center(true);
    if l.track.is_some() && !l.sources.is_empty() {
        // Says which text the player chose.
        let mut auto = MenuItem::new("Автоматически").selected(!l.picked);
        if let Some(src) = l.source.filter(|_| !l.picked) {
            auto = auto.shortcut(super::name(src));
        }
        menu = menu.item(auto).divider();
        rows.push(Row::Auto);
        for o in &l.sources {
            let mut item = MenuItem::new(super::name(o.source))
                .enabled(matches!(o.state, LyricsAvail::Ready(_)))
                .selected(l.picked && l.source == Some(o.source));
            let state = super::source_state(l, o.source);
            if !state.is_empty() {
                item = item.shortcut(state);
            }
            menu = menu.item(item);
            rows.push(Row::Source(o.source));
        }
    }
    let busy = super::busy(state).is_some();
    let follows = super::home(l).is_some() || super::no_craft(state).is_some() || !state.prefs.lyrics_settings_seen || busy;
    if !rows.is_empty() && follows {
        menu = menu.divider();
    }
    // Where the shown text is kept, and what can be done with it.
    let mut section = false;
    if let Some(home) = super::home(l).filter(|_| l.track.is_some()) {
        menu = menu.group_label(home);
        section = true;
    }
    if l.savable && !busy {
        menu = menu.item(MenuItem::new("Сохранить текст"));
        rows.push(Row::Save);
    }
    if look == super::Variant::Menu {
        // Making timing shows on the control; the actions come back after.
        let actions = if busy { Vec::new() } else { super::actions(state) };
        for (target, label) in &actions {
            menu = menu.item(MenuItem::new(*label));
            rows.push(Row::Make(*target));
        }
    } else if l.track.is_some() && (busy || !super::actions(state).is_empty()) {
        // The window shows the timing being made too.
        menu = menu.item(MenuItem::new(if busy { "Подсветка делается…" } else { "Сделать подсветку…" }));
        rows.push(Row::Craft);
    }
    if let Some(why) = super::no_craft(state) {
        menu = menu.group_label(why);
        section = true;
    }
    // Offered until it was picked once (the settings remember).
    if !state.prefs.lyrics_settings_seen {
        if section || rows.iter().any(|r| matches!(r, Row::Save | Row::Make(_) | Row::Craft)) {
            menu = menu.divider();
        }
        menu = menu.item(MenuItem::new("Настройки текста…"));
        rows.push(Row::Settings);
    }
    // Nothing to offer (no text yet, settings seen): no empty menu.
    if rows.is_empty() {
        ctx.data_mut(|d| d.insert_temp(resp.id.with("md3e-menu-open"), false));
        return false;
    }

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
