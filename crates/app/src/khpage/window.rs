//! The child run on Windows: one off-screen WebView2 window (wry) that
//! reads a khinsider page once the check has cleared.

use std::io::Read as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::platform::windows::WindowAttributesExtWindows as _;
use winit::window::{Window, WindowId};
use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows as _};

use super::{ChildArgs, EXIT_ERROR, EXIT_NO_WEBVIEW, EXIT_OK, EXIT_TIMEOUT, READ_HTML, html_is_check, title_cleared};

const CHECK_EVERY: Duration = Duration::from_millis(500);
/// Chromium throttles a window it thinks is covered. These are the flags
/// the SoundCloud host already uses so an off-screen webview keeps running.
const BROWSER_ARGS: &str = "--disable-features=CalculateNativeWinOcclusion \
    --disable-backgrounding-occluded-windows --disable-renderer-backgrounding \
    --disable-background-timer-throttling";

enum Msg {
    Html(String),
}

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
    let event_loop = match EventLoop::<Msg>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => {
            log::error!("no event loop: {e}");
            return EXIT_ERROR;
        }
    };
    let mut app = App {
        args: args.clone(),
        context: WebContext::new(Some(args.profile.clone())),
        proxy: event_loop.create_proxy(),
        window: None,
        webview: None,
        title: Arc::new(Mutex::new(String::new())),
        loaded: Arc::new(AtomicBool::new(false)),
        asked: false,
        started: Instant::now(),
        next_check: Instant::now(),
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
    proxy: EventLoopProxy<Msg>,
    window: Option<Window>,
    webview: Option<WebView>,
    title: Arc<Mutex<String>>,
    loaded: Arc<AtomicBool>,
    asked: bool,
    started: Instant,
    next_check: Instant,
    parent_gone: Arc<AtomicBool>,
    outcome: Option<i32>,
}

impl App {
    fn open(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title("khinsider")
            .with_inner_size(LogicalSize::new(800.0, 600.0))
            .with_position(PhysicalPosition::new(-32000, -32000))
            .with_decorations(false)
            .with_active(false)
            .with_skip_taskbar(true)
            .with_visible(true);
        let window = el.create_window(attrs).map_err(|e| e.to_string())?;
        let title = self.title.clone();
        let proxy = self.proxy.clone();
        let loaded = Arc::new(AtomicBool::new(false));
        let loaded2 = loaded.clone();
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_url(&self.args.url)
            .with_additional_browser_args(BROWSER_ARGS)
            .with_document_title_changed_handler(move |t| {
                log::info!("title: {t}");
                *title.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = t;
            })
            .with_on_page_load_handler(move |ev, url| {
                if matches!(ev, PageLoadEvent::Finished) {
                    log::info!("loaded {}", url.split(['?', '#']).next().unwrap_or(&url));
                    loaded2.store(true, Ordering::SeqCst);
                }
            })
            .with_ipc_handler(move |req| {
                let _ = proxy.send_event(Msg::Html(req.body().clone()));
            })
            .with_new_window_req_handler(|url, _| {
                log::info!("popup not opened: {}", url.split('?').next().unwrap_or_default());
                NewWindowResponse::Deny
            })
            .build(&window)
            .map_err(|e| e.to_string())?;
        self.window = Some(window);
        self.webview = Some(webview);
        self.loaded = loaded;
        log::info!("window open");
        Ok(())
    }

    fn finish(&mut self, el: &ActiveEventLoop, code: i32) {
        if self.outcome.is_none() {
            self.outcome = Some(code);
        }
        self.webview = None;
        self.window = None;
        el.exit();
    }

    fn write(&self, html: &str) -> bool {
        match std::fs::write(&self.args.out, html) {
            Ok(()) => true,
            Err(e) => {
                log::error!("cannot write {}: {e}", self.args.out.display());
                false
            }
        }
    }

    fn check(&mut self, el: &ActiveEventLoop) {
        if self.parent_gone.load(Ordering::SeqCst) {
            log::info!("the app asked to close");
            return self.finish(el, EXIT_TIMEOUT);
        }
        if self.started.elapsed() > self.args.timeout {
            log::info!("the check did not clear in {:?}", self.args.timeout);
            return self.finish(el, EXIT_TIMEOUT);
        }
        if !self.loaded.load(Ordering::SeqCst) || self.asked {
            return;
        }
        let title = self.title.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        if !title_cleared(&title) {
            return;
        }
        let Some(webview) = &self.webview else { return };
        self.asked = true;
        if let Err(e) = webview.evaluate_script(READ_HTML) {
            log::warn!("script: {e}");
            self.asked = false;
        }
    }

    fn html(&mut self, el: &ActiveEventLoop, html: String) {
        if html_is_check(&html) && self.started.elapsed() < self.args.timeout {
            self.asked = false;
            return;
        }
        if html.is_empty() {
            self.asked = false;
            return;
        }
        let code = if self.write(&html) { EXIT_OK } else { EXIT_ERROR };
        self.finish(el, code);
    }
}

impl ApplicationHandler<Msg> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() || self.outcome.is_some() {
            return;
        }
        el.set_control_flow(ControlFlow::Wait);
        if let Err(e) = self.open(el) {
            log::error!("cannot open the window: {e}");
            self.finish(el, EXIT_ERROR);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
            self.finish(el, EXIT_TIMEOUT);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, msg: Msg) {
        match msg {
            Msg::Html(html) => self.html(el, html),
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
