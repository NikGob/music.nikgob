//! An in-process stand-in for the Soulseek server, and the bits of a peer
//! the tests drive by hand. Nothing here touches the real network.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use nikgob_soulseek::codec::{read_frame, split_u32, write_all};
use nikgob_soulseek::msg::{ConnType, DistribMsg, FromServer, ToServer};
use nikgob_soulseek::{Client, Config, ShareSettings, Timeouts};
use parking_lot::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

struct Online {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    kick: Arc<tokio::sync::Notify>,
    ip: Ipv4Addr,
    port: u32,
}

#[derive(Default)]
struct State {
    passwords: HashMap<String, String>,
    online: HashMap<String, Online>,
    hidden: HashSet<String>,
    statics: HashMap<String, SocketAddrV4>,
    received: Vec<(String, ToServer)>,
    /// Searches go to everyone else as embedded messages (each client is a
    /// branch root); off, they go nowhere and the test delivers them.
    embed: bool,
}

#[derive(Clone)]
pub struct MockServer {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
    /// A port nothing listens on.
    dead_port: u16,
}

impl MockServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dead_port = {
            let l = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            l.local_addr().unwrap().port()
        };
        let state = Arc::new(Mutex::new(State { embed: true, ..State::default() }));
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((s, peer)) = listener.accept().await else { break };
                let st = st.clone();
                tokio::spawn(async move { session(st, s, peer, dead_port).await });
            }
        });
        Self { addr, state, dead_port }
    }

    pub fn set_password(&self, user: &str, pw: &str) {
        self.state.lock().passwords.insert(user.into(), pw.into());
    }

    /// The server gives out an address nobody answers on for `user`, so
    /// only indirect connections reach it.
    pub fn hide_port(&self, user: &str) {
        self.state.lock().hidden.insert(user.into());
    }

    /// A peer the test runs by hand, not logged in.
    pub fn add_static_peer(&self, user: &str, addr: SocketAddrV4) {
        self.state.lock().statics.insert(user.into(), addr);
    }

    pub fn set_embed(&self, on: bool) {
        self.state.lock().embed = on;
    }

    pub fn received(&self, user: &str) -> Vec<ToServer> {
        self.state.lock().received.iter().filter(|(u, _)| u == user).map(|(_, m)| m.clone()).collect()
    }

    pub fn send_to(&self, user: &str, m: FromServer) -> bool {
        self.state.lock().online.get(user).is_some_and(|o| o.tx.send(m.encode()).is_ok())
    }

    /// Drops the user's session, as a server restart would.
    pub fn kick(&self, user: &str) {
        if let Some(o) = self.state.lock().online.get(user) {
            o.kick.notify_one();
        }
    }

    pub fn is_online(&self, user: &str) -> bool {
        self.state.lock().online.contains_key(user)
    }

    /// Waits until `user` sent something `pred` accepts.
    pub async fn wait_for(&self, user: &str, pred: impl Fn(&ToServer) -> bool, within: Duration) -> Option<ToServer> {
        let end = tokio::time::Instant::now() + within;
        loop {
            if let Some(m) = self.received(user).into_iter().find(|m| pred(m)) {
                return Some(m);
            }
            if tokio::time::Instant::now() > end {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

async fn session(st: Arc<Mutex<State>>, s: TcpStream, peer: SocketAddr, dead_port: u16) {
    let (mut rd, mut wr) = s.into_split();
    let Ok(frame) = read_frame(&mut rd, 1 << 20).await else { return };
    let (code, body) = split_u32(&frame).unwrap();
    let Ok(ToServer::Login { username, password, .. }) = ToServer::decode(code, body) else { return };
    let ip = match peer.ip() {
        std::net::IpAddr::V4(v) => v,
        _ => Ipv4Addr::LOCALHOST,
    };
    let refused = {
        let mut g = st.lock();
        match g.passwords.get(&username) {
            Some(pw) if *pw != password => true,
            Some(_) => false,
            None => {
                g.passwords.insert(username.clone(), password.clone());
                false
            }
        }
    };
    if refused {
        let m = FromServer::LoginFailed { reason: "INVALIDPASS".into(), detail: None };
        let _ = write_all(&mut wr, &m.encode()).await;
        return;
    }
    let ok = FromServer::LoginOk { greet: "welcome".into(), ip, supporter: false };
    if write_all(&mut wr, &ok.encode()).await.is_err() {
        return;
    }
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let kick = Arc::new(tokio::sync::Notify::new());
    st.lock().online.insert(username.clone(), Online { tx: tx.clone(), kick: kick.clone(), ip, port: 0 });
    let kicked = kick.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = kicked.notified() => break,
                b = rx.recv() => match b {
                    Some(b) => if write_all(&mut wr, &b).await.is_err() { break },
                    None => break,
                },
            }
        }
    });
    loop {
        let frame = tokio::select! {
            _ = kick.notified() => break,
            f = read_frame(&mut rd, 16 << 20) => match f {
                Ok(f) => f,
                Err(_) => break,
            },
        };
        let Ok((code, body)) = split_u32(&frame) else { break };
        let Ok(m) = ToServer::decode(code, body) else { continue };
        let mut g = st.lock();
        g.received.push((username.clone(), m.clone()));
        match m {
            ToServer::SetWaitPort(p) => {
                if let Some(o) = g.online.get_mut(&username) {
                    o.port = p;
                }
            }
            ToServer::GetPeerAddress(u) => {
                let (ip, port) = if let Some(a) = g.statics.get(&u) {
                    (*a.ip(), u32::from(a.port()))
                } else if let Some(o) = g.online.get(&u) {
                    (o.ip, if g.hidden.contains(&u) { u32::from(dead_port) } else { o.port })
                } else {
                    (Ipv4Addr::UNSPECIFIED, 0)
                };
                let _ = tx.send(FromServer::PeerAddress { user: u, ip, port }.encode());
            }
            ToServer::ConnectToPeer { token, user, kind } => {
                let me = g.online.get(&username).map(|o| (o.ip, o.port));
                if let (Some(target), Some((ip, port))) = (g.online.get(&user), me) {
                    let m = FromServer::ConnectToPeer { user: username.clone(), kind, ip, port, token, privileged: false };
                    let _ = target.tx.send(m.encode());
                }
            }
            ToServer::CantConnectToPeer { token, user } => {
                if let Some(o) = g.online.get(&user) {
                    let _ = o.tx.send(FromServer::CantConnectToPeer { token }.encode());
                }
            }
            ToServer::FileSearch { token, query } if g.embed => {
                let body = DistribMsg::search_body(&username, token, &query);
                for (u, o) in &g.online {
                    if *u != username {
                        let _ = o.tx.send(FromServer::EmbeddedMessage { code: 3, body: body.clone() }.encode());
                    }
                }
            }
            _ => {}
        }
    }
    let mut g = st.lock();
    if g.online.get(&username).is_some_and(|o| o.tx.same_channel(&tx)) {
        g.online.remove(&username);
    }
}

pub fn timeouts() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(2),
        indirect: Duration::from_secs(5),
        reconnect: Duration::from_millis(300),
        peer_idle: Duration::from_secs(30),
        transfer: Duration::from_secs(10),
        queue_check: Duration::from_secs(1),
    }
}

pub fn config(server: &MockServer, user: &str, share: Option<&std::path::Path>) -> Config {
    Config {
        server: server.addr.to_string(),
        username: user.into(),
        password: format!("{user}-pw"),
        listen_port: 0,
        share_folders: share.map(|p| vec![p.to_path_buf()]).unwrap_or_default(),
        share: ShareSettings { enabled: share.is_some(), upload_limit_kbps: 0, upload_slots: 2 },
        state_dir: None,
        rescan_every: None,
        timeouts: timeouts(),
        ..Config::default()
    }
}

/// A client logged in to `server` (and, when it shares, done listing).
pub async fn client(cfg: Config) -> Client {
    let _ = env_logger::builder().is_test(true).try_init();
    let sharing = cfg.share.enabled && !cfg.share_folders.is_empty();
    let c = Client::start(cfg).await.unwrap();
    c.wait_logged_in(Duration::from_secs(5)).await.unwrap();
    if sharing {
        wait(|| c.status().shared_files > 0, Duration::from_secs(10)).await;
    }
    c
}

pub async fn wait(mut f: impl FnMut() -> bool, within: Duration) -> bool {
    let end = tokio::time::Instant::now() + within;
    while !f() {
        if tokio::time::Instant::now() > end {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    true
}

/// Bytes that are easy to check: each one depends on its position.
pub fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i.wrapping_mul(31) ^ (i >> 8)) as u8).collect()
}

/// A library folder with a few albums.
pub fn library(root: &std::path::Path, song_bytes: usize) -> std::path::PathBuf {
    let lib = root.join("Music");
    let album = lib.join("Aimer").join("Daydream");
    std::fs::create_dir_all(&album).unwrap();
    std::fs::write(album.join("01 Insane Dream.flac"), pattern(song_bytes)).unwrap();
    std::fs::write(album.join("02 Falling Alone.flac"), pattern(song_bytes / 2)).unwrap();
    std::fs::write(album.join("cover.jpg"), pattern(1000)).unwrap();
    let other = lib.join("ZUTOMAYO").join("Single");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("勘冴えて悔しいわ.mp3"), pattern(2000)).unwrap();
    lib
}

/// A distributed parent the test runs: sends branch level and root, then
/// the searches it is handed.
pub struct MockParent {
    pub addr: SocketAddrV4,
    pub searches: mpsc::UnboundedSender<DistribMsg>,
    pub connected: Arc<Mutex<Option<String>>>,
}

impl MockParent {
    pub async fn start(name: &str) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = match listener.local_addr().unwrap() {
            SocketAddr::V4(a) => a,
            _ => unreachable!(),
        };
        let (tx, mut rx) = mpsc::unbounded_channel::<DistribMsg>();
        let connected = Arc::new(Mutex::new(None));
        let who = connected.clone();
        let name = name.to_owned();
        tokio::spawn(async move {
            let Ok((mut s, _)) = listener.accept().await else { return };
            let frame = read_frame(&mut s, 4096).await.unwrap();
            let (code, body) = nikgob_soulseek::codec::split_u8(&frame).unwrap();
            if let Ok(nikgob_soulseek::msg::PeerInit::Init { user, kind: ConnType::D, .. }) =
                nikgob_soulseek::msg::PeerInit::decode(code, body)
            {
                *who.lock() = Some(user);
            }
            write_all(&mut s, &DistribMsg::BranchLevel(0).encode()).await.unwrap();
            write_all(&mut s, &DistribMsg::BranchRoot(name).encode()).await.unwrap();
            while let Some(m) = rx.recv().await {
                if write_all(&mut s, &m.encode()).await.is_err() {
                    break;
                }
            }
        });
        Self { addr, searches: tx, connected }
    }
}
