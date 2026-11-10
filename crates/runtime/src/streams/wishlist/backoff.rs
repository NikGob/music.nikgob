//! What turned the downloads away, kept across restarts. Before, every
//! pause lived only in memory: the app restarted several times a day, and
//! each start asked every source again and began each item's retries from
//! the first five minutes, so one track was searched about 90 times in five
//! days (the sources diagnosis of 2026-11-10). Now one `cache` entry
//! ([`KEY`]) keeps
//!
//! - the web sources sitting out ([`Pauses`]): a bot check, a rate limit
//!   or a day's limit, the same error again and again at search; such a
//!   source is not asked until then, and «Источники» tell why and when;
//! - the sources set aside for refusing downloads ([`super::Refusals`]);
//! - each item's retries in a row and the sources its retry asks;
//! - each item's searches of the last day ([`DayTries`]): at most
//!   [`DAY_TRIES`], then it waits for the day to pass.
//!
//! What is past goes when it is read: a pause a day over its end, a day's
//! count a day old, the retries of an item that is done or turned down.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nikgob_core::model::WishlistId;
use nikgob_core::source::{SourceError, SourceKey};
use nikgob_sources::http::{LONGEST_PAUSE, STRIKES, is_check, pause_for};
use serde::{Deserialize, Serialize};

use super::{Refused, Refusals};

/// `cache` key of what is kept.
pub(super) const KEY: &str = "wish_backoff.v1";
/// Searches of one item in a day at most (its first one and the retries
/// after refusals fit; a restart does not start the count over).
pub(super) const DAY_TRIES: u32 = 8;
const DAY_MS: i64 = 86_400_000;

/// Sources a pause of the wishlist is for: the web ones (sites of strangers
/// that close or ration themselves). The indexer and Soulseek are the
/// user's own setup, asked as they are.
fn may_pause(key: SourceKey) -> bool {
    !matches!(key, SourceKey::Torrents | SourceKey::Soulseek)
}

/// Web sources whose searches turned us away, sitting out: a bot check at
/// once ([`pause_for`]: three hours, doubling up to a day), a wait
/// it asked for whole (a Worker's day: until midnight UTC), the same HTTP
/// error [`STRIKES`] times in a row (half an hour, doubling). A search
/// that answers ends it.
#[derive(Default)]
pub(crate) struct Pauses(HashMap<SourceKey, Pause>);

#[derive(Default, Clone, Debug)]
struct Pause {
    /// Pauses in a row.
    times: u32,
    /// HTTP errors in a row.
    strikes: u32,
    until: Option<Instant>,
    /// What started the pause.
    why: Option<SourceError>,
}

impl Pauses {
    /// A search of `key` failed with `e`: the time it sits out until, when
    /// that starts now. A search that set out before the pause does not
    /// lengthen it (a longer wait it asks for does).
    pub(crate) fn failed(&mut self, key: SourceKey, e: &SourceError, now: Instant) -> Option<Instant> {
        // A refused login or key is not kept: the user fixes it and starts
        // the app again, and it is asked at once.
        let counts = is_check(e) || matches!(e, SourceError::RateLimited { .. } | SourceError::Http { .. });
        if !may_pause(key) || !counts {
            return None;
        }
        let p = self.0.entry(key).or_default();
        if let Some(until) = p.until.filter(|u| *u > now) {
            if let Some(d) = e.wait() {
                p.until = Some(until.max(now + d.min(LONGEST_PAUSE)));
            }
            return None;
        }
        let start = match e {
            SourceError::Blocked(_) | SourceError::RateLimited { .. } => true,
            SourceError::Http { .. } => {
                p.strikes += 1;
                p.strikes >= STRIKES
            }
            _ => false,
        };
        if !start {
            return None;
        }
        let at = now + pause_for(e, p.times);
        p.until = Some(at);
        p.times += 1;
        p.strikes = 0;
        p.why = Some(e.clone());
        Some(at)
    }

    /// A search of `key` answered: its pause and counts end. True when it
    /// had any.
    pub(crate) fn answered(&mut self, key: SourceKey) -> bool {
        self.0.remove(&key).is_some()
    }

    /// Until when `key` sits out, if it does.
    pub(crate) fn until(&self, key: SourceKey, now: Instant) -> Option<Instant> {
        self.0.get(&key).and_then(|p| p.until).filter(|u| *u > now)
    }

    /// What `key` is told as while it sits out: what set it aside (a wait,
    /// with how long is left).
    pub(crate) fn error(&self, key: SourceKey, now: Instant) -> Option<SourceError> {
        let p = self.0.get(&key)?;
        let until = p.until.filter(|u| *u > now)?;
        Some(match &p.why {
            Some(SourceError::RateLimited { .. }) | None => SourceError::RateLimited { retry_after: Some(until - now) },
            Some(e) => e.clone(),
        })
    }
}

/// Real searches of each item in its last day.
#[derive(Default)]
pub(crate) struct DayTries(HashMap<WishlistId, Tries>);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Tries {
    /// When the day began (unix ms): its first search.
    since: i64,
    n: u32,
}

impl DayTries {
    /// `id` searches now (`now`, unix ms): counted, or `Err` with when its
    /// day is over when it searched [`DAY_TRIES`] times in it.
    pub(crate) fn take(&mut self, id: WishlistId, now: i64) -> Result<(), i64> {
        let t = self.0.entry(id).or_insert(Tries { since: now, n: 0 });
        if now - t.since >= DAY_MS || now < t.since {
            *t = Tries { since: now, n: 0 };
        }
        if t.n >= DAY_TRIES {
            return Err(t.since + DAY_MS);
        }
        t.n += 1;
        Ok(())
    }

    /// The user asked for `id` now: its count starts over.
    pub(crate) fn forget(&mut self, id: WishlistId) {
        self.0.remove(&id);
    }
}

/// A [`SourceError`] as kept (the ones that say something of the source).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Why {
    Blocked(String),
    Wait,
    Http(u16),
    Unreachable(String),
    Parse(String),
}

impl Why {
    fn of(e: &SourceError) -> Option<Self> {
        Some(match e {
            SourceError::Blocked(s) => Self::Blocked(s.clone()),
            SourceError::RateLimited { .. } => Self::Wait,
            SourceError::Http { status } => Self::Http(*status),
            SourceError::Unreachable(s) => Self::Unreachable(s.clone()),
            SourceError::Parse(s) => Self::Parse(s.clone()),
            SourceError::NotConfigured | SourceError::NothingToFetch | SourceError::Io(_) => return None,
        })
    }

    /// The error again; a wait's length comes from the pause it started.
    fn error(&self) -> SourceError {
        match self {
            Self::Blocked(s) => SourceError::Blocked(s.clone()),
            Self::Wait => SourceError::RateLimited { retry_after: None },
            Self::Http(status) => SourceError::Http { status: *status },
            Self::Unreachable(s) => SourceError::Unreachable(s.clone()),
            Self::Parse(s) => SourceError::Parse(s.clone()),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
struct Kept {
    key: SourceKey,
    times: u32,
    #[serde(default)]
    strikes: u32,
    /// Unix ms.
    until: i64,
    why: Option<Why>,
}

/// What [`KEY`] holds.
#[derive(Serialize, Deserialize, Default, Debug, PartialEq, Eq)]
pub(super) struct Saved {
    #[serde(default)]
    pauses: Vec<Kept>,
    #[serde(default)]
    aside: Vec<Kept>,
    #[serde(default)]
    pub(super) later: Vec<(WishlistId, u32)>,
    #[serde(default)]
    pub(super) retry_only: Vec<(WishlistId, Vec<SourceKey>)>,
    #[serde(default)]
    tries: Vec<(WishlistId, Tries)>,
}

/// `at` as unix ms, `now` being `now_ms`.
fn ms_of(at: Instant, now: Instant, now_ms: i64) -> i64 {
    if at >= now {
        now_ms.saturating_add((at - now).as_millis() as i64)
    } else {
        now_ms.saturating_sub((now - at).as_millis() as i64)
    }
}

/// Unix ms `ms` as an instant, when it is still to come.
fn instant_of(ms: i64, now: Instant, now_ms: i64) -> Option<Instant> {
    (ms > now_ms).then(|| now.checked_add(Duration::from_millis((ms - now_ms) as u64))).flatten()
}

/// A pause over for more than a day is forgotten: its count starts over.
fn stale(until: i64, now_ms: i64) -> bool {
    until + LONGEST_PAUSE.as_millis() as i64 <= now_ms
}

impl Saved {
    /// What is kept of the state at `now` (`now_ms`).
    pub(super) fn of(
        pauses: &Pauses,
        refusals: &Refusals,
        later: &HashMap<WishlistId, u32>,
        retry_only: &HashMap<WishlistId, Vec<SourceKey>>,
        tries: &DayTries,
        now: Instant,
        now_ms: i64,
    ) -> Self {
        let mut out = Self {
            pauses: pauses
                .0
                .iter()
                .filter_map(|(k, p)| {
                    Some(Kept {
                        key: *k,
                        times: p.times,
                        strikes: p.strikes,
                        until: ms_of(p.until?, now, now_ms),
                        why: p.why.as_ref().and_then(Why::of),
                    })
                })
                .collect(),
            aside: refusals
                .0
                .iter()
                .filter_map(|(k, r)| {
                    Some(Kept {
                        key: *k,
                        times: r.times,
                        strikes: 0,
                        until: ms_of(r.until?, now, now_ms),
                        why: r.why.as_ref().and_then(Why::of),
                    })
                })
                .collect(),
            later: later.iter().map(|(id, n)| (*id, *n)).collect(),
            retry_only: retry_only.iter().map(|(id, keys)| (*id, keys.clone())).collect(),
            tries: tries.0.iter().filter(|(_, t)| now_ms - t.since < DAY_MS).map(|(id, t)| (*id, *t)).collect(),
        };
        // One order for one state: the same state writes the same text.
        out.pauses.sort_by_key(|k| k.key);
        out.aside.sort_by_key(|k| k.key);
        out.later.sort_by_key(|(id, _)| *id);
        out.retry_only.sort_by_key(|(id, _)| *id);
        out.tries.sort_by_key(|(id, _)| *id);
        out
    }

    /// Back into the state at `now` (`now_ms`), what is past left out.
    pub(super) fn restore(self, now: Instant, now_ms: i64) -> Restored {
        let pauses = Pauses(
            self.pauses
                .into_iter()
                .filter(|k| may_pause(k.key) && !stale(k.until, now_ms))
                .map(|k| {
                    (k.key, Pause {
                        times: k.times,
                        strikes: k.strikes,
                        until: instant_of(k.until, now, now_ms),
                        why: k.why.as_ref().map(Why::error),
                    })
                })
                .collect(),
        );
        let refusals = Refusals(
            self.aside
                .into_iter()
                .filter(|k| !stale(k.until, now_ms))
                .map(|k| {
                    (k.key, Refused {
                        times: k.times,
                        until: instant_of(k.until, now, now_ms),
                        why: k.why.as_ref().map(Why::error),
                    })
                })
                .collect(),
        );
        let tries = DayTries(
            self.tries
                .into_iter()
                .filter(|(_, t)| now_ms >= t.since && now_ms - t.since < DAY_MS)
                .collect(),
        );
        Restored {
            pauses,
            refusals,
            later: self.later.into_iter().collect(),
            retry_only: self.retry_only.into_iter().collect(),
            tries,
        }
    }
}

/// The state [`Saved::restore`] gives back.
pub(super) struct Restored {
    pub(super) pauses: Pauses,
    pub(super) refusals: Refusals,
    pub(super) later: HashMap<WishlistId, u32>,
    pub(super) retry_only: HashMap<WishlistId, Vec<SourceKey>>,
    pub(super) tries: DayTries,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cf() -> SourceError {
        SourceError::Blocked(nikgob_sources::http::BOT_CHECK.into())
    }

    /// khinsider behind its bot check: one search sets it aside for hours,
    /// told as the check (not as a wait); an answer ends it.
    #[test]
    fn a_bot_check_sets_a_web_source_aside_for_long() {
        let (mut p, now) = (Pauses::default(), Instant::now());
        let until = p.failed(SourceKey::Khinsider, &cf(), now).expect("a pause");
        assert!(until - now >= Duration::from_secs(3 * 3600));
        assert_eq!(p.error(SourceKey::Khinsider, now), Some(cf()));
        // A search that set out before: the pause stands as it was.
        assert_eq!(p.failed(SourceKey::Khinsider, &cf(), now + Duration::from_secs(1)), None);
        assert_eq!(p.until(SourceKey::Khinsider, now), Some(until));
        // Once it is over, the next check doubles it.
        let again = p.failed(SourceKey::Khinsider, &cf(), until + Duration::from_secs(1)).unwrap();
        assert!(again - until >= Duration::from_secs(6 * 3600));
        assert!(p.answered(SourceKey::Khinsider));
        assert_eq!(p.until(SourceKey::Khinsider, now), None);
        // The user's own setup is asked as it is, and so is a refused login.
        assert_eq!(p.failed(SourceKey::Torrents, &cf(), now), None);
        assert_eq!(p.failed(SourceKey::Soulseek, &cf(), now), None);
        let login = SourceError::Blocked("the server refused the login: 40".into());
        assert_eq!(p.failed(SourceKey::Squidify, &login, now), None);
    }

    /// A Worker's day of requests: waited out whole, told as a wait with
    /// what is left; the same HTTP error twice in a row sets it aside too.
    #[test]
    fn a_wait_and_a_repeated_error_set_a_source_aside() {
        let (mut p, now) = (Pauses::default(), Instant::now());
        let day = SourceError::RateLimited { retry_after: Some(Duration::from_secs(5 * 3600)) };
        let until = p.failed(SourceKey::Qobuz, &day, now).unwrap();
        assert_eq!(until - now, Duration::from_secs(5 * 3600));
        let later = now + Duration::from_secs(3600);
        assert_eq!(p.error(SourceKey::Qobuz, later), Some(SourceError::RateLimited { retry_after: Some(Duration::from_secs(4 * 3600)) }));
        let after = until + Duration::from_secs(60);
        assert_eq!(p.failed(SourceKey::Qobuz, &SourceError::Http { status: 400 }, after), None, "once is not yet");
        assert!(p.failed(SourceKey::Qobuz, &SourceError::Http { status: 400 }, after).is_some());
        assert_eq!(p.error(SourceKey::Qobuz, after), Some(SourceError::Http { status: 400 }));
        assert_eq!(p.failed(SourceKey::Tidal, &SourceError::Parse("x".into()), now), None);
    }

    /// At most [`DAY_TRIES`] searches of an item in its day, then not until
    /// the day is over; the user asking starts the count over.
    #[test]
    fn an_item_searches_a_few_times_a_day() {
        let (mut t, id, t0) = (DayTries::default(), WishlistId::new(), 1_000_000_000_000i64);
        for i in 0..DAY_TRIES {
            assert_eq!(t.take(id, t0 + i as i64 * 60_000), Ok(()));
        }
        assert_eq!(t.take(id, t0 + 3_600_000), Err(t0 + DAY_MS));
        assert_eq!(t.take(id, t0 + DAY_MS), Ok(()), "a new day");
        t.forget(id);
        assert_eq!(t.take(id, t0 + DAY_MS + 1), Ok(()));
    }

    /// Kept and read back after a restart: the pauses, the sources set
    /// aside, the retries and the day's searches come back; what is a day
    /// past goes.
    #[test]
    fn the_pauses_outlive_a_restart() {
        let (now, now_ms) = (Instant::now(), 1_000_000_000_000i64);
        let mut pauses = Pauses::default();
        pauses.failed(SourceKey::Khinsider, &cf(), now);
        pauses.failed(SourceKey::Qobuz, &SourceError::RateLimited { retry_after: Some(Duration::from_secs(7200)) }, now);
        let mut refusals = Refusals::default();
        refusals.refused(SourceKey::Tidal, &SourceError::Http { status: 403 }, now);
        refusals.refused(SourceKey::Tidal, &SourceError::Http { status: 403 }, now);
        let (a, b) = (WishlistId::new(), WishlistId::new());
        let later = HashMap::from([(a, 3)]);
        let retry_only = HashMap::from([(a, vec![SourceKey::Khinsider])]);
        let mut tries = DayTries::default();
        tries.take(b, now_ms - 1000).unwrap();
        let saved = Saved::of(&pauses, &refusals, &later, &retry_only, &tries, now, now_ms);
        let json = serde_json::to_string(&saved).unwrap();
        let back: Saved = serde_json::from_str(&json).unwrap();
        assert_eq!(back, saved);
        // Ten minutes later, in a new run.
        let (now2, now2_ms) = (Instant::now(), now_ms + 600_000);
        let Restored { pauses: p, refusals: r, later: l, retry_only: o, tries: mut t } = back.restore(now2, now2_ms);
        let kh = p.until(SourceKey::Khinsider, now2).expect("still sits out");
        assert_eq!(kh - now2, Duration::from_secs(3 * 3600 - 600));
        assert!(nikgob_sources::http::is_check(&p.error(SourceKey::Khinsider, now2).unwrap()));
        let q = p.error(SourceKey::Qobuz, now2).unwrap();
        assert_eq!(q, SourceError::RateLimited { retry_after: Some(Duration::from_secs(7200 - 600)) });
        assert!(r.aside(SourceKey::Tidal, now2).is_some(), "still set aside");
        assert_eq!(r.why(SourceKey::Tidal), Some(&SourceError::Http { status: 403 }));
        assert_eq!((l.get(&a), o.get(&a)), (Some(&3), Some(&vec![SourceKey::Khinsider])));
        for _ in 1..DAY_TRIES {
            t.take(b, now2_ms).unwrap();
        }
        assert!(t.take(b, now2_ms).is_err(), "the day's count came back");
        // Two days later: the pauses are long over and forgotten.
        let late = now_ms + 2 * DAY_MS + 1;
        let Restored { pauses: p, refusals: r, tries: mut t, .. } = serde_json::from_str::<Saved>(&json).unwrap().restore(Instant::now(), late);
        assert!(p.0.is_empty() && r.0.is_empty());
        assert_eq!(t.take(b, late), Ok(()));
    }
}
