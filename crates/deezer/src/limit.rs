//! How hard the service may ask Deezer. The public API allows 50 requests
//! per 5 s per IP; we stay far below: at most [`IN_FLIGHT`] requests at
//! once and about [`PER_SECOND`] a second (a small burst first, so the three
//! requests of a search go out together). When Deezer answers its quota
//! error (`error.code 4`) the whole service pauses for [`QUOTA_PAUSE`]:
//! every call fails at once with [`StreamError::RateLimited`] meanwhile and
//! [`Gate::paused_until`] says until when, which is what the runtime's
//! automatic re-asks wait for (docs/stage3.md §9).

use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use nikgob_core::streaming::StreamError;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::time::Instant;

/// Requests at once.
pub(crate) const IN_FLIGHT: usize = 4;
/// Requests a second, on average.
pub(crate) const PER_SECOND: f64 = 8.0;
/// Requests that may go out at once after a quiet while.
pub(crate) const BURST: f64 = 4.0;
/// The pause after a quota error: Deezer's window is 5 s.
pub(crate) const QUOTA_PAUSE: Duration = Duration::from_secs(5);

struct Bucket {
    tokens: f64,
    at: Instant,
}

pub(crate) struct Gate {
    flight: Semaphore,
    bucket: Mutex<Bucket>,
    pause: Mutex<Option<(Instant, SystemTime)>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Gate {
    pub(crate) fn new() -> Self {
        Self {
            flight: Semaphore::new(IN_FLIGHT),
            bucket: Mutex::new(Bucket {
                tokens: BURST,
                at: Instant::now(),
            }),
            pause: Mutex::new(None),
        }
    }

    /// How long the service is still paused.
    pub(crate) fn paused_for(&self) -> Option<Duration> {
        let p = lock(&self.pause);
        let (until, _) = (*p)?;
        until.checked_duration_since(Instant::now()).filter(|d| !d.is_zero())
    }

    /// Until when, for [`nikgob_core::streaming::StreamService::paused_until`].
    pub(crate) fn paused_until(&self) -> Option<SystemTime> {
        self.paused_for()?;
        lock(&self.pause).map(|(_, at)| at)
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

    /// Deezer said its quota is used up: everything waits [`QUOTA_PAUSE`].
    pub(crate) fn quota_hit(&self) -> Duration {
        let mut p = lock(&self.pause);
        let now = Instant::now();
        let until = now + QUOTA_PAUSE;
        if p.is_none_or(|(u, _)| u < until) {
            *p = Some((until, SystemTime::now() + QUOTA_PAUSE));
            log::info!("deezer: quota used up, pausing for {} s", QUOTA_PAUSE.as_secs());
        }
        QUOTA_PAUSE
    }

    /// Waits for a turn: a free slot and a token. Fails at once while
    /// paused (also when the pause starts while waiting).
    pub(crate) async fn turn(&self) -> Result<SemaphorePermit<'_>, StreamError> {
        self.check()?;
        let permit = self
            .flight
            .acquire()
            .await
            .map_err(|_| StreamError::Network("closed".into()))?;
        loop {
            let wait = {
                let mut b = lock(&self.bucket);
                let now = Instant::now();
                let refill = now.duration_since(b.at).as_secs_f64() * PER_SECOND;
                b.tokens = (b.tokens + refill).min(BURST);
                b.at = now;
                // A hair under one token counts (float rounding must not
                // turn into a spin of zero-length sleeps).
                if b.tokens >= 0.999 {
                    b.tokens = (b.tokens - 1.0).max(0.0);
                    None
                } else {
                    let w = Duration::from_secs_f64((1.0 - b.tokens) / PER_SECOND);
                    Some(w.max(Duration::from_millis(1)))
                }
            };
            match wait {
                None => break,
                Some(w) => tokio::time::sleep(w).await,
            }
        }
        self.check()?;
        Ok(permit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(start_paused = true)]
    async fn a_burst_goes_at_once_then_about_eight_a_second_and_four_at_most_in_flight() {
        let gate = Arc::new(Gate::new());
        let start = tokio::time::Instant::now();
        let mut handles = Vec::new();
        let (now_in, max_in) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        for _ in 0..20 {
            let (g, now_in, max_in) = (Arc::clone(&gate), Arc::clone(&now_in), Arc::clone(&max_in));
            handles.push(tokio::spawn(async move {
                let _p = g.turn().await.unwrap();
                let n = now_in.fetch_add(1, Ordering::SeqCst) + 1;
                max_in.fetch_max(n, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(300)).await;
                now_in.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert!(max_in.load(Ordering::SeqCst) <= IN_FLIGHT);
        // 20 requests of 300 ms, four at a time: at least 1.5 s; the rate
        // keeps it at ~8/s: well under 4 s.
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(1500), "{took:?}");
        assert!(took < Duration::from_secs(4), "{took:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_quota_error_pauses_everything_for_five_seconds() {
        let gate = Gate::new();
        assert!(gate.check().is_ok());
        gate.quota_hit();
        match gate.turn().await {
            Err(StreamError::RateLimited { retry_after: Some(d) }) => assert!(d <= QUOTA_PAUSE && d > Duration::from_secs(4)),
            other => panic!("{other:?}"),
        }
        assert!(gate.paused_until().is_some());
        tokio::time::advance(QUOTA_PAUSE + Duration::from_millis(10)).await;
        assert!(gate.paused_until().is_none());
        assert!(gate.turn().await.is_ok());
    }
}
