//! «Войти через Google»: the app's sign-in window (a fake [`WebLogin`]) ends
//! in the ordinary cookies login; closing it, cancelling and logging out.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_core::streaming::{Account, FakeService, LoginInput, LoginPrompt, Service, StreamService};
use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig, WebLogin, WebLoginError};
use nikgob_view::{Command, Prefs, ViewState};

/// Answers `answer` once `release` is set (or when cancelled).
struct FakeWindow {
    answer: Mutex<Result<String, WebLoginError>>,
    release: AtomicBool,
    saw_cancel: AtomicBool,
    forgot: AtomicUsize,
}

impl FakeWindow {
    fn new(answer: Result<String, WebLoginError>) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            release: AtomicBool::new(false),
            saw_cancel: AtomicBool::new(false),
            forgot: AtomicUsize::new(0),
        })
    }
}

impl WebLogin for FakeWindow {
    fn sign_in(&self, cancel: &AtomicBool) -> Result<String, WebLoginError> {
        while !self.release.load(Ordering::SeqCst) {
            if cancel.load(Ordering::SeqCst) {
                self.saw_cancel.store(true, Ordering::SeqCst);
                return Err(WebLoginError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.answer.lock().unwrap().clone()
    }
    fn forget(&self) {
        self.forgot.fetch_add(1, Ordering::SeqCst);
    }
}

fn wait(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(20), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn yt(s: &ViewState) -> Account {
    s.account(Service::Youtube).map(|a| a.account.clone()).unwrap_or_default()
}

struct Rig {
    rt: Runtime,
    dir: tempfile::TempDir,
}

fn rig(window: Arc<FakeWindow>) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let settings = dir.path().join("settings.toml");
    nikgob_core::settings::Settings::default().save(&settings).unwrap();
    let streams: Vec<Arc<dyn StreamService>> = vec![Arc::new(FakeService::youtube().logged_out())];
    let rt = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            library_dir: dir.path().join("Music"),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: Some(settings),
            import: None,
            streams,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.set_web_login(Service::Youtube, window);
    Rig { rt, dir }
}

fn login(rt: &Runtime) {
    rt.send(Command::StreamLogin { service: Service::Youtube, input: LoginInput::Browser });
}

#[test]
fn the_window_cookies_log_in_like_pasted_ones() {
    let cookies = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tv\n";
    let w = FakeWindow::new(Ok(cookies.into()));
    let r = rig(w.clone());
    login(&r.rt);
    wait("the window prompt", || matches!(yt(&r.rt.state()), Account::LoggingIn(Some(LoginPrompt::Window(_)))));
    w.release.store(true, Ordering::SeqCst);
    wait("logged in", || yt(&r.rt.state()).is_logged_in());
    wait("the cookies kept", || r.rt.state().prefs.youtube_cookies_set);
    let saved = nikgob_core::settings::Settings::load(&r.dir.path().join("settings.toml")).unwrap();
    assert_eq!(saved.streaming.youtube_cookies.as_deref(), Some(cookies.trim()));

    // «Выйти» forgets the window's session too.
    r.rt.send(Command::StreamLogout(Service::Youtube));
    wait("the profile forgotten", || w.forgot.load(Ordering::SeqCst) == 1);
    wait("logged out", || matches!(yt(&r.rt.state()), Account::LoggedOut));
}

#[test]
fn a_closed_or_refused_window_reads_failed() {
    for (err, says) in [(WebLoginError::Closed, "окно закрыто"), (WebLoginError::Refused, "cookies")] {
        let w = FakeWindow::new(Err(err));
        let r = rig(w.clone());
        login(&r.rt);
        wait("the window prompt", || matches!(yt(&r.rt.state()), Account::LoggingIn(_)));
        w.release.store(true, Ordering::SeqCst);
        wait("failed", || matches!(yt(&r.rt.state()), Account::Failed(m) if m.contains(says)));
        assert!(!r.rt.state().prefs.youtube_cookies_set);
        // Trying again starts a new window.
        *w.answer.lock().unwrap() = Ok(".youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tv".into());
        login(&r.rt);
        wait("logged in", || yt(&r.rt.state()).is_logged_in());
    }
}

#[test]
fn cancel_closes_the_window_and_keeps_its_session() {
    let w = FakeWindow::new(Ok("unused".into()));
    let r = rig(w.clone());
    login(&r.rt);
    wait("the window prompt", || matches!(yt(&r.rt.state()), Account::LoggingIn(_)));
    r.rt.send(Command::StreamLogout(Service::Youtube));
    wait("the window told", || w.saw_cancel.load(Ordering::SeqCst));
    wait("logged out", || matches!(yt(&r.rt.state()), Account::LoggedOut));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(w.forgot.load(Ordering::SeqCst), 0);
    assert!(matches!(yt(&r.rt.state()), Account::LoggedOut), "no failure after a cancel");
}
