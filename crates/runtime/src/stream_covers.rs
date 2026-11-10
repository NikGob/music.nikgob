//! Real album art for stream tracks outside the library that show a
//! video's thumbnail (a recommendation card, a search result): the black
//! bars are cut on load, but the picture is still a frame of the video.
//!
//! The UI says which of those thumbnails come on screen
//! (`Command::RealCovers`), so only the cards on screen are looked up (the
//! library's own tracks are the library's sweep's): one
//! at a time, the latest drawn first, by the lookup of the library's poor
//! covers ([`better_covers::find`]: Deezer, the iTunes Search API, the
//! services' songs, under the metadata clients' rate limits). What the
//! song is comes from the stream track the services gave (its title, the
//! channel and the names the title credits). A lookup a service could not
//! answer (a timeout, a network hiccup) is skipped and the lookups rest a
//! while; [`STOP_AFTER`] such in a row (a refusal, a rate limit) stop them
//! for the run.
//!
//! What is found is kept in the library's cache
//! (`cover_better.stream.<video id>`, a miss too, asked again after
//! [`RETRY_MS`]); the thumbnail's url → the art's url goes to
//! `ViewState::real_covers`, and the covers on screen take it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_core::streaming::{RemoteId, Service, StreamService, StreamTrack, TrackKind};
use nikgob_library::now_ms;

use crate::Worker;
use crate::better_covers::{self, Want};
use crate::import::Internal;

/// A lookup that found nothing is asked again after this long.
const RETRY_MS: i64 = 14 * 86_400_000;
/// Thumbnails waiting at most: what scrolled away long ago is dropped (and
/// asked again when it is drawn again).
const QUEUE_MAX: usize = 24;
/// One lookup takes at most this long.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(120);
/// After a lookup a service could not answer, the lookups rest this long
/// times the failures in a row.
const PAUSE: Duration = Duration::from_secs(60);
/// This many failures in a row: the service refuses, nothing more is asked
/// this run.
const STOP_AFTER: u32 = 3;

/// The lookups of this run.
#[derive(Default)]
pub(crate) struct StreamCovers {
    /// Thumbnails looked at this run (waiting, asked, answered from the
    /// cache, or not a stream's).
    seen: HashSet<String>,
    /// Drawn before the services gave their stream track: looked at again
    /// with the next thumbnails drawn (at most [`QUEUE_MAX`]).
    unknown: VecDeque<String>,
    /// Waiting, the latest drawn first.
    queue: VecDeque<(String, StreamTrack)>,
    busy: bool,
    /// Lookups a service could not answer, in a row.
    failures: u32,
    /// Resting after one of them until then.
    paused_until: Option<Instant>,
    /// Services refused [`STOP_AFTER`] times in a row: nothing more is asked
    /// this run.
    stopped: bool,
    /// Thumbnail url → the art found.
    found: HashMap<String, String>,
}

/// A lookup's answer.
pub(crate) struct Found {
    url: String,
    video: String,
    result: Result<Option<String>, String>,
}

/// The video id of a YouTube thumbnail url (`i.ytimg.com/vi/<id>/…`).
pub(crate) fn video_id(url: &str) -> Option<&str> {
    let rest = url.split_once("i.ytimg.com/")?.1;
    let rest = rest.strip_prefix("vi/").or_else(|| rest.strip_prefix("vi_webp/"))?;
    rest.split(['/', '?', '#']).next().filter(|id| !id.is_empty())
}

fn key(video: &str) -> String {
    format!("cover_better.stream.{video}")
}

/// After the `failures`-th lookup in a row a service could not answer: how
/// long the lookups rest, or `None` when they stop for the run.
fn rest_after(failures: u32) -> Option<Duration> {
    (failures < STOP_AFTER).then(|| PAUSE * failures.max(1))
}

/// What a stream track says of its song: a clip's titles as its title has
/// them, its artists with the names the title credits.
pub(crate) fn stream_want(t: &StreamTrack) -> Want {
    let channel = t.artists.first().map_or("", |a| a.name.as_str());
    let mut artists = nikgob_core::clip_title::credited_names(&t.title, channel);
    for a in &t.artists {
        if !a.name.is_empty() && !artists.contains(&a.name) {
            artists.push(a.name.clone());
        }
    }
    let titles = if t.kind == TrackKind::Song { vec![t.title.clone()] } else { better_covers::clip_titles(&t.title, channel) };
    let album = t.album.as_ref().map_or("", |a| a.title.as_str());
    Want { titles, artists, context: format!("{} {album}", t.title), song_ok: true, ..Want::default() }
}

impl Worker {
    /// The online metadata the lookups ask, when covers are fetched online.
    fn real_covers_meta(&self) -> Option<nikgob_metadata::Metadata> {
        self.import.as_ref().filter(|s| s.config().fetch_cover_online).and_then(|s| s.metadata())
    }

    /// `Command::RealCovers`: video thumbnails drawn on screen. What the
    /// cache has shows at once; the rest of the stream tracks the services
    /// gave is looked up.
    pub(crate) fn real_covers_wanted(&mut self, urls: Vec<String>) {
        if self.stream_covers.stopped || self.real_covers_meta().is_none() {
            return;
        }
        let now = now_ms();
        let mut hits = false;
        // Those drawn before their stream track was known go first, the
        // new ones after (pushed to the front, the latest drawn wins).
        let earlier: Vec<String> = self.stream_covers.unknown.drain(..).collect();
        for url in earlier.into_iter().chain(urls) {
            if self.stream_covers.seen.contains(&url) {
                continue;
            }
            // Not a video's thumbnail: nothing to look up, ever.
            let Some(video) = video_id(&url).map(str::to_owned) else {
                self.stream_covers.seen.insert(url);
                continue;
            };
            match self.lib.cache_get(&key(&video)) {
                Ok(Some((_, art))) if !art.is_empty() => {
                    self.stream_covers.seen.insert(url.clone());
                    self.stream_covers.found.insert(url, art);
                    hits = true;
                    continue;
                }
                Ok(Some((at, _))) if now - at < RETRY_MS => {
                    self.stream_covers.seen.insert(url);
                    continue;
                }
                // Asked again when it is drawn again.
                Err(_) => continue,
                _ => {}
            }
            let Some(t) = self.streaming.known.get(&RemoteId::new(Service::Youtube, video)).cloned() else {
                let c = &mut self.stream_covers;
                if !c.unknown.contains(&url) {
                    c.unknown.push_back(url);
                }
                while c.unknown.len() > QUEUE_MAX {
                    c.unknown.pop_front();
                }
                continue;
            };
            self.stream_covers.seen.insert(url.clone());
            // The library's own tracks are the library's sweep's.
            if self.library_track_of(&t).and_then(|id| self.lib.track_in_library(id).ok().flatten()).unwrap_or(false) {
                continue;
            }
            self.stream_covers.queue.push_front((url, t));
        }
        let c = &mut self.stream_covers;
        while c.queue.len() > QUEUE_MAX {
            if let Some((url, _)) = c.queue.pop_back() {
                c.seen.remove(&url);
            }
        }
        if hits {
            self.publish_real_covers();
        }
        self.real_covers_next();
    }

    /// Asks the next waiting thumbnail, unless one is on its way.
    fn real_covers_next(&mut self) {
        let c = &self.stream_covers;
        if c.busy || c.stopped || c.paused_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let Some(meta) = self.real_covers_meta() else { return };
        let Some((url, t)) = self.stream_covers.queue.pop_front() else { return };
        let Some(video) = video_id(&url).map(str::to_owned) else { return };
        let services: Vec<Arc<dyn StreamService>> =
            [Service::Deezer, Service::Youtube].into_iter().filter_map(|k| self.service(k)).collect();
        let want = stream_want(&t);
        self.stream_covers.busy = true;
        self.streaming.spawn_internal(async move {
            let result = match tokio::time::timeout(LOOKUP_TIMEOUT, better_covers::find(Some(&meta), &services, &want)).await {
                Ok(r) => r,
                Err(_) => Err("a lookup took too long".into()),
            };
            Internal::StreamCover(Found { url, video, result })
        });
    }

    /// A lookup answered: kept (a miss too), shown, the next one asked.
    pub(crate) fn real_cover_found(&mut self, f: Found) {
        self.stream_covers.busy = false;
        match f.result {
            Ok(art) => {
                self.stream_covers.failures = 0;
                self.stream_covers.paused_until = None;
                let art = art.unwrap_or_default();
                let _ = self.lib.cache_put(&key(&f.video), now_ms(), &art);
                if !art.is_empty() {
                    log::info!("art for the stream {}: {art}", f.video);
                    self.stream_covers.found.insert(f.url, art);
                    self.publish_real_covers();
                }
                self.real_covers_next();
            }
            Err(e) => {
                let c = &mut self.stream_covers;
                // Not kept: asked again when it is drawn again.
                c.seen.remove(&f.url);
                c.failures += 1;
                match rest_after(c.failures) {
                    Some(rest) => {
                        log::info!("covers of streams: {} skipped, resting {}s ({e})", f.video, rest.as_secs());
                        c.paused_until = Some(Instant::now() + rest);
                    }
                    None => {
                        log::info!("covers of streams: stopped for this run ({e})");
                        c.stopped = true;
                        c.queue.clear();
                        c.unknown.clear();
                    }
                }
            }
        }
    }

    fn publish_real_covers(&mut self) {
        let map = Arc::new(self.stream_covers.found.clone());
        self.publish(|s| s.real_covers = map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_core::streaming::ArtistRef;

    /// A timeout or a hiccup skips one thumbnail and rests a while, longer
    /// each time; only failures in a row stop the lookups for the run.
    #[test]
    fn lookups_rest_after_a_failure_and_stop_only_after_several() {
        assert_eq!(rest_after(1), Some(PAUSE));
        assert_eq!(rest_after(STOP_AFTER - 1), Some(PAUSE * (STOP_AFTER - 1)));
        assert_eq!(rest_after(STOP_AFTER), None);
        assert_eq!(rest_after(STOP_AFTER + 1), None);
    }

    #[test]
    fn a_thumbnail_names_its_video() {
        assert_eq!(video_id("https://i.ytimg.com/vi/35nV_M3asRs/hqdefault.jpg"), Some("35nV_M3asRs"));
        assert_eq!(video_id("https://i.ytimg.com/vi_webp/abc/maxresdefault.webp"), Some("abc"));
        assert_eq!(video_id("https://i.ytimg.com/vi/x?sqp=1"), Some("x"));
        assert_eq!(video_id("https://lh3.googleusercontent.com/abc=w120"), None);
        assert_eq!(video_id("https://i.ytimg.com/an/xyz/a.jpg"), None);
    }

    #[test]
    fn a_clip_is_looked_up_by_its_song_and_the_names_its_title_credits() {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, "v1"), "LiSA - \"ReawakeR\" feat. Felix (Official Video)");
        t.artists = vec![ArtistRef::new(None, "LiSA Official YouTube")];
        t.kind = TrackKind::Video;
        let w = stream_want(&t);
        assert_eq!(w.titles.first().map(String::as_str), Some("ReawakeR"));
        assert!(w.artists.iter().any(|a| a == "LiSA Official YouTube"));
        assert!(w.song_ok && w.album.is_none() && w.target.is_none());
        t.kind = TrackKind::Song;
        t.title = "Gurenge".into();
        assert_eq!(stream_want(&t).titles, vec!["Gurenge".to_owned()]);
    }
}
