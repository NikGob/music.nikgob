//! Torrent downloads (PLAN §7): the embedded client starts only when there is
//! something to download or seed, finished payloads go through the importer,
//! and the seed policy decides when payloads leave `Downloads/`. Lossless
//! masters among them move to `.musiclib/masters/` instead of being deleted,
//! since nothing is backed up yet.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use nikgob_core::indexer::TorrentPayload;
use nikgob_core::model::{Download, DownloadStatus, SeedPolicy, SourceOrigin, WishlistId};
use nikgob_torrent::{TorrentClient, TorrentConfig, TorrentMeta, TorrentState, TorrentStatus};
use nikgob_view::{self as v, Command};

use crate::Worker;
use crate::import::Internal;

/// A request waiting for the client to start.
pub(crate) type Op = Box<dyn FnOnce(&mut Worker, TorrentClient) + Send>;

/// A torrent the client took.
pub(crate) struct Added {
    hash: String,
    name: String,
    files: Option<Vec<usize>>,
    magnet: Option<String>,
    /// The wishlist item it was added for.
    wish: Option<WishlistId>,
    /// The Inbox .torrent it came from, removed now that it is added.
    consume: Option<std::path::PathBuf>,
}

pub(crate) struct Torrents {
    client: Option<TorrentClient>,
    starting: bool,
    state_dir: PathBuf,
    offline: bool,
    pending: Vec<Op>,
    /// Payloads fetched for search results, by candidate id (so "Выбрать
    /// файлы…" then "Скачать" fetch once).
    payloads: HashMap<String, TorrentPayload>,
    /// Torrents whose files were handed to the importer this session.
    imported: HashSet<String>,
    /// Finished torrents whose payload waits for the importer to go idle.
    cleanup: Vec<(String, Vec<PathBuf>)>,
    history: VecDeque<v::Job>,
    last_policy: Instant,
    last_view: Option<(Vec<v::Job>, Vec<v::SeedRow>)>,
    /// `last_view` without what changes every second (bytes, rates,
    /// progress, ETAs).
    last_shape: Option<(Vec<v::Job>, Vec<v::SeedRow>)>,
    /// «Загрузки» is shown ([`Command::DownloadsShown`](nikgob_view::Command::DownloadsShown)).
    pub(crate) shown: bool,
}

impl Torrents {
    pub(crate) fn new(state_dir: PathBuf, offline: bool) -> Self {
        Self {
            client: None,
            starting: false,
            state_dir,
            offline,
            pending: Vec::new(),
            payloads: HashMap::new(),
            imported: HashSet::new(),
            cleanup: Vec::new(),
            history: VecDeque::new(),
            last_policy: Instant::now(),
            last_view: None,
            last_shape: None,
            shown: false,
        }
    }
}

/// A torrent view without the numbers that move every second.
fn shape(active: &[v::Job], seeding: &[v::SeedRow]) -> (Vec<v::Job>, Vec<v::SeedRow>) {
    let active = active
        .iter()
        .map(|j| v::Job {
            subtitle: String::new(),
            progress: j.progress.map(|_| 0.0),
            rate: None,
            eta: None,
            ..j.clone()
        })
        .collect();
    let seeding = seeding
        .iter()
        .map(|r| v::SeedRow { uploaded_bytes: 0, downloaded_bytes: 0, up_bps: 0, down_bps: 0, peers: 0, ..r.clone() })
        .collect();
    (active, seeding)
}

fn config(p: &v::Prefs, downloads: &Path, state: &Path) -> TorrentConfig {
    TorrentConfig {
        downloads_dir: downloads.to_owned(),
        state_dir: state.to_owned(),
        no_seed: p.no_seed,
        seed_ratio: p.seed_ratio,
        seed_days: p.seed_days,
        seed_limit_gb: p.seed_limit_gb,
        download_limit_kbps: p.download_limit_kbps,
        upload_limit_kbps: p.upload_limit_kbps,
    }
}

pub(crate) fn human_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} ГБ", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} МБ", b / 1e6)
    } else {
        format!("{:.0} КБ", b / 1e3)
    }
}

fn complete(t: &TorrentStatus) -> bool {
    t.completed_at.is_some()
}

/// Where a torrent is: the one [`v::Transfer`] its row and its wished
/// item's line both show.
fn transfer(t: &TorrentStatus) -> v::Transfer {
    let phase = match t.state {
        TorrentState::Paused => v::TransferPhase::Paused,
        TorrentState::Initializing => v::TransferPhase::Connecting,
        _ if t.total == 0 => v::TransferPhase::Starting,
        _ => v::TransferPhase::Going,
    };
    v::Transfer { phase, done: t.downloaded, total: (t.total > 0).then_some(t.total), bps: t.down_bps }
}

fn job(t: &TorrentStatus) -> v::Job {
    let mut job = transfer(t).job(t.id, t.name.clone(), None, None, true, true);
    if let TorrentState::Error(e) = &t.state {
        job.error = Some(e.clone());
    }
    job
}

impl Worker {
    fn torrent_config(&self) -> Option<TorrentConfig> {
        let svc = self.import.as_ref()?;
        let prefs = self.state.read().prefs.clone();
        Some(config(&prefs, &svc.config().downloads_dir, &self.torrents.state_dir))
    }

    /// Runs `op` with the client, starting it first if needed.
    pub(crate) fn with_client(&mut self, op: Op) {
        if let Some(c) = self.torrents.client.clone() {
            op(self, c);
            return;
        }
        self.torrents.pending.push(op);
        self.start_torrents();
    }

    /// Starts the client if there are downloads or seeds left from before.
    pub(crate) fn resume_torrents(&mut self) {
        let unfinished = self
            .lib
            .list_downloads()
            .unwrap_or_default()
            .iter()
            .any(|d| d.status != DownloadStatus::Done && d.status != DownloadStatus::Failed);
        if unfinished {
            self.start_torrents();
        }
    }

    fn start_torrents(&mut self) {
        if self.torrents.starting || self.torrents.client.is_some() {
            return;
        }
        let (Some(cfg), Some(svc)) = (self.torrent_config(), self.import.as_ref()) else {
            self.toast("Торренты недоступны", None);
            self.torrents.pending.clear();
            return;
        };
        self.torrents.starting = true;
        let offline = self.torrents.offline;
        let tx = svc.internal();
        svc.spawn(async move {
            let net = if offline {
                nikgob_torrent::NetOptions {
                    dht: false,
                    listen: false,
                    upnp: false,
                    local_discovery: false,
                }
            } else {
                nikgob_torrent::NetOptions::default()
            };
            let r = TorrentClient::start_with(cfg, net).await.map_err(|e| e.to_string());
            let _ = tx.send(Internal::TorrentStarted(r));
        });
    }

    pub(crate) fn torrent_started(&mut self, r: Result<TorrentClient, String>) {
        self.torrents.starting = false;
        match r {
            Ok(c) => {
                log::info!("torrent client started");
                self.torrents.client = Some(c.clone());
                for op in std::mem::take(&mut self.torrents.pending) {
                    op(self, c.clone());
                }
                self.poll_torrents();
            }
            Err(e) => {
                log::error!("torrent client: {e}");
                self.torrents.pending.clear();
                self.toast("Не запустился торрент-клиент", None);
            }
        }
    }

    // ------------------------------------------------------------- commands

    pub(crate) fn torrent_command(&mut self, c: Command) {
        match c {
            Command::DownloadTorrent { result, files } => self.download_result(result, files),
            Command::ListTorrentFiles(result) => self.list_files(result),
            Command::CloseTorrentFiles => self.publish(|s| s.downloads.files = None),
            Command::AddTorrentFile(path) => match std::fs::read(&path) {
                Ok(bytes) => {
                    // A .torrent dropped into the Inbox is consumed once the
                    // client took it; kept for another try if it did not.
                    let consume = self
                        .import
                        .as_ref()
                        .is_some_and(|svc| path.starts_with(&svc.config().inbox_dir))
                        .then_some(path);
                    self.add_payload_inner(TorrentPayload::TorrentFile(bytes), None, None, None, consume);
                }
                Err(e) => {
                    log::warn!("{}: {e}", path.display());
                    self.toast("Не получилось прочитать .torrent", None);
                }
            },
            Command::AddMagnet(m) => {
                let m = m.trim().to_owned();
                if m.starts_with("magnet:") {
                    self.add_payload(TorrentPayload::Magnet(m.clone()), None, Some(m));
                } else {
                    self.toast("Это не magnet-ссылка", None);
                }
            }
            // Import jobs (folders, files) are never paused; a cancel stops
            // one and takes back what it added. Their ids are not torrent
            // ids and must never reach the torrent client.
            // A download of another source is never paused here (its row
            // has no pause); a cancel turns its item down.
            Command::PauseJob(id, _) if crate::running::job_kind(id) == crate::running::JobKind::Direct => {}
            Command::CancelJob(id) if crate::running::job_kind(id) == crate::running::JobKind::Direct => self.cancel_run(id),
            Command::PauseJob(id, _) if id & crate::IMPORT_ID != 0 => {
                self.toast("Импорт нельзя поставить на паузу — его можно только отменить", None);
            }
            Command::CancelJob(id) if id & crate::IMPORT_ID != 0 => self.cancel_import(id & !crate::IMPORT_ID),
            Command::PauseJob(id, pause) => self.with_client(Box::new(move |w, c| {
                w.spawn_torrent_op(async move {
                    if pause { c.pause(id).await } else { c.resume(id).await }.map_err(|e| e.to_string())
                });
            })),
            Command::CancelJob(id) => self.with_client(Box::new(move |w, c| {
                let hash = c.list().into_iter().find(|t| t.id == id).map(|t| t.info_hash);
                // A torrent of a «Хочу скачать» item: its item is cancelled
                // with it, not left «качается» for ever.
                let item = hash.as_deref().and_then(|h| w.lib.get_download(h).ok().flatten()).and_then(|d| d.wishlist_id);
                // Its files go once nothing needs them (a cancelled import
                // took back what it added, masters of the library moved out).
                match &hash {
                    Some(h) => w.drop_torrent(&c, h),
                    None => w.spawn_torrent_op(async move { c.remove(id, true).await.map_err(|e| e.to_string()) }),
                }
                if let Some(item) = item
                    && w
                        .lib
                        .get_wishlist_item(item)
                        .ok()
                        .flatten()
                        .is_some_and(|i| i.status != nikgob_core::model::WishlistStatus::Done)
                {
                    w.wish_dismiss(&[item]);
                }
            })),
            Command::CleanSeeding => self.with_client(Box::new(|w, c| {
                let done: Vec<TorrentStatus> = c.list().into_iter().filter(complete).collect();
                let tx = w.import.as_ref().map(|s| s.internal());
                if let (Some(svc), Some(tx)) = (w.import.as_ref(), tx) {
                    svc.spawn(async move {
                        let mut stopped = Vec::new();
                        for t in done {
                            if c.remove(t.id, false).await.is_ok() {
                                stopped.push(t);
                            }
                        }
                        let _ = tx.send(Internal::SeedStopped(stopped));
                    });
                }
            })),
            _ => {}
        }
    }

    /// Runs a client call; a failure becomes a toast.
    pub(crate) fn spawn_torrent_op<F>(&self, f: F)
    where
        F: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let Some(svc) = &self.import else { return };
        let tx = svc.internal();
        svc.spawn(async move {
            if let Err(e) = f.await {
                let _ = tx.send(Internal::TorrentError(e));
            }
        });
    }

    /// The payload of a search result, fetched once.
    fn fetch_payload(&mut self, result: usize, then: impl FnOnce(&mut Worker, TorrentPayload) + Send + 'static) {
        // The cache is keyed by the candidate, not by its index: a new search
        // reuses the same indices for other releases.
        if let Some(p) = self
            .candidates
            .get(result)
            .and_then(|c| self.torrents.payloads.get(&c.id))
            .cloned()
        {
            then(self, p);
            return;
        }
        let (Some(c), Some(indexer), Some(svc)) =
            (self.candidates.get(result).cloned(), self.indexer.clone(), self.import.as_ref())
        else {
            self.toast("Результат поиска устарел — найди заново", None);
            return;
        };
        let cid = c.id.clone();
        let then = move |w: &mut Worker, p: TorrentPayload| {
            w.torrents.payloads.insert(cid, p.clone());
            then(w, p);
        };
        let tx = svc.internal();
        svc.spawn(async move {
            let r = tokio::time::timeout(Duration::from_secs(60), indexer.fetch_torrent(&c))
                .await
                .map_err(|_| "Prowlarr не отдал торрент".to_owned())
                .and_then(|r| r.map_err(|e| e.to_string()));
            let _ = tx.send(Internal::PayloadFetched(result, r, Box::new(then)));
        });
    }

    pub(crate) fn payload_fetched(
        &mut self,
        result: usize,
        r: Result<TorrentPayload, String>,
        then: Box<dyn FnOnce(&mut Worker, TorrentPayload) + Send>,
    ) {
        match r {
            Ok(p) => then(self, p),
            Err(e) => {
                log::warn!("fetching torrent: {e}");
                self.toast(format!("Не скачать .torrent: {e}"), None);
                self.publish(|s| {
                    if let Some(f) = s.downloads.files.as_mut().filter(|f| f.result == Some(result)) {
                        f.loading = false;
                        f.error = Some(e);
                    }
                });
            }
        }
    }

    fn download_result(&mut self, result: usize, files: Option<Vec<usize>>) {
        self.publish(|s| s.downloads.files = None);
        self.toast("Добавляю раздачу…", None);
        self.fetch_payload(result, move |w, p| {
            let magnet = match &p {
                TorrentPayload::Magnet(m) => Some(m.clone()),
                TorrentPayload::TorrentFile(_) => None,
            };
            w.add_payload(p, files, magnet);
        });
    }

    fn list_files(&mut self, result: usize) {
        let name = self
            .candidates
            .get(result)
            .map(|c| c.title.clone())
            .unwrap_or_default();
        self.publish(|s| {
            s.downloads.files = Some(v::TorrentFiles {
                result: Some(result),
                name,
                loading: true,
                ..Default::default()
            });
        });
        self.fetch_payload(result, move |w, p| {
            w.with_client(Box::new(move |w, c| {
                let Some(svc) = &w.import else { return };
                let tx = svc.internal();
                svc.spawn(async move {
                    let r = c.inspect(p).await.map_err(|e| e.to_string());
                    let _ = tx.send(Internal::TorrentFilesListed(result, r));
                });
            }));
        });
    }

    pub(crate) fn files_listed(&mut self, result: usize, r: Result<TorrentMeta, String>) {
        self.publish(|s| {
            let Some(f) = s.downloads.files.as_mut().filter(|f| f.result == Some(result)) else {
                return;
            };
            f.loading = false;
            match r {
                Ok(meta) => {
                    f.name = meta.name;
                    f.files = meta.files.into_iter().map(|x| (x.index, x.path, x.size)).collect();
                }
                Err(e) => f.error = Some(e),
            }
        });
    }

    fn add_payload(&mut self, p: TorrentPayload, files: Option<Vec<usize>>, magnet: Option<String>) {
        self.add_payload_for(p, files, magnet, None);
    }

    /// Adds a torrent; `wish` = the wishlist item it is downloaded for (its
    /// files are then imported as that item's tracks).
    pub(crate) fn add_payload_for(
        &mut self,
        p: TorrentPayload,
        files: Option<Vec<usize>>,
        magnet: Option<String>,
        wish: Option<WishlistId>,
    ) {
        self.add_payload_inner(p, files, magnet, wish, None);
    }

    /// [`Self::add_payload_for`]; `consume` = a file removed once the
    /// client has the torrent (never when adding failed).
    fn add_payload_inner(
        &mut self,
        p: TorrentPayload,
        files: Option<Vec<usize>>,
        magnet: Option<String>,
        wish: Option<WishlistId>,
        consume: Option<std::path::PathBuf>,
    ) {
        self.with_client(Box::new(move |w, c| {
            let Some(svc) = &w.import else { return };
            let tx = svc.internal();
            svc.spawn(async move {
                let r = match c.add(p, files.clone()).await {
                    Ok(id) => {
                        let st = c.list().into_iter().find(|t| t.id == id);
                        st.map(|t| Added {
                            hash: t.info_hash,
                            name: t.name,
                            files,
                            magnet,
                            wish,
                            consume,
                        })
                        .ok_or_else(|| "торрент пропал из сессии".to_owned())
                    }
                    Err(e) => Err(e.to_string()),
                };
                let _ = tx.send(Internal::TorrentAdded(r));
            });
        }));
    }

    pub(crate) fn torrent_added(&mut self, r: Result<Added, String>) {
        match r {
            Ok(Added {
                hash,
                name,
                files,
                magnet,
                wish,
                consume,
            }) => {
                if let Some(path) = consume
                    && let Err(e) = std::fs::remove_file(&path)
                {
                    log::warn!("{}: not removed from the Inbox: {e}", path.display());
                }
                let now = std::time::SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                let prefs = self.state.read().prefs.clone();
                let d = Download {
                    info_hash: hash,
                    magnet,
                    torrent_file: None,
                    selected_files: files
                        .unwrap_or_default()
                        .iter()
                        .map(ToString::to_string)
                        .collect(),
                    status: DownloadStatus::Downloading,
                    seed_policy: if prefs.no_seed {
                        SeedPolicy::NoSeed
                    } else {
                        SeedPolicy::Standard
                    },
                    bytes_up: 0,
                    bytes_down: 0,
                    wishlist_id: wish,
                    created_at: now,
                    updated_at: now,
                };
                // A torrent known already keeps the item it was added for.
                let known = self.lib.get_download(&d.info_hash).ok().flatten();
                let d = Download {
                    wishlist_id: d.wishlist_id.or(known.and_then(|k| k.wishlist_id)),
                    ..d
                };
                if let Err(e) = self.lib.put_download(&d) {
                    log::warn!("saving download: {e}");
                }
                if self.state.read().prefs.download_toasts {
                    self.toast(format!("Качается: {name}"), None);
                }
                self.poll_torrents();
            }
            Err(e) => {
                log::warn!("adding torrent: {e}");
                self.toast(format!("Не получилось добавить раздачу: {e}"), None);
            }
        }
    }

    // ------------------------------------------------------------- polling

    /// Once a second while the client runs: progress, finished payloads,
    /// seed policy, cleanup.
    pub(crate) fn poll_torrents(&mut self) {
        let Some(c) = self.torrents.client.clone() else {
            self.share_no_torrents();
            return;
        };
        let list = c.list();
        self.share_torrents(&list);
        let downloads: HashMap<String, Download> = self
            .lib
            .list_downloads()
            .unwrap_or_default()
            .into_iter()
            .map(|d| (d.info_hash.clone(), d))
            .collect();

        for t in list.iter().filter(|t| complete(t)) {
            let row = downloads.get(&t.info_hash);
            let imported = self.torrents.imported.contains(&t.info_hash)
                || row.is_some_and(|d| matches!(d.status, DownloadStatus::Seeding | DownloadStatus::Done));
            if imported {
                continue;
            }
            self.torrents.imported.insert(t.info_hash.clone());
            if row.is_some() {
                let _ = self.lib.update_download_progress(
                    &t.info_hash,
                    DownloadStatus::Seeding,
                    t.uploaded,
                    t.downloaded,
                );
            }
            self.import_torrent(t);
        }
        // Finished before the history was kept (or its line got lost):
        // log it now, so it stays in "История" after seeding is over.
        for t in list.iter().filter(|t| complete(t)) {
            let done = downloads
                .get(&t.info_hash)
                .is_some_and(|d| matches!(d.status, DownloadStatus::Seeding | DownloadStatus::Done));
            if done && !t.name.is_empty() && !self.torrents.history.iter().any(|j| j.title == t.name) {
                let subtitle = format!("Скачано {}", human_bytes(t.total));
                if let Err(e) = self.lib.log_job("torrent", &t.name, &subtitle, None) {
                    log::warn!("job log: {e}");
                }
                self.torrents.history.push_back(v::Job {
                    id: t.id,
                    title: t.name.clone(),
                    subtitle,
                    progress: Some(1.0),
                    rate: None,
                    eta: None,
                    paused: false,
                    queued: false,
                    controllable: false,
                    pausable: false,
                    error: None,
                    at: t.completed_at.and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map_or_else(nikgob_library::now_ms, |d| d.as_millis() as i64),
                });
                self.torrents.last_view = None;
            }
        }

        // Where the torrents of wishlist items are, for their lines (put
        // out with the other sources' at the second's poll).
        self.streaming.wish.torrent_transfers = list
            .iter()
            .filter(|t| !complete(t))
            .filter_map(|t| Some((downloads.get(&t.info_hash)?.wishlist_id?, transfer(t))))
            .collect();

        let seed_days = self.state.read().prefs.seed_days;
        let active: Vec<v::Job> = list.iter().filter(|t| !complete(t)).map(job).collect();
        let seeding: Vec<v::SeedRow> = list
            .iter()
            .filter(|t| complete(t))
            .map(|t| v::SeedRow {
                id: t.id,
                title: t.name.clone(),
                uploaded_bytes: t.uploaded,
                downloaded_bytes: t.downloaded,
                expires: (seed_days > 0)
                    .then(|| t.completed_at)
                    .flatten()
                    .map(|at| {
                        let end = at + Duration::from_secs(u64::from(seed_days) * 86_400);
                        let left = end
                            .duration_since(std::time::SystemTime::now())
                            .unwrap_or_default()
                            .as_secs();
                        format!("чистка через {} дн.", left.div_ceil(86_400))
                    }),
                up_bps: t.up_bps,
                down_bps: t.down_bps,
                peers: t.peers,
                size_bytes: t.total,
            })
            .collect();
        let seeded: u64 = list.iter().map(|t| t.uploaded).sum();
        let view = (active.clone(), seeding.clone());
        // Off «Загрузки» (or with the window hidden) only a change of what
        // is there is published: a torrent added, finished, paused or gone;
        // its numbers catch up once the page is shown.
        let live = self.torrents.shown && self.window_visible();
        let moved = self.torrents.last_view.as_ref() != Some(&view);
        let changed = moved && (live || {
            let shape = shape(&active, &seeding);
            self.torrents.last_shape.as_ref() != Some(&shape)
        });
        if changed {
            self.torrents.last_shape = Some(shape(&active, &seeding));
            self.torrents.last_view = Some(view);
            let history: Vec<v::Job> = self.torrents.history.iter().cloned().collect();
            self.publish(|s| {
                crate::running::put_jobs(&mut s.downloads.active, crate::running::JobKind::Torrent, active);
                s.downloads.seeding = seeding;
                s.downloads.seeded_bytes = seeded;
                s.downloads.history.retain(|j| j.id & crate::IMPORT_ID != 0);
                s.downloads.history.splice(0..0, history);
                crate::import::sort_history(&mut s.downloads.history);
            });
        }

        if self.torrents.last_policy.elapsed() > Duration::from_secs(60) {
            self.torrents.last_policy = Instant::now();
            if let Some(svc) = &self.import {
                let tx = svc.internal();
                svc.spawn(async move {
                    let stopped = c.enforce_seed_policy().await;
                    if !stopped.is_empty() {
                        let _ = tx.send(Internal::SeedStopped(stopped));
                    }
                });
            }
        }
        self.run_cleanup();
    }

    /// Finished downloads of earlier sessions ("Загрузки → История").
    pub(crate) fn load_torrent_history(&mut self) {
        let logged: Vec<v::Job> = self
            .lib
            .job_log(Some("torrent"), 100)
            .unwrap_or_default()
            .iter()
            .map(|e| crate::import::logged_job(e, 0))
            .collect();
        self.torrents.history.extend(logged.iter().cloned());
        self.publish(|s| {
            s.downloads.history.extend(logged);
            crate::import::sort_history(&mut s.downloads.history);
        });
    }

    fn import_torrent(&mut self, t: &TorrentStatus) {
        let files: Vec<PathBuf> = t.selected_files.iter().filter(|p| p.is_file()).cloned().collect();
        let subtitle = format!("Скачано {} · импорт в библиотеку", human_bytes(t.total));
        if let Err(e) = self.lib.log_job("torrent", &t.name, &subtitle, None) {
            log::warn!("job log: {e}");
        }
        self.torrents.history.push_front(v::Job {
            id: t.id,
            title: t.name.clone(),
            subtitle,
            progress: Some(1.0),
            rate: None,
            eta: None,
            paused: false,
            queued: false,
            controllable: false,
            pausable: false,
            error: None,
            at: nikgob_library::now_ms(),
        });
        self.torrents.history.truncate(100);
        self.torrents.last_view = None;
        if self.state.read().prefs.download_toasts {
            self.toast(format!("Скачано: {}", t.name), None);
        }
        // Downloaded for a wishlist item: its files become the item's tracks.
        let item = self.lib.get_download(&t.info_hash).ok().flatten().and_then(|d| d.wishlist_id);
        // A better copy the user turned down meanwhile replaces nothing.
        if let Some(i) = item.and_then(|i| self.lib.get_wishlist_item(i).ok().flatten())
            && i.origin == nikgob_core::model::WishlistOrigin::Upgrade
            && i.status == nikgob_core::model::WishlistStatus::Dismissed
        {
            log::info!("{}: its better copy was turned down, nothing imported", t.name);
            return;
        }
        // A better copy of an album asked for by hand: asked about first.
        if let Some(i) = item
            && self.album_better_torrent(i, &files, &t.info_hash)
        {
            return;
        }
        let targets = item.map(|item| self.wish_targets(item)).unwrap_or_default();
        self.import_for(
            files,
            format!("Импорт «{}»", t.name),
            SourceOrigin::Torrent {
                info_hash: t.info_hash.clone(),
                path_in_torrent: String::new(),
            },
            targets,
        );
    }

    pub(crate) fn seed_stopped(&mut self, stopped: Vec<TorrentStatus>) {
        for t in stopped {
            log::info!("seeding of {} stopped", t.name);
            if !self.torrents.imported.contains(&t.info_hash)
                && self
                    .lib
                    .get_download(&t.info_hash)
                    .ok()
                    .flatten()
                    .is_some_and(|d| d.status == DownloadStatus::Downloading)
            {
                self.torrents.imported.insert(t.info_hash.clone());
                self.import_torrent(&t);
            }
            let _ = self.lib.update_download_progress(
                &t.info_hash,
                DownloadStatus::Done,
                t.uploaded,
                t.downloaded,
            );
            self.torrents.cleanup.push((t.info_hash, t.selected_files));
        }
        self.torrents.last_view = None;
        self.poll_torrents();
    }

    /// A torrent whose files nobody wants (its item turned down, a copy not
    /// taken, its «Отменить»): out of the client at once, its files kept
    /// until the stopped payloads' cleanup ([`Self::run_cleanup`]) takes
    /// them, once the importer is quiet: an import cancelled with it has
    /// taken back what it added by then, and a master of the library among
    /// them is moved out, never deleted.
    pub(crate) fn drop_torrent(&mut self, c: &TorrentClient, hash: &str) {
        let _ = self.lib.delete_download(hash);
        let Some(t) = c.list().into_iter().find(|t| t.info_hash == hash) else { return };
        log::info!("{}: leaves the client, its files go once nothing needs them", t.name);
        self.torrents.cleanup.push((t.info_hash, t.selected_files));
        let (tid, c) = (t.id, c.clone());
        self.spawn_torrent_op(async move { c.remove(tid, false).await.map_err(|e| e.to_string()) });
    }

    /// Deletes stopped payloads once the importer has taken what it wants.
    fn run_cleanup(&mut self) {
        if self.torrents.cleanup.is_empty() || !self.import.as_ref().is_some_and(|s| s.quiet()) {
            return;
        }
        let Some(cfg) = self.import.as_ref().map(|s| s.config().clone()) else { return };
        let mut held = Vec::new();
        for (hash, files) in std::mem::take(&mut self.torrents.cleanup) {
            // Files of a better copy that wait for «Заменить»: kept.
            if self.album_better_holds(&hash) {
                held.push((hash, files));
                continue;
            }
            // Which files are masters must be known before any goes.
            let masters = match self.lib.masters_of_torrent(&hash) {
                Ok(m) => m,
                Err(e) => {
                    log::warn!("the masters of torrent {hash} are not read, its files stay for now: {e}");
                    held.push((hash, files));
                    continue;
                }
            };
            for f in files {
                let rel = f
                    .strip_prefix(&cfg.downloads_dir)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                let master = masters.iter().find(|m| {
                    matches!(&m.origin, Some(SourceOrigin::Torrent { path_in_torrent, .. }) if *path_in_torrent == rel)
                });
                if let Some(m) = master {
                    let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    let dest = cfg.masters_dir().join(format!("{}.{ext}", m.file_hash));
                    let _ = std::fs::create_dir_all(cfg.masters_dir());
                    let moved = nikgob_import::fsutil::move_file_verified(&f, &dest, Some(&m.file_hash)).is_ok();
                    if moved {
                        let _ = self.lib.set_master_origin_path(m.track_id, &dest.to_string_lossy());
                    }
                } else if let Err(e) = std::fs::remove_file(&f)
                    && e.kind() != std::io::ErrorKind::NotFound
                {
                    log::warn!("cannot delete {}: {e}", f.display());
                }
                if let Some(dir) = f.parent() {
                    remove_empty_up(dir, &cfg.downloads_dir);
                }
            }
        }
        self.torrents.cleanup.extend(held);
    }

    pub(crate) fn torrent_prefs(&mut self, p: &v::Prefs) {
        let (Some(c), Some(svc)) = (self.torrents.client.as_ref(), self.import.as_ref()) else { return };
        c.set_config(config(p, &svc.config().downloads_dir, &self.torrents.state_dir));
    }

    pub(crate) fn shutdown_torrents(&mut self) {
        if let (Some(c), Some(svc)) = (self.torrents.client.take(), self.import.as_ref()) {
            svc.block_on(async move { c.shutdown().await });
        }
    }
}

/// Removes `dir` and its parents while empty, stopping at `stop`.
fn remove_empty_up(dir: &Path, stop: &Path) {
    let mut d = dir.to_path_buf();
    while d.starts_with(stop) && d != stop {
        if std::fs::remove_dir(&d).is_err() {
            break;
        }
        if !d.pop() {
            break;
        }
    }
}
