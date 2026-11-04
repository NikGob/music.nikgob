//! What the web sources share: a client that reads like a browser, a pace
//! between requests to one site, a pause after a site turned us away
//! (a bot check, a ban, a rate limit), and downloads written to a `.part`
//! file renamed when whole.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nikgob_core::source::{Progress, SourceError};
use reqwest::{Client, Response, StatusCode};
use tokio::io::AsyncWriteExt;

/// A desktop browser's User-Agent: khinsider's guard turns other agents away.
pub const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A page or an API answer.
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
/// No byte of a download for this long: it stalled.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Bigger downloads are refused (a track, or a hi-res album's track).
const MAX_FILE: u64 = 2 << 30;

pub fn client() -> Client {
    Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .user_agent(BROWSER_UA)
        .build()
        .unwrap_or_default()
}

/// At most one request per `every` to one site.
pub struct Pace {
    every: Duration,
    next: tokio::sync::Mutex<Instant>,
}

impl Pace {
    pub fn new(every: Duration) -> Self {
        Self {
            every,
            next: tokio::sync::Mutex::new(Instant::now()),
        }
    }

    /// Waits for this request's turn.
    pub async fn wait(&self) {
        let mut next = self.next.lock().await;
        let now = Instant::now();
        if *next > now {
            tokio::time::sleep(*next - now).await;
        }
        *next = Instant::now() + self.every;
    }
}

/// A site that turned us away sits out a while: 30 minutes, doubling up
/// to a day while it keeps doing so; one answer that works ends it. A bot
/// check or a rate limit starts the pause at once; a 401/403 (an instance
/// whose account the service banned: it finds, it does not give files)
/// or no answer at all (an instance whose name is gone) only once it did
/// so [`STRIKES`] times with no working answer between. A wait the site
/// asked for is waited out whole (up to a day).
pub struct Breaker {
    state: Mutex<BreakerState>,
}

#[derive(Default)]
struct BreakerState {
    until: Option<Instant>,
    /// Pauses in a row.
    pauses: u32,
    /// 401/403 answers (or none) in a row.
    strikes: u32,
    /// What started the pause.
    why: Option<SourceError>,
}

const FIRST_PAUSE: Duration = Duration::from_secs(30 * 60);
const LONGEST_PAUSE: Duration = Duration::from_secs(24 * 3600);
/// 401/403 answers (or none) in a row that set an instance aside.
pub const STRIKES: u32 = 2;

impl Default for Breaker {
    fn default() -> Self {
        Self { state: Mutex::new(BreakerState::default()) }
    }
}

impl Breaker {
    /// Err while the site sits out: a wait (with how long is left) when
    /// it turned us away or asked to wait; what it kept answering (a 403,
    /// no answer) when that set it aside, so it is not told as a wait.
    pub fn check(&self) -> Result<(), SourceError> {
        let st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match st.until {
            Some(until) if Instant::now() < until => Err(match &st.why {
                Some(e @ (SourceError::Http { .. } | SourceError::Unreachable(_))) => e.clone(),
                _ => SourceError::RateLimited { retry_after: Some(until - Instant::now()) },
            }),
            _ => Ok(()),
        }
    }

    /// Notes what a request did: a block or a rate limit starts (or
    /// lengthens) the pause, so do repeated 401/403 answers and repeated
    /// failures to answer; an answer ends it.
    pub fn note<T>(&self, r: &Result<T, SourceError>) {
        let mut st = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let pause = match r {
            Ok(_) => {
                *st = BreakerState::default();
                return;
            }
            Err(SourceError::Blocked(_) | SourceError::RateLimited { .. }) => true,
            Err(SourceError::Http { status: 401 | 403 } | SourceError::Unreachable(_)) => {
                st.strikes += 1;
                st.strikes >= STRIKES
            }
            Err(_) => false,
        };
        if !pause {
            return;
        }
        let n = st.pauses.min(6);
        let long = (FIRST_PAUSE * 2u32.pow(n)).min(LONGEST_PAUSE);
        let long = match r {
            Err(SourceError::RateLimited { retry_after: Some(d) }) => (*d).clamp(Duration::from_secs(60), LONGEST_PAUSE),
            _ => long,
        };
        st.until = Some(Instant::now() + long);
        st.pauses += 1;
        st.strikes = 0;
        st.why = r.as_ref().err().cloned();
    }
}

/// A Cloudflare (or alike) check page instead of the answer.
pub fn is_bot_check(status: StatusCode, body: &str) -> bool {
    matches!(status.as_u16(), 403 | 429 | 503)
        && (body.contains("Just a moment") || body.contains("cf-chl") || body.contains("Attention Required") || body.contains("challenge-platform"))
}

/// A Cloudflare Worker past the requests its free plan gives a day
/// (error 1027): nothing until the count starts over at midnight UTC.
pub fn is_daily_limit(status: StatusCode, body: &str) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS && (body.contains("Error 1027") || body.contains("error code: 1027"))
}

/// From `now` (unix seconds) to the next midnight UTC.
pub fn until_utc_midnight(now: u64) -> Duration {
    const DAY: u64 = 86_400;
    Duration::from_secs(DAY - now % DAY)
}

/// No answer, with why (the name not found, the TLS handshake): reqwest's
/// own line says only that sending failed.
fn unreachable(e: reqwest::Error) -> SourceError {
    if e.is_timeout() {
        return SourceError::Unreachable("timed out".into());
    }
    let e = e.without_url();
    let mut text = e.to_string();
    let mut cause = std::error::Error::source(&e);
    while let Some(c) = cause {
        let line = c.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        cause = c.source();
    }
    SourceError::Unreachable(text)
}

fn retry_after(resp: &Response) -> Option<Duration> {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// The cookies a site sets, sent back on the next requests as a browser
/// does: a bot guard (Cloudflare's `__cf_bm`) turns away a client that
/// comes back without the cookie it gave.
#[derive(Default)]
pub struct Cookies(Mutex<Vec<(String, String)>>);

impl Cookies {
    fn header(&self) -> Option<String> {
        let jar = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (!jar.is_empty()).then(|| jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "))
    }

    fn keep(&self, resp: &Response) {
        let mut jar = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for v in resp.headers().get_all(reqwest::header::SET_COOKIE) {
            let Some((pair, _)) = v.to_str().ok().map(|s| s.split_once(';').unwrap_or((s, ""))) else { continue };
            let Some((k, val)) = pair.split_once('=') else { continue };
            let (k, val) = (k.trim().to_owned(), val.trim().to_owned());
            jar.retain(|(name, _)| *name != k);
            if !val.is_empty() {
                jar.push((k, val));
            }
        }
    }
}

/// The body of a page or an API answer; a bot check is `Blocked`, a 429
/// `RateLimited`, any other failure `Http`.
pub async fn get_text(http: &Client, url: &str) -> Result<String, SourceError> {
    get_page(http, url, None).await
}

/// [`get_text`] with the site's cookies kept and sent back.
pub async fn get_page(http: &Client, url: &str, cookies: Option<&Cookies>) -> Result<String, SourceError> {
    let mut req = http
        .get(url)
        .header(reqwest::header::ACCEPT, "text/html,application/xhtml+xml,application/json;q=0.9,*/*;q=0.8")
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .timeout(PAGE_TIMEOUT);
    if let Some(c) = cookies.and_then(Cookies::header) {
        req = req.header(reqwest::header::COOKIE, c);
    }
    let resp = req.send().await.map_err(unreachable)?;
    if let Some(c) = cookies {
        c.keep(&resp);
    }
    let status = resp.status();
    let wait = retry_after(&resp);
    let body = resp.text().await.map_err(unreachable)?;
    if status.is_success() {
        return Ok(body);
    }
    if is_bot_check(status, &body) {
        return Err(SourceError::Blocked("a bot check".into()));
    }
    if is_daily_limit(status, &body) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        return Err(SourceError::RateLimited { retry_after: Some(until_utc_midnight(now)) });
    }
    Err(match status {
        StatusCode::TOO_MANY_REQUESTS => SourceError::RateLimited { retry_after: wait },
        s => SourceError::Http { status: s.as_u16() },
    })
}

/// A JSON answer.
pub async fn get_json<T: serde::de::DeserializeOwned>(http: &Client, url: &str) -> Result<T, SourceError> {
    let text = get_text(http, url).await?;
    serde_json::from_str(&text).map_err(|e| SourceError::Parse(e.to_string()))
}

/// Downloads `url` into `dest` (through `dest.part`), saying how far it got.
/// A page instead of audio (an HTML error) is refused.
pub async fn download(http: &Client, url: &str, dest: &Path, progress: &Progress) -> Result<PathBuf, SourceError> {
    let io = |e: std::io::Error| SourceError::Io(e.to_string());
    let mut resp = http.get(url).send().await.map_err(unreachable)?;
    let status = resp.status();
    if !status.is_success() {
        return Err(match status {
            StatusCode::TOO_MANY_REQUESTS => SourceError::RateLimited { retry_after: retry_after(&resp) },
            s => SourceError::Http { status: s.as_u16() },
        });
    }
    let html = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.starts_with("text/html"));
    if html {
        return Err(SourceError::Parse("a page instead of audio".into()));
    }
    let total = resp.content_length();
    if total.is_some_and(|t| t > MAX_FILE) {
        return Err(SourceError::Parse("the file is too large".into()));
    }
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(io)?;
    }
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let mut file = tokio::fs::File::create(&part).await.map_err(io)?;
    let mut done = 0u64;
    let result: Result<(), SourceError> = async {
        while let Some(chunk) = resp.chunk().await.map_err(unreachable)? {
            done += chunk.len() as u64;
            if done > MAX_FILE {
                return Err(SourceError::Parse("the file is too large".into()));
            }
            file.write_all(&chunk).await.map_err(io)?;
            progress(done, total);
        }
        if total.is_some_and(|t| done < t) {
            return Err(SourceError::Unreachable("the download broke off".into()));
        }
        if done == 0 {
            return Err(SourceError::Parse("an empty file".into()));
        }
        file.flush().await.map_err(io)?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, dest).await.map_err(io)?;
    Ok(dest.to_owned())
}

/// `%XX` escapes decoded (a URL's last part as a file name).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The name of an album's track file as the library lays it out ("03 -
/// Title", "2-03 - Title" on a later disc), for the import to read.
pub fn track_file_name(disc: u32, no: u32, title: &str) -> String {
    if disc > 1 {
        format!("{disc}-{no:02} - {}", safe_name(title))
    } else {
        format!("{no:02} - {}", safe_name(title))
    }
}

/// A file name of `s` that Windows takes: no `<>:"/\|?*` or control
/// characters, no dot or space at the end, not too long.
pub fn safe_name(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    if out.chars().count() > 120 {
        out = out.chars().take(120).collect();
    }
    let out = out.trim_end_matches(['.', ' ']).trim().to_owned();
    if out.is_empty() { "track".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn bot_checks_and_limits_are_told_apart() {
        let server = MockServer::start().await;
        Mock::given(path("/cf"))
            .respond_with(ResponseTemplate::new(403).set_body_string("<title>Just a moment...</title>"))
            .mount(&server)
            .await;
        Mock::given(path("/limit"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
            .mount(&server)
            .await;
        Mock::given(path("/gone")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        let http = client();
        let at = |p: &str| format!("{}{p}", server.uri());
        assert!(matches!(get_text(&http, &at("/cf")).await, Err(SourceError::Blocked(_))));
        assert_eq!(
            get_text(&http, &at("/limit")).await.unwrap_err(),
            SourceError::RateLimited { retry_after: Some(Duration::from_secs(120)) }
        );
        assert_eq!(get_text(&http, &at("/gone")).await.unwrap_err(), SourceError::Http { status: 404 });
    }

    #[tokio::test]
    async fn downloads_land_whole_or_not_at_all() {
        let server = MockServer::start().await;
        Mock::given(path("/a.flac"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fLaC\0\0\0\x22rest".to_vec()))
            .mount(&server)
            .await;
        Mock::given(path("/err.flac"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html>", "text/html"))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(Mutex::new(0u64));
        let s = seen.clone();
        let progress: Progress = Arc::new(move |done, _| *s.lock().unwrap() = done);
        let http = client();
        let got = download(&http, &format!("{}/a.flac", server.uri()), &dir.path().join("01 - A.flac"), &progress)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&got).unwrap(), b"fLaC\0\0\0\x22rest");
        assert_eq!(*seen.lock().unwrap(), 12);
        let err = download(&http, &format!("{}/err.flac", server.uri()), &dir.path().join("x.flac"), &progress).await;
        assert!(matches!(err, Err(SourceError::Parse(_))));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "nothing left of the failed one");
    }

    #[tokio::test]
    async fn cookies_a_site_sets_go_back_to_it() {
        use wiremock::matchers::header;
        let server = MockServer::start().await;
        Mock::given(path("/a"))
            .respond_with(ResponseTemplate::new(200).insert_header("set-cookie", "__cf_bm=abc; path=/; HttpOnly").set_body_string("a"))
            .mount(&server)
            .await;
        Mock::given(path("/b"))
            .and(header("cookie", "__cf_bm=abc"))
            .respond_with(ResponseTemplate::new(200).set_body_string("b"))
            .expect(1)
            .mount(&server)
            .await;
        let (http, jar) = (client(), Cookies::default());
        assert_eq!(get_page(&http, &format!("{}/a", server.uri()), Some(&jar)).await.unwrap(), "a");
        assert_eq!(get_page(&http, &format!("{}/b", server.uri()), Some(&jar)).await.unwrap(), "b");
    }

    #[test]
    fn the_breaker_pauses_after_a_block_and_an_answer_ends_it() {
        let b = Breaker::default();
        assert!(b.check().is_ok());
        b.note::<()>(&Err(SourceError::Blocked("cf".into())));
        assert!(b.check().is_err());
        b.note(&Ok(()));
        assert!(b.check().is_ok());
    }

    /// One 403 may be one track the account cannot play; twice in a row,
    /// the instance only finds.
    #[test]
    fn repeated_403s_set_an_instance_aside() {
        let b = Breaker::default();
        b.note::<()>(&Err(SourceError::Http { status: 403 }));
        assert!(b.check().is_ok(), "once is not yet");
        b.note::<()>(&Err(SourceError::Http { status: 404 }));
        b.note::<()>(&Err(SourceError::Http { status: 403 }));
        assert_eq!(b.check(), Err(SourceError::Http { status: 403 }), "told as what it answered, not as a wait");
        b.note(&Ok(()));
        b.note::<()>(&Err(SourceError::Http { status: 403 }));
        assert!(b.check().is_ok(), "an answer between starts the count over");
    }

    /// An instance whose name is gone is asked twice, then sits out; it is
    /// told as gone, not as asking to wait.
    #[test]
    fn an_instance_with_no_answer_sits_out_as_gone() {
        let b = Breaker::default();
        b.note::<()>(&Err(SourceError::Unreachable("dns error".into())));
        assert!(b.check().is_ok());
        b.note::<()>(&Err(SourceError::Unreachable("dns error".into())));
        assert_eq!(b.check(), Err(SourceError::Unreachable("dns error".into())));
    }

    /// A wait the site asked for (a Worker's day of requests) is waited out
    /// whole, not cut to the first half hour.
    #[test]
    fn a_long_wait_asked_for_is_waited_out() {
        let b = Breaker::default();
        b.note::<()>(&Err(SourceError::RateLimited { retry_after: Some(Duration::from_secs(5 * 3600)) }));
        assert!(matches!(b.check(), Err(SourceError::RateLimited { retry_after: Some(d) }) if d > Duration::from_secs(4 * 3600)));
    }

    /// A Worker out of its day's requests (what the Qobuz-DL instance
    /// answered on 2026-11-04): a wait until midnight UTC, not a ban.
    #[tokio::test]
    async fn a_worker_past_its_daily_limit_waits_for_midnight_utc() {
        let server = MockServer::start().await;
        Mock::given(path("/api"))
            .respond_with(ResponseTemplate::new(429).set_body_raw(
                "<title>This website has been temporarily rate limited</title><h2 class=\"cf-subheadline\">Error 1027</h2>",
                "text/html",
            ))
            .mount(&server)
            .await;
        let got = get_text(&client(), &format!("{}/api", server.uri())).await.unwrap_err();
        let SourceError::RateLimited { retry_after: Some(wait) } = got else { panic!("{got:?}") };
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(86_400), "{wait:?}");
        assert_eq!(until_utc_midnight(86_400 * 20_000 + 3_600), Duration::from_secs(23 * 3_600));
        assert_eq!(until_utc_midnight(86_400 * 20_000), Duration::from_secs(86_400));
    }

    #[test]
    fn names_are_safe_for_windows() {
        assert_eq!(safe_name("AC/DC: Back?"), "AC_DC_ Back_");
        assert_eq!(safe_name("Title. "), "Title");
    }
}
