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
//!   miss, for days ([`super::resolve`]'s misses). A failed request (the
//!   network, the login, a lookup that hangs) or a rate limit stops every
//!   Spotify lookup of copies for [`super::StreamSchedule::prefer_retry`]
//!   (longer when the service asks), whatever the track: an outage asks
//!   nothing track after track.
//! * Gapless, the next track waits for its Spotify answer before it is
//!   handed to the engine, but not for long: at most
//!   [`super::StreamSchedule::prefer_wait`], and never closer than
//!   [`AHEAD_MARGIN`] to the end of the playing one (counted again after
//!   a seek or a change of speed; at once when the playing track's length
//!   is not known). Then it is handed over with what it has (its YouTube
//!   Music copy), so the join stays gapless; an answer after that is for
//!   the next time only. A request the engine holds is never taken back or
//!   swapped (it may have taken it over already), and the join records
//!   the source it was handed.
//!   Without a request handed (gapless off, or none prepared), the next
//!   track starts as any other: with the bounded wait.
//! * A pause during the wait is kept through whatever comes next (the
//!   YouTube Music lookup of a track with nothing that plays, the next
//!   track when it has nothing either); a track skipped or stopped
//!   meanwhile does not start when the answer comes.
//!
//! Nothing of this runs on the player loop but the bookkeeping: the lookup
//! and the wait are the streams runtime's.

use std::collections::HashSet;
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
/// A lookup still running after this is given up (failed): two that hang
/// would otherwise keep every later one from starting.
const LOOKUP_LIMIT: Duration = Duration::from_secs(30);
/// The next track is handed to the engine at least this long before the
/// playing one ends, its Spotify answer in or not: time to open it for a
/// gapless join.
const AHEAD_MARGIN: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct PreferState {
    /// Tracks whose preferred copy is being looked up.
    asking: HashSet<TrackId>,
    /// The queue entry that waits for its preferred copy.
    waiting: Option<u64>,
    /// The entry that waited already: it plays what it has now.
    passed: Option<u64>,
    /// The next entry held back from the engine for its answer, until
    /// [`Msg::PreferAhead`] of it (with [`Self::ahead_seq`]), and since
    /// when.
    ahead: Option<u64>,
    ahead_since: Option<Instant>,
    /// Counts the timers of [`Self::ahead`]: only the last one counts.
    ahead_seq: u64,
    /// The next entry whose answer did not come in time: it is handed
    /// over with what it has.
    ahead_passed: Option<u64>,
    /// No lookup of copies is asked of Spotify before this (a failed or
    /// rate-limited one).
    held_until: Option<Instant>,
}

impl PreferState {
    pub(super) fn hold(&mut self, for_: Duration) {
        let until = Instant::now() + for_;
        self.held_until = Some(self.held_until.map_or(until, |u| u.max(until)));
    }

    pub(super) fn held(&self) -> bool {
        self.held_until.is_some_and(|u| Instant::now() < u)
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
    /// the track has no copy there yet and did not miss there lately, and
    /// no Spotify lookup failed just now.
    fn preferred_for(&mut self, e: Entry) -> Option<Service> {
        let via = Service::Spotify;
        if !self.has_audio(via) || self.pause_left(via).is_some() || self.streaming.prefer.held() {
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

    /// The next entry waits for its preferred copy before it is handed to
    /// the engine (its lookup asked now if it was not), until the answer
    /// comes or [`Self::ahead_wait`] runs out. False when it is handed over
    /// as it is: the engine holds it already, its time ran out, or no
    /// lookup could start.
    pub(crate) fn prefer_pending(&mut self, next: Entry) -> bool {
        if self.playback.holds_next(next.id) || self.streaming.prefer.ahead_passed == Some(next.id) {
            return false;
        }
        let asking = self.prefer_asking(next.track) || self.preferred_for(next).is_some_and(|via| self.ask_preferred(next.track, via));
        if !asking {
            return false;
        }
        if self.streaming.prefer.ahead == Some(next.id) {
            return true;
        }
        self.streaming.prefer.ahead = Some(next.id);
        self.streaming.prefer.ahead_since = Some(Instant::now());
        self.arm_ahead()
    }

    /// Arms the timer of the next entry that waits for its answer (see
    /// [`Self::ahead_wait`]); when no time is left, it goes over at once
    /// (false: it is not waited for).
    fn arm_ahead(&mut self) -> bool {
        let Some(entry) = self.streaming.prefer.ahead else { return false };
        let wait = self.ahead_wait();
        if wait.is_zero() {
            self.streaming.prefer.ahead = None;
            self.streaming.prefer.ahead_passed = Some(entry);
            return false;
        }
        let st = &mut self.streaming.prefer;
        st.ahead_seq += 1;
        let seq = st.ahead_seq;
        self.streaming.spawn(async move {
            tokio::time::sleep(wait).await;
            Msg::PreferAhead(entry, seq)
        });
        true
    }

    /// How long the next entry may still wait for its answer:
    /// [`super::StreamSchedule::prefer_wait`] since it began, less when the
    /// playing track ends sooner than [`AHEAD_MARGIN`] after that (as fast
    /// as it plays now); nothing when the playing track's length is not
    /// known (its end could come any moment).
    fn ahead_wait(&mut self) -> Duration {
        let since = self.streaming.prefer.ahead_since.map_or(Duration::ZERO, |t| t.elapsed());
        let wait = self.streaming.schedule.prefer_wait.saturating_sub(since);
        let Some(p) = self.playing else { return wait };
        let Some(total) = self.lib.get_track(p.track).ok().flatten().and_then(|t| t.duration_ms) else {
            return Duration::ZERO;
        };
        let left = Duration::from_millis(u64::from(total)).saturating_sub(self.position_now());
        let left = left.div_f64(self.speed.max(0.25));
        wait.min(left.saturating_sub(AHEAD_MARGIN))
    }

    /// The playing track moved (a seek) or changed its speed: the next
    /// entry's time for its answer is counted again, and it goes over at
    /// once when too little is left.
    pub(crate) fn prefer_ahead_again(&mut self) {
        let Some(entry) = self.streaming.prefer.ahead else { return };
        if !self.arm_ahead() {
            log::info!("the playing track ends soon: the next one goes over with what it has");
            self.hand_ahead(entry);
        }
    }

    /// The next entry's time for its answer ran out: it is handed over
    /// with what it has (the answer, when it comes, is for the next time).
    pub(crate) fn prefer_ahead_over(&mut self, entry: u64, seq: u64) {
        if self.streaming.prefer.ahead != Some(entry) || self.streaming.prefer.ahead_seq != seq {
            return;
        }
        self.streaming.prefer.ahead = None;
        log::info!("the next track's preferred copy did not come in time: handed over with what it has");
        self.hand_ahead(entry);
    }

    /// The next entry `entry` goes to the engine with what it has, if it
    /// is still the next one and not handed over yet.
    fn hand_ahead(&mut self, entry: u64) {
        if !self.queue.peek_auto_next().is_some_and(|n| n.id == entry) || self.playback.holds_next(entry) {
            return;
        }
        self.streaming.prefer.ahead_passed = Some(entry);
        self.preload_next();
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

    /// An entry that the user paused while it waited starts now (`start`):
    /// whatever starts stays paused, this entry or the one after it when
    /// it has nothing to play; one that waits for a lookup next waits
    /// paused, and the copy found starts paused.
    pub(crate) fn start_kept_paused(&mut self, e: Entry) {
        self.start(e);
        if self.playing.is_none() {
            return;
        }
        if self.playing_source.is_some() {
            // Ready to go on, still paused.
            self.playback.pause();
            self.position = self.position_now();
        }
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
                let retry = self.streaming.schedule.prefer_retry;
                self.streaming.prefer.hold(retry_after.map_or(retry, |r| r.max(retry)));
            }
            Found::Failed => {
                log::info!("{track}: the {} lookup failed: no lookups there for a while", via.display_name());
                let retry = self.streaming.schedule.prefer_retry;
                self.streaming.prefer.hold(retry);
            }
        }
        let waiting = self
            .streaming
            .prefer
            .waiting
            .filter(|w| self.playing.is_some_and(|p| p.id == *w && p.track == track));
        if let Some(entry) = waiting {
            self.prefer_go_on(entry);
        } else if self.queue.peek_auto_next().is_some_and(|n| n.track == track && !self.playback.holds_next(n.id)) {
            // The next track waited for it: handed over now. One the engine
            // holds keeps what it was handed.
            self.preload_next();
        }
        self.resolve_ahead();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_stops_lookups_until_its_time_and_a_longer_one_wins() {
        let mut s = PreferState::default();
        assert!(!s.held());
        s.hold(Duration::from_secs(60));
        assert!(s.held());
        // A shorter hold after it does not cut it short.
        s.hold(Duration::ZERO);
        assert!(s.held());
        let mut s = PreferState::default();
        s.hold(Duration::ZERO);
        assert!(!s.held());
    }
}
