//! Streaming in the runtime (stage 3, docs/stage3.md): accounts, the stream
//! part of the unified search, pages of a service, playback resolution (a
//! Spotify track without Spotify sound plays from YouTube Music), likes both
//! ways, the wishlist pipeline (like → torrent → file attached to the same
//! track) and sharing.
//!
//! Every service call runs on this module's own tokio runtime (the import
//! one exists only when importing is set up) and comes back to the worker
//! as an [`Msg`], so the worker stays the only writer of the library state
//! it publishes and never waits on the network (docs/stage3.md §1).
//!
//! Part B of the stage (cold-start imports, playlist sync modes, artist
//! discography) has its commands in the view contract already; they are
//! answered by [`Worker::part_b_stub`] for now.

mod accounts;
mod likes;
mod pages;
mod resolve;
mod search;
mod share;
mod wishlist;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{TrackId, WishlistId};
use nikgob_core::streaming::{
    Account, RemoteId, Service, StreamAlbum, StreamError, StreamPlaylist, StreamService,
    StreamTrack,
};
use nikgob_view::{self as v, Command};

use crate::Worker;
use crate::import::Internal;

pub(crate) use wishlist::WISH_ID;

/// How long a failed YouTube lookup of a track is not repeated.
const MISS_TTL: Duration = Duration::from_secs(6 * 3600);
/// How often the services' liked lists are pulled while the app runs.
const PULL_EVERY: Duration = Duration::from_secs(15 * 60);
/// Stream tracks kept by remote id for play / like by id; the cache is
/// dropped when it grows past this.
const KNOWN_MAX: usize = 20_000;

/// Results of background work, for [`Worker::stream_msg`].
pub(crate) enum Msg {
    LoginDone(Service, nikgob_core::streaming::LoginInput, Result<(), StreamError>),
    Searched {
        seq: u64,
        service: Service,
        result: Result<nikgob_core::streaming::StreamSearch, StreamError>,
    },
    Page {
        seq: u64,
        page: v::StreamPage,
        result: Result<pages::PageData, StreamError>,
    },
    /// An album or playlist fetched for a play / like command.
    Fetched(pages::Then, Result<pages::PageData, StreamError>),
    Resolved {
        track: TrackId,
        found: Option<StreamTrack>,
    },
    LikesPulled(Service, Result<Vec<StreamTrack>, StreamError>),
    LikePushed(RemoteId, bool, Result<(), StreamError>),
    Wish(wishlist::WishMsg),
    Shared {
        seq: u64,
        target: v::ShareTarget,
        result: share::Shared,
    },
}

/// The worker's streaming state.
pub(crate) struct Streams {
    rt: tokio::runtime::Runtime,
    internal: Sender<Internal>,
    /// Stream tracks seen in results and pages, by remote id.
    known: HashMap<RemoteId, StreamTrack>,
    albums: HashMap<RemoteId, StreamAlbum>,
    playlists: HashMap<RemoteId, StreamPlaylist>,
    /// Logins running, last login errors.
    busy: HashSet<Service>,
    errors: HashMap<Service, String>,
    last_accounts: Vec<v::AccountView>,
    pub(crate) search: search::SearchState,
    page_seq: u64,
    /// YouTube lookups running / failed recently, by track.
    resolving: HashSet<TrackId>,
    misses: HashMap<TrackId, Instant>,
    /// Wishlist items whose "save from YouTube" waits for the lookup.
    save_after_resolve: HashMap<TrackId, WishlistId>,
    last_pull: Option<Instant>,
    pulling: HashSet<Service>,
    pub(crate) wish: wishlist::WishState,
    share_seq: u64,
    clipboard_id: u64,
    odesli: Option<nikgob_metadata::Odesli>,
}

impl Streams {
    pub(crate) fn new(internal: Sender<Internal>) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nikgob-streams")
            .enable_all()
            .build()
            .expect("streaming runtime");
        Self {
            rt,
            internal,
            known: HashMap::new(),
            albums: HashMap::new(),
            playlists: HashMap::new(),
            busy: HashSet::new(),
            errors: HashMap::new(),
            last_accounts: Vec::new(),
            search: search::SearchState::default(),
            page_seq: 0,
            resolving: HashSet::new(),
            misses: HashMap::new(),
            save_after_resolve: HashMap::new(),
            last_pull: None,
            pulling: HashSet::new(),
            wish: wishlist::WishState::default(),
            share_seq: 0,
            clipboard_id: 0,
            odesli: None,
        }
    }

    /// Runs `f` on the streaming runtime; its message goes to the worker.
    fn spawn<F>(&self, f: F) -> tokio::task::JoinHandle<()>
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.internal.clone();
        self.rt.spawn(async move {
            let msg = f.await;
            let _ = tx.send(Internal::Stream(msg));
        })
    }

    /// Remembers stream tracks by remote id.
    fn learn<'a>(&mut self, tracks: impl IntoIterator<Item = &'a StreamTrack>) {
        if self.known.len() > KNOWN_MAX {
            self.known.clear();
        }
        for t in tracks {
            self.known.insert(t.id.clone(), t.clone());
        }
    }
}

/// A service error as the user reads it.
pub(crate) fn error_text(e: &StreamError) -> String {
    match e {
        StreamError::NotConfigured => "Сервис не настроен".into(),
        StreamError::NotLoggedIn => "Нужно войти".into(),
        StreamError::PremiumRequired => "Нужен Premium".into(),
        StreamError::RateLimited { .. } => "Сервис просит подождать — попробуй позже".into(),
        StreamError::Unavailable => "Недоступно".into(),
        StreamError::Unsupported => "Сервис так не умеет".into(),
        StreamError::Http { status } => format!("Сервис ответил ошибкой (HTTP {status})"),
        StreamError::Network(m) => format!("Нет связи: {m}"),
        StreamError::Parse(_) => "Сервис ответил непонятно".into(),
    }
}

impl Worker {
    /// The registered service of `kind`, if it is switched on.
    pub(crate) fn service(&self, kind: Service) -> Option<Arc<dyn StreamService>> {
        if !self.state.read().prefs.service_enabled(kind) {
            return None;
        }
        self.streams.iter().find(|s| s.service() == kind).cloned()
    }

    /// The service plays sound for this account right now (Spotify needs
    /// Premium and the playback login; YouTube plays without a login).
    pub(crate) fn has_audio(&self, kind: Service) -> bool {
        let Some(s) = self.service(kind) else {
            return false;
        };
        match kind {
            Service::Spotify => matches!(s.account(), Account::LoggedIn { audio: true, .. }),
            Service::Youtube => !matches!(s.account(), Account::LoggedIn { audio: false, .. }),
        }
    }

    /// Start-up: accounts, the first pull of likes, the wishlist view.
    pub(crate) fn start_streams(&mut self) {
        self.publish_accounts();
        self.pull_all_likes();
        self.publish_wishlist();
    }

    /// Every 5 s: accounts while logging in, the periodic pull, the wishlist
    /// scheduler.
    pub(crate) fn stream_tick(&mut self) {
        self.publish_accounts();
        if self
            .streaming
            .last_pull
            .is_none_or(|t| t.elapsed() >= PULL_EVERY)
        {
            self.pull_all_likes();
        }
        self.wish_tick();
    }

    /// Streaming commands; anything else is handed back.
    pub(crate) fn stream_command(&mut self, c: Command) -> Option<Command> {
        match c {
            Command::StreamLogin { service, input } => self.stream_login(service, input),
            Command::StreamLogout(service) => self.stream_logout(service),
            Command::SetStreamSecrets {
                spotify_client_id,
                youtube_cookies,
            } => self.set_stream_secrets(spotify_client_id, youtube_cookies),
            Command::SetSearchSources(sources) => self.set_search_sources(sources),
            Command::LoadStream(page) => self.load_stream(page),
            Command::PlayStreamTracks {
                tracks,
                start,
                context,
            } => self.play_stream_tracks(&tracks, start, context),
            Command::PlayStreamAlbum { id, start, shuffle } => {
                self.with_album(id, pages::Then::Play { start, shuffle });
            }
            Command::PlayStreamPlaylist { id, start, shuffle } => {
                self.with_playlist(id, pages::Then::Play { start, shuffle });
            }
            Command::PlayStreamNext(tracks) => {
                let ids = self.seen_tracks(&tracks);
                self.command(Command::PlayNext(ids));
            }
            Command::EnqueueStream(tracks) => {
                let ids = self.seen_tracks(&tracks);
                self.command(Command::Enqueue(ids));
            }
            Command::LikeStream { track, on } => self.like_stream(&track, on),
            Command::LikeStreamAlbum { album, on } => self.with_album(album, pages::Then::Like(on)),
            Command::AddStreamToPlaylist(playlist, tracks) => {
                let ids = self.seen_tracks(&tracks);
                if !ids.is_empty() {
                    self.command(Command::AddToPlaylist(playlist, ids));
                }
            }
            Command::Share(target) => self.share(target),
            Command::LoadWishlist => self.publish_wishlist(),
            Command::WishlistDownload(select) => self.wish_download(&select),
            Command::WishlistDismiss(ids) => self.wish_dismiss(&ids),
            Command::WishlistRetry(id) => self.wish_retry(id),
            Command::WishlistSaveFromYoutube(id) => self.wish_save_from_youtube(id),
            Command::WishlistAttach { item, what } => self.wish_attach(item, what),
            c @ (Command::ImportFromService(_)
            | Command::ImportExportFiles(_)
            | Command::SetPlaylistSync { .. }
            | Command::LoadDiscography(_)) => self.part_b_stub(&c),
            other => return Some(other),
        }
        None
    }

    /// PART B (cold-start imports, playlist sync modes, discography): the
    /// commands exist so the UI can be built against the final contract;
    /// the follow-up stage implements them.
    fn part_b_stub(&mut self, c: &Command) {
        log::info!("not implemented yet: {c:?}");
        if let Command::LoadDiscography(id) = c {
            let id = *id;
            self.publish(|s, _| {
                if let Some(a) = s.artist.as_mut().filter(|a| a.card.id == id) {
                    a.discography.loading = false;
                }
            });
            // Asked by every artist page: no toast, the section stays hidden.
            return;
        }
        self.toast("Скоро", None);
    }

    pub(crate) fn stream_msg(&mut self, m: Msg) {
        match m {
            Msg::LoginDone(service, input, r) => self.login_done(service, input, r),
            Msg::Searched {
                seq,
                service,
                result,
            } => self.stream_searched(seq, service, result),
            Msg::Page { seq, page, result } => self.page_loaded(seq, page, result),
            Msg::Fetched(then, result) => self.fetched(then, result),
            Msg::Resolved { track, found } => self.resolved(track, found),
            Msg::LikesPulled(service, r) => self.likes_pulled(service, r),
            Msg::LikePushed(remote, on, r) => self.like_pushed(&remote, on, r),
            Msg::Wish(w) => self.wish_msg(w),
            Msg::Shared {
                seq,
                target,
                result,
            } => self.shared(seq, target, result),
        }
    }

    /// Library rows for stream tracks known by remote id (unknown ones are
    /// skipped): they become "seen" tracks, outside the library until liked.
    pub(crate) fn seen_tracks(&mut self, remotes: &[RemoteId]) -> Vec<TrackId> {
        let tracks: Vec<StreamTrack> = remotes
            .iter()
            .filter_map(|r| self.streaming.known.get(r).cloned())
            .collect();
        if tracks.len() < remotes.len() {
            log::warn!(
                "{} of {} stream tracks are not known",
                remotes.len() - tracks.len(),
                remotes.len()
            );
        }
        self.upsert_streams(&tracks)
    }

    pub(crate) fn upsert_streams(&mut self, tracks: &[StreamTrack]) -> Vec<TrackId> {
        self.log_err("стрим-треки", self.lib.upsert_stream_tracks(tracks))
            .unwrap_or_default()
    }

    /// Where saved stream audio waits for the importer.
    pub(crate) fn save_dir(&self) -> PathBuf {
        match &self.import {
            Some(svc) => svc.config().downloads_dir.join(".youtube"),
            None => std::env::temp_dir().join("nikgob-youtube"),
        }
    }
}
