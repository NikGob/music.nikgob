//! One SoundCloud track played by the official widget, as a live
//! [`PcmSource`] of our engine (like `nikgob_soloist::SoloistTrack`).
//!
//! The widget plays the track; the source hands the engine its sound as
//! captured from the host's process tree, the copy heard (the widget's own
//! output is turned down to a level the capture undoes). Without a capture
//! the source gives silence paced by the widget's position, and the widget
//! is heard itself.
//!
//! It starts once the widget's position moves (a DRM track takes ~5 s for
//! its licence); an error of the widget, the host going away, or no start
//! within [`START_TIMEOUT`] fails the item, and the runtime goes on to the
//! track's other sources as before. The end is the widget's FINISH, or its
//! position past the end. A FINISH well before the length the library
//! knows is a preview, not the track: that fails it too (from there the
//! runtime goes on on another source).
//!
//! The engine reads on its decoder thread every few milliseconds while a
//! read is [`PcmRead::Pending`]; nothing here blocks.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_audio::{PcmError, PcmRead, PcmSource};
use nikgob_soloist::route::{CHANNELS, Captured as _, Feed, RATE};

use crate::manager::{Live, ScWidget, Status};
use crate::protocol::Command;

/// How long the widget may take to start a track (its licence included).
pub const START_TIMEOUT: Duration = Duration::from_secs(15);
/// ... plus this when the host had to start for it.
const COLD_START: Duration = Duration::from_secs(10);
/// How long a seek may take to show in the widget's position.
const SEEK_TIMEOUT: Duration = Duration::from_secs(4);
/// A position this far past the track's end is its end.
const OVERRUN_MS: u64 = 3000;
/// A FINISH this far before the known length is a preview's end.
const PREVIEW_SLACK_MS: u64 = 10_000;

/// Every failure says DRM: the runtime's toast then tells the user why the
/// track goes on elsewhere.
fn err(text: &str) -> PcmError {
    format!("SoundCloud player (DRM track): {text}").into()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Unrequested,
    Requested { at: Instant, seq: u64, cold: bool },
    Playing { seq: u64 },
    Seeking { seq: u64, target_ms: u64, at: Instant },
    Ended,
}

pub struct WidgetTrack {
    widget: Arc<ScWidget>,
    /// The numeric track id (`None`: not one, it cannot play).
    track: Option<u64>,
    duration: Option<Duration>,
    phase: Phase,
    counted: bool,
    feed: Feed,
    /// The furthest position seen while playing.
    max_pos: u64,
    /// A seek asked for before the track started.
    pending_seek: Option<u64>,
}

impl WidgetTrack {
    pub(crate) fn new(widget: Arc<ScWidget>, track: Option<u64>, duration: Option<Duration>) -> Self {
        Self {
            widget,
            track,
            duration,
            phase: Phase::Unrequested,
            counted: false,
            feed: Feed::new("sc widget"),
            max_pos: 0,
            pending_seek: None,
        }
    }

    fn known_ms(&self, view: &Live) -> Option<u64> {
        self.duration.map(|d| d.as_millis() as u64).or(view.duration_ms)
    }

    fn start_playing(&mut self, seq: u64, view: &Live) {
        // What was captured before is the track before ours (or silence).
        self.widget.clear_captured();
        self.phase = Phase::Playing { seq };
        let pos = view.position_ms(seq).unwrap_or(0);
        self.feed.restart(pos);
        self.max_pos = pos;
        if let Some(ms) = self.pending_seek.take() {
            self.seek_to(seq, ms);
        }
    }

    fn seek_to(&mut self, seq: u64, ms: u64) {
        self.widget.send(Command::Seek { seq, ms });
        self.widget.clear_captured();
        self.phase = Phase::Seeking { seq, target_ms: ms, at: Instant::now() };
    }

    /// The track is over by the widget's state (`Err`: it was a preview).
    fn ended(&mut self, seq: u64, view: &Live) -> Result<bool, PcmError> {
        let pos = view.position_ms(seq).unwrap_or(self.max_pos);
        if view.status == Status::Playing {
            self.max_pos = self.max_pos.max(pos);
        }
        let known = self.known_ms(view);
        if view.finished == seq {
            if let Some(d) = self.duration.map(|d| d.as_millis() as u64)
                && self.max_pos + PREVIEW_SLACK_MS < d
            {
                log::info!("sc widget: finished at {} of {d} ms: only a preview", self.max_pos);
                return Err(err("only a preview played"));
            }
            return Ok(true);
        }
        Ok(known.is_some_and(|d| pos >= d + OVERRUN_MS))
    }
}

impl PcmSource for WidgetTrack {
    fn sample_rate(&self) -> u32 {
        RATE
    }

    fn channels(&self) -> u16 {
        CHANNELS
    }

    fn duration(&self) -> Option<Duration> {
        self.duration
    }

    fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
        let r = self.step(out);
        if r.is_err() {
            if let Some(t) = self.track {
                self.widget.refuse(t);
            }
            // Failed: it is done, and the widget stops.
            self.stop();
            self.phase = Phase::Ended;
        }
        r
    }

    fn seek(&mut self, position: Duration) -> bool {
        let ms = position.as_millis() as u64;
        match self.phase {
            Phase::Ended => false,
            Phase::Unrequested | Phase::Requested { .. } => {
                self.pending_seek = Some(ms);
                true
            }
            Phase::Playing { seq } | Phase::Seeking { seq, .. } => {
                self.seek_to(seq, ms);
                true
            }
        }
    }
}

impl WidgetTrack {
    /// [`PcmSource::read`]'s work.
    fn step(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
        let view = self.widget.view();
        match self.phase {
            Phase::Ended => return Ok(PcmRead::End),
            Phase::Unrequested => {
                let Some(track) = self.track else { return Err(err("not a SoundCloud track id")) };
                if !self.widget.usable() {
                    return Err(err("not available"));
                }
                let (seq, cold) = self.widget.load(track);
                self.widget.clear_captured();
                self.widget.playing(1);
                self.counted = true;
                self.phase = Phase::Requested { at: Instant::now(), seq, cold };
                return Ok(PcmRead::Pending);
            }
            Phase::Requested { at, seq, cold } => {
                self.widget.prepare_output();
                if let Some(why) = view.failure(seq) {
                    return Err(err(why));
                }
                let moving = view.seq == seq && view.status == Status::Playing && view.position.is_some_and(|(ms, _)| ms > 0);
                if moving {
                    log::info!("sc widget: load {seq} started after {} ms", at.elapsed().as_millis());
                    self.start_playing(seq, &view);
                } else if at.elapsed() > START_TIMEOUT + if cold { COLD_START } else { Duration::ZERO } {
                    return Err(err("it did not start"));
                } else {
                    return Ok(PcmRead::Pending);
                }
            }
            Phase::Seeking { seq, target_ms, at } => {
                if let Some(why) = view.failure(seq) {
                    return Err(err(why));
                }
                // A report that came after the seek, near where it went.
                let fresh = view.position.is_some_and(|(_, t)| t >= at);
                let pos = view.position_ms(seq).unwrap_or(0);
                let there = view.seq == seq && fresh && view.status == Status::Playing && pos.abs_diff(target_ms) < 2500;
                if there || at.elapsed() > SEEK_TIMEOUT {
                    self.widget.clear_captured();
                    self.phase = Phase::Playing { seq };
                    let from = if there { pos } else { target_ms };
                    self.feed.restart(from);
                    self.max_pos = from;
                } else {
                    return Ok(PcmRead::Pending);
                }
            }
            Phase::Playing { .. } => {}
        }
        let Phase::Playing { seq } = self.phase else { return Ok(PcmRead::Pending) };
        if let Some(why) = view.failure(seq) {
            return Err(err(why));
        }
        self.widget.prepare_output();
        if self.ended(seq, &view)? {
            self.phase = Phase::Ended;
            return Ok(PcmRead::End);
        }
        let pos = view.position_ms(seq).unwrap_or(self.max_pos);
        Ok(self.feed.produce(pos, &*self.widget, out))
    }

    /// Pauses this load in the widget unless the engine already paused it.
    fn stop(&mut self) {
        if let Phase::Requested { seq, .. } | Phase::Playing { seq } | Phase::Seeking { seq, .. } = self.phase
            && !self.widget.paused_by_us()
        {
            // Only this load: a newer one already asked for plays on.
            self.widget.send(Command::Pause { seq });
        }
    }
}

impl Drop for WidgetTrack {
    fn drop(&mut self) {
        self.stop();
        if self.counted {
            self.widget.playing(-1);
        }
    }
}
