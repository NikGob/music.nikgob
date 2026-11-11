//! Shared HTTP plumbing: one `reqwest::Client` with the project `User-Agent`,
//! per-service rate limiting and retries with backoff on 429/503.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, RETRY_AFTER};
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use tokio::time::{Instant, sleep, sleep_until};
use url::Url;

use crate::config::MetadataConfig;
use crate::error::{MetadataError, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Which of two requests waiting for one service goes first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Lane {
    /// What someone waits for (the playing song's lyrics): the next free
    /// slot, in call order.
    #[default]
    Now,
    /// A background pass: a slot only while no request of the `Now` lane
    /// wants it.
    Background,
}

tokio::task_local! {
    static LANE: Lane;
}

/// Runs `fut` in the background lane: each of its requests gives way to
/// any other request to the same service, which takes the next free slot
/// ahead of it. The rate stays the service's.
pub async fn in_background<F: std::future::Future>(fut: F) -> F::Output {
    LANE.scope(Lane::Background, fut).await
}

/// The lane of the task asking ([`in_background`]).
fn current_lane() -> Lane {
    LANE.try_with(|l| *l).unwrap_or_default()
}

/// Spaces requests at least `interval` apart. Clones share one schedule, so the
/// limit holds across every task that uses the same service.
#[derive(Clone, Debug)]
pub struct RateLimiter {
    inner: Arc<LimiterInner>,
}

#[derive(Debug)]
struct LimiterInner {
    interval: Duration,
    next: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            inner: Arc::new(LimiterInner {
                interval: min_interval,
                next: Mutex::new(None),
            }),
        }
    }

    pub fn interval(&self) -> Duration {
        self.inner.interval
    }

    /// Waits for this caller's slot. Slots are handed out in call order,
    /// a request of a background pass ([`in_background`]) after all others.
    pub async fn acquire(&self) {
        self.acquire_in(current_lane()).await;
    }

    /// [`Self::acquire`] in `lane`. A background request holds no slot while
    /// it waits: it takes the free one when its time comes, so a request of
    /// the `Now` lane asking meanwhile gets that slot and it waits again.
    pub(crate) async fn acquire_in(&self, lane: Lane) {
        if lane == Lane::Background {
            loop {
                let wait = {
                    let mut next = self.inner.next.lock().unwrap_or_else(|e| e.into_inner());
                    let now = Instant::now();
                    match *next {
                        Some(n) if n > now => n,
                        _ => {
                            *next = Some(now + self.inner.interval);
                            return;
                        }
                    }
                };
                sleep_until(wait).await;
            }
        }
        let slot = {
            let mut next = self.inner.next.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let slot = next.map_or(now, |n| n.max(now));
            *next = Some(slot + self.inner.interval);
            slot
        };
        if slot > Instant::now() {
            sleep_until(slot).await;
        }
    }

    /// Pushes the schedule back (the server asked us to slow down).
    pub fn penalize(&self, delay: Duration) {
        let mut next = self.inner.next.lock().unwrap_or_else(|e| e.into_inner());
        let until = Instant::now() + delay;
        *next = Some(next.map_or(until, |n| n.max(until)));
    }
}

/// Minimum spacing between requests, per service.
#[derive(Clone, Copy, Debug)]
pub struct RateLimits {
    pub musicbrainz: Duration,
    pub acoustid: Duration,
    pub cover_art: Duration,
    pub deezer: Duration,
    pub lrclib: Duration,
    pub lyricsplus: Duration,
    pub lrcred: Duration,
    pub betterlyrics: Duration,
    /// Odesli (song.link): 10 requests a minute without an API key.
    pub odesli: Duration,
    /// The iTunes Search API: about 20 requests a minute.
    pub itunes: Duration,
    /// ListenBrainz (its labs too): about one a second is polite.
    pub listenbrainz: Duration,
    /// Last.fm: five a second averaged, one a second here.
    pub lastfm: Duration,
    /// Genius's public pages: a request a second.
    pub genius: Duration,
    /// Wikidata and Wikimedia Commons (an artist's item and picture).
    pub wikimedia: Duration,
    /// Pictures on the services' CDNs (an artist's image).
    pub web_images: Duration,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            // MusicBrainz: 1 request/second per IP.
            musicbrainz: Duration::from_millis(1000),
            // AcoustID: 3 requests/second.
            acoustid: Duration::from_millis(340),
            cover_art: Duration::from_millis(200),
            // Deezer: 50 requests / 5 s.
            deezer: Duration::from_millis(200),
            lrclib: Duration::from_millis(200),
            lyricsplus: Duration::from_millis(500),
            lrcred: Duration::from_millis(500),
            betterlyrics: Duration::from_millis(500),
            odesli: Duration::from_secs(6),
            itunes: Duration::from_secs(3),
            listenbrainz: Duration::from_millis(1000),
            lastfm: Duration::from_millis(1000),
            genius: Duration::from_millis(1000),
            wikimedia: Duration::from_millis(500),
            web_images: Duration::from_millis(200),
        }
    }
}

impl RateLimits {
    pub const fn unlimited() -> Self {
        Self {
            musicbrainz: Duration::ZERO,
            acoustid: Duration::ZERO,
            cover_art: Duration::ZERO,
            deezer: Duration::ZERO,
            lrclib: Duration::ZERO,
            lyricsplus: Duration::ZERO,
            lrcred: Duration::ZERO,
            betterlyrics: Duration::ZERO,
            odesli: Duration::ZERO,
            itunes: Duration::ZERO,
            listenbrainz: Duration::ZERO,
            lastfm: Duration::ZERO,
            genius: Duration::ZERO,
            wikimedia: Duration::ZERO,
            web_images: Duration::ZERO,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Total attempts including the first.
    pub max_attempts: u32,
    pub base_delay: Duration,
    /// Upper bound for both our backoff and a server-provided `Retry-After`.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    fn backoff(&self, attempt: u32) -> Duration {
        let factor = 1u32 << (attempt - 1).min(16);
        self.base_delay.saturating_mul(factor).min(self.max_delay)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Service {
    MusicBrainz,
    AcoustId,
    CoverArt,
    Deezer,
    Lrclib,
    LyricsPlus,
    LrcRed,
    BetterLyrics,
    Odesli,
    Itunes,
    ListenBrainz,
    LastFm,
    Genius,
    Wikimedia,
    WebImages,
}

#[derive(Debug)]
struct Limiters {
    musicbrainz: RateLimiter,
    acoustid: RateLimiter,
    cover_art: RateLimiter,
    deezer: RateLimiter,
    lrclib: RateLimiter,
    lyricsplus: RateLimiter,
    lrcred: RateLimiter,
    betterlyrics: RateLimiter,
    odesli: RateLimiter,
    itunes: RateLimiter,
    listenbrainz: RateLimiter,
    lastfm: RateLimiter,
    genius: RateLimiter,
    wikimedia: RateLimiter,
    web_images: RateLimiter,
}

impl Limiters {
    fn new(l: &RateLimits) -> Self {
        Self {
            musicbrainz: RateLimiter::new(l.musicbrainz),
            acoustid: RateLimiter::new(l.acoustid),
            cover_art: RateLimiter::new(l.cover_art),
            deezer: RateLimiter::new(l.deezer),
            lrclib: RateLimiter::new(l.lrclib),
            lyricsplus: RateLimiter::new(l.lyricsplus),
            lrcred: RateLimiter::new(l.lrcred),
            betterlyrics: RateLimiter::new(l.betterlyrics),
            odesli: RateLimiter::new(l.odesli),
            itunes: RateLimiter::new(l.itunes),
            listenbrainz: RateLimiter::new(l.listenbrainz),
            lastfm: RateLimiter::new(l.lastfm),
            genius: RateLimiter::new(l.genius),
            wikimedia: RateLimiter::new(l.wikimedia),
            web_images: RateLimiter::new(l.web_images),
        }
    }

    fn get(&self, s: Service) -> &RateLimiter {
        match s {
            Service::MusicBrainz => &self.musicbrainz,
            Service::AcoustId => &self.acoustid,
            Service::CoverArt => &self.cover_art,
            Service::Deezer => &self.deezer,
            Service::Lrclib => &self.lrclib,
            Service::LyricsPlus => &self.lyricsplus,
            Service::LrcRed => &self.lrcred,
            Service::BetterLyrics => &self.betterlyrics,
            Service::Odesli => &self.odesli,
            Service::Itunes => &self.itunes,
            Service::ListenBrainz => &self.listenbrainz,
            Service::LastFm => &self.lastfm,
            Service::Genius => &self.genius,
            Service::Wikimedia => &self.wikimedia,
            Service::WebImages => &self.web_images,
        }
    }
}

/// Cheap to clone; all clones share the connection pool and the rate limiters.
#[derive(Clone, Debug)]
pub struct Http {
    client: Client,
    retry: RetryPolicy,
    limiters: Arc<Limiters>,
    has_contact: bool,
}

impl Http {
    pub fn new(config: &MetadataConfig) -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(config.user_agent())
            .build()
            .map_err(|e| MetadataError::Network(e.without_url().to_string()))?;
        Ok(Self {
            client,
            retry: RetryPolicy::default(),
            limiters: Arc::new(Limiters::new(&RateLimits::default())),
            has_contact: config.has_contact(),
        })
    }

    #[must_use]
    pub fn with_rate_limits(mut self, limits: RateLimits) -> Self {
        self.limiters = Arc::new(Limiters::new(&limits));
        self
    }

    #[must_use]
    pub fn with_retry_policy(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub(crate) fn has_contact(&self) -> bool {
        self.has_contact
    }

    pub(crate) fn client(&self) -> &Client {
        &self.client
    }

    /// Waits for the service's rate-limit slot (a request sent another way).
    pub(crate) async fn wait_turn(&self, service: Service) {
        self.limiters.get(service).acquire().await;
    }

    /// Sends the request, waiting for the service's rate-limit slot before every
    /// attempt and retrying on 429/503 and connection failures.
    pub(crate) async fn send(&self, service: Service, req: RequestBuilder) -> Result<Response> {
        let limiter = self.limiters.get(service);
        let max = self.retry.max_attempts.max(1);
        let mut attempt = 0;
        loop {
            attempt += 1;
            limiter.acquire().await;
            let this = req
                .try_clone()
                .ok_or_else(|| MetadataError::InvalidArgument("request is not cloneable".into()))?;
            match this.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    if status != StatusCode::TOO_MANY_REQUESTS
                        && status != StatusCode::SERVICE_UNAVAILABLE
                    {
                        return Ok(resp);
                    }
                    let hinted = resp
                        .headers()
                        .get(RETRY_AFTER)
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(Duration::from_secs)
                        // `Retry-After: 0` (MusicBrainz's 503) still waits a second.
                        .map(|d| d.max(Duration::from_secs(1)).min(self.retry.max_delay));
                    let delay = hinted.unwrap_or_else(|| self.retry.backoff(attempt));
                    // Shared schedule: other tasks hitting the same service back off
                    // too, also when this caller gives up (the server is still angry).
                    limiter.penalize(delay);
                    if attempt >= max {
                        if status.is_server_error() && matches!(service, Service::BetterLyrics) {
                            return Ok(resp);
                        }
                        return Err(MetadataError::RateLimited(hinted));
                    }
                    log::warn!("{service:?}: HTTP {status}, retry {attempt}/{max} in {delay:?}");
                }
                Err(e) if e.is_connect() && attempt < max => {
                    let delay = self.retry.backoff(attempt);
                    log::warn!("{service:?}: connect failed, retry {attempt}/{max} in {delay:?}");
                    sleep(delay).await;
                }
                Err(e) => return Err(MetadataError::Network(e.without_url().to_string())),
            }
        }
    }

    pub(crate) async fn get_json<T: DeserializeOwned>(
        &self,
        service: Service,
        url: Url,
    ) -> Result<T> {
        let resp = self.send(service, self.client.get(url)).await?;
        parse_json(&ok_body(resp).await?)
    }

    /// [`Self::get_json`] with an `Authorization` header when one is given.
    pub(crate) async fn get_json_auth<T: DeserializeOwned>(
        &self,
        service: Service,
        url: Url,
        auth: Option<&str>,
    ) -> Result<T> {
        let mut req = self.client.get(url);
        if let Some(a) = auth {
            req = req.header(AUTHORIZATION, a);
        }
        let resp = self.send(service, req).await?;
        parse_json(&ok_body(resp).await?)
    }

    pub(crate) async fn get_body(&self, service: Service, url: Url) -> Result<Vec<u8>> {
        let resp = self.send(service, self.client.get(url)).await?;
        ok_body(resp).await
    }

    pub(crate) async fn get_bytes(
        &self,
        service: Service,
        url: Url,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        let req = self.client.get(url).timeout(Duration::from_secs(90));
        let mut resp = self.send(service, req).await?;
        if resp
            .content_length()
            .is_some_and(|len| len > max_bytes as u64)
        {
            return Err(MetadataError::Parse("download is too large".into()));
        }
        if !resp.status().is_success() {
            // Maps 404 and the other statuses to their errors.
            return ok_body(resp).await;
        }
        // Without a Content-Length (chunked) the size is only known by reading:
        // stop at the limit instead of buffering whatever the server sends.
        let mut body = Vec::new();
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| MetadataError::Network(e.without_url().to_string()))?
        {
            if body.len() + chunk.len() > max_bytes {
                return Err(MetadataError::Parse("download is too large".into()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

pub(crate) fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    serde_json::from_slice(body).map_err(|e| MetadataError::Parse(e.to_string()))
}

/// Body of a successful response; 404 and other statuses become errors.
pub(crate) async fn ok_body(resp: Response) -> Result<Vec<u8>> {
    let status = resp.status();
    let body = resp
        .bytes()
        .await
        .map_err(|e| MetadataError::Network(e.without_url().to_string()))?;
    if status == StatusCode::NOT_FOUND {
        return Err(MetadataError::NotFound);
    }
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body);
        let snippet: String = text.chars().take(200).collect();
        return Err(MetadataError::Status {
            status: status.as_u16(),
            body: snippet,
        });
    }
    Ok(body.to_vec())
}

/// Parses a base URL and makes sure its path ends with `/` so `join` keeps prefixes.
pub(crate) fn base_url(raw: &str) -> Result<Url> {
    let mut url = Url::parse(raw.trim())
        .map_err(|e| MetadataError::InvalidArgument(format!("invalid base URL: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err(MetadataError::InvalidArgument(
            "base URL must be http(s) with a host".into(),
        ));
    }
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().ends_with('/') {
        let p = format!("{}/", url.path());
        url.set_path(&p);
    }
    Ok(url)
}

/// `base.join(path)` followed by query parameters.
pub(crate) fn build_url(base: &Url, path: &str, params: &[(&str, &str)]) -> Result<Url> {
    let mut url = base
        .join(path)
        .map_err(|e| MetadataError::InvalidArgument(format!("bad path: {e}")))?;
    if !params.is_empty() {
        url.query_pairs_mut().extend_pairs(params.iter().copied());
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn spaces_requests_by_the_interval() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        let start = Instant::now();
        limiter.acquire().await;
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "first request is immediate"
        );
        limiter.acquire().await;
        assert_eq!(start.elapsed(), Duration::from_secs(1));
        limiter.acquire().await;
        assert_eq!(start.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_time_does_not_build_up_a_burst() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        limiter.acquire().await;
        sleep(Duration::from_secs(10)).await;
        let t = Instant::now();
        limiter.acquire().await;
        limiter.acquire().await;
        assert_eq!(
            t.elapsed(),
            Duration::from_secs(1),
            "second one still waits"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn clones_and_concurrent_tasks_share_one_schedule() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        let start = Instant::now();
        let done = |l: RateLimiter| async move {
            l.acquire().await;
            start.elapsed()
        };
        let (a, b, c) = tokio::join!(
            done(limiter.clone()),
            done(limiter.clone()),
            done(limiter.clone())
        );
        let mut times = [a, b, c];
        times.sort();
        assert_eq!(
            times,
            [
                Duration::ZERO,
                Duration::from_secs(1),
                Duration::from_secs(2)
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn penalize_delays_everyone_sharing_the_limiter() {
        let limiter = RateLimiter::new(Duration::from_millis(100));
        let other = limiter.clone();
        limiter.penalize(Duration::from_secs(5));
        let start = Instant::now();
        other.acquire().await;
        assert_eq!(start.elapsed(), Duration::from_secs(5));
        // A shorter penalty never pulls the schedule forward.
        limiter.penalize(Duration::from_millis(10));
        let t = Instant::now();
        other.acquire().await;
        assert_eq!(t.elapsed(), Duration::from_millis(100));
    }

    /// The playing song's lookup takes the next slot ahead of a background
    /// pass's request already waiting for it; the rate stays one a second.
    #[tokio::test(start_paused = true)]
    async fn a_request_now_goes_before_a_background_one_waiting() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        let start = Instant::now();
        limiter.acquire().await;
        let l = limiter.clone();
        let background = tokio::spawn(in_background(async move {
            assert_eq!(current_lane(), Lane::Background);
            l.acquire().await;
            start.elapsed()
        }));
        sleep(Duration::from_millis(500)).await;
        assert_eq!(current_lane(), Lane::Now);
        limiter.acquire().await;
        assert_eq!(start.elapsed(), Duration::from_secs(1), "the slot at 1 s is the song's");
        assert_eq!(background.await.unwrap(), Duration::from_secs(2), "the pass waits for the next one");
        // Alone, a background pass keeps the usual pace.
        let t = Instant::now();
        in_background(async {
            limiter.acquire().await;
            limiter.acquire().await;
        })
        .await;
        assert_eq!(t.elapsed(), Duration::from_secs(2));
    }

    /// Several requests now go in call order before any background one; a
    /// penalty holds back both lanes.
    #[tokio::test(start_paused = true)]
    async fn background_requests_wait_for_every_request_now_and_for_penalties() {
        let limiter = RateLimiter::new(Duration::from_secs(1));
        let start = Instant::now();
        limiter.penalize(Duration::from_secs(3));
        let l = limiter.clone();
        let background = tokio::spawn(in_background(async move {
            l.acquire().await;
            start.elapsed()
        }));
        tokio::task::yield_now().await;
        let now = |l: RateLimiter| async move {
            l.acquire().await;
            start.elapsed()
        };
        let (a, b) = tokio::join!(now(limiter.clone()), now(limiter.clone()));
        let mut times = [a, b];
        times.sort();
        assert_eq!(times, [Duration::from_secs(3), Duration::from_secs(4)]);
        assert_eq!(background.await.unwrap(), Duration::from_secs(5));
    }

    #[test]
    fn http_clones_share_limiters_and_default_limits_match_the_services() {
        let http = Http::new(&MetadataConfig::new("x@example.com")).unwrap();
        let clone = http.clone();
        assert!(Arc::ptr_eq(&http.limiters, &clone.limiters));
        let l = RateLimits::default();
        assert_eq!(l.musicbrainz, Duration::from_secs(1));
        assert_eq!(
            http.limiters.get(Service::MusicBrainz).interval(),
            l.musicbrainz
        );
    }

    #[test]
    fn backoff_doubles_and_is_capped() {
        let p = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(3),
        };
        assert_eq!(p.backoff(1), Duration::from_millis(500));
        assert_eq!(p.backoff(2), Duration::from_secs(1));
        assert_eq!(p.backoff(3), Duration::from_secs(2));
        assert_eq!(p.backoff(4), Duration::from_secs(3));
        assert_eq!(p.backoff(30), Duration::from_secs(3));
    }

    #[test]
    fn user_agent_has_product_version_and_contact() {
        let ua = MetadataConfig::new(" me@example.com\n").user_agent();
        assert_eq!(
            ua,
            format!(
                "music.nikgob/{} ( me@example.com )",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert!(!MetadataConfig::default().user_agent().contains('('));
    }
}
