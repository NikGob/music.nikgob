#![allow(dead_code)]

use std::time::Duration;

use nikgob_metadata::{Http, MetadataConfig, RateLimits, RetryPolicy};

pub const CONTACT: &str = "tests@example.invalid";

/// No rate limiting, fast retries: tests must not depend on wall-clock pacing.
pub fn http() -> Http {
    Http::new(&MetadataConfig::new(CONTACT))
        .unwrap()
        .with_rate_limits(RateLimits::unlimited())
        .with_retry_policy(RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        })
}
