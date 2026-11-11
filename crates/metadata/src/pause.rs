//! Sources that keep turning us away sit out for a while instead of being
//! asked for every song: a lyrics source whose tunnel is down (LyricsPlus,
//! `HTTP 530, error code: 1033`) or that wants a key it does not have
//! (BetterLyrics, `HTTP 401: API key required`) answered so on every track.
//!
//! [`STRIKES`] refusals in a row (a server error, 403, a rate limit that
//! outlasted the retries) pause the source for [`FIRST_PAUSE`], doubling
//! with each pause in a row up to [`LONGEST_PAUSE`]; 401 (a key is needed)
//! pauses it for [`LONGEST_PAUSE`] at once. After a pause one more refusal
//! starts the next one. Any answer (a text or a clean miss) ends it all.
//! A network failure or a timeout says nothing about the source: it neither
//! counts nor clears. Clones share one state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::error::MetadataError;

/// Refusals in a row before the first pause.
pub const STRIKES: u32 = 3;
pub const FIRST_PAUSE: Duration = Duration::from_secs(10 * 60);
pub const LONGEST_PAUSE: Duration = Duration::from_secs(24 * 60 * 60);

/// What one answer says about the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// It answered.
    Answered,
    /// It turned us away.
    Refused,
    /// It wants a key we do not have.
    NeedsKey,
    /// Nothing about the source (the network, a timeout, a bad body).
    Unknown,
}

impl Verdict {
    pub fn of<T>(result: &Result<T, MetadataError>) -> Self {
        match result {
            Ok(_) | Err(MetadataError::NotFound) => Self::Answered,
            Err(MetadataError::Status { status: 401, .. }) => Self::NeedsKey,
            Err(MetadataError::Status { status, .. }) if *status == 403 || *status >= 500 => Self::Refused,
            Err(MetadataError::RateLimited(_)) => Self::Refused,
            Err(_) => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Pause {
    /// Refusals in a row.
    strikes: u32,
    /// Pauses in a row.
    times: u32,
    until: Option<Instant>,
}

/// The pauses of the sources, by a source's key.
#[derive(Clone, Debug, Default)]
pub struct Pauses(Arc<Mutex<HashMap<&'static str, Pause>>>);

impl Pauses {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long `source` still sits out at `now` (`None`: it may be asked).
    pub fn paused_for(&self, source: &'static str, now: Instant) -> Option<Duration> {
        let map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(source)?.until.filter(|u| *u > now).map(|u| u - now)
    }

    /// Notes what `source` answered at `now`; the pause this starts, if it
    /// starts one.
    pub fn note(&self, source: &'static str, verdict: Verdict, now: Instant) -> Option<Duration> {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match verdict {
            Verdict::Unknown => None,
            Verdict::Answered => {
                map.remove(source);
                None
            }
            Verdict::Refused | Verdict::NeedsKey => {
                let p = map.entry(source).or_default();
                p.strikes += 1;
                if verdict == Verdict::Refused && p.strikes < STRIKES {
                    return None;
                }
                let pause = if verdict == Verdict::NeedsKey {
                    LONGEST_PAUSE
                } else {
                    FIRST_PAUSE.saturating_mul(1u32 << p.times.min(16)).min(LONGEST_PAUSE)
                };
                p.times += 1;
                // The next refusal after this pause starts the next one.
                p.strikes = STRIKES - 1;
                p.until = Some(now + pause);
                Some(pause)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(status: u16) -> Result<(), MetadataError> {
        Err(MetadataError::Status { status, body: String::new() })
    }

    #[test]
    fn verdicts() {
        assert_eq!(Verdict::of(&Ok::<_, MetadataError>(1)), Verdict::Answered);
        assert_eq!(Verdict::of::<()>(&Err(MetadataError::NotFound)), Verdict::Answered);
        assert_eq!(Verdict::of(&status(401)), Verdict::NeedsKey);
        assert_eq!(Verdict::of(&status(403)), Verdict::Refused);
        assert_eq!(Verdict::of(&status(530)), Verdict::Refused);
        assert_eq!(Verdict::of(&status(502)), Verdict::Refused);
        assert_eq!(Verdict::of(&status(400)), Verdict::Unknown);
        assert_eq!(Verdict::of::<()>(&Err(MetadataError::RateLimited(None))), Verdict::Refused);
        assert_eq!(Verdict::of::<()>(&Err(MetadataError::Network("timeout".into()))), Verdict::Unknown);
    }

    #[test]
    fn refusals_in_a_row_pause_with_a_growing_interval_until_an_answer() {
        let p = Pauses::new();
        let t0 = Instant::now();
        assert_eq!(p.note("lyricsplus", Verdict::Refused, t0), None);
        assert_eq!(p.note("lyricsplus", Verdict::Refused, t0), None);
        assert_eq!(p.paused_for("lyricsplus", t0), None);
        assert_eq!(p.note("lyricsplus", Verdict::Refused, t0), Some(FIRST_PAUSE));
        assert_eq!(p.paused_for("lyricsplus", t0), Some(FIRST_PAUSE));
        assert_eq!(p.paused_for("lrclib", t0), None, "other sources are asked");
        // Over: one more refusal pauses it again, twice as long.
        let t1 = t0 + FIRST_PAUSE;
        assert_eq!(p.paused_for("lyricsplus", t1), None);
        assert_eq!(p.note("lyricsplus", Verdict::Refused, t1), Some(FIRST_PAUSE * 2));
        // A network failure changes nothing.
        let t2 = t1 + FIRST_PAUSE * 2;
        assert_eq!(p.note("lyricsplus", Verdict::Unknown, t2), None);
        assert_eq!(p.note("lyricsplus", Verdict::Refused, t2), Some(FIRST_PAUSE * 4));
        // Capped at a day.
        let mut t = t2;
        for _ in 0..12 {
            t += LONGEST_PAUSE;
            assert!(p.note("lyricsplus", Verdict::Refused, t).unwrap() <= LONGEST_PAUSE);
        }
        assert_eq!(p.paused_for("lyricsplus", t), Some(LONGEST_PAUSE));
        // An answer ends it all.
        p.note("lyricsplus", Verdict::Answered, t);
        assert_eq!(p.paused_for("lyricsplus", t), None);
        assert_eq!(p.note("lyricsplus", Verdict::Refused, t), None);
    }

    #[test]
    fn a_source_that_needs_a_key_sits_out_a_day_at_once() {
        let p = Pauses::new();
        let t0 = Instant::now();
        assert_eq!(p.note("betterlyrics", Verdict::NeedsKey, t0), Some(LONGEST_PAUSE));
        assert_eq!(p.paused_for("betterlyrics", t0 + FIRST_PAUSE), Some(LONGEST_PAUSE - FIRST_PAUSE));
        // Clones share it.
        assert!(p.clone().paused_for("betterlyrics", t0).is_some());
    }
}
