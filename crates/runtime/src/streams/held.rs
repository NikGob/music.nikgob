//! A service that asks to wait (a rate limit, YouTube's bot check) does not
//! run the queue down.
//!
//! - **Nothing tries it.** While a service is paused its streams are not
//!   picked to play (a stream whose whole audio is on disk still plays):
//!   [`Worker::waits_for_pause`]. Whether it is on disk is asked once per
//!   stream and pause, off the player's loop (the stream waits until the
//!   answer says it is there; then a player waiting on it goes on).
//! - **Another copy first.** A stream refused by the pause as it plays goes
//!   on from where it was on another copy of the track that plays now.
//! - **What plays now comes first.** An entry that only the paused service
//!   could play gives its turn to the first of the next [`LOOK_AHEAD`]
//!   entries that plays now (a file, a cached stream, another service); the
//!   waiting entries stay in the queue right after it, in their order, and
//!   come up again when it ends. The user's own pick is not passed over.
//! - **Or the player waits on it.** With nothing playable near, the player
//!   stands paused on the entry and starts it by itself when the pause
//!   lifts: the service says until when ([`StreamService::paused_until`]),
//!   one wake is set for then, no polling. Play pressed meanwhile tries
//!   again at once; Pause (the user's, the sleep timer's) or an edit of the
//!   queue cancels the wake: the player stays paused until Play.
//! - **Its place in the track.** A stream refused midway starts from where
//!   it stopped whenever its entry starts again: after a track that played
//!   in between, after Play pressed during the wait, after the wake. Such an
//!   entry is not handed to the engine ahead (a gapless step would start it
//!   from its beginning).
//! - **No lesson.** An entry passed over or waited on never started: no
//!   start, no skip, nothing for the recommendations. One whose stream was
//!   refused this way as it opened keeps the start it had (the event log
//!   is append-only) and gets no end: never an early skip.
//! - **One toast per pause**, in the toasts' usual words.
//! - **The listener's YouTube Music first.** While it plays or comes next
//!   ([`Worker::youtube_listened`]), no background call of YouTube Music
//!   starts ([`Worker::background_wait`]): each job waits and looks again
//!   after [`StreamSchedule::listener_recheck`](super::StreamSchedule::listener_recheck).
//!   A job of several requests looks before each ([`Held::listening`]). A
//!   request already sent cannot be called back: at most one is on its way
//!   when the listener starts.
//!
//! [`StreamService::paused_until`]: nikgob_core::streaming::StreamService::paused_until

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use nikgob_core::model::{SourceKind, TrackId};
use nikgob_core::streaming::Service;
use nikgob_view::Status;

use super::Msg;
use crate::Worker;
use crate::queue::Entry;

/// Entries looked at past one that waits for its service's pause.
pub(crate) const LOOK_AHEAD: usize = 4;
/// Streams whose place on disk is remembered for one pause, at most.
const CACHED_MAX: usize = 512;
/// Places in cut streams kept, at most.
const RESUME_MAX: usize = 32;

/// Where the queue stands with a service's pause.
#[derive(Default)]
pub(crate) struct Held {
    /// The entry the player waits on, and the wake that starts it.
    waiting: Option<(u64, u64)>,
    wake_seq: u64,
    /// The end of the pause the last toast was for.
    said_until: Option<Instant>,
    /// Where streams refused midway go on, by entry, until that entry
    /// starts.
    resume: HashMap<u64, Duration>,
    /// Whether a paused service's streams are on disk, for the pause that
    /// ends at the time kept with them (`None`: being asked).
    cached: Option<(SystemTime, HashMap<(Service, String), Option<bool>>)>,
    /// YouTube Music plays or comes next, as the worker last saw it, for
    /// the jobs off the loop: a job of several requests (an artist looked
    /// up, then its radio), the stream analysis before it downloads.
    pub(crate) listening: Arc<AtomicBool>,
}

impl Worker {
    /// A stream of `service` would only meet the service's pause now: it is
    /// paused and the stream's whole audio is not known to be on disk. No
    /// look at the disk on the loop: while the service is paused, each
    /// stream is asked once, off the loop ([`Self::cache_known`] hears it).
    pub(crate) fn waits_for_pause(&mut self, service: Service, id: &str) -> bool {
        let Some(svc) = self.service(service) else { return false };
        let Some(until) = svc.paused_until().filter(|t| *t > SystemTime::now()) else { return false };
        let h = &mut self.streaming.held;
        if h.cached.as_ref().is_none_or(|(at, _)| *at != until) {
            h.cached = Some((until, HashMap::new()));
        }
        let Some((_, memo)) = h.cached.as_mut() else { return true };
        let key = (service, id.to_owned());
        match memo.get(&key) {
            Some(Some(on_disk)) => return !on_disk,
            Some(None) => return true,
            None => {}
        }
        if memo.len() >= CACHED_MAX {
            memo.clear();
        }
        memo.insert(key, None);
        let id = id.to_owned();
        self.streaming.spawn(async move {
            let ask = id.clone();
            let on_disk = tokio::task::spawn_blocking(move || svc.has_cached_audio(&ask)).await.unwrap_or(false);
            Msg::CacheKnown { service, id, until, on_disk }
        });
        true
    }

    /// Whether a paused service's stream is on disk, as asked off the loop:
    /// one that is plays now; the player waiting on an entry tries it again.
    pub(crate) fn cache_known(&mut self, service: Service, id: String, until: SystemTime, on_disk: bool) {
        let Some((at, memo)) = self.streaming.held.cached.as_mut() else { return };
        if *at != until {
            return;
        }
        memo.insert((service, id), Some(on_disk));
        if !on_disk {
            return;
        }
        let waiting = self.streaming.held.waiting.map(|(entry, _)| entry);
        if let Some(entry) = waiting
            && self.playing.is_none()
            && self.status == Status::Paused
            && self.queue.current().is_some_and(|c| c.id == entry)
        {
            self.streaming.held.waiting = None;
            self.start_restored();
            return;
        }
        self.preload_next();
    }

    /// `track` plays now: a file that is there, or a stream whose service
    /// gives sound and is not paused (or has its audio on disk).
    pub(crate) fn plays_now(&mut self, track: TrackId) -> bool {
        let sources = self.lib.sources_of_track(track).unwrap_or_default();
        sources.iter().filter(|s| s.trashed_at.is_none() && s.available).any(|s| match s.kind.service() {
            None => true,
            Some(service) => self.has_audio(service) && !self.waits_for_pause(service, &s.locator),
        })
    }

    /// The paused service that keeps `e` silent, and for how long: a stream
    /// of it would play but for the pause, or the lookup of its sound would
    /// ask it.
    fn paused_for(&mut self, e: Entry) -> Option<(Service, Duration)> {
        let sources = self.lib.sources_of_track(e.track).ok()?;
        let refused = self.refused_videos();
        for s in sources.iter().filter(|s| s.trashed_at.is_none()) {
            let Some(service) = s.kind.service() else { continue };
            let usable = s.available || (s.kind == SourceKind::Youtube && !refused.contains(&s.locator));
            if !usable || !self.has_audio(service) {
                continue;
            }
            if let Some(left) = self.pause_left(service) {
                return Some((service, left));
            }
        }
        let via = self.play_lookup(e.track, super::resolve::PLAYED_MISS_TTL)?;
        Some((via, self.pause_left(via)?))
    }

    /// `e` has nothing that plays now because its service asks to wait (see
    /// the module docs): the next entry that plays now takes its turn, or
    /// the player waits on it. False when no pause is to blame.
    pub(crate) fn hold_for_pause(&mut self, e: Entry) -> bool {
        let Some((service, left)) = self.paused_for(e) else { return false };
        let who = service.display_name();
        // The user's own pick is waited for, not passed over.
        if self.next_origin != Some(nikgob_core::model::EventOrigin::Manual) {
            let ahead: Vec<Entry> = self.queue.upcoming().iter().take(LOOK_AHEAD).copied().collect();
            for n in ahead {
                if n.id == e.id || self.skips_disliked(n) || self.clip_not_ready(n) {
                    continue;
                }
                let Some((req, source, kind)) = self.request_skipping(n, &[]) else { continue };
                log::info!(
                    "{}: {who} asks to wait {} s; {} plays first, the waiting ones stay queued after it",
                    e.track,
                    left.as_secs(),
                    n.track
                );
                self.queue.move_next_to(n.id, e.id, false);
                self.queue.jump(n.id);
                self.pause_toast(service, left);
                self.tried.clear();
                self.start_with(n, req, source, kind);
                return true;
            }
        }
        log::info!("{}: {who} asks to wait {} s; the player waits on it", e.track, left.as_secs());
        self.pause_toast(service, left);
        // Shown where it stopped; it goes on there when it starts.
        let at = self.streaming.held.resume.get(&e.id).copied().unwrap_or(Duration::ZERO);
        self.playback.stop();
        self.playing = None;
        self.playing_source = None;
        self.restarted = None;
        self.failover = None;
        self.status = Status::Paused;
        self.position = at;
        // As a restored queue stands: shown, and play starts it.
        self.resume_at = Some(Duration::ZERO);
        let h = &mut self.streaming.held;
        h.wake_seq += 1;
        let seq = h.wake_seq;
        h.waiting = Some((e.id, seq));
        let after = left + super::schedule::wake_jitter();
        self.streaming.spawn(async move {
            tokio::time::sleep(after).await;
            Msg::PauseLifted(seq)
        });
        self.publish_now_playing();
        self.publish_player();
        true
    }

    /// The pause the player waited on is over: the entry starts, unless the
    /// user moved on or paused meanwhile (a longer pause waits again).
    pub(crate) fn pause_lifted(&mut self, seq: u64) {
        let Some((entry, s)) = self.streaming.held.waiting else { return };
        if s != seq {
            return;
        }
        self.streaming.held.waiting = None;
        if self.playing.is_some() || self.status != Status::Paused {
            return;
        }
        let Some(e) = self.queue.current().filter(|c| c.id == entry) else { return };
        log::info!("{}: the pause is over, it starts", e.track);
        self.start_restored();
    }

    /// The player is paused on purpose (the user, the sleep timer) or the
    /// user edits the queue: no wake starts the entry it waits on.
    pub(crate) fn cancel_pause_wake(&mut self) {
        if self.streaming.held.waiting.take().is_some() {
            log::info!("the player stays paused on the entry that waited for a service");
        }
    }

    /// `entry`'s stream was refused midway at `at`: it goes on there when it
    /// starts again.
    pub(crate) fn hold_position(&mut self, entry: u64, at: Duration) {
        let r = &mut self.streaming.held.resume;
        if at.is_zero() {
            r.remove(&entry);
            return;
        }
        if r.len() >= RESUME_MAX && !r.contains_key(&entry) {
            r.clear();
        }
        r.insert(entry, at);
    }

    /// `entry` goes on from a place of its own (see [`Self::hold_position`]).
    pub(crate) fn has_held_position(&self, entry: u64) -> bool {
        self.streaming.held.resume.contains_key(&entry)
    }

    /// Where `entry` goes on, once (see [`Self::hold_position`]).
    pub(crate) fn take_held_position(&mut self, entry: u64) -> Option<Duration> {
        self.streaming.held.resume.remove(&entry)
    }

    /// The user moved `entry`'s bar: their place wins.
    pub(crate) fn drop_held_position(&mut self, entry: u64) {
        self.streaming.held.resume.remove(&entry);
    }

    /// «YouTube Music просит подождать до 14:05 — продолжу сам», once a
    /// pause.
    pub(super) fn pause_toast(&mut self, service: Service, left: Duration) {
        let now = Instant::now();
        let h = &mut self.streaming.held;
        if h.said_until.is_some_and(|t| now < t) {
            return;
        }
        h.said_until = Some(now + left);
        let until = Some(left)
            .filter(|p| *p > super::search::SHORT_WAIT)
            .and_then(super::accounts::hm_after)
            .map(|hm| format!(" до {hm}"))
            .unwrap_or_default();
        self.toast(format!("{} просит подождать{until} — продолжу сам", service.display_name()), None);
    }

    /// YouTube Music plays, or comes next, while the player runs: the
    /// playing source is a YouTube video (or one is looked up for the
    /// playing entry), or the next entry has nothing that plays without
    /// YouTube Music (a file, another service with sound): its YouTube copy
    /// plays, or one is looked up. Kept in [`Held::listening`] for the jobs
    /// off the loop.
    pub(crate) fn youtube_listened(&mut self) -> bool {
        let listened = self.youtube_listened_now();
        self.streaming.held.listening.store(listened, Ordering::Relaxed);
        listened
    }

    fn youtube_listened_now(&mut self) -> bool {
        if !matches!(self.status, Status::Playing | Status::Buffering) || !self.has_audio(Service::Youtube) {
            return false;
        }
        match self.playing_source {
            Some((_, kind)) if kind == SourceKind::Youtube => return true,
            // Waiting for its YouTube copy to be found.
            None if self.playing.is_some() => return true,
            _ => {}
        }
        let Some(next) = self.queue.peek_auto_next() else { return false };
        let sources = self.lib.sources_of_track(next.track).unwrap_or_default();
        !sources.iter().filter(|s| s.trashed_at.is_none() && s.available).any(|s| match s.kind.service() {
            None => true,
            Some(Service::Youtube) => false,
            Some(k) => self.has_audio(k),
        })
    }

    /// A YouTube video starts playing: the jobs off the loop hold at once.
    pub(crate) fn youtube_plays(&self) {
        self.streaming.held.listening.store(true, Ordering::Relaxed);
    }

    /// How long a background call of `service` waits: its pause, or, for
    /// YouTube Music while the listener plays it or it comes next, until it
    /// looks again ([`super::StreamSchedule::listener_recheck`]): none
    /// starts meanwhile. `None`: it goes now.
    pub(crate) fn background_wait(&mut self, service: Service) -> Option<Duration> {
        if let Some(left) = self.pause_left(service) {
            return Some(left);
        }
        (service == Service::Youtube && self.youtube_listened()).then_some(self.streaming.schedule.listener_recheck)
    }
}
