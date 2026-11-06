//! Accounts: login / logout in the background, the account view, secrets
//! in settings.toml (never in prefs or logs).

use nikgob_core::streaming::{LoginInput, Service, StreamError};
use nikgob_view as v;

use super::{Msg, error_text};
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
        if accounts != self.streaming.last_accounts {
            self.streaming.last_accounts = accounts.clone();
            self.publish(|s| s.accounts = accounts);
        }
    }

    pub(crate) fn stream_login(&mut self, service: Service, input: LoginInput) {
        let Some(svc) = self.streams.iter().find(|s| s.service() == service).cloned() else {
            self.toast(format!("{} не подключён", service.display_name()), None);
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
                    LoginInput::Cookies(c) => self.save_stream_secrets(None, Some(c.clone()), false),
                    LoginInput::Browser | LoginInput::Playback => {}
                }
                self.login_toast(service, &input);
                self.pull_likes(service);
                self.discover_signed_in(service);
            }
            Err(e) => {
                log::warn!("{service}: login failed: {e}");
                // A login's own message (a busy port, a refused grant) is
                // already for the user; the rest reads as any service error.
                // Spotify without Premium is not connected at all.
                let text = match &e {
                    StreamError::Network(m) if !m.trim().is_empty() => m.clone(),
                    StreamError::PremiumRequired if service == Service::Spotify => {
                        nikgob_core::streaming::SPOTIFY_PREMIUM_REQUIRED.to_owned()
                    }
                    e => error_text(e),
                };
                self.streaming.errors.insert(service, text);
            }
        }
        self.publish_accounts();
        // Sound may have come (Premium) or gone: the next track resolves anew.
        self.preload_next();
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
        match (service, input) {
            (Service::Spotify, LoginInput::Playback) => self.toast("Звук из Spotify включён", None),
            (Service::Spotify, _) if without_sound => self.toast(
                "Spotify подключён · осталось включить звук",
                Some((
                    "Включить звук".into(),
                    nikgob_view::Command::StreamLogin { service, input: LoginInput::Playback },
                )),
            ),
            _ => self.toast(format!("{} подключён", service.display_name()), None),
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

    pub(crate) fn set_stream_secrets(&mut self, spotify: Option<String>, cookies: Option<String>) {
        self.save_stream_secrets(spotify, cookies, true);
    }

    /// `None` keeps a value, `Some("")` clears it.
    fn save_stream_secrets(&mut self, spotify: Option<String>, cookies: Option<String>, tell: bool) {
        let Some(path) = self.settings_path.clone() else {
            if tell {
                self.toast("Некуда сохранить настройки", None);
            }
            return;
        };
        let mut settings = match nikgob_core::settings::Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast("settings.toml не читается — поправь файл вручную", None);
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
            self.toast("Не удалось сохранить настройки", None);
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
            self.toast("Сохранено", None);
        }
    }
}
