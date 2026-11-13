//! A .torrent dropped into the Inbox is removed only once the torrent
//! client has it: one it refuses stays for another try.

use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_runtime::{ImportConfig, ImportSetup, NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, ViewState};

fn wait_for(rt: &Runtime, what: &str, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(60) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = rt.state();
    panic!("timed out waiting for {what}: toasts {:?}", s.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
}

#[test]
fn a_refused_inbox_torrent_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let bad = cfg.inbox_dir.join("broken.torrent");
    std::fs::write(&bad, b"d8:announce3:bad").unwrap();
    let rt = Runtime::start(
        RuntimeConfig {
            library: Library::open_in_memory().unwrap(),
            streams: Vec::new(),
            library_dir: cfg.library_dir.clone(),
            prefs: Prefs::default(),
            prefs_path: None,
            settings_path: None,
            import: Some(ImportSetup {
                config: cfg.clone(),
                metadata: None,
                watch_inbox: false,
                torrent_state_dir: root.join("torrent-state"),
                torrent_offline: true,
                models_dir: None,
            }),
        },
        Box::new(NullPlayback),
        Arc::new(|| {}),
    );
    rt.send(Command::AddTorrentFile(bad.clone()));
    wait_for(&rt, "the refusal", |s| s.toasts.iter().any(|t| t.text.starts_with("Не получилось добавить раздачу")));
    assert!(bad.exists(), "a torrent the client refused is not consumed");
}
