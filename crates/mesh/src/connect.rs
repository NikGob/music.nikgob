//! Playing on another device, the network part: the runtime hands its
//! jobs here ([`ConnectJob`]) and gets what came of them back
//! ([`Command::ConnectReport`]).
//!
//! - Following a zone: its event stream (`GET /v1/events?zone=1`) through
//!   the tunnel, each state reported with when it was (its arrival less
//!   half the round trip, measured by a light request every half second
//!   that also tells whether the device still answers: «буферизация»
//!   after 1.5 s of silence, «нет связи» after 3 s, live again as soon as
//!   it answers).
//! - Forwarding: the commands of a remote, in order, as few requests as
//!   they come (`POST /v1/player/remote`; volume drags folded).
//! - Transfers and their undo: a zone's snapshot and its load, this
//!   device's through its API in this process.
//! - Files: a track this device has no file of, from the device that sent
//!   the queue, into `<data dir>/cache/remote/<blake3>.<ext>` (resumed
//!   with Range, checked by its hash, the least recently used dropped
//!   past 4 GB). Never into the library's folders.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use futures_util::StreamExt as _;
use nikgob_api::{Caller, LocalApi};
use nikgob_core::model::TrackId;
use nikgob_runtime::RuntimeHandle;
use nikgob_view::{ApiActor, ApiPreset, Command, ConnectJob, ConnectReport, RemoteCommand, RemoteLink, ZoneLoaded, ZoneSnapshot, ZoneState};
use parking_lot::Mutex;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{Semaphore, mpsc};

use crate::{MeshClient, MeshError};

/// Jobs waiting for the service (the runtime never waits on it).
const JOBS: usize = 256;
/// Forwarded commands waiting to be sent.
const RUNS: usize = 256;
/// At most this many commands in one forward (the API's own limit).
const REMOTE_MAX: usize = 64;
/// A zone's event stream is open within this.
const OPEN_WAIT: Duration = Duration::from_secs(6);
/// The light request that tells the device still answers, this often…
const PING_EVERY: Duration = Duration::from_millis(500);
/// …each given this long.
const PING_WAIT: Duration = Duration::from_secs(6);
/// How often the link's state is looked at.
const CHECK_EVERY: Duration = Duration::from_millis(250);
/// A broken event stream is opened again after this, doubling up to
/// [`RECONNECT_MAX`].
const RECONNECT_MIN: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(2);
/// Silence from the device this long: buffering…
const RECONNECTING_AFTER: Duration = Duration::from_millis(1500);
/// …and this long: no link.
const LOST_AFTER: Duration = Duration::from_secs(3);
/// A forward is answered within this.
const RUN_WAIT: Duration = Duration::from_secs(6);
/// A snapshot is answered within this.
const SNAPSHOT_WAIT: Duration = Duration::from_secs(8);
/// A load is answered within this (the API gives the player 10 s).
const LOAD_WAIT: Duration = Duration::from_secs(15);
/// A file's answer begins within this…
const PULL_HEAD_WAIT: Duration = Duration::from_secs(10);
/// …and never stops longer than this.
const PULL_STALL: Duration = Duration::from_secs(20);
/// Files fetched at once.
const PULLS_AT_ONCE: usize = 2;
/// A file larger than this is not fetched.
const FILE_MAX: u64 = 2 << 30;
/// The cache holds at most this much.
const CACHE_MAX: u64 = 4 << 30;
/// A part not written to for this long is a leftover (no fetch of it runs).
const PART_STALE: Duration = Duration::from_secs(3600);
/// A JSON answer longer than this is refused.
const JSON_MAX: usize = 16 << 20;
/// An event stream's line (or event) longer than this breaks it.
const SSE_MAX: usize = 4 << 20;

/// Aborts its task when dropped.
struct Abort(tokio::task::JoinHandle<()>);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What the service's tasks share.
#[derive(Clone)]
struct Ctx {
    client: MeshClient,
    /// This device's API in this process (its snapshot and load).
    api: Option<LocalApi>,
    runtime: RuntimeHandle,
    /// `<data dir>/cache/remote`.
    cache: PathBuf,
}

/// Starts the service on the mesh's runtime `rt`; the runtime hands it its
/// jobs from now on ([`RuntimeHandle::link_connect`]).
pub(crate) fn start(rt: &tokio::runtime::Handle, client: MeshClient, api: Option<LocalApi>, runtime: RuntimeHandle, data_dir: &Path) {
    let (tx, rx) = mpsc::channel::<ConnectJob>(JOBS);
    runtime.link_connect(move |job| {
        if tx.try_send(job).is_err() {
            log::warn!("connect: a job dropped (the mesh is busy or stopped)");
        }
    });
    let ctx = Ctx { client, api, runtime, cache: data_dir.join("cache").join("remote") };
    rt.spawn(dispatch(ctx, rx));
}

/// Russian words for a request to a device that failed, for a toast.
fn why(e: &MeshError) -> String {
    log::debug!("connect: {e}");
    match e {
        MeshError::Stopped => "Межсеть выключена".to_owned(),
        MeshError::NotPaired => "устройство не в кольце".to_owned(),
        MeshError::Unreachable(_) => "устройство недоступно".to_owned(),
        MeshError::Http(_) => "запрос не прошёл".to_owned(),
    }
}

/// A request with a JSON body or none.
fn request(method: http::Method, path: &str, json: Option<Vec<u8>>) -> Result<http::Request<Body>, String> {
    let b = http::Request::builder().method(method).uri(path);
    let r = match json {
        Some(bytes) => b.header(http::header::CONTENT_TYPE, "application/json").body(Body::from(bytes)),
        None => b.body(Body::empty()),
    };
    r.map_err(|e| e.to_string())
}

impl Ctx {
    fn report(&self, r: ConnectReport) {
        self.runtime.send(Command::ConnectReport(Box::new(r)));
    }

    /// `req` to the API of `node` (None: this device's, its caller naming
    /// `peer`, the device the files come from), answered within `wait`.
    async fn call(&self, node: Option<&str>, peer: Option<&str>, req: http::Request<Body>, wait: Duration) -> Result<http::Response<Body>, String> {
        match node {
            Some(n) => match tokio::time::timeout(wait, self.client.request(n, req)).await {
                Ok(Ok(r)) => Ok(r),
                Ok(Err(e)) => Err(why(&e)),
                Err(_) => Err("устройство не ответило".to_owned()),
            },
            None => {
                let api = self.api.as_ref().ok_or_else(|| "API этого устройства не запущен".to_owned())?;
                let caller = Caller { token_id: format!("connect:{}", peer.unwrap_or("-")), name: "connect".to_owned(), preset: ApiPreset::Full, actor: ApiActor::Owner };
                tokio::time::timeout(wait, api.call(caller, req)).await.map_err(|_| "плеер не ответил".to_owned())
            }
        }
    }

    /// [`Self::call`] whose answer is JSON; an error answer is its message.
    async fn json(&self, node: Option<&str>, peer: Option<&str>, req: http::Request<Body>, wait: Duration) -> Result<serde_json::Value, String> {
        let r = self.call(node, peer, req, wait).await?;
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), JSON_MAX).await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            let message = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned));
            return Err(message.unwrap_or_else(|| status.to_string()));
        }
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    }

    /// Runs `commands` on `node`'s zone.
    async fn run(&self, node: &str, commands: &[RemoteCommand]) -> Result<(), String> {
        let body = serde_json::to_vec(&serde_json::json!({ "commands": commands })).map_err(|e| e.to_string())?;
        let req = request(http::Method::POST, "/v1/player/remote", Some(body))?;
        self.json(Some(node), None, req, RUN_WAIT).await.map(|_| ())
    }

    /// Loads `snapshot` into `node`'s zone (None: this device's, its files
    /// from `peer`).
    async fn load(&self, node: Option<&str>, peer: Option<&str>, snapshot: &ZoneSnapshot) -> Result<ZoneLoaded, String> {
        let body = serde_json::to_vec(snapshot).map_err(|e| e.to_string())?;
        let req = request(http::Method::POST, "/v1/player/load", Some(body))?;
        let v = self.json(node, peer, req, LOAD_WAIT).await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }
}

async fn dispatch(ctx: Ctx, mut jobs: mpsc::Receiver<ConnectJob>) {
    let (run_tx, run_rx) = mpsc::channel::<(String, Vec<RemoteCommand>)>(RUNS);
    let _runner = Abort(tokio::spawn(runner(ctx.clone(), run_rx)));
    // Dropping it stops the zone it followed.
    let mut _follower: Option<Abort> = None;
    let pulls = Arc::new(Semaphore::new(PULLS_AT_ONCE));
    let pulling: Arc<Mutex<HashSet<TrackId>>> = Arc::default();
    while let Some(job) = jobs.recv().await {
        match job {
            // A new stream every time: its first event is the state now,
            // which the runtime waits for.
            ConnectJob::Follow { node } => _follower = Some(Abort(tokio::spawn(follow(ctx.clone(), node)))),
            ConnectJob::Unfollow => _follower = None,
            ConnectJob::Run { node, commands } => {
                if run_tx.try_send((node, commands)).is_err() {
                    log::warn!("connect: commands dropped (too many waiting)");
                }
            }
            ConnectJob::Move { seq, from, to } => {
                let c = ctx.clone();
                tokio::spawn(async move {
                    let result = transfer(&c, from.as_deref(), to.as_deref()).await;
                    c.report(ConnectReport::Moved { seq, to, result });
                });
            }
            ConnectJob::Load { seq, node, snapshot } => {
                let c = ctx.clone();
                tokio::spawn(async move {
                    let result = c.load(Some(&node), None, &snapshot).await;
                    c.report(ConnectReport::Loaded { seq, node, result });
                });
            }
            ConnectJob::Pull { track, from } => {
                if !pulling.lock().insert(track) {
                    continue;
                }
                let (c, pulls, pulling) = (ctx.clone(), pulls.clone(), pulling.clone());
                tokio::spawn(async move {
                    let result = match pulls.acquire_owned().await {
                        Ok(_permit) => pull(&c, track, &from).await,
                        Err(_) => Err("Межсеть выключена".to_owned()),
                    };
                    pulling.lock().remove(&track);
                    if let Err(e) = &result {
                        log::info!("connect: the file of {track} did not come: {e}");
                    }
                    c.report(ConnectReport::Pulled { track, result });
                });
            }
        }
    }
}

/// Moves `from`'s music (None: this device's) to `to`'s zone (None:
/// here). A remote zone it leaves is paused first (no double sound), and
/// plays again when the other could not take it.
async fn transfer(ctx: &Ctx, from: Option<&str>, to: Option<&str>) -> Result<ZoneLoaded, String> {
    let req = request(http::Method::GET, "/v1/player/snapshot", None)?;
    let snapshot: ZoneSnapshot = serde_json::from_value(ctx.json(from, to, req, SNAPSHOT_WAIT).await?).map_err(|e| e.to_string())?;
    let was_playing = !snapshot.paused;
    if let Some(f) = from
        && was_playing
        && let Err(e) = ctx.run(f, &[RemoteCommand::Pause]).await
    {
        log::info!("connect: the device the music leaves did not pause: {e}");
    }
    let loaded = ctx.load(to, from, &snapshot).await;
    if loaded.is_err()
        && was_playing
        && let Some(f) = from
        && let Err(e) = ctx.run(f, &[RemoteCommand::Play]).await
    {
        log::info!("connect: the device the music stayed on did not play again: {e}");
    }
    loaded
}

// ---------------------------------------------------------------------------
// Forwarding
// ---------------------------------------------------------------------------

/// Consecutive volume sets, and seeks, folded into their last.
pub(crate) fn collapse(commands: Vec<RemoteCommand>) -> Vec<RemoteCommand> {
    let mut out: Vec<RemoteCommand> = Vec::with_capacity(commands.len());
    for c in commands {
        let same = matches!(
            (out.last(), &c),
            (Some(RemoteCommand::SetVolume { .. }), RemoteCommand::SetVolume { .. }) | (Some(RemoteCommand::Seek { .. }), RemoteCommand::Seek { .. })
        );
        if same {
            out.pop();
        }
        out.push(c);
    }
    out
}

/// The commands waiting, in order, one run per device in a row (each
/// folded).
pub(crate) fn group(batch: Vec<(String, Vec<RemoteCommand>)>) -> Vec<(String, Vec<RemoteCommand>)> {
    let mut out: Vec<(String, Vec<RemoteCommand>)> = Vec::new();
    for (node, commands) in batch {
        match out.last_mut() {
            Some((last, list)) if *last == node => list.extend(commands),
            _ => out.push((node, commands)),
        }
    }
    out.into_iter().map(|(node, list)| (node, collapse(list))).collect()
}

/// Sends the forwarded commands in order: what came while a request was
/// on its way goes in the next one.
async fn runner(ctx: Ctx, mut rx: mpsc::Receiver<(String, Vec<RemoteCommand>)>) {
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < RUNS
            && let Ok(more) = rx.try_recv()
        {
            batch.push(more);
        }
        for (node, commands) in group(batch) {
            for chunk in commands.chunks(REMOTE_MAX) {
                if let Err(why) = ctx.run(&node, chunk).await {
                    ctx.report(ConnectReport::RunFailed { node: node.clone(), why });
                    break;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Following a zone
// ---------------------------------------------------------------------------

/// The events of a `text/event-stream` as its bytes come.
#[derive(Default)]
pub(crate) struct Sse {
    line: Vec<u8>,
    event: String,
    data: String,
    has_data: bool,
}

impl Sse {
    /// The events `bytes` completes, as (name, data); `None` when a line
    /// or an event is longer than [`SSE_MAX`] (the stream is broken).
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Option<Vec<(String, String)>> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' {
                let mut line = std::mem::take(&mut self.line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.line_done(&line, &mut out);
                if self.data.len() > SSE_MAX {
                    return None;
                }
            } else {
                if self.line.len() >= SSE_MAX {
                    return None;
                }
                self.line.push(b);
            }
        }
        Some(out)
    }

    fn line_done(&mut self, line: &[u8], out: &mut Vec<(String, String)>) {
        if line.is_empty() {
            if self.has_data {
                let name = if self.event.is_empty() { "message".to_owned() } else { std::mem::take(&mut self.event) };
                out.push((name, std::mem::take(&mut self.data)));
            }
            self.event.clear();
            self.data.clear();
            self.has_data = false;
            return;
        }
        // A comment (`: keepalive`).
        if line[0] == b':' {
            return;
        }
        let text = String::from_utf8_lossy(line);
        let (field, value) = match text.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (&*text, ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            _ => {}
        }
    }
}

/// The link's state from when the device last answered.
pub(crate) struct Link {
    last_ok: Instant,
    told: RemoteLink,
}

impl Link {
    pub(crate) fn new(now: Instant) -> Self {
        Self { last_ok: now, told: RemoteLink::Live }
    }

    pub(crate) fn ok(&mut self, now: Instant) {
        self.last_ok = now;
    }

    /// The link's state now, when it is not what was told last.
    pub(crate) fn check(&mut self, now: Instant) -> Option<RemoteLink> {
        let silent = now.saturating_duration_since(self.last_ok);
        let link = if silent >= LOST_AFTER {
            RemoteLink::Lost
        } else if silent >= RECONNECTING_AFTER {
            RemoteLink::Reconnecting
        } else {
            RemoteLink::Live
        };
        (link != self.told).then(|| {
            self.told = link;
            link
        })
    }
}

/// The light request, each round trip it took sent on `tx`.
async fn ping(ctx: Ctx, node: String, tx: mpsc::Sender<Duration>) {
    let mut every = tokio::time::interval(PING_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        let Ok(req) = request(http::Method::GET, "/v1/token", None) else { return };
        let t = Instant::now();
        if let Ok(r) = ctx.call(Some(&node), None, req, PING_WAIT).await
            && r.status().is_success()
        {
            let rtt = t.elapsed();
            drop(r);
            if tx.send(rtt).await.is_err() {
                return;
            }
        }
    }
}

type Opening = std::pin::Pin<Box<dyn std::future::Future<Output = Result<http::Response<Body>, String>> + Send>>;

async fn opened(f: &mut Option<Opening>) -> Result<http::Response<Body>, String> {
    match f {
        Some(f) => f.await,
        None => std::future::pending().await,
    }
}

async fn chunk(s: &mut Option<axum::body::BodyDataStream>) -> Option<Result<axum::body::Bytes, axum::Error>> {
    match s {
        Some(s) => s.next().await,
        None => std::future::pending().await,
    }
}

/// Follows `node`'s zone until aborted: its states and its link to the
/// runtime. It never sends the device anything but reads.
async fn follow(ctx: Ctx, node: String) {
    let (rtt_tx, mut rtt_rx) = mpsc::channel::<Duration>(8);
    let _pinger = Abort(tokio::spawn(ping(ctx.clone(), node.clone(), rtt_tx)));
    let mut link = Link::new(Instant::now());
    let mut rtt: Option<Duration> = None;
    let mut check = tokio::time::interval(CHECK_EVERY);
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut opening: Option<Opening> = None;
    let mut stream: Option<axum::body::BodyDataStream> = None;
    let mut sse = Sse::default();
    let mut backoff = RECONNECT_MIN;
    let mut retry_at = tokio::time::Instant::now();
    loop {
        if stream.is_none() && opening.is_none() && tokio::time::Instant::now() >= retry_at {
            let (c, n) = (ctx.clone(), node.clone());
            opening = Some(Box::pin(async move {
                let req = request(http::Method::GET, "/v1/events?zone=1", None)?;
                c.call(Some(&n), None, req, OPEN_WAIT).await
            }));
        }
        tokio::select! {
            r = opened(&mut opening) => {
                opening = None;
                match r {
                    Ok(resp) if resp.status().is_success() => {
                        stream = Some(resp.into_body().into_data_stream());
                        sse = Sse::default();
                    }
                    other => {
                        let e = other.map(|r| r.status().to_string()).unwrap_or_else(|e| e);
                        log::debug!("connect: the zone's events did not open: {e}");
                        retry_at = tokio::time::Instant::now() + backoff;
                        backoff = (backoff * 2).min(RECONNECT_MAX);
                    }
                }
            }
            c = chunk(&mut stream) => match c {
                Some(Ok(bytes)) => match sse.push(&bytes) {
                    Some(events) => {
                        // Anything at all (a keep-alive too) says it answers.
                        link.ok(Instant::now());
                        backoff = RECONNECT_MIN;
                        for (name, data) in events {
                            if name != "zone" {
                                continue;
                            }
                            match serde_json::from_str::<ZoneState>(&data) {
                                Ok(state) => {
                                    let half = rtt.unwrap_or_default() / 2;
                                    let at = Instant::now().checked_sub(half).unwrap_or_else(Instant::now);
                                    ctx.report(ConnectReport::State { node: node.clone(), state: Box::new(state), at });
                                }
                                Err(e) => log::debug!("connect: a zone event not understood: {e}"),
                            }
                        }
                    }
                    None => {
                        log::debug!("connect: the zone's events broke (too long)");
                        stream = None;
                    }
                },
                Some(Err(e)) => {
                    log::debug!("connect: the zone's events ended: {e}");
                    stream = None;
                    retry_at = tokio::time::Instant::now() + backoff;
                }
                None => {
                    stream = None;
                    retry_at = tokio::time::Instant::now() + backoff;
                }
            },
            Some(took) = rtt_rx.recv() => {
                link.ok(Instant::now());
                let took = took.min(Duration::from_secs(2));
                rtt = Some(match rtt {
                    Some(r) => (r * 7 + took * 3) / 10,
                    None => took,
                });
            }
            _ = check.tick() => {}
        }
        if let Some(l) = link.check(Instant::now()) {
            ctx.report(ConnectReport::Link { node: node.clone(), link: l });
        }
    }
}

// ---------------------------------------------------------------------------
// Files
// ---------------------------------------------------------------------------

/// A hash as the media route gives it: 64 hex digits, lowercase.
pub(crate) fn clean_hash(h: &str) -> Option<String> {
    let h = h.trim().to_ascii_lowercase();
    (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then_some(h)
}

/// An extension as the media route gives it: a few letters and digits.
pub(crate) fn clean_ext(e: &str) -> String {
    let e = e.trim().to_ascii_lowercase();
    if !e.is_empty() && e.len() <= 8 && e.bytes().all(|b| b.is_ascii_alphanumeric()) { e } else { "bin".to_owned() }
}

fn header<'a>(r: &'a http::Response<Body>, name: &str) -> Option<&'a str> {
    r.headers().get(name).and_then(|v| v.to_str().ok())
}

/// The files to delete so `files` (path, size, last used) hold at most
/// `cap` bytes: the least recently used first, never `keep`.
pub(crate) fn evict_plan(mut files: Vec<(PathBuf, u64, SystemTime)>, cap: u64, keep: &Path) -> Vec<PathBuf> {
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.2);
    let mut out = Vec::new();
    for (path, size, _) in files {
        if total <= cap {
            break;
        }
        if path == keep {
            continue;
        }
        total = total.saturating_sub(size);
        out.push(path);
    }
    out
}

/// Keeps the cache at `dir` within [`CACHE_MAX`] (`keep` stays): its
/// files, and parts nobody writes any more.
fn evict(dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    let mut files = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let Ok(m) = e.metadata() else { continue };
        if !m.is_file() {
            continue;
        }
        let used = m.modified().unwrap_or(now);
        match path.extension().and_then(|x| x.to_str()) {
            Some("meta") => continue,
            Some("part") if now.duration_since(used).unwrap_or_default() < PART_STALE => continue,
            _ => files.push((path, m.len(), used)),
        }
    }
    for path in evict_plan(files, CACHE_MAX, keep) {
        match std::fs::remove_file(&path) {
            Ok(()) => {
                if path.extension().is_some_and(|x| x == "part") {
                    let _ = std::fs::remove_file(path.with_extension("meta"));
                }
            }
            // In use (playing, on Windows): next time.
            Err(e) => log::debug!("connect: {} stays in the cache: {e}", path.display()),
        }
    }
}

/// Marks `path` used now (the cache drops the least recently used).
fn touch(path: &Path) {
    if let Err(e) = std::fs::File::options().append(true).open(path).and_then(|f| f.set_modified(SystemTime::now())) {
        log::debug!("connect: {} not touched: {e}", path.display());
    }
}

/// The BLAKE3 (hex) of the file at `path`.
fn blake3_of(path: &Path) -> std::io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(std::fs::File::open(path)?)?;
    Ok(h.finalize().to_hex().to_string())
}

/// Writes `resp`'s body to `part` (after its first `offset` bytes, or over
/// it when `offset` is 0); how long the part is then.
pub(crate) async fn receive(resp: http::Response<Body>, part: &Path, offset: u64) -> Result<u64, String> {
    let io = |e: std::io::Error| format!("кэш: {e}");
    let mut file = if offset == 0 {
        tokio::fs::File::create(part).await.map_err(io)?
    } else {
        tokio::fs::OpenOptions::new().append(true).open(part).await.map_err(io)?
    };
    let mut len = offset;
    let mut body = resp.into_body().into_data_stream();
    loop {
        match tokio::time::timeout(PULL_STALL, body.next()).await {
            Ok(Some(Ok(bytes))) => {
                len += bytes.len() as u64;
                if len > FILE_MAX {
                    return Err("файл слишком большой".to_owned());
                }
                file.write_all(&bytes).await.map_err(io)?;
            }
            Ok(Some(Err(e))) => {
                file.flush().await.map_err(io)?;
                return Err(format!("передача оборвалась: {e}"));
            }
            Ok(None) => break,
            Err(_) => {
                file.flush().await.map_err(io)?;
                return Err("передача остановилась".to_owned());
            }
        }
    }
    file.flush().await.map_err(io)?;
    file.sync_all().await.map_err(io)?;
    Ok(len)
}

/// The fetched `part` checked against `hash` and kept as `done`; a part
/// that does not match is deleted.
pub(crate) async fn finish(part: &Path, hash: &str, done: &Path) -> Result<(), String> {
    let p = part.to_owned();
    let sum = tokio::task::spawn_blocking(move || blake3_of(&p)).await.map_err(|e| e.to_string())?.map_err(|e| format!("кэш: {e}"))?;
    if sum != hash {
        let _ = tokio::fs::remove_file(part).await;
        let _ = tokio::fs::remove_file(part.with_extension("meta")).await;
        return Err("файл пришёл повреждённым".to_owned());
    }
    tokio::fs::rename(part, done).await.map_err(|e| format!("кэш: {e}"))?;
    let _ = tokio::fs::remove_file(part.with_extension("meta")).await;
    Ok(())
}

/// `track`'s file from device `from` into the cache (or the copy kept
/// there); its path.
async fn pull(ctx: &Ctx, track: TrackId, from: &str) -> Result<PathBuf, String> {
    let dir = ctx.cache.clone();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("кэш: {e}"))?;
    let part = dir.join(format!("{track}.part"));
    let meta = part.with_extension("meta");
    // A part of an earlier fetch goes on where it stopped, if the file
    // there is still the one it was (its hash, kept beside it).
    let mut expected: Option<String> = tokio::fs::read_to_string(&meta).await.ok().and_then(|s| clean_hash(&s));
    let mut offset = match expected {
        Some(_) => tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0),
        None => 0,
    };
    for _ in 0..3 {
        let path = format!("/v1/media/tracks/{track}/audio?hash=1");
        let mut b = http::Request::builder().method(http::Method::GET).uri(path);
        if offset > 0 {
            b = b.header(http::header::RANGE, format!("bytes={offset}-"));
        }
        let req = b.body(Body::empty()).map_err(|e| e.to_string())?;
        let resp = ctx.call(Some(from), None, req, PULL_HEAD_WAIT).await?;
        let status = resp.status();
        if status == http::StatusCode::NOT_FOUND {
            return Err("на том устройстве нет файла этой песни".to_owned());
        }
        if status == http::StatusCode::RANGE_NOT_SATISFIABLE {
            // The part is no part of the file there: from the start.
            let _ = tokio::fs::remove_file(&part).await;
            (offset, expected) = (0, None);
            continue;
        }
        if !status.is_success() {
            return Err(format!("ответ {status}"));
        }
        let hash = header(&resp, nikgob_api::HASH_HEADER).and_then(clean_hash).ok_or_else(|| "тот плеер не сказал хэш файла".to_owned())?;
        let ext = clean_ext(header(&resp, nikgob_api::EXT_HEADER).unwrap_or(""));
        let done = dir.join(format!("{hash}.{ext}"));
        if tokio::fs::try_exists(&done).await.unwrap_or(false) {
            drop(resp);
            let (d, p) = (done.clone(), part.clone());
            let _ = tokio::task::spawn_blocking(move || {
                touch(&d);
                let _ = std::fs::remove_file(&p);
                let _ = std::fs::remove_file(p.with_extension("meta"));
            })
            .await;
            return Ok(done);
        }
        if expected.as_deref().is_some_and(|h| h != hash) {
            // The file there changed since the part began.
            let _ = tokio::fs::remove_file(&part).await;
            (offset, expected) = (0, None);
            continue;
        }
        let length = resp.headers().get(http::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
        // A whole file sent for a Range: over the part.
        let from_byte = if status == http::StatusCode::PARTIAL_CONTENT { offset } else { 0 };
        if length.is_some_and(|l| l.saturating_add(from_byte) > FILE_MAX) {
            return Err("файл слишком большой".to_owned());
        }
        if expected.is_none() {
            tokio::fs::write(&meta, &hash).await.map_err(|e| format!("кэш: {e}"))?;
            expected = Some(hash.clone());
        }
        receive(resp, &part, from_byte).await?;
        finish(&part, &hash, &done).await?;
        let (d, keep) = (dir.clone(), done.clone());
        let _ = tokio::task::spawn_blocking(move || evict(&d, &keep)).await;
        return Ok(done);
    }
    Err("файл на том устройстве всё время меняется".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_stream_is_read_as_it_comes() {
        let mut s = Sse::default();
        assert!(s.push(b"event: zone\nid: 3\ndata: {\"a\":").unwrap().is_empty());
        assert_eq!(s.push(b"1}\n\n: keepalive\n\n").unwrap(), vec![("zone".to_owned(), "{\"a\":1}".to_owned())]);
        assert_eq!(s.push(b"data: x\r\ndata: y\r\n\r\n").unwrap(), vec![("message".to_owned(), "x\ny".to_owned())]);
        assert!(s.push(b"event: state\n\n").unwrap().is_empty(), "no data, no event");
        let long = vec![b'a'; SSE_MAX + 1];
        assert!(Sse::default().push(&long).is_none());
    }

    #[test]
    fn volume_drags_fold_into_their_last() {
        use RemoteCommand::*;
        let c = collapse(vec![SetVolume { volume: 0.1 }, SetVolume { volume: 0.2 }, Pause, SetVolume { volume: 0.3 }, Seek { ms: 1 }, Seek { ms: 2 }, Next]);
        assert_eq!(c, vec![SetVolume { volume: 0.2 }, Pause, SetVolume { volume: 0.3 }, Seek { ms: 2 }, Next]);
        let g = group(vec![
            ("a".into(), vec![SetVolume { volume: 0.1 }]),
            ("a".into(), vec![SetVolume { volume: 0.5 }]),
            ("b".into(), vec![Pause]),
            ("a".into(), vec![Next]),
        ]);
        assert_eq!(g, vec![("a".into(), vec![SetVolume { volume: 0.5 }]), ("b".into(), vec![Pause]), ("a".into(), vec![Next])]);
    }

    #[test]
    fn the_link_goes_by_the_silence() {
        let t = Instant::now();
        let mut l = Link::new(t);
        assert_eq!(l.check(t + Duration::from_millis(1000)), None);
        assert_eq!(l.check(t + Duration::from_millis(1600)), Some(RemoteLink::Reconnecting));
        assert_eq!(l.check(t + Duration::from_millis(2000)), None, "told once");
        assert_eq!(l.check(t + Duration::from_millis(3100)), Some(RemoteLink::Lost));
        l.ok(t + Duration::from_millis(5000));
        assert_eq!(l.check(t + Duration::from_millis(5100)), Some(RemoteLink::Live));
    }

    #[test]
    fn the_least_recently_used_go_first() {
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let files = vec![(PathBuf::from("new"), 40, at(30)), (PathBuf::from("old"), 40, at(10)), (PathBuf::from("mid"), 40, at(20))];
        assert_eq!(evict_plan(files.clone(), 100, Path::new("x")), vec![PathBuf::from("old")]);
        assert_eq!(evict_plan(files.clone(), 40, Path::new("old")), vec![PathBuf::from("mid"), PathBuf::from("new")], "never the one kept");
        assert!(evict_plan(files, 200, Path::new("x")).is_empty());
    }

    #[test]
    fn headers_of_a_file_are_checked() {
        let h = "AB".repeat(32);
        assert_eq!(clean_hash(&h), Some("ab".repeat(32)));
        assert_eq!(clean_hash("abc"), None);
        assert_eq!(clean_hash(&"zz".repeat(32)), None);
        assert_eq!(clean_ext("FLAC"), "flac");
        assert_eq!(clean_ext("../x"), "bin");
        assert_eq!(clean_ext(""), "bin");
    }

    /// A file comes in two parts (the second resumed), is checked by its
    /// hash and kept; a damaged one is deleted.
    #[tokio::test]
    async fn a_file_is_received_checked_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..=255u8).cycle().take(10_000).collect();
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let part = dir.path().join("t.part");
        let resp = |b: &[u8]| http::Response::new(Body::from(b.to_vec()));
        assert_eq!(receive(resp(&bytes[..4000]), &part, 0).await.unwrap(), 4000);
        assert_eq!(receive(resp(&bytes[4000..]), &part, 4000).await.unwrap(), 10_000);
        let done = dir.path().join(format!("{hash}.flac"));
        finish(&part, &hash, &done).await.unwrap();
        assert_eq!(std::fs::read(&done).unwrap(), bytes);
        assert!(!part.exists());

        receive(resp(&bytes[..10]), &part, 0).await.unwrap();
        let other = dir.path().join("other.flac");
        assert_eq!(finish(&part, &hash, &other).await.unwrap_err(), "файл пришёл повреждённым");
        assert!(!part.exists() && !other.exists());
    }
}
