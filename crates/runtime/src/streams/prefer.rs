//! The service the user prefers plays first.
//!
//! The sources of a track rank local > Spotify > YouTube Music (see
//! [`nikgob_core::model::SourceKind::priority`]; there is no setting of
//! its own for the order of the services). A song known only from YouTube
//! Music (a like, a radio, a search) used to play from there even with
//! Spotify connected, and only its next play was Spotify's. Now, while
//! Spotify gives sound and does not ask to wait, a track about to start
//! from YouTube Music, or about to be looked up there because nothing of
//! it plays, waits (Buffering) at most [`super::StreamSchedule::prefer_wait`]
//! for its Spotify copy (ISRC first, then "artist title", as
//! [`super::resolve`] looks up): found in time, Spotify plays; not found,
//! a rate limit, or no answer in time, it plays what it has (or is looked
//! up on YouTube Music as before). A late answer still joins the track for
//! the next time. The next track of the queue is asked ahead, one at a
//! time, so it usually starts from Spotify without waiting. A miss is kept
//! as [`super::resolve`] keeps its misses (days), so nothing is asked twice.
//!
//! Nothing of this runs on the player loop but the bookkeeping: the lookup
//! and the wait are the streams runtime's.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use nikgob_core::model::{SourceKind, TrackId};
use nikgob_core::streaming::{Service, StreamTrack};

use super::Msg;
use super::clips::{AfterWait, after_wait};
use super::resolve::{Lookup, PLAYED_MISS_TTL, look_up, miss_key};
use crate::Worker;
use crate::queue::Entry;

/// Lookups of the preferred copy running at once, at most (the track
/// about to start and the next one).
const ASKING_MAX: usize = 2;

#[derive(Default)]
pub(crate) struct PreferState {
    /// Tracks whose preferred copy is being looked up.
    asking: HashSet<TrackId>,
    /// The queue entry that waits for its preferred copy.
    waiting: Option<u64>,
    /// The entry that waited already: it plays what it has now.
    passed: Option<u64>,
}

/// The answer of a lookup of the preferred copy.
pub(crate) struct PreferMsg {
    track: TrackId,
    via: Service,
    found: Lookup,
    filled: Option<Box<StreamTrack>>,
}

impl Worker {
    /// The service whose copy `e` should wait for before it starts: Spotify
    /// while it gives sound and does not ask to wait, when the best source
    /// of `e` that plays now is a stream ranked below it (YouTube Music),
    /// or when nothing plays and it would be looked up on YouTube Music;
    /// the track has no copy there yet and did not miss there lately.
    fn preferred_for(&mut self, e: Entry) -> Option<Service> {
        let via = Service::Spotify;
        if !self.has_audio(via) || self.pause_left(via).is_some() {
            return None;
        }
        let ranked_below = match self.pick_source(e, &[]) {
            Some(src) => src.kind.service().is_some() && src.kind.priority() > SourceKind::Spotify.priority(),
            // Nothing plays now: a lookup on YouTube Music would follow.
            None => self.play_lookup(e.track, PLAYED_MISS_TTL) == Some(Service::Youtube),
        };
        if !ranked_below {
            return None;
        }
        let ttl = self.streaming.schedule.miss_ttl;
        self.can_resolve_on(e.track, ttl, via).then_some(via)
    }

    /// The preferred copy of `track` is being looked up.
    pub(crate) fn prefer_asking(&self, track: TrackId) -> bool {
        self.streaming.prefer.asking.contains(&track)
    }

    /// Looks up the copy of `track` on `via` (once at a time). Whether it
    /// is being looked up.
    fn ask_preferred(&mut self, track: TrackId, via: Service) -> bool {
        if self.streaming.prefer.asking.contains(&track) {
            return true;
        }
        if self.streaming.prefer.asking.len() >= ASKING_MAX {
            return false;
        }
        let Some(svc) = self.service(via) else { return false };
        let Some((q, deezer)) = self.lookup_of(track) else { return false };
        self.streaming.prefer.asking.insert(track);
        log::info!("looking up {} on {} before it plays", q.text(), via.display_name());
        self.streaming.spawn(async move {
            let (found, filled) = look_up(svc, q, deezer).await;
            Msg::Preferred(PreferMsg { track, via, found, filled })
        });
        true
    }

    /// `e` is about to start: it waits (Buffering) a moment for its copy
    /// on the service the user prefers. False when it does not wait.
    pub(crate) fn wait_for_preferred(&mut self, e: Entry) -> bool {
        if self.streaming.prefer.passed == Some(e.id) {
            return false;
        }
        let Some(via) = self.preferred_for(e) else { return false };
        if !self.ask_preferred(e.track, via) {
            return false;
        }
        self.streaming.prefer.waiting = Some(e.id);
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
        let wait = self.streaming.schedule.prefer_wait;
        self.streaming.spawn(async move {
            tokio::time::sleep(wait).await;
            Msg::PreferWait(entry)
        });
        true
    }

    /// The next track of the queue: its preferred copy is looked up ahead
    /// (one at a time), so it starts from there without waiting.
    pub(crate) fn prefer_ahead(&mut self) {
        if !self.streaming.prefer.asking.is_empty() {
            return;
        }
        let Some(next) = self.queue.peek_auto_next() else { return };
        if self.queue.current().is_some_and(|c| c.track == next.track) {
            return;
        }
        // A clip the app queued becomes its song first.
        if self.queued_clip(next).is_some() {
            return;
        }
        if let Some(via) = self.preferred_for(next) {
            self.ask_preferred(next.track, via);
        }
    }

    /// The wait of `entry` for its preferred copy ran out: it plays what it
    /// has (the lookup goes on for the next time).
    pub(crate) fn prefer_wait_over(&mut self, entry: u64) {
        if self.streaming.prefer.waiting != Some(entry) {
            return;
        }
        log::info!("the preferred copy did not come in time: playing what there is");
        self.prefer_go_on(entry);
    }

    /// The entry that waited for its preferred copy starts now, whatever
    /// it plays; nothing when it no longer waits.
    fn prefer_go_on(&mut self, entry: u64) {
        self.streaming.prefer.waiting = None;
        let waits = self.playing.is_some_and(|p| p.id == entry) && self.playing_source.is_none();
        let Some(e) = self.queue.current().filter(|c| c.id == entry) else { return };
        self.streaming.prefer.passed = Some(entry);
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

    /// A lookup of a preferred copy answered: a hit joins the track, a miss
    /// is kept; the track that waits for it starts.
    pub(crate) fn preferred(&mut self, m: PreferMsg) {
        let PreferMsg { track, via, found, filled } = m;
        self.streaming.prefer.asking.remove(&track);
        if let Some(full) = &filled {
            self.isrc_filled(track, full);
        }
        match found {
            Ok(Some(hit)) => {
                log::info!("{track} plays from {} as {}", via.display_name(), hit.id.id);
                self.streaming.learn(std::slice::from_ref(&hit));
                if let Err(e) = self.lib.attach_stream_source(track, &hit) {
                    log::warn!("attaching the {} copy of {track}: {e}", via.display_name());
                }
            }
            Ok(None) => {
                log::info!("{track}: no copy on {}", via.display_name());
                let now = nikgob_library::now_ms();
                self.streaming.misses.insert((track, via), now);
                let _ = self.lib.cache_put(&miss_key(track, via), now, "");
            }
            // A rate limit is no miss: the service's pause keeps it from
            // being asked meanwhile.
            Err(_) => log::info!("{track}: the {} lookup was rate limited", via.display_name()),
        }
        let waiting = self
            .streaming
            .prefer
            .waiting
            .filter(|w| self.playing.is_some_and(|p| p.id == *w && p.track == track));
        if let Some(entry) = waiting {
            self.prefer_go_on(entry);
        } else if self.queue.peek_auto_next().is_some_and(|n| n.track == track) {
            self.preload_next();
        }
        self.resolve_ahead();
    }
}
