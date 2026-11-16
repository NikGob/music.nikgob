//! «Карта сети» (`Route::Devices`): the owner's devices as cards standing
//! around an open courtyard on a quiet mat — no grid, no glow. Two stand
//! left and right, three as a triangle, four as two pairs, five and more
//! on a ring; this computer first. A card: the kind's shape (its fill:
//! online; turning: music plays there), the name, ⋮; what plays there on a
//! plate of its own (the card grows for it); the storage as a bar and a
//! number.
//!
//! Between the cards nothing lies at rest. A line grows only while
//! something flows: files on their way — a wave from this computer to the
//! device, its speed in a capsule on it; music playing on
//! another device — a calm arc to it. Lines and cards come and go on
//! springs.
//!
//! A card opens the device's passport beside the mat (360 dp): its name,
//! kind, what it stores, the bar, what plays, version, address, when it
//! was last seen, «Гость», «Забыть». The unpaired players nearby wait at
//! the mat's foot with «Подключить», «Добавить устройство» at its end.
//! A narrow window stacks the cards and the passport rises as a sheet.
//!
//! The floating player stays alive under the page: it is a page of the
//! stack, not a window over it.

use std::collections::HashMap;
use std::f32::consts::TAU;

use egui::{Align, Layout, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, pos2, vec2};
use nikgob_view::{Command, MeshKind, MeshNodeView, MeshView, Route, ViewState, kind_label};

use super::PageState;
use super::add_device::{self, AddTab};
use super::mesh_ui::{self, Act, Look, MeshUi};
use crate::{
    Icon, Theme, TypeRole,
    components::{BottomSheet, Button, IconButton, Size, Style, SupportingPane, anim, paint, paint::Radii},
    tokens::{Elevation, state},
};

/// The page's title (and its button's name in Settings).
pub const TITLE: &str = "Карта сети";

/// A card at rest, and grown for what plays there.
pub(crate) const CARD_W: f32 = 240.0;
pub(crate) const CARD_H: f32 = 168.0;
const CARD_SHORT: f32 = 104.0;
const CARD_PAD: f32 = 14.0;
/// The courtyard's least room between two cards.
const COURT: f32 = 48.0;
const PANE_W: f32 = 360.0;
const GAP: f32 = 24.0;
/// The foot of the mat: the nearby players and «Добавить устройство».
const FOOT_H: f32 = 64.0;
/// Narrower than this the cards stack.
const NARROW: f32 = 640.0;

/// What the map keeps between frames.
#[derive(Default)]
pub(crate) struct MapState {
    /// The device whose passport is open (`""`: this computer).
    selected: Option<String>,
    /// Each transfer's last bytes seen, when, and its smoothed speed (B/s).
    speeds: HashMap<String, (u64, f64, f64)>,
}

impl MapState {
    /// Opens the passport of device `id` (`""`: this computer).
    pub(crate) fn select(&mut self, id: String) {
        self.selected = Some(id);
    }
}

/// One device as the map draws it.
struct Dev<'a> {
    /// `""` for this computer.
    id: &'a str,
    name: String,
    kind: MeshKind,
    look: Look,
    node: Option<&'a MeshNodeView>,
    /// What plays there: title, artists, progress 0..1.
    playing: Option<(String, String, Option<f32>)>,
    /// The bar's parts and the number under it.
    bar: Option<(f32, f32)>,
    number: Option<(String, bool)>,
}

fn devices<'a>(state: &'a ViewState, wall: i64) -> Vec<Dev<'a>> {
    let m = &state.mesh;
    let p = &state.player;
    let here = p.current.as_ref().filter(|_| p.status != nikgob_view::Status::Stopped).map(|t| {
        let progress = p.duration.filter(|d| !d.is_zero()).map(|d| (p.position_now().as_secs_f32() / d.as_secs_f32()).clamp(0.0, 1.0));
        (t.title.clone(), t.artist.clone(), progress)
    });
    let mut out = vec![Dev {
        id: "",
        name: this_name(m),
        kind: MeshKind::Pc,
        look: mesh_ui::this_look(state),
        node: None,
        playing: here,
        bar: mesh_ui::this_bar_parts(&state.downloads.storage),
        number: mesh_ui::this_storage_line(&state.downloads.storage).map(|t| (t, false)),
    }];
    for n in &m.nodes {
        let playing = n.playing.as_ref().filter(|_| n.online).map(|pl| {
            let progress = pl.duration_ms.filter(|d| *d > 0).map(|d| (pl.position_at(wall) as f32 / d as f32).clamp(0.0, 1.0));
            (pl.title.clone(), pl.artists.join(", "), progress)
        });
        out.push(Dev {
            id: &n.id,
            name: n.name.clone(),
            kind: n.kind,
            look: mesh_ui::node_look(n),
            node: Some(n),
            playing,
            bar: mesh_ui::bar_parts(n),
            number: mesh_ui::storage_line(n),
        });
    }
    out
}

fn this_name(m: &MeshView) -> String {
    let set = m.name_setting.trim();
    if !set.is_empty() {
        set.to_owned()
    } else if !m.this.name.is_empty() {
        m.this.name.clone()
    } else {
        "Этот компьютер".to_owned()
    }
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

/// Where `n` cards stand, as offsets of their centres from the courtyard's
/// centre, in a field of `size`: two side by side, three as a triangle,
/// four as two pairs, more on a ring from the top; the first is this
/// computer (left, top left, top).
pub(crate) fn slots(n: usize, size: Vec2) -> Vec<Vec2> {
    let sx = (size.x * 0.5 - CARD_W * 0.5 - 8.0).max(0.0);
    let sy = (size.y * 0.5 - CARD_H * 0.5 - 8.0).max(0.0);
    match n {
        0 => Vec::new(),
        1 => vec![Vec2::ZERO],
        2 => {
            let a = sx.min(CARD_W * 0.5 + 120.0);
            vec![vec2(-a, 0.0), vec2(a, 0.0)]
        }
        3 => {
            let (a, b) = (sx.min(CARD_W * 0.5 + 100.0), sy.min(CARD_H * 0.5 + 56.0));
            vec![vec2(-a, -b), vec2(a, -b), vec2(0.0, b)]
        }
        4 => {
            let (a, b) = (sx.min(CARD_W * 0.5 + 90.0), sy.min(CARD_H * 0.5 + 60.0));
            vec![vec2(-a, -b), vec2(a, -b), vec2(-a, b), vec2(a, b)]
        }
        _ => (0..n)
            .map(|i| {
                let t = -TAU / 4.0 + TAU * i as f32 / n as f32;
                vec2(sx * t.cos(), sy * t.sin())
            })
            .collect(),
    }
}

/// The cards at `slots` keep the courtyard between them and stay in the
/// field.
pub(crate) fn fits(slots: &[Vec2], size: Vec2) -> bool {
    let half = vec2(CARD_W, CARD_H) * 0.5;
    let rects: Vec<Rect> = slots.iter().map(|c| Rect::from_center_size(Pos2::ZERO + *c, vec2(CARD_W, CARD_H))).collect();
    let field = Rect::from_center_size(Pos2::ZERO, size);
    rects.iter().all(|r| field.contains_rect(*r))
        && rects.iter().enumerate().all(|(i, a)| rects[i + 1..].iter().all(|b| !a.expand(COURT * 0.5).intersects(*b)))
        && half.x > 0.0
}

/// Where the segment from `inside` toward `toward` leaves `rect` (the
/// point on its edge), `gap` further out.
pub(crate) fn edge_point(rect: Rect, toward: Pos2, gap: f32) -> Pos2 {
    let c = rect.center();
    let d = toward - c;
    if d.length() < 1e-3 {
        return c;
    }
    let tx = if d.x.abs() > 1e-6 { (rect.width() * 0.5) / d.x.abs() } else { f32::INFINITY };
    let ty = if d.y.abs() > 1e-6 { (rect.height() * 0.5) / d.y.abs() } else { f32::INFINITY };
    let t = tx.min(ty);
    c + d * t + d.normalized() * gap
}

/// The page: under its bar, the mat with the cards (and the passport beside
/// it), or a column of cards on a narrow window.
pub fn show(ui: &mut Ui, state: &ViewState, bottom_inset: f32, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    super::detail::with_bar(ui, TITLE, pages, |ui, pages| {
        let full = ui.max_rect();
        let top = ui.cursor().top();
        let area = Rect::from_min_max(pos2(full.left(), top), pos2(full.right(), (full.bottom() - bottom_inset).max(top + 200.0)));
        ui.allocate_rect(area, Sense::hover());
        let mem = &mut pages.mesh;
        // A device that went away takes its passport with it.
        if let Some(sel) = mem.map.selected.clone()
            && !sel.is_empty()
            && state.mesh.node(&sel).is_none()
        {
            mem.map.selected = None;
        }
        track_speeds(&mut mem.map, &state.mesh, anim::now(ui.ctx()));
        let wall = wall_ms();
        let devs = devices(state, wall);
        if !map_fits(area.width(), mem.map.selected.is_some()) {
            column(ui, area, state, &devs, mem, out);
        } else {
            map(ui, area, state, &devs, mem, out);
        }
        mesh_ui::overlays(ui, state, mem, out);
        None
    })
}

/// The mat of cards has room in `width` beside the passport when one is
/// open (else the column, the passport in a bottom sheet).
fn map_fits(width: f32, passport: bool) -> bool {
    let pane = if passport { PANE_W + GAP } else { 0.0 };
    width - pane >= NARROW
}

/// Each transfer's speed, smoothed, from how its bytes moved.
fn track_speeds(map: &mut MapState, m: &MeshView, now: f64) {
    map.speeds.retain(|id, _| m.node(id).is_some_and(|n| n.transfer.is_some()));
    for n in &m.nodes {
        let Some(t) = n.transfer else { continue };
        let e = map.speeds.entry(n.id.clone()).or_insert((t.done, now, 0.0));
        if t.done != e.0 {
            let dt = (now - e.1).max(0.05);
            let inst = t.done.saturating_sub(e.0) as f64 / dt;
            e.2 = if e.2 == 0.0 { inst } else { e.2 * 0.7 + inst * 0.3 };
            e.0 = t.done;
            e.1 = now;
        } else if now - e.1 > 3.0 {
            // Nothing came for a while: the speed falls.
            e.2 *= 0.5;
            e.1 = now;
        }
    }
}

/// «48,2 МиБ/с».
fn speed_text(bytes_per_s: f64) -> String {
    let kbps = (bytes_per_s / 1024.0).round().clamp(1.0, f64::from(u32::MAX)) as u32;
    super::settings::fmt_speed(kbps)
}

/// The wide page: the mat, the passport beside it.
fn map(ui: &mut Ui, area: Rect, state: &ViewState, devs: &[Dev<'_>], mem: &mut MeshUi, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let open = mem.map.selected.is_some();
    let pane_k = anim::fraction(&ctx, egui::Id::new("mesh-map-pane"), if open { 1.0 } else { 0.0 }, motion.default_spatial).clamp(0.0, 1.05);
    let mat_w = area.width() - (PANE_W + GAP) * pane_k.min(1.0);
    let mat = Rect::from_min_size(area.min, vec2(mat_w.max(NARROW * 0.5), area.height()));
    paint::container(ui.painter(), mat, Radii::all(28.0), s.surface_container_lowest, Stroke::NONE);
    let field = Rect::from_min_max(mat.min + vec2(16.0, 16.0), pos2(mat.right() - 16.0, mat.bottom() - FOOT_H - 8.0));
    let mut place = slots(devs.len(), field.size());
    // Too many for the field: a ring as wide as it goes, the cards may
    // touch rather than leave it.
    if !fits(&place, field.size()) && devs.len() >= 2 {
        place = slots(devs.len().max(5), field.size()).into_iter().take(devs.len()).collect();
    }
    // Every card moves to its place on a spring (a device that comes or
    // goes, a window that resizes).
    let centres: Vec<Pos2> = devs
        .iter()
        .zip(&place)
        .map(|(d, off)| {
            let id = egui::Id::new(("mesh-map-card-pos", d.id));
            let target = field.center() + *off;
            pos2(anim::f32(&ctx, id.with("x"), target.x, motion.default_spatial), anim::f32(&ctx, id.with("y"), target.y, motion.default_spatial))
        })
        .collect();
    let heights: Vec<f32> = devs
        .iter()
        .map(|d| anim::f32(&ctx, egui::Id::new(("mesh-map-card-h", d.id)), if d.playing.is_some() { CARD_H } else { CARD_SHORT }, motion.default_spatial))
        .collect();
    let rects: Vec<Rect> = centres.iter().zip(&heights).map(|(c, h)| Rect::from_center_size(*c, vec2(CARD_W, *h))).collect();
    // What flows, under the cards.
    if let Some(this) = rects.first().copied() {
        for (i, d) in devs.iter().enumerate().skip(1) {
            links(ui, this, rects[i], d, mem);
        }
    }
    let mut clicked = None;
    for (i, d) in devs.iter().enumerate() {
        let selected = mem.map.selected.as_deref() == Some(d.id);
        let (r, act) = card(ui, rects[i], d, selected, &state.mesh);
        if r.clicked() {
            clicked = Some(d.id.to_owned());
        }
        if let Some(a) = act {
            mesh_ui::act(ui, &state.mesh, d.node, a, mem, state, out);
        }
    }
    if let Some(id) = clicked {
        mem.map.selected = if mem.map.selected.as_deref() == Some(id.as_str()) { None } else { Some(id) };
    }
    foot(ui, Rect::from_min_max(pos2(mat.left() + 16.0, mat.bottom() - FOOT_H), pos2(mat.right() - 16.0, mat.bottom() - 8.0)), state, mem, out);
    if pane_k > 0.01 {
        let pane = Rect::from_min_size(pos2(mat.right() + GAP, area.top()), vec2(PANE_W, area.height()));
        let mut pui = ui.new_child(UiBuilder::new().max_rect(pane).id_salt("mesh-map-pane-ui"));
        pui.set_clip_rect(pane.intersect(ui.clip_rect()));
        pui.multiply_opacity(pane_k.min(1.0));
        let sel = mem.map.selected.clone();
        SupportingPane::new().padding(20.0).show(&mut pui, |ui| {
            if let Some(d) = sel.as_deref().and_then(|id| devs.iter().find(|d| d.id == id)) {
                passport(ui, state, d, mem, out);
            }
        });
    }
}

/// A narrow window: the cards one under another, then the nearby players;
/// the passport rises as a sheet.
fn column(ui: &mut Ui, area: Rect, state: &ViewState, devs: &[Dev<'_>], mem: &mut MeshUi, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let mut cui = ui.new_child(UiBuilder::new().max_rect(area).layout(Layout::top_down(Align::Center)).id_salt("mesh-map-column"));
    egui::ScrollArea::vertical().id_salt("mesh-map-column-scroll").auto_shrink([false, false]).show(&mut cui, |ui| {
        let w = ui.available_width().min(480.0);
        let mut clicked = None;
        for d in devs {
            let h = anim::f32(&ctx, egui::Id::new(("mesh-map-card-h", d.id)), if d.playing.is_some() { CARD_H } else { CARD_SHORT }, Theme::get(&ctx).motion.default_spatial);
            let (slot, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
            let selected = mem.map.selected.as_deref() == Some(d.id);
            let (r, act) = card(ui, slot, d, selected, &state.mesh);
            if r.clicked() {
                clicked = Some(d.id.to_owned());
            }
            if let Some(a) = act {
                mesh_ui::act(ui, &state.mesh, d.node, a, mem, state, out);
            }
            ui.add_space(12.0);
        }
        if let Some(id) = clicked {
            mem.map.selected = Some(id);
        }
        let (foot_rect, _) = ui.allocate_exact_size(vec2(w, FOOT_H), Sense::hover());
        foot(ui, foot_rect, state, mem, out);
    });
    let mut open = mem.map.selected.is_some();
    let sel = mem.map.selected.clone();
    let _ = BottomSheet::new("mesh-map-passport-sheet").show(&ctx, &mut open, |ui| {
        if let Some(d) = sel.as_deref().and_then(|id| devs.iter().find(|d| d.id == id)) {
            passport(ui, state, d, mem, out);
        }
    });
    if !open {
        mem.map.selected = None;
    }
}

/// One card at `rect`. Its response (a click opens the passport) and what
/// its ⋮ asked for.
fn card(ui: &mut Ui, rect: Rect, d: &Dev<'_>, selected: bool, m: &MeshView) -> (egui::Response, Option<Act>) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = egui::Id::new(("mesh-map-card", d.id));
    // The card where the music plays is the tonal one, a little larger
    // and rounder; out of reach it sinks into the mat.
    let (fill, radius, scale, lift) = if d.look.playing {
        (s.secondary_container, 28.0, 1.02, true)
    } else if d.look.online {
        (s.surface_container_low, 24.0, 1.0, true)
    } else {
        (s.surface_container, 24.0, 1.0, false)
    };
    let fill = anim::color(&ctx, id.with("fill"), fill, motion.default_effects);
    let radius = anim::f32(&ctx, id.with("r"), radius, motion.fast_spatial);
    let scale = anim::f32(&ctx, id.with("scale"), scale, motion.fast_spatial);
    let rect = Rect::from_center_size(rect.center(), rect.size() * scale);
    let response = ui.interact(rect, id, Sense::click());
    let radii = Radii::all(radius).clamped(rect);
    if lift {
        paint::shadow(ui.painter(), rect, radii, Elevation::Level1);
    }
    paint::container(ui.painter(), rect, radii, fill, Stroke::NONE);
    let layer = anim::fraction(
        &ctx,
        id.with("layer"),
        if response.is_pointer_button_down_on() { state::PRESSED } else if response.hovered() { state::HOVER } else { 0.0 },
        motion.fast_effects,
    );
    paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);
    let sel = anim::fraction(&ctx, id.with("sel"), if selected { 1.0 } else { 0.0 }, motion.fast_effects);
    if sel > 0.01 {
        ui.painter().rect_stroke(rect, radius, Stroke::new(2.0, s.primary.gamma_multiply(sel)), egui::StrokeKind::Inside);
    }
    let inner = rect.shrink(CARD_PAD);
    let mut clip_ui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("mesh-map-card-ui", d.id)));
    clip_ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    // Top: the shape, the name, «Гость», ⋮.
    let top = Rect::from_min_size(inner.min, vec2(inner.width(), 36.0));
    mesh_ui::paint_avatar(&clip_ui, pos2(top.left() + 18.0, top.center().y), 36.0, &format!("map-{}", d.id), d.kind, d.look);
    let mut right = clip_ui.new_child(UiBuilder::new().max_rect(top).layout(Layout::right_to_left(Align::Center)).id_salt(("mesh-map-card-top", d.id)));
    right.spacing_mut().item_spacing.x = 4.0;
    let act = mesh_ui::more_menu(&mut right, &format!("map-{}", d.id), d.node.is_none(), m.ring_holder);
    let guest = d.node.is_some_and(|n| n.scope == nikgob_view::MeshScope::Guest);
    if guest {
        guest_mark(&mut right);
    }
    let name_x = top.left() + 48.0;
    let name_w = (right.min_rect().left() - 8.0 - name_x).max(8.0);
    let name_color = if d.look.online { s.on_surface } else { s.on_surface_variant };
    let g = mesh_ui::text_line(clip_ui.painter(), &d.name, TypeRole::TitleMedium, name_color, name_w);
    clip_ui.painter().galley(pos2(name_x, top.center().y - g.size().y * 0.5), g, name_color);
    // Middle: what plays, while something does (the card grew for it).
    let grown = ((rect.height() / scale - CARD_SHORT) / (CARD_H - CARD_SHORT)).clamp(0.0, 1.0);
    if let Some((title, artists, progress)) = &d.playing
        && grown > 0.05
    {
        let plate = Rect::from_min_size(pos2(inner.left(), top.bottom() + 10.0), vec2(inner.width(), 56.0));
        let mut p = clip_ui.painter().clone();
        p.set_opacity(grown);
        let plate_fill = if d.look.playing { s.surface_container_high } else { s.surface_container_highest };
        paint::container(&p, plate, Radii::all(14.0), plate_fill, Stroke::NONE);
        let bars = Rect::from_center_size(pos2(plate.left() + 24.0, plate.center().y - 2.0), vec2(24.0, 24.0));
        let mut eq_ui = clip_ui.new_child(UiBuilder::new().max_rect(plate));
        eq_ui.multiply_opacity(grown);
        crate::components::Equalizer::new(("mesh-map-eq", d.id), s.primary).size(22.0).playing(d.look.playing).paint_at(&eq_ui, bars);
        let tx = plate.left() + 48.0;
        let tw = plate.right() - 12.0 - tx;
        let t = mesh_ui::text_line(&p, title, TypeRole::TitleSmall, s.on_surface, tw);
        p.galley(pos2(tx, plate.top() + 8.0), t, s.on_surface);
        let a = mesh_ui::text_line(&p, artists, TypeRole::BodySmall, s.on_surface_variant, tw);
        p.galley(pos2(tx, plate.top() + 28.0), a, s.on_surface_variant);
        if let Some(f) = progress {
            let (x0, x1, y) = (plate.left() + 12.0, plate.right() - 12.0, plate.bottom() - 4.0);
            let xf = x0 + (x1 - x0) * *f;
            p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(2.0, s.secondary_container));
            p.line_segment([pos2(x0, y), pos2(xf.max(x0), y)], Stroke::new(2.0, s.primary));
        }
        if d.look.playing {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
    // Bottom: the storage bar and its number.
    let bottom = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 28.0), inner.max);
    if let Some((music, masters)) = d.bar {
        mesh_ui::paint_bar(&clip_ui, Rect::from_min_size(bottom.min, vec2(bottom.width(), 6.0)), music, masters);
    }
    if let Some((text, error)) = &d.number {
        let color = if *error { s.error } else { s.on_surface_variant };
        let w = if d.node.is_some() { bottom.width() * 0.62 } else { bottom.width() };
        let g = mesh_ui::text_line(clip_ui.painter(), text, TypeRole::LabelSmall, color, w);
        clip_ui.painter().galley(pos2(bottom.left(), bottom.bottom() - g.size().y), g, color);
    }
    if let Some(n) = d.node {
        let g = mesh_ui::text_line(clip_ui.painter(), n.profile.label(), TypeRole::LabelSmall, s.on_surface_variant, bottom.width() * 0.36);
        clip_ui.painter().galley(pos2(bottom.right() - g.size().x, bottom.bottom() - g.size().y), g, s.on_surface_variant);
    }
    (response, act)
}

/// «Гость» on a guest's card: a small tonal mark.
fn guest_mark(ui: &mut Ui) {
    let s = Theme::get(ui.ctx()).scheme;
    let g = ui.painter().layout_no_wrap("Гость".to_owned(), TypeRole::LabelSmall.font_id(), s.on_tertiary_container);
    let (rect, _) = ui.allocate_exact_size(vec2(g.size().x + 12.0, 20.0), Sense::hover());
    ui.painter().rect_filled(rect, 10.0, s.tertiary_container);
    ui.painter().galley(rect.center() - g.size() * 0.5, g, s.on_tertiary_container);
}

/// What flows between this computer (`from`) and device `d` (`to`): the
/// wave of files with their speed, the arc of music playing
/// there. Each grows from this computer and fades away when it ends.
fn links(ui: &mut Ui, from: Rect, to: Rect, d: &Dev<'_>, mem: &MeshUi) {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let Some(n) = d.node else { return };
    let id = egui::Id::new(("mesh-map-link", d.id));
    let a = edge_point(from, to.center(), 8.0);
    let b = edge_point(to, from.center(), 8.0);
    // Files: a wave, its phase running toward the device.
    let files = anim::fraction(&ctx, id.with("files"), if n.transfer.is_some() { 1.0 } else { 0.0 }, motion.default_spatial).clamp(0.0, 1.0);
    if files > 0.002 {
        let paused = n.transfer.is_some_and(|t| t.paused);
        let running = !paused && n.transfer.is_some() && !theme.reduced_motion;
        let calm = anim::fraction(&ctx, id.with("calm"), if running { 1.0 } else { 0.0 }, motion.default_effects);
        let phase = mesh_ui::spin(&ctx, id.with("phase"), running, 1.0 / 0.8);
        let end = a + (b - a) * files;
        mesh_ui::wave_between(ui, a, end, 3.0 * calm, 36.0, phase, Stroke::new(4.0, s.primary.gamma_multiply(files)));
        // The capsule on the line: the speed, while bytes come.
        let speed = mem.map.speeds.get(&n.id).map_or(0.0, |e| e.2);
        if files > 0.6 && !paused && speed > 0.0 {
            let alpha = ((files - 0.6) / 0.4).clamp(0.0, 1.0);
            let g = ui.painter().layout_no_wrap(speed_text(speed), TypeRole::LabelSmall.font_id(), s.on_surface);
            let mid = a + (b - a) * 0.5;
            let cap = Rect::from_center_size(mid, vec2(g.size().x + 24.0, 28.0));
            let mut p = ui.painter().clone();
            p.set_opacity(alpha);
            paint::shadow(&p, cap, Radii::all(14.0), Elevation::Level2);
            paint::container(&p, cap, Radii::all(14.0), s.surface_container_high, Stroke::NONE);
            p.galley(cap.center() - g.size() * 0.5, g, s.on_surface);
        }
    }
    // Music there: a calm arc bowed away from the courtyard, a dot running
    // along it.
    let sound = anim::fraction(&ctx, id.with("sound"), if d.look.playing { 1.0 } else { 0.0 }, motion.slow_effects).clamp(0.0, 1.0);
    if sound > 0.002 {
        let mid = a + (b - a) * 0.5;
        let dir = b - a;
        let normal = vec2(-dir.y, dir.x).normalized();
        let bow = (dir.length() * 0.18).min(64.0);
        let ctrl = mid + normal * bow;
        let pts: Vec<Pos2> = (0..=32).map(|i| bezier(a, ctrl, b, i as f32 / 32.0)).collect();
        ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0, s.tertiary.gamma_multiply(0.7 * sound))));
        if !theme.reduced_motion {
            let t = (anim::now(&ctx) / 2.4).fract() as f32;
            ui.painter().circle_filled(bezier(a, ctrl, b, t), 3.0, s.tertiary.gamma_multiply(sound));
            ctx.request_repaint();
        }
    }
}

fn bezier(a: Pos2, c: Pos2, b: Pos2, t: f32) -> Pos2 {
    let u = 1.0 - t;
    pos2(u * u * a.x + 2.0 * u * t * c.x + t * t * b.x, u * u * a.y + 2.0 * u * t * c.y + t * t * b.y)
}

/// The mat's foot: the unpaired players nearby, each with «Подключить»,
/// and «Добавить устройство» at the end, on one line.
fn foot(ui: &mut Ui, rect: Rect, state: &ViewState, mem: &mut MeshUi, out: &mut Vec<Command>) {
    let m = &state.mesh;
    let s = Theme::get(ui.ctx()).scheme;
    let mut fui = ui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)).id_salt("mesh-map-foot"));
    fui.spacing_mut().item_spacing.x = 8.0;
    let can = m.can_pair();
    for n in &m.nearby {
        let name_g = mesh_ui::text_line(fui.painter(), &n.name, TypeRole::LabelLarge, s.on_surface, 160.0);
        let connect_w = 112.0;
        let w = 8.0 + 28.0 + 8.0 + name_g.size().x + 8.0 + connect_w;
        let (pill, _) = fui.allocate_exact_size(vec2(w, 44.0), Sense::hover());
        paint::container(fui.painter(), pill, Radii::all(22.0), s.surface, Stroke::new(1.0, s.outline_variant));
        mesh_ui::paint_avatar(&fui, pos2(pill.left() + 8.0 + 14.0, pill.center().y), 28.0, &format!("map-nearby-{}", n.id), n.kind, Look::default());
        fui.painter().galley(pos2(pill.left() + 44.0, pill.center().y - name_g.size().y * 0.5), name_g, s.on_surface);
        let br = Rect::from_min_max(pos2(pill.right() - connect_w, pill.top()), pill.max);
        let mut b = fui.new_child(UiBuilder::new().max_rect(br).layout(Layout::right_to_left(Align::Center)).id_salt(("mesh-map-connect", &n.id)));
        let r = b.add(Button::new("Подключить").style(Style::Text).size(Size::S).enabled(can).id_salt(("mesh-map-connect-btn", &n.id)));
        if r.clicked() {
            add_device::open_pair(ui.ctx(), m, mem, Some(r.rect), &n.id, out);
        }
    }
    let mut end = fui.new_child(UiBuilder::new().max_rect(rect).layout(Layout::right_to_left(Align::Center)).id_salt("mesh-map-foot-end"));
    let r = end.add(Button::new("Добавить устройство").icon(Icon::Add).style(Style::Tonal).size(Size::S).enabled(can).id_salt("mesh-map-add"));
    if r.clicked() {
        add_device::open(ui.ctx(), m, mem, Some(r.rect), AddTab::Connect, out);
    }
}

/// The passport of a device: its name (with «Переименовать»), kind, what
/// it stores and how full it is, what plays there, the facts, «Гость» and
/// «Забыть».
fn passport(ui: &mut Ui, state: &ViewState, d: &Dev<'_>, mem: &mut MeshUi, out: &mut Vec<Command>) {
    let m = &state.mesh;
    let s = Theme::get(ui.ctx()).scheme;
    let salt = format!("passport-{}", d.id);
    // Head: the shape, the name, rename, close.
    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::hover());
    mesh_ui::paint_avatar(ui, pos2(head.left() + 24.0, head.center().y), 48.0, &salt, d.kind, d.look);
    let mut right = ui.new_child(UiBuilder::new().max_rect(head).layout(Layout::right_to_left(Align::Center)).id_salt(("mesh-passport-head", d.id)));
    right.spacing_mut().item_spacing.x = 4.0;
    if right.add(IconButton::new(Icon::Close).size(Size::S).tooltip("Закрыть").id_salt(("mesh-passport-close", d.id))).clicked() {
        mem.map.selected = None;
    }
    let mut act = None;
    if right.add(IconButton::new(Icon::Edit).size(Size::S).tooltip("Переименовать").id_salt(("mesh-passport-rename", d.id))).clicked() {
        act = Some(Act::Rename);
    }
    let x = head.left() + 64.0;
    let w = (right.min_rect().left() - 8.0 - x).max(8.0);
    let name = mesh_ui::text_line(ui.painter(), &d.name, TypeRole::TitleLarge, s.on_surface, w);
    let kind = mesh_ui::text_line(ui.painter(), kind_label(d.kind), TypeRole::BodyMedium, s.on_surface_variant, w);
    ui.painter().galley(pos2(x, head.center().y - 24.0), name, s.on_surface);
    ui.painter().galley(pos2(x, head.center().y + 4.0), kind, s.on_surface_variant);
    ui.add_space(20.0);
    // What it stores, and how full it is.
    if let Some(n) = d.node {
        fact(ui, "Хранит", |ui| {
            if let Some(a) = mesh_ui::profile_chip(ui, &salt, n, true) {
                act = Some(a);
            }
        });
    }
    if let Some((music, masters)) = d.bar {
        ui.add_space(8.0);
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
        mesh_ui::paint_bar(ui, bar, music, masters);
    }
    if let Some((text, error)) = &d.number {
        ui.add_space(6.0);
        ui.label(TypeRole::LabelLarge.style().rich(text, if *error { s.error } else { s.on_surface_variant }));
    }
    if let Some(n) = d.node
        && let Some(st) = n.storage
        && n.profile.stores()
        && st.free >= mesh_ui::LOW_FREE
    {
        ui.label(TypeRole::BodyMedium.style().rich(format!("свободно {}", nikgob_view::free_ru(st.free)), s.on_surface_variant));
    }
    // What plays there.
    if let Some((title, artists, progress)) = &d.playing {
        ui.add_space(16.0);
        fact(ui, "Играет", |ui| {
            ui.vertical(|ui| {
                ui.add(egui::Label::new(TypeRole::BodyLarge.style().rich(title, s.on_surface)).truncate());
                if !artists.is_empty() {
                    ui.add(egui::Label::new(TypeRole::BodyMedium.style().rich(artists, s.on_surface_variant)).truncate());
                }
                if let Some(f) = progress {
                    ui.add_space(6.0);
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
                    ui.painter().rect_filled(r, 2.0, s.secondary_container);
                    ui.painter().rect_filled(Rect::from_min_size(r.min, vec2(r.width() * *f, r.height())), 2.0, s.primary);
                }
            });
        });
    }
    listen(ui, state, d, out);
    ui.add_space(16.0);
    let version = match d.node {
        Some(n) => n.version.clone(),
        None => m.this.version.clone().or_else(|| Some(env!("CARGO_PKG_VERSION").to_owned())),
    };
    if let Some(v) = version {
        fact_text(ui, "Версия", &v);
    }
    let addr = d.node.map_or_else(|| m.this.addr.clone(), |n| n.addr.clone());
    if let Some(a) = addr {
        fact_text(ui, "Адрес", &a);
    }
    if let Some(n) = d.node
        && !n.online
        && let Some(at) = n.last_seen_ms
    {
        fact_text(ui, "Был в сети", &super::settings::ago(wall_ms(), at));
    }
    if let Some(n) = d.node {
        ui.add_space(16.0);
        if let Some(a) = mesh_ui::guest_chip(ui, &salt, n, m.ring_holder, mem) {
            act = Some(a);
        }
        ui.add_space(24.0);
        let forget = Button::new("Забыть устройство").icon(Icon::Delete).style(Style::Outlined).size(Size::S).enabled(m.ring_holder).id_salt(("mesh-passport-forget", d.id));
        if ui.add(forget).clicked() {
            act = Some(Act::Forget);
        }
    }
    if let Some(a) = act {
        mesh_ui::act(ui, m, d.node, a, mem, state, out);
    }
}

/// What «Слушать здесь» and «Пульт» of a passport do: the music of this
/// window moves to that device (this computer's passport goes back here
/// while another device plays: a detach after an attach, the music brought
/// back after a transfer); this window drives what the device plays, as it
/// is. Neither for a device out of reach or the one that plays the
/// window's music already.
fn listen_commands(state: &ViewState, node: Option<&MeshNodeView>) -> (Option<Command>, Option<Command>) {
    let target = state.target.as_ref();
    match node {
        None => (crate::screens::output::back_here(state), None),
        Some(n) if !n.online || target.is_some_and(|t| t.node_id == n.id) => (None, None),
        Some(n) => (
            Some(Command::OutputTransfer { node_id: Some(n.id.clone()) }),
            n.playing.is_some().then(|| Command::OutputAttach { node_id: n.id.clone() }),
        ),
    }
}

/// «Слушать здесь» · «Пульт» of a passport ([`listen_commands`]).
fn listen(ui: &mut Ui, state: &ViewState, d: &Dev<'_>, out: &mut Vec<Command>) {
    let (here, remote) = listen_commands(state, d.node);
    if here.is_none() && remote.is_none() {
        return;
    }
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        if let Some(c) = here
            && ui.add(Button::new("Слушать здесь").icon(Icon::SpeakerGroup).style(Style::Filled).size(Size::S).id_salt(("mesh-passport-here", d.id))).clicked()
        {
            out.push(c);
        }
        if let Some(c) = remote
            && ui.add(Button::new("Пульт").style(Style::Tonal).size(Size::S).id_salt(("mesh-passport-remote", d.id))).clicked()
        {
            out.push(c);
        }
    });
}

/// A fact of the passport: its name above, what it is under it.
fn fact(ui: &mut Ui, label: &str, value: impl FnOnce(&mut Ui)) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.label(TypeRole::LabelLarge.style().rich(label, s.on_surface_variant));
    ui.add_space(4.0);
    value(ui);
    ui.add_space(8.0);
}

fn fact_text(ui: &mut Ui, label: &str, value: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    fact(ui, label, |ui| {
        ui.add(egui::Label::new(TypeRole::BodyLarge.style().rich(value, s.on_surface)).truncate());
    });
}

/// The number a device's chip of storage shows on the map, for its
/// profile (tests).
#[cfg(test)]
fn profile_word(p: nikgob_view::MeshProfile) -> &'static str {
    p.label()
}

#[cfg(test)]
mod tests {
    use nikgob_view::{MeshStorage, MeshTransfer};

    use super::*;

    /// Two to five cards keep the courtyard between them and stay on the
    /// mat at a usual window size; the first (this computer) stands left
    /// or on top.
    #[test]
    fn the_cards_stand_apart_around_the_courtyard() {
        let field = vec2(1000.0, 620.0);
        for n in 1..=5 {
            let p = slots(n, field);
            assert_eq!(p.len(), n);
            assert!(fits(&p, field), "{n} cards: {p:?}");
        }
        assert!(slots(2, field)[0].x < 0.0, "this computer on the left");
        assert!(slots(3, field)[0].y < 0.0 && slots(3, field)[0].x < 0.0, "top left");
        assert!(slots(5, field)[0].y < 0.0 && slots(5, field)[0].x.abs() < 1.0, "on top of the ring");
        // A field too small for two side by side does not fit them.
        assert!(!fits(&slots(2, vec2(420.0, 300.0)), vec2(420.0, 300.0)));
    }

    #[test]
    fn a_line_leaves_a_card_at_its_edge() {
        let r = Rect::from_center_size(pos2(0.0, 0.0), vec2(240.0, 168.0));
        let p = edge_point(r, pos2(1000.0, 0.0), 8.0);
        assert!((p.x - 128.0).abs() < 0.01 && p.y.abs() < 0.01, "{p:?}");
        let q = edge_point(r, pos2(0.0, -1000.0), 0.0);
        assert!((q.y + 84.0).abs() < 0.01, "{q:?}");
        assert_eq!(edge_point(r, pos2(0.0, 0.0), 8.0), pos2(0.0, 0.0));
    }

    #[test]
    fn a_transfer_has_a_speed_and_it_falls_when_nothing_comes() {
        let mut map = MapState::default();
        let mut m = MeshView::default();
        m.nodes.push(MeshNodeView { id: "pi".into(), transfer: Some(MeshTransfer { done: 0, total: 100 << 20, paused: false }), ..MeshNodeView::default() });
        track_speeds(&mut map, &m, 0.0);
        m.nodes[0].transfer = Some(MeshTransfer { done: 10 << 20, total: 100 << 20, paused: false });
        track_speeds(&mut map, &m, 1.0);
        let v = map.speeds["pi"].2;
        assert!((v - f64::from(10u32 << 20)).abs() < 1.0, "{v}");
        assert_eq!(speed_text(v), "10,0 МиБ/с");
        track_speeds(&mut map, &m, 5.0);
        assert!(map.speeds["pi"].2 < v);
        m.nodes[0].transfer = None;
        track_speeds(&mut map, &m, 6.0);
        assert!(map.speeds.is_empty(), "no transfer, no speed");
    }

    /// The map's devices: this computer first with its own player, then
    /// the paired ones with what plays there.
    /// «Слушать здесь» moves the music to a device in reach (back here
    /// from this computer's passport while another device plays), «Пульт»
    /// only drives a device that has music of its own; nothing for the
    /// device that plays the window's music already.
    #[test]
    fn the_passport_moves_the_music_or_drives_it() {
        let mut state = ViewState::default();
        let pi = MeshNodeView { id: "pi".into(), online: true, playing: Some(nikgob_view::MeshPlaying::default()), ..MeshNodeView::default() };
        let tv = MeshNodeView { id: "tv".into(), online: true, ..MeshNodeView::default() };
        let off = MeshNodeView { id: "off".into(), online: false, ..MeshNodeView::default() };
        assert_eq!(listen_commands(&state, None), (None, None));
        assert_eq!(
            listen_commands(&state, Some(&pi)),
            (Some(Command::OutputTransfer { node_id: Some("pi".into()) }), Some(Command::OutputAttach { node_id: "pi".into() }))
        );
        assert_eq!(listen_commands(&state, Some(&tv)), (Some(Command::OutputTransfer { node_id: Some("tv".into()) }), None));
        assert_eq!(listen_commands(&state, Some(&off)), (None, None));
        state.target = Some(nikgob_view::RemoteTarget {
            node_id: "pi".into(),
            name: "Pi".into(),
            kind: MeshKind::Pi,
            link: nikgob_view::RemoteLink::Live,
            volume: 0.5,
            muted: false,
            how: nikgob_view::RemoteHow::Transferred,
        });
        assert_eq!(listen_commands(&state, Some(&pi)), (None, None));
        assert_eq!(listen_commands(&state, None), (Some(Command::OutputTransfer { node_id: None }), None));
        // Driven by an attach: back here is this computer's own queue.
        state.target.as_mut().unwrap().how = nikgob_view::RemoteHow::Attached;
        assert_eq!(listen_commands(&state, None), (Some(Command::OutputDetach), None));
    }

    #[test]
    fn the_map_lists_this_computer_first() {
        let mut state = ViewState::default();
        state.mesh.nodes.push(MeshNodeView {
            id: "pi".into(),
            name: "Pi".into(),
            kind: MeshKind::Pi,
            online: true,
            storage: Some(MeshStorage { used: 10 << 30, free: 90 << 30 }),
            playing: Some(nikgob_view::MeshPlaying { title: "Song".into(), artists: vec!["A".into(), "B".into()], ..Default::default() }),
            ..MeshNodeView::default()
        });
        let devs = devices(&state, 0);
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].id, "");
        assert_eq!(devs[1].playing.as_ref().map(|p| p.1.as_str()), Some("A, B"));
        assert!(devs[1].look.playing);
        assert_eq!(devs[1].bar.map(|b| (b.0 * 10.0).round()), Some(1.0));
        assert_eq!(profile_word(nikgob_view::MeshProfile::Full), "Полная копия");
    }

    /// The cards keep the room left beside an open passport: a window of
    /// 700 dp shows the column then, 1100 dp keeps the mat.
    #[test]
    fn the_passport_takes_its_room_from_the_mat() {
        assert!(map_fits(700.0, false));
        assert!(!map_fits(700.0, true), "320 dp left: two cards would overlap");
        assert!(map_fits(1100.0, true));
        assert!(!map_fits(600.0, false));
    }

    /// The page draws on a wide and on a narrow window and sends nothing by
    /// drawing.
    #[test]
    fn the_page_draws_wide_and_narrow() {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        let mut state = ViewState::default();
        state.mesh.running = true;
        state.mesh.ring_holder = true;
        state.mesh.paired = true;
        state.mesh.nodes.push(MeshNodeView {
            id: "pi".into(),
            name: "Pi".into(),
            kind: MeshKind::Pi,
            online: true,
            transfer: Some(MeshTransfer { done: 1, total: 2, paused: false }),
            ..MeshNodeView::default()
        });
        let mut pages = PageState::default();
        pages.mesh.map.select("pi".into());
        let mut out = Vec::new();
        for (i, w) in [1400.0_f32, 1400.0, 500.0, 500.0].into_iter().enumerate() {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 900.0))),
                time: Some(i as f64 * 0.1),
                ..Default::default()
            };
            let mut done = ctx.run_ui(input, |ui| {
                let _ = show(ui, &state, 80.0, &mut pages, &mut out);
            });
            done.textures_delta.clear();
        }
        assert!(out.is_empty(), "{out:?}");
    }
}
