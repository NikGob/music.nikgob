//! Ordered SQL migrations, tracked with `PRAGMA user_version`.

use rusqlite::Connection;

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
