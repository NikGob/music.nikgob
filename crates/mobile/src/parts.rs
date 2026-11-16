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
}

impl PageOut {
    pub fn nav(nav: Option<Route>) -> Self {
        Self { nav, ..Self::default() }
    }
}

pub const BAR_H: f32 = 64.0;

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
        tapped = ui.put(b, IconButton::new(Icon::ArrowBack).id_salt("page-back")).clicked();
        x = b.right() + 4.0;
    }
    if k > 0.01 && !title.is_empty() {
        let style = TypeRole::TitleLarge.style();
        let g = text::line(ui, title, style, s.on_surface, row.right() - 16.0 - x);
        ui.painter().galley(pos2(x, row.center().y - g.size().y * 0.5), g, s.on_surface.gamma_multiply(k));
    }
    tapped
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
    ui.put(Rect::from_center_size(r.center(), vec2(48.0, 48.0)), LoadingIndicator::new().size(48.0));
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

/// The centred head of an album, a playlist or an artist: the picture,
/// the kicker over the title, the artist (a link), the numbers; then the
/// row of actions the caller draws under it. Returns where the head ends
/// and whether the artist was tapped.
pub struct Head<'a> {
    pub uri: Option<String>,
    pub picture: Picture,
    pub kicker: Option<String>,
    pub title: &'a str,
    pub artist: Option<&'a str>,
    pub numbers: Option<String>,
}

pub fn head(ui: &mut Ui, top: f32, h: &Head<'_>) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let width = ui.available_width();
    ui.add_space(top + BAR_H);
    let side = if h.picture == Picture::Round { 168.0 } else { 208.0f32.min(width - 64.0) };
    let (r, _) = ui.allocate_exact_size(vec2(width, side), Sense::hover());
    let pic = Rect::from_center_size(r.center(), vec2(side, side));
    match h.picture {
        Picture::Square => {
            paint::shadow(ui.painter(), pic, Radii::all(24.0), nikgob_md3e::Elevation::Level2);
            phone::paint_cover(ui, pic, h.uri.as_deref(), 24.0);
        }
        Picture::Round => {
            phone::paint_shape_avatar(ui, pic, h.uri.as_deref(), &text::initials(h.title), MaterialShape::Circle, s.primary_container, s.on_primary_container);
        }
    }
    ui.add_space(16.0);
    if let Some(k) = &h.kicker {
        let style = TypeRole::LabelMedium.emphasized();
        let g = text::centred(ui, k.to_uppercase(), style, s.primary, width - 32.0, 1);
        let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.center().x, r.top()), g, s.primary);
        ui.add_space(4.0);
    }
    let g = text::centred(ui, h.title, TypeRole::HeadlineSmall.style(), s.on_surface, width - 32.0, 2);
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
    if let Some(n) = &h.numbers {
        ui.add_space(2.0);
        let g = text::centred(ui, n, TypeRole::BodySmall.style(), s.on_surface_variant, width - 32.0, 1);
        let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
        ui.painter().galley(pos2(r.center().x, r.top()), g, s.on_surface_variant);
    }
    ui.add_space(16.0);
    tapped
}

/// A row of actions centred under a head; `add` draws them left to right
/// in a horizontal layout.
pub fn actions(ui: &mut Ui, width_hint: f32, add: impl FnOnce(&mut Ui)) {
    let width = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(width, 56.0), Sense::hover());
    let row = Rect::from_center_size(r.center(), vec2(width_hint.min(width - 32.0), 56.0));
    let mut cui = ui.new_child(UiBuilder::new().max_rect(row).layout(egui::Layout::left_to_right(egui::Align::Center)));
    cui.spacing_mut().item_spacing.x = 8.0;
    add(&mut cui);
    ui.add_space(8.0);
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
        Picture::Square => phone::paint_cover(ui, pic, uri, 16.0),
        Picture::Round => phone::paint_shape_avatar(ui, pic, uri, &text::initials(title), MaterialShape::Circle, s.secondary_container, s.on_secondary_container),
    }
    let ts = TypeRole::TitleSmall.style();
    let y = rect.top() + side + 8.0;
    text::paint_line(ui, pos2(rect.left(), y), title, ts, s.on_surface, side);
    if !sub.is_empty() {
        text::paint_line(ui, pos2(rect.left(), y + ts.line_height), sub, TypeRole::BodySmall.style(), s.on_surface_variant, side);
    }
    resp
}

/// The height of a card `w` wide.
pub fn card_h(w: f32) -> f32 {
    w + 8.0 + 20.0 + 16.0 + 8.0
}

/// Cards in `cols` columns with the page's margins; `draw` gets each cell.
pub fn grid(ui: &mut Ui, n: usize, cols: usize, mut draw: impl FnMut(&mut Ui, usize, Rect)) {
    if n == 0 {
        return;
    }
    let width = ui.available_width();
    let gap = 12.0;
    let cols = cols.max(1);
    let w = (width - 32.0 - gap * (cols as f32 - 1.0)) / cols as f32;
    let h = card_h(w);
    let lines = n.div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(width, lines as f32 * (h + gap)), Sense::hover());
    let clip = ui.clip_rect();
    for i in 0..n {
        let (line, col) = (i / cols, i % cols);
        let r = Rect::from_min_size(pos2(area.left() + 16.0 + col as f32 * (w + gap), area.top() + line as f32 * (h + gap)), vec2(w, h));
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

/// The tracks of a page, one row each: a tap plays `play(i)`, a swipe to
/// the right plays the song next, a long press or ⋮ opens its menu.
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
    let row_h = if numbered { rows::NUMBERED_H } else { rows::ROW_H };
    rows::list(ui, id, tracks.len(), row_h, |ui, i, r| {
        let t = &tracks[i];
        let look = if numbered { Look::numbered(t, state) } else { Look::list(t, state) };
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
