//! The spectrum checks of the local sources (migration 0022): what each
//! file's sound really is, kept so a "FLAC" made from an MP3 is never shown
//! as lossless, and the list of files still to check.

use nikgob_core::model::{Codec, SourceId, TrackId, UnixMs};
use nikgob_core::quality::{QualityCheck, SoundOrigin, Verdict, spectrum_from_blob, spectrum_to_blob};
use rusqlite::{OptionalExtension, Row, params};

use crate::catalog::refresh_active;
use crate::db::Library;
use crate::error::Result;
use crate::util::{get_id, now_ms, opt_u32};

/// A local source and what is known of its last check, for the background
/// check ([`Library::quality_todo`]).
#[derive(Clone, Debug, PartialEq)]
pub struct QualityTodo {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub locator: String,
    pub codec: Codec,
    pub bitrate: Option<u32>,
    pub quality_score: i32,
    /// `(size, mtime)` of the file when it was checked; `None`: never.
    pub checked: Option<(u64, UnixMs)>,
    /// The check is of the lossless file this source was made from: it
    /// stays as long as the source does.
    pub made_from: Option<Codec>,
}

const CHECK_COLS: &str = "verdict, cutoff_hz, steepness_db, origin, sample_rate, spectrum, made_from, \
                          file_size, file_mtime, checked_at";

fn check_from_row(r: &Row<'_>) -> rusqlite::Result<QualityCheck> {
    let blob: Option<Vec<u8>> = r.get(5)?;
    Ok(QualityCheck {
        verdict: Verdict::parse(&r.get::<_, String>(0)?),
        cutoff_hz: r.get::<_, f64>(1)? as f32,
        steepness_db: r.get::<_, f64>(2)? as f32,
        origin: SoundOrigin::parse(&r.get::<_, String>(3)?),
        sample_rate: opt_u32(r, 4)?.unwrap_or(0),
        spectrum_db: blob.as_deref().map(spectrum_from_blob).unwrap_or_default(),
        made_from: r.get::<_, Option<String>>(6)?.map(|c| Codec::parse(&c)),
        file_size: r.get::<_, i64>(7)?.max(0) as u64,
        file_mtime: r.get(8)?,
        checked_at: r.get(9)?,
    })
}

impl Library {
    /// Stores the check of a source (replacing an older one).
    pub fn put_quality_check(&self, source: SourceId, c: &QualityCheck) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO source_quality(source_id, verdict, cutoff_hz, steepness_db, origin,
                        sample_rate, spectrum, made_from, file_size, file_mtime, checked_at, failed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL)",
                params![
                    source.to_string(),
                    c.verdict.as_str(),
                    f64::from(c.cutoff_hz),
                    f64::from(c.steepness_db),
                    c.origin.as_str(),
                    c.sample_rate,
                    (!c.spectrum_db.is_empty()).then(|| spectrum_to_blob(&c.spectrum_db)),
                    c.made_from.as_ref().map(Codec::as_str),
                    c.file_size as i64,
                    c.file_mtime,
                    if c.checked_at == 0 { now_ms() } else { c.checked_at },
                ],
            )?;
            Ok(())
        })
    }

    /// A file that could not be checked (broken, unsupported): its row
    /// says "unknown" and it is not tried again until it changes.
    pub fn mark_quality_failed(&self, source: SourceId, file_size: u64, file_mtime: UnixMs, why: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO source_quality(source_id, verdict, origin, file_size, file_mtime,
                        checked_at, failed)
                 VALUES (?1, 'unknown', 'unknown', ?2, ?3, ?4, ?5)",
                params![source.to_string(), file_size as i64, file_mtime, now_ms(), why],
            )?;
            Ok(())
        })
    }

    /// The last check of a source, if any.
    pub fn quality_check(&self, source: SourceId) -> Result<Option<QualityCheck>> {
        self.read(|c| {
            Ok(c.prepare_cached(&format!("SELECT {CHECK_COLS} FROM source_quality WHERE source_id = ?1"))?
                .query_row([source.to_string()], check_from_row)
                .optional()?)
        })
    }

    /// Every live local source with its check stamp: never checked lossless
    /// files first, then never checked lossy ones, then the checked ones
    /// (whose files the caller compares with the stamp).
    pub fn quality_todo(&self) -> Result<Vec<QualityTodo>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT s.id, s.track_id, s.locator, s.codec, s.bitrate, s.quality_score,
                        q.file_size, q.file_mtime, q.made_from, q.source_id IS NOT NULL AS checked
                   FROM source s LEFT JOIN source_quality q ON q.source_id = s.id
                  WHERE s.kind = 'local_file' AND s.trashed_at IS NULL AND s.available = 1
                  ORDER BY checked,
                           CASE s.codec WHEN 'flac' THEN 0 WHEN 'alac' THEN 0 WHEN 'wav' THEN 0 ELSE 1 END,
                           s.added_at DESC, s.id",
            )?;
            let rows = stmt.query_map([], |r| {
                let checked: bool = r.get(9)?;
                Ok(QualityTodo {
                    source_id: get_id(r, 0)?,
                    track_id: get_id(r, 1)?,
                    locator: r.get(2)?,
                    codec: Codec::parse(&r.get::<_, String>(3)?),
                    bitrate: opt_u32(r, 4)?,
                    quality_score: r.get::<_, i64>(5)? as i32,
                    checked: checked
                        .then(|| -> rusqlite::Result<_> {
                            Ok((r.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0) as u64, r.get::<_, Option<i64>>(7)?.unwrap_or(0)))
                        })
                        .transpose()?,
                    made_from: r.get::<_, Option<String>>(8)?.map(|c| Codec::parse(&c)),
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Re-ranks a source (a lossless file found transcoded scores as what
    /// it really is) and re-selects its track's active source. True when
    /// the score changed.
    pub fn set_source_score(&self, source: SourceId, score: i32) -> Result<bool> {
        self.write(|tx| {
            let track: Option<String> = tx
                .query_row(
                    "SELECT track_id FROM source WHERE id = ?1 AND quality_score != ?2",
                    params![source.to_string(), score],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(track) = track else { return Ok(false) };
            tx.execute(
                "UPDATE source SET quality_score = ?2 WHERE id = ?1",
                params![source.to_string(), score],
            )?;
            let track = track.parse::<TrackId>().map_err(|_| crate::error::LibraryError::not_found("track", &track))?;
            refresh_active(tx, track)?;
            Ok(true)
        })
    }

    /// Drops the checks of sources that are no longer in the library.
    pub fn drop_orphan_checks(&self) -> Result<usize> {
        self.write(|tx| {
            Ok(tx.execute(
                "DELETE FROM source_quality WHERE source_id NOT IN (SELECT id FROM source)",
                [],
            )?)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{ArtistCredit, Source, SourceKind, Track, score};
    use nikgob_core::quality::{CheckSummary, SPECTRUM_POINTS};

    use super::*;

    fn file(lib: &Library, title: &str, codec: Codec, score: i32) -> Source {
        let t = lib.upsert_track(&Track::new(title, ArtistCredit::single(String::from("A"), None))).unwrap();
        let mut s = Source::new(t.id, SourceKind::LocalFile, format!("/m/{title}.{codec}"), codec);
        s.quality_score = score;
        lib.add_source(&s).unwrap()
    }

    fn fake_check() -> QualityCheck {
        QualityCheck {
            verdict: Verdict::Transcoded,
            cutoff_hz: 16_012.5,
            steepness_db: 52.25,
            origin: SoundOrigin::estimate(Verdict::Transcoded, 16_012.5),
            sample_rate: 44_100,
            spectrum_db: (0..SPECTRUM_POINTS).map(|i| -30.0 - i as f32 * 0.3).collect(),
            made_from: None,
            file_size: 26_733_824,
            file_mtime: 1_762_000_000_000,
            checked_at: 1_762_000_000_500,
        }
    }

    #[test]
    fn a_check_is_stored_read_back_and_shown_in_the_rows() {
        let lib = Library::open_in_memory().unwrap();
        let s = file(&lib, "Молли", Codec::Flac, score::CD_LOSSLESS);
        assert_eq!(lib.quality_check(s.id).unwrap(), None);
        assert_eq!(lib.get_track_row(s.track_id).unwrap().unwrap().check, None);

        let c = fake_check();
        lib.put_quality_check(s.id, &c).unwrap();
        let back = lib.quality_check(s.id).unwrap().unwrap();
        assert_eq!((back.verdict, back.origin, back.made_from.clone()), (c.verdict, c.origin, None));
        assert_eq!((back.cutoff_hz, back.steepness_db, back.sample_rate), (c.cutoff_hz, c.steepness_db, 44_100));
        assert_eq!((back.file_size, back.file_mtime, back.checked_at), (c.file_size, c.file_mtime, c.checked_at));
        // The spectrum keeps a tenth of a dB.
        assert_eq!(back.spectrum_db.len(), SPECTRUM_POINTS);
        assert!(back.spectrum_db.iter().zip(&c.spectrum_db).all(|(a, b)| (a - b).abs() <= 0.051));

        let row = lib.get_track_row(s.track_id).unwrap().unwrap();
        assert_eq!(row.check, Some(CheckSummary { verdict: Verdict::Transcoded, origin: SoundOrigin::Lossy { kbps: 128 }, made_from: None }));
        assert_eq!(nikgob_core::quality::row_label(row.codec.as_ref().unwrap(), row.check.as_ref()), "FLAC из MP3 ~128");

        // A failed check is not a verdict for the rows.
        lib.mark_quality_failed(s.id, 1, 2, "broken").unwrap();
        assert_eq!(lib.get_track_row(s.track_id).unwrap().unwrap().check, None);
        assert_eq!(lib.quality_check(s.id).unwrap().unwrap().verdict, Verdict::Unknown);
    }

    #[test]
    fn the_todo_puts_unchecked_lossless_first_and_a_fake_is_rescored() {
        let lib = Library::open_in_memory().unwrap();
        let mp3 = file(&lib, "a", Codec::Mp3, score::MP3_320);
        let flac = file(&lib, "b", Codec::Flac, score::CD_LOSSLESS);
        let done = file(&lib, "c", Codec::Flac, score::CD_LOSSLESS);
        lib.put_quality_check(done.id, &fake_check()).unwrap();
        let todo = lib.quality_todo().unwrap();
        let order: Vec<SourceId> = todo.iter().map(|t| t.source_id).collect();
        assert_eq!(order, vec![flac.id, mp3.id, done.id]);
        assert_eq!(todo[2].checked, Some((26_733_824, 1_762_000_000_000)));
        assert_eq!(todo[0].checked, None);

        assert!(lib.set_source_score(done.id, nikgob_core::quality::fake_score(SoundOrigin::Lossy { kbps: 128 })).unwrap());
        assert!(!lib.set_source_score(done.id, nikgob_core::quality::fake_score(SoundOrigin::Lossy { kbps: 128 })).unwrap());
        let row = lib.get_track_row(done.track_id).unwrap().unwrap();
        assert!(row.quality.unwrap() < score::MP3_V0);

        // Checks of sources that are gone are dropped.
        lib.put_quality_check(SourceId::new(), &fake_check()).unwrap();
        assert_eq!(lib.drop_orphan_checks().unwrap(), 1);
    }
}
