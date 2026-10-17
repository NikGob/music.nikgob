//! Query-side types: sort specs, list rows, reports.

use nikgob_core::model::{
    Artist, Codec, PlaylistEntry, PlaylistId, Release, Source, SourceId, SourceKind, TrackId,
    UnixMs,
};
use serde::{Deserialize, Serialize};

/// A sort key plus direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort<K> {
    pub key: K,
    pub desc: bool,
}

impl<K> Sort<K> {
    pub const fn asc(key: K) -> Self {
        Self { key, desc: false }
    }

    pub const fn desc(key: K) -> Self {
        Self { key, desc: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackSortKey {
    #[default]
    Title,
    /// Artist, then album, disc, track number.
    Artist,
    /// Album, then disc, track number.
    Album,
    Added,
    Duration,
    Quality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ReleaseSortKey {
    /// Artist, then year, then title.
    #[default]
    Artist,
    /// Year (unknown years last), then artist, title.
    Year,
    Title,
    Added,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ArtistSortKey {
    #[default]
    Name,
    Added,
}

pub type TrackSort = Sort<TrackSortKey>;
pub type ReleaseSort = Sort<ReleaseSortKey>;
pub type ArtistSort = Sort<ArtistSortKey>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TrackFilter {
    /// Every track, including those without a playable file.
    #[default]
    All,
    /// Only tracks whose active source is a local file.
    LocalOnly,
    /// Tracks without a local file (stream-only or file trashed).
    NoLocalFile,
}

/// One line of a (virtualised) track list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackRow {
    pub id: TrackId,
    pub title: String,
    /// Credit rendered as "A feat. B".
    pub artist: String,
    pub album: Option<String>,
    pub release_id: Option<nikgob_core::model::ReleaseId>,
    pub cover_ref: Option<String>,
    pub disc_no: u32,
    pub track_no: u32,
    pub duration_ms: Option<u32>,
    /// `quality_score` of the active source.
    pub quality: Option<i32>,
    pub codec: Option<Codec>,
    pub active_kind: Option<SourceKind>,
    /// The active source is a local file.
    pub has_local: bool,
    pub liked: bool,
    /// The first credited artist, when linked to an artist row.
    pub artist_id: Option<nikgob_core::model::ArtistId>,
    /// The video id of the active source when it is a YouTube one.
    #[serde(default)]
    pub youtube_id: Option<String>,
}

impl TrackRow {
    /// What plays is a YouTube music video, not the song (a liked clip
    /// whose song YouTube Music does not have).
    pub fn is_clip(&self) -> bool {
        self.active_kind == Some(SourceKind::Youtube)
            && self.quality == Some(nikgob_core::model::score::STREAM_YOUTUBE_CLIP)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRow {
    pub release: Release,
    /// Tracks of this release that have a local file.
    pub local_tracks: u32,
    /// Tracks of this release known to the library.
    pub known_tracks: u32,
    /// Every (local) track of the album is liked; see [`crate::Library::like_release`].
    pub liked: bool,
}

impl ReleaseRow {
    /// "3 из 12 у тебя".
    pub fn progress(&self) -> ReleaseProgress {
        ReleaseProgress::new(
            self.local_tracks,
            self.known_tracks,
            self.release.total_tracks,
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseProgress {
    /// Tracks with a local file ("3 из 12 у тебя").
    pub local: u32,
    /// Tracks in the library: with a file, or liked / in a playlist from a
    /// service (stream only, "стрим").
    #[serde(default)]
    pub in_library: u32,
    /// `total_tracks` of the release, or the number of known tracks if larger/unknown.
    pub total: u32,
}

impl ReleaseProgress {
    pub(crate) fn new(local: u32, known: u32, total_tracks: Option<u32>) -> Self {
        Self {
            local,
            in_library: known,
            total: total_tracks.unwrap_or(0).max(known),
        }
    }

    pub fn is_complete(&self) -> bool {
        self.total > 0 && self.local >= self.total
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistRow {
    pub artist: Artist,
    pub release_count: u32,
    /// Credited tracks that have a local file.
    pub local_track_count: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub tracks: Vec<TrackRow>,
    pub releases: Vec<ReleaseRow>,
    pub artists: Vec<ArtistRow>,
    pub playlists: Vec<PlaylistSummary>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.releases.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistKind {
    /// The pinned virtual "Любимые" playlist (liked tracks).
    Liked,
    User,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    /// `None` for the virtual liked playlist.
    pub id: Option<PlaylistId>,
    pub kind: PlaylistKind,
    pub name: String,
    pub cover: Option<String>,
    pub track_count: u32,
    pub updated_at: UnixMs,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistEntryRow {
    pub entry: PlaylistEntry,
    pub track: TrackRow,
}

/// Aggregates over the event log, for recommendations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackStats {
    /// Number of `start` events.
    pub plays: u32,
    pub completes: u32,
    pub skips: u32,
    /// Latest `start` or `complete`.
    pub last_played: Option<UnixMs>,
}

/// A source sitting in Trash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashEntry {
    pub source: Source,
    /// Where the import crate moved the file inside `Trash/`, if it told us.
    pub trash_path: Option<String>,
    pub trashed_at: UnixMs,
    pub track_title: String,
    pub artist: String,
}

/// A track the user deleted ("Корзина").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeletedTrack {
    pub track: TrackRow,
    pub deleted_at: UnixMs,
}

/// What [`crate::Library::purge_track`] removed; the caller deletes the files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgedTrack {
    /// Files of trashed sources, inside `Trash/` (or their original place when
    /// the file was already gone at trashing time and nothing lives there now).
    pub trash_files: Vec<String>,
    /// Files of live local sources (a permanent delete straight from the library).
    pub live_files: Vec<String>,
    /// Where every local file of the track lived in the library (trashed ones
    /// too): the album folders to tidy up.
    pub places: Vec<String>,
    /// Content hash of the lossless master, when the track had one: its file
    /// and sidecar in `.musiclib/masters/` go too.
    pub master_hash: Option<String>,
    /// The release went with its last track.
    pub release_removed: bool,
}

/// One finished job of "Загрузки → История".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// Row id, increasing.
    pub seq: i64,
    /// `import` or `torrent`.
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub error: Option<String>,
    pub at: UnixMs,
}

/// What [`crate::Library::purge_trash`] removed from the DB; the caller deletes the files.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgedFile {
    pub source_id: SourceId,
    pub track_id: TrackId,
    /// File to delete from disk: the recorded `trash_path`, else (for local files)
    /// the original locator. `None` for stream sources, which have no file.
    pub path: Option<String>,
    pub kind: SourceKind,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportReport {
    pub playlists: usize,
    pub playlist_entries: usize,
    pub likes: usize,
    pub events: usize,
    /// Stream-only tracks written to `streams.json` (liked or in playlists).
    pub stream_tracks: usize,
    /// Files whose content actually changed on disk.
    pub files_written: usize,
}

/// A library playlist linked to a playlist in a streaming service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistRemote {
    pub playlist_id: nikgob_core::model::PlaylistId,
    pub remote: nikgob_core::streaming::RemoteId,
    pub sync_mode: nikgob_core::streaming::PlaylistSyncMode,
    /// The service's snapshot id / etag at the last sync.
    pub remote_version: Option<String>,
    pub synced_at: Option<nikgob_core::model::UnixMs>,
    /// The content at the last sync (JSON, shape owned by the sync code):
    /// the base of a three-way merge.
    pub base_json: Option<String>,
}

/// One liked track of a streaming service as the last pull saw it (table
/// `remote_like`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteLike {
    pub remote: nikgob_core::streaming::RemoteId,
    pub track_id: TrackId,
    /// The local like was made by a pull from this service (so the service
    /// may take it back); `false` = the user liked it here (or before).
    pub pulled: bool,
    pub seen_at: UnixMs,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    /// Rows inserted or overwritten because the incoming copy won.
    pub applied: usize,
    /// Rows ignored because the local copy was newer (or identical).
    pub skipped: usize,
    pub events_added: usize,
    /// Stream tracks recreated (or re-linked) from `streams.json`.
    pub stream_tracks: usize,
    /// Unparsable lines in `events.jsonl`.
    pub invalid_lines: usize,
}
