//! An output device that does not open is said: silence with no word is
//! not what the listener gets.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, PlaybackEvent, Runtime, RuntimeConfig};
use nikgob_view::Prefs;

#[test]
fn a_device_that_does_not_open_is_said() {
    let rt = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            streams: Vec::new(),
            library_dir: std::env::temp_dir(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.playback_sender().send(PlaybackEvent::DeviceError { message: "no device".into() }).unwrap();
    let t = Instant::now();
    while !rt.state().toasts.iter().any(|t| t.text == "Не удалось открыть устройство вывода звука") {
        assert!(t.elapsed() < Duration::from_secs(30), "no toast");
        std::thread::sleep(Duration::from_millis(20));
    }
}
