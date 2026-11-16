//! What the phone's pages are made of: what a page asks for, the bar at the
//! top that turns solid with the page's title once the head has scrolled
//! under it, Metrolist's centred head of an album, a playlist or an artist,
//! section titles, cards in two columns, list rows of playlists and
//! artists, and the lists of tracks.

use egui::{Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{IconButton, LoadingIndicator, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_shapes::MaterialShape;
use nikgob_view::{Command, Route, TrackRow, ViewState};

use crate::rows::{self, Look};
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

/// The search bar at the top of Главная and Библиотека, and the room it
/// takes with its margins.
pub const SEARCH_H: f32 = 56.0;
pub const SEARCH_ROOM: f32 = 8.0 + SEARCH_H + 8.0;

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
    text::paint_line(ui, pos2(rect.left() + 56.0, rect.center().y - style.line_height * 0.5), "Поиск", style, s.on_surface_variant, rect.width() - 72.0);
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

/// A section's title over its content.
pub fn section(ui: &mut Ui, title: &str) {
    let theme = Theme::get(ui.ctx());
    ui.add_space(16.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let style = TypeRole::TitleMedium.emphasized();
    text::paint_line(ui, pos2(r.left() + 16.0, r.center().y - style.line_height * 0.5), title, style, theme.scheme.on_surface, r.width() - 32.0);
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
    let g = text::centred(ui, "Ничего не нашлось", TypeRole::TitleMedium.style(), s.on_surface_variant, r.width() - 32.0, 1);
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

/// One of the round toggles after «Слушать» and shuffle.
#[derive(Clone, Copy)]
pub struct Action {
    pub icon: Icon,
    pub on: bool,
}

/// What the row under a head was asked.
#[derive(Default)]
pub struct Played {
    pub play: bool,
    pub shuffle: bool,
    /// The toggle tapped.
    pub action: Option<usize>,
}

/// The width of the row's toggles and the room each takes.
const ACTION_W: f32 = 48.0;
const ACTION_STEP: f32 = ACTION_W + 8.0;

/// Metrolist's row under a head, 40 dp tall: «Слушать» and shuffle as one
/// connected pair across what is left, then round toggles (♥,
/// «Следующим», …), each on the primary colour while on.
pub fn play_row(ui: &mut Ui, id: Id, enabled: bool, actions: &[Action]) -> Played {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let width = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    let row = r.shrink2(vec2(16.0, 0.0));
    let mut out = Played::default();
    let pair_w = (row.width() - actions.len() as f32 * ACTION_STEP).max(ACTION_W * 2.0 + 2.0);
    let shuffle = Rect::from_min_size(pos2(row.left() + pair_w - ACTION_W, row.top()), vec2(ACTION_W, row.height()));
    let play = Rect::from_min_max(row.min, pos2(shuffle.left() - 2.0, row.bottom()));
    let alpha = if enabled { 1.0 } else { 0.38 };
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let tonal = if theme.dark { s.secondary_container.gamma_multiply(0.7) } else { s.secondary_container };

    let p = ui.interact(play, id.with("play"), sense);
    let pr = Radii::sides(20.0, 4.0);
    paint::container(ui.painter(), play, pr, s.primary.gamma_multiply(alpha), Stroke::NONE);
    press_layer(ui, play, pr, id.with("play-l"), &p, s.on_primary);
    let g = text::line(ui, "Слушать", TypeRole::LabelLarge.style(), s.on_primary.gamma_multiply(alpha), (play.width() - 52.0).max(8.0));
    let content = 20.0 + 8.0 + g.size().x;
    let x0 = play.center().x - content * 0.5;
    Icon::PlayArrow.widget(true, s.on_primary.gamma_multiply(alpha), 20.0).paint_at(ui, Rect::from_min_size(pos2(x0, play.center().y - 10.0), vec2(20.0, 20.0)));
    ui.painter().galley(pos2(x0 + 28.0, play.center().y - g.size().y * 0.5), g, s.on_primary);
    out.play = enabled && p.clicked();

    let sh = ui.interact(shuffle, id.with("shuffle"), sense);
    let sr = Radii::sides(4.0, 20.0);
    paint::container(ui.painter(), shuffle, sr, tonal.gamma_multiply(alpha), Stroke::NONE);
    press_layer(ui, shuffle, sr, id.with("shuffle-l"), &sh, s.on_secondary_container);
    Icon::Shuffle.widget(false, s.on_secondary_container.gamma_multiply(alpha), 20.0).paint_at(ui, Rect::from_center_size(shuffle.center(), vec2(20.0, 20.0)));
    out.shuffle = enabled && sh.clicked();

    let mut x = shuffle.right() + 8.0;
    for (i, a) in actions.iter().enumerate() {
        let b = Rect::from_min_size(pos2(x, row.top()), vec2(ACTION_W, row.height()));
        x += ACTION_STEP;
        let resp = ui.interact(b, id.with(("action", i)), Sense::click());
        let radii = Radii::all(20.0);
        let (fill, ink) = if a.on { (s.primary, s.on_primary) } else { (tonal, s.on_secondary_container) };
        paint::container(ui.painter(), b, radii, fill, Stroke::NONE);
        press_layer(ui, b, radii, id.with(("action-l", i)), &resp, ink);
        a.icon.widget(a.on, ink, 20.0).paint_at(ui, Rect::from_center_size(b.center(), vec2(20.0, 20.0)));
        if resp.clicked() {
            out.action = Some(i);
        }
    }
    ui.add_space(16.0);
    out
}

/// The state layer of a press on `rect`.
fn press_layer(ui: &Ui, rect: Rect, radii: Radii, id: Id, resp: &Response, ink: egui::Color32) {
    let theme = Theme::get(ui.ctx());
    let layer = anim::fraction(ui.ctx(), id, if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect, radii, ink, layer);
}

/// A card of a grid: the picture (square, or round for an artist) over a
/// title and a line under it. Tapped: the response's click.
pub fn card(ui: &mut Ui, rect: Rect, id: Id, uri: Option<&str>, title: &str, sub: &str, picture: Picture) -> Response {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let resp = ui.interact(rect, id, Sense::click());
    let side = rect.width();
    let pic = Rect::from_min_size(rect.min, vec2(side, side));
    let press = anim::fraction(ui.ctx(), id.with("press"), if resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, theme.motion.fast_spatial);
    let pic = pic.shrink(side * 0.03 * press);
    match picture {
        Picture::Square => phone::paint_cover(ui, pic, uri, 12.0),
        Picture::Round => phone::paint_shape_avatar(ui, pic, uri, &text::initials(title), MaterialShape::Circle, s.secondary_container, s.on_secondary_container),
    }
    // The caption: the title over a smaller line, centred under an artist.
    let ts = TypeRole::BodyMedium.emphasized();
    let ss = TypeRole::LabelSmall.style();
    let y = rect.top() + side + CAPTION_GAP;
    let centred = picture == Picture::Round;
    for (k, (line, style, color)) in [(title, ts, s.on_surface), (sub, ss, s.on_surface_variant)].into_iter().enumerate() {
        if line.is_empty() {
            continue;
        }
        let g = text::line(ui, line, style, color, side);
        let x = if centred { rect.center().x - g.size().x * 0.5 } else { rect.left() };
        ui.painter().galley(pos2(x, y + if k == 0 { 0.0 } else { ts.line_height }), g, color);
    }
    resp
}

/// The space between a card's picture and its caption.
const CAPTION_GAP: f32 = 6.0;
/// The gaps of a grid: across and down (Metrolist's).
const GRID_GAP_X: f32 = 6.0;
const GRID_GAP_Y: f32 = 12.0;

/// The height of a card `w` wide.
pub fn card_h(w: f32) -> f32 {
    w + CAPTION_GAP + TypeRole::BodyMedium.style().line_height + TypeRole::LabelSmall.style().line_height + 8.0
}

/// Cards in `cols` columns with the page's margins; `draw` gets each cell.
pub fn grid(ui: &mut Ui, n: usize, cols: usize, mut draw: impl FnMut(&mut Ui, usize, Rect)) {
    if n == 0 {
        return;
    }
    let width = ui.available_width();
    let cols = cols.max(1);
    let w = (width - 32.0 - GRID_GAP_X * (cols as f32 - 1.0)) / cols as f32;
    let h = card_h(w);
    let lines = n.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(width, lines as f32 * (h + GRID_GAP_Y)), Sense::hover());
    let clip = ui.clip_rect();
    for i in 0..n {
        let (line, col) = (i / cols, i % cols);
        let r = Rect::from_min_size(pos2(area.left() + 16.0 + col as f32 * (w + GRID_GAP_X), area.top() + line as f32 * (h + GRID_GAP_Y)), vec2(w, h));
        if !clip.intersects(r) {
            continue;
        }
        let mut cui = ui.new_child(UiBuilder::new().max_rect(r).id_salt(("grid-cell", i)));
        draw(&mut cui, i, r);
    }
}

/// A list row of a playlist or an artist: its picture, its name and a
/// line under it. Tapped: the response's click.
pub fn item(ui: &mut Ui, id: Id, uri: Option<&str>, title: &str, sub: &str, picture: Picture) -> Response {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 72.0), Sense::hover());
    let resp = ui.interact(r, id, Sense::click());
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), r.shrink2(vec2(8.0, 2.0)), Radii::all(16.0), s.on_surface, layer);
    let pic = Rect::from_min_size(pos2(r.left() + 16.0, r.center().y - 28.0), vec2(56.0, 56.0));
    match picture {
        Picture::Square => phone::paint_cover(ui, pic, uri, 8.0),
        Picture::Round => phone::paint_shape_avatar(ui, pic, uri, &text::initials(title), MaterialShape::Circle, s.secondary_container, s.on_secondary_container),
    }
    let x = pic.right() + 16.0;
    let w = r.right() - 16.0 - x;
    let ts = TypeRole::BodyLarge.style();
    let ss = TypeRole::BodyMedium.style();
    let y = r.center().y - (ts.line_height + if sub.is_empty() { 0.0 } else { ss.line_height }) * 0.5;
    text::paint_line(ui, pos2(x, y), title, ts, s.on_surface, w);
    if !sub.is_empty() {
        text::paint_line(ui, pos2(x, y + ts.line_height), sub, ss, s.on_surface_variant, w);
    }
    resp
}

/// The tracks of a page as one grouped list: a tap plays `play(i)`, a
/// swipe to the right plays the song next, a long press or ⋮ opens its
/// menu.
#[allow(clippy::too_many_arguments)]
pub fn tracks(
    ui: &mut Ui,
    id: &str,
    tracks: &[TrackRow],
    numbered: bool,
    here: &Route,
    state: &ViewState,
    play: impl Fn(usize) -> Command,
    out: &mut Vec<Command>,
    page: &mut PageOut,
) {
    let row_h = if numbered { rows::NUMBERED_H } else { rows::ROW_H } + rows::GROUP_GAP;
    let n = tracks.len();
    rows::list(ui, id, n, row_h, |ui, i, r| {
        let t = &tracks[i];
        let look = if numbered { Look::numbered(t, state) } else { Look::list(t, state) }.grouped(i, n);
        let o = rows::track(ui, r, Id::new((id, i)), t, look, state);
        if o.tap {
            out.push(if look.playing { Command::TogglePlay } else { play(i) });
        }
        if o.play_next {
            out.push(Command::PlayNext(vec![t.id]));
        }
        if o.menu {
            page.menu = Some((t.clone(), here.clone()));
        }
    });
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
