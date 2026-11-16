//! Настройки in one column: Вид (the theme, less motion), YouTube Music
//! (this phone's own sign-in), Межсеть (this phone among the owner's
//! devices: its code or a pasted invitation while it is in no ring, the
//! devices once it is, what it keeps with its «Избранное») and the rest of
//! the settings as the desktop draws them.

use egui::{Id, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_core::streaming::{Account, LoginInput, Service};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{Button, FilterChip, LoadingIndicator, Size, Style, Switch, TextField, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Command, LIKED_PLAYLIST, MeshPairing, Route, ThemeMode, ViewState, kind_label};

use crate::parts::{self, PageOut};
use crate::{PageFrame, detail, text};

const ROW_H: f32 = 56.0;

#[derive(Default)]
pub struct Settings {
    ticket: String,
}

/// An invitation as pasted: the `nkm1…` ticket inside whatever came with
/// it (a line of a message, the install command), if there is one.
pub fn ticket_of(pasted: &str) -> Option<String> {
    pasted
        .split(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .map(str::trim)
        .find(|w| w.len() > 8 && w.starts_with("nkm1"))
        .map(str::to_owned)
}

impl Settings {
    pub fn show(&mut self, ui: &mut Ui, frame: PageFrame, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
        let mut page = PageOut::default();
        let rect = frame.rect;
        let theme = Theme::get(ui.ctx());
        let s = theme.scheme;
        let head = Rect::from_min_size(pos2(rect.left(), rect.top() + frame.top), vec2(rect.width(), parts::BAR_H));
        let style = TypeRole::TitleLarge.style();
        text::paint_line(ui, pos2(head.left() + 16.0, head.center().y - style.line_height * 0.5), "Настройки", style, s.on_surface, head.width() - 32.0);
        let body = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max);
        let mut bui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("phone-settings"));
        bui.set_clip_rect(body.intersect(ui.clip_rect()));
        crate::scroll("phone-settings-scroll", frame.reset).show(&mut bui, |ui| {
            self.look(ui, state, out);
            self.youtube(ui, state, out);
            self.mesh(ui, state, out, &mut page);
            parts::section(ui, "Ещё");
            if link(ui, "set-all", Icon::Settings, "Все настройки", None) {
                page.nav = Some(Route::Settings);
            }
            ui.add_space(24.0);
        });
        page
    }

    fn look(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
        parts::section(ui, "Вид");
        let p = &state.prefs;
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
        let mut cui = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(16.0, 8.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        cui.spacing_mut().item_spacing.x = 8.0;
        for (mode, label) in [(ThemeMode::System, "Как в системе"), (ThemeMode::Dark, "Тёмная"), (ThemeMode::Light, "Светлая")] {
            if cui.add(FilterChip::new(label, p.theme == mode).one_of().id_salt(("set-theme", label))).clicked() && p.theme != mode {
                let mut next = p.clone();
                next.theme = mode;
                out.push(Command::SetPrefs(Box::new(next)));
            }
        }
        if let Some(on) = switch(ui, "set-motion", Icon::AutoAwesome, "Меньше движения", p.reduced_motion) {
            let mut next = p.clone();
            next.reduced_motion = on;
            out.push(Command::SetPrefs(Box::new(next)));
        }
    }

    fn youtube(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
        let Some(acc) = state.account(Service::Youtube) else { return };
        parts::section(ui, "YouTube Music");
        match &acc.account {
            Account::LoggedIn { name, .. } => {
                if link(ui, "set-yt-out", Icon::Logout, name, Some("Выйти")) {
                    out.push(Command::StreamLogout(Service::Youtube));
                }
            }
            Account::LoggingIn(_) => {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                ui.put(Rect::from_center_size(r.center(), vec2(32.0, 32.0)), LoadingIndicator::new().size(32.0));
            }
            _ => {
                if link(ui, "set-yt-in", Icon::Login, "Войти", None) {
                    out.push(Command::StreamLogin { service: Service::Youtube, input: LoginInput::Browser });
                }
            }
        }
        if let Some(e) = &acc.error {
            parts::error(ui, e);
        }
    }

    fn mesh(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>, page: &mut PageOut) {
        let m = &state.mesh;
        parts::section(ui, "Межсеть");
        if let Some(on) = switch(ui, "set-mesh", Icon::Sync, "Мои устройства", m.enabled) {
            out.push(Command::MeshSetEnabled(on));
        }
        if let Some(e) = &m.error {
            parts::error(ui, e);
        }
        if !m.enabled {
            return;
        }
        let theme = Theme::get(ui.ctx());
        let s = theme.scheme;
        // This phone.
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::hover());
        let c = pos2(r.left() + 16.0 + 20.0, r.center().y);
        phone::device_avatar(ui, c, 40.0, "set-this", m.this.kind, true, state.player.status == nikgob_view::Status::Playing);
        let name = if m.this.name.is_empty() { kind_label(m.this.kind).to_owned() } else { m.this.name.clone() };
        let ts = TypeRole::BodyLarge.style();
        text::paint_line(ui, pos2(c.x + 36.0, r.center().y - ts.line_height * 0.5), name, ts, s.on_surface, r.right() - c.x - 52.0);
        if !m.paired {
            self.pairing(ui, state, out);
            return;
        }
        // The owner's other devices.
        for n in &m.nodes {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
            let c = pos2(r.left() + 16.0 + 20.0, r.center().y);
            let playing = n.online && n.playing.as_ref().is_some_and(|p| !p.paused);
            phone::device_avatar(ui, c, 40.0, &format!("set-node-{}", n.id), n.kind, n.online, playing);
            let alpha = if n.online { 1.0 } else { 0.38 };
            text::paint_line(ui, pos2(c.x + 36.0, r.center().y - ts.line_height * 0.5), n.name.clone(), ts, s.on_surface.gamma_multiply(alpha), r.right() - c.x - 52.0);
            if resp.clicked() {
                page.nav = Some(Route::Devices);
            }
        }
        // What this phone keeps.
        parts::section(ui, "Избранное");
        let pins = &m.this.pins;
        if let Some(on) = switch(ui, "set-pin-liked", Icon::Favorite, "Любимые", pins.liked) {
            let mut p = pins.clone();
            p.liked = on;
            out.push(Command::MeshSetPins { node_id: m.this.id.clone(), pins: p });
        }
        for pl in state.library.playlists.iter().filter(|p| p.id != LIKED_PLAYLIST && !p.pinned) {
            let on = pins.playlists.contains(&pl.id);
            if switch(ui, ("set-pin", pl.id), Icon::QueueMusic, &pl.name, on).is_some() {
                out.push(Command::MeshSetPins { node_id: m.this.id.clone(), pins: detail::toggle_pin(pins, pl.id) });
            }
        }
    }

    /// In no ring yet: this phone's code for «Подключить» on the computer,
    /// or the computer's invitation pasted here.
    fn pairing(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
        let m = &state.mesh;
        let theme = Theme::get(ui.ctx());
        let s = theme.scheme;
        if let Some(code) = &m.code {
            let spaced: String = code.chars().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
            let g = text::centred(ui, spaced, TypeRole::DisplaySmall.style(), s.primary, ui.available_width() - 32.0, 1);
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 24.0), Sense::hover());
            let card = r.shrink2(vec2(16.0, 0.0));
            paint::container(ui.painter(), card, Radii::all(24.0), s.surface_container_high, Stroke::NONE);
            ui.painter().galley(pos2(card.center().x, card.top() + 12.0), g, s.primary);
            ui.add_space(12.0);
        }
        match &m.pairing {
            MeshPairing::Working => {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                ui.put(Rect::from_center_size(r.center(), vec2(32.0, 32.0)), LoadingIndicator::new().size(32.0));
                return;
            }
            MeshPairing::Failed(why) => parts::error(ui, why),
            _ => {}
        }
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
        let mut fui = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(16.0, 0.0))));
        TextField::new(&mut self.ticket, "Приглашение").leading_icon(Icon::Link).width(r.width() - 32.0).id_salt("set-ticket").show(&mut fui);
        let ticket = ticket_of(&self.ticket);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
        let mut bui = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(16.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if bui.add(Button::new("Подключить").icon(Icon::Link).style(Style::Filled).size(Size::S).enabled(ticket.is_some()).id_salt("set-join")).clicked()
            && let Some(t) = ticket
        {
            out.push(Command::MeshJoin { ticket: t });
            self.ticket.clear();
        }
    }
}

/// A row with a switch at its end; the new value when it was flipped.
fn switch(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, icon: Icon, label: &str, value: bool) -> Option<bool> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let id = Id::new(&salt);
    let resp = ui.interact(r, id, Sense::click());
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), r, Radii::all(0.0), s.on_surface, layer);
    icon.widget(false, s.on_surface_variant, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.left() + 28.0, r.center().y), vec2(24.0, 24.0)));
    let ts = TypeRole::BodyLarge.style();
    text::paint_line(ui, pos2(r.left() + 56.0, r.center().y - ts.line_height * 0.5), label, ts, s.on_surface, r.width() - 56.0 - 80.0);
    let mut v = value;
    let sw = Rect::from_min_size(pos2(r.right() - 16.0 - 52.0, r.center().y - 16.0), vec2(52.0, 32.0));
    ui.put(sw, Switch::new(&mut v).id_salt(id.with("sw")));
    let flipped = v != value;
    if flipped {
        Some(v)
    } else if resp.clicked() {
        Some(!value)
    } else {
        None
    }
}

/// A row that leads somewhere (or does something): its icon, its name,
/// a value at its end. True when tapped.
fn link(ui: &mut Ui, salt: &str, icon: Icon, label: &str, value: Option<&str>) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let id = Id::new(salt);
    let resp = ui.interact(r, id, Sense::click());
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), r, Radii::all(0.0), s.on_surface, layer);
    icon.widget(false, s.on_surface_variant, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.left() + 28.0, r.center().y), vec2(24.0, 24.0)));
    let ts = TypeRole::BodyLarge.style();
    let mut right = r.right() - 16.0;
    if let Some(v) = value {
        let ls = TypeRole::LabelLarge.style();
        let g = text::line(ui, v, ls, s.primary, 140.0);
        right -= g.size().x;
        ui.painter().galley(pos2(right, r.center().y - g.size().y * 0.5), g, s.primary);
        right -= 12.0;
    }
    text::paint_line(ui, pos2(r.left() + 56.0, r.center().y - ts.line_height * 0.5), label, ts, s.on_surface, right - r.left() - 56.0);
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ticket_is_found_in_what_was_pasted() {
        assert_eq!(ticket_of("nkm1ABCDEFGH"), Some("nkm1ABCDEFGH".to_owned()));
        assert_eq!(ticket_of("curl … | sh -s -- nkm1QWERTYUI12 "), Some("nkm1QWERTYUI12".to_owned()));
        assert_eq!(ticket_of("\"nkm1ZZZZZZZZZ\""), Some("nkm1ZZZZZZZZZ".to_owned()));
        assert_eq!(ticket_of("hello"), None);
        assert_eq!(ticket_of("nkm1"), None);
    }
}
