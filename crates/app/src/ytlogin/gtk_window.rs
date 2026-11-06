//! The child run on Linux: one GTK window with a WebKitGTK webview (wry)
//! showing Google's sign-in (see the parent module). wry's WebKitGTK webview
//! lives in GTK's own main loop rather than winit's, so the window, the
//! periodic check and the close button are GTK's too; the flow and the exit
//! codes are those of the Windows window.

use std::cell::Cell;
use std::io::Read as _;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
use gtk::glib;
use gtk::prelude::*;
use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix as _};

use super::{
    ChildArgs, Cookie, EXIT_CLOSED, EXIT_ERROR, EXIT_NO_WEBVIEW, EXIT_OK, EXIT_REFUSED, EXIT_TIMEOUT,
    START_URL, netscape, redact, refused, signed_in, write_secret,
};

const TITLE: &str = "Вход в YouTube Music";
/// How often the page and the cookie store are looked at.
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// Where the session cookies are read from.
const COOKIE_URLS: [&str; 2] = ["https://music.youtube.com/", "https://www.youtube.com/"];
/// The window icon's side, in pixels.
const ICON: usize = 64;

pub fn run(args: &ChildArgs) -> i32 {
    // No display (or no GTK at all) is as good as no webview: the parent
    // tells the user the window cannot be shown.
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
    // The profile holds the session cookies: the owner's alone.
    if let Err(e) = private_dir(&args.profile) {
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
    window.set_title(TITLE);
    window.set_default_size(520, 720);
    window.set_size_request(380, 480);
    window.set_position(gtk::WindowPosition::Center);
    let rgba = glib::Bytes::from_owned(crate::desktop::icon_rgba(ICON));
    let side = ICON as i32;
    window.set_icon(Some(&Pixbuf::from_bytes(&rgba, Colorspace::Rgb, true, 8, side, side, side * 4)));
    // wry fills a box with its webview.
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    window.add(&content);

    let mut context = WebContext::new(Some(args.profile.clone()));
    let webview = match WebViewBuilder::new_with_web_context(&mut context)
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
        .build_gtk(&content)
    {
        Ok(w) => w,
        Err(e) => {
            log::error!("cannot open the window: {e}");
            return EXIT_ERROR;
        }
    };
    window.show_all();
    window.present();
    log::info!("window open at {}", redact(START_URL));

    let outcome: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));
    let finish = {
        let outcome = outcome.clone();
        move |code: i32| {
            if outcome.get().is_none() {
                outcome.set(Some(code));
                gtk::main_quit();
            }
        }
    };
    window.connect_delete_event({
        let finish = finish.clone();
        move |_, _| {
            log::info!("window closed");
            finish(EXIT_CLOSED);
            glib::Propagation::Stop
        }
    });

    let webview = Rc::new(webview);
    let mut state = Check {
        outcome: outcome.clone(),
        args: args.clone(),
        started: Instant::now(),
        last_url: String::new(),
        parent_gone,
    };
    glib::timeout_add_local(CHECK_EVERY, {
        let webview = webview.clone();
        let outcome = outcome.clone();
        move || {
            if outcome.get().is_some() {
                return glib::ControlFlow::Break;
            }
            match state.check(&webview) {
                Some(code) => {
                    finish(code);
                    glib::ControlFlow::Break
                }
                None => glib::ControlFlow::Continue,
            }
        }
    });
    gtk::main();

    // The webview first: it writes its profile as it goes away.
    drop(webview);
    window.hide();
    drop(context);
    // Let WebKit's last writes (the cookie file) land before the process ends.
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
    let code = outcome.get().unwrap_or(EXIT_ERROR);
    log::info!("exit {code}");
    code
}

/// Creates `dir` (and its missing parents) readable by the owner only, and
/// takes the group and others away from one that already exists.
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// What the periodic check looks at.
struct Check {
    /// Set when the window ended some other way (the close button), also
    /// while this check waits for the cookie store.
    outcome: Rc<Cell<Option<i32>>>,
    args: ChildArgs,
    started: Instant,
    last_url: String,
    parent_gone: Arc<AtomicBool>,
}

impl Check {
    /// The exit code once the window is done, `None` while it waits.
    fn check(&mut self, webview: &WebView) -> Option<i32> {
        if self.parent_gone.load(Ordering::SeqCst) {
            log::info!("the app asked to close");
            return Some(EXIT_CLOSED);
        }
        if self.started.elapsed() > self.args.timeout {
            log::info!("no sign-in in {:?}", self.args.timeout);
            return Some(EXIT_TIMEOUT);
        }
        let url = webview.url().unwrap_or_default();
        if url != self.last_url {
            log::info!("at {}", redact(&url));
            self.last_url.clone_from(&url);
        }
        if refused(&url) {
            log::warn!("Google refused the sign-in in this window");
            return Some(EXIT_REFUSED);
        }
        if super::host(&url) != "music.youtube.com" {
            return None;
        }
        let mut cookies = Vec::new();
        for u in COOKIE_URLS {
            match webview.cookies_for_url(u) {
                Ok(list) => {
                    let read_for = super::host(u);
                    cookies.extend(list.iter().map(|c| convert(c, &read_for)));
                }
                Err(e) => log::warn!("cookies of {u}: {e}"),
            }
        }
        // Reading the cookies runs GTK's loop: the window may have been
        // closed meanwhile, and a closed window leaves no cookie file behind.
        if self.outcome.get().is_some() {
            return None;
        }
        if !signed_in(&url, &cookies) {
            return None;
        }
        log::info!("signed in: {} cookies", cookies.len());
        Some(match write_secret(&self.args.out, &netscape(&cookies)) {
            Ok(()) => EXIT_OK,
            Err(e) => {
                log::error!("cannot write {}: {e}", self.args.out.display());
                EXIT_ERROR
            }
        })
    }
}

fn convert(c: &wry::cookie::Cookie<'_>, read_for_host: &str) -> Cookie {
    Cookie {
        domain: super::cookie_domain(c.domain().unwrap_or_default(), read_for_host),
        path: c.path().unwrap_or("/").to_owned(),
        name: c.name().to_owned(),
        value: c.value().to_owned(),
        secure: c.secure().unwrap_or(false),
        http_only: c.http_only().unwrap_or(false),
        expires: c.expires_datetime().map(|t| t.unix_timestamp()),
    }
}
