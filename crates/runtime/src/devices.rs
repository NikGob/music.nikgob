use super::*;

impl Worker {
    /// Moves playback to another device, resuming where it was: at once
    /// with a backend that switches at once, else when the new device has
    /// opened ([`PlaybackEvent::DeviceReady`]).
    pub(crate) fn switch_device(&mut self, device: Option<String>) {
        if self.playback.set_device(device) {
            self.resume_on_new_device();
        } else {
            // A device error from now on is the new device's, not the one heard.
            self.api.switching_device = true;
        }
    }

    /// Engine `generation` on the device asked for has opened: it takes
    /// over and playback goes on from where it was.
    pub(crate) fn device_ready(&mut self, generation: u64) {
        if self.playback.adopt_device(generation) {
            self.resume_on_new_device();
        }
    }

    /// `Command::SetOutputDevice`: the device is chosen as in Settings
    /// (saved with the prefs, switched to live). A name no device has now
    /// is kept: the engine plays on the default output until it is back.
    pub(crate) fn set_output_device(&mut self, device: Option<String>) {
        let device = device.map(|d| d.trim().to_owned()).filter(|d| !d.is_empty());
        let mut p = self.state.read().prefs.clone();
        if p.output_device == device {
            return;
        }
        log::info!("output device: {}", device.as_deref().unwrap_or("the system default"));
        p.output_device = device;
        self.set_prefs(Box::new(p));
    }

    /// The engine was restarted on another device and lost what it played.
    fn resume_on_new_device(&mut self) {
        let volume = if self.muted { 0.0 } else { self.volume };
        self.playback.set_volume(volume);
        if let Some(gain) = self.duck.gain() {
            self.playback.set_duck(gain, Duration::ZERO);
        }
        let prefs = self.state.read().prefs.clone();
        self.playback.configure(crossfade(&prefs), prefs.normalization);
        self.playback.set_equalizer(prefs.eq_on, prefs.eq_gains);
        let Some(e) = self.playing else { return };

        let pos = self.position_now();
        let status = self.status;
        if let Some(req) = self.request(e) {
            self.playback.play(req);
            self.restarted = Some(e.id);
            self.playback.seek(pos);
            if status == Status::Paused {
                self.playback.pause();
            }
            self.position = pos;
            self.position_at = Instant::now();
            self.rate = self.speed;
            self.preload_next();
        }
    }
}
