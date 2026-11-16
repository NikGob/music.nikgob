//! The local REST API's settings (`[api]` of settings.toml), kept and
//! applied by the loop and never by the API's own thread, and what the
//! API server reports back: where it listens, the tokens it saw used.
//!
//! A revoked token is refused at once (the server follows
//! [`nikgob_view::ApiView`]) and stays in the file, marked, while its
//! snackbar's «Отменить» is up; then it goes for good. The last use of a
//! token is shown at once and written rarely.

use std::time::{Duration, Instant};

use nikgob_core::settings::{ApiSettings, ApiTokenRecord, Settings};
use nikgob_view::{ApiTokenView, ApiView, Command};

use crate::Worker;

/// The tokens' last uses are written to settings.toml at most this often
/// (and when the player quits).
const USED_SAVE_EVERY: Duration = Duration::from_secs(10 * 60);

/// A revoked token is removed for good no sooner than this after the
/// revoke (its snackbar gone).
const REVOKED_KEPT_MS: i64 = 5_000;

/// What the loop keeps of the API between turns.
#[derive(Default)]
pub(crate) struct ApiKeep {
    /// Uses of the tokens shown but not written yet.
    used_unsaved: bool,
    used_saved_at: Option<Instant>,
}

fn token_view(t: &ApiTokenRecord) -> ApiTokenView {
    ApiTokenView {
        id: t.id.clone(),
        name: t.name.clone(),
        preset: t.preset,
        actor: t.actor,
        created_ms: t.created_ms,
        last_used_ms: t.last_used_ms,
        revoked: t.revoked_ms.is_some(),
        hash: t.hash.clone(),
    }
}

/// `api` of settings.toml into the view; where the server listens (its
/// own report) and the sync point stay.
fn apply(view: &mut ApiView, api: &ApiSettings, configurable: bool) {
    view.enabled = api.enabled;
    view.access = api.access;
    view.address = api.address.trim().to_owned();
    view.port = api.port;
    view.tokens = api.tokens.iter().map(token_view).collect();
    view.configurable = configurable;
}

/// The revoked tokens that may go for good at `now` (unix ms): their
/// snackbar is gone (`undo_up` says whose is up) and they were revoked at
/// least [`REVOKED_KEPT_MS`] ago.
pub(crate) fn purgeable(tokens: &[ApiTokenRecord], now: i64, undo_up: impl Fn(&str) -> bool) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| t.revoked_ms.is_some_and(|at| now - at >= REVOKED_KEPT_MS) && !undo_up(&t.id))
        .map(|t| t.id.clone())
        .collect()
}

impl Worker {
    /// `[api]` into the view at start. Tokens revoked in a run that ended
    /// lost their «Отменить» with it: they go now.
    pub(crate) fn api_load(&mut self) {
        let Some(path) = self.settings_path.clone() else { return };
        let mut settings = match Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("api: settings.toml not read: {e}");
                return;
            }
        };
        let before = settings.api.tokens.len();
        settings.api.tokens.retain(|t| t.revoked_ms.is_none());
        if settings.api.tokens.len() != before
            && let Err(e) = settings.save(&path)
        {
            log::warn!("api: revoked tokens not removed: {e}");
        }
        let api = settings.api;
        self.publish(move |s| apply(&mut s.api, &api, true));
    }

    /// Changes `[api]` in settings.toml (`edit` says whether it changed
    /// anything) with the tokens' last uses as shown, and shows the result.
    /// False when nothing was saved.
    fn api_edit(&mut self, edit: impl FnOnce(&mut ApiSettings) -> bool) -> bool {
        let Some(path) = self.settings_path.clone() else {
            self.toast("Некуда сохранить настройки", None);
            return false;
        };
        // A file that cannot be read is not replaced by defaults: that would
        // drop every other secret in it.
        let mut settings = match Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                self.toast("settings.toml не читается — поправь файл вручную", None);
                return false;
            }
        };
        {
            let st = self.state.read();
            for t in &mut settings.api.tokens {
                if let Some(v) = st.api.tokens.iter().find(|v| v.id == t.id) {
                    t.last_used_ms = t.last_used_ms.max(v.last_used_ms);
                }
            }
        }
        if !edit(&mut settings.api) {
            return false;
        }
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            self.toast("Не удалось сохранить настройки", None);
            return false;
        }
        self.api.used_unsaved = false;
        self.api.used_saved_at = Some(Instant::now());
        let api = settings.api;
        self.publish(move |s| apply(&mut s.api, &api, true));
        true
    }

    pub(crate) fn api_command(&mut self, c: Command) {
        match c {
            Command::SetApiEnabled(on) => {
                self.api_edit(|a| {
                    a.enabled = on;
                    true
                });
            }
            Command::SetApiAccess(access) => {
                self.api_edit(|a| {
                    // An explicit address of the file stands for «Домашняя
                    // сеть»; «Этот компьютер» takes it away.
                    if access == nikgob_view::ApiAccess::Local {
                        a.address.clear();
                    }
                    a.access = access;
                    true
                });
            }
            Command::SetApiPort(port) => {
                if port < 1024 {
                    log::warn!("api: port {port} refused");
                    return;
                }
                self.api_edit(|a| {
                    a.port = port;
                    true
                });
            }
            Command::ApiCreateToken(t) => {
                if t.name.trim().is_empty() || nikgob_core::api_token::Hashed::parse(&t.hash).is_none() {
                    log::warn!("api: a token without a name or a hash refused");
                    return;
                }
                let record = ApiTokenRecord {
                    id: t.id,
                    name: t.name.trim().to_owned(),
                    preset: t.preset,
                    actor: t.actor,
                    hash: t.hash,
                    created_ms: nikgob_library::now_ms(),
                    last_used_ms: None,
                    revoked_ms: None,
                };
                self.api_edit(move |a| {
                    if a.tokens.iter().any(|x| x.id == record.id) {
                        return false;
                    }
                    a.tokens.push(record);
                    true
                });
            }
            Command::ApiRevokeToken(id) => {
                let name = self.state.read().api.tokens.iter().find(|t| t.id == id).map(|t| t.name.clone());
                let Some(name) = name else { return };
                let at = nikgob_library::now_ms();
                let which = id.clone();
                let revoked = self.api_edit(move |a| match a.tokens.iter_mut().find(|t| t.id == which && t.revoked_ms.is_none()) {
                    Some(t) => {
                        t.revoked_ms = Some(at);
                        true
                    }
                    None => false,
                });
                if revoked {
                    self.toast(format!("Токен «{name}» отозван"), Some(("Отменить".to_owned(), Command::ApiRestoreToken(id))));
                }
            }
            Command::ApiRestoreToken(id) => {
                self.api_edit(move |a| match a.tokens.iter_mut().find(|t| t.id == id && t.revoked_ms.is_some()) {
                    Some(t) => {
                        t.revoked_ms = None;
                        true
                    }
                    None => false,
                });
            }
            Command::ApiStatus { listening, error } => {
                self.publish(move |s| {
                    s.api.listening = listening;
                    s.api.error = error;
                });
            }
            Command::ApiTokensUsed(used) => {
                self.publish(move |s| {
                    for (id, at) in &used {
                        if let Some(t) = s.api.tokens.iter_mut().find(|t| &t.id == id) {
                            t.last_used_ms = t.last_used_ms.max(Some(*at));
                        }
                    }
                });
                self.api.used_unsaved = true;
            }
            Command::ApiSync(n) => self.publish(move |s| s.api.synced = s.api.synced.max(n)),
            other => log::warn!("api: not an API command {other:?}"),
        }
    }

    /// The loop's tick: revoked tokens whose «Отменить» is gone go for
    /// good; the tokens' last uses are written now and then.
    pub(crate) fn api_tick(&mut self) {
        let now = nikgob_library::now_ms();
        let revoked: Vec<(String, bool)> = {
            let st = self.state.read();
            st.api
                .tokens
                .iter()
                .filter(|t| t.revoked)
                .map(|t| {
                    let up = st.toasts.iter().any(|toast| matches!(&toast.action, Some((_, Command::ApiRestoreToken(id))) if *id == t.id));
                    (t.id.clone(), up)
                })
                .collect()
        };
        if !revoked.is_empty() {
            self.api_edit(move |a| {
                let gone = purgeable(&a.tokens, now, |id| revoked.iter().any(|(r, up)| r == id && *up));
                // A token revoked in the file but not in the view (another
                // run) waits for the next start.
                let gone: Vec<String> = gone.into_iter().filter(|id| revoked.iter().any(|(r, _)| r == id)).collect();
                a.tokens.retain(|t| !gone.contains(&t.id));
                !gone.is_empty()
            });
        }
        if self.api.used_unsaved && self.api.used_saved_at.is_none_or(|t| t.elapsed() >= USED_SAVE_EVERY) {
            // Tried once per period, whatever comes of it.
            self.api.used_saved_at = Some(Instant::now());
            self.api_edit(|_| true);
        }
    }

    /// At the end: the tokens' last uses not written yet.
    pub(crate) fn api_flush(&mut self) {
        if self.api.used_unsaved {
            self.api_edit(|_| true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, revoked_ms: Option<i64>) -> ApiTokenRecord {
        ApiTokenRecord {
            id: id.into(),
            name: id.into(),
            preset: Default::default(),
            actor: Default::default(),
            hash: "00".repeat(32),
            created_ms: 0,
            last_used_ms: None,
            revoked_ms,
        }
    }

    /// A revoked token goes only once its «Отменить» is gone and a few
    /// seconds have passed; a live one never.
    #[test]
    fn a_revoked_token_goes_when_its_undo_is_gone() {
        let tokens = [record("live", None), record("fresh", Some(9_000)), record("old", Some(1_000)), record("undo", Some(1_000))];
        let gone = purgeable(&tokens, 10_000, |id| id == "undo");
        assert_eq!(gone, vec!["old".to_owned()]);
        assert!(purgeable(&tokens, 10_000, |_| true).is_empty());
        assert_eq!(purgeable(&tokens, 20_000, |_| false), vec!["fresh".to_owned(), "old".to_owned(), "undo".to_owned()]);
    }

    #[test]
    fn the_view_has_no_secret_and_keeps_the_report() {
        let api = ApiSettings { enabled: true, tokens: vec![record("a", Some(5))], ..ApiSettings::default() };
        let mut view = ApiView { listening: Some("127.0.0.1:47210".into()), synced: 7, ..ApiView::default() };
        apply(&mut view, &api, true);
        assert!(view.enabled && view.configurable);
        assert_eq!(view.listening.as_deref(), Some("127.0.0.1:47210"));
        assert_eq!(view.synced, 7);
        assert!(view.tokens[0].revoked);
        assert_eq!(view.live_tokens().count(), 0);
    }
}
