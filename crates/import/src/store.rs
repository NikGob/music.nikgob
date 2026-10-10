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
use crate::identity::{Identity, credit_parts, parse_credit};
use crate::layout::VARIOUS_ARTISTS;

const PAGE: u32 = 500;

pub(crate) struct Store {
    lib: Library,
    loaded: bool,
    artists: HashMap<String, ArtistId>,
    releases: HashMap<(String, String), ReleaseId>,
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

impl Store {
    pub fn new(lib: Library) -> Self {
        Self {
            lib,
            loaded: false,
            artists: HashMap::new(),
            releases: HashMap::new(),
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
                .artists(offset, PAGE, ArtistSort::asc(Default::default()))?;
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
                .releases(offset, PAGE, ReleaseSort::asc(Default::default()))?;
            for row in &page {
                let r = &row.release;
                self.releases
                    .entry((album_key(&r.title), name_key(&r.artist_credit.display())))
                    .or_insert(r.id);
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
        if let Some(m) = &id.release_mbid
            && let Some(r) = self.releases_by_mbid.get(m)
        {
            return Ok(Some(*r));
        }
        let key = (
            album_key(&Self::release_title(id)),
            name_key(&Self::release_artist_line(id)),
        );
        Ok(self.releases.get(&key).copied())
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
        self.ensure_credit(&parts, id.artist_mbid.as_deref())
    }

    /// Creates or updates the release of this identity.
    pub fn ensure_release(&mut self, id: &Identity, cover_ref: Option<String>) -> Result<Release> {
        self.ensure_loaded()?;
        let existing = self.find_release(id)?;
        let line = Self::release_artist_line(id);
        let parts = parse_credit(&line);
        let parts = if parts.is_empty() {
            vec![(String::new(), line.clone())]
        } else {
            parts
        };
        // The album artist's MBID is unknown here; the track artist's MBID only
        // fits when both are the same person.
        let same_artist = id.album_artist.is_none() || id.album_artist.as_deref() == Some(&id.artist_line);
        let credit = self.ensure_credit(
            &parts,
            if same_artist { id.artist_mbid.as_deref() } else { None },
        )?;
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
        self.releases.insert(
            (
                album_key(&stored.title),
                name_key(&stored.artist_credit.display()),
            ),
            stored.id,
        );
        if let Some(m) = &stored.mbid {
            self.releases_by_mbid.insert(m.clone(), stored.id);
        }
        Ok(stored)
    }
}
