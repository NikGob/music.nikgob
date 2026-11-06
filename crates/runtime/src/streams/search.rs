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
//! A rate limit is not a failure: a service that answers 429 (or is not
//! asked because its `Retry-After` pause runs) keeps its section loading
//! with a quiet «отвечает с задержкой» and is asked again for the same
//! query when a pause of up to [`SHORT_WAIT`] ends (+ up to 0.5 s), at most
//! [`RETRIES`] times per query. A longer or unknown pause says the service
//! is overloaded while the others answer, and asks once more when it ends.
//!
//! The «Всё» page ([`v::SearchSources::mixed`]) asks every service that can
//! answer now (not Spotify without a login) its quick mixed search
//! ([`StreamService::search_top`]: YouTube Music one request, Deezer three)
//! and also gets one merged answer ([`v::SearchAnswer`], see
//! [`Worker::rebuild_answer`]): the library and the services' rows ranked
//! together ([`super::rank`]), each song once, played from the best source
//! it has. The previous answer stays until a service of the new query
//! answers; the top result and the first tracks are settled once Deezer
//! answered or [`SETTLE_GRACE`] after the first answer, and keep their
//! places while slower answers arrive below them.
//!
//! A query written with «е» is also asked of Deezer's tracks (its chip and
//! «Всё») under up to two «ё» spellings ([`rank::yo_variants`]: «лед» →
//! «лёд»), since the catalogue matches the letters as written; the lists
//! are taken in turn and the same-song merge and the ranking (which folds
//! «ё») do the rest. YouTube Music and Spotify get the query as typed.
//!
//! [`StreamService::search_in_parts`]: nikgob_core::streaming::StreamService::search_in_parts
//! [`StreamService::search_top`]: nikgob_core::streaming::StreamService::search_top

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::model::TrackId;
use nikgob_core::streaming::{RemoteId, SearchKind, Service, StreamError, StreamSearch, StreamTrack, TrackKind};
use nikgob_metadata::text::{TitleKey, album_key, artist_key, similarity, title_key};
use nikgob_view as v;

use super::{Msg, rank};
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
/// A rate-limit pause up to this long is waited out with the section
/// loading; a longer one says the service is overloaded.
pub(crate) const SHORT_WAIT: Duration = Duration::from_secs(60);
/// Automatic re-asks of a service per query after rate limits.
pub(crate) const RETRIES: u32 = 3;
/// When a service rate-limited without saying for how long, it is asked
/// again (once) this much later.
const UNKNOWN_WAIT: Duration = Duration::from_secs(60);
/// After the first service answered, how long the top of the «Всё» answer
/// waits for Deezer (fast, and the best catalogue) before it is settled.
pub(crate) const SETTLE_GRACE: Duration = Duration::from_millis(300);
/// The order services are merged in (and win ties): Spotify (when it is
/// asked at all), the Deezer catalogue, YouTube Music.
const MERGE_ORDER: [Service; 3] = [Service::Spotify, Service::Deezer, Service::Youtube];

/// What the settled top result is, to find it again in a later rebuild.
#[derive(Clone, Debug)]
enum TopKey {
    Artist(String),
    Track(SongKey),
    Album(String, String),
}

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
    /// The «Всё» answer's top is settled for this query (see the module
    /// docs) …
    settled: bool,
    /// … the grace timer runs …
    settling: bool,
    /// … and these keep their places: the top result and the first
    /// [`v::TOP_TRACKS`] tracks.
    frozen_top: Option<TopKey>,
    frozen_tracks: Vec<SongKey>,
}

impl SearchState {
    /// A search is being typed or answered: background discovery waits
    /// (the services' quotas go to the user first).
    pub(crate) fn active(&self) -> bool {
        !self.query.is_empty() && self.started.is_some_and(|t| t.elapsed() < Duration::from_secs(30))
    }
}

/// A query as the cache knows it: lower case, single spaces. The answers
/// of the quick «Всё» search and of the full one are kept apart.
fn cache_key(query: &str, mixed: bool) -> String {
    let q = query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    if mixed { format!("top:{q}") } else { q }
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
    /// Asked again after rate limits so far (at most [`RETRIES`]).
    retries: u32,
    /// The one re-ask after a long pause was used.
    long_retried: bool,
    /// The service rate-limited this query: what the section says.
    wait: Option<Wait>,
    /// `result` is this query's (not the previous one kept on screen).
    fresh: bool,
}

impl Slot {
    fn new(service: Service, result: Option<Answer>, done: bool) -> Self {
        Self {
            service,
            result,
            done,
            retries: 0,
            long_retried: false,
            wait: None,
            fresh: done,
        }
    }
}

/// Why a section waits after a rate limit.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Wait {
    /// A pause up to [`SHORT_WAIT`]: asked again when it ends.
    Short,
    /// Longer, or not told: the other services go on. `until` is the local
    /// time it ends, when known; `retry`: it is asked once more then.
    Long { until: Option<String>, retry: bool },
}

/// The quiet line of a waiting section (`other`: a service also asked).
fn wait_note(service: Service, wait: &Wait, other: Option<Service>) -> String {
    let name = service.display_name();
    match wait {
        Wait::Short => format!("{name} отвечает с задержкой — результаты появятся сами"),
        Wait::Long { until, retry } => {
            let until = until.as_ref().map(|hm| format!(" до {hm}")).unwrap_or_default();
            match other {
                Some(o) => format!("{name} сейчас перегружен{until} — ищу в {}", o.display_name()),
                None if *retry => format!("{name} сейчас перегружен{until} — поищу ещё раз сам"),
                None => format!("{name} сейчас перегружен{until}"),
            }
        }
    }
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
        st.settled = false;
        st.settling = false;
        st.frozen_top = None;
        st.frozen_tracks.clear();
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
            self.publish(|s| {
                s.search.streams.clear();
                s.search.answer = v::SearchAnswer::default();
            });
            return;
        }
        // A Spotify that is not connected is never asked (see
        // `Worker::service`); the UI says so under its chip.
        let mixed = sources.mixed;
        let asked: Vec<Service> = Service::ALL
            .iter()
            .copied()
            .filter(|&s| sources.service(s) && self.service(s).is_some())
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
            let key = (service, cache_key(query, mixed));
            if let Some((_, hit)) = st.cache.get(&key) {
                log::debug!("search #{seq} «{query}»: {service} from the cache");
                st.raw.push(Slot::new(service, Some(Ok(hit.clone())), true));
                continue;
            }
            st.raw.push(Slot::new(service, prev, false));
        }
        for &service in &asked {
            if self.streaming.search.raw.iter().any(|s| s.service == service && s.done) {
                continue;
            }
            self.ask_service(service, seq, query, DEBOUNCE);
        }
        // Nothing to wait for (everything cached, or no service asked).
        self.maybe_settle(None);
        self.rebuild_sections();
    }

    /// Settles the top of the «Всё» answer when it is time: every asked
    /// service is done, or Deezer answered; after the first answer
    /// (`answered`) of another service the grace timer starts.
    fn maybe_settle(&mut self, answered: Option<Service>) {
        let st = &mut self.streaming.search;
        if !st.sources.mixed || st.settled {
            return;
        }
        let all_done = st.raw.iter().all(|s| s.done);
        let deezer_done = st.raw.iter().any(|s| s.service == Service::Deezer && s.done);
        if all_done || deezer_done {
            st.settled = true;
            return;
        }
        if answered.is_some() && !st.settling {
            st.settling = true;
            let seq = st.seq;
            let task = self.streaming.spawn(async move {
                tokio::time::sleep(SETTLE_GRACE).await;
                Msg::SearchSettle { seq }
            });
            self.streaming.search.tasks.push(task);
        }
    }

    /// The grace after the first answer ended: the top is settled.
    pub(crate) fn search_settle(&mut self, seq: u64) {
        let st = &mut self.streaming.search;
        if seq != st.seq || st.settled {
            return;
        }
        st.settled = true;
        self.rebuild_sections();
    }

    /// Asks one service for `query` after `wait` (the debounce, or none for
    /// a re-ask after a rate limit); its parts and answer come back as
    /// [`Msg::Searched`].
    fn ask_service(&mut self, service: Service, seq: u64, query: &str, wait: Duration) {
        let Some(svc) = self.service(service) else { return };
        let mixed = self.streaming.search.sources.mixed;
        let q = query.to_owned();
        let yo = if service == Service::Deezer { rank::yo_variants(query) } else { Vec::new() };
        let tx = self.streaming.internal.clone();
        let task = self.streaming.spawn(async move {
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
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
            let main = async {
                if mixed {
                    svc.search_top(&q, part).await
                } else {
                    svc.search_in_parts(&q, part).await
                }
            };
            // Deezer matches «е» and «ё» as written: its tracks are also
            // asked under the «ё» spellings (one request each, cached by
            // Deezer under its own query; this answer is cached under the
            // query, whose variants they are).
            let variants = async {
                let mut out = Vec::new();
                for v in &yo {
                    match svc.search_kind(v, SearchKind::Songs).await {
                        Ok(r) => out.push(r.tracks),
                        Err(e) => log::debug!("search #{seq}: {service} «{v}» failed: {e}"),
                    }
                }
                out
            };
            let (mut result, more) = tokio::join!(main, variants);
            if let Ok(r) = &mut result {
                r.tracks = interleave(std::mem::take(&mut r.tracks), more);
            }
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

    /// The whole answer of `service` to the current query was a rate limit
    /// (see the module docs): its section waits, and a re-ask is set when
    /// that is still allowed.
    fn search_limited(&mut self, service: Service, retry_after: Option<Duration>) {
        let pause = retry_after.or_else(|| self.pause_left(service));
        let st = &mut self.streaming.search;
        let seq = st.seq;
        let Some(slot) = st.raw.iter_mut().find(|s| s.service == service) else { return };
        let again = if pause.is_some_and(|p| p <= SHORT_WAIT) && slot.retries < RETRIES {
            slot.retries += 1;
            slot.wait = Some(Wait::Short);
            slot.done = false;
            pause
        } else {
            let retry = slot.retries < RETRIES && !slot.long_retried;
            slot.wait = Some(Wait::Long {
                until: pause.filter(|p| *p > SHORT_WAIT).and_then(super::accounts::hm_after),
                retry,
            });
            slot.done = true;
            // An older query's rows would read as this one's.
            slot.result = None;
            slot.fresh = true;
            if retry {
                slot.retries += 1;
                slot.long_retried = true;
                Some(pause.unwrap_or(UNKNOWN_WAIT))
            } else {
                None
            }
        };
        let tries = slot.retries;
        match again {
            Some(after) => {
                let after = after + super::schedule::wake_jitter();
                log::info!(
                    "search #{seq}: {service} rate limited, asking again in {} ms ({tries} of {RETRIES})",
                    after.as_millis()
                );
                let task = self.streaming.spawn(async move {
                    tokio::time::sleep(after).await;
                    Msg::SearchRetry { seq, service }
                });
                self.streaming.search.tasks.push(task);
            }
            None => log::info!("search #{seq}: {service} rate limited, not asked again for this query"),
        }
    }

    /// A rate-limit pause of `service` ended: it is asked again, if the
    /// query is still the current one.
    pub(crate) fn search_retry(&mut self, seq: u64, service: Service) {
        let st = &mut self.streaming.search;
        if seq != st.seq {
            return; // a newer query
        }
        let Some(slot) = st.raw.iter_mut().find(|s| s.service == service && s.wait.is_some()) else {
            return;
        };
        slot.done = false;
        let query = st.query.clone();
        self.ask_service(service, seq, &query, Duration::ZERO);
        self.rebuild_sections();
    }

    pub(crate) fn set_search_sources(&mut self, sources: v::SearchSources) {
        self.streaming.search.sources = sources;
        let q = self.state.read().search.query.clone();
        self.publish(|s| s.search.sources = sources);
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
        if !partial && let Err(StreamError::RateLimited { retry_after }) = &result {
            self.search_limited(service, *retry_after);
            self.maybe_settle(Some(service));
            self.rebuild_sections();
            return;
        }
        if let Err(e) = &result {
            log::warn!("{service} search: {e}");
        }
        let st = &mut self.streaming.search;
        if !partial && let Ok(r) = &result {
            if st.cache.len() >= CACHE_MAX {
                st.cache.clear();
            }
            let key = (service, cache_key(&st.query, st.sources.mixed));
            st.cache.insert(key, (Instant::now(), r.clone()));
        }
        if let Some(slot) = st.raw.iter_mut().find(|s| s.service == service) {
            if slot.done {
                return; // a part after the whole answer
            }
            slot.result = Some(result.clone());
            slot.done = !partial;
            slot.wait = None;
            slot.fresh = true;
        }
        // A first part (tracks only) does not start the grace: the artists,
        // which may be the top result, are on their way.
        self.maybe_settle((!partial).then_some(service));
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
        let raw: Vec<(Service, Option<Answer>, bool, Option<Wait>)> = self
            .streaming
            .search
            .raw
            .iter()
            .map(|s| (s.service, s.result.clone(), s.done, s.wait.clone()))
            .collect();
        let asked: Vec<Service> = raw.iter().map(|r| r.0).collect();
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
        for (service, result, done, wait) in raw {
            let mut section = v::StreamSearchSection::new(service);
            section.searching = !done;
            let other = asked.iter().copied().find(|&s| s != service);
            section.note = wait.as_ref().map(|w| wait_note(service, w, other));
            match result {
                None => {}
                // An old error is not shown while the new query runs.
                Some(Err(_)) if !done => {}
                // Never a failure (see the module docs).
                Some(Err(StreamError::RateLimited { .. })) => {
                    let w = Wait::Long { until: None, retry: false };
                    section.note = Some(wait_note(service, &w, other));
                }
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
        let answer = self.rebuild_answer(&sections);
        let extra: Vec<v::TrackRow> = extra_lib.iter().map(|r| self.res.track(r)).collect();
        self.publish(|s| {
            if s.search.query.trim() != query {
                return;
            }
            for row in extra {
                if !s.search.tracks.iter().any(|t| t.id == row.id) {
                    s.search.tracks.push(row);
                }
            }
            s.search.streams = sections;
            match answer {
                Some(a) => s.search.answer = a,
                // The previous answer stays while the new query runs.
                None => s.search.answer.searching = true,
            }
        });
    }

    /// The merged «Всё» answer from the library hits, the services' fresh
    /// answers and their `sections` (cards already deduplicated across
    /// services); `None` while no service answered this query yet (the
    /// previous answer stays). See the module docs.
    fn rebuild_answer(&mut self, sections: &[v::StreamSearchSection]) -> Option<v::SearchAnswer> {
        let st = &self.streaming.search;
        if !st.sources.mixed {
            return Some(v::SearchAnswer::default());
        }
        let query = st.query.clone();
        if query.is_empty() {
            return Some(v::SearchAnswer::default());
        }
        let all_done = st.raw.iter().all(|s| s.done);
        if !all_done && !st.raw.iter().any(|s| s.fresh) {
            return None;
        }
        let q = rank::Query::new(&query);
        let fresh: Vec<Service> = st.raw.iter().filter(|s| s.fresh).map(|s| s.service).collect();
        let lib_keys: Vec<(TrackId, SongKey)> = st
            .lib_tracks
            .iter()
            .map(|(r, isrc)| (r.id, SongKey::of_library(r, isrc.as_deref())))
            .collect();
        let (lib_rows, lib_releases, lib_artists, lib_playlists) = {
            let s = self.state.read();
            let rows: Vec<v::TrackRow> = lib_keys
                .iter()
                .filter_map(|(id, _)| s.search.tracks.iter().find(|t| t.id == *id).cloned())
                .collect();
            (rows, s.search.releases.clone(), s.search.artists.clone(), s.search.playlists.clone())
        };

        // Tracks: one group per song.
        let mut groups: Vec<Group> = Vec::new();
        for (i, row) in lib_rows.into_iter().enumerate() {
            let Some((_, key)) = lib_keys.iter().find(|(id, _)| *id == row.id) else { continue };
            let score = q.track(&row.title, &[row.artist.as_str()], row.album.as_deref(), TrackKind::Song);
            groups.push(Group {
                key: key.clone(),
                lib: Some(row.id),
                row: v::SearchTrack::Library(row),
                score,
                pos: i,
                origin: 0,
                prio: 0,
            });
        }
        let shown: HashMap<RemoteId, v::StreamTrackRow> = sections
            .iter()
            .flat_map(|x| x.tracks.iter())
            .map(|r| (r.remote.clone(), r.clone()))
            .collect();
        let raw: Vec<(Service, StreamSearch)> = MERGE_ORDER
            .iter()
            .filter(|s| fresh.contains(s))
            .filter_map(|&s| {
                let slot = self.streaming.search.raw.iter().find(|x| x.service == s)?;
                match &slot.result {
                    Some(Ok(r)) => Some((s, r.clone())),
                    _ => None,
                }
            })
            .collect();
        for (origin, (service, r)) in raw.iter().enumerate() {
            for (i, t) in r.tracks.iter().enumerate() {
                let key = SongKey::of_stream(t);
                let linked = match self.streaming.search.linked.get(&t.id) {
                    Some(l) => *l,
                    None => {
                        let l = self.library_track_of(t);
                        self.streaming.search.linked.insert(t.id.clone(), l);
                        l
                    }
                };
                let names: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
                let score = q.track(&t.title, &names, t.album.as_ref().map(|a| a.title.as_str()), t.kind);
                let prio = self.play_prio(*service, t.kind);
                let found = groups
                    .iter_mut()
                    .find(|g| g.key.same(&key) || (linked.is_some() && g.lib == linked));
                if let Some(g) = found {
                    g.score = g.score.max(score);
                    g.merge(*service, prio, t, linked, &shown, self);
                    continue;
                }
                let row = shown.get(&t.id).cloned().unwrap_or_else(|| self.stream_row(t, linked));
                groups.push(Group {
                    key,
                    lib: linked,
                    row: v::SearchTrack::Stream(row),
                    score,
                    pos: i,
                    origin: origin + 1,
                    prio,
                });
            }
        }
        // A clip or upload shows only when nothing official (a library
        // track, a service's song) covers its song: «MORGENSHTERN -
        // ARISTOCRAT (Клип)» goes when «ARISTOCRAT» by MORGENSHTERN is here.
        let official: Vec<(String, String)> = groups
            .iter()
            .filter_map(|g| match &g.row {
                v::SearchTrack::Library(r) => Some((rank::song_title(&r.title), rank::norm(&r.artist))),
                v::SearchTrack::Stream(r) if r.kind == TrackKind::Song => {
                    Some((rank::song_title(&r.title), rank::norm(&r.artist)))
                }
                _ => None,
            })
            .collect();
        groups.retain(|g| match &g.row {
            v::SearchTrack::Stream(r) if r.kind != TrackKind::Song => {
                let (left, title) = rank::clip_parts(&r.title);
                let who: Vec<String> = left.into_iter().chain([rank::norm(&r.artist)]).filter(|w| !w.is_empty()).collect();
                !official.iter().any(|(t, a)| {
                    let by = |w: &String| a.contains(w.as_str()) || w.contains(a.as_str());
                    (*t == title && who.iter().any(by)) || title == format!("{a} {t}")
                })
            }
            _ => true,
        });
        let mut track_order: Vec<usize> = (0..groups.len()).collect();
        let quant = |s: f64| (s * 100.0).round() as i64;
        // Ties: the library's first (its songs are «у тебя»), then each
        // source in its own order.
        track_order.sort_by_key(|&i| (std::cmp::Reverse(quant(groups[i].score)), groups[i].origin, groups[i].pos));

        // Albums, artists, playlists: the library's first in a tie, the
        // services' cards as their sections give them (deduplicated).
        let lib_release_ids: Vec<_> = lib_releases.iter().map(|r| r.id).collect();
        let mut albums: Vec<Ranked<v::SearchAlbum, (String, String)>> = lib_releases
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let key = (album_key(&r.title), main_artist(&r.artist));
                let s = q.album(&r.title, &r.artist);
                (v::SearchAlbum::Library(r), s, i, 0, key)
            })
            .collect();
        let lib_artist_ids: Vec<_> = lib_artists.iter().map(|a| a.id).collect();
        let mut artists: Vec<Ranked<v::SearchArtist, String>> = lib_artists
            .into_iter()
            .enumerate()
            .map(|(i, a)| {
                let key = artist_key(&a.name);
                let s = q.artist(&a.name);
                (v::SearchArtist::Library(a), s, i, 0, key)
            })
            .collect();
        let mut playlists: Vec<(v::SearchPlaylist, f64, usize, usize)> = lib_playlists
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let s = q.playlist(&p.name);
                (v::SearchPlaylist::Library(p), s, i, 0)
            })
            .collect();
        for (origin, service) in MERGE_ORDER.iter().enumerate() {
            if !fresh.contains(service) {
                continue;
            }
            let Some(sec) = sections.iter().find(|x| x.service == *service) else { continue };
            for (i, a) in sec.albums.iter().enumerate() {
                let key = (album_key(&a.title), main_artist(&a.artist));
                if a.release.is_some_and(|r| lib_release_ids.contains(&r)) || albums.iter().any(|x| x.4 == key) {
                    continue;
                }
                albums.push((v::SearchAlbum::Stream(a.clone()), q.album(&a.title, &a.artist), i, origin + 1, key));
            }
            for (i, a) in sec.artists.iter().enumerate() {
                let key = artist_key(&a.name);
                if a.artist.is_some_and(|id| lib_artist_ids.contains(&id)) || artists.iter().any(|x| x.4 == key) {
                    // A library artist without a picture shows the service's.
                    if let Some(url) = &a.image_url
                        && let Some((v::SearchArtist::Library(c), ..)) = artists
                            .iter_mut()
                            .find(|x| x.4 == key || matches!(&x.0, v::SearchArtist::Library(c) if Some(c.id) == a.artist))
                        && c.image.is_none()
                    {
                        c.image = Some(v::Cover::web(url.clone()));
                    }
                    continue;
                }
                artists.push((v::SearchArtist::Stream(a.clone()), q.artist(&a.name), i, origin + 1, key));
            }
            for (i, p) in sec.playlists.iter().enumerate() {
                playlists.push((v::SearchPlaylist::Stream(p.clone()), q.playlist(&p.name), i, origin + 1));
            }
        }
        // Coarse buckets: a slower source's card goes after the ones of
        // the same kind of match already shown.
        let bucket = |s: f64| (s * 4.0).floor() as i64;
        albums.sort_by_key(|x| (std::cmp::Reverse(bucket(x.1)), x.2, x.3));
        artists.sort_by_key(|x| (std::cmp::Reverse(bucket(x.1)), x.2, x.3));
        playlists.sort_by_key(|x| (std::cmp::Reverse(bucket(x.1)), x.2, x.3));

        // The top result: settled once, then found again by its key.
        let st = &mut self.streaming.search;
        let settled = st.settled;
        // A settled top that is gone (a clip that an official song came to
        // cover) is picked again.
        let present = |k: &TopKey| match k {
            TopKey::Artist(k) => artists.iter().any(|x| x.4 == *k),
            TopKey::Album(t, a) => albums.iter().any(|x| x.4.0 == *t && x.4.1 == *a),
            TopKey::Track(k) => track_order.iter().any(|&i| groups[i].key.same(k)),
        };
        let gone = st.frozen_top.as_ref().is_some_and(|k| !present(k));
        let first_pick = st.frozen_top.is_none() && st.frozen_tracks.is_empty();
        if settled && (first_pick || gone) {
            let t_scores: Vec<f64> = track_order.iter().map(|&i| groups[i].score).collect();
            let a_scores: Vec<f64> = artists.iter().map(|x| x.1).collect();
            let al_scores: Vec<f64> = albums.iter().map(|x| x.1).collect();
            st.frozen_top = rank::top(&a_scores, &t_scores, &al_scores).map(|t| match t {
                rank::Top::Artist(i) => TopKey::Artist(artists[i].4.clone()),
                rank::Top::Track(i) => TopKey::Track(groups[track_order[i]].key.clone()),
                rank::Top::Album(i) => TopKey::Album(albums[i].4.0.clone(), albums[i].4.1.clone()),
            });
        }
        if settled && first_pick {
            let top_track = match &st.frozen_top {
                Some(TopKey::Track(k)) => track_order.iter().position(|&i| groups[i].key.same(k)),
                _ => None,
            };
            // Only good matches hold their place: a weak one shown early
            // gives way to better ones that come later.
            st.frozen_tracks = track_order
                .iter()
                .enumerate()
                .filter(|(n, _)| Some(*n) != top_track)
                .take(v::TOP_TRACKS)
                .filter(|(_, i)| groups[**i].score >= rank::TOP_MIN)
                .map(|(_, &i)| groups[i].key.clone())
                .collect();
        }
        let mut top = None;
        match st.frozen_top.clone() {
            Some(TopKey::Artist(k)) => {
                if let Some(i) = artists.iter().position(|x| x.4 == k) {
                    top = Some(v::TopResult::Artist(artists.remove(i).0));
                }
            }
            Some(TopKey::Album(t, a)) => {
                if let Some(i) = albums.iter().position(|x| x.4.0 == t && x.4.1 == a) {
                    top = Some(v::TopResult::Album(albums.remove(i).0));
                }
            }
            Some(TopKey::Track(k)) => {
                if let Some(n) = track_order.iter().position(|&i| groups[i].key.same(&k)) {
                    let i = track_order.remove(n);
                    top = Some(v::TopResult::Track(groups[i].row.clone()));
                }
            }
            None => {}
        }
        // The first tracks keep their places; the rest follows by rank.
        let mut ordered: Vec<usize> = Vec::with_capacity(track_order.len());
        for k in &st.frozen_tracks {
            if let Some(n) = track_order.iter().position(|&i| groups[i].key.same(k)) {
                ordered.push(track_order.remove(n));
            }
        }
        ordered.extend(track_order);

        let note = st.raw.iter().find_map(|s| {
            let w = s.wait.as_ref()?;
            let other = st.raw.iter().map(|x| x.service).find(|&x| x != s.service);
            Some(wait_note(s.service, w, other))
        });
        let mut answer = v::SearchAnswer {
            query: query.clone(),
            top,
            top_pending: !settled,
            tracks: ordered.into_iter().map(|i| groups[i].row.clone()).collect(),
            albums: albums.into_iter().map(|x| x.0).collect(),
            artists: artists.into_iter().map(|x| x.0).collect(),
            playlists: playlists.into_iter().map(|x| x.0).collect(),
            searching: !all_done,
            note,
            error: None,
        };
        if all_done && answer.is_empty() {
            answer.error = st.raw.iter().find_map(|s| match &s.result {
                Some(Err(StreamError::RateLimited { .. })) | None | Some(Ok(_)) => None,
                Some(Err(e)) => Some(search_error(s.service, e)),
            });
        }
        Some(answer)
    }

    /// Which copy of a song plays best (lower first): a library track,
    /// Spotify with sound, a YouTube Music song, Spotify without sound
    /// (plays from YouTube Music), Deezer (the same), a YouTube clip or
    /// upload.
    fn play_prio(&self, service: Service, kind: TrackKind) -> u8 {
        match service {
            Service::Spotify if self.has_audio(Service::Spotify) => 1,
            Service::Youtube if kind == TrackKind::Song => 2,
            Service::Spotify => 3,
            Service::Deezer => 4,
            Service::Soundcloud if kind == TrackKind::Song => 4,
            Service::Youtube | Service::Soundcloud => 5,
        }
    }
}

/// A card of the merged answer: it, its score, its place in its source,
/// the source (0 the library, then [`MERGE_ORDER`]), its dedup key.
type Ranked<T, K> = (T, f64, usize, usize, K);

/// One song of the merged answer and the copy it shows (and plays).
struct Group {
    key: SongKey,
    /// Its library track, when it has one.
    lib: Option<TrackId>,
    row: v::SearchTrack,
    /// The best score of its copies.
    score: f64,
    /// Its place in the first source that had it.
    pos: usize,
    /// That source: 0 the library, then [`MERGE_ORDER`].
    origin: usize,
    /// [`Worker::play_prio`] of the copy shown.
    prio: u8,
}

impl Group {
    /// One more copy of the song (from `service`): it is shown instead
    /// when it plays better; the other services go to `also_on`.
    fn merge(
        &mut self,
        service: Service,
        prio: u8,
        t: &StreamTrack,
        linked: Option<TrackId>,
        shown: &HashMap<RemoteId, v::StreamTrackRow>,
        w: &Worker,
    ) {
        let v::SearchTrack::Stream(row) = &mut self.row else { return }; // the library wins
        if prio < self.prio {
            let mut better = shown.get(&t.id).cloned().unwrap_or_else(|| w.stream_row(t, linked));
            better.also_on = row.also_on.clone();
            if !better.also_on.contains(&row.remote.service) {
                better.also_on.push(row.remote.service);
            }
            better.also_on.retain(|s| *s != service);
            self.prio = prio;
            self.lib = self.lib.or(linked);
            *row = better;
        } else if row.remote.service != service && !row.also_on.contains(&service) {
            row.also_on.push(service);
        }
    }
}

impl Worker {
    /// The library track a stream track is: linked by its remote id, else
    /// the one with its ISRC.
    pub(crate) fn library_track_of(&self, t: &StreamTrack) -> Option<TrackId> {
        self.lib.track_for_stream(t).ok().flatten()
    }
}

/// The tracks of a query and of its «ё» spellings as one list: taken in
/// turn (each list's best near the top), each track once.
fn interleave(first: Vec<StreamTrack>, more: Vec<Vec<StreamTrack>>) -> Vec<StreamTrack> {
    if more.iter().all(Vec::is_empty) {
        return first;
    }
    let mut lists: Vec<std::vec::IntoIter<StreamTrack>> =
        std::iter::once(first).chain(more).map(Vec::into_iter).collect();
    let mut out: Vec<StreamTrack> = Vec::new();
    loop {
        let mut any = false;
        for l in &mut lists {
            if let Some(t) = l.next() {
                any = true;
                if !out.iter().any(|x| x.id == t.id) {
                    out.push(t);
                }
            }
        }
        if !any {
            return out;
        }
    }
}

/// Why a service's section is empty, as the user reads it.
fn search_error(service: Service, e: &StreamError) -> String {
    match e {
        StreamError::RateLimited { .. } => format!("{} сейчас перегружен", service.display_name()),
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
