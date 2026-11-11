//! Artists, releases, tracks and sources.

use nikgob_core::model::{
    Artist, ArtistCredit, ArtistId, Codec, Release, ReleaseId, Source, SourceId, SourceOrigin,
    Track, TrackId, UnixMs,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::types::{
    ArtistRow, ArtistSort, ArtistSortKey, ReleaseListing, ReleaseProgress, ReleaseRow, ReleaseSort, ReleaseSortKey,
    TrackFilter, TrackRow, TrackSort, TrackSortKey,
};
use crate::streams::refresh_in_library;
use crate::util::{
    credit_columns, get_enum, get_id, get_json, get_opt_enum, get_opt_id, now_ms, opt_u32, sort_key,
};

// ---------------------------------------------------------------- row mapping

pub(crate) const TRACK_ROW_SELECT: &str =
    concat!("SELECT ", track_row_cols!(), " ", track_row_from!());

pub(crate) fn track_row(row: &Row<'_>) -> rusqlite::Result<TrackRow> {
    track_row_at(row, 0)
}

/// Maps the 17 columns of `track_row_cols!()` starting at column `base`.
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
        credit: get_json(row, base + 14)?,
        youtube_id: row.get(base + 15)?,
        check: row
            .get::<_, Option<String>>(base + 16)?
            .and_then(|c| nikgob_core::quality::CheckSummary::decode(&c)),
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

pub(crate) const TRACK_COLS: &str = "id, recording_mbid, isrc, acoustid, title, artist_credit, release_id, \
                          disc_no, track_no, duration_ms, added_at";

pub(crate) fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
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
        .prepare_cached(&format!("SELECT {ARTIST_COLS} FROM artist WHERE id = ?1"))?
        .query_row([id.to_string()], artist_from_row)
        .optional()?)
}

fn release_by_id(conn: &Connection, id: ReleaseId) -> Result<Option<Release>> {
    Ok(conn
        .prepare_cached(&format!("SELECT {RELEASE_COLS} FROM release WHERE id = ?1"))?
        .query_row([id.to_string()], release_from_row)
        .optional()?)
}

fn track_by_id(conn: &Connection, id: TrackId) -> Result<Option<Track>> {
    Ok(conn
        .prepare_cached(&format!("SELECT {TRACK_COLS} FROM track WHERE id = ?1"))?
        .query_row([id.to_string()], track_from_row)
        .optional()?)
}

pub(crate) fn source_by_id(conn: &Connection, id: SourceId) -> Result<Option<Source>> {
    Ok(conn
        .prepare_cached(&format!("SELECT {SOURCE_COLS} FROM source WHERE id = ?1"))?
        .query_row([id.to_string()], source_from_row)
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

pub(crate) fn write_credit_links(
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
/// Usable = not trashed and `available`. Order: [`nikgob_core::model::SourceKind::priority`]
/// (local file, Spotify, YouTube, SoundCloud, Deezer); within a kind the
/// higher `quality_score` wins, then the older one.
pub(crate) fn refresh_active(conn: &Connection, track: TrackId) -> Result<Option<SourceId>> {
    let tid = track.to_string();
    let best: Option<(String, String, String, i64)> = conn
        .query_row(
            "SELECT id, kind, codec, quality_score FROM source
              WHERE track_id = ?1 AND trashed_at IS NULL AND available = 1
              ORDER BY CASE kind WHEN 'local_file' THEN 0 WHEN 'spotify' THEN 1 WHEN 'youtube' THEN 2
                                 WHEN 'soundcloud' THEN 3 WHEN 'deezer' THEN 4 ELSE 5 END,
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

/// [`Library::upsert_artist`] inside an open write transaction.
pub(crate) fn upsert_artist_in(tx: &Connection, artist: &Artist) -> Result<Artist> {
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
}

/// [`Library::upsert_release`] inside an open write transaction.
pub(crate) fn upsert_release_in(tx: &Connection, release: &Release) -> Result<Release> {
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
}

/// [`Library::upsert_track`] inside an open write transaction.
pub(crate) fn upsert_track_in(tx: &Connection, track: &Track) -> Result<Track> {
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
}

/// [`Library::add_source`] inside an open write transaction.
pub(crate) fn add_source_in(tx: &Connection, source: &Source) -> Result<Source> {
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
    // A live file brings a deleted track back (the same song imported
    // again lands on its old id), and its song is wanted again.
    if source.trashed_at.is_none() && source.kind.is_local() {
        let back = tx.execute(
            "UPDATE track SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
            [&track],
        )?;
        if back > 0 {
            crate::unwanted::forget_unwanted_in(tx, &track)?;
        }
    }
    // A source handed over to another track must not stay "active" on
    // the track it left.
    if let Some(old) = moved_from {
        refresh_active(tx, parse_id(&old)?)?;
        refresh_in_library(tx, &old)?;
    }
    refresh_active(tx, source.track_id)?;
    refresh_in_library(tx, &track)?;
    // A file: its «Хочу скачать» items that did not start are done.
    if source.trashed_at.is_none() && source.kind.is_local() {
        crate::masters::settle_wishlist_in(tx, &track)?;
    }
    let sid: SourceId = parse_id(&id)?;
    source_by_id(tx, sid)?.ok_or_else(|| LibraryError::not_found("source", sid))
}

/// See [`Library::merge_track_into`]. Wishlist items of the track: one
/// per track stays (the one furthest along, else the oldest).
pub(crate) fn merge_track_in(tx: &Connection, from: TrackId, into: TrackId, device: &str, now: UnixMs) -> Result<bool> {
    if from == into {
        return Ok(false);
    }
    let (f, t) = (from.to_string(), into.to_string());
    for id in [&f, &t] {
        if first_string(tx, "SELECT id FROM track WHERE id = ?1", [id])?.is_none() {
            return Ok(false);
        }
    }
    tx.execute("UPDATE source SET track_id = ?2, is_active = 0 WHERE track_id = ?1", params![f, t])?;
    crate::recordings::carry_recording_in(tx, &f, &t)?;
    tx.execute(
        "UPDATE playlist_entry SET track_id = ?2, updated_at = ?3, updated_by = ?4
          WHERE track_id = ?1 AND deleted = 0",
        params![f, t, now, device],
    )?;
    let liked: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM likes WHERE target_kind = 'track' AND target_id = ?1 AND deleted = 0)",
        [&f],
        |r| r.get(0),
    )?;
    if liked {
        tx.execute(
            "INSERT INTO likes(target_kind, target_id, updated_at, updated_by, deleted)
             VALUES ('track', ?1, ?2, ?3, 0)
             ON CONFLICT(target_kind, target_id) DO UPDATE SET
                deleted = 0, updated_at = excluded.updated_at, updated_by = excluded.updated_by",
            params![t, now, device],
        )?;
        tx.execute(
            "UPDATE likes SET deleted = 1, updated_at = ?2, updated_by = ?3
              WHERE target_kind = 'track' AND target_id = ?1",
            params![f, now, device],
        )?;
    }
    tx.execute("UPDATE event SET track_id = ?2 WHERE track_id = ?1", params![f, t])?;
    crate::events::edited();
    tx.execute(
        "INSERT OR IGNORE INTO track_alias(track_id, title, key, source)
         SELECT ?2, title, key, source FROM track_alias WHERE track_id = ?1
            AND key != (SELECT title_key FROM track WHERE id = ?2)",
        params![f, t],
    )?;
    // Ids in the services and their likes follow; a "seen" marker would
    // hide `into` from the library, so it goes.
    tx.execute("DELETE FROM external_id WHERE kind = 'track' AND local_id = ?1 AND service = 'seen'", [&f])?;
    tx.execute("UPDATE external_id SET local_id = ?2 WHERE kind = 'track' AND local_id = ?1", params![f, t])?;
    tx.execute("UPDATE remote_like SET track_id = ?2 WHERE track_id = ?1", params![f, t])?;
    tx.execute("UPDATE wishlist SET track_id = ?2 WHERE track_id = ?1", params![f, t])?;
    dedupe_wishlist_of_track(tx, &t)?;
    // Its place among the user's songs of the album `into` is on.
    crate::selection::carry_track_choices_in(tx, &f, &t)?;
    // What is not taken for the song again follows it.
    tx.execute("UPDATE OR IGNORE wrong_audio SET track_id = ?2 WHERE track_id = ?1", params![f, t])?;
    tx.execute("DELETE FROM wrong_audio WHERE track_id = ?1", [&f])?;
    // The item of a stream twin merged into the track with the file.
    crate::masters::settle_wishlist_in(tx, &t)?;
    // What it leaves behind: its artists and its album, when nothing else
    // has them (the album's ids go to the album `into` is on, when it is
    // the same one).
    let artists: Vec<String> = {
        let mut stmt = tx.prepare_cached("SELECT artist_id FROM track_artist WHERE track_id = ?1")?;
        let rows = stmt.query_map([&f], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let (release, into_release): (Option<String>, Option<String>) = (
        first_string(tx, "SELECT release_id FROM track WHERE id = ?1 AND release_id IS NOT NULL", [&f])?,
        first_string(tx, "SELECT release_id FROM track WHERE id = ?1 AND release_id IS NOT NULL", [&t])?,
    );
    for table in ["track_artist", "lyrics", "lyrics_text", "lyrics_pick", "analysis", "analysis_failed", "master"] {
        tx.execute(&format!("DELETE FROM {table} WHERE track_id = ?1"), [&f])?;
    }
    tx.execute("DELETE FROM track WHERE id = ?1", [&f])?;
    refresh_active(tx, into)?;
    refresh_in_library(tx, &t)?;
    if let Some(r) = release.filter(|r| into_release.as_ref() != Some(r)) {
        crate::repairs::drop_empty_release_in(tx, &r, into_release.as_deref(), device, now)?;
    }
    for a in artists {
        crate::repairs::drop_orphan_artist_in(tx, &a)?;
    }
    Ok(true)
}

/// What points at the release `from` that goes into `to`: its ids in the
/// services and its «Хочу скачать» items follow, its tracklist mark goes.
fn carry_release_links(tx: &Connection, from: &str, to: &str) -> Result<()> {
    tx.execute("UPDATE external_id SET local_id = ?2 WHERE kind = 'release' AND local_id = ?1", params![from, to])?;
    tx.execute("UPDATE wishlist SET release_id = ?2 WHERE release_id = ?1", params![from, to])?;
    tx.execute("DELETE FROM meta WHERE key = ?1", [format!("cache.tracklist.{from}")])?;
    Ok(())
}

/// Keeps one track-scope wishlist item of a track: the one furthest along
/// (downloading, searching, waiting for a confirm, done, dismissed for good),
/// else the oldest; downloads of the others point at it.
fn dedupe_wishlist_of_track(tx: &Connection, track: &str) -> Result<()> {
    let keep = first_string(
        tx,
        "SELECT id FROM wishlist WHERE track_id = ?1 AND scope = 'track'
          ORDER BY CASE status WHEN 'downloading' THEN 0 WHEN 'searching' THEN 1
                               WHEN 'confirm' THEN 2 WHEN 'done' THEN 3 WHEN 'dismissed' THEN 4
                               WHEN 'no_torrent' THEN 5 WHEN 'new' THEN 6 WHEN 'waiting' THEN 7
                               ELSE 8 END, created_at, id
          LIMIT 1",
        [track],
    )?;
    if let Some(keep) = keep {
        tx.execute(
            "UPDATE download SET wishlist_id = ?2
              WHERE wishlist_id IN (SELECT id FROM wishlist WHERE track_id = ?1 AND scope = 'track' AND id != ?2)",
            params![track, keep],
        )?;
        tx.execute(
            "DELETE FROM wishlist WHERE track_id = ?1 AND scope = 'track' AND id != ?2",
            params![track, keep],
        )?;
    }
    Ok(())
}

// ------------------------------------------------------------------ public API

impl Library {
    // ---- upserts

    /// Inserts or updates an artist. Matches the row with the same `id`, else
    /// the one with the same `mbid`; returns the stored artist (whose `id` may
    /// differ from the argument when matched by `mbid`).
    pub fn upsert_artist(&self, artist: &Artist) -> Result<Artist> {
        self.write(|tx| upsert_artist_in(tx, artist))
    }

    /// Inserts or updates a release (match: `id`, then `mbid`). Optional
    /// identifiers (`mbid`, `release_group_mbid`, `cover_ref`, `year`,
    /// `total_tracks`) are kept when the new value is `None`.
    pub fn upsert_release(&self, release: &Release) -> Result<Release> {
        self.write(|tx| upsert_release_in(tx, release))
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
        let device = self.device_id();
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
            crate::selection::carry_release_choices_in(tx, &f, &t, false, now_ms(), device.as_str())?;
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
            carry_release_links(tx, &f, &t)?;
            crate::streams::refresh_in_library_of_release(tx, &t)?;
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

    /// Folds `from` into `into` (one recording imported twice): its sources
    /// become more sources of `into` (the best one plays), its playlist
    /// entries, like, listening history, ids in the services and wishlist
    /// items move over, and the `from` row goes. No file is touched. Returns
    /// false when either track is missing.
    pub fn merge_track_into(&self, from: TrackId, into: TrackId) -> Result<bool> {
        if from == into {
            return Ok(false);
        }
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| merge_track_in(tx, from, into, device.as_str(), now))
    }

    /// Moves the tracks of `from`, another edition of the same album, onto
    /// `to` and removes `from`; `to` keeps its title and MBIDs (unlike
    /// [`Self::merge_releases`], two editions are allowed). Returns how many
    /// tracks moved (none when `from` is gone: merging its last track
    /// removed it).
    pub fn absorb_edition(&self, from: ReleaseId, to: ReleaseId) -> Result<u32> {
        if from == to {
            return Ok(0);
        }
        let device = self.device_id();
        self.write(|tx| {
            let (f, t) = (from.to_string(), to.to_string());
            if first_string(tx, "SELECT id FROM release WHERE id = ?1", [&f])?.is_none() {
                return Ok(0);
            }
            for (id, rid) in [(&f, from), (&t, to)] {
                first_string(tx, "SELECT id FROM release WHERE id = ?1", [id])?
                    .ok_or_else(|| LibraryError::not_found("release", rid))?;
            }
            tx.execute(
                "UPDATE release SET
                        cover_ref = coalesce(cover_ref, (SELECT cover_ref FROM release WHERE id = ?1)),
                        year = coalesce(year, (SELECT year FROM release WHERE id = ?1))
                  WHERE id = ?2",
                params![f, t],
            )?;
            crate::selection::carry_release_choices_in(tx, &f, &t, true, now_ms(), device.as_str())?;
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
            carry_release_links(tx, &f, &t)?;
            crate::streams::refresh_in_library_of_release(tx, &t)?;
            let artists: Vec<String> = {
                let mut stmt = tx.prepare_cached("SELECT artist_id FROM release_artist WHERE release_id = ?1")?;
                let rows = stmt.query_map([&f], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            tx.execute("DELETE FROM release_artist WHERE release_id = ?1", [&f])?;
            tx.execute("DELETE FROM palette WHERE release_id = ?1", [&f])?;
            tx.execute("DELETE FROM release WHERE id = ?1", [&f])?;
            for a in artists {
                crate::repairs::drop_orphan_artist_in(tx, &a)?;
            }
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
        self.write(|tx| upsert_track_in(tx, track))
    }

    /// Puts a track back exactly as `track` was (an undo of a replaced
    /// file): unlike [`Self::upsert_track`], an identifier or a length it
    /// lacked is cleared, not kept from what replaced it.
    pub fn put_track_back(&self, track: &Track) -> Result<Track> {
        self.write(|tx| {
            let stored = upsert_track_in(tx, track)?;
            tx.execute(
                "UPDATE track SET recording_mbid = ?2, isrc = ?3, acoustid = ?4, duration_ms = ?5 WHERE id = ?1",
                params![stored.id.to_string(), track.recording_mbid, track.isrc, track.acoustid, track.duration_ms],
            )?;
            Ok(Track {
                recording_mbid: track.recording_mbid.clone(),
                isrc: track.isrc.clone(),
                acoustid: track.acoustid.clone(),
                duration_ms: track.duration_ms,
                ..stored
            })
        })
    }

    // ---- sources

    /// Adds a source, or updates the existing one with the same `id` or the
    /// same `(kind, locator)` among non-trashed sources (so re-importing a
    /// file is idempotent). Re-selects the track's active source and returns
    /// the stored row.
    pub fn add_source(&self, source: &Source) -> Result<Source> {
        self.write(|tx| add_source_in(tx, source))
    }

    /// Re-selects the active source of a track: local file > Spotify > YouTube,
    /// then the highest `quality_score`, among non-trashed available sources.
    /// Returns the new active source, `None` if the track has no usable one.
    pub fn set_active_source(&self, track: TrackId) -> Result<Option<SourceId>> {
        self.write(|tx| refresh_active(tx, track))
    }

    /// Gives a track that has no length the one heard (`ms`, what the
    /// engine found in a stream whose service did not say). True when it
    /// was stored; a length the track has stays.
    pub fn fill_duration(&self, track: TrackId, ms: u32) -> Result<bool> {
        self.write(|tx| {
            let n = tx.execute(
                "UPDATE track SET duration_ms = ?2 WHERE id = ?1 AND duration_ms IS NULL",
                params![track.to_string(), ms],
            )?;
            Ok(n > 0)
        })
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
            let mut stmt = c.prepare_cached(&format!(
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
            let mut stmt = c.prepare_cached(&format!(
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
            Ok(c.prepare_cached(concat!(
                "SELECT ", track_row_cols!(), " ", track_row_from!(), " WHERE t.id = ?1"
            ))?
            .query_row([id.to_string()], track_row)
            .optional()?)
        })
    }

    /// Live tracks with a file, the latest file first, each with when its
    /// file came («Скачано»: a song liked long ago is as new as its file).
    pub fn downloaded_tracks(&self, limit: u32) -> Result<Vec<(TrackRow, UnixMs)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(concat!(
                "SELECT f.at, ", track_row_cols!(), " ", track_row_from!(),
                " JOIN (SELECT track_id, max(added_at) AS at FROM source
                         WHERE kind = 'local_file' AND trashed_at IS NULL GROUP BY track_id) f ON f.track_id = t.id
                  WHERE t.deleted_at IS NULL
                  ORDER BY f.at DESC, t.album_key, t.disc_no, t.track_no, t.seq LIMIT ?1"
            ))?;
            let rows = stmt.query_map([limit], |r| Ok((track_row_at(r, 1)?, r.get(0)?)))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The rows of `ids` in one query, by id (a missing id has none).
    pub fn get_track_rows(&self, ids: &[TrackId]) -> Result<std::collections::HashMap<TrackId, TrackRow>> {
        if ids.is_empty() {
            return Ok(Default::default());
        }
        let list = serde_json::to_string(&ids.iter().map(ToString::to_string).collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".into());
        self.read(|c| {
            let mut stmt = c.prepare_cached(concat!(
                "SELECT ", track_row_cols!(), " ", track_row_from!(),
                " WHERE t.id IN (SELECT value FROM json_each(?1))"
            ))?;
            let rows = stmt.query_map([list], track_row)?;
            Ok(rows.map(|r| r.map(|r| (r.id, r))).collect::<rusqlite::Result<_>>()?)
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
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {TRACK_COLS} FROM track WHERE {column} = ?1 ORDER BY added_at, id"
            ))?;
            let rows = stmt.query_map([value], track_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    // ---- listings

    /// Tracks of a release in album order (disc, track number).
    pub fn tracks_of_release(&self, release: ReleaseId) -> Result<Vec<TrackRow>> {
        self.tracks_of_release_impl(release, false)
    }

    /// [`Self::tracks_of_release`] with deleted tracks too (an import of the
    /// same song brings the deleted track back instead of making a twin).
    pub fn tracks_of_release_with_deleted(&self, release: ReleaseId) -> Result<Vec<TrackRow>> {
        self.tracks_of_release_impl(release, true)
    }

    /// [`Self::tracks_of_release`] with the tracks outside the library too
    /// (stream tracks asked for in «Хочу скачать» without a like: the ones a
    /// download of the album is for). A stream track outside the library at
    /// the place of a track with a file and as long within 2 s is that
    /// file's song under another title (see `streams::same`) and is left
    /// out: the album lists each of its places once.
    pub fn tracks_of_release_all(&self, release: ReleaseId) -> Result<Vec<TrackRow>> {
        self.tracks_of_release_where(release, concat!("AND ", page_row!()))
    }

    /// The page of an album (docs/stage4.md §12.13): its rows
    /// ([`Self::tracks_of_release_all`]) and a hole for every place it
    /// counts that no row has, in order, with the counts every view of the
    /// album shows (the page's header, its card, «Хочу скачать»):
    /// `progress.total == entries.len()`, `progress.local` the rows with a
    /// file. With one disc a hole has its number (the first ones free up to
    /// the count); with several the holes come last, unnumbered.
    ///
    /// It also says which rows are the user's songs of the album and which
    /// were left out by hand ([`crate::selection`]); its progress counts
    /// them as the album's card does.
    pub fn release_listing(&self, release: ReleaseId) -> Result<Option<ReleaseListing>> {
        let Some(r) = self.get_release(release)? else { return Ok(None) };
        let rows = self.tracks_of_release_all(release)?;
        let rid = release.to_string();
        let unwanted = self.read(|c| crate::unwanted::unwanted_places_in(c, &rid))?;
        type Mine = (Vec<(TrackId, bool, bool)>, bool, Option<String>);
        let (marks, whole, view): Mine = self.read(|c| {
            let mut stmt = c.prepare_cached(concat!(
                "SELECT t.id, t.in_library, ",
                excluded_expr!(),
                " FROM track t WHERE t.release_id = ?1 AND ",
                page_row!()
            ))?;
            let marks = stmt
                .query_map([&rid], |row| Ok((get_id(row, 0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let whole: bool = c.query_row(
                concat!("SELECT ", release_whole_expr!(), " FROM release r WHERE r.id = ?1"),
                [&rid],
                |row| row.get(0),
            )?;
            let view: Option<String> = c
                .query_row(
                    "SELECT view FROM release_selection WHERE release_id = ?1 AND deleted = 0",
                    [&rid],
                    |row| row.get(0),
                )
                .optional()?;
            Ok((marks, whole, view))
        })?;
        let wanted = marks.iter().filter(|m| m.1 && !m.2).map(|m| m.0).collect();
        let excluded = marks.iter().filter(|m| m.2).map(|m| m.0).collect();
        let view = match view.as_deref() {
            Some("all") => Some(true),
            Some("mine") => Some(false),
            _ => None,
        };
        Ok(Some(ReleaseListing::new(rows, r.total_tracks, &unwanted).with_choices(wanted, excluded, whole, view)))
    }

    fn tracks_of_release_impl(&self, release: ReleaseId, deleted: bool) -> Result<Vec<TrackRow>> {
        let live = if deleted { "" } else { "AND t.deleted_at IS NULL AND t.in_library = 1" };
        self.tracks_of_release_where(release, live)
    }

    fn tracks_of_release_where(&self, release: ReleaseId, live: &str) -> Result<Vec<TrackRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "{TRACK_ROW_SELECT} WHERE t.release_id = ?1 {live}
                  ORDER BY t.disc_no, t.track_no, t.seq"
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
            let mut stmt = c.prepare_cached(&format!(
                "{TRACK_ROW_SELECT}
                  WHERE t.id IN (SELECT track_id FROM track_artist WHERE artist_id = ?1)
                    AND t.deleted_at IS NULL AND t.in_library = 1
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
            TrackFilter::All => "WHERE t.deleted_at IS NULL AND t.in_library = 1",
            TrackFilter::LocalOnly => "WHERE t.deleted_at IS NULL AND t.in_library = 1 AND t.has_local = 1",
            TrackFilter::NoLocalFile => "WHERE t.deleted_at IS NULL AND t.in_library = 1 AND t.has_local = 0",
        };
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "{TRACK_ROW_SELECT} {where_} ORDER BY {order} LIMIT ?1 OFFSET ?2"
            ))?;
            let rows = stmt.query_map(params![limit, offset], track_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn count_tracks(&self, filter: TrackFilter) -> Result<u32> {
        let where_ = match filter {
            TrackFilter::All => "WHERE deleted_at IS NULL AND in_library = 1",
            TrackFilter::LocalOnly => "WHERE deleted_at IS NULL AND in_library = 1 AND has_local = 1",
            TrackFilter::NoLocalFile => {
                "WHERE deleted_at IS NULL AND in_library = 1 AND has_local = 0"
            }
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
        self.read(|c| {
            let sql = format!("SELECT COUNT(*) FROM release rv WHERE {RELEASE_VISIBLE}");
            Ok(c.query_row(&sql, [], |r| r.get(0))?)
        })
    }

    pub fn count_artists(&self) -> Result<u32> {
        self.read(|c| {
            let sql = format!("SELECT COUNT(*) FROM artist av WHERE {ARTIST_VISIBLE}");
            Ok(c.query_row(&sql, [], |r| r.get(0))?)
        })
    }

    /// One page of releases with their "N of total locally" counters.
    pub fn releases(&self, offset: u32, limit: u32, sort: ReleaseSort) -> Result<Vec<ReleaseRow>> {
        self.releases_impl(offset, limit, sort, RELEASE_ROW_SELECT)
    }

    /// [`Self::releases`] including releases of deleted tracks only (the
    /// importer must find them rather than create twins).
    pub fn releases_with_deleted(
        &self,
        offset: u32,
        limit: u32,
        sort: ReleaseSort,
    ) -> Result<Vec<ReleaseRow>> {
        self.releases_impl(offset, limit, sort, RELEASE_ROW_SELECT_ALL)
    }

    fn releases_impl(
        &self,
        offset: u32,
        limit: u32,
        sort: ReleaseSort,
        select: &str,
    ) -> Result<Vec<ReleaseRow>> {
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
            let mut stmt = c.prepare_cached(&format!("{select} ORDER BY {order} LIMIT ?1 OFFSET ?2"))?;
            let rows = stmt.query_map(params![limit, offset], release_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// One release as [`Self::releases`] lists it (`None`: not listed).
    pub fn get_release_row(&self, id: ReleaseId) -> Result<Option<ReleaseRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!("{RELEASE_ROW_SELECT} WHERE r.id = ?1"))?;
            Ok(stmt.query_row([id.to_string()], release_row).optional()?)
        })
    }

    /// Releases credited to an artist, newest first.
    pub fn releases_of_artist(&self, artist: ArtistId) -> Result<Vec<ReleaseRow>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
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
        self.artists_impl(offset, limit, sort, ARTIST_ROW_SELECT)
    }

    /// [`Self::artists`] including artists of deleted tracks only.
    pub fn artists_with_deleted(
        &self,
        offset: u32,
        limit: u32,
        sort: ArtistSort,
    ) -> Result<Vec<ArtistRow>> {
        self.artists_impl(offset, limit, sort, ARTIST_ROW_SELECT_ALL)
    }

    fn artists_impl(
        &self,
        offset: u32,
        limit: u32,
        sort: ArtistSort,
        select: &str,
    ) -> Result<Vec<ArtistRow>> {
        let d = if sort.desc { "DESC" } else { "ASC" };
        let order = match sort.key {
            ArtistSortKey::Name => format!("a.sort_key {d}, a.seq {d}"),
            ArtistSortKey::Added => format!("a.added_at {d}, a.seq {d}"),
        };
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!("{select} ORDER BY {order} LIMIT ?1 OFFSET ?2"))?;
            let rows = stmt.query_map(params![limit, offset], artist_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

/// A release (alias `rv`) is listed while it has a live track: one whose
/// tracks are all deleted, or that a retag emptied, is no album to show. The
/// row itself stays (the importer may still point at it).
macro_rules! release_visible {
    () => {
        "EXISTS (SELECT 1 FROM track t WHERE t.release_id = rv.id AND t.deleted_at IS NULL AND t.in_library = 1)"
    };
}

/// An artist (alias `av`) is listed unless all it is credited on (its tracks
/// and the tracks of its releases) is deleted. An artist without any stays.
macro_rules! artist_visible {
    () => {
        "(EXISTS (SELECT 1 FROM track_artist ta JOIN track t ON t.id = ta.track_id
                   WHERE ta.artist_id = av.id AND t.deleted_at IS NULL AND t.in_library = 1)
          OR EXISTS (SELECT 1 FROM release_artist ra JOIN track t ON t.release_id = ra.release_id
                      WHERE ra.artist_id = av.id AND t.deleted_at IS NULL AND t.in_library = 1)
          OR NOT (EXISTS (SELECT 1 FROM track_artist ta WHERE ta.artist_id = av.id)
                  OR EXISTS (SELECT 1 FROM release_artist ra JOIN track t
                                ON t.release_id = ra.release_id WHERE ra.artist_id = av.id)))"
    };
}

pub(crate) const RELEASE_VISIBLE: &str = release_visible!();
pub(crate) const ARTIST_VISIBLE: &str = artist_visible!();

/// The columns of a [`ReleaseRow`] (alias `r`); deleted tracks count nowhere.
/// The user's songs of it ([`crate::selection`]) are its page's rows in the
/// library and not left out of it.
/// Its count leaves out the places of the songs the user deleted from it
/// that no live track takes (`unwanted`): no holes of the album.
macro_rules! release_row_cols {
    () => {
        concat!(
            "SELECT r.id, r.mbid, r.release_group_mbid, r.title, r.artist_credit, r.year,
                    r.release_type, r.cover_ref,
                    CASE WHEN r.total_tracks IS NULL THEN NULL ELSE max(r.total_tracks - (
                        SELECT COUNT(*) FROM unwanted u WHERE u.release_id = r.id
                           AND u.track_no BETWEEN 1 AND r.total_tracks
                           AND NOT EXISTS (SELECT 1 FROM track o WHERE o.release_id = r.id AND o.deleted_at IS NULL
                                              AND (o.title_key = u.title_key
                                                   OR (o.disc_no = u.disc_no AND o.track_no = u.track_no)))), 0) END,
                    r.added_at,
                    (SELECT COUNT(*) FROM track t WHERE t.release_id = r.id AND t.has_local = 1
                                                    AND t.deleted_at IS NULL AND t.in_library = 1),
                    (SELECT COUNT(*) FROM track t WHERE t.release_id = r.id AND t.deleted_at IS NULL AND t.in_library = 1),
                    ",
            release_liked_expr!(),
            ",
                    (SELECT COUNT(*) FROM track t WHERE t.release_id = r.id AND ",
            page_row!(),
            "),
                    (SELECT COUNT(*) FROM track t WHERE t.release_id = r.id AND t.in_library = 1 AND NOT ",
            excluded_expr!(),
            " AND ",
            page_row!(),
            "),
                    (SELECT COUNT(*) FROM track t WHERE t.release_id = r.id AND t.in_library = 1 AND t.has_local = 1
                        AND NOT ",
            excluded_expr!(),
            " AND ",
            page_row!(),
            "),
                    ",
            release_whole_expr!(),
            ",
                    (SELECT s.view FROM release_selection s WHERE s.release_id = r.id AND s.deleted = 0)"
        )
    };
}

/// Visible releases with their counters. Callers append `JOIN` / `WHERE` /
/// `ORDER BY` against alias `r`.
pub(crate) const RELEASE_ROW_SELECT: &str = concat!(
    release_row_cols!(),
    " FROM (SELECT * FROM release rv WHERE ",
    release_visible!(),
    ") r"
);

/// Every release, those of deleted tracks included (the importer's matching).
const RELEASE_ROW_SELECT_ALL: &str = concat!(release_row_cols!(), " FROM release r");

/// How many columns [`release_row`] reads: a query that adds its own puts
/// them after.
pub(crate) const RELEASE_ROW_COLS: usize = 18;

pub(crate) fn release_row(row: &Row<'_>) -> rusqlite::Result<ReleaseRow> {
    Ok(ReleaseRow {
        release: release_from_row(row)?,
        local_tracks: row.get::<_, i64>(10)? as u32,
        known_tracks: row.get::<_, i64>(11)? as u32,
        liked: row.get(12)?,
        rows: row.get::<_, i64>(13)? as u32,
        wanted: row.get::<_, i64>(14)? as u32,
        wanted_local: row.get::<_, i64>(15)? as u32,
        whole: row.get(16)?,
        view: row.get::<_, Option<String>>(17)?.and_then(|v| match v.as_str() {
            "all" => Some(true),
            "mine" => Some(false),
            _ => None,
        }),
    })
}

/// The columns of an [`ArtistRow`] (alias `a`): visible releases, live local tracks.
macro_rules! artist_row_cols {
    () => {
        concat!(
            "SELECT a.id, a.mbid, a.name, a.sort_name, a.images, a.added_at,
                    (SELECT COUNT(*) FROM release_artist ra2 JOIN release rv ON rv.id = ra2.release_id
                      WHERE ra2.artist_id = a.id AND ",
            release_visible!(),
            "),
                    (SELECT COUNT(*) FROM track_artist ta JOIN track t ON t.id = ta.track_id
                      WHERE ta.artist_id = a.id AND t.has_local = 1 AND t.deleted_at IS NULL AND t.in_library = 1),
                    EXISTS(SELECT 1 FROM likes lk WHERE lk.target_kind = 'artist'
                            AND lk.target_id = a.id AND lk.deleted = 0)"
        )
    };
}

/// Visible artists with their counters.
pub(crate) const ARTIST_ROW_SELECT: &str = concat!(
    artist_row_cols!(),
    " FROM (SELECT * FROM artist av WHERE ",
    artist_visible!(),
    ") a"
);

/// Every artist, those credited on deleted tracks only included.
const ARTIST_ROW_SELECT_ALL: &str = concat!(artist_row_cols!(), " FROM artist a");

pub(crate) fn artist_row(row: &Row<'_>) -> rusqlite::Result<ArtistRow> {
    Ok(ArtistRow {
        artist: artist_from_row(row)?,
        release_count: row.get::<_, i64>(6)? as u32,
        local_track_count: row.get::<_, i64>(7)? as u32,
        liked: row.get(8)?,
    })
}
