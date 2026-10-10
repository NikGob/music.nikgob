//! Inbox watcher: files dropped into `Inbox/` are imported once they stop
//! changing; `.torrent` files are reported to the runtime.
//!
//! Events are debounced per path: a file is released after [`SETTLE`] without
//! activity and a stable size that can be opened for reading, so half-copied
//! files are not touched.

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use nikgob_core::model::SourceOrigin;
use notify::{EventKind, RecursiveMode, Watcher};

use crate::error::{ImportError, Result};
use crate::events::ImportEvent;
use crate::fsutil;
use crate::importer::Importer;
use crate::sanitize::long_path;

/// Quiet time before a new file counts as complete.
pub const SETTLE: Duration = Duration::from_millis(1500);
const TICK: Duration = Duration::from_millis(250);
/// A file that still cannot be opened after this long is dropped from the queue.
const GIVE_UP: Duration = Duration::from_secs(300);

/// Stops watching when dropped.
pub struct InboxWatcher {
    _watcher: notify::RecommendedWatcher,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for InboxWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Waiting {
    since: Instant,
    last_event: Instant,
    last_size: Option<u64>,
}

impl Importer {
    /// Watches `Inbox/` (created if missing). Files already there are picked up
    /// too. Everything is reported on [`Importer::subscribe`]. Must be called
    /// inside a tokio runtime.
    pub fn watch_inbox(&self) -> Result<InboxWatcher> {
        let inbox = self.config().inbox_dir.clone();
        fsutil::create_dir_all(&inbox)?;
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| ImportError::Other("watch_inbox needs a tokio runtime".into()))?;
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .map_err(|e| ImportError::Other(format!("cannot watch the Inbox: {e}")))?;
        watcher
            .watch(&inbox, RecursiveMode::Recursive)
            .map_err(|e| ImportError::Other(format!("cannot watch the Inbox: {e}")))?;

        let stop = Arc::new(AtomicBool::new(false));
        let importer = self.clone();
        let stop2 = stop.clone();
        let thread = std::thread::Builder::new()
            .name("inbox-watcher".into())
            .spawn(move || watch_loop(importer, handle, rx, stop2))
            .map_err(|e| ImportError::Other(e.to_string()))?;
        Ok(InboxWatcher {
            _watcher: watcher,
            stop,
            thread: Some(thread),
        })
    }
}

fn hidden(path: &std::path::Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

fn watch_loop(
    importer: Importer,
    runtime: tokio::runtime::Handle,
    rx: mpsc::Receiver<notify::Result<notify::Event>>,
    stop: Arc<AtomicBool>,
) {
    let inbox = importer.config().inbox_dir.clone();
    let mut waiting: HashMap<PathBuf, Waiting> = HashMap::new();
    let track = |p: PathBuf, waiting: &mut HashMap<PathBuf, Waiting>| {
        let now = Instant::now();
        waiting
            .entry(p)
            .and_modify(|w| w.last_event = now)
            .or_insert(Waiting {
                since: now,
                last_event: now,
                last_size: None,
            });
    };
    // Files that were there before we started.
    let (audio, torrents) = fsutil::scan(std::slice::from_ref(&inbox));
    for p in audio.into_iter().chain(torrents) {
        track(p, &mut waiting);
    }

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(TICK) {
            Ok(Ok(ev)) => {
                if matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any) {
                    for p in ev.paths {
                        if hidden(&p) {
                            continue;
                        }
                        if p.is_dir() {
                            let (audio, torrents) = fsutil::scan(std::slice::from_ref(&p));
                            for f in audio.into_iter().chain(torrents) {
                                track(f, &mut waiting);
                            }
                        } else if fsutil::is_audio_file(&p) || fsutil::is_torrent_file(&p) {
                            track(p, &mut waiting);
                        }
                    }
                }
            }
            Ok(Err(e)) => log::warn!("inbox watcher: {e}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let now = Instant::now();
        let mut ready: Vec<PathBuf> = Vec::new();
        let mut gone: Vec<PathBuf> = Vec::new();
        for (path, w) in waiting.iter_mut() {
            if now.duration_since(w.last_event) < SETTLE {
                continue;
            }
            let size = std::fs::metadata(long_path(path)).map(|m| m.len()).ok();
            match size {
                None => gone.push(path.clone()),
                Some(s) if w.last_size == Some(s) && File::open(long_path(path)).is_ok() => {
                    ready.push(path.clone());
                }
                Some(s) => {
                    w.last_size = Some(s);
                    w.last_event = now;
                    if now.duration_since(w.since) > GIVE_UP {
                        gone.push(path.clone());
                    }
                }
            }
        }
        for p in gone {
            waiting.remove(&p);
        }
        if ready.is_empty() {
            continue;
        }
        let mut batch = Vec::new();
        for p in ready {
            waiting.remove(&p);
            if fsutil::is_torrent_file(&p) {
                let _ = importer
                    .inner
                    .events
                    .send(ImportEvent::TorrentFound(p));
            } else {
                batch.push(p);
            }
        }
        if !batch.is_empty() {
            batch.sort();
            let _enter = runtime.enter();
            let _job = importer.import_paths(batch, SourceOrigin::Inbox);
        }
    }
}
