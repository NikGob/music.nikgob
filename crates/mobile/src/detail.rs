//! The pages of an album, a playlist and an artist. An album or a playlist
//! has a 240 dp cover (radius 4), the title, the artist, the year and the
//! length centred under it, then ♥ or shuffle, ▶ and ⋮ as circles. The
//! tracks are flat 64 dp rows. An artist's picture stays centred, and its
//! actions are the same circles (♥, ▶, ⋮ with shuffle and radio). The
//! album's bar shows the title from the start; a playlist's bar shows none
//! until it scrolls.

use egui::{Color32, Id, Rect, Sense, Stroke, Ui, pos2, vec2};
use nikgob_md3e::components::paint::{self, Radii};
use nikgob_md3e::components::{Menu, MenuItem, anim};
use nikgob_md3e::screens::phone;
use nikgob_md3e::{Icon, Theme, TypeRole};
use nikgob_view::{
    ArtistId, Command, LIKED_PLAYLIST, LikeTarget, MeshPins, PlaylistId, RadioSeed, ReleaseDetail, ReleaseId, Route, ViewState,
};

use crate::parts::{self, Head, PageOut, Picture};
use crate::rows::{self, Look};
use crate::{PageFrame, text};

/// Scrolled this far past the picture the bar fills in.
const SOLID_AFTER: f32 = 24.0;

const COVER: f32 = 240.0;
const COVER_R: f32 = 4.0;
const UNDER_BAR: f32 = 8.0;
const UNDER_COVER: f32 = 20.0;
const TEXT_PAD: f32 = 32.0;
const ACTION_GAP: f32 = 16.0;
const SIDE_D: f32 = 48.0;
const PLAY_D: f32 = 72.0;
const PLAY_ICON: f32 = 32.0;
const SIDE_ICON: f32 = 24.0;


/// What a tap on a track of an album plays: the album from it, as the
/// page shows it (the user's songs, or the whole album).
pub fn album_play(d: &ReleaseDetail, start: Option<nikgob_view::TrackId>, shuffle: bool) -> Command {
    let id = d.card.id;
    match (d.partial, d.open_whole) {
        (false, _) => Command::PlayRelease { id, start, shuffle },
        (true, true) => Command::PlayReleaseWhole { id, start, shuffle },
        (true, false) => Command::PlayReleaseMine { id, start, shuffle },
    }
}

/// The width of ♥ or shuffle, ▶ and ⋮ with 16 dp gaps.
fn action_span() -> f32 {
    SIDE_D + ACTION_GAP + PLAY_D + ACTION_GAP + SIDE_D
}

/// Left of that row centred in `width`.
fn action_origin(width: f32) -> f32 {
    (width - action_span()) * 0.5
}

/// The scroll of a page and the bar over it. `title_always`: the title is
/// painted from the first frame (an album); otherwise it fades in once the
/// head has scrolled under the bar (a playlist shows none at the top).
fn page(ui: &mut Ui, frame: PageFrame, id: impl std::hash::Hash + std::fmt::Debug, title: &str, picture: f32, title_always: bool, mut body: impl FnMut(&mut Ui)) -> PageOut {
    let mut out = PageOut::default();
    let o = crate::scroll(id, frame.reset).show(ui, |ui| {
        body(ui);
        ui.add_space(24.0);
    });
    let solid = ((o.state.offset.y - picture - SOLID_AFTER) / 48.0).clamp(0.0, 1.0);
    if parts::top_bar(ui, frame.rect, frame.top, if title_always { "" } else { title }, solid, true) {
        out.back = true;
    }
    if title_always {
        paint_bar_title(ui, frame, title);
    }
    out
}

/// The album title in the bar, at x 56, from the first frame.
fn paint_bar_title(ui: &Ui, frame: PageFrame, title: &str) {
    if title.is_empty() {
        return;
    }
    let s = Theme::get(ui.ctx()).scheme;
    let row = Rect::from_min_size(pos2(frame.rect.left(), frame.rect.top() + frame.top), vec2(frame.rect.width(), parts::BAR_H));
    let x = row.left() + 56.0;
    let style = TypeRole::TitleLarge.style();
    text::paint_line(ui, pos2(x, row.center().y - style.line_height * 0.5), title, style, s.on_surface, row.right() - 16.0 - x);
}

struct Collection<'a> {
    uri: Option<&'a str>,
    title: &'a str,
    /// A playlist's title is 28 sp; an album's is 24.
    large_title: bool,
    artist: Option<&'a str>,
    year: Option<&'a str>,
    count: &'a str,
    side: Icon,
    side_on: bool,
    enabled: bool,
}

struct HeadOut {
    artist: bool,
    play: bool,
    side: bool,
    menu: Option<usize>,
}

/// The centred cover, the lines under it, and the three circles. `menu`
/// opens from ⋮.
fn collection_head(ui: &mut Ui, top: f32, id: Id, h: &Collection<'_>, menu: Menu) -> HeadOut {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let width = ui.available_width();
    ui.add_space(top + parts::BAR_H + UNDER_BAR);
    let (r, _) = ui.allocate_exact_size(vec2(width, COVER), Sense::hover());
    let pic = Rect::from_center_size(r.center(), vec2(COVER, COVER));
    paint::shadow(ui.painter(), pic, Radii::all(COVER_R), nikgob_md3e::Elevation::Level1);
    phone::paint_cover(ui, pic, h.uri, COVER_R);
    ui.add_space(UNDER_COVER);

    let title_style = if h.large_title { TypeRole::HeadlineMedium.emphasized() } else { TypeRole::HeadlineSmall.emphasized() };
    let max_w = width - TEXT_PAD * 2.0;
    let g = text::centred(ui, h.title, title_style, s.on_surface, max_w, 2);
    let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.center().x, r.top()), g, s.on_surface);

    let mut artist = false;
    if let Some(name) = h.artist {
        let g = text::centred(ui, name, TypeRole::TitleMedium.style(), s.primary, max_w, 1);
        let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
        let hit = Rect::from_center_size(r.center(), vec2(g.size().x + 16.0, r.height() + 8.0));
        artist = ui.interact(hit, id.with("artist"), Sense::click()).clicked();
        ui.painter().galley(pos2(r.center().x, r.top()), g, s.primary);
    }
    if let Some(year) = h.year {
        centred_line(ui, year, s.on_surface_variant, max_w);
    }
    centred_line(ui, h.count, s.on_surface_variant, max_w);

    ui.add_space(16.0);
    let row = circles(ui, id, h.side, h.side_on, h.enabled, menu);
    HeadOut { artist, play: row.play, side: row.side, menu: row.menu }
}

struct Circles {
    side: bool,
    play: bool,
    menu: Option<usize>,
}

/// ♥ or shuffle, ▶ and ⋮, centred: 48 / 72 / 48, 16 dp gaps. The side
/// circles sit on surface_container, ▶ on primary_container with a 32 dp
/// icon. 16 under the row.
fn circles(ui: &mut Ui, id: Id, side_icon: Icon, side_on: bool, enabled: bool, menu: Menu) -> Circles {
    let s = Theme::get(ui.ctx()).scheme;
    let width = ui.available_width();
    let (r, _) = ui.allocate_exact_size(vec2(width, PLAY_D), Sense::hover());
    let x0 = r.left() + action_origin(width);
    let side = Rect::from_min_size(pos2(x0, r.center().y - SIDE_D * 0.5), vec2(SIDE_D, SIDE_D));
    let play = Rect::from_min_size(pos2(x0 + SIDE_D + ACTION_GAP, r.top()), vec2(PLAY_D, PLAY_D));
    let more = Rect::from_min_size(pos2(play.right() + ACTION_GAP, r.center().y - SIDE_D * 0.5), vec2(SIDE_D, SIDE_D));
    let side_ink = if side_on { s.primary } else { s.on_surface };
    let side_resp = circle(ui, side, id.with("side"), side_icon, side_on, s.surface_container, side_ink, SIDE_ICON, enabled);
    let play_resp = circle(ui, play, id.with("play"), Icon::PlayArrow, true, s.primary_container, s.on_primary_container, PLAY_ICON, enabled);
    let more_resp = circle(ui, more, id.with("more"), Icon::MoreVert, false, s.surface_container, s.on_surface, SIDE_ICON, true);
    ui.add_space(16.0);
    Circles { side: enabled && side_resp.clicked(), play: enabled && play_resp.clicked(), menu: menu.align_center(true).attach(&more_resp) }
}

fn centred_line(ui: &mut Ui, line: &str, color: Color32, max_w: f32) {
    let width = ui.available_width();
    let g = text::centred(ui, line, TypeRole::BodyMedium.style(), color, max_w, 1);
    let (r, _) = ui.allocate_exact_size(vec2(width, g.size().y), Sense::hover());
    ui.painter().galley(pos2(r.center().x, r.top()), g, color);
}

fn circle(ui: &mut Ui, rect: Rect, id: Id, icon: Icon, filled: bool, fill: Color32, ink: Color32, icon_size: f32, enabled: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(rect, id, sense);
    let a = if enabled { 1.0 } else { 0.38 };
    let radii = Radii::all(rect.height() * 0.5);
    paint::container(ui.painter(), rect, radii, fill.gamma_multiply(a), Stroke::NONE);
    if enabled {
        let theme = Theme::get(ui.ctx());
        let layer = anim::fraction(ui.ctx(), id.with("l"), if resp.is_pointer_button_down_on() { 0.10 } else { 0.0 }, theme.motion.fast_effects);
        paint::state_layer(ui.painter(), rect, radii, ink, layer);
    }
    icon.widget(filled, ink.gamma_multiply(a), icon_size).paint_at(ui, Rect::from_center_size(rect.center(), vec2(icon_size, icon_size)));
    resp
}

/// The tracks of a page as flat 64 dp rows: a tap plays `play(i)`, a swipe
/// to the right plays the song next, a long press or ⋮ opens its menu.
#[allow(clippy::too_many_arguments)]
fn track_list(
    ui: &mut Ui,
    id: &str,
    tracks: &[nikgob_view::TrackRow],
    numbered: bool,
    here: &Route,
    state: &ViewState,
    play: impl Fn(usize) -> Command,
    out: &mut Vec<Command>,
    page: &mut PageOut,
) {
    rows::list(ui, id, tracks.len(), rows::ROW_H, |ui, i, r| {
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

pub fn album(ui: &mut Ui, frame: PageFrame, id: ReleaseId, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
    let d = state.release.as_ref().filter(|d| d.card.id == id);
    let mut inner = PageOut::default();
    let title = d.map_or("", |d| d.card.title.as_str());
    let mut bar = page(ui, frame, ("phone-album", id), title, COVER, true, |ui| {
        let Some(d) = d else {
            ui.add_space(frame.top + parts::BAR_H);
            parts::loading(ui);
            return;
        };
        let c = &d.card;
        let uri = c.cover.as_ref().map(|c| c.uri());
        let year = c.year.map(|y| y.to_string());
        let count = format!("{} • {}", text::tracks(d.tracks.len() as u64), text::total(d.total_duration));
        let menu = Menu::new(("album-menu", id))
            .item(MenuItem::new("Перемешать").icon(Icon::Shuffle))
            .item(MenuItem::new("Следующим").icon(Icon::QueueMusic));
        let head = collection_head(
            ui,
            frame.top,
            Id::new(("album-head", id)),
            &Collection {
                uri: uri.as_deref(),
                title: &c.title,
                large_title: false,
                artist: (!c.artist.is_empty()).then_some(c.artist.as_str()),
                year: year.as_deref(),
                count: &count,
                side: Icon::Favorite,
                side_on: d.liked,
                enabled: true,
            },
            menu,
        );
        if head.artist
            && let Some(a) = c.artist_id()
        {
            inner.nav = Some(Route::Artist(a));
        }
        if head.play {
            out.push(album_play(d, None, false));
        }
        if head.side {
            out.push(Command::Like(LikeTarget::Release(id), !d.liked));
        }
        match head.menu {
            Some(0) => out.push(album_play(d, None, true)),
            Some(1) => out.push(Command::PlayReleaseNext(id)),
            _ => {}
        }
        let here = Route::Release(id);
        track_list(ui, "album-tracks", &d.tracks, true, &here, state, |i| album_play(d, Some(d.tracks[i].id), false), out, &mut inner);
    });
    bar.nav = inner.nav;
    bar.menu = inner.menu;
    bar
}

/// The playlist is among what this phone keeps (its «Избранное»).
fn pinned(pins: &MeshPins, id: PlaylistId) -> bool {
    if id == LIKED_PLAYLIST { pins.liked } else { pins.playlists.contains(&id) }
}

/// The pins with the playlist kept or let go.
pub fn toggle_pin(pins: &MeshPins, id: PlaylistId) -> MeshPins {
    let mut p = pins.clone();
    if id == LIKED_PLAYLIST {
        p.liked = !p.liked;
    } else if let Some(i) = p.playlists.iter().position(|x| *x == id) {
        p.playlists.remove(i);
    } else {
        p.playlists.push(id);
    }
    p
}

pub fn playlist(ui: &mut Ui, frame: PageFrame, id: PlaylistId, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
    let d = state.playlist.as_ref().filter(|d| d.card.id == id);
    let mut inner = PageOut::default();
    let title = d.map_or("", |d| d.card.name.as_str());
    let mut bar = page(ui, frame, ("phone-playlist", id), title, COVER, false, |ui| {
        let Some(d) = d else {
            ui.add_space(frame.top + parts::BAR_H);
            parts::loading(ui);
            return;
        };
        let uri = d.card.cover.as_ref().map(|c| c.uri());
        let count = format!("{} • {}", text::tracks(d.tracks.len() as u64), text::total(d.total_duration));
        let empty = d.tracks.is_empty();
        let pins = &state.mesh.this.pins;
        let kept = pinned(pins, id);
        let mut menu = Menu::new(("pl-menu", id)).item(MenuItem::new("Следующим").icon(Icon::QueueMusic).enabled(!empty));
        if state.mesh.paired {
            menu = menu.item(MenuItem::new("Избранное").icon(if kept { Icon::DownloadDone } else { Icon::Download }).selected(kept));
        }
        let head = collection_head(
            ui,
            frame.top,
            Id::new(("pl-head", id)),
            &Collection {
                uri: uri.as_deref(),
                title: &d.card.name,
                large_title: true,
                artist: None,
                year: None,
                count: &count,
                side: Icon::Shuffle,
                side_on: false,
                enabled: !empty,
            },
            menu,
        );
        if head.play {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: false });
        }
        if head.side {
            out.push(Command::PlayPlaylist { id, start: None, shuffle: true });
        }
        match head.menu {
            Some(0) if !empty => out.push(Command::PlayPlaylistNext(id)),
            Some(1) => out.push(Command::MeshSetPins { node_id: state.mesh.this.id.clone(), pins: toggle_pin(pins, id) }),
            _ => {}
        }
        let here = Route::Playlist(id);
        track_list(ui, "pl-tracks", &d.tracks, false, &here, state, |i| Command::PlayPlaylist { id, start: Some(d.tracks[i].id), shuffle: false }, out, &mut inner);
    });
    bar.nav = inner.nav;
    bar.menu = inner.menu;
    bar
}

pub fn artist(ui: &mut Ui, frame: PageFrame, id: ArtistId, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
    let d = state.artist.as_ref().filter(|d| d.card.id == id);
    let mut inner = PageOut::default();
    let title = d.map_or("", |d| d.card.name.as_str());
    let mut bar = page(ui, frame, ("phone-artist", id), title, 168.0, false, |ui| {
        let Some(d) = d else {
            ui.add_space(frame.top + parts::BAR_H);
            parts::loading(ui);
            return;
        };
        let c = &d.card;
        let head = Head {
            uri: c.image.as_ref().map(|i| i.uri()),
            picture: Picture::Round,
            kicker: None,
            title: &c.name,
            artist: c.latin.as_deref(),
            numbers: (c.track_count > 0).then(|| text::tracks(u64::from(c.track_count))),
        };
        parts::head(ui, frame.top, &head);
        let menu = Menu::new(("ar-menu", id))
            .item(MenuItem::new("Перемешать").icon(Icon::Shuffle))
            .item(MenuItem::new("Радио исполнителя").icon(Icon::Radio));
        let row = circles(ui, Id::new(("ar-circles", id)), Icon::Favorite, d.liked, true, menu);
        if row.play {
            out.push(Command::PlayArtist { id, shuffle: false });
        }
        if row.side {
            out.push(Command::Like(LikeTarget::Artist(id), !d.liked));
        }
        match row.menu {
            Some(0) => out.push(Command::PlayArtist { id, shuffle: true }),
            Some(1) => out.push(Command::StartRadio { seed: RadioSeed::Artist(id) }),
            _ => {}
        }
        let here = Route::Artist(id);
        if !d.top_tracks.is_empty() {
            parts::section_head(ui, "Популярные треки");
            let top = &d.top_tracks[..d.top_tracks.len().min(5)];
            let ids: Vec<nikgob_view::TrackId> = top.iter().map(|t| t.id).collect();
            let context = format!("исполнителя «{}»", c.name);
            track_list(ui, "ar-top", top, false, &here, state, |i| Command::PlayTracks { tracks: ids.clone(), start: i, context: context.clone() }, out, &mut inner);
        }
        for (name, releases) in [("Альбомы", &d.albums), ("Синглы и EP", &d.singles_eps)] {
            if releases.is_empty() {
                continue;
            }
            parts::section_head(ui, name);
            parts::grid2(ui, name, releases.len(), |ui, i, r| {
                let rel = &releases[i];
                let uri = rel.cover.as_ref().map(|c| c.uri());
                let sub = rel.year.map(|y| y.to_string()).unwrap_or_default();
                if parts::grid_card(ui, r, Id::new(("ar-rel", rel.id)), uri.as_deref(), &rel.title, &sub, Picture::Square).clicked() {
                    inner.nav = Some(Route::Release(rel.id));
                }
            });
        }
    });
    bar.nav = inner.nav;
    bar.menu = inner.menu;
    bar
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::model::Uuid;

    #[test]
    fn a_pin_comes_and_goes() {
        let a = PlaylistId(Uuid::from_u128(7));
        let p = MeshPins::default();
        let on = toggle_pin(&p, a);
        assert!(pinned(&on, a));
        assert!(!pinned(&toggle_pin(&on, a), a));
        let liked = toggle_pin(&p, LIKED_PLAYLIST);
        assert!(liked.liked && pinned(&liked, LIKED_PLAYLIST));
    }

    #[test]
    fn the_action_circles_centre_on_a_phone() {
        assert_eq!(COVER, 240.0);
        assert_eq!(action_span(), 200.0);
        assert_eq!(action_origin(360.0), 80.0);
        assert_eq!(action_origin(384.0), 92.0);
        assert_eq!(action_origin(412.0), 106.0);
    }
}
