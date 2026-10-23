//! A seekable reader over an HTTP resource fetched in ranged chunks.
//!
//! [`HttpStream`] is the [`AudioStream`] handed to the audio engine. Nothing
//! happens until it is first used; then a download task on the service's
//! runtime resolves the stream (the YouTube player request), and fetches the
//! bytes in `Range` requests: a small first one so playback starts quickly,
//! then up to [`MAX_REQUEST`] each, from the read position to the end and
//! then any gaps left before it. The whole resource is kept in memory, so
//! seeking back (the Matroska demuxer reopens from byte 0 to seek) costs
//! nothing.
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

use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use nikgob_core::streaming::{AudioStream, BoxFuture, StreamError, format};
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
/// Re-resolves without any progress before a 403 is final.
const MAX_RESOLVES: u32 = 3;
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

/// "Opus ~160 кбит/с", "AAC ~128 кбит/с": what an itag plays.
pub(crate) fn quality_of(itag: u32, opus: bool, bitrate: u32) -> String {
    let kbps = match itag {
        251 => 160,
        250 => 70,
        249 => 50,
        140 => 128,
        141 => 256,
        _ => (bitrate / 1000).max(1),
    };
    nikgob_core::streaming::quality_label(if opus { "Opus" } else { "AAC" }, kbps, true)
}

/// Gets the whole resource once it is downloaded (for the disk cache).
pub(crate) type CompleteFn = Box<dyn FnOnce(&Resolved, &[u8]) + Send>;

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

struct State {
    resolved: Option<Resolved>,
    data: Vec<u8>,
    have: RangeSet,
    /// Where the reader is (the next byte it wants).
    want: u64,
    /// A final failure; reads of bytes not downloaded return it.
    error: Option<StreamError>,
    closed: bool,
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
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                resolved: None,
                data: Vec::new(),
                have: RangeSet::default(),
                want: 0,
                error: None,
                closed: false,
            }),
            arrived: Condvar::new(),
            moved: Notify::new(),
        });
        Self {
            shared,
            pos: 0,
            start: Mutex::new(Some(Box::new(move |shared: Arc<Shared>| {
                runtime.spawn(download(shared, http, resolve, on_complete));
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
                let n = buf.len().min((end - self.pos) as usize);
                let at = self.pos as usize;
                buf[..n].copy_from_slice(&st.data[at..at + n]);
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

    fn quality(&self) -> Option<String> {
        self.info_now()
            .map(|r| quality_of(r.itag, r.format == format::WEBM, r.bitrate))
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
    {
        let mut st = shared.lock();
        st.data = vec![0; resolved.len as usize];
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
                let data = shared.lock().data.clone();
                let r = resolved.clone();
                let _ = tokio::task::spawn_blocking(move || done(&r, &data)).await;
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
        let want = {
            let mut st = shared.lock();
            if st.closed {
                return Ok(Stop::Closed);
            }
            st.data[at as usize..at as usize + n].copy_from_slice(&bytes[..n]);
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
        HttpStream::new(rt.handle().clone(), reqwest::Client::new(), resolve, None)
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
