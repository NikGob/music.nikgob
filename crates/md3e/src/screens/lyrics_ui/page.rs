//! `page`: «Текст песни», a page of its own like Настройки (it opens with
//! the forward page transition and goes back the same way). The sources
//! on the left with how each goes and the timing action under them; the
//! shown text on the right, following the song.
//!
//! The right side is where the lyrics editor will go (the text and its
//! times by hand, saved as a lyrics file next to the track): `text_side`
//! is its slot.

use egui::{Align, Layout, Ui, vec2};
use nikgob_view::{Command, LyricsAvail, Route, ViewState};

use crate::components::{Button, ListItem, Size, Style, segmented_list};
use crate::{Icon, Theme, TypeRole};

const LEFT: f32 = 320.0;
const GAP: f32 = 24.0;

/// The page; `bottom` is the room the floating player takes at the bottom.
pub(crate) fn show(ui: &mut Ui, state: &ViewState, bottom: f32, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let view = ui.clip_rect();
    ui.set_max_width(ui.available_width().min(1120.0));
    ui.add_space(8.0);
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 44.0), Layout::left_to_right(Align::Center), |ui| {
        ui.label(TypeRole::HeadlineMedium.style().rich("Текст песни", s.on_surface));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let b = Button::new("Настройки текста").icon(Icon::Settings).style(Style::Text).size(Size::S).id_salt("lyrics-page-settings");
            if ui.add(b).clicked() {
                super::request(ui.ctx(), super::Request::Settings);
            }
        });
    });
    match super::playing(state) {
        Some(p) => {
            ui.label(TypeRole::BodyLarge.style().rich(p, s.on_surface_variant));
        }
        None => {
            ui.add_space(16.0);
            ui.label(TypeRole::BodyLarge.style().rich("Ничего не играет", s.on_surface_variant));
            return None;
        }
    }
    ui.add_space(GAP);

    let top = ui.cursor().top();
    // As tall as the window leaves (the page scrolls only on small ones).
    let h = (view.bottom() - top - bottom - 16.0).max(360.0);
    let w = ui.available_width();
    let narrow = w < LEFT + GAP + 360.0;
    if narrow {
        sources_side(ui, w, state, out);
        ui.add_space(GAP);
        text_side(ui, vec2(w, h), state, out);
    } else {
        ui.allocate_ui_with_layout(vec2(w, h), Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            ui.allocate_ui_with_layout(vec2(LEFT, h), Layout::top_down(Align::Min), |ui| sources_side(ui, LEFT, state, out));
            text_side(ui, vec2(w - LEFT - GAP, h), state, out);
        });
    }
    None
}

/// Every source with how its text goes (a click shows that one), then
/// what can be made for the shown text, or its progress.
fn sources_side(ui: &mut Ui, w: f32, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let l = &state.lyrics;
    ui.set_width(w);
    if let Some(track) = l.track.filter(|_| !l.sources.is_empty()) {
        let n = l.sources.len() + 1;
        let mut picked = None;
        segmented_list(ui, n, |i| {
            if i == 0 {
                let auto = l.source.filter(|_| !l.picked).map(|src| super::name(src).to_owned());
                let item = ListItem::new("Автоматически").selected(!l.picked).id_salt("lyrics-page-auto");
                return match auto {
                    Some(a) => item.supporting(a),
                    None => item,
                };
            }
            let o = &l.sources[i - 1];
            let state = super::source_state(l, o.source);
            let item = ListItem::new(super::name(o.source))
                .selected(l.picked && l.source == Some(o.source))
                .enabled(matches!(o.state, LyricsAvail::Ready(_)))
                .id_salt(("lyrics-page-src", o.source.key()));
            if state.is_empty() { item } else { item.supporting(super::capital(&state)) }
        })
        .into_iter()
        .enumerate()
        .for_each(|(i, r)| {
            if r.response.clicked() {
                picked = Some(i);
            }
        });
        match picked {
            Some(0) if l.picked => out.push(Command::PickLyricsSource(track, None)),
            Some(i) if i > 0 => {
                let src = l.sources[i - 1].source;
                if !(l.picked && l.source == Some(src)) {
                    out.push(Command::PickLyricsSource(track, Some(src)));
                }
            }
            _ => {}
        }
        ui.add_space(GAP);
    }
    if let Some(b) = super::busy(state) {
        super::progress(ui, &b, w);
        return;
    }
    if let Some(e) = super::failed(state) {
        ui.label(TypeRole::BodyMedium.style().rich(e, s.error));
        ui.add_space(12.0);
    } else if let Some(why) = super::no_craft(state) {
        ui.label(TypeRole::BodyMedium.style().rich(why, s.on_surface_variant));
        ui.add_space(12.0);
    }
    let actions = super::actions(state);
    let last = actions.len().saturating_sub(1);
    let save = state.lyrics.savable;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        // Timing made and checked here: keeping it is the primary action.
        if save && ui.add(Button::new("Сохранить текст").style(Style::Filled).size(Size::S).id_salt("lyrics-page-save")).clicked() {
            super::save(state, out);
        }
        // Else the finest timing is.
        for (i, (target, label)) in actions.into_iter().enumerate().rev() {
            let style = if i == last && !save { Style::Filled } else { Style::Tonal };
            if ui.add(Button::new(label).style(style).size(Size::S).id_salt(("lyrics-page-make", label))).clicked() {
                if super::failed(state).is_some() {
                    out.push(Command::ClearCraft);
                }
                super::make(state, target, out);
            }
        }
    });
}

/// The shown text in full, following the song. (The editor goes here.)
fn text_side(ui: &mut Ui, size: egui::Vec2, state: &ViewState, out: &mut Vec<Command>) {
    super::text_card(ui, size, state, out);
}
