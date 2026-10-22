//! What the one-time repairs of the import crate need from the catalog:
//! their `meta` marks, the files of torrents, and dropping what a repair
//! emptied (docs/stage4.md §12.4).

use nikgob_core::model::{ArtistId, ReleaseId, Source};
use rusqlite::params;

use crate::catalog::{SOURCE_COLS, source_from_row};
use crate::db::Library;
use crate::error::Result;
use crate::util::{meta_get, meta_set};

impl Library {
    /// A `meta` value (the marks of one-time repairs).
    pub fn meta_value(&self, key: &str) -> Result<Option<String>> {
        self.read(|c| meta_get(c, key))
    }

    pub fn set_meta_value(&self, key: &str, value: &str) -> Result<()> {
        self.write(|tx| meta_set(tx, key, value))
    }

    /// Live local files that came from torrents, oldest first.
    pub fn torrent_sources(&self) -> Result<Vec<Source>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SOURCE_COLS} FROM source
                  WHERE kind = 'local_file' AND origin_kind = 'torrent' AND trashed_at IS NULL
                  ORDER BY added_at, id"
            ))?;
            let rows = stmt.query_map([], source_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Removes a release without tracks (its artist links, palette and
    /// like go too). Returns whether it was removed.
    pub fn drop_empty_release(&self, release: ReleaseId) -> Result<bool> {
        let now = self.tick();
        let device = self.device_id();
        self.write(|tx| {
            let rid = release.to_string();
            let empty: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM release WHERE id = ?1)
                    AND NOT EXISTS(SELECT 1 FROM track WHERE release_id = ?1)",
                [&rid],
                |r| r.get(0),
            )?;
            if !empty {
                return Ok(false);
            }
            tx.execute("DELETE FROM release_artist WHERE release_id = ?1", [&rid])?;
            tx.execute("DELETE FROM palette WHERE release_id = ?1", [&rid])?;
            tx.execute(
                "UPDATE likes SET deleted = 1, updated_at = ?2, updated_by = ?3
                  WHERE target_kind = 'release' AND target_id = ?1 AND deleted = 0",
                params![rid, now, device.as_str()],
            )?;
            tx.execute("DELETE FROM release WHERE id = ?1", [&rid])?;
            Ok(true)
        })
    }

    /// Removes an artist no track or release credits any more (with its
    /// photo). Returns whether it was removed.
    pub fn drop_orphan_artist(&self, artist: ArtistId) -> Result<bool> {
        self.write(|tx| {
            let a = artist.to_string();
            let orphan: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM artist WHERE id = ?1)
                    AND NOT EXISTS(SELECT 1 FROM track_artist WHERE artist_id = ?1)
                    AND NOT EXISTS(SELECT 1 FROM release_artist WHERE artist_id = ?1)",
                [&a],
                |r| r.get(0),
            )?;
            if !orphan {
                return Ok(false);
            }
            tx.execute("DELETE FROM artist_photo WHERE artist_id = ?1", [&a])?;
            tx.execute("DELETE FROM external_id WHERE kind = 'artist' AND local_id = ?1", [&a])?;
            tx.execute("DELETE FROM artist WHERE id = ?1", [&a])?;
            Ok(true)
        })
    }
}
