//! Fractional indexing: order keys that can always be squeezed between two others.
//!
//! Playlist entries carry such a key as `position`. Inserting or moving an entry
//! only rewrites that one entry's key, so concurrent edits from several devices
//! merge without renumbering.
//!
//! Keys are base-62 strings (`0-9A-Za-z`, which is also ASCII order, so plain
//! binary string comparison orders them). A key is an *integer part*, whose
//! length is announced by its first letter (`a`..`z` = 2..27 digits,
//! `A`..`Z` = 27..2 digits for negatives), followed by a *fractional part* that
//! never ends in `0`. Appending increments the integer part, so a long run of
//! appends grows keys only logarithmically; repeated inserts into the same gap
//! grow them by about one character per six inserts.
//!
//! This is the algorithm of David Greenspan's "Implementing Fractional
//! Indexing", with a base-62 alphabet.

use std::cmp::Ordering;

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: usize = 62;
const ZERO: u8 = b'0';
const MAX_DIGIT: u8 = b'z';
/// The smallest possible integer part; no key may equal it with an empty fraction.
const SMALLEST_INT: &str = "A00000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FractionalError {
    #[error("invalid order key {0:?}")]
    InvalidKey(String),
    #[error("order keys are not strictly increasing: {0:?} >= {1:?}")]
    OutOfOrder(String, String),
    #[error("order key space exhausted")]
    Exhausted,
}

type Result<T> = std::result::Result<T, FractionalError>;

fn digit_value(c: u8) -> usize {
    // Callers validate the alphabet first.
    DIGITS.iter().position(|&d| d == c).unwrap_or(0)
}

/// Midpoint of two fractional parts (`a < b`; `b == None` means 1.0).
fn midpoint(a: &[u8], b: Option<&[u8]>) -> Vec<u8> {
    if let Some(b) = b {
        // Strip the longest common prefix, padding `a` with zeros as we go.
        let mut n = 0;
        while b
            .get(n)
            .is_some_and(|&bc| bc == a.get(n).copied().unwrap_or(ZERO))
        {
            n += 1;
        }
        if n > 0 {
            let mut out = b[..n].to_vec();
            out.extend(midpoint(&a[n.min(a.len())..], Some(&b[n..])));
            return out;
        }
    }
    // First digits (or lack of digit) differ.
    let digit_a = a.first().map_or(0, |&c| digit_value(c));
    let digit_b = match b {
        Some(b) if !b.is_empty() => digit_value(b[0]),
        _ => BASE,
    };
    if digit_b.saturating_sub(digit_a) > 1 {
        let mid = (digit_a + digit_b).div_ceil(2);
        vec![DIGITS[mid]]
    } else if let Some(b) = b.filter(|b| b.len() > 1) {
        // Consecutive first digits and `b` is longer than one digit: its first
        // digit alone is strictly between `a` and `b`.
        vec![b[0]]
    } else {
        // `b` is absent or a single digit, and a's first digit is the one just
        // below it: keep a's digit and recurse on the rest against "1.0".
        let mut out = vec![DIGITS[digit_a]];
        out.extend(midpoint(a.get(1..).unwrap_or(&[]), None));
        out
    }
}

fn integer_len(head: u8) -> Option<usize> {
    match head {
        b'a'..=b'z' => Some((head - b'a') as usize + 2),
        b'A'..=b'Z' => Some((b'Z' - head) as usize + 2),
        _ => None,
    }
}

fn validate(key: &str) -> Result<()> {
    let bad = || FractionalError::InvalidKey(key.to_owned());
    let bytes = key.as_bytes();
    if key == SMALLEST_INT || bytes.is_empty() {
        return Err(bad());
    }
    let int_len = integer_len(bytes[0]).ok_or_else(bad)?;
    if int_len > bytes.len() {
        return Err(bad());
    }
    if !bytes.iter().all(|c| DIGITS.contains(c)) {
        return Err(bad());
    }
    if bytes.len() > int_len && bytes[bytes.len() - 1] == ZERO {
        return Err(bad());
    }
    Ok(())
}

fn split(key: &str) -> (&str, &str) {
    // `validate` guarantees the head is a letter and the length is in range.
    let int_len = integer_len(key.as_bytes()[0]).unwrap_or(1);
    key.split_at(int_len)
}

fn increment_integer(int: &str) -> Option<String> {
    let mut bytes = int.as_bytes().to_vec();
    let head = bytes[0];
    let mut carry = true;
    for i in (1..bytes.len()).rev() {
        if !carry {
            break;
        }
        let d = digit_value(bytes[i]) + 1;
        if d == BASE {
            bytes[i] = DIGITS[0];
        } else {
            bytes[i] = DIGITS[d];
            carry = false;
        }
    }
    if carry {
        if head == b'Z' {
            return Some(format!("a{}", DIGITS[0] as char));
        }
        if head == b'z' {
            return None;
        }
        let h = head + 1;
        if h > b'a' {
            bytes.push(DIGITS[0]);
        } else {
            bytes.pop();
        }
        bytes[0] = h;
    }
    Some(String::from_utf8(bytes).expect("ascii"))
}

fn decrement_integer(int: &str) -> Option<String> {
    let mut bytes = int.as_bytes().to_vec();
    let head = bytes[0];
    let mut borrow = true;
    for i in (1..bytes.len()).rev() {
        if !borrow {
            break;
        }
        match digit_value(bytes[i]) {
            0 => bytes[i] = MAX_DIGIT,
            d => {
                bytes[i] = DIGITS[d - 1];
                borrow = false;
            }
        }
    }
    if borrow {
        if head == b'a' {
            return Some(format!("Z{}", MAX_DIGIT as char));
        }
        if head == b'A' {
            return None;
        }
        let h = head - 1;
        if h < b'Z' {
            bytes.push(MAX_DIGIT);
        } else {
            bytes.pop();
        }
        bytes[0] = h;
    }
    Some(String::from_utf8(bytes).expect("ascii"))
}

/// A key strictly between `a` and `b`. `None` means "no bound on that side":
/// `key_between(None, None)` is the first key, `key_between(Some(last), None)`
/// appends, `key_between(None, Some(first))` prepends.
pub fn key_between(a: Option<&str>, b: Option<&str>) -> Result<String> {
    if let Some(a) = a {
        validate(a)?;
    }
    if let Some(b) = b {
        validate(b)?;
    }
    if let (Some(a), Some(b)) = (a, b)
        && a.cmp(b) != Ordering::Less
    {
        return Err(FractionalError::OutOfOrder(a.to_owned(), b.to_owned()));
    }

    match (a, b) {
        (None, None) => Ok(format!("a{}", DIGITS[0] as char)),
        (None, Some(b)) => {
            let (ib, fb) = split(b);
            if ib == SMALLEST_INT {
                let mid = midpoint(b"", Some(fb.as_bytes()));
                return Ok(format!("{ib}{}", String::from_utf8_lossy(&mid)));
            }
            if ib < b {
                return Ok(ib.to_owned());
            }
            decrement_integer(ib).ok_or(FractionalError::Exhausted)
        }
        (Some(a), None) => {
            let (ia, fa) = split(a);
            match increment_integer(ia) {
                Some(i) => Ok(i),
                None => {
                    let mid = midpoint(fa.as_bytes(), None);
                    Ok(format!("{ia}{}", String::from_utf8_lossy(&mid)))
                }
            }
        }
        (Some(a), Some(b)) => {
            let (ia, fa) = split(a);
            let (ib, fb) = split(b);
            if ia == ib {
                let mid = midpoint(fa.as_bytes(), Some(fb.as_bytes()));
                return Ok(format!("{ia}{}", String::from_utf8_lossy(&mid)));
            }
            let i = increment_integer(ia).ok_or(FractionalError::Exhausted)?;
            if i.as_str() < b {
                Ok(i)
            } else {
                let mid = midpoint(fa.as_bytes(), None);
                Ok(format!("{ia}{}", String::from_utf8_lossy(&mid)))
            }
        }
    }
}

/// `n` evenly spread keys strictly between `a` and `b`, in increasing order.
pub fn n_keys_between(a: Option<&str>, b: Option<&str>, n: usize) -> Result<Vec<String>> {
    match n {
        0 => Ok(Vec::new()),
        1 => Ok(vec![key_between(a, b)?]),
        _ if b.is_none() => {
            let mut c = key_between(a, b)?;
            let mut out = vec![c.clone()];
            for _ in 0..n - 1 {
                c = key_between(Some(&c), b)?;
                out.push(c.clone());
            }
            Ok(out)
        }
        _ if a.is_none() => {
            let mut c = key_between(a, b)?;
            let mut out = vec![c.clone()];
            for _ in 0..n - 1 {
                c = key_between(a, Some(&c))?;
                out.push(c.clone());
            }
            out.reverse();
            Ok(out)
        }
        _ => {
            let mid = n / 2;
            let c = key_between(a, b)?;
            let mut out = n_keys_between(a, Some(&c), mid)?;
            out.push(c.clone());
            out.extend(n_keys_between(Some(&c), b, n - mid - 1)?);
            Ok(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(a: Option<&str>, b: Option<&str>) -> String {
        key_between(a, b).unwrap()
    }

    #[test]
    fn known_vectors() {
        // Reference values from the published base-62 implementation.
        assert_eq!(kb(None, None), "a0");
        assert_eq!(kb(Some("a0"), None), "a1");
        assert_eq!(kb(Some("a1"), None), "a2");
        assert_eq!(kb(Some("a0"), Some("a1")), "a0V");
        assert_eq!(kb(Some("a1"), Some("a2")), "a1V");
        assert_eq!(kb(None, Some("a0")), "Zz");
        assert_eq!(kb(None, Some("Zz")), "Zy");
        assert_eq!(kb(Some("Zz"), Some("a0")), "ZzV");
        assert_eq!(kb(Some("a0"), Some("a0V")), "a0G");
        assert_eq!(kb(Some("a0V"), Some("a1")), "a0l");
        assert_eq!(kb(Some("Zz"), Some("a1")), "a0");
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(
            key_between(Some("a1"), Some("a0")),
            Err(FractionalError::OutOfOrder(..))
        ));
        assert!(matches!(
            key_between(Some("a1"), Some("a1")),
            Err(FractionalError::OutOfOrder(..))
        ));
        assert!(matches!(
            key_between(Some("a10"), None),
            Err(FractionalError::InvalidKey(_))
        ));
        assert!(matches!(
            key_between(Some("!"), None),
            Err(FractionalError::InvalidKey(_))
        ));
        assert!(matches!(
            key_between(Some("a"), None),
            Err(FractionalError::InvalidKey(_))
        ));
    }

    #[test]
    fn append_grows_slowly() {
        let mut last = kb(None, None);
        for _ in 0..5000 {
            let next = kb(Some(&last), None);
            assert!(next > last);
            last = next;
        }
        assert!(last.len() <= 5, "5000 appends gave {last}");
    }

    #[test]
    fn prepend_grows_slowly() {
        let mut first = kb(None, None);
        for _ in 0..5000 {
            let next = kb(None, Some(&first));
            assert!(next < first);
            first = next;
        }
        assert!(first.len() <= 5, "5000 prepends gave {first}");
    }

    #[test]
    fn many_inserts_in_the_same_gap() {
        // Always insert right after the first key: the gap keeps halving.
        let first = kb(None, None);
        let mut second = kb(Some(&first), None);
        let mut all = vec![second.clone()];
        for _ in 0..1000 {
            second = kb(Some(&first), Some(&second));
            assert!(first < second);
            all.push(second.clone());
        }
        // Each new key sorts before the previous one.
        assert!(all.windows(2).all(|w| w[1] < w[0]));
        assert!(second.len() < 400, "{}", second.len());

        // Same, but squeezing towards the upper neighbour.
        let mut lo = first.clone();
        let hi = kb(Some(&first), None);
        let mut keys = Vec::new();
        for _ in 0..1000 {
            lo = kb(Some(&lo), Some(&hi));
            assert!(lo < hi);
            keys.push(lo.clone());
        }
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn n_keys_are_sorted_and_between() {
        for (a, b) in [
            (None, None),
            (Some("a0"), None),
            (None, Some("a0")),
            (Some("a0"), Some("a1")),
            (Some("a0"), Some("a0V")),
        ] {
            for n in [0, 1, 2, 7, 50] {
                let keys = n_keys_between(a, b, n).unwrap();
                assert_eq!(keys.len(), n);
                assert!(keys.windows(2).all(|w| w[0] < w[1]), "{a:?} {b:?} {keys:?}");
                if let (Some(a), Some(k)) = (a, keys.first()) {
                    assert!(a < k.as_str());
                }
                if let (Some(b), Some(k)) = (b, keys.last()) {
                    assert!(k.as_str() < b);
                }
            }
        }
    }

    #[test]
    fn random_walk_stays_consistent() {
        // Deterministic pseudo-random insertions at arbitrary indexes must keep
        // the list sorted and free of duplicates.
        let mut list: Vec<String> = Vec::new();
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        for _ in 0..2000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let idx = (state % (list.len() as u64 + 1)) as usize;
            let a = idx.checked_sub(1).map(|i| list[i].as_str());
            let b = list.get(idx).map(String::as_str);
            let k = key_between(a, b).unwrap();
            list.insert(idx, k);
        }
        assert!(list.windows(2).all(|w| w[0] < w[1]));
    }
}
