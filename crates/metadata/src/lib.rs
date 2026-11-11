//! Online metadata for the player: MusicBrainz, AcoustID, Cover Art Archive,
//! Deezer, the iTunes Search API, ListenBrainz and Last.fm (similar music), LyricsPlus, lrc.red, Better Lyrics and LRCLIB clients, LRC and TTML lyrics parsers, and the pure [`matcher`] that decides
//! whether a local file has been identified for sure.
//!
//! All clients share one [`Http`] (User-Agent `music.nikgob/<version> ( <contact> )`,
//! per-service rate limits, retries with backoff on 429/503). [`Metadata`] wires
//! everything up from a [`MetadataConfig`].

pub mod acoustid;
mod config;
pub mod coverart;
pub mod deezer;
pub mod itunes;
mod error;
mod http;
pub mod lastfm;
pub mod listenbrainz;
pub mod lrc;
pub mod lrclib;
pub mod matcher;
pub mod musicbrainz;
pub mod odesli;
pub mod pause;
pub mod lyricsplus;
pub mod lrcred;
pub mod betterlyrics;
pub mod genius;
pub mod text;
pub mod ttml;
pub mod wikidata;

pub use acoustid::{AcoustId, AcoustIdRecording, AcoustIdResult};
pub use config::MetadataConfig;
pub use coverart::{CoverArtArchive, CoverImage, CoverTarget, FetchedCover};
pub use deezer::{ArtistPicture, Deezer, DeezerAlbum, DeezerArtistHit, DeezerTrack};
pub use itunes::{Itunes, ItunesSong};
pub use error::{MetadataError, Result};
pub use http::{Http, RateLimiter, RateLimits, RetryPolicy, in_background};
pub use lastfm::{LastFm, LastFmSimilar};
pub use listenbrainz::{ListenBrainz, Similar};
pub use lrc::{parse_lrc, parse_lrc_lines};
pub use lrclib::{Lrclib, Lyrics};
pub use matcher::{
    Candidate, CandidateIds, Confidence, LocalEvidence, MatchCandidate, MatchResult, Source,
    TrackMetadata, candidate_from_deezer, candidates_from_recording, candidates_from_release,
    match_candidates,
};
pub use musicbrainz::{ArtistInfo, ArtistTags, MusicBrainz, RecordingQuery};
pub use odesli::{Odesli, OdesliPage};
pub use pause::{Pauses, Verdict};
pub use lyricsplus::{LyricsPlus, LyricsPlusText};
pub use lrcred::LrcRed;
pub use betterlyrics::BetterLyrics;
pub use genius::Genius;
pub use wikidata::{WebImages, Wikidata, WikidataArtist};
pub use ttml::{TtmlLyrics, mark_ttml_own, parse_ttml, parse_ttml_plain, serialize_ttml, serialize_ttml_plain, ttml_is_own};

/// Every client, built from one config and sharing one HTTP stack.
#[derive(Clone, Debug)]
pub struct Metadata {
    pub musicbrainz: MusicBrainz,
    /// `None` when no AcoustID client key is configured.
    pub acoustid: Option<AcoustId>,
    pub cover_art: CoverArtArchive,
    pub deezer: Deezer,
    pub lrclib: Lrclib,
    pub lyricsplus: LyricsPlus,
    pub lrcred: LrcRed,
    pub betterlyrics: BetterLyrics,
    /// Plain lyrics from Genius's public pages (no account).
    pub genius: Genius,
    pub itunes: Itunes,
    /// Similar artists and recordings (no key); the popularity with the
    /// user's token.
    pub listenbrainz: ListenBrainz,
    /// `None` without the user's Last.fm key.
    pub lastfm: Option<LastFm>,
    /// Wikidata and Wikimedia Commons (an artist's Latin label and picture).
    pub wikidata: Wikidata,
    /// Pictures on the services' CDNs.
    pub web_images: WebImages,
    /// Lyrics sources sitting out after refusals in a row (shared by the
    /// clones).
    pub lyrics_pauses: Pauses,
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
            lrclib: Lrclib::new(http.clone()),
            lyricsplus: LyricsPlus::new(http.clone()),
            lrcred: LrcRed::new(http.clone()),
            betterlyrics: BetterLyrics::new(http.clone()),
            genius: Genius::new(http.clone()),
            itunes: Itunes::new(http.clone()),
            listenbrainz: ListenBrainz::new(http.clone()).with_token(config.listenbrainz_token.as_deref()),
            wikidata: Wikidata::new(http.clone()),
            web_images: WebImages::new(http.clone()),
            lastfm: config.lastfm_key.as_deref().and_then(|k| LastFm::new(http, k)),
            lyrics_pauses: Pauses::new(),
        })
    }
}
