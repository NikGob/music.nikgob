//! Playlists and likes: the CRDT-friendly user data.
//!
//! * Every row carries `updated_at` / `updated_by` / `deleted`; nothing is ever
//!   hard-deleted, removal is a tombstone.
//! * Playlist entries have their own uuid and a fractional-index `position`;
//!   insert and move rewrite only that one entry, remove tombstones it.
//! * Editing entries does not touch the playlist row, so a concurrent rename
//!   on another device is never overwritten by an unrelated entry edit.
//! * Likes are an LWW-element-set keyed by `(kind, id)`.

use nikgob_core::model::{
    DeviceId, EntryId, LikeTarget, Playlist, PlaylistEntry, PlaylistId, ReleaseId, TrackId, UnixMs,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::catalog::{
    ARTIST_ROW_SELECT, RELEASE_ROW_SELECT, artist_row, release_row, track_row_at,
};
use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::fractional::{key_between, n_keys_between};
use crate::streams::{refresh_in_library, refresh_in_library_of_playlist};
use crate::types::{
    ArtistRow, PlaylistEntryRow, PlaylistKind, PlaylistSummary, ReleaseRow, TrackRow,
};
use crate::util::get_id;

/// Likes (or, `deleted`, unlikes) inside a transaction.
pub(crate) fn set_like_in(
    tx: &Connection,
    target: LikeTarget,
    deleted: bool,
    now: UnixMs,
    device: &DeviceId,
) -> Result<()> {
    tx.execute(
        "INSERT INTO likes(target_kind, target_id, updated_at, updated_by, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(target_kind, target_id) DO UPDATE SET
            updated_at = excluded.updated_at, updated_by = excluded.updated_by,
            deleted = excluded.deleted",
        params![target.kind_str(), target.id_string(), now, device.as_str(), deleted],
    )?;
    if let LikeTarget::Track(t) = target {
        refresh_in_library(tx, &t.to_string())?;
    }
    Ok(())
}

pub(crate) fn is_liked_in(c: &Connection, target: LikeTarget) -> Result<bool> {
    Ok(c
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM likes
                            WHERE target_kind = ?1 AND target_id = ?2 AND deleted = 0)",
        )?
        .query_row(params![target.kind_str(), target.id_string()], |r| r.get(0))?)
}

/// Name of the pinned virtual playlist of liked tracks.
pub const LIKED_PLAYLIST_NAME: &str = "Любимые";

pub(crate) const PLAYLIST_COLS: &str =
    "id, name, cover, created_at, updated_at, updated_by, deleted";

pub(crate) fn playlist_from_row(row: &Row<'_>) -> rusqlite::Result<Playlist> {
    Ok(Playlist {
        id: get_id(row, 0)?,
        name: row.get(1)?,
        cover: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        updated_by: DeviceId(row.get(5)?),
        deleted: row.get(6)?,
    })
}

pub(crate) const ENTRY_COLS: &str =
    "id, playlist_id, track_id, position, added_at, updated_at, updated_by, deleted";

pub(crate) fn entry_from_row(row: &Row<'_>) -> rusqlite::Result<PlaylistEntry> {
    Ok(PlaylistEntry {
        id: get_id(row, 0)?,
        playlist_id: get_id(row, 1)?,
        track_id: get_id(row, 2)?,
        position: row.get(3)?,
        added_at: row.get(4)?,
        updated_at: row.get(5)?,
        updated_by: DeviceId(row.get(6)?),
        deleted: row.get(7)?,
    })
}

/// Live playlists with track counts; add a `WHERE`/`ORDER BY` as needed.
pub(crate) const PLAYLIST_SUMMARY_SELECT: &str = "\
SELECT p.id, p.name, p.cover,
       max(p.updated_at, coalesce((SELECT MAX(e.updated_at) FROM playlist_entry e
                                    WHERE e.playlist_id = p.id), 0)),
       (SELECT COUNT(*) FROM playlist_entry e JOIN track t ON t.id = e.track_id
         WHERE e.playlist_id = p.id AND e.deleted = 0 AND t.deleted_at IS NULL)
  FROM playlist p";

pub(crate) fn playlist_summary(row: &Row<'_>) -> rusqlite::Result<PlaylistSummary> {
    Ok(PlaylistSummary {
        id: Some(get_id(row, 0)?),
        kind: PlaylistKind::User,
        name: row.get(1)?,
        cover: row.get(2)?,
        updated_at: row.get(3)?,
        track_count: row.get::<_, i64>(4)? as u32,
    })
}

fn live_playlist(conn: &Connection, id: PlaylistId) -> Result<Playlist> {
    conn.query_row(
        &format!("SELECT {PLAYLIST_COLS} FROM playlist WHERE id = ?1 AND deleted = 0"),
        [id.to_string()],
        playlist_from_row,
    )
    .optional()?
    .ok_or_else(|| LibraryError::not_found("playlist", id))
}

fn live_entry(conn: &Connection, id: EntryId) -> Result<PlaylistEntry> {
    conn.query_row(
        &format!("SELECT {ENTRY_COLS} FROM playlist_entry WHERE id = ?1 AND deleted = 0"),
        [id.to_string()],
        entry_from_row,
    )
    .optional()?
    .ok_or_else(|| LibraryError::not_found("playlist entry", id))
}

/// `(id, position)` of live entries in playlist order.
fn ordered_entries(conn: &Connection, playlist: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, position FROM playlist_entry
          WHERE playlist_id = ?1 AND deleted = 0 ORDER BY position, id",
    )?;
    let rows = stmt.query_map([playlist], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Rewrites every live entry's position with evenly spread fresh keys, keeping
/// the order. Used when neighbouring keys have become equal (concurrent
/// inserts into the same gap on two devices) or malformed.
fn rebalance(conn: &Connection, playlist: &str, now: UnixMs, device: &DeviceId) -> Result<()> {
    let entries = ordered_entries(conn, playlist)?;
    let keys = n_keys_between(None, None, entries.len())?;
    let mut upd = conn.prepare_cached(
        "UPDATE playlist_entry SET position = ?2, updated_at = ?3, updated_by = ?4 WHERE id = ?1",
    )?;
    for ((id, _), key) in entries.iter().zip(&keys) {
        upd.execute(params![id, key, now, device.as_str()])?;
    }
    Ok(())
}

/// A position key for slot `index` among the live entries, ignoring `exclude`
/// (the entry being moved). Rebalances once if the neighbours are unusable.
fn position_for(
    conn: &Connection,
    playlist: &str,
    exclude: Option<&str>,
    index: usize,
    now: UnixMs,
    device: &DeviceId,
) -> Result<String> {
    for attempt in 0..2 {
        let entries: Vec<_> = ordered_entries(conn, playlist)?
            .into_iter()
            .filter(|(id, _)| Some(id.as_str()) != exclude)
            .collect();
        let idx = index.min(entries.len());
        let prev = idx.checked_sub(1).map(|i| entries[i].1.as_str());
        let next = entries.get(idx).map(|e| e.1.as_str());
        match key_between(prev, next) {
            Ok(key) => return Ok(key),
            Err(e) if attempt == 1 => return Err(e.into()),
            Err(e) => {
                log::warn!("rebalancing playlist {playlist}: {e}");
                rebalance(conn, playlist, now, device)?;
            }
        }
    }
    unreachable!("loop returns on both attempts")
}

fn insert_entry(
    conn: &Connection,
    playlist: PlaylistId,
    track: TrackId,
    index: usize,
    now: UnixMs,
    device: &DeviceId,
) -> Result<PlaylistEntry> {
    insert_new_entry(conn, playlist, track, index, now, device).map(|(e, _)| e)
}

/// [`insert_entry`], and whether the entry is new.
fn insert_new_entry(
    conn: &Connection,
    playlist: PlaylistId,
    track: TrackId,
    index: usize,
    now: UnixMs,
    device: &DeviceId,
) -> Result<(PlaylistEntry, bool)> {
    live_playlist(conn, playlist)?;
    let pid = playlist.to_string();
    // A track lives in a playlist at most once: re-adding returns the existing entry.
    let existing = conn
        .query_row(
            &format!(
                "SELECT {ENTRY_COLS} FROM playlist_entry
                  WHERE playlist_id = ?1 AND track_id = ?2 AND deleted = 0 LIMIT 1"
            ),
            params![pid, track.to_string()],
            entry_from_row,
        )
        .optional()?;
    if let Some(entry) = existing {
        return Ok((entry, false));
    }
    let position = position_for(conn, &pid, None, index, now, device)?;
    let entry = PlaylistEntry {
        id: EntryId::new(),
        playlist_id: playlist,
        track_id: track,
        position,
        added_at: now,
        updated_at: now,
        updated_by: device.clone(),
        deleted: false,
    };
    conn.execute(
        "INSERT INTO playlist_entry(id, playlist_id, track_id, position, added_at, updated_at,
                                    updated_by, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, 0)",
        params![
            entry.id.to_string(),
            pid,
            track.to_string(),
            entry.position,
            now,
            device.as_str()
        ],
    )?;
    refresh_in_library(conn, &track.to_string())?;
    Ok((entry, true))
}

impl Library {
    // ---- playlists

    pub fn create_playlist(&self, name: &str) -> Result<Playlist> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let playlist = Playlist {
                id: PlaylistId::new(),
                name: name.to_owned(),
                cover: None,
                created_at: now,
                updated_at: now,
                updated_by: device,
                deleted: false,
            };
            tx.execute(
                "INSERT INTO playlist(id, name, cover, created_at, updated_at, updated_by, deleted)
                 VALUES (?1, ?2, NULL, ?3, ?3, ?4, 0)",
                params![
                    playlist.id.to_string(),
                    playlist.name,
                    now,
                    playlist.updated_by.as_str()
                ],
            )?;
            Ok(playlist)
        })
    }

    pub fn rename_playlist(&self, id: PlaylistId, name: &str) -> Result<()> {
        self.update_playlist_row(id, "name = ?4", Some(name.to_owned()))
    }

    pub fn set_playlist_cover(&self, id: PlaylistId, cover: Option<&str>) -> Result<()> {
        self.update_playlist_row(id, "cover = ?4", cover.map(str::to_owned))
    }

    fn update_playlist_row(
        &self,
        id: PlaylistId,
        assignment: &str,
        value: Option<String>,
    ) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            live_playlist(tx, id)?;
            tx.execute(
                &format!(
                    "UPDATE playlist SET updated_at = ?2, updated_by = ?3, {assignment}
                      WHERE id = ?1 AND deleted = 0"
                ),
                params![id.to_string(), now, device.as_str(), value],
            )?;
            Ok(())
        })
    }

    /// Tombstones the playlist (its entries stay, hidden).
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            live_playlist(tx, id)?;
            tx.execute(
                "UPDATE playlist SET deleted = 1, updated_at = ?2, updated_by = ?3 WHERE id = ?1",
                params![id.to_string(), now, device.as_str()],
            )?;
            refresh_in_library_of_playlist(tx, &id.to_string())?;
            Ok(())
        })
    }

    /// A live playlist; `None` when unknown or deleted.
    pub fn get_playlist(&self, id: PlaylistId) -> Result<Option<Playlist>> {
        self.read(|c| match live_playlist(c, id) {
            Ok(p) => Ok(Some(p)),
            Err(LibraryError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        })
    }

    /// The virtual "Любимые" playlist first (pinned), then user playlists by
    /// most recent change.
    pub fn list_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        let liked = self.liked_count()?;
        let mut out = vec![PlaylistSummary {
            id: None,
            kind: PlaylistKind::Liked,
            name: LIKED_PLAYLIST_NAME.to_owned(),
            cover: None,
            track_count: liked,
            updated_at: self.read(|c| {
                Ok(c.query_row(
                    "SELECT coalesce(MAX(updated_at), 0) FROM likes WHERE target_kind = 'track'",
                    [],
                    |r| r.get(0),
                )?)
            })?,
        }];
        out.extend(self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "{PLAYLIST_SUMMARY_SELECT} WHERE p.deleted = 0 ORDER BY 4 DESC, p.seq DESC"
            ))?;
            let rows = stmt.query_map([], playlist_summary)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?);
        Ok(out)
    }

    // ---- entries

    /// One page of a playlist, in order. Entries whose track is not in the
    /// catalog (yet) are skipped.
    pub fn playlist_entries(
        &self,
        playlist: PlaylistId,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<PlaylistEntryRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(concat!(
                "SELECT e.id, e.playlist_id, e.track_id, e.position, e.added_at, e.updated_at, \
                        e.updated_by, e.deleted, ",
                track_row_cols!(),
                " FROM playlist_entry e JOIN track t ON t.id = e.track_id \
                  LEFT JOIN release r ON r.id = t.release_id \
                  WHERE e.playlist_id = ?1 AND e.deleted = 0 AND t.deleted_at IS NULL \
                  ORDER BY e.position, e.id LIMIT ?2 OFFSET ?3"
            ))?;
            let rows = stmt.query_map(params![playlist.to_string(), limit, offset], |r| {
                Ok(PlaylistEntryRow {
                    entry: entry_from_row(r)?,
                    track: track_row_at(r, 8)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Number of visible entries.
    pub fn playlist_len(&self, playlist: PlaylistId) -> Result<u32> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM playlist_entry e JOIN track t ON t.id = e.track_id
                  WHERE e.playlist_id = ?1 AND e.deleted = 0 AND t.deleted_at IS NULL",
                [playlist.to_string()],
                |r| r.get(0),
            )?)
        })
    }

    /// Appends a track.
    pub fn playlist_add(&self, playlist: PlaylistId, track: TrackId) -> Result<PlaylistEntry> {
        self.playlist_insert(playlist, track, usize::MAX)
    }

    /// Inserts a track so it ends up at `index` (clamped to the end).
    pub fn playlist_insert(
        &self,
        playlist: PlaylistId,
        track: TrackId,
        index: usize,
    ) -> Result<PlaylistEntry> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| insert_entry(tx, playlist, track, index, now, &device))
    }

    /// Appends several tracks in one transaction.
    pub fn playlist_add_many(
        &self,
        playlist: PlaylistId,
        tracks: &[TrackId],
    ) -> Result<Vec<PlaylistEntry>> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            tracks
                .iter()
                .map(|t| insert_entry(tx, playlist, *t, usize::MAX, now, &device))
                .collect()
        })
    }

    /// Appends the tracks the playlist does not have yet, each once, in one
    /// transaction: how many were added and how many it had already.
    pub fn playlist_add_missing(&self, playlist: PlaylistId, tracks: &[TrackId]) -> Result<(usize, usize)> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let (mut added, mut already) = (0, 0);
            let mut seen = std::collections::HashSet::new();
            for t in tracks {
                if !seen.insert(*t) {
                    continue;
                }
                if insert_new_entry(tx, playlist, *t, usize::MAX, now, &device)?.1 {
                    added += 1;
                } else {
                    already += 1;
                }
            }
            Ok((added, already))
        })
    }

    /// Moves an entry so it ends up at `new_index` in the list (clamped to the
    /// end). Only that entry's `position` changes.
    pub fn playlist_move(&self, entry: EntryId, new_index: usize) -> Result<PlaylistEntry> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let current = live_entry(tx, entry)?;
            let pid = current.playlist_id.to_string();
            let eid = entry.to_string();
            let position = position_for(tx, &pid, Some(&eid), new_index, now, &device)?;
            tx.execute(
                "UPDATE playlist_entry SET position = ?2, updated_at = ?3, updated_by = ?4
                  WHERE id = ?1",
                params![eid, position, now, device.as_str()],
            )?;
            live_entry(tx, entry)
        })
    }

    /// Tombstones an entry.
    pub fn playlist_remove(&self, entry: EntryId) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let removed = live_entry(tx, entry)?;
            tx.execute(
                "UPDATE playlist_entry SET deleted = 1, updated_at = ?2, updated_by = ?3
                  WHERE id = ?1",
                params![entry.to_string(), now, device.as_str()],
            )?;
            refresh_in_library(tx, &removed.track_id.to_string())?;
            Ok(())
        })
    }

    // ---- likes

    pub fn like(&self, target: impl Into<LikeTarget>) -> Result<()> {
        self.set_like(target.into(), false)
    }

    /// A like set at `at` rather than now (the demo's history of likes).
    pub fn like_at(&self, target: impl Into<LikeTarget>, at: UnixMs) -> Result<()> {
        let device = self.device_id();
        self.write(|tx| set_like_in(tx, target.into(), false, at, &device))
    }

    pub fn unlike(&self, target: impl Into<LikeTarget>) -> Result<()> {
        self.set_like(target.into(), true)
    }

    fn set_like(&self, target: LikeTarget, deleted: bool) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| set_like_in(tx, target, deleted, now, &device))
    }

    pub fn is_liked(&self, target: impl Into<LikeTarget>) -> Result<bool> {
        let target = target.into();
        self.read(|c| is_liked_in(c, target))
    }

    /// Liked tracks, most recently liked first (the content of "Любимые").
    pub fn liked_tracks(&self, offset: u32, limit: u32) -> Result<Vec<TrackRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(concat!(
                "SELECT ",
                track_row_cols!(),
                " FROM likes l JOIN track t ON t.id = l.target_id \
                  LEFT JOIN release r ON r.id = t.release_id \
                  WHERE l.target_kind = 'track' AND l.deleted = 0 AND t.deleted_at IS NULL \
                  ORDER BY l.updated_at DESC, t.seq DESC LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit, offset], |r| track_row_at(r, 0))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn liked_count(&self) -> Result<u32> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM likes l JOIN track t ON t.id = l.target_id
                  WHERE l.target_kind = 'track' AND l.deleted = 0 AND t.deleted_at IS NULL",
                [],
                |r| r.get(0),
            )?)
        })
    }

    pub fn liked_releases(&self, offset: u32, limit: u32) -> Result<Vec<ReleaseRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "{RELEASE_ROW_SELECT}
                  JOIN likes l ON l.target_kind = 'release' AND l.target_id = r.id
                 WHERE l.deleted = 0 ORDER BY l.updated_at DESC, r.seq DESC LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit, offset], release_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn liked_artists(&self, offset: u32, limit: u32) -> Result<Vec<ArtistRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "{ARTIST_ROW_SELECT}
                  JOIN likes l ON l.target_kind = 'artist' AND l.target_id = a.id
                 WHERE l.deleted = 0 ORDER BY l.updated_at DESC, a.seq DESC LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit, offset], artist_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Likes (or unlikes) an album: every live track of it goes into (or out
    /// of) "Любимые" — the local ones when it has any, all of them otherwise —
    /// and the release's own like row follows, so the album reads as liked
    /// exactly when [`crate::ReleaseRow::liked`] says so. Returns the tracks.
    pub fn like_release(&self, release: ReleaseId, on: bool) -> Result<Vec<TrackId>> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let rid = release.to_string();
            let ids: Vec<String> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT t.id FROM track t
                      WHERE t.release_id = ?1 AND t.deleted_at IS NULL
                        AND (t.has_local = 1 OR NOT EXISTS (
                              SELECT 1 FROM track t2 WHERE t2.release_id = ?1
                                 AND t2.deleted_at IS NULL AND t2.has_local = 1))
                      ORDER BY t.disc_no, t.track_no, t.seq",
                )?;
                let rows = stmt.query_map([&rid], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            let mut upsert = tx.prepare_cached(
                "INSERT INTO likes(target_kind, target_id, updated_at, updated_by, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(target_kind, target_id) DO UPDATE SET
                    updated_at = excluded.updated_at, updated_by = excluded.updated_by,
                    deleted = excluded.deleted
                  WHERE likes.deleted != excluded.deleted",
            )?;
            // Album order in "Любимые" (most recent first): the first track
            // gets the newest stamp.
            for (i, id) in ids.iter().enumerate() {
                let at = now - i as i64;
                upsert.execute(params!["track", id, at, device.as_str(), !on])?;
            }
            upsert.execute(params!["release", rid, now, device.as_str(), !on])?;
            for id in &ids {
                refresh_in_library(tx, id)?;
            }
            ids.iter().map(|s| s.parse().map_err(|_| LibraryError::Invalid(s.clone()))).collect()
        })
    }

    /// Whether the album counts as liked (see [`Self::like_release`]).
    pub fn release_liked(&self, release: ReleaseId) -> Result<bool> {
        self.read(|c| {
            Ok(c.query_row(
                concat!("SELECT ", release_liked_expr!(), " FROM release r WHERE r.id = ?1"),
                [release.to_string()],
                |r| r.get::<_, Option<bool>>(0),
            )
            .optional()?
            .flatten()
            .unwrap_or(false))
        })
    }

    /// `(release, cover_ref)` of every live track of a playlist that has an
    /// album cover, in playlist order (one item per track, for counting).
    pub fn playlist_cover_refs(&self, playlist: PlaylistId) -> Result<Vec<(ReleaseId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT r.id, r.cover_ref FROM playlist_entry e
                   JOIN track t ON t.id = e.track_id
                   JOIN release r ON r.id = t.release_id
                  WHERE e.playlist_id = ?1 AND e.deleted = 0 AND t.deleted_at IS NULL
                    AND r.cover_ref IS NOT NULL
                  ORDER BY e.position, e.id",
            )?;
            let rows = stmt.query_map([playlist.to_string()], |r| Ok((get_id(r, 0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Ids of all liked tracks, whether or not the catalog knows them yet.
    pub fn liked_track_ids(&self) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT target_id FROM likes WHERE target_kind = 'track' AND deleted = 0
                  ORDER BY updated_at DESC",
            )?;
            let ids = stmt.query_map([], |r| get_id(r, 0))?;
            Ok(ids.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}
