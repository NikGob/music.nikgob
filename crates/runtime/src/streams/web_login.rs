//! Signing in through a window the app opens itself (YouTube Music: Google's
//! own sign-in page in a WebView2 window, see the app's `ytlogin`;
//! SoundCloud: soundcloud.com/signin, whose `oauth_token` cookie is the
//! sign-in).
//!
//! The runtime has no GUI: the app hands it a [`WebLogin`] per service
//! ([`crate::Runtime::set_web_login`]). A [`LoginInput::Browser`] login of
//! such a service runs [`WebLogin::sign_in`] on a thread of its own; while it
//! runs the account reads [`Account::LoggingIn`] with a
//! [`LoginPrompt::Window`]. The cookies it returns go through the ordinary
//! [`LoginInput::Cookies`] login, so they are kept exactly like pasted ones
//! (the service's cache and settings.toml). A window closed early, a timeout
//! or a refusal read [`Account::Failed`] with what to do next; «Отмена» (a
//! logout while it runs) just stops it. A logout otherwise also
//! [`WebLogin::forget`]s the window's saved Google session.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use nikgob_core::i18n::activity::{fmt_login_window_failed_in, fmt_sc_window_failed_in};
use nikgob_core::i18n::{Locale, activity_in};
use nikgob_core::streaming::{Account, LoginInput, LoginPrompt, Service};

use super::Msg;
use crate::Worker;
use crate::import::Internal;

/// What the window shows next to the account while it is open, in Russian:
/// kept for callers outside the runtime, which should use [`window_hint_in`].
pub const WINDOW_HINT: &str = "Войди в аккаунт Google в открывшемся окне. Пароль вводится только на странице Google — приложение его не видит.";
/// The same for SoundCloud's window, in Russian: see [`WINDOW_HINT`].
pub const SOUNDCLOUD_WINDOW_HINT: &str = "Войди в SoundCloud в открывшемся окне. Пароль вводится только на странице SoundCloud — приложение его не видит.";

/// The hint of a service's window, in `loc`.
pub fn window_hint_in(service: Service, loc: Locale) -> &'static str {
    let s = activity_in(loc);
    match service {
        Service::Soundcloud => s.soundcloud_window_hint,
        _ => s.window_hint,
    }
}

/// The hint of a service's window, in Russian.
pub fn window_hint(service: Service) -> &'static str {
    window_hint_in(service, Locale::Ru)
}

/// A sign-in window of the app.
pub trait WebLogin: Send + Sync {
    /// Opens the window and blocks until it is done: the session cookies as
    /// a Netscape cookies.txt. Stops soon after `cancel` becomes true. `loc`
    /// is the UI's language of this sign-in (the window's title and its
    /// errors are in it), not the one the app started with.
    fn sign_in(&self, cancel: &AtomicBool, loc: Locale) -> Result<String, WebLoginError>;
    /// Forgets the window's saved session (its browser profile). Blocking.
    fn forget(&self);
}

/// Why [`WebLogin::sign_in`] gave no cookies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebLoginError {
    /// The window was closed before the sign-in finished.
    Closed,
    /// The sign-in did not finish in time.
    TimedOut,
    /// The provider would not sign in inside an embedded browser.
    Refused,
    /// `sign_in` was cancelled.
    Cancelled,
    /// The window could not run (no WebView2, ...).
    Failed(String),
}

impl WebLoginError {
    /// As the user reads it, for a service's window, in `loc`.
    pub fn text_for_in(&self, service: Service, loc: Locale) -> String {
        if service != Service::Soundcloud {
            return self.text_in(loc);
        }
        match self {
            Self::Refused => activity_in(loc).sc_refused.to_owned(),
            Self::Failed(m) => fmt_sc_window_failed_in(loc, m),
            other => other.text_in(loc),
        }
    }

    /// As the user reads it, for a service's window, in Russian.
    pub fn text_for(&self, service: Service) -> String {
        self.text_for_in(service, Locale::Ru)
    }

    /// As the user reads it, in `loc`.
    pub fn text_in(&self, loc: Locale) -> String {
        let s = activity_in(loc);
        match self {
            Self::Closed => s.login_closed.to_owned(),
            Self::TimedOut => s.login_timed_out.to_owned(),
            Self::Refused => s.login_refused.to_owned(),
            Self::Cancelled => s.login_cancelled.to_owned(),
            Self::Failed(m) => fmt_login_window_failed_in(loc, m),
        }
    }

    /// As the user reads it, in Russian.
    pub fn text(&self) -> String {
        self.text_in(Locale::Ru)
    }
}

/// A web login of one service.
pub(crate) enum WebState {
    Running(Arc<AtomicBool>),
    /// Why it gave no cookies, as data: its words are made when the account
    /// is shown.
    Failed(WebLoginError),
}

impl Worker {
    pub(crate) fn set_web_login(&mut self, service: Service, login: Arc<dyn WebLogin>) {
        self.streaming.web.insert(service, login);
    }

    /// The account as the view shows it: a running or failed web login
    /// wins over what the service says (it knows nothing of the window).
    pub(crate) fn web_account(&self, service: Service, account: Account) -> Account {
        match self.streaming.web_state.get(&service) {
            Some(WebState::Running(_)) => {
                Account::LoggingIn(Some(LoginPrompt::Window(window_hint_in(service, self.locale()).into())))
            }
            Some(WebState::Failed(e)) if !account.is_logged_in() => Account::Failed(e.text_for_in(service, self.locale())),
            _ => account,
        }
    }

    /// A [`LoginInput::Browser`] login of a service with a [`WebLogin`].
    /// Returns false when the service has none.
    pub(crate) fn start_web_login(&mut self, service: Service) -> bool {
        let Some(login) = self.streaming.web.get(&service).cloned() else {
            return false;
        };
        if matches!(self.streaming.web_state.get(&service), Some(WebState::Running(_)))
            || self.streaming.busy.contains(&service)
        {
            return true;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.streaming.web_state.insert(service, WebState::Running(cancel.clone()));
        self.streaming.errors.remove(&service);
        log::info!("{service}: login in the app's window");
        let tx = self.streaming.internal.clone();
        let loc = self.locale();
        let spawned = std::thread::Builder::new()
            .name(format!("{service}-web-login"))
            .spawn(move || {
                let r = login.sign_in(&cancel, loc);
                let _ = tx.send(Internal::Stream(Msg::WebLoginDone(service, r)));
            });
        if let Err(e) = spawned {
            self.streaming.web_state.insert(service, WebState::Failed(WebLoginError::Failed(e.to_string())));
        }
        self.publish_accounts();
        true
    }

    pub(crate) fn web_login_done(&mut self, service: Service, r: Result<String, WebLoginError>) {
        // Cancelled from the app meanwhile: nothing to report.
        if !matches!(self.streaming.web_state.remove(&service), Some(WebState::Running(_))) {
            self.publish_accounts();
            return;
        }
        match r {
            Ok(cookies) => {
                log::info!("{service}: the sign-in window gave the session cookies");
                self.stream_login(service, LoginInput::Cookies(cookies));
            }
            Err(WebLoginError::Cancelled) => {}
            Err(e) => {
                log::warn!("{service}: sign-in window: {e:?}");
                self.streaming.web_state.insert(service, WebState::Failed(e));
            }
        }
        self.publish_accounts();
    }

    /// Stops a running web login; true if one ran.
    pub(crate) fn cancel_web_login(&mut self, service: Service) -> bool {
        match self.streaming.web_state.remove(&service) {
            Some(WebState::Running(cancel)) => {
                cancel.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }

    /// Forgets the window's saved session, off the worker.
    pub(crate) fn forget_web_login(&self, service: Service) {
        if let Some(login) = self.streaming.web.get(&service).cloned() {
            let _ = std::thread::Builder::new()
                .name(format!("{service}-web-forget"))
                .spawn(move || login.forget());
        }
    }
}
