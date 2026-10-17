//! Stream tracks and the service links (tables `external_id`,
//! `playlist_remote`, column `track.in_library`).
//!
//! * A track heard from Spotify / YouTube gets a permanent `track` row (its
//!   history needs an id) through [`Library::upsert_stream_track`], with a
//!   stream [`Source`] and its ids in the services. It stays out of the
//!   library listings (`in_library = 0`) until it is liked or put in a live
//!   playlist.
//! * `in_library` is maintained here ([`refresh_in_library`]) and recomputed
//!   whenever likes, playlist entries or sources change: 1 for a track that
//!   has a local source row (live or in Trash) or no stream source at all,
//!   otherwise 1 only while it is liked or in a live playlist. Tracks of
//!   local files therefore always stay in the library, exactly as before.
//! * A downloaded file of a stream track is added to the *same* track as a
//!   local source (the importer's job); the local file then plays, and the
//!   track is a library track for good.
//! * A track known only from an imported listening history, without an id in
//!   any service ("seen", see [`import`]), has no source at all; a marker row
//!   in `external_id` (service `seen`) keeps it out of the library too.

use nikgob_core::model::{
    Artist, ArtistCredit, ArtistId, PlaylistId, Release, ReleaseId, Source, SourceKind, Track,
    TrackId, UnixMs, Uuid,
};
use nikgob_core::streaming::{
    AlbumRef, ArtistRef, EntityKind, PlaylistSyncMode, RemoteId, Service, StreamTrack, TrackKind,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::catalog::{
    TRACK_COLS, add_source_in, track_from_row, upsert_artist_in, upsert_release_in,
    upsert_track_in,
};
use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::types::{PlaylistRemote, RemoteLike};
use crate::util::{get_id, meta_get, meta_set, sort_key};

/// `meta` key of the time a service's likes were first pulled.
fn likes_key(service: Service) -> String {
    format!("likes_pulled.{}", service.as_str())
}

/// `in_library` of the track row aliased `track` (see the module docs).
const IN_LIBRARY_EXPR: &str = "(
    EXISTS (SELECT 1 FROM source s WHERE s.track_id = track.id AND s.kind = 'local_file')
    OR (NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = track.id AND s.kind != 'local_file')
        AND NOT EXISTS (SELECT 1 FROM external_id x WHERE x.kind = 'track' AND x.local_id = track.id
                                                      AND x.service = 'seen'))
    OR EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'track' AND l.target_id = track.id
                                       AND l.deleted = 0)
    OR EXISTS (SELECT 1 FROM playlist_entry e JOIN playlist p ON p.id = e.playlist_id
                WHERE e.track_id = track.id AND e.deleted = 0 AND p.deleted = 0))";

/// Recomputes `in_library` of one track (by id text).
pub(crate) fn refresh_in_library(conn: &Connection, track: &str) -> Result<()> {
    conn.execute(
        &format!(
            "UPDATE track SET in_library = {IN_LIBRARY_EXPR}
              WHERE id = ?1 AND in_library IS NOT {IN_LIBRARY_EXPR}"
        ),
        [track],
    )?;
    Ok(())
}

/// Recomputes `in_library` of every track of a playlist (deleted entries too).
pub(crate) fn refresh_in_library_of_playlist(conn: &Connection, playlist: &str) -> Result<()> {
    conn.execute(
        &format!(
            "UPDATE track SET in_library = {IN_LIBRARY_EXPR}
              WHERE id IN (SELECT track_id FROM playlist_entry WHERE playlist_id = ?1)
                AND in_library IS NOT {IN_LIBRARY_EXPR}"
        ),
        [playlist],
    )?;
    Ok(())
}

/// Recomputes `in_library` of every track; returns how many changed.
pub(crate) fn refresh_in_library_all(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        &format!(
            "UPDATE track SET in_library = {IN_LIBRARY_EXPR}
              WHERE in_library IS NOT {IN_LIBRARY_EXPR}"
        ),
        [],
    )?)
}

// ------------------------------------------------------------- external ids

pub(crate) fn external_local(
    conn: &Connection,
    service: Service,
    kind: EntityKind,
    remote: &str,
) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT local_id FROM external_id WHERE service = ?1 AND kind = ?2 AND remote_id = ?3",
            params![service.as_str(), kind.as_str(), remote],
            |r| r.get(0),
        )
        .optional()?)
}

pub(crate) fn set_external(
    conn: &Connection,
    service: Service,
    kind: EntityKind,
    remote: &str,
    local: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO external_id(service, kind, remote_id, local_id) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(service, kind, remote_id) DO UPDATE SET local_id = excluded.local_id",
        params![service.as_str(), kind.as_str(), remote, local],
    )?;
    Ok(())
}

pub(crate) fn externals_of(
    conn: &Connection,
    kind: EntityKind,
    local: &str,
) -> Result<Vec<RemoteId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT service, remote_id FROM external_id WHERE kind = ?1 AND local_id = ?2
          ORDER BY service, remote_id",
    )?;
    let rows = stmt.query_map(params![kind.as_str(), local], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (service, remote) = row?;
        // A service unknown to this build comes from a newer one: skip it.
        if let Some(service) = Service::parse(&service) {
            out.push(RemoteId::new(service, remote));
        }
    }
    Ok(out)
}

fn exists(conn: &Connection, table: &str, id: &str) -> Result<bool> {
    Ok(conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
        [id],
        |r| r.get(0),
    )?)
}

/// The local row a remote id is linked to, if that row still exists.
fn linked(
    conn: &Connection,
    table: &str,
    service: Service,
    kind: EntityKind,
    remote: &str,
) -> Result<Option<String>> {
    match external_local(conn, service, kind, remote)? {
        Some(id) if exists(conn, table, &id)? => Ok(Some(id)),
        _ => Ok(None),
    }
}

fn parse<T: std::str::FromStr>(s: &str) -> Result<T> {
    s.parse().map_err(|_| LibraryError::Invalid(s.to_owned()))
}

/// Upper case, without dashes and spaces; `None` when empty.
fn normal_isrc(isrc: Option<&str>) -> Option<String> {
    let s: String = isrc?
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (!s.is_empty()).then_some(s)
}

// ------------------------------------------------------------ stream tracks

/// The library artist for a credited stream artist: the one linked to its
/// remote id, else the first one with the same name (so a stream track of
/// "Кино" lands on the library's Кино), else a new one.
fn artist_for(conn: &Connection, a: &ArtistRef) -> Result<ArtistId> {
    if let Some(remote) = &a.id
        && let Some(id) = linked(
            conn,
            "artist",
            remote.service,
            EntityKind::Artist,
            &remote.id,
        )?
    {
        return parse(&id);
    }
    let by_name: Option<String> = conn
        .query_row(
            "SELECT id FROM artist WHERE sort_key = ?1 ORDER BY seq LIMIT 1",
            [sort_key(&a.name)],
            |r| r.get(0),
        )
        .optional()?;
    let id = match by_name {
        Some(id) => parse(&id)?,
        None => upsert_artist_in(conn, &Artist::new(a.name.clone()))?.id,
    };
    if let Some(remote) = &a.id {
        set_external(
            conn,
            remote.service,
            EntityKind::Artist,
            &remote.id,
            &id.to_string(),
        )?;
    }
    Ok(id)
}

fn credit_for(conn: &Connection, artists: &[ArtistRef]) -> Result<ArtistCredit> {
    let mut credit = ArtistCredit::default();
    for a in artists {
        let id = artist_for(conn, a)?;
        credit.push(", ", a.name.clone(), Some(id));
    }
    Ok(credit)
}

/// The library release of a stream album: linked by remote id, else the one
/// with the same title and album artist, else a new one.
fn release_for(
    conn: &Connection,
    album: &AlbumRef,
    track_artists: &[ArtistRef],
) -> Result<ReleaseId> {
    if let Some(remote) = &album.id
        && let Some(id) = linked(
            conn,
            "release",
            remote.service,
            EntityKind::Album,
            &remote.id,
        )?
    {
        return parse(&id);
    }
    let artists = if album.artists.is_empty() {
        track_artists
    } else {
        &album.artists
    };
    let credit = credit_for(conn, artists)?;
    let same: Option<String> = conn
        .query_row(
            "SELECT id FROM release WHERE title_key = ?1 AND artist_key = ?2 ORDER BY seq LIMIT 1",
            params![sort_key(&album.title), sort_key(&credit.display())],
            |r| r.get(0),
        )
        .optional()?;
    let id = match same {
        Some(id) => parse(&id)?,
        None => {
            let mut r = Release::new(album.title.clone(), credit);
            r.year = album.year;
            upsert_release_in(conn, &r)?.id
        }
    };
    if let Some(remote) = &album.id {
        set_external(
            conn,
            remote.service,
            EntityKind::Album,
            &remote.id,
            &id.to_string(),
        )?;
    }
    Ok(id)
}

/// Links the remote ids of a stream track's album and artists to the
/// release and credited artists the library track already has (matched by
/// name), unless those remote ids are linked already.
fn link_known(conn: &Connection, track: TrackId, t: &StreamTrack) -> Result<()> {
    let row: Option<(String, Option<String>)> = conn
        .query_row(
            "SELECT artist_credit, release_id FROM track WHERE id = ?1",
            [track.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((credit, release)) = row else {
        return Ok(());
    };
    let credit: ArtistCredit = serde_json::from_str(&credit)?;
    let link = |kind: EntityKind, remote: &RemoteId, local: &str| -> Result<()> {
        if external_local(conn, remote.service, kind, &remote.id)?.is_none() {
            set_external(conn, remote.service, kind, &remote.id, local)?;
        }
        Ok(())
    };
    if let (Some(rid), Some(remote)) = (&release, t.album.as_ref().and_then(|a| a.id.as_ref())) {
        link(EntityKind::Album, remote, rid)?;
    }
    for a in &t.artists {
        let Some(remote) = &a.id else { continue };
        let key = sort_key(&a.name);
        if let Some(local) = credit
            .0
            .iter()
            .find(|p| sort_key(&p.name) == key)
            .and_then(|p| p.artist_id)
        {
            link(EntityKind::Artist, remote, &local.to_string())?;
        }
    }
    Ok(())
}

/// A cover reference that is a web address (a stream cover), not a file.
pub fn is_web_ref(cover_ref: &str) -> bool {
    cover_ref.starts_with("https://") || cover_ref.starts_with("http://")
}

/// Gives the release of a stream track the stream's cover url: when it has
/// no cover, or (`own`, the stream owns the track's metadata) when its
/// cover is another url. A cover file always stays.
fn set_stream_cover(conn: &Connection, track: TrackId, t: &StreamTrack, own: bool) -> Result<()> {
    let Some(url) = t.cover_url.as_deref().filter(|u| is_web_ref(u)) else {
        return Ok(());
    };
    if t.kind == TrackKind::Video {
        return Ok(()); // a frame of the clip, not the album's cover
    }
    conn.execute(
        "UPDATE release SET cover_ref = ?2
          WHERE id = (SELECT release_id FROM track WHERE id = ?1)
            AND (cover_ref IS NULL OR cover_ref = ''
                 OR (?3 AND (cover_ref LIKE 'https://%' OR cover_ref LIKE 'http://%')))
            AND cover_ref IS NOT ?2",
        params![track.to_string(), url, own],
    )?;
    Ok(())
}

/// The track has a live source of a service ranked above `service`.
fn better_stream_source(conn: &Connection, track: &str, service: Service) -> Result<bool> {
    let mine = service.source_kind().priority();
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT kind FROM source
          WHERE track_id = ?1 AND kind != 'local_file' AND trashed_at IS NULL",
    )?;
    let kinds = stmt.query_map([track], |r| r.get::<_, String>(0))?;
    for kind in kinds {
        if SourceKind::parse(&kind?).is_some_and(|k| k.priority() < mine) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn has_local_source(conn: &Connection, track: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM source WHERE track_id = ?1 AND kind = 'local_file')",
        [track],
        |r| r.get(0),
    )?)
}

/// See [`Library::upsert_stream_track`]. With `want`, the stream is attached
/// to that track (created with that id when missing) instead of being looked
/// up: the userdata import restores stream tracks under their old ids.
pub(crate) fn upsert_stream_track_in(
    conn: &Connection,
    t: &StreamTrack,
    want: Option<TrackId>,
) -> Result<TrackId> {
    let service = t.id.service;
    let isrc = normal_isrc(t.isrc.as_deref());
    // Which track, and may its metadata follow the stream?
    let (existing, own) = match want {
        Some(id) => {
            let found = exists(conn, "track", &id.to_string())?;
            (found.then_some(id), !found)
        }
        None => match linked(conn, "track", service, EntityKind::Track, &t.id.id)? {
            // The metadata follows the best service the track has (a
            // YouTube copy attached to a Spotify track does not rename it).
            Some(id) => {
                let better = better_stream_source(conn, &id, service)?;
                (Some(parse(&id)?), !better)
            }
            None => {
                let by_isrc: Option<String> = match &isrc {
                    Some(isrc) => conn
                        .query_row(
                            "SELECT id FROM track WHERE isrc = ?1
                              ORDER BY deleted_at IS NOT NULL, has_local DESC, seq LIMIT 1",
                            [isrc],
                            |r| r.get(0),
                        )
                        .optional()?,
                    None => None,
                };
                // Else the same song by name, artist, album and length
                // (the YouTube copy of a Spotify track has no ISRC).
                let found = match by_isrc {
                    Some(id) => Some(id),
                    None => same::same_song_track(conn, t)?,
                };
                (found.map(|id| parse(&id)).transpose()?, false)
            }
        },
    };
    let id = match existing {
        // Another service's or a local file's track: only gains a source
        // (and its album and artists learn their ids in this service).
        Some(id) if !own || has_local_source(conn, &id.to_string())? => {
            link_known(conn, id, t)?;
            set_stream_cover(conn, id, t, false)?;
            id
        }
        _ => {
            let credit = credit_for(conn, &t.artists)?;
            let release_id = t
                .album
                .as_ref()
                .map(|a| release_for(conn, a, &t.artists))
                .transpose()?;
            let mut track = Track::new(t.title.clone(), credit);
            track.id = existing.or(want).unwrap_or_default();
            track.isrc = isrc;
            track.release_id = release_id;
            track.disc_no = t.disc_no.unwrap_or(1).max(1);
            track.track_no = t.track_no.unwrap_or(0);
            track.duration_ms = t.duration_ms.and_then(|ms| u32::try_from(ms).ok());
            let fresh = existing.is_none();
            let id = upsert_track_in(conn, &track)?.id;
            set_stream_cover(conn, id, t, true)?;
            if fresh {
                // Out of the library from the start (no flash of a new row).
                conn.execute(
                    "UPDATE track SET in_library = 0 WHERE id = ?1",
                    [id.to_string()],
                )?;
            }
            id
        }
    };
    let mut source = Source::new(id, service.source_kind(), t.id.id.clone(), service.codec());
    source.bitrate = Some(service.bitrate_kbps());
    source.quality_score = service.quality_score();
    source.available = t.playable;
    add_source_in(conn, &source)?;
    set_external(conn, service, EntityKind::Track, &t.id.id, &id.to_string())?;
    refresh_in_library(conn, &id.to_string())?;
    Ok(id)
}

fn artist_refs(
    conn: &Connection,
    credit: &ArtistCredit,
    service: Service,
) -> Result<Vec<ArtistRef>> {
    credit
        .0
        .iter()
        .map(|part| {
            let id = match part.artist_id {
                Some(a) => externals_of(conn, EntityKind::Artist, &a.to_string())?
                    .into_iter()
                    .find(|r| r.service == service),
                None => None,
            };
            Ok(ArtistRef::new(id, part.name.clone()))
        })
        .collect()
}

/// title, artist_credit, release_id, disc_no, track_no, duration_ms, isrc
type TrackCols = (
    String,
    String,
    Option<String>,
    i64,
    i64,
    Option<i64>,
    Option<String>,
);

/// The stream tracks a library track is known as, one per live stream
/// source, rebuilt from the database (for the userdata export).
pub(crate) fn stream_refs_in(conn: &Connection, track: TrackId) -> Result<Vec<StreamTrack>> {
    let tid = track.to_string();
    let row: Option<TrackCols> = conn
        .query_row(
            "SELECT title, artist_credit, release_id, disc_no, track_no, duration_ms, isrc
               FROM track WHERE id = ?1",
            [&tid],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?;
    let Some((title, credit, release, disc, no, duration, isrc)) = row else {
        return Ok(Vec::new());
    };
    let credit: ArtistCredit = serde_json::from_str(&credit)?;
    let album: Option<(String, String, Option<i64>)> = match &release {
        Some(rid) => conn
            .query_row(
                "SELECT title, artist_credit, year FROM release WHERE id = ?1",
                [rid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?,
        None => None,
    };
    let sources: Vec<(String, String, bool)> = {
        let mut stmt = conn.prepare(
            "SELECT kind, locator, available FROM source
              WHERE track_id = ?1 AND kind != 'local_file' AND trashed_at IS NULL
              ORDER BY kind, added_at, id",
        )?;
        let rows = stmt.query_map([&tid], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut out = Vec::new();
    for (kind, locator, available) in sources {
        let Some(service) = SourceKind::parse(&kind).and_then(SourceKind::service) else {
            continue;
        };
        let mut t = StreamTrack::new(RemoteId::new(service, locator), title.clone());
        t.artists = artist_refs(conn, &credit, service)?;
        if let (Some(rid), Some((atitle, acredit, year))) = (&release, &album) {
            let acredit: ArtistCredit = serde_json::from_str(acredit)?;
            let mut a = AlbumRef::new(
                externals_of(conn, EntityKind::Album, rid)?
                    .into_iter()
                    .find(|r| r.service == service),
                atitle.clone(),
            );
            a.artists = artist_refs(conn, &acredit, service)?;
            a.year = year.map(|y| y as i32);
            t.album = Some(a);
        }
        t.duration_ms = duration.map(|d| d.max(0) as u64);
        t.isrc = isrc.clone();
        t.disc_no = Some(disc.max(1) as u32);
        t.track_no = (no > 0).then_some(no as u32);
        t.playable = available;
        out.push(t);
    }
    Ok(out)
}

// --------------------------------------------------------- playlist remotes

const REMOTE_COLS: &str =
    "playlist_id, service, remote_id, sync_mode, remote_version, synced_at, base_json";

fn remote_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<PlaylistRemote>> {
    let service: String = row.get(1)?;
    let mode: String = row.get(3)?;
    let (Some(service), Some(sync_mode)) =
        (Service::parse(&service), PlaylistSyncMode::parse(&mode))
    else {
        return Ok(None);
    };
    Ok(Some(PlaylistRemote {
        playlist_id: get_id(row, 0)?,
        remote: RemoteId::new(service, row.get::<_, String>(2)?),
        sync_mode,
        remote_version: row.get(4)?,
        synced_at: row.get(5)?,
        base_json: row.get(6)?,
    }))
}

impl Library {
    // ---- stream tracks

    /// The library track of a stream track, creating it when needed, and
    /// returns its id. Found by the stream's remote id, else by ISRC, else
    /// as the same song by artist, title, album and length (see [`same`]; so
    /// the same song from the other service, or a downloaded file, gets this
    /// stream as one more source), else created: with its release and
    /// artists (matched by remote id, then by name), outside the library
    /// (`in_library = 0`) until liked or put in a playlist.
    ///
    /// Always records a stream [`Source`] (locator = remote id, codec
    /// vorbis/opus, a `quality_score` below any local file) and the remote ids
    /// of the track, its album and artists. Calling it again for the same
    /// remote track returns the same id and refreshes the metadata of a
    /// stream-only track (a track with a local file keeps its tags).
    pub fn upsert_stream_track(&self, track: &StreamTrack) -> Result<TrackId> {
        self.write(|tx| upsert_stream_track_in(tx, track, None))
    }

    /// [`Self::upsert_stream_track`] for many tracks in one transaction.
    pub fn upsert_stream_tracks(&self, tracks: &[StreamTrack]) -> Result<Vec<TrackId>> {
        self.write(|tx| {
            tracks
                .iter()
                .map(|t| upsert_stream_track_in(tx, t, None))
                .collect()
        })
    }

    /// The library track a stream track already is, without writing: by
    /// remote id, else ISRC (a live track), else the same song (see
    /// [`same`]). What [`Self::upsert_stream_track`] would attach it to.
    pub fn track_for_stream(&self, t: &StreamTrack) -> Result<Option<TrackId>> {
        self.read(|c| {
            if let Some(id) = linked(c, "track", t.id.service, EntityKind::Track, &t.id.id)? {
                return parse(&id).map(Some);
            }
            if let Some(isrc) = normal_isrc(t.isrc.as_deref()) {
                let found: Option<String> = c
                    .query_row(
                        "SELECT id FROM track WHERE isrc = ?1 AND deleted_at IS NULL
                          ORDER BY has_local DESC, seq LIMIT 1",
                        [isrc],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(id) = found {
                    return parse(&id).map(Some);
                }
            }
            same::same_song_track(c, t)?.map(|id| parse(&id)).transpose()
        })
    }

    /// What a service says about an album the library links to (search
    /// hits, pages, discographies): fills the release's track count and
    /// cover url when it has none (a cover file and a known count stay).
    /// Returns whether it changed.
    pub fn learn_stream_album(
        &self,
        album: &RemoteId,
        total_tracks: Option<u32>,
        cover_url: Option<&str>,
    ) -> Result<bool> {
        let cover = cover_url.filter(|u| is_web_ref(u));
        if total_tracks.is_none() && cover.is_none() {
            return Ok(false);
        }
        self.write(|tx| {
            let Some(id) = linked(tx, "release", album.service, EntityKind::Album, &album.id)? else {
                return Ok(false);
            };
            Ok(tx.execute(
                "UPDATE release SET total_tracks = coalesce(total_tracks, ?2),
                        cover_ref = CASE WHEN cover_ref IS NULL OR cover_ref = '' THEN ?3 ELSE cover_ref END
                  WHERE id = ?1
                    AND ((total_tracks IS NULL AND ?2 IS NOT NULL)
                         OR ((cover_ref IS NULL OR cover_ref = '') AND ?3 IS NOT NULL))",
                params![id, total_tracks, cover],
            )? > 0)
        })
    }

    /// Gives a library artist the image url a service shows for it, unless
    /// it has an image of another kind (a file); returns whether it changed.
    pub fn set_stream_artist_image(&self, artist: ArtistId, url: &str) -> Result<bool> {
        if !is_web_ref(url) {
            return Ok(false);
        }
        self.write(|tx| {
            let images: Option<String> = tx
                .query_row("SELECT images FROM artist WHERE id = ?1", [artist.to_string()], |r| r.get(0))
                .optional()?;
            let Some(images) = images else { return Ok(false) };
            let images: Vec<String> = serde_json::from_str(&images).unwrap_or_default();
            if images.first().is_some_and(|i| i == url) || !images.iter().all(|i| is_web_ref(i)) {
                return Ok(false);
            }
            let json = serde_json::to_string(&[url])?;
            tx.execute("UPDATE artist SET images = ?2 WHERE id = ?1", params![artist.to_string(), json])?;
            Ok(true)
        })
    }

    /// The stream tracks a library track is known as (one per stream source).
    pub fn stream_refs(&self, track: TrackId) -> Result<Vec<StreamTrack>> {
        self.read(|c| stream_refs_in(c, track))
    }

    /// Adds the stream `t` as one more source of the existing `track` and
    /// links its remote ids (track, and album / artists where the names
    /// agree), keeping the track's own metadata. For a match found some
    /// other way than by ISRC (the YouTube copy of a Spotify-only track,
    /// found by name and duration): [`Self::upsert_stream_track`] would make
    /// a second track of it. The stream's ISRC fills an empty one.
    ///
    /// When the stream already has a track of its own (liked on that
    /// service before the match was found), that track is merged into
    /// `track` (its likes, playlist entries, history and wishlist item move
    /// over; see [`Self::merge_track_into`]).
    pub fn attach_stream_source(&self, track: TrackId, t: &StreamTrack) -> Result<()> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| {
            let tid = track.to_string();
            if !exists(tx, "track", &tid)? {
                return Err(LibraryError::not_found("track", tid));
            }
            // The stream is a track of its own already: one song, one track.
            if let Some(other) = linked(tx, "track", t.id.service, EntityKind::Track, &t.id.id)?
                && other != tid
            {
                crate::catalog::merge_track_in(tx, parse(&other)?, track, device.as_str(), now)?;
            }
            upsert_stream_track_in(tx, t, Some(track))?;
            if let Some(isrc) = normal_isrc(t.isrc.as_deref()) {
                tx.execute(
                    "UPDATE track SET isrc = ?2 WHERE id = ?1 AND (isrc IS NULL OR isrc = '')",
                    params![tid, isrc],
                )?;
            }
            Ok(())
        })
    }

    /// Live tracks without a local file that have a stream source: what a
    /// downloaded file may turn out to be (the importer's Decide stage).
    pub fn stream_only_tracks(&self) -> Result<Vec<Track>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {TRACK_COLS} FROM track
                  WHERE deleted_at IS NULL
                    AND NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = track.id
                                       AND s.kind = 'local_file' AND s.trashed_at IS NULL)
                    AND EXISTS (SELECT 1 FROM source s WHERE s.track_id = track.id
                                   AND s.kind != 'local_file' AND s.trashed_at IS NULL)
                  ORDER BY seq"
            ))?;
            let rows = stmt.query_map([], track_from_row)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    // ---- likes of the services

    /// The service's liked tracks as the last pull saw them.
    pub fn remote_likes(&self, service: Service) -> Result<Vec<RemoteLike>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT remote_id, track_id, pulled, seen_at FROM remote_like
                  WHERE service = ?1 ORDER BY seen_at, remote_id",
            )?;
            let rows = stmt.query_map([service.as_str()], |r| {
                Ok(RemoteLike {
                    remote: RemoteId::new(service, r.get::<_, String>(0)?),
                    track_id: get_id(r, 1)?,
                    pulled: r.get(2)?,
                    seen_at: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The remote likes of a track, in every service.
    pub fn remote_likes_of(&self, track: TrackId) -> Result<Vec<RemoteLike>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT service, remote_id, pulled, seen_at FROM remote_like
                  WHERE track_id = ?1 ORDER BY service, remote_id",
            )?;
            let rows = stmt.query_map([track.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (service, remote, pulled, seen_at) = row?;
                if let Some(service) = Service::parse(&service) {
                    out.push(RemoteLike {
                        remote: RemoteId::new(service, remote),
                        track_id: track,
                        pulled,
                        seen_at,
                    });
                }
            }
            Ok(out)
        })
    }

    /// Records that the service likes `like.remote` (insert or replace).
    pub fn put_remote_like(&self, like: &RemoteLike) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO remote_like(service, remote_id, track_id, pulled, seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(service, remote_id) DO UPDATE SET track_id = excluded.track_id,
                    pulled = excluded.pulled, seen_at = excluded.seen_at",
                params![
                    like.remote.service.as_str(),
                    like.remote.id,
                    like.track_id.to_string(),
                    like.pulled,
                    like.seen_at
                ],
            )?;
            Ok(())
        })
    }

    /// The service no longer likes `remote`; returns whether it was recorded.
    pub fn delete_remote_like(&self, remote: &RemoteId) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM remote_like WHERE service = ?1 AND remote_id = ?2",
                params![remote.service.as_str(), remote.id],
            )? > 0)
        })
    }

    /// Whether the service's likes were pulled at least once (the first pull
    /// is an import: its likes wait in the wishlist instead of downloading).
    pub fn likes_pulled(&self, service: Service) -> Result<bool> {
        self.read(|c| Ok(meta_get(c, &likes_key(service))?.is_some()))
    }

    pub fn set_likes_pulled(&self, service: Service, at: UnixMs) -> Result<()> {
        self.write(|tx| meta_set(tx, &likes_key(service), &at.to_string()))
    }

    /// Whether the track is shown in the library; `None` for an unknown track.
    pub fn track_in_library(&self, track: TrackId) -> Result<Option<bool>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT in_library FROM track WHERE id = ?1",
                [track.to_string()],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Recomputes `in_library` of these tracks (the library does it itself
    /// on likes, playlist and source changes; this is for repairs).
    pub fn recompute_in_library(&self, tracks: &[TrackId]) -> Result<()> {
        self.write(|tx| {
            for t in tracks {
                refresh_in_library(tx, &t.to_string())?;
            }
            Ok(())
        })
    }

    /// Recomputes `in_library` of every track; returns how many changed.
    pub fn recompute_in_library_all(&self) -> Result<usize> {
        self.write(|tx| refresh_in_library_all(tx))
    }

    // ---- external ids

    /// Links `remote` (of kind `kind`) to the local row `local` (a track,
    /// release, artist or playlist id), replacing an older link of `remote`.
    pub fn set_external_id(&self, kind: EntityKind, remote: &RemoteId, local: Uuid) -> Result<()> {
        self.write(|tx| set_external(tx, remote.service, kind, &remote.id, &local.to_string()))
    }

    /// The local id `remote` is linked to (the row may be gone).
    pub fn external_id_local(&self, kind: EntityKind, remote: &RemoteId) -> Result<Option<Uuid>> {
        self.read(|c| {
            external_local(c, remote.service, kind, &remote.id)?
                .map(|s| parse(&s))
                .transpose()
        })
    }

    /// Every remote id linked to the local row `local`.
    pub fn external_ids(&self, kind: EntityKind, local: Uuid) -> Result<Vec<RemoteId>> {
        self.read(|c| externals_of(c, kind, &local.to_string()))
    }

    /// Removes the link of `remote`; returns whether it existed.
    pub fn remove_external_id(&self, kind: EntityKind, remote: &RemoteId) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM external_id WHERE service = ?1 AND kind = ?2 AND remote_id = ?3",
                params![remote.service.as_str(), kind.as_str(), remote.id],
            )? > 0)
        })
    }

    /// The existing library track linked to a remote track.
    pub fn track_by_remote(&self, remote: &RemoteId) -> Result<Option<TrackId>> {
        self.read(|c| {
            linked(c, "track", remote.service, EntityKind::Track, &remote.id)?
                .map(|s| parse(&s))
                .transpose()
        })
    }

    /// The existing library release linked to a remote album.
    pub fn release_by_remote(&self, remote: &RemoteId) -> Result<Option<ReleaseId>> {
        self.read(|c| {
            linked(c, "release", remote.service, EntityKind::Album, &remote.id)?
                .map(|s| parse(&s))
                .transpose()
        })
    }

    /// The existing library artist linked to a remote artist.
    pub fn artist_by_remote(&self, remote: &RemoteId) -> Result<Option<ArtistId>> {
        self.read(|c| {
            linked(c, "artist", remote.service, EntityKind::Artist, &remote.id)?
                .map(|s| parse(&s))
                .transpose()
        })
    }

    // ---- playlist remotes

    /// Inserts or replaces the service link of a playlist.
    pub fn put_playlist_remote(&self, r: &PlaylistRemote) -> Result<()> {
        self.write(|tx| {
            // One remote playlist is linked to one library playlist.
            tx.execute(
                "DELETE FROM playlist_remote WHERE service = ?1 AND remote_id = ?2
                    AND playlist_id != ?3",
                params![
                    r.remote.service.as_str(),
                    r.remote.id,
                    r.playlist_id.to_string()
                ],
            )?;
            tx.execute(
                "INSERT INTO playlist_remote(playlist_id, service, remote_id, sync_mode,
                                             remote_version, synced_at, base_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(playlist_id) DO UPDATE SET service = excluded.service,
                    remote_id = excluded.remote_id, sync_mode = excluded.sync_mode,
                    remote_version = excluded.remote_version, synced_at = excluded.synced_at,
                    base_json = excluded.base_json",
                params![
                    r.playlist_id.to_string(),
                    r.remote.service.as_str(),
                    r.remote.id,
                    r.sync_mode.as_str(),
                    r.remote_version,
                    r.synced_at,
                    r.base_json
                ],
            )?;
            Ok(())
        })
    }

    pub fn playlist_remote(&self, playlist: PlaylistId) -> Result<Option<PlaylistRemote>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!("SELECT {REMOTE_COLS} FROM playlist_remote WHERE playlist_id = ?1"),
                [playlist.to_string()],
                remote_from_row,
            )
            .optional()?
            .flatten())
        })
    }

    /// The library playlist linked to a remote playlist.
    pub fn playlist_by_remote(&self, remote: &RemoteId) -> Result<Option<PlaylistRemote>> {
        self.read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {REMOTE_COLS} FROM playlist_remote WHERE service = ?1 AND remote_id = ?2"
                ),
                params![remote.service.as_str(), remote.id],
                remote_from_row,
            )
            .optional()?
            .flatten())
        })
    }

    /// Every playlist link, by playlist id.
    pub fn playlist_remotes(&self) -> Result<Vec<PlaylistRemote>> {
        self.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {REMOTE_COLS} FROM playlist_remote ORDER BY playlist_id"
            ))?;
            let rows = stmt.query_map([], remote_from_row)?;
            let mut out = Vec::new();
            for r in rows {
                out.extend(r?);
            }
            Ok(out)
        })
    }

    /// Records a sync: the new remote version, when, and the merge base.
    pub fn mark_playlist_synced(
        &self,
        playlist: PlaylistId,
        remote_version: Option<&str>,
        at: UnixMs,
        base_json: Option<&str>,
    ) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "UPDATE playlist_remote SET remote_version = ?2, synced_at = ?3, base_json = ?4
                  WHERE playlist_id = ?1",
                params![playlist.to_string(), remote_version, at, base_json],
            )? > 0)
        })
    }

    /// Unlinks a playlist; returns whether it was linked.
    pub fn delete_playlist_remote(&self, playlist: PlaylistId) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM playlist_remote WHERE playlist_id = ?1",
                [playlist.to_string()],
            )? > 0)
        })
    }
}

mod import;
mod same;

pub use import::event_id;

#[cfg(test)]
mod tests;
