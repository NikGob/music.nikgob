//! The day's copy of the library's database next to the lossless masters
//! (`<masters>/.nikgob-backup/library-YYYY-MM-DD.sqlite`, the last seven
//! kept), so likes, playlists, history and recommendations outlive the
//! system disk; and, once per start (and per masters folder), the
//! temporary files a crash left in the masters folder go
//! ([`nikgob_import::master_files`]).
//!
//! On a thread of its own, never on the loop: looked at now and then, one
//! copy a day at most. A masters folder that is not there (a disk
//! unplugged) gets nothing and is looked at again later; a copy that fails
//! (the library kept changing under it, a disk error, something else at the
//! day's name) is not tried again that day.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nikgob_import::master_files::{self, DbCopy};

use crate::{Later, Worker};

/// How often the copy is looked at.
const EVERY: Duration = Duration::from_secs(60 * 60);
/// The first look, a while after the start (the start has its own work).
const FIRST_AFTER: Duration = Duration::from_secs(2 * 60);

/// The worker's side of the copy.
pub(crate) struct DbCopies {
    started: Instant,
    /// A look is on its way.
    busy: bool,
    /// When the last look started.
    last: Option<Instant>,
    /// The masters folder the looks are of: another one starts afresh.
    root: Option<PathBuf>,
    /// The day that has its copy (or whose copy failed: not tried again).
    done: Option<String>,
    /// The masters folder's temporary files were looked at this start.
    cleaned: bool,
}

impl Default for DbCopies {
    fn default() -> Self {
        Self { started: Instant::now(), busy: false, last: None, root: None, done: None, cleaned: false }
    }
}

impl DbCopies {
    /// The masters folder is `root` now: a different one has neither its
    /// copy of the day nor its look at temporaries yet.
    fn folder(&mut self, root: &Path) {
        if self.root.as_deref() != Some(root) {
            self.root = Some(root.to_path_buf());
            self.done = None;
            self.cleaned = false;
            self.last = None;
        }
    }

    /// Whether a look is due at `now` on the day `today`.
    fn due(&self, now: Instant, today: &str) -> bool {
        if self.busy || now.saturating_duration_since(self.started) < FIRST_AFTER {
            return false;
        }
        let stale = self.last.is_none_or(|at| now.saturating_duration_since(at) >= EVERY);
        stale && (self.done.as_deref() != Some(today) || !self.cleaned)
    }
}

impl Worker {
    /// Once a second: starts a look at the copy when one is due.
    pub(crate) fn db_copy_poll(&mut self) {
        let Some(root) = self.import.as_ref().map(|s| s.config().masters_dir()) else { return };
        self.db_copies.folder(&root);
        let now = Instant::now();
        let today = master_files::today();
        if !self.db_copies.due(now, &today) {
            return;
        }
        let lib = self.lib.clone();
        let later = self.later_tx.clone();
        let clean = !self.db_copies.cleaned;
        let copy = self.db_copies.done.as_deref() != Some(today.as_str());
        self.db_copies.busy = true;
        self.db_copies.last = Some(now);
        let spawned = std::thread::Builder::new().name("nikgob-db-copy".into()).spawn(move || {
            let _low = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
            let there = root.is_dir();
            // Only a gone player's temporaries go; ones still in use (another
            // player running) are looked at again an hour later.
            let mut left_live = 0;
            if clean && there {
                let c = master_files::clean_stale_temps(&root);
                if c.removed > 0 {
                    log::info!("masters: {} temporary files a crash left are gone", c.removed);
                }
                left_live = c.live;
            }
            // Done for the day when it is made or was there, and when it
            // failed (tried again tomorrow); not when the folder is not there.
            let done = copy
                && match master_files::copy_database(&lib, &root, &today, master_files::BACKUP_KEEP) {
                    Ok(DbCopy::Made(to)) => {
                        log::info!("database copy: {}", to.display());
                        true
                    }
                    Ok(DbCopy::Had) => true,
                    Ok(DbCopy::NoFolder) => false,
                    Err(e) => {
                        log::warn!("database copy: none today: {e}");
                        true
                    }
                };
            let cleaned = clean && there && left_live == 0;
            let _ = later.send(Later::new("the database copy", move |w| w.db_copied(&root, done.then_some(today), cleaned)));
        });
        if let Err(e) = spawned {
            log::warn!("database copy: the look did not start: {e}");
            self.db_copies.busy = false;
        }
    }

    fn db_copied(&mut self, root: &Path, day: Option<String>, cleaned: bool) {
        self.db_copies.busy = false;
        // The masters folder changed meanwhile: this look was of the old one.
        if self.db_copies.root.as_deref() != Some(root) {
            return;
        }
        self.db_copies.cleaned |= cleaned;
        if day.is_some() {
            self.db_copies.done = day;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_is_due_once_an_hour_until_the_day_has_its_copy() {
        let mut c = DbCopies::default();
        c.folder(Path::new("m"));
        let start = c.started;
        assert!(!c.due(start, "2026-11-14"), "not right at the start");
        let later = start + FIRST_AFTER;
        assert!(c.due(later, "2026-11-14"));
        c.last = Some(later);
        assert!(!c.due(later + Duration::from_secs(60), "2026-11-14"), "not again within the hour");
        assert!(c.due(later + EVERY, "2026-11-14"), "the folder was not there: an hour later");
        c.done = Some("2026-11-14".into());
        c.cleaned = true;
        assert!(!c.due(later + EVERY, "2026-11-14"), "the day has its copy");
        assert!(c.due(later + EVERY, "2026-11-15"), "the next day");
        c.busy = true;
        assert!(!c.due(later + EVERY, "2026-11-15"), "one look at a time");
        c.busy = false;

        // Another masters folder: its own copy and its own look at once.
        c.folder(Path::new("m"));
        assert!(!c.due(later + Duration::from_secs(60), "2026-11-14"), "the same folder: as it was");
        c.folder(Path::new("n"));
        assert!(c.due(later + Duration::from_secs(60), "2026-11-14"));
        assert!(!c.cleaned && c.done.is_none());
    }
}
