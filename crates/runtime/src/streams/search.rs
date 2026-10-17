//! The stream part of the unified search: every chosen service is asked at
//! once and its section is published as it answers (its first part first,
//! see [`StreamService::search_in_parts`]); a newer query aborts the older
//! requests and drops whatever they still bring. While a new query runs, a
//! section keeps the previous answer on screen (marked searching), so typing
//! never collapses the page. Answers are kept for [`CACHE_TTL`]: the same
//! query again (backspace, retyping) shows at once without a request. Rows
//! are merged against the library (a library track wins) and across
//! services (the first service, Spotify, keeps the row).
//!
//! [`StreamService::search_in_parts`]: nikgob_core::streaming::StreamService::search_in_parts

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{Service, StreamError, StreamSearch, StreamTrack};
use nikgob_metadata::text::{TitleKey, album_key, artist_key, similarity, title_key};
use nikgob_view as v;

use super::Msg;
use crate::Worker;

/// Wait before asking the services, so typing does not fire a request per
/// key (an older query's task is aborted while it waits). The search bar
/// already waits ~180 ms before it sends the query: ~350 ms in all.
const DEBOUNCE: Duration = Duration::from_millis(170);
/// How long a service's answer to a query is reused.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Answers kept (per service and query).
const CACHE_MAX: usize = 64;
/// How far two lengths may be apart for the same song.
const SLACK_MS: u64 = 3_000;
/// Artist names this similar (0..=1) are the same artist.
const ARTIST_SIMILARITY: f64 = 0.85;

#[derive(Default)]
pub(crate) struct SearchState {
    pub(crate) seq: u64,
    pub(crate) query: String,
    pub(crate) sources: v::SearchSources,
    /// Per asked service, in section order.
    raw: Vec<Slot>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// When the current query was sent to the services.
    started: Option<Instant>,
    cache: HashMap<(Service, String), (Instant, StreamSearch)>,
    /// Library hits of the query (they win over stream rows).
    lib_tracks: Vec<(nikgob_library::TrackRow, Option<String>)>,
    /// The library track of each stream row of this query (the same-song
    /// lookup is a few queries; sections are rebuilt on every part).
    linked: HashMap<nikgob_core::streaming::RemoteId, Option<TrackId>>,
}

/// A query as the cache knows it: lower case, single spaces.
fn cache_key(query: &str) -> String {
    query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

type Answer = Result<StreamSearch, StreamError>;

/// One service's part of the search.
struct Slot {
    service: Service,
    /// The latest answer: this query's (part or whole), or the previous
    /// query's while this one runs.
    result: Option<Result<StreamSearch, StreamError>>,
    /// The answer to this query is complete.
    done: bool,
}

/// What makes two tracks the same song: the ISRC, else artist + title
/// (folded like the importer folds them) + length within [`SLACK_MS`].
#[derive(Clone, Debug)]
pub(crate) struct SongKey {
    isrc: Option<String>,
    title: TitleKey,
    artist: String,
    duration_ms: Option<u64>,
}

fn upper_isrc(isrc: Option<&str>) -> Option<String> {
    let s: String = isrc?
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (!s.is_empty()).then_some(s)
}

/// The main artist of a credit line ("A, B", "A feat. B" → "a").
fn main_artist(line: &str) -> String {
    let first = line.split(", ").next().unwrap_or(line);
    artist_key(first)
}

impl SongKey {
    pub(crate) fn of_stream(t: &StreamTrack) -> Self {
        Self {
            isrc: upper_isrc(t.isrc.as_deref()),
            title: title_key(&t.title),
            artist: main_artist(
                t.artists
                    .first()
                    .map_or(t.artist_line().as_str(), |a| a.name.as_str()),
            ),
            duration_ms: t.duration_ms,
        }
    }

    pub(crate) fn of_library(row: &nikgob_library::TrackRow, isrc: Option<&str>) -> Self {
        Self {
            isrc: upper_isrc(isrc),
            title: title_key(&row.title),
            artist: main_artist(&row.artist),
            duration_ms: row.duration_ms.map(u64::from),
        }
    }

    pub(crate) fn same(&self, o: &Self) -> bool {
        if let (Some(a), Some(b)) = (&self.isrc, &o.isrc)
            && a == b
        {
            return true;
        }
        if self.title.base.is_empty() || self.title != o.title {
            return false;
        }
        let artist = self.artist == o.artist
            || (!self.artist.is_empty()
                && !o.artist.is_empty()
                && similarity(&self.artist, &o.artist) >= ARTIST_SIMILARITY);
        let length = match (self.duration_ms, o.duration_ms) {
            (Some(a), Some(b)) => a.abs_diff(b) <= SLACK_MS,
            _ => true,
        };
        artist && length
    }
}

impl SearchState {
    /// The library changed: rows are matched against it again.
    pub(crate) fn forget_links(&mut self) {
        self.linked.clear();
    }
}

impl Worker {
    /// The stream part of [`Command::Search`](nikgob_view::Command::Search):
    /// `library` are the library hits already published.
    pub(crate) fn search_streams(&mut self, query: &str, library: Vec<nikgob_library::TrackRow>) {
        let st = &mut self.streaming.search;
        st.seq += 1;
        let stale = st.tasks.iter().filter(|t| !t.is_finished()).count();
        if stale > 0 {
            log::debug!(
                "search #{}: cancelled {stale} request(s) of «{}»",
                st.seq - 1,
                st.query
            );
        }
        for t in st.tasks.drain(..) {
            t.abort();
        }
        st.query = query.to_owned();
        st.started = Some(Instant::now());
        st.linked.clear();
        st.lib_tracks = library
            .into_iter()
            .map(|r| {
                let isrc = self.lib.get_track(r.id).ok().flatten().and_then(|t| t.isrc);
                (r, isrc)
            })
            .collect();
        let seq = st.seq;
        let sources = st.sources;
        if query.is_empty() {
            self.streaming.search.raw.clear();
            self.publish(|s, _| s.search.streams.clear());
            return;
        }
        // Spotify answers nothing without a login: it is not asked (the UI
        // says «войди, чтобы искать и там» instead of an error).
        let asked: Vec<Service> = Service::ALL
            .iter()
            .copied()
            .filter(|&s| sources.service(s) && self.service(s).is_some_and(|svc| s != Service::Spotify || svc.account().is_logged_in()))
            .collect();
        // Sections that stay keep what they showed until the new answer.
        let mut old = std::mem::take(&mut self.streaming.search.raw);
        let st = &mut self.streaming.search;
        st.cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        for &service in &asked {
            let prev = old
                .iter()
                .position(|s| s.service == service)
                .and_then(|i| old.swap_remove(i).result);
            let key = (service, cache_key(query));
            if let Some((_, hit)) = st.cache.get(&key) {
                log::debug!("search #{seq} «{query}»: {service} from the cache");
                st.raw.push(Slot {
                    service,
                    result: Some(Ok(hit.clone())),
                    done: true,
                });
                continue;
            }
            st.raw.push(Slot {
                service,
                result: prev,
                done: false,
            });
        }
        for &service in &asked {
            if self.streaming.search.raw.iter().any(|s| s.service == service && s.done) {
                continue;
            }
            let Some(svc) = self.service(service) else { continue };
            let q = query.to_owned();
            let tx = self.streaming.internal.clone();
            let task = self.streaming.spawn(async move {
                tokio::time::sleep(DEBOUNCE).await;
                let started = Instant::now();
                log::debug!("search #{seq} «{q}»: {service} asked");
                let part: nikgob_core::streaming::SearchParts = Arc::new(move |part| {
                    log::debug!(
                        "search #{seq}: {service} first part in {} ms ({} tracks)",
                        started.elapsed().as_millis(),
                        part.tracks.len()
                    );
                    let _ = tx.send(crate::import::Internal::Stream(Msg::Searched {
                        seq,
                        service,
                        result: Ok(part),
                        partial: true,
                    }));
                });
                let result = svc.search_in_parts(&q, part).await;
                match &result {
                    Ok(r) => log::debug!(
                        "search #{seq}: {service} answered in {} ms: {} tracks, {} albums, {} artists, {} playlists",
                        started.elapsed().as_millis(),
                        r.tracks.len(),
                        r.albums.len(),
                        r.artists.len(),
                        r.playlists.len()
                    ),
                    Err(e) => log::info!(
                        "search #{seq}: {service} failed in {} ms: {e}",
                        started.elapsed().as_millis()
                    ),
                }
                Msg::Searched {
                    seq,
                    service,
                    result,
                    partial: false,
                }
            });
            self.streaming.search.tasks.push(task);
        }
        self.rebuild_sections();
    }

    pub(crate) fn set_search_sources(&mut self, sources: v::SearchSources) {
        self.streaming.search.sources = sources;
        let q = self.state.read().search.query.clone();
        self.publish(|s, _| s.search.sources = sources);
        if !q.trim().is_empty() {
            self.search(q);
        }
    }

    pub(crate) fn stream_searched(
        &mut self,
        seq: u64,
        service: Service,
        result: Result<StreamSearch, StreamError>,
        partial: bool,
    ) {
        if seq != self.streaming.search.seq {
            return; // an older query
        }
        if let Err(e) = &result {
            log::warn!("{service} search: {e}");
        }
        let st = &mut self.streaming.search;
        if !partial && let Ok(r) = &result {
            if st.cache.len() >= CACHE_MAX {
                st.cache.clear();
            }
            st.cache.insert((service, cache_key(&st.query)), (Instant::now(), r.clone()));
        }
        if let Some(slot) = st.raw.iter_mut().find(|s| s.service == service) {
            if slot.done {
                return; // a part after the whole answer
            }
            slot.result = Some(result.clone());
            slot.done = !partial;
        }
        let rebuilt = Instant::now();
        self.rebuild_sections();
        log::debug!(
            "search #{seq}: {service} {} shown {} ms after the query ({} ms to merge)",
            if partial { "part" } else { "answer" },
            self.streaming.search.started.map_or(0, |t| t.elapsed().as_millis()),
            rebuilt.elapsed().as_millis()
        );
        // Learnt after the section is out: library writes never delay it.
        if !partial && let Ok(r) = &result {
            self.streaming.learn(&r.tracks);
            for a in &r.albums {
                self.learn_album(a);
            }
        }
    }

    /// Recomputes the stream sections from the raw answers (merging rows).
    pub(crate) fn rebuild_sections(&mut self) {
        let started = Instant::now();
        let raw: Vec<(Service, Option<Answer>, bool)> = self
            .streaming
            .search
            .raw
            .iter()
            .map(|s| (s.service, s.result.clone(), s.done))
            .collect();
        let lib_keys: Vec<(TrackId, SongKey)> = self
            .streaming
            .search
            .lib_tracks
            .iter()
            .map(|(r, isrc)| (r.id, SongKey::of_library(r, isrc.as_deref())))
            .collect();
        let mut extra_lib: Vec<nikgob_library::TrackRow> = Vec::new();
        let mut kept: Vec<(SongKey, usize, usize)> = Vec::new();
        let mut kept_albums: Vec<(String, String)> = Vec::new();
        let mut kept_artists: Vec<String> = Vec::new();
        let mut sections: Vec<v::StreamSearchSection> = Vec::new();
        // Under a service's chip the library section is not shown: its
        // songs stay in the service's list (marked «у тебя»).
        let library_shown = self.streaming.search.sources.library;
        for (service, result, done) in raw {
            let mut section = v::StreamSearchSection::new(service);
            section.searching = !done;
            match result {
                None => {}
                // An old error is not shown while the new query runs.
                Some(Err(_)) if !done => {}
                Some(Err(e)) => section.error = Some(search_error(service, &e)),
                Some(Ok(r)) => {
                    for t in &r.tracks {
                        let key = SongKey::of_stream(t);
                        if lib_keys.iter().any(|(_, k)| k.same(&key)) {
                            continue; // shown as the library hit
                        }
                        let linked = match self.streaming.search.linked.get(&t.id) {
                            Some(l) => *l,
                            None => {
                                let l = self.library_track_of(t);
                                self.streaming.search.linked.insert(t.id.clone(), l);
                                l
                            }
                        };
                        if library_shown
                            && let Some(id) = linked
                            && self.lib.track_in_library(id).ok().flatten() == Some(true)
                        {
                            if !lib_keys.iter().any(|(t, _)| *t == id)
                                && !extra_lib.iter().any(|r| r.id == id)
                                && let Some(row) = self.lib.get_track_row(id).ok().flatten()
                            {
                                extra_lib.push(row);
                            }
                            continue;
                        }
                        if let Some((_, si, ri)) = kept.iter().find(|(k, ..)| k.same(&key)) {
                            let row: &mut v::StreamTrackRow = if *si == sections.len() {
                                &mut section.tracks[*ri]
                            } else {
                                &mut sections[*si].tracks[*ri]
                            };
                            if !row.also_on.contains(&service) && row.remote.service != service {
                                row.also_on.push(service);
                            }
                            continue;
                        }
                        kept.push((key, sections.len(), section.tracks.len()));
                        section.tracks.push(self.stream_row(t, linked));
                    }
                    for a in &r.albums {
                        let key = (
                            album_key(&a.title),
                            main_artist(&nikgob_core::streaming::join_names(&a.artists)),
                        );
                        if kept_albums.contains(&key) {
                            continue;
                        }
                        kept_albums.push(key);
                        section.albums.push(self.album_card(a));
                    }
                    for a in &r.artists {
                        let key = artist_key(&a.name);
                        if kept_artists.contains(&key) {
                            continue;
                        }
                        kept_artists.push(key);
                        section.artists.push(self.artist_card(a));
                    }
                    for p in &r.playlists {
                        section.playlists.push(self.playlist_card(p));
                    }
                }
            }
            sections.push(section);
        }
        let query = self.streaming.search.query.clone();
        let merged = started.elapsed();
        if merged > Duration::from_millis(20) {
            log::debug!("search: merging the sections took {} ms", merged.as_millis());
        }
        self.publish(|s, res| {
            if s.search.query.trim() != query {
                return;
            }
            for row in &extra_lib {
                if !s.search.tracks.iter().any(|t| t.id == row.id) {
                    s.search.tracks.push(res.track(row));
                }
            }
            s.search.streams = sections;
        });
    }

    /// The library track a stream track is: linked by its remote id, else
    /// the one with its ISRC.
    pub(crate) fn library_track_of(&self, t: &StreamTrack) -> Option<TrackId> {
        self.lib.track_for_stream(t).ok().flatten()
    }
}

/// Why a service's section is empty, as the user reads it.
fn search_error(service: Service, e: &StreamError) -> String {
    match e {
        StreamError::RateLimited { .. } => {
            format!("{} просит подождать — повтори через минуту", service.display_name())
        }
        StreamError::Network(_) => format!("Нет связи с {}", service.display_name()),
        other => super::error_text(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{ArtistRef, RemoteId};

    fn track(service: Service, title: &str, artist: &str, secs: u64) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(service, title), title);
        t.artists = vec![ArtistRef::new(None, artist)];
        t.duration_ms = Some(secs * 1000);
        t
    }

    #[test]
    fn same_song_by_isrc_or_by_name_and_length() {
        let a = track(Service::Spotify, "Группа крови", "Кино", 285);
        let mut b = track(Service::Youtube, "Gruppa Krovi (Remastered 2019)", "Kino", 287);
        assert!(SongKey::of_stream(&a).same(&SongKey::of_stream(&b)));
        b.duration_ms = Some(295_000);
        assert!(!SongKey::of_stream(&a).same(&SongKey::of_stream(&b)), "10 s apart");
        b.isrc = Some("zz-fak-88-00001".into());
        let mut a2 = a.clone();
        a2.isrc = Some("ZZFAK8800001".into());
        assert!(SongKey::of_stream(&a2).same(&SongKey::of_stream(&b)), "same ISRC");
        let live = track(Service::Youtube, "Группа крови (Live)", "Кино", 285);
        assert!(!SongKey::of_stream(&a).same(&SongKey::of_stream(&live)));
        let other = track(Service::Youtube, "Группа крови", "Ария", 285);
        assert!(!SongKey::of_stream(&a).same(&SongKey::of_stream(&other)));
    }
}
