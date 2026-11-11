//! Ordered SQL migrations, tracked with `PRAGMA user_version`. A library
//! that has a schema already is copied first, once per day and version, to
//! `backup-before-schema-v<n>-<yyyymmdd>/library.sqlite` next to it
//! (`VACUUM INTO`, as the one-time repairs do).

use std::path::PathBuf;

use rusqlite::{Connection, OptionalExtension};

use crate::error::{LibraryError, Result};

/// Migration `n` (1-based) is `MIGRATIONS[n - 1]`. Never edit a released entry,
/// append a new one.
pub(crate) const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_lyrics_artist_photo.sql"),
    include_str!("../migrations/0003_artist_photo_lock.sql"),
    include_str!("../migrations/0004_lyrics_sources.sql"),
    include_str!("../migrations/0005_deleted_tracks.sql"),
    include_str!("../migrations/0006_streaming.sql"),
    include_str!("../migrations/0007_remote_likes.sql"),
    include_str!("../migrations/0008_lyricsplus_refetch.sql"),
    include_str!("../migrations/0009_recommendations.sql"),
    include_str!("../migrations/0010_confirm_queue.sql"),
    include_str!("../migrations/0011_artist_aliases.sql"),
    include_str!("../migrations/0012_event_outcome.sql"),
    include_str!("../migrations/0013_event_attention.sql"),
    include_str!("../migrations/0014_track_aliases.sql"),
    include_str!("../migrations/0015_recs_graph.sql"),
    include_str!("../migrations/0016_lyrics_align.sql"),
    include_str!("../migrations/0017_lyrics_show.sql"),
    include_str!("../migrations/0018_source_quality.sql"),
    include_str!("../migrations/0019_unwanted_tracks.sql"),
    include_str!("../migrations/0020_quality_keep.sql"),
    include_str!("../migrations/0021_same_audio.sql"),
    include_str!("../migrations/0022_recordings.sql"),
    include_str!("../migrations/0023_track_fx.sql"),
    include_str!("../migrations/0024_wishlist_better_than.sql"),
    include_str!("../migrations/0025_album_selection.sql"),
    include_str!("../migrations/0026_artist_latin.sql"),

];

/// Number of migrations known to this build (= the schema version it produces).
pub const SCHEMA_VERSION: u32 = MIGRATIONS.len() as u32;

pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Brings the database up to [`SCHEMA_VERSION`]; each migration runs in its
/// own transaction together with the version bump.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    let found = user_version(conn)?;
    if found > SCHEMA_VERSION {
        return Err(LibraryError::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found > 0 && found < SCHEMA_VERSION {
        backup(conn, SCHEMA_VERSION)?;
    }
    for (idx, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        let version = idx as u32 + 1;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        log::info!("library schema migrated to v{version}");
    }
    Ok(())
}

/// Copies a library file before it is migrated to `version` (nothing in
/// memory, nor when today's copy exists).
fn backup(conn: &Connection, version: u32) -> Result<Option<PathBuf>> {
    let file: Option<String> = conn
        .query_row("SELECT file FROM pragma_database_list WHERE name = 'main'", [], |r| r.get(0))
        .optional()?;
    let Some(db) = file.filter(|f| !f.is_empty()).map(PathBuf::from) else { return Ok(None) };
    let dir = db
        .parent()
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(format!("backup-before-schema-v{version}-{}", crate::repairs::ymd(crate::util::now_ms())));
    let to = dir.join("library.sqlite");
    if to.exists() {
        return Ok(Some(to));
    }
    std::fs::create_dir_all(&dir)?;
    conn.execute("VACUUM INTO ?1", [to.to_string_lossy()])?;
    log::info!("library: backed up to {} before the schema v{version}", to.display());
    Ok(Some(to))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library of an older schema is copied before it changes, keeps its
    /// events with the new columns empty, and opening it again changes
    /// nothing.
    #[test]
    fn a_migration_backs_up_first_and_runs_once() {
        let dir = std::env::temp_dir().join(format!("nikgob-migrate-{}", nikgob_core::model::EventId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.sqlite");
        {
            let mut c = Connection::open(&db).unwrap();
            let tx = c.transaction().unwrap();
            for sql in &MIGRATIONS[..11] {
                tx.execute_batch(sql).unwrap();
            }
            tx.pragma_update(None, "user_version", 11).unwrap();
            tx.execute(
                "INSERT INTO event(id, ts, track_id, type, context, device) VALUES ('e1', 1, 't1', 'start', 'album', 'd')",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let mut c = Connection::open(&db).unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(user_version(&c).unwrap(), SCHEMA_VERSION);
        let backups: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().is_dir()).collect();
        assert_eq!(backups.len(), 1);
        let copy = Connection::open(backups[0].path().join("library.sqlite")).unwrap();
        assert_eq!(user_version(&copy).unwrap(), 11, "the copy is the library as it was");
        let row: (i64, Option<String>, Option<String>, Option<String>) = c
            .query_row("SELECT COUNT(*), MAX(origin), MAX(context_id), MAX(end_reason) FROM event", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        assert_eq!(row, (1, None, None, None));
        migrate(&mut c).unwrap();
        assert_eq!(user_version(&c).unwrap(), SCHEMA_VERSION);
        drop((c, copy));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A library of schema v12 (outcomes, no attention) is copied to
    /// `backup-before-schema-v13-*` first, keeps its plays as they were and
    /// gets the attention and volume columns empty.
    #[test]
    fn the_attention_columns_come_empty_after_a_backup() {
        let dir = std::env::temp_dir().join(format!("nikgob-migrate-v13-{}", nikgob_core::model::EventId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.sqlite");
        {
            let mut c = Connection::open(&db).unwrap();
            let tx = c.transaction().unwrap();
            for sql in &MIGRATIONS[..12] {
                tx.execute_batch(sql).unwrap();
            }
            tx.pragma_update(None, "user_version", 12).unwrap();
            tx.execute(
                "INSERT INTO event(id, ts, track_id, type, position_ms, context, device, origin, context_id, end_reason)
                 VALUES ('e1', 1, 't1', 'start', 0, 'album', 'd', 'manual', 'release:r', NULL),
                        ('e2', 2, 't1', 'skip', 5000, 'album', 'd', NULL, NULL, 'user_next')",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let mut c = Connection::open(&db).unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(user_version(&c).unwrap(), SCHEMA_VERSION);
        let backups: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(backups.len(), 1);
        assert!(backups[0].starts_with(&format!("backup-before-schema-v{SCHEMA_VERSION}-")), "{backups:?}");
        let copy = Connection::open(dir.join(&backups[0]).join("library.sqlite")).unwrap();
        assert_eq!(user_version(&copy).unwrap(), 12);
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>, Option<f64>, Option<f64>)> = c
            .prepare("SELECT id, origin, context_id, end_reason, attention, volume FROM event ORDER BY ts")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows, vec![
            ("e1".to_owned(), Some("manual".to_owned()), Some("release:r".to_owned()), None, None, None),
            ("e2".to_owned(), None, None, Some("user_next".to_owned()), None, None),
        ]);
        drop((c, copy));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A copy of a real library migrated, its rows counted before and after
    /// and opened twice: `NIKGOB_REAL_LIBRARY=path/to/library.sqlite cargo
    /// test -p nikgob-library a_real_library -- --ignored --nocapture`. The
    /// file itself is never written (the copy, with its -wal and -shm, lives
    /// in a temporary directory).
    #[test]
    #[ignore = "needs NIKGOB_REAL_LIBRARY"]
    fn a_real_library_migrates_with_its_rows() {
        let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else { return };
        let dir = std::env::temp_dir().join(format!("nikgob-real-migrate-{}", nikgob_core::model::EventId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("library.sqlite");
        std::fs::copy(&src, &db).unwrap();
        for ext in ["-wal", "-shm"] {
            let mut side = src.clone();
            side.push(ext);
            if std::path::Path::new(&side).exists() {
                let mut to = db.clone().into_os_string();
                to.push(ext);
                std::fs::copy(&side, to).unwrap();
            }
        }
        let counts = |c: &Connection| -> Vec<(String, i64)> {
            let tables: Vec<String> = c
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE '%fts%' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            tables.into_iter().map(|t| { let n = c.query_row(&format!("SELECT COUNT(*) FROM \"{t}\""), [], |r| r.get(0)).unwrap(); (t, n) }).collect()
        };
        let mut c = Connection::open(&db).unwrap();
        let before = counts(&c);
        let from = user_version(&c).unwrap();
        migrate(&mut c).unwrap();
        let after = counts(&c);
        let to = user_version(&c).unwrap();
        migrate(&mut c).unwrap();
        let again = counts(&c);
        let cols: Vec<String> = c.prepare("SELECT name FROM pragma_table_info('event')").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap();
        let nulls: i64 = c.query_row("SELECT COUNT(*) FROM event WHERE origin IS NULL AND context_id IS NULL AND end_reason IS NULL", [], |r| r.get(0)).unwrap();
        let backups: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        println!("schema v{from} -> v{to}, again v{}; backups {backups:?}", user_version(&c).unwrap());
        println!("event columns: {cols:?}; rows with the new columns empty: {nulls}");
        for ((t, b), (_, a)) in before.iter().zip(&after) {
            println!("  {t}: {b} -> {a}");
        }
        assert_eq!(before, after, "no row changes");
        assert_eq!(after, again, "a second run changes nothing");
        assert_eq!(backups.len(), usize::from(from < SCHEMA_VERSION));
        drop(c);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
