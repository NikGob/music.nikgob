//! When the streaming runtime talks to the services by itself (docs/
//! stage3.md §1; the stage 3 request audit): like pulls, playlist checks,
//! the like queue, look-ups. Every periodic job runs with jitter, and the
//! services are staggered, so nothing fires in lockstep (the shared Spotify
//! app's quota is everyone's); while the window is hidden and nothing
//! plays, the periodic network work waits.

use std::time::Duration;

/// Tries of a rate-limited background job before it waits for its normal
/// interval (see [`StreamSchedule::rate_limit_retry`]).
pub(crate) const RATE_LIMIT_RETRIES: u32 = 3;

/// 0–500 ms, so retries woken by the same pause do not fire together.
pub(crate) fn wake_jitter() -> Duration {
    Duration::from_millis(fastrand::u64(0..=500))
}

/// The intervals; [`Default`] is what the app runs. Tests make them short
/// (see [`crate::Runtime::start_scheduled`]).
#[derive(Clone, Debug)]
pub struct StreamSchedule {
    /// A service's liked list is checked this often (± [`Self::jitter`]):
    /// one request when nothing changed.
    pub likes_every: Duration,
    /// On window focus or a playback start, a like check older than this
    /// runs at once.
    pub likes_fresh: Duration,
    /// The whole liked list is read at most this often (else only its
    /// first page), and whenever the counts do not add up.
    pub likes_full_every: Duration,
    /// The linked playlists are checked this often (± jitter)…
    pub sync_every: Duration,
    /// …first this long after the start (somewhere between the two)…
    pub sync_first: (Duration, Duration),
    /// …and each further service this much later than the one before.
    pub sync_stagger: Duration,
    /// A playlist whose service has no versions (YouTube; or someone else's
    /// playlist not in the listing) is fetched again after somewhere
    /// between these.
    pub stale: (Duration, Duration),
    /// A like / unlike waits this long (+ up to 0.5 s) to be sent together
    /// with the next ones; a like and its unlike cancel out.
    pub like_delay: Duration,
    /// The wishlist scheduler runs on events; this is its fallback.
    pub wish_every: Duration,
    /// A YouTube Music track that was not found is not looked up again for
    /// this long (kept in the library, so restarts do not ask again).
    pub miss_ttl: Duration,
    /// Pages of the services kept: albums, artists, playlists (a playlist
    /// also until its version changes).
    pub album_ttl: Duration,
    pub artist_ttl: Duration,
    pub playlist_ttl: Duration,
    /// Imported tracks are completed from Spotify / YouTube a batch at a
    /// time with this pause between batches.
    pub enrich_pause: (Duration, Duration),
    /// Liked YouTube clips are asked for their songs one at a time, this
    /// far apart (± jitter; see `clips`)…
    pub clip_gap: Duration,
    /// …and the clips left are looked for in the library at most this often.
    pub clip_refill: Duration,
    /// The sound analysis of the files starts this long after the start
    /// (playback and the first screens go first)…
    pub analysis_start: Duration,
    /// …and, with nothing to analyse, looks at the library again this often
    /// (imports wake it at once; the stream analysis is woken by changes
    /// of likes and playlists).
    pub analysis_idle: Duration,
    /// Stream-only tracks are downloaded for their analysis one at a time,
    /// this far apart (± 25 %)…
    pub analysis_gap: Duration,
    /// …at most this many a day.
    pub analysis_daily: u32,
    /// The recommendations are built again this long after the library
    /// changed (likes, playlists, imports, «Не рекомендовать»)…
    pub recs_debounce: Duration,
    /// …and at least this often anyway (the day part, decayed weights).
    pub recs_every: Duration,
    /// «Новое для тебя» asks the services one request at a time, somewhere
    /// between these apart…
    pub discover_gap: (Duration, Duration),
    /// …and looks for new music again this long after the last pass (and
    /// on a new day).
    pub discover_every: Duration,
    /// The graph of similar music (docs/recs-graph.md §1) asks each source
    /// one request at a time, every source at once: YouTube Music somewhere
    /// between these apart…
    pub graph_gap_youtube: (Duration, Duration),
    /// …Deezer between these…
    pub graph_gap_deezer: (Duration, Duration),
    /// …and the open data (MusicBrainz, ListenBrainz, Last.fm) between these
    /// on top of their clients' own limits (MusicBrainz one a second).
    pub graph_gap_open: (Duration, Duration),
    /// It plans this many requests of a source at a time, the next ones as
    /// soon as they are asked, until the library is covered…
    pub graph_chunk: usize,
    /// …then looks for what went stale this often.
    pub graph_every: Duration,
    /// What it keeps is built in at most this often while it goes on (and
    /// once when it is over).
    pub graph_build_every: Duration,
    /// The services' own suggestions (docs/stage4.md §12.3) are read again
    /// this long after the last time, per signed-in service…
    pub suggest_every: Duration,
    /// …the first time this long after the start (playback and the first
    /// screens go first).
    pub suggest_first: Duration,
    /// The worker's slow tick (toasts, the session file, the jobs above).
    pub tick: Duration,
    /// ± this share of every periodic interval.
    pub jitter: f64,
    /// A like check or playlist check that a service rate-limited tries
    /// again when the pause ends (+ up to 0.5 s), then after 2× and 4× the
    /// pause, at most this far away; after those the normal interval.
    pub rate_limit_cap: Duration,
    /// The pause assumed when the service did not say how long it is.
    pub rate_limit_unknown: Duration,
}

impl Default for StreamSchedule {
    fn default() -> Self {
        Self {
            likes_every: Duration::from_secs(45 * 60),
            likes_fresh: Duration::from_secs(10 * 60),
            likes_full_every: Duration::from_secs(24 * 3600),
            sync_every: Duration::from_secs(25 * 60),
            sync_first: (Duration::from_secs(2 * 60), Duration::from_secs(5 * 60)),
            sync_stagger: Duration::from_secs(60),
            stale: (Duration::from_secs(6 * 3600), Duration::from_secs(12 * 3600)),
            like_delay: Duration::from_millis(1500),
            wish_every: Duration::from_secs(60),
            miss_ttl: Duration::from_secs(5 * 24 * 3600),
            album_ttl: Duration::from_secs(24 * 3600),
            artist_ttl: Duration::from_secs(6 * 3600),
            playlist_ttl: Duration::from_secs(30 * 60),
            enrich_pause: (Duration::from_secs(60), Duration::from_secs(20)),
            clip_gap: Duration::from_secs(4),
            clip_refill: Duration::from_secs(30 * 60),
            analysis_start: Duration::from_secs(15),
            analysis_idle: Duration::from_secs(5 * 60),
            analysis_gap: Duration::from_secs(20),
            analysis_daily: 300,
            recs_debounce: Duration::from_secs(3),
            recs_every: Duration::from_secs(3 * 3600),
            discover_gap: (Duration::from_secs(2), Duration::from_secs(4)),
            discover_every: Duration::from_secs(24 * 3600),
            graph_gap_youtube: (Duration::from_millis(750), Duration::from_millis(1500)),
            graph_gap_deezer: (Duration::from_millis(250), Duration::from_millis(500)),
            graph_gap_open: (Duration::ZERO, Duration::from_millis(250)),
            graph_chunk: 240,
            graph_every: Duration::from_secs(2 * 3600),
            graph_build_every: Duration::from_secs(2 * 60),
            suggest_every: Duration::from_secs(6 * 3600),
            suggest_first: Duration::from_secs(90),
            tick: Duration::from_secs(5),
            jitter: 0.2,
            rate_limit_cap: Duration::from_secs(10 * 60),
            rate_limit_unknown: Duration::from_secs(30),
        }
    }
}

impl StreamSchedule {
    /// `base` ± [`Self::jitter`].
    pub(crate) fn jittered(&self, base: Duration) -> Duration {
        let j = self.jitter.clamp(0.0, 0.9);
        if j == 0.0 {
            return base;
        }
        base.mul_f64(1.0 - j + 2.0 * j * fastrand::f64())
    }

    /// Somewhere between `a` and `b`.
    pub(crate) fn between((a, b): (Duration, Duration)) -> Duration {
        if b <= a {
            return a;
        }
        a + (b - a).mul_f64(fastrand::f64())
    }

    /// A fixed point between `a` and `b` for `key` (so a playlist keeps its
    /// own staleness instead of a new dice roll on every check).
    pub(crate) fn between_for(key: &str, (a, b): (Duration, Duration)) -> Duration {
        if b <= a {
            return a;
        }
        let h = key
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, c| (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3));
        a + (b - a).mul_f64((h % 10_000) as f64 / 10_000.0)
    }

    /// When a background job a service rate-limited tries again: after the
    /// `failures`-th limit in a row (1, 2, 3) the pause times 1, 2, 4
    /// (never sooner than `retry_after`, at most [`Self::rate_limit_cap`]),
    /// plus up to 0.5 s; `None` past [`RATE_LIMIT_RETRIES`] (the normal
    /// interval again).
    pub(crate) fn rate_limit_retry(&self, retry_after: Option<Duration>, failures: u32) -> Option<Duration> {
        if failures == 0 || failures > RATE_LIMIT_RETRIES {
            return None;
        }
        let pause = retry_after.unwrap_or(self.rate_limit_unknown);
        let backoff = pause.saturating_mul(1 << (failures - 1)).min(self.rate_limit_cap).max(pause);
        Some(backoff + wake_jitter())
    }

    pub(crate) fn like_wait(&self) -> Duration {
        self.like_delay + Duration::from_millis(fastrand::u64(0..=500))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_stay_within_their_jitter() {
        let s = StreamSchedule::default();
        for _ in 0..200 {
            let d = s.jittered(s.likes_every);
            assert!(d >= Duration::from_secs(36 * 60) && d <= Duration::from_secs(54 * 60));
            let f = StreamSchedule::between(s.sync_first);
            assert!(f >= s.sync_first.0 && f <= s.sync_first.1);
        }
        for (n, x) in [(1, 6), (2, 12), (3, 24)] {
            let d = s.rate_limit_retry(Some(Duration::from_secs(6)), n).unwrap();
            assert!(d >= Duration::from_secs(x) && d <= Duration::from_millis(x * 1000 + 500), "{n}: {d:?}");
        }
        assert_eq!(s.rate_limit_retry(Some(Duration::from_secs(6)), 4), None, "then the normal interval");
        let capped = s.rate_limit_retry(Some(Duration::from_secs(400)), 3).unwrap();
        assert!(capped >= s.rate_limit_cap && capped <= s.rate_limit_cap + Duration::from_millis(500));
        let a = StreamSchedule::between_for("spotify:x", s.stale);
        assert_eq!(a, StreamSchedule::between_for("spotify:x", s.stale));
        assert!(a >= s.stale.0 && a <= s.stale.1);
    }
}
