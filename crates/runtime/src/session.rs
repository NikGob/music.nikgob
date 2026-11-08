//! What survives a restart besides the prefs: the volume and the queue,
//! with the place in the current track. It comes back paused; play picks
//! up where the last run left off.
//!
//! Written every few seconds, so a crash or a PC switched off at the wall
//! loses at most those seconds: the new file reaches the disk before it
//! replaces the old one, and the one before stays as `session.json.bak`
//! for a file a power cut still broke.

use std::path::Path;
use std::time::Duration;

use nikgob_view::Status;
use serde::{Deserialize, Serialize};

use crate::Worker;
use crate::queue::Snapshot;

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Session {
    /// None in a file from before it was saved: the default volume.
    pub volume: Option<f32>,
    pub muted: bool,
    pub queue: Snapshot,
    /// Where in the current track, in milliseconds.
    pub position_ms: u64,
    /// The radio the queue plays, if any (it goes on after a restart).
    pub radio: Option<crate::recs::RadioSave>,
    /// What the queue was started from when that has a page
    /// (`release:<id>`, `playlist:<id>`, `artist:<id>`): the header of Now
    /// Playing links it after a restart too.
    pub page: Option<String>,
}

impl Session {
    pub fn load(path: &Path) -> Self {
        let read = |p: &Path| std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok());
        read(path)
            .or_else(|| {
                let saved = read(&backup(path));
                if saved.is_some() {
                    log::warn!("{} is unreadable; the queue comes from the backup", path.display());
                }
                saved
            })
            .unwrap_or_default()
    }

    /// Puts `json` at `path` so that a crash at any moment leaves a whole
    /// file there or at its backup.
    fn write(path: &Path, json: &str) -> std::io::Result<()> {
        use std::io::Write;
        let tmp = path.with_extension("json.tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
        drop(f);
        if path.exists() {
            std::fs::rename(path, backup(path))?;
        }
        std::fs::rename(&tmp, path)
    }
}

fn backup(path: &Path) -> std::path::PathBuf {
    path.with_extension("json.bak")
}

impl Worker {
    /// Brings the last run's queue back, paused on its track.
    pub(crate) fn restore_session(&mut self) {
        let Some(path) = self.session_path.clone() else { return };
        let saved = Session::load(&path);
        if saved.queue.items.is_empty() {
            // Shuffle and repeat are the player's: kept without a queue too.
            self.queue.restore_modes(&saved.queue);
            self.publish_player();
            return;
        }
        self.queue.restore(&saved.queue);
        self.restored_page = saved.page.map(|p| (self.queue.generation(), p));
        if let Some(radio) = saved.radio {
            self.radio_restore(radio);
        }
        if self.queue.current().is_some() {
            let at = Duration::from_millis(saved.position_ms);
            self.resume_at = Some(at);
            self.position = at;
            self.status = Status::Paused;
        }
        self.publish_now_playing();
    }

    /// Starts the restored track where it was left.
    pub(crate) fn start_restored(&mut self) {
        let at = self.resume_at.take().unwrap_or_default();
        if let Some(e) = self.queue.current() {
            self.start(e);
            if !at.is_zero() {
                self.seek(at);
            }
        }
    }

    /// Writes `session.json` when something in it changed.
    pub(crate) fn save_session(&mut self) {
        let Some(path) = self.session_path.clone() else { return };
        let radio = self.radio_save();
        let page = self.context_page_id().filter(|id| crate::context_page(id).is_some()).map(str::to_owned);
        let session = Session {
            radio,
            page,
            volume: Some(self.volume),
            muted: self.muted,
            queue: self.queue.snapshot(),
            position_ms: if self.queue.current().is_some() { self.position_now().as_millis() as u64 } else { 0 },
        };
        let Ok(json) = serde_json::to_string_pretty(&session) else { return };
        if json == self.session_saved {
            return;
        }
        match Session::write(&path, &json) {
            Ok(()) => self.session_saved = json,
            Err(e) => log::warn!("cannot save {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(volume: f32) -> Session {
        Session { volume: Some(volume), ..Session::default() }
    }

    #[test]
    fn a_broken_file_falls_back_to_the_one_before() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        assert_eq!(Session::load(&path), Session::default());
        for v in [0.25, 0.5] {
            Session::write(&path, &serde_json::to_string(&session(v)).unwrap()).unwrap();
        }
        assert_eq!(Session::load(&path), session(0.5));
        assert_eq!(Session::load(&backup(&path)), session(0.25));
        // What a power cut can leave: the name in place, the bytes not.
        std::fs::write(&path, [0u8; 64]).unwrap();
        assert_eq!(Session::load(&path), session(0.25));
        std::fs::remove_file(&path).unwrap();
        assert_eq!(Session::load(&path), session(0.25));
    }
}
