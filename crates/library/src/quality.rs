//! The spectrum checks of the local sources (migration 0022): what each
//! file's sound really is, kept so a "FLAC" made from an MP3 is never shown
//! as lossless, and the list of files still to check.

use nikgob_core::model::{Codec, ReleaseId, SourceId, TrackId, UnixMs, quality_score, score};
use nikgob_core::quality::{
    HIGHEST_CLEAR_CUT_HZ, LOWEST_LOWPASS_HZ, QualityCheck, SoundOrigin, Verdict, clear_fake, spectrum_from_blob,
    spectrum_to_blob,
};
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

/// A lossless file the check found made from a lossy one, plainly enough
/// to offer it as an MP3 ([`Library::fake_offers`]).
#[derive(Clone, Debug, PartialEq)]
pub struct FakeOffer {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub title: String,
    pub release: Option<ReleaseId>,
    /// The album's title, or the artist's line of a loose track.
    pub album: String,
    pub locator: String,
    pub cutoff_hz: f32,
    pub steepness_db: f32,
    pub origin: SoundOrigin,
}

/// A fake lossless file turned into an MP3 ([`Library::conversions`]): the
/// MP3 that plays now and the old file in the Trash.
#[derive(Clone, Debug, PartialEq)]
pub struct Conversion {
    pub track_id: TrackId,
    pub mp3: SourceId,
    pub mp3_locator: String,
    pub original: SourceId,
    /// Where the original lived, and where it is in the Trash now.
    pub original_locator: String,
    pub trash_path: String,
}

/// The lossless codecs as the `source.codec` column spells them.
const LOSSLESS_SQL: &str = "('flac', 'alac', 'wav')";

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
            // The user's «keep it» stays over a check made again.
            tx.execute(
                "INSERT INTO source_quality(source_id, verdict, cutoff_hz, steepness_db, origin,
                        sample_rate, spectrum, made_from, file_size, file_mtime, checked_at, failed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL)
                 ON CONFLICT(source_id) DO UPDATE SET verdict = excluded.verdict, cutoff_hz = excluded.cutoff_hz,
                        steepness_db = excluded.steepness_db, origin = excluded.origin,
                        sample_rate = excluded.sample_rate, spectrum = excluded.spectrum,
                        made_from = excluded.made_from, file_size = excluded.file_size,
                        file_mtime = excluded.file_mtime, checked_at = excluded.checked_at, failed = NULL",
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
                "INSERT INTO source_quality(source_id, verdict, origin, file_size, file_mtime, checked_at, failed)
                 VALUES (?1, 'unknown', 'unknown', ?2, ?3, ?4, ?5)
                 ON CONFLICT(source_id) DO UPDATE SET verdict = 'unknown', cutoff_hz = 0, steepness_db = 0,
                        origin = 'unknown', sample_rate = 0, spectrum = NULL, made_from = NULL,
                        file_size = excluded.file_size, file_mtime = excluded.file_mtime,
                        checked_at = excluded.checked_at, failed = excluded.failed",
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

    /// Every live local lossless file with its check stamp: the never
    /// checked first, then the checked ones (whose files the caller
    /// compares with the stamp). A lossy file's own lowpass is what its
    /// format does: it is never checked.
    pub fn quality_todo(&self) -> Result<Vec<QualityTodo>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT s.id, s.track_id, s.locator, s.codec, s.bitrate, s.quality_score,
                        q.file_size, q.file_mtime, q.made_from, q.source_id IS NOT NULL AS checked
                   FROM source s LEFT JOIN source_quality q ON q.source_id = s.id
                  WHERE s.kind = 'local_file' AND s.trashed_at IS NULL AND s.available = 1
                    AND s.codec IN {LOSSLESS_SQL}
                  ORDER BY checked, s.added_at DESC, s.id"
            ))?;
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

    /// Puts the stored checks right with what a check means now: a lossy
    /// file's own check goes (only a lossless sound is judged), a
    /// transcoding found with a cut below any encoder's lowpass is no
    /// verdict, and a live lossless file that was ranked down for such a
    /// verdict is ranked as what it is again; one found cut where a real
    /// master may be cut too (at [`HIGHEST_CLEAR_CUT_HZ`] and above), and an
    /// MP3 made from it, rank no lower than an MP3 320. Returns the tracks
    /// whose rows change.
    pub fn tidy_quality_checks(&self) -> Result<Vec<TrackId>> {
        self.write(|tx| {
            let mut tracks: Vec<TrackId> = Vec::new();
            {
                let mut stmt = tx.prepare_cached(&format!(
                    "SELECT DISTINCT s.track_id FROM source_quality q JOIN source s ON s.id = q.source_id
                      WHERE q.made_from IS NULL AND s.codec NOT IN {LOSSLESS_SQL}"
                ))?;
                let rows = stmt.query_map([], |r| get_id::<TrackId>(r, 0))?;
                tracks.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
            }
            tx.execute(
                &format!(
                    "DELETE FROM source_quality WHERE made_from IS NULL
                        AND source_id IN (SELECT id FROM source WHERE codec NOT IN {LOSSLESS_SQL})"
                ),
                [],
            )?;
            // The live lossless files a cut no encoder makes ranked down.
            let wronged: Vec<(SourceId, TrackId, String, Option<u32>, Option<u32>, Option<u32>, i32)> = {
                let mut stmt = tx.prepare_cached(&format!(
                    "SELECT s.id, s.track_id, s.codec, s.bitrate, s.sample_rate, s.bit_depth, s.quality_score
                       FROM source_quality q JOIN source s ON s.id = q.source_id
                      WHERE q.verdict = 'transcoded' AND q.cutoff_hz < ?1 AND q.made_from IS NULL
                        AND s.codec IN {LOSSLESS_SQL} AND s.trashed_at IS NULL AND s.origin_kind != 'stream'"
                ))?;
                let rows = stmt.query_map([f64::from(LOWEST_LOWPASS_HZ)], |r| {
                    Ok((get_id(r, 0)?, get_id(r, 1)?, r.get(2)?, opt_u32(r, 3)?, opt_u32(r, 4)?, opt_u32(r, 5)?, r.get(6)?))
                })?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            tx.execute(
                "UPDATE source_quality SET verdict = 'unknown', origin = 'unknown'
                  WHERE verdict = 'transcoded' AND cutoff_hz < ?1",
                [f64::from(LOWEST_LOWPASS_HZ)],
            )?;
            for (id, track, codec, bitrate, rate, depth, was) in wronged {
                let depth = depth.and_then(|d| u8::try_from(d).ok());
                let want = quality_score(&Codec::parse(&codec), bitrate, rate, depth, false);
                if want != was {
                    tx.execute("UPDATE source SET quality_score = ?2 WHERE id = ?1", params![id.to_string(), want])?;
                    refresh_active(tx, track)?;
                    tracks.push(track);
                }
            }
            // A cut a real master may have too was ranked as a plain fake
            // before; it may be what it says (`fake_score`).
            let unsure: Vec<(SourceId, TrackId)> = {
                let mut stmt = tx.prepare_cached(&format!(
                    "SELECT s.id, s.track_id FROM source_quality q JOIN source s ON s.id = q.source_id
                      WHERE q.verdict = 'transcoded' AND q.cutoff_hz >= ?1 AND q.failed IS NULL
                        AND s.quality_score < ?2 AND s.trashed_at IS NULL AND s.origin_kind != 'stream'
                        AND ((q.made_from IS NULL AND s.codec IN {LOSSLESS_SQL}) OR q.made_from IN {LOSSLESS_SQL})"
                ))?;
                let rows = stmt.query_map(params![f64::from(HIGHEST_CLEAR_CUT_HZ), score::MP3_320], |r| {
                    Ok((get_id(r, 0)?, get_id(r, 1)?))
                })?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            for (id, track) in unsure {
                tx.execute("UPDATE source SET quality_score = ?2 WHERE id = ?1", params![id.to_string(), score::MP3_320])?;
                refresh_active(tx, track)?;
                tracks.push(track);
            }
            tracks.sort();
            tracks.dedup();
            Ok(tracks)
        })
    }

    /// The user's answer about a fake: kept as it is (`true`), or offered
    /// again.
    pub fn set_quality_keep(&self, source: SourceId, keep: bool) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "UPDATE source_quality SET keep = ?2 WHERE source_id = ?1",
                params![source.to_string(), i64::from(keep)],
            )?;
            Ok(())
        })
    }

    /// The playing lossless files the check found made from a lossy one
    /// plainly enough ([`clear_fake`]) to offer them as MP3s, that the user
    /// did not keep: by album, then by place.
    pub fn fake_offers(&self) -> Result<Vec<FakeOffer>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT s.id, s.track_id, t.title, t.release_id, coalesce(r.title, ''), s.locator,
                        q.verdict, q.cutoff_hz, q.steepness_db, q.origin, s.sample_rate, s.bit_depth
                   FROM source s JOIN source_quality q ON q.source_id = s.id
                   JOIN track t ON t.id = s.track_id
                   LEFT JOIN release r ON r.id = t.release_id
                  WHERE s.kind = 'local_file' AND s.is_active = 1 AND s.trashed_at IS NULL AND s.available = 1
                    AND s.codec IN {LOSSLESS_SQL} AND s.origin_kind != 'stream' AND t.deleted_at IS NULL
                    AND q.verdict = 'transcoded' AND q.made_from IS NULL AND q.failed IS NULL AND q.keep = 0
                  ORDER BY coalesce(r.title, ''), t.release_id, t.disc_no, t.track_no, t.title, s.id"
            ))?;
            let rows = stmt.query_map([], |r| {
                let verdict = Verdict::parse(&r.get::<_, String>(6)?);
                let cutoff = r.get::<_, f64>(7)? as f32;
                let steep = r.get::<_, f64>(8)? as f32;
                let hi_res = opt_u32(r, 10)?.is_some_and(|x| x > 48_000) || opt_u32(r, 11)?.is_some_and(|b| b > 16);
                let offer = FakeOffer {
                    source_id: get_id(r, 0)?,
                    track_id: get_id(r, 1)?,
                    title: r.get(2)?,
                    release: crate::util::get_opt_id(r, 3)?,
                    album: r.get(4)?,
                    locator: r.get(5)?,
                    cutoff_hz: cutoff,
                    steepness_db: steep,
                    origin: SoundOrigin::parse(&r.get::<_, String>(9)?),
                };
                Ok(clear_fake(verdict, cutoff, steep, hi_res).then_some(offer))
            })?;
            Ok(rows.filter_map(|r| r.transpose()).collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Every fake lossless file turned into an MP3 whose original is still
    /// in the Trash: the live MP3 made from a lossless file, and the
    /// trashed lossless source of the same track.
    pub fn conversions(&self) -> Result<Vec<Conversion>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT m.track_id, m.id, m.locator, o.id, o.locator, o.trash_path
                   FROM source m JOIN source_quality q ON q.source_id = m.id
                   JOIN source o ON o.track_id = m.track_id AND o.id != m.id
                  WHERE m.kind = 'local_file' AND m.trashed_at IS NULL AND m.codec = 'mp3'
                    AND q.made_from IN {LOSSLESS_SQL}
                    AND o.kind = 'local_file' AND o.codec IN {LOSSLESS_SQL}
                    AND o.trashed_at IS NOT NULL AND o.trash_path IS NOT NULL
                  ORDER BY m.track_id, o.trashed_at DESC"
            ))?;
            let rows = stmt.query_map([], |r| {
                Ok(Conversion {
                    track_id: get_id(r, 0)?,
                    mp3: get_id(r, 1)?,
                    mp3_locator: r.get(2)?,
                    original: get_id(r, 3)?,
                    original_locator: r.get(4)?,
                    trash_path: r.get(5)?,
                })
            })?;
            let mut out: Vec<Conversion> = Vec::new();
            for c in rows {
                let c = c?;
                // One original per MP3: the one trashed last.
                if !out.iter().any(|o| o.mp3 == c.mp3) {
                    out.push(c);
                }
            }
            Ok(out)
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
        assert_eq!(
            row.check,
            Some(CheckSummary { verdict: Verdict::Transcoded, origin: SoundOrigin::Lossy { kbps: 128 }, made_from: None, cutoff_hz: Some(16_012) })
        );
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
        assert_eq!(order, vec![flac.id, done.id], "a lossy file is never checked");
        assert!(!order.contains(&mp3.id));
        assert_eq!(todo[1].checked, Some((26_733_824, 1_762_000_000_000)));
        assert_eq!(todo[0].checked, None);

        assert!(lib.set_source_score(done.id, nikgob_core::quality::fake_score(SoundOrigin::Lossy { kbps: 128 }, 16_012.5)).unwrap());
        assert!(!lib.set_source_score(done.id, nikgob_core::quality::fake_score(SoundOrigin::Lossy { kbps: 128 }, 16_012.5)).unwrap());
        let row = lib.get_track_row(done.track_id).unwrap().unwrap();
        assert!(row.quality.unwrap() < score::MP3_V0);

        // Checks of sources that are gone are dropped.
        lib.put_quality_check(SourceId::new(), &fake_check()).unwrap();
        assert_eq!(lib.drop_orphan_checks().unwrap(), 1);
    }

    /// «RuLe.mp3 is Transcoded», «Clover.mp3 is Genuine»: an MP3's own
    /// verdict goes; an OST's quiet FLAC «cut at 2.8 kHz» is no fake and
    /// ranks as lossless again; a check made from a FLAC stays.
    #[test]
    fn stored_checks_are_put_right() {
        let lib = Library::open_in_memory().unwrap();
        let mp3 = file(&lib, "RuLe", Codec::Mp3, 500);
        lib.put_quality_check(mp3.id, &fake_check()).unwrap();
        let made = file(&lib, "made", Codec::Mp3, score::FAKE_LOSSLESS);
        lib.put_quality_check(made.id, &QualityCheck { made_from: Some(Codec::Flac), ..fake_check() }).unwrap();
        let quiet = file(&lib, "Title", Codec::Flac, 500);
        let quiet_check = QualityCheck { cutoff_hz: 2_820.0, ..fake_check() };
        lib.put_quality_check(quiet.id, &quiet_check).unwrap();
        let real_fake = file(&lib, "Молли", Codec::Flac, 500);
        lib.put_quality_check(real_fake.id, &fake_check()).unwrap();

        let changed = lib.tidy_quality_checks().unwrap();
        assert!(changed.contains(&mp3.track_id) && changed.contains(&quiet.track_id));
        assert_eq!(lib.quality_check(mp3.id).unwrap(), None);
        assert!(lib.quality_check(made.id).unwrap().is_some(), "the check of the original stays");
        assert_eq!(lib.quality_check(quiet.id).unwrap().unwrap().verdict, Verdict::Unknown);
        assert!(lib.get_track_row(quiet.track_id).unwrap().unwrap().quality.unwrap() >= score::CD_LOSSLESS);
        assert_eq!(lib.quality_check(real_fake.id).unwrap().unwrap().verdict, Verdict::Transcoded);
        assert!(lib.tidy_quality_checks().unwrap().is_empty(), "once");
    }

    /// OMORI's tracks cut at 20.1–20.2 kHz (may be the master's own filter)
    /// were ranked as plain fakes: they rank as an MP3 320 now, an MP3 made
    /// from one too; the rows say it as a possibility. A plain fake stays.
    #[test]
    fn a_cut_a_master_may_have_ranks_as_mp3_320_and_says_maybe() {
        let lib = Library::open_in_memory().unwrap();
        let unsure_check = QualityCheck {
            cutoff_hz: 20_186.0,
            steepness_db: 48.8,
            origin: SoundOrigin::estimate(Verdict::Transcoded, 20_186.0),
            ..fake_check()
        };
        let flac = file(&lib, "Title", Codec::Flac, score::FAKE_LOSSLESS);
        lib.put_quality_check(flac.id, &unsure_check).unwrap();
        let mp3 = file(&lib, "Final Duet", Codec::Mp3, score::FAKE_LOSSLESS);
        lib.put_quality_check(mp3.id, &QualityCheck { made_from: Some(Codec::Flac), cutoff_hz: 20_100.0, ..unsure_check.clone() }).unwrap();
        let plain = file(&lib, "Молли", Codec::Flac, score::FAKE_LOSSLESS);
        lib.put_quality_check(plain.id, &fake_check()).unwrap();

        let row = lib.get_track_row(flac.track_id).unwrap().unwrap();
        assert_eq!(row.check.as_ref().and_then(|c| c.cutoff_hz), Some(20_186));
        assert_eq!(nikgob_core::quality::row_label(row.codec.as_ref().unwrap(), row.check.as_ref()), "FLAC, возможно из MP3 320");
        let row = lib.get_track_row(mp3.track_id).unwrap().unwrap();
        assert_eq!(nikgob_core::quality::row_label(row.codec.as_ref().unwrap(), row.check.as_ref()), "MP3 из FLAC, возможно поддельного");
        let row = lib.get_track_row(plain.track_id).unwrap().unwrap();
        assert_eq!(nikgob_core::quality::row_label(row.codec.as_ref().unwrap(), row.check.as_ref()), "FLAC из MP3 ~128");

        let changed = lib.tidy_quality_checks().unwrap();
        assert!(changed.contains(&flac.track_id) && changed.contains(&mp3.track_id));
        assert!(!changed.contains(&plain.track_id));
        assert_eq!(lib.get_source(flac.id).unwrap().unwrap().quality_score, score::MP3_320);
        assert_eq!(lib.get_source(mp3.id).unwrap().unwrap().quality_score, score::MP3_320);
        assert_eq!(lib.get_source(plain.id).unwrap().unwrap().quality_score, score::FAKE_LOSSLESS);
        assert!(lib.tidy_quality_checks().unwrap().is_empty(), "once");
    }

    /// A plain fake is offered until the user keeps it; the keep survives
    /// a check made again.
    #[test]
    fn plain_fakes_are_offered_until_kept() {
        let lib = Library::open_in_memory().unwrap();
        let fake = file(&lib, "Молли", Codec::Flac, 690);
        lib.put_quality_check(fake.id, &QualityCheck { cutoff_hz: 16_100.0, steepness_db: 44.2, ..fake_check() }).unwrap();
        let weak = file(&lib, "На луне", Codec::Flac, 500);
        lib.put_quality_check(weak.id, &QualityCheck { cutoff_hz: 15_200.0, steepness_db: 19.3, ..fake_check() }).unwrap();
        let offers = lib.fake_offers().unwrap();
        assert_eq!(offers.iter().map(|o| o.source_id).collect::<Vec<_>>(), vec![fake.id], "a weak cut is not offered");
        lib.set_quality_keep(fake.id, true).unwrap();
        assert!(lib.fake_offers().unwrap().is_empty());
        lib.put_quality_check(fake.id, &QualityCheck { cutoff_hz: 16_100.0, steepness_db: 44.2, ..fake_check() }).unwrap();
        assert!(lib.fake_offers().unwrap().is_empty(), "kept over a new check");
    }

    /// A conversion is found by its MP3 and the original in the Trash.
    #[test]
    fn conversions_pair_the_mp3_with_its_original() {
        let lib = Library::open_in_memory().unwrap();
        let flac = file(&lib, "Молли", Codec::Flac, 690);
        let mut mp3 = Source::new(flac.track_id, SourceKind::LocalFile, "/m/Молли.mp3", Codec::Mp3);
        mp3.quality_score = 690;
        let mp3 = lib.add_source(&mp3).unwrap();
        lib.trash_source(flac.id, Some("/t/2026-11-04 Молли.flac")).unwrap();
        lib.put_quality_check(mp3.id, &QualityCheck { made_from: Some(Codec::Flac), ..fake_check() }).unwrap();
        // An Opus made from a real FLAC is no conversion.
        let opus_track = file(&lib, "Opus", Codec::Opus, score::CD_LOSSLESS);
        lib.put_quality_check(opus_track.id, &QualityCheck { verdict: Verdict::Genuine, made_from: Some(Codec::Flac), ..fake_check() }).unwrap();
        let all = lib.conversions().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!((all[0].mp3, all[0].original, all[0].trash_path.as_str()), (mp3.id, flac.id, "/t/2026-11-04 Молли.flac"));
    }
}
