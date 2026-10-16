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

use nikgob_core::model::{
    Codec, Master, Palette, Source, SourceKind, SourceOrigin, Track, TrackId, quality_score,
};
use nikgob_library::Library;
use nikgob_metadata::Metadata;
use nikgob_metadata::matcher::MatchCandidate;
use serde::Serialize;
use tokio::sync::{Semaphore, broadcast, mpsc};

use crate::analysis::{AudioAnalysis, analyze_file};
use crate::config::ImportConfig;
use crate::cover::{ProcessedCover, derive_visuals, process_cover};
use crate::decide::{Decision, Incoming, decide, find_existing};
use crate::encode::{EncodeOptions, Encoder, RopusEncoder};
use crate::error::{ImportError, Result};
use crate::events::{ConfirmItem, ImportEvent, ItemId, JobId, Stage};
use crate::folder::{Anchor, Member, album_group_dir, plan};
use crate::fsutil;
use crate::identity::{
    Identity, Lookup, apply_candidate, confirm_option, evidence, identity_from_tags,
    lookup as online_lookup,
};
use crate::layout::LayoutInput;
use crate::sanitize::long_path;
use crate::spectrum::Verdict;
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
    /// Already being handled.
    Skipped,
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
}

/// A file that is read and identified and waits for its folder's other files.
struct Ready {
    analyzed: Analyzed,
    /// What the tags alone say.
    base: Identity,
    identity: Identity,
}

enum Step {
    Ready(Box<Ready>),
    Done(FileOutcome),
}

enum Identified {
    Ready(Box<Ready>),
    Done(FileOutcome),
}

struct Parked {
    ctx: Ctx,
    analyzed: Analyzed,
    base: Identity,
    options: Vec<MatchCandidate>,
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
        Self {
            inner: Arc::new(Inner {
                store: Mutex::new(Store::new(library.clone())),
                cfg,
                library,
                metadata,
                encoder,
                next_job: AtomicU64::new(1),
                next_item: AtomicU64::new(1),
                events,
                pending: Mutex::new(HashMap::new()),
                active: Mutex::new(HashSet::new()),
                permits,
            }),
        }
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
        let id = JobId(self.inner.next_job.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = Ctx {
            job: id,
            tx,
            events: self.inner.events.clone(),
        };
        let this = self.clone();
        tokio::spawn(async move { this.run_job(ctx, paths, origin).await });
        ImportJob { id, events: rx }
    }

    async fn run_job(self, ctx: Ctx, paths: Vec<PathBuf>, origin: SourceOrigin) {
        let scanned = tokio::task::spawn_blocking(move || fsutil::scan(&paths)).await;
        let (files, torrents) = scanned.unwrap_or_default();
        ctx.emit(ImportEvent::Started {
            job: ctx.job,
            files: files.len(),
        });
        for t in torrents {
            ctx.emit(ImportEvent::TorrentFound(t));
        }
        // One album folder at a time: identify all its files, agree on a single
        // release, then import them.
        let mut folders: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
        for path in files {
            folders.entry(album_group_dir(&path)).or_default().push(path);
        }
        let (mut imported, mut duplicates, mut failed, mut pending) = (0, 0, 0, 0);
        for (_, files) in folders {
            let mut set = tokio::task::JoinSet::new();
            for path in files {
                let Ok(permit) = self.inner.permits.clone().acquire_owned().await else {
                    break;
                };
                let (this, ctx, origin) = (self.clone(), ctx.clone(), origin.clone());
                set.spawn(async move {
                    let identified = this.identify_file(&ctx, path, origin).await;
                    drop(permit);
                    identified
                });
            }
            let mut outcomes = Vec::new();
            let mut ready = Vec::new();
            while let Some(res) = set.join_next().await {
                match res {
                    Ok(Identified::Ready(r)) => ready.push(r),
                    Ok(Identified::Done(o)) => outcomes.push(o),
                    Err(_) => failed += 1,
                }
            }
            // Task completion order is arbitrary; the folder's vote must not be.
            ready.sort_by(|a, b| a.analyzed.path.cmp(&b.analyzed.path));
            let members: Vec<_> = ready
                .iter()
                .map(|r| Member {
                    id: &r.identity,
                    base: &r.base,
                })
                .collect();
            let anchor = plan(&members).map(Arc::new);
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
                    FileOutcome::Skipped => {}
                }
            }
        }
        ctx.emit(ImportEvent::Finished {
            job: ctx.job,
            imported,
            duplicates,
            failed,
            pending,
        });
    }

    /// Reads and identifies one file; never returns an error, failures become events.
    async fn identify_file(&self, ctx: &Ctx, path: PathBuf, origin: SourceOrigin) -> Identified {
        if !lock(&self.inner.active).insert(path.clone()) {
            return Identified::Done(FileOutcome::Skipped);
        }
        match self.identify(ctx, &path, origin).await {
            Ok(Step::Ready(r)) => Identified::Ready(r),
            Ok(Step::Done(o)) => {
                if !matches!(o, FileOutcome::Parked(_)) {
                    lock(&self.inner.active).remove(&path);
                }
                Identified::Done(o)
            }
            Err(e) => {
                lock(&self.inner.active).remove(&path);
                Identified::Done(self.failed(ctx, &path, &e))
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

    /// Steps 1..3: probe, fingerprint, identify.
    async fn identify(&self, ctx: &Ctx, path: &Path, origin: SourceOrigin) -> Result<Step> {
        let inner = self.inner.clone();
        ctx.stage(path, Stage::Probing);
        let origin = self.file_origin(&origin, path);
        ctx.stage(path, Stage::Fingerprinting);
        let analyzed = {
            let (p, o, i) = (path.to_path_buf(), origin, inner.clone());
            blocking(move || analyze(&i.cfg, &p, o)).await?
        };
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
                    });
                    Ok(Step::Done(FileOutcome::Duplicate { kept: t.id }))
                }
                None => Err(ImportError::Other(
                    "file is inside the library folder; use the library rebuild instead".into(),
                )),
            };
        }

        let base = identity_from_tags(&analyzed.tags, path);
        let known_track = analyzed
            .tags
            .track_id
            .is_some_and(|t| inner.library.get_track(t).ok().flatten().is_some());
        let fully_tagged = analyzed.tags.recording_mbid.is_some() && analyzed.tags.release_mbid.is_some();
        let identity = match &inner.metadata {
            Some(meta) if !known_track && !fully_tagged => {
                ctx.stage(path, Stage::Identifying);
                let ev = evidence(&analyzed.tags, &analyzed.info, path);
                match online_lookup(meta, &inner.cfg, ev, analyzed.audio.fingerprint.as_ref()).await {
                    Lookup::Sure(best) => apply_candidate(&base, &best),
                    Lookup::None => base.clone(),
                    Lookup::Unsure(options) => {
                        return Ok(Step::Done(self.park(ctx, analyzed, base, options)));
                    }
                }
            }
            _ => base.clone(),
        };
        Ok(Step::Ready(Box::new(Ready {
            analyzed,
            base,
            identity,
        })))
    }

    fn park(
        &self,
        ctx: &Ctx,
        analyzed: Analyzed,
        base: Identity,
        options: Vec<MatchCandidate>,
    ) -> FileOutcome {
        let id = ItemId(self.inner.next_item.fetch_add(1, Ordering::Relaxed));
        let item = ConfirmItem {
            id,
            job: ctx.job,
            path: analyzed.path.clone(),
            tag_title: analyzed.tags.title.clone(),
            tag_artist: analyzed.tags.artist.clone(),
            tag_album: analyzed.tags.album.clone(),
            duration_ms: Some(analyzed.info.duration_ms()),
            options: options.iter().map(confirm_option).collect(),
        };
        lock(&self.inner.pending).insert(
            id,
            Parked {
                ctx: ctx.clone(),
                analyzed,
                base,
                options,
                item: item.clone(),
            },
        );
        ctx.emit(ImportEvent::NeedsConfirmation { item });
        FileOutcome::Parked(id)
    }

    /// Continues a parked import: `Some(i)` takes option `i`, `None` imports with
    /// the file's own tags.
    pub async fn resolve(&self, item: ItemId, choice: Option<usize>) -> Result<FileOutcome> {
        let identity = {
            let pending = lock(&self.inner.pending);
            let parked = pending
                .get(&item)
                .ok_or_else(|| ImportError::Other(format!("no pending item {}", item.0)))?;
            match choice {
                Some(i) => {
                    let cand = parked.options.get(i).ok_or_else(|| {
                        ImportError::Other(format!("option {i} does not exist"))
                    })?;
                    apply_candidate(&parked.base, cand)
                }
                None => parked.base.clone(),
            }
        };
        let Some(parked) = lock(&self.inner.pending).remove(&item) else {
            return Err(ImportError::Other(format!("item {} was resolved already", item.0)));
        };
        let path = parked.analyzed.path.clone();
        let ctx = parked.ctx.clone();
        let result = self.finish_import(&ctx, parked.analyzed, identity).await;
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

        let layout = layout_input(&identity);
        let album_dir = layout.album_dir(&inner.cfg.library_dir);

        // Early decision: a worse duplicate costs nothing more than this.
        let early = {
            let (i, a, id) = (inner.clone(), analyzed.clone(), identity.clone());
            blocking(move || decide_for(&mut lock(&i.store), &a, &id)).await?
        };
        if let Decision::Duplicate(e) = &early {
            let kept = e.track.id;
            return Ok(self.report_duplicate(ctx, &analyzed, kept));
        }

        let cover = self.prepare_cover(&analyzed, &identity, &album_dir).await;

        ctx.stage(&path, Stage::Converting);
        let prepared = {
            let (i, a, dir) = (inner.clone(), analyzed.clone(), album_dir.clone());
            blocking(move || convert(&i, &a, &dir)).await?
        };

        ctx.stage(&path, Stage::Tagging);
        let committed = {
            let (i, a, id, c) = (inner.clone(), analyzed.clone(), identity.clone(), cover.clone());
            blocking(move || commit(&i, &a, &id, &prepared, c.as_ref())).await?
        };
        match committed {
            Committed::Duplicate { kept } => Ok(self.report_duplicate(ctx, &analyzed, kept)),
            Committed::Imported(done) => {
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
        });
        FileOutcome::Duplicate { kept }
    }

    /// Embedded cover, a picture next to the file, the album folder's cover, then
    /// the Cover Art Archive / Deezer. `None` is fine.
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
            Ok(sources.into_iter().find_map(|b| process_cover(&b, max).ok()))
        })
        .await
        .ok()
        .flatten();
        if local.is_some() {
            return local;
        }
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
        let bytes = match (bytes, &id.cover_url) {
            (Some(b), _) => Some(b),
            (None, Some(url)) => tokio::time::timeout(timeout, meta.cover_art.download(url))
                .await
                .ok()
                .and_then(|r| r.ok()),
            _ => None,
        }?;
        blocking(move || process_cover(&bytes, max)).await.ok()
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
    let score = quality_score(&info.codec, info.bitrate_kbps, rate, depth, fake_lossless);
    Ok(Analyzed {
        path: path.to_path_buf(),
        origin,
        tags,
        info,
        audio,
        fake_lossless,
        score,
    })
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
    let existing = find_existing(
        store,
        &Incoming {
            identity: id,
            tag_track_id: a.tags.track_id,
            fingerprint: a.audio.fingerprint.as_ref().map(|f| f.raw.as_slice()),
        },
    )?;
    Ok(decide(existing, a.score))
}

struct Prepared {
    tmp: PathBuf,
    ext: String,
    /// Lossless input encoded to Opus (so the original is a master).
    converted: bool,
    original_hash: Option<String>,
}

/// Lossless -> Opus, everything else copied as is (never lossy to lossy). The
/// result is an untagged temporary file inside the target album folder.
fn convert(inner: &Inner, a: &Analyzed, album_dir: &Path) -> Result<Prepared> {
    fsutil::create_dir_all(album_dir)?;
    let lossless = a.info.codec.is_lossless();
    let encode = lossless && (!a.fake_lossless || inner.cfg.convert_fake_lossless);
    let ext = if encode {
        "opus".to_owned()
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
    } else {
        fsutil::copy_file(&a.path, &tmp)?;
        None
    };
    Ok(Prepared {
        tmp,
        ext,
        converted: encode,
        original_hash,
    })
}

struct Done {
    track_id: TrackId,
    upgraded: bool,
    master: Option<Master>,
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
        lib.add_source(&source)?;

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

        let master = match (&prep.original_hash, prep.converted) {
            (Some(hash), true) => match register_master(inner, a, hash, track.id) {
                Ok(m) => Some(m),
                Err(e) => {
                    log::warn!("master of {} not registered: {e}", a.path.display());
                    None
                }
            },
            _ => None,
        };
        // A lossy file taken from the Inbox now lives in the library.
        if !prep.converted
            && a.path.starts_with(&cfg.inbox_dir)
            && let Err(e) = fsutil::remove_file(&a.path)
        {
            log::warn!("cannot remove imported {}: {e}", a.path.display());
        }
        Ok(Done {
            track_id: track.id,
            upgraded: matches!(decision, Decision::Upgrade(_)),
            master,
        })
    })();
    match finish {
        Ok(done) => Ok(Committed::Imported(done)),
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
