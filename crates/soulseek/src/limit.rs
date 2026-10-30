//! The upload speed limit: one token bucket shared by every upload, with a
//! rate that can change while uploads run.

use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;

pub struct Limiter {
    state: Mutex<Bucket>,
}

struct Bucket {
    /// Bytes per second; 0 = no limit.
    rate: u64,
    tokens: f64,
    at: Instant,
}

impl Limiter {
    pub fn new(kbps: u32) -> Self {
        Self { state: Mutex::new(Bucket { rate: u64::from(kbps) * 1024, tokens: 0.0, at: Instant::now() }) }
    }

    pub fn set_kbps(&self, kbps: u32) {
        let mut b = self.state.lock();
        b.rate = u64::from(kbps) * 1024;
        b.tokens = b.tokens.min(b.rate as f64 / 4.0);
    }

    /// The most bytes to send at once under the current rate (a quarter of
    /// a second's worth, so a low limit still sends smoothly).
    pub fn chunk(&self, max: usize) -> usize {
        let rate = self.state.lock().rate;
        if rate == 0 { max } else { (rate as usize / 4).clamp(1024, max) }
    }

    /// Waits until `n` bytes may be sent.
    pub async fn take(&self, n: usize) {
        loop {
            let wait = {
                let mut b = self.state.lock();
                if b.rate == 0 {
                    return;
                }
                let now = Instant::now();
                let burst = b.rate as f64 / 4.0;
                b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * b.rate as f64).min(burst.max(n as f64));
                b.at = now;
                if b.tokens >= n as f64 {
                    b.tokens -= n as f64;
                    return;
                }
                Duration::from_secs_f64((n as f64 - b.tokens) / b.rate as f64)
            };
            tokio::time::sleep(wait.max(Duration::from_millis(5))).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn holds_the_rate() {
        let l = Limiter::new(100);
        let start = Instant::now();
        let mut sent = 0;
        while sent < 500 * 1024 {
            let n = l.chunk(64 * 1024);
            l.take(n).await;
            sent += n;
        }
        let secs = start.elapsed().as_secs_f64();
        assert!((4.5..5.5).contains(&secs), "{secs}");
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn zero_is_unlimited() {
        let l = Limiter::new(0);
        let start = Instant::now();
        l.take(10 << 20).await;
        assert_eq!(start.elapsed(), Duration::ZERO);
    }
}
