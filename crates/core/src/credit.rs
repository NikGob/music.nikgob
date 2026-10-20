//! Splitting an artist credit line ("A feat. B", "A, B & C") into the
//! artists it names, keeping the text between them so the line reads the
//! same when glued back ([`crate::model::ArtistCredit::display`]).
//!
//! Every part is `(join_before, name)`: the text between the previous name
//! and this one (empty for the first).
//!
//! * `feat.`, `ft.`, `featuring` and `; ` always separate artists;
//! * `, ` and ` / ` separate artists, except where the right side is clearly
//!   part of one name (`Tyler, The Creator`, `Sammy Davis, Jr.`);
//! * ` & ` separates artists only inside a list (`A, B & C`, `A feat. B &
//!   C`): a lone `Simon & Garfunkel` is one name, unless the caller knows
//!   both sides as artists ([`split_pair`]);
//! * a few well-known names with commas or ampersands are never split.

/// Markers that always start a guest credit (lowercase, matched without case).
const FEAT_MARKERS: &[&str] = &[" feat. ", " feat ", " ft. ", " featuring ", "; "];

/// List separators: always split (with the exceptions of [`keeps_comma`]).
const LIST_SEPARATORS: &[&str] = &[", ", " / "];

/// Names that hold a list separator or an ampersand but are one act.
const KNOWN_NAMES: &[&str] = &[
    "earth, wind & fire",
    "crosby, stills, nash & young",
    "crosby, stills & nash",
    "emerson, lake & palmer",
    "blood, sweat & tears",
    "peter, paul and mary",
    "simon & garfunkel",
    "hall & oates",
    "daryl hall & john oates",
    "mumford & sons",
    "above & beyond",
    "chase & status",
    "belle & sebastian",
    "iron & wine",
    "brooks & dunn",
    "big & rich",
    "matt & kim",
    "ike & tina turner",
    "sam & dave",
    "kc & the sunshine band",
    "ac/dc",
];

fn is_known(name: &str) -> bool {
    let folded = name.trim().to_lowercase();
    KNOWN_NAMES.contains(&folded.as_str())
}

/// `Tyler, The Creator`, `Sammy Davis, Jr.`: the comma is inside one name.
fn keeps_comma(right: &str) -> bool {
    let r = right.trim_start().to_lowercase();
    r.starts_with("the ")
        || ["jr", "jr.", "sr", "sr.", "ii", "iii"].contains(&r.as_str())
}

/// `Nick Cave & The Bad Seeds`, `Чиж & Co`: the ampersand is inside one name.
fn keeps_ampersand(right: &str) -> bool {
    let r = right.trim_start().to_lowercase();
    r.starts_with("the ") || ["co", "co.", "his orchestra", "friends"].contains(&r.as_str())
}

/// Splits `text` at every case-insensitive occurrence of one of `seps`
/// (ASCII markers, so byte offsets agree between the text and its
/// lowercase form when the lengths match). Returns `(join_before, piece)`
/// with the author's spelling of each separator; the pieces are untrimmed.
fn split_raw(text: &str, seps: &[&str]) -> Vec<(String, String)> {
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        // Some character changes length when lowercased: offsets would not
        // agree. Match case-sensitively instead.
        return split_exact(text, seps);
    }
    let mut out = Vec::new();
    let mut join = String::new();
    let mut start = 0usize;
    loop {
        let hay = &lower[start..];
        let next = seps
            .iter()
            .filter_map(|m| hay.find(m).map(|i| (i, m.len())))
            .min_by_key(|(i, _)| *i);
        match next {
            Some((i, len)) => {
                out.push((std::mem::take(&mut join), text[start..start + i].to_owned()));
                join = text[start + i..start + i + len].to_owned();
                start += i + len;
            }
            None => {
                out.push((join, text[start..].to_owned()));
                return out;
            }
        }
    }
}

fn split_exact(text: &str, seps: &[&str]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut join = String::new();
    let mut start = 0usize;
    loop {
        let hay = &text[start..];
        let next = seps
            .iter()
            .filter_map(|m| hay.find(m).map(|i| (i, m.len())))
            .min_by_key(|(i, _)| *i);
        match next {
            Some((i, len)) => {
                out.push((std::mem::take(&mut join), text[start..start + i].to_owned()));
                join = text[start + i..start + i + len].to_owned();
                start += i + len;
            }
            None => {
                out.push((join, text[start..].to_owned()));
                return out;
            }
        }
    }
}

/// Splits one name at `seps`, gluing a piece back to the previous one when
/// `keep(right)` says the separator is inside a name. The first part keeps
/// `join` (the text before the whole name).
fn split_name(
    join: &str,
    name: &str,
    seps: &[&str],
    keep: fn(&str) -> bool,
) -> Vec<(String, String)> {
    if is_known(name) {
        return vec![(join.to_owned(), name.to_owned())];
    }
    let mut out: Vec<(String, String)> = Vec::new();
    for (sep, piece) in split_raw(name, seps) {
        match out.last_mut() {
            Some(last) if keep(&piece) || piece.trim().is_empty() || last.1.trim().is_empty() => {
                last.1.push_str(&sep);
                last.1.push_str(&piece);
            }
            Some(_) => out.push((sep, piece)),
            None => out.push((join.to_owned(), piece)),
        }
    }
    out
}

/// Trims names and drops empty ones (their text joins the next separator).
fn tidy(parts: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut carry = String::new();
    for (join, name) in parts {
        let lead = &name[..name.len() - name.trim_start().len()];
        let trail = &name[name.trim_end().len()..];
        let trimmed = name.trim();
        if trimmed.is_empty() {
            carry.push_str(&join);
            carry.push_str(&name);
            continue;
        }
        let mut j = std::mem::take(&mut carry);
        j.push_str(&join);
        j.push_str(lead);
        if out.is_empty() {
            j.clear();
        }
        out.push((j, trimmed.to_owned()));
        carry.push_str(trail);
    }
    out
}

/// Splits a credit line into its artists; see the module docs for the rules.
/// An empty (blank) line gives no parts.
pub fn split_credit(line: &str) -> Vec<(String, String)> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    // Guests first: "A, B feat. C" → ["A, B", "C"].
    let feats = split_raw(line, FEAT_MARKERS);
    let mut parts = Vec::new();
    for (join, name) in feats {
        parts.extend(split_name(&join, &name, LIST_SEPARATORS, keeps_comma));
    }
    let parts = tidy(parts);
    // An ampersand splits only inside a list.
    if parts.len() > 1 {
        let mut out = Vec::new();
        for (join, name) in parts {
            out.extend(split_name(&join, &name, &[" & "], keeps_ampersand));
        }
        tidy(out)
    } else {
        parts
    }
}

/// A lone `A & B` (or `A and B`, `A x B`) as two names, for a caller that
/// has other evidence they are two artists (both are known on their own).
/// `None` when the name is not such a pair.
pub fn split_pair(name: &str) -> Option<[(String, String); 2]> {
    if is_known(name) {
        return None;
    }
    for sep in [" & ", " and ", " x ", " и "] {
        let parts = split_raw(name, &[sep]);
        if let [(_, a), (j, b)] = parts.as_slice()
            && !a.trim().is_empty()
            && !b.trim().is_empty()
            && !keeps_ampersand(b)
        {
            return Some([
                (String::new(), a.trim().to_owned()),
                (j.clone(), b.trim().to_owned()),
            ]);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(line: &str) -> Vec<String> {
        split_credit(line).into_iter().map(|(_, n)| n).collect()
    }

    fn glue(parts: &[(String, String)]) -> String {
        parts.iter().map(|(j, n)| format!("{j}{n}")).collect()
    }

    #[test]
    fn commas_and_guests_split() {
        assert_eq!(names("A, B"), ["A", "B"]);
        assert_eq!(names("Кино feat. Земфира; Bi-2"), ["Кино", "Земфира", "Bi-2"]);
        assert_eq!(names("A / B"), ["A", "B"]);
        assert_eq!(names("Daft Punk feat. Pharrell Williams & Nile Rodgers"), [
            "Daft Punk",
            "Pharrell Williams",
            "Nile Rodgers"
        ]);
        assert_eq!(names("A, B & C"), ["A", "B", "C"]);
        assert_eq!(names("A Ft. B"), ["A", "B"]);
    }

    #[test]
    fn joins_keep_the_original_text() {
        for line in [
            "A, B",
            "A feat. B",
            "A Feat. B & C",
            "Кино feat. Земфира; Bi-2",
            "A, B & C",
            "Tyler, The Creator, A$AP Rocky",
        ] {
            assert_eq!(glue(&split_credit(line)), line, "{line}");
        }
        let p = split_credit("A Feat. B & C");
        assert_eq!(p[1].0, " Feat. ");
        assert_eq!(p[2].0, " & ");
    }

    #[test]
    fn single_names_stay_whole() {
        assert_eq!(names("Simon & Garfunkel"), ["Simon & Garfunkel"]);
        assert_eq!(names("Nick Cave & The Bad Seeds"), ["Nick Cave & The Bad Seeds"]);
        assert_eq!(names("Earth, Wind & Fire"), ["Earth, Wind & Fire"]);
        assert_eq!(names("Tyler, The Creator"), ["Tyler, The Creator"]);
        assert_eq!(names("Earth, Wind & Fire feat. X"), ["Earth, Wind & Fire", "X"]);
        assert_eq!(names("A, Nick Cave & The Bad Seeds"), ["A", "Nick Cave & The Bad Seeds"]);
        assert_eq!(names("AC/DC"), ["AC/DC"]);
        assert_eq!(names("Sammy Davis, Jr."), ["Sammy Davis, Jr."]);
        assert!(names("  ").is_empty());
    }

    #[test]
    fn pairs_need_evidence() {
        let [a, b] = split_pair("Macklemore & Ryan Lewis").unwrap();
        assert_eq!((a.1.as_str(), b.0.as_str(), b.1.as_str()), ("Macklemore", " & ", "Ryan Lewis"));
        assert!(split_pair("Simon & Garfunkel").is_none());
        assert!(split_pair("Kool & The Gang").is_none());
        assert!(split_pair("Solo").is_none());
    }
}
