//! The bento board of Главная (docs/stage4.md §12.1): a column grid where
//! the daylist takes a 2×2 block and «Твой микс» and the smaller mixes a
//! cell each, with no holes whatever the count; and the filled tile every
//! mix card of the page is drawn with (the board, moods, style mixes).

use egui::{Color32, Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_shapes::{MaterialShape, paint as shape_paint};

use super::widgets;
use crate::{
    Icon, Theme, TypeRole, TypeStyle,
    components::{IconButton, Radii, Size, Style, anim, button::Colors, paint},
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
/// landscape cell, so the 2×2 daylist stays a square-ish block.
pub fn row_height(cols: usize, col_w: f32) -> f32 {
    if cols >= 4 {
        (col_w * 0.66).clamp(156.0, 180.0)
    } else {
        (col_w * 0.4).clamp(144.0, 164.0)
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
const STAGGER: f32 = 0.06;

/// The board's one-time entrance for tile `i`: how much of its rise is
/// left (1 → 0, may overshoot) and its opacity. Counted from the first
/// frame the board was drawn and solved from the clock, so it plays once
/// per run and a window shown again lands where it would be. Reduced
/// motion: a fade only.
pub fn entrance(ui: &Ui, i: usize) -> (f32, f32) {
    let ctx = ui.ctx();
    let now = anim::now(ctx);
    let t0 = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(Id::new("home-bento-enter"), || now));
    let t = (now - t0) as f32 - i as f32 * STAGGER;
    if t > 1.5 {
        return (0.0, 1.0);
    }
    ctx.request_repaint();
    if t <= 0.0 {
        return (1.0, 0.0);
    }
    let theme = Theme::get(ctx);
    let alpha = 1.0 - theme.motion.default_effects.solve(1.0, 0.0, t).0;
    let rise = if theme.reduced_motion { 0.0 } else { theme.motion.default_spatial.solve(1.0, 0.0, t).0 };
    (rise, alpha.clamp(0.0, 1.0))
}

/// Draws `add` in a child of `ui` over `rect`, risen and faded in as tile
/// `i` of the entrance.
pub fn entering<R>(ui: &mut Ui, rect: Rect, i: usize, salt: impl std::hash::Hash + std::fmt::Debug, add: impl FnOnce(&mut Ui, Rect) -> R) -> R {
    let (rise, alpha) = entrance(ui, i);
    let rect = rect.translate(vec2(0.0, rise * 32.0));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(salt));
    if alpha < 1.0 {
        child.set_opacity(alpha);
    }
    add(&mut child, rect)
}

/// What a tile shows.
pub struct Tile<'a> {
    /// The container and content colours (scheme roles).
    pub fill: Color32,
    pub on: Color32,
    /// The quiet label over the title («ДЕЙЛИСТ»).
    pub kicker: Option<&'a str>,
    pub title: &'a str,
    /// The title may wrap into two lines.
    pub title_two_lines: bool,
    pub subtitle: &'a str,
    /// Cover uris: the first is cut into `shape`; with `stack`, up to three
    /// overlap (a style mix's albums).
    pub covers: &'a [String],
    pub shape: MaterialShape,
    pub stack: bool,
    /// The tooltip of ▶.
    pub play: String,
}

/// What happened to a tile this frame.
pub struct TileOutput {
    pub response: Response,
    /// The tile or its ▶ was clicked: play.
    pub play: bool,
}

/// The press squish, the hover/press state layer target and the hover of a
/// tile's response.
fn feedback(ui: &Ui, id: Id, resp: &Response, rect: Rect, depth: f32) -> (Rect, f32, bool) {
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
    (Rect::from_center_size(rect.center(), rect.size() * squish), layer, hover)
}

/// A text galley of `role` in `color`, wrapped to `width` in at most
/// `rows` rows (an ellipsis after the last).
fn text(ui: &Ui, s: &str, role: TypeStyle, color: Color32, width: f32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(s.to_owned(), role.text_format(color));
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(1.0),
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.painter().layout_job(job)
}

/// Shows the kicker only when it says more than the title does («НА
/// ПОВТОРЕ» over «На повторе» says nothing).
pub fn kicker<'a>(kicker: Option<&'a str>, title: &str) -> Option<&'a str> {
    kicker.filter(|k| k.to_lowercase() != title.to_lowercase())
}

/// ▶ on a tile: its content colour as the container and its container
/// colour as the glyph, so it stands out on every role (primary on a
/// primary or tertiary container would not).
fn play_colors(t: &Tile<'_>) -> Colors {
    Colors { container: t.on, content: t.fill, border: Color32::TRANSPARENT }
}

/// ▶ fading in at `slot` while the tile is hovered (fast effects); true
/// when clicked.
fn hover_play(ui: &mut Ui, id: Id, hover: bool, slot: Rect, size: Size, t: &Tile<'_>) -> bool {
    let theme = Theme::get(ui.ctx());
    let k = anim::fraction(ui.ctx(), id.with("play"), f32::from(u8::from(hover)), theme.motion.fast_effects).clamp(0.0, 1.0);
    if k <= 0.01 {
        return false;
    }
    let mut child = ui.new_child(UiBuilder::new().max_rect(slot).id_salt(id.with("play-slot")));
    child.set_opacity(k * ui.opacity());
    child
        .add_enabled(
            k > 0.5,
            IconButton::new(Icon::PlayArrow)
                .size(size)
                .style(Style::Filled)
                .filled_icon(true)
                .colors(play_colors(t))
                .tooltip(t.play.as_str())
                .id_salt(id.with("play-btn")),
        )
        .clicked()
}

/// A tile filling `rect`: a filled container in its scheme colour, the
/// cover cut into its shape in the corner, the kicker, title and a quiet
/// line at the bottom; ▶ on hover. A click anywhere plays.
pub fn tile(ui: &mut Ui, rect: Rect, t: &Tile<'_>, id: Id) -> TileOutput {
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let (card, layer, hover) = feedback(ui, id, &response, rect, 0.96);
    let radii = Radii::all(shape::EXTRA_LARGE);
    let painter = ui.painter().clone();
    paint::container(&painter, card, radii, t.fill, Stroke::NONE);

    let pad = 16.0;
    // A wide tile (a short last row) holds a big cover at its right end;
    // the others a smaller one in the corner over the text.
    let wide = card.width() > card.height() * 2.4;
    let (art, text_w) = if wide {
        let size = (card.height() - 2.0 * pad).min(160.0);
        let art = Rect::from_min_size(pos2(card.right() - pad - size, card.center().y - size * 0.5), vec2(size, size));
        (art, card.width() - 3.0 * pad - size)
    } else {
        let size = (card.width().min(card.height()) * 0.4).clamp(56.0, 104.0);
        (Rect::from_min_size(card.min + vec2(pad, pad), vec2(size, size)), card.width() - 2.0 * pad)
    };
    let theme = Theme::get(ui.ctx());
    let turn = anim::f32(ui.ctx(), id.with("turn"), if hover { 0.12 } else { 0.0 }, theme.motion.default_spatial);
    // The cover leans in under the pointer.
    let lean = anim::fraction(ui.ctx(), id.with("lean"), f32::from(u8::from(hover)), theme.motion.default_spatial);
    let art = art.expand(lean * art.width() * 0.03);
    let size = art.width();
    if t.covers.is_empty() {
        shape_paint::fill_shape(&painter, t.shape.polygon().cubics(), art, turn, t.on.gamma_multiply(0.16));
    } else if t.stack && t.covers.len() > 1 {
        // Up to three albums overlapping, the first on top.
        let n = t.covers.len().min(3);
        let step = size * 0.55;
        for i in (0..n).rev() {
            let r = art.translate(vec2(i as f32 * step, 0.0));
            widgets::shape_image(ui, Some(&t.covers[i]), "", r, STACK_SHAPES[i]);
        }
    } else {
        widgets::shape_image(ui, Some(&t.covers[0]), "", art, t.shape);
    }

    // Bottom up: the quiet line, the title, the kicker.
    let mut y = card.bottom() - pad;
    if !t.subtitle.is_empty() {
        let g = text(ui, t.subtitle, TypeRole::BodyMedium.style(), t.on, text_w, 1);
        y -= g.size().y;
        painter.galley(pos2(card.left() + pad, y), g, t.on);
        y -= 2.0;
    }
    // Two title lines only when they clear the cover.
    let role = TypeRole::TitleLarge.style();
    let room = y - if wide { card.top() + pad } else { art.bottom() + 8.0 } - if t.kicker.is_some() { 20.0 } else { 0.0 };
    let rows = if t.title_two_lines && room >= 2.0 * role.line_height { 2 } else { 1 };
    let mut g = text(ui, t.title, role, t.on, text_w, rows);
    if g.rows.len() > 1 {
        // A long name («A, B и другие») steps down to title medium.
        g = text(ui, t.title, TypeRole::TitleMedium.style(), t.on, text_w, rows);
    }
    y -= g.size().y;
    painter.galley(pos2(card.left() + pad, y), g, t.on);
    if let Some(k) = t.kicker {
        let g = text(ui, k, TypeRole::LabelMedium.style(), t.on, text_w, 1);
        y -= g.size().y + 2.0;
        painter.galley(pos2(card.left() + pad, y), g, t.on);
    }

    paint::state_layer(&painter, card, radii, t.on, layer);

    let size = Size::M.height();
    let slot = if wide {
        Rect::from_center_size(art.center(), vec2(size, size))
    } else {
        Rect::from_min_size(pos2(card.right() - pad - size, card.top() + pad), vec2(size, size))
    };
    let play = hover_play(ui, id, hover, slot, Size::M, t);
    let play = play || response.clicked();
    TileOutput { response, play }
}

/// The shapes of a style mix's overlapping albums.
const STACK_SHAPES: [MaterialShape; 3] = [MaterialShape::Cookie9Sided, MaterialShape::Clover4Leaf, MaterialShape::Sunny];

/// The shapes of the big tile's collage, left to right.
const COLLAGE_SHAPES: [MaterialShape; 4] =
    [MaterialShape::Sunny, MaterialShape::Cookie9Sided, MaterialShape::Clover4Leaf, MaterialShape::Pill];

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
/// ▶ side by side.
pub fn big_tile(ui: &mut Ui, rect: Rect, t: &Tile<'_>, id: Id, more: bool) -> BigOutput {
    let response = ui.interact(rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let (card, layer, hover) = feedback(ui, id, &response, rect, 0.98);
    let radii = Radii::all(shape::EXTRA_LARGE);
    let painter = ui.painter().clone();
    paint::container(&painter, card, radii, t.fill, Stroke::NONE);
    let theme = Theme::get(ui.ctx());
    let pad = 24.0;
    let flat = card.height() < 240.0 && card.width() > card.height() * 2.2;
    let lean = anim::fraction(ui.ctx(), id.with("lean"), f32::from(u8::from(hover)), theme.motion.default_spatial);

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
                .colors(play_colors(t))
                .tooltip(t.play.as_str())
                .id_salt(id.with("play-btn")),
        )
        .clicked();
    let more_clicked = more.as_ref().is_some_and(Response::clicked);
    BigOutput { play: play || (response.clicked() && !more_clicked), response, more }
}

/// Up to four covers in their shapes across `zone`, overlapping a little
/// and alternating up and down; they spread apart under the pointer
/// (`lean`). No covers: the first shape as a tonal placeholder.
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
        let r = rects[i];
        let shape = if i == 0 { first } else { COLLAGE_SHAPES[i % COLLAGE_SHAPES.len()] };
        match covers.get(i) {
            Some(uri) => widgets::shape_image(ui, Some(uri), "", r, shape),
            None => shape_paint::fill_shape(ui.painter(), shape.polygon().cubics(), r, 0.0, on.gamma_multiply(0.16)),
        }
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
}
