//! The track's menu as a bottom sheet (Metrolist's song menu): the song at
//! the top with «Поделиться», three tiles for what is done most («Нравится»,
//! «Следующим», «В плейлист»), then the rest of the desktop's menu for this
//! track in one list. «В плейлист» turns the same sheet into the list of
//! playlists, with a new one at its end.

use egui::{Context, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{BottomSheet, IconButton, TextInputDialog, TextInputResult, anim};
use nikgob_md3e::screens::phone::{self, PhonePages, TrackAct, TrackMenuEntry};
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{Command, LIKED_PLAYLIST, LikeTarget, Route, ShareTarget, TrackRow, ViewState};

use crate::text;

const HEADER_H: f32 = 72.0;
const TILE_H: f32 = 64.0;
const ITEM_H: f32 = 52.0;

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
                    .filter(|e| !matches!(e.act, TrackAct::Next | TrackAct::Playlist))
                    .collect();
                self.entries = Some(e.clone());
                e
            }
        };
        let playlists: Vec<(nikgob_view::PlaylistId, String)> =
            state.library.playlists.iter().filter(|p| !p.pinned && p.id != LIKED_PLAYLIST).map(|p| (p.id, p.name.clone())).collect();
        let screen = ctx.content_rect();
        let rows = if self.picker { playlists.len() + 2 } else { entries.len() };
        let want = 36.0 + HEADER_H + if self.picker { 0.0 } else { TILE_H + 16.0 } + rows as f32 * ITEM_H + 24.0;
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
    // The song.
    let head = Rect::from_min_size(full.min, vec2(full.width(), HEADER_H));
    let cover = Rect::from_min_size(pos2(head.left(), head.center().y - 24.0), vec2(48.0, 48.0));
    let uri = track.cover.as_ref().map(|c| c.uri());
    phone::paint_cover(ui, cover, uri.as_deref(), 8.0);
    let tx = cover.right() + 16.0;
    let tw = head.right() - 48.0 - 8.0 - tx;
    let title = TypeRole::TitleMedium.style();
    let sub = TypeRole::BodyMedium.style();
    let ty = head.center().y - (title.line_height + sub.line_height) * 0.5;
    text::paint_line(ui, pos2(tx, ty), track.title.clone(), title, s.on_surface, tw);
    text::paint_line(ui, pos2(tx, ty + title.line_height), track.artist.clone(), sub, s.on_surface_variant, tw);
    let share = Rect::from_min_size(pos2(head.right() - 48.0, head.center().y - 24.0), vec2(48.0, 48.0));
    if ui.put(share, IconButton::new(Icon::Share).id_salt("phone-menu-share")).clicked() {
        act = Some(Act::Share);
    }
    let mut y = head.bottom();
    if picker {
        let mut items: Vec<(Icon, String, Act)> = vec![(Icon::Favorite, "Любимые".to_owned(), Act::Into(LIKED_PLAYLIST))];
        items.extend(playlists.iter().map(|(id, name)| (Icon::QueueMusic, name.clone(), Act::Into(*id))));
        items.push((Icon::Add, "Новый плейлист".to_owned(), Act::NewPlaylist));
        let list = Rect::from_min_max(pos2(full.left(), y), full.max);
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        crate::scroll("phone-menu-picker", false).show(&mut lui, |ui| {
            for (i, (icon, label, a)) in items.into_iter().enumerate() {
                if item(ui, ("picker", i), icon, &label) {
                    act = Some(a);
                }
            }
        });
        return act;
    }
    // The three tiles.
    let gap = 8.0;
    let tw = (full.width() - gap * 2.0) / 3.0;
    let liked = track.liked;
    let tiles = [
        (if liked { "Убрать" } else { "Нравится" }, Icon::Favorite, liked, Act::Like),
        ("Следующим", Icon::QueueMusic, false, Act::Next),
        ("В плейлист", Icon::PlaylistAdd, false, Act::Picker),
    ];
    for (i, (label, icon, on, a)) in tiles.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(full.left() + i as f32 * (tw + gap), y + 8.0), vec2(tw, TILE_H));
        let id = ui.id().with(("tile", i));
        let resp = ui.interact(r, id, Sense::click());
        let fill = if on { s.secondary_container } else { s.surface_container_high };
        paint::container(ui.painter(), r, Radii::all(16.0), fill, Stroke::NONE);
        let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
        paint::state_layer(ui.painter(), r, Radii::all(16.0), s.on_surface, layer);
        let on_color = if on { s.on_secondary_container } else { s.on_surface };
        icon.widget(on, on_color, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.center().x, r.top() + 22.0), vec2(24.0, 24.0)));
        let g = text::line(ui, label, TypeRole::LabelMedium.style(), on_color, r.width() - 8.0);
        let gw = g.size().x;
        ui.painter().galley(pos2(r.center().x - gw * 0.5, r.top() + 40.0), g, on_color);
        if resp.clicked() {
            act = Some(a);
        }
    }
    y += TILE_H + 16.0;
    let list = Rect::from_min_max(pos2(full.left(), y), full.max);
    let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list));
    crate::scroll("phone-menu-items", false).show(&mut lui, |ui| {
        for (i, e) in entries.iter().enumerate() {
            if item(ui, ("item", i), e.icon.unwrap_or(Icon::MoreHoriz), &e.label) {
                act = Some(Act::Item(e.act));
            }
        }
    });
    act
}

/// One row of the sheet's list: its icon and its words. True when tapped.
fn item(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, icon: Icon, label: &str) -> bool {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ITEM_H), Sense::click());
    let id = ui.id().with(salt);
    let layer = anim::fraction(ui.ctx(), id, if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
    paint::state_layer(ui.painter(), r, Radii::all(12.0), s.on_surface, layer);
    icon.widget(false, s.on_surface_variant, 24.0).paint_at(ui, Rect::from_center_size(pos2(r.left() + 12.0 + 12.0, r.center().y), vec2(24.0, 24.0)));
    let style = TypeRole::BodyLarge.style();
    text::paint_line(ui, pos2(r.left() + 56.0, r.center().y - style.line_height * 0.5), label, style, s.on_surface, r.width() - 64.0);
    resp.clicked()
}
