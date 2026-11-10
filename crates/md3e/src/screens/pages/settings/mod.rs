//! Настройки (settings-v2-concept.md): one page, six tabs — Музыка ·
//! Тексты · Вид · Сервисы · Загрузки · Система — and one switch, Просто ·
//! Подробно (kept in the prefs). «Подробно» shows the same tabs, groups and
//! rows in the same order, with the rest of the rows between them; a row
//! never moves when the mode changes.
//!
//! The title line and the tabs stay; under them the rows scroll. Beside
//! the rows of «Тексты» and «Вид» stands the live stage (screens/stage.rs
//! draws it where the page asks): on the right of a wide page, as a strip
//! over the rows of a narrow one, none in mini mode. It follows the row
//! under the pointer at once (the option under the pointer shown before it
//! is picked) and restores the frame the pointer leaves the rows.
//!
//! Every change is sent as a whole new `Prefs` (the recommendations' rows
//! and the presets as their patches); Ctrl+Z steps back through this
//! visit's changes.

mod dev;
mod downloads;
mod look;
mod lyrics;
mod music;
mod preset;
mod rows;
mod services;
mod system;

use egui::{Align2, Key, Modifiers, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use nikgob_view::{Command, CoverShape, PrefPatch, Prefs, ViewState};

pub(super) use rows::fmt_speed;
pub(crate) use services::{SoloistDialog, open_url};

use super::settings_help::{self, DrawnRow, Level, SettingId as Id, TABS};
use super::{PageState, brand, connected};
use crate::screens::stage::{self, Scene, Script, StageRequest};
use crate::{
    Theme, TypeRole,
    components::{Divider, Size, anim},
};

/// How far the page's rows are scrolled, for the history's scroll restore.
pub(super) struct Scrolled {
    pub offset: f32,
    /// The furthest they can be scrolled.
    pub max: f32,
}

/// What Settings keeps between frames of one visit.
#[derive(Default)]
pub(crate) struct Memory {
    /// The screen each stage tab showed last (Тексты, Вид).
    scenes: [Option<Scene>; 2],
    /// What the stage shows for a row: kept across a gap between rows, so
    /// moving to the next one does not flash the saved settings, and dropped
    /// the frame the pointer leaves the rows.
    shown: Option<Shown>,
    /// The row a drag started on.
    changed: Option<(Id, f64)>,
    /// The prefs before each change of this visit, for Ctrl+Z.
    undo: Vec<Prefs>,
    /// The press going on already made its step (a slider's drag is one).
    held: bool,
    /// The mode just picked, until the prefs bring it back.
    deep_sent: Option<bool>,
}

/// What the stage was last asked to show for a row.
#[derive(Clone, Debug)]
struct Shown {
    id: Id,
    /// The option under the pointer (worked out again from the page's
    /// settings, so a click shows what it picked).
    opt: Option<usize>,
    /// A preset's settings.
    offer: Option<Prefs>,
    /// When the pointer was last on the rows.
    at: f64,
}

/// Ctrl+Z goes back this many changes at most.
const UNDO: usize = 20;

/// The page's parts under the tabs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Arranged {
    /// The rows' scroll area.
    pub rows: Rect,
    /// How wide the rows are inside it (a lone column stops at 840).
    pub content_w: f32,
    pub stage: Option<Rect>,
    pub buttons: Option<Rect>,
}

/// Between the rows and the stage, and around the stage.
const STAGE_GAP: f32 = 24.0;
/// From this page width the stage stands beside the rows. The concept
/// says 1000; a 1280 window with the rail open leaves the page 976, and
/// there the stage still fits on the right.
const WIDE: f32 = 960.0;

/// Where the rows and the stage go in `room` (the page under the tabs) of
/// a window `window` big: a wide page puts the rows on the left (42 %,
/// 460–600) and the stage on the right in the window's proportions; a
/// narrow one (600 up to [`WIDE`]) a strip over the rows (36 % high,
/// 180–300); mini mode and the tabs without a stage, the rows alone.
pub(crate) fn arrange(room: Rect, window: Vec2, stage: bool) -> Arranged {
    let buttons_h = Size::S.height();
    if !stage || room.width() < 600.0 {
        return Arranged { rows: room, content_w: room.width().min(840.0), stage: None, buttons: None };
    }
    if room.width() >= WIDE {
        let rows_w = (room.width() * 0.42).clamp(460.0, 600.0);
        let rows = Rect::from_min_size(room.min, vec2(rows_w, room.height()));
        let left = rows.right() + STAGE_GAP;
        let w = (room.right() - left).max(0.0);
        let aspect = (window.y / window.x.max(1.0)).clamp(0.4, 1.0);
        let h = (w * aspect).min(room.height() - 8.0 - 12.0 - buttons_h - STAGE_GAP).max(0.0);
        let stage = Rect::from_min_size(pos2(left, room.top() + 8.0), vec2(w, h));
        let buttons = Rect::from_min_size(pos2(left, stage.bottom() + 12.0), vec2(w, buttons_h));
        return Arranged { rows, content_w: rows_w, stage: Some(stage), buttons: Some(buttons) };
    }
    let h = (room.height() * 0.36).clamp(180.0, 300.0);
    let stage = Rect::from_min_size(room.min + vec2(0.0, 8.0), vec2(room.width(), h));
    let buttons = Rect::from_min_size(pos2(room.left(), stage.bottom() + 12.0), vec2(room.width(), buttons_h));
    let rows = Rect::from_min_max(pos2(room.left(), (buttons.bottom() + 8.0).min(room.bottom())), room.max);
    Arranged { rows, content_w: rows.width().min(840.0), stage: Some(stage), buttons: Some(buttons) }
}

/// Where a link to `id` lands, and whether this visit opens in «Подробно»
/// to show it: a link to a whole group, or in a handout build to a
/// developer row, goes to the nearest row users see; a row «Просто» does
/// not show opens the page in «Подробно» for this visit (the saved mode
/// stays).
pub(crate) fn link_target(id: Id, group: bool, deep: bool, own_build: bool) -> (Id, bool) {
    let id = if group || !own_build { id.user_neighbour() } else { id };
    (id, !deep && id.level() != Level::Simple)
}

/// The page: the title with «Просто · Подробно», the tabs, then the rows
/// (and the stage beside them). `restore` scrolls the rows back where the
/// history left them.
pub(super) fn page(
    ui: &mut Ui,
    state: &ViewState,
    bottom_inset: f32,
    restore: Option<f32>,
    pages: &mut PageState,
    out: &mut Vec<Command>,
) -> Scrolled {
    let ctx = ui.ctx().clone();
    let own = brand::own_build();
    // A new visit when the page was not drawn a moment ago: a link's
    // «Подробно», the open chevrons and Ctrl+Z were for the last one.
    let pass = ctx.cumulative_pass_nr();
    if pass > pages.settings_seen + 2 {
        pages.settings_visit_deep = false;
        let deep_sent = pages.settings_memory.deep_sent;
        pages.settings_memory = Memory { deep_sent, ..Memory::default() };
        preset::close_all(&ctx);
        // A «Текст песни» dialog whose gear (or any link) led here closes:
        // it would stand over the page it opened.
        crate::screens::lyrics_ui::close_dialog(&ctx);
    }
    pages.settings_seen = pass;
    // The saved mode is the prefs'; the one just picked counts until they
    // bring it back.
    let mem = &mut pages.settings_memory;
    if mem.deep_sent == Some(state.prefs.settings_deep) {
        mem.deep_sent = None;
    }
    let saved = pages.settings_deep.or(mem.deep_sent).unwrap_or(state.prefs.settings_deep);
    let mut deep = saved || pages.settings_visit_deep;
    if let Some(id) = pages.focus_setting.take() {
        let group = std::mem::take(&mut pages.focus_group);
        let (id, open_deep) = link_target(id, group, deep, own);
        if open_deep {
            pages.settings_visit_deep = true;
            deep = true;
        }
        pages.settings_tab = id.tab();
        settings_help::begin_focus(&ctx, id, group);
    }

    let full = ui.max_rect();
    ui.add_space(8.0);
    let picked_mode = header(ui, deep);
    if let Some(d) = picked_mode {
        pages.settings_visit_deep = false;
        deep = d;
        match pages.settings_deep.as_mut() {
            Some(held) => *held = d,
            None => pages.settings_memory.deep_sent = Some(d),
        }
    }
    ui.add_space(12.0);
    let tab = pages.settings_tab.min(TABS.len() - 1);
    if let Some(n) = tabs(ui, tab) {
        pages.settings_tab = n;
    }
    let tab = pages.settings_tab.min(TABS.len() - 1);
    ui.add_space(4.0);

    let room = Rect::from_min_max(pos2(full.left(), ui.cursor().top()), full.max);
    let window = ctx.content_rect().size();
    let at = arrange(room, window, settings_help::stage_tab(tab));
    if let Some(stage) = at.stage {
        // Under the stage, and what shows while it fades between screens.
        let s = Theme::get(&ctx).scheme;
        ui.painter().rect_filled(stage, crate::tokens::shape::EXTRA_LARGE, s.surface_container_low);
    }

    rows::set_mode(&ctx, rows::Mode { deep, dev: own, opacity: ui.opacity(), stage: at.stage.is_some() });
    let mut rui = ui.new_child(UiBuilder::new().max_rect(at.rows).id_salt(("settings-rows-ui", tab)));
    let mut area = egui::ScrollArea::vertical().id_salt(("settings-rows", tab)).auto_shrink([false, false]);
    if let Some(y) = restore {
        area = area.vertical_scroll_offset(y);
    }
    let mut p: Prefs = state.prefs.clone();
    let before = p.clone();
    let o = area.show(&mut rui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let page_x = ui.max_rect().x_range();
        ui.set_max_width(at.content_w);
        match tab {
            settings_help::TAB_MUSIC => music::show(ui, state, &mut p, out),
            settings_help::TAB_LYRICS => lyrics::show(ui, state, &mut p, out),
            settings_help::TAB_LOOK => look::show(ui, &mut p, out),
            settings_help::TAB_SERVICES => services::show(ui, state, &mut p, pages, out),
            settings_help::TAB_DOWNLOADS => downloads::show(ui, state, &mut p, pages, out),
            _ => system::show(ui, state, &mut p, pages, out),
        }
        // Beside the stage the ⓘ cards have no sample of their own.
        settings_help::finish(ui, &mut p, at.stage.is_none());
        ui.add_space(16.0);
        ui.add(Divider::new());
        ui.add_space(16.0);
        brand::made_by(ui, page_x);
        ui.add_space(8.0);
        ui.add_space(bottom_inset);
    });
    rows::set_mode(&ctx, rows::Mode { deep, dev: own, opacity: ui.opacity(), stage: at.stage.is_some() });
    let offer = preset::take_offer(&ctx);
    let sent = rows::take_sent(&ctx);

    // The recommendations' rows change one preference each.
    let patches = recs_patches(&before, &p);
    let mut patched = before.clone();
    for patch in &patches {
        patch.apply(&mut patched);
    }
    let changed = !patches.is_empty() || p != patched || !sent.is_empty();
    out.extend(patches.into_iter().map(Command::SetPref));
    if p != patched {
        out.push(Command::SetPrefs(Box::new(p.clone())));
    }
    // After the whole prefs, so they do not undo them.
    out.extend(sent.into_iter().map(Command::SetPref));
    if let Some(d) = picked_mode {
        out.push(Command::SetPref(PrefPatch::SettingsDeep(d)));
    }
    undo(&ctx, state, &mut pages.settings_memory, changed, out);

    if let (Some(rect), Some(buttons)) = (at.stage, at.buttons) {
        let drawn = settings_help::drawn_rows(&ctx);
        let rows = if drawn.0 == pass { drawn.1 } else { Vec::new() };
        let mut req = follow(&ctx, &mut pages.settings_memory, tab, rect, &p, &rows, offer, p != before);
        req.opacity = ui.opacity();
        // Трек · Текст · Главная · Альбом: which screen is on the stage,
        // and a way to look at any.
        let labels: Vec<&str> = Scene::ALL.iter().map(|s| s.label()).collect();
        let at_scene = Scene::ALL.iter().position(|s| *s == req.scene).unwrap_or(0);
        let mut bui = ui.new_child(UiBuilder::new().max_rect(buttons).id_salt(("settings-stage-scenes", tab)));
        if let Some(n) = connected(&mut bui, egui::Id::new("settings-stage-scene"), &labels, at_scene, &[]).0 {
            req.scene = Scene::ALL[n];
            req.parts.clear();
            req.script = Script::None;
            req.fit_home = false;
            req.beat = false;
            pages.settings_memory.scenes[stage_index(tab)] = Some(req.scene);
        }
        stage::request(&ctx, req);
    }
    Scrolled { offset: o.state.offset.y, max: (o.content_size.y - o.inner_rect.height()).max(0.0) }
}

/// The stage tab's slot in [`Memory::scenes`].
fn stage_index(tab: usize) -> usize {
    usize::from(tab == settings_help::TAB_LOOK)
}

/// The screen a stage tab shows when nothing asks for another.
fn tab_scene(tab: usize) -> Scene {
    if tab == settings_help::TAB_LYRICS { Scene::Lyrics } else { Scene::Track }
}

/// The settings option `i` of `id`'s row gives (shown on the stage before
/// it is picked), where it is a value of the prefs.
fn option_prefs(id: Id, p: &Prefs, i: usize) -> Option<Prefs> {
    match id {
        // A shape of the grid, the random pool's too: the cover in that
        // shape, big on the stage.
        Id::ShapePick | Id::ShapeRange => CoverShape::shapes().get(i).map(|c| Prefs { cover_shape: *c, ..p.clone() }),
        // The presets offer their own (the chevron's «Свои» too); the
        // sections of Главная are places, not values.
        Id::Preset(_) | Id::HomeSections => None,
        _ if i < id.choices().len() => {
            let mut q = p.clone();
            id.choose(&mut q, i);
            Some(q)
        }
        _ => None,
    }
}

/// What the stage shows this frame, by priority: the row being dragged, the
/// option or preset under the pointer (shown, not picked, the same frame it
/// is entered), the row under the pointer, else the tab's last screen.
/// Leaving the rows restores at once. A gap between neighbouring options or
/// rows keeps the last preview, so nothing flashes the saved values on the way.
#[allow(clippy::too_many_arguments)]
fn follow(
    ctx: &egui::Context,
    mem: &mut Memory,
    tab: usize,
    rect: Rect,
    p: &Prefs,
    rows: &[DrawnRow],
    offer: Option<preset::Offer>,
    changed: bool,
) -> StageRequest {
    let now = anim::now(ctx);
    let pointer = ctx.pointer_hover_pos();
    let down = ctx.input(|i| i.pointer.any_down());
    // The row and option under the pointer: an open menu's item first (it
    // lies over other rows), then the row's own buttons.
    let in_menu = pointer.and_then(|at| rows.iter().find_map(|r| r.menu.iter().position(|m| m.contains(at)).map(|i| (r.id, Some(i)))));
    let on_row = |mem: &Memory| {
        let r = rows.iter().find(|r| r.hovered && pointer.is_some_and(|at| r.clip.contains(at)))?;
        let hit = pointer.and_then(|at| r.choices.iter().position(|c| c.contains(at)));
        // Between two options of this row, the one just shown stays.
        let opt = hit.or_else(|| {
            let line = r.choices.iter().copied().reduce(|a, b| a.union(b))?;
            let held = mem.shown.as_ref().filter(|s| s.id == r.id && s.offer.is_none()).and_then(|s| s.opt);
            pointer.is_some_and(|at| line.expand2(vec2(4.0, 6.0)).contains(at)).then_some(held).flatten()
        });
        Some((r.id, opt))
    };
    // Keyboard focus counts as the pointer while the pointer is on no row.
    let focused = || rows.iter().find(|r| r.focused).map(|r| (r.id, None));
    let under = in_menu.or_else(|| on_row(mem)).or_else(focused);
    if changed {
        let dragging = down && mem.changed.is_some();
        match (dragging, under) {
            (true, _) => {
                if let Some(c) = mem.changed.as_mut() {
                    c.1 = now;
                }
            }
            (false, Some((id, _))) => mem.changed = Some((id, now)),
            (false, None) => {}
        }
    }
    let slot = stage_index(tab);
    let mut req = StageRequest::new(rect, mem.scenes[slot].unwrap_or_else(|| tab_scene(tab)), p.clone());
    // The stage just plays what the row or preset changes: no part outlined,
    // nothing zoomed in (the owner found both useless).
    let show = |req: &mut StageRequest, mem: &mut Memory, id: Id, prefs: Prefs| {
        if let Some(scene) = id.scene() {
            req.scene = scene;
            mem.scenes[slot] = Some(scene);
        }
        // In the look that has what the row tunes (the wave for a jump).
        let prefs = id.staged(&prefs);
        req.script = id.script(&prefs);
        req.prefs = prefs.clone();
        req.zoom = false;
        req.beat = id.needs_beat(&prefs);
        req.fit_home = id == Id::HomeSections;
    };
    // A drag stays on the row it started on. The pointer up, the row under
    // it shows at once.
    if let Some((id, _)) = mem.changed
        && down
        && id.part().is_some()
    {
        show(&mut req, &mut *mem, id, p.clone());
        mem.shown = None;
        ctx.request_repaint();
        return req;
    }
    let in_rows = pointer.is_some_and(|at| rows.iter().any(|r| r.clip.contains(at)));
    if let Some(o) = offer {
        mem.shown = Some(Shown { id: o.id, opt: None, offer: Some(o.prefs), at: now });
    } else if let Some((id, opt)) = under {
        mem.shown = id.part().is_some().then(|| Shown { id, opt, offer: None, at: now });
    } else if !in_rows {
        mem.shown = None;
    }
    if let Some(s) = mem.shown.clone() {
        let prefs = s.offer.or_else(|| s.opt.and_then(|i| option_prefs(s.id, p, i))).unwrap_or_else(|| p.clone());
        show(&mut req, &mut *mem, s.id, prefs);
    }
    req
}

/// Ctrl+Z: one step back through this visit's changes (a slider's drag is
/// one step). The runtime's own fields stay as they are now.
fn undo(ctx: &egui::Context, state: &ViewState, mem: &mut Memory, changed: bool, out: &mut Vec<Command>) {
    let down = ctx.input(|i| i.pointer.any_down());
    // A slider lets go of its value on the frame the button comes up.
    let pressed = down || ctx.input(|i| i.pointer.any_released());
    if changed {
        if !(pressed && mem.held) {
            mem.undo.push(state.prefs.clone());
            if mem.undo.len() > UNDO {
                mem.undo.remove(0);
            }
        }
        mem.held = down;
    } else if !down {
        mem.held = false;
    }
    // A text field being typed in keeps its own Ctrl+Z.
    if ctx.egui_wants_keyboard_input() || !ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z)) {
        return;
    }
    if let Some(before) = mem.undo.pop() {
        out.push(Command::SetPrefs(Box::new(restored(&before, &state.prefs))));
    }
}

/// `before` with what the runtime itself keeps in the prefs (accounts,
/// devices, the mode, hidden mixes) as it is `now`.
fn restored(before: &Prefs, now: &Prefs) -> Prefs {
    let mut p = before.clone();
    p.masters_dir = now.masters_dir.clone();
    p.fallback_seed = now.fallback_seed;
    p.prowlarr_url = now.prowlarr_url.clone();
    p.prowlarr_configured = now.prowlarr_configured;
    p.output_devices = now.output_devices.clone();
    p.metadata_contact = now.metadata_contact.clone();
    p.acoustid_configured = now.acoustid_configured;
    p.lastfm_configured = now.lastfm_configured;
    p.listenbrainz_configured = now.listenbrainz_configured;
    p.home_card = now.home_card;
    p.home_hidden_mixes = now.home_hidden_mixes.clone();
    p.lyrics_settings_seen = now.lyrics_settings_seen;
    p.settings_deep = now.settings_deep;
    p.settings_rev = now.settings_rev;
    p
}

/// «Настройки» and «Просто · Подробно» at the line's right (under the
/// title when the page is too narrow for both). Returns a new mode.
fn header(ui: &mut Ui, deep: bool) -> Option<bool> {
    let s = Theme::get(ui.ctx()).scheme;
    let width_key = egui::Id::new("settings-mode-w");
    let toggle_w: f32 = ui.ctx().data(|d| d.get_temp(width_key)).unwrap_or(180.0);
    let w = ui.available_width();
    let h = Size::S.height();
    let (line, _) = ui.allocate_exact_size(vec2(w, 40.0), Sense::hover());
    ui.painter().text(line.left_center(), Align2::LEFT_CENTER, "Настройки", TypeRole::HeadlineMedium.font_id(), s.on_surface);
    let at = if w < toggle_w + 220.0 {
        ui.add_space(8.0);
        let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        Rect::from_min_size(r.min, vec2(toggle_w.min(w), h))
    } else {
        Rect::from_min_size(pos2(line.right() - toggle_w, line.center().y - h * 0.5), vec2(toggle_w, h))
    };
    let mut cui = ui.new_child(UiBuilder::new().max_rect(at).id_salt("settings-mode"));
    let (pick, rects) = connected(&mut cui, egui::Id::new("set-mode"), &["Просто", "Подробно"], usize::from(deep), &[]);
    if let (Some(a), Some(b)) = (rects.first(), rects.last()) {
        let measured = b.right() - a.left();
        if (measured - toggle_w).abs() > 0.5 {
            ui.ctx().data_mut(|d| d.insert_temp(width_key, measured));
            ui.ctx().request_repaint();
        }
    }
    pick.map(|n| n == 1)
}

/// The six tabs: a connected group, or a menu chip where they do not fit.
fn tabs(ui: &mut Ui, tab: usize) -> Option<usize> {
    if ui.available_width() < 560.0 {
        let options: Vec<(String, bool)> = TABS.iter().enumerate().map(|(i, t)| ((*t).to_owned(), i == tab)).collect();
        return rows::dropdown(ui, "set-tab-menu", TABS[tab], &options);
    }
    connected(ui, egui::Id::new("set-tab"), &TABS, tab, &[]).0
}

/// The recommendations' preferences `p` changed from `before`, as
/// [`PrefPatch`]es.
fn recs_patches(before: &Prefs, p: &Prefs) -> Vec<PrefPatch> {
    let mut out = Vec::new();
    if p.autoplay != before.autoplay {
        out.push(PrefPatch::Autoplay(p.autoplay));
    }
    if p.autoplay_continue_world != before.autoplay_continue_world {
        out.push(PrefPatch::AutoplayContinueWorld(p.autoplay_continue_world));
    }
    if p.mixes_downloaded_only != before.mixes_downloaded_only {
        out.push(PrefPatch::MixesDownloadedOnly(p.mixes_downloaded_only));
    }
    if p.mixes_with_new != before.mixes_with_new {
        out.push(PrefPatch::MixesWithNew(p.mixes_with_new));
    }
    let cleared = p.radio_new_share_by.is_empty() && !before.radio_new_share_by.is_empty();
    if p.radio_new_share != before.radio_new_share || cleared {
        out.push(PrefPatch::RadioNewShare(p.radio_new_share));
    }
    for (kind, share) in &p.radio_new_share_by {
        if before.radio_new_share_by.get(kind) != Some(share) {
            out.push(PrefPatch::RadioNewShareBy(*kind, *share));
        }
    }
    let flags = [
        (p.dislike_button, before.dislike_button, PrefPatch::DislikeButton as fn(bool) -> PrefPatch),
        (p.dislike_skips, before.dislike_skips, PrefPatch::DislikeSkips),
        (p.dislike_skip_everywhere, before.dislike_skip_everywhere, PrefPatch::DislikeSkipEverywhere),
        (p.dislike_in_recs, before.dislike_in_recs, PrefPatch::DislikeInRecs),
        (p.background_counts_less, before.background_counts_less, PrefPatch::BackgroundCountsLess),
        (p.profiles_split, before.profiles_split, PrefPatch::ProfilesSplit),
        (p.mixes_profiles, before.mixes_profiles, PrefPatch::MixesProfiles),
        (p.mixes_everything, before.mixes_everything, PrefPatch::MixesEverything),
        (p.mixes_moods, before.mixes_moods, PrefPatch::MixesMoods),
        (p.mixes_on_repeat, before.mixes_on_repeat, PrefPatch::MixesOnRepeat),
        (p.graph_use, before.graph_use, PrefPatch::GraphUse),
        (p.graph_services, before.graph_services, PrefPatch::GraphServices),
        (p.graph_open_data, before.graph_open_data, PrefPatch::GraphOpenData),
        (p.graph_personal, before.graph_personal, PrefPatch::GraphPersonal),
        (p.library_as_taste, before.library_as_taste, PrefPatch::LibraryAsTaste),
    ];
    for (now, was, patch) in flags {
        if now != was {
            out.push(patch(now));
        }
    }
    if p.profile_names != before.profile_names {
        out.push(PrefPatch::ProfileNames(p.profile_names));
    }
    if p.background_cards != before.background_cards {
        out.push(PrefPatch::BackgroundCards(p.background_cards));
    }
    if p.radio_no_repeat_hours != before.radio_no_repeat_hours {
        out.push(PrefPatch::RadioNoRepeat(p.radio_no_repeat_hours));
    }
    if p.radio_explore != before.radio_explore {
        out.push(PrefPatch::RadioExplore(p.radio_explore));
    }
    if p.radio_unheard != before.radio_unheard {
        out.push(PrefPatch::RadioUnheard(p.radio_unheard));
    }
    out
}

/// «317 МБ», «1,1 ГБ».
fn size_label(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1000.0 { format!("{:.1} ГБ", mb / 1024.0).replace('.', ",") } else { format!("{mb:.0} МБ") }
}

/// Commands from list items' trailing closures (they cannot borrow `out`).
fn out_push(ui: &Ui, c: Command) {
    let id = egui::Id::new("set-align-pushed");
    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Vec<Command>>(id).push(c));
}

fn take_pushed(ui: &Ui) -> Vec<Command> {
    let id = egui::Id::new("set-align-pushed");
    ui.ctx().data_mut(|d| std::mem::take(d.get_temp_mut_or_default::<Vec<Command>>(id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::pages::settings_help::DrawnRow;

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        ctx
    }

    /// The rows the page draws on `tab` in a `width` wide page.
    fn drawn(ctx: &egui::Context, state: &ViewState, tab: usize, deep: bool, width: f32) -> Vec<DrawnRow> {
        let mut pages = PageState::default();
        pages.settings_tab = tab;
        pages.settings_deep = Some(deep);
        let mut rows = Vec::new();
        for frame in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 900.0))),
                time: Some(f64::from(frame) * 0.1),
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut done = ctx.run_ui(input, |ui| {
                page(ui, state, 96.0, None, &mut pages, &mut out);
            });
            done.textures_delta.clear();
            rows = settings_help::drawn_rows(ctx).1;
        }
        rows
    }

    /// «Подробно» draws the rows of «Просто» in the same order with the
    /// others between them, «Просто» draws only its own, and a developer
    /// row shows only in «Подробно» of an own build.
    #[test]
    fn simple_rows_are_a_subsequence_of_deep_rows_on_every_tab() {
        let ctx = context();
        let state = crate::screens::sweep::survey_state();
        for tab in 0..TABS.len() {
            let simple: Vec<Id> = drawn(&ctx, &state, tab, false, 1280.0).into_iter().map(|r| r.id).collect();
            let deep: Vec<Id> = drawn(&ctx, &state, tab, true, 1280.0).into_iter().map(|r| r.id).collect();
            assert!(!simple.is_empty() && deep.len() > simple.len(), "tab {tab}: {} / {}", simple.len(), deep.len());
            for id in &simple {
                assert_eq!(id.level(), Level::Simple, "{id:?} in «Просто»");
                assert_eq!(id.tab(), tab, "{id:?}");
            }
            let mut it = deep.iter();
            for id in &simple {
                assert!(it.any(|d| d == id), "tab {tab}: {id:?} out of order in «Подробно»");
            }
            for id in &deep {
                assert_eq!(id.tab(), tab, "{id:?}");
                assert!(id.level() != Level::Dev || brand::own_build(), "{id:?}");
            }
        }
    }

    /// A link to a row «Просто» does not show opens this visit in
    /// «Подробно»; a developer row in a handout build, and any whole
    /// group, lands on a row users see.
    #[test]
    fn links_show_their_row() {
        assert_eq!(link_target(Id::SeedRatio, false, false, false), (Id::SeedRatio, true));
        assert_eq!(link_target(Id::SeedRatio, false, true, false), (Id::SeedRatio, false));
        assert_eq!(link_target(Id::Autoplay, false, false, false), (Id::Autoplay, false));
        assert_eq!(link_target(Id::LyricsUi, false, false, true), (Id::LyricsUi, true));
        assert_eq!(link_target(Id::LyricsUi, false, false, false), (Id::LyricsStyle, false));
        assert_eq!(link_target(Id::LyricsUi, true, true, true), (Id::LyricsStyle, false));
        assert_eq!(link_target(Id::LyricsGpu, false, false, false), (Id::LyricsModels, true));

        // The page itself: «Просто», a link to a deep row draws it.
        let ctx = context();
        let state = ViewState::default();
        let mut pages = PageState::default();
        pages.settings_deep = Some(false);
        pages.open_setting(Id::SeedRatio);
        let mut seen = false;
        for frame in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 900.0))),
                time: Some(f64::from(frame) * 0.1),
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut done = ctx.run_ui(input, |ui| {
                page(ui, &state, 96.0, None, &mut pages, &mut out);
            });
            done.textures_delta.clear();
            seen |= settings_help::drawn_rows(&ctx).1.iter().any(|r| r.id == Id::SeedRatio);
        }
        assert!(seen, "the linked deep row is drawn");
        assert_eq!(pages.settings_tab, settings_help::TAB_DOWNLOADS);
        assert_eq!(pages.settings_deep, Some(false), "the saved mode stays");
    }

    /// One frame of the page alone at `time`, the pointer at `at`, with
    /// `events`; what it sent.
    fn run(ctx: &egui::Context, state: &ViewState, pages: &mut PageState, time: f64, at: Option<egui::Pos2>, events: Vec<egui::Event>) -> Vec<Command> {
        let mut all: Vec<egui::Event> = at.map(egui::Event::PointerMoved).into_iter().collect();
        all.extend(events);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 900.0))),
            time: Some(time),
            events: all,
            ..Default::default()
        };
        let mut out = Vec::new();
        let mut done = ctx.run_ui(input, |ui| {
            page(ui, state, 96.0, None, pages, &mut out);
        });
        done.textures_delta.clear();
        out
    }

    /// The saved mode is the prefs' (the owner's build is given «Подробно»
    /// once, a new user starts in «Просто»).
    #[test]
    fn the_mode_comes_from_the_prefs() {
        let ctx = context();
        for deep in [false, true] {
            let mut state = crate::screens::sweep::survey_state();
            state.prefs.settings_deep = deep;
            let mut pages = PageState::default();
            pages.settings_tab = settings_help::TAB_LOOK;
            for f in 0..3 {
                run(&ctx, &state, &mut pages, f64::from(f) * 0.1, None, Vec::new());
            }
            let rows = settings_help::drawn_rows(&ctx).1;
            assert_eq!(rows.iter().any(|r| r.id == Id::Contrast), deep, "deep {deep}");
            assert!(rows.iter().any(|r| r.id == Id::Preset(nikgob_view::presets::Area::Track)), "the preset shows in both");
        }
        assert!(!Prefs::first_start().settings_deep);
    }

    /// The preset under the pointer is drawn on the stage (all of it, every
    /// part outlined) and nothing is sent until it is clicked.
    #[test]
    fn a_preset_under_the_pointer_is_shown_not_kept() {
        use nikgob_view::presets::Area;
        let ctx = context();
        let state = crate::screens::sweep::survey_state();
        let mut pages = PageState::default();
        pages.settings_tab = settings_help::TAB_LOOK;
        pages.settings_deep = Some(true);
        for f in 0..3 {
            run(&ctx, &state, &mut pages, f64::from(f) * 0.1, None, Vec::new());
        }
        let rows = settings_help::drawn_rows(&ctx).1;
        let tiles = rows.iter().find(|r| r.id == Id::Preset(Area::Track)).map(|r| r.choices.clone()).unwrap_or_default();
        assert_eq!(tiles.len(), 3, "Строго · Живо · Максимум");
        let at = tiles[0].center();
        let mut sent = Vec::new();
        for t in [1.0, 1.1, 1.3, 1.5] {
            sent.extend(run(&ctx, &state, &mut pages, t, Some(at), Vec::new()));
        }
        assert!(!sent.iter().any(|c| matches!(c, Command::SetPrefs(_) | Command::SetPref(_))), "{sent:?}");
        let req = stage::requested(&ctx).expect("a stage beside the rows");
        assert_eq!(req.prefs, Area::Track.with(&state.prefs, 0));
        assert_eq!(req.scene, Scene::Track);
        assert!(req.parts.is_empty() && !req.zoom, "the stage just plays the preset");
    }

    /// What the stage shows by the row under the pointer, once it rests
    /// there: the row's screen and part (a small part brought up), the
    /// option under the pointer shown before it is picked with its scene
    /// acted out, and a row not on the stage changes nothing.
    #[test]
    fn the_stage_follows_the_row_under_the_pointer() {
        let ctx = context();
        let p = Prefs::default();
        let stage_rect = Rect::from_min_size(pos2(700.0, 100.0), vec2(500.0, 320.0));
        let at = pos2(60.0, 60.0);
        let here = Rect::from_center_size(at, vec2(40.0, 40.0));
        let row = |id: Id, choices: Vec<Rect>| DrawnRow {
            id,
            label: String::new(),
            group: String::new(),
            rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 120.0)),
            button: Rect::NOTHING,
            control: None,
            choices,
            menu: Vec::new(),
            clip: Rect::EVERYTHING,
            opacity: 1.0,
            enabled: true,
            hovered: true,
            focused: false,
        };
        let mut mem = Memory::default();
        let mut t = 0.0;
        let mut step = |rows: &[DrawnRow], mem: &mut Memory| -> StageRequest {
            t += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 900.0))),
                time: Some(t),
                events: vec![egui::Event::PointerMoved(at)],
                ..Default::default()
            };
            let mut req = None;
            let mut done = ctx.run_ui(input, |ui| {
                let c = ui.ctx().clone();
                req = Some(follow(&c, mem, settings_help::TAB_LOOK, stage_rect, &p, rows, None, false));
            });
            done.textures_delta.clear();
            req.expect("a request")
        };
        let progress = [row(Id::ProgressWave, Vec::new())];
        let first = step(&progress, &mut mem);
        assert_eq!((first.scene, first.parts.clone(), first.zoom), (Scene::Track, Vec::new(), false), "the row shows the frame it is entered");
        assert!(!first.beat, "a wave is not a beat");

        let volume = [row(Id::NpVolume, vec![Rect::from_min_size(pos2(200.0, 0.0), vec2(10.0, 10.0)), here])];
        for _ in 0..3 {
            let _ = step(&volume, &mut mem);
        }
        let pill = step(&volume, &mut mem);
        assert!(!pill.prefs.np_volume && pill.prefs.np_volume_pop, "the option under the pointer is shown");
        assert_eq!((pill.script, pill.parts.clone()), (Script::Volume, Vec::new()));

        let date = [row(Id::AlbumDate, Vec::new())];
        for _ in 0..3 {
            let _ = step(&date, &mut mem);
        }
        assert_eq!(step(&date, &mut mem).scene, Scene::Album);
        let window = [row(Id::RailCompact, Vec::new())];
        for _ in 0..3 {
            let _ = step(&window, &mut mem);
        }
        let still = step(&window, &mut mem);
        assert_eq!(still.scene, Scene::Album, "the last screen stays");
        assert!(still.parts.is_empty());
    }

    /// Ctrl+Z takes back this visit's changes one by one (a drag is one),
    /// at most 20, and leaves what the runtime keeps in the prefs alone.
    #[test]
    fn ctrl_z_takes_a_change_back() {
        let ctx = context();
        let mut state = ViewState::default();
        let mut mem = Memory::default();
        let mut t = 0.0;
        let mut frame = |state: &ViewState, mem: &mut Memory, changed: bool, events: Vec<egui::Event>| -> Vec<Command> {
            t += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 900.0))),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = Vec::new();
            let mut done = ctx.run_ui(input, |ui| {
                let c = ui.ctx().clone();
                undo(&c, state, mem, changed, &mut out);
            });
            done.textures_delta.clear();
            out
        };
        let was = state.prefs.ring;
        assert!(frame(&state, &mut mem, true, Vec::new()).is_empty());
        state.prefs.ring = !was;
        state.prefs.output_devices = vec!["Наушники".into()];
        let ctrl = egui::Modifiers { ctrl: true, command: true, ..Default::default() };
        let z = egui::Event::Key { key: Key::Z, physical_key: None, pressed: true, repeat: false, modifiers: ctrl };
        let out = frame(&state, &mut mem, false, vec![z.clone()]);
        let [Command::SetPrefs(back)] = out.as_slice() else { panic!("{out:?}") };
        assert_eq!(back.ring, was);
        assert_eq!(back.output_devices, state.prefs.output_devices, "the runtime's own stay");
        assert!(frame(&state, &mut mem, false, vec![z]).is_empty(), "nothing more to take back");

        // A drag: one step for the whole press.
        let at = pos2(10.0, 10.0);
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(&state, &mut mem, true, vec![egui::Event::PointerMoved(at), press(true)]);
        frame(&state, &mut mem, true, Vec::new());
        frame(&state, &mut mem, false, vec![press(false)]);
        assert_eq!(mem.undo.len(), 1);
        for _ in 0..30 {
            frame(&state, &mut mem, true, Vec::new());
        }
        assert_eq!(mem.undo.len(), UNDO);
    }

    /// Every field a preset owns has its row on the preset's tab, and every
    /// row of «Экран трека» and «Вид текста» has a screen and a part on the
    /// stage.
    #[test]
    fn preset_fields_have_rows_and_stage_rows_have_parts() {
        use nikgob_view::presets::Area;
        let ctx = context();
        let state = crate::screens::sweep::survey_state();
        for area in Area::ALL.into_iter().filter(|a| !a.names().is_empty()) {
            let tab = Id::Preset(area).tab();
            let rows: Vec<Id> = drawn(&ctx, &state, tab, true, 1280.0).into_iter().map(|r| r.id).collect();
            assert!(rows.contains(&Id::Preset(area)), "{area:?}");
            for &f in area.fields() {
                let ids = settings_help::preset_rows(f);
                assert!(!ids.is_empty(), "{f:?}");
                for id in ids {
                    assert_eq!(id.tab(), tab, "{f:?} → {id:?}");
                    assert!(rows.contains(&id), "{f:?} → {id:?} is not drawn");
                }
            }
            let simple: Vec<Id> = drawn(&ctx, &state, tab, false, 1280.0).into_iter().map(|r| r.id).collect();
            assert!(simple.contains(&Id::Preset(area)), "{area:?} in «Просто»");
        }
        for tab in [settings_help::TAB_LOOK, settings_help::TAB_LYRICS] {
            let rows = drawn(&ctx, &state, tab, true, 1280.0);
            let staged: Vec<&DrawnRow> = rows.iter().filter(|r| r.group == "Экран трека" || r.group == "Вид текста").collect();
            assert!(staged.len() > 10, "tab {tab}");
            for r in staged {
                assert!(r.id.scene().is_some() && r.id.part().is_some(), "{:?}", r.id);
            }
        }
    }

    /// Wide: the stage on the right, the rows ≤ 600; narrow: a strip over
    /// the rows; mini mode and tabs without a stage: none.
    #[test]
    fn the_stage_has_its_place_by_width() {
        let window = vec2(1280.0, 800.0);
        // A 1280 window with the rail open.
        let wide = arrange(Rect::from_min_size(pos2(280.0, 200.0), vec2(976.0, 600.0)), window, true);
        let stage = wide.stage.expect("a stage when wide");
        assert!(wide.rows.width() <= 600.0 && wide.rows.width() >= 460.0);
        assert!(stage.left() > wide.rows.right() && stage.right() <= 1256.0 + 0.1 && stage.width() > 400.0);
        assert!(wide.buttons.is_some_and(|b| b.top() > stage.bottom()));
        let narrow = arrange(Rect::from_min_size(pos2(100.0, 200.0), vec2(700.0, 600.0)), vec2(800.0, 800.0), true);
        let strip = narrow.stage.expect("a strip when narrow");
        assert_eq!(strip.width(), 700.0);
        assert!((180.0..=300.0).contains(&strip.height()));
        assert!(narrow.rows.top() > strip.bottom());
        let mini = arrange(Rect::from_min_size(pos2(0.0, 0.0), vec2(480.0, 600.0)), vec2(500.0, 800.0), true);
        assert!(mini.stage.is_none());
        let plain = arrange(Rect::from_min_size(pos2(0.0, 0.0), vec2(1400.0, 600.0)), window, false);
        assert!(plain.stage.is_none() && plain.content_w == 840.0);
    }
}
