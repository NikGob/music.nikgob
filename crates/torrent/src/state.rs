//! Small JSON sidecar in `state_dir`: completion times and lifetime upload
//! counters, which librqbit does not persist. Needed for `seed_days`,
//! `seed_ratio` and the total-GB cap to survive restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "nikgob-torrent.json";
/// Upload counters are written at most this often while seeding; a
/// completion, a removal or the shutdown is written at once.
const SAVE_EVERY: Duration = Duration::from_secs(30);

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Record {
    /// Unix seconds when the torrent was first seen.
    pub added_at: u64,
    /// Unix seconds when the selected files were first seen complete.
    pub completed_at: Option<u64>,
    /// Bytes uploaded over all runs.
    pub uploaded: u64,
    /// Last live counter value seen in this run (not persisted).
    #[serde(skip)]
    pub last_live: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct StateFile {
    /// Unix seconds when this state file was created (first session start).
    pub started_at: u64,
    /// Unix seconds of the most recent session start.
    pub last_start: u64,
    /// Keyed by lowercase hex info hash.
    pub torrents: HashMap<String, Record>,
    #[serde(skip)]
    pub dirty: bool,
    /// A change that must not wait for [`SAVE_EVERY`].
    #[serde(skip)]
    pub urgent: bool,
    #[serde(skip)]
    saved_at: Option<Instant>,
}

impl StateFile {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join(FILE_NAME)
    }

    /// Loads the file; a missing or corrupt file yields a fresh state.
    pub fn load(dir: &Path, now: u64) -> Self {
        let mut s: Self = std::fs::read(Self::path(dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if s.started_at == 0 {
            s.started_at = now;
        }
        s.last_start = now;
        s.dirty = true;
        s
    }

    /// Atomic write (temp file + rename). Clears `dirty` on success.
    pub fn save(&mut self, dir: &Path) -> std::io::Result<()> {
        let path = Self::path(dir);
        let tmp = path.with_extension("json.tmp");
        std::fs::create_dir_all(dir)?;
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        self.dirty = false;
        self.urgent = false;
        self.saved_at = Some(Instant::now());
        Ok(())
    }

    /// Whether a periodic look should write the file now: something changed
    /// and it is urgent, or the last write is [`SAVE_EVERY`] old.
    pub fn due(&self, now: Instant) -> bool {
        self.dirty && (self.urgent || self.saved_at.is_none_or(|t| now.duration_since(t) >= SAVE_EVERY))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_wait_and_urgent_changes_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = StateFile::load(dir.path(), 1);
        let now = Instant::now();
        assert!(s.due(now), "a fresh state is written at once");
        s.save(dir.path()).unwrap();
        assert!(!s.due(now));
        // An upload counter moved: it waits.
        s.dirty = true;
        assert!(!s.due(Instant::now()));
        assert!(s.due(Instant::now() + SAVE_EVERY));
        // A completion does not.
        s.urgent = true;
        assert!(s.due(Instant::now()));
        s.save(dir.path()).unwrap();
        assert!(!s.due(Instant::now() + SAVE_EVERY), "nothing changed");
    }

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = StateFile::load(dir.path(), 100);
        s.torrents.insert(
            "ab".into(),
            Record {
                added_at: 1,
                completed_at: Some(5),
                uploaded: 42,
                last_live: 9,
            },
        );
        s.save(dir.path()).unwrap();
        let back = StateFile::load(dir.path(), 200);
        assert_eq!(back.started_at, 100);
        assert_eq!(back.last_start, 200);
        let r = &back.torrents["ab"];
        assert_eq!((r.completed_at, r.uploaded, r.last_live), (Some(5), 42, 0));
    }

    #[test]
    fn corrupt_file_resets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(StateFile::path(dir.path()), b"not json").unwrap();
        let s = StateFile::load(dir.path(), 7);
        assert!(s.torrents.is_empty());
        assert_eq!(s.started_at, 7);
    }
}
