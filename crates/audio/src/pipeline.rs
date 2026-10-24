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

use crate::channels;
use crate::decode::{self, FrameSource, PcmFrames, ReadStatus};
use crate::render::{Mark, Start};
use crate::resample::StreamResampler;
use crate::shared::{LogLimiter, Shared, frames_to_ns, ns_to_frames};
use crate::types::{DecodeError, ItemSource, PlayItem};

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

/// Commands for the decoder thread.
pub(crate) enum DecCmd {
    Play {
        item: PlayItem,
        gen_id: u64,
    },
    SetNext(PlayItem),
    ClearNext,
    /// `id` is the item the listener saw when asking.
    Seek {
        pos: Duration,
        id: u64,
        gen_id: u64,
    },
    Stop {
        gen_id: u64,
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
}

impl Item {
    fn open(item: PlayItem) -> Result<Self, (u64, DecodeError)> {
        let id = item.id;
        let mut hint = item.duration_hint;
        let mut gain_db = item.gain_db;
        let src: Box<dyn FrameSource> = match item.source {
            ItemSource::File(path) => Box::new(decode::open_file(&path).map_err(|e| (id, e))?),
            ItemSource::Pcm(pcm) => Box::new(PcmFrames::new(pcm).map_err(|e| (id, e))?),
            ItemSource::Stream(open) => {
                let bytes = open().map_err(|m| (id, DecodeError::Stream(m)))?;
                let (src, info) = decode::open_stream(bytes).map_err(|e| (id, e))?;
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
        })
    }

    fn duration(&self) -> Option<Duration> {
        self.total_src
            .map(|f| Duration::from_secs_f64(f as f64 / f64::from(self.src_rate)))
    }
}

/// Decoded, resampled, channel-mapped audio of one item at the device format.
struct Stream {
    cfg: Cfg,
    resampler: Option<StreamResampler>,
    /// Map to the device channel count before resampling (cheaper when
    /// downmixing).
    map_first: bool,
    raw: Vec<f32>,
    tmp: Vec<f32>,
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
        let needs_resample = item.src_rate != cfg.rate;
        let map_first = item.src_channels > cfg.channels;
        let rs_channels = if map_first {
            cfg.channels
        } else {
            item.src_channels
        };
        let resampler = if needs_resample {
            Some(StreamResampler::new(item.src_rate, cfg.rate, rs_channels)?)
        } else {
            None
        };
        Ok(Self {
            cfg,
            resampler,
            map_first,
            raw: Vec::new(),
            tmp: Vec::new(),
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
        match &mut self.resampler {
            None => channels::convert(&self.raw, src_channels, dev, &mut self.ready),
            Some(rs) if self.map_first => {
                self.tmp.clear();
                channels::convert(&self.raw, src_channels, dev, &mut self.tmp);
                rs.process(&self.tmp, &mut self.ready)?;
            }
            Some(rs) => {
                self.tmp.clear();
                rs.process(&self.raw, &mut self.tmp)?;
                channels::convert(&self.tmp, src_channels, dev, &mut self.ready);
            }
        }
        self.raw.clear();
        Ok(())
    }

    fn flush(&mut self, src_channels: usize) -> Result<(), DecodeError> {
        let dev = self.cfg.channels;
        if let Some(rs) = &mut self.resampler {
            if self.map_first {
                rs.finish(&mut self.ready)?;
            } else {
                self.tmp.clear();
                rs.finish(&mut self.tmp)?;
                channels::convert(&self.tmp, src_channels, dev, &mut self.ready);
            }
        }
        self.src_done = true;
        Ok(())
    }

    /// Appends up to `want` frames to `out`. With `chain`, the end of `item`
    /// flows straight into the start of the next item (same rate and
    /// channels) through the same resampler.
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
                            n.src_rate == item.src_rate && n.src_channels == item.src_channels
                        });
                    if can_chain {
                        let rs_in = u128::from(self.src_in) * u128::from(self.cfg.rate);
                        let end = rs_in.div_ceil(u128::from(item.src_rate)) as u64;
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
}

impl Active {
    fn new(item: Item) -> Self {
        Self {
            item,
            stream: None,
            start_src: 0,
            produced: 0,
        }
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
        self.prepare(cfg)?;
        let Some(stream) = self.stream.as_mut() else {
            return Ok(StreamRead::Eof);
        };
        let r = stream.read(&mut self.item, chain, want, out)?;
        if let StreamRead::Frames(n) = r {
            self.produced += n as u64;
        }
        Ok(r)
    }

    /// Positions the source at `src_frame` and drops buffered audio.
    fn reposition(&mut self, src_frame: u64) -> Result<(), DecodeError> {
        self.item.src.seek(src_frame)?;
        self.stream = None;
        self.start_src = src_frame;
        self.produced = 0;
        Ok(())
    }

    /// Device frames from the item's end, if its length is known.
    fn remaining(&self, cfg: Cfg) -> Option<u64> {
        let total = self.item.total_src?;
        let total_dev =
            (u128::from(total) * u128::from(cfg.rate)).div_ceil(u128::from(self.item.src_rate));
        let start_dev =
            u128::from(self.start_src) * u128::from(cfg.rate) / u128::from(self.item.src_rate);
        let done = start_dev + u128::from(self.produced);
        Some(u64::try_from(total_dev.saturating_sub(done)).unwrap_or(u64::MAX))
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

pub(crate) struct Decoder {
    rx: Receiver<DecCmd>,
    /// Where opener threads post [`DecCmd::Opened`].
    tx: Sender<DecCmd>,
    /// Last token handed to an opener thread.
    open_seq: u64,
    /// A `Play` of a stream waiting for its opener: `(token, gen_id, item id)`.
    pending_play: Option<(u64, u64, u64)>,
    /// A seek of that item asked for meanwhile: `(position, gen_id)`.
    pending_seek: Option<(Duration, u64)>,
    /// A `SetNext` of a file or stream waiting for its opener (token). While
    /// it is set, the current item does not end into nothing: it waits.
    pending_next: Option<u64>,
    /// That pending next item is a local file: it opens in moments, so the
    /// current item holds just short of its end for it (see
    /// [`Self::hold_for_next`]).
    pending_next_file: bool,
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
            pending_play: None,
            pending_seek: None,
            pending_next: None,
            pending_next_file: false,
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
            DecCmd::Play { item, gen_id } => self.play(item, gen_id),
            DecCmd::SetNext(item) => self.set_next(item),
            DecCmd::ClearNext => {
                self.next = None;
                self.pending_next = None;
            }
            DecCmd::Seek { pos, id, gen_id } => self.seek(pos, id, gen_id),
            DecCmd::Stop { gen_id } => {
                self.pending_play = None;
                self.drop_pending_seek();
                self.pending_next = None;
                self.last_gen = gen_id;
                self.cur = None;
                self.next = None;
                self.xfade = None;
                self.push_flush(gen_id, None);
            }
            DecCmd::Opened { token, result } => self.opened(token, result),
            DecCmd::Shutdown => return false,
        }
        true
    }

    /// Opens an item on its own thread; the result comes back as
    /// [`DecCmd::Opened`] with the returned token. Counted as a command in
    /// flight until then (the offline device waits for it).
    fn open_async(&mut self, item: PlayItem) -> u64 {
        self.open_seq += 1;
        let token = self.open_seq;
        let id = item.id;
        self.sh.inflight.fetch_add(1, Ordering::AcqRel);
        let tx = self.tx.clone();
        let sh = Arc::clone(&self.sh);
        let spawned = std::thread::Builder::new()
            .name("nikgob-audio-open".into())
            .spawn(move || {
                let result = Item::open(item);
                if tx.send(DecCmd::Opened { token, result }).is_err() {
                    sh.inflight.fetch_sub(1, Ordering::AcqRel);
                }
            });
        if let Err(e) = spawned {
            self.sh.inflight.fetch_sub(1, Ordering::AcqRel);
            self.report(
                Some(id),
                &DecodeError::Stream(format!("cannot start opener: {e}")),
            );
        }
        token
    }

    /// A stream finished opening: start it, or make it the next item, if it
    /// is still wanted.
    fn opened(&mut self, token: u64, result: Result<Item, (u64, DecodeError)>) {
        if let Some((want, gen_id, _)) = self.pending_play
            && want == token
        {
            self.pending_play = None;
            let seek = self.pending_seek.take();
            match result {
                Ok(item) => {
                    let id = item.id;
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
            self.accept_next(result);
        }
        // Anything else was superseded meanwhile and is dropped.
    }

    fn report(&self, id: Option<u64>, error: &DecodeError) {
        let _ = self.events.send(DecEvent::Error {
            id,
            message: error.to_string(),
        });
    }

    fn play(&mut self, item: PlayItem, gen_id: u64) {
        self.pending_play = None;
        self.pending_next = None;
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
            let token = self.open_async(item);
            self.pending_play = Some((token, gen_id, id));
            return;
        }
        match Item::open(item) {
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
        let id = active.id();
        self.last_gen = gen_id;
        self.play_gen = gen_id;
        self.xfade = None;
        self.cur = Some(active);
        self.push_flush(
            gen_id,
            Some(Start {
                id,
                pos_ns: 0,
                announce: true,
            }),
        );
    }

    fn set_next(&mut self, item: PlayItem) {
        // Files open on the opener thread too: probing one (a cold disk, a
        // virus scanner, a big embedded cover) can take longer than the
        // buffered audio, and this runs while the current item plays.
        if !matches!(item.source, ItemSource::Pcm(_)) {
            self.next = None;
            self.pending_next_file = matches!(item.source, ItemSource::File(_));
            self.pending_next = Some(self.open_async(item));
            return;
        }
        self.pending_next = None;
        self.accept_next(Item::open(item));
    }

    fn accept_next(&mut self, result: Result<Item, (u64, DecodeError)>) {
        match result {
            Ok(item) => {
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
        let rate = cur.item.src_rate;
        let was_chained = cur.stream.as_ref().is_some_and(|s| s.chained);
        let result = cur.reposition(target);
        if was_chained {
            // The resampler had read ahead into the next item: rewind it.
            if let Some(next) = self.next.as_mut() {
                let _ = next.reposition(0);
            }
        }
        match result {
            Ok(()) => {
                self.last_gen = gen_id;
                self.push_flush(
                    gen_id,
                    Some(Start {
                        id,
                        pos_ns: frames_to_ns(target, rate),
                        announce: false,
                    }),
                );
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
            self.last_gen = gen_id;
            self.push_flush(
                gen_id,
                Some(Start {
                    id,
                    pos_ns,
                    announce: false,
                }),
            );
        } else {
            self.cur = None;
            self.push_flush(gen_id, None);
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
                Ok(()) => {
                    start = Some(Start {
                        id,
                        pos_ns,
                        announce: unapplied_play,
                    });
                }
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
        let cfg = self.cfg;
        let cross = self.crossfade_frames();
        let mut want = CHUNK_FRAMES as u64;
        if cross > 0
            && self.next.is_some()
            && let Some(rem) = self.cur.as_ref().and_then(|c| c.remaining(cfg))
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
        let gain = cur.gain(normalization);
        let chain = if cross == 0 {
            self.next.as_mut().map(|n| &mut n.item)
        } else {
            None
        };
        match cur.read(cfg, want, &mut self.buf, chain) {
            Ok(StreamRead::Frames(_)) => {
                apply_gain(&mut self.buf, gain);
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
                self.finish_current();
                Step::Worked
            }
            Err(e) => {
                self.report(Some(id), &e);
                self.finish_current();
                Step::Worked
            }
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
        let cfg = self.cfg;
        if let Err(e) = to.prepare(cfg) {
            self.report(Some(to.id()), &e);
            return;
        }
        let from = self.cur.as_ref().map(Active::id);
        let start = Start {
            id: to.id(),
            pos_ns: 0,
            announce: true,
        };
        self.push_mark(Mark {
            at: self.pushed,
            flush: false,
            cfg_gen: cfg.gen_id,
            gen_id: self.last_gen,
            ended: from,
            start: Some(start),
        });
        self.xfade = Some(Crossfade { to, len, done: 0 });
    }

    fn produce_crossfade(&mut self) -> Step {
        let cfg = self.cfg;
        let ch = cfg.channels;
        let normalization = self.sh.normalization.load(Ordering::Relaxed);
        let (Some(cur), Some(x)) = (self.cur.as_mut(), self.xfade.as_mut()) else {
            return Step::Idle;
        };
        let want = (x.len - x.done).min(CHUNK_FRAMES as u64).max(1) as usize;
        let (out_gain, in_gain) = (cur.gain(normalization), x.to.gain(normalization));
        let (cur_id, to_id) = (cur.id(), x.to.id());

        self.buf.clear();
        self.buf_b.clear();
        // Outgoing item: whatever it still has, silence once it is over.
        let from_err = cur.read(cfg, want, &mut self.buf, None).err();
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

        // Equal-power mix over `want` frames; a side that returned fewer
        // frames contributes silence for the rest.
        self.buf.resize(want * ch, 0.0);
        self.buf_b.resize(want * ch, 0.0);
        let len = x.len as f32;
        for i in 0..want {
            let t = (x.done + i as u64) as f32 / len;
            let angle = t * std::f32::consts::FRAC_PI_2;
            let (g_out, g_in) = (angle.cos() * out_gain, angle.sin() * in_gain);
            let base = i * ch;
            for k in base..base + ch {
                self.buf[k] = self.buf[k] * g_out + self.buf_b[k] * g_in;
            }
        }
        x.done += want as u64;
        let finished = x.done >= x.len || to_eof;
        if let Some(e) = from_err {
            self.report(Some(cur_id), &e);
        }
        if let Some(e) = to_err {
            self.report(Some(to_id), &e);
        }
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
        if let Some(mut next) = self.next.take() {
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
                    let start = Start {
                        id: next.id(),
                        pos_ns: 0,
                        announce: true,
                    };
                    self.push_mark(Mark {
                        at: self.pushed,
                        flush: false,
                        cfg_gen: cfg.gen_id,
                        gen_id: self.last_gen,
                        ended: Some(ended_id),
                        start: Some(start),
                    });
                    self.cur = Some(next);
                    return;
                }
                Err(e) => self.report(Some(next.id()), &e),
            }
        }
        self.push_mark(Mark {
            at: self.pushed,
            flush: false,
            cfg_gen: cfg.gen_id,
            gen_id: self.last_gen,
            ended: Some(ended_id),
            start: None,
        });
    }

    // ----- ring access ---------------------------------------------------

    fn push_flush(&mut self, gen_id: u64, start: Option<Start>) {
        self.push_mark(Mark {
            at: self.pushed,
            flush: true,
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
