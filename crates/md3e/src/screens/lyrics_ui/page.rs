//! `page`: «Текст песни», a page of its own like Настройки (it opens with
//! the forward page transition and goes back the same way). The sources
//! on the left with how each is timed, under them «Подсветка» (how the
//! text is seen and the timing to make) and where the text is kept with
//! «Сохранить текст»; the shown text on the right, following the song.
//!
//! The right side is where the lyrics editor will go (the text and its
//! times by hand, saved as a lyrics file next to the track): `text_side`
//! is its slot.

use egui::{Align, Layout, Ui, vec2};
use nikgob_view::{Command, LyricsAvail, Route, ViewState};

use crate::components::{Button, ConnectedGroup, ListItem, Size, Style, segmented_list};
use crate::{Icon, Theme, TypeRole};

/// Wide enough for «По словам | По строкам | Без подсветки» in one row.
const LEFT: f32 = 360.0;
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

/// Every source with how its text is timed (a click shows that one), then
/// how the shown text is seen and what can be made for it, or its
/// progress, then where it is kept and keeping it.
fn sources_side(ui: &mut Ui, w: f32, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let l = &state.lyrics;
    ui.set_width(w);
    if let Some(track) = l.track.filter(|_| !l.sources.is_empty()) {
        let n = l.sources.len() + 1;
        let mut picked = None;
        segmented_list(ui, n, |i| {
            if i == 0 {
                let auto = l.source.filter(|_| !l.picked).map(|src| super::source_name(l, src).to_owned());
                let item = ListItem::new("Автоматически").selected(!l.picked).id_salt("lyrics-page-auto");
                return match auto {
                    Some(a) => item.supporting(a),
                    None => item,
                };
            }
            let o = &l.sources[i - 1];
            let state = super::source_state(l, o.source);
            let item = ListItem::new(super::source_name(l, o.source))
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
    // «Подсветка»: how the text is seen and the timing to make, or its
    // progress.
    let busy = super::busy(state);
    let choice = !super::shows(state).is_empty();
    let actions = if busy.is_some() { Vec::new() } else { super::actions(state) };
    let save = l.savable && busy.is_none();
    // Keeping the text leads for timing the player made, or when there is
    // nothing else to do; else making timing does.
    let save_lead = save && (super::save_first(l) || actions.is_empty());
    let why = super::failed(state)
        .filter(|_| busy.is_none())
        .map(|e| (e, s.error))
        .or_else(|| super::no_craft(state).map(|w| (w, s.on_surface_variant)));
    if busy.is_some() || choice || !actions.is_empty() || why.is_some() {
        ui.label(TypeRole::TitleSmall.style().rich("Подсветка", s.on_surface));
        ui.add_space(12.0);
    }
    if let Some(b) = &busy {
        super::progress(ui, b, w);
        ui.add_space(12.0);
        if ui.add(Button::new("Отменить").style(Style::Tonal).size(Size::S).id_salt("lyrics-page-cancel")).clicked() {
            out.push(Command::CancelCraft);
        }
        return;
    }
    if choice {
        let all = super::all_shows(state);
        let labels = all.map(|a| a.1);
        let on = all.map(|a| a.2);
        let at = all.iter().position(|a| a.0 == super::seen(l)).unwrap_or(2);
        if let Some(i) = ConnectedGroup::new("lyrics-page-show", &labels).enabled(&on).show(ui, at) {
            super::show_as(state, all[i].0, out);
        }
        ui.add_space(12.0);
    }
    if let Some((text, color)) = why {
        ui.label(TypeRole::BodyMedium.style().rich(text, color));
        ui.add_space(12.0);
    }
    if !actions.is_empty() {
        // The finest timing is the primary action unless there is timing
        // to keep; its deep variant is in the split button's menu.
        let stacked = super::make_width(ui, &actions) > w;
        super::make_row(ui, state, "lyrics-page", !save_lead, stacked, out);
    }
    // Where the text is kept, where that matters, and keeping it (any text
    // not on the computer yet).
    let home = super::home(l);
    if home.is_some() || save {
        ui.add_space(GAP);
    }
    if let Some(home) = home {
        ui.label(TypeRole::BodyMedium.style().rich(home, s.on_surface_variant));
        ui.add_space(12.0);
    }
    let style = if save_lead { Style::Filled } else { Style::Tonal };
    if save && ui.add(Button::new(super::save_label(state)).style(style).size(Size::S).id_salt("lyrics-page-save")).clicked() {
        super::save(state, out);
    }
}

/// The shown text in full, following the song. (The editor goes here.)
fn text_side(ui: &mut Ui, size: egui::Vec2, state: &ViewState, out: &mut Vec<Command>) {
    super::text_card(ui, size, state, out);
}
