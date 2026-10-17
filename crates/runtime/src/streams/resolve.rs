//! Playback resolution beyond phase 1's source order: a track that only has
//! sources nobody can play right now (Spotify without its sound for this
//! account, a file that is gone) is looked up on YouTube Music in the
//! background — ISRC first, then "artist title" with the official song
//! preferred over a clip over an upload, a length within 3 s and the best
//! title match — and the hit is attached to the same track id as one more
//! source. Misses are not asked again for a while.

use std::time::Instant;

use nikgob_core::model::{SourceKind, TrackId};
use nikgob_core::streaming::{Service, StreamTrack, TrackKind};
use nikgob_metadata::text::{artist_key, search_artist, search_title, similarity, title_key};

use super::{MISS_TTL, Msg};
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
        .filter(|h| h.playable)
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

/// "artist title" first; when that finds nothing, the title alone with the
/// artist checked on the hits.
pub(crate) async fn search_youtube(
    yt: &dyn nikgob_core::streaming::StreamService,
    q: &ResolveQuery,
) -> Option<StreamTrack> {
    match yt.search(&q.text()).await {
        Ok(r) => {
            if let Some(hit) = pick_youtube(&r.tracks, q) {
                return Some(hit);
            }
        }
        Err(e) => log::warn!("YouTube lookup of {}: {e}", q.text()),
    }
    let hits = yt.search(&search_title(&q.title)).await.ok()?.tracks;
    let artist = artist_key(&q.artist);
    let by_artist: Vec<StreamTrack> = hits
        .into_iter()
        .filter(|h| {
            h.artists
                .iter()
                .any(|a| similarity(&artist_key(&a.name), &artist) >= MIN_ARTIST)
        })
        .collect();
    pick_youtube(&by_artist, q)
}

impl Worker {
    /// The track can be looked up on YouTube Music: YouTube is on, the
    /// track has no YouTube source yet, and it did not miss recently.
    pub(crate) fn can_resolve(&mut self, track: TrackId) -> bool {
        if self.service(Service::Youtube).is_none() || !self.has_audio(Service::Youtube) {
            return false;
        }
        if self.streaming.resolving.contains(&track) {
            return true;
        }
        if let Some(at) = self.streaming.misses.get(&track) {
            if at.elapsed() < MISS_TTL {
                return false;
            }
            self.streaming.misses.remove(&track);
        }
        let has_youtube = self.lib.sources_of_track(track).unwrap_or_default().iter().any(|s| {
            s.kind == SourceKind::Youtube && s.available && s.trashed_at.is_none()
        });
        !has_youtube && self.lib.get_track(track).ok().flatten().is_some()
    }

    /// Starts the YouTube lookup of `track` (once).
    pub(crate) fn begin_resolve(&mut self, track: TrackId) {
        if self.streaming.resolving.contains(&track) {
            return;
        }
        let Some(yt) = self.service(Service::Youtube) else { return };
        let Some(t) = self.lib.get_track(track).ok().flatten() else { return };
        let q = ResolveQuery {
            title: t.title.clone(),
            artist: t.artist_credit.display(),
            duration_ms: t.duration_ms.map(u64::from),
            isrc: t.isrc.clone(),
        };
        self.streaming.resolving.insert(track);
        log::info!("looking up {} on YouTube Music", q.text());
        self.streaming.spawn(async move {
            if let Some(isrc) = &q.isrc
                && let Ok(Some(hit)) = yt.find_isrc(isrc).await
                && pick_youtube(std::slice::from_ref(&hit), &q).is_some()
            {
                return Msg::Resolved {
                    track,
                    found: Some(hit),
                };
            }
            Msg::Resolved {
                track,
                found: search_youtube(yt.as_ref(), &q).await,
            }
        });
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

    pub(crate) fn resolved(&mut self, track: TrackId, found: Option<StreamTrack>) {
        self.streaming.resolving.remove(&track);
        match &found {
            Some(hit) => {
                log::info!("{track} plays from YouTube Music as {}", hit.id.id);
                self.streaming.learn(std::slice::from_ref(hit));
                if let Err(e) = self.lib.attach_stream_source(track, hit) {
                    log::warn!("attaching the YouTube copy of {track}: {e}");
                }
            }
            None => {
                self.streaming.misses.insert(track, Instant::now());
            }
        }
        if let Some(item) = self.streaming.save_after_resolve.remove(&track) {
            self.wish_save_from_youtube(item);
        }
        let waiting = self
            .playing
            .filter(|e| e.track == track && self.playing_source.is_none());
        if let Some(e) = waiting {
            let paused = self.status == nikgob_view::Status::Paused;
            match self.request_skipping(e, &[]) {
                Some((req, source, kind)) => {
                    self.start_with(e, req, source, kind);
                    if paused {
                        self.playback.pause();
                        self.status = nikgob_view::Status::Paused;
                        self.publish_player();
                    }
                }
                None => {
                    self.toast("Не нашёл этот трек на YouTube Music", None);
                    match self.queue.skip_forward() {
                        Some(n) if n.id != e.id => self.start(n),
                        _ => self.stop(),
                    }
                }
            }
        } else if self.queue.peek_auto_next().is_some_and(|n| n.track == track) {
            self.preload_next();
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
    }
}
