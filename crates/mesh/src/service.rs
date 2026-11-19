//! The mesh service: the endpoint and mDNS while the mesh is on, the links
//! to the paired devices (presence, heartbeats, the ring's news), pairing
//! both ways, the tunnel, and what it reports to the runtime.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use iroh::endpoint::{Connection, Endpoint, IncomingAddr, RecvStream, RelayMode, SendStream, VarInt, presets};
use iroh::{EndpointAddr, PublicKey, SecretKey};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use nikgob_api::LocalApi;
use nikgob_runtime::RuntimeHandle;
use nikgob_core::i18n::mesh::fmt_open_net_in;
use nikgob_core::i18n::{Locale, locale, mesh, mesh_in};
use nikgob_view::{Command, MeshInvite, MeshKind, MeshNearbyView, MeshNodeView, MeshPairing, MeshScope, MeshSnapshot, MeshSnapshotNode};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch};

use crate::discovery::{self, Change, Table};
use crate::keys::{self, MeshPaths};
use crate::pair::{self, Grant, Me, PairError, PairHost, Refusal, Served};
use crate::presence::{self, Presence};
use crate::ring::{Cert, Revocation, Ring, RingError, clean_name, hex, parse_id};
use crate::ticket::{self, Ticket};
use crate::wire::{read_frame, write_frame};
use crate::{ALPN_CTL, ALPN_HTTP, ALPN_PAIR, MESH_PORT, MeshContext, MeshError, tunnel};

/// The library's sync with the other devices (a child of this module: it
/// reads the ring and the links as they are here).
#[path = "sync.rs"]
mod sync;

/// The library's files on this device by the profile the ring holder gives
/// it, and the copies this computer's masters have on the others (a child
/// of this module for the same reason).
#[path = "replica.rs"]
mod replica;

/// A link sends its presence at least this often…
const HEARTBEAT: Duration = Duration::from_secs(5);
/// …and a link that hears nothing for this long is dropped (offline).
const OFFLINE_AFTER: Duration = Duration::from_secs(15);
/// How often the devices without a link are dialled.
const DIAL_EVERY: Duration = Duration::from_secs(2);
const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
/// A failed dial waits 2^n seconds, at most this long, before the next.
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// The device with the greater id dials too once it waited this long for
/// the other to dial it.
const TIE_WAIT: Duration = Duration::from_secs(8);
const HANDSHAKE: Duration = Duration::from_secs(10);
const PAIR_TIMEOUT: Duration = Duration::from_secs(30);
/// An endpoint that could not be bound is tried again this often.
const RETRY: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_secs(2);
/// Changes are reported to the runtime at most this often.
const PUBLISH_GAP: Duration = Duration::from_millis(150);
/// Incoming connections being set up at once (the QUIC handshake, then a
/// pairing or a link's hello); more are turned away.
const MAX_HANDSHAKES: usize = 16;
/// Tunnelled requests one device may have running here at once; the next
/// stream is taken when one ends.
const MAX_STREAMS: usize = 32;

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0))
}

/// This computer's best address on the home network (an IPv4 one first):
/// a private one of a real card ([`discovery::lan_addrs`]).
fn lan_ip() -> Option<IpAddr> {
    let all = discovery::lan_addrs();
    all.iter().copied().find(IpAddr::is_ipv4).or_else(|| all.first().copied())
}

/// The control link's messages (ALPN `nikgob/ctl/1`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Ctl {
    /// The first message both ways: the ring, every certificate and
    /// revocation the sender knows, its presence.
    Hello { ring: String, certs: Vec<Cert>, revoked: Vec<Revocation>, presence: Presence },
    /// The link is refused; the revocations say why when this device was
    /// forgotten.
    Refuse { reason: String, revoked: Vec<Revocation> },
    Presence { presence: Presence },
    /// The ring changed (a device paired, renamed by the holder, forgotten).
    Ring { certs: Vec<Cert>, revoked: Vec<Revocation> },
}

/// Why pairing a nearby device failed: a wrong code (its window shakes,
/// no word) or the words said.
enum NearbyFail {
    WrongCode,
    Said(String),
}

/// A paired device as the links know it.
#[derive(Default)]
struct Peer {
    /// The current link: its number and whether this side dialled it.
    link: Option<(u64, bool)>,
    dialing: bool,
    presence: Option<Presence>,
    last_seen_ms: Option<i64>,
    /// The address it was last dialled at.
    addr: Option<SocketAddr>,
    fails: u32,
    retry_at: Option<Instant>,
    /// Since when a link is wanted (for [`TIE_WAIT`]).
    wanted_since: Option<Instant>,
}

struct Invite {
    id: [u8; 8],
    secret: [u8; 16],
    expires_ms: i64,
    tries: u32,
    ticket: String,
    /// The key that opens `/install/` while it lives
    /// ([`nikgob_core::releases::download_key`]).
    key: String,
    /// What a device that joins with it becomes.
    scope: MeshScope,
}

/// Whether the invite `held` opens `/install/` to `key` at `now` (unix ms):
/// alive, not spent by wrong tries, its download key (constant time).
fn invite_opens(held: Option<&Invite>, key: &str, now: i64) -> bool {
    held.is_some_and(|i| i.tries < pair::TRIES && now < i.expires_ms && nikgob_core::releases::same_secret(i.key.as_bytes(), key.as_bytes()))
}

/// What a joiner becomes: the scope of the invite `held` when it joins
/// with an invite (`invite`, its id: checked alive before), an owner when
/// the holder added it by its code.
fn joiner_scope(invite: Option<&str>, held: Option<&Invite>) -> MeshScope {
    match (invite, held) {
        (Some(id), Some(i)) if hex(&i.id) == id => i.scope,
        _ => MeshScope::Owner,
    }
}

struct Code {
    code: String,
    tries: u32,
    busy: bool,
}

/// A tunnel connection a device opened here, kept so that it can be closed
/// when the ring forgets the device or makes it a guest.
struct HttpIn {
    n: u64,
    peer: PublicKey,
    conn: Connection,
    /// The device's scope when the connection opened.
    scope: MeshScope,
}

struct State {
    name: String,
    kind: MeshKind,
    ring: Option<Ring>,
    ring_key: Option<SecretKey>,
    running: bool,
    port: Option<u16>,
    lan: Option<IpAddr>,
    /// This device's home network subnets: only addresses inside them are
    /// dialled or remembered.
    subnets: Vec<discovery::Subnet>,
    peers: HashMap<PublicKey, Peer>,
    table: Table,
    /// Addresses the runtime keeps for the paired devices.
    known: HashMap<PublicKey, SocketAddr>,
    pairing: MeshPairing,
    invite: Option<Invite>,
    code: Option<Code>,
    error: Option<String>,
    next_link: u64,
}

/// The endpoint and the announcement while the mesh is on.
struct Node {
    ep: Endpoint,
    mdns: Option<ServiceDaemon>,
    announced: Mutex<Option<String>>,
    /// The addresses announced (told in the log when they change).
    said: Mutex<Vec<IpAddr>>,
    stop: watch::Sender<bool>,
    port: u16,
}

pub(crate) struct Shared {
    paths: MeshPaths,
    key: SecretKey,
    rt: RuntimeHandle,
    api: Option<LocalApi>,
    wake: nikgob_api::Wake,
    version: String,
    headless: bool,
    default_name: String,
    state: Mutex<State>,
    changed: Notify,
    ring_rev: watch::Sender<u64>,
    presence: watch::Sender<Presence>,
    node: Mutex<Option<Arc<Node>>>,
    http: Mutex<HashMap<PublicKey, Connection>>,
    /// The tunnel connections other devices opened here (with a counter).
    http_in: Mutex<(u64, Vec<HttpIn>)>,
    /// Each device's room for running tunnelled requests.
    streams: Mutex<HashMap<PublicKey, Arc<Semaphore>>>,
    library: Option<nikgob_library::Library>,
    /// This device's pull of the library's files and the pauses asked here.
    replica: replica::ReplicaShared,
    /// What each device holds of the files the replica plan gives it
    /// (bytes done, bytes planned), counted by [`replica::count_copies`].
    copies: Mutex<HashMap<String, (u64, u64)>>,
}

/// Binds an endpoint with `key`: LAN only, every ALPN of the mesh; on
/// [`MESH_PORT`] if it is free (`local`: 127.0.0.1, any port, for tests).
pub(crate) async fn bind(key: &SecretKey, local: bool) -> Result<Endpoint, String> {
    let alpns = vec![ALPN_PAIR.to_vec(), ALPN_CTL.to_vec(), ALPN_HTTP.to_vec()];
    let tries: Vec<String> = if local { vec!["127.0.0.1:0".to_owned()] } else { vec![format!("0.0.0.0:{MESH_PORT}"), "0.0.0.0:0".to_owned()] };
    let mut last = String::from("no address");
    for at in &tries {
        let builder = Endpoint::builder(presets::Minimal)
            .secret_key(key.clone())
            .alpns(alpns.clone())
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .clear_ip_transports();
        let builder = match builder.bind_addr(at.as_str()) {
            Ok(b) => b,
            Err(e) => {
                last = e.to_string();
                continue;
            }
        };
        match builder.bind().await {
            Ok(ep) => return Ok(ep),
            Err(e) => {
                log::info!("mesh: cannot bind {at}: {e}");
                last = e.to_string();
            }
        }
    }
    Err(last)
}

/// Dials `id` at `addrs` for `alpn`.
async fn connect(ep: &Endpoint, id: PublicKey, addrs: &[SocketAddr], alpn: &[u8]) -> Result<Connection, String> {
    if addrs.is_empty() {
        return Err("no address".to_owned());
    }
    let mut addr = EndpointAddr::new(id);
    for a in addrs {
        addr = addr.with_ip_addr(*a);
    }
    match tokio::time::timeout(DIAL_TIMEOUT, ep.connect(addr, alpn)).await {
        Ok(Ok(c)) => Ok(c),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out".to_owned()),
    }
}

/// Ends an exchange on one stream: this side's end is sent, the other
/// side's awaited (so the last message is read before the close).
async fn wind_down(conn: &Connection, send: &mut SendStream, recv: &mut RecvStream) {
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(3), recv.read_to_end(4096)).await;
    conn.close(VarInt::from_u32(0), b"done");
}

/// Joins the ring of `t`'s holder from endpoint `ep`.
async fn join_ticket(ep: &Endpoint, me: &Me, t: &Ticket) -> Result<(Grant, String), PairError> {
    let conn = connect(ep, t.holder, &t.addrs, ALPN_PAIR).await.map_err(PairError::Io)?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| PairError::Io(e.to_string()))?;
    let out = match tokio::time::timeout(PAIR_TIMEOUT, pair::join(&mut recv, &mut send, me, &t.holder, &t.invite, &t.secret)).await {
        Ok(r) => r,
        Err(_) => Err(PairError::Io("timed out".to_owned())),
    };
    wind_down(&conn, &mut send, &mut recv).await;
    out
}

/// A ticket's text, checked, with only the addresses inside this device's
/// home network subnets (none left: nothing is dialled).
fn ticket_on_lan(loc: Locale, text: &str) -> Result<Ticket, String> {
    let mut t = ticket_of(loc, text)?;
    let subnets = discovery::lan_subnets();
    t.addrs.retain(|a| discovery::on_lan(a.ip(), &subnets));
    Ok(t)
}

/// A ticket's text, checked: decodes, not expired.
fn ticket_of(loc: Locale, text: &str) -> Result<Ticket, String> {
    let t = Ticket::decode(text).map_err(|e| e.message_in(loc))?;
    if t.expired(now_ms()) {
        return Err(ticket::TicketError::Expired.message_in(loc));
    }
    Ok(t)
}

/// `nikgobd pair <ticket>` with no instance running.
pub(crate) async fn join_offline(paths: &MeshPaths, name: &str, kind: MeshKind, text: &str) -> Result<String, String> {
    let s = mesh();
    let key = keys::node_key(paths).map_err(|e| e.to_string())?;
    if let Ok(Some(_)) = Ring::load(&paths.ring(), &key.public()) {
        return Err(s.already_paired.to_owned());
    }
    let t = ticket_on_lan(locale(), text)?;
    let ep = bind(&key, false).await.map_err(|e| fmt_open_net_in(locale(), &e))?;
    let me = Me { id: key.public(), name: clean_name(name), kind };
    let out = join_ticket(&ep, &me, &t).await;
    ep.close().await;
    let (grant, holder_name) = out.map_err(|e| e.user_message())?;
    let ring = Ring::received(&grant.ring, &key.public(), grant.cert, &grant.members, &grant.revoked).map_err(|e| e.to_string())?;
    ring.save(&paths.ring()).map_err(|e| e.to_string())?;
    Ok(holder_name)
}

const REQUEST_FILE: &str = "join-request";
const RESULT_FILE: &str = "join-result";

/// `nikgobd pair <ticket>` with the instance running: the ticket goes to
/// it through a file of its data dir, the answer comes back the same way.
pub(crate) fn hand_over(paths: &MeshPaths, text: &str, wait: Duration) -> Result<String, String> {
    ticket_of(locale(), text)?;
    let request = paths.dir.join(REQUEST_FILE);
    let result = paths.dir.join(RESULT_FILE);
    let _ = std::fs::remove_file(&result);
    keys::write_atomic(&request, text.trim().as_bytes(), true).map_err(|e| format!("{}: {e}", request.display()))?;
    let until = Instant::now() + wait;
    while Instant::now() < until {
        if let Ok(answer) = std::fs::read_to_string(&result) {
            let _ = std::fs::remove_file(&result);
            let (status, rest) = answer.split_once('\n').unwrap_or((answer.as_str(), ""));
            return if status.trim() == "ok" { Ok(rest.trim().to_owned()) } else { Err(rest.trim().to_owned()) };
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::fs::remove_file(&request);
    Err(mesh().player_silent.to_owned())
}

impl Shared {
    pub(crate) fn new(ctx: MeshContext) -> std::io::Result<Self> {
        let paths = MeshPaths::of(&ctx.data_dir);
        let key = keys::node_key(&paths)?;
        let me = key.public();
        let ring = match Ring::load(&paths.ring(), &me) {
            Ok(r) => r,
            Err(RingError::Revoked) => {
                log::warn!("mesh: this device was forgotten by its ring");
                keys::wipe_ring(&paths);
                None
            }
            Err(e) => {
                log::warn!("mesh: {e}; in no ring until paired again");
                None
            }
        };
        let ring_key = match (&ring, keys::read_key(&paths.ring_key())) {
            (Some(r), Ok(Some(k))) if k.public() == r.public => Some(k),
            (Some(_), Ok(Some(_))) => {
                log::warn!("mesh: ring.key is not the ring's; not the ring holder");
                None
            }
            (_, Err(e)) => {
                log::warn!("mesh: ring.key: {e}");
                None
            }
            _ => None,
        };
        if ring.is_some() {
            log::info!("mesh: in a ring{}", if ring_key.is_some() { " (its holder)" } else { "" });
        }
        let name = clean_name(&ctx.device_name);
        let state = State {
            name: if name.is_empty() { crate::default_name() } else { name.clone() },
            kind: ctx.kind,
            ring,
            ring_key,
            running: false,
            port: None,
            lan: None,
            subnets: Vec::new(),
            peers: HashMap::new(),
            table: Table::default(),
            known: HashMap::new(),
            pairing: MeshPairing::Idle,
            invite: None,
            code: None,
            error: None,
            next_link: 0,
        };
        // The pauses asked here come back before the pull loop starts.
        let replica = replica::ReplicaShared::load(paths.dir.join(replica::PAUSED));
        Ok(Self {
            paths,
            key,
            rt: ctx.runtime,
            api: ctx.api,
            wake: ctx.wake,
            version: ctx.version,
            headless: ctx.headless,
            default_name: if name.is_empty() { crate::default_name() } else { name },
            state: Mutex::new(state),
            changed: Notify::new(),
            ring_rev: watch::channel(0).0,
            presence: watch::channel(Presence::default()).0,
            node: Mutex::new(None),
            http: Mutex::new(HashMap::new()),
            http_in: Mutex::new((0, Vec::new())),
            streams: Mutex::new(HashMap::new()),
            library: ctx.library,
            replica,
            copies: Mutex::new(HashMap::new()),
        })
    }

    fn notify(&self) {
        self.changed.notify_one();
    }

    /// The map's numbers of what each device holds of its plan: kept for
    /// the snapshot, the runtime told when they change.
    fn set_copies(&self, copies: HashMap<String, (u64, u64)>) {
        let changed = {
            let mut c = self.copies.lock();
            if *c == copies {
                false
            } else {
                *c = copies;
                true
            }
        };
        if changed {
            self.notify();
        }
    }

    /// The ring changed: the links tell the others, and the tunnel
    /// connections of devices it no longer trusts as before are closed.
    fn bump_ring(&self) {
        self.ring_rev.send_modify(|n| *n = n.wrapping_add(1));
        self.recheck_http();
    }

    /// Closes the incoming tunnel connections of devices forgotten, or made
    /// guests since the connection opened (their running requests end too).
    fn recheck_http(&self) {
        let peers: Vec<PublicKey> = self.http_in.lock().1.iter().map(|h| h.peer).collect();
        if peers.is_empty() {
            return;
        }
        let now: HashMap<PublicKey, Option<MeshScope>> = {
            let s = self.state.lock();
            peers.iter().map(|p| (*p, s.ring.as_ref().and_then(|r| r.trusted(p)).map(|c| c.scope))).collect()
        };
        self.http_in.lock().1.retain(|h| {
            let keep = match now.get(&h.peer) {
                // Opened after the check: checked at its first request.
                None => true,
                Some(None) => false,
                Some(Some(scope)) => !(h.scope == MeshScope::Owner && *scope == MeshScope::Guest),
            };
            if !keep {
                log::info!("mesh: the tunnel of {} closed: the ring changed", h.peer.fmt_short());
                h.conn.close(VarInt::from_u32(3), b"ring changed");
            }
            keep
        });
    }

    fn track_http(&self, peer: &PublicKey, conn: &Connection, scope: MeshScope) -> u64 {
        let mut g = self.http_in.lock();
        g.0 += 1;
        let n = g.0;
        g.1.push(HttpIn { n, peer: *peer, conn: conn.clone(), scope });
        n
    }

    fn untrack_http(&self, n: u64) {
        self.http_in.lock().1.retain(|h| h.n != n);
    }

    fn stream_slots(&self, peer: &PublicKey) -> Arc<Semaphore> {
        self.streams.lock().entry(*peer).or_insert_with(|| Arc::new(Semaphore::new(MAX_STREAMS))).clone()
    }

    fn me(&self) -> Me {
        let s = self.state.lock();
        Me { id: self.key.public(), name: s.name.clone(), kind: s.kind }
    }

    pub(crate) fn online(&self, node_id: &str) -> bool {
        let Some(id) = parse_id(node_id) else { return false };
        self.state.lock().peers.get(&id).is_some_and(|p| p.link.is_some())
    }

    fn trusted_cert(&self, id: &PublicKey) -> Option<Cert> {
        self.state.lock().ring.as_ref().and_then(|r| r.trusted(id).cloned())
    }

    /// Where `id` can be dialled, addresses inside this device's home
    /// network subnets only: mDNS first, then the last address that worked
    /// or that it came from, then the runtime's (its node row).
    fn addrs_of(&self, id: &PublicKey) -> Option<Vec<SocketAddr>> {
        let s = self.state.lock();
        let mut out: Vec<SocketAddr> = s.table.by_id(id).map(|x| x.addrs.clone()).unwrap_or_default();
        out.extend(s.peers.get(id).and_then(|p| p.addr));
        out.extend(s.known.get(id).copied());
        out.retain(|a| discovery::on_lan(a.ip(), &s.subnets));
        let mut seen = HashSet::new();
        out.retain(|a| seen.insert(*a));
        (!out.is_empty()).then_some(out)
    }

    /// A ring member reached this device from `from`: kept as where it can
    /// be dialled when mDNS has nothing of it (the snapshot passes it to
    /// its node row). Only an address inside this device's home network
    /// subnets.
    fn remember_addr(&self, peer: &PublicKey, from: Option<SocketAddr>) {
        let Some(from) = from.map(|a| SocketAddr::new(a.ip().to_canonical(), a.port())) else { return };
        if self.trusted_cert(peer).is_none() {
            return;
        }
        let changed = {
            let mut s = self.state.lock();
            if !discovery::on_lan(from.ip(), &s.subnets) {
                return;
            }
            let p = s.peers.entry(*peer).or_default();
            let changed = p.addr != Some(from);
            p.addr = Some(from);
            changed
        };
        if changed {
            log::info!("mesh: {} reached this device from {from}", peer.fmt_short());
            self.notify();
        }
    }

    /// This device's presence as `peer` hears it: a guest's device hears
    /// nothing of what the owner's devices store or pause.
    fn presence_for(&self, peer: &PublicKey, mut p: Presence) -> Presence {
        let owner = self.trusted_cert(peer).is_some_and(|c| c.scope == MeshScope::Owner);
        if !owner {
            p.holder = false;
            p.assign.clear();
            p.pause.clear();
        }
        p
    }

    fn set_pairing(&self, p: MeshPairing) {
        self.state.lock().pairing = p;
        self.notify();
    }

    /// The code file of a headless instance, as the state is.
    fn write_code_file(&self) {
        if !self.headless {
            return;
        }
        let code = {
            let s = self.state.lock();
            (s.running && s.ring.is_none()).then(|| s.code.as_ref().map(|c| c.code.clone())).flatten()
        };
        keys::write_code(&self.paths, code.as_deref());
    }

    /// A new code to show (logged on a headless instance).
    fn fresh_code(&self, s: &mut State) {
        s.code = pair::new_code().map(|code| Code { code, tries: 0, busy: false });
        if self.headless
            && let Some(c) = &s.code
        {
            log::info!("mesh: pairing code {} (nikgobd pair-code shows it)", c.code);
        }
    }

    /// Makes the ring here when there is none (this device becomes its
    /// holder). Whether it was made now.
    fn ensure_ring(&self, s: &mut State) -> Result<bool, Refusal> {
        if s.ring.is_some() {
            return if s.ring_key.is_some() { Ok(false) } else { Err(Refusal::NotHolder) };
        }
        let rk = keys::new_key(&self.paths.ring_key()).map_err(|e| {
            log::error!("mesh: cannot write ring.key: {e}");
            Refusal::Internal
        })?;
        let ring = Ring::create(&rk, &self.key.public(), &s.name, s.kind, now_ms());
        if let Err(e) = ring.save(&self.paths.ring()) {
            log::error!("mesh: cannot write ring.json: {e}");
            return Err(Refusal::Internal);
        }
        log::info!("mesh: a new ring, this device holds it");
        s.ring = Some(ring);
        s.ring_key = Some(rk);
        s.code = None;
        Ok(true)
    }

    /// Writes the ring as it is now.
    fn save_ring(&self, s: &State) -> std::io::Result<()> {
        match &s.ring {
            Some(r) => r.save(&self.paths.ring()),
            None => Ok(()),
        }
    }

    /// Signs `joiner`'s certificate (holder).
    fn admit(&self, joiner: &PublicKey, name: &str, kind: MeshKind, invite: Option<&str>) -> Result<Grant, Refusal> {
        let mut made = false;
        let out = {
            let mut s = self.state.lock();
            self.admit_locked(&mut s, &mut made, joiner, name, kind, invite)
        };
        if made {
            self.reannounce();
        }
        if out.is_ok() {
            self.bump_ring();
            log::info!("mesh: {} ({}) joins the ring", clean_name(name), joiner.fmt_short());
        } else if made {
            self.bump_ring();
        }
        self.notify();
        out
    }

    /// Whether `key` opens `/install/` now: the download key of the invite
    /// held, alive and not spent, compared in constant time under the lock
    /// that spends it.
    pub(crate) fn install_key_ok(&self, key: &str) -> bool {
        invite_opens(self.state.lock().invite.as_ref(), key, now_ms())
    }

    fn admit_locked(&self, s: &mut State, made: &mut bool, joiner: &PublicKey, name: &str, kind: MeshKind, invite: Option<&str>) -> Result<Grant, Refusal> {
        if let Some(inv) = invite {
            // Checked again under the lock the grant is made under: an
            // exchange begun before the invite expired (or was spent) gets
            // nothing after.
            let live = s.invite.as_ref().is_some_and(|i| hex(&i.id) == inv && i.tries < pair::TRIES && now_ms() < i.expires_ms);
            if !live {
                return Err(Refusal::NoInvite);
            }
        }
        // The scope chosen when the invite was made, signed into the
        // certificate under the same lock: never an owner first and a guest
        // after.
        let scope = joiner_scope(invite, s.invite.as_ref());
        *made = self.ensure_ring(s)?;
        let Some(rk) = s.ring_key.clone() else { return Err(Refusal::NotHolder) };
        let Some(current) = s.ring.as_ref() else { return Err(Refusal::Internal) };
        if !current.can_admit(joiner) {
            log::warn!("mesh: the ring has no room for {}", joiner.fmt_short());
            return Err(Refusal::Full);
        }
        let mut ring = current.clone();
        let node = joiner.to_string();
        let after = ring.revoked.iter().filter(|r| r.node == node).map(|r| r.revoked_ms + 1).max().unwrap_or(0);
        let cert = Cert::issue(&rk, joiner, name, kind, scope, now_ms().max(after));
        ring.admit(cert.clone());
        // Granted only once the ring with it is on disk: never a certificate
        // the holder would no longer know after a restart.
        if let Err(e) = ring.save(&self.paths.ring()) {
            log::error!("mesh: cannot write ring.json: {e}");
            return Err(Refusal::Internal);
        }
        let grant = Grant { ring: ring.public.to_string(), cert, members: ring.members.clone(), revoked: ring.revoked.clone() };
        s.ring = Some(ring);
        if invite.is_some() {
            // Spent; the window that showed it hears of the device at once
            // (it closes as a success, asks no new invite).
            s.invite = None;
            s.pairing = MeshPairing::Done { node_id: node };
        }
        Ok(grant)
    }

    /// Keeps the ring a holder gave this device.
    fn accept_grant(&self, grant: Grant) -> Result<(), PairError> {
        let ring = Ring::received(&grant.ring, &self.key.public(), grant.cert, &grant.members, &grant.revoked).map_err(PairError::Ring)?;
        if self.state.lock().ring.is_some() {
            return Err(PairError::Refused(Refusal::Paired));
        }
        ring.save(&self.paths.ring()).map_err(|e| PairError::Ring(RingError::Unreadable(e.to_string())))?;
        {
            let mut s = self.state.lock();
            s.ring = Some(ring);
            s.ring_key = None;
            s.code = None;
        }
        self.write_code_file();
        self.reannounce();
        self.bump_ring();
        self.notify();
        Ok(())
    }

    /// This device was forgotten by its ring: its files go, it shows a
    /// code again.
    fn forgotten(&self) {
        log::warn!("mesh: this device was forgotten by its ring");
        keys::wipe_ring(&self.paths);
        {
            let mut s = self.state.lock();
            s.ring = None;
            s.ring_key = None;
            s.invite = None;
            s.peers.clear();
            self.fresh_code(&mut s);
        }
        self.http.lock().clear();
        self.write_code_file();
        self.bump_ring();
        self.reannounce();
        self.notify();
    }

    fn reannounce(&self) {
        let node = self.node.lock().clone();
        if let Some(node) = node {
            self.announce(&node);
        }
    }

    fn announce(&self, node: &Node) {
        let Some(d) = &node.mdns else { return };
        let (name, tag, kind) = {
            let s = self.state.lock();
            (s.name.clone(), s.ring.as_ref().map(Ring::tag), s.kind)
        };
        if let Some(old) = node.announced.lock().take() {
            let _ = d.unregister(&old);
        }
        // The home network's addresses only, told once when they change.
        let ips = discovery::lan_addrs();
        {
            let mut said = node.said.lock();
            if *said != ips {
                let list: Vec<String> = ips.iter().map(ToString::to_string).collect();
                log::info!("mesh: announced on the home network at {}", if list.is_empty() { "no address".to_owned() } else { list.join(", ") });
                *said = ips.clone();
            }
        }
        match discovery::announce(d, &name, &self.key.public(), tag.as_deref(), kind, node.port, &ips) {
            Ok(full) => *node.announced.lock() = Some(full),
            Err(e) => log::warn!("mesh: cannot announce on the network: {e}"),
        }
    }

    /// Follows the view: the switch, this device's name, the addresses the
    /// runtime keeps, this device's presence. Whether the mesh is wanted.
    fn follow_view(&self) -> bool {
        let kind = self.state.lock().kind;
        let (enabled, name, known, p) = {
            let st = self.rt.state();
            let set = clean_name(&st.mesh.name_setting);
            let name = if set.is_empty() { self.default_name.clone() } else { set };
            let known: Vec<(PublicKey, SocketAddr)> = st
                .mesh
                .nodes
                .iter()
                .filter_map(|n| Some((parse_id(&n.id)?, n.addr.as_deref()?.parse().ok()?)))
                .collect();
            let p = presence::of_view(&st, &name, kind, &self.version, now_ms());
            (st.mesh.enabled, name, known, p)
        };
        // The copies: this device's pull, and the devices whose pulls the
        // owner paused here.
        let mut p = p;
        p.transfer = self.replica.transfer();
        p.pause = self.replica.pause_list();
        let renamed = {
            let mut s = self.state.lock();
            s.known = known.into_iter().collect();
            let renamed = s.name != name;
            s.name = name;
            renamed
        };
        if renamed {
            self.reannounce();
            self.notify();
        }
        if self.presence.send_if_modified(|cur| {
            if presence::differs(cur, &p) {
                *cur = p;
                true
            } else {
                // The heartbeat carries the transfer's smaller steps.
                cur.transfer = p.transfer;
                false
            }
        }) {
            self.notify();
        }
        enabled
    }

    async fn start_node(self: &Arc<Self>) -> Result<(), String> {
        let ep = bind(&self.key, false).await?;
        let sockets = ep.bound_sockets();
        let port = sockets.iter().find(|a| a.is_ipv4()).or(sockets.first()).map_or(0, SocketAddr::port);
        let mdns = match ServiceDaemon::new() {
            Ok(d) => Some(d),
            Err(e) => {
                log::warn!("mesh: no mDNS: {e}");
                None
            }
        };
        // mDNS keeps quiet on the virtual adapters (VPNs, virtual machines'
        // and containers' switches).
        if let Some(d) = &mdns {
            let quiet: Vec<mdns_sd::IfKind> = discovery::quiet_ifaces().into_iter().map(mdns_sd::IfKind::Name).collect();
            if !quiet.is_empty()
                && let Err(e) = d.disable_interface(quiet)
            {
                log::warn!("mesh: mDNS on the virtual adapters not turned off: {e}");
            }
        }
        let node = Arc::new(Node { ep, mdns, announced: Mutex::new(None), said: Mutex::new(Vec::new()), stop: watch::channel(false).0, port });
        {
            let mut s = self.state.lock();
            s.running = true;
            s.port = Some(port);
            s.lan = lan_ip();
            s.subnets = discovery::lan_subnets();
            s.error = None;
            if s.ring.is_none() && s.code.is_none() {
                self.fresh_code(&mut s);
            }
        }
        *self.node.lock() = Some(node.clone());
        self.write_code_file();
        self.announce(&node);
        tokio::spawn(accept_loop(self.clone(), node.clone()));
        tokio::spawn(dial_loop(self.clone(), node.clone()));
        if let Some(d) = &node.mdns {
            match d.browse(discovery::SERVICE) {
                Ok(rx) => {
                    tokio::spawn(browse_loop(self.clone(), node.clone(), rx));
                }
                Err(e) => log::warn!("mesh: cannot browse the network: {e}"),
            }
        }
        log::info!("mesh: on, id {}, port {port}", self.key.public());
        self.notify();
        Ok(())
    }

    async fn stop_node(&self) {
        let node = self.node.lock().take();
        let Some(node) = node else { return };
        let _ = node.stop.send(true);
        if let Some(d) = &node.mdns {
            if let Some(full) = node.announced.lock().take() {
                let _ = d.unregister(&full);
            }
            let _ = d.shutdown();
        }
        self.http.lock().clear();
        let _ = tokio::time::timeout(Duration::from_secs(2), node.ep.close()).await;
        {
            let mut s = self.state.lock();
            s.running = false;
            s.port = None;
            s.table = Table::default();
            for p in s.peers.values_mut() {
                p.link = None;
                p.dialing = false;
            }
        }
        self.write_code_file();
        log::info!("mesh: off");
        self.notify();
    }

    /// What the runtime shows.
    pub(crate) fn snapshot(&self) -> MeshSnapshot {
        let own = self.presence.borrow().clone();
        let s = self.state.lock();
        let copies = self.copies.lock().clone();
        let me = self.key.public();
        let this = MeshNodeView {
            id: me.to_string(),
            name: s.name.clone(),
            kind: s.kind,
            scope: s.ring.as_ref().map_or(MeshScope::Owner, |r| r.me.scope),
            online: s.running,
            last_seen_ms: None,
            addr: s.port.zip(s.lan).map(|(p, ip)| SocketAddr::new(ip, p).to_string()),
            version: Some(self.version.clone()),
            storage: own.storage,
            playing: own.playing,
            transfer: self.replica.transfer(),
            ..MeshNodeView::default()
        };
        let members: Vec<Cert> = s.ring.as_ref().map(|r| r.others().cloned().collect()).unwrap_or_default();
        let ids: HashSet<String> = members.iter().map(|c| c.node.clone()).collect();
        let mut nodes = Vec::with_capacity(members.len());
        for c in members {
            let Some(id) = c.node_id() else { continue };
            let p = s.peers.get(&id);
            let pres = p.and_then(|p| p.presence.as_ref());
            let online = p.is_some_and(|p| p.link.is_some());
            let addr = s
                .table
                .by_id(&id)
                .and_then(|x| x.addrs.first().copied())
                .or_else(|| p.and_then(|p| p.addr))
                .or_else(|| s.known.get(&id).copied())
                .filter(|a| discovery::on_lan(a.ip(), &s.subnets));
            let view = MeshNodeView {
                id: c.node.clone(),
                name: pres.map(|p| p.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| c.name.clone()),
                kind: pres.map_or(c.kind, |p| p.kind),
                scope: c.scope,
                online,
                // To the minute: the heartbeats do not make a change each.
                last_seen_ms: p.and_then(|p| p.last_seen_ms).map(|t| t - t.rem_euclid(60_000)),
                addr: addr.map(|a| a.to_string()),
                version: pres.map(|p| p.version.clone()).filter(|v| !v.is_empty()),
                storage: pres.and_then(|p| p.storage),
                playing: if online { pres.and_then(|p| p.playing.clone()) } else { None },
                transfer: if online { pres.and_then(|p| p.transfer) } else { None },
                copy: copies.get(&id).copied(),
                ..MeshNodeView::default()
            };
            nodes.push(MeshSnapshotNode { view, cert: serde_json::to_string(&c).unwrap_or_default() });
        }
        let mut nearby: Vec<MeshNearbyView> = s
            .table
            .by_name
            .values()
            .filter(|x| x.txt.ring.is_none() && !ids.contains(&x.txt.id.to_string()))
            .map(|x| MeshNearbyView { id: x.txt.id.to_string(), name: x.name.clone(), kind: x.txt.kind, addr: x.addrs.first().map(ToString::to_string) })
            .collect();
        nearby.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        MeshSnapshot {
            running: s.running,
            this,
            nodes,
            nearby,
            pairing: s.pairing.clone(),
            invite: s.invite.as_ref().map(|i| MeshInvite { ticket: i.ticket.clone(), expires_ms: i.expires_ms, scope: i.scope }),
            ring_holder: s.ring.is_some() && s.ring_key.is_some(),
            paired: s.ring.is_some(),
            code: if s.running && s.ring.is_none() { s.code.as_ref().map(|c| c.code.clone()) } else { None },
            error: s.error.clone(),
        }
    }

    pub(crate) async fn command(self: &Arc<Self>, c: Command) {
        match c {
            Command::MeshCreateInvite { scope } => self.create_invite(scope),
            Command::MeshCancelInvite => {
                self.state.lock().invite = None;
                self.notify();
            }
            Command::MeshPairNearby { nearby_id, code } => {
                if self.state.lock().pairing == MeshPairing::Working {
                    return;
                }
                self.set_pairing(MeshPairing::Working);
                let sh = self.clone();
                tokio::spawn(async move {
                    let out = sh.pair_nearby(&nearby_id, &code).await;
                    sh.set_pairing(match out {
                        Ok(id) => MeshPairing::Done { node_id: id },
                        Err(NearbyFail::WrongCode) => MeshPairing::FailedWrongCode,
                        Err(NearbyFail::Said(why)) => MeshPairing::Failed(why),
                    });
                });
            }
            Command::MeshJoin { ticket } => {
                if self.state.lock().pairing == MeshPairing::Working {
                    return;
                }
                self.set_pairing(MeshPairing::Working);
                let sh = self.clone();
                tokio::spawn(async move {
                    let out = sh.join(&ticket).await;
                    sh.set_pairing(match out {
                        Ok((holder, _)) => MeshPairing::Done { node_id: holder },
                        Err(why) => MeshPairing::Failed(why),
                    });
                });
            }
            Command::MeshForget { node_id } => {
                if let Err(why) = self.forget(&node_id) {
                    log::warn!("mesh: forgetting {node_id} failed: {why}");
                    self.rt.send(Command::MeshForgetFailed { node_id, why });
                }
            }
            Command::MeshSetScope { node_id, scope } => self.set_scope(&node_id, scope),
            Command::MeshClearPairing => self.set_pairing(MeshPairing::Idle),
            // The pause of a device's copies: this device's own, or told
            // to the other through the presence.
            Command::MeshPauseTransfer { node_id, paused } => {
                self.replica.set_paused(&node_id, paused);
                self.notify();
            }
            Command::ReplicaRecheck { card, hashes, need, leaving } => {
                tokio::spawn(replica::recheck(self.clone(), card, hashes, need, leaving));
            }
            other => log::debug!("mesh: not a command of the mesh: {other:?}"),
        }
    }

    fn create_invite(&self, scope: MeshScope) {
        let s = mesh_in(self.rt.state().prefs.resolved_locale());
        let node = self.node.lock().clone();
        let Some(node) = node else {
            self.set_pairing(MeshPairing::Failed(s.mesh_off.to_owned()));
            return;
        };
        let (Ok(a), Ok(b)) = (keys::random32(), keys::random32()) else {
            self.set_pairing(MeshPairing::Failed(s.invite_not_made.to_owned()));
            return;
        };
        let made;
        {
            let mut st = self.state.lock();
            made = match self.ensure_ring(&mut st) {
                Ok(m) => m,
                Err(Refusal::NotHolder) => {
                    st.pairing = MeshPairing::Failed(s.only_holder_adds.to_owned());
                    drop(st);
                    self.notify();
                    return;
                }
                Err(_) => {
                    st.pairing = MeshPairing::Failed(s.ring_not_made.to_owned());
                    drop(st);
                    self.notify();
                    return;
                }
            };
            let mut id = [0u8; 8];
            id.copy_from_slice(&a[..8]);
            let mut secret = [0u8; 16];
            secret.copy_from_slice(&b[..16]);
            let ip = st.lan.or_else(lan_ip).unwrap_or(IpAddr::from([127, 0, 0, 1]));
            let expires_ms = now_ms() + ticket::LIFETIME_MS;
            let t = Ticket { holder: self.key.public(), invite: id, secret, expires_ms, addrs: vec![SocketAddr::new(ip, node.port)] };
            let ticket = t.encode();
            let key = nikgob_core::releases::download_key(&ticket);
            st.invite = Some(Invite { id, secret, expires_ms, tries: 0, ticket, key, scope });
            st.pairing = MeshPairing::Idle;
        }
        if made {
            self.reannounce();
            self.bump_ring();
        }
        self.notify();
    }

    /// «Подключить» with the code a nearby device shows. Its id, or why it
    /// would not (a wrong code apart from the words said).
    async fn pair_nearby(self: &Arc<Self>, nearby_id: &str, code: &str) -> Result<String, NearbyFail> {
        let loc = self.rt.state().prefs.resolved_locale();
        let s = mesh_in(loc);
        let node = self.node.lock().clone().ok_or_else(|| NearbyFail::Said(s.mesh_off.to_owned()))?;
        let id = parse_id(nearby_id).ok_or_else(|| NearbyFail::Said(s.no_such_device.to_owned()))?;
        let code = code.trim();
        if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err(NearbyFail::Said(s.code_six_digits.to_owned()));
        }
        {
            let st = self.state.lock();
            if st.ring.is_some() && st.ring_key.is_none() {
                return Err(NearbyFail::Said(PairError::Refused(Refusal::NotHolder).user_message_in(loc)));
            }
        }
        let addrs = {
            let st = self.state.lock();
            let on_lan = |a: &&SocketAddr| discovery::on_lan(a.ip(), &st.subnets);
            st.table.by_id(&id).map(|x| x.addrs.iter().filter(on_lan).copied().collect::<Vec<SocketAddr>>()).filter(|a| !a.is_empty())
        }
        .ok_or_else(|| NearbyFail::Said(s.device_gone.to_owned()))?;
        let conn = connect(&node.ep, id, &addrs, ALPN_PAIR).await.map_err(|e| NearbyFail::Said(PairError::Io(e).user_message_in(loc)))?;
        let me = self.me();
        let (mut send, mut recv) = conn.open_bi().await.map_err(|e| NearbyFail::Said(PairError::Io(e.to_string()).user_message_in(loc)))?;
        let host = HostRef(self.clone());
        let admit = |name: &str, kind: MeshKind| host.admit(&id, name, kind, None);
        let out = match tokio::time::timeout(PAIR_TIMEOUT, pair::offer(&mut recv, &mut send, &me, &id, code, admit)).await {
            Ok(r) => r,
            Err(_) => Err(PairError::Io("timed out".to_owned())),
        };
        wind_down(&conn, &mut send, &mut recv).await;
        match out {
            Ok((name, _)) => {
                log::info!("mesh: paired {name}");
                Ok(id.to_string())
            }
            Err(e) => {
                log::info!("mesh: pairing {} failed: {e}", id.fmt_short());
                Err(match e {
                    PairError::Wrong | PairError::Refused(Refusal::WrongCode) => NearbyFail::WrongCode,
                    other => NearbyFail::Said(other.user_message_in(loc)),
                })
            }
        }
    }

    /// Joins the ring of a ticket's holder: its id and name.
    async fn join(self: &Arc<Self>, text: &str) -> Result<(String, String), String> {
        let loc = self.rt.state().prefs.resolved_locale();
        let s = mesh_in(loc);
        let t = ticket_on_lan(loc, text)?;
        if self.state.lock().ring.is_some() {
            return Err(s.already_paired.to_owned());
        }
        let node = self.node.lock().clone().ok_or_else(|| s.mesh_off.to_owned())?;
        let me = self.me();
        let (grant, holder_name) = join_ticket(&node.ep, &me, &t).await.map_err(|e| {
            log::info!("mesh: joining failed: {e}");
            e.user_message_in(loc)
        })?;
        self.accept_grant(grant).map_err(|e| e.user_message_in(loc))?;
        log::info!("mesh: joined the ring of {holder_name}");
        Ok((t.holder.to_string(), holder_name))
    }

    /// «Забыть» (holder): a revocation every member hears of, on disk before
    /// it counts. Why it could not be done, in words.
    fn forget(&self, node_id: &str) -> Result<(), String> {
        let s = mesh_in(self.rt.state().prefs.resolved_locale());
        let id = parse_id(node_id).ok_or_else(|| s.forget_no_device.to_owned())?;
        {
            let mut st = self.state.lock();
            let Some(rk) = st.ring_key.clone() else { return Err(s.forget_only_holder.to_owned()) };
            let Some(current) = st.ring.as_ref() else { return Err(s.forget_not_linked.to_owned()) };
            let mut ring = current.clone();
            if !ring.revoke(&rk, &id, now_ms()) {
                // Not a member (forgotten already): nothing left to do.
                return Ok(());
            }
            if let Err(e) = ring.save(&self.paths.ring()) {
                log::error!("mesh: cannot write ring.json: {e}");
                return Err(s.forget_ring_disk.to_owned());
            }
            st.ring = Some(ring);
        }
        self.http.lock().remove(&id);
        log::info!("mesh: {} forgotten", id.fmt_short());
        self.bump_ring();
        self.notify();
        Ok(())
    }

    /// «Свой» / «Гость» (holder): the device's certificate signed anew.
    fn set_scope(&self, node_id: &str, scope: MeshScope) {
        let Some(id) = parse_id(node_id) else { return };
        {
            let mut s = self.state.lock();
            let Some(rk) = s.ring_key.clone() else { return };
            let Some(current) = s.ring.as_ref() else { return };
            let Some(old) = current.trusted(&id).cloned() else { return };
            if old.scope == scope {
                return;
            }
            let mut ring = current.clone();
            ring.admit(Cert::issue(&rk, &id, &old.name, old.kind, scope, now_ms().max(old.issued_ms + 1)));
            if let Err(e) = ring.save(&self.paths.ring()) {
                log::error!("mesh: cannot write ring.json: {e}; the scope stays");
                return;
            }
            s.ring = Some(ring);
        }
        self.bump_ring();
        self.notify();
    }

    /// The loop's tick: an invite that expired goes; on a headless
    /// instance, a ticket handed over by `nikgobd pair`.
    async fn tick(self: &Arc<Self>) {
        let running = self.state.lock().running;
        let all = if running { discovery::lan_addrs() } else { Vec::new() };
        let subnets = if running { discovery::lan_subnets() } else { Vec::new() };
        let lan = all.iter().copied().find(IpAddr::is_ipv4).or_else(|| all.first().copied());
        // The home network's addresses changed (another network, a new
        // lease): announced again.
        let node = self.node.lock().clone();
        if let Some(node) = node
            && *node.said.lock() != all
        {
            self.announce(&node);
        }
        let expired = {
            let mut s = self.state.lock();
            s.lan = lan;
            s.subnets = subnets;
            if s.invite.as_ref().is_some_and(|i| now_ms() >= i.expires_ms) {
                s.invite = None;
                true
            } else {
                false
            }
        };
        if expired {
            self.notify();
        }
        if !self.headless {
            return;
        }
        let request = self.paths.dir.join(REQUEST_FILE);
        let Ok(text) = std::fs::read_to_string(&request) else { return };
        let _ = std::fs::remove_file(&request);
        let sh = self.clone();
        tokio::spawn(async move {
            let answer = match sh.join(&text).await {
                Ok((_, name)) => format!("ok\n{name}\n"),
                Err(why) => format!("error\n{why}\n"),
            };
            if let Err(e) = keys::write_atomic(&sh.paths.dir.join(RESULT_FILE), answer.as_bytes(), true) {
                log::warn!("mesh: cannot answer nikgobd pair: {e}");
            }
        });
    }

    // ------------------------------------------------------------ links

    fn hello(&self, peer: &PublicKey) -> Option<Ctl> {
        let presence = self.presence_for(peer, self.presence.borrow().clone());
        let s = self.state.lock();
        let r = s.ring.as_ref()?;
        Some(Ctl::Hello { ring: r.public.to_string(), certs: r.members.clone(), revoked: r.revoked.clone(), presence })
    }

    fn ring_msg(&self) -> Option<Ctl> {
        let s = self.state.lock();
        s.ring.as_ref().map(|r| Ctl::Ring { certs: r.members.clone(), revoked: r.revoked.clone() })
    }

    /// Takes a peer's news of the ring. Whether the link goes on (the peer
    /// still trusted, this device not forgotten).
    fn ring_news(&self, peer: &PublicKey, certs: &[Cert], revoked: &[Revocation]) -> bool {
        let (changed, me_revoked, trusted) = {
            let mut s = self.state.lock();
            let Some(r) = s.ring.as_mut() else { return false };
            let changed = r.merge(certs, revoked);
            let out = (changed, r.me_revoked(), r.trusted(peer).is_some());
            // News from the ring counts even when it cannot be written (a
            // revocation must not wait for the disk); the others tell it
            // again after a restart.
            if changed && let Err(e) = self.save_ring(&s) {
                log::error!("mesh: cannot write ring.json: {e}");
            }
            out
        };
        if me_revoked {
            self.forgotten();
            return false;
        }
        if changed {
            self.bump_ring();
            self.notify();
        }
        trusted
    }

    /// A peer's first message: the same ring, a member's certificate.
    fn check_hello(&self, peer: &PublicKey, msg: Ctl) -> Result<(), Ctl> {
        let refuse = |reason: &str, revoked: Vec<Revocation>| Ctl::Refuse { reason: reason.to_owned(), revoked };
        let Ctl::Hello { ring, certs, revoked, presence } = msg else { return Err(refuse("protocol", Vec::new())) };
        {
            let s = self.state.lock();
            let Some(r) = s.ring.as_ref() else { return Err(refuse("in no ring", Vec::new())) };
            if parse_id(&ring) != Some(r.public) {
                return Err(refuse("another ring", Vec::new()));
            }
        }
        if !self.ring_news(peer, &certs, &revoked) {
            let revoked = self.state.lock().ring.as_ref().map(|r| r.revoked.clone()).unwrap_or_default();
            return Err(refuse("not a member", revoked));
        }
        {
            let mut s = self.state.lock();
            let p = s.peers.entry(*peer).or_default();
            p.presence = Some(presence);
            p.last_seen_ms = Some(now_ms());
        }
        self.notify();
        Ok(())
    }

    /// A refusal's revocations: this device may have been forgotten.
    fn refused(&self, revoked: &[Revocation]) {
        let (changed, me_revoked) = {
            let mut s = self.state.lock();
            let Some(r) = s.ring.as_mut() else { return };
            let changed = r.merge(&[], revoked);
            let me_revoked = r.me_revoked();
            if changed && let Err(e) = self.save_ring(&s) {
                log::error!("mesh: cannot write ring.json: {e}");
            }
            (changed, me_revoked)
        };
        if me_revoked {
            self.forgotten();
        } else if changed {
            self.bump_ring();
            self.notify();
        }
    }

    /// A new link with `peer`: its number, or `None` when the link there is
    /// the one to keep (the one dialled by the smaller id).
    fn register_link(&self, peer: &PublicKey, outgoing: bool) -> Option<u64> {
        let me = self.key.public();
        let preferred = |out: bool| if out { me < *peer } else { *peer < me };
        let mut s = self.state.lock();
        s.next_link += 1;
        let n = s.next_link;
        let p = s.peers.entry(*peer).or_default();
        if let Some((_, existing_out)) = p.link
            && preferred(existing_out)
            && !preferred(outgoing)
        {
            return None;
        }
        p.link = Some((n, outgoing));
        p.dialing = false;
        p.fails = 0;
        p.retry_at = None;
        p.wanted_since = None;
        p.last_seen_ms = Some(now_ms());
        drop(s);
        self.notify();
        Some(n)
    }

    fn link_current(&self, peer: &PublicKey, n: u64) -> bool {
        self.state.lock().peers.get(peer).and_then(|p| p.link).is_some_and(|(m, _)| m == n)
    }

    fn link_ended(&self, peer: &PublicKey, n: u64) {
        let ended = {
            let mut s = self.state.lock();
            match s.peers.get_mut(peer) {
                Some(p) if p.link.is_some_and(|(m, _)| m == n) => {
                    p.link = None;
                    true
                }
                _ => false,
            }
        };
        if ended {
            log::info!("mesh: {} disconnected", peer.fmt_short());
            self.notify();
        }
    }

    fn peer_presence(&self, peer: &PublicKey, presence: Presence) {
        let changed = {
            let mut s = self.state.lock();
            let p = s.peers.entry(*peer).or_default();
            let changed = p.presence.as_ref().is_none_or(|old| presence::differs(old, &presence));
            p.presence = Some(presence);
            changed
        };
        if changed {
            self.notify();
        }
    }

    fn peer_heard(&self, peer: &PublicKey) {
        if let Some(p) = self.state.lock().peers.get_mut(peer) {
            p.last_seen_ms = Some(now_ms());
        }
    }

    fn dial_failed(&self, peer: &PublicKey) {
        let mut s = self.state.lock();
        let p = s.peers.entry(*peer).or_default();
        p.dialing = false;
        p.fails = p.fails.saturating_add(1);
        let wait = Duration::from_secs(1u64 << p.fails.min(6)).min(BACKOFF_MAX);
        p.retry_at = Some(Instant::now() + wait);
    }

    /// The paired devices to dial now (each marked as being dialled).
    fn to_dial(&self) -> Vec<(PublicKey, Vec<SocketAddr>)> {
        let me = self.key.public();
        let now = Instant::now();
        let members: Vec<PublicKey> = {
            let s = self.state.lock();
            let Some(r) = s.ring.as_ref() else { return Vec::new() };
            r.others().filter_map(Cert::node_id).collect()
        };
        let mut out = Vec::new();
        for id in members {
            let Some(addrs) = self.addrs_of(&id) else { continue };
            let mut s = self.state.lock();
            let p = s.peers.entry(id).or_default();
            if p.link.is_some() || p.dialing {
                p.wanted_since = None;
                continue;
            }
            if p.retry_at.is_some_and(|t| now < t) {
                continue;
            }
            let since = *p.wanted_since.get_or_insert(now);
            if !(me < id || now.saturating_duration_since(since) >= TIE_WAIT) {
                continue;
            }
            p.dialing = true;
            if p.addr.is_none() {
                p.addr = addrs.first().copied();
            }
            out.push((id, addrs));
        }
        out
    }

    async fn dial(self: Arc<Self>, node: Arc<Node>, id: PublicKey, addrs: Vec<SocketAddr>) {
        let opened: Result<(Connection, SendStream, RecvStream), String> = async {
            let conn = connect(&node.ep, id, &addrs, ALPN_CTL).await?;
            let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
            let hello = self.hello(&id).ok_or("in no ring")?;
            write_frame(&mut send, &hello).await.map_err(|e| e.to_string())?;
            let reply: Ctl = match tokio::time::timeout(HANDSHAKE, read_frame(&mut recv)).await {
                Ok(r) => r.map_err(|e| e.to_string())?,
                Err(_) => return Err("timed out".to_owned()),
            };
            match reply {
                Ctl::Refuse { reason, revoked } => {
                    self.refused(&revoked);
                    conn.close(VarInt::from_u32(0), b"refused");
                    return Err(format!("refused: {reason}"));
                }
                hello @ Ctl::Hello { .. } => {
                    if self.check_hello(&id, hello).is_err() {
                        conn.close(VarInt::from_u32(0), b"refused");
                        return Err("not a member".to_owned());
                    }
                }
                _ => return Err("protocol".to_owned()),
            }
            Ok((conn, send, recv))
        }
        .await;
        match opened {
            Ok((conn, send, recv)) => {
                if let Some(a) = addrs.first()
                    && let Some(p) = self.state.lock().peers.get_mut(&id)
                {
                    p.addr = Some(*a);
                }
                self.run_link(node, conn, id, send, recv, true).await;
            }
            Err(e) => {
                log::debug!("mesh: dialling {} failed: {e}", id.fmt_short());
                self.dial_failed(&id);
            }
        }
    }

    async fn accept_link(self: Arc<Self>, node: Arc<Node>, conn: Connection, peer: PublicKey, permit: OwnedSemaphorePermit, from: Option<SocketAddr>) {
        let Ok(Ok((mut send, mut recv))) = tokio::time::timeout(HANDSHAKE, conn.accept_bi()).await else {
            conn.close(VarInt::from_u32(0), b"timeout");
            return;
        };
        let hello: Ctl = match tokio::time::timeout(HANDSHAKE, read_frame(&mut recv)).await {
            Ok(Ok(h)) => h,
            _ => {
                conn.close(VarInt::from_u32(0), b"protocol");
                return;
            }
        };
        if let Err(refusal) = self.check_hello(&peer, hello) {
            let _ = write_frame(&mut send, &refusal).await;
            wind_down(&conn, &mut send, &mut recv).await;
            return;
        }
        self.remember_addr(&peer, from);
        let Some(mine) = self.hello(&peer) else { return };
        if write_frame(&mut send, &mine).await.is_err() {
            return;
        }
        drop(permit);
        self.run_link(node, conn, peer, send, recv, false).await;
    }

    async fn run_link(self: Arc<Self>, node: Arc<Node>, conn: Connection, peer: PublicKey, mut send: SendStream, mut recv: RecvStream, outgoing: bool) {
        let Some(n) = self.register_link(&peer, outgoing) else {
            conn.close(VarInt::from_u32(0), b"duplicate");
            return;
        };
        log::info!("mesh: connected to {}", peer.fmt_short());
        let (tx, mut rx) = mpsc::channel::<Ctl>(16);
        let reader = tokio::spawn(async move {
            while let Ok(f) = read_frame::<_, Ctl>(&mut recv).await {
                if tx.send(f).await.is_err() {
                    break;
                }
            }
        });
        let mut pres = self.presence.subscribe();
        let mut ring_rx = self.ring_rev.subscribe();
        let mut stop = node.stop.subscribe();
        let mut beat = tokio::time::interval(HEARTBEAT);
        beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut heard = Instant::now();
        loop {
            tokio::select! {
                f = rx.recv() => {
                    let Some(f) = f else { break };
                    heard = Instant::now();
                    self.peer_heard(&peer);
                    match f {
                        Ctl::Presence { presence } => self.peer_presence(&peer, presence),
                        Ctl::Ring { certs, revoked } => {
                            if !self.ring_news(&peer, &certs, &revoked) {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                r = pres.changed() => {
                    if r.is_err() {
                        break;
                    }
                    let p = self.presence_for(&peer, pres.borrow_and_update().clone());
                    if write_frame(&mut send, &Ctl::Presence { presence: p }).await.is_err() {
                        break;
                    }
                }
                r = ring_rx.changed() => {
                    if r.is_err() {
                        break;
                    }
                    ring_rx.borrow_and_update();
                    let Some(msg) = self.ring_msg() else { break };
                    if write_frame(&mut send, &msg).await.is_err() {
                        break;
                    }
                    if self.trusted_cert(&peer).is_none() {
                        // Forgotten: it hears of its revocation first.
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        break;
                    }
                }
                _ = beat.tick() => {
                    if heard.elapsed() > OFFLINE_AFTER {
                        log::info!("mesh: {} went quiet", peer.fmt_short());
                        break;
                    }
                    if !self.link_current(&peer, n) {
                        break;
                    }
                    let p = self.presence_for(&peer, self.presence.borrow().clone());
                    if write_frame(&mut send, &Ctl::Presence { presence: p }).await.is_err() {
                        break;
                    }
                }
                _ = stop.changed() => break,
            }
        }
        reader.abort();
        conn.close(VarInt::from_u32(0), b"bye");
        self.link_ended(&peer, n);
    }

    // ------------------------------------------------------------ incoming

    /// An incoming connection, set up; `permit` is its place among those
    /// being set up, held until its handshake is done.
    /// `from`: the address it came from (kept for a ring member as where
    /// it can be dialled).
    async fn incoming(self: Arc<Self>, node: Arc<Node>, conn: Connection, permit: OwnedSemaphorePermit, from: Option<SocketAddr>) {
        let peer = conn.remote_id();
        let alpn = conn.alpn().to_vec();
        if alpn == ALPN_PAIR {
            self.clone().serve_pair(conn, peer).await;
            drop(permit);
            self.remember_addr(&peer, from);
        } else if alpn == ALPN_CTL {
            self.accept_link(node, conn, peer, permit, from).await;
        } else if alpn == ALPN_HTTP {
            self.remember_addr(&peer, from);
            self.serve_http(conn, peer, permit).await;
        } else {
            conn.close(VarInt::from_u32(1), b"alpn");
        }
    }

    async fn serve_pair(self: Arc<Self>, conn: Connection, peer: PublicKey) {
        let Ok(Ok((mut send, mut recv))) = tokio::time::timeout(HANDSHAKE, conn.accept_bi()).await else {
            conn.close(VarInt::from_u32(0), b"timeout");
            return;
        };
        let me = self.me();
        let host = HostRef(self.clone());
        let out = match tokio::time::timeout(PAIR_TIMEOUT, pair::serve(&mut recv, &mut send, &me, &peer, &host)).await {
            Ok(r) => r,
            Err(_) => Err(PairError::Io("timed out".to_owned())),
        };
        wind_down(&conn, &mut send, &mut recv).await;
        match out {
            Ok(Served::Admitted { node, name, .. }) => {
                log::info!("mesh: {name} paired with a ticket");
                self.set_pairing(MeshPairing::Done { node_id: node.to_string() });
            }
            Ok(Served::Joined { holder, holder_name }) => {
                log::info!("mesh: joined the ring of {holder_name}");
                self.set_pairing(MeshPairing::Done { node_id: holder.to_string() });
            }
            Err(e) => log::info!("mesh: a pairing from {} failed: {e}", peer.fmt_short()),
        }
    }

    async fn serve_http(self: Arc<Self>, conn: Connection, peer: PublicKey, permit: OwnedSemaphorePermit) {
        let Some(api) = self.api.clone() else {
            conn.close(VarInt::from_u32(2), b"no api");
            return;
        };
        let Some(first) = self.trusted_cert(&peer) else {
            conn.close(VarInt::from_u32(3), b"not a member");
            return;
        };
        drop(permit);
        let n = self.track_http(&peer, &conn, first.scope);
        let slots = self.stream_slots(&peer);
        loop {
            // A place among the device's running requests first: past
            // [`MAX_STREAMS`] its next stream waits (QUIC holds it back).
            let Ok(slot) = slots.clone().acquire_owned().await else { break };
            let Ok((send, recv)) = conn.accept_bi().await else { break };
            // The ring is asked at each request, not once per connection: a
            // device forgotten or made a guest since gets no more than the
            // ring allows now.
            let Some(cert) = self.trusted_cert(&peer) else {
                conn.close(VarInt::from_u32(3), b"not a member");
                break;
            };
            let api = api.clone();
            tokio::spawn(async move {
                tunnel::serve(recv, send, api, tunnel::caller_of(&cert)).await;
                drop(slot);
            });
        }
        self.untrack_http(n);
    }

    // ------------------------------------------------------------ tunnel

    async fn http_conn(&self, node: &Node, id: PublicKey) -> Result<Connection, MeshError> {
        let cached = self.http.lock().get(&id).cloned();
        if let Some(c) = cached
            && c.close_reason().is_none()
        {
            return Ok(c);
        }
        let addrs = self.addrs_of(&id).ok_or_else(|| MeshError::Unreachable("no address".to_owned()))?;
        let c = connect(&node.ep, id, &addrs, ALPN_HTTP).await.map_err(MeshError::Unreachable)?;
        self.http.lock().insert(id, c.clone());
        Ok(c)
    }

    pub(crate) async fn request(self: &Arc<Self>, node_id: &str, req: http::Request<Body>) -> Result<http::Response<Body>, MeshError> {
        let id = parse_id(node_id).ok_or(MeshError::NotPaired)?;
        if self.trusted_cert(&id).is_none() {
            return Err(MeshError::NotPaired);
        }
        let node = self.node.lock().clone().ok_or(MeshError::Stopped)?;
        let conn = self.http_conn(&node, id).await?;
        let (send, recv) = match conn.open_bi().await {
            Ok(s) => s,
            Err(_) => {
                // The kept connection had gone: once more with a new one.
                self.http.lock().remove(&id);
                let conn = self.http_conn(&node, id).await?;
                conn.open_bi().await.map_err(|e| MeshError::Unreachable(e.to_string()))?
            }
        };
        tunnel::request(recv, send, req).await.map_err(MeshError::Http)
    }
}

/// The pairing's side of the service.
struct HostRef(Arc<Shared>);

impl PairHost for HostRef {
    fn invite_secret(&self, invite: &str) -> Result<Vec<u8>, Refusal> {
        let s = self.0.state.lock();
        if s.ring.is_some() && s.ring_key.is_none() {
            return Err(Refusal::NotHolder);
        }
        match &s.invite {
            Some(i) if hex(&i.id) == invite && i.tries < pair::TRIES && now_ms() < i.expires_ms => Ok(i.secret.to_vec()),
            _ => Err(Refusal::NoInvite),
        }
    }

    fn invite_failed(&self, invite: &str) {
        {
            let mut s = self.0.state.lock();
            let spent = match s.invite.as_mut() {
                Some(i) if hex(&i.id) == invite => {
                    i.tries += 1;
                    i.tries >= pair::TRIES
                }
                _ => false,
            };
            if spent {
                log::warn!("mesh: an invite spent by wrong tries");
                s.invite = None;
                s.pairing = MeshPairing::Failed(mesh_in(self.0.rt.state().prefs.resolved_locale()).invite_cancelled.to_owned());
            }
        }
        self.0.notify();
    }

    fn admit(&self, joiner: &PublicKey, name: &str, kind: MeshKind, invite: Option<&str>) -> Result<Grant, Refusal> {
        self.0.admit(joiner, name, kind, invite)
    }

    fn take_code(&self) -> Result<String, Refusal> {
        let mut s = self.0.state.lock();
        if s.ring.is_some() {
            return Err(Refusal::Paired);
        }
        match s.code.as_mut() {
            Some(c) if !c.busy => {
                c.busy = true;
                Ok(c.code.clone())
            }
            Some(_) => Err(Refusal::Busy),
            None => Err(Refusal::Internal),
        }
    }

    fn code_done(&self, ok: bool) {
        {
            let mut s = self.0.state.lock();
            let renew = match s.code.as_mut() {
                Some(c) => {
                    c.busy = false;
                    if !ok {
                        c.tries += 1;
                    }
                    !ok && c.tries >= pair::TRIES
                }
                None => false,
            };
            if renew {
                log::warn!("mesh: {} wrong codes: a new code", pair::TRIES);
                self.0.fresh_code(&mut s);
            }
        }
        self.0.write_code_file();
        self.0.notify();
    }

    fn accept(&self, grant: Grant, _holder: &PublicKey, _holder_name: &str) -> Result<(), PairError> {
        self.0.accept_grant(grant)
    }
}

async fn accept_loop(sh: Arc<Shared>, node: Arc<Node>) {
    let mut stop = node.stop.subscribe();
    let setting_up = Arc::new(Semaphore::new(MAX_HANDSHAKES));
    loop {
        let incoming = tokio::select! {
            i = node.ep.accept() => i,
            _ = stop.changed() => break,
        };
        let Some(incoming) = incoming else { break };
        // Too many half-open connections (a flood from the network): this
        // one is turned away (dropped unanswered) instead of waiting.
        let Ok(permit) = setting_up.clone().try_acquire_owned() else {
            log::debug!("mesh: too many connections being set up; one turned away");
            drop(incoming);
            continue;
        };
        let from = match incoming.remote_addr() {
            IncomingAddr::Ip(a) => Some(a),
            _ => None,
        };
        let (sh, node) = (sh.clone(), node.clone());
        tokio::spawn(async move {
            match tokio::time::timeout(HANDSHAKE, incoming).await {
                Ok(Ok(conn)) => sh.incoming(node, conn, permit, from).await,
                Ok(Err(e)) => log::debug!("mesh: an incoming connection failed: {e}"),
                Err(_) => log::debug!("mesh: an incoming connection timed out"),
            }
        });
    }
}

async fn dial_loop(sh: Arc<Shared>, node: Arc<Node>) {
    let mut stop = node.stop.subscribe();
    let mut every = tokio::time::interval(DIAL_EVERY);
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = every.tick() => {}
            _ = stop.changed() => break,
        }
        for (id, addrs) in sh.to_dial() {
            tokio::spawn(sh.clone().dial(node.clone(), id, addrs));
        }
        // Entries not heard of for a while: asked whether they still exist
        // (the daemon reports them removed when they do not answer).
        let stale = sh.state.lock().table.to_verify(Instant::now());
        if let Some(d) = &node.mdns {
            for full in stale {
                let _ = d.verify(full, Duration::from_secs(3));
            }
        }
    }
}

async fn browse_loop(sh: Arc<Shared>, node: Arc<Node>, rx: mdns_sd::Receiver<ServiceEvent>) {
    let mut stop = node.stop.subscribe();
    let me = sh.key.public();
    loop {
        let ev = tokio::select! {
            e = rx.recv_async() => e,
            _ = stop.changed() => break,
        };
        let Ok(ev) = ev else { break };
        let change = {
            let mut s = sh.state.lock();
            discovery::apply(&mut s.table, ev, &me)
        };
        if change == Change::Changed {
            sh.notify();
        }
    }
}

/// Reports the snapshot when it changed, at most every [`PUBLISH_GAP`].
async fn publish_loop(sh: Arc<Shared>) {
    let mut last: Option<MeshSnapshot> = None;
    loop {
        sh.changed.notified().await;
        tokio::time::sleep(PUBLISH_GAP).await;
        let snap = sh.snapshot();
        if last.as_ref() != Some(&snap) {
            sh.rt.send(Command::MeshUpdate(Box::new(snap.clone())));
            last = Some(snap);
        }
    }
}

/// The mesh's loop: on while the view says so, the commands, the tick.
pub(crate) async fn run(sh: Arc<Shared>, mut cmds: mpsc::Receiver<Command>, mut stop: oneshot::Receiver<()>) {
    let mut changes = sh.wake.subscribe();
    let publisher = tokio::spawn(publish_loop(sh.clone()));
    let syncer = tokio::spawn(sync::run(sh.clone()));
    let replicator = tokio::spawn(replica::run(sh.clone()));
    let auditor = tokio::spawn(replica::audit(sh.clone()));
    let copier = tokio::spawn(replica::count_copies(sh.clone()));
    let mut retry_at: Option<Instant> = None;
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    sh.notify();
    loop {
        let want = sh.follow_view();
        let running = sh.node.lock().is_some();
        if want && !running && retry_at.is_none_or(|t| Instant::now() >= t) {
            match sh.start_node().await {
                Ok(()) => retry_at = None,
                Err(e) => {
                    log::warn!("mesh: cannot start: {e}");
                    sh.state.lock().error = Some(fmt_open_net_in(sh.rt.state().prefs.resolved_locale(), &e));
                    sh.notify();
                    retry_at = Some(Instant::now() + RETRY);
                }
            }
        } else if !want && running {
            sh.stop_node().await;
            retry_at = None;
        } else if !want && sh.state.lock().error.take().is_some() {
            sh.notify();
        }
        tokio::select! {
            _ = &mut stop => break,
            r = changes.changed() => {
                if r.is_err() {
                    break;
                }
            }
            c = cmds.recv() => match c {
                Some(c) => sh.command(c).await,
                None => break,
            },
            _ = tick.tick() => sh.tick().await,
        }
    }
    // What the runtime passed on before the stop (the forgets of a player
    // closing, [`crate::MeshHandle::stop`]): done before the endpoint goes,
    // so their revocations are on disk; the others hear of them at the next
    // start.
    while let Ok(c) = cmds.try_recv() {
        if matches!(c, Command::MeshForget { .. } | Command::MeshSetScope { .. }) {
            sh.command(c).await;
        }
    }
    syncer.abort();
    replicator.abort();
    auditor.abort();
    copier.abort();
    sh.stop_node().await;
    publisher.abort();
    sh.rt.send(Command::MeshUpdate(Box::new(sh.snapshot())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pair::tests::Host;
    use crate::ring::tests::key;

    /// Two endpoints on this computer pair over real QUIC streams: SPAKE2
    /// with a ticket's secret, and both end up in one ring.
    #[tokio::test]
    async fn two_endpoints_pair_with_a_ticket() {
        let (pc_key, pi_key) = (key(2), key(3));
        let pc = bind(&pc_key, true).await.expect("bind");
        let pi = bind(&pi_key, true).await.expect("bind");
        let host = Arc::new(Host::new(&pc_key.public(), pi_key.public()));
        let me_pc = Me { id: pc_key.public(), name: "PC".into(), kind: MeshKind::Pc };
        let server = {
            let (pc, host, me_pc) = (pc.clone(), host.clone(), me_pc.clone());
            tokio::spawn(async move {
                let conn = pc.accept().await.expect("incoming").await.expect("handshake");
                assert_eq!(conn.alpn(), ALPN_PAIR);
                let peer = conn.remote_id();
                let (mut send, mut recv) = conn.accept_bi().await.unwrap();
                let out = pair::serve(&mut recv, &mut send, &me_pc, &peer, &*host).await;
                wind_down(&conn, &mut send, &mut recv).await;
                out
            })
        };
        let t = Ticket {
            holder: pc_key.public(),
            invite: [1, 2, 3, 4, 5, 6, 7, 8],
            secret: [9; 16],
            expires_ms: now_ms() + 60_000,
            addrs: pc.bound_sockets(),
        };
        let me_pi = Me { id: pi_key.public(), name: "Pi".into(), kind: MeshKind::Pi };
        let (grant, holder_name) = join_ticket(&pi, &me_pi, &t).await.expect("joined");
        assert_eq!(holder_name, "PC");
        let served = server.await.unwrap().expect("served");
        assert!(matches!(served, Served::Admitted { .. }));
        let theirs = Ring::received(&grant.ring, &pi_key.public(), grant.cert, &grant.members, &grant.revoked).unwrap();
        assert!(theirs.trusted(&pc_key.public()).is_some());
        assert!(host.ring.lock().unwrap().trusted(&pi_key.public()).is_some());
        pc.close().await;
        pi.close().await;
    }

    /// A request through the tunnel over real QUIC streams reaches a router.
    #[tokio::test]
    async fn the_tunnel_works_over_quic() {
        use axum::routing::get;
        let (a_key, b_key) = (key(4), key(5));
        let a = bind(&a_key, true).await.unwrap();
        let b = bind(&b_key, true).await.unwrap();
        let api = LocalApi::for_router(axum::Router::new().route("/v1/state", get(|| async { "{\"status\":\"playing\"}" })));
        let server = {
            let b = b.clone();
            tokio::spawn(async move {
                let conn = b.accept().await.unwrap().await.unwrap();
                let (send, recv) = conn.accept_bi().await.unwrap();
                let caller = nikgob_api::Caller {
                    token_id: "mesh:x".into(),
                    name: "A".into(),
                    preset: nikgob_view::ApiPreset::Full,
                    actor: nikgob_view::ApiActor::Owner,
                };
                tunnel::serve(recv, send, api, caller).await;
                conn
            })
        };
        let conn = connect(&a, b_key.public(), &b.bound_sockets(), ALPN_HTTP).await.unwrap();
        let (send, recv) = conn.open_bi().await.unwrap();
        let resp = tunnel::request(recv, send, http::Request::get("/v1/state").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        assert_eq!(&body[..], b"{\"status\":\"playing\"}");
        let _ = server.await;
        a.close().await;
        b.close().await;
    }

    fn invite(scope: MeshScope, expires_ms: i64) -> Invite {
        let ticket = "nkm1abcdefgh234567".to_owned();
        Invite { id: [7; 8], secret: [1; 16], expires_ms, tries: 0, key: nikgob_core::releases::download_key(&ticket), ticket, scope }
    }

    /// The invite held opens `/install/` to its own key only, while alive
    /// and not spent by wrong tries.
    #[test]
    fn only_the_live_invite_held_opens_the_install() {
        let now = now_ms();
        let i = invite(MeshScope::Owner, now + 60_000);
        let key = i.key.clone();
        assert!(invite_opens(Some(&i), &key, now));
        assert!(!invite_opens(Some(&i), &key, now + 60_000), "expired");
        assert!(!invite_opens(Some(&i), &i.ticket, now), "the ticket is no key");
        assert!(!invite_opens(None, &key, now), "spent or cancelled");
        let spent = Invite { tries: pair::TRIES, ..invite(MeshScope::Owner, now + 60_000) };
        assert!(!invite_opens(Some(&spent), &key, now));
    }

    /// A device joining with a guest invite is signed a guest; one the
    /// holder adds by its code, an owner.
    #[test]
    fn a_joiner_takes_the_scope_of_its_invite() {
        let g = invite(MeshScope::Guest, now_ms() + 60_000);
        let id = hex(&g.id);
        assert_eq!(joiner_scope(Some(id.as_str()), Some(&g)), MeshScope::Guest);
        assert_eq!(joiner_scope(None, Some(&g)), MeshScope::Owner, "added by its code");
        assert_eq!(joiner_scope(Some("00"), Some(&g)), MeshScope::Owner);
        assert_eq!(joiner_scope(Some(id.as_str()), None), MeshScope::Owner);
    }

    #[test]
    fn a_ticket_must_be_alive() {
        let t = Ticket { holder: key(2).public(), invite: [0; 8], secret: [1; 16], expires_ms: now_ms() - 1, addrs: Vec::new() };
        assert_eq!(ticket_of(locale(), &t.encode()), Err("Приглашение истекло".to_owned()));
        let alive = Ticket { expires_ms: now_ms() + 60_000, ..t };
        assert!(ticket_of(locale(), &alive.encode()).is_ok());
        assert!(ticket_of(locale(), "nope").is_err());
    }

    /// A ticket handed to a running instance that never answers: the
    /// request file does not stay behind.
    #[test]
    fn a_hand_over_nobody_takes_is_withdrawn() {
        let dir = tempfile::tempdir().unwrap();
        let paths = MeshPaths::of(dir.path());
        let t = Ticket { holder: key(2).public(), invite: [0; 8], secret: [1; 16], expires_ms: now_ms() + 60_000, addrs: Vec::new() };
        assert!(hand_over(&paths, &t.encode(), Duration::from_millis(300)).is_err());
        assert!(!paths.dir.join(REQUEST_FILE).exists());
        // An answer written by the instance is read.
        let p2 = paths.clone();
        let answer = std::thread::spawn(move || {
            for _ in 0..40 {
                if p2.dir.join(REQUEST_FILE).exists() {
                    std::fs::write(p2.dir.join(RESULT_FILE), "ok\nPC\n").unwrap();
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        assert_eq!(hand_over(&paths, &t.encode(), Duration::from_secs(3)), Ok("PC".to_owned()));
        answer.join().unwrap();
    }
}
