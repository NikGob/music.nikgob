//! Progress events of the import pipeline.

use std::path::PathBuf;

use nikgob_core::model::{Master, TrackId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JobId(pub u64);

impl std::fmt::Display for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "job-{}", self.0)
    }
}

/// A file waiting for the user in the confirmation queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

/// Steps of PLAN section 6, as reported in [`ImportEvent::FileProgress`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    Probing,
    Fingerprinting,
    Identifying,
    QualityCheck,
    Deciding,
    Converting,
    Tagging,
    Placing,
    Deriving,
}

/// [`ConfirmOption::source`] of "attach to this stream track".
pub const STREAM_OPTION: &str = "stream";
/// [`ConfirmOption::source`] of "what the file's own tags say" (always the
/// first option).
pub const TAGS_OPTION: &str = "tags";

/// One possible identity offered to the user.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmOption {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    /// Length of the recording (of the file, for [`TAGS_OPTION`]).
    pub duration_ms: Option<u32>,
    /// 0..=1, from the matcher.
    pub score: f64,
    /// `"musicbrainz"`, `"deezer"`, [`TAGS_OPTION`], or [`STREAM_OPTION`]
    /// for a liked stream track the file may be (its length is off).
    pub source: &'static str,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
}

/// A parked file: the user picks one of `options` (or none) via
/// [`crate::Importer::resolve`].
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmItem {
    pub id: ItemId,
    pub job: JobId,
    /// The file stays here until resolved.
    pub path: PathBuf,
    pub tag_title: Option<String>,
    pub tag_artist: Option<String>,
    pub tag_album: Option<String>,
    pub duration_ms: Option<u32>,
    /// The file's own tags first, then up to three candidates, best first.
    pub options: Vec<ConfirmOption>,
    /// The «Хочу скачать» item the file was downloaded for.
    pub wishlist: Option<nikgob_core::model::WishlistId>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ImportEvent {
    Started {
        job: JobId,
        files: usize,
    },
    FileProgress {
        job: JobId,
        path: PathBuf,
        stage: Stage,
    },
    /// The file became (or replaced the file of) a track.
    Imported {
        job: JobId,
        path: PathBuf,
        track_id: TrackId,
        upgraded: bool,
    },
    NeedsConfirmation {
        item: ConfirmItem,
    },
    /// Not better than what the library has; `kept` is the existing track.
    Duplicate {
        job: JobId,
        path: PathBuf,
        kept: TrackId,
    },
    Failed {
        job: JobId,
        path: PathBuf,
        error: String,
    },
    /// All files of the job are done or parked.
    Finished {
        job: JobId,
        imported: usize,
        duplicates: usize,
        failed: usize,
        pending: usize,
    },
    /// A `.torrent` file showed up in the Inbox (or in the job's paths); the
    /// runtime hands it to the torrent client.
    TorrentFound(PathBuf),
    /// A lossless master was registered; stage 6 (backup) moves it later.
    MasterRegistered(Master),
}
