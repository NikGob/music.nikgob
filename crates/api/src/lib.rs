//! The player's local REST API: the voice assistant on the Raspberry Pi,
//! scripts and (next) the MCP server drive the player with it, through
//! the same loop as the window's buttons.
//!
//! [`start`] runs it on a thread of its own with its own tokio runtime. It
//! follows the settings live (`[api]` of settings.toml, as the runtime
//! shows them in [`nikgob_view::ApiView`]): on or off, this computer
//! (`127.0.0.1`) or the home network (`0.0.0.0`, or an explicit address),
//! the port (47210 by default); it tells the runtime where it listens or
//! why it cannot ([`nikgob_view::Command::ApiStatus`]). Every `/v1` route
//! needs a token; `/docs` (Swagger UI) and `/openapi.json` do not, and
//! `/install/` wants the download key of the invite the mesh holds instead. It
//! needs no window: the headless player starts it the same way.

mod auth;
mod confirm;
mod connect;
mod discover;
mod docs;
mod downloads;
mod error;
mod events;
mod install;
mod library;
mod mcp;
mod media;
mod mutate;
mod outputs;
mod play;
mod replica;
mod routes;
mod state;
mod stats;
mod sync;
pub mod tokens;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use axum::Router;
use axum::routing::{delete, get, post, put};
use tokio::sync::{oneshot, watch};

use nikgob_library::Library;
use nikgob_runtime::RuntimeHandle;
use nikgob_view::{ApiActor, ApiReply, Command};

pub use auth::Caller;
pub use media::{EXT_HEADER, HASH_HEADER};
pub use tokens::create_token;
pub use install::{InstallCheck, set_install_check};

/// Wakes the API when the runtime publishes a change. Give the runtime a
/// wake that calls both the window's and this one's [`Wake::wake`].
#[derive(Clone)]
pub struct Wake(Arc<watch::Sender<u64>>);

impl Wake {
    pub fn new() -> Self {
        Self(Arc::new(watch::channel(0).0))
    }

    /// Cheap and never blocks for long: call it from the runtime's wake.
    pub fn wake(&self) {
        self.0.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// A receiver that sees every wake (the mesh follows the view with it).
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}

impl Default for Wake {
    fn default() -> Self {
        Self::new()
    }
}

/// What the API needs of its host.
pub struct ApiContext {
    /// The player, as the window drives it.
    pub runtime: RuntimeHandle,
    /// A handle of the library, for reads (it has its own read connections).
    pub library: Library,
    /// The wake the runtime calls on every change.
    pub wake: Wake,
    /// Folders besides the library and the masters folder whose files may
    /// be served (a covers folder of the host's own); none is fine.
    pub media_roots: Vec<std::path::PathBuf>,
}

/// The running API; [`ApiHandle::stop`] (or dropping it) stops the server
/// and its thread.
pub struct ApiHandle {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    local: LocalApi,
}

/// The `/v1` routes for callers known by other means than a token (a
/// paired device through the mesh's tunnel): the same handlers, rights and
/// rate limit, served in this process, whether the TCP listener is on or
/// not.
#[derive(Clone)]
pub struct LocalApi {
    router: Router,
    /// Ends the event streams it serves when the API stops.
    closing: Option<Arc<watch::Sender<bool>>>,
}

/// The caller of an in-process request, put there by [`LocalApi::call`]
/// only (a request from the network cannot carry it).
#[derive(Clone)]
pub(crate) struct Trusted(pub Caller);

/// Marks a request of an in-process caller (a paired device through the
/// mesh's tunnel, or this device's own connect service): the guard puts
/// it there for those only, never for a token.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Paired;

impl LocalApi {
    fn new(sh: Arc<Shared>) -> Self {
        let (closing, closed) = watch::channel(false);
        Self { router: router(sh, Closing(closed)), closing: Some(Arc::new(closing)) }
    }

    /// A router of one's own behind [`Self::call`] (no rights checked): for
    /// tests of what carries the requests.
    pub fn for_router(router: Router) -> Self {
        Self { router, closing: None }
    }

    /// Answers `req` as `caller`.
    pub async fn call(&self, caller: Caller, mut req: axum::extract::Request) -> axum::response::Response {
        req.extensions_mut().insert(Trusted(caller));
        match tower::ServiceExt::oneshot(self.router.clone(), req).await {
            Ok(r) => r,
            Err(never) => match never {},
        }
    }

    fn close(&self) {
        if let Some(c) = &self.closing {
            let _ = c.send(true);
        }
    }
}

impl ApiHandle {
    pub fn stop(mut self) {
        self.shutdown();
    }

    /// The routes in this process, for the mesh's tunnel.
    pub fn local(&self) -> LocalApi {
        self.local.clone()
    }

    fn shutdown(&mut self) {
        self.local.close();
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ApiHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts the API's thread. It listens only while the settings say so.
pub fn start(ctx: ApiContext) -> std::io::Result<ApiHandle> {
    let (stop_tx, stop_rx) = oneshot::channel();
    let sh = Arc::new(Shared::new(ctx));
    let local = LocalApi::new(sh.clone());
    let thread = std::thread::Builder::new().name("nikgob-api".into()).spawn(move || {
        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("nikgob-api-io").enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                log::error!("api: no runtime: {e}");
                return;
            }
        };
        rt.block_on(supervise(sh, stop_rx));
        rt.shutdown_timeout(Duration::from_secs(1));
    })?;
    Ok(ApiHandle { stop: Some(stop_tx), thread: Some(thread), local })
}

/// How long an answer waits for the runtime to get past its commands.
const SYNC_WAIT: Duration = Duration::from_secs(2);
/// A server that could not listen tries again this often.
const RETRY: Duration = Duration::from_secs(15);
/// How long a stopping server may finish its answers.
const STOP_WAIT: Duration = Duration::from_secs(2);

/// What the routes share, across restarts of the server.
pub(crate) struct Shared {
    pub rt: RuntimeHandle,
    pub lib: Library,
    pub media_roots: Vec<std::path::PathBuf>,
    pub wake: Wake,
    pub tokens: auth::Tokens,
    pub limiter: auth::Limiter,
    pub uses: auth::Uses,
    pub sse_clients: AtomicUsize,
    /// The last acknowledgement sent; held while a request is sent, so the
    /// loop gets the requests in the order of their acknowledgements.
    sent: parking_lot::Mutex<u64>,
    /// The plans of destructive requests waiting for their confirmation.
    pub confirms: confirm::Store,
    /// Lookups on the services and the sources, per token (slower).
    pub lookups: auth::Limiter,
    /// The library's sound vectors for similar tracks, made at most every
    /// few minutes.
    pub sound: stats::SoundCache,
    /// Service tracks the API found, handed to the loop with a request
    /// that acts on them.
    pub seen: parking_lot::Mutex<discover::Seen>,
}

impl Shared {
    fn new(ctx: ApiContext) -> Self {
        Self {
            rt: ctx.runtime,
            lib: ctx.library,
            media_roots: ctx.media_roots,
            wake: ctx.wake,
            tokens: auth::Tokens::default(),
            limiter: auth::Limiter::default(),
            uses: auth::Uses::default(),
            sse_clients: AtomicUsize::new(0),
            sent: parking_lot::Mutex::new(0),
            confirms: confirm::Store::default(),
            lookups: auth::Limiter::new(auth::LOOKUPS_PER_S, auth::LOOKUP_BURST),
            sound: stats::SoundCache::default(),
            seen: parking_lot::Mutex::new(discover::Seen::default()),
        }
    }

    /// Sends `commands` as one request with an acknowledgement of its own
    /// (the loop runs them with nothing between them), then waits (at most
    /// [`SYNC_WAIT`]) until the loop shows it: acknowledgements are sent in
    /// order under one lock, so a larger one shown means this one was run.
    /// Not shown in time: an error, not a state the commands may not have
    /// made.
    pub async fn run(&self, commands: Vec<Command>) -> Result<(), error::ApiError> {
        self.run_by(None, commands).await
    }

    /// [`Self::run`] of a request whose token plays for `actor`: the loop
    /// keeps it while the commands run (for the history).
    pub async fn run_by(&self, actor: Option<ApiActor>, commands: Vec<Command>) -> Result<(), error::ApiError> {
        let mut changes = self.wake.subscribe();
        let ack = {
            let mut sent = self.sent.lock();
            *sent += 1;
            self.rt.send(Command::ApiRun { commands, ack: *sent, actor });
            *sent
        };
        let deadline = tokio::time::Instant::now() + SYNC_WAIT;
        loop {
            {
                let st = self.rt.state();
                if st.api.synced >= ack {
                    // What the loop could not do (a service track it no
                    // longer knows, a file outside the library) is not done.
                    return match st.api.skipped.iter().find(|(a, _)| *a == ack) {
                        Some((_, why)) => Err(error::ApiError::of(axum::http::StatusCode::CONFLICT, "not_done", format!("Not done: {}", why.join("; ")))),
                        None => Ok(()),
                    };
                }
            }
            match tokio::time::timeout_at(deadline, changes.changed()).await {
                Ok(Ok(())) => {}
                _ => {
                    log::warn!("api: the player did not answer in time");
                    return Err(error::ApiError::timeout());
                }
            }
        }
    }

    /// Sends the command `make` builds around a reply (as one request of
    /// `actor`), then waits at most `wait` for the loop's answer: work of
    /// its own (a lookup, a read of files) that ends after the loop took
    /// the request.
    pub async fn ask(&self, actor: Option<ApiActor>, wait: Duration, make: impl FnOnce(ApiReply) -> Command) -> Result<serde_json::Value, error::ApiError> {
        let (tx, rx) = oneshot::channel();
        let slot = Arc::new(parking_lot::Mutex::new(Some(tx)));
        let reply = ApiReply::new(move |answer| {
            if let Some(tx) = slot.lock().take() {
                let _ = tx.send(answer);
            }
        });
        self.run_by(actor, vec![make(reply)]).await?;
        match tokio::time::timeout(wait, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(why))) => Err(error::ApiError::of(axum::http::StatusCode::BAD_GATEWAY, "lookup_failed", why)),
            // The loop dropped the request without an answer.
            Ok(Err(_)) => Err(error::ApiError::internal("The player did not answer the request")),
            Err(_) => Err(error::ApiError::of(axum::http::StatusCode::GATEWAY_TIMEOUT, "lookup_timeout", "The search did not make it in time")),
        }
    }

    /// Whether `caller` may make another lookup on the services now
    /// (`429` otherwise).
    pub fn lookup_allowed(&self, caller: &Caller) -> Result<(), error::ApiError> {
        if self.lookups.allow(&caller.token_id, std::time::Instant::now()) {
            Ok(())
        } else {
            Err(error::ApiError::of(axum::http::StatusCode::TOO_MANY_REQUESTS, "rate_limited", "Service searches are too frequent: wait a bit"))
        }
    }

    /// The caller of `secret` as the tokens are now (a revoke counts at
    /// once).
    pub fn caller(&self, secret: &str) -> Option<Caller> {
        {
            let st = self.rt.state();
            self.tokens.refresh(&st.api.tokens);
        }
        self.tokens.find(secret)
    }

    /// Whether token `id` still works (kept and not revoked). A caller of
    /// this process (a paired device, `mesh:…`) has no token to revoke: its
    /// id has a `:`, which a token's never has.
    pub fn token_works(&self, id: &str) -> bool {
        id.contains(':') || self.rt.state().api.tokens.iter().any(|t| t.id == id && !t.revoked)
    }

    /// No sound can come out: `503 no_audio` (unless the music plays on
    /// another device the window drives).
    pub fn audio(&self) -> Result<(), error::ApiError> {
        let st = self.rt.state();
        if st.no_audio && st.target.is_none() { Err(error::ApiError::no_audio()) } else { Ok(()) }
    }

    /// The state now.
    pub fn snapshot(&self) -> state::PlayerState {
        let st = self.rt.state();
        state::PlayerState::of(&st, nikgob_library::now_ms())
    }
}

/// Stops a server's event streams when it stops (axum waits for every
/// connection to end, and a stream never does by itself).
#[derive(Clone)]
pub(crate) struct Closing(pub watch::Receiver<bool>);

async fn not_found() -> error::ApiError {
    error::ApiError::not_found("No such route")
}

/// The routes: `/v1` and `/mcp` behind the token, the docs open.
pub(crate) fn router(sh: Arc<Shared>, closing: Closing) -> Router {
    let v1 = v1_router(sh.clone(), closing);
    // The headless player's builds for a device with the invite shown now
    // (the ticket is its key, no token).
    let install = Router::new().route("/install/{file}", get(install::serve)).with_state(sh.clone());
    // MCP calls the `/v1` routes in-process with the caller's token.
    let mcp = Router::new()
        .route("/mcp", post(mcp::post).get(mcp::other).delete(mcp::other))
        .route_layer(axum::middleware::from_fn_with_state(sh.clone(), auth::guard))
        .layer(axum::Extension(mcp::Inner(v1.clone())))
        .with_state(sh);
    Router::new().merge(v1).merge(mcp).merge(install).merge(docs::swagger()).fallback(not_found)
}

/// The `/v1` routes behind the token.
fn v1_router(sh: Arc<Shared>, closing: Closing) -> Router {
    Router::new()
        .route("/v1/state", get(routes::state))
        .route("/v1/token", get(auth::whoami))
        .route("/v1/events", get(events::events))
        .route("/v1/player/play", post(routes::play))
        .route("/v1/player/pause", post(routes::pause))
        .route("/v1/player/resume", post(routes::resume))
        .route("/v1/player/toggle", post(routes::toggle))
        .route("/v1/player/next", post(routes::next))
        .route("/v1/player/previous", post(routes::previous))
        .route("/v1/player/seek", post(routes::seek))
        .route("/v1/player/mode", put(routes::mode))
        .route("/v1/player/volume", put(routes::volume))
        .route("/v1/player/duck", put(routes::duck).delete(routes::unduck))
        .route("/v1/player/output", put(routes::set_output))
        .route("/v1/player/snapshot", get(connect::snapshot))
        .route("/v1/player/load", post(connect::load))
        .route("/v1/player/remote", post(connect::remote))
        .route("/v1/outputs", get(routes::list_outputs))
        .route("/v1/queue", get(routes::queue).post(routes::queue_add).delete(routes::queue_clear))
        .route("/v1/queue/move", post(routes::queue_move))
        .route("/v1/queue/{entry}", delete(routes::queue_remove))
        .route("/v1/queue/{entry}/play", post(routes::queue_play))
        .route("/v1/search", get(library::search))
        .route("/v1/tracks/{id}", get(library::track))
        .route("/v1/tracks/{id}/lyrics", get(library::lyrics))
        .route("/v1/albums/{id}", get(library::album))
        .route("/v1/artists/{id}", get(library::artist))
        .route("/v1/playlists", get(library::playlists).post(mutate::create_playlist))
        .route("/v1/playlists/{id}", get(library::playlist).put(mutate::rename_playlist).delete(mutate::delete_playlist))
        .route("/v1/playlists/{id}/tracks", post(mutate::add_to_playlist))
        .route("/v1/playlists/{id}/remove", post(mutate::remove_from_playlist))
        .route("/v1/library/feedback", post(mutate::feedback))
        .route("/v1/library/retag", post(mutate::retag))
        .route("/v1/library/trash", post(mutate::trash))
        .route("/v1/stats/listening", get(stats::listening))
        .route("/v1/stats/forgotten", get(stats::forgotten))
        .route("/v1/stats/health", get(stats::health))
        .route("/v1/stats/similar", get(stats::similar))
        .route("/v1/discover/search", get(discover::search))
        .route("/v1/discover/artists/{id}/discography", get(discover::discography))
        .route("/v1/discover/share", get(discover::share))
        .route("/v1/downloads", get(downloads::inspect).post(downloads::enqueue))
        .route("/v1/downloads/sources", get(downloads::sources))
        .route("/v1/media/tracks/{id}/audio", get(media::audio))
        .route("/v1/media/albums/{id}/cover", get(media::cover))
        .route("/v1/sync/heads", get(sync::heads))
        .route("/v1/sync/changes", get(sync::changes))
        .route("/v1/sync/apply", post(sync::apply))
        .route("/v1/sync/blob", get(sync::blob))
        .route("/v1/replica/masters", get(replica::masters))
        .route("/v1/replica/masters/{hash}", get(replica::master_file))
        .route("/v1/replica/check", post(replica::check))
        .route_layer(axum::middleware::from_fn_with_state(sh.clone(), auth::guard))
        .layer(axum::Extension(closing))
        .with_state(sh)
}

/// The address to listen on: `ip:port` (`[ip]:port` for IPv6).
pub(crate) fn listen_addr(ip: &str, port: u16) -> String {
    if ip.contains(':') && !ip.starts_with('[') { format!("[{ip}]:{port}") } else { format!("{ip}:{port}") }
}

/// Why the server cannot listen, in words for the settings row (the app's
/// UI locale: the row is drawn in it).
pub(crate) fn bind_error(e: &std::io::Error, ip: &str, port: u16) -> String {
    use nikgob_core::i18n::{ErrorStrings, locale};
    let loc = locale();
    match e.kind() {
        std::io::ErrorKind::AddrInUse => ErrorStrings::api_port_busy_in(loc, port),
        std::io::ErrorKind::AddrNotAvailable => ErrorStrings::api_no_address_in(loc, ip),
        std::io::ErrorKind::PermissionDenied => ErrorStrings::api_port_denied_in(loc, port),
        _ => ErrorStrings::api_port_failed_in(loc, port, &e.to_string()),
    }
}

/// A listening server.
struct Server {
    addr: String,
    local: std::net::SocketAddr,
    closing: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn bind(addr: &str, sh: Arc<Shared>) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let (closing, closed) = watch::channel(false);
        let app = router(sh, Closing(closed.clone()));
        let mut stop = closed;
        let task = tokio::spawn(async move {
            let shutdown = async move {
                let _ = stop.wait_for(|c| *c).await;
            };
            if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(shutdown).await {
                log::warn!("api: the server stopped: {e}");
            }
        });
        Ok(Self { addr: addr.to_owned(), local, closing, task })
    }

    async fn stop(self) {
        let _ = self.closing.send(true);
        let mut task = self.task;
        if tokio::time::timeout(STOP_WAIT, &mut task).await.is_err() {
            log::warn!("api: the server did not stop in time");
            task.abort();
        }
    }
}

/// Keeps the server as the settings say, until `stop`.
async fn supervise(sh: Arc<Shared>, mut stop: oneshot::Receiver<()>) {
    let mut changes = sh.wake.subscribe();
    let mut running: Option<Server> = None;
    let mut reported: Option<(Option<String>, Option<String>)> = None;
    // The address that could not be had, and when to try it again (a fixed
    // time: other wakes do not put it off).
    let mut failed: Option<(String, tokio::time::Instant)> = None;
    let report = |reported: &mut Option<(Option<String>, Option<String>)>, listening: Option<String>, error: Option<String>| {
        let now = (listening, error);
        if reported.as_ref() != Some(&now) {
            sh.rt.send(Command::ApiStatus { listening: now.0.clone(), error: now.1.clone() });
            *reported = Some(now);
        }
    };
    loop {
        let (want, ip, port) = {
            let st = sh.rt.state();
            sh.tokens.refresh(&st.api.tokens);
            let ip = st.api.bind_ip();
            (st.api.enabled.then(|| listen_addr(&ip, st.api.port)), ip, st.api.port)
        };
        if want.as_deref() != running.as_ref().map(|s| s.addr.as_str()) {
            if let Some(s) = running.take() {
                s.stop().await;
                log::info!("api: stopped");
            }
            match &want {
                Some(addr) => {
                    let due = failed.as_ref().is_none_or(|(a, at)| a != addr || tokio::time::Instant::now() >= *at);
                    if due {
                        match Server::bind(addr, sh.clone()).await {
                            Ok(s) => {
                                log::info!("api: listening on {}", s.local);
                                report(&mut reported, Some(s.local.to_string()), None);
                                running = Some(s);
                                failed = None;
                            }
                            Err(e) => {
                                log::warn!("api: cannot listen on {addr}: {e}");
                                report(&mut reported, None, Some(bind_error(&e, &ip, port)));
                                failed = Some((addr.clone(), tokio::time::Instant::now() + RETRY));
                            }
                        }
                    }
                }
                None => {
                    report(&mut reported, None, None);
                    failed = None;
                }
            }
        }
        tokio::select! {
            _ = &mut stop => break,
            r = changes.changed() => {
                if r.is_err() {
                    break;
                }
            }
            () = tokio::time::sleep_until(failed.as_ref().map_or_else(tokio::time::Instant::now, |(_, at)| *at)), if failed.is_some() => {}
        }
    }
    if let Some(s) = running.take() {
        s.stop().await;
    }
    report(&mut reported, None, None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(listen_addr("127.0.0.1", 47210), "127.0.0.1:47210");
        assert_eq!(listen_addr("::1", 47210), "[::1]:47210");
        assert_eq!(listen_addr("[::1]", 1), "[::1]:1");
        let busy = std::io::Error::from(std::io::ErrorKind::AddrInUse);
        assert_eq!(bind_error(&busy, "127.0.0.1", 47210), "Порт 47210 занят");
        let none = std::io::Error::from(std::io::ErrorKind::AddrNotAvailable);
        assert_eq!(bind_error(&none, "192.168.0.157", 47210), "У компьютера нет адреса 192.168.0.157");
    }

    /// A paired device's request in this process: its caller's rights
    /// count as a token's would, and nothing without a caller gets in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_in_process_caller_has_its_rights() {
        use axum::body::Body;
        use nikgob_view::{ApiActor, ApiPreset};
        let library = Library::open_in_memory().unwrap();
        let rt = nikgob_runtime::Runtime::start(
            nikgob_runtime::RuntimeConfig {
                library: library.clone(),
                streams: Vec::new(),
                library_dir: std::env::temp_dir(),
                prefs: nikgob_view::Prefs::default(),
                prefs_path: None,
                settings_path: None,
                import: None,
            },
            Box::new(nikgob_runtime::NullPlayback),
            Arc::new(|| {}),
        );
        let ctx = ApiContext { runtime: rt.handle(), library, wake: Wake::new(), media_roots: Vec::new() };
        let local = LocalApi::new(Arc::new(Shared::new(ctx)));
        let caller = |preset| Caller { token_id: "mesh:abc".into(), name: "Pi".into(), preset, actor: ApiActor::Owner };
        let get = || axum::http::Request::get("/v1/state").body(Body::empty()).unwrap();
        let r = local.call(caller(ApiPreset::Full), get()).await;
        assert_eq!(r.status(), axum::http::StatusCode::OK);
        let pause = || axum::http::Request::post("/v1/player/pause").body(Body::empty()).unwrap();
        let r = local.call(caller(ApiPreset::Read), pause()).await;
        assert_eq!(r.status(), axum::http::StatusCode::FORBIDDEN);
        // Straight to the router, as from the network: no token, no entry.
        let r = tower::ServiceExt::oneshot(local.router.clone(), get()).await.unwrap();
        assert_eq!(r.status(), axum::http::StatusCode::UNAUTHORIZED);
        drop(rt);
    }

    /// `/install/` answers only to the key the mesh's check takes, and only
    /// with this version's builds; the script carries the key, not the
    /// ticket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_install_script_wants_the_invite() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        let library = Library::open_in_memory().unwrap();
        let rt = nikgob_runtime::Runtime::start(
            nikgob_runtime::RuntimeConfig {
                library: library.clone(),
                streams: Vec::new(),
                library_dir: std::env::temp_dir(),
                prefs: nikgob_view::Prefs::default(),
                prefs_path: None,
                settings_path: None,
                import: None,
            },
            Box::new(nikgob_runtime::NullPlayback),
            Arc::new(|| {}),
        );
        let h = rt.handle();
        let ctx = ApiContext { runtime: h.clone(), library, wake: Wake::new(), media_roots: Vec::new() };
        let local = LocalApi::new(Arc::new(Shared::new(ctx)));
        let ask = |path: &str| Request::get(path).header("host", "192.168.0.10:47210").body(Body::empty()).unwrap();
        let key = nikgob_core::releases::download_key("nkm1abc");
        let script = format!("/install/nikgobd.sh?k={key}");
        set_install_check(None);
        let r = tower::ServiceExt::oneshot(local.router.clone(), ask(&script)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "no mesh, no invite");
        let want = key.clone();
        set_install_check(Some(Arc::new(move |k: &str| nikgob_core::releases::same_secret(k.as_bytes(), want.as_bytes()))));
        let other = format!("/install/nikgobd.sh?k={}", nikgob_core::releases::download_key("nkm1abd"));
        let r = tower::ServiceExt::oneshot(local.router.clone(), ask(&other)).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "another invite's key");
        let r = tower::ServiceExt::oneshot(local.router.clone(), ask("/install/nikgobd.sh?t=nkm1abc")).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "the ticket opens nothing");
        let r = tower::ServiceExt::oneshot(local.router.clone(), ask(&script)).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("base='http://192.168.0.10:47210'"), "{text}");
        assert!(text.contains(&format!("key='{key}'")), "{text}");
        assert!(!text.contains("nkm1abc"), "{text}");
        let old = ask(&format!("/install/nikgobd-0.0.0-linux-aarch64.tar.gz?k={key}"));
        let r = tower::ServiceExt::oneshot(local.router.clone(), old).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "not this version");
        let r = tower::ServiceExt::oneshot(local.router.clone(), ask(&format!("/install/settings.toml?k={key}"))).await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        set_install_check(None);
        drop(rt);
    }

    /// Two servers cannot share a port: the second says so.
    #[tokio::test]
    async fn a_taken_port_is_an_error() {
        let first = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = first.local_addr().unwrap().port();
        let second = tokio::net::TcpListener::bind(listen_addr("127.0.0.1", port)).await;
        let e = second.err().expect("the port is taken");
        assert_eq!(bind_error(&e, "127.0.0.1", port), format!("Порт {port} занят"));
    }
}
