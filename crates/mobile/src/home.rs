//! Главная: the search bar pinned at the top (Metrolist's), then ours, as
//! the desktop draws it (the greeting, the shortcuts, the mixes, «Слушать
//! снова»…), laid out for the phone's width with its margins, in a list a
//! finger drags under the bar; the worlds on top are the phone's own row of
//! cards (worlds.rs).

use egui::{Id, Margin, Rect, Ui, UiBuilder, pos2};
use nikgob_md3e::screens::phone::PhonePages;
use nikgob_view::{Command, ViewState};

use crate::PageFrame;
use crate::parts::{self, PageOut};
use crate::worlds::Worlds;

pub(crate) fn show(ui: &mut Ui, frame: PageFrame, shell: &mut PhonePages, worlds: &mut Worlds, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
    let mut page = PageOut::default();
    let rect = frame.rect;
    let bar = parts::search_rect(rect, frame.top);
    page.search = parts::search_bar(ui, bar, Id::new("home-search"));
    let body = Rect::from_min_max(pos2(rect.left(), bar.bottom() + 8.0), rect.max);
    let mut bui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("phone-home-body"));
    bui.set_clip_rect(body.intersect(ui.clip_rect()));
    page.nav = crate::scroll("phone-home", frame.reset)
        .show(&mut bui, |ui| {
            ui.add_space(8.0);
            let world = worlds.show(ui, state, out);
            if state.prefs.home_card_shown() && !state.home.worlds.is_empty() {
                ui.add_space(16.0);
            }
            let nav = egui::Frame::new()
                .inner_margin(Margin::symmetric(16, 0))
                .show(ui, |ui| shell.home(ui, state, out))
                .inner;
            ui.add_space(24.0);
            world.or(nav)
        })
        .inner;
    page
}
