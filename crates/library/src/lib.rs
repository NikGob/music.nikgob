//! The library database: single source of truth for the catalog (artists,
//! releases, tracks, sources) and for user data (playlists, likes, history).
//!
//! * [`Library`] is a cheap-to-clone handle: one writer connection plus a small
//!   pool of read-only connections (WAL), so UI reads never wait behind an
//!   import transaction.
//! * The schema (with the backup/network "tails") is created and
//!   migrated by [`migrations`] using `PRAGMA user_version`.
//! * User data is CRDT-friendly: row-level last-writer-wins with tombstones,
//!   fractional-index playlist order, append-only events. See [`userdata`] for
//!   export/import (the DB can be rebuilt from files + exported user data).
//! * Full-text search (FTS5) works for Cyrillic; see [`search`].

#[macro_use]
mod macros;

mod albums;
mod aliases;
mod catalog;
mod confirm;
mod credits;
mod db;
mod derived;
mod error;
mod events;
pub mod fractional;
mod joblog;
mod latin;
mod lyrics;
mod masters;
mod migrations;
mod nodes;
mod photos;
mod places;
mod playlists;
mod prints;
mod quality;
mod rec_facts;
mod graph;
mod recordings;
mod recs;
mod repairs;
pub mod search;
mod selection;
mod storage;
mod streams;
pub mod sync;
mod track_fx;
mod trash;

mod types;
mod unwanted;
mod userdata;
mod util;

#[cfg(test)]
mod tests;

pub use db::Library;
pub use lyrics::{AlignCounts, AlignedLyrics, StoredLyrics};
pub use error::{LibraryError, Result};
pub use joblog::JOB_LOG_KEEP;
pub use latin::{ArtistLatin, LatinFound, ROMAJI};
pub use migrations::SCHEMA_VERSION;
pub use nodes::NodeRow;
pub use places::{PLACES_FROM_WISH_KEY, PlaceChange};
pub use playlists::LIKED_PLAYLIST_NAME;
pub use prints::{AudioPrint, NearPrint, PrintTodo, SameAudioOffer, SameSide, WrongAudio};
pub use quality::{Conversion, FakeOffer, QualityTodo};
pub use storage::LocalFileSizes;
pub use sync::{SyncApplied, SyncConflicts, SyncCursors, SyncGroup, SyncHeads, SyncPage, SyncRow, SyncTable, TABLES as SYNC_TABLES, page_ahead, sync_table};
pub use rec_facts::{LyricsLevel, LyricsSample, PlaylistTracks, TrackFactsRow};
pub use graph::{GraphEntry, GraphFetch, GraphRow, GraphTag};
pub use recordings::{PrintToLink, RecordingSide};
pub use recs::RecTrack;
pub use selection::{ReleaseChoice, TrackChoice, TrackLibraryWas, WholeUndo};
pub use unwanted::Unwanted;
pub use streams::{LikedClip, MatchRow, TrackSnapshot, event_id, is_web_ref, other_scripts};
pub use types::{
    AlbumPlan, AlbumRules, AnalysisCounts, AnalysisTodo, ArtistRow, ArtistSort, ArtistSortKey, ConfirmRow, DeletedTrack, ExportReport, ImportReport, JobLogEntry,
    PlaylistEntryRow, PlaylistKind, PlaylistRemote, PlaylistSummary, PurgedFile, PurgedTrack,
    ListingEntry, Membership, RecFeedback, ReleaseListing, ReleaseProgress, RemoteLike, SketchRow, StreamAnalysisTodo,
    ReleaseRow, ReleaseSort, ReleaseSortKey, SearchResults, Sort, TrackFilter, TrackRow,
    TrackSort, TrackSortKey, TrackStats, TrashEntry,
};
pub use userdata::{
    ALBUM_CHOICES_FILE, EVENTS_FILE, FINGERPRINT_FILE, LIKES_FILE, PLAYLISTS_FILE, REC_FEEDBACK_FILE, RECORDINGS_FILE, SAVED_FILE, STREAMS_FILE,
    USERDATA_VERSION,
};
pub use util::{fold, now_ms, sort_key};
