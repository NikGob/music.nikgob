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
//! a rate limit, a failed request or no answer in time, it plays what it
//! has (or is looked up on YouTube Music as before). A late answer still
//! joins the track for the next time.
//!
//! * Only a definite «not there» (every request answered) is kept as a
//!   miss, for days ([`super::resolve`]'s misses); a failed request (the
//!   network, the login, a lookup that hangs) or a rate limit keeps the
//!   track from being asked of Spotify for [`RETRY_AFTER`] only, by
//!   [`super::resolve`]'s Spotify lookups too.
//! * The next track of the queue is asked ahead. Gapless, it is handed to
//!   the engine only once that answer is in (with the copy that answer
//!   leaves), so the engine never holds a source a moment from being
//!   replaced, and the source a join records is the one handed over. A
//!   join is never waited at: a next track whose answer is not in when the
//!   one before ends starts with what it has.
//! * A pause during the wait is kept through whatever comes next (the
//!   YouTube Music lookup of a track with nothing that plays); a track
//!   skipped or stopped meanwhile does not start when the answer comes.
//!
//! Nothing of this runs on the player loop but the bookkeeping: the lookup
//! and the wait are the streams runtime's.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use nikgob_core::model::{SourceKind, TrackId};
use nikgob_core::streaming::{Service, StreamTrack};

use super::Msg;
use super::clips::{AfterWait, after_wait};
use super::resolve::{Found, PLAYED_MISS_TTL, look_up, miss_key};
use crate::Worker;
use crate::queue::Entry;

/// Lookups of the preferred copy running at once, at most (the track
/// about to start and the next one).
const ASKING_MAX: usize = 2;
/// A lookup that failed or was rate limited is not asked again for this
/// long (a rate limit: as long as the service says, if longer).
pub(super) const RETRY_AFTER: Duration = Duration::from_secs(2 * 60);
/// A lookup still running after this is given up (failed): two that hang
/// would otherwise keep every later one from starting.
const LOOKUP_LIMIT: Duration = Duration::from_secs(30);
/// Tracks held back that way, at most (the oldest go first).
const HELD_MAX: usize = 64;

#[derive(Default)]
pub(crate) struct PreferState {
    /// Tracks whose preferred copy is being looked up.
    asking: HashSet<TrackId>,
    /// The queue entry that waits for its preferred copy.
    waiting: Option<u64>,
    /// The entry that waited already: it plays what it has now.
    passed: Option<u64>,
    /// A track starts at a join (the one before ended): it never waits.
    at_join: bool,
    /// The entry to keep paused when what it waits for comes (the user
    /// paused during an earlier wait of it).
    keep_paused: Option<u64>,
    /// Tracks whose lookup failed or was rate limited, until when they are
    /// not asked.
    held: HashMap<TrackId, Instant>,
}

impl PreferState {
    pub(super) fn hold(&mut self, track: TrackId, for_: Duration) {
        let now = Instant::now();
        self.held.retain(|_, until| *until > now);
        if self.held.len() >= HELD_MAX
            && let Some(first) = self.held.iter().min_by_key(|(_, until)| **until).map(|(t, _)| *t)
        {
            self.held.remove(&first);
        }
        self.held.insert(track, now + for_);
    }

    pub(super) fn holds(&self, track: TrackId) -> bool {
        self.held.get(&track).is_some_and(|until| *until > Instant::now())
    }
}

/// The answer of a lookup of the preferred copy.
pub(crate) struct PreferMsg {
    track: TrackId,
    via: Service,
    found: Found,
    filled: Option<Box<StreamTrack>>,
}

impl Worker {
    /// The service whose copy `e` should wait for before it starts: Spotify
    /// while it gives sound and does not ask to wait, when the best source
    /// of `e` that plays now is a stream ranked below it (YouTube Music),
    /// or when nothing plays and it would be looked up on YouTube Music;
    /// the track has no copy there yet, did not miss there lately, and its
    /// last lookup did not just fail.
    fn preferred_for(&mut self, e: Entry) -> Option<Service> {
        let via = Service::Spotify;
        if !self.has_audio(via) || self.pause_left(via).is_some() || self.streaming.prefer.holds(e.track) {
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

    /// The next entry's preferred copy is not settled yet: it is being
    /// looked up (asked now if it was not), so it is not handed to the
    /// engine ahead until that answer is in. False when no lookup could
    /// start (two still running): it is handed over as it is.
    pub(crate) fn prefer_pending(&mut self, next: Entry) -> bool {
        if self.prefer_asking(next.track) {
            return true;
        }
        match self.preferred_for(next) {
            Some(via) => self.ask_preferred(next.track, via),
            None => false,
        }
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
            // A lookup that hangs gives its place up (it counts as failed).
            let (found, filled) = tokio::time::timeout(LOOKUP_LIMIT, look_up(svc, q, deezer))
                .await
                .unwrap_or((Found::Failed, None));
            Msg::Preferred(PreferMsg { track, via, found, filled })
        });
        true
    }

    /// The one before ended and `e` comes up: it starts with what it has,
    /// never waiting for its preferred copy (asked ahead already).
    pub(crate) fn start_at_join(&mut self, e: Entry) {
        self.streaming.prefer.at_join = true;
        self.start(e);
        self.streaming.prefer.at_join = false;
    }

    /// `e` is about to start: it waits (Buffering) a moment for its copy
    /// on the service the user prefers. False when it does not wait.
    pub(crate) fn wait_for_preferred(&mut self, e: Entry) -> bool {
        if self.streaming.prefer.passed == Some(e.id) || self.streaming.prefer.at_join {
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
        // Paused during an earlier wait of it (a clip's): it stays paused.
        let paused = self.streaming.prefer.keep_paused.take() == Some(e.id);
        self.status = if paused { nikgob_view::Status::Paused } else { nikgob_view::Status::Buffering };
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

    /// An entry that the user paused while it waited starts now (`start`):
    /// it stays paused, also while it waits for something else next.
    pub(crate) fn start_kept_paused(&mut self, e: Entry) {
        self.streaming.prefer.keep_paused = Some(e.id);
        self.start(e);
        self.streaming.prefer.keep_paused = None;
        if !self.playing.is_some_and(|p| p.id == e.id) {
            return;
        }
        if self.playing_source.is_some() {
            // Ready to go on, still paused.
            self.playback.pause();
            self.position = self.position_now();
        }
        // Waiting for its YouTube Music copy: the copy found starts paused.
        if self.status != nikgob_view::Status::Paused {
            self.status = nikgob_view::Status::Paused;
            self.publish_player();
        }
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
        // A clip the app queued becomes its song first. One the engine
        // holds already plays as it was handed (an answer now would swap it
        // under a join).
        if self.queued_clip(next).is_some() || self.playback.holds_next(next.id) {
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
        match after_wait(waits, self.status) {
            // Stopped or moved on meanwhile: nothing starts.
            AfterWait::Nothing => {}
            AfterWait::Start => {
                self.streaming.prefer.passed = Some(entry);
                self.start(e);
            }
            AfterWait::StartPaused => {
                self.streaming.prefer.passed = Some(entry);
                self.start_kept_paused(e);
            }
        }
    }

    /// A lookup of a preferred copy answered: a hit joins the track, a
    /// definite miss is kept for days, a failure or a rate limit holds the
    /// track back a little; the track that waits for it starts.
    pub(crate) fn preferred(&mut self, m: PreferMsg) {
        let PreferMsg { track, via, found, filled } = m;
        self.streaming.prefer.asking.remove(&track);
        if let Some(full) = &filled {
            self.isrc_filled(track, full);
        }
        match found {
            Found::Hit(hit) => {
                log::info!("{track} plays from {} as {}", via.display_name(), hit.id.id);
                self.streaming.learn(std::slice::from_ref(&hit));
                if let Err(e) = self.lib.attach_stream_source(track, &hit) {
                    log::warn!("attaching the {} copy of {track}: {e}", via.display_name());
                }
            }
            Found::Miss => {
                log::info!("{track}: no copy on {}", via.display_name());
                let now = nikgob_library::now_ms();
                self.streaming.misses.insert((track, via), now);
                let _ = self.lib.cache_put(&miss_key(track, via), now, "");
            }
            Found::Limited(retry_after) => {
                log::info!("{track}: the {} lookup was rate limited", via.display_name());
                self.streaming.prefer.hold(track, retry_after.unwrap_or(RETRY_AFTER).max(RETRY_AFTER));
            }
            Found::Failed => {
                log::info!("{track}: the {} lookup failed: asked again later", via.display_name());
                self.streaming.prefer.hold(track, RETRY_AFTER);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_track_is_held_until_its_time_and_the_list_stays_bounded() {
        let mut s = PreferState::default();
        let t = TrackId::new();
        s.hold(t, Duration::from_secs(60));
        assert!(s.holds(t));
        s.hold(t, Duration::ZERO);
        assert!(!s.holds(t));
        for _ in 0..HELD_MAX + 10 {
            s.hold(TrackId::new(), Duration::from_secs(60));
        }
        assert!(s.held.len() <= HELD_MAX);
    }
}
