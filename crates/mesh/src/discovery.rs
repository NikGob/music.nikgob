//! The instances on the home network (mDNS, `_nikgob._udp.local.`): this
//! one announced while the mesh is on (instance = its name; TXT `v`, `id`,
//! `ring`, `kind`, `port`), the others browsed all the time. An entry not
//! heard of for a while is asked about again; one that does not answer
//! goes (the daemon then reports it removed).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
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
    let mut v: Vec<SocketAddr> = ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect();
    v.sort_by_key(|a| (a.ip().is_loopback(), !a.is_ipv4(), a.ip().to_string()));
    v.dedup();
    v
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

/// This instance's announcement.
pub fn announce(daemon: &ServiceDaemon, name: &str, id: &PublicKey, ring: Option<&str>, kind: MeshKind, port: u16) -> Result<String, String> {
    let host = format!("nikgob-{}.local.", &id.to_string()[..12]);
    let props = txt_of(id, ring, kind, port);
    let info = ServiceInfo::new(SERVICE, name, &host, "", port, props.as_slice()).map_err(|e| e.to_string())?.enable_addr_auto();
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
        assert_eq!(order[0], "192.168.0.157:47220".parse::<SocketAddr>().unwrap());
        assert_eq!(order[2], "127.0.0.1:47220".parse::<SocketAddr>().unwrap());
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
}
