//! Derived per-track / per-release data: analysis vectors and palettes.

use nikgob_core::model::{Analysis, Palette, ReleaseId, TrackId};
use rusqlite::{OptionalExtension, params};

use crate::db::Library;
use crate::error::{LibraryError, Result};
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

impl Library {
    pub fn put_analysis(&self, analysis: &Analysis) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO analysis(track_id, bliss_vector, analyzed_with, analyzed_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(track_id) DO UPDATE SET bliss_vector = excluded.bliss_vector,
                    analyzed_with = excluded.analyzed_with, analyzed_at = excluded.analyzed_at",
                params![
                    analysis.track_id.to_string(),
                    encode_vector(&analysis.bliss_vector),
                    analysis.analyzed_with,
                    now_ms()
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_analysis(&self, track: TrackId) -> Result<Option<Analysis>> {
        self.read(|c| {
            let row: Option<(Vec<u8>, String)> = c
                .query_row(
                    "SELECT bliss_vector, analyzed_with FROM analysis WHERE track_id = ?1",
                    [track.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            row.map(|(blob, analyzed_with)| {
                Ok(Analysis {
                    track_id: track,
                    bliss_vector: decode_vector(&blob)?,
                    analyzed_with,
                })
            })
            .transpose()
        })
    }

    /// Local tracks that have no analysis with this `analyzed_with` tag yet
    /// (work queue for the analysis job).
    pub fn tracks_needing_analysis(&self, analyzed_with: &str, limit: u32) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT t.id FROM track t
                  LEFT JOIN analysis a ON a.track_id = t.id AND a.analyzed_with = ?1
                  WHERE t.has_local = 1 AND a.track_id IS NULL ORDER BY t.seq LIMIT ?2",
            )?;
            let ids = stmt.query_map(params![analyzed_with, limit], |r| get_id(r, 0))?;
            Ok(ids.collect::<rusqlite::Result<Vec<_>>>()?)
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

    pub fn get_palette(&self, release: ReleaseId) -> Result<Option<Palette>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT seed_argb, scheme_blob, ambient_blur FROM palette WHERE release_id = ?1",
                [release.to_string()],
                |r| {
                    Ok(Palette {
                        release_id: release,
                        seed_argb: r.get::<_, i64>(0)? as u32,
                        scheme_blob: r.get(1)?,
                        ambient_blur: r.get(2)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Releases with a cover file but no palette yet (work queue; a stream
    /// cover url is not read).
    pub fn releases_needing_palette(&self, limit: u32) -> Result<Vec<ReleaseId>> {
        self.read(|c| {
            let mut stmt = c.prepare(
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
