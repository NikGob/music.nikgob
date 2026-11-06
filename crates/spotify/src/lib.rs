//! Spotify as a [`StreamService`]: catalogue, library and playlists over the
//! Web API, sound over librespot. The only crate that knows about Spotify.
//!
//! The scheme follows [Spotifast](https://github.com/crmne/spotifast)
//! (MIT, Copyright (c) 2026 Carmine Paolino); the ported files say so in
//! their headers.
//!
//! # Spotify needs Premium (2026-10-17)
//!
//! The public Web API app several open-source clients share (Spotifast,
//! spotify-player, ncspot) is not used any more: its quota is shared by all
//! of their users and answered 429 with `Retry-After` 5–30 s even to a
//! first request. Without it Spotify works only with Premium, and without
//! Premium it is not connected at all (YouTube Music serves Free users):
//! a login whose `/me` says another plan, whose app Spotify refuses because
//! its owner has no Premium, or whose playback session is not Premium ends
//! signed out with [`SPOTIFY_PREMIUM_REQUIRED`], keeping nothing.
//!
//! # Grants
//!
//! * **Web API** ([`LoginInput::PersonalApp`]; [`LoginInput::Browser`]
//!   again with the remembered Client ID): the user's own Development Mode
//!   app (Spotify lets it work only while its owner has Premium) with its
//!   own quota. Every Web API request goes to it; the account is connected
//!   while it has this grant.
//! * **Playback** ([`LoginInput::Playback`]): Spotify's own desktop client
//!   identity, exchanged once for a reusable librespot credential. Its
//!   session plays audio and, when it is up, reads other people's playlists
//!   (Development Mode gives no entries of those), artists and track
//!   details without any Web API quota.
//!
//! Grants and the credential live in Windows Credential Manager; a small
//! state file (`state.json`, no secrets) in [`SpotifyConfig::data_dir`]
//! remembers which exist, the profile and the personal Client ID, so the
//! account shows at once on startup without a browser or a request. A
//! state file from before still listing the shared app's grant has it
//! deleted on start (see [`Spotify::new`]).
//!
//! # Quota
//!
//! Every grant has its own budget of 4 concurrent requests and its own
//! `Retry-After` pause ([`api`]); albums, artists, playlists and tracks are
//! cached in memory for a while; track details missing from album listings
//! come from the session or one batch request.

mod api;
mod audio;
mod auth;
mod cache;
mod convert;
mod credentials;
mod session;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use librespot_core::authentication::Credentials;
use nikgob_core::streaming::{
    Account, AudioStream, BoxFuture, EntityKind, LikedHead, LoginInput, LoginPrompt, Played,
    SPOTIFY_PREMIUM_REQUIRED, SearchKind, Service, StreamAlbum, StreamArtist, StreamError,
    StreamPlaylist, StreamSearch, StreamService, StreamTrack, SuggestedArtist, SuggestedTrack,
    SuggestionFeed, SuggestionKind, Suggestions, rank_weight,
};
use tokio::runtime::Handle;
use tokio::sync::watch;

use api::{ApiClient, Endpoints, Lane, TokenEvent};
use auth::{AuthError, Grant, StoredToken};
use cache::TtlCache;
use credentials::{PlaybackCredential, Profile, Secret, Slot, Store};

pub use auth::{PLAYBACK_REDIRECT_PORT, WEB_REDIRECT_PORT};

/// The redirect URI the user's app must register.
pub const WEB_REDIRECT_URI: &str = "http://127.0.0.1:8989/login";
/// Where a personal app is created.
pub const DEVELOPER_DASHBOARD: &str = "https://developer.spotify.com/dashboard";

/// Playlists change: a sync reads them fresh anyway ([`StreamService::forget`]).
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
/// Tracks and albums hardly ever change.
const CATALOG_TTL: Duration = Duration::from_secs(24 * 3600);
/// A discography gains a release now and then.
const ARTIST_TTL: Duration = Duration::from_secs(12 * 3600);
const SEARCH_TTL: Duration = Duration::from_secs(10 * 60);
/// `GET /me` is asked again only after this.
const PROFILE_TTL: Duration = Duration::from_secs(24 * 3600);
/// A `Retry-After` pause this long is shown as «paused until».
const SHOWN_PAUSE: Duration = Duration::from_secs(30);
/// After a failed session connect, reads use the Web API for this long.
const CONNECT_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Pages read at most from one list (a playlist, the liked songs).
const MAX_PAGES: u32 = 400;

#[derive(Clone, Debug)]
pub struct SpotifyConfig {
    /// The service's own folder inside the app's data dir (state file,
    /// librespot's temporary downloads).
    pub data_dir: PathBuf,
    /// A personal Client ID from settings; a [`LoginInput::PersonalApp`]
    /// login replaces it and is remembered.
    pub personal_client_id: Option<String>,
    pub user_agent: String,
}

impl SpotifyConfig {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            personal_client_id: None,
            user_agent: concat!("nikgob-music/", env!("CARGO_PKG_VERSION")).into(),
        }
    }
}

/// How the account relates to a playlist, which decides what reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Access {
    Owned,
    Collaborative,
    External,
    #[default]
    Unknown,
}

/// Whether the librespot session, when up, reads a playlist instead of the
/// Web API: playlists that are not the account's (a Development Mode app
/// gets no entries of those; Spotifast's `src/api/gateway.rs` sent them to
/// the shared app, which is gone). The account's own ones go to its app.
fn session_reads_playlist(access: Access) -> bool {
    matches!(access, Access::External | Access::Unknown)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A search text as the cache knows it: lower case, single spaces.
/// `tracks` as suggestions of `kind`, weighted by their place.
fn weighted(tracks: Vec<StreamTrack>, kind: SuggestionKind) -> Vec<SuggestedTrack> {
    let n = tracks.len();
    tracks
        .into_iter()
        .enumerate()
        .map(|(rank, track)| SuggestedTrack {
            track,
            kind,
            weight: rank_weight(rank, n),
        })
        .collect()
}

fn search_key(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// How long the tracks of an account whose audio keys Spotify refused
/// play from YouTube Music before Spotify is asked again.
pub const KEYS_RETRY: Duration = Duration::from_secs(12 * 3600);
/// Refusals in a row that mean the account's, not the track's.
const KEY_REFUSALS: u32 = 2;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What [`Spotify::account`] is computed from.
#[derive(Default)]
struct AccountState {
    saved: credentials::State,
    playback: Option<PlaybackCredential>,
    /// Grants are being read from the credential store.
    restoring: bool,
    /// `Some` while a login waits for the browser.
    prompt: Option<Option<LoginPrompt>>,
    failure: Option<String>,
    /// Audio keys refused in a row since the last track that opened.
    key_refusals: u32,
}

impl AccountState {
    /// Spotify refuses this account's audio keys lately ([`KEYS_RETRY`]).
    fn keys_refused(&self) -> bool {
        self.saved
            .keys_refused_at
            .is_some_and(|at| now_secs().saturating_sub(at) < KEYS_RETRY.as_secs())
    }

    fn premium(&self) -> Option<bool> {
        self.saved.session_premium.or_else(|| {
            self.saved
                .profile
                .as_ref()
                .and_then(|p| p.product.as_deref())
                .map(|p| p == "premium")
        })
    }

    fn add_slot(&mut self, slot: Slot) {
        if !self.saved.slots.contains(&slot) {
            self.saved.slots.push(slot);
        }
    }
}

pub struct Spotify {
    me: Weak<Spotify>,
    rt: Handle,
    http: reqwest::Client,
    /// The Web API, through the user's own app.
    web: ApiClient,
    store: Store,
    state: Mutex<AccountState>,
    sessions: session::Sessions,
    /// Bumped on logout: work started before it must not write its result.
    epoch: AtomicU64,
    /// The audio cache's size and when it was measured, and when a song was
    /// last opened (only that fills the cache): see [`CACHE_SETTLE`].
    cache_size: Mutex<CacheSize>,
    login_cancel: Mutex<Option<watch::Sender<bool>>>,
    access: Mutex<HashMap<String, Access>>,
    tracks: TtlCache<StreamTrack>,
    albums: TtlCache<StreamAlbum>,
    artists: TtlCache<StreamArtist>,
    playlists: TtlCache<StreamPlaylist>,
    searches: TtlCache<StreamSearch>,
    /// The last track that played, still open (see [`audio::Recent`]).
    recent: audio::Recent,
    /// Opens sign-in pages (the system browser; a stub in tests).
    opener: Mutex<auth::Opener>,
    /// When connecting the session for reads last failed.
    connect_failed: Mutex<Option<Instant>>,
}

/// The system browser; in tests a stub that only logs (a test reads the
/// sign-in URL from the account's prompt and answers the redirect itself),
/// unless the live tests run by hand.
fn default_opener() -> auth::Opener {
    #[cfg(test)]
    if !auth::live_tests() {
        return Arc::new(|url: &str| log::debug!("test: not opening {url}"));
    }
    Arc::new(auth::open_browser)
}

/// The default size limit of the encrypted audio cache.
pub const DEFAULT_CACHE_LIMIT: u64 = 500 * 1024 * 1024;

/// Bytes in a folder and its subfolders.
/// The audio cache only changes while songs stream: it is measured again
/// until this long after the last one was opened (librespot writes a file
/// in once it is all fetched), then the last size stands.
const CACHE_SETTLE: Duration = Duration::from_secs(15 * 60);

#[derive(Default)]
struct CacheSize {
    measured: Option<(u64, Instant)>,
    opened: Option<Instant>,
}

impl CacheSize {
    /// The size measured last, while it still holds.
    fn known(&self) -> Option<u64> {
        let (bytes, at) = self.measured?;
        self.opened.is_none_or(|o| at > o + CACHE_SETTLE).then_some(bytes)
    }
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

/// The runtime the service's work runs on: the caller's, or one of its own
/// when created outside any.
fn runtime() -> Handle {
    static OWN: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    Handle::try_current().unwrap_or_else(|_| {
        OWN.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_name("spotify")
                .enable_all()
                .build()
                .expect("spotify runtime")
        })
        .handle()
        .clone()
    })
}

/// librespot's hyper-rustls enables ring while reqwest uses aws-lc-rs, so
/// rustls cannot pick a provider by itself; ring is installed as the
/// process default unless something else already installed one.
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

impl Spotify {
    /// Creates the service and restores saved grants in the background
    /// (never blocking the caller). Uses the current tokio runtime, or a
    /// small one of its own when there is none.
    ///
    /// A state file from before 2026-10-17 that still lists the shared
    /// app's grant has it deleted from the credential store and dropped
    /// (one info line, no error and no browser): without a grant of the
    /// user's own app the account reads as not connected.
    pub fn new(config: SpotifyConfig) -> Arc<Self> {
        let store = Store::native(config.data_dir.clone());
        Self::with_parts(config, store, Endpoints::default())
    }

    /// `endpoints`: where Web API requests go (tests point them at a mock
    /// server).
    fn with_parts(config: SpotifyConfig, store: Store, endpoints: Endpoints) -> Arc<Self> {
        install_crypto_provider();
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        let device_id = {
            use sha2::Digest;
            let hash = sha2::Sha256::digest(config.data_dir.to_string_lossy().as_bytes());
            hash[..20].iter().map(|b| format!("{b:02x}")).collect()
        };
        let spotify = Arc::new_cyclic(|me: &Weak<Spotify>| Spotify {
            me: me.clone(),
            rt: runtime(),
            web: ApiClient::new(http.clone(), endpoints),
            http,
            store,
            state: Mutex::new(AccountState {
                restoring: true,
                saved: credentials::State {
                    personal_client_id: config.personal_client_id.clone(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            sessions: session::Sessions::new(device_id, config.data_dir.join("tmp")),
            epoch: AtomicU64::new(0),
            cache_size: Mutex::default(),
            login_cancel: Mutex::new(None),
            access: Mutex::new(HashMap::new()),
            tracks: TtlCache::new(CATALOG_TTL, 5000),
            albums: TtlCache::new(CATALOG_TTL, 300),
            artists: TtlCache::new(ARTIST_TTL, 150),
            playlists: TtlCache::new(CACHE_TTL, 50),
            searches: TtlCache::new(SEARCH_TTL, 100),
            recent: Default::default(),
            opener: Mutex::new(default_opener()),
            connect_failed: Mutex::new(None),
        });
        let me = spotify.me.clone();
        spotify.web.set_listener(Box::new(move |event| {
            if let Some(s) = me.upgrade() {
                s.token_event(event);
            }
        }));
        let me = spotify.clone();
        spotify
            .rt
            .spawn(async move { me.restore(config.personal_client_id).await });
        spotify
    }

    /// Replaces how sign-in pages are opened (the system browser).
    pub fn set_browser_opener(&self, open: auth::Opener) {
        *lock(&self.opener) = open;
    }

    /// Keeps played files in librespot's encrypted cache in `dir` (Spotify
    /// audio is never stored decrypted).
    pub fn with_audio_cache(self: Arc<Self>, dir: PathBuf) -> Arc<Self> {
        self.sessions.set_cache_dir(dir);
        self
    }

    fn arc(&self) -> Option<Arc<Self>> {
        self.me.upgrade()
    }

    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }


    /// The account's Spotify user id, once known.
    fn user_id(&self) -> Option<String> {
        lock(&self.state)
            .saved
            .profile
            .as_ref()
            .map(|p| p.id.clone())
    }

    fn persist(&self, st: &AccountState) {
        self.store.save_state(&st.saved);
    }

    /// The personal Client ID in use or remembered.
    pub fn personal_client_id(&self) -> Option<String> {
        self.web
            .client_id()
            .or_else(|| lock(&self.state).saved.personal_client_id.clone())
    }

    // ---- restore ---------------------------------------------------------

    async fn restore(self: Arc<Self>, configured_client: Option<String>) {
        let epoch = self.epoch();
        let mut saved = self.store.load_state().await;
        if saved.personal_client_id.is_none() {
            saved.personal_client_id = configured_client;
        }
        let mut kept = Vec::new();
        let mut playback = None;
        for slot in saved.slots.clone() {
            if slot == Slot::Shared {
                // The shared public app is not used any more: its grant is
                // deleted, quietly (docs/stage3.md §0, 2026-10-17).
                self.store.clear(Slot::Shared);
                log::info!("spotify: the shared Web API app's saved grant is removed (no longer used)");
                continue;
            }
            match self.store.load(slot).await {
                Ok(Some(Secret::Web(token))) => {
                    if epoch == self.epoch() {
                        self.web.install(token);
                    }
                    kept.push(slot);
                }
                Ok(Some(Secret::Playback(c))) => {
                    playback = Some(c);
                    kept.push(slot);
                }
                Ok(None) => log::info!("spotify: saved {slot:?} grant is gone"),
                Err(e) => {
                    // Locked or unavailable: keep the record for the next start.
                    log::warn!("spotify: cannot read the {slot:?} grant: {e}");
                    kept.push(slot);
                }
            }
        }
        if epoch != self.epoch() {
            return;
        }
        self.apply_probes(&saved);
        let has_web = self.web.has_grant();
        // The name and plan were read lately: no `GET /me` on this start.
        let profile_fresh = saved.profile.is_some()
            && saved
                .profile_at
                .is_some_and(|at| now_secs().saturating_sub(at) < PROFILE_TTL.as_secs());
        {
            let mut st = lock(&self.state);
            saved.slots = kept;
            st.saved = saved;
            st.playback = playback;
            st.restoring = false;
            self.persist(&st);
        }
        if !has_web {
            // Not connected: nothing is asked, not even the session.
            return;
        }
        if !profile_fresh {
            self.refresh_profile().await;
        }
        let (has_playback, premium) = {
            let st = lock(&self.state);
            (st.playback.clone(), st.premium())
        };
        match (has_playback, premium) {
            (_, Some(false)) => self.premium_required(),
            (Some(c), None) => {
                // The plan is not known yet; ask the session once.
                if let Ok(s) = self.sessions.get(c.to_librespot()).await {
                    self.note_session(&s).await;
                    if lock(&self.state).premium() == Some(false) {
                        self.premium_required();
                    }
                }
            }
            (Some(c), Some(true)) => {
                // Premium with the playback login: the session is connected
                // ahead, so the first play (and the ISRCs of album pages,
                // read over it without Web API quota) do not wait for it.
                if let Err(e) = self.sessions.get(c.to_librespot()).await {
                    log::info!("spotify: session not connected ahead: {e}");
                }
            }
            _ => {}
        }
    }

    /// Gives the client what earlier starts learnt about its app.
    fn apply_probes(&self, saved: &credentials::State) {
        let Some(id) = self.web.client_id() else { return };
        if let Some((_, p)) = saved.probes.iter().find(|(c, _)| *c == id) {
            self.web.set_probes(*p);
        }
    }

    /// One `GET /me` to refresh the name and plan.
    async fn refresh_profile(&self) {
        match self.web.me().await {
            Ok(me) if !me.id.is_empty() => {
                let mut st = lock(&self.state);
                let same = st.saved.profile.as_ref().is_none_or(|p| p.id == me.id);
                if !same {
                    log::warn!("spotify: the Web API grant is another account now");
                    return;
                }
                let product = me
                    .product
                    .or_else(|| st.saved.profile.as_ref().and_then(|p| p.product.clone()));
                st.saved.profile = Some(Profile {
                    name: me
                        .display_name
                        .filter(|n| !n.is_empty())
                        .unwrap_or(me.id.clone()),
                    id: me.id,
                    product,
                });
                st.saved.profile_at = Some(now_secs());
                self.persist(&st);
            }
            Ok(_) => {}
            // A refusal for want of Premium has signed out already (the
            // client's listener).
            Err(e) => log::info!("spotify: profile refresh failed: {e}"),
        }
    }

    /// The account has no Premium: Spotify is not connected without it.
    /// Every grant is signed out and forgotten (the Client ID stays, it is
    /// the user's app), and the account says why.
    fn premium_required(&self) {
        log::info!("spotify: the account has no Premium; signed out");
        self.sign_out(Some(SPOTIFY_PREMIUM_REQUIRED.into()));
    }

    /// Signs out of everything (the Client ID stays); `failure` is what the
    /// account says after. The grants go under the state lock together
    /// with the reset, so [`Account`] never shows a half-done sign-out (a
    /// plain `LoggedOut` before the reason, or a profile still kept).
    fn sign_out(&self, failure: Option<String>) {
        {
            let mut st = lock(&self.state);
            self.epoch.fetch_add(1, Ordering::SeqCst);
            self.web.clear();
            for slot in Slot::ALL {
                self.store.clear(slot);
            }
            let personal_client_id = st.saved.personal_client_id.take();
            *st = AccountState {
                saved: credentials::State {
                    personal_client_id,
                    ..Default::default()
                },
                failure,
                ..Default::default()
            };
            self.persist(&st);
        }
        if let Some(cancel) = lock(&self.login_cancel).take() {
            let _ = cancel.send(true);
        }
        self.sessions.close();
        self.clear_caches();
    }

    /// Records the account type a session reports.
    async fn note_session(&self, session: &librespot_core::Session) {
        let epoch = self.epoch();
        if let Some(premium) = session::premium(session).await {
            let mut st = lock(&self.state);
            if epoch == self.epoch() && st.saved.session_premium != Some(premium) {
                st.saved.session_premium = Some(premium);
                self.persist(&st);
            }
        }
    }

    fn token_event(&self, event: TokenEvent) {
        let slot = Slot::Personal;
        match event {
            TokenEvent::Refreshed(token) => {
                let store = self.store.clone();
                let generation = store.generation(slot);
                self.rt.spawn(async move {
                    if let Err(e) = store.save(slot, generation, Secret::Web(token)).await {
                        log::warn!("spotify: cannot save the refreshed {slot:?} grant: {e}");
                    }
                });
            }
            TokenEvent::Probed(p) => {
                let Some(id) = self.web.client_id() else { return };
                log::info!("spotify: the app lacks {p:?}; remembered");
                let mut st = lock(&self.state);
                st.saved.probes.retain(|(c, _)| *c != id);
                st.saved.probes.push((id, p));
                self.persist(&st);
            }
            TokenEvent::Rejected => {
                self.store.clear(slot);
                let mut st = lock(&self.state);
                st.saved.slots.retain(|s| *s != slot);
                st.failure = Some("Вход в Spotify истёк, войди снова".into());
                self.persist(&st);
            }
            TokenEvent::PremiumRequired => {
                // Not inside the request that tells: the logout clears the
                // client it runs on.
                if let Some(me) = self.arc() {
                    self.rt.spawn(async move { me.premium_required() });
                }
            }
        }
    }

    // ---- login -----------------------------------------------------------

    /// Records a failed login and turns it into the returned error.
    fn fail(&self, e: AuthError) -> StreamError {
        let message = e.to_string();
        log::warn!("spotify: login failed: {message}");
        // A cancelled login (logout, or another login) is not a failure
        // to show.
        if !matches!(e, AuthError::Cancelled) {
            lock(&self.state).failure = Some(message.clone());
        }
        match e {
            AuthError::Cancelled => StreamError::NotLoggedIn,
            AuthError::Rejected { .. } => StreamError::NotLoggedIn,
            _ => StreamError::Network(message),
        }
    }

    fn fail_text(&self, message: &str) -> StreamError {
        log::warn!("spotify: login failed: {message}");
        lock(&self.state).failure = Some(message.into());
        StreamError::Network(message.into())
    }

    /// Reserves the port, shows the prompt, opens the browser and waits
    /// for the code. The prompt is cleared when the returned guard drops.
    async fn browser_code(
        &self,
        grant: &Grant,
    ) -> Result<(String, String, PromptGuard<'_>), StreamError> {
        let listener = auth::listen_for_redirect(grant.redirect_port).map_err(|e| self.fail(e))?;
        let flow = auth::begin(grant);
        let (cancel, cancelled) = watch::channel(false);
        if let Some(old) = lock(&self.login_cancel).replace(cancel) {
            let _ = old.send(true);
        }
        {
            let mut st = lock(&self.state);
            st.prompt = Some(Some(LoginPrompt::OpenUrl(flow.url.clone())));
            st.failure = None;
        }
        let guard = PromptGuard(self, flow.url.clone());
        let open = lock(&self.opener).clone();
        open(&flow.url);
        let code = auth::wait_for_code(listener, &flow.state, cancelled)
            .await
            .map_err(|e| self.fail(e))?;
        Ok((code, flow.verifier, guard))
    }

    /// The Web API sign-in with the user's own app. Verified with `GET /me`
    /// on a scratch client before anything is kept: an account without
    /// Premium (its plan, or Spotify refusing the app for its owner) ends
    /// signed out with [`SPOTIFY_PREMIUM_REQUIRED`].
    async fn login_web(&self, grant: Grant) -> Result<(), StreamError> {
        let epoch = self.epoch();
        let endpoints = self.web.endpoints().clone();
        let (code, verifier, _prompt) = self.browser_code(&grant).await?;
        let response = auth::exchange_code(&self.http, &endpoints.token, &grant, &code, &verifier)
            .await
            .map_err(|e| self.fail(e))?;
        let token = StoredToken::from_response(&grant.client_id, response, None)
            .ok_or_else(|| self.fail_text("Spotify не выдал refresh-токен"))?;
        // Verify on a scratch client: nothing sees an unverified grant.
        let probe = ApiClient::new(self.http.clone(), endpoints);
        probe.install(token.clone());
        let me = match probe.me().await {
            Ok(me) => me,
            Err(e) if e.is_premium_refusal() => return Err(self.premium_login_failed(epoch)),
            Err(e) => {
                return Err(self.fail_text(&format!("Spotify не подтвердил аккаунт: {e}")));
            }
        };
        if epoch != self.epoch() {
            return Err(StreamError::NotLoggedIn);
        }
        if me.product.as_deref().is_some_and(|p| p != "premium") {
            return Err(self.premium_login_failed(epoch));
        }
        let known = self.user_id();
        let slot = Slot::Personal;
        let generation = self.store.generation(slot);
        self.web.install(token.clone());
        {
            let mut st = lock(&self.state);
            st.add_slot(slot);
            st.saved.personal_client_id = Some(grant.client_id.clone());
            let product = me.product.clone().or_else(|| {
                st.saved
                    .profile
                    .as_ref()
                    .filter(|p| p.id == me.id)
                    .and_then(|p| p.product.clone())
            });
            if known.as_deref() != Some(me.id.as_str()) {
                // Another account: what was cached about the old one goes.
                self.clear_caches();
            }
            st.saved.profile = Some(Profile {
                name: me
                    .display_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| me.id.clone()),
                id: me.id.clone(),
                product,
            });
            st.saved.profile_at = Some(now_secs());
            st.failure = None;
            self.persist(&st);
        }
        if let Err(e) = self.store.save(slot, generation, Secret::Web(token)).await {
            log::warn!("spotify: the {slot:?} grant works but is not remembered: {e}");
        }
        // A playback login kept from before that is known not to be Premium.
        if lock(&self.state).premium() == Some(false) {
            return Err(self.premium_login_failed(epoch));
        }
        Ok(())
    }

    /// A login found no Premium: nothing is kept, the account says why.
    fn premium_login_failed(&self, epoch: u64) -> StreamError {
        if epoch == self.epoch() {
            self.premium_required();
        }
        StreamError::PremiumRequired
    }

    async fn login_playback(&self) -> Result<(), StreamError> {
        let epoch = self.epoch();
        let grant = Grant::playback();
        let (code, verifier, _prompt) = self.browser_code(&grant).await?;
        let token_url = self.web.endpoints().token.clone();
        let response = auth::exchange_code(&self.http, &token_url, &grant, &code, &verifier)
            .await
            .map_err(|e| self.fail(e))?;
        let session = self
            .sessions
            .connect(Credentials::with_access_token(response.access_token))
            .await
            .map_err(|e| {
                self.fail_text(&format!("Spotify не пустил сессию воспроизведения: {e}"))
            })?;
        let credential = PlaybackCredential {
            username: session.username(),
            auth_data: session.auth_data(),
        };
        if epoch != self.epoch() {
            self.sessions.close();
            return Err(StreamError::NotLoggedIn);
        }
        if let Some(id) = self.user_id()
            && id != credential.username
        {
            log::warn!("spotify: the playback login is a different account than the Web API one");
        }
        let generation = self.store.generation(Slot::Playback);
        {
            let mut st = lock(&self.state);
            st.playback = Some(credential.clone());
            st.add_slot(Slot::Playback);
            st.saved.playback_username = Some(credential.username.clone());
            st.saved.session_premium = None;
            st.failure = None;
            self.persist(&st);
        }
        if let Err(e) = self
            .store
            .save(Slot::Playback, generation, Secret::Playback(credential))
            .await
        {
            log::warn!("spotify: the playback credential works but is not remembered: {e}");
        }
        self.note_session(&session).await;
        if lock(&self.state).saved.session_premium == Some(false) {
            return Err(self.premium_login_failed(epoch));
        }
        Ok(())
    }

    fn clear_caches(&self) {
        self.tracks.clear();
        self.albums.clear();
        self.artists.clear();
        self.playlists.clear();
        self.searches.clear();
        lock(&self.access).clear();
    }

    // ---- routing -----------------------------------------------------------

    /// The Web API client, when the user's app has a grant (every Web API
    /// request goes there).
    fn web(&self) -> Result<&ApiClient, StreamError> {
        if self.web.has_grant() {
            Ok(&self.web)
        } else {
            Err(StreamError::NotLoggedIn)
        }
    }

    fn access_of(&self, playlist: &str) -> Access {
        lock(&self.access)
            .get(playlist)
            .copied()
            .unwrap_or_default()
    }

    fn observe(&self, p: &api::models::Playlist) {
        let me = self.user_id();
        let access = if me.is_some() && p.owner.id == me {
            Access::Owned
        } else if p.collaborative {
            Access::Collaborative
        } else if p.owner.id.is_some() {
            Access::External
        } else {
            Access::Unknown
        };
        lock(&self.access).insert(p.id.clone(), access);
    }

    fn playback_credential(&self) -> Option<Credentials> {
        lock(&self.state)
            .playback
            .as_ref()
            .map(|c| c.to_librespot())
    }

    /// The session if it is up, else a new one when there is a credential
    /// (not again for [`CONNECT_BACKOFF`] after a failed connect, so reads
    /// that would like the session never wait on an unreachable one).
    async fn session_or_connect(&self) -> Option<librespot_core::Session> {
        if let Some(s) = self.sessions.current() {
            return Some(s);
        }
        let credential = self.playback_credential()?;
        if lock(&self.connect_failed).is_some_and(|t| t.elapsed() < CONNECT_BACKOFF) {
            return None;
        }
        match self.sessions.get(credential).await {
            Ok(s) => Some(s),
            Err(e) => {
                log::info!("spotify: no session for reads: {e}");
                *lock(&self.connect_failed) = Some(Instant::now());
                None
            }
        }
    }

    // ---- reads -------------------------------------------------------------

    /// One `/search` for every type at once (10 per type, the Development
    /// Mode cap), playlists included.
    async fn search_now(&self, query: &str) -> Result<StreamSearch, StreamError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(StreamSearch::default());
        }
        let key = search_key(query);
        if let Some(hit) = self.searches.get(&key) {
            log::debug!("spotify: search cache hit");
            return Ok(hit);
        }
        let client = self.web()?;
        let me = self.user_id();
        let started = Instant::now();
        let all = client
            .search(
                query,
                &["track", "album", "artist", "playlist"],
                api::SEARCH_LIMIT,
            )
            .await?;
        let mut out = self.catalog_part(&all);
        out.playlists = self.playlist_part(&all, me.as_deref());
        for t in &out.tracks {
            self.tracks.put(&t.id.id, t.clone());
        }
        log::debug!(
            "spotify: search done in {} ms: {} tracks, {} albums, {} artists, {} playlists",
            started.elapsed().as_millis(),
            out.tracks.len(),
            out.albums.len(),
            out.artists.len(),
            out.playlists.len()
        );
        self.searches.put(&key, out.clone());
        Ok(out)
    }

    /// One category of a search, in the background lane (resolution,
    /// discography, share never queue the user's search).
    async fn search_kind_now(&self, query: &str, kind: SearchKind) -> Result<StreamSearch, StreamError> {
        let query = query.trim();
        let types: &[&str] = match kind {
            SearchKind::Songs => &["track"],
            SearchKind::Albums => &["album"],
            SearchKind::Artists => &["artist"],
            // Spotify has no music videos.
            SearchKind::Videos => return Ok(StreamSearch::default()),
        };
        if query.is_empty() {
            return Ok(StreamSearch::default());
        }
        let key = format!("{kind:?}|{}", search_key(query));
        if let Some(hit) = self.searches.get(&key) {
            return Ok(hit);
        }
        let client = self.web()?;
        let r = client
            .search_in(Lane::Background, query, types, api::SEARCH_LIMIT)
            .await?;
        let out = self.catalog_part(&r);
        for t in &out.tracks {
            self.tracks.put(&t.id.id, t.clone());
        }
        self.searches.put(&key, out.clone());
        Ok(out)
    }

    /// Many tracks: cached ones as they are, the rest from the session when
    /// there is a playback login (one request per 200, no quota), else one
    /// `/tracks?ids=` per 50, else one by one.
    async fn tracks_now(&self, ids: &[String]) -> Result<Vec<Option<StreamTrack>>, StreamError> {
        let client = self.web()?;
        let mut found: HashMap<String, StreamTrack> = HashMap::new();
        let mut missing: Vec<String> = Vec::new();
        for id in ids {
            match self.tracks.get(id) {
                Some(t) => {
                    found.insert(id.clone(), t);
                }
                None if !missing.contains(id) => missing.push(id.clone()),
                None => {}
            }
        }
        if !missing.is_empty()
            && let Some(s) = self.session_or_connect().await
            && let Ok(map) = session::tracks(&s, &missing).await
        {
            missing.retain(|id| !map.contains_key(id));
            found.extend(map);
        }
        if !missing.is_empty() {
            let mut batched = true;
            for chunk in missing.chunks(50) {
                match client.tracks(chunk).await? {
                    Some(full) => found.extend(
                        full.iter()
                            .filter_map(|t| convert::track(t, None))
                            .map(|t| (t.id.id.clone(), t)),
                    ),
                    None => {
                        batched = false;
                        break;
                    }
                }
            }
            if !batched {
                let rest: Vec<String> = missing.iter().filter(|id| !found.contains_key(*id)).cloned().collect();
                for id in &rest {
                    match client.track(id).await {
                        Ok(t) => {
                            if let Some(t) = convert::track(&t, None) {
                                found.insert(id.clone(), t);
                            }
                        }
                        Err(e) if e.status() == Some(404) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
        }
        for t in found.values() {
            self.tracks.put(&t.id.id, t.clone());
        }
        Ok(ids.iter().map(|id| found.get(id).cloned()).collect())
    }

    async fn liked_head_now(&self) -> Result<LikedHead, StreamError> {
        let client = self.web()?;
        let page = client.saved_tracks(0).await?;
        Ok(LikedHead {
            tracks: page
                .items
                .iter()
                .filter_map(|s| convert::track(&s.track, None))
                .collect(),
            total: page.total,
            complete: page.next_offset().is_none(),
        })
    }

    /// The snapshot id alone (`fields=snapshot_id`), never cached.
    async fn playlist_version_now(&self, id: &str) -> Result<Option<String>, StreamError> {
        Ok(self.web()?.playlist_snapshot(id).await?)
    }

    fn catalog_part(&self, r: &api::models::SearchResults) -> StreamSearch {
        let mut out = StreamSearch::default();
        if let Some(page) = &r.tracks {
            out.tracks = page
                .items
                .iter()
                .filter_map(|t| convert::track(t, None))
                .collect();
        }
        if let Some(page) = &r.albums {
            out.albums = page.items.iter().filter_map(convert::album).collect();
        }
        if let Some(page) = &r.artists {
            out.artists = page.items.iter().filter_map(convert::artist).collect();
        }
        out
    }

    fn playlist_part(
        &self,
        r: &api::models::SearchResults,
        me: Option<&str>,
    ) -> Vec<StreamPlaylist> {
        let Some(page) = &r.playlists else {
            return Vec::new();
        };
        for p in &page.items {
            self.observe(p);
        }
        page.items
            .iter()
            .filter_map(|p| convert::playlist(p, me))
            .collect()
    }

    async fn track_now(&self, id: &str) -> Result<StreamTrack, StreamError> {
        if let Some(t) = self.tracks.get(id) {
            return Ok(t);
        }
        self.web()?;
        // The session spends no quota.
        if let Some(s) = self.sessions.current()
            && let Ok(t) = session::track_by_id(&s, id).await
        {
            self.tracks.put(id, t.clone());
            return Ok(t);
        }
        let result = match self.web() {
            Ok(c) => c.track(id).await.map_err(StreamError::from),
            Err(e) => Err(e),
        };
        let track = match result {
            Ok(t) => convert::track(&t, None).ok_or(StreamError::Unavailable)?,
            Err(e @ StreamError::RateLimited { .. }) => {
                let s = self.session_or_connect().await.ok_or(e)?;
                session::track_by_id(&s, id)
                    .await
                    .map_err(|_| StreamError::Unavailable)?
            }
            Err(e) => return Err(e),
        };
        self.tracks.put(id, track.clone());
        Ok(track)
    }

    /// Fills in the ISRCs album listings leave out: from the session when it
    /// is up (no quota), else one batch request per 50 tracks where the app
    /// still has the batch endpoint.
    async fn fill_isrcs(&self, client: &ApiClient, tracks: &mut [StreamTrack]) {
        let missing: Vec<String> = tracks
            .iter()
            .filter(|t| t.isrc.is_none())
            .map(|t| t.id.id.clone())
            .collect();
        if missing.is_empty() {
            return;
        }
        let mut found: HashMap<String, String> = HashMap::new();
        // With a playback login the session is connected for this: one
        // request for the whole album instead of the Web API.
        if let Some(s) = self.session_or_connect().await {
            if let Ok(map) = session::tracks(&s, &missing).await {
                found.extend(map.into_iter().filter_map(|(id, t)| Some((id, t.isrc?))));
            }
        } else {
            for chunk in missing.chunks(50) {
                match client.tracks(chunk).await {
                    Ok(Some(full)) => found.extend(full.iter().filter_map(|t| {
                        let t = convert::track(t, None)?;
                        Some((t.id.id, t.isrc?))
                    })),
                    _ => break,
                }
            }
        }
        for t in tracks.iter_mut().filter(|t| t.isrc.is_none()) {
            t.isrc = found.get(&t.id.id).cloned();
        }
    }

    async fn album_now(&self, id: &str) -> Result<StreamAlbum, StreamError> {
        if let Some(a) = self.albums.get(id) {
            return Ok(a);
        }
        let client = self.web()?;
        let raw = client.album(id).await?;
        let mut album = convert::album(&raw).ok_or(StreamError::Unavailable)?;
        let mut next = raw.tracks.as_ref().and_then(|p| p.next_offset());
        let mut pages = 0;
        while let Some(offset) = next
            && pages < MAX_PAGES
        {
            let page = client.album_tracks(id, offset).await?;
            album.tracks.extend(
                page.items
                    .iter()
                    .filter_map(|t| convert::track(t, Some(&raw))),
            );
            next = page.next_offset();
            pages += 1;
        }
        self.fill_isrcs(client, &mut album.tracks).await;
        self.albums.put(id, album.clone());
        Ok(album)
    }

    async fn artist_now(&self, id: &str) -> Result<StreamArtist, StreamError> {
        if let Some(a) = self.artists.get(id) {
            return Ok(a);
        }
        // Not connected: nothing is asked, not even the session.
        let client = self.web()?;
        // The session reads the whole discography in two requests without
        // quota (the personal app pages it 10 albums at a time).
        if let Some(s) = self.session_or_connect().await {
            match session::artist(&s, id).await {
                Ok(a) => {
                    self.artists.put(id, a.clone());
                    return Ok(a);
                }
                Err(e) => log::info!("spotify: artist {id} over the session: {e}"),
            }
        }
        let raw = client.artist(id).await?;
        let mut artist = convert::artist(&raw).ok_or(StreamError::Unavailable)?;
        let mut albums: Vec<api::models::Album> = Vec::new();
        let mut offset = Some(0);
        let mut pages = 0;
        while let Some(o) = offset
            && pages < MAX_PAGES / 10
        {
            let page = client.artist_albums(id, o).await?;
            albums.extend(page.items.iter().cloned());
            offset = page.next_offset();
            pages += 1;
        }
        // Newest first; the listing is grouped by kind.
        albums.sort_by(|a, b| b.release_date.cmp(&a.release_date));
        let mut seen = HashSet::new();
        artist.albums = albums
            .iter()
            .filter_map(convert::album)
            .filter(|a| seen.insert(a.id.id.clone()))
            .collect();
        self.artists.put(id, artist.clone());
        Ok(artist)
    }

    async fn playlist_now(&self, id: &str) -> Result<StreamPlaylist, StreamError> {
        if let Some(p) = self.playlists.get(id) {
            return Ok(p);
        }
        let access = self.access_of(id);
        let me = self.user_id();
        if session_reads_playlist(access)
            && let Some(s) = self.sessions.current()
            && let Ok(p) = session::playlist(&s, id, me.as_deref()).await
        {
            self.playlists.put(id, p.clone());
            return Ok(p);
        }
        let playlist = match self.playlist_web(id, me.as_deref()).await {
            Ok(p) => p,
            Err(StreamError::NotLoggedIn) => return Err(StreamError::NotLoggedIn),
            Err(e) => {
                // Development Mode gives no entries of other people's
                // playlists; the session has them.
                let s = self.session_or_connect().await.ok_or(e.clone())?;
                session::playlist(&s, id, me.as_deref())
                    .await
                    .map_err(|_| e)?
            }
        };
        self.playlists.put(id, playlist.clone());
        Ok(playlist)
    }

    async fn playlist_web(
        &self,
        id: &str,
        me: Option<&str>,
    ) -> Result<StreamPlaylist, StreamError> {
        let client = self.web()?;
        let raw = client.playlist(id).await?;
        self.observe(&raw);
        let mut playlist = convert::playlist(&raw, me).ok_or(StreamError::Unavailable)?;
        let total = raw.total().unwrap_or(0);
        let first = raw.entries().filter(|e| !e.items.is_empty());
        let (mut next, mut read) = match first {
            Some(e) => {
                playlist.tracks = convert::entries(e);
                let read = e.offset.unwrap_or(0) + e.items.len() as u32;
                (e.next.is_some().then_some(read), read)
            }
            None if total > 0 => (Some(0), 0),
            None => (None, 0),
        };
        let mut pages = 0;
        while let Some(offset) = next
            && pages < MAX_PAGES
        {
            let page = client.playlist_items(id, offset).await?;
            if page.items.is_empty() {
                break;
            }
            playlist.tracks.extend(convert::entries(&page));
            read = offset + page.items.len() as u32;
            next = page.next.is_some().then_some(read);
            pages += 1;
        }
        if total > 0 && read == 0 {
            return Err(StreamError::Unavailable);
        }
        Ok(playlist)
    }

    async fn liked_now(&self) -> Result<Vec<StreamTrack>, StreamError> {
        let client = self.web()?;
        let mut out = Vec::new();
        let mut offset = Some(0);
        let mut pages = 0;
        while let Some(o) = offset
            && pages < MAX_PAGES
        {
            let page = client.saved_tracks(o).await?;
            out.extend(
                page.items
                    .iter()
                    .filter_map(|s| convert::track(&s.track, None)),
            );
            offset = page.next_offset();
            pages += 1;
        }
        Ok(out)
    }

    async fn my_playlists_now(&self) -> Result<Vec<StreamPlaylist>, StreamError> {
        let client = self.web()?;
        let me = self.user_id();
        let mut out = Vec::new();
        let mut offset = Some(0);
        let mut pages = 0;
        while let Some(o) = offset
            && pages < MAX_PAGES
        {
            let page = client.my_playlists(o).await?;
            for p in &page.items {
                self.observe(p);
            }
            out.extend(
                page.items
                    .iter()
                    .filter_map(|p| convert::playlist(p, me.as_deref())),
            );
            offset = page.next_offset();
            pages += 1;
        }
        Ok(out)
    }

    async fn playlist_edit_now(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<Option<String>, StreamError> {
        let client = self.web()?;
        let uri = |id: &String| format!("spotify:track:{id}");
        let mut version = None;
        let removes: Vec<String> = remove.iter().map(uri).collect();
        for chunk in removes.chunks(100) {
            version = client.remove_items(id, chunk).await?.or(version);
        }
        let adds: Vec<String> = add.iter().map(uri).collect();
        for chunk in adds.chunks(100) {
            version = client.add_items(id, chunk).await?.or(version);
        }
        self.playlists.remove(id);
        Ok(version)
    }

    async fn recent_now(&self) -> Result<Vec<Played>, StreamError> {
        let client = self.web()?;
        let page = client.recently_played().await?;
        Ok(page
            .items
            .iter()
            .filter_map(|h| {
                Some(Played {
                    track: convert::track(&h.track, None)?,
                    at_ms: convert::rfc3339_ms(h.played_at.as_deref()?)?,
                })
            })
            .collect())
    }

    /// One feed of suggestions (docs/stage4.md §12.3): the user's top
    /// tracks or artists of about a month or half a year, or the recently
    /// played. A token from before `user-top-read` was asked gets a 403 on
    /// the tops: that is [`Suggestions::needs_reconnect`], not an error.
    async fn suggestions_now(&self, feed: SuggestionFeed) -> Result<Suggestions, StreamError> {
        let client = self.web()?;
        let top = |range| async move {
            match feed {
                SuggestionFeed::TopArtistsMonth | SuggestionFeed::TopArtistsHalfYear => {
                    client.top_artists(range).await.map(|p| (Vec::new(), p.items))
                }
                _ => client.top_tracks(range).await.map(|p| (p.items, Vec::new())),
            }
        };
        let (kind, answer) = match feed {
            SuggestionFeed::TopTracksMonth | SuggestionFeed::TopArtistsMonth => {
                (SuggestionKind::TopMonth, top("short_term").await)
            }
            SuggestionFeed::TopTracksHalfYear | SuggestionFeed::TopArtistsHalfYear => {
                (SuggestionKind::TopHalfYear, top("medium_term").await)
            }
            SuggestionFeed::RecentlyPlayed => {
                let page = client.recently_played().await?;
                let mut tracks: Vec<StreamTrack> = Vec::new();
                for t in page.items.iter().filter_map(|h| convert::track(&h.track, None)) {
                    if !tracks.iter().any(|x| x.id == t.id) {
                        tracks.push(t);
                    }
                }
                return Ok(Suggestions {
                    tracks: weighted(tracks, SuggestionKind::Recent),
                    ..Suggestions::default()
                });
            }
            _ => return Err(StreamError::Unsupported),
        };
        let (tracks, artists) = match answer {
            Ok(x) => x,
            Err(e) if e.status() == Some(403) && !e.is_premium_refusal() => {
                log::info!("spotify: {feed:?}: the sign-in lacks user-top-read");
                return Ok(Suggestions {
                    needs_reconnect: true,
                    ..Suggestions::default()
                });
            }
            Err(e) => return Err(e.into()),
        };
        let tracks: Vec<StreamTrack> = tracks.iter().filter_map(|t| convert::track(t, None)).collect();
        let artists: Vec<_> = artists
            .iter()
            .filter(|a| !a.id.is_empty() && !a.name.is_empty())
            .map(|a| nikgob_core::streaming::ArtistRef::new(Some(convert::rid(&a.id)), a.name.clone()))
            .collect();
        let n = artists.len();
        Ok(Suggestions {
            tracks: weighted(tracks, kind),
            artists: artists
                .into_iter()
                .enumerate()
                .map(|(rank, artist)| SuggestedArtist {
                    artist,
                    kind,
                    weight: rank_weight(rank, n),
                })
                .collect(),
            needs_reconnect: false,
        })
    }

    /// The artist's most popular tracks (up to 10), read over the playback
    /// session: the Web API removed this endpoint in February 2026. Needs
    /// the playback login.
    pub async fn artist_top_tracks(&self, id: &str) -> Result<Vec<StreamTrack>, StreamError> {
        self.web()?;
        let s = self
            .session_or_connect()
            .await
            .ok_or(StreamError::NotLoggedIn)?;
        session::artist_top_tracks(&s, id)
            .await
            .map_err(|e| StreamError::Network(e.to_string()))
    }

    // ---- audio -------------------------------------------------------------

    async fn open_track(&self, id: String) -> Result<audio::Opened, StreamError> {
        if let Some(o) = audio::take_recent(&self.recent, &id) {
            log::debug!("spotify: {id}: reopened from memory");
            return Ok(o);
        }
        let started = Instant::now();
        let r = self.open_track_now(&id).await;
        match &r {
            Ok(o) => log::debug!(
                "spotify: {id}: opened in {} ms (Vorbis {} kbps, {})",
                started.elapsed().as_millis(),
                o.kbps,
                if o.cached { "from the encrypted cache" } else { "downloading" }
            ),
            Err(e) => log::info!("spotify: {id}: did not open in {} ms: {e}", started.elapsed().as_millis()),
        }
        r
    }

    async fn open_track_now(&self, id: &str) -> Result<audio::Opened, StreamError> {
        let id = id.to_owned();
        let credential = self
            .playback_credential()
            .ok_or(StreamError::PremiumRequired)?;
        let premium = lock(&self.state).premium();
        for attempt in 0..2 {
            let session = if attempt == 0 {
                self.sessions.get(credential.clone()).await
            } else {
                log::info!("spotify: session dropped, reconnecting");
                self.sessions.connect(credential.clone()).await
            }
            .map_err(|e| StreamError::Network(e.to_string()))?;
            if let Some(false) = session.get_user_attribute("type").map(|t| t == "premium") {
                self.set_session_premium(false);
                return Err(StreamError::PremiumRequired);
            }
            lock(&self.cache_size).opened = Some(Instant::now());
            match audio::open(&session, &id, premium).await {
                Ok(opened) => {
                    self.note_key(true);
                    if let Some(me) = self.arc() {
                        let s = session.clone();
                        self.rt.spawn(async move { me.note_session(&s).await });
                    }
                    return Ok(opened);
                }
                Err(e) if e.retry_with_new_session && attempt == 0 => continue,
                Err(e) => {
                    if e.error == StreamError::PremiumRequired {
                        self.set_session_premium(false);
                    }
                    if e.key_refused {
                        self.note_key(false);
                    }
                    return Err(e.error);
                }
            }
        }
        Err(StreamError::Unavailable)
    }

    /// Counts the audio keys refused in a row: at [`KEY_REFUSALS`] the
    /// account's tracks play from YouTube Music for [`KEYS_RETRY`]; a track
    /// that opens ends it.
    fn note_key(&self, given: bool) {
        let mut st = lock(&self.state);
        if given {
            st.key_refusals = 0;
            if st.saved.keys_refused_at.take().is_some() {
                self.persist(&st);
            }
            return;
        }
        st.key_refusals += 1;
        if st.key_refusals >= KEY_REFUSALS && !st.keys_refused() {
            log::warn!("spotify: audio keys refused {} times in a row: tracks play from YouTube Music for {} h", st.key_refusals, KEYS_RETRY.as_secs() / 3600);
            st.saved.keys_refused_at = Some(now_secs());
            self.persist(&st);
        }
    }

    /// The session found the account is not Premium (a track refused):
    /// Spotify is not connected without it.
    fn set_session_premium(&self, premium: bool) {
        {
            let mut st = lock(&self.state);
            if st.saved.session_premium != Some(premium) {
                st.saved.session_premium = Some(premium);
                self.persist(&st);
            }
        }
        if !premium && self.web.has_grant() {
            self.premium_required();
        }
    }
}

/// Clears the login prompt when a login ends, however it ends, unless a
/// newer login has put up its own.
struct PromptGuard<'a>(&'a Spotify, String);

impl Drop for PromptGuard<'_> {
    fn drop(&mut self) {
        let mut st = lock(&self.0.state);
        if matches!(&st.prompt, Some(Some(LoginPrompt::OpenUrl(url))) if *url == self.1) {
            st.prompt = None;
        }
    }
}

impl StreamService for Spotify {
    fn service(&self) -> Service {
        Service::Spotify
    }

    fn audio_refused(&self) -> bool {
        lock(&self.state).keys_refused()
    }

    fn account(&self) -> Account {
        let st = lock(&self.state);
        if let Some(prompt) = &st.prompt {
            return Account::LoggingIn(prompt.clone());
        }
        // Connected while the user's app has a grant; a playback login kept
        // from before alone does not make it.
        if self.web.has_grant() {
            let name = st
                .saved
                .profile
                .as_ref()
                .map(|p| p.name.clone())
                .or_else(|| st.playback.as_ref().map(|c| c.username.clone()))
                .unwrap_or_else(|| "Spotify".into());
            let premium = st.premium();
            return Account::LoggedIn {
                name,
                premium,
                audio: st.playback.is_some() && premium == Some(true),
            };
        }
        if st.restoring {
            return Account::LoggingIn(None);
        }
        match &st.failure {
            Some(f) => Account::Failed(f.clone()),
            None => Account::LoggedOut,
        }
    }

    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>> {
        Box::pin(async move {
            match input {
                // Again with the Client ID the last sign-in used.
                LoginInput::Browser => match self.personal_client_id().as_deref().and_then(Grant::personal) {
                    Some(grant) => self.login_web(grant).await,
                    None => Err(StreamError::NotConfigured),
                },
                LoginInput::PersonalApp { client_id } => match Grant::personal(&client_id) {
                    Some(grant) => self.login_web(grant).await,
                    None => Err(StreamError::NotConfigured),
                },
                LoginInput::Playback => self.login_playback().await,
                LoginInput::Cookies(_) => Err(StreamError::Unsupported),
            }
        })
    }

    fn logout(&self) {
        self.sign_out(None);
    }

    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let query = query.to_string();
        Box::pin(async move { self.search_now(&query).await })
    }

    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>> {
        let id = id.to_string();
        Box::pin(async move { self.track_now(&id).await })
    }

    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>> {
        let id = id.to_string();
        Box::pin(async move { self.album_now(&id).await })
    }

    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>> {
        let id = id.to_string();
        Box::pin(async move { self.artist_now(&id).await })
    }

    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>> {
        let id = id.to_string();
        Box::pin(async move { self.playlist_now(&id).await })
    }

    fn find_isrc(&self, isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        let isrc = convert::isrc(Some(isrc));
        Box::pin(async move {
            let Some(isrc) = isrc else { return Ok(None) };
            let client = self.web()?;
            let found = client
                .search_in(Lane::Background, &format!("isrc:{isrc}"), &["track"], 1)
                .await?;
            Ok(found
                .tracks
                .and_then(|p| p.items.iter().find_map(|t| convert::track(t, None))))
        })
    }

    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        Box::pin(self.liked_now())
    }

    fn liked_head(&self) -> BoxFuture<'_, Result<LikedHead, StreamError>> {
        Box::pin(self.liked_head_now())
    }

    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        self.set_liked_many(std::slice::from_ref(&id.to_owned()), liked)
    }

    /// 40 ids per request.
    fn set_liked_many(&self, ids: &[String], liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        let ids = ids.to_vec();
        Box::pin(async move {
            let client = self.web()?;
            client.set_saved(&ids, liked).await.map_err(Into::into)
        })
    }

    fn search_kind(
        &self,
        query: &str,
        kind: SearchKind,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let query = query.to_owned();
        Box::pin(async move { self.search_kind_now(&query, kind).await })
    }

    fn tracks(&self, ids: &[String]) -> BoxFuture<'_, Result<Vec<Option<StreamTrack>>, StreamError>> {
        let ids = ids.to_vec();
        Box::pin(async move { self.tracks_now(&ids).await })
    }

    fn playlist_version(&self, id: &str) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        let id = id.to_owned();
        Box::pin(async move { self.playlist_version_now(&id).await })
    }

    fn forget(&self, kind: EntityKind, id: &str) {
        match kind {
            EntityKind::Track => self.tracks.remove(id),
            EntityKind::Album => self.albums.remove(id),
            EntityKind::Artist => self.artists.remove(id),
            EntityKind::Playlist => self.playlists.remove(id),
        }
    }

    /// The Web API's `Retry-After` pause, when it is a long one.
    fn paused_until(&self) -> Option<std::time::SystemTime> {
        let client = self.web().ok()?;
        let left = client.cooldown_left().filter(|l| *l >= SHOWN_PAUSE)?;
        Some(std::time::SystemTime::now() + left)
    }

    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>> {
        Box::pin(self.my_playlists_now())
    }

    fn create_playlist(&self, name: &str) -> BoxFuture<'_, Result<String, StreamError>> {
        let name = name.to_string();
        Box::pin(async move {
            let client = self.web()?;
            let me = self.user_id().unwrap_or_default();
            let p = client.create_playlist(&name, &me).await?;
            if p.id.is_empty() {
                return Err(StreamError::Parse("created playlist without id".into()));
            }
            lock(&self.access).insert(p.id.clone(), Access::Owned);
            Ok(p.id)
        })
    }

    fn playlist_edit(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        let (id, add, remove) = (id.to_string(), add.to_vec(), remove.to_vec());
        Box::pin(async move { self.playlist_edit_now(&id, &add, &remove).await })
    }

    fn recent(&self) -> BoxFuture<'_, Result<Vec<Played>, StreamError>> {
        Box::pin(self.recent_now())
    }

    fn suggestion_feeds(&self) -> &'static [SuggestionFeed] {
        &[
            SuggestionFeed::TopTracksMonth,
            SuggestionFeed::TopArtistsMonth,
            SuggestionFeed::TopTracksHalfYear,
            SuggestionFeed::TopArtistsHalfYear,
            SuggestionFeed::RecentlyPlayed,
        ]
    }

    fn suggestions(&self, feed: SuggestionFeed) -> BoxFuture<'_, Result<Suggestions, StreamError>> {
        Box::pin(self.suggestions_now(feed))
    }

    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        self.web()?;
        {
            let st = lock(&self.state);
            if st.playback.is_none() || st.premium() == Some(false) {
                return Err(StreamError::PremiumRequired);
            }
        }
        let me = self.arc().ok_or(StreamError::Unavailable)?;
        let id = id.to_string();
        let handle = self.rt.clone();
        let recent = self.recent.clone();
        let kept = id.clone();
        Ok(Box::new(
            audio::SpotifyStream::new(Box::new(move || {
                audio::block_on_runtime(&handle, async move { me.open_track(id).await })
            }))
            .keep_in(kept, recent),
        ))
    }

    fn cache_size(&self) -> u64 {
        if let Some(bytes) = lock(&self.cache_size).known() {
            return bytes;
        }
        let bytes = self.sessions.cache_dir().map_or(0, |d| dir_size(&d));
        lock(&self.cache_size).measured = Some((bytes, Instant::now()));
        bytes
    }

    fn set_cache_limit(&self, bytes: u64) {
        self.sessions.set_cache_limit(bytes);
        lock(&self.cache_size).measured = None;
    }

    fn clear_cache(&self) {
        lock(&self.cache_size).measured = None;
        if let Some(d) = self.sessions.cache_dir() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
            log::info!("spotify: audio cache cleared");
        }
    }

    fn save_audio(&self, _id: &str, _dir: &Path) -> BoxFuture<'_, Result<PathBuf, StreamError>> {
        // Never downloaded from Spotify (docs/stage3.md §0).
        Box::pin(async { Err(StreamError::Unsupported) })
    }

    fn web_url(&self, kind: EntityKind, id: &str) -> String {
        let kind = match kind {
            EntityKind::Track => "track",
            EntityKind::Album => "album",
            EntityKind::Artist => "artist",
            EntityKind::Playlist => "playlist",
        };
        format!("https://open.spotify.com/{kind}/{id}")
    }
}

#[cfg(test)]
mod tests;
