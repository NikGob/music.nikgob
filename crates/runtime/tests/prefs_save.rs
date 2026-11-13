//! Settings that cannot be written are said, not lost without a word.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_runtime::{NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, ViewState};

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(30) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let s = rt.state();
    panic!("timed out waiting for {what}: toasts {:?}", s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
}

#[test]
fn a_failed_prefs_write_is_said() {
    let dir = tempfile::tempdir().unwrap();
    // A folder that is not there: the write fails.
    let path = dir.path().join("gone").join("prefs.json");
    let rt = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            streams: Vec::new(),
            library_dir: dir.path().to_owned(),
            prefs: Prefs::default(),
            prefs_path: Some(path.clone()),
            settings_path: None,
            import: None,
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    let mut p = rt.state().prefs.clone();
    p.normalization = !p.normalization;
    rt.send(Command::SetPrefs(Box::new(p)));
    wait_for(&rt, "the error", |s| s.toasts.iter().any(|t| t.text == "Не удалось сохранить настройки"));
    assert!(!path.exists());
}
