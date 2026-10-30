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
use std::time::{Duration, Instant};

use log::{debug, info};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::client::{Inner, PeerHandle};
use crate::msg::{ConnType, PeerMsg, ToServer};
use crate::{DownloadInfo, DownloadState, Error, SearchResult};

/// Tries of one download before it fails (an interrupted transfer, the
/// sharer saying the upload failed).
const DOWNLOAD_TRIES: u32 = 6;
const MAX_QUEUED_PER_USER: usize = 200;
const MAX_QUEUED: usize = 5000;
/// Finished downloads kept in the list.
const FINISHED_KEPT: usize = 200;
const BUF: usize = 64 * 1024;
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

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
    size: AtomicU64,
    tries: AtomicU32,
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
        let mut v: Vec<DownloadInfo> = self
            .by_id
            .values()
            .map(|e| DownloadInfo {
                id: e.id,
                user: e.user.clone(),
                path: e.path.clone(),
                size: e.size.load(Ordering::Relaxed),
                state: e.state.borrow().clone(),
            })
            .collect();
        v.sort_by_key(|d| d.id);
        v
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
    let s = s.trim().trim_end_matches(['.', ' ']).to_owned();
    if s.is_empty() || s == "." || s == ".." { "_".into() } else { s }
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
    let folder = r.folder().rsplit('\\').next().unwrap_or("");
    let dir = if folder.is_empty() { dir.to_path_buf() } else { dir.join(sanitize(folder)) };
    let e = Arc::new(DownloadEntry {
        id: d.next_id,
        user: r.user.clone(),
        path: r.path.clone(),
        name: sanitize(r.file_name()),
        dir,
        state: watch::Sender::new(DownloadState::Queued { place: None }),
        cancel: inner.shutdown.child_token(),
        size: AtomicU64::new(r.size),
        tries: AtomicU32::new(0),
        busy: AtomicBool::new(false),
    });
    d.by_id.insert(e.id, e.clone());
    drop(d);
    let handle = e.handle();
    // A file of that name and size there already is taken as done.
    let done = e.dir.join(&e.name);
    if r.size > 0 && std::fs::metadata(&done).is_ok_and(|m| m.len() == r.size) {
        e.set(DownloadState::Done { path: done });
        return handle;
    }
    let me = inner.clone();
    tokio::spawn(async move {
        let watch = e.clone();
        let inner = me.clone();
        let mut rx = watch.state.subscribe();
        tokio::spawn(async move {
            let finished = async {
                while !rx.borrow_and_update().is_finished() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
            };
            tokio::select! {
                _ = watch.cancel.cancelled() => {
                    watch.set(DownloadState::Failed { reason: "Cancelled".into() });
                    inner.downloads.lock().ready.retain(|_, v| !Arc::ptr_eq(v, &watch));
                }
                _ = finished => {}
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
                h.send(&PeerMsg::QueueUpload(e.path.clone()));
                h.send(&PeerMsg::PlaceInQueueRequest(e.path.clone()));
                return;
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
    let part = e.dir.join(format!("{}.part", e.name));
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
    let done = free_path(&e.dir, &e.name);
    tokio::fs::rename(&part, &done).await?;
    e.busy.store(false, Ordering::Relaxed);
    info!("soulseek: downloaded {} from {}", e.name, e.user);
    e.set(DownloadState::Done { path: done });
    Ok(())
}

// ---- uploads ----

struct QueuedUpload {
    user: String,
    vpath: String,
    real: PathBuf,
    size: u64,
}

struct ActiveUpload {
    id: u64,
    user: String,
    vpath: String,
    cancel: CancellationToken,
}

#[derive(Default)]
pub(crate) struct Uploads {
    queue: VecDeque<QueuedUpload>,
    active: Vec<ActiveUpload>,
    next_id: u64,
}

impl Uploads {
    pub(crate) fn counts(&self) -> (u32, u32) {
        (self.active.len() as u32, self.queue.len() as u32)
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
            u.queue.push_back(QueuedUpload { user: user.to_owned(), vpath: vpath.to_owned(), real, size });
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
            u.next_id += 1;
            let id = u.next_id;
            let cancel = self.shutdown.child_token();
            u.active.push(ActiveUpload { id, user: q.user.clone(), vpath: q.vpath.clone(), cancel: cancel.clone() });
            drop(u);
            let me = self.clone();
            tokio::spawn(async move {
                let user = q.user.clone();
                let vpath = q.vpath.clone();
                match me.upload(&q, &cancel).await {
                    Ok(()) => {}
                    Err(e) => {
                        debug!("soulseek: upload of {vpath} to {user} stopped: {e}");
                        if let Some(h) = me.live_peer(&user) {
                            h.send(&PeerMsg::UploadFailed(vpath.clone()));
                        }
                    }
                }
                me.uploads.lock().active.retain(|a| a.id != id);
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

    async fn upload(self: &Arc<Self>, q: &QueuedUpload, cancel: &CancellationToken) -> Result<(), Error> {
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
                    Some(e) if !e.busy.load(Ordering::Relaxed) => {
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
                if let Some(e) = self.downloads.lock().find(user, &filename)
                    && matches!(*e.state.borrow(), DownloadState::Queued { .. })
                {
                    e.state.send_replace(DownloadState::Queued { place: Some(place) });
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
    fn free_names_count_up() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(free_path(d.path(), "a.flac"), d.path().join("a.flac"));
        std::fs::write(d.path().join("a.flac"), b"x").unwrap();
        assert_eq!(free_path(d.path(), "a.flac"), d.path().join("a (2).flac"));
    }
}
