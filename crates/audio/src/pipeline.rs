//! The decoder thread: decodes, resamples, mixes (gapless / crossfade) and
//! feeds the ring the audio callback drains.
//!
//! Everything the callback needs to know about *where* things happen in the
//! sample stream is written to the mark queue before the samples themselves
//! (see [`crate::render`]).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use rtrb::Producer;

use crate::cancel::{Slot, cancelled};
use crate::channels;
use crate::decode::{self, FrameSource, PcmFrames, ReadStatus};
use crate::formant::Formant;
use crate::fx::Chain;
use crate::openers::{Openers, Start as Open};
use crate::render::{Mark, Start};
use crate::resample::StreamResampler;
use crate::shared::{LogLimiter, Shared, frames_to_ns, ns_to_frames};
use crate::types::{DecodeError, ItemFx, ItemSource, PlayItem, Speed, gcd};

/// Frames produced per step. Small enough that commands are answered
/// promptly, large enough to keep per-chunk overhead negligible.
const CHUNK_FRAMES: usize = 1024;
/// How much audio the decoder keeps ready in the ring. This is the headroom
/// against everything that can hold the decoder thread up: a stream read
/// waiting for the network, a slow disk, a busy CPU. Seeks, replaces and
/// stops flush the ring, so it costs no latency there; only a normalization
/// toggle is heard this much later.
const FILL_TARGET_MS: u64 = 1000;
/// One decoder step taking longer than this is logged (rate-limited): it
/// is what eats the headroom above and the evidence when the output ran dry.
const SLOW_STEP: Duration = Duration::from_millis(150);
/// At most one slow-step warning this often.
const SLOW_STEP_LOG_EVERY: Duration = Duration::from_secs(10);
/// How long a gain that changes while its item sounds takes to get there
/// ([`Player::set_gain`](crate::Player::set_gain)): long enough not to be
/// heard as a step.
const GAIN_GLIDE: Duration = Duration::from_millis(400);

thread_local! {
    /// The slot of the item whose opener runs on this thread.
    static OPENING: std::cell::RefCell<Option<Arc<Slot>>> = const { std::cell::RefCell::new(None) };
}

/// Runs a stream's opener with its slot known to [`open_wanted`].
fn opening(
    slot: Option<Arc<Slot>>,
    open: crate::types::StreamOpener,
) -> Result<Box<dyn crate::types::ByteSource>, String> {
    OPENING.with(|o| *o.borrow_mut() = slot);
    let opened = open();
    OPENING.with(|o| *o.borrow_mut() = None);
    opened
}

/// For a [`StreamOpener`](crate::StreamOpener), while it runs: whether the
/// item it opens is still wanted. A service lookup the opener waits for
/// (before the stream exists, so before an
/// [`interrupter`](crate::ByteSource::interrupter) can reach it) should
/// give up once this says false: the item was superseded (another play, a
/// stop, a new next item). Asked outside an opener, it always says true.
pub fn open_wanted() -> impl Fn() -> bool + Send + Sync + 'static {
    let slot = OPENING.with(|o| o.borrow().clone());
    move || !cancelled(slot.as_ref())
}

/// Commands for the decoder thread.
pub(crate) enum DecCmd {
    /// `slot`: see [`crate::cancel`] (none for a PCM source).
    Play {
        item: PlayItem,
        gen_id: u64,
        slot: Option<Arc<Slot>>,
    },
    SetNext(PlayItem, Option<Arc<Slot>>),
    ClearNext,
    /// `id` is the item the listener saw when asking; `woke`, the items
    /// the seek interrupted (see [`crate::cancel`]).
    Seek {
        pos: Duration,
        id: u64,
        gen_id: u64,
        woke: Vec<Arc<Slot>>,
    },
    Stop {
        gen_id: u64,
    },
    /// New speed, tone and reverb for item `id` (playing or prepared).
    SetFx {
        id: u64,
        fx: ItemFx,
        gen_id: u64,
    },
    /// A new loudness correction for item `id` (playing, prepared or
    /// opening).
    SetGain {
        id: u64,
        gain_db: f32,
    },
    /// An item (a stream, or a file set as next) finished opening on its
    /// opener thread.
    Opened {
        token: u64,
        result: Result<Item, (u64, DecodeError)>,
    },
    Shutdown,
}

/// What the decoder thread reports to the control thread.
pub(crate) enum DecEvent {
    /// An item was opened; its length is known now (if at all).
    Opened {
        id: u64,
        duration: Option<Duration>,
    },
    Error {
        id: Option<u64>,
        message: String,
    },
    /// A seek was ignored without a flush: the position it was waiting for
    /// will never be applied.
    SeekDropped {
        gen_id: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cfg {
    rate: u32,
    channels: usize,
    gen_id: u32,
}

/// An opened but not yet started item.
pub(crate) struct Item {
    id: u64,
    gain_db: f32,
    src: Box<dyn FrameSource>,
    src_rate: u32,
    src_channels: usize,
    /// Playable length in source frames, from the file or the caller's hint.
    total_src: Option<u64>,
    fx: ItemFx,
    speed: Speed,
    /// What the player knows about its reads (see [`crate::cancel`]).
    slot: Option<Arc<Slot>>,
}

impl Item {
    fn open(item: PlayItem, slot: Option<Arc<Slot>>) -> Result<Self, (u64, DecodeError)> {
        let id = item.id;
        let fx = item.fx;
        let mut hint = item.duration_hint;
        let mut gain_db = item.gain_db;
        let src: Box<dyn FrameSource> = match item.source {
            ItemSource::File(path) => Box::new(decode::open_file(&path).map_err(|e| (id, e))?),
            ItemSource::Pcm(pcm) => Box::new(PcmFrames::new(pcm).map_err(|e| (id, e))?),
            ItemSource::Stream(open) => {
                // Superseded while it waited for a thread: not even begun.
                if cancelled(slot.as_ref()) {
                    return Err((id, DecodeError::Stream("cancelled".into())));
                }
                let bytes = opening(slot.clone(), open).map_err(|m| (id, DecodeError::Stream(m)))?;
                // From here on a cancel reaches the reads of the probe too.
                if let Some(s) = &slot
                    && !s.attach(bytes.interrupter())
                {
                    return Err((id, DecodeError::Stream("cancelled".into())));
                }
                let (src, info) =
                    decode::open_stream(bytes, slot.clone()).map_err(|e| (id, e))?;
                gain_db = info.gain_db.unwrap_or(gain_db);
                hint = info.duration.or(hint);
                Box::new(src)
            }
        };
        let src_rate = src.sample_rate();
        // A zero length means "unknown" (an unset tag, a FLAC without a
        // sample count), not an empty item: it would clamp every seek to 0.
        let total_src = src
            .total_frames()
            .or_else(|| hint.map(|d| (d.as_secs_f64() * f64::from(src_rate)).round() as u64))
            .filter(|&n| n > 0);
        Ok(Self {
            id,
            gain_db: if gain_db.is_finite() {
                gain_db.clamp(-60.0, 30.0)
            } else {
                0.0
            },
            src_channels: src.channels(),
            src_rate,
            src,
            total_src,
            fx,
            speed: Speed::new(fx.speed),
            slot,
        })
        .inspect(|item| {
            if let Some(s) = &item.slot {
                s.mark_opened();
            }
        })
    }

    fn cancelled(&self) -> bool {
        cancelled(self.slot.as_ref())
    }

    /// A read of it failed because it was cancelled or interrupted (not
    /// the stream's own failure); the note is taken.
    fn gave_way(&self) -> bool {
        self.slot.as_ref().is_some_and(|s| s.take_gave_way())
    }

    /// A seek that interrupted it is on its way.
    fn interrupted(&self) -> bool {
        self.slot.as_ref().is_some_and(|s| s.interrupted())
    }

    /// It takes over as the next item; false when it was superseded.
    fn start(&self) -> bool {
        self.slot.as_ref().is_none_or(|s| s.start())
    }

    fn set_fx(&mut self, fx: ItemFx) {
        self.fx = fx;
        self.speed = Speed::new(fx.speed);
    }

    /// Source frames in and device frames out, as the smallest ratio: the
    /// source rate times the speed against the device rate. Everything
    /// that converts between the two timelines uses this one ratio, so the
    /// end of the item, the crossfade and the join to the next item fall
    /// where the sound really is.
    fn ratio(&self, device_rate: u32) -> (u64, u64) {
        let a = u64::from(self.src_rate) * u64::from(self.speed.num);
        let b = u64::from(device_rate) * u64::from(self.speed.den);
        let g = gcd(a, b);
        (a / g, b / g)
    }

    /// Whether the timbre stage runs for this item.
    fn keeps_formants(&self) -> bool {
        self.fx.keep_formants && !self.speed.is_one()
    }

    fn duration(&self) -> Option<Duration> {
        self.total_src
            .map(|f| Duration::from_secs_f64(f as f64 / f64::from(self.src_rate)))
    }
}

/// Decoded, resampled, channel-mapped audio of one item at the device format.
struct Stream {
    cfg: Cfg,
    /// Source frames in, device frames out (see [`Item::ratio`]).
    ratio: (u64, u64),
    resampler: Option<StreamResampler>,
    /// Puts the voice's timbre back after a speed change; like the
    /// resampler it runs on through a chained item, so the join stays
    /// continuous.
    post: Option<Formant>,
    /// Map to the device channel count before resampling (cheaper when
    /// downmixing).
    map_first: bool,
    raw: Vec<f32>,
    tmp: Vec<f32>,
    /// Device-format frames on their way into `post`.
    stage: Vec<f32>,
    /// Ready device-format frames and how many of them were handed out.
    ready: Vec<f32>,
    ready_pos: usize,
    src_done: bool,
    /// Source frames fed into the resampler since it was built (this item plus
    /// any chained follower).
    src_in: u64,
    /// Device frames handed out since the stream (or the last rebase) began.
    handed: u64,
    /// Device frames of the items before the last rebase, so a boundary
    /// computed from `src_in` is relative to the current item.
    out_base: u64,
    /// Where this item's own output ends once the stream has chained on
    /// into the next item.
    boundary: Option<u64>,
    /// The resampler is being fed from the next item's source, so it never
    /// sees a false silence at the join (gapless across a rate conversion).
    chained: bool,
    chain_id: Option<u64>,
}

enum StreamRead {
    /// This many frames were appended.
    Frames(usize),
    Pending,
    Eof,
}

impl Stream {
    fn new(item: &Item, cfg: Cfg) -> Result<Self, DecodeError> {
        let ratio = item.ratio(cfg.rate);
        let needs_resample = ratio.0 != ratio.1;
        let map_first = item.src_channels > cfg.channels;
        let rs_channels = if map_first {
            cfg.channels
        } else {
            item.src_channels
        };
        let resampler = if needs_resample {
            let narrow = |v: u64| {
                u32::try_from(v)
                    .map_err(|_| DecodeError::Unsupported("sample rate ratio too large".into()))
            };
            // As rates, not as the reduced ratio: the resampler sizes its
            // filter from them.
            let rate_in = u64::from(item.src_rate) * u64::from(item.speed.num);
            let rate_out = u64::from(cfg.rate) * u64::from(item.speed.den);
            Some(StreamResampler::new(
                narrow(rate_in)?,
                narrow(rate_out)?,
                rs_channels,
            )?)

        } else {
            None
        };
        let post = (item.keeps_formants() && needs_resample).then(|| {
            let speed = item.speed.num as f32 / item.speed.den as f32;
            Formant::new(cfg.rate, cfg.channels, speed)
        });
        Ok(Self {
            cfg,
            ratio,
            resampler,
            post,
            map_first,
            raw: Vec::new(),
            tmp: Vec::new(),
            stage: Vec::new(),
            ready: Vec::new(),
            ready_pos: 0,
            src_done: false,
            src_in: 0,
            handed: 0,
            out_base: 0,
            boundary: None,
            chained: false,
            chain_id: None,
        })
    }

    fn ready_frames(&self) -> usize {
        (self.ready.len() - self.ready_pos) / self.cfg.channels
    }

    /// The stream now belongs to the item it chained on into.
    fn rebase(&mut self) {
        // `src_in` keeps counting: part of the new item was already fed to
        // the resampler while it was chained on.
        self.out_base += self.boundary.unwrap_or(self.handed);
        self.handed = 0;
        self.boundary = None;
        self.chained = false;
        self.chain_id = None;
    }

    /// Converts freshly decoded source frames and queues them.
    fn ingest(&mut self, src_channels: usize) -> Result<(), DecodeError> {
        let dev = self.cfg.channels;
        self.src_in += (self.raw.len() / src_channels) as u64;
        let Self {
            resampler,
            post,
            map_first,
            raw,
            tmp,
            stage,
            ready,
            ..
        } = self;
        // With the timbre stage on, the converted audio passes through it.
        let dst: &mut Vec<f32> = if post.is_some() {
            stage.clear();
            &mut *stage
        } else {
            &mut *ready
        };
        match resampler {
            None => channels::convert(raw, src_channels, dev, dst),
            Some(rs) if *map_first => {
                tmp.clear();
                channels::convert(raw, src_channels, dev, tmp);
                rs.process(tmp, dst)?;
            }
            Some(rs) => {
                tmp.clear();
                rs.process(raw, tmp)?;
                channels::convert(tmp, src_channels, dev, dst);
            }
        }
        if let Some(post) = post {
            post.process(stage, ready);
        }
        raw.clear();
        Ok(())
    }

    fn flush(&mut self, src_channels: usize) -> Result<(), DecodeError> {
        let dev = self.cfg.channels;
        let Self {
            resampler,
            post,
            map_first,
            tmp,
            stage,
            ready,
            ..
        } = self;
        let dst: &mut Vec<f32> = if post.is_some() {
            stage.clear();
            &mut *stage
        } else {
            &mut *ready
        };
        if let Some(rs) = resampler {
            if *map_first {
                rs.finish(dst)?;
            } else {
                tmp.clear();
                rs.finish(tmp)?;
                channels::convert(tmp, src_channels, dev, dst);
            }
        }
        if let Some(post) = post {
            post.process(stage, ready);
            post.finish(ready);
        }
        self.src_done = true;
        Ok(())
    }

    /// Appends up to `want` frames to `out`. With `chain`, the end of `item`
    /// flows straight into the start of the next item (same rate, channels
    /// and speed) through the same resampler. Items at different speeds
    /// need different resamplers: they join as two flushed streams, with
    /// no silence between them.
    fn read(
        &mut self,
        item: &mut Item,
        mut chain: Option<&mut Item>,
        want: usize,
        out: &mut Vec<f32>,
    ) -> Result<StreamRead, DecodeError> {
        loop {
            let limit = self.boundary.map(|b| b.saturating_sub(self.handed));
            let target = limit.map_or(want, |l| want.min(l as usize));
            if self.ready_frames() >= target || self.src_done {
                break;
            }
            let status = if self.chained {
                match chain.as_deref_mut() {
                    Some(next) => next.src.read(&mut self.raw)?,
                    None => ReadStatus::Eof,
                }
            } else {
                item.src.read(&mut self.raw)?
            };
            match status {
                ReadStatus::Data => self.ingest(item.src_channels)?,
                ReadStatus::Pending => {
                    if self.ready_frames() > 0 {
                        break;
                    }
                    return Ok(StreamRead::Pending);
                }
                ReadStatus::Eof => {
                    let next = chain.as_deref();
                    let can_chain = !self.chained
                        && self.resampler.is_some()
                        && next.is_some_and(|n| {
                            n.src_rate == item.src_rate
                                && n.src_channels == item.src_channels
                                && n.speed == item.speed
                                && n.keeps_formants() == item.keeps_formants()
                        });
                    if can_chain {
                        let rs_in = u128::from(self.src_in) * u128::from(self.ratio.1);
                        let end = rs_in.div_ceil(u128::from(self.ratio.0)) as u64;
                        self.boundary = Some(end.saturating_sub(self.out_base));
                        self.chained = true;
                        self.chain_id = next.map(|n| n.id);
                    } else {
                        self.flush(item.src_channels)?;
                    }
                }
            }
        }
        let limit = self.boundary.map(|b| b.saturating_sub(self.handed));
        let target = limit.map_or(want, |l| want.min(l as usize));
        let n = self.ready_frames().min(target);
        if n == 0 {
            return Ok(if self.src_done || limit == Some(0) {
                StreamRead::Eof
            } else {
                StreamRead::Pending
            });
        }
        let ch = self.cfg.channels;
        out.extend_from_slice(&self.ready[self.ready_pos..self.ready_pos + n * ch]);
        self.ready_pos += n * ch;
        self.handed += n as u64;
        if self.ready_pos >= self.ready.len() / 2 {
            self.ready.drain(..self.ready_pos);
            self.ready_pos = 0;
        }
        Ok(StreamRead::Frames(n))
    }
}

/// An item being produced.
struct Active {
    item: Item,
    stream: Option<Stream>,
    /// Source frame the stream (re)started from.
    start_src: u64,
    /// Device frames handed out since then.
    produced: u64,
    /// The item's tone and reverb, built for `fx_cfg` on the first read.
    fx: Option<Chain>,
    fx_cfg: Option<Cfg>,
    /// The item is over and its reverb plays out: frames left, and of how
    /// many.
    ring: Option<(u64, u64)>,
    /// A change of the item's gain gliding in ([`Self::set_gain_db`]).
    glide: Option<Glide>,
    /// A read failed because a seek interrupted it, and the seek did not
    /// come: the demuxer may have stopped inside a packet, so the item
    /// starts again where the listener is before it is read on.
    stalled: bool,
}

/// A gain change on its way: from `from_db` to the item's own gain over
/// `len` device frames, `done` of them out so far.
#[derive(Clone, Copy, Debug)]
struct Glide {
    from_db: f32,
    done: u64,
    len: u64,
}

impl Active {
    fn new(item: Item) -> Self {
        Self {
            item,
            stream: None,
            start_src: 0,
            produced: 0,
            fx: None,
            fx_cfg: None,
            ring: None,
            glide: None,
            stalled: false,
        }
    }

    /// The gain in dB `done` frames into the glide (the item's own when
    /// nothing glides).
    fn gain_db_at(&self, done: u64) -> f32 {
        match self.glide {
            Some(g) => {
                let t = (done as f32 / g.len.max(1) as f32).min(1.0);
                g.from_db + (self.item.gain_db - g.from_db) * t
            }
            None => self.item.gain_db,
        }
    }

    /// The item's gain becomes `gain_db`: at once when `glide` is false
    /// (nothing of it is out yet), else from where it is now over
    /// [`GAIN_GLIDE`] at `rate`.
    fn set_gain_db(&mut self, gain_db: f32, rate: u32, glide: bool) {
        let now = self.gain_db_at(self.glide.map_or(0, |g| g.done));
        self.item.gain_db = gain_db;
        let len = (u128::from(rate.max(1)) * GAIN_GLIDE.as_millis() / 1000) as u64;
        self.glide = (glide && (now - gain_db).abs() > 0.01).then_some(Glide {
            from_db: now,
            done: 0,
            len: len.max(1),
        });
    }

    /// The linear gain at the start and at the end of the next `frames`
    /// frames (the same unless a change glides); the glide moves on by them.
    fn gains(&mut self, normalization: bool, frames: usize) -> (f32, f32) {
        let Some(g) = self.glide else {
            let gain = self.gain(normalization);
            return (gain, gain);
        };
        let end = g.done + frames as u64;
        let linear = |db: f32| {
            if normalization {
                10f32.powf(db / 20.0)
            } else {
                1.0
            }
        };
        let pair = (
            linear(self.gain_db_at(g.done)),
            linear(self.gain_db_at(end)),
        );
        self.glide = (end < g.len).then_some(Glide { done: end, ..g });
        pair
    }

    fn id(&self) -> u64 {
        self.item.id
    }

    /// Builds the conversion stream for `cfg` if there is none.
    fn prepare(&mut self, cfg: Cfg) -> Result<(), DecodeError> {
        if self.stream.as_ref().is_none_or(|s| s.cfg != cfg) {
            self.stream = Some(Stream::new(&self.item, cfg)?);
        }
        Ok(())
    }

    /// Appends up to `want` device-format frames to `out`.
    fn read(
        &mut self,
        cfg: Cfg,
        want: usize,
        out: &mut Vec<f32>,
        chain: Option<&mut Item>,
    ) -> Result<StreamRead, DecodeError> {
        if self.ring.is_some() {
            return Ok(self.read_ring(cfg, want, out));
        }
        self.prepare(cfg)?;
        let Some(stream) = self.stream.as_mut() else {
            return Ok(StreamRead::Eof);
        };
        let from = out.len();
        let r = stream.read(&mut self.item, chain, want, out)?;
        if let StreamRead::Frames(n) = r {
            self.produced += n as u64;
            self.apply_fx(cfg, &mut out[from..]);
        }
        Ok(r)
    }

    /// Runs the item's tone and reverb over freshly read frames (or over
    /// silence after its end, which lets the reverb ring on).
    fn apply_fx(&mut self, cfg: Cfg, buf: &mut [f32]) {
        if self.fx_cfg != Some(cfg) {
            self.fx = Chain::new(&self.item.fx, cfg.rate, cfg.channels);
            self.fx_cfg = Some(cfg);
        }
        if let Some(fx) = &mut self.fx {
            fx.process(buf);
        }
    }

    /// The source is over and nothing follows: lets a reverb play out
    /// instead of cutting it. False when there is nothing to play out (or
    /// it already did).
    fn start_ring_out(&mut self) -> bool {
        if self.ring.is_some() {
            return false;
        }
        let Some(fx) = self.fx.as_ref().filter(|fx| fx.has_long_tail()) else {
            return false;
        };
        let frames = fx.tail_frames();
        self.ring = Some((frames, frames));
        frames > 0
    }

    fn ringing(&self) -> bool {
        self.ring.is_some()
    }

    /// The next frames of the reverb playing out, eased to nothing at its
    /// end.
    fn read_ring(&mut self, cfg: Cfg, want: usize, out: &mut Vec<f32>) -> StreamRead {
        let (Some((left, total)), Some(fx)) = (self.ring, self.fx.as_mut()) else {
            return StreamRead::Eof;
        };
        let n = (want as u64).min(left) as usize;
        if n == 0 || self.fx_cfg != Some(cfg) {
            return StreamRead::Eof;
        }
        let ch = cfg.channels;
        let from = out.len();
        out.resize(from + n * ch, 0.0);
        fx.process(&mut out[from..]);
        for (i, frame) in out[from..].chunks_exact_mut(ch).enumerate() {
            let g = (left - i as u64) as f32 / total as f32;
            for s in frame {
                *s *= g;
            }
        }
        self.ring = Some((left - n as u64, total));
        self.produced += n as u64;
        StreamRead::Frames(n)
    }

    /// Positions the source at `src_frame` and drops buffered audio.
    fn reposition(&mut self, src_frame: u64) -> Result<(), DecodeError> {
        self.item.src.seek(src_frame)?;
        self.stalled = false;
        self.stream = None;
        self.start_src = src_frame;
        self.produced = 0;
        self.fx = None;
        self.fx_cfg = None;
        self.ring = None;
        Ok(())
    }

    /// Back to its start: a next item the stream had read ahead into. One
    /// that cannot get there now starts over when it takes over.
    fn rewind(&mut self) {
        if self.reposition(0).is_err() {
            self.item.gave_way();
            self.stalled = true;
        }
    }

    /// Device frames from the item's end, if its length is known.
    fn remaining(&self, cfg: Cfg) -> Option<u64> {
        let total = self.item.total_src?;
        let (src, dev) = self.item.ratio(cfg.rate);
        let total_dev = (u128::from(total) * u128::from(dev)).div_ceil(u128::from(src));
        let start_dev = u128::from(self.start_src) * u128::from(dev) / u128::from(src);
        let done = start_dev + u128::from(self.produced);
        Some(u64::try_from(total_dev.saturating_sub(done)).unwrap_or(u64::MAX))
    }

    /// What starts when this item does, at `pos_ns` of its timeline.
    fn start_at(&self, pos_ns: u64, announce: bool) -> Start {
        Start {
            id: self.item.id,
            pos_ns,
            announce,
            speed: self.item.speed,
        }
    }

    fn gain(&self, normalization: bool) -> f32 {
        if normalization {
            10f32.powf(self.item.gain_db / 20.0)
        } else {
            1.0
        }
    }
}

struct Crossfade {
    to: Active,
    len: u64,
    done: u64,
}

/// How the item being produced began in the pushed stream. The decoder
/// runs about a second ahead of the listener, so "the current item" here
/// may not be heard yet, with the end of the one before it still in the
/// ring: what may be done to it depends on how it got there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Began {
    /// After a flush (a play, a seek): nothing older is worth keeping.
    Flush,
    /// At a gapless join, at this index of the pushed stream.
    Boundary(u64),
    /// Inside a crossfade: its start is mixed with the item before it.
    Mixed,
}

/// The tone and reverb of an item that ended at a gapless join, still
/// sounding under what follows. It is fed silence; being linear, what it
/// gives added to the next item's own chain is exactly the uncut sound.
struct Tail {
    fx: Chain,
    gain_db: f32,
    left: u64,
    total: u64,
}

pub(crate) struct Decoder {
    rx: Receiver<DecCmd>,
    /// Where opener threads post [`DecCmd::Opened`].
    tx: Sender<DecCmd>,
    /// Last token handed to an opener thread.
    open_seq: u64,
    /// The opener threads.
    openers: Openers,
    /// A `Play` of a stream waiting for its opener: `(token, gen_id, item id)`.
    pending_play: Option<(u64, u64, u64)>,
    /// The slot of that item.
    pending_play_slot: Option<Arc<Slot>>,
    /// Tokens of superseded opens no longer counted in flight.
    released: Vec<u64>,
    /// A seek of that item asked for meanwhile: `(position, gen_id)`.
    pending_seek: Option<(Duration, u64)>,
    /// A `SetNext` of a file or stream waiting for its opener (token). While
    /// it is set, the current item does not end into nothing: it waits.
    pending_next: Option<u64>,
    /// The slot of that item.
    pending_next_slot: Option<Arc<Slot>>,
    /// That pending next item is a local file: it opens in moments, so the
    /// current item holds just short of its end for it (see
    /// [`Self::hold_for_next`]).
    pending_next_file: bool,
    /// Id of that pending next item.
    pending_next_id: Option<u64>,
    /// A change of speed, tone and reverb asked for an item that is still
    /// opening: applied when it is open.
    pending_fx: Option<(u64, ItemFx)>,
    /// A gain asked for an item that is still opening: its gain when open.
    pending_gain: Option<(u64, f32)>,
    events: Sender<DecEvent>,
    sh: Arc<Shared>,
    ring: Producer<f32>,
    ring_capacity: usize,
    marks: Producer<Mark>,
    pushed: u64,
    cfg: Cfg,
    fill_target: usize,
    last_gen: u64,
    /// Generation of the newest `Play`; a flush that reconfigures the device
    /// must announce the item if the callback never applied that play.
    play_gen: u64,

    cur: Option<Active>,
    next: Option<Active>,
    xfade: Option<Crossfade>,
    tail: Option<Tail>,
    cur_began: Began,
    /// A change of speed, tone and reverb for the item being produced that
    /// has to wait until the listener reaches it (see [`Self::set_fx`]).
    deferred_fx: Option<(u64, ItemFx)>,

    buf: Vec<f32>,
    buf_b: Vec<f32>,
    /// Wait only briefly next time: a live source had nothing yet.
    short_wait: bool,
    /// Rate limit of the slow-step warning: when it last went out, and the
    /// slow steps since then.
    slow_log: LogLimiter,
}

enum Step {
    Worked,
    Idle,
}

impl Decoder {
    pub(crate) fn new(
        rx: Receiver<DecCmd>,
        tx: Sender<DecCmd>,
        events: Sender<DecEvent>,
        sh: Arc<Shared>,
        ring: Producer<f32>,
        marks: Producer<Mark>,
    ) -> Self {
        let ring_capacity = ring.buffer().capacity();
        Self {
            rx,
            tx,
            open_seq: 0,
            openers: Openers::default(),
            pending_play: None,
            pending_play_slot: None,
            released: Vec::new(),
            pending_seek: None,
            pending_next: None,
            pending_next_slot: None,
            pending_next_file: false,
            pending_next_id: None,
            pending_fx: None,
            pending_gain: None,
            events,
            sh,
            ring,
            ring_capacity,
            marks,
            pushed: 0,
            cfg: Cfg::default(),
            fill_target: 0,
            last_gen: 0,
            play_gen: 0,
            cur: None,
            next: None,
            xfade: None,
            tail: None,
            cur_began: Began::Flush,
            deferred_fx: None,
            buf: Vec::with_capacity(CHUNK_FRAMES * 2),
            buf_b: Vec::with_capacity(CHUNK_FRAMES * 2),
            short_wait: false,
            slow_log: LogLimiter::new(SLOW_STEP_LOG_EVERY),
        }
    }

    pub(crate) fn run(mut self) {
        // Above the background work (sound analysis, torrent hashing) that
        // could otherwise hold the decoder up until the ring runs dry.
        if let Err(e) =
            thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max)
        {
            log::debug!("audio: decoder thread priority: {e:?}");
        }
        // How late the thread woke from its last idle wait, and how much
        // audio was ready before that wait.
        let mut woke_late = Duration::ZERO;
        let mut buffered_before = None;
        loop {
            loop {
                match self.rx.try_recv() {
                    Ok(cmd) => {
                        if !self.handle(cmd) {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            self.check_config();
            self.try_deferred_fx();
            let buffered = buffered_before.take().unwrap_or_else(|| self.buffered_ms());
            let began = Instant::now();
            let step = self.pump();
            self.note_step_time(began, std::mem::take(&mut woke_late), buffered);
            self.sh.decoder_idle.store(
                self.cur.is_none() && self.xfade.is_none(),
                Ordering::Release,
            );
            if matches!(step, Step::Idle) {
                let wait = if self.short_wait {
                    Duration::from_millis(2)
                } else if self.cur.is_some() {
                    Duration::from_millis(8)
                } else {
                    Duration::from_millis(25)
                };
                buffered_before = Some(self.buffered_ms());
                let waited = Instant::now();
                match self.rx.recv_timeout(wait) {
                    Ok(cmd) => {
                        if !self.handle(cmd) {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        // A starved thread wakes long after it asked to:
                        // that eats the headroom as surely as a slow step.
                        woke_late = waited.elapsed().saturating_sub(wait);
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }

    // ----- commands -------------------------------------------------------

    /// Returns `false` to shut down.
    fn handle(&mut self, cmd: DecCmd) -> bool {
        let counted = !matches!(cmd, DecCmd::Shutdown);
        let keep_going = self.dispatch(cmd);
        self.sh.decoder_idle.store(
            self.cur.is_none() && self.xfade.is_none(),
            Ordering::Release,
        );
        if counted {
            self.sh.inflight.fetch_sub(1, Ordering::AcqRel);
        }
        keep_going
    }

    fn dispatch(&mut self, cmd: DecCmd) -> bool {
        match cmd {
            DecCmd::Play { item, gen_id, slot } => self.play(item, gen_id, slot),
            DecCmd::SetNext(item, slot) => self.set_next(item, slot),
            DecCmd::ClearNext => {
                self.next = None;
                self.release_next();
                self.pending_next_slot = None;
            }
            DecCmd::Seek {
                pos,
                id,
                gen_id,
                woke,
            } => {
                // Taken: the reads it interrupted wait again, unless a
                // newer seek is on its way too.
                for s in &woke {
                    s.end_interrupt();
                }
                self.seek(pos, id, gen_id);
            }
            DecCmd::Stop { gen_id } => {
                self.release_play();
                self.pending_play_slot = None;
                self.drop_pending_seek();
                self.release_next();
                self.pending_next_slot = None;
                self.last_gen = gen_id;
                self.cur = None;
                self.next = None;
                self.xfade = None;
                self.push_flush(gen_id, None);
            }
            DecCmd::SetFx { id, fx, gen_id } => self.set_fx(id, fx, gen_id),
            DecCmd::SetGain { id, gain_db } => self.set_gain(id, gain_db),
            DecCmd::Opened { token, result } => {
                if let Some(i) = self.released.iter().position(|t| *t == token) {
                    // No longer counted (see `release`): `handle` counts it
                    // off once more.
                    self.released.swap_remove(i);
                    self.sh.inflight.fetch_add(1, Ordering::AcqRel);
                }
                self.opened(token, result);
            }
            DecCmd::Shutdown => return false,
        }
        true
    }

    /// Opens an item on an opener thread (see [`crate::openers`]); the
    /// result comes back as [`DecCmd::Opened`] with the returned token.
    /// Counted as a command in flight until then (the offline device waits
    /// for it), or until it turns out it will not open (cancelled first).
    fn open_async(&mut self, item: PlayItem, slot: Option<Arc<Slot>>) -> u64 {
        self.open_seq += 1;
        let token = self.open_seq;
        let id = item.id;
        self.sh.inflight.fetch_add(1, Ordering::AcqRel);
        let tx = self.tx.clone();
        let sh = Arc::clone(&self.sh);
        let job_slot = slot.clone();
        self.openers.submit(
            slot,
            Box::new(move |start| {
                let result = match start {
                    Open::Run => Item::open(item, job_slot),
                    Open::Skip => Err((id, DecodeError::Stream("cancelled".into()))),
                    Open::Failed(e) => Err((
                        id,
                        DecodeError::Stream(format!("cannot start opener: {e}")),
                    )),
                };
                if tx.send(DecCmd::Opened { token, result }).is_err() {
                    sh.inflight.fetch_sub(1, Ordering::AcqRel);
                }
            }),
        );
        token
    }

    /// The pending play's open is superseded (see [`Self::release`]).
    fn release_play(&mut self) {
        if let Some((token, _, _)) = self.pending_play.take() {
            self.release(token);
        }
    }

    /// The pending next item's open is superseded (see [`Self::release`]).
    fn release_next(&mut self) {
        if let Some(token) = self.pending_next.take() {
            self.release(token);
        }
    }

    /// An open superseded is no longer a command in flight: the offline
    /// device does not wait for it (its result is dropped when it comes,
    /// whenever an open that cannot be interrupted returns).
    fn release(&mut self, token: u64) {
        self.sh.inflight.fetch_sub(1, Ordering::AcqRel);
        self.released.push(token);
    }

    /// A stream finished opening: start it, or make it the next item, if it
    /// is still wanted.
    fn opened(&mut self, token: u64, result: Result<Item, (u64, DecodeError)>) {
        if let Some((want, gen_id, _)) = self.pending_play
            && want == token
        {
            self.pending_play = None;
            let slot = self.pending_play_slot.take();
            let seek = self.pending_seek.take();
            if cancelled(slot.as_ref()) {
                // Superseded by a play or a stop that is on its way: not
                // started, nothing reported.
                if let Some((_, seek_gen)) = seek {
                    let _ = self.events.send(DecEvent::SeekDropped { gen_id: seek_gen });
                }
                return;
            }
            match result {
                Ok(mut item) => {
                    let id = item.id;
                    if let Some((_, fx)) = self.pending_fx.take_if(|(want, _)| *want == id) {
                        item.set_fx(fx);
                    }
                    if let Some((_, gain)) = self.pending_gain.take_if(|(want, _)| *want == id) {
                        item.gain_db = gain;
                    }
                    self.start_item(item, gen_id);
                    if let Some((pos, seek_gen)) = seek {
                        self.seek(pos, id, seek_gen);
                    }
                }
                Err((id, e)) => {
                    self.report(Some(id), &e);
                    if let Some((_, seek_gen)) = seek {
                        let _ = self.events.send(DecEvent::SeekDropped { gen_id: seek_gen });
                    }
                }
            }
        } else if self.pending_next == Some(token) {
            self.pending_next = None;
            if !cancelled(self.pending_next_slot.take().as_ref()) {
                self.accept_next(result);
            }
        }
        // Anything else was superseded meanwhile and is dropped.
    }

    fn report(&self, id: Option<u64>, error: &DecodeError) {
        let _ = self.events.send(DecEvent::Error {
            id,
            message: error.to_string(),
        });
    }

    fn play(&mut self, item: PlayItem, gen_id: u64, slot: Option<Arc<Slot>>) {
        self.release_play();
        self.pending_play_slot = None;
        self.release_next();
        self.pending_next_slot = None;
        self.pending_fx = None;
        self.pending_gain = None;
        self.drop_pending_seek();
        if item.source.is_stream() {
            // Cut what plays now and start the stream once it is open. The
            // silence keeps the old generation, so until the start the
            // player reports the requested item at its start (and a seek of
            // it waits for it, see `seek`).
            self.xfade = None;
            self.next = None;
            self.cur = None;
            self.push_flush(self.last_gen, None);
            let id = item.id;
            let token = self.open_async(item, slot.clone());
            self.pending_play = Some((token, gen_id, id));
            self.pending_play_slot = slot;
            return;
        }
        match Item::open(item, slot) {
            Ok(item) => {
                self.next = None;
                self.start_item(item, gen_id);
            }
            Err((id, e)) => self.report(Some(id), &e),
        }
    }

    fn start_item(&mut self, item: Item, gen_id: u64) {
        let _ = self.events.send(DecEvent::Opened {
            id: item.id,
            duration: item.duration(),
        });
        let active = Active::new(item);
        let start = active.start_at(0, true);
        self.last_gen = gen_id;
        self.play_gen = gen_id;
        self.xfade = None;
        self.cur = Some(active);
        self.push_flush(gen_id, Some(start));
    }

    fn set_next(&mut self, item: PlayItem, slot: Option<Arc<Slot>>) {
        // A change asked for the next item that was opening is over with it
        // (the new item brings its own).
        if self
            .pending_fx
            .is_some_and(|(id, _)| self.pending_play.is_none_or(|(_, _, playing)| playing != id))
        {
            self.pending_fx = None;
        }
        self.pending_gain = self.pending_gain.filter(|(id, _)| {
            self.pending_play
                .is_some_and(|(_, _, playing)| playing == *id)
        });
        // Files open on the opener thread too: probing one (a cold disk, a
        // virus scanner, a big embedded cover) can take longer than the
        // buffered audio, and this runs while the current item plays.
        if !matches!(item.source, ItemSource::Pcm(_)) {
            self.next = None;
            self.pending_next_file = matches!(item.source, ItemSource::File(_));
            self.pending_next_id = Some(item.id);
            self.release_next();
            self.pending_next = Some(self.open_async(item, slot.clone()));
            self.pending_next_slot = slot;
            return;
        }
        self.release_next();
        self.pending_next_slot = None;
        self.accept_next(Item::open(item, slot));
    }

    fn accept_next(&mut self, result: Result<Item, (u64, DecodeError)>) {
        match result {
            Ok(mut item) => {
                if let Some((_, fx)) = self.pending_fx.take_if(|(want, _)| *want == item.id) {
                    item.set_fx(fx);
                }
                if let Some((_, gain)) = self.pending_gain.take_if(|(want, _)| *want == item.id) {
                    item.gain_db = gain;
                }
                let _ = self.events.send(DecEvent::Opened {
                    id: item.id,
                    duration: item.duration(),
                });
                self.next = Some(Active::new(item));
            }
            Err((id, e)) => {
                self.next = None;
                self.report(Some(id), &e);
            }
        }
    }

    fn drop_pending_seek(&mut self) {
        if let Some((_, gen_id)) = self.pending_seek.take() {
            let _ = self.events.send(DecEvent::SeekDropped { gen_id });
        }
    }

    fn seek(&mut self, pos: Duration, id: u64, gen_id: u64) {
        if self.pending_play.is_some_and(|(_, _, want)| want == id) {
            // The stream is still opening: seek once it is there.
            self.drop_pending_seek();
            self.pending_seek = Some((pos, gen_id));
            return;
        }
        // The callback trails the decoder by up to ~1 s, so the item the
        // listener saw may already be over here. Seeking its successor to a
        // position meant for another track would jump the wrong item (and
        // switch to it without an event).
        let producing = self
            .xfade
            .as_ref()
            .map(|x| x.to.id())
            .or_else(|| self.cur.as_ref().map(Active::id));
        if producing != Some(id) {
            let _ = self.events.send(DecEvent::SeekDropped { gen_id });
            return;
        }
        // During a crossfade the audible item is the incoming one.
        if let Some(x) = self.xfade.take() {
            self.cur = Some(x.to);
        }
        let Some(cur) = self.cur.as_mut() else {
            return;
        };
        let mut target = (pos.as_secs_f64() * f64::from(cur.item.src_rate)).round() as u64;
        if let Some(total) = cur.item.total_src {
            target = target.min(total);
        }
        let id = cur.id();
        let start = cur.start_at(frames_to_ns(target, cur.item.src_rate), false);
        let was_chained = cur.stream.as_ref().is_some_and(|s| s.chained);
        let result = cur.reposition(target);
        if was_chained {
            // The resampler had read ahead into the next item: rewind it.
            if let Some(next) = self.next.as_mut() {
                next.rewind();
            }
        }
        match result {
            Ok(()) => {
                self.last_gen = gen_id;
                self.push_flush(gen_id, Some(start));
            }
            Err(_) if cur.item.gave_way() => {
                // A newer seek, a play or a stop is on its way and made
                // the reposition give way: that one decides.
                cur.stalled = true;
            }
            Err(e) => {
                self.report(Some(id), &e);
                // The source may have moved before failing: start over from
                // where the listener is, so state stays consistent.
                self.recover_after_failed_seek(gen_id);
            }
        }
    }

    fn recover_after_failed_seek(&mut self, gen_id: u64) {
        let sh = &self.sh;
        let Some(cur) = self.cur.as_mut() else { return };
        let id = cur.id();
        let pos_ns = if sh.consumed_id.load(Ordering::Relaxed) == id {
            sh.consumed_pos_ns.load(Ordering::Relaxed)
        } else {
            0
        };
        let frame = ns_to_frames(pos_ns, cur.item.src_rate);
        if cur.reposition(frame).is_ok() {
            let start = cur.start_at(pos_ns, false);
            self.last_gen = gen_id;
            self.push_flush(gen_id, Some(start));
        } else {
            self.cur = None;
            self.push_flush(gen_id, None);
        }
    }

    /// `Player::set_fx`: the item's speed, tone and reverb change now. The
    /// playing item restarts where the listener is with the new sound (the
    /// buffered second of the old one is flushed, with the usual de-click
    /// fades, so the change is heard at once); a prepared next item simply
    /// starts with it.
    /// A new loudness correction for item `id` (its file's tags were read
    /// after it was asked to play): an item still opening takes it when
    /// open, one not started yet at once, one being produced glides to it
    /// ([`GAIN_GLIDE`]). What the ring already holds plays as it was.
    fn set_gain(&mut self, id: u64, gain_db: f32) {
        let gain_db = if gain_db.is_finite() {
            gain_db.clamp(-60.0, 30.0)
        } else {
            0.0
        };
        let opening = self.pending_play.is_some_and(|(_, _, want)| want == id)
            || (self.pending_next.is_some() && self.pending_next_id == Some(id));
        if opening {
            self.pending_gain = Some((id, gain_db));
        }
        let rate = self.cfg.rate;
        if let Some(next) = self.next.as_mut().filter(|n| n.id() == id) {
            next.set_gain_db(gain_db, rate, false);
        }
        if let Some(x) = self.xfade.as_mut().filter(|x| x.to.id() == id) {
            x.to.set_gain_db(gain_db, rate, true);
        }
        if let Some(cur) = self.cur.as_mut().filter(|c| c.id() == id) {
            let out = cur.produced > 0 || cur.start_src > 0;
            cur.set_gain_db(gain_db, rate, out);
        }
    }

    fn set_fx(&mut self, id: u64, fx: ItemFx, gen_id: u64) {
        let opening = self.pending_play.is_some_and(|(_, _, want)| want == id)
            || (self.pending_next.is_some() && self.pending_next_id == Some(id));
        if opening {
            self.pending_fx = Some((id, fx));
        }

        let chained_into_it = self.cur.as_ref().is_some_and(|c| {
            c.stream
                .as_ref()
                .is_some_and(|s| s.chained && s.chain_id == Some(id))
        });
        if let Some(next) = self.next.as_mut().filter(|n| n.id() == id) {
            next.item.set_fx(fx);
            next.fx = None;
            next.fx_cfg = None;
            if next.stream.is_some() || chained_into_it {
                let _ = next.reposition(0);
            }
        }
        let producing = self
            .xfade
            .as_ref()
            .map(|x| x.to.id())
            .or_else(|| self.cur.as_ref().map(Active::id));
        let is_current = producing == Some(id);
        if !is_current && !chained_into_it {
            let _ = self.events.send(DecEvent::SeekDropped { gen_id });
            return;
        }
        // The item is being produced but the listener is not there yet: the
        // end of the item before it is still in the ring and must play.
        // Flushing would cut that off (up to a second of music).
        let heard = self.sh.consumed_id.load(Ordering::Relaxed) == id;
        if is_current && !heard {
            match self.cur_began {
                // It began at a clean join: start it over with the new
                // sound and have the callback skip what was pushed of the
                // old one, right at the join.
                Began::Boundary(at) if self.xfade.is_none() => {
                    self.restart_unheard(fx, at);
                    let _ = self.events.send(DecEvent::SeekDropped { gen_id });
                    return;
                }
                // Its start is mixed into a crossfade: nothing can be taken
                // back. The change waits until the listener is in it.
                Began::Boundary(_) | Began::Mixed => {
                    self.deferred_fx = Some((id, fx));
                    let _ = self.events.send(DecEvent::SeekDropped { gen_id });
                    return;
                }
                // A play or a seek the callback has not applied yet: what
                // is in the ring goes anyway.
                Began::Flush => {}
            }
        }
        // During a crossfade the audible item is the incoming one.
        if let Some(x) = self.xfade.take() {
            self.cur = Some(x.to);
        }
        if is_current && let Some(cur) = self.cur.as_mut() {
            cur.item.set_fx(fx);
        }
        // Otherwise the playing item's resampler had already run on into
        // the changed next item: it starts over to let go of that.
        self.restart_current(gen_id);
    }

    /// The item being produced began at a gapless join (at `at` of the
    /// pushed stream) and nobody has heard it yet: it starts over from its
    /// first frame with `fx`. What was pushed of it so far stays in the
    /// ring behind the join; a mark tells the callback to skip it when it
    /// gets there, so the item before it plays to its end and the join is
    /// as gapless as it was.
    fn restart_unheard(&mut self, fx: ItemFx, at: u64) {
        let Some(cur) = self.cur.as_mut() else {
            return;
        };
        let id = cur.id();
        let was_chained = cur.stream.as_ref().is_some_and(|s| s.chained);
        cur.item.set_fx(fx);
        let result = cur.reposition(0);
        let start = cur.start_at(0, false);
        if was_chained && let Some(next) = self.next.as_mut() {
            let _ = next.reposition(0);
        }
        match result {
            Ok(()) => {
                self.push_mark(Mark {
                    at,
                    flush: false,
                    skip_to: Some(self.pushed),
                    cfg_gen: self.cfg.gen_id,
                    gen_id: self.last_gen,
                    ended: None,
                    start: Some(start),
                });
                self.cur_began = Began::Boundary(self.pushed);
            }
            Err(e) => {
                // It cannot start over: it ends here, as on a decode error.
                self.report(Some(id), &e);
                self.finish_current();
            }
        }
    }

    /// Applies a change that waited for the listener to reach its item.
    fn try_deferred_fx(&mut self) {
        let Some((id, fx)) = self.deferred_fx else {
            return;
        };
        let producing = self
            .xfade
            .as_ref()
            .map(|x| x.to.id())
            .or_else(|| self.cur.as_ref().map(Active::id));
        if producing != Some(id) {
            // The item is gone (it ended, or something else was played).
            self.deferred_fx = None;
            return;
        }
        let heard = self.sh.consumed_id.load(Ordering::Relaxed) == id;
        if heard || self.cur_began == Began::Flush {
            self.deferred_fx = None;
            // Not a request of its own any more: it keeps the generation.
            let gen_id = self.last_gen;
            self.set_fx(id, fx, gen_id);
        }
    }

    /// Rebuilds the playing item's stream where the listener is and flushes
    /// what was buffered of it.
    fn restart_current(&mut self, gen_id: u64) {
        let cursor = self.sh.cursor.read();
        let unapplied_play = cursor.applied_gen < self.play_gen;
        // A flush of ours the callback has not applied yet (a seek a moment
        // ago): what it consumed is from before that, the item is where
        // that flush put it.
        let settled = cursor.applied_gen >= self.last_gen;
        let Some(cur) = self.cur.as_mut() else {
            let _ = self.events.send(DecEvent::SeekDropped { gen_id });
            return;
        };
        let id = cur.id();
        let rate = cur.item.src_rate;
        let consumed = (settled && self.sh.consumed_id.load(Ordering::Relaxed) == id)
            .then(|| self.sh.consumed_pos_ns.load(Ordering::Relaxed));
        let pos_ns = consumed.unwrap_or_else(|| frames_to_ns(cur.start_src, rate));
        let mut frame = ns_to_frames(pos_ns, rate);
        if let Some(total) = cur.item.total_src {
            frame = frame.min(total);
        }
        let was_chained = cur.stream.as_ref().is_some_and(|s| s.chained);
        let result = cur.reposition(frame);
        let start = cur.start_at(pos_ns, unapplied_play);
        if was_chained && let Some(next) = self.next.as_mut() {
            next.rewind();
        }
        match result {
            Ok(()) => {
                self.last_gen = gen_id;
                self.push_flush(gen_id, Some(start));
            }
            Err(_) if cur.item.gave_way() => {
                // A seek, a play or a stop on its way made it give way.
                cur.stalled = true;
            }
            Err(e) => {
                self.report(Some(id), &e);
                self.cur = None;
                self.last_gen = gen_id;
                self.push_flush(gen_id, None);
            }
        }
    }

    // ----- configuration --------------------------------------------------

    fn check_config(&mut self) {
        let gen_id = self.sh.cfg_gen.load(Ordering::Acquire);
        if gen_id == self.cfg.gen_id {
            return;
        }
        let rate = self.sh.cfg_rate.load(Ordering::Relaxed);
        let channels = self.sh.cfg_channels.load(Ordering::Relaxed) as usize;
        if rate == 0 || channels == 0 {
            return;
        }
        self.cfg = Cfg {
            rate,
            channels,
            gen_id,
        };
        self.fill_target = fill_target(rate, channels, self.ring_capacity);

        if let Some(x) = self.xfade.take() {
            self.cur = Some(x.to);
        }
        // Resume the audible item where the listener is.
        let unapplied_play = self.sh.cursor.read().applied_gen < self.play_gen;
        let mut start = None;
        if let Some(cur) = self.cur.as_mut() {
            let id = cur.id();
            let consumed = (self.sh.consumed_id.load(Ordering::Relaxed) == id)
                .then(|| self.sh.consumed_pos_ns.load(Ordering::Relaxed));
            let pos_ns = consumed.unwrap_or_else(|| frames_to_ns(cur.start_src, cur.item.src_rate));
            let frame = ns_to_frames(pos_ns, cur.item.src_rate);
            // Nothing was read since the source was last positioned: it is
            // there already (and a source that cannot seek, e.g. a live PCM
            // stream, must not be asked to).
            let in_place = cur.stream.is_none() && frame == cur.start_src;
            match if in_place {
                Ok(())
            } else {
                cur.reposition(frame)
            } {
                Ok(()) => start = Some(cur.start_at(pos_ns, unapplied_play)),
                Err(e) => {
                    let _ = self.events.send(DecEvent::Error {
                        id: Some(id),
                        message: e.to_string(),
                    });
                    self.cur = None;
                }
            }
        }
        if let Some(next) = self.next.as_mut() {
            let _ = next.reposition(0);
        }
        let gen_for_flush = self.last_gen;
        self.push_flush(gen_for_flush, start);
    }

    // ----- production -----------------------------------------------------

    /// Milliseconds of audio ready in the ring.
    fn buffered_ms(&self) -> u64 {
        let per_ms = u64::from(self.cfg.rate) * self.cfg.channels as u64 / 1000;
        if per_ms == 0 {
            return 0;
        }
        (self.ring_capacity - self.ring.slots()) as u64 / per_ms
    }

    /// Warns (rate-limited) when one production step, with the time the
    /// thread woke late before it, held the thread up long enough to
    /// matter: a stream read waiting for the network, a stalled disk, a
    /// starved CPU. With `buffered` ms ready before it, a stall longer than
    /// that is a dropout.
    fn note_step_time(&mut self, began: Instant, woke_late: Duration, buffered: u64) {
        let took = began.elapsed() + woke_late;
        if took < SLOW_STEP || self.cur.is_none() {
            return;
        }
        if let Some(n) = self.slow_log.record(1, Instant::now()) {
            log::warn!(
                "audio: decoding stalled for {} ms ({} ms of it waking up late) with {buffered} ms buffered ({n} slow step(s) since the last warning)",
                took.as_millis(),
                woke_late.as_millis()
            );
        }
    }

    fn pump(&mut self) -> Step {
        self.short_wait = false;
        if self.cfg.rate == 0 || self.cur.is_none() {
            return Step::Idle;
        }
        let occupied = self.ring_capacity - self.ring.slots();
        if occupied >= self.fill_target {
            return Step::Idle;
        }
        if self.xfade.is_some() {
            return self.produce_crossfade();
        }
        self.produce_single()
    }

    fn crossfade_frames(&self) -> u64 {
        let ms = u64::from(self.sh.crossfade_ms.load(Ordering::Relaxed));
        u64::from(self.cfg.rate) * ms / 1000
    }

    /// The longest crossfade the incoming item can take (half its length).
    fn next_len_limit(&self) -> u64 {
        self.next
            .as_ref()
            .and_then(|n| n.remaining(self.cfg))
            .map_or(u64::MAX, |r| r / 2)
    }

    fn produce_single(&mut self) -> Step {
        if let Some(cur) = self.cur.as_ref().filter(|c| c.stalled) {
            let waits = |i: &Item| i.cancelled() || i.interrupted();
            if waits(&cur.item) || self.next.as_ref().is_some_and(|n| waits(&n.item)) {
                // The command it gave way to is on its way.
                self.short_wait = true;
                return Step::Idle;
            }
            // It never came (a seek of another item): read on from where
            // the listener is.
            self.restart_current(self.last_gen);
            return Step::Worked;
        }
        let cfg = self.cfg;
        let cross = self.crossfade_frames();
        let mut want = CHUNK_FRAMES as u64;
        if cross > 0
            && self.next.is_some()
            && let Some(rem) = self
                .cur
                .as_ref()
                .filter(|c| !c.ringing())
                .and_then(|c| c.remaining(cfg))
        {
            let len = cross.min(self.next_len_limit());
            if len > 0 && rem <= len {
                self.start_crossfade(len.min(rem.max(1)));
                return Step::Worked;
            }
            if len > 0 {
                want = want.min(rem - len);
            }
        }
        let want = want.max(1) as usize;
        if self.hold_for_next(want) {
            self.short_wait = true;
            return Step::Idle;
        }
        let normalization = self.sh.normalization.load(Ordering::Relaxed);
        self.buf.clear();
        let Some(cur) = self.cur.as_mut() else {
            return Step::Idle;
        };
        let id = cur.id();
        let chain = if cross == 0 {
            self.next.as_mut().map(|n| &mut n.item)
        } else {
            None
        };
        match cur.read(cfg, want, &mut self.buf, chain) {
            Ok(StreamRead::Frames(_)) => {
                let ch = cfg.channels.max(1);
                let (from, to) = cur.gains(normalization, self.buf.len() / ch);
                apply_gain_glide(&mut self.buf, ch, from, to);
                self.mix_tail(normalization);
                self.push_samples_buf();
                Step::Worked
            }
            Ok(StreamRead::Pending) => {
                self.short_wait = true;
                Step::Idle
            }
            Ok(StreamRead::Eof) if self.pending_next.is_some() => {
                // The next item is a stream still opening: wait for it
                // rather than end into nothing.
                self.short_wait = true;
                Step::Idle
            }
            Ok(StreamRead::Eof) => {
                // Nothing follows: a reverb plays out before the end.
                let rings =
                    self.next.is_none() && self.cur.as_mut().is_some_and(Active::start_ring_out);
                if !rings {
                    self.finish_current();
                }
                Step::Worked
            }
            // The read that failed gave way (its own source says so: the
            // item, or the next one it flowed on into at a gapless join):
            // a seek, a play, a stop or a new next item is on its way. The
            // demuxer may have stopped inside a packet: the item is read
            // on only after a reposition.
            Err(_) if cur.item.gave_way() | self.next.as_ref().is_some_and(|n| n.item.gave_way()) => {
                cur.stalled = true;
                self.short_wait = true;
                Step::Idle
            }
            Err(e) => {
                self.report(Some(id), &e);
                self.finish_current();
                Step::Worked
            }
        }
    }

    /// Adds what the item before a gapless join still sounds like to
    /// `self.buf` (see [`Tail`]); eased to nothing over its last quarter.
    fn mix_tail(&mut self, normalization: bool) {
        let Some(tail) = self.tail.as_mut() else {
            return;
        };
        let ch = self.cfg.channels;
        let n = ((self.buf.len() / ch) as u64).min(tail.left) as usize;
        self.buf_b.clear();
        self.buf_b.resize(n * ch, 0.0);
        tail.fx.process(&mut self.buf_b);
        let gain = if normalization {
            10f32.powf(tail.gain_db / 20.0)
        } else {
            1.0
        };
        let quarter = (tail.total / 4).max(1) as f32;
        for (i, (out, add)) in self
            .buf
            .chunks_exact_mut(ch)
            .zip(self.buf_b.chunks_exact(ch))
            .enumerate()
        {
            let ease = ((tail.left - i as u64) as f32 / quarter).min(1.0);
            for (o, a) in out.iter_mut().zip(add) {
                *o += a * gain * ease;
            }
        }
        tail.left -= n as u64;
        if tail.left == 0 {
            self.tail = None;
        }
    }

    /// A local next file is still opening and the current item is about to
    /// run out: wait for it a moment instead of reading on, so the stream can
    /// still flow on into it through the same resampler (a continuous join),
    /// as it did when files were opened on this thread. A stream that is
    /// still opening is not waited for here: that can take seconds, and the
    /// wait belongs at the very end (`StreamRead::Eof`), not inside the music.
    fn hold_for_next(&self, want: usize) -> bool {
        if self.pending_next.is_none() || !self.pending_next_file {
            return false;
        }
        // The source reports its end once fewer than `want` frames are
        // ready; the resampler's delay and a whole packet may still be
        // pending behind that.
        let margin = (want + 4 * CHUNK_FRAMES) as u64 + u64::from(self.cfg.rate) / 20;
        self.cur
            .as_ref()
            .and_then(|c| c.remaining(self.cfg))
            .is_some_and(|rem| rem <= margin)
    }

    /// Starts overlapping the prepared next item with the tail of the
    /// current one. The listener-visible item changes *now*.
    fn start_crossfade(&mut self, len: u64) {
        let Some(mut to) = self.next.take() else {
            return;
        };
        if !to.item.start() {
            // Superseded meanwhile: the current item plays to its end.
            return;
        }
        let cfg = self.cfg;
        if let Err(e) = to.prepare(cfg) {
            self.report(Some(to.id()), &e);
            return;
        }
        let from = self.cur.as_ref().map(Active::id);
        let start = to.start_at(0, true);
        self.push_mark(Mark {
            at: self.pushed,
            flush: false,
            skip_to: None,
            cfg_gen: cfg.gen_id,
            gen_id: self.last_gen,
            ended: from,
            start: Some(start),
        });
        self.cur_began = Began::Mixed;
        self.xfade = Some(Crossfade { to, len, done: 0 });
    }

    fn produce_crossfade(&mut self) -> Step {
        let cfg = self.cfg;
        let ch = cfg.channels;
        let normalization = self.sh.normalization.load(Ordering::Relaxed);
        let (Some(cur), Some(x)) = (self.cur.as_mut(), self.xfade.as_mut()) else {
            return Step::Idle;
        };
        if x.to.stalled {
            if x.to.item.cancelled() || x.to.item.interrupted() {
                // What it gave way to is on its way (see below).
                self.short_wait = true;
                return Step::Idle;
            }
            // The seek its interruption was for went to the outgoing item:
            // the incoming one goes on alone, from where the listener is.
            if let Some(x) = self.xfade.take() {
                self.cur = Some(x.to);
            }
            return Step::Worked;
        }
        let want = (x.len - x.done).min(CHUNK_FRAMES as u64).max(1) as usize;
        let (out_from, out_to) = cur.gains(normalization, want);
        let (in_from, in_to) = x.to.gains(normalization, want);
        let (cur_id, to_id) = (cur.id(), x.to.id());

        self.buf.clear();
        self.buf_b.clear();
        // Outgoing item: whatever it still has, silence once it is over
        // (through its reverb, which rings on under the incoming item).
        let mut from_err = cur.read(cfg, want, &mut self.buf, None).err();
        let got = self.buf.len().min(want * ch);
        self.buf.resize(want * ch, 0.0);
        if got < want * ch && cur.fx.is_some() {
            cur.apply_fx(cfg, &mut self.buf[got..]);
        }
        // Incoming item.
        let mut to_eof = false;
        let to_err = match x.to.read(cfg, want, &mut self.buf_b, None) {
            Ok(StreamRead::Frames(_)) => None,
            Ok(StreamRead::Pending) => {
                self.short_wait = true;
                None
            }
            Ok(StreamRead::Eof) => {
                to_eof = true;
                None
            }
            Err(e) => {
                to_eof = true;
                Some(e)
            }
        };
        // Either side may be the one a read gave way in (see
        // `produce_single`); each looks at its own source.
        let from_gave = from_err.is_some() && cur.item.gave_way();
        let to_gave = to_err.is_some() && x.to.item.gave_way();
        if from_gave && cur.item.cancelled() || to_gave && x.to.item.cancelled() {
            // A play or a stop is on its way.
            self.short_wait = true;
            return Step::Idle;
        }
        if to_gave {
            // A seek is on its way: one of the incoming item takes it.
            x.to.stalled = true;
            self.short_wait = true;
            return Step::Idle;
        }
        // The outgoing item gave way to a seek: a seek of it is dropped (the
        // listener is past it at once) and one of the incoming item drops
        // it too, so it ends here.
        let from_gone = from_gave;
        if from_gone {
            from_err = None;
        }

        // Equal-power mix over `want` frames; a side that returned fewer
        // frames contributes silence for the rest.
        self.buf.resize(want * ch, 0.0);
        self.buf_b.resize(want * ch, 0.0);
        let len = x.len as f32;
        for i in 0..want {
            let t = (x.done + i as u64) as f32 / len;
            let angle = t * std::f32::consts::FRAC_PI_2;
            let f = (i + 1) as f32 / want as f32;
            let out_gain = out_from + (out_to - out_from) * f;
            let in_gain = in_from + (in_to - in_from) * f;
            let (g_out, g_in) = (angle.cos() * out_gain, angle.sin() * in_gain);
            let base = i * ch;
            for k in base..base + ch {
                self.buf[k] = self.buf[k] * g_out + self.buf_b[k] * g_in;
            }
        }
        x.done += want as u64;
        let finished = x.done >= x.len || to_eof || from_gone;
        if let Some(e) = from_err {
            self.report(Some(cur_id), &e);
        }
        if let Some(e) = to_err {
            self.report(Some(to_id), &e);
        }
        self.mix_tail(normalization);
        self.push_samples_buf();
        if finished && let Some(x) = self.xfade.take() {
            self.cur = Some(x.to);
        }
        Step::Worked
    }

    /// The current item is over (end of stream or error): hand over to the
    /// prepared next item gaplessly, or end.
    fn finish_current(&mut self) {
        let Some(mut ended) = self.cur.take() else {
            return;
        };
        let ended_id = ended.id();
        let cfg = self.cfg;
        // A next item superseded meanwhile does not start: the current one
        // ends into nothing, and the command that superseded it follows.
        let next = match self.next.take().filter(|n| n.item.start()) {
            // A read ahead into it gave way: it starts over.
            Some(mut n) if n.stalled => match n.reposition(0) {
                Ok(()) => Some(n),
                Err(e) => {
                    self.report(Some(n.id()), &e);
                    None
                }
            },
            other => other,
        };
        if let Some(mut next) = next {
            // If the resampler already flowed on into this item, keep it: the
            // join is then continuous instead of two separately flushed streams.
            let carried = ended
                .stream
                .take()
                .filter(|s| s.chained && s.chain_id == Some(next.id()) && s.cfg == cfg);
            let ready = match carried {
                Some(mut stream) => {
                    stream.rebase();
                    next.stream = Some(stream);
                    next.start_src = 0;
                    next.produced = 0;
                    Ok(())
                }
                None => next.prepare(cfg),
            };
            match ready {
                Ok(()) => {
                    // Its shelves and reverb ring on under the next item
                    // (unless the reverb already played out).
                    let rang_out = ended.ringing();
                    let same_format = ended.fx_cfg == Some(cfg);
                    self.tail = ended
                        .fx
                        .take()
                        .filter(|_| same_format && !rang_out)
                        .map(|fx| {
                            let total = fx.tail_frames().max(1);
                            Tail {
                                fx,
                                gain_db: ended.item.gain_db,
                                left: total,
                                total,
                            }
                        });
                    let start = next.start_at(0, true);
                    self.push_mark(Mark {
                        at: self.pushed,
                        flush: false,
                        skip_to: None,
                        cfg_gen: cfg.gen_id,
                        gen_id: self.last_gen,
                        ended: Some(ended_id),
                        start: Some(start),
                    });
                    self.cur_began = Began::Boundary(self.pushed);
                    self.cur = Some(next);
                    return;
                }
                Err(e) => self.report(Some(next.id()), &e),
            }
        }
        self.push_mark(Mark {
            at: self.pushed,
            flush: false,
            skip_to: None,
            cfg_gen: cfg.gen_id,
            gen_id: self.last_gen,
            ended: Some(ended_id),
            start: None,
        });
    }

    // ----- ring access ---------------------------------------------------

    fn push_flush(&mut self, gen_id: u64, start: Option<Start>) {
        // What rang on from an earlier item is stale too.
        self.tail = None;
        self.cur_began = Began::Flush;

        self.push_mark(Mark {
            at: self.pushed,
            flush: true,
            skip_to: None,
            cfg_gen: self.cfg.gen_id,
            gen_id,
            ended: None,
            start,
        });
    }

    fn push_mark(&mut self, mark: Mark) {
        for _ in 0..50 {
            if self.marks.push(mark).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        log::warn!("audio: mark queue is stuck, dropping a mark");
    }

    /// Writes `self.buf` (device-format frames) to the ring.
    fn push_samples_buf(&mut self) {
        let mut offset = 0;
        while offset < self.buf.len() {
            let free = self.ring.slots();
            if free == 0 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            let n = free.min(self.buf.len() - offset);
            if let Ok(chunk) = self.ring.write_chunk_uninit(n) {
                chunk.fill_from_iter(self.buf[offset..offset + n].iter().copied());
                offset += n;
            }
        }
        self.pushed += self.buf.len() as u64;
    }
}

/// Samples the decoder keeps in the ring at `rate` and `channels`: about
/// [`FILL_TARGET_MS`], but never more than half the ring.
fn fill_target(rate: u32, channels: usize, ring_capacity: usize) -> usize {
    let target = u64::from(rate) * channels as u64 * FILL_TARGET_MS / 1000;
    let ceiling = (ring_capacity / 2) as u64;
    target.min(ceiling) as usize
}

fn apply_gain(buf: &mut [f32], gain: f32) {
    if gain != 1.0 {
        for s in buf {
            *s *= gain;
        }
    }
}

/// [`apply_gain`] going from `from` to `to` across the frames of `buf`
/// (`channels` samples each), reaching `to` on the last one.
fn apply_gain_glide(buf: &mut [f32], channels: usize, from: f32, to: f32) {
    if from == to {
        apply_gain(buf, to);
        return;
    }
    let channels = channels.max(1);
    let frames = (buf.len() / channels).max(1) as f32;
    for (i, frame) in buf.chunks_exact_mut(channels).enumerate() {
        let gain = from + (to - from) * ((i + 1) as f32 / frames);
        for s in frame {
            *s *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `left` frames of stereo silence, handed out in odd-sized pieces.
    struct Silence {
        left: u64,
    }

    impl FrameSource for Silence {
        fn sample_rate(&self) -> u32 {
            44_100
        }

        fn channels(&self) -> usize {
            2
        }

        fn total_frames(&self) -> Option<u64> {
            None
        }

        fn read(&mut self, out: &mut Vec<f32>) -> Result<ReadStatus, DecodeError> {
            if self.left == 0 {
                return Ok(ReadStatus::Eof);
            }
            let n = self.left.min(701);
            self.left -= n;
            out.extend(std::iter::repeat_n(0.0, n as usize * 2));
            Ok(ReadStatus::Data)
        }

        fn seek(&mut self, _frame: u64) -> Result<(), DecodeError> {
            Ok(())
        }
    }

    fn item(id: u64, frames: u64) -> Item {
        Item {
            id,
            gain_db: 0.0,
            src: Box::new(Silence { left: frames }),
            src_rate: 44_100,
            src_channels: 2,
            total_src: Some(frames),
            fx: ItemFx::default(),
            speed: Speed::ONE,
            slot: None,
        }

    }

    /// Reads `item` (chaining into `next`) until its own end; returns the
    /// number of frames handed out.
    fn read_item(stream: &mut Stream, item: &mut Item, next: &mut Item) -> u64 {
        let mut out = Vec::new();
        loop {
            match stream.read(item, Some(next), 1024, &mut out).unwrap() {
                StreamRead::Frames(_) => {}
                StreamRead::Pending => panic!("pending"),
                StreamRead::Eof => return (out.len() / 2) as u64,
            }
        }
    }

    fn linear(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    #[test]
    fn a_gain_set_while_sounding_glides_there() {
        let mut a = Active::new(item(1, 100_000));
        a.item.gain_db = -3.0;
        // Nothing of it out yet: the new gain holds at once.
        a.set_gain_db(-9.0, 48_000, false);
        assert_eq!(a.gains(true, 1024), (linear(-9.0), linear(-9.0)));
        // Sounding: from where it is, chunk after chunk, to the new gain.
        a.set_gain_db(-3.0, 48_000, true);
        let len = 48_000 * GAIN_GLIDE.as_millis() as usize / 1000;
        let whole = linear(-3.0) - linear(-9.0);
        let (mut last, mut done) = (linear(-9.0), 0);
        while done < len {
            let (from, to) = a.gains(true, 1024);
            assert!((from - last).abs() < 1e-6, "a step at frame {done}");
            assert!(to > from, "not rising at frame {done}");
            assert!(to - from < whole / 10.0, "too fast at frame {done}");
            last = to;
            done += 1024;
        }
        assert!((last - linear(-3.0)).abs() < 1e-6);
        assert_eq!(a.gains(true, 1024), (linear(-3.0), linear(-3.0)));
        // Normalization off: no gain at all, gliding or not.
        a.set_gain_db(-12.0, 48_000, true);
        assert_eq!(a.gains(false, 1024), (1.0, 1.0));
        // The same gain again starts no glide.
        let mut b = Active::new(item(2, 100_000));
        b.set_gain_db(0.0, 48_000, true);
        assert!(b.glide.is_none());
    }

    #[test]
    fn a_gliding_gain_reaches_its_end_on_the_last_frame() {
        let mut buf = vec![1.0f32; 8];
        apply_gain_glide(&mut buf, 2, 0.0, 1.0);
        assert_eq!(buf, vec![0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0]);
        let mut flat = vec![1.0f32; 4];
        apply_gain_glide(&mut flat, 2, 0.5, 0.5);
        assert_eq!(flat, vec![0.5; 4]);
    }

    #[test]
    fn the_ring_is_kept_a_second_ahead() {
        // The engine's ring (2^19 samples) holds a full second at every
        // common rate, so the target is not cut by the half-ring ceiling.
        let ring = 1 << 19;
        for rate in [44_100u32, 48_000, 96_000] {
            let samples = fill_target(rate, 2, ring);
            let ms = samples as u64 * 1000 / (u64::from(rate) * 2);
            assert!(ms >= 1000, "{rate} Hz: only {ms} ms ahead");
        }
        // A small ring still leaves half of itself free for the producer.
        assert_eq!(fill_target(48_000, 2, 4096), 2048);
    }

    fn at_speed(mut item: Item, speed: f32) -> Item {
        item.set_fx(ItemFx {
            speed,
            ..ItemFx::default()
        });
        item
    }

    const CFG: Cfg = Cfg {
        rate: 48_000,
        channels: 2,
        gen_id: 1,
    };

    #[test]
    fn the_end_of_an_item_is_counted_in_sound_not_in_source_frames() {
        // A second of 44.1 kHz at 1.25 is 0.8 s of sound: 38400 frames.
        let fast = Active::new(at_speed(item(1, 44_100), 1.25));
        assert_eq!(fast.remaining(CFG), Some(38_400));
        let slow = Active::new(at_speed(item(1, 44_100), 0.85));
        assert_eq!(slow.remaining(CFG), Some((44_100u64 * 3200).div_ceil(2499)));
        let plain = Active::new(item(1, 44_100));
        assert_eq!(plain.remaining(CFG), Some(48_000));
        // From the middle, after a seek: what is left of it.
        let mut fast = Active::new(at_speed(item(1, 44_100), 1.25));
        fast.reposition(22_050).unwrap();
        assert_eq!(fast.remaining(CFG), Some(19_200));
    }

    #[test]
    fn items_at_one_speed_share_a_resampler_and_keep_exact_lengths() {
        let frames = [44_101u64, 50_003];
        let (mut a, mut b) = (
            at_speed(item(1, frames[0]), 1.2),
            at_speed(item(2, frames[1]), 1.2),
        );
        let mut c = at_speed(item(3, 10), 1.2);
        let mut stream = Stream::new(&a, CFG).unwrap();
        let ceil = |n: u64| (n * 400).div_ceil(441);
        assert_eq!(read_item(&mut stream, &mut a, &mut b), ceil(frames[0]));
        assert!(stream.chained);
        stream.rebase();
        assert_eq!(
            read_item(&mut stream, &mut b, &mut c),
            ceil(frames[0] + frames[1]) - ceil(frames[0])
        );
    }

    #[test]
    fn items_at_different_speeds_do_not_share_a_resampler() {
        let (mut a, mut b) = (at_speed(item(1, 44_101), 1.2), at_speed(item(2, 50_003), 0.85));
        let mut stream = Stream::new(&a, CFG).unwrap();
        // The first is flushed to its own exact end instead of flowing on.
        assert_eq!(read_item(&mut stream, &mut a, &mut b), (44_101u64 * 400).div_ceil(441));
        assert!(!stream.chained);
        // Neither do a plain item and one that keeps its timbre.
        let (mut a, mut b) = (at_speed(item(1, 44_101), 1.2), at_speed(item(2, 50_003), 1.2));
        b.fx.keep_formants = true;
        let mut stream = Stream::new(&a, CFG).unwrap();
        assert_eq!(read_item(&mut stream, &mut a, &mut b), (44_101u64 * 400).div_ceil(441));
        assert!(!stream.chained);
    }

    #[test]
    fn keeping_the_timbre_keeps_the_exact_length_through_a_chain() {
        let keep = |item: Item| {
            let mut item = at_speed(item, 1.2);
            item.fx.keep_formants = true;
            item
        };
        let frames = [44_101u64, 50_003];
        let (mut a, mut b, mut c) = (keep(item(1, frames[0])), keep(item(2, frames[1])), keep(item(3, 10)));
        let mut stream = Stream::new(&a, CFG).unwrap();
        assert!(stream.post.is_some());
        let ceil = |n: u64| (n * 400).div_ceil(441);
        assert_eq!(read_item(&mut stream, &mut a, &mut b), ceil(frames[0]));
        assert!(stream.chained);
        stream.rebase();
        assert_eq!(
            read_item(&mut stream, &mut b, &mut c),
            ceil(frames[0] + frames[1]) - ceil(frames[0])
        );
    }

    #[test]
    fn chained_items_get_their_own_exact_lengths_through_one_resampler() {

        let cfg = Cfg {
            rate: 48_000,
            channels: 2,
            gen_id: 1,
        };
        let frames = [44_101u64, 50_003, 30_011];
        let (mut a, mut b, mut c) = (item(1, frames[0]), item(2, frames[1]), item(3, frames[2]));
        let mut stream = Stream::new(&a, cfg).unwrap();
        let ceil = |n: u64| (n * 48_000).div_ceil(44_100);

        assert_eq!(read_item(&mut stream, &mut a, &mut b), ceil(frames[0]));
        assert!(stream.chained);
        stream.rebase();
        // The follower's frames already fed during the chain still count.
        assert_eq!(
            read_item(&mut stream, &mut b, &mut c),
            ceil(frames[0] + frames[1]) - ceil(frames[0])
        );
    }
}
