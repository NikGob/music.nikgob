//! The audio callback: [`fastframe_audio::Render`] over a lock-free ring.
//!
//! Real-time rules: no locks, no allocation, no I/O, no logging. The decoder
//! thread feeds interleaved `f32` frames (already at the device rate and
//! channel count) into an SPSC ring and describes *where* things happen in
//! that stream with [`Mark`]s on a second SPSC queue. This is what makes
//! events and position sample-exact: a mark says "from sample `at` of the
//! stream onwards, item X plays", and the callback applies it when its own
//! read counter reaches `at`, in the middle of a block if need be.
//!
//! Two kinds of marks exist:
//! * a *cut* is a natural boundary inside the stream (gapless transition,
//!   crossfade start, end of the last item);
//! * a *flush* means everything before `at` is stale (seek, replace, stop,
//!   new device configuration): the callback fades out the stale audio over a
//!   few milliseconds, skips to `at`, and fades the new audio in.

use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use fastframe_audio::Render;
use rtrb::{Consumer, Producer};

use crate::shared::{CursorSnapshot, PlayClock, Shared, frames_to_ns};

/// Length of the de-click ramps around flushes.
const DECLICK_MS: u32 = 8;
/// Fade before the device is paused / after it resumes.
const PAUSE_FADE_MS: u32 = 12;
const RESUME_FADE_MS: u32 = 10;
/// Volume smoothing time constant.
const VOLUME_TAU_MS: f32 = 8.0;
/// Level above which the soft limiter starts to bend the signal (about -1 dBFS).
const LIMITER_KNEE: f32 = 0.89;

/// Which item starts playing, and where in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Start {
    pub id: u64,
    /// Position of the item's first sample of this stretch.
    pub pos_ns: u64,
    /// Tell the application (`Started` / `TrackChanged`). False for seeks
    /// and device reconfiguration.
    pub announce: bool,
}

/// A position in the sample stream where something changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Mark {
    /// Absolute index (in samples, all channels) in the pushed stream.
    pub at: u64,
    /// Everything before `at` is stale.
    pub flush: bool,
    /// Device configuration generation the stream was produced for.
    pub cfg_gen: u32,
    /// Flush generation, echoed back through the cursor once applied.
    pub gen_id: u64,
    pub ended: Option<u64>,
    pub start: Option<Start>,
}

/// What the callback tells the control thread.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CbEvent {
    Started(u64),
    Changed {
        from: u64,
        to: u64,
    },
    /// The item ended and nothing followed it.
    Ended(u64),
    /// The pause fade finished: the device may be paused now.
    PauseReady,
}

struct Pending {
    fade_end_at: u64,
    mark: Mark,
}

pub(crate) struct Renderer {
    ring: Consumer<f32>,
    marks: Consumer<Mark>,
    events: Producer<CbEvent>,
    sh: Arc<Shared>,
    clock: Arc<OnceLock<PlayClock>>,

    rate: u32,
    ch: usize,
    cfg_gen: u32,
    awaiting_sync: bool,
    /// Samples consumed (or skipped) from the ring so far.
    popped: u64,

    cur: Option<u64>,
    base_ns: u64,
    cur_frames: u64,
    moving: bool,
    applied_gen: u64,

    clock_base_ns: u64,
    rendered: u64,

    env: f32,
    env_target: f32,
    env_step: f32,
    pausing: bool,
    holding: bool,
    pending: Option<Pending>,

    vol: f32,
    vol_k: f32,
    declick_frames: u64,
    pause_frames: u64,
    resume_frames: u64,
}

impl Renderer {
    pub(crate) fn new(
        ring: Consumer<f32>,
        marks: Consumer<Mark>,
        events: Producer<CbEvent>,
        sh: Arc<Shared>,
        clock: Arc<OnceLock<PlayClock>>,
    ) -> Self {
        let vol = sh.volume_gain();
        Self {
            ring,
            marks,
            events,
            sh,
            clock,
            rate: 0,
            ch: 0,
            cfg_gen: 0,
            awaiting_sync: true,
            popped: 0,
            cur: None,
            base_ns: 0,
            cur_frames: 0,
            moving: false,
            applied_gen: 0,
            clock_base_ns: 0,
            rendered: 0,
            env: 0.0,
            env_target: 1.0,
            env_step: 1.0,
            pausing: false,
            holding: false,
            pending: None,
            vol,
            vol_k: 1.0,
            declick_frames: 1,
            pause_frames: 1,
            resume_frames: 1,
        }
    }

    /// The device was reconfigured and the decoder has not answered yet
    /// (offline harness).
    pub(crate) fn waiting_for_decoder(&self) -> bool {
        self.awaiting_sync && self.marks.slots() == 0
    }

    /// Samples currently readable from the ring (offline harness).
    pub(crate) fn ring_available(&self) -> usize {
        self.ring.slots()
    }

    fn push_event(&mut self, event: CbEvent) {
        // A full event queue means the control thread is gone or stuck;
        // dropping an event is better than blocking the audio thread.
        let _ = self.events.push(event);
    }

    fn publish_cursor(&self, offset_frames: usize) {
        let pos = self.base_ns + frames_to_ns(self.cur_frames, self.rate);
        let clock_ns =
            self.clock_base_ns + frames_to_ns(self.rendered + offset_frames as u64, self.rate);
        self.sh.cursor.write(CursorSnapshot {
            has_item: self.cur.is_some(),
            moving: self.moving && self.cur.is_some(),
            id: self.cur.unwrap_or(0),
            pos_ns: pos,
            clock_ns,
            applied_gen: self.applied_gen,
        });
    }

    fn update_control(&mut self) {
        let want_pause = self.sh.pause_req.load(Ordering::Acquire);
        if want_pause && !self.pausing {
            self.pausing = true;
            self.env_target = 0.0;
            self.env_step = self.env.max(0.001) / self.pause_frames as f32;
            if self.cur.is_none() || self.env <= 0.0 {
                self.env = 0.0;
                self.enter_hold();
            }
        } else if !want_pause && self.pausing {
            self.pausing = false;
            self.holding = false;
            self.env_target = 1.0;
            self.env_step = 1.0 / self.resume_frames as f32;
            self.moving = false;
        }
    }

    fn enter_hold(&mut self) {
        self.holding = true;
        self.moving = false;
        self.push_event(CbEvent::PauseReady);
        self.publish_cursor(0);
    }

    /// Applies queued flush marks. Only the newest matters: everything the
    /// older ones described is stale too.
    fn drain_flush_marks(&mut self) {
        loop {
            let queued = self.marks.slots();
            if queued == 0 {
                return;
            }
            let Ok(chunk) = self.marks.read_chunk(queued) else {
                return;
            };
            let (a, b) = chunk.as_slices();
            let mut last = None;
            for (i, m) in a.iter().chain(b.iter()).enumerate() {
                if m.flush && m.cfg_gen == self.cfg_gen {
                    last = Some((i, *m));
                }
            }
            match last {
                Some((i, mark)) => {
                    chunk.commit(i + 1);
                    self.apply_flush(mark);
                    // More may have arrived meanwhile; look again.
                }
                None => {
                    if self.awaiting_sync {
                        // Nothing matches the new configuration yet; all of
                        // it describes the old one.
                        chunk.commit_all();
                    }
                    return;
                }
            }
        }
    }

    fn apply_flush(&mut self, mark: Mark) {
        let immediate = if self.awaiting_sync {
            self.awaiting_sync = false;
            true
        } else {
            false
        };
        if !immediate {
            if let Some(p) = &mut self.pending {
                p.mark = mark;
                return;
            }
            let audible = self.env > 0.02
                && self.cur.is_some()
                && !self.pausing
                && !self.holding
                && mark.at > self.popped;
            if audible {
                let stale_frames = (mark.at - self.popped) / self.ch as u64;
                let fade = self.declick_frames.min(stale_frames);
                if fade > 0 {
                    self.env_target = 0.0;
                    self.env_step = self.env / fade as f32;
                    self.pending = Some(Pending {
                        fade_end_at: self.popped + fade * self.ch as u64,
                        mark,
                    });
                    return;
                }
            }
        }
        self.pending = None;
        self.do_flush(mark);
    }

    fn do_flush(&mut self, mark: Mark) {
        let stale = mark.at.saturating_sub(self.popped) as usize;
        let stale = stale.min(self.ring.slots());
        if stale > 0
            && let Ok(chunk) = self.ring.read_chunk(stale)
        {
            chunk.commit_all();
        }
        self.popped = self.popped.max(mark.at.min(self.popped + stale as u64));
        self.cur_frames = 0;
        self.moving = false;
        self.announce(mark.ended, mark.start);
        self.cur = mark.start.map(|s| s.id);
        self.base_ns = mark.start.map_or(0, |s| s.pos_ns);
        if mark.start.is_some() && !self.pausing && !self.holding {
            self.env = 0.0;
            self.env_target = 1.0;
            self.env_step = 1.0 / self.declick_frames as f32;
        }
        self.applied_gen = mark.gen_id;
        self.publish_cursor(0);
    }

    /// Emits the events a change of audible item implies.
    fn announce(&mut self, ended: Option<u64>, start: Option<Start>) {
        match (ended, start) {
            (Some(from), Some(s)) => self.push_event(CbEvent::Changed { from, to: s.id }),
            (Some(id), None) => self.push_event(CbEvent::Ended(id)),
            // Another item than the audible one is always a track change,
            // even on a reconfiguration flush: that flush swallowed the
            // boundary mark which would have announced it.
            (None, Some(s)) => match self.cur {
                Some(from) if from != s.id => self.push_event(CbEvent::Changed { from, to: s.id }),
                // The same id played anew (a restart, or the next source of
                // the same queue entry): a start the listener must hear of.
                Some(_) if s.announce => self.push_event(CbEvent::Started(s.id)),
                Some(_) => {}
                None if s.announce => self.push_event(CbEvent::Started(s.id)),
                None => {}
            },
            _ => {}
        }
    }

    fn apply_boundary(&mut self, mark: Mark, offset_frames: usize) {
        self.announce(mark.ended, mark.start);
        self.cur = mark.start.map(|s| s.id);
        self.base_ns = mark.start.map_or(0, |s| s.pos_ns);
        self.cur_frames = 0;
        if mark.start.is_none() {
            self.moving = false;
            self.sh
                .end_frame
                .store(self.rendered + offset_frames as u64, Ordering::Release);
        }
        self.publish_cursor(offset_frames);
    }

    /// The fade-out before a flush ran its course: perform the flush.
    fn finish_pending(&mut self) {
        if let Some(p) = self.pending.take() {
            self.do_flush(p.mark);
        }
    }

    /// Applies envelope, tap, volume and limiter to one segment of freshly
    /// read samples.
    fn shape(&mut self, seg: &mut [f32]) {
        let ch = self.ch;
        if self.env != 1.0 || self.env_target != 1.0 {
            for frame in seg.chunks_exact_mut(ch) {
                if self.env < self.env_target {
                    self.env = (self.env + self.env_step).min(self.env_target);
                } else if self.env > self.env_target {
                    self.env = (self.env - self.env_step).max(self.env_target);
                }
                for s in frame {
                    *s *= self.env;
                }
            }
        }
        // The visualizer sees the music as decoded, before the user's volume.
        self.sh.tap.write_interleaved(seg, ch);

        let target = self.sh.volume_gain();
        if (target - self.vol).abs() > 1e-6 {
            for frame in seg.chunks_exact_mut(ch) {
                self.vol += (target - self.vol) * self.vol_k;
                for s in frame {
                    *s = soft_limit(*s * self.vol);
                }
            }
            if (target - self.vol).abs() <= 1e-6 {
                self.vol = target;
            }
        } else {
            self.vol = target;
            for s in seg.iter_mut() {
                *s = soft_limit(*s * target);
            }
        }
    }
}

/// Transparent below the knee, smoothly bends towards +-1.0 above it.
#[inline]
fn soft_limit(x: f32) -> f32 {
    let a = x.abs();
    if a <= LIMITER_KNEE {
        x
    } else {
        let room = 1.0 - LIMITER_KNEE;
        (LIMITER_KNEE + room * ((a - LIMITER_KNEE) / room).tanh()).copysign(x)
    }
}

impl Render for Renderer {
    fn configure(&mut self, sample_rate: u32, channels: u16) {
        let changed = sample_rate != self.rate || usize::from(channels) != self.ch;
        // Frames already counted belong to the old rate: fold them into the
        // time base before the rate changes.
        if self.rate != 0 {
            self.base_ns += frames_to_ns(self.cur_frames, self.rate);
            self.cur_frames = 0;
        }
        self.rate = sample_rate;
        self.ch = usize::from(channels);
        self.clock_base_ns = self.clock.get().map_or(0, PlayClock::played_ns);
        self.rendered = 0;
        self.moving = false;
        let rate = sample_rate.max(1);
        self.declick_frames = u64::from(rate) * u64::from(DECLICK_MS) / 1000;
        self.pause_frames = u64::from(rate) * u64::from(PAUSE_FADE_MS) / 1000;
        self.resume_frames = u64::from(rate) * u64::from(RESUME_FADE_MS) / 1000;
        self.declick_frames = self.declick_frames.max(1);
        self.pause_frames = self.pause_frames.max(1);
        self.resume_frames = self.resume_frames.max(1);
        self.vol_k = 1.0 - (-1.0 / (rate as f32 * VOLUME_TAU_MS / 1000.0)).exp();
        self.sh.tap.set_sample_rate(sample_rate);
        if changed || self.cfg_gen == 0 {
            self.sh.cfg_rate.store(sample_rate, Ordering::Relaxed);
            self.sh
                .cfg_channels
                .store(u32::from(channels), Ordering::Relaxed);
            self.cfg_gen = self.sh.cfg_gen.fetch_add(1, Ordering::AcqRel) + 1;
            self.awaiting_sync = true;
            self.pending = None;
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        let ch = self.ch;
        if ch == 0 || self.rate == 0 {
            out.fill(0.0);
            return;
        }
        let frames = out.len() / ch;
        self.update_control();

        let mut w = 0usize;
        while w < frames {
            self.drain_flush_marks();
            if self.awaiting_sync || self.holding {
                break;
            }

            // How far can we copy before the next thing happens?
            let mut limit = u64::MAX;
            if let Some(p) = &self.pending {
                limit = p.fade_end_at.saturating_sub(self.popped);
                if limit == 0 {
                    self.finish_pending();
                    continue;
                }
            } else if let Ok(&mark) = self.marks.peek() {
                if mark.cfg_gen != self.cfg_gen {
                    let _ = self.marks.pop();
                    continue;
                }
                let until = mark.at.saturating_sub(self.popped);
                if until == 0 {
                    let _ = self.marks.pop();
                    self.apply_boundary(mark, w);
                    continue;
                }
                limit = until;
            }
            if self.pausing && self.env > 0.0 {
                // Stop exactly where the fade reaches silence.
                let to_zero = (self.env / self.env_step).ceil().max(1.0) as u64;
                limit = limit.min(to_zero * ch as u64);
            }

            let want = (frames - w) * ch;
            let mut n = want.min(usize::try_from(limit).unwrap_or(usize::MAX));
            n = n.min(self.ring.slots());
            n -= n % ch;
            if n == 0 {
                break;
            }
            let Ok(chunk) = self.ring.read_chunk(n) else {
                break;
            };
            let (a, b) = chunk.as_slices();
            let dst = &mut out[w * ch..w * ch + n];
            dst[..a.len()].copy_from_slice(a);
            dst[a.len()..].copy_from_slice(b);
            chunk.commit_all();
            self.popped += n as u64;

            let seg_frames = n / ch;
            if self.cur.is_some() && !self.moving {
                self.moving = true;
                self.publish_cursor(w);
            }
            self.shape(&mut out[w * ch..(w + seg_frames) * ch]);
            self.cur_frames += seg_frames as u64;
            w += seg_frames;

            if self.pausing && !self.holding && self.env <= 0.0 {
                self.enter_hold();
            }
        }

        if w < frames {
            out[w * ch..].fill(0.0);
            self.sh.tap.write_interleaved(&out[w * ch..frames * ch], ch);
            if self.cur.is_some() && !self.awaiting_sync && !self.holding && self.pending.is_none()
            {
                self.sh.underruns.fetch_add(1, Ordering::Relaxed);
            }
            if self.moving {
                // Time passes but the item does not: freeze its position.
                self.moving = false;
                self.publish_cursor(w);
            }
        } else if out.len() > frames * ch {
            out[frames * ch..].fill(0.0);
        }
        self.rendered += frames as u64;

        let pos = self.base_ns + frames_to_ns(self.cur_frames, self.rate);
        self.sh.consumed_pos_ns.store(pos, Ordering::Relaxed);
        self.sh
            .consumed_id
            .store(self.cur.unwrap_or(u64::MAX), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_is_transparent_then_bounded_and_monotonic() {
        assert_eq!(soft_limit(0.5), 0.5);
        assert_eq!(soft_limit(-0.88), -0.88);
        let mut prev = 0.0;
        for i in 0..200 {
            let x = i as f32 * 0.05;
            let y = soft_limit(x);
            assert!(y >= prev - 1e-6 && y <= 1.0, "{x} -> {y}");
            prev = y;
        }
        assert!(soft_limit(10.0) <= 1.0);
        assert!(soft_limit(-10.0) >= -1.0);
        assert!((soft_limit(0.89) - 0.89).abs() < 1e-6);
    }
}
