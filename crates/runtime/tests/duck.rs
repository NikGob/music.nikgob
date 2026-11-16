//! The duck: lowered at once, restored by itself when its ttl ends (to a
//! tenth of a second, not at a poll), extended by a new request, kept over
//! a device switch; and the output device chosen by a command.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_runtime::{PlayRequest, Playback, Runtime, RuntimeConfig};
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
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// What reaches the engine: the ducks with when they came, and the
/// devices.
#[derive(Clone, Default)]
struct Ducks {
    ducks: Arc<Mutex<Vec<(f32, Duration, Instant)>>>,
    devices: Arc<Mutex<Vec<Option<String>>>>,
}

impl Ducks {
    fn ducks(&self) -> Vec<(f32, Duration, Instant)> {
        self.ducks.lock().unwrap().clone()
    }
    fn devices(&self) -> Vec<Option<String>> {
        self.devices.lock().unwrap().clone()
    }
}

impl Playback for Ducks {
    fn play(&mut self, _: PlayRequest) {}
    fn set_next(&mut self, _: Option<PlayRequest>) {}
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn stop(&mut self) {}
    fn seek(&mut self, _: Duration) {}
    fn set_volume(&mut self, _: f32) {}
    fn position(&self) -> Duration {
        Duration::ZERO
    }
    fn set_duck(&mut self, gain: f32, fade: Duration) {
        self.ducks.lock().unwrap().push((gain, fade, Instant::now()));
    }
    fn set_device(&mut self, device: Option<String>) -> bool {
        self.devices.lock().unwrap().push(device);
        // Restarted at once: the runtime sets it up again, the duck too.
        true
    }
}

#[test]
fn a_duck_ends_by_itself_on_time_and_shows_meanwhile() {
    let backend = Ducks::default();
    let rt = Runtime::start(config(), Box::new(backend.clone()), Arc::new(|| {}));
    rt.send(Command::SetDuck { gain: 0.18, fade_ms: 120, ttl_ms: 300 });
    wait("the duck", || !backend.ducks().is_empty());
    // The ttl runs from when the loop took the request.
    let asked = backend.ducks()[0].2;
    assert_eq!(backend.ducks()[0].0, 0.18);
    assert_eq!(backend.ducks()[0].1, Duration::from_millis(120));
    wait("the duck shown", || rt.state().player.duck.is_some());
    let shown = rt.state().player.duck.unwrap();
    assert_eq!(shown.gain, 0.18);
    assert!(shown.until_unix_ms > 0);

    wait("the restore", || backend.ducks().len() >= 2);
    let (gain, fade, at) = backend.ducks()[1];
    assert_eq!(gain, 1.0);
    assert_eq!(fade, Duration::from_millis(450), "the restore fades over 450 ms");
    let after = at.duration_since(asked);
    assert!(after >= Duration::from_millis(300), "restored early: {after:?}");
    assert!(after < Duration::from_millis(300 + 100), "restored late: {after:?}");
    wait("the duck gone", || rt.state().player.duck.is_none());
}

#[test]
fn a_new_duck_before_the_end_extends_it_and_a_clear_ends_it_once() {
    let backend = Ducks::default();
    let rt = Runtime::start(config(), Box::new(backend.clone()), Arc::new(|| {}));
    rt.send(Command::SetDuck { gain: 0.03, fade_ms: 80, ttl_ms: 400 });
    wait("the first duck", || !backend.ducks().is_empty());
    std::thread::sleep(Duration::from_millis(250));
    rt.send(Command::SetDuck { gain: 0.18, fade_ms: 120, ttl_ms: 400 });
    // Past the first end, before the second.
    std::thread::sleep(Duration::from_millis(250));
    let gains: Vec<f32> = backend.ducks().iter().map(|d| d.0).collect();
    assert_eq!(gains, [0.03, 0.18], "not restored at the first end");
    rt.send(Command::ClearDuck { fade_ms: 200 });
    wait("the clear", || backend.ducks().len() == 3);
    assert_eq!(backend.ducks()[2].0, 1.0);
    assert_eq!(backend.ducks()[2].1, Duration::from_millis(200));
    // A clear without a duck, and the old end passing: nothing more.
    rt.send(Command::ClearDuck { fade_ms: 450 });
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(backend.ducks().len(), 3);
    assert!(rt.state().player.duck.is_none());
}

#[test]
fn odd_values_are_kept_within_the_limits() {
    let backend = Ducks::default();
    let rt = Runtime::start(config(), Box::new(backend.clone()), Arc::new(|| {}));
    rt.send(Command::SetDuck { gain: 3.0, fade_ms: 60_000, ttl_ms: 1 });
    wait("the duck", || !backend.ducks().is_empty());
    assert_eq!(backend.ducks()[0].0, 1.0);
    assert_eq!(backend.ducks()[0].1, Duration::from_millis(5000));
    // The ttl is at least 100 ms.
    wait("the restore", || backend.ducks().len() >= 2);
    let d = backend.ducks();
    assert!(d[1].2.duration_since(d[0].2) >= Duration::from_millis(95), "{:?}", d[1].2.duration_since(d[0].2));
}

#[test]
fn the_output_device_is_chosen_saved_and_the_duck_kept_on_it() {
    let backend = Ducks::default();
    let rt = Runtime::start(config(), Box::new(backend.clone()), Arc::new(|| {}));
    rt.send(Command::SetDuck { gain: 0.18, fade_ms: 0, ttl_ms: 10_000 });
    let sink = "alsa_output.usb-3142_fifine_Headset-00.analog-stereo";
    rt.send(Command::SetOutputDevice(Some(format!("  {sink} "))));
    wait("the switch", || !backend.devices().is_empty());
    assert_eq!(backend.devices(), [Some(sink.to_owned())]);
    wait("the prefs", || rt.state().prefs.output_device.as_deref() == Some(sink));
    // The engine restarted at once and is told the duck again, at once.
    wait("the duck again", || backend.ducks().len() >= 2);
    assert_eq!(backend.ducks()[1].0, 0.18);
    assert_eq!(backend.ducks()[1].1, Duration::ZERO);
    // The same device again: nothing; the default: switched.
    rt.send(Command::SetOutputDevice(Some(sink.to_owned())));
    rt.send(Command::SetOutputDevice(None));
    wait("the default", || backend.devices().len() == 2);
    assert_eq!(backend.devices()[1], None);
    assert_eq!(rt.state().prefs.output_device, None);
}
