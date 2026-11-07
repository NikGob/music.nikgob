//! The host of SoundCloud's official player (`nikgob_sc_widget`): a child
//! run of the same exe, like the sign-in window (winit allows one event
//! loop per process and eframe owns the app's).
//!
//! ```text
//! nikgob.exe --sc-widget <profile dir>
//! ```
//!
//! One window placed far off-screen (a real, visible window: Chromium does
//! not throttle it, and it is kept out of the taskbar and never activated)
//! with a WebView2 webview showing `nikgob_sc_widget::PAGE` from its own
//! origin (http://scwidget.localhost/, a secure context, autoplay without a
//! gesture). Commands come one per line on stdin and go into the page as
//! `window.nk.*` calls, held until the page says it is up; what the page
//! posts goes to stdout as it is (`nikgob_sc_widget::protocol`). It logs to
//! stderr, which the app copies into its own log, and exits when its stdin
//! closes (the app closed it, or the app is gone).

use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;

use nikgob_sc_widget::protocol::{Command, Event, FLAG};

/// The host run, if this is one: its exit code.
pub fn child_main() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|a| a != FLAG) {
        return None;
    }
    let Some(profile) = args.next().map(PathBuf::from) else {
        log::error!("sc widget: no profile dir");
        return Some(1);
    };
    #[cfg(windows)]
    {
        Some(host::run(profile))
    }
    #[cfg(not(windows))]
    {
        let _ = profile;
        say(&Event::Fail("only on Windows".into()).line());
        Some(1)
    }
}

/// One line to the app.
fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// Reads the app's commands; `on_line` gets each, `on_end` once stdin closes.
fn read_stdin(on_line: impl Fn(Command) + Send + 'static, on_end: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name("stdin".into()).spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match Command::parse(&line) {
                Some(c) => on_line(c),
                None => log::warn!("not a command: {}", line.chars().take(80).collect::<String>()),
            }
        }
        on_end();
    });
}

#[cfg(windows)]
mod host {
    use std::borrow::Cow;
    use std::path::PathBuf;

    use winit::application::ApplicationHandler;
    use winit::dpi::{LogicalSize, PhysicalPosition};
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
    use winit::platform::windows::WindowAttributesExtWindows as _;
    use winit::window::{Window, WindowId};
    use wry::http::Response;
    use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows as _};

    use super::{Command, Event, read_stdin, say};

    const ORIGIN: &str = "http://scwidget.localhost/";
    /// wry's own defaults (they are replaced by any arguments given), plus
    /// no media keys: the keyboard's play/pause and the system's media
    /// flyout never reach the widget, only the app pauses it.
    const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,HardwareMediaKeyHandling \
                                --autoplay-policy=no-user-gesture-required";

    /// Keeps the off-screen window out of Alt+Tab and Task View (a tool
    /// window; the taskbar already leaves it out).
    #[allow(unsafe_code)]
    fn hide_from_switcher(window: &Window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            SetWindowLongPtrW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
        };
        use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
        let Ok(handle) = window.window_handle() else { return };
        let RawWindowHandle::Win32(h) = handle.as_raw() else { return };
        let hwnd = h.hwnd.get() as *mut core::ffi::c_void;
        // SAFETY: the window is ours and alive; only its extended style
        // changes, then its frame is refreshed without moving or activating.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let style = (style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    enum Msg {
        Command(Command),
        /// The page and SC.Widget's script are up.
        PageUp,
        /// stdin closed.
        Quit,
    }

    pub fn run(profile: PathBuf) -> i32 {
        match wry::webview_version() {
            Ok(v) => log::info!("WebView2 {v}"),
            Err(e) => {
                log::error!("no WebView2 runtime: {e}");
                say(&Event::Fail("no WebView2".into()).line());
                return 5;
            }
        }
        if let Err(e) = std::fs::create_dir_all(&profile) {
            log::error!("cannot create {}: {e}", profile.display());
            say(&Event::Fail("no profile dir".into()).line());
            return 1;
        }
        let event_loop = match EventLoop::<Msg>::with_user_event().build() {
            Ok(l) => l,
            Err(e) => {
                log::error!("no event loop: {e}");
                return 1;
            }
        };
        let proxy = event_loop.create_proxy();
        {
            let (lines, end) = (proxy.clone(), proxy.clone());
            read_stdin(
                move |c| {
                    let _ = lines.send_event(Msg::Command(c));
                },
                move || {
                    let _ = end.send_event(Msg::Quit);
                },
            );
        }
        let mut app = Host {
            context: WebContext::new(Some(profile)),
            proxy,
            window: None,
            webview: None,
            page_up: false,
            held: Vec::new(),
            failed: false,
        };
        if let Err(e) = event_loop.run_app(&mut app) {
            log::error!("event loop: {e}");
            return 1;
        }
        log::info!("exit");
        i32::from(app.failed)
    }

    struct Host {
        context: WebContext,
        proxy: EventLoopProxy<Msg>,
        window: Option<Window>,
        webview: Option<WebView>,
        page_up: bool,
        /// Commands that came before the page was up.
        held: Vec<Command>,
        failed: bool,
    }

    impl Host {
        fn open(&mut self, el: &ActiveEventLoop) -> Result<(), String> {
            let attrs = Window::default_attributes()
                .with_title("SoundCloud")
                .with_inner_size(LogicalSize::new(420.0, 240.0))
                .with_position(PhysicalPosition::new(-32000, -32000))
                .with_decorations(false)
                .with_active(false)
                .with_skip_taskbar(true)
                .with_visible(true);
            let window = el.create_window(attrs).map_err(|e| e.to_string())?;
            hide_from_switcher(&window);
            let up = self.proxy.clone();
            let webview = WebViewBuilder::new_with_web_context(&mut self.context)
                .with_custom_protocol("scwidget".into(), |_, _| {
                    Response::builder()
                        .header("Content-Type", "text/html; charset=utf-8")
                        .header("Cache-Control", "no-store")
                        .body(Cow::Borrowed(nikgob_sc_widget::PAGE.as_bytes()))
                        .unwrap_or_else(|_| Response::new(Cow::Borrowed(&b""[..])))
                })
                .with_url(ORIGIN)
                .with_autoplay(true)
                .with_additional_browser_args(BROWSER_ARGS)
                .with_ipc_handler(move |req| {
                    let body = req.body();
                    match Event::parse(body) {
                        Some(Event::Host) => {
                            let _ = up.send_event(Msg::PageUp);
                            say(body.trim());
                        }
                        Some(e) => say(&e.line()),
                        None => log::info!("page: {}", body.chars().take(160).collect::<String>()),
                    }
                })
                .with_on_page_load_handler(|ev, url| {
                    if matches!(ev, PageLoadEvent::Finished) {
                        log::info!("loaded {url}");
                    }
                })
                // The widget's links (SoundCloud's site, sign-in): never a window.
                .with_new_window_req_handler(|url, _| {
                    log::info!("popup not opened: {}", url.split('?').next().unwrap_or_default());
                    NewWindowResponse::Deny
                })
                .build(&window)
                .map_err(|e| e.to_string())?;
            self.window = Some(window);
            self.webview = Some(webview);
            Ok(())
        }

        fn run_js(&self, c: &Command) {
            if let Some(w) = &self.webview
                && let Err(e) = w.evaluate_script(&c.js())
            {
                log::warn!("script: {e}");
            }
        }

        fn quit(&mut self, el: &ActiveEventLoop) {
            // The webview first: it writes its profile as it goes away.
            self.webview = None;
            self.window = None;
            el.exit();
        }
    }

    impl ApplicationHandler<Msg> for Host {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            el.set_control_flow(ControlFlow::Wait);
            if let Err(e) = self.open(el) {
                log::error!("cannot open the player window: {e}");
                say(&Event::Fail("no player window".into()).line());
                self.failed = true;
                self.quit(el);
            }
        }

        fn user_event(&mut self, el: &ActiveEventLoop, msg: Msg) {
            match msg {
                Msg::Command(c) if self.page_up => self.run_js(&c),
                Msg::Command(c) => {
                    // A newer load replaces one not sent yet.
                    if matches!(c, Command::Load { .. }) {
                        self.held.clear();
                    }
                    self.held.push(c);
                }
                Msg::PageUp => {
                    // A reload of the page sends it again: what it had is gone.
                    self.page_up = true;
                    for c in std::mem::take(&mut self.held) {
                        self.run_js(&c);
                    }
                }
                Msg::Quit => {
                    log::info!("the app closed the player");
                    self.quit(el);
                }
            }
        }

        fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
                self.quit(el);
            }
        }
    }
}
