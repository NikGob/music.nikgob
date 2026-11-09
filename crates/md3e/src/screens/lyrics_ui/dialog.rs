//! `dialog`: a modal window over the dimmed player that grows out of the
//! control (container transform: its bounds and corners morph on the
//! default spatial spring, the content fades in once it has room). Inside:
//! the sources as a choice, the shown text in full following the song,
//! under it «Подсветка» (how the text is seen and the timing to make, or
//! its progress), and at the foot where the text is kept with «Сохранить
//! текст». The lyrics editor opens in the same kind of window
//! ([`window`]).

use egui::{Align, Area, Context, Id, Key, Layout, Modifiers, Order, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, vec2};
use nikgob_view::{Command, LyricsAvail, ViewState};

use crate::components::{Button, ConnectedGroup, FilterChip, IconButton, Size, Style, anim, paint, paint::Radii};
use crate::tokens::{Elevation, shape};
use crate::{Icon, Theme, TypeRole};

const OPEN: &str = "lyrics-dialog-open";
const FROM: &str = "lyrics-dialog-from";
const MAX_W: f32 = 720.0;
const MAX_H: f32 = 640.0;
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
    let theme = Theme::get(ctx);
    let id = Id::new(key);
    // Both springs start from the control.
    anim::f32(ctx, id.with("k"), 0.0, theme.motion.default_spatial);
    anim::snap_f32(ctx, id.with("k"), 0.0);
    anim::fraction(ctx, id.with("a"), 0.0, theme.motion.default_effects);
    anim::snap_f32(ctx, id.with("a"), 0.0);
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
    window(ctx, OPEN, vec2(MAX_W, MAX_H), true, |ui, inner| body(ui, inner, state, out));
}

/// The window `key` while it is open or still shrinking back: a modal
/// over the dimmed player, as large as `max` allows, that grows out of the
/// control and back; `body` fills its inner rect (usable once it has
/// nearly arrived). A click on the scrim closes it, and Escape when
/// `escape`; true when it was closed so.
pub(super) fn window(ctx: &Context, key: &str, max: Vec2, escape: bool, body: impl FnOnce(&mut Ui, Rect)) -> bool {
    let theme = Theme::get(ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = Id::new(key);
    let open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let k = anim::f32(ctx, id.with("k"), if open { 1.0 } else { 0.0 }, motion.default_spatial);
    let alpha = anim::fraction(ctx, id.with("a"), if open { 1.0 } else { 0.0 }, motion.default_effects).clamp(0.0, 1.0);
    if !open && alpha < 0.002 && k.abs() < 0.002 {
        return false;
    }
    let screen = ctx.content_rect();
    let target = Rect::from_center_size(
        screen.center(),
        vec2((screen.width() - 96.0).clamp(320.0, max.x), (screen.height() - 96.0).clamp(320.0, max.y)),
    );
    // Back into the control where it is now (it may have moved).
    let from = ctx
        .data(|d| d.get_temp::<(Id, Rect)>(Id::new(super::LAST)).map(|(_, r)| r).or_else(|| d.get_temp::<Rect>(Id::new(FROM))))
        .filter(|r| r.is_positive())
        .unwrap_or_else(|| Rect::from_center_size(target.center(), vec2(160.0, 32.0)));
    let kc = k.clamp(0.0, 1.03);
    let rect = Rect::from_min_max(from.min.lerp(target.min, kc), from.max.lerp(target.max, kc));
    let radius = 16.0 + (shape::EXTRA_LARGE - 16.0) * k.clamp(0.0, 1.0);
    // The content appears once the container has grown most of the way.
    let content_a = ((k - 0.45) / 0.55).clamp(0.0, 1.0) * alpha;

    // One layer over everything (Now Playing too): the scrim that dims the
    // player and closes on a click, then the dialog.
    let mut scrim_clicked = false;
    Area::new(id.with("dialog"))
        .fade_in(false)
        .order(Order::Tooltip)
        .fixed_pos(screen.min)
        .constrain(false)
        .interactable(open)
        .show(ctx, |ui| {
            ui.set_clip_rect(screen);
            let (r, scrim) = ui.allocate_exact_size(screen.size(), Sense::click());
            ui.painter().rect_filled(r, 0.0, s.scrim_overlay().gamma_multiply(alpha));
            scrim_clicked = scrim.clicked() && !rect.contains(scrim.interact_pointer_pos().unwrap_or_default());
            let radii = Radii::all(radius).clamped(rect);
            // The control's colour turns into the dialog's.
            let fill = lerp_color(s.secondary_container, s.surface_container_high, k.clamp(0.0, 1.0));
            paint::shadow(ui.painter(), rect, radii, if alpha > 0.5 { Elevation::Level3 } else { Elevation::Level1 });
            paint::container(ui.painter(), rect, radii, fill, Stroke::NONE);
            let inner = target.shrink(24.0);
            let mut cui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)).id_salt("lyrics-dialog-body"));
            cui.set_clip_rect(rect.shrink(2.0).intersect(screen));
            cui.multiply_opacity(content_a);
            // Usable once it has (nearly) arrived; a disabled ui fades what
            // it paints, so only the content is.
            if !(open && k > 0.9) {
                cui.disable();
            }
            body(&mut cui, inner);
        });
    let closed = open && (scrim_clicked || (escape && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))));
    if closed {
        shut(ctx, key);
    }
    closed
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    let m = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8;
    egui::Color32::from_rgba_unmultiplied(m(0), m(1), m(2), m(3))
}

fn body(ui: &mut Ui, inner: Rect, state: &ViewState, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let l = &state.lyrics;
    ui.spacing_mut().item_spacing.y = 0.0;
    // Headline, what is playing, and the way out.
    ui.allocate_ui_with_layout(vec2(inner.width(), 40.0), Layout::left_to_right(Align::Center), |ui| {
        ui.label(TypeRole::HeadlineSmall.style().rich("Текст песни", s.on_surface));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(IconButton::new(Icon::Settings).tooltip("Настройки текста").id_salt("lyrics-dialog-settings")).clicked() {
                close(ui.ctx());
                super::request(ui.ctx(), super::Request::Settings);
            }
        });
    });
    if let Some(p) = super::playing(state) {
        ui.label(TypeRole::BodyMedium.style().rich(p, s.on_surface_variant));
    }
    ui.add_space(16.0);

    // The sources: one choice, applied at once.
    if let Some(track) = l.track.filter(|_| !l.sources.is_empty()) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            if ui.add(FilterChip::new("Автоматически", !l.picked).id_salt("lyrics-dialog-auto")).clicked() && l.picked {
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
    let (head_h, foot_h) = (24.0, 40.0);
    // Room for the progress (48) with one row; rows of buttons 12 apart.
    let timing_h = if rows <= 1 { 56.0 } else { rows as f32 * ROW + (rows - 1) as f32 * 12.0 };
    // 24 of air above the footer under «Подсветка».
    let below = (if timing { 16.0 + head_h + 8.0 + timing_h + 24.0 } else { 16.0 }) + foot_h;
    let text_h = (inner.bottom() - ui.cursor().top() - below).max(120.0);
    super::text_card(ui, vec2(inner.width(), text_h), state, out);
    let save = l.savable && busy.is_none();
    // Keeping the text leads for timing the player made, or when there is
    // nothing else to do; else making timing does.
    let save_lead = save && (super::save_first(l) || actions.is_empty());
    if timing {
        ui.add_space(16.0);
        ui.allocate_ui_with_layout(vec2(inner.width(), head_h), Layout::left_to_right(Align::Center), |ui| {
            ui.label(TypeRole::TitleSmall.style().rich("Подсветка", s.on_surface));
        });
        ui.add_space(8.0);
        let top = ui.cursor().top();
        if let Some(b) = &busy {
            // The progress in the whole height (centred in it).
            ui.allocate_ui_with_layout(vec2(inner.width(), timing_h), Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                super::progress(ui, b, (inner.width() - 160.0).min(360.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(Button::new("Отменить").style(Style::Tonal).size(Size::S).id_salt("lyrics-dialog-cancel")).clicked() {
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
        ui.add_space(24.0);
    } else {
        ui.add_space(16.0);
    }
    ui.allocate_ui_with_layout(vec2(inner.width(), foot_h), Layout::left_to_right(Align::Center), |ui| {
        if let Some(e) = super::failed(state).filter(|_| busy.is_none()) {
            ui.label(TypeRole::BodyMedium.style().rich(e, s.error));
        } else if let Some(why) = super::no_craft(state) {
            ui.label(TypeRole::BodyMedium.style().rich(why, s.on_surface_variant));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new("Готово").style(Style::Text).size(Size::S).id_salt("lyrics-dialog-done")).clicked() {
                close(ui.ctx());
            }
            // Keeping the text: any text not on the computer yet.
            let style = if save_lead { Style::Filled } else { Style::Tonal };
            if save && ui.add(Button::new(super::save_label(state)).style(style).size(Size::S).id_salt("lyrics-dialog-save")).clicked() {
                super::save(state, out);
            }
        });
    });
}
