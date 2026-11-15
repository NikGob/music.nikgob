//! Artist photo cache: where the downloaded picture lives, or that there is none.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nikgob_core::model::{ArtistId, UnixMs};
use rusqlite::{OptionalExtension, params};

use crate::db::Library;
use crate::error::Result;
use crate::util::{get_id, now_ms};

/// `artist_photo.source` of a lookup that found a photo it could not prove
/// this artist's.
const UNVERIFIED: &str = "unverified";

impl Library {
    /// The stored photo path of an artist (`None` = unknown or looked up without luck).
    pub fn artist_photo(&self, artist: ArtistId) -> Result<Option<String>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT path FROM artist_photo WHERE artist_id = ?1",
                [artist.to_string()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
        })
    }

    /// The raw cache row: `(path, fetched_at)`; a `None` path is a recorded miss.
    pub fn artist_photo_lookup(&self, artist: ArtistId) -> Result<Option<(Option<String>, UnixMs)>> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT path, fetched_at FROM artist_photo WHERE artist_id = ?1",
                [artist.to_string()],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, UnixMs>(1)?)),
            )
            .optional()?)
        })
    }

    /// The artist's photo is to be looked up: never tried, a miss older than
    /// `retry_before`, or an unverified one older than `unverified_before`
    /// (see [`Library::artists_needing_photo`]). Never for a role.
    pub fn artist_photo_due(&self, artist: ArtistId, retry_before: UnixMs, unverified_before: UnixMs) -> Result<bool> {
        self.read(|c| {
            let role: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM artist_latin WHERE artist_id = ?1 AND character = 1)",
                [artist.to_string()],
                |r| r.get(0),
            )?;
            if role {
                return Ok(false);
            }
            let row = c
                .query_row(
                    "SELECT path IS NULL AND photo_locked = 0, fetched_at, source = ?2 FROM artist_photo WHERE artist_id = ?1",
                    params![artist.to_string(), UNVERIFIED],
                    |r| Ok((r.get::<_, bool>(0)?, r.get::<_, UnixMs>(1)?, r.get::<_, bool>(2)?)),
                )
                .optional()?;
            Ok(match row {
                None => true,
                Some((open, at, unverified)) => open && (at < retry_before || (unverified && at < unverified_before)),
            })
        })
    }

    /// Every artist that has a photo.
    pub fn artist_photos(&self) -> Result<HashMap<ArtistId, String>> {
        self.read(|c| {
            let mut stmt =
                c.prepare_cached("SELECT artist_id, path FROM artist_photo WHERE path IS NOT NULL")?;
            let rows = stmt.query_map([], |r| {
                Ok((get_id::<ArtistId>(r, 0)?, r.get::<_, String>(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
        })
    }

    /// The artists whose photo the user chose ([`Library::set_custom_artist_photo`]):
    /// the only photo a role shows.
    pub fn locked_artist_photos(&self) -> Result<std::collections::HashSet<ArtistId>> {
        self.read(|c| {
            let mut stmt =
                c.prepare_cached("SELECT artist_id FROM artist_photo WHERE path IS NOT NULL AND photo_locked = 1")?;
            let rows = stmt.query_map([], |r| get_id::<ArtistId>(r, 0))?;
            Ok(rows.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?)
        })
    }

    /// Records the photo of an artist; `None` records "looked up, none found".
    /// A photo the user locked is never touched (see [`Library::set_custom_artist_photo`]).
    pub fn set_artist_photo(&self, artist: ArtistId, path: Option<&str>) -> Result<()> {
        self.set_artist_photo_from(artist, path, "deezer")
    }

    /// [`Library::set_artist_photo`] of a photo from `source` (`deezer`,
    /// `youtube`, `spotify`, `wikimedia`).
    pub fn set_artist_photo_from(&self, artist: ArtistId, path: Option<&str>, source: &str) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO artist_photo(artist_id, path, source, fetched_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(artist_id) DO UPDATE SET path = excluded.path,
                    source = excluded.source, fetched_at = excluded.fetched_at
                 WHERE photo_locked = 0",
                params![artist.to_string(), path, source, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Records "a photo was there but could not be proved this artist's":
    /// not a miss for days, it is asked again by the next session
    /// ([`Library::artists_needing_photo`]'s `unverified_before`).
    pub fn set_artist_photo_unverified(&self, artist: ArtistId) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO artist_photo(artist_id, path, source, fetched_at) VALUES (?1, NULL, ?2, ?3)
                 ON CONFLICT(artist_id) DO UPDATE SET path = NULL,
                    source = excluded.source, fetched_at = excluded.fetched_at
                 WHERE photo_locked = 0",
                params![artist.to_string(), UNVERIFIED, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Forgets the photos of `artists` (their files are gone): they are
    /// looked up again as if never tried. A photo the user locked stays.
    pub fn forget_artist_photos(&self, artists: &[ArtistId]) -> Result<usize> {
        if artists.is_empty() {
            return Ok(0);
        }
        self.write(|tx| {
            let mut n = 0;
            for a in artists {
                n += tx.execute(
                    "UPDATE artist_photo SET path = NULL, source = 'deezer', fetched_at = 0
                      WHERE artist_id = ?1 AND photo_locked = 0",
                    [a.to_string()],
                )?;
            }
            Ok(n)
        })
    }

    /// Whether the user chose this artist's photo by hand.
    pub fn artist_photo_locked(&self, artist: ArtistId) -> Result<bool> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT photo_locked FROM artist_photo WHERE artist_id = ?1",
                [artist.to_string()],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false))
        })
    }

    /// Stores `bytes` (a JPEG or PNG) as `<dir>/<artist>-custom.<ext>`, makes it
    /// the artist's photo and locks it against background enrichment. Returns
    /// the path.
    pub fn set_custom_artist_photo(
        &self,
        artist: ArtistId,
        dir: &Path,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) { "png" } else { "jpg" };
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{artist}-custom.{ext}"));
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &path)?;
        let stored = path.display().to_string();
        self.write(|tx| {
            tx.execute(
                "INSERT INTO artist_photo(artist_id, path, source, fetched_at, photo_locked)
                 VALUES (?1, ?2, 'custom', ?3, 1)
                 ON CONFLICT(artist_id) DO UPDATE SET path = excluded.path,
                    source = 'custom', fetched_at = excluded.fetched_at, photo_locked = 1",
                params![artist.to_string(), stored, now_ms()],
            )?;
            Ok(())
        })?;
        Ok(path)
    }

    /// Drops the photo and its lock: the artist becomes "never tried" and the
    /// worker resolves a photo again. A custom file is deleted.
    pub fn clear_artist_photo(&self, artist: ArtistId) -> Result<()> {
        let old = self.read(|c| {
            Ok(c.query_row(
                "SELECT path FROM artist_photo WHERE artist_id = ?1 AND source = 'custom'",
                [artist.to_string()],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
        })?;
        self.write(|tx| {
            tx.execute(
                "UPDATE artist_photo SET path = NULL, source = 'deezer', fetched_at = 0,
                        photo_locked = 0 WHERE artist_id = ?1",
                [artist.to_string()],
            )?;
            Ok(())
        })?;
        if let Some(old) = old {
            let _ = std::fs::remove_file(old);
        }
        Ok(())
    }

    /// Artists with a local track or a library track (a stream one too) and
    /// no photo: never tried, a miss older than `retry_before`, or an
    /// unverified one older than `unverified_before` (unix ms). Never-tried
    /// first, artists with files first, most tracks first. A compilation's
    /// «Various Artists» is nobody and has no photo to look up, a role (an
    /// anime character, [`Library::mark_voice_roles`]) no photo of its own.
    pub fn artists_needing_photo(
        &self,
        retry_before: UnixMs,
        unverified_before: UnixMs,
        limit: u32,
    ) -> Result<Vec<(ArtistId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT a.id, a.name FROM artist a LEFT JOIN artist_photo p ON p.artist_id = a.id
                  WHERE EXISTS (SELECT 1 FROM track_artist ta JOIN track t ON t.id = ta.track_id
                                 WHERE ta.artist_id = a.id
                                   AND (t.has_local = 1 OR (t.in_library = 1 AND t.deleted_at IS NULL)))
                    AND NOT various_artists(a.name)
                    AND NOT EXISTS (SELECT 1 FROM artist_latin l WHERE l.artist_id = a.id AND l.character = 1)
                    AND (p.artist_id IS NULL
                         OR (p.path IS NULL AND p.photo_locked = 0
                             AND (p.fetched_at < ?1 OR (p.source = ?4 AND p.fetched_at < ?3))))
                  ORDER BY p.fetched_at IS NOT NULL,
                           NOT EXISTS (SELECT 1 FROM track_artist ta JOIN track t ON t.id = ta.track_id
                                        WHERE ta.artist_id = a.id AND t.has_local = 1),
                           p.fetched_at,
                           (SELECT COUNT(*) FROM track_artist ta WHERE ta.artist_id = a.id) DESC,
                           a.seq
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![retry_before, limit, unverified_before, UNVERIFIED], |r| {
                Ok((get_id::<ArtistId>(r, 0)?, r.get::<_, String>(1)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}

#[cfg(test)]
mod tests {
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Source, SourceKind, Track};

    use super::*;

    fn artist_with_file(lib: &Library, name: &str, local: bool) -> Artist {
        let a = lib.upsert_artist(&Artist::new(name)).unwrap();
        let t = lib
            .upsert_track(&Track::new("t", ArtistCredit::single(name, Some(a.id))))
            .unwrap();
        if local {
            lib.add_source(
                &Source::new(t.id, SourceKind::LocalFile, format!("{name}.flac"), Codec::Flac)
                    .with_quality(false),
            )
            .unwrap();
        }
        a
    }

    #[test]
    fn photo_roundtrip_and_work_queue() {
        let lib = Library::open_in_memory().unwrap();
        let with = artist_with_file(&lib, "Кино", true);
        let missed = artist_with_file(&lib, "Nobody", true);
        let fresh = artist_with_file(&lib, "Земфира", true);
        // In the library without a file (a liked stream song): asked after
        // the artists with files.
        let remote = artist_with_file(&lib, "Remote", false);
        // A stream song outside the library: its artist is not asked.
        let heard = nikgob_core::streaming::FakeService::youtube().tracks()[4].clone();
        lib.upsert_stream_track(&heard).unwrap();
        // A compilation's artist line names nobody: never asked (Deezer
        // would give its generic picture).
        artist_with_file(&lib, "Various Artists", true);
        artist_with_file(&lib, "Разные исполнители", false);

        lib.set_artist_photo(with.id, Some("artists/kino.jpg")).unwrap();
        lib.set_artist_photo(missed.id, None).unwrap();
        assert_eq!(
            lib.artist_photo(with.id).unwrap().as_deref(),
            Some("artists/kino.jpg")
        );
        assert_eq!(lib.artist_photo(missed.id).unwrap(), None);
        let all = lib.artist_photos().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[&with.id], "artists/kino.jpg");

        let week_ago = now_ms() - 7 * 86_400_000;
        let ids = |q: Vec<(ArtistId, String)>| q.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        assert_eq!(ids(lib.artists_needing_photo(week_ago, week_ago, 10).unwrap()), vec![fresh.id, remote.id]);
        // An old miss is retried after the untried ones.
        assert_eq!(
            ids(lib.artists_needing_photo(now_ms() + 1000, week_ago, 10).unwrap()),
            vec![fresh.id, remote.id, missed.id]
        );
    }

    #[test]
    fn an_unverified_photo_is_asked_again_by_the_next_session() {
        let lib = Library::open_in_memory().unwrap();
        let a = artist_with_file(&lib, "Aminé", true);
        let session = now_ms() - 1000;
        lib.set_artist_photo_unverified(a.id).unwrap();
        let week_ago = now_ms() - 7 * 86_400_000;
        // Not again in this session, nor as a miss of days.
        assert!(lib.artists_needing_photo(week_ago, session, 10).unwrap().is_empty());
        // The next session asks again.
        assert_eq!(lib.artists_needing_photo(week_ago, now_ms() + 1000, 10).unwrap().len(), 1);
        // A real miss after that waits its days, even for a new session.
        lib.set_artist_photo(a.id, None).unwrap();
        assert!(lib.artists_needing_photo(week_ago, now_ms() + 1000, 10).unwrap().is_empty());
    }

    #[test]
    fn a_role_gets_no_photo_lookups() {
        let lib = Library::open_in_memory().unwrap();
        let riko = artist_with_file(&lib, "リコ", true);
        let person = artist_with_file(&lib, "富田美憂", true);
        let found = crate::LatinFound { character: true, ..crate::LatinFound::default() };
        lib.set_artist_latin(riko.id, &found).unwrap();
        let week_ago = now_ms() - 7 * 86_400_000;
        let due: Vec<ArtistId> = lib.artists_needing_photo(week_ago, week_ago, 10).unwrap().into_iter().map(|(id, _)| id).collect();
        assert_eq!(due, vec![person.id]);
        assert!(!lib.artist_photo_due(riko.id, week_ago, week_ago).unwrap());
        assert!(lib.artist_photo_due(person.id, week_ago, week_ago).unwrap());
        // Another source is recorded as such.
        lib.set_artist_photo_from(person.id, Some("p.jpg"), "wikimedia").unwrap();
        assert_eq!(lib.artist_photo(person.id).unwrap().as_deref(), Some("p.jpg"));
    }

    #[test]
    fn a_photo_whose_file_is_gone_is_forgotten() {
        let lib = Library::open_in_memory().unwrap();
        let a = artist_with_file(&lib, "Кино", true);
        let b = artist_with_file(&lib, "Земфира", true);
        lib.set_artist_photo(a.id, Some("gone.jpg")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        lib.set_custom_artist_photo(b.id, dir.path(), &[0xFF, 0xD8, 0xFF]).unwrap();
        assert_eq!(lib.forget_artist_photos(&[a.id, b.id]).unwrap(), 1, "a locked photo stays");
        assert_eq!(lib.artist_photo(a.id).unwrap(), None);
        let week_ago = now_ms() - 7 * 86_400_000;
        let due: Vec<ArtistId> = lib.artists_needing_photo(week_ago, week_ago, 10).unwrap().into_iter().map(|(id, _)| id).collect();
        assert_eq!(due, vec![a.id]);
    }

    #[test]
    fn locked_photo_survives_enrichment_and_clear_requeues() {
        let lib = Library::open_in_memory().unwrap();
        let a = artist_with_file(&lib, "Christopher Larkin", true);
        let dir = tempfile::tempdir().unwrap();
        let png = [0x89, b'P', b'N', b'G', 1, 2];
        let path = lib.set_custom_artist_photo(a.id, dir.path(), &png).unwrap();
        assert!(path.extension().is_some_and(|e| e == "png"));
        assert!(lib.artist_photo_locked(a.id).unwrap());

        // Enrichment can neither replace the photo nor record a miss over it.
        lib.set_artist_photo(a.id, Some("deezer.jpg")).unwrap();
        lib.set_artist_photo(a.id, None).unwrap();
        assert_eq!(lib.artist_photo(a.id).unwrap(), Some(path.display().to_string()));
        assert!(lib.artists_needing_photo(now_ms() + 1000, now_ms() + 1000, 10).unwrap().is_empty());

        lib.clear_artist_photo(a.id).unwrap();
        assert!(!path.exists());
        assert!(!lib.artist_photo_locked(a.id).unwrap());
        assert_eq!(lib.artist_photo(a.id).unwrap(), None);
        assert_eq!(lib.artists_needing_photo(now_ms() - 1000, now_ms() - 1000, 10).unwrap().len(), 1);
        lib.set_artist_photo(a.id, Some("deezer.jpg")).unwrap();
        assert_eq!(lib.artist_photo(a.id).unwrap().as_deref(), Some("deezer.jpg"));
    }

    #[test]
    fn migration_drops_unverified_photos_but_keeps_misses() {
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        for sql in &crate::migrations::MIGRATIONS[..2] {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute(
            "INSERT INTO artist_photo(artist_id, path, fetched_at) VALUES ('a', 'x.jpg', 5), ('b', NULL, 7)",
            [],
        )
        .unwrap();
        crate::migrations::migrate(&mut conn).unwrap();
        let rows: Vec<(String, Option<String>, i64, bool)> = conn
            .prepare_cached("SELECT artist_id, path, fetched_at, photo_locked FROM artist_photo ORDER BY artist_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows[0], ("a".into(), None, 0, false));
        // Still a miss; migration 32 makes it due again.
        assert_eq!(rows[1], ("b".into(), None, 0, false));
    }

    /// Migration 32: every miss of an older library is asked again at the
    /// next start; a photo, and a miss the user locked, stay as they are.
    #[test]
    fn migration_makes_old_misses_due_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        drop(Library::open(&path).unwrap());
        let recent = now_ms() - 86_400_000;
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "INSERT INTO artist_photo(artist_id, path, source, fetched_at, photo_locked) VALUES
                    ('a', 'a.jpg', 'deezer', ?1, 0), ('b', NULL, 'deezer', ?1, 0),
                    ('c', NULL, 'unverified', ?1, 0), ('d', NULL, 'custom', ?1, 1)",
                [recent],
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 31).unwrap();
        }
        let lib = Library::open(&path).unwrap();
        let rows: Vec<(String, i64)> = lib
            .read(|c| {
                let mut stmt = c.prepare("SELECT artist_id, fetched_at FROM artist_photo ORDER BY artist_id")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
            })
            .unwrap();
        assert_eq!(rows, vec![("a".into(), recent), ("b".into(), 0), ("c".into(), 0), ("d".into(), recent)]);
    }

    /// A miss with `fetched_at = 0` (what migration 32 leaves) is due for
    /// the queue and for an opened artist page alike.
    #[test]
    fn a_miss_of_time_zero_is_due() {
        let lib = Library::open_in_memory().unwrap();
        let a = artist_with_file(&lib, "Noize MC", true);
        lib.set_artist_photo(a.id, None).unwrap();
        let retry = now_ms() - 14 * 86_400_000;
        assert!(lib.artists_needing_photo(retry, now_ms() - 1000, 10).unwrap().is_empty());
        assert!(!lib.artist_photo_due(a.id, retry, now_ms() - 1000).unwrap());
        lib.write(|tx| {
            tx.execute("UPDATE artist_photo SET fetched_at = 0 WHERE path IS NULL AND photo_locked = 0", [])?;
            Ok(())
        })
        .unwrap();
        assert_eq!(lib.artists_needing_photo(retry, now_ms() - 1000, 10).unwrap().len(), 1);
        assert!(lib.artist_photo_due(a.id, retry, now_ms() - 1000).unwrap());
    }
}
