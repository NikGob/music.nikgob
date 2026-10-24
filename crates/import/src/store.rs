//! Find-or-create of artists and releases.
//!
//! The library matches artists and releases by id or MusicBrainz id only, so the
//! importer keeps name indexes (loaded lazily, updated as rows are created) to
//! avoid a second "Кино" row for every file without MBIDs.

use std::collections::HashMap;

use nikgob_core::model::{Artist, ArtistCredit, ArtistCreditPart, ArtistId, Release, ReleaseId};
use nikgob_library::{ArtistSort, Library, ReleaseSort};
use nikgob_metadata::text::{album_key, artist_key};

use crate::error::Result;
use crate::folder::primary_credit;
use crate::identity::{Identity, credit_parts, parse_credit};
use crate::layout::VARIOUS_ARTISTS;

const PAGE: u32 = 500;

pub(crate) struct Store {
    lib: Library,
    loaded: bool,
    artists: HashMap<String, ArtistId>,
    releases: HashMap<(String, String), ReleaseId>,
    /// By first credited name; `None` when two releases share it.
    releases_by_lead: HashMap<(String, String), Option<ReleaseId>>,
    releases_by_mbid: HashMap<String, ReleaseId>,
}

fn name_key(name: &str) -> String {
    let k = artist_key(name);
    if k.is_empty() {
        nikgob_library::fold(name.trim())
    } else {
        k
    }
}

/// What makes two releases the same one: the album title and the full credit of
/// the artist it is filed under, both case- and punctuation-insensitive. "A & B"
/// and "A & C" are two artists.
pub(crate) fn release_key(title: &str, artist_line: &str) -> (String, String) {
    (album_key(title), name_key(artist_line))
}

/// Like [`release_key`] with only the first name of the credit: where the album
/// artist is missing and a track artist stands in, "A" and "A, B" (a guest) are
/// one artist.
pub(crate) fn lead_key(title: &str, artist_line: &str) -> (String, String) {
    (album_key(title), name_key(primary_credit(artist_line)))
}

/// The bracket closing the last voice credit of `line` (`リコ(CV:富田美憂)`)
/// as the credit's tail.
fn close_credit(credit: &mut ArtistCredit, line: &str) {
    if let Some(last) = credit.0.last_mut() {
        last.join_phrase = nikgob_core::credit::closing_tail(line, &last.name);
    }
}

/// The release has a real album artist (not just the track artist standing in).
fn has_album_artist(id: &Identity) -> bool {
    id.compilation
        || id.release_type == nikgob_core::model::ReleaseType::Compilation
        || id.album_artist.as_deref().is_some_and(|a| !a.trim().is_empty())
}

impl Store {
    pub fn new(lib: Library) -> Self {
        Self {
            lib,
            loaded: false,
            artists: HashMap::new(),
            releases: HashMap::new(),
            releases_by_lead: HashMap::new(),
            releases_by_mbid: HashMap::new(),
        }
    }

    pub fn library(&self) -> &Library {
        &self.lib
    }

    fn ensure_loaded(&mut self) -> Result<()> {
        if self.loaded {
            return Ok(());
        }
        let mut offset = 0;
        loop {
            let page = self
                .lib
                .artists_with_deleted(offset, PAGE, ArtistSort::asc(Default::default()))?;
            for row in &page {
                self.artists
                    .entry(name_key(&row.artist.name))
                    .or_insert(row.artist.id);
            }
            if page.len() < PAGE as usize {
                break;
            }
            offset += PAGE;
        }
        let mut offset = 0;
        loop {
            let page = self
                .lib
                .releases_with_deleted(offset, PAGE, ReleaseSort::asc(Default::default()))?;
            for row in &page {
                let r = &row.release;
                self.index_release(&r.title, &r.artist_credit.display(), r.id);
                if let Some(m) = &r.mbid {
                    self.releases_by_mbid.insert(m.clone(), r.id);
                }
            }
            if page.len() < PAGE as usize {
                break;
            }
            offset += PAGE;
        }
        self.loaded = true;
        Ok(())
    }

    /// The credit line of the release this track belongs to.
    pub fn release_artist_line(id: &Identity) -> String {
        if id.compilation || id.release_type == nikgob_core::model::ReleaseType::Compilation {
            return VARIOUS_ARTISTS.to_owned();
        }
        id.album_artist
            .clone()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| id.artist_line.clone())
    }

    pub fn release_title(id: &Identity) -> String {
        id.album
            .clone()
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| id.title.clone())
    }

    /// The existing release this identity belongs to, if any (nothing is created).
    pub fn find_release(&mut self, id: &Identity) -> Result<Option<ReleaseId>> {
        self.ensure_loaded()?;
        if let Some(r) = id.release_id
            && self.lib.get_release(r)?.is_some()
        {
            return Ok(Some(r));
        }
        if let Some(m) = &id.release_mbid
            && let Some(r) = self.releases_by_mbid.get(m)
        {
            return Ok(Some(*r));
        }
        let (title, line) = (Self::release_title(id), Self::release_artist_line(id));
        if let Some(r) = self.releases.get(&release_key(&title, &line)) {
            return Ok(Some(*r));
        }
        // An explicit album artist matches in full; a track artist standing in
        // for it may carry guests.
        if has_album_artist(id) {
            return Ok(None);
        }
        Ok(self.releases_by_lead.get(&lead_key(&title, &line)).copied().flatten())
    }

    fn index_release(&mut self, title: &str, credit: &str, id: ReleaseId) {
        self.releases.entry(release_key(title, credit)).or_insert(id);
        let lead = self.releases_by_lead.entry(lead_key(title, credit)).or_insert(Some(id));
        if *lead != Some(id) {
            *lead = None;
        }
    }

    /// Artist rows for the credit parts `(join_before, name)`; only the first
    /// artist can carry `first_mbid`.
    pub fn ensure_credit(
        &mut self,
        parts: &[(String, String)],
        first_mbid: Option<&str>,
    ) -> Result<ArtistCredit> {
        self.ensure_loaded()?;
        let mut credit = ArtistCredit::default();
        for (i, (join, name)) in parts.iter().enumerate() {
            let key = name_key(name);
            let mut artist = match self.artists.get(&key) {
                Some(id) => self
                    .lib
                    .get_artist(*id)?
                    .unwrap_or_else(|| Artist::new(name.clone())),
                None => Artist::new(name.clone()),
            };
            if i == 0
                && let Some(m) = first_mbid.filter(|m| !m.is_empty())
                && artist.mbid.is_none()
            {
                artist.mbid = Some(m.to_owned());
            }
            let stored = self.lib.upsert_artist(&artist)?;
            self.artists.insert(key, stored.id);
            credit.0.push(ArtistCreditPart {
                artist_id: Some(stored.id),
                name: name.clone(),
                join_phrase: String::new(),
            });
            if i > 0 {
                let n = credit.0.len();
                credit.0[n - 2].join_phrase = join.clone();
            }
        }
        Ok(credit)
    }

    /// Credit of the track: the individual names aligned with the display line.
    pub fn track_credit(&mut self, id: &Identity) -> Result<ArtistCredit> {
        let parts = credit_parts(&id.artist_line, &id.artists);
        let parts = self.split_known_pairs(parts)?;
        let mut credit = self.ensure_credit(&parts, id.artist_mbid.as_deref())?;
        close_credit(&mut credit, &id.artist_line);
        Ok(credit)
    }

    /// A lone `A & B` is one name for the line splitter (`Simon &
    /// Garfunkel`); it is two artists when the library already knows both
    /// on their own and not the pair.
    fn split_known_pairs(&mut self, parts: Vec<(String, String)>) -> Result<Vec<(String, String)>> {
        self.ensure_loaded()?;
        let mut out = Vec::with_capacity(parts.len());
        for (join, name) in parts {
            let known = |n: &str| self.artists.contains_key(&name_key(n));
            match nikgob_core::credit::split_pair(&name) {
                Some([(_, a), (j, b)]) if !known(&name) && known(&a) && known(&b) => {
                    out.push((join, a));
                    out.push((j, b));
                }
                _ => out.push((join, name)),
            }
        }
        Ok(out)
    }

    /// Creates or updates the release of this identity.
    pub fn ensure_release(&mut self, id: &Identity, cover_ref: Option<String>) -> Result<Release> {
        self.ensure_loaded()?;
        let existing = self.find_release(id)?;
        // A track credited to the artist and a guest must not rename the album
        // it lands on; only a real album artist may.
        let no_album_artist = id.album_artist.as_deref().is_none_or(|a| a.trim().is_empty());
        let old = match existing {
            Some(r) if no_album_artist => self.lib.get_release(r)?,
            _ => None,
        };
        let credit = match old {
            Some(old) => old.artist_credit,
            None => {
                let line = Self::release_artist_line(id);
                let parts = parse_credit(&line);
                let parts = if parts.is_empty() {
                    vec![(String::new(), line.clone())]
                } else {
                    self.split_known_pairs(parts)?
                };
                // The album artist's MBID is unknown here; the track artist's MBID
                // only fits when both are the same person.
                let same_artist = id.album_artist.is_none() || id.album_artist.as_deref() == Some(&id.artist_line);
                let mut credit = self.ensure_credit(
                    &parts,
                    if same_artist { id.artist_mbid.as_deref() } else { None },
                )?;
                close_credit(&mut credit, &line);
                credit
            }
        };
        let mut release = Release::new(Self::release_title(id), credit);
        if let Some(r) = existing {
            release.id = r;
        }
        release.mbid = id.release_mbid.clone();
        release.release_group_mbid = id.release_group_mbid.clone();
        release.year = id.year;
        release.release_type = id.release_type;
        release.total_tracks = id.track_total;
        release.cover_ref = cover_ref;
        let stored = self.lib.upsert_release(&release)?;
        self.index_release(&stored.title, &stored.artist_credit.display(), stored.id);
        if let Some(m) = &stored.mbid {
            self.releases_by_mbid.insert(m.clone(), stored.id);
        }
        Ok(stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::identity_from_tags;
    use crate::tags::TagSet;
    use std::path::Path;

    fn identity(artist: &str, album_artist: Option<&str>, album: &str) -> Identity {
        identity_from_tags(
            &TagSet {
                title: Some("Song".into()),
                artist: Some(artist.into()),
                album_artist: album_artist.map(Into::into),
                album: Some(album.into()),
                ..TagSet::default()
            },
            Path::new("x.flac"),
        )
    }

    #[test]
    fn explicit_album_artists_are_compared_in_full() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        let ab = store.ensure_release(&identity("A", Some("A & B"), "Live"), None).unwrap();
        let ac = store.ensure_release(&identity("A", Some("A & C"), "Live"), None).unwrap();
        assert_ne!(ab.id, ac.id);
        assert_eq!(ab.artist_credit.display(), "A & B", "the second import did not overwrite the credit");
        let again = store.ensure_release(&identity("A", Some("a & b"), "LIVE"), None).unwrap();
        assert_eq!(again.id, ab.id, "case does not matter");
    }

    #[test]
    fn a_track_artist_standing_in_for_the_album_artist_may_carry_guests() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        let plain = store.ensure_release(&identity("Radwimps", Some("Radwimps"), "Suzume"), None).unwrap();
        let guest = identity("RADWIMPS, Kazuma Jinnouchi", None, "Suzume");
        assert_eq!(store.find_release(&guest).unwrap(), Some(plain.id));
        // Two releases that share the first name make the guess ambiguous.
        store.ensure_release(&identity("Radwimps", Some("Radwimps & Friend"), "Suzume"), None).unwrap();
        assert_eq!(store.find_release(&guest).unwrap(), None);
        // An explicit album artist never falls back to the first name.
        let explicit = identity("Radwimps", Some("Radwimps, Kazuma Jinnouchi"), "Suzume");
        assert_eq!(store.find_release(&explicit).unwrap(), None);
    }

    #[test]
    fn a_track_of_two_artists_links_to_both() {
        use nikgob_core::model::Track;
        let lib = Library::open_in_memory().unwrap();
        let mut store = Store::new(lib.clone());
        let id = identity("Кино, Земфира", None, "Live");
        let credit = store.track_credit(&id).unwrap();
        assert_eq!(credit.0.len(), 2);
        assert_eq!(credit.0[0].name, "Кино");
        assert_eq!(credit.0[1].name, "Земфира");
        assert_eq!(credit.display(), "Кино, Земфира", "the line reads as tagged");
        let ids: Vec<_> = credit.artist_ids().collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        lib.upsert_track(&Track::new("Song", credit)).unwrap();
        for a in ids {
            let rows = lib.tracks_of_artist(a, 0, 10).unwrap();
            assert_eq!(rows.len(), 1, "each artist page has the track");
            assert_eq!(rows[0].credit.0.len(), 2);
        }
        // A lone "A & B" splits once both are known on their own.
        let pair = store.track_credit(&identity("Кино & Земфира", None, "X")).unwrap();
        assert_eq!(pair.0.len(), 2);
        let band = store.track_credit(&identity("Simon & Garfunkel", None, "X")).unwrap();
        assert_eq!(band.0.len(), 1);
    }

    #[test]
    fn credit_parts_keep_the_closing_bracket() {
        let mut store = Store::new(Library::open_in_memory().unwrap());
        // The names one by one (the ARTISTS tag) and as one line.
        let mut tagged = identity("リコ(CV:富田美憂)", None, "Deep in Abyss");
        tagged.artists = vec!["リコ".into(), "富田美憂".into()];
        let line = identity("リコ(CV:富田美憂)", None, "Deep in Abyss");
        let full = identity("リコ（CV：富田美憂）", None, "Deep in Abyss");
        for id in [&tagged, &line, &full] {
            let credit = store.track_credit(id).unwrap();
            let names: Vec<_> = credit.0.iter().map(|p| p.name.as_str()).collect();
            assert_eq!(names, ["リコ", "富田美憂"], "{}", id.artist_line);
            assert_eq!(credit.display(), id.artist_line, "the line reads as tagged");
        }
        let a = store.track_credit(&tagged).unwrap();
        let b = store.track_credit(&full).unwrap();
        assert_eq!(a.artist_ids().collect::<Vec<_>>(), b.artist_ids().collect::<Vec<_>>(), "one character, one actor");
    }
}
