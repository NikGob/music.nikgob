//! The services' own suggestions: YouTube Music's
//! quick picks, listen again and history, Spotify's tops and recently
//! played, read as a signal of taste for our mixes (never shown as cards of
//! their own).
//!
//! * **When**: each signed-in service at most every
//!   [`StreamSchedule::suggest_every`] (6 h), the first time
//!   [`StreamSchedule::suggest_first`] after the start; one service at a
//!   time; a pass that got nothing is tried again after [`RETRY_AFTER`].
//!   A service switched off or signed out is asked nothing.
//! * **How**: one request a feed ([`StreamService::suggestion_feeds`]), at
//!   most [`SUGGEST_BUDGET`] a pass, [`StreamSchedule::discover_gap`] apart,
//!   in the service's background lane, never while a search is typed or
//!   answered or while the periodic network work is idle. A rate limit
//!   waits out the pause and asks the same feed again.
//! * **Kept** in the library's cache (`suggest.{service}`), with when they
//!   were read; a change of what the services suggest builds the
//!   recommendations again (the snapshot matches them against the library,
//!   [`nikgob_recs::match_suggestions`]) and then ranks the day's
//!   discoveries again with the services' votes.
//! * **Settings → Источники**: a quiet line per service, «Подсказки: 48
//!   треков · обновлено в 14:10», or why there are none; a Spotify token
//!   without `user-top-read` adds «Переподключи Spotify, чтобы брать твой
//!   топ».
//!
//! [`StreamSchedule::suggest_every`]: crate::StreamSchedule::suggest_every
//! [`StreamSchedule::suggest_first`]: crate::StreamSchedule::suggest_first
//! [`StreamSchedule::discover_gap`]: crate::StreamSchedule::discover_gap

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::i18n::activity::{fmt_count_in, fmt_reconnect_in};
use nikgob_core::i18n::{Locale, activity_in, common_in};
use nikgob_core::model::UnixMs;
use nikgob_core::streaming::{
    RemoteId, Service, StreamError, StreamService, SuggestedArtist, SuggestedTrack, SuggestionFeed, Suggestions,
};
use nikgob_library::now_ms;
use serde::{Deserialize, Serialize};

use super::Msg;
use crate::Worker;
use crate::import::Internal;
use crate::streams::StreamSchedule;

/// Requests of one pass.
pub(crate) const SUGGEST_BUDGET: u32 = 10;
/// A pass that got nothing is tried again after this.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);
/// Library cache keys: `suggest.youtube`, `suggest.spotify`.
const KEY: &str = "suggest.";
/// The services that can have suggestions, in the order they are asked.
const SERVICES: [Service; 3] = [Service::Youtube, Service::Spotify, Service::Soundcloud];

/// One service's suggestions as kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// When they were read (unix ms).
    pub at: UnixMs,
    pub tracks: Vec<SuggestedTrack>,
    pub artists: Vec<SuggestedArtist>,
    /// A feed needs a new sign-in (Spotify's tops without `user-top-read`).
    #[serde(default)]
    pub needs_reconnect: bool,
}

impl Kept {
    /// What it suggests, for telling a change (not when).
    fn content(&self) -> (HashSet<&RemoteId>, Vec<&str>, bool) {
        let mut artists: Vec<&str> = self.artists.iter().map(|a| a.artist.name.as_str()).collect();
        artists.sort_unstable();
        artists.dedup();
        (self.tracks.iter().map(|t| &t.track.id).collect(), artists, self.needs_reconnect)
    }

    fn add(&mut self, s: Suggestions) {
        self.tracks.extend(s.tracks);
        self.artists.extend(s.artists);
        self.needs_reconnect |= s.needs_reconnect;
    }
}

/// A pass over one service's feeds.
struct Pass {
    service: Service,
    feeds: VecDeque<SuggestionFeed>,
    got: Kept,
    /// Feeds that answered.
    answered: u32,
    requests: u32,
}

pub(crate) enum SuggestMsg {
    Answered {
        service: Service,
        feed: SuggestionFeed,
        result: Result<Suggestions, StreamError>,
    },
    /// A gap or a rate-limit pause is over.
    Wake,
}

#[derive(Default)]
pub(crate) struct SuggestState {
    loaded: bool,
    kept: HashMap<Service, Kept>,
    pass: Option<Pass>,
    busy: bool,
    next_at: Option<Instant>,
    /// The first tick of this run (the first pass waits
    /// [`StreamSchedule::suggest_first`] from it).
    started: Option<Instant>,
    /// When a pass of a service last got nothing.
    failed_at: HashMap<Service, Instant>,
}

fn key(service: Service) -> String {
    format!("{KEY}{}", service.as_str())
}

/// Every service's kept suggestions (a build without a worker,
/// [`super::preview_home`]).
pub(crate) fn cached(lib: &nikgob_library::Library) -> Vec<Kept> {
    SERVICES.iter().filter_map(|s| super::cache_read(lib, &key(*s))).collect()
}

/// «14:10» today, «21.10 в 14:10» another day (local time).
fn when(at: UnixMs, loc: Locale) -> String {
    let Ok(ts) = jiff::Timestamp::from_millisecond(at) else { return String::new() };
    let tz = jiff::tz::TimeZone::system();
    let at = ts.to_zoned(tz.clone());
    let today = jiff::Timestamp::now().to_zoned(tz).date();
    let s = activity_in(loc);
    if at.date() == today {
        format!("{} {}", s.at, at.strftime("%H:%M"))
    } else {
        format!("{} {} {}", at.strftime("%d.%m"), s.at, at.strftime("%H:%M"))
    }
}

/// «Подсказки: 48 треков, 6 исполнителей · обновлено в 14:10».
pub(crate) fn line_of(k: &Kept, loc: Locale) -> String {
    let s = activity_in(loc);
    let tracks: HashSet<&RemoteId> = k.tracks.iter().map(|t| &t.track.id).collect();
    let artists: HashSet<&str> = k.artists.iter().map(|a| a.artist.name.as_str()).collect();
    let mut what = Vec::new();
    if !tracks.is_empty() {
        what.push(fmt_count_in(loc, tracks.len(), &common_in(loc).tracks));
    }
    if !artists.is_empty() {
        what.push(fmt_count_in(loc, artists.len(), &common_in(loc).artists));
    }
    let when = when(k.at, loc);
    if what.is_empty() {
        format!("{}: {} · {} {when}", s.suggestions, s.service_no_suggestions, s.updated)
    } else {
        format!("{}: {} · {} {when}", s.suggestions, what.join(", "), s.updated)
    }
}

impl Worker {
    pub(crate) fn suggest_load(&mut self) {
        if std::mem::replace(&mut self.recs.suggest.loaded, true) {
            return;
        }
        for s in SERVICES {
            let kept: Option<Kept> = super::cache_read(&self.lib, &key(s));
            if let Some(k) = kept {
                self.recs.suggest.kept.insert(s, k);
            }
        }
    }

    /// A service that may be asked for suggestions now: registered, on,
    /// signed in, with feeds.
    fn suggest_service(&self, service: Service) -> Option<Arc<dyn StreamService>> {
        self.service(service)
            .filter(|s| s.account().is_logged_in() && !s.catalogue_only() && !s.suggestion_feeds().is_empty())
    }

    /// The kept suggestions of the services switched on (the snapshot's
    /// input).
    pub(crate) fn suggestions_kept(&mut self) -> Vec<Kept> {
        self.suggest_load();
        let prefs = self.state.read().prefs.clone();
        SERVICES
            .iter()
            .filter(|s| prefs.service_enabled(**s))
            .filter_map(|s| self.recs.suggest.kept.get(s).cloned())
            .collect()
    }

    /// The quiet lines of «Источники» for `service`: what its suggestions
    /// are (or why there are none) and a note when a feed needs a new
    /// sign-in.
    /// What was kept is loaded first ([`Self::suggest_load`]).
    pub(crate) fn suggest_lines(&self, service: Service) -> (Option<String>, Option<String>) {
        let Some(svc) = self.streams.iter().find(|s| s.service() == service) else { return (None, None) };
        if svc.suggestion_feeds().is_empty() || !self.state.read().prefs.service_enabled(service) {
            return (None, None);
        }
        let signed_in = svc.account().is_logged_in();
        let kept = self.recs.suggest.kept.get(&service);
        let loc = self.locale();
        let s = activity_in(loc);
        let line = match kept {
            _ if !signed_in => match service {
                // Spotify's own block already says it is not connected.
                Service::Spotify => None,
                _ => Some(s.suggestions_sign_in.to_owned()),
            },
            Some(k) => Some(line_of(k, loc)),
            None => Some(s.suggestions_not_loaded.to_owned()),
        };
        let note = kept
            .filter(|k| signed_in && k.needs_reconnect)
            .map(|_| fmt_reconnect_in(loc, service.display_name()));
        (line, note)
    }

    /// Every tick: a pass when one is due, its next request when it is
    /// time.
    pub(crate) fn suggest_tick(&mut self) {
        // Not while a game is in front (the tick goes on when it is over).
        if crate::quiet::busy() {
            return;
        }
        self.suggest_load();
        let st = &mut self.recs.suggest;
        let started = *st.started.get_or_insert_with(Instant::now);
        if started.elapsed() < self.streaming.schedule.suggest_first {
            return;
        }
        if self.recs.suggest.pass.is_none() && !self.suggest_plan() {
            return;
        }
        self.suggest_step();
    }

    /// Starts a pass over the first service whose suggestions are due.
    fn suggest_plan(&mut self) -> bool {
        let every = self.streaming.schedule.suggest_every.as_millis() as i64;
        let now = now_ms();
        for service in SERVICES {
            let Some(svc) = self.suggest_service(service) else { continue };
            let st = &self.recs.suggest;
            let fresh = st.kept.get(&service).is_some_and(|k| now - k.at < every && k.at <= now);
            let failed = st.failed_at.get(&service).is_some_and(|t| t.elapsed() < RETRY_AFTER);
            if fresh || failed {
                continue;
            }
            let feeds: VecDeque<SuggestionFeed> = svc.suggestion_feeds().iter().copied().take(SUGGEST_BUDGET as usize).collect();
            log::info!("suggest: reading {} feeds of {service}", feeds.len());
            self.recs.suggest.pass = Some(Pass {
                service,
                feeds,
                got: Kept::default(),
                answered: 0,
                requests: 0,
            });
            return true;
        }
        false
    }

    fn suggest_wait(&mut self, d: Duration) {
        self.recs.suggest.next_at = Some(Instant::now() + d);
        self.streaming.spawn_internal(async move {
            tokio::time::sleep(d).await;
            Internal::Recs(Msg::Suggest(SuggestMsg::Wake))
        });
    }

    /// Sends the next request of the pass when everything allows it.
    fn suggest_step(&mut self) {
        let st = &self.recs.suggest;
        if st.busy || st.next_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        if self.streaming.search.active() || self.streams_idle() {
            return;
        }
        let Some(service) = st.pass.as_ref().map(|p| p.service) else { return };
        let Some(svc) = self.suggest_service(service) else {
            // Switched off or signed out meanwhile: nothing more is asked.
            log::info!("suggest: {service} is not available any more");
            self.recs.suggest.pass = None;
            return;
        };
        // Its pause, or the listener's turn on YouTube Music.
        if let Some(left) = self.background_wait(service) {
            log::info!("suggest: {service} asks to wait {} s", left.as_secs());
            self.suggest_wait(left + crate::streams::wake_jitter());
            return;
        }
        let Some(pass) = self.recs.suggest.pass.as_mut() else { return };
        let feed = match pass.feeds.pop_front() {
            Some(f) if pass.requests < SUGGEST_BUDGET => f,
            _ => {
                self.suggest_finish();
                return;
            }
        };
        pass.requests += 1;
        self.recs.suggest.busy = true;
        self.streaming.spawn_internal(async move {
            let result = svc.suggestions(feed).await;
            Internal::Recs(Msg::Suggest(SuggestMsg::Answered { service, feed, result }))
        });
    }

    pub(crate) fn suggest_msg(&mut self, m: SuggestMsg) {
        let (service, feed, result) = match m {
            SuggestMsg::Answered { service, feed, result } => (service, feed, result),
            SuggestMsg::Wake => {
                self.suggest_tick();
                return;
            }
        };
        self.recs.suggest.busy = false;
        let gap = StreamSchedule::between(self.streaming.schedule.discover_gap);
        let unknown = self.streaming.schedule.rate_limit_unknown;
        let paused = self.pause_left(service);
        let Some(pass) = self.recs.suggest.pass.as_mut().filter(|p| p.service == service) else { return };
        match result {
            Ok(s) => {
                pass.answered += 1;
                pass.got.add(s);
            }
            Err(StreamError::RateLimited { retry_after }) => {
                // Not a failure: the same feed after the pause.
                pass.requests = pass.requests.saturating_sub(1);
                pass.feeds.push_front(feed);
                let wait = retry_after.or(paused).unwrap_or(unknown);
                log::info!("suggest: rate limited by {service}, again in {} s", wait.as_secs());
                self.suggest_wait(wait + crate::streams::wake_jitter());
                return;
            }
            Err(StreamError::NotLoggedIn) => {
                log::info!("suggest: {service} is signed out");
                self.recs.suggest.pass = None;
                return;
            }
            Err(e) => log::info!("suggest: {service} {feed:?}: {e}"),
        }
        if pass.feeds.is_empty() {
            self.suggest_finish();
        } else {
            self.suggest_wait(gap);
        }
    }

    /// The pass is done: kept, and the recommendations built again when
    /// the suggestions changed.
    fn suggest_finish(&mut self) {
        let Some(pass) = self.recs.suggest.pass.take() else { return };
        let service = pass.service;
        if pass.answered == 0 {
            log::info!("suggest: nothing from {service}, again later");
            self.recs.suggest.failed_at.insert(service, Instant::now());
            return;
        }
        let kept = Kept { at: now_ms(), ..pass.got };
        log::info!(
            "suggest: {service} suggests {} tracks and {} artists ({} requests)",
            kept.tracks.len(),
            kept.artists.len(),
            pass.requests
        );
        super::cache_write(&self.lib, &key(service), kept.at, &kept);
        let changed = self.recs.suggest.kept.get(&service).is_none_or(|old| old.content() != kept.content());
        self.recs.suggest.kept.insert(service, kept);
        self.recs.suggest.failed_at.remove(&service);
        self.publish_accounts();
        if changed {
            // The discoveries are ranked again once the snapshot has them.
            self.recs.discover_rerank = true;
            self.recs_library_changed();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{ArtistRef, StreamTrack, SuggestionKind};

    fn track(id: &str) -> SuggestedTrack {
        SuggestedTrack {
            track: StreamTrack::new(RemoteId::new(Service::Youtube, id), id),
            kind: SuggestionKind::QuickPicks,
            weight: 1.0,
        }
    }

    #[test]
    fn the_settings_line_counts_songs_and_artists() {
        let at = now_ms();
        let mut k = Kept {
            at,
            tracks: vec![track("a"), track("b"), track("a")],
            artists: Vec::new(),
            needs_reconnect: false,
        };
        let line = line_of(&k, Locale::Ru);
        assert!(line.starts_with("Подсказки: 2 трека · обновлено в "), "{line}");
        k.artists = (0..5)
            .map(|i| SuggestedArtist {
                artist: ArtistRef::new(None, format!("A{i}")),
                kind: SuggestionKind::TopMonth,
                weight: 1.0,
            })
            .collect();
        assert!(line_of(&k, Locale::Ru).starts_with("Подсказки: 2 трека, 5 исполнителей · "), "{}", line_of(&k, Locale::Ru));
        let empty = Kept { at, ..Kept::default() };
        assert!(line_of(&empty, Locale::Ru).starts_with("Подсказки: сервис пока ничего не советует"));
        assert!(line_of(&empty, Locale::En).starts_with("Suggestions: the service has nothing to suggest yet"));
        let old = Kept { at: at - 3 * 24 * 3600 * 1000, ..k };
        assert!(line_of(&old, Locale::Ru).contains('.'), "another day says the date: {}", line_of(&old, Locale::Ru));
    }

    #[test]
    fn a_change_is_what_they_suggest_not_when() {
        let a = Kept {
            at: 1,
            tracks: vec![track("a")],
            ..Kept::default()
        };
        let mut b = Kept { at: 2, ..a.clone() };
        assert_eq!(a.content(), b.content());
        b.tracks.push(track("c"));
        assert_ne!(a.content(), b.content());
        let c = Kept {
            needs_reconnect: true,
            ..a.clone()
        };
        assert_ne!(a.content(), c.content());
    }
}
