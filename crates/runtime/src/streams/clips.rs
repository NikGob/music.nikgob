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

use nikgob_core::model::{EventContext, SourceKind, TrackId, score};
use nikgob_core::streaming::{ArtistRef, RemoteId, Service, StreamError, StreamTrack, TrackKind};
use nikgob_library::LikedClip;

use super::Msg;
use super::resolve::{ResolveQuery, pick_youtube};
use crate::Worker;
use crate::queue::Entry;

/// New videos of one like check whose songs are asked before the likes
/// are stored; more go to the repair.
pub(crate) const CONVERT_NOW: usize = 5;
/// Library cache key of what a liked video turned out to be.
pub(crate) fn clip_key(video: &str) -> String {
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
            // Unliked here a moment ago (replaced by its song by hand).
            .filter(|t| self.streaming.replace.echo(&t.id) != Some(false))
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

    /// Makes the library track of the clip `video` its `song`, and what
    /// points at the track follows (queue, wishlist item). Whether it did.
    fn convert_clip(&mut self, track: TrackId, video: &StreamTrack, song: &StreamTrack) -> bool {
        match self.lib.clip_to_song(track, &video.id, song) {
            Ok(into) => {
                log::info!("{}: the liked clip {} is the song {}", video.title, video.id.id, song.id.id);
                let now = nikgob_library::now_ms();
                let _ = self.lib.cache_put(&clip_key(&video.id.id), now, IS_SONG);
                if into != track {
                    self.queue.retarget(track, into);
                }
                // The clip's lyrics were for its audio: the song's are looked up.
                if let Some(svc) = self.import.as_mut() {
                    svc.lyrics_cache.remove(&track);
                    svc.lyrics_cache.remove(&into);
                }
                self.wish_track_changed(into);
                true
            }
            Err(e) => {
                log::warn!("turning the clip {} into its song: {e}", video.id.id);
                false
            }
        }
    }

    /// The clip's verdict is known and still fresh.
    fn clip_settled(&self, video: &str) -> bool {
        let ttl = self.streaming.schedule.miss_ttl.as_millis() as i64;
        let now = nikgob_library::now_ms();
        match self.lib.cache_get(&clip_key(video)).ok().flatten() {
            Some((_, v)) if v == IS_SONG || v == super::replace::MANUAL => true,
            Some((at, _)) => now - at < ttl,
            None => false,
        }
    }

    /// From the tick (the app in use): the next clip of the repair, when
    /// its turn has come.
    pub(crate) fn clips_tick(&mut self) {
        if std::mem::take(&mut self.streaming.clips.dirty) {
            self.after_like_change(crate::Held::Tracks(None), Vec::new());
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
                .filter(|c| !self.clip_settled(&c.video.id.id) && !self.replaced_by_hand(c.track))
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
                    // The likes' lists show it as the song.
                    self.streaming.clips.dirty |= self.convert_clip(clip.track, &clip.video, &song);
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

/// Library cache key: the misses of an older title cleaner were forgotten.
const MISSES_RETRIED: &str = "yt_clip_retry.v2";

impl Worker {
    /// Once: the clips the repair found no song for are asked again with
    /// the better title cleaner (`nikgob_core::clip_title`), at the
    /// repair's usual pace.
    pub(crate) fn retry_clip_misses_once(&mut self) {
        if self.lib.cache_get(MISSES_RETRIED).ok().flatten().is_some() {
            return;
        }
        match self.lib.cache_forget("yt_clip.", NO_SONG) {
            Ok(0) => {}
            Ok(n) => log::info!("youtube: {n} liked clips without a song are looked up again"),
            Err(e) => log::warn!("forgetting the clip misses: {e}"),
        }
        let _ = self.lib.cache_put(MISSES_RETRIED, nikgob_library::now_ms(), "");
    }
}

// ---------------------------------------------------------------------------
// Clips the app queued
// ---------------------------------------------------------------------------

/// How long a clip the app queued waits for its song before it plays as
/// it is.
const QUEUE_CLIP_WAIT: Duration = Duration::from_secs(4);
/// A clip whose song could not be asked (a failure, a rate limit, a wait
/// that ran out) is not asked again for this long.
const QUEUE_CLIP_HOLD: Duration = Duration::from_secs(10 * 60);
/// Clips held at most (the oldest go first).
const QUEUE_CLIP_HELD_MAX: usize = 64;

/// The songs of the clips a radio, the wave, autoplay, a mix or a guest
/// of «✦ С дополнением» queued: a music video YouTube Music's radios and
/// suggestions bring is asked for its song before it plays (the next
/// tracks ahead, and the one about to start waits a moment for it); the
/// song takes the clip's place ([`nikgob_library::Library::clip_to_song`]),
/// a clip without one plays as a last resort and is marked so. A video
/// uploaded by someone else («FACE - ЮМОРИСТ» of a lyrics channel) is
/// looked up as the song its title names.
#[derive(Default)]
pub(crate) struct QueueClips {
    /// Tracks whose clip's song is being asked.
    asking: HashSet<TrackId>,
    /// The queue entry that waits for its clip's song, and the video.
    waiting: Option<(u64, String)>,
    /// Videos not asked again for a while (see [`QUEUE_CLIP_HOLD`]).
    held: HashMap<String, Instant>,
}

impl QueueClips {
    fn hold(&mut self, video: &str) {
        self.held.retain(|_, at| at.elapsed() < QUEUE_CLIP_HOLD);
        if self.held.len() >= QUEUE_CLIP_HELD_MAX
            && let Some(oldest) = self.held.iter().min_by_key(|(_, at)| **at).map(|(v, _)| v.clone())
        {
            self.held.remove(&oldest);
        }
        self.held.insert(video.to_owned(), Instant::now());
    }

    fn holds(&self, video: &str) -> bool {
        self.held.get(video).is_some_and(|at| at.elapsed() < QUEUE_CLIP_HOLD)
    }
}

/// What the answer for a queued clip makes of it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ClipVerdict {
    /// Its song: it takes the clip's place.
    Song(StreamTrack),
    /// The video is a song itself.
    IsSong,
    /// No song: the clip plays, marked a clip.
    NoSong,
    /// Not known now (a failure, a rate limit): asked again later.
    Later,
}

/// The verdict of a song lookup for the clip `video`.
pub(crate) fn clip_verdict(video: &StreamTrack, r: &Result<Option<StreamTrack>, StreamError>) -> ClipVerdict {
    match r {
        Ok(Some(s)) if s.id == video.id => ClipVerdict::IsSong,
        Ok(Some(s)) if s.kind == TrackKind::Song && s.playable => ClipVerdict::Song(s.clone()),
        Ok(_) => ClipVerdict::NoSong,
        Err(_) => ClipVerdict::Later,
    }
}

/// The song a video uploaded by someone else names: «Artist - Title
/// (…)» on a channel that is not the artist's. `None` for a title that
/// names no other artist.
pub(crate) fn reupload_query(video: &StreamTrack) -> Option<ResolveQuery> {
    let channel = video.artists.first().map_or("", |a| a.name.as_str());
    let (head, tail) = [" - ", " – ", " — "].iter().find_map(|d| video.title.split_once(d))?;
    let head = head.trim();
    if head.is_empty() || nikgob_core::names::same_name(head, channel) {
        return None;
    }
    // What the uploader adds in brackets («(ТЕКСТ И ПЕСНЯ)», «[Lyrics]»).
    let bare = tail.split(['(', '[', '【', '|']).next().unwrap_or(tail).trim();
    let title = nikgob_core::clip_title::display_title(if bare.is_empty() { tail } else { bare }, head);
    if nikgob_core::clip_title::fold(&title).is_empty() {
        return None;
    }
    Some(ResolveQuery {
        title,
        artist: head.to_owned(),
        duration_ms: None,
        isrc: None,
        exclude: Vec::new(),
        aliases: Vec::new(),
    })
}

/// The song among `hits` for the reupload `video` asked as `q`: a song of
/// YouTube Music by the artist the title names, as long as the video or
/// up to a minute shorter (an intro), never much longer.
pub(crate) fn reupload_song(hits: &[StreamTrack], q: &ResolveQuery, video: &StreamTrack) -> Option<StreamTrack> {
    let songs: Vec<StreamTrack> = hits
        .iter()
        .filter(|h| h.kind == TrackKind::Song && h.id != video.id)
        .filter(|h| match (h.duration_ms, video.duration_ms) {
            (Some(song), Some(clip)) => song <= clip + 10_000 && song + 60_000 >= clip,
            _ => true,
        })
        .cloned()
        .collect();
    pick_youtube(&songs, q)
}

/// The song of a clip: the one YouTube Music links to it or finds by its
/// title ([`StreamService::song_for_video`]), else, for a reupload, the
/// song its title names (one search of songs).
///
/// [`StreamService::song_for_video`]: nikgob_core::streaming::StreamService::song_for_video
async fn song_of_clip(
    yt: &dyn nikgob_core::streaming::StreamService,
    video: &StreamTrack,
) -> Result<Option<StreamTrack>, StreamError> {
    if let Some(song) = yt.song_for_video(video).await? {
        return Ok(Some(song));
    }
    let Some(q) = reupload_query(video) else { return Ok(None) };
    let r = yt.search_kind(&q.text(), nikgob_core::streaming::SearchKind::Songs).await?;
    Ok(reupload_song(&r.tracks, &q, video))
}

/// The track may become its clip's song: a track the app picked and the
/// user never kept, as the library says now (not liked, in no playlist,
/// not in the library in any way, not replaced by hand). A library track
/// is the user's: it is never merged or rewritten from a queue.
pub(crate) fn queue_clip_convertible(lib: &nikgob_library::Library, track: TrackId) -> bool {
    lib.track_in_library(track).ok().flatten() == Some(false)
        && lib.is_liked(track).ok() == Some(false)
        && lib.cache_get(&super::replace::replaced_key(track)).ok().flatten().is_none()
}

/// What the end of a clip's wait for its song does with the entry that
/// waited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterWait {
    /// Nothing: another entry plays (or none waits).
    Nothing,
    /// It starts.
    Start,
    /// It is prepared and stays paused (the user paused while it waited).
    StartPaused,
}

/// [`AfterWait`] for the waiting entry: still the one playing and nothing
/// started for it, and whether the user paused meanwhile.
pub(crate) fn after_wait(waits: bool, status: nikgob_view::Status) -> AfterWait {
    match (waits, status) {
        (false, _) | (true, nikgob_view::Status::Stopped) => AfterWait::Nothing,
        (true, nikgob_view::Status::Paused) => AfterWait::StartPaused,
        (true, _) => AfterWait::Start,
    }
}

impl Worker {
    /// The app picked the queue entry `e` (a radio, the wave, autoplay, a
    /// mix of the home, a guest of «✦ С дополнением»), not the user.
    fn app_queued(&mut self, e: Entry) -> bool {
        matches!(self.play_context, EventContext::HomeSection | EventContext::Radio)
            || self.autoplay_owns(e.id)
            || self.radio_origin(e.id).is_some()
            || self.recs.plus.guests().contains(&e.id)
    }

    /// The video `e` would play when it is a clip the app queued whose song
    /// was not asked yet (see [`QueueClips`]); `None` for anything else, a
    /// track of the library, or while YouTube Music cannot be asked.
    pub(crate) fn queued_clip(&mut self, e: Entry) -> Option<StreamTrack> {
        self.service(Service::Youtube)?;
        if self.pause_left(Service::Youtube).is_some() || !self.app_queued(e) {
            return None;
        }
        let src = self.pick_source(e, &[])?;
        if src.kind != SourceKind::Youtube || src.quality_score != score::STREAM_YOUTUBE_CLIP {
            return None;
        }
        if self.streaming.queue_clips.holds(&src.locator) || self.clip_settled(&src.locator) {
            return None;
        }
        // A clip of the library (liked, in a playlist) is the user's: the
        // likes' repair asks for liked ones. Read live, not from the recs'
        // snapshot (it may be older than a like a moment ago).
        if !queue_clip_convertible(&self.lib, e.track) {
            return None;
        }
        let remote = RemoteId::new(Service::Youtube, src.locator.clone());
        if let Some(t) = self.streaming.known.get(&remote) {
            return Some(StreamTrack { kind: TrackKind::Video, ..t.clone() });
        }
        let t = self.lib.get_track(e.track).ok().flatten()?;
        let mut video = StreamTrack::new(remote, t.title.clone());
        video.artists = t.artist_credit.0.iter().map(|p| ArtistRef::new(None, p.name.clone())).collect();
        video.duration_ms = t.duration_ms.map(u64::from);
        video.kind = TrackKind::Video;
        Some(video)
    }

    /// Asks for the song of the clip `video` that `track` plays (once at a
    /// time per track). Whether it is being asked.
    fn ask_queue_clip(&mut self, track: TrackId, video: StreamTrack) -> bool {
        if self.streaming.queue_clips.asking.contains(&track) {
            return true;
        }
        let Some(yt) = self.service(Service::Youtube) else { return false };
        self.streaming.queue_clips.asking.insert(track);
        log::info!("{track}: asking for the song of the clip {}", video.id.id);
        self.streaming.spawn(async move {
            let result = song_of_clip(yt.as_ref(), &video).await;
            Msg::QueueClipSong { track, video: Box::new(video), result }
        });
        true
    }

    /// The next tracks of the queue that are clips the app queued: their
    /// songs are asked, one at a time.
    pub(crate) fn clips_ahead(&mut self) {
        if !self.streaming.queue_clips.asking.is_empty() || self.service(Service::Youtube).is_none() {
            return;
        }
        let mut next: Vec<Entry> = self.queue.peek_auto_next().into_iter().collect();
        next.extend(self.queue.upcoming().iter().skip(1).take(1).copied());
        let current = self.queue.current().map(|c| c.track);
        for e in next {
            if Some(e.track) == current {
                continue;
            }
            if let Some(video) = self.queued_clip(e) {
                self.ask_queue_clip(e.track, video);
                return;
            }
        }
    }

    /// The next track is a clip the app queued whose song is not known yet:
    /// it is not handed to the engine ahead (it starts through
    /// [`Self::wait_for_clip_song`]); its song is asked.
    pub(crate) fn clip_not_ready(&mut self, next: Entry) -> bool {
        let Some(video) = self.queued_clip(next) else { return false };
        self.ask_queue_clip(next.track, video)
    }

    /// `e` is about to start: a clip the app queued waits (Buffering) a
    /// moment for its song; the song plays when it comes in time, else the
    /// clip. False when it does not wait.
    pub(crate) fn wait_for_clip_song(&mut self, e: Entry) -> bool {
        let Some(video) = self.queued_clip(e) else { return false };
        let id = video.id.id.clone();
        if !self.ask_queue_clip(e.track, video) {
            return false;
        }
        self.streaming.queue_clips.waiting = Some((e.id, id));
        self.playback.stop();
        self.playing = Some(e);
        self.playing_source = None;
        self.restarted = None;
        self.status = nikgob_view::Status::Buffering;
        self.position = Duration::ZERO;
        self.position_at = Instant::now();
        self.rate = 1.0;
        self.publish_now_playing();
        let entry = e.id;
        self.streaming.spawn(async move {
            tokio::time::sleep(QUEUE_CLIP_WAIT).await;
            Msg::QueueClipWait(entry)
        });
        true
    }

    /// The entry that waits for its clip's song, if it still waits: it
    /// plays now (the waiting is over either way).
    fn clip_wait_over(&mut self) {
        let Some((entry, _)) = self.streaming.queue_clips.waiting.take() else { return };
        let waits = self.playing.is_some_and(|p| p.id == entry) && self.playing_source.is_none();
        // The queue's own entry: a song that took a merged track's place
        // is retargeted there.
        let Some(e) = self.queue.current().filter(|c| c.id == entry) else { return };
        match after_wait(waits, self.status) {
            AfterWait::Nothing => {}
            AfterWait::Start => self.start(e),
            AfterWait::StartPaused => {
                // Paused while it waited: ready to go on, still paused.
                self.start(e);
                if self.playing.is_some_and(|p| p.id == entry) && self.playing_source.is_some() {
                    self.playback.pause();
                    self.position = self.position_now();
                    self.status = nikgob_view::Status::Paused;
                    self.publish_player();
                }
            }
        }
    }

    /// The song of a waiting clip did not come in time: the clip plays (its
    /// answer still turns it into its song for the next time).
    pub(crate) fn queue_clip_wait(&mut self, entry: u64) {
        let Some((waiting, video)) = self.streaming.queue_clips.waiting.clone() else { return };
        if waiting != entry {
            return;
        }
        log::info!("the song of the clip {video} did not come in time: the clip plays");
        self.streaming.queue_clips.hold(&video);
        self.clip_wait_over();
    }

    /// The answer for a clip the app queued (see [`QueueClips`]).
    pub(crate) fn queue_clip_song(&mut self, track: TrackId, video: StreamTrack, result: Result<Option<StreamTrack>, StreamError>) {
        self.streaming.queue_clips.asking.remove(&track);
        let now = nikgob_library::now_ms();
        let key = clip_key(&video.id.id);
        // Under the user's ears: its song comes the next time.
        let playing = self.playing.is_some_and(|e| e.track == track) && self.playing_source.is_some();
        match clip_verdict(&video, &result) {
            ClipVerdict::Song(song) => {
                self.streaming.learn(std::slice::from_ref(&song));
                // Still the clip's track, and still no track of the user's
                // (liked or put in a playlist while the song was asked).
                let mine = self.lib.track_by_remote(&video.id).ok().flatten() == Some(track) && queue_clip_convertible(&self.lib, track);
                if playing || !mine || !self.convert_clip(track, &video, &song) {
                    self.streaming.queue_clips.hold(&video.id.id);
                }
            }
            ClipVerdict::IsSong => {
                // Kept as the song it is (its source no longer ranks as a clip).
                if let Ok(Some(song)) = &result
                    && let Err(e) = self.lib.upsert_stream_track(song)
                {
                    log::warn!("keeping {} as a song: {e}", video.id.id);
                }
                let _ = self.lib.cache_put(&key, now, IS_SONG);
            }
            ClipVerdict::NoSong => {
                log::info!("{track}: the clip {} has no song: it plays as a clip", video.id.id);
                if let Err(e) = self.lib.mark_clip(&video.id) {
                    log::warn!("marking the clip {}: {e}", video.id.id);
                }
                let _ = self.lib.cache_put(&key, now, NO_SONG);
            }
            ClipVerdict::Later => {
                if let Err(e) = &result {
                    log::info!("youtube: the song of the clip {}: {e}", video.id.id);
                }
                self.streaming.queue_clips.hold(&video.id.id);
            }
        }
        if self.streaming.queue_clips.waiting.as_ref().is_some_and(|(_, v)| *v == video.id.id) {
            self.clip_wait_over();
        } else {
            self.preload_next();
        }
        self.resolve_ahead();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str, title: &str, channel: &str, secs: u64) -> StreamTrack {
        let mut t = StreamTrack::new(RemoteId::new(Service::Youtube, id), title);
        t.artists = vec![ArtistRef::new(None, channel)];
        t.duration_ms = Some(secs * 1000);
        t.kind = TrackKind::Video;
        t
    }

    fn song(id: &str, title: &str, artist: &str, secs: u64) -> StreamTrack {
        StreamTrack { kind: TrackKind::Song, ..video(id, title, artist, secs) }
    }

    /// A clip with a song becomes the song; without one it stays a clip
    /// (marked); a failure decides nothing.
    #[test]
    fn a_clip_with_a_song_becomes_the_song() {
        let clip = video("34Na4j8AVgA", "Starboy ft. Daft Punk (Official Video) (feat. Daft Punk)", "The Weeknd", 274);
        let starboy = song("starboy", "Starboy", "The Weeknd", 230);
        assert_eq!(clip_verdict(&clip, &Ok(Some(starboy.clone()))), ClipVerdict::Song(starboy));
        assert_eq!(clip_verdict(&clip, &Ok(None)), ClipVerdict::NoSong);
        assert_eq!(clip_verdict(&clip, &Ok(Some(song("34Na4j8AVgA", "Starboy", "The Weeknd", 274)))), ClipVerdict::IsSong);
        // Another video is no song.
        assert_eq!(clip_verdict(&clip, &Ok(Some(video("other", "Starboy (Live)", "The Weeknd", 250)))), ClipVerdict::NoSong);
        let mut gone = song("gone", "Starboy", "The Weeknd", 230);
        gone.playable = false;
        assert_eq!(clip_verdict(&clip, &Ok(Some(gone))), ClipVerdict::NoSong);
        let limited: Result<Option<StreamTrack>, StreamError> = Err(StreamError::RateLimited { retry_after: None });
        assert_eq!(clip_verdict(&clip, &limited), ClipVerdict::Later);
    }

    /// A clip of the user's (liked, in a playlist, replaced by hand) is
    /// never turned into a song from a queue; a seen one is.
    #[test]
    fn only_a_clip_the_user_never_kept_becomes_its_song() {
        let lib = nikgob_library::Library::open_in_memory().unwrap();
        let seen = lib.upsert_stream_track(&video("seen", "Song (Official Video)", "Band", 200)).unwrap();
        assert!(queue_clip_convertible(&lib, seen));
        let liked = lib.upsert_stream_track(&video("liked", "Other (Official Video)", "Band", 200)).unwrap();
        lib.like(liked).unwrap();
        assert!(!queue_clip_convertible(&lib, liked), "liked");
        let listed = lib.upsert_stream_track(&video("listed", "Third (Official Video)", "Band", 200)).unwrap();
        let p = lib.create_playlist("Mine").unwrap();
        lib.playlist_add(p.id, listed).unwrap();
        assert!(!queue_clip_convertible(&lib, listed), "in a playlist");
        lib.cache_put(&super::super::replace::replaced_key(seen), 1, "").unwrap();
        assert!(!queue_clip_convertible(&lib, seen), "replaced by hand");
        assert!(!queue_clip_convertible(&lib, TrackId::new()), "unknown");
    }

    /// Paused while the clip waited: it is prepared and stays paused; an
    /// entry that no longer waits is left alone.
    #[test]
    fn a_pause_during_the_wait_is_kept() {
        use nikgob_view::Status;
        assert_eq!(after_wait(true, Status::Buffering), AfterWait::Start);
        assert_eq!(after_wait(true, Status::Paused), AfterWait::StartPaused);
        assert_eq!(after_wait(true, Status::Stopped), AfterWait::Nothing);
        assert_eq!(after_wait(false, Status::Buffering), AfterWait::Nothing);
        assert_eq!(after_wait(false, Status::Paused), AfterWait::Nothing);
    }

    /// A lyrics channel's upload is looked up as the song its title names:
    /// FACE's «ЮМОРИСТ», as long as the upload, by FACE.
    #[test]
    fn a_reupload_is_looked_up_as_the_song_it_names() {
        let up = video("up", "FACE - ЮМОРИСТ (ТЕКСТ И ПЕСНЯ)", "RusLyrics", 144);
        let q = reupload_query(&up).expect("a reupload");
        assert_eq!((q.artist.as_str(), q.title.as_str()), ("FACE", "ЮМОРИСТ"));
        assert_eq!(q.duration_ms, None);
        let hits = [
            video("clip", "FACE - ЮМОРИСТ", "FACE", 150),
            song("other", "ЮМОРИСТ", "Кто-то ещё", 144),
            song("long", "ЮМОРИСТ", "FACE", 200),
            song("face", "ЮМОРИСТ", "FACE", 143),
        ];
        assert_eq!(reupload_song(&hits, &q, &up).map(|s| s.id.id), Some("face".into()));
        assert_eq!(reupload_song(&hits[..3], &q, &up), None, "no song of FACE as long as the upload");
        // The artist's own channel names no one else; a plain title names no one.
        assert!(reupload_query(&video("own", "FACE - ЮМОРИСТ", "FACE", 144)).is_none());
        assert!(reupload_query(&video("plain", "ЮМОРИСТ", "RusLyrics", 144)).is_none());
    }
}
