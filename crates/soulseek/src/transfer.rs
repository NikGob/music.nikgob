//! File transfers. A download asks the sharer to queue the file
//! (`QueueUpload`); when its turn comes the sharer sends `TransferRequest`,
//! we accept, and it opens a file connection on which we tell how much we
//! have (`FileOffset`), so a partial file left from an interrupted try is
//! resumed. Uploads go the same way the other round, through a queue with
//! a fixed number of slots (one running upload per user at a time) and the
//! shared speed limit; they only ever open the shared files for reading.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use log::{debug, info};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::client::{Inner, PeerHandle};
use crate::msg::{ConnType, PeerMsg, ToServer};
use crate::{DownloadInfo, DownloadState, Error, SearchResult, UploadInfo, UploadState};

/// Tries of one download before it fails (an interrupted transfer, the
/// sharer saying the upload failed).
const DOWNLOAD_TRIES: u32 = 6;
const MAX_QUEUED_PER_USER: usize = 200;
const MAX_QUEUED: usize = 5000;
/// Finished downloads kept in the list.
const FINISHED_KEPT: usize = 200;
/// Finished uploads kept in the list.
const UPLOADS_KEPT: usize = 50;
const BUF: usize = 64 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// A file bigger than this is refused whatever size the sharer gives (a
/// long hi-res album as one FLAC stays well under it).
pub const MAX_FILE_BYTES: u64 = 8 << 30;
/// The longest file or folder name kept, in UTF-8 bytes (never fewer than
/// its UTF-16 units): room for the partial's tag and a ` (2)` under the 255
/// every file system allows.
const NAME_MAX: usize = 200;
/// Queue checks in a row without a word from the sharer about the file
/// (its place, the transfer) before the download is given up.
const UNANSWERED_CHECKS: u32 = 3;

/// A download in progress (or finished); cheap to clone.
#[derive(Clone)]
pub struct Download {
    id: u64,
    state: watch::Receiver<DownloadState>,
    cancel: CancellationToken,
}

impl Download {
    pub(crate) fn new(id: u64, state: watch::Receiver<DownloadState>, cancel: CancellationToken) -> Self {
        Self { id, state, cancel }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn state(&self) -> DownloadState {
        self.state.borrow().clone()
    }

    /// Changes of the state (for a progress bar).
    pub fn subscribe(&self) -> watch::Receiver<DownloadState> {
        self.state.clone()
    }

    /// Waits for the end: the file's path, or why it failed.
    pub async fn wait(&mut self) -> Result<PathBuf, Error> {
        loop {
            match &*self.state.borrow_and_update() {
                DownloadState::Done { path } => return Ok(path.clone()),
                DownloadState::Failed { reason } if reason == "Cancelled" => return Err(Error::Cancelled),
                DownloadState::Failed { reason } => return Err(Error::Denied(reason.clone())),
                _ => {}
            }
            if self.state.changed().await.is_err() {
                return Err(Error::Cancelled);
            }
        }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

pub(crate) struct DownloadEntry {
    id: u64,
    user: String,
    path: String,
    /// The folder the file goes to.
    dir: PathBuf,
    name: String,
    state: watch::Sender<DownloadState>,
    cancel: CancellationToken,
    /// The size the search gave: with the user and the path, what the
    /// partial file is tied to.
    listed: u64,
    size: AtomicU64,
    tries: AtomicU32,
    /// Queue checks in a row the sharer gave no word about this file.
    unanswered: AtomicU32,
    /// A file connection is running.
    busy: AtomicBool,
}

impl DownloadEntry {
    fn handle(&self) -> Download {
        Download::new(self.id, self.state.subscribe(), self.cancel.clone())
    }

    fn finished(&self) -> bool {
        self.state.borrow().is_finished()
    }

    fn info(&self) -> DownloadInfo {
        DownloadInfo {
            id: self.id,
            user: self.user.clone(),
            path: self.path.clone(),
            size: self.size.load(Ordering::Relaxed),
            state: self.state.borrow().clone(),
        }
    }

    fn set(&self, s: DownloadState) {
        if !self.finished() {
            self.state.send_replace(s);
        }
    }

    fn fail(&self, reason: impl Into<String>) {
        let reason = reason.into();
        info!("soulseek: download of {} from {} failed: {reason}", self.name, self.user);
        self.set(DownloadState::Failed { reason });
    }
}

#[derive(Default)]
pub(crate) struct Downloads {
    by_id: HashMap<u64, Arc<DownloadEntry>>,
    /// Accepted transfers waiting for their file connection, by (user,
    /// token).
    ready: HashMap<(String, u32), Arc<DownloadEntry>>,
    next_id: u64,
}

impl Downloads {
    fn find(&self, user: &str, path: &str) -> Option<Arc<DownloadEntry>> {
        self.by_id.values().find(|e| e.user == user && e.path == path && !e.finished()).cloned()
    }

    pub(crate) fn active(&self) -> u32 {
        self.by_id.values().filter(|e| !e.finished()).count() as u32
    }

    pub(crate) fn list(&self) -> Vec<DownloadInfo> {
        let mut v: Vec<DownloadInfo> = self.by_id.values().map(|e| e.info()).collect();
        v.sort_by_key(|d| d.id);
        v
    }

    pub(crate) fn info(&self, id: u64) -> Option<DownloadInfo> {
        self.by_id.get(&id).map(|e| e.info())
    }

    pub(crate) fn handle(&self, id: u64) -> Option<Download> {
        self.by_id.get(&id).map(|e| e.handle())
    }

    fn prune(&mut self) {
        let mut done: Vec<u64> = self.by_id.values().filter(|e| e.finished()).map(|e| e.id).collect();
        if done.len() > FINISHED_KEPT {
            done.sort_unstable();
            for id in &done[..done.len() - FINISHED_KEPT] {
                self.by_id.remove(id);
            }
        }
    }
}

/// A name that is safe as one path component on every system.
pub(crate) fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c })
        .collect();
    let s = s.trim().trim_end_matches(['.', ' ']);
    if s.is_empty() || s == "." || s == ".." {
        return "_".into();
    }
    let s = shorten(s, NAME_MAX);
    if reserved_on_windows(&s) { format!("_{s}") } else { s }
}

/// `name` cut to at most `max` bytes on a character boundary, its
/// extension kept.
fn shorten(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let cut = |s: &str, n: usize| -> String {
        let mut end = n.min(s.len());
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s[..end].trim_end_matches(['.', ' ']).to_owned()
    };
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 16 => {
            let stem = cut(stem, max - ext.len() - 1);
            format!("{}.{ext}", if stem.is_empty() { "_" } else { &stem })
        }
        _ => {
            let s = cut(name, max);
            if s.is_empty() { "_".into() } else { s }
        }
    }
}

/// `CON`, `nul.flac`, `COM1.txt`…: device names Windows does not open as
/// files, whatever the extension.
fn reserved_on_windows(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    let b = stem.as_bytes();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (b.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && b[3].is_ascii_digit())
}

/// The folder a download goes to under `dir`: the sharer's last folder.
fn dest_dir(dir: &Path, r: &SearchResult) -> PathBuf {
    let folder = r.folder().rsplit('\\').next().unwrap_or("");
    if folder.is_empty() { dir.to_path_buf() } else { dir.join(sanitize(folder)) }
}

/// 128 bits of SHA-256 over the parts, each length-prefixed (no two
/// different lists give the same bytes): the same on every run and build,
/// and no name a peer picks makes another file's tag.
fn identity_tag(parts: &[&[u8]]) -> String {
    use sha2::{Digest as _, Sha256};
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    h.finalize()[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// The partial file's name: tagged with the user, the path there and the
/// listed size, so only bytes of that very file are ever resumed into it
/// (one download per user and path runs at a time, so no two jobs share it).
fn partial_name(user: &str, path: &str, listed: u64, name: &str) -> String {
    let tag = identity_tag(&[user.as_bytes(), path.as_bytes(), &listed.to_le_bytes()]);
    format!("{name}.{tag}.part")
}

/// Where the partial file of a download of `r` into `dir` is kept.
pub fn partial_path(dir: &Path, r: &SearchResult) -> PathBuf {
    dest_dir(dir, r).join(partial_name(&r.user, &r.path, r.size, &sanitize(r.file_name())))
}

/// Moves the finished `part` to a free name for `name` in `dir`, by a move
/// that fails when anything is at the name (taken meanwhile by anyone): the
/// next name is tried then, and no file is ever replaced or left behind.
/// It runs to the end even when the caller stops waiting.
pub(crate) async fn finish_into(part: &Path, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    let (part, dir, name) = (part.to_path_buf(), dir.to_path_buf(), name.to_owned());
    tokio::task::spawn_blocking(move || publish(&part, &dir, &name)).await.map_err(std::io::Error::other)?
}

fn publish(part: &Path, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    for _ in 0..1000 {
        let p = free_path(dir, name);
        match no_replace::rename(part, &p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, format!("no free name for {name}")))
}

/// A rename that never replaces: it fails with `AlreadyExists` when
/// anything is at the target, decided by the file system in the same step.
#[allow(unsafe_code)]
mod no_replace {
    use std::io;
    use std::path::Path;

    #[cfg(windows)]
    pub(super) fn rename(from: &Path, to: &Path) -> io::Result<()> {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
        let wide = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<u16>>();
        let (from, to) = (wide(from), wide(to));
        // SAFETY: two NUL-terminated UTF-16 strings that outlive the call.
        // Without MOVEFILE_REPLACE_EXISTING the move fails if `to` exists.
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    pub(super) fn rename(from: &Path, to: &Path) -> io::Result<()> {
        use std::os::unix::ffi::OsStrExt as _;
        let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput));
        let (f, t) = (c(from)?, c(to)?);
        // SAFETY: two NUL-terminated paths that outlive the call.
        let r = unsafe { libc::renameat2(libc::AT_FDCWD, f.as_ptr(), libc::AT_FDCWD, t.as_ptr(), libc::RENAME_NOREPLACE) };
        if r == 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if !matches!(err.raw_os_error(), Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)) {
            return Err(err);
        }
        // A file system without the flag: a new hard link is just as
        // exclusive (it fails if the name exists), then the old name goes.
        std::fs::hard_link(from, to)?;
        let _ = std::fs::remove_file(from);
        Ok(())
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    pub(super) fn rename(from: &Path, to: &Path) -> io::Result<()> {
        std::fs::hard_link(from, to)?;
        let _ = std::fs::remove_file(from);
        Ok(())
    }
}

/// `path`, or `name (2).ext`… when something is there already.
fn free_path(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    (2..).map(|i| dir.join(format!("{stem} ({i}){ext}"))).find(|p| !p.exists()).expect("a free name")
}

pub(crate) fn start_download(inner: &Arc<Inner>, r: &SearchResult, dir: &Path) -> Download {
    let mut d = inner.downloads.lock();
    if let Some(e) = d.find(&r.user, &r.path) {
        return e.handle();
    }
    d.prune();
    d.next_id += 1;
    let e = Arc::new(DownloadEntry {
        id: d.next_id,
        user: r.user.clone(),
        path: r.path.clone(),
        name: sanitize(r.file_name()),
        dir: dest_dir(dir, r),
        state: watch::Sender::new(DownloadState::Queued { place: None }),
        cancel: inner.shutdown.child_token(),
        listed: r.size,
        size: AtomicU64::new(r.size),
        tries: AtomicU32::new(0),
        unanswered: AtomicU32::new(0),
        busy: AtomicBool::new(false),
    });
    d.by_id.insert(e.id, e.clone());
    drop(d);
    let handle = e.handle();
    if r.size > MAX_FILE_BYTES {
        e.fail("the file is too large");
        return handle;
    }
    // A file of that name there already is not taken for this one, whatever
    // its size: the download always comes from the sharer, to a free name.
    let me = inner.clone();
    tokio::spawn(async move {
        let watch = e.clone();
        let inner = me.clone();
        let mut rx = watch.state.subscribe();
        tokio::spawn(async move {
            let check = inner.cfg.timeouts.queue_check;
            while !rx.borrow_and_update().is_finished() {
                tokio::select! {
                    _ = watch.cancel.cancelled() => {
                        watch.set(DownloadState::Failed { reason: "Cancelled".into() });
                        inner.downloads.lock().ready.retain(|_, v| !Arc::ptr_eq(v, &watch));
                        break;
                    }
                    r = rx.changed() => if r.is_err() { break },
                    // Still in the sharer's queue with no word for a while:
                    // ask again (the place too). A sharer that went away is
                    // found offline or unreachable; one that keeps the
                    // connection but never answers about the file gives up
                    // the download after a few checks; one with a long queue
                    // answers with its place and keeps it.
                    _ = tokio::time::sleep(check) => {
                        if matches!(*watch.state.borrow(), DownloadState::Queued { .. }) {
                            if watch.unanswered.fetch_add(1, Ordering::Relaxed) + 1 > UNANSWERED_CHECKS {
                                watch.fail("the sharer does not answer");
                                break;
                            }
                            let (inner, e) = (inner.clone(), watch.clone());
                            tokio::spawn(async move { request(&inner, &e).await });
                        }
                    }
                }
            }
        });
        request(&me, &e).await;
    });
    handle
}

/// Asks the sharer to queue the file (again).
async fn request(inner: &Arc<Inner>, e: &Arc<DownloadEntry>) {
    for attempt in 0..3 {
        if e.finished() {
            return;
        }
        match inner.peer(&e.user).await {
            Ok(h) => {
                if h.send(&PeerMsg::QueueUpload(e.path.clone())) && h.send(&PeerMsg::PlaceInQueueRequest(e.path.clone())) {
                    return;
                }
                // The connection closed meanwhile: the next try opens another.
                debug!("soulseek: the connection to {} closed before the request", e.user);
            }
            Err(Error::Offline(_)) => {
                e.fail("the user is offline");
                return;
            }
            Err(err) if attempt == 2 => {
                e.fail(err.to_string());
                return;
            }
            Err(err) => {
                debug!("soulseek: cannot reach {} yet: {err}", e.user);
                tokio::select! {
                    _ = e.cancel.cancelled() => return,
                    _ = tokio::time::sleep(inner.cfg.timeouts.reconnect) => {}
                }
            }
        }
    }
}

/// Counts a failed try and asks again, or gives up.
fn retry(inner: &Arc<Inner>, e: &Arc<DownloadEntry>, why: &str) {
    if e.finished() {
        return;
    }
    if e.tries.fetch_add(1, Ordering::Relaxed) + 1 >= DOWNLOAD_TRIES {
        e.fail(why.to_owned());
        return;
    }
    e.set(DownloadState::Queued { place: None });
    let (inner, e) = (inner.clone(), e.clone());
    tokio::spawn(async move { request(&inner, &e).await });
}

/// An incoming file connection: the sharer sends the transfer's token
/// first.
pub(crate) async fn incoming_file(inner: Arc<Inner>, user: String, mut s: TcpStream) {
    let Ok(Ok(token)) = timeout(inner.cfg.timeouts.transfer, s.read_u32_le()).await else { return };
    let Some(e) = inner.downloads.lock().ready.remove(&(user.clone(), token)) else {
        debug!("soulseek: file connection from {user} for an unknown transfer");
        return;
    };
    if let Err(err) = receive(&inner, &e, s).await {
        debug!("soulseek: transfer of {} from {user} stopped: {err}", e.name);
        e.busy.store(false, Ordering::Relaxed);
        if !e.cancel.is_cancelled() {
            retry(&inner, &e, &err.to_string());
        }
    }
}

async fn receive(inner: &Arc<Inner>, e: &Arc<DownloadEntry>, mut s: TcpStream) -> Result<(), Error> {
    e.busy.store(true, Ordering::Relaxed);
    let size = e.size.load(Ordering::Relaxed);
    tokio::fs::create_dir_all(&e.dir).await?;
    let part = e.dir.join(partial_name(&e.user, &e.path, e.listed, &e.name));
    let mut offset = tokio::fs::metadata(&part).await.map_or(0, |m| m.len());
    let mut file = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&part).await?;
    if offset > size {
        offset = 0;
    }
    file.set_len(offset).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    s.write_u64_le(offset).await?;
    s.flush().await?;
    let mut got = offset;
    let mut buf = vec![0u8; BUF];
    let started = Instant::now();
    let mut shown = Instant::now() - PROGRESS_EVERY;
    e.set(DownloadState::Transferring { received: got, size, bytes_per_sec: 0 });
    while got < size {
        let want = ((size - got) as usize).min(BUF);
        let n = tokio::select! {
            _ = e.cancel.cancelled() => {
                file.flush().await?;
                return Ok(());
            }
            r = timeout(inner.cfg.timeouts.transfer, s.read(&mut buf[..want])) => r.map_err(|_| Error::Timeout)??,
        };
        if n == 0 {
            file.flush().await?;
            return Err(Error::Protocol(format!("the sharer closed the transfer at {got} of {size} bytes")));
        }
        file.write_all(&buf[..n]).await?;
        got += n as u64;
        if shown.elapsed() >= PROGRESS_EVERY {
            shown = Instant::now();
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let rate = ((got - offset) as f64 / secs) as u64;
            e.set(DownloadState::Transferring { received: got, size, bytes_per_sec: rate });
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    // The downloader closes the connection when it has everything.
    drop(s);
    if e.cancel.is_cancelled() {
        // Cancelled as the last bytes came: nothing is published, the
        // partial stays for a resume.
        e.busy.store(false, Ordering::Relaxed);
        return Ok(());
    }
    let done = finish_into(&part, &e.dir, &e.name).await?;
    e.busy.store(false, Ordering::Relaxed);
    info!("soulseek: downloaded {} from {}", e.name, e.user);
    e.set(DownloadState::Done { path: done });
    Ok(())
}

// ---- uploads ----

struct QueuedUpload {
    id: u64,
    user: String,
    vpath: String,
    real: PathBuf,
    size: u64,
}

/// How far a running upload is, written by its task.
#[derive(Default)]
struct UploadProgress {
    /// The place in the file.
    sent: AtomicU64,
    /// Bytes sent by this transfer (a resumed one starts past 0).
    moved: AtomicU64,
}

struct ActiveUpload {
    id: u64,
    user: String,
    vpath: String,
    size: u64,
    cancel: CancellationToken,
    progress: Arc<UploadProgress>,
    started: Instant,
}

impl ActiveUpload {
    /// The row, with the speed this transfer averaged so far.
    fn info(&self, state: UploadState, ended_at: u64) -> UploadInfo {
        let secs = self.started.elapsed().as_secs_f64().max(0.001);
        UploadInfo {
            id: self.id,
            user: self.user.clone(),
            path: self.vpath.clone(),
            size: self.size,
            sent: self.progress.sent.load(Ordering::Relaxed),
            bytes_per_sec: (self.progress.moved.load(Ordering::Relaxed) as f64 / secs) as u64,
            state,
            ended_at,
        }
    }
}

fn unix_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Default)]
pub(crate) struct Uploads {
    queue: VecDeque<QueuedUpload>,
    active: Vec<ActiveUpload>,
    /// The last finished ones, newest first.
    finished: VecDeque<UploadInfo>,
    next_id: u64,
}

impl Uploads {
    pub(crate) fn counts(&self) -> (u32, u32) {
        (self.active.len() as u32, self.queue.len() as u32)
    }

    /// Running uploads, then the queue in order, then the finished ones
    /// (newest first).
    pub(crate) fn list(&self) -> Vec<UploadInfo> {
        let running = self.active.iter().map(|a| a.info(UploadState::Sending, 0));
        let queued = self.queue.iter().map(|q| UploadInfo {
            id: q.id,
            user: q.user.clone(),
            path: q.vpath.clone(),
            size: q.size,
            sent: 0,
            bytes_per_sec: 0,
            state: UploadState::Queued,
            ended_at: 0,
        });
        running.chain(queued).chain(self.finished.iter().cloned()).collect()
    }

    /// Takes running upload `id` off the list and keeps how it ended: done
    /// when the whole file went, failed otherwise (a peer that declined
    /// ends the task without an error but has nothing).
    fn finish(&mut self, id: u64, outcome: Result<(), String>) {
        let Some(i) = self.active.iter().position(|a| a.id == id) else { return };
        let a = self.active.remove(i);
        let complete = a.progress.sent.load(Ordering::Relaxed) >= a.size;
        let state = match outcome {
            Ok(()) if complete => UploadState::Done,
            Ok(()) => UploadState::Failed { reason: "Declined".into() },
            Err(reason) => UploadState::Failed { reason },
        };
        self.finished.push_front(a.info(state, unix_secs()));
        self.finished.truncate(UPLOADS_KEPT);
    }

    fn place(&self, user: &str, vpath: &str) -> Option<u32> {
        if self.active.iter().any(|a| a.user == user && a.vpath == vpath) {
            return Some(0);
        }
        self.queue.iter().position(|q| q.user == user && q.vpath == vpath).map(|p| p as u32 + 1)
    }
}

impl Inner {
    /// Queues `vpath` for `user`, or says why not.
    fn enqueue_upload(self: &Arc<Self>, user: &str, vpath: &str) -> Result<(), &'static str> {
        if !self.share_settings.lock().enabled {
            return Err("File not shared.");
        }
        let (real, size) = self.share.read().resolve(vpath).ok_or("File not shared.")?;
        {
            let mut u = self.uploads.lock();
            if u.place(user, vpath).is_some() {
                return Ok(());
            }
            if u.queue.len() >= MAX_QUEUED || u.queue.iter().filter(|q| q.user == user).count() >= MAX_QUEUED_PER_USER {
                return Err("Too many files");
            }
            u.next_id += 1;
            let id = u.next_id;
            u.queue.push_back(QueuedUpload { id, user: user.to_owned(), vpath: vpath.to_owned(), real, size });
        }
        self.pump_uploads();
        Ok(())
    }

    /// Starts queued uploads while slots are free, one per user at a time.
    pub(crate) fn pump_uploads(self: &Arc<Self>) {
        let slots = self.share_settings.lock().upload_slots.max(1) as usize;
        loop {
            let mut u = self.uploads.lock();
            if u.active.len() >= slots {
                return;
            }
            let Some(i) = u.queue.iter().position(|q| !u.active.iter().any(|a| a.user == q.user)) else { return };
            let q = u.queue.remove(i).expect("an index in range");
            let id = q.id;
            let cancel = self.shutdown.child_token();
            let progress = Arc::new(UploadProgress::default());
            u.active.push(ActiveUpload {
                id,
                user: q.user.clone(),
                vpath: q.vpath.clone(),
                size: q.size,
                cancel: cancel.clone(),
                progress: progress.clone(),
                started: Instant::now(),
            });
            drop(u);
            let me = self.clone();
            tokio::spawn(async move {
                let user = q.user.clone();
                let vpath = q.vpath.clone();
                let outcome = match me.upload(&q, &cancel, &progress).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        debug!("soulseek: upload of {vpath} to {user} stopped: {e}");
                        if let Some(h) = me.live_peer(&user) {
                            h.send(&PeerMsg::UploadFailed(vpath.clone()));
                        }
                        Err(e.to_string())
                    }
                };
                me.uploads.lock().finish(id, outcome);
                me.pump_uploads();
            });
        }
    }

    /// Ends every queued and running upload (sharing was turned off).
    pub(crate) fn stop_uploads(self: &Arc<Self>) {
        let mut u = self.uploads.lock();
        let queued: Vec<QueuedUpload> = u.queue.drain(..).collect();
        for a in &u.active {
            a.cancel.cancel();
        }
        drop(u);
        for q in queued {
            if let Some(h) = self.live_peer(&q.user) {
                h.send(&PeerMsg::UploadDenied { filename: q.vpath, reason: "File not shared.".into() });
            }
        }
    }

    async fn upload(
        self: &Arc<Self>,
        q: &QueuedUpload,
        cancel: &CancellationToken,
        progress: &UploadProgress,
    ) -> Result<(), Error> {
        let h = self.peer(&q.user).await?;
        let token = self.token();
        let (tx, rx) = oneshot::channel();
        self.transfer_responses.lock().insert((q.user.clone(), token), tx);
        h.send(&PeerMsg::TransferRequest { direction: 1, token, filename: q.vpath.clone(), size: Some(q.size) });
        let answer = tokio::select! {
            _ = cancel.cancelled() => Err(Error::Cancelled),
            r = timeout(self.cfg.timeouts.transfer, rx) => r.map_err(|_| Error::Timeout).and_then(|r| r.map_err(|_| Error::Cancelled)),
        };
        self.transfer_responses.lock().remove(&(q.user.clone(), token));
        let (allowed, reason) = answer?;
        if !allowed {
            debug!("soulseek: {} declined {}: {}", q.user, q.vpath, reason.unwrap_or_default());
            return Ok(());
        }
        let mut s = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            r = self.connect(&q.user, ConnType::F) => r?,
        };
        s.write_u32_le(token).await?;
        s.flush().await?;
        let offset = timeout(self.cfg.timeouts.transfer, s.read_u64_le()).await.map_err(|_| Error::Timeout)??;
        if offset > q.size {
            return Err(Error::Protocol(format!("offset {offset} past the end")));
        }
        // Read-only: the library is never written.
        let mut file = tokio::fs::File::open(&q.real).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut buf = vec![0u8; BUF];
        let mut sent = offset;
        progress.sent.store(sent, Ordering::Relaxed);
        let started = Instant::now();
        while sent < q.size {
            let n = self.limiter.chunk(BUF).min((q.size - sent) as usize);
            let n = file.read(&mut buf[..n]).await?;
            if n == 0 {
                return Err(Error::Protocol("the file got shorter".into()));
            }
            tokio::select! {
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                _ = self.limiter.take(n) => {}
            }
            s.write_all(&buf[..n]).await?;
            sent += n as u64;
            self.uploaded_bytes.fetch_add(n as u64, Ordering::Relaxed);
            progress.sent.store(sent, Ordering::Relaxed);
            progress.moved.fetch_add(n as u64, Ordering::Relaxed);
        }
        s.flush().await?;
        // The downloader closes the connection once it has everything.
        let mut one = [0u8; 1];
        let _ = timeout(self.cfg.timeouts.transfer, s.read(&mut one)).await;
        let secs = started.elapsed().as_secs_f64().max(0.001);
        let speed = ((sent - offset) as f64 / secs).min(f64::from(u32::MAX)) as u32;
        if sent > offset {
            let old = self.avg_speed.load(Ordering::Relaxed);
            self.avg_speed.store(if old == 0 { speed } else { (old / 4) * 3 + speed / 4 }, Ordering::Relaxed);
            let _ = self.server_send(ToServer::SendUploadSpeed(speed));
        }
        info!("soulseek: uploaded {} to {}", q.vpath, q.user);
        Ok(())
    }

    pub(crate) fn on_transfer_msg(self: &Arc<Self>, user: &str, m: PeerMsg, h: &Arc<PeerHandle>) {
        match m {
            // The sharer is ready to send us a file.
            PeerMsg::TransferRequest { direction: 1, token, filename, size } => {
                let found = self.downloads.lock().find(user, &filename);
                match found {
                    Some(e) if size.is_some_and(|s| s > MAX_FILE_BYTES) => {
                        h.send(&PeerMsg::TransferResponse {
                            token,
                            allowed: false,
                            size: None,
                            reason: Some("Cancelled".into()),
                        });
                        e.fail("the file is too large");
                    }
                    Some(e) if !e.busy.load(Ordering::Relaxed) => {
                        e.unanswered.store(0, Ordering::Relaxed);
                        if let Some(s) = size {
                            e.size.store(s, Ordering::Relaxed);
                        }
                        self.downloads.lock().ready.insert((user.to_owned(), token), e.clone());
                        e.set(DownloadState::Connecting);
                        h.send(&PeerMsg::TransferResponse { token, allowed: true, size: None, reason: None });
                        // The file connection must follow; if it does not, ask again.
                        let (me, user) = (self.clone(), user.to_owned());
                        tokio::spawn(async move {
                            tokio::time::sleep(me.cfg.timeouts.transfer).await;
                            let stale = me.downloads.lock().ready.remove(&(user, token));
                            if let Some(e) = stale {
                                retry(&me, &e, "the sharer did not connect for the transfer");
                            }
                        });
                    }
                    _ => {
                        h.send(&PeerMsg::TransferResponse {
                            token,
                            allowed: false,
                            size: None,
                            reason: Some("Cancelled".into()),
                        });
                    }
                }
            }
            // A legacy download request: queue it and say so.
            PeerMsg::TransferRequest { token, filename, .. } => {
                let reason = match self.enqueue_upload(user, &filename) {
                    Ok(()) => "Queued",
                    Err(r) => r,
                };
                h.send(&PeerMsg::TransferResponse { token, allowed: false, size: None, reason: Some(reason.into()) });
            }
            PeerMsg::TransferResponse { token, allowed, reason, .. } => {
                if let Some(w) = self.transfer_responses.lock().remove(&(user.to_owned(), token)) {
                    let _ = w.send((allowed, reason));
                }
            }
            PeerMsg::QueueUpload(filename) => {
                if let Err(reason) = self.enqueue_upload(user, &filename) {
                    h.send(&PeerMsg::UploadDenied { filename, reason: reason.into() });
                }
            }
            PeerMsg::PlaceInQueueRequest(filename) => {
                let place = self.uploads.lock().place(user, &filename);
                if let Some(place) = place {
                    h.send(&PeerMsg::PlaceInQueueResponse { filename, place });
                }
            }
            PeerMsg::PlaceInQueueResponse { filename, place } => {
                if let Some(e) = self.downloads.lock().find(user, &filename) {
                    e.unanswered.store(0, Ordering::Relaxed);
                    if matches!(*e.state.borrow(), DownloadState::Queued { .. }) {
                        e.state.send_replace(DownloadState::Queued { place: Some(place) });
                    }
                }
            }
            PeerMsg::UploadDenied { filename, reason } => {
                if let Some(e) = self.downloads.lock().find(user, &filename) {
                    e.fail(reason);
                }
            }
            PeerMsg::UploadFailed(filename) => {
                let found = self.downloads.lock().find(user, &filename);
                if let Some(e) = found
                    && matches!(*e.state.borrow(), DownloadState::Connecting)
                {
                    self.downloads.lock().ready.retain(|_, v| !Arc::ptr_eq(v, &e));
                    retry(self, &e, "the sharer could not send the file");
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(sanitize("a/b\\c:d?.flac"), "a_b_c_d_.flac");
        assert_eq!(sanitize(".."), "_");
        assert_eq!(sanitize("Album. "), "Album");
        assert_eq!(sanitize("勘冴えて悔しいわ.flac"), "勘冴えて悔しいわ.flac");
    }

    #[test]
    fn windows_device_names_and_long_names_are_made_safe() {
        assert_eq!(sanitize("CON.flac"), "_CON.flac");
        assert_eq!(sanitize("nul"), "_nul");
        assert_eq!(sanitize("Aux.Live.mp3"), "_Aux.Live.mp3");
        assert_eq!(sanitize("com1.flac"), "_com1.flac");
        assert_eq!(sanitize("LPT9"), "_LPT9");
        assert_eq!(sanitize("Console.flac"), "Console.flac");
        assert_eq!(sanitize("COM10.flac"), "COM10.flac");
        let long = format!("{}.flac", "я".repeat(300));
        let s = sanitize(&long);
        assert!(s.len() <= NAME_MAX, "{}", s.len());
        assert!(s.ends_with("я.flac"));
        let s = sanitize(&"a".repeat(400));
        assert_eq!(s.len(), NAME_MAX);
        // The partial's name stays under every system's 255.
        assert!(partial_name("u", "p", 1, &sanitize(&long)).len() < 255);
    }

    #[test]
    fn partials_are_tied_to_the_sharer_the_path_and_the_size() {
        let a = partial_name("carol", r"Music\A.flac", 100, "01.flac");
        assert_eq!(a, partial_name("carol", r"Music\A.flac", 100, "01.flac"));
        assert!(a.starts_with("01.flac.") && a.ends_with(".part"));
        assert_ne!(a, partial_name("dave", r"Music\A.flac", 100, "01.flac"));
        assert_ne!(a, partial_name("carol", r"Other\A.flac", 100, "01.flac"));
        assert_ne!(a, partial_name("carol", r"Music\A.flac", 101, "01.flac"));
        // Fixed across runs: a partial left by an earlier run is found again.
        assert_eq!(identity_tag(&[b"a", b"b"]), identity_tag(&[b"a", b"b"]));
        assert_ne!(identity_tag(&[b"ab", b""]), identity_tag(&[b"a", b"b"]));
        assert_eq!(identity_tag(&[b"a"]).len(), 32);
        // The two names that shared a 32-bit tag do not share this one.
        let path = r"Shared\Album\track.flac";
        assert_ne!(
            partial_name("13777875243598285780", path, 1000, "track.flac"),
            partial_name("4034700295885533334", path, 1000, "track.flac")
        );
    }

    #[test]
    fn the_no_replace_rename_leaves_a_file_there_alone() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("a.part"), d.path().join("a.flac"));
        std::fs::write(&from, b"new").unwrap();
        std::fs::write(&to, b"theirs").unwrap();
        let err = no_replace::rename(&from, &to).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists, "{err}");
        assert_eq!(std::fs::read(&to).unwrap(), b"theirs");
        assert_eq!(std::fs::read(&from).unwrap(), b"new");
        std::fs::remove_file(&to).unwrap();
        no_replace::rename(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
        assert!(!from.exists());
    }

    #[tokio::test]
    async fn finishing_never_replaces_a_file_there() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.flac"), b"mine").unwrap();
        let part = d.path().join("x.part");
        std::fs::write(&part, b"new").unwrap();
        let done = finish_into(&part, d.path(), "a.flac").await.unwrap();
        assert_eq!(done, d.path().join("a (2).flac"));
        assert_eq!(std::fs::read(d.path().join("a.flac")).unwrap(), b"mine");
        assert_eq!(std::fs::read(&done).unwrap(), b"new");
        assert!(!part.exists());
        // Nothing else was made (no placeholder left behind).
        assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 2);
    }

    fn active(u: &mut Uploads, size: u64, sent: u64) -> u64 {
        u.next_id += 1;
        let progress = Arc::new(UploadProgress::default());
        progress.sent.store(sent, Ordering::Relaxed);
        progress.moved.store(sent, Ordering::Relaxed);
        u.active.push(ActiveUpload {
            id: u.next_id,
            user: "peer".into(),
            vpath: format!("@@music\\{}.flac", u.next_id),
            size,
            cancel: CancellationToken::new(),
            progress,
            started: Instant::now(),
        });
        u.next_id
    }

    #[test]
    fn uploads_list_running_queued_then_finished() {
        let mut u = Uploads::default();
        let a = active(&mut u, 10, 4);
        u.next_id += 1;
        u.queue.push_back(QueuedUpload {
            id: u.next_id,
            user: "other".into(),
            vpath: "@@music\\q.flac".into(),
            real: PathBuf::from("q.flac"),
            size: 7,
        });
        let list = u.list();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].id, list[0].sent, list[0].state.clone()), (a, 4, UploadState::Sending));
        assert_eq!(list[1].state, UploadState::Queued);

        u.finish(a, Ok(()));
        let list = u.list();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].state, UploadState::Queued);
        assert!(matches!(list[1].state, UploadState::Failed { .. }), "4 of 10 bytes is not done");
        assert!(list[1].ended_at > 0);
    }

    #[test]
    fn finished_uploads_are_done_failed_and_capped() {
        let mut u = Uploads::default();
        let whole = active(&mut u, 10, 10);
        u.finish(whole, Ok(()));
        let cut = active(&mut u, 10, 3);
        u.finish(cut, Err("timed out".into()));
        let list = u.list();
        assert_eq!(list[0].state, UploadState::Failed { reason: "timed out".into() });
        assert_eq!(list[1].state, UploadState::Done);
        // An id no longer running changes nothing.
        u.finish(whole, Ok(()));
        assert_eq!(u.list().len(), 2);
        for _ in 0..UPLOADS_KEPT + 5 {
            let id = active(&mut u, 1, 1);
            u.finish(id, Ok(()));
        }
        assert_eq!(u.list().len(), UPLOADS_KEPT);
    }

    #[test]
    fn free_names_count_up() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(free_path(d.path(), "a.flac"), d.path().join("a.flac"));
        std::fs::write(d.path().join("a.flac"), b"x").unwrap();
        assert_eq!(free_path(d.path(), "a.flac"), d.path().join("a (2).flac"));
    }
}
