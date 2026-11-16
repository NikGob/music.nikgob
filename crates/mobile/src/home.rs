//! Главная: ours, as the desktop draws it (the card of the worlds, the
//! greeting, the shortcuts, the mixes, «Слушать снова»…), laid out for the
//! phone's width with its margins, under the status bar, in a list a finger
//! drags.

use egui::{Margin, Ui};
use nikgob_md3e::screens::phone::PhonePages;
use nikgob_view::{Command, Route, ViewState};

use crate::PageFrame;

pub(crate) fn show(ui: &mut Ui, frame: PageFrame, shell: &mut PhonePages, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    crate::scroll("phone-home", frame.reset)
        .show(ui, |ui| {
            ui.add_space(frame.top + 8.0);
            let nav = egui::Frame::new()
                .inner_margin(Margin::symmetric(16, 0))
                .show(ui, |ui| shell.home(ui, state, out))
                .inner;
            ui.add_space(24.0);
            nav
        })
        .inner
}
