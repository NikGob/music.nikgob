//! «Где играть», like Spotify Connect: the output button beside the volume
//! once the ring has another device (out of reach too, so it does not come
//! and go), the small pill it grows into while another device plays (this
//! window is that device's remote: the kind's icon and the device's name
//! on the play button's colours), and the popover a click opens — this
//! computer with its sound devices under a chevron, the paired devices
//! with what plays there, the volume of the one that plays, «Добавить» and
//! «Карта сети». A middle click on the button opens «Карта сети».
//!
//! A click on a free device moves what plays here there (a transfer); on a
//! device that plays its own music it only makes this window its remote
//! (an attach), and ⇄ in its row moves this queue over it.
//!
//! No word says a state: the pill's colour and name say where the music
//! is, a link that is lost fades the pill, a link that comes back shows
//! the player's loading indicator.

use std::sync::Arc;

use egui::{Align, Area, Color32, Context, Id, Key, Layout, Modifiers, Order, Rect, Sense, Stroke, Ui, UiBuilder, emath::TSTransform, pos2, vec2};
use nikgob_view::{Command, MeshKind, MeshNodeView, MeshScope, RemoteHow, RemoteLink, Route, Status, ViewState};

use super::chrome::Chrome;
use super::pages::mesh_ui::{self, Look as Avatar};
use crate::{
    Elevation, Icon, Theme, TypeRole, TypeStyle,
    components::{Button, IconButton, Size, Slider, Style, anim, paint, paint::Radii, tooltip},
    tokens::{shape, state},
};

/// The pill's icon, the room between it and the name, the widest name.
const PILL_ICON: f32 = 14.0;
const PILL_GAP: f32 = 4.0;
const NAME_MAX: f32 = 88.0;
/// A link lost: the pill fades to this.
const LOST_ALPHA: f32 = 0.55;

const POPOVER_W: f32 = 320.0;
const PAD: f32 = 8.0;
/// A place's row (one height for all, the list's two-line row), the room
/// inside its container on either side, its avatar, the start of its text.
const ROW_H: f32 = 72.0;
const SIDE: f32 = 12.0;
const AVATAR: f32 = 40.0;
const TEXT_X: f32 = SIDE + AVATAR + 12.0;
/// The headline's and the supporting line's boxes (body-large, body-medium)
/// and the glyph before what plays.
const HEADLINE_H: f32 = 24.0;
const SUPPORT_H: f32 = 20.0;
const SUPPORT_ICON: f32 = 16.0;
/// Between two places.
const GAP: f32 = 2.0;
/// A sound device of this computer under its row.
const SUB_H: f32 = 40.0;
/// The volume under the place that plays.
const VOLUME_H: f32 = 48.0;
/// «Добавить» · «Карта сети».
const FOOT_GAP: f32 = 4.0;
const FOOT_H: f32 = 40.0;
/// The name of this computer's sound device when none is chosen.
const SYSTEM_DEVICE: &str = "Системное по умолчанию";

/// There is somewhere else to play: another device in the ring, or this
/// window drives one now.
pub(crate) fn available(state: &ViewState) -> bool {
    state.target.is_some() || state.mesh.device_count() >= 2
}

/// The remote's state comes late: the player shows its loading indicator.
pub(crate) fn reconnecting(state: &ViewState) -> bool {
    state.target.as_ref().is_some_and(|t| t.link == RemoteLink::Reconnecting)
}

/// Nothing from the remote for a while: its music is not shown as playing.
pub(crate) fn lost(state: &ViewState) -> bool {
    state.target.as_ref().is_some_and(|t| t.link == RemoteLink::Lost)
}

/// The speaker of a volume surface away from the output button (the
/// bubble, Now Playing's pill, «Громкость…»): while another device plays,
/// the icon of its kind, whose volume it sets (crossed out when muted, as
/// always).
pub(crate) fn volume_icon(state: &ViewState, level: f32, muted: bool) -> Icon {
    match &state.target {
        Some(t) if !muted && level > 0.0 => mesh_ui::kind_icon(t.kind),
        _ => Icon::volume(level, muted),
    }
}

/// How the button sits where it is drawn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Look {
    /// The round button's side while this device plays; 0: none, the pill
    /// alone (it grows from nothing).
    pub circle: f32,
    /// The pill's height while another device plays.
    pub pill: f32,
    /// The round button's glyph.
    pub content: Color32,
    /// The pill's container and content (the play button's pair).
    pub accent: (Color32, Color32),
    /// The pill shows the kind's icon alone (the name in its tooltip).
    pub collapsed: bool,
}

/// Where the button is this frame.
struct Frame {
    /// In (1) or gone (0), a little over on the spring.
    show: f32,
    /// The round button (0) or the pill (1).
    pill: f32,
    /// The device the pill names (the last one while it leaves).
    name: String,
    kind: MeshKind,
}

fn frame(ctx: &Context, state: &ViewState, salt: &str, wanted: bool) -> Frame {
    let id = Id::new(("md3e-output", salt));
    let motion = Theme::get(ctx).motion;
    let target = state.target.as_ref();
    let (name, kind) = ctx.data_mut(|d| {
        let last = d.get_temp_mut_or_insert_with(id.with("last"), || (String::new(), MeshKind::Pc));
        if let Some(t) = target {
            if last.0 != t.name {
                last.0.clone_from(&t.name);
            }
            last.1 = t.kind;
        }
        last.clone()
    });
    let show = anim::f32(ctx, id.with("show"), if wanted { 1.0 } else { 0.0 }, motion.fast_spatial).max(0.0);
    let pill = anim::f32(ctx, id.with("pill"), if target.is_some() { 1.0 } else { 0.0 }, motion.fast_spatial);
    Frame { show, pill, name, kind }
}

/// The device's name in the pill: label-small emphasized, cut at 88.
fn name_galley(ui: &Ui, name: &str, color: Color32) -> Arc<egui::Galley> {
    let mut job = TypeRole::LabelSmall.emphasized().job(name, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(NAME_MAX);
    crate::typography::layout(ui.painter(), job)
}

/// The pill's padding on the icon's side (the name's side has 2 more).
fn pill_pad(look: Look) -> f32 {
    (look.pill - PILL_ICON) / 2.0 + 2.0
}

fn pill_width(ui: &Ui, f: &Frame, look: Look) -> f32 {
    if look.collapsed || f.name.is_empty() {
        return look.pill;
    }
    let pad = pill_pad(look);
    pad + PILL_ICON + PILL_GAP + name_galley(ui, &f.name, Color32::WHITE).size().x + pad + 2.0
}

/// The width between the round button and the pill, at the spring's place.
fn width_at(ui: &Ui, f: &Frame, look: Look) -> f32 {
    let from = if look.circle > 0.0 { look.circle } else { look.pill };
    from + (pill_width(ui, f, look) - from) * f.pill
}

/// The room the button takes in its row now (it grows in and out on a
/// spring, and from the round button into the pill): what a row lays out
/// around it, 0 once it is gone. `wanted`: it belongs in the row now.
pub(crate) fn slot_width(ui: &Ui, state: &ViewState, salt: &str, look: Look, wanted: bool) -> f32 {
    let f = frame(ui.ctx(), state, salt, wanted);
    if f.show <= 0.001 { 0.0 } else { width_at(ui, &f, look) * f.show }
}

/// Draws the button (or the pill) where `ui` lays it out: a click opens the
/// popover out of it, a middle click opens «Карта сети». Nothing once it
/// has gone ([`slot_width`]).
pub(crate) fn button(ui: &mut Ui, state: &ViewState, salt: &str, look: Look, wanted: bool, chrome: &mut Chrome) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let f = frame(&ctx, state, salt, wanted);
    if f.show <= 0.001 {
        return;
    }
    let id = Id::new(("md3e-output", salt));
    let full = width_at(ui, &f, look);
    let (slot, resp) = ui.allocate_exact_size(vec2(full * f.show, look.circle.max(look.pill)), Sense::click());
    // Grows from its centre; between the round button and the pill the
    // height follows the same spring as the width.
    let scale = f.show.clamp(0.0, 1.2);
    let k = f.pill;
    let kc = k.clamp(0.0, 1.0);
    let from_h = if look.circle > 0.0 { look.circle } else { look.pill };
    let h = (from_h + (look.pill - from_h) * k).max(0.0) * scale.min(1.0);
    let rect = Rect::from_center_size(slot.center(), vec2(full * scale, h));
    let radii = Radii::all(rect.height() * 0.5);
    let painter = ui.painter_at(slot.expand(4.0));
    let fade = anim::fraction(&ctx, id.with("lost"), if lost(state) { LOST_ALPHA } else { 1.0 }, theme.motion.fast_effects);
    let (bg, fg) = look.accent;
    paint::container(&painter, rect, radii, bg.gamma_multiply(kc * fade), Stroke::NONE);
    let target_layer = if !wanted {
        0.0
    } else if resp.is_pointer_button_down_on() {
        state::PRESSED
    } else if resp.hovered() {
        state::HOVER
    } else {
        0.0
    };
    let layer = anim::fraction(&ctx, id.with("layer"), target_layer, theme.motion.fast_effects);
    if layer > 0.001 {
        paint::state_layer(&painter, rect, radii, lerp_color(look.content, fg, kc), layer);
    }
    // The round button's glyph leaves as the pill comes.
    let round = (1.0 - kc * 1.6).clamp(0.0, 1.0);
    if look.circle > 0.0 && round > 0.001 && scale > 0.3 {
        let g = 24.0 * scale.min(1.0);
        Icon::SpeakerGroup.widget(false, look.content.gamma_multiply(round), g).paint_at(ui, Rect::from_center_size(rect.center(), vec2(g, g)));
    }
    let inside = ((kc - 0.35) / 0.65).clamp(0.0, 1.0) * fade;
    if inside > 0.001 && scale > 0.5 {
        let c = fg.gamma_multiply(inside);
        let icon = mesh_ui::kind_icon(f.kind).widget(true, c, PILL_ICON);
        if look.collapsed || f.name.is_empty() {
            icon.paint_at(ui, Rect::from_center_size(rect.center(), vec2(PILL_ICON, PILL_ICON)));
        } else {
            let text = name_galley(ui, &f.name, c);
            let left = rect.center().x - (PILL_ICON + PILL_GAP + text.size().x) / 2.0;
            let at = Rect::from_min_size(pos2(left, rect.center().y - PILL_ICON / 2.0), vec2(PILL_ICON, PILL_ICON));
            icon.paint_at(ui, at);
            painter.galley(pos2(at.right() + PILL_GAP, rect.center().y - text.size().y / 2.0), text, c);
        }
    }
    chrome.output.note(id, slot);
    if !wanted {
        return;
    }
    let tip = if look.collapsed && state.target.is_some() && !f.name.is_empty() { f.name.clone() } else { "Где играть".to_owned() };
    let resp = tooltip(resp, &tip, None);
    if resp.middle_clicked() {
        chrome.output.close();
        chrome.nav = Some(Route::Devices);
    } else if resp.clicked() {
        chrome.output.toggle(id, slot);
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    let ch = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(ch(0), ch(1), ch(2), ch(3))
}

/// `c` with no alpha: what a colour spring starts from without passing
/// through black.
fn clear(c: Color32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, 0)
}

/// What the popover keeps between frames.
#[derive(Default)]
pub(crate) struct OutputUi {
    open: bool,
    /// The control it opened from, and where that is (noted each frame it
    /// is drawn).
    from: Option<Id>,
    anchor: Option<Rect>,
    /// The popover as drawn last frame (a press inside keeps it).
    rect: Option<Rect>,
    /// «Этот компьютер» shows its sound devices.
    devices: bool,
    /// The last output button drawn (scripted screenshots open it).
    last_button: Option<Rect>,
    /// «Добавить»: the window to open, out of this rect (the shell opens
    /// it, the devices' window lives with the pages).
    pub(crate) add_device: Option<Rect>,
}

impl OutputUi {
    fn note(&mut self, id: Id, rect: Rect) {
        self.last_button = Some(rect);
        if self.from == Some(id) {
            self.anchor = Some(rect);
        }
    }

    fn toggle(&mut self, id: Id, rect: Rect) {
        if self.open && self.from == Some(id) {
            self.close();
        } else {
            self.open_at(id, rect);
        }
    }

    /// Opens the popover out of `rect` (the control `id`).
    pub(crate) fn open_at(&mut self, id: Id, rect: Rect) {
        if !self.open {
            self.devices = false;
        }
        self.open = true;
        self.from = Some(id);
        self.anchor = Some(rect);
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// Opens the popover out of the output button drawn last, else out of
    /// `fallback` (scripted screenshots).
    pub(crate) fn open_from_last(&mut self, fallback: Rect) {
        let rect = self.last_button.unwrap_or(fallback);
        self.open_at(Id::new("md3e-output-script"), rect);
    }
}

/// What going back to this computer does while another device plays: back
/// to this computer's own queue after an attach (nothing moves), that
/// device's music brought here after a transfer. `None` while this computer
/// plays.
pub(crate) fn back_here(state: &ViewState) -> Option<Command> {
    state.target.as_ref().map(|t| match t.how {
        RemoteHow::Attached => Command::OutputDetach,
        RemoteHow::Transferred => Command::OutputTransfer { node_id: None },
    })
}

/// The second line of a place.
#[derive(Clone, Debug, PartialEq)]
enum Second {
    /// What plays there, or waits paused: «Артист — Трек».
    Playing { line: String, paused: bool },
    /// This computer's sound device, when another than the system's.
    Device(String),
}

/// One place the music can play, as the popover shows it.
#[derive(Clone, Debug, PartialEq)]
struct Place {
    /// `None`: this computer.
    node: Option<String>,
    name: String,
    kind: MeshKind,
    avatar: Avatar,
    guest: bool,
    second: Option<Second>,
    /// The music this window shows plays there.
    selected: bool,
    /// Out of reach: shown, not chosen.
    enabled: bool,
    /// It plays its own music (a click attaches).
    plays: bool,
    /// ⇄ in its row.
    swap: bool,
}

/// «Артист — Трек».
fn track_line(artist: &str, title: &str) -> String {
    if artist.trim().is_empty() { title.to_owned() } else { format!("{artist} — {title}") }
}

/// This computer, then the paired devices (those in reach first, each
/// group in the ring's order).
fn places(state: &ViewState) -> Vec<Place> {
    let target = state.target.as_ref();
    // The window drives a device's own music: this computer's queue waits
    // parked, and ⇄ beside each can move the one over the other.
    let attached = target.is_some_and(|t| t.how == RemoteHow::Attached);
    let parked = state.parked.as_ref().filter(|_| target.is_some()).and_then(|p| p.current.as_ref());
    let second = match parked {
        Some(t) => Some(Second::Playing { line: track_line(&t.artist, &t.title), paused: true }),
        None => state.prefs.output_device.clone().map(Second::Device),
    };
    let mut out = vec![Place {
        node: None,
        name: "Этот компьютер".to_owned(),
        kind: MeshKind::Pc,
        avatar: Avatar { online: true, playing: target.is_none() && state.player.status == Status::Playing },
        guest: false,
        second,
        selected: target.is_none(),
        enabled: true,
        plays: false,
        swap: attached,
    }];
    let mut nodes: Vec<&MeshNodeView> = state.mesh.nodes.iter().collect();
    nodes.sort_by_key(|n| !n.online);
    for n in nodes {
        let is_target = target.is_some_and(|t| t.node_id == n.id);
        let plays = n.online && n.playing.as_ref().is_some_and(|p| !p.paused);
        let second = n.playing.as_ref().filter(|_| n.online).map(|p| Second::Playing { line: track_line(&p.artists.join(", "), &p.title), paused: p.paused });
        out.push(Place {
            node: Some(n.id.clone()),
            name: n.name.clone(),
            kind: n.kind,
            avatar: mesh_ui::node_look(n),
            guest: n.scope == MeshScope::Guest,
            second,
            selected: is_target,
            enabled: n.online || is_target,
            plays,
            // Over what the window drives already only when it came there
            // by an attach (this queue is not there).
            swap: plays && (!is_target || attached),
        });
    }
    out
}

/// A click on a place's row (`None`: nothing to send). This computer while
/// another device plays: [`back_here`]. A device: attach to one that plays
/// its own music, else move what plays here there.
fn row_choice(p: &Place, state: &ViewState) -> Option<Command> {
    match &p.node {
        None => back_here(state),
        Some(_) if !p.enabled || p.selected => None,
        Some(id) if p.plays => Some(Command::OutputAttach { node_id: id.clone() }),
        Some(id) => Some(Command::OutputTransfer { node_id: Some(id.clone()) }),
    }
}

/// ⇄ of a place: this queue over what plays on that device, or that
/// device's music here.
fn swap_choice(p: &Place) -> Command {
    Command::OutputTransfer { node_id: p.node.clone() }
}

/// What the rows of one place asked for this frame.
#[derive(Default)]
struct Hit {
    row: bool,
    swap: bool,
    chevron: bool,
    /// A sound device of this computer (0: the system's).
    device: Option<usize>,
    mute: bool,
    volume: Option<f32>,
}

/// One place as drawn: its springs.
struct Room {
    /// The sound devices' list (this computer), the volume (the place that
    /// plays), 0..1 on their springs.
    sub: f32,
    volume: f32,
}

impl Room {
    /// With `sub_rows` of sound devices under it ([`sub_rows`]).
    fn height(&self, sub_rows: usize) -> f32 {
        ROW_H + self.sub * sub_rows as f32 * SUB_H + self.volume * VOLUME_H
    }
}

/// The rows of this computer's sound devices: the system's, then each
/// one; none when it knows of none.
fn sub_rows(devices: &[String]) -> usize {
    if devices.is_empty() { 0 } else { devices.len() + 1 }
}

/// The volume under the place that plays.
struct VolumeRow {
    value: f32,
    muted: bool,
    /// Lowered for a while (a voice assistant): the handle stays on the
    /// real volume, the track in a muted tone.
    duck: bool,
    enabled: bool,
}

/// A single line cut to `max_w`.
fn line(ui: &Ui, text: &str, style: TypeStyle, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let mut job = style.job(text, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(1.0));
    crate::typography::layout(ui.painter(), job)
}

/// One place: its row (the avatar, the name with «Гость», what plays
/// there in the list's supporting style, ⇄ and the chevron), this
/// computer's sound devices under it, the volume of the place that plays;
/// on its container while chosen. Rows are all of one height.
#[allow(clippy::too_many_arguments)]
fn place(ui: &mut Ui, id: Id, p: &Place, room: &Room, devices: &[String], chosen_device: Option<&str>, chevron: Option<bool>, clickable: bool, volume: &VolumeRow) -> Hit {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let mut hit = Hit::default();
    let w = ui.available_width();
    let (block, _) = ui.allocate_exact_size(vec2(w, room.height(sub_rows(devices))), Sense::hover());
    let fill = anim::color(&ctx, id.with("bg"), if p.selected { s.secondary_container } else { clear(s.secondary_container) }, motion.fast_effects);
    paint::container(ui.painter(), block, Radii::all(shape::LARGE), fill, Stroke::NONE);
    let row = Rect::from_min_size(block.min, vec2(w, ROW_H));
    let resp = ui.interact(row, id.with("row"), if clickable { Sense::click() } else { Sense::hover() });
    let content = if !p.enabled {
        s.on_surface.gamma_multiply(state::DISABLED_CONTENT)
    } else if p.selected {
        s.on_secondary_container
    } else {
        s.on_surface
    };
    let variant = if !p.enabled || p.selected { content } else { s.on_surface_variant };
    let layer_to = if !clickable {
        0.0
    } else if resp.is_pointer_button_down_on() {
        state::PRESSED
    } else if resp.hovered() {
        state::HOVER
    } else {
        0.0
    };
    let layer = anim::fraction(&ctx, id.with("layer"), layer_to, motion.fast_effects);
    if layer > 0.001 {
        paint::state_layer(ui.painter(), row, Radii::all(shape::LARGE), content, layer);
    }
    hit.row = clickable && resp.clicked();
    let salt = format!("output-{}", p.node.as_deref().unwrap_or("this"));
    mesh_ui::paint_avatar(ui, pos2(row.left() + SIDE + AVATAR / 2.0, row.center().y), AVATAR, &salt, p.kind, p.avatar);

    // ⇄ and the chevron at the end (after the row, so they take their clicks).
    let trail = Rect::from_min_max(pos2(row.left() + TEXT_X, row.top()), pos2(row.right() - SIDE, row.bottom()));
    let mut tui = ui.new_child(UiBuilder::new().max_rect(trail).layout(Layout::right_to_left(Align::Center)).id_salt(id.with("trail")));
    tui.spacing_mut().item_spacing.x = 4.0;
    let mut text_right = row.right() - SIDE;
    if let Some(open) = chevron {
        let b = tui.add(IconButton::new(if open { Icon::ExpandLess } else { Icon::ExpandMore }).size(Size::Xs).tooltip("Устройства вывода").id_salt(id.with("chevron")));
        hit.chevron = b.clicked();
        text_right = text_right.min(b.rect.left() - 8.0);
    }
    if p.swap {
        let tip = if p.node.is_some() { "Перенести мою очередь сюда" } else { "Перенести сюда" };
        let b = tui.add(IconButton::new(Icon::SwapHoriz).size(Size::Xs).style(Style::Tonal).tooltip(tip).id_salt(id.with("swap")));
        hit.swap = b.clicked();
        text_right = text_right.min(b.rect.left() - 8.0);
    }

    // The name («Гость» after it) and, under it, what plays there: the
    // list's headline and supporting text, the block centred in the row.
    let x = row.left() + TEXT_X;
    let max_w = (text_right - x).max(8.0);
    let top = if p.second.is_some() { row.center().y - (HEADLINE_H + SUPPORT_H) / 2.0 } else { row.center().y - HEADLINE_H / 2.0 };
    let badge = p.guest.then(|| line(ui, "Гость", TypeRole::LabelSmall.style(), s.on_tertiary_container, 80.0));
    let badge_w = badge.as_ref().map_or(0.0, |g| g.size().x + 12.0 + 8.0);
    let name = line(ui, &p.name, TypeRole::BodyLarge.style(), content, max_w - badge_w);
    let name_w = name.size().x;
    ui.painter().galley(pos2(x, top), name, content);
    if let Some(g) = badge {
        let alpha = if p.enabled { 1.0 } else { state::DISABLED_CONTENT };
        let r = Rect::from_min_size(pos2(x + name_w + 8.0, top + (HEADLINE_H - 16.0) / 2.0), vec2(g.size().x + 12.0, 16.0));
        paint::container(ui.painter(), r, Radii::all(8.0), s.tertiary_container.gamma_multiply(alpha), Stroke::NONE);
        ui.painter().galley(pos2(r.left() + 6.0, r.center().y - g.size().y / 2.0), g, s.on_tertiary_container.gamma_multiply(alpha));
    }
    if let Some(second) = &p.second {
        let y = top + HEADLINE_H;
        let (icon, text) = match second {
            Second::Playing { line, paused } => (Some(if *paused { Icon::Pause } else { Icon::PlayArrow }), line.as_str()),
            Second::Device(d) => (None, d.as_str()),
        };
        let mut tx = x;
        if let Some(i) = icon {
            let at = Rect::from_min_size(pos2(x, y + (SUPPORT_H - SUPPORT_ICON) / 2.0), vec2(SUPPORT_ICON, SUPPORT_ICON));
            i.widget(true, variant, SUPPORT_ICON).paint_at(ui, at);
            tx += SUPPORT_ICON + 4.0;
        }
        let g = line(ui, text, TypeRole::BodyMedium.style(), variant, text_right - tx);
        ui.painter().galley(pos2(tx, y), g, variant);
    }

    // This computer's sound devices, revealed by their spring.
    let sub = Rect::from_min_size(row.left_bottom(), vec2(w, room.sub.max(0.0) * sub_rows(devices) as f32 * SUB_H));
    if room.sub > 0.01 && !devices.is_empty() {
        let mut sui = ui.new_child(UiBuilder::new().max_rect(sub).id_salt(id.with("sub")));
        sui.set_clip_rect(sub.intersect(ui.clip_rect()));
        sui.multiply_opacity(room.sub.clamp(0.0, 1.0));
        for (i, name) in std::iter::once(SYSTEM_DEVICE).chain(devices.iter().map(String::as_str)).enumerate() {
            let r = Rect::from_min_size(pos2(row.left(), sub.top() + i as f32 * SUB_H), vec2(w, SUB_H));
            let shown = r.intersect(sub);
            if shown.height() < 1.0 {
                continue;
            }
            let chosen = if i == 0 { chosen_device.is_none() } else { chosen_device == Some(name) };
            let dr = sui.interact(shown, id.with(("device", i)), Sense::click());
            let dl = anim::fraction(&ctx, id.with(("device-layer", i)), if dr.hovered() { state::HOVER } else { 0.0 }, motion.fast_effects);
            let hl = Rect::from_min_max(pos2(r.left() + TEXT_X - 12.0, r.top() + 2.0), pos2(r.right() - 4.0, r.bottom() - 2.0));
            if dl > 0.001 {
                paint::state_layer(sui.painter(), hl, Radii::all(shape::MEDIUM), content, dl);
            }
            let color = if chosen { content } else { variant };
            // The check stands under the chevron.
            let check = pos2(r.right() - SIDE - 16.0, r.center().y);
            let g = line(&sui, name, TypeRole::BodyMedium.style(), color, check.x - 12.0 - 8.0 - (r.left() + TEXT_X));
            sui.painter().galley(pos2(r.left() + TEXT_X, r.center().y - g.size().y / 2.0), g, color);
            if chosen {
                Icon::Check.widget(true, color, 20.0).paint_at(&sui, Rect::from_center_size(check, vec2(20.0, 20.0)));
            }
            if dr.clicked() {
                hit.device = Some(i);
            }
        }
    }

    // The volume of the place that plays: its mute button under the
    // avatar, the percent where the row's buttons end. Only the chosen
    // place's answers; one that leaves folds away disabled.
    let vol = Rect::from_min_size(sub.left_bottom(), vec2(w, room.volume.max(0.0) * VOLUME_H));
    if room.volume > 0.01 {
        let live = p.selected && volume.enabled;
        let inner = Rect::from_min_max(pos2(vol.left() + SIDE + AVATAR / 2.0 - 16.0, vol.top()), pos2(vol.right() - SIDE, vol.top() + VOLUME_H));
        let mut vui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)).id_salt(id.with("volume")));
        vui.set_clip_rect(vol.intersect(ui.clip_rect()));
        vui.multiply_opacity(room.volume.clamp(0.0, 1.0));
        vui.spacing_mut().item_spacing.x = 8.0;
        if !live {
            vui.disable();
        }
        let shown = if volume.muted { 0.0 } else { volume.value };
        let mute = vui.add(IconButton::new(Icon::volume(shown, volume.muted)).size(Size::Xs).tooltip("Без звука").id_salt(id.with("mute"))).clicked();
        let label_w = 40.0;
        let slider_w = (vui.available_width() - label_w - 8.0).max(48.0);
        let track = if volume.duck { s.primary.gamma_multiply(0.45) } else { s.primary };
        let o = Slider::new(id.with("slider"), shown)
            .track(8.0)
            .width(slider_w)
            .colors(track, s.on_secondary_container.gamma_multiply(0.16))
            .handle_color(s.primary)
            .value_label(|v| format!("{:.0}%", v * 100.0))
            .show(&mut vui);
        let moved = o.dragging.or(o.committed);
        let pct = moved.filter(|_| live).unwrap_or(shown);
        vui.add_sized(vec2(label_w, 20.0), egui::Label::new(TypeRole::LabelMedium.style().rich(format!("{:.0}%", pct * 100.0), content)).selectable(false));
        if live {
            hit.mute = mute;
            hit.volume = moved;
        }
    }
    hit
}

/// The popover, while it is open (and while it leaves); drawn with the
/// overlays, over Now Playing too. As tall as the window lets it: the
/// places scroll, «Добавить» · «Карта сети» stay.
pub(crate) fn popover(chrome: &mut Chrome, ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    let theme = Theme::get(ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = Id::new("md3e-output-popover");
    // Nowhere else to play any more: it goes.
    if chrome.output.open && !available(state) {
        chrome.output.open = false;
    }
    if chrome.output.open {
        let anchor = chrome.output.anchor.unwrap_or(Rect::NOTHING);
        let pop = chrome.output.rect;
        let press_out = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|p| !anchor.expand(2.0).contains(p) && !pop.is_some_and(|r| r.expand(2.0).contains(p)))
        });
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        if press_out || esc {
            chrome.output.open = false;
        }
    }
    let open = chrome.output.open;
    let alpha = anim::fraction(ctx, id.with("alpha"), if open { 1.0 } else { 0.0 }, motion.fast_effects).clamp(0.0, 1.0);
    let k = anim::f32(ctx, id.with("k"), if open { 1.0 } else { 0.0 }, motion.fast_spatial);
    let Some(anchor) = chrome.output.anchor.filter(|_| open || alpha > 0.01) else {
        chrome.output.rect = None;
        return;
    };

    let places = places(state);
    let devices = &state.prefs.output_devices;
    let devices_open = chrome.output.devices && !devices.is_empty();
    let rooms: Vec<Room> = places
        .iter()
        .map(|p| {
            let key = p.node.as_deref().unwrap_or("");
            let sub = if p.node.is_none() { anim::f32(ctx, id.with("sub"), if devices_open { 1.0 } else { 0.0 }, motion.default_spatial).max(0.0) } else { 0.0 };
            let volume = anim::f32(ctx, id.with(("volume", key)), if p.selected { 1.0 } else { 0.0 }, motion.default_spatial).max(0.0);
            Room { sub, volume }
        })
        .collect();
    let rows_h: f32 = places.iter().zip(&rooms).map(|(p, r)| r.height(if p.node.is_none() { sub_rows(devices) } else { 0 })).sum::<f32>()
        + GAP * places.len().saturating_sub(1) as f32;
    // Above its button when it fits there, else where there is more room;
    // never taller than that room (the places scroll).
    let screen = ctx.content_rect().shrink(8.0);
    let chrome_h = PAD + FOOT_GAP + FOOT_H + PAD;
    let room_above = (anchor.top() - 8.0 - screen.top()).max(0.0);
    let room_below = (screen.bottom() - anchor.bottom() - 8.0).max(0.0);
    let full_h = chrome_h + rows_h;
    let above = full_h <= room_above || room_above >= room_below;
    let height = full_h.min(if above { room_above } else { room_below }).max(chrome_h + ROW_H.min(rows_h));
    let list_h = (height - chrome_h).max(0.0);
    let size = vec2(POPOVER_W, height);
    let x = (anchor.center().x - size.x / 2.0).clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
    let y = if above { anchor.top() - 8.0 - size.y } else { anchor.bottom() + 8.0 };
    let rect = Rect::from_min_size(pos2(x, y), size);
    chrome.output.rect = Some(rect);
    // The wheel over it scrolls it (Now Playing turns the volume elsewhere).
    chrome.wheel_block(rect);

    let volume = VolumeRow {
        value: chrome.volume_now(ctx, state),
        muted: state.player.muted,
        duck: state.player.duck.is_some(),
        enabled: !lost(state),
    };
    let remote = state.target.is_some();
    let chosen_device = state.prefs.output_device.clone();
    let mut hits: Vec<Hit> = Vec::new();
    let mut add_at = None;
    let mut to_map = false;
    let area = Area::new(id.with("area"))
        .order(Order::Foreground)
        .fixed_pos(rect.min)
        .constrain(false)
        .fade_in(false)
        .interactable(open)
        .show(ctx, |ui| {
            ui.set_opacity(alpha);
            let (r, _) = ui.allocate_exact_size(size, Sense::hover());
            let radii = Radii::all(shape::LARGE_INCREASED);
            paint::shadow(ui.painter(), r, radii, Elevation::Level3);
            paint::container(ui.painter(), r, radii, s.surface_container_high, Stroke::NONE);
            // A click between the rows stays on it.
            ui.interact(r, id.with("bg"), Sense::click());
            let inner = r.shrink(PAD);
            let mut col = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)).id_salt(id.with("col")));
            col.set_clip_rect(r.intersect(ui.clip_rect()));
            col.spacing_mut().item_spacing.y = 0.0;
            egui::ScrollArea::vertical().id_salt(id.with("places")).max_height(list_h).auto_shrink([false, true]).show(&mut col, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, (p, room)) in places.iter().zip(&rooms).enumerate() {
                    if i > 0 {
                        ui.add_space(GAP);
                    }
                    let this = p.node.is_none();
                    let list: &[String] = if this { devices.as_slice() } else { &[] };
                    let chevron = (this && !devices.is_empty()).then_some(devices_open);
                    // This computer while it plays: its row opens the sound
                    // devices; a device that plays here already: nothing.
                    let clickable = p.enabled && if this { remote || !devices.is_empty() } else { !p.selected };
                    let pid = id.with(("place", p.node.as_deref().unwrap_or("")));
                    hits.push(place(ui, pid, p, room, list, chosen_device.as_deref(), chevron, clickable, &volume));
                }
            });
            // The footer under the places, at the popover's foot.
            let foot = Rect::from_min_size(pos2(inner.left(), inner.bottom() - FOOT_H), vec2(inner.width(), FOOT_H));
            let mut left = col.new_child(UiBuilder::new().max_rect(foot).layout(Layout::left_to_right(Align::Center)).id_salt(id.with("foot-left")));
            let add = left.add(Button::new("Добавить").icon(Icon::Add).style(Style::Text).size(Size::Xs).id_salt(id.with("add")));
            if add.clicked() {
                add_at = Some(add.rect);
            }
            let mut right = col.new_child(UiBuilder::new().max_rect(foot).layout(Layout::right_to_left(Align::Center)).id_salt(id.with("foot-right")));
            to_map = right.add(Button::new("Карта сети").icon(Icon::OpenInNew).style(Style::Text).size(Size::Xs).id_salt(id.with("map"))).clicked();
        });
    let layer = area.response.layer_id;
    ctx.move_to_top(layer);
    // It grows out of its button and back into it.
    let pivot = pos2(anchor.center().x.clamp(rect.left(), rect.right()), if above { rect.bottom() } else { rect.top() });
    let scale = 0.9 + 0.1 * k;
    let transform = if (scale - 1.0).abs() < 1e-3 { TSTransform::IDENTITY } else { TSTransform::new(pivot.to_vec2() * (1.0 - scale), scale) };
    ctx.set_transform_layer(layer, transform);
    if !open {
        return;
    }

    for (p, hit) in places.iter().zip(hits) {
        let this = p.node.is_none();
        let mut choice = None;
        if hit.row {
            match row_choice(p, state) {
                Some(c) => choice = Some(c),
                None if this => chrome.output.devices = !chrome.output.devices,
                None => {}
            }
        }
        if hit.chevron {
            chrome.output.devices = !chrome.output.devices;
        }
        if hit.swap {
            choice = Some(swap_choice(p));
        }
        if let Some(i) = hit.device {
            out.push(Command::SetOutputDevice(if i == 0 { None } else { devices.get(i - 1).cloned() }));
            // A sound device of this computer means playing here.
            choice = row_choice(p, state);
        }
        if hit.mute {
            out.push(Command::ToggleMute);
        }
        if let Some(v) = hit.volume {
            chrome.push_volume(state, v, out);
        }
        if let Some(c) = choice {
            out.push(c);
        }
    }
    if let Some(at) = add_at {
        chrome.output.add_device = Some(at);
        chrome.output.open = false;
    }
    if to_map {
        chrome.nav = Some(Route::Devices);
        chrome.output.open = false;
    }
}

#[cfg(test)]
mod tests {
    use nikgob_view::{MeshNodeView, MeshPlaying, ParkedView, RemoteTarget, TrackId, TrackRow};

    use super::*;

    fn node(id: &str, online: bool, playing: Option<bool>) -> MeshNodeView {
        MeshNodeView {
            id: id.into(),
            name: id.to_uppercase(),
            kind: MeshKind::Pi,
            online,
            playing: playing.map(|paused| MeshPlaying { title: "Фонари".into(), artists: vec!["Кино".into()], paused, ..MeshPlaying::default() }),
            ..MeshNodeView::default()
        }
    }

    fn target(id: &str, how: RemoteHow) -> RemoteTarget {
        RemoteTarget { node_id: id.into(), name: id.to_uppercase(), kind: MeshKind::Pi, link: RemoteLink::Live, volume: 0.5, muted: false, how }
    }

    fn track(title: &str) -> TrackRow {
        TrackRow {
            id: TrackId::new(),
            title: title.into(),
            artist: "Аквариум".into(),
            album: None,
            release_id: None,
            artists: Default::default(),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local: true,
            liked: false,
            cover: None,
            clip: false,
            in_library: true,
        }
    }

    /// The button stands while there is another device, out of reach too,
    /// and while the window drives one.
    #[test]
    fn the_button_is_there_with_another_device() {
        let mut state = ViewState::default();
        assert!(!available(&state));
        state.mesh.nodes.push(node("pi", false, None));
        assert!(available(&state), "offline counts");
        state.mesh.nodes.clear();
        state.target = Some(target("pi", RemoteHow::Transferred));
        assert!(available(&state));
    }

    /// Away from the button, the volume's speaker is the kind of the device
    /// whose volume it sets; muted, crossed out as always.
    #[test]
    fn the_volume_shows_whose_it_is() {
        let mut state = ViewState::default();
        assert_eq!(volume_icon(&state, 0.8, false), Icon::VolumeUp);
        state.target = Some(target("pi", RemoteHow::Transferred));
        assert_eq!(volume_icon(&state, 0.8, false), Icon::DeveloperBoard);
        assert_eq!(volume_icon(&state, 0.8, true), Icon::VolumeOff);
        assert_eq!(volume_icon(&state, 0.0, false), Icon::VolumeOff);
        state.target.as_mut().unwrap().link = RemoteLink::Reconnecting;
        assert!(reconnecting(&state) && !lost(&state));
        state.target.as_mut().unwrap().link = RemoteLink::Lost;
        assert!(lost(&state) && !reconnecting(&state));
    }

    /// This computer first and chosen while it plays here, with its sound
    /// device when another than the system's; devices in reach before those
    /// out of it; what plays there under each name.
    #[test]
    fn the_places_read_in_order() {
        let mut state = ViewState::default();
        state.mesh.nodes = vec![node("phone", false, None), node("pi", true, Some(false)), node("speaker", true, None)];
        state.prefs.output_device = Some("Наушники".into());
        let p = places(&state);
        let ids: Vec<Option<&str>> = p.iter().map(|p| p.node.as_deref()).collect();
        assert_eq!(ids, [None, Some("pi"), Some("speaker"), Some("phone")]);
        assert!(p[0].selected && !p[1].selected);
        assert_eq!(p[0].second, Some(Second::Device("Наушники".into())));
        assert_eq!(p[1].second, Some(Second::Playing { line: "Кино — Фонари".into(), paused: false }));
        assert!(p[1].plays && p[1].swap, "a device playing its own music has ⇄");
        assert!(!p[2].plays && !p[2].swap);
        assert!(!p[3].enabled, "out of reach");
        assert!(!p[0].swap);
    }

    /// While the window drives a device: that device is chosen, this
    /// computer says what waits parked here; ⇄ stays on the device only
    /// when the window came there by an attach (as the runtime says), and
    /// then this computer has ⇄ too.
    #[test]
    fn a_remote_shows_the_parked_queue_and_how_it_came() {
        let mut state = ViewState::default();
        state.mesh.nodes = vec![node("pi", true, Some(false))];
        state.target = Some(target("pi", RemoteHow::Transferred));
        state.parked = Some(ParkedView { current: Some(track("Город золотой")), position: std::time::Duration::from_secs(40) });
        let moved = places(&state);
        assert!(!moved[0].selected && moved[1].selected);
        assert_eq!(moved[0].second, Some(Second::Playing { line: "Аквариум — Город золотой".into(), paused: true }));
        assert!(!moved[1].swap && !moved[0].swap, "the queue went there already");
        state.target = Some(target("pi", RemoteHow::Attached));
        let attached = places(&state);
        assert!(attached[1].swap && attached[0].swap);
    }

    /// A free device takes what plays here; one playing its own music is
    /// only driven from here; this computer brings the music back after a
    /// transfer, or after an attach goes back to its own queue; ⇄ moves.
    #[test]
    fn clicks_choose_transfer_or_attach() {
        let mut state = ViewState::default();
        state.mesh.nodes = vec![node("pi", true, Some(false)), node("tv", true, Some(true)), node("phone", false, None)];
        let p = places(&state);
        assert_eq!(row_choice(&p[0], &state), None);
        assert_eq!(row_choice(&p[1], &state), Some(Command::OutputAttach { node_id: "pi".into() }));
        assert_eq!(row_choice(&p[2], &state), Some(Command::OutputTransfer { node_id: Some("tv".into()) }), "paused is free");
        assert_eq!(row_choice(&p[3], &state), None, "out of reach");
        assert_eq!(swap_choice(&p[1]), Command::OutputTransfer { node_id: Some("pi".into()) });

        state.target = Some(target("pi", RemoteHow::Transferred));
        let moved = places(&state);
        assert_eq!(row_choice(&moved[0], &state), Some(Command::OutputTransfer { node_id: None }));
        assert_eq!(row_choice(&moved[1], &state), None, "it plays here already");
        state.target = Some(target("pi", RemoteHow::Attached));
        let attached = places(&state);
        assert_eq!(row_choice(&attached[0], &state), Some(Command::OutputDetach));
        assert_eq!(swap_choice(&attached[0]), Command::OutputTransfer { node_id: None });
        assert_eq!(back_here(&ViewState::default()), None);
    }
}
