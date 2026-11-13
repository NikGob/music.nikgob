//! An output device that does not open is said, and a device switch keeps
//! the engine heard until the new device has opened.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, PlayRequest, Playback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs};

fn config() -> RuntimeConfig {
    RuntimeConfig {
        library: Library::open_in_memory().unwrap(),
        streams: Vec::new(),
        library_dir: std::env::temp_dir(),
        prefs: Prefs::default(),
        prefs_path: None,
        settings_path: None,
        import: None,
    }
}

fn wait(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_device_that_does_not_open_is_said() {
    let rt = Runtime::start(config(), Box::new(NullPlayback), Arc::new(|| {}));
    rt.playback_sender().send(PlaybackEvent::DeviceError { generation: 1, message: "no device".into() }).unwrap();
    wait("the toast", || rt.state().toasts.iter().any(|t| t.text == "Не удалось открыть устройство вывода звука"));
}

/// A backend whose device switch opens later, recording what it is told.
#[derive(Clone, Default)]
struct Switching(Arc<Mutex<Vec<String>>>);

impl Switching {
    fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
    fn note(&self, s: String) {
        self.0.lock().unwrap().push(s);
    }
}

impl Playback for Switching {
    fn play(&mut self, _: PlayRequest) {}
    fn set_next(&mut self, _: Option<PlayRequest>) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {
        self.note("volume".into());
    }
    fn position(&self) -> Duration {
        Duration::ZERO
    }
    fn set_device(&mut self, device: Option<String>) -> bool {
        self.note(format!("switch {device:?}"));
        false
    }
    fn adopt_device(&mut self, generation: u64) -> bool {
        self.note(format!("adopt {generation}"));
        generation == 2
    }
    fn drop_device(&mut self, generation: u64) {
        self.note(format!("drop {generation}"));
    }
}

#[test]
fn a_device_switch_waits_for_the_new_device() {
    let backend = Switching::default();
    let rt = Runtime::start(config(), Box::new(backend.clone()), Arc::new(|| {}));
    let mut p = rt.state().prefs.clone();
    p.output_device = Some("USB DAC".into());
    rt.send(Command::SetPrefs(Box::new(p)));
    wait("the switch", || backend.calls().iter().any(|c| c.starts_with("switch")));
    std::thread::sleep(Duration::from_millis(200));
    let before = backend.calls().len();
    assert!(!backend.calls().iter().any(|c| c.starts_with("adopt")), "taken over before it opened: {:?}", backend.calls());
    // It did not open: dropped, the old engine plays on.
    rt.playback_sender().send(PlaybackEvent::DeviceError { generation: 2, message: "busy".into() }).unwrap();
    wait("the drop", || backend.calls().iter().any(|c| c == "drop 2"));
    // It opened: it takes over and playback is set up on it.
    rt.playback_sender().send(PlaybackEvent::DeviceReady { generation: 2 }).unwrap();
    wait("the take-over", || backend.calls()[before..].iter().any(|c| c == "volume"));
    assert!(backend.calls().contains(&"adopt 2".to_owned()));
}
