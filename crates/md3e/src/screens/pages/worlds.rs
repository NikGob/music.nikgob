//! The card at the top of Главная: «Миры». A world is a big body of music to travel through and
//! finish: a soundtrack franchise or an artist's discography. A row of
//! them, paged by its chevrons; the one chosen stands on a stage in the
//! colours of its own cover — what it is, its name (a link to its page),
//! how far the user got and the buttons that always mean the same
//! («Дослушать» / «Начать», «Перемешать»), three one-click cards under them
//! and the cover itself flush with the card's right end. With one world
//! there is no row (it is «Серия», A).
//!
//! Paging: the row shows as many tiles as fit; ‹ › slide it by a page on
//! the default spatial spring without changing the choice, and ←/→ walk
//! through every world, paging at the ends. The wheel always scrolls the
//! page.
//!
//! Choosing a world (a tile, ←/→): the tiles resize on the fast spatial
//! spring, the colours flow to the new cover's scheme (slow effects), the
//! text and the cards leave 32 to the left (fast effects) while a copy of
//! the new cover flies from its tile onto the stage (default spatial); the
//! new text comes in from 32 at the right after 150 ms with its name
//! settling from wght 300 / wdth 116 onto its axes, the cards rising one
//! after another. The stage cover drifts slowly and stops under the
//! pointer. Reduced motion: the switch is instant and nothing drifts.

use std::collections::HashMap;

use egui::{
    Color32, Context, CornerRadius, Id, Key, Rect, Sense, Ui, UiBuilder, epaint::text::VariationCoords, pos2,
    text::LayoutJob, vec2,
};
use nikgob_view::{Command, Route, Status, ViewState, World, WorldCard, WorldGo, WorldIcon};

use super::{PageState, cards, home::Grid};
use crate::{
    ColorScheme, Icon, Theme, TypeRole,
    components::{Button, IconButton, Menu, MenuItem, Radii, Size, Style, TooltipExt, anim, paint},
    tokens::{shape, state},
};

/// The space under the card, to the greeting.
pub const GAP_BELOW: f32 = 32.0;
/// The stage's padding.
const PAD: f32 = 32.0;

/// The card's height for a window `h` tall: 70 % of what is under the
/// search bar, a multiple of 8, 440…640 (568 at 900, 496 at 800).
pub fn height(window_h: f32) -> f32 {
    ((0.7 * (window_h - 88.0) / 8.0).floor() * 8.0).clamp(440.0, 640.0)
}

/// The side of a world's tile on a page `width` wide (the chosen one is 32
/// more): 80 at 1280, 88 at 1440, 104 at 1920.
pub fn tile_side(width: f32) -> f32 {
    if width < 1200.0 {
        80.0
    } else if width < 1500.0 {
        88.0
    } else {
        104.0
    }
}

fn key_id() -> Id {
    Id::new("home-world-focus")
}

fn page_id() -> Id {
    Id::new("home-world-page")
}

/// The tiles a page of the row holds in `width` (one of them chosen, 32
/// larger), at least one.
pub fn per_page(width: f32, base: f32) -> usize {
    (((width - 32.0 + 12.0) / (base + 12.0)).floor().max(1.0)) as usize
}

/// ←/→ (`dir` −1 or 1) from tile `i` of `n`: the tile it moves to and the
/// page that one is on, `per` to a page; the ends stay.
pub fn step(i: usize, n: usize, dir: i32, per: usize) -> (usize, usize) {
    let j = (i as i64 + i64::from(dir)).clamp(0, n.saturating_sub(1) as i64) as usize;
    (j, j / per.max(1))
}

/// Where the strip of tiles starts page `page` (tiles `base` + 12 apart;
/// the chosen one, at `chosen`, is 32 wider).
fn page_x(page: usize, per: usize, base: f32, chosen: usize) -> f32 {
    let k = page * per;
    k as f32 * (base + 12.0) + if chosen < k { 32.0 } else { 0.0 }
}

/// Chooses the world of `key` on the card, its page of the row shown (as
/// a click on its tile does, without the flight).
pub fn choose(ctx: &Context, key: &str) {
    ctx.data_mut(|d| {
        d.insert_temp::<Option<String>>(key_id(), Some(key.to_owned()));
        d.remove::<usize>(page_id());
    });
}

/// The world's page.
pub fn route(w: &World) -> Route {
    Route::World(w.key.clone())
}

fn stage_off_id() -> Id {
    Id::new("home-card-off")
}

/// The phone draws the worlds as a row of its own: the card stays off.
pub(crate) fn hide_stage(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(stage_off_id(), true));
}

/// The card takes room on the page now (shown, or still folding away).
pub fn shown(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new("home-card-shown"))).unwrap_or(false)
}

/// A world switch in progress.
#[derive(Clone, Debug)]
struct Switch {
    from: String,
    to: String,
    at: f64,
    /// Where the new cover's copy flies from.
    tile: Rect,
}

/// The progress of a spring from 0 to 1 at `t` seconds (0 before).
fn progress(spring: nikgob_motion::Spring, t: f32) -> f32 {
    if t <= 0.0 { 0.0 } else { 1.0 - spring.solve(1.0, 0.0, t).0 }
}

/// The scheme of a world's cover in the user's variant and mode.
pub(super) fn scheme_of(ctx: &Context, state: &ViewState, w: &World) -> ColorScheme {
    let seed = w.cover.as_ref().and_then(|c| c.seed).unwrap_or(state.prefs.fallback_seed);
    let (dark, variant, contrast) = crate::screens::theme_key(ctx, &state.prefs);
    ctx.data_mut(|d| {
        let cache = d.get_temp_mut_or_default::<HashMap<(u32, bool, u8, u8), ColorScheme>>(Id::new("home-world-schemes"));
        *cache
            .entry((seed, dark, variant as u8, contrast as u8))
            .or_insert_with(|| ColorScheme::from_seed(seed, variant, dark, contrast))
    })
}

/// The card's colours flowing to `target` (slow effects).
fn flowing(ctx: &Context, target: &ColorScheme) -> ColorScheme {
    let spring = Theme::get(ctx).motion.slow_effects;
    let mut s = *target;
    macro_rules! flow {
        ($($role:ident),*) => {
            $( s.$role = anim::color(ctx, Id::new(("home-world-color", stringify!($role))), target.$role, spring); )*
        };
    }
    flow!(
        primary,
        on_primary,
        primary_container,
        on_primary_container,
        secondary,
        on_secondary,
        secondary_container,
        on_secondary_container,
        outline,
        outline_variant,
        on_surface,
        on_surface_variant,
        surface_container_low,
        surface_container_high
    );
    s
}

/// Whether `w` plays now (the runtime's view of it, or the player's).
fn playing(state: &ViewState, w: &World) -> bool {
    matches!(state.player.status, Status::Playing | Status::Buffering)
        && state.player.current.as_ref().and_then(|t| t.release_id).is_some_and(|r| w.albums.contains(&r))
}

/// Draws the card (and its room) at the top of the page.
pub fn show(ui: &mut Ui, grid: Grid, state: &ViewState, n: usize, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let worlds = &state.home.worlds;
    let on = state.prefs.home_card_shown() && !worlds.is_empty() && !ctx.data(|d| d.get_temp::<bool>(stage_off_id())).unwrap_or(false);
    let k = anim::f32(&ctx, Id::new("home-card-k"), f32::from(u8::from(on)), theme.motion.default_spatial.without_bounce()).clamp(0.0, 1.0);
    let alpha_target = if on && k > 0.4 { 1.0 } else { 0.0 };
    let alpha_spring = if on { theme.motion.default_effects } else { theme.motion.fast_effects };
    let alpha = anim::fraction(&ctx, Id::new("home-card-alpha"), alpha_target, alpha_spring);
    let h2 = height(ctx.content_rect().height());
    let room = k * (h2 + GAP_BELOW);
    ctx.data_mut(|d| d.insert_temp(Id::new("home-card-shown"), room > 0.5));
    if room <= 0.5 || worlds.is_empty() {
        return None;
    }
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), room), Sense::hover());
    let card = Rect::from_min_size(pos2(grid.left, row.top()), vec2(grid.width, (k * h2).max(1.0)));
    let full = Rect::from_min_size(card.min, vec2(grid.width, h2));

    // Which world is chosen: kept while the page lives, else the playing one,
    // else the first.
    let mut focus: Option<String> = ctx.data(|d| d.get_temp::<Option<String>>(key_id())).flatten();
    if !focus.as_ref().is_some_and(|f| worlds.iter().any(|w| &w.key == f)) {
        let first = worlds.iter().find(|w| w.playing || playing(state, w)).or(worlds.first());
        focus = first.map(|w| w.key.clone());
        ctx.data_mut(|d| d.insert_temp::<Option<String>>(key_id(), focus.clone()));
    }
    let focus = focus?;
    let world = worlds.iter().find(|w| w.key == focus)?;
    let target = scheme_of(&ctx, state, world);
    let mut s = flowing(&ctx, &target);
    // On the card the widgets stand on secondary_container: what they draw
    // in the surface's roles takes the card's.
    s.on_surface = s.on_secondary_container;
    s.on_surface_variant = s.on_secondary_container;
    s.outline_variant = s.outline;

    let mut nav = None;
    super::home::entering(ui, row, n, "card", |ui| {
        let mut c = ui.new_child(UiBuilder::new().max_rect(card).id_salt("home-card"));
        c.set_clip_rect(card.intersect(ui.clip_rect()));
        if alpha < 1.0 {
            c.set_opacity(alpha);
        }
        // Widgets inside take the card's colours.
        let page_theme = Theme::get(&ctx);
        Theme::set(&ctx, page_theme.with_scheme(s));
        nav = stage(&mut c, full, state, world, &s, pages, out);
        Theme::set(&ctx, page_theme);
    });
    nav
}

/// The card's content in `card` (its full height; folding clips it).
fn stage(ui: &mut Ui, card: Rect, state: &ViewState, world: &World, s: &ColorScheme, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let worlds = &state.home.worlds;
    let r = shape::EXTRA_LARGE;
    paint::container(ui.painter(), card, Radii::all(r), s.secondary_container, egui::Stroke::NONE);

    // The cover: a square of the card's height flush with its right end
    // (in a narrow card at most half of it, and narrower still, down to
    // 38 %, for the whole row of worlds to fit; the picture is cropped at
    // the sides).
    let base = tile_side(card.width());
    let row_w = if worlds.len() > 1 { base + 32.0 + (worlds.len() - 1) as f32 * (base + 12.0) } else { 0.0 };
    let side = card.height().min(card.width() * 0.5).min((card.width() - 2.0 * PAD - row_w).max(card.width() * 0.38));
    let cover = Rect::from_min_size(pos2(card.right() - side, card.top()), vec2(side, card.height()));
    let stage = Rect::from_min_max(card.min, pos2(cover.left(), card.bottom())).shrink(PAD);

    // The switch in progress.
    let now = anim::now(&ctx);
    let sw: Option<Switch> = ctx.data(|d| d.get_temp(Id::new("home-world-switch")));
    let sw = sw.filter(|w| w.to == world.key && now - w.at < 1.4 && !theme.reduced_motion);
    if sw.is_some() {
        ctx.request_repaint();
    }
    let t = sw.as_ref().map_or(10.0, |w| (now - w.at) as f32);
    let old = sw.as_ref().and_then(|w| worlds.iter().find(|x| x.key == w.from));

    // Row of worlds, paged.
    let mut top = stage.top();
    let mut pick: Option<(String, Rect)> = None;
    if worlds.len() > 1 {
        let row_h = base + 32.0;
        let chosen = worlds.iter().position(|w| w.key == world.key).unwrap_or(0);
        // The chevrons take the row's right end when the worlds do not fit.
        let fits = per_page(stage.width(), base) >= worlds.len();
        let controls = if fits { 0.0 } else { 2.0 * Size::S.height() + 4.0 + 16.0 };
        let per = per_page(stage.width() - controls, base);
        // Exactly a page wide, so no tile of the next one peeks in.
        let page_w = if fits { stage.width() } else { per as f32 * (base + 12.0) - 12.0 + 32.0 };
        let area = Rect::from_min_size(pos2(stage.left(), top), vec2(page_w, row_h));
        let pages = worlds.len().div_ceil(per);
        let mut page = ctx.data(|d| d.get_temp::<usize>(page_id())).unwrap_or(chosen / per).min(pages - 1);
        let focused_any = ui.memory(|m| m.focused()).is_some_and(|f| worlds.iter().any(|w| f == tile_id(ui, &w.key)));
        let mut keyed: Option<usize> = None;
        if focused_any {
            let dir = ctx.input(|i| i32::from(i.key_pressed(Key::ArrowRight)) - i32::from(i.key_pressed(Key::ArrowLeft)));
            if dir != 0 {
                let (j, p) = step(chosen, worlds.len(), dir, per);
                if j != chosen {
                    keyed = Some(j);
                    page = p;
                    ui.memory_mut(|m| m.request_focus(tile_id(ui, &worlds[j].key)));
                }
            }
        }
        // The chevrons: a page at a time, the choice stays.
        if !fits {
            let d = Size::S.height();
            let y = top + (base - d) / 2.0;
            let next = Rect::from_min_size(pos2(stage.right() - d, y), vec2(d, d));
            let prev = Rect::from_min_size(pos2(next.left() - 4.0 - d, y), vec2(d, d));
            let mut c = ui.new_child(UiBuilder::new().max_rect(prev.union(next)).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt("home-world-pager"));
            c.spacing_mut().item_spacing.x = 4.0;
            // ‹ shows once there is a page before; its place stays.
            let back = anim::fraction(&ctx, Id::new("home-world-prev-shown"), f32::from(u8::from(page > 0)), theme.motion.fast_effects);
            let mut pc = c.new_child(UiBuilder::new().max_rect(prev).id_salt("home-world-prev"));
            if back < 1.0 {
                pc.set_opacity(back);
            }
            if back > 0.01
                && cards::scaled(&mut pc, prev.center(), 0.5 + 0.5 * back, |ui| {
                    ui.add_enabled(page > 0, IconButton::new(Icon::ChevronLeft).size(Size::S).style(Style::Standard).tooltip("Предыдущие миры").id_salt("home-world-prev-btn"))
                })
                .clicked()
            {
                page -= 1;
            }
            let mut nc = c.new_child(UiBuilder::new().max_rect(next).id_salt("home-world-next"));
            let more = page + 1 < pages;
            if nc
                .add_enabled(more, IconButton::new(Icon::ChevronRight).size(Size::S).style(Style::Standard).tooltip("Следующие миры").id_salt("home-world-next-btn"))
                .clicked()
            {
                page += 1;
            }
        }
        ctx.data_mut(|d| d.insert_temp(page_id(), page));
        // The strip slides to its page (default spatial).
        let off = anim::f32(&ctx, Id::new("home-world-strip"), page_x(page, per, base, chosen), theme.motion.default_spatial);
        let mut strip = ui.new_child(UiBuilder::new().max_rect(area).id_salt("home-world-row"));
        // The focus ring and the hover zoom keep a few points round the tiles.
        strip.set_clip_rect(area.expand2(vec2(0.0, 4.0)).intersect(ui.clip_rect()));
        let mut x = 0.0;
        for (i, w) in worlds.iter().enumerate() {
            let is_chosen = w.key == world.key;
            let size = anim::f32(&ctx, Id::new(("home-world-tile", &w.key)), if is_chosen { base + 32.0 } else { base }, theme.motion.fast_spatial);
            let radius = anim::f32(&ctx, Id::new(("home-world-tile-r", &w.key)), if is_chosen { shape::EXTRA_LARGE } else { shape::LARGE }, theme.motion.fast_spatial);
            let rect = Rect::from_min_size(pos2(area.left() + x - off, top), vec2(size, size));
            x += size + 12.0;
            if rect.right() < area.left() - 1.0 || rect.left() > area.right() + 1.0 {
                continue;
            }
            if tile(&mut strip, rect, radius.max(0.0), w, s, i) {
                pick = Some((w.key.clone(), rect));
            }
        }
        if let Some(j) = keyed {
            // The copy flies from where its tile will stand.
            let x = page_x(page, per, base, chosen);
            let at = j as f32 * (base + 12.0) + if chosen < j { 32.0 } else { 0.0 } - x;
            let r = Rect::from_min_size(pos2(area.left() + at, top), vec2(base, base));
            pick = Some((worlds[j].key.clone(), r));
        }
        top += row_h + 28.0;
    }

    // The stage cover: the old one fading under a copy of the new one
    // flying in from its tile; then the cover itself, drifting.
    let corners = CornerRadius { nw: 0, ne: r as u8, se: r as u8, sw: 0 };
    let cover_hover = ui.rect_contains_pointer(cover);
    let flying = sw.as_ref().filter(|_| t < 0.7);
    match (flying, old) {
        (Some(w), Some(o)) => {
            let fade = 1.0 - progress(theme.motion.default_effects, t);
            paint_drift(ui, cover, o.cover.as_ref().map(|c| c.uri()).as_deref(), corners, 1.0, (0.0, 0.0), fade);
            let p = progress(theme.motion.default_spatial, t);
            let rect = lerp_rect(w.tile, cover, p);
            let lerp = |a: f32, b: f32| a + (b - a) * p.clamp(0.0, 1.2);
            let fly = CornerRadius {
                nw: lerp(16.0, 0.0).clamp(0.0, 255.0) as u8,
                ne: lerp(16.0, r).clamp(0.0, 255.0) as u8,
                se: lerp(16.0, r).clamp(0.0, 255.0) as u8,
                sw: lerp(16.0, 0.0).clamp(0.0, 255.0) as u8,
            };
            paint_drift(ui, rect, world.cover.as_ref().map(|c| c.uri()).as_deref(), fly, 1.0, (0.0, 0.0), 1.0);
        }
        _ => {
            let (zoom, shift) = drift(&ctx, cover_hover && !theme.reduced_motion, theme.reduced_motion);
            paint_sharp(ui, cover, world, corners, zoom, shift);
        }
    }
    let cover_resp = ui.interact(cover, Id::new("home-world-cover"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let d = Size::M.height();
    let slot = Rect::from_min_size(cover.max - vec2(24.0 + d, 24.0 + d), vec2(d, d));
    let play_tip = world.primary_tip.clone();
    let cover_play = cards::grow_button(
        ui,
        Id::new("home-world-cover-play"),
        cover_resp.contains_pointer(),
        slot,
        IconButton::new(Icon::PlayArrow).size(Size::M).style(Style::Filled).filled_icon(true).tooltip(&play_tip).id_salt("home-world-cover-play-btn"),
    )
    .is_some_and(|b| b.clicked());
    if cover_play || cover_resp.clicked() {
        out.push(play_primary(world));
    }
    // Enter on a tile opens its world.
    let mut nav = None;
    if ui.memory(|m| m.focused()).is_some_and(|f| worlds.iter().any(|w| f == tile_id(ui, &w.key))) && ctx.input(|i| i.key_pressed(Key::Enter)) {
        nav = Some(route(world));
    }

    // The text and the cards: the old ones leaving, the new ones coming in.
    if let (Some(_), Some(o)) = (sw.as_ref(), old) {
        let p = progress(theme.motion.fast_effects, t);
        if p < 0.999 {
            let mut c = ui.new_child(UiBuilder::new().max_rect(stage).id_salt(("home-world-old", &o.key)));
            c.set_opacity(1.0 - p);
            cards::moved(&mut c, vec2(-32.0 * p, 0.0), |ui| {
                let _ = text_block(ui, stage, top, o, state, s, (500.0, 100.0), &mut Vec::new(), false);
                cards_row(ui, stage, o, state, s, None, &mut Vec::new(), false);
            });
        }
    }
    let t_in = t - 0.15;
    let (dx, a_in, axes) = if sw.is_some() && old.is_some() {
        let p = progress(theme.motion.default_spatial, t_in);
        let a = progress(theme.motion.fast_effects, t_in);
        let q = progress(theme.motion.fast_spatial, t_in);
        (32.0 * (1.0 - p), a, (300.0 + 200.0 * q, 116.0 - 16.0 * q))
    } else {
        (0.0, 1.0, (500.0, 100.0))
    };
    if a_in > 0.001 {
        let mut c = ui.new_child(UiBuilder::new().max_rect(stage).id_salt(("home-world-new", &world.key)));
        if a_in < 1.0 {
            c.set_opacity(a_in);
        }
        let live = a_in > 0.5;
        if let Some(r) = cards::moved(&mut c, vec2(dx, 0.0), |ui| text_block(ui, stage, top, world, state, s, axes, out, live)) {
            nav = Some(r);
        }
        let rise = sw.as_ref().filter(|_| old.is_some()).map(|_| t);
        let mut c = ui.new_child(UiBuilder::new().max_rect(stage).id_salt(("home-world-cards", &world.key)));
        if let Some(r) = cards_row(&mut c, stage, world, state, s, rise, out, true) {
            match r {
                Go::Route(r) => nav = Some(r),
                Go::Player => pages.open_now_playing = true,
            }
        }
    }

    if let Some((key, rect)) = pick
        && key != world.key
    {
        ctx.data_mut(|d| {
            d.insert_temp::<Option<String>>(key_id(), Some(key.clone()));
            d.insert_temp(Id::new("home-world-switch"), Switch { from: world.key.clone(), to: key, at: now, tile: rect });
        });
        ctx.request_repaint();
    }
    nav
}

fn tile_id(ui: &Ui, key: &str) -> Id {
    let _ = ui;
    Id::new(("home-world-tile-btn", key))
}

/// A world's tile: only its cover (the name in the tooltip and for the
/// screen reader); under the pointer the picture comes closer inside its
/// corners and the state layer shows; a press squishes it. True on click.
fn tile(ui: &mut Ui, rect: Rect, radius: f32, w: &World, s: &ColorScheme, i: usize) -> bool {
    let theme = Theme::get(ui.ctx());
    let id = tile_id(ui, &w.key);
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &w.title));
    let resp = resp.md3_tooltip(&w.title, None);
    let pressed = resp.is_pointer_button_down_on();
    let hover = resp.contains_pointer() || resp.has_focus();
    let squish = anim::f32(ui.ctx(), id.with("squish"), if pressed { 0.96 } else { 1.0 }, theme.motion.fast_spatial);
    let zoom = 1.0 + 0.06 * anim::fraction(ui.ctx(), id.with("zoom"), f32::from(u8::from(hover)), theme.motion.default_effects);
    let layer = anim::fraction(
        ui.ctx(),
        id.with("layer"),
        if pressed { state::PRESSED } else if hover { state::HOVER } else { 0.0 },
        theme.motion.fast_effects,
    );
    let c = CornerRadius::same(radius.round().clamp(0.0, 255.0) as u8);
    cards::scaled(ui, rect.center(), squish, |ui| {
        crate::components::paint_image_zoomed(ui, rect, w.cover.as_ref().map(|c| c.uri()).as_deref(), c, Icon::Album, zoom);
        paint::state_layer(ui.painter(), rect, Radii::all(radius), s.on_secondary_container, layer);
        if resp.has_focus() {
            paint::focus_ring(ui.painter(), rect, Radii::all(radius), s.secondary);
        }
    });
    let _ = i;
    resp.clicked()
}

/// The slow drift of the stage cover: 1 → 1.06 and a shift of −1.2 % /
/// −0.8 %, 30 s there and 30 back (ease in and out), stopped while `hold`.
fn drift(ctx: &Context, hold: bool, reduced: bool) -> (f32, (f32, f32)) {
    if reduced {
        return (1.0, (0.0, 0.0));
    }
    let now = anim::now(ctx);
    let id = Id::new("home-world-drift");
    let (phase, last) = ctx.data(|d| d.get_temp::<(f64, f64)>(id)).unwrap_or((0.0, now));
    let dt = (now - last).clamp(0.0, 0.25);
    let phase = if hold { phase } else { phase + dt };
    ctx.data_mut(|d| d.insert_temp(id, (phase, now)));
    // A tenth of a second is far under a pixel of this drift.
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
    let x = (phase % 60.0) / 30.0;
    let tri = if x < 1.0 { x } else { 2.0 - x } as f32;
    let e = 0.5 - 0.5 * (std::f32::consts::PI * tri).cos();
    (1.0 + 0.06 * e, (-0.012 * e, -0.008 * e))
}

/// A cover in `rect` with `corners`, magnified `zoom` times and its picture
/// shifted by `shift` (a share of its size), at `alpha`.
fn paint_drift(ui: &Ui, rect: Rect, uri: Option<&str>, corners: CornerRadius, zoom: f32, shift: (f32, f32), alpha: f32) {
    if alpha <= 0.001 {
        return;
    }
    if let Some(uri) = uri
        && let Some(texture) = crate::covers::texture(ui.ctx(), uri, rect.size(), zoom)
    {
        let (tw, th) = (texture.size.x.max(1.0), texture.size.y.max(1.0));
        let target = rect.width() / rect.height().max(1.0);
        let aspect = tw / th;
        let uv = if aspect > target {
            let f = target / aspect;
            Rect::from_min_max(pos2((1.0 - f) * 0.5, 0.0), pos2((1.0 + f) * 0.5, 1.0))
        } else {
            let f = aspect / target;
            Rect::from_min_max(pos2(0.0, (1.0 - f) * 0.5), pos2(1.0, (1.0 + f) * 0.5))
        };
        let size = uv.size() / zoom.max(1.0);
        let room = uv.size() - size;
        let center = uv.center() + vec2(shift.0 * uv.width(), shift.1 * uv.height());
        let center = pos2(
            center.x.clamp(uv.center().x - room.x / 2.0, uv.center().x + room.x / 2.0),
            center.y.clamp(uv.center().y - room.y / 2.0, uv.center().y + room.y / 2.0),
        );
        egui::Image::from_texture(texture)
            .texture_options(crate::components::PHOTO_TEXTURE)
            .uv(Rect::from_center_size(center, size))
            .corner_radius(corners)
            .tint(Color32::WHITE.gamma_multiply(alpha))
            .paint_at(ui, rect);
        return;
    }
    crate::components::paint_image_zoomed(ui, rect, None, corners, Icon::Album, 1.0);
}

/// The stage cover: the album's own at once, and the larger copy of a
/// service over it once that has loaded, fading in (default effects).
pub(super) fn paint_sharp(ui: &Ui, rect: Rect, w: &World, corners: CornerRadius, zoom: f32, shift: (f32, f32)) {
    let own = w.cover.as_ref().map(|c| c.uri());
    let hd = w.cover_hd.as_ref().map(|c| c.uri());
    let ready = hd.as_deref().is_some_and(|u| crate::covers::texture(ui.ctx(), u, rect.size(), zoom).is_some());
    let theme = Theme::get(ui.ctx());
    let k = anim::fraction(ui.ctx(), Id::new(("home-world-sharp", hd.as_deref().unwrap_or(""))), f32::from(u8::from(ready)), theme.motion.default_effects);
    if k < 0.999 {
        paint_drift(ui, rect, own.as_deref(), corners, zoom, shift, 1.0);
    }
    if ready {
        paint_drift(ui, rect, hd.as_deref(), corners, zoom, shift, k);
    }
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rect::from_min_max(pos2(l(a.left(), b.left()), l(a.top(), b.top())), pos2(l(a.right(), b.right()), l(a.bottom(), b.bottom())))
}

/// «Дослушать» / «Начать» / «С начала».
pub fn play_primary(w: &World) -> Command {
    Command::PlayTracks { tracks: w.primary.clone(), start: 0, context: format!("мира «{}»", w.title) }
}

/// The world's name in DisplayLarge 57/64 at `axes` (wght, wdth); a name
/// too long for `width` narrows on the width axis (not under 85) before it
/// is cut.
fn name_galley(ui: &Ui, name: &str, color: Color32, width: f32, axes: (f32, f32)) -> std::sync::Arc<egui::Galley> {
    let style = TypeRole::DisplayLarge.style();
    let job = |wght: f32, wdth: f32, cut: bool| {
        let format = egui::TextFormat {
            font_id: egui::FontId::new(style.size, crate::typography::family(400)),
            extra_letter_spacing: style.tracking,
            line_height: Some(style.line_height),
            color,
            coords: VariationCoords::new([(b"wght", wght), (b"wdth", wdth)]),
            ..Default::default()
        };
        let mut j = LayoutJob::single_section(name.to_owned(), format);
        if cut {
            j.wrap.max_width = width;
            j.wrap.max_rows = 1;
            j.wrap.overflow_character = Some('…');
        }
        j
    };
    // The width it settles on (wdth 100, narrowed when too long).
    let full = crate::typography::layout(ui.painter(), job(500.0, 100.0, false)).size().x;
    let settle = if full > width { (100.0 * width / full).floor().max(85.0) } else { 100.0 };
    // Quantized so a switch lays out a handful of faces, not one a frame.
    let wght = (axes.0 / 20.0).round() * 20.0;
    let wdth = (settle + (axes.1 - 100.0)).max(settle) ;
    let wdth = (wdth / 2.0).round() * 2.0;
    crate::typography::layout(ui.painter(), job(wght, wdth.min(116.0), true))
}

/// The kicker, the name, the facts and the buttons of `w` from `top`.
#[allow(clippy::too_many_arguments)]
fn text_block(
    ui: &mut Ui,
    stage: Rect,
    top: f32,
    w: &World,
    state: &ViewState,
    s: &ColorScheme,
    axes: (f32, f32),
    out: &mut Vec<Command>,
    live: bool,
) -> Option<Route> {
    let on = s.on_secondary_container;
    let kicker = if playing(state, w) { "Слушаешь сейчас" } else { w.kicker.as_str() };
    let k = crate::typography::layout(ui.painter(), TypeRole::LabelLarge.style().job(kicker, on));
    ui.painter().galley(pos2(stage.left(), top), k, on);
    let mut y = top + 20.0;
    let mut nav = None;
    // The name opens the world's page.
    let name = name_galley(ui, &w.title, on, stage.width(), axes);
    let name_rect = Rect::from_min_size(pos2(stage.left(), y), name.size());
    let name_resp = ui
        .interact(name_rect, Id::new(("home-world-name", &w.key)), if live { Sense::click() } else { Sense::hover() })
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let name_resp = if live { name_resp.md3_tooltip("Открыть мир", None) } else { name_resp };
    ui.painter().galley(name_rect.min, name, on);
    if name_resp.hovered() || name_resp.has_focus() {
        // A link: underlined under the pointer.
        let base_y = name_rect.bottom() - 6.0;
        ui.painter().line_segment([pos2(name_rect.left(), base_y), pos2(name_rect.right(), base_y)], egui::Stroke::new(2.0, on));
    }
    if name_resp.clicked() {
        nav = Some(route(w));
    }
    y += 72.0;
    // The room down to the cards decides whether the facts take two lines.
    let cards_top = stage.bottom() - 72.0;
    let tight = cards_top - y < 24.0 * 2.0 + 8.0 + 24.0 + 24.0 + Size::M.height() + 16.0;
    let facts = super::widgets::wrapped(ui, &w.facts, TypeRole::BodyLarge.style(), on, stage.width(), if tight { 1 } else { 2 });
    let fh = facts.size().y;
    ui.painter().galley(pos2(stage.left(), y), facts, on);
    y += fh.max(24.0) + 8.0;
    // How far: «Ты слышал 36 из 134» after the player's wavy progress.
    progress_line(ui, pos2(stage.left(), y), stage.width(), w, s, live);
    y += 24.0 + if tight { 16.0 } else { 24.0 };
    // [▶ Дослушать] [⤨ Перемешать] ⋮
    let row = Rect::from_min_size(pos2(stage.left(), y), vec2(stage.width(), Size::M.height()));
    let mut b = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt(("home-world-buttons", &w.key)));
    b.spacing_mut().item_spacing.x = 8.0;
    if !live {
        b.disable();
    }
    let label = w.start.label();
    let primary = b
        .add(Button::new(label).icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).id_salt(("home-world-primary", &w.key)))
        .md3_tooltip(&w.primary_tip, None);
    if primary.clicked() {
        out.push(play_primary(w));
    }
    if b.add(Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).style(Style::Outlined).id_salt(("home-world-shuffle", &w.key))).clicked() {
        out.push(Command::PlayTracks { tracks: w.shuffled.clone(), start: 0, context: format!("мира «{}»", w.title) });
    }
    let more = b.add(IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Standard).tooltip("Ещё").id_salt(("home-world-more", &w.key)));
    let items = world_items(state.home.worlds.len());
    let picked = world_menu(("home-world-menu", &w.key), &items).align_right(false).attach(&more);
    match picked.and_then(|k| items.get(k).copied()) {
        Some(WorldItem::Open) => return Some(route(w)),
        Some(WorldItem::PlayNext) => out.push(Command::PlayNext(w.primary.clone())),
        Some(WorldItem::Enqueue) => out.push(Command::Enqueue(w.primary.clone())),
        Some(WorldItem::HideWorld) => out.push(Command::SetPref(nikgob_view::PrefPatch::HideWorld(w.key.clone()))),
        Some(WorldItem::HideCard) => out.push(Command::HideHomeCard),
        None => {}
    }
    nav
}

/// An item of the worlds card's ⋮.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldItem {
    Open,
    PlayNext,
    Enqueue,
    HideWorld,
    HideCard,
}

/// The worlds card's ⋮: «Открыть мир», what «Дослушать» plays next or to
/// the queue │ «Убрать из ряда» (while there is another world to show) and
/// «Скрыть карточку».
pub fn world_items(worlds: usize) -> Vec<WorldItem> {
    let mut items = vec![WorldItem::Open, WorldItem::PlayNext, WorldItem::Enqueue];
    if worlds > 1 {
        items.push(WorldItem::HideWorld);
    }
    items.push(WorldItem::HideCard);
    items
}

fn world_menu(key: impl egui::AsId, items: &[WorldItem]) -> Menu {
    let mut m = Menu::new(key);
    for (k, item) in items.iter().enumerate() {
        if k > 0 && matches!(item, WorldItem::HideWorld | WorldItem::HideCard) && !matches!(items[k - 1], WorldItem::HideWorld) {
            m = m.divider();
        }
        m = m.item(match item {
            WorldItem::Open => MenuItem::new("Открыть мир").icon(Icon::Album),
            WorldItem::PlayNext => MenuItem::new(super::PLAY_NEXT).icon(Icon::QueueMusic),
            WorldItem::Enqueue => MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic),
            WorldItem::HideWorld => MenuItem::new("Убрать из ряда").icon(Icon::Block),
            WorldItem::HideCard => MenuItem::new("Скрыть карточку").icon(Icon::Close),
        });
    }
    m
}

/// «Ты слышал 36 из 134», «Ты слышал все 134», «Ты ещё не слышал ни одного
/// из 160».
pub fn heard_line(heard: u32, total: u32) -> String {
    if heard == 0 {
        format!("Ты ещё не слышал ни одного из {total}")
    } else if heard >= total {
        format!("Ты слышал все {total}")
    } else {
        format!("Ты слышал {heard} из {total}")
    }
}

/// The progress line of a world at `at`: the wavy progress of the player
/// (its track in the card's colours) and how many of its tracks were heard.
pub fn progress_line(ui: &mut Ui, at: egui::Pos2, width: f32, w: &World, s: &ColorScheme, live: bool) {
    let on = s.on_secondary_container;
    let text = super::widgets::wrapped(ui, &heard_line(w.heard, w.total), TypeRole::BodyMedium.style(), on, width, 1);
    let bar_w = (width - text.size().x - 16.0).clamp(80.0, 240.0);
    let row = Rect::from_min_size(at, vec2(bar_w, 24.0));
    let mut c = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt(("home-world-progress", &w.key)));
    let f = if w.total == 0 { 0.0 } else { w.heard as f32 / w.total as f32 };
    // Before the text comes in, the bar starts empty and fills (as A's do).
    crate::components::wavy_progress_on(&mut c, ("world-progress", &w.key), if live { f } else { 0.0 }, bar_w, on.gamma_multiply(0.24));
    let ty = at.y + (24.0 - text.size().y) / 2.0;
    ui.painter().galley(pos2(at.x + bar_w + 16.0, ty), text, on);
}

enum Go {
    Route(Route),
    Player,
}

/// The three cards at the bottom of the stage; `rise`: seconds since a
/// switch (each card rises 16 at 60 + 40·k ms).
#[allow(clippy::too_many_arguments)]
fn cards_row(
    ui: &mut Ui,
    stage: Rect,
    w: &World,
    state: &ViewState,
    s: &ColorScheme,
    rise: Option<f32>,
    out: &mut Vec<Command>,
    live: bool,
) -> Option<Go> {
    let theme = Theme::get(ui.ctx());
    let mut list: Vec<WorldCard> = w.cards.clone();
    if playing(state, w)
        && let Some(cur) = state.player.current.as_ref()
    {
        let now = WorldCard { icon: WorldIcon::NowPlaying, kicker: "Сейчас играет".into(), title: cur.title.clone(), go: WorldGo::Player };
        if list.first().is_some_and(|c| c.icon != WorldIcon::NowPlaying) {
            list[0] = now;
        } else if list.is_empty() {
            list.push(now);
        }
    }
    let gap = 12.0;
    // Thirds; a world with fewer cards shares the row between them.
    let n = list.len().clamp(1, 3) as f32;
    let cw = (stage.width() - (n - 1.0) * gap) / n;
    let top = stage.bottom() - 72.0;
    let mut go = None;
    for (k, c) in list.iter().take(3).enumerate() {
        let rect = Rect::from_min_size(pos2(stage.left() + k as f32 * (cw + gap), top), vec2(cw, 72.0));
        let (dy, a) = match rise {
            Some(t) => {
                let t = t - 0.06 - 0.04 * k as f32;
                (16.0 * (1.0 - progress(theme.motion.default_spatial, t)), progress(theme.motion.fast_effects, t))
            }
            None => (0.0, 1.0),
        };
        if a <= 0.001 {
            continue;
        }
        let mut cui = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(("home-world-card", &w.key, k)));
        if a < 1.0 {
            cui.set_opacity(a);
        }
        let clicked = cards::moved(&mut cui, vec2(0.0, dy), |ui| world_card(ui, rect, c, s, (&w.key, k)));
        if clicked && live {
            match &c.go {
                WorldGo::Player => go = Some(Go::Player),
                WorldGo::Play(t) => out.push(Command::PlayTracks { tracks: t.clone(), start: 0, context: format!("мира «{}»", w.title) }),
                WorldGo::Release(r) => go = Some(Go::Route(Route::Release(*r))),
                WorldGo::Stream(r) => go = Some(Go::Route(Route::Stream(nikgob_view::StreamPage::Album(r.clone())))),
            }
        }
    }
    go
}

/// One card: 72 high, r16, on_secondary_container at 8 %, the icon 24 in
/// primary, the kicker and the title a line each.
fn world_card(ui: &mut Ui, rect: Rect, c: &WorldCard, s: &ColorScheme, salt: impl std::hash::Hash + std::fmt::Debug) -> bool {
    let theme = Theme::get(ui.ctx());
    let id = Id::new(("home-world-card-btn", salt));
    let resp = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let pressed = resp.is_pointer_button_down_on();
    let hover = resp.contains_pointer() || resp.has_focus();
    let squish = anim::f32(ui.ctx(), id.with("squish"), if pressed { 0.98 } else { 1.0 }, theme.motion.fast_spatial);
    let layer = anim::fraction(
        ui.ctx(),
        id.with("layer"),
        if pressed { state::PRESSED } else if hover { state::HOVER } else { 0.0 },
        theme.motion.fast_effects,
    );
    let r = shape::LARGE;
    cards::scaled(ui, rect.center(), squish, |ui| {
        paint::container(ui.painter(), rect, Radii::all(r), s.on_secondary_container.gamma_multiply(0.08), egui::Stroke::NONE);
        paint::state_layer(ui.painter(), rect, Radii::all(r), s.on_secondary_container, layer);
        let icon = match c.icon {
            WorldIcon::NowPlaying => Icon::GraphicEq,
            WorldIcon::Play => Icon::PlayArrow,
            WorldIcon::Next => Icon::SkipNext,
            WorldIcon::Album => Icon::Album,
            WorldIcon::Favorite => Icon::Favorite,
            WorldIcon::Sound => Icon::GraphicEq,
        };
        let ir = Rect::from_center_size(pos2(rect.left() + 16.0 + 12.0, rect.center().y), vec2(24.0, 24.0));
        icon.widget(c.icon == WorldIcon::Favorite, s.primary, 24.0).paint_at(ui, ir);
        let left = ir.right() + 16.0;
        let w = (rect.right() - 16.0 - left).max(10.0);
        let k = super::widgets::wrapped(ui, &c.kicker, TypeRole::LabelMedium.style(), s.on_secondary_container, w, 1);
        let t = super::widgets::wrapped(ui, &c.title, TypeRole::TitleSmall.style(), s.on_secondary_container, w, 1);
        let y = rect.center().y - (k.size().y + t.size().y) / 2.0;
        let kh = k.size().y;
        ui.painter().galley(pos2(left, y), k, s.on_secondary_container);
        ui.painter().galley(pos2(left, y + kh), t, s.on_secondary_container);
    });
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worlds card's ⋮ can play the world next or queue it, and offers
    /// «Убрать из ряда» only while there is another world to show.
    #[test]
    fn the_worlds_menu_plays_next_and_hides_what_cannot_work() {
        use WorldItem as W;
        assert_eq!(world_items(3), [W::Open, W::PlayNext, W::Enqueue, W::HideWorld, W::HideCard]);
        assert_eq!(world_items(1), [W::Open, W::PlayNext, W::Enqueue, W::HideCard]);
    }

    #[test]
    fn the_card_takes_seventy_percent_under_the_search() {
        assert_eq!(height(900.0), 568.0);
        assert_eq!(height(800.0), 496.0);
        assert_eq!(height(1080.0), 640.0);
        assert_eq!(height(600.0), 440.0);
        assert_eq!(tile_side(1296.0), 88.0);
        assert_eq!(tile_side(1136.0), 80.0);
        assert_eq!(tile_side(1552.0), 104.0);
    }

    #[test]
    fn the_row_pages_through_every_world() {
        // 88 tiles 12 apart, one 32 larger: six in 664, one at least.
        assert_eq!(per_page(664.0, 88.0), 6);
        assert_eq!(per_page(100.0, 88.0), 1);
        // ←/→ walk all nine, paging at the ends of a page, stopping at the ends.
        let walk: Vec<(usize, usize)> = (0..9).scan(0, |i, _| {
            let next = step(*i, 9, 1, 6);
            *i = next.0;
            Some(next)
        })
        .collect();
        assert_eq!(walk, [(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 1), (7, 1), (8, 1), (8, 1)]);
        assert_eq!(step(6, 9, -1, 6), (5, 0));
        assert_eq!(step(0, 9, -1, 6), (0, 0));
        // A page starts where its first tile stands, past the chosen one's 32.
        assert_eq!(page_x(0, 6, 88.0, 0), 0.0);
        assert_eq!(page_x(1, 6, 88.0, 0), 6.0 * 100.0 + 32.0);
        assert_eq!(page_x(1, 6, 88.0, 7), 600.0);
    }
}
