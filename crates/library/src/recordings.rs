//! One song on two albums (migration 0022): an OST and its special edition
//! both keep their track, with its own number, tags, cover and file; the
//! two tracks become one *recording*. Nothing is deleted or moved: a link
//! is a row.
//!
//! * [`Library::link_recording`]: two tracks become one recording (their
//!   groups merge), unless the user said they are different songs
//!   ([`Library::set_recording_apart`]); a like of either is a like of all.
//! * What finds them: [`Library::recording_id_pairs`] (the same ISRC,
//!   recording MBID or AcoustID on two albums) and
//!   [`Library::prints_to_link`] (the stored prints not yet compared); the
//!   rule that decides is `nikgob_import::same_audio::same_recording`.
//! * What a link does here: a like or unlike of one track is one of every
//!   track of its recording ([`spread_like_in`]); «Любимые» and the search
//!   show one row of it (`recording_spare`); the plays of either count for
//!   the recording ([`crate::events`]); the lyrics found for one are the
//!   other's when the two are as long ([`Library::share_recording_lyrics`]);
//!   a track item of «Хочу скачать» is done when another album has the
//!   recording's file (`masters::has_recording_file`).

use std::collections::HashMap;

use nikgob_core::model::{ReleaseId, SourceId, TrackId, Uuid};
use rusqlite::{Connection, OptionalExtension, params};

use crate::db::Library;
use crate::error::Result;
use crate::prints::print_from_blob;
use crate::streams::refresh_in_library;
use crate::util::{get_id, get_opt_id, now_ms, opt_u32};

/// What decides whether a track is the same recording as another one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingSide {
    pub track_id: TrackId,
    pub title: String,
    pub release_id: Option<ReleaseId>,
    pub duration_ms: Option<u32>,
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    pub acoustid: Option<String>,
}

/// A stored print not yet compared with the other albums.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintToLink {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub print: Vec<u32>,
    /// Length of the file (the track's when the print does not say).
    pub duration_ms: Option<u32>,
}

/// A link as exported (`recordings.json`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordingLink {
    pub track_id: TrackId,
    pub recording: String,
    pub how: String,
    pub at: i64,
}

/// Two tracks the user said are different songs, as exported.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordingApart {
    pub track_id: TrackId,
    pub other_track: TrackId,
    pub at: i64,
}

const SIDE_COLS: &str = "id, title, release_id, duration_ms, isrc, recording_mbid, acoustid";

fn side_at(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<RecordingSide> {
    Ok(RecordingSide {
        track_id: get_id(r, at)?,
        title: r.get(at + 1)?,
        release_id: get_opt_id(r, at + 2)?,
        duration_ms: opt_u32(r, at + 3)?,
        isrc: r.get(at + 4)?,
        recording_mbid: r.get(at + 5)?,
        acoustid: r.get(at + 6)?,
    })
}

/// The recording `track` is in, if any.
fn group_in(c: &Connection, track: &str) -> Result<Option<String>> {
    Ok(c.prepare_cached("SELECT recording FROM recording_link WHERE track_id = ?1")?
        .query_row([track], |r| r.get(0))
        .optional()?)
}

/// The tracks of the recording `track` is in (just `track` without one).
fn members_in(c: &Connection, track: &str) -> Result<Vec<String>> {
    let Some(g) = group_in(c, track)? else { return Ok(vec![track.to_owned()]) };
    let mut stmt = c.prepare_cached("SELECT track_id FROM recording_link WHERE recording = ?1 ORDER BY track_id")?;
    let rows = stmt.query_map([g], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The other live tracks of `track`'s recording, the oldest first.
pub(crate) fn twins_in(c: &Connection, track: &str) -> Result<Vec<String>> {
    let mut stmt = c.prepare_cached(
        "SELECT b.track_id FROM recording_link a
           JOIN recording_link b ON b.recording = a.recording AND b.track_id != a.track_id
           JOIN track t ON t.id = b.track_id
          WHERE a.track_id = ?1 AND t.deleted_at IS NULL
          ORDER BY t.seq",
    )?;
    let rows = stmt.query_map([track], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn apart_in(c: &Connection, a: &str, b: &str) -> Result<bool> {
    Ok(c
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM recording_apart
                            WHERE (track_id = ?1 AND other_track = ?2) OR (track_id = ?2 AND other_track = ?1))",
        )?
        .query_row(params![a, b], |r| r.get(0))?)
}

fn put_apart_in(c: &Connection, a: &str, b: &str, at: i64) -> Result<()> {
    if a == b {
        return Ok(());
    }
    let (x, y) = if a <= b { (a, b) } else { (b, a) };
    c.execute(
        "INSERT OR IGNORE INTO recording_apart(track_id, other_track, at) VALUES (?1, ?2, ?3)",
        params![x, y, at],
    )?;
    Ok(())
}

/// Links `a` and `b` (merging their recordings) unless they are one
/// already or a track of one was said to be another song than a track of
/// the other. A like of any track of the result goes to all of them.
/// Returns whether anything changed.
pub(crate) fn link_in(c: &Connection, a: &str, b: &str, how: &str, at: i64, device: &str) -> Result<bool> {
    if a == b {
        return Ok(false);
    }
    let (ga, gb) = (group_in(c, a)?, group_in(c, b)?);
    if ga.is_some() && ga == gb {
        return Ok(false);
    }
    let (ma, mb) = (members_in(c, a)?, members_in(c, b)?);
    for x in &ma {
        for y in &mb {
            if apart_in(c, x, y)? {
                return Ok(false);
            }
        }
    }
    let insert = |track: &str, group: &str| -> Result<()> {
        c.execute(
            "INSERT INTO recording_link(track_id, recording, how, at) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(track_id) DO UPDATE SET recording = excluded.recording",
            params![track, group, how, at],
        )?;
        Ok(())
    };
    match (ga, gb) {
        (Some(x), Some(y)) => {
            c.execute("UPDATE recording_link SET recording = ?1 WHERE recording = ?2", params![x, y])?;
        }
        (Some(x), None) => insert(b, &x)?,
        (None, Some(y)) => insert(a, &y)?,
        (None, None) => {
            let g = Uuid::now_v7().to_string();
            insert(a, &g)?;
            insert(b, &g)?;
        }
    }
    // One ♡ for the recording: liked once, liked everywhere.
    let all = members_in(c, a)?;
    let liked = {
        let mut stmt = c.prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM likes WHERE target_kind = 'track' AND target_id = ?1 AND deleted = 0)",
        )?;
        let mut any = false;
        for t in &all {
            any |= stmt.query_row([t], |r| r.get::<_, bool>(0))?;
        }
        any
    };
    if liked {
        for t in &all {
            like_row_in(c, t, false, at, device)?;
        }
    }
    Ok(true)
}

/// Sets the like row of track `t` (a tombstone only over a live like;
/// a row that says so already keeps its stamp).
fn like_row_in(c: &Connection, t: &str, deleted: bool, at: i64, device: &str) -> Result<()> {
    let changed = if deleted {
        c.execute(
            "UPDATE likes SET deleted = 1, updated_at = ?2, updated_by = ?3
              WHERE target_kind = 'track' AND target_id = ?1 AND deleted = 0",
            params![t, at, device],
        )?
    } else {
        c.execute(
            "INSERT INTO likes(target_kind, target_id, updated_at, updated_by, deleted)
             VALUES ('track', ?1, ?2, ?3, 0)
             ON CONFLICT(target_kind, target_id) DO UPDATE SET
                updated_at = excluded.updated_at, updated_by = excluded.updated_by, deleted = 0
              WHERE likes.deleted != 0",
            params![t, at, device],
        )?
    };
    if changed > 0 {
        refresh_in_library(c, t)?;
    }
    Ok(())
}

/// A like (or, `deleted`, an unlike) of `track` goes to the other tracks of
/// its recording: one ♡ for the song.
pub(crate) fn spread_like_in(c: &Connection, track: &str, deleted: bool, at: i64, device: &str) -> Result<()> {
    for t in twins_in(c, track)? {
        like_row_in(c, &t, deleted, at, device)?;
    }
    Ok(())
}

/// `from` folds into `into` (one track imported twice): its place in a
/// recording and what it was said to be apart from go to `into`.
pub(crate) fn carry_recording_in(c: &Connection, from: &str, into: &str) -> Result<()> {
    match (group_in(c, from)?, group_in(c, into)?) {
        (Some(_), None) => {
            c.execute("UPDATE recording_link SET track_id = ?2 WHERE track_id = ?1", params![from, into])?;
        }
        (Some(f), Some(i)) => {
            if f != i {
                c.execute("UPDATE recording_link SET recording = ?2 WHERE recording = ?1", params![f, i])?;
            }
            c.execute("DELETE FROM recording_link WHERE track_id = ?1", [from])?;
        }
        _ => {}
    }
    c.execute("UPDATE OR IGNORE recording_apart SET track_id = ?2 WHERE track_id = ?1", params![from, into])?;
    c.execute("UPDATE OR IGNORE recording_apart SET other_track = ?2 WHERE other_track = ?1", params![from, into])?;
    c.execute("DELETE FROM recording_apart WHERE track_id = ?1 OR other_track = ?1 OR track_id = other_track", [from])?;
    Ok(())
}

fn parse(s: &str) -> Result<TrackId> {
    s.parse().map_err(|_| crate::error::LibraryError::Invalid(s.to_owned()))
}

impl Library {
    /// `a` and `b` are one recording (`how`: `ids`, `audio`, `user`).
    /// Returns whether anything changed (nothing when they are one already,
    /// or were said to be different songs).
    pub fn link_recording(&self, a: TrackId, b: TrackId, how: &str) -> Result<bool> {
        let device = self.device_id();
        let now = self.tick();
        self.write(|tx| link_in(tx, &a.to_string(), &b.to_string(), how, now, device.as_str()))
    }

    /// «Разные песни»: `a` and `b` are never linked, whatever finds them.
    pub fn set_recording_apart(&self, a: TrackId, b: TrackId) -> Result<()> {
        self.write(|tx| put_apart_in(tx, &a.to_string(), &b.to_string(), now_ms()))
    }

    /// Whether `a` and `b` were said to be different songs.
    pub fn recording_apart(&self, a: TrackId, b: TrackId) -> Result<bool> {
        self.read(|c| apart_in(c, &a.to_string(), &b.to_string()))
    }

    /// Takes `track` out of its recording: it is another song than every
    /// track that was linked with it (never linked again). Returns those.
    pub fn unlink_recording(&self, track: TrackId) -> Result<Vec<TrackId>> {
        self.write(|tx| {
            let t = track.to_string();
            let Some(g) = group_in(tx, &t)? else { return Ok(Vec::new()) };
            let others: Vec<String> = members_in(tx, &t)?.into_iter().filter(|m| *m != t).collect();
            tx.execute("DELETE FROM recording_link WHERE track_id = ?1", [&t])?;
            let at = now_ms();
            for o in &others {
                put_apart_in(tx, &t, o, at)?;
            }
            // A recording of one track is none.
            tx.execute(
                "DELETE FROM recording_link WHERE recording = ?1
                    AND (SELECT count(*) FROM recording_link WHERE recording = ?1) < 2",
                [&g],
            )?;
            others.iter().map(|o| parse(o)).collect()
        })
    }

    /// Whether `a` and `b` are one recording.
    pub fn same_recording(&self, a: TrackId, b: TrackId) -> Result<bool> {
        if a == b {
            return Ok(true);
        }
        self.read(|c| {
            let (ga, gb) = (group_in(c, &a.to_string())?, group_in(c, &b.to_string())?);
            Ok(ga.is_some() && ga == gb)
        })
    }

    /// The other live tracks of `track`'s recording (the same song on other
    /// albums), the oldest first.
    pub fn recording_twins(&self, track: TrackId) -> Result<Vec<TrackId>> {
        self.read(|c| twins_in(c, &track.to_string())?.iter().map(|t| parse(t)).collect())
    }

    /// Every linked track that is not the copy standing for its recording,
    /// with that copy (`recording_canon`): what «Любимые», the search, the
    /// history and the mixes fold into one.
    pub fn recording_spares(&self) -> Result<HashMap<TrackId, TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT track_id, canon FROM recording_spare")?;
            let rows = stmt.query_map([], |r| Ok((get_id(r, 0)?, get_id(r, 1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// What decides whether `track` is another track's recording.
    pub fn recording_side(&self, track: TrackId) -> Result<Option<RecordingSide>> {
        self.read(|c| {
            Ok(c.prepare_cached(&format!("SELECT {SIDE_COLS} FROM track WHERE id = ?1 AND deleted_at IS NULL"))?
                .query_row([track.to_string()], |r| side_at(r, 0))
                .optional()?)
        })
    }

    /// Two live tracks on different albums (or without one) with the same
    /// ISRC, recording MBID or AcoustID, at least one with a file, not one
    /// recording yet nor said to be different songs: `track`'s only
    /// (`Some`), or all of them once. Tracks in the library, with a file or
    /// wished for only (not every stream track the history saw).
    pub fn recording_id_pairs(&self, track: Option<TrackId>) -> Result<Vec<(RecordingSide, RecordingSide)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "WITH live AS (
                     SELECT t.id, t.title, t.release_id, t.duration_ms, t.isrc, t.recording_mbid, t.acoustid, t.has_local
                       FROM track t
                      WHERE t.deleted_at IS NULL
                        AND (t.in_library = 1 OR t.has_local = 1
                             OR EXISTS (SELECT 1 FROM wishlist w WHERE w.track_id = t.id))),
                 pairs AS (
                     SELECT a.id AS x, b.id AS y FROM live a JOIN live b ON b.isrc = a.isrc AND b.id != a.id
                      WHERE coalesce(a.isrc, '') != ''
                     UNION
                     SELECT a.id, b.id FROM live a JOIN live b ON b.recording_mbid = a.recording_mbid AND b.id != a.id
                      WHERE coalesce(a.recording_mbid, '') != ''
                     UNION
                     SELECT a.id, b.id FROM live a JOIN live b ON b.acoustid = a.acoustid AND b.id != a.id
                      WHERE coalesce(a.acoustid, '') != '')
                 SELECT a.id, a.title, a.release_id, a.duration_ms, a.isrc, a.recording_mbid, a.acoustid,
                        b.id, b.title, b.release_id, b.duration_ms, b.isrc, b.recording_mbid, b.acoustid
                   FROM pairs p JOIN live a ON a.id = p.x JOIN live b ON b.id = p.y
                  WHERE (?1 IS NULL OR a.id = ?1) AND (?1 IS NOT NULL OR a.id < b.id)
                    AND (a.has_local = 1 OR b.has_local = 1)
                    AND NOT (a.release_id IS NOT NULL AND a.release_id = b.release_id)
                    AND NOT EXISTS (SELECT 1 FROM recording_link la JOIN recording_link lb ON lb.recording = la.recording
                                     WHERE la.track_id = a.id AND lb.track_id = b.id)
                    AND NOT EXISTS (SELECT 1 FROM recording_apart ra
                                     WHERE (ra.track_id = a.id AND ra.other_track = b.id)
                                        OR (ra.track_id = b.id AND ra.other_track = a.id))
                  ORDER BY a.id, b.id",
            )?;
            let rows = stmt.query_map([track.map(|t| t.to_string())], |r| Ok((side_at(r, 0)?, side_at(r, 7)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// Stored prints of live local files not yet compared with the other
    /// albums (a new print of a file is compared again), the newest first.
    pub fn prints_to_link(&self, limit: usize) -> Result<Vec<PrintToLink>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT p.source_id, s.track_id, p.print, coalesce(p.duration_ms, t.duration_ms)
                   FROM audio_print p
                   JOIN source s ON s.id = p.source_id
                   JOIN track t ON t.id = s.track_id
                  WHERE p.linked_at IS NULL AND p.print IS NOT NULL
                    AND s.kind = 'local_file' AND s.trashed_at IS NULL AND t.deleted_at IS NULL
                  ORDER BY p.at DESC, p.source_id
                  LIMIT ?1",
            )?;
            let rows = stmt.query_map([limit as i64], |r| {
                Ok(PrintToLink {
                    source_id: get_id(r, 0)?,
                    track_id: get_id(r, 1)?,
                    print: print_from_blob(&r.get::<_, Vec<u8>>(2)?),
                    duration_ms: opt_u32(r, 3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    /// The print of `source` was compared with the other albums.
    pub fn mark_print_linked(&self, source: SourceId) -> Result<()> {
        self.write(|tx| {
            tx.execute("UPDATE audio_print SET linked_at = ?2 WHERE source_id = ?1", params![source.to_string(), now_ms()])?;
            Ok(())
        })
    }

    /// Another album has `track`'s recording with a file (a track item of
    /// it needs no download).
    pub fn recording_has_file(&self, track: TrackId) -> Result<bool> {
        self.read(|c| {
            let sql = format!("SELECT {}", crate::masters::has_recording_file("?1"));
            Ok(c.prepare_cached(&sql)?.query_row([track.to_string()], |r| r.get(0))?)
        })
    }

    /// For each linked track, another album its recording is on: the album
    /// and its title (the one with a file first, then the oldest).
    pub fn recording_albums(&self) -> Result<HashMap<TrackId, (ReleaseId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT a.track_id, r.id, r.title FROM recording_link a
                   JOIN track ta ON ta.id = a.track_id
                   JOIN recording_link b ON b.recording = a.recording AND b.track_id != a.track_id
                   JOIN track tb ON tb.id = b.track_id
                   JOIN release r ON r.id = tb.release_id
                  WHERE ta.deleted_at IS NULL AND tb.deleted_at IS NULL
                    AND (ta.release_id IS NULL OR ta.release_id != tb.release_id)
                  ORDER BY a.track_id, tb.has_local DESC, tb.seq",
            )?;
            let rows = stmt.query_map([], |r| Ok((get_id::<TrackId>(r, 0)?, get_id::<ReleaseId>(r, 1)?, r.get::<_, String>(2)?)))?;
            let mut out = HashMap::new();
            for row in rows {
                let (t, r, title) = row?;
                out.entry(t).or_insert((r, title));
            }
            Ok(out)
        })
    }

    /// The lyrics of `track` found once for its recording: when it has none
    /// (or only a "not found"), another track of its recording as long as
    /// it (within a second) gives it its lyrics, the other sources' texts
    /// and the word timing made against the recording. Copies of different
    /// lengths keep their own timing. Returns whether anything came.
    pub fn share_recording_lyrics(&self, track: TrackId) -> Result<bool> {
        self.write(|tx| {
            let t = track.to_string();
            let own: Option<bool> = tx
                .prepare_cached(
                    "SELECT synced IS NULL AND plain IS NULL AND instrumental = 0 FROM lyrics WHERE track_id = ?1",
                )?
                .query_row([&t], |r| r.get(0))
                .optional()?;
            if own == Some(false) {
                return Ok(false);
            }
            let twin: Option<String> = tx
                .prepare_cached(
                    "SELECT b.track_id FROM recording_link a
                       JOIN recording_link b ON b.recording = a.recording AND b.track_id != a.track_id
                       JOIN track ta ON ta.id = a.track_id
                       JOIN track tb ON tb.id = b.track_id
                       JOIN lyrics l ON l.track_id = b.track_id
                      WHERE a.track_id = ?1 AND tb.deleted_at IS NULL
                        AND (l.synced IS NOT NULL OR l.plain IS NOT NULL OR l.instrumental = 1)
                        AND ta.duration_ms IS NOT NULL AND tb.duration_ms IS NOT NULL
                        AND abs(ta.duration_ms - tb.duration_ms) <= 1000
                      ORDER BY l.fetched_at DESC, tb.seq
                      LIMIT 1",
                )?
                .query_row([&t], |r| r.get(0))
                .optional()?;
            let Some(twin) = twin else { return Ok(false) };
            tx.execute(
                "INSERT INTO lyrics(track_id, synced, plain, instrumental, source, fetched_at)
                 SELECT ?1, synced, plain, instrumental, source, fetched_at FROM lyrics WHERE track_id = ?2
                 ON CONFLICT(track_id) DO UPDATE SET synced = excluded.synced, plain = excluded.plain,
                    instrumental = excluded.instrumental, source = excluded.source, fetched_at = excluded.fetched_at",
                params![t, twin],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO lyrics_text(track_id, source, body, fetched_at)
                 SELECT ?1, source, body, fetched_at FROM lyrics_text WHERE track_id = ?2 AND body IS NOT NULL",
                params![t, twin],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO lyrics_align(track_id, source, text_hash, fit, timing, offset_ms, covered, body,
                                                    made_by, made_at, saved)
                 SELECT ?1, source, text_hash, fit, timing, offset_ms, covered, body, made_by, made_at, saved
                   FROM lyrics_align WHERE track_id = ?2",
                params![t, twin],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO lyrics_align_done(track_id, state, detail, made_by, at)
                 SELECT ?1, state, detail, made_by, at FROM lyrics_align_done WHERE track_id = ?2",
                params![t, twin],
            )?;
            Ok(true)
        })
    }
}

/// Every link and every «Разные песни», for the export.
pub(crate) fn read_recordings(c: &Connection) -> Result<(Vec<RecordingLink>, Vec<RecordingApart>)> {
    let mut stmt = c.prepare_cached("SELECT track_id, recording, how, at FROM recording_link ORDER BY recording, track_id")?;
    let links = stmt
        .query_map([], |r| Ok(RecordingLink { track_id: get_id(r, 0)?, recording: r.get(1)?, how: r.get(2)?, at: r.get(3)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = c.prepare_cached("SELECT track_id, other_track, at FROM recording_apart ORDER BY track_id, other_track")?;
    let apart = stmt
        .query_map([], |r| Ok(RecordingApart { track_id: get_id(r, 0)?, other_track: get_id(r, 1)?, at: r.get(2)? }))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok((links, apart))
}

/// Merges exported links: the «Разные песни» first, then each exported
/// recording's tracks are linked here (the groups of two devices join;
/// what one said apart stays apart). Returns how many links were new.
pub(crate) fn merge_recordings_in(c: &Connection, links: &[RecordingLink], apart: &[RecordingApart], device: &str) -> Result<usize> {
    for p in apart {
        put_apart_in(c, &p.track_id.to_string(), &p.other_track.to_string(), p.at)?;
    }
    let mut groups: HashMap<&str, Vec<&RecordingLink>> = HashMap::new();
    for l in links {
        groups.entry(l.recording.as_str()).or_default().push(l);
    }
    let known = |t: &str| -> Result<bool> {
        Ok(c.prepare_cached("SELECT EXISTS(SELECT 1 FROM track WHERE id = ?1)")?.query_row([t], |r| r.get(0))?)
    };
    let mut n = 0;
    for list in groups.values() {
        let ids: Vec<(String, &RecordingLink)> = list.iter().map(|l| (l.track_id.to_string(), *l)).collect();
        let ids: Vec<(String, &RecordingLink)> = {
            let mut kept = Vec::new();
            for (id, l) in ids {
                if known(&id)? {
                    kept.push((id, l));
                }
            }
            kept
        };
        let Some((first, _)) = ids.first() else { continue };
        for (id, l) in ids.iter().skip(1) {
            if link_in(c, first, id, &l.how, l.at, device)? {
                n += 1;
            }
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{ArtistCredit, Codec, EventContext, EventType, LikeTarget, Release, Source, SourceKind, Track};

    use super::*;

    fn album(lib: &Library, title: &str) -> ReleaseId {
        lib.upsert_release(&Release::new(title, ArtistCredit::single(String::from("Joe Hisaishi"), None))).unwrap().id
    }

    /// A track of `release` with a file (`file`) and an ISRC.
    fn song(lib: &Library, title: &str, release: Option<ReleaseId>, isrc: Option<&str>, ms: u32, file: bool) -> TrackId {
        let mut t = Track::new(title, ArtistCredit::single(String::from("Joe Hisaishi"), None));
        t.release_id = release;
        t.isrc = isrc.map(str::to_owned);
        t.duration_ms = Some(ms);
        let t = lib.upsert_track(&t).unwrap();
        if file {
            let s = Source::new(t.id, SourceKind::LocalFile, format!("/m/{}/{title}.opus", t.id), Codec::Opus);
            lib.add_source(&s).unwrap();
        }
        t.id
    }

    /// The same ISRC on two albums is one pair; one album's two tracks, or
    /// a pair said to be different songs, are none.
    #[test]
    fn the_same_isrc_on_two_albums_is_a_pair() {
        let lib = Library::open_in_memory().unwrap();
        let (ost, special) = (album(&lib, "OST"), album(&lib, "Special Edition"));
        let a = song(&lib, "Oración", Some(ost), Some("JPA123"), 276_900, true);
        let b = song(&lib, "Oración", Some(special), Some("JPA123"), 277_600, true);
        // One album's two tracks with one ISRC (a tagging slip): no pair.
        song(&lib, "Main Theme", Some(ost), Some("JPA999"), 100_000, true);
        song(&lib, "Main Theme (reprise)", Some(ost), Some("JPA999"), 100_000, true);
        let pairs = lib.recording_id_pairs(None).unwrap();
        assert_eq!(pairs.len(), 1, "{pairs:?}");
        let ids = [pairs[0].0.track_id, pairs[0].1.track_id];
        assert!(ids.contains(&a) && ids.contains(&b));
        assert_eq!(lib.recording_id_pairs(Some(a)).unwrap().len(), 1);
        assert!(lib.link_recording(a, b, "ids").unwrap());
        assert!(lib.recording_id_pairs(None).unwrap().is_empty(), "one recording already");
        assert!(lib.same_recording(a, b).unwrap());
        assert_eq!(lib.recording_twins(a).unwrap(), vec![b]);
        assert!(!lib.link_recording(b, a, "ids").unwrap(), "nothing changes");
        // Said to be different songs: never paired nor linked again.
        assert_eq!(lib.unlink_recording(a).unwrap(), vec![b]);
        assert!(!lib.same_recording(a, b).unwrap());
        assert!(lib.recording_apart(b, a).unwrap());
        assert!(lib.recording_id_pairs(None).unwrap().is_empty());
        assert!(!lib.link_recording(a, b, "audio").unwrap());
    }

    /// One ♡ for the recording: liking either likes both, unliking clears
    /// both, and a link of a liked track likes the other.
    #[test]
    fn a_like_of_one_copy_is_a_like_of_the_recording() {
        let lib = Library::open_in_memory().unwrap();
        let (ost, special) = (album(&lib, "OST"), album(&lib, "Special Edition"));
        let a = song(&lib, "Oración", Some(ost), None, 276_900, true);
        let b = song(&lib, "オラシオン", Some(special), None, 277_600, true);
        let c = song(&lib, "Oración", None, None, 276_900, false);
        lib.like(a).unwrap();
        assert!(lib.link_recording(a, b, "user").unwrap());
        assert!(lib.is_liked(b).unwrap(), "linked while liked: both liked");
        lib.unlike(b).unwrap();
        assert!(!lib.is_liked(a).unwrap() && !lib.is_liked(b).unwrap(), "unlike clears both");
        lib.like(LikeTarget::Track(b)).unwrap();
        assert!(lib.is_liked(a).unwrap());
        // «Любимые» shows the recording once: the copy with a file on the
        // album played more.
        assert_eq!(lib.liked_count().unwrap(), 1);
        let rows = lib.liked_tracks(0, 10).unwrap();
        assert_eq!(rows.len(), 1);
        for _ in 0..3 {
            lib.record_event(b, EventType::Start, None, Some(0), EventContext::Album).unwrap();
        }
        assert_eq!(lib.liked_tracks(0, 10).unwrap()[0].id, b, "the album played more");
        assert_eq!(lib.recording_spares().unwrap().get(&a), Some(&b));
        // Plays of either count for the recording once.
        lib.record_event(a, EventType::Start, None, Some(0), EventContext::Album).unwrap();
        assert_eq!(lib.track_stats(a).unwrap().plays, 4);
        assert_eq!(lib.track_stats(b).unwrap().plays, 4);
        let all = lib.all_track_stats().unwrap();
        assert_eq!(all.len(), 1, "one row for the recording: {all:?}");
        assert_eq!((all[0].0, all[0].1.plays), (b, 4));
        // A third copy without a file joins the recording's like.
        assert!(lib.link_recording(c, a, "ids").unwrap());
        assert!(lib.is_liked(c).unwrap());
        assert_eq!(lib.recording_albums().unwrap().get(&a).map(|x| x.1.as_str()), Some("Special Edition"));
    }

    /// The search shows one row of the recording.
    #[test]
    fn the_search_shows_one_row_of_a_recording() {
        let lib = Library::open_in_memory().unwrap();
        let (ost, special) = (album(&lib, "OST"), album(&lib, "Special Edition"));
        let a = song(&lib, "Oración", Some(ost), None, 276_900, true);
        let b = song(&lib, "Oración", Some(special), None, 277_600, true);
        assert_eq!(lib.search("Oración", 10).unwrap().tracks.len(), 2);
        lib.link_recording(a, b, "audio").unwrap();
        assert_eq!(lib.search("Oración", 10).unwrap().tracks.len(), 1);
    }

    /// A track item of a song the library has on another album is done;
    /// the recording's lyrics are found once when the lengths agree.
    #[test]
    fn a_wish_and_the_lyrics_follow_the_recording() {
        let lib = Library::open_in_memory().unwrap();
        let (ost, special) = (album(&lib, "OST"), album(&lib, "Special Edition"));
        let a = song(&lib, "Oración", Some(ost), Some("JPA123"), 276_900, true);
        let b = song(&lib, "Oración", Some(special), Some("JPA123"), 277_600, false);
        let far = song(&lib, "Oración", None, None, 290_000, false);
        assert!(!lib.recording_has_file(b).unwrap());
        lib.link_recording(a, b, "ids").unwrap();
        lib.link_recording(a, far, "user").unwrap();
        assert!(lib.recording_has_file(b).unwrap());
        assert!(!lib.recording_has_file(a).unwrap(), "only another track's file counts");
        lib.put_lyrics(a, Some("[00:01.00]Oración"), None, false).unwrap();
        assert!(lib.share_recording_lyrics(b).unwrap());
        assert_eq!(lib.lyrics(b).unwrap().unwrap().synced.as_deref(), Some("[00:01.00]Oración"));
        assert!(!lib.share_recording_lyrics(b).unwrap(), "it has its own now");
        assert!(!lib.share_recording_lyrics(far).unwrap(), "13 s longer: its own timing");
    }

    /// The links and the «Разные песни» go through the export.
    #[test]
    fn links_survive_an_export_and_an_import() {
        let lib = Library::open_in_memory().unwrap();
        let (ost, special) = (album(&lib, "OST"), album(&lib, "Special Edition"));
        let a = song(&lib, "Oración", Some(ost), None, 276_900, true);
        let b = song(&lib, "Oración", Some(special), None, 277_600, true);
        let c = song(&lib, "Other", Some(special), None, 100_000, true);
        lib.link_recording(a, b, "audio").unwrap();
        lib.set_recording_apart(a, c).unwrap();
        let (links, apart) = lib.read(read_recordings).unwrap();
        assert_eq!((links.len(), apart.len()), (2, 1));
        let other = Library::open_in_memory().unwrap();
        // The same tracks on another device.
        for t in [a, b, c] {
            let mut track = lib.get_track(t).unwrap().unwrap();
            track.release_id = None;
            other.upsert_track(&track).unwrap();
        }
        let n = other.write(|tx| merge_recordings_in(tx, &links, &apart, "other")).unwrap();
        assert_eq!(n, 1);
        assert!(other.same_recording(a, b).unwrap());
        assert!(other.recording_apart(c, a).unwrap());
    }
}
