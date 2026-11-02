//! Public instances of a free service (docs/stage4.5.md §4): asked in
//! order, each at its own pace, the next one taking over when one is down,
//! turns us away or does not have the track; one that turned us away sits
//! out a while ([`Breaker`]).

use std::future::Future;
use std::time::Duration;

use nikgob_core::source::SourceError;

use crate::http::{Breaker, Pace};

pub struct Instances {
    list: Vec<(String, Breaker)>,
    pace: Pace,
}

impl Instances {
    /// `urls` empty: `defaults`. Trailing slashes go.
    pub fn new(urls: &[String], defaults: &[&str], every: Duration) -> Self {
        let list: Vec<String> = if urls.iter().any(|u| !u.trim().is_empty()) {
            urls.iter().map(|u| u.trim().to_owned()).filter(|u| !u.is_empty()).collect()
        } else {
            defaults.iter().map(|u| (*u).to_owned()).collect()
        };
        Self {
            list: list.into_iter().map(|u| (u.trim_end_matches('/').to_owned(), Breaker::default())).collect(),
            pace: Pace::new(every),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// `op` on each instance in turn until one answers. All failing: the
    /// first refusal (a block, a rate limit) if there was one, else the
    /// first error of an instance that answered (a 403 says more than the
    /// next one being gone), else the last error.
    pub async fn call<T, F, Fut>(&self, op: F) -> Result<T, SourceError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<T, SourceError>>,
    {
        let mut refused: Option<SourceError> = None;
        let mut answered: Option<SourceError> = None;
        let mut last = SourceError::NotConfigured;
        for (base, breaker) in &self.list {
            if let Err(e) = breaker.check() {
                refused.get_or_insert(e);
                continue;
            }
            self.pace.wait().await;
            let r = op(base.clone()).await;
            breaker.note(&r);
            match r {
                Ok(v) => return Ok(v),
                Err(e) => {
                    log::info!("{base}: {e}");
                    if matches!(e, SourceError::Blocked(_) | SourceError::RateLimited { .. }) {
                        refused.get_or_insert(e.clone());
                    } else if !matches!(e, SourceError::Unreachable(_)) {
                        answered.get_or_insert(e.clone());
                    }
                    last = e;
                }
            }
        }
        Err(refused.or(answered).unwrap_or(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_next_instance_takes_over() {
        let i = Instances::new(&[], &["https://a", "https://b/"], Duration::ZERO);
        let got = i
            .call(|base| async move {
                if base == "https://a" { Err(SourceError::Http { status: 502 }) } else { Ok(base) }
            })
            .await;
        assert_eq!(got.unwrap(), "https://b");
        let mine = Instances::new(&["https://mine".into()], &["https://a"], Duration::ZERO);
        let all_down = mine.call(|_| async { Err::<(), _>(SourceError::Blocked("ban".into())) }).await;
        assert!(matches!(all_down, Err(SourceError::Blocked(_))));
        // It sits out now: refused without a request.
        let again = mine.call(|_| async { Ok(()) }).await;
        assert!(matches!(again, Err(SourceError::RateLimited { .. })));
    }

    /// An instance that only finds (403 on every file) is set aside after
    /// a second 403: the next call goes to the others without asking it.
    #[tokio::test]
    async fn an_instance_refusing_again_and_again_is_set_aside() {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let i = Instances::new(&[], &["https://finds-only", "https://works"], Duration::ZERO);
        let op = |base: String| {
            let asked = asked.clone();
            async move {
                asked.lock().unwrap().push(base.clone());
                if base == "https://finds-only" { Err(SourceError::Http { status: 403 }) } else { Ok(base) }
            }
        };
        for _ in 0..3 {
            assert_eq!(i.call(op).await.unwrap(), "https://works");
        }
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|b| *b == "https://finds-only").count(), 2, "{asked:?}");
        assert_eq!(asked.iter().filter(|b| *b == "https://works").count(), 3);
    }

    /// The one live instance said 403 and the others had no DNS: the 403
    /// is what went wrong, not the last one being gone.
    #[tokio::test]
    async fn the_error_of_an_instance_that_answered_is_told() {
        let i = Instances::new(&[], &["https://live", "https://gone-1", "https://gone-2"], Duration::ZERO);
        let got = i
            .call(|base| async move {
                if base == "https://live" {
                    Err::<(), _>(SourceError::Http { status: 403 })
                } else {
                    Err(SourceError::Unreachable("dns error".into()))
                }
            })
            .await;
        assert!(matches!(got, Err(SourceError::Http { status: 403 })), "{got:?}");
        let all_gone = Instances::new(&[], &["https://a", "https://b"], Duration::ZERO);
        let got = all_gone.call(|_| async { Err::<(), _>(SourceError::Unreachable("dns error".into())) }).await;
        assert!(matches!(got, Err(SourceError::Unreachable(_))));
    }
}
