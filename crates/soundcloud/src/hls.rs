//! SoundCloud's sound: AAC in HLS, as fragmented MP4 (an `init.mp4` and
//! about ten-second `dataNNN.m4s` segments), read by the audio engine as
//! one seekable MP4 file.
//!
//! **The file.** [`HlsStream`] lays the init segment and every media
//! segment end to end; Symphonia's MP4 reader plays such a fragmented file
//! (moov + moof/mdat pairs) as it is, and seeks in it by walking the moof
//! boxes up to the wanted time (checked with a real track on 2026-11-06:
//! the whole track decodes, seeks forward and back land on the sample).
//! Two things are patched on the way, without moving a byte:
//!
//! * each segment's `sidx` box is renamed `free`: the reader would take the
//!   first segment's index (ten seconds) for the length of the track;
//! * the init segment's `mdhd` duration (0 in fragmented files) is set to
//!   the playlist's total, so the track has its length from the start.
//!
//! **Downloading.** Nothing happens until the first read. Then a task on
//! the service's runtime resolves the stream (the track's transcoding to a
//! signed playlist url, then the playlist), fetches the init segment and
//! the first [`HEAD`] bytes of every segment (16 at a time: their sizes from
//! `Content-Range`, and their moof boxes, so a seek walks the boxes without
//! waiting for whole segments), and then whole segments in order from the
//! one being read, on to the end of a window of [`KEEP_AHEAD`] seconds.
//! Whole segments farther than [`KEEP_BEHIND`] seconds behind the reader
//! (or [`KEEP_AHEAD`] ahead of it) are dropped again, so a long track is
//! never all in memory; the heads stay (8 KB a segment), and a seek into
//! a dropped segment fetches it again. Reads block only while their bytes
//! are not there yet.
//!
//! **Expired urls.** The urls are signed for a limited time (CloudFront
//! `Policy`/`Signature`, about an hour). A 403 or 410 of a segment
//! resolves the stream again (a new playlist with new signatures; the
//! segments are the same files, so their sizes still hold) and continues
//! at the same segment; a resolve is also done ahead of time when the
//! playlist says it expires within a minute.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nikgob_core::streaming::{AudioStream, BoxFuture, StreamError, format};
use tokio::runtime::Handle;
use tokio::sync::{Notify, Semaphore};

/// Bytes of each segment fetched up front (styp, sidx and moof are about
/// 2 KB for ten seconds of AAC).
const HEAD: u64 = 8 * 1024;
/// Segment heads fetched at once.
const HEADS_IN_FLIGHT: usize = 16;
/// Attempts without progress before a network error is final.
const MAX_ATTEMPTS: u32 = 5;
/// Re-resolves without progress before a refusal is final.
const MAX_RESOLVES: u32 = 3;
/// Longest wait of a read for its bytes.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// A playlist expiring within this is resolved again before a segment.
const EXPIRY_MARGIN: u64 = 60;
/// Seconds of whole segments kept behind the reader, and downloaded and
/// kept ahead of it.
const KEEP_BEHIND: f64 = 60.0;
const KEEP_AHEAD: f64 = 60.0;
/// Rounds of retries of one segment head before the stream fails.
const HEAD_ROUNDS: u32 = 3;
/// How often the idle download looks at the reader again.
const IDLE_TICK: Duration = Duration::from_secs(2);
/// Whole segments are asked again this often while the network is gone,
/// and for this long at most.
const OUTAGE_RETRY: Duration = Duration::from_secs(5);
const OUTAGE_MAX: Duration = Duration::from_secs(120);
/// A media 429 asking to wait up to this long is waited out; a longer one
/// fails the stream at once (its `Retry-After` goes with the error).
const MEDIA_WAIT_MAX: Duration = Duration::from_secs(10);

/// The segments `[lo, hi]` kept around segment `want` (by their seconds).
fn window(durations: &[f64], want: usize) -> (usize, usize) {
    let last = durations.len().saturating_sub(1);
    let want = want.min(last);
    let mut lo = want;
    let mut sum = 0.0;
    while lo > 0 && sum + durations[lo - 1].max(0.0) <= KEEP_BEHIND {
        lo -= 1;
        sum += durations[lo].max(0.0);
    }
    let mut hi = want;
    let mut sum = durations.get(want).copied().unwrap_or(0.0).max(0.0);
    while hi < last && sum < KEEP_AHEAD {
        hi += 1;
        sum += durations[hi].max(0.0);
    }
    (lo, hi)
}

/// Where the stream's pieces are, as the playlist says.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Plan {
    pub(crate) init: String,
    /// Segment urls and their lengths in seconds.
    pub(crate) segments: Vec<(String, f64)>,
    /// Unix seconds the urls are signed until (`expires=`), if they say.
    pub(crate) expires: Option<u64>,
}

impl Plan {
    pub(crate) fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.segments.iter().map(|(_, d)| d.max(0.0)).sum())
    }
}

/// Parses a media playlist. An encrypted one (`#EXT-X-KEY` with a method)
/// is DRM: never played here.
pub(crate) fn parse_playlist(text: &str) -> Result<Plan, StreamError> {
    if !text.trim_start().starts_with("#EXTM3U") {
        return Err(StreamError::Parse("not an HLS playlist".into()));
    }
    let mut init = None;
    let mut segments = Vec::new();
    let mut pending: Option<f64> = None;
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("#EXT-X-KEY:") {
            if !rest.contains("METHOD=NONE") {
                return Err(StreamError::Drm);
            }
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MAP:") {
            init = attr(rest, "URI");
        } else if let Some(rest) = line.strip_prefix("#EXTINF:") {
            pending = rest.split(',').next().and_then(|d| d.trim().parse().ok());
        } else if !line.is_empty() && !line.starts_with('#') {
            segments.push((line.to_owned(), pending.take().unwrap_or(0.0)));
        }
    }
    let init = init.ok_or_else(|| StreamError::Parse("HLS playlist without an init segment".into()))?;
    if segments.is_empty() {
        return Err(StreamError::Parse("HLS playlist without segments".into()));
    }
    let expires = query_param(&init, "expires").and_then(|e| e.parse().ok());
    Ok(Plan { init, segments, expires })
}

/// `URI="…"` of an attribute list.
fn attr(list: &str, name: &str) -> Option<String> {
    let at = list.find(&format!("{name}="))? + name.len() + 1;
    let rest = &list[at..];
    if let Some(q) = rest.strip_prefix('"') {
        q.split('"').next().map(str::to_owned)
    } else {
        rest.split(',').next().map(str::to_owned)
    }
}

pub(crate) fn query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let (_, q) = url.split_once('?')?;
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

/// Renames the top-level `sidx` boxes before the first `moof` of a
/// segment's start to `free` (see the module docs). `buf` may be cut short.
pub(crate) fn patch_segment(buf: &mut [u8]) {
    let mut at = 0usize;
    while at + 8 <= buf.len() {
        let size = u32::from_be_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]) as usize;
        match &buf[at + 4..at + 8] {
            b"moof" | b"mdat" => return,
            b"sidx" => buf[at + 4..at + 8].copy_from_slice(b"free"),
            _ => {}
        }
        if size < 8 {
            return;
        }
        at += size;
    }
}

/// Sets the `mdhd` duration of an init segment to `total` (see the module
/// docs). Returns false when there is no such box.
pub(crate) fn patch_init(buf: &mut [u8], total: Duration) -> bool {
    let Some(i) = buf.windows(4).position(|w| w == b"mdhd") else { return false };
    let Some(&version) = buf.get(i + 4) else { return false };
    let read32 = |b: &[u8], at: usize| b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]));
    if version == 0 {
        let Some(scale) = read32(buf, i + 16) else { return false };
        let ticks = (total.as_secs_f64() * f64::from(scale)).round() as u64;
        let Some(slot) = buf.get_mut(i + 20..i + 24) else { return false };
        slot.copy_from_slice(&u32::try_from(ticks).unwrap_or(u32::MAX).to_be_bytes());
    } else {
        let Some(scale) = read32(buf, i + 24) else { return false };
        let ticks = (total.as_secs_f64() * f64::from(scale)).round() as u64;
        let Some(slot) = buf.get_mut(i + 28..i + 36) else { return false };
        slot.copy_from_slice(&ticks.to_be_bytes());
    }
    true
}

/// Resolves (again) where the stream is. Called on the download task.
pub(crate) type ResolveFn = Arc<dyn Fn() -> BoxFuture<'static, Result<Plan, StreamError>> + Send + Sync>;

struct Segment {
    /// Offset in the virtual file.
    start: u64,
    len: u64,
    /// The first bytes (patched).
    head: Vec<u8>,
    /// The whole segment (patched), once downloaded.
    data: Option<Vec<u8>>,
}

struct Layout {
    init: Vec<u8>,
    segments: Vec<Segment>,
    total: u64,
}

impl Layout {
    /// The segment holding `pos` (`None`: the init segment or past the end).
    fn segment_at(&self, pos: u64) -> Option<usize> {
        if pos < self.init.len() as u64 || pos >= self.total {
            return None;
        }
        let i = self.segments.partition_point(|s| s.start <= pos);
        i.checked_sub(1)
    }
}

#[derive(Default)]
struct State {
    layout: Option<Layout>,
    error: Option<StreamError>,
    /// The segment the reader waits for or reads.
    want: usize,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    ready: Condvar,
    wake: Notify,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn fail(&self, e: StreamError) {
        self.lock().error.get_or_insert(e);
        self.ready.notify_all();
    }
}

/// The [`AudioStream`] of one SoundCloud track (see the module docs).
pub(crate) struct HlsStream {
    shared: Arc<Shared>,
    pos: u64,
    started: bool,
    handle: Handle,
    http: reqwest::Client,
    resolve: Option<ResolveFn>,
    /// The bitrate of the stream the resolve picked.
    kbps: Arc<AtomicU64>,
    duration: Arc<Mutex<Option<Duration>>>,
}

impl HlsStream {
    pub(crate) fn new(handle: Handle, http: reqwest::Client, resolve: ResolveFn, kbps: Arc<AtomicU64>) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                ready: Condvar::new(),
                wake: Notify::new(),
            }),
            pos: 0,
            started: false,
            handle,
            http,
            resolve: Some(resolve),
            kbps,
            duration: Arc::new(Mutex::new(None)),
        }
    }

    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let Some(resolve) = self.resolve.take() else { return };
        let task = Download {
            shared: Arc::clone(&self.shared),
            http: self.http.clone(),
            resolve,
            duration: Arc::clone(&self.duration),
        };
        self.handle.spawn(task.run());
    }

    /// Waits until the layout is known (or the download failed).
    fn layout_wait(&mut self) -> io::Result<MutexGuard<'_, State>> {
        self.start();
        let shared = &self.shared;
        let mut st = shared.lock();
        let deadline = std::time::Instant::now() + READ_TIMEOUT;
        while st.layout.is_none() {
            if let Some(e) = &st.error {
                return Err(io_error(e.clone()));
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "SoundCloud: no answer"));
            }
            st = shared.ready.wait_timeout(st, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        Ok(st)
    }
}

impl Drop for HlsStream {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_one();
    }
}

fn io_error(e: StreamError) -> io::Error {
    io::Error::other(e)
}

impl Read for HlsStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let pos = self.pos;
        let deadline = std::time::Instant::now() + READ_TIMEOUT;
        let shared = Arc::clone(&self.shared);
        let mut st = self.layout_wait()?;
        loop {
            let layout = st.layout.as_ref().expect("waited for the layout");
            if pos >= layout.total {
                return Ok(0);
            }
            let init_len = layout.init.len() as u64;
            if pos < init_len {
                let from = &layout.init[pos as usize..];
                let n = from.len().min(buf.len());
                buf[..n].copy_from_slice(&from[..n]);
                drop(st);
                self.pos += n as u64;
                return Ok(n);
            }
            let i = layout.segment_at(pos).expect("inside the file");
            let seg = &layout.segments[i];
            let off = (pos - seg.start) as usize;
            let bytes = match &seg.data {
                Some(d) => Some(&d[..]),
                None if off < seg.head.len() => Some(&seg.head[..]),
                None => None,
            };
            if let Some(bytes) = bytes {
                let avail = (bytes.len().min(seg.len as usize)).saturating_sub(off);
                let n = avail.min(buf.len());
                buf[..n].copy_from_slice(&bytes[off..off + n]);
                // The download follows the reader along the track.
                let moved = st.want != i;
                if moved {
                    st.want = i;
                }
                drop(st);
                if moved {
                    shared.wake.notify_one();
                }
                self.pos += n as u64;
                return Ok(n);
            }
            if let Some(e) = &st.error {
                return Err(io_error(e.clone()));
            }
            if st.want != i {
                st.want = i;
                shared.wake.notify_one();
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "SoundCloud: the stream stalled"));
            }
            st = shared.ready.wait_timeout(st, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }
}

impl Seek for HlsStream {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let to = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => {
                let total = self.layout_wait()?.layout.as_ref().map_or(0, |l| l.total);
                total.checked_add_signed(d)
            }
        };
        let to = to.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        self.pos = to;
        // Tell the download where the reader went (a far seek).
        if self.started {
            let mut st = self.shared.lock();
            if let Some(i) = st.layout.as_ref().and_then(|l| l.segment_at(to))
                && st.want != i
            {
                st.want = i;
                drop(st);
                self.shared.wake.notify_one();
            }
        }
        Ok(to)
    }
}

impl AudioStream for HlsStream {
    fn byte_len(&self) -> Option<u64> {
        self.shared.lock().layout.as_ref().map(|l| l.total)
    }

    fn format_hint(&self) -> &'static str {
        format::M4A
    }

    fn duration(&self) -> Option<Duration> {
        *self.duration.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn quality(&self) -> Option<String> {
        let kbps = u32::try_from(self.kbps.load(Ordering::SeqCst)).unwrap_or(160);
        Some(nikgob_core::streaming::quality_label("AAC", kbps, false))
    }
}

/// The download task of one stream.
struct Download {
    shared: Arc<Shared>,
    http: reqwest::Client,
    resolve: ResolveFn,
    duration: Arc<Mutex<Option<Duration>>>,
}

/// What a GET of a piece gave.
enum Got {
    Bytes(Vec<u8>, Option<u64>),
    /// The signature expired or was refused (403/410).
    Refused,
}

/// A failure the network coming back can end (not a refusal or a gone file).
fn transient(e: &StreamError) -> bool {
    match e {
        StreamError::Network(_) => true,
        StreamError::Http { status } => *status >= 500,
        _ => false,
    }
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Download {
    async fn run(self) {
        if let Err(e) = self.download().await {
            log::warn!("soundcloud: stream failed: {e}");
            self.shared.fail(e);
        }
    }

    fn closed(&self) -> bool {
        self.shared.lock().closed
    }

    /// One GET (a `Range` when `range` is set), retried on transient errors
    /// (a 429 after its `Retry-After`, if it is short).
    async fn get(&self, url: &str, range: Option<u64>) -> Result<Got, StreamError> {
        let mut attempt = 0;
        loop {
            let mut wait = Duration::ZERO;
            let mut req = self.http.get(url);
            if let Some(n) = range {
                req = req.header("Range", format!("bytes=0-{}", n - 1));
            }
            let r = req.send().await;
            let err = match r {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    if status == 403 || status == 410 {
                        return Ok(Got::Refused);
                    }
                    if status == 404 {
                        return Err(StreamError::Unavailable);
                    }
                    if (200..300).contains(&status) {
                        // `Content-Range: bytes 0-8191/202075`
                        let total = resp
                            .headers()
                            .get("content-range")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| v.rsplit('/').next())
                            .and_then(|v| v.trim().parse::<u64>().ok());
                        match resp.bytes().await {
                            Ok(b) => {
                                let total = if status == 206 { total } else { Some(b.len() as u64) };
                                return Ok(Got::Bytes(b.to_vec(), total));
                            }
                            Err(e) => StreamError::Network(e.without_url().to_string()),
                        }
                    } else if status == 429 {
                        let retry_after = resp
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok()?.trim().parse::<u64>().ok())
                            .map(Duration::from_secs);
                        if retry_after.is_some_and(|w| w > MEDIA_WAIT_MAX) {
                            return Err(StreamError::RateLimited { retry_after });
                        }
                        wait = retry_after.unwrap_or_default();
                        StreamError::RateLimited { retry_after }
                    } else if status >= 500 {
                        StreamError::Http { status }
                    } else {
                        return Err(StreamError::Http { status });
                    }
                }
                Err(e) => StreamError::Network(e.without_url().to_string()),
            };
            attempt += 1;
            if attempt >= MAX_ATTEMPTS || self.closed() {
                return Err(err);
            }
            tokio::time::sleep(wait.max(Duration::from_millis(300 * u64::from(attempt)))).await;
        }
    }

    async fn download(&self) -> Result<(), StreamError> {
        let mut plan = (self.resolve)().await?;
        // A clock that runs ahead would make every url look expired: then
        // the expiry is ignored (a refused url still resolves again).
        let mut ignore_expiry = plan.expires.is_some_and(|e| e <= now_secs());
        let total_time = plan.duration();
        *self.duration.lock().unwrap_or_else(PoisonError::into_inner) = Some(total_time);
        let mut resolves = 0;

        // The init segment.
        let mut init = loop {
            match self.get(&plan.init, None).await? {
                Got::Bytes(b, _) => break b,
                Got::Refused => {
                    resolves += 1;
                    if resolves > MAX_RESOLVES {
                        return Err(StreamError::Http { status: 403 });
                    }
                    plan = (self.resolve)().await?;
                    ignore_expiry |= plan.expires.is_some_and(|e| e <= now_secs());
                }
            }
        };
        if !patch_init(&mut init, total_time) {
            log::debug!("soundcloud: init segment without mdhd");
        }

        // Every segment's head (and size), a few at a time. A long track
        // takes a while: urls about to expire, or refused during the pass,
        // resolve the stream again (the same segments) and the pass starts
        // over.
        let heads = loop {
            if !ignore_expiry && plan.expires.is_some_and(|e| e <= now_secs() + EXPIRY_MARGIN) {
                plan = self.resolve_again(&plan).await?;
                ignore_expiry = plan.expires.is_some_and(|e| e <= now_secs());
            }
            match self.heads(&plan).await? {
                Some(heads) => break heads,
                None => {
                    resolves += 1;
                    if resolves > MAX_RESOLVES {
                        return Err(StreamError::Http { status: 403 });
                    }
                    log::info!("soundcloud: segment heads refused, resolving the stream again");
                    plan = self.resolve_again(&plan).await?;
                    ignore_expiry |= plan.expires.is_some_and(|e| e <= now_secs());
                }
            }
        };
        let mut segments = Vec::with_capacity(heads.len());
        let mut at = init.len() as u64;
        for (head, len, whole) in heads {
            segments.push(Segment {
                start: at,
                len,
                data: whole.then(|| head.clone()),
                head,
            });
            at += len;
        }
        let count = segments.len();
        {
            let mut st = self.shared.lock();
            st.layout = Some(Layout { init, segments, total: at });
        }
        self.shared.ready.notify_all();

        // Whole segments, from the one being read to the end of the window;
        // the ones outside it are dropped.
        let durations: Vec<f64> = plan.segments.iter().map(|(_, d)| *d).collect();
        // Since when whole segments fail for want of a network.
        let mut outage: Option<std::time::Instant> = None;
        loop {
            if self.closed() {
                return Ok(());
            }
            let next = {
                let mut st = self.shared.lock();
                let want = st.want.min(count.saturating_sub(1));
                let (lo, hi) = window(&durations, want);
                let layout = st.layout.as_mut().expect("set above");
                for (i, seg) in layout.segments.iter_mut().enumerate() {
                    if (i < lo || i > hi) && seg.data.is_some() {
                        seg.data = None;
                    }
                }
                (want..=hi).find(|&i| layout.segments[i].data.is_none())
            };
            let Some(i) = next else {
                // Nothing to fetch: wait for the reader to move.
                let _ = tokio::time::timeout(IDLE_TICK, self.shared.wake.notified()).await;
                continue;
            };
            if !ignore_expiry && plan.expires.is_some_and(|e| e <= now_secs() + EXPIRY_MARGIN) {
                plan = self.resolve_again(&plan).await?;
                ignore_expiry = plan.expires.is_some_and(|e| e <= now_secs());
            }
            let got = match self.get(&plan.segments[i].0, None).await {
                Ok(got) => {
                    outage = None;
                    got
                }
                // The network went away (or the CDN is down) for longer than
                // one GET's retries: the reader still plays what is here, so
                // the segment is asked again, calmly, until the network is
                // back or the outage has lasted too long.
                Err(e) if transient(&e) => {
                    let since = *outage.get_or_insert_with(std::time::Instant::now);
                    if since.elapsed() >= OUTAGE_MAX {
                        return Err(e);
                    }
                    log::info!("soundcloud: segment {i} not fetched ({e}); asking again");
                    let _ = tokio::time::timeout(OUTAGE_RETRY, self.shared.wake.notified()).await;
                    continue;
                }
                Err(e) => return Err(e),
            };
            match got {
                Got::Bytes(mut b, _) => {
                    patch_segment(&mut b);
                    resolves = 0;
                    let mut st = self.shared.lock();
                    if let Some(seg) = st.layout.as_mut().and_then(|l| l.segments.get_mut(i)) {
                        if b.len() as u64 != seg.len {
                            // Not the file the layout was made of (another
                            // representation, a cut answer): no bytes are
                            // made up, the stream fails and the player
                            // goes on from here with another copy.
                            log::info!("soundcloud: segment {i} is {} bytes, {} expected", b.len(), seg.len);
                            return Err(StreamError::Parse("the stream changed while playing".into()));
                        }
                        seg.data = Some(b);
                    }
                    drop(st);
                    self.shared.ready.notify_all();
                }
                Got::Refused => {
                    resolves += 1;
                    if resolves > MAX_RESOLVES {
                        return Err(StreamError::Http { status: 403 });
                    }
                    log::info!("soundcloud: segment url refused, resolving the stream again");
                    plan = self.resolve_again(&plan).await?;
                    ignore_expiry |= plan.expires.is_some_and(|e| e <= now_secs());
                }
            }
        }
    }

    /// A new plan for the same stream (new signatures, same segments).
    async fn resolve_again(&self, old: &Plan) -> Result<Plan, StreamError> {
        let plan = (self.resolve)().await?;
        if plan.segments.len() != old.segments.len() {
            return Err(StreamError::Parse("the stream changed while playing".into()));
        }
        Ok(plan)
    }

    /// `(head, length, head is the whole segment)` of every segment;
    /// `None` when a url was refused (403/410: the signatures expired).
    async fn heads(&self, plan: &Plan) -> Result<Option<Vec<(Vec<u8>, u64, bool)>>, StreamError> {
        let limit = Arc::new(Semaphore::new(HEADS_IN_FLIGHT));
        let mut set = tokio::task::JoinSet::new();
        for (i, (url, _)) in plan.segments.iter().enumerate() {
            let (limit, http, url) = (Arc::clone(&limit), self.http.clone(), url.clone());
            let me = Download {
                shared: Arc::clone(&self.shared),
                http,
                resolve: Arc::clone(&self.resolve),
                duration: Arc::clone(&self.duration),
            };
            set.spawn(async move {
                let _turn = limit.acquire().await;
                // One head failing (after its own retries) is asked again
                // a few times before the stream fails.
                let mut last = StreamError::Network("stream closed".into());
                for round in 0..HEAD_ROUNDS {
                    if me.closed() {
                        break;
                    }
                    match me.get(&url, Some(HEAD)).await {
                        Ok(got) => return (i, Ok(got)),
                        // Asked to wait long: asking again makes it worse.
                        Err(e @ StreamError::RateLimited { .. }) => return (i, Err(e)),
                        Err(e) => last = e,
                    }
                    tokio::time::sleep(Duration::from_secs(u64::from(round) + 1)).await;
                }
                (i, Err(last))
            });
        }
        let mut out: Vec<Option<(Vec<u8>, u64, bool)>> = vec![None; plan.segments.len()];
        while let Some(joined) = set.join_next().await {
            if self.closed() {
                set.abort_all();
                return Err(StreamError::Network("stream closed".into()));
            }
            let (i, r) = joined.map_err(|e| StreamError::Network(format!("task failed: {e}")))?;
            match r? {
                Got::Bytes(mut b, total) => {
                    let len = total.unwrap_or(b.len() as u64);
                    let whole = b.len() as u64 >= len;
                    patch_segment(&mut b);
                    out[i] = Some((b, len, whole));
                }
                // The rest is not waited for: the stream resolves again.
                Got::Refused => {
                    set.abort_all();
                    return Ok(None);
                }
            }
        }
        Ok(Some(out.into_iter().map(|h| h.expect("every head joined")).collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYLIST: &str = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:10\n#EXT-X-PLAYLIST-TYPE:VOD\n\
#EXT-X-MAP:URI=\"https://cdn/aac_160k/x/init.mp4?expires=1793968669&Policy=p&Signature=s\"\n\
#EXTINF:10.007800,\nhttps://cdn/aac_160k/x/data000.m4s?expires=1793968669\n\
#EXTINF:2.589025,\nhttps://cdn/aac_160k/x/data001.m4s?expires=1793968669\n#EXT-X-ENDLIST\n";

    #[test]
    fn a_media_playlist_parses() {
        let p = parse_playlist(PLAYLIST).unwrap();
        assert_eq!(p.init, "https://cdn/aac_160k/x/init.mp4?expires=1793968669&Policy=p&Signature=s");
        assert_eq!(p.segments.len(), 2);
        assert_eq!(p.segments[1].1, 2.589025);
        assert_eq!(p.expires, Some(1_793_968_669));
        assert!((p.duration().as_secs_f64() - 12.596825).abs() < 1e-6);
        let drm = PLAYLIST.replace("#EXT-X-VERSION:7", "#EXT-X-KEY:METHOD=SAMPLE-AES-CTR,URI=\"skd://x\"");
        assert_eq!(parse_playlist(&drm), Err(StreamError::Drm));
        assert!(parse_playlist("<html>").is_err());
    }

    fn boxed(kind: &[u8; 4], size: u32) -> Vec<u8> {
        let mut b = size.to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.resize(size as usize, 0);
        b
    }

    #[test]
    fn segments_lose_their_index_and_the_init_gets_the_length() {
        let mut seg = [boxed(b"styp", 16), boxed(b"sidx", 44), boxed(b"moof", 24), boxed(b"sidx", 12)].concat();
        patch_segment(&mut seg);
        assert_eq!(&seg[20..24], b"free");
        assert_eq!(&seg[64..68], b"moof");
        assert_eq!(&seg[88..92], b"sidx", "after the moof: left alone");
        // A cut-short head is patched as far as it goes.
        let mut cut = seg[..30].to_vec();
        patch_segment(&mut cut);

        // mdhd v0: timescale 44100, duration 0.
        let mut mdhd = vec![0, 0, 0, 32];
        mdhd.extend_from_slice(b"mdhd");
        mdhd.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        mdhd.extend_from_slice(&44_100u32.to_be_bytes());
        mdhd.extend_from_slice(&0u32.to_be_bytes());
        mdhd.extend_from_slice(&[0, 0, 0, 0]);
        assert!(patch_init(&mut mdhd, Duration::from_secs_f64(322.606425)));
        assert_eq!(u32::from_be_bytes(mdhd[24..28].try_into().unwrap()), 14_226_943);
        assert!(!patch_init(&mut [0u8; 16], Duration::from_secs(1)));
    }

    #[test]
    fn the_window_keeps_a_minute_each_way() {
        let d = vec![10.0; 40];
        assert_eq!(window(&d, 20), (14, 25));
        assert_eq!(window(&d, 0), (0, 5));
        assert_eq!(window(&d, 39), (33, 39));
        assert_eq!(window(&[3.0], 5), (0, 0));
    }

    mod served {
        //! The download against a local server: refusals during the heads,
        //! a segment of another size, a media 429.
        use super::super::*;
        use std::sync::atomic::AtomicU32;
        use wiremock::matchers::{header_exists, method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        /// A stream whose resolves hand out signatures 1, 2, … in turn.
        fn stream(server: &MockServer, segments: usize, resolves: Arc<AtomicU32>) -> HlsStream {
            let plan_of = {
                let uri = server.uri();
                move |sig: u32| {
                    let base = uri.clone();
                    Plan {
                        init: format!("{base}/init.mp4?sig={sig}"),
                        segments: (0..segments).map(|i| (format!("{base}/data{i}.m4s?sig={sig}"), 10.0)).collect(),
                        expires: None,
                    }
                }
            };
            let resolve: ResolveFn = Arc::new(move || {
                let sig = resolves.fetch_add(1, Ordering::SeqCst) + 1;
                let plan = plan_of(sig);
                Box::pin(async move { Ok(plan) })
            });
            HlsStream::new(Handle::current(), reqwest::Client::new(), resolve, Arc::new(AtomicU64::new(160)))
        }

        async fn serve(server: &MockServer, url_path: &str, sig: u32, status: u16, body: Vec<u8>, total: Option<usize>) {
            let mut r = ResponseTemplate::new(status).set_body_bytes(body.clone());
            if let Some(t) = total {
                r = r.insert_header("Content-Range", format!("bytes 0-{}/{t}", body.len().saturating_sub(1)).as_str());
            }
            Mock::given(method("GET"))
                .and(path(url_path))
                .and(query_param("sig", sig.to_string().as_str()))
                .respond_with(r)
                .mount(server)
                .await;
        }

        fn read_all(mut s: HlsStream) -> std::io::Result<Vec<u8>> {
            let mut out = Vec::new();
            s.read_to_end(&mut out)?;
            Ok(out)
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn heads_refused_during_the_first_pass_resolve_again() {
            let server = MockServer::start().await;
            serve(&server, "/init.mp4", 1, 200, vec![1; 50], None).await;
            serve(&server, "/init.mp4", 2, 200, vec![1; 50], None).await;
            // The first signatures expired while the heads were asked for.
            for i in 0..2 {
                serve(&server, &format!("/data{i}.m4s"), 1, 403, Vec::new(), None).await;
                serve(&server, &format!("/data{i}.m4s"), 2, 206, vec![7 + i as u8; 100], Some(100)).await;
            }
            let resolves = Arc::new(AtomicU32::new(0));
            let s = stream(&server, 2, resolves.clone());
            let bytes = tokio::task::spawn_blocking(move || read_all(s)).await.unwrap().unwrap();
            assert_eq!(bytes.len(), 250);
            assert_eq!(&bytes[50..150], &[7u8; 100][..]);
            assert_eq!(&bytes[150..], &[8u8; 100][..]);
            assert_eq!(resolves.load(Ordering::SeqCst), 2, "one resolve again");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_segment_of_another_size_fails_instead_of_being_padded() {
            let server = MockServer::start().await;
            serve(&server, "/init.mp4", 1, 200, vec![1; 50], None).await;
            // The head says 10000 bytes; the whole segment comes as 9000.
            Mock::given(method("GET"))
                .and(path("/data0.m4s"))
                .and(header_exists("Range"))
                .respond_with(
                    ResponseTemplate::new(206)
                        .set_body_bytes(vec![3u8; HEAD as usize])
                        .insert_header("Content-Range", format!("bytes 0-{}/10000", HEAD - 1).as_str()),
                )
                .with_priority(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/data0.m4s"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![3u8; 9000]))
                .with_priority(2)
                .mount(&server)
                .await;
            let s = stream(&server, 1, Arc::new(AtomicU32::new(0)));
            let e = tokio::task::spawn_blocking(move || read_all(s)).await.unwrap().unwrap_err();
            assert!(e.to_string().contains("changed"), "{e}");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_segment_failing_for_a_while_is_asked_again_until_it_comes() {
            let server = MockServer::start().await;
            serve(&server, "/init.mp4", 1, 200, vec![1; 50], None).await;
            Mock::given(method("GET"))
                .and(path("/data0.m4s"))
                .and(header_exists("Range"))
                .respond_with(
                    ResponseTemplate::new(206)
                        .set_body_bytes(vec![3u8; HEAD as usize])
                        .insert_header("Content-Range", format!("bytes 0-{}/10000", HEAD - 1).as_str()),
                )
                .with_priority(1)
                .mount(&server)
                .await;
            // One GET's retries all fail (the network is gone), then it is back.
            Mock::given(method("GET"))
                .and(path("/data0.m4s"))
                .respond_with(ResponseTemplate::new(503))
                .up_to_n_times(MAX_ATTEMPTS as u64)
                .with_priority(2)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/data0.m4s"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![3u8; 10000]))
                .with_priority(3)
                .mount(&server)
                .await;
            let s = stream(&server, 1, Arc::new(AtomicU32::new(0)));
            let bytes = tokio::task::spawn_blocking(move || read_all(s)).await.unwrap().unwrap();
            assert_eq!(bytes.len(), 10_050);
        }

        #[test]
        fn only_network_failures_are_waited_out() {
            assert!(transient(&StreamError::Network("reset".into())));
            assert!(transient(&StreamError::Http { status: 503 }));
            assert!(!transient(&StreamError::Http { status: 404 }));
            assert!(!transient(&StreamError::Unavailable));
            assert!(!transient(&StreamError::RateLimited { retry_after: Some(Duration::from_secs(60)) }));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn a_long_media_retry_after_fails_at_once_with_its_time() {
            let server = MockServer::start().await;
            serve(&server, "/init.mp4", 1, 200, vec![1; 50], None).await;
            Mock::given(method("GET"))
                .and(path("/data0.m4s"))
                .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
                .expect(1)
                .mount(&server)
                .await;
            let s = stream(&server, 1, Arc::new(AtomicU32::new(0)));
            let started = std::time::Instant::now();
            let e = tokio::task::spawn_blocking(move || read_all(s)).await.unwrap().unwrap_err();
            assert!(started.elapsed() < Duration::from_secs(5), "no retries: {:?}", started.elapsed());
            let inner = e.into_inner().and_then(|b| b.downcast::<StreamError>().ok()).map(|b| *b);
            assert_eq!(inner, Some(StreamError::RateLimited { retry_after: Some(Duration::from_secs(120)) }));
        }
    }

    #[test]
    fn the_layout_finds_segments() {
        let seg = |start, len| Segment { start, len, head: Vec::new(), data: None };
        let l = Layout { init: vec![0; 100], segments: vec![seg(100, 50), seg(150, 50)], total: 200 };
        assert_eq!(l.segment_at(0), None);
        assert_eq!(l.segment_at(100), Some(0));
        assert_eq!(l.segment_at(149), Some(0));
        assert_eq!(l.segment_at(150), Some(1));
        assert_eq!(l.segment_at(199), Some(1));
        assert_eq!(l.segment_at(200), None);
    }
}
