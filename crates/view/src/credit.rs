//! The artists of a track or album line, for links on each name.

use std::ops::Range;

/// One credited name, the artist it links to (if any) and the text that
/// follows it (`", "`, `" feat. "`; empty for the last).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CreditName<Id> {
    pub name: String,
    pub id: Option<Id>,
    pub join: String,
}

/// The credited artists of a line, in order. `Id` is a library
/// [`crate::ArtistId`] or a service's [`crate::RemoteId`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Credit<Id>(pub Vec<CreditName<Id>>);

impl<Id> Default for Credit<Id> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<Id: Clone + PartialEq> Credit<Id> {
    pub fn single(name: impl Into<String>, id: Option<Id>) -> Self {
        Self(vec![CreditName { name: name.into(), id, join: String::new() }])
    }

    /// Names with the same `join` between each (`", "` for a service's list).
    pub fn joined(names: impl IntoIterator<Item = (String, Option<Id>)>, join: &str) -> Self {
        let mut out: Vec<CreditName<Id>> = names
            .into_iter()
            .map(|(name, id)| CreditName { name, id, join: join.to_owned() })
            .collect();
        if let Some(last) = out.last_mut() {
            last.join.clear();
        }
        Self(out)
    }

    /// The line as shown: names glued with their joins (a missing join
    /// between two names reads `", "`).
    pub fn text(&self) -> String {
        let mut out = String::new();
        let last = self.0.len().saturating_sub(1);
        for (i, n) in self.0.iter().enumerate() {
            out.push_str(&n.name);
            if i < last {
                out.push_str(if n.join.is_empty() { ", " } else { &n.join });
            }
        }
        out
    }

    /// The first (main) artist.
    pub fn main(&self) -> Option<&Id> {
        self.0.first().and_then(|n| n.id.as_ref())
    }

    /// Each artist that has a page, once, with the name it is credited as.
    pub fn linked(&self) -> Vec<(&str, &Id)> {
        let mut out: Vec<(&str, &Id)> = Vec::new();
        for n in &self.0 {
            if let Some(id) = &n.id
                && !out.iter().any(|(_, o)| *o == id)
            {
                out.push((n.name.as_str(), id));
            }
        }
        out
    }

    /// Where each linked name is in `line` (byte ranges, in order), with its
    /// artist. `line` is the credit's text or starts with it ("A, B ·
    /// альбом"); the names are looked for one after another, so a line
    /// written with other separators still finds them. A name the line
    /// does not have is skipped.
    pub fn ranges_in<'a>(&'a self, line: &str) -> Vec<(Range<usize>, &'a Id)> {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        for n in &self.0 {
            if n.name.is_empty() {
                continue;
            }
            let Some(pos) = line.get(cursor..).and_then(|rest| rest.find(n.name.as_str())) else {
                continue;
            };
            let start = cursor + pos;
            let end = start + n.name.len();
            if let Some(id) = &n.id {
                out.push((start..end, id));
            }
            cursor = end;
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The artists of a library track or album.
pub type ArtistLinks = Credit<crate::ArtistId>;

/// The artists of a service's track or album.
pub type StreamArtistLinks = Credit<crate::RemoteId>;

impl From<&nikgob_core::model::ArtistCredit> for ArtistLinks {
    fn from(c: &nikgob_core::model::ArtistCredit) -> Self {
        Self(
            c.0.iter()
                .map(|p| CreditName { name: p.name.clone(), id: p.artist_id, join: p.join_phrase.clone() })
                .collect(),
        )
    }
}

/// A service lists names; they read `"A, B"` (as `join_names`).
impl From<&[nikgob_core::streaming::ArtistRef]> for StreamArtistLinks {
    fn from(a: &[nikgob_core::streaming::ArtistRef]) -> Self {
        Self::joined(a.iter().map(|r| (r.name.clone(), r.id.clone())), ", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credit() -> Credit<u32> {
        Credit(vec![
            CreditName { name: "Daft Punk".into(), id: Some(1), join: " feat. ".into() },
            CreditName { name: "Pharrell".into(), id: Some(2), join: " & ".into() },
            CreditName { name: "Nile".into(), id: None, join: String::new() },
        ])
    }

    #[test]
    fn text_glues_names_with_their_joins() {
        assert_eq!(credit().text(), "Daft Punk feat. Pharrell & Nile");
        let plain: Credit<u32> = Credit::joined([("A".into(), Some(1)), ("B".into(), None)], ", ");
        assert_eq!(plain.text(), "A, B");
        assert_eq!(plain.0[1].join, "");
    }

    #[test]
    fn ranges_point_at_each_linked_name() {
        let c = credit();
        let line = format!("{} · Random Access Memories", c.text());
        let r = c.ranges_in(&line);
        assert_eq!(r.len(), 2, "Nile has no page");
        assert_eq!(&line[r[0].0.clone()], "Daft Punk");
        assert_eq!(*r[0].1, 1);
        assert_eq!(&line[r[1].0.clone()], "Pharrell");
        assert_eq!(*r[1].1, 2);
    }

    #[test]
    fn ranges_follow_the_order_and_skip_missing_names() {
        let c: Credit<u32> =
            Credit::joined([("A".into(), Some(1)), ("AB".into(), Some(2)), ("Z".into(), Some(3))], ", ");
        // "A" must not be found inside "AB" a second time.
        let r = c.ranges_in("A & AB");
        assert_eq!(r.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>(), [0..1, 4..6]);
        assert!(c.ranges_in("").is_empty());
        let cyr: Credit<u32> = Credit::joined([("Кино".into(), Some(1)), ("Земфира".into(), Some(2))], ", ");
        let line = "Кино, Земфира";
        let r = cyr.ranges_in(line);
        assert_eq!(&line[r[1].0.clone()], "Земфира");
    }

    #[test]
    fn linked_lists_each_artist_once() {
        let c: Credit<u32> =
            Credit::joined([("A".into(), Some(1)), ("A (live)".into(), Some(1)), ("B".into(), None)], ", ");
        assert_eq!(c.linked(), vec![("A", &1)]);
        assert_eq!(c.main(), Some(&1));
    }
}
