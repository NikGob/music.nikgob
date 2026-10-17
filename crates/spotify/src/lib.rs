//! Spotify as a [`StreamService`]: catalogue, library and playlists over the
//! Web API, sound over librespot. The only crate that knows about Spotify.
//!
//! The scheme follows [Spotifast](https://github.com/crmne/spotifast)
//! (MIT, Copyright (c) 2026 Carmine Paolino); the ported files say so in
//! their headers.
//!
//! # Grants
//!
//! * **Shared** ([`LoginInput::Browser`]): the public Web API app several
//!   open-source clients share. Its quota is shared by all of their users,
//!   so it is asked little: playlists in search and other people's
//!   playlists, and everything when there is no personal app.
//! * **Personal** ([`LoginInput::PersonalApp`]): the user's own Development
//!   Mode app with its own quota. It serves catalogue search, likes and the
//!   user's own playlists. Both must be the same Spotify account.
//! * **Playback** ([`LoginInput::Playback`]): Spotify's own desktop client
//!   identity, exchanged once for a reusable librespot credential. Its
//!   session plays audio (Premium only) and, when it is up, reads playlists
//!   and artists' top tracks without any Web API quota.
//!
//! Grants and the credential live in Windows Credential Manager; a small
//! state file (`state.json`, no secrets) in [`SpotifyConfig::data_dir`]
//! remembers which exist, the profile and the personal Client ID, so the
//! account shows at once on startup without a browser or a request.
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
use std::time::Duration;

use librespot_core::authentication::Credentials;
use nikgob_core::streaming::{
    Account, AudioStream, BoxFuture, EntityKind, LoginInput, LoginPrompt, Played, Service,
    StreamAlbum, StreamArtist, StreamError, StreamPlaylist, StreamSearch, StreamService,
    StreamTrack,
};
use tokio::runtime::Handle;
use tokio::sync::watch;

use api::{ApiClient, Endpoints, Source, TokenEvent};
use auth::{AuthError, Grant, StoredToken};
use cache::TtlCache;
use credentials::{PlaybackCredential, Profile, Secret, Slot, Store};

pub use auth::{PLAYBACK_REDIRECT_PORT, SHARED_CLIENT_ID, WEB_REDIRECT_PORT};

/// The redirect URI a personal app must register (the same as the shared
/// app's).
pub const WEB_REDIRECT_URI: &str = "http://127.0.0.1:8989/login";
/// Where a personal app is created.
pub const DEVELOPER_DASHBOARD: &str = "https://developer.spotify.com/dashboard";

const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const SEARCH_TTL: Duration = Duration::from_secs(2 * 60);
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

/// How the account relates to a playlist, which decides the grant reading it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Access {
    Owned,
    Collaborative,
    External,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// Catalogue search and reads (tracks, albums, artists).
    Catalog,
    /// Liked songs and listening history.
    UserData,
    /// The account's playlist list.
    PlaylistLibrary,
    PlaylistSearch,
    PlaylistCreate,
    PlaylistRead(Access),
    PlaylistEdit(Access),
}

/// The grant an operation goes to. Ported from Spotifast's
/// `src/api/gateway.rs` routing: the personal app takes what is the
/// account's own; the shared app keeps the playlist part of search, the
/// playlist list, and playlists that are not the account's (no personal
/// app may read those in Development Mode).
fn plan(op: Op, personal_ready: bool) -> Source {
    use Access::*;
    match op {
        Op::PlaylistLibrary | Op::PlaylistSearch => Source::Shared,
        Op::PlaylistRead(External | Unknown) | Op::PlaylistEdit(External | Unknown) => {
            Source::Shared
        }
        _ if personal_ready => Source::Personal,
        _ => Source::Shared,
    }
}

/// Whether the librespot session, when up, reads a playlist instead of the
/// Web API: whatever the shared app would have been asked for.
fn session_reads_playlist(access: Access, personal_ready: bool) -> bool {
    plan(Op::PlaylistRead(access), personal_ready) == Source::Shared
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
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
}

impl AccountState {
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
    shared: ApiClient,
    personal: ApiClient,
    store: Store,
    state: Mutex<AccountState>,
    sessions: session::Sessions,
    /// Bumped on logout: work started before it must not write its result.
    epoch: AtomicU64,
    login_cancel: Mutex<Option<watch::Sender<bool>>>,
    access: Mutex<HashMap<String, Access>>,
    tracks: TtlCache<StreamTrack>,
    albums: TtlCache<StreamAlbum>,
    artists: TtlCache<StreamArtist>,
    playlists: TtlCache<StreamPlaylist>,
    searches: TtlCache<StreamSearch>,
    /// Opens sign-in pages (the system browser; a stub in tests).
    opener: Mutex<auth::Opener>,
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
    pub fn new(config: SpotifyConfig) -> Arc<Self> {
        let store = Store::native(config.data_dir.clone());
        Self::with_parts(config, store, Endpoints::default(), Endpoints::default())
    }

    /// `shared` / `personal`: where each grant's requests go (tests point
    /// them at mock servers).
    fn with_parts(
        config: SpotifyConfig,
        store: Store,
        shared: Endpoints,
        personal: Endpoints,
    ) -> Arc<Self> {
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
            shared: ApiClient::new(Source::Shared, http.clone(), shared),
            personal: ApiClient::new(Source::Personal, http.clone(), personal),
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
            login_cancel: Mutex::new(None),
            access: Mutex::new(HashMap::new()),
            tracks: TtlCache::new(CACHE_TTL, 2000),
            albums: TtlCache::new(CACHE_TTL, 200),
            artists: TtlCache::new(CACHE_TTL, 100),
            playlists: TtlCache::new(CACHE_TTL, 50),
            searches: TtlCache::new(SEARCH_TTL, 50),
            opener: Mutex::new(default_opener()),
        });
        for client in [&spotify.shared, &spotify.personal] {
            let me = spotify.me.clone();
            client.set_listener(Box::new(move |source, event| {
                if let Some(s) = me.upgrade() {
                    s.token_event(source, event);
                }
            }));
        }
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

    fn arc(&self) -> Option<Arc<Self>> {
        self.me.upgrade()
    }

    fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    fn client(&self, source: Source) -> &ApiClient {
        match source {
            Source::Shared => &self.shared,
            Source::Personal => &self.personal,
        }
    }

    fn slot_of(source: Source) -> Slot {
        match source {
            Source::Shared => Slot::Shared,
            Source::Personal => Slot::Personal,
        }
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
        self.personal
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
            match self.store.load(slot).await {
                Ok(Some(Secret::Web(token))) => {
                    if epoch == self.epoch() {
                        let source = match slot {
                            Slot::Personal => Source::Personal,
                            _ => Source::Shared,
                        };
                        self.client(source).install(token);
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
        let has_web = self.shared.has_grant() || self.personal.has_grant();
        {
            let mut st = lock(&self.state);
            saved.slots = kept;
            st.saved = saved;
            st.playback = playback;
            st.restoring = false;
            self.persist(&st);
        }
        if has_web {
            self.refresh_profile().await;
        }
        let (has_playback, premium) = {
            let st = lock(&self.state);
            (st.playback.clone(), st.saved.session_premium)
        };
        if let (Some(c), None) = (has_playback, premium) {
            // The plan decides whether there is sound; ask the session once.
            if let Ok(s) = self.sessions.get(c.to_librespot()).await {
                self.note_session(&s).await;
            }
        }
    }

    /// One `GET /me` to refresh the name and plan (the personal grant
    /// first, to spare the shared quota).
    async fn refresh_profile(&self) {
        let client = if self.personal.has_grant() {
            &self.personal
        } else {
            &self.shared
        };
        match client.me().await {
            Ok(me) if !me.id.is_empty() => {
                let mut st = lock(&self.state);
                let same = st.saved.profile.as_ref().is_none_or(|p| p.id == me.id);
                if !same {
                    log::warn!(
                        "spotify: the {} grant is another account now",
                        client.source
                    );
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
                self.persist(&st);
            }
            Ok(_) => {}
            Err(e) => log::info!("spotify: profile refresh failed: {e}"),
        }
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

    fn token_event(&self, source: Source, event: TokenEvent) {
        let slot = Self::slot_of(source);
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
            TokenEvent::Rejected => {
                self.store.clear(slot);
                let mut st = lock(&self.state);
                st.saved.slots.retain(|s| *s != slot);
                if !self.shared.has_grant() && !self.personal.has_grant() && st.playback.is_none() {
                    st.failure = Some("Вход в Spotify истёк, войди снова".into());
                }
                self.persist(&st);
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

    async fn login_web(&self, source: Source, grant: Grant) -> Result<(), StreamError> {
        let epoch = self.epoch();
        let endpoints = self.client(source).endpoints().clone();
        let (code, verifier, _prompt) = self.browser_code(&grant).await?;
        let response = auth::exchange_code(&self.http, &endpoints.token, &grant, &code, &verifier)
            .await
            .map_err(|e| self.fail(e))?;
        let token = StoredToken::from_response(&grant.client_id, response, None)
            .ok_or_else(|| self.fail_text("Spotify не выдал refresh-токен"))?;
        // Verify on a scratch client: routing never sees an unverified grant.
        let probe = ApiClient::new(source, self.http.clone(), endpoints);
        probe.install(token.clone());
        let me = probe
            .me()
            .await
            .map_err(|e| self.fail_text(&format!("Spotify не подтвердил аккаунт: {e}")))?;
        if epoch != self.epoch() {
            return Err(StreamError::NotLoggedIn);
        }
        let other = match source {
            Source::Shared => &self.personal,
            Source::Personal => &self.shared,
        };
        let known = self.user_id();
        if other.has_grant() && known.as_deref().is_some_and(|id| id != me.id) {
            return Err(self.fail_text(match source {
                Source::Personal => {
                    "Личное приложение вошло другим аккаунтом Spotify: войди тем же аккаунтом"
                }
                Source::Shared => "Это другой аккаунт Spotify, чем у личного приложения",
            }));
        }
        let slot = Self::slot_of(source);
        let generation = self.store.generation(slot);
        self.client(source).install(token.clone());
        {
            let mut st = lock(&self.state);
            st.add_slot(slot);
            if source == Source::Personal {
                st.saved.personal_client_id = Some(grant.client_id.clone());
            }
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
            st.failure = None;
            self.persist(&st);
        }
        if let Err(e) = self.store.save(slot, generation, Secret::Web(token)).await {
            log::warn!("spotify: the {slot:?} grant works but is not remembered: {e}");
        }
        Ok(())
    }

    async fn login_playback(&self) -> Result<(), StreamError> {
        let epoch = self.epoch();
        let grant = Grant::playback();
        let (code, verifier, _prompt) = self.browser_code(&grant).await?;
        let token_url = self.shared.endpoints().token.clone();
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

    /// The client for an operation: the planned one, else the other when
    /// only that one has a grant.
    fn client_for(&self, op: Op) -> Result<&ApiClient, StreamError> {
        let planned = self.client(plan(op, self.personal.has_grant()));
        if planned.has_grant() {
            return Ok(planned);
        }
        [&self.shared, &self.personal]
            .into_iter()
            .find(|c| c.has_grant())
            .ok_or(StreamError::NotLoggedIn)
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

    /// The session if it is up, else a new one when there is a credential.
    async fn session_or_connect(&self) -> Option<librespot_core::Session> {
        if let Some(s) = self.sessions.current() {
            return Some(s);
        }
        let credential = self.playback_credential()?;
        self.sessions.get(credential).await.ok()
    }

    // ---- reads -------------------------------------------------------------

    async fn search_now(&self, query: &str) -> Result<StreamSearch, StreamError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(StreamSearch::default());
        }
        if let Some(hit) = self.searches.get(query) {
            return Ok(hit);
        }
        let catalog = self.client_for(Op::Catalog)?;
        let playlists = self.client_for(Op::PlaylistSearch)?;
        let me = self.user_id();
        let mut out = StreamSearch::default();
        let (catalog_results, playlist_results) = if std::ptr::eq(catalog, playlists) {
            let all = catalog
                .search(
                    query,
                    &["track", "album", "artist", "playlist"],
                    api::SEARCH_LIMIT,
                )
                .await?;
            (all.clone(), Ok(all))
        } else {
            let (c, p) = tokio::join!(
                catalog.search(query, &["track", "album", "artist"], api::SEARCH_LIMIT),
                playlists.search(query, &["playlist"], api::SEARCH_LIMIT)
            );
            (c?, p)
        };
        if let Some(page) = &catalog_results.tracks {
            out.tracks = page
                .items
                .iter()
                .filter_map(|t| convert::track(t, None))
                .collect();
        }
        if let Some(page) = &catalog_results.albums {
            out.albums = page.items.iter().filter_map(convert::album).collect();
        }
        if let Some(page) = &catalog_results.artists {
            out.artists = page.items.iter().filter_map(convert::artist).collect();
        }
        match playlist_results {
            Ok(r) => {
                if let Some(page) = r.playlists {
                    for p in &page.items {
                        self.observe(p);
                    }
                    out.playlists = page
                        .items
                        .iter()
                        .filter_map(|p| convert::playlist(p, me.as_deref()))
                        .collect();
                }
            }
            // The shared app being throttled must not hide the catalogue.
            Err(e) => log::info!("spotify: playlist search failed: {e}"),
        }
        for t in &out.tracks {
            self.tracks.put(&t.id.id, t.clone());
        }
        self.searches.put(query, out.clone());
        Ok(out)
    }

    async fn track_now(&self, id: &str) -> Result<StreamTrack, StreamError> {
        if let Some(t) = self.tracks.get(id) {
            return Ok(t);
        }
        // The session spends no quota.
        if let Some(s) = self.sessions.current()
            && let Ok(t) = session::track_by_id(&s, id).await
        {
            self.tracks.put(id, t.clone());
            return Ok(t);
        }
        let result = match self.client_for(Op::Catalog) {
            Ok(c) => c.track(id).await.map_err(StreamError::from),
            Err(e) => Err(e),
        };
        let track = match result {
            Ok(t) => convert::track(&t, None).ok_or(StreamError::Unavailable)?,
            Err(e @ (StreamError::NotLoggedIn | StreamError::RateLimited { .. })) => {
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
        if let Some(s) = self.sessions.current() {
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
        let client = self.client_for(Op::Catalog)?;
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
        let client = self.client_for(Op::Catalog)?;
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
        let via_session = session_reads_playlist(access, self.personal.has_grant());
        if via_session
            && let Some(s) = self.sessions.current()
            && let Ok(p) = session::playlist(&s, id, me.as_deref()).await
        {
            self.playlists.put(id, p.clone());
            return Ok(p);
        }
        let playlist = match self.playlist_web(id, access, me.as_deref()).await {
            Ok(p) => p,
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
        access: Access,
        me: Option<&str>,
    ) -> Result<StreamPlaylist, StreamError> {
        let client = self.client_for(Op::PlaylistRead(access))?;
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
        let client = self.client_for(Op::UserData)?;
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
        let client = self.client_for(Op::PlaylistLibrary)?;
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
        let client = self.client_for(Op::PlaylistEdit(self.access_of(id)))?;
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
        let client = self.client_for(Op::UserData)?;
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

    /// The artist's most popular tracks (up to 10), read over the playback
    /// session: the Web API removed this endpoint in February 2026. Needs
    /// the playback login.
    pub async fn artist_top_tracks(&self, id: &str) -> Result<Vec<StreamTrack>, StreamError> {
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
            match audio::open(&session, &id, premium).await {
                Ok(opened) => {
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
                    return Err(e.error);
                }
            }
        }
        Err(StreamError::Unavailable)
    }

    fn set_session_premium(&self, premium: bool) {
        let mut st = lock(&self.state);
        if st.saved.session_premium != Some(premium) {
            st.saved.session_premium = Some(premium);
            self.persist(&st);
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

    fn account(&self) -> Account {
        let st = lock(&self.state);
        if let Some(prompt) = &st.prompt {
            return Account::LoggingIn(prompt.clone());
        }
        if self.shared.has_grant() || self.personal.has_grant() || st.playback.is_some() {
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
                LoginInput::Browser => self.login_web(Source::Shared, Grant::shared()).await,
                LoginInput::PersonalApp { client_id } => match Grant::personal(&client_id) {
                    Some(grant) => self.login_web(Source::Personal, grant).await,
                    None => Err(StreamError::NotConfigured),
                },
                LoginInput::Playback => self.login_playback().await,
                LoginInput::Cookies(_) => Err(StreamError::Unsupported),
            }
        })
    }

    fn logout(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(cancel) = lock(&self.login_cancel).take() {
            let _ = cancel.send(true);
        }
        self.shared.clear();
        self.personal.clear();
        for slot in Slot::ALL {
            self.store.clear(slot);
        }
        self.sessions.close();
        self.clear_caches();
        let mut st = lock(&self.state);
        let personal_client_id = st.saved.personal_client_id.take();
        *st = AccountState {
            saved: credentials::State {
                personal_client_id,
                ..Default::default()
            },
            ..Default::default()
        };
        self.persist(&st);
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
            let client = self.client_for(Op::Catalog)?;
            let found = client
                .search(&format!("isrc:{isrc}"), &["track"], 1)
                .await?;
            Ok(found
                .tracks
                .and_then(|p| p.items.iter().find_map(|t| convert::track(t, None))))
        })
    }

    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        Box::pin(self.liked_now())
    }

    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        let id = id.to_string();
        Box::pin(async move {
            let client = self.client_for(Op::UserData)?;
            client.set_saved(&[id], liked).await.map_err(Into::into)
        })
    }

    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>> {
        Box::pin(self.my_playlists_now())
    }

    fn create_playlist(&self, name: &str) -> BoxFuture<'_, Result<String, StreamError>> {
        let name = name.to_string();
        Box::pin(async move {
            let client = self.client_for(Op::PlaylistCreate)?;
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

    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        {
            let st = lock(&self.state);
            if st.playback.is_none() || st.premium() == Some(false) {
                return Err(StreamError::PremiumRequired);
            }
        }
        let me = self.arc().ok_or(StreamError::Unavailable)?;
        let id = id.to_string();
        let handle = self.rt.clone();
        Ok(Box::new(audio::SpotifyStream::new(Box::new(move || {
            audio::block_on_runtime(&handle, async move { me.open_track(id).await })
        }))))
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
