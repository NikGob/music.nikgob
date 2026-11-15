//! SoundCloud as a [`StreamService`], over api-v2 (what soundcloud.com
//! itself uses), the way yt-dlp reads it.
//!
//! * **Client id**: api-v2 wants the web player's `client_id`. It is
//!   scraped from soundcloud.com's scripts, kept in `client_id` in the
//!   service's folder, and scraped again when the API answers 401 or 403.
//! * **Sign-in**: the user's own `oauth_token` cookie of soundcloud.com
//!   (the app's sign-in window or pasted, see [`token`]), sent as
//!   `Authorization: OAuth …` and kept in the platform credential store.
//!   The catalogue answers without it; likes, playlists, history,
//!   followings and the home shelves need it. A token the API refuses
//!   marks the account failed (sign in again).
//! * **Requests** go through [`limit`]: the user's search at once, a few
//!   catalogue calls at a time, signed-in calls one at a time, background
//!   starts spaced; a 429 or DataDome's bot check pauses the whole service.
//!   Listings follow `next_href` page by page.
//! * **Sound**: HLS AAC as fragmented MP4 ([`hls`]). A track the account
//!   may only preview (`policy: SNIP`, Go+ tracks) is
//!   [`StreamError::Unavailable`]; one with only encrypted streams
//!   (`cbc-/ctr-encrypted-hls`, major labels) is [`StreamError::Drm`]:
//!   never decrypted, left for a route through SoundCloud's own player.
//! * **Read only**: likes are sent both ways (one request each), playlists
//!   never ([`StreamPlaylist::editable`] is false), nothing is saved to
//!   disk ([`StreamService::save_audio`] is unsupported).
//! * **Originals**: a track whose uploader lets it be downloaded
//!   (`downloadable`, `has_downloads_left`) has its uploaded file behind
//!   `/tracks/{id}/download` (a `redirectUri`), for the signed-in user only
//!   ([`Soundcloud::downloadable`], [`Soundcloud::original_url`]; the
//!   download source is crates/sources).
//! * **Discovery**: related tracks, the track's and the artist's stations,
//!   related artists, an artist's top tracks and albums. Suggestions:
//!   the personal shelves of `/mixed-selections` ([`SuggestionFeed::Home`]),
//!   the play history ([`SuggestionFeed::History`]), followed artists
//!   ([`SuggestionFeed::Followings`]) and the user's reposts
//!   ([`SuggestionFeed::Reposts`]).
//!
//! Every future runs on the service's own small tokio runtime and is
//! awaited through a handle that cancels it when dropped.

mod api;
mod cache;
mod hls;
mod limit;
pub mod token;

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use nikgob_core::streaming::{
    Account, AudioStream, BoxFuture, EntityKind, LikedHead, LoginInput, Played, SearchKind, SearchParts, Service,
    StreamAlbum, StreamArtist, StreamError, StreamPlaylist, StreamSearch, StreamService, StreamTrack,
    SuggestedArtist, SuggestedTrack, SuggestionFeed, SuggestionKind, Suggestions, rank_weight,
};
use serde::de::DeserializeOwned;
use tokio::task::JoinHandle;

use cache::TtlCache;
use limit::Lane;

/// The API.
pub const API_BASE: &str = "https://api-v2.soundcloud.com";
/// The site whose scripts carry the client id, and the sign-in page.
pub const SITE: &str = "https://soundcloud.com";
pub const SIGNIN_URL: &str = "https://soundcloud.com/signin";
/// What the requests say they are: the web player in a desktop Chrome.
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
/// What a refused session reads (the account's failure).
pub const SESSION_EXPIRED: &str = "Вход в SoundCloud истёк — войди снова";

/// Hits per search category.
const SEARCH_LIMIT: u32 = 20;
/// Page size of listings.
const PAGE: u32 = 200;
/// Tracks asked per `/tracks?ids=` request.
const IDS_PER_CALL: usize = 50;
/// Most likes, history entries and playlist tracks read.
const LIKES_MAX: usize = 20_000;
const HISTORY_MAX: usize = 1_000;
const PLAYLISTS_MAX: usize = 1_000;
/// Discovery lists.
const DISCOVERY: u32 = 50;
const RELATED_ARTISTS: u32 = 20;
const ARTIST_ALBUMS_MAX: usize = 200;
/// Personal shelves read for the home suggestions.
const HOME_SHELVES: usize = 3;
const SEARCH_TTL: Duration = Duration::from_secs(10 * 60);
const ITEM_TTL: Duration = Duration::from_secs(3600);
const PLAYLIST_TTL: Duration = Duration::from_secs(10 * 60);
/// A track's stream data (its `track_authorization` is short-lived).
const RAW_TTL: Duration = Duration::from_secs(5 * 60);
/// A scraped client id older than this is scraped again at the start.
const CLIENT_ID_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
/// The client id is scraped again at most this often after refusals.
const RESCRAPE_EVERY: Duration = Duration::from_secs(10 * 60);

/// Where the service keeps its state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundcloudConfig {
    /// The service's folder (`<data>/soundcloud`): the scraped client id.
    pub dir: PathBuf,
    pub api_base: String,
    pub site: String,
    /// Keep the sign-in in memory only (tests, the demo).
    pub memory_store: bool,
}

impl SoundcloudConfig {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            api_base: API_BASE.to_owned(),
            site: SITE.to_owned(),
            memory_store: false,
        }
    }
}

struct Inner {
    http: reqwest::Client,
    dir: PathBuf,
    api: String,
    site: String,
    client_id: Mutex<Option<String>>,
    scrape: tokio::sync::Mutex<()>,
    token: Mutex<Option<String>>,
    me: Mutex<Option<api::Me>>,
    account: Mutex<Account>,
    /// Bumped by every logout: a login or restore that started before is
    /// dropped.
    generation: AtomicU64,
    store: token::Store,
    /// A sign-in is being tried: a refusal of the new token is the login's
    /// to handle, not a sign-out.
    trying: AtomicBool,
    /// When the client id was last scraped because of refusals.
    rescraped_at: Mutex<Option<Instant>>,
    /// The `/me` likes count minus the tracks the last full read kept (stubs
    /// and removed tracks), so the count can say how long the list is.
    likes_skew: Mutex<Option<i64>>,
    /// The last full read stopped at [`LIKES_MAX`].
    likes_capped: AtomicBool,
    gate: limit::Gate,
    /// The media CDN's `Retry-After`, for every stream.
    media: hls::MediaCooldown,
    searches: TtlCache<StreamSearch>,
    raw: TtlCache<api::Track>,
    tracks: TtlCache<StreamTrack>,
    albums: TtlCache<StreamAlbum>,
    artists: TtlCache<StreamArtist>,
    playlists: TtlCache<StreamPlaylist>,
    lists: TtlCache<Vec<StreamTrack>>,
    similar: TtlCache<Vec<StreamArtist>>,
    /// Web pages of what was listed (`track:1`, `user:2`, `playlist:3`).
    permalinks: Mutex<HashMap<String, String>>,
}

/// The runtime every future of the service runs on.
struct Runtime(Option<tokio::runtime::Runtime>);

impl Runtime {
    fn handle(&self) -> &tokio::runtime::Handle {
        self.0.as_ref().expect("runtime lives as long as the service").handle()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_background();
        }
    }
}

/// A track its uploader lets anyone download: the file it was uploaded as
/// (often a WAV, an AIFF or a FLAC, or an MP3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Downloadable {
    pub id: u64,
    pub title: String,
    /// The credited artists: the record's (a label's upload), else the
    /// uploader.
    pub artists: Vec<String>,
    pub uploader: String,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub isrc: Option<String>,
    /// Its page.
    pub url: Option<String>,
}

/// SoundCloud; see the crate docs.
pub struct Soundcloud {
    inner: Arc<Inner>,
    runtime: Runtime,
}

/// A spawned task, aborted when dropped.
struct Joined<T>(JoinHandle<Result<T, StreamError>>);

impl<T> Future for Joined<T> {
    type Output = Result<T, StreamError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0)
            .poll(cx)
            .map(|r| r.unwrap_or_else(|e| Err(StreamError::Network(format!("task failed: {e}")))))
    }
}

impl<T> Drop for Joined<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Soundcloud {
    /// Builds the client and the service's runtime, and restores the saved
    /// sign-in in the background.
    pub fn new(config: SoundcloudConfig) -> Result<Arc<Self>, StreamError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("soundcloud")
            .enable_all()
            .build()
            .map_err(|e| StreamError::Network(format!("cannot start the runtime: {e}")))?;
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| StreamError::Network(e.to_string()))?;
        let store = if config.memory_store {
            token::Store::memory(&config.dir)
        } else {
            token::Store::native(&config.dir)
        };
        let cached_id = read_client_id(&config.dir);
        let me = Arc::new(Self {
            inner: Arc::new(Inner {
                http,
                dir: config.dir,
                api: config.api_base.trim_end_matches('/').to_owned(),
                site: config.site.trim_end_matches('/').to_owned(),
                client_id: Mutex::new(cached_id),
                scrape: tokio::sync::Mutex::new(()),
                token: Mutex::new(None),
                me: Mutex::new(None),
                account: Mutex::new(Account::LoggedOut),
                generation: AtomicU64::new(0),
                store,
                trying: AtomicBool::new(false),
                rescraped_at: Mutex::new(None),
                likes_skew: Mutex::new(None),
                likes_capped: AtomicBool::new(false),
                gate: limit::Gate::new(),
                media: hls::MediaCooldown::default(),
                searches: TtlCache::new(SEARCH_TTL, 64),
                raw: TtlCache::new(RAW_TTL, 500),
                tracks: TtlCache::new(ITEM_TTL, 2_000),
                albums: TtlCache::new(ITEM_TTL, 200),
                artists: TtlCache::new(ITEM_TTL, 200),
                playlists: TtlCache::new(PLAYLIST_TTL, 50),
                lists: TtlCache::new(ITEM_TTL, 400),
                similar: TtlCache::new(ITEM_TTL, 200),
                permalinks: Mutex::new(HashMap::new()),
            }),
            runtime: Runtime(Some(runtime)),
        });
        let inner = Arc::clone(&me.inner);
        me.runtime.handle().spawn(async move { inner.restore().await });
        Ok(me)
    }

    /// Whether the user is signed in (the originals need it).
    pub fn signed_in(&self) -> bool {
        self.inner.token().is_some()
    }

    /// Tracks found for `query` whose uploader lets the original file be
    /// downloaded.
    pub fn downloadable(&self, query: &str) -> BoxFuture<'_, Result<Vec<Downloadable>, StreamError>> {
        let q = query.trim().to_owned();
        self.run(move |inner| async move { inner.downloadable(&q).await })
    }

    /// The link of a downloadable track's original file (short-lived: ask
    /// right before downloading). [`StreamError::NotLoggedIn`] when signed
    /// out.
    pub fn original_url(&self, id: u64) -> BoxFuture<'_, Result<String, StreamError>> {
        self.run(move |inner| async move { inner.original_url(id).await })
    }

    fn run<T, F>(&self, f: impl FnOnce(Arc<Inner>) -> F) -> BoxFuture<'_, Result<T, StreamError>>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, StreamError>> + Send + 'static,
    {
        let task = self.runtime.handle().spawn(f(Arc::clone(&self.inner)));
        Box::pin(Joined(task))
    }
}

// ---------------------------------------------------------------------------
// The client id
// ---------------------------------------------------------------------------

fn client_id_path(dir: &Path) -> PathBuf {
    dir.join("client_id")
}

/// The kept client id, unless it is old.
fn read_client_id(dir: &Path) -> Option<String> {
    let path = client_id_path(dir);
    let age = std::fs::metadata(&path).ok()?.modified().ok()?.elapsed().ok()?;
    if age > CLIENT_ID_MAX_AGE {
        return None;
    }
    let id = std::fs::read_to_string(path).ok()?;
    valid_client_id(id.trim()).then(|| id.trim().to_owned())
}

fn valid_client_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `client_id:"…"` (or `client_id=…`) in a script.
fn find_client_id(js: &str) -> Option<String> {
    for pat in ["client_id:\"", "client_id=\"", "client_id:'", "?client_id=", "&client_id="] {
        let mut from = 0;
        while let Some(at) = js[from..].find(pat) {
            let start = from + at + pat.len();
            let id: String = js[start..].chars().take_while(char::is_ascii_alphanumeric).collect();
            if valid_client_id(&id) {
                return Some(id);
            }
            from = start;
        }
    }
    None
}

/// The script urls of soundcloud.com's page, last first (the client id is
/// in one of the last bundles).
fn script_urls(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = html[from..].find("src=\"") {
        let start = from + at + 5;
        let Some(end) = html[start..].find('"') else { break };
        let url = &html[start..start + end];
        if url.ends_with(".js") && url.contains("sndcdn.com") {
            out.push(url.to_owned());
        }
        from = start + end;
    }
    out.reverse();
    out
}

/// DataDome's answer (its bot check) instead of the API's.
fn is_bot_check(headers: &reqwest::header::HeaderMap, body: &[u8]) -> bool {
    headers.contains_key("x-datadome") || headers.contains_key("x-dd-b") || {
        let text = String::from_utf8_lossy(&body[..body.len().min(4096)]);
        text.contains("captcha-delivery.com") || text.contains("datadome")
    }
}

/// `url` with `client_id` set to `id` (replacing one it has).
fn with_client_id(url: &str, id: &str) -> String {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut parts: Vec<String> =
        query.split('&').filter(|kv| !kv.is_empty() && !kv.starts_with("client_id=")).map(str::to_owned).collect();
    parts.push(format!("client_id={id}"));
    format!("{base}?{}", parts.join("&"))
}

/// Both urls are on one origin (scheme, host and port).
fn same_host(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (reqwest::Url::parse(a), reqwest::Url::parse(b)) else { return false };
    a.host_str().is_some()
        && a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// What one request answered.
enum Answer {
    Ok(Vec<u8>),
    /// 401/403 (not a bot check): a stale client id or a refused token.
    Refused(u16),
}

impl Inner {
    async fn client_id(&self, fresh: bool, stale: Option<&str>) -> Result<String, StreamError> {
        if !fresh && let Some(id) = lock(&self.client_id).clone() {
            return Ok(id);
        }
        let _one = self.scrape.lock().await;
        // Another call scraped while this one waited.
        if let Some(id) = lock(&self.client_id).clone()
            && (!fresh || stale.is_some_and(|s| s != id))
        {
            return Ok(id);
        }
        let id = self.scrape_client_id().await?;
        log::info!("soundcloud: client id scraped");
        let _ = std::fs::create_dir_all(&self.dir);
        if let Err(e) = std::fs::write(client_id_path(&self.dir), &id) {
            log::debug!("soundcloud: cannot keep the client id: {e}");
        }
        *lock(&self.client_id) = Some(id.clone());
        Ok(id)
    }

    async fn scrape_client_id(&self) -> Result<String, StreamError> {
        let page = self.fetch_text(&format!("{}/", self.site)).await?;
        let scripts = script_urls(&page);
        if scripts.is_empty() {
            return Err(StreamError::Parse("soundcloud.com without scripts".into()));
        }
        for url in scripts.iter().take(12) {
            if let Ok(js) = self.fetch_text(url).await
                && let Some(id) = find_client_id(&js)
            {
                return Ok(id);
            }
        }
        Err(StreamError::Parse("no client id in soundcloud.com's scripts".into()))
    }

    async fn fetch_text(&self, url: &str) -> Result<String, StreamError> {
        let resp = self.http.get(url).send().await.map_err(|e| StreamError::Network(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.map_err(|e| StreamError::Network(e.without_url().to_string()))?;
        if status == 403 && is_bot_check(&headers, &body) {
            return Err(StreamError::RateLimited { retry_after: None });
        }
        if !(200..300).contains(&status) {
            return Err(StreamError::Http { status });
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    fn token(&self) -> Option<String> {
        lock(&self.token).clone()
    }

    /// One request, no retries.
    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        query: &[(&str, String)],
        client_id: &str,
        token: Option<&str>,
    ) -> Result<Answer, StreamError> {
        let url = if url.starts_with("http") { url.to_owned() } else { format!("{}{url}", self.api) };
        let mut req = self
            .http
            .request(method, with_client_id(&url, client_id))
            .query(query)
            .header("Accept", "application/json, text/javascript, */*; q=0.01")
            .header("Origin", self.site.as_str())
            .header("Referer", format!("{}/", self.site));
        // The sign-in goes to the API's host only (a `next_href` or a stream
        // url is not one).
        if let Some(t) = token
            && same_host(&url, &self.api)
        {
            req = req.header("Authorization", format!("OAuth {t}"));
        }
        let resp = req.send().await.map_err(|e| StreamError::Network(e.without_url().to_string()))?;
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let retry_after =
            headers.get("retry-after").and_then(|v| v.to_str().ok()?.parse::<u64>().ok()).map(Duration::from_secs);
        let body = resp.bytes().await.map_err(|e| StreamError::Network(e.without_url().to_string()))?;
        match status {
            200..=299 => Ok(Answer::Ok(body.to_vec())),
            429 => Err(StreamError::RateLimited { retry_after }),
            403 if is_bot_check(&headers, &body) => {
                log::warn!("soundcloud: bot check (DataDome) on {}", url.split('?').next().unwrap_or(""));
                Err(StreamError::RateLimited { retry_after })
            }
            401 | 403 => Ok(Answer::Refused(status)),
            404 | 410 => Err(StreamError::Unavailable),
            451 => Err(StreamError::RegionBlocked),
            _ => {
                log::info!("soundcloud: {} -> HTTP {status}", url.split('?').next().unwrap_or(""));
                Err(StreamError::Http { status })
            }
        }
    }

    /// Whether the catalogue answers with this client id and no sign-in.
    async fn anonymous_ok(&self, id: &str) -> Result<bool, StreamError> {
        let query = [("q", "a".to_owned()), ("limit", "1".to_owned())];
        Ok(matches!(self.send(reqwest::Method::GET, "/search/tracks", &query, id, None).await?, Answer::Ok(_)))
    }

    /// The client id scraped again, at most once in [`RESCRAPE_EVERY`] (a
    /// scrape another call made meanwhile counts).
    async fn rescrape(&self, id: &str) -> Option<String> {
        let due = {
            let mut at = lock(&self.rescraped_at);
            if at.is_none_or(|t| t.elapsed() >= RESCRAPE_EVERY) {
                *at = Some(Instant::now());
                true
            } else {
                false
            }
        };
        if due {
            return self.client_id(true, Some(id)).await.ok();
        }
        lock(&self.client_id).clone().filter(|c| c != id)
    }

    /// A request with the client id (scraped again when refused and the
    /// catalogue refuses a plain probe too) and the sign-in when there is
    /// one. `signed` calls need the sign-in: a 401 of theirs is the token's
    /// (a 403 is not: it is only a refusal of that call).
    async fn request(
        &self,
        method: reqwest::Method,
        url: &str,
        query: &[(&str, String)],
        signed: bool,
    ) -> Result<Vec<u8>, StreamError> {
        let token = self.token();
        if signed && token.is_none() {
            return Err(StreamError::NotLoggedIn);
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let mut id = self.client_id(false, None).await?;
        let mut rescraped = false;
        let mut with_token = token.is_some();
        let mut token_status = None;
        loop {
            let t = if with_token { token.as_deref() } else { None };
            match self.send(method.clone(), url, query, &id, t).await? {
                Answer::Ok(body) => {
                    // The catalogue answered without the sign-in that it
                    // had refused with a 401.
                    if !with_token && token_status == Some(401) {
                        self.token_refused(generation, token.as_deref());
                    }
                    return Ok(body);
                }
                Answer::Refused(status) => {
                    if with_token && !signed {
                        token_status = Some(status);
                        with_token = false;
                        continue;
                    }
                    if self.anonymous_ok(&id).await? {
                        // The client id is fine: the refusal is the call's.
                        if signed && status == 401 {
                            self.token_refused(generation, token.as_deref());
                            return Err(StreamError::NotLoggedIn);
                        }
                        return Err(StreamError::Http { status });
                    }
                    if rescraped {
                        return Err(StreamError::Http { status });
                    }
                    rescraped = true;
                    match self.rescrape(&id).await {
                        Some(new) => id = new,
                        None => return Err(StreamError::Http { status }),
                    }
                }
            }
        }
    }

    async fn get<T: DeserializeOwned>(
        &self,
        lane: Lane,
        url: &str,
        query: &[(&str, String)],
        signed: bool,
    ) -> Result<T, StreamError> {
        let body = self.gate.run(lane, self.request(reqwest::Method::GET, url, query, signed)).await?;
        serde_json::from_slice(&body)
            .map_err(|e| StreamError::Parse(format!("{}: {e}", url.split('?').next().unwrap_or(url))))
    }

    /// Every page of a listing, up to `max` items.
    async fn pages<T: DeserializeOwned>(
        &self,
        lane: Lane,
        path: &str,
        query: &[(&str, String)],
        signed: bool,
        max: usize,
    ) -> Result<(Vec<T>, bool), StreamError> {
        let mut out = Vec::new();
        let mut page: api::Page<T> = self.get(lane, path, query, signed).await?;
        loop {
            let got = page.collection.len();
            out.extend(page.collection);
            let next = page.next_href.filter(|n| !n.is_empty());
            if out.len() >= max || got == 0 {
                out.truncate(max);
                return Ok((out, next.is_none()));
            }
            let Some(next) = next else { return Ok((out, true)) };
            page = self.get(lane, &next, &[], signed).await?;
        }
    }

    // -- the account --------------------------------------------------------

    fn account(&self) -> MutexGuard<'_, Account> {
        lock(&self.account)
    }

    fn need_login(&self) -> Result<(), StreamError> {
        if self.token().is_some() { Ok(()) } else { Err(StreamError::NotLoggedIn) }
    }

    /// The token was refused: the account must sign in again, and the
    /// saved copy must not come back.
    fn token_refused(&self, generation: u64, used: Option<&str>) {
        if self.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        {
            let mut current = lock(&self.token);
            if current.as_deref() != used {
                return;
            }
            current.take();
        }
        // A sign-in being tried: the login restores the old one itself.
        if self.trying.load(Ordering::SeqCst) {
            return;
        }
        log::warn!("soundcloud: the sign-in was refused");
        *lock(&self.me) = None;
        *self.account() = Account::Failed(SESSION_EXPIRED.into());
        self.store.clear();
    }

    /// The saved sign-in, at the start.
    async fn restore(&self) {
        let generation = self.generation.load(Ordering::SeqCst);
        let Some(token) = self.store.load().await else { return };
        if self.generation.load(Ordering::SeqCst) != generation || self.token().is_some() {
            return;
        }
        *lock(&self.token) = Some(token);
        *self.account() = Account::LoggedIn {
            name: "SoundCloud".into(),
            premium: None,
            audio: true,
        };
        match self.load_me().await {
            Ok(_) => log::info!("soundcloud: signed in again"),
            // Offline at the start: the sign-in stays, /me is asked later.
            Err(e) => log::info!("soundcloud: the saved sign-in is not checked yet: {e}"),
        }
    }

    /// `/me`, kept; sets the account's name.
    async fn load_me(&self) -> Result<api::Me, StreamError> {
        let generation = self.generation.load(Ordering::SeqCst);
        let me: api::Me = self.get(Lane::SignedIn, "/me", &[], true).await?;
        if me.id == 0 {
            return Err(StreamError::Parse("/me without an id".into()));
        }
        if self.generation.load(Ordering::SeqCst) == generation && self.token().is_some() {
            let name = me.full_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| me.username.clone());
            *self.account() = Account::LoggedIn {
                name,
                premium: None,
                audio: true,
            };
            *lock(&self.me) = Some(me.clone());
        }
        Ok(me)
    }

    async fn me_id(&self) -> Result<u64, StreamError> {
        self.need_login()?;
        if let Some(me) = lock(&self.me).as_ref() {
            return Ok(me.id);
        }
        Ok(self.load_me().await?.id)
    }

    async fn login(&self, input: LoginInput) -> Result<(), StreamError> {
        let LoginInput::Cookies(text) = input else {
            return Err(StreamError::Unsupported);
        };
        let Some(token) = token::parse(&text) else {
            return Err(StreamError::Network("Нет oauth_token SoundCloud — вставь cookies soundcloud.com или токен".into()));
        };
        let generation = self.generation.load(Ordering::SeqCst);
        let before = lock(&self.token).replace(token.clone());
        let account_before = self.account().clone();
        let me_before = lock(&self.me).take();
        self.trying.store(true, Ordering::SeqCst);
        let result = self.load_me().await;
        self.trying.store(false, Ordering::SeqCst);
        match result {
            Ok(_) if self.generation.load(Ordering::SeqCst) == generation => {
                self.store.save(&token);
                log::info!("soundcloud: signed in");
                Ok(())
            }
            Ok(_) => Err(StreamError::NotLoggedIn),
            Err(e) => {
                if self.generation.load(Ordering::SeqCst) == generation {
                    // The old sign-in, if any, holds with its account.
                    *lock(&self.token) = before;
                    *self.account() = account_before;
                    *lock(&self.me) = me_before;
                }
                if matches!(e, StreamError::NotLoggedIn) {
                    Err(StreamError::Network("SoundCloud не принял вход — токен устарел".into()))
                } else {
                    Err(e)
                }
            }
        }
    }

    fn logout(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        *lock(&self.token) = None;
        *lock(&self.me) = None;
        *self.account() = Account::LoggedOut;
        self.store.clear();
    }

    // -- conversion with the pages remembered -------------------------------

    fn remember(&self, key: String, url: Option<&String>) {
        if let Some(u) = url.filter(|u| u.starts_with("https://soundcloud.com/")) {
            let mut map = lock(&self.permalinks);
            if map.len() > 20_000 {
                map.clear();
            }
            map.insert(key, u.clone());
        }
    }

    fn track(&self, t: &api::Track) -> StreamTrack {
        self.remember(format!("track:{}", t.id), t.permalink_url.as_ref());
        if let Some(u) = &t.user {
            self.remember(format!("user:{}", u.id), u.permalink_url.as_ref());
        }
        if !t.is_stub() && t.media.is_some() {
            self.raw.put(&t.id.to_string(), t.clone());
        }
        api::track(t)
    }

    fn tracks_of(&self, list: &[api::Track]) -> Vec<StreamTrack> {
        list.iter().filter(|t| t.id > 0 && !t.is_stub()).map(|t| self.track(t)).collect()
    }

    fn artist(&self, u: &api::User) -> StreamArtist {
        self.remember(format!("user:{}", u.id), u.permalink_url.as_ref());
        api::artist(u)
    }

    fn playlist_page(&self, p: &api::Playlist) {
        if let Some(k) = p.key() {
            self.remember(format!("playlist:{k}"), p.permalink_url.as_ref());
        }
    }

    // -- catalogue ------------------------------------------------------------

    async fn search_tracks(&self, q: &str) -> Result<Vec<StreamTrack>, StreamError> {
        let p: api::Page<api::Track> = self
            .get(Lane::Interactive, "/search/tracks", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())], false)
            .await?;
        Ok(self.tracks_of(&p.collection))
    }

    /// Tracks of a search whose uploader lets the original file go
    /// (a background call: the catalogue's lane, not the user's search).
    async fn downloadable(&self, q: &str) -> Result<Vec<Downloadable>, StreamError> {
        if q.is_empty() {
            return Ok(Vec::new());
        }
        let p: api::Page<api::Track> = self
            .get(Lane::Catalog, "/search/tracks", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())], false)
            .await?;
        Ok(p.collection
            .iter()
            .filter(|t| t.id > 0 && !t.is_stub() && t.offers_original())
            .map(|t| {
                let s = self.track(t);
                Downloadable {
                    id: t.id,
                    title: s.title,
                    artists: s.artists.into_iter().map(|a| a.name).filter(|n| !n.is_empty()).collect(),
                    uploader: t.user.as_ref().map(|u| u.username.trim().to_owned()).unwrap_or_default(),
                    album: s.album.map(|a| a.title),
                    duration_ms: s.duration_ms,
                    isrc: s.isrc,
                    url: t.permalink_url.clone(),
                }
            })
            .collect())
    }

    /// Where the original file of track `id` is (`/tracks/{id}/download`,
    /// a short-lived link). Only with the sign-in: api-v2 answers 401
    /// without one (checked 2026-11-15). A refusal of this call is the
    /// track's (its uploader closed downloads, the limit is reached), never
    /// taken for the sign-in failing.
    async fn original_url(&self, id: u64) -> Result<String, StreamError> {
        let Some(token) = self.token() else { return Err(StreamError::NotLoggedIn) };
        let client_id = self.client_id(false, None).await?;
        let url = format!("/tracks/{id}/download");
        let answer = self.gate.run(Lane::SignedIn, self.send(reqwest::Method::GET, &url, &[], &client_id, Some(&token))).await?;
        match answer {
            Answer::Ok(body) => {
                let link: api::DownloadLink =
                    serde_json::from_slice(&body).map_err(|e| StreamError::Parse(format!("/tracks/download: {e}")))?;
                link.redirect_uri.filter(|u| u.starts_with("http")).ok_or(StreamError::Unavailable)
            }
            Answer::Refused(status) => {
                log::info!("soundcloud: the original of track {id} was refused (HTTP {status})");
                Err(StreamError::Http { status })
            }
        }
    }

    async fn search_albums(&self, q: &str) -> Result<Vec<StreamAlbum>, StreamError> {
        let p: api::Page<api::Playlist> = self
            .get(Lane::Interactive, "/search/albums", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())], false)
            .await?;
        Ok(p.collection
            .iter()
            .map(|a| {
                self.playlist_page(a);
                api::album(a, &[])
            })
            .collect())
    }

    async fn search_artists(&self, q: &str) -> Result<Vec<StreamArtist>, StreamError> {
        let p: api::Page<api::User> = self
            .get(Lane::Interactive, "/search/users", &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())], false)
            .await?;
        Ok(p.collection.iter().filter(|u| u.id > 0).map(|u| self.artist(u)).collect())
    }

    async fn search_playlists(&self, q: &str) -> Result<Vec<StreamPlaylist>, StreamError> {
        let p: api::Page<api::Playlist> = self
            .get(
                Lane::Interactive,
                "/search/playlists_without_albums",
                &[("q", q.to_owned()), ("limit", SEARCH_LIMIT.to_string())],
                false,
            )
            .await?;
        Ok(p.collection
            .iter()
            .map(|pl| {
                self.playlist_page(pl);
                let mut out = api::playlist(pl, &[]);
                out.tracks.clear();
                out
            })
            .collect())
    }

    async fn search(&self, q: &str, playlists: bool, part: SearchParts) -> Result<StreamSearch, StreamError> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(StreamSearch::default());
        }
        let key = search_key(if playlists { "all" } else { "top" }, q);
        if let Some(hit) = self.searches.get(&key) {
            return Ok(hit);
        }
        self.gate.check()?;
        let tracks = async {
            let r = self.search_tracks(q).await;
            if let Ok(t) = &r {
                part(StreamSearch {
                    tracks: t.clone(),
                    ..StreamSearch::default()
                });
            }
            r
        };
        let pl = async {
            if playlists { self.search_playlists(q).await } else { Ok(Vec::new()) }
        };
        let (tracks, albums, artists, pl) = tokio::join!(tracks, self.search_albums(q), self.search_artists(q), pl);
        let mut first_err = None;
        let tracks = part_of(tracks, &mut first_err);
        let albums = part_of(albums, &mut first_err);
        let artists = part_of(artists, &mut first_err);
        let pl = part_of(pl, &mut first_err);
        if tracks.is_none() && albums.is_none() && artists.is_none() {
            return Err(first_err.unwrap_or(StreamError::Unavailable));
        }
        let out = StreamSearch {
            tracks: tracks.unwrap_or_default(),
            albums: albums.unwrap_or_default(),
            artists: artists.unwrap_or_default(),
            playlists: pl.unwrap_or_default(),
        };
        if first_err.is_none() {
            self.searches.put(&key, out.clone());
        }
        Ok(out)
    }

    /// A track's full listing (with its streams); `fresh` skips the cache.
    async fn raw_track(&self, id: &str, fresh: bool) -> Result<api::Track, StreamError> {
        let id = num_id(id)?;
        if !fresh && let Some(t) = self.raw.get(id) {
            return Ok(t);
        }
        let t: api::Track = self.get(Lane::Interactive, &format!("/tracks/{id}"), &[], false).await?;
        if t.id == 0 {
            return Err(StreamError::Unavailable);
        }
        self.raw.put(id, t.clone());
        Ok(t)
    }

    async fn track_one(&self, id: &str) -> Result<StreamTrack, StreamError> {
        if let Some(t) = self.tracks.get(id) {
            return Ok(t);
        }
        let id = num_id(id)?;
        let t: api::Track = self.get(Lane::Catalog, &format!("/tracks/{id}"), &[], false).await?;
        if t.id == 0 {
            return Err(StreamError::Unavailable);
        }
        let s = self.track(&t);
        self.tracks.put(id, s.clone());
        Ok(s)
    }

    /// Full listings of many tracks, `/tracks?ids=` in batches; the ones
    /// SoundCloud does not return are missing from the map.
    async fn raw_tracks(&self, lane: Lane, ids: &[u64]) -> Result<HashMap<u64, api::Track>, StreamError> {
        let mut out = HashMap::new();
        for chunk in ids.chunks(IDS_PER_CALL) {
            let list = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
            let got: Vec<api::Track> = self.get(lane, "/tracks", &[("ids", list)], false).await?;
            out.extend(got.into_iter().filter(|t| t.id > 0 && !t.is_stub()).map(|t| (t.id, t)));
        }
        Ok(out)
    }

    async fn tracks_many(&self, ids: &[String]) -> Result<Vec<Option<StreamTrack>>, StreamError> {
        let nums: Vec<u64> = ids.iter().filter_map(|i| i.trim().parse().ok()).collect();
        let missing: Vec<u64> = nums.iter().copied().filter(|n| self.tracks.get(&n.to_string()).is_none()).collect();
        let got = self.raw_tracks(Lane::Catalog, &missing).await?;
        for t in got.values() {
            let s = self.track(t);
            self.tracks.put(&t.id.to_string(), s);
        }
        Ok(ids.iter().map(|i| self.tracks.get(i.trim())).collect())
    }

    /// A playlist's tracks with the stubs filled, in order.
    async fn filled(&self, lane: Lane, tracks: Vec<api::Track>) -> Result<Vec<api::Track>, StreamError> {
        let stubs: Vec<u64> = tracks.iter().filter(|t| t.is_stub() && t.id > 0).map(|t| t.id).collect();
        if stubs.is_empty() {
            return Ok(tracks);
        }
        let mut got = self.raw_tracks(lane, &stubs).await?;
        Ok(tracks
            .into_iter()
            .filter_map(|t| if t.is_stub() { got.remove(&t.id) } else { Some(t) })
            .collect())
    }

    async fn raw_playlist(&self, id: &str) -> Result<(api::Playlist, Vec<api::Track>), StreamError> {
        let id = id.trim();
        let path = if id.starts_with("soundcloud:system-playlists:") {
            format!("/system-playlists/{id}")
        } else {
            format!("/playlists/{}", num_id(id)?)
        };
        let mut p: api::Playlist = self.get(Lane::Catalog, &path, &[], false).await?;
        self.playlist_page(&p);
        let tracks = self.filled(Lane::Catalog, std::mem::take(&mut p.tracks)).await?;
        for t in &tracks {
            self.track(t);
        }
        Ok((p, tracks))
    }

    async fn album(&self, id: &str) -> Result<StreamAlbum, StreamError> {
        if let Some(a) = self.albums.get(id) {
            return Ok(a);
        }
        let (p, tracks) = self.raw_playlist(id).await?;
        let a = api::album(&p, &tracks);
        self.albums.put(id, a.clone());
        Ok(a)
    }

    async fn playlist(&self, id: &str) -> Result<StreamPlaylist, StreamError> {
        if let Some(p) = self.playlists.get(id) {
            return Ok(p);
        }
        let (p, tracks) = self.raw_playlist(id).await?;
        let out = api::playlist(&p, &tracks);
        self.playlists.put(id, out.clone());
        Ok(out)
    }

    async fn artist_page(&self, id: &str) -> Result<StreamArtist, StreamError> {
        if let Some(a) = self.artists.get(id) {
            return Ok(a);
        }
        let id = num_id(id)?;
        let u: api::User = self.get(Lane::Catalog, &format!("/users/{id}"), &[], false).await?;
        if u.id == 0 {
            return Err(StreamError::Unavailable);
        }
        let (albums, _) = self
            .pages::<api::Playlist>(
                Lane::Catalog,
                &format!("/users/{id}/albums"),
                &[("limit", "50".into())],
                false,
                ARTIST_ALBUMS_MAX,
            )
            .await?;
        let mut a = self.artist(&u);
        a.albums = albums
            .iter()
            .map(|p| {
                self.playlist_page(p);
                let mut al = api::album(p, &[]);
                if al.artists.is_empty() {
                    al.artists = vec![api::user_ref(&u)];
                }
                al
            })
            .collect();
        a.albums.sort_by(|x, y| {
            (y.year.unwrap_or(i32::MIN), &y.release_date).cmp(&(x.year.unwrap_or(i32::MIN), &x.release_date))
        });
        self.artists.put(id, a.clone());
        Ok(a)
    }

    /// A list of tracks at `path` (discovery), cached by `key`.
    async fn track_list(&self, key: String, path: String, limit: u32) -> Result<Vec<StreamTrack>, StreamError> {
        if let Some(hit) = self.lists.get(&key) {
            return Ok(hit);
        }
        let p: api::Page<api::Track> = self.get(Lane::Catalog, &path, &[("limit", limit.to_string())], false).await?;
        let out = self.tracks_of(&p.collection);
        self.lists.put(&key, out.clone());
        Ok(out)
    }

    async fn similar_artists(&self, id: &str) -> Result<Vec<StreamArtist>, StreamError> {
        let id = num_id(id)?;
        if let Some(hit) = self.similar.get(id) {
            return Ok(hit);
        }
        let p: api::Page<api::User> = self
            .get(Lane::Catalog, &format!("/users/{id}/relatedartists"), &[("limit", RELATED_ARTISTS.to_string())], false)
            .await?;
        let out: Vec<StreamArtist> = p.collection.iter().filter(|u| u.id > 0).map(|u| self.artist(u)).collect();
        self.similar.put(id, out.clone());
        Ok(out)
    }

    // -- the account's data -------------------------------------------------

    /// Every like (up to [`LIKES_MAX`]); keeps what the head check needs:
    /// whether the list was cut, and how the `/me` count relates to it.
    async fn liked_all(&self) -> Result<Vec<StreamTrack>, StreamError> {
        let (tracks, complete) = self.liked(LIKES_MAX).await?;
        self.likes_capped.store(!complete, Ordering::SeqCst);
        let skew = if complete {
            self.load_me().await.ok().and_then(|m| m.likes()).map(|c| c as i64 - tracks.len() as i64)
        } else {
            None
        };
        *lock(&self.likes_skew) = skew;
        Ok(tracks)
    }

    /// The length the list has now, from the `/me` count (as the full
    /// read counted it), if it can be told.
    async fn liked_total(&self) -> Option<u32> {
        let skew = (*lock(&self.likes_skew))?;
        let count = self.load_me().await.ok()?.likes()?;
        u32::try_from(count as i64 - skew).ok()
    }

    async fn liked(&self, max: usize) -> Result<(Vec<StreamTrack>, bool), StreamError> {
        let me = self.me_id().await?;
        let (items, complete) = self
            .pages::<api::Wrapped>(
                Lane::SignedIn,
                &format!("/users/{me}/track_likes"),
                &[("limit", PAGE.min(max as u32).to_string())],
                true,
                max,
            )
            .await?;
        let tracks: Vec<api::Track> = items.into_iter().filter_map(|w| w.track).collect();
        let tracks = self.filled(Lane::SignedIn, tracks).await?;
        Ok((self.tracks_of(&tracks), complete))
    }

    async fn set_liked(&self, id: &str, liked: bool) -> Result<(), StreamError> {
        let me = self.me_id().await?;
        let id = num_id(id)?;
        let method = if liked { reqwest::Method::PUT } else { reqwest::Method::DELETE };
        let path = format!("/users/{me}/track_likes/{id}");
        match self.gate.run(Lane::SignedIn, self.request(method, &path, &[], true)).await {
            // Unliking what is not liked.
            Ok(_) | Err(StreamError::Unavailable) if !liked => Ok(()),
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
    }

    async fn my_playlists(&self) -> Result<Vec<StreamPlaylist>, StreamError> {
        let me = self.me_id().await?;
        let (own, _) = self
            .pages::<api::Playlist>(
                Lane::SignedIn,
                &format!("/users/{me}/playlists_without_albums"),
                &[("limit", "100".into())],
                true,
                PLAYLISTS_MAX,
            )
            .await?;
        let (liked, _) = self
            .pages::<api::Wrapped>(
                Lane::SignedIn,
                &format!("/users/{me}/playlist_likes"),
                &[("limit", "100".into())],
                true,
                PLAYLISTS_MAX,
            )
            .await?;
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for p in own.iter().chain(liked.iter().filter_map(|w| w.playlist.as_ref().or(w.system_playlist.as_ref()))) {
            let Some(key) = p.key() else { continue };
            if !seen.insert(key) {
                continue;
            }
            self.playlist_page(p);
            let mut pl = api::playlist(p, &[]);
            pl.tracks.clear();
            out.push(pl);
        }
        Ok(out)
    }

    async fn recent(&self) -> Result<Vec<Played>, StreamError> {
        self.need_login()?;
        let (items, _) = self
            .pages::<api::Wrapped>(Lane::SignedIn, "/me/play-history/tracks", &[("limit", "200".into())], true, HISTORY_MAX)
            .await?;
        Ok(items
            .iter()
            .filter_map(|w| {
                let t = w.track.as_ref().filter(|t| !t.is_stub())?;
                Some(Played {
                    track: self.track(t),
                    at_ms: w.played_at?,
                })
            })
            .collect())
    }

    // -- suggestions ----------------------------------------------------------

    async fn suggestions(&self, feed: SuggestionFeed) -> Result<Suggestions, StreamError> {
        self.need_login()?;
        match feed {
            SuggestionFeed::Home => self.home().await,
            SuggestionFeed::History => {
                let p: api::Page<api::Wrapped> =
                    self.get(Lane::SignedIn, "/me/play-history/tracks", &[("limit", "50".into())], true).await?;
                let tracks: Vec<StreamTrack> =
                    p.collection.iter().filter_map(|w| w.track.as_ref()).filter(|t| !t.is_stub()).map(|t| self.track(t)).collect();
                Ok(Suggestions {
                    tracks: weighted(tracks, SuggestionKind::Recent),
                    ..Suggestions::default()
                })
            }
            SuggestionFeed::Followings => {
                let me = self.me_id().await?;
                let p: api::Page<api::User> =
                    self.get(Lane::SignedIn, &format!("/users/{me}/followings"), &[("limit", "200".into())], true).await?;
                let len = p.collection.len();
                Ok(Suggestions {
                    artists: p
                        .collection
                        .iter()
                        .filter(|u| u.id > 0)
                        .enumerate()
                        .map(|(i, u)| SuggestedArtist {
                            artist: api::user_ref(u),
                            kind: SuggestionKind::Followed,
                            weight: rank_weight(i, len),
                        })
                        .collect(),
                    ..Suggestions::default()
                })
            }
            SuggestionFeed::Reposts => {
                let me = self.me_id().await?;
                let p: api::Page<api::Wrapped> = self
                    .get(Lane::SignedIn, &format!("/stream/users/{me}/reposts"), &[("limit", "50".into())], true)
                    .await?;
                let tracks: Vec<StreamTrack> =
                    p.collection.iter().filter_map(|w| w.track.as_ref()).filter(|t| !t.is_stub()).map(|t| self.track(t)).collect();
                Ok(Suggestions {
                    tracks: weighted(tracks, SuggestionKind::Reposted),
                    ..Suggestions::default()
                })
            }
            _ => Err(StreamError::Unsupported),
        }
    }

    /// The personal shelves of the home (`/mixed-selections`, signed in):
    /// the tracks of the first few system playlists made for the user.
    async fn home(&self) -> Result<Suggestions, StreamError> {
        let p: api::Page<api::Selection> =
            self.get(Lane::SignedIn, "/mixed-selections", &[("limit", "20".into())], true).await?;
        let personal = |urn: &str| ["personal", "made-for", "your", "daily", "weekly"].iter().any(|w| urn.contains(w));
        let mut urns = Vec::new();
        for s in &p.collection {
            let shelf = s.urn.as_deref().unwrap_or("");
            for item in &s.items.collection {
                let Some(urn) = item.urn.as_deref().filter(|u| u.starts_with("soundcloud:system-playlists:")) else {
                    continue;
                };
                if (personal(shelf) || personal(urn)) && !urns.iter().any(|u: &String| u == urn) {
                    urns.push(urn.to_owned());
                }
            }
        }
        log::debug!("soundcloud: {} personal shelves on the home", urns.len());
        let mut tracks = Vec::new();
        for urn in urns.iter().take(HOME_SHELVES) {
            match self.raw_playlist(urn).await {
                Ok((_, list)) => tracks.extend(self.tracks_of(&list)),
                Err(StreamError::RateLimited { retry_after }) => return Err(StreamError::RateLimited { retry_after }),
                Err(e) => log::debug!("soundcloud: shelf {urn}: {e}"),
            }
        }
        let mut seen = std::collections::HashSet::new();
        tracks.retain(|t| seen.insert(t.id.clone()));
        Ok(Suggestions {
            tracks: weighted(tracks, SuggestionKind::QuickPicks),
            ..Suggestions::default()
        })
    }

    // -- sound ----------------------------------------------------------------

    /// Where a track's stream is now (see [`hls`]); `fresh` asks the track
    /// again (its `track_authorization` and stream urls expire).
    async fn plan(&self, id: &str, fresh: bool) -> Result<(hls::Plan, u32), StreamError> {
        let t = self.raw_track(id, fresh).await?;
        let Some(tc) = t.playable_transcoding() else {
            if t.drm_only() {
                log::info!("soundcloud: track {id} has only encrypted streams");
                return Err(StreamError::Drm);
            }
            return Err(StreamError::Unavailable);
        };
        let kbps = match tc.preset.as_str() {
            "aac_96k" => 96,
            "aac_256k" => 256,
            _ => 160,
        };
        let mut query = Vec::new();
        if let Some(a) = t.track_authorization.as_deref().filter(|a| !a.is_empty()) {
            query.push(("track_authorization", a.to_owned()));
        }
        let su: api::StreamUrl = self.get(Lane::Interactive, &tc.url, &query, false).await?;
        if su.url.is_empty() {
            return Err(StreamError::Parse("no stream url".into()));
        }
        let text = self.fetch_text(&su.url).await?;
        Ok((hls::parse_playlist(&text)?, kbps))
    }
}

/// Suggested tracks weighted by their place.
fn weighted(tracks: Vec<StreamTrack>, kind: SuggestionKind) -> Vec<SuggestedTrack> {
    let len = tracks.len();
    tracks
        .into_iter()
        .enumerate()
        .map(|(i, track)| SuggestedTrack {
            track,
            kind,
            weight: rank_weight(i, len),
        })
        .collect()
}

fn search_key(kind: &str, q: &str) -> String {
    format!("{kind}:{}", q.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
}

fn part_of<T>(r: Result<T, StreamError>, first_err: &mut Option<StreamError>) -> Option<T> {
    r.map_err(|e| {
        first_err.get_or_insert(e);
    })
    .ok()
}

/// A SoundCloud id is a number; anything else is no such item.
/// How often a thread waiting for a lookup asks whether it is still wanted.
const WANTED_POLL: Duration = Duration::from_millis(20);

/// Runs `fut` on the runtime and waits for it on this (non-runtime)
/// thread while `wanted` says so; `None` when it stopped wanting it (the
/// lookup is then aborted).
fn wait_while<T: Send + 'static>(
    handle: &tokio::runtime::Handle,
    fut: impl Future<Output = T> + Send + 'static,
    wanted: &(dyn Fn() -> bool + Sync),
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let task = handle.spawn(async move {
        let _ = tx.send(fut.await);
    });
    loop {
        if !wanted() {
            task.abort();
            return None;
        }
        match rx.recv_timeout(WANTED_POLL) {
            Ok(v) => return Some(v),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn num_id(id: &str) -> Result<&str, StreamError> {
    let id = id.trim();
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) { Ok(id) } else { Err(StreamError::Unavailable) }
}

fn unsupported<'a, T: Send + 'a>() -> BoxFuture<'a, Result<T, StreamError>> {
    Box::pin(std::future::ready(Err(StreamError::Unsupported)))
}

impl StreamService for Soundcloud {
    fn service(&self) -> Service {
        Service::Soundcloud
    }

    fn account(&self) -> Account {
        self.inner.account().clone()
    }

    /// Pasted cookies or token, or the sign-in window's cookies
    /// ([`LoginInput::Cookies`]); the window itself is the runtime's.
    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>> {
        self.run(move |inner| async move { inner.login(input).await })
    }

    fn logout(&self) {
        self.inner.logout();
    }

    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        self.search_in_parts(query, Arc::new(|_| {}))
    }

    /// Tracks, albums, artists and playlists: four requests at once.
    fn search_in_parts(&self, query: &str, part: SearchParts) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.to_owned();
        self.run(move |inner| async move { inner.search(&q, true, part).await })
    }

    /// Tracks, albums and artists (playlists are for the chip).
    fn search_top(&self, query: &str, part: SearchParts) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.to_owned();
        self.run(move |inner| async move { inner.search(&q, false, part).await })
    }

    /// Songs are every track (SoundCloud has no music videos).
    fn search_kind(&self, query: &str, kind: SearchKind) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.trim().to_owned();
        self.run(move |inner| async move {
            let mut out = StreamSearch::default();
            if q.is_empty() {
                return Ok(out);
            }
            let key = search_key(&format!("{kind:?}"), &q);
            if let Some(hit) = inner.searches.get(&key) {
                return Ok(hit);
            }
            match kind {
                SearchKind::Songs => out.tracks = inner.search_tracks(&q).await?,
                SearchKind::Videos => return Ok(out),
                SearchKind::Albums => out.albums = inner.search_albums(&q).await?,
                SearchKind::Artists => out.artists = inner.search_artists(&q).await?,
            }
            inner.searches.put(&key, out.clone());
            Ok(out)
        })
    }

    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.track_one(&id).await })
    }

    /// `/tracks?ids=`: 50 a request.
    fn tracks(&self, ids: &[String]) -> BoxFuture<'_, Result<Vec<Option<StreamTrack>>, StreamError>> {
        let ids = ids.to_vec();
        self.run(move |inner| async move { inner.tracks_many(&ids).await })
    }

    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.album(&id).await })
    }

    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.artist_page(&id).await })
    }

    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.playlist(&id).await })
    }

    fn forget(&self, kind: EntityKind, id: &str) {
        let i = &self.inner;
        match kind {
            EntityKind::Track => {
                i.tracks.remove(id);
                i.raw.remove(id);
            }
            EntityKind::Album => i.albums.remove(id),
            EntityKind::Artist => {
                i.artists.remove(id);
                i.similar.remove(id);
                for what in ["top", "station"] {
                    i.lists.remove(&format!("{what}:{id}"));
                }
            }
            EntityKind::Playlist => i.playlists.remove(id),
        }
    }

    fn paused_until(&self) -> Option<std::time::SystemTime> {
        self.inner.gate.paused_until()
    }

    /// SoundCloud cannot look ISRCs up.
    fn find_isrc(&self, _isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        Box::pin(std::future::ready(Ok(None)))
    }

    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        self.run(move |inner| async move { inner.liked_all().await })
    }

    fn liked_partial(&self) -> bool {
        self.inner.likes_capped.load(Ordering::SeqCst)
    }

    fn liked_head(&self) -> BoxFuture<'_, Result<LikedHead, StreamError>> {
        self.run(move |inner| async move {
            let (tracks, complete) = inner.liked(50).await?;
            let total = if complete { u32::try_from(tracks.len()).ok() } else { inner.liked_total().await };
            Ok(LikedHead {
                total,
                tracks,
                complete,
            })
        })
    }

    /// `PUT` / `DELETE /users/{me}/track_likes/{id}`.
    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move { inner.set_liked(&id, liked).await })
    }

    /// The user's own playlists and the liked ones (albums included).
    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>> {
        self.run(move |inner| async move { inner.my_playlists().await })
    }

    /// Read only: nothing is written to SoundCloud's playlists.
    fn create_playlist(&self, _name: &str) -> BoxFuture<'_, Result<String, StreamError>> {
        unsupported()
    }

    fn playlist_edit(&self, _id: &str, _add: &[String], _remove: &[String]) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        unsupported()
    }

    /// `/me/play-history/tracks`, up to 1000 plays.
    fn recent(&self) -> BoxFuture<'_, Result<Vec<Played>, StreamError>> {
        self.run(move |inner| async move { inner.recent().await })
    }

    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        self.open_audio_while(id, &|| true)
    }

    fn open_audio_while(
        &self,
        id: &str,
        wanted: &(dyn Fn() -> bool + Sync),
    ) -> Result<Box<dyn AudioStream>, StreamError> {
        let id = num_id(id)?.to_owned();
        // A DRM-only track fails here, at the open, so the engine can hand
        // it to SoundCloud's own player; inside the stream's first read the
        // error would only be a failed read. The listing just shown usually
        // has the track already. The lookup gives up (and is stopped) once
        // the track is no longer wanted.
        if tokio::runtime::Handle::try_current().is_err() {
            let inner = Arc::clone(&self.inner);
            let probe = id.clone();
            let looked = wait_while(
                self.runtime.handle(),
                async move { inner.raw_track(&probe, false).await },
                wanted,
            );
            match looked {
                Some(Ok(t)) if t.playable_transcoding().is_none() && t.drm_only() => {
                    log::info!("soundcloud: track {id} has only encrypted streams");
                    return Err(StreamError::Drm);
                }
                Some(_) => {}
                None => return Err(StreamError::Network("no longer wanted".into())),
            }
        }
        let inner = Arc::clone(&self.inner);
        let first = Arc::new(AtomicBool::new(true));
        let kbps = Arc::new(AtomicU64::new(160));
        let k = Arc::clone(&kbps);
        let resolve: hls::ResolveFn = Arc::new(move || {
            let (inner, id, first, k) = (Arc::clone(&inner), id.clone(), Arc::clone(&first), Arc::clone(&k));
            Box::pin(async move {
                // The listing just shown may serve the first resolve.
                let fresh = !first.swap(false, Ordering::SeqCst);
                let (plan, kbps) = inner.plan(&id, fresh).await?;
                k.store(u64::from(kbps), Ordering::SeqCst);
                Ok(plan)
            })
        });
        Ok(Box::new(hls::HlsStream::new(
            self.runtime.handle().clone(),
            self.inner.http.clone(),
            resolve,
            kbps,
            self.inner.media.clone(),
        )))
    }

    /// Nothing of SoundCloud is saved to disk.
    fn save_audio(&self, _id: &str, _dir: &Path) -> BoxFuture<'_, Result<PathBuf, StreamError>> {
        unsupported()
    }

    /// `/tracks/{id}/related`.
    fn related(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = track_id.to_owned();
        self.run(move |inner| async move {
            let id = num_id(&id)?.to_owned();
            inner.track_list(format!("related:{id}"), format!("/tracks/{id}/related"), DISCOVERY).await
        })
    }

    /// The track's station.
    fn radio(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = track_id.to_owned();
        self.run(move |inner| async move {
            let id = num_id(&id)?.to_owned();
            let mut out = inner
                .track_list(
                    format!("radio:{id}"),
                    format!("/stations/soundcloud:track-stations:{id}/tracks"),
                    DISCOVERY,
                )
                .await?;
            out.retain(|t| t.id.id != id);
            Ok(out)
        })
    }

    /// `/users/{id}/relatedartists`.
    fn similar_artists(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamArtist>, StreamError>> {
        let id = artist_id.to_owned();
        self.run(move |inner| async move { inner.similar_artists(&id).await })
    }

    /// `/users/{id}/toptracks`.
    fn artist_top(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = artist_id.to_owned();
        self.run(move |inner| async move {
            let id = num_id(&id)?.to_owned();
            inner.track_list(format!("top:{id}"), format!("/users/{id}/toptracks"), DISCOVERY).await
        })
    }

    /// The artist's station.
    fn artist_radio(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = artist_id.to_owned();
        self.run(move |inner| async move {
            let id = num_id(&id)?.to_owned();
            inner
                .track_list(
                    format!("station:{id}"),
                    format!("/stations/soundcloud:artist-stations:{id}/tracks"),
                    DISCOVERY,
                )
                .await
        })
    }

    fn suggestion_feeds(&self) -> &'static [SuggestionFeed] {
        &[SuggestionFeed::Home, SuggestionFeed::History, SuggestionFeed::Followings, SuggestionFeed::Reposts]
    }

    fn suggestions(&self, feed: SuggestionFeed) -> BoxFuture<'_, Result<Suggestions, StreamError>> {
        self.run(move |inner| async move { inner.suggestions(feed).await })
    }

    /// The page SoundCloud listed for it; the embedded player for an item
    /// not listed in this run (SoundCloud has no page by id).
    fn web_url(&self, kind: EntityKind, id: &str) -> String {
        let (key, api) = match kind {
            EntityKind::Track => (format!("track:{id}"), "tracks"),
            EntityKind::Artist => (format!("user:{id}"), "users"),
            EntityKind::Album | EntityKind::Playlist => (format!("playlist:{id}"), "playlists"),
        };
        if let Some(url) = lock(&self.inner.permalinks).get(&key) {
            return url.clone();
        }
        if let Some(rest) = id.strip_prefix("soundcloud:system-playlists:") {
            return format!("https://soundcloud.com/discover/sets/{rest}");
        }
        format!("https://w.soundcloud.com/player/?url=https%3A%2F%2Fapi.soundcloud.com%2F{api}%2F{id}")
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_lookup_no_longer_wanted_is_given_up_and_stopped() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
        let wanted = Arc::new(AtomicBool::new(true));
        let dropped = Arc::new(AtomicBool::new(false));
        struct Flag(Arc<AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let flag = Flag(Arc::clone(&dropped));
        let w = Arc::clone(&wanted);
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            w.store(false, Ordering::SeqCst);
        });
        let began = std::time::Instant::now();
        let got = super::wait_while(
            rt.handle(),
            async move {
                let _flag = flag;
                std::future::pending::<()>().await
            },
            &|| wanted.load(Ordering::SeqCst),
        );
        assert!(got.is_none());
        assert!(began.elapsed() < std::time::Duration::from_secs(2));
        t.join().unwrap();
        let t = std::time::Instant::now();
        while !dropped.load(Ordering::SeqCst) {
            assert!(t.elapsed() < std::time::Duration::from_secs(2), "the lookup is stopped");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let fine = super::wait_while(rt.handle(), async { 7 }, &|| true);
        assert_eq!(fine, Some(7));
    }

    use super::*;

    #[test]
    fn the_client_id_is_found_in_a_script() {
        let js = r#"…,e.exports={client_id:"39AVoiZVrdvJEMFLH8T0c5WGshQaAPRJ",env:"production"}"#;
        assert_eq!(find_client_id(js).as_deref(), Some("39AVoiZVrdvJEMFLH8T0c5WGshQaAPRJ"));
        assert_eq!(find_client_id(r#"client_id:"short""#), None);
        let html = r#"<script crossorigin src="https://a-v2.sndcdn.com/assets/0-6a272fe8.js"></script>
            <script crossorigin src="https://a-v2.sndcdn.com/assets/55-9bfd359c.js"></script>
            <script src="/other.js"></script>"#;
        assert_eq!(
            script_urls(html),
            ["https://a-v2.sndcdn.com/assets/55-9bfd359c.js", "https://a-v2.sndcdn.com/assets/0-6a272fe8.js"]
        );
    }

    #[test]
    fn the_client_id_replaces_an_old_one() {
        assert_eq!(with_client_id("https://x/a", "N"), "https://x/a?client_id=N");
        assert_eq!(
            with_client_id("https://x/a?offset=5&client_id=OLD&limit=2", "N"),
            "https://x/a?offset=5&limit=2&client_id=N"
        );
    }

    #[test]
    fn ids_are_numbers() {
        assert_eq!(num_id(" 123 "), Ok("123"));
        assert_eq!(num_id("abc"), Err(StreamError::Unavailable));
        assert_eq!(num_id(""), Err(StreamError::Unavailable));
    }

    #[test]
    fn web_urls_without_a_listing() {
        let dir = std::env::temp_dir().join("nikgob-soundcloud-test-web");
        let mut cfg = SoundcloudConfig::new(dir);
        cfg.memory_store = true;
        let sc = Soundcloud::new(cfg).unwrap();
        assert!(sc.web_url(EntityKind::Track, "5").contains("api.soundcloud.com%2Ftracks%2F5"));
        assert_eq!(
            sc.web_url(EntityKind::Playlist, "soundcloud:system-playlists:trending-by-genre:trap"),
            "https://soundcloud.com/discover/sets/trending-by-genre:trap"
        );
        lock(&sc.inner.permalinks).insert("track:5".into(), "https://soundcloud.com/a/b".into());
        assert_eq!(sc.web_url(EntityKind::Track, "5"), "https://soundcloud.com/a/b");
    }
}
