//! The pieces of Главная (home-v2 spec §3): a shortcut, a mix with its
//! cover and its name on it, the tile of «Любимые», a row of a song, a tile of the day and an album card, each laid
//! out in the rect the page's grid gives it, plus the motion helpers they
//! share (a picture moved or scaled after it was painted, a button that
//! grows in under the pointer).

use std::sync::Arc;

use egui::{Color32, Galley, Id, Pos2, Rect, Response, Sense, Ui, UiBuilder, Vec2, emath::TSTransform, pos2, vec2};
use nikgob_view::{Command, LaneCard, MixCard, PlaylistCard, Route, Shortcut, Status, ViewState};

use crate::{
    Icon, Theme, TypeRole, TypeStyle,
    components::{IconButton, Menu, MenuItem, Radii, Shape, Size, Style, TooltipExt, anim, button::Colors, paint},
    tokens::{shape, state},
};

/// Under a mix's tile: 8, its name (24) and a line of its artists (20).
pub const MIX_TEXT_H: f32 = 8.0 + 44.0;
/// A line of a tile's name.
const TITLE_LINE: f32 = 24.0;
/// Under an album's cover: 8, two lines of the title (40) and a line (16).
pub const ALBUM_TEXT_H: f32 = 8.0 + 40.0 + 16.0;

/// Draws `add`, then moves what it painted by `by`. Input keeps the
/// layout's rects (an entrance is over in a moment).
pub fn moved<R>(ui: &mut Ui, by: Vec2, add: impl FnOnce(&mut Ui) -> R) -> R {
    transformed(ui, TSTransform::from_translation(by), by.length() > 1e-3, add)
}

/// Draws `add`, then scales what it painted by `k` around `center`.
pub fn scaled<R>(ui: &mut Ui, center: Pos2, k: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = TSTransform::new(center.to_vec2() * (1.0 - k), k);
    transformed(ui, t, (k - 1.0).abs() > 1e-4, add)
}

fn transformed<R>(ui: &mut Ui, t: TSTransform, apply: bool, add: impl FnOnce(&mut Ui) -> R) -> R {
    let layer = ui.layer_id();
    let start = ui.ctx().graphics(|g| g.get(layer).map_or(egui::layers::ShapeIdx(0), |l| l.next_idx()));
    let r = add(ui);
    if apply {
        ui.ctx().graphics_mut(|g| {
            let list = g.entry(layer);
            let end = list.next_idx();
            list.transform_range(start, end, t);
        });
    }
    r
}

/// An icon button growing in at `slot` from half its size (fast spatial)
/// and fading in (fast effects) while `shown`; the response once it is
/// mostly there.
#[allow(clippy::too_many_arguments)]
pub fn grow_button(ui: &mut Ui, id: Id, shown: bool, slot: Rect, button: IconButton) -> Option<Response> {
    let theme = Theme::get(ui.ctx());
    let target = f32::from(u8::from(shown));
    let k = anim::fraction(ui.ctx(), id.with("fade"), target, theme.motion.fast_effects).clamp(0.0, 1.0);
    if k <= 0.01 {
        return None;
    }
    let grow = anim::f32(ui.ctx(), id.with("grow"), target, theme.motion.fast_spatial);
    let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt(id.with("slot")));
    child.set_opacity(k * ui.opacity());
    let r = scaled(&mut child, slot.center(), 0.5 + 0.5 * grow.max(0.0), |ui| ui.add_enabled(k > 0.5, button));
    Some(r)
}

/// ▶ (⏸ in a square while it plays) in primary.
/// The ⋮ of a card under the pointer (`shown`): a small filled button at
/// `slot` that grows in, as on the mix cards.
pub fn more_button(ui: &mut Ui, id: Id, shown: bool, slot: Rect) -> Option<Response> {
    let s = Theme::get(ui.ctx()).scheme;
    let colors = Colors { container: s.surface_container_high, content: s.on_surface, border: Color32::TRANSPARENT };
    let button = IconButton::new(Icon::MoreVert).size(Size::S).style(Style::Filled).colors(colors).tooltip("Ещё").id_salt(id.with("more-btn"));
    grow_button(ui, id.with("more"), shown, slot, button)
}

/// Keeps whether the menu of the card `id`'s ⋮ (`more`) is open, for the
/// card to stay as under the pointer (its ⋮ shown) while it is: read next
/// frame from `id.with("more-open")`.
pub fn keep_more_open(ctx: &egui::Context, id: Id, more: Option<&Response>) {
    let open = more.is_some_and(|m| ctx.data(|d| d.get_temp::<bool>(m.id.with("md3e-menu-open"))).unwrap_or(false));
    ctx.data_mut(|d| d.insert_temp(id.with("more-open"), open));
}

fn play_button(playing: bool, size: Size, tip: &str, salt: impl egui::AsId) -> IconButton {
    let s_icon = if playing { Icon::Pause } else { Icon::PlayArrow };
    let mut b = IconButton::new(s_icon).size(size).style(Style::Filled).filled_icon(true).tooltip(tip).id_salt(salt);
    if playing {
        b = b.shape(Shape::Square);
    }
    b
}

/// The press squish, the state layer and the hover of `resp`.
fn feedback(ui: &Ui, id: Id, resp: &Response, depth: f32) -> (f32, f32, bool) {
    let theme = Theme::get(ui.ctx());
    let pressed = resp.is_pointer_button_down_on();
    let squish = anim::f32(ui.ctx(), id.with("squish"), if pressed { depth } else { 1.0 }, theme.motion.fast_spatial);
    let hover = resp.contains_pointer() || resp.has_focus();
    let target = if pressed {
        state::PRESSED
    } else if resp.has_focus() {
        state::FOCUS
    } else if hover {
        state::HOVER
    } else {
        0.0
    };
    let layer = anim::fraction(ui.ctx(), id.with("layer"), target, theme.motion.fast_effects);
    (squish, layer, hover)
}

/// `text` in `role`, wrapped to `width` in at most `rows` rows.
fn text(ui: &Ui, s: &str, role: TypeStyle, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    super::widgets::wrapped(ui, s, role, color, width, rows)
}

fn line_w(ui: &Ui, s: &str, role: TypeStyle) -> f32 {
    crate::typography::layout(ui.painter(), role.job(s, Color32::WHITE)).size().x
}

/// `rect` with its edges on whole pixels, so a card and the cover flush
/// with its edge start on the same pixel in every column of the grid.
pub fn on_pixels(ui: &Ui, rect: Rect) -> Rect {
    let ppp = ui.ctx().pixels_per_point().max(0.01);
    let f = |v: f32| (v * ppp).round() / ppp;
    Rect::from_min_max(pos2(f(rect.min.x), f(rect.min.y)), pos2(f(rect.max.x), f(rect.max.y)))
}

/// A cover in `rect` cut to the outline a container of `radii` has there
/// (the card's own corners where they share an edge), `zoom` times closer.
fn cover_in(ui: &Ui, rect: Rect, uri: Option<&str>, radii: Radii, fallback: Icon, zoom: f32) {
    crate::components::paint_image_shaped(ui, rect, uri, radii, fallback, zoom, &|_, _| false);
}

/// Covers as a 2×2 mosaic (2 apart) in a square of radius `r`; fewer than
/// four: the first alone. Magnified `zoom` times inside.
pub fn mosaic(ui: &Ui, rect: Rect, covers: &[String], r: f32, gap: f32, zoom: f32) {
    if covers.len() < 4 {
        cover_in(ui, rect, covers.first().map(String::as_str), Radii::all(r), Icon::QueueMusic, zoom);
        return;
    }
    let half = (rect.size() - vec2(gap, gap)) * 0.5;
    for (i, uri) in covers.iter().take(4).enumerate() {
        let (col, row) = ((i % 2) as f32, (i / 2) as f32);
        let q = Rect::from_min_size(rect.min + vec2(col * (half.x + gap), row * (half.y + gap)), half);
        let c = match i {
            0 => Radii([r, 0.0, 0.0, 0.0]),
            1 => Radii([0.0, r, 0.0, 0.0]),
            2 => Radii([0.0, 0.0, 0.0, r]),
            _ => Radii([0.0, 0.0, r, 0.0]),
        };
        cover_in(ui, q, Some(uri), c, Icon::QueueMusic, zoom);
    }
}

/// «Серега Пират, mzlff, MORGENSHTERN и другие»: as many whole names as fit
/// in `rows` lines of `width`, in their order — a name that does not fit is
/// skipped for a shorter one after it; «и …» ends the line. With `new`
/// names: «Серега Пират, mzlff и новое: Пошлая Молли».
pub fn names_line(fits: &dyn Fn(&str) -> bool, names: &[String], new: &[String]) -> String {
    let compose = |mine: &[&str], fresh: &[&str], all_mine: bool| -> String {
        let mut s = String::new();
        if fresh.is_empty() {
            match mine {
                [] => {}
                [a] if all_mine => s.push_str(a),
                [a, b] if all_mine => s = format!("{a} и {b}"),
                _ if all_mine => {
                    let (last, head) = mine.split_last().unwrap_or((&"", &[]));
                    s = format!("{} и {last}", head.join(", "));
                }
                _ => s = format!("{} и другие", mine.join(", ")),
            }
        } else {
            s = format!("{} и новое: {}", mine.join(", "), fresh.join(", "));
        }
        s
    };
    // The user's own names, with room for the first new one (when even one
    // name and it do not fit, the new one gives way).
    let own = |fresh: &[&str]| -> Vec<&str> {
        let mut mine: Vec<&str> = Vec::new();
        for (k, n) in names.iter().enumerate() {
            let mut trial = mine.clone();
            trial.push(n);
            let all = k + 1 == names.len() && mine.len() + 1 == names.len();
            if fits(&compose(&trial, fresh, all)) {
                mine = trial;
            }
            if fresh.is_empty() && mine.len() >= 3 {
                // Enough: the line says «и другие» after three.
                break;
            }
        }
        mine
    };
    let first_new: Vec<&str> = new.iter().take(1).map(String::as_str).collect();
    let mut mine = own(&first_new);
    if mine.is_empty() && !first_new.is_empty() {
        mine = own(&[]);
    }
    // Not one whole name fits: the first, cut where the line ends.
    if mine.is_empty()
        && let Some(n) = names.first()
    {
        mine.push(n);
    }
    let all_mine = mine.len() == names.len();
    let mut fresh: Vec<&str> = Vec::new();
    for n in new {
        let mut trial = fresh.clone();
        trial.push(n);
        if fits(&compose(&mine, &trial, all_mine)) {
            fresh = trial;
        }
    }
    compose(&mine, &fresh, all_mine)
}

// ---- Shortcut.

/// The track playing now belongs to `sc`.
fn shortcut_playing(state: &ViewState, sc: &Shortcut) -> bool {
    if !matches!(state.player.status, Status::Playing | Status::Buffering) {
        return false;
    }
    let Some(cur) = state.player.current.as_ref() else { return false };
    match sc {
        Shortcut::Release(r) => cur.release_id == Some(r.id),
        Shortcut::Artist(a) => cur.artist_id() == Some(a.id),
        Shortcut::Playlist(p, _) => state.player.context.as_deref().is_some_and(|c| c.contains(&format!("«{}»", p.name))),
    }
}

/// A shortcut (§3.2): 72 high, the cover flush with the edge (a playlist:
/// a 2×2 mosaic; an artist: a circle 56 inset 8), the name in two lines and
/// a quiet line; the equalizer while it plays. A click opens it, ▶ on the
/// cover plays it.
pub fn shortcut(ui: &mut Ui, cell: Rect, sc: &Shortcut, st: &ViewState, i: usize, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let cell = on_pixels(ui, cell);
    let id = ui.id().with(("home-shortcut", i));
    let resp = ui.interact(cell, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let (squish, layer, hover) = feedback(ui, id, &resp, 0.98);
    let r = shape::MEDIUM;
    let playing = shortcut_playing(st, sc);
    let (title, sub, route, play) = match sc {
        Shortcut::Release(a) => (
            a.title.clone(),
            a.artist.clone(),
            Route::Release(a.id),
            Command::PlayRelease { id: a.id, start: None, shuffle: false },
        ),
        Shortcut::Playlist(p, _) => {
            let n = u64::from(p.track_count);
            (
                p.name.clone(),
                format!("Плейлист · {n} {}", crate::screens::common::plural(n, "трек", "трека", "треков")),
                Route::Playlist(p.id),
                Command::PlayPlaylist { id: p.id, start: None, shuffle: false },
            )
        }
        Shortcut::Artist(a) => (a.name.clone(), "Исполнитель".to_owned(), Route::Artist(a.id), Command::PlayArtist { id: a.id, shuffle: false }),
    };
    let cover = Rect::from_min_size(cell.min, vec2(cell.height(), cell.height()));
    let clicked_play = scaled(ui, cell.center(), squish, |ui| {
        paint::container(ui.painter(), cell, Radii::all(r), s.surface_container, egui::Stroke::NONE);
        match sc {
            Shortcut::Release(a) => {
                let uri = a.cover.as_ref().map(|c| c.uri());
                cover_in(ui, cover, uri.as_deref(), Radii::sides(r, 0.0), Icon::Album, 1.0);
            }
            Shortcut::Playlist(p, covers) => {
                let uris: Vec<String> = covers.iter().map(|c| c.uri()).collect();
                if uris.len() >= 4 {
                    // The mosaic keeps the tile's corners at the left; its
                    // four pictures meet without a seam.
                    let mid = cover.center();
                    let seam = |a: Pos2, b: Pos2| {
                        let on = |v: f32, m: f32| (v - m).abs() < 0.01;
                        (on(a.x, mid.x) && on(b.x, mid.x)) || (on(a.y, mid.y) && on(b.y, mid.y))
                    };
                    for (k, u) in uris.iter().take(4).enumerate() {
                        let min = pos2(if k % 2 == 0 { cover.left() } else { mid.x }, if k / 2 == 0 { cover.top() } else { mid.y });
                        let max = pos2(if k % 2 == 0 { mid.x } else { cover.right() }, if k / 2 == 0 { mid.y } else { cover.bottom() });
                        let c = match k {
                            0 => Radii([r, 0.0, 0.0, 0.0]),
                            2 => Radii([0.0, 0.0, 0.0, r]),
                            _ => Radii::all(0.0),
                        };
                        crate::components::paint_image_shaped(ui, Rect::from_min_max(min, max), Some(u), c, Icon::QueueMusic, 1.0, &seam);
                    }
                } else {
                    let uri = p.cover.as_ref().map(|c| c.uri()).or_else(|| uris.first().cloned());
                    cover_in(ui, cover, uri.as_deref(), Radii::sides(r, 0.0), Icon::QueueMusic, 1.0);
                }
            }
            Shortcut::Artist(a) => {
                let circle = cover.shrink(8.0);
                super::widgets::artist_picture(ui, a, circle);
            }
        }
        paint::state_layer(ui.painter(), cell, Radii::all(r), s.on_surface, layer);
        // The name and the line, centred as a block.
        let left = cover.right() + 16.0;
        let trail = if playing { 48.0 } else { 16.0 };
        let w = (cell.right() - trail - left).max(10.0);
        let name_c = if playing { s.primary } else { s.on_surface };
        let name = text(ui, &title, TypeRole::TitleSmall.style(), name_c, w, 2);
        let line = text(ui, &sub, TypeRole::BodySmall.style(), s.on_surface_variant, w, 1);
        let h = name.size().y + line.size().y;
        let y = cell.center().y - h / 2.0;
        ui.painter().galley(pos2(left, y), name.clone(), name_c);
        ui.painter().galley(pos2(left, y + name.size().y), line, s.on_surface_variant);
        if playing {
            let slot = Rect::from_center_size(pos2(cell.right() - 16.0 - 16.0, cell.center().y), vec2(32.0, 32.0));
            crate::components::Equalizer::new(id.with("eq"), s.primary).size(20.0).playing(true).paint_at(ui, slot);
        }
        let slot = Rect::from_center_size(cover.center(), vec2(40.0, 40.0));
        grow_button(ui, id.with("play"), hover, slot, play_button(false, Size::S, &format!("Слушать «{title}»"), id.with("play-btn")))
            .is_some_and(|r| r.clicked())
    });
    let menu_route = match sc {
        Shortcut::Release(a) => super::widgets::release_menu(&resp, a, out),
        Shortcut::Artist(a) => {
            super::widgets::artist_menu(&resp, a, out);
            None
        }
        Shortcut::Playlist(p, _) => {
            super::widgets::playlist_menu_at(&resp, p, None, out);
            None
        }
    };
    if clicked_play {
        out.push(play);
        return None;
    }
    if resp.clicked() {
        return Some(route);
    }
    menu_route
}

// ---- Mix.

/// The rows `t` takes in `role` at `width`.
fn rows_at(ui: &Ui, t: &str, role: TypeStyle, width: f32) -> usize {
    let mut job = egui::text::LayoutJob::single_section(t.to_owned(), role.text_format(Color32::WHITE));
    job.wrap.max_width = width;
    crate::typography::layout(ui.painter(), job).rows.len()
}

/// A mix of «Миксы для тебя» (§3.3, §4): the cover of its leading artist's
/// album cut to a Material shape of the mix's own on a tonal tile, its name
/// and its artists in a line each under it, as «Любимые» has them. A click
/// opens its page (`opens_list`; of the world `world`'s page for its
/// mixes) or plays it; under the pointer ⋮ at the top and ▶ at the bottom
/// grow in and the cover comes a little closer; while it plays the name is
/// primary and ⏸ stays in a square. With `can_blend` ⋮ offers «Смешать
/// с…» (the sheet of the other profiles).
#[allow(clippy::too_many_arguments)]
pub fn mix(
    ui: &mut Ui,
    cell: Rect,
    l: &LaneCard,
    with_new: bool,
    can_blend: bool,
    playing: bool,
    status: Status,
    opens_list: bool,
    world: Option<&str>,
    out: &mut Vec<Command>,
) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let theme = Theme::get(ui.ctx());
    let id = ui.id().with(("home-mix", l.key));
    let cell = on_pixels(ui, cell);
    let art = Rect::from_min_size(cell.min, vec2(cell.width(), cell.width()));
    let resp = ui.interact(cell, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let menu_open = ui.ctx().data(|d| d.get_temp::<bool>(id.with("more-open"))).unwrap_or(false);
    let (squish, layer, hover) = feedback(ui, id, &resp, 0.96);
    let hover = hover || menu_open;
    let zoom = 1.0 + 0.06 * anim::fraction(ui.ctx(), id.with("zoom"), f32::from(u8::from(hover)), theme.motion.default_effects);
    let cover: Option<String> = l.covers.first().map(|c| c.uri());
    let r = shape::LARGE;
    let (cut, tile) = mix_look(l, &s);
    // A blend's name too long for its line says a shorter form of it, the
    // whole in the name's tooltip.
    let title_role = TypeRole::TitleMedium.style();
    let title = std::iter::once(&l.title)
        .chain(&l.short_titles)
        .find(|t| rows_at(ui, t, title_role, cell.width()) <= 1)
        .or(l.short_titles.last())
        .unwrap_or(&l.title);
    let mut play = false;
    let mut more: Option<Response> = None;
    scaled(ui, art.center(), squish, |ui| {
        paint::container(ui.painter(), art, Radii::all(r), tile, egui::Stroke::NONE);
        let inner = Rect::from_center_size(art.center(), art.size() * 0.82 * zoom);
        crate::components::paint_shape_avatar(ui, inner, cover.as_deref(), "", cut, s.surface_container_lowest, s.on_surface);
        paint::state_layer(ui.painter(), art, Radii::all(r), s.on_surface, layer);
        let d = Size::M.height();
        let slot = Rect::from_min_size(art.max - vec2(8.0 + d, 8.0 + d), vec2(d, d));
        let tip = if playing { "Пауза".to_owned() } else { format!("Слушать «{}»", l.title) };
        if let Some(b) = grow_button(ui, id.with("play"), hover || playing, slot, play_button(playing, Size::M, &tip, id.with("play-btn"))) {
            play = b.clicked();
        }
        let m = Size::S.height();
        let slot = Rect::from_min_size(pos2(art.right() - 8.0 - m, art.top() + 8.0), vec2(m, m));
        more = more_button(ui, id, hover, slot);
    });
    let title_c = if playing { s.primary } else { s.on_surface };
    let name = text(ui, title, title_role, title_c, cell.width(), 1);
    let name_rect = Rect::from_min_size(pos2(cell.left(), art.bottom() + 8.0), vec2(cell.width(), TITLE_LINE));
    ui.painter().galley(name_rect.min, name, title_c);
    // Over the name: the whole of it when it is shortened; elsewhere why
    // the mix is offered.
    if *title != l.title && resp.hover_pos().is_some_and(|p| name_rect.contains(p)) {
        resp.clone().md3_tooltip(&l.title, None);
    } else if let Some(why) = &l.why {
        resp.clone().md3_tooltip(why, None);
    }
    let role = TypeRole::BodyMedium.style();
    let fits = |t: &str| rows_at(ui, t, role, cell.width()) <= 1;
    let new: &[String] = if with_new { &l.new_artists } else { &[] };
    let line = names_line(&fits, &l.artists, new);
    let g = text(ui, &line, role, s.on_surface_variant, cell.width(), 1);
    ui.painter().galley(pos2(cell.left(), name_rect.bottom()), g, s.on_surface_variant);

    let items = mix_items(l, can_blend, !opens_list, false);
    let mut picked = mix_menu(id.with("ctx"), l, &items).context_menu(&resp);
    if let Some(m) = &more {
        picked = picked.or(mix_menu(id.with("more-menu"), l, &items).align_right(true).attach(m));
    }
    let open = Menu::item_rects(ui.ctx(), id.with("more-menu")).is_some();
    ui.ctx().data_mut(|d| d.insert_temp(id.with("more-open"), open));
    let mut nav = picked.and_then(|k| mix_do(ui.ctx(), l, items[k], world, out));
    let card = resp.clicked() && more.as_ref().is_none_or(|m| !m.clicked());
    if card && opens_list && !play {
        nav = Some(mix_route(l, world));
    } else if play || card {
        if playing && status == Status::Playing {
            out.push(Command::Pause);
        } else {
            out.push(Command::PlayMix { id: l.id() });
        }
    }
    nav
}

/// The shape a mix's cover is cut to and its tile's colour: its own by
/// its key, the same every day.
fn mix_look(l: &LaneCard, s: &crate::ColorScheme) -> (nikgob_shapes::MaterialShape, Color32) {
    use nikgob_shapes::MaterialShape as M;
    use std::hash::{Hash, Hasher};
    const CUTS: [M; 9] = [M::Cookie9Sided, M::Clover4Leaf, M::Flower, M::Cookie6Sided, M::SoftBurst, M::Puffy, M::Cookie12Sided, M::Sunny, M::Clover8Leaf];
    let mut h = std::collections::hash_map::DefaultHasher::new();
    l.key.hash(&mut h);
    let n = h.finish();
    let tile = [s.secondary_container, s.tertiary_container, s.surface_container_highest][(n / 16 % 3) as usize];
    (CUTS[(n % CUTS.len() as u64) as usize], tile)
}

/// «Любимые» in «Миксы для тебя»: the playlist of every liked song itself,
/// a heart on a container of the scheme, its name and its length under it.
/// A click opens the playlist, ▶ plays it; under the pointer ⋮ at the top
/// grows in as on a mix (the playlist's own actions, the right click too);
/// while it plays the name is primary and ⏸ stays in its square.
pub fn liked_playlist(ui: &mut Ui, cell: Rect, p: &PlaylistCard, playing: bool, status: Status, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    let id = ui.id().with("home-liked-playlist");
    let cell = on_pixels(ui, cell);
    let art = Rect::from_min_size(cell.min, vec2(cell.width(), cell.width()));
    let resp = ui.interact(cell, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let menu_open = ui.ctx().data(|d| d.get_temp::<bool>(id.with("more-open"))).unwrap_or(false);
    let (squish, layer, hover) = feedback(ui, id, &resp, 0.96);
    let hover = hover || menu_open;
    let r = shape::LARGE;
    let mut more: Option<Response> = None;
    let play = scaled(ui, art.center(), squish, |ui| {
        paint::container(ui.painter(), art, Radii::all(r), s.primary_container, egui::Stroke::NONE);
        let size = (art.width() * 0.4).clamp(24.0, 96.0);
        Icon::Favorite.widget(true, s.on_primary_container, size).paint_at(ui, Rect::from_center_size(art.center(), vec2(size, size)));
        paint::state_layer(ui.painter(), art, Radii::all(r), s.on_surface, layer);
        let d = Size::M.height();
        let slot = Rect::from_min_size(art.max - vec2(8.0 + d, 8.0 + d), vec2(d, d));
        let tip = if playing { "Пауза".to_owned() } else { format!("Слушать «{}»", p.name) };
        let clicked = grow_button(ui, id.with("play"), hover || playing, slot, play_button(playing, Size::M, &tip, id.with("play-btn")))
            .is_some_and(|r| r.clicked());
        let m = Size::S.height();
        let slot = Rect::from_min_size(pos2(art.right() - 8.0 - m, art.top() + 8.0), vec2(m, m));
        more = more_button(ui, id, hover, slot);
        clicked
    });
    let name_c = if playing { s.primary } else { s.on_surface };
    let name = text(ui, &p.name, TypeRole::TitleMedium.style(), name_c, cell.width(), 1);
    ui.painter().galley(pos2(cell.left(), art.bottom() + 8.0), name, name_c);
    let n = u64::from(p.track_count);
    let count = format!("{n} {}", crate::screens::common::plural(n, "трек", "трека", "треков"));
    let line = text(ui, &count, TypeRole::BodyMedium.style(), s.on_surface_variant, cell.width(), 1);
    ui.painter().galley(pos2(cell.left(), art.bottom() + 8.0 + TITLE_LINE), line, s.on_surface_variant);
    // ⋮ and the right click: what a playlist's card offers.
    let rows = || super::widgets::playlist_rows(super::widgets::PlaylistMenuOf::default());
    let (menu, acts) = super::widgets::menu_of(id.with("ctx"), rows());
    let mut picked = menu.context_menu(&resp).and_then(|i| acts.get(i).copied());
    if let Some(m) = &more {
        let (menu, acts) = super::widgets::menu_of(id.with("more-menu"), rows());
        picked = picked.or(menu.align_right(true).attach(m).and_then(|i| acts.get(i).copied()));
    }
    let open = Menu::item_rects(ui.ctx(), id.with("more-menu")).is_some();
    ui.ctx().data_mut(|d| d.insert_temp(id.with("more-open"), open));
    if let Some(act) = picked {
        out.extend(super::widgets::playlist_command(act, p.id));
        return None;
    }
    if play {
        if playing && status == Status::Playing {
            out.push(Command::Pause);
        } else {
            out.push(Command::PlayPlaylist { id: p.id, start: None, shuffle: false });
        }
        return None;
    }
    let card = resp.clicked() && more.as_ref().is_none_or(|m| !m.clicked());
    card.then_some(Route::Playlist(p.id))
}

/// The page of mix `l` (of the world `world`'s page, if any).
pub fn mix_route(l: &LaneCard, world: Option<&str>) -> Route {
    Route::Mix { key: l.key, world: world.map(str::to_owned) }
}

/// An item of a mix's ⋮.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixItem {
    List,
    Play,
    Shuffle,
    PlayNext,
    Enqueue,
    Save,
    /// «Скачать недостающие»: its songs without a file to «Хочу скачать».
    Download,
    Redraw,
    Blend,
    Hide,
}

/// The items of mix `l`'s ⋮ in the order of the other collections' menus:
/// «Список треков» first with `list`, then play, shuffle, next, the queue
/// │ «Сохранить как плейлист» │ what it lacks │ its own («Другой состав»,
/// «Смешать с…») │ «Не предлагать этот микс». On its page (`on_page`)
/// «Слушать» and «Перемешать» are buttons and are left out. «На повторе»
/// is what played: no other draw of it, and its switch is in Settings; a
/// blend is the user's own for the day; a world's mix is of its page, the
/// world's own; a mix of the day is another one tomorrow (its row is
/// switched off in «Разделы главной»). With `can_blend` it offers «Смешать
/// с…».
pub fn mix_items(l: &LaneCard, can_blend: bool, list: bool, on_page: bool) -> Vec<MixItem> {
    let world = matches!(l.key, nikgob_view::MixKey::World(_));
    let redraw = l.key != nikgob_view::MixKey::OnRepeat && !world;
    let blend = can_blend && l.blend_base.is_some();
    let hide = !matches!(l.key, nikgob_view::MixKey::OnRepeat | nikgob_view::MixKey::Blend(_) | nikgob_view::MixKey::Daily(_)) && !world;
    let mut items = Vec::new();
    if list {
        items.push(MixItem::List);
    }
    if !on_page {
        items.extend([MixItem::Play, MixItem::Shuffle]);
    }
    items.extend([MixItem::PlayNext, MixItem::Enqueue, MixItem::Save]);
    if l.tracks.iter().any(|t| !t.has_local) {
        items.push(MixItem::Download);
    }
    if redraw {
        items.push(MixItem::Redraw);
    }
    if blend {
        items.push(MixItem::Blend);
    }
    if hide {
        items.push(MixItem::Hide);
    }
    items
}

/// The group of a mix's item: a divider stands between two groups.
fn mix_group(item: MixItem) -> u8 {
    match item {
        MixItem::List | MixItem::Play | MixItem::Shuffle | MixItem::PlayNext | MixItem::Enqueue => 0,
        MixItem::Save => 1,
        MixItem::Download => 2,
        MixItem::Redraw | MixItem::Blend => 3,
        MixItem::Hide => 4,
    }
}

/// The menu of `items` (of [`mix_items`]).
pub fn mix_menu(key: Id, l: &LaneCard, items: &[MixItem]) -> Menu {
    let mut m = Menu::new(key);
    for (k, item) in items.iter().enumerate() {
        if k > 0 && mix_group(items[k - 1]) != mix_group(*item) {
            m = m.divider();
        }
        m = m.item(match item {
            MixItem::List => MenuItem::new("Список треков").icon(Icon::QueueMusic).shortcut(l.tracks.len().to_string()),
            MixItem::Play => MenuItem::new("Слушать").icon(Icon::PlayArrow),
            MixItem::Shuffle => MenuItem::new("Перемешать").icon(Icon::Shuffle),
            MixItem::PlayNext => MenuItem::new(super::PLAY_NEXT).icon(Icon::QueueMusic),
            MixItem::Enqueue => MenuItem::new(super::TO_QUEUE).icon(Icon::QueueMusic),
            MixItem::Save => MenuItem::new("Сохранить как плейлист").icon(Icon::PlaylistAdd),
            MixItem::Download => MenuItem::new("Скачать недостающие").icon(Icon::Download),
            MixItem::Redraw => MenuItem::new("Другой состав").icon(Icon::Refresh),
            MixItem::Blend => MenuItem::new("Смешать с…").icon(Icon::Add),
            MixItem::Hide => MenuItem::new("Не предлагать этот микс").icon(Icon::Block),
        });
    }
    m
}

/// Does what `item` of mix `l`'s ⋮ says; «Список треков» opens its page.
pub fn mix_do(ctx: &egui::Context, l: &LaneCard, item: MixItem, world: Option<&str>, out: &mut Vec<Command>) -> Option<Route> {
    let tracks = || l.tracks.iter().map(|t| t.id).collect::<Vec<nikgob_view::TrackId>>();
    match item {
        MixItem::List => return Some(mix_route(l, world)),
        MixItem::Play => out.push(Command::PlayMix { id: l.id() }),
        MixItem::Shuffle => {
            let mut t = tracks();
            shuffle(&mut t);
            out.push(Command::PlayTracks { tracks: t, start: 0, context: l.context() });
        }
        MixItem::PlayNext => out.push(Command::PlayNext(tracks())),
        MixItem::Enqueue => out.push(Command::Enqueue(tracks())),
        MixItem::Save => out.push(Command::SaveTracksAsPlaylist { name: l.title.clone(), tracks: tracks() }),
        MixItem::Download => out.push(Command::DownloadTracks(l.tracks.iter().filter(|t| !t.has_local).map(|t| t.id).collect())),
        MixItem::Redraw => out.push(Command::RefreshMix { id: l.id() }),
        MixItem::Blend => {
            if let Some(base) = l.blend_base {
                super::home::open_blend(ctx, base);
            }
        }
        MixItem::Hide => out.push(Command::SetPref(nikgob_view::PrefPatch::HideMix(nikgob_view::HiddenMix { key: l.key, title: l.title.clone() }))),
    }
    None
}

// ---- Song row.

pub struct RowOutput {
    pub response: Response,
    pub clicked: bool,
    /// ♡ was clicked.
    pub liked: bool,
}

/// A song row (§3.4, §3.7): 72 high, reaching 8 into the margins so the
/// covers stand on the grid; the cover 56 r8, the title (primary while it
/// plays) and the artist; the hover layer r16. With `liked`, ♡ stays at
/// the right.
#[allow(clippy::too_many_arguments)]
pub fn song_row(
    ui: &mut Ui,
    cell: Rect,
    title: &str,
    artist: &str,
    cover: Option<String>,
    playing: bool,
    liked: Option<bool>,
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> RowOutput {
    let s = Theme::get(ui.ctx()).scheme;
    let id = ui.id().with(("home-row", salt));
    let row = cell.expand2(vec2(8.0, 0.0));
    let resp = ui.interact(row, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let (squish, layer, _) = feedback(ui, id, &resp, 0.98);
    let mut like = false;
    scaled(ui, row.center(), squish, |ui| {
        paint::state_layer(ui.painter(), row, Radii::all(shape::LARGE), s.on_surface, layer);
        let c = Rect::from_min_size(pos2(cell.left(), cell.top() + 8.0), vec2(56.0, 56.0));
        cover_in(ui, on_pixels(ui, c), cover.as_deref(), Radii::all(8.0), Icon::MusicNote, 1.0);
        let left = c.right() + 16.0;
        let trail = if liked.is_some() { 56.0 } else { 8.0 };
        let w = (row.right() - trail - left).max(10.0);
        let tc = if playing { s.primary } else { s.on_surface };
        let t = text(ui, title, TypeRole::BodyLarge.style(), tc, w, 1);
        let a = text(ui, artist, TypeRole::BodyMedium.style(), s.on_surface_variant, w, 1);
        let y = cell.center().y - (t.size().y + a.size().y) / 2.0;
        let th = t.size().y;
        ui.painter().galley(pos2(left, y), t, tc);
        ui.painter().galley(pos2(left, y + th), a, s.on_surface_variant);
        if let Some(on) = liked {
            let slot = Rect::from_center_size(pos2(row.right() - 8.0 - 20.0, cell.center().y), vec2(40.0, 40.0));
            let b = IconButton::new(Icon::Favorite)
                .size(Size::S)
                .style(Style::Standard)
                .filled_icon(on)
                .selected(on)
                .tooltip(super::like_label(on))
                .id_salt(id.with("like"));
            like = ui.put(slot, b).clicked();
        }
    });
    RowOutput { clicked: resp.clicked() && !like, liked: like, response: resp }
}

// ---- Tile of the day.

/// A tile of «Подборки дня» (§3.5): 112 high on surface_container, the
/// cover flush with the edge (an artist: a circle 88 inset 12), the kicker,
/// the title in up to two lines and the length. A click plays; ▶ 40 grows
/// in over the cover, ⋮ at the top right; ⋮ and the right click:
/// [`of_day_rows`].
pub fn of_day(ui: &mut Ui, cell: Rect, m: &MixCard, photo: Option<&str>, i: usize, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let id = ui.id().with(("home-of-day", i));
    let cell = on_pixels(ui, cell);
    let resp = ui.interact(cell, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let menu_open = ui.ctx().data(|d| d.get_temp::<bool>(id.with("more-open"))).unwrap_or(false);
    let (squish, layer, hover) = feedback(ui, id, &resp, 0.98);
    let hover = hover || menu_open;
    let r = shape::LARGE;
    let cover = Rect::from_min_size(cell.min, vec2(cell.height(), cell.height()));
    let first = m.covers.first().map(|c| c.uri());
    let mut more = None;
    let play = scaled(ui, cell.center(), squish, |ui| {
        paint::container(ui.painter(), cell, Radii::all(r), s.surface_container, egui::Stroke::NONE);
        if let Some(p) = photo {
            let circle = cover.shrink(12.0);
            cover_in(ui, circle, Some(p), Radii::all(circle.width() / 2.0), Icon::Person, 1.0);
        } else {
            cover_in(ui, cover, first.as_deref(), Radii::sides(r, 0.0), Icon::QueueMusic, 1.0);
        }
        paint::state_layer(ui.painter(), cell, Radii::all(r), s.on_surface, layer);
        let left = cover.right() + 16.0;
        let w = (cell.right() - 16.0 - left).max(10.0);
        let k = text(ui, &m.kicker, TypeRole::LabelMedium.style(), s.on_surface_variant, w, 1);
        let t = text(ui, &m.title, TypeRole::TitleMedium.style(), s.on_surface, w, 2);
        let d = text(ui, &m.subtitle, TypeRole::BodyMedium.style(), s.on_surface_variant, w, 1);
        let h = k.size().y + 4.0 + t.size().y + d.size().y;
        let mut y = cell.center().y - h / 2.0;
        let (kh, tth) = (k.size().y, t.size().y);
        ui.painter().galley(pos2(left, y), k, s.on_surface_variant);
        y += kh + 4.0;
        ui.painter().galley(pos2(left, y), t, s.on_surface);
        y += tth;
        ui.painter().galley(pos2(left, y), d, s.on_surface_variant);
        let slot = Rect::from_center_size(cover.center(), vec2(40.0, 40.0));
        let clicked = grow_button(ui, id.with("play"), hover, slot, play_button(false, Size::S, &format!("Слушать «{}»", m.title), id.with("play-btn")))
            .is_some_and(|r| r.clicked());
        let d = Size::S.height();
        more = more_button(ui, id, hover, Rect::from_min_size(pos2(cell.right() - 8.0 - d, cell.top() + 8.0), vec2(d, d)));
        clicked
    });
    keep_more_open(ui.ctx(), id, more.as_ref());
    let mut picked = None;
    if let Some(b) = &more {
        picked = of_day_menu(id.with("more-menu")).align_right(true).attach(b);
    }
    if !Menu::context_idle(&resp, id.with("ctx")) {
        picked = picked.or(of_day_menu(id.with("ctx")).context_menu(&resp));
    }
    let more_clicked = more.as_ref().is_some_and(Response::clicked);
    match picked.and_then(|k| of_day_rows().get(k).copied()) {
        Some(OfDayItem::Play) => out.push(Command::PlayMix { id: m.id }),
        Some(OfDayItem::Download) => out.push(Command::DownloadMix { id: m.id }),
        None if play || (resp.clicked() && !more_clicked) => out.push(Command::PlayMix { id: m.id }),
        None => {}
    }
}

/// An item of a tile of the day's menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfDayItem {
    Play,
    /// «Скачать недостающие»: its songs without a file to «Хочу скачать».
    Download,
}

/// The menu of a tile of the day: what its click does, and what it lacks
/// (the runtime knows which of its songs those are).
pub fn of_day_rows() -> [OfDayItem; 2] {
    [OfDayItem::Play, OfDayItem::Download]
}

fn of_day_menu(key: Id) -> Menu {
    Menu::new(key)
        .item(MenuItem::new("Слушать").icon(Icon::PlayArrow))
        .divider()
        .item(MenuItem::new("Скачать недостающие").icon(Icon::Download))
}

// ---- Album.

pub struct AlbumOutput {
    pub response: Response,
    pub clicked: bool,
    pub play: bool,
    /// Its ⋮ under the pointer, for the album's menu (the same as its
    /// right click).
    pub more: Option<Response>,
}

/// An album card (§3.6, §3.8): the cover r16 without a container, the
/// title in two lines and «Artist · tail» where the tail («слушал 5 из 52»,
/// «8 окт.») is never cut. A click opens it; ▶ 56 on the cover plays it;
/// ⋮ grows in at its top right.
pub fn album(ui: &mut Ui, cell: Rect, title: &str, artist: &str, tail: &str, cover: Option<String>, salt: impl std::hash::Hash + std::fmt::Debug) -> AlbumOutput {
    let s = Theme::get(ui.ctx()).scheme;
    let id = ui.id().with(("home-album", salt));
    let cell = on_pixels(ui, cell);
    let art = Rect::from_min_size(cell.min, vec2(cell.width(), cell.width()));
    let resp = ui.interact(cell, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let menu_open = ui.ctx().data(|d| d.get_temp::<bool>(id.with("more-open"))).unwrap_or(false);
    let (squish, layer, hover) = feedback(ui, id, &resp, 0.96);
    let hover = hover || menu_open;
    let r = shape::LARGE;
    let mut more = None;
    let play = scaled(ui, art.center(), squish, |ui| {
        cover_in(ui, art, cover.as_deref(), Radii::all(r), Icon::Album, 1.0);
        paint::state_layer(ui.painter(), art, Radii::all(r), s.on_surface, layer);
        let d = Size::M.height();
        let slot = Rect::from_min_size(art.max - vec2(8.0 + d, 8.0 + d), vec2(d, d));
        let clicked = grow_button(ui, id.with("play"), hover, slot, play_button(false, Size::M, &format!("Слушать «{title}»"), id.with("play-btn")))
            .is_some_and(|r| r.clicked());
        let m = Size::S.height();
        more = more_button(ui, id, hover, Rect::from_min_size(pos2(art.right() - 8.0 - m, art.top() + 8.0), vec2(m, m)));
        clicked
    });
    keep_more_open(ui.ctx(), id, more.as_ref());
    let w = cell.width();
    let t = text(ui, title, TypeRole::TitleSmall.style(), s.on_surface, w, 2);
    // The line under the title follows it: right under a one-line title.
    let th = t.size().y.clamp(20.0, 40.0);
    ui.painter().galley(pos2(cell.left(), art.bottom() + 8.0), t, s.on_surface);
    let role = TypeRole::BodySmall.style();
    let y = art.bottom() + 8.0 + th;
    if tail.is_empty() {
        let a = text(ui, artist, role, s.on_surface_variant, w, 1);
        ui.painter().galley(pos2(cell.left(), y), a, s.on_surface_variant);
    } else {
        let tail = format!(" · {tail}");
        let tw = line_w(ui, &tail, role);
        let a = text(ui, artist, role, s.on_surface_variant, (w - tw).max(24.0), 1);
        let aw = a.size().x;
        ui.painter().galley(pos2(cell.left(), y), a, s.on_surface_variant);
        let tg = text(ui, &tail, role, s.on_surface_variant, tw + 1.0, 1);
        ui.painter().galley(pos2(cell.left() + aw, y), tg, s.on_surface_variant);
    }
    let more_clicked = more.as_ref().is_some_and(Response::clicked);
    AlbumOutput { clicked: resp.clicked() && !play && !more_clicked, play, response: resp, more }
}

/// Shuffles `items` in place (a xorshift seeded by the clock: a menu's
/// «Перемешать» needs no more).
pub fn shuffle<T>(items: &mut [T]) {
    let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9e37_79b9, |d| d.as_nanos() as u64) | 1;
    for i in (1..items.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        items.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mix's ⋮ is in the order of the other collections' menus:
    /// «Слушать» first on the card, «В очередь», «Скачать недостающие»
    /// while a song has no file; on its page without its buttons.
    #[test]
    fn a_mix_menu_is_in_the_order_of_the_other_menus() {
        let song = |has_local: bool| nikgob_view::TrackRow {
            id: nikgob_view::TrackId::new(), title: "Song".into(), artist: "Artist".into(),
            album: None, release_id: None, artists: Default::default(), disc_no: 1, track_no: 1,
            duration: None, quality: None, source: None, has_local, liked: false, cover: None, clip: false,
        };
        let l = LaneCard {
            key: nikgob_view::MixKey::OnRepeat,
            title: "Mix".into(),
            short_titles: Vec::new(),
            artists: Vec::new(),
            new_artists: Vec::new(),
            covers: Vec::new(),
            new_cover: None,
            tracks: vec![song(true), song(false)],
            blend_base: None,
            queue: None,
            why: None,
        };
        let card = mix_items(&l, false, false, false);
        assert_eq!(card, [MixItem::Play, MixItem::Shuffle, MixItem::PlayNext, MixItem::Enqueue, MixItem::Save, MixItem::Download]);
        let page = mix_items(&l, false, false, true);
        assert_eq!(page, [MixItem::PlayNext, MixItem::Enqueue, MixItem::Save, MixItem::Download]);
        let all_here = LaneCard { tracks: vec![song(true)], ..l };
        assert!(!mix_items(&all_here, false, true, false).contains(&MixItem::Download));
        assert_eq!(mix_items(&all_here, false, true, false)[0], MixItem::List);
    }

    #[test]
    fn names_are_whole_and_a_long_one_is_skipped() {
        let names: Vec<String> = ["Radwimps", "Christopher Larkin", "OMORI", "Kevin Penkin"].iter().map(|s| (*s).to_owned()).collect();
        // Room for about 45 characters.
        let fits = |t: &str| t.chars().count() <= 45;
        assert_eq!(names_line(&fits, &names, &[]), "Radwimps, Christopher Larkin, OMORI и другие");
        let fits = |t: &str| t.chars().count() <= 30;
        // «Christopher Larkin» does not fit: the shorter ones after it do.
        assert_eq!(names_line(&fits, &names, &[]), "Radwimps, OMORI и другие");
        let two: Vec<String> = vec!["Eve".into(), "Ado".into()];
        assert_eq!(names_line(&|_| true, &two, &[]), "Eve и Ado");
        let new: Vec<String> = vec!["Пошлая Молли".into()];
        let mine: Vec<String> = vec!["Серега Пират".into(), "mzlff".into()];
        assert_eq!(names_line(&|t| t.chars().count() <= 60, &mine, &new), "Серега Пират, mzlff и новое: Пошлая Молли");
    }

    /// A new name too long for the line («Песни на японском» with «С
    /// новым») no longer takes the user's own names with it.
    #[test]
    fn a_long_new_name_leaves_the_own_names() {
        let mine: Vec<String> = ["Eve", "ammo", "大原ゆい子", "Ado"].iter().map(|s| (*s).to_owned()).collect();
        let new: Vec<String> = vec!["ヨルシカ / n-buna Official Channel and Friends".into()];
        let fits = |t: &str| t.chars().count() <= 40;
        assert_eq!(names_line(&fits, &mine, &new), "Eve, ammo, 大原ゆい子 и другие");
        // Not even one whole name: the first, for the line to cut.
        let long: Vec<String> = vec!["A name longer than any line could ever hold".into()];
        assert_eq!(names_line(&|t| t.chars().count() <= 10, &long, &[]), "A name longer than any line could ever hold");
    }
}

