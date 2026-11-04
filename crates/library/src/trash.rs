//! Trash bookkeeping (DB side only).
//!
//! The import crate moves the file into `Trash/` and then calls
//! [`Library::trash_source`] with the new path. After the retention period,
//! [`Library::purge_trash`] deletes the rows and returns the files the caller
//! must remove from disk.
//!
//! Source trash (an upgrade replaced the file) never removes the track. A
//! track the user deletes is marked `deleted_at` and its sources are trashed
//! with that same timestamp: the track leaves every listing (the "Корзина"
//! view lists it), [`Library::undelete_track`] brings it back and
//! [`Library::purge_track`] removes it for good.

use nikgob_core::model::{Source, SourceId, TrackId, UnixMs};
use rusqlite::{OptionalExtension, params};

use crate::catalog::{SOURCE_COLS, refresh_active, source_by_id, source_from_row, track_row_at};
use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::streams::refresh_in_library;
use crate::types::{DeletedTrack, PurgedFile, PurgedTrack, TrashEntry};
use crate::util::{get_id, now_ms};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Sources of deleted tracks leave together with their track
/// ([`Library::purge_track`]), not one by one.
const NOT_OF_DELETED: &str = "track_id NOT IN (SELECT id FROM track WHERE deleted_at IS NOT NULL)";

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
            let mut stmt = c.prepare_cached(&format!(
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
                let mut stmt = tx.prepare_cached(&format!(
                    "SELECT {SOURCE_COLS}, trash_path FROM source
                      WHERE trashed_at IS NOT NULL AND trashed_at < ?1 AND {NOT_OF_DELETED}
                      ORDER BY trashed_at, id"
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
                &format!(
                    "DELETE FROM source WHERE trashed_at IS NOT NULL AND trashed_at < ?1
                        AND {NOT_OF_DELETED}"
                ),
                [cutoff],
            )?;
            // A stream track whose last downloaded file is gone for good is
            // a stream track again.
            let mut touched: Vec<String> = purged.iter().map(|p| p.track_id.to_string()).collect();
            touched.dedup();
            for t in &touched {
                refresh_in_library(tx, t)?;
            }
            Ok(purged)
        })
    }
}

// ------------------------------------------------------------ deleted tracks

impl Library {
    /// Marks a track deleted at `at`: it leaves every listing. The caller has
    /// trashed its live sources with the same `at` ([`Self::trash_source_at`]).
    pub fn mark_track_deleted(&self, track: TrackId, at: UnixMs) -> Result<()> {
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE track SET deleted_at = ?2 WHERE id = ?1",
                params![track.to_string(), at],
            )?;
            if n == 0 {
                return Err(LibraryError::not_found("track", track));
            }
            // Its song is «не нужна» on its album from now on.
            crate::unwanted::note_unwanted_in(tx, &track.to_string(), at)?;
            Ok(())
        })
    }

    /// When the track was deleted; `None` for a live (or unknown) track.
    pub fn track_deleted_at(&self, track: TrackId) -> Result<Option<UnixMs>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT deleted_at FROM track WHERE id = ?1",
                [track.to_string()],
                |r| r.get::<_, Option<UnixMs>>(0),
            )
            .optional()?
            .flatten())
        })
    }

    /// The sources trashed together with the track's deletion: what a restore
    /// brings back (older upgrade leftovers stay in Trash).
    pub fn deleted_track_sources(&self, track: TrackId) -> Result<Vec<TrashEntry>> {
        let Some(at) = self.track_deleted_at(track)? else {
            return Ok(Vec::new());
        };
        Ok(self
            .list_trash()?
            .into_iter()
            .filter(|e| e.source.track_id == track && e.trashed_at == at)
            .collect())
    }

    /// Takes the deleted mark off (the caller restored the files and sources).
    pub fn undelete_track(&self, track: TrackId) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE track SET deleted_at = NULL WHERE id = ?1",
                [track.to_string()],
            )?;
            crate::unwanted::forget_unwanted_in(tx, &track.to_string())?;
            refresh_active(tx, track)?;
            Ok(())
        })
    }

    /// Deleted tracks, most recently deleted first.
    pub fn deleted_tracks(&self) -> Result<Vec<DeletedTrack>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(concat!(
                "SELECT t.deleted_at, ",
                track_row_cols!(),
                " ",
                track_row_from!(),
                " WHERE t.deleted_at IS NOT NULL
                  ORDER BY t.deleted_at DESC, t.album_key, t.disc_no, t.track_no, t.seq"
            ))?;
            let rows = stmt.query_map([], |r| {
                Ok(DeletedTrack {
                    deleted_at: r.get(0)?,
                    track: track_row_at(r, 1)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Tracks deleted strictly before `cutoff` (the 30-day auto-purge).
    pub fn deleted_tracks_before(&self, cutoff: UnixMs) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id FROM track WHERE deleted_at IS NOT NULL AND deleted_at < ?1
                  ORDER BY deleted_at, seq",
            )?;
            let rows = stmt.query_map([cutoff], |r| get_id(r, 0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Removes a track for good: its sources, credits, lyrics, analysis and
    /// master rows; its likes and playlist entries are tombstoned (so a sync
    /// does not bring them back); a release left without tracks goes too, and
    /// so do artists left without any credit. Listening history stays (it is
    /// append-only; readers skip events of unknown tracks).
    /// Returns the files the caller must delete.
    pub fn purge_track(&self, track: TrackId) -> Result<PurgedTrack> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let tid = track.to_string();
            let release: Option<String> = tx
                .query_row("SELECT release_id FROM track WHERE id = ?1", [&tid], |r| r.get(0))
                .optional()?
                .ok_or_else(|| LibraryError::not_found("track", track))?;
            // Deleted for good at once (no Trash): its song is «не нужна»
            // too; a row of a track from the Trash stays without it.
            let live: bool = tx.query_row("SELECT deleted_at IS NULL FROM track WHERE id = ?1", [&tid], |r| r.get(0))?;
            if live {
                crate::unwanted::note_unwanted_in(tx, &tid, now)?;
            }
            crate::unwanted::unlink_unwanted_in(tx, &tid)?;
            let mut out = PurgedTrack::default();
            let files: Vec<(String, String, bool, Option<String>)> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT kind, locator, trashed_at IS NOT NULL, trash_path FROM source
                      WHERE track_id = ?1 ORDER BY added_at, id",
                )?;
                let rows = stmt.query_map([&tid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (kind, locator, trashed, trash_path) in files {
                if kind != "local_file" {
                    continue;
                }
                if !out.places.contains(&locator) {
                    out.places.push(locator.clone());
                }
                match (trashed, trash_path) {
                    (true, Some(p)) => out.trash_files.push(p),
                    // Trashed while the file was already gone: its old place
                    // may hold another live file by now.
                    (true, None) => {
                        let taken: bool = tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM source WHERE kind = 'local_file'
                                AND locator = ?1 AND trashed_at IS NULL AND track_id != ?2)",
                            params![locator, tid],
                            |r| r.get(0),
                        )?;
                        if !taken {
                            out.trash_files.push(locator);
                        }
                    }
                    (false, _) => out.live_files.push(locator),
                }
            }
            out.master_hash = tx
                .query_row("SELECT file_hash FROM master WHERE track_id = ?1", [&tid], |r| r.get(0))
                .optional()?;
            let artists: Vec<String> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT artist_id FROM track_artist WHERE track_id = ?1
                     UNION SELECT artist_id FROM release_artist WHERE release_id = ?2",
                )?;
                let rows = stmt.query_map(params![tid, release], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };

            tx.execute("DELETE FROM source WHERE track_id = ?1", [&tid])?;
            for table in ["track_artist", "lyrics", "lyrics_text", "lyrics_pick", "analysis", "analysis_failed", "master"] {
                tx.execute(&format!("DELETE FROM {table} WHERE track_id = ?1"), [&tid])?;
            }
            if let Some(hash) = &out.master_hash {
                tx.execute(
                    "DELETE FROM replica WHERE file_hash = ?1
                       AND NOT EXISTS (SELECT 1 FROM master WHERE file_hash = ?1)",
                    [hash],
                )?;
            }
            tx.execute(
                "UPDATE likes SET deleted = 1, updated_at = ?2, updated_by = ?3
                  WHERE target_kind = 'track' AND target_id = ?1 AND deleted = 0",
                params![tid, now, device.as_str()],
            )?;
            tx.execute(
                "UPDATE playlist_entry SET deleted = 1, updated_at = ?2, updated_by = ?3
                  WHERE track_id = ?1 AND deleted = 0",
                params![tid, now, device.as_str()],
            )?;
            // Nothing points at it any more: its ids in the services go,
            // its «Хочу скачать» items forget it.
            tx.execute("DELETE FROM external_id WHERE kind = 'track' AND local_id = ?1", [&tid])?;
            tx.execute("UPDATE wishlist SET track_id = NULL WHERE track_id = ?1", [&tid])?;
            tx.execute("DELETE FROM track WHERE id = ?1", [&tid])?;

            if let Some(rid) = &release {
                out.release_removed = crate::repairs::drop_empty_release_in(tx, rid, None, device.as_str(), now)?;
            }
            for a in artists {
                crate::repairs::drop_orphan_artist_in(tx, &a)?;
            }
            Ok(out)
        })
    }
}
