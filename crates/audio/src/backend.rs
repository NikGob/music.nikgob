//! Where rendered audio goes: the real device through fastframe-audio, or an
//! offline sink driven by hand (tests, and headless nodes without a sound
//! card).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, TryLockError};

use fastframe_audio::{Maintained, OpenError, Output, OutputOptions, Reason, Render};

use crate::render::Renderer;
use crate::shared::PlayClock;

/// The renderer behind a mutex so a failed `Output::open` (which consumes its
/// renderer) can be retried with the same one. The audio callback only ever
/// `try_lock`s: the control thread never holds the lock.
#[derive(Clone)]
pub(crate) struct RenderHandle(pub(crate) Arc<Mutex<Renderer>>);

impl Render for RenderHandle {
    fn configure(&mut self, sample_rate: u32, channels: u16) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .configure(sample_rate, channels);
    }

    fn render(&mut self, out: &mut [f32]) {
        match self.0.try_lock() {
            Ok(mut renderer) => renderer.render(out),
            Err(TryLockError::Poisoned(p)) => p.into_inner().render(out),
            Err(TryLockError::WouldBlock) => out.fill(0.0),
        }
    }
}

/// Things the backend noticed while maintaining the output.
#[derive(Debug)]
pub(crate) enum BackendNote {
    /// A new stream is playing (possibly another device or rate).
    Reopened {
        device: String,
        sample_rate: u32,
        channels: u16,
        /// The listener should hear about it.
        notify: bool,
    },
    Released,
    /// A device problem worth telling the application about.
    Problem {
        fatal: bool,
        message: String,
    },
}

pub(crate) trait Backend: Send {
    /// Makes sure a stream is open (the device may have been missing).
    /// Returns an error message when it cannot be.
    fn ensure_open(&mut self) -> Result<(), String>;
    fn is_open(&self) -> bool;
    fn pause(&mut self);
    fn resume(&mut self);
    fn maintain(&mut self) -> Vec<BackendNote>;
}

/// The real thing: fastframe-audio's cpal output.
pub(crate) struct DeviceBackend {
    options: OutputOptions,
    handle: RenderHandle,
    clock_cell: Arc<OnceLock<PlayClock>>,
    output: Option<Output<RenderHandle>>,
}

impl DeviceBackend {
    pub(crate) fn new(
        options: OutputOptions,
        handle: RenderHandle,
        clock_cell: Arc<OnceLock<PlayClock>>,
    ) -> Self {
        Self {
            options,
            handle,
            clock_cell,
            output: None,
        }
    }
}

impl DeviceBackend {
    /// The options to open the output with. On Linux a device named by its
    /// PulseAudio sink name is opened by its display name, which is the
    /// one fastframe-audio knows (see [`crate::devices`]); a name no device
    /// has now is left for fastframe-audio, which plays on the default
    /// output instead.
    fn options_to_open(&self) -> OutputOptions {
        #[allow(unused_mut)]
        let mut options = self.options.clone();
        #[cfg(target_os = "linux")]
        if let fastframe_audio::Device::Named(wanted) = options.device.clone() {
            match crate::devices::output_devices() {
                Ok(devices) => {
                    if let Some(name) = crate::devices::resolve_device(&wanted, &devices)
                        && name != wanted
                    {
                        log::info!("audio device {wanted:?} is {name:?}");
                        options.device = fastframe_audio::Device::Named(name);
                    }
                }
                Err(e) => log::warn!("audio devices: {e}"),
            }
        }
        options
    }
}

impl Backend for DeviceBackend {
    fn ensure_open(&mut self) -> Result<(), String> {
        if self.output.is_some() {
            return Ok(());
        }
        match Output::open(self.options_to_open(), self.handle.clone()) {
            Ok(output) => {
                let _ = self.clock_cell.set(PlayClock::Device(output.clock()));
                log::info!(
                    "audio device: {} at {} Hz, {} channels",
                    output.device_name(),
                    output.sample_rate(),
                    output.channels()
                );
                self.output = Some(output);
                Ok(())
            }
            Err(OpenError::NoDevice) => Err("there is no audio output device".into()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn is_open(&self) -> bool {
        self.output.is_some()
    }

    fn pause(&mut self) {
        if let Some(output) = &mut self.output {
            output.pause();
        }
    }

    fn resume(&mut self) {
        if let Some(output) = &mut self.output {
            output.resume();
        }
    }

    fn maintain(&mut self) -> Vec<BackendNote> {
        let mut notes = Vec::new();
        let Some(output) = &mut self.output else {
            return notes;
        };
        match output.maintain() {
            Maintained::Unchanged => {}
            Maintained::Reopened {
                device,
                sample_rate,
                channels,
                reason,
            } => notes.push(BackendNote::Reopened {
                device,
                sample_rate,
                channels,
                notify: reason != Reason::Resumed,
            }),
            Maintained::Released => notes.push(BackendNote::Released),
            Maintained::Failed(e) => notes.push(BackendNote::Problem {
                fatal: false,
                message: e.to_string(),
            }),
        }
        for error in output.take_errors() {
            notes.push(BackendNote::Problem {
                fatal: error.is_fatal(),
                message: error.to_string(),
            });
        }
        notes
    }
}

/// A backend that plays nowhere: a test (or a node without a sound card)
/// pulls the audio itself through [`crate::OfflineDevice`].
pub(crate) struct OfflineBackend {
    pub(crate) paused: Arc<AtomicBool>,
}

impl Backend for OfflineBackend {
    fn ensure_open(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn pause(&mut self) {
        self.paused.store(true, Ordering::Release);
    }

    fn resume(&mut self) {
        self.paused.store(false, Ordering::Release);
    }

    fn maintain(&mut self) -> Vec<BackendNote> {
        Vec::new()
    }
}
