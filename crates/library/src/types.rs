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
    /// Every credited artist with the text between them (`artist` is its
    /// display).
    #[serde(default)]
    pub credit: nikgob_core::model::ArtistCredit,
    /// The video id of the active source when it is a YouTube one.
    #[serde(default)]
    pub youtube_id: Option<String>,
    /// What the spectrum check of the active source found (a "FLAC" made
    /// from an MP3); `None` until it is checked.
    #[serde(default)]
    pub check: Option<nikgob_core::quality::CheckSummary>,
}

impl TrackRow {
    /// The first credited artist, when linked to an artist row.
    pub fn artist_id(&self) -> Option<nikgob_core::model::ArtistId> {
        self.credit.primary().and_then(|p| p.artist_id)
    }

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
    /// The album is liked: saved, or, while album likes like the tracks
    /// too, every (local) track of it is; see [`crate::Library::like_release`].
    pub liked: bool,
    /// Rows of its page ([`crate::Library::tracks_of_release_all`]: outside
    /// the library too).
    pub rows: u32,
    /// The user's songs of it: rows of its page that are library tracks and
    /// not left out of it by hand (`crate::selection`).
    #[serde(default)]
    pub wanted: u32,
    /// Of them, the ones with a file.
    #[serde(default)]
    pub wanted_local: u32,
    /// The whole album is the user's: added whole or saved (its own like).
    #[serde(default)]
    pub whole: bool,
    /// What its page shows while only some of it is the user's:
    /// `Some(true)` the whole album, `Some(false)` the user's songs, `None`
    /// the setting ([`crate::Library::set_release_view`]).
    #[serde(default)]
    pub view: Option<bool>,
}

impl ReleaseRow {
    /// "3 из 12 у тебя", as its page counts ([`ReleaseListing`]), and the
    /// user's songs of it.
    pub fn progress(&self) -> ReleaseProgress {
        ReleaseProgress::new(self.local_tracks, self.known_tracks, self.rows, self.release.total_tracks)
            .mine(self.wanted, self.wanted_local, self.whole)
    }
}

/// What a row of an album's page is to the user ([`ReleaseListing::membership`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Membership {
    /// One of the user's songs of the album.
    Wanted,
    /// Another version of one of the album's songs («(Instrumental)»,
    /// «(Off Vocal)», «(Remix)»…) the user did not choose.
    Variant,
    /// Left out of the user's songs of the album by hand.
    Excluded,
    /// Only in the album's tracklist: nothing says the user wants it or not.
    Undecided,
}

/// One line of an album's page ([`crate::Library::release_listing`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ListingEntry {
    Track(TrackRow),
    /// A place the album counts that no row has: `no` 0 when unknown.
    Hole { disc: u32, no: u32 },
}

/// An album's page: its rows and holes, and the counts of both; which rows
/// are the user's songs of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseListing {
    pub entries: Vec<ListingEntry>,
    pub progress: ReleaseProgress,
    /// The rows that are the user's songs of the album.
    pub wanted: std::collections::HashSet<TrackId>,
    /// The rows left out of them by hand.
    pub excluded: std::collections::HashSet<TrackId>,
    /// What its page shows while only some of it is the user's (see
    /// [`ReleaseRow::view`]).
    pub view: Option<bool>,
}

impl ReleaseListing {
    /// `unwanted`: the places of songs the user deleted from the album
    /// (`crate::unwanted`); within its count and free, they are no holes.
    pub(crate) fn new(rows: Vec<TrackRow>, total_tracks: Option<u32>, unwanted: &[(u32, u32)]) -> Self {
        let local = rows.iter().filter(|r| r.has_local).count() as u32;
        let count = total_tracks.unwrap_or(0);
        let placed: std::collections::HashSet<(u32, u32)> = rows.iter().map(|r| (r.disc_no.max(1), r.track_no)).collect();
        let gone: std::collections::HashSet<(u32, u32)> =
            unwanted.iter().copied().filter(|(d, n)| (1..=count).contains(n) && !placed.contains(&(*d, *n))).collect();
        let total_tracks = total_tracks.map(|t| t.saturating_sub(gone.len() as u32));
        let progress = ReleaseProgress::new(local, local, rows.len() as u32, total_tracks);
        let holes = progress.total as usize - rows.len();
        let discs: std::collections::BTreeSet<u32> = rows.iter().map(|r| r.disc_no.max(1)).collect();
        let disc = discs.iter().next_back().copied().unwrap_or(1);
        // Numbered holes on one disc: the free numbers up to the count.
        let mut numbered: Vec<u32> = Vec::new();
        if discs.len() <= 1 {
            let taken: std::collections::HashSet<u32> = rows.iter().map(|r| r.track_no).collect();
            let skipped: std::collections::HashSet<u32> = gone.iter().map(|(_, n)| *n).collect();
            numbered = (1..=progress.total.max(count))
                .filter(|n| !taken.contains(n) && !skipped.contains(n))
                .take(holes)
                .collect();
        }
        let mut entries = Vec::with_capacity(progress.total as usize);
        let mut next = numbered.iter().copied().peekable();
        for r in rows {
            while let Some(&n) = next.peek() {
                if r.track_no == 0 || n > r.track_no {
                    break;
                }
                entries.push(ListingEntry::Hole { disc, no: n });
                next.next();
            }
            entries.push(ListingEntry::Track(r));
        }
        entries.extend(next.map(|no| ListingEntry::Hole { disc, no }));
        while entries.len() < progress.total as usize {
            entries.push(ListingEntry::Hole { disc, no: 0 });
        }
        Self {
            entries,
            progress,
            wanted: std::collections::HashSet::new(),
            excluded: std::collections::HashSet::new(),
            view: None,
        }
    }

    /// The user's songs of the album (`wanted`, a subset of its rows), the
    /// rows left out by hand, whether the whole album is theirs and what
    /// its page shows: counted into [`Self::progress`].
    pub(crate) fn with_choices(
        mut self,
        wanted: std::collections::HashSet<TrackId>,
        excluded: std::collections::HashSet<TrackId>,
        whole: bool,
        view: Option<bool>,
    ) -> Self {
        let local = self.tracks().filter(|t| t.has_local && wanted.contains(&t.id)).count() as u32;
        self.progress = self.progress.mine(wanted.len() as u32, local, whole);
        self.wanted = wanted;
        self.excluded = excluded;
        self.view = view;
        self
    }

    /// The user's songs of the album, in its order.
    pub fn wanted_ids(&self) -> Vec<TrackId> {
        self.tracks().filter(|t| self.wanted.contains(&t.id)).map(|t| t.id).collect()
    }

    /// Every row of the album, in its order.
    pub fn track_ids(&self) -> Vec<TrackId> {
        self.tracks().map(|t| t.id).collect()
    }

    /// What each row is to the user. With `versions` a row that is another
    /// version of a song of the same album (its title without the version
    /// mark is that song's title, the artists agree: «SugaRiddle
    /// (Instrumental)» beside «SugaRiddle») and is not one of the user's
    /// songs is [`Membership::Variant`]; a version the user chose stays
    /// theirs, and a mark without its song beside it proves nothing
    /// (undecided). Without `versions` no row is a variant.
    pub fn membership(&self, versions: bool) -> std::collections::HashMap<TrackId, Membership> {
        use nikgob_core::clip_title::{fold, version_split};
        // The album's songs that are no version: title and artist.
        let plain: Vec<(String, String)> = if versions {
            self.tracks()
                .filter(|t| version_split(&t.title).is_none())
                .map(|t| (fold(&t.title), fold(&t.artist)))
                .collect()
        } else {
            Vec::new()
        };
        let agree = |a: &str, b: &str| a.is_empty() || b.is_empty() || a.contains(b) || b.contains(a);
        self.tracks()
            .map(|t| {
                let variant = versions
                    && version_split(&t.title).is_some_and(|(base, _)| {
                        let (base, artist) = (fold(&base), fold(&t.artist));
                        plain.iter().any(|(title, a)| *title == base && agree(a, &artist))
                    });
                let m = if self.wanted.contains(&t.id) {
                    Membership::Wanted
                } else if self.excluded.contains(&t.id) {
                    Membership::Excluded
                } else if variant {
                    Membership::Variant
                } else {
                    Membership::Undecided
                };
                (t.id, m)
            })
            .collect()
    }

    pub fn holes(&self) -> usize {
        self.entries.iter().filter(|e| matches!(e, ListingEntry::Hole { .. })).count()
    }

    pub fn tracks(&self) -> impl Iterator<Item = &TrackRow> {
        self.entries.iter().filter_map(|e| match e {
            ListingEntry::Track(t) => Some(t),
            ListingEntry::Hole { .. } => None,
        })
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
    /// `total_tracks` of the release, or the rows of its page if more or
    /// unknown: the lines of its page ([`ReleaseListing`]).
    pub total: u32,
    /// The user's songs of it ([`ReleaseRow::wanted`]).
    #[serde(default)]
    pub wanted: u32,
    /// Of them, the ones with a file.
    #[serde(default)]
    pub wanted_local: u32,
    /// The whole album is the user's ([`ReleaseRow::whole`]).
    #[serde(default)]
    pub whole: bool,
}

impl ReleaseProgress {
    /// The counts of the album; its tracks in the library stand for the
    /// user's songs until [`Self::mine`] says.
    pub(crate) fn new(local: u32, known: u32, rows: u32, total_tracks: Option<u32>) -> Self {
        Self {
            local,
            in_library: known,
            total: total_tracks.unwrap_or(0).max(rows).max(known),
            wanted: known,
            wanted_local: local.min(known),
            whole: false,
        }
    }

    /// With the user's songs of the album counted.
    pub(crate) fn mine(self, wanted: u32, wanted_local: u32, whole: bool) -> Self {
        Self { wanted, wanted_local, whole, ..self }
    }

    /// Lines of the page without a file: «1 нет».
    pub fn missing(&self) -> u32 {
        self.total.saturating_sub(self.local)
    }

    /// Only some of the album is the user's: some of its songs, not all
    /// of its places (a place without a row counts), not added whole.
    /// Its page shows and plays the user's songs, nothing of the rest is
    /// owed; an album with none of them is a plain catalog page.
    pub fn partial(&self) -> bool {
        !self.whole && self.wanted > 0 && self.wanted < self.total
    }

    /// The user's songs of the album without a file.
    pub fn wanted_missing(&self) -> u32 {
        self.wanted.saturating_sub(self.wanted_local)
    }

    /// What «Скачать недостающие» asks for: of a partial album the user's
    /// songs without a file, else every place without one.
    pub fn lacking(&self) -> u32 {
        if self.partial() { self.wanted_missing() } else { self.missing() }
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
    /// The artist itself is liked.
    #[serde(default)]
    pub liked: bool,
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

/// A file parked in «Подтверди» (see `Library::put_confirm_item`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmRow {
    pub id: i64,
    pub path: String,
    /// The «Хочу скачать» item the file was downloaded for.
    pub wishlist: Option<nikgob_core::model::WishlistId>,
    /// The importer's own json.
    pub payload: String,
    pub created_at: UnixMs,
}

/// One finished job of "Загрузки → История".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// Row id, increasing.
    pub seq: i64,
    /// `import`, `torrent`, or `wish:<item>` (the one line of a wished
    /// download).
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
    /// «Не рекомендовать» marks written to `rec_feedback.json`.
    pub rec_feedback: usize,
    /// Marks of songs saved in a service's library written to `saved.json`.
    pub saved: usize,
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
    /// Marks of songs saved in a service's library taken from `saved.json`.
    pub saved: usize,
    /// Unparsable lines in `events.jsonl`.
    pub invalid_lines: usize,
}

/// A track whose local file is due for sound analysis
/// ([`crate::Library::tracks_needing_analysis`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisTodo {
    pub track_id: TrackId,
    /// The active local source.
    pub source_id: SourceId,
    /// Its path (relative to the library root, or absolute).
    pub locator: String,
    /// Which file this is (source id, hash, size): a failure is remembered
    /// for this key ([`crate::Library::mark_analysis_failed`]).
    pub file_key: String,
}

/// A stream-only track due for sound analysis
/// ([`crate::Library::streams_needing_analysis`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamAnalysisTodo {
    pub track_id: TrackId,
    /// Its YouTube video, if it has one that plays (else its song has to
    /// be found on YouTube Music first).
    pub youtube: Option<String>,
}

/// How far the sound analysis is ([`crate::Library::analysis_counts`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnalysisCounts {
    /// Tracks with a file analysed from it with the current analysis.
    pub files_done: u32,
    /// Tracks with a file, without those that failed to decode.
    pub files_total: u32,
    /// Stream-only tracks that are liked or in a playlist and have a
    /// current analysis.
    pub streams_done: u32,
    pub streams_total: u32,
}

/// One «Не рекомендовать» mark ([`crate::Library::set_rec_feedback`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecFeedback {
    pub target_kind: String,
    pub target_id: String,
    pub dislike: bool,
    pub updated_at: UnixMs,
}
