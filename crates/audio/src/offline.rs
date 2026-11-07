//! A device-less output: the caller pulls the audio the engine renders.
//!
//! Used by the tests, and usable by a headless node (Pi, server) that wants
//! the engine's decoding and sequencing without a sound card.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use fastframe_audio::Render;

use crate::backend::RenderHandle;
use crate::render::dropout_frames;
use crate::shared::{ManualClock, Shared, frames_to_ns};

/// Format of an [`OfflineDevice`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OfflineOptions {
    /// Device rate in Hz.
    pub sample_rate: u32,
    /// Device channel count.
    pub channels: u16,
    /// Frames per simulated audio callback.
    pub block_frames: usize,
}

impl Default for OfflineOptions {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            block_frames: 256,
        }
    }
}

/// The pulling side of [`Player::offline`](crate::Player::offline). Behaves
/// like a sound card: it only "asks for audio" while the player is not paused,
/// and the player's position clock follows the frames it has pulled.
pub struct OfflineDevice {
    options: OfflineOptions,
    handle: RenderHandle,
    clock: Arc<ManualClock>,
    paused: Arc<AtomicBool>,
    sh: Arc<Shared>,
    rendered: u64,
    clock_ns: u64,
}

impl OfflineDevice {
    pub(crate) fn new(
        options: OfflineOptions,
        mut handle: RenderHandle,
        clock: Arc<ManualClock>,
        paused: Arc<AtomicBool>,
        sh: Arc<Shared>,
    ) -> Self {
        handle.configure(options.sample_rate, options.channels);
        Self {
            options,
            handle,
            clock,
            paused,
            sh,
            rendered: 0,
            clock_ns: 0,
        }
    }

    /// Frames pulled so far.
    #[must_use]
    pub fn rendered_frames(&self) -> u64 {
        self.rendered
    }

    /// Whether the player has paused the device (a real one would get no
    /// callbacks).
    #[must_use]
    pub fn is_device_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Pulls `frames` frames (interleaved). Waits for the decoder thread when
    /// it is behind. While the device is paused nothing is pulled and the
    /// result is empty.
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let ch = usize::from(self.options.channels);
        let mut out = Vec::with_capacity(frames * ch);
        let mut left = frames;
        while left > 0 && !self.is_device_paused() {
            let n = left.min(self.options.block_frames);
            self.wait_for_data(n);
            let mut block = vec![0.0; n * ch];
            self.pull(&mut block);
            out.extend_from_slice(&block);
            left -= n;
        }
        out
    }

    /// Pulls `frames` frames the way a sound card does: without waiting for
    /// the decoder, so a decoder that falls behind is heard (and counted) as
    /// a dropout. Empty while the device is paused.
    pub fn render_now(&mut self, frames: usize) -> Vec<f32> {
        let ch = usize::from(self.options.channels);
        let mut out = Vec::with_capacity(frames * ch);
        let mut left = frames;
        while left > 0 && !self.is_device_paused() {
            let n = left.min(self.options.block_frames);
            let mut block = vec![0.0; n * ch];
            self.pull(&mut block);
            out.extend_from_slice(&block);
            left -= n;
        }
        out
    }

    /// Pulls until the playing item ends with nothing following it (or
    /// `max_frames` frames, or the device is paused). The result ends exactly
    /// at the last sample of the last item.
    pub fn drain(&mut self, max_frames: usize) -> Vec<f32> {
        let ch = usize::from(self.options.channels);
        self.sh.end_frame.store(u64::MAX, Ordering::Release);
        let start = self.rendered;
        let mut out = Vec::new();
        while out.len() / ch < max_frames && !self.is_device_paused() {
            let n = self.options.block_frames;
            self.wait_for_data(n);
            let mut block = vec![0.0; n * ch];
            self.pull(&mut block);
            out.extend_from_slice(&block);
            let end = self.sh.end_frame.load(Ordering::Acquire);
            if end != u64::MAX {
                out.truncate((end.saturating_sub(start)) as usize * ch);
                break;
            }
        }
        out
    }

    fn pull(&mut self, block: &mut [f32]) {
        let ch = usize::from(self.options.channels);
        self.handle.render(block);
        let frames = (block.len() / ch) as u64;
        self.rendered += frames;
        self.clock_ns += frames_to_ns(frames, self.options.sample_rate);
        self.clock.set(self.clock_ns);
    }

    /// Simulates the output moving to another device or stream format (a
    /// headphone jack, a new default device): the engine is told through
    /// `configure`, exactly like a real reopen, and carries on from the same
    /// position.
    pub fn reconfigure(&mut self, sample_rate: u32, channels: u16) {
        self.options.sample_rate = sample_rate;
        self.options.channels = channels;
        self.handle.configure(sample_rate, channels);
    }

    fn wait_for_data(&mut self, frames: usize) {
        // A real device's ring is far ahead; with only a block in it the
        // callback would take the decoder for stalled and ramp out.
        let reserve = dropout_frames(self.options.sample_rate) as usize;
        let need = (frames + reserve) * usize::from(self.options.channels);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (available, resyncing) = {
                let renderer = self.handle.0.lock().unwrap_or_else(PoisonError::into_inner);
                (renderer.ring_available(), renderer.waiting_for_decoder())
            };
            if resyncing && Instant::now() <= deadline {
                std::thread::sleep(Duration::from_micros(200));
                continue;
            }
            let quiet = self.sh.inflight.load(Ordering::Acquire) == 0;
            let settled = quiet && self.sh.decoder_idle.load(Ordering::Acquire);
            if (quiet && available >= need) || settled || Instant::now() > deadline {
                // A pending mark (end of item) needs one more look after the
                // decoder settled; the ring may hold less than a block then.
                return;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    }
}
