//! A service that asks to wait (a rate limit, YouTube's bot check) does not
//! run the queue down.
//!
//! - **Nothing tries it.** While a service is paused its streams are not
//!   picked to play (a stream whose whole audio is on disk still plays):
//!   [`Worker::waits_for_pause`].
//! - **What plays now comes first.** An entry that only the paused service
//!   could play gives its turn to the first of the next [`LOOK_AHEAD`]
//!   entries that plays now (a file, a cached stream, another service); the
//!   waiting entries stay in the queue right after it, in their order, and
//!   come up again when it ends. The user's own pick is not passed over.
//! - **Or the player waits on it.** With nothing playable near, the player
//!   stands paused on the entry and starts it by itself when the pause
//!   lifts: the service says until when ([`StreamService::paused_until`]),
//!   one wake is set for then, no polling. Play pressed meanwhile tries
//!   again at once.
//! - **No lesson.** An entry passed over or waited on never started: no
//!   start, no skip, nothing for the recommendations. One whose stream was
//!   refused this way as it opened keeps the start it had (the event log
//!   is append-only) and gets no end: never an early skip.
//! - **One toast per pause**, in the toasts' usual words.
//!
//! [`StreamService::paused_until`]: nikgob_core::streaming::StreamService::paused_until

use std::time::{Duration, Instant};

use nikgob_core::model::{EventOrigin, SourceKind};
use nikgob_core::streaming::Service;
use nikgob_view::Status;

use super::Msg;
use crate::Worker;
use crate::queue::Entry;

/// Entries looked at past one that waits for its service's pause.
pub(crate) const LOOK_AHEAD: usize = 4;

/// Where the queue stands with a service's pause.
#[derive(Default)]
pub(crate) struct Held {
    /// The entry the player waits on, and the wake that starts it.
    waiting: Option<(u64, u64)>,
    wake_seq: u64,
    /// The end of the pause the last toast was for.
    said_until: Option<Instant>,
}

impl Worker {
    /// A stream of `service` would only meet the service's pause now: it is
    /// paused and the stream's whole audio is not on disk. Cheap while the
    /// service is not paused (no file is opened then).
    pub(crate) fn waits_for_pause(&self, service: Service, id: &str) -> bool {
        let Some(svc) = self.service(service) else { return false };
        svc.paused_until().is_some_and(|t| t > std::time::SystemTime::now()) && svc.cached_audio(id).is_none()
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
        if self.next_origin != Some(EventOrigin::Manual) {
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
        // A stream refused midway goes on from where it stopped.
        let at = self.failover.filter(|(id, _)| *id == e.id).map_or(Duration::ZERO, |(_, at)| at);
        self.playback.stop();
        self.playing = None;
        self.playing_source = None;
        self.restarted = None;
        self.failover = None;
        self.status = Status::Paused;
        self.position = at;
        // As a restored queue stands: shown, and play starts it there.
        self.resume_at = Some(at);
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
    /// user moved on meanwhile (a longer pause waits again).
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

}
