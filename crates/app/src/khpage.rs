//! A hidden webview that reads one khinsider page after Cloudflare's check
//! (see `nikgob_sources::khinsider`). Modelled on [`crate::ytlogin`]: winit
//! allows one event loop per process, so the window is a child run of the
//! same exe.
//!
//! ```text
//! nikgob.exe --khinsider-page <url> --out <file> --profile <dir> [--timeout <secs>] [--watch-stdin]
//! ```
//!
//! The child opens the URL in its own profile (`<data>/khinsider/webview`),
//! off-screen, waits up to 20 s for the title to leave «Just a moment…»,
//! reads `document.documentElement.outerHTML` once and writes it to `<out>`.
//! Cookies stay in that profile; nothing is copied into the HTTP client.
//! No WebView2 / WebKitGTK: exit [`EXIT_NO_WEBVIEW`], and the source keeps
//! the HTTP answer.

#[cfg(windows)]
mod window;
#[cfg(target_os = "linux")]
mod gtk_window;

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::indexer::BoxFuture;
use nikgob_core::source::SourceError;
use nikgob_runtime::PageBrowser;

pub const EXIT_OK: i32 = 0;
pub const EXIT_ERROR: i32 = 1;
pub const EXIT_TIMEOUT: i32 = 3;
pub const EXIT_NO_WEBVIEW: i32 = 5;

/// How long the child waits for the check to clear.
pub const PAGE_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the parent waits for the child, past the child's own timeout.
const PARENT_WAIT: Duration = Duration::from_secs(25);
const FLAG: &str = "--khinsider-page";

/// The child run's arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildArgs {
    pub url: String,
    pub out: PathBuf,
    pub profile: PathBuf,
    pub timeout: Duration,
    pub watch_stdin: bool,
}

/// `None` when this is not a `--khinsider-page` run.
pub fn parse_args(args: impl IntoIterator<Item = OsString>) -> Option<Result<ChildArgs, String>> {
    let mut it = args.into_iter().skip(1);
    let mut url = None;
    let mut out = None;
    let mut profile = None;
    let mut timeout = PAGE_TIMEOUT;
    let mut watch_stdin = false;
    let mut child = false;
    while let Some(a) = it.next() {
        match a.to_str() {
            Some(flag) if flag == FLAG => {
                child = true;
                url = it.next().and_then(|s| s.into_string().ok());
            }
            Some("--out") => out = it.next().map(PathBuf::from),
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
    let Some(url) = url.filter(|u| u.starts_with("https://")) else {
        return Some(Err("--khinsider-page needs an https URL".into()));
    };
    let Some(out) = out.filter(|o| !o.as_os_str().is_empty()) else {
        return Some(Err("--khinsider-page needs --out".into()));
    };
    let profile = profile.unwrap_or_else(|| out.with_file_name("webview"));
    Some(Ok(ChildArgs { url, out, profile, timeout, watch_stdin }))
}

/// The child run, if this is one: its exit code.
pub fn child_main() -> Option<i32> {
    let args = match parse_args(std::env::args_os())? {
        Ok(a) => a,
        Err(e) => {
            log::error!("khinsider page: {e}");
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
        log::error!("khinsider page: only on Windows and Linux");
        Some(EXIT_NO_WEBVIEW)
    }
}

/// The document title is still Cloudflare's check, or not a page yet.
pub fn title_cleared(title: &str) -> bool {
    let t = title.trim().to_ascii_lowercase();
    !t.is_empty() && !t.contains("just a moment") && !t.contains("checking your") && !t.contains("attention required") && !t.contains("cloudflare")
}

/// The HTML is still the check page.
pub fn html_is_check(html: &str) -> bool {
    html.contains("Just a moment") || html.contains("cf-chl") || html.contains("challenge-platform") || html.contains("Attention Required")
}

/// Reads `documentElement.outerHTML` and posts it. Called once the title
/// has cleared, and again only if that read was still the check.
pub const READ_HTML: &str = "window.ipc.postMessage(document.documentElement ? document.documentElement.outerHTML : '')";

/// The hidden page reader. One child at a time.
#[derive(Clone)]
pub struct HiddenPage {
    inner: Arc<Inner>,
}

struct Inner {
    exe: PathBuf,
    profile: PathBuf,
    gate: Mutex<()>,
}

impl HiddenPage {
    /// `dir` is `<data>/khinsider`. `None` when this exe cannot be found.
    pub fn new(dir: &Path) -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        Some(Self {
            inner: Arc::new(Inner { exe, profile: dir.join("webview"), gate: Mutex::new(()) }),
        })
    }

    fn fetch(&self, url: &str) -> Result<String, SourceError> {
        let _gate = self.inner.gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let out = std::env::temp_dir().join(format!("nikgob-khinsider-{n}.html"));
        let _ = std::fs::remove_file(&out);
        if let Err(e) = std::fs::create_dir_all(&self.inner.profile) {
            log::warn!("khinsider: cannot create {}: {e}", self.inner.profile.display());
            return Err(SourceError::NotConfigured);
        }
        let mut child = Command::new(&self.inner.exe)
            .arg(FLAG)
            .arg(url)
            .arg("--out")
            .arg(&out)
            .arg("--profile")
            .arg(&self.inner.profile)
            .arg("--timeout")
            .arg(PAGE_TIMEOUT.as_secs().to_string())
            .arg("--watch-stdin")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                log::warn!("khinsider: cannot open the page window: {e}");
                SourceError::NotConfigured
            })?;
        log::info!("khinsider: page window started (pid {})", child.id());
        if let Some(err) = child.stderr.take() {
            let _ = std::thread::Builder::new().name("khinsider-page-log".into()).spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    log::info!("[khinsider page] {line}");
                }
            });
        }
        let stdin = child.stdin.take();
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break Some(s),
                Ok(None) if started.elapsed() > PARENT_WAIT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(150)),
                Err(e) => {
                    log::warn!("khinsider: page window: {e}");
                    break None;
                }
            }
        };
        drop(stdin);
        let code = status.and_then(|s| s.code());
        let read = std::fs::read_to_string(&out).ok().filter(|s| !s.is_empty());
        let _ = std::fs::remove_file(&out);
        log::info!("khinsider: page window exited ({code:?})");
        match code {
            Some(EXIT_NO_WEBVIEW) => Err(SourceError::NotConfigured),
            Some(EXIT_OK) => read.ok_or_else(|| SourceError::Blocked(nikgob_sources_bot())),
            _ => Err(SourceError::Blocked(nikgob_sources_bot())),
        }
    }
}

fn nikgob_sources_bot() -> String {
    "a bot check".into()
}

impl PageBrowser for HiddenPage {
    fn document<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, SourceError>> {
        let this = self.clone();
        let url = url.to_owned();
        Box::pin(async move {
            match tokio::task::spawn_blocking(move || this.fetch(&url)).await {
                Ok(r) => r,
                Err(e) => Err(SourceError::Io(e.to_string())),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_child_flag_is_parsed_and_a_check_title_is_not_cleared() {
        let args = ["nikgob", FLAG, "https://downloads.khinsider.com/search?search=a", "--out", "page.html", "--profile", "prof", "--timeout", "20"];
        let parsed = parse_args(args.into_iter().map(OsString::from)).unwrap().unwrap();
        assert_eq!(parsed.url, "https://downloads.khinsider.com/search?search=a");
        assert_eq!(parsed.out, PathBuf::from("page.html"));
        assert_eq!(parsed.profile, PathBuf::from("prof"));
        assert_eq!(parsed.timeout, PAGE_TIMEOUT);
        assert!(parse_args(["nikgob", "--other"].into_iter().map(OsString::from)).is_none());
        assert!(!title_cleared("Checking your browser"));
        assert!(title_cleared("Search - KHInsider"));
        assert!(html_is_check("<title>Just a moment...</title>"));
        assert!(!html_is_check("Found 3 matching albums"));
    }
}
