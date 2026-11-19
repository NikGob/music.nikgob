//! A seekable reader over an HTTP resource fetched in ranged chunks.
//!
//! [`HttpStream`] is the [`AudioStream`] handed to the audio engine. Nothing
//! happens until it is first used; then a download task on the service's
//! runtime resolves the stream (the YouTube player request), and fetches the
//! bytes in `Range` requests: a small first one so playback starts quickly,
//! then up to [`MAX_REQUEST`] each, from the read position to the end and
//! then any gaps left before it. The whole resource is kept, so seeking back
//! (the Matroska demuxer reopens from byte 0 to seek) costs nothing: in
//! memory blocks allocated as bytes arrive, or in a temporary file when it
//! is longer than [`MEMORY_MAX`] (memory never follows the claimed length).
//!
//! Reads (on the engine's decoder thread) only copy downloaded bytes and
//! block while the wanted byte is not there yet. A read far away from the
//! running request makes the task drop it and continue at the new position.
//! Transient failures (network errors, 5xx, a response cut short) are
//! retried with backoff; a 403/410 (the signed URL expired or was refused)
//! resolves the stream again, telling the resolve which URL was refused (so
//! the player is asked through another client), and continues at the same
//! offset. A final
//! failure reaches the reader as a [`StreamError`] inside the I/O error, so
//! a refusal is not taken for a lost connection.
//!
//! Once everything is downloaded the blocks are shared with the disk cache's
//! writer (`Arc`s), not copied: nothing writes into them any more. The
//! stores alive are counted ([`buffers`]) for the app's memory line.

use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nikgob_core::streaming::{
    AudioStream, BoxFuture, StreamError, StreamInterrupter, format, interrupted_read,
};
use tokio::runtime::Handle;
use tokio::sync::Notify;

/// Size of the first request (fast start).
pub(crate) const FIRST_REQUEST: u64 = 256 * 1024;
/// Upper bound of every later request.
pub(crate) const MAX_REQUEST: u64 = 8 * 1024 * 1024;
/// A read this far beyond the running request's download point restarts
/// the download there.
const SEEK_RESTART: u64 = 1024 * 1024;
/// Attempts without any progress before a transient error is final.
const MAX_ATTEMPTS: u32 = 5;
/// Re-resolves without any progress before a 403 is final. A fresh URL is
/// often refused once or twice right after the first chunk and then
/// serves the rest (the live log of 2026-10-28): one more try than that,
/// each a little later.
const MAX_RESOLVES: u32 = 4;
/// The pause before the n-th re-resolve in a row: n times this.
const RESOLVE_PAUSE: Duration = Duration::from_millis(400);
/// Longest wait of a read for its bytes (a stalled connection is cut by the
/// HTTP client's read timeout well before).
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// What a resolve says about the stream.
#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    /// Signed media URL.
    pub url: String,
    /// Content length in bytes.
    pub len: u64,
    /// One of [`format`].
    pub format: &'static str,
    pub duration: Option<Duration>,
    pub gain_db: Option<f32>,
    /// YouTube's format id (251 = Opus ~160 kbps, 140 = AAC 128 kbps).
    pub itag: u32,
    /// Peak bitrate in bit/s, as the player says.
    pub bitrate: u32,
    /// Which of `player::ROUTES` gave the URL (`None`: RustyPipe's player).
    pub route: Option<usize>,
}

/// What an itag plays (Opus ~160, AAC ~128): a codec and its bitrate.
pub(crate) fn quality_of(itag: u32, opus: bool, bitrate: u32) -> nikgob_core::streaming::StreamQuality {
    let kbps = match itag {
        251 => 160,
        250 => 70,
        249 => 50,
        140 => 128,
        141 => 256,
        _ => (bitrate / 1000).max(1),
    };
    nikgob_core::streaming::StreamQuality::new(if opus { "Opus" } else { "AAC" }, kbps, true)
}

/// Gets the whole resource once it is downloaded (for the disk cache).
pub(crate) type CompleteFn = Box<dyn FnOnce(&Resolved, &mut dyn Read) + Send>;

/// Resolves (again) where the bytes are; with the resolve whose URL was
/// refused when there was one. Called on the download task.
pub(crate) type ResolveFn =
    Arc<dyn Fn(Option<Resolved>) -> BoxFuture<'static, Result<Resolved, StreamError>> + Send + Sync>;

/// Sorted, disjoint, non-adjacent byte ranges.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct RangeSet(Vec<Range<u64>>);

impl RangeSet {
    pub(crate) fn insert(&mut self, r: Range<u64>) {
        if r.is_empty() {
            return;
        }
        let (mut start, mut end) = (r.start, r.end);
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut placed = false;
        for x in self.0.drain(..) {
            if x.end < start {
                out.push(x);
            } else if x.start > end {
                if !placed {
                    out.push(start..end);
                    placed = true;
                }
                out.push(x);
            } else {
                start = start.min(x.start);
                end = end.max(x.end);
            }
        }
        if !placed {
            out.push(start..end);
        }
        self.0 = out;
    }

    /// End of the downloaded run holding `pos`, if `pos` is downloaded.
    pub(crate) fn run_end(&self, pos: u64) -> Option<u64> {
        self.0
            .iter()
            .find(|r| r.start <= pos && pos < r.end)
            .map(|r| r.end)
    }

    /// The first missing range at or after `from`, below `len`.
    pub(crate) fn gap_from(&self, from: u64, len: u64) -> Option<Range<u64>> {
        let mut start = from;
        for r in &self.0 {
            if r.end <= start {
                continue;
            }
            if r.start > start {
                return Some(start..r.start.min(len));
            }
            start = r.end;
        }
        (start < len).then_some(start..len)
    }
}

/// Stream stores alive and the bytes they hold in memory ([`buffers`]).
static BUFFERS: AtomicUsize = AtomicUsize::new(0);
static BUFFER_BYTES: AtomicU64 = AtomicU64::new(0);

/// How many stream stores are alive and the bytes they hold in memory (a
/// store grows with what is downloaded; a long resource lives in a
/// temporary file and takes none).
pub fn buffers() -> (usize, u64) {
    (BUFFERS.load(Ordering::Relaxed), BUFFER_BYTES.load(Ordering::Relaxed))
}

/// Bytes of one in-memory block: memory follows what is downloaded, not
/// the length the resolve claims.
const BLOCK: u64 = 256 * 1024;
/// A resource up to this long is kept in memory; a longer one (an hour-long
/// mix) goes to a temporary file, so memory stays small however long it is.
pub(crate) const MEMORY_MAX: u64 = 64 * 1024 * 1024;
/// A length beyond this is not an audio stream (a 12-hour recording at
/// 256 kbit/s is about 1.3 GB).
const MAX_LEN: u64 = 4 << 30;
/// Prefix of the temporary files.
const TEMP_PREFIX: &str = "nikgob-stream-";

/// Where a stream's downloaded bytes live: blocks in memory allocated as
/// bytes arrive, or a temporary file. Only ranges already in
/// [`State::have`] are ever read.
enum Store {
    /// Not resolved yet.
    None,
    /// Blocks by index; every block but the last is [`BLOCK`] long. Shared
    /// (not copied) with the disk cache's writer once complete: nothing
    /// writes into them any more.
    Memory {
        len: u64,
        blocks: BTreeMap<u64, Arc<Vec<u8>>>,
    },
    File(Arc<TempFile>),
}

impl Store {
    /// The store for a resource of `len` bytes.
    fn new(len: u64, memory_max: u64) -> io::Result<Self> {
        let store = if len > memory_max {
            Self::File(Arc::new(TempFile::create(len)?))
        } else {
            Self::Memory {
                len,
                blocks: BTreeMap::new(),
            }
        };
        BUFFERS.fetch_add(1, Ordering::Relaxed);
        Ok(store)
    }

    /// The temporary file, written to without holding the state's lock.
    fn file(&self) -> Option<Arc<TempFile>> {
        match self {
            Self::File(f) => Some(Arc::clone(f)),
            _ => None,
        }
    }

    /// Bytes held in memory.
    fn memory(&self) -> u64 {
        match self {
            Self::Memory { blocks, .. } => blocks.values().map(|b| b.len() as u64).sum(),
            _ => 0,
        }
    }

    /// Copies `bytes` in at `at` (memory only; a file is written by
    /// [`TempFile::write_at`]).
    fn write(&mut self, mut at: u64, mut bytes: &[u8]) {
        let Self::Memory { len, blocks } = self else {
            return;
        };
        let len = *len;
        while !bytes.is_empty() && at < len {
            let index = at / BLOCK;
            let block = blocks.entry(index).or_insert_with(|| {
                let size = BLOCK.min(len - index * BLOCK);
                BUFFER_BYTES.fetch_add(size, Ordering::Relaxed);
                Arc::new(vec![0; size as usize])
            });
            let off = (at % BLOCK) as usize;
            let n = bytes.len().min(block.len() - off);
            // Not shared while the download writes; were it shared, the
            // write would go into a copy of its own.
            Arc::make_mut(block)[off..off + n].copy_from_slice(&bytes[..n]);
            bytes = &bytes[n..];
            at += n as u64;
        }
    }

    /// Reads downloaded bytes at `at` into `buf` (at most up to the end of
    /// a block); how many.
    fn read(&self, at: u64, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::None => Ok(0),
            Self::Memory { blocks, .. } => {
                let Some(block) = blocks.get(&(at / BLOCK)) else {
                    return Err(io::Error::other("stream block missing"));
                };
                let off = (at % BLOCK) as usize;
                let n = buf.len().min(block.len().saturating_sub(off));
                buf[..n].copy_from_slice(&block[off..off + n]);
                Ok(n)
            }
            Self::File(f) => f.read_at(at, buf),
        }
    }

    /// The complete resource as a reader for the disk cache (the same
    /// blocks, or the same file).
    fn reader(&self) -> io::Result<Box<dyn Read + Send>> {
        match self {
            Self::None => Ok(Box::new(io::empty())),
            Self::Memory { blocks, .. } => Ok(Box::new(Blocks {
                blocks: blocks.values().cloned().collect(),
                at: (0, 0),
            })),
            Self::File(f) => Ok(Box::new(FileReader { file: Arc::clone(f), at: 0 })),
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        if !matches!(self, Self::None) {
            BUFFERS.fetch_sub(1, Ordering::Relaxed);
            BUFFER_BYTES.fetch_sub(self.memory(), Ordering::Relaxed);
        }
    }
}

/// A reader over complete blocks in order.
struct Blocks {
    blocks: Vec<Arc<Vec<u8>>>,
    /// (block, offset in it).
    at: (usize, usize),
}

impl Read for Blocks {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        while let Some(block) = self.blocks.get(self.at.0) {
            let rest = &block[self.at.1..];
            if rest.is_empty() {
                self.at = (self.at.0 + 1, 0);
                continue;
            }
            let n = rest.len().min(buf.len());
            buf[..n].copy_from_slice(&rest[..n]);
            self.at.1 += n;
            return Ok(n);
        }
        Ok(0)
    }
}

/// A temporary file of a long stream that the system removes when its last
/// handle closes (deleted at once on Unix, opened delete-on-close on
/// Windows): nothing is left after a crash, and no other run's file is ever
/// touched. Read and written at offsets, so the reader and the download
/// never share a cursor.
struct TempFile {
    file: std::fs::File,
    /// Its name while it had one (for the log and the tests).
    path: std::path::PathBuf,
}

impl TempFile {
    fn create(len: u64) -> io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let pid = std::process::id();
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{TEMP_PREFIX}{pid}-{n}.part"));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_DELETE_ON_CLOSE.
            options.custom_flags(0x0400_0000);
        }
        let file = options.open(&path)?;
        #[cfg(unix)]
        std::fs::remove_file(&path)?;
        log::debug!("youtube: {len} bytes stream kept in {}", path.display());
        Ok(Self { file, path })
    }

    fn write_at(&self, mut at: u64, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let n = positional::write(&self.file, bytes, at)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[n..];
            at += n as u64;
        }
        Ok(())
    }

    fn read_at(&self, at: u64, buf: &mut [u8]) -> io::Result<usize> {
        positional::read(&self.file, buf, at)
    }
}

/// A reader of a whole [`TempFile`] from its start, through the same handle.
struct FileReader {
    file: Arc<TempFile>,
    at: u64,
}

impl Read for FileReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read_at(self.at, buf)?;
        self.at += n as u64;
        Ok(n)
    }
}

#[cfg(unix)]
mod positional {
    use std::os::unix::fs::FileExt;
    pub(super) fn read(f: &std::fs::File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
        f.read_at(buf, at)
    }
    pub(super) fn write(f: &std::fs::File, buf: &[u8], at: u64) -> std::io::Result<usize> {
        f.write_at(buf, at)
    }
}

#[cfg(windows)]
mod positional {
    use std::os::windows::fs::FileExt;
    pub(super) fn read(f: &std::fs::File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
        f.seek_read(buf, at)
    }
    pub(super) fn write(f: &std::fs::File, buf: &[u8], at: u64) -> std::io::Result<usize> {
        f.seek_write(buf, at)
    }
}

struct State {
    resolved: Option<Resolved>,
    store: Store,
    have: RangeSet,
    /// Where the reader is (the next byte it wants).
    want: u64,
    /// A final failure; reads of bytes not downloaded return it.
    error: Option<StreamError>,
    closed: bool,
    /// The engine interrupted the reader (see [`StreamInterrupter`]): a
    /// read that would wait returns at once instead.
    interrupted: bool,
}

struct Shared {
    state: Mutex<State>,
    /// Signals the reader: bytes arrived, the stream resolved or failed.
    arrived: Condvar,
    /// Signals the task: the reader moved or went away.
    moved: Notify,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The reader side; see the module docs.
pub(crate) struct HttpStream {
    shared: Arc<Shared>,
    pos: u64,
    start: Mutex<Option<StartFn>>,
}

type StartFn = Box<dyn FnOnce(Arc<Shared>) + Send>;

impl HttpStream {
    /// Nothing runs until the stream is first read, seeked or asked about.
    pub(crate) fn new(
        runtime: Handle,
        http: reqwest::Client,
        resolve: ResolveFn,
        on_complete: Option<CompleteFn>,
    ) -> Self {
        Self::with_memory_max(runtime, http, resolve, on_complete, MEMORY_MAX)
    }

    /// [Self::new] keeping a resource longer than memory_max in a
    /// temporary file.
    pub(crate) fn with_memory_max(
        runtime: Handle,
        http: reqwest::Client,
        resolve: ResolveFn,
        on_complete: Option<CompleteFn>,
        memory_max: u64,
    ) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                resolved: None,
                store: Store::None,
                have: RangeSet::default(),
                want: 0,
                error: None,
                closed: false,
                interrupted: false,
            }),
            arrived: Condvar::new(),
            moved: Notify::new(),
        });
        Self {
            shared,
            pos: 0,
            start: Mutex::new(Some(Box::new(move |shared: Arc<Shared>| {
                runtime.spawn(download(shared, http, resolve, on_complete, memory_max));
            }))),
        }
    }

    fn ensure_started(&self) {
        let start = self
            .start
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(start) = start {
            start(Arc::clone(&self.shared));
        }
    }

    /// Waits until the stream is resolved; its info, or the failure.
    fn info(&self) -> io::Result<Resolved> {
        self.ensure_started();
        let mut st = self.shared.lock();
        loop {
            if let Some(r) = &st.resolved {
                return Ok(r.clone());
            }
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            st = wait(&self.shared, st)?;
        }
    }

    /// Info if already resolved; never blocks.
    fn info_now(&self) -> Option<Resolved> {
        self.shared.lock().resolved.clone()
    }
}

fn wait<'a>(shared: &'a Shared, st: MutexGuard<'a, State>) -> io::Result<MutexGuard<'a, State>> {
    if st.interrupted {
        return Err(interrupted_read());
    }
    let (st, timeout) = shared
        .arrived
        .wait_timeout(st, READ_TIMEOUT)
        .unwrap_or_else(PoisonError::into_inner);
    if timeout.timed_out() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "stream download stalled",
        ));
    }
    Ok(st)
}

impl Read for HttpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let len = self.info()?.len;
        if buf.is_empty() || self.pos >= len {
            return Ok(0);
        }
        let mut st = self.shared.lock();
        if st.want != self.pos {
            st.want = self.pos;
            self.shared.moved.notify_one();
        }
        loop {
            if let Some(end) = st.have.run_end(self.pos) {
                let n = buf
                    .len()
                    .min(usize::try_from(end - self.pos).unwrap_or(usize::MAX));
                let n = st.store.read(self.pos, &mut buf[..n])?;
                self.pos += n as u64;
                st.want = self.pos;
                return Ok(n);
            }
            if let Some(e) = &st.error {
                return Err(io::Error::other(e.clone()));
            }
            st = wait(&self.shared, st)?;
        }
    }
}

impl Seek for HttpStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.info()?.len.checked_add_signed(d),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before byte 0"))?;
        self.pos = target;
        Ok(target)
    }
}

impl AudioStream for HttpStream {
    fn byte_len(&self) -> Option<u64> {
        self.info_now().map(|r| r.len)
    }

    /// The engine asks this before the first read, so this is what starts
    /// the stream and waits for it to resolve (on the engine's opener
    /// thread). A failed resolve says WebM; the read then reports the error.
    fn format_hint(&self) -> &'static str {
        self.info().map_or(format::WEBM, |r| r.format)
    }

    fn gain_db(&self) -> Option<f32> {
        self.info_now().and_then(|r| r.gain_db)
    }

    fn duration(&self) -> Option<Duration> {
        self.info_now().and_then(|r| r.duration)
    }

    fn quality(&self) -> Option<nikgob_core::streaming::StreamQuality> {
        self.info_now()
            .map(|r| quality_of(r.itag, r.format == format::WEBM, r.bitrate))
    }

    /// Wakes a read waiting for its bytes (or for the resolve); the
    /// download goes on.
    fn interrupter(&self) -> Option<StreamInterrupter> {
        let shared = Arc::clone(&self.shared);
        Some(Arc::new(move |on| {
            shared.lock().interrupted = on;
            shared.arrived.notify_all();
        }))
    }
}

impl Drop for HttpStream {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.moved.notify_one();
    }
}

/// Why a request stopped.
enum Stop {
    /// The reader went away.
    Closed,
    /// The reader moved elsewhere; pick the next range again.
    Moved,
    /// The request range is done.
    Done,
}

enum Fail {
    /// The URL is refused or expired: resolve again.
    Expired,
    /// Worth retrying.
    Transient(String),
    /// Not worth retrying.
    Fatal(StreamError),
}

async fn download(
    shared: Arc<Shared>,
    http: reqwest::Client,
    resolve: ResolveFn,
    mut on_complete: Option<CompleteFn>,
    memory_max: u64,
) {
    let started = std::time::Instant::now();
    let mut resolved = match resolve(None).await {
        Ok(r) => r,
        Err(e) => return fail(&shared, "cannot resolve the stream", e),
    };
    log::debug!(
        "youtube: stream resolved in {} ms (itag {}, {} bytes)",
        started.elapsed().as_millis(),
        resolved.itag,
        resolved.len
    );
    if resolved.len > MAX_LEN {
        return fail(
            &shared,
            "resolved",
            StreamError::Parse(format!("stream length {} out of range", resolved.len)),
        );
    }
    let store = match Store::new(resolved.len, memory_max) {
        Ok(s) => s,
        Err(e) => {
            return fail(
                &shared,
                "temporary file",
                StreamError::Network(format!("cannot keep the stream: {e}")),
            );
        }
    };
    {
        let mut st = shared.lock();
        st.store = store;
        st.resolved = Some(resolved.clone());
    }
    shared.arrived.notify_all();

    let (mut attempts, mut resolves, mut first) = (0u32, 0u32, true);
    loop {
        // Arm the notification before looking, so a move in between is seen.
        let moved = shared.moved.notified();
        let next = {
            let st = shared.lock();
            if st.closed {
                return;
            }
            st.have
                .gap_from(st.want, resolved.len)
                .or_else(|| st.have.gap_from(0, resolved.len))
        };
        let Some(gap) = next else {
            // Everything is here: kept for next time, then nothing to do
            // but wait for the drop.
            if let Some(done) = on_complete.take() {
                log::debug!(
                    "youtube: stream downloaded in {} ms",
                    started.elapsed().as_millis()
                );
                // The same bytes, not a second copy of the resource.
                let reader = shared.lock().store.reader();
                let r = resolved.clone();
                match reader {
                    Ok(mut reader) => {
                        let _ = tokio::task::spawn_blocking(move || done(&r, &mut *reader)).await;
                    }
                    Err(e) => log::info!("youtube: cannot read the stream back to cache it: {e}"),
                }
            }
            moved.await;
            if shared.lock().closed {
                return;
            }
            continue;
        };
        let size = if first { FIRST_REQUEST } else { MAX_REQUEST };
        let range = gap.start..gap.end.min(gap.start + size);
        drop(moved);
        let before = progress(&shared);
        match fetch(&shared, &http, &resolved.url, range).await {
            Ok(Stop::Closed) => return,
            Ok(Stop::Moved | Stop::Done) => {
                if first {
                    log::debug!(
                        "youtube: first {} KB after {} ms",
                        progress(&shared) / 1024,
                        started.elapsed().as_millis()
                    );
                }
                first = false;
            }
            Err(Fail::Expired) => {
                resolves = if progress(&shared) > before {
                    1
                } else {
                    resolves + 1
                };
                if resolves > MAX_RESOLVES {
                    return fail(
                        &shared,
                        "stream URL keeps being refused",
                        StreamError::Http { status: 403 },
                    );
                }
                log::info!("youtube: stream URL refused, resolving again");
                if resolves > 1 {
                    tokio::time::sleep(RESOLVE_PAUSE * (resolves - 1)).await;
                }
                match resolve(Some(resolved.clone())).await {
                    Ok(r) if r.len == resolved.len => resolved = r,
                    Ok(_) => {
                        return fail(
                            &shared,
                            "re-resolved",
                            StreamError::Parse("stream changed size on re-resolve".into()),
                        );
                    }
                    Err(e) => return fail(&shared, "cannot resolve the stream", e),
                }
                continue;
            }
            Err(Fail::Transient(e)) => {
                attempts = if progress(&shared) > before {
                    1
                } else {
                    attempts + 1
                };
                if attempts >= MAX_ATTEMPTS {
                    return fail(&shared, "giving up", StreamError::Network(e));
                }
                log::debug!("youtube: stream request failed ({e}), retrying");
                tokio::time::sleep(Duration::from_millis(200 << attempts)).await;
                continue;
            }
            Err(Fail::Fatal(e)) => return fail(&shared, "stream request", e),
        }
        attempts = 0;
        resolves = 0;
    }
}

/// What a read of an [`HttpStream`] failed of: the stream's own failure,
/// else the I/O error as a network one.
pub(crate) fn stream_error(e: &io::Error) -> StreamError {
    e.get_ref()
        .and_then(|e| e.downcast_ref::<StreamError>())
        .cloned()
        .unwrap_or_else(|| StreamError::Network(e.to_string()))
}

/// Bytes downloaded so far.
fn progress(shared: &Shared) -> u64 {
    shared.lock().have.0.iter().map(|r| r.end - r.start).sum()
}

fn fail(shared: &Shared, what: &str, e: StreamError) {
    log::warn!("youtube: stream failed: {what}: {e}");
    shared.lock().error = Some(e);
    shared.arrived.notify_all();
}

/// One ranged request; its bytes go into the buffer as they arrive.
async fn fetch(
    shared: &Shared,
    http: &reqwest::Client,
    url: &str,
    range: Range<u64>,
) -> Result<Stop, Fail> {
    let resp = http
        .get(url)
        .header(
            reqwest::header::RANGE,
            format!("bytes={}-{}", range.start, range.end - 1),
        )
        .send()
        .await
        .map_err(|e| Fail::Transient(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    // A 200 is the whole resource: skip to the range.
    let mut skip = match status {
        206 => 0,
        200 => range.start,
        403 | 410 => return Err(Fail::Expired),
        408 | 429 | 500..=599 => return Err(Fail::Transient(format!("HTTP {status}"))),
        _ => return Err(Fail::Fatal(StreamError::Http { status })),
    };
    let mut resp = resp;
    let mut at = range.start;
    // A long stream's file is written without holding the lock (the reader
    // only reads what is already marked downloaded).
    let file = shared.lock().store.file();
    while at < range.end {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => return Err(Fail::Transient("response ended early".into())),
            Err(e) => return Err(Fail::Transient(e.without_url().to_string())),
        };
        let mut bytes = &chunk[..];
        if skip > 0 {
            let n = skip.min(bytes.len() as u64);
            skip -= n;
            bytes = &bytes[n as usize..];
        }
        let n = bytes.len().min((range.end - at) as usize);
        if n == 0 {
            continue;
        }
        if let Some(f) = &file {
            f.write_at(at, &bytes[..n]).map_err(|e| {
                Fail::Fatal(StreamError::Network(format!("cannot keep the stream: {e}")))
            })?;
        }
        let want = {
            let mut st = shared.lock();
            if st.closed {
                return Ok(Stop::Closed);
            }
            st.store.write(at, &bytes[..n]);
            st.have.insert(at..at + n as u64);
            st.want
        };
        shared.arrived.notify_all();
        at += n as u64;
        // The reader jumped ahead past this request or behind it into a gap.
        let st = shared.lock();
        if st.have.run_end(want).is_none() && !(at..at + SEEK_RESTART).contains(&want) {
            return Ok(Stop::Moved);
        }
    }
    Ok(Stop::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// What the test server does with the n-th request.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Answer {
        Ok,
        /// Sends half of the body, then closes.
        Truncate,
        /// 403 unless the URL is a re-resolved one.
        RefuseOldUrl,
        /// 403 whatever the URL.
        Refuse,
        NotFound,
    }

    /// `(path, start, end inclusive)` of every request.
    type Log = Arc<Mutex<Vec<(String, u64, u64)>>>;

    struct Server {
        port: u16,
        log: Log,
    }

    fn fixture(len: usize) -> Arc<Vec<u8>> {
        let mut x = 0x1234_5678_u32;
        Arc::new(
            (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect(),
        )
    }

    /// A tiny HTTP/1.1 server with `Range` support; `script` picks the
    /// answer for the n-th request (0-based).
    fn serve(data: Arc<Vec<u8>>, script: fn(usize) -> Answer) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Log::default();
        let log2 = Arc::clone(&log);
        std::thread::spawn(move || {
            for (n, conn) in listener.incoming().enumerate() {
                let Ok(conn) = conn else { return };
                let (data, log) = (Arc::clone(&data), Arc::clone(&log2));
                std::thread::spawn(move || answer(conn, &data, &log, script(n)));
            }
        });
        Server { port, log }
    }

    fn answer(mut conn: TcpStream, data: &[u8], log: &Log, answer: Answer) {
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let path = line.split(' ').nth(1).unwrap_or("").to_owned();
        let len = data.len() as u64;
        let (mut start, mut end) = (0, len - 1);
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                break;
            }
            if let Some(r) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                let (a, b) = r.trim().split_once('-').unwrap();
                start = a.parse().unwrap();
                end = b.parse::<u64>().unwrap_or(len - 1).min(len - 1);
            }
        }
        log.lock().unwrap().push((path.clone(), start, end));
        let refuse = answer == Answer::Refuse || (answer == Answer::RefuseOldUrl && path == "/v1");
        if refuse || answer == Answer::NotFound {
            let status = if refuse {
                "403 Forbidden"
            } else {
                "404 Not Found"
            };
            let _ = write!(
                conn,
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            return;
        }
        let body = &data[start as usize..=end as usize];
        let _ = write!(
            conn,
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\n\
             Content-Range: bytes {start}-{end}/{len}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let body = if answer == Answer::Truncate {
            &body[..body.len() / 2]
        } else {
            body
        };
        let _ = conn.write_all(body);
    }

    /// A stream over the test server (`port` 0: the resolve fails); every
    /// resolve bumps `resolves` and points at `/v<count>`.
    fn open(
        rt: &tokio::runtime::Runtime,
        port: u16,
        len: u64,
        resolves: &Arc<AtomicU32>,
    ) -> HttpStream {
        open_with(rt, port, len, resolves, None, MEMORY_MAX)
    }

    fn open_with(
        rt: &tokio::runtime::Runtime,
        port: u16,
        len: u64,
        resolves: &Arc<AtomicU32>,
        on_complete: Option<CompleteFn>,
        memory_max: u64,
    ) -> HttpStream {
        let resolves = Arc::clone(resolves);
        let own = Arc::new(AtomicU32::new(0));
        let resolve: ResolveFn = Arc::new(move |refused: Option<Resolved>| {
            let n = resolves.fetch_add(1, Ordering::SeqCst) + 1;
            let first = own.fetch_add(1, Ordering::SeqCst) == 0;
            Box::pin(async move {
                if port == 0 {
                    return Err(StreamError::Unavailable);
                }
                // A resolve again says which URL was refused: the previous
                // one.
                let previous = format!("http://127.0.0.1:{port}/v{}", n - 1);
                if refused.map(|r| r.url) != (!first).then_some(previous) {
                    return Err(StreamError::Parse("not told the refused URL".into()));
                }
                Ok(Resolved {
                    url: format!("http://127.0.0.1:{port}/v{n}"),
                    len,
                    format: format::M4A,
                    duration: Some(Duration::from_secs(3)),
                    gain_db: Some(-1.5),
                    itag: 140,
                    bitrate: 128_000,
                    route: None,
                })
            })
        });
        HttpStream::with_memory_max(
            rt.handle().clone(),
            reqwest::Client::new(),
            resolve,
            on_complete,
            memory_max,
        )
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    fn read_at(s: &mut HttpStream, pos: u64, n: usize) -> Vec<u8> {
        s.seek(SeekFrom::Start(pos)).unwrap();
        let mut buf = vec![0; n];
        s.read_exact(&mut buf).unwrap();
        buf
    }

    #[test]
    fn reads_seeks_survives_truncation_and_reresolves_on_403() {
        let data = fixture(3 * 1024 * 1024 + 123);
        let len = data.len() as u64;
        // 0: the small first request; 1: cut short; 2: the old URL refused.
        let server = serve(Arc::clone(&data), |n| match n {
            1 => Answer::Truncate,
            2 => Answer::RefuseOldUrl,
            _ => Answer::Ok,
        });
        let rt = runtime();
        let resolves = Arc::new(AtomicU32::new(0));
        let mut s = open(&rt, server.port, len, &resolves);
        assert_eq!(
            resolves.load(Ordering::SeqCst),
            0,
            "nothing before first use"
        );
        assert_eq!(s.format_hint(), format::M4A);
        assert_eq!(s.byte_len(), Some(len));
        assert_eq!(s.gain_db(), Some(-1.5));
        assert_eq!(s.duration(), Some(Duration::from_secs(3)));

        assert_eq!(read_at(&mut s, 0, 1000), data[..1000]);
        let far = 2_500_000;
        assert_eq!(
            read_at(&mut s, far, 10_000),
            data[far as usize..far as usize + 10_000]
        );
        s.seek(SeekFrom::End(-100)).unwrap();
        let mut tail = Vec::new();
        s.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, data[data.len() - 100..]);
        s.seek(SeekFrom::Start(0)).unwrap();
        let mut all = Vec::new();
        s.read_to_end(&mut all).unwrap();
        assert!(all == *data, "whole stream differs");
        let mut none = [0u8; 8];
        assert_eq!(s.read(&mut none).unwrap(), 0, "end of stream");

        let log = server.log.lock().unwrap().clone();
        assert_eq!(
            resolves.load(Ordering::SeqCst),
            2,
            "one re-resolve after the 403: {log:?}"
        );
        let (path, start, end) = &log[0];
        assert_eq!((path.as_str(), *start, *end), ("/v1", 0, FIRST_REQUEST - 1));
        assert!(log.iter().all(|(_, a, b)| b - a < MAX_REQUEST), "{log:?}");
        assert!(log.iter().skip(3).all(|(p, _, _)| p == "/v2"), "{log:?}");
    }

    #[test]
    fn failures_reach_the_reader() {
        let rt = runtime();
        let resolves = Arc::new(AtomicU32::new(0));
        // The resolve itself fails.
        let mut s = open(&rt, 0, 10, &resolves);
        assert_eq!(s.format_hint(), format::WEBM);
        assert!(s.read(&mut [0; 4]).is_err());
        assert_eq!(s.byte_len(), None);
        // The server answers 404.
        let server = serve(fixture(1000), |_| Answer::NotFound);
        let mut s = open(&rt, server.port, 1000, &resolves);
        let e = s.read(&mut [0; 4]).unwrap_err();
        assert_eq!(stream_error(&e), StreamError::Http { status: 404 });
        // Refused even after resolving again: a refusal, not a lost
        // connection.
        let server = serve(fixture(1000), |_| Answer::Refuse);
        let mut s = open(&rt, server.port, 1000, &resolves);
        let e = s.read(&mut [0; 4]).unwrap_err();
        assert_eq!(stream_error(&e), StreamError::Http { status: 403 });
        let other = io::Error::new(io::ErrorKind::TimedOut, "stream download stalled");
        assert!(matches!(stream_error(&other), StreamError::Network(_)));
        // Seeking before the start is refused.
        assert!(s.seek(SeekFrom::Current(-1)).is_err());
    }

    /// A read that waits for a resolve that never comes.
    fn waiting_read(s: HttpStream) -> std::sync::mpsc::Receiver<io::Result<usize>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut s = s;
            let _ = tx.send(s.read(&mut [0; 4]));
        });
        rx
    }

    #[test]
    fn an_interrupt_ends_a_waiting_read_at_once_until_it_is_lifted() {
        let rt = runtime();
        let never: ResolveFn = Arc::new(|_| Box::pin(std::future::pending()));
        let s = HttpStream::new(rt.handle().clone(), reqwest::Client::new(), never, None);
        let interrupt = s.interrupter().unwrap();
        let read = waiting_read(s);
        std::thread::sleep(Duration::from_millis(100));
        assert!(read.try_recv().is_err(), "the read waits");
        interrupt(true);
        let got = read.recv_timeout(Duration::from_secs(1)).expect("the read returns at once");
        assert!(got.is_err());

        // Lifted: reads wait for their bytes again.
        let never: ResolveFn = Arc::new(|_| Box::pin(std::future::pending()));
        let s = HttpStream::new(rt.handle().clone(), reqwest::Client::new(), never, None);
        let interrupt = s.interrupter().unwrap();
        interrupt(true);
        interrupt(false);
        let read = waiting_read(s);
        std::thread::sleep(Duration::from_millis(100));
        assert!(read.try_recv().is_err(), "the read waits");
        interrupt(true);
        assert!(read.recv_timeout(Duration::from_secs(1)).unwrap().is_err());

        // Bytes already there are read while interrupted.
        let data = fixture(100_000);
        let server = serve(Arc::clone(&data), |_| Answer::Ok);
        let resolves = Arc::new(AtomicU32::new(0));
        let mut s = open(&rt, server.port, data.len() as u64, &resolves);
        assert_eq!(read_at(&mut s, 0, 1000), data[..1000]);
        s.interrupter().unwrap()(true);
        assert_eq!(read_at(&mut s, 10, 100), data[10..110]);
    }

    #[test]
    fn memory_follows_downloaded_bytes_not_the_claimed_length() {
        // A resolve claiming 60 MB allocates nothing until bytes arrive.
        let mut s = Store::new(60 << 20, MEMORY_MAX).unwrap();
        assert_eq!(s.memory(), 0);
        s.write(BLOCK - 2, &[1, 2, 3, 4]);
        assert_eq!(s.memory(), 2 * BLOCK, "only the two blocks written");
        let mut buf = [0; 8];
        assert_eq!(s.read(BLOCK - 2, &mut buf).unwrap(), 2, "up to the block end");
        assert_eq!(&buf[..2], &[1, 2]);
        assert_eq!(s.read(BLOCK, &mut buf).unwrap(), 8);
        assert_eq!(&buf[..2], &[3, 4]);
        // The last block is only as long as the resource.
        let mut s = Store::new(BLOCK + 10, MEMORY_MAX).unwrap();
        s.write(BLOCK, &[9; 20]);
        assert_eq!(s.memory(), 10);
        // Longer than the memory limit: a file, nothing in memory.
        let s = Store::new(MEMORY_MAX + 1, MEMORY_MAX).unwrap();
        let path = s.file().unwrap().path.clone();
        // Nameless from the start on Unix; named until closed on Windows.
        assert_eq!(path.exists(), cfg!(windows));
        assert_eq!(s.memory(), 0);
        drop(s);
        assert!(!path.exists(), "the temporary file goes with the store");
        // Another running instance's file is never touched.
        let other = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-0.part", std::process::id() + 1));
        std::fs::write(&other, b"live").unwrap();
        drop(Store::new(MEMORY_MAX + 1, MEMORY_MAX).unwrap());
        assert!(other.exists());
        let _ = std::fs::remove_file(&other);
    }

    #[test]
    fn complete_blocks_read_back_in_order() {
        let mut s = Store::new(2 * BLOCK + 5, MEMORY_MAX).unwrap();
        let data = fixture((2 * BLOCK + 5) as usize);
        // Written out of order, as a seek makes the download do.
        s.write(BLOCK, &data[BLOCK as usize..]);
        s.write(0, &data[..BLOCK as usize]);
        let mut all = Vec::new();
        s.reader().unwrap().read_to_end(&mut all).unwrap();
        assert!(all == *data);
    }

    /// Reads a whole stream over the test server, keeping more than
    /// memory_max in a file; what the cache got.
    fn whole_stream(memory_max: u64) -> (Arc<Vec<u8>>, Vec<u8>, Vec<u8>) {
        let data = fixture(3 * 1024 * 1024 + 7);
        let len = data.len() as u64;
        let server = serve(Arc::clone(&data), |_| Answer::Ok);
        let rt = runtime();
        let resolves = Arc::new(AtomicU32::new(0));
        let cached = Arc::new(Mutex::new(Vec::new()));
        let cached2 = Arc::clone(&cached);
        let done: CompleteFn = Box::new(move |_r, bytes| {
            bytes.read_to_end(&mut cached2.lock().unwrap()).unwrap();
        });
        let mut s = open_with(&rt, server.port, len, &resolves, Some(done), memory_max);
        // A seek ahead first, so the file is written out of order.
        assert_eq!(read_at(&mut s, 2_000_000, 100), data[2_000_000..2_000_100]);
        s.seek(SeekFrom::Start(0)).unwrap();
        let mut all = Vec::new();
        s.read_to_end(&mut all).unwrap();
        let in_memory = s.shared.lock().store.memory();
        let file = s.shared.lock().store.file().map(|f| f.path.clone());
        if memory_max < len {
            assert_eq!(in_memory, 0, "a long stream takes no memory");
            assert!(file.is_some());
        } else {
            assert!(file.is_none() && in_memory == len);
        }
        // The cache gets the bytes once the download is done.
        for _ in 0..200 {
            if cached.lock().unwrap().len() as u64 == len {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(s);
        if let Some(p) = file {
            for _ in 0..100 {
                if !p.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(!p.exists(), "temporary file removed");
        }
        let cached = cached.lock().unwrap().clone();
        (data, all, cached)
    }

    #[test]
    fn a_long_stream_lives_in_a_temporary_file() {
        let (data, all, cached) = whole_stream(1024 * 1024);
        assert!(all == *data, "file-backed stream differs");
        assert!(cached == *data, "cache got other bytes");
        let (data, all, cached) = whole_stream(MEMORY_MAX);
        assert!(all == *data && cached == *data);
    }

    #[test]
    fn an_impossible_length_fails_without_allocating() {
        let rt = runtime();
        let resolves = Arc::new(AtomicU32::new(0));
        let server = serve(fixture(1000), |_| Answer::Ok);
        let mut s = open(&rt, server.port, MAX_LEN + 1, &resolves);
        let e = s.read(&mut [0; 4]).unwrap_err();
        assert!(matches!(stream_error(&e), StreamError::Parse(_)), "{e}");
    }

    #[test]
    fn range_set_merges_and_finds_gaps() {
        let mut s = RangeSet::default();
        s.insert(10..20);
        s.insert(30..40);
        s.insert(20..25);
        assert_eq!(s.0, vec![10..25, 30..40]);
        s.insert(5..32);
        assert_eq!(s.0, vec![5..40]);
        s.insert(50..60);
        assert_eq!(s.run_end(7), Some(40));
        assert_eq!(s.run_end(45), None);
        assert_eq!(s.gap_from(0, 100), Some(0..5));
        assert_eq!(s.gap_from(12, 100), Some(40..50));
        assert_eq!(s.gap_from(55, 100), Some(60..100));
        assert_eq!(s.gap_from(55, 60), None);
        s.insert(0..5);
        s.insert(40..50);
        s.insert(60..100);
        assert_eq!(s.0, vec![0..100]);
        assert_eq!(s.gap_from(0, 100), None);
    }
}
