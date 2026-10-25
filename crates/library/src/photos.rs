//! Artist photo cache: where the downloaded picture lives, or that there is none.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nikgob_core::model::{ArtistId, UnixMs};
use rusqlite::{OptionalExtension, params};

use crate::db::Library;
use crate::error::Result;
use crate::util::{get_id, now_ms};

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

    /// Records the photo of an artist; `None` records "looked up, none found".
    /// A photo the user locked is never touched (see [`Library::set_custom_artist_photo`]).
    pub fn set_artist_photo(&self, artist: ArtistId, path: Option<&str>) -> Result<()> {
        self.write(|tx| {
            tx.execute(
                "INSERT INTO artist_photo(artist_id, path, fetched_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(artist_id) DO UPDATE SET path = excluded.path,
                    fetched_at = excluded.fetched_at
                 WHERE photo_locked = 0",
                params![artist.to_string(), path, now_ms()],
            )?;
            Ok(())
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

    /// Artists with a local track and no photo: never tried, or a miss older
    /// than `retry_before` (unix ms). Never-tried first, most tracks first.
    pub fn artists_needing_photo(
        &self,
        retry_before: UnixMs,
        limit: u32,
    ) -> Result<Vec<(ArtistId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT a.id, a.name FROM artist a LEFT JOIN artist_photo p ON p.artist_id = a.id
                  WHERE EXISTS (SELECT 1 FROM track_artist ta JOIN track t ON t.id = ta.track_id
                                 WHERE ta.artist_id = a.id AND t.has_local = 1)
                    AND (p.artist_id IS NULL OR (p.path IS NULL AND p.fetched_at < ?1))
                  ORDER BY p.fetched_at IS NOT NULL, p.fetched_at,
                           (SELECT COUNT(*) FROM track_artist ta WHERE ta.artist_id = a.id) DESC,
                           a.seq
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![retry_before, limit], |r| {
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
        let _streaming_only = artist_with_file(&lib, "Remote", false);

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
        assert_eq!(ids(lib.artists_needing_photo(week_ago, 10).unwrap()), vec![fresh.id]);
        // An old miss is retried after the untried ones.
        assert_eq!(
            ids(lib.artists_needing_photo(now_ms() + 1000, 10).unwrap()),
            vec![fresh.id, missed.id]
        );
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
        assert!(lib.artists_needing_photo(now_ms() + 1000, 10).unwrap().is_empty());

        lib.clear_artist_photo(a.id).unwrap();
        assert!(!path.exists());
        assert!(!lib.artist_photo_locked(a.id).unwrap());
        assert_eq!(lib.artist_photo(a.id).unwrap(), None);
        assert_eq!(lib.artists_needing_photo(now_ms() - 1000, 10).unwrap().len(), 1);
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
        assert_eq!(rows[1], ("b".into(), None, 7, false));
    }
}
