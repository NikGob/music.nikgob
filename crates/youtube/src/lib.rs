//! YouTube Music as a [`StreamService`], on top of RustyPipe.
//!
//! * **Catalog** (search, tracks, albums, artists, playlists) works without
//!   a login, through the YouTube Music web client.
//! * **Audio** works without a login too: the player is fetched with the
//!   visionOS client (plain URLs, no PO token, see `player`), RustyPipe's
//!   own player being the fallback. [`Youtube::open_audio`]
//!   returns a lazy ranged-HTTP reader (see `range`), [`Youtube::save_audio`]
//!   keeps the Opus stream as an Ogg Opus file without re-encoding.
//! * **Login** is by cookies of the user's main account
//!   ([`LoginInput::Cookies`]): RustyPipe keeps them in its cache file in
//!   [`YoutubeConfig::data_dir`]. OAuth device codes only work for RustyPipe's
//!   TV client, which cannot read any user data, so [`LoginInput::Browser`]
//!   answers [`StreamError::Unsupported`] here; the app's «Войти через
//!   Google» window (the runtime's `WebLogin`) gets the cookies instead and
//!   logs in with them. Likes, playlist creation and edits
//!   go through RustyPipe's raw request (see `innertube`).
//! * **Discovery**, each request in the spaced
//!   catalogue lane and kept an hour in memory (see `memo`): the radio of
//!   a track ([`StreamService::radio`], one `next` request, ~50 tracks, the
//!   best source of songs new to the listener), the related tracks
//!   ([`StreamService::related`], «Вам может понравиться»: the track's
//!   details, then its related page — two requests), and from the artist
//!   page (one request, shared): the top songs it shows
//!   ([`StreamService::artist_top`], about five), the similar artists and
//!   the id of the artist's radio ([`StreamService::artist_radio`], one
//!   more request). [`StreamService::cached_audio`] hands out a complete
//!   file of the audio cache, never the network.
//! * **Suggestions** (see `suggest`), signed in only,
//!   one request a feed in the signed-in lane: the home's quick picks and
//!   listen again, and the history.
//!
//! Every future runs on the service's own small tokio runtime (so the pooled
//! connections and timers always belong to one runtime) and is awaited
//! through a handle that cancels it when dropped; callers may poll on any
//! executor.

mod cache;
mod convert;
mod counterpart;
mod innertube;
mod limit;
mod memo;
mod player;
mod range;
mod remux;
mod suggest;

use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use nikgob_core::streaming::{
    Account, AudioStream, BoxFuture, EntityKind, LikedHead, LoginInput, Played, SearchKind,
    SearchParts, Service, StreamAlbum, StreamArtist, StreamError, StreamPlaylist, StreamSearch,
    StreamService, StreamTrack, SuggestionFeed, Suggestions, format, join_names,
};
use limit::Lane;
use rustypipe::client::{ClientType, RustyPipe};
use rustypipe::error::{AuthError, Error as RpError, ExtractionError, UnavailabilityReason};
use rustypipe::model::TrackType;
use rustypipe::param::{Country, Language};
use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;

pub use cache::DEFAULT_LIMIT as DEFAULT_CACHE_LIMIT;
pub use remux::Tags;

pub use range::buffers as stream_buffers;
use range::{HttpStream, ResolveFn};

/// Upper bound of tracks fetched for one playlist (YouTube caps listings
/// around 5000 anyway).
const PLAYLIST_LIMIT: usize = 10_000;
/// A video whose player refused it is not asked again for this long (a
/// preload, a retry and the next play fail at once with the same reason).
const REFUSED_TTL: Duration = Duration::from_secs(30 * 60);
/// Name shown when the account menu does not say.
const DEFAULT_NAME: &str = "YouTube Music";
/// Our own non-secret account notes next to RustyPipe's cache.
const ACCOUNT_FILE: &str = "youtube_account.json";
/// A resolved stream URL is used again until this long before it expires.
const URL_MARGIN: Duration = Duration::from_secs(30 * 60);
/// How long a stream URL lives when it does not say (YouTube's are ~6 h).
const URL_LIFE: Duration = Duration::from_secs(5 * 3600);
/// Resolved URLs kept.
const URLS_MAX: usize = 200;
/// How long discovery answers are kept.
const DISCOVERY_TTL: Duration = Duration::from_secs(3600);

/// Where and how the service runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YoutubeConfig {
    /// Holds RustyPipe's cache (`rustypipe_cache.json`: client versions,
    /// deobfuscation code, **the login cookies**) and our account notes.
    /// Created if missing.
    pub data_dir: PathBuf,
    /// Content language, e.g. `ru` or `ru-RU`; unknown → English.
    pub language: String,
    /// Content country, e.g. `RU`; unknown → US.
    pub country: String,
    /// Optional `rustypipe-botguard` binary (an external program) for
    /// RustyPipe's fallback player, which then tries the desktop client with
    /// PO tokens first. Not needed for the default visionOS player.
    pub botguard_bin: Option<PathBuf>,
}

impl YoutubeConfig {
    /// Russian content for Russia, no botguard.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            language: "ru".into(),
            country: "RU".into(),
            botguard_bin: None,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct AccountNotes {
    name: Option<String>,
}

struct Inner {
    rp: RustyPipe,
    http: reqwest::Client,
    account: Mutex<Account>,
    data_dir: PathBuf,
    /// Language and country for our own player requests.
    hl: String,
    gl: String,
    /// Recently played audio on disk.
    cache: cache::AudioCache,
    /// Videos the player refused lately, by video id, and why.
    refused: Mutex<std::collections::HashMap<String, (std::time::Instant, StreamError)>>,
    /// Turns and the service-wide pause (see `limit`).
    gate: limit::Gate,
    /// Which player routes answered and failed lately (see `player`).
    chain: player::Chain,
    /// Stream URLs resolved lately, by video id, with when they expire.
    urls: Mutex<std::collections::HashMap<String, (std::time::SystemTime, range::Resolved)>>,
    /// What discovery read off artist pages, by artist id.
    pages: memo::Memo<convert::ArtistPage>,
    /// Related tracks and radios, by `related:{video}`, `radio:{video}`,
    /// `artist_radio:{channel}`.
    lists: memo::Memo<Vec<StreamTrack>>,
}

impl Inner {
    fn account(&self) -> MutexGuard<'_, Account> {
        self.account.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn need_login(&self) -> Result<(), StreamError> {
        if self.account().is_logged_in() {
            Ok(())
        } else {
            Err(StreamError::NotLoggedIn)
        }
    }

    /// Display name of the logged-in user (for "is this playlist mine").
    fn me(&self) -> Option<String> {
        match &*self.account() {
            Account::LoggedIn { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Maps an error of an authenticated call; a refused session marks the
    /// account as expired.
    fn user_err(&self, e: RpError) -> StreamError {
        let e = map_err(e);
        if e == StreamError::NotLoggedIn {
            *self.account() = Account::Failed("YouTube session expired, log in again".into());
        }
        e
    }

    fn notes_path(&self) -> PathBuf {
        self.data_dir.join(ACCOUNT_FILE)
    }

    /// A stream URL resolved lately that is still good.
    fn known_url(&self, id: &str) -> Option<range::Resolved> {
        let urls = self.urls.lock().unwrap_or_else(PoisonError::into_inner);
        let (expires, r) = urls.get(id)?;
        (std::time::SystemTime::now() + URL_MARGIN < *expires).then(|| r.clone())
    }

    fn keep_url(&self, id: &str, r: &range::Resolved) {
        let expires = url_expiry(&r.url)
            .unwrap_or_else(|| std::time::SystemTime::now() + URL_LIFE);
        let mut urls = self.urls.lock().unwrap_or_else(PoisonError::into_inner);
        if urls.len() >= URLS_MAX {
            let now = std::time::SystemTime::now();
            urls.retain(|_, (e, _)| *e > now + URL_MARGIN);
            if urls.len() >= URLS_MAX {
                urls.clear();
            }
        }
        urls.insert(id.to_owned(), (expires, r.clone()));
    }

    /// The discovery part of an artist page: one request, or none while
    /// it is kept.
    async fn artist_page(&self, id: &str) -> Result<convert::ArtistPage, StreamError> {
        if let Some(p) = self.pages.get(id) {
            return Ok(p);
        }
        let a = self
            .gate
            .run(Lane::Catalog, async { self.rp.query().music_artist(id, false).await.map_err(map_err) })
            .await?;
        let p = convert::artist_page(&a);
        self.pages.put(id, p.clone());
        Ok(p)
    }

    /// A radio's first page (about 50 tracks) without `seed`, kept under
    /// `key`.
    async fn radio(&self, key: &str, radio_id: &str, seed: Option<&str>) -> Result<Vec<StreamTrack>, StreamError> {
        if let Some(hit) = self.lists.get(key) {
            return Ok(hit);
        }
        let p = self
            .gate
            .run(Lane::Catalog, async { self.rp.query().music_radio(radio_id).await.map_err(map_err) })
            .await?;
        let out = convert::tracks_without(&p.items, seed);
        self.lists.put(key, out.clone());
        Ok(out)
    }

    /// «Вам может понравиться» of a track: its details (for the id of the
    /// related page), then that page.
    async fn related(&self, id: &str) -> Result<Vec<StreamTrack>, StreamError> {
        let key = format!("related:{id}");
        if let Some(hit) = self.lists.get(&key) {
            return Ok(hit);
        }
        let d = self
            .gate
            .run(Lane::Catalog, async { self.rp.query().music_details(id).await.map_err(map_err) })
            .await?;
        let out = match d.related_id {
            Some(related) => {
                let r = self
                    .gate
                    .run(Lane::Catalog, async { self.rp.query().music_related(&related).await.map_err(map_err) })
                    .await?;
                convert::tracks_without(&r.tracks, Some(id))
            }
            None => Vec::new(),
        };
        self.lists.put(&key, out.clone());
        Ok(out)
    }

    fn drop_url(&self, id: &str) {
        self.urls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }
}

/// The `expire=` (unix seconds) of a signed stream URL.
fn url_expiry(url: &str) -> Option<std::time::SystemTime> {
    let query = url.split_once('?')?.1;
    let secs: u64 = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("expire="))?
        .parse()
        .ok()?;
    Some(std::time::UNIX_EPOCH + Duration::from_secs(secs))
}

/// The runtime every future of the service runs on.
struct Runtime(Option<tokio::runtime::Runtime>);

impl Runtime {
    fn handle(&self) -> &tokio::runtime::Handle {
        self.0
            .as_ref()
            .expect("runtime lives as long as the service")
            .handle()
    }
}

impl Drop for Runtime {
    /// The last `Arc<Youtube>` may go away inside some async context, where a
    /// blocking shutdown would panic.
    fn drop(&mut self) {
        if let Some(rt) = self.0.take() {
            rt.shutdown_background();
        }
    }
}

/// `cookies` (a `Cookie:` header or a cookies.txt) with the language and
/// country of the PREF cookie set to `hl` / `gl`.
fn prefer_language(cookies: &str, hl: &str, gl: &str) -> String {
    let pref = |value: &str| -> String {
        let mut parts: Vec<String> = value
            .split('&')
            .filter(|p| !p.is_empty() && !p.starts_with("hl=") && !p.starts_with("gl="))
            .map(str::to_owned)
            .collect();
        parts.push(format!("hl={hl}"));
        parts.push(format!("gl={gl}"));
        parts.join("&")
    };
    if cookies.lines().any(|l| l.split('\t').count() >= 7) {
        return cookies
            .lines()
            .map(|l| {
                let mut f: Vec<String> = l.split('\t').map(str::to_owned).collect();
                if f.len() >= 7 && f[5] == "PREF" {
                    f[6] = pref(&f[6]);
                }
                f.join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    cookies
        .split(';')
        .map(|c| {
            let lead = &c[..c.len() - c.trim_start().len()];
            match c.trim_start().strip_prefix("PREF=") {
                Some(v) => format!("{lead}PREF={}", pref(v)),
                None => c.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(";")
}

/// YouTube Music; see the crate docs.
pub struct Youtube {
    inner: Arc<Inner>,
    runtime: Runtime,
}

impl Youtube {
    /// Builds the client (reads RustyPipe's cache file, no network) and the
    /// service's runtime. Fails only if the runtime cannot start.
    pub fn new(config: YoutubeConfig) -> Result<Arc<Self>, StreamError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("youtube")
            .enable_all()
            .build()
            .map_err(|e| StreamError::Network(format!("cannot start the runtime: {e}")))?;
        if let Err(e) = std::fs::create_dir_all(&config.data_dir) {
            log::warn!("youtube: cannot create {}: {e}", config.data_dir.display());
        }
        let lang = Language::from_str(&config.language).unwrap_or(Language::En);
        let country = parse_country(&config.country).unwrap_or(Country::Us);
        let mut builder = RustyPipe::builder()
            .storage_dir(&config.data_dir)
            .lang(lang)
            .country(country)
            .timeout(Duration::from_secs(20));
        builder = match &config.botguard_bin {
            Some(bin) => builder.botguard_bin(bin.as_os_str()),
            None => builder.no_botguard(),
        };
        let rp = builder
            .build()
            .map_err(|e| StreamError::Network(format!("cannot build the YouTube client: {e}")))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| StreamError::Network(e.to_string()))?;
        let notes: AccountNotes = std::fs::read(config.data_dir.join(ACCOUNT_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let account = if rp.query().auth_enabled(ClientType::DesktopMusic) {
            Account::LoggedIn {
                name: notes.name.unwrap_or_else(|| DEFAULT_NAME.into()),
                premium: None,
                audio: true,
            }
        } else {
            Account::LoggedOut
        };
        Ok(Arc::new(Self {
            inner: Arc::new(Inner {
                rp,
                http,
                account: Mutex::new(account),
                data_dir: config.data_dir,
                hl: serde_json::to_value(lang)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "en".into()),
                gl: serde_json::to_value(country)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "US".into()),
                cache: cache::AudioCache::new(),
                refused: Mutex::new(std::collections::HashMap::new()),
                gate: limit::Gate::new(),
                chain: player::Chain::default(),
                urls: Mutex::new(std::collections::HashMap::new()),
                pages: memo::Memo::new(DISCOVERY_TTL, 300),
                lists: memo::Memo::new(DISCOVERY_TTL, 300),
            }),
            runtime: Runtime(Some(runtime)),
        }))
    }

    /// Keeps recently played audio in `dir` (see `cache`), up to
    /// [`DEFAULT_CACHE_LIMIT`] until [`StreamService::set_cache_limit`].
    pub fn with_audio_cache(self: Arc<Self>, dir: PathBuf) -> Arc<Self> {
        self.inner.cache.set_dir(dir);
        self
    }

    /// Gets the client ready in the background (visitor data, client
    /// version, a pooled connection), so the first search of a session does
    /// not pay for it (~2.2 s instead of ~0.6 s). One small request.
    pub fn warm_up(&self) {
        let inner = Arc::clone(&self.inner);
        self.runtime.handle().spawn(async move {
            let started = std::time::Instant::now();
            match player::no_panic(inner.rp.query().music_search_suggestion("music")).await {
                Ok(Ok(_)) => log::debug!("youtube: warmed up in {} ms", started.elapsed().as_millis()),
                Ok(Err(e)) => log::info!("youtube: warm-up failed: {e}"),
                Err(panic) => log::info!("youtube: warm-up failed: RustyPipe failed: {panic}"),
            }
        });
    }

    /// Runs `f` on the service's runtime; the returned future cancels it
    /// when dropped. A panic inside RustyPipe (it unwraps a failed visitor
    /// data request) is a network error of that request.
    fn run<T, F>(&self, f: impl FnOnce(Arc<Inner>) -> F) -> BoxFuture<'_, Result<T, StreamError>>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, StreamError>> + Send + 'static,
    {
        let fut = f(Arc::clone(&self.inner));
        let task = self.runtime.handle().spawn(async move {
            player::no_panic(fut)
                .await
                .unwrap_or_else(|panic| Err(StreamError::Network(format!("RustyPipe failed: {panic}"))))
        });
        Box::pin(Joined(task))
    }

    /// Metadata for the tags of a saved file (best effort).
    async fn tags(inner: &Inner, id: &str) -> Tags {
        match inner.rp.query().music_details(id).await {
            Ok(d) => {
                let t = convert::track(&d.track);
                Tags {
                    title: Some(t.title.clone()),
                    artist: (!t.artists.is_empty()).then(|| join_names(&t.artists)),
                    album: t.album.map(|a| a.title),
                }
            }
            Err(e) => {
                log::debug!("youtube: no tags for {id}: {e}");
                Tags::default()
            }
        }
    }
}

/// A video's song details from YouTube Music, else (no song details of it,
/// or an answer RustyPipe cannot read) the video's own page.
async fn details_or_page(i: &Inner, id: &str) -> Result<StreamTrack, StreamError> {
    let details = i
        .gate
        .run(Lane::Catalog, async { i.rp.query().music_details(id).await.map_err(map_err) })
        .await;
    match details {
        Ok(d) => Ok(convert::track(&d.track)),
        Err(e @ (StreamError::Unavailable | StreamError::Parse(_))) => {
            log::debug!("youtube: no song details of {id} ({e}), its own page then");
            let v = i
                .gate
                .run(Lane::Catalog, async { i.rp.query().video_details(id).await.map_err(map_err) })
                .await?;
            Ok(convert::video(&v))
        }
        Err(e) => Err(e),
    }
}

/// A spawned task, aborted when dropped.
/// What a save stopped by its caller ends with.
const CANCELLED: &str = "cancelled";

/// Raises its flag when dropped: the caller of a blocking job went away.
struct StopOnDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// All of `r`, a chunk at a time, unless `cancel` is raised meanwhile
/// (then an error, and what was read is let go).
fn read_unless(r: &mut impl Read, cancel: &std::sync::atomic::AtomicBool) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(std::io::Error::other(CANCELLED));
        }
        match r.read(&mut chunk) {
            Ok(0) => return Ok(bytes),
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

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

/// One part of a search; a failed part keeps the first error.
fn part_of<T>(r: Result<T, RpError>, first_err: &mut Option<StreamError>) -> Option<T> {
    r.map_err(|e| {
        first_err.get_or_insert(map_err(e));
    })
    .ok()
}

/// Logs how long one YouTube request took (`started`: the search's start)
/// and how it ended.
async fn timed<T>(
    kind: &str,
    started: std::time::Instant,
    f: impl Future<Output = Result<T, RpError>>,
) -> Result<T, RpError> {
    let r = f.await;
    match &r {
        Ok(_) => log::debug!("youtube: {kind} -> ok in {} ms", started.elapsed().as_millis()),
        Err(e) => log::info!(
            "youtube: {kind} -> failed in {} ms: {e}",
            started.elapsed().as_millis()
        ),
    }
    r
}

fn parse_country(s: &str) -> Option<Country> {
    serde_json::from_value(serde_json::Value::String(s.trim().to_ascii_uppercase())).ok()
}

/// RustyPipe's errors → [`StreamError`].
pub(crate) fn map_err(e: RpError) -> StreamError {
    match e {
        RpError::Extraction(ExtractionError::Unavailable { reason, msg }) => match reason {
            UnavailabilityReason::IpBan
            | UnavailabilityReason::VpnBan
            | UnavailabilityReason::Captcha
            | UnavailabilityReason::TryAgain => {
                log::warn!("youtube: blocked ({reason}): {msg}");
                StreamError::RateLimited { retry_after: None }
            }
            UnavailabilityReason::AgeRestricted => StreamError::AgeRestricted,
            UnavailabilityReason::Geoblocked => StreamError::RegionBlocked,
            _ => StreamError::Unavailable,
        },
        RpError::Extraction(ExtractionError::NotFound { .. }) => StreamError::Unavailable,
        RpError::Extraction(ExtractionError::BadRequest(_)) => StreamError::Http { status: 400 },
        RpError::Extraction(e) => StreamError::Parse(e.to_string()),
        RpError::HttpStatus(429, _) => StreamError::RateLimited { retry_after: None },
        RpError::HttpStatus(401 | 403, _) | RpError::Auth(AuthError::NoLogin) => {
            StreamError::NotLoggedIn
        }
        RpError::HttpStatus(status, _) => StreamError::Http { status },
        RpError::Auth(e) => {
            log::warn!("youtube: auth error: {e}");
            StreamError::NotLoggedIn
        }
        RpError::Http(e) => StreamError::Network(e.into_owned()),
        RpError::Other(e) => StreamError::Network(e.into_owned()),
    }
}

/// The audio of `id` over HTTP; a complete download goes to the cache when
/// `keep`.
fn stream(inner: &Arc<Inner>, runtime: tokio::runtime::Handle, id: &str, keep: bool) -> HttpStream {
    let (inner2, id2) = (Arc::clone(inner), id.to_owned());
    // The first resolve may reuse a URL resolved lately; one after a
    // refusal forgets that URL, benches the client that gave it and asks
    // the player again.
    let resolve: ResolveFn = Arc::new(move |refused: Option<range::Resolved>| {
        let (inner, id) = (Arc::clone(&inner2), id2.clone());
        Box::pin(async move {
            match &refused {
                None => {
                    if let Some(r) = inner.known_url(&id) {
                        log::debug!("youtube: {id}: stream URL reused");
                        return Ok(r);
                    }
                }
                Some(r) => {
                    inner.drop_url(&id);
                    inner.chain.refused(r);
                }
            }
            let known = inner
                .refused
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&id)
                .filter(|(at, _)| at.elapsed() < REFUSED_TTL)
                .map(|(_, e)| e.clone());
            if let Some(e) = known {
                log::info!("youtube: {id}: refused lately ({e}), not asked again");
                return Err(e);
            }
            inner.gate.check()?;
            let signed_in = inner.account().is_logged_in();
            let r = player::resolve(&inner.rp, &inner.http, &inner.chain, &id, &inner.gl, signed_in).await;
            inner.gate.note(&r);
            if let Ok(resolved) = &r {
                inner.keep_url(&id, resolved);
            }
            if let Err(e @ (StreamError::AgeRestricted | StreamError::RegionBlocked | StreamError::Unavailable)) = &r {
                log::info!("youtube: {id}: player refused: {e}");
                inner
                    .refused
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id.clone(), (std::time::Instant::now(), e.clone()));
            }
            r
        })
    });
    let on_complete: Option<range::CompleteFn> = keep.then(|| {
        let (inner, id) = (Arc::clone(inner), id.to_owned());
        Box::new(move |r: &range::Resolved, bytes: &mut dyn std::io::Read| inner.cache.store(&id, r, bytes)) as range::CompleteFn
    });
    HttpStream::new(runtime, inner.http.clone(), resolve, on_complete)
}

/// Unix ms of a history entry. YouTube only gives the day; entries of one
/// day are spread one second apart below its end, so the order survives.
fn played_at(date: Option<time::Date>, index_in_day: i64) -> Option<i64> {
    let end = date?.next_day()?.midnight().assume_utc().unix_timestamp() * 1000;
    Some(end - 1000 * (index_in_day + 1))
}

fn valid_isrc(isrc: &str) -> bool {
    isrc.len() == 12 && isrc.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl StreamService for Youtube {
    fn service(&self) -> Service {
        Service::Youtube
    }

    fn account(&self) -> Account {
        self.inner.account().clone()
    }

    fn login(&self, input: LoginInput) -> BoxFuture<'_, Result<(), StreamError>> {
        let LoginInput::Cookies(cookies) = input else {
            return Box::pin(std::future::ready(Err(StreamError::Unsupported)));
        };
        *self.inner.account() = Account::LoggingIn(None);
        self.run(move |inner| async move {
            // The browser's PREF cookie carries its interface language
            // (`hl=en`): YouTube may then answer in it, and Cyrillic titles
            // come transliterated («LEGENDARNAJa PYL'»). Ours wins.
            let cookies = prefer_language(&cookies, &inner.hl, &inner.gl);
            let text = cookies.trim();
            let set = if text.lines().any(|l| l.split('\t').count() >= 7) {
                inner.rp.user_auth_set_cookie_txt(text).await
            } else {
                let header = text
                    .strip_prefix("Cookie:")
                    .or_else(|| text.strip_prefix("cookie:"))
                    .unwrap_or(text)
                    .trim();
                inner.rp.user_auth_set_cookie(header).await
            };
            if let Err(e) = set {
                let msg = format!("YouTube did not accept the cookies: {e}");
                *inner.account() = Account::Failed(msg);
                return Err(match map_err(e) {
                    StreamError::Parse(_) => StreamError::NotLoggedIn,
                    other => other,
                });
            }
            let name = match innertube::account_name(&inner.rp).await {
                Ok(n) => n,
                Err(e) => {
                    log::warn!("youtube: logged in, but no account name: {e}");
                    None
                }
            };
            let notes = AccountNotes { name: name.clone() };
            if let Err(e) = serde_json::to_vec(&notes)
                .map_err(std::io::Error::other)
                .and_then(|b| std::fs::write(inner.notes_path(), b))
            {
                log::warn!("youtube: cannot save account notes: {e}");
            }
            *inner.account() = Account::LoggedIn {
                name: name.unwrap_or_else(|| DEFAULT_NAME.into()),
                premium: None,
                audio: true,
            };
            // The age gate may open now.
            inner.refused.lock().unwrap_or_else(PoisonError::into_inner).clear();
            Ok(())
        })
    }

    fn logout(&self) {
        *self.inner.account() = Account::LoggedOut;
        let _ = std::fs::remove_file(self.inner.notes_path());
        let rp = self.inner.rp.clone();
        // Only touches the cache file, no network.
        self.runtime.handle().spawn(async move {
            if let Err(e) = rp.user_auth_remove_cookie().await {
                log::debug!("youtube: logout: {e}");
            }
        });
    }

    fn search(&self, query: &str) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        self.search_in_parts(query, Arc::new(|_| {}))
    }

    /// Songs, videos, albums, artists and playlists are five requests sent
    /// at once; the songs go to `part` as soon as they are in (the section
    /// shows while the rest still loads).
    fn search_in_parts(
        &self,
        query: &str,
        part: SearchParts,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.trim().to_owned();
        self.run(move |inner| async move {
            if q.is_empty() {
                return Ok(StreamSearch::default());
            }
            inner.gate.check()?;
            let me = inner.me();
            let rq = inner.rp.query();
            let started = std::time::Instant::now();
            let songs = async {
                let r = timed("songs", started, rq.music_search_tracks(&q)).await;
                if let Ok(r) = &r {
                    part(StreamSearch {
                        tracks: r.items.items.iter().map(convert::track).collect(),
                        ..StreamSearch::default()
                    });
                }
                r
            };
            let (songs, videos, albums, artists, playlists) = tokio::join!(
                songs,
                timed("videos", started, rq.music_search_videos(&q)),
                timed("albums", started, rq.music_search_albums(&q)),
                timed("artists", started, rq.music_search_artists(&q)),
                timed("playlists", started, rq.music_search_playlists(&q, true)),
            );
            let mut first_err = None;
            let songs = part_of(songs, &mut first_err);
            let videos = part_of(videos, &mut first_err);
            let albums = part_of(albums, &mut first_err);
            let artists = part_of(artists, &mut first_err);
            let playlists = part_of(playlists, &mut first_err);
            if let Some(e) = &first_err {
                inner.gate.note(&Err::<(), _>(e.clone()));
            }
            if songs.is_none() && videos.is_none() && albums.is_none() && artists.is_none() {
                return Err(first_err.unwrap_or(StreamError::Unavailable));
            }
            let mut out = StreamSearch::default();
            for r in [songs, videos].into_iter().flatten() {
                out.tracks.extend(r.items.items.iter().map(convert::track));
            }
            if let Some(r) = albums {
                out.albums = r.items.items.iter().map(convert::album_item).collect();
            }
            if let Some(r) = artists {
                out.artists = r.items.items.iter().map(convert::artist_item).collect();
            }
            if let Some(r) = playlists {
                out.playlists = r
                    .items
                    .items
                    .iter()
                    .map(|p| convert::playlist_item(p, me.as_deref()))
                    .collect();
            }
            log::debug!(
                "youtube: search done in {} ms: {} tracks, {} albums, {} artists, {} playlists",
                started.elapsed().as_millis(),
                out.tracks.len(),
                out.albums.len(),
                out.artists.len(),
                out.playlists.len()
            );
            Ok(out)
        })
    }

    /// The «Всё» page: one mixed request (`music_search_main`): songs,
    /// clips, albums, artists and playlists as YouTube Music ranks them
    /// together. The five category requests stay for its own chip.
    fn search_top(
        &self,
        query: &str,
        _part: SearchParts,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.trim().to_owned();
        self.run(move |inner| async move {
            if q.is_empty() {
                return Ok(StreamSearch::default());
            }
            inner.gate.check()?;
            let me = inner.me();
            let started = std::time::Instant::now();
            let r = timed("main", started, inner.rp.query().music_search_main(&q)).await;
            let r = match r {
                Ok(r) => r,
                Err(e) => {
                    let e = map_err(e);
                    inner.gate.note(&Err::<(), _>(e.clone()));
                    return Err(e);
                }
            };
            let mut out = StreamSearch::default();
            for item in &r.items.items {
                match item {
                    rustypipe::model::MusicItem::Track(t) => out.tracks.push(convert::track(t)),
                    rustypipe::model::MusicItem::Album(a) => out.albums.push(convert::album_item(a)),
                    rustypipe::model::MusicItem::Artist(a) => out.artists.push(convert::artist_item(a)),
                    rustypipe::model::MusicItem::Playlist(p) => out.playlists.push(convert::playlist_item(p, me.as_deref())),
                    _ => {}
                }
            }
            log::debug!(
                "youtube: mixed search done in {} ms: {} tracks, {} albums, {} artists, {} playlists",
                started.elapsed().as_millis(),
                out.tracks.len(),
                out.albums.len(),
                out.artists.len(),
                out.playlists.len()
            );
            Ok(out)
        })
    }

    /// One category: one request, in the spaced catalogue lane.
    fn search_kind(
        &self,
        query: &str,
        kind: SearchKind,
    ) -> BoxFuture<'_, Result<StreamSearch, StreamError>> {
        let q = query.trim().to_owned();
        self.run(move |inner| async move {
            if q.is_empty() {
                return Ok(StreamSearch::default());
            }
            let i = &inner;
            i.gate
                .run(Lane::Catalog, async {
                    let rq = i.rp.query();
                    let mut out = StreamSearch::default();
                    match kind {
                        SearchKind::Songs => {
                            let r = rq.music_search_tracks(&q).await.map_err(map_err)?;
                            out.tracks = r.items.items.iter().map(convert::track).collect();
                        }
                        SearchKind::Videos => {
                            let r = rq.music_search_videos(&q).await.map_err(map_err)?;
                            out.tracks = r.items.items.iter().map(convert::track).collect();
                        }
                        SearchKind::Albums => {
                            let r = rq.music_search_albums(&q).await.map_err(map_err)?;
                            out.albums = r.items.items.iter().map(convert::album_item).collect();
                        }
                        SearchKind::Artists => {
                            let r = rq.music_search_artists(&q).await.map_err(map_err)?;
                            out.artists = r.items.items.iter().map(convert::artist_item).collect();
                        }
                    }
                    Ok(out)
                })
                .await
        })
    }

    fn track(&self, id: &str) -> BoxFuture<'_, Result<StreamTrack, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move {
            let i = &inner;
            i.gate
                .run(Lane::Catalog, async {
                    let d = i.rp.query().music_details(&id).await.map_err(map_err)?;
                    Ok(convert::track(&d.track))
                })
                .await
        })
    }

    /// Each video's details, one by one. A video YouTube Music has no song
    /// details of (a plain upload, one it does not list: `music_details`
    /// fails for it) comes from its own page, its title and channel without
    /// a length; one that is gone, or whose answer cannot be read, is
    /// `None`: it fails alone, not the whole batch. Only a pause (rate
    /// limit), the login or the network fail them all.
    fn tracks(&self, ids: &[String]) -> BoxFuture<'_, Result<Vec<Option<StreamTrack>>, StreamError>> {
        let ids = ids.to_vec();
        self.run(move |inner| async move {
            let mut out = Vec::with_capacity(ids.len());
            for id in &ids {
                match details_or_page(&inner, id).await {
                    Ok(t) => out.push(Some(t)),
                    Err(StreamError::Unavailable | StreamError::Parse(_)) => out.push(None),
                    Err(e) => return Err(e),
                }
            }
            Ok(out)
        })
    }

    fn album(&self, id: &str) -> BoxFuture<'_, Result<StreamAlbum, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move {
            let i = &inner;
            i.gate
                .run(Lane::Catalog, async {
                    let a = i.rp.query().music_album(&id).await.map_err(map_err)?;
                    Ok(convert::album(&a))
                })
                .await
        })
    }

    fn artist(&self, id: &str) -> BoxFuture<'_, Result<StreamArtist, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move {
            let i = &inner;
            i.gate
                .run(Lane::Catalog, async {
                    let a = i.rp.query().music_artist(&id, true).await.map_err(map_err)?;
                    // The same page serves discovery.
                    i.pages.put(&id, convert::artist_page(&a));
                    Ok(convert::artist(&a))
                })
                .await
        })
    }

    fn playlist(&self, id: &str) -> BoxFuture<'_, Result<StreamPlaylist, StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move {
            let i = &inner;
            let logged_in = i.account().is_logged_in();
            let lane = if logged_in { Lane::SignedIn } else { Lane::Catalog };
            i.gate
                .run(lane, async {
                    let mut q = i.rp.query();
                    if logged_in {
                        q = q.authenticated();
                    }
                    let mut p = q.music_playlist(&id).await.map_err(|e| i.user_err(e))?;
                    p.tracks
                        .extend_limit(&q, PLAYLIST_LIMIT)
                        .await
                        .map_err(map_err)?;
                    Ok(convert::playlist(&p, i.me().as_deref()))
                })
                .await
        })
    }

    /// YouTube has no playlist versions (the runtime goes by the track
    /// count of the listing instead): no request.
    fn playlist_version(&self, _id: &str) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        Box::pin(std::future::ready(Ok(None)))
    }

    fn paused_until(&self) -> Option<std::time::SystemTime> {
        self.inner.gate.paused_until()
    }

    /// Drops the discovery answers kept for a track or an artist.
    fn forget(&self, kind: EntityKind, id: &str) {
        match kind {
            EntityKind::Track => {
                self.inner.lists.remove(&format!("related:{id}"));
                self.inner.lists.remove(&format!("radio:{id}"));
            }
            EntityKind::Artist => {
                self.inner.pages.remove(id);
                self.inner.lists.remove(&format!("artist_radio:{id}"));
            }
            EntityKind::Album | EntityKind::Playlist => {}
        }
    }

    /// YouTube Music finds a quoted ISRC in its song search.
    fn find_isrc(&self, isrc: &str) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        let isrc = isrc.trim().replace('-', "").to_ascii_uppercase();
        self.run(move |inner| async move {
            if !valid_isrc(&isrc) {
                return Ok(None);
            }
            let i = &inner;
            let r = i
                .gate
                .run(Lane::Catalog, async {
                    i.rp.query()
                        .music_search_tracks(format!("\"{isrc}\""))
                        .await
                        .map_err(map_err)
                })
                .await?;
            Ok(r.items
                .items
                .iter()
                .find(|t| t.track_type == TrackType::Track)
                .map(|t| {
                    let mut t = convert::track(t);
                    t.isrc = Some(isrc.clone());
                    t
                }))
        })
    }

    /// The counterpart YouTube Music links the video to (one `next`
    /// request, signed in: the switch of its own clients), else a song
    /// search by the cleaned title, one per half of a dual title, at most
    /// two (see `counterpart`). Logged out, the `next` request is left
    /// out: without the session it never has the counterpart. Every request
    /// waits for its turn in the spaced lanes and obeys the pause.
    fn song_for_video(&self, video: &StreamTrack) -> BoxFuture<'_, Result<Option<StreamTrack>, StreamError>> {
        let video = video.clone();
        self.run(move |inner| async move {
            let i = &inner;
            let id = video.id.id.clone();
            let mut clip = video;
            if i.account().is_logged_in() {
                let w = i
                    .gate
                    .run(Lane::SignedIn, innertube::watch(&i.rp, &id, true))
                    .await;
                match w {
                    Ok(Some(w)) => {
                        if w.item.kind == nikgob_core::streaming::TrackKind::Song {
                            return Ok(Some(w.item));
                        }
                        if let Some(song) = w.counterpart {
                            log::debug!("youtube: {id}: the song is {}", song.id.id);
                            return Ok(Some(song));
                        }
                        // Its own title and length, as YouTube says them now.
                        if clip.duration_ms.is_none() {
                            clip.duration_ms = w.item.duration_ms;
                        }
                        if clip.artists.is_empty() {
                            clip.artists = w.item.artists;
                        }
                    }
                    Ok(None) => log::debug!("youtube: {id}: not in its own queue"),
                    Err(e @ StreamError::RateLimited { .. }) => return Err(e),
                    Err(e) => log::info!("youtube: {id}: next: {e}"),
                }
            }
            for q in counterpart::queries(&clip) {
                let r = i
                    .gate
                    .run(Lane::Catalog, async {
                        i.rp.query().music_search_tracks(&q).await.map_err(map_err)
                    })
                    .await?;
                let hits: Vec<StreamTrack> = r.items.items.iter().map(convert::track).collect();
                if let Some(song) = counterpart::pick_song(&hits, &clip) {
                    log::debug!("youtube: {id}: found the song {} by its title", song.id.id);
                    return Ok(Some(song));
                }
                if hits.iter().any(|h| h.id.id == id && h.kind == nikgob_core::streaming::TrackKind::Song) {
                    return Ok(hits.into_iter().find(|h| h.id.id == id));
                }
            }
            Ok(None)
        })
    }

    fn liked(&self) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            i.gate
                .run(Lane::SignedIn, async {
                    let q = i.rp.query().authenticated();
                    let mut p = q.music_liked_tracks().await.map_err(|e| i.user_err(e))?;
                    p.tracks
                        .extend_limit(&q, PLAYLIST_LIMIT)
                        .await
                        .map_err(|e| i.user_err(e))?;
                    Ok(p.tracks.items.iter().map(convert::track).collect())
                })
                .await
        })
    }

    /// The first page of «Понравившиеся» and its track count: one request.
    fn liked_head(&self) -> BoxFuture<'_, Result<LikedHead, StreamError>> {
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            i.gate
                .run(Lane::SignedIn, async {
                    let q = i.rp.query().authenticated();
                    let p = q.music_liked_tracks().await.map_err(|e| i.user_err(e))?;
                    Ok(LikedHead {
                        tracks: p.tracks.items.iter().map(convert::track).collect(),
                        total: p
                            .track_count
                            .or(p.tracks.count)
                            .and_then(|n| u32::try_from(n).ok()),
                        complete: p.tracks.is_exhausted(),
                    })
                })
                .await
        })
    }

    fn set_liked(&self, id: &str, liked: bool) -> BoxFuture<'_, Result<(), StreamError>> {
        let id = id.to_owned();
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            i.gate
                .run(Lane::SignedIn, innertube::set_liked(&i.rp, &id, liked))
                .await
        })
    }

    fn my_playlists(&self) -> BoxFuture<'_, Result<Vec<StreamPlaylist>, StreamError>> {
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            let p = i
                .gate
                .run(Lane::SignedIn, async {
                    let q = i.rp.query().authenticated();
                    let mut p = q.music_saved_playlists().await.map_err(|e| i.user_err(e))?;
                    p.extend_limit(&q, 1000).await.map_err(|e| i.user_err(e))?;
                    Ok(p)
                })
                .await?;
            let me = inner.me();
            Ok(p.items
                .iter()
                // Liked music and "episodes for later" are not playlists here.
                .filter(|p| p.id != "LM" && p.id != "SE")
                .map(|p| convert::playlist_item(p, me.as_deref()))
                .collect())
        })
    }

    fn create_playlist(&self, name: &str) -> BoxFuture<'_, Result<String, StreamError>> {
        let name = name.to_owned();
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            i.gate
                .run(Lane::SignedIn, innertube::create_playlist(&i.rp, &name))
                .await
        })
    }

    /// YouTube has no playlist version, so the result is always `None`.
    fn playlist_edit(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> BoxFuture<'_, Result<Option<String>, StreamError>> {
        let (id, add, remove) = (id.to_owned(), add.to_vec(), remove.to_vec());
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            i.gate
                .run(Lane::SignedIn, innertube::edit_playlist(&i.rp, &id, &add, &remove))
                .await?;
            Ok(None)
        })
    }

    fn recent(&self) -> BoxFuture<'_, Result<Vec<Played>, StreamError>> {
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            let h = i
                .gate
                .run(Lane::SignedIn, async {
                    i.rp.query().music_history().await.map_err(|e| i.user_err(e))
                })
                .await?;
            let mut out = Vec::new();
            let (mut day, mut index) = (None, 0);
            let mut last_at = None;
            for item in &h.items {
                let date = item.playback_date.or(day);
                index = if date == day { index + 1 } else { 0 };
                day = date;
                let at = played_at(date, index).or(last_at).unwrap_or(0);
                last_at = Some(at);
                out.push(Played {
                    track: convert::track(&item.item),
                    at_ms: at,
                });
            }
            Ok(out)
        })
    }

    fn open_audio(&self, id: &str) -> Result<Box<dyn AudioStream>, StreamError> {
        if id.is_empty() {
            return Err(StreamError::Unavailable);
        }
        if let Some(cached) = self.inner.cache.open(id) {
            return Ok(Box::new(cached));
        }
        Ok(Box::new(stream(
            &self.inner,
            self.runtime.handle().clone(),
            id,
            true,
        )))
    }

    fn save_audio(&self, id: &str, dir: &Path) -> BoxFuture<'_, Result<PathBuf, StreamError>> {
        let (id, dir) = (id.to_owned(), dir.to_owned());
        let handle = self.runtime.handle().clone();
        self.run(move |inner| async move {
            let tags = Self::tags(&inner, &id).await;
            // A recently played track is on disk already.
            let mut s: Box<dyn AudioStream> = match inner.cache.open(&id) {
                Some(c) => {
                    log::debug!("youtube: {id}: saved from the cache ({})", c.path().display());
                    Box::new(c)
                }
                None => Box::new(stream(&inner, handle, &id, true)),
            };
            // The save stops with its caller: a stopped task (✕ on its row)
            // drops this guard, and the blocking part below sees the flag.
            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let _stop = StopOnDrop(Arc::clone(&cancel));
            let task = tokio::task::spawn_blocking(move || -> Result<PathBuf, StreamError> {
                let io = |e: std::io::Error| StreamError::Network(e.to_string());
                // A stream refused is not a lost connection (see `range`).
                let bytes = read_unless(&mut s, &cancel).map_err(|e| range::stream_error(&e))?;
                let ext = if s.format_hint() == format::WEBM {
                    "opus"
                } else {
                    "m4a"
                };
                std::fs::create_dir_all(&dir).map_err(io)?;
                let path = dir.join(format!("{id}.{ext}"));
                let part = dir.join(format!("{id}.{ext}.part"));
                let file = std::io::BufWriter::new(std::fs::File::create(&part).map_err(io)?);
                let written = if ext == "opus" {
                    let serial = id
                        .bytes()
                        .fold(0x6e69_6b67_u32, |h, b| h.rotate_left(5) ^ u32::from(b));
                    remux::webm_to_ogg(&bytes, &tags, serial, file).map_err(StreamError::Parse)
                } else {
                    let mut file = file;
                    std::io::Write::write_all(&mut file, &bytes)
                        .and_then(|()| std::io::Write::flush(&mut file))
                        .map_err(io)
                };
                // Stopped while it wrote: the part goes, nothing is kept.
                let stopped = || {
                    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        Err(StreamError::Network(CANCELLED.into()))
                    } else {
                        Ok(())
                    }
                };
                if let Err(e) = written
                    .and_then(|()| stopped())
                    .and_then(|()| std::fs::rename(&part, &path).map_err(io))
                {
                    let _ = std::fs::remove_file(&part);
                    return Err(e);
                }
                Ok(path)
            });
            task.await
                .unwrap_or_else(|e| Err(StreamError::Network(format!("task failed: {e}"))))
        })
    }

    fn cache_size(&self) -> u64 {
        self.inner.cache.size()
    }

    /// A complete file of the audio cache (the bytes as YouTube sent them:
    /// [`format::WEBM`] Opus, or [`format::M4A`] AAC); a partial or missing
    /// one is `None`.
    fn cached_audio(&self, id: &str) -> Option<Box<dyn AudioStream>> {
        self.inner
            .cache
            .open(id)
            .map(|c| Box::new(c) as Box<dyn AudioStream>)
    }

    fn has_cached_audio(&self, id: &str) -> bool {
        self.inner.cache.has(id)
    }

    /// The complete cached file (`{id}.{itag}.webm` / `.m4a`) itself.
    fn cached_file(&self, id: &str) -> Option<std::path::PathBuf> {
        self.inner.cache.open(id).map(|c| c.path().to_owned())
    }

    /// The «Вам может понравиться» shelf of the track: two requests.
    fn related(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = track_id.to_owned();
        self.run(move |inner| async move { inner.related(&id).await })
    }

    /// The track's radio (`RDAMVM{id}`): one request, about 50 tracks
    /// after the track, songs and clips as YouTube Music queues them.
    fn radio(&self, track_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = track_id.to_owned();
        self.run(move |inner| async move {
            inner
                .radio(&format!("radio:{id}"), &format!("RDAMVM{id}"), Some(&id))
                .await
        })
    }

    /// From the artist page: one request, shared with [`Self::artist_top`]
    /// and [`Self::artist_radio`] (and with [`Self::artist`] opened lately).
    fn similar_artists(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamArtist>, StreamError>> {
        let id = artist_id.to_owned();
        self.run(move |inner| async move { Ok(inner.artist_page(&id).await?.similar) })
    }

    /// The top songs the artist page shows (about five; its videos are
    /// left out): no request of its own.
    fn artist_top(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = artist_id.to_owned();
        self.run(move |inner| async move { Ok(inner.artist_page(&id).await?.top) })
    }

    /// The artist's radio from the id its page gives: the page (if not
    /// kept) and the radio. An artist without one is
    /// [`StreamError::Unavailable`].
    fn artist_radio(&self, artist_id: &str) -> BoxFuture<'_, Result<Vec<StreamTrack>, StreamError>> {
        let id = artist_id.to_owned();
        self.run(move |inner| async move {
            let key = format!("artist_radio:{id}");
            if let Some(hit) = inner.lists.get(&key) {
                return Ok(hit);
            }
            let radio_id = inner.artist_page(&id).await?.radio_id.ok_or(StreamError::Unavailable)?;
            inner.radio(&key, &radio_id, None).await
        })
    }

    fn set_cache_limit(&self, bytes: u64) {
        self.inner.cache.set_limit(bytes);
    }

    fn clear_cache(&self) {
        self.inner.cache.clear();
    }

    fn suggestion_feeds(&self) -> &'static [SuggestionFeed] {
        &[SuggestionFeed::Home, SuggestionFeed::History]
    }

    /// The home (`FEmusic_home`: quick picks, listen again) or the history
    /// (`FEmusic_history`), one request with the cookies.
    fn suggestions(&self, feed: SuggestionFeed) -> BoxFuture<'_, Result<Suggestions, StreamError>> {
        self.run(move |inner| async move {
            inner.need_login()?;
            let i = &inner;
            match feed {
                SuggestionFeed::Home => i.gate.run(Lane::SignedIn, suggest::home(&i.rp)).await,
                SuggestionFeed::History => i.gate.run(Lane::SignedIn, suggest::history(&i.rp)).await,
                _ => Err(StreamError::Unsupported),
            }
        })
    }

    fn web_url(&self, kind: EntityKind, id: &str) -> String {
        match kind {
            EntityKind::Track => format!("https://music.youtube.com/watch?v={id}"),
            EntityKind::Album => format!("https://music.youtube.com/browse/{id}"),
            EntityKind::Artist => format!("https://music.youtube.com/channel/{id}"),
            EntityKind::Playlist => format!("https://music.youtube.com/playlist?list={id}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A save stopped by its caller stops reading at the next chunk; one
    /// let be reads to the end.
    #[test]
    fn a_stopped_save_stops_reading() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Slow<'a> {
            left: usize,
            reads: usize,
            stop_after: usize,
            cancel: &'a AtomicBool,
        }
        impl Read for Slow<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.left == 0 {
                    return Ok(0);
                }
                self.reads += 1;
                if self.reads == self.stop_after {
                    self.cancel.store(true, Ordering::Relaxed);
                }
                let n = buf.len().min(self.left).min(1000);
                self.left -= n;
                Ok(n)
            }
        }
        let cancel = AtomicBool::new(false);
        let mut r = Slow { left: 10_000, reads: 0, stop_after: 3, cancel: &cancel };
        let e = read_unless(&mut r, &cancel).unwrap_err();
        assert_eq!(e.to_string(), CANCELLED);
        assert_eq!(r.reads, 3, "no read after the flag");

        let free = AtomicBool::new(false);
        let mut r = Slow { left: 10_000, reads: 0, stop_after: usize::MAX, cancel: &free };
        assert_eq!(read_unless(&mut r, &free).unwrap().len(), 10_000);

        // The guard raises the flag when its task goes.
        let flag = Arc::new(AtomicBool::new(false));
        drop(StopOnDrop(Arc::clone(&flag)));
        assert!(flag.load(Ordering::Relaxed));
    }

    #[test]
    fn errors_map_to_stream_errors() {
        let unavailable = RpError::Extraction(ExtractionError::Unavailable {
            reason: UnavailabilityReason::Geoblocked,
            msg: "no".into(),
        });
        assert_eq!(map_err(unavailable), StreamError::RegionBlocked);
        let ban = RpError::Extraction(ExtractionError::Unavailable {
            reason: UnavailabilityReason::Captcha,
            msg: "no".into(),
        });
        assert_eq!(map_err(ban), StreamError::RateLimited { retry_after: None });
        assert_eq!(
            map_err(RpError::HttpStatus(500, "x".into())),
            StreamError::Http { status: 500 }
        );
        assert_eq!(
            map_err(RpError::Auth(AuthError::NoLogin)),
            StreamError::NotLoggedIn
        );
    }

    #[test]
    fn history_days_keep_their_order() {
        let d = time::Date::from_calendar_date(2026, time::Month::October, 16).unwrap();
        let a = played_at(Some(d), 0).unwrap();
        // One second before 2026-10-17 00:00 UTC.
        assert_eq!(a, 1_792_195_200_000 - 1000);
        let b = played_at(Some(d), 1).unwrap();
        assert!(a > b);
        assert_eq!(a % 1000, 0);
        assert_eq!(played_at(None, 0), None);
    }

    #[test]
    fn stream_urls_say_when_they_expire() {
        let at = url_expiry("https://rr1.googlevideo.com/videoplayback?expire=1792195200&ei=x&itag=251")
            .unwrap();
        assert_eq!(at, std::time::UNIX_EPOCH + Duration::from_secs(1_792_195_200));
        assert_eq!(url_expiry("https://x/y?itag=251"), None);
    }

    #[test]
    fn isrc_check_and_country() {
        assert!(valid_isrc("DEUM71602459"));
        assert!(!valid_isrc("DEUM7160245"));
        assert!(!valid_isrc("\"DEUM716024\""));
        assert_eq!(parse_country("ru"), Some(Country::Ru));
    }

    #[test]
    fn the_pref_cookie_asks_for_our_language() {
        assert_eq!(
            prefer_language("SID=a; PREF=f6=40000000&hl=en&gl=US; HSID=b", "ru", "RU"),
            "SID=a; PREF=f6=40000000&hl=ru&gl=RU; HSID=b"
        );
        assert_eq!(prefer_language("SID=a", "ru", "RU"), "SID=a", "no PREF, nothing to change");
        let txt = ".youtube.com\tTRUE\t/\tTRUE\t0\tPREF\thl=en\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tx";
        assert_eq!(
            prefer_language(txt, "ru", "RU"),
            ".youtube.com\tTRUE\t/\tTRUE\t0\tPREF\thl=ru&gl=RU\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tx"
        );
        assert_eq!(parse_country("nope"), None);
    }

    #[test]
    fn cached_audio_is_only_a_complete_file_and_never_the_network() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let yt = Youtube::new(YoutubeConfig::new(dir.join("data")))
            .unwrap()
            .with_audio_cache(dir.join("cache"));
        assert!(yt.cached_audio("abc").is_none(), "not played: no download either");
        let r = range::Resolved {
            url: String::new(),
            len: 4,
            format: format::WEBM,
            duration: Some(Duration::from_secs(1)),
            gain_db: None,
            itag: 251,
            bitrate: 160_000,
            route: None,
        };
        assert!(!yt.has_cached_audio("abc"));
        yt.inner.cache.store("abc", &r, &mut &[1, 2, 3][..]);
        assert!(yt.cached_audio("abc").is_none(), "a partial download is not served");
        assert!(!yt.has_cached_audio("abc"));
        yt.inner.cache.store("abc", &r, &mut &[1, 2, 3, 4][..]);
        // The cheap check opens nothing: the file keeps its place in the
        // eviction order.
        let file = dir.join("cache").join("abc.251.webm");
        let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        std::fs::File::options().write(true).open(&file).unwrap().set_modified(old).unwrap();
        assert!(yt.has_cached_audio("abc"));
        assert_eq!(std::fs::metadata(&file).unwrap().modified().unwrap(), old);
        let mut a = yt.cached_audio("abc").expect("complete");
        assert_eq!((a.format_hint(), a.byte_len()), (format::WEBM, Some(4)));
        let mut bytes = Vec::new();
        a.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, [1, 2, 3, 4]);
        drop(a);
        drop(yt);
    }

    #[test]
    fn web_urls_and_offline_behaviour() {
        let dir = std::env::temp_dir().join(format!("nikgob-yt-unit-{}", std::process::id()));
        let yt = Youtube::new(YoutubeConfig::new(&dir)).unwrap();
        assert_eq!(yt.account(), Account::LoggedOut);
        assert_eq!(
            yt.web_url(EntityKind::Track, "xtxjm7ciwmc"),
            "https://music.youtube.com/watch?v=xtxjm7ciwmc"
        );
        assert_eq!(
            yt.web_url(EntityKind::Album, "MPREb_OLmD8O5IYNS"),
            "https://music.youtube.com/browse/MPREb_OLmD8O5IYNS"
        );
        assert_eq!(
            yt.web_url(EntityKind::Artist, "UC1"),
            "https://music.youtube.com/channel/UC1"
        );
        assert_eq!(
            yt.web_url(EntityKind::Playlist, "PL1"),
            "https://music.youtube.com/playlist?list=PL1"
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            assert_eq!(yt.liked().await, Err(StreamError::NotLoggedIn));
            assert_eq!(yt.recent().await, Err(StreamError::NotLoggedIn));
            assert_eq!(yt.set_liked("x", true).await, Err(StreamError::NotLoggedIn));
            assert_eq!(
                yt.login(LoginInput::Browser).await,
                Err(StreamError::Unsupported)
            );
            assert_eq!(
                yt.login(LoginInput::Playback).await,
                Err(StreamError::Unsupported)
            );
            assert_eq!(yt.find_isrc("nope").await, Ok(None));
        });
        // Opening does no network: an id that does not exist still opens.
        assert!(yt.open_audio("does-not-exist").is_ok());
        drop(yt);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
