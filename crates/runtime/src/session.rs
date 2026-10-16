//! What survives a restart besides the prefs: the volume and the queue,
//! with the place in the current track. It comes back paused; play picks
//! up where the last run left off.

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
}

impl Session {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
}

impl Worker {
    /// Brings the last run's queue back, paused on its track.
    pub(crate) fn restore_session(&mut self) {
        let Some(path) = self.session_path.clone() else { return };
        let saved = Session::load(&path);
        if saved.queue.items.is_empty() {
            self.publish_player();
            return;
        }
        self.queue.restore(&saved.queue);
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
        let session = Session {
            volume: Some(self.volume),
            muted: self.muted,
            queue: self.queue.snapshot(),
            position_ms: if self.queue.current().is_some() { self.position_now().as_millis() as u64 } else { 0 },
        };
        let Ok(json) = serde_json::to_string_pretty(&session) else { return };
        if json == self.session_saved {
            return;
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
            self.session_saved = json;
        }
    }
}
