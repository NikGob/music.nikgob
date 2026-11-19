//! Настройки in one column of Metrolist's settings groups (a title in the
//! primary colour over rows that stand as one grouped card): Вид (the
//! theme, less motion), YouTube Music (this phone's own sign-in), Межсеть
//! (this phone among the owner's devices: its code or a pasted invitation
//! while it is in no ring, the devices once it is, what it keeps with its
//! «Избранное») and the rest of the settings as the desktop draws them.

use egui::{Id, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_core::i18n::{common, errors, mobile, nav};
use nikgob_core::streaming::{Account, LoginInput, Service};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{Button, ConnectedGroup, LoadingIndicator, Menu, MenuItem, Size, Style, Switch, TextField, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Command, LIKED_PLAYLIST, LangPref, Locale, MeshPairing, PrefPatch, Route, ThemeMode, ViewState, kind_label};

use crate::parts::{self, PageOut};
use crate::{PageFrame, detail, text};

const ROW_H: f32 = 56.0;
/// The space between the rows of a group.
const ROW_GAP: f32 = 4.0;
/// A group's ends and its inner corners.
const GROUP_END: f32 = 20.0;
const GROUP_INNER: f32 = 4.0;

/// The corners of row `i` of a group of `n`.
fn row_radii(i: usize, n: usize) -> Radii {
    let top = if i == 0 { GROUP_END } else { GROUP_INNER };
    let bottom = if i + 1 >= n { GROUP_END } else { GROUP_INNER };
    Radii([top, top, bottom, bottom])
}

/// A group's title: the primary colour, over its rows.
fn group_title(ui: &mut Ui, title: &str) {
    let theme = Theme::get(ui.ctx());
    ui.add_space(16.0);
    let style = TypeRole::TitleSmall.emphasized();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), style.line_height + 12.0), Sense::hover());
    text::paint_line(ui, pos2(r.left() + 32.0, r.top()), title, style, theme.scheme.primary, r.width() - 48.0);
}

/// Row `i` of a group of `n`: its card with the page's margins (a press
/// lights it). Returns the card and its response.
fn group_row(ui: &mut Ui, id: Id, i: usize, n: usize, sense: Sense) -> (Rect, egui::Response) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H + ROW_GAP), Sense::hover());
    let card = Rect::from_min_size(pos2(slot.left() + 16.0, slot.top()), vec2(slot.width() - 32.0, ROW_H));
    let resp = ui.interact(card, id, sense);
    let radii = row_radii(i, n);
    paint::container(ui.painter(), card, radii, s.surface_container, Stroke::NONE);
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), card, radii, s.on_surface, layer);
    (card, resp)
}

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
        text::paint_line(ui, pos2(head.left() + 16.0, head.center().y - style.line_height * 0.5), nav().settings, style, s.on_surface, head.width() - 32.0);
        let body = Rect::from_min_max(pos2(rect.left(), head.bottom()), rect.max);
        let mut bui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("phone-settings"));
        bui.set_clip_rect(body.intersect(ui.clip_rect()));
        crate::scroll("phone-settings-scroll", frame.reset).show(&mut bui, |ui| {
            self.look(ui, state, out);
            self.youtube(ui, state, out);
            self.mesh(ui, state, out, &mut page);
            group_title(ui, common().more);
            if link(ui, "set-all", (0, 1), Icon::Settings, mobile().all_settings, None) {
                page.nav = Some(Route::Settings);
            }
            ui.add_space(16.0);
            nikgob_md3e::screens::brand::made_by(ui, ui.max_rect().x_range());
            ui.add_space(24.0);
        });
        page
    }

    fn look(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
        let m = mobile();
        group_title(ui, m.group_look);
        let p = &state.prefs;
        let loc = p.resolved_locale();
        let lang_label = nikgob_core::i18n::settings_in(loc).language_label;
        if let Some(lang) = language_picker(ui, (0, 3), Icon::Tune, lang_label, p.language, loc) {
            out.push(Command::SetPref(PrefPatch::Language(lang)));
        }
        // The theme: a connected group as wide as its row (three chips did
        // not fit a 360 dp phone).
        let (r, _) = group_row(ui, Id::new("set-theme-row"), 1, 3, Sense::hover());
        let inner = r.shrink2(vec2(8.0, 8.0));
        let mut cui = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
        let modes = [ThemeMode::System, ThemeMode::Dark, ThemeMode::Light];
        let at = modes.iter().position(|m| *m == p.theme).unwrap_or(0);
        let w = (inner.width() - 2.0 * 2.0) / 3.0;
        if let Some(i) = ConnectedGroup::new("set-theme", &[m.theme_system, m.theme_dark, m.theme_light]).size(Size::S).equal_width(w).show(&mut cui, at) {
            let mut next = p.clone();
            next.theme = modes[i];
            out.push(Command::SetPrefs(Box::new(next)));
        }
        if let Some(on) = switch(ui, "set-motion", (2, 3), Icon::AutoAwesome, m.reduced_motion, p.reduced_motion) {
            let mut next = p.clone();
            next.reduced_motion = on;
            out.push(Command::SetPrefs(Box::new(next)));
        }
    }

    fn youtube(&mut self, ui: &mut Ui, state: &ViewState, out: &mut Vec<Command>) {
        let Some(acc) = state.account(Service::Youtube) else { return };
        group_title(ui, "YouTube Music");
        match &acc.account {
            Account::LoggedIn { name, .. } => {
                if link(ui, "set-yt-out", (0, 1), Icon::Logout, name, Some(mobile().sign_out)) {
                    out.push(Command::StreamLogout(Service::Youtube));
                }
            }
            Account::LoggingIn(_) => {
                let (r, _) = group_row(ui, Id::new("set-yt-wait"), 0, 1, Sense::hover());
                crate::parts::put(ui, Rect::from_center_size(r.center(), vec2(32.0, 32.0)), LoadingIndicator::new().size(32.0));
            }
            _ => {
                if link(ui, "set-yt-in", (0, 1), Icon::Login, errors().sign_in, None) {
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
        group_title(ui, mobile().group_mesh);
        // The switch, this phone and, in a ring, the owner's other devices
        // as one group.
        let n = if !m.enabled { 1 } else if m.paired { 2 + m.nodes.len() } else { 2 };
        if let Some(on) = switch(ui, "set-mesh", (0, n), Icon::Sync, mobile().my_devices, m.enabled) {
            out.push(Command::MeshSetEnabled(on));
        }
        if m.enabled {
            let theme = Theme::get(ui.ctx());
            let s = theme.scheme;
            let ts = TypeRole::BodyLarge.style();
            let (r, _) = group_row(ui, Id::new("set-this"), 1, n, Sense::hover());
            let c = pos2(r.left() + 12.0 + 20.0, r.center().y);
            phone::device_avatar(ui, c, 40.0, "set-this", m.this.kind, true, state.player.status == nikgob_view::Status::Playing);
            let name = if m.this.name.is_empty() { kind_label(m.this.kind).to_owned() } else { m.this.name.clone() };
            text::paint_line(ui, pos2(c.x + 36.0, r.center().y - ts.line_height * 0.5), name, ts, s.on_surface, r.right() - c.x - 52.0);
            if m.paired {
                for (k, node) in m.nodes.iter().enumerate() {
                    let (r, resp) = group_row(ui, Id::new(("set-node", node.id.as_str())), 2 + k, n, Sense::click());
                    let c = pos2(r.left() + 12.0 + 20.0, r.center().y);
                    let playing = node.online && node.playing.as_ref().is_some_and(|p| !p.paused);
                    phone::device_avatar(ui, c, 40.0, &format!("set-node-{}", node.id), node.kind, node.online, playing);
                    let alpha = if node.online { 1.0 } else { 0.38 };
                    text::paint_line(ui, pos2(c.x + 36.0, r.center().y - ts.line_height * 0.5), node.name.clone(), ts, s.on_surface.gamma_multiply(alpha), r.right() - c.x - 52.0);
                    if resp.clicked() {
                        page.nav = Some(Route::Devices);
                    }
                }
            }
        }
        if let Some(e) = &m.error {
            parts::error(ui, e);
        }
        if !m.enabled {
            return;
        }
        if !m.paired {
            self.pairing(ui, state, out);
            return;
        }
        // What this phone keeps.
        group_title(ui, mobile().group_favorites);
        let pins = &m.this.pins;
        let kept: Vec<&nikgob_view::PlaylistCard> = state.library.playlists.iter().filter(|p| p.id != LIKED_PLAYLIST && !p.pinned).collect();
        let n = 1 + kept.len();
        if let Some(on) = switch(ui, "set-pin-liked", (0, n), Icon::Favorite, nav().liked, pins.liked) {
            let mut p = pins.clone();
            p.liked = on;
            out.push(Command::MeshSetPins { node_id: m.this.id.clone(), pins: p });
        }
        for (k, pl) in kept.into_iter().enumerate() {
            let on = pins.playlists.contains(&pl.id);
            if switch(ui, ("set-pin", pl.id), (1 + k, n), Icon::QueueMusic, &pl.name, on).is_some() {
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
                crate::parts::put(ui, Rect::from_center_size(r.center(), vec2(32.0, 32.0)), LoadingIndicator::new().size(32.0));
                return;
            }
            MeshPairing::Failed(why) => parts::error(ui, why),
            _ => {}
        }
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
        let mut fui = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(16.0, 0.0))));
        TextField::new(&mut self.ticket, mobile().invitation).leading_icon(Icon::Link).width(r.width() - 32.0).id_salt("set-ticket").show(&mut fui);
        let ticket = ticket_of(&self.ticket);
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
        let mut bui = ui.new_child(UiBuilder::new().max_rect(r.shrink2(vec2(16.0, 0.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
        if bui.add(Button::new(mobile().connect).icon(Icon::Link).style(Style::Filled).size(Size::S).enabled(ticket.is_some()).id_salt("set-join")).clicked()
            && let Some(t) = ticket
        {
            out.push(Command::MeshJoin { ticket: t });
            self.ticket.clear();
        }
    }
}

/// A row of a group (`pos`: its place and the group's length) with a switch
/// at its end; the new value when it was flipped.
fn switch(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, pos: (usize, usize), icon: Icon, label: &str, value: bool) -> Option<bool> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let id = Id::new(&salt);
    let (r, resp) = group_row(ui, id, pos.0, pos.1, Sense::click());
    icon.widget(false, s.on_surface_variant, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.left() + 16.0 + 12.0, r.center().y), vec2(24.0, 24.0)));
    let ts = TypeRole::BodyLarge.style();
    text::paint_line(ui, pos2(r.left() + 56.0, r.center().y - ts.line_height * 0.5), label, ts, s.on_surface, r.width() - 56.0 - 80.0);
    let mut v = value;
    let sw = Rect::from_min_size(pos2(r.right() - 16.0 - 52.0, r.center().y - 16.0), vec2(52.0, 32.0));
    crate::parts::put(ui, sw, Switch::new(&mut v).id_salt(id.with("sw")));
    let flipped = v != value;
    if flipped {
        Some(v)
    } else if resp.clicked() {
        Some(!value)
    } else {
        None
    }
}

/// A row of a group (`pos`: its place and the group's length) that opens a
/// menu to pick a [`LangPref`], showing the current choice at its end.
fn language_picker(ui: &mut Ui, pos: (usize, usize), icon: Icon, label: &str, current: LangPref, loc: Locale) -> Option<LangPref> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let id = Id::new("set-lang-row");
    let (r, resp) = group_row(ui, id, pos.0, pos.1, Sense::click());
    icon.widget(false, s.on_surface_variant, 24.0)
        .paint_at(ui, Rect::from_center_size(pos2(r.left() + 16.0 + 12.0, r.center().y), vec2(24.0, 24.0)));
    let ts = TypeRole::BodyLarge.style();
    let chevron = Rect::from_center_size(pos2(r.right() - 16.0 - 9.0, r.center().y), vec2(18.0, 18.0));
    Icon::ExpandMore.widget(false, s.on_surface_variant, 18.0).paint_at(ui, chevron);
    let ls = TypeRole::LabelLarge.style();
    let g = text::line(ui, current.label_in(loc), ls, s.primary, 140.0);
    let val_x = chevron.left() - 6.0 - g.size().x;
    ui.painter().galley(pos2(val_x, r.center().y - g.size().y * 0.5), g, s.primary);
    let right = val_x - 12.0;
    text::paint_line(ui, pos2(r.left() + 56.0, r.center().y - ts.line_height * 0.5), label, ts, s.on_surface, right - r.left() - 56.0);
    let mut menu = Menu::new("set-lang-menu").align_right(true);
    for &opt in &LangPref::ALL {
        menu = menu.item(MenuItem::new(opt.label_in(loc)).selected(opt == current));
    }
    menu.attach(&resp).and_then(|i| LangPref::ALL.get(i).copied()).filter(|&lang| lang != current)
}

/// A row of a group that leads somewhere (or does something): its icon,
/// its name, a value at its end. True when tapped.
fn link(ui: &mut Ui, salt: &str, pos: (usize, usize), icon: Icon, label: &str, value: Option<&str>) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let id = Id::new(salt);
    let (r, resp) = group_row(ui, id, pos.0, pos.1, Sense::click());
    icon.widget(false, s.on_surface_variant, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.left() + 16.0 + 12.0, r.center().y), vec2(24.0, 24.0)));
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
    fn a_group_is_round_at_its_ends() {
        assert_eq!(row_radii(0, 3).0, [GROUP_END, GROUP_END, GROUP_INNER, GROUP_INNER]);
        assert_eq!(row_radii(1, 3).0, [GROUP_INNER; 4]);
        assert_eq!(row_radii(2, 3).0, [GROUP_INNER, GROUP_INNER, GROUP_END, GROUP_END]);
        assert_eq!(row_radii(0, 2).0, [GROUP_END, GROUP_END, GROUP_INNER, GROUP_INNER]);
        assert_eq!(row_radii(1, 2).0, [GROUP_INNER, GROUP_INNER, GROUP_END, GROUP_END]);
        assert_eq!(row_radii(0, 1).0, [GROUP_END; 4]);
    }

    #[test]
    fn language_picker_opens_menu_and_emits_patch() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        nikgob_md3e::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        nikgob_md3e::icons::install(&ctx);
        Theme::default().install(&ctx);

        let mut settings = Settings::default();
        let mut state = ViewState::default();
        state.prefs.language = LangPref::Ru;
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 800.0));
        let frame = PageFrame { rect: screen, top: 0.0, reset: false };
        let mut t = 0.0;
        let mut step = |events: Vec<egui::Event>, state: &ViewState| -> (Vec<Command>, Option<Vec<Rect>>) {
            t += 0.1;
            let mut out = Vec::new();
            let mut rects = None;
            let mut done = ctx.run_ui(
                egui::RawInput { screen_rect: Some(screen), time: Some(t), events, ..Default::default() },
                |root| {
                    egui::CentralPanel::default().show(root, |ui| {
                        settings.show(ui, frame, state, &mut out);
                    });
                    rects = Menu::item_rects(root, "set-lang-menu");
                },
            );
            done.textures_delta.clear();
            (out, rects)
        };

        // Initial render.
        step(Vec::new(), &state);
        let row_rect = ctx.read_response(Id::new("set-lang-row")).expect("language row").rect;
        let tap_row = row_rect.center();
        let click = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Tap row to open menu.
        step(vec![egui::Event::PointerMoved(tap_row)], &state);
        step(Vec::new(), &state);
        step(vec![click(tap_row, true)], &state);
        step(vec![click(tap_row, false)], &state);
        let (_, rects) = step(Vec::new(), &state);
        let item_rects = rects.expect("open language menu");
        assert_eq!(item_rects.len(), LangPref::ALL.len());
        let en_idx = LangPref::ALL.iter().position(|l| *l == LangPref::En).expect("En option");
        let pick_pos = item_rects[en_idx].center();
        step(vec![egui::Event::PointerMoved(pick_pos)], &state);
        step(Vec::new(), &state);
        let (mut out, _) = step(vec![click(pick_pos, true)], &state);
        out.extend(step(vec![click(pick_pos, false)], &state).0);
        out.extend(step(Vec::new(), &state).0);
        assert!(out.contains(&Command::SetPref(PrefPatch::Language(LangPref::En))), "{out:?}");
    }

    #[test]
    fn the_ticket_is_found_in_what_was_pasted() {
        assert_eq!(ticket_of("nkm1ABCDEFGH"), Some("nkm1ABCDEFGH".to_owned()));
        assert_eq!(ticket_of("curl … | sh -s -- nkm1QWERTYUI12 "), Some("nkm1QWERTYUI12".to_owned()));
        assert_eq!(ticket_of("\"nkm1ZZZZZZZZZ\""), Some("nkm1ZZZZZZZZZ".to_owned()));
        assert_eq!(ticket_of("hello"), None);
        assert_eq!(ticket_of("nkm1"), None);
    }
}
