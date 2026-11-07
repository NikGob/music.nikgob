//! Background sound analysis of the library's files (docs/stage4.md §2).
//!
//! One thread (two with 8 or more logical cores, the second only while
//! nothing plays) at the lowest priority takes batches of [`Library::tracks_needing_analysis`]: tracks with a file
//! and no current analysis, an analysis of an older bliss, or one of a
//! stream. Each file is decoded with the importer's decoder straight into
//! [`PcmAnalyzer`] (only its first 8 minutes, as 22 050 Hz mono), and the
//! vector goes to the library and into the in-memory [`SoundIndex`] the
//! recommendations query. A file that cannot be decoded is marked and not
//! tried again until it changes; one that cannot be opened (a drive that is
//! not there) is only skipped until the next start. Imports wake the job up
//! ([`FileAnalysis::kick`]); without work it looks again now and then. The
//! worker thread never waits for any of it: progress comes back as
//! [`Msg::Progress`].
//!
//! bliss computes its descriptors on a few scoped threads of its own, which
//! run at normal priority for the second or so the features of one track
//! take; decoding and resampling, the bulk of the time, run on ours. Only
//! one such feature pass runs at a time (see `nikgob_recs`), and while
//! music plays only one thread works, so the two never pile up on the CPU
//! the audio threads need.

use std::collections::{HashSet, VecDeque};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{Analysis, AnalysisSource, TrackId};
use nikgob_import::ImportError;
use nikgob_import::decode::{PcmSpec, decode_bytes, decode_file};
use nikgob_library::{AnalysisTodo, Library};
use nikgob_recs::{ANALYZED_WITH, AnalysisError, PcmAnalyzer, Vector, VectorIndex};
use nikgob_view as v;
use parking_lot::{Condvar, Mutex, RwLock};

use crate::Worker;
use crate::import::Internal;

/// Every analysed track's vector, by track (files and streams alike).
pub(crate) type SoundIndex = Arc<RwLock<VectorIndex<TrackId>>>;

/// Tracks taken from the library at a time.
const BATCH: u32 = 32;
/// Progress goes to the view at most this often while the job works.
const PROGRESS_EVERY: Duration = Duration::from_secs(2);
/// A shutdown waits this long for a track being analysed.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// From the analysis threads to the worker.
pub(crate) enum Msg {
    Progress(v::AnalysisProgress),
}

/// Why a file has no vector.
#[derive(Debug)]
pub(crate) enum FileError {
    /// The job is stopping.
    Stopped,
    /// The file could not be read (missing, locked, a drive not there):
    /// maybe later.
    Unavailable(String),
    /// The file is read but cannot be analysed (a broken or unsupported
    /// stream, too short): not until it changes.
    Undecodable(String),
}

/// Decodes `path` and analyses its first 8 minutes. `stop` aborts the decode.
pub(crate) fn analyze_file(path: &Path, stop: &AtomicBool) -> Result<Vector, FileError> {
    analyze_decoded(|sink| decode_file(path, sink), stop)
}

/// [`analyze_file`] for encoded audio in memory (a stream's; `ext` is its
/// container as a file extension, `name` names it in the log).
pub(crate) fn analyze_bytes(bytes: Vec<u8>, ext: &str, name: &str, stop: &AtomicBool) -> Result<Vector, FileError> {
    analyze_decoded(|sink| decode_bytes(bytes, ext, name, sink), stop)
}

type Sink<'a> = &'a mut dyn FnMut(&[f32], PcmSpec) -> ControlFlow<()>;

fn analyze_decoded(
    decode: impl FnOnce(Sink<'_>) -> Result<PcmSpec, ImportError>,
    stop: &AtomicBool,
) -> Result<Vector, FileError> {
    let mut analyzer: Option<PcmAnalyzer> = None;
    let mut format: Option<AnalysisError> = None;
    let mut sink = |chunk: &[f32], spec: PcmSpec| -> ControlFlow<()> {
        if stop.load(Ordering::Relaxed) {
            return ControlFlow::Break(());
        }
        let a = match &mut analyzer {
            Some(a) => a,
            None => match PcmAnalyzer::new(spec.sample_rate, spec.channels) {
                Ok(a) => analyzer.insert(a),
                Err(e) => {
                    format = Some(e);
                    return ControlFlow::Break(());
                }
            },
        };
        a.push(chunk)
    };
    let decoded = decode(&mut sink);
    if stop.load(Ordering::Relaxed) {
        return Err(FileError::Stopped);
    }
    match decoded {
        Ok(_) => {}
        Err(e @ ImportError::Io { .. }) => return Err(FileError::Unavailable(e.to_string())),
        Err(e) => return Err(FileError::Undecodable(e.to_string())),
    }
    if let Some(e) = format {
        return Err(FileError::Undecodable(e.to_string()));
    }
    let analyzer = analyzer.ok_or_else(|| FileError::Undecodable("no audio".into()))?;
    analyzer.finish().map_err(|e| FileError::Undecodable(e.to_string()))
}

/// Stores a track's vector in the library and, if it was stored (a
/// stream's never replaces a file's), in the index.
pub(crate) fn store_vector(
    lib: &Library,
    index: &SoundIndex,
    track: TrackId,
    vector: Vector,
    source: AnalysisSource,
) -> nikgob_library::Result<bool> {
    let stored = lib.put_analysis(&Analysis {
        track_id: track,
        bliss_vector: vector.to_vec(),
        analyzed_with: ANALYZED_WITH.to_owned(),
        source,
    })?;
    if stored {
        index.write().insert(track, vector);
    }
    Ok(stored)
}

/// Fills `index` with every current vector of the library.
pub(crate) fn load_index(lib: &Library, index: &SoundIndex) {
    let rows = match lib.analysis_vectors(ANALYZED_WITH) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("analysis: loading the vectors: {e}");
            return;
        }
    };
    let mut idx = index.write();
    for row in rows {
        match Vector::try_from(row.bliss_vector.as_slice()) {
            Ok(v) => {
                idx.insert(row.track_id, v);
            }
            Err(_) => log::warn!("analysis: track {} has a vector of {} features", row.track_id, row.bliss_vector.len()),
        }
    }
    log::info!("analysis: {} vectors in memory", idx.len());
}

/// Absolute path of a source locator (relative ones are under the library).
fn file_path(root: &Path, locator: &str) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { root.join(p) }
}

#[derive(Default)]
struct Queue {
    todo: VecDeque<AnalysisTodo>,
    /// Being analysed right now (by either thread).
    busy: HashSet<TrackId>,
    /// Not tried again in this run (could not be opened, or not stored).
    skip: HashSet<TrackId>,
    /// Work may have arrived since the last look.
    kicked: bool,
}

struct Shared {
    lib: Library,
    root: PathBuf,
    index: SoundIndex,
    tx: Sender<Internal>,
    stop: AtomicBool,
    queue: Mutex<Queue>,
    wake: Condvar,
    /// The progress last sent, and when.
    sent: Mutex<(Option<v::AnalysisProgress>, Instant)>,
    /// Without work, the library is looked at again this often.
    idle: Duration,
    /// Music is playing: the second thread waits.
    playing: AtomicBool,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Sleeps `d` unless stopped or kicked meanwhile.
    fn pause(&self, d: Duration) {
        let mut q = self.queue.lock();
        let until = Instant::now() + d;
        while !self.stopped() && !q.kicked {
            if self.wake.wait_until(&mut q, until).timed_out() {
                break;
            }
        }
    }

    /// The next file to analyse; waits while there is none (and, for the
    /// `helper` thread, while music plays). `None` = stop.
    fn next(&self, helper: bool) -> Option<AnalysisTodo> {
        let mut q = self.queue.lock();
        loop {
            if self.stopped() {
                return None;
            }
            if helper && self.playing.load(Ordering::Relaxed) {
                self.wake.wait_for(&mut q, self.idle);
                continue;
            }
            while let Some(t) = q.todo.pop_front() {
                if q.busy.insert(t.track_id) {
                    return Some(t);
                }
            }
            q.kicked = false;
            // The library is read without the queue lock, so a kick from
            // the worker never waits for it (one arriving meanwhile sets
            // `kicked` again and the wait below is skipped).
            let listed =
                parking_lot::MutexGuard::unlocked(&mut q, || self.lib.tracks_needing_analysis(ANALYZED_WITH, BATCH));
            match listed {
                Ok(list) => {
                    let fresh: Vec<_> = list
                        .into_iter()
                        .filter(|t| !q.busy.contains(&t.track_id) && !q.skip.contains(&t.track_id))
                        .collect();
                    if !fresh.is_empty() {
                        q.todo.extend(fresh);
                        continue;
                    }
                }
                Err(e) => log::warn!("analysis: reading the queue: {e}"),
            }
            parking_lot::MutexGuard::unlocked(&mut q, || self.report(true));
            if !q.kicked && !self.stopped() {
                self.wake.wait_for(&mut q, self.idle);
            }
        }
    }

    fn done(&self, track: TrackId, skip: bool) {
        let mut q = self.queue.lock();
        q.busy.remove(&track);
        if skip {
            q.skip.insert(track);
        }
    }

    fn work(&self, todo: AnalysisTodo) {
        let path = file_path(&self.root, &todo.locator);
        let started = Instant::now();
        // At info, so a dropout in the log can be set against the analysis.
        log::info!(
            "analysis: starting {}{}",
            path.display(),
            if self.playing.load(Ordering::Relaxed) { " (music playing)" } else { "" }
        );
        let skip = match analyze_file(&path, &self.stop) {
            Ok(vector) => match store_vector(&self.lib, &self.index, todo.track_id, vector, AnalysisSource::File) {
                Ok(_) => {
                    log::info!("analysis: {} done in {:?}", path.display(), started.elapsed());
                    false
                }
                Err(e) => {
                    log::warn!("analysis: storing {}: {e}", todo.track_id);
                    true
                }
            },
            Err(FileError::Stopped) => false,
            Err(FileError::Unavailable(e)) => {
                log::info!("analysis: skipping {}: {e}", path.display());
                true
            }
            Err(FileError::Undecodable(e)) => {
                log::info!("analysis: cannot analyse {}: {e}", path.display());
                match self.lib.mark_analysis_failed(&todo, &e) {
                    Ok(()) => false,
                    Err(e) => {
                        log::warn!("analysis: marking {}: {e}", todo.track_id);
                        true
                    }
                }
            }
        };
        self.done(todo.track_id, skip);
        self.report(false);
    }

    /// Sends the progress if it changed (and, unless `now`, at most every
    /// [`PROGRESS_EVERY`]).
    fn report(&self, now: bool) {
        {
            let sent = self.sent.lock();
            if !now && sent.0.is_some() && sent.1.elapsed() < PROGRESS_EVERY {
                return;
            }
        }
        let counts = match self.lib.analysis_counts(ANALYZED_WITH) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("analysis: counting: {e}");
                return;
            }
        };
        let p = v::AnalysisProgress {
            files_done: counts.files_done,
            files_total: counts.files_total,
            streams_done: counts.streams_done,
            streams_total: counts.streams_total,
        };
        let mut sent = self.sent.lock();
        sent.1 = Instant::now();
        if sent.0 != Some(p) {
            sent.0 = Some(p);
            let _ = self.tx.send(Internal::Analysis(Msg::Progress(p)));
        }
    }

    fn run(self: &Arc<Self>, helper: bool, start: Duration, alive: Sender<()>) {
        lower_priority();
        if !helper {
            load_index(&self.lib, &self.index);
            self.report(true);
            self.pause(start);
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
            if cores >= 8 && !self.stopped() {
                let shared = self.clone();
                let alive = alive.clone();
                let spawned = std::thread::Builder::new()
                    .name("nikgob-analysis-2".into())
                    .spawn(move || shared.run(true, Duration::ZERO, alive));
                if let Err(e) = spawned {
                    log::warn!("analysis: second thread: {e}");
                }
            }
        }
        while let Some(todo) = self.next(helper) {
            self.work(todo);
        }
        drop(alive);
    }
}

pub(crate) fn lower_priority() {
    if let Err(e) = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min) {
        log::debug!("analysis: thread priority: {e:?}");
    }
}

/// The background file analysis and the vector index it fills.
pub(crate) struct FileAnalysis {
    index: SoundIndex,
    tx: Sender<Internal>,
    shared: Option<Arc<Shared>>,
    /// Disconnects when every analysis thread has ended.
    gone: Option<crossbeam_channel::Receiver<()>>,
}

impl FileAnalysis {
    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self {
            index: Arc::new(RwLock::new(VectorIndex::new())),
            tx,
            shared: None,
            gone: None,
        }
    }

    /// The sound vectors of the library (files and streams), z-normalised
    /// neighbours via [`VectorIndex::nearest`]; filled at start and as the
    /// analysis goes. The recommendations copy it into their snapshot.
    pub(crate) fn index(&self) -> &SoundIndex {
        &self.index
    }

    /// Loads the index and starts analysing after `start` (playback and the
    /// first screens go first); `idle` is how often the library is looked
    /// at again when there is nothing to do.
    pub(crate) fn start(&mut self, lib: Library, root: PathBuf, start: Duration, idle: Duration) {
        if self.shared.is_some() {
            return;
        }
        let shared = Arc::new(Shared {
            lib,
            root,
            index: self.index.clone(),
            tx: self.tx.clone(),
            stop: AtomicBool::new(false),
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
            sent: Mutex::new((None, Instant::now())),
            idle,
            playing: AtomicBool::new(false),
        });
        let (alive, gone) = crossbeam_channel::bounded::<()>(0);
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("nikgob-analysis".into())
            .spawn(move || s.run(false, start, alive));
        match spawned {
            Ok(_) => {
                self.shared = Some(shared);
                self.gone = Some(gone);
            }
            Err(e) => log::warn!("analysis: cannot start: {e}"),
        }
    }

    /// Whether music plays: while it does, only one thread analyses.
    pub(crate) fn set_playing(&self, playing: bool) {
        if let Some(s) = &self.shared
            && s.playing.swap(playing, Ordering::Relaxed) != playing
            && !playing
        {
            let _q = s.queue.lock();
            s.wake.notify_all();
        }
    }

    /// New files may be waiting (an import landed).
    pub(crate) fn kick(&self) {
        if let Some(s) = &self.shared {
            s.queue.lock().kicked = true;
            s.wake.notify_all();
        }
    }

    /// Stops the threads: a decode stops at its next chunk, bliss finishes
    /// the track it is on. Waits up to [`STOP_WAIT`].
    pub(crate) fn shutdown(&mut self) {
        let Some(s) = self.shared.take() else { return };
        s.stop.store(true, Ordering::Relaxed);
        {
            let _q = s.queue.lock();
            s.wake.notify_all();
        }
        if let Some(gone) = self.gone.take()
            && let Err(crossbeam_channel::RecvTimeoutError::Timeout) = gone.recv_timeout(STOP_WAIT)
        {
            log::warn!("analysis: a thread is still busy at shutdown");
        }
    }
}

impl Drop for FileAnalysis {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Worker {
    pub(crate) fn start_analysis(&mut self) {
        let root = self.res.root().to_owned();
        let (start, idle) = (self.streaming.schedule.analysis_start, self.streaming.schedule.analysis_idle);
        self.analysis.start(self.lib.clone(), root, start, idle);
    }

    pub(crate) fn analysis_msg(&mut self, m: Msg) {
        match m {
            Msg::Progress(p) => {
                if self.state.read().analysis != p {
                    self.publish(|s| s.analysis = p);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_wav(path: &Path, rate: u32, secs: f32, freq: f32) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        let mut seed = 7u32;
        for i in 0..(secs * rate as f32) as u32 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((seed >> 16) as f32 / 65_536.0 - 0.5) * 0.1;
            let t = i as f32 / rate as f32;
            // A note every half second, decaying.
            let env = (-6.0 * (t % 0.5)).exp();
            let tone = (t * freq * std::f32::consts::TAU).sin() * 0.4 * env;
            let s = ((tone + noise) * 20_000.0) as i16;
            w.write_sample(s).unwrap();
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn a_wav_file_analyses_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let low = dir.path().join("low.wav");
        let high = dir.path().join("high.wav");
        write_wav(&low, 44_100, 6.0, 220.0);
        write_wav(&high, 48_000, 6.0, 1760.0);
        let stop = AtomicBool::new(false);
        let a = analyze_file(&low, &stop).unwrap();
        let b = analyze_file(&high, &stop).unwrap();
        assert!(a.iter().chain(&b).all(|x| x.is_finite()));
        assert_ne!(a, b);
        assert_eq!(analyze_file(&low, &stop).unwrap(), a, "deterministic");

        // Not a sound: marked as undecodable; missing: only unavailable.
        let junk = dir.path().join("junk.flac");
        std::fs::write(&junk, b"not audio at all").unwrap();
        assert!(matches!(analyze_file(&junk, &stop), Err(FileError::Undecodable(_))));
        assert!(matches!(
            analyze_file(&dir.path().join("gone.opus"), &stop),
            Err(FileError::Unavailable(_))
        ));
        // Too short to say anything.
        let blip = dir.path().join("blip.wav");
        write_wav(&blip, 44_100, 0.3, 440.0);
        assert!(matches!(analyze_file(&blip, &stop), Err(FileError::Undecodable(_))));
        stop.store(true, Ordering::Relaxed);
        assert!(matches!(analyze_file(&low, &stop), Err(FileError::Stopped)));
    }

    #[test]
    fn stored_vectors_reach_the_index_and_streams_never_replace_files() {
        let lib = Library::open_in_memory().unwrap();
        let index: SoundIndex = Arc::new(RwLock::new(VectorIndex::new()));
        let credit = nikgob_core::model::ArtistCredit::single(String::from("A"), None);
        let t = lib.upsert_track(&nikgob_core::model::Track::new("T", credit)).unwrap().id;
        let file = [0.5; nikgob_recs::DIM];
        let stream = [0.1; nikgob_recs::DIM];
        assert!(store_vector(&lib, &index, t, file, AnalysisSource::File).unwrap());
        assert!(!store_vector(&lib, &index, t, stream, AnalysisSource::Youtube).unwrap());
        assert_eq!(index.read().get(t), Some(&file));
        // What a restart loads.
        let other: SoundIndex = Arc::new(RwLock::new(VectorIndex::new()));
        load_index(&lib, &other);
        assert_eq!(other.read().get(t), Some(&file));
    }
}
