//! Online metadata for the player: MusicBrainz, AcoustID, Cover Art Archive,
//! Deezer and LRCLIB clients, an LRC parser, and the pure [`matcher`] that decides
//! whether a local file has been identified for sure.
//!
//! All clients share one [`Http`] (User-Agent `music.nikgob/<version> ( <contact> )`,
//! per-service rate limits, retries with backoff on 429/503). [`Metadata`] wires
//! everything up from a [`MetadataConfig`].

pub mod acoustid;
mod config;
pub mod coverart;
pub mod deezer;
mod error;
mod http;
pub mod lrc;
pub mod lrclib;
pub mod matcher;
pub mod musicbrainz;
pub mod text;

pub use acoustid::{AcoustId, AcoustIdRecording, AcoustIdResult};
pub use config::MetadataConfig;
pub use coverart::{CoverArtArchive, CoverImage, CoverTarget, FetchedCover};
pub use deezer::{Deezer, DeezerAlbum, DeezerArtistHit, DeezerTrack};
pub use error::{MetadataError, Result};
pub use http::{Http, RateLimiter, RateLimits, RetryPolicy};
pub use lrc::parse_lrc;
pub use lrclib::{Lrclib, Lyrics};
pub use matcher::{
    Candidate, CandidateIds, Confidence, LocalEvidence, MatchCandidate, MatchResult, Source,
    TrackMetadata, candidate_from_deezer, candidates_from_recording, candidates_from_release,
    match_candidates,
};
pub use musicbrainz::{MusicBrainz, RecordingQuery};

/// Every client, built from one config and sharing one HTTP stack.
#[derive(Clone, Debug)]
pub struct Metadata {
    pub musicbrainz: MusicBrainz,
    /// `None` when no AcoustID client key is configured.
    pub acoustid: Option<AcoustId>,
    pub cover_art: CoverArtArchive,
    pub deezer: Deezer,
    pub lrclib: Lrclib,
}

impl Metadata {
    /// Fails with [`MetadataError::NotConfigured`] when no contact is set (MusicBrainz
    /// rejects anonymous clients).
    pub fn new(config: &MetadataConfig) -> Result<Self> {
        let http = Http::new(config)?;
        Ok(Self {
            musicbrainz: MusicBrainz::new(http.clone())?,
            acoustid: AcoustId::new(http.clone(), config).ok(),
            cover_art: CoverArtArchive::new(http.clone()),
            deezer: Deezer::new(http.clone()),
            lrclib: Lrclib::new(http),
        })
    }
}
