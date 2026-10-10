//! Trash bookkeeping (DB side only).
//!
//! The import crate moves the file into `Trash/` and then calls
//! [`Library::trash_source`] with the new path. After the retention period,
//! [`Library::purge_trash`] deletes the rows and returns the files the caller
//! must remove from disk.
//!
//! Tracks are never removed here: a track whose last source is trashed stays in
//! playlists and likes, it just has no file ("нет файла").

use nikgob_core::model::{Source, SourceId, UnixMs};
use rusqlite::params;

use crate::catalog::{SOURCE_COLS, refresh_active, source_by_id, source_from_row};
use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::types::{PurgedFile, TrashEntry};
use crate::util::now_ms;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

impl Library {
    /// Marks a source as trashed now. `trash_path` is where the file now lives
    /// under `Trash/` (if the caller moved it). The track's active source is
    /// re-selected.
    pub fn trash_source(&self, source: SourceId, trash_path: Option<&str>) -> Result<()> {
        self.trash_source_at(source, trash_path, now_ms())
    }

    /// Like [`Self::trash_source`] with an explicit timestamp.
    pub fn trash_source_at(
        &self,
        source: SourceId,
        trash_path: Option<&str>,
        at: UnixMs,
    ) -> Result<()> {
        self.write(|tx| {
            let src = source_by_id(tx, source)?
                .ok_or_else(|| LibraryError::not_found("source", source))?;
            tx.execute(
                "UPDATE source SET trashed_at = ?2, trash_path = ?3, is_active = 0
                  WHERE id = ?1 AND trashed_at IS NULL",
                params![source.to_string(), at, trash_path],
            )?;
            refresh_active(tx, src.track_id)?;
            Ok(())
        })
    }

    /// Takes a source back out of Trash. Fails if another live source with the
    /// same `(kind, locator)` exists meanwhile.
    pub fn restore_source(&self, source: SourceId) -> Result<Source> {
        self.write(|tx| {
            let src = source_by_id(tx, source)?
                .ok_or_else(|| LibraryError::not_found("source", source))?;
            tx.execute(
                "UPDATE source SET trashed_at = NULL, trash_path = NULL WHERE id = ?1",
                [source.to_string()],
            )?;
            refresh_active(tx, src.track_id)?;
            source_by_id(tx, source)?.ok_or_else(|| LibraryError::not_found("source", source))
        })
    }

    /// Everything in Trash, most recently trashed first.
    pub fn list_trash(&self) -> Result<Vec<TrashEntry>> {
        self.read(|c| {
            let cols = SOURCE_COLS
                .split(", ")
                .map(|c| format!("s.{}", c.trim()))
                .collect::<Vec<_>>()
                .join(", ");
            let mut stmt = c.prepare(&format!(
                "SELECT {cols}, s.trash_path, t.title, t.artist_display
                   FROM source s JOIN track t ON t.id = s.track_id
                  WHERE s.trashed_at IS NOT NULL ORDER BY s.trashed_at DESC, s.id"
            ))?;
            let n = SOURCE_COLS.split(',').count();
            let rows = stmt.query_map([], |r| {
                let source = source_from_row(r)?;
                Ok(TrashEntry {
                    trashed_at: source.trashed_at.unwrap_or_default(),
                    source,
                    trash_path: r.get(n)?,
                    track_title: r.get(n + 1)?,
                    artist: r.get(n + 2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Deletes trashed sources older than `days` and returns the files to
    /// remove from disk.
    pub fn purge_trash(&self, older_than_days: u32) -> Result<Vec<PurgedFile>> {
        self.purge_trash_before(now_ms() - i64::from(older_than_days) * DAY_MS)
    }

    /// Deletes trashed sources trashed strictly before `cutoff` (unix ms).
    pub fn purge_trash_before(&self, cutoff: UnixMs) -> Result<Vec<PurgedFile>> {
        self.write(|tx| {
            let purged = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT {SOURCE_COLS}, trash_path FROM source
                      WHERE trashed_at IS NOT NULL AND trashed_at < ?1 ORDER BY trashed_at, id"
                ))?;
                let n = SOURCE_COLS.split(',').count();
                let rows = stmt.query_map([cutoff], |r| {
                    let source = source_from_row(r)?;
                    let trash_path: Option<String> = r.get(n)?;
                    Ok((source, trash_path))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let purged = purged
                .into_iter()
                .map(|(source, trash_path)| {
                    let path = match trash_path {
                        Some(p) => Some(p),
                        // No file went to Trash, so the original locator is all
                        // we know. A newer live source may sit exactly there
                        // (an upgrade lands on the old path when the old file
                        // was already gone): that file must not be deleted.
                        None if source.kind.is_local() => {
                            let live: bool = tx.query_row(
                                "SELECT EXISTS(SELECT 1 FROM source
                                  WHERE kind = ?1 AND locator = ?2 AND trashed_at IS NULL)",
                                params![source.kind.as_str(), source.locator],
                                |r| r.get(0),
                            )?;
                            (!live).then(|| source.locator.clone())
                        }
                        None => None,
                    };
                    Ok(PurgedFile {
                        source_id: source.id,
                        track_id: source.track_id,
                        path,
                        kind: source.kind,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            tx.execute(
                "DELETE FROM source WHERE trashed_at IS NOT NULL AND trashed_at < ?1",
                [cutoff],
            )?;
            Ok(purged)
        })
    }
}
