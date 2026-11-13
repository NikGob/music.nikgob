//! What survives a restart besides the prefs: the volume and the queue,
//! with the place in the current track. It comes back paused; play picks
//! up where the last run left off.
//!
//! Written every few seconds, so a crash or a PC switched off at the wall
//! loses at most those seconds: the new file reaches the disk before it
//! replaces the old one, and the one before stays as `session.json.bak`
//! for a file a power cut still broke.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
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
    /// What each entry of a playlist's queue is to «✦ С дополнением».
    #[serde(default)]
    pub plus: Option<crate::recs::PlusSave>,
    /// The places (play order) and tracks of the entries «Автовоспроизведение»
    /// added to continue an album's world (its radio's are the radio's).
    #[serde(default)]
    pub autoplay_at: Vec<(usize, nikgob_core::model::TrackId)>,
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

/// Writes `session.json` on a thread of its own, so the disk (a sync every
/// few seconds) never holds the player's thread. One session waits at most:
/// a newer one replaces it, so a stalled disk never piles snapshots up.
/// [`Self::flush`] (and the drop) waits until the last one is written and
/// tries it once more on the caller's thread if that write failed.
#[derive(Default)]
pub(crate) struct Saver {
    /// The last session handed over, and where.
    last: Option<(std::path::PathBuf, Arc<Session>)>,
    shared: Arc<SaverShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Default)]
struct SaverShared {
    slot: Mutex<Slot>,
    changed: Condvar,
}

#[derive(Default)]
struct Slot {
    /// The one session waiting to be written.
    next: Option<(std::path::PathBuf, Arc<Session>)>,
    stop: bool,
    /// The last write failed.
    failed: bool,
}

impl SaverShared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Serializes and writes `session`; whether it is on disk.
fn write_session(path: &Path, session: &Session) -> bool {
    let written = serde_json::to_string_pretty(session)
        .map_err(std::io::Error::other)
        .and_then(|json| Session::write(path, &json));
    match written {
        Ok(()) => true,
        Err(e) => {
            log::warn!("cannot save {}: {e}", path.display());
            false
        }
    }
}

impl Saver {
    /// Hands `session` over to be written at `path`, unless it is what was
    /// handed over last and written.
    pub(crate) fn save(&mut self, path: &Path, session: Session) {
        let failed = std::mem::take(&mut self.shared.lock().failed);
        if !failed && self.last.as_ref().is_some_and(|(p, s)| p == path && **s == session) {
            return;
        }
        let session = Arc::new(session);
        self.last = Some((path.to_owned(), Arc::clone(&session)));
        if self.thread.is_none() {
            let shared = Arc::clone(&self.shared);
            match std::thread::Builder::new().name("nikgob-session".into()).spawn(move || writer(&shared)) {
                Ok(t) => self.thread = Some(t),
                Err(e) => {
                    // No thread: written here, as before.
                    log::warn!("no session writer thread: {e}");
                    self.shared.lock().failed = !write_session(path, &session);
                    return;
                }
            }
        }
        self.shared.lock().next = Some((path.to_owned(), session));
        self.shared.changed.notify_all();
    }

    /// Waits until what was handed over is written; a last write that
    /// failed is tried once more here (the final save at the end of a run
    /// is not lost to a passing failure).
    pub(crate) fn flush(&mut self) {
        if let Some(t) = self.thread.take() {
            self.shared.lock().stop = true;
            self.shared.changed.notify_all();
            let _ = t.join();
        }
        let failed = std::mem::take(&mut self.shared.lock().failed);
        if failed && let Some((path, session)) = &self.last {
            self.shared.lock().failed = !write_session(path, session);
        }
        self.shared.lock().stop = false;
    }
}

/// The writing thread: the waiting session, one at a time, until stopped
/// with nothing left.
fn writer(shared: &SaverShared) {
    loop {
        let (path, session) = {
            let mut slot = shared.lock();
            loop {
                if let Some(next) = slot.next.take() {
                    break next;
                }
                if slot.stop {
                    return;
                }
                slot = shared.changed.wait(slot).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let ok = write_session(&path, &session);
        let mut slot = shared.lock();
        slot.failed = !ok;
        shared.changed.notify_all();
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        self.flush();
    }
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
        self.autoplay_restore(&saved.autoplay_at);
        if let Some(radio) = saved.radio {
            self.radio_restore(radio);
        }
        if let Some(plus) = saved.plus {
            self.plus_restore(plus);
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
        let plus = self.plus_save();
        let autoplay_at = self.autoplay_save();
        let page = self.context_page_id().filter(|id| crate::context_page(id).is_some()).map(str::to_owned);
        let session = Session {
            radio,
            plus,
            autoplay_at,
            page,
            volume: Some(self.volume),
            muted: self.muted,
            queue: self.queue.snapshot(),
            position_ms: if self.queue.current().is_some() { self.position_now().as_millis() as u64 } else { 0 },
        };
        // Serialized and written on the saver's thread.
        self.session_saved.save(&path, session);
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

    /// The session is written off the caller's thread; a failed write is
    /// tried again at the next save even when nothing changed.
    #[test]
    fn the_saver_writes_on_its_thread_and_retries_a_failure() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("not-yet");
        let path = missing.join("session.json");
        let mut saver = Saver::default();
        saver.save(&path, session(0.75));
        saver.flush();
        assert!(!path.exists());
        std::fs::create_dir_all(&missing).unwrap();
        // Unchanged, but the last write failed: written again.
        saver.save(&path, session(0.75));
        saver.flush();
        assert_eq!(Session::load(&path), session(0.75));
        // The newest of several is what stays.
        for v in [0.1, 0.2, 0.3] {
            saver.save(&path, session(v));
        }
        drop(saver);
        assert_eq!(Session::load(&path), session(0.3));
    }

    /// The last save of a run fails on its thread after the final save saw
    /// nothing new to hand over: the flush at the end writes it after all.
    #[test]
    fn the_final_session_is_not_lost_to_a_failed_write_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("later");
        let path = missing.join("session.json");
        let mut saver = Saver::default();
        saver.save(&path, session(0.5));
        let t = std::time::Instant::now();
        while !saver.shared.lock().failed {
            assert!(t.elapsed() < Duration::from_secs(10), "the write did not fail");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::fs::create_dir_all(&missing).unwrap();
        saver.flush();
        assert_eq!(Session::load(&path), session(0.5));
    }

    /// What autoplay added to continue an album's world is kept by its
    /// places; a file from before it was kept reads as none.
    #[test]
    fn autoplay_places_are_kept_and_old_files_read() {
        let track = nikgob_core::model::TrackId::new();
        let s = Session { autoplay_at: vec![(3, track)], ..session(0.5) };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Session>(&json).unwrap(), s);
        let old: Session = serde_json::from_str(r#"{"volume":0.5,"muted":false}"#).unwrap();
        assert!(old.autoplay_at.is_empty());
    }
}
