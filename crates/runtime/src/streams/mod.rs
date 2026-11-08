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
//! Part B: the cold-start imports from the API and from export files
//! ([`cold`], [`export`]), the sync of linked playlists ([`sync`]) and the
//! discography of library artists ([`discography`]).
//!
//! Stage 4: the sound analysis of stream-only tracks ([`analysis`]).

mod accounts;
mod analysis;
mod clips;
mod cold;
mod complete;
mod discography;
mod export;
mod likes;
mod pages;
mod rank;
mod replace;
pub(crate) mod resolve;
mod schedule;
mod search;
mod share;
mod songs;
mod sync;
mod tracklist;
mod web_login;
mod wishlist;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{TrackId, WishlistId};
use nikgob_core::streaming::{
    Account, RemoteId, Service, StreamError, StreamService,
    StreamTrack,
};
use nikgob_view::{self as v, Command};

use crate::Worker;
use crate::import::Internal;

pub(crate) use cold::{IMPORT_DEVICE, STREAM_JOB, plural};
pub(crate) use replace::shown_title;
pub(crate) use wishlist::{WISH_ID, upgrade_job_words};
pub(crate) use schedule::wake_jitter;
pub use schedule::StreamSchedule;
pub use web_login::{SOUNDCLOUD_WINDOW_HINT, WINDOW_HINT, WebLogin, WebLoginError, window_hint};

/// How long a YouTube video that did not play is not picked again.
const REFUSED_TTL: Duration = Duration::from_secs(6 * 3600);
/// Stream tracks kept by remote id for play / like by id; past this the
/// least recently learnt quarter goes.
const KNOWN_MAX: usize = 20_000;

/// Stream tracks seen in results and pages, by remote id; the oldest go
/// first when it is full (a whole-map clear made tracks on screen
/// «устарел» and fetched again).
#[derive(Default)]
pub(crate) struct Known {
    map: HashMap<RemoteId, (u64, StreamTrack)>,
    tick: u64,
}

impl Known {
    pub(crate) fn get(&self, r: &RemoteId) -> Option<&StreamTrack> {
        self.map.get(r).map(|(_, t)| t)
    }

    pub(crate) fn contains_key(&self, r: &RemoteId) -> bool {
        self.map.contains_key(r)
    }

    pub(crate) fn insert(&mut self, t: &StreamTrack) {
        self.tick += 1;
        self.map.insert(t.id.clone(), (self.tick, t.clone()));
        if self.map.len() > KNOWN_MAX {
            let mut ticks: Vec<u64> = self.map.values().map(|(n, _)| *n).collect();
            let cut = KNOWN_MAX / 4;
            ticks.select_nth_unstable(cut);
            let below = ticks[cut];
            self.map.retain(|_, (n, _)| *n > below);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

/// When a service's liked list was checked and is checked next.
#[derive(Default)]
pub(crate) struct LikeSync {
    pub(crate) last: Option<Instant>,
    pub(crate) next_at: Option<Instant>,
    pub(crate) pulling: bool,
    /// Checks rate-limited in a row (their retry is set in `next_at`).
    pub(crate) limited: u32,
}

/// Results of background work, for [`Worker::stream_msg`].
pub(crate) enum Msg {
    LoginDone(Service, nikgob_core::streaming::LoginInput, Result<(), StreamError>),
    /// The app's sign-in window is done (see [`web_login`]).
    WebLoginDone(Service, Result<String, WebLoginError>),
    Searched {
        seq: u64,
        service: Service,
        result: Result<nikgob_core::streaming::StreamSearch, StreamError>,
        /// The first part; the whole answer follows.
        partial: bool,
    },
    /// A rate-limit pause of a service's search ended: ask again.
    SearchRetry {
        seq: u64,
        service: Service,
    },

    /// A page (or an album / playlist for a play or like command) came;
    /// everyone who waits for it hears.
    Loaded {
        key: pages::Key,
        result: Result<pages::PageData, StreamError>,
    },
    Resolved {
        track: TrackId,
        /// Where it was looked up: YouTube Music, or Spotify while
        /// YouTube Music is off.
        via: Service,
        found: resolve::Lookup,
        /// The full Deezer track asked for its ISRC on the way.
        filled: Option<Box<StreamTrack>>,
    },
    /// The full Deezer track of a liked track (its ISRC).
    IsrcFilled(TrackId, Box<StreamTrack>),
    /// The top of the «Всё» answer is settled (the grace after the first
    /// answer ended).
    SearchSettle { seq: u64 },
    /// A rate-limit pause of a waiting track's lookup ended.
    ResolveRetry(TrackId),
    /// The whole liked list.
    LikesPulled(Service, Result<Vec<StreamTrack>, StreamError>),
    /// Its first page and length.
    LikesHead(Service, Result<nikgob_core::streaming::LikedHead, StreamError>),
    /// The songs of a like check's new clips (see [`clips`]); the likes
    /// are stored with them.
    LikesConverted {
        service: Service,
        liked: Vec<StreamTrack>,
        songs: clips::Songs,
        misses: Vec<String>,
        partial: bool,
    },
    /// The repair's answer for one liked clip.
    ClipSong {
        clip: Box<nikgob_library::LikedClip>,
        result: Result<Option<StreamTrack>, StreamError>,
    },
    /// Time to send the queued likes of a service.
    LikeFlush(Service),
    LikesPushed {
        service: Service,
        remotes: Vec<RemoteId>,
        on: bool,
        result: Result<(), StreamError>,
    },
    Wish(wishlist::WishMsg),
    Shared {
        seq: u64,
        target: v::ShareTarget,
        result: share::Shared,
    },
    Cold(cold::ColdMsg),
    Sync(sync::SyncMsg),
    Disco(discography::DiscoMsg),
    /// A library album's whole tracklist (see [`tracklist`]).
    Tracklist(tracklist::TracklistMsg),
    /// Songs for the «Найти песню…» dialog (see [`replace`]).
    SongsFound {
        seq: u64,
        service: Service,
        result: Result<Vec<StreamTrack>, StreamError>,
    },
    /// The YouTube song of a Deezer pick (or none): the replacement goes on.
    ReplaceResolved {
        track: TrackId,
        pick: Box<StreamTrack>,
        youtube_song: Option<Box<StreamTrack>>,
        youtube: bool,
    },
    /// A job of the stream sound analysis ended (see [`analysis`]).
    Analysed(analysis::Done),
    /// Its gap or rate-limit pause is over.
    AnalysisWake,
    /// The details of a playing track that lacked its album or length
    /// (see [`complete`]).
    Completed {
        track: TrackId,
        id: String,
        result: Result<StreamTrack, StreamError>,
    },
}

/// The worker's streaming state.
pub(crate) struct Streams {
    rt: tokio::runtime::Runtime,
    internal: Sender<Internal>,
    /// Stream tracks seen in results and pages, by remote id.
    pub(crate) known: Known,
    /// Albums, artists and playlists fetched, and who waits for a fetch.
    pub(crate) pages: pages::PageCache,
    pub(crate) schedule: StreamSchedule,
    /// The app window is shown / has the focus ([`Command::WindowState`]).
    window_visible: bool,
    window_focused: bool,
    /// The window reported its state at least once (else how far it is
    /// from the user is not known).
    window_known: bool,
    last_wish_tick: Option<Instant>,
    /// Logins running, last login errors.
    busy: HashSet<Service>,
    errors: HashMap<Service, String>,
    /// The app's sign-in windows, and the ones open or failed (see [`web_login`]).
    web: HashMap<Service, Arc<dyn WebLogin>>,
    web_state: HashMap<Service, web_login::WebState>,
    last_accounts: Vec<v::AccountView>,
    /// SoundCloud's own account read Failed at the last publish.
    sc_refused: bool,
    /// Soloist's status as last published (polled every second).
    last_soloist: Option<nikgob_core::streaming::SoloistStatus>,
    pub(crate) search: search::SearchState,
    page_seq: u64,
    /// Copy lookups running / failed recently (unix ms), by track and
    /// the service asked.
    resolving: HashSet<TrackId>,
    /// Where each running lookup asks (YouTube Music or Spotify).
    resolve_via: HashMap<TrackId, Service>,
    resolve_limit: resolve::ResolveLimit,
    misses: HashMap<(TrackId, Service), i64>,
    /// YouTube videos that did not play, by video id (not by track: the
    /// same song has other copies).
    refused: HashMap<String, Instant>,
    /// Why the last source of a track failed, for the toast (and where
    /// the toast's button leads, if anywhere).
    pub(crate) failures: HashMap<TrackId, (String, Option<v::SettingLink>)>,
    /// Wishlist items whose "save from YouTube" waits for the lookup.
    save_after_resolve: HashMap<TrackId, WishlistId>,
    likes: HashMap<Service, LikeSync>,
    like_queue: likes::LikeQueue,
    /// Liked YouTube clips turning into their songs.
    clips: clips::ClipRepair,
    /// «Найти песню…»: its searches, the undo of replacements.
    replace: replace::ReplaceState,
    pub(crate) wish: wishlist::WishState,
    share_seq: u64,
    /// The «Ищу ссылку…» toast of the share that runs, replaced by its end.
    share_toast: Option<u64>,
    clipboard_id: u64,
    odesli: Option<nikgob_metadata::Odesli>,
    cold: cold::ColdState,
    sync: sync::SyncState,
    disco: discography::DiscoState,
    tracklist: tracklist::TracklistState,
    /// The sound analysis of stream-only tracks.
    pub(crate) analysis: analysis::StreamAnalysis,
    /// Playing tracks completed from their service.
    completing: complete::Completing,
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
            known: Known::default(),
            pages: pages::PageCache::default(),
            schedule: StreamSchedule::default(),
            window_visible: true,
            window_focused: true,
            window_known: false,
            last_wish_tick: None,
            busy: HashSet::new(),
            errors: HashMap::new(),
            web: HashMap::new(),
            web_state: HashMap::new(),
            last_accounts: Vec::new(),
            sc_refused: false,
            last_soloist: None,
            search: search::SearchState::default(),
            page_seq: 0,
            resolving: HashSet::new(),
            resolve_via: HashMap::new(),
            resolve_limit: resolve::ResolveLimit::default(),
            misses: HashMap::new(),
            refused: HashMap::new(),
            failures: HashMap::new(),
            save_after_resolve: HashMap::new(),
            likes: HashMap::new(),
            like_queue: likes::LikeQueue::default(),
            clips: clips::ClipRepair::default(),
            replace: replace::ReplaceState::default(),
            wish: wishlist::WishState::default(),
            share_seq: 0,
            share_toast: None,
            clipboard_id: 0,
            odesli: None,
            cold: cold::ColdState::default(),
            sync: sync::SyncState::default(),
            disco: discography::DiscoState::default(),
            tracklist: tracklist::TracklistState::default(),
            analysis: analysis::StreamAnalysis::default(),
            completing: complete::Completing::default(),
        }
    }

    /// Runs `f` on the streaming runtime; its message goes to the worker.
    pub(crate) fn spawn<F>(&self, f: F) -> tokio::task::JoinHandle<()>
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.internal.clone();
        self.rt.spawn(async move {
            let msg = f.await;
            let _ = tx.send(Internal::Stream(msg));
        })
    }

    /// Runs `f` on the streaming runtime; its message (of any part of the
    /// runtime) goes to the worker.
    pub(crate) fn spawn_internal<F>(&self, f: F) -> tokio::task::JoinHandle<()>
    where
        F: std::future::Future<Output = Internal> + Send + 'static,
    {
        let tx = self.internal.clone();
        self.rt.spawn(async move {
            let _ = tx.send(f.await);
        })
    }

    /// Runs `f` on the streaming runtime with a sender to the worker, for a
    /// job that reports more than once.
    pub(crate) fn spawn_reporting<F>(&self, f: impl FnOnce(Sender<Internal>) -> F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(f(self.internal.clone()));
    }

    /// Remembers stream tracks by remote id.
    pub(crate) fn learn<'a>(&mut self, tracks: impl IntoIterator<Item = &'a StreamTrack>) {
        for t in tracks {
            self.known.insert(t);
        }
    }
}

/// A service error as the user reads it.
pub(crate) fn error_text(e: &StreamError) -> String {
    match e {
        StreamError::NotConfigured => "Сервис не настроен".into(),
        StreamError::NotLoggedIn => "Нужно войти".into(),
        StreamError::PremiumRequired => "Нужен Premium".into(),
        // Said with its end when that is a while away.
        StreamError::RateLimited { retry_after } => match retry_after
            .filter(|d| *d > search::SHORT_WAIT)
            .and_then(accounts::hm_after)
        {
            Some(hm) => format!("Сервис просит подождать до {hm}"),
            None => "Сервис просит подождать".into(),
        },
        StreamError::Unavailable => "Недоступно".into(),
        StreamError::Unsupported => "Сервис так не умеет".into(),
        StreamError::AgeRestricted => "Только для 18+ — нужно войти".into(),
        StreamError::RegionBlocked => "Недоступно в твоём регионе".into(),
        StreamError::Drm => "Только в защищённом виде — сыграет с YouTube Music".into(),
        StreamError::Http { status } => format!("Сервис ответил ошибкой (HTTP {status})"),
        // The details go to the log at the call site.
        StreamError::Network(_) => "Нет связи — проверь интернет".into(),
        StreamError::Parse(_) => "Сервис ответил непонятно".into(),
    }
}

/// Whether a service can be asked anything in this account state: Spotify
/// only while connected (it needs Premium and the user's own app, docs/
/// stage3.md §0); YouTube Music answers without a login, the Deezer
/// catalogue has none.
pub(crate) fn usable(kind: Service, account: &Account) -> bool {
    match kind {
        Service::Spotify => account.is_logged_in(),
        Service::Youtube | Service::Deezer | Service::Soundcloud => true,
    }
}

impl Worker {
    /// The registered service of `kind`, if it is switched on and can be
    /// asked ([`usable`]): a Spotify that is not connected gets no request
    /// at all (no search, likes, playlist checks or enrichment).
    pub(crate) fn service(&self, kind: Service) -> Option<Arc<dyn StreamService>> {
        if !self.state.read().prefs.service_enabled(kind) {
            return None;
        }
        self.streams
            .iter()
            .find(|s| s.service() == kind && usable(kind, &s.account()))
            .cloned()
    }

    /// Why [`Self::service`] has nothing, as the user reads it.
    pub(crate) fn off_text(&self, kind: Service) -> String {
        if self.state.read().prefs.service_enabled(kind) {
            format!("{} не подключён", kind.display_name())
        } else {
            format!("{} выключен", kind.display_name())
        }
    }

    /// How long the service is still paused by a rate limit, if it says
    /// (see [`StreamService::paused_until`]). No network.
    pub(crate) fn pause_left(&self, kind: Service) -> Option<Duration> {
        let until = self.service(kind)?.paused_until()?;
        until.duration_since(std::time::SystemTime::now()).ok()
    }

    /// The service plays sound for this account right now (Spotify needs
    /// Premium and the playback login; YouTube plays without a login; a
    /// catalogue such as Deezer never does: its tracks play from YouTube
    /// Music, see [`resolve`]).
    pub(crate) fn has_audio(&self, kind: Service) -> bool {
        let Some(s) = self.service(kind) else {
            return false;
        };
        if s.catalogue_only() || s.audio_refused() {
            return false;
        }
        match kind {
            Service::Spotify => matches!(s.account(), Account::LoggedIn { audio: true, .. }),
            Service::Youtube | Service::Deezer | Service::Soundcloud => {
                !matches!(s.account(), Account::LoggedIn { audio: false, .. })
            }
        }
    }


    /// Spotify's Soloist, if the registered Spotify has one (switched on
    /// in the settings or not).
    fn spotify_soloist(&self) -> Option<(Arc<dyn StreamService>, nikgob_core::streaming::SoloistStatus)> {
        let s = self.streams.iter().find(|s| s.service() == Service::Spotify)?.clone();
        let status = s.soloist()?;
        Some((s, status))
    }

    /// Spotify's sound comes from Soloist now.
    pub(crate) fn soloist_ready(&self) -> bool {
        self.state.read().prefs.spotify_enabled && self.spotify_soloist().is_some_and(|(_, st)| st.ready())
    }

    /// Soloist runs only while Spotify is switched on.
    pub(crate) fn soloist_wanted(&mut self) {
        let on = self.state.read().prefs.spotify_enabled;
        if let Some(s) = self.streams.iter().find(|s| s.service() == Service::Spotify) {
            s.set_soloist_wanted(on);
        }
    }

    pub(crate) fn soloist_act(&mut self, action: nikgob_core::streaming::SoloistAction) {
        if let Some((s, _)) = self.spotify_soloist() {
            s.soloist_act(action);
        }
        self.soloist_poll();
    }

    /// Every second: Soloist's steps (download progress, pairing) show as
    /// they happen; the accounts are published when it changed.
    pub(crate) fn soloist_poll(&mut self) {
        let now = self.spotify_soloist().map(|(_, st)| st);
        if now != self.streaming.last_soloist {
            let was_ready = self.streaming.last_soloist.as_ref().is_some_and(|s| s.ready());
            let ready = now.as_ref().is_some_and(|s| s.ready());
            self.streaming.last_soloist = now;
            self.publish_accounts();
            if was_ready != ready {
                // What plays next may now come from Soloist, or no longer.
                self.preload_next();
                self.publish_now_playing();
            }
        }
    }

    /// The services' audio caches follow [`v::Prefs::stream_cache_mb`].
    pub(crate) fn apply_stream_cache(&mut self) {
        let bytes = u64::from(self.state.read().prefs.stream_cache_mb) * 1024 * 1024;
        for s in &self.streams {
            s.set_cache_limit(bytes);
        }
        self.publish_stream_cache();
    }

    pub(crate) fn publish_stream_cache(&mut self) {
        let bytes: u64 = self.streams.iter().map(|s| s.cache_size()).sum();
        if self.state.read().stream_cache_bytes != bytes {
            self.publish(|s| s.stream_cache_bytes = bytes);
        }
    }

    /// Start-up: accounts, the first check of likes (each service as soon
    /// as its login is restored), the wishlist view.
    pub(crate) fn start_streams(&mut self) {
        self.soloist_wanted();
        self.apply_stream_cache();
        self.retry_clip_misses_once();
        self.publish_accounts();
        self.likes_tick();
        self.publish_wishlist();
    }

    /// The window is hidden and nothing plays: the periodic network work
    /// waits (F20 of the request audit).
    pub(crate) fn streams_idle(&self) -> bool {
        !self.streaming.window_visible
            && !matches!(self.status, v::Status::Playing | v::Status::Buffering)
    }

    /// Every 5 s: accounts while logging in, the like checks, playlist
    /// sync, completing imported tracks (not while idle); the stream sound
    /// analysis; the wishlist scheduler (it also runs on every event) and
    /// the cache size once a minute.
    pub(crate) fn stream_tick(&mut self) {
        let idle = self.streams_idle();
        self.sync_tick(!idle);
        if !idle {
            self.tracklist_tick();
            self.enrich_tick();
            self.likes_tick();
            self.clips_tick();
        }
        self.stream_analysis_tick();
        self.publish_accounts();
        let every = self.streaming.schedule.wish_every;
        if self.streaming.last_wish_tick.is_none_or(|t| t.elapsed() >= every) {
            self.streaming.last_wish_tick = Some(Instant::now());
            self.publish_stream_cache();
            self.wish_tick();
            self.upgrade_tick();
        }
    }

    /// The window is shown (not in the tray, not minimized).
    pub(crate) fn window_visible(&self) -> bool {
        self.streaming.window_visible
    }

    /// How far the window is from the user, as a play's attention counts
    /// it: 0 focused, 0.5 in view behind another window, 1 hidden or
    /// minimised; `None` before the window said.
    pub(crate) fn window_away(&self) -> Option<f32> {
        let s = &self.streaming;
        s.window_known.then_some(if s.window_focused {
            0.0
        } else if s.window_visible {
            0.5
        } else {
            1.0
        })
    }

    /// `Command::WindowState`: a focus after a while checks the likes; the
    /// playing track's attention counts the state that ends here.
    fn window_state(&mut self, visible: bool, focused: bool) {
        self.heard_tick();
        let gained = focused && !self.streaming.window_focused;
        self.streaming.window_visible = visible;
        self.streaming.window_focused = focused;
        self.streaming.window_known = true;
        log::debug!("window: visible {visible}, focused {focused}");
        if visible && gained {
            self.likes_if_stale();
        }
    }

    /// Streaming commands; anything else is handed back.
    pub(crate) fn stream_command(&mut self, c: Command) -> Option<Command> {
        match c {
            Command::StreamLogin { service, input } => self.stream_login(service, input),
            Command::StreamLogout(service) => self.stream_logout(service),
            Command::Soloist(action) => self.soloist_act(action),
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
            Command::PlaySearch {
                tracks,
                start,
                context,
            } => self.play_search(&tracks, start, context),
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
            Command::DownloadStream { tracks } => self.download_stream(&tracks),
            Command::DownloadStreamAlbum(album) => self.with_album(album, pages::Then::Download),
            Command::DownloadTracks(tracks) => self.download_tracks(&tracks),
            Command::DownloadRelease(release) => self.download_release(release),
            Command::FollowStreamPlaylist(playlist) => self.with_playlist(playlist, pages::Then::Follow),
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
            Command::WishlistRestore(ids) => self.wish_restore(&ids),
            Command::WishlistRetry(id) => self.wish_retry(id),
            Command::WishlistRetryAll => self.wish_retry_all(),
            Command::WishlistSaveFromYoutube(id) => self.wish_save_from_youtube(id),
            Command::WishlistAttach { item, what } => self.wish_attach(item, what),
            Command::ImportFromService(service) => self.import_from_service(service),
            Command::ImportExportFiles(path) => self.import_export_files(path),
            Command::WindowState { visible, focused } => self.window_state(visible, focused),
            Command::ClearStreamCache => {
                for s in &self.streams {
                    s.clear_cache();
                }
                self.publish_stream_cache();
                self.toast("Кэш стриминга очищен", None);
            }
            Command::SetPlaylistSync { playlist, mode } => self.set_playlist_sync(playlist, mode),
            Command::LoadDiscography(artist) => self.load_discography(artist, true),
            Command::FindSong { track, query } => self.find_song(track, query),
            Command::CloseFindSong => self.publish(|s| s.find_song = None),
            Command::ReplaceTrack { track, with, youtube } => self.replace_track(track, with, youtube),
            Command::UndoReplace(id) => self.undo_replace(id),
            other => return Some(other),
        }
        None
    }

    pub(crate) fn stream_msg(&mut self, m: Msg) {
        match m {
            Msg::LoginDone(service, input, r) => self.login_done(service, input, r),
            Msg::WebLoginDone(service, r) => self.web_login_done(service, r),
            Msg::Searched {
                seq,
                service,
                result,
                partial,
            } => self.stream_searched(seq, service, result, partial),
            Msg::SearchRetry { seq, service } => self.search_retry(seq, service),
            Msg::Loaded { key, result } => self.loaded(key, result),
            Msg::Resolved { track, via, found, filled } => self.resolved(track, via, found, filled),
            Msg::IsrcFilled(track, full) => self.isrc_filled(track, &full),
            Msg::SearchSettle { seq } => self.search_settle(seq),
            Msg::ResolveRetry(track) => self.resolve_retry(track),
            Msg::LikesPulled(service, r) => self.likes_pulled(service, r),
            Msg::LikesHead(service, r) => self.likes_head(service, r),
            Msg::LikesConverted {
                service,
                liked,
                songs,
                misses,
                partial,
            } => self.likes_converted(service, &liked, &songs, &misses, partial),
            Msg::ClipSong { clip, result } => self.clip_song(*clip, result),
            Msg::LikeFlush(service) => self.flush_likes(service),
            Msg::LikesPushed {
                service,
                remotes,
                on,
                result,
            } => self.likes_pushed(service, &remotes, on, result),
            Msg::Wish(w) => self.wish_msg(w),
            Msg::Shared {
                seq,
                target,
                result,
            } => self.shared(seq, target, result),
            Msg::Cold(m) => self.cold_msg(m),
            Msg::Sync(m) => self.sync_msg(m),
            Msg::Disco(m) => self.disco_msg(m),
            Msg::Tracklist(m) => self.tracklist_msg(m),
            Msg::SongsFound { seq, service, result } => self.songs_found(seq, service, result),
            Msg::ReplaceResolved {
                track,
                pick,
                youtube_song,
                youtube,
            } => self.replace_resolved(track, *pick, youtube_song.map(|s| *s), youtube),
            Msg::Analysed(done) => self.stream_analysed(done),
            Msg::AnalysisWake => self.stream_analysis_step(),
            Msg::Completed { track, id, result } => self.completed(track, &id, result),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_known_tracks_drop_their_oldest_quarter_not_everything() {
        let mut k = Known::default();
        for i in 0..=KNOWN_MAX {
            k.insert(&StreamTrack::new(RemoteId::new(Service::Spotify, format!("t{i}")), "x"));
        }
        assert!(k.len() >= KNOWN_MAX * 3 / 4 - 1 && k.len() < KNOWN_MAX, "{}", k.len());
        assert!(k.contains_key(&RemoteId::new(Service::Spotify, format!("t{KNOWN_MAX}"))), "the newest stays");
        assert!(!k.contains_key(&RemoteId::new(Service::Spotify, "t0")), "the oldest went");
    }
}
