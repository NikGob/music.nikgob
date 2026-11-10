//! How a track is played when not as recorded (table `track_fx`, migration
//! 0023): the version chosen in its «Скорость» menu, by name. The library
//! only keeps the name; what a version sounds like is the player's matter.

use nikgob_core::model::TrackId;
use rusqlite::params;

use crate::db::Library;
use crate::error::Result;
use crate::util::now_ms;

impl Library {
    /// Every track with a chosen version, and its name.
    pub fn track_fx(&self) -> Result<Vec<(TrackId, String)>> {
        self.read(|c| {
            let mut stmt = c.prepare("SELECT track_id, variant FROM track_fx")?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            // A row whose id does not read is not a track of this library.
            Ok(rows.into_iter().filter_map(|(id, variant)| Some((id.parse().ok()?, variant))).collect())
        })
    }

    /// Remembers `variant` for `track`; `None` forgets it (as recorded).
    pub fn set_track_fx(&self, track: TrackId, variant: Option<&str>) -> Result<()> {
        self.write(|tx| {
            match variant {
                Some(v) => tx.execute(
                    "INSERT INTO track_fx(track_id, variant, updated_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(track_id) DO UPDATE SET variant = excluded.variant, updated_at = excluded.updated_at",
                    params![track.to_string(), v, now_ms()],
                )?,
                None => tx.execute("DELETE FROM track_fx WHERE track_id = ?1", [track.to_string()])?,
            };
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::{MIGRATIONS, SCHEMA_VERSION};

    #[test]
    fn a_choice_is_kept_changed_and_forgotten() {
        let lib = Library::open_in_memory().unwrap();
        let (a, b) = (TrackId::new(), TrackId::new());
        assert!(lib.track_fx().unwrap().is_empty());
        lib.set_track_fx(a, Some("fast")).unwrap();
        lib.set_track_fx(b, Some("slow_echo")).unwrap();
        lib.set_track_fx(a, Some("nightcore")).unwrap();
        let mut rows = lib.track_fx().unwrap();
        rows.sort();
        let mut want = vec![(a, "nightcore".to_owned()), (b, "slow_echo".to_owned())];
        want.sort();
        assert_eq!(rows, want);
        lib.set_track_fx(a, None).unwrap();
        lib.set_track_fx(a, None).unwrap();
        assert_eq!(lib.track_fx().unwrap(), vec![(b, "slow_echo".to_owned())]);
    }

    /// A library of the schema before this table opens, gets the table
    /// empty and keeps what it had.
    #[test]
    fn an_older_library_gets_the_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            for (i, sql) in MIGRATIONS.iter().take(22).enumerate() {
                conn.execute_batch(sql).unwrap();
                conn.pragma_update(None, "user_version", i as u32 + 1).unwrap();
            }
            conn.execute_batch(
                "INSERT INTO track(id, title, title_key, artist_credit, artist_display, artist_key, added_at)
                 VALUES ('00000000-0000-0000-0000-000000000001', 'Song', 'song', '[]', '', '', 1);",
            )
            .unwrap();
        }
        let lib = Library::open(&path).unwrap();
        assert_eq!(lib.schema_version().unwrap(), SCHEMA_VERSION);
        assert!(lib.track_fx().unwrap().is_empty());
        let track: TrackId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        lib.set_track_fx(track, Some("slow")).unwrap();
        assert_eq!(lib.track_fx().unwrap(), vec![(track, "slow".to_owned())]);
        let songs: i64 = lib.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM track", [], |r| r.get(0))?)).unwrap();
        assert_eq!(songs, 1);
    }
}
