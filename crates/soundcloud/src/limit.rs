//! How hard the service may ask SoundCloud: a few catalogue calls at once,
//! signed-in calls one at a time, background starts spaced out with jitter,
//! and a pause of the whole service after a rate limit or a bot check
//! (DataDome; 5 minutes, doubling up to an hour while it keeps happening),
//! during which every call fails at once instead of making things worse.
//!
//! The user's own search ([`Lane::Interactive`]) is not spaced (its four
//! category requests go out together) but obeys the pause. Audio segments
//! do not go through the gate: they come from SoundCloud's media CDN, not
//! from api-v2.

use std::future::Future;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use nikgob_core::streaming::StreamError;
use tokio::sync::{Semaphore, SemaphorePermit};

/// Catalogue calls (search, albums, artists, playlists, tracks) at once.
pub(crate) const CATALOG_IN_FLIGHT: usize = 3;
/// Calls with the user's cookies at once.
pub(crate) const SIGNED_IN_FLIGHT: usize = 1;
/// Background starts are this far apart, plus up to [`SPACING_JITTER`].
pub(crate) const SPACING: Duration = Duration::from_millis(200);
pub(crate) const SPACING_JITTER: Duration = Duration::from_millis(200);
/// The first pause after a rate limit, and the longest.
pub(crate) const FIRST_PAUSE: Duration = Duration::from_secs(5 * 60);
pub(crate) const MAX_PAUSE: Duration = Duration::from_secs(3600);
/// A rate limit this long after the last one starts again from
/// [`FIRST_PAUSE`].
const CALM_AFTER: Duration = Duration::from_secs(2 * 3600);

/// What kind of call waits for its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lane {
    /// The user's search: no spacing, its own concurrency.
    Interactive,
    /// Catalogue reads.
    Catalog,
    /// Calls with the login (likes, the user's playlists, edits, history).
    SignedIn,
}

#[derive(Default)]
struct Pause {
    until: Option<(Instant, SystemTime)>,
    level: Duration,
    last: Option<Instant>,
}

pub(crate) struct Gate {
    catalog: Semaphore,
    signed: Semaphore,
    next_start: Mutex<Instant>,
    pause: Mutex<Pause>,
    spacing: Duration,
    jitter: Duration,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Gate {
    pub(crate) fn new() -> Self {
        Self::with_spacing(SPACING, SPACING_JITTER)
    }

    pub(crate) fn with_spacing(spacing: Duration, jitter: Duration) -> Self {
        Self {
            catalog: Semaphore::new(CATALOG_IN_FLIGHT),
            signed: Semaphore::new(SIGNED_IN_FLIGHT),
            next_start: Mutex::new(Instant::now()),
            pause: Mutex::new(Pause::default()),
            spacing,
            jitter,
        }
    }

    /// How long the service is still paused.
    pub(crate) fn paused_for(&self) -> Option<Duration> {
        let p = lock(&self.pause);
        p.until.and_then(|(at, _)| at.checked_duration_since(Instant::now()))
    }

    /// Until when the service is paused (for the view).
    pub(crate) fn paused_until(&self) -> Option<SystemTime> {
        self.paused_for()?;
        lock(&self.pause).until.map(|(_, at)| at)
    }

    /// Fails at once while paused.
    pub(crate) fn check(&self) -> Result<(), StreamError> {
        match self.paused_for() {
            Some(left) => Err(StreamError::RateLimited {
                retry_after: Some(left),
            }),
            None => Ok(()),
        }
    }

    /// SoundCloud said "slow down" (or showed its bot check): the whole
    /// service pauses.
    pub(crate) fn trip(&self) {
        let mut p = lock(&self.pause);
        let now = Instant::now();
        if p.until.is_some_and(|(at, _)| at > now) {
            return; // already paused: one limit, many answers
        }
        p.level = match p.last {
            Some(last) if now.duration_since(last) < CALM_AFTER && !p.level.is_zero() => {
                (p.level * 2).min(MAX_PAUSE)
            }
            _ => FIRST_PAUSE,
        };
        p.last = Some(now);
        p.until = Some((now + p.level, SystemTime::now() + p.level));
        log::warn!(
            "soundcloud: rate limited or bot check; every call pauses for {} min",
            p.level.as_secs() / 60
        );
    }

    /// Looks at a result: a rate limit pauses the service.
    pub(crate) fn note<T>(&self, r: &Result<T, StreamError>) {
        if matches!(r, Err(StreamError::RateLimited { .. })) {
            self.trip();
        }
    }

    async fn pace(&self) {
        let wait = {
            let mut next = lock(&self.next_start);
            let now = Instant::now();
            let at = (*next).max(now);
            let jitter = if self.jitter.is_zero() {
                Duration::ZERO
            } else {
                Duration::from_millis(fastrand::u64(0..=self.jitter.as_millis() as u64))
            };
            *next = at + self.spacing + jitter;
            at - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Waits for a turn in `lane` (fails at once while paused).
    async fn enter(&self, lane: Lane) -> Result<Option<SemaphorePermit<'_>>, StreamError> {
        self.check()?;
        let permit = match lane {
            Lane::Interactive => None,
            Lane::Catalog => Some(self.catalog.acquire().await),
            Lane::SignedIn => Some(self.signed.acquire().await),
        };
        if lane != Lane::Interactive {
            self.pace().await;
        }
        // Paused while this call queued.
        self.check()?;
        permit.map(|p| p.map_err(|_| StreamError::Unavailable)).transpose()
    }

    /// Runs one call in `lane`: waits for its turn, pauses the service when
    /// it is rate limited.
    pub(crate) async fn run<T>(
        &self,
        lane: Lane,
        f: impl Future<Output = Result<T, StreamError>>,
    ) -> Result<T, StreamError> {
        let _permit = self.enter(lane).await?;
        let r = f.await;
        self.note(&r);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(flavor = "multi_thread")]
    async fn a_rate_limit_pauses_everything_and_the_pause_doubles() {
        let gate = Gate::with_spacing(Duration::ZERO, Duration::ZERO);
        let calls = AtomicUsize::new(0);
        let limited = gate
            .run(Lane::Catalog, async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(StreamError::RateLimited { retry_after: None })
            })
            .await;
        assert!(limited.is_err());
        let left = gate.paused_for().unwrap();
        assert!(left > Duration::from_secs(290) && left <= FIRST_PAUSE);
        assert!(gate.paused_until().is_some());
        // Every lane fails at once, without calling.
        for lane in [Lane::Interactive, Lane::Catalog, Lane::SignedIn] {
            let r = gate
                .run(lane, async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await;
            assert!(matches!(r, Err(StreamError::RateLimited { retry_after: Some(_) })));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The next limit (soon after) pauses twice as long, up to an hour.
        lock(&gate.pause).until = None;
        gate.trip();
        assert!(gate.paused_for().unwrap() > Duration::from_secs(590));
        for _ in 0..6 {
            lock(&gate.pause).until = None;
            gate.trip();
        }
        assert!(gate.paused_for().unwrap() <= MAX_PAUSE);
        assert!(gate.paused_for().unwrap() > MAX_PAUSE - Duration::from_secs(5));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn catalogue_and_signed_in_calls_are_bounded_and_spaced() {
        let gate = Arc::new(Gate::with_spacing(Duration::from_millis(40), Duration::ZERO));
        let now = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let started = Instant::now();
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (gate, now, peak) = (gate.clone(), now.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                gate.run(Lane::SignedIn, async {
                    let n = now.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    now.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, StreamError>(())
                })
                .await
            }));
        }
        for t in tasks {
            t.await.unwrap().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1, "signed-in calls one at a time");
        assert!(started.elapsed() >= Duration::from_millis(7 * 40), "spaced");
    }
}
