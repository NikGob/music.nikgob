//! Sign-in in a window of the app (like pear-desktop): the service's own
//! sign-in page in the system webview (WebView2 on Windows, WebKitGTK on
//! Linux), then the session's cookies go into the ordinary cookies login.
//! Two sites use it ([`Site`]): YouTube Music (Google's sign-in) and
//! SoundCloud (soundcloud.com/signin, done once `.soundcloud.com` has an
//! `oauth_token` cookie; its «Continue with Google / Apple / Facebook»
//! popups are let through, every other popup is not).
//!
//! **Two processes.** winit allows one event loop per process and eframe owns
//! the app's, so the window is a child run of the same exe:
//!
//! ```text
//! nikgob.exe --youtube-login <out-file> --profile <webview-dir> [--timeout <secs>] [--watch-stdin]
//! nikgob.exe --soundcloud-login <out-file> --profile <webview-dir> [...]
//! ```
//!
//! What follows says it for YouTube Music; SoundCloud's window works the
//! same way with its own page, cookies and profile (`<data>/soundcloud/webview`).
//!
//! The child (`window` on Windows, `gtk_window` on Linux) opens «Вход в
//! YouTube Music» at Google's sign-in page with a persistent webview profile
//! (`<data>/youtube/webview`, so a later sign-in is one click). Once the
//! page is on music.youtube.com and the
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
#[cfg(target_os = "linux")]
mod gtk_window;

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nikgob_core::i18n::{Locale, locale, view_in};
use nikgob_core::i18n::view::{fmt_cookies_unreadable_in, fmt_window_error_in};
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

/// SoundCloud's sign-in page.
pub const SOUNDCLOUD_START_URL: &str = "https://soundcloud.com/signin";

/// The site a sign-in window is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Youtube,
    Soundcloud,
}

impl Site {
    /// The child run's flag.
    pub const fn flag(self) -> &'static str {
        match self {
            Self::Youtube => "--youtube-login",
            Self::Soundcloud => "--soundcloud-login",
        }
    }

    /// For the log.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Youtube => "youtube",
            Self::Soundcloud => "soundcloud",
        }
    }

    /// The window's title in `loc`.
    pub const fn title_in(self, loc: Locale) -> &'static str {
        match self {
            Self::Youtube => view_in(loc).sign_in_youtube,
            Self::Soundcloud => view_in(loc).sign_in_soundcloud,
        }
    }

    /// The window's title in the active process locale.
    pub fn title(self) -> &'static str {
        self.title_in(locale())
    }

    pub const fn start_url(self) -> &'static str {
        match self {
            Self::Youtube => START_URL,
            Self::Soundcloud => SOUNDCLOUD_START_URL,
        }
    }

    /// Where the session cookies are read from.
    pub const fn cookie_urls(self) -> &'static [&'static str] {
        match self {
            Self::Youtube => &["https://music.youtube.com/", "https://www.youtube.com/"],
            Self::Soundcloud => &["https://soundcloud.com/"],
        }
    }

    /// The page is where the cookies are worth reading.
    pub fn at_home(self, url: &str) -> bool {
        let h = host(url);
        match self {
            Self::Youtube => h == "music.youtube.com",
            Self::Soundcloud => h == "soundcloud.com" || h.ends_with(".soundcloud.com"),
        }
    }

    /// The sign-in is done.
    pub fn signed_in(self, url: &str, cookies: &[Cookie]) -> bool {
        match self {
            Self::Youtube => signed_in(url, cookies),
            Self::Soundcloud => soundcloud_signed_in(url, cookies),
        }
    }

    /// The provider refuses to sign in inside the window.
    pub fn refused(self, url: &str) -> bool {
        match self {
            Self::Youtube => refused(url),
            // Google's refusal would come in SoundCloud's «Continue with
            // Google» popup, a window of its own; the user closes it.
            Self::Soundcloud => false,
        }
    }

    /// A popup the window lets open (SoundCloud's sign-in with Google,
    /// Apple or Facebook). SoundCloud opens these as a blank window first
    /// and only then sends it to the provider, so a blank one is let
    /// through too.
    pub fn popup_allowed(self, url: &str) -> bool {
        match self {
            Self::Youtube => false,
            Self::Soundcloud => url.is_empty() || url == "about:blank" || matches!(
                host(url).as_str(),
                "accounts.google.com" | "appleid.apple.com" | "www.facebook.com" | "m.facebook.com" | "secure.soundcloud.com"
            ),
        }
    }
}

/// SoundCloud's sign-in is done: a soundcloud.com page is open and the
/// cookie store has a non-empty `oauth_token` for .soundcloud.com.
pub fn soundcloud_signed_in(url: &str, cookies: &[Cookie]) -> bool {
    Site::Soundcloud.at_home(url)
        && cookies.iter().any(|c| {
            c.name == "oauth_token"
                && c.domain.trim_start_matches('.').eq_ignore_ascii_case("soundcloud.com")
                && !c.value.is_empty()
        })
}

/// The child run's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildArgs {
    pub site: Site,
    pub out: PathBuf,
    pub profile: PathBuf,
    pub timeout: Duration,
    pub watch_stdin: bool,
}

/// `None` when this is not a child run (no `--youtube-login` or
/// `--soundcloud-login`).
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Option<Result<ChildArgs, String>> {
    let mut it = args.into_iter().skip(1);
    let mut out = None;
    let mut profile = None;
    let mut timeout = TIMEOUT;
    let mut watch_stdin = false;
    let mut child = None;
    while let Some(a) = it.next() {
        match a.to_str() {
            Some(flag @ ("--youtube-login" | "--soundcloud-login")) => {
                child = Some(if flag == "--youtube-login" { Site::Youtube } else { Site::Soundcloud });
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
    let site = child?;
    let Some(out) = out.filter(|o| !o.as_os_str().is_empty()) else {
        return Some(Err(format!("{} needs the file to write the cookies to", site.flag())));
    };
    let profile = profile.unwrap_or_else(|| out.with_file_name("webview"));
    Some(Ok(ChildArgs { site, out, profile, timeout, watch_stdin }))
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
    #[cfg(target_os = "linux")]
    {
        Some(gtk_window::run(&args))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = args;
        log::error!("sign-in window: only on Windows and Linux");
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
    site: Site,
    exe: PathBuf,
    /// The webview's profile.
    profile: PathBuf,
    /// Where the child writes the cookies.
    out: PathBuf,
    /// The UI locale taken at construction (this runs on the app's main
    /// thread); the sign-in's own thread never reads the global locale.
    loc: Locale,
}

impl WindowLogin {
    /// `dir` is the service's data folder.
    pub fn new(site: Site, dir: &Path) -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Some(Self {
            site,
            exe,
            profile: dir.join("webview"),
            out: dir.join("signin-cookies.txt"),
            loc: locale(),
        })
    }
}

/// What the user is told when the child finds no webview to show, in `loc`.
fn no_webview_in(loc: Locale) -> &'static str {
    let v = view_in(loc);
    if cfg!(windows) { v.no_webview2 } else { v.no_webkitgtk }
}

/// How long the child gets to close by itself after «Отмена».
const CANCEL_GRACE: Duration = Duration::from_secs(3);

impl WebLogin for WindowLogin {
    fn sign_in(&self, cancel: &AtomicBool) -> Result<String, WebLoginError> {
        let _ = std::fs::remove_file(&self.out);
        if let Some(d) = self.out.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let site = self.site.name();
        let mut child = Command::new(&self.exe)
            .arg(self.site.flag())
            .arg(&self.out)
            .arg("--profile")
            .arg(&self.profile)
            .arg("--watch-stdin")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| WebLoginError::Failed(e.to_string()))?;
        log::info!("{site}: sign-in window started (pid {})", child.id());
        if let Some(err) = child.stderr.take() {
            let _ = std::thread::Builder::new().name(format!("{site}-signin-log")).spawn(move || {
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
                    log::warn!("{site}: sign-in window: {e}");
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
        log::info!("{site}: sign-in window exited ({code:?})");
        if code != Some(EXIT_OK) {
            // Whatever it left behind (a close that raced the write) is
            // not a sign-in.
            let _ = std::fs::remove_file(&self.out);
        }
        match code {
            Some(EXIT_OK) => match read() {
                Ok(t) if !t.trim().is_empty() => Ok(t),
                Ok(_) => Err(WebLoginError::Failed(view_in(self.loc).window_no_cookies.into())),
                Err(e) => Err(WebLoginError::Failed(fmt_cookies_unreadable_in(self.loc, &e.to_string()))),
            },
            Some(EXIT_CLOSED) => Err(WebLoginError::Closed),
            Some(EXIT_TIMEOUT) => Err(WebLoginError::TimedOut),
            Some(EXIT_REFUSED) => Err(WebLoginError::Refused),
            Some(EXIT_NO_WEBVIEW) => Err(WebLoginError::Failed(no_webview_in(self.loc).into())),
            other => {
                Err(WebLoginError::Failed(match other {
                    Some(c) => fmt_window_error_in(self.loc, c),
                    None => view_in(self.loc).window_crashed.into(),
                }))
            }
        }
    }

    fn forget(&self) {
        // The webview's own processes (WebView2's, WebKit's) may hold the
        // files for a moment after the window is gone.
        for _ in 0..20 {
            match std::fs::remove_dir_all(&self.profile) {
                Ok(()) => {
                    log::info!("{}: sign-in window profile forgotten", self.site.name());
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) => std::thread::sleep(Duration::from_millis(500)),
            }
        }
        log::warn!("{}: cannot remove {}", self.site.name(), self.profile.display());
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
    fn soundcloud_completes_with_its_token() {
        let token = [c("soundcloud.com", "oauth_token", "2-1-2-abc")];
        assert!(Site::Soundcloud.signed_in("https://soundcloud.com/discover", &token));
        assert!(Site::Soundcloud.signed_in("https://soundcloud.com/signin", &[c(".soundcloud.com", "oauth_token", "t")]));
        assert!(!Site::Soundcloud.signed_in("https://soundcloud.com/", &[c(".soundcloud.com", "oauth_token", "")]));
        assert!(!Site::Soundcloud.signed_in("https://accounts.google.com/", &token));
        assert!(!Site::Soundcloud.signed_in("https://soundcloud.com/", &[c(".soundcloud.com", "sc_anonymous_id", "x")]));
        assert!(Site::Soundcloud.popup_allowed("https://accounts.google.com/o/oauth2/auth?x"));
        assert!(!Site::Soundcloud.popup_allowed("https://evil.example/"));
        assert!(!Site::Youtube.popup_allowed("https://accounts.google.com/"));
        // «Continue with Google» opens a blank popup first.
        assert!(Site::Soundcloud.popup_allowed("about:blank"));
        assert!(!Site::Youtube.popup_allowed("about:blank"));
        // The window's cookies go through the service's own parser.
        let text = netscape(&[c(".soundcloud.com", "oauth_token", "2-1-2-abc")]);
        assert_eq!(nikgob_soundcloud::token::parse(&text).as_deref(), Some("2-1-2-abc"));
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
        // The profile goes beside the file, in this system's own paths.
        let (out, profile) = if cfg!(windows) { ("C:\\d\\out.txt", "C:\\d\\webview") } else { ("/d/out.txt", "/d/webview") };
        assert_eq!(
            args(&["--youtube-login", out]),
            Some(Ok(ChildArgs {
                site: Site::Youtube,
                out: out.into(),
                profile: profile.into(),
                timeout: TIMEOUT,
                watch_stdin: false,
            }))
        );
        assert_eq!(
            args(&["--youtube-login", "o.txt", "--profile", "p", "--timeout", "30", "--watch-stdin"]),
            Some(Ok(ChildArgs {
                site: Site::Youtube,
                out: "o.txt".into(),
                profile: "p".into(),
                timeout: Duration::from_secs(30),
                watch_stdin: true,
            }))
        );
        assert!(matches!(args(&["--youtube-login"]), Some(Err(_))));
        assert_eq!(args(&["--soundcloud-login", "o.txt", "--profile", "p"]).unwrap().unwrap().site, Site::Soundcloud);
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
