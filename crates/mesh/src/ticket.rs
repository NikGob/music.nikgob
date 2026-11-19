//! The invite ticket (`nkm1…`): what a device without a window needs to
//! join the ring — the holder's node id and LAN addresses, the invite's id,
//! a one-time secret (128 bits, the SPAKE2 password) and when it expires.
//! Base32, lowercase, so it survives a shell and a QR code.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use iroh::PublicKey;

pub const PREFIX: &str = "nkm1";
const VERSION: u8 = 1;
/// Addresses a ticket carries at most.
pub const MAX_ADDRS: usize = 8;
/// How long an invite works.
pub const LIFETIME_MS: i64 = 15 * 60 * 1000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub holder: PublicKey,
    pub invite: [u8; 8],
    pub secret: [u8; 16],
    pub expires_ms: i64,
    pub addrs: Vec<SocketAddr>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TicketError {
    /// Not a ticket of this player.
    NotATicket,
    /// A ticket of a newer player.
    Version,
    Expired,
}

impl TicketError {
    /// What the owner is told about `nikgobd pair`'s argument, in `loc`.
    pub fn message_in(&self, loc: nikgob_core::i18n::Locale) -> String {
        let s = nikgob_core::i18n::mesh_in(loc);
        match self {
            Self::NotATicket => s.not_a_ticket.to_owned(),
            Self::Version => s.newer_invite.to_owned(),
            Self::Expired => s.invite_expired.to_owned(),
        }
    }
}

impl std::fmt::Display for TicketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotATicket => "not an invite ticket",
            Self::Version => "an invite ticket of a newer version",
            Self::Expired => "the invite ticket has expired",
        })
    }
}

impl Ticket {
    pub fn encode(&self) -> String {
        let mut b = Vec::with_capacity(70 + self.addrs.len() * 19);
        b.push(VERSION);
        b.extend_from_slice(self.holder.as_bytes());
        b.extend_from_slice(&self.invite);
        b.extend_from_slice(&self.secret);
        b.extend_from_slice(&self.expires_ms.to_be_bytes());
        let addrs: Vec<&SocketAddr> = self.addrs.iter().take(MAX_ADDRS).collect();
        b.push(addrs.len() as u8);
        for a in addrs {
            match a.ip() {
                IpAddr::V4(ip) => {
                    b.push(4);
                    b.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    b.push(6);
                    b.extend_from_slice(&ip.octets());
                }
            }
            b.extend_from_slice(&a.port().to_be_bytes());
        }
        format!("{PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&b).to_ascii_lowercase())
    }

    /// A ticket as pasted: spaces and line breaks are ignored, the case too.
    /// An expired one decodes (the caller says so with [`Self::expired`]).
    pub fn decode(text: &str) -> Result<Self, TicketError> {
        let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        let lower = compact.to_ascii_lowercase();
        let body = lower.strip_prefix(PREFIX).ok_or(TicketError::NotATicket)?;
        let bytes = data_encoding::BASE32_NOPAD.decode(body.to_ascii_uppercase().as_bytes()).map_err(|_| TicketError::NotATicket)?;
        let mut r = Reader { b: &bytes, at: 0 };
        match r.u8()? {
            VERSION => {}
            v if v > VERSION => return Err(TicketError::Version),
            _ => return Err(TicketError::NotATicket),
        }
        let holder = PublicKey::from_bytes(&r.array::<32>()?).map_err(|_| TicketError::NotATicket)?;
        let invite = r.array::<8>()?;
        let secret = r.array::<16>()?;
        let expires_ms = i64::from_be_bytes(r.array::<8>()?);
        let n = r.u8()? as usize;
        if n > MAX_ADDRS {
            return Err(TicketError::NotATicket);
        }
        let mut addrs = Vec::with_capacity(n);
        for _ in 0..n {
            let ip = match r.u8()? {
                4 => IpAddr::V4(Ipv4Addr::from(r.array::<4>()?)),
                6 => IpAddr::V6(Ipv6Addr::from(r.array::<16>()?)),
                _ => return Err(TicketError::NotATicket),
            };
            let port = u16::from_be_bytes(r.array::<2>()?);
            addrs.push(SocketAddr::new(ip, port));
        }
        if r.at != bytes.len() {
            return Err(TicketError::NotATicket);
        }
        Ok(Self { holder, invite, secret, expires_ms, addrs })
    }

    pub fn expired(&self, now_ms: i64) -> bool {
        now_ms >= self.expires_ms
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, TicketError> {
        let v = *self.b.get(self.at).ok_or(TicketError::NotATicket)?;
        self.at += 1;
        Ok(v)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], TicketError> {
        let s = self.b.get(self.at..self.at + N).ok_or(TicketError::NotATicket)?;
        self.at += N;
        let mut a = [0u8; N];
        a.copy_from_slice(s);
        Ok(a)
    }
}

/// The one-line command that pairs a Linux device with a ticket.
pub fn linux_command(ticket: &str) -> String {
    format!("nikgobd pair {ticket}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Ticket {
        Ticket {
            holder: iroh::SecretKey::from_bytes(&[7; 32]).public(),
            invite: [1, 2, 3, 4, 5, 6, 7, 8],
            secret: [9; 16],
            expires_ms: 1_763_300_000_000,
            addrs: vec!["192.168.0.10:47220".parse().unwrap(), "[fe80::1]:47220".parse().unwrap()],
        }
    }

    #[test]
    fn a_ticket_round_trips_and_survives_pasting() {
        let t = sample();
        let s = t.encode();
        assert!(s.starts_with("nkm1"));
        assert!(s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()), "{s}");
        assert_eq!(Ticket::decode(&s), Ok(t.clone()));
        let pasted = format!("  {}\n{}  ", &s[..20].to_ascii_uppercase(), &s[20..]);
        assert_eq!(Ticket::decode(&pasted), Ok(t.clone()));
        assert_eq!(linux_command(&s), format!("nikgobd pair {s}"));
        let none = Ticket { addrs: Vec::new(), ..t };
        assert_eq!(Ticket::decode(&none.encode()), Ok(none));
    }

    #[test]
    fn a_broken_or_foreign_ticket_is_refused() {
        let s = sample().encode();
        assert_eq!(Ticket::decode("hello"), Err(TicketError::NotATicket));
        assert_eq!(Ticket::decode(&s[..s.len() - 4]), Err(TicketError::NotATicket));
        assert_eq!(Ticket::decode(&format!("{s}aa")), Err(TicketError::NotATicket));
        let mut b = data_encoding::BASE32_NOPAD.decode(s[4..].to_ascii_uppercase().as_bytes()).unwrap();
        b[0] = 9;
        let newer = format!("nkm1{}", data_encoding::BASE32_NOPAD.encode(&b).to_ascii_lowercase());
        assert_eq!(Ticket::decode(&newer), Err(TicketError::Version));
    }

    #[test]
    fn a_ticket_expires() {
        let t = sample();
        assert!(!t.expired(t.expires_ms - 1));
        assert!(t.expired(t.expires_ms));
        assert_eq!(TicketError::Expired.message_in(nikgob_core::i18n::Locale::Ru), "Приглашение истекло");
        assert_eq!(TicketError::Expired.message_in(nikgob_core::i18n::Locale::En), "The invite has expired");
        assert_eq!(TicketError::NotATicket.message_in(nikgob_core::i18n::Locale::En), "Not an invite");
    }
}
