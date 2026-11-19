//! The owner's devices drawn alike wherever they show (Settings → Межсеть →
//! «Устройства» and «Карта сети»): each kind's shape and icon, the avatar
//! whose colour says the device is online and whose turning says music
//! plays there, the storage as numbers, the profile and «Гость» chips, ⋮,
//! and the dialogs they open — a name, «Избранное» (the cap and the
//! playlists kept), the forget that could lose originals — with
//! «Добавить устройство» ([`super::add_device`]).
//!
//! No word says a state: colour, shape and motion do; text is only names,
//! numbers, choices and real blockers.

use std::f32::consts::TAU;
use std::sync::Arc;

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_shapes::MaterialShape;
use nikgob_view::{
    Command, FAVORITES_CAP_DEFAULT, FAVORITES_CAP_MAX_GB, FAVORITES_CAP_MIN_GB, LIKED_PLAYLIST, MeshKind, MeshNodeView, MeshPins, MeshTransfer,
    MeshProfile, MeshScope, MeshView, PlaylistId, StorageView, ViewState, free_in, size_in,
};

use crate::{
    Icon, Theme, TypeRole,
    components::{Checkbox, Dialog, DialogAction, Equalizer, FilterChip, IconButton, Menu, MenuItem, Size, Slider, TextField, anim},
    tokens::state,
};

const GB: u64 = 1 << 30;
/// Free space under this on a device that stores music is a blocker.
pub(crate) const LOW_FREE: u64 = 2 * GB;
/// How long a «Гость» clicked shows its new place before the ring has it.
const SENT_S: f64 = 3.0;

/// What the devices' surfaces keep between frames: the dialogs and the
/// window they share, choices sent but not shown yet.
#[derive(Default)]
pub(crate) struct MeshUi {
    /// «Добавить устройство».
    pub(crate) add: super::add_device::AddState,
    /// A nearby device's code window.
    pub(crate) pair: super::add_device::PairState,
    rename: Option<Rename>,
    favorites: Option<Favorites>,
    /// The forget asked about whose answer went (until the view drops it).
    answered: Option<String>,
    /// «Гость» clicked: (device, scope, when), until the view has it.
    scope_sent: Vec<(String, MeshScope, f64)>,
    /// «Карта сети».
    pub(crate) map: super::network::MapState,
}

/// «Переименовать»: of this device (`None`) or of a paired one.
struct Rename {
    node: Option<String>,
    name: String,
    /// This device's own name as the field's placeholder when it has one
    /// (data, not text); with an empty name the placeholder is the
    /// localized «Компьютер», drawn each frame.
    placeholder: Option<String>,
    empty_name: bool,
    focus: bool,
}

/// «Избранное…» of a device: its cap and what it keeps.
struct Favorites {
    node: String,
    name: String,
    cap_gb: u64,
    liked: bool,
    playlists: Vec<PlaylistId>,
}

/// What a control of a device asked for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Act {
    Rename,
    Forget,
    Profile(MeshProfile),
    /// «Избранное…»: its dialog.
    Favorites,
    Scope(MeshScope),
}

/// The shape a kind of device stands in.
pub(crate) fn kind_shape(k: MeshKind) -> MaterialShape {
    match k {
        MeshKind::Pc => MaterialShape::Square,
        MeshKind::Pi => MaterialShape::Cookie6Sided,
        MeshKind::Phone => MaterialShape::Pill,
        MeshKind::Tv => MaterialShape::PuffyDiamond,
        MeshKind::Server => MaterialShape::Cookie4Sided,
    }
}

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

/// How a device's avatar looks: online (its colour), music playing there
/// (accent fill, the outline turning, bars in place of the icon).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Look {
    pub online: bool,
    pub playing: bool,
}

pub(crate) fn node_look(n: &MeshNodeView) -> Look {
    Look { online: n.online, playing: n.online && n.playing.as_ref().is_some_and(|p| !p.paused) }
}

/// This computer: online while the mesh runs (or there is none to be off
/// from), playing while its player plays.
pub(crate) fn this_look(state: &ViewState) -> Look {
    // While another device plays, this one is its remote: nothing plays here.
    Look { online: true, playing: state.target.is_none() && state.player.status == nikgob_view::Status::Playing }
}

/// A turning that advances only while `running` (`period` seconds a turn)
/// and holds where it stopped, so the next start goes on from there.
pub(crate) fn spin(ctx: &egui::Context, id: egui::Id, running: bool, period: f32) -> f32 {
    let now = anim::now(ctx);
    if running {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| {
        let (turns, last) = d.get_temp_mut_or_insert_with(id, || (0.0_f64, now));
        // A frame that comes late (a hidden window) does not spin it on.
        let dt = (now - *last).clamp(0.0, 0.1);
        if running {
            *turns = (*turns + dt / f64::from(period.max(0.1))).rem_euclid(1.0);
        }
        *last = now;
        *turns as f32 * TAU
    })
}

/// A device's avatar of `size` centred at `center`.
pub(crate) fn paint_avatar(ui: &Ui, center: Pos2, size: f32, salt: &str, kind: MeshKind, look: Look) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = egui::Id::new(("mesh-avatar", salt));
    let (fill, on) = if look.playing {
        (s.primary, s.on_primary)
    } else if look.online {
        (s.primary_container, s.on_primary_container)
    } else {
        (s.surface_container_highest, s.on_surface_variant.gamma_multiply(state::DISABLED_CONTENT))
    };
    let fill = anim::color(&ctx, id.with("fill"), fill, motion.default_effects);
    let on = anim::color(&ctx, id.with("on"), on, motion.default_effects);
    // Online it stands at full size; out of reach a little smaller, so
    // coming back is a small bounce of the shape.
    let k = anim::f32(&ctx, id.with("k"), if look.online { 1.0 } else { 0.9 }, motion.fast_spatial);
    let d = size * k;
    let rect = Rect::from_center_size(center, vec2(d, d));
    let turn = spin(&ctx, id.with("spin"), look.playing && !theme.reduced_motion, 20.0);
    crate::components::paint_shape_avatar_turned(ui, rect, None, "", kind_shape(kind), fill, on, turn);
    let glyph = (size * 0.5).round();
    let g = Rect::from_center_size(center, vec2(glyph, glyph));
    // The icon and the bars cross-fade.
    let p = anim::fraction(&ctx, id.with("bars-in"), if look.playing { 1.0 } else { 0.0 }, motion.fast_effects).clamp(0.0, 1.0);
    if p < 0.999 {
        kind_icon(kind).widget(look.online, on.gamma_multiply(1.0 - p), glyph).paint_at(ui, g);
    }
    if p > 0.001 {
        Equalizer::new(("mesh-avatar-bars", salt), on.gamma_multiply(p)).size(glyph * 0.9).playing(look.playing).paint_at(ui, g);
    }
}

/// One line of text cut to `max_w`.
pub(crate) fn text_line(painter: &egui::Painter, text: &str, role: TypeRole, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let mut job = role.style().job(text, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(1.0));
    crate::typography::layout(painter, job)
}

/// «18,4 / 25 ГБ» / "18.4 / 25 GB": the unit once when both are in it.
pub(crate) fn fraction_in(loc: nikgob_core::i18n::Locale, used: u64, cap: u64) -> String {
    let (u, c) = (size_in(loc, used), size_in(loc, cap));
    match (u.rsplit_once(' '), c.rsplit_once(' ')) {
        (Some((un, uu)), Some((_, cu))) if uu == cu => format!("{un} / {c}"),
        _ => format!("{u} / {c}"),
    }
}

/// «1,2 из 27,6 ГБ»: how much of the files on their way came.
pub(crate) fn transfer_line(t: &MeshTransfer) -> String {
    let loc = nikgob_core::i18n::locale();
    let (d, w) = (size_in(loc, t.done), size_in(loc, t.total));
    let of = nikgob_core::i18n::mesh_in(loc).size_of;
    match (d.rsplit_once(' '), w.rsplit_once(' ')) {
        (Some((dn, du)), Some((_, wu))) if du == wu => format!("{dn} {of} {w}"),
        _ => format!("{d} {of} {w}"),
    }
}

/// How much of this computer's music a device of profile `p` is to hold:
/// all of it with «Полная копия», the tracks without the originals with
/// «Библиотека» (none: no such goal, or this computer's not counted yet).
pub(crate) fn copy_goal(p: MeshProfile, here: &StorageView) -> Option<u64> {
    if !here.known {
        return None;
    }
    match p {
        MeshProfile::Full => Some(here.music()),
        MeshProfile::Library => Some(here.library),
        _ => None,
    }
    .filter(|g| *g > 0)
}

/// A device's bar and line on the map: of a full copy or the library, how
/// much of this computer's music it holds («14 из 27,6 ГБ», the whole
/// once it is there); else while files come, how far they got; else what
/// it holds. A nearly full disk says so before anything.
pub(crate) fn node_bar_and_line(n: &MeshNodeView, here: &StorageView) -> (Option<(f32, f32)>, Option<(String, bool)>) {
    let line = storage_line(n);
    if line.as_ref().is_some_and(|(_, error)| *error) {
        return (bar_parts(n), line);
    }
    if let (Some(goal), Some(st)) = (copy_goal(n.profile, here), n.storage) {
        // Within a few per cent it is there (sizes on two file systems).
        if st.used.saturating_mul(100) < goal.saturating_mul(98) {
            let t = MeshTransfer { done: st.used, total: goal, paused: false, speed: 0 };
            let f = (st.used as f64 / goal as f64).clamp(0.0, 1.0) as f32;
            return (Some((f, 0.0)), Some((transfer_line(&t), false)));
        }
        return (bar_parts(n), line);
    }
    match n.transfer {
        Some(t) => {
            let f = if t.total == 0 { 0.0 } else { (t.done as f64 / t.total as f64).clamp(0.0, 1.0) as f32 };
            (Some((f, 0.0)), Some((transfer_line(&t), false)))
        }
        _ => (bar_parts(n), line),
    }
}

/// What a device's second line says: the music it holds (of its cap with
/// «Избранное»), or that its disk is nearly full (an error); nothing for
/// «Только звук» or before it said.
pub(crate) fn storage_line(n: &MeshNodeView) -> Option<(String, bool)> {
    if !n.profile.stores() {
        return None;
    }
    let st = n.storage?;
    let loc = nikgob_core::i18n::locale();
    if st.free < LOW_FREE {
        return Some((nikgob_core::i18n::mesh::fmt_low_space_in(loc, &free_in(loc, st.free)), true));
    }
    Some(match n.profile {
        MeshProfile::Favorites { cap_bytes } => (fraction_in(loc, st.used, cap_bytes), false),
        _ => (size_in(loc, st.used), false),
    })
}

/// This computer's second line: all the music it holds, the originals
/// included — the number the other devices show of it — and of that the
/// originals when it has some («296 ГБ · оригиналы 112 ГБ»).
pub(crate) fn this_storage_line(st: &StorageView) -> Option<String> {
    if !st.known {
        return None;
    }
    let loc = nikgob_core::i18n::locale();
    Some(if st.masters > 0 { nikgob_core::i18n::mesh::fmt_masters_in(loc, &size_in(loc, st.music()), &size_in(loc, st.masters)) } else { size_in(loc, st.music()) })
}

/// The share of a device's bar each part takes: (music, originals, rest)
/// of its disk or its cap, as fractions.
pub(crate) fn bar_parts(n: &MeshNodeView) -> Option<(f32, f32)> {
    let st = n.storage?;
    let whole = match n.profile {
        MeshProfile::Favorites { cap_bytes } => cap_bytes.max(1),
        _ => st.used.saturating_add(st.free).max(1),
    };
    Some(((st.used as f64 / whole as f64).clamp(0.0, 1.0) as f32, 0.0))
}

/// This computer's bar: the music, the originals, of its first disk.
pub(crate) fn this_bar_parts(st: &StorageView) -> Option<(f32, f32)> {
    if !st.known {
        return None;
    }
    let disk = st.disks.first()?;
    let whole = disk.total.max(1) as f64;
    Some(((st.library as f64 / whole).clamp(0.0, 1.0) as f32, (st.masters as f64 / whole).clamp(0.0, 1.0) as f32))
}

/// A storage bar `thick` high: the music in primary, the originals in
/// tertiary, the rest in secondary-container, 2 apart; past 95 % the music
/// is in error.
pub(crate) fn paint_bar(ui: &Ui, rect: Rect, music: f32, masters: f32) {
    let s = Theme::get(ui.ctx()).scheme;
    let r = rect.height() * 0.5;
    let p = ui.painter();
    let w = rect.width();
    let full = music + masters >= 0.95;
    let gap = 2.0;
    let mut x = rect.left();
    let part = |frac: f32, color: Color32, x: &mut f32| {
        if frac <= 0.0 {
            return;
        }
        let pw = (frac * w).max(rect.height());
        let seg = Rect::from_min_max(pos2(*x, rect.top()), pos2((*x + pw).min(rect.right()), rect.bottom()));
        p.rect_filled(seg, r, color);
        *x = seg.right() + gap;
    };
    part(music, if full { s.error } else { s.primary }, &mut x);
    part(masters, s.tertiary, &mut x);
    if x < rect.right() - rect.height() {
        p.rect_filled(Rect::from_min_max(pos2(x, rect.top()), rect.max), r, s.secondary_container);
    }
}

/// A sine line from `a` to `b` (its phase running from `a` toward `b`), of
/// `amp` across its way, its ends round.
pub(crate) fn wave_between(ui: &Ui, a: Pos2, b: Pos2, amp: f32, wavelength: f32, phase: f32, stroke: Stroke) {
    let d = b - a;
    let len = d.length();
    if len < 1.0 {
        return;
    }
    let (dir, normal) = (d / len, vec2(-d.y, d.x) / len);
    let n = (len / 2.0).ceil().max(2.0) as usize;
    let k = TAU / wavelength.max(1.0);
    // The crests fade in over the first and last half wave: the line
    // leaves and meets the cards flat.
    let ease = (wavelength * 0.5).max(1.0);
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = len * i as f32 / n as f32;
            let edge = (t.min(len - t) / ease).clamp(0.0, 1.0);
            a + dir * t + normal * (amp * edge * (k * t - phase).sin())
        })
        .collect();
    ui.painter().add(egui::Shape::line(pts, stroke));
    ui.painter().circle_filled(a, stroke.width * 0.5, stroke.color);
    ui.painter().circle_filled(b, stroke.width * 0.5, stroke.color);
}

/// The profile chip of a device with its menu; «Избранное…» opens its
/// dialog.
pub(crate) fn profile_chip(ui: &mut Ui, salt: &str, n: &MeshNodeView, enabled: bool) -> Option<Act> {
    let at = match n.profile {
        MeshProfile::Full => 0,
        MeshProfile::Library => 1,
        MeshProfile::Favorites { .. } => 2,
        MeshProfile::SoundOnly => 3,
    };
    let chip = FilterChip::new(n.profile.label(), false).trailing_icon(Icon::ExpandMore).enabled(enabled).id_salt(("mesh-profile", salt));
    let resp = ui.add(chip);
    let mut menu = Menu::new(("mesh-profile-menu", salt));
    for (i, label) in nikgob_core::i18n::mesh::profile_choices_in(nikgob_core::i18n::locale()).into_iter().enumerate() {
        menu = menu.item(MenuItem::new(label).selected(i == at));
    }
    match menu.attach(&resp)? {
        0 => Some(Act::Profile(MeshProfile::Full)),
        1 => Some(Act::Profile(MeshProfile::Library)),
        2 => Some(Act::Favorites),
        _ => Some(Act::Profile(MeshProfile::SoundOnly)),
    }
}

/// «Гость»: on while the device is a guest (as clicked, until the ring
/// has it); only the device holding the ring changes it.
pub(crate) fn guest_chip(ui: &mut Ui, salt: &str, n: &MeshNodeView, holder: bool, mem: &MeshUi) -> Option<Act> {
    let guest = shown_scope(mem, n, anim::now(ui.ctx())) == MeshScope::Guest;
    let r = ui.add(FilterChip::new(nikgob_core::i18n::mesh().guest, guest).enabled(holder).id_salt(("mesh-guest", salt)));
    r.clicked().then_some(Act::Scope(if guest { MeshScope::Owner } else { MeshScope::Guest }))
}

/// The scope a device shows: the one clicked, for a moment, until the
/// ring has it.
fn shown_scope(mem: &MeshUi, n: &MeshNodeView, now: f64) -> MeshScope {
    mem.scope_sent.iter().find(|(id, s, at)| *id == n.id && *s != n.scope && now - at < SENT_S).map_or(n.scope, |(_, s, _)| *s)
}

/// ⋮ of a device: «Переименовать», and for a paired one «Забыть» (only
/// the device holding the ring forgets).
pub(crate) fn more_menu(ui: &mut Ui, salt: &str, this: bool, can_forget: bool) -> Option<Act> {
    let b = ui.add(IconButton::new(Icon::MoreVert).size(Size::Xs).tooltip(nikgob_core::i18n::common().more).id_salt(("mesh-more", salt)));
    let mut menu = Menu::new(("mesh-more-menu", salt)).align_right(true).item(MenuItem::new(nikgob_core::i18n::common().rename).icon(Icon::Edit));
    if !this {
        menu = menu.divider().item(MenuItem::new(nikgob_core::i18n::mesh().forget).icon(Icon::Delete).enabled(can_forget));
    }
    match menu.attach(&b)? {
        0 => Some(Act::Rename),
        _ => Some(Act::Forget),
    }
}

/// Does what a control of device `node` (`None`: this one) asked for.
pub(crate) fn act(ui: &Ui, m: &MeshView, node: Option<&MeshNodeView>, a: Act, mem: &mut MeshUi, state: &ViewState, out: &mut Vec<Command>) {
    let now = anim::now(ui.ctx());
    match (a, node) {
        (Act::Rename, None) => {
            let empty_name = m.this.name.is_empty();
            mem.rename = Some(Rename {
                node: None,
                name: m.name_setting.clone(),
                placeholder: (!empty_name).then(|| m.this.name.clone()),
                empty_name,
                focus: true,
            });
        }
        (Act::Rename, Some(n)) => mem.rename = Some(Rename { node: Some(n.id.clone()), name: n.name.clone(), placeholder: None, empty_name: false, focus: true }),
        // Checked by the loop first (the originals only it holds): it asks
        // through the view when some would go.
        (Act::Forget, Some(n)) if m.ring_holder => out.push(Command::MeshForget { node_id: n.id.clone() }),
        (Act::Profile(p), Some(n)) if p != n.profile => out.push(Command::MeshSetProfile { node_id: n.id.clone(), profile: p }),
        (Act::Favorites, Some(n)) => {
            let cap = match n.profile {
                MeshProfile::Favorites { cap_bytes } => cap_bytes,
                _ => FAVORITES_CAP_DEFAULT,
            };
            let lists: Vec<PlaylistId> = n.pins.playlists.iter().copied().filter(|p| state.library.playlists.iter().any(|c| c.id == *p)).collect();
            mem.favorites = Some(Favorites {
                node: n.id.clone(),
                name: n.name.clone(),
                cap_gb: (cap / GB).clamp(FAVORITES_CAP_MIN_GB, FAVORITES_CAP_MAX_GB),
                // A first choice of «Избранное» keeps «Любимые».
                liked: n.pins.liked || (!matches!(n.profile, MeshProfile::Favorites { .. }) && n.pins.playlists.is_empty()),
                playlists: lists,
            });
        }
        (Act::Scope(scope), Some(n)) if m.ring_holder => {
            mem.scope_sent.retain(|(id, _, _)| *id != n.id);
            mem.scope_sent.push((n.id.clone(), scope, now));
            out.push(Command::MeshSetScope { node_id: n.id.clone(), scope });
        }
        _ => {}
    }
}

fn overlays_pass_id() -> egui::Id {
    egui::Id::new("mesh-overlays-pass")
}

/// [`overlays`] ran this frame already.
pub(crate) fn overlays_drawn(ctx: &egui::Context) -> bool {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<u64>(overlays_pass_id())) == Some(pass)
}

/// The dialogs and the window of the devices, wherever they were opened
/// (once a frame).
pub(crate) fn overlays(ui: &mut Ui, state: &ViewState, mem: &mut MeshUi, out: &mut Vec<Command>) {
    if overlays_drawn(ui.ctx()) {
        return;
    }
    let pass = ui.ctx().cumulative_pass_nr();
    ui.ctx().data_mut(|d| d.insert_temp(overlays_pass_id(), pass));
    let now = anim::now(ui.ctx());
    mem.scope_sent.retain(|(id, s, at)| now - at < SENT_S && state.mesh.node(id).is_some_and(|n| n.scope != *s));
    rename_dialog(ui, mem, out);
    favorites_dialog(ui, state, mem, out);
    forget_dialog(ui, state, mem, out);
    super::add_device::window(ui, state, mem, out);
    super::add_device::pair_window(ui, state, mem, out);
}

/// «Переименовать»: the name this device goes by everywhere (empty: the
/// computer's own), or the one this computer shows for another.
fn rename_dialog(ui: &mut Ui, mem: &mut MeshUi, out: &mut Vec<Command>) {
    let Some(d) = mem.rename.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let mut open = true;
    let mut submitted = false;
    // This device may go back to the computer's name; another needs one.
    let can = d.node.is_none() || !d.name.trim().is_empty();
    let r = Dialog::new("mesh-rename-dialog", nikgob_core::i18n::common().rename)
        .max_width(400.0)
        .action(nikgob_core::i18n::common().cancel)
        .action_with(DialogAction::new(nikgob_core::i18n::common().save).enabled(can))
        .show_with(&ctx, &mut open, |ui| {
            ui.add_space(16.0);
            let mut f = TextField::new(&mut d.name, nikgob_core::i18n::mesh().name_label).width(ui.available_width()).request_focus(d.focus).id_salt("mesh-rename-field");
            if d.empty_name {
                f = f.placeholder(nikgob_core::i18n::mesh().kind_pc);
            } else if let Some(p) = &d.placeholder {
                f = f.placeholder(p.clone());
            }
            let f = f.show(ui);
            d.focus = false;
            submitted = f.submitted;
        });
    if (r.action == Some(1) || submitted) && can {
        out.push(Command::MeshRename { node_id: d.node.clone(), name: d.name.trim().to_owned() });
        mem.rename = None;
    } else if r.action.is_some() || r.dismissed || !open {
        mem.rename = None;
    }
}

/// «Избранное»: how much the device may fill and what it keeps — «Любимые»
/// and any playlists.
fn favorites_dialog(ui: &mut Ui, state: &ViewState, mem: &mut MeshUi, out: &mut Vec<Command>) {
    let Some(d) = mem.favorites.as_mut() else { return };
    let ctx = ui.ctx().clone();
    let s = Theme::get(&ctx).scheme;
    let mut open = true;
    let lists: Vec<(PlaylistId, String)> = state.library.playlists.iter().filter(|p| p.id != LIKED_PLAYLIST).map(|p| (p.id, p.name.clone())).collect();
    let r = Dialog::new("mesh-favorites-dialog", d.name.clone())
        .max_width(440.0)
        .action(nikgob_core::i18n::common().cancel)
        .action_with(DialogAction::new(nikgob_core::i18n::common().save))
        .show_with(&ctx, &mut open, |ui| {
            ui.add_space(16.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
            ui.painter().text(row.left_center(), egui::Align2::LEFT_CENTER, nikgob_core::i18n::mesh().favorites, TypeRole::LabelLarge.font_id(), s.on_surface_variant);
            ui.painter().text(row.right_center(), egui::Align2::RIGHT_CENTER, format!("{} {}", d.cap_gb, nikgob_core::i18n::common().gb), TypeRole::TitleMedium.font_id(), s.on_surface);
            ui.add_space(8.0);
            let span = (FAVORITES_CAP_MAX_GB - FAVORITES_CAP_MIN_GB) as f32;
            let frac = (d.cap_gb - FAVORITES_CAP_MIN_GB) as f32 / span;
            let o = Slider::new("mesh-favorites-cap", frac)
                .width(ui.available_width())
                .value_label(move |v| format!("{} {}", cap_at(v, span), nikgob_core::i18n::common().gb))
                .show(ui);
            if let Some(v) = o.dragging.or(o.committed) {
                d.cap_gb = cap_at(v, span);
            }
            ui.add_space(12.0);
            check_row(ui, "mesh-favorites-liked", nikgob_core::i18n::mesh().liked, &mut d.liked);
            egui::ScrollArea::vertical().id_salt("mesh-favorites-lists").max_height(240.0).auto_shrink([false, true]).show(ui, |ui| {
                for (id, name) in &lists {
                    let mut on = d.playlists.contains(id);
                    if check_row(ui, ("mesh-favorites-list", *id), name, &mut on) {
                        if on {
                            d.playlists.push(*id);
                        } else {
                            d.playlists.retain(|p| p != id);
                        }
                    }
                }
            });
        });
    if r.action == Some(1) {
        out.push(Command::MeshSetProfile { node_id: d.node.clone(), profile: MeshProfile::Favorites { cap_bytes: d.cap_gb * GB } });
        out.push(Command::MeshSetPins { node_id: d.node.clone(), pins: MeshPins { liked: d.liked, playlists: d.playlists.clone() } });
        mem.favorites = None;
    } else if r.action.is_some() || r.dismissed || !open {
        mem.favorites = None;
    }
}

/// The cap at `v` of the slider, in whole GiB.
fn cap_at(v: f32, span: f32) -> u64 {
    FAVORITES_CAP_MIN_GB + (v.clamp(0.0, 1.0) * span).round() as u64
}

/// A checkbox and its name, the name a click target too. True when it
/// changed.
fn check_row(ui: &mut Ui, salt: impl egui::AsId + Copy, name: &str, on: &mut bool) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let was = *on;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add(Checkbox::new(on).label(name).id_salt(("check", salt)));
        let l = ui.add(egui::Label::new(TypeRole::BodyLarge.style().rich(name, s.on_surface)).truncate().sense(Sense::click()));
        if l.clicked() {
            *on = !*on;
        }
    });
    *on != was
}

/// «Забыть» stopped by the check made at that moment: originals have their
/// only copy on that device (or the check failed). It asks; the answer goes
/// once.
fn forget_dialog(ui: &mut Ui, state: &ViewState, mem: &mut MeshUi, out: &mut Vec<Command>) {
    let Some(d) = state.mesh.forget_ask.as_ref() else {
        mem.answered = None;
        return;
    };
    if mem.answered.as_deref() == Some(d.node_id.as_str()) {
        return;
    }
    let ctx = ui.ctx().clone();
    let mut open = true;
    let r = Dialog::new("mesh-forget-dialog", nikgob_core::i18n::mesh::fmt_forget_in(nikgob_core::i18n::locale(), &d.name))
        .icon(Icon::Error)
        .supporting(forget_text(d.originals))
        .max_width(420.0)
        .action(nikgob_core::i18n::common().cancel)
        .action_with(DialogAction::new(nikgob_core::i18n::mesh().forget_anyway))
        .show_with(&ctx, &mut open, |_| {});
    let answer = if r.action == Some(1) {
        Some(true)
    } else if r.action.is_some() || r.dismissed || !open {
        Some(false)
    } else {
        None
    };
    if let Some(forget) = answer {
        out.push(Command::MeshForgetAnswer { node_id: d.node_id.clone(), forget });
        mem.answered = Some(d.node_id.clone());
    }
}

/// What the forget's dialog says: how many originals only that device
/// holds, or that it could not be checked.
fn forget_text(originals: Option<u64>) -> String {
    nikgob_core::i18n::mesh::fmt_forget_text_in(nikgob_core::i18n::locale(), originals)
}

#[cfg(test)]
mod tests {
    use nikgob_view::{MeshStorage, MeshPlaying};

    use super::*;

    #[test]
    fn every_kind_has_its_own_shape_and_icon() {
        let shapes: std::collections::HashSet<MaterialShape> = MeshKind::ALL.iter().map(|k| kind_shape(*k)).collect();
        let icons: std::collections::HashSet<Icon> = MeshKind::ALL.iter().map(|k| kind_icon(*k)).collect();
        assert_eq!(shapes.len(), MeshKind::ALL.len());
        assert_eq!(icons.len(), MeshKind::ALL.len());
    }

    #[test]
    fn storage_reads_as_numbers_and_a_full_disk_as_a_blocker() {
        let mut n = MeshNodeView { storage: Some(MeshStorage { used: 184 * GB + GB / 5, free: 500 * GB }), ..MeshNodeView::default() };
        assert_eq!(storage_line(&n), Some(("184 ГБ".to_owned(), false)));
        n.profile = MeshProfile::Favorites { cap_bytes: 25 * GB };
        n.storage = Some(MeshStorage { used: 18 * GB + 2 * GB / 5, free: 40 * GB });
        assert_eq!(storage_line(&n), Some(("18,4 / 25 ГБ".to_owned(), false)));
        n.storage = Some(MeshStorage { used: 18 * GB, free: GB });
        assert_eq!(storage_line(&n), Some(("Мало места: свободно 1 ГБ".to_owned(), true)));
        n.profile = MeshProfile::SoundOnly;
        assert_eq!(storage_line(&n), None, "nothing stored, nothing said");
        let st = StorageView { known: true, library: 184 * GB, masters: 112 * GB, disks: Vec::new() };
        assert_eq!(this_storage_line(&st).as_deref(), Some("296 ГБ · оригиналы 112 ГБ"), "the whole, as the other devices show it");
        let st = StorageView { known: true, library: 4 * GB + 4 * GB / 5, masters: 11 * GB, disks: Vec::new() };
        assert_eq!(this_storage_line(&st).as_deref(), Some("15,8 ГБ · оригиналы 11 ГБ"));
        assert_eq!(this_storage_line(&StorageView::default()), None);
        assert_eq!(fraction_in(nikgob_core::i18n::Locale::Ru, 512 << 20, 25 * GB), "512 МБ / 25 ГБ");
        assert_eq!(fraction_in(nikgob_core::i18n::Locale::En, 512 << 20, 25 * GB), "512 MB / 25 GB");
    }

    #[test]
    fn files_on_their_way_show_how_much_came() {
        let t = MeshTransfer { done: GB + GB / 5, total: 27 * GB + 3 * GB / 5, paused: false, speed: 0 };
        let none = StorageView::default();
        let fav = MeshProfile::Favorites { cap_bytes: 25 * GB };
        let mut n = MeshNodeView { storage: Some(MeshStorage { used: 0, free: 197 * GB }), transfer: Some(t), profile: fav, ..MeshNodeView::default() };
        let (bar, line) = node_bar_and_line(&n, &none);
        assert_eq!(line, Some(("1,2 из 27,6 ГБ".to_owned(), false)));
        assert!(bar.is_some_and(|(f, _)| (f - 1.2 / 27.6).abs() < 0.01), "{bar:?}");
        assert_eq!(transfer_line(&MeshTransfer { done: 300 << 20, total: 2 * GB, ..t }), "300 МБ из 2 ГБ");
        n.storage = Some(MeshStorage { used: 18 * GB, free: GB });
        assert!(node_bar_and_line(&n, &none).1.is_some_and(|(_, error)| error), "a full disk still says so");
        // A full copy: how much of this computer's music is there, whether
        // files come now or not.
        // «16,2 ГБ · оригиналы 11,4 ГБ» here: the originals are of the 16,2.
        let here = StorageView { known: true, library: 4 * GB + 4 * GB / 5, masters: 11 * GB + 2 * GB / 5, disks: Vec::new() };
        n.profile = MeshProfile::Full;
        n.transfer = None;
        n.storage = Some(MeshStorage { used: 14 * GB, free: 197 * GB });
        let (bar, line) = node_bar_and_line(&n, &here);
        assert_eq!(line, Some(("14 из 16,2 ГБ".to_owned(), false)));
        assert!(bar.is_some_and(|(f, _)| (f - 14.0 / 16.2).abs() < 0.01), "{bar:?}");
        n.storage = Some(MeshStorage { used: 2 * GB, free: 197 * GB });
        n.profile = MeshProfile::Library;
        assert_eq!(node_bar_and_line(&n, &here).1, Some(("2 из 4,8 ГБ".to_owned(), false)));
        n.storage = Some(MeshStorage { used: 16 * GB + GB / 5, free: 197 * GB });
        n.profile = MeshProfile::Full;
        assert_eq!(node_bar_and_line(&n, &here).1, Some(("16,2 ГБ".to_owned(), false)), "all there: what it holds");
    }

    #[test]
    fn a_device_plays_only_while_online_and_not_paused() {
        let mut n = MeshNodeView { online: true, playing: Some(MeshPlaying { paused: false, ..MeshPlaying::default() }), ..MeshNodeView::default() };
        assert_eq!(node_look(&n), Look { online: true, playing: true });
        n.playing.as_mut().unwrap().paused = true;
        assert!(!node_look(&n).playing);
        n.online = false;
        assert_eq!(node_look(&n), Look::default());
    }

    /// The forget's question says how many originals would go, or that the
    /// check failed.
    #[test]
    fn the_forget_says_what_would_go() {
        assert_eq!(forget_text(Some(12)), "Только на этом устройстве лежат проверенные копии: 12 оригиналов FLAC.");
        assert_eq!(forget_text(Some(1)), "Только на этом устройстве лежат проверенные копии: 1 оригинал FLAC.");
        assert!(forget_text(None).starts_with("Не удалось проверить"));
    }

    #[test]
    fn the_cap_moves_in_whole_gigabytes() {
        let span = (FAVORITES_CAP_MAX_GB - FAVORITES_CAP_MIN_GB) as f32;
        assert_eq!(cap_at(0.0, span), FAVORITES_CAP_MIN_GB);
        assert_eq!(cap_at(1.0, span), FAVORITES_CAP_MAX_GB);
        assert_eq!(cap_at(2.0, span), FAVORITES_CAP_MAX_GB);
    }
}
