//! The embedded client: the server session, the listener, peer connections
//! (direct and through the server), searches both ways and the distributed
//! network. Transfers are in [`crate::transfer`].

use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use parking_lot::{Mutex, RwLock};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, Semaphore, mpsc, oneshot, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use crate::codec::{self, read_frame, split_u8, split_u32};
use crate::limit::Limiter;
use crate::msg::{ConnType, DirEntry, DistribMsg, FromServer, MAJOR_VERSION, MINOR_VERSION, PeerInit, PeerMsg, ToServer};
use crate::share::ShareIndex;
use crate::transfer::{Downloads, Uploads};
use crate::{Config, Connection, Error, SearchQuery, SearchResult, ShareSettings, Status};

use crate::transfer::Download;

/// Frames longer than these are refused (and the connection dropped).
const SERVER_FRAME_LIMIT: usize = 16 << 20;
pub(crate) const PEER_FRAME_LIMIT: usize = 128 << 20;
const INIT_FRAME_LIMIT: usize = 4096;
const DISTRIB_FRAME_LIMIT: usize = 1 << 20;
/// Results kept per search.
const RESULTS_PER_SEARCH: usize = 5000;
/// Files sent in one search answer.
const ANSWER_LIMIT: usize = 100;
/// Search answers being sent at once; more are dropped.
const ANSWERS_AT_ONCE: usize = 4;
const SERVER_PING: Duration = Duration::from_secs(120);

type Waiters<T> = Mutex<HashMap<(String, u32), oneshot::Sender<T>>>;

/// A connection to a peer for peer messages.
pub(crate) struct PeerHandle {
    id: u64,
    tx: mpsc::UnboundedSender<Vec<u8>>,
    closed: CancellationToken,
}

impl PeerHandle {
    pub(crate) fn send(&self, m: &PeerMsg) -> bool {
        self.tx.send(m.encode()).is_ok()
    }

    fn send_raw(&self, bytes: Vec<u8>) -> bool {
        self.tx.send(bytes).is_ok()
    }

    fn live(&self) -> bool {
        !self.closed.is_cancelled()
    }
}

pub(crate) struct SearchSink {
    results: Mutex<Vec<SearchResult>>,
    created: Instant,
}

#[derive(Default)]
struct Distrib {
    parent: Option<String>,
    parent_conn: Option<CancellationToken>,
    trying: bool,
    /// We told the server we are a branch root.
    root: bool,
}

pub(crate) struct Inner {
    pub(crate) cfg: Config,
    pub(crate) listen_port: u16,
    pub(crate) share_settings: Mutex<ShareSettings>,
    server_tx: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    conn_state: watch::Sender<Connection>,
    tokens: AtomicU32,
    conn_ids: AtomicU64,
    addr_waiters: Mutex<HashMap<String, Vec<oneshot::Sender<Option<SocketAddrV4>>>>>,
    /// Indirect connection requests waiting for a PierceFireWall (or None:
    /// the peer could not connect).
    pierce: Mutex<HashMap<u32, oneshot::Sender<Option<TcpStream>>>>,
    peers: Mutex<HashMap<String, Arc<PeerHandle>>>,
    connect_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    searches: Mutex<HashMap<u32, Arc<SearchSink>>>,
    folder_waiters: Waiters<Vec<DirEntry>>,
    /// Answers to our transfer requests, by (user, token).
    pub(crate) transfer_responses: Waiters<(bool, Option<String>)>,
    pub(crate) downloads: Mutex<Downloads>,
    pub(crate) uploads: Mutex<Uploads>,
    pub(crate) share: RwLock<Arc<ShareIndex>>,
    excluded: RwLock<Vec<String>>,
    pub(crate) limiter: Limiter,
    distrib: Mutex<Distrib>,
    answer_slots: Arc<Semaphore>,
    answered: Mutex<VecDeque<(String, u32)>>,
    pub(crate) uploaded_bytes: AtomicU64,
    /// Our average upload speed, bytes per second (what search answers
    /// tell).
    pub(crate) avg_speed: AtomicU32,
    pub(crate) shutdown: CancellationToken,
    rescan: Notify,
}

/// The embedded Soulseek client. Cheap to clone; [`Client::shutdown`] stops
/// it.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<Inner>,
}

/// A running search; results keep coming until it is dropped.
pub struct SearchHandle {
    inner: Arc<Inner>,
    token: u32,
    sink: Arc<SearchSink>,
}

impl SearchHandle {
    pub fn token(&self) -> u32 {
        self.token
    }

    /// What came in so far.
    pub fn results(&self) -> Vec<SearchResult> {
        self.sink.results.lock().clone()
    }

    /// Waits `d`, then returns what came in.
    pub async fn wait(&self, d: Duration) -> Vec<SearchResult> {
        tokio::time::sleep(d).await;
        self.results()
    }
}

impl Drop for SearchHandle {
    fn drop(&mut self) {
        self.inner.searches.lock().remove(&self.token);
    }
}

async fn bind(port: u16) -> std::io::Result<TcpListener> {
    if port == 0 {
        return TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).await;
    }
    let mut last = None;
    for p in port..port.saturating_add(10) {
        match TcpListener::bind((Ipv4Addr::UNSPECIFIED, p)).await {
            Ok(l) => return Ok(l),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("no port")))
}

/// Waits for the first of two attempts to succeed (the direct and the
/// indirect connection).
async fn first_ok<T>(
    a: impl Future<Output = Result<T, Error>>,
    b: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    tokio::pin!(a, b);
    let mut a_err: Option<Error> = None;
    let mut b_err: Option<Error> = None;
    loop {
        tokio::select! {
            r = &mut a, if a_err.is_none() => match r {
                Ok(v) => return Ok(v),
                // An offline user will not answer the indirect request either.
                Err(e @ Error::Offline(_)) => return Err(e),
                Err(e) => a_err = Some(e),
            },
            r = &mut b, if b_err.is_none() => match r {
                Ok(v) => return Ok(v),
                Err(e) => b_err = Some(e),
            },
        }
        if a_err.is_some() && b_err.is_some() {
            return Err(b_err.or(a_err).expect("an error"));
        }
    }
}

/// Removes a pending indirect request when the attempt ends.
struct PierceGuard<'a>(&'a Inner, u32);

impl Drop for PierceGuard<'_> {
    fn drop(&mut self) {
        self.0.pierce.lock().remove(&self.1);
    }
}

/// The search in a message the server embeds, as sent, or (older servers)
/// with the body length in front.
fn embedded_search(code: u8, body: &[u8]) -> Option<(String, u32, String)> {
    if code != 3 {
        return None;
    }
    let parse = |b: &[u8]| match DistribMsg::decode(3, b) {
        Ok(DistribMsg::Search { user, token, query }) => Some((user, token, query)),
        _ => None,
    };
    parse(body).or_else(|| {
        let len = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
        (len == body.len() - 4).then(|| parse(&body[4..])).flatten()
    })
}

impl Client {
    /// Starts the client: binds the listener, then logs in and shares in
    /// the background. Fails at once when there are no credentials or no
    /// port to listen on.
    pub async fn start(cfg: Config) -> Result<Self, Error> {
        if cfg.username.trim().is_empty() || cfg.password.is_empty() {
            return Err(Error::Config("the Soulseek username and password are empty".into()));
        }
        let listener = bind(cfg.listen_port).await?;
        let listen_port = listener.local_addr()?.port();
        let inner = Arc::new(Inner {
            limiter: Limiter::new(cfg.share.upload_limit_kbps),
            share_settings: Mutex::new(cfg.share),
            cfg,
            listen_port,
            server_tx: Mutex::new(None),
            conn_state: watch::Sender::new(Connection::Disconnected),
            tokens: AtomicU32::new(fastrand_seed()),
            conn_ids: AtomicU64::new(1),
            addr_waiters: Mutex::default(),
            pierce: Mutex::default(),
            peers: Mutex::default(),
            connect_locks: Mutex::default(),
            searches: Mutex::default(),
            folder_waiters: Mutex::default(),
            transfer_responses: Mutex::default(),
            downloads: Mutex::default(),
            uploads: Mutex::default(),
            share: RwLock::new(Arc::new(ShareIndex::empty())),
            excluded: RwLock::default(),
            distrib: Mutex::default(),
            answer_slots: Arc::new(Semaphore::new(ANSWERS_AT_ONCE)),
            answered: Mutex::default(),
            uploaded_bytes: AtomicU64::new(0),
            avg_speed: AtomicU32::new(0),
            shutdown: CancellationToken::new(),
            rescan: Notify::new(),
        });
        info!("soulseek: listening on port {listen_port}");
        tokio::spawn(inner.clone().accept_loop(listener));
        tokio::spawn(inner.clone().share_loop());
        tokio::spawn(inner.clone().server_loop());
        Ok(Self { inner })
    }

    pub fn status(&self) -> Status {
        self.inner.status()
    }

    /// Waits until logged in, or until the login fails for good.
    pub async fn wait_logged_in(&self, within: Duration) -> Result<(), Error> {
        let mut rx = self.inner.conn_state.subscribe();
        let wait = async {
            loop {
                match &*rx.borrow_and_update() {
                    Connection::LoggedIn => return Ok(()),
                    Connection::Failed { reason } => return Err(Error::Login(reason.clone())),
                    _ => {}
                }
                if rx.changed().await.is_err() {
                    return Err(Error::NotLoggedIn);
                }
            }
        };
        timeout(within, wait).await.map_err(|_| Error::Timeout)?
    }

    /// Sends a search to the network; results come in on the handle.
    pub fn start_search(&self, query: &SearchQuery) -> Result<SearchHandle, Error> {
        let inner = &self.inner;
        let token = inner.token();
        let sink = Arc::new(SearchSink { results: Mutex::default(), created: Instant::now() });
        {
            let mut s = inner.searches.lock();
            s.retain(|_, v| v.created.elapsed() < Duration::from_secs(600));
            s.insert(token, sink.clone());
        }
        let handle = SearchHandle { inner: inner.clone(), token, sink };
        inner.server_send(ToServer::FileSearch { token, query: query.text.trim().to_owned() })?;
        Ok(handle)
    }

    /// Searches for `wait`, then returns the results.
    pub async fn search(&self, query: &SearchQuery, wait: Duration) -> Result<Vec<SearchResult>, Error> {
        let h = self.start_search(query)?;
        Ok(h.wait(wait).await)
    }

    /// Every file in a sharer's folder and its subfolders.
    pub async fn folder(&self, user: &str, folder: &str) -> Result<Vec<SearchResult>, Error> {
        let inner = &self.inner;
        let token = inner.token();
        let (tx, rx) = oneshot::channel();
        inner.folder_waiters.lock().insert((user.to_owned(), token), tx);
        let res = async {
            let h = inner.peer(user).await?;
            h.send(&PeerMsg::FolderContentsRequest { token, folder: folder.to_owned() });
            timeout(inner.cfg.timeouts.transfer, rx).await.map_err(|_| Error::Timeout)?.map_err(|_| Error::Cancelled)
        }
        .await;
        inner.folder_waiters.lock().remove(&(user.to_owned(), token));
        let dirs = res?;
        let mut out = Vec::new();
        for d in dirs {
            for f in &d.files {
                let path = format!("{}\\{}", d.name.trim_end_matches('\\'), f.name);
                out.push(SearchResult::from_entry(user, path, f, false, 0, 0));
            }
        }
        Ok(out)
    }

    /// Downloads `result` into `<dir>/<the sharer's folder name>/`; a
    /// partial file there from before is resumed.
    pub fn download(&self, result: &SearchResult, dir: &Path) -> Download {
        crate::transfer::start_download(&self.inner, result, dir)
    }

    pub fn downloads(&self) -> Vec<crate::DownloadInfo> {
        self.inner.downloads.lock().list()
    }

    /// Running and queued uploads, then the last finished ones.
    pub fn uploads(&self) -> Vec<crate::UploadInfo> {
        self.inner.uploads.lock().list()
    }

    /// A download started earlier, by id.
    pub fn download_by_id(&self, id: u64) -> Option<Download> {
        self.inner.downloads.lock().handle(id)
    }

    pub fn share_settings(&self) -> ShareSettings {
        *self.inner.share_settings.lock()
    }

    /// Changes sharing live: turning it off ends queued and running uploads
    /// and leaves the distributed network.
    pub fn set_share(&self, s: ShareSettings) {
        self.inner.set_share(s);
    }

    /// Lists the shared folders again (after an import, say).
    pub fn rescan_share(&self) {
        self.inner.rescan.notify_one();
    }

    /// Stops everything; partial downloads stay for a resume.
    pub fn shutdown(&self) {
        self.inner.shutdown.cancel();
        self.inner.server_tx.lock().take();
    }
}

fn fastrand_seed() -> u32 {
    // Tokens only need to differ between runs; the clock is enough.
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
    (n % 1_000_000) + 1000
}

impl Inner {
    pub(crate) fn token(&self) -> u32 {
        self.tokens.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn server_send(&self, m: ToServer) -> Result<(), Error> {
        let g = self.server_tx.lock();
        let tx = g.as_ref().ok_or(Error::NotLoggedIn)?;
        tx.send(m.encode()).map_err(|_| Error::NotLoggedIn)
    }

    fn status(&self) -> Status {
        let share = self.share.read().clone();
        let enabled = self.share_settings.lock().enabled;
        let (uploads_active, uploads_queued) = self.uploads.lock().counts();
        let d = self.distrib.lock();
        Status {
            connection: self.conn_state.borrow().clone(),
            username: self.cfg.username.clone(),
            listen_port: self.listen_port,
            shared_files: if enabled { share.file_count() } else { 0 },
            shared_dirs: if enabled { share.dir_count() } else { 0 },
            uploads_active,
            uploads_queued,
            uploaded_bytes: self.uploaded_bytes.load(Ordering::Relaxed),
            downloads_active: self.downloads.lock().active(),
            distributed: d.parent.is_some() || d.root,
        }
    }

    fn set_state(&self, c: Connection) {
        self.conn_state.send_replace(c);
    }

    // ---- the server session ----

    async fn server_loop(self: Arc<Self>) {
        let mut wait = self.cfg.timeouts.reconnect;
        loop {
            if self.shutdown.is_cancelled() {
                break;
            }
            self.set_state(Connection::Connecting);
            let started = Instant::now();
            let res = self.server_session().await;
            self.server_tx.lock().take();
            self.addr_waiters.lock().clear();
            self.drop_parent(false);
            match res {
                Ok(()) => break,
                Err(Error::Login(reason)) => {
                    warn!("soulseek: login refused: {reason}");
                    self.set_state(Connection::Failed { reason });
                    break;
                }
                Err(Error::Denied(reason)) => {
                    warn!("soulseek: {reason}");
                    self.set_state(Connection::Failed { reason });
                    break;
                }
                Err(e) => {
                    info!("soulseek: server connection ended: {e}");
                    self.set_state(Connection::Disconnected);
                }
            }
            if started.elapsed() > Duration::from_secs(600) {
                wait = self.cfg.timeouts.reconnect;
            }
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
            wait = (wait * 2).min(Duration::from_secs(300));
        }
        if !matches!(*self.conn_state.borrow(), Connection::Failed { .. }) {
            self.set_state(Connection::Disconnected);
        }
    }

    /// One session: connect, log in, then serve until it ends. `Ok` = shut
    /// down.
    async fn server_session(self: &Arc<Self>) -> Result<(), Error> {
        let stream = timeout(self.cfg.timeouts.connect, TcpStream::connect(&self.cfg.server))
            .await
            .map_err(|_| Error::Timeout)??;
        let _ = stream.set_nodelay(true);
        let ka = socket2::TcpKeepalive::new().with_time(Duration::from_secs(60)).with_interval(Duration::from_secs(20));
        let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(&ka);
        let (mut rd, mut wr) = stream.into_split();
        let login = ToServer::Login {
            username: self.cfg.username.clone(),
            password: self.cfg.password.clone(),
            major: MAJOR_VERSION,
            minor: MINOR_VERSION,
        };
        codec::write_all(&mut wr, &login.encode()).await?;
        let frame = timeout(self.cfg.timeouts.indirect, read_frame(&mut rd, SERVER_FRAME_LIMIT))
            .await
            .map_err(|_| Error::Timeout)??;
        let (code, body) = split_u32(&frame)?;
        match FromServer::decode(code, body)? {
            FromServer::LoginOk { .. } => {}
            FromServer::LoginFailed { reason, detail } => {
                return Err(Error::Login(match detail {
                    Some(d) => format!("{reason}: {d}"),
                    None => reason,
                }));
            }
            _ => return Err(Error::Protocol("no login answer".into())),
        }
        info!("soulseek: logged in as {}", self.cfg.username);
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        *self.server_tx.lock() = Some(tx);
        let stop = self.shutdown.child_token();
        let writer_stop = stop.clone();
        let writer = tokio::spawn(async move {
            let mut ping = tokio::time::interval(SERVER_PING);
            ping.tick().await;
            loop {
                let bytes = tokio::select! {
                    _ = writer_stop.cancelled() => break,
                    _ = ping.tick() => ToServer::Ping.encode(),
                    m = rx.recv() => match m {
                        Some(b) => b,
                        None => break,
                    },
                };
                if codec::write_all(&mut wr, &bytes).await.is_err() {
                    break;
                }
            }
            writer_stop.cancel();
        });
        let share = *self.share_settings.lock();
        self.server_send(ToServer::SetWaitPort(u32::from(self.listen_port)))?;
        self.server_send(ToServer::SetStatus(2))?;
        self.announce_share();
        if share.enabled {
            self.server_send(ToServer::HaveNoParent(true))?;
        }
        self.server_send(ToServer::AcceptChildren(false))?;
        self.set_state(Connection::LoggedIn);
        let res = loop {
            let frame = tokio::select! {
                _ = stop.cancelled() => break if self.shutdown.is_cancelled() { Ok(()) } else { Err(Error::Protocol("server writer stopped".into())) },
                f = read_frame(&mut rd, SERVER_FRAME_LIMIT) => f,
            };
            let frame = match frame {
                Ok(f) => f,
                Err(e) => break Err(Error::Io(e)),
            };
            let Ok((code, body)) = split_u32(&frame) else { continue };
            match FromServer::decode(code, body) {
                Ok(FromServer::Relogged) => {
                    break Err(Error::Denied("logged in from somewhere else with this username".into()));
                }
                Ok(m) => self.on_server(m),
                Err(e) => debug!("soulseek: server message {code}: {e}"),
            }
        };
        stop.cancel();
        let _ = writer.await;
        res
    }

    fn on_server(self: &Arc<Self>, m: FromServer) {
        match m {
            FromServer::PeerAddress { user, ip, port } => {
                // No address: offline. Port 0: online but not listening, so
                // only the indirect way can work.
                let addr = (!ip.is_unspecified()).then(|| SocketAddrV4::new(ip, u16::try_from(port).unwrap_or(0)));
                for w in self.addr_waiters.lock().remove(&user).unwrap_or_default() {
                    let _ = w.send(addr);
                }
            }
            FromServer::ConnectToPeer { user, kind, ip, port, token, .. } => {
                if kind == ConnType::D || port > u32::from(u16::MAX) {
                    let _ = self.server_send(ToServer::CantConnectToPeer { token, user });
                    return;
                }
                let me = self.clone();
                tokio::spawn(async move {
                    let addr = SocketAddrV4::new(ip, port as u16);
                    let pierce = async {
                        let mut s = timeout(me.cfg.timeouts.connect, TcpStream::connect(addr)).await.ok()?.ok()?;
                        codec::write_all(&mut s, &PeerInit::PierceFirewall(token).encode()).await.ok()?;
                        Some(s)
                    };
                    match pierce.await {
                        Some(s) => me.dispatch(user, kind, s),
                        None => {
                            debug!("soulseek: cannot reach {user} for its {} connection", kind.as_str());
                            let _ = me.server_send(ToServer::CantConnectToPeer { token, user });
                        }
                    }
                });
            }
            FromServer::CantConnectToPeer { token } => {
                if let Some(w) = self.pierce.lock().remove(&token) {
                    let _ = w.send(None);
                }
            }
            FromServer::MessageUser { id, .. } => {
                let _ = self.server_send(ToServer::MessageAcked(id));
            }
            FromServer::FileSearch { user, token, query } => self.on_search(&user, token, &query),
            FromServer::EmbeddedMessage { code, body } => {
                if let Some((user, token, query)) = embedded_search(code, &body) {
                    self.became_root();
                    self.on_search(&user, token, &query);
                }
            }
            FromServer::PossibleParents(list) => self.try_parents(list),
            FromServer::ResetDistributed => self.drop_parent(true),
            FromServer::ExcludedSearchPhrases(v) => {
                *self.excluded.write() = v.into_iter().map(|p| p.to_lowercase()).collect();
            }
            _ => {}
        }
    }

    pub(crate) fn announce_share(&self) {
        let enabled = self.share_settings.lock().enabled;
        let share = self.share.read().clone();
        let (dirs, files) = if enabled { (share.dir_count(), share.file_count()) } else { (0, 0) };
        let _ = self.server_send(ToServer::SharedFoldersFiles { dirs, files });
    }

    fn set_share(self: &Arc<Self>, s: ShareSettings) {
        let was = std::mem::replace(&mut *self.share_settings.lock(), s);
        self.limiter.set_kbps(s.upload_limit_kbps);
        if was.enabled && !s.enabled {
            self.stop_uploads();
            self.drop_parent(false);
            self.distrib.lock().root = false;
            *self.share.write() = Arc::new(ShareIndex::empty());
            self.announce_share();
        } else if !was.enabled && s.enabled {
            self.rescan.notify_one();
            let _ = self.server_send(ToServer::HaveNoParent(true));
        }
        self.pump_uploads();
    }

    async fn share_loop(self: Arc<Self>) {
        loop {
            let enabled = self.share_settings.lock().enabled;
            if enabled && !self.cfg.share_folders.is_empty() {
                let folders = self.cfg.share_folders.clone();
                let cache = self.cfg.state_dir.as_ref().map(|d| d.join("share.cache"));
                let opts = self.cfg.scan;
                let started = Instant::now();
                let index = tokio::task::spawn_blocking(move || {
                    let _ = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Min);
                    ShareIndex::scan(&folders, cache.as_deref(), opts)
                })
                .await;
                if let Ok(index) = index
                    && self.share_settings.lock().enabled
                {
                    info!(
                        "soulseek: sharing {} files in {} folders (listed in {:.1} s)",
                        index.file_count(),
                        index.dir_count(),
                        started.elapsed().as_secs_f64()
                    );
                    *self.share.write() = Arc::new(index);
                    self.announce_share();
                }
            }
            let every = self.cfg.rescan_every.unwrap_or(Duration::from_secs(u64::from(u32::MAX)));
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = self.rescan.notified() => {}
                _ = tokio::time::sleep(every) => {}
            }
        }
    }

    // ---- peer connections ----

    async fn accept_loop(self: Arc<Self>, listener: TcpListener) {
        loop {
            let (s, _) = tokio::select! {
                _ = self.shutdown.cancelled() => break,
                r = listener.accept() => match r {
                    Ok(v) => v,
                    Err(e) => {
                        debug!("soulseek: accept: {e}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                },
            };
            let me = self.clone();
            tokio::spawn(async move { me.inbound(s).await });
        }
    }

    async fn inbound(self: Arc<Self>, mut s: TcpStream) {
        let Ok(Ok(frame)) = timeout(self.cfg.timeouts.indirect, read_frame(&mut s, INIT_FRAME_LIMIT)).await else {
            return;
        };
        let Ok((code, body)) = split_u8(&frame) else { return };
        match PeerInit::decode(code, body) {
            Ok(PeerInit::PierceFirewall(token)) => {
                if let Some(w) = self.pierce.lock().remove(&token) {
                    let _ = w.send(Some(s));
                }
            }
            Ok(PeerInit::Init { user, kind, .. }) => self.dispatch(user, kind, s),
            Err(e) => debug!("soulseek: peer init: {e}"),
        }
    }

    /// A connection a peer opened (directly, or by our answer to its
    /// indirect request).
    fn dispatch(self: &Arc<Self>, user: String, kind: ConnType, s: TcpStream) {
        match kind {
            ConnType::P => {
                self.attach_peer(&user, s);
            }
            // A file connection is always opened by the side that uploads.
            ConnType::F => {
                tokio::spawn(crate::transfer::incoming_file(self.clone(), user, s));
            }
            // We take no children in the distributed network.
            ConnType::D => {}
        }
    }

    async fn peer_address(&self, user: &str) -> Result<Option<SocketAddrV4>, Error> {
        let (tx, rx) = oneshot::channel();
        let first = {
            let mut w = self.addr_waiters.lock();
            let v = w.entry(user.to_owned()).or_default();
            v.retain(|s| !s.is_closed());
            v.push(tx);
            v.len() == 1
        };
        if first {
            self.server_send(ToServer::GetPeerAddress(user.to_owned()))?;
        }
        match timeout(self.cfg.timeouts.indirect, rx).await {
            Ok(Ok(a)) => Ok(a),
            Ok(Err(_)) => Err(Error::NotLoggedIn),
            Err(_) => Err(Error::Timeout),
        }
    }

    /// Opens a connection of `kind` to `user`: directly to the address the
    /// server gives, and at the same time asks the server to have the peer
    /// connect to us; the first that works is used.
    pub(crate) async fn connect(self: &Arc<Self>, user: &str, kind: ConnType) -> Result<TcpStream, Error> {
        let token = self.token();
        let (tx, rx) = oneshot::channel();
        self.pierce.lock().insert(token, tx);
        let _guard = PierceGuard(self, token);
        self.server_send(ToServer::ConnectToPeer { token, user: user.to_owned(), kind })?;
        let direct = async {
            let addr = self.peer_address(user).await?.ok_or_else(|| Error::Offline(user.to_owned()))?;
            if addr.port() == 0 {
                return Err(Error::Unreachable(user.to_owned()));
            }
            let mut s = timeout(self.cfg.timeouts.connect, TcpStream::connect(addr))
                .await
                .map_err(|_| Error::Unreachable(user.to_owned()))??;
            let init = PeerInit::Init { user: self.cfg.username.clone(), kind, token: 0 };
            codec::write_all(&mut s, &init.encode()).await?;
            Ok(s)
        };
        let indirect = async {
            match timeout(self.cfg.timeouts.indirect, rx).await {
                Ok(Ok(Some(s))) => Ok(s),
                _ => Err(Error::Unreachable(user.to_owned())),
            }
        };
        let s = first_ok(direct, indirect).await?;
        let _ = s.set_nodelay(true);
        Ok(s)
    }

    /// The open `P` connection to `user`, if any.
    pub(crate) fn live_peer(&self, user: &str) -> Option<Arc<PeerHandle>> {
        self.peers.lock().get(user).filter(|h| h.live()).cloned()
    }

    /// The `P` connection to `user`, opened if there is none.
    pub(crate) async fn peer(self: &Arc<Self>, user: &str) -> Result<Arc<PeerHandle>, Error> {
        if let Some(h) = self.live_peer(user) {
            return Ok(h);
        }
        let lock = self.connect_locks.lock().entry(user.to_owned()).or_default().clone();
        let _g = lock.lock().await;
        if let Some(h) = self.live_peer(user) {
            return Ok(h);
        }
        let res = self.connect(user, ConnType::P).await;
        self.connect_locks.lock().remove(user);
        Ok(self.attach_peer(user, res?))
    }

    fn attach_peer(self: &Arc<Self>, user: &str, s: TcpStream) -> Arc<PeerHandle> {
        let (rd, mut wr) = s.into_split();
        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let closed = self.shutdown.child_token();
        let h = Arc::new(PeerHandle { id: self.conn_ids.fetch_add(1, Ordering::Relaxed), tx, closed: closed.clone() });
        self.peers.lock().insert(user.to_owned(), h.clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = closed.cancelled() => break,
                    m = rx.recv() => match m {
                        Some(b) => if codec::write_all(&mut wr, &b).await.is_err() { break },
                        None => break,
                    },
                }
            }
            closed.cancel();
        });
        tokio::spawn(self.clone().peer_reader(user.to_owned(), rd, h.clone()));
        h
    }

    async fn peer_reader(self: Arc<Self>, user: String, mut rd: OwnedReadHalf, h: Arc<PeerHandle>) {
        loop {
            let frame = tokio::select! {
                _ = h.closed.cancelled() => break,
                r = timeout(self.cfg.timeouts.peer_idle, read_frame(&mut rd, PEER_FRAME_LIMIT)) => r,
            };
            let Ok(Ok(frame)) = frame else { break };
            let Ok((code, body)) = split_u32(&frame) else { break };
            match PeerMsg::decode(code, body) {
                Ok(m) => self.on_peer(&user, m, &h),
                Err(e) => debug!("soulseek: peer message {code} from {user}: {e}"),
            }
        }
        h.closed.cancel();
        let mut peers = self.peers.lock();
        if peers.get(&user).is_some_and(|p| p.id == h.id) {
            peers.remove(&user);
        }
    }

    fn on_peer(self: &Arc<Self>, user: &str, m: PeerMsg, h: &Arc<PeerHandle>) {
        let sharing = self.share_settings.lock().enabled;
        match m {
            PeerMsg::SharedFileListRequest => {
                if sharing {
                    let share = self.share.read().clone();
                    h.send_raw(share.browse_message().to_vec());
                } else {
                    h.send(&PeerMsg::SharedFileListResponse(Vec::new()));
                }
            }
            PeerMsg::FileSearchResponse(reply) => {
                let Some(sink) = self.searches.lock().get(&reply.token).cloned() else { return };
                let mut res = sink.results.lock();
                for f in &reply.files {
                    if res.len() >= RESULTS_PER_SEARCH {
                        break;
                    }
                    res.push(SearchResult::from_entry(
                        user,
                        f.name.clone(),
                        f,
                        reply.slot_free,
                        reply.avg_speed,
                        reply.queue_len,
                    ));
                }
            }
            PeerMsg::UserInfoRequest => {
                let s = *self.share_settings.lock();
                let (active, queued) = self.uploads.lock().counts();
                h.send(&PeerMsg::UserInfoResponse {
                    description: String::new(),
                    total_uploads: s.upload_slots,
                    queue_size: queued,
                    slots_free: s.enabled && active < s.upload_slots.max(1),
                });
            }
            PeerMsg::FolderContentsRequest { token, folder } => {
                let dirs = if sharing { self.share.read().folder(&folder) } else { Vec::new() };
                h.send(&PeerMsg::FolderContentsResponse { token, folder, dirs });
            }
            PeerMsg::FolderContentsResponse { token, dirs, .. } => {
                if let Some(w) = self.folder_waiters.lock().remove(&(user.to_owned(), token)) {
                    let _ = w.send(dirs);
                }
            }
            other => self.on_transfer_msg(user, other, h),
        }
    }

    // ---- searches from others ----

    fn on_search(self: &Arc<Self>, user: &str, token: u32, query: &str) {
        if user == self.cfg.username || !self.share_settings.lock().enabled {
            return;
        }
        {
            let mut seen = self.answered.lock();
            if seen.iter().any(|(u, t)| u == user && *t == token) {
                return;
            }
            if seen.len() >= 512 {
                seen.pop_front();
            }
            seen.push_back((user.to_owned(), token));
        }
        let files = {
            let excluded = self.excluded.read();
            self.share.read().search(query, &excluded, ANSWER_LIMIT)
        };
        if files.is_empty() {
            return;
        }
        let Ok(permit) = self.answer_slots.clone().try_acquire_owned() else {
            debug!("soulseek: busy, a search answer is dropped");
            return;
        };
        let me = self.clone();
        let user = user.to_owned();
        tokio::spawn(async move {
            let s = *me.share_settings.lock();
            let (active, queued) = me.uploads.lock().counts();
            let reply = crate::msg::SearchReply {
                user: me.cfg.username.clone(),
                token,
                files,
                slot_free: active < s.upload_slots.max(1),
                avg_speed: me.avg_speed.load(Ordering::Relaxed),
                queue_len: queued,
                private: Vec::new(),
            };
            match me.peer(&user).await {
                Ok(h) => {
                    h.send(&PeerMsg::FileSearchResponse(reply));
                }
                Err(e) => debug!("soulseek: cannot answer {user}'s search: {e}"),
            }
            drop(permit);
        });
    }

    // ---- the distributed network ----

    fn became_root(&self) {
        let mut d = self.distrib.lock();
        if d.root || d.parent.is_some() {
            return;
        }
        d.root = true;
        drop(d);
        let _ = self.server_send(ToServer::BranchLevel(0));
        let _ = self.server_send(ToServer::BranchRoot(self.cfg.username.clone()));
    }

    fn drop_parent(&self, ask_again: bool) {
        let mut d = self.distrib.lock();
        d.parent = None;
        if let Some(c) = d.parent_conn.take() {
            c.cancel();
        }
        drop(d);
        if ask_again && self.share_settings.lock().enabled {
            let _ = self.server_send(ToServer::HaveNoParent(true));
        }
    }

    fn try_parents(self: &Arc<Self>, list: Vec<(String, Ipv4Addr, u32)>) {
        {
            let mut d = self.distrib.lock();
            if d.parent.is_some() || d.trying || !self.share_settings.lock().enabled {
                return;
            }
            d.trying = true;
        }
        let me = self.clone();
        tokio::spawn(async move {
            let mut set = tokio::task::JoinSet::new();
            for (user, ip, port) in list.into_iter().take(10) {
                if user == me.cfg.username || port == 0 || port > u32::from(u16::MAX) {
                    continue;
                }
                let me = me.clone();
                set.spawn(async move {
                    let addr = SocketAddrV4::new(ip, port as u16);
                    let direct = async {
                        let mut s = timeout(me.cfg.timeouts.connect, TcpStream::connect(addr))
                            .await
                            .map_err(|_| Error::Timeout)??;
                        let init = PeerInit::Init { user: me.cfg.username.clone(), kind: ConnType::D, token: 0 };
                        codec::write_all(&mut s, &init.encode()).await?;
                        Ok::<_, Error>(s)
                    };
                    let s = match direct.await {
                        Ok(s) => s,
                        Err(_) => me.connect(&user, ConnType::D).await?,
                    };
                    me.parent_conn(user, s).await;
                    Ok::<_, Error>(())
                });
            }
            while set.join_next().await.is_some() {
                if me.distrib.lock().parent.is_some() {
                    // The others give up once they see a parent was taken.
                    break;
                }
            }
            set.detach_all();
            let mut d = me.distrib.lock();
            if d.parent.is_none() {
                d.trying = false;
            }
        });
    }

    /// Reads a (would-be) parent's connection; the first that sends its
    /// branch level and a search becomes the parent.
    async fn parent_conn(self: &Arc<Self>, user: String, s: TcpStream) {
        let (mut rd, _wr) = s.into_split();
        let stop = self.shutdown.child_token();
        let mut level: Option<i32> = None;
        let mut root: Option<String> = None;
        let mut adopted = false;
        loop {
            let frame = tokio::select! {
                _ = stop.cancelled() => break,
                r = timeout(Duration::from_secs(300), read_frame(&mut rd, DISTRIB_FRAME_LIMIT)) => r,
            };
            let Ok(Ok(frame)) = frame else { break };
            let Ok((code, body)) = split_u8(&frame) else { break };
            let search = match DistribMsg::decode(code, body) {
                Ok(DistribMsg::BranchLevel(l)) => {
                    level = Some(l);
                    if l == 0 {
                        root.get_or_insert_with(|| user.clone());
                    }
                    None
                }
                Ok(DistribMsg::BranchRoot(r)) => {
                    root = Some(r);
                    None
                }
                Ok(DistribMsg::Search { user, token, query }) => Some((user, token, query)),
                Ok(DistribMsg::Embedded { code, body }) => embedded_search(code, &body),
                _ => None,
            };
            let Some((from, token, query)) = search else { continue };
            if !adopted {
                let Some(l) = level else { continue };
                let mut d = self.distrib.lock();
                if d.parent.is_some() || !self.share_settings.lock().enabled {
                    break;
                }
                d.parent = Some(user.clone());
                d.parent_conn = Some(stop.clone());
                d.root = false;
                d.trying = false;
                drop(d);
                adopted = true;
                info!("soulseek: distributed parent {user} (branch level {})", l + 1);
                let _ = self.server_send(ToServer::HaveNoParent(false));
                let _ = self.server_send(ToServer::BranchLevel((l + 1).max(1) as u32));
                let _ = self.server_send(ToServer::BranchRoot(root.clone().unwrap_or_else(|| user.clone())));
            }
            self.on_search(&from, token, &query);
        }
        if adopted {
            let mut d = self.distrib.lock();
            if d.parent.as_deref() == Some(user.as_str()) {
                d.parent = None;
                d.parent_conn = None;
                drop(d);
                if !self.shutdown.is_cancelled() && self.share_settings.lock().enabled {
                    let _ = self.server_send(ToServer::HaveNoParent(true));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_search_reads_both_layouts() {
        let body = DistribMsg::search_body("bob", 7, "aimer");
        assert_eq!(embedded_search(3, &body), Some(("bob".into(), 7, "aimer".into())));
        let mut prefixed = (body.len() as u32).to_le_bytes().to_vec();
        prefixed.extend_from_slice(&body);
        assert_eq!(embedded_search(3, &prefixed), Some(("bob".into(), 7, "aimer".into())));
        assert_eq!(embedded_search(4, &body), None);
    }
}
