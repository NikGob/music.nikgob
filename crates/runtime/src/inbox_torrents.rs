//! .torrent files dropped into the Inbox.
//!
//! * **Owned before it is read.** The file is renamed to a hidden staging
//!   name next to it (`.name.torrent.<pid>-<n>.adding`, which the Inbox
//!   watcher ignores) and the bytes sent to the client are read from there.
//!   Only that staging file is removed once the client took the torrent: a
//!   new file put under the old name meanwhile is never touched.
//! * **Refused: given back.** A torrent the client refuses (a broken file)
//!   goes back under its own name (or a free one next to it) and is not
//!   tried again this run, though the watcher sees it come back.
//! * **Client not started: tried again.** When the torrent client does not
//!   start, the staged files are tried again after [`RETRY_STEP`], twice as
//!   long the second time; after [`MAX_ATTEMPTS`] they go back under their
//!   names and wait for the next run.
//! * **Nothing stays hidden.** At the end of a run and at the start of the
//!   next (after a crash), staged files go back under their names.

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use nikgob_torrent::TorrentPayload;

use crate::{Later, Worker};

/// Wait before trying a staged torrent again after the client did not
/// start (times the attempts so far).
pub(crate) const RETRY_STEP: Duration = Duration::from_secs(10);
/// Attempts before a staged torrent goes back to wait for the next run.
pub(crate) const MAX_ATTEMPTS: u32 = 3;
/// Contents not tried again this run, at most.
const SKIP_MAX: usize = 256;
const STAGED_EXT: &str = "adding";

/// A torrent taken out of the Inbox and handed to the client.
struct Staged {
    staging: PathBuf,
    original: PathBuf,
    key: u64,
    attempts: u32,
}

/// The Inbox torrents of this run.
#[derive(Default)]
pub(crate) struct InboxTorrents {
    staged: Vec<Staged>,
    /// Contents refused or given up on this run (see the module docs).
    skip: VecDeque<u64>,
}

impl InboxTorrents {
    fn skip(&mut self, key: u64) {
        if !self.skip.contains(&key) {
            self.skip.push_back(key);
            if self.skip.len() > SKIP_MAX {
                self.skip.pop_front();
            }
        }
    }
}

/// What a file's contents are, for [`InboxTorrents::skip`].
fn content_key(bytes: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// The hidden staging name of `original`.
pub(crate) fn staging_name(original: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = original.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    original.with_file_name(format!(".{name}.{}-{n}.{STAGED_EXT}", std::process::id()))
}

/// The name a staging file came from.
pub(crate) fn original_of(staging: &Path) -> Option<PathBuf> {
    let name = staging.file_name()?.to_str()?;
    let inner = name.strip_prefix('.')?.strip_suffix(&format!(".{STAGED_EXT}"))?;
    let (original, tag) = inner.rsplit_once('.')?;
    let (pid, n) = tag.split_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(pid) && digits(n) && !original.is_empty()).then(|| staging.with_file_name(original))
}

/// `wanted`, or `stem (2).ext`, `stem (3).ext`… when it is taken.
pub(crate) fn free_name(wanted: &Path) -> PathBuf {
    if !wanted.exists() {
        return wanted.to_owned();
    }
    let stem = wanted.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = wanted.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..)
        .map(|n| wanted.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| wanted.to_owned())
}

/// Puts a staging file back under its name (or a free one next to it).
fn give_back(staging: &Path, original: &Path) {
    let to = free_name(original);
    match std::fs::rename(staging, &to) {
        Ok(()) => log::info!("{}: back in the Inbox", to.display()),
        Err(e) => log::warn!("{}: not given back as {}: {e}", staging.display(), to.display()),
    }
}

/// Gives back the staging files a run left in `inbox` (it ended without
/// its cleanup).
pub(crate) fn give_back_leftovers(inbox: &Path) {
    for e in std::fs::read_dir(inbox).into_iter().flatten().flatten() {
        let path = e.path();
        if let Some(original) = original_of(&path) {
            give_back(&path, &original);
        }
    }
}

impl Worker {
    /// A .torrent in the Inbox: staged, then handed to the client.
    pub(crate) fn add_inbox_torrent(&mut self, path: PathBuf) {
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("{}: {e}", path.display());
                self.toast_err("Не получилось прочитать .torrent", None);
                return;
            }
        };
        if self.torrents.inbox.skip.contains(&content_key(&bytes)) {
            log::info!("{}: refused before, not tried again this run", path.display());
            return;
        }
        let staging = staging_name(&path);
        if let Err(e) = std::fs::rename(&path, &staging) {
            // Cannot be owned (held by another program): added as read,
            // the file stays.
            log::warn!("{}: not staged ({e}), added and left in place", path.display());
            self.add_payload_inner(TorrentPayload::TorrentFile(bytes), None, None, None, None);
            return;
        }
        // What was staged is what is sent, whatever was read before.
        let bytes = match std::fs::read(&staging) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("{}: {e}", staging.display());
                give_back(&staging, &path);
                self.toast_err("Не получилось прочитать .torrent", None);
                return;
            }
        };
        self.torrents.inbox.staged.push(Staged {
            staging: staging.clone(),
            original: path,
            key: content_key(&bytes),
            attempts: 0,
        });
        self.add_payload_inner(TorrentPayload::TorrentFile(bytes), None, None, None, Some(staging));
    }

    /// The client answered for a staged torrent: removed when it took it,
    /// given back when it refused it.
    pub(crate) fn inbox_torrent_answered(&mut self, staging: &Path, taken: bool) {
        let inbox = &mut self.torrents.inbox;
        let Some(i) = inbox.staged.iter().position(|s| s.staging == staging) else { return };
        let s = inbox.staged.remove(i);
        if taken {
            if let Err(e) = std::fs::remove_file(&s.staging) {
                log::warn!("{}: not removed from the Inbox: {e}", s.staging.display());
            }
        } else {
            give_back(&s.staging, &s.original);
            inbox.skip(s.key);
        }
    }

    /// The client did not start: every staged torrent is tried again later
    /// (`transient`), or given back now.
    pub(crate) fn inbox_torrents_not_started(&mut self, transient: bool) {
        let staged = std::mem::take(&mut self.torrents.inbox.staged);
        for mut s in staged {
            s.attempts += 1;
            if transient && s.attempts < MAX_ATTEMPTS {
                let (later, staging, wait) = (self.later_tx.clone(), s.staging.clone(), RETRY_STEP * s.attempts);
                let spawned = std::thread::Builder::new().name("nikgob-inbox-retry".into()).spawn(move || {
                    std::thread::sleep(wait);
                    let _ = later.send(Later::new("an Inbox torrent again", move |w| w.retry_inbox_torrent(&staging)));
                });
                if spawned.is_ok() {
                    self.torrents.inbox.staged.push(s);
                    continue;
                }
            }
            give_back(&s.staging, &s.original);
            self.torrents.inbox.skip(s.key);
        }
    }

    fn retry_inbox_torrent(&mut self, staging: &Path) {
        if !self.torrents.inbox.staged.iter().any(|s| s.staging == staging) {
            return;
        }
        match std::fs::read(staging) {
            Ok(bytes) => {
                log::info!("{}: tried again", staging.display());
                self.add_payload_inner(TorrentPayload::TorrentFile(bytes), None, None, None, Some(staging.to_owned()));
            }
            Err(e) => {
                log::warn!("{}: gone ({e})", staging.display());
                self.torrents.inbox.staged.retain(|s| s.staging != staging);
            }
        }
    }

    /// At the end of a run: nothing stays staged.
    pub(crate) fn give_back_inbox_torrents(&mut self) {
        for s in std::mem::take(&mut self.torrents.inbox.staged) {
            give_back(&s.staging, &s.original);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_names_come_back_to_their_originals() {
        let o = Path::new("/inbox/My album [FLAC].torrent");
        let s = staging_name(o);
        let name = s.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with(".My album [FLAC].torrent.") && name.ends_with(".adding"), "{name}");
        assert_eq!(original_of(&s).as_deref(), Some(o));
        assert_eq!(original_of(Path::new("/inbox/.hidden.torrent")), None);
        assert_eq!(original_of(Path::new("/inbox/x.torrent.adding")), None);
        assert_eq!(original_of(Path::new("/inbox/.x.torrent.12-a.adding")), None);
    }

    #[test]
    fn a_taken_name_gets_a_free_one() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.torrent");
        assert_eq!(free_name(&a), a);
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(free_name(&a), dir.path().join("a (2).torrent"));
    }

    #[test]
    fn leftovers_of_a_cut_run_go_back() {
        let dir = tempfile::tempdir().unwrap();
        let original = dir.path().join("b.torrent");
        let staging = staging_name(&original);
        std::fs::write(&staging, b"d4:infoe").unwrap();
        give_back_leftovers(dir.path());
        assert!(original.exists() && !staging.exists());
    }

    #[test]
    fn the_skip_list_stays_bounded() {
        let mut t = InboxTorrents::default();
        for k in 0..(SKIP_MAX as u64 + 10) {
            t.skip(k);
            t.skip(k);
        }
        assert_eq!(t.skip.len(), SKIP_MAX);
        assert!(!t.skip.contains(&0) && t.skip.contains(&(SKIP_MAX as u64 + 9)));
    }
}
