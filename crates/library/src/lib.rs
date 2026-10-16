//! The library database: single source of truth for the catalog (artists,
//! releases, tracks, sources) and for user data (playlists, likes, history).
//!
//! * [`Library`] is a cheap-to-clone handle: one writer connection plus a small
//!   pool of read-only connections (WAL), so UI reads never wait behind an
//!   import transaction.
//! * The schema (PLAN §4, with the backup/network "tails") is created and
//!   migrated by [`migrations`] using `PRAGMA user_version`.
//! * User data is CRDT-friendly: row-level last-writer-wins with tombstones,
//!   fractional-index playlist order, append-only events. See [`userdata`] for
//!   export/import (the DB can be rebuilt from files + exported user data).
//! * Full-text search (FTS5) works for Cyrillic; see [`search`].

#[macro_use]
mod macros;

mod catalog;
mod db;
mod derived;
mod error;
mod events;
pub mod fractional;
mod joblog;
mod lyrics;
mod masters;
mod migrations;
mod photos;
mod playlists;
pub mod search;
mod streams;
mod trash;
mod types;
mod userdata;
mod util;

#[cfg(test)]
mod tests;

pub use db::Library;
pub use lyrics::StoredLyrics;
pub use error::{LibraryError, Result};
pub use joblog::JOB_LOG_KEEP;
pub use migrations::SCHEMA_VERSION;
pub use playlists::LIKED_PLAYLIST_NAME;
pub use types::{
    ArtistRow, ArtistSort, ArtistSortKey, DeletedTrack, ExportReport, ImportReport, JobLogEntry,
    PlaylistEntryRow, PlaylistKind, PlaylistRemote, PlaylistSummary, PurgedFile, PurgedTrack,
    ReleaseProgress, RemoteLike,
    ReleaseRow, ReleaseSort, ReleaseSortKey, SearchResults, Sort, TrackFilter, TrackRow,
    TrackSort, TrackSortKey, TrackStats, TrashEntry,
};
pub use userdata::{EVENTS_FILE, LIKES_FILE, PLAYLISTS_FILE, STREAMS_FILE, USERDATA_VERSION};
pub use util::{fold, now_ms, sort_key};
