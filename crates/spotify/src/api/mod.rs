//! An authenticated, rate-limited Spotify Web API client, one per grant.
//!
//! Ported (simplified) from Spotifast `src/api/client.rs`
//! (https://github.com/crmne/spotifast).
//! Copyright (c) 2026 Carmine Paolino, MIT License.
//!
//! Each grant (the shared public app, the user's own app) has its own
//! client: its own token, its own budget of [`MAX_IN_FLIGHT`] concurrent
//! requests, and its own `Retry-After` cooldown, so a throttled shared app
//! never pauses the personal one. The shared app's quota is shared by every
//! user of several open-source clients: requests are few, small and paged.

pub mod models;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use nikgob_core::streaming::StreamError;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::auth::{self, AuthError, StoredToken};
use models::*;

pub const API_URL: &str = "https://api.spotify.com/v1";
/// Concurrent requests per grant.
pub const MAX_IN_FLIGHT: usize = 4;
/// Retries of one request after a 429.
const RATE_LIMIT_RETRIES: u32 = 2;
/// A longer `Retry-After` is not waited out: the caller gets
/// [`ApiError::RateLimited`] at once (and later calls too, until it passes).
pub const MAX_RETRY_WAIT: Duration = Duration::from_secs(30);
/// The longest `Retry-After` pause an interactive request (search) waits
/// out; a longer one answers [`ApiError::RateLimited`] at once.
pub const INTERACTIVE_WAIT: Duration = Duration::from_millis(1500);
/// Concurrent interactive requests per grant, besides [`MAX_IN_FLIGHT`].
const INTERACTIVE_IN_FLIGHT: usize = 2;
/// Background requests of one grant start at least this far apart (~4 per
/// second): a burst of paging never hammers the shared quota.
pub const BACKGROUND_SPACING: Duration = Duration::from_millis(250);
/// Waiters woken after a `Retry-After` pause are spread over this much, so
/// they do not all hit Spotify in the same instant.
const WAKE_JITTER_MS: u64 = 500;
/// Ids per library write (`/me/library` takes at most 40 URIs).
pub const SAVE_CHUNK: usize = 40;

/// How a request waits for its turn (see [`ApiClient::send_in`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Background,
    Interactive,
}

/// The path without ids and the query, for the log ("/albums/{id}/tracks").
fn endpoint_name(path: &str) -> String {
    let path = path.split('?').next().unwrap_or_default();
    let path = path.strip_prefix(API_URL).unwrap_or(path);
    path.split('/')
        .map(|seg| {
            if seg.len() >= 16 && seg.chars().all(|c| c.is_ascii_alphanumeric()) {
                "{id}"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}
/// Development Mode caps every search type at 10 results.
pub const SEARCH_LIMIT: u32 = 10;
pub const PAGE: u32 = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Shared,
    Personal,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Shared => "shared",
            Self::Personal => "personal",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("not signed in")]
    NotSignedIn,
    /// The refresh token was rejected; the grant is gone.
    #[error("sign-in expired")]
    SignInExpired,
    #[error("HTTP {status}: {message}")]
    Status { status: u16, message: String },
    #[error("rate limited")]
    RateLimited { retry_after: Option<Duration> },
    #[error("network: {0}")]
    Network(String),
    #[error("decode: {0}")]
    Decode(String),
}

impl ApiError {
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The endpoint is not there for this app (Development Mode removed
    /// it, or the app may not read this item): worth one fallback.
    pub fn is_refusal(&self) -> bool {
        matches!(self.status(), Some(400 | 403 | 404 | 405 | 410))
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_decode() {
            Self::Decode(e.without_url().to_string())
        } else {
            Self::Network(e.without_url().to_string())
        }
    }
}

impl From<ApiError> for StreamError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::NotSignedIn | ApiError::SignInExpired => Self::NotLoggedIn,
            ApiError::Status { status: 401, .. } => Self::NotLoggedIn,
            ApiError::Status { status: 404, .. } => Self::Unavailable,
            ApiError::Status { status, .. } => Self::Http { status },
            ApiError::RateLimited { retry_after } => Self::RateLimited { retry_after },
            ApiError::Network(m) => Self::Network(m),
            ApiError::Decode(m) => Self::Parse(m),
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;

/// What happened to a client's grant, for persistence.
pub enum TokenEvent {
    /// A refresh minted a new token (save it).
    Refreshed(StoredToken),
    /// Spotify rejected the refresh token: the grant was dropped.
    Rejected,
    /// The client learnt what this app lacks ([`Probes`]): remember it.
    Probed(Probes),
}

/// What an app was found to lack, so later starts do not ask again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Probes {
    /// `GET /tracks?ids=` is removed (Development Mode).
    #[serde(default)]
    pub no_batch: bool,
    /// `/me/library` is not there; the classic `/me/tracks` is.
    #[serde(default)]
    pub legacy_library: bool,
    /// `/playlists/{id}/items` answers 404; `/tracks` works.
    #[serde(default)]
    pub legacy_items: bool,
}

pub type TokenListener = Box<dyn Fn(Source, TokenEvent) + Send + Sync>;

/// Where requests go; tests point it at a mock server.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub api: String,
    pub token: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            api: API_URL.into(),
            token: auth::TOKEN_URL.into(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct ApiClient {
    pub source: Source,
    http: reqwest::Client,
    endpoints: Endpoints,
    grant: Mutex<Option<StoredToken>>,
    /// Bumped on every install and clear: a refresh that started under an
    /// older grant must not overwrite a newer one.
    generation: AtomicU64,
    refreshing: tokio::sync::Mutex<()>,
    limiter: Semaphore,
    interactive: Semaphore,
    cooldown_until: Mutex<Instant>,
    listener: OnceLock<TokenListener>,
    /// `GET /tracks?ids=` answered as removed (Development Mode).
    pub(crate) no_batch: AtomicBool,
    /// `/me/library` is not there; use the classic `/me/tracks`.
    pub(crate) legacy_library: AtomicBool,
    /// `/playlists/{id}/items` is not there; use `/tracks`.
    pub(crate) legacy_items: AtomicBool,
    /// When the next background request may start (see
    /// [`BACKGROUND_SPACING`]).
    next_background: Mutex<Instant>,
}

impl ApiClient {
    pub fn new(source: Source, http: reqwest::Client, endpoints: Endpoints) -> Self {
        Self {
            source,
            http,
            endpoints,
            grant: Mutex::new(None),
            generation: AtomicU64::new(0),
            refreshing: tokio::sync::Mutex::new(()),
            limiter: Semaphore::new(MAX_IN_FLIGHT),
            interactive: Semaphore::new(INTERACTIVE_IN_FLIGHT),
            cooldown_until: Mutex::new(Instant::now()),
            listener: OnceLock::new(),
            no_batch: AtomicBool::new(false),
            legacy_library: AtomicBool::new(false),
            legacy_items: AtomicBool::new(false),
            next_background: Mutex::new(Instant::now()),
        }
    }

    /// What this app was found to lack.
    pub fn probes(&self) -> Probes {
        Probes {
            no_batch: self.no_batch.load(Ordering::Relaxed),
            legacy_library: self.legacy_library.load(Ordering::Relaxed),
            legacy_items: self.legacy_items.load(Ordering::Relaxed),
        }
    }

    /// Takes what an earlier start learnt about this app.
    pub fn set_probes(&self, p: Probes) {
        self.no_batch.store(p.no_batch, Ordering::Relaxed);
        self.legacy_library.store(p.legacy_library, Ordering::Relaxed);
        self.legacy_items.store(p.legacy_items, Ordering::Relaxed);
    }

    /// Sets one probe flag; tells the listener when it is news.
    fn learn(&self, flag: &AtomicBool) {
        if !flag.swap(true, Ordering::Relaxed) {
            self.notify(TokenEvent::Probed(self.probes()));
        }
    }

    /// Waits for this background request's turn: starts are kept
    /// [`BACKGROUND_SPACING`] apart.
    async fn pace(&self) {
        let wait = {
            let mut next = lock(&self.next_background);
            let now = Instant::now();
            let at = (*next).max(now);
            *next = at + BACKGROUND_SPACING;
            at - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    pub fn set_listener(&self, listener: TokenListener) {
        let _ = self.listener.set(listener);
    }

    fn notify(&self, event: TokenEvent) {
        if let Some(l) = self.listener.get() {
            l(self.source, event);
        }
    }

    pub fn install(&self, token: StoredToken) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *lock(&self.grant) = Some(token);
    }

    pub fn clear(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *lock(&self.grant) = None;
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.endpoints
    }

    pub fn has_grant(&self) -> bool {
        lock(&self.grant).is_some()
    }

    pub fn client_id(&self) -> Option<String> {
        lock(&self.grant).as_ref().map(|t| t.client_id.clone())
    }

    /// A valid access token, refreshed first when it is close to expiry or
    /// when `rejected` (the token a 401 refused) is still the current one.
    async fn access_token(&self, rejected: Option<&str>) -> Result<String> {
        let _serial = self.refreshing.lock().await;
        let generation = self.generation.load(Ordering::SeqCst);
        let current = lock(&self.grant).clone().ok_or(ApiError::NotSignedIn)?;
        let force = rejected.is_some_and(|t| t == current.access_token);
        if !force && !current.needs_refresh() {
            return Ok(current.access_token);
        }
        match auth::refresh(
            &self.http,
            &self.endpoints.token,
            &current.client_id,
            &current.refresh_token,
        )
        .await
        {
            Ok(response) => {
                let Some(updated) = StoredToken::from_response(
                    &current.client_id,
                    response,
                    Some(&current.refresh_token),
                ) else {
                    return Err(ApiError::Decode("token response".into()));
                };
                if self.generation.load(Ordering::SeqCst) != generation {
                    return Err(ApiError::NotSignedIn);
                }
                *lock(&self.grant) = Some(updated.clone());
                let access = updated.access_token.clone();
                self.notify(TokenEvent::Refreshed(updated));
                Ok(access)
            }
            Err(AuthError::Rejected { .. }) => {
                if self.generation.load(Ordering::SeqCst) == generation {
                    self.clear();
                    log::warn!(
                        "spotify: {} grant rejected on refresh, dropped",
                        self.source
                    );
                    self.notify(TokenEvent::Rejected);
                }
                Err(ApiError::SignInExpired)
            }
            Err(e) => {
                if force || current.expired() {
                    return Err(ApiError::Network(e.to_string()));
                }
                log::warn!("spotify: token refresh failed, using the current token: {e}");
                Ok(current.access_token)
            }
        }
    }

    /// How long this grant is still paused by a `Retry-After`.
    pub fn cooldown_left(&self) -> Option<Duration> {
        lock(&self.cooldown_until).checked_duration_since(Instant::now())
    }

    fn extend_cooldown(&self, wait: Duration) {
        let mut until = lock(&self.cooldown_until);
        *until = (*until).max(Instant::now() + wait);
    }

    // ---- transport -------------------------------------------------------

    async fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<String> {
        self.send_in(Lane::Background, method, path, query, body).await
    }

    /// Sends one request. `lane` decides how it waits: background work
    /// shares [`MAX_IN_FLIGHT`] slots and sits out `Retry-After` pauses up to
    /// [`MAX_RETRY_WAIT`]; an interactive request (search) has slots of its
    /// own, so background paging never queues it, and gives up on a pause
    /// longer than [`INTERACTIVE_WAIT`] at once (the user sees «занят»
    /// instead of a search that hangs).
    async fn send_in(
        &self,
        lane: Lane,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<String> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{}{path}", self.endpoints.api)
        };
        let endpoint = endpoint_name(path);
        let max_wait = match lane {
            Lane::Background => MAX_RETRY_WAIT,
            Lane::Interactive => INTERACTIVE_WAIT,
        };
        let retries = match lane {
            Lane::Background => RATE_LIMIT_RETRIES,
            Lane::Interactive => 1,
        };
        let started = Instant::now();
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let queued = Instant::now();
            if lane == Lane::Background {
                // Paused for longer than this lane waits: no turn is taken.
                if let Some(left) = self.cooldown_left().filter(|l| *l > max_wait) {
                    return Err(ApiError::RateLimited {
                        retry_after: Some(left),
                    });
                }
                self.pace().await;
            }
            let permit = match lane {
                Lane::Background => self.limiter.acquire().await,
                Lane::Interactive => self.interactive.acquire().await,
            }
            .map_err(|_| ApiError::NotSignedIn)?;
            let queued_ms = queued.elapsed().as_millis();
            if queued_ms > 50 {
                log::debug!(
                    "spotify: source={} {method} {endpoint} waited {queued_ms} ms for a request slot",
                    self.source
                );
            }
            // After the permit: a 429 answered while this request queued has
            // extended the cooldown, and sending now would earn another.
            if let Some(left) = self.cooldown_left() {
                if left > max_wait {
                    log::info!(
                        "spotify: source={} {method} {endpoint} not sent: Retry-After pause, {} s left",
                        self.source,
                        left.as_secs()
                    );
                    return Err(ApiError::RateLimited {
                        retry_after: Some(left),
                    });
                }
                // Spread out: every waiter woke at the same instant before.
                let jitter = Duration::from_millis(rand::random_range(0..WAKE_JITTER_MS));
                log::debug!(
                    "spotify: source={} {method} {endpoint} waits out Retry-After: {} ms",
                    self.source,
                    (left + jitter).as_millis()
                );
                tokio::time::sleep(left + jitter).await;
            }
            let token = self.access_token(None).await?;
            let mut request = self
                .http
                .request(method.clone(), &url)
                .bearer_auth(&token)
                .query(query);
            if let Some(body) = body {
                request = request.json(body);
            } else if method != Method::GET {
                request = request.header(reqwest::header::CONTENT_LENGTH, "0");
            }
            let sent = Instant::now();
            let response = match request.send().await {
                Ok(r) => r,
                Err(e) => {
                    let e = e.without_url();
                    log::info!(
                        "spotify: source={} {method} {endpoint} failed after {} ms: {e}",
                        self.source,
                        sent.elapsed().as_millis(),
                    );
                    return Err(ApiError::from(e));
                }
            };
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED && attempt == 1 {
                log::debug!(
                    "spotify: source={} {method} {endpoint} -> 401 in {} ms, refreshing the token",
                    self.source,
                    sent.elapsed().as_millis()
                );
                drop(permit);
                self.access_token(Some(&token)).await?;
                continue;
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                let wait = response
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map_or(Duration::from_secs(1), Duration::from_secs);
                let text = response.text().await.unwrap_or_default();
                drop(permit);
                if is_quota_exhausted(&text) {
                    log::warn!("spotify: {} app quota exhausted", self.source);
                    self.extend_cooldown(MAX_RETRY_WAIT * 2);
                    return Err(ApiError::RateLimited { retry_after: None });
                }
                log::warn!(
                    "spotify: source={} {method} {endpoint} -> 429 in {} ms, Retry-After {} s (attempt {attempt})",
                    self.source,
                    sent.elapsed().as_millis(),
                    wait.as_secs()
                );
                self.extend_cooldown(wait);
                if wait > max_wait || attempt > retries {
                    return Err(ApiError::RateLimited {
                        retry_after: Some(wait),
                    });
                }
                continue;
            }
            if status.is_server_error() && method == Method::GET && attempt == 1 {
                log::info!(
                    "spotify: source={} {method} {endpoint} -> {} in {} ms, retrying",
                    self.source,
                    status.as_u16(),
                    sent.elapsed().as_millis()
                );
                drop(permit);
                tokio::time::sleep(Duration::from_millis(800)).await;
                continue;
            }
            let text = response.text().await?;
            log::debug!(
                "spotify: source={} {method} {endpoint} -> {} in {} ms (total {} ms, attempt {attempt})",
                self.source,
                status.as_u16(),
                sent.elapsed().as_millis(),
                started.elapsed().as_millis()
            );
            if status.is_success() {
                return Ok(text);
            }
            let message = serde_json::from_str::<ApiErrorBody>(&text)
                .ok()
                .map(|b| b.error.message)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").into());
            return Err(ApiError::Status {
                status: status.as_u16(),
                message,
            });
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, query: &[(&str, String)]) -> Result<T> {
        let text = self.send(Method::GET, path, query, None).await?;
        let text = if text.trim().is_empty() {
            "null"
        } else {
            &text
        };
        serde_json::from_str(text).map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// A change: the status decides success; a JSON body is returned when
    /// there is one.
    async fn write(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&Value>,
    ) -> Result<Option<Value>> {
        let text = self.send(method, path, query, body).await?;
        Ok(serde_json::from_str(&text).ok())
    }

    // ---- account ---------------------------------------------------------

    pub async fn me(&self) -> Result<User> {
        self.get("/me", &[]).await
    }

    // ---- catalogue -------------------------------------------------------

    /// One `/search` with every type asked at once, in the interactive lane.
    pub async fn search(&self, query: &str, types: &[&str], limit: u32) -> Result<SearchResults> {
        self.search_in(Lane::Interactive, query, types, limit).await
    }

    /// One `/search` in `lane` (background work searches in the background
    /// lane, never queueing the user's searches).
    pub async fn search_in(
        &self,
        lane: Lane,
        query: &str,
        types: &[&str],
        limit: u32,
    ) -> Result<SearchResults> {
        let text = self
            .send_in(
                lane,
                Method::GET,
                "/search",
                &[
                    ("q", query.to_string()),
                    ("type", types.join(",")),
                    ("limit", limit.min(SEARCH_LIMIT).to_string()),
                ],
                None,
            )
            .await?;
        serde_json::from_str(&text).map_err(|e| ApiError::Decode(e.to_string()))
    }

    pub async fn track(&self, id: &str) -> Result<Track> {
        self.get(&format!("/tracks/{id}"), &[]).await
    }

    /// Up to 50 tracks in one request. Development Mode removed the batch
    /// endpoint; once it answers as removed, this grant stops asking.
    pub async fn tracks(&self, ids: &[String]) -> Result<Option<Vec<Track>>> {
        if self.no_batch.load(Ordering::Relaxed) || ids.is_empty() {
            return Ok(None);
        }
        match self
            .get::<Tracks>("/tracks", &[("ids", ids.join(","))])
            .await
        {
            Ok(t) => Ok(Some(t.tracks)),
            Err(e) if e.is_refusal() => {
                log::info!("spotify: {} app has no batch track lookup", self.source);
                self.learn(&self.no_batch);
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    pub async fn album(&self, id: &str) -> Result<Album> {
        self.get(&format!("/albums/{id}"), &[]).await
    }

    pub async fn album_tracks(&self, id: &str, offset: u32) -> Result<Page<Track>> {
        self.get(
            &format!("/albums/{id}/tracks"),
            &[("limit", PAGE.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn artist(&self, id: &str) -> Result<Artist> {
        self.get(&format!("/artists/{id}"), &[]).await
    }

    /// Development Mode apps get at most 10 albums per page.
    pub fn artist_albums_limit(&self) -> u32 {
        match self.source {
            Source::Shared => PAGE,
            Source::Personal => 10,
        }
    }

    pub async fn artist_albums(&self, id: &str, offset: u32) -> Result<Page<Album>> {
        self.get(
            &format!("/artists/{id}/albums"),
            &[
                ("include_groups", "album,single,compilation".to_string()),
                ("limit", self.artist_albums_limit().to_string()),
                ("offset", offset.to_string()),
            ],
        )
        .await
    }

    // ---- playlists -------------------------------------------------------

    pub async fn playlist(&self, id: &str) -> Result<Playlist> {
        self.get(&format!("/playlists/{id}"), &[]).await
    }

    /// Only the snapshot id of a playlist (a few bytes instead of the list).
    pub async fn playlist_snapshot(&self, id: &str) -> Result<Option<String>> {
        let s: Snapshot = self
            .get(&format!("/playlists/{id}"), &[("fields", "snapshot_id".to_string())])
            .await?;
        Ok(s.snapshot_id)
    }

    /// `/playlists/{id}/items`, or `/tracks` where this app lacks `/items`.
    fn items_path(&self, id: &str) -> String {
        if self.legacy_items.load(Ordering::Relaxed) {
            format!("/playlists/{id}/tracks")
        } else {
            format!("/playlists/{id}/items")
        }
    }

    /// One page of a playlist's entries (`/items`; `/tracks` before 2026).
    pub async fn playlist_items(&self, id: &str, offset: u32) -> Result<PlaylistEntries> {
        let query = [
            ("limit", PAGE.to_string()),
            ("offset", offset.to_string()),
            ("additional_types", "track".to_string()),
        ];
        match self.get(&self.items_path(id), &query).await {
            Err(e) if e.status() == Some(404) && !self.legacy_items.load(Ordering::Relaxed) => {
                let r = self.get(&format!("/playlists/{id}/tracks"), &query).await;
                if r.is_ok() {
                    self.learn(&self.legacy_items);
                }
                r
            }
            other => other,
        }
    }

    pub async fn my_playlists(&self, offset: u32) -> Result<Page<Playlist>> {
        self.get(
            "/me/playlists",
            &[("limit", PAGE.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    pub async fn create_playlist(&self, name: &str, user_id: &str) -> Result<Playlist> {
        let body = json!({ "name": name, "public": false });
        let value = match self
            .write(Method::POST, "/me/playlists", &[], Some(&body))
            .await
        {
            Err(e) if e.is_refusal() && !user_id.is_empty() => {
                self.write(
                    Method::POST,
                    &format!("/users/{user_id}/playlists"),
                    &[],
                    Some(&body),
                )
                .await?
            }
            other => other?,
        };
        serde_json::from_value(value.unwrap_or(Value::Null))
            .map_err(|e| ApiError::Decode(e.to_string()))
    }

    /// Appends up to 100 URIs; returns the new snapshot.
    pub async fn add_items(&self, id: &str, uris: &[String]) -> Result<Option<String>> {
        let body = json!({ "uris": uris });
        let value = match self
            .write(Method::POST, &self.items_path(id), &[], Some(&body))
            .await
        {
            Err(e) if e.status() == Some(404) && !self.legacy_items.load(Ordering::Relaxed) => {
                self.write(
                    Method::POST,
                    &format!("/playlists/{id}/tracks"),
                    &[],
                    Some(&body),
                )
                .await?
            }
            other => other?,
        };
        Ok(snapshot(value))
    }

    /// Removes every occurrence of up to 100 URIs; returns the new snapshot.
    pub async fn remove_items(&self, id: &str, uris: &[String]) -> Result<Option<String>> {
        let entries: Vec<Value> = uris.iter().map(|u| json!({ "uri": u })).collect();
        let legacy = self.legacy_items.load(Ordering::Relaxed);
        let first = if legacy {
            self.write(
                Method::DELETE,
                &format!("/playlists/{id}/tracks"),
                &[],
                Some(&json!({ "tracks": entries })),
            )
            .await
        } else {
            self.write(
                Method::DELETE,
                &format!("/playlists/{id}/items"),
                &[],
                Some(&json!({ "items": entries })),
            )
            .await
        };
        let value = match first {
            Err(e) if e.status() == Some(404) && !legacy => {
                self.write(
                    Method::DELETE,
                    &format!("/playlists/{id}/tracks"),
                    &[],
                    Some(&json!({ "tracks": entries })),
                )
                .await?
            }
            other => other?,
        };
        Ok(snapshot(value))
    }

    // ---- library ---------------------------------------------------------

    pub async fn saved_tracks(&self, offset: u32) -> Result<Page<SavedTrack>> {
        self.get(
            "/me/tracks",
            &[("limit", PAGE.to_string()), ("offset", offset.to_string())],
        )
        .await
    }

    /// Saves or removes tracks by id, [`SAVE_CHUNK`] per request.
    pub async fn set_saved(&self, ids: &[String], saved: bool) -> Result<()> {
        for chunk in ids.chunks(SAVE_CHUNK) {
            self.set_saved_chunk(chunk, saved).await?;
        }
        Ok(())
    }

    /// Saves or removes up to [`SAVE_CHUNK`] tracks: the URI-based
    /// `/me/library` since February 2026, the classic `/me/tracks` where
    /// that is not there.
    async fn set_saved_chunk(&self, ids: &[String], saved: bool) -> Result<()> {
        let method = if saved { Method::PUT } else { Method::DELETE };
        if !self.legacy_library.load(Ordering::Relaxed) {
            let uris: Vec<String> = ids.iter().map(|id| format!("spotify:track:{id}")).collect();
            match self
                .write(
                    method.clone(),
                    "/me/library",
                    &[("uris", uris.join(","))],
                    None,
                )
                .await
            {
                Ok(_) => return Ok(()),
                Err(e) if matches!(e.status(), Some(404 | 405 | 410)) => {
                    self.learn(&self.legacy_library);
                }
                Err(e) => return Err(e),
            }
        }
        self.write(method, "/me/tracks", &[("ids", ids.join(","))], None)
            .await
            .map(drop)
    }

    pub async fn recently_played(&self) -> Result<CursorPage<PlayHistory>> {
        self.get("/me/player/recently-played", &[("limit", PAGE.to_string())])
            .await
    }
}

fn snapshot(value: Option<Value>) -> Option<String> {
    value
        .and_then(|v| serde_json::from_value::<Snapshot>(v).ok())
        .and_then(|s| s.snapshot_id)
}

fn is_quota_exhausted(body: &str) -> bool {
    serde_json::from_str::<ApiErrorBody>(body)
        .ok()
        .and_then(|b| b.error.reason)
        .is_some_and(|r| r == "QUOTA_EXCEEDED")
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    /// A valid grant for `client`.
    pub fn token(client: &str) -> StoredToken {
        StoredToken {
            client_id: client.into(),
            access_token: "access".into(),
            refresh_token: "refresh".into(),
            expires_at: auth::now_secs() + 3600,
            scope: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::token;
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    fn client(server: &MockServer, source: Source) -> ApiClient {
        let c = ApiClient::new(
            source,
            reqwest::Client::new(),
            Endpoints {
                api: server.uri(),
                token: format!("{}/api/token", server.uri()),
            },
        );
        c.install(token("id"));
        c
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn retry_after_pauses_only_the_grant_that_was_throttled() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        struct Once429(Arc<AtomicUsize>);
        impl Respond for Once429 {
            fn respond(&self, _: &Request) -> ResponseTemplate {
                if self.0.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(429).insert_header("Retry-After", "1")
                } else {
                    ResponseTemplate::new(200).set_body_string(r#"{"id":"u","display_name":"U"}"#)
                }
            }
        }
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(Once429(calls.clone()))
            .mount(&server)
            .await;
        let shared = client(&server, Source::Shared);
        let personal = client(&server, Source::Personal);
        let started = Instant::now();
        let me = shared.me().await.unwrap();
        assert_eq!(me.id, "u");
        assert!(
            started.elapsed() >= Duration::from_millis(950),
            "waited out"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        // A long Retry-After is not waited out, and the grant stays paused.
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
            .expect(1)
            .mount(&server)
            .await;
        let e = shared.me().await.unwrap_err();
        assert!(
            matches!(e, ApiError::RateLimited { retry_after: Some(d) } if d == Duration::from_secs(120))
        );
        let started = Instant::now();
        let again = shared.me().await.unwrap_err();
        assert!(
            matches!(again, ApiError::RateLimited { retry_after: Some(d) } if d > Duration::from_secs(100))
        );
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "no request, no wait"
        );
        assert!(
            personal.cooldown_left().is_none(),
            "the other grant is not paused"
        );
        server.verify().await;
        assert_eq!(
            StreamError::from(e),
            StreamError::RateLimited {
                retry_after: Some(Duration::from_secs(120))
            }
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_grant_never_has_more_than_its_budget_in_flight() {
        let server = MockServer::start().await;
        let now = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        struct Slow(Arc<AtomicUsize>, Arc<AtomicUsize>);
        impl Respond for Slow {
            fn respond(&self, _: &Request) -> ResponseTemplate {
                // wiremock answers synchronously; the delay keeps requests
                // overlapping, and counting happens in the request matcher.
                let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
                self.1.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(60));
                self.0.fetch_sub(1, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_string("{}")
            }
        }
        Mock::given(method("GET"))
            .and(path("/tracks/x"))
            .respond_with(Slow(now.clone(), peak.clone()))
            .mount(&server)
            .await;
        let shared = Arc::new(client(&server, Source::Shared));
        let tasks: Vec<_> = (0..12)
            .map(|_| {
                let c = shared.clone();
                tokio::spawn(async move { c.track("x").await })
            })
            .collect();
        for t in tasks {
            t.await.unwrap().unwrap();
        }
        let peak = peak.load(Ordering::SeqCst);
        assert!(peak <= MAX_IN_FLIGHT, "peak {peak}");
        assert!(shared.limiter.available_permits() == MAX_IN_FLIGHT);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_rejected_refresh_drops_the_grant() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/token"))
            .respond_with(
                ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#),
            )
            .mount(&server)
            .await;
        let c = client(&server, Source::Personal);
        let rejected = Arc::new(AtomicBool::new(false));
        let flag = rejected.clone();
        c.set_listener(Box::new(move |source, event| {
            assert_eq!(source, Source::Personal);
            if matches!(event, TokenEvent::Rejected) {
                flag.store(true, Ordering::SeqCst);
            }
        }));
        let mut old = token("id");
        old.expires_at = auth::now_secs();
        c.install(old);
        assert_eq!(c.me().await.unwrap_err(), ApiError::SignInExpired);
        assert!(!c.has_grant());
        assert!(rejected.load(Ordering::SeqCst));
        assert_eq!(c.me().await.unwrap_err(), ApiError::NotSignedIn);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn library_writes_fall_back_to_the_classic_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/me/library"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/me/tracks"))
            .respond_with(ResponseTemplate::new(200))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/tracks"))
            .respond_with(ResponseTemplate::new(403))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server, Source::Shared);
        c.set_saved(&["a".into()], true).await.unwrap();
        c.set_saved(&["a".into()], true).await.unwrap();
        assert!(c.tracks(&["a".into()]).await.unwrap().is_none());
        assert!(c.tracks(&["a".into()]).await.unwrap().is_none());
        server.verify().await;
    }
}
