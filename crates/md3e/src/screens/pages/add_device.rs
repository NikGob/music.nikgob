//! «Добавить устройство»: a window grown out of the button that opened it
//! (container transform), two tabs on a card.
//!
//! «Подключить»: the QR of the invite (a phone scans it) and, for a device
//! seen nearby, six cells for the code it shows: the sixth digit sends
//! them, a wrong code empties them, the window closes itself once the
//! device is paired (as a guest when «Гость» is on).
//!
//! «Установить»: per platform whose build this computer has — Linux and
//! Raspberry Pi get one line that downloads the headless player from here,
//! installs it, pairs and starts it (`curl … | sh`, the ticket in it);
//! without a build, the `nikgobd pair` line for one installed by hand.
//! Windows gets the address of this version's program. Both need the API
//! on the home network: until then the blocker and the switch that lifts
//! it stand under the line.
//!
//! The invite lives 15 minutes; while the window stays open a new one
//! takes its place shortly before.

use egui::{Align, Color32, Event, Key, Layout, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{ApiAccess, ApiView, Command, MeshPairing, MeshScope, MeshView, ViewState};

use super::mesh_ui::{self, Look, MeshUi};
use crate::{
    Icon, Theme, TypeRole,
    components::{ConnectedGroup, FilterChip, IconButton, LoadingIndicator, ModalWindow, Size, Style, anim, modal_window, paint, paint::Radii},
};

/// The widest the window gets.
const MAX_W: f32 = 600.0;
const WINDOW_INSETS: f32 = 2.0 * modal_window::SHELL_PADDING;
const HEADER_H: f32 = 40.0;
const HEADER_GAP: f32 = 12.0;
const CARD_RADIUS: f32 = 24.0;
const CARD_PAD: f32 = 20.0;
/// The QR's plate.
const QR: f32 = 176.0;
/// A cell of the code, and between two.
const CELL: egui::Vec2 = vec2(44.0, 56.0);
const CELL_GAP: f32 = 8.0;
/// The invite is made anew this long before it ends (ms).
const RENEW_BEFORE_MS: i64 = 60_000;
/// …at most this often (s).
const RENEW_EVERY_S: f64 = 10.0;
/// How long the window waits for the first invite (s).
const INVITE_WAIT_S: f64 = 5.0;

/// A tab of the window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum AddTab {
    #[default]
    Connect,
    Install,
}

/// The builds this computer can hand out, looked for when the window opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Builds {
    linux: bool,
    windows: bool,
}

/// What the window keeps.
#[derive(Default)]
pub(crate) struct AddState {
    open: bool,
    tab: AddTab,
    /// The control it grew out of.
    from: Option<Rect>,
    /// The nearby device whose code is typed.
    nearby: Option<String>,
    code: String,
    /// The code sent, while its pairing runs.
    sent: Option<String>,
    error: Option<String>,
    focus: bool,
    guest: bool,
    /// «Установить»: the platform shown (0 Linux, 1 Windows).
    platform: usize,
    opened_at: f64,
    renewed_at: f64,
    builds: Builds,
    /// This computer's address in the home network, for the lines.
    lan: Option<String>,
    /// The QR drawn, and of which ticket and colours.
    qr: Option<(String, Color32, egui::TextureHandle)>,
    /// The pairing's end as it was when the window opened (another
    /// window's), not this one's to act on.
    stale: Option<MeshPairing>,
}

fn window_id() -> egui::Id {
    egui::Id::new("mesh-add-device")
}

/// Opens the window on `tab` (out of `from`), for the nearby device
/// `nearby` if one was picked; asks for an invite.
pub(crate) fn open(ctx: &egui::Context, m: &MeshView, mem: &mut MeshUi, from: Option<Rect>, tab: AddTab, nearby: Option<String>, out: &mut Vec<Command>) {
    let a = &mut mem.add;
    if !a.open {
        ModalWindow::rewind(ctx, window_id());
        a.from = from;
        a.code.clear();
        a.sent = None;
        a.error = None;
        a.guest = false;
        a.builds = find_builds();
        a.lan = lan_of(m);
        a.platform = 0;
    }
    let now = anim::now(ctx);
    a.stale = matches!(m.pairing, MeshPairing::Done { .. } | MeshPairing::Failed(_)).then(|| m.pairing.clone());
    a.open = true;
    a.tab = tab;
    a.focus = true;
    a.opened_at = now;
    a.renewed_at = now;
    if nearby.is_some() {
        a.nearby = nearby;
        a.code.clear();
        a.error = None;
    }
    out.push(Command::MeshClearPairing);
    if m.can_pair() {
        out.push(Command::MeshCreateInvite);
    }
}

/// The window is up.
pub(crate) fn is_open(a: &AddState) -> bool {
    a.open
}

/// The builds of this version in the releases folder.
fn find_builds() -> Builds {
    use nikgob_core::releases::{LINUX_ARCHES, find_in, nikgobd_tarball, releases_dir, windows_exe};
    let version = env!("CARGO_PKG_VERSION");
    let Some(dir) = releases_dir() else { return Builds::default() };
    Builds {
        linux: LINUX_ARCHES.iter().any(|a| find_in(&dir, &nikgobd_tarball(version, a)).is_some()),
        windows: find_in(&dir, &windows_exe(version)).is_some(),
    }
}

/// This computer's home address: the mesh's, else the route out.
fn lan_of(m: &MeshView) -> Option<String> {
    let mesh = m.this.addr.as_deref().and_then(|a| a.parse::<std::net::SocketAddr>().ok()).map(|a| a.ip()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified());
    mesh.map(|ip| ip.to_string()).or_else(super::settings::lan_ip)
}

/// Where another device reaches the API: the address set, else this
/// computer's home address.
pub(crate) fn api_host(api: &ApiView, lan: Option<&str>) -> Option<String> {
    let ip = api.bind_ip();
    let host = if ip == "0.0.0.0" || ip == "::" || api.local_only() { lan?.to_owned() } else { ip };
    Some(if host.contains(':') { format!("[{host}]") } else { host })
}

/// The line that installs the headless player on Linux from this computer.
pub(crate) fn install_line(host: &str, port: u16, ticket: &str) -> String {
    format!("curl -fsSL \"http://{host}:{port}/install/{SCRIPT}?t={ticket}\" | sh")
}

/// The install script's name (the API's `/install/` serves it).
pub(crate) const SCRIPT: &str = "nikgobd.sh";

/// Where Windows downloads this version of the player from this computer.
pub(crate) fn windows_url(host: &str, port: u16, ticket: &str) -> String {
    format!("http://{host}:{port}/install/{}?t={ticket}", nikgob_core::releases::windows_exe(env!("CARGO_PKG_VERSION")))
}

/// What keeps the API from the home network, and the command that lifts
/// it: off, or this computer only. A bind error says itself.
fn api_blocker(api: &ApiView) -> Option<(String, Option<(&'static str, Command)>)> {
    if !api.enabled {
        Some(("API выключен".to_owned(), Some(("Включить", Command::SetApiEnabled(true)))))
    } else if api.local_only() {
        Some(("Доступ открыт только на этом компьютере".to_owned(), Some(("Включить домашнюю сеть", Command::SetApiAccess(ApiAccess::Network)))))
    } else {
        api.error.clone().map(|e| (e, None))
    }
}

/// Why this computer cannot add a device now, in words.
fn cannot_pair(m: &MeshView) -> Option<String> {
    if !m.enabled {
        Some("Связь с устройствами выключена".to_owned())
    } else if !m.running {
        Some(m.error.clone().unwrap_or_else(|| "Межсеть не запущена".to_owned()))
    } else if m.paired && !m.ring_holder {
        Some("Добавлять устройства может только главное устройство".to_owned())
    } else {
        None
    }
}

/// The window, while it is open (and while it leaves).
pub(crate) fn window(ui: &mut Ui, state: &ViewState, mem: &mut MeshUi, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let m = &state.mesh;
    let now = anim::now(&ctx);
    let wall = wall_ms();
    let a = &mut mem.add;
    if a.open {
        follow_pairing(a, m, out);
        // Before the invite ends, a new one in its place.
        if let Some(inv) = &m.invite
            && inv.expires_ms - wall < RENEW_BEFORE_MS
            && now - a.renewed_at > RENEW_EVERY_S
            && m.can_pair()
        {
            a.renewed_at = now;
            out.push(Command::MeshCreateInvite);
        }
        if m.invite.is_none() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
    let open = a.open;
    let tab = a.tab;
    let height_key = window_id().with(("content", tab));
    let content = ctx.data(|d| d.get_temp::<f32>(height_key)).unwrap_or(HEADER_H + HEADER_GAP + 2.0 * CARD_PAD + QR);
    let height = anim::f32(&ctx, window_id().with("height"), content + WINDOW_INSETS, theme.motion.default_spatial);
    let shell = ModalWindow::new(window_id(), vec2(MAX_W, f32::INFINITY)).height(height).shell();
    let shell = match a.from {
        Some(from) => shell.origin(from),
        None => shell,
    };
    let mut close = false;
    let mut to_tab = tab;
    let dismissed = shell.show(&ctx, open, |ui, _| {
        close = header(ui, &mut to_tab);
        ui.add_space(HEADER_GAP);
        let top = ui.cursor().min;
        let card = Rect::from_min_size(top, vec2(ui.available_width(), (height - WINDOW_INSETS - HEADER_H - HEADER_GAP).max(0.0)));
        paint::container(ui.painter(), card, Radii::all(CARD_RADIUS), theme.scheme.surface_container_highest, Stroke::NONE);
        let room = Rect::from_min_max(card.min + vec2(CARD_PAD, CARD_PAD), pos2(card.right() - CARD_PAD, card.bottom() + 2000.0));
        let mut on_card = ui.new_child(UiBuilder::new().max_rect(room).layout(Layout::top_down(Align::Min)).id_salt("mesh-add-card"));
        on_card.set_clip_rect(card.intersect(ui.clip_rect()));
        match cannot_pair(m) {
            Some(why) => error_line(&mut on_card, &why),
            None => match tab {
                AddTab::Connect => connect(&mut on_card, state, a, now),
                AddTab::Install => install(&mut on_card, state, a, now, out),
            },
        }
        let drawn = HEADER_H + HEADER_GAP + on_card.min_rect().height() + 2.0 * CARD_PAD;
        ui.ctx().data_mut(|d| d.insert_temp(height_key, drawn));
    });
    if to_tab != tab {
        a.tab = to_tab;
        a.focus = to_tab == AddTab::Connect;
    }
    if open && (dismissed || close) {
        close_window(a, out);
    }
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

fn close_window(a: &mut AddState, out: &mut Vec<Command>) {
    a.open = false;
    a.sent = None;
    a.code.clear();
    out.push(Command::MeshCancelInvite);
    out.push(Command::MeshClearPairing);
}

/// What the pairing's state does to the window: paired — it closes (the
/// new device a guest when asked); a wrong code — said, the cells empty;
/// six digits typed — sent once.
fn follow_pairing(a: &mut AddState, m: &MeshView, out: &mut Vec<Command>) {
    // The end of a pairing from before the window opened, until the
    // runtime has cleared it.
    let stale = match &a.stale {
        Some(p) if *p == m.pairing => true,
        _ => {
            a.stale = None;
            false
        }
    };
    match &m.pairing {
        _ if stale => {}
        MeshPairing::Done { node_id } => {
            if a.guest {
                out.push(Command::MeshSetScope { node_id: node_id.clone(), scope: MeshScope::Guest });
            }
            close_window(a, out);
            return;
        }
        MeshPairing::Failed(why) => {
            a.error = Some(why.clone());
            if a.sent.take().is_some() {
                a.code.clear();
                a.focus = true;
            }
            out.push(Command::MeshClearPairing);
        }
        _ => {}
    }
    // The device picked went away: the first one left, if any.
    if a.nearby.as_ref().is_none_or(|id| !m.nearby.iter().any(|n| n.id == *id)) {
        a.nearby = m.nearby.first().map(|n| n.id.clone());
    }
    if a.code.len() == 6 && a.sent.is_none() && m.pairing != MeshPairing::Working
        && let Some(id) = a.nearby.clone()
    {
        a.sent = Some(a.code.clone());
        a.error = None;
        out.push(Command::MeshPairNearby { nearby_id: id, code: a.code.clone() });
    }
}

/// The tabs and the close button. True when closed.
fn header(ui: &mut Ui, tab: &mut AddTab) -> bool {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), HEADER_H), Sense::hover());
    let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)).id_salt("mesh-add-header-right"));
    let closed = right
        .add(IconButton::new(Icon::Close).size(Size::S).style(Style::Standard).tooltip("Закрыть (Esc)").id_salt("mesh-add-close"))
        .clicked();
    let left = Rect::from_min_max(row.min, pos2((right.min_rect().left() - 12.0).max(row.left()), row.bottom()));
    let mut left = ui.new_child(UiBuilder::new().max_rect(left).layout(Layout::left_to_right(Align::Center)).id_salt("mesh-add-header-left"));
    let at = usize::from(*tab == AddTab::Install);
    if let Some(picked) = ConnectedGroup::new("mesh-add-tabs", &["Подключить", "Установить"]).show(&mut left, at) {
        *tab = if picked == 0 { AddTab::Connect } else { AddTab::Install };
    }
    closed
}

fn error_line(ui: &mut Ui, text: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(text, s.error)).wrap());
}

/// «Подключить»: the QR, and the code of a nearby device.
fn connect(ui: &mut Ui, state: &ViewState, a: &mut AddState, now: f64) {
    let m = &state.mesh;
    let narrow = ui.available_width() < QR + 24.0 + 6.0 * CELL.x + 5.0 * CELL_GAP;
    let invite_late = m.invite.is_none() && now - a.opened_at > INVITE_WAIT_S;
    let col_h_key = window_id().with("connect-col");
    let col_h: f32 = ui.ctx().data(|d| d.get_temp(col_h_key)).unwrap_or(0.0);
    let column = |ui: &mut Ui, a: &mut AddState| {
        let r = ui.scope(|ui| code_column(ui, state, a, invite_late)).response.rect;
        ui.ctx().data_mut(|d| d.insert_temp(col_h_key, r.height()));
    };
    if narrow {
        ui.vertical_centered(|ui| qr_plate(ui, m, a, invite_late));
        ui.add_space(20.0);
        column(ui, a);
    } else {
        ui.horizontal_top(|ui| {
            qr_plate(ui, m, a, invite_late);
            ui.add_space(24.0);
            ui.vertical(|ui| {
                // On the QR's middle line.
                ui.add_space(((QR - col_h) * 0.5).max(0.0));
                column(ui, a);
            });
        });
    }
}

/// The QR of the invite on its plate (a loading indicator until there is
/// one; an error when it never came).
fn qr_plate(ui: &mut Ui, m: &MeshView, a: &mut AddState, late: bool) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(vec2(QR, QR), Sense::hover());
    // Dark modules on light, whatever the theme: a reader wants them so.
    let (dark, light) = if luma(s.on_surface) < luma(s.surface) { (s.on_surface, s.surface) } else { (s.surface, s.on_surface) };
    let Some(inv) = &m.invite else {
        paint::container(ui.painter(), rect, Radii::all(16.0), s.surface_container_high, Stroke::NONE);
        if !late {
            let mut c = ui.new_child(UiBuilder::new().max_rect(Rect::from_center_size(rect.center(), vec2(48.0, 48.0))));
            c.add(LoadingIndicator::new().size(48.0));
        }
        return;
    };
    paint::container(ui.painter(), rect, Radii::all(16.0), light, Stroke::NONE);
    let fresh = a.qr.as_ref().is_some_and(|(t, d, _)| *t == inv.ticket && *d == dark);
    if !fresh {
        a.qr = qr_texture(ui.ctx(), &inv.ticket, dark, light).map(|t| (inv.ticket.clone(), dark, t));
    }
    if let Some((_, _, tex)) = &a.qr {
        let img = rect.shrink(10.0);
        ui.painter().image(tex.id(), img, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

fn luma(c: Color32) -> f32 {
    0.299 * f32::from(c.r()) + 0.587 * f32::from(c.g()) + 0.114 * f32::from(c.b())
}

/// The modules of the QR of `ticket` (upper case: the reader's
/// alphanumeric mode, a smaller code; the ticket reads either way), with a
/// quiet zone of two, row by row: true is dark.
pub(crate) fn qr_modules(ticket: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::with_error_correction_level(ticket.to_ascii_uppercase().as_bytes(), qrcode::EcLevel::M).ok()?;
    let w = code.width();
    let q = 2;
    let n = w + 2 * q;
    let colors = code.to_colors();
    let mut out = vec![false; n * n];
    for y in 0..w {
        for x in 0..w {
            out[(y + q) * n + x + q] = colors[y * w + x] == qrcode::Color::Dark;
        }
    }
    Some((n, out))
}

fn qr_texture(ctx: &egui::Context, ticket: &str, dark: Color32, light: Color32) -> Option<egui::TextureHandle> {
    let (n, modules) = qr_modules(ticket)?;
    let pixels = modules.into_iter().map(|d| if d { dark } else { light }).collect();
    let image = egui::ColorImage::new([n, n], pixels);
    Some(ctx.load_texture("mesh-invite-qr", image, egui::TextureOptions::NEAREST))
}

/// Beside the QR: the nearby device (a chip each when there are several),
/// its code's cells, what went wrong, «Гость».
fn code_column(ui: &mut Ui, state: &ViewState, a: &mut AddState, invite_late: bool) {
    let m = &state.mesh;
    let s = Theme::get(ui.ctx()).scheme;
    let working = a.sent.is_some() || m.pairing == MeshPairing::Working;
    if m.nearby.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            for n in &m.nearby {
                let on = a.nearby.as_deref() == Some(n.id.as_str());
                if ui.add(FilterChip::new(n.name.as_str(), on).one_of().enabled(!working).id_salt(("mesh-add-nearby", &n.id))).clicked() && !on {
                    a.nearby = Some(n.id.clone());
                    a.code.clear();
                    a.error = None;
                    a.focus = true;
                }
            }
        });
        ui.add_space(12.0);
    } else if let Some(n) = m.nearby.first() {
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width().min(6.0 * CELL.x + 5.0 * CELL_GAP), 32.0), Sense::hover());
        mesh_ui::paint_avatar(ui, pos2(row.left() + 16.0, row.center().y), 32.0, &format!("add-{}", n.id), n.kind, Look::default());
        let g = mesh_ui::text_line(ui.painter(), &n.name, TypeRole::TitleMedium, s.on_surface, row.width() - 44.0);
        ui.painter().galley(pos2(row.left() + 44.0, row.center().y - g.size().y * 0.5), g, s.on_surface);
        ui.add_space(12.0);
    }
    if a.nearby.is_some() {
        let focus = std::mem::take(&mut a.focus);
        code_cells(ui, egui::Id::new("mesh-add-code"), &mut a.code, !working, a.error.is_some(), focus);
        ui.add_space(8.0);
        if working {
            ui.add(LoadingIndicator::new().size(24.0));
        }
    }
    let error = a.error.clone().or_else(|| invite_late.then(|| "Межсеть не ответила".to_owned()));
    if let Some(e) = error.filter(|_| !working) {
        ui.add_space(4.0);
        error_line(ui, &e);
    }
    ui.add_space(12.0);
    if ui.add(FilterChip::new("Гость", a.guest).id_salt("mesh-add-guest")).clicked() {
        a.guest = !a.guest;
    }
}

/// Six cells for a code: typed or pasted digits fill them, Backspace
/// takes the last; the cell the next digit goes to is outlined while they
/// have the keys, all of them in error after a wrong code. True when the
/// code changed.
pub(crate) fn code_cells(ui: &mut Ui, id: egui::Id, code: &mut String, enabled: bool, error: bool, focus: bool) -> bool {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let w = 6.0 * CELL.x + 5.0 * CELL_GAP;
    let (rect, _) = ui.allocate_exact_size(vec2(w, CELL.y), Sense::hover());
    let live = enabled && ui.is_enabled();
    let resp = ui.interact(rect, id, if live { Sense::click() } else { Sense::hover() });
    if live && (focus || resp.clicked()) {
        resp.request_focus();
    }
    let keys = live && resp.has_focus();
    let mut changed = false;
    if keys {
        let events = ui.input(|i| i.events.clone());
        let texted = events.iter().any(|e| matches!(e, Event::Text(_) | Event::Paste(_)));
        for e in &events {
            match e {
                Event::Text(t) | Event::Paste(t) => changed |= take_digits(code, t),
                // Where a platform sends keys and no text for them.
                Event::Key { key, pressed: true, .. } if !texted => {
                    if let Some(d) = digit_of(*key) {
                        changed |= take_digits(code, &d.to_string());
                    }
                }
                _ => {}
            }
            if let Event::Key { key: Key::Backspace, pressed: true, .. } = e
                && code.pop().is_some()
            {
                changed = true;
            }
        }
    }
    let next = code.chars().count().min(5);
    for i in 0..6 {
        let cell = Rect::from_min_size(pos2(rect.left() + i as f32 * (CELL.x + CELL_GAP), rect.top()), CELL);
        let lit = keys && i == next && code.chars().count() < 6;
        let (width, color) = if error {
            (2.0, s.error)
        } else if lit {
            (2.0, s.primary)
        } else {
            (1.0, s.outline_variant)
        };
        let k = anim::fraction(&ctx, id.with(("lit", i)), if lit || error { 1.0 } else { 0.0 }, theme.motion.fast_effects);
        let stroke = Stroke::new(1.0 + (width - 1.0) * k, color);
        paint::container(ui.painter(), cell, Radii::all(12.0), s.surface, stroke);
        if let Some(c) = code.chars().nth(i) {
            ui.painter().text(cell.center(), egui::Align2::CENTER_CENTER, c, TypeRole::HeadlineMedium.font_id(), s.on_surface);
        }
    }
    if keys {
        // The outlined cell follows the keys at once.
        ctx.request_repaint();
    }
    changed
}

/// Appends the digits of `text` to `code`, up to six. True when any.
pub(crate) fn take_digits(code: &mut String, text: &str) -> bool {
    let mut any = false;
    for c in text.chars().filter(char::is_ascii_digit) {
        if code.chars().count() >= 6 {
            break;
        }
        code.push(c);
        any = true;
    }
    any
}

fn digit_of(key: Key) -> Option<u8> {
    Some(match key {
        Key::Num0 => 0,
        Key::Num1 => 1,
        Key::Num2 => 2,
        Key::Num3 => 3,
        Key::Num4 => 4,
        Key::Num5 => 5,
        Key::Num6 => 6,
        Key::Num7 => 7,
        Key::Num8 => 8,
        Key::Num9 => 9,
        _ => return None,
    })
}

/// «Установить»: the platforms this computer has builds for; the line to
/// copy; what keeps the API from the home network.
fn install(ui: &mut Ui, state: &ViewState, a: &mut AddState, now: f64, out: &mut Vec<Command>) {
    let m = &state.mesh;
    let s = Theme::get(ui.ctx()).scheme;
    let Some(inv) = &m.invite else {
        if now - a.opened_at > INVITE_WAIT_S {
            error_line(ui, "Межсеть не ответила");
        } else {
            ui.add(LoadingIndicator::new().size(32.0));
        }
        return;
    };
    // Windows only when its program is here (no tab that leads nowhere).
    let platforms: &[&str] = if a.builds.windows { &["Linux и Raspberry Pi", "Windows"] } else { &["Linux и Raspberry Pi"] };
    if a.platform >= platforms.len() {
        a.platform = 0;
    }
    if platforms.len() > 1 {
        if let Some(p) = ConnectedGroup::new("mesh-add-platform", platforms).show(ui, a.platform) {
            a.platform = p;
        }
        ui.add_space(16.0);
    }
    let api = &state.api;
    let host = api_host(api, a.lan.as_deref());
    let served = (a.platform == 0 && a.builds.linux) || a.platform == 1;
    let line = match (a.platform, &host) {
        (0, Some(h)) if a.builds.linux => install_line(h, api.port, &inv.ticket),
        (1, Some(h)) => windows_url(h, api.port, &inv.ticket),
        // No build to hand out (or no home address): pairing one installed
        // by hand.
        _ => mesh_ui_pair_line(&inv.ticket),
    };
    super::settings::code_block(ui, ("mesh-add-line", a.platform), &line, s);
    if served {
        let blocker = api_blocker(api).or_else(|| host.is_none().then(|| ("У компьютера нет адреса в домашней сети".to_owned(), None)));
        if let Some((text, fix)) = blocker {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(TypeRole::BodyMedium.style().rich(text, s.on_surface_variant));
                if let Some((label, c)) = fix
                    && ui.add(crate::components::Button::new(label).style(Style::Text).size(Size::S).id_salt("mesh-add-fix")).clicked()
                {
                    out.push(c);
                }
            });
        }
    }
}

/// The line that pairs a headless player installed by hand.
pub(crate) fn mesh_ui_pair_line(ticket: &str) -> String {
    format!("nikgobd pair {ticket}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_fill_six_cells_at_most() {
        let mut code = String::new();
        assert!(take_digits(&mut code, "04 29-1"));
        assert_eq!(code, "04291");
        assert!(take_digits(&mut code, "789"));
        assert_eq!(code, "042917");
        assert!(!take_digits(&mut code, "5"), "full");
        assert!(!take_digits(&mut String::new(), "abc"));
        assert_eq!(digit_of(Key::Num7), Some(7));
        assert_eq!(digit_of(Key::A), None);
    }

    #[test]
    fn the_lines_carry_the_address_and_the_ticket() {
        assert_eq!(
            install_line("192.168.0.10", 47210, "nkm1abc"),
            "curl -fsSL \"http://192.168.0.10:47210/install/nikgobd.sh?t=nkm1abc\" | sh"
        );
        assert!(windows_url("192.168.0.10", 47210, "nkm1abc").starts_with("http://192.168.0.10:47210/install/music.nikgob-"));
        assert!(windows_url("192.168.0.10", 47210, "nkm1abc").ends_with(".exe?t=nkm1abc"));
        assert_eq!(mesh_ui_pair_line("nkm1abc"), "nikgobd pair nkm1abc");
        assert_eq!(SCRIPT, "nikgobd.sh");
    }

    #[test]
    fn the_api_is_reached_at_the_home_address() {
        let mut api = ApiView::default();
        assert_eq!(api_host(&api, Some("192.168.0.10")).as_deref(), Some("192.168.0.10"));
        assert!(api_blocker(&api).is_some(), "off");
        api.enabled = true;
        let (text, fix) = api_blocker(&api).expect("this computer only");
        assert_eq!(text, "Доступ открыт только на этом компьютере");
        assert_eq!(fix.map(|f| f.1), Some(Command::SetApiAccess(ApiAccess::Network)));
        api.access = ApiAccess::Network;
        assert!(api_blocker(&api).is_none());
        assert_eq!(api_host(&api, None), None, "no home address");
        api.address = "192.168.0.157".into();
        assert_eq!(api_host(&api, None).as_deref(), Some("192.168.0.157"));
    }

    #[test]
    fn who_cannot_add_a_device_is_told_why() {
        let mut m = MeshView { enabled: true, running: true, ..MeshView::default() };
        assert_eq!(cannot_pair(&m), None);
        m.paired = true;
        assert!(cannot_pair(&m).is_some(), "a member that does not hold the ring");
        m.ring_holder = true;
        assert_eq!(cannot_pair(&m), None);
        m.running = false;
        assert_eq!(cannot_pair(&m).as_deref(), Some("Межсеть не запущена"));
    }

    /// The ticket's QR is square with its quiet zone, and dark modules in
    /// the finder's corner.
    #[test]
    fn a_ticket_makes_a_qr() {
        let (n, m) = qr_modules("nkm1ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8").expect("a QR");
        assert_eq!(m.len(), n * n);
        assert!(n >= 21 + 4);
        assert!(!m[0], "the quiet zone is light");
        assert!(m[2 * n + 2], "the finder's corner is dark");
    }

    /// A pairing that ended before the window opened does not close it.
    #[test]
    fn an_old_pairing_does_not_close_a_new_window() {
        let mut a = AddState { open: true, stale: Some(MeshPairing::Done { node_id: "x".into() }), ..AddState::default() };
        let m = MeshView { pairing: MeshPairing::Done { node_id: "x".into() }, ..MeshView::default() };
        let mut out = Vec::new();
        follow_pairing(&mut a, &m, &mut out);
        assert!(a.open);
        follow_pairing(&mut a, &MeshView::default(), &mut out);
        assert!(a.stale.is_none(), "cleared once the runtime has");
    }

    /// Paired: the window closes and the device becomes a guest when asked;
    /// a wrong code is said and the cells empty; six digits go once.
    #[test]
    fn the_pairing_drives_the_window() {
        let mut a = AddState { open: true, nearby: Some("tv".into()), code: "123456".into(), ..AddState::default() };
        let mut m = MeshView { enabled: true, running: true, ..MeshView::default() };
        m.nearby = vec![nikgob_view::MeshNearbyView { id: "tv".into(), name: "TV".into(), ..Default::default() }];
        let mut out = Vec::new();
        follow_pairing(&mut a, &m, &mut out);
        assert_eq!(out, vec![Command::MeshPairNearby { nearby_id: "tv".into(), code: "123456".into() }]);
        out.clear();
        follow_pairing(&mut a, &m, &mut out);
        assert!(out.is_empty(), "sent once");
        m.pairing = MeshPairing::Failed("Неверный код".into());
        follow_pairing(&mut a, &m, &mut out);
        assert_eq!(a.error.as_deref(), Some("Неверный код"));
        assert!(a.code.is_empty() && a.sent.is_none());
        out.clear();
        a.guest = true;
        m.pairing = MeshPairing::Done { node_id: "tv".into() };
        follow_pairing(&mut a, &m, &mut out);
        assert!(!a.open);
        assert_eq!(out[0], Command::MeshSetScope { node_id: "tv".into(), scope: MeshScope::Guest });
        assert!(out.contains(&Command::MeshCancelInvite));
    }
}
