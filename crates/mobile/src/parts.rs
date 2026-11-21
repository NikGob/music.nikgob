//! What the phone's pages are made of: what a page asks for, the bar at the
//! top that turns solid with the page's title once the head has scrolled
//! under it, Metrolist's centred head of an album, a playlist or an artist,
//! section titles, cards in two columns, list rows of playlists and
//! artists, and the lists of tracks.

use egui::{Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_core::i18n::{common, mobile};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{IconButton, LoadingIndicator, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_shapes::MaterialShape;
use nikgob_view::{Route, TrackRow};

use crate::rows;
use crate::text;

/// What a page asked for this frame.
#[derive(Default)]
pub struct PageOut {
    pub nav: Option<Route>,
    pub back: bool,
    /// The menu of a track shown on a page.
    pub menu: Option<(TrackRow, Route)>,
    /// The search bar at the top was tapped.
    pub search: bool,
}

impl PageOut {
    pub fn nav(nav: Option<Route>) -> Self {
        Self { nav, ..Self::default() }
    }
}

pub const BAR_H: f32 = 64.0;

/// The search bar at the top of Главная and Библиотека.
pub const SEARCH_H: f32 = 56.0;

/// Where the search bar of a page `rect` wide goes under the status bar
/// `top` high.
pub fn search_rect(rect: Rect, top: f32) -> Rect {
    Rect::from_min_size(pos2(rect.left() + 16.0, rect.top() + top + 8.0), vec2(rect.width() - 32.0, SEARCH_H))
}

/// Metrolist's search bar at the top of a page: a tap opens Поиск with the
/// keyboard (the field itself lives there). True when tapped.
pub fn search_bar(ui: &mut Ui, rect: Rect, id: Id) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let resp = ui.interact(rect, id, Sense::click());
    let radii = Radii::all(rect.height() * 0.5);
    paint::container(ui.painter(), rect, radii, s.surface_container_high, Stroke::NONE);
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect, radii, s.on_surface, layer);
    let c = pos2(rect.left() + 28.0, rect.center().y);
    Icon::Search.widget(false, s.on_surface, 24.0).paint_at(ui, Rect::from_center_size(c, vec2(24.0, 24.0)));
    let style = TypeRole::BodyLarge.style();
    text::paint_line(ui, pos2(rect.left() + 56.0, rect.center().y - style.line_height * 0.5), common().search, style, s.on_surface_variant, rect.width() - 72.0);
    resp.clicked()
}

/// The bar over a page: ← and the page's title, which fades in on the
/// surface container as `solid` goes 0 → 1 (the head scrolled under it).
/// Returns ← tapped.
pub fn top_bar(ui: &mut Ui, rect: Rect, top: f32, title: &str, solid: f32, back: bool) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), top + BAR_H));
    let k = anim::fraction(ui.ctx(), ui.id().with("bar-solid"), solid.clamp(0.0, 1.0), theme.motion.default_effects);
    if k > 0.001 {
        paint::container(ui.painter(), bar, Radii::all(0.0), s.surface_container.gamma_multiply(k), Stroke::NONE);
    }
    let row = Rect::from_min_size(pos2(bar.left(), bar.top() + top), vec2(bar.width(), BAR_H));
    let mut tapped = false;
    let mut x = row.left() + 16.0;
    if back {
        let b = Rect::from_min_size(pos2(row.left() + 4.0, row.center().y - 24.0), vec2(48.0, 48.0));
        tapped = put(ui, b, IconButton::new(Icon::ArrowBack).id_salt("page-back")).clicked();
        x = b.right() + 4.0;
    }
    if k > 0.01 && !title.is_empty() {
        let style = TypeRole::TitleLarge.style();
        let g = text::line(ui, title, style, s.on_surface, row.right() - 16.0 - x);
        ui.painter().galley(pos2(x, row.center().y - g.size().y * 0.5), g, s.on_surface.gamma_multiply(k));
    }
    tapped
}

/// `widget` centred over `rect`, leaving `ui`'s cursor where it was:
/// egui's `put` moves the cursor of a column to under `rect`, so a row that
/// put its switch or its ⋮ pulled the next row up under itself.
pub fn put(ui: &mut Ui, rect: Rect, widget: impl egui::Widget) -> Response {
    ui.new_child(UiBuilder::new().max_rect(rect).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown))).add(widget)
}

/// The waiting indicator in the middle of the page's room.
pub fn loading(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 160.0), Sense::hover());
    put(ui, Rect::from_center_size(r.center(), vec2(48.0, 48.0)), LoadingIndicator::new().size(48.0));
}

/// An answer with nothing in it: the search icon over «Ничего не нашлось».
pub fn nothing(ui: &mut Ui) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 160.0), Sense::hover());
    Icon::Search.widget(false, s.on_surface_variant, 48.0).paint_at(ui, Rect::from_center_size(pos2(r.center().x, r.top() + 56.0), vec2(48.0, 48.0)));
    let g = text::centred(ui, mobile().nothing_found, TypeRole::TitleMedium.style(), s.on_surface_variant, r.width() - 32.0, 1);
    ui.painter().galley(pos2(r.center().x, r.top() + 96.0), g, s.on_surface_variant);
}

/// An error, the only words a page says of itself.
pub fn error(ui: &mut Ui, why: &str) {
    let theme = Theme::get(ui.ctx());
    let w = ui.available_width() - 32.0;
    let g = text::centred(ui, why, TypeRole::BodyMedium.style(), theme.scheme.error, w, 4);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 32.0), Sense::hover());
    ui.painter().galley(pos2(r.center().x, r.top() + 16.0), g, theme.scheme.error);
}

/// How a head's picture is cut.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Picture {
    Square,
    Round,
}

/// The head of an album, a playlist or an artist. A square picture makes
/// Metrolist's collection head: the cover across the page, then a card
/// with the title, the artist (a link) and one line of what it is and its
/// numbers. A round one (an artist) stays centred: the picture, the name,
/// the line under it. The caller draws the row of actions under it.
/// Returns whether the artist was tapped.
pub struct Head<'a> {
    pub uri: Option<String>,
    pub picture: Picture,
    pub kicker: Option<String>,
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub numbers: Option<String>,
}

/// The side of a collection's cover on a page `width` wide and a screen
/// `height` tall: across the page, but never more than half the screen.
pub fn cover_side(width: f32, height: f32) -> f32 {
    (width - 32.0).min(height * 0.5).max(96.0)
}

/// The facts line of a collection: «Альбом • 2020 • 12 треков • 45 мин».
pub fn facts(kicker: Option<&str>, numbers: Option<&str>) -> String {
    [kicker, numbers].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" • ")
}

pub fn head(ui: &mut Ui, top: f32, h: &Head<'_>) -> bool {
    if h.picture == Picture::Round {
        return round_head(ui, top, h);
    }
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let width = ui.available_width();
    ui.add_space(top + BAR_H);
    let side = cover_side(width, ui.ctx().content_rect().height());
    let (r, _) = ui.allocate_exact_size(vec2(width, side), Sense::hover());
    let pic = Rect::from_center_size(r.center(), vec2(side, side));
    paint::shadow(ui.painter(), pic, Radii::all(28.0), nikgob_md3e::Elevation::Level1);
    phone::paint_cover(ui, pic, h.uri.as_deref(), 28.0);
    ui.add_space(16.0);
    // The card under it.
    let inner_w = width - 32.0 - 32.0;
    let title = text::block(ui, h.title, TypeRole::HeadlineMedium.emphasized(), s.on_surface, inner_w, 2);
    let artist = h.artist.filter(|a| !a.is_empty()).map(|a| text::line(ui, a, TypeRole::TitleLarge.style(), s.primary, inner_w));
    let facts = facts(h.kicker.as_deref(), h.numbers.as_deref());
    let line = (!facts.is_empty()).then(|| text::line(ui, facts, TypeRole::BodyMedium.style(), s.on_surface_variant, inner_w));
    let card_h = 16.0 + title.size().y + artist.as_ref().map_or(0.0, |g| g.size().y) + line.as_ref().map_or(0.0, |g| g.size().y) + 16.0;
    let (r, _) = ui.allocate_exact_size(vec2(width, card_h), Sense::hover());
    let card = r.shrink2(vec2(16.0, 0.0));
    paint::container(ui.painter(), card, Radii::all(20.0), s.surface_container_high, Stroke::NONE);
    let x = card.left() + 16.0;
    let mut y = card.top() + 16.0;
    let th = title.size().y;
    ui.painter().galley(pos2(x, y), title, s.on_surface);
    y += th;
    let mut tapped = false;
    if let Some(g) = artist {
        let hit = Rect::from_min_size(pos2(x, y), g.size()).expand2(vec2(8.0, 6.0));
        tapped = ui.interact(hit, ui.id().with("head-artist"), Sense::click()).clicked();
        let gh = g.size().y;
        ui.painter().galley(pos2(x, y), g, s.primary);
        y += gh;
    }
    if let Some(g) = line {
        ui.painter().galley(pos2(x, y), g, s.on_surface_variant);
    }
    ui.add_space(16.0);
    tapped
}

/// An artist's head: the picture in a circle, the name, the line under it.
fn round_head(ui: &mut Ui, top: f32, h: &Head<'_>) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let width = ui.available_width();
    ui.add_space(top + BAR_H);
    let side = 168.0;
    let (r, _) = ui.allocate_exact_size(vec2(width, side), Sense::hover());
    let pic = Rect::from_center_size(r.center(), vec2(side, side));
    phone::paint_shape_avatar(ui, pic, h.uri.as_deref(), &text::initials(h.title), MaterialShape::Circle, s.primary_container, s.on_primary_container);
    ui.add_space(16.0);
    let g = text::centred(ui, h.title, TypeRole::HeadlineMedium.emphasized(), s.on_surface, width - 32.0, 2);
    let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.center().x, r.top()), g, s.on_surface);
    let mut tapped = false;
    if let Some(a) = h.artist.filter(|a| !a.is_empty()) {
        ui.add_space(4.0);
        let g = text::centred(ui, a, TypeRole::TitleMedium.style(), s.on_surface_variant, width - 48.0, 1);
        let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
        let hit = Rect::from_center_size(r.center(), vec2(g.size().x + 16.0, r.height() + 12.0));
        tapped = ui.interact(hit, ui.id().with("head-artist"), Sense::click()).clicked();
        ui.painter().galley(pos2(r.center().x, r.top()), g, s.on_surface_variant);
    }
    let facts = facts(h.kicker.as_deref(), h.numbers.as_deref());
    if !facts.is_empty() {
        ui.add_space(2.0);
        let g = text::centred(ui, facts, TypeRole::BodyMedium.style(), s.on_surface_variant, width - 32.0, 1);
        let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.center().x, r.top()), g, s.on_surface_variant);
    }
    ui.add_space(16.0);
    tapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_facts_join_what_there_is() {
        assert_eq!(facts(Some("Альбом • 2020"), Some("12 треков")), "Альбом • 2020 • 12 треков");
        assert_eq!(facts(None, Some("3 трека")), "3 трека");
        assert_eq!(facts(Some(""), None), "");
    }

    #[test]
    fn a_cover_is_across_the_page_but_never_more_than_half_the_screen() {
        assert_eq!(cover_side(392.0, 852.0), 360.0);
        assert_eq!(cover_side(412.0, 600.0), 300.0);
    }
}

/// A section header: 22 sp bold in `primary`, a 52 dp row, the text at x 16.
pub const SECTION_H: f32 = 52.0;

pub fn section_head(ui: &mut Ui, title: &str) {
    let theme = Theme::get(ui.ctx());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), SECTION_H), Sense::hover());
    let style = TypeRole::TitleLarge.emphasized();
    text::paint_line(ui, pos2(r.left() + 16.0, r.center().y - style.line_height * 0.5), title, style, theme.scheme.primary, r.width() - 32.0);
}

/// A library grid: 12 dp outer margin, 24 dp between the two thumbs.
pub const GRID_OUTER: f32 = 12.0;
pub const GRID_BETWEEN: f32 = 24.0;
/// Under a caption before the next thumb (the measured pitch is about 242).
const GRID_ROW_GAP: f32 = 20.0;
const GRID_CAPTION: f32 = 6.0;

/// Thumb side of a 2-column grid on a page `page_w` wide: 168 at 384.
pub fn grid_thumb(page_w: f32) -> f32 {
    (page_w - GRID_OUTER * 2.0 - GRID_BETWEEN) / 2.0
}

fn grid_cell_h(thumb: f32) -> f32 {
    thumb + GRID_CAPTION + TypeRole::TitleMedium.style().line_height + TypeRole::BodyLarge.style().line_height
}

/// Cards in two columns; `draw` gets each cell.
pub fn grid2(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, n: usize, mut draw: impl FnMut(&mut Ui, usize, Rect)) {
    if n == 0 {
        return;
    }
    let width = ui.available_width();
    let w = grid_thumb(width);
    let h = grid_cell_h(w);
    let lines = n.div_ceil(2);
    let total = lines as f32 * h + lines.saturating_sub(1) as f32 * GRID_ROW_GAP;
    let (area, _) = ui.allocate_exact_size(vec2(width, total), Sense::hover());
    let clip = ui.clip_rect();
    let base = Id::new(id);
    for i in 0..n {
        let (line, col) = (i / 2, i % 2);
        let r = Rect::from_min_size(
            pos2(area.left() + GRID_OUTER + col as f32 * (w + GRID_BETWEEN), area.top() + line as f32 * (h + GRID_ROW_GAP)),
            vec2(w, h),
        );
        if !clip.intersects(r) {
            continue;
        }
        let mut cui = ui.new_child(UiBuilder::new().max_rect(r).id_salt(base.with(i)));
        draw(&mut cui, i, r);
    }
}

/// A grid card: radius 4 (a circle for an artist), 16 sp bold then 16 sp
/// muted, 6 dp under the thumb. No play mark on the cover.
pub fn grid_card(ui: &mut Ui, rect: Rect, id: Id, uri: Option<&str>, title: &str, sub: &str, picture: Picture) -> Response {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let resp = ui.interact(rect, id, Sense::click());
    let side = rect.width();
    let pic = Rect::from_min_size(rect.min, vec2(side, side));
    let round = picture == Picture::Round;
    let radii = if round { Radii::all(side * 0.5) } else { Radii::all(rows::THUMB_R) };
    match picture {
        Picture::Square => phone::paint_cover(ui, pic, uri, rows::THUMB_R),
        Picture::Round => phone::paint_shape_avatar(ui, pic, uri, &text::initials(title), MaterialShape::Circle, s.secondary_container, s.on_secondary_container),
    }
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), pic, radii, s.on_surface, layer);
    let ts = TypeRole::TitleMedium.emphasized();
    let ss = TypeRole::BodyLarge.style();
    let y = pic.bottom() + GRID_CAPTION;
    for (k, (line, style, color)) in [(title, ts, s.on_surface), (sub, ss, s.on_surface_variant)].into_iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let g = text::line(ui, line, style, color, side);
        let x = if round { rect.center().x - g.size().x * 0.5 } else { rect.left() };
        ui.painter().galley(pos2(x, y + if k == 0 { 0.0 } else { ts.line_height }), g, color);
    }
    resp
}

/// A 64 dp list row with a 48 dp thumb at x 14, radius 4 (a circle for an
/// artist). Title 14 sp bold, subtitle 12 sp.
pub fn media_row(ui: &mut Ui, id: Id, uri: Option<&str>, title: &str, sub: &str, picture: Picture) -> Response {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows::ROW_H), Sense::hover());
    let resp = ui.interact(r, id, Sense::click());
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), r, Radii::all(0.0), s.on_surface, layer);
    let pic = rows::thumb_rect(r);
    match picture {
        Picture::Square => phone::paint_cover(ui, pic, uri, rows::THUMB_R),
        Picture::Round => phone::paint_shape_avatar(ui, pic, uri, &text::initials(title), MaterialShape::Circle, s.secondary_container, s.on_secondary_container),
    }
    let x = r.left() + rows::TITLE_X;
    let w = (r.right() - 16.0 - x).max(16.0);
    let ts = TypeRole::TitleSmall.emphasized();
    let ss = TypeRole::BodySmall.style();
    let y = r.center().y - (ts.line_height + if sub.is_empty() { 0.0 } else { ss.line_height }) * 0.5;
    text::paint_line(ui, pos2(x, y), title, ts, s.on_surface, w);
    if !sub.is_empty() {
        text::paint_line(ui, pos2(x, y + ts.line_height), sub, ss, s.on_surface_variant, w);
    }
    resp
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    #[test]
    fn a_library_thumb_is_168_on_a_384_page() {
        assert_eq!(grid_thumb(384.0), 168.0);
        assert_eq!(grid_thumb(360.0), 156.0);
        assert_eq!(grid_thumb(392.0), 172.0);
        assert_eq!(grid_thumb(412.0), 182.0);
        assert_eq!(grid_cell_h(168.0), 168.0 + 6.0 + TypeRole::TitleMedium.style().line_height + TypeRole::BodyLarge.style().line_height);
        assert_eq!(SECTION_H, 52.0);
    }
}
