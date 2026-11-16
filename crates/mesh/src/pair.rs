//! Pairing (ALPN `nikgob/pair/1`): a device joins the ring after SPAKE2
//! with a password both sides know, bound to both node ids (the QUIC
//! handshake has already proved who each side is):
//!
//! - **ticket**: the joiner dials the ring holder; the password is the
//!   ticket's one-time secret; 5 wrong tries spend the invite;
//! - **code**: the holder dials a nearby device that shows a 6-digit code;
//!   the password is the code; 5 wrong tries make the device show a new one.
//!
//! Both sides prove they derived the same key (keyed BLAKE3 over the whole
//! exchange) before anything is given; then the holder signs the joiner's
//! certificate and sends it with the ring's members and revocations.
//!
//! SPAKE2 side A is always the joiner, side B the holder.

use iroh::PublicKey;
use nikgob_view::MeshKind;
use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::ring::{Cert, Revocation, RingError, hex, unhex};
use crate::wire::{read_frame, write_frame};

/// Wrong codes (or secrets) before the code (or invite) is spent.
pub const TRIES: u32 = 5;

/// What the ring gives a new member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// The ring's public key (hex).
    pub ring: String,
    /// The new member's certificate.
    pub cert: Cert,
    pub members: Vec<Cert>,
    pub revoked: Vec<Revocation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "m", rename_all = "snake_case")]
enum Mode {
    Ticket { invite: String },
    Code,
}

impl Mode {
    fn label(&self) -> &'static str {
        match self {
            Self::Ticket { .. } => "ticket",
            Self::Code => "code",
        }
    }
}

/// Why the other side would not pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    WrongCode,
    /// No such invite, or it expired or was spent.
    NoInvite,
    /// Another pairing runs there.
    Busy,
    /// It is in a ring already.
    Paired,
    /// It cannot sign (not the ring's holder).
    NotHolder,
    Internal,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum Msg {
    Hello { mode: Mode, spake: String, name: String, kind: MeshKind },
    Reply { spake: String, confirm: String, name: String, kind: MeshKind },
    Confirm { confirm: String },
    Grant { confirm: Option<String>, grant: Grant },
    Fail { reason: Refusal },
}

/// Why a pairing failed.
#[derive(Debug, PartialEq, Eq)]
pub enum PairError {
    /// The code or the secret did not match.
    Wrong,
    Refused(Refusal),
    /// The other side broke the protocol.
    Protocol,
    Io(String),
    Ring(RingError),
}

impl PairError {
    /// In words for the settings row.
    pub fn user_message(&self) -> String {
        match self {
            Self::Wrong | Self::Refused(Refusal::WrongCode) => "Неверный код".to_owned(),
            Self::Refused(Refusal::NoInvite) => "Приглашение истекло или уже использовано".to_owned(),
            Self::Refused(Refusal::Busy) => "Устройство занято другой привязкой".to_owned(),
            Self::Refused(Refusal::Paired) => "Устройство уже привязано".to_owned(),
            Self::Refused(Refusal::NotHolder) => "Привязывать может только главное устройство".to_owned(),
            Self::Refused(Refusal::Internal) | Self::Protocol => "Устройство ответило не так".to_owned(),
            Self::Io(_) => "Нет связи с устройством".to_owned(),
            Self::Ring(_) => "Не удалось сохранить привязку".to_owned(),
        }
    }
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "pairing: {e}"),
            Self::Ring(e) => write!(f, "pairing: {e}"),
            other => write!(f, "pairing: {other:?}"),
        }
    }
}

impl From<std::io::Error> for PairError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// This side of a pairing.
#[derive(Clone, Debug)]
pub struct Me {
    pub id: PublicKey,
    pub name: String,
    pub kind: MeshKind,
}

/// What the listening side keeps and decides.
pub trait PairHost: Send + Sync {
    /// The secret of a live invite (holder).
    fn invite_secret(&self, invite: &str) -> Result<Vec<u8>, Refusal>;
    /// A wrong try with this invite (holder).
    fn invite_failed(&self, invite: &str);
    /// Admits `joiner`: signs its certificate (and spends `invite`, if
    /// any). Holder only.
    fn admit(&self, joiner: &PublicKey, name: &str, kind: MeshKind, invite: Option<&str>) -> Result<Grant, Refusal>;
    /// The code shown now, taken for one try (joiner).
    fn take_code(&self) -> Result<String, Refusal>;
    /// The try with the code ended (joiner): a failure counts.
    fn code_done(&self, ok: bool);
    /// Keeps what the ring gave (joiner).
    fn accept(&self, grant: Grant, holder: &PublicKey, holder_name: &str) -> Result<(), PairError>;
}

/// What a listening side did.
#[derive(Debug, PartialEq, Eq)]
pub enum Served {
    /// A device joined this ring.
    Admitted { node: PublicKey, name: String, kind: MeshKind },
    /// This device joined the dialer's ring.
    Joined { holder: PublicKey, holder_name: String },
}

fn start(side_a: bool, password: &[u8], a: &PublicKey, b: &PublicKey) -> (Spake2<Ed25519Group>, Vec<u8>) {
    let pw = Password::new(password);
    let (ia, ib) = (Identity::new(a.as_bytes()), Identity::new(b.as_bytes()));
    if side_a { Spake2::<Ed25519Group>::start_a(&pw, &ia, &ib) } else { Spake2::<Ed25519Group>::start_b(&pw, &ia, &ib) }
}

/// The proof of one side (`role`) that it holds the exchange's key.
fn tag(key: &[u8], role: &str, mode: &str, a: &PublicKey, b: &PublicKey, msg_a: &[u8], msg_b: &[u8]) -> String {
    let k = blake3::derive_key("nikgob pair 1 confirm", key);
    let mut h = blake3::Hasher::new_keyed(&k);
    for part in [role.as_bytes(), mode.as_bytes(), a.as_bytes().as_slice(), b.as_bytes().as_slice(), msg_a, msg_b] {
        h.update(&(part.len() as u32).to_be_bytes());
        h.update(part);
    }
    h.finalize().to_hex().to_string()
}

/// Two proofs are the same (compared in constant time).
fn same(a: &str, b: &str) -> bool {
    match (blake3::Hash::from_hex(a), blake3::Hash::from_hex(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

fn bytes(h: &str) -> Result<Vec<u8>, PairError> {
    unhex(h).ok_or(PairError::Protocol)
}

/// The joiner dials the holder of `ticket`: the ring's grant and the
/// holder's name.
pub async fn join<R, W>(
    r: &mut R,
    w: &mut W,
    me: &Me,
    holder: &PublicKey,
    invite: &[u8; 8],
    secret: &[u8],
) -> Result<(Grant, String), PairError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mode = Mode::Ticket { invite: hex(invite) };
    let label = mode.label();
    let (s, msg_a) = start(true, secret, &me.id, holder);
    write_frame(w, &Msg::Hello { mode, spake: hex(&msg_a), name: me.name.clone(), kind: me.kind }).await?;
    let (msg_b, confirm, holder_name) = match read_frame(r).await? {
        Msg::Reply { spake, confirm, name, .. } => (bytes(&spake)?, confirm, name),
        Msg::Fail { reason } => return Err(PairError::Refused(reason)),
        _ => return Err(PairError::Protocol),
    };
    let key = s.finish(&msg_b).map_err(|_| PairError::Protocol)?;
    if !same(&confirm, &tag(&key, "holder", label, &me.id, holder, &msg_a, &msg_b)) {
        let _ = write_frame(w, &Msg::Fail { reason: Refusal::WrongCode }).await;
        return Err(PairError::Wrong);
    }
    write_frame(w, &Msg::Confirm { confirm: tag(&key, "joiner", label, &me.id, holder, &msg_a, &msg_b) }).await?;
    match read_frame(r).await? {
        Msg::Grant { grant, .. } => Ok((grant, holder_name)),
        Msg::Fail { reason } => Err(PairError::Refused(reason)),
        _ => Err(PairError::Protocol),
    }
}

/// The holder dials a device that shows `code`; `admit` signs its
/// certificate once the code proved right. The device's name and kind.
pub async fn offer<R, W, F>(
    r: &mut R,
    w: &mut W,
    me: &Me,
    joiner: &PublicKey,
    code: &str,
    admit: F,
) -> Result<(String, MeshKind), PairError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnOnce(&str, MeshKind) -> Result<Grant, Refusal>,
{
    let label = Mode::Code.label();
    let (s, msg_b) = start(false, code.trim().as_bytes(), joiner, &me.id);
    write_frame(w, &Msg::Hello { mode: Mode::Code, spake: hex(&msg_b), name: me.name.clone(), kind: me.kind }).await?;
    let (msg_a, confirm, name, kind) = match read_frame(r).await? {
        Msg::Reply { spake, confirm, name, kind } => (bytes(&spake)?, confirm, name, kind),
        Msg::Fail { reason } => return Err(PairError::Refused(reason)),
        _ => return Err(PairError::Protocol),
    };
    let key = s.finish(&msg_a).map_err(|_| PairError::Protocol)?;
    if !same(&confirm, &tag(&key, "joiner", label, joiner, &me.id, &msg_a, &msg_b)) {
        let _ = write_frame(w, &Msg::Fail { reason: Refusal::WrongCode }).await;
        return Err(PairError::Wrong);
    }
    let grant = match admit(&name, kind) {
        Ok(g) => g,
        Err(reason) => {
            let _ = write_frame(w, &Msg::Fail { reason }).await;
            return Err(PairError::Refused(reason));
        }
    };
    let proof = tag(&key, "holder", label, joiner, &me.id, &msg_a, &msg_b);
    write_frame(w, &Msg::Grant { confirm: Some(proof), grant }).await?;
    Ok((name, kind))
}

/// The listening side: a joiner with a ticket (this device holds the
/// ring), or a holder offering to this device that shows a code.
pub async fn serve<R, W>(r: &mut R, w: &mut W, me: &Me, peer: &PublicKey, host: &dyn PairHost) -> Result<Served, PairError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let (mode, spake, peer_name, peer_kind) = match read_frame(r).await? {
        Msg::Hello { mode, spake, name, kind } => (mode, spake, name, kind),
        _ => return Err(PairError::Protocol),
    };
    let label = mode.label();
    match mode {
        Mode::Ticket { invite } => {
            let secret = match host.invite_secret(&invite) {
                Ok(s) => s,
                Err(reason) => {
                    let _ = write_frame(w, &Msg::Fail { reason }).await;
                    return Err(PairError::Refused(reason));
                }
            };
            let msg_a = bytes(&spake)?;
            let (s, msg_b) = start(false, &secret, peer, &me.id);
            let Ok(key) = s.finish(&msg_a) else {
                host.invite_failed(&invite);
                let _ = write_frame(w, &Msg::Fail { reason: Refusal::Internal }).await;
                return Err(PairError::Protocol);
            };
            let proof = tag(&key, "holder", label, peer, &me.id, &msg_a, &msg_b);
            write_frame(w, &Msg::Reply { spake: hex(&msg_b), confirm: proof, name: me.name.clone(), kind: me.kind }).await?;
            let confirm = match read_frame(r).await {
                Ok(Msg::Confirm { confirm }) => confirm,
                // The joiner found this side's proof wrong: a wrong secret.
                Ok(Msg::Fail { .. }) => {
                    host.invite_failed(&invite);
                    return Err(PairError::Wrong);
                }
                Ok(_) => {
                    host.invite_failed(&invite);
                    return Err(PairError::Protocol);
                }
                Err(e) => {
                    host.invite_failed(&invite);
                    return Err(e.into());
                }
            };
            if !same(&confirm, &tag(&key, "joiner", label, peer, &me.id, &msg_a, &msg_b)) {
                host.invite_failed(&invite);
                let _ = write_frame(w, &Msg::Fail { reason: Refusal::WrongCode }).await;
                return Err(PairError::Wrong);
            }
            let grant = match host.admit(peer, &peer_name, peer_kind, Some(&invite)) {
                Ok(g) => g,
                Err(reason) => {
                    let _ = write_frame(w, &Msg::Fail { reason }).await;
                    return Err(PairError::Refused(reason));
                }
            };
            write_frame(w, &Msg::Grant { confirm: None, grant }).await?;
            Ok(Served::Admitted { node: *peer, name: peer_name, kind: peer_kind })
        }
        Mode::Code => {
            let code = match host.take_code() {
                Ok(c) => c,
                Err(reason) => {
                    let _ = write_frame(w, &Msg::Fail { reason }).await;
                    return Err(PairError::Refused(reason));
                }
            };
            let out = async {
                let msg_b = bytes(&spake)?;
                let (s, msg_a) = start(true, code.as_bytes(), &me.id, peer);
                let key = s.finish(&msg_b).map_err(|_| PairError::Protocol)?;
                let proof = tag(&key, "joiner", label, &me.id, peer, &msg_a, &msg_b);
                write_frame(w, &Msg::Reply { spake: hex(&msg_a), confirm: proof, name: me.name.clone(), kind: me.kind }).await?;
                match read_frame(r).await? {
                    Msg::Grant { confirm: Some(c), grant } => {
                        if !same(&c, &tag(&key, "holder", label, &me.id, peer, &msg_a, &msg_b)) {
                            return Err(PairError::Wrong);
                        }
                        host.accept(grant, peer, &peer_name)?;
                        Ok(Served::Joined { holder: *peer, holder_name: peer_name.clone() })
                    }
                    Msg::Fail { reason: Refusal::WrongCode } => Err(PairError::Wrong),
                    Msg::Fail { reason } => Err(PairError::Refused(reason)),
                    _ => Err(PairError::Protocol),
                }
            }
            .await;
            host.code_done(out.is_ok());
            out
        }
    }
}

/// A new 6-digit code (uniform, from the system's randomness).
pub fn new_code() -> Option<String> {
    loop {
        let mut b = [0u8; 4];
        getrandom::fill(&mut b).ok()?;
        let n = u32::from_be_bytes(b);
        // Rejects the top sliver so every code is equally likely.
        if n < u32::MAX - (u32::MAX % 1_000_000) {
            return Some(format!("{:06}", n % 1_000_000));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use nikgob_view::MeshScope;

    use super::*;
    use crate::ring::Ring;
    use crate::ring::tests::key;

    /// A host for both roles: the holder's ring and invite, the joiner's
    /// code and what it got.
    pub(crate) struct Host {
        ring_key: iroh::SecretKey,
        pub(crate) ring: Mutex<Ring>,
        invite: Mutex<Option<(String, Vec<u8>, u32)>>,
        code: Mutex<(String, u32, bool)>,
        joiner: iroh::PublicKey,
        got: Mutex<Option<Ring>>,
    }

    impl Host {
        pub(crate) fn new(holder: &iroh::PublicKey, joiner: iroh::PublicKey) -> Self {
            let ring_key = key(1);
            Self {
                ring: Mutex::new(Ring::create(&ring_key, holder, "PC", MeshKind::Pc, 10)),
                ring_key,
                invite: Mutex::new(Some(("0102030405060708".into(), vec![9; 16], 0))),
                code: Mutex::new(("123456".into(), 0, false)),
                joiner,
                got: Mutex::new(None),
            }
        }
    }

    impl PairHost for Host {
        fn invite_secret(&self, invite: &str) -> Result<Vec<u8>, Refusal> {
            match &*self.invite.lock().unwrap() {
                Some((id, s, tries)) if id == invite && *tries < TRIES => Ok(s.clone()),
                _ => Err(Refusal::NoInvite),
            }
        }
        fn invite_failed(&self, _: &str) {
            if let Some(i) = self.invite.lock().unwrap().as_mut() {
                i.2 += 1;
            }
        }
        fn admit(&self, joiner: &iroh::PublicKey, name: &str, kind: MeshKind, invite: Option<&str>) -> Result<Grant, Refusal> {
            if invite.is_some() {
                *self.invite.lock().unwrap() = None;
            }
            let cert = Cert::issue(&self.ring_key, joiner, name, kind, MeshScope::Owner, 20);
            let mut ring = self.ring.lock().unwrap();
            ring.admit(cert.clone());
            Ok(Grant { ring: ring.public.to_string(), cert, members: ring.members.clone(), revoked: ring.revoked.clone() })
        }
        fn take_code(&self) -> Result<String, Refusal> {
            let mut c = self.code.lock().unwrap();
            if c.2 {
                return Err(Refusal::Busy);
            }
            c.2 = true;
            Ok(c.0.clone())
        }
        fn code_done(&self, ok: bool) {
            let mut c = self.code.lock().unwrap();
            c.2 = false;
            if !ok {
                c.1 += 1;
                if c.1 >= TRIES {
                    c.0 = "654321".into();
                    c.1 = 0;
                }
            }
        }
        fn accept(&self, grant: Grant, _: &iroh::PublicKey, _: &str) -> Result<(), PairError> {
            let ring = Ring::received(&grant.ring, &self.joiner, grant.cert, &grant.members, &grant.revoked).map_err(PairError::Ring)?;
            *self.got.lock().unwrap() = Some(ring);
            Ok(())
        }
    }

    fn me(n: u8, name: &str, kind: MeshKind) -> Me {
        Me { id: key(n).public(), name: name.into(), kind }
    }

    async fn ticket_try(secret: Vec<u8>, host: &Host, pc: &Me, pi: &Me) -> (Result<(Grant, String), PairError>, Result<Served, PairError>) {
        let (a, b) = tokio::io::duplex(64 << 10);
        let (mut ar, mut aw) = tokio::io::split(a);
        let (mut br, mut bw) = tokio::io::split(b);
        let invite = [1, 2, 3, 4, 5, 6, 7, 8];
        tokio::join!(join(&mut ar, &mut aw, pi, &pc.id, &invite, &secret), serve(&mut br, &mut bw, pc, &pi.id, host))
    }

    #[tokio::test]
    async fn a_ticket_pairs_once_and_both_hold_one_ring() {
        let (pc, pi) = (me(2, "PC", MeshKind::Pc), me(3, "Pi", MeshKind::Pi));
        let host = Host::new(&pc.id, pi.id);
        let (joined, served) = ticket_try(vec![9; 16], &host, &pc, &pi).await;
        let (grant, holder_name) = joined.unwrap();
        assert_eq!(holder_name, "PC");
        assert_eq!(served.unwrap(), Served::Admitted { node: pi.id, name: "Pi".into(), kind: MeshKind::Pi });
        let theirs = Ring::received(&grant.ring, &pi.id, grant.cert, &grant.members, &grant.revoked).unwrap();
        let ours = host.ring.lock().unwrap().clone();
        assert_eq!(theirs.public, ours.public);
        assert!(theirs.trusted(&pc.id).is_some(), "the joiner trusts the holder");
        assert!(ours.trusted(&pi.id).is_some(), "the holder trusts the joiner");
        // The invite is spent.
        let (again, served) = ticket_try(vec![9; 16], &host, &pc, &pi).await;
        assert_eq!(again.unwrap_err(), PairError::Refused(Refusal::NoInvite));
        assert_eq!(served.unwrap_err(), PairError::Refused(Refusal::NoInvite));
    }

    #[tokio::test]
    async fn a_wrong_secret_gives_nothing_and_five_spend_the_invite() {
        let (pc, pi) = (me(2, "PC", MeshKind::Pc), me(3, "Pi", MeshKind::Pi));
        let host = Host::new(&pc.id, pi.id);
        for _ in 0..TRIES {
            let (joined, served) = ticket_try(vec![8; 16], &host, &pc, &pi).await;
            assert_eq!(joined.unwrap_err(), PairError::Wrong);
            assert_eq!(served.unwrap_err(), PairError::Wrong);
        }
        assert!(host.ring.lock().unwrap().trusted(&pi.id).is_none());
        let (joined, _) = ticket_try(vec![9; 16], &host, &pc, &pi).await;
        assert_eq!(joined.unwrap_err(), PairError::Refused(Refusal::NoInvite), "spent by the wrong tries");
    }

    async fn code_try(code: &str, host: &Host, pc: &Me, pi: &Me) -> (Result<(String, MeshKind), PairError>, Result<Served, PairError>) {
        let (a, b) = tokio::io::duplex(64 << 10);
        let (mut ar, mut aw) = tokio::io::split(a);
        let (mut br, mut bw) = tokio::io::split(b);
        let admit = |name: &str, kind: MeshKind| host.admit(&pi.id, name, kind, None);
        tokio::join!(offer(&mut ar, &mut aw, pc, &pi.id, code, admit), serve(&mut br, &mut bw, pi, &pc.id, host))
    }

    #[tokio::test]
    async fn the_right_code_pairs_a_nearby_device() {
        let (pc, pi) = (me(2, "PC", MeshKind::Pc), me(3, "Pi", MeshKind::Pi));
        let host = Host::new(&pc.id, pi.id);
        let (offered, served) = code_try(" 123456 ", &host, &pc, &pi).await;
        assert_eq!(offered.unwrap(), ("Pi".to_owned(), MeshKind::Pi));
        assert_eq!(served.unwrap(), Served::Joined { holder: pc.id, holder_name: "PC".into() });
        let got = host.got.lock().unwrap().clone().expect("the joiner kept the ring");
        assert!(got.trusted(&pc.id).is_some());
        assert_eq!(got.me.kind, MeshKind::Pi);
        assert!(host.ring.lock().unwrap().trusted(&pi.id).is_some());
    }

    #[tokio::test]
    async fn five_wrong_codes_make_a_new_one() {
        let (pc, pi) = (me(2, "PC", MeshKind::Pc), me(3, "Pi", MeshKind::Pi));
        let host = Host::new(&pc.id, pi.id);
        for i in 0..TRIES {
            let (offered, served) = code_try("000000", &host, &pc, &pi).await;
            assert_eq!(offered.unwrap_err(), PairError::Wrong, "try {i}");
            assert_eq!(served.unwrap_err(), PairError::Wrong);
            assert!(host.got.lock().unwrap().is_none());
        }
        assert_eq!(host.code.lock().unwrap().0, "654321", "a new code after five wrong ones");
        let (old, _) = code_try("123456", &host, &pc, &pi).await;
        assert_eq!(old.unwrap_err(), PairError::Wrong, "the old code no longer works");
        let (new, _) = code_try("654321", &host, &pc, &pi).await;
        assert!(new.is_ok());
    }

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..50 {
            let c = new_code().unwrap();
            assert_eq!(c.len(), 6);
            assert!(c.bytes().all(|b| b.is_ascii_digit()));
        }
        assert!(same(&tag(b"k", "a", "m", &key(1).public(), &key(2).public(), b"x", b"y"), &tag(b"k", "a", "m", &key(1).public(), &key(2).public(), b"x", b"y")));
        assert!(!same(&tag(b"k", "a", "m", &key(1).public(), &key(2).public(), b"x", b"y"), &tag(b"k", "b", "m", &key(1).public(), &key(2).public(), b"x", b"y")));
        assert!(!same("zz", "zz"));
    }
}
