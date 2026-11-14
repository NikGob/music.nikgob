//! A library album knows its whole tracklist: an
//! album that came from one file (or one liked song) used to look complete,
//! and its page offered nothing to download.
//!
//! * Incomplete: no track count, or fewer tracks than the count.
//! * Asked when its page opens, and in the background one album at a time
//!   ([`EVERY`]) for albums with a file; each album at most once per
//!   [`MISS_TTL_MS`] when nothing was found (an hour after an error), never
//!   again once found (the library cache).
//! * An album without a year (one of streams only too) is asked the same
//!   way for it; a whole one only for its year: the same title or one of
//!   its songs' ISRCs, never another edition. A found album without its
//!   day (YouTube Music gives the year only) asks the Deezer catalogue for
//!   the day.
//! * The album in a service: the id linked to the release first; else a
//!   search for «artist title» on YouTube Music, then the Deezer catalogue,
//!   taking an album of the same title (folded, or its Cyrillic for a
//!   transliterated one) by an artist of the same name, else one whose
//!   title extends the other («Discovery» and «Discovery Bonus Mix»), else
//!   (a title in another script: «夏子» is «Natsuko») the one of that
//!   artist the service puts one of the album's ISRCs on, or, when its
//!   songs have no ISRC, the one of the same track count and year. An
//!   album of several artists («JVKE, Bryant Barnes») is any of theirs.
//! * Its tracks the release lacks become stream tracks of it, outside the
//!   library ([`nikgob_library::Library::adopt_stream_album`]): the page
//!   shows them muted with ▶ and ↓, the cards say «1 из 15». Found is
//!   kept only once every place the album counts has a line; an album
//!   still short of its count is asked again like a miss.
//! * The albums the background pass asks are looked for off the player's
//!   loop (every library album is read for it), at most once per
//!   [`RESCAN`]; the loop only takes the next one of them.
//! * Once per library (`tracklist_retry.v1`): the misses kept before the
//!   matching above are forgotten, so those albums are asked again (one
//!   per [`EVERY`]) instead of a week later.
//! * Once per library (`wished_album_places_fixed.v1`, see
//!   [`nikgob_library::Library::place_album_tracks`]): a «Хочу скачать»
//!   album whose files came from several sources and brought their own
//!   numbers is put back in the order of the album it is linked to in a
//!   service, one album per [`EVERY`] before the pass; the files follow
//!   (tags and names). The library is backed up before the first change;
//!   an album whose tracklist could not be had leaves the mark unset, so
//!   the next start asks again.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::{ReleaseId, TrackId, UnixMs};
use nikgob_core::streaming::{EntityKind, SearchKind, Service, StreamAlbum, StreamError, StreamService};
use nikgob_core::translit::same_translit;
use nikgob_metadata::text::{album_key, artist_key};

use super::Msg;
use crate::Worker;

/// A miss is asked again after this…
const MISS_TTL_MS: UnixMs = 7 * 24 * 3600 * 1000;
/// …a failure (offline, rate limit) after this.
const ERROR_TTL_MS: UnixMs = 3600 * 1000;
/// One album of the background pass per this.
pub(crate) const EVERY: Duration = Duration::from_secs(60);
/// Albums of a search looked at.
const CANDIDATES: usize = 5;
/// The albums the background pass asks are looked for at most this often.
const RESCAN: Duration = Duration::from_secs(10 * 60);
/// Albums of that list taken in one step of the pass, at most (each a few
/// reads; one that asks a service ends the step).
const STEP_TAKES: usize = 8;
/// Library cache key: the misses of the older album matching were forgotten.
const MISSES_RETRIED: &str = "tracklist_retry.v1";

pub(crate) enum TracklistMsg {
    /// The album of a library album in a service (or none).
    Album {
        release: ReleaseId,
        result: Result<Option<StreamAlbum>, StreamError>,
    },
    /// The albums the background pass asks next, found off the loop.
    Candidates(Vec<ReleaseId>),
}

#[derive(Default)]
pub(crate) struct TracklistState {
    asking: HashSet<ReleaseId>,
    last: Option<Instant>,
    /// The one-time repair of wished albums' places: the albums left to
    /// put in order (`None` before it started), the one asked now, and
    /// whether one of them could not be done.
    places: Option<Vec<ReleaseId>>,
    placing: Option<ReleaseId>,
    places_missed: bool,
    /// The albums the background pass asks next (the last first).
    queue: Vec<ReleaseId>,
    /// They are being looked for, off the loop; and when that last began.
    scanning: bool,
    scanned: Option<Instant>,
}

fn cache_key(release: ReleaseId) -> String {
    format!("tracklist.{release}")
}

/// Where the day a service says a library album came out is kept (the
/// library stores only its year).
fn released_key(release: ReleaseId) -> String {
    format!("released.{release}")
}

/// The same album title: folded alike, or one the transliteration of the
/// other.
pub(crate) fn same_album(a: &str, b: &str) -> bool {
    let (x, y) = (album_key(a), album_key(b));
    (!x.is_empty() && x == y) || same_translit(a, b)
}

/// One title the other with more words after it, folded.
fn extends_album(a: &str, b: &str) -> bool {
    let (x, y) = (album_key(a), album_key(b));
    let starts = |long: &str, short: &str| !short.is_empty() && long.strip_prefix(short).is_some_and(|r| r.starts_with(' '));
    starts(&x, &y) || starts(&y, &x)
}

fn same_artist(a: &str, b: &str) -> bool {
    let (x, y) = (artist_key(a), artist_key(b));
    (!x.is_empty() && x == y) || same_translit(a, b)
}

/// What a library album is looked up by in a service.
#[derive(Clone, Debug, Default)]
struct Wanted {
    artist: String,
    /// The names of the album's artists, when it has several.
    names: Vec<String>,
    title: String,
    /// The ISRCs of its tracks.
    isrcs: Vec<String>,
    /// Its track count and year, when known.
    count: Option<u32>,
    year: Option<i32>,
    /// Its tracklist is whole, only its year is asked: no other edition
    /// will do (one with more words, one by count).
    year_only: bool,
}

impl Wanted {
    /// `name` is the album's artist, or one of its artists.
    fn by(&self, name: &str) -> bool {
        same_artist(name, &self.artist) || self.names.iter().any(|n| same_artist(name, n))
    }
}

/// The names of a credit's artists, also of one name that lists several
/// («リコ、レグ、ナナチ»; «& Emilio Piano» of a credit split at its commas
/// is «Emilio Piano»); none for a credit of one artist.
fn credit_names(credit: &nikgob_core::model::ArtistCredit) -> Vec<String> {
    let names: Vec<String> = credit
        .0
        .iter()
        .flat_map(|p| p.name.split([',', '、', '&', ';', '/']))
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect();
    if names.len() < 2 { Vec::new() } else { names }
}

/// ISRCs of an album looked up in a service, at most.
const ISRC_ASKS: usize = 2;

/// The album in one service: by its id, else by a search.
async fn find(svc: Arc<dyn StreamService>, linked: Option<String>, w: &Wanted) -> Result<Option<StreamAlbum>, StreamError> {
    if let Some(id) = linked {
        return svc.album(&id).await.map(Some);
    }
    let hits = svc.search_kind(&format!("{} {}", w.artist, w.title), SearchKind::Albums).await?.albums;
    let by: Vec<_> = hits
        .into_iter()
        .take(CANDIDATES)
        .filter(|a| a.artists.iter().any(|x| w.by(&x.name)))
        .collect();
    // The same title first; an edition with more words only without one.
    let mut hit = by.iter().find(|a| same_album(&a.title, &w.title)).map(|a| a.id.id.clone());
    if hit.is_none() && !w.year_only {
        hit = by.iter().find(|a| extends_album(&a.title, &w.title)).map(|a| a.id.id.clone());
    }
    // No title alike (one in another script: «夏子» is «Natsuko»): the
    // album one of its songs is on, by the song's ISRC; without ISRCs one
    // of the same count and year.
    if hit.is_none() && !by.is_empty() {
        hit = album_of_isrcs(svc.as_ref(), &by, &w.isrcs).await?;
        if hit.is_none() && w.isrcs.is_empty() && !w.year_only {
            hit = by.iter().find(|a| same_count_and_year(a, w)).map(|a| a.id.id.clone());
        }
    }
    match hit {
        Some(id) => svc.album(&id).await.map(Some),
        None => Ok(None),
    }
}

/// The candidate of `by` the service puts a song of `isrcs` on (each ISRC
/// asked once, at most [`ISRC_ASKS`]). A rate limit stops the lookup.
async fn album_of_isrcs(svc: &dyn StreamService, by: &[StreamAlbum], isrcs: &[String]) -> Result<Option<String>, StreamError> {
    for isrc in isrcs.iter().take(ISRC_ASKS) {
        match svc.find_isrc(isrc).await {
            Ok(Some(t)) => {
                let on = t.album.and_then(|a| a.id);
                if let Some(a) = by.iter().find(|a| on.as_ref() == Some(&a.id)) {
                    return Ok(Some(a.id.id.clone()));
                }
            }
            Ok(None) => {}
            Err(e @ StreamError::RateLimited { .. }) => return Err(e),
            Err(e) => log::debug!("tracklist: ISRC {isrc}: {e}"),
        }
    }
    Ok(None)
}

/// The day `a` came out (and its year, when it has none) from the same
/// album in `svc` (the Deezer catalogue): the same title or one of its
/// songs, by the same artist, of the same year when both tell one.
async fn date_from(svc: Arc<dyn StreamService>, a: &mut StreamAlbum, w: &Wanted) {
    let want = Wanted { year_only: true, ..w.clone() };
    match find(svc, None, &want).await {
        Ok(Some(d)) => {
            let agree = match (a.year, d.year) {
                (Some(x), Some(y)) => x == y,
                _ => true,
            };
            if agree && d.release_date.is_some() {
                a.release_date = d.release_date;
                a.year = a.year.or(d.year);
            }
        }
        Ok(None) => {}
        Err(e) => log::debug!("tracklist: the day of «{}»: {e}", w.title),
    }
}

/// A candidate with the album's track count and year, both known.
fn same_count_and_year(a: &StreamAlbum, w: &Wanted) -> bool {
    w.count.is_some() && a.total_tracks == w.count && w.year.is_some() && a.year == w.year
}

/// What a found tracklist leaves in the cache: found once every place the
/// album counts has a line and the album has its year, else a miss, asked
/// again later.
fn found_mark(lib: &nikgob_library::Library, release: ReleaseId) -> &'static str {
    let yearless = lib.get_release(release).ok().flatten().is_some_and(|r| r.year.is_none());
    match lib.release_listing(release) {
        Ok(Some(l)) if l.holes() > 0 => "miss",
        _ if yearless => "miss",
        _ => "found",
    }
}

impl Worker {
    /// Every place the library album counts has a track.
    fn release_whole(&self, release: ReleaseId) -> bool {
        let Some(r) = self.lib.get_release(release).ok().flatten() else { return false };
        let known = self.lib.tracks_of_release_all(release).map(|t| t.len()).unwrap_or(0) as u32;
        r.total_tracks.is_some_and(|t| known >= t)
    }

    /// Keeps the day the service's `album` came out for the library album
    /// `release` (its page shows it when no file of it tells the day).
    pub(crate) fn keep_released(&self, release: ReleaseId, album: &StreamAlbum) {
        let Some(day) = album.release_date.as_deref().map(str::trim) else { return };
        if nikgob_view::ReleaseDay::parse(day, None).is_none() {
            return;
        }
        let key = released_key(release);
        if self.lib.cache_get(&key).ok().flatten().is_some_and(|(_, kept)| kept == day) {
            return;
        }
        let _ = self.lib.cache_put(&key, nikgob_library::now_ms(), day);
    }

    /// The day a service said the library album `release` came out, as kept
    /// by [`Self::keep_released`].
    pub(crate) fn kept_released(&self, release: ReleaseId) -> Option<String> {
        self.lib.cache_get(&released_key(release)).ok().flatten().map(|(_, day)| day)
    }

    /// Asks the services for the whole tracklist of a library album that
    /// lacks it (see the module docs).
    pub(crate) fn complete_tracklist(&mut self, release: ReleaseId) {
        let now = nikgob_library::now_ms();
        if let Some((at, what)) = self.lib.cache_get(&cache_key(release)).ok().flatten()
            && answer_fresh(at, &what, now)
        {
            return;
        }
        let Some(r) = self.lib.get_release(release).ok().flatten() else { return };
        let rows = self.lib.tracks_of_release_all(release).unwrap_or_default();
        let known = rows.len() as u32;
        // A whole album is asked only for the year it lacks.
        let whole = r.total_tracks.is_some_and(|t| known >= t);
        if known == 0 || (whole && r.year.is_some()) {
            return;
        }
        let linked = self.lib.external_ids(EntityKind::Album, *release.as_uuid()).unwrap_or_default();
        let mut asks = Vec::new();
        for kind in [Service::Youtube, Service::Deezer, Service::Spotify] {
            let Some(svc) = self.service(kind) else { continue };
            let id = linked.iter().find(|l| l.service == kind).map(|l| l.id.clone());
            if id.is_some() || kind != Service::Spotify {
                asks.push((svc, id));
            }
        }
        // A linked album first.
        asks.sort_by_key(|(_, id)| id.is_none());
        let mut isrcs: Vec<String> = rows
            .iter()
            .filter_map(|t| self.lib.get_track(t.id).ok().flatten().and_then(|t| t.isrc))
            .collect();
        isrcs.dedup();
        let w = Wanted {
            artist: r.artist_credit.display(),
            names: credit_names(&r.artist_credit),
            title: r.title.clone(),
            isrcs,
            count: r.total_tracks,
            year: r.year,
            year_only: whole,
        };
        // A found album without its day (YouTube Music gives the year
        // only): the Deezer catalogue is asked for it.
        let dates = if r.year.is_none() { self.service(Service::Deezer) } else { None };
        self.ask_tracklist(release, w, asks, dates);
    }

    /// Asks the services `asks` (each with the album's id there, if linked)
    /// for the tracklist of the album `w`, in order; a found album without
    /// its day asks `dates` for it. The answer comes as a [`TracklistMsg`].
    fn ask_tracklist(
        &mut self,
        release: ReleaseId,
        w: Wanted,
        asks: Vec<(Arc<dyn StreamService>, Option<String>)>,
        dates: Option<Arc<dyn StreamService>>,
    ) {
        if asks.is_empty() || !self.streaming.tracklist.asking.insert(release) {
            return;
        }
        self.streaming.spawn(async move {
            let mut last = Ok(None);
            for (svc, id) in asks {
                match find(svc, id, &w).await {
                    Ok(Some(a)) if !a.tracks.is_empty() => {
                        last = Ok(Some(a));
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => last = Err(e),
                }
            }
            if let Ok(Some(a)) = &mut last
                && a.release_date.is_none()
                && let Some(dz) = dates.filter(|d| d.service() != a.id.service)
            {
                date_from(dz, a, &w).await;
            }
            Msg::Tracklist(TracklistMsg::Album { release, result: last })
        });
    }

    pub(crate) fn tracklist_msg(&mut self, m: TracklistMsg) {
        let (release, result) = match m {
            TracklistMsg::Album { release, result } => (release, result),
            TracklistMsg::Candidates(found) => {
                let t = &mut self.streaming.tracklist;
                t.scanning = false;
                t.queue = found;
                return;
            }
        };
        let m = TracklistAnswer { release, result };
        self.streaming.tracklist.asking.remove(&m.release);
        if self.streaming.tracklist.placing == Some(m.release) {
            self.streaming.tracklist.placing = None;
            self.places_found(m.release, m.result);
            return;
        }
        let now = nikgob_library::now_ms();
        let key = cache_key(m.release);
        match m.result {
            // A whole album asked for its year: only the year is taken.
            Ok(Some(album)) if self.release_whole(m.release) => {
                match self.lib.fill_release_year(m.release, album.year) {
                    Ok(true) => log::info!("album {}: its year from {}", m.release, album.id),
                    Ok(false) => {}
                    Err(e) => log::warn!("album {}: year not stored: {e}", m.release),
                }
                // Still without a year: asked again later.
                let _ = self.lib.cache_put(&key, now, found_mark(&self.lib, m.release));
                self.keep_released(m.release, &album);
                self.load_library();
                if self.state.read().release.as_ref().is_some_and(|r| r.card.id == m.release) {
                    self.load_release(m.release);
                }
            }
            Ok(Some(album)) => {
                match self.lib.adopt_stream_album(m.release, &album) {
                    Ok(n) => log::info!("album {}: {n} more tracks from {}", m.release, album.id),
                    Err(e) => log::warn!("album {}: tracklist not stored: {e}", m.release),
                }
                let _ = self.lib.cache_put(&key, now, found_mark(&self.lib, m.release));
                self.keep_released(m.release, &album);
                self.extend_album_queue(m.release);
                self.load_library();
                if self.state.read().release.as_ref().is_some_and(|r| r.card.id == m.release) {
                    self.load_release(m.release);
                }
            }
            Ok(None) => {
                let _ = self.lib.cache_put(&key, now, "miss");
            }
            Err(e) => {
                log::debug!("album {}: tracklist: {e}", m.release);
                let _ = self.lib.cache_put(&key, now, "error");
            }
        }
    }

    /// The queue still plays `release` (replaced by nothing since): its
    /// tracks the queue lacks join it in album order, and the first of
    /// them plays when nothing could. They only stream: each is looked up
    /// when it comes up, as playing the album does. A queue of the user's
    /// songs of the album takes only a new one of them (an album added
    /// whole takes in its new songs): a song of the tracklist the user
    /// never chose does not join it.
    pub(crate) fn extend_album_queue(&mut self, release: ReleaseId) {
        let Some((r, generation, whole)) = self.album_queue else { return };
        if r != release || generation != self.queue.generation() {
            return;
        }
        let have: HashSet<_> = self.queue.tracks().into_iter().collect();
        let Some(listing) = self.lib.release_listing(release).ok().flatten() else { return };
        let rows = listing.joins(self.album_rules(), whole);
        let more: Vec<_> = rows.into_iter().filter(|t| !have.contains(t)).collect();
        if more.is_empty() {
            return;
        }
        log::info!("album {release}: {} more tracks join the queue", more.len());
        let idle = self.playing.is_none();
        // Under shuffle they join the shuffled rest, not its end.
        self.queue.enqueue_shuffled(&more);
        if idle {
            if let Some(e) = self.queue.skip_forward().filter(|e| e.track == more[0]).or_else(|| self.queue.current()) {
                self.start(e);
            }
        }
        self.publish_player();
    }

    /// The background pass: one incomplete album with a file per [`EVERY`],
    /// the first one [`EVERY`] after the start (the start has enough to ask).
    /// The albums to ask are looked for off the loop ([`pass_candidates`]).
    pub(crate) fn tracklist_tick(&mut self) {
        let t = &mut self.streaming.tracklist;
        let last = *t.last.get_or_insert_with(Instant::now);
        if !t.asking.is_empty() || last.elapsed() < EVERY {
            return;
        }
        t.last = Some(Instant::now());
        if self.places_tick() {
            return;
        }
        for _ in 0..STEP_TAKES {
            let Some(release) = self.streaming.tracklist.queue.pop() else { break };
            self.complete_tracklist(release);
            if !self.streaming.tracklist.asking.is_empty() {
                return;
            }
        }
        let t = &mut self.streaming.tracklist;
        if !t.queue.is_empty() || t.scanning || t.scanned.is_some_and(|at| at.elapsed() < RESCAN) {
            return;
        }
        t.scanning = true;
        t.scanned = Some(Instant::now());
        let lib = self.lib.clone();
        self.streaming.spawn(async move {
            let found = tokio::task::spawn_blocking(move || pass_candidates(&lib)).await.unwrap_or_default();
            Msg::Tracklist(TracklistMsg::Candidates(found))
        });
    }

    /// Once per library: the tracklist misses kept before the album
    /// matching learnt other scripts, ISRCs and several artists are
    /// forgotten, so those albums are asked again now (see the module docs).
    pub(crate) fn retry_tracklist_misses_once(&mut self) {
        if self.lib.cache_get(MISSES_RETRIED).ok().flatten().is_some() {
            return;
        }
        match self.lib.cache_forget("tracklist.", "miss") {
            Ok(0) => {}
            Ok(n) => log::info!("tracklist: {n} albums not found before are asked again"),
            Err(e) => {
                log::warn!("forgetting the tracklist misses: {e}");
                return;
            }
        }
        let _ = self.lib.cache_put(MISSES_RETRIED, nikgob_library::now_ms(), "");
    }
}

/// The answer for one album (see [`TracklistMsg::Album`]).
struct TracklistAnswer {
    release: ReleaseId,
    result: Result<Option<StreamAlbum>, StreamError>,
}

/// Whether a cached answer of a tracklist lookup made at `at` still holds
/// at `now`.
fn answer_fresh(at: UnixMs, what: &str, now: UnixMs) -> bool {
    match what {
        "found" => true,
        "error" => now - at < ERROR_TTL_MS,
        _ => now - at < MISS_TTL_MS,
    }
}

/// The library albums the background pass asks, the first last (it pops
/// them): short of their count with a file, or without a year (one of
/// streams only too: its page shows no date), and not asked lately. Read
/// off the player's loop: it reads every album of the library.
fn pass_candidates(lib: &nikgob_library::Library) -> Vec<ReleaseId> {
    let now = nikgob_library::now_ms();
    let releases = lib.releases(0, 100_000, nikgob_library::ReleaseSort::asc(Default::default())).unwrap_or_default();
    let mut out: Vec<ReleaseId> = releases
        .into_iter()
        .filter(|r| {
            let short = r.local_tracks > 0 && !r.release.total_tracks.is_some_and(|t| r.known_tracks >= t);
            short || r.release.year.is_none()
        })
        .filter(|r| match lib.cache_get(&cache_key(r.release.id)).ok().flatten() {
            Some((at, what)) => !answer_fresh(at, &what, now),
            None => true,
        })
        .map(|r| r.release.id)
        .collect();
    out.reverse();
    out
}

impl Worker {
    /// The next step of the one-time repair of wished albums' places (see
    /// the module docs). Returns whether it asked a service now.
    fn places_tick(&mut self) -> bool {
        let key = nikgob_library::PLACES_FROM_WISH_KEY;
        if self.streaming.tracklist.placing.is_some() || self.lib.meta_value(key).ok().flatten().is_some() {
            return false;
        }
        if self.streaming.tracklist.places.is_none() {
            let found = self.lib.wished_albums_out_of_place().unwrap_or_default();
            if !found.is_empty() {
                log::info!("tracklist: {} wished albums are out of order", found.len());
            }
            self.streaming.tracklist.places = Some(found.into_iter().rev().collect());
        }
        loop {
            let Some(release) = self.streaming.tracklist.places.as_mut().and_then(Vec::pop) else {
                // All done (or none to do): once, unless one could not be had.
                if !self.streaming.tracklist.places_missed {
                    let _ = self.lib.set_meta_value(key, &nikgob_library::now_ms().to_string());
                }
                return false;
            };
            let Some(r) = self.lib.get_release(release).ok().flatten() else { continue };
            // Only the album the wish is linked to says its order: a search
            // could find another edition.
            let linked = self.lib.external_ids(EntityKind::Album, *release.as_uuid()).unwrap_or_default();
            let asks: Vec<(Arc<dyn StreamService>, Option<String>)> = [Service::Deezer, Service::Youtube, Service::Spotify]
                .into_iter()
                .filter_map(|kind| {
                    let id = linked.iter().find(|l| l.service == kind)?.id.clone();
                    Some((self.service(kind)?, Some(id)))
                })
                .collect();
            if asks.is_empty() {
                log::info!("tracklist: «{}» is linked to no service on, its places stay", r.title);
                if !linked.is_empty() {
                    // Its services are off now: asked again next start.
                    self.streaming.tracklist.places_missed = true;
                }
                continue;
            }
            self.streaming.tracklist.placing = Some(release);
            let w = Wanted {
                artist: r.artist_credit.display(),
                names: credit_names(&r.artist_credit),
                title: r.title.clone(),
                ..Wanted::default()
            };
            self.ask_tracklist(release, w, asks, None);
            if self.streaming.tracklist.asking.contains(&release) {
                return true;
            }
            self.streaming.tracklist.placing = None;
        }
    }

    /// The tracklist of a wished album out of order came: its tracks take
    /// their places, their files follow.
    fn places_found(&mut self, release: ReleaseId, result: Result<Option<StreamAlbum>, StreamError>) {
        let album = match result {
            Ok(Some(a)) => a,
            Ok(None) => {
                log::info!("tracklist: places of {release}: its album has no tracks there now");
                self.streaming.tracklist.places_missed = true;
                return;
            }
            Err(e) => {
                log::info!("tracklist: places of {release}: {e}");
                self.streaming.tracklist.places_missed = true;
                return;
            }
        };
        if let Err(e) = self.lib.backup_before(nikgob_library::PLACES_FROM_WISH_KEY) {
            log::warn!("tracklist: no backup before putting albums in order: {e}");
            self.streaming.tracklist.places_missed = true;
            return;
        }
        let moved = match self.lib.place_album_tracks(release, &album) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("tracklist: places of {release}: {e}");
                self.streaming.tracklist.places_missed = true;
                return;
            }
        };
        log::info!("album {release}: {} tracks put at their places on {}", moved.len(), album.id);
        // Every file of the album then says its place, the count and the
        // album (one of another edition kept its own), named so.
        let files: Vec<TrackId> = if moved.is_empty() {
            Vec::new()
        } else {
            self.lib.tracks_of_release(release).unwrap_or_default().into_iter().filter(|t| t.has_local).map(|t| t.id).collect()
        };
        if let Some(svc) = &self.import
            && !files.is_empty()
        {
            let importer = svc.importer().clone();
            let tx = svc.internal();
            svc.spawn_blocking(move || {
                for t in files {
                    let r = importer.place_track_file(t).map(|_| ()).map_err(|e| e.to_string());
                    let _ = tx.send(crate::import::Internal::Placed(t, r));
                }
            });
        }
        self.load_library();
        if self.state.read().release.as_ref().is_some_and(|r| r.card.id == release) {
            self.load_release(release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn album_titles_match_folded_or_transliterated() {
        assert!(same_album("LEGENDARNAJa PYL'", "ЛЕГЕНДАРНАЯ ПЫЛЬ"));
        assert!(same_album("Группа крови (Remastered)", "группа крови"));
        assert!(!same_album("Группа крови", "Звезда по имени Солнце"));
        assert!(same_artist("MORGENSHTERN", "Моргенштерн"));
        assert!(extends_album("Discovery Bonus Mix", "Discovery"));
        assert!(!extends_album("Discoveryland", "Discovery"));
        assert!(!extends_album("Mercury – Act 1", "Mercury : Acts 1 & 2"));
    }

    /// A self-titled album of «Big Band» whose service album is called
    /// otherwise («夏子» and «Natsuko»): its search finds the album, the
    /// titles do not match.
    fn natsuko() -> (Arc<dyn StreamService>, Wanted) {
        let svc: Arc<dyn StreamService> = Arc::new(nikgob_core::streaming::FakeService::deezer().with_album("natsuko", "Natsuko", 3));
        let w = Wanted {
            artist: "Big Band".into(),
            names: Vec::new(),
            title: "Big Band".into(),
            isrcs: vec!["ZZBIG0000001".into()],
            count: Some(3),
            year: Some(2020),
            year_only: false,
        };
        (svc, w)
    }

    #[tokio::test]
    async fn an_album_titled_otherwise_is_found_by_the_isrc_of_its_song() {
        let (svc, w) = natsuko();
        let a = find(svc.clone(), None, &w).await.unwrap().expect("found by ISRC");
        assert_eq!(a.title, "Natsuko");
        assert_eq!(a.tracks.len(), 3);
        // Asked for its year only: the ISRC is exact enough.
        let a = find(svc.clone(), None, &Wanted { year_only: true, ..w.clone() }).await.unwrap();
        assert!(a.is_some());
        // An ISRC of no song of it: no album, whatever the count says.
        let other = Wanted { isrcs: vec!["ZZNONE0000001".into()], ..w.clone() };
        assert!(find(svc.clone(), None, &other).await.unwrap().is_none());
        // Without ISRCs: the same count and year.
        let bare = Wanted { isrcs: Vec::new(), ..w.clone() };
        assert!(find(svc.clone(), None, &bare).await.unwrap().is_some());
        assert!(find(svc.clone(), None, &Wanted { count: Some(2), ..bare.clone() }).await.unwrap().is_none());
        assert!(find(svc.clone(), None, &Wanted { year: None, ..bare.clone() }).await.unwrap().is_none());
        assert!(find(svc, None, &Wanted { year_only: true, ..bare }).await.unwrap().is_none());
    }

    /// An album of several artists is any of theirs: «juliet» of «JVKE,
    /// Bryant Barnes, & Emilio Piano» is JVKE's album in a service; an
    /// album of one artist is that artist's in any case.
    #[test]
    fn an_album_of_several_artists_is_any_of_theirs() {
        use nikgob_core::model::{ArtistCredit, ArtistCreditPart};
        let part = |name: &str, join: &str| ArtistCreditPart { artist_id: None, name: name.into(), join_phrase: join.into() };
        let credit = ArtistCredit(vec![part("JVKE", ", "), part("Bryant Barnes", ", "), part("& Emilio Piano", "")]);
        let w = Wanted { artist: credit.display(), names: credit_names(&credit), ..Wanted::default() };
        assert!(w.by("JVKE") && w.by("Emilio Piano") && w.by("Bryant Barnes"));
        assert!(!w.by("Someone Else"));
        let listed = ArtistCredit::single("リコ(CV:富田美憂)、レグ(CV:伊瀬茉莉也)", None);
        assert_eq!(credit_names(&listed).len(), 2);
        let one = ArtistCredit::single("FUNKY MONKEY BΛBY'S", None);
        assert!(credit_names(&one).is_empty());
        let w = Wanted { artist: one.display(), names: credit_names(&one), ..Wanted::default() };
        assert!(w.by("Funky Monkey Bλby's"));
        assert!(!w.by("Funky Monkey"));
    }

    #[test]
    fn tracklist_found_is_not_cached_while_places_are_missing() {
        use nikgob_core::model::{ArtistCredit, Release, Track};
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let mut r = Release::new("Album", ArtistCredit::single("A", None));
        r.total_tracks = Some(3);
        r.year = Some(2001);
        let r = lib.upsert_release(&r).unwrap();
        for no in [1, 2] {
            let mut t = Track::new(format!("Song {no}"), ArtistCredit::single("A", None));
            t.release_id = Some(r.id);
            t.track_no = no;
            lib.upsert_track(&t).unwrap();
        }
        assert_eq!(found_mark(&lib, r.id), "miss");
        let mut t = Track::new("Song 3", ArtistCredit::single("A", None));
        t.release_id = Some(r.id);
        t.track_no = 3;
        lib.upsert_track(&t).unwrap();
        assert_eq!(found_mark(&lib, r.id), "found");
        // Whole, but the service gave no year: asked again later.
        let mut yearless = Release::new("Other", ArtistCredit::single("A", None));
        yearless.total_tracks = Some(1);
        let yearless = lib.upsert_release(&yearless).unwrap();
        let mut t = Track::new("Song", ArtistCredit::single("A", None));
        t.release_id = Some(yearless.id);
        t.track_no = 1;
        lib.upsert_track(&t).unwrap();
        assert_eq!(found_mark(&lib, yearless.id), "miss");
    }
}
