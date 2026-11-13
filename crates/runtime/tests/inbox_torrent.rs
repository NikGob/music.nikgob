//! A .torrent dropped into the Inbox is staged under a hidden name and
//! removed only once the torrent client has it: one it refuses goes back,
//! a new file put under the same name meanwhile is never touched, and when
//! the client does not start the file is tried again.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_library::Library;
use nikgob_runtime::{ImportConfig, ImportSetup, NullPlayback, Runtime, RuntimeConfig};
use nikgob_view::{Command, Prefs, ViewState};

fn wait_for(rt: &Runtime, what: &str, secs: u64, f: impl Fn(&ViewState) -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f(&rt.state()) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let toasts: Vec<String> = rt.state().toasts.iter().map(|t| t.text.clone()).collect();
    panic!("timed out waiting for {what}: toasts {toasts:?}");
}

fn wait_until(what: &str, secs: u64, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(secs), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The hidden staging files in `inbox`.
fn staged(inbox: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(inbox)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "adding"))
        .collect()
}

fn start(root: &Path, cfg: &ImportConfig) -> Runtime {
    Runtime::start(
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
    )
}

/// A real .torrent of a small folder.
fn torrent(root: &Path, name: &str) -> Vec<u8> {
    let src = root.join(format!("src-{name}"));
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.bin"), name.repeat(5000)).unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let t = librqbit::create_torrent(
            &src,
            librqbit::CreateTorrentOptions::default(),
            &librqbit::spawn_utils::BlockingSpawner::new(1),
        )
        .await
        .unwrap();
        t.as_bytes().unwrap().to_vec()
    })
}

#[test]
fn a_refused_inbox_torrent_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let bad = cfg.inbox_dir.join("broken.torrent");
    std::fs::write(&bad, b"d8:announce3:bad").unwrap();
    let rt = start(root, &cfg);
    rt.send(Command::AddTorrentFile(bad.clone()));
    wait_for(&rt, "the refusal", 60, |s| s.toasts.iter().any(|t| t.text.starts_with("Не получилось добавить раздачу")));
    assert!(bad.exists(), "a torrent the client refused is given back under its name");
    assert!(staged(&cfg.inbox_dir).is_empty());
    // Seen again (the watcher sees it come back): not tried again.
    rt.send(Command::AddTorrentFile(bad.clone()));
    std::thread::sleep(Duration::from_millis(500));
    assert!(bad.exists() && staged(&cfg.inbox_dir).is_empty());
}

#[test]
fn a_new_file_under_the_same_name_is_not_the_one_consumed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    let path = cfg.inbox_dir.join("album.torrent");
    std::fs::write(&path, torrent(root, "a")).unwrap();
    let rt = start(root, &cfg);
    rt.send(Command::AddTorrentFile(path.clone()));
    // Taken out of its name at once; a new file comes under it.
    wait_until("the staging", 30, || !path.exists());
    let newer = torrent(root, "b");
    std::fs::write(&path, &newer).unwrap();
    wait_until("the first one consumed", 60, || staged(&cfg.inbox_dir).is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), newer, "the new file was touched");
    drop(rt);
}

#[test]
fn an_inbox_torrent_is_tried_again_when_the_client_did_not_start() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cfg = ImportConfig::with_root(root);
    std::fs::create_dir_all(&cfg.inbox_dir).unwrap();
    // The client's state folder is a file: it cannot start.
    let blocker = root.join("torrent-state");
    std::fs::write(&blocker, b"not a folder").unwrap();
    let path = cfg.inbox_dir.join("album.torrent");
    std::fs::write(&path, torrent(root, "c")).unwrap();
    let rt = start(root, &cfg);
    rt.send(Command::AddTorrentFile(path.clone()));
    wait_for(&rt, "the failed start", 60, |s| s.toasts.iter().any(|t| t.text == "Не запустился торрент-клиент"));
    assert_eq!(staged(&cfg.inbox_dir).len(), 1, "kept staged for another try");
    // The client can start now: the retry takes it.
    std::fs::remove_file(&blocker).unwrap();
    wait_until("the retry", 60, || staged(&cfg.inbox_dir).is_empty());
    assert!(!path.exists(), "consumed by the retry");
    drop(rt);
}
