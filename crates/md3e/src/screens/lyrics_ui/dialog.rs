//! `dialog`: a modal window over the dimmed player that grows out of the
//! control (container transform: its bounds and corners morph on the
//! default spatial spring, the content fades in once it has room). Inside:
//! the sources as a choice, the shown text in full following the song,
//! and one primary action to make the timing, with its progress.

use egui::{Align, Area, Context, Id, Key, Layout, Modifiers, Order, Rect, Sense, Stroke, Ui, UiBuilder, vec2};
use nikgob_view::{Command, LyricsAvail, ViewState};

use crate::components::{Button, FilterChip, IconButton, Size, Style, anim, paint, paint::Radii};
use crate::tokens::{Elevation, shape};
use crate::{Icon, Theme, TypeRole};

const OPEN: &str = "lyrics-dialog-open";
const FROM: &str = "lyrics-dialog-from";
const MAX_W: f32 = 720.0;
const MAX_H: f32 = 640.0;

/// Opens the dialog growing out of `from` (the control).
pub(super) fn open(ctx: &Context, from: Rect) {
    let theme = Theme::get(ctx);
    let id = Id::new(OPEN);
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

fn close(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(Id::new(OPEN), false));
}

/// Draws the dialog while it is open or still shrinking back.
pub(crate) fn show(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    if super::variant(&state.prefs) != super::Variant::Dialog {
        return;
    }
    let theme = Theme::get(ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = Id::new(OPEN);
    let open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let k = anim::f32(ctx, id.with("k"), if open { 1.0 } else { 0.0 }, motion.default_spatial);
    let alpha = anim::fraction(ctx, id.with("a"), if open { 1.0 } else { 0.0 }, motion.default_effects).clamp(0.0, 1.0);
    if !open && alpha < 0.002 && k.abs() < 0.002 {
        return;
    }
    let screen = ctx.content_rect();
    let target = Rect::from_center_size(
        screen.center(),
        vec2((screen.width() - 96.0).clamp(320.0, MAX_W), (screen.height() - 96.0).clamp(320.0, MAX_H)),
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
            body(&mut cui, inner, state, out);
        });
    if open && (scrim_clicked || ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))) {
        close(ctx);
    }
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
        ui.add_space(8.0);
        // What the chosen text is, in the control's own words.
        let mut line = super::chip_label(l);
        if let Some(state) = l.source.map(|src| super::source_state(l, src)).filter(|s| !s.is_empty()) {
            line = format!("{line} · {state}");
        }
        ui.label(TypeRole::BodySmall.style().rich(line, s.on_surface_variant));
        ui.add_space(12.0);
    }

    // The text, and under it the action or its progress.
    let actions_h = 56.0;
    let text_h = (inner.bottom() - ui.cursor().top() - actions_h - 16.0).max(120.0);
    super::text_card(ui, vec2(inner.width(), text_h), state, out);
    ui.add_space(16.0);
    ui.allocate_ui_with_layout(vec2(inner.width(), actions_h), Layout::left_to_right(Align::Center), |ui| {
        if let Some(b) = super::busy(state) {
            super::progress(ui, &b, (inner.width() - 160.0).min(360.0));
        } else if let Some(e) = super::failed(state) {
            ui.label(TypeRole::BodyMedium.style().rich(e, s.error));
        } else if let Some(why) = super::no_craft(state) {
            ui.label(TypeRole::BodyMedium.style().rich(why, s.on_surface_variant));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui.add(Button::new("Готово").style(Style::Text).size(Size::S).id_salt("lyrics-dialog-done")).clicked() {
                close(ui.ctx());
            }
            if super::busy(state).is_none() {
                // Timing made and checked here: keeping it is the primary
                // action, making it again goes beside it.
                let save = l.savable;
                if save && ui.add(Button::new(super::save_label(state)).style(Style::Filled).size(Size::S).id_salt("lyrics-dialog-save")).clicked() {
                    super::save(state, out);
                }
                // Else the primary action is the finest timing; lines beside it.
                for (i, (target, label)) in super::actions(state).into_iter().rev().enumerate() {
                    let style = if i == 0 && !save { Style::Filled } else { Style::Tonal };
                    let b = Button::new(label).style(style).size(Size::S).id_salt(("lyrics-dialog-make", label));
                    if ui.add(b).clicked() {
                        if super::failed(state).is_some() {
                            out.push(Command::ClearCraft);
                        }
                        super::make(state, target, out);
                    }
                }
            }
        });
    });
}
