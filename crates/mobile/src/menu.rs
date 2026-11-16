//! The track's menu as a bottom sheet (Metrolist's song menu): the song at
//! the top with ♥ and «Поделиться» as large round buttons, three connected
//! tiles for what is done most («Следующим», «В очередь», «В плейлист»),
//! then the rest of the desktop's menu for this track as one grouped list.
//! «В плейлист» turns the same sheet into the list of playlists, with a new
//! one at its end.

use egui::{Context, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{BottomSheet, TextInputDialog, TextInputResult, anim};
use nikgob_md3e::screens::phone::{self, PhonePages, TrackAct, TrackMenuEntry};
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Command, LIKED_PLAYLIST, LikeTarget, Route, ShareTarget, TrackRow, ViewState};

use crate::text;

const HEADER_H: f32 = 88.0;
const TILE_H: f32 = 86.0;
const ITEM_H: f32 = 56.0;
/// The gap between the members of a connected row or a grouped list.
const JOIN: f32 = 2.0;
/// Their ends' corners and their inner ones.
const END: f32 = 28.0;
const INNER: f32 = 6.0;

#[derive(Default)]
pub struct TrackMenu {
    open: bool,
    track: Option<TrackRow>,
    here: Option<Route>,
    picker: bool,
    /// «Новый плейлист»: the dialog is up, with what is typed.
    naming: bool,
    name: String,
    /// The items, made once each time it opens.
    entries: Option<Vec<TrackMenuEntry>>,
}

impl TrackMenu {
    /// Opens the menu of `track` shown on `here`.
    pub fn open(&mut self, track: TrackRow, here: Route) {
        self.track = Some(track);
        self.here = Some(here);
        self.open = true;
        self.picker = false;
        self.entries = None;
    }

    pub fn is_open(&self) -> bool {
        self.open || self.naming
    }

    pub fn close(&mut self) {
        self.open = false;
        self.naming = false;
    }

    /// Draws the sheet; returns a page an item leads to.
    pub fn show(&mut self, ctx: &Context, shell: &mut PhonePages, state: &ViewState, out: &mut Vec<Command>) -> Option<Route> {
        let mut nav = None;
        if self.naming {
            let mut open = true;
            let r = TextInputDialog::new("phone-new-playlist", "Новый плейлист", "Название")
                .icon(Icon::PlaylistAdd)
                .buttons("Создать", "Отмена")
                .validate(|t| t.trim().is_empty().then(|| "Введи название".to_owned()))
                .show(ctx, &mut open, &mut self.name);
            if matches!(r, Some(TextInputResult::Confirmed))
                && let Some(t) = &self.track
            {
                out.push(Command::SaveTracksAsPlaylist { name: self.name.trim().to_owned(), tracks: vec![t.id] });
            }
            if r.is_some() || !open {
                self.naming = false;
            }
        }
        let Some(track) = self.track.clone() else { return None };
        let here = self.here.clone().unwrap_or(Route::Home);
        let entries: Vec<TrackMenuEntry> = match &self.entries {
            Some(e) => e.clone(),
            None => {
                let e: Vec<TrackMenuEntry> = shell
                    .track_menu(ctx, &track, &here, state)
                    .into_iter()
                    .filter(|e| !matches!(e.act, TrackAct::Next | TrackAct::Queue | TrackAct::Playlist | TrackAct::Share))
                    .collect();
                self.entries = Some(e.clone());
                e
            }
        };
        let playlists: Vec<(nikgob_view::PlaylistId, String)> =
            state.library.playlists.iter().filter(|p| !p.pinned && p.id != LIKED_PLAYLIST).map(|p| (p.id, p.name.clone())).collect();
        let screen = ctx.content_rect();
        let rows = if self.picker { playlists.len() + 2 } else { entries.len() };
        let want = 36.0 + HEADER_H + if self.picker { 0.0 } else { TILE_H + 24.0 } + rows as f32 * (ITEM_H + JOIN) + 24.0;
        let height = want.min(screen.height() * 0.88);
        let mut act: Option<Act> = None;
        BottomSheet::new("phone-track-menu").height(height).max_width(screen.width()).show(ctx, &mut self.open, |ui| {
            act = body(ui, &track, &entries, &playlists, self.picker);
        });
        match act {
            Some(Act::Like) => {
                out.push(Command::Like(LikeTarget::Track(track.id), !track.liked));
                self.open = false;
            }
            Some(Act::Next) => {
                out.push(Command::PlayNext(vec![track.id]));
                self.open = false;
            }
            Some(Act::Queue) => {
                out.push(Command::Enqueue(vec![track.id]));
                self.open = false;
            }
            Some(Act::Picker) => self.picker = true,
            Some(Act::Share) => {
                out.push(Command::Share(ShareTarget::Track(track.id)));
                self.open = false;
            }
            Some(Act::Into(id)) => {
                out.push(Command::AddToPlaylist(id, vec![track.id]));
                self.open = false;
            }
            Some(Act::NewPlaylist) => {
                self.name.clear();
                self.naming = true;
                self.open = false;
            }
            Some(Act::Item(a)) => {
                nav = shell.apply(ctx, a, &track, &here, state, out);
                self.open = false;
            }
            None => {}
        }
        nav
    }
}

enum Act {
    Like,
    Next,
    Queue,
    Picker,
    Share,
    Into(nikgob_view::PlaylistId),
    NewPlaylist,
    Item(TrackAct),
}

fn body(ui: &mut Ui, track: &TrackRow, entries: &[TrackMenuEntry], playlists: &[(nikgob_view::PlaylistId, String)], picker: bool) -> Option<Act> {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let full = ui.max_rect();
    let mut act = None;
    // The song, with ♥ and «Поделиться».
    let head = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
    let cover = Rect::from_min_size(pos2(head.left(), head.center().y - 32.0), vec2(64.0, 64.0));
    let uri = track.cover.as_ref().map(|c| c.uri());
    phone::paint_cover(ui, cover, uri.as_deref(), 8.0);
    let share = Rect::from_min_size(pos2(head.right() - 56.0, head.center().y - 28.0), vec2(56.0, 56.0));
    let like = share.translate(vec2(-60.0, 0.0));
    let tx = cover.right() + 16.0;
    let tw = like.left() - 8.0 - tx;
    let title = TypeRole::TitleMedium.style();
    let sub = TypeRole::BodyMedium.style();
    let ty = head.center().y - (title.line_height + sub.line_height) * 0.5;
    text::paint_line(ui, pos2(tx, ty), track.title.clone(), title, s.on_surface, tw);
    text::paint_line(ui, pos2(tx, ty + title.line_height), track.artist.clone(), sub, s.on_surface_variant, tw);
    let (like_id, share_id) = (ui.id().with("like"), ui.id().with("share"));
    if big_button(ui, like, like_id, Icon::Favorite, track.liked, true) {
        act = Some(Act::Like);
    }
    if big_button(ui, share, share_id, Icon::Share, false, false) {
        act = Some(Act::Share);
    }
    let mut y = head.bottom();
    if picker {
        let mut items: Vec<(Icon, String, Act)> = vec![(Icon::Favorite, "Любимые".to_owned(), Act::Into(LIKED_PLAYLIST))];
        items.extend(playlists.iter().map(|(id, name)| (Icon::QueueMusic, name.clone(), Act::Into(*id))));
        items.push((Icon::Add, "Новый плейлист".to_owned(), Act::NewPlaylist));
        let list = Rect::from_min_max(pos2(full.left(), y), full.max);
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        let n = items.len();
        crate::scroll("phone-menu-picker", false).show(&mut lui, |ui| {
            for (i, (icon, label, a)) in items.into_iter().enumerate() {
                if item(ui, ("picker", i), i, n, icon, &label) {
                    act = Some(a);
                }
            }
        });
        return act;
    }
    // The three connected tiles.
    let tiles = [("Следующим", Icon::QueueMusic, Act::Next), ("В очередь", Icon::Add, Act::Queue), ("В плейлист", Icon::PlaylistAdd, Act::Picker)];
    let tw = (full.width() - JOIN * 2.0) / 3.0;
    for (i, (label, icon, a)) in tiles.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(full.left() + i as f32 * (tw + JOIN), y + 12.0), vec2(tw, TILE_H));
        let id = ui.id().with(("tile", i));
        let resp = ui.interact(r, id, Sense::click());
        let radii = Radii::sides(if i == 0 { END } else { INNER }, if i == 2 { END } else { INNER });
        paint::container(ui.painter(), r, radii, s.secondary_container, Stroke::NONE);
        let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
        paint::state_layer(ui.painter(), r, radii, s.on_secondary_container, layer);
        let style = TypeRole::TitleSmall.style();
        let block = 28.0 + 8.0 + style.line_height;
        let top = r.center().y - block * 0.5;
        icon.widget(false, s.on_secondary_container, 28.0).paint_at(ui, Rect::from_center_size(pos2(r.center().x, top + 14.0), vec2(28.0, 28.0)));
        let g = text::line(ui, label, style, s.on_secondary_container, r.width() - 16.0);
        let gw = g.size().x;
        ui.painter().galley(pos2(r.center().x - gw * 0.5, top + 36.0), g, s.on_secondary_container);
        if resp.clicked() {
            act = Some(a);
        }
    }
    y += 12.0 + TILE_H + 12.0;
    let list = Rect::from_min_max(pos2(full.left(), y), full.max);
    let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list));
    let n = entries.len();
    crate::scroll("phone-menu-items", false).show(&mut lui, |ui| {
        for (i, e) in entries.iter().enumerate() {
            if item(ui, ("item", i), i, n, e.icon.unwrap_or(Icon::MoreHoriz), &e.label) {
                act = Some(Act::Item(e.act));
            }
        }
    });
    act
}

/// A 56 dp button of the head: a circle while it toggles (on: filled
/// icon), a rounded square otherwise. True when tapped.
fn big_button(ui: &mut Ui, rect: Rect, id: egui::Id, icon: Icon, on: bool, toggle: bool) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let resp = ui.interact(rect, id, Sense::click());
    let radii = Radii::all(if toggle { rect.width() * 0.5 } else { 16.0 });
    let (fill, ink) = if toggle { (s.surface_container_high, if on { s.primary } else { s.on_surface_variant }) } else { (s.secondary_container, s.on_secondary_container) };
    paint::container(ui.painter(), rect, radii, fill, Stroke::NONE);
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), rect, radii, ink, layer);
    icon.widget(on, ink, 28.0).paint_at(ui, Rect::from_center_size(rect.center(), vec2(28.0, 28.0)));
    resp.clicked()
}

/// The corners of row `i` of a grouped list of `n` (Metrolist's menu).
fn item_radii(i: usize, n: usize) -> Radii {
    let top = if i == 0 { END } else { INNER };
    let bottom = if i + 1 >= n { END } else { INNER };
    Radii([top, top, bottom, bottom])
}

/// One row of the sheet's grouped list: its icon and its words on the
/// surface container. True when tapped.
fn item(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, i: usize, n: usize, icon: Icon, label: &str) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), ITEM_H + JOIN), Sense::hover());
    let r = Rect::from_min_size(slot.min, vec2(slot.width(), ITEM_H));
    let id = ui.id().with(salt);
    let resp = ui.interact(r, id, Sense::click());
    let radii = item_radii(i, n);
    paint::container(ui.painter(), r, radii, s.surface_container_high, Stroke::NONE);
    let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), r, radii, s.on_surface, layer);
    icon.widget(false, s.on_surface, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.left() + 20.0 + 12.0, r.center().y), vec2(24.0, 24.0)));
    let style = TypeRole::TitleMedium.style();
    text::paint_line(ui, pos2(r.left() + 60.0, r.center().y - style.line_height * 0.5), label, style, s.on_surface, r.width() - 80.0);
    resp.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_round_at_its_ends() {
        assert_eq!(item_radii(0, 4).0, [END, END, INNER, INNER]);
        assert_eq!(item_radii(3, 4).0, [INNER, INNER, END, END]);
        assert_eq!(item_radii(0, 1).0, [END; 4]);
    }
}
