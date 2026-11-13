//! Embedded BitTorrent client for the player, built on [librqbit].
//!
//! Choices:
//! - **TLS**: librqbit's `rust-tls` feature (rustls + ring SHA-1) with default
//!   features off, so nothing but a C compiler (already needed for `ring`) is
//!   required on Windows: no OpenSSL, no CMake, and no HTTP API server.
//! - **Persistence**: librqbit's JSON session store (plus fastresume) lives in
//!   `state_dir`, so torrents and their ids survive restarts. librqbit does not
//!   keep completion times or lifetime upload counters, so a small sidecar
//!   (`nikgob-torrent.json`) does; it feeds `seed_days`, `seed_ratio` and the
//!   total-GB cap.
//! - **Seeding policy** is enforced by [`TorrentClient::enforce_seed_policy`]
//!   (the caller ticks it): a torrent whose selected files are complete and
//!   that hits a limit is removed from the session, files kept, and returned so
//!   the caller can import them. The decision itself is the pure
//!   [`policy::torrents_to_stop`].
//! - **Rate limits** are session-wide and applied live by `set_config`.
//!   `downloads_dir` / `state_dir` changes only take effect on the next start.
//! - Existing data in `downloads_dir` is always accepted (`overwrite`), so a
//!   restarted or re-added torrent re-checks and resumes or seeds in place.
//!
//! [librqbit]: https://crates.io/crates/librqbit

pub mod policy;
mod state;

use std::borrow::Cow;
use std::collections::HashSet;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use librqbit::api::TorrentIdOrHash;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ListenerMode,
    ListenerOptions, ManagedTorrent, ManagedTorrentState, Session, SessionOptions,
    SessionPersistenceConfig, TorrentStatsState,
};
pub use nikgob_core::indexer::TorrentPayload;

use crate::policy::{PolicyConfig, PolicyItem};
use crate::state::StateFile;

type ManagedTorrentHandle = Arc<ManagedTorrent>;

/// How long `inspect` waits for a magnet's metadata.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `add` waits for a magnet's metadata.
const ADD_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, thiserror::Error)]
pub enum TorrentError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("torrent engine: {0}")]
    Engine(String),
    #[error("no such torrent: {0}")]
    NotFound(u64),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("timed out waiting for torrent metadata")]
    Timeout,
}

pub type Result<T> = std::result::Result<T, TorrentError>;

fn engine(e: impl std::fmt::Display) -> TorrentError {
    TorrentError::Engine(format!("{e:#}"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct TorrentConfig {
    /// Payloads go here, one folder per multi-file torrent.
    pub downloads_dir: PathBuf,
    /// librqbit session persistence and the policy sidecar.
    pub state_dir: PathBuf,
    /// Stop uploading as soon as the selected files complete.
    pub no_seed: bool,
    /// Stop at this uploaded/selected ratio; 0 = no limit.
    pub seed_ratio: f32,
    /// Seed at most N days after completion; 0 = no limit.
    pub seed_days: u32,
    /// Cap on total uploaded GiB across held torrents; 0 = none.
    pub seed_limit_gb: u32,
    /// Download limit in KiB/s; 0 = unlimited.
    pub download_limit_kbps: u32,
    /// Upload limit in KiB/s; 0 = unlimited.
    pub upload_limit_kbps: u32,
}

impl TorrentConfig {
    fn policy(&self) -> PolicyConfig {
        PolicyConfig {
            no_seed: self.no_seed,
            seed_ratio: self.seed_ratio,
            seed_days: self.seed_days,
            seed_limit_gb: self.seed_limit_gb,
        }
    }
}

/// Network knobs; the defaults are what the app wants. Tests turn them off.
#[derive(Debug, Clone, Copy)]
pub struct NetOptions {
    pub dht: bool,
    /// Accept incoming peer connections (TCP + uTP, random port).
    pub listen: bool,
    pub upnp: bool,
    pub local_discovery: bool,
}

impl Default for NetOptions {
    fn default() -> Self {
        Self {
            dht: true,
            listen: true,
            upnp: true,
            local_discovery: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TorrentFile {
    pub index: usize,
    /// Path inside the torrent, `/` separated.
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TorrentMeta {
    /// Lowercase hex.
    pub info_hash: String,
    pub name: String,
    pub files: Vec<TorrentFile>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TorrentState {
    Initializing,
    Downloading,
    Paused,
    Seeding,
    /// Complete and not uploading (`no_seed`).
    Finished,
    Error(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TorrentStatus {
    /// Stable across restarts (librqbit persists it).
    pub id: u64,
    pub info_hash: String,
    pub name: String,
    pub state: TorrentState,
    /// 0..1 over the selected files.
    pub progress: f32,
    pub down_bps: u64,
    pub up_bps: u64,
    pub downloaded: u64,
    /// Lifetime uploaded bytes (persisted across restarts).
    pub uploaded: u64,
    /// Selected bytes.
    pub total: u64,
    pub eta: Option<Duration>,
    pub completed_at: Option<SystemTime>,
    /// Absolute on-disk paths of the selected files.
    pub selected_files: Vec<PathBuf>,
    /// Peers connected right now (0 when not live).
    pub peers: u32,
}

struct Inner {
    session: Arc<Session>,
    cfg: Mutex<TorrentConfig>,
    state: Mutex<StateFile>,
    state_dir: PathBuf,
}

/// Cheap to clone; all clones share one librqbit session.
#[derive(Clone)]
pub struct TorrentClient {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn bps(kbps: u32) -> Option<NonZeroU32> {
    NonZeroU32::new(kbps.saturating_mul(1024))
}

fn limits(cfg: &TorrentConfig) -> LimitsConfig {
    LimitsConfig {
        upload_bps: bps(cfg.upload_limit_kbps),
        download_bps: bps(cfg.download_limit_kbps),
    }
}

fn add_torrent(payload: TorrentPayload) -> AddTorrent<'static> {
    match payload {
        TorrentPayload::TorrentFile(bytes) => AddTorrent::from_bytes(bytes),
        TorrentPayload::Magnet(uri) => AddTorrent::Url(Cow::Owned(uri)),
    }
}

/// A status plus what the policy needs, computed in one pass over a handle.
struct Snapshot {
    status: TorrentStatus,
    complete: bool,
    completed_age: Option<Duration>,
}

impl TorrentClient {
    /// Starts the session with default networking and restores persisted torrents.
    pub async fn start(cfg: TorrentConfig) -> Result<Self> {
        Self::start_with(cfg, NetOptions::default()).await
    }

    /// Like [`start`](Self::start) with explicit network options.
    pub async fn start_with(cfg: TorrentConfig, net: NetOptions) -> Result<Self> {
        std::fs::create_dir_all(&cfg.downloads_dir)?;
        std::fs::create_dir_all(&cfg.state_dir)?;

        let mut state = StateFile::load(&cfg.state_dir, unix_now());
        if let Err(e) = state.save(&cfg.state_dir) {
            log::warn!("torrent state file not writable: {e}");
        }

        let dht = net.dht.then(|| DhtSessionConfig {
            persistence: Some(librqbit::dht::DhtPersistenceConfig {
                config_filename: Some(cfg.state_dir.join("dht.json")),
                ..Default::default()
            }),
            ..Default::default()
        });
        let listen = net.listen.then(|| ListenerOptions {
            mode: ListenerMode::TcpAndUtp,
            enable_upnp_port_forwarding: net.upnp,
            ..Default::default()
        });
        let opts = SessionOptions {
            dht,
            listen,
            fastresume: true,
            persistence: Some(SessionPersistenceConfig::Json {
                folder: Some(cfg.state_dir.join("session")),
            }),
            ratelimits: limits(&cfg),
            disable_local_service_discovery: !net.local_discovery,
            ..Default::default()
        };
        let session = Session::new_with_opts(cfg.downloads_dir.clone(), opts)
            .await
            .map_err(engine)?;

        Ok(Self {
            inner: Arc::new(Inner {
                session,
                state_dir: cfg.state_dir.clone(),
                cfg: Mutex::new(cfg),
                state: Mutex::new(state),
            }),
        })
    }

    /// Lists a torrent's files without downloading. Magnets resolve metadata
    /// via DHT/trackers, giving up after 60 s ([`TorrentError::Timeout`]).
    pub async fn inspect(&self, payload: TorrentPayload) -> Result<TorrentMeta> {
        let opts = AddTorrentOptions {
            list_only: true,
            ..Default::default()
        };
        let fut = self
            .inner
            .session
            .add_torrent(add_torrent(payload), Some(opts));
        let resp = tokio::time::timeout(INSPECT_TIMEOUT, fut)
            .await
            .map_err(|_| TorrentError::Timeout)?
            .map_err(engine)?;
        let AddTorrentResponse::ListOnly(r) = resp else {
            return Err(TorrentError::Engine("unexpected add response".into()));
        };
        let info_hash = r.info_hash.as_string();
        let files = r
            .info
            .iter_file_details()
            .enumerate()
            .filter(|(_, fd)| !fd.attrs().padding)
            .map(|(index, fd)| TorrentFile {
                index,
                path: fd.filename.to_vec().join("/"),
                size: fd.len,
            })
            .collect();
        let name = r
            .info
            .name()
            .map_or_else(|| info_hash.clone(), Cow::into_owned);
        Ok(TorrentMeta {
            info_hash,
            name,
            files,
        })
    }

    /// Adds a torrent downloading only `only_files` (all files if `None`).
    /// An already-known info hash returns its id, widening the selection if needed.
    pub async fn add(
        &self,
        payload: TorrentPayload,
        only_files: Option<Vec<usize>>,
    ) -> Result<u64> {
        if only_files.as_ref().is_some_and(Vec::is_empty) {
            return Err(TorrentError::Invalid("empty file selection".into()));
        }
        let opts = AddTorrentOptions {
            overwrite: true,
            only_files: only_files.clone(),
            ..Default::default()
        };
        let fut = self
            .inner
            .session
            .add_torrent(add_torrent(payload), Some(opts));
        let resp = tokio::time::timeout(ADD_TIMEOUT, fut)
            .await
            .map_err(|_| TorrentError::Timeout)?
            .map_err(engine)?;
        match resp {
            AddTorrentResponse::Added(id, _) => {
                self.flush();
                Ok(id as u64)
            }
            AddTorrentResponse::AlreadyManaged(id, handle) => {
                self.widen_selection(&handle, only_files).await?;
                Ok(id as u64)
            }
            AddTorrentResponse::ListOnly(_) => {
                Err(TorrentError::Engine("unexpected add response".into()))
            }
        }
    }

    async fn widen_selection(
        &self,
        handle: &ManagedTorrentHandle,
        wanted: Option<Vec<usize>>,
    ) -> Result<()> {
        let Some(current) = handle.only_files() else {
            return Ok(()); // already everything
        };
        let merged: HashSet<usize> = match wanted {
            Some(w) => current.iter().chain(&w).copied().collect(),
            None => {
                let n = handle
                    .with_metadata(|m| m.file_infos.len())
                    .map_err(engine)?;
                (0..n).collect()
            }
        };
        if merged.len() == current.len() {
            return Ok(());
        }
        tokio::time::timeout(Duration::from_secs(30), handle.wait_until_initialized())
            .await
            .map_err(|_| TorrentError::Timeout)?
            .map_err(engine)?;
        self.inner
            .session
            .update_only_files(handle, &merged)
            .await
            .map_err(engine)
    }

    fn handle(&self, id: u64) -> Result<ManagedTorrentHandle> {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.inner.session.get(TorrentIdOrHash::Id(i)))
            .ok_or(TorrentError::NotFound(id))
    }

    /// Current status of every torrent in the session.
    pub fn list(&self) -> Vec<TorrentStatus> {
        self.snapshots().into_iter().map(|s| s.status).collect()
    }

    fn snapshots(&self) -> Vec<Snapshot> {
        let handles: Vec<ManagedTorrentHandle> = self
            .inner
            .session
            .with_torrents(|it| it.map(|(_, h)| h.clone()).collect());
        let cfg = lock(&self.inner.cfg).clone();
        let now = unix_now();
        let mut state = lock(&self.inner.state);
        let mut out: Vec<Snapshot> = handles
            .iter()
            .map(|h| snapshot(h, &cfg, &mut state, now))
            .collect();
        if state.due(std::time::Instant::now())
            && let Err(e) = state.save(&self.inner.state_dir)
        {
            log::warn!("cannot save torrent state: {e}");
        }
        out.sort_by_key(|s| s.status.id);
        out
    }

    /// Takes the counters now and writes whatever changed.
    fn flush(&self) {
        let _ = self.snapshots();
        let mut state = lock(&self.inner.state);
        if state.dirty
            && let Err(e) = state.save(&self.inner.state_dir)
        {
            log::warn!("cannot save torrent state: {e}");
        }
    }

    pub async fn pause(&self, id: u64) -> Result<()> {
        let h = self.handle(id)?;
        if h.is_paused() {
            return Ok(());
        }
        self.inner.session.pause(&h).await.map_err(engine)
    }

    pub async fn resume(&self, id: u64) -> Result<()> {
        let h = self.handle(id)?;
        if h.with_state(|s| matches!(s, ManagedTorrentState::Live(_))) {
            return Ok(());
        }
        self.inner.session.unpause(&h).await.map_err(engine)
    }

    /// Removes from the session; `delete_files` also deletes the payload.
    pub async fn remove(&self, id: u64, delete_files: bool) -> Result<()> {
        let h = self.handle(id)?;
        let hash = h.info_hash().as_string();
        self.inner
            .session
            .delete(TorrentIdOrHash::Id(id as usize), delete_files)
            .await
            .map_err(engine)?;
        self.forget(&hash);
        Ok(())
    }

    fn forget(&self, hash: &str) {
        let mut state = lock(&self.inner.state);
        if state.torrents.remove(hash).is_some() {
            state.dirty = true;
        }
        if let Err(e) = state.save(&self.inner.state_dir) {
            log::warn!("cannot save torrent state: {e}");
        }
    }

    /// Applies new rate limits and seed policy live. `downloads_dir` and
    /// `state_dir` are fixed at start.
    pub fn set_config(&self, cfg: TorrentConfig) {
        let lim = limits(&cfg);
        self.inner
            .session
            .ratelimits
            .set_download_bps(lim.download_bps);
        self.inner.session.ratelimits.set_upload_bps(lim.upload_bps);
        *lock(&self.inner.cfg) = cfg;
    }

    /// Removes (keeping files) complete torrents that hit `no_seed`, the ratio,
    /// the day limit or the total-upload cap, and returns them. Call periodically.
    pub async fn enforce_seed_policy(&self) -> Vec<TorrentStatus> {
        let policy = lock(&self.inner.cfg).policy();
        let snaps = self.snapshots();
        let items: Vec<PolicyItem> = snaps
            .iter()
            .map(|s| PolicyItem {
                id: s.status.id,
                complete: s.complete,
                total: s.status.total,
                uploaded: s.status.uploaded,
                completed_age: s.completed_age,
            })
            .collect();

        let mut stopped = Vec::new();
        for id in policy::torrents_to_stop(&items, &policy) {
            let Some(snap) = snaps.iter().find(|s| s.status.id == id) else {
                continue;
            };
            match self
                .inner
                .session
                .delete(TorrentIdOrHash::Id(id as usize), false)
                .await
            {
                Ok(()) => {
                    self.forget(&snap.status.info_hash);
                    let mut status = snap.status.clone();
                    status.state = TorrentState::Finished;
                    status.up_bps = 0;
                    stopped.push(status);
                }
                Err(e) => log::warn!("cannot stop seeding torrent {id}: {e:#}"),
            }
        }
        stopped
    }

    /// Saves state and stops the session. Torrents resume on the next start.
    pub async fn shutdown(&self) {
        self.flush();
        self.inner.session.stop().await;
    }
}

fn snapshot(
    handle: &ManagedTorrentHandle,
    cfg: &TorrentConfig,
    state: &mut StateFile,
    now: u64,
) -> Snapshot {
    let stats = handle.stats();
    let hash = handle.info_hash().as_string();
    let live_state = matches!(stats.state, TorrentStatsState::Live);
    let known_state = live_state || matches!(stats.state, TorrentStatsState::Paused);
    let complete = known_state && stats.finished;

    let rec = state
        .torrents
        .entry(hash.clone())
        .or_insert_with(|| state::Record {
            added_at: now,
            ..Default::default()
        });
    let mut changed = false;
    if live_state {
        let live_up = stats.uploaded_bytes;
        let delta = if live_up >= rec.last_live {
            live_up - rec.last_live
        } else {
            live_up
        };
        rec.last_live = live_up;
        if delta > 0 {
            rec.uploaded += delta;
            changed = true;
        }
    } else {
        rec.last_live = 0;
    }
    if complete && rec.completed_at.is_none() {
        rec.completed_at = Some(now);
        changed = true;
        state.urgent = true;
    } else if known_state && !complete && rec.completed_at.is_some() {
        rec.completed_at = None;
        changed = true;
        state.urgent = true;
    }
    let uploaded = rec.uploaded;
    let completed_at = rec.completed_at;
    state.dirty |= changed;

    let (down_bps, up_bps) = stats.live.as_ref().map_or((0, 0), |l| {
        (l.download_speed.as_bytes(), l.upload_speed.as_bytes())
    });
    let peers = stats
        .live
        .as_ref()
        .map_or(0, |l| l.snapshot.peer_stats.live);
    // librqbit counts whole pieces, which spill into neighbouring unselected
    // files; per-file sizes and progress give exact selected bytes.
    let files = selected_files(handle);
    let (total, downloaded) = if stats.file_progress.is_empty() {
        (stats.total_bytes, stats.progress_bytes)
    } else {
        files.iter().fold((0, 0), |(t, d), f| {
            (
                t + f.size,
                d + stats
                    .file_progress
                    .get(f.index)
                    .copied()
                    .unwrap_or(0)
                    .min(f.size),
            )
        })
    };
    let progress = if total == 0 {
        if complete { 1.0 } else { 0.0 }
    } else {
        (downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32
    };
    let eta = (!complete && down_bps > 0)
        .then(|| Duration::from_secs(total.saturating_sub(downloaded) / down_bps));

    let torrent_state = match stats.state {
        TorrentStatsState::Error => TorrentState::Error(stats.error.clone().unwrap_or_default()),
        TorrentStatsState::Initializing { paused: true } | TorrentStatsState::Paused => {
            TorrentState::Paused
        }
        TorrentStatsState::Initializing { paused: false } => TorrentState::Initializing,
        TorrentStatsState::Live if !complete => TorrentState::Downloading,
        TorrentStatsState::Live if cfg.no_seed => TorrentState::Finished,
        TorrentStatsState::Live => TorrentState::Seeding,
    };

    Snapshot {
        status: TorrentStatus {
            id: handle.id() as u64,
            name: handle.name().unwrap_or_else(|| hash.clone()),
            info_hash: hash,
            state: torrent_state,
            progress,
            down_bps,
            up_bps,
            downloaded,
            uploaded,
            total,
            eta,
            completed_at: completed_at.map(|s| UNIX_EPOCH + Duration::from_secs(s)),
            selected_files: files.into_iter().map(|f| f.path).collect(),
            peers,
        },
        complete,
        completed_age: completed_at.map(|c| Duration::from_secs(now.saturating_sub(c))),
    }
}

struct Selected {
    index: usize,
    path: PathBuf,
    size: u64,
}

fn selected_files(handle: &ManagedTorrentHandle) -> Vec<Selected> {
    let only: Option<HashSet<usize>> = handle.only_files().map(|v| v.into_iter().collect());
    let root = handle.output_folder();
    handle
        .with_metadata(|m| {
            m.file_infos
                .iter()
                .enumerate()
                .filter(|(i, f)| !f.attrs.padding && only.as_ref().is_none_or(|o| o.contains(i)))
                .map(|(index, f)| Selected {
                    index,
                    path: root.join(&f.relative_filename),
                    size: f.len,
                })
                .collect()
        })
        .unwrap_or_default()
}
