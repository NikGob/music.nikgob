//! The pages of an album, a playlist and an artist (Metrolist's album,
//! playlist and artist screens): the bar with ← over a centred head (the
//! picture, the kicker, the title, the artist, the numbers), the row of
//! actions, then the tracks; an artist's page has its popular tracks, its
//! albums and its singles under the head. The bar turns solid with the
//! title once the head has scrolled under it.

use egui::{Id, Ui};
use nikgob_md3e::components::{Button, IconButton, Size, Style};
use nikgob_md3e::Icon;
use nikgob_view::{
    ArtistId, Command, LIKED_PLAYLIST, LikeTarget, MeshPins, PlaylistId, RadioSeed, ReleaseDetail, ReleaseId, ReleaseType, Route, ViewState,
};

use crate::parts::{self, Head, PageOut, Picture};
use crate::{PageFrame, text};

/// Scrolled this far the bar shows the title.
const SOLID_FROM: f32 = 220.0;

/// How an album's word reads in the kicker.
pub fn kind_word(t: ReleaseType) -> &'static str {
    match t {
        ReleaseType::Album => "Альбом",
        ReleaseType::Single => "Сингл",
        ReleaseType::Ep => "EP",
        ReleaseType::Compilation => "Сборник",
        ReleaseType::Other => "Релиз",
    }
}

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

/// The scroll of a page and the bar over it; `body` draws the page.
fn page(ui: &mut Ui, frame: PageFrame, id: impl std::hash::Hash + std::fmt::Debug, title: &str, mut body: impl FnMut(&mut Ui)) -> PageOut {
    let mut out = PageOut::default();
    let o = crate::scroll(id, frame.reset).show(ui, |ui| {
        body(ui);
        ui.add_space(24.0);
    });
    let solid = ((o.state.offset.y - SOLID_FROM) / 48.0).clamp(0.0, 1.0);
    if parts::top_bar(ui, frame.rect, frame.top, title, solid, true) {
        out.back = true;
    }
    out
}

pub fn album(ui: &mut Ui, frame: PageFrame, id: ReleaseId, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
    let d = state.release.as_ref().filter(|d| d.card.id == id);
    let mut inner = PageOut::default();
    let title = d.map_or("", |d| d.card.title.as_str());
    let mut bar = page(ui, frame, ("phone-album", id), title, |ui| {
        let Some(d) = d else {
            ui.add_space(frame.top + parts::BAR_H);
            parts::loading(ui);
            return;
        };
        let c = &d.card;
        let kicker = match c.year {
            Some(y) => format!("{} · {y}", kind_word(c.release_type)),
            None => kind_word(c.release_type).to_owned(),
        };
        let numbers = format!("{} · {}", text::tracks(d.tracks.len() as u64), text::total(d.total_duration));
        let head = Head {
            uri: c.cover.as_ref().map(|c| c.uri()),
            picture: Picture::Square,
            kicker: Some(kicker),
            title: &c.title,
            artist: Some(&c.artist),
            numbers: Some(numbers),
        };
        if parts::head(ui, frame.top, &head)
            && let Some(a) = c.artist_id()
        {
            inner.nav = Some(Route::Artist(a));
        }
        parts::actions(ui, 300.0, |ui| {
            if ui.add(Button::new("Слушать").icon(Icon::PlayArrow).style(Style::Filled).size(Size::M).id_salt("album-play")).clicked() {
                out.push(album_play(d, None, false));
            }
            if ui.add(IconButton::new(Icon::Shuffle).style(Style::Tonal).size(Size::M).id_salt("album-shuffle")).clicked() {
                out.push(album_play(d, None, true));
            }
            if ui.add(IconButton::new(Icon::Favorite).selected(d.liked).size(Size::M).id_salt("album-like")).clicked() {
                out.push(Command::Like(LikeTarget::Release(id), !d.liked));
            }
            if ui.add(IconButton::new(Icon::QueueMusic).size(Size::M).id_salt("album-next")).clicked() {
                out.push(Command::PlayReleaseNext(id));
            }
        });
        let here = Route::Release(id);
        parts::tracks(ui, "album-tracks", &d.tracks, true, &here, state, |i| album_play(d, Some(d.tracks[i].id), false), out, &mut inner);
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
    let mut bar = page(ui, frame, ("phone-playlist", id), title, |ui| {
        let Some(d) = d else {
            ui.add_space(frame.top + parts::BAR_H);
            parts::loading(ui);
            return;
        };
        let numbers = format!("{} · {}", text::tracks(d.tracks.len() as u64), text::total(d.total_duration));
        let head = Head {
            uri: d.card.cover.as_ref().map(|c| c.uri()),
            picture: Picture::Square,
            kicker: Some("Плейлист".to_owned()),
            title: &d.card.name,
            artist: None,
            numbers: Some(numbers),
        };
        parts::head(ui, frame.top, &head);
        let empty = d.tracks.is_empty();
        parts::actions(ui, 300.0, |ui| {
            if ui.add(Button::new("Слушать").icon(Icon::PlayArrow).style(Style::Filled).size(Size::M).enabled(!empty).id_salt("pl-play")).clicked() {
                out.push(Command::PlayPlaylist { id, start: None, shuffle: false });
            }
            if ui.add(IconButton::new(Icon::Shuffle).style(Style::Tonal).size(Size::M).enabled(!empty).id_salt("pl-shuffle")).clicked() {
                out.push(Command::PlayPlaylist { id, start: None, shuffle: true });
            }
            if ui.add(IconButton::new(Icon::QueueMusic).size(Size::M).enabled(!empty).id_salt("pl-next")).clicked() {
                out.push(Command::PlayPlaylistNext(id));
            }
            // Kept on this phone with its «Избранное», while it is one of
            // the owner's devices.
            if state.mesh.paired {
                let pins = &state.mesh.this.pins;
                let on = pinned(pins, id);
                let icon = if on { Icon::DownloadDone } else { Icon::Download };
                if ui.add(IconButton::new(icon).selected(on).size(Size::M).id_salt("pl-pin")).clicked() {
                    out.push(Command::MeshSetPins { node_id: state.mesh.this.id.clone(), pins: toggle_pin(pins, id) });
                }
            }
        });
        let here = Route::Playlist(id);
        parts::tracks(
            ui,
            "pl-tracks",
            &d.tracks,
            false,
            &here,
            state,
            |i| Command::PlayPlaylist { id, start: Some(d.tracks[i].id), shuffle: false },
            out,
            &mut inner,
        );
    });
    bar.nav = inner.nav;
    bar.menu = inner.menu;
    bar
}

pub fn artist(ui: &mut Ui, frame: PageFrame, id: ArtistId, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
    let d = state.artist.as_ref().filter(|d| d.card.id == id);
    let mut inner = PageOut::default();
    let title = d.map_or("", |d| d.card.name.as_str());
    let mut bar = page(ui, frame, ("phone-artist", id), title, |ui| {
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
        parts::actions(ui, 300.0, |ui| {
            if ui.add(Button::new("Слушать").icon(Icon::PlayArrow).style(Style::Filled).size(Size::M).id_salt("ar-play")).clicked() {
                out.push(Command::PlayArtist { id, shuffle: false });
            }
            if ui.add(IconButton::new(Icon::Shuffle).style(Style::Tonal).size(Size::M).id_salt("ar-shuffle")).clicked() {
                out.push(Command::PlayArtist { id, shuffle: true });
            }
            if ui.add(IconButton::new(Icon::Radio).size(Size::M).id_salt("ar-radio")).clicked() {
                out.push(Command::StartRadio { seed: RadioSeed::Artist(id) });
            }
            if ui.add(IconButton::new(Icon::Favorite).selected(d.liked).size(Size::M).id_salt("ar-like")).clicked() {
                out.push(Command::Like(LikeTarget::Artist(id), !d.liked));
            }
        });
        let here = Route::Artist(id);
        if !d.top_tracks.is_empty() {
            parts::section(ui, "Популярные треки");
            let top = &d.top_tracks[..d.top_tracks.len().min(5)];
            let ids: Vec<nikgob_view::TrackId> = top.iter().map(|t| t.id).collect();
            let context = format!("исполнителя «{}»", c.name);
            parts::tracks(
                ui,
                "ar-top",
                top,
                false,
                &here,
                state,
                |i| Command::PlayTracks { tracks: ids.clone(), start: i, context: context.clone() },
                out,
                &mut inner,
            );
        }
        for (name, releases) in [("Альбомы", &d.albums), ("Синглы и EP", &d.singles_eps)] {
            if releases.is_empty() {
                continue;
            }
            parts::section(ui, name);
            parts::grid(ui, releases.len(), 2, |ui, i, r| {
                let rel = &releases[i];
                let uri = rel.cover.as_ref().map(|c| c.uri());
                let sub = rel.year.map(|y| y.to_string()).unwrap_or_default();
                if parts::card(ui, r, Id::new(("ar-rel", rel.id)), uri.as_deref(), &rel.title, &sub, Picture::Square).clicked() {
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
    fn the_kicker_names_the_kind() {
        assert_eq!(kind_word(ReleaseType::Ep), "EP");
        assert_eq!(kind_word(ReleaseType::Album), "Альбом");
    }
}
