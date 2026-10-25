//! The card at the top of Главная: «Миры» (home-v2 spec §15.3, placement
//! and hiding §14). A row of the user's soundtrack worlds; the one chosen
//! stands on a stage in the colours of its own cover — its name, what is
//! left of it and one button that plays it, three one-click cards under
//! them and the cover itself flush with the card's right end. With one
//! world there is no row (it is «Серия», A).
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
fn scheme_of(ctx: &Context, state: &ViewState, w: &World) -> ColorScheme {
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
    let on = state.prefs.home_card && !worlds.is_empty();
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

    // Row of worlds.
    let mut top = stage.top();
    let mut pick: Option<(String, Rect)> = None;
    if worlds.len() > 1 {
        let row_h = base + 32.0;
        let mut x = stage.left();
        let focused_any = ui.memory(|m| m.focused()).is_some_and(|f| worlds.iter().any(|w| f == tile_id(ui, &w.key)));
        for (i, w) in worlds.iter().enumerate() {
            let chosen = w.key == world.key;
            let size = anim::f32(&ctx, Id::new(("home-world-tile", &w.key)), if chosen { base + 32.0 } else { base }, theme.motion.fast_spatial);
            let radius = anim::f32(&ctx, Id::new(("home-world-tile-r", &w.key)), if chosen { shape::EXTRA_LARGE } else { shape::LARGE }, theme.motion.fast_spatial);
            let rect = Rect::from_min_size(pos2(x, top), vec2(size, size));
            if rect.right() > stage.right() + 1.0 {
                break;
            }
            if tile(ui, rect, radius.max(0.0), w, s, i) {
                pick = Some((w.key.clone(), rect));
            }
            x += size + 12.0;
        }
        if focused_any {
            let step = ctx.input(|i| i32::from(i.key_pressed(Key::ArrowRight)) - i32::from(i.key_pressed(Key::ArrowLeft)));
            if step != 0
                && let Some(i) = worlds.iter().position(|w| w.key == world.key)
            {
                let j = (i as i32 + step).clamp(0, worlds.len() as i32 - 1) as usize;
                if j != i {
                    let next = &worlds[j];
                    let r = Rect::from_min_size(pos2(stage.left() + j as f32 * (base + 12.0), top), vec2(base, base));
                    pick = Some((next.key.clone(), r));
                    ui.memory_mut(|m| m.request_focus(tile_id(ui, &next.key)));
                }
            }
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
            paint_drift(ui, cover, world.cover.as_ref().map(|c| c.uri()).as_deref(), corners, zoom, shift, 1.0);
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
    if ui.memory(|m| m.focused()).is_some_and(|f| worlds.iter().any(|w| f == tile_id(ui, &w.key))) && ctx.input(|i| i.key_pressed(Key::Enter)) {
        out.push(play_primary(world));
    }

    // The text and the cards: the old ones leaving, the new ones coming in.
    let mut nav = None;
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
        nav = cards::moved(&mut c, vec2(dx, 0.0), |ui| text_block(ui, stage, top, world, state, s, axes, out, live));
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

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    let l = |x: f32, y: f32| x + (y - x) * t;
    Rect::from_min_max(pos2(l(a.left(), b.left()), l(a.top(), b.top())), pos2(l(a.right(), b.right()), l(a.bottom(), b.bottom())))
}

/// «Дослушать» / «Начать».
fn play_primary(w: &World) -> Command {
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
    let name = name_galley(ui, &w.title, on, stage.width(), axes);
    ui.painter().galley(pos2(stage.left(), y), name, on);
    y += 72.0;
    let facts = super::widgets::wrapped(ui, &w.facts, TypeRole::BodyLarge.style(), on, stage.width(), 2);
    let fh = facts.size().y;
    ui.painter().galley(pos2(stage.left(), y), facts, on);
    y += fh.max(24.0) + 24.0;
    // [▶ Дослушать] [⤨ Вперемешку] ⋮
    let row = Rect::from_min_size(pos2(stage.left(), y), vec2(stage.width(), Size::M.height()));
    let mut b = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)).id_salt(("home-world-buttons", &w.key)));
    b.spacing_mut().item_spacing.x = 8.0;
    if !live {
        b.disable();
    }
    let label = if w.fresh { "Начать" } else { "Дослушать" };
    let primary = b
        .add(Button::new(label).icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).id_salt(("home-world-primary", &w.key)))
        .md3_tooltip(&w.primary_tip, None);
    if primary.clicked() {
        out.push(play_primary(w));
    }
    if b.add(Button::new("Вперемешку").icon(Icon::Shuffle).size(Size::M).style(Style::Outlined).id_salt(("home-world-shuffle", &w.key))).clicked() {
        out.push(Command::PlayTracks { tracks: w.shuffled.clone(), start: 0, context: format!("мира «{}»", w.title) });
    }
    let more = b.add(IconButton::new(Icon::MoreVert).size(Size::M).style(Style::Standard).tooltip("Ещё").id_salt(("home-world-more", &w.key)));
    let picked = Menu::new(("home-world-menu", &w.key))
        .item(MenuItem::new("Открыть мир").icon(Icon::Album))
        .item(MenuItem::new("В очередь").icon(Icon::QueueMusic))
        .item(MenuItem::new("Убрать из ряда").icon(Icon::Block).enabled(state.home.worlds.len() > 1))
        .divider()
        .item(MenuItem::new("Скрыть карточку").icon(Icon::Close))
        .align_right(false)
        .attach(&more);
    match picked {
        Some(0) => return w.release.map(Route::Release),
        Some(1) => out.push(Command::Enqueue(w.primary.clone())),
        Some(2) => {
            let mut p = state.prefs.clone();
            if !p.home_hidden_worlds.contains(&w.key) {
                p.home_hidden_worlds.push(w.key.clone());
            }
            out.push(Command::SetPrefs(Box::new(p)));
        }
        Some(3) => out.push(Command::HideHomeCard),
        _ => {}
    }
    None
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
}
