//! Import pipeline.

pub mod analysis;
pub mod config;
pub mod cover;
pub mod decide;
pub mod decode;
pub mod encode;
pub mod error;
pub mod events;
mod album;
mod confirm;
mod folder;
pub mod fsutil;
pub mod identity;
pub mod importer;
pub mod layout;
pub mod quality;
pub mod resample;
pub mod repair;
pub mod retag;
pub mod rebuild;
pub mod sanitize;
pub mod spectrum;
mod store;
pub mod tags;
pub mod trash;
pub mod watch;

pub use album::{AlbumTrack, folder_has_album};
pub use config::{ImportConfig, normalize_masters_dir};
pub use decide::ImportTargets;
pub use encode::{EncodeOptions, EncodedInfo, Encoder, FfmpegEncoder, RopusEncoder};
pub use error::{ImportError, Result};
pub use events::{ConfirmItem, ConfirmOption, ImportEvent, ItemId, JobId, Stage};
pub use importer::{FileOutcome, ImportJob, Importer, RelocateReport, relocate_masters};
pub use rebuild::{EditionGroup, RebuildReport, find_release_editions, fold_editions, merge_split_releases, rebuild_from_files};
pub use repair::{
    ArtistBack, IdentityRepair, Moved, TORRENT_ARTIST_REPAIR, TORRENT_IDENTITY_REPAIR, release_files, wished_release,
};
pub use retag::TagChanges;
pub use spectrum::{SpectrumReport, Verdict};
pub use trash::{
    PurgeReport, RestoreReport, TRASH_DAYS, delete_track_to_trash, purge_deleted_tracks, purge_track,
    restore_track, trash_file, trash_purge,
};
pub use watch::InboxWatcher;
