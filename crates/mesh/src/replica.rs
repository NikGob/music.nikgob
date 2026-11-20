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
//! The sending device writes nothing. What the plan gives the device and
//! what of it it holds (bytes) goes out in its presence
//! ([`crate::presence::Presence::copy`]): the cards show the device's own
//! word; the owner's computer counts only for a build that does not tell
//! it.
//!
//! **Copies of this computer's masters** (a device with a window): every
//! [`AUDIT_EVERY`] each owner device online is asked which of this
//! computer's masters it holds (`/v1/replica/check`) and the `replica`
//! rows follow its answer; right before masters go to the Trash the
//! devices read their copies again ([`recheck`]).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{Metadata, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use axum::body::Body;
use nikgob_core::model::{Codec, Source, SourceKind};
use nikgob_library::Library;
use nikgob_core::model::TrackId;
use nikgob_library::replica::{CheckAnswer, CheckAsk, Held, HeldFile, Keep, MasterItem, MastersPage, Offer, Want, best_offers, held_satisfies, plan_tracks};
use nikgob_view::{Command, MeshCopy, MeshProfile, MeshScope, MeshTransfer};
use parking_lot::Mutex;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time::Instant;

use super::Shared;
use crate::connect::{clean_ext, clean_hash, range_starts_at};
use crate::presence::Assigned;

#[path = "replica/plan.rs"]
mod plan;
#[path = "replica/io.rs"]
mod io;
#[path = "replica/audit.rs"]
mod audit;

pub(crate) use audit::{audit, confirmed, count_copies, recheck};
pub(crate) use io::{Part, find_part, open_part, place_song, put_in_place, receive_paced, verify_part, write_note};
pub(crate) use plan::{Rehash, copy_stats, held_on_disk, masters_on_disk, plan_masters};
use io::pull;
use plan::replan;

#[cfg(all(test, unix))]
use io::blake3_plain;
#[cfg(test)]
use io::{place_master, read_note};
#[cfg(test)]
use plan::{masters_bytes, masters_plan, masters_progress, planned_bytes, songs_progress};

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
/// What each device holds of its plan is counted again this often (the
/// map's cards).
const COPIES_EVERY: Duration = Duration::from_secs(60);
/// The first count comes this soon after the mesh starts (the holder's
/// word is out by then).
const COPIES_FIRST: Duration = Duration::from_secs(10);
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
    /// What this device's plan gives it and what of it it holds (none: no
    /// such plan).
    copy: Mutex<Option<MeshCopy>>,
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

    pub(crate) fn copy(&self) -> Option<MeshCopy> {
        *self.copy.lock()
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
    /// What its plan gives it and what of it it holds (none: no plan).
    copy: Option<MeshCopy>,
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
                Ok((job, era, Ok(bytes))) => {
                    log::debug!("replica: {} kept ({bytes} bytes)", job.key());
                    st.failed.remove(&job.key());
                    // The plan's progress moves with each file kept (the
                    // next plan counts the disk again).
                    if era == ctl.era.load(Ordering::Relaxed)
                        && let Some(c) = st.copy.as_mut()
                    {
                        c.done = c.done.saturating_add(job.size()).min(c.goal);
                    }
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
    st.copy = None;
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
            if out.is_ok() {
                sh.rt.send(Command::ReplicaFileKept);
            }
            (job, era, out)
        });
        st.next_start = Some(Instant::now() + GAP);
    }
    show(sh, st, ctl, tasks, false);
}

/// The transfer as this device shows it (and tells the others), and what
/// its plan gives it and what of that it holds.
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
    let copy_changed = {
        let mut cur = sh.replica.copy.lock();
        let changed = *cur != st.copy;
        *cur = st.copy;
        changed
    };
    if changed || copy_changed {
        sh.notify();
    }
}

#[cfg(test)]
#[path = "replica/tests.rs"]
mod tests;
