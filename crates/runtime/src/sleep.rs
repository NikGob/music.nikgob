//! «Таймер сна» ([`v::SleepChoice`]).
//!
//! * **Minutes** count the clock: a pause by hand does not stop them (as in
//!   Apple Music and Spotify). Played to the end, the sound fades out over
//!   the last [`v::SLEEP_FADE`] and the player pauses where it is; already
//!   paused then, the timer just ends.
//! * **Until the track ends** follows the playing track (after a skip, the
//!   new one); **until the album ends** waits for the last entry of the
//!   queue as it was when the timer was set (what autoplay added after it
//!   does not count; another queue started meanwhile: its own end). Both
//!   wait for the music: paused, they wait too. The last track is not
//!   preloaded, so nothing follows it; it fades out over its last
//!   [`v::SLEEP_FADE`] and, once it ends, the next entry waits paused at its
//!   start (the queue stays).
//!
//! The fade is the engine's own ([`crate::Playback::set_sleep_fade`]): the
//! user's volume is never touched, and the next play sounds as before.
//! Nothing is kept between runs.

use std::time::{Duration, Instant};

use nikgob_view::{self as v, Repeat, SLEEP_FADE, SleepChoice, Status};

use crate::{Later, Worker};

/// Each second the fade is given where it should be a little later; the
/// engine glides there, so the next step finds it on its way.
const STEP: Duration = Duration::from_millis(1250);
/// A timer turned off (or put off) in the middle of the fade: the sound
/// comes back over this.
const BACK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Timer {
    /// Pauses at this moment.
    At(Instant),
    /// Pauses when the playing track ends.
    TrackEnd,
    /// Pauses when this queue entry ends.
    After(u64),
}

/// The timer and what the engine was told.
pub(crate) struct Sleep {
    timer: Option<Timer>,
    choice: SleepChoice,
    /// The fade level the engine was last given (1: no fade).
    level: f32,
    /// What [`Self::after_current`] was counted for ([`order_key`]).
    counted: Option<u64>,
    /// How long the entries after the playing one, up to the timer's, last
    /// (None: not counted yet, or one of them has no known length).
    after_current: Option<Duration>,
}

impl Default for Sleep {
    fn default() -> Self {
        Self { timer: None, choice: SleepChoice::Off, level: 1.0, counted: None, after_current: None }
    }
}

/// What the length of the entries after `current` up to `target` depends
/// on: the entries themselves, in their order (a move in the queue changes
/// it as a new queue does).
pub(crate) fn order_key(current: Option<u64>, target: u64, upcoming: &[u64]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (current, target).hash(&mut h);
    for id in upcoming {
        id.hash(&mut h);
        if *id == target {
            break;
        }
    }
    h.finish()
}

/// The whole length of `lengths` (None when one is unknown).
pub(crate) fn total(lengths: impl IntoIterator<Item = Option<u32>>) -> Option<Duration> {
    lengths.into_iter().try_fold(Duration::ZERO, |sum, ms| Some(sum + Duration::from_millis(u64::from(ms?))))
}

/// The fade level with `left` to go: down from 1 over the last
/// [`SLEEP_FADE`], on the volume's perceptual curve (the engine's).
pub(crate) fn fade_level(left: Duration) -> f32 {
    (left.as_secs_f32() / SLEEP_FADE.as_secs_f32()).clamp(0.0, 1.0)
}

/// What a timer of minutes does at `now`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MinutesStep {
    /// Nothing yet.
    Wait,
    /// Glide to this level over [`STEP`].
    Fade(f32),
    /// Time is up: pause (if playing) and end.
    End,
}

pub(crate) fn minutes_step(deadline: Instant, now: Instant, playing: bool) -> MinutesStep {
    if now >= deadline {
        return MinutesStep::End;
    }
    let left = deadline - now;
    if playing && left <= SLEEP_FADE + STEP {
        MinutesStep::Fade(fade_level(left.saturating_sub(STEP)))
    } else {
        MinutesStep::Wait
    }
}

impl Worker {
    /// `Command::SleepTimer`.
    pub(crate) fn sleep_set(&mut self, choice: SleepChoice) {
        let timer = match choice {
            SleepChoice::Off => None,
            SleepChoice::Minutes(m) => Some(Timer::At(Instant::now() + Duration::from_secs(u64::from(m) * 60))),
            SleepChoice::TrackEnd if self.queue.current().is_some() => Some(Timer::TrackEnd),
            SleepChoice::ContextEnd => match self.sleep_context_end() {
                Some(e) => Some(Timer::After(e)),
                None => return,
            },
            SleepChoice::TrackEnd => return,
        };
        log::info!("sleep timer: {choice:?}");
        self.sleep.timer = timer;
        self.sleep.choice = choice;
        self.sleep.counted = None;
        self.sleep_come_back();
        self.sleep_preload();
        self.sleep_publish();
    }

    /// The last entry of the queue that autoplay did not add (the playing
    /// one when it is the last).
    fn sleep_context_end(&self) -> Option<u64> {
        let current = self.queue.current()?;
        Some(
            self.queue
                .upcoming()
                .iter()
                .rev()
                .find(|e| !self.autoplay_entries.contains(&e.id))
                .map_or(current.id, |e| e.id),
        )
    }

    /// The entry after which the timer pauses, when it waits for one.
    fn sleep_target(&self) -> Option<u64> {
        match self.sleep.timer? {
            Timer::At(_) => None,
            Timer::TrackEnd => self.queue.current().map(|e| e.id),
            // Repeat-one never gets past the playing track: it is the end.
            Timer::After(_) if self.queue.repeat == Repeat::One => self.queue.current().map(|e| e.id),
            Timer::After(e) => Some(e),
        }
    }

    /// Nothing is preloaded after the playing entry when the timer ends
    /// with it ([`Self::prepare_next`]).
    pub(crate) fn sleep_ends_with_playing(&self) -> bool {
        self.playing.is_some() && self.sleep_target() == self.playing.map(|e| e.id)
    }

    /// The preload follows the timer: cleared for the last entry, back when
    /// the timer goes.
    fn sleep_preload(&mut self) {
        if self.playing.is_some() {
            self.preload_next();
        }
    }

    /// How long until the timer pauses, as of `now` (None: no timer, its
    /// entry is not ahead, or a length on the way is unknown).
    fn sleep_left(&self, now: Instant) -> Option<Duration> {
        match self.sleep.timer? {
            Timer::At(t) => Some(t.saturating_duration_since(now)),
            Timer::TrackEnd | Timer::After(_) => {
                let target = self.sleep_target()?;
                let current = self.queue.current()?;
                if current.id != target && !self.queue.upcoming().iter().any(|e| e.id == target) {
                    return None;
                }
                let after = if current.id == target { Duration::ZERO } else { self.sleep.after_current? };
                let duration = self.state.read().player.duration?;
                let rest = duration.saturating_sub(self.position_at_instant(now));
                let rest = if self.speed > 0.0 { rest.div_f64(self.speed) } else { rest };
                Some(rest + after)
            }
        }
    }

    /// Once a second: the countdown, the fade, the end of a timer of
    /// minutes; an album's end lost from the queue is looked for again.
    pub(crate) fn sleep_poll(&mut self) {
        let Some(timer) = self.sleep.timer else { return };
        if let Timer::After(e) = timer
            && self.queue.current().is_none_or(|c| c.id != e)
            && !self.queue.upcoming().iter().any(|u| u.id == e)
        {
            // Another queue (or the entry taken out): its own end.
            match self.sleep_context_end() {
                Some(end) => self.sleep.timer = Some(Timer::After(end)),
                None => self.sleep.timer = None,
            }
            self.sleep_preload();
        }
        let now = Instant::now();
        let playing = self.status == Status::Playing;
        match self.sleep.timer {
            None => {
                self.sleep_come_back();
                self.sleep_publish();
                return;
            }
            Some(Timer::At(deadline)) => match minutes_step(deadline, now, playing) {
                MinutesStep::Wait => {}
                MinutesStep::Fade(level) => self.sleep_fade(level, STEP),
                MinutesStep::End => {
                    log::info!("sleep timer: time is up");
                    self.sleep.timer = None;
                    self.sleep.choice = SleepChoice::Off;
                    if matches!(self.status, Status::Playing | Status::Buffering) {
                        self.pause();
                    }
                }
            },
            Some(_) => {
                self.sleep_count_after();
                if playing {
                    match self.sleep_left(now) {
                        Some(left) => self.sleep_fade(fade_level(left.saturating_sub(STEP)), STEP),
                        // Not known how far the end is (a stream without a
                        // length): no fade yet, a faded sound comes back.
                        None => self.sleep_fade(1.0, BACK),
                    }
                }
            }
        }
        self.sleep_publish();
    }

    /// Recounts how long the entries after the playing one last, up to
    /// the timer's, when they changed: read off the loop, unknown meanwhile.
    fn sleep_count_after(&mut self) {
        let Some(target) = self.sleep_target() else { return };
        let current = self.queue.current().map(|e| e.id);
        let upcoming = self.queue.upcoming();
        let ids: Vec<u64> = upcoming.iter().map(|e| e.id).collect();
        let key = order_key(current, target, &ids);
        if self.sleep.counted == Some(key) {
            return;
        }
        self.sleep.counted = Some(key);
        self.sleep.after_current = None;
        let tracks: Vec<_> = match upcoming.iter().position(|e| e.id == target) {
            Some(i) if current != Some(target) => upcoming[..=i].iter().map(|e| e.track).collect(),
            _ => {
                self.sleep.after_current = Some(Duration::ZERO);
                return;
            }
        };
        let lib = self.lib.clone();
        let later = self.later_tx.clone();
        let spawned = std::thread::Builder::new().name("nikgob-sleep".into()).spawn(move || {
            let rows = lib.get_track_rows(&tracks).unwrap_or_default();
            let after = total(tracks.iter().map(|t| rows.get(t).and_then(|r| r.duration_ms)));
            let _ = later.send(Later::new("the sleep timer's lengths", move |w| {
                // Only for the queue it was asked for.
                if w.sleep.counted == Some(key) {
                    w.sleep.after_current = after;
                    w.sleep_publish();
                }
            }));
        });
        if let Err(e) = spawned {
            log::warn!("sleep timer: the lengths were not read: {e}");
        }
    }

    /// Gives the engine a fade level (only when it is another one).
    fn sleep_fade(&mut self, level: f32, over: Duration) {
        if (level - self.sleep.level).abs() < 1e-4 {
            return;
        }
        self.sleep.level = level;
        self.playback.set_sleep_fade(level, over);
    }

    /// The level the sound should be at now: the fade's while one is due,
    /// else full.
    fn sleep_level_now(&self) -> f32 {
        self.sleep_left(Instant::now()).map_or(1.0, fade_level)
    }

    /// A timer changed or ended: a sound faded by an earlier one comes
    /// back (over a moment while it plays).
    fn sleep_come_back(&mut self) {
        let want = self.sleep_level_now();
        if want > self.sleep.level {
            let over = if self.status == Status::Playing { BACK } else { Duration::ZERO };
            self.sleep_fade(want, over);
        }
    }

    /// Before a play or a resume: the sound as loud as it should be now,
    /// at once. A timer of minutes inside its fade: the fade's level; one
    /// waiting for the music: full (the track's own position and length are
    /// not set yet; the next poll fades from there if its end is near).
    pub(crate) fn sleep_before_play(&mut self) {
        let want = match self.sleep.timer {
            Some(Timer::At(_)) => self.sleep_level_now(),
            _ => 1.0,
        };
        self.sleep_fade(want, Duration::ZERO);
    }

    /// The engine went on from the timer's last entry into the next one by
    /// itself (a crossfade, or a preload it had taken before the timer
    /// cleared it): the boundary is crossed, so the next one is paused at
    /// its start, silent until played again.
    pub(crate) fn sleep_crossed(&mut self) {
        log::info!("sleep timer: the next track had begun, paused at its start");
        self.sleep.timer = None;
        self.sleep.choice = SleepChoice::Off;
        self.sleep_fade(0.0, Duration::ZERO);
        self.pause();
        self.playback.seek(Duration::ZERO);
        self.position = Duration::ZERO;
        self.position_at = Instant::now();
        self.publish_player();
        self.sleep_publish();
        self.sleep_preload();
    }

    /// The engine finished entry `id`: when the timer ends with it, the
    /// next entry waits paused at its start (none: the player stops) and
    /// true is returned (nothing else is started).
    pub(crate) fn sleep_ended(&mut self, id: u64) -> bool {
        if self.sleep_target() != Some(id) {
            return false;
        }
        log::info!("sleep timer: the music it waited for ended");
        self.sleep.timer = None;
        self.sleep.choice = SleepChoice::Off;
        match self.queue.advance_auto() {
            Some(_) => {
                self.playback.stop();
                self.playing = None;
                self.playing_source = None;
                self.failover = None;
                self.resume_at = Some(Duration::ZERO);
                self.position = Duration::ZERO;
                self.position_at = Instant::now();
                self.status = Status::Paused;
                self.publish_now_playing();
            }
            None => self.stop(),
        }
        self.sleep_publish();
        true
    }

    /// The timer as the player shows it.
    fn sleep_publish(&mut self) {
        let view = self.sleep.timer.map(|t| v::SleepView {
            choice: self.sleep.choice,
            deadline: match t {
                Timer::At(at) => Some(at),
                _ => None,
            },
            after_current: match t {
                Timer::At(_) => Some(Duration::ZERO),
                _ if self.sleep_target() == self.queue.current().map(|e| e.id) => Some(Duration::ZERO),
                _ => self.sleep.after_current,
            },
        });
        if self.state.read().player.sleep == view {
            return;
        }
        self.publish(|s| s.player.sleep = view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_move_in_the_queue_changes_what_was_counted() {
        let before = order_key(Some(1), 4, &[2, 3, 4, 5]);
        assert_eq!(before, order_key(Some(1), 4, &[2, 3, 4, 9]), "after the target: the same");
        assert_ne!(before, order_key(Some(1), 4, &[3, 4, 2, 5]), "one moved past the target");
        assert_ne!(before, order_key(Some(1), 4, &[3, 2, 4, 5]), "the order before it");
    }

    #[test]
    fn an_unknown_length_makes_the_whole_unknown() {
        assert_eq!(total([Some(1000), Some(2500)]), Some(Duration::from_millis(3500)));
        assert_eq!(total([Some(1000), None, Some(2500)]), None);
        assert_eq!(total([]), Some(Duration::ZERO));
    }

    #[test]
    fn the_fade_takes_the_last_twenty_seconds() {
        assert_eq!(fade_level(Duration::from_secs(60)), 1.0);
        assert_eq!(fade_level(SLEEP_FADE), 1.0);
        assert!((fade_level(Duration::from_secs(10)) - 0.5).abs() < 1e-6);
        assert_eq!(fade_level(Duration::ZERO), 0.0);
    }

    #[test]
    fn minutes_count_the_clock_fade_while_playing_and_end() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(15 * 60);
        assert_eq!(minutes_step(deadline, now, true), MinutesStep::Wait);
        assert_eq!(minutes_step(deadline, deadline - Duration::from_secs(60), true), MinutesStep::Wait);
        // Ten seconds before: where the sound should be a step later.
        match minutes_step(deadline, deadline - Duration::from_secs(10), true) {
            MinutesStep::Fade(l) => assert!((l - fade_level(Duration::from_secs(10) - STEP)).abs() < 1e-6, "{l}"),
            other => panic!("{other:?}"),
        }
        // Paused by hand: the clock goes on, nothing to fade.
        assert_eq!(minutes_step(deadline, deadline - Duration::from_secs(10), false), MinutesStep::Wait);
        assert_eq!(minutes_step(deadline, deadline, false), MinutesStep::End);
        assert_eq!(minutes_step(deadline, deadline + Duration::from_secs(5), true), MinutesStep::End);
    }
}
