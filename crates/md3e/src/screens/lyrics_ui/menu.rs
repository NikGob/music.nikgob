//! `menu`: no window at all. The control's own menu lists «Автоматически»,
//! every source with how its text goes, the timing that can be made for
//! the shown text and «Настройки текста…». While timing is made the
//! control shows its progress; the result replaces the text by itself.

use egui::{Id, Ui};
use nikgob_view::{Command, LyricsAvail, LyricsSync, ViewState};

use crate::Theme;
use crate::components::{Menu, MenuItem, anim};

/// What a menu row does.
enum Row {
    Auto,
    Source(nikgob_view::LyricsSource),
    Make(LyricsSync),
    Settings,
}

/// The control and its menu; true for «Настройки текста…».
pub(super) fn control(ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) -> bool {
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
            auto = auto.shortcut(src.label());
        }
        menu = menu.item(auto).divider();
        rows.push(Row::Auto);
        for o in &l.sources {
            menu = menu.item(
                MenuItem::new(o.source.label())
                    .shortcut(super::source_state(l, o.source))
                    .enabled(matches!(o.state, LyricsAvail::Ready(_)))
                    .selected(l.picked && l.source == Some(o.source)),
            );
            rows.push(Row::Source(o.source));
        }
        menu = menu.divider();
    }
    // Making timing shows on the control; the actions come back after.
    let actions = if super::busy(state).is_some() { Vec::new() } else { super::actions(state) };
    for (target, label) in &actions {
        menu = menu.item(MenuItem::new(*label));
        rows.push(Row::Make(*target));
    }
    if !actions.is_empty() {
        menu = menu.divider();
    }
    menu = menu.item(MenuItem::new("Настройки текста…"));
    rows.push(Row::Settings);

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
        Some(Row::Settings) => true,
        None => false,
    }
}
