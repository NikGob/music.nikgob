//! The import pipeline: probe, fingerprint, identify, quality
//! check, decide, convert, tag, place, derive, register.
//!
//! * Heavy work (decoding, encoding, hashing) runs on blocking threads at the
//!   lowest priority, at most [`ImportConfig::max_parallel`] files at a time.
//! * Everything that decides and writes (find the track, trash the replaced
//!   file, rename into place, library rows) happens inside one critical section
//!   per file, and the decision is made twice: before converting, to skip
//!   pointless work, and again under the lock, so two copies of one song
//!   imported at once cannot both become tracks.
//! * Nothing online ever blocks an import: timeouts and errors mean "tags only".

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use nikgob_core::cover_frame;
use nikgob_core::model::{
    Codec, Master, Palette, ReleaseId, Source, SourceId, SourceKind, SourceOrigin, Track, TrackId, quality_score,
};
use nikgob_library::Library;
use nikgob_metadata::Metadata;
use serde::Serialize;
use tokio::sync::{Semaphore, broadcast, mpsc};

use crate::analysis::{AudioAnalysis, analyze_file};
use crate::config::ImportConfig;
use crate::cover::{ProcessedCover, derive_visuals, process_cover};
use crate::cue_import::{self, Image, Segment};
use crate::album::{Evidence, OnRelease, Reference, lookup_album, names_of};
use crate::confirm::{Choice, Payload, TagView};
use crate::decide::{Decision, ImportTargets, Incoming, decide, find_existing, stream_doubt};
use crate::encode::{EncodeOptions, Encoder, RopusEncoder};
use crate::error::{ImportError, Result};
use crate::events::{ConfirmItem, ImportEvent, ItemId, JobId, Stage};
use crate::folder::{Anchor, Member, album_group_dir, plan};
use crate::fsutil;
use crate::identity::{
    Identity, Lookup, apply_candidate, evidence, identity_from_tags, identity_of_track,
    lookup as online_lookup,
};
use crate::layout::LayoutInput;
use crate::master_files;
use crate::same_audio::{Heard, Judgement, Known, file_keys, judge, marked_before, print_file};
use crate::sanitize::long_path;
use crate::spectrum::Verdict;
use crate::quality::{Mp3Encoder, check_of, file_stamp};
use crate::store::Store;
use crate::tags::{self, FileInfo, TagSet};
use crate::trash::trash_file;

mod confirm;
mod cover;
mod commit;
mod masters;

use commit::{
    Committed, analyze, analyze_track, audio_check, commit, convert, decide_for, incoming, quality_line,
};
use masters::{Undo, register_master, roll_back};
pub use masters::{RelocateReport, master_writes, relocate_masters};

#[cfg(test)]
use commit::Replaced;
#[cfg(test)]
use cover::{cut_bars, large_web_cover};

/// What happened to one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileOutcome {
    Imported { track_id: TrackId, upgraded: bool },
    Duplicate { kept: TrackId },
    /// Waiting in the confirmation queue.
    Parked(ItemId),
    Failed,
    /// Already being handled (or the job was cancelled).
    Skipped,
    /// None of the tracks a better copy was fetched for: not imported.
    LeftOut,
}

/// A running import: progress events arrive on `events`.
pub struct ImportJob {
    pub id: JobId,
    pub events: mpsc::UnboundedReceiver<ImportEvent>,
}

impl ImportJob {
    pub async fn next(&mut self) -> Option<ImportEvent> {
        self.events.recv().await
    }

    /// Waits for `Finished` and returns every event of the job.
    pub async fn wait(mut self) -> Vec<ImportEvent> {
        let mut all = Vec::new();
        while let Some(ev) = self.events.recv().await {
            let done = matches!(ev, ImportEvent::Finished { .. });
            all.push(ev);
            if done {
                break;
            }
        }
        all
    }
}

#[derive(Clone)]
struct Ctx {
    job: JobId,
    tx: mpsc::UnboundedSender<ImportEvent>,
    events: broadcast::Sender<ImportEvent>,
    /// The tracks the job's files are meant for.
    targets: Arc<ImportTargets>,
    /// When the job started: a file the library had before it is settled.
    started: i64,
    /// The tracks cut out of single-file rips (see [`crate::cue_import`]).
    cue: Arc<Mutex<CueJob>>,
}

/// What a job cut out of its single-file rips.
#[derive(Default)]
struct CueJob {
    /// Each track file and what it is.
    segments: HashMap<PathBuf, Arc<Segment>>,
    /// Track files that turned out to be duplicates.
    duplicates: HashSet<PathBuf>,
}

impl Ctx {
    fn segment(&self, path: &Path) -> Option<Arc<Segment>> {
        lock(&self.cue).segments.get(path).cloned()
    }

    fn emit(&self, ev: ImportEvent) {
        let _ = self.tx.send(ev.clone());
        let _ = self.events.send(ev);
    }

    fn stage(&self, path: &Path, stage: Stage) {
        self.emit(ImportEvent::FileProgress {
            job: self.job,
            path: path.to_path_buf(),
            stage,
        });
    }
}

/// Everything learned about a file before it is identified.
#[derive(Clone)]
struct Analyzed {
    path: PathBuf,
    origin: SourceOrigin,
    tags: TagSet,
    info: FileInfo,
    audio: AudioAnalysis,
    fake_lossless: bool,
    score: i32,
    /// The job's targets (see [`ImportTargets`]).
    targets: Arc<ImportTargets>,
    /// The track this file is, whatever the tags say: the job's forced
    /// target, or the stream track the user confirmed.
    force_track: Option<TrackId>,
    /// BLAKE3 of the file as it came.
    came_hash: Option<String>,
    /// When its job started ([`Ctx::started`]).
    job_started: i64,
    /// A track cut out of a single-file rip: the image is its master.
    cue: Option<Arc<Segment>>,
}

impl Analyzed {
    /// The lossless file the track's master is: the image a cut track came
    /// from, else the file itself.
    fn master_path(&self) -> &Path {
        self.cue.as_ref().map_or(&self.path, |s| &s.image.path)
    }
}

/// A file that is read (tags, length, fingerprint) and waits for its folder.
struct Pre {
    analyzed: Analyzed,
    /// What the tags (or the track the job says it is) say.
    base: Identity,
    /// The library knows which track it is: its own id, the job says so, or
    /// it is one of the tracks the job is for.
    known: bool,
    /// Its tags carry the recording and release MBIDs already.
    fully_tagged: bool,
}

impl Pre {
    fn duration_ms(&self) -> Option<u32> {
        (!self.analyzed.info.duration.is_zero()).then(|| self.analyzed.info.duration_ms())
    }
}

enum Read {
    Ready(Box<Pre>),
    Done(FileOutcome),
}

/// A file that is read and identified and waits for its folder's other files.
struct Ready {
    analyzed: Analyzed,
    /// What the tags alone say.
    base: Identity,
    identity: Identity,
}

/// A file in «Подтверди».
struct Parked {
    /// The job it came from; `None` for one parked before a restart.
    ctx: Option<Ctx>,
    /// When that job started ([`Ctx::started`]), kept across a restart.
    job_started: i64,
    origin: SourceOrigin,
    targets: Arc<ImportTargets>,
    /// The file as read and what it was taken for; `None` after a restart
    /// (it is read again).
    read: Option<(Analyzed, Identity)>,
    /// What the options do, in the order of `item.options`.
    choices: Vec<Choice>,
    item: ConfirmItem,
}

pub(crate) struct Inner {
    pub cfg: ImportConfig,
    pub library: Library,
    pub metadata: Option<Metadata>,
    encoder: Arc<dyn Encoder>,
    next_job: AtomicU64,
    next_item: AtomicU64,
    pub events: broadcast::Sender<ImportEvent>,
    pending: Mutex<HashMap<ItemId, Parked>>,
    active: Mutex<HashSet<PathBuf>>,
    pub(crate) store: Mutex<Store>,
    permits: Arc<Semaphore>,
    /// Jobs the user cancelled: no file of theirs is committed any more.
    cancelled: Mutex<HashSet<u64>>,
    /// What each running job committed, so a cancel takes it back.
    journal: Mutex<HashMap<u64, Vec<Undo>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Lowers the priority of the current thread for the duration of a scope.
struct LowPriority(Option<thread_priority::ThreadPriority>);

impl LowPriority {
    fn enter() -> Self {
        let prev = thread_priority::get_current_thread_priority().ok();
        let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
        Self(prev)
    }
}

impl Drop for LowPriority {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = thread_priority::set_current_thread_priority(p);
        }
    }
}

/// Runs `f` on a blocking thread with low priority.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(move || {
        let _low = LowPriority::enter();
        f()
    })
    .await
    .map_err(|e| ImportError::Other(format!("worker failed: {e}")))?
}

/// The importer. Cheap to clone.
#[derive(Clone)]
pub struct Importer {
    pub(crate) inner: Arc<Inner>,
}

impl Importer {
    /// Uses the in-process Opus encoder.
    pub fn new(cfg: ImportConfig, library: Library, metadata: Option<Metadata>) -> Self {
        Self::with_encoder(cfg, library, metadata, Arc::new(RopusEncoder))
    }

    /// Swap the Opus encoder (for example [`crate::encode::FfmpegEncoder`]).
    pub fn with_encoder(
        cfg: ImportConfig,
        library: Library,
        metadata: Option<Metadata>,
        encoder: Arc<dyn Encoder>,
    ) -> Self {
        let (events, _) = broadcast::channel(1024);
        let permits = Arc::new(Semaphore::new(cfg.max_parallel.max(1)));
        let this = Self {
            inner: Arc::new(Inner {
                store: Mutex::new(Store::new(library.clone())),
                cfg,
                library,
                metadata,
                encoder,
                next_job: AtomicU64::new(1),
                // Items not stored in the database (it failed) count from
                // far above its row ids.
                next_item: AtomicU64::new(1 << 40),
                events,
                pending: Mutex::new(HashMap::new()),
                active: Mutex::new(HashSet::new()),
                permits,
                cancelled: Mutex::new(HashSet::new()),
                journal: Mutex::new(HashMap::new()),
            }),
        };
        this.restore_pending();
        this
    }

    pub fn config(&self) -> &ImportConfig {
        &self.inner.cfg
    }

    pub fn library(&self) -> &Library {
        &self.inner.library
    }

    /// Name of the Opus encoder in use.
    pub fn encoder_name(&self) -> &'static str {
        self.inner.encoder.name()
    }

    /// Every event of every job (and the Inbox watcher), for the runtime and UI.
    pub fn subscribe(&self) -> broadcast::Receiver<ImportEvent> {
        self.inner.events.subscribe()
    }

    /// Files waiting for the user.
    pub fn pending(&self) -> Vec<ConfirmItem> {
        let mut items: Vec<_> = lock(&self.inner.pending)
            .values()
            .map(|p| p.item.clone())
            .collect();
        items.sort_by_key(|i| i.id);
        items
    }

    /// Imports audio files (directories are scanned recursively). Must be called
    /// inside a tokio runtime. `.torrent` files among the paths are reported as
    /// [`ImportEvent::TorrentFound`].
    pub fn import_paths(&self, paths: Vec<PathBuf>, origin: SourceOrigin) -> ImportJob {
        self.import_paths_for(paths, origin, ImportTargets::default())
    }

    /// [`Self::import_paths`] for files meant for `targets` (a wishlist
    /// download): a file that fits one of them becomes its local source.
    /// [`SourceOrigin::Stream`] marks audio saved from a stream: kept as it
    /// is (never converted, no master) with the low
    /// [`nikgob_core::model::score::SAVED_STREAM`] score, so a real download
    /// upgrades it later.
    pub fn import_paths_for(
        &self,
        paths: Vec<PathBuf>,
        origin: SourceOrigin,
        targets: ImportTargets,
    ) -> ImportJob {
        let id = JobId(self.inner.next_job.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = Ctx {
            job: id,
            tx,
            events: self.inner.events.clone(),
            targets: Arc::new(targets),
            started: nikgob_library::now_ms(),
            cue: Arc::default(),
        };
        let this = self.clone();
        tokio::spawn(async move { this.run_job(ctx, paths, origin).await });
        ImportJob { id, events: rx }
    }

    /// Stops a running job: no more of its files are committed, and what
    /// it committed already is taken back once it stops (the files to the
    /// Trash, the rows out, a replaced file back in place).
    pub fn cancel(&self, job: JobId) {
        if lock(&self.inner.journal).contains_key(&job.0) {
            log::info!("import job {} cancelled", job.0);
            lock(&self.inner.cancelled).insert(job.0);
        }
    }

    fn is_cancelled(&self, job: JobId) -> bool {
        lock(&self.inner.cancelled).contains(&job.0)
    }

    async fn run_job(self, ctx: Ctx, paths: Vec<PathBuf>, origin: SourceOrigin) {
        lock(&self.inner.journal).insert(ctx.job.0, Vec::new());
        // Single-file rips with a cue sheet are cut into their tracks
        // folder by folder below; the rest imports as it is.
        let scanned = tokio::task::spawn_blocking(move || {
            let found = fsutil::scan_import(&paths);
            let mut given = found.audio;
            given.extend(found.images);
            (cue_import::plan(given, &found.cues), found.torrents)
        })
        .await;
        let (planned, torrents) = scanned.unwrap_or_default();
        let files = planned.files;
        // An image another job is cutting is left to it.
        let images: Vec<Image> = {
            let mut active = lock(&self.inner.active);
            planned.images
                .into_iter()
                .filter(|i| {
                    let free = active.insert(i.path.clone());
                    if !free {
                        log::info!("import: {} is being imported already", i.path.display());
                    }
                    free
                })
                .collect()
        };
        let work = cue_import::job_dir(&self.inner.cfg.work_dir(), ctx.job.0, ctx.started);
        ctx.emit(ImportEvent::Started {
            job: ctx.job,
            files: files.len() + images.iter().map(|i| i.tracks().len()).sum::<usize>(),
        });
        for t in torrents {
            ctx.emit(ImportEvent::TorrentFound(t));
        }
        // The «Хочу скачать» release the job is for, if any.
        let reference = if ctx.targets.tracks.is_empty() && ctx.targets.release.is_none() {
            None
        } else {
            let (lib, targets) = (self.inner.library.clone(), ctx.targets.clone());
            tokio::task::spawn_blocking(move || Reference::load(&lib, &targets))
                .await
                .ok()
                .flatten()
        };
        // One album folder at a time: read all its files, identify them as
        // one album, agree on a single release, then import them.
        let mut folders: BTreeMap<PathBuf, (Vec<PathBuf>, Vec<Image>)> = BTreeMap::new();
        for path in files {
            folders.entry(album_group_dir(&path)).or_default().0.push(path);
        }
        for image in images {
            folders.entry(album_group_dir(&image.path)).or_default().1.push(image);
        }
        let (mut imported, mut duplicates, mut failed, mut pending, mut left_out) = (0, 0, 0, 0, 0);
        let mut cut_no = 0usize;
        for (dir, (mut files, images)) in folders {
            if self.is_cancelled(ctx.job) {
                let mut active = lock(&self.inner.active);
                for i in &images {
                    active.remove(&i.path);
                }
                continue;
            }
            // The folder's images become their tracks' files.
            let mut cut: Vec<(Image, PathBuf, Vec<PathBuf>, String)> = Vec::new();
            for image in images {
                cut_no += 1;
                let at = work.join(format!("{cut_no:03}"));
                ctx.stage(&image.path, Stage::Probing);
                let (img, d, check) = (image.clone(), at.clone(), self.inner.cfg.check_fake_lossless);
                match blocking(move || cue_import::cut(&img, &d, check)).await {
                    Ok(tracks) => {
                        let paths: Vec<PathBuf> = tracks.iter().map(|(p, _)| p.clone()).collect();
                        let hash = tracks.first().map(|(_, s)| s.image.hash.clone()).unwrap_or_default();
                        lock(&ctx.cue).segments.extend(tracks);
                        files.extend(paths.iter().cloned());
                        cut.push((image, at, paths, hash));
                    }
                    Err(e) => {
                        lock(&self.inner.active).remove(&image.path);
                        if image.importable_whole() {
                            log::warn!(
                                "cue {}: {} is not cut ({e}); it imports as one file",
                                image.cue.display(),
                                image.path.display()
                            );
                            files.push(image.path.clone());
                        } else {
                            self.failed(&ctx, &image.path, &e);
                            failed += 1;
                        }
                    }
                }
            }
            let mut set = tokio::task::JoinSet::new();
            for path in files {
                let Ok(permit) = self.inner.permits.clone().acquire_owned().await else {
                    break;
                };
                let (this, ctx, origin) = (self.clone(), ctx.clone(), origin.clone());
                set.spawn(async move {
                    let read = this.read_file(&ctx, path, origin).await;
                    drop(permit);
                    read
                });
            }
            let mut outcomes = Vec::new();
            let mut read = Vec::new();
            while let Some(res) = set.join_next().await {
                match res {
                    Ok(Read::Ready(r)) => read.push(*r),
                    Ok(Read::Done(o)) => outcomes.push(o),
                    Err(_) => failed += 1,
                }
            }
            // Task completion order is arbitrary; the folder's vote must not be.
            read.sort_by(|a, b| a.analyzed.path.cmp(&b.analyzed.path));
            let (ready, done, settled) = self.identify_folder(&ctx, &dir, read, reference.as_ref()).await;
            outcomes.extend(done);
            let members: Vec<_> = ready
                .iter()
                .map(|r| Member {
                    id: &r.identity,
                    base: &r.base,
                })
                .collect();
            // A folder already placed on one release (the wishlist's) needs
            // no vote.
            let anchor = if settled { None } else { plan(&members).map(Arc::new) };
            drop(members);
            let mut set = tokio::task::JoinSet::new();
            for r in ready {
                let Ok(permit) = self.inner.permits.clone().acquire_owned().await else {
                    break;
                };
                let (this, ctx, anchor) = (self.clone(), ctx.clone(), anchor.clone());
                set.spawn(async move {
                    let outcome = this.complete_file(&ctx, *r, anchor.as_deref()).await;
                    drop(permit);
                    outcome
                });
            }
            while let Some(res) = set.join_next().await {
                match res {
                    Ok(o) => outcomes.push(o),
                    Err(_) => failed += 1,
                }
            }
            for o in outcomes {
                match o {
                    FileOutcome::Imported { .. } => imported += 1,
                    FileOutcome::Duplicate { .. } => duplicates += 1,
                    FileOutcome::Parked(_) => pending += 1,
                    FileOutcome::Failed => failed += 1,
                    FileOutcome::LeftOut => left_out += 1,
                    FileOutcome::Skipped => {}
                }
            }
            if !cut.is_empty() {
                let (this, c) = (self.clone(), ctx.clone());
                let _ = blocking(move || {
                    this.after_cut(&c, cut);
                    Ok(())
                })
                .await;
            }
        }
        let _ = std::fs::remove_dir_all(long_path(&work));
        let journal = lock(&self.inner.journal).remove(&ctx.job.0).unwrap_or_default();
        let cancelled = lock(&self.inner.cancelled).remove(&ctx.job.0);
        if cancelled {
            let inner = self.inner.clone();
            let undone = tokio::task::spawn_blocking(move || roll_back(&inner, journal)).await.unwrap_or(0);
            log::info!("import job {}: cancelled, {undone} files taken back", ctx.job.0);
            imported = 0;
        }
        ctx.emit(ImportEvent::Finished {
            job: ctx.job,
            imported,
            duplicates,
            failed,
            pending,
            left_out,
            cancelled,
        });
    }

    /// A folder's images are done: their track files go, and an image of
    /// the Inbox whose every track was a duplicate goes to the Trash with
    /// its sheet (as a duplicate file does). The images are free for other
    /// jobs again.
    ///
    /// A sheet in the Inbox goes once every image of it is kept as a master
    /// with its own copy of the sheet (or went to the Trash: then the sheet
    /// does too); while any image of it stays, the sheet stays.
    fn after_cut(&self, ctx: &Ctx, cut: Vec<(Image, PathBuf, Vec<PathBuf>, String)>) {
        let cfg = &self.inner.cfg;
        let mut sheets: BTreeMap<PathBuf, (usize, Vec<cue_import::ImageEnd>)> = BTreeMap::new();
        for (image, dir, paths, hash) in cut {
            let all_duplicates = {
                let mut job = lock(&ctx.cue);
                let all = paths.iter().all(|p| job.duplicates.contains(p));
                for p in &paths {
                    job.segments.remove(p);
                    job.duplicates.remove(p);
                }
                all
            };
            let _ = std::fs::remove_dir_all(long_path(&dir));
            if image.path.starts_with(&cfg.inbox_dir) {
                use cue_import::ImageEnd;
                let end = if long_path(&image.path).exists() {
                    if all_duplicates {
                        match trash_file(&cfg.trash_dir, &image.path) {
                            Ok(to) => {
                                log::info!("import: {} is a duplicate, in the Trash at {}", image.path.display(), to.display());
                                ImageEnd::Trashed
                            }
                            Err(e) => {
                                log::warn!("cannot trash duplicate {}: {e}", image.path.display());
                                ImageEnd::Stayed
                            }
                        }
                    } else {
                        ImageEnd::Stayed
                    }
                } else if long_path(&cfg.masters_dir().join(format!("{hash}.cue"))).exists() {
                    ImageEnd::Kept
                } else {
                    ImageEnd::Stayed
                };
                let images = image.sheet.files.iter().filter(|f| f.tracks.len() >= 2).count();
                sheets.entry(image.cue.clone()).or_insert((images, Vec::new())).1.push(end);
            }
            lock(&self.inner.active).remove(&image.path);
        }
        for (sheet, (images, ends)) in sheets {
            // Every image of the sheet must have been this folder's.
            if ends.len() < images || !long_path(&sheet).exists() {
                continue;
            }
            match cue_import::sheet_done(&ends) {
                Some(true) => match trash_file(&cfg.trash_dir, &sheet) {
                    Ok(to) => log::info!("import: {} is a duplicate, in the Trash at {}", sheet.display(), to.display()),
                    Err(e) => log::warn!("cannot trash duplicate {}: {e}", sheet.display()),
                },
                Some(false) => {
                    if let Err(e) = fsutil::remove_file(&sheet) {
                        log::warn!("cannot remove imported {}: {e}", sheet.display());
                    }
                }
                None => {}
            }
        }
    }

    /// Reads one file; never returns an error, failures become events.
    async fn read_file(&self, ctx: &Ctx, path: PathBuf, origin: SourceOrigin) -> Read {
        if !lock(&self.inner.active).insert(path.clone()) {
            return Read::Done(FileOutcome::Skipped);
        }
        match self.read(ctx, &path, origin).await {
            Ok(Read::Done(o)) => {
                lock(&self.inner.active).remove(&path);
                Read::Done(o)
            }
            Ok(r) => r,
            Err(e) => {
                lock(&self.inner.active).remove(&path);
                Read::Done(self.failed(ctx, &path, &e))
            }
        }
    }

    /// Imports an identified file under the folder's agreed release.
    async fn complete_file(&self, ctx: &Ctx, ready: Ready, anchor: Option<&Anchor>) -> FileOutcome {
        let Ready {
            analyzed,
            base,
            mut identity,
        } = ready;
        if self.is_cancelled(ctx.job) {
            lock(&self.inner.active).remove(&analyzed.path);
            return FileOutcome::Skipped;
        }
        if let Some(a) = anchor
            && a.fits(&identity, &base)
        {
            a.apply(&mut identity, &base);
        }
        let path = analyzed.path.clone();
        let outcome = match self.finish_import(ctx, analyzed, identity).await {
            Ok(o) => o,
            Err(e) => self.failed(ctx, &path, &e),
        };
        lock(&self.inner.active).remove(&path);
        outcome
    }

    fn failed(&self, ctx: &Ctx, path: &Path, e: &ImportError) -> FileOutcome {
        log::warn!("import of {} failed: {e}", path.display());
        if let ImportError::WrongAudio { track, title, sounds_like } = e {
            ctx.emit(ImportEvent::WrongAudio {
                job: ctx.job,
                path: path.to_path_buf(),
                track: *track,
                title: title.clone(),
                sounds_like: sounds_like.clone(),
            });
        }
        ctx.emit(ImportEvent::Failed {
            job: ctx.job,
            path: path.to_path_buf(),
            error: e.to_string(),
        });
        FileOutcome::Failed
    }

    fn file_origin(&self, origin: &SourceOrigin, path: &Path) -> SourceOrigin {
        match origin {
            SourceOrigin::Torrent { info_hash, .. } => {
                let rel = path
                    .strip_prefix(&self.inner.cfg.downloads_dir)
                    .ok()
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|| {
                        path.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    });
                SourceOrigin::Torrent {
                    info_hash: info_hash.clone(),
                    path_in_torrent: rel,
                }
            }
            other => other.clone(),
        }
    }

    /// Steps 1..2: probe, fingerprint, what the tags say.
    async fn read(&self, ctx: &Ctx, path: &Path, origin: SourceOrigin) -> Result<Read> {
        let inner = self.inner.clone();
        ctx.stage(path, Stage::Probing);
        let segment = ctx.segment(path);
        // A track cut out of an image came from where the image did.
        let origin = self.file_origin(&origin, segment.as_ref().map_or(path, |s| &s.image.path));
        ctx.stage(path, Stage::Fingerprinting);
        let mut analyzed = {
            let (p, o, i) = (path.to_path_buf(), origin, inner.clone());
            blocking(move || analyze_track(&i.cfg, &p, o, segment)).await?
        };
        analyzed.force_track = ctx.targets.track_of(path);
        analyzed.targets = ctx.targets.clone();
        analyzed.job_started = ctx.started;
        ctx.stage(path, Stage::QualityCheck);

        if path.starts_with(&inner.cfg.library_dir) {
            return match analyzed
                .tags
                .track_id
                .and_then(|t| inner.library.get_track(t).ok().flatten())
            {
                Some(t) => {
                    ctx.emit(ImportEvent::Duplicate {
                        job: ctx.job,
                        path: path.to_path_buf(),
                        kept: t.id,
                        quality: None,
                    });
                    Ok(Read::Done(FileOutcome::Duplicate { kept: t.id }))
                }
                None => Err(ImportError::Other(
                    "file is inside the library folder; use the library rebuild instead".into(),
                )),
            };
        }

        let mut base = identity_from_tags(&analyzed.tags, path);
        // A file the user (or the stream it was saved from) says is a given
        // track takes that track's metadata: its own tags may say nothing.
        if let Some(t) = analyzed.force_track
            && let Some(id) = identity_of_track(&inner.library, t, &base)
        {
            base = id;
        }
        let known = analyzed.force_track.is_some()
            || analyzed
                .tags
                .track_id
                .is_some_and(|t| inner.library.get_track(t).ok().flatten().is_some());
        let fully_tagged = analyzed.tags.recording_mbid.is_some() && analyzed.tags.release_mbid.is_some();
        Ok(Read::Ready(Box::new(Pre {
            analyzed,
            base,
            known,
            fully_tagged,
        })))
    }

    /// Step 3 for a folder (see [`crate::album`]): the tracks the job is
    /// for, the job's release, the album as a whole, and only then file by
    /// file. Returns the identified files, the parked ones and whether the
    /// folder already sits on one release.
    async fn identify_folder(
        &self,
        ctx: &Ctx,
        dir: &Path,
        mut read: Vec<Pre>,
        reference: Option<&Reference>,
    ) -> (Vec<Box<Ready>>, Vec<FileOutcome>, bool) {
        let inner = self.inner.clone();
        let torrent_path = read.iter().find_map(|p| match &p.analyzed.origin {
            SourceOrigin::Torrent { path_in_torrent, .. } => Some(path_in_torrent.clone()),
            _ => None,
        });
        let names = names_of(dir, torrent_path.as_deref());
        let ev = {
            let bases: Vec<&Identity> = read.iter().map(|p| &p.base).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            Evidence::new(&bases, &names)
        };
        let mut ids: Vec<Option<Identity>> = vec![None; read.len()];
        let mut settled = false;
        let mut parked = Vec::new();
        // The tracks the job is for: by number, title and length, each
        // track for one file.
        if let Some(r) = reference {
            let files: Vec<Option<(&Identity, Option<u32>)>> =
                read.iter().map(|p| (!p.known).then(|| (&p.base, p.duration_ms()))).collect();
            let found = r.match_files(&files);
            let matched = found.iter().flatten().count();
            let mut unwanted = Vec::new();
            for (i, p) in read.iter_mut().enumerate() {
                match found[i] {
                    Some(OnRelease::Track(t)) => {
                        p.analyzed.force_track = Some(t);
                        p.known = true;
                        ids[i] = identity_of_track(&inner.library, t, &p.base);
                    }
                    Some(OnRelease::Unwanted) => unwanted.push(i),
                    None => {}
                }
            }
            if r.fits(&ev, matched) {
                // A song the user deleted from the album is not needed.
                for i in unwanted.into_iter().rev() {
                    let p = read.remove(i);
                    ids.remove(i);
                    log::info!("import: {} is a song deleted from «{}», left out", p.analyzed.path.display(), r.release.title);
                    lock(&inner.active).remove(&p.analyzed.path);
                    parked.push(FileOutcome::Skipped);
                }
                // The rest of the folder is more of the release, and so is
                // a track of it that has no place yet: after its tracks, in
                // the folder's order, whatever their own numbers.
                let unplaced = |i: usize| match &ids[i] {
                    None => !read[i].known,
                    Some(id) => read[i].analyzed.force_track.is_some() && id.track_no == 0,
                };
                let mut rest: Vec<usize> = (0..read.len()).filter(|&i| unplaced(i)).collect();
                rest.sort_by(|&a, &b| {
                    let (x, y) = (&read[a].base, &read[b].base);
                    (x.disc_no, x.track_no == 0, x.track_no)
                        .cmp(&(y.disc_no, y.track_no == 0, y.track_no))
                        .then_with(|| read[a].analyzed.path.cmp(&read[b].analyzed.path))
                });
                // A file's own number stays when it is a free place inside
                // the release (a hole its tracklist doesn't fill); a taken
                // or outside one goes after the release's tracks.
                let (disc, mut next) = r.after();
                let mut taken: std::collections::HashSet<(u32, u32)> =
                    r.tracks.iter().map(|t| (t.disc_no.max(1), t.track_no)).collect();
                taken.extend(ids.iter().flatten().map(|id| (id.disc_no.max(1), id.track_no)));
                for i in rest {
                    let own = (read[i].base.disc_no.max(1), read[i].base.track_no);
                    let place = if own.1 > 0 && own.0 == disc && own.1 < next && !taken.contains(&own) {
                        own
                    } else {
                        next += 1;
                        (disc, next - 1)
                    };
                    taken.insert(place);
                    if let Some(id) = ids[i].as_mut() {
                        (id.disc_no, id.track_no) = place;
                    } else {
                        ids[i] = Some(r.identity(&read[i].base, place));
                    }
                }
                settled = true;
            }
        }
        // A better copy of the job's tracks is one of them or nothing: the
        // rest of the folder (another album that came under the same
        // name) is left out, never imported as an album of its own.
        // A job of confirmed files takes those alone, each as its track.
        if ctx.targets.only_targets || !ctx.targets.pinned.is_empty() {
            for i in (0..read.len()).rev() {
                if ctx.targets.unpinned(read[i].analyzed.master_path()) || (ids[i].is_none() && !read[i].known) {
                    let p = read.remove(i);
                    ids.remove(i);
                    log::info!("import: {} is none of the tracks it was fetched for, left out", p.analyzed.path.display());
                    lock(&inner.active).remove(&p.analyzed.path);
                    parked.push(FileOutcome::LeftOut);
                }
            }
            settled = true;
        }
        let open = |ids: &[Option<Identity>], read: &[Pre]| -> Vec<usize> {
            (0..read.len())
                .filter(|&i| ids[i].is_none() && !read[i].known && !read[i].fully_tagged)
                .collect()
        };
        if !settled && ev.is_album() {
            // One album: what the folder says fills what its tags lack, and
            // MusicBrainz is asked for the release as a whole.
            for p in &mut read {
                if !p.known {
                    ev.fill(&mut p.base);
                }
            }
            if let Some(meta) = &inner.metadata
                && !open(&ids, &read).is_empty()
            {
                for p in &read {
                    ctx.stage(&p.analyzed.path, Stage::Identifying);
                }
                let files: Vec<(&Identity, Option<u32>)> = read.iter().map(|p| (&p.base, p.duration_ms())).collect();
                if let Some(found) = lookup_album(meta, &inner.cfg, &ev, &files).await {
                    for i in open(&ids, &read) {
                        ids[i] = found[i].as_ref().map(|c| apply_candidate(&read[i].base, c));
                    }
                }
            }
        } else if !settled && let Some(meta) = &inner.metadata {
            // A compilation or loose files: one by one, strangers vetoed.
            let mut set = tokio::task::JoinSet::new();
            for i in open(&ids, &read) {
                let p = &read[i];
                ctx.stage(&p.analyzed.path, Stage::Identifying);
                let evidence = evidence(&p.analyzed.tags, &p.analyzed.info, &p.analyzed.path);
                let fingerprint = p.analyzed.audio.fingerprint.clone();
                let artists = ev.artists_for(&p.base);
                let (meta, cfg) = (meta.clone(), inner.cfg.clone());
                set.spawn(async move {
                    (i, online_lookup(&meta, &cfg, evidence, fingerprint.as_ref(), &artists).await)
                });
            }
            let mut asked = Vec::new();
            while let Some(res) = set.join_next().await {
                match res {
                    Ok((i, Lookup::Sure(best))) => ids[i] = Some(apply_candidate(&read[i].base, &best)),
                    Ok((i, Lookup::Unsure(options))) => asked.push((i, options)),
                    Ok((_, Lookup::None)) | Err(_) => {}
                }
            }
            asked.sort_by_key(|(i, _)| *i);
            // Highest index first, so the indexes of the rest stay put.
            // A track cut out of an image lives only while its job runs:
            // it is never parked, its sheet's tags stand.
            asked.retain(|(i, _)| read[*i].analyzed.cue.is_none());
            for (i, options) in asked.into_iter().rev() {
                let p = read.remove(i);
                ids.remove(i);
                let mut choices = vec![Choice::Tags];
                choices.extend(options.iter().map(|c| Choice::Candidate(Box::new(c.into()))));
                parked.push(self.park(ctx, p.analyzed, p.base, choices));
            }
        }
        let mut ready = Vec::with_capacity(read.len());
        for (p, id) in read.into_iter().zip(ids) {
            let Pre { analyzed, base, known, .. } = p;
            let identity = id.unwrap_or_else(|| base.clone());
            // A liked stream track of another length (TV size, radio edit)
            // is asked about rather than attached silently.
            if analyzed.force_track.is_none() && !known && analyzed.cue.is_none() {
                let (i, a, id) = (inner.clone(), analyzed.clone(), identity.clone());
                let doubt = blocking(move || {
                    let mut store = lock(&i.store);
                    let inc = incoming(&a, &id);
                    match find_existing(&mut store, &inc)? {
                        Some(_) => Ok(None),
                        None => stream_doubt(&mut store, &inc),
                    }
                })
                .await;
                match doubt {
                    Ok(Some(track)) => {
                        parked.push(self.park_stream(ctx, analyzed, identity, &track));
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        let path = analyzed.path.clone();
                        lock(&inner.active).remove(&path);
                        parked.push(self.failed(ctx, &path, &e));
                        continue;
                    }
                }
            }
            ready.push(Box::new(Ready { analyzed, base, identity }));
        }
        (ready, parked, settled)
    }

    /// Steps 4..9 for an identified file.
    async fn finish_import(
        &self,
        ctx: &Ctx,
        analyzed: Analyzed,
        identity: Identity,
    ) -> Result<FileOutcome> {
        let inner = self.inner.clone();
        let path = analyzed.path.clone();
        ctx.stage(&path, Stage::Deciding);

        // Early decision: a worse duplicate costs nothing more than this.
        let early = {
            let (i, a, id) = (inner.clone(), analyzed.clone(), identity.clone());
            blocking(move || decide_for(&mut lock(&i.store), &a, &id)).await?
        };
        if let Decision::Duplicate(e) = &early {
            let kept = e.track.id;
            return Ok(self.report_duplicate(ctx, &analyzed, kept));
        }
        // Another song's audio is not imported as this one: refused before
        // any work is spent on it (the commit asks again).
        {
            let (i, a, id) = (inner.clone(), analyzed.clone(), identity.clone());
            let track = match &early {
                Decision::Upgrade(e) | Decision::AddSource(e) | Decision::Duplicate(e) => Some(e.track.clone()),
                Decision::New => None,
            };
            blocking(move || {
                let release = match &track {
                    Some(t) => t.release_id,
                    None => lock(&i.store).find_release(&id)?,
                };
                audio_check(&i, &a, &id, track.as_ref(), release, true).map(|_| ())
            })
            .await?;
        }
        // A track of the wished album: its album and place (the commit
        // decides again).
        let identity = match &early {
            Decision::Upgrade(e) | Decision::AddSource(e) => {
                crate::album::keep_place(&inner.library, &analyzed.targets, &e.track, &identity).unwrap_or(identity)
            }
            _ => identity,
        };

        let layout = layout_input(&identity);
        let album_dir = layout.album_dir(&inner.cfg.library_dir);

        let cover = self.prepare_cover(&analyzed, &identity, &album_dir).await;

        if self.is_cancelled(ctx.job) {
            return Ok(FileOutcome::Skipped);
        }
        ctx.stage(&path, Stage::Converting);
        let prepared = {
            let (i, a, dir) = (inner.clone(), analyzed.clone(), album_dir.clone());
            blocking(move || convert(&i, &a, &dir)).await?
        };

        // Cancelled while it converted: the converted copy goes.
        if self.is_cancelled(ctx.job) {
            let _ = fsutil::remove_file(&prepared.tmp);
            if let Some(dir) = prepared.tmp.parent() {
                fsutil::prune_empty_dirs(dir, &inner.cfg.library_dir);
            }
            return Ok(FileOutcome::Skipped);
        }
        ctx.stage(&path, Stage::Tagging);
        let committed = {
            let (i, a, id, c) = (inner.clone(), analyzed.clone(), identity.clone(), cover.clone());
            blocking(move || commit(&i, &a, &id, &prepared, c.as_ref())).await?
        };
        match committed {
            Committed::Duplicate { kept } => Ok(self.report_duplicate(ctx, &analyzed, kept)),
            Committed::Imported(mut done) => {
                // Kept while the job runs: a cancel takes it back.
                if let Some(undo) = done.undo.take()
                    && let Some(list) = lock(&inner.journal).get_mut(&ctx.job.0)
                {
                    list.push(undo);
                }
                ctx.stage(&path, Stage::Deriving);
                // Its row is written with its file ([`register_master`]).
                if let Some(master) = done.master.clone() {
                    ctx.emit(ImportEvent::MasterRegistered(master));
                }
                ctx.emit(ImportEvent::Imported {
                    job: ctx.job,
                    path,
                    track_id: done.track_id,
                    upgraded: done.upgraded,
                });
                Ok(FileOutcome::Imported {
                    track_id: done.track_id,
                    upgraded: done.upgraded,
                })
            }
        }
    }

    /// A worse (or equal) copy: an Inbox file goes to Trash, anything else stays.
    fn report_duplicate(&self, ctx: &Ctx, a: &Analyzed, kept: TrackId) -> FileOutcome {
        let cfg = &self.inner.cfg;
        if a.cue.is_some() {
            lock(&ctx.cue).duplicates.insert(a.path.clone());
        }
        if a.path.starts_with(&cfg.inbox_dir)
            && let Err(e) = trash_file(&cfg.trash_dir, &a.path)
        {
            log::warn!("cannot trash duplicate {}: {e}", a.path.display());
        }
        ctx.emit(ImportEvent::Duplicate {
            job: ctx.job,
            path: a.path.clone(),
            kept,
            quality: Some(quality_line(a)),
        });
        FileOutcome::Duplicate { kept }
    }

}

pub(crate) fn layout_input(id: &Identity) -> LayoutInput {
    LayoutInput {
        album_artist: id.album_artist.clone().unwrap_or_default(),
        artist: id.artist_line.clone(),
        album: id.album.clone(),
        title: id.title.clone(),
        year: id.year,
        disc_no: id.disc_no,
        disc_total: id.disc_total,
        track_no: id.track_no,
        release_type: id.release_type,
        compilation: id.compilation,
    }
}

pub(crate) fn rel_to_library(cfg: &ImportConfig, path: &Path) -> String {
    path.strip_prefix(&cfg.library_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

include!("importer/tests.rs");
