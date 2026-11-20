//! «Барахолка» (`SourceKey::Junkyard`): the grey ripper sites of the
//! Lucida kind, one source over several backends asked in turn. A backend
//! behind a captcha or a Cloudflare check is skipped for a while, never
//! passed.
//!
//! A backend is one protocol, not one site: the Qobuz-DL one
//! (`/api/get-music`, `/api/get-album`, `/api/download-music`), the
//! hifi-api one (`/search/`, `/album/`, `/track/`) and the Rhythmax one
//! (Tidal's own catalogue and a DASH manifest of FLAC frames, in
//! `rhythmax.rs`). The first two are protocols [`crate::qobuz`] and
//! [`crate::tidal`] already speak, so those backends hand their work to
//! one of those sources with the list `sources.toml` gives them
//! ([`Sites`]): the pace, the per-site pauses, the daily quota and the
//! dead token are theirs, and one backend sitting out never stops the
//! others.
//!
//! The sites of this class come and go and most of them answer with a
//! check, so only the one that was proven to answer without one is built
//! in ([`DEFAULT_RHYTHMAX`]); the others are the user's to name in
//! `sources.toml`, and a kind with no address is not asked at all.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::source::{Fetched, Offer, Progress, Source, SourceError, SourceKey, Want};

use crate::matching;
use crate::qobuz;
use crate::tidal;

mod rhythmax;

/// The Qobuz-DL sites built in: none. A site of this class that answers
/// without a check is the user's to name in `sources.toml`.
pub const DEFAULT_QOBUZ_DL: &[&str] = &[];
/// The hifi-api sites built in: none, for the same reason.
pub const DEFAULT_HIFI: &[&str] = &[];
/// The Rhythmax site built in: the one that answered search and gave a
/// file without a check (22 Tidal accounts, 17 of them active).
pub const DEFAULT_RHYTHMAX: &[&str] = &["https://hifi.odskyler.com"];

/// One request to one site per this, as the other free services keep.
const PACE: Duration = Duration::from_millis(1500);

/// The sites each backend kind is asked at: `sources.toml`'s `[junkyard]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sites {
    /// Qobuz-DL instances, without `/api` at the end.
    pub qobuz_dl: Vec<String>,
    /// hifi-api instances.
    pub hifi: Vec<String>,
    /// Rhythmax (odskyler) bases.
    pub rhythmax: Vec<String>,
}

/// One protocol of the grey sites: its own sites, its own pauses, and the
/// way to search and fetch through it.
trait Backend: Send + Sync {
    /// The site name an offer shows.
    fn name(&self) -> &'static str;

    /// The prefix of the handles this backend makes.
    fn key(&self) -> &'static str;

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>>;

    fn fetch<'a>(
        &'a self,
        offer: &'a Offer,
        want: &'a Want,
        dest: &'a Path,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Fetched, SourceError>>;
}

/// A backend whose protocol one of the free services already speaks: the
/// work is theirs, and the offers are told as this source's, carrying the
/// backend's name and key.
struct Delegated {
    key: &'static str,
    name: &'static str,
    /// The key the inner source's own offers carry.
    inner_key: SourceKey,
    inner: Arc<dyn Source>,
}

impl Delegated {
    fn new(key: &'static str, name: &'static str, inner: Arc<dyn Source>) -> Self {
        Self { key, name, inner_key: inner.key(), inner }
    }

    /// The offer as the inner source made it: its own key, and the handle
    /// without this backend's prefix.
    fn inner_offer(&self, offer: &Offer) -> Result<Offer, SourceError> {
        let rest = offer
            .handle
            .strip_prefix(self.key)
            .and_then(|h| h.strip_prefix(':'))
            .ok_or_else(|| SourceError::Parse(format!("not a {} handle: {}", self.key, offer.handle)))?;
        Ok(Offer { source: self.inner_key, id: rest.to_owned(), handle: rest.to_owned(), ..offer.clone() })
    }

    /// The offer as this source tells it.
    fn outer_offer(&self, offer: Offer) -> Offer {
        Offer {
            source: SourceKey::Junkyard,
            provider: self.name.to_owned(),
            id: format!("{}:{}", self.key, offer.id),
            handle: format!("{}:{}", self.key, offer.handle),
            ..offer
        }
    }
}

impl Backend for Delegated {
    fn name(&self) -> &'static str {
        self.name
    }

    fn key(&self) -> &'static str {
        self.key
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            let found = self.inner.search(want).await?;
            Ok(found.into_iter().map(|o| self.outer_offer(o)).collect())
        })
    }

    fn fetch<'a>(
        &'a self,
        offer: &'a Offer,
        want: &'a Want,
        dest: &'a Path,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let inner = self.inner_offer(offer)?;
            self.inner.fetch(&inner, want, dest, progress).await
        })
    }
}

pub struct Junkyard {
    /// Asked in this order; the first that gives a lossless offer of the
    /// wish ends the search.
    backends: Vec<Box<dyn Backend>>,
}

impl Junkyard {
    /// The backends whose list `sites` names. A kind with no address is
    /// not asked at all.
    pub fn new(sites: &Sites) -> Self {
        let mut backends: Vec<Box<dyn Backend>> = Vec::new();
        if !sites.qobuz_dl.is_empty() {
            let inner = qobuz::Qobuz::from_list(&sites.qobuz_dl, true, PACE);
            backends.push(Box::new(Delegated::new("qobuz-dl", "Qobuz-DL", Arc::new(inner))));
        }
        if !sites.hifi.is_empty() {
            let inner = tidal::Tidal::from_list(&sites.hifi, true, PACE);
            backends.push(Box::new(Delegated::new("hifi", "hifi-api", Arc::new(inner))));
        }
        if !sites.rhythmax.is_empty() {
            backends.push(Box::new(rhythmax::Rhythmax::new(&sites.rhythmax)));
        }
        Self { backends }
    }
}

impl Default for Junkyard {
    fn default() -> Self {
        Self::new(&Sites::default())
    }
}

/// The error of the backend that comes back soonest: the shortest wait one
/// asked for, else the first one that said nothing of a wait.
fn soonest(errors: Vec<SourceError>) -> SourceError {
    let mut best: Option<(u64, SourceError)> = None;
    for e in errors {
        let rank = e.wait().map_or(u64::MAX, |d| d.as_secs());
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, e));
        }
    }
    best.map_or(SourceError::NotConfigured, |(_, e)| e)
}

impl Source for Junkyard {
    fn key(&self) -> SourceKey {
        SourceKey::Junkyard
    }

    fn name(&self) -> &str {
        SourceKey::Junkyard.display_name()
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            if self.backends.is_empty() {
                return Err(SourceError::NotConfigured);
            }
            let mut offers: Vec<Offer> = Vec::new();
            let mut errors: Vec<SourceError> = Vec::new();
            let mut answered = false;
            for backend in &self.backends {
                match backend.search(want).await {
                    Ok(found) => {
                        answered = true;
                        let enough = found.iter().any(|o| o.lossless() && matching::judge(o, want).is_some());
                        offers.extend(found);
                        if enough {
                            break;
                        }
                    }
                    Err(e) => {
                        log::info!("junkyard: {}: {e}", backend.name());
                        errors.push(e);
                    }
                }
            }
            // Every backend sitting out or failing is not "not found": the
            // wishlist would take it for a miss.
            if !answered {
                return Err(soonest(errors));
            }
            Ok(offers)
        })
    }

    fn fetch<'a>(
        &'a self,
        offer: &'a Offer,
        want: &'a Want,
        dest: &'a Path,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let key = offer.handle.split_once(':').map_or("", |(k, _)| k);
            let backend = self.backends.iter().find(|b| b.key() == key).ok_or(SourceError::NotConfigured)?;
            backend.fetch(offer, want, dest, progress).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nikgob_core::model::Codec;
    use nikgob_core::source::{Format, OfferScope};

    fn want() -> Want {
        Want {
            artists: vec!["Daft Punk".into()],
            title: Some("One More Time".into()),
            album: Some("Discovery".into()),
            duration_ms: Some(320_000),
            ..Want::default()
        }
    }

    fn offer(handle: &str, lossless: bool) -> Offer {
        let format = if lossless {
            Format::new(Codec::Flac)
        } else {
            Format { bitrate_kbps: Some(320), ..Format::new(Codec::Mp3) }
        };
        Offer {
            source: SourceKey::Junkyard,
            provider: "test".into(),
            id: handle.into(),
            title: "Daft Punk — One More Time".into(),
            artist: Some("Daft Punk".into()),
            track_title: Some("One More Time".into()),
            album: Some("Discovery".into()),
            duration_ms: Some(320_000),
            isrc: None,
            scope: OfferScope::Track,
            format: Some(format),
            size_bytes: None,
            seeders: None,
            info_url: None,
            handle: handle.into(),
        }
    }

    /// A backend that answers what it was told to, and counts the asks.
    struct Fake {
        key: &'static str,
        answer: Result<Vec<Offer>, SourceError>,
        asked: Arc<AtomicUsize>,
    }

    impl Fake {
        fn new(key: &'static str, answer: Result<Vec<Offer>, SourceError>) -> Self {
            Self { key, answer, asked: Arc::new(AtomicUsize::new(0)) }
        }

        fn counter(&self) -> Arc<AtomicUsize> {
            self.asked.clone()
        }
    }

    impl Backend for Fake {
        fn name(&self) -> &'static str {
            self.key
        }

        fn key(&self) -> &'static str {
            self.key
        }

        fn search<'a>(&'a self, _want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
            Box::pin(async move {
                self.asked.fetch_add(1, Ordering::Relaxed);
                self.answer.clone()
            })
        }

        fn fetch<'a>(
            &'a self,
            _offer: &'a Offer,
            _want: &'a Want,
            _dest: &'a Path,
            _progress: Progress,
        ) -> BoxFuture<'a, Result<Fetched, SourceError>> {
            Box::pin(async move { Err(SourceError::NothingToFetch) })
        }
    }

    /// An inner source that answers one album offer and remembers the
    /// handle it was asked to fetch.
    struct Inner {
        key: SourceKey,
        fetched: Arc<Mutex<Vec<String>>>,
    }

    impl Source for Inner {
        fn key(&self) -> SourceKey {
            self.key
        }

        fn name(&self) -> &str {
            "inner"
        }

        fn search<'a>(&'a self, _want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
            Box::pin(async move { Ok(vec![offer("album:7", true)]) })
        }

        fn fetch<'a>(
            &'a self,
            offer: &'a Offer,
            _want: &'a Want,
            _dest: &'a Path,
            _progress: Progress,
        ) -> BoxFuture<'a, Result<Fetched, SourceError>> {
            Box::pin(async move {
                self.fetched.lock().unwrap().push(offer.handle.clone());
                Ok(Fetched::Files(Vec::new()))
            })
        }
    }

    fn progress() -> Progress {
        Arc::new(|_, _| {})
    }

    /// One backend behind a check is skipped; the next one serves.
    #[tokio::test]
    async fn a_backend_that_sits_out_does_not_stop_the_others() {
        let junkyard = Junkyard {
            backends: vec![
                Box::new(Fake::new("blocked", Err(SourceError::Blocked("a bot check".into())))),
                Box::new(Fake::new("works", Ok(vec![offer("works:1", true)]))),
            ],
        };
        let got = junkyard.search(&want()).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].handle, "works:1");
    }

    /// Every backend sitting out is an error, and it is the one that comes
    /// back soonest; one backend that answered with nothing is not.
    #[tokio::test]
    async fn the_error_is_told_only_when_every_backend_sits_out() {
        let junkyard = Junkyard {
            backends: vec![
                Box::new(Fake::new(
                    "long",
                    Err(SourceError::RateLimited { retry_after: Some(Duration::from_secs(6 * 3600)) }),
                )),
                Box::new(Fake::new("short", Err(SourceError::RateLimited { retry_after: Some(Duration::from_secs(60)) }))),
            ],
        };
        assert_eq!(
            junkyard.search(&want()).await.unwrap_err(),
            SourceError::RateLimited { retry_after: Some(Duration::from_secs(60)) },
            "the backend that comes back soonest is the one told"
        );
        let one_answered = Junkyard {
            backends: vec![
                Box::new(Fake::new("blocked", Err(SourceError::Blocked("a bot check".into())))),
                Box::new(Fake::new("empty", Ok(Vec::new()))),
            ],
        };
        assert!(one_answered.search(&want()).await.unwrap().is_empty(), "one answered: not an error");
    }

    /// A lossless offer of the wish ends the search; a lossy one does not.
    #[tokio::test]
    async fn a_lossless_offer_of_the_wish_stops_the_asking() {
        let first = Fake::new("first", Ok(vec![offer("first:1", true)]));
        let (first_asked, second) = (first.counter(), Fake::new("second", Ok(vec![offer("second:1", true)])));
        let second_asked = second.counter();
        let junkyard = Junkyard { backends: vec![Box::new(first), Box::new(second)] };
        assert_eq!(junkyard.search(&want()).await.unwrap().len(), 1);
        assert_eq!(first_asked.load(Ordering::Relaxed), 1);
        assert_eq!(second_asked.load(Ordering::Relaxed), 0, "the second backend is not asked");

        let first = Fake::new("first", Ok(vec![offer("first:1", false)]));
        let (first_asked, second) = (first.counter(), Fake::new("second", Ok(vec![offer("second:1", true)])));
        let second_asked = second.counter();
        let junkyard = Junkyard { backends: vec![Box::new(first), Box::new(second)] };
        assert_eq!(junkyard.search(&want()).await.unwrap().len(), 2, "a lossy offer is not enough");
        assert_eq!(first_asked.load(Ordering::Relaxed), 1);
        assert_eq!(second_asked.load(Ordering::Relaxed), 1);
    }

    /// An offer says which backend it came from, and the fetch goes back
    /// through that backend with the handle it made.
    #[tokio::test]
    async fn an_offer_says_its_backend_and_fetches_through_it() {
        let fetched = Arc::new(Mutex::new(Vec::new()));
        let inner = Arc::new(Inner { key: SourceKey::Qobuz, fetched: fetched.clone() });
        let backend = Delegated::new("qobuz-dl", "Qobuz-DL", inner);
        let found = backend.search(&want()).await.unwrap();
        assert_eq!(found[0].source, SourceKey::Junkyard);
        assert_eq!(found[0].provider, "Qobuz-DL");
        assert_eq!(found[0].handle, "qobuz-dl:album:7");
        assert_eq!(found[0].id, "qobuz-dl:album:7");
        let junkyard = Junkyard { backends: vec![Box::new(backend)] };
        junkyard.fetch(&found[0], &want(), Path::new("."), progress()).await.unwrap();
        assert_eq!(*fetched.lock().unwrap(), ["album:7"], "the inner source gets its own handle back");
    }

    /// No site named: nothing is asked, and the source says it is not
    /// configured rather than that it has nothing.
    #[tokio::test]
    async fn no_sites_is_not_configured() {
        let junkyard = Junkyard::new(&Sites::default());
        assert_eq!(junkyard.search(&want()).await.unwrap_err(), SourceError::NotConfigured);
        let stray = offer("gone:1", true);
        assert_eq!(
            junkyard.fetch(&stray, &want(), Path::new("."), progress()).await.unwrap_err(),
            SourceError::NotConfigured,
            "a handle of a backend that is gone"
        );
    }

    /// A list makes its backend, in the order the kinds are asked.
    #[test]
    fn a_site_list_makes_its_backend() {
        let junkyard = Junkyard::new(&Sites { qobuz_dl: vec!["https://q.example".into()], ..Sites::default() });
        assert_eq!(junkyard.backends.len(), 1);
        assert_eq!(junkyard.backends[0].key(), "qobuz-dl");
        let both = Junkyard::new(&Sites {
            qobuz_dl: vec!["https://q.example".into()],
            hifi: vec!["https://h.example".into()],
            rhythmax: vec!["https://r.example".into()],
        });
        assert_eq!(both.backends.iter().map(|b| b.key()).collect::<Vec<_>>(), ["qobuz-dl", "hifi", "rhythmax"]);
    }

    /// A Qobuz-DL answer, trimmed to what the source reads, becomes an
    /// offer of this source: the backend's name, and a handle that says
    /// which backend and which track.
    #[tokio::test]
    async fn a_qobuz_dl_answer_becomes_a_junkyard_offer() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let answer = r#"{
            "success": true,
            "data": {
                "tracks": {"items": [{
                    "id": 52155495,
                    "title": "One More Time",
                    "duration": 320,
                    "maximum_bit_depth": 16,
                    "maximum_sampling_rate": 44.1,
                    "performer": {"name": "Daft Punk"},
                    "album": {"title": "Discovery", "artist": {"name": "Daft Punk"}}
                }]},
                "albums": {"items": []}
            }
        }"#;
        let server = MockServer::start().await;
        Mock::given(path("/api/get-music"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(answer, "application/json"))
            .mount(&server)
            .await;
        let inner = qobuz::Qobuz::from_list(&[server.uri()], true, Duration::ZERO);
        let junkyard = Junkyard { backends: vec![Box::new(Delegated::new("qobuz-dl", "Qobuz-DL", Arc::new(inner)))] };
        let got = junkyard.search(&want()).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].source, SourceKey::Junkyard);
        assert_eq!(got[0].provider, "Qobuz-DL");
        assert_eq!(got[0].handle, "qobuz-dl:52155495");
        assert_eq!(got[0].title, "Daft Punk — One More Time");
        assert_eq!(got[0].duration_ms, Some(320_000));
        assert!(got[0].lossless());
    }
}
