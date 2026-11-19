//! `dialog`: a modal window over the dimmed player that grows out of the
//! control (container transform: its bounds and corners morph on the
//! default spatial spring, the content fades in once it has room). Inside:
//! the sources as a choice, the shown text in full following the song,
//! under it how the text is seen and the timing to make (or its
//! progress), and at the foot where the text is kept with «Сохранить
//! текст». The lyrics editor opens in the same kind of window
//! ([`window`]).

use egui::{Align, Context, Id, Layout, Rect, Ui, UiBuilder, Vec2, pos2, vec2};
use nikgob_view::{Command, LyricsAvail, ViewState};

use crate::components::{Button, ConnectedGroup, FilterChip, IconButton, ModalWindow, Size, Style, paint, paint::Radii};
use crate::{Icon, Theme, TypeRole};

const OPEN: &str = "lyrics-dialog-open";
const FROM: &str = "lyrics-dialog-from";
const MAX_W: f32 = 720.0;
const MAX_H: f32 = 640.0;
/// The head: the title, the settings and the close button…
const HEAD_H: f32 = 40.0;
/// …and the room between it and the card under it.
const HEAD_GAP: f32 = 12.0;
/// The card (as the «Скорость | Эквалайзер» window's): its corners and
/// the room at its edge.
const CARD_RADIUS: f32 = 24.0;
const CARD_PAD: f32 = 20.0;
/// A row of buttons under «Подсветка».
const ROW: f32 = 40.0;
/// The least room between the choice and making timing on one row.
const BESIDE_GAP: f32 = 24.0;

/// Opens the dialog growing out of `from` (the control).
pub(super) fn open(ctx: &Context, from: Rect) {
    grow(ctx, OPEN, from);
}

/// Opens the window `key` ([`window`]) growing out of `from` (the control).
pub(super) fn grow(ctx: &Context, key: &str, from: Rect) {
    let id = Id::new(key);
    // Both springs start from the control.
    ModalWindow::rewind(ctx, id);
    ctx.data_mut(|d| {
        d.insert_temp(id, true);
        if from.is_positive() {
            d.insert_temp(Id::new(FROM), from);
        }
    });
}

pub(super) fn close(ctx: &Context) {
    shut(ctx, OPEN);
}

/// Closes the window `key` (it shrinks back into the control).
pub(super) fn shut(ctx: &Context, key: &str) {
    ctx.data_mut(|d| d.insert_temp(Id::new(key), false));
}

/// Draws the dialog while it is open or still shrinking back.
pub(crate) fn show(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    if super::variant(&state.prefs) != super::Variant::Dialog {
        return;
    }
    window(ctx, OPEN, vec2(MAX_W, MAX_H + 40.0), true, true, |ui, inner| body(ui, inner, state, out));
}

/// The window `key` while it is open or still shrinking back: a modal
/// over the dimmed player, as large as `max` allows, that grows out of the
/// control and back; `body` fills its inner rect (usable once it has
/// nearly arrived). A `shell` holds cards (as «Скорость | Эквалайзер»).
/// A click on the scrim closes it, and Escape when `escape`; true when it
/// was closed so.
pub(super) fn window(ctx: &Context, key: &str, max: Vec2, escape: bool, shell: bool, body: impl FnOnce(&mut Ui, Rect)) -> bool {
    let id = Id::new(key);
    let open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let window = ModalWindow::new(id, max).escape(escape);
    let window = if shell { window.shell() } else { window };
    // Back into the control where it is now (it may have moved).
    let from = ctx
        .data(|d| d.get_temp::<(Id, Rect)>(Id::new(super::LAST)).map(|(_, r)| r).or_else(|| d.get_temp::<Rect>(Id::new(FROM))))
        .filter(|r| r.is_positive())
        .unwrap_or_else(|| Rect::from_center_size(window.target(ctx).center(), vec2(160.0, 32.0)));
    let closed = window.origin(from).show(ctx, open, body);
    if closed {
        shut(ctx, key);
    }
    closed
}

fn body(ui: &mut Ui, inner: Rect, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.spacing_mut().item_spacing.y = 0.0;
    // The head: the title in line with the card's content, the settings and
    // the way out at its end (the song is the one in front: no line repeats
    // it).
    let (row, _) = ui.allocate_exact_size(vec2(inner.width(), HEAD_H), egui::Sense::hover());
    let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)).id_salt("lyrics-dialog-head-right"));
    right.spacing_mut().item_spacing.x = 4.0;
    if right.add(IconButton::new(Icon::Close).size(Size::S).style(Style::Standard).tooltip(nikgob_core::i18n::screens().close_esc).id_salt("lyrics-dialog-close")).clicked() {
        close(right.ctx());
    }
    if right.add(IconButton::new(Icon::Settings).size(Size::S).style(Style::Standard).tooltip(nikgob_core::i18n::lyrics().settings).id_salt("lyrics-dialog-settings")).clicked() {
        close(right.ctx());
        super::request(right.ctx(), super::Request::Settings);
    }
    let left = Rect::from_min_max(row.min + vec2(CARD_PAD, 0.0), pos2((right.min_rect().left() - 12.0).max(row.left()), row.bottom()));
    let mut left = ui.new_child(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)).id_salt("lyrics-dialog-head-left"));
    left.label(TypeRole::TitleLarge.style().rich(nikgob_core::i18n::lyrics().title, s.on_surface));
    ui.add_space(HEAD_GAP);
    // The card: the sources, the text, its timing and keeping it.
    let card = Rect::from_min_max(ui.cursor().min, pos2(inner.right(), inner.bottom().max(ui.cursor().min.y)));
    paint::container(ui.painter(), card, Radii::all(CARD_RADIUS), s.surface_container_highest, egui::Stroke::NONE);
    let room = card.shrink(CARD_PAD);
    let mut c = ui.new_child(UiBuilder::new().max_rect(room).layout(Layout::top_down(Align::Min)).id_salt("lyrics-dialog-card"));
    c.set_clip_rect(card.intersect(ui.clip_rect()));
    on_card(&mut c, room, state, out);
}

fn on_card(ui: &mut Ui, inner: Rect, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let l = &state.lyrics;
    ui.spacing_mut().item_spacing.y = 0.0;

    // The sources: one choice, applied at once.
    if let Some(track) = l.track.filter(|_| !l.sources.is_empty()) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            if ui.add(FilterChip::new(nikgob_core::i18n::lyrics().auto, !l.picked).id_salt("lyrics-dialog-auto")).clicked() && l.picked {
                out.push(Command::PickLyricsSource(track, None));
            }
            for o in &l.sources {
                let on = l.picked && l.source == Some(o.source);
                let chip = FilterChip::new(super::source_name(l, o.source), on)
                    .enabled(matches!(o.state, LyricsAvail::Ready(_)))
                    .id_salt(("lyrics-dialog-src", o.source.key()));
                if ui.add(chip).clicked() && !on {
                    out.push(Command::PickLyricsSource(track, Some(o.source)));
                }
            }
        });
        // The checked chip says where the text is from; no caption repeats it.
        ui.add_space(16.0);
    }
    // The text, then the timing (how it is seen and made, or its
    // progress), then where it is kept and the way out.
    let busy = super::busy(state);
    let choice = !super::shows(state).is_empty();
    let actions = if busy.is_some() { Vec::new() } else { super::actions(state) };
    let timing = busy.is_some() || choice || !actions.is_empty();
    // Under «Подсветка», left-aligned, each on its row: how the text is
    // seen, then the timing to make (one row more where it does not fit
    // the width). Measured on what is offered also while timing is made,
    // so the text card keeps its height when a button is pressed.
    let offered = super::actions(state);
    // Making timing goes on the choice's row, at its right end, where both
    // fit with room between them.
    let choice_w = super::all_shows(state).iter().map(|a| Button::new(a.1).size(Size::S).natural_width(ui)).sum::<f32>() + 2.0 * 2.0;
    let beside = choice && !offered.is_empty() && choice_w + BESIDE_GAP + super::make_width(ui, &offered) <= inner.width();
    let make_rows = match offered.len() {
        0 => 0,
        _ if beside => 0,
        n if n > 1 && super::make_width(ui, &offered) > inner.width() => n,
        _ => 1,
    };
    let rows = usize::from(choice) + make_rows;
    let save = l.savable && busy.is_none();
    let reason = super::failed(state).filter(|_| busy.is_none()).map(|e| (e, true)).or_else(|| super::no_craft(state).map(|w| (w, false)));
    // The foot only for keeping the text or a reason (the head closes it).
    let foot = save || reason.is_some();
    let foot_h = if foot { 40.0 } else { 0.0 };
    // Room for the progress (48) with one row; rows of buttons 12 apart.
    let timing_h = if rows <= 1 { 56.0 } else { rows as f32 * ROW + (rows - 1) as f32 * 12.0 };
    // 24 of air above the footer under the timing (no heading: its
    // buttons say what it is).
    let air = if foot { 24.0 } else { 0.0 };
    let below = if timing { 16.0 + timing_h + air + foot_h } else if foot { 16.0 + foot_h } else { 0.0 };
    let text_h = (inner.bottom() - ui.cursor().top() - below).max(120.0);
    super::text_card(ui, vec2(inner.width(), text_h), state, out);
    // Keeping the text leads for timing the player made, or when there is
    // nothing else to do; else making timing does.
    let save_lead = save && (super::save_first(l) || actions.is_empty());
    if timing {
        ui.add_space(16.0);
        let top = ui.cursor().top();
        if let Some(b) = &busy {
            // The progress in the whole height (centred in it).
            ui.allocate_ui_with_layout(vec2(inner.width(), timing_h), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                super::progress(ui, b, (inner.width() - 160.0).min(360.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(Button::new(nikgob_core::i18n::common().cancel).style(Style::Tonal).size(Size::S).id_salt("lyrics-dialog-cancel")).clicked() {
                        out.push(Command::CancelCraft);
                    }
                });
            });
        } else {
            // How the text is seen, where it has timing to choose from.
            let mut show = |ui: &mut Ui| {
                let all = super::all_shows(state);
                let labels = all.map(|a| a.1);
                let on = all.map(|a| a.2);
                let at = all.iter().position(|a| a.0 == super::seen(l)).unwrap_or(2);
                if let Some(i) = ConnectedGroup::new("lyrics-dialog-show", &labels).enabled(&on).show(ui, at) {
                    super::show_as(state, all[i].0, out);
                }
            };
            if beside {
                // One row: the choice on the left, making timing at the
                // right end, over the footer's buttons.
                let mut make = Vec::new();
                ui.allocate_ui_with_layout(vec2(inner.width(), ROW), Layout::left_to_right(Align::Center), |ui| {
                    ui.allocate_ui_with_layout(vec2(choice_w, ROW), Layout::left_to_right(Align::Center), &mut show);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        super::make_row(ui, state, "lyrics-dialog", !save_lead, false, &mut make);
                    });
                });
                out.append(&mut make);
            } else if choice {
                show(ui);
            }
            // The timing to make under it: the finest is the primary action
            // unless there is timing to keep; its deep variant is in the
            // split button's menu.
            if !actions.is_empty() && !beside {
                if choice {
                    ui.add_space(12.0);
                }
                super::make_row(ui, state, "lyrics-dialog", !save_lead, make_rows > 1, out);
            }
        }
        // The same height however it is filled.
        let used = ui.cursor().top() - top;
        if used < timing_h {
            ui.add_space(timing_h - used);
        }
        ui.add_space(air);
    } else if foot {
        ui.add_space(16.0);
    }
    if !foot {
        return;
    }
    ui.allocate_ui_with_layout(vec2(inner.width(), foot_h), Layout::left_to_right(Align::Center), |ui| {
        if let Some((why, error)) = reason {
            ui.label(TypeRole::BodyMedium.style().rich(why, if error { s.error } else { s.on_surface_variant }));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // Keeping the text: any text not on the computer yet.
            let style = if save_lead { Style::Filled } else { Style::Tonal };
            if save && ui.add(Button::new(super::save_label(state)).style(style).size(Size::S).id_salt("lyrics-dialog-save")).clicked() {
                super::save(state, out);
            }
        });
    });
}
