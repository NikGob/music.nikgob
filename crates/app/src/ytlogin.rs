//! YouTube Music sign-in in a window of the app (like pear-desktop): Google's
//! own sign-in page in the system WebView2, then the session's cookies go
//! into the ordinary cookies login.
//!
//! **Two processes.** winit allows one event loop per process and eframe owns
//! the app's, so the window is a child run of the same exe:
//!
//! ```text
//! nikgob.exe --youtube-login <out-file> --profile <webview-dir> [--timeout <secs>] [--watch-stdin]
//! ```
//!
//! The child ([`window`]) opens «Вход в YouTube Music» at Google's sign-in
//! page with a persistent WebView2 profile (`<data>/youtube/webview`, so a
//! later sign-in is one click). Once the page is on music.youtube.com and the
//! cookie store has `SAPISID` and `LOGIN_INFO` for .youtube.com, it writes
//! the cookies of music.youtube.com and www.youtube.com as a Netscape
//! cookies.txt to `<out-file>` and exits [`EXIT_OK`]; a closed window exits
//! [`EXIT_CLOSED`], 10 minutes without a sign-in [`EXIT_TIMEOUT`], Google's
//! "this browser may not be secure" page [`EXIT_REFUSED`]. It logs the
//! pages it visits (no query strings) and titles to stderr, never cookie
//! values. With `--watch-stdin` it closes itself when its stdin closes: the
//! parent's «Отмена», or the parent gone.
//!
//! The parent ([`WindowLogin`], a [`nikgob_runtime::WebLogin`] the app gives
//! the runtime with `Runtime::set_web_login`) runs the child, logs its
//! stderr, reads and deletes the file, and hands the text back; the runtime
//! then logs in with `LoginInput::Cookies`, so the cookies are stored exactly
//! like pasted ones. The user types the password only into Google's page.

#[cfg(windows)]
mod window;

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nikgob_runtime::{WebLogin, WebLoginError};

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_CLOSED: i32 = 2;
pub const EXIT_TIMEOUT: i32 = 3;
pub const EXIT_REFUSED: i32 = 4;
pub const EXIT_NO_WEBVIEW: i32 = 5;

/// Where the window starts: Google's sign-in, then back to YouTube Music.
pub const START_URL: &str =
    "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";
/// How long the window waits for a sign-in.
pub const TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The cookie domain RustyPipe takes its login cookies from.
const COOKIE_DOMAIN: &str = ".youtube.com";

/// The child run's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildArgs {
    pub out: PathBuf,
    pub profile: PathBuf,
    pub timeout: Duration,
    pub watch_stdin: bool,
}

/// `None` when this is not a child run (no `--youtube-login`).
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Option<Result<ChildArgs, String>> {
    let mut it = args.into_iter().skip(1);
    let mut out = None;
    let mut profile = None;
    let mut timeout = TIMEOUT;
    let mut watch_stdin = false;
    let mut child = false;
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--youtube-login") => {
                child = true;
                out = it.next().map(PathBuf::from);
            }
            Some("--profile") => profile = it.next().map(PathBuf::from),
            Some("--timeout") => match it.next().and_then(|s| s.to_str()?.parse::<u64>().ok()) {
                Some(s) if s > 0 => timeout = Duration::from_secs(s),
                _ => return Some(Err("--timeout needs a number of seconds".into())),
            },
            Some("--watch-stdin") => watch_stdin = true,
            _ => {}
        }
    }
    if !child {
        return None;
    }
    let Some(out) = out.filter(|o| !o.as_os_str().is_empty()) else {
        return Some(Err("--youtube-login needs the file to write the cookies to".into()));
    };
    let profile = profile.unwrap_or_else(|| out.with_file_name("webview"));
    Some(Ok(ChildArgs { out, profile, timeout, watch_stdin }))
}

/// The child run, if this is one: its exit code.
pub fn child_main() -> Option<i32> {
    let args = match parse_args(std::env::args_os())? {
        Ok(a) => a,
        Err(e) => {
            log::error!("sign-in window: {e}");
            return Some(EXIT_ERROR);
        }
    };
    #[cfg(windows)]
    {
        Some(window::run(&args))
    }
    #[cfg(not(windows))]
    {
        let _ = args;
        log::error!("sign-in window: only on Windows");
        Some(EXIT_ERROR)
    }
}

// ---------------------------------------------------------------------------
// Cookies
// ---------------------------------------------------------------------------

/// One cookie of the WebView's store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cookie {
    /// `.youtube.com` (the domain and its subdomains) or a host.
    pub domain: String,
    pub path: String,
    pub name: String,
    pub value: String,
    pub secure: bool,
    pub http_only: bool,
    /// Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
}

/// A Netscape cookies.txt (what browser extensions export and RustyPipe
/// reads): HttpOnly cookies as `#HttpOnly_` lines, session cookies with
/// expiry 0. Duplicates (domain, path, name) keep the first; cookies whose
/// fields would break the format are left out.
pub fn netscape(cookies: &[Cookie]) -> String {
    let mut out = String::from("# Netscape HTTP Cookie File\n# Written by music.nikgob's sign-in window. Secret: do not share.\n\n");
    let mut seen = std::collections::HashSet::new();
    let bad = |s: &str| s.contains(['\t', '\n', '\r']);
    for c in cookies {
        let path = if c.path.is_empty() { "/" } else { &c.path };
        if c.name.is_empty() || c.domain.is_empty() || [&c.domain, path, &c.name, &c.value].iter().any(|s| bad(s)) {
            continue;
        }
        if !seen.insert((c.domain.to_ascii_lowercase(), path.to_owned(), c.name.clone())) {
            continue;
        }
        let flag = |b: bool| if b { "TRUE" } else { "FALSE" };
        out.push_str(&format!(
            "{}{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            if c.http_only { "#HttpOnly_" } else { "" },
            c.domain,
            flag(c.domain.starts_with('.')),
            path,
            flag(c.secure),
            c.expires.unwrap_or(0).max(0),
            c.name,
            c.value,
        ));
    }
    out
}

/// A cookie's domain as the cookies file needs it. The `cookie` crate (what
/// wry hands out) strips the leading dot of a domain cookie, so
/// `.youtube.com` comes back as `youtube.com`; a cookie read for a page
/// whose host is not its domain can only be a domain cookie and gets its
/// dot back, a host-only one keeps its host.
pub fn cookie_domain(raw: &str, read_for_host: &str) -> String {
    if raw.is_empty() || raw.starts_with('.') || raw.eq_ignore_ascii_case(read_for_host) {
        raw.to_owned()
    } else {
        format!(".{raw}")
    }
}

/// The host of an http(s) url, lowercased.
pub fn host(url: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else { return String::new() };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    host.split(':').next().unwrap_or("").to_ascii_lowercase()
}

/// A url without its query and fragment (for logs: Google's urls carry
/// session tokens there).
pub fn redact(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// The sign-in is done: YouTube Music is open and its cookie store has the
/// cookies RustyPipe needs (`SAPISID` for its auth header and `LOGIN_INFO`,
/// both for .youtube.com).
pub fn signed_in(url: &str, cookies: &[Cookie]) -> bool {
    let has = |name: &str| {
        cookies
            .iter()
            .any(|c| {
                c.name == name
                    && c.domain.trim_start_matches('.').eq_ignore_ascii_case(COOKIE_DOMAIN.trim_start_matches('.'))
                    && !c.value.is_empty()
            })
    };
    host(url) == "music.youtube.com" && has("SAPISID") && has("LOGIN_INFO")
}

/// Google's "Couldn't sign you in — this browser or app may not be secure".
pub fn refused(url: &str) -> bool {
    let path = redact(url);
    host(url) == "accounts.google.com"
        && (path.contains("/signin/rejected") || path.contains("/signin/v2/rejected") || url.contains("disallowed_useragent"))
}

/// Writes the cookies file: a new file only (a stale one is removed first),
/// opened unshared while written, inside the app's per-user data folder.
pub fn write_secret(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let _ = std::fs::remove_file(path);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        o.share_mode(0);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

// ---------------------------------------------------------------------------
// Parent
// ---------------------------------------------------------------------------

/// Runs the sign-in window as a child of this exe (see the module docs).
pub struct WindowLogin {
    exe: PathBuf,
    /// The WebView2 profile.
    profile: PathBuf,
    /// Where the child writes the cookies.
    out: PathBuf,
}

impl WindowLogin {
    /// `dir` is YouTube Music's data folder.
    pub fn new(dir: &Path) -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Some(Self {
            exe,
            profile: dir.join("webview"),
            out: dir.join("signin-cookies.txt"),
        })
    }
}

/// How long the child gets to close by itself after «Отмена».
const CANCEL_GRACE: Duration = Duration::from_secs(3);

impl WebLogin for WindowLogin {
    fn sign_in(&self, cancel: &AtomicBool) -> Result<String, WebLoginError> {
        let _ = std::fs::remove_file(&self.out);
        if let Some(d) = self.out.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let mut child = Command::new(&self.exe)
            .arg("--youtube-login")
            .arg(&self.out)
            .arg("--profile")
            .arg(&self.profile)
            .arg("--watch-stdin")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| WebLoginError::Failed(e.to_string()))?;
        log::info!("youtube: sign-in window started (pid {})", child.id());
        if let Some(err) = child.stderr.take() {
            let _ = std::thread::Builder::new().name("youtube-signin-log".into()).spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    log::info!("[sign-in window] {line}");
                }
            });
        }
        let mut stdin = child.stdin.take();
        let mut cancelled: Option<Instant> = None;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) => {}
                Err(e) => {
                    log::warn!("youtube: sign-in window: {e}");
                    break None;
                }
            }
            if cancelled.is_none() && cancel.load(Ordering::SeqCst) {
                // Closing its stdin asks it to close (and keep its profile tidy).
                stdin = None;
                cancelled = Some(Instant::now());
            }
            if cancelled.is_some_and(|t| t.elapsed() > CANCEL_GRACE) {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(150));
        };
        drop(stdin);
        let read = || {
            let text = std::fs::read_to_string(&self.out);
            let _ = std::fs::remove_file(&self.out);
            text
        };
        if cancelled.is_some() {
            let _ = std::fs::remove_file(&self.out);
            return Err(WebLoginError::Cancelled);
        }
        let code = status.and_then(|s| s.code());
        log::info!("youtube: sign-in window exited ({code:?})");
        match code {
            Some(EXIT_OK) => match read() {
                Ok(t) if !t.trim().is_empty() => Ok(t),
                Ok(_) => Err(WebLoginError::Failed("окно не отдало cookies".into())),
                Err(e) => Err(WebLoginError::Failed(format!("cookies не читаются: {e}"))),
            },
            Some(EXIT_CLOSED) => Err(WebLoginError::Closed),
            Some(EXIT_TIMEOUT) => Err(WebLoginError::TimedOut),
            Some(EXIT_REFUSED) => Err(WebLoginError::Refused),
            Some(EXIT_NO_WEBVIEW) => Err(WebLoginError::Failed("в системе нет WebView2".into())),
            other => {
                let _ = std::fs::remove_file(&self.out);
                Err(WebLoginError::Failed(match other {
                    Some(c) => format!("ошибка {c}"),
                    None => "окно упало".into(),
                }))
            }
        }
    }

    fn forget(&self) {
        // WebView2's own processes may hold the files for a moment after the
        // window is gone.
        for _ in 0..20 {
            match std::fs::remove_dir_all(&self.profile) {
                Ok(()) => {
                    log::info!("youtube: sign-in window profile forgotten");
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        log::warn!("youtube: cannot remove {}", self.profile.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(domain: &str, name: &str, value: &str) -> Cookie {
        Cookie {
            domain: domain.into(),
            path: "/".into(),
            name: name.into(),
            value: value.into(),
            secure: true,
            http_only: false,
            expires: Some(1_900_000_000),
        }
    }

    #[test]
    fn netscape_lines() {
        let mut http = c(".youtube.com", "LOGIN_INFO", "abc");
        http.http_only = true;
        let mut session = c("music.youtube.com", "S", "1");
        session.expires = None;
        session.secure = false;
        session.path = String::new();
        let text = netscape(&[
            c(".youtube.com", "SAPISID", "x/y"),
            http,
            session,
            c(".youtube.com", "SAPISID", "dup"),
            c(".youtube.com", "BAD", "a\tb"),
        ]);
        let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty() && !l.starts_with("# ")).collect();
        assert!(text.starts_with("# Netscape HTTP Cookie File\n"));
        assert_eq!(
            lines,
            [
                ".youtube.com\tTRUE\t/\tTRUE\t1900000000\tSAPISID\tx/y",
                "#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1900000000\tLOGIN_INFO\tabc",
                "music.youtube.com\tFALSE\t/\tFALSE\t0\tS\t1",
            ]
        );
    }

    #[test]
    fn domain_cookies_get_their_dot_back() {
        // wry gives `.youtube.com` as `youtube.com` (the cookie crate strips the dot).
        assert_eq!(cookie_domain("youtube.com", "music.youtube.com"), ".youtube.com");
        assert_eq!(cookie_domain("youtube.com", "www.youtube.com"), ".youtube.com");
        assert_eq!(cookie_domain(".youtube.com", "music.youtube.com"), ".youtube.com");
        assert_eq!(cookie_domain("music.youtube.com", "music.youtube.com"), "music.youtube.com");
        assert_eq!(cookie_domain("", "music.youtube.com"), "");
    }

    #[test]
    fn signed_in_with_the_dot_stripped() {
        let url = "https://music.youtube.com/";
        let stripped = [c("youtube.com", "SAPISID", "x"), c("youtube.com", "LOGIN_INFO", "y")];
        assert!(signed_in(url, &stripped));
        let dotted = [c(".youtube.com", "SAPISID", "x"), c(".youtube.com", "LOGIN_INFO", "y")];
        assert!(signed_in(url, &dotted));
        assert!(!signed_in(url, &[c("youtube.com", "SAPISID", "x")]));
        assert!(!signed_in(url, &[c("music.youtube.com", "SAPISID", "x"), c("music.youtube.com", "LOGIN_INFO", "y")]));
    }

    #[test]
    fn netscape_is_what_the_cookies_login_takes() {
        // The youtube crate takes a text as cookies.txt when a line has 7 tab fields.
        let text = netscape(&[c(".youtube.com", "SAPISID", "v")]);
        assert!(text.lines().any(|l| l.split('\t').count() == 7));
    }

    #[test]
    fn completion() {
        let both = [c(".youtube.com", "SAPISID", "a"), c(".youtube.com", "LOGIN_INFO", "b")];
        assert!(signed_in("https://music.youtube.com/", &both));
        assert!(signed_in("https://music.youtube.com/library?x=1", &both));
        // Still on Google, or another YouTube.
        assert!(!signed_in("https://accounts.google.com/ServiceLogin", &both));
        assert!(!signed_in("https://www.youtube.com/", &both));
        // A cookie missing, empty or on another domain.
        assert!(!signed_in("https://music.youtube.com/", &both[..1]));
        assert!(!signed_in("https://music.youtube.com/", &[c(".youtube.com", "SAPISID", "a"), c(".youtube.com", "LOGIN_INFO", "")]));
        assert!(!signed_in("https://music.youtube.com/", &[c(".google.com", "SAPISID", "a"), c(".youtube.com", "LOGIN_INFO", "b")]));
        // __Secure-3PAPISID alone is not enough for RustyPipe's auth header.
        assert!(!signed_in("https://music.youtube.com/", &[c(".youtube.com", "__Secure-3PAPISID", "a"), c(".youtube.com", "LOGIN_INFO", "b")]));
    }

    #[test]
    fn refusal_and_urls() {
        assert!(refused("https://accounts.google.com/v3/signin/rejected?continue=x"));
        assert!(refused("https://accounts.google.com/signin/rejected"));
        assert!(!refused("https://accounts.google.com/v3/signin/identifier?dsh=1"));
        assert!(!refused("https://music.youtube.com/signin/rejected"));
        assert_eq!(host("https://User@Music.YouTube.com:443/x?y"), "music.youtube.com");
        assert_eq!(host("about:blank"), "");
        assert_eq!(redact("https://accounts.google.com/a/b?token=secret#f"), "https://accounts.google.com/a/b");
    }

    fn args(a: &[&str]) -> Option<Result<ChildArgs, String>> {
        parse_args(std::iter::once("nikgob.exe").chain(a.iter().copied()).map(OsString::from))
    }

    #[test]
    fn child_args() {
        assert_eq!(args(&[]), None);
        assert_eq!(args(&["--other"]), None);
        assert_eq!(
            args(&["--youtube-login", "C:\\d\\out.txt"]),
            Some(Ok(ChildArgs {
                out: "C:\\d\\out.txt".into(),
                profile: "C:\\d\\webview".into(),
                timeout: TIMEOUT,
                watch_stdin: false,
            }))
        );
        assert_eq!(
            args(&["--youtube-login", "o.txt", "--profile", "p", "--timeout", "30", "--watch-stdin"]),
            Some(Ok(ChildArgs {
                out: "o.txt".into(),
                profile: "p".into(),
                timeout: Duration::from_secs(30),
                watch_stdin: true,
            }))
        );
        assert!(matches!(args(&["--youtube-login"]), Some(Err(_))));
        assert!(matches!(args(&["--youtube-login", "o", "--timeout", "x"]), Some(Err(_))));
    }

    #[test]
    fn secret_file_replaces_a_stale_one() {
        let dir = std::env::temp_dir().join(format!("nikgob-ytlogin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.txt");
        std::fs::write(&p, "old").unwrap();
        write_secret(&p, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
