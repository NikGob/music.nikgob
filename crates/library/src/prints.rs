//! What the local files sound like (migration 0021): the
//! Chromaprint of each file, the files that held another song's audio (never
//! taken for that track again) and the pairs of files that sound the same
//! under different titles, waiting for the user.
//!
//! * [`Library::put_audio_print`]: the import stores the print of each file
//!   it adds (with the hash of the file as it came and where it came from);
//!   the background check fills in the rest ([`Library::prints_todo`]).
//! * [`Library::prints_near`]: the prints an incoming file is compared with
//!   (its album, its artist).
//! * [`Library::put_wrong_audio`] / [`Library::wrong_audio_of`]: what is
//!   not taken for a track again.
//! * [`Library::put_same_audio`] / [`Library::same_audio_offers`]: the
//!   pairs «Подтверди» asks about.

use std::collections::HashMap;

use nikgob_core::model::{Codec, ReleaseId, SourceId, TrackId, UnixMs};
use rusqlite::{OptionalExtension, params};

use crate::db::Library;
use crate::error::Result;
use crate::util::{get_id, get_opt_id, now_ms, opt_u32};

/// A local file's Chromaprint as the import or the background check stored it.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPrint {
    pub source_id: SourceId,
    pub track_id: TrackId,
    /// Raw Chromaprint items of its first 120 s; `None`: it could not be read.
    pub print: Option<Vec<u32>>,
    /// Length of the whole file.
    pub duration_ms: Option<u32>,
    /// BLAKE3 of the file as it came, before tags or conversion.
    pub came_hash: Option<String>,
    /// Where it came from: `torrent:<info hash>`, `soulseek:<user\path>`, …
    pub came_from: Vec<String>,
    /// Why it could not be read.
    pub failed: Option<String>,
}

/// The stored print of a live local file near a track (on its album, by
/// its artist), for [`Library::prints_near`].
#[derive(Clone, Debug, PartialEq)]
pub struct NearPrint {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub title: String,
    pub release_id: Option<ReleaseId>,
    /// Length of the file (the track's when the print does not say).
    pub duration_ms: Option<u32>,
    pub print: Vec<u32>,
    /// When the file came into the library.
    pub added_at: UnixMs,
}

/// A live local file without a stored print, for the background check.
#[derive(Clone, Debug, PartialEq)]
pub struct PrintTodo {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub locator: String,
    pub release_id: Option<ReleaseId>,
}

/// One thing not taken for a track again ([`Library::put_wrong_audio`]).
#[derive(Clone, Debug, PartialEq)]
pub struct WrongAudio {
    pub track_id: TrackId,
    /// `hash:<blake3>` of the file as it came, or `from:<where>`.
    pub key: String,
    /// The track whose song the file held, when known.
    pub sounds_like: Option<TrackId>,
    pub print: Option<Vec<u32>>,
    pub at: UnixMs,
}

/// One file of a [`SameAudioOffer`].
#[derive(Clone, Debug, PartialEq)]
pub struct SameSide {
    pub source_id: SourceId,
    pub track_id: TrackId,
    pub title: String,
    pub duration_ms: Option<u32>,
    pub codec: Codec,
    pub bitrate: Option<u32>,
    /// When the file came into the library.
    pub added_at: UnixMs,
    /// Its track's album, and the album's title ('' for a loose track).
    pub release_id: Option<ReleaseId>,
    pub album: String,
}

impl SameSide {
    /// The two files are on one album (not two loose tracks): one of them
    /// may hold another song; on two albums they may be one recording.
    pub fn same_release(&self, other: &SameSide) -> bool {
        self.release_id.is_some() && self.release_id == other.release_id
    }
}

/// Two playing files that sound the same under different titles, not
/// answered yet.
#[derive(Clone, Debug, PartialEq)]
pub struct SameAudioOffer {
    pub a: SameSide,
    pub b: SameSide,
    pub similarity: f64,
    /// The album of the first, or `None` for a loose track.
    pub release: Option<ReleaseId>,
    /// Its title (the artist's line of a loose track).
    pub album: String,
    /// The artist's line of the first.
    pub artist: String,
}

/// Raw print items as stored (u32 little endian).
pub(crate) fn print_to_blob(print: &[u32]) -> Vec<u8> {
    print.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub(crate) fn print_from_blob(blob: &[u8]) -> Vec<u32> {
    blob.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn lines(v: &[String]) -> Option<String> {
    let kept: Vec<&str> = v.iter().map(|s| s.trim()).filter(|s| !s.is_empty() && !s.contains('\n')).collect();
    (!kept.is_empty()).then(|| kept.join("\n"))
}

fn from_lines(s: Option<String>) -> Vec<String> {
    s.map(|s| s.lines().map(str::to_owned).filter(|l| !l.is_empty()).collect()).unwrap_or_default()
}

/// The two sources of a pair in their stored order.
fn ordered(a: (SourceId, TrackId), b: (SourceId, TrackId)) -> ((SourceId, TrackId), (SourceId, TrackId)) {
    if a.0.to_string() <= b.0.to_string() { (a, b) } else { (b, a) }
}

const PRINT_COLS: &str = "source_id, track_id, print, duration_ms, came_hash, came_from, failed";

fn print_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AudioPrint> {
    let blob: Option<Vec<u8>> = r.get(2)?;
    Ok(AudioPrint {
        source_id: get_id(r, 0)?,
        track_id: get_id(r, 1)?,
        print: blob.as_deref().map(print_from_blob),
        duration_ms: opt_u32(r, 3)?,
        came_hash: r.get(4)?,
        came_from: from_lines(r.get(5)?),
        failed: r.get(6)?,
    })
}

impl Library {
    /// Stores the print of a file (replacing an older one of it).
    pub fn put_audio_print(&self, p: &AudioPrint) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO audio_print(source_id, track_id, print, duration_ms, came_hash, came_from, failed, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(source_id) DO UPDATE SET track_id = excluded.track_id, print = excluded.print,
                        duration_ms = excluded.duration_ms,
                        came_hash = coalesce(excluded.came_hash, audio_print.came_hash),
                        came_from = coalesce(excluded.came_from, audio_print.came_from),
                        failed = excluded.failed, at = excluded.at,
                        linked_at = CASE WHEN excluded.print IS audio_print.print THEN audio_print.linked_at END",
                params![
                    p.source_id.to_string(),
                    p.track_id.to_string(),
                    p.print.as_deref().filter(|v| !v.is_empty()).map(print_to_blob),
                    p.duration_ms,
                    p.came_hash,
                    lines(&p.came_from),
                    p.failed,
                    now_ms(),
                ],
            )?;
            Ok(())
        })
    }

    /// The stored print of a file, if any.
    pub fn audio_print(&self, source: SourceId) -> Result<Option<AudioPrint>> {
        self.read(|c| {
            Ok(c.prepare_cached(&format!("SELECT {PRINT_COLS} FROM audio_print WHERE source_id = ?1"))?
                .query_row([source.to_string()], print_from_row)
                .optional()?)
        })
    }

    /// The prints of a track's live local files (the file it plays first).
    pub fn prints_of_track(&self, track: TrackId) -> Result<Vec<Vec<u32>>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT p.print FROM audio_print p JOIN source s ON s.id = p.source_id
                  WHERE s.track_id = ?1 AND s.trashed_at IS NULL AND s.kind = 'local_file' AND p.print IS NOT NULL
                  ORDER BY s.is_active DESC, s.added_at DESC",
            )?;
            let rows = stmt.query_map([track.to_string()], |r| r.get::<_, Vec<u8>>(0))?;
            Ok(rows.map(|b| b.map(|b| print_from_blob(&b))).collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The stored prints of the live local files on `release`, by the main
    /// artist of `track`, or with the credit key `artist_key` (whichever
    /// are given), the files of deleted tracks left out.
    pub fn prints_near(
        &self,
        release: Option<ReleaseId>,
        track: Option<TrackId>,
        artist_key: Option<&str>,
    ) -> Result<Vec<NearPrint>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT p.source_id, s.track_id, t.title, t.release_id, coalesce(p.duration_ms, t.duration_ms),
                        p.print, s.added_at
                   FROM audio_print p
                   JOIN source s ON s.id = p.source_id
                   JOIN track t ON t.id = s.track_id
                  WHERE p.print IS NOT NULL AND s.trashed_at IS NULL AND s.kind = 'local_file'
                    AND t.deleted_at IS NULL
                    AND ((?1 IS NOT NULL AND t.release_id = ?1)
                      OR (?2 IS NOT NULL AND t.id IN (
                            SELECT a.track_id FROM track_artist a WHERE a.artist_id IN (
                                SELECT m.artist_id FROM track_artist m WHERE m.track_id = ?2 AND m.position = 0)))
                      OR (?3 IS NOT NULL AND ?3 != '' AND t.artist_key = ?3))
                  ORDER BY t.release_id, t.disc_no, t.track_no, s.id",
            )?;
            let rows = stmt.query_map(
                params![release.map(|r| r.to_string()), track.map(|t| t.to_string()), artist_key],
                |r| {
                    Ok(NearPrint {
                        source_id: get_id(r, 0)?,
                        track_id: get_id(r, 1)?,
                        title: r.get(2)?,
                        release_id: get_opt_id(r, 3)?,
                        duration_ms: opt_u32(r, 4)?,
                        print: print_from_blob(&r.get::<_, Vec<u8>>(5)?),
                        added_at: r.get(6)?,
                    })
                },
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Live local files without a stored print: those of `release` only
    /// (`Some`), or all of them, the newest first; at most `limit`.
    pub fn prints_todo(&self, release: Option<ReleaseId>, limit: usize) -> Result<Vec<PrintTodo>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT s.id, s.track_id, s.locator, t.release_id
                   FROM source s JOIN track t ON t.id = s.track_id
                  WHERE s.kind = 'local_file' AND s.trashed_at IS NULL AND s.available = 1
                    AND t.deleted_at IS NULL
                    AND (?1 IS NULL OR t.release_id = ?1)
                    AND NOT EXISTS (SELECT 1 FROM audio_print p WHERE p.source_id = s.id)
                  ORDER BY s.added_at DESC, s.id
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![release.map(|r| r.to_string()), limit as i64], |r| {
                Ok(PrintTodo {
                    source_id: get_id(r, 0)?,
                    track_id: get_id(r, 1)?,
                    locator: r.get(2)?,
                    release_id: get_opt_id(r, 3)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Drops the prints and pairs of sources that are no longer in the
    /// library. Returns how many rows went.
    pub fn drop_orphan_prints(&self) -> Result<usize> {
        self.write(|tx| {
            let mut n = tx.execute("DELETE FROM audio_print WHERE source_id NOT IN (SELECT id FROM source)", [])?;
            n += tx.execute(
                "DELETE FROM same_audio WHERE source_id NOT IN (SELECT id FROM source)
                    OR other_source NOT IN (SELECT id FROM source)",
                [],
            )?;
            Ok(n)
        })
    }

    /// Notes a file that held another song's audio: `keys` (`hash:…`,
    /// `from:…`) are never taken for `track` again; `print` goes on the
    /// first of them.
    pub fn put_wrong_audio(
        &self,
        track: TrackId,
        keys: &[String],
        sounds_like: Option<TrackId>,
        print: Option<&[u32]>,
    ) -> Result<()> {
        self.write(|tx| {
            let at = now_ms();
            for (i, key) in keys.iter().filter(|k| !k.trim().is_empty()).enumerate() {
                let blob = (i == 0).then(|| print.filter(|p| !p.is_empty()).map(print_to_blob)).flatten();
                tx.execute(
                    "INSERT INTO wrong_audio(track_id, key, sounds_like, print, at) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(track_id, key) DO UPDATE SET
                        sounds_like = coalesce(excluded.sounds_like, wrong_audio.sounds_like),
                        print = coalesce(excluded.print, wrong_audio.print), at = excluded.at",
                    params![track.to_string(), key.trim(), sounds_like.map(|t| t.to_string()), blob, at],
                )?;
            }
            Ok(())
        })
    }

    /// Takes back [`Self::put_wrong_audio`] (its undo).
    pub fn forget_wrong_audio(&self, track: TrackId, keys: &[String]) -> Result<()> {
        self.write(|tx| {
            for key in keys {
                tx.execute(
                    "DELETE FROM wrong_audio WHERE track_id = ?1 AND key = ?2",
                    params![track.to_string(), key.trim()],
                )?;
            }
            Ok(())
        })
    }

    /// Everything not taken for `track` again.
    pub fn wrong_audio_of(&self, track: TrackId) -> Result<Vec<WrongAudio>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT track_id, key, sounds_like, print, at FROM wrong_audio WHERE track_id = ?1 ORDER BY at, key",
            )?;
            let rows = stmt.query_map([track.to_string()], |r| {
                let blob: Option<Vec<u8>> = r.get(3)?;
                Ok(WrongAudio {
                    track_id: get_id(r, 0)?,
                    key: r.get(1)?,
                    sounds_like: get_opt_id(r, 2)?,
                    print: blob.as_deref().map(print_from_blob),
                    at: r.get(4)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// The places a file came from that held another song for each of
    /// `tracks` (the `from:` keys without their prefix).
    pub fn wrong_sources(&self, tracks: &[TrackId]) -> Result<HashMap<TrackId, Vec<String>>> {
        let mut out: HashMap<TrackId, Vec<String>> = HashMap::new();
        if tracks.is_empty() {
            return Ok(out);
        }
        self.read(|c| {
            let mut stmt =
                c.prepare_cached("SELECT key FROM wrong_audio WHERE track_id = ?1 AND key LIKE 'from:%' ORDER BY key")?;
            for t in tracks {
                let rows = stmt.query_map([t.to_string()], |r| r.get::<_, String>(0))?;
                for key in rows {
                    let key = key?;
                    if let Some(from) = key.strip_prefix("from:") {
                        out.entry(*t).or_default().push(from.to_owned());
                    }
                }
            }
            Ok(())
        })?;
        Ok(out)
    }

    /// Notes two files that sound the same under different titles (a pair
    /// the user answered already stays answered).
    pub fn put_same_audio(
        &self,
        a: (SourceId, TrackId),
        b: (SourceId, TrackId),
        similarity: f64,
    ) -> Result<()> {
        if a.0 == b.0 || a.1 == b.1 {
            return Ok(());
        }
        let ((sa, ta), (sb, tb)) = ordered(a, b);
        self.write(|tx| {
            tx.execute(
                "INSERT INTO same_audio(source_id, other_source, track_id, other_track, similarity, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(source_id, other_source) DO UPDATE SET
                    track_id = excluded.track_id, other_track = excluded.other_track,
                    similarity = excluded.similarity",
                params![sa.to_string(), sb.to_string(), ta.to_string(), tb.to_string(), similarity, now_ms()],
            )?;
            Ok(())
        })
    }

    /// The user said both files of a pair are right: it is not asked again.
    pub fn keep_same_audio(&self, a: SourceId, b: SourceId) -> Result<()> {
        let (sa, sb) = if a.to_string() <= b.to_string() { (a, b) } else { (b, a) };
        self.write(|tx| {
            tx.execute(
                "UPDATE same_audio SET kept = 1 WHERE source_id = ?1 AND other_source = ?2",
                params![sa.to_string(), sb.to_string()],
            )?;
            Ok(())
        })
    }

    /// Forgets an open pair (a stricter rule no longer finds it): it is not
    /// offered, and a later check may note it again.
    pub fn drop_same_audio(&self, a: SourceId, b: SourceId) -> Result<()> {
        let (sa, sb) = if a.to_string() <= b.to_string() { (a, b) } else { (b, a) };
        self.write(|tx| {
            tx.execute(
                "DELETE FROM same_audio WHERE source_id = ?1 AND other_source = ?2 AND kept = 0",
                params![sa.to_string(), sb.to_string()],
            )?;
            Ok(())
        })
    }

    /// The open pairs whose two files are still the ones their tracks play,
    /// by album and place.
    pub fn same_audio_offers(&self) -> Result<Vec<SameAudioOffer>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT p.similarity, ta.release_id, coalesce(r.title, ta.artist_display), ta.artist_display,
                        sa.id, ta.id, ta.title, ta.duration_ms, sa.codec, sa.bitrate, sa.added_at, ta.release_id, coalesce(r.title, ''),
                        sb.id, tb.id, tb.title, tb.duration_ms, sb.codec, sb.bitrate, sb.added_at, tb.release_id, coalesce(rb.title, '')
                   FROM same_audio p
                   JOIN source sa ON sa.id = p.source_id
                   JOIN source sb ON sb.id = p.other_source
                   JOIN track ta ON ta.id = sa.track_id
                   JOIN track tb ON tb.id = sb.track_id
                   LEFT JOIN release r ON r.id = ta.release_id
                   LEFT JOIN release rb ON rb.id = tb.release_id
                  WHERE p.kept = 0 AND ta.id != tb.id
                    AND NOT EXISTS (SELECT 1 FROM recording_link la JOIN recording_link lb ON lb.recording = la.recording
                                     WHERE la.track_id = ta.id AND lb.track_id = tb.id)
                    AND ta.deleted_at IS NULL AND tb.deleted_at IS NULL
                    AND sa.is_active = 1 AND sa.trashed_at IS NULL AND sa.kind = 'local_file' AND sa.available = 1
                    AND sb.is_active = 1 AND sb.trashed_at IS NULL AND sb.kind = 'local_file' AND sb.available = 1
                  ORDER BY coalesce(r.title, ta.artist_display), ta.release_id, ta.disc_no, ta.track_no, sa.id, sb.id",
            )?;
            let side = |r: &rusqlite::Row<'_>, at: usize| -> rusqlite::Result<SameSide> {
                Ok(SameSide {
                    source_id: get_id(r, at)?,
                    track_id: get_id(r, at + 1)?,
                    title: r.get(at + 2)?,
                    duration_ms: opt_u32(r, at + 3)?,
                    codec: Codec::parse(&r.get::<_, String>(at + 4)?),
                    bitrate: opt_u32(r, at + 5)?,
                    added_at: r.get(at + 6)?,
                    release_id: get_opt_id(r, at + 7)?,
                    album: r.get(at + 8)?,
                })
            };
            let rows = stmt.query_map([], |r| {
                Ok(SameAudioOffer {
                    similarity: r.get(0)?,
                    release: get_opt_id(r, 1)?,
                    album: r.get(2)?,
                    artist: r.get(3)?,
                    a: side(r, 4)?,
                    b: side(r, 13)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{ArtistCredit, Release, Source, SourceKind, Track};

    use super::*;

    fn song(lib: &Library, title: &str, release: Option<ReleaseId>) -> Source {
        let mut t = Track::new(title, ArtistCredit::single(String::from("Пошлая Молли"), None));
        t.release_id = release;
        let t = lib.upsert_track(&t).unwrap();
        let s = Source::new(t.id, SourceKind::LocalFile, format!("/m/{title}.opus"), Codec::Opus);
        lib.add_source(&s).unwrap()
    }

    fn print(lib: &Library, s: &Source, items: Vec<u32>) {
        lib.put_audio_print(&AudioPrint {
            source_id: s.id,
            track_id: s.track_id,
            print: Some(items),
            duration_ms: Some(160_480),
            came_hash: Some("abc".into()),
            came_from: vec!["soulseek:user\\Music\\09.opus".into()],
            failed: None,
        })
        .unwrap();
    }

    #[test]
    fn prints_are_stored_and_found_near_a_track() {
        let lib = Library::open_in_memory().unwrap();
        let album = lib
            .upsert_release(&Release::new("8 способов", ArtistCredit::single(String::from("Пошлая Молли"), None)))
            .unwrap();
        let a = song(&lib, "Тмстс", Some(album.id));
        let b = song(&lib, "Сфотографирую", Some(album.id));
        let other = song(&lib, "Другая", None);
        assert_eq!(lib.prints_todo(None, 10).unwrap().len(), 3);
        print(&lib, &a, vec![1, 2, 3, u32::MAX]);
        let back = lib.audio_print(a.id).unwrap().unwrap();
        assert_eq!(back.print.as_deref(), Some(&[1, 2, 3, u32::MAX][..]));
        assert_eq!(back.came_from, vec!["soulseek:user\\Music\\09.opus".to_owned()]);
        assert_eq!(lib.prints_of_track(a.track_id).unwrap(), vec![vec![1, 2, 3, u32::MAX]]);
        print(&lib, &other, vec![9, 9]);
        let near = lib.prints_near(Some(album.id), None, None).unwrap();
        assert_eq!(near.len(), 1, "only the album's printed files");
        assert_eq!((near[0].title.as_str(), near[0].duration_ms), ("Тмстс", Some(160_480)));
        let todo = lib.prints_todo(Some(album.id), 10).unwrap();
        assert_eq!(todo.iter().map(|t| t.source_id).collect::<Vec<_>>(), vec![b.id]);
        // A file that goes to the Trash is no longer near anything.
        lib.trash_source(a.id, None).unwrap();
        assert!(lib.prints_near(Some(album.id), None, None).unwrap().is_empty());
    }

    #[test]
    fn wrong_files_and_pairs_are_kept_per_track() {
        let lib = Library::open_in_memory().unwrap();
        let a = song(&lib, "Тмстс", None);
        let b = song(&lib, "Сфотографирую", None);
        let keys = vec!["hash:abc".to_owned(), "from:soulseek:user\\09.opus".to_owned()];
        lib.put_wrong_audio(b.track_id, &keys, Some(a.track_id), Some(&[5, 6, 7])).unwrap();
        let rows = lib.wrong_audio_of(b.track_id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.iter().filter(|r| r.print.is_some()).count(), 1, "the print goes on one row");
        assert!(lib.wrong_audio_of(a.track_id).unwrap().is_empty());
        let from = lib.wrong_sources(&[a.track_id, b.track_id]).unwrap();
        assert_eq!(from.get(&b.track_id), Some(&vec!["soulseek:user\\09.opus".to_owned()]));
        assert!(!from.contains_key(&a.track_id));
        lib.forget_wrong_audio(b.track_id, &keys).unwrap();
        assert!(lib.wrong_audio_of(b.track_id).unwrap().is_empty());

        lib.put_same_audio((b.id, b.track_id), (a.id, a.track_id), 0.97).unwrap();
        lib.put_same_audio((a.id, a.track_id), (b.id, b.track_id), 0.98).unwrap();
        let offers = lib.same_audio_offers().unwrap();
        assert_eq!(offers.len(), 1, "one pair whichever way round");
        assert_eq!(offers[0].similarity, 0.98);
        lib.keep_same_audio(b.id, a.id).unwrap();
        assert!(lib.same_audio_offers().unwrap().is_empty());
        lib.put_same_audio((a.id, a.track_id), (b.id, b.track_id), 0.99).unwrap();
        assert!(lib.same_audio_offers().unwrap().is_empty(), "an answered pair stays answered");
    }
}
