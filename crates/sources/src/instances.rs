//! Public instances of a free service: asked in
//! order, each at its own pace, the next one taking over when one is down,
//! turns us away or does not have the track; one that turned us away sits
//! out a while ([`Breaker`]), and the next call skips it until that pause
//! ends. The first that is not sitting out is the one that serves. The
//! list can change while the app runs (a service's own list of the
//! instances that work, [`Instances::replace`], or the user's
//! `sources.toml`, [`Instances::apply`]).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use nikgob_core::source::SourceError;

use crate::http::{Breaker, Pace};

struct State {
    list: Vec<(String, Arc<Breaker>)>,
    /// The user's own list: an uptime list does not replace it.
    own: bool,
}

pub struct Instances {
    state: Mutex<State>,
    pace: Pace,
}

fn base_of(url: &str) -> String {
    url.trim().trim_end_matches('/').to_owned()
}

fn bases(urls: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for u in urls {
        let u = base_of(u);
        if !u.is_empty() && !out.contains(&u) {
            out.push(u);
        }
    }
    out
}

fn with_breakers(urls: Vec<String>, kept: &[(String, Arc<Breaker>)]) -> Vec<(String, Arc<Breaker>)> {
    urls.into_iter()
        .map(|u| {
            let breaker = kept.iter().find(|(b, _)| *b == u).map_or_else(|| Arc::new(Breaker::default()), |(_, b)| b.clone());
            (u, breaker)
        })
        .collect()
}

impl Instances {
    /// `urls` empty: `defaults`. A non-empty `urls` is the user's own list.
    /// Trailing slashes go.
    pub fn new(urls: &[String], defaults: &[&str], every: Duration) -> Self {
        let own = urls.iter().any(|u| !u.trim().is_empty());
        Self::with_own(urls, defaults, own, every)
    }

    /// `own`: nothing but [`Self::apply`] replaces the list (an uptime list
    /// does not). An empty `urls` is `defaults`.
    pub fn with_own(urls: &[String], defaults: &[&str], own: bool, every: Duration) -> Self {
        let list = if urls.iter().any(|u| !u.trim().is_empty()) {
            bases(urls)
        } else {
            defaults.iter().map(|u| base_of(u)).filter(|u| !u.is_empty()).collect()
        };
        Self {
            state: Mutex::new(State { list: with_breakers(list, &[]), own }),
            pace: Pace::new(every),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn is_empty(&self) -> bool {
        self.lock().list.is_empty()
    }

    /// The instances asked, in order.
    pub fn urls(&self) -> Vec<String> {
        self.lock().list.iter().map(|(u, _)| u.clone()).collect()
    }

    /// The user set the list: nothing replaces it but [`Self::apply`].
    pub fn is_own(&self) -> bool {
        self.lock().own
    }

    /// The instance is sitting out (a quota, a dead token, a ban): a call
    /// skips it without a request.
    pub fn paused(&self, base: &str) -> bool {
        let base = base_of(base);
        self.lock().list.iter().any(|(u, b)| *u == base && b.check().is_err())
    }

    /// Notes what a request to `base` did, on that instance's pause only.
    pub fn note_at<T>(&self, base: &str, r: &Result<T, SourceError>) {
        let base = base_of(base);
        if let Some((_, b)) = self.lock().list.iter().find(|(u, _)| *u == base) {
            b.note(r);
        }
    }

    /// The pace between requests to these instances.
    pub async fn wait(&self) {
        self.pace.wait().await;
    }

    /// Asks `urls` from now on (an empty list changes nothing); an instance
    /// that stays keeps its pause. The user's own list is kept as it is.
    pub fn replace(&self, urls: &[String]) {
        let mut st = self.lock();
        if st.own {
            return;
        }
        let seen = bases(urls);
        if seen.is_empty() {
            return;
        }
        st.list = with_breakers(seen, &st.list);
    }

    /// The list from `sources.toml` (or the built-in one). An instance that
    /// stays keeps its pause. `own` is whether this list is the user's.
    pub fn apply(&self, urls: &[String], defaults: &[&str], own: bool) {
        let mut st = self.lock();
        let list = if urls.iter().any(|u| !u.trim().is_empty()) {
            bases(urls)
        } else {
            defaults.iter().map(|u| base_of(u)).filter(|u| !u.is_empty()).collect()
        };
        st.list = with_breakers(list, &st.list);
        st.own = own;
    }

    /// `op` on each instance in turn until one answers. One that is sitting
    /// out is skipped without a request. All failing: the first refusal
    /// (a block, a rate limit) if there was one, else the first error of an
    /// instance that answered (a 403 says more than the next one being
    /// gone), else the last error.
    pub async fn call<T, F, Fut>(&self, op: F) -> Result<T, SourceError>
    where
        F: Fn(String) -> Fut,
        Fut: Future<Output = Result<T, SourceError>>,
    {
        let mut refused: Option<SourceError> = None;
        let mut answered: Option<SourceError> = None;
        let mut last = SourceError::NotConfigured;
        let list = self.lock().list.clone();
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

    /// A daily quota (or a dead token) pauses that instance only. The next
    /// call does not ask it; the other one serves. The pause stays when
    /// the user's file reorders the list.
    #[tokio::test]
    async fn a_quota_pauses_one_instance_and_the_next_healthy_one_serves() {
        let i = Instances::new(&[], &["https://limited", "https://ok"], Duration::ZERO);
        let first = i
            .call(|base| async move {
                if base == "https://limited" {
                    Err(SourceError::RateLimited { retry_after: Some(Duration::from_secs(6 * 3600)) })
                } else {
                    Ok(base)
                }
            })
            .await;
        assert_eq!(first.unwrap(), "https://ok");
        assert!(i.paused("https://limited"));
        assert!(!i.paused("https://ok"));
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let asked2 = asked.clone();
        let second = i
            .call(move |base| {
                let asked2 = asked2.clone();
                async move {
                    asked2.lock().unwrap().push(base.clone());
                    Ok(base)
                }
            })
            .await;
        assert_eq!(second.unwrap(), "https://ok");
        assert_eq!(asked.lock().unwrap().as_slice(), ["https://ok"], "the paused one is not asked");
        i.apply(&["https://ok".into(), "https://limited/".into()], &["https://other"], true);
        assert!(i.is_own());
        assert!(i.paused("https://limited"), "a pause survives a new list that keeps the instance");
        i.replace(&["https://replaced".into()]);
        assert_eq!(i.urls(), ["https://ok", "https://limited"], "the user's list is not replaced");
        i.apply(&[], &["https://builtin"], false);
        assert_eq!(i.urls(), ["https://builtin"]);
        assert!(!i.is_own());
    }
}
