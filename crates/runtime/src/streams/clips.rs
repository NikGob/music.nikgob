//! Liked YouTube clips become their songs (docs/stage3.md §7 «Клипы в
//! лайках»).
//!
//! A video liked on YouTube (an official clip: intro, outro, another mix)
//! is a poor library track: no album, no cover, lyrics that drift. Its song
//! is asked of the service ([`StreamService::song_for_video`]) and the
//! library track becomes that song while the clip's id stays the like's
//! identity ([`nikgob_library::Library::clip_to_song`]); a clip without a
//! song stays one and is shown as «клип» ([`nikgob_library::Library::mark_clip`]).
//!
//! * **New likes**: a like check that brings up to [`CONVERT_NOW`] new
//!   videos asks for their songs before it stores the likes (so the
//!   track, its wishlist item and its first play are the song's); more
//!   than that (the first import of a few hundred likes) go to the repair.
//! * **The repair**: the likes already in the library that are still
//!   clips ([`nikgob_library::Library::liked_youtube_clips`]), one at a time,
//!   [`super::StreamSchedule::clip_gap`] apart (± jitter), only while the
//!   app is in use, never while the service is paused (a rate limit stops
//!   it until the pause ends, then it goes on where it was). Each answer is
//!   kept in the library cache: a song is never asked again, a clip without
//!   one not for [`super::StreamSchedule::miss_ttl`] (5 days), so a restart
//!   asks nothing twice. A track that plays right now waits for its end.
//!
//! [`StreamService::song_for_video`]: nikgob_core::streaming::StreamService::song_for_video

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use nikgob_core::streaming::{Service, StreamError, StreamTrack, TrackKind};
use nikgob_library::LikedClip;

use super::Msg;
use crate::Worker;

/// New videos of one like check whose songs are asked before the likes
/// are stored; more go to the repair.
pub(crate) const CONVERT_NOW: usize = 5;
/// Library cache key of what a liked video turned out to be.
fn clip_key(video: &str) -> String {
    format!("yt_clip.{video}")
}
/// The cached answers.
const IS_SONG: &str = "song";
const NO_SONG: &str = "clip";

/// The repair's state.
#[derive(Default)]
pub(crate) struct ClipRepair {
    queue: VecDeque<LikedClip>,
    /// A lookup is on its way.
    busy: bool,
    /// No lookup starts before this (spacing, a rate-limit pause).
    next_at: Option<Instant>,
    /// When the queue was last filled from the library (`None`: fill it
    /// at the next tick).
    filled_at: Option<Instant>,
    /// Converted since the library lists were last refreshed.
    dirty: bool,
}

/// What a like check's new videos turned out to be: the song of each
/// video id that has one.
pub(crate) type Songs = HashMap<String, StreamTrack>;

impl Worker {
    /// The new YouTube videos among `liked` (not known yet), if few enough
    /// to ask for their songs before the likes are stored.
    pub(crate) fn videos_to_convert(&self, service: Service, liked: &[StreamTrack]) -> Vec<StreamTrack> {
        if service != Service::Youtube {
            return Vec::new();
        }
        let known: HashSet<String> = self
            .lib
            .remote_likes(service)
            .unwrap_or_default()
            .into_iter()
            .map(|k| k.remote.id)
            .collect();
        let new: Vec<StreamTrack> = liked
            .iter()
            .filter(|t| t.kind == TrackKind::Video && !known.contains(&t.id.id))
            .cloned()
            .collect();
        if new.len() > CONVERT_NOW {
            log::info!("{service}: {} new liked clips: their songs come in the background", new.len());
            return Vec::new();
        }
        new
    }

    /// Asks for the songs of `videos` (one after the other), then stores the
    /// likes with them ([`Msg::LikesConverted`]). A rate limit stops the
    /// asking; the rest stay clips for the repair.
    pub(crate) fn convert_then_apply(&mut self, service: Service, liked: Vec<StreamTrack>, videos: Vec<StreamTrack>, partial: bool) {
        let Some(svc) = self.service(service) else {
            self.apply_remote_likes(service, &liked, false, partial);
            return;
        };
        self.streaming.likes.entry(service).or_default().pulling = true;
        log::debug!("{service}: asking for the songs of {} new liked clips", videos.len());
        self.streaming.spawn(async move {
            let mut songs = Songs::new();
            let mut misses = Vec::new();
            for v in &videos {
                match svc.song_for_video(v).await {
                    Ok(Some(s)) if s.id != v.id => {
                        songs.insert(v.id.id.clone(), s);
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => misses.push(v.id.id.clone()),
                    Err(e) => {
                        log::info!("{service}: the song of {}: {e}", v.id.id);
                        if matches!(e, StreamError::RateLimited { .. }) {
                            break;
                        }
                    }
                }
            }
            Msg::LikesConverted {
                service,
                liked,
                songs,
                misses,
                partial,
            }
        });
    }

    /// The songs of a like check's new clips came: the likes are stored,
    /// each clip that has a song as the song (its track, wishlist item and
    /// first play are the song's from the start).
    pub(crate) fn likes_converted(&mut self, service: Service, liked: &[StreamTrack], songs: &Songs, misses: &[String], partial: bool) {
        self.streaming.likes.entry(service).or_default().pulling = false;
        self.streaming.learn(songs.values());
        let now = nikgob_library::now_ms();
        for (video, song) in songs {
            log::info!("{service}: the new liked clip {video} is the song {}", song.id.id);
            let _ = self.lib.cache_put(&clip_key(video), now, IS_SONG);
        }
        for video in misses {
            let _ = self.lib.cache_put(&clip_key(video), now, NO_SONG);
        }
        self.apply_remote_likes_with(service, liked, false, partial, songs);
        // Whatever stayed a clip (a rate limit) is the repair's.
        self.streaming.clips.filled_at = None;
    }

    /// Makes the library track of the liked clip `video` its `song`, and
    /// what points at the track follows (queue, wishlist item).
    fn convert_clip(&mut self, track: nikgob_core::model::TrackId, video: &StreamTrack, song: &StreamTrack) {
        match self.lib.clip_to_song(track, &video.id, song) {
            Ok(into) => {
                log::info!("{}: the liked clip {} is the song {}", video.title, video.id.id, song.id.id);
                let now = nikgob_library::now_ms();
                let _ = self.lib.cache_put(&clip_key(&video.id.id), now, IS_SONG);
                if into != track {
                    self.queue.retarget(track, into);
                }
                self.wish_track_changed(into);
                self.streaming.clips.dirty = true;
            }
            Err(e) => log::warn!("turning the clip {} into its song: {e}", video.id.id),
        }
    }

    /// The clip's verdict is known and still fresh.
    fn clip_settled(&self, video: &str) -> bool {
        let ttl = self.streaming.schedule.miss_ttl.as_millis() as i64;
        let now = nikgob_library::now_ms();
        match self.lib.cache_get(&clip_key(video)).ok().flatten() {
            Some((_, v)) if v == IS_SONG => true,
            Some((at, _)) => now - at < ttl,
            None => false,
        }
    }

    /// From the tick (the app in use): the next clip of the repair, when
    /// its turn has come.
    pub(crate) fn clips_tick(&mut self) {
        if std::mem::take(&mut self.streaming.clips.dirty) {
            self.after_like_change();
        }
        let c = &self.streaming.clips;
        if c.busy || c.next_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let Some(yt) = self.service(Service::Youtube) else { return };
        if let Some(left) = self.pause_left(Service::Youtube) {
            self.streaming.clips.next_at = Some(Instant::now() + left + super::schedule::wake_jitter());
            return;
        }
        if self.streaming.clips.queue.is_empty() {
            let every = self.streaming.schedule.clip_refill;
            if self.streaming.clips.filled_at.is_some_and(|t| t.elapsed() < every) {
                return;
            }
            let clips: Vec<LikedClip> = self
                .lib
                .liked_youtube_clips()
                .unwrap_or_default()
                .into_iter()
                .filter(|c| !self.clip_settled(&c.video.id.id))
                .collect();
            if !clips.is_empty() {
                log::info!("youtube: {} liked clips to look up songs for", clips.len());
            }
            self.streaming.clips.queue = clips.into();
            self.streaming.clips.filled_at = Some(Instant::now());
        }
        let playing = self.playing.map(|e| e.track);
        let Some(next) = self.streaming.clips.queue.pop_front() else { return };
        if Some(next.track) == playing {
            // Not under the user's ears: after the others (or the next fill).
            if !self.streaming.clips.queue.is_empty() {
                self.streaming.clips.queue.push_back(next);
            }
            return;
        }
        self.streaming.clips.busy = true;
        self.streaming.spawn(async move {
            let result = yt.song_for_video(&next.video).await;
            Msg::ClipSong { clip: Box::new(next), result }
        });
    }

    /// The repair's answer for one clip.
    pub(crate) fn clip_song(&mut self, clip: LikedClip, r: Result<Option<StreamTrack>, StreamError>) {
        let gap = self.streaming.schedule.jittered(self.streaming.schedule.clip_gap);
        let c = &mut self.streaming.clips;
        c.busy = false;
        c.next_at = Some(Instant::now() + gap);
        let now = nikgob_library::now_ms();
        let key = clip_key(&clip.video.id.id);
        match r {
            Ok(Some(song)) if song.id == clip.video.id => {
                // A song already (an old like without its album).
                let _ = self.lib.cache_put(&key, now, IS_SONG);
            }
            Ok(Some(song)) => {
                self.streaming.learn(std::slice::from_ref(&song));
                // Still the clip's track (not unliked or merged meanwhile).
                if self.lib.track_by_remote(&clip.video.id).ok().flatten() == Some(clip.track) {
                    self.convert_clip(clip.track, &clip.video, &song);
                }
            }
            Ok(None) => {
                if self.lib.mark_clip(&clip.video.id).unwrap_or(false) {
                    self.streaming.clips.dirty = true;
                }
                let _ = self.lib.cache_put(&key, now, NO_SONG);
            }
            Err(StreamError::RateLimited { retry_after }) => {
                // Not a miss: this clip again once the pause is over.
                let wait = retry_after
                    .or_else(|| self.pause_left(Service::Youtube))
                    .unwrap_or(self.streaming.schedule.rate_limit_unknown);
                log::info!("youtube: clip lookups rate limited: on again in {} s", wait.as_secs());
                let c = &mut self.streaming.clips;
                c.queue.push_front(clip);
                c.next_at = Some(Instant::now() + wait + super::schedule::wake_jitter());
            }
            Err(e) => {
                // A failure: asked again at a later fill, not now.
                log::info!("youtube: the song of {}: {e}", clip.video.id.id);
                self.streaming.clips.next_at = Some(Instant::now() + gap + Duration::from_secs(30));
            }
        }
    }
}
