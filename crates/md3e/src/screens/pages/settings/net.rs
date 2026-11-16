//! «Межсеть»: the devices of the home (a group of its own from the
//! devices wave: until then it has no row and no title), then «API и
//! MCP» — its switch with «Открыть /docs», who may reach it, its port, and
//! the tokens: «Новый токен» (and two ready ones in its menu), a dialog
//! that shows the secret once with a ready snippet to paste, the list
//! with «Отозвать» (and «Отменить» in the snackbar), a row that pulses
//! when its token is used.
//!
//! Nothing here explains itself or reports a status in words: the switch,
//! the enabled «Открыть /docs» and the absence of an error say that the
//! API listens; the ⓘ cards say the rest.

use std::collections::HashMap;

use egui::{Key, Modifiers, Ui, vec2};
use nikgob_view::{ApiAccess, ApiActor, ApiNewToken, ApiPreset, ApiTokenView, ApiView, Command, ViewState, actor_label, preset_label};

use super::super::PageState;
use super::super::settings_help::SettingId as Id;
use super::rows::{Note, dimmed, group, row, row_note};
use super::services::copy_button;
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Button, Dialog, DialogAction, ListItem, LoadingIndicator, MenuItem, Position, Size, SplitButton, Style, Switch,
        TextField, anim,
    },
};

/// What «Межсеть» keeps between frames.
#[derive(Default)]
pub(crate) struct NetState {
    dialog: Option<TokenDialog>,
    /// What is typed into «Порт», and the runtime's port it was filled from.
    port_draft: String,
    port_from: Option<u16>,
    /// «Порт» had the focus last frame (the runtime's port does not
    /// overwrite what is being typed).
    port_focus: bool,
    /// The switch just flipped (and when), until the runtime shows it.
    enabled_sent: Option<(bool, f64)>,
    /// Each token's last use as seen, and when a new one came (its pulse).
    seen: HashMap<String, Option<i64>>,
    pulses: HashMap<String, f64>,
}

/// The token dialog: the form, then the secret (shown once).
struct TokenDialog {
    name: String,
    preset: ApiPreset,
    actor: ApiActor,
    /// The user picked «Кто включает» (a preset no longer picks it).
    actor_picked: bool,
    /// The name field takes the focus when the dialog opens.
    focus: bool,
    /// Sent, the secret held until the player says the token is kept.
    waiting: Option<Waiting>,
    /// Why the last one was not kept.
    error: Option<String>,
    made: Option<Made>,
}

/// A token sent to the player, and when.
struct Waiting {
    made: Made,
    at: f64,
}

/// What became of a token sent.
enum Kept {
    Yes,
    No(String),
    NotYet,
}

/// A token just made.
struct Made {
    id: String,
    name: String,
    secret: String,
    actor: ApiActor,
    /// [`SNIPPETS`] shown.
    snippet: usize,
    /// This computer's address in the home network, for the snippets.
    lan: Option<String>,
}

impl Drop for Made {
    /// The secret does not outlive the dialog.
    fn drop(&mut self) {
        let n = self.secret.len();
        self.secret.replace_range(.., &"0".repeat(n));
        self.secret.clear();
    }
}

const PRESETS: [ApiPreset; 3] = [ApiPreset::Control, ApiPreset::Full, ApiPreset::Read];
const ACTORS: [ApiActor; 4] = [ApiActor::Owner, ApiActor::Assistant, ApiActor::Agent, ApiActor::Guest];
/// The snippets of the dialog's second step.
const SNIPPETS: [&str; 3] = ["Claude Code", "mcpServers JSON", "curl"];
/// How long a used token's row stays lit before it fades back.
const PULSE_S: f64 = 0.25;
/// How long the switch shows its new place before the runtime has it.
const SENT_S: f64 = 3.0;
/// How long a token sent may wait for the player to keep it.
const KEEP_S: f64 = 5.0;

pub(super) fn show(ui: &mut Ui, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) {
    // «Устройства» comes with the devices wave; a group with no row has no
    // title either.
    group(ui, "Устройства");
    group(ui, "API и MCP");
    let api = &state.api;
    let mem = &mut pages.net;
    let now = anim::now(ui.ctx());
    let on = enabled_shown(mem, api, now);
    enabled_row(ui, api, on, mem, now, out);
    dimmed(ui, !on, |ui| {
        access_row(ui, api, out);
        port_row(ui, api, mem, out);
        tokens_row(ui, mem, now, out);
        token_list(ui, api, mem, now, out);
    });
    dialog(ui, api, mem, now, out);
}

/// The switch as the user flipped it, until the runtime shows that.
fn enabled_shown(mem: &mut NetState, api: &ApiView, now: f64) -> bool {
    match mem.enabled_sent {
        Some((v, at)) if v != api.enabled && now - at < SENT_S => v,
        _ => {
            mem.enabled_sent = None;
            api.enabled
        }
    }
}

/// Where a browser on this computer reaches the API.
fn local_host(api: &ApiView) -> String {
    let ip = api.bind_ip();
    let host = if ip == "0.0.0.0" || ip == "::" { "127.0.0.1".to_owned() } else { ip };
    if host.contains(':') { format!("[{host}]") } else { host }
}

/// «Использовать»: the switch, a moment's indicator while the server opens,
/// «Открыть /docs» (only while it really listens); a bind error under the
/// name.
fn enabled_row(ui: &mut Ui, api: &ApiView, on: bool, mem: &mut NetState, now: f64, out: &mut Vec<Command>) {
    let note = if !api.configurable {
        Some(Note::Blocker("Нет файла настроек"))
    } else if on {
        api.error.as_deref().map(Note::Error)
    } else {
        None
    };
    let listening = on && api.enabled && api.listening.is_some();
    let opening = on && api.configurable && api.listening.is_none() && api.error.is_none();
    row_note(ui, Id::ApiEnabled, note, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let mut v = on;
            ui.add(Switch::new(&mut v).enabled(api.configurable).id_salt(("set-switch", Id::ApiEnabled)));
            if v != on {
                mem.enabled_sent = Some((v, now));
                out.push(Command::SetApiEnabled(v));
            }
            if opening {
                ui.add(LoadingIndicator::new().size(20.0));
            }
            let docs = Button::new("Открыть /docs")
                .icon(Icon::OpenInNew)
                .style(Style::Outlined)
                .size(Size::S)
                .enabled(listening)
                .id_salt("api-open-docs");
            if ui.add(docs).clicked() {
                super::open_url(&format!("http://{}:{}/docs/", local_host(api), api.port));
            }
        });
    });
}

/// «Доступ»: this computer or the home network; an address set in
/// settings.toml is its value.
fn access_row(ui: &mut Ui, api: &ApiView, out: &mut Vec<Command>) {
    let explicit = (!api.address.is_empty()).then_some(api.address.as_str());
    row_note(ui, Id::ApiAccess, explicit.map(Note::Value), |ui| {
        let at = usize::from(!api.local_only());
        let labels = ["Этот компьютер", "Домашняя сеть"];
        if let Some(n) = super::super::connected(ui, egui::Id::new("api-access"), &labels, at, &[]).0
            && n != at
        {
            out.push(Command::SetApiAccess(if n == 0 { ApiAccess::Local } else { ApiAccess::Network }));
        }
    });
}

/// A port typed in, if it is one the API may take.
pub(crate) fn port_of(text: &str) -> Option<u16> {
    text.trim().parse::<u16>().ok().filter(|p| *p >= 1024)
}

/// «Порт»: kept on Enter or when the field is left; «Сбросить» while it is
/// not 47210.
fn port_row(ui: &mut Ui, api: &ApiView, mem: &mut NetState, out: &mut Vec<Command>) {
    if mem.port_from != Some(api.port) && !mem.port_focus {
        mem.port_draft = api.port.to_string();
        mem.port_from = Some(api.port);
    }
    row(ui, Id::ApiPort, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let valid = port_of(&mem.port_draft);
            let mut field = TextField::new(&mut mem.port_draft, "").width(112.0).id_salt("api-port-field");
            if valid.is_none() {
                field = field.error("1024–65535");
            }
            let f = field.show(ui);
            mem.port_focus = f.response.has_focus();
            if (f.submitted || f.response.lost_focus())
                && let Some(p) = valid
                && p != api.port
            {
                out.push(Command::SetApiPort(p));
            }
            if api.port != nikgob_view::API_PORT
                && ui.add(Button::new("Сбросить").style(Style::Text).size(Size::S).id_salt("api-port-reset")).clicked()
            {
                out.push(Command::SetApiPort(nikgob_view::API_PORT));
                mem.port_draft = nikgob_view::API_PORT.to_string();
            }
        });
    });
}

/// A ready token of the split button's menu: its name, preset, who plays
/// with it and the snippet its dialog opens on.
fn ready(i: usize) -> (&'static str, ApiPreset, ApiActor) {
    match i {
        0 => ("Агент на этом ПК", ApiPreset::Full, ApiActor::Agent),
        _ => ("Голосовой ассистент", ApiPreset::Control, ApiActor::Assistant),
    }
}

/// The snippet a dialog opens on: the agent's own for an agent, else curl.
fn first_snippet(actor: ApiActor) -> usize {
    if actor == ApiActor::Agent { 0 } else { 2 }
}

/// «Токены»: «Новый токен», its menu two ready ones.
fn tokens_row(ui: &mut Ui, mem: &mut NetState, now: f64, out: &mut Vec<Command>) {
    row(ui, Id::ApiTokens, |ui| {
        let o = SplitButton::new("api-new-token", "Новый токен")
            .icon(Icon::Add)
            .style(Style::Tonal)
            .size(Size::S)
            .item(MenuItem::new("Для агента на этом ПК"))
            .item(MenuItem::new("Для голосового ассистента"))
            .show(ui);
        if o.main.clicked() {
            mem.dialog = Some(TokenDialog {
                name: String::new(),
                preset: ApiPreset::Control,
                actor: ApiActor::Assistant,
                actor_picked: false,
                focus: true,
                waiting: None,
                error: None,
                made: None,
            });
        }
        if let Some(i) = o.picked {
            let (name, preset, actor) = ready(i);
            mem.dialog = make(name, preset, actor, out).map(|made| TokenDialog {
                name: name.to_owned(),
                preset,
                actor,
                actor_picked: true,
                focus: false,
                waiting: Some(Waiting { made, at: now }),
                error: None,
                made: None,
            });
        }
    });
}

/// A token made now: the runtime keeps its hash, the dialog its secret.
fn make(name: &str, preset: ApiPreset, actor: ApiActor, out: &mut Vec<Command>) -> Option<Made> {
    let Some((secret, token)) = ApiNewToken::make(name, preset, actor) else {
        log::error!("api: no randomness for a token");
        return None;
    };
    let (id, shown) = (token.id.clone(), token.name.clone());
    out.push(Command::ApiCreateToken(token));
    Some(Made { id, name: shown, secret, actor, snippet: first_snippet(actor), lan: lan_ip() })
}

/// Whether the player kept token `id` (it is in the view), refused it,
/// or not yet, `waited` seconds after it was sent.
fn kept(api: &ApiView, id: &str, waited: f64) -> Kept {
    if api.tokens.iter().any(|t| t.id == id) {
        Kept::Yes
    } else if let Some((_, why)) = api.token_failed.as_ref().filter(|(f, _)| f == id) {
        Kept::No(why.clone())
    } else if waited > KEEP_S {
        Kept::No("Плеер не ответил".to_owned())
    } else {
        Kept::NotYet
    }
}

/// This computer's address in the home network: the one a connection out
/// would leave from (a UDP socket only picks its route, nothing is sent).
fn lan_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("10.254.254.254:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then(|| ip.to_string())
}

/// «5 мин назад», «вчера».
pub(crate) fn ago(now_ms: i64, then_ms: i64) -> String {
    let s = (now_ms - then_ms).max(0) / 1000;
    match s {
        0..60 => "только что".to_owned(),
        60..3600 => format!("{} мин назад", s / 60),
        3600..86_400 => format!("{} ч назад", s / 3600),
        86_400..172_800 => "вчера".to_owned(),
        _ => format!("{} дн. назад", s / 86_400),
    }
}

fn actor_icon(a: ApiActor) -> Icon {
    match a {
        ApiActor::Owner => Icon::Person,
        ApiActor::Assistant => Icon::GraphicEq,
        ApiActor::Agent => Icon::AutoAwesome,
        ApiActor::Guest => Icon::Login,
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

/// The tokens that work, a segmented list (nothing while there are none):
/// who plays with it, its name, its rights, when it was last used, and
/// «Отозвать». A use lights its row for a moment.
fn token_list(ui: &mut Ui, api: &ApiView, mem: &mut NetState, now: f64, out: &mut Vec<Command>) {
    let live: Vec<&ApiTokenView> = api.live_tokens().collect();
    // New uses since the last frame: their rows pulse (a token seen for
    // the first time does not).
    for t in &live {
        match mem.seen.insert(t.id.clone(), t.last_used_ms) {
            Some(before) if before != t.last_used_ms => {
                mem.pulses.insert(t.id.clone(), now);
            }
            _ => {}
        }
    }
    mem.seen.retain(|id, _| live.iter().any(|t| &t.id == id));
    mem.pulses.retain(|id, at| now - *at < 2.0 && live.iter().any(|t| &t.id == id));
    if live.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let wall = now_ms();
    ui.add_space(4.0);
    let old = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;
    let n = live.len();
    for (i, t) in live.iter().enumerate() {
        let lit = mem.pulses.get(&t.id).is_some_and(|at| now - at < PULSE_S);
        if let Some(at) = mem.pulses.get(&t.id).filter(|_| lit) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((PULSE_S - (now - at)).max(0.0)));
        }
        let target = if lit { s.secondary_container } else { s.surface_container_low };
        let fill = anim::color(&ctx, egui::Id::new(("api-token-pulse", &t.id)), target, motion.default_effects);
        let mut revoke = false;
        let mut item = ListItem::new(t.name.as_str())
            .leading_icon(actor_icon(t.actor))
            .supporting(format!("{} · {}", preset_label(t.preset), actor_label(t.actor)))
            .position(Position::of(i, n))
            .fill(fill)
            .id_salt(("api-token", &t.id));
        if let Some(at) = t.last_used_ms {
            item = item.trailing_text(ago(wall, at));
        }
        item.trailing(|ui| {
            revoke = ui.add(Button::new("Отозвать").style(Style::Text).size(Size::S).id_salt(("api-revoke", &t.id))).clicked();
        })
        .show(ui);
        if revoke {
            out.push(Command::ApiRevokeToken(t.id.clone()));
        }
    }
    ui.spacing_mut().item_spacing.y = old;
}

/// A string as a JSON string literal.
fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The snippet `kind` ([`SNIPPETS`]) with the real values: the player's
/// program, the token, where the API is.
pub(crate) fn snippet(kind: usize, token: &str, exe: &str, host: &str, port: u16) -> String {
    match kind {
        0 => format!("claude mcp add music -e NIKGOB_TOKEN={token} -- \"{exe}\" mcp"),
        1 => format!(
            "{{\n  \"mcpServers\": {{\n    \"music\": {{\n      \"command\": {},\n      \"args\": [\"mcp\"],\n      \"env\": {{ \"NIKGOB_TOKEN\": {} }}\n    }}\n  }}\n}}",
            json_str(exe),
            json_str(token)
        ),
        _ => format!("curl -H \"Authorization: Bearer {token}\" http://{host}:{port}/v1/state"),
    }
}

/// Where another device reaches the API: this computer's address while it
/// is open to the home network, else this computer only.
fn snippet_host(api: &ApiView, lan: Option<&str>) -> String {
    if api.local_only() {
        return "127.0.0.1".to_owned();
    }
    let ip = api.bind_ip();
    let host = if ip == "0.0.0.0" || ip == "::" { lan.unwrap_or("127.0.0.1").to_owned() } else { ip };
    if host.contains(':') { format!("[{host}]") } else { host }
}

/// The token dialog: the form, then (in the same window) the secret with
/// its snippets.
fn dialog(ui: &mut Ui, api: &ApiView, mem: &mut NetState, now: f64, out: &mut Vec<Command>) {
    let Some(d) = mem.dialog.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    let mut open = true;
    // The secret shows only once the player has kept the token.
    if let Some(w) = d.waiting.as_ref() {
        match kept(api, &w.made.id, now - w.at) {
            Kept::Yes => d.made = d.waiting.take().map(|w| w.made),
            Kept::No(why) => {
                d.waiting = None;
                d.error = Some(why);
            }
            Kept::NotYet => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        }
    }
    if d.made.is_none() {
        let can = !d.name.trim().is_empty() && d.waiting.is_none();
        let waiting = d.waiting.is_some();
        let mut submitted = false;
        let r = Dialog::new("api-token-dialog", "Новый токен")
            .max_width(480.0)
            .action("Отмена")
            .action_with(DialogAction::new("Создать").enabled(can))
            .show_with(&ctx, &mut open, |ui| {
                ui.add_space(16.0);
                let f = TextField::new(&mut d.name, "Название")
                    .placeholder("Ассистент на Pi")
                    .width(ui.available_width())
                    .request_focus(d.focus)
                    .id_salt("api-token-name")
                    .show(ui);
                d.focus = false;
                submitted = f.submitted;
                ui.add_space(16.0);
                ui.label(TypeRole::LabelLarge.style().rich("Права", s.on_surface_variant));
                ui.add_space(8.0);
                let labels = PRESETS.map(preset_label);
                let at = PRESETS.iter().position(|p| *p == d.preset).unwrap_or(0);
                if let Some(n) = super::super::connected(ui, egui::Id::new("api-token-preset"), &labels, at, &[]).0 {
                    d.preset = PRESETS[n];
                    // «Управление» is an assistant's, «Полный» an agent's,
                    // until the user says who.
                    if !d.actor_picked {
                        match d.preset {
                            ApiPreset::Control => d.actor = ApiActor::Assistant,
                            ApiPreset::Full => d.actor = ApiActor::Agent,
                            ApiPreset::Read => {}
                        }
                    }
                }
                ui.add_space(16.0);
                // A token that only reads starts nothing: who plays with
                // it does not matter.
                dimmed(ui, d.preset == ApiPreset::Read, |ui| {
                    ui.label(TypeRole::LabelLarge.style().rich("Кто включает", s.on_surface_variant));
                    ui.add_space(8.0);
                    let labels = ACTORS.map(actor_label);
                    let at = ACTORS.iter().position(|a| *a == d.actor).unwrap_or(0);
                    if let Some(n) = super::super::connected(ui, egui::Id::new("api-token-actor"), &labels, at, &[]).0 {
                        d.actor = ACTORS[n];
                        d.actor_picked = true;
                    }
                });
                if let Some(e) = &d.error {
                    ui.add_space(16.0);
                    ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(e, s.error)).wrap());
                }
                if waiting {
                    ui.add_space(16.0);
                    ui.add(LoadingIndicator::new().size(24.0));
                }
            });
        if (r.action == Some(1) || submitted) && can {
            match make(&d.name, d.preset, d.actor, out) {
                Some(made) => {
                    d.error = None;
                    d.waiting = Some(Waiting { made, at: now });
                }
                None => d.error = Some("Не удалось создать токен".to_owned()),
            }
        } else if r.action.is_some() || r.dismissed || !open {
            mem.dialog = None;
        }
        return;
    }
    // The secret is on screen: a click beside the window does not close
    // it, Esc and «Готово» do.
    let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    let Some(m) = d.made.as_mut() else { return };
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "nikgob".to_owned());
    let host = snippet_host(api, m.lan.as_deref());
    let blocked = matches!(m.actor, ApiActor::Assistant | ApiActor::Guest) && api.local_only();
    let mut open_network = false;
    let r = Dialog::new("api-token-dialog", m.name.clone())
        .max_width(560.0)
        .dismissible(false)
        .action("Готово")
        .show_with(&ctx, &mut open, |ui| {
            ui.add_space(16.0);
            code_block(ui, "api-token-secret", &m.secret, s);
            ui.add_space(16.0);
            if let Some(n) = super::super::connected(ui, egui::Id::new("api-token-snippet"), &SNIPPETS, m.snippet, &[]).0 {
                m.snippet = n;
            }
            ui.add_space(8.0);
            let text = snippet(m.snippet, &m.secret, &exe, &host, api.port);
            code_block(ui, ("api-token-snippet-copy", m.snippet), &text, s);
            if blocked {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(TypeRole::BodyMedium.style().rich("Доступ открыт только на этом компьютере", s.on_surface_variant));
                    open_network = ui
                        .add(Button::new("Включить домашнюю сеть").style(Style::Text).size(Size::S).id_salt("api-open-network"))
                        .clicked();
                });
            }
        });
    if open_network {
        out.push(Command::SetApiAccess(ApiAccess::Network));
    }
    if r.action.is_some() || esc || !open {
        mem.dialog = None;
    }
}

/// Text to copy in a `surface-container-low` box with «Копировать».
fn code_block(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, text: &str, s: crate::ColorScheme) {
    egui::Frame::new().fill(s.surface_container_low).corner_radius(egui::CornerRadius::same(12)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let copy_w = 132.0;
            ui.allocate_ui(vec2((ui.available_width() - copy_w).max(80.0), 0.0), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(text).monospace().size(13.0).color(s.on_surface)).wrap());
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| copy_button(ui, salt, text));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_use_reads_as_time_ago() {
        let now = 10_000_000_000;
        assert_eq!(ago(now, now - 20_000), "только что");
        assert_eq!(ago(now, now - 5 * 60_000), "5 мин назад");
        assert_eq!(ago(now, now - 3 * 3_600_000), "3 ч назад");
        assert_eq!(ago(now, now - 30 * 3_600_000), "вчера");
        assert_eq!(ago(now, now - 5 * 86_400_000), "5 дн. назад");
        assert_eq!(ago(now, now + 5_000), "только что", "a clock a little ahead");
    }

    #[test]
    fn ports() {
        assert_eq!(port_of("47210"), Some(47210));
        assert_eq!(port_of(" 8080 "), Some(8080));
        assert_eq!(port_of("80"), None);
        assert_eq!(port_of("70000"), None);
        assert_eq!(port_of("abc"), None);
    }

    #[test]
    fn snippets_have_the_real_values() {
        let exe = r"C:\Program Files\music.nikgob\nikgob.exe";
        let t = "nk_0123456789abcdefghijABCDEFGHIJkl";
        let claude = snippet(0, t, exe, "127.0.0.1", 47210);
        assert_eq!(claude, format!("claude mcp add music -e NIKGOB_TOKEN={t} -- \"{exe}\" mcp"));
        let json = snippet(1, t, exe, "127.0.0.1", 47210);
        assert!(json.contains(r#""command": "C:\\Program Files\\music.nikgob\\nikgob.exe""#), "{json}");
        assert!(json.contains(&format!("\"NIKGOB_TOKEN\": \"{t}\"")), "{json}");
        let curl = snippet(2, t, exe, "192.168.0.10", 47211);
        assert_eq!(curl, format!("curl -H \"Authorization: Bearer {t}\" http://192.168.0.10:47211/v1/state"));
    }

    #[test]
    fn where_the_snippets_point() {
        let mut api = ApiView::default();
        assert_eq!(snippet_host(&api, Some("192.168.0.10")), "127.0.0.1");
        api.access = ApiAccess::Network;
        assert_eq!(snippet_host(&api, Some("192.168.0.10")), "192.168.0.10");
        assert_eq!(snippet_host(&api, None), "127.0.0.1");
        api.address = "192.168.0.157".into();
        assert_eq!(snippet_host(&api, Some("192.168.0.10")), "192.168.0.157");
        assert_eq!(local_host(&api), "192.168.0.157");
        api.address.clear();
        assert_eq!(local_host(&api), "127.0.0.1");
    }

    /// The secret shows only for a token the player kept; a refusal or no
    /// answer in time is an error.
    #[test]
    fn a_token_is_shown_once_kept() {
        let mut api = ApiView::default();
        assert!(matches!(kept(&api, "a", 0.5), Kept::NotYet));
        assert!(matches!(kept(&api, "a", KEEP_S + 1.0), Kept::No(_)));
        api.token_failed = Some(("a".into(), "Не удалось сохранить настройки".into()));
        assert!(matches!(kept(&api, "a", 0.5), Kept::No(ref why) if why == "Не удалось сохранить настройки"));
        assert!(matches!(kept(&api, "b", 0.5), Kept::NotYet), "another token's failure");
        api.tokens.push(ApiTokenView {
            id: "b".into(),
            name: "B".into(),
            preset: ApiPreset::Full,
            actor: ApiActor::Agent,
            created_ms: 0,
            last_used_ms: None,
            revoked: false,
            hash: String::new(),
        });
        assert!(matches!(kept(&api, "b", 0.5), Kept::Yes));
    }

    #[test]
    fn a_preset_picks_who_plays_until_the_user_does() {
        assert_eq!(first_snippet(ApiActor::Agent), 0);
        assert_eq!(first_snippet(ApiActor::Assistant), 2);
        assert_eq!(ready(0).1, ApiPreset::Full);
        assert_eq!(ready(1), ("Голосовой ассистент", ApiPreset::Control, ApiActor::Assistant));
    }

    /// The tab draws its rows in both modes, the tokens' list only with a
    /// token, and the dialog's secret leaves nothing behind when closed.
    #[test]
    fn the_tab_draws_its_rows() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        let mut state = ViewState::default();
        state.api.configurable = true;
        state.api.tokens.push(ApiTokenView {
            id: "a".into(),
            name: "Ассистент на Pi".into(),
            preset: ApiPreset::Control,
            actor: ApiActor::Assistant,
            created_ms: 0,
            last_used_ms: Some(now_ms() - 60_000),
            revoked: false,
            hash: String::new(),
        });
        let mut pages = PageState::default();
        let mut out = Vec::new();
        for frame in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(900.0, 900.0))),
                time: Some(f64::from(frame) * 0.1),
                ..Default::default()
            };
            let mut done = ctx.run_ui(input, |ui| {
                super::super::rows::set_mode(ui.ctx(), super::super::rows::Mode { deep: true, dev: false, opacity: 1.0, stage: false });
                show(ui, &state, &mut pages, &mut out);
            });
            done.textures_delta.clear();
        }
        let drawn: Vec<Id> = super::super::settings_help::drawn_rows(&ctx).1.into_iter().map(|r| r.id).collect();
        for id in [Id::ApiEnabled, Id::ApiAccess, Id::ApiPort, Id::ApiTokens] {
            assert!(drawn.contains(&id), "{id:?}: {drawn:?}");
        }
        assert!(out.is_empty(), "nothing is sent by drawing: {out:?}");
        // A new use of the token: its row pulses.
        state.api.tokens[0].last_used_ms = Some(now_ms());
        let input = egui::RawInput { time: Some(0.4), ..Default::default() };
        let mut done = ctx.run_ui(input, |ui| show(ui, &state, &mut pages, &mut out));
        done.textures_delta.clear();
        assert!(pages.net.pulses.contains_key("a"));
    }
}
