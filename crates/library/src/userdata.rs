//! Export and import of user data (playlists, likes, history).
//!
//! The export lives next to the music (`Music/.musiclib/userdata/`) so the
//! database can be rebuilt from the library folder alone, and doubles as the
//! interchange format for sync:
//!
//! * `playlists.json` - playlists with their entries, tombstones included;
//! * `likes.json` - the LWW like set, tombstones included;
//! * `events.jsonl` - one event per line, oldest first (append-friendly: tools
//!   may append lines, import ignores ids it already has);
//! * `streams.json` - the stream-only tracks the likes and playlists point at
//!   (no file to rebuild them from): their ids and their remote ids and
//!   metadata per service, so an import recreates them under the same ids.
//!   Written only once there is something to write.
//! * `rec_feedback.json` - «Не рекомендовать» marks (tracks, artists and the
//!   songs and artists of the services), cleared ones included; merged by
//!   their time like likes. Written only once there is something to write.
//!
//! Every file is written to a temp file and renamed into place. Import merges
//! with last-writer-wins per row: the copy with the larger `updated_at` wins;
//! on a tie a tombstone beats a live row, then the larger `updated_by`
//! decides. It is therefore commutative, idempotent and safe to run on top of
//! an existing database.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use nikgob_core::model::{Event, Like, Playlist, PlaylistEntry, TrackId, Uuid};
use nikgob_core::streaming::StreamTrack;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::events::insert_event;
use crate::playlists::{ENTRY_COLS, PLAYLIST_COLS, entry_from_row, playlist_from_row};
use crate::streams::{refresh_in_library_all, stream_refs_in, upsert_stream_track_in};
use crate::types::{ExportReport, ImportReport, RecFeedback};
use crate::util::{meta_get, meta_set};

/// Format version written into the JSON files.
pub const USERDATA_VERSION: u32 = 1;

pub const PLAYLISTS_FILE: &str = "playlists.json";
pub const LIKES_FILE: &str = "likes.json";
pub const EVENTS_FILE: &str = "events.jsonl";
pub const STREAMS_FILE: &str = "streams.json";
pub const REC_FEEDBACK_FILE: &str = "rec_feedback.json";

#[derive(Serialize, Deserialize)]
struct PlaylistsFile {
    version: u32,
    playlists: Vec<PlaylistDoc>,
}

#[derive(Serialize, Deserialize)]
struct PlaylistDoc {
    #[serde(flatten)]
    playlist: Playlist,
    #[serde(default)]
    entries: Vec<PlaylistEntry>,
}

#[derive(Serialize, Deserialize)]
struct StreamsFile {
    version: u32,
    tracks: Vec<StreamDoc>,
}

#[derive(Serialize, Deserialize)]
struct StreamDoc {
    track_id: TrackId,
    /// One per service the track streams from.
    refs: Vec<StreamTrack>,
}

#[derive(Serialize, Deserialize)]
struct RecFeedbackFile {
    version: u32,
    feedback: Vec<RecFeedback>,
}

#[derive(Serialize, Deserialize)]
struct LikesFile {
    version: u32,
    likes: Vec<Like>,
}

/// Does the incoming copy of a row replace the stored one?
fn incoming_wins(new: (i64, bool, &str), old: (i64, bool, &str)) -> bool {
    let (new_ts, new_deleted, new_by) = new;
    let (old_ts, old_deleted, old_by) = old;
    if new_ts != old_ts {
        new_ts > old_ts
    } else if new_deleted != old_deleted {
        new_deleted
    } else {
        new_by > old_by
    }
}

/// Writes `bytes` to `path` via temp file + rename, unless the file already
/// holds exactly these bytes. Returns whether the file changed.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<bool> {
    if fs::read(path).is_ok_and(|existing| existing == bytes) {
        return Ok(false);
    }
    atomic_with(path, |w| w.write_all(bytes))?;
    Ok(true)
}

fn atomic_with(
    path: &Path,
    fill: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>,
) -> io::Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", Uuid::now_v7().simple()));
    let result = (|| {
        let mut w = BufWriter::new(File::create(&tmp)?);
        fill(&mut w)?;
        w.flush()?;
        w.get_ref().sync_all()?;
        drop(w);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn events_marker_key(dir: &Path) -> String {
    format!("events_export_count:{}", dir.display())
}

/// Where the last export of `events.jsonl` ended: how many events it held,
/// the last one written (by `ts, id`) and the file's length then.
struct EventsMark {
    count: u64,
    last: (i64, String),
    len: u64,
}

impl EventsMark {
    fn parse(s: &str) -> Option<Self> {
        let mut it = s.splitn(4, '|');
        let count = it.next()?.parse().ok()?;
        let ts = it.next()?.parse().ok()?;
        let id = it.next()?.to_owned();
        let len = it.next()?.parse().ok()?;
        Some(Self { count, last: (ts, id), len })
    }

    fn encode(&self) -> String {
        format!("{}|{}|{}|{}", self.count, self.last.0, self.last.1, self.len)
    }
}

/// `events.jsonl` brought up to date with the database: left alone when the
/// last export holds every event, the newer events appended when only newer
/// ones came (the usual case: plays since the last export), else written
/// anew (events older than the last one written, removed ones, a file
/// changed by someone else). The new mark, when the file changed.
fn export_events(
    c: &Connection,
    path: &Path,
    mark: Option<EventsMark>,
    count: u64,
) -> Result<Option<EventsMark>> {
    let len = fs::metadata(path).ok().map(|m| m.len());
    if let Some(m) = mark.filter(|m| len == Some(m.len) && m.count <= count) {
        if m.count == count {
            return Ok(None);
        }
        let mut newer = Vec::new();
        let (n, last) = write_events(c, &mut newer, Some((m.last.0, &m.last.1)))?;
        if m.count + n == count {
            let mut f = fs::OpenOptions::new().append(true).open(path)?;
            f.write_all(&newer)?;
            f.sync_all()?;
            let last = last.unwrap_or(m.last);
            return Ok(Some(EventsMark { count, last, len: m.len + newer.len() as u64 }));
        }
    }
    let mut last = None;
    atomic_with(path, |w| {
        last = write_events(c, w, None)?.1;
        Ok(())
    })?;
    let len = fs::metadata(path)?.len();
    Ok(Some(EventsMark { count, last: last.unwrap_or((i64::MIN, String::new())), len }))
}

impl Library {
    /// Writes `playlists.json`, `likes.json` and `events.jsonl` into `dir`
    /// (created if missing). Files whose content did not change are left
    /// untouched; new events are appended to `events.jsonl` (see
    /// [`export_events`]).
    pub fn export_userdata(&self, dir: impl AsRef<Path>) -> Result<ExportReport> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;
        let marker_key = events_marker_key(dir);
        let events_path = dir.join(EVENTS_FILE);

        // One snapshot for everything.
        let (playlists, likes, streams, feedback, events_mark, event_count) = self.read(|c| {
            let playlists = read_playlists(c)?;
            let likes = read_likes(c)?;
            let streams = read_streams(c)?;
            let feedback = read_feedback(c)?;
            let event_count: u64 =
                c.query_row("SELECT COUNT(*) FROM event", [], |r| r.get::<_, i64>(0))? as u64;
            let mark = meta_get(c, &marker_key)?.as_deref().and_then(EventsMark::parse);
            let mark = export_events(c, &events_path, mark, event_count)?;
            Ok((playlists, likes, streams, feedback, mark, event_count))
        })?;

        let mut report = ExportReport {
            playlists: playlists.len(),
            playlist_entries: playlists.iter().map(|p| p.entries.len()).sum(),
            likes: likes.len(),
            events: event_count as usize,
            stream_tracks: streams.len(),
            rec_feedback: feedback.len(),
            files_written: usize::from(events_mark.is_some()),
        };
        let feedback_path = dir.join(REC_FEEDBACK_FILE);
        if !feedback.is_empty() || feedback_path.exists() {
            let json = serde_json::to_vec_pretty(&RecFeedbackFile {
                version: USERDATA_VERSION,
                feedback,
            })?;
            report.files_written += usize::from(write_atomic(&feedback_path, &json)?);
        }
        let streams_path = dir.join(STREAMS_FILE);
        if !streams.is_empty() || streams_path.exists() {
            let json = serde_json::to_vec_pretty(&StreamsFile {
                version: USERDATA_VERSION,
                tracks: streams,
            })?;
            report.files_written += usize::from(write_atomic(&streams_path, &json)?);
        }

        let playlists_json = serde_json::to_vec_pretty(&PlaylistsFile {
            version: USERDATA_VERSION,
            playlists,
        })?;
        let likes_json = serde_json::to_vec_pretty(&LikesFile {
            version: USERDATA_VERSION,
            likes,
        })?;
        report.files_written +=
            usize::from(write_atomic(&dir.join(PLAYLISTS_FILE), &playlists_json)?);
        report.files_written += usize::from(write_atomic(&dir.join(LIKES_FILE), &likes_json)?);

        if let Some(mark) = events_mark {
            self.write(|tx| meta_set(tx, &marker_key, &mark.encode()))?;
        }
        Ok(report)
    }

    /// A cheap fingerprint of what [`Library::export_userdata`] writes: the
    /// same as at the last export means there is nothing new to write
    /// (playlists, entries, likes and feedback are stamped on every change,
    /// a tombstone included; events only come; stream tracks come with
    /// their sources).
    pub fn userdata_fingerprint(&self) -> Result<Vec<i64>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT (SELECT COUNT(*) FROM playlist), (SELECT COALESCE(SUM(updated_at), 0) FROM playlist),
                        (SELECT COUNT(*) FROM playlist_entry),
                        (SELECT COALESCE(SUM(updated_at), 0) FROM playlist_entry),
                        (SELECT COUNT(*) FROM likes), (SELECT COALESCE(SUM(updated_at), 0) FROM likes),
                        (SELECT COUNT(*) FROM rec_feedback),
                        (SELECT COALESCE(SUM(updated_at + dislike), 0) FROM rec_feedback),
                        (SELECT COUNT(*) FROM event), (SELECT COALESCE(MAX(ts), 0) FROM event),
                        (SELECT COUNT(*) FROM track), (SELECT COUNT(*) FROM source)",
            )?;
            Ok(stmt.query_row([], |r| (0..12).map(|i| r.get(i)).collect())?)
        })
    }

    /// Merges the files found in `dir` into the database (missing files are
    /// skipped). See the module docs for the merge rules.
    pub fn import_userdata(&self, dir: impl AsRef<Path>) -> Result<ImportReport> {
        let dir = dir.as_ref();

        let playlists: Option<PlaylistsFile> = read_json(&dir.join(PLAYLISTS_FILE))?;
        let likes: Option<LikesFile> = read_json(&dir.join(LIKES_FILE))?;
        let streams: Option<StreamsFile> = read_json(&dir.join(STREAMS_FILE))?;
        let feedback: Option<RecFeedbackFile> = read_json(&dir.join(REC_FEEDBACK_FILE))?;
        for version in [
            playlists.as_ref().map(|f| f.version),
            likes.as_ref().map(|f| f.version),
            streams.as_ref().map(|f| f.version),
            feedback.as_ref().map(|f| f.version),
        ]
        .into_iter()
        .flatten()
        {
            if version > USERDATA_VERSION {
                return Err(LibraryError::UnsupportedFormat(version));
            }
        }

        let mut events = Vec::new();
        let mut invalid_lines = 0;
        match File::open(dir.join(EVENTS_FILE)) {
            Ok(file) => {
                for line in BufReader::new(file).lines() {
                    let line = line?;
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Event>(&line) {
                        Ok(e) => events.push(e),
                        Err(_) => invalid_lines += 1,
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }

        self.write(|tx| {
            let mut report = ImportReport {
                invalid_lines,
                ..ImportReport::default()
            };
            for doc in streams.iter().flat_map(|f| &f.tracks) {
                for r in &doc.refs {
                    upsert_stream_track_in(tx, r, Some(doc.track_id))?;
                }
                report.stream_tracks += 1;
            }
            for doc in playlists.iter().flat_map(|f| &f.playlists) {
                merge_playlist(tx, &doc.playlist, &mut report)?;
                for entry in &doc.entries {
                    merge_entry(tx, entry, &mut report)?;
                }
            }
            for like in likes.iter().flat_map(|f| &f.likes) {
                merge_like(tx, like, &mut report)?;
            }
            for f in feedback.iter().flat_map(|f| &f.feedback) {
                merge_feedback(tx, f, &mut report)?;
            }
            for event in &events {
                if insert_event(tx, event)? {
                    report.events_added += 1;
                }
            }
            // Likes and entries came in: stream tracks may join or leave.
            refresh_in_library_all(tx)?;
            Ok(report)
        })
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn read_playlists(conn: &Connection) -> Result<Vec<PlaylistDoc>> {
    let mut stmt = conn.prepare_cached(&format!("SELECT {PLAYLIST_COLS} FROM playlist ORDER BY id"))?;
    let playlists = stmt
        .query_map([], playlist_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut entries_stmt = conn.prepare_cached(&format!(
        "SELECT {ENTRY_COLS} FROM playlist_entry WHERE playlist_id = ?1 ORDER BY position, id"
    ))?;
    playlists
        .into_iter()
        .map(|playlist| {
            let entries = entries_stmt
                .query_map([playlist.id.to_string()], entry_from_row)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(PlaylistDoc { playlist, entries })
        })
        .collect()
}

/// Stream-only tracks (no local source row) that a live like or a live
/// playlist entry points at.
fn read_streams(conn: &Connection) -> Result<Vec<StreamDoc>> {
    let ids: Vec<TrackId> = {
        let mut stmt = conn.prepare_cached(
            "SELECT t.id FROM track t
              WHERE NOT EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id
                                   AND s.kind = 'local_file')
                AND EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id
                               AND s.kind != 'local_file' AND s.trashed_at IS NULL)
                AND (EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'track'
                                AND l.target_id = t.id AND l.deleted = 0)
                     OR EXISTS (SELECT 1 FROM playlist_entry e WHERE e.track_id = t.id
                                   AND e.deleted = 0))
              ORDER BY t.id",
        )?;
        let rows = stmt.query_map([], |r| crate::util::get_id(r, 0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut out = Vec::new();
    for track_id in ids {
        let refs = stream_refs_in(conn, track_id)?;
        if !refs.is_empty() {
            out.push(StreamDoc { track_id, refs });
        }
    }
    Ok(out)
}

fn read_likes(conn: &Connection) -> Result<Vec<Like>> {
    let mut stmt = conn.prepare_cached(
        "SELECT target_kind, target_id, updated_at, updated_by, deleted FROM likes
          ORDER BY target_kind, target_id",
    )?;
    let rows = stmt.query_map([], |r| {
        let kind: String = r.get(0)?;
        let id: String = r.get(1)?;
        Ok((kind, id, r.get(2)?, r.get::<_, String>(3)?, r.get(4)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (kind, id, updated_at, updated_by, deleted) = row?;
        // Rows with an unknown kind can only come from a newer schema; skip them.
        if let Some(target) = nikgob_core::model::LikeTarget::parse(&kind, &id) {
            out.push(Like {
                target,
                updated_at,
                updated_by: updated_by.into(),
                deleted,
            });
        }
    }
    Ok(out)
}

fn read_feedback(conn: &Connection) -> Result<Vec<RecFeedback>> {
    let mut stmt = conn.prepare_cached(
        "SELECT target_kind, target_id, dislike, updated_at FROM rec_feedback
          ORDER BY target_kind, target_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(RecFeedback {
            target_kind: r.get(0)?,
            target_id: r.get(1)?,
            dislike: r.get(2)?,
            updated_at: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// The later mark wins (the stored one on a tie).
fn merge_feedback(conn: &Connection, f: &RecFeedback, report: &mut ImportReport) -> Result<()> {
    let n = conn.execute(
        "INSERT INTO rec_feedback(target_kind, target_id, dislike, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(target_kind, target_id) DO UPDATE SET dislike = excluded.dislike,
            updated_at = excluded.updated_at
          WHERE excluded.updated_at > rec_feedback.updated_at",
        params![f.target_kind, f.target_id, f.dislike, f.updated_at],
    )?;
    if n > 0 {
        report.applied += 1;
    } else {
        report.skipped += 1;
    }
    Ok(())
}

/// Writes the events (all, or those after `after` by `ts, id`) one per line,
/// oldest first; how many, and the last one's `ts, id`.
fn write_events(
    conn: &Connection,
    w: &mut impl Write,
    after: Option<(i64, &str)>,
) -> io::Result<(u64, Option<(i64, String)>)> {
    let to_io = |e: rusqlite::Error| io::Error::other(e);
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, ts, track_id, source_kind, type, position_ms, context, device
               FROM event WHERE ?1 IS NULL OR ts > ?1 OR (ts = ?1 AND id > ?2)
              ORDER BY ts, id",
        )
        .map_err(to_io)?;
    let rows = stmt
        .query_map(params![after.map(|a| a.0), after.map_or("", |a| a.1)], crate::events::event_from_row)
        .map_err(to_io)?;
    let (mut n, mut last) = (0, None);
    for row in rows {
        let event: Event = row.map_err(to_io)?;
        serde_json::to_writer(&mut *w, &event).map_err(io::Error::other)?;
        w.write_all(b"\n")?;
        n += 1;
        last = Some((event.ts, event.id.to_string()));
    }
    Ok((n, last))
}

fn merge_playlist(conn: &Connection, p: &Playlist, report: &mut ImportReport) -> Result<()> {
    let existing: Option<(i64, bool, String, i64)> = conn
        .query_row(
            "SELECT updated_at, deleted, updated_by, created_at FROM playlist WHERE id = ?1",
            [p.id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    match existing {
        None => {
            conn.execute(
                "INSERT INTO playlist(id, name, cover, created_at, updated_at, updated_by, deleted)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    p.id.to_string(),
                    p.name,
                    p.cover,
                    p.created_at,
                    p.updated_at,
                    p.updated_by.as_str(),
                    p.deleted
                ],
            )?;
            report.applied += 1;
        }
        Some((ts, deleted, by, created)) => {
            if incoming_wins(
                (p.updated_at, p.deleted, p.updated_by.as_str()),
                (ts, deleted, &by),
            ) {
                conn.execute(
                    "UPDATE playlist SET name = ?2, cover = ?3, created_at = ?4, updated_at = ?5,
                            updated_by = ?6, deleted = ?7 WHERE id = ?1",
                    params![
                        p.id.to_string(),
                        p.name,
                        p.cover,
                        created.min(p.created_at),
                        p.updated_at,
                        p.updated_by.as_str(),
                        p.deleted
                    ],
                )?;
                report.applied += 1;
            } else {
                report.skipped += 1;
            }
        }
    }
    Ok(())
}

fn merge_entry(conn: &Connection, e: &PlaylistEntry, report: &mut ImportReport) -> Result<()> {
    let existing: Option<(i64, bool, String)> = conn
        .query_row(
            "SELECT updated_at, deleted, updated_by FROM playlist_entry WHERE id = ?1",
            [e.id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let wins = match &existing {
        None => true,
        Some((ts, deleted, by)) => incoming_wins(
            (e.updated_at, e.deleted, e.updated_by.as_str()),
            (*ts, *deleted, by),
        ),
    };
    if !wins {
        report.skipped += 1;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO playlist_entry(id, playlist_id, track_id, position, added_at, updated_at,
                                    updated_by, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET playlist_id = excluded.playlist_id,
            track_id = excluded.track_id, position = excluded.position,
            added_at = excluded.added_at, updated_at = excluded.updated_at,
            updated_by = excluded.updated_by, deleted = excluded.deleted",
        params![
            e.id.to_string(),
            e.playlist_id.to_string(),
            e.track_id.to_string(),
            e.position,
            e.added_at,
            e.updated_at,
            e.updated_by.as_str(),
            e.deleted
        ],
    )?;
    report.applied += 1;
    Ok(())
}

fn merge_like(conn: &Connection, like: &Like, report: &mut ImportReport) -> Result<()> {
    let kind = like.target.kind_str();
    let id = like.target.id_string();
    let existing: Option<(i64, bool, String)> = conn
        .query_row(
            "SELECT updated_at, deleted, updated_by FROM likes
              WHERE target_kind = ?1 AND target_id = ?2",
            params![kind, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let wins = match &existing {
        None => true,
        Some((ts, deleted, by)) => incoming_wins(
            (like.updated_at, like.deleted, like.updated_by.as_str()),
            (*ts, *deleted, by),
        ),
    };
    if !wins {
        report.skipped += 1;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO likes(target_kind, target_id, updated_at, updated_by, deleted)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(target_kind, target_id) DO UPDATE SET updated_at = excluded.updated_at,
            updated_by = excluded.updated_by, deleted = excluded.deleted",
        params![
            kind,
            id,
            like.updated_at,
            like.updated_by.as_str(),
            like.deleted
        ],
    )?;
    report.applied += 1;
    Ok(())
}
