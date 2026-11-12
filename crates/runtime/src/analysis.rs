//! Background sound analysis of the library's files (docs/stage4.md §2,
//! docs/recommendations.md §5).
//!
//! One thread at the lowest priority, paced by [`crate::quiet`] (it rests
//! five times what it worked, waits while a game runs, and takes the turn
//! of heavy jobs so it never runs beside the lyrics alignment's pass or
//! the quality check), takes batches of [`Library::tracks_needing_analysis`]:
//! tracks with a file and no current analysis, an analysis of an older
//! bliss, or one of a stream. Each file is decoded with the importer's
//! decoder only in its three windows ([`nikgob_recs::windows`]: 15 s at
//! 20, 45 and 70 % of its length, seeking to each) straight into a
//! [`WindowedAnalyzer`], and the vector goes to the library and into the
//! in-memory [`SoundIndex`] the recommendations query. A file that cannot be
//! decoded is marked and not tried again until it changes; one that cannot
//! be opened (a drive that is not there) is only skipped until the next
//! start. Imports wake the job up ([`FileAnalysis::kick`]); without work it
//! looks again now and then. The worker thread never waits for any of it:
//! progress comes back as [`Msg::Progress`].
//!
//! With «Нейросетевой анализ звука» on ([`Neural`]), the same windows also
//! go through the neural sound model ([`crate::sound_model::Effnet`]) and
//! its compact vector is kept beside bliss's; the files analysed before get
//! theirs from a queue of their own ([`Library::tracks_needing_embedding`])
//! once bliss's is empty. The model is downloaded here when a mode needs it
//! and dropped from memory [`crate::sound_model::KEEP_LOADED`] after the
//! last window.
//!
//! bliss computes its descriptors on a few scoped threads of its own, which
//! run at normal priority for the second or so the features of one track
//! take; decoding and resampling run on ours. Only one such feature pass
//! runs at a time (see `nikgob_recs`), and the rest after each track counts
//! it too.

use std::collections::{HashSet, VecDeque};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{Analysis, AnalysisSource, TrackId};
use nikgob_import::ImportError;
use nikgob_import::decode::{Feed, PcmSpec, Span, Take, decode_bytes_windows, decode_file_windows};
use nikgob_library::{AnalysisTodo, Library};
use nikgob_recs::{
    ANALYZED_WITH, Analysed, AnalysisError, Compact, EMBEDDED_WITH, SoundModel, Vector, VectorIndex, Window, WindowedAnalyzer,
};
use nikgob_view as v;
use parking_lot::{Condvar, Mutex, RwLock};

use crate::Worker;
use crate::import::Internal;
use crate::quiet::{Pacer, lower_priority};
use crate::sound_model::{Effnet, ModelState};

/// Every analysed track's vector, by track (files and streams alike).
pub(crate) type SoundIndex = Arc<RwLock<VectorIndex<TrackId>>>;

/// Tracks taken from the library at a time.
const BATCH: u32 = 32;
/// Progress goes to the view at most this often while the job works.
const PROGRESS_EVERY: Duration = Duration::from_secs(2);
/// A shutdown waits this long for a track being analysed.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// From the analysis thread to the worker.
pub(crate) enum Msg {
    Progress(v::AnalysisProgress),
    SoundModel(v::SoundModelView),
}

/// What the neural sound model hears now: «Нейросетевой анализ звука» as
/// the prefs say ([`v::Prefs::sound_neural_or`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Neural {
    /// The library's tracks.
    pub(crate) library: bool,
    /// The previews of new music.
    pub(crate) previews: bool,
}

impl Neural {
    pub(crate) fn of(mode: v::SoundNeural) -> Self {
        Self { library: mode.library(), previews: mode.previews() }
    }
}

/// What a pass over a track computes.
#[derive(Clone, Copy)]
pub(crate) struct Want<'m> {
    /// bliss's vector.
    pub(crate) bliss: bool,
    /// The neural sound model's.
    pub(crate) model: Option<&'m dyn SoundModel>,
}

#[cfg(test)]
impl Want<'_> {
    /// bliss alone.
    pub(crate) const BLISS: Want<'static> = Want { bliss: true, model: None };
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

/// Decodes the windows of `path` and analyses them, at `pacer`'s pace (its
/// stop aborts the decode).
pub(crate) fn analyze_file(path: &Path, want: Want<'_>, pacer: &mut Pacer<'_>) -> Result<Analysed, FileError> {
    analyze_decoded(|plan, sink| decode_file_windows(path, plan, sink), nikgob_recs::windows, want, pacer)
}

/// [`analyze_file`] for encoded audio in memory (a stream's; `ext` is its
/// container as a file extension, `name` names it in the log); a
/// `preview` (the head of a stream) is analysed in its one window
/// ([`nikgob_recs::preview_windows`]).
pub(crate) fn analyze_bytes(
    bytes: Vec<u8>,
    ext: &str,
    name: &str,
    preview: bool,
    want: Want<'_>,
    pacer: &mut Pacer<'_>,
) -> Result<Analysed, FileError> {
    let plan = if preview { nikgob_recs::preview_windows } else { nikgob_recs::windows };
    analyze_decoded(|p, sink| decode_bytes_windows(bytes, ext, name, p, sink), plan, want, pacer)
}

type Plan<'a> = &'a mut dyn FnMut(Option<f64>) -> Vec<Span>;
type Sink<'a> = &'a mut dyn FnMut(Feed<'_>) -> Take;

fn analyze_decoded(
    decode: impl FnOnce(Plan<'_>, Sink<'_>) -> Result<PcmSpec, ImportError>,
    windows: fn(Option<f64>) -> Vec<Window>,
    want: Want<'_>,
    pacer: &mut Pacer<'_>,
) -> Result<Analysed, FileError> {
    let mut analyzer: Option<WindowedAnalyzer<'_>> = None;
    let mut format: Option<AnalysisError> = None;
    let mut plan = |d: Option<f64>| -> Vec<Span> { windows(d).into_iter().map(|w| Span { start: w.start, len: w.len }).collect() };
    // Audio read past between windows counts as work too: the pace, the
    // game pause and a stop apply while the heavy turn is held.
    let mut sink = |feed: Feed<'_>| -> Take {
        if format.is_some() || !pacer.step() {
            return Take::Stop;
        }
        let Feed::Audio { span, pcm, spec } = feed else { return Take::More };
        let a = match &mut analyzer {
            Some(a) => a,
            None => match WindowedAnalyzer::new(spec.sample_rate, spec.channels, want.bliss, want.model) {
                Ok(a) => analyzer.insert(a),
                Err(e) => {
                    format = Some(e);
                    return Take::Stop;
                }
            },
        };
        match a.push(span, pcm) {
            ControlFlow::Continue(()) => Take::More,
            ControlFlow::Break(()) => Take::NextSpan,
        }
    };
    let decoded = decode(&mut plan, &mut sink);
    if pacer.stopped() {
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
    let out = analyzer.finish().map_err(|e| FileError::Undecodable(e.to_string()))?;
    if let Some(Err(e)) = &out.bliss {
        return Err(FileError::Undecodable(e.to_string()));
    }
    Ok(out)
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

/// Stores the neural model's vector of a track (after its bliss vector:
/// the index keeps it only beside one). A track the model cannot hear (too
/// short) is marked so; a failure of the model itself stores nothing
/// (`Ok(None)`: tried again later).
pub(crate) fn store_embedding(
    lib: &Library,
    index: &SoundIndex,
    track: TrackId,
    embedding: &Result<Vec<f32>, AnalysisError>,
    source: AnalysisSource,
) -> nikgob_library::Result<Option<bool>> {
    match embedding {
        Ok(raw) => match Compact::of_raw(raw) {
            Some(c) => {
                let stored = lib.put_embedding(track, Some(&c.to_bytes()), EMBEDDED_WITH, source)?;
                if stored {
                    index.write().set_embedding(track, c);
                }
                Ok(Some(stored))
            }
            None => Ok(Some(lib.put_embedding(track, None, EMBEDDED_WITH, source)?)),
        },
        Err(AnalysisError::Model(_)) => Ok(None),
        Err(_) => Ok(Some(lib.put_embedding(track, None, EMBEDDED_WITH, source)?)),
    }
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

/// Puts every stored vector of the neural model into `index` (beside the
/// bliss vectors already there).
pub(crate) fn load_embeddings(lib: &Library, index: &SoundIndex) {
    let rows = match lib.embedding_vectors(EMBEDDED_WITH) {
        Ok(rows) => rows,
        Err(e) => {
            log::warn!("analysis: loading the sound model's vectors: {e}");
            return;
        }
    };
    let mut idx = index.write();
    let mut n = 0;
    for (track, bytes) in rows {
        if let Some(c) = Compact::from_bytes(&bytes)
            && idx.set_embedding(track, c)
        {
            n += 1;
        }
    }
    log::info!("analysis: {n} vectors of the sound model in memory");
}

/// The first [`BATCH`] rows of a queue (`read` with a limit) that are not
/// `aside`: it reads past the rows set aside, so files that cannot be
/// opened at the head of the queue never hide the rest (nor the model's
/// queue and its download behind it).
fn fresh(
    read: impl Fn(u32) -> nikgob_library::Result<Vec<AnalysisTodo>>,
    aside: &HashSet<TrackId>,
) -> nikgob_library::Result<Vec<AnalysisTodo>> {
    let limit = BATCH.saturating_add(u32::try_from(aside.len()).unwrap_or(u32::MAX));
    Ok(read(limit)?.into_iter().filter(|t| !aside.contains(&t.track_id)).take(BATCH as usize).collect())
}

/// Absolute path of a source locator (relative ones are under the library).
fn file_path(root: &Path, locator: &str) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { root.join(p) }
}

/// What a queued track needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Its bliss vector (and the model's, when it is on).
    Bliss,
    /// Only the neural model's vector (bliss's is current).
    Embed,
}

#[derive(Default)]
struct Queue {
    todo: VecDeque<(AnalysisTodo, Kind)>,
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
    /// The model's state last sent.
    model_sent: Mutex<Option<v::SoundModelView>>,
    /// Without work, the library is looked at again this often.
    idle: Duration,
    /// Music is playing (said in the log).
    playing: AtomicBool,
    /// The neural model (none without a models folder: tests).
    effnet: Option<Arc<Effnet>>,
    neural: Mutex<Neural>,
    /// The model's vectors are in the index.
    embeddings_loaded: AtomicBool,
}

impl Shared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn neural(&self) -> Neural {
        *self.neural.lock()
    }

    /// The model, when the library's tracks get its vector and it can run.
    fn model(&self) -> Option<&Effnet> {
        let e = self.effnet.as_deref()?;
        (self.neural().library && e.ready() && !e.resting()).then_some(e)
    }

    /// The index holds the model's vectors exactly while the mode wants
    /// them (loaded here, off the worker's loop).
    fn sync_embeddings(&self) {
        let want = self.neural().library && self.effnet.is_some();
        if want != self.embeddings_loaded.load(Ordering::Relaxed) {
            if want {
                load_embeddings(&self.lib, &self.index);
            } else {
                self.index.write().clear_embeddings();
                if let Some(e) = &self.effnet {
                    e.unload();
                }
                log::info!("analysis: the sound model's vectors left memory");
            }
            self.embeddings_loaded.store(want, Ordering::Relaxed);
        }
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

    /// The library's tracks due now, without those `aside` (being analysed
    /// or skipped in this run): bliss's queue first, then the model's.
    fn list(&self, aside: &HashSet<TrackId>) -> nikgob_library::Result<Vec<(AnalysisTodo, Kind)>> {
        let bliss = fresh(|limit| self.lib.tracks_needing_analysis(ANALYZED_WITH, limit), aside)?;
        if !bliss.is_empty() {
            return Ok(bliss.into_iter().map(|t| (t, Kind::Bliss)).collect());
        }
        let Some(e) = self.effnet.as_deref().filter(|_| self.neural().library) else { return Ok(Vec::new()) };
        // The mode wants the model: fetched once (resumed if cut off).
        if !e.ready() && !e.resting() {
            e.download(&self.stop, &mut || self.report_model());
            self.report_model();
        }
        if !e.ready() || e.resting() {
            return Ok(Vec::new());
        }
        let embed = fresh(|limit| self.lib.tracks_needing_embedding(EMBEDDED_WITH, ANALYZED_WITH, limit), aside)?;
        Ok(embed.into_iter().map(|t| (t, Kind::Embed)).collect())
    }

    /// The next file to analyse; waits while there is none. `None` = stop.
    fn next(&self) -> Option<(AnalysisTodo, Kind)> {
        let mut q = self.queue.lock();
        loop {
            if self.stopped() {
                return None;
            }
            while let Some(t) = q.todo.pop_front() {
                if q.busy.insert(t.0.track_id) {
                    return Some(t);
                }
            }
            q.kicked = false;
            // The library is read without the queue lock, so a kick from
            // the worker never waits for it (one arriving meanwhile sets
            // `kicked` again and the wait below is skipped).
            let aside: HashSet<TrackId> = q.busy.union(&q.skip).copied().collect();
            let listed = parking_lot::MutexGuard::unlocked(&mut q, || {
                self.sync_embeddings();
                self.list(&aside)
            });
            match listed {
                Ok(list) => {
                    let fresh: Vec<_> = list
                        .into_iter()
                        .filter(|t| !q.busy.contains(&t.0.track_id) && !q.skip.contains(&t.0.track_id))
                        .collect();
                    if !fresh.is_empty() {
                        q.todo.extend(fresh);
                        continue;
                    }
                }
                Err(e) => log::warn!("analysis: reading the queue: {e}"),
            }
            parking_lot::MutexGuard::unlocked(&mut q, || {
                self.report(true);
                // Nothing to do: what the last tracks left goes back.
                crate::release_thread_memory();
            });
            // (The model's idle session goes by its own watch: nothing here
            // waits for it while holding the queue.)
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

    fn work(&self, todo: AnalysisTodo, kind: Kind, pacer: &mut Pacer<'_>) {
        let path = file_path(&self.root, &todo.locator);
        let started = Instant::now();
        let model = self.model();
        if kind == Kind::Embed && model.is_none() {
            self.done(todo.track_id, false);
            return;
        }
        // At info, so a dropout in the log can be set against the analysis.
        log::info!(
            "analysis: starting {}{}{}",
            path.display(),
            if kind == Kind::Embed { " (sound model only)" } else { "" },
            if self.playing.load(Ordering::Relaxed) { " (music playing)" } else { "" }
        );
        let want = Want { bliss: kind == Kind::Bliss, model: model.map(|m| m as &dyn SoundModel) };
        let skip = match analyze_file(&path, want, pacer) {
            Ok(out) => self.keep(todo.track_id, &path, out, started),
            Err(FileError::Stopped) => false,
            Err(FileError::Unavailable(e)) => {
                log::info!("analysis: skipping {}: {e}", path.display());
                true
            }
            Err(FileError::Undecodable(e)) if kind == Kind::Embed => {
                log::info!("analysis: the sound model cannot hear {}: {e}", path.display());
                let failed = Err(AnalysisError::TooShort);
                store_embedding(&self.lib, &self.index, todo.track_id, &failed, AnalysisSource::File).is_err()
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

    /// Stores what a pass gave; true when the track is to be skipped in
    /// this run (it could not be stored).
    fn keep(&self, track: TrackId, path: &Path, out: Analysed, started: Instant) -> bool {
        if let Some(Ok(vector)) = out.bliss
            && let Err(e) = store_vector(&self.lib, &self.index, track, vector, AnalysisSource::File)
        {
            log::warn!("analysis: storing {track}: {e}");
            return true;
        }
        let embedded = match &out.embedding {
            None => "",
            Some(result) => match store_embedding(&self.lib, &self.index, track, result, AnalysisSource::File) {
                Ok(Some(_)) if result.is_ok() => " with the sound model",
                Ok(Some(_)) => " (the sound model cannot hear it)",
                Ok(None) => " (the sound model failed)",
                Err(e) => {
                    log::warn!("analysis: storing the sound model's vector of {track}: {e}");
                    return true;
                }
            },
        };
        log::info!("analysis: {} done{embedded} in {:?} ({:.0} s of audio)", path.display(), started.elapsed(), out.seconds);
        false
    }

    fn model_view(&self) -> v::SoundModelView {
        let Some(e) = &self.effnet else { return v::SoundModelView::default() };
        let state = e.state();
        let (done, total) = if self.neural().library { self.lib.embedding_counts(EMBEDDED_WITH).unwrap_or((0, 0)) } else { (0, 0) };
        v::SoundModelView {
            ready: e.ready(),
            downloading: match state {
                ModelState::Downloading(f) => Some((f * 100.0).round().clamp(0.0, 100.0) as u8),
                _ => None,
            },
            failed: state == ModelState::Failed,
            done,
            total,
        }
    }

    /// Sends the model's state if it changed.
    fn report_model(&self) {
        let m = self.model_view();
        let mut sent = self.model_sent.lock();
        if *sent != Some(m) {
            *sent = Some(m);
            let _ = self.tx.send(Internal::Analysis(Msg::SoundModel(m)));
        }
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
        self.report_model();
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

    fn run(self: &Arc<Self>, start: Duration, alive: Sender<()>) {
        lower_priority();
        load_index(&self.lib, &self.index);
        self.sync_embeddings();
        self.report(true);
        self.pause(start);
        let mut pacer = Pacer::heavy(&self.stop, "sound analysis");
        let mut since_release = 0;
        while let Some((todo, kind)) = self.next() {
            if !pacer.begin() {
                self.done(todo.track_id, false);
                break;
            }
            self.work(todo, kind, &mut pacer);
            // A batch's worth of decoding and bliss's threads: this
            // thread's heap gives back what they left.
            since_release += 1;
            if since_release >= BATCH {
                since_release = 0;
                crate::release_thread_memory();
            }
            pacer.rest();
        }
        if let Some(e) = &self.effnet {
            e.unload();
        }
        drop(alive);
    }
}

/// The background file analysis and the vector index it fills.
pub(crate) struct FileAnalysis {
    index: SoundIndex,
    tx: Sender<Internal>,
    shared: Option<Arc<Shared>>,
    /// Disconnects when the analysis thread has ended.
    gone: Option<crossbeam_channel::Receiver<()>>,
    /// The neural model (made with the first start that has a models folder).
    effnet: Option<Arc<Effnet>>,
    neural: Neural,
}

impl FileAnalysis {
    pub(crate) fn new(tx: Sender<Internal>) -> Self {
        Self {
            index: Arc::new(RwLock::new(VectorIndex::new())),
            tx,
            shared: None,
            gone: None,
            effnet: None,
            neural: Neural::default(),
        }
    }

    /// The sound vectors of the library (files and streams), z-normalised
    /// neighbours via [`VectorIndex::nearest`]; filled at start and as the
    /// analysis goes. The recommendations copy it into their snapshot.
    pub(crate) fn index(&self) -> &SoundIndex {
        &self.index
    }

    /// The neural sound model, for the stream analysis's previews.
    pub(crate) fn effnet(&self) -> Option<Arc<Effnet>> {
        self.effnet.clone()
    }

    /// What the neural model hears now.
    pub(crate) fn neural(&self) -> Neural {
        self.neural
    }

    /// The model's files are on disk (`false` without a models folder).
    pub(crate) fn model_ready(&self, models: Option<&Path>) -> bool {
        match (&self.effnet, models) {
            (Some(e), _) => e.ready(),
            (None, Some(dir)) => Effnet::new(dir.to_owned()).ready(),
            (None, None) => false,
        }
    }

    /// «Нейросетевой анализ звука» and «На чём считать анализ звука» as the
    /// prefs say: the thread loads or drops the model's vectors and picks
    /// the model's queue up.
    pub(crate) fn set_neural(&mut self, neural: Neural, device: nikgob_view::SoundDevice) {
        self.neural = neural;
        if let Some(e) = &self.effnet {
            e.set_device(device);
        }
        if let Some(s) = &self.shared {
            *s.neural.lock() = neural;
        }
        self.kick();
    }

    /// Loads the index and starts analysing after `start` (playback and the
    /// first screens go first); `idle` is how often the library is looked
    /// at again when there is nothing to do; `models` is where the neural
    /// model lives.
    pub(crate) fn start(&mut self, lib: Library, root: PathBuf, models: Option<PathBuf>, start: Duration, idle: Duration) {
        if self.shared.is_some() {
            return;
        }
        if self.effnet.is_none() {
            self.effnet = models.map(|m| Arc::new(Effnet::new(m)));
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
            model_sent: Mutex::new(None),
            idle,
            playing: AtomicBool::new(false),
            effnet: self.effnet.clone(),
            neural: Mutex::new(self.neural),
            embeddings_loaded: AtomicBool::new(false),
        });
        let (alive, gone) = crossbeam_channel::bounded::<()>(0);
        let s = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("nikgob-analysis".into())
            .spawn(move || s.run(start, alive));
        match spawned {
            Ok(_) => {
                self.shared = Some(shared);
                self.gone = Some(gone);
            }
            Err(e) => log::warn!("analysis: cannot start: {e}"),
        }
    }

    /// Whether music plays (said in the log).
    pub(crate) fn set_playing(&self, playing: bool) {
        if let Some(s) = &self.shared {
            s.playing.store(playing, Ordering::Relaxed);
        }
    }

    /// New files may be waiting (an import landed).
    pub(crate) fn kick(&self) {
        if let Some(s) = &self.shared {
            s.queue.lock().kicked = true;
            s.wake.notify_all();
        }
    }

    /// Stops the thread: a decode stops at its next chunk, bliss finishes
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
            log::warn!("analysis: the thread is still busy at shutdown");
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
        self.analysis.start(self.lib.clone(), root, self.models_dir.clone(), start, idle);
        self.sound_neural_prefs();
    }

    /// «Нейросетевой анализ звука» and «На чём считать анализ звука» to the
    /// analysis. Before the user picks a mode it is the full one when the
    /// model is on disk already, and that is written into the prefs so the
    /// settings show what runs (never a download without a choice).
    pub(crate) fn sound_neural_prefs(&mut self) {
        let p = self.state.read().prefs.clone();
        let ready = self.analysis.model_ready(self.models_dir.as_deref());
        let mode = p.sound_neural_or(ready);
        if p.sound_neural.is_none() && mode != v::SoundNeural::Off {
            self.publish(|s| s.prefs.sound_neural = Some(mode));
            self.save_prefs();
        }
        let neural = Neural::of(mode);
        if neural != self.analysis.neural() {
            log::info!("analysis: the sound model hears {}", match mode {
                v::SoundNeural::Off => "nothing",
                v::SoundNeural::Library => "the library",
                v::SoundNeural::LibraryAndPreviews => "the library and previews",
            });
        }
        self.analysis.set_neural(neural, p.sound_device);
    }

    pub(crate) fn analysis_msg(&mut self, m: Msg) {
        match m {
            Msg::Progress(p) => {
                if self.state.read().analysis != p {
                    self.publish(|s| s.analysis = p);
                }
                // The sound vectors are in memory by the first report: the
                // run's first mixes need not wait for the first tick.
                self.recs_first_build();
            }
            Msg::SoundModel(m) => {
                if self.state.read().sound_model != m {
                    self.publish(|s| s.sound_model = m);
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

    fn bliss(path: &Path, pacer: &mut Pacer<'_>) -> Result<Vector, FileError> {
        analyze_file(path, Want::BLISS, pacer).map(|out| out.bliss.unwrap().unwrap())
    }

    #[test]
    fn a_wav_file_analyses_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let low = dir.path().join("low.wav");
        let high = dir.path().join("high.wav");
        write_wav(&low, 44_100, 6.0, 220.0);
        write_wav(&high, 48_000, 6.0, 1760.0);
        let stop = AtomicBool::new(false);
        let a = bliss(&low, &mut Pacer::unpaced(&stop)).unwrap();
        let b = bliss(&high, &mut Pacer::unpaced(&stop)).unwrap();
        assert!(a.iter().chain(&b).all(|x| x.is_finite()));
        assert_ne!(a, b);
        assert_eq!(bliss(&low, &mut Pacer::unpaced(&stop)).unwrap(), a, "deterministic");

        // Not a sound: marked as undecodable; missing: only unavailable.
        let junk = dir.path().join("junk.flac");
        std::fs::write(&junk, b"not audio at all").unwrap();
        assert!(matches!(bliss(&junk, &mut Pacer::unpaced(&stop)), Err(FileError::Undecodable(_))));
        assert!(matches!(bliss(&dir.path().join("gone.opus"), &mut Pacer::unpaced(&stop)), Err(FileError::Unavailable(_))));
        // Too short to say anything.
        let blip = dir.path().join("blip.wav");
        write_wav(&blip, 44_100, 0.3, 440.0);
        assert!(matches!(bliss(&blip, &mut Pacer::unpaced(&stop)), Err(FileError::Undecodable(_))));
        stop.store(true, Ordering::Relaxed);
        assert!(matches!(bliss(&low, &mut Pacer::unpaced(&stop)), Err(FileError::Stopped)));
    }

    /// A long file is read only in its three windows: 45 s of audio reach
    /// the analysis whatever its length; a model gets them too.
    #[test]
    fn a_long_file_is_read_in_three_windows() {
        struct Count(Mutex<Vec<usize>>);
        impl SoundModel for Count {
            fn name(&self) -> &str {
                "count"
            }
            fn dim(&self) -> usize {
                1
            }
            fn sample_rate(&self) -> u32 {
                16_000
            }
            fn embed(&self, mono: &[f32]) -> Result<Vec<f32>, AnalysisError> {
                self.0.lock().push(mono.len());
                Ok(vec![1.0])
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let long = dir.path().join("long.wav");
        write_wav(&long, 22_050, 200.0, 330.0);
        let stop = AtomicBool::new(false);
        let model = Count(Mutex::new(Vec::new()));
        let out = analyze_file(&long, Want { bliss: true, model: Some(&model) }, &mut Pacer::unpaced(&stop)).unwrap();
        assert!((out.seconds - 45.0).abs() < 0.1, "{}", out.seconds);
        assert!(out.bliss.unwrap().is_ok());
        assert_eq!(out.embedding.unwrap().unwrap(), vec![1.0]);
        let windows = model.0.lock().clone();
        assert_eq!(windows.len(), 3, "{windows:?}");
        assert!(windows.iter().all(|n| n.abs_diff(15 * 16_000) < 400), "{windows:?}");
        // Only the model: bliss is left alone.
        let out = analyze_file(&long, Want { bliss: false, model: Some(&model) }, &mut Pacer::unpaced(&stop)).unwrap();
        assert!(out.bliss.is_none() && out.embedding.is_some());
    }

    /// Files set aside at the head of the queue (they could not be opened)
    /// never hide the rest: the read goes past them.
    #[test]
    fn the_queue_reads_past_files_set_aside() {
        let todo = |n: u32| AnalysisTodo {
            track_id: TrackId::new(),
            source_id: nikgob_core::model::SourceId::new(),
            locator: format!("{n}.opus"),
            file_key: String::new(),
        };
        let queue: Vec<AnalysisTodo> = (0..100).map(todo).collect();
        let read = |limit: u32| Ok(queue.iter().take(limit as usize).cloned().collect::<Vec<_>>());
        // The whole first batch set aside.
        let aside: HashSet<TrackId> = queue.iter().take(BATCH as usize).map(|t| t.track_id).collect();
        let got = fresh(read, &aside).unwrap();
        assert_eq!(got.len(), BATCH as usize);
        assert_eq!(got[0].track_id, queue[BATCH as usize].track_id);
        assert!(got.iter().all(|t| !aside.contains(&t.track_id)));
        // Nothing left but what is set aside: empty, so the model's queue
        // (and its download) come next.
        let all: HashSet<TrackId> = queue.iter().map(|t| t.track_id).collect();
        assert!(fresh(read, &all).unwrap().is_empty());
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

        // The model's vector goes beside it, and comes back after a restart.
        let raw: Vec<f32> = (0..1280).map(|i| (i as f32 * 0.37).sin()).collect();
        assert_eq!(store_embedding(&lib, &index, t, &Ok(raw.clone()), AnalysisSource::File).unwrap(), Some(true));
        assert!(index.read().embedding(t).is_some());
        load_embeddings(&lib, &other);
        assert_eq!(other.read().embedding(t), index.read().embedding(t));
        // A failing model stores nothing; a track it cannot hear is marked.
        let u = lib.upsert_track(&nikgob_core::model::Track::new("U", nikgob_core::model::ArtistCredit::single(String::from("A"), None))).unwrap().id;
        assert_eq!(store_embedding(&lib, &index, u, &Err(AnalysisError::Model("x".into())), AnalysisSource::File).unwrap(), None);
        assert!(!lib.has_embedding(u, EMBEDDED_WITH).unwrap());
        assert_eq!(store_embedding(&lib, &index, u, &Err(AnalysisError::TooShort), AnalysisSource::File).unwrap(), Some(true));
        assert!(lib.has_embedding(u, EMBEDDED_WITH).unwrap());
    }
}
