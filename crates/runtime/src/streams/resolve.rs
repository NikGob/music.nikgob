//! Playback resolution beyond phase 1's source order: a track that only has
//! sources nobody can play right now (Spotify without its sound for this
//! account, a file that is gone) is looked up on YouTube Music in the
//! background — ISRC first, then "artist title" with the official song
//! preferred over a clip over an upload, a length within 3 s and the best
//! title match — and the hit is attached to the same track id as one more
//! source. Misses are not asked again for days (kept in the library's
//! cache, so a restart does not ask either). The lookups ask one category
//! at a time: songs, videos only when no song fits (2 requests instead of
//! up to 11). The next one or two tracks of the queue are looked up ahead,
//! one after the other, so a Spotify-only track starts without waiting.
//!
//! A Deezer track (the catalogue has no sound) goes the same way. Deezer
//! lists album tracks without their ISRC, so a track that has none yet is
//! first asked of Deezer alone (`/track/{id}`, one request): the ISRC it
//! brings is kept on the track and makes the YouTube lookup exact.
//!
//! A rate limit is not a miss: nothing is recorded, the lookups ahead wait
//! for the pause to end, and a track that waits for its lookup (Buffering)
//! is looked up again by itself when a pause of up to a minute ends («YouTube
//! Music просит подождать — продолжу сам»), at most three times. After a
//! longer pause playback stops and the toast says when to try again.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use nikgob_core::model::{SourceKind, TrackId};
use nikgob_core::streaming::{SearchKind, Service, StreamTrack, TrackKind};
use nikgob_metadata::text::{artist_key, search_artist, search_title, similarity, title_key};

use super::Msg;
use crate::Worker;
use crate::queue::Entry;

/// Lengths this far apart (ms) are another recording.
const SLACK_MS: u64 = 3_000;
/// A hit needs at least this title similarity (0..=1).
const MIN_TITLE: f64 = 0.6;
/// A hit found by its title alone needs this artist similarity (0..=1).
const MIN_ARTIST: f64 = 0.7;

/// What is looked up on YouTube Music for a track.
#[derive(Clone, Debug)]
pub(crate) struct ResolveQuery {
    pub(crate) title: String,
    pub(crate) artist: String,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) isrc: Option<String>,
    /// Videos that did not play (age gate, region block): another
    /// candidate is picked instead.
    pub(crate) exclude: Vec<String>,
}

impl ResolveQuery {
    /// The search text: "artist title" without release noise.
    pub(crate) fn text(&self) -> String {
        format!("{} {}", search_artist(&self.artist), search_title(&self.title))
    }
}

fn kind_rank(k: TrackKind) -> u8 {
    match k {
        TrackKind::Song => 0,
        TrackKind::Video => 1,
        TrackKind::Upload => 2,
    }
}

/// Title similarity of a hit; a clip titled "Artist - Title (…)" is also
/// compared without its "Artist - " part.
fn title_score(hit: &StreamTrack, want: &str) -> f64 {
    let base = |s: &str| title_key(s).base;
    let mut best = similarity(&base(&hit.title), want);
    if let Some((_, rest)) = hit.title.split_once(" - ") {
        best = best.max(similarity(&base(rest), want));
    }
    best
}

/// The best YouTube hit for `q`, if any is good enough.
pub(crate) fn pick_youtube(hits: &[StreamTrack], q: &ResolveQuery) -> Option<StreamTrack> {
    let want = title_key(&q.title).base;
    let want_artist = artist_key(&q.artist);
    let mut ranked: Vec<(u8, f64, &StreamTrack)> = hits
        .iter()
        .filter(|h| h.playable && !q.exclude.contains(&h.id.id))
        .filter(|h| match (h.duration_ms, q.duration_ms) {
            (Some(a), Some(b)) => a.abs_diff(b) <= SLACK_MS,
            _ => true,
        })
        .filter_map(|h| {
            let title = title_score(h, &want);
            if title < MIN_TITLE {
                return None;
            }
            let artist = h
                .artists
                .iter()
                .map(|a| similarity(&artist_key(&a.name), &want_artist))
                .fold(0.0, f64::max);
            Some((kind_rank(h.kind), title + 0.3 * artist, h))
        })
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    ranked.first().map(|(.., h)| (*h).clone())
}

/// Tracks to look up ahead in the queue.
const AHEAD: usize = 2;
/// Library cache key of a lookup that found nothing.
fn miss_key(track: TrackId) -> String {
    format!("yt_miss.{track}")
}

/// Re-lookups of a waiting track after rate limits.
const LIMIT_RETRIES: u32 = 3;
/// The pause assumed when YouTube Music does not say how long it is.
const UNKNOWN_PAUSE: Duration = Duration::from_secs(30);

/// A lookup's answer: the hit (or none), or `Err(retry_after)` when the
/// service rate-limited it.
pub(crate) type Lookup = Result<Option<StreamTrack>, Option<Duration>>;

/// Rate-limit state of the lookups.
#[derive(Default)]
pub(crate) struct ResolveLimit {
    /// No lookup ahead starts before this.
    until: Option<Instant>,
    /// Re-lookups of the waiting track so far.
    retries: HashMap<TrackId, u32>,
}

/// Songs, then videos only when no song fits; a rate limit stops it.
async fn songs_then_videos(
    yt: &dyn nikgob_core::streaming::StreamService,
    text: &str,
    keep: &(dyn Fn(&StreamTrack) -> bool + Sync),
    q: &ResolveQuery,
) -> Lookup {
    for kind in [SearchKind::Songs, SearchKind::Videos] {
        match yt.search_kind(text, kind).await {
            Ok(r) => {
                let hits: Vec<StreamTrack> = r.tracks.into_iter().filter(|t| keep(t)).collect();
                if let Some(hit) = pick_youtube(&hits, q) {
                    return Ok(Some(hit));
                }
            }
            Err(e @ nikgob_core::streaming::StreamError::RateLimited { retry_after }) => {
                log::warn!("YouTube lookup of {text}: {e}");
                return Err(retry_after);
            }
            Err(e) => log::warn!("YouTube lookup of {text}: {e}"),
        }
    }
    Ok(None)
}

/// "artist title" first; when that finds nothing, the title alone with the
/// artist checked on the hits. A rate limit is `None` too.
pub(crate) async fn search_youtube(
    yt: &dyn nikgob_core::streaming::StreamService,
    q: &ResolveQuery,
) -> Option<StreamTrack> {
    lookup_youtube(yt, q).await.ok().flatten()
}

/// [`search_youtube`] telling a rate limit apart.
async fn lookup_youtube(yt: &dyn nikgob_core::streaming::StreamService, q: &ResolveQuery) -> Lookup {
    if let Some(hit) = songs_then_videos(yt, &q.text(), &|_| true, q).await? {
        return Ok(Some(hit));
    }
    let artist = artist_key(&q.artist);
    let by_artist = move |h: &StreamTrack| {
        h.artists
            .iter()
            .any(|a| similarity(&artist_key(&a.name), &artist) >= MIN_ARTIST)
    };
    songs_then_videos(yt, &search_title(&q.title), &by_artist, q).await
}

/// Why a stream did not play, from the engine's error text, as the user
/// reads it ("ARISTOCRAT: <this>"), and the settings row that helps, if
/// one does (the toast's «Войти»).
pub(crate) fn failure_text(
    message: &str,
    youtube_signed_in: bool,
    service: Option<Service>,
) -> (String, Option<nikgob_view::SettingLink>) {
    use nikgob_view::SettingLink;
    let m = message.to_lowercase();
    if m.contains("age-restricted") {
        if youtube_signed_in {
            ("на YouTube только для 18+, и со входом не открылся".into(), None)
        } else {
            ("на YouTube только для 18+ — нужен вход в YouTube Music".into(), Some(SettingLink::Youtube))
        }
    } else if m.contains("blocked in this region") {
        ("недоступно в твоём регионе".into(), None)
    } else if m.contains("premium") {
        ("для звука из Spotify нужен Premium".into(), None)
    } else if m.contains("rate limited") {
        // Playback goes on with the next track; this one waits for the user.
        let who = service.map_or("сервис", |s| s.display_name());
        (format!("{who} просит подождать — включи трек чуть позже"), None)
    } else if m.contains("not available") || m.contains("недоступ") {
        ("трек недоступен".into(), None)
    } else {
        ("не получилось воспроизвести".into(), None)
    }
}

impl Worker {
    /// Videos refused lately (see [`Self::stream_failed`]).
    pub(crate) fn refused_videos(&mut self) -> Vec<String> {
        self.streaming
            .refused
            .retain(|_, at| at.elapsed() < super::REFUSED_TTL);
        self.streaming.refused.keys().cloned().collect()
    }

    /// The playing source of `track` did not open: a YouTube video that
    /// is refused (age gate, region block, gone) is set aside, so the next
    /// lookup picks another copy of the song; the reason is kept for the
    /// toast if none plays.
    pub(crate) fn stream_failed(&mut self, track: TrackId, source: nikgob_core::model::SourceId, message: &str) {
        let Some(src) = self.lib.get_source(source).ok().flatten() else { return };
        let signed_in = self
            .service(Service::Youtube)
            .is_some_and(|y| y.account().is_logged_in());
        let why = failure_text(message, signed_in, src.kind.service());
        log::info!("{track}: {} source {} failed: {message}", src.kind.as_str(), src.locator);
        self.streaming.failures.insert(track, why);
        if src.kind == SourceKind::Youtube {
            self.streaming.refused.insert(src.locator.clone(), Instant::now());
            if let Err(e) = self.lib.set_source_available(source, false) {
                log::warn!("setting aside the YouTube copy of {track}: {e}");
            }
        }
    }

    /// The track can be looked up on YouTube Music: YouTube is on, the
    /// track has no YouTube source yet, and it did not miss recently.
    pub(crate) fn can_resolve(&mut self, track: TrackId) -> bool {
        if self.service(Service::Youtube).is_none() || !self.has_audio(Service::Youtube) {
            return false;
        }
        if self.streaming.resolving.contains(&track) {
            return true;
        }
        let ttl = self.streaming.schedule.miss_ttl.as_millis() as i64;
        let now = nikgob_library::now_ms();
        let missed = match self.streaming.misses.get(&track) {
            Some(at) => Some(*at),
            None => self
                .lib
                .cache_get(&miss_key(track))
                .ok()
                .flatten()
                .map(|(at, _)| at),
        };
        if let Some(at) = missed {
            if now - at < ttl {
                self.streaming.misses.insert(track, at);
                return false;
            }
            self.streaming.misses.remove(&track);
        }
        let has_youtube = self.lib.sources_of_track(track).unwrap_or_default().iter().any(|s| {
            s.kind == SourceKind::Youtube && s.available && s.trashed_at.is_none()
        });
        !has_youtube && self.lib.get_track(track).ok().flatten().is_some()
    }

    /// The Deezer id of a track that has no ISRC yet, and the catalogue to
    /// ask it of (see the module docs).
    pub(crate) fn isrc_source(&self, track: TrackId) -> Option<(std::sync::Arc<dyn nikgob_core::streaming::StreamService>, String)> {
        let dz = self.service(Service::Deezer)?;
        let id = self
            .lib
            .external_ids(nikgob_core::streaming::EntityKind::Track, *track.as_uuid())
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.service == Service::Deezer)?
            .id;
        Some((dz, id))
    }

    /// Starts the YouTube lookup of `track` (once).
    pub(crate) fn begin_resolve(&mut self, track: TrackId) {
        if self.streaming.resolving.contains(&track) {
            return;
        }
        let Some(yt) = self.service(Service::Youtube) else { return };
        let Some(t) = self.lib.get_track(track).ok().flatten() else { return };
        let mut q = ResolveQuery {
            title: t.title.clone(),
            artist: t.artist_credit.display(),
            duration_ms: t.duration_ms.map(u64::from),
            isrc: t.isrc.clone(),
            exclude: self.refused_videos(),
        };
        let deezer = if q.isrc.is_none() { self.isrc_source(track) } else { None };
        self.streaming.resolving.insert(track);
        log::info!("looking up {} on YouTube Music", q.text());
        self.streaming.spawn(async move {
            // A Deezer track without its ISRC: Deezer tells it first.
            let mut filled = None;
            if let Some((dz, id)) = deezer {
                match dz.track(&id).await {
                    Ok(full) => {
                        q.isrc = full.isrc.clone();
                        filled = Some(full);
                    }
                    Err(e) => log::info!("deezer: no ISRC for {id}: {e}"),
                }
            }
            if let Some(isrc) = &q.isrc
                && let Ok(Some(hit)) = yt.find_isrc(isrc).await
                && pick_youtube(std::slice::from_ref(&hit), &q).is_some()
            {
                return Msg::Resolved {
                    track,
                    found: Ok(Some(hit)),
                    filled,
                };
            }
            Msg::Resolved {
                track,
                found: lookup_youtube(yt.as_ref(), &q).await,
                filled,
            }
        });
    }

    /// The full Deezer track of a library track came (with its ISRC): the
    /// ISRC fills the track's empty one.
    pub(crate) fn isrc_filled(&mut self, track: TrackId, full: &StreamTrack) {
        if full.isrc.is_none() {
            return;
        }
        self.streaming.learn(std::slice::from_ref(full));
        if let Err(e) = self.lib.attach_stream_source(track, full) {
            log::warn!("keeping the ISRC of {track}: {e}");
        }
    }

    /// `e` has nothing playable now but can be looked up: it waits
    /// (Buffering) for the lookup instead of being skipped.
    pub(crate) fn wait_for_youtube(&mut self, e: Entry) -> bool {
        if !self.can_resolve(e.track) {
            return false;
        }
        self.begin_resolve(e.track);
        self.playback.stop();
        self.playing = Some(e);
        self.playing_source = None;
        self.restarted = None;
        self.status = nikgob_view::Status::Buffering;
        self.position = std::time::Duration::ZERO;
        self.position_at = Instant::now();
        self.publish_now_playing();
        true
    }

    pub(crate) fn resolved(&mut self, track: TrackId, found: Lookup, filled: Option<StreamTrack>) {
        self.streaming.resolving.remove(&track);
        if let Some(full) = &filled {
            self.isrc_filled(track, full);
        }
        let found = match found {
            Ok(f) => {
                self.streaming.resolve_limit.retries.remove(&track);
                f
            }
            Err(retry_after) => return self.resolve_limited(track, retry_after),
        };
        match &found {
            Some(hit) => {
                log::info!("{track} plays from YouTube Music as {}", hit.id.id);
                self.streaming.learn(std::slice::from_ref(hit));
                if let Err(e) = self.lib.attach_stream_source(track, hit) {
                    log::warn!("attaching the YouTube copy of {track}: {e}");
                }
            }
            None => {
                let now = nikgob_library::now_ms();
                self.streaming.misses.insert(track, now);
                let _ = self.lib.cache_put(&miss_key(track), now, "");
            }
        }
        let why = self.streaming.failures.remove(&track);
        if let Some(item) = self.streaming.save_after_resolve.remove(&track) {
            self.wish_save_from_youtube(item);
        }
        let waiting = self
            .playing
            .filter(|e| e.track == track && self.playing_source.is_none());
        if let Some(e) = waiting {
            let paused = self.status == nikgob_view::Status::Paused;
            // Not the sources that failed a moment ago (Spotify refusing
            // sound): straight to the copy just found.
            let skip = self.tried.clone();
            match self.request_skipping(e, &skip) {
                Some((req, source, kind)) => {
                    self.start_with(e, req, source, kind);
                    if paused {
                        self.playback.pause();
                        self.status = nikgob_view::Status::Paused;
                        self.publish_player();
                    }
                }
                None => {
                    let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
                    match why {
                        // The only copies did not play: say why.
                        Some((why, Some(link))) => {
                            self.toast_open(format!("{title}: {why}"), "Войти", nikgob_view::Link::Setting(link));
                        }
                        Some((why, None)) => self.toast(format!("{title}: {why}"), None),
                        None => self.toast(format!("{title}: не нашёл на YouTube Music"), None),
                    }
                    match self.queue.skip_forward() {
                        Some(n) if n.id != e.id => self.start(n),
                        _ => self.stop(),
                    }
                }
            }
        } else if self.queue.peek_auto_next().is_some_and(|n| n.track == track) {
            self.preload_next();
        }
        self.resolve_ahead();
    }

    /// YouTube Music rate-limited the lookup of `track` (see the module
    /// docs): not a miss. The waiting track is looked up again when a short
    /// pause ends; after a long one playback stops and says until when.
    fn resolve_limited(&mut self, track: TrackId, retry_after: Option<Duration>) {
        let pause = retry_after.or_else(|| self.pause_left(Service::Youtube));
        let wait = pause.unwrap_or(UNKNOWN_PAUSE);
        self.streaming.resolve_limit.until = Some(Instant::now() + wait);
        self.streaming.failures.remove(&track);
        let waiting = self
            .playing
            .filter(|e| e.track == track && self.playing_source.is_none());
        if waiting.is_none() {
            // A lookup ahead: done again when the pause is over.
            log::info!("{track}: YouTube lookup rate limited, later");
            return;
        }
        let n = self.streaming.resolve_limit.retries.entry(track).or_default();
        *n += 1;
        let n = *n;
        if wait <= super::search::SHORT_WAIT && n <= LIMIT_RETRIES {
            log::info!("{track}: YouTube lookup rate limited, again in {} s ({n} of {LIMIT_RETRIES})", wait.as_secs());
            if n == 1 {
                self.toast("YouTube Music просит подождать — продолжу сам", None);
            }
            let after = wait + super::schedule::wake_jitter();
            self.streaming.spawn(async move {
                tokio::time::sleep(after).await;
                Msg::ResolveRetry(track)
            });
            return;
        }
        self.streaming.resolve_limit.retries.remove(&track);
        let title = self.lib.get_track(track).ok().flatten().map(|t| t.title).unwrap_or_default();
        let until = pause
            .filter(|p| *p > super::search::SHORT_WAIT)
            .and_then(super::accounts::hm_after)
            .map(|hm| format!(" до {hm}"))
            .unwrap_or_default();
        log::info!("{track}: YouTube lookup still rate limited: stopping");
        // Not skipped: the next tracks would hit the same pause.
        self.toast(format!("{title}: YouTube Music просит подождать{until} — включи трек позже"), None);
        self.stop();
    }

    /// The pause of a waiting track's lookup ended: it is looked up again
    /// if it still waits.
    pub(crate) fn resolve_retry(&mut self, track: TrackId) {
        let waiting = self
            .playing
            .is_some_and(|e| e.track == track && self.playing_source.is_none());
        if !waiting {
            self.streaming.resolve_limit.retries.remove(&track);
            return;
        }
        self.begin_resolve(track);
    }

    /// Looks up the next [`AHEAD`] tracks of the queue that only YouTube
    /// Music may have, one at a time (the next starts when this one
    /// answered), whatever the gapless setting; none while YouTube Music
    /// asked to wait.
    pub(crate) fn resolve_ahead(&mut self) {
        if !self.streaming.resolving.is_empty() {
            return; // one lookup at a time; the playing track's goes first
        }
        if self.streaming.resolve_limit.until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let mut next: Vec<Entry> = self.queue.peek_auto_next().into_iter().collect();
        next.extend(self.queue.upcoming().iter().skip(1).take(AHEAD - 1).copied());
        let current = self.queue.current().map(|c| c.track);
        for e in next {
            if Some(e.track) == current {
                continue;
            }
            if self.request(e).is_none() && self.can_resolve(e.track) {
                self.begin_resolve(e.track);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::{ArtistRef, RemoteId};

    fn hit(id: &str, title: &str, secs: u64, kind: TrackKind) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = vec![ArtistRef::new(None, "Кино")];
        t.duration_ms = Some(secs * 1000);
        t.kind = kind;
        t
    }

    #[test]
    fn songs_first_then_clips_with_the_same_length() {
        let q = ResolveQuery {
            title: "Группа крови".into(),
            artist: "Кино".into(),
            duration_ms: Some(285_000),
            isrc: None,
            exclude: Vec::new(),
        };
        assert_eq!(q.text(), "Кино Группа крови");
        let clip = hit("clip", "Кино - Группа крови (клип)", 286, TrackKind::Video);
        let song = hit("song", "Группа крови", 284, TrackKind::Song);
        let long = hit("long", "Группа крови", 300, TrackKind::Song);
        let other = hit("other", "Звезда по имени Солнце", 285, TrackKind::Song);
        let upload = hit("up", "Группа крови", 285, TrackKind::Upload);
        let all = [clip.clone(), long.clone(), other.clone(), upload, song.clone()];
        assert_eq!(pick_youtube(&all, &q).unwrap().id.id, "song");
        assert_eq!(pick_youtube(&[clip.clone(), long, other], &q).unwrap().id.id, "clip");
        assert!(pick_youtube(&[hit("x", "Война", 285, TrackKind::Song)], &q).is_none());
        let mut gone = song;
        gone.playable = false;
        assert!(pick_youtube(&[gone], &q).is_none());
        // A video that did not play: the next candidate.
        let q = ResolveQuery {
            exclude: vec!["song".into()],
            ..q
        };
        assert_eq!(pick_youtube(&all, &q).unwrap().id.id, "clip");
    }
}
