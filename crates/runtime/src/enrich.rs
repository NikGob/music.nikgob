//! Background enrichment from the network: lyrics for tracks that have none,
//! artist photos and readable artist names ([`crate::artist_names`]). All are cached in the library (including "nothing found"
//! markers, so the services are not asked again and again) and both stay off
//! when the app is offline (`ImportService::metadata` is `None`).
//!
//! The network parts are plain async functions over the metadata clients and the
//! library, so tests can run them against a mock server; the `Worker` glue at
//! the bottom only spawns them and applies their results to the view state.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nikgob_core::model::ArtistId;
use nikgob_core::streaming::{EntityKind, Service};
use nikgob_library::{Library, ROMAJI, StoredLyrics, TrackRow, now_ms};
use nikgob_metadata::{ArtistPicture, Deezer, Lrclib, Lyrics, Metadata, MetadataError};
use nikgob_view as v;

use crate::Worker;
use crate::artist_names::After;
use crate::import::Internal;

/// A lyrics lookup that found nothing is repeated after this long.
pub(crate) const LYRICS_RETRY_MS: i64 = 7 * 86_400_000;
/// Same for artist photos.
const PHOTO_RETRY_MS: i64 = 14 * 86_400_000;
const BATCH: u32 = 25;
/// Consecutive network failures after which a background job gives up until the next trigger.
pub(crate) const MAX_FAILURES: u32 = 3;

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

/// What a photo or names run found, for the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    /// A new photo of the artist.
    Photo(ArtistId),
    /// The artist's readable name, or that it is a role.
    Name(ArtistId),
    /// Roles of voice credits newly marked: the library's artists are
    /// read again.
    Reload,
}

/// Results of the background jobs on their way to the worker.
pub(crate) enum Event {
    Found(Found),
    PhotosDone,
    NamesDone,
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
    names_running: bool,
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
/// Only a definite answer is cached: lyrics, or LRCLIB saying it has none
/// (which never replaces lyrics stored before).
/// Any error (5xx, 429, network, timeout) is returned and nothing is stored.
pub(crate) async fn fetch_and_store(
    lrclib: &Lrclib,
    lib: &Library,
    row: &TrackRow,
) -> Result<StoredLyrics, MetadataError> {
    // A stream row without a known length carries 0: that is "unknown".
    let duration = row
        .duration_ms
        .filter(|&ms| ms > 0)
        .map(|ms| Duration::from_millis(ms.into()));
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
    // A miss never takes the place of lyrics found earlier (a search asked
    // again): they are kept.
    if stored.is_miss() && lib.lyrics(row.id).ok().flatten().is_some_and(|old| !old.is_miss()) {
        return Ok(stored);
    }
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
    crate::quiet::free().await;
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
            // Unattended: waits while a game is in front.
            crate::quiet::free().await;
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
    /// Deezer has same-named artists with photos, none proved this one;
    /// recorded, asked again by the next session.
    Unverified,
    /// Deezer could not answer for now (a 404, out of time); recorded,
    /// asked again after a day, then after longer each time in a row.
    Later,
    /// No network or an unexpected answer; nothing recorded, not asked
    /// again in this run.
    Failed,
}

/// When this session's first photo run started: an unverified photo
/// recorded before it is asked again.
fn session_start() -> i64 {
    static START: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *START.get_or_init(now_ms)
}

/// Photos whose file is gone (a deleted «Artists» folder) are looked up
/// again; not while the folder above theirs is missing too (a drive that
/// is not there now). Returns how many.
fn forget_gone_photos(lib: &Library) -> usize {
    let gone: Vec<ArtistId> = lib
        .artist_photos()
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, p)| {
            let p = Path::new(p);
            p.is_absolute() && !p.exists() && p.parent().and_then(Path::parent).is_some_and(Path::exists)
        })
        .map(|(id, _)| id)
        .collect();
    lib.forget_artist_photos(&gone).unwrap_or_else(|e| {
        log::warn!("forgetting the photos whose files are gone: {e}");
        0
    })
}

pub(crate) fn looks_like_image(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xD8, 0xFF]) || bytes.starts_with(&[0x89, b'P', b'N', b'G'])
}

/// How many local tracks are tried as anchors before falling back to the name.
const ANCHOR_TRACKS: usize = 3;
/// Local track / album titles handed to the name-search verification.
const VERIFY_TITLES: u32 = 60;

/// How long one name's Deezer lookup may take, and the image download.
#[cfg(not(test))]
const NAME_BUDGET: Duration = Duration::from_secs(40);
#[cfg(not(test))]
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(20);
#[cfg(test)]
const NAME_BUDGET: Duration = Duration::from_secs(2);
#[cfg(test)]
const DOWNLOAD_BUDGET: Duration = Duration::from_secs(2);

/// What Deezer says of an artist's photo, before anything is recorded.
enum Verdict {
    Image(Vec<u8>),
    /// Deezer has no usable photo.
    Missing,
    /// Same-named artists with photos, none proved this one.
    Unverified,
    /// Deezer could not answer for this artist now: a 404 somewhere in the
    /// lookup or the image download with nothing else proved, or out of
    /// time.
    Later,
    /// No network or an unexpected answer.
    Failed,
}

impl Verdict {
    /// Of the answers without a photo, the one asked again soonest wins.
    fn rank(&self) -> u8 {
        match self {
            Verdict::Missing => 0,
            Verdict::Later => 1,
            Verdict::Unverified => 2,
            Verdict::Image(_) | Verdict::Failed => 3,
        }
    }
}

/// `best` or `v`, whichever is asked again sooner.
fn sooner(best: Option<Verdict>, v: Verdict) -> Option<Verdict> {
    Some(match best {
        Some(b) if b.rank() >= v.rank() => b,
        _ => v,
    })
}

/// What one name finds on Deezer: a photo's url, or why not.
enum Named {
    Url(String),
    Not(Verdict),
}

/// The names Deezer is asked by: the artist's own, then its Latin name
/// when one was identified (an alias or a service, not a romaji guess,
/// which proves nobody).
fn photo_names(lib: &Library, id: ArtistId, name: &str) -> Vec<String> {
    let mut names = vec![name.to_owned()];
    let latin = lib.artist_latin(id).ok().flatten().filter(|l| l.source != ROMAJI).and_then(|l| l.name);
    names.extend(latin.filter(|l| !l.trim().is_empty() && l != name));
    names
}

/// One name's lookup: an anchor on the artist's own tracks (local ones
/// first), else the name search whose candidates' albums / top tracks must
/// overlap ours. A 404 on the way with nothing proved is no answer.
async fn named_lookup(deezer: &Deezer, anchors: &[&TrackRow], known: &[String], who: &str) -> Result<Named, MetadataError> {
    let mut gap = false;
    for t in anchors.iter().take(ANCHOR_TRACKS) {
        let duration = t.duration_ms.map(|ms| Duration::from_millis(ms.into()));
        match deezer.artist_picture_by_track(who, &t.title, duration).await {
            Ok(Some(u)) => return Ok(Named::Url(u)),
            Err(MetadataError::NotFound) => gap = true,
            // Any other failed anchor just means no anchor; the name search decides.
            Ok(None) | Err(_) => {}
        }
    }
    Ok(match deezer.verified_artist_lookup(who, known).await {
        Ok(ArtistPicture::Found(u)) => Named::Url(u),
        Ok(ArtistPicture::Unverified) => Named::Not(Verdict::Unverified),
        Ok(ArtistPicture::Missing) if !gap => Named::Not(Verdict::Missing),
        Ok(ArtistPicture::Missing) | Err(MetadataError::NotFound) => Named::Not(Verdict::Later),
        Err(e) => return Err(e),
    })
}

/// Finds the artist on Deezer and downloads its photo; records nothing.
///
/// A photo is only taken from a Deezer artist known to be this one: the
/// Deezer artist the library has linked it to, else one of the artist's
/// own tracks (its `artist.id`), else a name search whose albums / top
/// tracks overlap the local titles. No proof, no photo. When the own name
/// brings no photo, the identified Latin name is asked the same way. Each
/// name has its own time: what the own name answered stands when the
/// Latin one runs out of time or fails.
async fn deezer_verdict(deezer: &Deezer, lib: &Library, id: ArtistId, name: &str) -> Verdict {
    let tracks = lib.tracks_of_artist(id, 0, VERIFY_TITLES).unwrap_or_default();
    let mut anchors: Vec<&TrackRow> = tracks.iter().filter(|t| t.has_local).collect();
    anchors.extend(tracks.iter().filter(|t| !t.has_local));
    let mut known: Vec<String> = tracks.iter().map(|t| t.title.clone()).collect();
    known.extend(tracks.iter().filter_map(|t| t.album.clone()));
    let linked = lib
        .external_ids(EntityKind::Artist, *id.as_uuid())
        .unwrap_or_default()
        .into_iter()
        .find(|r| r.service == Service::Deezer)
        .and_then(|r| r.id.parse::<u64>().ok());
    let mut url = None;
    let mut best = None;
    if let Some(n) = linked {
        match tokio::time::timeout(NAME_BUDGET, deezer.artist(n)).await {
            Ok(Ok(a)) => url = a.best_picture().map(str::to_owned),
            Ok(Err(MetadataError::NotFound)) | Err(_) => best = Some(Verdict::Later),
            Ok(Err(_)) => {}
        }
    }
    for (i, who) in photo_names(lib, id, name).iter().enumerate() {
        if url.is_some() {
            break;
        }
        match tokio::time::timeout(NAME_BUDGET, named_lookup(deezer, &anchors, &known, who)).await {
            Ok(Ok(Named::Url(u))) => url = Some(u),
            Ok(Ok(Named::Not(v))) => best = sooner(best, v),
            Ok(Err(e)) if i == 0 => {
                log::debug!("photo of {name}: {e}");
                return Verdict::Failed;
            }
            Err(_) if i == 0 => best = sooner(best, Verdict::Later),
            // The Latin name is a second chance: its failure keeps what the own name answered.
            Ok(Err(_)) | Err(_) => break,
        }
    }
    let Some(url) = url else {
        return best.unwrap_or(Verdict::Missing);
    };
    match tokio::time::timeout(DOWNLOAD_BUDGET, deezer.download_image(&url)).await {
        Ok(Ok(b)) if looks_like_image(&b) => Verdict::Image(b),
        Ok(Ok(_)) => Verdict::Missing,
        // A picture that is not there now, or slow: no answer.
        Ok(Err(MetadataError::NotFound)) | Err(_) => Verdict::Later,
        Ok(Err(e)) => {
            log::debug!("photo of {name}: {e}");
            Verdict::Failed
        }
    }
}

/// Records a verdict without a photo: a miss of days, an unverified one
/// asked again by the next session, or one asked again later.
fn record_verdict(lib: &Library, id: ArtistId, verdict: &Verdict) -> PhotoOutcome {
    match verdict {
        Verdict::Missing => {
            let _ = lib.set_artist_photo(id, None);
            PhotoOutcome::NotFound
        }
        Verdict::Unverified => {
            let _ = lib.set_artist_photo_unverified(id);
            PhotoOutcome::Unverified
        }
        Verdict::Later => {
            let _ = lib.set_artist_photo_later(id);
            PhotoOutcome::Later
        }
        Verdict::Image(_) | Verdict::Failed => PhotoOutcome::Failed,
    }
}

/// Deezer's photo of the artist, downloaded into `dir` and recorded (see
/// [`deezer_verdict`]). A photo locked by the user is left alone.
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
    match deezer_verdict(deezer, lib, id, name).await {
        Verdict::Image(bytes) => save_photo(lib, id, name, dir, &bytes, "deezer"),
        v => record_verdict(lib, id, &v),
    }
}

/// Writes the photo `bytes` of `id` into `dir` and records it as from
/// `source`; never for a role (checked again right before) nor over a
/// photo the user chose.
fn save_photo(lib: &Library, id: ArtistId, name: &str, dir: &Path, bytes: &[u8], source: &str) -> PhotoOutcome {
    let role = lib.artist_latin(id).ok().flatten().is_some_and(|r| r.character);
    if role || lib.artist_photo_locked(id).unwrap_or(false) {
        return PhotoOutcome::NotFound;
    }
    let path = dir.join(format!("{id}.jpg"));
    let tmp = path.with_extension("part");
    let saved = std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&tmp, bytes))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = saved {
        log::warn!("saving the photo of {name}: {e}");
        return PhotoOutcome::Failed;
    }
    if let Err(e) = lib.set_artist_photo_from(id, Some(&path.display().to_string()), source) {
        log::warn!("recording the photo of {name}: {e}");
        return PhotoOutcome::Failed;
    }
    PhotoOutcome::Saved(path)
}

/// The photo of an artist from every source ([`crate::artist_names`]):
///
/// 1. who it is is settled first: a role gets no lookup at all, and when
///    that cannot be settled now (MusicBrainz busy) nothing is asked or
///    recorded;
/// 2. Deezer;
/// 3. when Deezer has no photo it could prove this artist's: the
///    connected services, then Wikimedia Commons. Deezer's verdict is
///    recorded when none has one; when one of them could not answer for
///    now, the artist is asked again by the next session, not after days.
pub(crate) async fn fetch_photo(
    deezer: &Deezer,
    after: &After,
    lib: &Library,
    id: ArtistId,
    name: &str,
    dir: &Path,
    found: &impl Fn(Found),
) -> PhotoOutcome {
    if lib.artist_photo_locked(id).unwrap_or(false) {
        return PhotoOutcome::NotFound;
    }
    let known = match crate::artist_names::known(after, lib, id, name, found).await {
        Ok(k) => k,
        Err(_) => return PhotoOutcome::Failed,
    };
    if known.character {
        return PhotoOutcome::NotFound;
    }
    let verdict = match deezer_verdict(deezer, lib, id, name).await {
        Verdict::Image(bytes) => return save_photo(lib, id, name, dir, &bytes, "deezer"),
        Verdict::Failed => return PhotoOutcome::Failed,
        v => v,
    };
    let more = crate::artist_names::photo_after_deezer(after, lib, id, name, &known);
    match tokio::time::timeout(Duration::from_secs(90), more).await {
        Ok(Ok(Some((bytes, source)))) => save_photo(lib, id, name, dir, &bytes, source),
        Ok(Ok(None)) => record_verdict(lib, id, &verdict),
        Ok(Err(_)) | Err(_) => record_verdict(lib, id, &Verdict::Unverified),
    }
}

/// Fetches photos for artists that have none (artists whose pages were opened
/// first), telling `found` of each new one; an unverified one recorded
/// before `unverified_before` is asked again, a photo whose file is gone
/// too. Says in one log line what the run did.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_photos(
    deezer: Deezer,
    after: After,
    lib: Library,
    dir: PathBuf,
    unverified_before: i64,
    priority: Arc<Mutex<Vec<(ArtistId, String)>>>,
    pace: Pace,
    found: impl Fn(Found),
) {
    tokio::time::sleep(pace.start).await;
    crate::quiet::free().await;
    let mut tally = PhotoTally {
        gone: forget_gone_photos(&lib),
        ..PhotoTally::default()
    };
    let queue = PhotoQueue {
        priority: &priority,
        unverified_before,
        pace,
    };
    photo_batches(&deezer, &after, &lib, &dir, &queue, &found, &mut tally).await;
    tally.log();
}

/// What one photo run did, for its log line.
#[derive(Default)]
struct PhotoTally {
    saved: u32,
    not_found: u32,
    unverified: u32,
    later: u32,
    failed: u32,
    /// Photos whose file was gone, looked up again.
    gone: usize,
}

impl PhotoTally {
    fn log(&self) {
        if self.saved + self.not_found + self.unverified + self.later + self.failed == 0 && self.gone == 0 {
            return;
        }
        let gone = if self.gone > 0 { format!(", {} files gone", self.gone) } else { String::new() };
        log::info!(
            "artist photos: {} saved, {} not found, {} unverified, {} later, {} failed{gone}",
            self.saved,
            self.not_found,
            self.unverified,
            self.later,
            self.failed
        );
    }
}

/// Which artists a photo run takes, in what order and at what pace.
struct PhotoQueue<'a> {
    /// Artists whose pages were opened: first.
    priority: &'a Mutex<Vec<(ArtistId, String)>>,
    unverified_before: i64,
    pace: Pace,
}

/// [`run_photos`]' batches until nothing is due (or the network failed
/// [`MAX_FAILURES`] times in a row). An artist whose lookup failed is not
/// asked again in this run: the queue goes on past it.
async fn photo_batches(
    deezer: &Deezer,
    after: &After,
    lib: &Library,
    dir: &Path,
    q: &PhotoQueue<'_>,
    found: &impl Fn(Found),
    tally: &mut PhotoTally,
) {
    let mut failures = 0;
    let mut failed: HashSet<ArtistId> = HashSet::new();
    loop {
        // The failed ones are still due: asked for on top, then left out.
        let limit = BATCH.saturating_add(u32::try_from(failed.len()).unwrap_or(u32::MAX));
        let batch: Vec<(ArtistId, String)> =
            match lib.artists_needing_photo(now_ms() - PHOTO_RETRY_MS, q.unverified_before, limit) {
                Ok(b) => b.into_iter().filter(|(id, _)| !failed.contains(id)).collect(),
                Err(e) => {
                    log::warn!("photo queue: {e}");
                    return;
                }
            };
        let mut queue = batch.into_iter();
        let mut did_any = false;
        loop {
            let urgent = q.priority.lock().ok().and_then(|mut p| p.pop());
            let Some((id, name)) = urgent.or_else(|| queue.next()) else {
                break;
            };
            if failed.contains(&id) {
                continue;
            }
            did_any = true;
            match fetch_photo(deezer, after, lib, id, &name, dir, found).await {
                PhotoOutcome::Saved(_) => {
                    failures = 0;
                    tally.saved += 1;
                    found(Found::Photo(id));
                }
                PhotoOutcome::NotFound => {
                    failures = 0;
                    tally.not_found += 1;
                }
                PhotoOutcome::Unverified => {
                    failures = 0;
                    tally.unverified += 1;
                }
                PhotoOutcome::Later => {
                    failures = 0;
                    tally.later += 1;
                }
                PhotoOutcome::Failed => {
                    failures += 1;
                    tally.failed += 1;
                    failed.insert(id);
                    if failures >= MAX_FAILURES {
                        return;
                    }
                }
            }
            tokio::time::sleep(q.pace.between).await;
            // Unattended: waits while a game is in front.
            crate::quiet::free().await;
        }
        if !did_any {
            return;
        }
    }
}

/// Marks the roles of the voice credits ([`Library::mark_voice_roles`]):
/// they get no photo lookups. Returns how many were new.
fn mark_roles(lib: &Library, found: &dyn Fn(Found)) -> usize {
    let roles = lib.mark_voice_roles().unwrap_or_else(|e| {
        log::warn!("marking the roles of voice credits: {e}");
        0
    });
    if roles > 0 {
        found(Found::Reload);
    }
    roles
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

    /// Looks for artist photos in the background, then for the readable
    /// names of the artists that need one ([`crate::artist_names`]); `first`
    /// (an artist page that was just opened) goes ahead of the photo queue.
    pub(crate) fn kick_photos(&mut self, first: Option<ArtistId>) {
        let lib = self.lib.clone();
        // The connected services are photo sources after Deezer.
        let services: Vec<Arc<dyn nikgob_core::streaming::StreamService>> =
            [nikgob_core::streaming::Service::Youtube, nikgob_core::streaming::Service::Spotify]
                .into_iter()
                .filter_map(|k| self.service(k))
                .collect();
        let session = session_start();
        let first = first.and_then(|id| {
            let due = lib.artist_photo_due(id, now_ms() - PHOTO_RETRY_MS, session).ok()?;
            let artist = lib.get_artist(id).ok().flatten().filter(|a| due && !nikgob_core::names::is_various_artists(&a.name))?;
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
        let tx = svc.internal();
        let found_tx = tx.clone();
        let found = move |f: Found| {
            let _ = found_tx.send(Internal::Enrich(Event::Found(f)));
        };
        // The roles of voice credits are marked by whichever run starts
        // first ([`mark_roles`]).
        if !svc.enrich.names_running {
            svc.enrich.names_running = true;
            let (meta, services, lib, tx, found) = (meta.clone(), services.clone(), lib.clone(), tx.clone(), found.clone());
            svc.spawn(async move {
                let roles = mark_roles(&lib, &found);
                crate::artist_names::run_names(meta, services, lib, Pace::PHOTOS, roles, &found).await;
                let _ = tx.send(Internal::Enrich(Event::NamesDone));
            });
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
        svc.spawn(async move {
            mark_roles(&lib, &found);
            let deezer = meta.deezer.clone();
            let after = After { meta: Some(meta), services, gate: Arc::default() };
            run_photos(deezer, after, lib, dir, session, priority, Pace::PHOTOS, &found).await;
            let _ = tx.send(Internal::Enrich(Event::PhotosDone));
        });
    }

    /// A new photo or readable name of `id`: its cards everywhere show it.
    fn artist_card_changed(&mut self, id: ArtistId) {
        self.res.invalidate_photos();
        let image = self.res.photo(id);
        let name = self.lib.get_artist(id).ok().flatten().map(|a| a.name).unwrap_or_default();
        let (latin, character) = self.res.latin_name(id, &name);
        self.publish(|s| {
            let fix = |a: &mut v::ArtistCard| {
                if a.id == id {
                    // A role shows no automatic photo.
                    if image.is_some() || character {
                        a.image = image.clone();
                    }
                    a.latin = latin.clone();
                    a.character = character;
                }
            };
            s.library.artists.iter_mut().for_each(fix);
            s.search.artists.iter_mut().for_each(fix);
            if let Some(a) = s.artist.as_mut() {
                fix(&mut a.card);
            }
        });
        // Kept for a home page on its way too.
        self.patch_home(move |home| {
            let fix = |a: &mut v::ArtistCard| {
                if a.id == id {
                    // A role shows no automatic photo.
                    if image.is_some() || character {
                        a.image = image.clone();
                    }
                    a.latin = latin.clone();
                    a.character = character;
                }
            };
            home.artist_mixes.iter_mut().for_each(fix);
            for sc in &mut home.shortcuts {
                if let v::Shortcut::Artist(a) = sc {
                    fix(a);
                }
            }
        });
    }

    pub(crate) fn enrich_event(&mut self, ev: Event) {
        match ev {
            Event::Found(Found::Photo(id) | Found::Name(id)) => self.artist_card_changed(id),
            Event::Found(Found::Reload) => {
                self.res.invalidate_photos();
                self.home_input_changed();
                self.load_library();
            }
            Event::PhotosDone => {
                if let Some(svc) = &mut self.import {
                    svc.enrich.photos_running = false;
                }
            }
            Event::NamesDone => {
                if let Some(svc) = &mut self.import {
                    svc.enrich.names_running = false;
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

        // Lyrics stored earlier stay when LRCLIB has none now (asked again).
        lib.put_lyrics(miss.id, Some("[00:01.00]Было"), None, false).unwrap();
        assert!(fetch_and_store(&lrclib, &lib, &row).await.unwrap().is_miss());
        assert_eq!(lib.lyrics(miss.id).unwrap().unwrap().synced.as_deref(), Some("[00:01.00]Было"));
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
            .and(path("/search/track"))
            .respond_with(json(r#"{"data": []}"#))
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
            After::default(),
            lib.clone(),
            dir.path().to_owned(),
            now_ms(),
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
        run_photos(deezer, After::default(), lib.clone(), dir.path().to_owned(), now_ms() + 1, Arc::default(), NO_WAIT, |_| {
            panic!("no new photos expected")
        })
        .await;

        // A photo whose file is gone is looked up again.
        std::fs::remove_file(&stored).unwrap();
        assert_eq!(forget_gone_photos(&lib), 1);
        assert_eq!(lib.artist_photo(kino.id).unwrap(), None);
        assert!(lib.artist_photo_due(kino.id, now_ms() - PHOTO_RETRY_MS, 0).unwrap());
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
        // Not «Deezer has no photo»: asked again by the next session.
        assert_eq!(out, PhotoOutcome::Unverified);
        assert_eq!(lib.artist_photo(fresh.id).unwrap(), None);
        let retry = now_ms() - PHOTO_RETRY_MS;
        assert!(!lib.artist_photo_due(fresh.id, retry, now_ms() - 60_000).unwrap(), "not again this session");
        assert!(lib.artist_photo_due(fresh.id, retry, now_ms() + 1).unwrap(), "again the next one");
    }

    /// Deezer has no photo of a seiyuu: her MusicBrainz artist (the id her
    /// files carry) links a Wikidata item whose Commons picture is hers,
    /// and her English alias becomes the name shown beside her own.
    #[tokio::test]
    async fn a_photo_deezer_lacks_comes_from_wikimedia_through_musicbrainz() {
        const MBID: &str = "0b2f0b5a-0000-4000-8000-000000000001";
        let server = MockServer::start().await;
        for p in ["/search", "/search/track", "/search/artist"] {
            Mock::given(method("GET")).and(path(p)).respond_with(json(r#"{"data": []}"#)).mount(&server).await;
        }
        Mock::given(method("GET"))
            .and(path(format!("/artist/{MBID}")))
            .respond_with(json(&format!(
                r#"{{"id": "{MBID}", "name": "佐宗綾子", "sort-name": "Saso, Ayako", "type": "Person",
                    "aliases": [{{"name": "Ayako Saso", "locale": "en", "type": "Artist name", "primary": true}}],
                    "relations": [{{"type": "wikidata", "url": {{"resource": "https://www.wikidata.org/wiki/Q11"}}}}]}}"#
            )))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/w/api.php"))
            .respond_with(json(&format!(
                r#"{{"entities": {{"Q11": {{"labels": {{"en": {{"value": "Ayako Saso"}}}},
                    "claims": {{"P18": [{{"mainsnak": {{"datavalue": {{"value": "Saso.jpg"}}}}}}],
                                "P434": [{{"mainsnak": {{"datavalue": {{"value": "{MBID}"}}}}}}]}}}}}}}}"#
            )))
            .mount(&server)
            .await;
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 7];
        Mock::given(method("GET"))
            .and(path("/wiki/Special:FilePath/Saso.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(jpeg.clone(), "image/jpeg"))
            .mount(&server)
            .await;
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        meta.musicbrainz = nikgob_metadata::MusicBrainz::with_base_url(http(), &server.uri()).unwrap();
        meta.wikidata =
            nikgob_metadata::Wikidata::with_base_urls(http(), &format!("{}/w/", server.uri()), &format!("{}/wiki/", server.uri()))
                .unwrap();
        let lib = Library::open_in_memory().unwrap();
        let mut saso = Artist::new("佐宗綾子");
        saso.mbid = Some(MBID.into());
        let saso = lib.upsert_artist(&saso).unwrap();
        let t = lib.upsert_track(&Track::new("Song", ArtistCredit::single("佐宗綾子", Some(saso.id)))).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, "/nonexistent/song.flac", Codec::Flac).with_quality(false)).unwrap();
        let dir = tempfile::tempdir().unwrap();

        // Deezer alone has nothing.
        let alone = fetch_photo(&meta.deezer, &After::default(), &lib, saso.id, "佐宗綾子", dir.path(), &|_| {}).await;
        assert_eq!(alone, PhotoOutcome::NotFound);
        let found = Mutex::new(Vec::new());
        let after = After { meta: Some(meta.clone()), ..After::default() };
        let out = fetch_photo(&meta.deezer, &after, &lib, saso.id, "佐宗綾子", dir.path(), &|f| found.lock().unwrap().push(f)).await;
        let PhotoOutcome::Saved(saved) = out else { panic!("{out:?}") };
        assert_eq!(std::fs::read(saved).unwrap(), jpeg);
        assert_eq!(found.lock().unwrap().as_slice(), [Found::Name(saso.id)]);
        let row = lib.artist_latin(saso.id).unwrap().unwrap();
        assert_eq!((row.name.as_deref(), row.wikidata.as_deref()), (Some("Ayako Saso"), Some("Q11")));
    }

    /// The artist a photo is looked up for, with its MusicBrainz id, and
    /// clients pointed at `server`.
    fn tagged(server: &MockServer, name: &str) -> (Metadata, Library, ArtistId) {
        let mut meta = Metadata::new(&MetadataConfig::new("tests@example.invalid")).unwrap();
        meta.deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        meta.musicbrainz = nikgob_metadata::MusicBrainz::with_base_url(http(), &server.uri()).unwrap();
        meta.wikidata =
            nikgob_metadata::Wikidata::with_base_urls(http(), &format!("{}/w/", server.uri()), &format!("{}/wiki/", server.uri()))
                .unwrap();
        let lib = Library::open_in_memory().unwrap();
        let mut a = Artist::new(name);
        a.mbid = Some(TAGGED.into());
        let a = lib.upsert_artist(&a).unwrap();
        let t = lib.upsert_track(&Track::new("Song", ArtistCredit::single(name, Some(a.id)))).unwrap();
        lib.add_source(&Source::new(t.id, SourceKind::LocalFile, "/nonexistent/song.flac", Codec::Flac).with_quality(false)).unwrap();
        (meta, lib, a.id)
    }

    const TAGGED: &str = "0b2f0b5a-0000-4000-8000-000000000003";

    /// A role is known before any photo lookup: MusicBrainz says Character,
    /// so Deezer and the rest are never asked and nothing is saved.
    #[tokio::test]
    async fn a_role_is_settled_before_any_photo_lookup() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/artist/{TAGGED}")))
            .respond_with(json(&format!(r#"{{"id": "{TAGGED}", "name": "ナナチ", "type": "Character"}}"#)))
            .mount(&server)
            .await;
        let (meta, lib, id) = tagged(&server, "ナナチ");
        let dir = tempfile::tempdir().unwrap();
        let after = After { meta: Some(meta.clone()), ..After::default() };
        let out = fetch_photo(&meta.deezer, &after, &lib, id, "ナナチ", dir.path(), &|_| {}).await;
        assert_eq!(out, PhotoOutcome::NotFound);
        let asked: Vec<String> = server.received_requests().await.unwrap().iter().map(|r| r.url.path().to_owned()).collect();
        assert_eq!(asked, vec![format!("/artist/{TAGGED}")], "only who it is");
        assert!(lib.artist_latin(id).unwrap().unwrap().character);
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
        assert!(!lib.artist_photo_due(id, 0, now_ms() + 1).unwrap(), "never queued again");
    }

    /// MusicBrainz busy: who it is cannot be settled, so no photo lookup and
    /// nothing recorded.
    #[tokio::test]
    async fn a_busy_identity_lookup_records_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let (meta, lib, id) = tagged(&server, "佐宗綾子");
        let dir = tempfile::tempdir().unwrap();
        let after = After { meta: Some(meta.clone()), ..After::default() };
        let out = fetch_photo(&meta.deezer, &after, &lib, id, "佐宗綾子", dir.path(), &|_| {}).await;
        assert_eq!(out, PhotoOutcome::Failed);
        assert_eq!(lib.artist_photo_lookup(id).unwrap(), None, "no miss");
        assert_eq!(lib.artist_latin(id).unwrap(), None);
    }

    /// A recent identity: its Wikidata item is read again for the picture
    /// and checked against the MusicBrainz id. Wikidata busy: asked again by
    /// the next session, not a miss of days. An item that names another
    /// MusicBrainz artist: no photo.
    #[tokio::test]
    async fn a_cached_wikidata_item_is_checked_and_a_busy_one_is_no_miss() {
        let server = MockServer::start().await;
        for p in ["/search/track", "/search/artist"] {
            Mock::given(method("GET")).and(path(p)).respond_with(json(r#"{"data": []}"#)).mount(&server).await;
        }
        Mock::given(method("GET")).and(path("/w/api.php")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let (meta, lib, id) = tagged(&server, "佐宗綾子");
        let known = nikgob_library::LatinFound {
            mbid: Some(TAGGED.into()),
            wikidata: Some("Q11".into()),
            complete: true,
            ..nikgob_library::LatinFound::default()
        };
        lib.set_artist_latin(id, &known).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let after = After { meta: Some(meta.clone()), ..After::default() };
        let out = fetch_photo(&meta.deezer, &after, &lib, id, "佐宗綾子", dir.path(), &|_| {}).await;
        assert_eq!(out, PhotoOutcome::Unverified);
        assert!(!lib.artist_photo_due(id, now_ms() - PHOTO_RETRY_MS, now_ms() - 60_000).unwrap(), "not again this session");
        assert!(lib.artist_photo_due(id, now_ms() - PHOTO_RETRY_MS, now_ms() + 1).unwrap(), "again the next one");

        server.reset().await;
        for p in ["/search/track", "/search/artist"] {
            Mock::given(method("GET")).and(path(p)).respond_with(json(r#"{"data": []}"#)).mount(&server).await;
        }
        Mock::given(method("GET"))
            .and(path("/w/api.php"))
            .respond_with(json(
                r#"{"entities": {"Q11": {"claims": {
                    "P18": [{"mainsnak": {"datavalue": {"value": "Someone.jpg"}}}],
                    "P434": [{"mainsnak": {"datavalue": {"value": "0b2f0b5a-0000-4000-8000-000000000009"}}}]}}}}"#,
            ))
            .mount(&server)
            .await;
        let after = After { meta: Some(meta.clone()), ..After::default() };
        let out = fetch_photo(&meta.deezer, &after, &lib, id, "佐宗綾子", dir.path(), &|_| {}).await;
        assert_eq!(out, PhotoOutcome::NotFound);
        let asked = server.received_requests().await.unwrap();
        assert!(asked.iter().all(|r| !r.url.path().contains("FilePath")), "another artist's picture is not fetched");
    }

    /// The searches and pages of a Deezer artist `id` called `name`, with
    /// a picture and `titles` as its albums and top tracks.
    async fn deezer_artist(server: &MockServer, id: u64, name: &str, titles: &str) {
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .and(query_param("q", name))
            .respond_with(json(&format!(
                r#"{{"data": [{{"id": {id}, "name": "{name}", "nb_fan": 9, "picture_xl": "{}/img/{id}.jpg"}}]}}"#,
                server.uri()
            )))
            .mount(server)
            .await;
        for tail in ["albums", "top"] {
            Mock::given(method("GET"))
                .and(path(format!("/artist/{id}/{tail}")))
                .respond_with(json(&format!(r#"{{"data": [{{"id": 5, "title": "{titles}"}}]}}"#)))
                .mount(server)
                .await;
        }
    }

    async fn picture(server: &MockServer, id: u64, jpeg: &[u8]) {
        Mock::given(method("GET"))
            .and(path(format!("/img/{id}.jpg")))
            .respond_with(ResponseTemplate::new(200).set_body_raw(jpeg.to_vec(), "image/jpeg"))
            .mount(server)
            .await;
    }

    async fn empty_searches(server: &MockServer) {
        for p in ["/search/track", "/search/artist"] {
            Mock::given(method("GET")).and(path(p)).respond_with(json(r#"{"data": []}"#)).mount(server).await;
        }
    }

    /// A picture the CDN answers 404 for is not «Deezer has no photo»: it
    /// is asked again after a day, not for two weeks, and not by the next
    /// session either.
    #[tokio::test]
    async fn a_404_picture_is_asked_again_later_not_every_session() {
        let server = MockServer::start().await;
        deezer_artist(&server, 1, "Кино", "Группа крови").await;
        empty_searches(&server).await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (kino, _) = library_track(&lib, "Кино", "Группа крови");
        let dir = tempfile::tempdir().unwrap();
        let out = fetch_artist_photo(&deezer, &lib, kino.id, "Кино", dir.path()).await;
        assert_eq!(out, PhotoOutcome::Later);
        let retry = now_ms() - PHOTO_RETRY_MS;
        assert!(!lib.artist_photo_due(kino.id, retry, now_ms() + 1).unwrap(), "not by the next session");
        // The next session's run asks Deezer nothing.
        server.reset().await;
        run_photos(deezer, After::default(), lib.clone(), dir.path().to_owned(), now_ms() + 1, Arc::default(), NO_WAIT, |_| {
            panic!("no photo expected")
        })
        .await;
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    /// The track anchors meet a 404 and the name search finds nobody:
    /// not proved absent, so no miss of days.
    #[tokio::test]
    async fn an_anchor_404_with_nothing_found_is_no_miss() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/search/artist")).respond_with(json(r#"{"data": []}"#)).mount(&server).await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (a, _) = library_track(&lib, "Кино", "Группа крови");
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fetch_artist_photo(&deezer, &lib, a.id, "Кино", dir.path()).await, PhotoOutcome::Later);
    }

    /// Deezer knows the artist only by the Latin name the app found for
    /// it: that name is asked, and the photo is still proved by a title.
    #[tokio::test]
    async fn the_latin_name_is_asked_when_the_own_one_finds_nothing() {
        let server = MockServer::start().await;
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 4];
        deezer_artist(&server, 3, "Yoshiaki Fujisawa", "ギレーヌと二人きり").await;
        picture(&server, 3, &jpeg).await;
        empty_searches(&server).await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (a, _) = library_track(&lib, "藤澤慶昌", "ギレーヌと二人きり");
        let latin = nikgob_library::LatinFound {
            name: Some("Yoshiaki Fujisawa".into()),
            source: "musicbrainz".into(),
            complete: true,
            ..nikgob_library::LatinFound::default()
        };
        lib.set_artist_latin(a.id, &latin).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let out = fetch_artist_photo(&deezer, &lib, a.id, "藤澤慶昌", dir.path()).await;
        let PhotoOutcome::Saved(saved) = out else { panic!("{out:?}") };
        assert_eq!(std::fs::read(saved).unwrap(), jpeg);
    }

    /// A romaji reading is a guess, not the artist: Deezer is never asked
    /// by it.
    #[tokio::test]
    async fn a_romaji_guess_is_never_asked() {
        let server = MockServer::start().await;
        empty_searches(&server).await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (a, _) = library_track(&lib, "ふじさわ", "ギレーヌと二人きり");
        let guess = nikgob_library::LatinFound {
            name: Some("Fujisawa".into()),
            source: ROMAJI.into(),
            complete: true,
            ..nikgob_library::LatinFound::default()
        };
        lib.set_artist_latin(a.id, &guess).unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fetch_artist_photo(&deezer, &lib, a.id, "ふじさわ", dir.path()).await, PhotoOutcome::NotFound);
        let asked = server.received_requests().await.unwrap();
        assert!(!asked.is_empty());
        assert!(asked.iter().all(|r| !r.url.query().unwrap_or("").contains("Fujisawa")), "asked by the guess");
    }

    /// The Latin name's lookup runs out of time: what the own name
    /// answered (an unverified namesake) stands, not a failure.
    #[tokio::test]
    async fn a_slow_latin_lookup_keeps_the_own_names_answer() {
        let server = MockServer::start().await;
        deezer_artist(&server, 4, "藤澤慶昌", "『無職転生』 オリジナル･サウンドトラック").await;
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .and(query_param("q", "Yoshiaki Fujisawa"))
            .respond_with(json(r#"{"data": []}"#).set_delay(NAME_BUDGET * 3))
            .mount(&server)
            .await;
        empty_searches(&server).await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (a, _) = library_track(&lib, "藤澤慶昌", "ギレーヌと二人きり");
        let latin = nikgob_library::LatinFound {
            name: Some("Yoshiaki Fujisawa".into()),
            source: "musicbrainz".into(),
            complete: true,
            ..nikgob_library::LatinFound::default()
        };
        lib.set_artist_latin(a.id, &latin).unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(fetch_artist_photo(&deezer, &lib, a.id, "藤澤慶昌", dir.path()).await, PhotoOutcome::Unverified);
    }

    /// Who the artist is (its MusicBrainz id) does not tell which Deezer
    /// artist of its name it is: no photo on the name alone. The Deezer
    /// artist the library has linked it to gives its photo.
    #[tokio::test]
    async fn only_a_linked_deezer_artist_is_taken_without_titles() {
        let server = MockServer::start().await;
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 6];
        deezer_artist(&server, 4, "藤澤慶昌", "『無職転生』 オリジナル･サウンドトラック").await;
        picture(&server, 4, &jpeg).await;
        Mock::given(method("GET"))
            .and(path("/artist/4"))
            .respond_with(json(&format!(
                r#"{{"id": 4, "name": "藤澤慶昌", "picture_xl": "{}/img/4.jpg"}}"#,
                server.uri()
            )))
            .mount(&server)
            .await;
        empty_searches(&server).await;
        let (meta, lib, id) = tagged(&server, "藤澤慶昌");
        let known = nikgob_library::LatinFound {
            mbid: Some(TAGGED.into()),
            complete: true,
            ..nikgob_library::LatinFound::default()
        };
        lib.set_artist_latin(id, &known).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let after = After { meta: Some(meta.clone()), ..After::default() };
        let out = fetch_photo(&meta.deezer, &after, &lib, id, "藤澤慶昌", dir.path(), &|_| {}).await;
        assert_eq!(out, PhotoOutcome::Unverified);
        let linked = nikgob_core::streaming::RemoteId::new(Service::Deezer, "4");
        lib.set_external_id(EntityKind::Artist, &linked, *id.as_uuid()).unwrap();
        let out = fetch_photo(&meta.deezer, &after, &lib, id, "藤澤慶昌", dir.path(), &|_| {}).await;
        let PhotoOutcome::Saved(saved) = out else { panic!("{out:?}") };
        assert_eq!(std::fs::read(saved).unwrap(), jpeg);
    }

    /// An artist whose lookup fails is asked once per run; the queue goes
    /// on to the next one instead of asking it again until the run gives up.
    #[tokio::test]
    async fn a_failing_artist_is_asked_once_and_the_queue_goes_on() {
        let server = MockServer::start().await;
        let jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 8];
        Mock::given(method("GET"))
            .and(path("/search/artist"))
            .and(query_param("q", "Alpha"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        deezer_artist(&server, 2, "Beta", "Beta Song").await;
        picture(&server, 2, &jpeg).await;
        empty_searches(&server).await;
        let deezer = Deezer::with_base_url(http(), &server.uri()).unwrap();
        let lib = Library::open_in_memory().unwrap();
        let (alpha, _) = library_track(&lib, "Alpha", "Alpha Song");
        let (beta, _) = library_track(&lib, "Beta", "Beta Song");
        let dir = tempfile::tempdir().unwrap();
        run_photos(deezer, After::default(), lib.clone(), dir.path().to_owned(), now_ms(), Arc::default(), NO_WAIT, |_| {})
            .await;
        assert!(lib.artist_photo(beta.id).unwrap().is_some());
        assert_eq!(lib.artist_photo_lookup(alpha.id).unwrap(), None, "a failure is not recorded");
        let asked = server.received_requests().await.unwrap();
        let alpha_searches = asked
            .iter()
            .filter(|r| r.url.path() == "/search/artist" && r.url.query().unwrap_or("").contains("Alpha"))
            .count();
        assert_eq!(alpha_searches, 1);
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
