//! The local REST API's settings (`[api]` of settings.toml), kept and
//! applied by the loop and never by the API's own thread, and what the
//! API server reports back: where it listens, the tokens it saw used.
//!
//! A revoked token is refused at once (the server follows
//! [`nikgob_view::ApiView`]) and stays in the file, marked, while its
//! snackbar's «Отменить» is up; then it goes for good. The last use of a
//! token is shown at once and written rarely.
//!
//! The requests of the API that wait for work of the loop's own (a share
//! link, the tags in files, a search of the download sources) answer
//! through their [`ApiReply`]; a download too large to start by itself
//! waits in «Подтверди» as a card of its own ([`API_CONFIRM_ID`]).

use std::time::{Duration, Instant};

use nikgob_core::model::{ReleaseId, TrackId, WishlistItem, WishlistScope, WishlistStatus};
use nikgob_core::settings::{ApiSettings, ApiTokenRecord, Settings};
use nikgob_core::source::SourceError;
use nikgob_view::{ApiActor, ApiReply, ApiTokenView, ApiView, Command, Confirm, ConfirmOption};

use crate::{PlaybackEvent, Worker};

/// Bit of the «Подтверди» ids of downloads the API asked for (the other
/// parts of the runtime use bits 44–48).
pub(crate) const API_CONFIRM_ID: u64 = 1 << 50;
/// Cards of the API in «Подтверди» at most; a new one pushes out the oldest.
const API_CONFIRMS: usize = 16;
/// Runs whose skips are kept for the API to read.
const API_SKIPS_KEPT: usize = 32;
/// Offers of a dry search of the sources answered at most.
const OFFERS_ANSWERED: usize = 12;
/// A source's dry search at most (the API waits a little longer).
const SOURCE_SEARCH_TIMEOUT: Duration = Duration::from_secs(60);

/// A download of the API waiting in «Подтверди».
struct ApiConfirm {
    card: Confirm,
    /// What «Скачать» runs.
    commands: Vec<Command>,
}

/// The card of a download the API asked for.
pub(crate) fn confirm_card(id: u64, title: &str, origin: &str, lines: &[String]) -> Confirm {
    Confirm {
        id,
        title: title.to_owned(),
        origin: origin.to_owned(),
        question: "Скачать?".to_owned(),
        options: lines.iter().map(|l| ConfirmOption { label: l.clone(), detail: String::new(), recommended: false }).collect(),
        skip_label: "Не скачивать".to_owned(),
        accept_label: "Скачать".to_owned(),
        pick: None,
        yes_no: true,
    }
}

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
    /// The shuffle an API request asked for the next list it makes
    /// ([`Command::ApiShuffle`]); gone at the end of its request.
    pub(crate) next_shuffle: Option<bool>,
    /// A device switch is waiting for its device: a device error now is the
    /// new device's (the one heard plays on).
    pub(crate) switching_device: bool,
    /// The device heard failed and nothing has played since.
    device_failed: bool,
    /// Who the API request being run plays for (its token's actor), while
    /// its commands run: what the history's events are to carry once they
    /// have an actor.
    pub(crate) actor: Option<ApiActor>,
    /// Downloads of the API waiting in «Подтверди», oldest first.
    confirms: Vec<ApiConfirm>,
    next_confirm: u64,
    /// An [`Command::ApiRun`] is being run: what its commands could not do
    /// goes to [`Self::skipped`].
    pub(crate) in_run: bool,
    pub(crate) skipped: Vec<String>,
}

/// What a device event says of the sound: `Some(true)` it comes out,
/// `Some(false)` the device heard failed, None nothing new. `switching`:
/// a device switch waits for its device.
pub(crate) fn audio_news(ev: &PlaybackEvent, switching: bool) -> Option<bool> {
    match ev {
        PlaybackEvent::DeviceError { .. } => (!switching).then_some(false),
        PlaybackEvent::Started { .. }
        | PlaybackEvent::TrackChanged { .. }
        | PlaybackEvent::Resumed
        | PlaybackEvent::Position { .. }
        | PlaybackEvent::PositionAt { .. }
        | PlaybackEvent::DeviceReady { .. } => Some(true),
        _ => None,
    }
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
        self.publish_audio();
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

    /// Whether sound can come out, into the view: the engine started, and
    /// the device heard did not fail since something last played.
    pub(crate) fn publish_audio(&mut self) {
        let no_audio = !self.playback.available() || self.api.device_failed;
        if self.state.read().no_audio != no_audio {
            self.publish(move |s| s.no_audio = no_audio);
        }
    }

    /// A playback event, for [`nikgob_view::ViewState::no_audio`].
    pub(crate) fn api_audio_event(&mut self, ev: &PlaybackEvent) {
        if matches!(ev, PlaybackEvent::DeviceError { .. } | PlaybackEvent::DeviceReady { .. }) {
            // The switch got its answer either way.
            let switching = std::mem::take(&mut self.api.switching_device);
            if let Some(ok) = audio_news(ev, switching) {
                self.api.device_failed = !ok;
            }
        } else if audio_news(ev, false) == Some(true) {
            if !self.api.device_failed {
                return;
            }
            self.api.device_failed = false;
        } else {
            return;
        }
        self.publish_audio();
    }

    /// Changes `[api]` in settings.toml (`edit` says whether it changed
    /// anything) with the tokens' last uses as shown, and shows the result.
    /// False when nothing was saved; a failure is a toast.
    fn api_edit(&mut self, edit: impl FnOnce(&mut ApiSettings) -> bool) -> bool {
        match self.api_save(edit) {
            Ok(saved) => saved,
            Err(why) => {
                self.toast(why, None);
                false
            }
        }
    }

    /// [`Self::api_edit`] with the failure in words instead of a toast.
    fn api_save(&mut self, edit: impl FnOnce(&mut ApiSettings) -> bool) -> Result<bool, &'static str> {
        let Some(path) = self.settings_path.clone() else {
            return Err("Некуда сохранить настройки");
        };
        // A file that cannot be read is not replaced by defaults: that would
        // drop every other secret in it.
        let mut settings = match Settings::load(&path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("loading settings: {e}");
                return Err("settings.toml не читается — поправь файл вручную");
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
            return Ok(false);
        }
        if let Err(e) = settings.save(&path) {
            log::error!("saving settings: {e}");
            return Err("Не удалось сохранить настройки");
        }
        self.api.used_unsaved = false;
        self.api.used_saved_at = Some(Instant::now());
        let api = settings.api;
        self.publish(move |s| apply(&mut s.api, &api, true));
        Ok(true)
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
                let id = t.id.clone();
                if t.name.trim().is_empty() || nikgob_core::api_token::Hashed::parse(&t.hash).is_none() {
                    log::warn!("api: a token without a name or a hash refused");
                    self.publish(move |s| s.api.token_failed = Some((id, "Токен без названия".to_owned())));
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
                // The dialog shows the secret once the token is in the view
                // (kept in the file), or this failure.
                let failed = match self.api_save(move |a| {
                    if a.tokens.iter().any(|x| x.id == record.id) {
                        return false;
                    }
                    a.tokens.push(record);
                    true
                }) {
                    Ok(true) => None,
                    Ok(false) => Some("Такой токен уже есть"),
                    Err(why) => Some(why),
                };
                if let Some(why) = failed {
                    self.publish(move |s| s.api.token_failed = Some((id, why.to_owned())));
                }
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
            Command::ApiRun { commands, ack, actor } => {
                self.api.actor = actor;
                self.api.in_run = true;
                self.api.skipped.clear();
                // As the window's: while the target is another device, the
                // player's commands go there.
                for c in commands {
                    self.user_command(c);
                }
                self.api.in_run = false;
                let skipped = std::mem::take(&mut self.api.skipped);
                if !skipped.is_empty() {
                    self.publish(move |s| {
                        s.api.skipped.push((ack, skipped));
                        let over = s.api.skipped.len().saturating_sub(API_SKIPS_KEPT);
                        s.api.skipped.drain(..over);
                    });
                }
                // A shuffle asked for a list the request did not make (it had
                // nothing to play) does not reach a later one.
                self.api.next_shuffle = None;
                self.api.actor = None;
                self.publish(move |s| s.api.synced = s.api.synced.max(ack));
            }
            Command::ApiShuffle(on) => self.api.next_shuffle = Some(on),
            Command::ApiShare { target, reply } => self.share_for_api(target, reply),
            Command::ApiReadTags { tracks, reply } => self.api_read_tags(tracks, reply),
            Command::ApiRetag { tracks, reply } => self.api_retag(tracks, reply),
            Command::ApiTrash { tracks, what } => {
                // Never for good: with the Trash off nothing is deleted (the
                // API refuses such a request before it gets here).
                if !self.state.read().prefs.use_trash {
                    log::warn!("api: {} tracks not deleted, the Trash is off", tracks.len());
                    self.api_skip("the Trash is off".to_owned());
                    return;
                }
                self.delete_tracks(tracks, &what);
            }
            Command::ApiSourceSearch { track, release, reply } => self.api_source_search(track, release, reply),
            Command::ApiConfirmDownload { title, origin, lines, commands } => {
                self.api.next_confirm += 1;
                let id = API_CONFIRM_ID | self.api.next_confirm;
                if self.api.confirms.len() >= API_CONFIRMS {
                    let old = self.api.confirms.remove(0);
                    log::info!("api: the download «{}» waited too long in «Подтверди», dropped", old.card.title);
                }
                self.api.confirms.push(ApiConfirm { card: confirm_card(id, &title, &origin, &lines), commands });
                self.publish_api_confirms();
            }
            Command::ApiKnow(tracks) => self.streaming.learn(&tracks),
            Command::ApiCreatePlaylist { id, name, tracks } => self.api_create_playlist(id, &name, &tracks),
            other => log::warn!("api: not an API command {other:?}"),
        }
    }

    /// The API's cards into «Подтверди» (the other cards stay).
    fn publish_api_confirms(&mut self) {
        let cards: Vec<Confirm> = self.api.confirms.iter().map(|c| c.card.clone()).collect();
        self.publish(move |s| {
            s.downloads.confirm.retain(|c| c.id & API_CONFIRM_ID == 0);
            s.downloads.confirm.extend(cards);
        });
    }

    /// Something a command of an [`Command::ApiRun`] could not do: the API
    /// answers it instead of «done». Outside a run (the window's own
    /// commands) nothing is kept.
    pub(crate) fn api_skip(&mut self, why: String) {
        if self.api.in_run && self.api.skipped.len() < 50 {
            self.api.skipped.push(why);
        }
    }

    /// [`Command::ApiCreatePlaylist`]: the playlist under the API's id, as
    /// «Сохранить как плейлист» makes one.
    fn api_create_playlist(&mut self, id: nikgob_core::model::PlaylistId, name: &str, tracks: &[TrackId]) {
        let before = self.library_tracks();
        let Some(p) = self.log_err("плейлист", self.lib.create_playlist_with_id(id, name)) else {
            self.api_skip(format!("playlist «{name}» not created"));
            return;
        };
        if !tracks.is_empty() {
            let _ = self.log_err("плейлист", self.lib.playlist_add_many(p.id, tracks));
        }
        self.toast(format!("Плейлист «{name}» сохранён"), None);
        self.library_edited(before, Vec::new());
    }

    /// «Скачать» (`Some`) or «Не скачивать» (`None`) of an API card.
    pub(crate) fn api_confirm(&mut self, id: u64, option: Option<usize>) {
        let Some(at) = self.api.confirms.iter().position(|c| c.card.id == id) else { return };
        let c = self.api.confirms.remove(at);
        self.publish_api_confirms();
        if option.is_some() {
            log::info!("api: «{}» downloads, as confirmed", c.card.title);
            for command in c.commands {
                self.command(command);
            }
        }
    }

    /// The download sources searched for a track or an album, nothing
    /// downloaded: the offers that match it, best first, to `reply`.
    fn api_source_search(&mut self, track: Option<TrackId>, release: Option<ReleaseId>, reply: ApiReply) {
        let mut item = WishlistItem::new(WishlistStatus::New, nikgob_library::now_ms());
        item.track_id = track;
        item.release_id = release;
        item.scope = if track.is_some() { WishlistScope::Track } else { WishlistScope::Album };
        let want = self.want_of(&item);
        if want.artists.is_empty() || (want.title.is_none() && want.album.is_none()) {
            reply.send(Err("Не знаю, что искать: у песни нет исполнителя или названия".to_owned()));
            return;
        }
        let sources = self.download_sources();
        if sources.is_empty() {
            reply.send(Err("Источники загрузок не настроены".to_owned()));
            return;
        }
        let order: Vec<_> = self.state.read().prefs.source_order().iter().filter(|s| s.on).map(|s| s.key).collect();
        self.streaming.spawn_reporting(move |_| async move {
            let asks = sources.iter().map(|s| {
                let s = s.clone();
                let want = &want;
                async move {
                    let r = match tokio::time::timeout(SOURCE_SEARCH_TIMEOUT, s.search(want)).await {
                        Ok(r) => r,
                        Err(_) => Err(SourceError::Unreachable("no answer in time".into())),
                    };
                    (s.key(), r)
                }
            });
            let mut offers = Vec::new();
            let mut asked = Vec::new();
            let mut failed = Vec::new();
            for (key, r) in futures_util::future::join_all(asks).await {
                match r {
                    Ok(o) => {
                        asked.push(key.display_name());
                        offers.extend(o);
                    }
                    Err(SourceError::NotConfigured) => {}
                    Err(e) => failed.push(serde_json::json!({"source": key.display_name(), "error": e.to_string()})),
                }
            }
            let found = offers.len();
            let ranked = nikgob_sources::select::rank(offers, &want, &order);
            let list: Vec<serde_json::Value> = ranked
                .iter()
                .take(OFFERS_ANSWERED)
                .map(|r| {
                    let o = &r.offer;
                    serde_json::json!({
                        "source": o.source.display_name(),
                        "provider": o.provider,
                        "title": o.title,
                        "codec": o.format.as_ref().map(|f| f.codec.as_str()),
                        "bitrate_kbps": o.format.as_ref().and_then(|f| f.bitrate_kbps),
                        "lossless": o.lossless(),
                        "size_bytes": o.size_bytes,
                        "seeders": o.seeders,
                        "sure": r.verdict.sure,
                        "score": (r.verdict.score * 100.0).round() / 100.0,
                    })
                })
                .collect();
            reply.send(Ok(serde_json::json!({
                "wanted": {"artists": want.artists, "title": want.title, "album": want.album},
                "asked": asked,
                "failed": failed,
                "found": found,
                "matching": ranked.len(),
                "offers": list,
            })));
        });
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

    /// A device error is the heard device's unless a switch waits; any
    /// sound playing clears it.
    #[test]
    fn the_sound_comes_out_or_not() {
        let err = PlaybackEvent::DeviceError { generation: 1, message: "busy".into() };
        assert_eq!(audio_news(&err, false), Some(false));
        assert_eq!(audio_news(&err, true), None, "the switch's device failed, the heard one plays on");
        assert_eq!(audio_news(&PlaybackEvent::Started { id: 3 }, false), Some(true));
        assert_eq!(audio_news(&PlaybackEvent::Resumed, false), Some(true));
        assert_eq!(audio_news(&PlaybackEvent::DeviceReady { generation: 2 }, true), Some(true));
        assert_eq!(audio_news(&PlaybackEvent::Paused, false), None);
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

    /// A download of the API waits as a yes-or-no card of its own bit, its
    /// lines shown, «Скачать» and «Не скачивать».
    #[test]
    fn an_api_download_card() {
        let c = confirm_card(API_CONFIRM_ID | 3, "A — B и ещё 4", "Запрос «Claude»", &["A — B".to_owned(), "C — D".to_owned()]);
        assert_eq!(c.id & API_CONFIRM_ID, API_CONFIRM_ID);
        for other in [1u64 << 44, 1 << 45, 1 << 47, 1 << 48] {
            assert_eq!(c.id & other, 0, "no other part's bit");
        }
        assert!(c.yes_no && c.answerable());
        assert_eq!(c.options.len(), 2);
        assert_eq!((c.accept_label.as_str(), c.skip_label.as_str()), ("Скачать", "Не скачивать"));
    }
}
