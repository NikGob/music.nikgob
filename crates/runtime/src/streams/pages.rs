//! Album / playlist / artist pages of a service, and playing and liking
//! stream items by remote id. Nothing joins the library by being shown; a
//! track gets its (outside-the-library) row when it is queued.

use std::time::Duration;

use nikgob_core::model::{EventContext, LikeTarget, TrackId};
use nikgob_core::streaming::{
    RemoteId, StreamAlbum, StreamArtist, StreamError, StreamPlaylist, StreamTrack, join_names,
};
use nikgob_view::{self as v, Command};

use super::{Msg, error_text};
use crate::Worker;

/// What a service sent for a page.
#[derive(Clone)]
pub(crate) enum PageData {
    Album(StreamAlbum),
    Playlist(StreamPlaylist),
    Artist(StreamArtist),
}

/// What to do with an album / playlist once it is fetched.
#[derive(Clone)]
pub(crate) enum Then {
    Play {
        start: Option<RemoteId>,
        shuffle: bool,
    },
    Like(bool),
}

impl Worker {
    pub(crate) fn load_stream(&mut self, page: v::StreamPage) {
        let remote = page.remote().clone();
        let Some(svc) = self.service(remote.service) else {
            let mut view = v::StreamPageView::loading(page);
            view.loading = false;
            view.error = Some(format!("{} выключен", remote.service.display_name()));
            self.publish(|s, _| s.stream_page = Some(view));
            return;
        };
        self.streaming.page_seq += 1;
        let seq = self.streaming.page_seq;
        // Already fetched this session: shown at once, refreshed anyway.
        let cached = match &page {
            v::StreamPage::Album(id) => self.streaming.albums.get(id).cloned().map(PageData::Album),
            v::StreamPage::Playlist(id) => {
                self.streaming.playlists.get(id).cloned().map(PageData::Playlist)
            }
            v::StreamPage::Artist(_) => None,
        };
        let view = match &cached {
            Some(data) => {
                let mut view = self.page_view(&page, data);
                view.loading = true;
                view
            }
            None => v::StreamPageView::loading(page.clone()),
        };
        self.publish(|s, _| s.stream_page = Some(view));
        let p = page.clone();
        self.streaming.spawn(async move {
            let id = p.remote().id.clone();
            let result = match &p {
                v::StreamPage::Album(_) => svc.album(&id).await.map(PageData::Album),
                v::StreamPage::Playlist(_) => svc.playlist(&id).await.map(PageData::Playlist),
                v::StreamPage::Artist(_) => svc.artist(&id).await.map(PageData::Artist),
            };
            Msg::Page {
                seq,
                page: p,
                result,
            }
        });
    }

    fn remember(&mut self, data: &PageData) {
        match data {
            PageData::Album(a) => {
                self.learn_album(a);
                self.streaming.learn(&a.tracks);
                self.streaming.albums.insert(a.id.clone(), a.clone());
            }
            PageData::Playlist(p) => {
                self.streaming.learn(&p.tracks);
                self.streaming.playlists.insert(p.id.clone(), p.clone());
            }
            PageData::Artist(_) => {}
        }
    }

    pub(crate) fn page_loaded(
        &mut self,
        seq: u64,
        page: v::StreamPage,
        result: Result<PageData, StreamError>,
    ) {
        if let Ok(data) = &result {
            self.remember(data);
        }
        if seq != self.streaming.page_seq {
            return;
        }
        let view = match result {
            Ok(data) => self.page_view(&page, &data),
            Err(e) => {
                log::warn!("{page:?}: {e}");
                let mut view = v::StreamPageView::loading(page);
                view.loading = false;
                view.error = Some(error_text(&e));
                view
            }
        };
        self.publish(|s, _| s.stream_page = Some(view));
    }

    /// Refreshes the open page's library marks (likes, files) after a change.
    pub(crate) fn refresh_stream_page(&mut self) {
        let Some(page) = self.state.read().stream_page.as_ref().map(|p| p.page.clone()) else {
            return;
        };
        let data = match &page {
            v::StreamPage::Album(id) => self.streaming.albums.get(id).cloned().map(PageData::Album),
            v::StreamPage::Playlist(id) => {
                self.streaming.playlists.get(id).cloned().map(PageData::Playlist)
            }
            v::StreamPage::Artist(_) => None,
        };
        if let Some(data) = data {
            let view = self.page_view(&page, &data);
            self.publish(|s, _| s.stream_page = Some(view));
        }
    }

    fn page_view(&mut self, page: &v::StreamPage, data: &PageData) -> v::StreamPageView {
        let mut view = v::StreamPageView::loading(page.clone());
        view.loading = false;
        match data {
            PageData::Album(a) => {
                view.title = a.title.clone();
                view.subtitle = join_names(&a.artists);
                view.cover_url = a.cover_url.clone();
                view.year = a.year;
                view.release_type = Some(a.release_type);
                view.tracks = a.tracks.iter().map(|t| self.stream_row_auto(t)).collect();
                (view.release, view.have) = self.album_have(a);
                view.liked = !view.tracks.is_empty() && view.tracks.iter().all(|t| t.liked);
            }
            PageData::Playlist(p) => {
                view.title = p.name.clone();
                view.subtitle = p.owner.clone().unwrap_or_default();
                view.cover_url = p.cover_url.clone();
                view.tracks = p.tracks.iter().map(|t| self.stream_row_auto(t)).collect();
                view.playlist = self.lib.playlist_by_remote(&p.id).ok().flatten().map(|r| r.playlist_id);
                view.editable = p.editable;
            }
            PageData::Artist(a) => {
                view.title = a.name.clone();
                view.cover_url = a.image_url.clone();
                view.albums = a.albums.iter().map(|al| self.album_card(al)).collect();
                view.artist = self.lib.artist_by_remote(&a.id).ok().flatten();
                if let (Some(id), Some(url)) = (view.artist, &a.image_url) {
                    let _ = self.lib.set_stream_artist_image(id, url);
                }
                view.liked = view.artist.is_some_and(|id| self.lib.is_liked(id).unwrap_or(false));
            }
        }
        view.total_duration = view.tracks.iter().filter_map(|t| t.duration).sum();
        view
    }

    /// A row with its library track looked up.
    fn stream_row_auto(&self, t: &StreamTrack) -> v::StreamTrackRow {
        self.stream_row(t, self.library_track_of(t))
    }

    pub(crate) fn stream_row(&self, t: &StreamTrack, track: Option<TrackId>) -> v::StreamTrackRow {
        let row = track.and_then(|id| self.lib.get_track_row(id).ok().flatten());
        let album = t.album.as_ref();
        v::StreamTrackRow {
            remote: t.id.clone(),
            title: t.title.clone(),
            artist: t.artist_line(),
            artist_id: t.artists.first().and_then(|a| a.id.clone()),
            album: album.map(|a| a.title.clone()),
            album_id: album.and_then(|a| a.id.clone()),
            duration: t.duration_ms.map(Duration::from_millis),
            disc_no: t.disc_no.unwrap_or(1),
            track_no: t.track_no.unwrap_or(0),
            cover_url: t.cover_url.clone(),
            kind: t.kind,
            playable: t.playable,
            in_library: track
                .and_then(|id| self.lib.track_in_library(id).ok().flatten())
                .unwrap_or(false),
            liked: row.as_ref().is_some_and(|r| r.liked),
            has_local: row.as_ref().is_some_and(|r| r.has_local),
            track,
            also_on: Vec::new(),
        }
    }

    /// How much of a service's album the library has: the one count its
    /// card, its page and the library card of the linked album all show
    /// ([`v::AlbumHave`]). Counted over the linked library album; the total
    /// is the larger of the service's track count (of the full album when
    /// it was fetched) and the library's.
    pub(crate) fn album_have(&self, a: &StreamAlbum) -> (Option<nikgob_core::model::ReleaseId>, v::AlbumHave) {
        let release = self.lib.release_by_remote(&a.id).ok().flatten();
        let full = self.streaming.albums.get(&a.id).unwrap_or(a);
        let listed = (!full.tracks.is_empty()).then_some(full.tracks.len() as u32);
        let theirs = full.total_tracks.max(a.total_tracks).max(listed).unwrap_or(0);
        let p = release.and_then(|r| self.lib.release_progress(r).ok().flatten());
        let have = v::AlbumHave {
            local: p.map_or(0, |p| p.local),
            in_library: p.map_or(0, |p| p.in_library),
            total: p.map_or(0, |p| p.total).max(theirs),
        };
        (release, have)
    }

    /// The linked library album learns the track count and cover.
    pub(crate) fn learn_album(&self, a: &StreamAlbum) {
        let listed = (!a.tracks.is_empty()).then_some(a.tracks.len() as u32);
        let _ = self
            .lib
            .learn_stream_album(&a.id, a.total_tracks.max(listed), a.cover_url.as_deref());
    }

    pub(crate) fn album_card(&self, a: &StreamAlbum) -> v::StreamAlbumCard {
        let (release, have) = self.album_have(a);
        v::StreamAlbumCard {
            remote: a.id.clone(),
            title: a.title.clone(),
            artist: join_names(&a.artists),
            artist_id: a.artists.first().and_then(|ar| ar.id.clone()),
            year: a.year,
            release_type: a.release_type,
            cover_url: a.cover_url.clone(),
            total_tracks: a.total_tracks,
            release,
            have,
        }
    }

    pub(crate) fn artist_card(&self, a: &StreamArtist) -> v::StreamArtistCard {
        v::StreamArtistCard {
            remote: a.id.clone(),
            name: a.name.clone(),
            image_url: a.image_url.clone(),
            artist: self.lib.artist_by_remote(&a.id).ok().flatten(),
        }
    }

    pub(crate) fn playlist_card(&self, p: &StreamPlaylist) -> v::StreamPlaylistCard {
        let linked = self.lib.playlist_by_remote(&p.id).ok().flatten();
        v::StreamPlaylistCard {
            remote: p.id.clone(),
            name: p.name.clone(),
            owner: p.owner.clone(),
            cover_url: p.cover_url.clone(),
            total_tracks: p.total_tracks,
            editable: p.editable,
            playlist: linked.as_ref().map(|r| r.playlist_id),
            sync_mode: linked.map(|r| r.sync_mode),
        }
    }

    // ------------------------------------------------------------- play / like

    pub(crate) fn play_stream_tracks(&mut self, remotes: &[RemoteId], start: usize, context: String) {
        let start_remote = remotes.get(start).cloned();
        let known: Vec<RemoteId> = remotes
            .iter()
            .filter(|r| self.streaming.known.contains_key(*r))
            .cloned()
            .collect();
        let ids = self.seen_tracks(&known);
        if ids.is_empty() {
            self.toast("Нечего играть", None);
            return;
        }
        let start = start_remote
            .and_then(|r| known.iter().position(|k| *k == r))
            .unwrap_or(0);
        self.queue.shuffle = false;
        self.queue.set(&ids, start, Some(context));
        self.play_context = EventContext::Search;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
    }

    pub(crate) fn with_album(&mut self, id: RemoteId, then: Then) {
        if let Some(a) = self.streaming.albums.get(&id).cloned() {
            self.fetched(then, Ok(PageData::Album(a)));
            return;
        }
        let Some(svc) = self.service(id.service) else {
            self.toast(format!("{} выключен", id.service.display_name()), None);
            return;
        };
        self.streaming.spawn(async move {
            let r = svc.album(&id.id).await.map(PageData::Album);
            Msg::Fetched(then, r)
        });
    }

    pub(crate) fn with_playlist(&mut self, id: RemoteId, then: Then) {
        if let Some(p) = self.streaming.playlists.get(&id).cloned() {
            self.fetched(then, Ok(PageData::Playlist(p)));
            return;
        }
        let Some(svc) = self.service(id.service) else {
            self.toast(format!("{} выключен", id.service.display_name()), None);
            return;
        };
        self.streaming.spawn(async move {
            let r = svc.playlist(&id.id).await.map(PageData::Playlist);
            Msg::Fetched(then, r)
        });
    }

    pub(crate) fn fetched(&mut self, then: Then, result: Result<PageData, StreamError>) {
        let data = match result {
            Ok(d) => d,
            Err(e) => {
                log::warn!("fetching for play/like: {e}");
                self.toast(format!("Не получилось: {}", error_text(&e)), None);
                return;
            }
        };
        self.remember(&data);
        let (tracks, title, ctx, album) = match &data {
            PageData::Album(a) => (a.tracks.clone(), a.title.clone(), EventContext::Album, Some(a.id.clone())),
            PageData::Playlist(p) => (p.tracks.clone(), p.name.clone(), EventContext::Playlist, None),
            PageData::Artist(_) => return,
        };
        let playable: Vec<StreamTrack> = tracks.into_iter().filter(|t| t.playable).collect();
        match then {
            Then::Play { start, shuffle } => {
                let ids = self.upsert_streams(&playable);
                let start = start
                    .and_then(|r| playable.iter().position(|t| t.id == r))
                    .and_then(|i| ids.get(i).copied());
                self.play_list(ids, start, shuffle, Some(title), ctx);
            }
            Then::Like(on) => {
                let ids = self.upsert_streams(&playable);
                let release = album
                    .and_then(|a| self.lib.release_by_remote(&a).ok().flatten())
                    .or_else(|| {
                        ids.first()
                            .and_then(|t| self.lib.get_track(*t).ok().flatten())
                            .and_then(|t| t.release_id)
                    });
                match release {
                    Some(r) => self.command(Command::Like(LikeTarget::Release(r), on)),
                    None => self.toast("Не получилось лайкнуть альбом", None),
                }
                self.refresh_stream_page();
            }
        }
    }

    pub(crate) fn like_stream(&mut self, remote: &RemoteId, on: bool) {
        let Some(t) = self.streaming.known.get(remote).cloned() else {
            self.toast("Трек устарел — найди его заново", None);
            return;
        };
        let Some(id) = self.upsert_streams(std::slice::from_ref(&t)).first().copied() else {
            return;
        };
        self.command(Command::Like(LikeTarget::Track(id), on));
        self.refresh_stream_page();
        // A liked track is a library hit now: the sections merge again.
        self.rebuild_sections();
    }
}
