//! Album / playlist / artist pages of a service, and playing and liking
//! stream items by remote id. Nothing joins the library by being shown; a
//! track gets its (outside-the-library) row when it is queued.
//!
//! What was fetched is kept ([`PageCache`]): albums for a day, artists for
//! six hours, playlists for half an hour or until a sync sees a new
//! version. Opening it again (going back) shows it at once and asks
//! nothing; one fetch serves everyone who asked while it ran (opening an
//! album and pressing play is one request).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nikgob_core::model::{EventContext, LikeTarget, TrackId};
use nikgob_core::streaming::{
    RemoteId, Service, StreamAlbum, StreamArtist, StreamError, StreamPlaylist, StreamTrack, join_names,
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
    /// The album to «Хочу скачать» without a like.
    Download,
    /// The playlist into the library, following its service.
    Follow,
    /// After the current track ([`Command::PlayStreamAlbumNext`]).
    Next,
    /// At the end of the queue ([`Command::EnqueueStreamAlbum`]).
    Enqueue,
    /// Into a library playlist ([`Command::AddStreamAlbumToPlaylist`]).
    AddToPlaylist(v::PlaylistId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Album,
    Playlist,
    Artist,
}

/// A page of a service: its kind and remote id.
pub(crate) type Key = (Kind, RemoteId);

/// Pages kept at most.
const PAGES_MAX: usize = 400;

pub(crate) fn key_of(page: &v::StreamPage) -> Key {
    let kind = match page {
        v::StreamPage::Album(_) => Kind::Album,
        v::StreamPage::Playlist(_) => Kind::Playlist,
        v::StreamPage::Artist(_) => Kind::Artist,
    };
    (kind, page.remote().clone())
}

fn key_of_data(d: &PageData) -> Key {
    match d {
        PageData::Album(a) => (Kind::Album, a.id.clone()),
        PageData::Playlist(p) => (Kind::Playlist, p.id.clone()),
        PageData::Artist(a) => (Kind::Artist, a.id.clone()),
    }
}

/// Who waits for a fetch.
pub(crate) enum Waiter {
    Page { seq: u64, page: v::StreamPage },
    Then(Then),
}

/// Fetched pages, and the fetches running with who waits for them.
#[derive(Default)]
pub(crate) struct PageCache {
    entries: HashMap<Key, (Instant, PageData)>,
    inflight: HashMap<Key, Vec<Waiter>>,
}

impl PageCache {
    /// Whatever was fetched, however long ago (marks, counts).
    pub(crate) fn get(&self, key: &Key) -> Option<&PageData> {
        self.entries.get(key).map(|(_, d)| d)
    }

    /// Fetched less than `ttl` ago.
    pub(crate) fn fresh(&self, key: &Key, ttl: Duration) -> Option<&PageData> {
        self.entries
            .get(key)
            .filter(|(at, _)| at.elapsed() < ttl)
            .map(|(_, d)| d)
    }

    pub(crate) fn album(&self, id: &RemoteId) -> Option<&StreamAlbum> {
        match self.get(&(Kind::Album, id.clone())) {
            Some(PageData::Album(a)) => Some(a),
            _ => None,
        }
    }

    pub(crate) fn put(&mut self, data: PageData) {
        if self.entries.len() >= PAGES_MAX
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(key_of_data(&data), (Instant::now(), data));
    }

    /// The playlist changed in its service: the next open fetches it.
    pub(crate) fn stale_playlist(&mut self, id: &RemoteId) {
        if let Some((at, _)) = self.entries.get_mut(&(Kind::Playlist, id.clone())) {
            *at = Instant::now() - Duration::from_secs(365 * 24 * 3600);
        }
    }
}

/// The row of a service's track `t`, `track` its library track if it has
/// one (it reads only the library: the home's builder makes rows too).
pub(crate) fn stream_row_of(lib: &nikgob_library::Library, t: &StreamTrack, track: Option<TrackId>) -> v::StreamTrackRow {
    let row = track.and_then(|id| lib.get_track_row(id).ok().flatten());
    let album = t.album.as_ref();
    v::StreamTrackRow {
        remote: t.id.clone(),
        title: t.title.clone(),
        artist: t.artist_line(),
        artists: t.artists.as_slice().into(),
        album: album.map(|a| a.title.clone()),
        album_id: album.and_then(|a| a.id.clone()),
        duration: t.duration_ms.map(Duration::from_millis),
        disc_no: t.disc_no.unwrap_or(1),
        track_no: t.track_no.unwrap_or(0),
        cover_url: t.cover_url.clone(),
        kind: t.kind,
        playable: t.playable,
        in_library: track
            .and_then(|id| lib.track_in_library(id).ok().flatten())
            .unwrap_or(false),
        liked: row.as_ref().is_some_and(|r| r.liked),
        has_local: row.as_ref().is_some_and(|r| r.has_local),
        elsewhere: None,
        track,
        also_on: Vec::new(),
    }
}

/// How much of a service's album `a` the library has ([`Worker::album_have`]),
/// `theirs` the service's track count ([`Worker::album_theirs`]).
pub(crate) fn album_have_of(
    lib: &nikgob_library::Library,
    a: &StreamAlbum,
    theirs: u32,
) -> (Option<nikgob_core::model::ReleaseId>, v::AlbumHave) {
    let release = lib.release_by_remote(&a.id).ok().flatten();
    let p = release.and_then(|r| lib.release_progress(r).ok().flatten());
    let have = v::AlbumHave {
        local: p.map_or(0, |p| p.local),
        in_library: p.map_or(0, |p| p.in_library),
        total: p.map_or(0, |p| p.total).max(theirs),
    };
    (release, have)
}

/// The card of a service's album `a` ([`album_have_of`] says how much of it
/// the library has).
pub(crate) fn album_card_of(lib: &nikgob_library::Library, a: &StreamAlbum, theirs: u32) -> v::StreamAlbumCard {
    let (release, have) = album_have_of(lib, a, theirs);
    v::StreamAlbumCard {
        remote: a.id.clone(),
        title: a.title.clone(),
        artist: join_names(&a.artists),
        artists: a.artists.as_slice().into(),
        year: a.year,
        release_type: a.release_type,
        cover_url: a.cover_url.clone(),
        total_tracks: a.total_tracks,
        release,
        have,
    }
}

impl Worker {
    pub(crate) fn load_stream(&mut self, page: v::StreamPage) {
        let remote = page.remote().clone();
        let Some(svc) = self.service(remote.service) else {
            let mut view = v::StreamPageView::loading(page);
            view.loading = false;
            view.error = Some(self.off_text(remote.service));
            self.publish(|s| s.stream_page = Some(view));
            return;
        };
        self.streaming.page_seq += 1;
        let seq = self.streaming.page_seq;
        let key = key_of(&page);
        let ttl = self.page_ttl(key.0);
        // Fetched lately: shown at once, nothing is asked.
        if let Some(data) = self.streaming.pages.fresh(&key, ttl).cloned() {
            log::debug!("{:?} {}: from the page cache", key.0, key.1);
            let view = self.page_view(&page, &data);
            self.publish(|s| s.stream_page = Some(view));
            return;
        }
        // Fetched long ago: shown while it is fetched again.
        let view = match self.streaming.pages.get(&key).cloned() {
            Some(data) => {
                let mut view = self.page_view(&page, &data);
                view.loading = true;
                view
            }
            None => v::StreamPageView::loading(page.clone()),
        };
        self.publish(|s| s.stream_page = Some(view));
        self.fetch_page(key, Waiter::Page { seq, page }, svc);
    }

    /// How long a page of this kind is shown without asking again.
    fn page_ttl(&self, kind: Kind) -> Duration {
        let s = &self.streaming.schedule;
        match kind {
            Kind::Album => s.album_ttl,
            Kind::Artist => s.artist_ttl,
            Kind::Playlist => s.playlist_ttl,
        }
    }

    /// Fetches a page for `waiter`, or adds it to the fetch already running.
    fn fetch_page(&mut self, key: Key, waiter: Waiter, svc: std::sync::Arc<dyn nikgob_core::streaming::StreamService>) {
        if let Some(waiting) = self.streaming.pages.inflight.get_mut(&key) {
            log::debug!("{:?} {}: joins the fetch running", key.0, key.1);
            waiting.push(waiter);
            return;
        }
        self.streaming.pages.inflight.insert(key.clone(), vec![waiter]);
        let k = key.clone();
        self.streaming.spawn(async move {
            let id = &k.1.id;
            let result = match k.0 {
                Kind::Album => svc.album(id).await.map(PageData::Album),
                Kind::Playlist => svc.playlist(id).await.map(PageData::Playlist),
                Kind::Artist => svc.artist(id).await.map(PageData::Artist),
            };
            Msg::Loaded { key: k, result }
        });
    }

    /// A fetch came back: kept, and everyone waiting for it hears.
    pub(crate) fn loaded(&mut self, key: Key, result: Result<PageData, StreamError>) {
        if let Ok(data) = &result {
            self.remember(data);
        }
        for w in self.streaming.pages.inflight.remove(&key).unwrap_or_default() {
            match w {
                Waiter::Page { seq, page } => self.page_loaded(seq, page, result.clone()),
                Waiter::Then(then) => self.fetched(then, result.clone()),
            }
        }
    }

    pub(crate) fn remember(&mut self, data: &PageData) {
        match data {
            PageData::Album(a) => {
                self.learn_album(a);
                self.streaming.learn(&a.tracks);
            }
            PageData::Playlist(p) => self.streaming.learn(&p.tracks),
            PageData::Artist(_) => {}
        }
        self.streaming.pages.put(data.clone());
    }

    pub(crate) fn page_loaded(
        &mut self,
        seq: u64,
        page: v::StreamPage,
        result: Result<PageData, StreamError>,
    ) {
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
        self.publish(|s| s.stream_page = Some(view));
    }

    /// Refreshes the open page's library marks (likes, files) after a change.
    pub(crate) fn refresh_stream_page(&mut self) {
        let Some(page) = self.state.read().stream_page.as_ref().map(|p| p.page.clone()) else {
            return;
        };
        let data = self.streaming.pages.get(&key_of(&page)).cloned();
        if let Some(data) = data {
            let view = self.page_view(&page, &data);
            self.publish(|s| s.stream_page = Some(view));
        }
    }

    fn page_view(&mut self, page: &v::StreamPage, data: &PageData) -> v::StreamPageView {
        let mut view = v::StreamPageView::loading(page.clone());
        view.loading = false;
        match data {
            PageData::Album(a) => {
                view.title = a.title.clone();
                view.subtitle = join_names(&a.artists);
                view.subtitle_artists = a.artists.as_slice().into();
                view.cover_url = a.cover_url.clone();
                view.year = a.year;
                view.released = a.release_date.as_deref().and_then(|d| v::ReleaseDay::parse(d, a.year));
                view.release_type = Some(a.release_type);
                view.tracks = a.tracks.iter().map(|t| self.stream_row_auto(t)).collect();
                (view.release, view.have) = self.album_have(a);
                if let Some(r) = view.release {
                    self.keep_released(r, a);
                    self.mark_elsewhere(r, &mut view.tracks);
                    view.library = self.library_have(r);
                }
                // The page's own rows: a song of it filed on another album
                // is had, but the counts never speak of rows it does not show.
                if !view.tracks.is_empty() {
                    let rows = &view.tracks;
                    view.have = v::AlbumHave {
                        local: rows.iter().filter(|t| t.has_local).count() as u32,
                        in_library: rows.iter().filter(|t| t.in_library).count() as u32,
                        total: rows.len() as u32,
                    };
                }
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
    pub(crate) fn stream_row_auto(&self, t: &StreamTrack) -> v::StreamTrackRow {
        self.stream_row(t, self.library_track_of(t))
    }

    pub(crate) fn stream_row(&self, t: &StreamTrack, track: Option<TrackId>) -> v::StreamTrackRow {
        stream_row_of(&self.lib, t, track)
    }

    /// How much of a service's album the library has: the one count its
    /// card, its page and the library card of the linked album all show
    /// ([`v::AlbumHave`]). Counted over the linked library album; the total
    /// is the larger of the service's track count (of the full album when
    /// it was fetched) and the library's.
    pub(crate) fn album_have(&self, a: &StreamAlbum) -> (Option<nikgob_core::model::ReleaseId>, v::AlbumHave) {
        album_have_of(&self.lib, a, self.album_theirs(a))
    }

    /// The service's track count of `a`: of the full album when it was
    /// fetched ([`Self::album_have`]).
    pub(crate) fn album_theirs(&self, a: &StreamAlbum) -> u32 {
        let full = self.streaming.pages.album(&a.id).unwrap_or(a);
        let listed = (!full.tracks.is_empty()).then_some(full.tracks.len() as u32);
        full.total_tracks.max(a.total_tracks).max(listed).unwrap_or(0)
    }

    /// Rows whose library track is on another album than `release` name it.
    fn mark_elsewhere(&self, release: nikgob_core::model::ReleaseId, rows: &mut [v::StreamTrackRow]) {
        for t in rows.iter_mut() {
            let Some(row) = t.track.and_then(|id| self.lib.get_track_row(id).ok().flatten()) else { continue };
            if let Some(other) = row.release_id.filter(|r| *r != release) {
                t.elsewhere = self.lib.get_release(other).ok().flatten().map(|r| r.title);
            }
        }
    }

    /// The linked library album's title and count, as its own page says.
    fn library_have(&self, release: nikgob_core::model::ReleaseId) -> Option<(String, v::AlbumHave)> {
        let r = self.lib.get_release(release).ok().flatten()?;
        let p = self.lib.release_progress(release).ok().flatten()?;
        Some((r.title, v::AlbumHave { local: p.local, in_library: p.in_library, total: p.total }))
    }

    /// The linked library album learns the track count and cover.
    pub(crate) fn learn_album(&self, a: &StreamAlbum) {
        let listed = (!a.tracks.is_empty()).then_some(a.tracks.len() as u32);
        let _ = self
            .lib
            .learn_stream_album(&a.id, a.total_tracks.max(listed), a.cover_url.as_deref());
    }

    pub(crate) fn album_card(&self, a: &StreamAlbum) -> v::StreamAlbumCard {
        album_card_of(&self.lib, a, self.album_theirs(a))
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
            .filter(|r| self.streaming.known.contains_key(r))
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
        self.user_switch(nikgob_core::model::EventOrigin::Manual);
        self.new_list();
        self.queue.set(&ids, start, Some(context));
        self.play_context = EventContext::Search;
        self.play_context_id = None;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
        self.after_stream_play();
    }

    /// `Command::PlaySearch`: the merged tracks of the «Всё» search as one
    /// queue; stream rows become "seen" tracks, library rows are what they
    /// are. A row the runtime does not know (any more) is left out.
    pub(crate) fn play_search(&mut self, picks: &[v::SearchPick], start: usize, context: String) {
        let mut ids = Vec::with_capacity(picks.len());
        let mut start_at = 0;
        for (i, p) in picks.iter().enumerate() {
            let id = match p {
                v::SearchPick::Library(t) => Some(*t),
                v::SearchPick::Stream(r) => self
                    .streaming
                    .known
                    .get(r)
                    .cloned()
                    .and_then(|t| self.upsert_streams(std::slice::from_ref(&t)).first().copied()),
            };
            if let Some(id) = id {
                if i == start {
                    start_at = ids.len();
                }
                ids.push(id);
            }
        }
        if ids.is_empty() {
            self.toast("Нечего играть", None);
            return;
        }
        self.user_switch(nikgob_core::model::EventOrigin::Manual);
        self.new_list();
        self.queue.set(&ids, start_at, Some(context));
        self.play_context = EventContext::Search;
        self.play_context_id = None;
        if let Some(e) = self.queue.current() {
            self.start(e);
        }
        self.after_stream_play();
    }

    /// Stream rows that just got their track rows show which one plays: the
    /// open page and the search sections are drawn again with them.
    fn after_stream_play(&mut self) {
        self.refresh_stream_page();
        self.streaming.search.forget_links();
        self.rebuild_sections();
    }

    pub(crate) fn with_album(&mut self, id: RemoteId, then: Then) {
        self.with_page((Kind::Album, id), then);
    }

    pub(crate) fn with_playlist(&mut self, id: RemoteId, then: Then) {
        self.with_page((Kind::Playlist, id), then);
    }

    /// Plays / likes a page: kept (fresh) → at once, else fetched (or the
    /// fetch already running is joined).
    fn with_page(&mut self, key: Key, then: Then) {
        let ttl = self.page_ttl(key.0);
        if let Some(data) = self.streaming.pages.fresh(&key, ttl).cloned() {
            self.fetched(then, Ok(data));
            return;
        }
        let Some(svc) = self.service(key.1.service) else {
            self.toast(self.off_text(key.1.service), None);
            return;
        };
        self.fetch_page(key, Waiter::Then(then), svc);
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
        if let (Then::Follow, PageData::Playlist(p)) = (&then, &data) {
            self.follow_playlist(p);
            return;
        }
        let (tracks, name, title, ctx, album, playlist) = match &data {
            PageData::Album(a) => (
                a.tracks.clone(),
                a.title.clone(),
                format!("альбома «{}»", a.title),
                EventContext::Album,
                Some(a.id.clone()),
                None,
            ),
            PageData::Playlist(p) => (
                p.tracks.clone(),
                p.name.clone(),
                format!("плейлиста «{}»", p.name),
                EventContext::Playlist,
                None,
                Some(p.id.clone()),
            ),
            PageData::Artist(_) => return,
        };
        let playable: Vec<StreamTrack> = tracks.into_iter().filter(|t| t.playable).collect();
        match then {
            // As the library album's next / queue / «В плейлист…» do.
            Then::Next => {
                let ids = self.upsert_streams(&playable);
                if ids.is_empty() {
                    return;
                }
                self.queue.play_next(&ids);
                self.preload_next();
                self.publish_player();
                self.toast(format!("Следующим: «{name}»"), None);
                self.after_stream_play();
            }
            Then::Enqueue => {
                let ids = self.upsert_streams(&playable);
                self.command(Command::Enqueue(ids));
                self.after_stream_play();
            }
            Then::AddToPlaylist(pid) => {
                let ids = self.upsert_streams(&playable);
                if !ids.is_empty() {
                    self.command(Command::AddToPlaylist(pid, ids));
                }
            }
            Then::Play { start, shuffle } => {
                let ids = self.upsert_streams(&playable);
                let start = start
                    .and_then(|r| playable.iter().position(|t| t.id == r))
                    .and_then(|i| ids.get(i).copied());
                // The album or playlist of the library the page is, if any.
                let context_id = match (&album, &playlist) {
                    (Some(a), _) => self.lib.release_by_remote(a).ok().flatten().map(|r| format!("release:{r}")),
                    (_, Some(p)) => self.lib.playlist_by_remote(p).ok().flatten().map(|l| format!("playlist:{}", l.playlist_id)),
                    _ => None,
                };
                self.play_list(ids, start, shuffle, Some(title), ctx, context_id);
                self.after_stream_play();
            }
            Then::Download => {
                let ids = self.upsert_streams(&playable);
                let release = album
                    .and_then(|a| self.lib.release_by_remote(&a).ok().flatten())
                    .or_else(|| {
                        ids.first()
                            .and_then(|t| self.lib.get_track(*t).ok().flatten())
                            .and_then(|t| t.release_id)
                    });
                match release {
                    Some(r) => self.download_release(r),
                    None => self.toast("Не получилось скачать альбом", None),
                }
            }
            Then::Follow => {}
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

    /// `Command::FollowStreamPlaylist`: a library playlist that follows
    /// `p` (the default sync mode; someone else's playlist is only
    /// followed), its tracks in the library (waiting in «Хочу скачать» like
    /// an imported playlist's).
    fn follow_playlist(&mut self, p: &StreamPlaylist) {
        let open = |id| v::Link::Route(v::Route::Playlist(id));
        if let Some(link) = self.lib.playlist_by_remote(&p.id).ok().flatten()
            && self.lib.get_playlist(link.playlist_id).ok().flatten().is_some()
        {
            self.toast_open(format!("«{}» уже в библиотеке", p.name), "Открыть", open(link.playlist_id));
            return;
        }
        let Some(created) = self.log_err("плейлист", self.lib.create_playlist(&p.name)) else { return };
        let pid = created.id;
        let ids = self.upsert_streams(&p.tracks);
        let _ = self.log_err("плейлист", self.lib.playlist_add_many(pid, &ids));
        self.wish_for_likes(&ids, nikgob_core::model::WishlistOrigin::Import);
        // A catalogue (Deezer) is not synced with: its playlist is a
        // snapshot.
        let catalogue = self.service(p.id.service).is_none_or(|s| s.catalogue_only());
        let mode = match self.state.read().prefs.playlist_sync_default {
            _ if catalogue => nikgob_core::streaming::PlaylistSyncMode::Once,
            nikgob_core::streaming::PlaylistSyncMode::TwoWay if !p.editable => {
                nikgob_core::streaming::PlaylistSyncMode::Follow
            }
            m => m,
        };
        let base: Vec<String> = p.tracks.iter().map(|t| t.id.id.clone()).collect();
        let synced = self.sync_stamp(pid);
        let _ = self.log_err(
            "плейлист",
            self.lib.put_playlist_remote(&nikgob_library::PlaylistRemote {
                playlist_id: pid,
                remote: p.id.clone(),
                sync_mode: mode,
                remote_version: super::sync::version_of(p),
                synced_at: Some(synced),
                base_json: serde_json::to_string(&base).ok(),
            }),
        );
        self.load_library();
        self.publish_wishlist();
        self.refresh_stream_page();
        self.toast_open(format!("«{}» в библиотеке", p.name), "Открыть", open(pid));
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
        // A liked Deezer track learns its ISRC (search hits and album
        // tracks come without): one request for the track alone.
        if on
            && t.isrc.is_none()
            && remote.service == Service::Deezer
            && let Some(dz) = self.service(Service::Deezer)
        {
            let rid = remote.id.clone();
            self.streaming.spawn(async move {
                match dz.track(&rid).await {
                    Ok(full) => Msg::IsrcFilled(id, Box::new(full)),
                    Err(e) => {
                        log::info!("deezer: no ISRC for {rid}: {e}");
                        Msg::IsrcFilled(id, Box::new(StreamTrack::new(RemoteId::new(Service::Deezer, rid), "")))
                    }
                }
            });
        }
        self.refresh_stream_page();
        // A liked track is a library hit now: the sections merge again.
        self.streaming.search.forget_links();
        self.rebuild_sections();
    }
}
