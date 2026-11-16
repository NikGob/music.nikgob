//! Playing on another device, the network part: the runtime hands its
//! jobs here ([`ConnectJob`]) and gets what came of them back
//! ([`Command::ConnectReport`]).
//!
//! - Sessions: each target is a session with a generation; work sent for
//!   one (forwards, a pause, a transfer, an undo) is dropped once another
//!   began, before it touches the network, and a forward on its way is
//!   cut off.
//! - One at a time: forwards, transfers, undos and «Продолжить здесь» run
//!   in order from one queue, each step with a deadline for the whole
//!   answer.
//! - A transfer pauses the source and waits for its answer (none: nothing
//!   moves), then loads the destination; a load not answered is checked
//!   against what the destination plays. Nothing is ever started again to
//!   make up for a failure: the source stays paused.
//! - Following a zone: its event stream (`GET /v1/events?zone=1`) through
//!   the tunnel, each state reported with when it was (its arrival less
//!   half the round trip, measured by a light request every half second
//!   that also tells whether the device still answers: «буферизация»
//!   after 1.5 s of silence, «нет связи» after 3 s, live again as soon as
//!   it answers), and its whole snapshot fetched when its queue changes.
//! - Files: a track this device has no file of, from the device that sent
//!   the queue, into `<data dir>/cache/remote/<blake3>.<ext>` (resumed
//!   with Range, checked by its hash, a kept copy checked again before it
//!   is used, the least recently used dropped past 4 GB but never one in
//!   use). Never into the library's folders.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use futures_util::StreamExt as _;
use nikgob_api::{Caller, LocalApi};
use nikgob_core::model::TrackId;
use nikgob_runtime::RuntimeHandle;
use nikgob_view::{
    ApiActor, ApiPreset, Command, ConnectFail, ConnectJob, ConnectReport, RemoteCommand, RemoteLink, ZoneLoaded, ZoneSnapshot, ZoneState,
};
use parking_lot::Mutex;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::{Semaphore, mpsc, watch};

use crate::{MeshClient, MeshError};

/// Jobs waiting for the service (the runtime never waits on it).
const JOBS: usize = 256;
/// Operations waiting for the one that runs.
const OPS: usize = 256;
/// At most this many commands in one forward (the API's own limit).
const REMOTE_MAX: usize = 64;
/// A zone's event stream is open within this (its body then streams).
const OPEN_WAIT: Duration = Duration::from_secs(6);
/// The light request that tells the device still answers, this often…
const PING_EVERY: Duration = Duration::from_millis(500);
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
/// A small request (a forward, a pause, a snapshot, the ping) is answered,
/// all of it, within this…
const SMALL_WAIT: Duration = Duration::from_secs(5);
/// …and a load within this (the API gives the player 10 s).
const LOAD_WAIT: Duration = Duration::from_secs(15);
/// A followed zone's snapshot is fetched at most this often.
const SNAPSHOT_GAP: Duration = Duration::from_secs(1);
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
/// The generation of a session a transfer or an undo ended (until the
/// runtime begins the next one).
const ENDED: u64 = u64::MAX;

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
    /// The session now.
    live: Arc<watch::Sender<u64>>,
    /// The cached files in use (the runtime's last word): never evicted.
    keep: Arc<Mutex<Vec<PathBuf>>>,
    /// One eviction at a time.
    evicting: Arc<Mutex<()>>,
}

/// Starts the service on the mesh's runtime `rt`; the runtime hands it its
/// jobs from now on ([`RuntimeHandle::link_connect`]).
pub(crate) fn start(rt: &tokio::runtime::Handle, client: MeshClient, api: Option<LocalApi>, runtime: RuntimeHandle, data_dir: &Path) {
    let (tx, rx) = mpsc::channel::<ConnectJob>(JOBS);
    runtime.link_connect(move |job| match tx.try_send(job) {
        Ok(()) => true,
        Err(_) => {
            log::warn!("connect: a job dropped (the mesh is busy or stopped)");
            false
        }
    });
    let ctx = Ctx {
        client,
        api,
        runtime,
        cache: data_dir.join("cache").join("remote"),
        live: Arc::new(watch::channel(0).0),
        keep: Arc::default(),
        evicting: Arc::default(),
    };
    rt.spawn(dispatch(ctx, rx));
}

/// Russian words for a request to a device that failed.
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

    /// The session now.
    fn session(&self) -> u64 {
        *self.live.borrow()
    }

    /// Session `sess` ended (by a transfer or an undo it made): what was
    /// sent for it and still waits is dropped.
    fn end(&self, sess: u64) {
        self.live.send_if_modified(|g| {
            let same = *g == sess;
            if same {
                *g = ENDED;
            }
            same
        });
    }

    /// `req` to the API of `node` (None: this device's, its caller naming
    /// `peer`, the device the files come from); its head within `wait`.
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

    /// [`Self::call`] whose answer is JSON, all of it within `wait`; an
    /// error answer is its message.
    async fn json(&self, node: Option<&str>, peer: Option<&str>, req: http::Request<Body>, wait: Duration) -> Result<serde_json::Value, String> {
        let whole = async {
            let r = self.call(node, peer, req, wait).await?;
            let status = r.status();
            let bytes = axum::body::to_bytes(r.into_body(), JSON_MAX).await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                let message = serde_json::from_slice::<serde_json::Value>(&bytes).ok().and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_owned));
                return Err(message.unwrap_or_else(|| status.to_string()));
            }
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())
        };
        tokio::time::timeout(wait, whole).await.unwrap_or_else(|_| Err("устройство не ответило".to_owned()))
    }

    /// Runs `commands` on `node`'s zone (None: this device's).
    async fn run(&self, node: Option<&str>, commands: &[RemoteCommand]) -> Result<(), String> {
        let body = serde_json::to_vec(&serde_json::json!({ "commands": commands })).map_err(|e| e.to_string())?;
        let req = request(http::Method::POST, "/v1/player/remote", Some(body))?;
        self.json(node, None, req, SMALL_WAIT).await.map(|_| ())
    }

    /// `node`'s zone as a snapshot (None: this device's).
    async fn snapshot(&self, node: Option<&str>) -> Result<ZoneSnapshot, String> {
        let req = request(http::Method::GET, "/v1/player/snapshot", None)?;
        let v = self.json(node, None, req, SMALL_WAIT).await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
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

/// What runs one at a time, in order.
enum Op {
    Run { sess: u64, node: String, commands: Vec<RemoteCommand> },
    Pause { sess: u64, node: String, until: Instant },
    Move { seq: u64, sess: u64, from: Option<String>, to: Option<String> },
    Undo { seq: u64, sess: u64, node: String, snapshot: Box<ZoneSnapshot> },
    Here { seq: u64, sess: u64, snapshot: Box<ZoneSnapshot> },
}

impl Op {
    /// The report of an operation that will not run.
    fn not_run(self, why: ConnectFail) -> Option<ConnectReport> {
        match self {
            Op::Move { seq, to, .. } => Some(ConnectReport::Moved { seq, to, result: Err(why) }),
            Op::Undo { seq, node, .. } => Some(ConnectReport::Undone { seq, node, result: Err(why) }),
            Op::Here { seq, .. } => Some(ConnectReport::Here { seq, result: Err(why) }),
            Op::Run { .. } | Op::Pause { .. } => None,
        }
    }
}

async fn dispatch(ctx: Ctx, mut jobs: mpsc::Receiver<ConnectJob>) {
    let (op_tx, op_rx) = mpsc::channel::<Op>(OPS);
    let _runner = Abort(tokio::spawn(runner(ctx.clone(), op_rx)));
    // Dropping it stops the zone it followed.
    let mut _follower: Option<Abort> = None;
    let pulls = Arc::new(Semaphore::new(PULLS_AT_ONCE));
    let pulling: Arc<Mutex<HashSet<(TrackId, String, u64)>>> = Arc::default();
    while let Some(job) = jobs.recv().await {
        let op = match job {
            ConnectJob::Session { sess, follow } => {
                ctx.live.send_replace(sess);
                // A new stream every time: its first event is the state
                // now, which the runtime waits for.
                _follower = follow.map(|node| Abort(tokio::spawn(follow_zone(ctx.clone(), node, sess))));
                continue;
            }
            ConnectJob::Pull { track, from, load, keep } => {
                *ctx.keep.lock() = keep;
                let key = (track, from.clone(), load);
                if !pulling.lock().insert(key.clone()) {
                    continue;
                }
                let (c, pulls, pulling) = (ctx.clone(), pulls.clone(), pulling.clone());
                tokio::spawn(async move {
                    let result = match pulls.acquire_owned().await {
                        Ok(_permit) => pull(&c, track, &from).await,
                        Err(_) => Err("Межсеть выключена".to_owned()),
                    };
                    pulling.lock().remove(&key);
                    if let Err(e) = &result {
                        log::info!("connect: the file of {track} did not come: {e}");
                    }
                    c.report(ConnectReport::Pulled { track, from, load, result });
                });
                continue;
            }
            ConnectJob::Run { sess, node, commands } => Op::Run { sess, node, commands },
            ConnectJob::Pause { sess, node, until } => Op::Pause { sess, node, until },
            ConnectJob::Move { seq, sess, from, to } => Op::Move { seq, sess, from, to },
            ConnectJob::Undo { seq, sess, node, snapshot } => Op::Undo { seq, sess, node, snapshot },
            ConnectJob::Here { seq, sess, snapshot } => Op::Here { seq, sess, snapshot },
        };
        if let Err(e) = op_tx.try_send(op) {
            log::warn!("connect: too much waiting, an operation dropped");
            let op = match e {
                mpsc::error::TrySendError::Full(op) | mpsc::error::TrySendError::Closed(op) => op,
            };
            if let Some(r) = op.not_run(ConnectFail::NothingDone("слишком много дел сразу".to_owned())) {
                ctx.report(r);
            }
        }
    }
}

/// The operations, one at a time; those of an ended session dropped.
async fn runner(ctx: Ctx, mut rx: mpsc::Receiver<Op>) {
    let mut next: Option<Op> = None;
    loop {
        let op = match next.take() {
            Some(op) => op,
            None => match rx.recv().await {
                Some(op) => op,
                None => return,
            },
        };
        let gen_now = ctx.session();
        match op {
            Op::Run { sess, node, mut commands } => {
                // What waits of the same session and device goes along.
                while commands.len() < REMOTE_MAX {
                    match rx.try_recv() {
                        Ok(Op::Run { sess: g, node: n, commands: more }) if g == sess && n == node => commands.extend(more),
                        Ok(other) => {
                            next = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if sess != gen_now {
                    continue;
                }
                let mut ended = ctx.live.subscribe();
                for chunk in collapse(commands).chunks(REMOTE_MAX) {
                    tokio::select! {
                        r = ctx.run(Some(&node), chunk) => {
                            if let Err(why) = r {
                                ctx.report(ConnectReport::RunFailed { sess, node: node.clone(), why });
                                break;
                            }
                        }
                        () = async {
                            let _ = ended.wait_for(|g| *g != sess).await;
                        } => break,
                    }
                }
            }
            Op::Pause { sess, node, until } => {
                if sess != gen_now {
                    continue;
                }
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    continue;
                }
                match tokio::time::timeout(left, ctx.run(Some(&node), &[RemoteCommand::Pause])).await {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => log::info!("connect: the device left did not pause: {e}"),
                    Err(_) => log::info!("connect: the device left did not pause in time"),
                }
            }
            Op::Move { seq, sess, from, to } => {
                let result = if sess != gen_now { Err(ConnectFail::Stale) } else { transfer(&ctx, from.as_deref(), to.as_deref()).await };
                if result.is_ok() {
                    ctx.end(sess);
                }
                ctx.report(ConnectReport::Moved { seq, to, result });
            }
            Op::Undo { seq, sess, node, snapshot } => {
                let result = if sess != gen_now { Err(ConnectFail::Stale) } else { undo(&ctx, &node, &snapshot).await };
                if result.is_ok() {
                    ctx.end(sess);
                }
                ctx.report(ConnectReport::Undone { seq, node, result });
            }
            Op::Here { seq, sess, snapshot } => {
                let result = if sess != gen_now {
                    Err(ConnectFail::Stale)
                } else {
                    // Only what this device has a file of: the device left
                    // may not answer.
                    ctx.load(None, None, &snapshot).await.map_err(ConnectFail::NotLoaded)
                };
                ctx.report(ConnectReport::Here { seq, result });
            }
        }
    }
}

/// Whether `dest` (a zone's snapshot) plays what `sent` asked of it.
pub(crate) fn plays_what_was_sent(dest: &ZoneSnapshot, sent: &ZoneSnapshot) -> bool {
    dest.current().is_some() && dest.current() == sent.current() && dest.paused == sent.paused
}

/// Moves `from`'s music (None: this device's) to `to`'s zone (None:
/// here): the source paused (its answer awaited), then the load; a load
/// not answered is checked against what the destination plays. Nothing is
/// started again: a failure after the pause leaves the source paused.
async fn transfer(ctx: &Ctx, from: Option<&str>, to: Option<&str>) -> Result<ZoneLoaded, ConnectFail> {
    let mut snapshot = ctx.snapshot(from).await.map_err(ConnectFail::NothingDone)?;
    if !snapshot.paused {
        let asked = Instant::now();
        ctx.run(from, &[RemoteCommand::Pause]).await.map_err(ConnectFail::NothingDone)?;
        // It played on while the pause went: the destination starts there.
        snapshot.position_ms = snapshot.position_ms.saturating_add(u64::try_from(asked.elapsed().as_millis()).unwrap_or(0));
    }
    match ctx.load(to, from, &snapshot).await {
        Ok(loaded) => Ok(loaded),
        Err(e) => match ctx.snapshot(to).await {
            // It took it after all (the answer was lost).
            Ok(dest) if plays_what_was_sent(&dest, &snapshot) => {
                log::info!("connect: the load's answer did not come, but the device plays it");
                Ok(ZoneLoaded { previous: None, skipped: Vec::new(), loaded: dest.tracks.len() })
            }
            _ => Err(ConnectFail::NotLoaded(e)),
        },
    }
}

/// The undo of a transfer: `snapshot` back into `node`'s zone (checked
/// against what it plays when the answer does not come).
async fn undo(ctx: &Ctx, node: &str, snapshot: &ZoneSnapshot) -> Result<ZoneLoaded, ConnectFail> {
    match ctx.load(Some(node), None, snapshot).await {
        Ok(loaded) => Ok(loaded),
        Err(e) => match ctx.snapshot(Some(node)).await {
            Ok(dest) if plays_what_was_sent(&dest, snapshot) => Ok(ZoneLoaded { previous: None, skipped: Vec::new(), loaded: dest.tracks.len() }),
            _ => Err(ConnectFail::NotLoaded(e)),
        },
    }
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
        if ctx.json(Some(&node), None, req, SMALL_WAIT).await.is_ok() && tx.send(t.elapsed()).await.is_err() {
            return;
        }
    }
}

type Opening = std::pin::Pin<Box<dyn std::future::Future<Output = Result<http::Response<Body>, String>> + Send>>;
type Fetching = std::pin::Pin<Box<dyn std::future::Future<Output = (String, Result<ZoneSnapshot, String>)> + Send>>;

async fn opened(f: &mut Option<Opening>) -> Result<http::Response<Body>, String> {
    match f {
        Some(f) => f.await,
        None => std::future::pending().await,
    }
}

async fn fetched(f: &mut Option<Fetching>) -> (String, Result<ZoneSnapshot, String>) {
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

/// When a followed zone's snapshot is to be fetched: its queue changed
/// since the last one fetched, none on its way, the last at least
/// [`SNAPSHOT_GAP`] ago.
#[derive(Default)]
pub(crate) struct SnapshotWant {
    fetched: Option<String>,
    wanted: Option<String>,
    on_way: bool,
    last: Option<Instant>,
}

impl SnapshotWant {
    /// The zone's queue is `key` now.
    pub(crate) fn seen(&mut self, key: String) {
        if self.fetched.as_deref() != Some(key.as_str()) {
            self.wanted = Some(key);
        }
    }

    /// The key to fetch now, if one is.
    pub(crate) fn start(&mut self, now: Instant) -> Option<String> {
        if self.on_way || self.last.is_some_and(|l| now.saturating_duration_since(l) < SNAPSHOT_GAP) {
            return None;
        }
        let key = self.wanted.take()?;
        self.on_way = true;
        self.last = Some(now);
        Some(key)
    }

    /// The fetch of `key` ended (`ok`: its snapshot came).
    pub(crate) fn done(&mut self, key: String, ok: bool) {
        self.on_way = false;
        if ok {
            if self.wanted.as_deref() == Some(key.as_str()) {
                self.wanted = None;
            }
            self.fetched = Some(key);
        } else if self.wanted.is_none() {
            self.wanted = Some(key);
        }
    }
}

/// Follows `node`'s zone (session `sess`) until aborted: its states, its
/// snapshot when its queue changes, and its link, to the runtime. It
/// never sends the device anything but reads.
async fn follow_zone(ctx: Ctx, node: String, sess: u64) {
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
    let mut want = SnapshotWant::default();
    let mut fetching: Option<Fetching> = None;
    loop {
        if stream.is_none() && opening.is_none() && tokio::time::Instant::now() >= retry_at {
            let (c, n) = (ctx.clone(), node.clone());
            opening = Some(Box::pin(async move {
                let req = request(http::Method::GET, "/v1/events?zone=1", None)?;
                c.call(Some(&n), None, req, OPEN_WAIT).await
            }));
        }
        if fetching.is_none()
            && let Some(key) = want.start(Instant::now())
        {
            let (c, n) = (ctx.clone(), node.clone());
            fetching = Some(Box::pin(async move {
                let r = c.snapshot(Some(&n)).await;
                (key, r)
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
            (key, r) = fetched(&mut fetching) => {
                fetching = None;
                match r {
                    Ok(snapshot) => {
                        want.done(key, true);
                        ctx.report(ConnectReport::Snapshot { sess, node: node.clone(), snapshot: Box::new(snapshot) });
                    }
                    Err(e) => {
                        log::debug!("connect: the zone's snapshot did not come: {e}");
                        want.done(key, false);
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
                                    want.seen(state.queue_key());
                                    let half = rtt.unwrap_or_default() / 2;
                                    let at = Instant::now().checked_sub(half).unwrap_or_else(Instant::now);
                                    ctx.report(ConnectReport::State { sess, node: node.clone(), state: Box::new(state), at });
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
            ctx.report(ConnectReport::Link { sess, node: node.clone(), link: l });
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

/// Whether a `Content-Range` (`bytes a-b/n`) begins at `offset`.
pub(crate) fn range_starts_at(content_range: Option<&str>, offset: u64) -> bool {
    content_range
        .and_then(|v| v.trim().strip_prefix("bytes "))
        .and_then(|v| v.split_once('-'))
        .and_then(|(a, _)| a.trim().parse::<u64>().ok())
        == Some(offset)
}

/// The files to delete so `files` (path, size, last used) hold at most
/// `cap` bytes: the least recently used first, never one of `keep`.
pub(crate) fn evict_plan(mut files: Vec<(PathBuf, u64, SystemTime)>, cap: u64, keep: &[PathBuf]) -> Vec<PathBuf> {
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.2);
    let mut out = Vec::new();
    for (path, size, _) in files {
        if total <= cap {
            break;
        }
        if keep.contains(&path) {
            continue;
        }
        total = total.saturating_sub(size);
        out.push(path);
    }
    out
}

/// Keeps the cache at `dir` within [`CACHE_MAX`]: its files, and parts
/// nobody writes any more; `keep` (in use, just fetched) stay.
fn evict(dir: &Path, keep: &[PathBuf]) {
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

/// A copy kept as `done` that still is `hash` (marked used); one that is
/// not is deleted.
pub(crate) async fn reuse(done: &Path, hash: &str) -> bool {
    let (d, h) = (done.to_owned(), hash.to_owned());
    tokio::task::spawn_blocking(move || match blake3_of(&d) {
        Ok(sum) if sum == h => {
            touch(&d);
            true
        }
        Ok(_) => {
            log::warn!("connect: {} changed in the cache: fetched again", d.display());
            let _ = std::fs::remove_file(&d);
            false
        }
        Err(_) => false,
    })
    .await
    .unwrap_or(false)
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
/// there, checked); its path.
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
        // The part is no part of the file there, or the answer is not the
        // part asked for: from the start.
        let wrong_part = status == http::StatusCode::PARTIAL_CONTENT && !range_starts_at(header(&resp, "content-range"), offset);
        if status == http::StatusCode::RANGE_NOT_SATISFIABLE || wrong_part {
            drop(resp);
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
        if tokio::fs::try_exists(&done).await.unwrap_or(false) && reuse(&done, &hash).await {
            drop(resp);
            let _ = tokio::fs::remove_file(&part).await;
            let _ = tokio::fs::remove_file(&meta).await;
            return Ok(done);
        }
        if expected.as_deref().is_some_and(|h| h != hash) {
            // The file there changed since the part began.
            drop(resp);
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
        let mut keep = ctx.keep.lock().clone();
        keep.push(done.clone());
        let (d, evicting) = (dir.clone(), ctx.evicting.clone());
        let _ = tokio::task::spawn_blocking(move || {
            let _one = evicting.lock();
            evict(&d, &keep);
        })
        .await;
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
    fn the_least_recently_used_go_first_but_never_one_in_use() {
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let files = vec![(PathBuf::from("new"), 40, at(30)), (PathBuf::from("old"), 40, at(10)), (PathBuf::from("mid"), 40, at(20))];
        assert_eq!(evict_plan(files.clone(), 100, &[]), vec![PathBuf::from("old")]);
        assert_eq!(evict_plan(files.clone(), 40, &[PathBuf::from("old")]), vec![PathBuf::from("mid"), PathBuf::from("new")], "never one kept");
        assert_eq!(evict_plan(files.clone(), 0, &[PathBuf::from("old"), PathBuf::from("new")]), vec![PathBuf::from("mid")]);
        assert!(evict_plan(files, 200, &[]).is_empty());
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
        assert!(range_starts_at(Some("bytes 4000-9999/10000"), 4000));
        assert!(!range_starts_at(Some("bytes 0-9999/10000"), 4000));
        assert!(!range_starts_at(None, 4000));
        assert!(!range_starts_at(Some("items 4000-9999/10000"), 4000));
    }

    #[test]
    fn a_followed_zones_snapshot_is_fetched_when_its_queue_changes() {
        let t = Instant::now();
        let mut w = SnapshotWant::default();
        assert_eq!(w.start(t), None, "nothing seen");
        w.seen("a".into());
        assert_eq!(w.start(t).as_deref(), Some("a"));
        w.seen("b".into());
        assert_eq!(w.start(t + Duration::from_secs(5)), None, "one on its way");
        w.done("a".into(), true);
        assert_eq!(w.start(t + Duration::from_millis(500)), None, "too soon");
        assert_eq!(w.start(t + Duration::from_secs(2)).as_deref(), Some("b"));
        w.done("b".into(), false);
        assert_eq!(w.start(t + Duration::from_secs(4)).as_deref(), Some("b"), "a failed fetch is tried again");
        w.done("b".into(), true);
        w.seen("b".into());
        assert_eq!(w.start(t + Duration::from_secs(9)), None, "the same queue");
    }

    #[test]
    fn a_load_is_known_taken_by_what_the_device_plays() {
        let t: Vec<TrackId> = (0..3).map(|_| TrackId::new()).collect();
        let sent = ZoneSnapshot { tracks: t.clone(), index: Some(1), ..ZoneSnapshot::default() };
        let same = ZoneSnapshot { tracks: vec![t[1], t[2]], index: Some(0), ..ZoneSnapshot::default() };
        assert!(plays_what_was_sent(&same, &sent));
        let paused = ZoneSnapshot { paused: true, ..same.clone() };
        assert!(!plays_what_was_sent(&paused, &sent));
        let other = ZoneSnapshot { tracks: vec![t[0]], index: Some(0), ..ZoneSnapshot::default() };
        assert!(!plays_what_was_sent(&other, &sent));
        assert!(!plays_what_was_sent(&ZoneSnapshot::default(), &ZoneSnapshot::default()), "nothing is not it");
    }

    /// A file comes in two parts (the second resumed), is checked by its
    /// hash and kept; a damaged one is deleted; a kept copy is checked
    /// again before it is used.
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
        assert!(reuse(&done, &hash).await);

        receive(resp(&bytes[..10]), &part, 0).await.unwrap();
        let other = dir.path().join("other.flac");
        assert_eq!(finish(&part, &hash, &other).await.unwrap_err(), "файл пришёл повреждённым");
        assert!(!part.exists() && !other.exists());

        std::fs::write(&done, b"changed").unwrap();
        assert!(!reuse(&done, &hash).await, "a changed copy is not used");
        assert!(!done.exists(), "and is dropped");
    }
}
