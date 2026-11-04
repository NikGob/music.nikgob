//! The import pipeline (PLAN section 6): probe, fingerprint, identify, quality
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
    Codec, Master, Palette, Source, SourceId, SourceKind, SourceOrigin, Track, TrackId, quality_score,
};
use nikgob_library::Library;
use nikgob_metadata::Metadata;
use serde::Serialize;
use tokio::sync::{Semaphore, broadcast, mpsc};

use crate::analysis::{AudioAnalysis, analyze_file};
use crate::config::ImportConfig;
use crate::cover::{ProcessedCover, derive_visuals, process_cover};
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
use crate::sanitize::long_path;
use crate::spectrum::Verdict;
use crate::quality::{Mp3Encoder, check_of, file_stamp};
use crate::store::Store;
use crate::tags::{self, FileInfo, TagSet};
use crate::trash::trash_file;

const COVER_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart"];
const COVER_FILE: &str = "cover.jpg";
const AMBIENT_FILE: &str = "ambient.png";

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
}

impl Ctx {
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
        let scanned = tokio::task::spawn_blocking(move || fsutil::scan(&paths)).await;
        let (files, torrents) = scanned.unwrap_or_default();
        ctx.emit(ImportEvent::Started {
            job: ctx.job,
            files: files.len(),
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
        let mut folders: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for path in files {
            folders.entry(album_group_dir(&path)).or_default().push(path);
        }
        let (mut imported, mut duplicates, mut failed, mut pending, mut left_out) = (0, 0, 0, 0, 0);
        for (dir, files) in folders {
            if self.is_cancelled(ctx.job) {
                break;
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
        }
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
        let origin = self.file_origin(&origin, path);
        ctx.stage(path, Stage::Fingerprinting);
        let mut analyzed = {
            let (p, o, i) = (path.to_path_buf(), origin, inner.clone());
            blocking(move || analyze(&i.cfg, &p, o)).await?
        };
        analyzed.force_track = ctx.targets.forced();
        analyzed.targets = ctx.targets.clone();
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
        if ctx.targets.only_targets {
            for i in (0..read.len()).rev() {
                if ids[i].is_none() && !read[i].known {
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
            if analyzed.force_track.is_none() && !known {
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

    /// Parks a file that may be the stream track `track` (its length is
    /// off): option 1 attaches it to that track; the file's own tags
    /// import it as a track of its own.
    fn park_stream(&self, ctx: &Ctx, analyzed: Analyzed, identity: Identity, track: &Track) -> FileOutcome {
        let lib = &self.inner.library;
        let album = track
            .release_id
            .and_then(|r| lib.get_release(r).ok().flatten())
            .map(|r| r.title);
        let choice = Choice::Stream {
            track: track.id,
            title: track.title.clone(),
            artist: track.artist_credit.display(),
            album,
            track_no: (track.track_no > 0).then_some(track.track_no),
            duration_ms: track.duration_ms,
            recording_mbid: track.recording_mbid.clone(),
        };
        self.park(ctx, analyzed, identity, vec![Choice::Tags, choice])
    }

    /// Puts a file into «Подтверди», in memory and in the database.
    fn park(&self, ctx: &Ctx, analyzed: Analyzed, base: Identity, choices: Vec<Choice>) -> FileOutcome {
        let tags = TagView {
            title: analyzed.tags.title.clone(),
            artist: analyzed.tags.artist.clone(),
            album: analyzed.tags.album.clone(),
            year: analyzed.tags.year,
            track_no: analyzed.tags.track_no,
            duration_ms: Some(analyzed.info.duration_ms()),
            quality: Some(quality_line(&analyzed)),
        };
        let payload = Payload {
            origin: analyzed.origin.clone(),
            targets: (*analyzed.targets).clone(),
            tags,
            choices,
        };
        let path = analyzed.path.clone();
        let wishlist = analyzed.targets.wishlist;
        let stored = serde_json::to_string(&payload)
            .map_err(|e| ImportError::Other(e.to_string()))
            .and_then(|json| Ok(self.inner.library.put_confirm_item(&path.to_string_lossy(), wishlist, &json)?));
        let id = match stored {
            Ok(id) => ItemId(id as u64),
            Err(e) => {
                log::warn!("«Подтверди» for {} is not stored: {e}", path.display());
                ItemId(self.inner.next_item.fetch_add(1, Ordering::Relaxed))
            }
        };
        let item = ConfirmItem {
            id,
            job: ctx.job,
            path: path.clone(),
            tag_title: payload.tags.title.clone(),
            tag_artist: payload.tags.artist.clone(),
            tag_album: payload.tags.album.clone(),
            duration_ms: payload.tags.duration_ms,
            options: payload.choices.iter().map(|c| c.option(&payload.tags, &path)).collect(),
            wishlist,
            quality: payload.tags.quality.clone(),
        };
        lock(&self.inner.pending).insert(
            id,
            Parked {
                ctx: Some(ctx.clone()),
                origin: payload.origin,
                targets: analyzed.targets.clone(),
                read: Some((analyzed, base)),
                choices: payload.choices,
                item: item.clone(),
            },
        );
        ctx.emit(ImportEvent::NeedsConfirmation { item });
        FileOutcome::Parked(id)
    }

    /// The «Подтверди» queue of an earlier run, from the database (a file
    /// that is gone leaves it).
    fn restore_pending(&self) {
        let lib = &self.inner.library;
        let rows = match lib.confirm_items() {
            Ok(r) => r,
            Err(e) => {
                log::warn!("«Подтверди» not restored: {e}");
                return;
            }
        };
        for row in rows {
            let path = PathBuf::from(&row.path);
            let payload = serde_json::from_str::<Payload>(&row.payload).ok();
            let Some(p) = payload.filter(|_| long_path(&path).exists()) else {
                let _ = lib.delete_confirm_item(row.id);
                continue;
            };
            let id = ItemId(row.id as u64);
            let item = ConfirmItem {
                id,
                job: JobId(0),
                path: path.clone(),
                tag_title: p.tags.title.clone(),
                tag_artist: p.tags.artist.clone(),
                tag_album: p.tags.album.clone(),
                duration_ms: p.tags.duration_ms,
                options: p.choices.iter().map(|c| c.option(&p.tags, &path)).collect(),
                wishlist: row.wishlist,
                quality: p.tags.quality.clone(),
            };
            lock(&self.inner.active).insert(path);
            lock(&self.inner.pending).insert(
                id,
                Parked {
                    ctx: None,
                    origin: p.origin,
                    targets: Arc::new(p.targets),
                    read: None,
                    choices: p.choices,
                    item,
                },
            );
        }
    }

    /// Continues a parked import: `Some(i)` takes option `i` (0 is the
    /// file's own tags), `None` imports with the file's own tags. A file
    /// parked in an earlier run is read again first.
    pub async fn resolve(&self, item: ItemId, choice: Option<usize>) -> Result<FileOutcome> {
        let chosen = {
            let pending = lock(&self.inner.pending);
            let parked = pending
                .get(&item)
                .ok_or_else(|| ImportError::Other(format!("no pending item {}", item.0)))?;
            match choice {
                None => Choice::Tags,
                Some(i) => parked
                    .choices
                    .get(i)
                    .cloned()
                    .ok_or_else(|| ImportError::Other(format!("option {i} does not exist")))?,
            }
        };
        let Some(parked) = lock(&self.inner.pending).remove(&item) else {
            return Err(ImportError::Other(format!("item {} was resolved already", item.0)));
        };
        if let Err(e) = self.inner.library.delete_confirm_item(item.0 as i64) {
            log::warn!("«Подтверди» row {} stays: {e}", item.0);
        }
        let path = parked.item.path.clone();
        let ctx = parked.ctx.clone().unwrap_or_else(|| self.detached_ctx(parked.targets.clone()));
        let result = async {
            let (mut analyzed, base) = match parked.read {
                Some(r) => r,
                None => {
                    let (inner, p, o) = (self.inner.clone(), path.clone(), parked.origin.clone());
                    let mut a = blocking(move || analyze(&inner.cfg, &p, o)).await?;
                    a.targets = parked.targets.clone();
                    a.force_track = parked.targets.forced();
                    let mut base = identity_from_tags(&a.tags, &path);
                    if let Some(t) = a.force_track
                        && let Some(id) = identity_of_track(&self.inner.library, t, &base)
                    {
                        base = id;
                    }
                    (a, base)
                }
            };
            let identity = match &chosen {
                Choice::Tags => base,
                Choice::Candidate(c) => apply_candidate(&base, &c.candidate()),
                Choice::Stream { track, .. } => {
                    analyzed.force_track = Some(*track);
                    base
                }
            };
            self.finish_import(&ctx, analyzed, identity).await
        }
        .await;
        lock(&self.inner.active).remove(&path);
        match result {
            Ok(o) => Ok(o),
            Err(e) => {
                ctx.emit(ImportEvent::Failed {
                    job: ctx.job,
                    path,
                    error: e.to_string(),
                });
                Err(e)
            }
        }
    }

    /// Events of a file answered after a restart: no job listens, the
    /// broadcast does.
    fn detached_ctx(&self, targets: Arc<ImportTargets>) -> Ctx {
        let (tx, _) = mpsc::unbounded_channel();
        Ctx {
            job: JobId(self.inner.next_job.fetch_add(1, Ordering::Relaxed)),
            tx,
            events: self.inner.events.clone(),
            targets,
        }
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
                if let Some(master) = done.master.clone() {
                    if let Err(e) = inner.library.put_master(&master) {
                        log::warn!("master of {} not persisted: {e}", path.display());
                    }
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

    /// Embedded cover, a picture next to the file, the album folder's cover, then
    /// the Cover Art Archive / Deezer. `None` is fine.
    ///
    /// A picture that is a frame of a video (a file saved from YouTube
    /// embeds the clip's thumbnail) gives way to the album art online and to
    /// the art the target track's album already shows (the stream's); only
    /// when there is none it is kept, without its black bars.
    async fn prepare_cover(
        &self,
        a: &Analyzed,
        id: &Identity,
        album_dir: &Path,
    ) -> Option<ProcessedCover> {
        let max = self.inner.cfg.cover_max_edge;
        let (embedded, src_dir, album_dir) = (
            a.tags.cover.clone(),
            a.path.parent().map(Path::to_path_buf),
            album_dir.to_path_buf(),
        );
        let local = blocking(move || {
            let mut sources: Vec<Vec<u8>> = Vec::new();
            sources.extend(embedded);
            if let Some(dir) = src_dir {
                sources.extend(find_cover_file(&dir));
            }
            if let Ok(b) = std::fs::read(long_path(&album_dir.join(COVER_FILE))) {
                sources.push(b);
            }
            let mut frame = None;
            for b in sources {
                let Ok(c) = process_cover(&b, max) else { continue };
                if !cover_frame::is_frame(c.width, c.height) {
                    return Ok((Some(c), None));
                }
                frame = frame.or(Some(c));
            }
            Ok((None, frame))
        })
        .await
        .unwrap_or((None, None));
        let (local, frame) = local;
        if local.is_some() {
            return local;
        }
        let online = self.online_cover(a, id).await;
        match (online, frame) {
            (Some(c), _) => Some(c),
            (None, Some(f)) => blocking(move || Ok(cut_bars(f))).await.ok(),
            (None, None) => None,
        }
    }

    /// The album art online (the Cover Art Archive, the identity's cover
    /// url), else the web art the target track's album has.
    async fn online_cover(&self, a: &Analyzed, id: &Identity) -> Option<ProcessedCover> {
        let max = self.inner.cfg.cover_max_edge;
        let meta = self.inner.metadata.as_ref()?;
        if !self.inner.cfg.fetch_cover_online {
            return None;
        }
        let timeout = self.inner.cfg.online_timeout;
        let bytes = if let Some(rel) = &id.release_mbid {
            tokio::time::timeout(
                timeout,
                meta.cover_art
                    .fetch_front_with_fallback(rel, id.release_group_mbid.as_deref(), max),
            )
            .await
            .ok()
            .and_then(|r| r.ok())
            .flatten()
            .map(|c| c.bytes)
        } else {
            None
        };
        let target_art = {
            let (lib, targets) = (self.inner.library.clone(), a.targets.clone());
            blocking(move || Ok(target_web_cover(&lib, &targets))).await.ok().flatten()
        };
        let mut bytes = bytes;
        for url in [id.cover_url.clone(), target_art].into_iter().flatten() {
            if bytes.is_some() {
                break;
            }
            bytes = tokio::time::timeout(timeout, meta.cover_art.download(&large_web_cover(&url)))
                .await
                .ok()
                .and_then(|r| r.ok());
        }
        let bytes = bytes?;
        let c = blocking(move || process_cover(&bytes, max)).await.ok()?;
        (!cover_frame::is_frame(c.width, c.height)).then_some(c)
    }


}

// ------------------------------------------------------------------ stages

fn analyze(cfg: &ImportConfig, path: &Path, origin: SourceOrigin) -> Result<Analyzed> {
    let (tags, info) = tags::read_tags(path)?;
    let lossless = info.codec.is_lossless();
    let audio = analyze_file(path, cfg.check_fake_lossless && lossless, info.duration)?;
    let verdict = audio.spectrum.map(|s| s.verdict);
    let fake_lossless = lossless && verdict == Some(Verdict::Transcoded);
    let upsampled = lossless && verdict == Some(Verdict::Upsampled);
    let mut info = info;
    if info.duration.is_zero() {
        info.duration = audio.duration();
    }
    // Upsampled "Hi-Res" is CD quality in a big container.
    let (rate, depth) = if upsampled {
        (Some(44_100), Some(16))
    } else {
        (info.sample_rate, info.bit_depth)
    };
    let score = if origin == SourceOrigin::Stream {
        nikgob_core::model::score::SAVED_STREAM
    } else if fake_lossless {
        // As what its content really is: an MP3 of about that bitrate.
        nikgob_core::quality::fake_score(sound_origin(&audio))
    } else {
        quality_score(&info.codec, info.bitrate_kbps, rate, depth, false)
    };
    Ok(Analyzed {
        path: path.to_path_buf(),
        origin,
        tags,
        info,
        audio,
        fake_lossless,
        score,
        targets: Arc::default(),
        force_track: None,
    })
}

/// The likely origin of an analysed file's sound (unknown without a check).
fn sound_origin(audio: &AudioAnalysis) -> nikgob_core::quality::SoundOrigin {
    audio.spectrum.map_or(nikgob_core::quality::SoundOrigin::Unknown, |s| {
        nikgob_core::quality::SoundOrigin::estimate(s.verdict, s.cutoff_hz)
    })
}

/// What a file really is, for «Подтверди»: «FLAC из MP3 ~320», «MP3 320».
fn quality_line(a: &Analyzed) -> String {
    let check = a.audio.spectrum.map(|s| nikgob_core::quality::CheckSummary {
        verdict: s.verdict,
        origin: nikgob_core::quality::SoundOrigin::estimate(s.verdict, s.cutoff_hz),
        made_from: None,
    });
    nikgob_core::quality::label(&a.info.codec, a.info.bitrate_kbps, check.as_ref())
}

/// What [`find_existing`] needs about an analyzed file.
fn incoming<'a>(a: &'a Analyzed, id: &'a Identity) -> Incoming<'a> {
    Incoming {
        identity: id,
        tag_track_id: a.force_track.or(a.tags.track_id),
        fingerprint: a.audio.fingerprint.as_ref().map(|f| f.raw.as_slice()),
        targets: &a.targets.tracks,
        duration_ms: (!a.info.duration.is_zero()).then(|| a.info.duration_ms()),
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

fn decide_for(store: &mut Store, a: &Analyzed, id: &Identity) -> Result<Decision> {
    let existing = find_existing(store, &incoming(a, id))?;
    Ok(decide(existing, a.score))
}

struct Prepared {
    tmp: PathBuf,
    ext: String,
    /// Lossless input encoded to Opus (so the original is a master).
    converted: bool,
    original_hash: Option<String>,
    /// A fake lossless input written as MP3 (no master: it is not lossless).
    fake_mp3: bool,
}

/// Lossless -> Opus, everything else copied as is (never lossy to lossy). The
/// result is an untagged temporary file inside the target album folder.
fn convert(inner: &Inner, a: &Analyzed, album_dir: &Path) -> Result<Prepared> {
    fsutil::create_dir_all(album_dir)?;
    // Audio saved from a stream is kept exactly as it came.
    let lossless = a.info.codec.is_lossless() && a.origin != SourceOrigin::Stream;
    let encode = lossless && !a.fake_lossless;
    // A plain fake becomes an MP3 that keeps all it has, when the user lets
    // that happen without asking; without ffmpeg (or a doubtful cut) it
    // stays as it is, labelled for what it is.
    let hi_res = a.info.bit_depth.is_some_and(|b| b > 16) || a.info.sample_rate.is_some_and(|r| r > 48_000);
    let clear = a
        .audio
        .spectrum
        .is_some_and(|r| nikgob_core::quality::clear_fake(r.verdict, r.cutoff_hz, r.steepness_db, hi_res));
    let mp3 = (lossless && a.fake_lossless && clear && inner.cfg.convert_fake_lossless)
        .then(Mp3Encoder::find)
        .flatten();
    let ext = if encode {
        "opus".to_owned()
    } else if mp3.is_some() {
        "mp3".to_owned()
    } else {
        a.path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_else(|| "bin".to_owned())
    };
    let tmp = album_dir.join(format!(".{}.nikgob-tmp.{ext}", TrackId::new()));
    let original_hash = if encode {
        inner.encoder.encode_to_opus(
            &a.path,
            &tmp,
            &EncodeOptions {
                bitrate_kbps: inner.cfg.opus_bitrate_kbps,
            },
        )?;
        Some(fsutil::hash_file(&a.path)?)
    } else if let Some(enc) = &mp3 {
        let target = nikgob_core::quality::Mp3Target::for_origin(sound_origin(&a.audio));
        enc.encode(&a.path, &tmp, target)?;
        None
    } else {
        fsutil::copy_file(&a.path, &tmp)?;
        None
    };
    Ok(Prepared {
        tmp,
        ext,
        converted: encode,
        original_hash,
        fake_mp3: mp3.is_some(),
    })
}

struct Done {
    track_id: TrackId,
    upgraded: bool,
    master: Option<Master>,
    master_hash: Option<String>,
    undo: Option<Undo>,
}

/// What one committed file changed, so a cancelled job can take it back.
struct Undo {
    track: TrackId,
    /// The track as it was before (`None`: the import made it).
    before: Option<Track>,
    source: SourceId,
    /// The file it put in the library.
    dest: PathBuf,
    /// A master file it put in the masters folder (not one that was there).
    master_file: Option<PathBuf>,
    master_hash: Option<String>,
    replaced: Replaced,
}

/// Takes back what a cancelled job committed, the last file first: its
/// files go to the Trash (never deleted: an Inbox file may have been the
/// only copy), its rows go, a file it replaced comes back. Returns how many
/// files were taken back.
fn roll_back(inner: &Inner, journal: Vec<Undo>) -> usize {
    let (cfg, lib) = (&inner.cfg, &inner.library);
    let mut n = 0;
    for u in journal.into_iter().rev() {
        let mut gone = vec![u.dest.clone(), u.dest.with_extension("lrc")];
        gone.extend(u.master_file.clone());
        for f in gone {
            if long_path(&f).exists() {
                match trash_file(&cfg.trash_dir, &f) {
                    Ok(to) => log::info!("import cancelled: {} moved to {}", f.display(), to.display()),
                    Err(e) => log::warn!("import cancelled: {} stays: {e}", f.display()),
                }
            }
        }
        if let Some(hash) = &u.master_hash {
            let _ = lib.delete_master(u.track);
            let _ = fsutil::remove_file(&cfg.masters_dir().join(format!("{hash}.json")));
        }
        if let Err(e) = lib.forget_source(u.source) {
            log::warn!("import cancelled: the row of {} stays: {e}", u.dest.display());
        }
        u.replaced.undo(lib);
        match &u.before {
            Some(t) => {
                if let Err(e) = lib.upsert_track(t) {
                    log::warn!("import cancelled: {} keeps its new tags: {e}", t.title);
                }
            }
            None => {
                if let Err(e) = lib.forget_new_track(u.track) {
                    log::warn!("import cancelled: track {} stays: {e}", u.track);
                }
            }
        }
        if let Some(dir) = u.dest.parent() {
            fsutil::prune_empty_dirs(dir, &cfg.library_dir);
        }
        n += 1;
    }
    // Albums and artists the job made may be gone: the cache reads again.
    *lock(&inner.store) = Store::new(lib.clone());
    n
}

enum Committed {
    Imported(Done),
    Duplicate { kept: TrackId },
}

fn build_tagset(
    a: &Analyzed,
    id: &Identity,
    track_id: TrackId,
    cover: Option<&ProcessedCover>,
) -> TagSet {
    TagSet {
        title: Some(id.title.clone()),
        artists: id.artists.clone(),
        artist: Some(id.artist_line.clone()),
        album: id.album.clone(),
        album_artist: Some(Store::release_artist_line(id)),
        track_no: (id.track_no > 0).then_some(id.track_no),
        track_total: id.track_total,
        disc_no: Some(id.disc_no),
        disc_total: id.disc_total,
        year: id.year,
        date: id.date.clone(),
        genre: id.genre.clone(),
        compilation: id.compilation,
        recording_mbid: id.recording_mbid.clone(),
        release_mbid: id.release_mbid.clone(),
        release_group_mbid: id.release_group_mbid.clone(),
        artist_mbid: id.artist_mbid.clone(),
        isrc: id.isrc.clone(),
        acoustid_id: id.acoustid_id.clone(),
        acoustid_fingerprint: a.audio.fingerprint.as_ref().map(|f| f.encoded.clone()),
        track_id: Some(track_id),
        lyrics: a.tags.lyrics.clone(),
        loudness_lufs: a.audio.loudness_lufs,
        cover: cover.map(|c| c.jpeg.clone()),
        ..TagSet::default()
    }
}

#[derive(Serialize)]
struct MasterSidecar<'a> {
    track_id: TrackId,
    codec: &'a Codec,
    file_hash: &'a str,
    size_bytes: u64,
    origin: &'a SourceOrigin,
    /// Where the lossless file lived at import time. A torrent payload later
    /// moves to `masters/{file_hash}.<ext>` when its torrent is cleaned up, so
    /// readers must look there by hash and not trust this field.
    path: String,
    original_name: String,
    quality_score: i32,
}

/// Registers the lossless original as the track's master (PLAN section 6, step
/// 9): torrent payloads stay where they are (they keep seeding), Inbox files move
/// to `.musiclib/masters/`, anything else is copied there.
fn register_master(
    inner: &Inner,
    a: &Analyzed,
    hash: &str,
    track_id: TrackId,
) -> Result<Master> {
    let cfg = &inner.cfg;
    let size = fsutil::file_size(&a.path)?;
    let path = match &a.origin {
        SourceOrigin::Torrent { .. } => a.path.clone(),
        _ => {
            let dir = cfg.masters_dir();
            fsutil::create_dir_all(&dir)?;
            let ext = a
                .path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_else(|| "flac".to_owned());
            let dest = dir.join(format!("{hash}.{ext}"));
            if !long_path(&dest).exists() {
                if a.path.starts_with(&cfg.inbox_dir) {
                    fsutil::move_file_verified(&a.path, &dest, Some(hash))?;
                } else {
                    fsutil::copy_file_verified(&a.path, &dest, Some(hash))?;
                }
            } else if a.path.starts_with(&cfg.inbox_dir) {
                if fsutil::hash_file(&dest).ok().as_deref() == Some(hash) {
                    fsutil::remove_file(&a.path)?;
                } else {
                    fsutil::move_file_verified(&a.path, &dest, Some(hash))?;
                }
            }
            dest
        }
    };
    let sidecar = MasterSidecar {
        track_id,
        codec: &a.info.codec,
        file_hash: hash,
        size_bytes: size,
        origin: &a.origin,
        path: path.to_string_lossy().into_owned(),
        original_name: a
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        quality_score: a.score,
    };
    let dir = cfg.masters_dir();
    fsutil::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(&sidecar).map_err(|e| ImportError::Other(e.to_string()))?;
    let sidecar_path = dir.join(format!("{hash}.json"));
    std::fs::write(long_path(&sidecar_path), json).map_err(fsutil::io_err(&sidecar_path))?;
    Ok(Master {
        track_id,
        codec: a.info.codec.clone(),
        file_hash: hash.to_owned(),
        size_bytes: size,
        origin: Some(a.origin.clone()),
        replicas: Vec::new(),
    })
}

/// Summary of relocating lossless masters to a new directory/drive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelocateReport {
    /// Lossless audio files copied, verified by BLAKE3 hash, and removed from `old_dir`.
    pub audio_moved: usize,
    /// Sidecar `.json` files moved and updated to point to `new_dir`.
    pub sidecars_moved: usize,
    /// Total audio bytes transferred and verified.
    pub bytes_moved: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct OwnedMasterSidecar {
    track_id: TrackId,
    codec: Codec,
    file_hash: String,
    size_bytes: u64,
    origin: SourceOrigin,
    path: String,
    original_name: String,
    quality_score: i32,
}

/// Safely migrates all lossless master files and `.json` sidecars from `old_dir`
/// to `new_dir`:
/// 1. Probes `new_dir` for write access.
/// 2. Copies each audio file to a `.nikgob-part` temporary file in `new_dir`,
///    flushes to disk (`sync_all`), verifies its byte size and BLAKE3 hash,
///    renames into place, and deletes the original in `old_dir` only after
///    verification succeeds.
/// 3. Moves and updates `.json` sidecars and rewrites `master.origin_path` in
///    the library database.
/// 4. Removes `old_dir` if it becomes empty.
pub fn relocate_masters<F>(
    lib: &Library,
    old_dir: &Path,
    new_dir: &Path,
    mut on_progress: F,
) -> Result<RelocateReport>
where
    F: FnMut(usize, usize, &Path),
{
    fsutil::create_dir_all(new_dir)?;
    let probe = new_dir.join(".nikgob-write-probe");
    std::fs::write(long_path(&probe), b"ok").map_err(fsutil::io_err(&probe))?;
    fsutil::remove_file(&probe)?;

    if old_dir == new_dir || !long_path(old_dir).exists() {
        return Ok(RelocateReport::default());
    }
    if let (Ok(a), Ok(b)) = (
        std::fs::canonicalize(long_path(old_dir)),
        std::fs::canonicalize(long_path(new_dir)),
    ) {
        if a == b {
            return Ok(RelocateReport::default());
        }
    }

    let rd = std::fs::read_dir(long_path(old_dir)).map_err(fsutil::io_err(old_dir))?;
    let mut audio_files = Vec::new();
    let mut sidecar_files = Vec::new();
    for entry in rd.flatten() {
        let path = old_dir.join(entry.file_name());
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            if name.ends_with(".nikgob-part") || name.ends_with(".nikgob-write-probe") {
                let _ = fsutil::remove_file(&path);
            }
            continue;
        }
        let Ok(meta) = std::fs::metadata(long_path(&path)) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
        {
            sidecar_files.push(path);
        } else if fsutil::is_audio_file(&path) {
            audio_files.push(path);
        }
    }
    audio_files.sort();
    sidecar_files.sort();

    let mut report = RelocateReport::default();
    let total = audio_files.len();

    for (idx, src) in audio_files.iter().enumerate() {
        on_progress(idx, total, src);
        let size = fsutil::file_size(src)?;
        let stem = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let sidecar_path = old_dir.join(format!("{stem}.json"));
        let sidecar_hash = std::fs::read(long_path(&sidecar_path))
            .ok()
            .and_then(|b| serde_json::from_slice::<OwnedMasterSidecar>(&b).ok())
            .map(|s| s.file_hash);
        let expected_hash = sidecar_hash.or_else(|| {
            (stem.len() == 64 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| stem.to_ascii_lowercase())
        });

        let dst = new_dir.join(src.file_name().unwrap());
        fsutil::move_file_verified(src, &dst, expected_hash.as_deref())?;
        report.audio_moved += 1;
        report.bytes_moved += size;
    }

    for src_json in &sidecar_files {
        let file_name = src_json.file_name().unwrap();
        let dst_json = new_dir.join(file_name);
        let raw = std::fs::read(long_path(src_json)).map_err(fsutil::io_err(src_json))?;
        let out_bytes = match serde_json::from_slice::<OwnedMasterSidecar>(&raw) {
            Ok(mut sc) => {
                let p = Path::new(&sc.path);
                if let Ok(rel) = p.strip_prefix(old_dir) {
                    sc.path = new_dir.join(rel).to_string_lossy().into_owned();
                }
                serde_json::to_vec_pretty(&sc).unwrap_or(raw)
            }
            Err(_) => raw,
        };
        let tmp = dst_json.with_file_name(format!(
            ".{}.nikgob-part",
            file_name.to_string_lossy()
        ));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(long_path(&tmp)).map_err(fsutil::io_err(&tmp))?;
            f.write_all(&out_bytes).map_err(fsutil::io_err(&tmp))?;
            f.sync_all().map_err(fsutil::io_err(&tmp))?;
        }
        let verified_size = fsutil::file_size(&tmp)?;
        if verified_size != out_bytes.len() as u64 {
            let _ = fsutil::remove_file(&tmp);
            return Err(ImportError::Other(format!(
                "sidecar verification failed for {}",
                dst_json.display()
            )));
        }
        std::fs::rename(long_path(&tmp), long_path(&dst_json)).map_err(|e| {
            let _ = fsutil::remove_file(&tmp);
            ImportError::io(&dst_json, e)
        })?;
        fsutil::remove_file(src_json)?;
        report.sidecars_moved += 1;
    }

    let _ = lib.relocate_master_paths(old_dir, new_dir);

    if let Ok(mut rd) = std::fs::read_dir(long_path(old_dir)) {
        if rd.next().is_none() {
            let _ = std::fs::remove_dir(long_path(old_dir));
        }
    }

    Ok(report)
}

/// The web cover (a stream's album art) of the first target track's album.
fn target_web_cover(lib: &Library, targets: &ImportTargets) -> Option<String> {
    targets.tracks.iter().find_map(|t| {
        let release = lib.get_track(*t).ok().flatten()?.release_id?;
        lib.get_release(release).ok().flatten()?.cover_ref.filter(|c| nikgob_library::is_web_ref(c))
    })
}

/// A YouTube Music cover url (`…=w544-h544-l90-rj`) asked at 1200 px; any
/// other url as it is.
fn large_web_cover(url: &str) -> String {
    match url.rsplit_once('=') {
        Some((base, size)) if url.contains("googleusercontent.com") && size.starts_with('w') => {
            format!("{base}=w1200-h1200-l90-rj")
        }
        _ => url.to_owned(),
    }
}

/// A frame of a video without its black bars (the art of a song uploaded
/// with it), re-encoded; as it is when it has none or does not decode.
fn cut_bars(c: ProcessedCover) -> ProcessedCover {
    let Ok(img) = image::load_from_memory(&c.jpeg) else { return c };
    let rgba = img.to_rgba8();
    let Some((x, y, w, h)) = cover_frame::content(rgba.width(), rgba.height(), rgba.as_raw()) else { return c };
    let cut = image::DynamicImage::ImageRgba8(rgba).crop_imm(x, y, w, h);
    let mut png = Vec::new();
    if cut.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).is_err() {
        return c;
    }
    process_cover(&png, w.max(h)).unwrap_or(c)
}

/// Writes `cover.jpg` and `ambient.png` when the album folder has none or a
/// smaller cover. Returns whether the cover changed.
fn write_cover_files(dir: &Path, cover: &ProcessedCover) -> Result<bool> {
    let target = dir.join(COVER_FILE);
    let replace = match image::image_dimensions(long_path(&target)) {
        Ok((w, h)) => cover.width.max(cover.height) > w.max(h),
        Err(_) => true,
    };
    if !replace {
        return Ok(false);
    }
    let visuals = derive_visuals(&cover.jpeg)?;
    let tmp = dir.join(".cover.nikgob-tmp.jpg");
    std::fs::write(long_path(&tmp), &cover.jpeg).map_err(fsutil::io_err(&tmp))?;
    fsutil::move_file(&tmp, &target)?;
    let ambient = dir.join(AMBIENT_FILE);
    std::fs::write(long_path(&ambient), &visuals.ambient_png).map_err(fsutil::io_err(&ambient))?;
    Ok(true)
}

fn find_cover_file(dir: &Path) -> Option<Vec<u8>> {
    let rd = std::fs::read_dir(long_path(dir)).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for e in rd.flatten() {
        let path = e.path();
        let Some(stem) = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()) else {
            continue;
        };
        let ext = path
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if !matches!(ext.as_str(), "jpg" | "jpeg" | "png") {
            continue;
        }
        if let Some(rank) = COVER_NAMES.iter().position(|n| *n == stem)
            && best.as_ref().is_none_or(|(r, _)| rank < *r)
        {
            best = Some((rank, path));
        }
    }
    std::fs::read(long_path(&best?.1)).ok()
}

pub(crate) fn rel_to_library(cfg: &ImportConfig, path: &Path) -> String {
    path.strip_prefix(&cfg.library_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The critical section: decide again, tag, trash the replaced file, rename into
/// place, write the library rows, register the master.
fn commit(
    inner: &Inner,
    a: &Analyzed,
    id: &Identity,
    prep: &Prepared,
    cover: Option<&ProcessedCover>,
) -> Result<Committed> {
    let result = commit_locked(inner, a, id, prep, cover);
    if result.is_err() || matches!(result, Ok(Committed::Duplicate { .. })) {
        let _ = fsutil::remove_file(&prep.tmp);
        if let Some(dir) = prep.tmp.parent() {
            fsutil::prune_empty_dirs(dir, &inner.cfg.library_dir);
        }
    }
    result
}

fn commit_locked(
    inner: &Inner,
    a: &Analyzed,
    id: &Identity,
    prep: &Prepared,
    cover: Option<&ProcessedCover>,
) -> Result<Committed> {
    let cfg = &inner.cfg;
    let lib = &inner.library;
    let mut store = lock(&inner.store);
    let decision = decide_for(&mut store, a, id)?;
    let existing = match &decision {
        Decision::Duplicate(e) => return Ok(Committed::Duplicate { kept: e.track.id }),
        Decision::New => None,
        Decision::Upgrade(e) | Decision::AddSource(e) => Some(e),
    };
    let before = existing.map(|e| e.track.clone());
    // A track of the wished album keeps its place there (named and tagged
    // so), whatever number the file brought.
    let kept_place;
    let id = match existing.and_then(|e| crate::album::keep_place(lib, &a.targets, &e.track, id)) {
        Some(placed) => {
            kept_place = placed;
            &kept_place
        }
        None => id,
    };
    let track_id = match existing {
        Some(e) => e.track.id,
        // Keep the id a rebuilt/backed-up file already carries, if it is free.
        None => a
            .tags
            .track_id
            .filter(|t| lib.get_track(*t).ok().flatten().is_none())
            .unwrap_or_default(),
    };

    tags::write_tags(&prep.tmp, &build_tagset(a, id, track_id, cover))?;

    // The replaced file goes to Trash first: it may sit exactly where the new one lands.
    let mut replaced = Replaced::default();
    if let Decision::Upgrade(e) = &decision
        && let Some(old) = &e.local
    {
        let old_path = PathBuf::from(&old.locator);
        let trash_path = if long_path(&old_path).exists() {
            let dest = trash_file(&cfg.trash_dir, &old_path)?;
            log::info!("import: {} is replaced, the old file is in the Trash at {}", old_path.display(), dest.display());
            replaced.file = Some((old_path.clone(), dest.clone()));
            let lrc = old_path.with_extension("lrc");
            if long_path(&lrc).exists()
                && let Ok(lrc_dest) = trash_file(&cfg.trash_dir, &lrc)
            {
                replaced.lrc = Some((lrc, lrc_dest));
            }
            Some(dest.to_string_lossy().into_owned())
        } else {
            None
        };
        if let Err(e) = lib.trash_source(old.id, trash_path.as_deref()) {
            replaced.undo(lib);
            return Err(e.into());
        }
        replaced.source = Some(old.id);
    }

    let layout = layout_input(id);
    let album_dir = layout.album_dir(&cfg.library_dir);
    let dest = layout.unique_target_path(&cfg.library_dir, &prep.ext);
    if let Err(e) = fsutil::move_file(&prep.tmp, &dest) {
        // Put the replaced file back; the library still lists it as trashed, so
        // restore that too.
        replaced.undo(lib);
        return Err(e);
    }

    let finish = (|| -> Result<Done> {
        let out_info = tags::probe_info(&dest)?;
        let file_hash = fsutil::hash_file(&dest)?;
        let size = fsutil::file_size(&dest)?;

        let mut new_cover = false;
        let cover_ref = match cover {
            Some(c) => {
                new_cover = write_cover_files(&album_dir, c)?;
                Some(rel_to_library(cfg, &album_dir.join(COVER_FILE)))
            }
            None => None,
        };

        let release = store.ensure_release(id, cover_ref)?;
        let credit = store.track_credit(id)?;
        let mut track = Track::new(id.title.clone(), credit);
        track.id = track_id;
        track.recording_mbid = id.recording_mbid.clone();
        track.isrc = id.isrc.clone();
        track.acoustid = id.acoustid_id.clone();
        track.release_id = Some(release.id);
        track.disc_no = id.disc_no;
        track.track_no = id.track_no;
        track.duration_ms = Some(out_info.duration_ms());
        let track = lib.upsert_track(&track)?;

        let mut source = Source::new(
            track.id,
            SourceKind::LocalFile,
            dest.to_string_lossy().into_owned(),
            out_info.codec.clone(),
        );
        source.bitrate = out_info.bitrate_kbps;
        source.sample_rate = out_info.sample_rate;
        source.bit_depth = out_info.bit_depth;
        // A converted file keeps the rank of the lossless original it came from,
        // so re-importing that original is a duplicate and not an endless upgrade.
        source.quality_score = a.score;
        source.file_hash = Some(file_hash);
        source.size_bytes = Some(size);
        source.origin = a.origin.clone();
        let stored = lib.add_source(&source)?;
        // What the check found, kept for the labels and the quality panel;
        // a converted file's check is of the original it was made from.
        if let Some(report) = &a.audio.spectrum {
            let made_from = (prep.converted || prep.fake_mp3).then(|| a.info.codec.clone());
            let check = check_of(report, a.audio.spectrum_db.clone(), a.audio.spec.sample_rate, made_from, file_stamp(&dest));
            if let Err(e) = lib.put_quality_check(stored.id, &check) {
                log::warn!("the check of {} is not stored: {e}", dest.display());
            }
        }

        // From here on the track is in the library: nothing below may fail the import.
        // Palette and ambient texture, once per release (or when the cover grew).
        if cover.is_some() {
            let palette = (|| -> Result<()> {
                if new_cover || lib.get_palette(release.id)?.is_none() {
                    let bytes = std::fs::read(long_path(&album_dir.join(COVER_FILE)))
                        .map_err(fsutil::io_err(&album_dir))?;
                    let visuals = derive_visuals(&bytes)?;
                    let mut palette = Palette::new(
                        release.id,
                        visuals.seed_argb.unwrap_or(cfg.fallback_seed_argb),
                    );
                    palette.ambient_blur = Some(visuals.ambient_png);
                    lib.put_palette(&palette)?;
                }
                Ok(())
            })();
            if let Err(e) = palette {
                log::warn!("palette of {} not stored: {e}", dest.display());
            }
        }

        // The master file this import puts in the masters folder (not one a
        // track of the same sound put there before), for a cancel.
        let mut master_file = None;
        let master = match (&prep.original_hash, prep.converted) {
            (Some(hash), true) => {
                let in_dir = (!matches!(a.origin, SourceOrigin::Torrent { .. })).then(|| {
                    let ext = a
                        .path
                        .extension()
                        .map(|e| e.to_string_lossy().to_lowercase())
                        .unwrap_or_else(|| "flac".to_owned());
                    cfg.masters_dir().join(format!("{hash}.{ext}"))
                });
                let was_there = in_dir.as_ref().is_some_and(|p| long_path(p).exists());
                match register_master(inner, a, hash, track.id) {
                    Ok(m) => {
                        master_file = in_dir.filter(|_| !was_there);
                        Some(m)
                    }
                    Err(e) => {
                        log::warn!("master of {} not registered: {e}", a.path.display());
                        None
                    }
                }
            }
            _ => None,
        };
        // A lossy file taken from the Inbox now lives in the library; the
        // fake lossless file an MP3 was made from goes to the Trash.
        if !prep.converted && a.path.starts_with(&cfg.inbox_dir) {
            let gone = if prep.fake_mp3 {
                trash_file(&cfg.trash_dir, &a.path).map(|to| {
                    log::info!("import: {} came as an MP3, the fake FLAC is in the Trash at {}", a.path.display(), to.display());
                })
            } else {
                fsutil::remove_file(&a.path)
            };
            if let Err(e) = gone {
                log::warn!("cannot remove imported {}: {e}", a.path.display());
            }
        }
        Ok(Done {
            track_id: track.id,
            upgraded: matches!(decision, Decision::Upgrade(_)),
            master_hash: master.as_ref().map(|m| m.file_hash.clone()),
            master,
            undo: Some(Undo {
                track: track.id,
                before: before.clone(),
                source: stored.id,
                dest: dest.clone(),
                master_file,
                master_hash: None,
                replaced: Replaced::default(),
            }),
        })
    })();
    match finish {
        Ok(mut done) => {
            if let Some(u) = done.undo.as_mut() {
                u.master_hash = done.master_hash.take();
                u.replaced = replaced;
            }
            Ok(Committed::Imported(done))
        }
        Err(e) => {
            let _ = fsutil::remove_file(&dest);
            // The old file may have to come back to the very path just freed.
            replaced.undo(lib);
            Err(e)
        }
    }
}

/// What an upgrade moved out of the way, so a failed import can put it back.
#[derive(Default)]
struct Replaced {
    /// `(original place, place in Trash)` of the old file.
    file: Option<(PathBuf, PathBuf)>,
    lrc: Option<(PathBuf, PathBuf)>,
    /// The old source, once the library lists it as trashed.
    source: Option<nikgob_core::model::SourceId>,
}

impl Replaced {
    fn undo(&self, lib: &Library) {
        for (orig, in_trash) in self.file.iter().chain(&self.lrc) {
            if let Err(e) = fsutil::move_file(in_trash, orig) {
                log::warn!("cannot restore {} from Trash: {e}", orig.display());
            }
        }
        if let Some(id) = self.source
            && let Err(e) = lib.restore_source(id)
        {
            log::warn!("cannot restore the replaced source: {e}");
        }
    }
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn a_youtube_music_cover_is_asked_large_and_others_as_they_are() {
        assert_eq!(
            large_web_cover("https://yt3.googleusercontent.com/abc=w544-h544-l90-rj"),
            "https://yt3.googleusercontent.com/abc=w1200-h1200-l90-rj"
        );
        let deezer = "https://cdn-images.dzcdn.net/images/cover/x/1000x1000-000000-80-0-0.jpg";
        assert_eq!(large_web_cover(deezer), deezer);
    }

    #[test]
    fn a_pillarboxed_frame_keeps_only_its_art() {
        let frame = image::RgbImage::from_fn(640, 360, |x, _| if (140..500).contains(&x) { image::Rgb([30, 90, 200]) } else { image::Rgb([0, 0, 0]) });
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(frame).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let c = process_cover(&png, 1200).unwrap();
        assert!(cover_frame::is_frame(c.width, c.height));
        let cut = cut_bars(c);
        assert_eq!((cut.width, cut.height), (360, 360));
        // A frame without bars stays as it is.
        let still = process_cover(&crate::cover::testimg::solid_jpeg([90, 90, 90], 640, 360), 1200).unwrap();
        assert_eq!((cut_bars(still.clone()).width, still.height), (640, 360));
    }
}
