//! Artists, releases, tracks and sources.

use nikgob_core::model::{
    Artist, ArtistCredit, ArtistId, Codec, Release, ReleaseId, Source, SourceId, SourceOrigin,
    Track, TrackId,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::types::{
    ArtistRow, ArtistSort, ArtistSortKey, ReleaseProgress, ReleaseRow, ReleaseSort, ReleaseSortKey,
    TrackFilter, TrackRow, TrackSort, TrackSortKey,
};
use crate::util::{
    credit_columns, get_enum, get_id, get_json, get_opt_enum, get_opt_id, now_ms, opt_u32, sort_key,
};

// ---------------------------------------------------------------- row mapping

pub(crate) const TRACK_ROW_SELECT: &str =
    concat!("SELECT ", track_row_cols!(), " ", track_row_from!());

pub(crate) fn track_row(row: &Row<'_>) -> rusqlite::Result<TrackRow> {
    track_row_at(row, 0)
}

/// Maps the 15 columns of `track_row_cols!()` starting at column `base`.
pub(crate) fn track_row_at(row: &Row<'_>, base: usize) -> rusqlite::Result<TrackRow> {
    Ok(TrackRow {
        id: get_id(row, base)?,
        title: row.get(base + 1)?,
        artist: row.get(base + 2)?,
        album: row.get(base + 3)?,
        release_id: get_opt_id(row, base + 4)?,
        cover_ref: row.get(base + 5)?,
        disc_no: row.get::<_, i64>(base + 6)?.max(0) as u32,
        track_no: row.get::<_, i64>(base + 7)?.max(0) as u32,
        duration_ms: opt_u32(row, base + 8)?,
        quality: row.get::<_, Option<i64>>(base + 9)?.map(|q| q as i32),
        codec: row
            .get::<_, Option<String>>(base + 10)?
            .map(|c| Codec::parse(&c)),
        active_kind: get_opt_enum(row, base + 11)?,
        has_local: row.get(base + 12)?,
        liked: row.get(base + 13)?,
        artist_id: get_opt_id(row, base + 14)?,
    })
}

const ARTIST_COLS: &str = "id, mbid, name, sort_name, images, added_at";

fn artist_from_row(row: &Row<'_>) -> rusqlite::Result<Artist> {
    Ok(Artist {
        id: get_id(row, 0)?,
        mbid: row.get(1)?,
        name: row.get(2)?,
        sort_name: row.get(3)?,
        images: get_json(row, 4)?,
        added_at: row.get(5)?,
    })
}

const RELEASE_COLS: &str = "id, mbid, release_group_mbid, title, artist_credit, year, \
                            release_type, cover_ref, total_tracks, added_at";

fn release_from_row(row: &Row<'_>) -> rusqlite::Result<Release> {
    Ok(Release {
        id: get_id(row, 0)?,
        mbid: row.get(1)?,
        release_group_mbid: row.get(2)?,
        title: row.get(3)?,
        artist_credit: get_json(row, 4)?,
        year: row.get::<_, Option<i64>>(5)?.map(|y| y as i32),
        release_type: get_enum(row, 6)?,
        cover_ref: row.get(7)?,
        total_tracks: opt_u32(row, 8)?,
        added_at: row.get(9)?,
    })
}

const TRACK_COLS: &str = "id, recording_mbid, isrc, acoustid, title, artist_credit, release_id, \
                          disc_no, track_no, duration_ms, added_at";

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    Ok(Track {
        id: get_id(row, 0)?,
        recording_mbid: row.get(1)?,
        isrc: row.get(2)?,
        acoustid: row.get(3)?,
        title: row.get(4)?,
        artist_credit: get_json(row, 5)?,
        release_id: get_opt_id(row, 6)?,
        disc_no: row.get::<_, i64>(7)?.max(0) as u32,
        track_no: row.get::<_, i64>(8)?.max(0) as u32,
        duration_ms: opt_u32(row, 9)?,
        added_at: row.get(10)?,
    })
}

pub(crate) const SOURCE_COLS: &str = "id, track_id, kind, locator, codec, bitrate, sample_rate, \
    bit_depth, quality_score, is_active, file_hash, size_bytes, origin_kind, origin_info_hash, \
    origin_path, available, added_at, trashed_at";

pub(crate) fn source_from_row(row: &Row<'_>) -> rusqlite::Result<Source> {
    let origin_kind: String = row.get(12)?;
    let origin = match origin_kind.as_str() {
        "torrent" => SourceOrigin::Torrent {
            info_hash: row.get::<_, Option<String>>(13)?.unwrap_or_default(),
            path_in_torrent: row.get::<_, Option<String>>(14)?.unwrap_or_default(),
        },
        "stream" => SourceOrigin::Stream,
        _ => SourceOrigin::Inbox,
    };
    Ok(Source {
        id: get_id(row, 0)?,
        track_id: get_id(row, 1)?,
        kind: get_enum(row, 2)?,
        locator: row.get(3)?,
        codec: Codec::parse(&row.get::<_, String>(4)?),
        bitrate: opt_u32(row, 5)?,
        sample_rate: opt_u32(row, 6)?,
        bit_depth: row.get::<_, Option<i64>>(7)?.map(|b| b as u8),
        quality_score: row.get::<_, i64>(8)? as i32,
        is_active: row.get(9)?,
        file_hash: row.get(10)?,
        size_bytes: row.get::<_, Option<i64>>(11)?.map(|s| s as u64),
        origin,
        available: row.get(15)?,
        added_at: row.get(16)?,
        trashed_at: row.get(17)?,
    })
}

pub(crate) fn origin_columns(origin: &SourceOrigin) -> (&'static str, Option<&str>, Option<&str>) {
    match origin {
        SourceOrigin::Torrent {
            info_hash,
            path_in_torrent,
        } => ("torrent", Some(info_hash), Some(path_in_torrent)),
        SourceOrigin::Inbox => ("inbox", None, None),
        SourceOrigin::Stream => ("stream", None, None),
    }
}

// -------------------------------------------------- in-transaction primitives

fn artist_by_id(conn: &Connection, id: ArtistId) -> Result<Option<Artist>> {
    Ok(conn
        .query_row(
            &format!("SELECT {ARTIST_COLS} FROM artist WHERE id = ?1"),
            [id.to_string()],
            artist_from_row,
        )
        .optional()?)
}

fn release_by_id(conn: &Connection, id: ReleaseId) -> Result<Option<Release>> {
    Ok(conn
        .query_row(
            &format!("SELECT {RELEASE_COLS} FROM release WHERE id = ?1"),
            [id.to_string()],
            release_from_row,
        )
        .optional()?)
}

fn track_by_id(conn: &Connection, id: TrackId) -> Result<Option<Track>> {
    Ok(conn
        .query_row(
            &format!("SELECT {TRACK_COLS} FROM track WHERE id = ?1"),
            [id.to_string()],
            track_from_row,
        )
        .optional()?)
}

pub(crate) fn source_by_id(conn: &Connection, id: SourceId) -> Result<Option<Source>> {
    Ok(conn
        .query_row(
            &format!("SELECT {SOURCE_COLS} FROM source WHERE id = ?1"),
            [id.to_string()],
            source_from_row,
        )
        .optional()?)
}

fn first_string(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Option<String>> {
    Ok(conn.query_row(sql, params, |r| r.get(0)).optional()?)
}

fn parse_id<T: std::str::FromStr>(s: &str) -> Result<T> {
    s.parse().map_err(|_| LibraryError::Invalid(s.to_owned()))
}

fn write_credit_links(
    conn: &Connection,
    table: &str,
    owner_col: &str,
    owner: &str,
    credit: &ArtistCredit,
) -> Result<()> {
    conn.execute(
        &format!("DELETE FROM {table} WHERE {owner_col} = ?1"),
        [owner],
    )?;
    let mut insert = conn.prepare_cached(&format!(
        "INSERT OR IGNORE INTO {table}({owner_col}, artist_id, position) VALUES (?1, ?2, ?3)"
    ))?;
    for (pos, artist_id) in credit.artist_ids().enumerate() {
        insert.execute(params![owner, artist_id.to_string(), pos as i64])?;
    }
    Ok(())
}

/// Picks the best usable source of the track and records it as active, both on
/// the source rows and in the track's denormalised columns.
///
/// Usable = not trashed and `available`. Order: local file, then Spotify, then
/// YouTube; within a kind the higher `quality_score` wins, then the older one.
pub(crate) fn refresh_active(conn: &Connection, track: TrackId) -> Result<Option<SourceId>> {
    let tid = track.to_string();
    let best: Option<(String, String, String, i64)> = conn
        .query_row(
            "SELECT id, kind, codec, quality_score FROM source
              WHERE track_id = ?1 AND trashed_at IS NULL AND available = 1
              ORDER BY CASE kind WHEN 'local_file' THEN 0 WHEN 'spotify' THEN 1 ELSE 2 END,
                       quality_score DESC, added_at ASC, id ASC
              LIMIT 1",
            [&tid],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;

    // Deactivate first: the partial unique index allows one active row per track.
    conn.execute(
        "UPDATE source SET is_active = 0 WHERE track_id = ?1 AND is_active = 1 AND id IS NOT ?2",
        params![tid, best.as_ref().map(|b| &b.0)],
    )?;
    match &best {
        Some((id, kind, codec, quality)) => {
            conn.execute(
                "UPDATE source SET is_active = 1 WHERE id = ?1 AND is_active = 0",
                [id],
            )?;
            conn.execute(
                "UPDATE track SET active_kind = ?2, active_codec = ?3, active_quality = ?4,
                                  has_local = ?5 WHERE id = ?1",
                params![tid, kind, codec, quality, i64::from(kind == "local_file")],
            )?;
        }
        None => {
            conn.execute(
                "UPDATE track SET active_kind = NULL, active_codec = NULL, active_quality = NULL,
                                  has_local = 0 WHERE id = ?1",
                [&tid],
            )?;
        }
    }
    best.map(|b| parse_id(&b.0)).transpose()
}

// ------------------------------------------------------------------ public API

impl Library {
    // ---- upserts

    /// Inserts or updates an artist. Matches the row with the same `id`, else
    /// the one with the same `mbid`; returns the stored artist (whose `id` may
    /// differ from the argument when matched by `mbid`).
    pub fn upsert_artist(&self, artist: &Artist) -> Result<Artist> {
        self.write(|tx| {
            let mut found = first_string(
                tx,
                "SELECT id FROM artist WHERE id = ?1",
                [artist.id.to_string()],
            )?;
            if found.is_none()
                && let Some(mbid) = &artist.mbid
            {
                found = first_string(tx, "SELECT id FROM artist WHERE mbid = ?1", [mbid])?;
            }
            let sort_name = if artist.sort_name.trim().is_empty() {
                &artist.name
            } else {
                &artist.sort_name
            };
            let key = sort_key(sort_name);
            let images = serde_json::to_string(&artist.images)?;
            let id = match found {
                Some(id) => {
                    tx.execute(
                        "UPDATE artist SET mbid = coalesce(?2, mbid), name = ?3, sort_name = ?4,
                                sort_key = ?5,
                                images = CASE WHEN ?6 = '[]' THEN images ELSE ?6 END
                          WHERE id = ?1",
                        params![id, artist.mbid, artist.name, sort_name, key, images],
                    )?;
                    id
                }
                None => {
                    let added = if artist.added_at == 0 {
                        now_ms()
                    } else {
                        artist.added_at
                    };
                    let id = artist.id.to_string();
                    tx.execute(
                        "INSERT INTO artist(id, mbid, name, sort_name, sort_key, images, added_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![id, artist.mbid, artist.name, sort_name, key, images, added],
                    )?;
                    id
                }
            };
            let id: ArtistId = parse_id(&id)?;
            artist_by_id(tx, id)?.ok_or_else(|| LibraryError::not_found("artist", id))
        })
    }

    /// Inserts or updates a release (match: `id`, then `mbid`). Optional
    /// identifiers (`mbid`, `release_group_mbid`, `cover_ref`, `year`,
    /// `total_tracks`) are kept when the new value is `None`.
    pub fn upsert_release(&self, release: &Release) -> Result<Release> {
        self.write(|tx| {
            let mut found = first_string(
                tx,
                "SELECT id FROM release WHERE id = ?1",
                [release.id.to_string()],
            )?;
            if found.is_none()
                && let Some(mbid) = &release.mbid
            {
                found = first_string(tx, "SELECT id FROM release WHERE mbid = ?1", [mbid])?;
            }
            let (credit_json, display, artist_key) = credit_columns(&release.artist_credit)?;
            let title_key = sort_key(&release.title);
            let kind = release.release_type.as_str();
            let id = match found {
                Some(id) => {
                    tx.execute(
                        "UPDATE release SET mbid = coalesce(?2, mbid),
                                release_group_mbid = coalesce(?3, release_group_mbid),
                                title = ?4, title_key = ?5, artist_credit = ?6,
                                artist_display = ?7, artist_key = ?8,
                                year = coalesce(?9, year), release_type = ?10,
                                cover_ref = coalesce(?11, cover_ref),
                                total_tracks = coalesce(?12, total_tracks)
                          WHERE id = ?1",
                        params![
                            id,
                            release.mbid,
                            release.release_group_mbid,
                            release.title,
                            title_key,
                            credit_json,
                            display,
                            artist_key,
                            release.year,
                            kind,
                            release.cover_ref,
                            release.total_tracks
                        ],
                    )?;
                    id
                }
                None => {
                    let added = if release.added_at == 0 {
                        now_ms()
                    } else {
                        release.added_at
                    };
                    let id = release.id.to_string();
                    tx.execute(
                        "INSERT INTO release(id, mbid, release_group_mbid, title, title_key,
                                artist_credit, artist_display, artist_key, year, release_type,
                                cover_ref, total_tracks, added_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                        params![
                            id,
                            release.mbid,
                            release.release_group_mbid,
                            release.title,
                            title_key,
                            credit_json,
                            display,
                            artist_key,
                            release.year,
                            kind,
                            release.cover_ref,
                            release.total_tracks,
                            added
                        ],
                    )?;
                    id
                }
            };
            write_credit_links(
                tx,
                "release_artist",
                "release_id",
                &id,
                &release.artist_credit,
            )?;
            tx.execute(
                "UPDATE track SET album_key = ?2 WHERE release_id = ?1 AND album_key != ?2",
                params![id, title_key],
            )?;
            let id: ReleaseId = parse_id(&id)?;
            release_by_id(tx, id)?.ok_or_else(|| LibraryError::not_found("release", id))
        })
    }

    /// Moves every track of `from` onto `to` and removes `from` (its palette,
    /// artist links and, when `to` has none, its like move over). Fields `to` has
    /// no value for (cover, year, total, MBIDs) are taken from `from`. Two
    /// different MBIDs are two editions: that is an error and nothing changes.
    /// Returns how many tracks moved.
    pub fn merge_releases(&self, from: ReleaseId, to: ReleaseId) -> Result<u32> {
        if from == to {
            return Ok(0);
        }
        self.write(|tx| {
            let (f, t) = (from.to_string(), to.to_string());
            for (id, rid) in [(&f, from), (&t, to)] {
                first_string(tx, "SELECT id FROM release WHERE id = ?1", [id])?
                    .ok_or_else(|| LibraryError::not_found("release", rid))?;
            }
            for column in ["mbid", "release_group_mbid"] {
                let sql = format!("SELECT {column} FROM release WHERE id = ?1");
                let ids: Vec<Option<String>> = [&f, &t]
                    .iter()
                    .map(|id| tx.query_row(&sql, [id], |r| r.get(0)))
                    .collect::<rusqlite::Result<_>>()?;
                if let [Some(a), Some(b)] = &ids[..]
                    && a != b
                {
                    return Err(LibraryError::Invalid(format!("releases {f} and {t} are different editions")));
                }
            }
            let mbid: Option<String> =
                tx.query_row("SELECT mbid FROM release WHERE id = ?1", [&f], |r| r.get(0))?;
            tx.execute(
                "UPDATE release SET
                        cover_ref = coalesce(cover_ref, (SELECT cover_ref FROM release WHERE id = ?1)),
                        year = coalesce(year, (SELECT year FROM release WHERE id = ?1)),
                        total_tracks = coalesce(total_tracks, (SELECT total_tracks FROM release WHERE id = ?1)),
                        release_group_mbid = coalesce(release_group_mbid,
                                (SELECT release_group_mbid FROM release WHERE id = ?1))
                  WHERE id = ?2",
                params![f, t],
            )?;
            let moved = tx.execute(
                "UPDATE track SET release_id = ?2,
                        album_key = (SELECT title_key FROM release WHERE id = ?2)
                  WHERE release_id = ?1",
                params![f, t],
            )?;
            tx.execute(
                "UPDATE likes SET target_id = ?2, updated_at = ?3
                  WHERE target_kind = 'release' AND target_id = ?1
                    AND NOT EXISTS (SELECT 1 FROM likes
                                     WHERE target_kind = 'release' AND target_id = ?2)",
                params![f, t, now_ms()],
            )?;
            tx.execute("DELETE FROM release_artist WHERE release_id = ?1", [&f])?;
            tx.execute("DELETE FROM palette WHERE release_id = ?1", [&f])?;
            tx.execute("DELETE FROM release WHERE id = ?1", [&f])?;
            // After the delete: an MBID may be held by one row only.
            tx.execute(
                "UPDATE release SET mbid = coalesce(mbid, ?2) WHERE id = ?1",
                params![t, mbid],
            )?;
            Ok(moved as u32)
        })
    }

    /// Inserts or updates a track.
    ///
    /// Matching: the row with the same `id` (so the import crate can pass the
    /// id read from file tags), else the row with the same `recording_mbid` *on
    /// the same release* (the same recording on a compilation is a different
    /// track). Optional identifiers and `duration_ms` are kept when the new
    /// value is `None`. The referenced release must already exist.
    pub fn upsert_track(&self, track: &Track) -> Result<Track> {
        self.write(|tx| {
            let release_id = track.release_id.map(|r| r.to_string());
            let mut found = first_string(
                tx,
                "SELECT id FROM track WHERE id = ?1",
                [track.id.to_string()],
            )?;
            if found.is_none()
                && let Some(mbid) = &track.recording_mbid
            {
                found = first_string(
                    tx,
                    "SELECT id FROM track WHERE recording_mbid = ?1 AND release_id IS ?2",
                    params![mbid, release_id],
                )?;
            }
            let (credit_json, display, artist_key) = credit_columns(&track.artist_credit)?;
            let title_key = sort_key(&track.title);
            let album_key: String = match &release_id {
                Some(r) => first_string(tx, "SELECT title_key FROM release WHERE id = ?1", [r])?
                    .ok_or_else(|| LibraryError::not_found("release", r))?,
                None => String::new(),
            };
            let id = match found {
                Some(id) => {
                    tx.execute(
                        "UPDATE track SET recording_mbid = coalesce(?2, recording_mbid),
                                isrc = coalesce(?3, isrc), acoustid = coalesce(?4, acoustid),
                                title = ?5, title_key = ?6, artist_credit = ?7,
                                artist_display = ?8, artist_key = ?9, release_id = ?10,
                                album_key = ?11, disc_no = ?12, track_no = ?13,
                                duration_ms = coalesce(?14, duration_ms)
                          WHERE id = ?1",
                        params![
                            id,
                            track.recording_mbid,
                            track.isrc,
                            track.acoustid,
                            track.title,
                            title_key,
                            credit_json,
                            display,
                            artist_key,
                            release_id,
                            album_key,
                            track.disc_no,
                            track.track_no,
                            track.duration_ms
                        ],
                    )?;
                    id
                }
                None => {
                    let added = if track.added_at == 0 {
                        now_ms()
                    } else {
                        track.added_at
                    };
                    let id = track.id.to_string();
                    tx.execute(
                        "INSERT INTO track(id, recording_mbid, isrc, acoustid, title, title_key,
                                artist_credit, artist_display, artist_key, release_id, album_key,
                                disc_no, track_no, duration_ms, added_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                        params![
                            id,
                            track.recording_mbid,
                            track.isrc,
                            track.acoustid,
                            track.title,
                            title_key,
                            credit_json,
                            display,
                            artist_key,
                            release_id,
                            album_key,
                            track.disc_no,
                            track.track_no,
                            track.duration_ms,
                            added
                        ],
                    )?;
                    id
                }
            };
            write_credit_links(tx, "track_artist", "track_id", &id, &track.artist_credit)?;
            let id: TrackId = parse_id(&id)?;
            track_by_id(tx, id)?.ok_or_else(|| LibraryError::not_found("track", id))
        })
    }

    // ---- sources

    /// Adds a source, or updates the existing one with the same `id` or the
    /// same `(kind, locator)` among non-trashed sources (so re-importing a
    /// file is idempotent). Re-selects the track's active source and returns
    /// the stored row.
    pub fn add_source(&self, source: &Source) -> Result<Source> {
        self.write(|tx| {
            let track = source.track_id.to_string();
            let kind = source.kind.as_str();
            let mut found = first_string(
                tx,
                "SELECT id FROM source WHERE id = ?1",
                [source.id.to_string()],
            )?;
            if found.is_none() && source.trashed_at.is_none() {
                found = first_string(
                    tx,
                    "SELECT id FROM source
                      WHERE kind = ?1 AND locator = ?2 AND trashed_at IS NULL",
                    params![kind, source.locator],
                )?;
            }
            let (origin_kind, info_hash, origin_path) = origin_columns(&source.origin);
            let size = source.size_bytes.map(|s| s as i64);
            let mut moved_from: Option<String> = None;
            let id = match found {
                Some(id) => {
                    moved_from = first_string(
                        tx,
                        "SELECT track_id FROM source WHERE id = ?1 AND track_id != ?2",
                        params![id, track],
                    )?;
                    tx.execute(
                        "UPDATE source SET track_id = ?2, kind = ?3, locator = ?4, codec = ?5,
                                is_active = CASE WHEN track_id = ?2 THEN is_active ELSE 0 END,
                                bitrate = ?6, sample_rate = ?7, bit_depth = ?8, quality_score = ?9,
                                file_hash = coalesce(?10, file_hash),
                                size_bytes = coalesce(?11, size_bytes),
                                origin_kind = ?12, origin_info_hash = ?13, origin_path = ?14,
                                available = ?15
                          WHERE id = ?1",
                        params![
                            id,
                            track,
                            kind,
                            source.locator,
                            source.codec.as_str(),
                            source.bitrate,
                            source.sample_rate,
                            source.bit_depth,
                            source.quality_score,
                            source.file_hash,
                            size,
                            origin_kind,
                            info_hash,
                            origin_path,
                            source.available
                        ],
                    )?;
                    id
                }
                None => {
                    let added = if source.added_at == 0 {
                        now_ms()
                    } else {
                        source.added_at
                    };
                    let id = source.id.to_string();
                    tx.execute(
                        "INSERT INTO source(id, track_id, kind, locator, codec, bitrate,
                                sample_rate, bit_depth, quality_score, is_active, file_hash,
                                size_bytes, origin_kind, origin_info_hash, origin_path, available,
                                added_at, trashed_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11, ?12, ?13, ?14,
                                 ?15, ?16, ?17)",
                        params![
                            id,
                            track,
                            kind,
                            source.locator,
                            source.codec.as_str(),
                            source.bitrate,
                            source.sample_rate,
                            source.bit_depth,
                            source.quality_score,
                            source.file_hash,
                            size,
                            origin_kind,
                            info_hash,
                            origin_path,
                            source.available,
                            added,
                            source.trashed_at
                        ],
                    )?;
                    id
                }
            };
            // A source handed over to another track must not stay "active" on
            // the track it left.
            if let Some(old) = moved_from {
                refresh_active(tx, parse_id(&old)?)?;
            }
            refresh_active(tx, source.track_id)?;
            let sid: SourceId = parse_id(&id)?;
            source_by_id(tx, sid)?.ok_or_else(|| LibraryError::not_found("source", sid))
        })
    }

    /// Re-selects the active source of a track: local file > Spotify > YouTube,
    /// then the highest `quality_score`, among non-trashed available sources.
    /// Returns the new active source, `None` if the track has no usable one.
    pub fn set_active_source(&self, track: TrackId) -> Result<Option<SourceId>> {
        self.write(|tx| refresh_active(tx, track))
    }

    /// Marks a source (un)playable (stream withdrawn, file missing) and
    /// re-selects the active source.
    pub fn set_source_available(&self, source: SourceId, available: bool) -> Result<()> {
        self.write(|tx| {
            let src = source_by_id(tx, source)?
                .ok_or_else(|| LibraryError::not_found("source", source))?;
            tx.execute(
                "UPDATE source SET available = ?2 WHERE id = ?1",
                params![source.to_string(), available],
            )?;
            refresh_active(tx, src.track_id)?;
            Ok(())
        })
    }

    pub fn get_source(&self, id: SourceId) -> Result<Option<Source>> {
        self.read(|c| source_by_id(c, id))
    }

    /// Non-trashed sources of a track, the active one first.
    pub fn sources_of_track(&self, track: TrackId) -> Result<Vec<Source>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SOURCE_COLS} FROM source WHERE track_id = ?1 AND trashed_at IS NULL
                  ORDER BY is_active DESC, quality_score DESC, added_at, id"
            ))?;
            let rows = stmt.query_map([track.to_string()], source_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn active_source(&self, track: TrackId) -> Result<Option<Source>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {SOURCE_COLS} FROM source WHERE track_id = ?1 AND is_active = 1"),
                [track.to_string()],
                source_from_row,
            )
            .optional()?)
        })
    }

    /// Non-trashed sources with this content hash (duplicate detection).
    pub fn sources_by_hash(&self, file_hash: &str) -> Result<Vec<Source>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SOURCE_COLS} FROM source WHERE file_hash = ?1 AND trashed_at IS NULL"
            ))?;
            let rows = stmt.query_map([file_hash], source_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    // ---- point lookups

    pub fn get_track(&self, id: TrackId) -> Result<Option<Track>> {
        self.read(|c| track_by_id(c, id))
    }

    pub fn get_track_row(&self, id: TrackId) -> Result<Option<TrackRow>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("{TRACK_ROW_SELECT} WHERE t.id = ?1"),
                [id.to_string()],
                track_row,
            )
            .optional()?)
        })
    }

    pub fn get_release(&self, id: ReleaseId) -> Result<Option<Release>> {
        self.read(|c| release_by_id(c, id))
    }

    pub fn get_artist(&self, id: ArtistId) -> Result<Option<Artist>> {
        self.read(|c| artist_by_id(c, id))
    }

    /// Tracks sharing a MusicBrainz recording id (one per release it appears on).
    pub fn tracks_by_recording_mbid(&self, mbid: &str) -> Result<Vec<Track>> {
        self.tracks_where("recording_mbid", mbid)
    }

    pub fn tracks_by_acoustid(&self, acoustid: &str) -> Result<Vec<Track>> {
        self.tracks_where("acoustid", acoustid)
    }

    pub fn tracks_by_isrc(&self, isrc: &str) -> Result<Vec<Track>> {
        self.tracks_where("isrc", isrc)
    }

    fn tracks_where(&self, column: &'static str, value: &str) -> Result<Vec<Track>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {TRACK_COLS} FROM track WHERE {column} = ?1 ORDER BY added_at, id"
            ))?;
            let rows = stmt.query_map([value], track_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    // ---- listings

    /// Tracks of a release in album order (disc, track number).
    pub fn tracks_of_release(&self, release: ReleaseId) -> Result<Vec<TrackRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "{TRACK_ROW_SELECT} WHERE t.release_id = ?1 ORDER BY t.disc_no, t.track_no, t.seq"
            ))?;
            let rows = stmt.query_map([release.to_string()], track_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Tracks credited to an artist, page by page, ordered by album then track.
    pub fn tracks_of_artist(
        &self,
        artist: ArtistId,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<TrackRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "{TRACK_ROW_SELECT}
                  WHERE t.id IN (SELECT track_id FROM track_artist WHERE artist_id = ?1)
                  ORDER BY t.album_key, t.disc_no, t.track_no, t.seq LIMIT ?2 OFFSET ?3"
            ))?;
            let rows = stmt.query_map(params![artist.to_string(), limit, offset], track_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// One page of the track list (for a virtualised list).
    pub fn tracks(&self, offset: u32, limit: u32, sort: TrackSort) -> Result<Vec<TrackRow>> {
        self.tracks_filtered(offset, limit, sort, TrackFilter::All)
    }

    pub fn tracks_filtered(
        &self,
        offset: u32,
        limit: u32,
        sort: TrackSort,
        filter: TrackFilter,
    ) -> Result<Vec<TrackRow>> {
        let d = if sort.desc { "DESC" } else { "ASC" };
        let order = match sort.key {
            TrackSortKey::Title => format!("t.title_key {d}, t.seq {d}"),
            TrackSortKey::Artist => format!(
                "t.artist_key {d}, t.album_key {d}, t.disc_no {d}, t.track_no {d}, t.seq {d}"
            ),
            TrackSortKey::Album => {
                format!("t.album_key {d}, t.disc_no {d}, t.track_no {d}, t.seq {d}")
            }
            TrackSortKey::Added => format!("t.added_at {d}, t.seq {d}"),
            TrackSortKey::Duration => format!("t.duration_ms {d}, t.seq {d}"),
            TrackSortKey::Quality => format!("t.active_quality {d}, t.seq {d}"),
        };
        let where_ = match filter {
            TrackFilter::All => "",
            TrackFilter::LocalOnly => "WHERE t.has_local = 1",
            TrackFilter::NoLocalFile => "WHERE t.has_local = 0",
        };
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "{TRACK_ROW_SELECT} {where_} ORDER BY {order} LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit, offset], track_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn count_tracks(&self, filter: TrackFilter) -> Result<u32> {
        let where_ = match filter {
            TrackFilter::All => "",
            TrackFilter::LocalOnly => "WHERE has_local = 1",
            TrackFilter::NoLocalFile => "WHERE has_local = 0",
        };
        self.read(|c| {
            Ok(
                c.query_row(&format!("SELECT COUNT(*) FROM track {where_}"), [], |r| {
                    r.get(0)
                })?,
            )
        })
    }

    pub fn count_releases(&self) -> Result<u32> {
        self.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM release", [], |r| r.get(0))?))
    }

    pub fn count_artists(&self) -> Result<u32> {
        self.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM artist", [], |r| r.get(0))?))
    }

    /// One page of releases with their "N of total locally" counters.
    pub fn releases(&self, offset: u32, limit: u32, sort: ReleaseSort) -> Result<Vec<ReleaseRow>> {
        let d = if sort.desc { "DESC" } else { "ASC" };
        // Unknown years always sort last, whatever the direction.
        let order = match sort.key {
            ReleaseSortKey::Artist => {
                format!("r.artist_key {d}, r.year IS NULL, r.year {d}, r.title_key {d}, r.seq {d}")
            }
            ReleaseSortKey::Year => {
                format!("r.year IS NULL, r.year {d}, r.artist_key {d}, r.title_key {d}, r.seq {d}")
            }
            ReleaseSortKey::Title => format!("r.title_key {d}, r.seq {d}"),
            ReleaseSortKey::Added => format!("r.added_at {d}, r.seq {d}"),
        };
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "{RELEASE_ROW_SELECT} ORDER BY {order} LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit, offset], release_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Releases credited to an artist, newest first.
    pub fn releases_of_artist(&self, artist: ArtistId) -> Result<Vec<ReleaseRow>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "{RELEASE_ROW_SELECT}
                  WHERE r.id IN (SELECT release_id FROM release_artist WHERE artist_id = ?1)
                  ORDER BY r.year IS NULL, r.year DESC, r.title_key, r.seq"
            ))?;
            let rows = stmt.query_map([artist.to_string()], release_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// "3 из 12 у тебя" for one release.
    pub fn release_progress(&self, release: ReleaseId) -> Result<Option<ReleaseProgress>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("{RELEASE_ROW_SELECT} WHERE r.id = ?1"),
                [release.to_string()],
                release_row,
            )
            .optional()?
            .map(|row| row.progress()))
        })
    }

    pub fn artists(&self, offset: u32, limit: u32, sort: ArtistSort) -> Result<Vec<ArtistRow>> {
        let d = if sort.desc { "DESC" } else { "ASC" };
        let order = match sort.key {
            ArtistSortKey::Name => format!("a.sort_key {d}, a.seq {d}"),
            ArtistSortKey::Added => format!("a.added_at {d}, a.seq {d}"),
        };
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "{ARTIST_ROW_SELECT} ORDER BY {order} LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit, offset], artist_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

pub(crate) const RELEASE_ROW_SELECT: &str = "\
SELECT r.id, r.mbid, r.release_group_mbid, r.title, r.artist_credit, r.year, r.release_type,
       r.cover_ref, r.total_tracks, r.added_at,
       (SELECT COUNT(*) FROM track t WHERE t.release_id = r.id AND t.has_local = 1),
       (SELECT COUNT(*) FROM track t WHERE t.release_id = r.id)
  FROM release r";

pub(crate) fn release_row(row: &Row<'_>) -> rusqlite::Result<ReleaseRow> {
    Ok(ReleaseRow {
        release: release_from_row(row)?,
        local_tracks: row.get::<_, i64>(10)? as u32,
        known_tracks: row.get::<_, i64>(11)? as u32,
    })
}

pub(crate) const ARTIST_ROW_SELECT: &str = "\
SELECT a.id, a.mbid, a.name, a.sort_name, a.images, a.added_at,
       (SELECT COUNT(*) FROM release_artist ra WHERE ra.artist_id = a.id),
       (SELECT COUNT(*) FROM track_artist ta JOIN track t ON t.id = ta.track_id
         WHERE ta.artist_id = a.id AND t.has_local = 1)
  FROM artist a";

pub(crate) fn artist_row(row: &Row<'_>) -> rusqlite::Result<ArtistRow> {
    Ok(ArtistRow {
        artist: artist_from_row(row)?,
        release_count: row.get::<_, i64>(6)? as u32,
        local_track_count: row.get::<_, i64>(7)? as u32,
    })
}
