//! The tokens of the local REST API: `nk_` and 32 base62 characters from
//! the system's random source (about 190 bits). Only the BLAKE3 hash of a
//! token is kept; a token given to the API is hashed and compared with
//! each kept hash in constant time.

/// Every token starts with this.
pub const PREFIX: &str = "nk_";

/// Base62 characters after [`PREFIX`].
pub const SECRET_LEN: usize = 32;

/// Characters of a token id.
pub const ID_LEN: usize = 10;

const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// `n` base62 characters from the system's random source, drawn without
/// bias (a byte of 248 or more is thrown away). `None` when the system
/// gives no randomness.
fn base62(n: usize) -> Option<String> {
    let mut out = String::with_capacity(n);
    let mut buf = [0u8; 64];
    while out.len() < n {
        getrandom::fill(&mut buf).ok()?;
        for b in buf {
            if out.len() == n {
                break;
            }
            // 248 = 62 * 4: below it every character is as likely.
            if b < 248 {
                out.push(ALPHABET[usize::from(b % 62)] as char);
            }
        }
    }
    Some(out)
}

/// A new secret token (shown to the user once, never kept).
pub fn generate() -> Option<String> {
    Some(format!("{PREFIX}{}", base62(SECRET_LEN)?))
}

/// A new token id (kept with the hash, shown nowhere).
pub fn new_id() -> Option<String> {
    base62(ID_LEN)
}

/// The kept form of a token: its BLAKE3 hash in hex.
pub fn hash(secret: &str) -> String {
    blake3::hash(secret.as_bytes()).to_hex().to_string()
}

/// Whether `secret` has the shape of a token (cheap, before hashing).
pub fn well_formed(secret: &str) -> bool {
    secret
        .strip_prefix(PREFIX)
        .is_some_and(|rest| rest.len() == SECRET_LEN && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// A kept hash parsed once, for [`Hashed::matches`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hashed(blake3::Hash);

impl Hashed {
    /// `None` for a hash that is not 64 hex digits.
    pub fn parse(hex: &str) -> Option<Self> {
        blake3::Hash::from_hex(hex.trim()).ok().map(Self)
    }

    /// Hashes a presented secret once, to compare with every kept hash.
    pub fn of(secret: &str) -> Self {
        Self(blake3::hash(secret.as_bytes()))
    }

    /// Constant-time comparison (`blake3::Hash`'s equality is).
    pub fn matches(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl std::fmt::Debug for Hashed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hashed(<redacted>)")
    }
}

/// Whether `secret` is the token kept as `hash_hex`.
pub fn matches(secret: &str, hash_hex: &str) -> bool {
    Hashed::parse(hash_hex).is_some_and(|kept| kept.matches(&Hashed::of(secret)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_nk_and_32_base62() {
        let t = generate().expect("randomness");
        assert!(t.starts_with("nk_"), "{t}");
        assert_eq!(t.len(), 3 + SECRET_LEN);
        assert!(t[3..].bytes().all(|b| b.is_ascii_alphanumeric()));
        assert!(well_formed(&t));
        assert!(!well_formed("nk_short"));
        assert!(!well_formed(&t.replace("nk_", "xx_")));
        assert!(!well_formed(&format!("{t}-")));
        let id = new_id().expect("randomness");
        assert_eq!(id.len(), ID_LEN);
    }

    #[test]
    fn tokens_do_not_repeat() {
        let all: std::collections::HashSet<String> = (0..200).map(|_| generate().unwrap()).collect();
        assert_eq!(all.len(), 200);
    }

    #[test]
    fn every_character_turns_up() {
        // 62 characters, thousands drawn: each is there (a biased or
        // truncated draw would leave some out).
        let text: String = (0..200).map(|_| generate().unwrap()[3..].to_owned()).collect();
        for c in ALPHABET {
            assert!(text.as_bytes().contains(c), "{}", *c as char);
        }
    }

    #[test]
    fn only_the_hash_is_kept_and_it_verifies() {
        let t = generate().unwrap();
        let h = hash(&t);
        assert_eq!(h.len(), 64);
        assert!(!h.contains(&t[3..]));
        assert!(matches(&t, &h));
        assert!(!matches(&generate().unwrap(), &h));
        assert!(!matches(&t, "not hex"));
        assert!(!matches(&t, ""));
        let kept = Hashed::parse(&h).unwrap();
        assert!(kept.matches(&Hashed::of(&t)));
        assert!(!format!("{kept:?}").contains(&h));
        // A known value: BLAKE3 of the empty string.
        assert_eq!(hash(""), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
    }
}
