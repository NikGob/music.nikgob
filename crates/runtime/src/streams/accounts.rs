//! Accounts: login / logout in the background, the account view, secrets
//! in settings.toml (never in prefs or logs).

use nikgob_core::streaming::{LoginInput, Service, StreamError};
use nikgob_view as v;

use super::{Msg, error_text_in};
use crate::Worker;

/// Unix ms of `at`, rounded up to the whole minute (stable between ticks).
fn minute_up(at: std::time::SystemTime) -> i64 {
    let ms = at
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    (ms + 59_999) / 60_000 * 60_000
}

/// «14:05»: unix ms as the local time of day.
fn local_hm(ms: i64) -> Option<String> {
    let at = jiff::Timestamp::from_millisecond(ms).ok()?;
    Some(at.to_zoned(jiff::tz::TimeZone::system()).strftime("%H:%M").to_string())
}

/// «14:05»: the local minute (rounded up) this far from now.
pub(crate) fn hm_after(left: std::time::Duration) -> Option<String> {
    local_hm(minute_up(std::time::SystemTime::now() + left))
}

impl Worker {
    /// Refreshes [`v::ViewState::accounts`] (published only when changed).
    pub(crate) fn publish_accounts(&mut self) {
        self.suggest_load();
        let accounts: Vec<v::AccountView> = Service::ALL
            .iter()
            .map(|&service| {
                let registered = self.streams.iter().find(|s| s.service() == service);
                let enabled = self.state.read().prefs.service_enabled(service);
                let account = self.web_account(service, registered.map(|s| s.account()).unwrap_or_default());
                let (suggestions, suggestions_note) = self.suggest_lines(service);
                v::AccountView {
                    service,
                    registered: registered.is_some(),
                    enabled,
                    audio: registered.is_some() && enabled && self.has_audio(service),
                    audio_refused_until_hm: registered
                        .filter(|_| enabled)
                        .and_then(|s| s.audio_refused_until())
                        .map(minute_up)
                        .and_then(local_hm),
                    usable: registered.is_some() && enabled && super::usable(service, &account),
                    account,
                    busy: self.streaming.busy.contains(&service),
                    error: self.streaming.errors.get(&service).cloned(),
                    paused_until: registered.and_then(|s| s.paused_until()).map(minute_up),
                    paused_until_hm: registered.and_then(|s| s.paused_until()).map(minute_up).and_then(local_hm),
                    suggestions,
                    suggestions_note,
                    soloist: registered.and_then(|s| s.soloist()),
                }
            })
            .collect();
        // SoundCloud refused the sign-in: the window's saved cookie is as
        // dead as the token, so «Войти» must not hand it back.
        let refused = self
            .streams
            .iter()
            .find(|s| s.service() == Service::Soundcloud)
            .is_some_and(|s| matches!(s.account(), nikgob_core::streaming::Account::Failed(_)));
        if refused != self.streaming.sc_refused {
            self.streaming.sc_refused = refused;
            if refused {
                self.forget_web_login(Service::Soundcloud);
            }
        }
        if accounts != self.streaming.last_accounts {
            self.streaming.last_accounts = accounts.clone();
            self.publish(|s| s.accounts = accounts);
        }
    }

    pub(crate) fn stream_login(&mut self, service: Service, input: LoginInput) {
        let Some(svc) = self.streams.iter().find(|s| s.service() == service).cloned() else {
            let msg = nikgob_core::i18n::ErrorStrings::service_not_connected_in(self.locale(), service.display_name());
            self.toast_err(msg, None);
            return;
        };
        if !matches!(self.streaming.web_state.get(&service), Some(super::web_login::WebState::Running(_))) {
            self.streaming.web_state.remove(&service);
        }
        // YouTube Music: the app's Google sign-in window, then its cookies.
        if input == LoginInput::Browser && self.start_web_login(service) {
            return;
        }
        if !self.streaming.busy.insert(service) {
            return;
        }
        self.streaming.errors.remove(&service);
        log::info!("{service}: login ({input:?})");
        let again = input.clone();
        self.streaming
            .spawn(async move { Msg::LoginDone(service, again, svc.login(input).await) });
        self.publish_accounts();
    }

    pub(crate) fn login_done(&mut self, service: Service, input: LoginInput, r: Result<(), StreamError>) {
        self.streaming.busy.remove(&service);
        match r {
            Ok(()) => {
                log::info!("{service}: logged in");
                // What the user typed is kept for the next start.
                match &input {
                    LoginInput::PersonalApp { client_id } => {
                        self.save_stream_secrets(Some(client_id.clone()), None, false);
                    }
                    // SoundCloud keeps its token in the credential store itself.
                    LoginInput::Cookies(c) if service == Service::Youtube => {
                        self.save_stream_secrets(None, Some(c.clone()), false);
                    }
                    LoginInput::Cookies(_) => {}
                    LoginInput::Browser | LoginInput::Playback => {}
                }
                self.switch_on(service);
                self.login_toast(service, &input);
                self.pull_likes(service);
                self.discover_signed_in(service);
            }
            Err(e) => {
                log::warn!("{service}: login failed: {e}");
                // A login's own message (a busy port, a refused grant) is
                // already for the user; the rest reads as any service error.
                // Spotify without Premium is not connected at all.
                let loc = self.locale();
                let text = match &e {
                    StreamError::Network(m) if !m.trim().is_empty() => m.clone(),
                    StreamError::PremiumRequired if service == Service::Spotify => {
                        nikgob_core::streaming::premium_required_in(loc).to_owned()
                    }
                    e => error_text_in(e, loc),
                };
                self.streaming.errors.insert(service, text);
            }
        }
        self.publish_accounts();
        // Sound may have come (Premium) or gone: the next track resolves anew.
        self.preload_next();
    }

    /// A service just connected is switched on (a fresh install ships
    /// Spotify off): what the user connected is what plays and searches.
    fn switch_on(&mut self, service: Service) {
        let mut prefs = self.state.read().prefs.clone();
        if prefs.service_enabled(service) {
            return;
        }
        log::info!("{service}: switched on by its login");
        match service {
            Service::Spotify => prefs.spotify_enabled = true,
            Service::Youtube => prefs.youtube_enabled = true,
            Service::Deezer => prefs.deezer_enabled = true,
            Service::Soundcloud => prefs.soundcloud_enabled = true,
        }
        self.set_prefs(Box::new(prefs));
    }

    /// What a finished login says. A Spotify account without the playback
    /// login yet is offered the second step right away (a login without
    /// Premium does not get here: it is not connected).
    fn login_toast(&mut self, service: Service, input: &LoginInput) {
        let account = self.streams.iter().find(|s| s.service() == service).map(|s| s.account());
        let without_sound = matches!(
            account,
            Some(nikgob_core::streaming::Account::LoggedIn { premium: Some(true) | None, audio: false, .. })
        );
        let loc = self.locale();
        let err = nikgob_core::i18n::errors_in(loc);
        match (service, input) {
            (Service::Spotify, LoginInput::Playback) => self.toast_ok(err.spotify_audio_enabled, None),
            (Service::Spotify, _) if without_sound => self.toast_ok(
                err.spotify_connected_enable_audio,
                Some((
                    err.enable_audio.into(),
                    nikgob_view::Command::StreamLogin { service, input: LoginInput::Playback },
                )),
            ),
            _ => self.toast_ok(nikgob_core::i18n::ErrorStrings::service_connected_in(loc, service.display_name()), None),
        }
    }

    pub(crate) fn stream_logout(&mut self, service: Service) {
        // «Отмена» of an open sign-in window: it closes, its saved session stays.
        if self.cancel_web_login(service) {
            log::info!("{service}: sign-in window cancelled");
            self.publish_accounts();
            return;
        }
        if let Some(s) = self.streams.iter().find(|s| s.service() == service) {
            s.logout();
        }
        self.streaming.web_state.remove(&service);
        self.forget_web_login(service);
        self.streaming.errors.remove(&service);
        self.publish_accounts();
        self.preload_next();
    }

    /// [`v::Command::RetryStreamAudio`]: the service is asked for sound
    /// again at once; what plays next may come from it.
    pub(crate) fn retry_stream_audio(&mut self, service: Service) {
        if let Some(s) = self.streams.iter().find(|s| s.service() == service) {
            s.retry_audio();
        }
        self.publish_accounts();
        self.preload_next();
    }

    pub(crate) fn set_stream_secrets(&mut self, spotify: Option<String>, cookies: Option<String>) {
        self.save_stream_secrets(spotify, cookies, true);
    }

    /// `None` keeps a value, `Some("")` clears it.
    fn save_stream_secrets(&mut self, spotify: Option<String>, cookies: Option<String>, tell: bool) {
        let loc = self.locale();
        let err = nikgob_core::i18n::errors_in(loc);
        let Some(path) = self.settings_path.clone() else {
            if tell {
                self.toast_err(err.no_settings_path, None);
            }
            return;
        };
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast_err(err.settings_unreadable, None);
                return;
            }
        };
        let clean = |v: String| Some(v.trim().to_owned()).filter(|v| !v.is_empty());
        if let Some(id) = spotify {
            settings.streaming.spotify_client_id = clean(id);
        }
        if let Some(c) = cookies {
            settings.streaming.youtube_cookies = clean(c);
        }
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast_failed_save_prefs();
            return;
        }
        let (personal, cookies) = (
            settings.streaming.spotify_client_id.is_some(),
            settings.streaming.youtube_cookies.is_some(),
        );
        self.publish(|s| {
            s.prefs.spotify_personal_app = personal;
            s.prefs.youtube_cookies_set = cookies;
        });
        if tell {
            self.toast_ok(err.saved, None);
        }
    }
}
