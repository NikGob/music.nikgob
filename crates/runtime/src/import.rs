//! Import, Inbox, the confirmation queue, Trash and lyrics: the parts of the
//! runtime that run on tokio. Results come back to the worker as [`Internal`]
//! messages, so the worker stays the only writer of the view state.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use nikgob_core::lyrics::LyricsSource;
use nikgob_core::model::{SourceOrigin, TrackId, WishlistId};
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
    /// Where the lyrics models are downloaded (docs/lyrics-align.md);
    /// None = no aligner.
    pub models_dir: Option<std::path::PathBuf>,
}

impl Internal {
    /// What the worker's loop is busy with while it handles this.
    pub(crate) fn turn_name(&self) -> String {
        match self {
            Self::Import(_) => "an import event".into(),
            Self::Stream(_) => "a streaming answer".into(),
            Self::Analysis(_) => "the sound analysis".into(),
            Self::Recs(m) => format!("the recommendations ({})", m.turn_name()),
            Self::LyricsFetched(..) | Self::LyricsEmbedded(..) | Self::Align(_) => "lyrics".into(),
            other => format!("a background answer ({:?})", std::mem::discriminant(other)),
        }
    }
}

/// Messages from background tasks to the worker.
pub(crate) enum Internal {
    Import(ImportEvent),
    Resolved {
        item: u64,
        /// The «Хочу скачать» item the file was for.
        wishlist: Option<WishlistId>,
        error: Option<String>,
    },
    LyricsFetched(TrackId, LyricsSource, Result<Option<crate::lyrics::Text>, String>),
    LyricsEmbedded(TrackId, Result<String, String>),
    /// From the lyrics aligner's thread.
    Align(crate::align::Msg),
    Enrich(crate::enrich::Event),
    TagSuggestions(TrackId, Result<Vec<(String, v::TagEdit)>, String>),
    Retagged(TrackId, Result<(), String>),
    /// An uploader's song took its performer's names.
    Relabeled(TrackId, Result<(), String>),
    /// The file of a track of a wished album followed its new place (its
    /// tags and name): the one-time repair of wished albums' places.
    Placed(TrackId, Result<(), String>),
    TorrentResults(String, Result<Vec<nikgob_core::indexer::Candidate>, String>),
    IndexerChecked(Result<usize, nikgob_core::indexer::IndexerError>),
    TorrentStarted(Result<nikgob_torrent::TorrentClient, String>),
    TorrentError(String),
    #[allow(clippy::type_complexity)]
    PayloadFetched(
        usize,
        Result<nikgob_core::indexer::TorrentPayload, String>,
        Box<dyn FnOnce(&mut Worker, nikgob_core::indexer::TorrentPayload) + Send>,
    ),
    TorrentFilesListed(usize, Result<nikgob_torrent::TorrentMeta, String>),
    TorrentAdded(Result<crate::torrents::Added, String>),
    /// Streaming (stage 3), see [`crate::streams`].
    Stream(crate::streams::Msg),
    /// The background sound analysis, see [`crate::analysis`].
    Analysis(crate::analysis::Msg),
    /// The background spectrum check, see [`crate::quality`].
    Quality(crate::quality::Msg),
    /// Files that hold another song's audio, see [`crate::same_audio`].
    SameAudio(crate::same_audio::Msg),
    /// Recommendations (stage 4), see [`crate::recs`].
    Recs(crate::recs::Msg),
    /// Real art of a stream track's video thumbnail, see
    /// [`crate::stream_covers`].
    StreamCover(crate::stream_covers::Found),
    SeedStopped(Vec<nikgob_torrent::TorrentStatus>),
    /// A look at Soulseek for «Раздача».
    Sharing(crate::sharing::SlskPoll),
    MastersMoveProgress {
        job: u64,
        done: usize,
        total: usize,
        file: String,
    },
    MastersMoved {
        job: u64,
        old_dir: PathBuf,
        new_dir: PathBuf,
        result: Result<nikgob_import::RelocateReport, String>,
    },
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
    /// Titles of the files that held another song's audio.
    wrong: Vec<String>,
}

/// The error line of a finished import: which files held another song
/// (said by title), and how many failed otherwise.
pub(crate) fn failed_words(failed: usize, wrong: &[String]) -> Option<String> {
    if failed == 0 && wrong.is_empty() {
        return None;
    }
    let titles = wrong.iter().map(|t| format!("«{t}»")).collect::<Vec<_>>().join(", ");
    let other = failed.saturating_sub(wrong.len());
    Some(match (wrong.is_empty(), other) {
        (true, n) => format!("Не удалось: {n}"),
        (false, 0) => format!("В файле другая песня: {titles}"),
        (false, n) => format!("В файле другая песня: {titles} · не удалось ещё: {n}"),
    })
}

/// Ids of history entries loaded from the job log (above any job id of
/// this session).
pub(crate) const LOGGED_ID: u64 = 1 << 40;

/// A persisted job as a history row; `kind_bit` is [`IMPORT_ID`] or 0.
pub(crate) fn logged_job(e: &nikgob_library::JobLogEntry, kind_bit: u64) -> v::Job {
    v::Job {
        id: kind_bit | LOGGED_ID | e.seq.max(0) as u64,
        title: e.title.clone(),
        // Logged before it was put in the past tense: the files were long
        // confirmed, the line still said they wait.
        subtitle: e.subtitle.replace("ждут подтверждения", PARKED),
        progress: Some(1.0),
        rate: None,
        eta: None,
        paused: false,
        controllable: false,
        pausable: false,
        error: e.error.clone(),
        at: e.at,
    }
}

/// A finished import's files that went to «Подтверди» (a history line, so
/// in the past: they may be long confirmed).
const PARKED: &str = "отправлено на подтверждение";

/// "История" newest first: the torrents, imports and service imports are
/// kept apart and each part is spliced in on its own.
pub(crate) fn sort_history(history: &mut [v::Job]) {
    history.sort_by_key(|j| std::cmp::Reverse(j.at));
}

/// Job-log kind of the one line of a «Хочу скачать» item: `wish:<id>`.
pub(crate) const WISH_KIND: &str = "wish:";

pub(crate) fn wish_kind(id: WishlistId) -> String {
    format!("{WISH_KIND}{id}")
}

/// A line older versions wrote for a wished download on every refusal or
/// retry, each its own ("Tidal: … · не дал скачать: ошибка 403 · попробую
/// в 21:48", "Скачано 1 из 10 — остальное попробую в …").
fn old_wish_line(e: &nikgob_library::JobLogEntry) -> bool {
    e.kind == "import"
        && [e.subtitle.as_str(), e.error.as_deref().unwrap_or_default()]
            .iter()
            .any(|t| ["не дал", "Не дал", "попробую", "поищу снова"].iter().any(|w| t.contains(*w)))
}

/// What a wish's line is about: its title without the source an older
/// line began with ("Tidal: Eve — Underdog" → "Eve — Underdog").
fn wish_key(title: &str) -> &str {
    nikgob_core::source::SourceKey::ALL
        .iter()
        .find_map(|k| title.strip_prefix(k.display_name()).and_then(|t| t.strip_prefix(": ")))
        .unwrap_or(title)
}

/// The history kept across restarts (newest first), one line per wish: of
/// a wish's lines (its own one, or the many older versions wrote) only the
/// newest stays.
pub(crate) fn kept_history(entries: Vec<nikgob_library::JobLogEntry>) -> Vec<nikgob_library::JobLogEntry> {
    let mut seen: HashSet<String> = HashSet::new();
    entries
        .into_iter()
        .filter(|e| {
            if !e.kind.starts_with(WISH_KIND) && !old_wish_line(e) {
                return true;
            }
            seen.insert(wish_key(&e.title).to_owned())
        })
        .collect()
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
    /// Jobs of «Хочу скачать» items: the item settles when its job ends.
    wish_jobs: HashMap<u64, WishlistId>,
    /// Parked items being imported right now (already out of `pending`).
    resolving: usize,
    history: VecDeque<v::Job>,
    /// The history line of each «Хочу скачать» item that has one, by its
    /// job-log kind ([`wish_kind`]): a new state replaces it.
    wish_lines: HashMap<String, u64>,
    dirty: bool,
    last_reload: Instant,
    /// Lyrics lookups on their way: a source is asked once at a time.
    pub(crate) lyrics_asking: HashSet<(TrackId, LyricsSource)>,
    /// Background lyrics backfill and artist photos.
    pub(crate) enrich: crate::enrich::Enrich,
    pub(crate) lyrics_cache: HashMap<TrackId, crate::lyrics::TrackLyrics>,
    /// `lyrics_cache` keys, least recently stored first.
    pub(crate) lyrics_order: VecDeque<TrackId>,
    /// Lyrics to write into files that were playing (or preloaded) when the
    /// text settled: rewriting a file under the decoder shifts its pages.
    pub(crate) embed_later: HashSet<TrackId>,
    /// New settings, applied once nothing is importing.
    reconfigure: Option<ImportConfig>,
    next_move_id: u64,
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
        // Albums imported before folders were imported as a whole may be in pieces.
        match nikgob_import::merge_split_releases(&library) {
            Ok(0) => {}
            Ok(n) => log::info!("merged {n} split releases"),
            Err(e) => log::warn!("merging split releases: {e}"),
        }
        let importer = Importer::new(setup.config, library.clone(), metadata.clone());
        // Once: albums a torrent brought in pieces (each file under a
        // stranger) go back together, files moved with them.
        match importer.repair_torrent_identities(true) {
            Ok(r) if !r.moved.is_empty() || !r.folded.is_empty() => log::info!(
                "torrent albums repaired: {} tracks back, {} releases folded, {} files moved",
                r.moved.len(),
                r.folded.len(),
                r.files_moved
            ),
            Ok(_) => {}
            Err(e) => log::warn!("repairing torrent albums: {e}"),
        }
        // Once: the composers and guests the first rule of that repair
        // replaced with the album artist get their own names back.
        match importer.repair_torrent_artists(true) {
            Ok(b) if !b.is_empty() => log::info!("torrent tracks given their own artist back: {}", b.len()),
            Ok(_) => {}
            Err(e) => log::warn!("giving torrent tracks their artists back: {e}"),
        }
        // Finished imports survive a restart ("Загрузки → История"), and
        // so does the latest line of each wished download.
        let mut kept = kept_history(library.job_log_with("import", WISH_KIND, 200).unwrap_or_default());
        kept.truncate(100);
        let wish_lines: HashMap<String, u64> = kept
            .iter()
            .filter(|e| e.kind.starts_with(WISH_KIND))
            .map(|e| (e.kind.clone(), logged_job(e, IMPORT_ID).id))
            .collect();
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
            wish_jobs: HashMap::new(),
            resolving: 0,
            history: kept.iter().map(|e| logged_job(e, IMPORT_ID)).collect(),
            wish_lines,
            dirty: false,
            last_reload: Instant::now(),
            lyrics_asking: HashSet::new(),
            enrich: Default::default(),
            lyrics_cache: HashMap::new(),
            lyrics_order: VecDeque::new(),
            embed_later: HashSet::new(),
            reconfigure: None,
            next_move_id: 0,
        };
        svc.attach();

        // Trash older than 30 days goes for good, once per start: deleted
        // tracks first (with their files and masters), then replaced files.
        let lib = library;
        let cfg = svc.config().clone();
        svc.rt.spawn_blocking(move || {
            match nikgob_import::purge_deleted_tracks(&lib, &cfg, nikgob_import::TRASH_DAYS) {
                Ok(r) if r.rows > 0 => log::info!("deleted tracks purge: {r:?}"),
                Ok(_) => {}
                Err(e) => log::warn!("deleted tracks purge: {e}"),
            }
            match nikgob_import::trash_purge(&lib, nikgob_import::TRASH_DAYS) {
                Ok(r) if r.rows > 0 => log::info!("trash purge: {r:?}"),
                Ok(_) => {}
                Err(e) => log::warn!("trash purge: {e}"),
            }
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

    /// Files waiting in «Подтверди».
    pub(crate) fn pending(&self) -> Vec<nikgob_import::ConfirmItem> {
        self.importer.pending()
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
                    // Cancelled, what it added goes back; never paused.
                    controllable: true,
                    pausable: false,
                    error: p.last_error.clone(),
                    at: 0,
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
                // What the file really is («FLAC из MP3 ~320»).
                if let Some(q) = &item.quality {
                    origin.push_str(&format!(" · {q}"));
                }
                // The file's own tags come first; the best other answer is
                // the one to take.
                let recommended = item
                    .options
                    .iter()
                    .position(|o| o.source != nikgob_import::events::TAGS_OPTION)
                    .unwrap_or(0);
                v::Confirm {
                    yes_no: false,
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
                            } else if let Some(y) = o.year {
                                detail.push(y.to_string());
                            }
                            if let Some(n) = o.track_no {
                                detail.push(format!("№{n}"));
                            }
                            if let Some(ms) = o.duration_ms {
                                detail.push(fmt_duration(Duration::from_millis(ms.into())));
                            }
                            match o.source {
                                nikgob_import::events::TAGS_OPTION => detail.push("теги файла".to_owned()),
                                nikgob_import::events::STREAM_OPTION => {
                                    detail.push("лайкнутый стрим-трек, другая длина".to_owned());
                                }
                                source => {
                                    detail.push(format!("совпадение {:.0}%", o.score * 100.0));
                                    detail.push(match source {
                                        "musicbrainz" => "MusicBrainz".to_owned(),
                                        "deezer" => "Deezer".to_owned(),
                                        other => other.to_owned(),
                                    });
                                }
                            }
                            let artist = if o.artist.is_empty() { "исполнитель неизвестен" } else { o.artist.as_str() };
                            v::ConfirmOption {
                                label: format!("{} — {artist}", o.title),
                                detail: detail.join(" · "),
                                recommended: i == recommended,
                            }
                        })
                        .collect(),
                    // The file's own tags are an option of their own.
                    skip_label: String::new(),
                    accept_label: "Импортировать".into(),
                    pick: None,
                    question: "Это какой трек?".into(),
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
        self.publish(|s| {
            // Cold-start jobs of the streaming part share the id space.
            let others = |id: u64| id & IMPORT_ID == 0 || id & crate::streams::STREAM_JOB != 0;
            s.downloads.active.retain(|j| others(j.id));
            s.downloads.active.extend(active);
            s.downloads.confirm.retain(|c| c.id & IMPORT_ID == 0);
            s.downloads.confirm.extend(confirm);
            s.downloads.history.retain(|j| others(j.id));
            s.downloads.history.extend(history);
            sort_history(&mut s.downloads.history);
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
        self.import_for(paths, title, origin, nikgob_import::ImportTargets::default())
    }

    /// The line of «Загрузки → История» of the wished download `item` (a
    /// source that turned it away, an album that came in part, when it is
    /// tried again): one per item, its latest state replacing the one
    /// before, at the top; kept with the imports' lines over a restart.
    pub(crate) fn wish_line(&mut self, item: WishlistId, title: String, subtitle: String, error: Option<String>) {
        let kind = wish_kind(item);
        let seq = match self.lib.put_job_line(&kind, &title, &subtitle, error.as_deref()) {
            Ok(seq) => seq.max(0) as u64,
            Err(e) => {
                log::warn!("job log: {e}");
                0
            }
        };
        let Some(svc) = &mut self.import else { return };
        let id = IMPORT_ID | LOGGED_ID | seq;
        if let Some(old) = svc.wish_lines.insert(kind, id) {
            svc.history.retain(|j| j.id != old);
        }
        svc.history.push_front(v::Job {
            id,
            title,
            subtitle,
            progress: Some(1.0),
            rate: None,
            eta: None,
            paused: false,
            controllable: false,
            pausable: false,
            error,
            at: nikgob_library::now_ms(),
        });
        svc.history.truncate(100);
        self.publish_downloads();
    }

    /// The wished download `item` came: its line goes (the import's own
    /// line says what came).
    pub(crate) fn wish_line_drop(&mut self, item: WishlistId) {
        let kind = wish_kind(item);
        let Some(svc) = &mut self.import else { return };
        let Some(old) = svc.wish_lines.remove(&kind) else { return };
        svc.history.retain(|j| j.id != old);
        if let Err(e) = self.lib.drop_job_lines(&kind) {
            log::warn!("job log: {e}");
        }
        self.publish_downloads();
    }

    /// An import whose files are meant for `targets` (a wishlist item).
    pub(crate) fn import_for(
        &mut self,
        paths: Vec<PathBuf>,
        title: String,
        origin: SourceOrigin,
        targets: nikgob_import::ImportTargets,
    ) -> bool {
        let Some(svc) = &mut self.import else {
            self.toast("Импорт недоступен", None);
            return false;
        };
        if paths.is_empty() {
            return false;
        }
        let wishlist = targets.wishlist;
        let job = {
            let _guard = svc.rt.enter();
            svc.importer.import_paths_for(paths, origin, targets)
        };
        if let Some(w) = wishlist {
            svc.wish_jobs.insert(job.id.0, w);
        }
        svc.titles.insert(job.id.0, title);
        svc.launching.insert(job.id.0, Instant::now());
        // Events arrive through the broadcast subscription; the per-job
        // channel is not needed.
        drop(job);
        true
    }

    /// «Отменить» on an import's row: the importer stops it and takes back
    /// what it added (the files to the Trash, the rows out, a replaced file
    /// back); its line says so once it stopped.
    pub(crate) fn cancel_import(&mut self, job: u64) {
        let Some(svc) = &self.import else { return };
        if !svc.jobs.contains_key(&job) && !svc.launching.contains_key(&job) {
            return;
        }
        svc.importer.cancel(nikgob_import::JobId(job));
        self.toast("Отменяю импорт — добавленное уберу", None);
    }

    pub(crate) fn confirm_choice(&mut self, id: u64, option: Option<usize>) {
        let Some(svc) = &mut self.import else { return };
        let item = id & !IMPORT_ID;
        svc.resolving += 1;
        let importer = svc.importer.clone();
        let wishlist = importer.pending().into_iter().find(|p| p.id.0 == item).and_then(|p| p.wishlist);
        let tx = svc.internal.clone();
        svc.rt.spawn(async move {
            let error = importer
                .resolve(ItemId(item), option)
                .await
                .err()
                .map(|e| e.to_string());
            let _ = tx.send(Internal::Resolved { item, wishlist, error });
        });
    }

    pub(crate) fn internal(&mut self, msg: Internal) {
        match msg {
            Internal::Import(ev) => self.import_event(ev),
            Internal::Resolved { item, wishlist, error } => {
                if let Some(svc) = &mut self.import {
                    svc.resolving = svc.resolving.saturating_sub(1);
                }
                if let Some(e) = error {
                    log::warn!("resolving item {item}: {e}");
                    self.toast("Не получилось импортировать файл", None);
                }
                if let Some(w) = wishlist {
                    self.wish_settle(w);
                }
                self.reload_after_import();
                self.publish_downloads();
            }
            m @ (Internal::TagSuggestions(..) | Internal::Retagged(..)) => self.tag_result(m),
            Internal::Relabeled(track, r) => {
                if let Err(e) = r {
                    log::warn!("relabelling {track}: {e}");
                }
                self.reload_after_import();
            }
            Internal::Placed(track, r) => {
                if let Err(e) = r {
                    log::warn!("the file of {track} stays as it was: {e}");
                }
                if let Some(svc) = &mut self.import {
                    svc.dirty = true;
                }
            }
            Internal::TorrentResults(q, r) => self.torrent_results(q, r),
            Internal::IndexerChecked(r) => self.indexer_checked(r),
            Internal::TorrentStarted(r) => self.torrent_started(r),
            Internal::TorrentError(e) => {
                log::warn!("torrent: {e}");
                self.toast(format!("Торрент: {e}"), None);
            }
            Internal::PayloadFetched(i, r, then) => self.payload_fetched(i, r, then),
            Internal::TorrentFilesListed(i, r) => self.files_listed(i, r),
            Internal::TorrentAdded(r) => self.torrent_added(r),
            Internal::Stream(m) => self.stream_msg(m),
            Internal::Analysis(m) => self.analysis_msg(m),
            Internal::Quality(m) => self.quality_msg(m),
            Internal::SameAudio(m) => self.same_audio_msg(m),
            Internal::Recs(m) => self.recs_msg(m),
            Internal::StreamCover(f) => self.real_cover_found(f),
            Internal::SeedStopped(list) => self.seed_stopped(list),
            Internal::MastersMoveProgress {
                job,
                done,
                total,
                file,
            } => {
                if let Some(svc) = &mut self.import
                    && let Some(p) = svc.jobs.get_mut(&job)
                {
                    p.files = total;
                    p.done = done;
                    p.current = Some((file, Stage::Placing));
                }
                self.publish_downloads();
            }
            Internal::MastersMoved {
                job,
                old_dir,
                new_dir,
                result,
            } => {
                if let Some(svc) = &mut self.import {
                    svc.jobs.remove(&job);
                }
                match result {
                    Ok(report) => {
                        if let Some(svc) = &mut self.import {
                            let subtitle = if report.audio_moved > 0 {
                                format!(
                                    "перенесено {} · проверено BLAKE3 → {}",
                                    report.audio_moved,
                                    new_dir.display()
                                )
                            } else {
                                format!("папка → {}", new_dir.display())
                            };
                            svc.history.push_front(v::Job {
                                id: IMPORT_ID | job,
                                title: "Перенос архива FLAC".to_owned(),
                                subtitle,
                                progress: Some(1.0),
                                rate: None,
                                eta: None,
                                paused: false,
                                controllable: false,
                                pausable: false,
                                error: None,
                                at: nikgob_library::now_ms(),
                            });
                            svc.history.truncate(100);
                            svc.apply_reconfigure();
                        }
                        self.publish_downloads();
                        if report.audio_moved > 0 {
                            self.toast(
                                format!(
                                    "Перенесено FLAC-оригиналов: {} (проверено по хешу, старые удалены)",
                                    report.audio_moved
                                ),
                                None,
                            );
                        } else {
                            self.toast(format!("Папка FLAC: {}", new_dir.display()), None);
                        }
                    }
                    Err(err) => {
                        log::error!("moving masters from {} to {}: {err}", old_dir.display(), new_dir.display());
                        if let Some(svc) = &mut self.import {
                            let mut cfg = svc
                                .reconfigure
                                .take()
                                .unwrap_or_else(|| svc.config().clone());
                            cfg.masters_dir = old_dir.clone();
                            svc.reconfigure = Some(cfg);
                            svc.apply_reconfigure();
                        }
                        self.publish(|s| s.prefs.masters_dir = old_dir);
                        self.save_prefs();
                        self.publish_downloads();
                        self.toast(format!("Не удалось перенести FLAC: {err}"), None);
                    }
                }
            }
            Internal::Enrich(ev) => self.enrich_event(ev),
            Internal::LyricsFetched(track, source, got) => self.lyrics_fetched(track, source, got),
            Internal::LyricsEmbedded(track, result) => self.lyrics_embedded(track, result),
            Internal::Align(m) => self.align_msg(m),
            Internal::Sharing(p) => self.share_polled(p),
        }
    }

    fn import_event(&mut self, ev: ImportEvent) {
        // The wishlist hears about every file that landed (or failed).
        match &ev {
            ImportEvent::Imported { track_id, path, .. } => {
                self.lyrics_imported(*track_id, path);
                self.wish_file_came(*track_id, path);
                self.wish_imported(*track_id, path);
                self.analysis.kick();
                self.quality.kick();
                self.same_audio.kick();
            }
            ImportEvent::Duplicate { job, kept, path, quality } => {
                self.lyrics_imported(*kept, path);
                if let Some(w) = self.import.as_ref().and_then(|s| s.wish_jobs.get(&job.0).copied()) {
                    self.wish_duplicate(w, quality.clone());
                }
                self.wish_imported(*kept, path);
                self.analysis.kick();
            }
            ImportEvent::Failed { path, .. } => {
                self.lyrics_import_failed(path);
                self.wish_import_failed(path);
            }
            ImportEvent::WrongAudio { job, sounds_like, .. } => {
                if let Some(w) = self.import.as_ref().and_then(|s| s.wish_jobs.get(&job.0).copied()) {
                    self.wish_wrong_audio(w, sounds_like.clone());
                }
            }
            _ => {}
        }
        // An import for a better copy says what came of it in plain words.
        let (upgrade_job, dup) = match &ev {
            ImportEvent::Finished { job, .. } => {
                match self.import.as_ref().and_then(|s| s.wish_jobs.get(&job.0).copied()) {
                    Some(w) if self.is_upgrade(w) => (true, self.upgrade_dup(w)),
                    _ => (false, None),
                }
            }
            _ => (false, None),
        };
        let Some(svc) = &mut self.import else { return };
        let mut finished = None;
        let mut parked_for = None;
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
                        wrong: Vec::new(),
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
            ImportEvent::WrongAudio { job, path, title, sounds_like, .. } => {
                log::warn!("import: {} is not «{title}» (it sounds like {sounds_like:?})", path.display());
                if let Some(p) = svc.jobs.get_mut(&job.0) {
                    p.wrong.push(title);
                }
            }
            ImportEvent::NeedsConfirmation { item } => {
                if let Some(p) = svc.jobs.get_mut(&item.job.0) {
                    p.done += 1;
                    p.parked += 1;
                }
                parked_for = item.wishlist;
            }
            ImportEvent::Finished {
                job,
                imported,
                duplicates,
                failed,
                pending,
                left_out,
                cancelled,
            } => {
                let (title, wrong) = svc
                    .jobs
                    .remove(&job.0)
                    .map_or_else(|| ("Импорт".to_owned(), Vec::new()), |p| (p.title, p.wrong));
                let summary = if cancelled {
                    "отменено — добавленное убрано".to_owned()
                } else if upgrade_job {
                    let mut s = crate::streams::upgrade_job_words(imported, duplicates, left_out, failed, dup.as_deref());
                    if pending > 0 {
                        s.push_str(&format!(" · {PARKED} {pending}"));
                    }
                    s
                } else {
                    let mut parts = vec![format!("добавлено {imported}")];
                    if duplicates > 0 {
                        parts.push(format!("дублей {duplicates}"));
                    }
                    if left_out > 0 {
                        parts.push(format!("не из этого альбома {left_out}"));
                    }
                    if failed > 0 {
                        parts.push(format!("ошибок {failed}"));
                    }
                    if pending > 0 {
                        parts.push(format!("{PARKED} {pending}"));
                    }
                    parts.join(" · ")
                };
                let error = failed_words(failed, &wrong);
                if let Err(e) = self.lib.log_job("import", &title, &summary, error.as_deref()) {
                    log::warn!("job log: {e}");
                }
                svc.history.push_front(v::Job {
                    id: IMPORT_ID | job.0,
                    title,
                    subtitle: summary.clone(),
                    progress: Some(1.0),
                    rate: None,
                    eta: None,
                    paused: false,
                    controllable: false,
                    pausable: false,
                    error,
                    at: nikgob_library::now_ms(),
                });
                svc.history.truncate(100);
                finished = Some((summary, cancelled, svc.wish_jobs.remove(&job.0)));
            }
            ImportEvent::TorrentFound(path) => {
                self.command(Command::AddTorrentFile(path));
                return;
            }
            ImportEvent::MasterRegistered(m) => {
                log::info!("master registered: {} ({} bytes)", m.file_hash, m.size_bytes);
            }
        }
        if let Some(w) = parked_for {
            self.wish_parked(w);
        }
        if let Some((summary, cancelled, wishlist)) = finished {
            match wishlist {
                // Cancelled: the item does not go on by itself (it can be
                // brought back from the toast).
                Some(w) if cancelled => self.wish_dismiss(&[w]),
                Some(w) => self.wish_settle(w),
                None => {}
            }
            self.reload_after_import();
            self.offer_editions();
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
        // An import may have replaced a cover the UI holds, and the palette
        // of the playing track's album with it.
        self.publish(|s| s.cover_files += 1);
        self.publish_now_playing();
        self.load_library();
        self.load_home();
        self.start_enrich();
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
        // «Скачано» once opened: the new files on top.
        if !self.state.read().library.downloaded.is_empty() {
            self.load_downloaded();
        }
    }

    // ------------------------------------------------------------- trash

    /// Deletes tracks: into the Trash (30 days), or for good when the Trash
    /// is off in the settings.
    pub(crate) fn delete_tracks(&mut self, tracks: Vec<TrackId>, what: &str) {
        if tracks.is_empty() {
            return;
        }
        if !self.state.read().prefs.use_trash {
            self.purge_tracks(tracks);
            return;
        }
        let Some(svc) = &self.import else {
            self.toast("Удаление недоступно", None);
            return;
        };
        let cfg = svc.config().clone();
        if self.playing.is_some_and(|e| tracks.contains(&e.track)) {
            self.command(Command::Next);
        }
        let albums = self.wish_drop_deleted(&tracks);
        let mut failed = 0;
        for &t in &tracks {
            if let Err(e) = nikgob_import::delete_track_to_trash(&self.lib, &cfg.trash_dir, &cfg.library_dir, t) {
                log::error!("trash {t}: {e}");
                failed += 1;
            }
        }
        self.wish_settle_albums(&albums);
        if failed == tracks.len() {
            self.toast("Не получилось удалить", None);
        } else {
            self.toast(
                format!("{what} в корзине (30 дней)"),
                Some(("Отменить".into(), Command::RestoreTracks(tracks))),
            );
        }
        self.after_delete();
    }

    /// Takes deleted tracks back out of the Trash.
    pub(crate) fn restore_tracks(&mut self, tracks: Vec<TrackId>) {
        let mut ok = true;
        for t in tracks {
            match nikgob_import::restore_track(&self.lib, t) {
                Ok(r) => ok &= r.files_failed == 0,
                Err(e) => {
                    log::error!("restore {t}: {e}");
                    ok = false;
                }
            }
        }
        self.toast(if ok { "Восстановлено" } else { "Восстановить удалось не всё" }, None);
        self.after_delete();
    }

    /// Deletes tracks for good: files, master, rows.
    pub(crate) fn purge_tracks(&mut self, tracks: Vec<TrackId>) {
        let Some(svc) = &self.import else {
            self.toast("Удаление недоступно", None);
            return;
        };
        let cfg = svc.config().clone();
        if self.playing.is_some_and(|e| tracks.contains(&e.track)) {
            self.command(Command::Next);
        }
        let albums = self.wish_drop_deleted(&tracks);
        let (mut done, mut failed) = (0usize, 0usize);
        for &t in &tracks {
            match nikgob_import::purge_track(&self.lib, &cfg, t) {
                Ok(r) => {
                    done += 1;
                    failed += r.files_failed;
                }
                Err(e) => {
                    log::error!("purge {t}: {e}");
                    failed += 1;
                }
            }
        }
        self.wish_settle_albums(&albums);
        self.toast(
            if failed > 0 {
                "Удалено, но не все файлы получилось стереть".to_owned()
            } else {
                format!("Удалено навсегда: {done}")
            },
            None,
        );
        self.after_delete();
    }

    pub(crate) fn empty_trash(&mut self) {
        let ids: Vec<TrackId> = self
            .lib
            .deleted_tracks()
            .unwrap_or_default()
            .into_iter()
            .map(|d| d.track.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        self.purge_tracks(ids);
    }

    /// The "Корзина" list.
    pub(crate) fn load_trash(&mut self) {
        const DAY_MS: i64 = 24 * 60 * 60 * 1000;
        let now = nikgob_library::now_ms();
        let rows = self.lib.deleted_tracks().unwrap_or_default();
        let res = &mut self.res;
        let trash = rows
                .iter()
                .map(|d| {
                    let ago = ((now - d.deleted_at).max(0) / DAY_MS) as u32;
                    v::TrashItem {
                        track: res.track(&d.track),
                        deleted: match ago {
                            0 => "сегодня".to_owned(),
                            1 => "вчера".to_owned(),
                            n => format!("{n} дн. назад"),
                        },
                        days_left: nikgob_import::TRASH_DAYS.saturating_sub(ago),
                    }
                })
                .collect();
        self.publish(|s| s.library.trash = trash);
    }

    /// The «Скачано» tab: the latest files by the local day they came.
    pub(crate) fn load_downloaded(&mut self) {
        const MONTHS: [&str; 12] = [
            "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября",
            "декабря",
        ];
        let tz = jiff::tz::TimeZone::system();
        let today = jiff::Zoned::now().date();
        let label = |date: jiff::civil::Date| {
            if date == today {
                "Сегодня".to_owned()
            } else if today.yesterday().ok() == Some(date) {
                "Вчера".to_owned()
            } else if date.year() == today.year() {
                format!("{} {}", date.day(), MONTHS[date.month() as usize - 1])
            } else {
                format!("{} {} {}", date.day(), MONTHS[date.month() as usize - 1], date.year())
            }
        };
        let rows = self.lib.downloaded_tracks(500).unwrap_or_default();
        let mut days: Vec<(jiff::civil::Date, v::DownloadedDay)> = Vec::new();
        for (row, at) in &rows {
            let Ok(ts) = jiff::Timestamp::from_millisecond(*at) else { continue };
            let date = ts.to_zoned(tz.clone()).date();
            if days.last().is_none_or(|(d, _)| *d != date) {
                days.push((date, v::DownloadedDay { label: label(date), tracks: Vec::new() }));
            }
            if let Some((_, day)) = days.last_mut() {
                day.tracks.push(self.res.track(row));
            }
        }
        let downloaded = days.into_iter().map(|(_, d)| d).collect();
        self.publish(|s| s.library.downloaded = downloaded);
    }

    fn after_delete(&mut self) {
        self.reload_after_import();
        self.load_trash();
    }


    // ------------------------------------------------------------- settings

    /// Import-related prefs changed: rebuild the importer when it is idle.
    pub(crate) fn import_prefs(&mut self, p: &v::Prefs) {
        let move_to = {
            let Some(svc) = &mut self.import else { return };
            let old = svc.reconfigure.as_ref().unwrap_or(svc.config());
            let mut cfg = old.clone();
            cfg.opus_bitrate_kbps = p.opus_bitrate;
            cfg.check_fake_lossless = p.check_fake_lossless;
            cfg.convert_fake_lossless = p.convert_fake_lossless && p.fake_flac_auto;
            cfg.fallback_seed_argb = p.fallback_seed;
            if !p.library_dir.as_os_str().is_empty() {
                cfg.library_dir = p.library_dir.clone();
            }
            if !p.inbox_dir.as_os_str().is_empty() {
                cfg.inbox_dir = p.inbox_dir.clone();
            }
            let move_to = if !p.masters_dir.as_os_str().is_empty() {
                let normalized = nikgob_import::normalize_masters_dir(&p.masters_dir, &cfg.library_dir);
                (normalized != old.masters_dir()).then_some(p.masters_dir.clone())
            } else {
                None
            };
            let same = cfg.opus_bitrate_kbps == old.opus_bitrate_kbps
                && cfg.check_fake_lossless == old.check_fake_lossless
                && cfg.convert_fake_lossless == old.convert_fake_lossless
                && cfg.fallback_seed_argb == old.fallback_seed_argb
                && cfg.library_dir == old.library_dir
                && cfg.inbox_dir == old.inbox_dir;
            if !same {
                svc.reconfigure = Some(cfg);
                svc.apply_reconfigure();
            }
            move_to
        };
        if let Some(dir) = move_to {
            self.move_masters_dir(dir);
        }
    }

    /// Relocates lossless masters (`.flac` originals and `.json` sidecars) to a
    /// new directory or drive, verifying every file's BLAKE3 hash before
    /// deleting the old copy.
    pub(crate) fn move_masters_dir(&mut self, chosen: PathBuf) {
        let Some(svc) = &mut self.import else {
            self.toast("Перенос папки FLAC недоступен", None);
            return;
        };
        let cur_cfg = svc.reconfigure.as_ref().unwrap_or(svc.config()).clone();
        let new_dir = nikgob_import::normalize_masters_dir(&chosen, &cur_cfg.library_dir);
        let old_dir = cur_cfg.masters_dir();

        let default_masters = cur_cfg.musiclib_dir().join("masters");
        let inside_work_dir = new_dir != default_masters
            && (new_dir.starts_with(&cur_cfg.inbox_dir)
                || new_dir.starts_with(&cur_cfg.downloads_dir)
                || new_dir.starts_with(&cur_cfg.trash_dir)
                || (new_dir.starts_with(&cur_cfg.library_dir)
                    && !new_dir.starts_with(cur_cfg.musiclib_dir())));
        if inside_work_dir {
            self.toast(
                "Нельзя хранить FLAC-архив внутри папки музыки, Inbox или загрузок",
                None,
            );
            return;
        }

        if old_dir == new_dir {
            self.publish(|s| s.prefs.masters_dir = new_dir);
            self.save_prefs();
            self.toast("Эта папка FLAC уже используется", None);
            return;
        }

        let mut cfg = cur_cfg;
        cfg.masters_dir = new_dir.clone();
        svc.reconfigure = Some(cfg);
        svc.apply_reconfigure();

        svc.next_move_id += 1;
        let job_id = (1u64 << 40) | svc.next_move_id;
        svc.jobs.insert(
            job_id,
            Progress {
                title: "Перенос архива FLAC".to_owned(),
                files: 0,
                done: 0,
                imported: 0,
                duplicates: 0,
                failed: 0,
                parked: 0,
                current: None,
                last_error: None,
                wrong: Vec::new(),
            },
        );

        let lib = self.lib.clone();
        let tx = svc.internal.clone();
        let (from, to) = (old_dir, new_dir.clone());
        svc.spawn_blocking(move || {
            let tx_prog = tx.clone();
            let result = nikgob_import::relocate_masters(&lib, &from, &to, |done, total, path| {
                let _ = tx_prog.send(Internal::MastersMoveProgress {
                    job: job_id,
                    done,
                    total,
                    file: file_name(path),
                });
            })
            .map_err(|e| e.to_string());
            let _ = tx.send(Internal::MastersMoved {
                job: job_id,
                old_dir: from,
                new_dir: to,
                result,
            });
        });

        self.publish(|s| s.prefs.masters_dir = new_dir);
        self.save_prefs();
        self.publish_downloads();
    }

    /// The MusicBrainz contact or AcoustID key changed.
    pub(crate) fn set_metadata_config(&mut self, cfg: &MetadataConfig) {
        let Some(svc) = &mut self.import else { return };
        svc.metadata = Metadata::new(cfg)
            .map_err(|e| log::warn!("online metadata disabled: {e}"))
            .ok();
        svc.reconfigure = Some(svc.reconfigure.take().unwrap_or_else(|| svc.config().clone()));
        svc.apply_reconfigure();
        self.start_enrich();
    }

    // ------------------------------------------------------------- lyrics & tags

    /// Lyrics and file tags of the track that just started.
    pub(crate) fn track_details(&mut self, track: TrackId) {
        // The previous track is no longer under the decoder.
        let later: Vec<TrackId> = self
            .import
            .as_mut()
            .map(|s| std::mem::take(&mut s.embed_later).into_iter().collect())
            .unwrap_or_default();
        for t in later {
            self.embed_lyrics(t);
        }
        self.write_later_tags();
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
            self.publish(|s| {
                if s.info.track == Some(track) {
                    s.info.tags.extend(extra);
                }
            });
        }

        self.load_lyrics(track, path, tags.as_ref());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line(seq: i64, kind: &str, title: &str, subtitle: &str, error: Option<&str>) -> nikgob_library::JobLogEntry {
        nikgob_library::JobLogEntry {
            seq,
            kind: kind.into(),
            title: title.into(),
            subtitle: subtitle.into(),
            error: error.map(str::to_owned),
            at: seq,
        }
    }

    /// The history of 2026-11-04: the same refusal of one song again and
    /// again, from Tidal and from Qobuz, an album's «Скачано 1 из 10» every
    /// few hours. One line of each song stays, the newest; the imports'
    /// lines all stay.
    #[test]
    fn one_line_of_each_wish_is_kept() {
        let entries = vec![
            line(9, "wish:a", "frutiger pm — cloud city", "Tidal сейчас не отдаёт этот трек", None),
            line(8, "import", "Tidal: frutiger pm — cloud city", "", Some("Не дал скачать: ошибка 403 · попробую в 21:48")),
            line(7, "import", "MORGENSHTERN — ЛЕГЕНДАРНАЯ ПЫЛЬ", "", Some("Скачано 1 из 10 — остальное попробую в 21:47")),
            line(6, "import", "Qobuz: frutiger pm — cloud city", "", Some("Не дал скачать: просит подождать · попробую в 19:40")),
            line(5, "import", "Импорт «a.flac»", "добавлено 1", None),
            line(4, "import", "MORGENSHTERN — ЛЕГЕНДАРНАЯ ПЫЛЬ", "", Some("Скачано 1 из 10 — остальное попробую в 17:40")),
            line(3, "import", "Tidal: Eve — Underdog", "не дал скачать: ошибка 403 · беру из Qobuz", None),
            line(2, "import", "Qobuz: Eve", "добавлено 1", None),
            line(1, "import", "Импорт «a.flac»", "добавлено 1", None),
        ];
        let kept: Vec<i64> = kept_history(entries).iter().map(|e| e.seq).collect();
        assert_eq!(kept, [9, 7, 5, 3, 2, 1]);
    }
}
