//! «Межсеть → Устройства»: a segmented list — this computer first, then the
//! paired devices in a steady order — each row the kind's shape (its fill:
//! online; turning, bars inside: music plays there), the name, the music
//! it holds as a number (of its cap with «Избранное»; a nearly full disk as
//! the blocker it is), a thin wave along its bottom edge while files go to
//! it; on the right what it stores, «Гость» and ⋮. This computer's row has
//! its pairing code while it is in no ring. Under the list «Добавить
//! устройство» (a window grown out of the button) and «Карта сети» once
//! there are two devices; a row opens the map at its device. «Рядом»: the
//! unpaired players on the home network, «Подключить» each. In «Подробно»
//! «Оригиналы FLAC на ПК» and «Связь с устройствами».
//!
//! Nothing here says a state in words: shape, colour and motion do.

use std::collections::HashSet;

use egui::{Align, Layout, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_core::i18n;
use nikgob_view::{Command, KeepMasters, MeshKind, MeshPairing, MeshTransfer, MeshView, Route, ViewState};

use super::super::PageState;
use super::super::add_device::{self, AddTab};
use super::super::mesh_ui::{self, Act, Look, MeshUi};
use super::super::settings_help::{self, SettingId as Id};
use super::rows::{dimmed, group, item, row, titled};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Position, Size, Style, Switch, anim, paint, paint::Radii},
    tokens::state,
};

/// What the devices' group keeps between frames (the dialogs and the
/// window are the devices' own, in [`MeshUi`]).
#[derive(Default)]
pub(crate) struct DevicesState {
    /// The switch just flipped (and when), until the runtime shows it.
    enabled_sent: Option<(bool, f64)>,
    /// «Оригиналы FLAC на ПК» just picked, until the runtime shows it.
    keep_sent: Option<(KeepMasters, f64)>,
    /// The devices drawn before: a new one grows in.
    seen: HashSet<String>,
    primed: bool,
}

/// How long a choice shows its new place before the runtime has it.
const SENT_S: f64 = 3.0;
const ROW_1: f32 = 56.0;
const ROW_2: f32 = 72.0;
const AVATAR: f32 = 40.0;

/// A code as it is read out: «042 917».
pub(crate) fn spaced_code(code: &str) -> String {
    if code.len() == 6 { format!("{} {}", &code[..3], &code[3..]) } else { code.to_owned() }
}

pub(super) fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    let m = &state.mesh;
    let now = anim::now(ui.ctx());
    group(ui, i18n::settings_tabs().g_devices);
    let on = enabled_shown(&mut pages.net.devices, m, now);
    let mut nav = None;
    {
        let PageState { net, mesh, .. } = &mut *pages;
        let mem = &mut net.devices;
        let _ = titled(ui, Id::MeshDevices, |ui| {
            dimmed(ui, !on, |ui| {
                if let Some(id) = device_list(ui, state, mem, mesh, out) {
                    mesh.map.select(id);
                    nav = Some(Route::Devices);
                }
                if actions(ui, state, on, mesh, out) {
                    nav = Some(Route::Devices);
                }
            });
        });
        if !m.nearby.is_empty() {
            dimmed(ui, !on, |ui| nearby(ui, m, mesh, out));
        }
        keep_masters_row(ui, m, mem, now, out);
        enabled_row(ui, m, on, mem, now, out);
        mesh_ui::overlays(ui, state, mesh, out);
    }
    if nav.is_some() {
        pages.pending_nav = nav;
    }
}

fn enabled_shown(mem: &mut DevicesState, m: &MeshView, now: f64) -> bool {
    match mem.enabled_sent {
        Some((v, at)) if v != m.enabled && now - at < SENT_S => v,
        _ => {
            mem.enabled_sent = None;
            m.enabled
        }
    }
}

/// The list: this computer, then the paired devices. The device whose row
/// was clicked (`""`: this computer), for the map.
fn device_list(ui: &mut Ui, state: &ViewState, mem: &mut DevicesState, shared: &mut MeshUi, out: &mut Vec<Command>) -> Option<String> {
    let m = &state.mesh;
    let s = Theme::get(ui.ctx()).scheme;
    let map = m.device_count() >= 2;
    let n = m.device_count();
    let mut clicked = None;
    // A device seen for the first time after the list was drawn grows in.
    let fresh: Vec<String> = if mem.primed { m.nodes.iter().filter(|d| !mem.seen.contains(&d.id)).map(|d| d.id.clone()).collect() } else { Vec::new() };
    mem.seen = m.nodes.iter().map(|d| d.id.clone()).collect();
    mem.primed = true;
    let old = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;
    // This computer.
    let this = Spec {
        salt: "this".to_owned(),
        kind: MeshKind::Pc,
        look: mesh_ui::this_look(state),
        name: this_name(m),
        line: mesh_ui::this_storage_line(&state.downloads.storage).map(|t| (t, false)),
        position: Position::of(0, n),
        transfer: None,
        clickable: map,
        fresh: false,
    };
    let mut act = None;
    let r = device_row(ui, &this, |ui| {
        act = mesh_ui::more_menu(ui, "this", true, false);
        if let Some(code) = &m.code {
            code_chip(ui, code, s);
        }
    });
    if let Some(a) = act {
        mesh_ui::act(ui, m, None, a, shared, state, out);
    }
    if r.clicked() {
        clicked = Some(String::new());
    }
    for (i, d) in m.nodes.iter().enumerate() {
        let spec = Spec {
            salt: d.id.clone(),
            kind: d.kind,
            look: mesh_ui::node_look(d),
            name: d.name.clone(),
            line: mesh_ui::storage_line(d),
            position: Position::of(i + 1, n),
            transfer: d.transfer,
            clickable: map,
            fresh: fresh.contains(&d.id),
        };
        let mut act: Option<Act> = None;
        let r = device_row(ui, &spec, |ui| {
            act = mesh_ui::more_menu(ui, &d.id, false, m.ring_holder);
            if let Some(a) = mesh_ui::guest_chip(ui, &d.id, d, m.ring_holder, shared) {
                act = Some(a);
            }
            if let Some(a) = mesh_ui::profile_chip(ui, &d.id, d, true) {
                act = Some(a);
            }
        });
        if let Some(a) = act {
            mesh_ui::act(ui, m, Some(d), a, shared, state, out);
        }
        if r.clicked() {
            clicked = Some(d.id.clone());
        }
    }
    ui.spacing_mut().item_spacing.y = old;
    clicked
}

/// This computer's name: the one set, else the computer's own.
fn this_name(m: &MeshView) -> String {
    let set = m.name_setting.trim();
    if !set.is_empty() {
        set.to_owned()
    } else if !m.this.name.is_empty() {
        m.this.name.clone()
    } else {
        i18n::settings_tabs().this_computer.to_owned()
    }
}

/// The pairing code of this computer while it is in no ring: «042 917» in
/// a pill, the digits tabular.
fn code_chip(ui: &mut Ui, code: &str, s: crate::ColorScheme) {
    egui::Frame::new().fill(s.surface_container_high).corner_radius(egui::CornerRadius::same(16)).inner_margin(egui::Margin::symmetric(12, 4)).show(ui, |ui| {
        ui.label(egui::RichText::new(spaced_code(code)).monospace().size(18.0).color(s.on_surface));
    });
}

/// One row of the list.
struct Spec {
    salt: String,
    kind: MeshKind,
    look: Look,
    name: String,
    /// The second line and whether it is an error.
    line: Option<(String, bool)>,
    position: Position,
    transfer: Option<MeshTransfer>,
    clickable: bool,
    /// Grows in (a device just paired).
    fresh: bool,
}

/// A device's row: 56 high, 72 with a second line (the height moves on a
/// spring), the segmented list's corners and tone; the trailing controls
/// laid out right to left at its end.
fn device_row(ui: &mut Ui, spec: &Spec, trailing: impl FnOnce(&mut Ui)) -> Response {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = ui.id().with(("mesh-row", &spec.salt));
    if spec.fresh {
        anim::fraction_starts_at(&ctx, id.with("in"), 0.0);
    }
    let grown = anim::fraction(&ctx, id.with("in"), 1.0, motion.default_spatial).clamp(0.0, 1.05);
    let target = if spec.line.is_some() { ROW_2 } else { ROW_1 };
    let h = anim::f32(&ctx, id.with("h"), target, motion.default_spatial).max(ROW_1) * grown.min(1.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h.max(0.0)), Sense::hover());
    let response = ui.interact(rect, id, if spec.clickable { Sense::click() } else { Sense::hover() });
    if h < 1.0 || !ui.is_rect_visible(rect) {
        return response;
    }
    let clip = ui.clip_rect().intersect(rect);
    let mut painter = ui.painter().with_clip_rect(clip);
    let radii = spec.position.radii().clamped(rect);
    painter.set_opacity(grown.min(1.0));
    paint::container(&painter, rect, radii, s.surface_container_low, Stroke::NONE);
    let hover = spec.clickable && response.hovered();
    let pressed = spec.clickable && response.is_pointer_button_down_on();
    let layer_target = if pressed { state::PRESSED } else if hover { state::HOVER } else { 0.0 };
    let layer = anim::fraction(&ctx, id.with("layer"), layer_target, motion.fast_effects);
    paint::state_layer(&painter, rect, radii, s.on_surface, layer);
    // The trailing controls first: the text is cut to the room they leave.
    let right = rect.right() - 16.0;
    let area = Rect::from_min_max(pos2(rect.left() + 72.0, rect.top()), pos2(right, rect.bottom()));
    let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)).id_salt(("mesh-row-trailing", &spec.salt)));
    child.set_clip_rect(clip);
    child.multiply_opacity(grown.min(1.0));
    child.spacing_mut().item_spacing.x = 8.0;
    trailing(&mut child);
    let trailing_w = child.min_rect().width();
    let mut avatar_ui = ui.new_child(UiBuilder::new().max_rect(rect));
    avatar_ui.set_clip_rect(clip);
    avatar_ui.multiply_opacity(grown.min(1.0));
    mesh_ui::paint_avatar(&avatar_ui, pos2(rect.left() + 16.0 + AVATAR * 0.5, rect.center().y), AVATAR, &spec.salt, spec.kind, spec.look);
    let x = rect.left() + 16.0 + AVATAR + 16.0;
    let text_w = (right - trailing_w - 12.0 - x).max(8.0);
    let head_color = if spec.look.online { s.on_surface } else { s.on_surface_variant };
    let block = if spec.line.is_some() { 44.0 } else { 24.0 };
    let y = rect.center().y - block * 0.5;
    let head = mesh_ui::text_line(&painter, &spec.name, TypeRole::BodyLarge, head_color, text_w);
    painter.galley(pos2(x, y), head, head_color);
    if let Some((text, error)) = &spec.line {
        let color = if *error { s.error } else { s.on_surface_variant };
        let g = mesh_ui::text_line(&painter, text, TypeRole::BodyMedium, color, text_w);
        painter.galley(pos2(x, y + 24.0), g, color);
    }
    transfer_wave(ui, rect, id, spec.transfer);
    response
}

/// While files go to a device: a 3 dp wave along the row's bottom edge as
/// far as they got, the rest of the way a quiet track; it calms on a pause
/// and fades when the files are there.
fn transfer_wave(ui: &Ui, rect: Rect, id: egui::Id, transfer: Option<MeshTransfer>) {
    let ctx = ui.ctx();
    let theme = Theme::get(ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let last_key = id.with("transfer-last");
    if let Some(t) = transfer {
        ctx.data_mut(|d| d.insert_temp(last_key, t));
    }
    let shown = anim::fraction(ctx, id.with("transfer-in"), if transfer.is_some() { 1.0 } else { 0.0 }, motion.slow_effects).clamp(0.0, 1.0);
    if shown < 0.002 {
        return;
    }
    let Some(t) = transfer.or_else(|| ctx.data(|d| d.get_temp::<MeshTransfer>(last_key))) else { return };
    let frac = if t.total > 0 { (t.done as f64 / t.total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let f = anim::fraction(ctx, id.with("transfer-frac"), frac, motion.default_spatial).clamp(0.0, 1.0);
    let running = !t.paused && transfer.is_some() && !theme.reduced_motion;
    let calm = anim::fraction(ctx, id.with("transfer-calm"), if running { 1.0 } else { 0.0 }, motion.default_effects);
    let phase = mesh_ui::spin(ctx, id.with("transfer-phase"), running, 1.0 / 0.6);
    let (x0, x1, y) = (rect.left() + 12.0, rect.right() - 12.0, rect.bottom() - 3.0);
    let xf = x0 + (x1 - x0) * f;
    let wave = Stroke::new(3.0, s.primary.gamma_multiply(shown));
    crate::components::slider::wave_line(ui, x0 + 1.5, xf.max(x0 + 1.5), y, 2.0 * calm, 32.0, phase, wave);
    if xf + 4.0 + 1.5 < x1 {
        ui.painter().line_segment([pos2(xf + 4.0 + 1.5, y), pos2(x1, y)], Stroke::new(2.0, s.secondary_container.gamma_multiply(shown)));
    }
}

/// «Добавить устройство» and «Карта сети» on one line; under it what keeps
/// a device from being added. True when «Карта сети» was clicked.
fn actions(ui: &mut Ui, state: &ViewState, on: bool, shared: &mut MeshUi, out: &mut Vec<Command>) -> bool {
    let m = &state.mesh;
    let s = Theme::get(ui.ctx()).scheme;
    let st = i18n::settings_tabs();
    let mut map = false;
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let add = Button::new(st.add_device).icon(Icon::Add).style(Style::Tonal).size(Size::S).enabled(m.can_pair()).id_salt("mesh-add-device");
        let r = ui.add(add);
        if r.clicked() {
            add_device::open(ui.ctx(), m, shared, Some(r.rect), AddTab::Connect, out);
        }
        if m.device_count() >= 2 {
            map = ui.add(Button::new(st.network_map).style(Style::Outlined).size(Size::S).id_salt("mesh-open-map")).clicked();
        }
    });
    // What blocks it, in words, and the switch out of it when there is one.
    let failed = match (&m.pairing, shared_add_closed(shared)) {
        (MeshPairing::Failed(why), true) => Some(why.text_in(i18n::locale())),
        _ => None,
    };
    let (text, fix): (Option<String>, Option<(&str, Command)>) = if !m.configurable {
        (Some(st.no_settings_file.to_owned()), None)
    } else if !on {
        (Some(st.mesh_off.to_owned()), Some((st.turn_on, Command::MeshSetEnabled(true))))
    } else if let Some(e) = m.error.as_ref() {
        (Some(e.text_in(i18n::locale())), None)
    } else {
        (failed, None)
    };
    if let Some(text) = text {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let color = if fix.is_some() || !m.configurable { s.on_surface_variant } else { s.error };
            ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(text, color)).wrap());
            if let Some((label, c)) = fix
                && ui.add(Button::new(label).style(Style::Text).size(Size::S).id_salt("mesh-blocker-fix")).clicked()
            {
                out.push(c);
            }
        });
    }
    map
}

/// The window is not up (a failure there is said there).
fn shared_add_closed(shared: &MeshUi) -> bool {
    !add_device::is_open(&shared.add) && !shared.pair.open
}

/// «Рядом»: the unpaired players on the home network, each an outlined
/// row apart from the own devices, «Подключить» opening the window at its
/// code.
fn nearby(ui: &mut Ui, m: &MeshView, shared: &mut MeshUi, out: &mut Vec<Command>) {
    let _ = item(ui, Id::MeshNearby, |ui| {
        let s = Theme::get(ui.ctx()).scheme;
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(TypeRole::LabelLarge.style().rich(Id::MeshNearby.label(), s.on_surface_variant));
            settings_help::help_button(ui, Id::MeshNearby);
        });
        ui.add_space(6.0);
        let can = m.can_pair();
        for d in &m.nearby {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_1), Sense::hover());
            paint::container(ui.painter(), rect, Radii::all(16.0), s.surface, Stroke::new(1.0, s.outline_variant));
            mesh_ui::paint_avatar(ui, pos2(rect.left() + 16.0 + AVATAR * 0.5, rect.center().y), AVATAR, &format!("nearby-{}", d.id), d.kind, Look::default());
            let area = Rect::from_min_max(pos2(rect.left() + 72.0, rect.top()), pos2(rect.right() - 12.0, rect.bottom()));
            let mut child = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)).id_salt(("mesh-nearby", &d.id)));
            let b = child.add(Button::new(i18n::settings_tabs().connect).style(Style::Filled).size(Size::S).enabled(can).id_salt(("mesh-connect", &d.id)));
            if b.clicked() {
                add_device::open_pair(ui.ctx(), m, shared, Some(b.rect), &d.id, out);
            }
            let x = rect.left() + 72.0;
            let g = mesh_ui::text_line(ui.painter(), &d.name, TypeRole::BodyLarge, s.on_surface, (child.min_rect().left() - 12.0 - x).max(8.0));
            ui.painter().galley(pos2(x, rect.center().y - g.size().y * 0.5), g, s.on_surface);
            ui.add_space(8.0);
        }
    });
}

/// «Оригиналы FLAC на ПК» («Подробно»): always, after one copy elsewhere,
/// after two.
fn keep_masters_row(ui: &mut Ui, m: &MeshView, mem: &mut DevicesState, now: f64, out: &mut Vec<Command>) {
    let shown = match mem.keep_sent {
        Some((k, at)) if k != m.keep_masters && now - at < SENT_S => k,
        _ => {
            mem.keep_sent = None;
            m.keep_masters
        }
    };
    row(ui, Id::MeshKeepMasters, |ui| {
        let labels = i18n::settings_tabs::keep_masters_in(i18n::locale());
        let at = KeepMasters::ALL.iter().position(|k| *k == shown).unwrap_or(0);
        if let Some(n) = super::super::connected(ui, egui::Id::new("mesh-keep-masters"), &labels, at, &[]).0
            && n != at
        {
            mem.keep_sent = Some((KeepMasters::ALL[n], now));
            out.push(Command::MeshSetKeepMasters(KeepMasters::ALL[n]));
        }
    });
}

/// «Связь с устройствами» («Подробно»): the mesh on or off.
fn enabled_row(ui: &mut Ui, m: &MeshView, on: bool, mem: &mut DevicesState, now: f64, out: &mut Vec<Command>) {
    row(ui, Id::MeshEnabled, |ui| {
        let mut v = on;
        ui.add(Switch::new(&mut v).enabled(m.configurable).id_salt(("set-switch", Id::MeshEnabled)));
        if v != on {
            mem.enabled_sent = Some((v, now));
            out.push(Command::MeshSetEnabled(v));
        }
    });
}

/// The rows of the devices shown on a node (tests).
#[cfg(test)]
fn row_line(n: &nikgob_view::MeshNodeView) -> Option<(String, bool)> {
    mesh_ui::storage_line(n)
}

#[cfg(test)]
mod tests {
    use nikgob_view::{MeshNearbyView, MeshNodeView, MeshPlaying, MeshProfile, MeshStorage};

    use super::*;

    #[test]
    fn codes_read_in_two_halves() {
        assert_eq!(spaced_code("042917"), "042 917");
        assert_eq!(spaced_code("42"), "42");
    }

    #[test]
    fn this_computer_goes_by_the_name_set_or_its_own() {
        let mut m = MeshView::default();
        assert_eq!(this_name(&m), "Этот компьютер");
        m.this.name = "DESKTOP-1".into();
        assert_eq!(this_name(&m), "DESKTOP-1");
        m.name_setting = "Кабинет".into();
        assert_eq!(this_name(&m), "Кабинет");
    }

    fn view() -> ViewState {
        let mut state = ViewState::default();
        state.mesh.enabled = true;
        state.mesh.running = true;
        state.mesh.configurable = true;
        state.mesh.ring_holder = true;
        state.mesh.paired = true;
        state.mesh.this = MeshNodeView { id: "me".into(), name: "PC".into(), ..MeshNodeView::default() };
        state.mesh.nodes = vec![
            MeshNodeView {
                id: "pi".into(),
                name: "Raspberry Pi".into(),
                kind: MeshKind::Pi,
                online: true,
                storage: Some(MeshStorage { used: 100 << 30, free: 400 << 30 }),
                playing: Some(MeshPlaying { title: "Song".into(), ..MeshPlaying::default() }),
                transfer: Some(MeshTransfer { done: 1, total: 4, paused: false, speed: 0 }),
                ..MeshNodeView::default()
            },
            MeshNodeView { id: "ph".into(), name: "Телефон".into(), kind: MeshKind::Phone, profile: MeshProfile::Favorites { cap_bytes: 16 << 30 }, ..MeshNodeView::default() },
        ];
        state.mesh.nearby = vec![MeshNearbyView { id: "tv".into(), name: "TV".into(), kind: MeshKind::Tv, addr: None }];
        state
    }

    fn frame(ctx: &egui::Context, t: f64, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(900.0, 900.0))),
            time: Some(t),
            ..Default::default()
        };
        let mut done = ctx.run_ui(input, |ui| {
            super::super::rows::set_mode(ui.ctx(), super::super::rows::Mode { deep: true, dev: false, opacity: 1.0, stage: false });
            show(ui, state, pages, out);
        });
        done.textures_delta.clear();
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        ctx
    }

    /// The group draws its rows (the devices, those nearby, the originals'
    /// choice, the switch) and sends nothing by drawing.
    #[test]
    fn the_group_draws_its_rows() {
        let ctx = context();
        let state = view();
        let mut pages = PageState::default();
        let mut out = Vec::new();
        for i in 0..3 {
            frame(&ctx, f64::from(i) * 0.1, &state, &mut pages, &mut out);
        }
        let drawn: Vec<Id> = settings_help::drawn_rows(&ctx).1.into_iter().map(|r| r.id).collect();
        for id in [Id::MeshDevices, Id::MeshNearby, Id::MeshKeepMasters, Id::MeshEnabled] {
            assert!(drawn.contains(&id), "{id:?}: {drawn:?}");
        }
        assert!(out.is_empty(), "nothing is sent by drawing: {out:?}");
        assert_eq!(row_line(&state.mesh.nodes[0]), Some(("100 ГБ".to_owned(), false)));
    }

    /// A device paired while the list is up grows in; the ones there at
    /// the first frame do not.
    #[test]
    fn a_new_device_grows_in() {
        let ctx = context();
        let mut state = view();
        let mut pages = PageState::default();
        let mut out = Vec::new();
        frame(&ctx, 0.0, &state, &mut pages, &mut out);
        assert!(pages.net.devices.primed);
        assert!(pages.net.devices.seen.contains("pi"));
        state.mesh.nodes.push(MeshNodeView { id: "tv".into(), name: "TV".into(), kind: MeshKind::Tv, ..MeshNodeView::default() });
        frame(&ctx, 0.1, &state, &mut pages, &mut out);
        assert!(pages.net.devices.seen.contains("tv"));
    }
}
