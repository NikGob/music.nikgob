//! Background enrichment from the network: lyrics for tracks that have none and
//! artist photos. Both are cached in the library (including "nothing found"
//! markers, so the services are not asked again and again) and both stay off
//! when the app is offline (`ImportService::metadata` is `None`).
//!
//! The network parts are plain async functions over the metadata clients and the
//! library, so tests can run them against a mock server; the `Worker` glue at
//! the bottom only spawns them and applies their results to the view state.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nikgob_core::model::ArtistId;
use nikgob_library::{Library, StoredLyrics, TrackRow, now_ms};
use nikgob_metadata::{Deezer, Lrclib, Lyrics, Metadata, MetadataError};
use nikgob_view as v;

use crate::Worker;
use crate::import::Internal;

/// A lyrics lookup that found nothing is repeated after this long.
pub(crate) const LYRICS_RETRY_MS: i64 = 7 * 86_400_000;
/// Same for artist photos.
const PHOTO_RETRY_MS: i64 = 14 * 86_400_000;
const BATCH: u32 = 25;
/// Consecutive network failures after which a background job gives up until the next trigger.
const MAX_FAILURES: u32 = 3;

/// Delays of the background jobs (zero in tests).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pace {
    /// Before the first request: playback and the first screens go first.
    pub(crate) start: Duration,
    /// Between two tracks / artists.
    pub(crate) between: Duration,
}

impl Pace {
    const LYRICS: Self = Self {
        start: Duration::from_secs(20),
        between: Duration::from_secs(1),
    };
    const PHOTOS: Self = Self {
        start: Duration::ZERO,
        between: Duration::from_millis(800),
    };
}

/// Results of the background jobs on their way to the worker.
pub(crate) enum Event {
    ArtistPhoto(ArtistId),
    PhotosDone,
    BackfillDone,
    LyricsReady(nikgob_core::model::TrackId, crate::lyrics::TrackLyrics),
    /// A larger cover of this album was found ([`crate::hd_covers`]).
    HdCover(nikgob_core::model::ReleaseId),
    /// Real art was found for poor covers ([`crate::better_covers`]).
    BetterCovers,
}

#[derive(Default)]
pub(crate) struct Enrich {
    photos_running: bool,
    backfill_running: bool,
    /// Artists whose pages were opened: fetched before the rest.
    priority: Arc<Mutex<Vec<(ArtistId, String)>>>,
}

// ------------------------------------------------------------------- lyrics

/// Lyrics that came with the file: a `.lrc` next to it, else the tag. Returns
/// `(synced LRC text, plain text)`.
pub(crate) fn local_lyrics(
    file: &Path,
    tags: Option<&nikgob_import::tags::TagSet>,
) -> Option<(Option<String>, Option<String>)> {
    let lrc = nikgob_import::sanitize::long_path(&file.with_extension("lrc"));
    if let Ok(text) = std::fs::read_to_string(&lrc)
        && !nikgob_metadata::parse_lrc(&text).is_empty()
    {
        return Some((Some(text), None));
    }
    let text = tags?.lyrics.clone().filter(|t| !t.trim().is_empty())?;
    if nikgob_metadata::parse_lrc(&text).is_empty() {
        Some((None, Some(text)))
    } else {
        Some((Some(text), None))
    }
}

/// Asks LRCLIB (with a deadline) and caches the answer, "not found" included.
/// Only a definite answer is cached: lyrics, or LRCLIB saying it has none.
/// Any error (5xx, 429, network, timeout) is returned and nothing is stored.
pub(crate) async fn fetch_and_store(
    lrclib: &Lrclib,
    lib: &Library,
    row: &TrackRow,
) -> Result<StoredLyrics, MetadataError> {
    let duration = row.duration_ms.map(|ms| Duration::from_millis(ms.into()));
    let lookup = tokio::time::timeout(
        crate::lyrics::LOOKUP_TIMEOUT,
        lrclib.find(&row.artist, &row.title, row.album.as_deref(), duration),
    )
    .await
    .unwrap_or_else(|_| Err(MetadataError::Network("timeout".into())));
    let found: Option<Lyrics> = match lookup {
        Ok(found) => found,
        // Only a clean "no such track" (404 / empty search) is a miss; every
        // other failure (5xx, 429, timeouts, bad bodies) is retried later.
        Err(e) => return Err(e),
    };
    let stored = StoredLyrics {
        synced: found
            .as_ref()
            .filter(|l| l.has_synced())
            .and_then(|l| l.synced_lyrics.clone()),
        plain: found
            .as_ref()
            .and_then(|l| l.plain_lyrics.clone())
            .filter(|p| !p.trim().is_empty()),
        instrumental: found.as_ref().is_some_and(|l| l.instrumental),
        fetched_at: now_ms(),
    };
    if let Err(e) = lib.put_lyrics(
        row.id,
        stored.synced.as_deref(),
        stored.plain.as_deref(),
        stored.instrumental,
    ) {
        log::warn!("caching lyrics: {e}");
    }
    Ok(stored)
}

/// Fills in the lyrics of every local track that has none, slowly, for as long
/// as there is work, using the same source chain as playback. Its requests
/// give way to the shown song's ([`nikgob_metadata::in_background`]).
pub(crate) async fn backfill_lyrics(
    meta: Metadata,
    lib: Library,
    library_dir: PathBuf,
    pace: Pace,
    tx: crossbeam_channel::Sender<Internal>,
    genius: bool,
) {
    tokio::time::sleep(pace.start).await;
    let mut failures = 0;
    loop {
        let ids = match lib.tracks_needing_lyrics(now_ms() - LYRICS_RETRY_MS, BATCH) {
            Ok(ids) if !ids.is_empty() => ids,
            Ok(_) => return,
            Err(e) => {
                log::warn!("lyrics backfill: {e}");
                return;
            }
        };
        for id in ids {
            let Some(row) = lib.get_track_row(id).ok().flatten() else {
                let _ = lib.put_lyrics(id, None, None, false);
                continue;
            };
            let file = lib
                .active_source(id)
                .ok()
                .flatten()
                .filter(|s| s.kind.is_local())
                .map(|s| resolve(&library_dir, &s.locator));
            // The playing song's lookups go first (`in_background`).
            let tl = nikgob_metadata::in_background(crate::lyrics::fetch_track_lyrics(meta.clone(), lib.clone(), row, file, genius, None)).await;
            if tl.failed {
                failures += 1;
                if failures >= MAX_FAILURES { return }
            } else {
                failures = 0;
            }
            let _ = tx.send(Internal::Enrich(Event::LyricsReady(id, tl)));
            tokio::time::sleep(pace.between).await;
        }
    }
}

fn resolve(root: &Path, locator: &str) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() {
        p.to_owned()
    } else {
        root.join(p)
    }
}

// ------------------------------------------------------------------- photos

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PhotoOutcome {
    Saved(PathBuf),
    /// Deezer has no usable photo; recorded, retried much later.
    NotFound,
    /// No network or an unexpected answer; nothing recorded.
    Failed,
}

fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(&[0x89, b'P', b'N', b'G'])
}

/// How many local tracks are tried as anchors before falling back to the name.
const ANCHOR_TRACKS: usize = 3;
/// Local track / album titles handed to the name-search verification.
const VERIFY_TITLES: u32 = 60;

/// Finds the artist on Deezer, downloads the photo into `dir` and records it.
///
/// A photo is only taken from a Deezer artist known to be this one: first via
/// one of the artist's own tracks (its `artist.id`), then by name search whose
/// albums / top tracks must overlap the local titles. No proof, no photo.
/// A photo locked by the user is left alone.
pub(crate) async fn fetch_artist_photo(
    deezer: &Deezer,
    lib: &Library,
    id: ArtistId,
    name: &str,
    dir: &Path,
) -> PhotoOutcome {
    if lib.artist_photo_locked(id).unwrap_or(false) {
        return PhotoOutcome::NotFound;
    }
    let tracks = lib.tracks_of_artist(id, 0, VERIFY_TITLES).unwrap_or_default();
    let work = async {
        // 1. Anchor on the artist's own tracks (local ones first).
        let mut anchors: Vec<&TrackRow> = tracks.iter().filter(|t| t.has_local).collect();
        anchors.extend(tracks.iter().filter(|t| !t.has_local));
        let mut url = None;
        for t in anchors.into_iter().take(ANCHOR_TRACKS) {
            let duration = t.duration_ms.map(|ms| Duration::from_millis(ms.into()));
            // A failed anchor lookup just means no anchor; the fallback decides.
            if let Ok(Some(u)) = deezer.artist_picture_by_track(name, &t.title, duration).await {
                url = Some(u);
                break;
            }
        }
        // 2. Name search, accepted only when the candidate's records overlap ours.
        if url.is_none() {
            let mut known: Vec<String> = tracks.iter().map(|t| t.title.clone()).collect();
            known.extend(tracks.iter().filter_map(|t| t.album.clone()));
            url = deezer.verified_artist_picture(name, &known).await?;
        }
        let Some(url) = url else {
            return Ok(None);
        };
        deezer.download_image(&url).await.map(Some)
    };
    let bytes = match tokio::time::timeout(Duration::from_secs(45), work).await {
        Ok(Ok(Some(b))) if looks_like_image(&b) => b,
        Ok(Ok(_) | Err(MetadataError::NotFound)) => {
            let _ = lib.set_artist_photo(id, None);
            return PhotoOutcome::NotFound;
        }
        Ok(Err(e)) => {
            log::debug!("photo of {name}: {e}");
            return PhotoOutcome::Failed;
        }
        Err(_) => return PhotoOutcome::Failed,
    };
    let path = dir.join(format!("{id}.jpg"));
    let tmp = path.with_extension("part");
    let saved = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, &bytes))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = saved {
        log::warn!("saving the photo of {name}: {e}");
        return PhotoOutcome::Failed;
    }
    if let Err(e) = lib.set_artist_photo(id, Some(&path.display().to_string())) {
        log::warn!("recording the photo of {name}: {e}");
        return PhotoOutcome::Failed;
    }
    PhotoOutcome::Saved(path)
}

/// Fetches photos for artists that have none (artists whose pages were opened
/// first), calling `saved` for each new one.
pub(crate) async fn run_photos(
    deezer: Deezer,
    lib: Library,
    dir: PathBuf,
    priority: Arc<Mutex<Vec<(ArtistId, String)>>>,
    pace: Pace,
    saved: impl Fn(ArtistId),
) {
    tokio::time::sleep(pace.start).await;
    let mut failures = 0;
    loop {
        let batch = match lib.artists_needing_photo(now_ms() - PHOTO_RETRY_MS, BATCH) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("photo queue: {e}");
                return;
            }
        };
        let mut queue = batch.into_iter();
        let mut did_any = false;
        loop {
            let urgent = priority.lock().ok().and_then(|mut p| p.pop());
            let Some((id, name)) = urgent.or_else(|| queue.next()) else {
                break;
            };
            did_any = true;
            match fetch_artist_photo(&deezer, &lib, id, &name, &dir).await {
                PhotoOutcome::Saved(_) => {
                    failures = 0;
                    saved(id);
                }
                PhotoOutcome::NotFound => failures = 0,
                PhotoOutcome::Failed => {
                    failures += 1;
                    if failures >= MAX_FAILURES {
                        return;
                    }
                }
            }
            tokio::time::sleep(pace.between).await;
        }
        if !did_any {
            return;
        }
    }
}

// ------------------------------------------------------------------- worker

impl Worker {
    /// Starts the background jobs that make sense right now (idempotent).
    pub(crate) fn start_enrich(&mut self) {
        self.kick_lyrics_backfill();
        self.kick_photos(None);
        self.kick_better_covers(None);
    }

    pub(crate) fn kick_lyrics_backfill(&mut self) {
        let lib = self.lib.clone();
        let Some(svc) = &mut self.import else { return };
        let Some(meta) = svc.metadata() else { return };
        if !svc.config().fetch_lyrics || svc.enrich.backfill_running {
            return;
        }
        svc.enrich.backfill_running = true;
        let dir = svc.config().library_dir.clone();
        let tx = svc.internal();
        let genius = self.state.read().prefs.lyrics_genius;
        svc.spawn(async move {
            backfill_lyrics(meta, lib, dir, Pace::LYRICS, tx.clone(), genius).await;
            let _ = tx.send(Internal::Enrich(Event::BackfillDone));
        });
    }

    /// Looks for artist photos in the background; `first` (an artist page that
    /// was just opened) goes ahead of the queue.
    pub(crate) fn kick_photos(&mut self, first: Option<ArtistId>) {
        let lib = self.lib.clone();
        let first = first.and_then(|id| {
            let due = match lib.artist_photo_lookup(id).ok()? {
                None => true,
                Some((Some(_), _)) => false,
                Some((None, at)) => at < now_ms() - PHOTO_RETRY_MS,
            };
            let artist = lib.get_artist(id).ok().flatten().filter(|_| due)?;
            Some((id, artist.name))
        });
        let Some(svc) = &mut self.import else { return };
        let Some(meta) = svc.metadata() else { return };
        if !svc.config().fetch_cover_online {
            return;
        }
        if let Some(first) = first
            && let Ok(mut p) = svc.enrich.priority.lock()
            && !p.iter().any(|(id, _)| *id == first.0)
        {
            p.push(first);
        }
        if svc.enrich.photos_running {
            return;
        }
        svc.enrich.photos_running = true;
        let cfg = svc.config();
        let dir = cfg
            .downloads_dir
            .parent()
            .unwrap_or(&cfg.library_dir)
            .join("Artists");
        let priority = svc.enrich.priority.clone();
        let tx = svc.internal();
        svc.spawn(async move {
            let saved_tx = tx.clone();
            run_photos(meta.deezer, lib, dir, priority, Pace::PHOTOS, move |id| {
                let _ = saved_tx.send(Internal::Enrich(Event::ArtistPhoto(id)));
            })
            .await;
            let _ = tx.send(Internal::Enrich(Event::PhotosDone));
        });
    }

    pub(crate) fn enrich_event(&mut self, ev: Event) {
        match ev {
            Event::ArtistPhoto(id) => {
                self.res.invalidate_photos();
                let image = self.res.photo(id);
                self.publish(|s| {
                    let fix = |a: &mut v::ArtistCard| {
                        if a.id == id {
                            a.image = image.clone();
                        }
                    };
                    s.library.artists.iter_mut().for_each(fix);
                    s.search.artists.iter_mut().for_each(fix);
                    s.home.artist_mixes.iter_mut().for_each(fix);
                    if let Some(a) = s.artist.as_mut() {
                        fix(&mut a.card);
                    }
                });
            }
            Event::PhotosDone => {
                if let Some(svc) = &mut self.import {
                    svc.enrich.photos_running = false;
                }
            }
            Event::LyricsReady(track, tl) => self.lyrics_backfilled(track, tl),
            Event::HdCover(id) => self.hd_cover_found(id),
            Event::BetterCovers => self.better_covers_found(),
            Event::BackfillDone => {
                if let Some(svc) = &mut self.import {
                    svc.enrich.backfill_running = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};
    use nikgob_metadata::{Http, MetadataConfig, RateLimits, RetryPolicy};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const NO_WAIT: Pace = Pace {
        start: Duration::ZERO,
        between: Duration::ZERO,
    };

    fn http() -> Http {
        Http::new(&MetadataConfig::new("tests@example.invalid"))
            .unwrap()
            .with_rate_limits(RateLimits::unlimited())
            .with_retry_policy(RetryPolicy {
                max_attempts: 1,
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(1),
            })
    }

    async fn metadata(server: &MockServer) -> Metadata {
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.lrclib = Lrclib::with_base_url(http(), &server.uri()).unwrap();
        meta.lyricsplus = nikgob_metadata::LyricsPlus::with_base_url(http(), &server.uri()).unwrap();
        meta.lrcred = nikgob_metadata::LrcRed::with_base_url(http(), &server.uri()).unwrap();
        meta.deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        meta.betterlyrics = nikgob_metadata::BetterLyrics::with_base_urls(http(), &server.uri(), &server.uri()).unwrap();
        Mock::given(path("/search/track")).respond_with(json(r#"{"data":[]}"#)).mount(server).await;
        meta
    }

    fn json(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "application/json")
    }

    fn library_track(lib: &Library, artist: &str, title: &str) -> (Artist, Track) {
        let a = lib.upsert_artist(&Artist::new(artist)).unwrap();
        let mut t = Track::new(title, ArtistCredit::single(artist, Some(a.id)));
        t.duration_ms = Some(285_000);
        let t = lib.upsert_track(&t).unwrap();
        lib.add_source(
            &Source::new(t.id, SourceKind::LocalFile, format!("/nonexistent/{title}.flac"), Codec::Flac)
                .with_quality(false),
        )
        .unwrap();
        (a, t)
    }

    const LRC_HIT: &str = r#"{"id": 1, "trackName": "Группа крови", "artistName": "Кино",
        "duration": 285, "instrumental": false, "plainLyrics": "Тёплое место",
        "syncedLyrics": "[00:12.00]Тёплое место"}"#;

    #[tokio::test]
    async fn lyrics_are_fetched_cached_and_misses_are_remembered() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .and(query_param("track_name", "Группа крови"))
            .respond_with(json(LRC_HIT))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .respond_with(ResponseTemplate::new(404).set_body_raw(r#"{"code":404}"#, "application/json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/search"))
            .respond_with(json("[]"))
            .mount(&server)
            .await;
        let lrclib = Lrclib::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (_, hit) = library_track(&lib, "Кино", "Группа крови");
        let (_, miss) = library_track(&lib, "Кино", "Нет такой песни");

        let row = lib.get_track_row(hit.id).unwrap().unwrap();
        let st = fetch_and_store(&lrclib, &lib, &row).await.unwrap();
        assert_eq!(st.synced.as_deref(), Some("[00:12.00]Тёплое место"));
        assert_eq!(lib.lyrics(hit.id).unwrap().unwrap().plain.as_deref(), Some("Тёплое место"));

        let row = lib.get_track_row(miss.id).unwrap().unwrap();
        let st = fetch_and_store(&lrclib, &lib, &row).await.unwrap();
        assert!(st.is_miss());
        assert!(lib.lyrics(miss.id).unwrap().unwrap().is_miss());
    }

    #[tokio::test]
    async fn server_errors_are_not_cached_as_misses_and_stop_the_backfill() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(502).set_body_raw("bad gateway", "text/plain"))
            .mount(&server)
            .await;
        let lrclib = Lrclib::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let tracks: Vec<_> = (0..6)
            .map(|i| library_track(&lib, "Кино", &format!("Песня {i}")).1)
            .collect();
        let row = lib.get_track_row(tracks[0].id).unwrap().unwrap();
        assert!(fetch_and_store(&lrclib, &lib, &row).await.is_err());

        backfill_lyrics(metadata(&server).await, lib.clone(), PathBuf::from("/nonexistent"), NO_WAIT, crossbeam_channel::unbounded().0, false).await;
        for t in &tracks {
            assert!(lib.lyrics(t.id).unwrap().is_none(), "outage must not leave misses");
        }
        // Gave up after a few consecutive failures instead of hammering every track.
        let requests = server.received_requests().await.unwrap().len();
        assert!(requests < tracks.len() * 3, "{requests} requests");
    }

    #[tokio::test]
    async fn network_failure_is_not_cached() {
        let lib = Library::open_in_memory().unwrap();
        let (_, t) = library_track(&lib, "Кино", "Группа крови");
        // Nothing listens on this port.
        let lrclib = Lrclib::with_base_url(http(), "http://127.0.0.1:9/").unwrap();
        let row = lib.get_track_row(t.id).unwrap().unwrap();
        assert!(fetch_and_store(&lrclib, &lib, &row).await.is_err());
        assert!(lib.lyrics(t.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn backfill_covers_every_track_once_and_stops() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .and(query_param("track_name", "Группа крови"))
            .respond_with(json(LRC_HIT))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/get"))
            .respond_with(ResponseTemplate::new(404).set_body_raw(r#"{"code":404}"#, "application/json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/search"))
            .respond_with(json("[]"))
            .mount(&server)
            .await;
        let lib = Library::open_in_memory().unwrap();
        let (_, a) = library_track(&lib, "Кино", "Группа крови");
        let (_, b) = library_track(&lib, "Кино", "Нет такой песни");

        backfill_lyrics(metadata(&server).await, lib.clone(), PathBuf::from("/nonexistent"), NO_WAIT, crossbeam_channel::unbounded().0, false).await;
        assert!(!lib.lyrics(a.id).unwrap().unwrap().is_miss());
        assert!(lib.lyrics(b.id).unwrap().unwrap().is_miss());
        assert!(lib.tracks_needing_lyrics(now_ms() - LYRICS_RETRY_MS, 10).unwrap().is_empty());
    }

    #[tokio::test]
    async fn photos_are_downloaded_once_and_misses_recorded() {
        let server = MockServer::start().await;
        let body = |name: &str, pic: &str| {
            format!(
                r#"{{"data": [{{"id": 1, "name": "{name}", "nb_fan": 10, "picture_xl": "{}/img/{pic}.jpg"}}]}}"#,
                server.uri()
            )
        };
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .and(query_param("q", "Кино"))
            .respond_with(json(&body("Кино", "kino")))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .respond_with(json(r#"{"data": []}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/artist/1/albums"))
            .respond_with(json(r#"{"data": [{"id": 5, "title": "Группа крови"}]}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/artist/1/top"))
            .respond_with(json(r#"{"data": []}"#))
            .mount(&server)
            .await;
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3];
        Mock::given(method("GET"))
            .and(path("/img/kino.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(jpeg.clone(), "image/jpeg"))
            .expect(1)
            .mount(&server)
            .await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (kino, _) = library_track(&lib, "Кино", "Группа крови");
        let (nobody, _) = library_track(&lib, "Никто Неизвестный", "x");
        let dir = tempfile::tempdir().unwrap();

        let saved = Arc::new(AtomicUsize::new(0));
        let counter = saved.clone();
        run_photos(
            deezer.clone(),
            lib.clone(),
            dir.path().to_owned(),
            Arc::default(),
            NO_WAIT,
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        )
        .await;

        assert_eq!(saved.load(Ordering::SeqCst), 1);
        let stored = lib.artist_photo(kino.id).unwrap().expect("photo path");
        assert_eq!(std::fs::read(&stored).unwrap(), jpeg);
        assert!(stored.starts_with(&dir.path().display().to_string()));
        assert_eq!(lib.artist_photo(nobody.id).unwrap(), None);
        assert!(lib.artist_photo_lookup(nobody.id).unwrap().is_some(), "miss is recorded");

        // A second pass has nothing to do (the mock expects a single download).
        run_photos(deezer, lib, dir.path().to_owned(), Arc::default(), NO_WAIT, |_| {
            panic!("no new photos expected")
        })
        .await;
    }

    #[tokio::test]
    async fn placeholder_images_and_html_are_not_saved() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .respond_with(json(&format!(
                r#"{{"data": [{{"id": 1, "name": "Кино", "picture_xl": "{}/img/x.jpg"}}]}}"#,
                server.uri()
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/img/x.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html>oops</html>", "text/html"))
            .mount(&server)
            .await;
        for tail in ["albums", "top"] {
            Mock::given(method("GET"))
                .and(path(format!("/artist/1/{tail}")))
                .respond_with(json(r#"{"data": [{"id": 5, "title": "Группа крови"}]}"#))
                .mount(&server)
                .await;
        }
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (kino, _) = library_track(&lib, "Кино", "Группа крови");
        let dir = tempfile::tempdir().unwrap();
        let out = fetch_artist_photo(&deezer, &lib, kino.id, "Кино", dir.path()).await;
        assert_eq!(out, PhotoOutcome::NotFound);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    /// Two artists share a name; the photo must come from the one who recorded
    /// the local track, or from nobody.
    #[tokio::test]
    async fn namesakes_get_the_photo_of_the_track_anchored_artist_or_none() {
        let server = MockServer::start().await;
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 9];
        Mock::given(method("GET"))
            .and(path("/search/track"))
            .respond_with(json(
                r#"{"data": [{"id": 1, "title": "Hollow Knight", "duration": 285,
                    "artist": {"id": 77, "name": "Christopher Larkin"}}]}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/artist/77"))
            .respond_with(json(&format!(
                r#"{{"id": 77, "name": "Christopher Larkin", "picture_xl": "{}/img/composer.jpg"}}"#,
                server.uri()
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/img/composer.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(jpeg.clone(), "image/jpeg"))
            .mount(&server)
            .await;
        // The name search would find a namesake with unrelated records.
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .respond_with(json(&format!(
                r#"{{"data": [{{"id": 5, "name": "Christopher Larkin", "nb_fan": 99,
                    "picture_xl": "{}/img/actor.jpg"}}]}}"#,
                server.uri()
            )))
            .mount(&server)
            .await;
        for tail in ["albums", "top"] {
            Mock::given(method("GET"))
                .and(path(format!("/artist/5/{tail}")))
                .respond_with(json(r#"{"data": [{"id": 1, "title": "Unrelated"}]}"#))
                .mount(&server)
                .await;
        }
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (larkin, _) = library_track(&lib, "Christopher Larkin", "Hollow Knight");
        let dir = tempfile::tempdir().unwrap();
        let out = fetch_artist_photo(&deezer, &lib, larkin.id, "Christopher Larkin", dir.path()).await;
        let PhotoOutcome::Saved(saved) = out else { panic!("{out:?}") };
        assert_eq!(std::fs::read(saved).unwrap(), jpeg);

        // Without an anchor (track unknown to Deezer) the namesake is rejected.
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/search/track"))
            .respond_with(json(r#"{"data": []}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .respond_with(json(&format!(
                r#"{{"data": [{{"id": 5, "name": "Christopher Larkin",
                    "picture_xl": "{}/img/actor.jpg"}}]}}"#,
                server.uri()
            )))
            .mount(&server)
            .await;
        for tail in ["albums", "top"] {
            Mock::given(method("GET"))
                .and(path(format!("/artist/5/{tail}")))
                .respond_with(json(r#"{"data": [{"id": 1, "title": "Unrelated"}]}"#))
                .mount(&server)
                .await;
        }
        let dir2 = tempfile::tempdir().unwrap();
        let (fresh, _) = library_track(&lib, "Another Larkin", "Some Local Rip 2");
        let out = fetch_artist_photo(&deezer, &lib, fresh.id, "Christopher Larkin", dir2.path()).await;
        assert_eq!(out, PhotoOutcome::NotFound);
        assert_eq!(lib.artist_photo(fresh.id).unwrap(), None);
    }

    #[tokio::test]
    async fn locked_photo_is_not_refetched() {
        let server = MockServer::start().await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (a, _) = library_track(&lib, "Кино", "Группа крови");
        let dir = tempfile::tempdir().unwrap();
        let mine = lib.set_custom_artist_photo(a.id, dir.path(), &[0xFF, 0xD8, 0xFF, 1]).unwrap();
        let out = fetch_artist_photo(&deezer, &lib, a.id, "Кино", dir.path()).await;
        assert_eq!(out, PhotoOutcome::NotFound);
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(lib.artist_photo(a.id).unwrap(), Some(mine.display().to_string()));
    }
}
