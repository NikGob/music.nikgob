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
use nikgob_core::source::{Fetched, Offer, Progress, Source, SourceError, SourceKey, Want, WantScope};

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

    /// The sites it is asked at, as it was built: a list that did not
    /// change keeps its backend, and with it the pauses of its sites.
    fn sites(&self) -> Vec<String>;

    /// Whether it has anything for `scope` at all: a backend that lists
    /// only tracks is not asked for an album.
    fn supports(&self, scope: WantScope) -> bool;

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
    /// The sites it was built with.
    sites: Vec<String>,
    /// The key the inner source's own offers carry.
    inner_key: SourceKey,
    inner: Arc<dyn Source>,
}

impl Delegated {
    fn new(key: &'static str, name: &'static str, sites: &[String], inner: Arc<dyn Source>) -> Self {
        Self { key, name, sites: sites.to_vec(), inner_key: inner.key(), inner }
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

    fn sites(&self) -> Vec<String> {
        self.sites.clone()
    }

    /// Qobuz-DL and hifi-api list albums as well as tracks.
    fn supports(&self, _scope: WantScope) -> bool {
        true
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

/// The kinds of backend, in the order they are asked.
#[derive(Clone, Copy)]
enum Kind {
    QobuzDl,
    Hifi,
    Rhythmax,
}

impl Kind {
    const ALL: [Self; 3] = [Self::QobuzDl, Self::Hifi, Self::Rhythmax];

    fn key(self) -> &'static str {
        match self {
            Self::QobuzDl => "qobuz-dl",
            Self::Hifi => "hifi",
            Self::Rhythmax => "rhythmax",
        }
    }

    /// The sites `sites` names for this kind.
    fn list<'a>(self, sites: &'a Sites) -> &'a [String] {
        match self {
            Self::QobuzDl => &sites.qobuz_dl,
            Self::Hifi => &sites.hifi,
            Self::Rhythmax => &sites.rhythmax,
        }
    }

    fn build(self, urls: &[String]) -> Arc<dyn Backend> {
        match self {
            Self::QobuzDl => {
                let inner = qobuz::Qobuz::from_list(urls, true, PACE);
                Arc::new(Delegated::new("qobuz-dl", "Qobuz-DL", urls, Arc::new(inner)))
            }
            Self::Hifi => {
                let inner = tidal::Tidal::from_list(urls, true, PACE);
                Arc::new(Delegated::new("hifi", "hifi-api", urls, Arc::new(inner)))
            }
            Self::Rhythmax => Arc::new(rhythmax::Rhythmax::new(urls)),
        }
    }
}

pub struct Junkyard {
    /// Asked in this order; the first that gives a lossless offer of the
    /// wish ends the search. `sources.toml` can change the list while the
    /// app runs, so it sits behind a lock and a call works on a copy.
    backends: std::sync::Mutex<Vec<Arc<dyn Backend>>>,
}

impl Junkyard {
    /// The backends whose list `sites` names. A kind with no address is
    /// not asked at all.
    pub fn new(sites: &Sites) -> Self {
        Self::of(Kind::ALL.iter().filter(|k| !k.list(sites).is_empty()).map(|k| k.build(k.list(sites))).collect())
    }

    fn of(backends: Vec<Arc<dyn Backend>>) -> Self {
        Self { backends: std::sync::Mutex::new(backends) }
    }

    /// The backends asked now, in order.
    fn now(&self) -> Vec<Arc<dyn Backend>> {
        self.backends.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// The sites `sources.toml` names now. A kind whose list did not change
    /// keeps its backend, and with it the pauses of its sites.
    pub fn set_sites(&self, sites: &Sites) {
        let mut backends = self.backends.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut next: Vec<Arc<dyn Backend>> = Vec::new();
        for kind in Kind::ALL {
            let urls = kind.list(sites);
            if urls.is_empty() {
                continue;
            }
            let kept = backends.iter().find(|b| b.key() == kind.key() && b.sites().as_slice() == urls);
            next.push(match kept {
                Some(b) => b.clone(),
                None => kind.build(urls),
            });
        }
        *backends = next;
    }
}

impl Default for Junkyard {
    fn default() -> Self {
        Self::new(&Sites::default())
    }
}

/// How long the wishlist sits the whole source out for `e`: a check or a
/// rate limit pauses it ([`crate::http::pause_for`]), an HTTP strike only
/// counts towards one, and anything else does not pause it at all. An error
/// that costs no pause ranks first, so one backend's quota never borrows
/// another backend's pause for the source.
fn pause_of(e: &SourceError) -> u64 {
    if crate::http::is_check(e) || matches!(e, SourceError::RateLimited { .. }) {
        return crate::http::pause_for(e, 0).as_secs();
    }
    0
}

/// The error of the backend that comes back soonest: the shortest pause
/// the wishlist would take from it, else the first one that costs none.
fn soonest(errors: Vec<SourceError>) -> SourceError {
    let mut best: Option<(u64, SourceError)> = None;
    for e in errors {
        let rank = pause_of(&e);
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
            let backends: Vec<Arc<dyn Backend>> = self.now().into_iter().filter(|b| b.supports(want.scope)).collect();
            // Nothing that could answer this at all: not a miss, and not a
            // pause either.
            if backends.is_empty() {
                return Err(SourceError::NotConfigured);
            }
            let mut offers: Vec<Offer> = Vec::new();
            let mut errors: Vec<SourceError> = Vec::new();
            let mut answered = false;
            for backend in &backends {
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
            let backends = self.now();
            let key = offer.handle.split_once(':').map_or("", |(k, _)| k);
            let Some(at) = backends.iter().position(|b| b.key() == key) else {
                return Err(SourceError::NotConfigured);
            };
            match backends[at].fetch(offer, want, dest, progress.clone()).await {
                // One site turning the file away is not the whole source
                // sitting out: another backend may have the same track.
                Err(e) if e.is_refusal() => {
                    log::info!("junkyard: {} turned the file away: {e}", backends[at].name());
                    for other in backends.iter().filter(|b| b.key() != key && b.supports(want.scope)) {
                        let Ok(found) = other.search(want).await else { continue };
                        let best = found.into_iter().filter(|o| matching::judge(o, want).is_some()).max_by_key(Offer::quality);
                        let Some(best) = best else { continue };
                        if let Ok(got) = other.fetch(&best, want, dest, progress.clone()).await {
                            return Ok(got);
                        }
                    }
                    Err(e)
                }
                r => r,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
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
        /// What a fetch of one of its offers gives, or refuses with.
        file: Result<PathBuf, SourceError>,
        asked: Arc<AtomicUsize>,
        /// Whether it has anything for an album.
        albums: bool,
    }

    impl Fake {
        fn new(key: &'static str, answer: Result<Vec<Offer>, SourceError>) -> Self {
            Self {
                key,
                answer,
                file: Err(SourceError::NothingToFetch),
                asked: Arc::new(AtomicUsize::new(0)),
                albums: true,
            }
        }

        /// A backend that lists tracks only, as Rhythmax does.
        fn tracks_only(key: &'static str, answer: Result<Vec<Offer>, SourceError>) -> Self {
            Self { albums: false, ..Self::new(key, answer) }
        }

        /// The file it gives, or the error it refuses with.
        fn file(mut self, file: Result<&str, SourceError>) -> Self {
            self.file = file.map(PathBuf::from);
            self
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

        fn sites(&self) -> Vec<String> {
            Vec::new()
        }

        fn supports(&self, scope: WantScope) -> bool {
            self.albums || scope == WantScope::Track
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
            Box::pin(async move { self.file.clone().map(|p| Fetched::Files(vec![p])) })
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
        let junkyard = Junkyard::of(vec![
            Arc::new(Fake::new("blocked", Err(SourceError::Blocked("a bot check".into())))),
            Arc::new(Fake::new("works", Ok(vec![offer("works:1", true)]))),
        ]);
        let got = junkyard.search(&want()).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].handle, "works:1");
    }

    /// Every backend sitting out is an error, and it is the one that comes
    /// back soonest; one backend that answered with nothing is not.
    #[tokio::test]
    async fn the_error_is_told_only_when_every_backend_sits_out() {
        let junkyard = Junkyard::of(vec![
            Arc::new(Fake::new(
                "long",
                Err(SourceError::RateLimited { retry_after: Some(Duration::from_secs(6 * 3600)) }),
            )),
            Arc::new(Fake::new("short", Err(SourceError::RateLimited { retry_after: Some(Duration::from_secs(60)) }))),
        ]);
        assert_eq!(
            junkyard.search(&want()).await.unwrap_err(),
            SourceError::RateLimited { retry_after: Some(Duration::from_secs(60)) },
            "the backend that comes back soonest is the one told"
        );
        let one_answered = Junkyard::of(vec![
            Arc::new(Fake::new("blocked", Err(SourceError::Blocked("a bot check".into())))),
            Arc::new(Fake::new("empty", Ok(Vec::new()))),
        ]);
        assert!(one_answered.search(&want()).await.unwrap().is_empty(), "one answered: not an error");
    }

    /// An error the wishlist does not pause the source for is told ahead of
    /// one it does: a backend that timed out must not borrow another
    /// backend's three-hour check for the whole source.
    #[tokio::test]
    async fn a_transient_failure_is_told_ahead_of_a_long_pause() {
        let junkyard = Junkyard::of(vec![
            Arc::new(Fake::new("checked", Err(SourceError::Blocked(crate::http::BOT_CHECK.into())))),
            Arc::new(Fake::new("timed-out", Err(SourceError::Unreachable("timed out".into())))),
        ]);
        assert_eq!(
            junkyard.search(&want()).await.unwrap_err(),
            SourceError::Unreachable("timed out".into()),
            "the error that costs the source no pause is the one told"
        );
        assert_eq!(pause_of(&SourceError::Unreachable("dns".into())), 0);
        assert_eq!(pause_of(&SourceError::Http { status: 403 }), 0, "a strike, not a pause");
        assert_eq!(pause_of(&SourceError::Blocked(crate::http::BOT_CHECK.into())), crate::http::BLOCKED_PAUSE.as_secs());
        assert_eq!(pause_of(&SourceError::RateLimited { retry_after: Some(Duration::from_secs(60)) }), 60);
    }

    /// A lossless offer of the wish ends the search; a lossy one does not.
    #[tokio::test]
    async fn a_lossless_offer_of_the_wish_stops_the_asking() {
        let first = Fake::new("first", Ok(vec![offer("first:1", true)]));
        let (first_asked, second) = (first.counter(), Fake::new("second", Ok(vec![offer("second:1", true)])));
        let second_asked = second.counter();
        let junkyard = Junkyard::of(vec![Arc::new(first), Arc::new(second)]);
        assert_eq!(junkyard.search(&want()).await.unwrap().len(), 1);
        assert_eq!(first_asked.load(Ordering::Relaxed), 1);
        assert_eq!(second_asked.load(Ordering::Relaxed), 0, "the second backend is not asked");

        let first = Fake::new("first", Ok(vec![offer("first:1", false)]));
        let (first_asked, second) = (first.counter(), Fake::new("second", Ok(vec![offer("second:1", true)])));
        let second_asked = second.counter();
        let junkyard = Junkyard::of(vec![Arc::new(first), Arc::new(second)]);
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
        let backend = Delegated::new("qobuz-dl", "Qobuz-DL", &["https://q.example".to_owned()], inner);
        let found = backend.search(&want()).await.unwrap();
        assert_eq!(found[0].source, SourceKey::Junkyard);
        assert_eq!(found[0].provider, "Qobuz-DL");
        assert_eq!(found[0].handle, "qobuz-dl:album:7");
        assert_eq!(found[0].id, "qobuz-dl:album:7");
        let junkyard = Junkyard::of(vec![Arc::new(backend)]);
        junkyard.fetch(&found[0], &want(), Path::new("."), progress()).await.unwrap();
        assert_eq!(*fetched.lock().unwrap(), ["album:7"], "the inner source gets its own handle back");
    }

    /// A backend that lists only tracks is not asked for an album, and a
    /// source with nothing for albums says so instead of answering empty.
    #[tokio::test]
    async fn an_album_is_not_asked_of_a_backend_that_lists_only_tracks() {
        let tracks = Fake::tracks_only("tracks", Ok(vec![offer("tracks:1", true)]));
        let asked = tracks.counter();
        let junkyard = Junkyard::of(vec![Arc::new(tracks)]);
        let album = Want { scope: WantScope::Album, title: None, ..want() };
        assert_eq!(junkyard.search(&album).await.unwrap_err(), SourceError::NotConfigured);
        assert_eq!(asked.load(Ordering::Relaxed), 0, "it is not asked at all");
        assert_eq!(junkyard.search(&want()).await.unwrap().len(), 1, "a track is still its");
    }

    /// A refused file is asked of the other backends before the whole
    /// source is told it was turned away.
    #[tokio::test]
    async fn a_refused_file_is_asked_of_another_backend() {
        let first = Fake::new("first", Ok(vec![offer("first:1", true)])).file(Err(SourceError::Http { status: 403 }));
        let second = Fake::new("second", Ok(vec![offer("second:1", true)])).file(Ok("second.flac"));
        let junkyard = Junkyard::of(vec![Arc::new(first), Arc::new(second)]);
        let found = junkyard.search(&want()).await.unwrap();
        assert_eq!(found.len(), 1, "the first backend's lossless offer ends the search");
        let got = junkyard.fetch(&found[0], &want(), Path::new("."), progress()).await.unwrap();
        let Fetched::Files(files) = got else { panic!("{got:?}") };
        assert_eq!(files, [PathBuf::from("second.flac")], "the other backend served it");

        // Every backend refusing is the refusal, as before.
        let only = Fake::new("only", Ok(vec![offer("only:1", true)])).file(Err(SourceError::Http { status: 403 }));
        let junkyard = Junkyard::of(vec![Arc::new(only)]);
        let found = junkyard.search(&want()).await.unwrap();
        assert_eq!(
            junkyard.fetch(&found[0], &want(), Path::new("."), progress()).await.unwrap_err(),
            SourceError::Http { status: 403 }
        );
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
        assert_eq!(junkyard.now().len(), 1);
        assert_eq!(junkyard.now()[0].key(), "qobuz-dl");
        let both = Junkyard::new(&Sites {
            qobuz_dl: vec!["https://q.example".into()],
            hifi: vec!["https://h.example".into()],
            rhythmax: vec!["https://r.example".into()],
        });
        assert_eq!(both.now().iter().map(|b| b.key()).collect::<Vec<_>>(), ["qobuz-dl", "hifi", "rhythmax"]);
    }

    /// A new list keeps the backend whose sites did not change, and with it
    /// the pauses of its sites; a kind it drops is not asked any more.
    #[test]
    fn a_new_list_keeps_the_backend_whose_sites_did_not_change() {
        let junkyard = Junkyard::new(&Sites { qobuz_dl: vec!["https://q.example".into()], ..Sites::default() });
        let before = junkyard.now()[0].clone();
        junkyard.set_sites(&Sites {
            qobuz_dl: vec!["https://q.example".into()],
            hifi: vec!["https://h.example".into()],
            rhythmax: Vec::new(),
        });
        let after = junkyard.now();
        assert_eq!(after.iter().map(|b| b.key()).collect::<Vec<_>>(), ["qobuz-dl", "hifi"]);
        assert!(Arc::ptr_eq(&before, &after[0]), "the unchanged backend is the same one");
        junkyard.set_sites(&Sites { hifi: vec!["https://h.example".into()], ..Sites::default() });
        assert_eq!(junkyard.now().iter().map(|b| b.key()).collect::<Vec<_>>(), ["hifi"], "a dropped kind goes");
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
        let backend = Delegated::new("qobuz-dl", "Qobuz-DL", &[server.uri()], Arc::new(inner));
        let junkyard = Junkyard::of(vec![Arc::new(backend)]);
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
