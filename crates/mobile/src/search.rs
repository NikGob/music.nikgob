//! Поиск, opened by the search bar of Главная or Библиотека over the page:
//! its own bar at the top with ← (it takes the keyboard as it opens), then
//! the one answer of the library and the services together: the best
//! match, the songs, the albums, the artists and the playlists. A song
//! plays the answer's songs from it. While the services are asked the
//! library's own hits stand as the answer (they come at once), and an
//! answer with nothing in it says so.

use egui::{Id, Rect, Ui, UiBuilder, pos2, vec2};
use nikgob_md3e::components::{SearchBar, anim};
use nikgob_view::{
    Command, Route, SearchAlbum, SearchAnswer, SearchArtist, SearchPlaylist, SearchTrack, SearchView, StreamPage, TopResult, ViewState,
};

use crate::parts::{self, PageOut, Picture};
use crate::rows::{self, Look};
use crate::{PageFrame, text};

/// The text rests this long before it is looked for.
const DEBOUNCE: f64 = 0.18;

#[derive(Default)]
pub struct Search {
    query: String,
    sent: String,
    edited: Option<f64>,
    /// The bar takes the keyboard on the next frame.
    pub focus: bool,
    /// The library's hits as an answer while the merged one is on its way,
    /// and the state's revision they were made at.
    early: Option<(u64, SearchAnswer)>,
}

/// The library's own hits for its query as an answer (the merged answer
/// replaces it when it comes).
fn library_answer(v: &SearchView) -> SearchAnswer {
    SearchAnswer {
        query: v.query.clone(),
        tracks: v.tracks.iter().cloned().map(SearchTrack::Library).collect(),
        albums: v.releases.iter().cloned().map(SearchAlbum::Library).collect(),
        artists: v.artists.iter().cloned().map(SearchArtist::Library).collect(),
        playlists: v.playlists.iter().cloned().map(SearchPlaylist::Library).collect(),
        searching: true,
        ..Default::default()
    }
}

/// What answers `q`: the merged answer once it is for `q`, the library's
/// hits while only they are, else the previous answer (none: nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Merged,
    Library,
    Previous,
}

fn source(v: &SearchView, q: &str) -> Source {
    if v.answer.query.trim() == q {
        Source::Merged
    } else if v.query.trim() == q {
        Source::Library
    } else {
        Source::Previous
    }
}

/// Where an album of the answer opens.
fn album_route(a: &SearchAlbum) -> Route {
    match a {
        SearchAlbum::Library(r) => Route::Release(r.id),
        SearchAlbum::Stream(c) => c.release.map_or_else(|| Route::Stream(StreamPage::Album(c.remote.clone())), Route::Release),
    }
}

/// Where an artist of the answer opens.
fn artist_route(a: &SearchArtist) -> Route {
    match a {
        SearchArtist::Library(c) => Route::Artist(c.id),
        SearchArtist::Stream(c) => c.artist.map_or_else(|| Route::Stream(StreamPage::Artist(c.remote.clone())), Route::Artist),
    }
}

impl Search {
    /// Puts `q` in the bar, looked for on the next frame (screenshot runs).
    pub fn set_query(&mut self, q: &str, now: f64) {
        if !q.is_empty() {
            self.query = q.to_owned();
            self.edited = Some(now - DEBOUNCE);
        }
    }

    pub fn show(&mut self, ui: &mut Ui, frame: PageFrame, state: &ViewState, out: &mut Vec<Command>) -> PageOut {
        let mut page = PageOut::default();
        let ctx = ui.ctx().clone();
        let now = anim::now(&ctx);
        let rect = frame.rect;
        let bar = Rect::from_min_size(pos2(rect.left() + 16.0, rect.top() + frame.top + 8.0), vec2(rect.width() - 32.0, 56.0));
        let mut bui = ui.new_child(UiBuilder::new().max_rect(bar).id_salt("phone-search-bar"));
        let before = self.query.clone();
        let o = SearchBar::new("phone-search").placeholder("Поиск").back(true).shortcut(false).width(bar.width()).show(&mut bui, &mut self.query);
        if std::mem::take(&mut self.focus) {
            o.response.request_focus();
        }
        if o.back_clicked {
            o.response.surrender_focus();
            page.back = true;
        }
        if self.query != before {
            self.edited = Some(now);
        }
        let q = self.query.trim().to_owned();
        let rest = self.edited.is_some_and(|t| now - t >= DEBOUNCE);
        if (o.submitted || rest) && q != self.sent {
            self.sent = q.clone();
            self.edited = None;
            if !q.is_empty() {
                out.push(Command::Search(q.clone()));
            }
        } else if self.edited.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
        let body = Rect::from_min_max(pos2(rect.left(), bar.bottom() + 8.0), rect.max);
        let mut cui = ui.new_child(UiBuilder::new().max_rect(body).id_salt("phone-search-body"));
        cui.set_clip_rect(body.intersect(ui.clip_rect()));
        if q.is_empty() {
            return page;
        }
        let view = &state.search;
        let from = source(view, &q);
        if from != Source::Library {
            self.early = None;
        } else if self.early.as_ref().is_none_or(|(rev, _)| *rev != state.revision) {
            self.early = Some((state.revision, library_answer(view)));
        }
        let waiting = from != Source::Merged || view.answer.searching || view.searching || self.sent != q;
        let a = self.early.as_ref().map_or(&view.answer, |(_, early)| early);
        crate::scroll("phone-search-scroll", frame.reset).show(&mut cui, |ui| {
            // A scroll started by the finger puts the keyboard away.
            if ui.input(|i| i.pointer.is_decidedly_dragging()) {
                o.response.surrender_focus();
            }
            if a.is_empty() {
                if let Some(e) = a.error.as_ref().filter(|_| !waiting) {
                    parts::error(ui, e);
                } else if waiting {
                    parts::loading(ui);
                } else {
                    parts::nothing(ui);
                }
                return;
            }
            let picks: Vec<nikgob_view::SearchPick> = a.tracks.iter().map(SearchTrack::pick).collect();
            if let Some(top) = &a.top {
                parts::section(ui, "Лучший результат");
                match top {
                    TopResult::Track(t) => {
                        let (uri, sub) = track_face(t);
                        if parts::item(ui, Id::new("search-top"), uri.as_deref(), t.title(), &sub, Picture::Square).clicked() {
                            out.push(Command::PlaySearch { tracks: vec![t.pick()], start: 0, context: "поиска".into() });
                        }
                    }
                    TopResult::Album(al) => {
                        let (uri, title, sub) = album_face(al);
                        if parts::item(ui, Id::new("search-top"), uri.as_deref(), &title, &sub, Picture::Square).clicked() {
                            page.nav = Some(album_route(al));
                        }
                    }
                    TopResult::Artist(ar) => {
                        let (uri, name) = artist_face(ar);
                        if parts::item(ui, Id::new("search-top"), uri.as_deref(), &name, "", Picture::Round).clicked() {
                            page.nav = Some(artist_route(ar));
                        }
                    }
                }
            }
            if !a.tracks.is_empty() {
                parts::section(ui, "Треки");
                let here = Route::Home;
                rows::list(ui, "search-tracks", a.tracks.len(), rows::ROW_H, |ui, i, r| match &a.tracks[i] {
                    SearchTrack::Library(t) => {
                        let look = Look::list(t, state);
                        let o = rows::track(ui, r, Id::new(("search-track", i)), t, look, state);
                        if o.tap {
                            out.push(Command::PlaySearch { tracks: picks.clone(), start: i, context: "поиска".into() });
                        }
                        if o.play_next {
                            out.push(Command::PlayNext(vec![t.id]));
                        }
                        if o.menu {
                            page.menu = Some((t.clone(), here.clone()));
                        }
                    }
                    SearchTrack::Stream(t) => {
                        let sub = match t.duration {
                            Some(d) => format!("{} • {}", t.artist, text::duration(d)),
                            None => t.artist.clone(),
                        };
                        if rows::simple(ui, r, Id::new(("search-stream", i)), t.cover_url.as_deref(), &t.title, &sub) {
                            out.push(Command::PlaySearch { tracks: picks.clone(), start: i, context: "поиска".into() });
                        }
                    }
                });
            }
            if !a.albums.is_empty() {
                parts::section(ui, "Альбомы");
                parts::grid(ui, a.albums.len().min(9), 3, |ui, i, r| {
                    let al = &a.albums[i];
                    let (uri, title, sub) = album_face(al);
                    if parts::card(ui, r, Id::new(("search-album", i)), uri.as_deref(), &title, &sub, Picture::Square).clicked() {
                        page.nav = Some(album_route(al));
                    }
                });
            }
            if !a.artists.is_empty() {
                parts::section(ui, "Исполнители");
                parts::grid(ui, a.artists.len().min(9), 3, |ui, i, r| {
                    let ar = &a.artists[i];
                    let (uri, name) = artist_face(ar);
                    if parts::card(ui, r, Id::new(("search-artist", i)), uri.as_deref(), &name, "", Picture::Round).clicked() {
                        page.nav = Some(artist_route(ar));
                    }
                });
            }
            if !a.playlists.is_empty() {
                parts::section(ui, "Плейлисты");
                for (i, p) in a.playlists.iter().enumerate() {
                    match p {
                        SearchPlaylist::Library(c) => {
                            let uri = c.cover.as_ref().map(|c| c.uri());
                            if parts::item(ui, Id::new(("search-pl", i)), uri.as_deref(), &c.name, &text::tracks(u64::from(c.track_count)), Picture::Square).clicked() {
                                page.nav = Some(Route::Playlist(c.id));
                            }
                        }
                        SearchPlaylist::Stream(c) => {
                            let owner = c.owner.clone().unwrap_or_default();
                            if parts::item(ui, Id::new(("search-pl", i)), None, &c.name, &owner, Picture::Square).clicked() {
                                page.nav = Some(Route::Stream(StreamPage::Playlist(c.remote.clone())));
                            }
                        }
                    }
                }
            }
            if let Some(e) = &a.error {
                parts::error(ui, e);
            }
        });
        page
    }
}

/// A song's picture and its line under the title.
fn track_face(t: &SearchTrack) -> (Option<String>, String) {
    match t {
        SearchTrack::Library(r) => (r.cover.as_ref().map(|c| c.uri()), r.artist.clone()),
        SearchTrack::Stream(r) => (r.cover_url.clone(), r.artist.clone()),
    }
}

fn album_face(a: &SearchAlbum) -> (Option<String>, String, String) {
    match a {
        SearchAlbum::Library(r) => (r.cover.as_ref().map(|c| c.uri()), r.title.clone(), r.artist.clone()),
        SearchAlbum::Stream(c) => (c.cover_url.clone(), c.title.clone(), c.artist.clone()),
    }
}

fn artist_face(a: &SearchArtist) -> (Option<String>, String) {
    match a {
        SearchArtist::Library(c) => (c.image.as_ref().map(|i| i.uri()), c.name.clone()),
        SearchArtist::Stream(c) => (c.image_url.clone(), c.name.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_answers_until_the_merged_answer_comes() {
        let mut v = SearchView { query: "radiohead".into(), ..Default::default() };
        assert_eq!(source(&v, "radiohead"), Source::Library);
        v.answer.query = "radiohead".into();
        assert_eq!(source(&v, "radiohead"), Source::Merged);
        assert_eq!(source(&v, "radio"), Source::Previous);
    }

    #[test]
    fn the_library_answer_is_marked_as_still_searching() {
        let v = SearchView { query: "x".into(), ..Default::default() };
        let a = library_answer(&v);
        assert!(a.searching);
        assert_eq!(a.query, "x");
        assert!(a.is_empty());
    }
}
