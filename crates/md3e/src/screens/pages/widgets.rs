//! Building blocks shared by pages: section headers, horizontal card rows,
//! cards with their context menus, track lists, shape-cropped avatars.

use egui::{
    Color32, Id, Rect, Response, Sense, Stroke, Ui, UiBuilder, pos2, vec2,
};
use nikgob_core::i18n;
use nikgob_shapes::MaterialShape;
use std::collections::HashMap;

use nikgob_view::{
    AlbumMark, ArtistCard, Command, LIKED_PLAYLIST, LikeTarget, PlaylistCard, PlaylistId, ReleaseCard, ReleaseId,
    ReleaseLine, Route, SourceKind, Status, TrackId, TrackRow, ViewState, WishlistScope, WishlistStatus, WishlistView,
};

use super::{Ask, PageState};
use crate::{
    Icon, Theme, TypeRole,
    components::{
        Equalizer, IconButton, Leading, ListItem, MediaCard, Menu, MenuItem, Radii, Size, Style,
        anim, paint,
    },
    screens::{artist_links, common},
    tokens::shape,
};
mod cards;
mod menu;
mod track_list;

pub use cards::*;
pub use menu::{Row, SubId, menu_of, row_acts, shown_rows, sub_rows_mut};
pub use track_list::*;

#[cfg(test)]
pub use menu::{row_items, sub_rows};

#[cfg(test)]
include!("widgets/tests.rs");


pub const CARD_W: f32 = 160.0;
pub const CARD_GAP: f32 = 12.0;
pub const SECTION_GAP: f32 = 32.0;

/// A quiet line of text with one button after it, centred on the line;
/// under it when the line is long for the width. `button`: its label, icon,
/// style and id. True when the button was clicked.
pub fn quiet_line(ui: &mut Ui, text: &str, button: Option<(&str, Option<Icon>, Style, &str)>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let text_w = ui
        .painter()
        .layout_no_wrap(text.to_owned(), TypeRole::BodyMedium.font_id(), s.on_surface_variant)
        .size()
        .x;
    // The button: its label, an icon and the padding.
    let button_w = button.map_or(0.0, |(label, icon, ..)| {
        let label_w = ui.painter().layout_no_wrap(label.to_owned(), TypeRole::LabelLarge.font_id(), s.primary).size().x;
        label_w + if icon.is_some() { 64.0 } else { 48.0 }
    });
    let mut clicked = false;
    let mut add_button = |ui: &mut Ui| {
        if let Some((label, icon, style, salt)) = button {
            let mut b = crate::components::Button::new(label).style(style).id_salt(salt);
            if let Some(i) = icon {
                b = b.icon(i);
            }
            clicked = ui.add(b).clicked();
        }
    };
    if text_w + button_w + 8.0 <= ui.available_width() {
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 40.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(TypeRole::BodyMedium.style().rich(text, s.on_surface_variant));
            add_button(ui);
        });
    } else {
        ui.label(TypeRole::BodyMedium.style().rich_para(text, s.on_surface_variant));
        add_button(ui);
    }
    clicked
}

/// «Перемешать» of a page's action row: a button, or in a narrow page an
/// icon button, so the row keeps to one line.
pub fn shuffle_member(ui: &Ui) -> crate::components::Member {
    if ui.available_width() < 640.0 {
        IconButton::new(Icon::Shuffle).size(Size::M).style(Style::Tonal).tooltip(i18n::common().shuffle).into()
    } else {
        crate::components::Button::new(i18n::common().shuffle).icon(Icon::Shuffle).size(Size::M).style(Style::Tonal).into()
    }
}

/// Title-large section header with an optional "Все →" text button.
/// Returns true when "Все" is clicked.
pub fn section_header(ui: &mut Ui, title: &str, all: bool) -> bool {
    section_header_with(ui, title, all.then_some((i18n::common().all, Icon::ArrowForward)))
}

/// [`section_header`] with its own text button on the right («Слушать
/// всё» with ▶). True when it is clicked.
pub fn section_header_with(ui: &mut Ui, title: &str, action: Option<(&str, Icon)>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let mut clicked = false;
    // A fixed-height row so the title and the 40 dp button share a centre
    // (`horizontal` would top-align the title in a shorter row).
    let row = egui::vec2(ui.available_width(), 40.0);
    ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.label(TypeRole::TitleLarge.style().rich(title, s.on_surface));
        if let Some((label, icon)) = action {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui
                    .add(
                        crate::components::Button::new(label)
                            .style(crate::components::Style::Text)
                            .icon(icon)
                            .id_salt(("all", title)),
                    )
                    .clicked();
            });
        }
    });
    ui.add_space(8.0);
    clicked
}

/// A horizontally scrolling row of cards.
pub fn card_row(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::ScrollArea::horizontal()
        .id_salt(id)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = CARD_GAP;
                add(ui);
            });
        });
}

/// `text` in `role` and `color`, wrapped to `width` in at most `rows` rows
/// (an ellipsis after the last).
pub fn wrapped(ui: &Ui, text: &str, role: crate::TypeStyle, color: Color32, width: f32, rows: usize) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), role.text_format(color));
    job.wrap = egui::text::TextWrapping {
        max_width: width.max(1.0),
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    crate::typography::layout(ui.painter(), job)
}

/// Lays out `n` rows of `row_h` one under another (the page's item
/// spacing between them) and draws only those that meet the viewport.
/// Returns where every row is.
pub fn virtual_rows(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    n: usize,
    row_h: f32,
    mut draw: impl FnMut(&mut Ui, usize),
) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let gap = ui.spacing().item_spacing.y;
    let step = row_h + gap;
    let width = ui.available_width();
    let (area, _) = ui.allocate_exact_size(vec2(width, n as f32 * step - gap), Sense::hover());
    let rect_of = |i: usize| Rect::from_min_size(pos2(area.left(), area.top() + i as f32 * step), vec2(width, row_h));
    let clip = ui.clip_rect();
    // A row above and below the viewport stay drawn (focus, menus, hover).
    let first = (((clip.top() - area.top()) / step).floor().max(0.0) as usize).saturating_sub(1);
    let last = ((((clip.bottom() - area.top()) / step).ceil().max(0.0)) as usize + 1).min(n);
    let base = Id::new(id);
    for i in first..last.max(first) {
        let mut child = ui.new_child(UiBuilder::new().max_rect(rect_of(i)).id_salt(base.with(i)));
        draw(&mut child, i);
    }
    (0..n).map(rect_of).collect()
}

/// A centred empty-state block: icon, title, body, optional button.
pub fn empty_state(ui: &mut Ui, icon: Icon, title: &str, body: &str, action: Option<&str>) -> bool {
    let s = Theme::get(ui.ctx()).scheme;
    let mut clicked = false;
    ui.add_space(48.0);
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::hover());
        shape_image(ui, None, "", r, MaterialShape::Cookie9Sided);
        icon.widget(false, s.on_tertiary_container, 40.0)
            .paint_at(ui, Rect::from_center_size(r.center(), vec2(40.0, 40.0)));
        ui.add_space(16.0);
        ui.label(TypeRole::TitleLarge.style().rich(title, s.on_surface));
        // An empty `body`: the title alone, no band under it.
        if !body.is_empty() {
            ui.add_space(4.0);
            ui.label(TypeRole::BodyMedium.style().rich_para(body, s.on_surface_variant));
        }
        if let Some(a) = action {
            ui.add_space(16.0);
            clicked = ui
                .add(crate::components::Button::new(a).style(crate::components::Style::Filled).id_salt(("empty", title)))
                .clicked();
        }
    });
    clicked
}

/// Number of grid columns that fit `width` with `card` cards.
pub fn columns(width: f32, card: f32) -> usize {
    crate::fit::Fit::at_least(width, CARD_GAP, card).count
}

/// The side of the big cover in a page's header (an album, a playlist, a
/// mix, an artist, a world): 232 beside a wide page, 160 otherwise.
pub fn header_cover(width: f32) -> f32 {
    if width > 720.0 { 232.0 } else { 160.0 }
}

/// The grid of cards at least `card` wide over `width`: how many stand in a
/// row and how wide each one is (whole points, the row filled).
pub fn grid_fit(width: f32, card: f32) -> (usize, f32) {
    let fit = crate::fit::Fit::at_least(width, CARD_GAP, card);
    (fit.count, fit.size.floor())
}

/// Which items of a long list are laid out this frame: those that meet
/// the viewport and one more on each side (`now`), and those laid out last
/// frame, so one that leaves the view still settles its hover and press as
/// it always did. The others only take their room. While a menu is up
/// every item is laid out (the one it belongs to stays under it).
pub struct Shown {
    now: std::ops::Range<usize>,
    last: std::ops::Range<usize>,
    all: bool,
}

impl Shown {
    pub fn of(ctx: &egui::Context, key: Id, now: std::ops::Range<usize>) -> Self {
        let last = ctx.data_mut(|d| {
            let last = d.get_temp::<std::ops::Range<usize>>(key);
            d.insert_temp(key, now.clone());
            last.unwrap_or_default()
        });
        Self { now, last, all: Menu::shown_lately(ctx) }
    }

    pub fn contains(&self, i: usize) -> bool {
        self.all || self.now.contains(&i) || self.last.contains(&i)
    }
}

/// The items `0..n` whose spans `start + i * step .. + size` meet `lo..hi`,
/// one more on each side.
pub fn span_range(n: usize, start: f32, step: f32, size: f32, lo: f32, hi: f32) -> std::ops::Range<usize> {
    if n == 0 || step <= 0.0 {
        return 0..0;
    }
    let first = ((lo - start - size) / step).ceil().max(0.0) as usize;
    let last = (((hi - start) / step).floor().max(-1.0) + 1.0) as usize;
    first.saturating_sub(1)..(last + 1).min(n)
}

/// Lays cards in a wrapping grid; cards get stretched to fill the row.
/// `height` is how tall an item's card is at a width: rows out of sight
/// take that room without being laid out.
pub fn grid<T>(
    ui: &mut Ui,
    items: &[T],
    min_card: f32,
    height: impl Fn(&T, f32) -> f32,
    mut draw: impl FnMut(&mut Ui, &T, f32) -> Option<Route>,
) -> Option<Route> {
    let (cols, card) = grid_fit(ui.available_width(), min_card);
    let key = ui.next_auto_id().with(("md3e-grid", items.len()));
    let rows: Vec<&[T]> = items.chunks(cols).collect();
    let heights: Vec<f32> = rows
        .iter()
        .map(|row| row.iter().map(|it| height(it, card)).fold(0.0, f32::max))
        .collect();
    // Where each row would be, and which ones meet the viewport.
    let gap = ui.spacing().item_spacing.y + CARD_GAP;
    let clip = ui.clip_rect();
    let mut y = ui.cursor().top();
    let (mut first, mut last) = (usize::MAX, 0);
    for (i, h) in heights.iter().enumerate() {
        if y + h >= clip.top() && y <= clip.bottom() {
            first = first.min(i);
            last = i + 1;
        }
        y += h + gap;
    }
    let now = if first < last { first.saturating_sub(1)..(last + 1).min(rows.len()) } else { 0..0 };
    let shown = Shown::of(ui.ctx(), key, now);
    let mut nav = None;
    for (i, row) in rows.into_iter().enumerate() {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            if shown.contains(i) {
                for it in row {
                    nav = draw(ui, it, card).or(nav.take());
                }
            } else {
                let n = row.len() as f32;
                ui.allocate_space(vec2(n * card + (n - 1.0) * CARD_GAP, heights[i]));
            }
        });
        ui.add_space(CARD_GAP);
    }
    nav
}

/// [`card_row`] of `n` items `item_w` wide, of which only those in sight
/// (see [`Shown`]) are laid out by `draw`.
pub fn card_row_virtual(ui: &mut Ui, id: &str, n: usize, item_w: f32, mut draw: impl FnMut(&mut Ui, usize)) {
    card_row(ui, id, |ui| {
        let start = ui.cursor().left();
        let clip = ui.clip_rect();
        let now = span_range(n, start, item_w + CARD_GAP, item_w, clip.left(), clip.right());
        let shown = Shown::of(ui.ctx(), ui.id().with(("md3e-row", id)), now);
        let mut skipped = 0usize;
        for i in 0..n {
            if shown.contains(i) {
                if skipped > 0 {
                    let k = skipped as f32;
                    ui.allocate_space(vec2(k * item_w + (k - 1.0) * CARD_GAP, 0.0));
                    skipped = 0;
                }
                draw(ui, i);
            } else {
                skipped += 1;
            }
        }
        if skipped > 0 {
            let k = skipped as f32;
            ui.allocate_space(vec2(k * item_w + (k - 1.0) * CARD_GAP, 0.0));
        }
    });
}

/// "Похоже, это один альбом": editions of one album the runtime found,
/// with a yes and a no (nothing merges on its own). `artist` narrows them
/// to one artist's page.
pub fn edition_offers(ui: &mut Ui, state: &ViewState, artist: Option<&str>, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    let offers = state.editions.iter().filter(|o| artist.is_none_or(|a| o.artist == a));
    for o in offers {
        crate::components::Card::filled().id_salt(("edition", o.id)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(TypeRole::TitleMedium.style().rich(i18n::pages().ed_title, s.on_surface));
            ui.add_space(4.0);
            let names = std::iter::once(&o.keep).chain(&o.others).map(|t| format!("«{t}»")).collect::<Vec<_>>().join(i18n::pages().ed_and);
            let mut what = i18n::pages::fmt_ed_what(&names, &o.artist);
            if o.same_songs > 0 {
                what.push_str(&i18n::pages::fmt_ed_same(o.same_songs));
            }
            if o.new_songs > 0 {
                what.push_str(&i18n::pages::fmt_ed_new(o.new_songs, &o.keep));
            }
            ui.label(TypeRole::BodyMedium.style().rich_para(what, s.on_surface_variant));
            ui.add_space(8.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 40.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui
                        .add(crate::components::Button::new(i18n::pages().ed_merge).style(Style::Filled).id_salt(("ed-merge", o.id)))
                        .on_hover_text(i18n::pages::fmt_ed_merge_tip(&o.keep))
                        .clicked()
                    {
                        out.push(Command::MergeEditions(o.id));
                    }
                    if ui
                        .add(crate::components::Button::new(i18n::pages().ed_keep).style(Style::Text).id_salt(("ed-keep", o.id)))
                        .on_hover_text(i18n::pages().ed_keep_tip)
                        .clicked()
                    {
                        out.push(Command::KeepEditionsApart(o.id));
                    }
                },
            );
        });
        ui.add_space(12.0);
    }
}

