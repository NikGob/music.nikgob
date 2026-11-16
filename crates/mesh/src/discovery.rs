//! The instances on the home network (mDNS, `_nikgob._udp.local.`): this
//! one announced while the mesh is on (instance = its name; TXT `v`, `id`,
//! `ring`, `kind`, `port`), the others browsed all the time. An entry not
//! heard of for a while is asked about again; one that does not answer
//! goes (the daemon then reports it removed).
//!
//! Only the home network counts: the addresses announced are private ones
//! ([`lan_ip_ok`]: `10/8`, `172.16/12`, `192.168/16`, IPv6 `fc00::/7` but
//! Tailscale's) of real cards ([`lan_addrs`]: never Tailscale, Hyper-V/WSL
//! `vEthernet`, Docker, VirtualBox, VMware, ZeroTier, a tunnel or a
//! point-to-point link, whatever routes through it), those of the
//! interface the default route goes through first. Another device's
//! address is dialled or remembered only inside the subnet of one of those
//! cards ([`on_lan`]): a private address routed elsewhere (a VPN's, an
//! office's) is not the home network, nor a public one heard of or kept
//! from before.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, UdpSocket};
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

/// The addresses to dial, the home network's only ([`lan_ip_ok`]), IPv4
/// before IPv6.
pub fn dial_order(ips: impl IntoIterator<Item = IpAddr>, port: u16) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = ips.into_iter().map(|ip| ip.to_canonical()).filter(|ip| lan_ip_ok(*ip)).map(|ip| SocketAddr::new(ip, port)).collect();
    v.sort_by_key(|a| (!a.is_ipv4(), a.ip().to_string()));
    v.dedup();
    v
}

/// Tailscale's IPv6 range, `fd7a:115c:a1e0::/48`.
fn is_tailscale_v6(v: Ipv6Addr) -> bool {
    v.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0]
}

/// Whether `ip` is one the owner's devices may reach each other at on the
/// home network: a private one ([`is_private`]; an IPv4 one written as
/// IPv6 counts as itself). Public, loopback, link-local and Tailscale's
/// addresses never are.
pub fn lan_ip_ok(ip: IpAddr) -> bool {
    is_private(ip.to_canonical())
}

/// A private address: RFC 1918 (`10/8`, `172.16/12`, `192.168/16`), or an
/// IPv6 unique local one (`fc00::/7`) that is not Tailscale's.
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

/// A subnet this device is on: a real card's private address and the
/// length of its prefix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subnet {
    pub ip: IpAddr,
    pub prefix: u8,
}

impl Subnet {
    /// Whether `ip` is inside it (an IPv4 one written as IPv6 as itself).
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.ip, ip.to_canonical()) {
            (IpAddr::V4(a), IpAddr::V4(b)) => same_prefix(u128::from(u32::from(a)) << 96, u128::from(u32::from(b)) << 96, self.prefix.min(32)),
            (IpAddr::V6(a), IpAddr::V6(b)) => same_prefix(u128::from(a), u128::from(b), self.prefix.min(128)),
            _ => false,
        }
    }
}

/// Whether `a` and `b` agree in their first `bits` bits.
fn same_prefix(a: u128, b: u128, bits: u8) -> bool {
    bits == 0 || (a ^ b) >> (128 - u32::from(bits)) == 0
}

/// The home network's subnets among `ifaces` (name, address, prefix
/// length, usable: up and not point-to-point): private addresses of real
/// cards whose prefix names a network (IPv4 /8 or longer, IPv6 /16 or
/// longer).
pub fn subnets_in(ifaces: &[(&str, IpAddr, u8, bool)]) -> Vec<Subnet> {
    let mut out: Vec<Subnet> = ifaces
        .iter()
        .filter(|(name, ip, prefix, usable)| *usable && lan_ip_ok(*ip) && !virtual_iface(name) && *prefix >= if ip.is_ipv4() { 8 } else { 16 })
        .map(|(_, ip, prefix, _)| Subnet { ip: *ip, prefix: *prefix })
        .collect();
    out.dedup();
    out
}

/// Whether another device's address `ip` may be dialled or remembered: a
/// private address inside one of this device's home network `subnets`.
pub fn on_lan(ip: IpAddr, subnets: &[Subnet]) -> bool {
    let ip = ip.to_canonical();
    lan_ip_ok(ip) && subnets.iter().any(|s| s.contains(ip))
}

/// This computer's interface addresses: name, address, prefix length,
/// usable (up and not a point-to-point link).
fn local_ifaces() -> Vec<(String, IpAddr, u8, bool)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            let down = matches!(i.oper_status, if_addrs::IfOperStatus::Down | if_addrs::IfOperStatus::NotPresent | if_addrs::IfOperStatus::LowerLayerDown);
            let prefix = match &i.addr {
                if_addrs::IfAddr::V4(a) => a.prefixlen,
                if_addrs::IfAddr::V6(a) => a.prefixlen,
            };
            let usable = !down && !i.is_p2p;
            (i.name.clone(), i.ip(), prefix, usable)
        })
        .collect()
}

/// This computer's home network subnets ([`subnets_in`]).
pub fn lan_subnets() -> Vec<Subnet> {
    let ifaces = local_ifaces();
    let list: Vec<(&str, IpAddr, u8, bool)> = ifaces.iter().map(|(n, ip, p, u)| (n.as_str(), *ip, *p, *u)).collect();
    subnets_in(&list)
}

/// The LAN addresses among `ifaces` (name, address, up), best first: those
/// of the interface the default route goes through (`route`: its
/// address), then the others; only private ones ([`lan_ip_ok`]) of real
/// cards — never a virtual interface's, whatever routes through it.
pub fn pick_lan(ifaces: &[(&str, IpAddr, bool)], route: Option<IpAddr>) -> Vec<IpAddr> {
    let route_iface = route.filter(|r| lan_ip_ok(*r)).and_then(|r| ifaces.iter().find(|(_, ip, _)| *ip == r).map(|(n, ..)| *n));
    let mut out: Vec<(bool, bool, IpAddr)> = ifaces
        .iter()
        .filter(|(name, ip, up)| *up && lan_ip_ok(*ip) && !virtual_iface(name))
        .map(|(name, ip, _)| (route_iface != Some(*name), ip.is_ipv6(), *ip))
        .collect();
    out.sort();
    out.dedup_by_key(|(.., ip)| *ip);
    out.into_iter().map(|(.., ip)| ip).collect()
}

/// This computer's LAN addresses, best first ([`pick_lan`]; a
/// point-to-point link counts as down).
pub fn lan_addrs() -> Vec<IpAddr> {
    let ifaces = local_ifaces();
    let list: Vec<(&str, IpAddr, bool)> = ifaces.iter().map(|(n, ip, _, u)| (n.as_str(), *ip, *u)).collect();
    pick_lan(&list, default_route_ip())
}

/// The virtual interfaces mDNS keeps quiet on: all of them.
pub fn quiet_ifaces() -> Vec<String> {
    let ifaces = if_addrs::get_if_addrs().unwrap_or_default();
    let mut names: Vec<String> = ifaces.iter().filter(|i| virtual_iface(&i.name)).map(|i| i.name.clone()).collect();
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

    /// Only the home network's addresses: private ones of real cards, the
    /// default route's first; never Tailscale's, a virtual adapter's (one
    /// the default route goes through too), public, loopback or
    /// link-local ones.
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
        assert!(pick_lan(&bridged, Some(ip("192.168.0.20"))).is_empty(), "a virtual switch never, the default route through it or not");
        let public = [("Ethernet", ip("85.64.1.2"), true), ("Wi-Fi", ip("2a02::5"), true), ("Wi-Fi", ip("fd12::5"), true)];
        assert_eq!(pick_lan(&public, Some(ip("85.64.1.2"))), [ip("fd12::5")]);
        assert!(!lan_ip_ok(ip("100.64.0.1")) && !lan_ip_ok(ip("100.128.0.1")) && !lan_ip_ok(ip("8.8.8.8")));
        assert!(lan_ip_ok(ip("10.1.2.3")) && lan_ip_ok(ip("172.16.0.1")) && !lan_ip_ok(ip("172.32.0.1")) && lan_ip_ok(ip("192.168.1.1")));
        assert!(!lan_ip_ok(ip("fd7a:115c:a1e0::1")) && lan_ip_ok(ip("fd00::1")) && !lan_ip_ok(ip("2a02::5")));
        assert!(lan_ip_ok(ip("::ffff:192.168.0.5")) && !lan_ip_ok(ip("::ffff:8.8.8.8")));
        assert!(!lan_ip_ok(ip("127.0.0.1")) && !lan_ip_ok(ip("169.254.1.1")) && !lan_ip_ok(ip("fe80::1")));
        for name in ["tailscale0", "vEthernet (Default Switch)", "VirtualBox Host-Only Network", "VMware Network Adapter VMnet8", "docker0", "wg0", "tun0", "tap0", "utun3", "ZeroTier One [abc]", "zt7nnig26"] {
            assert!(virtual_iface(name), "{name}");
        }
        assert!(!virtual_iface("Wi-Fi") && !virtual_iface("eth0") && !virtual_iface("Ethernet 2") && !virtual_iface("wlan0") && !virtual_iface("enp3s0"));
        let order = dial_order([ip("100.101.1.2"), ip("fe80::2"), ip("2a02::5"), ip("fd00::9"), ip("192.168.0.157"), ip("::ffff:10.0.0.4")], 47220);
        assert_eq!(order, [SocketAddr::new(ip("10.0.0.4"), 47220), SocketAddr::new(ip("192.168.0.157"), 47220), SocketAddr::new(ip("fd00::9"), 47220)]);
    }

    /// Another device is dialled or remembered only inside the subnet of
    /// one of this device's real cards: a private address routed
    /// elsewhere, a VPN's or a virtual switch's is not the home network.
    #[test]
    fn only_this_networks_subnets_are_dialled() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let ifaces = [
            ("Wi-Fi", ip("192.168.0.157"), 24, true),
            ("Ethernet", ip("10.0.0.7"), 24, true),
            ("Tailscale", ip("100.101.1.2"), 32, true),
            ("vEthernet (WSL)", ip("172.28.80.1"), 20, true),
            ("OpenVPN Data Channel", ip("10.8.0.6"), 24, false),
            ("Wi-Fi", ip("fd12:3456:789a:1::5"), 64, true),
            ("Wi-Fi", ip("fe80::1"), 64, true),
            ("Ethernet 3", ip("192.168.7.2"), 0, true),
        ];
        let subnets = subnets_in(&ifaces);
        assert_eq!(subnets.len(), 3, "{subnets:?}");
        assert!(on_lan(ip("192.168.0.20"), &subnets) && on_lan(ip("::ffff:192.168.0.20"), &subnets));
        assert!(on_lan(ip("10.0.0.200"), &subnets));
        assert!(!on_lan(ip("192.168.1.20"), &subnets), "private, another network: routed");
        assert!(!on_lan(ip("10.8.0.1"), &subnets), "a VPN's");
        assert!(!on_lan(ip("172.28.80.5"), &subnets), "a virtual switch's");
        assert!(!on_lan(ip("192.168.7.9"), &subnets), "a prefix that names no network");
        assert!(on_lan(ip("fd12:3456:789a:1::9"), &subnets) && !on_lan(ip("fd12:3456:789a:2::9"), &subnets));
        assert!(!on_lan(ip("8.8.8.8"), &[Subnet { ip: ip("8.8.8.1"), prefix: 24 }]), "never a public one");
        assert!(!on_lan(ip("192.168.0.20"), &[]), "no card: nothing");
        assert!(Subnet { ip: ip("10.1.2.3"), prefix: 32 }.contains(ip("10.1.2.3")) && !Subnet { ip: ip("10.1.2.3"), prefix: 32 }.contains(ip("10.1.2.4")));
    }
}
