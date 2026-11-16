//! The owner's devices find each other on the home network, pair once and
//! trust each other from then on (the ring of certificates), see each
//! other's state, and reach each other's API.
//!
//! [`start`] runs the mesh on a thread of its own with its own tokio
//! runtime, like the API. It follows the runtime's view: `[mesh] enabled`
//! turns it on and off; it reports what is live with
//! [`nikgob_view::Command::MeshUpdate`] and takes the network commands the
//! runtime passes on to it (pairing, invites, forgetting).
//!
//! On the network: one iroh (QUIC) endpoint per instance, its node id the
//! public half of `mesh/node.key`; relays and address lookups off (LAN
//! only); addresses come from mDNS. ALPNs: `nikgob/pair/1` (pairing, the
//! only thing that answers a device not in the ring, and only with
//! SPAKE2), `nikgob/ctl/1` (presence, heartbeats, the ring's updates) and
//! `nikgob/http/1` (the tunnel to the API).

pub mod discovery;
pub mod keys;
pub mod pair;
pub mod presence;
pub mod ring;
mod service;
pub mod ticket;
pub mod tunnel;
mod wire;

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use nikgob_runtime::RuntimeHandle;
use nikgob_view::{Command, MeshKind};
use tokio::sync::{mpsc, oneshot};

pub use keys::{MeshPaths, read_pair_code};
pub use ticket::linux_command;

pub const ALPN_PAIR: &[u8] = b"nikgob/pair/1";
pub const ALPN_CTL: &[u8] = b"nikgob/ctl/1";
pub const ALPN_HTTP: &[u8] = b"nikgob/http/1";
/// The UDP port the endpoint tries first (another one is taken when it is
/// busy: a second instance on the same computer).
pub const MESH_PORT: u16 = 47220;

/// What the mesh needs of its host.
pub struct MeshContext {
    /// The data dir: the mesh's files go to `<data_dir>/mesh`.
    pub data_dir: PathBuf,
    /// This device's name while `[mesh] name` is empty (the computer's
    /// name: [`default_name`]).
    pub device_name: String,
    pub kind: MeshKind,
    pub runtime: RuntimeHandle,
    /// The API's routes in this process, for the tunnel (`None`: the
    /// tunnel refuses).
    pub api: Option<nikgob_api::LocalApi>,
    /// The wake the runtime calls on every change.
    pub wake: nikgob_api::Wake,
    /// This player's version, told to the others.
    pub version: String,
    /// No window: the code is written to `mesh/pair-code` and logged, and
    /// `nikgobd pair` hands tickets over through `mesh/join-request`.
    pub headless: bool,
}

/// The computer's name, for a device named nothing else.
pub fn default_name() -> String {
    let host = gethostname::gethostname().to_string_lossy().trim().to_owned();
    let name = ring::clean_name(&host);
    if name.is_empty() { "nikgob".to_owned() } else { name }
}

/// Why a request to another device failed.
#[derive(Debug, PartialEq, Eq)]
pub enum MeshError {
    /// The mesh is off or stopping.
    Stopped,
    /// No such device in the ring.
    NotPaired,
    /// The device cannot be reached now.
    Unreachable(String),
    /// The request did not go through.
    Http(String),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped => write!(f, "the mesh is off"),
            Self::NotPaired => write!(f, "no such paired device"),
            Self::Unreachable(e) => write!(f, "the device cannot be reached: {e}"),
            Self::Http(e) => write!(f, "the request failed: {e}"),
        }
    }
}

impl std::error::Error for MeshError {}

/// Requests to the other devices' APIs, for other parts of the player
/// (playing on another device, the copies).
#[derive(Clone)]
pub struct MeshClient {
    sh: Arc<service::Shared>,
    rt: tokio::runtime::Handle,
}

impl MeshClient {
    /// Sends `req` (its path is what counts: `/v1/state`) to the API of
    /// paired device `node_id` and gives its answer, the body streaming
    /// (a `Range` request works).
    pub async fn request(&self, node_id: &str, req: http::Request<Body>) -> Result<http::Response<Body>, MeshError> {
        let sh = self.sh.clone();
        let id = node_id.to_owned();
        self.rt.spawn(async move { sh.request(&id, req).await }).await.map_err(|_| MeshError::Stopped)?
    }

    /// Whether `node_id` is connected now.
    pub fn online(&self, node_id: &str) -> bool {
        self.sh.online(node_id)
    }
}

/// The running mesh; [`MeshHandle::stop`] (or dropping it) stops it.
pub struct MeshHandle {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    client: MeshClient,
}

impl MeshHandle {
    pub fn client(&self) -> MeshClient {
        self.client.clone()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for MeshHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts the mesh's thread. The runtime passes the network commands to it
/// from now on ([`RuntimeHandle::link_mesh`]).
pub fn start(ctx: MeshContext) -> std::io::Result<MeshHandle> {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("nikgob-mesh-io").enable_all().build()?;
    let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(64);
    let runtime = ctx.runtime.clone();
    let sh = Arc::new(service::Shared::new(ctx)?);
    let client = MeshClient { sh: sh.clone(), rt: rt.handle().clone() };
    runtime.link_mesh(move |c| {
        if cmd_tx.try_send(c).is_err() {
            log::warn!("mesh: a command dropped (the mesh is busy or stopped)");
        }
    });
    let (stop_tx, stop_rx) = oneshot::channel();
    let thread = std::thread::Builder::new().name("nikgob-mesh".into()).spawn(move || {
        rt.block_on(service::run(sh, cmd_rx, stop_rx));
        rt.shutdown_timeout(std::time::Duration::from_secs(1));
    })?;
    Ok(MeshHandle { stop: Some(stop_tx), thread: Some(thread), client })
}

/// `nikgobd pair <ticket>` while no instance of `data_dir` runs: joins the
/// ring of the ticket's holder with a short-lived endpoint and keeps the
/// ring in `data_dir` (the instance takes it at its next start). The
/// holder's name.
pub fn pair_offline(data_dir: &std::path::Path, name: &str, kind: MeshKind, ticket: &str) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().map_err(|e| e.to_string())?;
    let paths = MeshPaths::of(data_dir);
    let out = rt.block_on(service::join_offline(&paths, name, kind, ticket));
    rt.shutdown_timeout(std::time::Duration::from_secs(1));
    out
}

/// `nikgobd pair <ticket>` while the instance of `data_dir` runs: hands the
/// ticket to it and waits (at most `wait`) for what came of it. The
/// holder's name.
pub fn pair_through_running(data_dir: &std::path::Path, ticket: &str, wait: std::time::Duration) -> Result<String, String> {
    service::hand_over(&MeshPaths::of(data_dir), ticket, wait)
}
