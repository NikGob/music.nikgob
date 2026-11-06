//! Spike: play a SoundCloud track through the official embed widget in a
//! WebView2 window placed off-screen, and log what happens.
//!
//! `sc-widget-probe <track permalink> [seconds] [profile dir]`
//!
//! The page (`page.html`, served as http://probe.localhost/) embeds
//! `https://w.soundcloud.com/player/` and drives it with SC.Widget; its script
//! reports EME support and the widget's events to stdout through IPC. The
//! widget volume is set to 5 so the test is nearly silent.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};
use wry::http::Response;
use wry::{WebContext, WebView, WebViewBuilder};

const PAGE: &str = include_str!("page.html");

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(url) = args.next() else {
        eprintln!("usage: sc-widget-probe <track permalink> [seconds] [profile dir]");
        std::process::exit(2);
    };
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20);
    let profile = args.next().map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("sc-widget-probe"));
    println!("pid {}", std::process::id());
    match wry::webview_version() {
        Ok(v) => println!("WebView2 {v}"),
        Err(e) => {
            println!("no WebView2: {e}");
            std::process::exit(1);
        }
    }
    let page = PAGE.replace("__URL__", &url.replace('&', "%26"));
    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App {
        page,
        deadline: Instant::now() + Duration::from_secs(secs + 8),
        context: WebContext::new(Some(profile)),
        window: None,
        webview: None,
    };
    event_loop.run_app(&mut app).expect("run");
}

struct App {
    page: String,
    deadline: Instant,
    context: WebContext,
    window: Option<Window>,
    webview: Option<WebView>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500)));
        let attrs = Window::default_attributes()
            .with_title("sc-widget-probe")
            .with_inner_size(LogicalSize::new(420.0, 240.0))
            .with_position(PhysicalPosition::new(-32000, -32000))
            .with_visible(true);
        let window = el.create_window(attrs).expect("window");
        let page = self.page.clone();
        let webview = WebViewBuilder::new_with_web_context(&mut self.context)
            .with_custom_protocol("probe".into(), move |_, _| {
                Response::builder()
                    .header("Content-Type", "text/html; charset=utf-8")
                    .body(page.clone().into_bytes().into())
                    .expect("response")
            })
            .with_url("http://probe.localhost/")
            .with_ipc_handler(|req| println!("JS {}", req.body()))
            .with_new_window_req_handler(|u, _| {
                println!("popup denied {u}");
                wry::NewWindowResponse::Deny
            })
            .build(&window)
            .expect("webview");
        self.window = Some(window);
        self.webview = Some(webview);
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, ev: WindowEvent) {
        if matches!(ev, WindowEvent::CloseRequested) {
            el.exit();
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if Instant::now() >= self.deadline {
            println!("done");
            el.exit();
        } else {
            el.set_control_flow(ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(500)));
        }
    }
}
