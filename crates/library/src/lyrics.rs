//! Lyrics cache: what LRCLIB answered for a track, including "nothing found".

use nikgob_core::model::{TrackId, UnixMs};
use rusqlite::{OptionalExtension, params};

use crate::db::Library;
use crate::error::Result;
use crate::util::{get_id, now_ms};

/// One cached lookup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoredLyrics {
    /// Raw LRC text.
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub instrumental: bool,
    pub fetched_at: UnixMs,
}

impl StoredLyrics {
    /// The lookup found nothing (a "not found" marker).
    pub fn is_miss(&self) -> bool {
        self.synced.is_none() && self.plain.is_none() && !self.instrumental
    }
}

/// What the player made of one text against the recording (`lyrics_align`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlignedLyrics {
    pub source: String,
    pub text_hash: String,
    pub fit: f32,
    /// `none` | `fits` | `shifted` | `elsewhere`.
    pub timing: String,
    pub offset_ms: i64,
    pub covered: f32,
    /// JSON lines with the made word timing.
    pub body: Option<String>,
    pub made_by: String,
    pub made_at: UnixMs,
    pub saved: bool,
}

/// How far the background alignment pass is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AlignCounts {
    pub done: u32,
    pub total: u32,
}

impl Library {
    pub fn lyrics(&self, track: TrackId) -> Result<Option<StoredLyrics>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT synced, plain, instrumental, fetched_at FROM lyrics WHERE track_id = ?1",
                [track.to_string()],
                |r| {
                    Ok(StoredLyrics {
                        synced: r.get(0)?,
                        plain: r.get(1)?,
                        instrumental: r.get::<_, i64>(2)? != 0,
                        fetched_at: r.get(3)?,
                    })
                },
            )
            .optional()?)
        })
    }

    /// Stores (or replaces) the result of a lookup; blank strings count as absent.
    /// With nothing to store this records the "not found" marker.
    pub fn put_lyrics(
        &self,
        track: TrackId,
        synced: Option<&str>,
        plain: Option<&str>,
        instrumental: bool,
    ) -> Result<()> {
        let keep = |s: Option<&str>| s.filter(|s| !s.trim().is_empty()).map(str::to_owned);
        let (synced, plain) = (keep(synced), keep(plain));
        self.write(|tx| {
            tx.execute(
                "INSERT INTO lyrics(track_id, synced, plain, instrumental, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(track_id) DO UPDATE SET synced = excluded.synced,
                    plain = excluded.plain, instrumental = excluded.instrumental,
                    fetched_at = excluded.fetched_at",
                params![track.to_string(), synced, plain, instrumental, now_ms()],
            )?;
            Ok(())
        })
    }

    /// A text from another source (`source` is a `LyricsSource::key`):
    /// `Some((body, fetched_at))` once looked up, body `None` = nothing found.
    pub fn lyrics_text(&self, track: TrackId, source: &str) -> Result<Option<(Option<String>, UnixMs)>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT body, fetched_at FROM lyrics_text WHERE track_id = ?1 AND source = ?2",
                params![track.to_string(), source],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
        })
    }

    /// Stores a looked-up text; `None` (or blank) records "not found".
    pub fn put_lyrics_text(&self, track: TrackId, source: &str, body: Option<&str>) -> Result<()> {
        let body = body.filter(|s| !s.trim().is_empty());
        self.write(|tx| {
            tx.execute(
                "INSERT INTO lyrics_text(track_id, source, body, fetched_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(track_id, source) DO UPDATE SET body = excluded.body,
                    fetched_at = excluded.fetched_at",
                params![track.to_string(), source, body, now_ms()],
            )?;
            Ok(())
        })
    }

    /// The lyrics source the user picked for a track, if any.
    pub fn lyrics_pick(&self, track: TrackId) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT source FROM lyrics_pick WHERE track_id = ?1",
                [track.to_string()],
                |r| r.get(0),
            )
            .optional()?)
        })
    }

    /// Remembers the picked source; `None` goes back to the automatic choice.
    pub fn set_lyrics_pick(&self, track: TrackId, source: Option<&str>) -> Result<()> {
        self.write(|tx| {
            match source {
                Some(s) => tx.execute(
                    "INSERT INTO lyrics_pick(track_id, source) VALUES (?1, ?2)
                     ON CONFLICT(track_id) DO UPDATE SET source = excluded.source",
                    params![track.to_string(), s],
                )?,
                None => tx.execute("DELETE FROM lyrics_pick WHERE track_id = ?1", [track.to_string()])?,
            };
            Ok(())
        })
    }

    /// How the user chose to see the track's lyrics (`word`, `line` or
    /// `plain`), if they did.
    pub fn lyrics_show(&self, track: TrackId) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.query_row("SELECT sync FROM lyrics_show WHERE track_id = ?1", [track.to_string()], |r| r.get(0))
                .optional()?)
        })
    }

    /// Remembers how the track's lyrics are seen; `None` forgets it.
    pub fn set_lyrics_show(&self, track: TrackId, sync: Option<&str>) -> Result<()> {
        self.write(|tx| {
            match sync {
                Some(s) => tx.execute(
                    "INSERT INTO lyrics_show(track_id, sync) VALUES (?1, ?2)
                     ON CONFLICT(track_id) DO UPDATE SET sync = excluded.sync",
                    params![track.to_string(), s],
                )?,
                None => tx.execute("DELETE FROM lyrics_show WHERE track_id = ?1", [track.to_string()])?,
            };
            Ok(())
        })
    }

    /// Local tracks without lyrics that are worth a lookup: never tried, or
    /// "not found" markers older than `retry_before` (unix ms). Untried first.
    pub fn tracks_needing_lyrics(&self, retry_before: UnixMs, limit: u32) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT t.id FROM track t LEFT JOIN lyrics l ON l.track_id = t.id
                  WHERE t.has_local = 1
                    AND (l.track_id IS NULL
                         OR (l.synced IS NULL AND l.plain IS NULL AND l.instrumental = 0
                             AND l.fetched_at < ?1))
                  ORDER BY l.fetched_at IS NOT NULL, l.fetched_at, t.seq
                  LIMIT ?2",
            )?;
            let ids = stmt.query_map(params![retry_before, limit], |r| get_id(r, 0))?;
            Ok(ids.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

impl Library {
    /// Every text of a track the player has fitted to the recording.
    pub fn lyrics_aligned(&self, track: TrackId) -> Result<Vec<AlignedLyrics>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT source, text_hash, fit, timing, offset_ms, covered, body, made_by, made_at, saved
                   FROM lyrics_align WHERE track_id = ?1 ORDER BY source",
            )?;
            let rows = stmt.query_map([track.to_string()], |r| {
                Ok(AlignedLyrics {
                    source: r.get(0)?,
                    text_hash: r.get(1)?,
                    fit: r.get::<_, f64>(2)? as f32,
                    timing: r.get(3)?,
                    offset_ms: r.get(4)?,
                    covered: r.get::<_, f64>(5)? as f32,
                    body: r.get(6)?,
                    made_by: r.get(7)?,
                    made_at: r.get(8)?,
                    saved: r.get::<_, i64>(9)? != 0,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Stores (or replaces) what was made of one text. A text the user saved
    /// is only replaced by another saved one.
    pub fn put_lyrics_aligned(&self, track: TrackId, a: &AlignedLyrics) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO lyrics_align(track_id, source, text_hash, fit, timing, offset_ms, covered, body, made_by, made_at, saved)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(track_id, source) DO UPDATE SET text_hash = excluded.text_hash, fit = excluded.fit,
                    timing = excluded.timing, offset_ms = excluded.offset_ms, covered = excluded.covered,
                    body = excluded.body, made_by = excluded.made_by, made_at = excluded.made_at, saved = excluded.saved
                 WHERE lyrics_align.saved = 0 OR excluded.saved = 1",
                params![
                    track.to_string(),
                    a.source,
                    a.text_hash,
                    f64::from(a.fit),
                    a.timing,
                    a.offset_ms,
                    f64::from(a.covered),
                    a.body,
                    a.made_by,
                    if a.made_at == 0 { now_ms() } else { a.made_at },
                    a.saved,
                ],
            )?;
            Ok(())
        })
    }

    /// Forgets what was made of a track's text (`None`: of all its texts).
    pub fn delete_lyrics_aligned(&self, track: TrackId, source: Option<&str>) -> Result<()> {
        self.write(|tx| {
            match source {
                Some(s) => tx.execute("DELETE FROM lyrics_align WHERE track_id = ?1 AND source = ?2", params![track.to_string(), s])?,
                None => tx.execute("DELETE FROM lyrics_align WHERE track_id = ?1", [track.to_string()])?,
            };
            Ok(())
        })
    }

    /// Local tracks the background pass of `made_by` has not been through,
    /// or that failed before `retry_before` (unix ms). Newest first.
    pub fn tracks_needing_alignment(&self, made_by: &str, retry_before: UnixMs, limit: u32) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT t.id FROM track t LEFT JOIN lyrics_align_done d ON d.track_id = t.id
                  WHERE t.has_local = 1 AND t.deleted_at IS NULL
                    AND (d.track_id IS NULL OR d.made_by <> ?1 OR (d.state = 'failed' AND d.at < ?2))
                  ORDER BY d.track_id IS NOT NULL, t.seq DESC
                  LIMIT ?3",
            )?;
            let ids = stmt.query_map(params![made_by, retry_before, limit], |r| get_id(r, 0))?;
            Ok(ids.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Records the background pass's result for a track.
    pub fn mark_alignment(&self, track: TrackId, state: &str, detail: Option<&str>, made_by: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO lyrics_align_done(track_id, state, detail, made_by, at) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(track_id) DO UPDATE SET state = excluded.state, detail = excluded.detail,
                    made_by = excluded.made_by, at = excluded.at",
                params![track.to_string(), state, detail, made_by, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Local tracks, and how many of them the pass of `made_by` is done with.
    pub fn alignment_counts(&self, made_by: &str) -> Result<AlignCounts> {
        self.read(|c| {
            let (total, done): (i64, i64) = c.query_row(
                "SELECT COUNT(*), COUNT(d.track_id) FROM track t
                   LEFT JOIN lyrics_align_done d ON d.track_id = t.id AND d.made_by = ?1 AND d.state <> 'failed'
                  WHERE t.has_local = 1 AND t.deleted_at IS NULL",
                [made_by],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok(AlignCounts { done: done as u32, total: total as u32 })
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};

    use super::*;

    fn local_track(lib: &Library, title: &str) -> Track {
        let a = lib.upsert_artist(&Artist::new("Кино")).unwrap();
        let t = lib
            .upsert_track(&Track::new(title, ArtistCredit::single("Кино", Some(a.id))))
            .unwrap();
        lib.add_source(
            &Source::new(t.id, SourceKind::LocalFile, format!("{title}.flac"), Codec::Flac)
                .with_quality(false),
        )
        .unwrap();
        t
    }

    #[test]
    fn roundtrip_and_miss_marker() {
        let lib = Library::open_in_memory().unwrap();
        let t = local_track(&lib, "Группа крови");
        assert!(lib.lyrics(t.id).unwrap().is_none());

        lib.put_lyrics(t.id, None, Some("  "), false).unwrap();
        assert!(lib.lyrics(t.id).unwrap().unwrap().is_miss());

        lib.put_lyrics(t.id, Some("[00:01.00]Привет"), Some("Привет"), false)
            .unwrap();
        let got = lib.lyrics(t.id).unwrap().unwrap();
        assert!(!got.is_miss());
        assert_eq!(got.synced.as_deref(), Some("[00:01.00]Привет"));
        assert_eq!(got.plain.as_deref(), Some("Привет"));
    }

    #[test]
    fn other_sources_and_pick() {
        let lib = Library::open_in_memory().unwrap();
        let t = local_track(&lib, "Кукушка");
        assert!(lib.lyrics_text(t.id, "lyricsplus").unwrap().is_none());
        lib.put_lyrics_text(t.id, "lyricsplus", Some(" ")).unwrap();
        assert_eq!(lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap().0, None);
        lib.put_lyrics_text(t.id, "lyricsplus", Some("<tt/>")).unwrap();
        assert_eq!(lib.lyrics_text(t.id, "lyricsplus").unwrap().unwrap().0.as_deref(), Some("<tt/>"));

        assert_eq!(lib.lyrics_pick(t.id).unwrap(), None);
        lib.set_lyrics_pick(t.id, Some("lrclib")).unwrap();
        lib.set_lyrics_pick(t.id, Some("lyricsplus")).unwrap();
        assert_eq!(lib.lyrics_pick(t.id).unwrap().as_deref(), Some("lyricsplus"));
        lib.set_lyrics_pick(t.id, None).unwrap();
        assert_eq!(lib.lyrics_pick(t.id).unwrap(), None);

        assert_eq!(lib.lyrics_show(t.id).unwrap(), None);
        lib.set_lyrics_show(t.id, Some("line")).unwrap();
        lib.set_lyrics_show(t.id, Some("plain")).unwrap();
        assert_eq!(lib.lyrics_show(t.id).unwrap().as_deref(), Some("plain"));
        lib.set_lyrics_show(t.id, None).unwrap();
        assert_eq!(lib.lyrics_show(t.id).unwrap(), None);
    }

    #[test]
    fn aligned_texts_roundtrip_and_a_saved_one_stays() {
        let lib = Library::open_in_memory().unwrap();
        let t = local_track(&lib, "Спокойная ночь");
        assert!(lib.lyrics_aligned(t.id).unwrap().is_empty());
        let auto = AlignedLyrics {
            source: "lrclib".into(),
            text_hash: "a".into(),
            fit: 0.8,
            timing: "shifted".into(),
            offset_ms: -2180,
            covered: 0.9,
            body: Some("[]".into()),
            made_by: "v1".into(),
            ..Default::default()
        };
        lib.put_lyrics_aligned(t.id, &auto).unwrap();
        let got = lib.lyrics_aligned(t.id).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].offset_ms, got[0].timing.as_str(), got[0].saved), (-2180, "shifted", false));
        assert!(got[0].made_at > 0);
        // What the user saved is not overwritten by the background.
        lib.put_lyrics_aligned(t.id, &AlignedLyrics { saved: true, body: Some("[1]".into()), ..auto.clone() }).unwrap();
        lib.put_lyrics_aligned(t.id, &AlignedLyrics { body: Some("[2]".into()), ..auto.clone() }).unwrap();
        assert_eq!(lib.lyrics_aligned(t.id).unwrap()[0].body.as_deref(), Some("[1]"));
        lib.delete_lyrics_aligned(t.id, Some("lrclib")).unwrap();
        assert!(lib.lyrics_aligned(t.id).unwrap().is_empty());
    }

    #[test]
    fn alignment_pass_queue_and_counts() {
        let lib = Library::open_in_memory().unwrap();
        let a = local_track(&lib, "a");
        let b = local_track(&lib, "b");
        assert_eq!(lib.alignment_counts("v1").unwrap(), AlignCounts { done: 0, total: 2 });
        assert_eq!(lib.tracks_needing_alignment("v1", 0, 10).unwrap(), vec![b.id, a.id]);
        lib.mark_alignment(b.id, "done", None, "v1").unwrap();
        lib.mark_alignment(a.id, "failed", Some("decode"), "v1").unwrap();
        assert!(lib.tracks_needing_alignment("v1", 0, 10).unwrap().is_empty());
        assert_eq!(lib.tracks_needing_alignment("v1", now_ms() + 1000, 10).unwrap(), vec![a.id]);
        assert_eq!(lib.alignment_counts("v1").unwrap(), AlignCounts { done: 1, total: 2 });
        // A new aligner goes through everything again.
        assert_eq!(lib.tracks_needing_alignment("v2", 0, 10).unwrap().len(), 2);
    }

    #[test]
    fn backfill_queue_skips_found_and_fresh_misses() {
        let lib = Library::open_in_memory().unwrap();
        let fresh = local_track(&lib, "a");
        let found = local_track(&lib, "b");
        let never = local_track(&lib, "c");
        lib.put_lyrics(fresh.id, None, None, false).unwrap();
        lib.put_lyrics(found.id, None, Some("text"), false).unwrap();

        let week_ago = now_ms() - 7 * 86_400_000;
        assert_eq!(lib.tracks_needing_lyrics(week_ago, 10).unwrap(), vec![never.id]);
        // Once the marker is old enough it is retried, after the untried tracks.
        let queue = lib.tracks_needing_lyrics(now_ms() + 1000, 10).unwrap();
        assert_eq!(queue, vec![never.id, fresh.id]);
    }
}
