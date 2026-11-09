//! Public instances of a free service (docs/stage4.5.md §4): asked in
//! order, each at its own pace, the next one taking over when one is down,
//! turns us away or does not have the track; one that turned us away sits
//! out a while ([`Breaker`]). The list can change while the app runs (a
//! service's own list of the instances that work, [`Instances::replace`]).

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nikgob_core::source::SourceError;

use crate::http::{Breaker, Pace};

pub struct Instances {
    list: Mutex<Vec<(String, Arc<Breaker>)>>,
    pace: Pace,
    /// The user's own list (settings.toml): kept as it is.
    own: bool,
}

fn base_of(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

impl Instances {
    /// `urls` empty: `defaults`. Trailing slashes go.
    pub fn new(urls: &[String], defaults: &[&str], every: Duration) -> Self {
        let own = urls.iter().any(|u| !u.trim().is_empty());
        let list: Vec<String> = if own {
            urls.iter().map(|u| u.trim().to_owned()).filter(|u| !u.is_empty()).collect()
        } else {
            defaults.iter().map(|u| (*u).to_owned()).collect()
        };
        Self {
            list: Mutex::new(list.iter().map(|u| (base_of(u), Arc::new(Breaker::default()))).collect()),
            pace: Pace::new(every),
            own,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(String, Arc<Breaker>)>> {
        self.list.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// The instances asked, in order.
    pub fn urls(&self) -> Vec<String> {
        self.lock().iter().map(|(u, _)| u.clone()).collect()
    }

    /// The user set the list: nothing replaces it.
    pub fn is_own(&self) -> bool {
        self.own
    }

    /// Asks `urls` from now on (an empty list changes nothing); an instance
    /// that stays keeps its pause. The user's own list is kept as it is.
    pub fn replace(&self, urls: &[String]) {
        if self.own {
            return;
        }
        let mut seen: Vec<String> = Vec::new();
        for u in urls.iter().map(|u| base_of(u)).filter(|u| !u.is_empty()) {
            if !seen.contains(&u) {
                seen.push(u);
            }
        }
        if seen.is_empty() {
            return;
        }
        let mut list = self.lock();
        let next: Vec<(String, Arc<Breaker>)> = seen
            .into_iter()
            .map(|u| {
                let breaker = list.iter().find(|(b, _)| *b == u).map_or_else(|| Arc::new(Breaker::default()), |(_, b)| b.clone());
                (u, breaker)
            })
            .collect();
        *list = next;
    }

    /// `op` on each instance in turn until one answers. All failing: the
    /// first refusal (a block, a rate limit) if there was one, else the
    /// first error of an instance that answered (a 403 says more than the
    /// next one being gone), else the last error. An instance sitting out
    /// counts as what set it aside, without a request.
    pub async fn call<T, F, Fut>(&self, op: F) -> Result<T, SourceError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<T, SourceError>>,
    {
        let mut refused: Option<SourceError> = None;
        let mut answered: Option<SourceError> = None;
        let mut last = SourceError::NotConfigured;
        let list = self.lock().clone();
        for (base, breaker) in &list {
            let r = match breaker.check() {
                Err(e) => Err(e),
                Ok(()) => {
                    self.pace.wait().await;
                    let r = op(base.clone()).await;
                    breaker.note(&r);
                    if let Err(e) = &r {
                        log::info!("{base}: {e}");
                    }
                    r
                }
            };
            match r {
                Ok(v) => return Ok(v),
                Err(e) => {
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
        // It sits out now: refused without a request, told as what it was.
        let again = mine.call(|_| async { Ok(()) }).await;
        assert!(matches!(again, Err(SourceError::Blocked(_))));
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

    /// Names that are gone (no DNS) are asked twice, then left alone: the
    /// live one is asked at once, and its answer is what is told.
    #[tokio::test]
    async fn gone_instances_are_left_alone() {
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let i = Instances::new(&[], &["https://gone", "https://live"], Duration::ZERO);
        let op = |base: String| {
            let asked = asked.clone();
            async move {
                asked.lock().unwrap().push(base.clone());
                if base == "https://gone" {
                    Err::<(), _>(SourceError::Unreachable("dns error".into()))
                } else {
                    Err(SourceError::Http { status: 403 })
                }
            }
        };
        for _ in 0..4 {
            assert_eq!(i.call(op).await, Err(SourceError::Http { status: 403 }));
        }
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|b| *b == "https://gone").count(), 2, "{asked:?}");
    }

    /// A new list (the service's own list of what works) takes over; an
    /// instance on both keeps its pause; the user's own list stays.
    #[tokio::test]
    async fn a_new_list_keeps_the_pauses_of_the_instances_it_keeps() {
        let i = Instances::new(&[], &["https://a", "https://b"], Duration::ZERO);
        let _ = i.call(|_| async { Err::<(), _>(SourceError::Blocked("ban".into())) }).await;
        i.replace(&["https://c/".into(), "https://a".into(), "https://c".into()]);
        assert_eq!(i.urls(), ["https://c", "https://a"]);
        let got = i.call(|base| async move { if base == "https://c" { Err(SourceError::Http { status: 404 }) } else { Ok(base) } }).await;
        assert!(matches!(got, Err(SourceError::Blocked(_))), "a still sits out: {got:?}");
        i.replace(&[]);
        assert_eq!(i.urls().len(), 2, "an empty list changes nothing");
        let mine = Instances::new(&["https://mine".into()], &["https://a"], Duration::ZERO);
        mine.replace(&["https://other".into()]);
        assert_eq!(mine.urls(), ["https://mine"]);
    }
}
