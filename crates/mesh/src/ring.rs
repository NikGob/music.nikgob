//! The ring: the owner's set of devices. The device that made it holds
//! its Ed25519 key and signs a certificate for each member (`{node, name,
//! kind, scope, issued}`) and a revocation for each forgotten one. Every
//! member keeps the ring's public key, its own certificate, the others' and
//! the revocations (`ring.json`). Two devices trust each other when both
//! show a certificate of the same ring that no revocation came after.
//!
//! A ring keeps one revocation per forgotten device (the newest) and drops
//! the certificates they made void. The holder admits a device only while
//! every member could still be forgotten within [`MAX_REVOKED`], and every
//! member keeps the same limits, so nothing the holder signs is ever left
//! out somewhere on the way round.

use std::path::Path;
use std::str::FromStr;

use iroh::{PublicKey, SecretKey, Signature};
use nikgob_view::{MeshKind, MeshScope};
use serde::{Deserialize, Serialize};

use crate::keys::write_atomic;

/// Members a ring may have at once (the holder too)…
pub const MAX_MEMBERS: usize = 64;
/// …and forgotten devices it remembers (one revocation each). Both lists
/// in one control message stay well under its 256 KiB.
pub const MAX_REVOKED: usize = 512;

/// Lowercase hex.
pub(crate) fn hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    data_encoding::HEXLOWER_PERMISSIVE.decode(s.trim().as_bytes()).ok()
}

/// A node id from its hex (or z-base32) form.
pub fn parse_id(s: &str) -> Option<PublicKey> {
    PublicKey::from_str(s.trim()).ok()
}

/// The ring's short tag for mDNS: the first 8 hex digits of BLAKE3 of its
/// public key.
pub fn ring_tag(ring: &PublicKey) -> String {
    blake3::hash(ring.as_bytes()).to_hex()[..8].to_owned()
}

fn push_str(m: &mut Vec<u8>, s: &str) {
    m.extend_from_slice(&u32::try_from(s.len()).unwrap_or(u32::MAX).to_be_bytes());
    m.extend_from_slice(s.as_bytes());
}

fn signature(hex_sig: &str) -> Option<Signature> {
    let bytes = unhex(hex_sig)?;
    let arr: [u8; 64] = bytes.as_slice().try_into().ok()?;
    Some(Signature::from_bytes(&arr))
}

/// A member's certificate, signed by the ring's key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cert {
    /// The member's node id (hex).
    pub node: String,
    pub name: String,
    pub kind: MeshKind,
    pub scope: MeshScope,
    pub issued_ms: i64,
    /// Hex of the 64-byte signature.
    pub sig: String,
}

impl Cert {
    fn message(node: &PublicKey, name: &str, kind: MeshKind, scope: MeshScope, issued_ms: i64) -> Vec<u8> {
        let mut m = b"nikgob/cert/1\0".to_vec();
        m.extend_from_slice(node.as_bytes());
        push_str(&mut m, kind.key());
        push_str(&mut m, scope.key());
        m.extend_from_slice(&issued_ms.to_be_bytes());
        push_str(&mut m, name);
        m
    }

    /// A certificate for `node`, signed with the ring's key.
    pub fn issue(ring: &SecretKey, node: &PublicKey, name: &str, kind: MeshKind, scope: MeshScope, issued_ms: i64) -> Self {
        let name = clean_name(name);
        let sig = ring.sign(&Self::message(node, &name, kind, scope, issued_ms));
        Self { node: node.to_string(), name, kind, scope, issued_ms, sig: hex(&sig.to_bytes()) }
    }

    pub fn node_id(&self) -> Option<PublicKey> {
        parse_id(&self.node)
    }

    /// Signed by `ring`, as it says.
    pub fn verify(&self, ring: &PublicKey) -> bool {
        let (Some(node), Some(sig)) = (self.node_id(), signature(&self.sig)) else { return false };
        ring.verify(&Self::message(&node, &self.name, self.kind, self.scope, self.issued_ms), &sig).is_ok()
    }
}

/// A forgotten member: certificates of `node` issued up to `revoked_ms`
/// no longer count.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    pub node: String,
    pub revoked_ms: i64,
    pub sig: String,
}

impl Revocation {
    fn message(node: &PublicKey, revoked_ms: i64) -> Vec<u8> {
        let mut m = b"nikgob/revoke/1\0".to_vec();
        m.extend_from_slice(node.as_bytes());
        m.extend_from_slice(&revoked_ms.to_be_bytes());
        m
    }

    pub fn issue(ring: &SecretKey, node: &PublicKey, revoked_ms: i64) -> Self {
        let sig = ring.sign(&Self::message(node, revoked_ms));
        Self { node: node.to_string(), revoked_ms, sig: hex(&sig.to_bytes()) }
    }

    pub fn verify(&self, ring: &PublicKey) -> bool {
        let (Some(node), Some(sig)) = (parse_id(&self.node), signature(&self.sig)) else { return false };
        ring.verify(&Self::message(&node, self.revoked_ms), &sig).is_ok()
    }
}

/// A device name as certificates and mDNS carry it: trimmed, no control
/// characters, at most 63 bytes (an mDNS label).
pub fn clean_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > 63 {
            break;
        }
        out.push(c);
    }
    out
}

/// `ring.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct RingFile {
    v: u32,
    ring: String,
    me: Cert,
    members: Vec<Cert>,
    revoked: Vec<Revocation>,
}

/// The ring as this member keeps it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ring {
    pub public: PublicKey,
    /// This member's certificate.
    pub me: Cert,
    /// Every member's newest certificate, this one's too, each verified.
    pub members: Vec<Cert>,
    pub revoked: Vec<Revocation>,
}

/// Why a ring from a file or a grant is not taken.
#[derive(Debug, PartialEq, Eq)]
pub enum RingError {
    Unreadable(String),
    BadKey,
    /// The certificate is not this device's, or not the ring's.
    BadCert,
    Revoked,
}

impl std::fmt::Display for RingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(e) => write!(f, "ring file: {e}"),
            Self::BadKey => write!(f, "not a ring key"),
            Self::BadCert => write!(f, "the certificate is not this device's or not the ring's"),
            Self::Revoked => write!(f, "this device was forgotten by the ring"),
        }
    }
}

impl Ring {
    /// A ring made here: `ring_key` signs this device's own certificate.
    pub fn create(ring_key: &SecretKey, me: &PublicKey, name: &str, kind: MeshKind, now_ms: i64) -> Self {
        let cert = Cert::issue(ring_key, me, name, kind, MeshScope::Owner, now_ms);
        Self { public: ring_key.public(), me: cert.clone(), members: vec![cert], revoked: Vec::new() }
    }

    /// A ring received: its key (hex), this device's certificate, the
    /// members and revocations as given (the invalid ones dropped).
    pub fn received(ring: &str, me: &PublicKey, cert: Cert, members: &[Cert], revoked: &[Revocation]) -> Result<Self, RingError> {
        let public = parse_id(ring).ok_or(RingError::BadKey)?;
        if !cert.verify(&public) || cert.node_id().as_ref() != Some(me) {
            return Err(RingError::BadCert);
        }
        let mut r = Self { public, me: cert.clone(), members: vec![cert], revoked: Vec::new() };
        r.merge(members, revoked);
        if r.me_revoked() {
            return Err(RingError::Revoked);
        }
        Ok(r)
    }

    pub fn tag(&self) -> String {
        ring_tag(&self.public)
    }

    /// A certificate of `node` issued at `issued_ms` is revoked.
    pub fn is_revoked(&self, node: &str, issued_ms: i64) -> bool {
        self.revoked.iter().any(|r| r.node == node && r.revoked_ms >= issued_ms)
    }

    /// This device's certificate no longer counts.
    pub fn me_revoked(&self) -> bool {
        self.is_revoked(&self.me.node, self.me.issued_ms)
    }

    /// The certificate `id` is trusted by: a member's, not revoked.
    pub fn trusted(&self, id: &PublicKey) -> Option<&Cert> {
        let node = id.to_string();
        self.members.iter().find(|c| c.node == node && !self.is_revoked(&c.node, c.issued_ms))
    }

    /// The trusted members but this device.
    pub fn others(&self) -> impl Iterator<Item = &Cert> {
        self.members.iter().filter(|c| c.node != self.me.node && !self.is_revoked(&c.node, c.issued_ms))
    }

    /// Takes a revocation signed by the ring: the newest one per device
    /// stays. Past [`MAX_REVOKED`] only a member's is still taken (a holder
    /// keeps within it, so no member is ever left unforgettable). Whether it
    /// changed anything.
    fn take_revocation(&mut self, r: &Revocation) -> bool {
        if !r.verify(&self.public) {
            return false;
        }
        match self.revoked.iter_mut().find(|x| x.node == r.node) {
            Some(x) if r.revoked_ms > x.revoked_ms => {
                *x = r.clone();
                true
            }
            Some(_) => false,
            None => {
                if self.revoked.len() >= MAX_REVOKED && !self.members.iter().any(|m| m.node == r.node) {
                    return false;
                }
                self.revoked.push(r.clone());
                true
            }
        }
    }

    /// Drops the certificates a revocation made void (this device's own
    /// stays: [`Ring::me_revoked`] tells). Whether any went.
    fn prune(&mut self) -> bool {
        let before = self.members.len();
        let (revoked, me) = (std::mem::take(&mut self.revoked), self.me.node.clone());
        self.members.retain(|c| c.node == me || !revoked.iter().any(|r| r.node == c.node && r.revoked_ms >= c.issued_ms));
        self.revoked = revoked;
        self.members.len() != before
    }

    /// Takes the certificates and revocations of another member (or of a
    /// grant): only the ring's own, newer ones; a certificate a revocation
    /// voids is not taken. Whether anything changed.
    pub fn merge(&mut self, certs: &[Cert], revoked: &[Revocation]) -> bool {
        let mut changed = false;
        for r in revoked {
            changed |= self.take_revocation(r);
        }
        changed |= self.prune();
        for c in certs {
            if !c.verify(&self.public) || self.is_revoked(&c.node, c.issued_ms) {
                continue;
            }
            match self.members.iter().position(|m| m.node == c.node) {
                Some(i) if c.issued_ms > self.members[i].issued_ms => {
                    self.members[i] = c.clone();
                    changed = true;
                }
                Some(_) => {}
                None if self.members.len() < MAX_MEMBERS => {
                    self.members.push(c.clone());
                    changed = true;
                }
                None => {}
            }
        }
        if let Some(newer) = self.members.iter().find(|m| m.node == self.me.node && m.issued_ms > self.me.issued_ms) {
            self.me = newer.clone();
            changed = true;
        }
        changed
    }

    /// The holder may sign `node` a certificate: it is a member already, or
    /// there is room for one more member, and forgetting every member, the
    /// new one too, would still fit in [`MAX_REVOKED`].
    pub fn can_admit(&self, node: &PublicKey) -> bool {
        if self.trusted(node).is_some() {
            return true;
        }
        let id = node.to_string();
        let has_rev = |n: &str| self.revoked.iter().any(|r| r.node == n);
        let would = self.revoked.len()
            + self.members.iter().filter(|c| c.node != self.me.node && !has_rev(&c.node)).count()
            + usize::from(!has_rev(&id));
        self.members.len() < MAX_MEMBERS && would <= MAX_REVOKED
    }

    /// Forgets member `node` (holder): a revocation after every certificate
    /// it has, signed with `ring_key`; its certificate goes. Always taken
    /// for a member; `false` when `node` is not one (or is this device).
    pub fn revoke(&mut self, ring_key: &SecretKey, node: &PublicKey, now_ms: i64) -> bool {
        let id = node.to_string();
        if id == self.me.node {
            return false;
        }
        let Some(issued) = self.members.iter().filter(|c| c.node == id).map(|c| c.issued_ms).max() else { return false };
        let after = self.revoked.iter().filter(|r| r.node == id).map(|r| r.revoked_ms + 1).max().unwrap_or(i64::MIN);
        let rev = Revocation::issue(ring_key, node, now_ms.max(issued).max(after));
        let taken = self.take_revocation(&rev);
        self.prune();
        taken
    }

    /// Adds (or renews) a member's certificate signed here.
    pub fn admit(&mut self, cert: Cert) {
        match self.members.iter_mut().find(|m| m.node == cert.node) {
            Some(m) => *m = cert.clone(),
            None => self.members.push(cert.clone()),
        }
        if cert.node == self.me.node {
            self.me = cert;
        }
    }

    /// Reads `ring.json`: `None` when there is none. A file this device's
    /// certificate is not valid in is an error.
    pub fn load(path: &Path, me: &PublicKey) -> Result<Option<Self>, RingError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(RingError::Unreadable(e.to_string())),
        };
        let f: RingFile = serde_json::from_str(&text).map_err(|e| RingError::Unreadable(e.to_string()))?;
        Self::received(&f.ring, me, f.me, &f.members, &f.revoked).map(Some)
    }

    /// Writes `ring.json` atomically.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let f = RingFile {
            v: 1,
            ring: self.public.to_string(),
            me: self.me.clone(),
            members: self.members.clone(),
            revoked: self.revoked.clone(),
        };
        let text = serde_json::to_vec_pretty(&f).map_err(std::io::Error::other)?;
        write_atomic(path, &text, false)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn key(n: u8) -> SecretKey {
        SecretKey::from_bytes(&[n; 32])
    }

    #[test]
    fn a_certificate_verifies_only_with_its_ring_and_as_issued() {
        let (ring, other, pi) = (key(1), key(2), key(3).public());
        let c = Cert::issue(&ring, &pi, "Raspberry Pi", MeshKind::Pi, MeshScope::Owner, 1000);
        assert!(c.verify(&ring.public()));
        assert!(!c.verify(&other.public()), "another ring");
        let mut renamed = c.clone();
        renamed.name = "Evil".into();
        assert!(!renamed.verify(&ring.public()), "a changed name breaks the signature");
        let mut guest = c.clone();
        guest.scope = MeshScope::Guest;
        assert!(!guest.verify(&ring.public()));
        let mut bad = c.clone();
        bad.sig = "00".into();
        assert!(!bad.verify(&ring.public()));
    }

    #[test]
    fn names_are_cleaned_and_cut() {
        assert_eq!(clean_name("  Pi\n "), "Pi");
        let long = "Я".repeat(40);
        let cut = clean_name(&long);
        assert!(cut.len() <= 63 && cut.chars().all(|c| c == 'Я'));
        assert_eq!(cut.chars().count(), 31);
    }

    #[test]
    fn a_revocation_counts_for_certificates_up_to_it() {
        let ring_key = key(1);
        let (pc, pi) = (key(2).public(), key(3).public());
        let mut ring = Ring::create(&ring_key, &pc, "PC", MeshKind::Pc, 10);
        ring.admit(Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 20));
        assert!(ring.trusted(&pi).is_some());
        assert_eq!(ring.others().count(), 1);
        let rev = Revocation::issue(&ring_key, &pi, 30);
        assert!(ring.merge(&[], std::slice::from_ref(&rev)));
        assert!(!ring.merge(&[], std::slice::from_ref(&rev)), "once");
        assert!(ring.trusted(&pi).is_none());
        assert_eq!(ring.others().count(), 0);
        // Paired again later: a newer certificate counts.
        assert!(ring.merge(&[Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 40)], &[]));
        assert!(ring.trusted(&pi).is_some());
        // A revocation not signed by the ring is ignored.
        let forged = Revocation::issue(&key(9), &pi, 50);
        assert!(!ring.merge(&[], &[forged]));
        assert!(ring.trusted(&pi).is_some());
    }

    /// The holder forgets the TV: the Pi hears of it from the holder's list,
    /// the TV from anyone's, and finds itself out of the ring.
    #[test]
    fn a_revocation_goes_round_the_ring() {
        let ring_key = key(1);
        let (pc, pi, tv) = (key(2).public(), key(3).public(), key(4).public());
        let mut holder = Ring::create(&ring_key, &pc, "PC", MeshKind::Pc, 10);
        let pi_cert = Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 20);
        let tv_cert = Cert::issue(&ring_key, &tv, "TV", MeshKind::Tv, MeshScope::Guest, 30);
        holder.admit(pi_cert.clone());
        holder.admit(tv_cert.clone());
        let mut at_pi = Ring::received(&holder.public.to_string(), &pi, pi_cert, &holder.members, &holder.revoked).unwrap();
        let mut at_tv = Ring::received(&holder.public.to_string(), &tv, tv_cert, &holder.members, &holder.revoked).unwrap();
        assert!(at_pi.trusted(&tv).is_some() && at_tv.trusted(&pi).is_some());
        holder.merge(&[], &[Revocation::issue(&ring_key, &tv, 40)]);
        assert!(at_pi.merge(&holder.members, &holder.revoked));
        assert!(at_pi.trusted(&tv).is_none(), "the Pi no longer trusts the TV");
        assert!(at_tv.merge(&at_pi.members, &at_pi.revoked));
        assert!(at_tv.me_revoked(), "the TV learns it was forgotten");
        assert!(!at_pi.merge(&at_tv.members, &at_tv.revoked), "nothing new comes back");
    }

    /// One revocation per device, the newest; the certificate it voids goes
    /// and an old copy of it coming round again is not taken back.
    #[test]
    fn revocations_stay_one_per_device_and_void_certificates_go() {
        let ring_key = key(1);
        let (pc, pi) = (key(2).public(), key(3).public());
        let mut ring = Ring::create(&ring_key, &pc, "PC", MeshKind::Pc, 10);
        let old = Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 20);
        ring.admit(old.clone());
        assert!(ring.revoke(&ring_key, &pi, 30));
        assert_eq!(ring.members.len(), 1, "the voided certificate went");
        assert!(!ring.merge(&[old.clone()], &[]), "an old copy is not taken back");
        // Paired again and forgotten again: still one revocation.
        ring.admit(Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 40));
        assert!(ring.trusted(&pi).is_some());
        assert!(ring.revoke(&ring_key, &pi, 35), "after its newest certificate even with a clock behind");
        assert_eq!(ring.revoked.len(), 1);
        assert!(ring.revoked[0].revoked_ms >= 40);
        assert!(ring.trusted(&pi).is_none());
        // An older revocation changes nothing.
        assert!(!ring.merge(&[], &[Revocation::issue(&ring_key, &pi, 30)]));
        assert!(!ring.revoke(&ring_key, &pc, 50), "the holder does not forget itself");
        assert!(!ring.revoke(&ring_key, &key(7).public(), 50), "nor a stranger");
    }

    fn many(i: usize) -> PublicKey {
        let mut b = [7u8; 32];
        b[0] = (i & 0xff) as u8;
        b[1] = (i >> 8) as u8;
        SecretKey::from_bytes(&b).public()
    }

    /// The holder admits only devices it could still forget, and forgetting
    /// a member is never refused, even with the list of revocations full.
    #[test]
    fn a_member_can_always_be_forgotten() {
        let ring_key = key(1);
        let (pc, pi, tv) = (key(2).public(), key(3).public(), key(4).public());
        let mut ring = Ring::create(&ring_key, &pc, "PC", MeshKind::Pc, 10);
        let strangers: Vec<Revocation> = (0..MAX_REVOKED - 1).map(|i| Revocation::issue(&ring_key, &many(i), 5)).collect();
        assert!(ring.merge(&[], &strangers));
        assert_eq!(ring.revoked.len(), MAX_REVOKED - 1);
        assert!(ring.can_admit(&pi));
        ring.admit(Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 20));
        assert!(!ring.can_admit(&tv), "forgetting both would not fit");
        assert!(ring.can_admit(&pi), "a member is renewed");
        assert!(ring.revoke(&ring_key, &pi, 30));
        assert_eq!(ring.revoked.len(), MAX_REVOKED);
        assert!(ring.trusted(&pi).is_none());
        // Full: a stranger's revocation is not kept, a member's still is.
        assert!(!ring.merge(&[], &[Revocation::issue(&ring_key, &many(9999), 5)]));
        let mut other = ring.clone();
        other.members.push(Cert::issue(&ring_key, &tv, "TV", MeshKind::Tv, MeshScope::Guest, 40));
        assert!(other.merge(&[], &[Revocation::issue(&ring_key, &tv, 50)]));
        assert!(other.trusted(&tv).is_none());
    }

    #[test]
    fn merging_takes_only_the_rings_newer_certificates() {
        let ring_key = key(1);
        let (pc, pi, tv) = (key(2).public(), key(3).public(), key(4).public());
        let mut ring = Ring::create(&ring_key, &pc, "PC", MeshKind::Pc, 10);
        let old = Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 20);
        let new = Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Guest, 25);
        let foreign = Cert::issue(&key(9), &tv, "TV", MeshKind::Tv, MeshScope::Guest, 30);
        assert!(ring.merge(&[new.clone(), foreign], &[]));
        assert!(!ring.merge(&[old], &[]), "an older one changes nothing");
        assert_eq!(ring.trusted(&pi).map(|c| c.scope), Some(MeshScope::Guest));
        assert!(ring.trusted(&tv).is_none());
    }

    #[test]
    fn a_received_ring_must_carry_this_devices_certificate() {
        let ring_key = key(1);
        let (pc, pi) = (key(2).public(), key(3).public());
        let holder = Ring::create(&ring_key, &pc, "PC", MeshKind::Pc, 10);
        let cert = Cert::issue(&ring_key, &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 20);
        let got = Ring::received(&holder.public.to_string(), &pi, cert.clone(), &holder.members, &[]).unwrap();
        assert!(got.trusted(&pc).is_some());
        assert_eq!(got.tag(), holder.tag());
        assert_eq!(Ring::received(&holder.public.to_string(), &pc, cert.clone(), &[], &[]), Err(RingError::BadCert));
        let wrong = Cert::issue(&key(9), &pi, "Pi", MeshKind::Pi, MeshScope::Owner, 20);
        assert_eq!(Ring::received(&holder.public.to_string(), &pi, wrong, &[], &[]), Err(RingError::BadCert));
        let rev = Revocation::issue(&ring_key, &pi, 20);
        assert_eq!(Ring::received(&holder.public.to_string(), &pi, cert, &[], &[rev]), Err(RingError::Revoked));
    }

    #[test]
    fn the_ring_file_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mesh").join("ring.json");
        let ring_key = key(1);
        let pc = key(2).public();
        assert_eq!(Ring::load(&path, &pc), Ok(None));
        let mut ring = Ring::create(&ring_key, &pc, "PC", MeshKind::Pc, 10);
        ring.admit(Cert::issue(&ring_key, &key(3).public(), "Pi", MeshKind::Pi, MeshScope::Owner, 20));
        ring.save(&path).unwrap();
        assert_eq!(Ring::load(&path, &pc).unwrap(), Some(ring.clone()));
        // Someone else's file is not taken.
        assert!(Ring::load(&path, &key(5).public()).is_err());
        std::fs::write(&path, "{").unwrap();
        assert!(matches!(Ring::load(&path, &pc), Err(RingError::Unreadable(_))));
    }

    #[test]
    fn the_tag_is_short_hex() {
        let t = ring_tag(&key(1).public());
        assert_eq!(t.len(), 8);
        assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
