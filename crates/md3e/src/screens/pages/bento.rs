//! The bento board of Главная (docs/stage4.md §12.1, §12.12): a column grid
//! where the daylist takes a 2×2 block and «Твой микс» and the smaller
//! mixes a cell each, with no holes whatever the count; and the live tile
//! the board and the moods are drawn with: each kind with its own art (a
//! cover in a shape, a fan of covers, a photo under a scrim, a big
//! typographic mark, an icon, the lottery's coin) over the kicker, the
//! title and a quiet line.

use std::f32::consts::PI;
use std::sync::Arc;

use egui::{
    Align2, Color32, CornerRadius, Galley, Id, Mesh, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, UiBuilder,
    emath::{Rot2, TSTransform},
    epaint::TextShape,
    pos2, vec2,
};
use nikgob_shapes::{Cubic, MaterialShape, paint as shape_paint};

use crate::{
    Icon, Spring, Theme, TypeRole, TypeStyle,
    components::{IconButton, Radii, Size, Style, TooltipExt, anim, button::Colors, paint},
    screens::now_playing::{pair_morph, shape_cubics},
    tokens::{shape, state},
};

/// The gap between the board's tiles.
pub const GAP: f32 = 12.0;

/// The board's columns for a content `width`: four when it is wide, two
/// otherwise.
pub fn columns(width: f32) -> usize {
    if width >= 960.0 { 4 } else { 2 }
}

/// The height of a board row for `cols` columns of `col_w`: about a
/// landscape cell, so the 2×2 daylist stays a square-ish block and a small
/// tile keeps room for its art over the text.
pub fn row_height(cols: usize, col_w: f32) -> f32 {
    if cols >= 4 {
        (col_w * 0.66).clamp(156.0, 180.0)
    } else {
        (col_w * 0.46).clamp(156.0, 176.0)
    }
}

/// Where one tile of the board goes, in grid cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub col: usize,
    pub row: usize,
    pub w: usize,
    pub h: usize,
}

/// `k` tiles sharing a row of `cols` columns: one each, the spare columns
/// going to the first ones.
fn row_widths(k: usize, cols: usize) -> impl Iterator<Item = usize> {
    (0..k).map(move |i| cols / k + usize::from(i < cols % k))
}

/// Places `n` tiles on `cols` columns, the first one big when `big`. The
/// grid ends up without holes: with few small tiles the big one widens
/// (three columns beside one or two tiles, two beside three or four), a
/// lone tile beside it grows to its height, and the tiles of a short last
/// row share its width. Two columns: the big tile spans both, the rest go
/// two per row.
pub fn layout(cols: usize, big: bool, n: usize) -> Vec<Cell> {
    let mut cells = Vec::with_capacity(n);
    let mut row = 0;
    if big && n > 0 {
        let k = n - 1;
        if cols < 4 {
            cells.push(Cell { col: 0, row: 0, w: cols, h: 2 });
            row = 2;
        } else if k == 0 {
            // Alone: one flat row across.
            cells.push(Cell { col: 0, row: 0, w: cols, h: 1 });
            row = 1;
        } else {
            // The block beside the big tile: one column (one or two tiles,
            // five or six) or two (three or four).
            let bw = if matches!(k, 3 | 4) { 2 } else { 1 };
            let left = cols - bw;
            cells.push(Cell { col: 0, row: 0, w: left, h: 2 });
            let beside = k.min(bw * 2);
            let top = beside.min(bw);
            // A single row beside it stretches to the big tile's height.
            let h = if beside == top { 2 } else { 1 };
            let mut col = left;
            for w in row_widths(top, bw) {
                cells.push(Cell { col, row: 0, w, h });
                col += w;
            }
            let mut col = left;
            for w in row_widths(beside - top, bw) {
                cells.push(Cell { col, row: 1, w, h: 1 });
                col += w;
            }
            row = 2;
        }
    }
    while cells.len() < n {
        let k = (n - cells.len()).min(cols);
        let mut col = 0;
        for w in row_widths(k, cols) {
            cells.push(Cell { col, row, w, h: 1 });
            col += w;
        }
        row += 1;
    }
    cells
}

/// The rows `cells` take.
pub fn rows(cells: &[Cell]) -> usize {
    cells.iter().map(|c| c.row + c.h).max().unwrap_or(0)
}

/// A cell's rectangle on a board at `origin` with columns `col_w` wide and
/// rows `row_h` tall.
pub fn cell_rect(c: Cell, origin: egui::Pos2, col_w: f32, row_h: f32) -> Rect {
    let min = origin + vec2(c.col as f32 * (col_w + GAP), c.row as f32 * (row_h + GAP));
    let size = vec2(c.w as f32 * col_w + (c.w - 1) as f32 * GAP, c.h as f32 * row_h + (c.h - 1) as f32 * GAP);
    Rect::from_min_size(min, size)
}

/// The delay between two tiles of the entrance, in seconds.
const STAGGER: f32 = 0.04;

/// The scale a tile enters from.
const ENTER_FROM: f32 = 0.92;

/// Seconds after which a tile's entrance has surely settled.
const ENTER_SECONDS: f32 = 1.0;

/// Tile `i`'s entrance `t` seconds after the board first showed: its
/// scale (from [`ENTER_FROM`] to 1 on the default spatial spring, a little
/// past it and back) and opacity (fast effects), each tile [`STAGGER`]
/// after the one before. Reduced motion: a fade only.
pub fn entrance_at(t: f32, i: usize, spatial: Spring, effects: Spring, reduced: bool) -> (f32, f32) {
    let t = t - i as f32 * STAGGER;
    if t >= ENTER_SECONDS {
        return (1.0, 1.0);
    }
    let from = if reduced { 1.0 } else { ENTER_FROM };
    if t <= 0.0 {
        return (from, 0.0);
    }
    let alpha = (1.0 - effects.solve(1.0, 0.0, t).0).clamp(0.0, 1.0);
    let scale = if reduced { 1.0 } else { 1.0 - (1.0 - ENTER_FROM) * spatial.solve(1.0, 0.0, t).0 };
    (scale, alpha)
}

/// The board's one-time entrance for tile `i` ([`entrance_at`]), counted
/// from the first frame the board was drawn and solved from the clock, so
/// it plays once per run, a window shown again lands where it would be,
/// and nothing repaints once it is over.
pub fn entrance(ui: &Ui, i: usize) -> (f32, f32) {
    let ctx = ui.ctx();
    let now = anim::now(ctx);
    let t0 = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(Id::new("home-bento-enter"), || now));
    let t = (now - t0) as f32;
    if t - i as f32 * STAGGER >= ENTER_SECONDS {
        return (1.0, 1.0);
    }
    ctx.request_repaint();
    let theme = Theme::get(ctx);
    entrance_at(t, i, theme.motion.default_spatial, theme.motion.fast_effects, theme.reduced_motion)
}

/// Draws `add` in a child of `ui` over `rect`, scaled up and faded in as
/// tile `i` of the entrance.
pub fn entering<R>(ui: &mut Ui, rect: Rect, i: usize, salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui, Rect) -> R) -> R {
    let (scale, alpha) = entrance(ui, i);
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(salt));
    if alpha < 1.0 {
        child.set_opacity(alpha);
    }
    scaled(&mut child, rect.center(), scale, |ui| add(ui, rect))
}

/// Draws `add`, then scales what it painted by `k` around `center`. Only
/// the picture moves: input keeps the layout's rects and the text keeps
/// its layout, so a tile growing or squished never rewraps.
pub fn scaled<R>(ui: &mut Ui, center: Pos2, k: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let layer = ui.layer_id();
    let start = ui.ctx().graphics(|g| g.get(layer).map_or(egui::layers::ShapeIdx(0), |l| l.next_idx()));
    let r = add(ui);
    if (k - 1.0).abs() > 1e-4 {
        let t = TSTransform::new(center.to_vec2() * (1.0 - k), k);
        ui.ctx().graphics_mut(|g| {
            let list = g.entry(layer);
            let end = list.next_idx();
            list.transform_range(start, end, t);
        });
    }
    r
}

/// What a tile shows beside its text.
#[derive(Clone, Copy, Debug)]
pub enum Art<'a> {
    /// A cover cut into a shape, turning into a sibling shape under the
    /// pointer (no cover yet: the shape in a tonal fill).
    Cover { uri: Option<&'a str>, shape: MaterialShape },
    /// Up to three covers fanned like cards, opening under the pointer.
    Fan(&'a [String]),
    /// A cover in a heart, beating under the pointer.
    Heart(Option<&'a str>),
    /// A photo (an artist, an album) filling the tile, the text on a scrim
    /// of the container colour.
    Bleed(&'a str),
    /// A big typographic mark: «7:00+», «あ».
    Glyph(&'a str),
    /// An icon in a filled shape, the shape turned by `turn` (radians).
    Icon { icon: Icon, shape: MaterialShape, turn: f32 },
    /// The lottery's coin, tossed on every click.
    Coin,
}

/// What a tile shows.
pub struct Tile<'a> {
    /// The container and content colours (scheme roles).
    pub fill: Color32,
    pub on: Color32,
    /// The quiet label over the title («ДЕЙЛИСТ»).
    pub kicker: Option<&'a str>,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub art: Art<'a>,
    /// The tooltip of ▶ (of the tile itself for the coin, which has no ▶).
    pub play: String,
}

/// What happened to a tile this frame.
pub struct TileOutput {
    pub response: Response,
    /// The tile or its ▶ was clicked: play.
    pub play: bool,
}

/// The press squish, the hover/press state layer and the hover of a
/// tile's response.
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

/// A text galley of `role` in `color`, wrapped to `width` in at most
/// `rows` rows (an ellipsis after the last).
fn text(ui: &Ui, s: &str, role: TypeStyle, color: Color32, width: f32, rows: usize) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::single_section(s.to_owned(), role.text_format(color));
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(1.0),
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// The width `s` takes on one line in `role`.
fn line_width(ui: &Ui, s: &str, role: TypeStyle) -> f32 {
    ui.painter().layout_no_wrap(s.to_owned(), role.font_id(), Color32::WHITE).size().x
}

/// A tile's title: title large on one line when it fits there; a longer
/// one («Underground River (opening)») in title medium over two lines when
/// `two` allows, else cut. Only the text and the width decide, never the
/// pointer.
fn title(ui: &Ui, s: &str, on: Color32, width: f32, two: bool) -> Arc<Galley> {
    let large = TypeRole::TitleLarge.style();
    if !two || line_width(ui, s, large) <= width {
        text(ui, s, large, on, width, 1)
    } else {
        text(ui, s, TypeRole::TitleMedium.style(), on, width, 2)
    }
}

/// Shows the kicker only when it says more than the title does («НА
/// ПОВТОРЕ» over «На повторе» says nothing).
pub fn kicker<'a>(kicker: Option<&'a str>, title: &str) -> Option<&'a str> {
    kicker.filter(|k| k.to_lowercase() != title.to_lowercase())
}

/// ▶ on a tile: its content colour as the container and its container
/// colour as the glyph, so it stands out on every role (primary on a
/// primary or tertiary container would not).
fn play_colors(fill: Color32, on: Color32) -> Colors {
    Colors { container: on, content: fill, border: Color32::TRANSPARENT }
}

/// ▶ growing in at `slot` on the fast spatial spring and fading in (fast
/// effects) while the tile is hovered; true when clicked.
#[allow(clippy::too_many_arguments)]
pub fn hover_play(ui: &mut Ui, id: Id, hover: bool, slot: Rect, size: Size, fill: Color32, on: Color32, tip: &str) -> bool {
    let theme = Theme::get(ui.ctx());
    let shown = f32::from(u8::from(hover));
    let k = anim::fraction(ui.ctx(), id.with("play"), shown, theme.motion.fast_effects).clamp(0.0, 1.0);
    if k <= 0.01 {
        return false;
    }
    let grow = anim::f32(ui.ctx(), id.with("play-grow"), shown, theme.motion.fast_spatial);
    let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt(id.with("play-slot")));
    child.set_opacity(k * ui.opacity());
    scaled(&mut child, slot.center(), 0.5 + 0.5 * grow.max(0.0), |ui| {
        ui.add_enabled(
            k > 0.5,
            IconButton::new(Icon::PlayArrow)
                .size(size)
                .style(Style::Filled)
                .filled_icon(true)
                .colors(play_colors(fill, on))
                .tooltip(tip)
                .id_salt(id.with("play-btn")),
        )
        .clicked()
    })
}

/// The shape a cover turns into under the pointer: a sibling of its own.
pub fn sibling(shape: MaterialShape) -> MaterialShape {
    use MaterialShape::*;
    match shape {
        Circle => Cookie12Sided,
        Cookie12Sided => SoftBurst,
        Cookie9Sided => Cookie12Sided,
        Cookie7Sided => Cookie9Sided,
        Cookie6Sided => Cookie7Sided,
        Cookie4Sided => Clover4Leaf,
        Clover4Leaf => Cookie4Sided,
        Clover8Leaf => Flower,
        Flower => Clover8Leaf,
        Sunny => VerySunny,
        VerySunny => Sunny,
        Arch => Bun,
        Bun => Pill,
        Pill => Oval,
        Oval => Pill,
        Ghostish => Puffy,
        Puffy => Cookie9Sided,
        Gem => Pentagon,
        Pentagon => Gem,
        Boom => SoftBoom,
        SoftBoom => Boom,
        Burst => SoftBurst,
        SoftBurst => Burst,
        Fan => Arch,
        SemiCircle => Arch,
        PuffyDiamond => Diamond,
        Diamond => PuffyDiamond,
        Arrow => Triangle,
        Triangle => PixelTriangle,
        PixelTriangle => Triangle,
        PixelCircle => Circle,
        Slanted => Square,
        Square => Slanted,
        ClamShell => Bun,
        Heart => Cookie9Sided,
    }
}

/// `uri` cut into `cubics` in `rect` and turned by `angle` (the picture
/// turns with its outline), cropped to cover the rect; without the
/// picture (none, or still loading) the outline in `empty`.
fn paint_cover(ui: &Ui, cubics: &[Cubic], rect: Rect, angle: f32, uri: Option<&str>, empty: Color32) {
    let opts = shape_paint::FillOptions::for_ppp(ui.ctx().pixels_per_point());
    let texture = uri.and_then(|u| crate::covers::texture(ui.ctx(), u, rect.size(), 1.0));
    let mut mesh = match texture {
        Some(t) => {
            let aspect = t.size.x.max(1.0) / t.size.y.max(1.0);
            let target = rect.width() / rect.height().max(1.0);
            let uv = if aspect > target {
                let f = target / aspect;
                Rect::from_min_max(pos2((1.0 - f) * 0.5, 0.0), pos2((1.0 + f) * 0.5, 1.0))
            } else {
                let f = aspect / target;
                Rect::from_min_max(pos2(0.0, (1.0 - f) * 0.5), pos2(1.0, (1.0 + f) * 0.5))
            };
            shape_paint::texture_mesh(cubics, rect, 0.0, t.id, uv, Color32::WHITE, &opts)
        }
        None => shape_paint::fill_mesh(cubics, rect, 0.0, empty, &opts),
    };
    if angle != 0.0 {
        mesh.rotate(Rot2::from_angle(angle), rect.center());
    }
    if !mesh.is_empty() {
        ui.painter().add(Shape::mesh(mesh));
    }
}

/// The type roles a glyph tries, largest first.
const GLYPH_ROLES: [TypeRole; 5] =
    [TypeRole::DisplayLarge, TypeRole::DisplayMedium, TypeRole::DisplaySmall, TypeRole::HeadlineLarge, TypeRole::HeadlineMedium];

/// A glyph in the largest display role that fits `h` × `max_w`.
fn glyph(ui: &Ui, s: &str, on: Color32, h: f32, max_w: f32) -> Arc<Galley> {
    for role in GLYPH_ROLES {
        let style = role.emphasized();
        if style.line_height <= h && line_width(ui, s, style) <= max_w {
            return text(ui, s, style, on, max_w, 1);
        }
    }
    text(ui, s, TypeRole::HeadlineMedium.emphasized(), on, max_w, 1)
}

/// Where a tile's parts go: computed from its rect and texts alone, so
/// hovering or pressing it never moves or rewraps anything.
struct Layout {
    art: Rect,
    glyph: Option<Arc<Galley>>,
    kicker: Option<(Pos2, Arc<Galley>)>,
    title: (Pos2, Arc<Galley>),
    sub: Option<(Pos2, Arc<Galley>)>,
    /// The top of the text block (a photo's scrim fades in above it).
    text_top: f32,
}

/// The padding inside a tile.
const PAD: f32 = 16.0;

/// The width the art takes at height `h`, at most `max_w`.
fn art_width(ui: &Ui, art: &Art<'_>, on: Color32, h: f32, max_w: f32) -> (f32, Option<Arc<Galley>>) {
    match art {
        Art::Fan(_) => ((h * 0.86 * 1.84).min(max_w), None),
        Art::Glyph(s) => {
            let g = glyph(ui, s, on, h, max_w);
            (g.size().x, Some(g))
        }
        _ => (h.min(max_w), None),
    }
}

/// Lays the kicker, title and quiet line bottom up from `bottom` at `x`;
/// returns their positions and the top of the block.
#[allow(clippy::type_complexity)]
fn stack(
    x: f32,
    bottom: f32,
    kicker: Option<Arc<Galley>>,
    title: Arc<Galley>,
    sub: Option<Arc<Galley>>,
) -> (Option<(Pos2, Arc<Galley>)>, (Pos2, Arc<Galley>), Option<(Pos2, Arc<Galley>)>, f32) {
    let mut y = bottom;
    let sub = sub.map(|g| {
        y -= g.size().y;
        let at = pos2(x, y);
        y -= 2.0;
        (at, g)
    });
    y -= title.size().y;
    let title = (pos2(x, y), title);
    let kicker = kicker.map(|g| {
        y -= g.size().y + 4.0;
        (pos2(x, y), g)
    });
    (kicker, title, sub, y)
}

/// Lays a small tile out in `card`. A photo fills it with the text at the
/// bottom; a wide tile (a short last row) holds the art at its right end,
/// the text beside it; a tall one the art over the text; the others the
/// art in the top right corner, the kicker beside its foot, the title and
/// the quiet line across the bottom.
fn layout_tile(ui: &Ui, card: Rect, t: &Tile<'_>) -> Layout {
    let inner = card.shrink(PAD);
    let on = t.on;
    let sub_of = |w: f32| (!t.subtitle.is_empty()).then(|| text(ui, t.subtitle, TypeRole::BodyMedium.style(), on, w, 1));
    let kicker_of = |w: f32, rows: usize| t.kicker.map(|k| text(ui, k, TypeRole::LabelMedium.style(), on, w, rows));
    let height = |g: &Option<Arc<Galley>>, gap: f32| g.as_ref().map_or(0.0, |g| g.size().y + gap);

    if matches!(t.art, Art::Bleed(_)) {
        let w = inner.width();
        let (kicker, title, sub, top) = stack(inner.left(), inner.bottom(), kicker_of(w, 2), title(ui, t.title, on, w, true), sub_of(w));
        return Layout { art: card, glyph: None, kicker, title, sub, text_top: top };
    }
    if card.width() > card.height() * 2.4 {
        let (art_w, glyph) = art_width(ui, &t.art, on, inner.height(), inner.width() * 0.45);
        let art = Rect::from_min_size(pos2(inner.right() - art_w, inner.top()), vec2(art_w, inner.height()));
        let w = inner.width() - art_w - 16.0;
        let (kicker, title, sub, top) = stack(inner.left(), inner.bottom(), kicker_of(w, 2), title(ui, t.title, on, w, true), sub_of(w));
        return Layout { art, glyph, kicker, title, sub, text_top: top };
    }

    let w = inner.width();
    let sub = sub_of(w);
    let below = |title: &Arc<Galley>| title.size().y + height(&sub, 2.0);
    let tall = inner.height() > inner.width();
    if tall {
        // The art over the text, as wide as the tile allows.
        let kicker = kicker_of(w, 2);
        let mut name = title(ui, t.title, on, w, true);
        let room = |name: &Arc<Galley>| inner.height() - below(name) - height(&kicker, 4.0) - 12.0;
        if room(&name) < 64.0 {
            name = title(ui, t.title, on, w, false);
        }
        let h = room(&name).min(w);
        let (art_w, glyph) = art_width(ui, &t.art, on, h, w);
        let art = Rect::from_min_size(pos2(inner.right() - art_w, inner.top()), vec2(art_w, h));
        let (kicker, title, sub, top) = stack(inner.left(), inner.bottom(), kicker, name, sub);
        return Layout { art, glyph, kicker, title, sub, text_top: top };
    }
    // Two title lines only while the art keeps a fair size.
    let mut name = title(ui, t.title, on, w, true);
    if inner.height() - below(&name) - 8.0 < 64.0 {
        name = title(ui, t.title, on, w, false);
    }
    let h = (inner.height() - below(&name) - 8.0).max(24.0);
    let (art_w, glyph) = art_width(ui, &t.art, on, h, w * 0.55);
    let art = Rect::from_min_size(pos2(inner.right() - art_w, inner.top()), vec2(art_w, h));
    // The kicker beside the art's foot, in two rows if it needs them and
    // there is height for them.
    let kicker_w = w - art_w - 12.0;
    let title_top = inner.bottom() - below(&name);
    let two = title_top - 4.0 - 2.0 * TypeRole::LabelMedium.style().line_height >= inner.top();
    let kicker = kicker_of(kicker_w, if two { 2 } else { 1 });
    let (kicker, title, sub, top) = stack(inner.left(), inner.bottom(), kicker, name, sub);
    Layout { art, glyph, kicker, title, sub, text_top: top }
}

/// A gradient of `fill` from clear at `from` to `peak` at `to`, then
/// `peak` down to the tile's foot with its lower corners: a scrim under
/// the text on a photo. `from` and `to` stay below the tile's upper
/// corners, so the straight band never spills past them.
fn scrim(ui: &Ui, card: Rect, from: f32, to: f32, fill: Color32, peak: f32) {
    let r = shape::EXTRA_LARGE;
    let from = from.max(card.top() + r);
    let to = to.clamp(from + 1.0, card.bottom() - r);
    let mut mesh = Mesh::default();
    const STEPS: u32 = 8;
    for k in 0..=STEPS {
        let f = k as f32 / STEPS as f32;
        let y = from + (to - from) * f;
        // Smoothstep: no visible edge where the gradient starts or ends.
        let a = peak * f * f * (3.0 - 2.0 * f);
        let c = fill.gamma_multiply(a);
        mesh.colored_vertex(pos2(card.left(), y), c);
        mesh.colored_vertex(pos2(card.right(), y), c);
        if k > 0 {
            let i = 2 * k;
            mesh.add_triangle(i - 2, i - 1, i);
            mesh.add_triangle(i - 1, i + 1, i);
        }
    }
    ui.painter().add(Shape::mesh(mesh));
    let r = r as u8;
    let foot = Rect::from_min_max(pos2(card.left(), to), card.max);
    ui.painter().rect_filled(foot, CornerRadius { nw: 0, ne: 0, sw: r, se: r }, fill.gamma_multiply(peak));
}

/// The coin's spring: a few quick half-turns that swing a little past the
/// last face and settle on it.
const TOSS: Spring = Spring::new(0.62, 90.0);
/// Half-turns of one toss: odd, so every toss lands on the other face.
const HALF_TURNS: f32 = 5.0;
/// Seconds of a toss (the spring has settled by then).
pub const TOSS_SECONDS: f32 = 1.0;
/// Seconds the coin is in the air.
const AIR: f32 = 0.5;

/// The coin `t` seconds into its `n`-th toss: its turn about the vertical
/// axis (radians; whole half-turns at rest) and how high it is (0..1).
/// Reduced motion: no flight, the face swaps at once.
pub fn coin_pose(n: u32, t: f32, reduced: bool) -> (f32, f32) {
    let rest = |k: u32| k as f32 * HALF_TURNS * PI;
    if n == 0 {
        return (0.0, 0.0);
    }
    if reduced || t >= TOSS_SECONDS {
        return (rest(n), 0.0);
    }
    let t = t.max(0.0);
    let p = 1.0 - TOSS.solve(1.0, -6.0, t).0;
    let lift = if t < AIR { (PI * t / AIR).sin() } else { 0.0 };
    (rest(n - 1) + HALF_TURNS * PI * p, lift)
}

/// When the coin of tile `id` was last tossed and how many times.
fn coin_state(ctx: &egui::Context, id: Id) -> (f64, u32) {
    ctx.data(|d| d.get_temp(id.with("coin"))).unwrap_or((0.0, 0))
}

/// Tosses the coin of tile `id` once more.
fn toss(ctx: &egui::Context, id: Id) {
    let (_, n) = coin_state(ctx, id);
    let now = anim::now(ctx);
    ctx.data_mut(|d| d.insert_temp(id.with("coin"), (now, n + 1)));
}

/// The lottery's coin in `rect`: a disc in the tile's content colour, «?»
/// on one face and the shuffle mark on the other, narrowing with the cosine
/// of its turn as it flips (the face fades out edge-on); it lifts a little
/// under the pointer. Repaints only while it flies.
fn paint_coin(ui: &Ui, rect: Rect, id: Id, hover: f32, fill: Color32, on: Color32) {
    let ctx = ui.ctx();
    let (start, n) = coin_state(ctx, id);
    let t = (anim::now(ctx) - start) as f32;
    let reduced = Theme::get(ctx).reduced_motion;
    let (turn, lift) = coin_pose(n, t, reduced);
    if n > 0 && t < TOSS_SECONDS && !reduced {
        ctx.request_repaint();
    }
    let r = rect.width().min(rect.height()) * 0.5 * (1.0 + 0.04 * hover);
    let c = rect.center() - vec2(0.0, lift * rect.height() * 0.24);
    let squash = turn.cos();
    let rx = (r * squash.abs()).max(1.5);
    let painter = ui.painter();
    painter.add(Shape::ellipse_filled(c, vec2(rx, r), on));
    painter.add(Shape::ellipse_stroke(c, vec2(rx * 0.84, r * 0.84), Stroke::new(2.0, fill.gamma_multiply(0.45))));
    let face = ((squash.abs() - 0.2) / 0.5).clamp(0.0, 1.0);
    if face <= 0.0 {
        return;
    }
    let mark = fill.gamma_multiply(face);
    if squash >= 0.0 {
        let g = glyph(ui, "?", mark, r * 1.3, r * 1.4);
        painter.galley(c - g.size() * 0.5, g, mark);
    } else {
        let s = r * 0.9;
        Icon::Shuffle.widget(true, mark, s).paint_at(ui, Rect::from_center_size(c, vec2(s * squash.abs(), s)));
    }
}

/// Up to three covers fanned like cards at the right of `art`, the first
/// in front; they open under the pointer.
fn paint_fan(ui: &Ui, art: Rect, covers: &[String], hover: f32, on: Color32) {
    let n = covers.len().clamp(1, 3);
    let d = (art.height() * 0.86).min(art.width());
    let step = ((art.width() - d) * 0.5).clamp(0.0, d * 0.42);
    let open = 1.0 + 0.6 * hover;
    let cubics = shape_cubics(MaterialShape::Square);
    for i in (0..n).rev() {
        let k = i as f32;
        let centre = pos2(art.right() - d * 0.5 - k * step, art.center().y);
        let angle = (0.06 - 0.11 * k) * open;
        let rect = Rect::from_center_size(centre, vec2(d, d));
        paint_cover(ui, cubics, rect, angle, covers.get(i).map(String::as_str), on.gamma_multiply(0.16 + 0.06 * k));
    }
}

/// Paints a tile's art (not the photo of [`Art::Bleed`], drawn under
/// everything) at `l.art`; `hover` is the pointer's spring (0 → 1, may
/// overshoot a little).
fn paint_art(ui: &Ui, t: &Tile<'_>, l: &Layout, id: Id, hover: f32) {
    let art = l.art;
    let square = Rect::from_min_size(pos2(art.right() - art.height().min(art.width()), art.top()), vec2(1.0, 1.0) * art.height().min(art.width()));
    let tonal = t.on.gamma_multiply(0.16);
    match t.art {
        Art::Cover { uri, shape } => {
            let cubics = pair_morph(shape, sibling(shape)).as_cubics(hover.clamp(0.0, 1.1));
            paint_cover(ui, &cubics, square, 0.12 * hover, uri, tonal);
        }
        Art::Heart(uri) => {
            let beat = Rect::from_center_size(square.center(), square.size() * (1.0 + 0.06 * hover));
            paint_cover(ui, shape_cubics(MaterialShape::Heart), beat, -0.1 * hover, uri, tonal);
        }
        Art::Fan(covers) => paint_fan(ui, art, covers, hover, t.on),
        Art::Glyph(_) => {
            if let Some(g) = &l.glyph {
                let at = pos2(art.right() - g.size().x, art.center().y - g.size().y * 0.5);
                let shape = TextShape::new(at, g.clone(), t.on).with_angle_and_anchor(-0.06 * hover, Align2::CENTER_CENTER);
                ui.painter().add(shape);
            }
        }
        Art::Icon { icon, shape, turn } => {
            let cubics = pair_morph(shape, sibling(shape)).as_cubics(hover.clamp(0.0, 1.1));
            paint_cover(ui, &cubics, square, turn + 0.12 * hover, None, t.on);
            let s = (square.width() * 0.42).round();
            icon.widget(true, t.fill, s).paint_at(ui, Rect::from_center_size(square.center(), vec2(s, s)));
        }
        Art::Coin => paint_coin(ui, square, id, hover, t.fill, t.on),
        Art::Bleed(_) => {}
    }
}

/// A tile filling `rect`: a filled container in its scheme colour, its art
/// ([`Art`]), the kicker, title and a quiet line; ▶ grows in under the
/// pointer (the coin has none: the tile tosses it). A click anywhere plays.
pub fn tile(ui: &mut Ui, rect: Rect, t: &Tile<'_>, id: Id) -> TileOutput {
    let mut response = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let coin = matches!(t.art, Art::Coin);
    if coin {
        response = response.md3_tooltip(t.play.as_str(), None);
        if response.clicked() {
            toss(ui.ctx(), id);
        }
    }
    let (squish, layer, hover) = feedback(ui, id, &response, 0.96);
    let theme = Theme::get(ui.ctx());
    let lean = anim::fraction(ui.ctx(), id.with("lean"), f32::from(u8::from(hover)), theme.motion.default_spatial);
    let lean = if theme.reduced_motion { 0.0 } else { lean };
    let l = layout_tile(ui, rect, t);
    let radii = Radii::all(shape::EXTRA_LARGE);
    let card = rect;
    let play = scaled(ui, card.center(), squish, |ui| {
        let painter = ui.painter().clone();
        paint::container(&painter, card, radii, t.fill, Stroke::NONE);
        if let Art::Bleed(uri) = t.art {
            let r = shape::EXTRA_LARGE as u8;
            crate::components::paint_image(ui, card, Some(uri), CornerRadius::same(r), Icon::Person);
            scrim(ui, card, l.text_top - 48.0, l.text_top + 8.0, t.fill, 0.92);
        } else {
            paint_art(ui, t, &l, id, lean);
        }
        if let Some((at, g)) = &l.kicker {
            painter.galley(*at, g.clone(), t.on);
        }
        painter.galley(l.title.0, l.title.1.clone(), t.on);
        if let Some((at, g)) = &l.sub {
            painter.galley(*at, g.clone(), t.on);
        }
        paint::state_layer(&painter, card, radii, t.on, layer);
        if coin {
            return false;
        }
        let (size, slot) = if matches!(t.art, Art::Bleed(_)) {
            let d = Size::M.height();
            (Size::M, Rect::from_min_size(pos2(card.right() - PAD - d, card.top() + PAD), vec2(d, d)))
        } else {
            let size = if l.art.height() >= 72.0 { Size::M } else { Size::S };
            let d = size.height();
            let c = Rect::from_min_size(pos2(l.art.right() - l.art.height().min(l.art.width()), l.art.top()), vec2(1.0, 1.0) * l.art.height().min(l.art.width())).center();
            let c = if matches!(t.art, Art::Fan(_) | Art::Glyph(_)) { pos2(l.art.right() - l.art.height() * 0.43, l.art.center().y) } else { c };
            (size, Rect::from_center_size(c, vec2(d, d)))
        };
        hover_play(ui, id, hover, slot, size, t.fill, t.on, &t.play)
    });
    let play = play || response.clicked();
    TileOutput { response, play }
}

/// The shapes of the big tile's collage, left to right.
const COLLAGE_SHAPES: [MaterialShape; 4] =
    [MaterialShape::Sunny, MaterialShape::Cookie9Sided, MaterialShape::Clover4Leaf, MaterialShape::Pill];

/// What the big tile shows.
pub struct BigTile<'a> {
    pub fill: Color32,
    pub on: Color32,
    pub kicker: Option<&'a str>,
    pub title: &'a str,
    pub subtitle: &'a str,
    /// Up to four cover uris for the collage.
    pub covers: &'a [String],
    /// The first cover's shape.
    pub shape: MaterialShape,
    /// The tooltip of ▶.
    pub play: String,
}

/// What happened to the big tile this frame.
pub struct BigOutput {
    pub response: Response,
    pub play: bool,
    /// The ⋮ button, for its menu.
    pub more: Option<Response>,
}

/// The board's big tile (the daylist, or «Твой микс» without one): the
/// largest type of the screen, a collage of covers in shapes, a big ▶
/// and ⋮. Tall: the collage at the top beside ⋮, then the kicker, title
/// and line with ▶ beside them. Flat (one row): the collage, the text and
/// ▶ side by side. Laid out from its rect alone; pressing squishes the
/// picture, never the layout.
pub fn big_tile(ui: &mut Ui, rect: Rect, t: &BigTile<'_>, id: Id, more: bool) -> BigOutput {
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let (squish, layer, hover) = feedback(ui, id, &response, 0.98);
    let radii = Radii::all(shape::EXTRA_LARGE);
    let card = rect;
    let theme = Theme::get(ui.ctx());
    let pad = 24.0;
    let flat = card.height() < 240.0 && card.width() > card.height() * 2.2;
    let lean = anim::fraction(ui.ctx(), id.with("lean"), f32::from(u8::from(hover)), theme.motion.default_spatial);
    let lean = if theme.reduced_motion { 0.0 } else { lean };

    let play_size = if flat || card.height() < 300.0 { Size::M } else { Size::L };
    let play_d = play_size.height();
    let more_d = Size::S.height();
    let covers = &t.covers[..t.covers.len().min(4)];

    let (art_zone, text_rect, play_rect) = if flat {
        let art_w = (card.width() * 0.34).min(360.0);
        let art = Rect::from_min_max(card.min + vec2(pad, pad), pos2(card.left() + pad + art_w, card.bottom() - pad));
        let text = Rect::from_min_max(pos2(art.right() + 32.0, card.top() + pad), pos2(card.right() - pad - play_d - 24.0, card.bottom() - pad));
        let play = Rect::from_min_size(pos2(card.right() - pad - play_d, card.center().y - play_d * 0.5), vec2(play_d, play_d));
        (art, text, play)
    } else {
        let play = Rect::from_min_size(card.right_bottom() - vec2(pad + play_d, pad + play_d), vec2(play_d, play_d));
        let text = Rect::from_min_max(pos2(card.left() + pad, card.top() + pad), pos2(play.left() - 16.0, card.bottom() - pad));
        // The collage sits under the kicker row and over the text, laid
        // out once the text's height is known.
        (Rect::NOTHING, text, play)
    };

    // Text, bottom-aligned in its rect.
    // The largest type of the screen: display medium when the tile is
    // big enough and the title fits a line there, else display small (two
    // lines at most when tall).
    let tw = text_rect.width();
    let sub = (!t.subtitle.is_empty()).then(|| text(ui, t.subtitle, TypeRole::BodyLarge.style(), t.on, tw, 2));
    let rows = if flat { 1 } else { 2 };
    let medium = (card.width() >= 440.0 && !flat)
        .then(|| text(ui, t.title, TypeRole::DisplayMedium.emphasized(), t.on, tw, rows))
        .filter(|g| g.rows.len() == 1);
    let title = medium.unwrap_or_else(|| text(ui, t.title, TypeRole::DisplaySmall.emphasized(), t.on, tw, rows));
    let kick = t.kicker.map(|k| text(ui, k, TypeRole::LabelLarge.style(), t.on, tw, 1));
    let sub_h = sub.as_ref().map_or(0.0, |g| g.size().y + 4.0);
    let (kick_pos, title_pos, sub_pos);
    if flat {
        let total = kick.as_ref().map_or(0.0, |g| g.size().y + 4.0) + title.size().y + sub_h;
        let mut y = text_rect.center().y - total * 0.5;
        kick_pos = pos2(text_rect.left(), y);
        y += kick.as_ref().map_or(0.0, |g| g.size().y + 4.0);
        title_pos = pos2(text_rect.left(), y);
        sub_pos = pos2(text_rect.left(), y + title.size().y + 4.0);
    } else {
        // Kicker, title and line at the bottom; the collage above them.
        let sub_y = text_rect.bottom() - sub_h + 4.0;
        title_pos = pos2(text_rect.left(), sub_y - 4.0 - title.size().y);
        sub_pos = pos2(text_rect.left(), sub_y);
        kick_pos = pos2(text_rect.left(), title_pos.y - 4.0 - kick.as_ref().map_or(0.0, |g| g.size().y));
    }

    let art_zone = if flat {
        art_zone
    } else {
        let bottom = if kick.is_some() { kick_pos.y } else { title_pos.y } - 16.0;
        Rect::from_min_max(card.min + vec2(pad, pad), pos2(card.right() - pad - more_d - 8.0, bottom))
    };

    let (play, more) = scaled(ui, card.center(), squish, |ui| {
        let painter = ui.painter().clone();
        paint::container(&painter, card, radii, t.fill, Stroke::NONE);
        collage(ui, art_zone, covers, t.shape, lean, t.on);
        if let Some(g) = kick {
            painter.galley(kick_pos, g, t.on);
        }
        painter.galley(title_pos, title, t.on);
        if let Some(g) = sub {
            painter.galley(sub_pos, g, t.on);
        }
        paint::state_layer(&painter, card, radii, t.on, layer);

        // ⋮ in the corner, ▶ always there (the board's one big action).
        let more = more.then(|| {
            let r = Rect::from_min_size(pos2(card.right() - pad + 8.0 - more_d, card.top() + pad - 8.0 + 4.0), vec2(more_d, more_d));
            let mut mui = ui.new_child(UiBuilder::new().max_rect(r).id_salt(id.with("more")));
            mui.add(
                IconButton::new(Icon::MoreVert)
                    .size(Size::S)
                    .colors(Colors { container: Color32::TRANSPARENT, content: t.on, border: Color32::TRANSPARENT })
                    .tooltip("Ещё")
                    .id_salt(id.with("more-btn")),
            )
        });
        let mut pui = ui.new_child(UiBuilder::new().max_rect(play_rect).id_salt(id.with("play")));
        let play = pui
            .add(
                IconButton::new(Icon::PlayArrow)
                    .size(play_size)
                    .style(Style::Filled)
                    .filled_icon(true)
                    .colors(play_colors(t.fill, t.on))
                    .tooltip(t.play.as_str())
                    .id_salt(id.with("play-btn")),
            )
            .clicked();
        (play, more)
    });
    let more_clicked = more.as_ref().is_some_and(Response::clicked);
    BigOutput { play: play || (response.clicked() && !more_clicked), response, more }
}

/// Up to four covers in their shapes across `zone`, overlapping a little
/// and alternating up and down; under the pointer (`lean`) they spread
/// apart, each turning into its sibling shape and tilting. No covers: the
/// first shape as a tonal placeholder.
fn collage(ui: &Ui, zone: Rect, covers: &[String], first: MaterialShape, lean: f32, on: Color32) {
    if zone.height() < 40.0 || zone.width() < 40.0 {
        return;
    }
    let n = covers.len().max(1);
    // Each cover a step smaller than the one before, overlapping it a
    // little, alternating up and down.
    let scale = |i: usize| 0.86_f32.powi(i as i32);
    let overlap = 0.78 + 0.08 * lean;
    let jog = 0.1;
    let span: f32 = 1.0 + (1..n).map(|i| scale(i) * overlap).sum::<f32>();
    // Big as the zone allows, both ways.
    let size = (zone.height() / (1.0 + jog)).min(zone.width() / span).min(176.0);
    let mut x = zone.left();
    let mut rects = Vec::with_capacity(n);
    for i in 0..n {
        let d = size * scale(i);
        let dy = if i % 2 == 0 { -jog } else { jog } * size * 0.5;
        rects.push(Rect::from_center_size(pos2(x + d * 0.5, zone.center().y + dy), vec2(d, d)));
        x += d * overlap;
    }
    for i in (0..n).rev() {
        let shape = if i == 0 { first } else { COLLAGE_SHAPES[i % COLLAGE_SHAPES.len()] };
        let cubics = pair_morph(shape, sibling(shape)).as_cubics(lean.clamp(0.0, 1.1));
        let tilt = if i % 2 == 0 { 0.08 } else { -0.08 } * lean;
        paint_cover(ui, &cubics, rects[i], tilt, covers.get(i).map(String::as_str), on.gamma_multiply(0.16));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cell of the board's rows is covered exactly once.
    fn assert_no_holes(cols: usize, big: bool, n: usize) {
        let cells = layout(cols, big, n);
        assert_eq!(cells.len(), n, "{cols} cols, big {big}, {n} tiles");
        let rows = rows(&cells);
        let mut grid = vec![0u8; rows * cols];
        for c in &cells {
            assert!(c.w >= 1 && c.h >= 1 && c.col + c.w <= cols, "{c:?}");
            for r in c.row..c.row + c.h {
                for x in c.col..c.col + c.w {
                    grid[r * cols + x] += 1;
                }
            }
        }
        assert!(grid.iter().all(|&g| g == 1), "{cols} cols, big {big}, {n} tiles: {cells:?}");
    }

    #[test]
    fn the_board_has_no_holes() {
        for cols in [2, 4] {
            for big in [false, true] {
                for n in 0..=8 {
                    assert_no_holes(cols, big, n);
                }
            }
        }
    }

    #[test]
    fn columns_follow_the_width() {
        assert_eq!(columns(1200.0), 4);
        assert_eq!(columns(960.0), 4);
        assert_eq!(columns(959.0), 2);
        assert_eq!(columns(560.0), 2);
    }

    #[test]
    fn the_daylist_is_two_by_two_beside_four_tiles_and_widens_beside_fewer() {
        let c = |col, row, w, h| Cell { col, row, w, h };
        // The daylist, «Твой микс» and three tiles: the sketch of §12.1.
        assert_eq!(layout(4, true, 5), vec![c(0, 0, 2, 2), c(2, 0, 1, 1), c(3, 0, 1, 1), c(2, 1, 1, 1), c(3, 1, 1, 1)]);
        // No tiles: the daylist and «Твой микс» at its full height.
        assert_eq!(layout(4, true, 2), vec![c(0, 0, 3, 2), c(3, 0, 1, 2)]);
        // Two beside it stack in one column.
        assert_eq!(layout(4, true, 3), vec![c(0, 0, 3, 2), c(3, 0, 1, 1), c(3, 1, 1, 1)]);
        // Three: the last one takes both columns of its row.
        assert_eq!(layout(4, true, 4), vec![c(0, 0, 2, 2), c(2, 0, 1, 1), c(3, 0, 1, 1), c(2, 1, 2, 1)]);
        // Everything: two beside it, four in the row under it.
        assert_eq!(
            layout(4, true, 7),
            vec![c(0, 0, 3, 2), c(3, 0, 1, 1), c(3, 1, 1, 1), c(0, 2, 1, 1), c(1, 2, 1, 1), c(2, 2, 1, 1), c(3, 2, 1, 1)]
        );
        // Alone: one flat row.
        assert_eq!(layout(4, true, 1), vec![c(0, 0, 4, 1)]);
        // Narrow: full width, then two per row, a lone last one wide.
        assert_eq!(layout(2, true, 4), vec![c(0, 0, 2, 2), c(0, 2, 1, 1), c(1, 2, 1, 1), c(0, 3, 2, 1)]);
        // Without a big tile: rows, the short last one shared.
        assert_eq!(layout(4, false, 3), vec![c(0, 0, 2, 1), c(2, 0, 1, 1), c(3, 0, 1, 1)]);
    }

    #[test]
    fn cells_map_to_rects_with_the_gap() {
        let r = cell_rect(Cell { col: 1, row: 1, w: 2, h: 1 }, pos2(0.0, 0.0), 100.0, 50.0);
        assert_eq!(r, Rect::from_min_size(pos2(112.0, 62.0), vec2(212.0, 50.0)));
    }

    #[test]
    fn a_kicker_that_repeats_the_title_is_left_out() {
        assert_eq!(kicker(Some("НА ПОВТОРЕ"), "На повторе"), None);
        assert_eq!(kicker(Some("ДЕЙЛИСТ"), "Тягучий вечер четверга"), Some("ДЕЙЛИСТ"));
        assert_eq!(kicker(None, "Спокойное"), None);
    }

    #[test]
    fn tiles_grow_in_one_after_another_and_then_stand_still() {
        let m = crate::Motion::expressive();
        let (spatial, effects) = (m.default_spatial, m.fast_effects);
        // Before its turn: small and clear; the next one waits its stagger.
        assert_eq!(entrance_at(0.0, 0, spatial, effects, false), (ENTER_FROM, 0.0));
        assert_eq!(entrance_at(STAGGER * 0.5, 1, spatial, effects, false).1, 0.0);
        // On the way: growing, visible.
        let (s, a) = entrance_at(0.08, 0, spatial, effects, false);
        assert!(s > ENTER_FROM && s < 1.02 && a > 0.5, "{s} {a}");
        // Done: exactly at rest, whatever the clock says later.
        assert_eq!(entrance_at(ENTER_SECONDS, 0, spatial, effects, false), (1.0, 1.0));
        assert_eq!(entrance_at(60.0, 7, spatial, effects, false), (1.0, 1.0));
        // Reduced motion: a fade only.
        assert_eq!(entrance_at(0.05, 0, spatial, effects, true).0, 1.0);
    }

    #[test]
    fn the_coin_flips_to_the_other_face_and_settles() {
        // Untossed: the «?» face up.
        assert_eq!(coin_pose(0, 5.0, false), (0.0, 0.0));
        for n in 1..=4u32 {
            // A toss starts where the last one rested and ends on the other
            // face, on the ground.
            let (start, lift) = coin_pose(n, 0.0, false);
            assert!((start - (n - 1) as f32 * HALF_TURNS * PI).abs() < 1e-3 && lift.abs() < 1e-3);
            let (end, lift) = coin_pose(n, TOSS_SECONDS, false);
            assert_eq!(lift, 0.0);
            assert!((end.cos() - if n % 2 == 1 { -1.0 } else { 1.0 }).abs() < 1e-3, "toss {n}: {end}");
            // In the air half-way through the flight, turning fast.
            let (mid, lift) = coin_pose(n, AIR * 0.5, false);
            assert!(lift > 0.9 && mid > start + PI, "toss {n}: {mid} {lift}");
            // Nearly settled by the end of the toss: no jump at the cut.
            let (late, _) = coin_pose(n, TOSS_SECONDS - 0.01, false);
            assert!((late - end).abs() < 0.08, "toss {n}: {late} vs {end}");
            // Reduced motion: straight to the other face.
            assert_eq!(coin_pose(n, 0.0, true), (end, 0.0));
        }
    }

    #[test]
    fn every_shape_morphs_into_its_sibling() {
        // `Morph::new` panics on outlines it cannot match.
        for shape in MaterialShape::ALL {
            let to = sibling(shape);
            assert_ne!(shape, to);
            assert!(!pair_morph(shape, to).as_cubics(0.5).is_empty(), "{shape:?} -> {to:?}");
        }
    }
}
