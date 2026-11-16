//! The instances on the home network (mDNS, `_nikgob._udp.local.`): this
//! one announced while the mesh is on (instance = its name; TXT `v`, `id`,
//! `ring`, `kind`, `port`), the others browsed all the time. An entry not
//! heard of for a while is asked about again; one that does not answer
//! goes (the daemon then reports it removed).
//!
//! Only the home network counts: the addresses announced and dialled are
//! the LAN's ([`lan_addrs`], [`lan_ip_ok`]) — never loopback, link-local,
//! Tailscale's (100.64.0.0/10, `fd7a:115c:a1e0::/48`) or a virtual
//! adapter's (Hyper-V/WSL `vEthernet`, Docker, VPNs), the private
//! (RFC 1918) ones of the interface the default route goes through first.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use iroh::PublicKey;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use nikgob_view::MeshKind;

use crate::ring::parse_id;

pub const SERVICE: &str = "_nikgob._udp.local.";
/// An entry not refreshed for this long is asked whether it still exists.
pub const STALE: Duration = Duration::from_secs(20);

/// What an instance says of itself in its TXT record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Txt {
    pub id: PublicKey,
    /// The tag of its ring, `None` while it is in none.
    pub ring: Option<String>,
    pub kind: MeshKind,
    pub port: u16,
}

/// The TXT of this instance.
pub fn txt_of(id: &PublicKey, ring: Option<&str>, kind: MeshKind, port: u16) -> Vec<(String, String)> {
    vec![
        ("v".to_owned(), "1".to_owned()),
        ("id".to_owned(), id.to_string()),
        ("ring".to_owned(), ring.unwrap_or("-").to_owned()),
        ("kind".to_owned(), kind.key().to_owned()),
        ("port".to_owned(), port.to_string()),
    ]
}

/// Parses a TXT record (`get` gives a key's value): `None` for another
/// version or a record that is not ours.
pub fn parse_txt(get: impl Fn(&str) -> Option<String>) -> Option<Txt> {
    if get("v")?.trim() != "1" {
        return None;
    }
    let id = parse_id(&get("id")?)?;
    let ring = get("ring").map(|r| r.trim().to_owned()).filter(|r| !r.is_empty() && r != "-");
    let kind = get("kind").and_then(|k| MeshKind::parse(&k)).unwrap_or_default();
    let port = get("port")?.trim().parse::<u16>().ok().filter(|p| *p != 0)?;
    Some(Txt { id, ring, kind, port })
}

/// The instance's name in a full service name (`Кухня._nikgob._udp.local.`).
pub fn instance_name(fullname: &str) -> String {
    let base = fullname.strip_suffix(SERVICE).unwrap_or(fullname);
    base.trim_end_matches('.').replace("\\.", ".").replace("\\032", " ")
}

/// The addresses to dial, best first: IPv4 before IPv6, loopback last.
pub fn dial_order(ips: impl IntoIterator<Item = IpAddr>, port: u16) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = ips.into_iter().filter(|ip| lan_ip_ok(*ip)).map(|ip| SocketAddr::new(ip, port)).collect();
    v.sort_by_key(|a| (!is_private(a.ip()), !a.is_ipv4(), a.ip().to_string()));
    v.dedup();
    v
}

/// Tailscale's IPv4 range, 100.64.0.0/10 (shared address space).
fn is_cgnat(v: Ipv4Addr) -> bool {
    let o = v.octets();
    o[0] == 100 && (o[1] & 0xc0) == 64
}

/// Tailscale's IPv6 range, `fd7a:115c:a1e0::/48`.
fn is_tailscale_v6(v: Ipv6Addr) -> bool {
    v.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0]
}

/// Whether `ip` is one the owner's devices may reach each other at on the
/// home network: not loopback, link-local, unspecified, multicast or
/// Tailscale's.
pub fn lan_ip_ok(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => !(v.is_loopback() || v.is_link_local() || v.is_unspecified() || v.is_multicast() || v.is_broadcast() || is_cgnat(v)),
        IpAddr::V6(v) => {
            let link_local = (v.segments()[0] & 0xffc0) == 0xfe80;
            !(v.is_loopback() || v.is_unspecified() || v.is_multicast() || link_local || is_tailscale_v6(v))
        }
    }
}

/// A private address: RFC 1918 (`10/8`, `172.16/12`, `192.168/16`), or an
/// IPv6 unique local one that is not Tailscale's.
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private(),
        IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00 && !is_tailscale_v6(v),
    }
}

/// Whether the interface named `name` is a virtual one: a VPN (Tailscale,
/// WireGuard, ZeroTier, a tunnel), a virtual machine's or a container's
/// switch (Hyper-V/WSL `vEthernet`, Docker, VirtualBox, VMware, libvirt).
pub fn virtual_iface(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    const STARTS: &[&str] = &[
        "tailscale", "vethernet", "docker", "veth", "br-", "virbr", "vmnet", "vboxnet", "virtualbox", "vmware", "zerotier", "zt", "wg", "tun",
        "tap", "utun", "lxc", "lxd", "podman", "cni", "flannel", "cali", "hyper-v", "wsl", "npcap", "teredo", "isatap", "loopback",
    ];
    const HAS: &[&str] = &["tailscale", "hyper-v", "virtual", "vpn", "wsl", "docker", "vmware", "zerotier", "wireguard"];
    STARTS.iter().any(|p| n.starts_with(p)) || HAS.iter().any(|p| n.contains(p))
}

/// The address this computer reaches the internet from (the interface the
/// default route goes through): nothing is sent.
fn default_route_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:53").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// The LAN addresses among `ifaces` (name, address, up), best first: the
/// private ones of the interface the default route goes through (`route`:
/// its address), the other private ones, then the others; never those
/// [`lan_ip_ok`] refuses, never a virtual interface's (unless the default
/// route goes through it: a virtual switch bridged to the real card).
pub fn pick_lan(ifaces: &[(&str, IpAddr, bool)], route: Option<IpAddr>) -> Vec<IpAddr> {
    let route_iface = route.filter(|r| lan_ip_ok(*r)).and_then(|r| ifaces.iter().find(|(_, ip, _)| *ip == r).map(|(n, ..)| *n));
    let mut out: Vec<(u8, bool, IpAddr)> = ifaces
        .iter()
        .filter(|(_, ip, up)| *up && lan_ip_ok(*ip))
        .filter_map(|(name, ip, _)| {
            let on_route = route_iface == Some(*name);
            if !on_route && virtual_iface(name) {
                return None;
            }
            let rank = match (on_route, is_private(*ip)) {
                (true, true) => 0,
                (false, true) => 1,
                (true, false) => 2,
                (false, false) => 3,
            };
            Some((rank, ip.is_ipv6(), *ip))
        })
        .collect();
    out.sort();
    out.dedup_by_key(|(.., ip)| *ip);
    out.into_iter().map(|(.., ip)| ip).collect()
}

/// This computer's LAN addresses, best first ([`pick_lan`]).
pub fn lan_addrs() -> Vec<IpAddr> {
    let ifaces = if_addrs::get_if_addrs().unwrap_or_default();
    let list: Vec<(&str, IpAddr, bool)> = ifaces
        .iter()
        .map(|i| {
            let down = matches!(i.oper_status, if_addrs::IfOperStatus::Down | if_addrs::IfOperStatus::NotPresent | if_addrs::IfOperStatus::LowerLayerDown);
            (i.name.as_str(), i.ip(), !down)
        })
        .collect();
    pick_lan(&list, default_route_ip())
}

/// The virtual interfaces mDNS keeps quiet on (not the one the default
/// route goes through).
pub fn quiet_ifaces() -> Vec<String> {
    let ifaces = if_addrs::get_if_addrs().unwrap_or_default();
    let route = default_route_ip();
    let mut names: Vec<String> = ifaces
        .iter()
        .filter(|i| virtual_iface(&i.name) && route.is_none_or(|r| !ifaces.iter().any(|j| j.name == i.name && j.ip() == r)))
        .map(|i| i.name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// An instance seen on the network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub fullname: String,
    pub name: String,
    pub txt: Txt,
    pub addrs: Vec<SocketAddr>,
    pub at: Instant,
    /// When it was last asked about (after going stale).
    pub verified: Option<Instant>,
}

/// The browse's table: by full name.
#[derive(Default)]
pub struct Table {
    pub by_name: HashMap<String, Seen>,
}

/// What an event changed.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Nothing,
    Changed,
}

impl Table {
    /// A resolved instance (not this one).
    pub fn resolved(&mut self, fullname: &str, txt: Txt, ips: Vec<IpAddr>, me: &PublicKey, now: Instant) -> Change {
        if &txt.id == me {
            return Change::Nothing;
        }
        let addrs = dial_order(ips, txt.port);
        let name = instance_name(fullname);
        // One entry per device: a new name of the same id replaces the old.
        let old: Vec<String> = self.by_name.iter().filter(|(n, s)| s.txt.id == txt.id && n.as_str() != fullname).map(|(n, _)| n.clone()).collect();
        for n in old {
            self.by_name.remove(&n);
        }
        let seen = Seen { fullname: fullname.to_owned(), name, txt, addrs, at: now, verified: None };
        match self.by_name.get(fullname) {
            Some(s) if s.name == seen.name && s.txt == seen.txt && s.addrs == seen.addrs => {
                self.by_name.insert(fullname.to_owned(), seen);
                Change::Nothing
            }
            _ => {
                self.by_name.insert(fullname.to_owned(), seen);
                Change::Changed
            }
        }
    }

    pub fn removed(&mut self, fullname: &str) -> Change {
        if self.by_name.remove(fullname).is_some() { Change::Changed } else { Change::Nothing }
    }

    /// The entries to ask about now (stale, not asked about lately); each
    /// is marked asked.
    pub fn to_verify(&mut self, now: Instant) -> Vec<String> {
        let mut out = Vec::new();
        for s in self.by_name.values_mut() {
            let stale = now.saturating_duration_since(s.at) >= STALE;
            let asked_lately = s.verified.is_some_and(|v| now.saturating_duration_since(v) < STALE);
            if stale && !asked_lately {
                s.verified = Some(now);
                out.push(s.fullname.clone());
            }
        }
        out
    }

    pub fn by_id(&self, id: &PublicKey) -> Option<&Seen> {
        self.by_name.values().find(|s| &s.txt.id == id)
    }
}

/// This instance's announcement, at its LAN addresses `ips` only.
pub fn announce(daemon: &ServiceDaemon, name: &str, id: &PublicKey, ring: Option<&str>, kind: MeshKind, port: u16, ips: &[IpAddr]) -> Result<String, String> {
    if ips.is_empty() {
        return Err("no address on the home network".to_owned());
    }
    let host = format!("nikgob-{}.local.", &id.to_string()[..12]);
    let props = txt_of(id, ring, kind, port);
    let info = ServiceInfo::new(SERVICE, name, &host, ips, port, props.as_slice()).map_err(|e| e.to_string())?;
    let fullname = info.get_fullname().to_owned();
    daemon.register(info).map_err(|e| e.to_string())?;
    Ok(fullname)
}

/// Parses a daemon event into a table change; `me` is skipped.
pub fn apply(table: &mut Table, ev: ServiceEvent, me: &PublicKey) -> Change {
    match ev {
        ServiceEvent::ServiceResolved(info) => {
            let get = |k: &str| info.get_property_val_str(k).map(str::to_owned);
            let Some(txt) = parse_txt(get) else { return Change::Nothing };
            let ips: Vec<IpAddr> = info.get_addresses().iter().map(|a| a.to_ip_addr()).collect();
            table.resolved(info.get_fullname(), txt, ips, me, Instant::now())
        }
        ServiceEvent::ServiceRemoved(_, fullname) => table.removed(&fullname),
        _ => Change::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ring::tests::key;

    #[test]
    fn the_txt_round_trips_and_strangers_are_skipped() {
        let id = key(3).public();
        let txt: HashMap<String, String> = txt_of(&id, None, MeshKind::Pi, 47220).into_iter().collect();
        let parsed = parse_txt(|k| txt.get(k).cloned()).unwrap();
        assert_eq!(parsed, Txt { id, ring: None, kind: MeshKind::Pi, port: 47220 });
        let txt: HashMap<String, String> = txt_of(&id, Some("a1b2c3d4"), MeshKind::Pc, 1).into_iter().collect();
        assert_eq!(parse_txt(|k| txt.get(k).cloned()).unwrap().ring.as_deref(), Some("a1b2c3d4"));
        let mut other = txt.clone();
        other.insert("v".into(), "2".into());
        assert_eq!(parse_txt(|k| other.get(k).cloned()), None, "another version");
        let mut bad = txt.clone();
        bad.insert("id".into(), "zz".into());
        assert_eq!(parse_txt(|k| bad.get(k).cloned()), None);
        let mut no_port = txt.clone();
        no_port.insert("port".into(), "0".into());
        assert_eq!(parse_txt(|k| no_port.get(k).cloned()), None);
        let mut odd_kind = txt;
        odd_kind.insert("kind".into(), "fridge".into());
        assert_eq!(parse_txt(|k| odd_kind.get(k).cloned()).unwrap().kind, MeshKind::Pc);
    }

    #[test]
    fn names_and_addresses() {
        assert_eq!(instance_name("Кухня._nikgob._udp.local."), "Кухня");
        assert_eq!(instance_name("My\\.Pi._nikgob._udp.local."), "My.Pi");
        let order = dial_order(["127.0.0.1".parse().unwrap(), "fe80::1".parse().unwrap(), "192.168.0.157".parse().unwrap()], 47220);
        assert_eq!(order, ["192.168.0.157:47220".parse::<SocketAddr>().unwrap()], "loopback and link-local are not the home network");
    }

    #[test]
    fn the_table_keeps_one_entry_a_device_and_asks_about_stale_ones() {
        let me = key(2).public();
        let pi = key(3).public();
        let txt = Txt { id: pi, ring: None, kind: MeshKind::Pi, port: 47220 };
        let mut t = Table::default();
        let t0 = Instant::now();
        let ips = vec!["192.168.0.157".parse().unwrap()];
        assert_eq!(t.resolved("Pi._nikgob._udp.local.", txt.clone(), ips.clone(), &me, t0), Change::Changed);
        assert_eq!(t.resolved("Pi._nikgob._udp.local.", txt.clone(), ips.clone(), &me, t0), Change::Nothing);
        assert_eq!(t.resolved("Кухня._nikgob._udp.local.", txt.clone(), ips.clone(), &me, t0), Change::Changed);
        assert_eq!(t.by_name.len(), 1, "renamed: one entry");
        assert_eq!(t.by_id(&pi).unwrap().name, "Кухня");
        let mine = Txt { id: me, ..txt };
        assert_eq!(t.resolved("Me._nikgob._udp.local.", mine, ips, &me, t0), Change::Nothing, "this instance");
        assert!(t.to_verify(t0 + Duration::from_secs(5)).is_empty());
        let later = t0 + STALE + Duration::from_secs(1);
        assert_eq!(t.to_verify(later), vec!["Кухня._nikgob._udp.local.".to_owned()]);
        assert!(t.to_verify(later + Duration::from_secs(1)).is_empty(), "asked once a while");
        assert_eq!(t.removed("Кухня._nikgob._udp.local."), Change::Changed);
        assert_eq!(t.removed("Кухня._nikgob._udp.local."), Change::Nothing);
    }

    /// Only the home network's addresses: the default route's private
    /// ones first, never Tailscale's, a virtual adapter's, loopback or
    /// link-local; a virtual switch the default route goes through counts.
    #[test]
    fn only_lan_addresses_are_announced_and_dialled() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let ifaces = [
            ("Tailscale", ip("100.101.1.2"), true),
            ("vEthernet (WSL)", ip("172.28.80.1"), true),
            ("Loopback Pseudo-Interface 1", ip("127.0.0.1"), true),
            ("Ethernet", ip("10.0.0.7"), true),
            ("Wi-Fi", ip("192.168.0.157"), true),
            ("Wi-Fi", ip("fe80::1"), true),
            ("Wi-Fi 2", ip("192.168.5.2"), false),
            ("docker0", ip("172.17.0.1"), true),
        ];
        assert_eq!(pick_lan(&ifaces, Some(ip("192.168.0.157"))), [ip("192.168.0.157"), ip("10.0.0.7")]);
        assert_eq!(pick_lan(&ifaces, Some(ip("100.101.1.2"))), [ip("10.0.0.7"), ip("192.168.0.157")], "an exit node: no route to prefer");
        let bridged = [("vEthernet (External)", ip("192.168.0.20"), true), ("vEthernet (WSL)", ip("172.28.80.1"), true)];
        assert_eq!(pick_lan(&bridged, Some(ip("192.168.0.20"))), [ip("192.168.0.20")]);
        assert!(!lan_ip_ok(ip("100.64.0.1")) && lan_ip_ok(ip("100.128.0.1")));
        assert!(!lan_ip_ok(ip("fd7a:115c:a1e0::1")) && is_private(ip("fd00::1")));
        assert!(virtual_iface("tailscale0") && virtual_iface("vEthernet (Default Switch)") && !virtual_iface("Wi-Fi") && !virtual_iface("eth0"));
        let order = dial_order([ip("100.101.1.2"), ip("fe80::2"), ip("2a02::5"), ip("192.168.0.157")], 47220);
        assert_eq!(order, [SocketAddr::new(ip("192.168.0.157"), 47220), SocketAddr::new(ip("2a02::5"), 47220)]);
    }
}
