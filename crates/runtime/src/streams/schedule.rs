//! When the streaming runtime talks to the services by itself (docs/
//! stage3.md §1; the stage 3 request audit): like pulls, playlist checks,
//! the like queue, look-ups. Every periodic job runs with jitter, and the
//! services are staggered, so nothing fires in lockstep (the shared Spotify
//! app's quota is everyone's); while the window is hidden and nothing
//! plays, the periodic network work waits.

use std::time::Duration;

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
    /// The worker's slow tick (toasts, the session file, the jobs above).
    pub tick: Duration,
    /// ± this share of every periodic interval.
    pub jitter: f64,
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
            tick: Duration::from_secs(5),
            jitter: 0.2,
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
        let a = StreamSchedule::between_for("spotify:x", s.stale);
        assert_eq!(a, StreamSchedule::between_for("spotify:x", s.stale));
        assert!(a >= s.stale.0 && a <= s.stale.1);
    }
}
