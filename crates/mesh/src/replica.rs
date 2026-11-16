//! The library's files on the owner's devices.
//!
//! **Pulling** (a device with a storing profile): the ring holder tells
//! each device what it stores (its presence, [`Assigned`]; the last word is
//! kept in `mesh/replica.json` for when the holder is away). From the
//! synced catalog the device works out what it should hold — «Полная
//! копия»: every track's file and every lossless master; «Библиотека»:
//! every track's file; «Избранное»: «Любимые» and the pinned playlists up
//! to the cap — and pulls what it lacks from the owner's devices that have
//! it, through the tunnel: a song's best file by its hash from
//! `/v1/media/tracks/{id}/audio?hash=` (the original as it is:
//! [`nikgob_view::MeshFormat`]), a master from `/v1/replica/masters/{hash}`.
//! What it holds is what is on its disk now, not what rows say: a file
//! inside its folders (links resolved), the very file (what it hashed to
//! while its size and change time are the same, else read again, a few a
//! plan; one not read yet waits). At most [`AT_ONCE`] files at a time,
//! paced to [`RATE`], resumed from a part left by an earlier try (Range),
//! checked against its BLAKE3 before it is kept, paused by the owner
//! (here, or on the device that asks it in its presence; the pauses asked
//! here outlive a restart), never past [`MIN_FREE`] left on the disks it
//! writes to (the running pulls' bytes reserved, [`UNKNOWN_SIZE`] for a
//! file of a size not known; each write weighed against the free space
//! seen, looked at again every [`ROOM_STEP`]; each placement weighed on
//! the disk it goes to; a disk whose free space is not known gets
//! nothing). A new profile or new pins drop the old plan at once. The
//! parts wait in `<library>/.musiclib/replica` under names no one can
//! guess, found by listing the folder and opened never through a link; a
//! file lands as the library lays songs out (`Artist/YYYY - Album/NN -
//! Title.ext`, a master in the masters folder), only in folders inside
//! them, never over another file, checked again after a copy across disks,
//! and becomes this device's own (a source, a master row, a `replica` row).
//! The sending device writes nothing.
//!
//! **Copies of this computer's masters** (a device with a window): every
//! [`AUDIT_EVERY`] each owner device online is asked which of this
//! computer's masters it holds (`/v1/replica/check`) and the `replica`
//! rows follow its answer; right before masters go to the Trash the
//! devices read their copies again ([`recheck`]).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{Metadata, OpenOptions};
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Body;
use futures_util::StreamExt as _;
use nikgob_core::model::{Codec, Source, SourceKind};
use nikgob_library::Library;
use nikgob_core::model::TrackId;
use nikgob_library::replica::{CheckAnswer, CheckAsk, Held, HeldFile, Keep, MasterItem, MastersPage, Offer, Want, best_offers, held_satisfies, plan_tracks};
use nikgob_view::{Command, MeshProfile, MeshScope, MeshTransfer};
use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::Shared;
use crate::connect::{clean_ext, clean_hash, range_starts_at};
use crate::presence::Assigned;

/// Files pulled at once.
pub(crate) const AT_ONCE: usize = 2;
/// Bytes a second all pulls together take at most (the home network and
/// the music playing keep their room).
pub(crate) const RATE: f64 = (32u64 << 20) as f64;
/// What the pacing lets run ahead of it.
const BURST: Duration = Duration::from_millis(250);
/// What is left on the disk: a file that would go below waits.
pub(crate) const MIN_FREE: u64 = 2 << 30;
/// The pause between two files started.
const GAP: Duration = Duration::from_millis(200);
/// The whole library is planned again at most this often.
const PLAN_AGAIN: Duration = Duration::from_secs(120);
/// A device's list of masters is asked again after this.
const MASTERS_AGAIN: Duration = Duration::from_secs(30 * 60);
/// Jobs kept from one plan (the next plan comes when they are done).
const QUEUE_MAX: usize = 5_000;
/// A file's head is waited for this long, then its bytes this long apart.
const HEAD_WAIT: Duration = Duration::from_secs(15);
const STALL: Duration = Duration::from_secs(30);
/// A file that failed waits this long, doubling, at most an hour.
const RETRY_MIN: Duration = Duration::from_secs(60);
const RETRY_MAX: Duration = Duration::from_secs(3_600);
/// The largest file pulled (a hi-res disc image fits).
const FILE_MAX: u64 = 8 << 30;
/// Failures remembered at most.
const FAILS_MAX: usize = 10_000;
/// Masters a page asks for, hashes a check asks about.
const PAGE: u32 = 500;
/// The least time between two small requests of a loop.
const PACE: Duration = Duration::from_millis(70);
/// The largest JSON answer taken.
const JSON_MAX: usize = 32 << 20;
/// A JSON request's answer is waited for this long (a check reads files).
const ASK_WAIT: Duration = Duration::from_secs(120);
/// The copies of this computer's masters are asked about this often per
/// device (sooner again after a failure).
const AUDIT_EVERY: Duration = Duration::from_secs(10 * 60);
const AUDIT_RETRY: Duration = Duration::from_secs(2 * 60);
const AUDIT_TICK: Duration = Duration::from_secs(60);
/// Masters a second look asks a device about at once, and how long it may
/// read them.
const RECHECK_BATCH: usize = 10;
const RECHECK_WAIT: Duration = Duration::from_secs(5 * 60);
/// Where the profile the holder gave is kept, in the mesh's folder.
const SAVED: &str = "replica.json";
/// Where the pauses asked here are kept, in the mesh's folder.
pub(crate) const PAUSED: &str = "replica-paused.json";
/// While a file is written, the disk's free space is looked at again each
/// time this much more came (each write is weighed against what was seen
/// then, less what came since).
const ROOM_STEP: u64 = 4 << 20;
/// What a file of a size not known keeps free for itself on the disks
/// while it is pulled.
const UNKNOWN_SIZE: u64 = 200 << 20;
/// A disk without room (or whose room is not known) is looked at again
/// after this.
const ROOM_WAIT: Duration = Duration::from_secs(60);
/// Files of «Избранное» whose size is not known are asked for it, at most
/// this many a plan.
const SIZE_ASKS: usize = 200;
/// Sizes learned (or asked for in vain) kept at most.
const SIZES_MAX: usize = 50_000;
/// Files here whose hash is not known for their size and change time are
/// read again, at most this many and this many bytes a plan (the first one
/// whatever its size); the others wait for the next plan.
const REHASH_FILES: usize = 200;
const REHASH_BYTES: u64 = 4 << 30;
/// The longest note of a part read.
const NOTE_MAX: u64 = 512;
/// Windows: open a link or another reparse point itself, not what it
/// points to; the attribute that tells one.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;

/// What the service keeps of the copies (in [`Shared`]).
#[derive(Default)]
pub(crate) struct ReplicaShared {
    /// This device's pull now (none: nothing to pull).
    transfer: Mutex<Option<MeshTransfer>>,
    /// The devices whose pulls the owner paused here (this one's id: its
    /// own).
    paused: Mutex<HashSet<String>>,
    /// Wakes the loop (a pause, a resume).
    poke: Notify,
    /// Where the pauses are kept (none: not kept).
    file: Option<PathBuf>,
}

impl ReplicaShared {
    /// The pauses kept in `file`: restored before the pull loop starts.
    pub(crate) fn load(file: PathBuf) -> Self {
        let paused: HashSet<String> =
            std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok()).unwrap_or_default().into_iter().collect();
        if !paused.is_empty() {
            log::info!("replica: {} devices' pulls paused here", paused.len());
        }
        Self { paused: Mutex::new(paused), file: Some(file), ..Self::default() }
    }

    pub(crate) fn transfer(&self) -> Option<MeshTransfer> {
        *self.transfer.lock()
    }

    pub(crate) fn pause_list(&self) -> Vec<String> {
        let mut v: Vec<String> = self.paused.lock().iter().cloned().collect();
        v.sort();
        v
    }

    pub(crate) fn set_paused(&self, node: &str, on: bool) {
        {
            let mut p = self.paused.lock();
            if on {
                p.insert(node.to_owned());
            } else {
                p.remove(node);
            }
        }
        if let Some(file) = &self.file {
            let list = self.pause_list();
            match serde_json::to_vec(&list) {
                Ok(bytes) => {
                    if let Err(e) = crate::keys::write_atomic(file, &bytes, false) {
                        log::warn!("replica: the pauses not kept: {}: {e}", file.display());
                    }
                }
                Err(e) => log::warn!("replica: the pauses not kept: {e}"),
            }
        }
        self.poke.notify_one();
    }
}

/// Keeps the pulls' bytes under [`RATE`] together.
pub(crate) struct Pace {
    rate: f64,
    next: Mutex<Option<Instant>>,
}

impl Pace {
    pub(crate) fn new(rate: f64) -> Self {
        Self { rate: rate.max(1.0), next: Mutex::new(None) }
    }

    /// Waits as long as `n` bytes take at the rate.
    pub(crate) async fn take(&self, n: usize) {
        let wait = {
            let now = Instant::now();
            let mut next = self.next.lock();
            let at = next.filter(|t| *t > now).unwrap_or(now) + Duration::from_secs_f64(n as f64 / self.rate);
            *next = Some(at);
            at.saturating_duration_since(now).saturating_sub(BURST)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

/// What the running pulls share.
pub(crate) struct Ctl {
    /// Stop between two pieces (paused, or nothing to pull any more): the
    /// part stays for the next try.
    pub(crate) stop: AtomicBool,
    /// The plan's number: a pull of an older plan stops between two
    /// pieces (a new profile or new pins).
    pub(crate) era: AtomicU64,
    /// Bytes pulled in this round (a resumed part counts whole).
    pub(crate) received: AtomicU64,
    /// Bytes the running pulls may still write: kept free on the disks
    /// ([`Reserve`]).
    pub(crate) reserved: AtomicU64,
    /// What the disks keep free besides ([`MIN_FREE`]).
    pub(crate) keep_free: u64,
    pub(crate) pace: Pace,
}

impl Ctl {
    pub(crate) fn new(rate: f64) -> Self {
        Self {
            stop: AtomicBool::new(false),
            era: AtomicU64::new(0),
            received: AtomicU64::new(0),
            reserved: AtomicU64::new(0),
            keep_free: MIN_FREE,
            pace: Pace::new(rate),
        }
    }

    /// Whether a pull of plan `era` stops now.
    fn halted(&self, era: u64) -> bool {
        self.stop.load(Ordering::Relaxed) || self.era.load(Ordering::Relaxed) != era
    }
}

/// A running pull's share of the room kept on the disks
/// ([`Ctl::reserved`]): what it may still write; what it did not write
/// goes back when it ends.
pub(crate) struct Reserve {
    ctl: Arc<Ctl>,
    left: u64,
}

impl Reserve {
    pub(crate) fn new(ctl: Arc<Ctl>, bytes: u64) -> Self {
        ctl.reserved.fetch_add(bytes, Ordering::Relaxed);
        Self { ctl, left: bytes }
    }

    /// `n` bytes written: that much less is kept for them.
    fn wrote(&mut self, n: u64) {
        let k = n.min(self.left);
        self.left -= k;
        self.ctl.reserved.fetch_sub(k, Ordering::Relaxed);
    }

    /// What the other running pulls keep.
    fn others(&self) -> u64 {
        self.ctl.reserved.load(Ordering::Relaxed).saturating_sub(self.left)
    }
}

impl Drop for Reserve {
    fn drop(&mut self) {
        self.ctl.reserved.fetch_sub(self.left, Ordering::Relaxed);
    }
}

/// What a placement must leave on the disk it writes to: the reserve and
/// what the other pulls keep.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Room {
    pub(crate) keep: u64,
    pub(crate) others: u64,
}

impl Room {
    /// Whether the disk of `dir` takes `size` more bytes and still keeps
    /// it; a disk whose free space is not known does not.
    fn fits(&self, dir: &Path, size: u64) -> bool {
        existing(dir).and_then(nikgob_runtime::free_bytes).is_some_and(|free| free >= size.saturating_add(self.others).saturating_add(self.keep))
    }
}

/// Why a pull did not end with the file kept.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failed {
    /// Stopped between two pieces: its part stays, it goes on later.
    Stopped,
    /// The disk came near its reserve: its part stays, it waits for room.
    NoRoom,
    /// No device asked has it.
    NotThere,
    Bad(String),
}

/// A file to pull.
#[derive(Clone, Debug)]
enum Job {
    /// A song's file: from the devices of its offers.
    Track(Want),
    /// A lossless master, from the devices that list it.
    Master { item: MasterItem, from: Vec<String> },
}

impl Job {
    /// The name of its part (and of its failures).
    fn key(&self) -> String {
        match self {
            Self::Track(w) => format!("t-{}", w.track_id),
            Self::Master { item, .. } => format!("m-{}", item.hash),
        }
    }

    fn size(&self) -> u64 {
        match self {
            Self::Track(w) => w.size(),
            Self::Master { item, .. } => item.size,
        }
    }

    /// The very file it is (its BLAKE3).
    fn hash(&self) -> &str {
        match self {
            Self::Track(w) => &w.file_hash,
            Self::Master { item, .. } => &item.hash,
        }
    }

    /// The devices it may come from.
    fn holders(&self) -> Vec<String> {
        match self {
            Self::Track(w) => w.offers.iter().map(|o| o.node_id.clone()).collect(),
            Self::Master { from, .. } => from.clone(),
        }
    }
}

/// Where the files land: the library folder and the masters folder.
#[derive(Clone, Debug)]
pub(crate) struct Dirs {
    pub(crate) library: PathBuf,
    pub(crate) masters: PathBuf,
}

/// A file that failed: how often, and when it is tried again.
struct Waiting {
    fails: u32,
    until: Instant,
}

/// What the loop keeps between turns.
#[derive(Default)]
struct Puller {
    queue: VecDeque<Job>,
    /// When the queue was planned, and for what.
    planned_at: Option<Instant>,
    plan_for: Option<Assigned>,
    /// The last plan was cut at [`QUEUE_MAX`]: the next comes as soon as
    /// it is done, the round goes on.
    cut: bool,
    failed: HashMap<String, Waiting>,
    /// The devices' lists of masters, and when each came.
    masters: HashMap<String, (Instant, Vec<MasterItem>)>,
    /// The sizes of files no device told, learned by asking (None: asked
    /// in vain; asked again once all the others were), by hash, for the
    /// session.
    sizes: HashMap<String, Option<u64>>,
    /// The holder's last word (also kept on disk).
    saved: Option<Assigned>,
    next_start: Option<Instant>,
    /// No file starts before this (a disk without room).
    room_wait: Option<Instant>,
    /// The round's bytes in all (done ones included).
    total: u64,
    /// For the speed: the bytes and the time of the last turn.
    last: Option<(u64, Instant)>,
    speed: f64,
}

impl Puller {
    fn waiting(&self, key: &str) -> bool {
        self.failed.get(key).is_some_and(|w| Instant::now() < w.until)
    }

    fn fail(&mut self, key: String) {
        if self.failed.len() >= FAILS_MAX && !self.failed.contains_key(&key) {
            self.failed.clear();
        }
        let w = self.failed.entry(key).or_insert(Waiting { fails: 0, until: Instant::now() });
        w.fails = w.fails.saturating_add(1);
        w.until = Instant::now() + retry_after(w.fails);
    }
}

/// The wait after `fails` failures of a file.
pub(crate) fn retry_after(fails: u32) -> Duration {
    RETRY_MIN.saturating_mul(1u32 << fails.saturating_sub(1).min(10)).min(RETRY_MAX)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> nikgob_library::Result<T> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

// ------------------------------------------------------------- the ring

/// The owner's devices online now, by node id (none when this device is
/// not an owner's).
fn owner_peers(sh: &Shared) -> Vec<String> {
    let s = sh.state.lock();
    let Some(r) = s.ring.as_ref() else { return Vec::new() };
    if r.me.scope != MeshScope::Owner {
        return Vec::new();
    }
    let mut out: Vec<String> = r
        .others()
        .filter(|c| c.scope == MeshScope::Owner)
        .filter(|c| c.node_id().and_then(|id| s.peers.get(&id)).is_some_and(|p| p.link.is_some()))
        .map(|c| c.node.clone())
        .collect();
    out.sort();
    out
}

/// The presences of the owner's devices online now.
fn owner_presences(sh: &Shared) -> Vec<crate::presence::Presence> {
    let s = sh.state.lock();
    let Some(r) = s.ring.as_ref() else { return Vec::new() };
    s.peers
        .iter()
        .filter(|(id, p)| p.link.is_some() && r.trusted(id).is_some_and(|c| c.scope == MeshScope::Owner))
        .filter_map(|(_, p)| p.presence.clone())
        .collect()
}

/// What the ring holder (among `presences`) gives device `me` to store.
pub(crate) fn assigned_by(presences: &[crate::presence::Presence], me: &str) -> Option<Assigned> {
    presences.iter().filter(|p| p.holder).find_map(|p| p.assign.iter().find(|a| a.node == me).cloned())
}

/// Whether the pulls of `me` are paused: by the owner here (`here`), or by
/// a device that asks it in its presence.
pub(crate) fn paused_by(here: &HashSet<String>, presences: &[crate::presence::Presence], me: &str) -> bool {
    here.contains(me) || presences.iter().any(|p| p.pause.iter().any(|n| n == me))
}

/// The holder's word now, or the last one kept; a new word is kept on
/// disk. None when this device is in no ring (the kept word goes).
fn assignment(sh: &Shared, me: &str, st: &mut Puller) -> Option<Assigned> {
    let in_ring = sh.state.lock().ring.as_ref().is_some_and(|r| r.me.scope == MeshScope::Owner);
    let file = sh.paths.dir.join(SAVED);
    if !in_ring {
        if st.saved.take().is_some() {
            let _ = std::fs::remove_file(&file);
        }
        return None;
    }
    if let Some(now) = assigned_by(&owner_presences(sh), me)
        && st.saved.as_ref() != Some(&now)
    {
        match serde_json::to_vec(&now) {
            Ok(bytes) => {
                if let Err(e) = crate::keys::write_atomic(&file, &bytes, false) {
                    log::warn!("replica: {}: {e}", file.display());
                }
            }
            Err(e) => log::warn!("replica: the profile not kept: {e}"),
        }
        log::info!("replica: this device stores «{}»", now.profile.label());
        st.saved = Some(now);
    }
    st.saved.clone()
}

/// The holder's last word kept on disk.
fn load_saved(sh: &Shared) -> Option<Assigned> {
    std::fs::read(sh.paths.dir.join(SAVED)).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

// ------------------------------------------------------------ requests

async fn get_json<T: DeserializeOwned>(sh: &Arc<Shared>, peer: &str, uri: &str) -> Result<T, String> {
    let req = http::Request::get(uri).body(Body::empty()).map_err(|e| e.to_string())?;
    answer(sh, peer, req, ASK_WAIT).await
}

async fn post_json<T: DeserializeOwned>(sh: &Arc<Shared>, peer: &str, uri: &str, body: &impl Serialize, wait: Duration) -> Result<T, String> {
    let bytes = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let req = http::Request::post(uri)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .map_err(|e| e.to_string())?;
    answer(sh, peer, req, wait).await
}

/// `req` to device `peer`, its JSON answer, all of it within `wait`.
async fn answer<T: DeserializeOwned>(sh: &Arc<Shared>, peer: &str, req: http::Request<Body>, wait: Duration) -> Result<T, String> {
    let whole = async {
        let resp = sh.request(peer, req).await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), JSON_MAX).await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("{}: {}", status.as_u16(), String::from_utf8_lossy(&bytes[..bytes.len().min(200)])));
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    };
    tokio::time::timeout(wait, whole).await.unwrap_or_else(|_| Err("timed out".to_owned()))
}

/// The masters device `peer` lists, all pages.
async fn masters_of(sh: &Arc<Shared>, peer: &str) -> Result<Vec<MasterItem>, String> {
    let mut out = Vec::new();
    let mut after = String::new();
    loop {
        let uri = if after.is_empty() { format!("/v1/replica/masters?limit={PAGE}") } else { format!("/v1/replica/masters?after={after}&limit={PAGE}") };
        let page: MastersPage = get_json(sh, peer, &uri).await?;
        out.extend(page.masters.into_iter().filter(|m| clean_hash(&m.hash).as_deref() == Some(m.hash.as_str())));
        match page.next.as_deref().and_then(clean_hash) {
            Some(next) if next > after => after = next,
            _ => break,
        }
        tokio::time::sleep(PACE).await;
    }
    Ok(out)
}

// ------------------------------------------------------------ planning

/// A locator as a path (a relative one is the library folder's).
fn absolute(locator: &str, library: &Path) -> PathBuf {
    let p = Path::new(locator);
    if p.is_absolute() { p.to_owned() } else { library.join(p) }
}

/// The library folder's and the masters folder's real paths (those there).
fn real_roots(dirs: &Dirs) -> Vec<PathBuf> {
    [&dirs.library, &dirs.masters].into_iter().filter_map(|d| std::fs::canonicalize(d).ok()).collect()
}

/// A file here as it is now: its real path (links resolved) inside one of
/// `roots`, a plain file; nothing else.
fn real_file(path: &Path, roots: &[PathBuf]) -> Option<(PathBuf, Metadata)> {
    let real = std::fs::canonicalize(path).ok()?;
    if !roots.iter().any(|r| real.starts_with(r)) {
        return None;
    }
    let m = std::fs::metadata(&real).ok().filter(Metadata::is_file)?;
    Some((real, m))
}

/// When a file was changed, in unix ms.
fn mtime_ms(m: &Metadata) -> Option<i64> {
    m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

/// The files read again for their hash in one plan, at most.
pub(crate) struct Rehash {
    files: usize,
    bytes: u64,
}

impl Rehash {
    pub(crate) fn new() -> Self {
        Self { files: REHASH_FILES, bytes: REHASH_BYTES }
    }

    /// Whether a file of `size` bytes may still be read in this plan
    /// (counted when it may).
    fn take(&mut self, size: u64) -> bool {
        let first = self.files == REHASH_FILES;
        if self.files == 0 || (size > self.bytes && !first) {
            return false;
        }
        self.files -= 1;
        self.bytes = self.bytes.saturating_sub(size);
        true
    }
}

/// What a file here hashes to.
enum Hashed {
    Known(String),
    /// Not read in this plan: it waits for the next.
    Later,
    Unreadable,
}

/// What the file at `real` (`m` its metadata) hashes to: what it hashed to
/// while its size and change time are the same, else read again when
/// `rehash` lets it (and noted for the next time).
fn hash_here(lib: &Library, real: &Path, m: &Metadata, rehash: &mut Rehash) -> Hashed {
    let key = real.to_string_lossy();
    let mtime = mtime_ms(m);
    if let Some(t) = mtime
        && let Ok(Some((size, at, hash))) = lib.file_stamp(&key)
        && size == m.len()
        && at == t
    {
        return Hashed::Known(hash);
    }
    if !rehash.take(m.len()) {
        return Hashed::Later;
    }
    let Ok(hash) = blake3_plain(real) else { return Hashed::Unreadable };
    if let Some(t) = mtime
        && let Err(e) = lib.put_file_stamp(&key, m.len(), t, &hash)
    {
        log::debug!("replica: the hash of {} not noted: {e}", real.display());
    }
    Hashed::Known(hash)
}

/// Notes what the file at `path` hashes to (by its real path, with its
/// size and change time now): a later plan takes it without reading it.
fn stamp(lib: &Library, path: &Path, hash: &str) {
    let Ok(real) = std::fs::canonicalize(path) else { return };
    let Ok(m) = std::fs::metadata(&real) else { return };
    let Some(t) = mtime_ms(&m) else { return };
    if let Err(e) = lib.put_file_stamp(&real.to_string_lossy(), m.len(), t, hash) {
        log::debug!("replica: the hash of {} not noted: {e}", real.display());
    }
}

/// The songs' files this device holds now, each checked: a live local
/// source whose file is inside the library or the masters folder (links
/// resolved), a plain file, the very file its source names (its hash, as
/// [`hash_here`] knows it), its size as it is on the disk. And the tracks
/// with a file here not read yet (`rehash` ran out), with its bytes: they
/// wait for the next plan.
#[allow(clippy::type_complexity)]
pub(crate) fn held_on_disk(lib: &Library, dirs: &Dirs, rehash: &mut Rehash) -> nikgob_library::Result<(HashMap<TrackId, Vec<HeldFile>>, HashMap<TrackId, u64>)> {
    let roots = real_roots(dirs);
    let mut held: HashMap<TrackId, Vec<HeldFile>> = HashMap::new();
    let mut pending: HashMap<TrackId, u64> = HashMap::new();
    for mut f in lib.replica_held_files()? {
        let Some((real, m)) = real_file(&absolute(&f.locator, &dirs.library), &roots) else { continue };
        f.size = m.len();
        match hash_here(lib, &real, &m, rehash) {
            // A file whose source names another hash is not that file
            // any more.
            Hashed::Known(h) if f.file_hash.as_deref().is_none_or(|want| want == h) => {
                f.file_hash = Some(h);
                held.entry(f.track_id).or_default().push(f);
            }
            Hashed::Later => {
                let n = pending.entry(f.track_id).or_default();
                *n = n.saturating_add(f.size);
            }
            Hashed::Known(_) | Hashed::Unreadable => {}
        }
    }
    Ok((held, pending))
}

/// Whether `s` is a BLAKE3 as the library writes it (64 lowercase hex).
fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One file of master `hash` at `path`: held when it is that very file
/// inside `roots`, waiting when it was not read yet.
fn check_master(lib: &Library, path: &Path, roots: &[PathBuf], hash: &str, rehash: &mut Rehash, held: &mut HashSet<String>, pending: &mut HashSet<String>) {
    if held.contains(hash) {
        return;
    }
    let Some((real, m)) = real_file(path, roots) else { return };
    match hash_here(lib, &real, &m, rehash) {
        Hashed::Known(h) if h == hash => {
            held.insert(hash.to_owned());
        }
        Hashed::Later => {
            pending.insert(hash.to_owned());
        }
        Hashed::Known(_) | Hashed::Unreadable => {}
    }
}

/// The masters this device holds now, each checked to be its file inside
/// the masters folder (links resolved): those its master rows keep by
/// name, and the plain files kept flat (`{hash}.<ext>` right in the
/// masters folder, a disc image's too). And those with a file here not
/// read yet: they wait for the next plan.
pub(crate) fn masters_on_disk(lib: &Library, masters: &Path, rehash: &mut Rehash) -> nikgob_library::Result<(HashSet<String>, HashSet<String>)> {
    let (mut held, mut pending) = (HashSet::new(), HashSet::new());
    let Ok(root) = std::fs::canonicalize(masters) else { return Ok((held, pending)) };
    let roots = [root.clone()];
    for (h, p) in lib.master_paths()? {
        if let Some(p) = p {
            check_master(lib, Path::new(&p), &roots, &h, rehash, &mut held, &mut pending);
        }
    }
    if let Ok(rd) = std::fs::read_dir(&root) {
        for p in rd.flatten().map(|e| e.path()) {
            let side = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("cue"));
            if side || !std::fs::symlink_metadata(&p).is_ok_and(|m| plain_meta(&m)) {
                continue;
            }
            if let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned())
                && is_hash(&stem)
            {
                check_master(lib, &p, &roots, &stem, rehash, &mut held, &mut pending);
            }
        }
    }
    pending.retain(|h| !held.contains(h));
    Ok((held, pending))
}

/// What a plan starts from: the offers of the devices online, «Избранное»'s
/// order, what this device holds on its disk and the tracks whose file
/// here waits to be read.
struct Ground {
    offers: Vec<Offer>,
    keep: Keep,
    held: HashMap<TrackId, Vec<HeldFile>>,
    pending: HashMap<TrackId, u64>,
}

fn ground(lib: &Library, a: &Assigned, peers: &[String], dirs: &Dirs) -> nikgob_library::Result<Option<Ground>> {
    let keep = match a.profile {
        MeshProfile::Full | MeshProfile::Library => Keep::All,
        MeshProfile::Favorites { cap_bytes } => Keep::Ordered { order: lib.replica_order(a.liked, &a.playlists)?, cap: cap_bytes },
        MeshProfile::SoundOnly => return Ok(None),
    };
    let (held, pending) = held_on_disk(lib, dirs, &mut Rehash::new())?;
    Ok(Some(Ground { offers: lib.replica_offers(peers)?, keep, held, pending }))
}

/// The best files of «Избранное»'s tracks whose size no device told and
/// that were not asked about yet (`known`), in its order, at most
/// [`SIZE_ASKS`]; a track held (or whose file here waits to be read) is
/// not asked about.
fn sizes_to_ask(g: &Ground, known: &HashMap<String, Option<u64>>) -> Vec<Offer> {
    let Keep::Ordered { order, .. } = &g.keep else { return Vec::new() };
    let best = best_offers(&g.offers);
    let none: Vec<HeldFile> = Vec::new();
    let mut seen = HashSet::new();
    order
        .iter()
        .filter(|t| seen.insert(**t) && !g.pending.contains_key(t))
        .filter_map(|t| best.get(t))
        .filter(|o| o.size == 0 && !known.contains_key(&o.file_hash))
        .filter(|o| !held_satisfies(o, g.held.get(&o.track_id).unwrap_or(&none)))
        .take(SIZE_ASKS)
        .cloned()
        .collect()
}

/// The sizes learned put into the offers that told none.
fn learn(offers: &mut [Offer], sizes: &HashMap<String, Option<u64>>) {
    for o in offers.iter_mut().filter(|o| o.size == 0) {
        if let Some(Some(n)) = sizes.get(&o.file_hash) {
            o.size = *n;
        }
    }
}

/// The size of the file `hash` of track `track` on device `peer`, from the
/// answer to its first byte.
async fn size_of(sh: &Arc<Shared>, peer: &str, track: TrackId, hash: &str) -> Option<u64> {
    let uri = format!("/v1/media/tracks/{track}/audio?profile={}&hash={hash}", nikgob_view::MeshFormat::Orig.key());
    let req = http::Request::get(uri).header(http::header::RANGE, "bytes=0-0").body(Body::empty()).ok()?;
    let resp = tokio::time::timeout(HEAD_WAIT, sh.request(peer, req)).await.ok()?.ok()?;
    let total = match resp.status() {
        http::StatusCode::PARTIAL_CONTENT => header(&resp, "content-range").and_then(|v| v.rsplit_once('/')).and_then(|(_, n)| n.trim().parse().ok()),
        http::StatusCode::OK => header(&resp, "content-length").and_then(|v| v.trim().parse().ok()),
        _ => None,
    };
    drop(resp);
    total.filter(|n| *n > 0)
}

/// The masters to pull: those the devices list that this device neither
/// keeps nor holds a copy of (`held`), each with the devices listing it,
/// in hash order.
pub(crate) fn plan_masters(lists: &HashMap<String, (Instant, Vec<MasterItem>)>, held: &HashSet<String>) -> Vec<(MasterItem, Vec<String>)> {
    let mut by_hash: BTreeMap<String, (MasterItem, Vec<String>)> = BTreeMap::new();
    let mut peers: Vec<&String> = lists.keys().collect();
    peers.sort();
    for peer in peers {
        for item in &lists[peer].1 {
            if held.contains(&item.hash) {
                continue;
            }
            by_hash.entry(item.hash.clone()).or_insert_with(|| (item.clone(), Vec::new())).1.push(peer.clone());
        }
    }
    by_hash.into_values().collect()
}

/// A new plan: what `a` wants and this device lacks.
async fn replan(sh: &Arc<Shared>, lib: &Library, dirs: &Dirs, a: &Assigned, peers: &[String], st: &mut Puller, ctl: &Ctl) {
    let ground = {
        let (l, a, p, d) = (lib.clone(), a.clone(), peers.to_vec(), dirs.clone());
        match blocking(move || ground(&l, &a, &p, &d)).await {
            Ok(g) => g,
            Err(e) => {
                log::warn!("replica: the songs to keep not worked out: {e}");
                st.planned_at = Some(Instant::now());
                return;
            }
        }
    };
    let songs = match ground {
        None => Vec::new(),
        Some(mut g) => {
            // «Избранное» charges each file with its real size: one no
            // device told is asked for it first, the sizes learned kept
            // for the next plans (so the asks go on to the next files; one
            // asked in vain is asked again once all the others were); one
            // still not known is never taken as free.
            learn(&mut g.offers, &st.sizes);
            let ask = sizes_to_ask(&g, &st.sizes);
            if ask.is_empty() {
                st.sizes.retain(|_, n| n.is_some());
            }
            for o in ask {
                let n = size_of(sh, &o.node_id, o.track_id, &o.file_hash).await;
                if st.sizes.len() >= SIZES_MAX {
                    st.sizes.clear();
                }
                st.sizes.insert(o.file_hash.clone(), n);
                tokio::time::sleep(PACE).await;
            }
            learn(&mut g.offers, &st.sizes);
            match tokio::task::spawn_blocking(move || plan_tracks(&g.keep, g.offers, &g.held, &g.pending)).await {
                Ok(w) => w,
                Err(e) => {
                    log::warn!("replica: the songs to keep not worked out: {e}");
                    st.planned_at = Some(Instant::now());
                    return;
                }
            }
        }
    };
    let mut jobs: Vec<Job> = songs.into_iter().map(Job::Track).collect();
    if a.profile == MeshProfile::Full {
        for peer in peers {
            let fresh = st.masters.get(peer).is_some_and(|(at, _)| at.elapsed() < MASTERS_AGAIN);
            if fresh {
                continue;
            }
            match masters_of(sh, peer).await {
                Ok(list) => {
                    st.masters.insert(peer.clone(), (Instant::now(), list));
                }
                Err(e) => log::info!("replica: the masters of {peer} not listed: {e}"),
            }
        }
        st.masters.retain(|p, _| peers.contains(p));
        // Held, or a file here not read yet (it waits): not pulled.
        let skip = {
            let (l, m) = (lib.clone(), dirs.masters.clone());
            match blocking(move || masters_on_disk(&l, &m, &mut Rehash::new())).await {
                Ok((held, pending)) => held.into_iter().chain(pending).collect::<HashSet<String>>(),
                Err(e) => {
                    log::warn!("replica: the masters here not read: {e}");
                    st.planned_at = Some(Instant::now());
                    return;
                }
            }
        };
        jobs.extend(plan_masters(&st.masters, &skip).into_iter().map(|(item, from)| Job::Master { item, from }));
    }
    jobs.retain(|j| !st.waiting(&j.key()));
    let all: u64 = jobs.iter().map(Job::size).sum();
    let goes_on = st.cut;
    st.cut = jobs.len() > QUEUE_MAX;
    jobs.truncate(QUEUE_MAX);
    if goes_on {
        st.total = ctl.received.load(Ordering::Relaxed) + all;
    } else {
        ctl.received.store(0, Ordering::Relaxed);
        st.total = all;
        st.last = None;
        st.speed = 0.0;
    }
    if !jobs.is_empty() {
        log::info!("replica: {} files to pull ({} MB)", jobs.len(), all >> 20);
    }
    st.queue = jobs.into();
    st.planned_at = Some(Instant::now());
    st.plan_for = Some(a.clone());
}

// ------------------------------------------------------------- the loop

/// A pull's end: the job, the plan it was of, how it went.
type Done = (Job, u64, Result<u64, Failed>);

/// Pulls what this device should hold, until the mesh stops (it is aborted
/// then; the running pulls go with it, their parts stay).
pub(crate) async fn run(sh: Arc<Shared>) {
    let Some(lib) = sh.library.clone() else { return };
    let me = sh.key.public().to_string();
    let ctl = Arc::new(Ctl::new(RATE));
    let mut st = Puller { saved: load_saved(&sh), ..Puller::default() };
    let mut tasks: JoinSet<Done> = JoinSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = sh.replica.poke.notified() => {}
            Some(done) = tasks.join_next(), if !tasks.is_empty() => match done {
                Ok((job, _, Ok(bytes))) => {
                    log::debug!("replica: {} kept ({bytes} bytes)", job.key());
                    st.failed.remove(&job.key());
                }
                // A pull of an older plan is dropped with it.
                Ok((job, era, Err(Failed::Stopped))) => {
                    if era == ctl.era.load(Ordering::Relaxed) {
                        st.queue.push_front(job);
                    }
                }
                Ok((job, era, Err(Failed::NoRoom))) => {
                    log::info!("replica: the disk is near its reserve; {} waits", job.key());
                    st.room_wait = Some(Instant::now() + ROOM_WAIT);
                    if era == ctl.era.load(Ordering::Relaxed) {
                        st.queue.push_front(job);
                    }
                }
                Ok((job, _, Err(e))) => {
                    log::info!("replica: {} not pulled: {e:?}", job.key());
                    st.fail(job.key());
                }
                Err(e) => log::warn!("replica: a pull ended badly: {e}"),
            },
        }
        turn(&sh, &lib, &me, &ctl, &mut st, &mut tasks).await;
    }
}

/// The nearest folder of `p` that exists (itself, or a parent).
fn existing(p: &Path) -> Option<&Path> {
    p.ancestors().find(|a| !a.as_os_str().is_empty() && a.exists())
}

/// Whether each disk of `dirs` keeps [`MIN_FREE`] after `need` more bytes
/// and the running pulls' `reserved` ones; a disk whose free space is not
/// known has no room.
pub(crate) fn has_room(dirs: &[&Path], need: u64, reserved: u64) -> bool {
    dirs.iter().all(|d| {
        existing(d).and_then(nikgob_runtime::free_bytes).is_some_and(|free| free >= need.saturating_add(reserved).saturating_add(MIN_FREE))
    })
}

/// The old plan goes: no more pulls of it, the running ones stop between
/// two pieces (their parts stay), the next plan comes at once.
fn drop_plan(st: &mut Puller, ctl: &Ctl) {
    ctl.era.fetch_add(1, Ordering::Relaxed);
    st.queue.clear();
    st.plan_for = None;
    st.planned_at = None;
    st.cut = false;
}

/// One turn: whether this device keeps copies now, a plan when due, the
/// next pulls started, the numbers shown.
async fn turn(sh: &Arc<Shared>, lib: &Library, me: &str, ctl: &Arc<Ctl>, st: &mut Puller, tasks: &mut JoinSet<Done>) {
    let running = sh.node.lock().is_some();
    let peers = if running { owner_peers(sh) } else { Vec::new() };
    let assigned = assignment(sh, me, st);
    let dirs = {
        let st = sh.rt.state();
        let library = st.prefs.library_dir.clone();
        let masters = if st.prefs.masters_dir.as_os_str().is_empty() { library.join(".musiclib").join("masters") } else { st.prefs.masters_dir.clone() };
        Dirs { library, masters }
    };
    let keeps = assigned.as_ref().filter(|a| a.profile.stores() && a.profile.format().ready());
    let Some(a) = keeps.filter(|_| running && !dirs.library.as_os_str().is_empty()).cloned() else {
        ctl.stop.store(true, Ordering::Relaxed);
        if st.plan_for.is_some() || !st.queue.is_empty() {
            drop_plan(st, ctl);
        }
        show(sh, st, ctl, tasks, false);
        return;
    };
    // A new profile or new pins: what the old plan still had to pull goes
    // at once.
    if st.plan_for.as_ref().is_some_and(|p| *p != a) {
        log::info!("replica: the profile or the pins changed; planned again");
        drop_plan(st, ctl);
    }
    let here = sh.replica.paused.lock().clone();
    let paused = paused_by(&here, &owner_presences(sh), me);
    ctl.stop.store(paused, Ordering::Relaxed);
    if paused {
        show(sh, st, ctl, tasks, true);
        return;
    }
    let due = st.planned_at.is_none_or(|t| t.elapsed() >= PLAN_AGAIN) || st.cut || st.plan_for.as_ref() != Some(&a);
    if st.queue.is_empty() && tasks.is_empty() && due && !peers.is_empty() {
        replan(sh, lib, &dirs, &a, &peers, st, ctl).await;
    }
    let era = ctl.era.load(Ordering::Relaxed);
    while tasks.len() < AT_ONCE && st.next_start.is_none_or(|t| Instant::now() >= t) && st.room_wait.is_none_or(|t| Instant::now() >= t) {
        let Some(job) = st.queue.pop_front() else { break };
        let from: Vec<String> = job.holders().into_iter().filter(|p| peers.contains(p)).collect();
        if from.is_empty() || st.waiting(&job.key()) {
            continue;
        }
        // The disks it writes to (the part's and the file's) keep their
        // reserve, the running pulls' bytes counted.
        let dest = match &job {
            Job::Track(_) => dirs.library.as_path(),
            Job::Master { .. } => dirs.masters.as_path(),
        };
        let need = match job.size() {
            0 => UNKNOWN_SIZE,
            n => n,
        };
        if !has_room(&[dirs.library.as_path(), dest], need, ctl.reserved.load(Ordering::Relaxed)) {
            log::info!("replica: no room kept on the disk (or its free space not known); the pulls wait");
            st.queue.push_front(job);
            st.room_wait = Some(Instant::now() + ROOM_WAIT);
            break;
        }
        let mut room = Reserve::new(ctl.clone(), need);
        let (sh, lib, me, dirs, ctl) = (sh.clone(), lib.clone(), me.to_owned(), dirs.clone(), ctl.clone());
        tasks.spawn(async move {
            let out = pull(&sh, &lib, &me, &dirs, &job, &from, era, &ctl, &mut room).await;
            drop(room);
            (job, era, out)
        });
        st.next_start = Some(Instant::now() + GAP);
    }
    show(sh, st, ctl, tasks, false);
}

/// The transfer as this device shows it (and tells the others).
fn show(sh: &Shared, st: &mut Puller, ctl: &Ctl, tasks: &JoinSet<Done>, paused: bool) {
    let busy = !st.queue.is_empty() || !tasks.is_empty();
    let now = Instant::now();
    let received = ctl.received.load(Ordering::Relaxed);
    if let Some((bytes, at)) = st.last {
        let secs = now.saturating_duration_since(at).as_secs_f64();
        if secs > 0.2 {
            let rate = received.saturating_sub(bytes) as f64 / secs;
            st.speed = if st.speed == 0.0 { rate } else { st.speed * 0.7 + rate * 0.3 };
        }
    }
    st.last = Some((received, now));
    let t = busy.then(|| MeshTransfer {
        done: received.min(st.total),
        total: st.total.max(received),
        paused,
        speed: if paused || tasks.is_empty() { 0 } else { st.speed as u64 },
    });
    let changed = {
        let mut cur = sh.replica.transfer.lock();
        let changed = *cur != t;
        *cur = t;
        changed
    };
    if changed {
        sh.notify();
    }
}

// --------------------------------------------------------------- a pull

/// The folder of the parts, made if need be only inside the library
/// folder: its real path.
fn parts_dir(library: &Path) -> std::io::Result<PathBuf> {
    nikgob_import::fsutil::create_dir_inside(&library.join(".musiclib").join("replica"), library)
}

/// Opening never follows a link at the end of the path (Unix
/// `O_NOFOLLOW`, and no waiting on a pipe).
#[cfg(unix)]
fn no_follow(opts: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt as _;
    opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

/// Opening takes a link or another reparse point itself, not what it
/// points to (then refused: [`reparse`]).
#[cfg(windows)]
fn no_follow(opts: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt as _;
    opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(not(any(unix, windows)))]
fn no_follow(_: &mut OpenOptions) {}

/// Whether `m` is a reparse point's (a link, a mount point…).
#[cfg(windows)]
fn reparse(m: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn reparse(_: &Metadata) -> bool {
    false
}

/// Whether `m` (not followed) is a plain file's: no link, no reparse point.
fn plain_meta(m: &Metadata) -> bool {
    m.file_type().is_file() && !reparse(m)
}

/// Opens `path` with `opts`, never through a link at its end, and only
/// when it is a plain file.
fn plain_open(path: &Path, opts: &mut OpenOptions) -> std::io::Result<std::fs::File> {
    no_follow(opts);
    let f = opts.open(path)?;
    if !plain_meta(&f.metadata()?) {
        return Err(std::io::Error::other("не обычный файл"));
    }
    Ok(f)
}

/// Opens the part at `path` to write from `offset` on (what is after it
/// dropped), never through a link: one not there yet is made (only when
/// nothing has its name, `offset` 0); one there must be a plain file.
pub(crate) fn open_part(path: &Path, offset: u64) -> std::io::Result<std::fs::File> {
    let mut f = match plain_open(path, OpenOptions::new().write(true)) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && offset == 0 => plain_open(path, OpenOptions::new().write(true).create_new(true))?,
        Err(e) => return Err(e),
    };
    f.set_len(offset)?;
    f.seek(SeekFrom::Start(offset))?;
    Ok(f)
}

/// The BLAKE3 (hex) of the plain file at `path` (never through a link).
fn blake3_plain(path: &Path) -> std::io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(plain_open(path, OpenOptions::new().read(true))?)?;
    Ok(h.finalize().to_hex().to_string())
}

/// A part in the parts' folder: the file being pulled
/// (`{key}.{token}.part`) and its note (`.meta`: the file's hash and its
/// extension). The token is random: no one makes a file of that name
/// ahead of it.
#[derive(Clone, Debug)]
pub(crate) struct Part {
    pub(crate) file: PathBuf,
    pub(crate) note: PathBuf,
}

impl Part {
    /// A new part of `key` in `dir`.
    pub(crate) fn new(dir: &Path, key: &str) -> Result<Self, Failed> {
        let mut b = [0u8; 16];
        getrandom::fill(&mut b).map_err(|e| Failed::Bad(format!("no randomness: {e}")))?;
        Ok(Self::named(dir, key, &data_encoding::HEXLOWER.encode(&b)))
    }

    fn named(dir: &Path, key: &str, token: &str) -> Self {
        Self { file: dir.join(format!("{key}.{token}.part")), note: dir.join(format!("{key}.{token}.meta")) }
    }

    /// Its file and its note go (a link of that name goes itself).
    fn remove(&self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_file(&self.note);
    }
}

/// Writes a part's note, a new file put over nothing: the file's hash and
/// its extension.
pub(crate) fn write_note(note: &Path, hash: &str, ext: &str) -> std::io::Result<()> {
    let mut f = plain_open(note, OpenOptions::new().write(true).create_new(true))?;
    f.write_all(format!("{hash}\n{ext}").as_bytes())?;
    f.sync_all()
}

/// A part's note: the file's hash and its extension (none: no note, or
/// not one).
fn read_note(note: &Path) -> Option<(String, String)> {
    let f = plain_open(note, OpenOptions::new().read(true)).ok()?;
    let mut s = String::new();
    f.take(NOTE_MAX).read_to_string(&mut s).ok()?;
    let (h, e) = s.split_once('\n')?;
    Some((clean_hash(h)?, clean_ext(e)))
}

/// The part of `key` an earlier try left in `dir`, found by listing it
/// (only a plain file counts; a link of such a name is never opened): the
/// one whose note says it is of the file `hash`, with its extension and
/// its length. The other parts of `key` go, and the notes whose part
/// never came.
pub(crate) fn find_part(dir: &Path, key: &str, hash: &str) -> Option<(Part, String, u64)> {
    let prefix = format!("{key}.");
    let mut found: Option<(Part, String, u64)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&prefix) else { continue };
        let (token, is_part) = match (rest.strip_suffix(".part"), rest.strip_suffix(".meta")) {
            (Some(t), _) => (t, true),
            (None, Some(t)) => (t, false),
            (None, None) => continue,
        };
        if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let part = Part::named(dir, key, token);
        if !is_part {
            if std::fs::symlink_metadata(&part.file).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
                let _ = std::fs::remove_file(&part.note);
            }
            continue;
        }
        let Some(len) = std::fs::symlink_metadata(&part.file).ok().filter(plain_meta).map(|m| m.len()) else { continue };
        match read_note(&part.note) {
            Some((h, ext)) if found.is_none() && h == hash => found = Some((part, ext, len)),
            _ => part.remove(),
        }
    }
    found
}

/// The part is the file `hash`; one that is not is deleted with its note.
pub(crate) fn verify_part(part: &Path, meta: &Path, hash: &str) -> Result<(), String> {
    let sum = blake3_plain(part).map_err(|e| format!("{}: {e}", part.display()))?;
    if sum != hash {
        let _ = std::fs::remove_file(part);
        let _ = std::fs::remove_file(meta);
        return Err("файл пришёл повреждённым".to_owned());
    }
    Ok(())
}

/// Writes `resp`'s body to `part` (after its first `offset` bytes, or over
/// it when `offset` is 0; never through a link: [`open_part`]), paced, at
/// most `limit` bytes in all; stops between two pieces when told or when
/// its plan (`era`) is no longer the one ([`Failed::Stopped`]), or when a
/// write would take the part's disk past its reserve, the other pulls'
/// bytes kept (`room`; [`Failed::NoRoom`]); the part stays. How long the
/// part is then.
pub(crate) async fn receive_paced(resp: http::Response<Body>, part: &Path, offset: u64, limit: u64, era: u64, ctl: &Ctl, room: &mut Reserve) -> Result<u64, Failed> {
    let io = |e: std::io::Error| Failed::Bad(format!("{}: {e}", part.display()));
    let disk = part.parent().unwrap_or(part).to_owned();
    let mut file = {
        let p = part.to_owned();
        let f = tokio::task::spawn_blocking(move || open_part(&p, offset)).await.map_err(|e| Failed::Bad(e.to_string()))?.map_err(io)?;
        tokio::fs::File::from_std(f)
    };
    let mut len = offset;
    // The disk as last looked at: its free bytes less what the other
    // pulls keep (None: not known), and what this one wrote since.
    let mut seen: Option<Option<u64>> = None;
    let mut since_look = 0u64;
    let mut body = resp.into_body().into_data_stream();
    loop {
        if ctl.halted(era) {
            file.flush().await.map_err(io)?;
            return Err(Failed::Stopped);
        }
        match tokio::time::timeout(STALL, body.next()).await {
            Ok(Some(Ok(bytes))) => {
                let n = bytes.len() as u64;
                len += n;
                if len > limit {
                    file.flush().await.map_err(io)?;
                    return Err(Failed::Bad("файл больше, чем ожидалось".to_owned()));
                }
                if seen.is_none() || since_look >= ROOM_STEP {
                    let others = room.others();
                    seen = Some(nikgob_runtime::free_bytes(&disk).map(|free| free.saturating_sub(others)));
                    since_look = 0;
                }
                // Each write weighed: what the disk would keep after it.
                let keeps = seen.flatten().is_some_and(|free| free.saturating_sub(since_look).saturating_sub(n) >= ctl.keep_free);
                if !keeps {
                    file.flush().await.map_err(io)?;
                    return Err(Failed::NoRoom);
                }
                file.write_all(&bytes).await.map_err(io)?;
                since_look += n;
                room.wrote(n);
                ctl.received.fetch_add(n, Ordering::Relaxed);
                ctl.pace.take(bytes.len()).await;
            }
            Ok(Some(Err(e))) => {
                file.flush().await.map_err(io)?;
                return Err(Failed::Bad(format!("передача оборвалась: {e}")));
            }
            Ok(None) => break,
            Err(_) => {
                file.flush().await.map_err(io)?;
                return Err(Failed::Bad("передача остановилась".to_owned()));
            }
        }
    }
    file.flush().await.map_err(io)?;
    file.sync_all().await.map_err(io)?;
    Ok(len)
}

fn header<'a>(r: &'a http::Response<Body>, name: &str) -> Option<&'a str> {
    r.headers().get(name).and_then(|v| v.to_str().ok())
}

/// Pulls `job` (exactly its file, by its hash) from the devices `from`
/// (the first that has it), keeps it and makes it this device's own. Its
/// bytes.
#[allow(clippy::too_many_arguments)]
async fn pull(sh: &Arc<Shared>, lib: &Library, me: &str, dirs: &Dirs, job: &Job, from: &[String], era: u64, ctl: &Ctl, room: &mut Reserve) -> Result<u64, Failed> {
    let library = dirs.library.clone();
    let dir = tokio::task::spawn_blocking(move || parts_dir(&library))
        .await
        .map_err(|e| Failed::Bad(e.to_string()))?
        .map_err(|e| Failed::Bad(format!("папка частей: {e}")))?;
    let key = job.key();
    let want = job.hash().to_owned();
    // A part of an earlier try goes on where it stopped when its note says
    // it is of this very file; the others of this job go.
    let found = {
        let (d, k, w) = (dir.clone(), key.clone(), want.clone());
        tokio::task::spawn_blocking(move || find_part(&d, &k, &w)).await.map_err(|e| Failed::Bad(e.to_string()))?
    };
    let (mut part, mut ext, mut offset) = match found {
        Some((p, e, len)) => (p, Some(e), len),
        None => (Part::new(&dir, &key)?, None, 0),
    };
    // A whole part left (kept, then not made the device's own): kept now.
    if let Some(e) = ext.clone()
        && offset > 0
        && offset == job.size()
    {
        let (p, m, h) = (part.file.clone(), part.note.clone(), want.clone());
        let whole = tokio::task::spawn_blocking(move || verify_part(&p, &m, &h)).await.map_err(|e| Failed::Bad(e.to_string()))?;
        if whole.is_ok() {
            ctl.received.fetch_add(offset, Ordering::Relaxed);
            let r = Room { keep: ctl.keep_free, others: room.others() };
            return keep(lib, me, dirs, job, &part, &want, &e, offset, r).await.map(|()| offset);
        }
        (part, ext, offset) = (Part::new(&dir, &key)?, None, 0);
    }
    // Exactly the planned file: its size when it is known.
    let limit = if job.size() > 0 { job.size() } else { FILE_MAX };
    let mut not_there = 0usize;
    'devices: for peer in from {
        for _ in 0..3 {
            let uri = match job {
                Job::Track(w) => format!("/v1/media/tracks/{}/audio?profile={}&hash={}", w.track_id, nikgob_view::MeshFormat::Orig.key(), w.file_hash),
                Job::Master { item, .. } => format!("/v1/replica/masters/{}", item.hash),
            };
            let mut b = http::Request::get(uri);
            if offset > 0 {
                b = b.header(http::header::RANGE, format!("bytes={offset}-"));
            }
            let req = b.body(Body::empty()).map_err(|e| Failed::Bad(e.to_string()))?;
            let resp = match tokio::time::timeout(HEAD_WAIT, sh.request(peer, req)).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    log::debug!("replica: {key} from {peer}: {e}");
                    continue 'devices;
                }
                Err(_) => continue 'devices,
            };
            let status = resp.status();
            if status == http::StatusCode::NOT_FOUND {
                not_there += 1;
                continue 'devices;
            }
            // The part is no part of the file there, or the answer is not
            // the part asked for: a new part, from the start.
            let wrong_part = status == http::StatusCode::PARTIAL_CONTENT && !range_starts_at(header(&resp, "content-range"), offset);
            if status == http::StatusCode::RANGE_NOT_SATISFIABLE || wrong_part {
                drop(resp);
                part.remove();
                (part, ext, offset) = (Part::new(&dir, &key)?, None, 0);
                continue;
            }
            if !status.is_success() {
                return Err(Failed::Bad(format!("ответ {status}")));
            }
            let Some(hash) = header(&resp, nikgob_api::HASH_HEADER).and_then(clean_hash) else {
                return Err(Failed::Bad("то устройство не сказало хэш файла".to_owned()));
            };
            if hash != want {
                return Err(Failed::Bad("не тот файл".to_owned()));
            }
            let e = clean_ext(header(&resp, nikgob_api::EXT_HEADER).unwrap_or(""));
            let from_byte = if status == http::StatusCode::PARTIAL_CONTENT { offset } else { 0 };
            if ext.is_none() {
                let (n, h, x) = (part.note.clone(), hash.clone(), e.clone());
                tokio::task::spawn_blocking(move || write_note(&n, &h, &x))
                    .await
                    .map_err(|e| Failed::Bad(e.to_string()))?
                    .map_err(|err| Failed::Bad(format!("{}: {err}", part.note.display())))?;
                ext = Some(e.clone());
            }
            ctl.received.fetch_add(from_byte, Ordering::Relaxed);
            let len = receive_paced(resp, &part.file, from_byte, limit, era, ctl, room).await?;
            let (p, m, h) = (part.file.clone(), part.note.clone(), hash.clone());
            tokio::task::spawn_blocking(move || verify_part(&p, &m, &h)).await.map_err(|e| Failed::Bad(e.to_string()))?.map_err(Failed::Bad)?;
            let e = ext.clone().unwrap_or(e);
            let r = Room { keep: ctl.keep_free, others: room.others() };
            keep(lib, me, dirs, job, &part, &hash, &e, len, r).await?;
            return Ok(len);
        }
    }
    if not_there > 0 { Err(Failed::NotThere) } else { Err(Failed::Bad("ни одно устройство не отдало файл".to_owned())) }
}

/// The checked part put in its place and made this device's own (on the
/// blocking pool), only when the disk it goes to keeps `room` after it.
#[allow(clippy::too_many_arguments)]
async fn keep(lib: &Library, me: &str, dirs: &Dirs, job: &Job, part: &Part, hash: &str, ext: &str, size: u64, room: Room) -> Result<(), Failed> {
    let (lib, me, dirs, job, part, hash, ext) = (lib.clone(), me.to_owned(), dirs.clone(), job.clone(), part.clone(), hash.to_owned(), ext.to_owned());
    tokio::task::spawn_blocking(move || -> Result<(), Failed> {
        let placed = match &job {
            Job::Track(w) => place_song(&lib, &dirs.library, &me, w, &part.file, &hash, &ext, size, room),
            Job::Master { item, .. } => place_master(&lib, &dirs.masters, &me, item, &part.file, &ext, room),
        }?;
        let _ = std::fs::remove_file(&part.note);
        log::info!("replica: {} kept", placed.display());
        Ok(())
    })
    .await
    .map_err(|e| Failed::Bad(e.to_string()))?
}

/// Whether `path` holds exactly the file of `size` bytes and `hash`.
fn same_file(path: &Path, size: u64, hash: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == size) && blake3_plain(path).is_ok_and(|h| h == hash)
}


/// Puts the checked `part` at `target`, or at `target (2)`… when the name
/// is another file's — never over anything, its folders made only inside
/// `root`, only when the disk it goes to keeps `room` after its bytes,
/// whatever disk that is (a move on one disk is weighed as a copy too;
/// [`Failed::NoRoom`]: the part stays), a copy across disks checked before
/// the part goes;
/// the very file there already (a plain file inside `root`) is taken as it
/// is (the part goes). Where it is, and whether the part was moved there
/// (false: the file was there already).
pub(crate) fn put_in_place(part: &Path, target: &Path, hash: &str, size: u64, root: &Path, room: Room) -> Result<(PathBuf, bool), Failed> {
    let dir = target.parent().ok_or_else(|| Failed::Bad(format!("{} has no folder", target.display())))?;
    nikgob_import::fsutil::create_dir_inside(dir, root).map_err(|e| Failed::Bad(e.to_string()))?;
    if nikgob_import::fsutil::plain_file_inside(target, root) && same_file(target, size, hash) {
        let _ = std::fs::remove_file(part);
        return Ok((target.to_owned(), false));
    }
    if !room.fits(dir, size) {
        return Err(Failed::NoRoom);
    }
    let placed = nikgob_import::fsutil::move_unique_verified(part, target, hash).map_err(|e| Failed::Bad(e.to_string()))?;
    if !nikgob_import::fsutil::plain_file_inside(&placed, root) {
        let _ = nikgob_import::fsutil::move_new_verified(&placed, part, hash);
        return Err(Failed::Bad(format!("{} is outside {}", placed.display(), root.display())));
    }
    Ok((placed, true))
}

/// A flat master, `{hash}.<ext>` right in `root`: only under that very
/// name (a master kept flat is found by it), never over another file, only
/// when the disk it goes to keeps `room` after its bytes.
fn put_flat(part: &Path, root: &Path, hash: &str, ext: &str, size: u64, room: Room) -> Result<(PathBuf, bool), Failed> {
    nikgob_import::fsutil::create_dir_inside(root, root).map_err(|e| Failed::Bad(e.to_string()))?;
    let target = root.join(format!("{hash}.{ext}"));
    if nikgob_import::fsutil::plain_file_inside(&target, root) && same_file(&target, size, hash) {
        let _ = std::fs::remove_file(part);
        return Ok((target, false));
    }
    if !room.fits(root, size) {
        return Err(Failed::NoRoom);
    }
    nikgob_import::fsutil::move_new_verified(part, &target, hash).map_err(|e| Failed::Bad(format!("{}: {e}", target.display())))?;
    Ok((target, true))
}

/// A pulled song's file laid out in `library` as the library names songs
/// (`Artist/YYYY - Album/NN - Title.ext`) and registered as this device's
/// (`me`) own source; its format from the device that had it; what it
/// hashes to noted for the plans. When it cannot be registered it goes
/// back to its part (kept for the next try).
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_song(lib: &Library, library: &Path, me: &str, want: &Want, part: &Path, hash: &str, ext: &str, size: u64, room: Room) -> Result<PathBuf, Failed> {
    let layout = nikgob_import::master_files::layout_of_track(lib, want.track_id).ok_or_else(|| Failed::Bad("этой песни ещё нет в библиотеке".to_owned()))?;
    let target = layout.target_path(library, ext);
    let (placed, moved) = put_in_place(part, &target, hash, size, library, room)?;
    let of = lib.offer_of_hash(hash).ok().flatten().or_else(|| want.offers.first().cloned());
    let codec = of.as_ref().map_or_else(|| Codec::parse(ext), |o| Codec::parse(&o.codec));
    let mut s = Source::new(want.track_id, SourceKind::LocalFile, placed.to_string_lossy().into_owned(), codec);
    if let Some(o) = &of {
        s.bitrate = o.bitrate;
        s.sample_rate = o.sample_rate;
        s.bit_depth = o.bit_depth;
        s.quality_score = o.quality_score;
    }
    s.file_hash = Some(hash.to_owned());
    s.size_bytes = Some(size);
    s.added_at = nikgob_library::now_ms();
    if let Err(e) = lib.put_pulled_file(&s, me) {
        // Only what was moved goes back (a file found there stays).
        if moved {
            let _ = nikgob_import::fsutil::move_new_verified(&placed, part, hash);
        }
        return Err(Failed::Bad(e.to_string()));
    }
    stamp(lib, &placed, hash);
    Ok(placed)
}

/// A pulled master laid out in `masters` as the library keeps masters (by
/// name for one track, flat for a disc image or a track not named yet) and
/// kept as this device's (`me`); what it hashes to noted for the plans.
fn place_master(lib: &Library, masters: &Path, me: &str, item: &MasterItem, part: &Path, ext: &str, room: Room) -> Result<PathBuf, Failed> {
    let layout = match item.tracks.as_slice() {
        [one] => nikgob_import::master_files::layout_of_track(lib, *one),
        _ => None,
    };
    let named = layout.and_then(|l| nikgob_import::master_files::master_target(masters, &l, None, ext));
    let ((placed, moved), by_name) = match named {
        Some(target) => (put_in_place(part, &target, &item.hash, item.size, masters, room)?, true),
        None => (put_flat(part, masters, &item.hash, ext, item.size, room)?, false),
    };
    let path = by_name.then(|| placed.to_string_lossy().into_owned());
    if let Err(e) = lib.put_pulled_masters(item, path.as_deref(), me) {
        if moved {
            let _ = nikgob_import::fsutil::move_new_verified(&placed, part, &item.hash);
        }
        return Err(Failed::Bad(e.to_string()));
    }
    stamp(lib, &placed, &item.hash);
    Ok(placed)
}

// ------------------------------------- the copies of this computer's masters

/// Asks each owner device online, every [`AUDIT_EVERY`], which of this
/// computer's masters it holds, and keeps the `replica` rows as it says;
/// a change of who holds what wakes the worker's look at the masters that
/// may go. Only on a device with a window (where the owner decides).
pub(crate) async fn audit(sh: Arc<Shared>) {
    let Some(lib) = sh.library.clone() else { return };
    if sh.headless {
        return;
    }
    let mut next: HashMap<String, Instant> = HashMap::new();
    loop {
        tokio::time::sleep(AUDIT_TICK).await;
        if sh.node.lock().is_none() {
            continue;
        }
        let peers = owner_peers(&sh);
        next.retain(|p, _| peers.contains(p));
        let mut changed = false;
        for peer in peers {
            if next.get(&peer).is_some_and(|t| Instant::now() < *t) {
                continue;
            }
            match audit_peer(&sh, &lib, &peer).await {
                Ok(c) => {
                    changed |= c;
                    next.insert(peer, Instant::now() + AUDIT_EVERY);
                }
                Err(e) => {
                    log::debug!("replica: the copies on {peer} not asked: {e}");
                    next.insert(peer, Instant::now() + AUDIT_RETRY);
                }
            }
        }
        if changed {
            sh.rt.send(Command::ReplicaCopiesChanged);
        }
    }
}

/// Asks `peer` about every master here, a page at a time. Whether who
/// holds what changed.
async fn audit_peer(sh: &Arc<Shared>, lib: &Library, peer: &str) -> Result<bool, String> {
    let mut after = String::new();
    let mut changed = false;
    loop {
        let hashes = {
            let (l, a) = (lib.clone(), after.clone());
            blocking(move || l.master_hashes_after(&a, PAGE)).await?
        };
        let Some(last) = hashes.last().cloned() else { break };
        let ask = CheckAsk { hashes, rehash: false };
        let a: CheckAnswer = post_json(sh, peer, "/v1/replica/check", &ask, ASK_WAIT).await?;
        // A file not read this time is not one gone: its row stays.
        let asked: Vec<String> = ask.hashes.into_iter().filter(|h| !a.deferred.contains(h)).collect();
        let (l, p) = (lib.clone(), peer.to_owned());
        changed |= blocking(move || l.set_node_replicas(&p, &asked, &a.held)).await?;
        after = last;
        tokio::time::sleep(PACE).await;
    }
    Ok(changed)
}

/// What the devices' answers confirm: for each hash, how many devices hold
/// it (each device once).
pub(crate) fn confirmed(answers: &[(String, Vec<Held>)], asked: &[String]) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    for (_, held) in answers {
        let mine: HashSet<&str> = held.iter().map(|h| h.hash.as_str()).filter(|h| asked.iter().any(|a| a == h)).collect();
        for h in mine {
            *out.entry(h.to_owned()).or_default() += 1;
        }
    }
    out
}

/// The second look right before masters go: each trusted owner device
/// online (none of `leaving`) reads its copies of `hashes` again now; the
/// masters `need` devices still hold go back to the worker
/// ([`Command::ReplicaRechecked`]), the `replica` rows follow the answers.
pub(crate) async fn recheck(sh: Arc<Shared>, card: u64, hashes: Vec<String>, need: u32, leaving: Vec<String>) {
    let peers: Vec<String> = owner_peers(&sh).into_iter().filter(|p| !leaving.contains(p)).collect();
    let mut answers: Vec<(String, Vec<Held>)> = Vec::new();
    let mut failure: Option<String> = None;
    for peer in &peers {
        let mut held = Vec::new();
        let mut ok = true;
        // A batch first; a file it did not read (the budget ran out) is
        // asked again alone (a file larger than the budget is read so).
        let mut asks: VecDeque<Vec<String>> = hashes.chunks(RECHECK_BATCH).map(<[String]>::to_vec).collect();
        let mut alone: HashSet<String> = HashSet::new();
        while let Some(chunk) = asks.pop_front() {
            let ask = CheckAsk { hashes: chunk.clone(), rehash: true };
            match post_json::<CheckAnswer>(&sh, peer, "/v1/replica/check", &ask, RECHECK_WAIT).await {
                Ok(a) => {
                    let read: Vec<String> = chunk.iter().filter(|h| !a.deferred.contains(h)).cloned().collect();
                    if let Some(lib) = sh.library.clone() {
                        let (p, c, h) = (peer.clone(), read, a.held.clone());
                        if let Err(e) = blocking(move || lib.set_node_replicas(&p, &c, &h)).await {
                            log::warn!("replica: the copies on {peer} not noted: {e}");
                        }
                    }
                    for d in a.deferred.iter().filter(|d| chunk.contains(d)) {
                        if alone.insert(d.clone()) {
                            asks.push_back(vec![d.clone()]);
                        }
                    }
                    held.extend(a.held);
                }
                Err(e) => {
                    log::warn!("replica: {peer} did not read its copies again: {e}");
                    if failure.is_none() {
                        failure = Some(e);
                    }
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            answers.push((peer.clone(), held));
        }
    }
    let counts = confirmed(&answers, &hashes);
    let ok: Vec<String> = hashes.iter().filter(|h| counts.get(h.as_str()).copied().unwrap_or(0) >= need as usize).cloned().collect();
    let failed = if !ok.is_empty() {
        None
    } else if peers.is_empty() {
        Some("ни одного устройства в сети".to_owned())
    } else {
        failure
    };
    sh.rt.send(Command::ReplicaRechecked { card, ok, failed });
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Master, Release, Track};

    use super::*;
    use crate::presence::Presence;

    #[test]
    fn a_failed_file_waits_longer_each_time() {
        assert_eq!(retry_after(1), RETRY_MIN);
        assert_eq!(retry_after(2), RETRY_MIN * 2);
        assert_eq!(retry_after(30), RETRY_MAX);
    }

    /// The profile is the ring holder's word (another device's is not);
    /// the pause is this device's own or one a device asks.
    #[test]
    fn the_holder_says_what_is_kept_and_any_owner_may_pause() {
        let a = Assigned { node: "pi".into(), profile: MeshProfile::Full, ..Assigned::default() };
        let other = Presence { assign: vec![Assigned { profile: MeshProfile::SoundOnly, ..a.clone() }], ..Presence::default() };
        let holder = Presence { holder: true, assign: vec![a.clone()], ..Presence::default() };
        assert_eq!(assigned_by(&[other.clone(), holder.clone()], "pi"), Some(a));
        assert_eq!(assigned_by(&[other.clone()], "pi"), None, "only the holder's word counts");
        assert_eq!(assigned_by(&[holder.clone()], "tv"), None);
        let none = HashSet::new();
        assert!(!paused_by(&none, &[holder.clone()], "pi"));
        assert!(paused_by(&["pi".to_owned()].into(), &[], "pi"));
        let asks = Presence { pause: vec!["pi".into()], ..Presence::default() };
        assert!(paused_by(&none, &[asks], "pi"));
    }

    #[test]
    fn masters_are_taken_from_any_device_listing_them_once() {
        let item = |h: &str| MasterItem { hash: h.into(), codec: "flac".into(), size: 10, tracks: Vec::new() };
        let now = Instant::now();
        let lists: HashMap<String, (Instant, Vec<MasterItem>)> =
            [("pc".to_owned(), (now, vec![item("a"), item("b")])), ("lap".to_owned(), (now, vec![item("b"), item("c")]))].into();
        let held: HashSet<String> = ["c".to_owned()].into();
        let plan = plan_masters(&lists, &held);
        assert_eq!(plan.iter().map(|(m, _)| m.hash.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(plan[1].1, ["lap", "pc"], "both devices have it");
    }

    #[test]
    fn a_device_counts_once_for_a_copy() {
        let h = |s: &str| Held { hash: s.into(), verified_ms: 1 };
        let answers = vec![("pi".to_owned(), vec![h("a"), h("a"), h("x")]), ("lap".to_owned(), vec![h("a"), h("b")])];
        let c = confirmed(&answers, &["a".to_owned(), "b".to_owned()]);
        assert_eq!(c.get("a"), Some(&2));
        assert_eq!(c.get("b"), Some(&1));
        assert_eq!(c.get("x"), None, "not asked");
    }

    fn chunks_body(chunks: Vec<Vec<u8>>, stop_at: usize, ctl: Arc<Ctl>) -> http::Response<Body> {
        let stream = futures_util::stream::iter(chunks.into_iter().enumerate().map(move |(i, c)| {
            if i == stop_at {
                ctl.stop.store(true, Ordering::Relaxed);
            }
            Ok::<_, std::io::Error>(axum::body::Bytes::from(c))
        }));
        http::Response::new(Body::from_stream(stream))
    }

    /// A pause stops a file between two pieces, its part kept; the pull
    /// goes on from there and the whole file checks; a damaged one is
    /// dropped with its note.
    #[tokio::test]
    async fn a_paused_file_goes_on_where_it_stopped_and_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..=255u8).cycle().take(30_000).collect();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let part = dir.path().join("t.part");
        let meta = dir.path().join("t.meta");
        std::fs::write(&meta, &hash).unwrap();
        let ctl = Arc::new(Ctl { keep_free: 0, ..Ctl::new(RATE) });
        let mut room = Reserve::new(ctl.clone(), 30_000);
        let chunks: Vec<Vec<u8>> = bytes.chunks(10_000).map(<[u8]>::to_vec).collect();
        let first = receive_paced(chunks_body(chunks.clone(), 1, ctl.clone()), &part, 0, 30_000, 0, &ctl, &mut room).await;
        assert_eq!(first, Err(Failed::Stopped));
        let kept = std::fs::metadata(&part).unwrap().len();
        assert_eq!(kept, 20_000, "the pieces before the pause stay");
        assert_eq!(ctl.reserved.load(Ordering::Relaxed), 10_000, "what is written is no longer kept for it");
        ctl.stop.store(false, Ordering::Relaxed);
        let rest = http::Response::new(Body::from(bytes[kept as usize..].to_vec()));
        assert_eq!(receive_paced(rest, &part, kept, 30_000, 0, &ctl, &mut room).await, Ok(30_000));
        assert_eq!(ctl.received.load(Ordering::Relaxed), 30_000);
        assert!(verify_part(&part, &meta, &hash).is_ok());
        drop(room);
        assert_eq!(ctl.reserved.load(Ordering::Relaxed), 0);
        let mut room = Reserve::new(ctl.clone(), 0);

        std::fs::write(&part, &bytes[..100]).unwrap();
        assert_eq!(verify_part(&part, &meta, &hash).unwrap_err(), "файл пришёл повреждённым");
        assert!(!part.exists() && !meta.exists());

        // A new profile or new pins: a pull of the old plan stops.
        ctl.era.store(1, Ordering::Relaxed);
        let old = http::Response::new(Body::from(bytes.clone()));
        assert_eq!(receive_paced(old, &dir.path().join("o.part"), 0, 30_000, 0, &ctl, &mut room).await, Err(Failed::Stopped));
        // Never more than the planned file's bytes.
        let big = http::Response::new(Body::from(bytes.clone()));
        assert!(matches!(receive_paced(big, &dir.path().join("b.part"), 0, 100, 1, &ctl, &mut room).await, Err(Failed::Bad(_))));
        // No write takes the disk past its reserve.
        let full = Arc::new(Ctl { keep_free: u64::MAX, ..Ctl::new(RATE) });
        let mut none = Reserve::new(full.clone(), 0);
        let f = dir.path().join("f.part");
        let r = receive_paced(http::Response::new(Body::from(bytes.clone())), &f, 0, 30_000, 0, &full, &mut none).await;
        assert_eq!(r, Err(Failed::NoRoom));
        assert_eq!(std::fs::metadata(&f).unwrap().len(), 0, "not a byte past the reserve");
    }

    /// A pull keeps for itself what it may still write; the others' share
    /// is what the rest keep; what it did not write goes back at its end.
    #[test]
    fn a_pull_keeps_its_bytes_while_it_runs() {
        let c = Arc::new(Ctl::new(RATE));
        let mut a = Reserve::new(c.clone(), 100);
        let b = Reserve::new(c.clone(), 50);
        assert_eq!((a.others(), b.others()), (50, 100));
        a.wrote(30);
        assert_eq!(c.reserved.load(Ordering::Relaxed), 120);
        a.wrote(500);
        assert_eq!(c.reserved.load(Ordering::Relaxed), 50, "a file larger than kept takes nothing of the others'");
        drop(b);
        drop(a);
        assert_eq!(c.reserved.load(Ordering::Relaxed), 0);
    }

    /// A part is found by listing its folder, only a plain file whose note
    /// is of the very file; a new one is never made over anything, an old
    /// one of another file goes, and it is written from where it stopped.
    #[test]
    fn parts_are_found_by_listing_and_never_made_over_anything() {
        let dir = tempfile::tempdir().unwrap();
        let (h1, h2) = ("1".repeat(64), "2".repeat(64));
        let p = Part::new(dir.path(), "t-a").unwrap();
        assert!(p.file.file_name().unwrap().to_string_lossy().len() > "t-a..part".len() + 30, "a random name");
        write_note(&p.note, &h1, "flac").unwrap();
        assert!(write_note(&p.note, &h1, "flac").is_err(), "a note is never written over");
        {
            let mut f = open_part(&p.file, 0).unwrap();
            std::io::Write::write_all(&mut f, b"12345").unwrap();
        }
        assert!(open_part(&Part::new(dir.path(), "t-a").unwrap().file, 3).is_err(), "only a new part starts at 0");
        let (found, ext, len) = find_part(dir.path(), "t-a", &h1).unwrap();
        assert_eq!((found.file.clone(), ext.as_str(), len), (p.file.clone(), "flac", 5));
        {
            let mut f = open_part(&p.file, 3).unwrap();
            std::io::Write::write_all(&mut f, b"xy").unwrap();
        }
        assert_eq!(std::fs::read(&p.file).unwrap(), b"123xy", "from where it stopped, what was after it dropped");
        assert!(find_part(dir.path(), "t-b", &h1).is_none(), "another job's");
        assert!(find_part(dir.path(), "t-a", &h2).is_none(), "of another file");
        assert!(!p.file.exists() && !p.note.exists(), "it went");
        assert!(read_note(&dir.path().join("t-a.0.meta")).is_none());
        let orphan = Part::new(dir.path(), "t-a").unwrap();
        write_note(&orphan.note, &h1, "flac").unwrap();
        assert!(find_part(dir.path(), "t-a", &h1).is_none());
        assert!(!orphan.note.exists(), "a note whose part never came goes");
    }

    /// A part's name taken by a link is never written or read through.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_part_never_goes_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.flac");
        std::fs::write(&song, b"music").unwrap();
        let p = Part::named(dir.path(), "t-a", &"0".repeat(32));
        std::os::unix::fs::symlink(&song, &p.file).unwrap();
        std::os::unix::fs::symlink(&song, &p.note).unwrap();
        let ctl = Arc::new(Ctl { keep_free: 0, ..Ctl::new(RATE) });
        let mut room = Reserve::new(ctl.clone(), 0);
        let r = receive_paced(http::Response::new(Body::from(vec![1u8; 10])), &p.file, 0, 100, 0, &ctl, &mut room).await;
        assert!(matches!(r, Err(Failed::Bad(_))));
        assert!(open_part(&p.file, 0).is_err() && blake3_plain(&p.file).is_err() && read_note(&p.note).is_none());
        assert!(write_note(&p.note, &"1".repeat(64), "flac").is_err());
        assert!(find_part(dir.path(), "t-a", &"1".repeat(64)).is_none());
        assert_eq!(std::fs::read(&song).unwrap(), b"music", "the file it points to is untouched");
    }

    /// A disk whose free space is not known gets nothing; the reserve and
    /// the running pulls' bytes are kept.
    #[test]
    fn a_disk_keeps_its_reserve() {
        assert!(!has_room(&[Path::new("")], 0, 0), "free space not known");
        let dir = tempfile::tempdir().unwrap();
        assert!(!has_room(&[dir.path()], u64::MAX / 2, 0), "never past the reserve");
        assert!(!has_room(&[dir.path()], 0, u64::MAX / 2), "the running pulls' bytes count");
    }

    /// The pauses asked here are kept and come back after a restart.
    #[test]
    fn pauses_asked_here_outlive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(PAUSED);
        let r = ReplicaShared::load(file.clone());
        r.set_paused("pi", true);
        r.set_paused("tv", true);
        r.set_paused("tv", false);
        assert_eq!(ReplicaShared::load(file).pause_list(), ["pi"]);
    }

    /// The pulls together keep to the rate.
    #[tokio::test(start_paused = true)]
    async fn pulls_are_paced() {
        let pace = Pace::new(1_000_000.0);
        let start = Instant::now();
        for _ in 0..10 {
            pace.take(500_000).await;
        }
        assert!(start.elapsed() >= Duration::from_secs(5) - BURST, "five megabytes at a megabyte a second");
    }

    fn song(lib: &Library) -> Track {
        let artist = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let credit = ArtistCredit::single(artist.name.clone(), Some(artist.id));
        let mut r = Release::new("Группа крови", credit.clone());
        r.year = Some(1988);
        let release = lib.upsert_release(&r).unwrap();
        let mut t = Track::new("Группа крови", credit);
        t.release_id = Some(release.id);
        t.track_no = 1;
        lib.upsert_track(&t).unwrap()
    }

    fn want(t: TrackId, hash: &str, size: u64) -> Want {
        Want {
            track_id: t,
            file_hash: hash.into(),
            size,
            offers: vec![Offer {
                track_id: t,
                node_id: "pc".into(),
                file_hash: hash.into(),
                size,
                codec: "opus".into(),
                quality_score: 70,
                bitrate: Some(160),
                sample_rate: Some(48_000),
                bit_depth: None,
            }],
        }
    }

    /// A pulled song lands where the library lays songs out and plays as
    /// this device's own; a name another file has gets a number (never
    /// over it); the very file there already is taken as it is.
    #[test]
    fn a_pulled_song_is_laid_out_registered_and_never_overwrites() {
        let lib = Library::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let library = root.path().join("Music");
        std::fs::create_dir_all(&library).unwrap();
        let t = song(&lib);
        let bytes = b"OggS opus".to_vec();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let target = library.join("Кино").join("1988 - Группа крови").join("01 - Группа крови.opus");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"another file").unwrap();
        let part = root.path().join("t.part");
        std::fs::write(&part, &bytes).unwrap();

        let placed = place_song(&lib, &library, "me", &want(t.id, &hash, bytes.len() as u64), &part, &hash, "opus", bytes.len() as u64, Room::default()).unwrap();
        assert_eq!(placed, library.join("Кино").join("1988 - Группа крови").join("01 - Группа крови (2).opus"));
        assert_eq!(std::fs::read(&target).unwrap(), b"another file", "never over another file");
        assert_eq!(std::fs::read(&placed).unwrap(), bytes);
        assert!(!part.exists());
        let s = lib.active_source(t.id).unwrap().unwrap();
        assert_eq!(s.locator, placed.to_string_lossy());
        assert_eq!((s.file_hash.as_deref(), s.quality_score, s.codec), (Some(hash.as_str()), 70, Codec::Opus));
        let dirs = Dirs { library: library.clone(), masters: library.join(".musiclib").join("masters") };
        // Its hash noted when it was put there: held without reading it.
        let (held, pending) = held_on_disk(&lib, &dirs, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert_eq!(held[&t.id].iter().map(|f| f.file_hash.as_deref()).collect::<Vec<_>>(), [Some(hash.as_str())]);
        assert!(pending.is_empty());
        assert!(lib.replica_verified(&hash, "me").unwrap().is_some());

        // A disk that would go below its reserve: the part stays.
        std::fs::write(&part, &bytes).unwrap();
        let tight = Room { keep: u64::MAX / 2, others: 0 };
        let elsewhere = library.join("Кино").join("1988 - Группа крови").join("02 - Другая.opus");
        assert_eq!(put_in_place(&part, &elsewhere, &hash, bytes.len() as u64, &library, tight), Err(Failed::NoRoom));
        assert!(part.exists() && !elsewhere.exists());

        // The same file at the name already: taken as it is.
        std::fs::write(&target, &bytes).unwrap();
        std::fs::write(&part, &bytes).unwrap();
        let again = put_in_place(&part, &target, &hash, bytes.len() as u64, &library, Room::default()).unwrap();
        assert_eq!(again, (target.clone(), false), "not moved: the file was there");
        assert!(!part.exists());
    }

    /// A master of one track goes by name into the masters folder, a disc
    /// image's flat under its hash; both kept as this device's.
    #[test]
    fn a_pulled_master_goes_into_the_masters_folder() {
        let lib = Library::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let masters = root.path().join("Music").join(".musiclib").join("masters");
        let t = song(&lib);
        let bytes = b"fLaC master".to_vec();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let part = root.path().join("m.part");
        std::fs::write(&part, &bytes).unwrap();
        let item = MasterItem { hash: hash.clone(), codec: "flac".into(), size: bytes.len() as u64, tracks: vec![t.id] };
        let placed = place_master(&lib, &masters, "me", &item, &part, "flac", Room::default()).unwrap();
        assert!(placed.starts_with(&masters) && placed.parent() != Some(masters.as_path()), "by name: {}", placed.display());
        let by_name = placed.to_string_lossy().into_owned();
        assert_eq!(lib.get_master(t.id).unwrap().unwrap().file_path.as_deref(), Some(by_name.as_str()));

        let image = b"fLaC image".to_vec();
        let ihash = blake3::hash(&image).to_hex().to_string();
        std::fs::write(&part, &image).unwrap();
        let disc = MasterItem { hash: ihash.clone(), codec: "flac".into(), size: image.len() as u64, tracks: vec![TrackId::new(), TrackId::new()] };
        let flat = place_master(&lib, &masters, "me", &disc, &part, "flac", Room::default()).unwrap();
        assert_eq!(flat, masters.join(format!("{ihash}.flac")));
        let (here, pending) = masters_on_disk(&lib, &masters, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert!(pending.is_empty(), "their hashes noted when they were put there");
        assert!(here.contains(&ihash), "its copy, though its tracks are not here yet");
        assert!(here.contains(&hash));
    }

    /// What this device holds is what is on its disk, the very file: a
    /// source or a master row whose file is gone, outside its folders or
    /// another file now is not held; a flat master file is; a file not
    /// read yet waits (its bytes charged) and is read in a later plan.
    #[test]
    fn only_the_very_files_on_the_disk_are_held() {
        let lib = Library::open_in_memory().unwrap();
        let root = tempfile::tempdir().unwrap();
        let dirs = Dirs { library: root.path().join("Music"), masters: root.path().join("Music").join(".musiclib").join("masters") };
        std::fs::create_dir_all(&dirs.masters).unwrap();
        let t = song(&lib);
        fn sum(b: &[u8]) -> String {
            blake3::hash(b).to_hex().to_string()
        }
        let here = dirs.library.join("a.opus");
        std::fs::write(&here, b"a").unwrap();
        let changed = dirs.library.join("b.opus");
        std::fs::write(&changed, b"now another file").unwrap();
        let outside = root.path().join("c.opus");
        std::fs::write(&outside, b"c").unwrap();
        let rows = [(here.clone(), sum(b"a")), (dirs.library.join("gone.opus"), sum(b"g")), (changed, sum(b"b")), (outside, sum(b"c"))];
        for (loc, h) in rows {
            let mut s = Source::new(t.id, SourceKind::LocalFile, loc.to_string_lossy().into_owned(), Codec::Opus);
            s.file_hash = Some(h);
            lib.add_source(&s).unwrap();
        }
        let (held, pending) = held_on_disk(&lib, &dirs, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert!(held.is_empty(), "nothing read yet");
        assert_eq!(pending.get(&t.id), Some(&(1 + 16)), "the files inside, their bytes on the disk");
        let (held, pending) = held_on_disk(&lib, &dirs, &mut Rehash::new()).unwrap();
        assert!(pending.is_empty());
        assert_eq!(held[&t.id].iter().map(|f| (f.file_hash.clone(), f.size)).collect::<Vec<_>>(), [(Some(sum(b"a")), 1)]);

        let flat = sum(b"x");
        std::fs::write(dirs.masters.join(format!("{flat}.flac")), b"x").unwrap();
        let wrong = sum(b"y");
        std::fs::write(dirs.masters.join(format!("{wrong}.flac")), b"not y").unwrap();
        let gone = dirs.masters.join("Кино").join("01.flac");
        lib.put_master(&Master {
            track_id: t.id,
            codec: Codec::Flac,
            file_hash: "e".repeat(64),
            size_bytes: 1,
            origin: None,
            file_path: Some(gone.to_string_lossy().into_owned()),
            replicas: Vec::new(),
        })
        .unwrap();
        let (m, waiting) = masters_on_disk(&lib, &dirs.masters, &mut Rehash { files: 0, bytes: 0 }).unwrap();
        assert!(m.is_empty() && waiting.contains(&flat) && waiting.contains(&wrong));
        let (m, waiting) = masters_on_disk(&lib, &dirs.masters, &mut Rehash::new()).unwrap();
        assert!(waiting.is_empty());
        assert!(m.contains(&flat));
        assert!(!m.contains(&wrong), "a file that is not its master");
        assert!(!m.contains(&"e".repeat(64)), "its file is gone: pulled again");
    }

    /// A plan reads again at most so many files, the first whatever its
    /// size.
    #[test]
    fn a_plan_reads_a_bounded_number_of_files_again() {
        let mut r = Rehash::new();
        assert!(r.take(REHASH_BYTES * 2), "the first one whatever its size");
        assert!(!r.take(1), "then its bytes ran out");
        let mut r = Rehash::new();
        for _ in 0..REHASH_FILES {
            assert!(r.take(1));
        }
        assert!(!r.take(1));
    }
}
