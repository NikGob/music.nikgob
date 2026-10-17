//! The child run: one winit window with a WebView2 webview (wry) showing
//! Google's sign-in (see the parent module).

use std::io::Read as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Icon, Window, WindowId};
use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder};

use super::{
    ChildArgs, Cookie, EXIT_CLOSED, EXIT_ERROR, EXIT_NO_WEBVIEW, EXIT_OK, EXIT_REFUSED, EXIT_TIMEOUT,
    START_URL, netscape, redact, refused, signed_in, write_secret,
};

const TITLE: &str = "Вход в YouTube Music";
/// How often the page and the cookie store are looked at.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// Where the session cookies are read from.
const COOKIE_URLS: [&str; 2] = ["https://music.youtube.com/", "https://www.youtube.com/"];

pub fn run(args: &ChildArgs) -> i32 {
    match wry::webview_version() {
        Ok(v) => log::info!("WebView2 {v}"),
        Err(e) => {
            log::error!("no WebView2 runtime: {e}");
            return EXIT_NO_WEBVIEW;
        }
    }
    if let Err(e) = std::fs::create_dir_all(&args.profile) {
        log::error!("cannot create {}: {e}", args.profile.display());
        return EXIT_ERROR;
    }
    let parent_gone = Arc::new(AtomicBool::new(false));
    if args.watch_stdin {
        let flag = parent_gone.clone();
        let _ = std::thread::Builder::new().name("stdin-watch".into()).spawn(move || {
            let mut buf = [0u8; 64];
            let mut stdin = std::io::stdin();
            while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
            flag.store(true, Ordering::SeqCst);
        });
    }
    let event_loop = match EventLoop::new() {
        Ok(l) => l,
        Err(e) => {
            log::error!("no event loop: {e}");
            return EXIT_ERROR;
        }
    };
    let mut app = App {
        args: args.clone(),
        context: WebContext::new(Some(args.profile.clone())),
        window: None,
        webview: None,
        started: Instant::now(),
        next_check: Instant::now() + CHECK_EVERY,
        last_url: String::new(),
        parent_gone,
        outcome: None,
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        log::error!("event loop: {e}");
        return EXIT_ERROR;
    }
    let code = app.outcome.unwrap_or(EXIT_ERROR);
    log::info!("exit {code}");
    code
}

struct App {
    args: ChildArgs,
    context: WebContext,
    window: Option<Window>,
    webview: Option<WebView>,
    started: Instant,
    next_check: Instant,
    last_url: String,
    parent_gone: Arc<AtomicBool>,
    outcome: Option<i32>,
}

impl App {
    fn open(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let icon = Icon::from_rgba(crate::desktop::icon_rgba(64), 64, 64).ok();
        let attrs = Window::default_attributes()
            .with_title(TITLE)
            .with_inner_size(LogicalSize::new(520.0, 720.0))
            .with_min_inner_size(LogicalSize::new(380.0, 480.0))
            .with_window_icon(icon)
            .with_visible(false);
        let window = el.create_window(attrs).map_err(|e| e.to_string())?;
        if let Some(m) = window.current_monitor().or_else(|| el.primary_monitor()) {
            let (ms, mp, ws) = (m.size(), m.position(), window.outer_size());
            let x = mp.x + (i64::from(ms.width) - i64::from(ws.width)).max(0) as i32 / 2;
            let y = mp.y + (i64::from(ms.height) - i64::from(ws.height)).max(0) as i32 / 2;
            window.set_outer_position(PhysicalPosition::new(x, y));
        }
        window.set_visible(true);
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_url(START_URL)
            .with_document_title_changed_handler(|t| log::info!("title: {t}"))
            .with_on_page_load_handler(|ev, url| {
                if matches!(ev, PageLoadEvent::Finished) {
                    log::info!("loaded {}", redact(&url));
                }
            })
            // Help links and the like: no second window.
            .with_new_window_req_handler(|url, _| {
                log::info!("popup not opened: {}", redact(&url));
                NewWindowResponse::Deny
            })
            .build(&window)
            .map_err(|e| e.to_string())?;
        window.focus_window();
        self.window = Some(window);
        self.webview = Some(webview);
        log::info!("window open at {}", redact(START_URL));
        Ok(())
    }

    fn finish(&mut self, el: &ActiveEventLoop, code: i32) {
        if self.outcome.is_none() {
            self.outcome = Some(code);
        }
        // The webview first: it writes its profile as it goes away.
        self.webview = None;
        self.window = None;
        el.exit();
    }

    fn check(&mut self, el: &ActiveEventLoop) {
        if self.parent_gone.load(Ordering::SeqCst) {
            log::info!("the app asked to close");
            return self.finish(el, EXIT_CLOSED);
        }
        if self.started.elapsed() > self.args.timeout {
            log::info!("no sign-in in {:?}", self.args.timeout);
            return self.finish(el, EXIT_TIMEOUT);
        }
        let Some(webview) = &self.webview else { return };
        let url = webview.url().unwrap_or_default();
        if url != self.last_url {
            log::info!("at {}", redact(&url));
            self.last_url.clone_from(&url);
        }
        if refused(&url) {
            log::warn!("Google refused the sign-in in this window");
            return self.finish(el, EXIT_REFUSED);
        }
        if super::host(&url) != "music.youtube.com" {
            return;
        }
        let mut cookies = Vec::new();
        for u in COOKIE_URLS {
            match webview.cookies_for_url(u) {
                Ok(list) => cookies.extend(list.iter().map(convert)),
                Err(e) => log::warn!("cookies of {u}: {e}"),
            }
        }
        if !signed_in(&url, &cookies) {
            return;
        }
        log::info!("signed in: {} cookies", cookies.len());
        let code = match write_secret(&self.args.out, &netscape(&cookies)) {
            Ok(()) => EXIT_OK,
            Err(e) => {
                log::error!("cannot write {}: {e}", self.args.out.display());
                EXIT_ERROR
            }
        };
        self.finish(el, code);
    }
}

fn convert(c: &wry::cookie::Cookie<'_>) -> Cookie {
    Cookie {
        domain: c.domain().unwrap_or_default().to_owned(),
        path: c.path().unwrap_or("/").to_owned(),
        name: c.name().to_owned(),
        value: c.value().to_owned(),
        secure: c.secure().unwrap_or(false),
        http_only: c.http_only().unwrap_or(false),
        expires: c.expires_datetime().map(|t| t.unix_timestamp()),
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() || self.outcome.is_some() {
            return;
        }
        if let Err(e) = self.open(el) {
            log::error!("cannot open the window: {e}");
            self.finish(el, EXIT_ERROR);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
            log::info!("window closed");
            self.finish(el, EXIT_CLOSED);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.outcome.is_some() {
            return;
        }
        if Instant::now() >= self.next_check {
            self.check(el);
            self.next_check = Instant::now() + CHECK_EVERY;
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next_check));
    }
}
