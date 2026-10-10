//! Import, Inbox, the confirmation queue, Trash and lyrics: the parts of the
//! runtime that run on tokio. Results come back to the worker as [`Internal`]
//! messages, so the worker stays the only writer of the view state.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::model::{SourceId, SourceOrigin, TrackId};
use nikgob_import::{ImportConfig, ImportEvent, Importer, InboxWatcher, ItemId, Stage};
use nikgob_metadata::{Metadata, MetadataConfig};
use nikgob_view::{self as v, Command};

use crate::Worker;

/// What the app hands the runtime to enable importing.
pub struct ImportSetup {
    pub config: ImportConfig,
    /// None = offline: identification from tags only, no lyrics lookups.
    pub metadata: Option<MetadataConfig>,
    /// Watch `Inbox/` for dropped files.
    pub watch_inbox: bool,
    /// Where the torrent client keeps its session.
    pub torrent_state_dir: std::path::PathBuf,
    /// No DHT, listeners, UPnP or local discovery (tests).
    pub torrent_offline: bool,
}

/// Messages from background tasks to the worker.
pub(crate) enum Internal {
    Import(ImportEvent),
    Resolved { item: u64, error: Option<String> },
    Lyrics(TrackId, v::LyricsView),
    TagSuggestions(TrackId, Result<Vec<(String, v::TagEdit)>, String>),
    Retagged(TrackId, Result<(), String>),
    TorrentResults(String, Result<Vec<nikgob_core::indexer::Candidate>, String>),
    TorrentStarted(Result<nikgob_torrent::TorrentClient, String>),
    TorrentError(String),
    #[allow(clippy::type_complexity)]
    PayloadFetched(
        usize,
        Result<nikgob_core::indexer::TorrentPayload, String>,
        Box<dyn FnOnce(&mut Worker, nikgob_core::indexer::TorrentPayload) + Send>,
    ),
    TorrentFilesListed(usize, Result<nikgob_torrent::TorrentMeta, String>),
    #[allow(clippy::type_complexity)]
    TorrentAdded(Result<(String, String, Option<Vec<usize>>, Option<String>), String>),
    SeedStopped(Vec<nikgob_torrent::TorrentStatus>),
}

/// Bits above this mark ids of import jobs in the shared "Загрузки" lists;
/// torrent jobs use small ids.
pub(crate) const IMPORT_ID: u64 = 1 << 48;

struct Progress {
    title: String,
    files: usize,
    done: usize,
    imported: usize,
    duplicates: usize,
    failed: usize,
    parked: usize,
    current: Option<(String, Stage)>,
    last_error: Option<String>,
}

/// One file the user trashed, for "Отменить".
pub(crate) struct Trashed {
    track: TrackId,
    files: Vec<(SourceId, PathBuf, PathBuf)>,
}

pub(crate) struct ImportService {
    rt: tokio::runtime::Runtime,
    importer: Importer,
    metadata: Option<Metadata>,
    watch_inbox: bool,
    watcher: Option<InboxWatcher>,
    internal: Sender<Internal>,
    jobs: BTreeMap<u64, Progress>,
    titles: HashMap<u64, String>,
    /// Jobs started but whose `Started` event has not reached the worker yet.
    launching: HashMap<u64, Instant>,
    /// Parked items being imported right now (already out of `pending`).
    resolving: usize,
    history: VecDeque<v::Job>,
    dirty: bool,
    last_reload: Instant,
    /// Lyrics looked up this session (found or not): no second request.
    lyrics_tried: HashSet<TrackId>,
    lyrics_cache: HashMap<TrackId, v::LyricsView>,
    pub(crate) last_trashed: Option<Trashed>,
    /// New settings, applied once nothing is importing.
    reconfigure: Option<ImportConfig>,
}

impl ImportService {
    pub(crate) fn start(
        setup: ImportSetup,
        library: nikgob_library::Library,
        internal: Sender<Internal>,
    ) -> Option<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("nikgob-import")
            .enable_all()
            .build()
            .map_err(|e| log::error!("import runtime: {e}"))
            .ok()?;
        let metadata = setup.metadata.as_ref().and_then(|c| {
            Metadata::new(c)
                .map_err(|e| log::warn!("online metadata disabled: {e}"))
                .ok()
        });
        let importer = Importer::new(setup.config, library.clone(), metadata.clone());
        let mut svc = Self {
            rt,
            importer,
            metadata,
            watch_inbox: setup.watch_inbox,
            watcher: None,
            internal,
            jobs: BTreeMap::new(),
            titles: HashMap::new(),
            launching: HashMap::new(),
            resolving: 0,
            history: VecDeque::new(),
            dirty: false,
            last_reload: Instant::now(),
            lyrics_tried: HashSet::new(),
            lyrics_cache: HashMap::new(),
            last_trashed: None,
            reconfigure: None,
        };
        svc.attach();

        // Trash older than 30 days goes for good, once per start.
        let lib = library;
        svc.rt.spawn_blocking(move || match nikgob_import::trash_purge(&lib, nikgob_import::TRASH_DAYS) {
            Ok(r) if r.rows > 0 => log::info!("trash purge: {r:?}"),
            Ok(_) => {}
            Err(e) => log::warn!("trash purge: {e}"),
        });
        Some(svc)
    }

    /// Forwards the importer's events and starts the Inbox watcher.
    fn attach(&mut self) {
        let mut sub = self.importer.subscribe();
        let tx = self.internal.clone();
        self.rt.spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match sub.recv().await {
                    Ok(ev) => {
                        if tx.send(Internal::Import(ev)).is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(n)) => log::warn!("import events lagged by {n}"),
                    Err(RecvError::Closed) => break,
                }
            }
        });
        if self.watch_inbox {
            let _guard = self.rt.enter();
            self.watcher = self
                .importer
                .watch_inbox()
                .map_err(|e| log::warn!("Inbox watcher: {e}"))
                .ok();
        }
    }

    pub(crate) fn importer(&self) -> &Importer {
        &self.importer
    }

    pub(crate) fn metadata(&self) -> Option<Metadata> {
        self.metadata.clone()
    }

    pub(crate) fn internal(&self) -> Sender<Internal> {
        self.internal.clone()
    }

    pub(crate) fn spawn<F>(&self, f: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.rt.spawn(f);
    }

    pub(crate) fn spawn_blocking(&self, f: impl FnOnce() + Send + 'static) {
        self.rt.spawn_blocking(f);
    }

    pub(crate) fn config(&self) -> &ImportConfig {
        self.importer.config()
    }

    pub(crate) fn block_on<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    pub(crate) fn idle(&self) -> bool {
        self.jobs.is_empty() && self.importer.pending().is_empty()
    }

    /// Nothing is reading files: no job, no item waiting for the user, and no
    /// work in the gaps between those (a job whose `Started` has not arrived
    /// yet, a parked item being resolved). Payload cleanup waits for this;
    /// deleting a file the importer is about to read would lose it, and with
    /// it a lossless master.
    pub(crate) fn quiet(&self) -> bool {
        let stale = Duration::from_secs(300);
        self.idle()
            && self.resolving == 0
            && self.launching.values().all(|at| at.elapsed() > stale)
    }

    fn apply_reconfigure(&mut self) {
        if !self.idle() {
            return;
        }
        let Some(cfg) = self.reconfigure.take() else { return };
        self.watcher = None;
        let lib = self.importer.library().clone();
        self.importer = Importer::new(cfg, lib, self.metadata.clone());
        self.attach();
        log::info!("importer reconfigured");
    }

    fn view(&self) -> (Vec<v::Job>, Vec<v::Confirm>, Vec<v::Job>) {
        let active = self
            .jobs
            .iter()
            .map(|(&id, p)| {
                let subtitle = match &p.current {
                    Some((file, stage)) => format!("{} · {}", stage_label(*stage), file),
                    None => "Подготовка…".to_owned(),
                };
                v::Job {
                    id: IMPORT_ID | id,
                    title: format!("{} · {} из {}", p.title, p.done, p.files.max(p.done)),
                    subtitle,
                    progress: (p.files > 0).then(|| p.done as f32 / p.files as f32),
                    rate: None,
                    eta: None,
                    paused: false,
                    error: p.last_error.clone(),
                }
            })
            .collect();
        let confirm = self
            .importer
            .pending()
            .into_iter()
            .map(|item| {
                let file = item
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let title = match (&item.tag_title, &item.tag_artist) {
                    (Some(t), Some(a)) => format!("{t} — {a}"),
                    (Some(t), None) => t.clone(),
                    _ => file.clone(),
                };
                let mut origin = format!("Файл: {file}");
                if let Some(ms) = item.duration_ms {
                    origin.push_str(&format!(" · {}", fmt_duration(Duration::from_millis(ms.into()))));
                }
                v::Confirm {
                    id: IMPORT_ID | item.id.0,
                    title,
                    origin,
                    options: item
                        .options
                        .iter()
                        .enumerate()
                        .map(|(i, o)| {
                            let mut detail = Vec::new();
                            if let Some(a) = &o.album {
                                detail.push(match o.year {
                                    Some(y) => format!("{a} ({y})"),
                                    None => a.clone(),
                                });
                            }
                            if let Some(n) = o.track_no {
                                detail.push(format!("№{n}"));
                            }
                            detail.push(format!("совпадение {:.0}%", o.score * 100.0));
                            detail.push(match o.source {
                                "musicbrainz" => "MusicBrainz".to_owned(),
                                "deezer" => "Deezer".to_owned(),
                                other => other.to_owned(),
                            });
                            v::ConfirmOption {
                                label: format!("{} — {}", o.title, o.artist),
                                detail: detail.join(" · "),
                                recommended: i == 0,
                            }
                        })
                        .collect(),
                    skip_label: "Оставить теги файла".into(),
                    accept_label: "Импортировать".into(),
                }
            })
            .collect();
        (active, confirm, self.history.iter().cloned().collect())
    }
}

fn stage_label(s: Stage) -> &'static str {
    match s {
        Stage::Probing => "Чтение",
        Stage::Fingerprinting => "Отпечаток",
        Stage::Identifying => "Опознание",
        Stage::QualityCheck => "Проверка качества",
        Stage::Deciding => "Решение",
        Stage::Converting => "Конвертация",
        Stage::Tagging => "Теги",
        Stage::Placing => "Раскладка",
        Stage::Deriving => "Палитра",
    }
}

fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

impl Worker {
    pub(crate) fn publish_downloads(&mut self) {
        let Some(svc) = &self.import else { return };
        let (active, confirm, history) = svc.view();
        self.publish(|s, _| {
            s.downloads.active.retain(|j| j.id & IMPORT_ID == 0);
            s.downloads.active.extend(active);
            s.downloads.confirm.retain(|c| c.id & IMPORT_ID == 0);
            s.downloads.confirm.extend(confirm);
            s.downloads.history.retain(|j| j.id & IMPORT_ID == 0);
            s.downloads.history.extend(history);
        });
    }

    pub(crate) fn import_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let title = if paths.len() == 1 {
            format!("Импорт «{}»", file_name(&paths[0]))
        } else {
            format!("Импорт {} объектов", paths.len())
        };
        if self.import_with_origin(paths, title, SourceOrigin::Inbox) {
            self.toast("Импорт начался · смотри «Загрузки»", None);
        }
    }

    pub(crate) fn import_with_origin(&mut self, paths: Vec<PathBuf>, title: String, origin: SourceOrigin) -> bool {
        let Some(svc) = &mut self.import else {
            self.toast("Импорт недоступен", None);
            return false;
        };
        if paths.is_empty() {
            return false;
        }
        let job = {
            let _guard = svc.rt.enter();
            svc.importer.import_paths(paths, origin)
        };
        svc.titles.insert(job.id.0, title);
        svc.launching.insert(job.id.0, Instant::now());
        // Events arrive through the broadcast subscription; the per-job
        // channel is not needed.
        drop(job);
        true
    }

    pub(crate) fn confirm_choice(&mut self, id: u64, option: Option<usize>) {
        let Some(svc) = &mut self.import else { return };
        let item = id & !IMPORT_ID;
        svc.resolving += 1;
        let importer = svc.importer.clone();
        let tx = svc.internal.clone();
        svc.rt.spawn(async move {
            let error = importer
                .resolve(ItemId(item), option)
                .await
                .err()
                .map(|e| e.to_string());
            let _ = tx.send(Internal::Resolved { item, error });
        });
    }

    pub(crate) fn internal(&mut self, msg: Internal) {
        match msg {
            Internal::Import(ev) => self.import_event(ev),
            Internal::Resolved { item, error } => {
                if let Some(svc) = &mut self.import {
                    svc.resolving = svc.resolving.saturating_sub(1);
                }
                if let Some(e) = error {
                    log::warn!("resolving item {item}: {e}");
                    self.toast("Не получилось импортировать файл", None);
                }
                self.reload_after_import();
                self.publish_downloads();
            }
            m @ (Internal::TagSuggestions(..) | Internal::Retagged(..)) => self.tag_result(m),
            Internal::TorrentResults(q, r) => self.torrent_results(q, r),
            Internal::TorrentStarted(r) => self.torrent_started(r),
            Internal::TorrentError(e) => {
                log::warn!("torrent: {e}");
                self.toast(format!("Торрент: {e}"), None);
            }
            Internal::PayloadFetched(i, r, then) => self.payload_fetched(i, r, then),
            Internal::TorrentFilesListed(i, r) => self.files_listed(i, r),
            Internal::TorrentAdded(r) => self.torrent_added(r),
            Internal::SeedStopped(list) => self.seed_stopped(list),
            Internal::Lyrics(track, lyrics) => {
                if let Some(svc) = &mut self.import {
                    svc.lyrics_cache.insert(track, lyrics.clone());
                }
                self.publish(|s, _| {
                    if s.lyrics.track == Some(track) {
                        s.lyrics = lyrics;
                    }
                });
            }
        }
    }

    fn import_event(&mut self, ev: ImportEvent) {
        let Some(svc) = &mut self.import else { return };
        let mut finished = None;
        match ev {
            ImportEvent::Started { job, files } => {
                svc.launching.remove(&job.0);
                let title = svc
                    .titles
                    .remove(&job.0)
                    .unwrap_or_else(|| "Импорт из Inbox".to_owned());
                svc.jobs.insert(
                    job.0,
                    Progress {
                        title,
                        files,
                        done: 0,
                        imported: 0,
                        duplicates: 0,
                        failed: 0,
                        parked: 0,
                        current: None,
                        last_error: None,
                    },
                );
            }
            ImportEvent::FileProgress { job, path, stage } => {
                if let Some(p) = svc.jobs.get_mut(&job.0) {
                    p.current = Some((file_name(&path), stage));
                }
            }
            ImportEvent::Imported { job, .. } => {
                if let Some(p) = svc.jobs.get_mut(&job.0) {
                    p.done += 1;
                    p.imported += 1;
                }
                svc.dirty = true;
            }
            ImportEvent::Duplicate { job, .. } => {
                if let Some(p) = svc.jobs.get_mut(&job.0) {
                    p.done += 1;
                    p.duplicates += 1;
                }
            }
            ImportEvent::Failed { job, path, error } => {
                log::warn!("import of {} failed: {error}", path.display());
                if let Some(p) = svc.jobs.get_mut(&job.0) {
                    p.done += 1;
                    p.failed += 1;
                    p.last_error = Some(format!("{}: {error}", file_name(&path)));
                }
            }
            ImportEvent::NeedsConfirmation { item } => {
                if let Some(p) = svc.jobs.get_mut(&item.job.0) {
                    p.done += 1;
                    p.parked += 1;
                }
            }
            ImportEvent::Finished {
                job,
                imported,
                duplicates,
                failed,
                pending,
            } => {
                let title = svc
                    .jobs
                    .remove(&job.0)
                    .map_or_else(|| "Импорт".to_owned(), |p| p.title);
                let mut parts = vec![format!("добавлено {imported}")];
                if duplicates > 0 {
                    parts.push(format!("дублей {duplicates}"));
                }
                if failed > 0 {
                    parts.push(format!("ошибок {failed}"));
                }
                if pending > 0 {
                    parts.push(format!("ждут подтверждения {pending}"));
                }
                let summary = parts.join(" · ");
                svc.history.push_front(v::Job {
                    id: IMPORT_ID | job.0,
                    title,
                    subtitle: summary.clone(),
                    progress: Some(1.0),
                    rate: None,
                    eta: None,
                    paused: false,
                    error: (failed > 0).then(|| format!("Не удалось: {failed}")),
                });
                svc.history.truncate(100);
                finished = Some((summary, pending));
            }
            ImportEvent::TorrentFound(path) => {
                self.command(Command::AddTorrentFile(path));
                return;
            }
            ImportEvent::MasterRegistered(m) => {
                log::info!("master registered: {} ({} bytes)", m.file_hash, m.size_bytes);
            }
        }
        if let Some((summary, pending)) = finished {
            self.reload_after_import();
            let _ = pending;
            self.toast(format!("Импорт: {summary}"), None);
            if let Some(svc) = &mut self.import {
                svc.apply_reconfigure();
            }
        } else if self
            .import
            .as_ref()
            .is_some_and(|s| s.dirty && s.last_reload.elapsed() > Duration::from_secs(3))
        {
            self.reload_after_import();
        }
        self.publish_downloads();
    }

    /// New tracks: drop cached covers, reload what is on screen.
    pub(crate) fn reload_after_import(&mut self) {
        if let Some(svc) = &mut self.import {
            svc.dirty = false;
            svc.last_reload = Instant::now();
        }
        self.res.invalidate();
        self.load_library();
        self.load_home();
        let (release, artist, playlist) = {
            let s = self.state.read();
            (
                s.release.as_ref().map(|r| r.card.id),
                s.artist.as_ref().map(|a| a.card.id),
                s.playlist.as_ref().map(|p| p.card.id),
            )
        };
        if let Some(id) = release {
            self.load_release(id);
        }
        if let Some(id) = artist {
            self.load_artist(id);
        }
        if let Some(id) = playlist {
            self.load_playlist(id);
        }
    }

    // ------------------------------------------------------------- trash

    pub(crate) fn trash_track(&mut self, track: TrackId) {
        let Some(svc) = &self.import else {
            self.toast("Удаление недоступно", None);
            return;
        };
        let cfg = svc.config().clone();
        if self.playing.map(|e| e.track) == Some(track) {
            self.command(Command::Next);
        }
        let before: Vec<_> = self
            .lib
            .sources_of_track(track)
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.kind.is_local())
            .collect();
        match nikgob_import::delete_track_to_trash(&self.lib, &cfg.trash_dir, &cfg.library_dir, track) {
            Ok(_) => {
                // Pair each source with where its file went, as recorded in the
                // library. (Pairing by order would hand a source whose file was
                // already missing the next source's file.)
                let trash = self.lib.list_trash().unwrap_or_default();
                let mut files = Vec::new();
                for s in before {
                    let dest = trash
                        .iter()
                        .find(|t| t.source.id == s.id)
                        .and_then(|t| t.trash_path.clone());
                    if let Some(dest) = dest {
                        files.push((s.id, PathBuf::from(&s.locator), PathBuf::from(dest)));
                    }
                }
                if let Some(svc) = &mut self.import {
                    svc.last_trashed = Some(Trashed { track, files });
                }
                self.toast(
                    "Перемещено в корзину (30 дней)",
                    Some(("Отменить".into(), Command::UndoTrash(track))),
                );
                self.reload_after_import();
            }
            Err(e) => {
                log::error!("trash {track}: {e}");
                self.toast("Не получилось удалить", None);
            }
        }
    }

    pub(crate) fn undo_trash(&mut self, track: TrackId) {
        let Some(t) = self
            .import
            .as_mut()
            .and_then(|s| s.last_trashed.take_if(|t| t.track == track))
        else {
            return;
        };
        let mut ok = true;
        for (source, orig, dest) in t.files {
            if let Some(dir) = orig.parent() {
                let _ = std::fs::create_dir_all(nikgob_import::sanitize::long_path(dir));
            }
            // Something new took the old path (an upgrade): never overwrite it.
            let taken = nikgob_import::sanitize::long_path(&orig).exists();
            let moved = !taken && nikgob_import::fsutil::move_file(&dest, &orig).is_ok();
            let lrc = dest.with_extension("lrc");
            if moved && nikgob_import::sanitize::long_path(&lrc).exists() {
                let _ = nikgob_import::fsutil::move_file(&lrc, &orig.with_extension("lrc"));
            }
            ok &= moved && self.lib.restore_source(source).is_ok();
        }
        self.toast(if ok { "Восстановлено" } else { "Восстановить удалось не всё" }, None);
        self.reload_after_import();
    }

    // ------------------------------------------------------------- settings

    /// Import-related prefs changed: rebuild the importer when it is idle.
    pub(crate) fn import_prefs(&mut self, p: &v::Prefs) {
        let Some(svc) = &mut self.import else { return };
        let old = svc.reconfigure.as_ref().unwrap_or(svc.config());
        let mut cfg = old.clone();
        cfg.opus_bitrate_kbps = p.opus_bitrate;
        cfg.check_fake_lossless = p.check_fake_lossless;
        cfg.fallback_seed_argb = p.fallback_seed;
        if !p.library_dir.as_os_str().is_empty() {
            cfg.library_dir = p.library_dir.clone();
        }
        if !p.inbox_dir.as_os_str().is_empty() {
            cfg.inbox_dir = p.inbox_dir.clone();
        }
        let same = cfg.opus_bitrate_kbps == old.opus_bitrate_kbps
            && cfg.check_fake_lossless == old.check_fake_lossless
            && cfg.fallback_seed_argb == old.fallback_seed_argb
            && cfg.library_dir == old.library_dir
            && cfg.inbox_dir == old.inbox_dir;
        if !same {
            svc.reconfigure = Some(cfg);
            svc.apply_reconfigure();
        }
    }

    /// The MusicBrainz contact or AcoustID key changed.
    pub(crate) fn set_metadata_config(&mut self, cfg: &MetadataConfig) {
        let Some(svc) = &mut self.import else { return };
        svc.metadata = Metadata::new(cfg)
            .map_err(|e| log::warn!("online metadata disabled: {e}"))
            .ok();
        svc.reconfigure = Some(svc.reconfigure.take().unwrap_or_else(|| svc.config().clone()));
        svc.apply_reconfigure();
    }

    // ------------------------------------------------------------- lyrics & tags

    /// Lyrics and file tags of the track that just started.
    pub(crate) fn track_details(&mut self, track: TrackId) {
        let src = self
            .lib
            .active_source(track)
            .ok()
            .flatten()
            .filter(|s| s.kind.is_local());
        let path = src.as_ref().map(|s| self.res.path(&s.locator));
        let tags = path
            .as_deref()
            .and_then(|p| nikgob_import::tags::read_tags(p).ok())
            .map(|(t, _)| t);

        // Info tab: what the file says beyond the basics.
        if let Some(t) = &tags {
            let mut extra = Vec::new();
            let mut add = |k: &str, v: Option<String>| {
                if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
                    extra.push((k.to_owned(), v));
                }
            };
            add("Исполнитель альбома", t.album_artist.clone());
            add("Год", t.year.map(|y| y.to_string()));
            add("Жанр", t.genre.clone());
            add("ISRC", t.isrc.clone());
            add("MusicBrainz", t.recording_mbid.clone());
            add(
                "Громкость",
                t.r128_track_gain
                    .map(|q| format!("R128 {:+.1} dB", f32::from(q) / 256.0))
                    .or_else(|| t.replaygain_track_gain_db.map(|g| format!("ReplayGain {g:+.1} dB"))),
            );
            self.publish(|s, _| {
                if s.info.track == Some(track) {
                    s.info.tags.extend(extra);
                }
            });
        }

        // Lyrics: cache → .lrc next to the file → tags → LRCLIB.
        if let Some(l) = self.import.as_ref().and_then(|s| s.lyrics_cache.get(&track)).cloned() {
            self.publish(|s, _| s.lyrics = l);
            return;
        }
        let local = path.as_deref().and_then(|p| {
            let lrc = nikgob_import::sanitize::long_path(&p.with_extension("lrc"));
            if let Ok(text) = std::fs::read_to_string(&lrc) {
                let lines = nikgob_metadata::parse_lrc(&text);
                if !lines.is_empty() {
                    return Some(v::LyricsView {
                        track: Some(track),
                        lines,
                        plain: None,
                        loading: false,
                    });
                }
            }
            let text = tags.as_ref()?.lyrics.clone()?;
            let lines = nikgob_metadata::parse_lrc(&text);
            Some(v::LyricsView {
                track: Some(track),
                plain: lines.is_empty().then_some(text),
                lines,
                loading: false,
            })
        });
        if let Some(l) = local {
            self.publish(|s, _| s.lyrics = l);
            return;
        }
        let row = self.lib.get_track_row(track).ok().flatten();
        let lookup = self.import.as_mut().and_then(|svc| {
            let meta = svc.metadata.clone()?;
            let row = row?;
            svc.lyrics_tried.insert(track).then_some((svc, meta, row))
        });
        let Some((svc, meta, row)) = lookup else {
            self.publish(|s, _| {
                s.lyrics = v::LyricsView {
                    track: Some(track),
                    ..Default::default()
                };
            });
            return;
        };
        let tx = svc.internal.clone();
        let lrc_path = path.map(|p| p.with_extension("lrc"));
        svc.rt.spawn(async move {
            let duration = row.duration_ms.map(|ms| Duration::from_millis(ms.into()));
            let found = tokio::time::timeout(
                Duration::from_secs(15),
                meta.lrclib.find(&row.artist, &row.title, row.album.as_deref(), duration),
            )
            .await;
            let mut view = v::LyricsView {
                track: Some(track),
                ..Default::default()
            };
            if let Ok(Ok(Some(l))) = found {
                if let Some(text) = l.synced_lyrics.as_deref().filter(|_| l.has_synced()) {
                    view.lines = nikgob_metadata::parse_lrc(text);
                    if let Some(p) = &lrc_path {
                        let _ = std::fs::write(nikgob_import::sanitize::long_path(p), text);
                    }
                }
                if view.lines.is_empty() {
                    view.plain = l.plain_lyrics.filter(|p| !p.trim().is_empty());
                }
            }
            let _ = tx.send(Internal::Lyrics(track, view));
        });
        self.publish(|s, _| {
            s.lyrics = v::LyricsView {
                track: Some(track),
                loading: true,
                ..Default::default()
            };
        });
    }
}

/// Track gain from the file's tags, normalized to −18 LUFS (ReplayGain 2.0
/// reference; R128 gains are relative to −23 LUFS).
pub(crate) fn file_gain_db(path: &Path) -> f32 {
    let Ok((t, _)) = nikgob_import::tags::read_tags(path) else {
        return 0.0;
    };
    t.r128_track_gain
        .map(|q| f32::from(q) / 256.0 + 5.0)
        .or(t.replaygain_track_gain_db)
        .unwrap_or(0.0)
}
