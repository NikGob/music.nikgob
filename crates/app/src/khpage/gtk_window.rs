//! The child run on Linux: one off-screen WebKitGTK window (wry) that
//! reads a khinsider page once the check has cleared. No GTK, no display
//! or no WebKit: [`super::EXIT_NO_WEBVIEW`], and the source stays on HTTP.

use std::cell::Cell;
use std::io::Read as _;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use super::{ChildArgs, EXIT_ERROR, EXIT_NO_WEBVIEW, EXIT_OK, EXIT_TIMEOUT, READ_HTML, html_is_check, title_cleared};

const CHECK_EVERY: Duration = Duration::from_millis(500);

pub fn run(args: &ChildArgs) -> i32 {
    if let Err(e) = gtk::init() {
        log::error!("GTK cannot start: {e}");
        return EXIT_NO_WEBVIEW;
    }
    match wry::webview_version() {
        Ok(v) => log::info!("WebKitGTK {v}"),
        Err(e) => {
            log::error!("no WebKitGTK: {e}");
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

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("khinsider");
    window.set_decorated(false);
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);
    window.set_default_size(800, 600);
    window.move_(-32000, -32000);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&content);

    let title = Arc::new(Mutex::new(String::new()));
    let html: Rc<Mutex<Option<String>>> = Rc::new(Mutex::new(None));
    let mut context = WebContext::new(Some(args.profile.clone()));
    let title2 = title.clone();
    let html2 = html.clone();
    let webview = match WebViewBuilder::new_with_web_context(&mut context)
        .with_url(&args.url)
        .with_document_title_changed_handler(move |t| {
            log::info!("title: {t}");
            *title2.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = t;
        })
        .with_on_page_load_handler(|ev, url| {
            if matches!(ev, PageLoadEvent::Finished) {
                log::info!("loaded {}", url.split(['?', '#']).next().unwrap_or(&url));
            }
        })
        .with_ipc_handler(move |req| {
            *html2.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(req.body().clone());
        })
        .with_new_window_req_handler(|url, _| {
            log::info!("popup not opened: {}", url.split('?').next().unwrap_or_default());
            NewWindowResponse::Deny
        })
        .build_gtk(&content)
    {
        Ok(w) => w,
        Err(e) => {
            log::error!("cannot open the window: {e}");
            return EXIT_ERROR;
        }
    };
    window.show_all();
    log::info!("window open");

    let outcome: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let started = Instant::now();
    let asked = Rc::new(Cell::new(false));
    let webview = Rc::new(webview);
    let args = args.clone();
    glib::timeout_add_local(CHECK_EVERY, {
        let webview = webview.clone();
        let outcome = outcome.clone();
        let title = title.clone();
        let html = html.clone();
        let asked = asked.clone();
        move || {
            if outcome.get().is_some() {
                return glib::ControlFlow::Break;
            }
            if parent_gone.load(Ordering::SeqCst) || started.elapsed() > args.timeout {
                log::info!("the check did not clear");
                outcome.set(Some(EXIT_TIMEOUT));
                gtk::main_quit();
                return glib::ControlFlow::Break;
            }
            if let Some(body) = html.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
                if html_is_check(&body) && started.elapsed() < args.timeout {
                    asked.set(false);
                } else if body.is_empty() {
                    asked.set(false);
                } else {
                    let code = match std::fs::write(&args.out, &body) {
                        Ok(()) => EXIT_OK,
                        Err(e) => {
                            log::error!("cannot write {}: {e}", args.out.display());
                            EXIT_ERROR
                        }
                    };
                    outcome.set(Some(code));
                    gtk::main_quit();
                    return glib::ControlFlow::Break;
                }
            }
            let current = title.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            if title_cleared(&current) && !asked.get() {
                asked.set(true);
                if let Err(e) = webview.evaluate_script(READ_HTML) {
                    log::warn!("script: {e}");
                    asked.set(false);
                }
            }
            glib::ControlFlow::Continue
        }
    });
    gtk::main();
    drop(webview);
    window.hide();
    drop(context);
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
    let code = outcome.get().unwrap_or(EXIT_ERROR);
    log::info!("exit {code}");
    code
}
