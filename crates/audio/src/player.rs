//! The public handle and the control thread behind it.
//!
//! Threads: the **control thread** owns the output device, turns callback and
//! decoder reports into [`Event`]s, and sequences pause/resume with the
//! device; the **decoder thread** ([`crate::pipeline`]) produces audio; the
//! **audio callback** ([`crate::render`]) consumes it. The `Player` handle
//! talks to the control thread through a channel and reads the playback
//! position straight from lock-free shared state.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use fastframe_audio::{Buffer, BufferSize, OutputOptions};

use crate::analyzer::Analyzer;
use crate::backend::{Backend, BackendNote, DeviceBackend, OfflineBackend, RenderHandle};
use crate::offline::{OfflineDevice, OfflineOptions};
use crate::pipeline::{DecCmd, DecEvent, Decoder};
use crate::render::{CbEvent, Mark, Renderer};
use crate::shared::{LogLimiter, ManualClock, PlayClock, Shared, volume_curve};
use crate::tap::VisualizerTap;
use crate::types::{Event, PlayItem, SpawnError};

/// Samples the ring can hold (about 5 s of 48 kHz stereo; the decoder keeps
/// it only about 1 s full).
const RING_SAMPLES: usize = 1 << 19;
const MARK_QUEUE: usize = 1024;
const EVENT_QUEUE: usize = 512;
/// How often `Position` events go out while playing.
const POSITION_INTERVAL: Duration = Duration::from_millis(250);
/// How often the output is checked for failures / default-device changes.
const MAINTAIN_INTERVAL: Duration = Duration::from_secs(1);
/// If the callback has not confirmed its pause fade by then, pause anyway.
const PAUSE_GRACE: Duration = Duration::from_millis(300);
/// After playback ends, keep the device running this long (so its buffer
/// drains to silence) before pausing it.
const IDLE_PAUSE_DELAY: Duration = Duration::from_millis(400);
/// At most one "the output ran dry" warning this often.
const UNDERRUN_LOG_EVERY: Duration = Duration::from_secs(10);
/// The output's timing (late callbacks, the ring's low-water mark) is
/// summed up this often while it runs.
const TIMING_REPORT_EVERY: Duration = Duration::from_secs(10);
/// Less decoded audio ahead than this at some callback is worth an info line.
const RING_LOW_NOTE: Duration = Duration::from_millis(500);
/// How much the device buffers: the render thread's slack against a busy
/// CPU. Seeks, track changes, pauses and volume changes are heard this much
/// later; the played clock (so the position, the lyrics and the visualizer)
/// accounts for it.
const DEVICE_BUFFER: Duration = Duration::from_millis(90);

/// A callback for the application's event loop: called (from the engine's
/// threads) every time an [`Event`] is queued, e.g. to request a repaint.
pub type WakeFn = Box<dyn Fn() + Send + Sync>;

/// Options for [`Player::spawn`].
pub struct PlayerOptions {
    /// How to open the output device.
    pub output: OutputOptions,
    /// Called after each queued event.
    pub wake: Option<WakeFn>,
}

impl Default for PlayerOptions {
    fn default() -> Self {
        Self {
            output: OutputOptions {
                // Shared-mode WASAPI underruns with the driver's own buffer.
                buffer: Buffer::FixedOnWindows(BufferSize::Duration(DEVICE_BUFFER)),
                ..OutputOptions::default()
            },
            wake: None,
        }
    }
}

impl std::fmt::Debug for PlayerOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerOptions")
            .field("output", &self.output)
            .field("wake", &self.wake.is_some())
            .finish()
    }
}

enum Command {
    Play { item: PlayItem, gen_id: u64 },
    SetNext(PlayItem),
    ClearNext,
    Pause,
    Resume,
    Toggle,
    Stop { gen_id: u64 },
    Seek { pos: Duration, id: u64, gen_id: u64 },
}

/// A position the listener asked for that the audio callback has not applied
/// yet (play, seek): reported by [`Player::position`] meanwhile.
#[derive(Clone, Copy, Debug)]
struct PendingPos {
    gen_id: u64,
    id: u64,
    pos_ns: u64,
}

struct Inner {
    cmd: Sender<Command>,
    sh: Arc<Shared>,
    clock: Arc<OnceLock<PlayClock>>,
    pending: Arc<Mutex<Option<PendingPos>>>,
    gen_counter: AtomicU64,
    slider: Mutex<f32>,
    paused: Arc<AtomicBool>,
}

/// The audio engine's handle. Cheap to clone and `Send + Sync`; every method
/// returns immediately.
#[derive(Clone)]
pub struct Player {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player")
            .field("current", &self.current_id())
            .field("paused", &self.is_paused())
            .finish()
    }
}

impl Player {
    /// Starts the engine on the system's audio output. Opening the device
    /// happens on the control thread; a missing device is reported as an
    /// [`Event::Error`] and retried when playback is requested.
    pub fn spawn(options: PlayerOptions) -> Result<(Player, Receiver<Event>), SpawnError> {
        let PlayerOptions { output, wake } = options;
        let parts = Parts::new();
        let buffer = match output.buffer {
            Buffer::Fixed(BufferSize::Duration(d)) => Some(d),
            Buffer::FixedOnWindows(BufferSize::Duration(d)) if cfg!(windows) => Some(d),
            _ => None,
        };
        if let Some(d) = buffer {
            parts.shared.device_buffer_ns.store(
                u64::try_from(d.as_nanos()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
        }
        let backend = DeviceBackend::new(
            output,
            RenderHandle(Arc::clone(&parts.renderer)),
            Arc::clone(&parts.clock),
        );
        let (player, events) = parts.start(Box::new(backend), wake)?;
        Ok((player, events))
    }

    /// Starts the engine without a device: nothing is played unless the
    /// returned [`OfflineDevice`] is pulled. For tests and for nodes without
    /// a sound card.
    pub fn offline(
        options: OfflineOptions,
    ) -> Result<(Player, Receiver<Event>, OfflineDevice), SpawnError> {
        let parts = Parts::new();
        let paused = Arc::new(AtomicBool::new(false));
        let manual = Arc::new(ManualClock::default());
        let _ = parts.clock.set(PlayClock::Manual(Arc::clone(&manual)));
        let device = OfflineDevice::new(
            options,
            RenderHandle(Arc::clone(&parts.renderer)),
            manual,
            Arc::clone(&paused),
            Arc::clone(&parts.shared),
        );
        let (player, events) = parts.start(Box::new(OfflineBackend { paused }), None)?;
        Ok((player, events, device))
    }

    fn next_gen(&self) -> u64 {
        self.inner.gen_counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn send(&self, cmd: Command) {
        if matches!(
            cmd,
            Command::Play { .. }
                | Command::SetNext(_)
                | Command::ClearNext
                | Command::Stop { .. }
                | Command::Seek { .. }
        ) {
            self.inner.sh.inflight.fetch_add(1, Ordering::AcqRel);
        }
        // A closed channel means the engine is shut down; nothing to do.
        let _ = self.inner.cmd.send(cmd);
    }

    fn set_pending(&self, p: Option<PendingPos>) {
        *self
            .inner
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = p;
    }

    /// Plays `item` now, replacing whatever plays. Clears the prepared next
    /// item: call [`set_next`](Self::set_next) afterwards.
    pub fn play(&self, item: PlayItem) {
        let gen_id = self.next_gen();
        self.set_pending(Some(PendingPos {
            gen_id,
            id: item.id,
            pos_ns: 0,
        }));
        self.send(Command::Play { item, gen_id });
    }

    /// Prepares the item that follows the current one. The decoder opens it
    /// right away and starts it at the exact end of the current item, so
    /// there is no gap (or a crossfade, see
    /// [`set_crossfade`](Self::set_crossfade)).
    pub fn set_next(&self, item: PlayItem) {
        self.send(Command::SetNext(item));
    }

    /// Forgets the prepared next item.
    pub fn clear_next(&self) {
        self.send(Command::ClearNext);
    }

    /// Pauses with a short fade; the device stops asking for audio.
    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    /// Continues after [`pause`](Self::pause).
    pub fn resume(&self) {
        self.send(Command::Resume);
    }

    /// Pauses if playing, resumes if paused.
    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    /// Stops and forgets the current and next items.
    pub fn stop(&self) {
        let gen_id = self.next_gen();
        self.set_pending(None);
        self.send(Command::Stop { gen_id });
    }

    /// Seeks the current item (sample-accurate). [`position`](Self::position)
    /// reports the target immediately.
    pub fn seek(&self, pos: Duration) {
        let Some(id) = self.current_id() else { return };
        let gen_id = self.next_gen();
        self.set_pending(Some(PendingPos {
            gen_id,
            id,
            pos_ns: u64::try_from(pos.as_nanos()).unwrap_or(u64::MAX),
        }));
        self.send(Command::Seek { pos, id, gen_id });
    }

    /// Sets the volume, 0.0 to 1.0, on a perceptual (cubic) curve. Takes
    /// effect within a few milliseconds without zipper noise.
    pub fn set_volume(&self, volume: f32) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        *self
            .inner
            .slider
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = v;
        self.inner
            .sh
            .volume
            .store(volume_curve(v).to_bits(), Ordering::Relaxed);
    }

    /// The last value given to [`set_volume`](Self::set_volume).
    #[must_use]
    pub fn volume(&self) -> f32 {
        *self
            .inner
            .slider
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// `Some(d)`: equal-power crossfade of length `d` between the current
    /// and the next item; `None`: gapless. Takes effect for the next
    /// transition (items of unknown length are never crossfaded).
    pub fn set_crossfade(&self, duration: Option<Duration>) {
        let ms = duration.map_or(0, |d| u32::try_from(d.as_millis()).unwrap_or(u32::MAX));
        self.inner.sh.crossfade_ms.store(ms, Ordering::Relaxed);
    }

    /// Applies each item's `gain_db` (or ignores it). The change is heard
    /// after the audio already buffered (about 1 s) has played.
    pub fn set_normalization(&self, on: bool) {
        self.inner.sh.normalization.store(on, Ordering::Relaxed);
    }

    /// Whether the listener paused playback.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::Acquire)
    }

    /// Time-based position of the audible item, from the output device's
    /// played clock (so it stays smooth and accounts for device latency).
    /// `None` when nothing is loaded. Safe to call every frame.
    #[must_use]
    pub fn position(&self) -> Option<Duration> {
        position_of(&self.inner.sh, &self.inner.clock, &self.inner.pending).map(|(_, p)| p)
    }

    /// Id of the audible item (or of the one just requested).
    #[must_use]
    pub fn current_id(&self) -> Option<u64> {
        position_of(&self.inner.sh, &self.inner.clock, &self.inner.pending).map(|(id, _)| id)
    }

    /// The raw visualizer sample ring.
    #[must_use]
    pub fn visualizer_tap(&self) -> Arc<VisualizerTap> {
        Arc::clone(&self.inner.sh.tap)
    }

    /// A spectrum analyzer over the visualizer tap. Create one per
    /// visualizer; call [`Analyzer::update`] only while it is visible.
    #[must_use]
    pub fn analyzer(&self) -> Analyzer {
        Analyzer::new(self.visualizer_tap())
    }

    /// Callback blocks that ran dry while playing (diagnostics).
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.inner.sh.underruns.load(Ordering::Relaxed)
    }

    /// Device callbacks that came late or found the device buffer drained
    /// while playing: the device played silence (diagnostics).
    #[must_use]
    pub fn late_callbacks(&self) -> u64 {
        self.inner.sh.late_callbacks.load(Ordering::Relaxed)
    }
}

fn position_of(
    sh: &Shared,
    clock: &OnceLock<PlayClock>,
    pending: &Mutex<Option<PendingPos>>,
) -> Option<(u64, Duration)> {
    let snap = sh.cursor.read();
    let pending = *pending.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(p) = pending
        && snap.applied_gen < p.gen_id
    {
        return Some((p.id, Duration::from_nanos(p.pos_ns)));
    }
    if !snap.has_item {
        return None;
    }
    let mut pos = snap.pos_ns;
    if snap.moving
        && let Some(clock) = clock.get()
    {
        pos += clock.played_ns().saturating_sub(snap.clock_ns);
    }
    Some((snap.id, Duration::from_nanos(pos)))
}

/// Everything created before the threads start.
struct Parts {
    shared: Arc<Shared>,
    clock: Arc<OnceLock<PlayClock>>,
    renderer: Arc<Mutex<Renderer>>,
    ring_tx: rtrb::Producer<f32>,
    marks_tx: rtrb::Producer<Mark>,
    cb_events: rtrb::Consumer<CbEvent>,
}

impl Parts {
    fn new() -> Self {
        let shared = Arc::new(Shared::new());
        let clock = Arc::new(OnceLock::new());
        let (ring_tx, ring_rx) = rtrb::RingBuffer::new(RING_SAMPLES);
        let (marks_tx, marks_rx) = rtrb::RingBuffer::new(MARK_QUEUE);
        let (cb_tx, cb_events) = rtrb::RingBuffer::new(EVENT_QUEUE);
        let renderer = Renderer::new(
            ring_rx,
            marks_rx,
            cb_tx,
            Arc::clone(&shared),
            Arc::clone(&clock),
        );
        Self {
            shared,
            clock,
            renderer: Arc::new(Mutex::new(renderer)),
            ring_tx,
            marks_tx,
            cb_events,
        }
    }

    fn start(
        self,
        backend: Box<dyn Backend>,
        wake: Option<WakeFn>,
    ) -> Result<(Player, Receiver<Event>), SpawnError> {
        let (cmd_tx, cmd_rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let (dec_tx, dec_rx) = unbounded();
        let (dec_ev_tx, dec_ev_rx) = unbounded();
        let pending = Arc::new(Mutex::new(None));
        let paused = Arc::new(AtomicBool::new(false));

        let decoder = Decoder::new(
            dec_rx,
            dec_tx.clone(),
            dec_ev_tx,
            Arc::clone(&self.shared),
            self.ring_tx,
            self.marks_tx,
        );
        let decoder_thread = std::thread::Builder::new()
            .name("nikgob-audio-decoder".into())
            .spawn(move || decoder.run())?;

        let control = Control {
            rx: cmd_rx,
            dec_tx,
            dec_events: dec_ev_rx,
            cb_events: self.cb_events,
            events: ev_tx,
            wake: wake.map(Arc::from),
            sh: Arc::clone(&self.shared),
            clock: Arc::clone(&self.clock),
            pending: Arc::clone(&pending),
            paused: Arc::clone(&paused),
            backend,
            decoder_thread: Some(decoder_thread),
            is_paused: false,
            device_paused: false,
            pause_deadline: None,
            idle_pause_at: None,
            durations: VecDeque::new(),
            last_position: Instant::now(),
            last_maintain: Instant::now(),
            last_open_error: None,
            underruns_seen: 0,
            underrun_log: LogLimiter::new(UNDERRUN_LOG_EVERY),
            late_seen: 0,
            timing_since: Instant::now(),
        };
        std::thread::Builder::new()
            .name("nikgob-audio-control".into())
            .spawn(move || control.run())?;

        let inner = Inner {
            cmd: cmd_tx,
            sh: self.shared,
            clock: self.clock,
            pending,
            gen_counter: AtomicU64::new(0),
            slider: Mutex::new(1.0),
            paused,
        };
        Ok((
            Player {
                inner: Arc::new(inner),
            },
            ev_rx,
        ))
    }
}

struct Control {
    rx: Receiver<Command>,
    dec_tx: Sender<DecCmd>,
    dec_events: Receiver<DecEvent>,
    cb_events: rtrb::Consumer<CbEvent>,
    events: Sender<Event>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    sh: Arc<Shared>,
    clock: Arc<OnceLock<PlayClock>>,
    pending: Arc<Mutex<Option<PendingPos>>>,
    paused: Arc<AtomicBool>,
    backend: Box<dyn Backend>,
    decoder_thread: Option<JoinHandle<()>>,

    /// The listener's pause state.
    is_paused: bool,
    /// The device stream itself is paused (no callbacks).
    device_paused: bool,
    pause_deadline: Option<Instant>,
    idle_pause_at: Option<Instant>,
    /// Lengths of recently opened items, for `Started` events.
    durations: VecDeque<(u64, Option<Duration>)>,
    last_position: Instant,
    last_maintain: Instant,
    last_open_error: Option<Instant>,
    /// Underruns already reported (or seen) and the rate limit of the report.
    underruns_seen: u64,
    underrun_log: LogLimiter,
    /// Late callbacks already reported, and when the timing report began.
    late_seen: u64,
    timing_since: Instant,
}

impl Control {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        if let Some(wake) = &self.wake {
            wake();
        }
    }

    fn run(mut self) {
        self.open_device();
        // Nothing plays yet: do not keep the device calling back.
        self.idle_pause_at = Some(Instant::now() + IDLE_PAUSE_DELAY);
        loop {
            let timeout = if self.device_paused {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(15)
            };
            match self.rx.recv_timeout(timeout) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.poll_decoder();
            self.poll_callback();
            self.tick();
        }
        let _ = self.dec_tx.send(DecCmd::Shutdown);
        if let Some(handle) = self.decoder_thread.take() {
            let _ = handle.join();
        }
    }

    fn open_device(&mut self) -> bool {
        match self.backend.ensure_open() {
            Ok(()) => true,
            Err(message) => {
                // Do not spam: one report per ten seconds.
                let due = self
                    .last_open_error
                    .is_none_or(|t| t.elapsed() > Duration::from_secs(10));
                if due {
                    self.last_open_error = Some(Instant::now());
                    self.emit(Event::Error { id: None, message });
                }
                false
            }
        }
    }

    fn remember_duration(&mut self, id: u64, duration: Option<Duration>) {
        self.durations.retain(|(i, _)| *i != id);
        self.durations.push_back((id, duration));
        while self.durations.len() > 8 {
            self.durations.pop_front();
        }
    }

    fn duration_of(&self, id: u64) -> Option<Duration> {
        self.durations
            .iter()
            .find(|(i, _)| *i == id)
            .and_then(|(_, d)| *d)
    }

    fn has_current(&self) -> bool {
        position_of(&self.sh, &self.clock, &self.pending).is_some()
    }

    fn wake_device(&mut self) {
        if self.device_paused {
            self.backend.resume();
            self.device_paused = false;
        }
    }

    fn set_paused(&mut self, paused: bool) {
        self.is_paused = paused;
        self.paused.store(paused, Ordering::Release);
    }

    fn do_resume(&mut self) {
        if !self.is_paused {
            return;
        }
        self.set_paused(false);
        self.pause_deadline = None;
        self.sh.pause_req.store(false, Ordering::Release);
        self.wake_device();
        self.emit(Event::Resumed);
    }

    fn do_pause(&mut self) {
        if self.is_paused || !self.has_current() {
            return;
        }
        self.set_paused(true);
        self.idle_pause_at = None;
        self.sh.pause_req.store(true, Ordering::Release);
        self.pause_deadline = Some(Instant::now() + PAUSE_GRACE);
        self.emit(Event::Paused);
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Play { item, gen_id } => {
                self.idle_pause_at = None;
                if self.is_paused {
                    self.do_resume();
                }
                if self.open_device() {
                    self.wake_device();
                }
                let _ = self.dec_tx.send(DecCmd::Play { item, gen_id });
            }
            Command::SetNext(item) => {
                let _ = self.dec_tx.send(DecCmd::SetNext(item));
            }
            Command::ClearNext => {
                let _ = self.dec_tx.send(DecCmd::ClearNext);
            }
            Command::Pause => self.do_pause(),
            Command::Resume => self.do_resume(),
            Command::Toggle => {
                if self.is_paused {
                    self.do_resume();
                } else {
                    self.do_pause();
                }
            }
            Command::Stop { gen_id } => {
                let was_paused = self.is_paused;
                if was_paused {
                    // Leave the device paused; the flush is applied when it
                    // next runs.
                    self.set_paused(false);
                    self.pause_deadline = None;
                    self.sh.pause_req.store(false, Ordering::Release);
                }
                let _ = self.dec_tx.send(DecCmd::Stop { gen_id });
                self.idle_pause_at = Some(Instant::now() + IDLE_PAUSE_DELAY);
                self.emit(Event::Stopped);
            }
            Command::Seek { pos, id, gen_id } => {
                let _ = self.dec_tx.send(DecCmd::Seek { pos, id, gen_id });
                if let Some((id, p)) = position_of(&self.sh, &self.clock, &self.pending) {
                    self.emit(Event::Position { id, pos: p });
                }
            }
        }
    }

    fn poll_callback(&mut self) {
        while let Ok(event) = self.cb_events.pop() {
            match event {
                CbEvent::Started(id) => {
                    let duration = self.duration_of(id);
                    self.emit(Event::Started { id, duration });
                }
                CbEvent::Changed { from, to } => {
                    let duration = self.duration_of(to);
                    self.emit(Event::TrackChanged { from, to, duration });
                }
                CbEvent::Ended(id) => {
                    self.emit(Event::Ended { id });
                    self.emit(Event::QueueEmpty);
                    self.idle_pause_at = Some(Instant::now() + IDLE_PAUSE_DELAY);
                }
                CbEvent::PauseReady => {
                    if self.is_paused && !self.device_paused {
                        // The fade is rendered, not heard: let the device
                        // play out its buffer first, or pausing cuts the
                        // music before the fade and resuming plays the rest.
                        let latency = self.clock.get().map_or(Duration::ZERO, PlayClock::latency);
                        if latency.is_zero() {
                            self.backend.pause();
                            self.device_paused = true;
                            self.pause_deadline = None;
                        } else {
                            self.pause_deadline = Some(Instant::now() + latency);
                        }
                    }
                }
            }
        }
    }

    fn poll_decoder(&mut self) {
        while let Ok(event) = self.dec_events.try_recv() {
            match event {
                DecEvent::Opened { id, duration } => self.remember_duration(id, duration),
                DecEvent::Error { id, message } => {
                    if let Some(id) = id {
                        let mut pending =
                            self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                        if pending.is_some_and(|p| p.id == id) {
                            // The play/seek that was waiting for this item
                            // will never be applied.
                            *pending = None;
                        }
                    }
                    self.emit(Event::Error { id, message });
                }
                DecEvent::SeekDropped { gen_id } => {
                    let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
                    if pending.is_some_and(|p| p.gen_id == gen_id) {
                        *pending = None;
                    }
                }
            }
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();

        if let Some(deadline) = self.pause_deadline
            && now >= deadline
            && self.is_paused
            && !self.device_paused
        {
            self.backend.pause();
            self.device_paused = true;
            self.pause_deadline = None;
        }
        if let Some(at) = self.idle_pause_at
            && now >= at
        {
            self.idle_pause_at = None;
            if !self.device_paused && !self.is_paused && !self.has_current() {
                self.backend.pause();
                self.device_paused = true;
            }
        }

        if !self.is_paused && now.duration_since(self.last_position) >= POSITION_INTERVAL {
            self.last_position = now;
            if let Some((id, pos)) = position_of(&self.sh, &self.clock, &self.pending)
                && self.sh.cursor.read().moving
            {
                self.emit(Event::Position { id, pos });
            }
        }

        self.report_underruns(now);
        self.report_timing(now);

        let maintain_every = if self.device_paused {
            Duration::from_secs(3)
        } else {
            MAINTAIN_INTERVAL
        };
        if now.duration_since(self.last_maintain) >= maintain_every {
            self.last_maintain = now;
            self.maintain();
        }
    }

    /// Logs the callback blocks that found the ring short (the callback only
    /// bumps an atomic counter), at most once per [`UNDERRUN_LOG_EVERY`].
    fn report_underruns(&mut self, now: Instant) {
        let total = self.sh.underruns.load(Ordering::Relaxed);
        let new = total.saturating_sub(self.underruns_seen);
        self.underruns_seen = total;
        if let Some(n) = self.underrun_log.record(new, now) {
            log::warn!(
                "audio: the output ran dry {n} time(s) since the last report ({total} in all): the decoder fell behind"
            );
        }
    }

    /// Logs what the callback measured of the output's timing: callbacks
    /// that came late (the device ran dry, with the ring full), the longest
    /// gap between two, and the least decoded audio ahead.
    fn report_timing(&mut self, now: Instant) {
        if now.duration_since(self.timing_since) < TIMING_REPORT_EVERY {
            return;
        }
        self.timing_since = now;
        let total = self.sh.late_callbacks.load(Ordering::Relaxed);
        let late = total.saturating_sub(self.late_seen);
        self.late_seen = total;
        let worst_ms = self.sh.worst_callback_gap_us.swap(0, Ordering::Relaxed) / 1000;
        let low = self.sh.ring_low_samples.swap(u64::MAX, Ordering::Relaxed);
        let per_ms = u64::from(self.sh.cfg_rate.load(Ordering::Relaxed))
            * u64::from(self.sh.cfg_channels.load(Ordering::Relaxed))
            / 1000;
        let low_ms = (low != u64::MAX && per_ms > 0).then(|| low / per_ms);
        let low_text = low_ms.map_or_else(|| "not measured".to_owned(), |ms| format!("{ms} ms"));
        if late > 0 {
            let buffer_ms = self.sh.device_buffer_ns.load(Ordering::Relaxed) / 1_000_000;
            log::warn!(
                "audio: the output callback came late {late} time(s) in the last {} s ({total} in all; longest gap {worst_ms} ms with a {buffer_ms} ms device buffer, least audio ahead {low_text}): the render thread was held up, and past the buffer the device plays silence",
                TIMING_REPORT_EVERY.as_secs()
            );
        } else if low_ms.is_some_and(|ms| ms < RING_LOW_NOTE.as_millis() as u64) {
            log::info!(
                "audio: decoded audio ahead fell to {low_text} in the last {} s (longest callback gap {worst_ms} ms)",
                TIMING_REPORT_EVERY.as_secs()
            );
        } else if low_ms.is_some() {
            log::debug!(
                "audio: least audio ahead {low_text}, longest callback gap {worst_ms} ms in the last {} s",
                TIMING_REPORT_EVERY.as_secs()
            );
        }
    }

    fn maintain(&mut self) {
        if !self.backend.is_open() {
            // Try again only if somebody wants sound.
            if self.has_current() {
                self.open_device();
            }
            return;
        }
        for note in self.backend.maintain() {
            match note {
                BackendNote::Reopened {
                    device,
                    sample_rate,
                    channels,
                    notify,
                } => {
                    if notify {
                        self.emit(Event::DeviceChanged {
                            device,
                            sample_rate,
                            channels,
                        });
                    }
                }
                BackendNote::Released => {
                    log::debug!("audio device released after a long pause");
                }
                BackendNote::Problem { fatal, message } => {
                    if fatal {
                        let id =
                            position_of(&self.sh, &self.clock, &self.pending).map(|(id, _)| id);
                        self.emit(Event::Error { id, message });
                    } else {
                        log::debug!("audio device: {message}");
                    }
                }
            }
        }
    }
}
