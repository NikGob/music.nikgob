//! A file's tags (its gain, channels and details) read off the loop when a
//! start needs them and they are not kept yet: «Next» never waits on a
//! slow disk. The start waits [`FACTS_WAIT`] at most; past that it plays
//! with a guessed gain ([`crate::convert::FactsCache::guess_gain`]) and
//! the engine glides to the real one when the read is done
//! ([`crate::Playback::set_gain`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::Receiver;
use nikgob_core::model::TrackId;

use crate::{Later, Worker};

/// How long a start waits for its file's tags before it plays with a
/// guessed gain: a read of a warm file is done well within it, so the
/// gain is then right from the first sample; a cold disk or a virus
/// scanner (seconds) is not waited for.
const FACTS_WAIT: Duration = Duration::from_millis(50);
/// Gains closer than this sound the same: a guess this close stays.
const SAME_GAIN_DB: f32 = 0.05;

/// The reads of files' tags on their own threads, one per file at a time.
#[derive(Default)]
pub(crate) struct FactsReads {
    reading: HashMap<PathBuf, Reading>,
}

struct Reading {
    /// The gain the read found, sent once.
    done: Receiver<f32>,
    /// The items that play the file with a guessed gain until the read is
    /// done: their id and the guess.
    guessed: Vec<(u64, f32)>,
}

impl FactsReads {
    /// Reads `path` with `read` on a thread of its own; `then` runs on
    /// that thread with the gain found. A read of `path` on its way is not
    /// started again. False when no thread could start.
    pub(crate) fn start(
        &mut self,
        path: &Path,
        read: impl FnOnce() -> f32 + Send + 'static,
        then: impl FnOnce(f32) + Send + 'static,
    ) -> bool {
        if self.reading.contains_key(path) {
            return true;
        }
        let (tx, done) = crossbeam_channel::bounded(1);
        let spawned = std::thread::Builder::new().name("nikgob-facts".into()).spawn(move || {
            let gain = read();
            let _ = tx.send(gain);
            then(gain);
        });
        match spawned {
            Ok(_) => {
                self.reading.insert(path.to_owned(), Reading { done, guessed: Vec::new() });
                true
            }
            Err(e) => {
                log::warn!("{}: its tags are read on the loop: {e}", path.display());
                false
            }
        }
    }

    /// The gain of `path` if its read is on its way and done within `wait`.
    pub(crate) fn wait(&self, path: &Path, wait: Duration) -> Option<f32> {
        self.reading.get(path)?.done.recv_timeout(wait).ok()
    }

    /// Item `id` plays `path` with `guess` until its read is done.
    pub(crate) fn guessed(&mut self, path: &Path, id: u64, guess: f32) {
        if let Some(r) = self.reading.get_mut(path) {
            r.guessed.push((id, guess));
        }
    }

    /// The read of `path` is done: the items that played it with a guess.
    pub(crate) fn done(&mut self, path: &Path) -> Vec<(u64, f32)> {
        self.reading.remove(path).map(|r| r.guessed).unwrap_or_default()
    }
}

impl Worker {
    /// The gain item `id` plays the file at `path` with: from its tags
    /// when they are kept or read within [`FACTS_WAIT`], else a guess it
    /// plays with until they are read off the loop (the engine then glides
    /// to the real one, see [`Self::facts_read`]).
    pub(crate) fn file_gain(&mut self, id: u64, path: &Path) -> f32 {
        if let Some(f) = self.res.facts().get(path) {
            return f.gain_db;
        }
        if !self.read_facts_off_loop(path) {
            return self.res.facts().of(path).gain_db;
        }
        if let Some(gain) = self.facts_reads.wait(path, FACTS_WAIT) {
            return gain;
        }
        let guess = self.res.facts().guess_gain(path);
        log::info!(
            "{}: its tags are slow to read; it starts at {guess:+.1} dB until they are",
            path.display()
        );
        self.facts_reads.guessed(path, id, guess);
        guess
    }

    /// Reads the tags of `path` on a thread of its own (not again while a
    /// read of it is on its way); [`Self::facts_read`] follows on the
    /// loop. False when no thread could start.
    pub(crate) fn read_facts_off_loop(&mut self, path: &Path) -> bool {
        let facts = self.res.facts().clone();
        let later = self.later_tx.clone();
        let (file, read) = (path.to_owned(), path.to_owned());
        self.facts_reads.start(
            path,
            move || facts.of(&read).gain_db,
            move |gain| {
                let _ = later.send(Later::new("a file's tags", move |w| w.facts_read(&file, gain)));
            },
        )
    }

    /// The tags of `path` were read (`gain` is its gain): what played it
    /// with a guess takes the real gain, the playing track's details come,
    /// and the next track is prepared if it waited for them.
    fn facts_read(&mut self, path: &Path, gain: f32) {
        for (id, guess) in self.facts_reads.done(path) {
            if (gain - guess).abs() > SAME_GAIN_DB {
                log::info!("{}: its gain is {gain:+.1} dB, not {guess:+.1}", path.display());
                self.playback.set_gain(id, gain);
            }
        }
        if let Some((_, track)) = self.details_waiting.take_if(|(p, _)| p.as_path() == path)
            && self.playing.is_some_and(|e| e.track == track)
        {
            // «Источник и качество» with the file's channels, then the
            // info panel's tags and the embedded lyrics.
            self.publish_now_playing();
            self.track_details(track);
        }
        if self.reading_ahead.as_deref() == Some(path) {
            self.reading_ahead = None;
            self.prepare_next(false);
        }
    }

    /// The details of `track` (the info panel's tags, the embedded lyrics)
    /// come when the tags of its file at `path` are read off the loop.
    /// False when no thread could start (they are read here then).
    pub(crate) fn details_after_read(&mut self, path: &Path, track: TrackId) -> bool {
        if !self.read_facts_off_loop(path) {
            return false;
        }
        self.details_waiting = Some((path.to_owned(), track));
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    use super::*;

    #[test]
    fn a_slow_read_does_not_hold_the_start_and_brings_its_gain_later() {
        let mut reads = FactsReads::default();
        let path = Path::new("music/a/01.flac");
        let (tx, rx) = crossbeam_channel::bounded(1);
        assert!(reads.start(
            path,
            || {
                std::thread::sleep(Duration::from_millis(800));
                -7.5
            },
            move |gain| {
                let _ = tx.send(gain);
            },
        ));
        let asked = Instant::now();
        assert_eq!(reads.wait(path, FACTS_WAIT), None);
        assert!(asked.elapsed() < Duration::from_millis(400), "held for {:?}", asked.elapsed());
        reads.guessed(path, 7, -6.0);
        // The read ends on its own thread and says so.
        assert_eq!(rx.recv_timeout(Duration::from_secs(10)), Ok(-7.5));
        assert_eq!(reads.done(path), vec![(7, -6.0)]);
        assert!(reads.done(path).is_empty());
    }

    #[test]
    fn a_quick_read_gives_its_gain_to_the_start_itself() {
        let mut reads = FactsReads::default();
        let path = Path::new("music/a/02.flac");
        assert!(reads.start(path, || -4.0, |_| {}));
        assert_eq!(reads.wait(path, Duration::from_secs(10)), Some(-4.0));
        // Nothing played it with a guess.
        assert!(reads.done(path).is_empty());
    }

    #[test]
    fn a_file_is_read_once_at_a_time() {
        let mut reads = FactsReads::default();
        let path = Path::new("music/a/03.flac");
        let count = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = crossbeam_channel::bounded::<()>(0);
        for _ in 0..3 {
            let count = count.clone();
            let rx = rx.clone();
            assert!(reads.start(
                path,
                move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    let _ = rx.recv_timeout(Duration::from_secs(10));
                    0.0
                },
                |_| {},
            ));
        }
        drop(tx);
        assert_eq!(reads.wait(path, Duration::from_secs(10)), Some(0.0));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // Done: the next start reads it again.
        reads.done(path);
        assert!(reads.start(path, || 1.0, |_| {}));
        assert_eq!(reads.wait(path, Duration::from_secs(10)), Some(1.0));
    }
}
