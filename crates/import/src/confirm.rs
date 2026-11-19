//! «Подтверди» in the database: what a parked file
//! is offered, kept as json in `confirm_item` so the queue survives a
//! restart. The file itself is read again when the user answers.

use std::path::Path;
use std::time::Duration;

use nikgob_core::model::{SourceOrigin, TrackId};
use nikgob_metadata::matcher::{CandidateIds, MatchCandidate, Source as MatchSource, TrackMetadata};
use serde::{Deserialize, Serialize};

use crate::decide::ImportTargets;
use crate::events::{ConfirmOption, STREAM_OPTION, TAGS_OPTION};
use crate::identity::confirm_option;

/// A candidate as stored: what [`crate::identity::apply_candidate`] needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct StoredCandidate {
    pub score: f64,
    pub deezer: bool,
    pub title: String,
    pub artist: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration_ms: Option<u64>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub date: Option<String>,
    pub isrcs: Vec<String>,
    pub release_type: Option<String>,
    pub release_secondary_types: Vec<String>,
    pub cover_url: Option<String>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub release_group_mbid: Option<String>,
    pub artist_mbid: Option<String>,
}

impl From<&MatchCandidate> for StoredCandidate {
    fn from(c: &MatchCandidate) -> Self {
        let m = &c.metadata;
        Self {
            score: c.score,
            deezer: c.source == MatchSource::Deezer,
            title: m.title.clone(),
            artist: m.artist.clone(),
            artists: m.artists.clone(),
            album: m.album.clone(),
            album_artist: m.album_artist.clone(),
            duration_ms: m.duration.map(|d| d.as_millis() as u64),
            track_no: m.track_no,
            disc_no: m.disc_no,
            date: m.date.clone(),
            isrcs: m.isrcs.clone(),
            release_type: m.release_type.clone(),
            release_secondary_types: m.release_secondary_types.clone(),
            cover_url: m.cover_url.clone(),
            recording_mbid: c.ids.recording_mbid.clone(),
            release_mbid: c.ids.release_mbid.clone(),
            release_group_mbid: c.ids.release_group_mbid.clone(),
            artist_mbid: c.ids.artist_mbid.clone(),
        }
    }
}

impl StoredCandidate {
    pub fn candidate(&self) -> MatchCandidate {
        MatchCandidate {
            score: self.score,
            source: if self.deezer { MatchSource::Deezer } else { MatchSource::MusicBrainz },
            ids: CandidateIds {
                recording_mbid: self.recording_mbid.clone(),
                release_mbid: self.release_mbid.clone(),
                release_group_mbid: self.release_group_mbid.clone(),
                artist_mbid: self.artist_mbid.clone(),
                ..CandidateIds::default()
            },
            metadata: TrackMetadata {
                title: self.title.clone(),
                artist: self.artist.clone(),
                artists: self.artists.clone(),
                album: self.album.clone(),
                album_artist: self.album_artist.clone(),
                duration: self.duration_ms.map(Duration::from_millis),
                track_no: self.track_no,
                disc_no: self.disc_no,
                date: self.date.clone(),
                isrcs: self.isrcs.clone(),
                release_type: self.release_type.clone(),
                release_secondary_types: self.release_secondary_types.clone(),
                cover_url: self.cover_url.clone(),
                ..TrackMetadata::default()
            },
            breakdown: Default::default(),
        }
    }
}

/// One answer to a parked file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Choice {
    /// Import with the file's own tags.
    Tags,
    Candidate(Box<StoredCandidate>),
    /// Attach the file to this liked stream track (its length is off).
    Stream {
        track: TrackId,
        title: String,
        artist: String,
        album: Option<String>,
        track_no: Option<u32>,
        duration_ms: Option<u32>,
        recording_mbid: Option<String>,
    },
}

/// What the file's tags say, for the first option.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TagView {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<i32>,
    pub track_no: Option<u32>,
    pub duration_ms: Option<u32>,
    /// What the file really is («FLAC из MP3 ~320»), as data; a row kept
    /// before it was holds the words themselves.
    #[serde(default)]
    pub quality: Option<nikgob_core::i18n::status::SavedQuality>,
}

/// The json of a `confirm_item` row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Payload {
    pub origin: SourceOrigin,
    pub targets: ImportTargets,
    pub tags: TagView,
    pub choices: Vec<Choice>,
    /// When the file's job started (what the library had before it is
    /// settled), for an answer after a restart; `None` in rows stored
    /// before it was kept.
    #[serde(default)]
    pub job_started: Option<i64>,
}

impl Choice {
    pub fn option(&self, tags: &TagView, path: &Path) -> ConfirmOption {
        match self {
            Self::Tags => ConfirmOption {
                title: tags.title.clone().unwrap_or_else(|| {
                    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                }),
                artist: tags.artist.clone().unwrap_or_default(),
                album: tags.album.clone(),
                year: tags.year,
                track_no: tags.track_no,
                duration_ms: tags.duration_ms,
                score: 0.0,
                source: TAGS_OPTION,
                recording_mbid: None,
                release_mbid: None,
            },
            Self::Candidate(c) => confirm_option(&c.candidate()),
            Self::Stream { title, artist, album, track_no, duration_ms, recording_mbid, .. } => ConfirmOption {
                title: title.clone(),
                artist: artist.clone(),
                album: album.clone(),
                year: None,
                track_no: *track_no,
                duration_ms: *duration_ms,
                score: 0.5,
                source: STREAM_OPTION,
                recording_mbid: recording_mbid.clone(),
                release_mbid: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_reads_back_as_written() {
        let cand = MatchCandidate {
            score: 0.7,
            source: MatchSource::MusicBrainz,
            ids: CandidateIds { release_mbid: Some("rel".into()), ..Default::default() },
            metadata: TrackMetadata {
                title: "Cash Machine".into(),
                artist: "Oliver Tree".into(),
                artists: vec!["Oliver Tree".into()],
                album: Some("Ugly Is Beautiful".into()),
                duration: Some(Duration::from_millis(176_950)),
                date: Some("2020-07-17".into()),
                ..Default::default()
            },
            breakdown: Default::default(),
        };
        let p = Payload {
            origin: SourceOrigin::Torrent { info_hash: "bdc5".into(), path_in_torrent: "A/3.flac".into() },
            targets: ImportTargets::new(vec![TrackId::new()]),
            tags: TagView { title: Some("Cash Machine".into()), ..TagView::default() },
            choices: vec![Choice::Tags, Choice::Candidate(Box::new((&cand).into()))],
            job_started: Some(1_700_000_000_000),
        };
        let back: Payload = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
        // A row stored before the job's start was kept reads without it.
        let mut old: serde_json::Value = serde_json::to_value(&p).unwrap();
        old.as_object_mut().unwrap().remove("job_started");
        let old: Payload = serde_json::from_value(old).unwrap();
        assert_eq!(old.job_started, None);
        let Choice::Candidate(c) = &back.choices[1] else { panic!() };
        assert_eq!(c.candidate().metadata, cand.metadata);
        let o = back.choices[1].option(&back.tags, Path::new("3.flac"));
        assert_eq!((o.year, o.duration_ms, o.album.as_deref()), (Some(2020), Some(176_950), Some("Ugly Is Beautiful")));
        assert_eq!(back.choices[0].option(&back.tags, Path::new("3.flac")).source, TAGS_OPTION);
    }
}
