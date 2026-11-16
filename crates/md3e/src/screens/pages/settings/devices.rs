//! «Межсеть → Устройства»: this device (its name; its 6-digit code while
//! it is in no ring), the paired devices (the kind in a circle whose
//! colour says whether it is online, ⋮ «Переименовать», «Забыть»), the
//! unpaired ones nearby («Подключить» → a dialog for their code that pairs
//! and closes itself), and «Добавить устройство» (a one-time ticket with
//! «Копировать» and the command for Linux).
//!
//! Small and plain until the devices' own design: no captions, errors only.

use egui::{Rect, Ui, pos2, vec2};
use nikgob_view::{Command, MeshKind, MeshPairing, MeshView, ViewState};

use super::super::settings_help::SettingId as Id;
use super::net::code_block;
use super::rows::{Note, dimmed, group, row, row_note, titled};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, Dialog, DialogAction, IconButton, Leading, ListItem, LoadingIndicator, Menu, MenuItem, Position, Size, Style, Switch, TextField,
        anim,
    },
};

/// What the devices' group keeps between frames.
#[derive(Default)]
pub(crate) struct DevicesState {
    /// What is typed into this device's name, and the setting it came from.
    name_draft: String,
    name_from: Option<String>,
    name_focus: bool,
    /// The switch just flipped (and when), until the runtime shows it.
    enabled_sent: Option<(bool, f64)>,
    code: Option<CodeDialog>,
    invite: Option<InviteDialog>,
    rename: Option<RenameDialog>,
}

/// «Подключить»: the code of a nearby device.
struct CodeDialog {
    id: String,
    name: String,
    code: String,
    /// The code sent, while its pairing runs.
    sent: Option<String>,
    focus: bool,
    error: Option<String>,
}

/// «Добавить устройство»: the ticket, once the mesh made it.
struct InviteDialog {
    at: f64,
}

/// «Переименовать» of a paired device.
struct RenameDialog {
    id: String,
    name: String,
    focus: bool,
}

/// How long the switch shows its new place before the runtime has it.
const SENT_S: f64 = 3.0;
/// How long «Добавить устройство» waits for the ticket.
const INVITE_WAIT_S: f64 = 5.0;

/// The icon of a kind of device.
pub(crate) fn kind_icon(k: MeshKind) -> Icon {
    match k {
        MeshKind::Pc => Icon::Computer,
        MeshKind::Pi => Icon::DeveloperBoard,
        MeshKind::Phone => Icon::Smartphone,
        MeshKind::Tv => Icon::Tv,
        MeshKind::Server => Icon::Dns,
    }
}

/// Only the digits of a code typed or pasted, at most six.
pub(crate) fn code_digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).take(6).collect()
}

/// The command that pairs a Linux device with `ticket`.
pub(crate) fn linux_command(ticket: &str) -> String {
    format!("nikgobd pair {ticket}")
}

/// A code as it is read out: «042 917».
pub(crate) fn spaced_code(code: &str) -> String {
    if code.len() == 6 { format!("{} {}", &code[..3], &code[3..]) } else { code.to_owned() }
}

pub(super) fn show(ui: &mut Ui, state: &ViewState, mem: &mut DevicesState, out: &mut Vec<Command>) {
    let m = &state.mesh;
    let now = anim::now(ui.ctx());
    group(ui, "Устройства");
    this_row(ui, m, mem, out);
    let on = enabled_shown(mem, m, now);
    enabled_row(ui, m, on, mem, now, out);
    dimmed(ui, !on, |ui| {
        devices_row(ui, m, mem, now, out);
        device_list(ui, m, mem, out);
        nearby(ui, m, mem, out);
    });
    code_dialog(ui, m, mem, out);
    invite_dialog(ui, m, mem, now, out);
    rename_dialog(ui, mem, out);
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

/// «Это устройство»: its name (empty: the computer's), its code while it
/// is in no ring.
fn this_row(ui: &mut Ui, m: &MeshView, mem: &mut DevicesState, out: &mut Vec<Command>) {
    if mem.name_from.as_deref() != Some(m.name_setting.as_str()) && !mem.name_focus {
        mem.name_draft = m.name_setting.clone();
        mem.name_from = Some(m.name_setting.clone());
    }
    let note = if !m.configurable {
        Some(Note::Blocker("Нет файла настроек"))
    } else {
        m.error.as_deref().map(Note::Error)
    };
    let s = Theme::get(ui.ctx()).scheme;
    row_note(ui, Id::MeshThis, note, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let placeholder = if m.this.name.is_empty() { "Компьютер".to_owned() } else { m.this.name.clone() };
            let f = TextField::new(&mut mem.name_draft, "")
                .placeholder(placeholder)
                .width(220.0)
                .enabled(m.configurable)
                .id_salt("mesh-this-name")
                .show(ui);
            mem.name_focus = f.response.has_focus();
            if (f.submitted || f.response.lost_focus()) && mem.name_draft.trim() != m.name_setting.trim() {
                out.push(Command::MeshRename { node_id: None, name: mem.name_draft.trim().to_owned() });
            }
            if let Some(code) = &m.code {
                egui::Frame::new()
                    .fill(s.surface_container_high)
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(12, 6))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(spaced_code(code)).monospace().size(20.0).color(s.on_surface));
                    });
            }
        });
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

/// «Мои устройства»: «Добавить устройство»; a pairing that failed out of
/// any dialog says why under the name.
fn devices_row(ui: &mut Ui, m: &MeshView, mem: &mut DevicesState, now: f64, out: &mut Vec<Command>) {
    let failed = match (&m.pairing, mem.code.is_none() && mem.invite.is_none()) {
        (MeshPairing::Failed(why), true) => Some(why.as_str()),
        _ => None,
    };
    row_note(ui, Id::MeshDevices, failed.map(Note::Error), |ui| {
        let add = Button::new("Добавить устройство")
            .icon(Icon::Add)
            .style(Style::Tonal)
            .size(Size::S)
            .enabled(m.can_pair())
            .id_salt("mesh-add-device");
        if ui.add(add).clicked() {
            mem.invite = Some(InviteDialog { at: now });
            out.push(Command::MeshClearPairing);
            out.push(Command::MeshCreateInvite);
        }
    });
}

/// A device's kind in a circle: `primary-container` while it is online,
/// `surface-container-highest` while not (the colour eases between them).
fn paint_kind(ui: &Ui, rect: Rect, salt: &str, kind: MeshKind, online: bool) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let (fill, on) = if online { (s.primary_container, s.on_primary_container) } else { (s.surface_container_highest, s.on_surface_variant) };
    let fill = anim::color(&ctx, egui::Id::new(("mesh-kind-fill", salt)), fill, motion.default_effects);
    let on = anim::color(&ctx, egui::Id::new(("mesh-kind-on", salt)), on, motion.default_effects);
    let c = pos2(rect.left() + 36.0, rect.center().y);
    ui.painter().circle_filled(c, 20.0, fill);
    kind_icon(kind).widget(online, on, 24.0).paint_at(ui, Rect::from_center_size(c, vec2(24.0, 24.0)));
}

/// The paired devices, a segmented list (nothing while there are none).
fn device_list(ui: &mut Ui, m: &MeshView, mem: &mut DevicesState, out: &mut Vec<Command>) {
    if m.nodes.is_empty() || !super::rows::shown(ui.ctx(), Id::MeshDevices) {
        return;
    }
    ui.add_space(4.0);
    let old = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;
    let n = m.nodes.len();
    for (i, d) in m.nodes.iter().enumerate() {
        let mut picked: Option<usize> = None;
        let can_forget = m.ring_holder;
        let output = ListItem::new(d.name.as_str())
            .leading(Leading::Avatar(None, kind_icon(d.kind)))
            .position(Position::of(i, n))
            .id_salt(("mesh-device", &d.id))
            .trailing(|ui| {
                let b = ui.add(IconButton::new(Icon::MoreVert).size(Size::S).tooltip("Ещё").id_salt(("mesh-device-more", &d.id)));
                picked = Menu::new(("mesh-device-menu", &d.id))
                    .align_right(true)
                    .item(MenuItem::new("Переименовать").icon(Icon::Edit))
                    .item(MenuItem::new("Забыть").icon(Icon::Delete).enabled(can_forget))
                    .attach(&b);
            })
            .show(ui);
        paint_kind(ui, output.rect, &d.id, d.kind, d.online);
        match picked {
            Some(0) => mem.rename = Some(RenameDialog { id: d.id.clone(), name: d.name.clone(), focus: true }),
            Some(1) if can_forget => out.push(Command::MeshForget { node_id: d.id.clone() }),
            _ => {}
        }
    }
    ui.spacing_mut().item_spacing.y = old;
}

/// «Рядом»: the unpaired instances on the home network (only while there
/// are some), each with «Подключить».
fn nearby(ui: &mut Ui, m: &MeshView, mem: &mut DevicesState, out: &mut Vec<Command>) {
    if m.nearby.is_empty() {
        return;
    }
    let can = m.can_pair();
    let _ = titled(ui, Id::MeshNearby, |ui| {
        let old = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 2.0;
        let n = m.nearby.len();
        for (i, d) in m.nearby.iter().enumerate() {
            let mut connect = false;
            let output = ListItem::new(d.name.as_str())
                .leading(Leading::Avatar(None, kind_icon(d.kind)))
                .position(Position::of(i, n))
                .id_salt(("mesh-nearby", &d.id))
                .trailing(|ui| {
                    connect = ui
                        .add(Button::new("Подключить").style(Style::Text).size(Size::S).enabled(can).id_salt(("mesh-connect", &d.id)))
                        .clicked();
                })
                .show(ui);
            paint_kind(ui, output.rect, &d.id, d.kind, false);
            if connect {
                mem.code = Some(CodeDialog { id: d.id.clone(), name: d.name.clone(), code: String::new(), sent: None, focus: true, error: None });
                out.push(Command::MeshClearPairing);
            }
        }
        ui.spacing_mut().item_spacing.y = old;
    });
}

/// What the code dialog does with the pairing's state: whether it is done
/// (close it), and an error to show.
pub(crate) fn code_outcome(pairing: &MeshPairing, id: &str, sent: bool) -> (bool, Option<String>) {
    if !sent {
        return (false, None);
    }
    match pairing {
        MeshPairing::Done { node_id } if node_id == id => (true, None),
        MeshPairing::Failed(why) => (false, Some(why.clone())),
        _ => (false, None),
    }
}

/// «Подключить»: six digits; the last one sends them; the device moves to
/// the list and the dialog closes itself; a wrong code is said under the
/// field, which empties for the next try.
fn code_dialog(ui: &mut Ui, m: &MeshView, mem: &mut DevicesState, out: &mut Vec<Command>) {
    let Some(d) = mem.code.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let (done, failed) = code_outcome(&m.pairing, &d.id, d.sent.is_some());
    if done {
        mem.code = None;
        out.push(Command::MeshClearPairing);
        return;
    }
    if let Some(why) = failed {
        d.error = Some(why);
        d.sent = None;
        d.code.clear();
        d.focus = true;
        out.push(Command::MeshClearPairing);
    }
    let working = d.sent.is_some();
    if working {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let mut open = true;
    let r = Dialog::new("mesh-code-dialog", d.name.clone()).max_width(400.0).action("Отмена").show_with(&ctx, &mut open, |ui| {
        ui.add_space(16.0);
        let f = TextField::new(&mut d.code, "Код")
            .placeholder("000000")
            .width(ui.available_width())
            .request_focus(d.focus)
            .enabled(!working)
            .error_opt(d.error.clone())
            .id_salt("mesh-code-field")
            .show(ui);
        d.focus = false;
        if f.response.changed() {
            d.error = None;
        }
        if working {
            ui.add_space(16.0);
            ui.add(LoadingIndicator::new().size(24.0));
        }
    });
    let digits = code_digits(&d.code);
    if digits != d.code {
        d.code = digits;
    }
    if d.code.len() == 6 && d.sent.is_none() && m.pairing != MeshPairing::Working {
        d.sent = Some(d.code.clone());
        out.push(Command::MeshPairNearby { nearby_id: d.id.clone(), code: d.code.clone() });
    }
    if r.action.is_some() || r.dismissed || !open {
        mem.code = None;
        out.push(Command::MeshClearPairing);
    }
}

/// «Добавить устройство»: the ticket and the command for Linux, each with
/// «Копировать»; closes itself once a device joined with it.
fn invite_dialog(ui: &mut Ui, m: &MeshView, mem: &mut DevicesState, now: f64, out: &mut Vec<Command>) {
    let Some(d) = mem.invite.as_ref() else { return };
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    if matches!(m.pairing, MeshPairing::Done { .. }) {
        mem.invite = None;
        out.push(Command::MeshCancelInvite);
        out.push(Command::MeshClearPairing);
        return;
    }
    let error = match (&m.invite, &m.pairing) {
        (_, MeshPairing::Failed(why)) => Some(why.clone()),
        (None, _) if now - d.at > INVITE_WAIT_S => Some("Межсеть не ответила".to_owned()),
        _ => None,
    };
    if m.invite.is_none() && error.is_none() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut open = true;
    let r = Dialog::new("mesh-invite-dialog", "Добавить устройство")
        .max_width(560.0)
        .dismissible(false)
        .action_with(DialogAction::new("Готово"))
        .show_with(&ctx, &mut open, |ui| {
            ui.add_space(16.0);
            match (&m.invite, &error) {
                (_, Some(e)) => {
                    ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(e, s.error)).wrap());
                }
                (Some(inv), None) => {
                    code_block(ui, "mesh-invite-ticket", &inv.ticket, s);
                    ui.add_space(12.0);
                    ui.label(TypeRole::LabelLarge.style().rich("Linux и Raspberry Pi", s.on_surface_variant));
                    ui.add_space(8.0);
                    code_block(ui, "mesh-invite-command", &linux_command(&inv.ticket), s);
                }
                (None, None) => {
                    ui.add(LoadingIndicator::new().size(24.0));
                }
            }
        });
    if r.action.is_some() || esc || !open {
        mem.invite = None;
        out.push(Command::MeshCancelInvite);
        out.push(Command::MeshClearPairing);
    }
}

/// «Переименовать»: the name this device shows for another one.
fn rename_dialog(ui: &mut Ui, mem: &mut DevicesState, out: &mut Vec<Command>) {
    let Some(d) = mem.rename.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let mut open = true;
    let mut submitted = false;
    let can = !d.name.trim().is_empty();
    let r = Dialog::new("mesh-rename-dialog", "Переименовать")
        .max_width(400.0)
        .action("Отмена")
        .action_with(DialogAction::new("Сохранить").enabled(can))
        .show_with(&ctx, &mut open, |ui| {
            ui.add_space(16.0);
            let f = TextField::new(&mut d.name, "Название").width(ui.available_width()).request_focus(d.focus).id_salt("mesh-rename-field").show(ui);
            d.focus = false;
            submitted = f.submitted;
        });
    if (r.action == Some(1) || submitted) && can {
        out.push(Command::MeshRename { node_id: Some(d.id.clone()), name: d.name.trim().to_owned() });
        mem.rename = None;
    } else if r.action.is_some() || r.dismissed || !open {
        mem.rename = None;
    }
}

#[cfg(test)]
mod tests {
    use nikgob_view::MeshNodeView;

    use super::*;
    use super::super::super::PageState;

    #[test]
    fn codes_take_digits_only() {
        assert_eq!(code_digits("04 29-17"), "042917");
        assert_eq!(code_digits("1234567"), "123456");
        assert_eq!(code_digits("abc"), "");
        assert_eq!(spaced_code("042917"), "042 917");
        assert_eq!(linux_command("nkm1abc"), "nikgobd pair nkm1abc");
    }

    #[test]
    fn the_code_dialog_closes_on_its_device_and_says_why_otherwise() {
        assert_eq!(code_outcome(&MeshPairing::Done { node_id: "a".into() }, "a", true), (true, None));
        assert_eq!(code_outcome(&MeshPairing::Done { node_id: "b".into() }, "a", true), (false, None));
        assert_eq!(code_outcome(&MeshPairing::Failed("Неверный код".into()), "a", true), (false, Some("Неверный код".into())));
        assert_eq!(code_outcome(&MeshPairing::Failed("old".into()), "a", false), (false, None), "nothing sent yet");
        assert_eq!(code_outcome(&MeshPairing::Working, "a", true), (false, None));
    }

    #[test]
    fn every_kind_has_its_icon() {
        let icons: std::collections::HashSet<Icon> = MeshKind::ALL.iter().map(|k| kind_icon(*k)).collect();
        assert_eq!(icons.len(), MeshKind::ALL.len());
    }

    /// The group draws this device, the paired ones and those nearby, and
    /// sends nothing by drawing; a typed code sends the pairing once.
    #[test]
    fn the_group_draws_its_rows() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        let mut state = ViewState::default();
        state.mesh.enabled = true;
        state.mesh.running = true;
        state.mesh.configurable = true;
        state.mesh.ring_holder = true;
        state.mesh.paired = true;
        state.mesh.this = MeshNodeView { id: "me".into(), name: "PC".into(), ..MeshNodeView::default() };
        state.mesh.nodes = vec![MeshNodeView { id: "pi".into(), name: "Raspberry Pi".into(), kind: MeshKind::Pi, online: true, ..MeshNodeView::default() }];
        state.mesh.nearby = vec![nikgob_view::MeshNearbyView { id: "tv".into(), name: "TV".into(), kind: MeshKind::Tv, addr: None }];
        let mut pages = PageState::default();
        let mut out = Vec::new();
        let frame = |ctx: &egui::Context, t: f64, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(900.0, 900.0))),
                time: Some(t),
                ..Default::default()
            };
            let mut done = ctx.run_ui(input, |ui| {
                super::super::rows::set_mode(ui.ctx(), super::super::rows::Mode { deep: true, dev: false, opacity: 1.0, stage: false });
                show(ui, state, &mut pages.net.devices, out);
            });
            done.textures_delta.clear();
        };
        for i in 0..3 {
            frame(&ctx, f64::from(i) * 0.1, &state, &mut pages, &mut out);
        }
        let drawn: Vec<Id> = super::super::settings_help::drawn_rows(&ctx).1.into_iter().map(|r| r.id).collect();
        for id in [Id::MeshThis, Id::MeshEnabled, Id::MeshDevices, Id::MeshNearby] {
            assert!(drawn.contains(&id), "{id:?}: {drawn:?}");
        }
        assert!(out.is_empty(), "nothing is sent by drawing: {out:?}");
        // The code dialog of the TV: six digits send one pairing.
        pages.net.devices.code = Some(CodeDialog { id: "tv".into(), name: "TV".into(), code: "12 34 56".into(), sent: None, focus: false, error: None });
        frame(&ctx, 0.5, &state, &mut pages, &mut out);
        assert_eq!(out, vec![Command::MeshPairNearby { nearby_id: "tv".into(), code: "123456".into() }]);
        out.clear();
        frame(&ctx, 0.6, &state, &mut pages, &mut out);
        assert!(out.is_empty(), "sent once");
        // A wrong code: said, the field empties.
        state.mesh.pairing = MeshPairing::Failed("Неверный код".into());
        frame(&ctx, 0.7, &state, &mut pages, &mut out);
        let d = pages.net.devices.code.as_ref().expect("still open");
        assert_eq!(d.error.as_deref(), Some("Неверный код"));
        assert!(d.code.is_empty());
        // Paired: the dialog closes itself.
        pages.net.devices.code.as_mut().unwrap().sent = Some("654321".into());
        state.mesh.pairing = MeshPairing::Done { node_id: "tv".into() };
        frame(&ctx, 0.8, &state, &mut pages, &mut out);
        assert!(pages.net.devices.code.is_none());
    }
}
