//! A library album knows its whole tracklist (docs/stage4.md §12.8): an
//! album that came from one file (or one liked song) used to look complete,
//! and its page offered nothing to download.
//!
//! * Incomplete: no track count, or fewer tracks than the count.
//! * Asked when its page opens, and in the background one album at a time
//!   ([`EVERY`]) for albums with a file; each album at most once per
//!   [`MISS_TTL_MS`] when nothing was found (an hour after an error), never
//!   again once found (the library cache).
//! * The album in a service: the id linked to the release first; else a
//!   search for «artist title» on YouTube Music, then the Deezer catalogue,
//!   taking an album of the same title (folded, or its Cyrillic for a
//!   transliterated one) by an artist of the same name, else one whose
//!   title extends the other («Discovery» and «Discovery Bonus Mix»).
//! * Its tracks the release lacks become stream tracks of it, outside the
//!   library ([`nikgob_library::Library::adopt_stream_album`]): the page
//!   shows them muted with ▶ and ↓, the cards say «1 из 15». Found is
//!   kept only once every place the album counts has a line; an album
//!   still short of its count is asked again like a miss.
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

pub(crate) struct TracklistMsg {
    release: ReleaseId,
    result: Result<Option<StreamAlbum>, StreamError>,
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
}

fn cache_key(release: ReleaseId) -> String {
    format!("tracklist.{release}")
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

/// The album in one service: by its id, else by a search.
async fn find(
    svc: Arc<dyn StreamService>,
    linked: Option<String>,
    artist: String,
    title: String,
) -> Result<Option<StreamAlbum>, StreamError> {
    if let Some(id) = linked {
        return svc.album(&id).await.map(Some);
    }
    let hits = svc.search_kind(&format!("{artist} {title}"), SearchKind::Albums).await?.albums;
    let by: Vec<_> = hits
        .into_iter()
        .take(CANDIDATES)
        .filter(|a| a.artists.iter().any(|x| same_artist(&x.name, &artist)))
        .collect();
    // The same title first; an edition with more words only without one.
    let hit = by.iter().find(|a| same_album(&a.title, &title)).or_else(|| by.iter().find(|a| extends_album(&a.title, &title)));
    match hit {
        Some(a) => svc.album(&a.id.id).await.map(Some),
        None => Ok(None),
    }
}

/// What a found tracklist leaves in the cache: found once every place the
/// album counts has a line, else a miss, asked again later.
fn found_mark(lib: &nikgob_library::Library, release: ReleaseId) -> &'static str {
    match lib.release_listing(release) {
        Ok(Some(l)) if l.holes() > 0 => "miss",
        _ => "found",
    }
}

impl Worker {
    /// Asks the services for the whole tracklist of a library album that
    /// lacks it (see the module docs).
    pub(crate) fn complete_tracklist(&mut self, release: ReleaseId) {
        let Some(r) = self.lib.get_release(release).ok().flatten() else { return };
        let known = self.lib.tracks_of_release_all(release).map(|t| t.len()).unwrap_or(0) as u32;
        if known == 0 || r.total_tracks.is_some_and(|t| known >= t) {
            return;
        }
        let now = nikgob_library::now_ms();
        if let Some((at, what)) = self.lib.cache_get(&cache_key(release)).ok().flatten() {
            let fresh = match what.as_str() {
                "found" => true,
                "error" => now - at < ERROR_TTL_MS,
                _ => now - at < MISS_TTL_MS,
            };
            if fresh {
                return;
            }
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
        self.ask_tracklist(release, &r, asks);
    }

    /// Asks the services `asks` (each with the album's id there, if linked)
    /// for the tracklist of `r`, in order; the answer comes as a
    /// [`TracklistMsg`].
    fn ask_tracklist(
        &mut self,
        release: ReleaseId,
        r: &nikgob_core::model::Release,
        asks: Vec<(Arc<dyn StreamService>, Option<String>)>,
    ) {
        if asks.is_empty() || !self.streaming.tracklist.asking.insert(release) {
            return;
        }
        let (artist, title) = (r.artist_credit.display(), r.title.clone());
        self.streaming.spawn(async move {
            let mut last = Ok(None);
            for (svc, id) in asks {
                match find(svc, id, artist.clone(), title.clone()).await {
                    Ok(Some(a)) if !a.tracks.is_empty() => {
                        last = Ok(Some(a));
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => last = Err(e),
                }
            }
            Msg::Tracklist(TracklistMsg { release, result: last })
        });
    }

    pub(crate) fn tracklist_msg(&mut self, m: TracklistMsg) {
        self.streaming.tracklist.asking.remove(&m.release);
        if self.streaming.tracklist.placing == Some(m.release) {
            self.streaming.tracklist.placing = None;
            self.places_found(m.release, m.result);
            return;
        }
        let now = nikgob_library::now_ms();
        let key = cache_key(m.release);
        match m.result {
            Ok(Some(album)) => {
                match self.lib.adopt_stream_album(m.release, &album) {
                    Ok(n) => log::info!("album {}: {n} more tracks from {}", m.release, album.id),
                    Err(e) => log::warn!("album {}: tracklist not stored: {e}", m.release),
                }
                let _ = self.lib.cache_put(&key, now, found_mark(&self.lib, m.release));
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
    /// when it comes up, as playing the album does.
    pub(crate) fn extend_album_queue(&mut self, release: ReleaseId) {
        if self.album_queue != Some((release, self.queue.generation())) {
            return;
        }
        let have: HashSet<_> = self.queue.tracks().into_iter().collect();
        let rows = self.lib.tracks_of_release_all(release).unwrap_or_default();
        let more: Vec<_> = rows.iter().map(|r| r.id).filter(|t| !have.contains(t)).collect();
        if more.is_empty() {
            return;
        }
        log::info!("album {release}: {} more tracks join the queue", more.len());
        let idle = self.playing.is_none();
        self.queue.enqueue(&more);
        if idle {
            if let Some(e) = self.queue.skip_forward().filter(|e| e.track == more[0]).or_else(|| self.queue.current()) {
                self.start(e);
            }
        }
        self.publish_player();
    }

    /// The background pass: one incomplete album with a file per [`EVERY`],
    /// the first one [`EVERY`] after the start (the start has enough to ask).
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
        let releases = self.lib.releases(0, 100_000, nikgob_library::ReleaseSort::asc(Default::default())).unwrap_or_default();
        for r in releases {
            if r.local_tracks == 0 || r.release.total_tracks.is_some_and(|t| r.known_tracks >= t) {
                continue;
            }
            self.complete_tracklist(r.release.id);
            if !self.streaming.tracklist.asking.is_empty() {
                break;
            }
        }
    }
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
            self.ask_tracklist(release, &r, asks);
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

    #[test]
    fn tracklist_found_is_not_cached_while_places_are_missing() {
        use nikgob_core::model::{ArtistCredit, Release, Track};
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let mut r = Release::new("Album", ArtistCredit::single("A", None));
        r.total_tracks = Some(3);
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
    }
}
