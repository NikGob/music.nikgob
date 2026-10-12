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

    /// Local tracks without lyrics that are worth a lookup: never tried, or
    /// "not found" markers older than `retry_before` (unix ms). Untried first.
    pub fn tracks_needing_lyrics(&self, retry_before: UnixMs, limit: u32) -> Result<Vec<TrackId>> {
        self.read(|c| {
            let mut stmt = c.prepare(
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
