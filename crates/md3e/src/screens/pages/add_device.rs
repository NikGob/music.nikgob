//! «Добавить устройство»: a window grown out of the button that opened it
//! (container transform), two tabs on a card.
//!
//! «Подключить»: the QR of the invite (a phone scans it), the devices seen
//! nearby (each opens its code window), «Гость» — the invite made anew as
//! a guest's (the holder signs the guest's certificate when it joins). The
//! window closes itself once a device joined with the invite.
//!
//! A nearby device's own small window (from its «Подключить»): its avatar
//! and name, six cells that have the keys at once, nothing else; the sixth
//! digit sends them, a wrong code empties them and shakes them, a pairing
//! closes it.
//!
//! «Установить»: per platform whose build this computer has — Linux and
//! Raspberry Pi get one line that downloads the install script from here
//! with the invite's download key, checks it against its SHA-256 and runs
//! it with the ticket ([`nikgob_core::releases`]: the ticket never crosses
//! the network); without a build, the `~/.local/bin/nikgobd pair` line for
//! one installed by hand (where the README installs it). Windows gets the address of this version's program.
//! Both need the API on the home network: until then the blocker and the
//! switch that lifts it stand under the line.
//!
//! The invite lives 15 minutes; while the window stays open a new one
//! takes its place shortly before, and one spent or gone is asked for
//! again.

use std::sync::{Arc, Mutex, PoisonError};

use egui::{Align, Color32, Event, Key, Layout, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_core::releases::{self, LinuxBuild};
use nikgob_view::{ApiAccess, ApiView, Command, MeshKind, MeshPairing, MeshScope, MeshView, ViewState};

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
/// The column beside the QR.
const COLUMN_W: f32 = 280.0;
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

/// The builds this computer can hand out, looked for (and the Linux ones'
/// checksums made) on a thread of their own when the window opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Builds {
    linux: Vec<LinuxBuild>,
    windows: bool,
}

/// This version (the API's: one version for the whole player).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the window keeps.
#[derive(Default)]
pub(crate) struct AddState {
    open: bool,
    tab: AddTab,
    /// The control it grew out of.
    from: Option<Rect>,
    /// Why the invite could not be made, in words.
    error: Option<String>,
    guest: bool,
    /// «Установить»: the platform shown (0 Linux, 1 Windows).
    platform: usize,
    /// Since when no invite of the scope wanted is shown (none: one is).
    missing_since: Option<f64>,
    /// When an invite was last asked for.
    renewed_at: f64,
    /// The builds, once looked for.
    builds: Arc<Mutex<Option<Builds>>>,
    /// The Linux line made, and of which address and ticket.
    line: Option<(String, String, String)>,
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

/// Opens the window on `tab` (out of `from`) and asks for an invite; a
/// nearby device's code window gives way to it.
pub(crate) fn open(ctx: &egui::Context, m: &MeshView, mem: &mut MeshUi, from: Option<Rect>, tab: AddTab, out: &mut Vec<Command>) {
    if mem.pair.open {
        close_pair(&mut mem.pair, out);
    }
    let a = &mut mem.add;
    if !a.open {
        ModalWindow::rewind(ctx, window_id());
        a.from = from;
        a.error = None;
        a.guest = false;
        a.lan = lan_of(m);
        a.platform = 0;
        a.line = None;
        a.builds = look_for_builds(ctx);
    }
    let now = anim::now(ctx);
    a.stale = matches!(m.pairing, MeshPairing::Done { .. } | MeshPairing::Failed(_)).then(|| m.pairing.clone());
    a.open = true;
    a.tab = tab;
    a.missing_since = Some(now);
    a.renewed_at = now;
    out.push(Command::MeshClearPairing);
    if m.can_pair() {
        out.push(Command::MeshCreateInvite { scope: wanted_scope(a) });
    }
}

/// The window is up.
pub(crate) fn is_open(a: &AddState) -> bool {
    a.open
}

/// What a device joining now should become.
fn wanted_scope(a: &AddState) -> MeshScope {
    if a.guest { MeshScope::Guest } else { MeshScope::Owner }
}

/// Looks for the builds on a thread (the tarballs' checksums take a
/// moment the first time); the window draws again when they are known.
fn look_for_builds(ctx: &egui::Context) -> Arc<Mutex<Option<Builds>>> {
    let slot = Arc::new(Mutex::new(None));
    let (to, ctx) = (slot.clone(), ctx.clone());
    let started = std::thread::Builder::new().name("nikgob-builds".into()).spawn(move || {
        let found = find_builds();
        *to.lock().unwrap_or_else(PoisonError::into_inner) = Some(found);
        ctx.request_repaint();
    });
    if started.is_err() {
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(Builds::default());
    }
    slot
}

/// The builds of this version in the releases folder.
fn find_builds() -> Builds {
    let Some(dir) = releases::releases_dir() else { return Builds::default() };
    Builds { linux: releases::linux_builds(&dir, VERSION), windows: releases::find_in(&dir, &releases::windows_exe(VERSION)).is_some() }
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

/// The line that installs the headless player on Linux from `host`
/// (host:port): the script the API serves for the ticket's download key,
/// pinned by the checksum of exactly those bytes, run with the ticket.
pub(crate) fn install_line(host: &str, ticket: &str, builds: &[LinuxBuild]) -> String {
    let key = releases::download_key(ticket);
    let script = releases::install_script(host, &key, VERSION, builds);
    releases::install_command(host, &key, &releases::sha256_hex(script.as_bytes()), ticket)
}

/// Where Windows downloads this version of the player from `host`
/// (host:port).
pub(crate) fn windows_url(host: &str, ticket: &str) -> String {
    releases::windows_url(host, &releases::download_key(ticket), VERSION)
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
        let scope = wanted_scope(a);
        if needs_invite(m, scope, wall) && now - a.renewed_at > RENEW_EVERY_S && m.can_pair() {
            a.renewed_at = now;
            out.push(Command::MeshCreateInvite { scope });
        }
        if shown_invite(m, scope).is_some() {
            a.missing_since = None;
        } else {
            a.missing_since.get_or_insert(now);
            // Until it comes: soon after the ask, then at the next try.
            let wait = if now - a.renewed_at < INVITE_WAIT_S { 0.2 } else { 1.0 };
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
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
    // A nearby device picked: its code window takes over.
    let mut pick: Option<(String, Rect)> = None;
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
                AddTab::Connect => connect(&mut on_card, state, a, now, &mut pick, out),
                AddTab::Install => install(&mut on_card, state, a, now, out),
            },
        }
        let drawn = HEADER_H + HEADER_GAP + on_card.min_rect().height() + 2.0 * CARD_PAD;
        ui.ctx().data_mut(|d| d.insert_temp(height_key, drawn));
    });
    if to_tab != tab {
        a.tab = to_tab;
    }
    if open && (dismissed || close) {
        close_window(a, out);
    }
    if let Some((id, from)) = pick {
        open_pair(&ctx, m, mem, Some(from), &id, out);
    }
}

/// The invite to show: the one held, if it makes what «Гость» asks for.
fn shown_invite(m: &MeshView, scope: MeshScope) -> Option<&nikgob_view::MeshInvite> {
    m.invite.as_ref().filter(|i| i.scope == scope)
}

/// An open window wants another invite at `wall` (unix ms): none is held
/// (spent by a device that joined, gone with an expiry), it is of the
/// other scope, or it ends soon.
fn needs_invite(m: &MeshView, scope: MeshScope, wall: i64) -> bool {
    match &m.invite {
        None => true,
        Some(i) => i.scope != scope || i.expires_ms - wall < RENEW_BEFORE_MS,
    }
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

fn close_window(a: &mut AddState, out: &mut Vec<Command>) {
    a.open = false;
    out.push(Command::MeshCancelInvite);
    out.push(Command::MeshClearPairing);
}

/// What the pairing's state does to the window: a device joined with the
/// invite — it closes; the invite could not be made — said.
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
        MeshPairing::Done { .. } => close_window(a, out),
        MeshPairing::Failed(why) => {
            a.error = Some(why.clone());
            out.push(Command::MeshClearPairing);
        }
        _ => {}
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

/// «Подключить»: the QR, the devices nearby, «Гость».
fn connect(ui: &mut Ui, state: &ViewState, a: &mut AddState, now: f64, pick: &mut Option<(String, Rect)>, out: &mut Vec<Command>) {
    let m = &state.mesh;
    let narrow = ui.available_width() < QR + 24.0 + COLUMN_W;
    let invite_late = a.missing_since.is_some_and(|at| now - at > INVITE_WAIT_S);
    let col_h_key = window_id().with("connect-col");
    let col_h: f32 = ui.ctx().data(|d| d.get_temp(col_h_key)).unwrap_or(0.0);
    let column = |ui: &mut Ui, a: &mut AddState, pick: &mut Option<(String, Rect)>, out: &mut Vec<Command>| {
        let r = ui.scope(|ui| invite_column(ui, state, a, invite_late, now, pick, out)).response.rect;
        ui.ctx().data_mut(|d| d.insert_temp(col_h_key, r.height()));
    };
    if narrow {
        ui.vertical_centered(|ui| qr_plate(ui, m, a, invite_late));
        ui.add_space(20.0);
        column(ui, a, pick, out);
    } else {
        ui.horizontal_top(|ui| {
            qr_plate(ui, m, a, invite_late);
            ui.add_space(24.0);
            ui.vertical(|ui| {
                // On the QR's middle line.
                ui.add_space(((QR - col_h) * 0.5).max(0.0));
                column(ui, a, pick, out);
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
    let Some(inv) = shown_invite(m, wanted_scope(a)) else {
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

/// Beside the QR: the devices nearby (a row each: its avatar, its name,
/// «Подключить» opening its code window), what went wrong, «Гость».
fn invite_column(ui: &mut Ui, state: &ViewState, a: &mut AddState, invite_late: bool, now: f64, pick: &mut Option<(String, Rect)>, out: &mut Vec<Command>) {
    let m = &state.mesh;
    let s = Theme::get(ui.ctx()).scheme;
    for n in &m.nearby {
        let (row, _) = ui.allocate_exact_size(vec2(ui.available_width().min(COLUMN_W), 40.0), Sense::hover());
        mesh_ui::paint_avatar(ui, pos2(row.left() + 16.0, row.center().y), 32.0, &format!("add-{}", n.id), n.kind, Look::default());
        let mut right = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)).id_salt(("mesh-add-near", &n.id)));
        let b = right.add(crate::components::Button::new("Подключить").style(Style::Text).size(Size::S).id_salt(("mesh-add-pair", &n.id)));
        if b.clicked() {
            *pick = Some((n.id.clone(), b.rect));
        }
        let room = (b.rect.left() - row.left() - 44.0 - 8.0).max(0.0);
        let g = mesh_ui::text_line(ui.painter(), &n.name, TypeRole::BodyLarge, s.on_surface, room);
        ui.painter().galley(pos2(row.left() + 44.0, row.center().y - g.size().y * 0.5), g, s.on_surface);
    }
    if !m.nearby.is_empty() {
        ui.add_space(12.0);
    }
    let error = a.error.clone().or_else(|| invite_late.then(|| "Межсеть не ответила".to_owned()));
    if let Some(e) = error {
        error_line(ui, &e);
        ui.add_space(12.0);
    }
    if ui.add(FilterChip::new("Гость", a.guest).id_salt("mesh-add-guest")).clicked() {
        a.guest = !a.guest;
        // The invite shown stops being one: another, of the new scope,
        // signs what joins with it (the QR waits for it).
        if m.can_pair() {
            a.renewed_at = now;
            out.push(Command::MeshCreateInvite { scope: wanted_scope(a) });
        }
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
    let builds = a.builds.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let (Some(inv), Some(builds)) = (shown_invite(m, wanted_scope(a)), builds) else {
        if a.missing_since.is_some_and(|at| now - at > INVITE_WAIT_S) {
            error_line(ui, "Межсеть не ответила");
        } else {
            ui.add(LoadingIndicator::new().size(32.0));
        }
        return;
    };
    let linux = !builds.linux.is_empty();
    // Windows only when its program is here (no tab that leads nowhere).
    let platforms: &[&str] = if builds.windows { &["Linux и Raspberry Pi", "Windows"] } else { &["Linux и Raspberry Pi"] };
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
    let host = api_host(api, a.lan.as_deref()).map(|h| format!("{h}:{}", api.port));
    let served = (a.platform == 0 && linux) || a.platform == 1;
    let line = match (a.platform, &host) {
        (0, Some(h)) if linux => {
            // Made once per address and ticket (a checksum of the script).
            if !a.line.as_ref().is_some_and(|(lh, lt, _)| lh == h && *lt == inv.ticket) {
                a.line = Some((h.clone(), inv.ticket.clone(), install_line(h, &inv.ticket, &builds.linux)));
            }
            a.line.as_ref().map(|(_, _, l)| l.clone()).unwrap_or_default()
        }
        (1, Some(h)) => windows_url(h, &inv.ticket),
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

/// The line that pairs a headless player installed by hand (where the
/// README installs it: `~/.local/bin` is often not on the PATH of a fresh
/// shell).
pub(crate) fn mesh_ui_pair_line(ticket: &str) -> String {
    format!("~/.local/bin/nikgobd pair {ticket}")
}

/// What the device says to a wrong code ([`MeshPairing::Failed`]): the
/// cells shake, no word.
const WRONG_CODE: &str = "Неверный код";
/// How long the cells shake (s), how far (dp) and how many times.
const SHAKE_S: f64 = 0.45;
const SHAKE_DP: f32 = 10.0;
const SHAKE_TIMES: f32 = 4.0;
/// The code window: its avatar, between it and the name, the name's room,
/// between the name and the cells.
const PAIR_AVATAR: f32 = 56.0;
const PAIR_GAP: f32 = 12.0;
const PAIR_NAME_H: f32 = 28.0;
const PAIR_CELLS_GAP: f32 = 24.0;

/// A nearby device's code window.
#[derive(Default)]
pub(crate) struct PairState {
    pub(crate) open: bool,
    from: Option<Rect>,
    nearby: String,
    name: String,
    kind: MeshKind,
    code: String,
    /// The code went; its pairing runs.
    sent: bool,
    /// The last code was wrong: the cells in error until a digit comes.
    wrong: bool,
    /// When the cells began to shake.
    shake_at: Option<f64>,
    /// Another failure (no link, busy), in words.
    error: Option<String>,
    focus: bool,
    /// The pairing's end as it was when the window opened.
    stale: Option<MeshPairing>,
}

fn pair_id() -> egui::Id {
    egui::Id::new("mesh-pair-code")
}

/// Opens the code window of the nearby device `nearby_id` (out of
/// `from`); «Добавить устройство» gives way to it.
pub(crate) fn open_pair(ctx: &egui::Context, m: &MeshView, mem: &mut MeshUi, from: Option<Rect>, nearby_id: &str, out: &mut Vec<Command>) {
    let Some(n) = m.nearby.iter().find(|n| n.id == nearby_id) else { return };
    if mem.add.open {
        close_window(&mut mem.add, out);
    }
    let p = &mut mem.pair;
    if !p.open || p.nearby != nearby_id {
        ModalWindow::rewind(ctx, pair_id());
    }
    *p = PairState {
        open: true,
        from,
        nearby: n.id.clone(),
        name: n.name.clone(),
        kind: n.kind,
        focus: true,
        stale: matches!(m.pairing, MeshPairing::Done { .. } | MeshPairing::Failed(_)).then(|| m.pairing.clone()),
        ..PairState::default()
    };
    out.push(Command::MeshClearPairing);
}

fn close_pair(p: &mut PairState, out: &mut Vec<Command>) {
    p.open = false;
    p.sent = false;
    p.code.clear();
    out.push(Command::MeshClearPairing);
}

/// What the pairing does to the code window: paired — it closes; a wrong
/// code — the cells empty and shake; another failure — said; six digits —
/// sent once.
fn follow_pair(p: &mut PairState, m: &MeshView, now: f64, out: &mut Vec<Command>) {
    let stale = match &p.stale {
        Some(s) if *s == m.pairing => true,
        _ => {
            p.stale = None;
            false
        }
    };
    match &m.pairing {
        _ if stale => {}
        MeshPairing::Done { .. } if p.sent => {
            close_pair(p, out);
            return;
        }
        MeshPairing::Failed(why) => {
            if std::mem::take(&mut p.sent) {
                p.code.clear();
                p.focus = true;
            }
            if why == WRONG_CODE {
                p.wrong = true;
                p.shake_at = Some(now);
                p.error = None;
            } else {
                p.error = Some(why.clone());
            }
            out.push(Command::MeshClearPairing);
        }
        _ => {}
    }
    if p.code.len() == 6 && !p.sent && m.pairing != MeshPairing::Working {
        p.sent = true;
        p.error = None;
        out.push(Command::MeshPairNearby { nearby_id: p.nearby.clone(), code: p.code.clone() });
    }
}

/// The cells' sideways offset `t` seconds into a shake: a few swings that
/// die away.
fn shake_offset(t: f64) -> f32 {
    if !(0.0..SHAKE_S).contains(&t) {
        return 0.0;
    }
    let k = (t / SHAKE_S) as f32;
    SHAKE_DP * (1.0 - k) * (k * SHAKE_TIMES * std::f32::consts::TAU).sin()
}

/// The code window, while it is open (and while it leaves).
pub(crate) fn pair_window(ui: &mut Ui, state: &ViewState, mem: &mut MeshUi, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let s = theme.scheme;
    let m = &state.mesh;
    let now = anim::now(&ctx);
    let p = &mut mem.pair;
    if p.open {
        follow_pair(p, m, now, out);
    }
    let cells_w = 6.0 * CELL.x + 5.0 * CELL_GAP;
    let error_h = if p.error.is_some() { 36.0 } else { 0.0 };
    let content = PAIR_AVATAR + PAIR_GAP + PAIR_NAME_H + PAIR_CELLS_GAP + CELL.y + error_h;
    let height = anim::f32(&ctx, pair_id().with("height"), content + 2.0 * modal_window::PADDING, theme.motion.default_spatial);
    let window = ModalWindow::new(pair_id(), vec2(cells_w + 2.0 * modal_window::PADDING, f32::INFINITY)).height(height);
    let window = match p.from {
        Some(from) => window.origin(from),
        None => window,
    };
    let open = p.open;
    let dismissed = window.show(&ctx, open, |ui, inner| {
        let mid = inner.center().x;
        let top = inner.top();
        mesh_ui::paint_avatar(ui, pos2(mid, top + PAIR_AVATAR * 0.5), PAIR_AVATAR, &format!("pair-{}", p.nearby), p.kind, Look::default());
        let g = mesh_ui::text_line(ui.painter(), &p.name, TypeRole::TitleLarge, s.on_surface, inner.width());
        let name_y = top + PAIR_AVATAR + PAIR_GAP + (PAIR_NAME_H - g.size().y) * 0.5;
        ui.painter().galley(pos2(mid - g.size().x * 0.5, name_y), g, s.on_surface);
        let shake = p.shake_at.map_or(0.0, |at| shake_offset(now - at));
        if p.shake_at.is_some_and(|at| now - at < SHAKE_S) {
            ctx.request_repaint();
        } else {
            p.shake_at = None;
        }
        let cells_top = top + PAIR_AVATAR + PAIR_GAP + PAIR_NAME_H + PAIR_CELLS_GAP;
        let cells = Rect::from_min_size(pos2(mid - cells_w * 0.5 + shake, cells_top), vec2(cells_w, CELL.y));
        let mut c = ui.new_child(UiBuilder::new().max_rect(cells).layout(Layout::left_to_right(Align::Min)).id_salt("mesh-pair-cells"));
        let working = p.sent || m.pairing == MeshPairing::Working;
        let focus = std::mem::take(&mut p.focus);
        if code_cells(&mut c, egui::Id::new("mesh-pair-code-cells"), &mut p.code, !working, p.wrong, focus) && !p.code.is_empty() {
            p.wrong = false;
        }
        if let Some(e) = &p.error {
            let r = Rect::from_min_size(pos2(inner.left(), cells.bottom() + 12.0), vec2(inner.width(), 24.0));
            let mut e_ui = ui.new_child(UiBuilder::new().max_rect(r).layout(Layout::top_down(Align::Center)).id_salt("mesh-pair-error"));
            error_line(&mut e_ui, e);
        }
    });
    if open && dismissed {
        close_pair(p, out);
    }
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

    /// The Linux line downloads with the key, pins the very script the API
    /// serves for it and hands the script the ticket; no pipe into sh.
    #[test]
    fn the_lines_carry_the_address_the_key_and_the_ticket() {
        let builds = [LinuxBuild { arch: "aarch64", sha256: "ab".repeat(32) }];
        let host = "192.168.0.10:47210";
        let key = releases::download_key("nkm1abc");
        let line = install_line(host, "nkm1abc", &builds);
        let served = releases::install_script(host, &key, VERSION, &builds);
        let sum = releases::sha256_hex(served.as_bytes());
        assert_eq!(line, releases::install_command(host, &key, &sum, "nkm1abc"));
        assert!(line.contains(&format!("/install/nikgobd.sh?k={key}")), "{line}");
        assert!(line.contains(&sum) && line.ends_with(" nkm1abc"), "{line}");
        assert!(!line.contains("| sh "), "{line}");
        assert_ne!(line, install_line(host, "nkm1abc", &[]), "another set of builds, another script");
        let url = windows_url(host, "nkm1abc");
        assert!(url.starts_with("http://192.168.0.10:47210/install/music.nikgob-"), "{url}");
        assert!(url.ends_with(&format!(".exe?k={key}")) && !url.contains("nkm1abc"), "{url}");
        assert_eq!(mesh_ui_pair_line("nkm1abc"), "~/.local/bin/nikgobd pair nkm1abc");
    }

    /// An open window asks for an invite when none is held (spent, gone),
    /// when the one held is of the other scope, and before it ends.
    #[test]
    fn the_window_keeps_an_invite_of_its_scope() {
        let mut m = MeshView::default();
        assert!(needs_invite(&m, MeshScope::Owner, 0), "none held");
        m.invite = Some(nikgob_view::MeshInvite { ticket: "nkm1abc".into(), expires_ms: 600_000, scope: MeshScope::Owner });
        assert!(!needs_invite(&m, MeshScope::Owner, 0));
        assert!(shown_invite(&m, MeshScope::Owner).is_some());
        assert!(needs_invite(&m, MeshScope::Guest, 0), "«Гость» wants a guest's invite");
        assert!(shown_invite(&m, MeshScope::Guest).is_none(), "the owner's QR is not shown for a guest");
        assert!(needs_invite(&m, MeshScope::Owner, 600_000 - RENEW_BEFORE_MS + 1), "ending soon");
    }

    /// A device that joined with the invite closes the window as it was
    /// signed (no scope set after); only one added by its code here becomes
    /// a guest after.
    #[test]
    fn a_guest_invite_is_not_made_a_guest_again() {
        let mut a = AddState { open: true, guest: true, ..AddState::default() };
        let m = MeshView { pairing: MeshPairing::Done { node_id: "pi".into() }, ..MeshView::default() };
        let mut out = Vec::new();
        follow_pairing(&mut a, &m, &mut out);
        assert!(!a.open);
        assert!(!out.iter().any(|c| matches!(c, Command::MeshSetScope { .. })), "{out:?}");
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

    /// The code window: six digits go once; a wrong code empties the cells
    /// and shakes them without a word; another failure is said; a pairing
    /// closes it.
    #[test]
    fn the_pairing_drives_the_code_window() {
        let mut p = PairState { open: true, nearby: "tv".into(), code: "123456".into(), ..PairState::default() };
        let mut m = MeshView { enabled: true, running: true, ..MeshView::default() };
        let mut out = Vec::new();
        follow_pair(&mut p, &m, 1.0, &mut out);
        assert_eq!(out, vec![Command::MeshPairNearby { nearby_id: "tv".into(), code: "123456".into() }]);
        out.clear();
        follow_pair(&mut p, &m, 1.1, &mut out);
        assert!(out.is_empty(), "sent once");
        m.pairing = MeshPairing::Failed(WRONG_CODE.into());
        follow_pair(&mut p, &m, 2.0, &mut out);
        assert!(p.code.is_empty() && !p.sent && p.wrong && p.focus);
        assert_eq!((p.shake_at, p.error.as_deref()), (Some(2.0), None), "a shake, no caption");
        m.pairing = MeshPairing::Failed("Нет связи с устройством".into());
        follow_pair(&mut p, &m, 3.0, &mut out);
        assert_eq!(p.error.as_deref(), Some("Нет связи с устройством"));
        out.clear();
        p.code = "654321".into();
        m.pairing = MeshPairing::Idle;
        follow_pair(&mut p, &m, 4.0, &mut out);
        assert!(p.sent && p.error.is_none(), "the new code went");
        m.pairing = MeshPairing::Done { node_id: "tv".into() };
        follow_pair(&mut p, &m, 5.0, &mut out);
        assert!(!p.open);
        assert!(!out.iter().any(|c| matches!(c, Command::MeshSetScope { .. })), "an owner's device: «Гость» is the list's");
    }

    /// The shake swings and dies away, and rests outside its time.
    #[test]
    fn the_cells_shake_and_settle() {
        assert_eq!(shake_offset(-0.1), 0.0);
        assert_eq!(shake_offset(SHAKE_S), 0.0);
        let peak = (0..45).map(|i| shake_offset(f64::from(i) * 0.01).abs()).fold(0.0_f32, f32::max);
        assert!(peak > SHAKE_DP * 0.5 && peak <= SHAKE_DP, "{peak}");
        assert!(shake_offset(SHAKE_S * 0.95).abs() < SHAKE_DP * 0.1, "nearly still at the end");
    }

    /// From a nearby row the code window opens for that device alone, and
    /// «Добавить устройство» gives way to it.
    #[test]
    fn a_nearby_device_opens_its_own_code_window() {
        let ctx = egui::Context::default();
        let mut m = MeshView { enabled: true, running: true, ..MeshView::default() };
        m.nearby = vec![nikgob_view::MeshNearbyView { id: "pi".into(), name: "raspberrypi".into(), kind: MeshKind::Pi, ..Default::default() }];
        let mut mem = MeshUi::default();
        mem.add.open = true;
        let mut out = Vec::new();
        open_pair(&ctx, &m, &mut mem, None, "pi", &mut out);
        assert!(mem.pair.open && !mem.add.open);
        assert_eq!((mem.pair.name.as_str(), mem.pair.kind), ("raspberrypi", MeshKind::Pi));
        assert!(mem.pair.focus, "the cells have the keys at once");
        assert!(out.contains(&Command::MeshCancelInvite));
        open_pair(&ctx, &m, &mut mem, None, "gone", &mut out);
        assert_eq!(mem.pair.nearby, "pi", "no such device: nothing changes");
    }
}
