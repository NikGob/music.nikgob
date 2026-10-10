//! Pure seed-policy decision logic (no librqbit types, easy to test).

use std::time::Duration;

const GIB: u64 = 1024 * 1024 * 1024;

/// The subset of the client config that drives seeding decisions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolicyConfig {
    pub no_seed: bool,
    /// Uploaded / selected bytes at which to stop; 0 = no limit.
    pub seed_ratio: f32,
    /// Seed at most this many days after completion; 0 = no limit.
    pub seed_days: u32,
    /// Cap on bytes uploaded by all held torrents together; 0 = none.
    pub seed_limit_gb: u32,
}

/// One torrent as seen by the policy.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolicyItem {
    pub id: u64,
    /// Selected files are fully downloaded (and the torrent is healthy).
    pub complete: bool,
    /// Selected bytes.
    pub total: u64,
    /// Bytes uploaded over the torrent's lifetime (persisted across restarts).
    pub uploaded: u64,
    /// Time since completion, if known.
    pub completed_age: Option<Duration>,
}

/// Ids of torrents that should stop seeding (be removed from the session).
///
/// Only complete torrents are ever returned. The total-GB cap sums uploads of
/// every held torrent; once reached, all complete ones are stopped.
pub fn torrents_to_stop(items: &[PolicyItem], cfg: &PolicyConfig) -> Vec<u64> {
    let cap = u64::from(cfg.seed_limit_gb) * GIB;
    let over_cap = cap > 0 && items.iter().map(|i| i.uploaded).sum::<u64>() >= cap;
    let max_age = Duration::from_secs(u64::from(cfg.seed_days) * 86_400);

    items
        .iter()
        .filter(|i| i.complete)
        .filter(|i| {
            cfg.no_seed
                || over_cap
                || (cfg.seed_ratio > 0.0
                    && i.total > 0
                    && i.uploaded as f64 >= f64::from(cfg.seed_ratio) * i.total as f64)
                || (cfg.seed_days > 0 && i.completed_age.is_some_and(|age| age >= max_age))
        })
        .map(|i| i.id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFF: PolicyConfig = PolicyConfig {
        no_seed: false,
        seed_ratio: 0.0,
        seed_days: 0,
        seed_limit_gb: 0,
    };

    fn item(id: u64) -> PolicyItem {
        PolicyItem {
            id,
            complete: true,
            total: 1000,
            uploaded: 0,
            completed_age: Some(Duration::from_secs(10)),
        }
    }

    #[test]
    fn everything_off_stops_nothing() {
        assert!(torrents_to_stop(&[item(1), item(2)], &OFF).is_empty());
    }

    #[test]
    fn no_seed_stops_only_complete() {
        let cfg = PolicyConfig {
            no_seed: true,
            ..OFF
        };
        let mut downloading = item(2);
        downloading.complete = false;
        assert_eq!(torrents_to_stop(&[item(1), downloading], &cfg), vec![1]);
    }

    #[test]
    fn ratio_threshold() {
        let cfg = PolicyConfig {
            seed_ratio: 1.5,
            ..OFF
        };
        let mut a = item(1);
        a.uploaded = 1499;
        let mut b = item(2);
        b.uploaded = 1500;
        assert_eq!(torrents_to_stop(&[a, b], &cfg), vec![2]);
    }

    #[test]
    fn ratio_ignores_empty_selection() {
        let cfg = PolicyConfig {
            seed_ratio: 1.0,
            ..OFF
        };
        let mut a = item(1);
        a.total = 0;
        assert!(torrents_to_stop(&[a], &cfg).is_empty());
    }

    #[test]
    fn days_threshold_and_unknown_age() {
        let cfg = PolicyConfig {
            seed_days: 2,
            ..OFF
        };
        let mut old = item(1);
        old.completed_age = Some(Duration::from_secs(2 * 86_400));
        let mut young = item(2);
        young.completed_age = Some(Duration::from_secs(86_400));
        let mut unknown = item(3);
        unknown.completed_age = None;
        assert_eq!(torrents_to_stop(&[old, young, unknown], &cfg), vec![1]);
    }

    #[test]
    fn total_cap_counts_incomplete_uploads_but_stops_only_complete() {
        let cfg = PolicyConfig {
            seed_limit_gb: 1,
            ..OFF
        };
        let mut a = item(1);
        a.uploaded = GIB / 2;
        let mut b = item(2);
        b.complete = false;
        b.uploaded = GIB / 2;
        let c = item(3);
        assert_eq!(torrents_to_stop(&[a, b, c], &cfg), vec![1, 3]);
    }

    #[test]
    fn under_total_cap_keeps_all() {
        let cfg = PolicyConfig {
            seed_limit_gb: 1,
            ..OFF
        };
        let mut a = item(1);
        a.uploaded = GIB - 1;
        assert!(torrents_to_stop(&[a], &cfg).is_empty());
    }
}
