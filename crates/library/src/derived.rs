//! Derived per-track / per-release data: analysis vectors and palettes, and
//! the «не рекомендовать» feedback of the recommendations.

use nikgob_core::model::{Analysis, AnalysisSource, Palette, ReleaseId, TrackId, UnixMs};
use rusqlite::{OptionalExtension, params};

use crate::db::Library;
use crate::error::{LibraryError, Result};
use crate::types::{AnalysisCounts, AnalysisTodo, RecFeedback, StreamAnalysisTodo};
use crate::util::{get_id, now_ms};

fn encode_vector(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return Err(LibraryError::Invalid(
            "analysis vector blob length is not a multiple of 4".into(),
        ));
    }
    let (chunks, _) = bytes.as_chunks::<4>();
    Ok(chunks.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

/// `track t` has a local file; `s` is its active local source. The key of
/// that file for [`Library::mark_analysis_failed`].
const FILE_KEY: &str = "s.id || ':' || coalesce(s.file_hash, '') || ':' || coalesce(s.size_bytes, '')";

/// `track t` (alias) is a stream-only track the background stream analysis
/// takes: no file, a live stream source, liked or in a live playlist.
pub(crate) const STREAM_ANALYSABLE: &str = "t.has_local = 0 AND t.deleted_at IS NULL
    AND EXISTS (SELECT 1 FROM source s WHERE s.track_id = t.id AND s.kind <> 'local_file'
                   AND s.trashed_at IS NULL)
    AND (EXISTS (SELECT 1 FROM likes l WHERE l.target_kind = 'track' AND l.target_id = t.id
                    AND l.deleted = 0)
         OR EXISTS (SELECT 1 FROM playlist_entry e JOIN playlist p ON p.id = e.playlist_id
                     WHERE e.track_id = t.id AND e.deleted = 0 AND p.deleted = 0))";

/// `analysis_failed.file_key` of a stream-only track whose analysis failed
/// (no song on YouTube Music, a video that does not play or decode): tried
/// again after a while ([`Library::streams_needing_analysis`]); a file that
/// arrives is analysed regardless.
const STREAM_FAILED: &str = "stream";

fn analysis_from_row(r: &rusqlite::Row<'_>, track: TrackId, at: usize) -> Result<Analysis> {
    let blob: Vec<u8> = r.get(at)?;
    let source: String = r.get(at + 2)?;
    Ok(Analysis {
        track_id: track,
        bliss_vector: decode_vector(&blob)?,
        analyzed_with: r.get(at + 1)?,
        source: AnalysisSource::parse(&source)
            .ok_or_else(|| LibraryError::Invalid(format!("analysis source {source:?}")))?,
    })
}

impl Library {
    /// Stores a track's sound analysis. An analysis of a stream never
    /// replaces one of the file with the same `analyzed_with` (the file's
    /// always wins); returns whether it was stored. A file's analysis also
    /// clears its failure mark.
    pub fn put_analysis(&self, analysis: &Analysis) -> Result<bool> {
        self.write(|tx| {
            let tid = analysis.track_id.to_string();
            let stored = tx.execute(
                "INSERT INTO analysis(track_id, bliss_vector, analyzed_with, analyzed_at, source)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(track_id) DO UPDATE SET bliss_vector = excluded.bliss_vector,
                    analyzed_with = excluded.analyzed_with, analyzed_at = excluded.analyzed_at,
                    source = excluded.source
                  WHERE NOT (analysis.source = 'file' AND excluded.source <> 'file'
                             AND analysis.analyzed_with = excluded.analyzed_with)",
                params![
                    tid,
                    encode_vector(&analysis.bliss_vector),
                    analysis.analyzed_with,
                    now_ms(),
                    analysis.source.as_str()
                ],
            )? > 0;
            if analysis.source == AnalysisSource::File {
                tx.execute("DELETE FROM analysis_failed WHERE track_id = ?1", [&tid])?;
            }
            Ok(stored)
        })
    }

    pub fn get_analysis(&self, track: TrackId) -> Result<Option<Analysis>> {
        self.read(|c| {
            let mut stmt =
                c.prepare_cached("SELECT bliss_vector, analyzed_with, source FROM analysis WHERE track_id = ?1")?;
            let mut rows = stmt.query([track.to_string()])?;
            rows.next()?.map(|r| analysis_from_row(r, track, 0)).transpose()
        })
    }

    /// Every vector computed with `analyzed_with` (older ones are not
    /// comparable), of tracks that are not deleted: what the runtime's
    /// in-memory index starts from.
    pub fn analysis_vectors(&self, analyzed_with: &str) -> Result<Vec<Analysis>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT a.track_id, a.bliss_vector, a.analyzed_with, a.source FROM analysis a
                   JOIN track t ON t.id = a.track_id
                  WHERE a.analyzed_with = ?1 AND t.deleted_at IS NULL",
            )?;
            let mut rows = stmt.query([analyzed_with])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push(analysis_from_row(r, get_id(r, 0)?, 1)?);
            }
            Ok(out)
        })
    }

    /// Tracks with a local file whose analysis is due (work queue for the
    /// analysis job): none yet, one of another `analyzed_with`, or one of a
    /// stream. Never analysed first, newest first; a file that failed to
    /// decode is left out until it changes.
    pub fn tracks_needing_analysis(&self, analyzed_with: &str, limit: u32) -> Result<Vec<AnalysisTodo>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT t.id, s.id, s.locator, {FILE_KEY} FROM track t
                   JOIN source s ON s.track_id = t.id AND s.is_active = 1 AND s.kind = 'local_file'
                   LEFT JOIN analysis a ON a.track_id = t.id
                   LEFT JOIN analysis_failed f ON f.track_id = t.id
                  WHERE t.has_local = 1 AND t.deleted_at IS NULL
                    AND (a.track_id IS NULL OR a.analyzed_with <> ?1 OR a.source <> 'file')
                    AND (f.track_id IS NULL OR f.file_key <> {FILE_KEY})
                  ORDER BY a.track_id IS NOT NULL, t.seq DESC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![analyzed_with, limit], |r| {
                Ok(AnalysisTodo {
                    track_id: get_id(r, 0)?,
                    source_id: get_id(r, 1)?,
                    locator: r.get(2)?,
                    file_key: r.get(3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Remembers that this file of the track could not be analysed: it is
    /// not queued again until the track's file changes.
    pub fn mark_analysis_failed(&self, todo: &AnalysisTodo, error: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO analysis_failed(track_id, file_key, error, failed_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(track_id) DO UPDATE SET file_key = excluded.file_key,
                    error = excluded.error, failed_at = excluded.failed_at",
                params![todo.track_id.to_string(), todo.file_key, error, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Stream-only tracks that are liked or in a playlist and have no
    /// analysis with `analyzed_with` (work queue of the background stream
    /// analysis), newest first; one whose stream analysis failed at or
    /// after `retry_before` is left out.
    pub fn streams_needing_analysis(
        &self,
        analyzed_with: &str,
        retry_before: UnixMs,
        limit: u32,
    ) -> Result<Vec<StreamAnalysisTodo>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT t.id,
                        (SELECT y.locator FROM source y WHERE y.track_id = t.id AND y.kind = 'youtube'
                            AND y.available = 1 AND y.trashed_at IS NULL
                          ORDER BY y.is_active DESC, y.quality_score DESC LIMIT 1)
                   FROM track t
                   LEFT JOIN analysis a ON a.track_id = t.id
                   LEFT JOIN analysis_failed f ON f.track_id = t.id
                  WHERE {STREAM_ANALYSABLE}
                    AND (a.track_id IS NULL OR a.analyzed_with <> ?1)
                    AND (f.track_id IS NULL OR f.file_key <> '{STREAM_FAILED}' OR f.failed_at < ?2)
                  ORDER BY t.seq DESC LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![analyzed_with, retry_before, limit], |r| {
                Ok(StreamAnalysisTodo {
                    track_id: get_id(r, 0)?,
                    youtube: r.get(1)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Remembers that the stream analysis of this stream-only track failed:
    /// it leaves the queue (and the progress) until the mark is older than
    /// the `retry_before` the queue is asked with, or a file arrives.
    pub fn mark_stream_analysis_failed(&self, track: TrackId, error: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO analysis_failed(track_id, file_key, error, failed_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(track_id) DO UPDATE SET file_key = excluded.file_key,
                    error = excluded.error, failed_at = excluded.failed_at",
                params![track.to_string(), STREAM_FAILED, error, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Progress of the analysis with `analyzed_with`: files, and the
    /// stream-only tracks that are liked or in a playlist (without those
    /// whose stream analysis failed).
    pub fn analysis_counts(&self, analyzed_with: &str) -> Result<AnalysisCounts> {
        self.read(|c| {
            let (files_total, files_done): (u32, u32) = c.query_row(
                &format!(
                    "SELECT count(*), coalesce(sum(a.analyzed_with = ?1 AND a.source = 'file'), 0)
                       FROM track t
                       JOIN source s ON s.track_id = t.id AND s.is_active = 1 AND s.kind = 'local_file'
                       LEFT JOIN analysis a ON a.track_id = t.id
                       LEFT JOIN analysis_failed f ON f.track_id = t.id
                      WHERE t.has_local = 1 AND t.deleted_at IS NULL
                        AND (f.track_id IS NULL OR f.file_key <> {FILE_KEY})"
                ),
                [analyzed_with],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let (streams_total, streams_done): (u32, u32) = c.query_row(
                &format!(
                    "SELECT count(*), coalesce(sum(a.analyzed_with = ?1), 0)
                       FROM track t LEFT JOIN analysis a ON a.track_id = t.id
                       LEFT JOIN analysis_failed f ON f.track_id = t.id
                      WHERE {STREAM_ANALYSABLE}
                        AND (f.track_id IS NULL OR f.file_key <> '{STREAM_FAILED}'
                             OR a.analyzed_with IS ?1)"
                ),
                [analyzed_with],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(AnalysisCounts {
                files_done,
                files_total,
                streams_done,
                streams_total,
            })
        })
    }

    // ---- The neural sound model's vectors (docs/recommendations.md §5)

    /// Stores a track's sound-model vector (`vector`: its 260 bytes; `None`:
    /// the model cannot make one of this audio, not tried again with this
    /// `embedded_with`). A stream's never replaces a file's of the same
    /// model; returns whether it was stored.
    pub fn put_embedding(&self, track: TrackId, vector: Option<&[u8]>, embedded_with: &str, source: AnalysisSource) -> Result<bool> {
        self.write(|tx| {
            Ok(tx.execute(
                "INSERT INTO sound_embedding(track_id, vector, embedded_with, source, embedded_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(track_id) DO UPDATE SET vector = excluded.vector,
                    embedded_with = excluded.embedded_with, source = excluded.source,
                    embedded_at = excluded.embedded_at
                  WHERE NOT (sound_embedding.source = 'file' AND excluded.source <> 'file'
                             AND sound_embedding.embedded_with = excluded.embedded_with
                             AND sound_embedding.vector IS NOT NULL)",
                params![track.to_string(), vector, embedded_with, source.as_str(), now_ms()],
            )? > 0)
        })
    }

    /// The track has a row of `embedded_with` (a vector, or the mark that
    /// it cannot have one).
    pub fn has_embedding(&self, track: TrackId, embedded_with: &str) -> Result<bool> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT 1 FROM sound_embedding WHERE track_id = ?1 AND embedded_with = ?2",
                params![track.to_string(), embedded_with],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
        })
    }

    /// Every vector of `embedded_with` of tracks that are not deleted: what
    /// the in-memory index takes at start.
    pub fn embedding_vectors(&self, embedded_with: &str) -> Result<Vec<(TrackId, Vec<u8>)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT e.track_id, e.vector FROM sound_embedding e JOIN track t ON t.id = e.track_id
                  WHERE e.embedded_with = ?1 AND e.vector IS NOT NULL AND t.deleted_at IS NULL",
            )?;
            let mut rows = stmt.query([embedded_with])?;
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                out.push((get_id(r, 0)?, r.get(1)?));
            }
            Ok(out)
        })
    }

    /// Tracks with a file whose bliss analysis (`analyzed_with`, from the
    /// file) is current but whose sound-model vector of `embedded_with` is
    /// missing, of another model or a stream's: the model's own queue once
    /// bliss's is done. Newest first.
    pub fn tracks_needing_embedding(&self, embedded_with: &str, analyzed_with: &str, limit: u32) -> Result<Vec<AnalysisTodo>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT t.id, s.id, s.locator, {FILE_KEY} FROM track t
                   JOIN source s ON s.track_id = t.id AND s.is_active = 1 AND s.kind = 'local_file'
                   JOIN analysis a ON a.track_id = t.id AND a.analyzed_with = ?2 AND a.source = 'file'
                   LEFT JOIN sound_embedding e ON e.track_id = t.id
                  WHERE t.has_local = 1 AND t.deleted_at IS NULL
                    AND (e.track_id IS NULL OR e.embedded_with <> ?1 OR e.source <> 'file')
                  ORDER BY t.seq DESC LIMIT ?3"
            ))?;
            let rows = stmt.query_map(params![embedded_with, analyzed_with, limit], |r| {
                Ok(AnalysisTodo { track_id: get_id(r, 0)?, source_id: get_id(r, 1)?, locator: r.get(2)?, file_key: r.get(3)? })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Files with a sound-model vector of `embedded_with` (or its mark) and
    /// files in all, as [`Self::analysis_counts`] counts them.
    pub fn embedding_counts(&self, embedded_with: &str) -> Result<(u32, u32)> {
        self.read(|c| {
            Ok(c.query_row(
                &format!(
                    "SELECT coalesce(sum(e.embedded_with = ?1 AND e.source = 'file'), 0), count(*)
                       FROM track t
                       JOIN source s ON s.track_id = t.id AND s.is_active = 1 AND s.kind = 'local_file'
                       LEFT JOIN sound_embedding e ON e.track_id = t.id
                       LEFT JOIN analysis_failed f ON f.track_id = t.id
                      WHERE t.has_local = 1 AND t.deleted_at IS NULL
                        AND (f.track_id IS NULL OR f.file_key <> {FILE_KEY})"
                ),
                [embedded_with],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
    }

    // ---- «Не рекомендовать»

    /// Marks (`dislike`) or clears «не рекомендовать» for a target
    /// (`track` / `artist` and its id, or whatever kind the recommender
    /// names). Last writer wins, like likes.
    pub fn set_rec_feedback(&self, target_kind: &str, target_id: &str, dislike: bool) -> Result<()> {
        let now = self.tick();
        self.write(|tx| {
            tx.execute(
                "INSERT INTO rec_feedback(target_kind, target_id, dislike, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(target_kind, target_id) DO UPDATE SET dislike = excluded.dislike,
                    updated_at = excluded.updated_at
                  WHERE excluded.updated_at >= rec_feedback.updated_at",
                params![target_kind, target_id, dislike, now],
            )?;
            Ok(())
        })
    }

    pub fn is_rec_disliked(&self, target_kind: &str, target_id: &str) -> Result<bool> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM rec_feedback
                                WHERE target_kind = ?1 AND target_id = ?2 AND dislike = 1)",
                params![target_kind, target_id],
                |r| r.get(0),
            )?)
        })
    }

    /// Everything marked «не рекомендовать», newest first.
    pub fn rec_dislikes(&self) -> Result<Vec<RecFeedback>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT target_kind, target_id, dislike, updated_at FROM rec_feedback
                  WHERE dislike = 1 ORDER BY updated_at DESC",
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
        })
    }

    pub fn put_palette(&self, palette: &Palette) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO palette(release_id, seed_argb, scheme_blob, ambient_blur, computed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(release_id) DO UPDATE SET seed_argb = excluded.seed_argb,
                    scheme_blob = excluded.scheme_blob, ambient_blur = excluded.ambient_blur,
                    computed_at = excluded.computed_at",
                params![
                    palette.release_id.to_string(),
                    i64::from(palette.seed_argb),
                    palette.scheme_blob,
                    palette.ambient_blur,
                    now_ms()
                ],
            )?;
            Ok(())
        })
    }

    /// The palette seeds of every release that has one (cards in bulk).
    pub fn palette_seeds(&self) -> Result<std::collections::HashMap<ReleaseId, u32>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached("SELECT release_id, seed_argb FROM palette")?;
            let rows = stmt.query_map([], |r| Ok((get_id(r, 0)?, r.get::<_, i64>(1)? as u32)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
    }

    pub fn get_palette(&self, release: ReleaseId) -> Result<Option<Palette>> {
        self.read(|c| {
            Ok(c
                .prepare_cached("SELECT seed_argb, scheme_blob, ambient_blur FROM palette WHERE release_id = ?1")?
                .query_row([release.to_string()], |r| {
                    Ok(Palette {
                        release_id: release,
                        seed_argb: r.get::<_, i64>(0)? as u32,
                        scheme_blob: r.get(1)?,
                        ambient_blur: r.get(2)?,
                    })
                })
                .optional()?)
        })
    }

    /// Releases with a cover file but no palette yet (work queue; a stream
    /// cover url is not read).
    pub fn releases_needing_palette(&self, limit: u32) -> Result<Vec<ReleaseId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT r.id FROM release r LEFT JOIN palette p ON p.release_id = r.id
                  WHERE p.release_id IS NULL AND r.cover_ref IS NOT NULL
                    AND r.cover_ref NOT LIKE 'https://%' AND r.cover_ref NOT LIKE 'http://%'
                  ORDER BY r.seq LIMIT ?1",
            )?;
            let ids = stmt.query_map([limit], |r| get_id(r, 0))?;
            Ok(ids.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}
