//! A group's presets (settings-v2-concept.md §1.2, §2.4): the group's first
//! row, in «Просто» too. Buttons where there is no stage, live tiles for
//! the track screen; «Свои» after them while there are values of the
//! user's own.
//!
//! The preset under the pointer is shown before it is applied: the stage
//! draws it, and the rows it would change take their future values on a
//! tonal underlay (in «Просто» a chevron opens those rows in place, until
//! the page is left). A click sends the preset as its own patch, so the
//! values kept for «Свои» are the runtime's, not the page's copy.

use egui::{Rect, Sense, Stroke, Ui, vec2};
use nikgob_view::presets::{Area, CUSTOM};
use nikgob_view::{PrefPatch, Prefs};

use super::super::connected;
use super::super::settings_help::{self, Level, SettingId as Id};
use super::rows;
use crate::screens::{ambient, bg_shapes, now_playing};
use crate::{
    Icon, Theme, TypeRole,
    components::{IconButton, Size, anim},
    tokens::shape,
};

/// The pointer must rest this long on a preset before it is shown (passing
/// over the buttons shows nothing).
pub(super) const DWELL: f64 = 0.12;

/// What the preset row offers this pass while the pointer rests on one of
/// its presets: the settings it would give.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Offer {
    pub id: Id,
    pub prefs: Prefs,
}

fn offer_key() -> egui::Id {
    egui::Id::new("settings-preset-offer")
}

/// The preset under the pointer this pass, if any (the page reads it for
/// the stage).
pub(super) fn take_offer(ctx: &egui::Context) -> Option<Offer> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.remove_temp::<(u64, Offer)>(offer_key())).filter(|(p, _)| *p == pass).map(|(_, o)| o)
}

fn open_key() -> egui::Id {
    egui::Id::new("settings-preset-open")
}

/// The chevrons opened this visit are closed again (a new visit).
pub(super) fn close_all(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Vec<Area>>(open_key()));
}

/// Since when the pointer rests on option `at` of `id`'s row.
fn dwelt(ui: &Ui, id: Id, at: Option<usize>) -> bool {
    let key = egui::Id::new(("settings-preset-dwell", id));
    let now = anim::now(ui.ctx());
    let Some(at) = at else {
        ui.ctx().data_mut(|d| d.remove::<(usize, f64)>(key));
        return false;
    };
    let since = match ui.ctx().data(|d| d.get_temp::<(usize, f64)>(key)) {
        Some((i, t)) if i == at => t,
        _ => {
            ui.ctx().data_mut(|d| d.insert_temp(key, (at, now)));
            now
        }
    };
    if now - since < DWELL {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(DWELL - (now - since) + 0.01));
        return false;
    }
    true
}

/// The settings option `i` of `area`'s row gives: a preset, or «Свои» (the
/// values kept, or the current ones when none were).
fn offered(area: Area, p: &Prefs, i: usize) -> Prefs {
    if i < area.names().len() { area.with(p, i) } else { area.custom(p).unwrap_or_else(|| p.clone()) }
}

/// The presets of `area` as the first row of its group. Returns the
/// settings the preset under the pointer would give (the group's rows are
/// drawn from them meanwhile; nothing of them is kept).
pub(super) fn row(ui: &mut Ui, area: Area, p: &Prefs) -> Option<Prefs> {
    let names = area.names();
    if names.is_empty() {
        return None;
    }
    let id = Id::Preset(area);
    let custom = area.custom_shown(p);
    let current = area.current(p).unwrap_or(names.len());
    let ctx = ui.ctx().clone();
    let mode = rows::mode(&ctx);
    // A chevron opens the group's «Подробно» rows in «Просто».
    let hidden = !mode.deep && area.fields().iter().flat_map(|f| settings_help::preset_rows(*f)).any(|r| r.level() == Level::Deep);
    let mut open = ctx.data(|d| d.get_temp::<Vec<Area>>(open_key())).unwrap_or_default();
    let mut toggled = false;
    let drawn = rows::item(ui, id, |ui| {
        ui.add_space(8.0);
        let (pick, rects) = if area.tiles() { tiles(ui, area, p, custom, current) } else { buttons(ui, area, custom, current) };
        // The presets are the row's options (the ⓘ card names the one
        // under the pointer).
        settings_help::report_choices(ui.ctx(), rects.iter().take(names.len()).copied().collect());
        if let Some(last) = rects.last() {
            settings_help::help_button_beside(ui, id, *last);
            if hidden {
                let at = Rect::from_min_size(last.right_top() + vec2(8.0 + settings_help::BTN + 4.0, (last.height() - 32.0) * 0.5), vec2(32.0, 32.0));
                let is_open = open.contains(&area);
                let icon = if is_open { Icon::ExpandLess } else { Icon::ExpandMore };
                if ui.put(at, IconButton::new(icon).size(Size::Xs).id_salt(("set-preset-more", id))).clicked() {
                    toggled = true;
                }
            }
        }
        ui.add_space(8.0);
        (pick, rects)
    });
    let (pick, rects) = drawn?;
    if toggled {
        if let Some(i) = open.iter().position(|a| *a == area) {
            open.remove(i);
        } else {
            open.push(area);
        }
        ctx.data_mut(|d| d.insert_temp(open_key(), open.clone()));
    }
    if hidden && open.contains(&area) {
        rows::expand(&ctx);
    }
    match pick {
        Some(n) if n < names.len() => rows::send(&ctx, PrefPatch::Preset(area, n)),
        Some(_) if p.custom_presets.contains_key(&area) => rows::send(&ctx, PrefPatch::PresetCustom(area)),
        _ => {}
    }
    let under = rects.iter().position(|r| ui.rect_contains_pointer(*r));
    if !dwelt(ui, id, under) {
        return None;
    }
    let i = under?;
    let shown = offered(area, p, i);
    settings_help::set_future(&ctx, settings_help::preset_changes(area, p, &shown));
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(offer_key(), (pass, Offer { id, prefs: shown.clone() })));
    Some(shown)
}

/// The presets as a connected group of buttons, «Свои» last.
fn buttons(ui: &mut Ui, area: Area, custom: bool, current: usize) -> (Option<usize>, Vec<Rect>) {
    let mut labels: Vec<&str> = area.names().to_vec();
    if custom {
        labels.push(CUSTOM);
    }
    connected(ui, egui::Id::new(("set-preset", area)), &labels, current, &[])
}

/// A tile's card.
const TILE: egui::Vec2 = vec2(112.0, 72.0);

/// The presets as live tiles: a small sample of the track screen (glow,
/// shapes, the cover with its ring) and the name under it; only the tile
/// under the pointer moves. «Свои» draws the values kept.
fn tiles(ui: &mut Ui, area: Area, p: &Prefs, custom: bool, current: usize) -> (Option<usize>, Vec<Rect>) {
    let ctx = ui.ctx().clone();
    let app = Theme::get(&ctx);
    let s = app.scheme;
    let n = area.names().len() + usize::from(custom);
    let mut picked = None;
    let mut rects = Vec::with_capacity(n);
    let label_h = TypeRole::LabelLarge.style().line_height;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        for i in 0..n {
            let name = area.names().get(i).copied().unwrap_or(CUSTOM);
            let (rect, _) = ui.allocate_exact_size(vec2(TILE.x, TILE.y + 6.0 + label_h), Sense::hover());
            let resp = ui.interact(rect, egui::Id::new(("set-preset-tile", area, i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            let card = Rect::from_min_size(rect.min, TILE);
            let selected = i == current;
            let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(card));
            painter.rect_filled(card, shape::MEDIUM, s.surface_container_high);
            // The sample: still unless the pointer is on it.
            let moving = resp.hovered();
            let shown = offered(area, p, i);
            let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(card).id_salt(("set-preset-tile-ui", area, i)));
            tui.set_clip_rect(ui.clip_rect().intersect(card));
            Theme::set(&ctx, app.with_reduced_motion(app.reduced_motion || !moving));
            let t = anim::now(&ctx) as f32;
            let kick = if moving { settings_help::beat(t) } else { 0.0 };
            if shown.ambient != nikgob_view::Ambient::Off {
                let bass = if shown.ambient == nikgob_view::Ambient::Reactive { kick } else { 0.0 };
                ambient::paint(&ctx, tui.painter(), card, bass, moving, shown.ambient_strength);
            }
            if shown.bg_shapes != nikgob_view::BgShapes::Off {
                let transients = [kick; 8];
                bg_shapes::preview(&tui, card, &shown, kick, &transients, egui::Id::new(("set-preset-bg", area, i)));
            }
            now_playing::preview(&tui, card.shrink(4.0), &shown, shown.ring);
            Theme::set(&ctx, app);
            if moving {
                painter.rect_filled(card, shape::MEDIUM, s.on_surface.gamma_multiply(0.08));
                ctx.request_repaint();
            }
            if selected {
                painter.rect_stroke(card, shape::MEDIUM, Stroke::new(2.0, s.primary), egui::StrokeKind::Inside);
            }
            let color = if selected { s.primary } else { s.on_surface_variant };
            ui.painter().text(
                egui::pos2(card.center().x, card.bottom() + 6.0),
                egui::Align2::CENTER_TOP,
                name,
                TypeRole::LabelLarge.font_id(),
                color,
            );
            rects.push(rect);
            if resp.clicked() && !selected {
                picked = Some(i);
            }
        }
    });
    (picked, rects)
}
