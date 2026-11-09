//! Настройки (settings-v2-concept.md): one page, six tabs — Музыка ·
//! Тексты · Вид · Сервисы · Загрузки · Система — and one switch, Просто ·
//! Подробно. «Подробно» shows the same tabs, groups and rows in the same
//! order, with the rest of the rows between them; a row never moves when
//! the mode changes.
//!
//! The title line and the tabs stay; under them the rows scroll. Beside
//! the rows of «Тексты» and «Вид» the page keeps room for the live stage
//! (r21-stage draws it there): on the right of a wide page, as a strip
//! over the rows of a narrow one, none in mini mode. Every change is sent
//! as a whole new `Prefs` (the recommendations' rows as their patches).

mod dev;
mod downloads;
mod look;
mod lyrics;
mod music;
mod rows;
mod services;
mod system;

use egui::{Align2, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};
use nikgob_view::{Command, PrefPatch, Prefs, ViewState};

pub(super) use rows::fmt_speed;
pub(crate) use services::{SoloistDialog, open_url};

use super::settings_help::{self, Level, SettingId as Id, TABS};
use super::{PageState, brand, connected};
use crate::{
    Theme, TypeRole,
    components::{Divider, Size},
};

/// How far the page's rows are scrolled, for the history's scroll restore.
pub(super) struct Scrolled {
    pub offset: f32,
    /// The furthest they can be scrolled.
    pub max: f32,
}

/// Where the live stage goes this pass: its rectangle and the room under
/// it for the buttons of its screens (Трек · Текст · Главная · Альбом), on
/// the page's tab, at the page's opacity. The page only keeps the room;
/// the stage is drawn over it on its own layer.
// TODO(r21-wire): becomes `stage::StageRequest { rect, scene, prefs, part,
// script, opacity }` once the rows say their scene and part; the buttons
// are drawn into `buttons`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StageSlot {
    pub pass: u64,
    pub rect: Rect,
    pub buttons: Rect,
    pub tab: usize,
    pub opacity: f32,
}

fn slot_key() -> egui::Id {
    egui::Id::new("settings-stage-slot")
}

/// The stage's place if the settings page kept one this pass.
pub fn stage_slot(ctx: &egui::Context) -> Option<StageSlot> {
    ctx.data(|d| d.get_temp::<StageSlot>(slot_key())).filter(|s| s.pass == ctx.cumulative_pass_nr())
}

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
/// (and the stage's room). `restore` scrolls the rows back where the
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
    // «Подробно» was for the last one.
    let pass = ctx.cumulative_pass_nr();
    if pass > pages.settings_seen + 2 {
        pages.settings_visit_deep = false;
    }
    pages.settings_seen = pass;
    // TODO(r21-wire): the saved mode is `Prefs::settings_deep`
    // (`PrefPatch::SettingsDeep`); until then the session keeps it, the
    // owner's builds starting in «Подробно» and a handout in «Просто».
    let mut deep = pages.settings_deep.unwrap_or(own) || pages.settings_visit_deep;
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
    if let Some(d) = header(ui, deep) {
        pages.settings_deep = Some(d);
        pages.settings_visit_deep = false;
        deep = d;
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
    if let (Some(stage), Some(buttons)) = (at.stage, at.buttons) {
        // The room stays empty until the stage covers it.
        let s = Theme::get(&ctx).scheme;
        ui.painter().rect_filled(stage, crate::tokens::shape::EXTRA_LARGE, s.surface_container_low);
        let slot = StageSlot { pass, rect: stage, buttons, tab, opacity: ui.opacity() };
        ctx.data_mut(|d| d.insert_temp(slot_key(), slot));
    }

    rows::set_mode(&ctx, rows::Mode { deep, dev: own, opacity: ui.opacity() });
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

    // The recommendations' rows change one preference each.
    let patches = recs_patches(&before, &p);
    let mut patched = before.clone();
    for patch in &patches {
        patch.apply(&mut patched);
    }
    out.extend(patches.into_iter().map(Command::SetPref));
    if p != patched {
        out.push(Command::SetPrefs(Box::new(p)));
    }
    Scrolled { offset: o.state.offset.y, max: (o.content_size.y - o.inner_rect.height()).max(0.0) }
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
