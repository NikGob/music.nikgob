//! The day's copy of the library's database next to the lossless masters
//! (`<masters>/.nikgob-backup/library-YYYY-MM-DD.sqlite`, the last seven
//! kept), so likes, playlists, history and recommendations outlive the
//! system disk; and, once per start, the temporary files a crash left in
//! the masters folder go ([`nikgob_import::master_files`]).
//!
//! On a thread of its own, never on the loop: looked at now and then, one
//! copy a day at most. A masters folder that is not there (a disk
//! unplugged) gets nothing and is looked at again later.

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
    /// The day that has its copy.
    done: Option<String>,
    /// The masters folder's temporary files were looked at this start.
    cleaned: bool,
}

impl Default for DbCopies {
    fn default() -> Self {
        Self { started: Instant::now(), busy: false, last: None, done: None, cleaned: false }
    }
}

impl DbCopies {
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
        let now = Instant::now();
        let today = master_files::today();
        if !self.db_copies.due(now, &today) {
            return;
        }
        let Some(root) = self.import.as_ref().map(|s| s.config().masters_dir()) else { return };
        let lib = self.lib.clone();
        let later = self.later_tx.clone();
        let clean = !self.db_copies.cleaned;
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
            let made = match master_files::copy_database(&lib, &root, &today, master_files::BACKUP_KEEP) {
                Ok(DbCopy::Made(to)) => {
                    log::info!("database copy: {}", to.display());
                    true
                }
                Ok(DbCopy::Had) => true,
                Ok(DbCopy::NoFolder) => false,
                Err(e) => {
                    log::warn!("database copy: {e}");
                    false
                }
            };
            let cleaned = clean && there && left_live == 0;
            let _ = later.send(Later::new("the database copy", move |w| w.db_copied(made.then_some(today), cleaned)));
        });
        if let Err(e) = spawned {
            log::warn!("database copy: the look did not start: {e}");
            self.db_copies.busy = false;
        }
    }

    fn db_copied(&mut self, day: Option<String>, cleaned: bool) {
        self.db_copies.busy = false;
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
    }
}
