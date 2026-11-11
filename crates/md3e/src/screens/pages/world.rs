//! A world's page (home-v2 §15.3, «Открыть мир»): its name in the large
//! app bar, the header in the colours of the world's cover — the cover,
//! when it was heard, its facts, how far the user got and «Дослушать» / «Перемешать» — then
//! its mixes («Миксы»: «Лучшее», «Ещё не слышал», «В дорогу», its calm and
//! its lively side), its albums in order with how much of each was heard,
//! its new album of the services («Новое») and its liked tracks
//! («Любимое», from three). The mix and album cards and the track list are
//! the ones of Главная and the album pages; ← and the mouse's back button
//! return to Главная where it was scrolled.

use egui::{CornerRadius, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_view::{Command, Route, StreamPage, ViewState, WorldView};

use super::{PageState, cards, detail, home::Grid, widgets, worlds};
use crate::{
    Icon, Theme, TypeRole,
    components::{Button, LoadingIndicator, Radii, Size, Style, TooltipExt, paint},
    tokens::shape,
};

/// The header's cover (232, 160 on a narrow page) and the padding round it.
const PAD: f32 = 24.0;

fn cover_side(width: f32) -> f32 {
    if width > 720.0 { 232.0 } else { 160.0 }
}

/// «слышал 5 из 52», «слышал все 20», «не слушал».
pub fn album_tail(heard: u32, total: u32) -> String {
    match heard {
        0 => "не слушал".to_owned(),
        h if h >= total => format!("слышал все {total}"),
        h => format!("слышал {h} из {total}"),
    }
}

pub fn show(ui: &mut Ui, key: &str, state: &ViewState, pages: &mut PageState, out: &mut Vec<Command>) -> Option<Route> {
    let Some(v) = state.world.as_ref().filter(|w| w.world.key == key) else {
        return detail::with_bar(ui, "", pages, |ui, _| {
            ui.add(LoadingIndicator::new());
            None
        });
    };
    detail::with_bar(ui, &v.world.title, pages, |ui, pages| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let grid = Grid::new(ui.cursor().left(), ui.available_width());
        ui.add_space(8.0);
        header(ui, grid, v, state, out);
        let mut nav = None;
        if !v.mixes.is_empty() {
            ui.add_space(widgets::SECTION_GAP);
            nav = mixes(ui, grid, v, state, out);
        }
        ui.add_space(widgets::SECTION_GAP);
        nav = albums(ui, grid, v, out).or(nav);
        let wishlist = &state.downloads.wishlist;
        if let Some(a) = &v.new_album {
            ui.add_space(widgets::SECTION_GAP);
            section(ui, grid, "Новое");
            let w = grid.tile(grid.cards_per_line());
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), w + cards::ALBUM_TEXT_H), Sense::hover());
            let cell = Rect::from_min_size(pos2(grid.left, row.top()), vec2(w, w + cards::ALBUM_TEXT_H));
            let year = a.year.map(|y| y.to_string()).unwrap_or_default();
            let o = cards::album(ui, cell, &a.title, &a.artist, &year, a.cover_url.clone(), ("world-new", &a.remote));
            if o.play {
                out.push(Command::PlayStreamAlbum { id: a.remote.clone(), start: None, shuffle: false });
            } else if o.clicked {
                nav = Some(Route::Stream(StreamPage::Album(a.remote.clone())));
            }
            nav = super::streams::album_menus(&o.response, o.more.as_ref(), a, wishlist, out).or(nav);
        }
        if v.liked.len() >= 3 {
            ui.add_space(widgets::SECTION_GAP);
            section(ui, grid, "Любимое");
            let ids = v.liked.iter().map(|t| t.id).collect();
            let mut list = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(grid.left, ui.cursor().top()), vec2(grid.width, f32::INFINITY))).id_salt("world-liked"));
            let r = widgets::track_list(
                &mut list,
                "world-liked",
                &v.liked,
                widgets::Lead::Cover,
                &widgets::PlayFrom::Tracks(ids, v.world.title.clone()),
                state,
                pages,
                out,
            );
            let used = list.min_rect().height();
            ui.add_space(used);
            nav = r.or(nav);
        }
        ui.add_space(widgets::SECTION_GAP);
        nav
    })
}

/// «Миксы»: the mix cards of Главная, as many to a line as there (three on a
/// narrow page), their menus without what only Главная's have.
fn mixes(ui: &mut Ui, grid: Grid, v: &WorldView, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
    section(ui, grid, "Миксы");
    let per = grid.cards_per_line();
    let cw = grid.tile(per);
    let h = cw + cards::MIX_TEXT_H;
    let lines = v.mixes.len().div_ceil(per.max(1));
    let (block, _) = ui.allocate_exact_size(vec2(ui.available_width(), lines as f32 * (h + 24.0) - 24.0), Sense::hover());
    let mut nav = None;
    for (i, l) in v.mixes.iter().enumerate() {
        let cell = grid.cell(i, per, block.top(), h, 24.0);
        let (playing, opens) = (super::home::lane_playing(state, l), state.prefs.mix_click_opens_list);
        nav = cards::mix(ui, cell, l, false, false, playing, state.player.status, opens, Some(&v.world.key), out).or(nav);
    }
    nav
}

fn section(ui: &mut Ui, grid: Grid, title: &str) {
    let mut c = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(grid.left, ui.cursor().top()), vec2(grid.width, 48.0))).id_salt(("world-section", title)));
    widgets::section_header(&mut c, title, false);
    ui.add_space(48.0);
}

/// The header: a container r28 in the scheme of the world's cover.
fn header(ui: &mut Ui, grid: Grid, v: &WorldView, state: &ViewState, out: &mut Vec<Command>) {
    let ctx = ui.ctx().clone();
    let w = &v.world;
    let side = cover_side(grid.width);
    let h = side + 2.0 * PAD;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let card = Rect::from_min_size(pos2(grid.left, row.top()), vec2(grid.width, h));
    let target = worlds::scheme_of(&ctx, state, w);
    let mut s = target;
    s.on_surface = s.on_secondary_container;
    s.on_surface_variant = s.on_secondary_container;
    s.outline_variant = s.outline;
    let page_theme = Theme::get(&ctx);
    Theme::set(&ctx, page_theme.with_scheme(s));
    paint::container(ui.painter(), card, Radii::all(shape::EXTRA_LARGE), s.secondary_container, Stroke::NONE);
    let cover = Rect::from_min_size(card.min + vec2(PAD, PAD), vec2(side, side));
    worlds::paint_sharp(ui, cover, w, CornerRadius::same(shape::LARGE as u8), 1.0, (0.0, 0.0));
    let left = cover.right() + PAD;
    let text_w = (card.right() - PAD - left).max(80.0);
    let on = s.on_secondary_container;
    let mut c = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(pos2(left, cover.top()), pos2(card.right() - PAD, cover.bottom()))).id_salt(("world-header", &w.key)));
    let mut y = cover.top();
    // The name is the page's title right above; the header says what the
    // world is and how far the user got.
    let kicker = widgets::wrapped(&c, &w.kicker, TypeRole::LabelLarge.style(), on, text_w, 1);
    c.painter().galley(pos2(left, y), kicker, on);
    y += 28.0;
    let facts = widgets::wrapped(&c, &w.facts, TypeRole::BodyLarge.style(), on, text_w, 2);
    let fh = facts.size().y;
    c.painter().galley(pos2(left, y), facts, on);
    y += fh.max(24.0) + 8.0;
    worlds::progress_line(&mut c, pos2(left, y), text_w, w, &s, true);
    // The buttons under the progress, never below the cover.
    let by = (y + 24.0 + 32.0).min(cover.bottom() - Size::M.height());
    let mut b = c.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(pos2(left, by), vec2(text_w, Size::M.height())))
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt(("world-buttons", &w.key)),
    );
    b.spacing_mut().item_spacing.x = 8.0;
    if b
        .add(Button::new(w.start.label()).icon(Icon::PlayArrow).size(Size::M).style(Style::Filled).id_salt("world-primary"))
        .md3_tooltip(&w.primary_tip, None)
        .clicked()
    {
        out.push(worlds::play_primary(w));
    }
    if b.add(Button::new("Перемешать").icon(Icon::Shuffle).size(Size::M).style(Style::Outlined).id_salt("world-shuffle")).clicked() {
        out.push(Command::PlayTracks { tracks: w.shuffled.clone(), start: 0, context: format!("мира «{}»", w.title) });
    }
    Theme::set(&ctx, page_theme);
}

/// «Альбомы»: the album cards of Главная in order, year and «слышал 5 из
/// 52» under the title, a thin progress under them.
fn albums(ui: &mut Ui, grid: Grid, v: &WorldView, out: &mut Vec<Command>) -> Option<Route> {
    let s = Theme::get(ui.ctx()).scheme;
    section(ui, grid, if v.albums.len() == 1 { "Альбом" } else { "Альбомы" });
    let per = grid.cards_per_line();
    let cw = grid.tile(per);
    let h = cw + cards::ALBUM_TEXT_H + 12.0;
    let lines = v.albums.len().div_ceil(per.max(1));
    let (block, _) = ui.allocate_exact_size(vec2(ui.available_width(), lines as f32 * (h + 16.0) - 16.0), Sense::hover());
    let mut nav = None;
    for (i, a) in v.albums.iter().enumerate() {
        let cell = grid.cell(i, per, block.top(), h, 16.0);
        let year = a.card.year.map(|y| y.to_string()).unwrap_or_default();
        let card = Rect::from_min_size(cell.min, vec2(cell.width(), cell.width() + cards::ALBUM_TEXT_H));
        // «2017 · слышал 5 из 52», or the count alone without a year.
        let tail = album_tail(a.heard, a.total);
        let (line, tail) = if year.is_empty() { (tail.as_str(), "") } else { (year.as_str(), tail.as_str()) };
        let o = cards::album(ui, card, &a.card.title, line, tail, a.card.cover.as_ref().map(|c| c.uri()), ("world-album", a.card.id));
        if o.play {
            out.push(Command::PlayRelease { id: a.card.id, start: None, shuffle: false });
        } else if o.clicked {
            nav = Some(Route::Release(a.card.id));
        }
        nav = widgets::release_menus(&o.response, o.more.as_ref(), &a.card, out).or(nav);
        // The thin linear progress: indicator, a gap of 4, the track.
        let y = card.bottom() + 4.0;
        let f = if a.total == 0 { 0.0 } else { a.heard as f32 / a.total as f32 };
        let x = cell.left() + f * cell.width();
        let thick = 4.0;
        if f > 0.0 {
            ui.painter().line_segment([pos2(cell.left() + thick / 2.0, y), pos2((x - thick / 2.0).max(cell.left() + thick / 2.0), y)], Stroke::new(thick, s.primary));
        }
        let from = if f > 0.0 { x + 4.0 + thick / 2.0 } else { cell.left() + thick / 2.0 };
        if from < cell.right() - thick / 2.0 {
            ui.painter().line_segment([pos2(from, y), pos2(cell.right() - thick / 2.0, y)], Stroke::new(thick, s.secondary_container));
        }
    }
    nav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_says_how_much_of_it_was_heard() {
        assert_eq!(album_tail(5, 52), "слышал 5 из 52");
        assert_eq!(album_tail(20, 20), "слышал все 20");
        assert_eq!(album_tail(0, 30), "не слушал");
        assert_eq!(worlds::heard_line(36, 134), "Ты слышал 36 из 134");
        assert_eq!(worlds::heard_line(0, 160), "Ты ещё не слышал ни одного из 160");
    }
}
