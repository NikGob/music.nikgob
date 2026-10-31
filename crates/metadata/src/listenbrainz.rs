//! ListenBrainz: open data of its listeners' sessions, no key (the graph of
//! similar music, docs/recs-graph.md).
//!
//! * `labs.api.listenbrainz.org/similar-artists/json` and
//!   `similar-recordings/json`: the artists and recordings heard in the
//!   same sessions as an artist or recording (by MusicBrainz id), the most
//!   first;
//! * `api.listenbrainz.org/1/popularity/top-recordings-for-artist/<mbid>`:
//!   an artist's recordings by how many listen to them.
//!
//! An id ListenBrainz does not know answers an empty list (some of its
//! labs answer 400 for it).

use serde::Deserialize;
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url};

const LABS_BASE: &str = "https://labs.api.listenbrainz.org/";
const API_BASE: &str = "https://api.listenbrainz.org/1/";
/// The labs' datasets the lists come from.
const ARTISTS_ALGORITHM: &str = "session_based_days_7500_session_300_contribution_5_threshold_10_limit_100_filter_True_skip_30";
const RECORDINGS_ALGORITHM: &str = "session_based_days_9000_session_300_contribution_5_threshold_15_limit_50_skip_30";

/// One entry of a list: its MusicBrainz id, name and artist (a
/// recording's), as ListenBrainz gives them.
#[derive(Clone, Debug, PartialEq)]
pub struct Similar {
    pub mbid: String,
    pub name: String,
    pub artist: String,
    pub score: f32,
}

#[derive(Clone, Debug)]
pub struct ListenBrainz {
    http: Http,
    labs: Url,
    api: Url,
}

#[derive(Deserialize)]
struct RawArtist {
    artist_mbid: String,
    name: String,
    #[serde(default)]
    score: f32,
}

#[derive(Deserialize)]
struct RawRecording {
    recording_mbid: String,
    #[serde(default)]
    recording_name: String,
    #[serde(default, alias = "artist_name")]
    artist_credit_name: String,
    #[serde(default, alias = "total_listen_count")]
    score: f32,
}

fn mbid_ok(mbid: &str) -> Result<()> {
    if mbid.len() == 36 && mbid.chars().all(|c| c == '-' || c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(MetadataError::InvalidArgument(format!("not a MusicBrainz id: {mbid:?}")))
    }
}

/// An id it does not know is an empty list.
fn unknown_is_empty<T>(r: Result<Vec<T>>) -> Result<Vec<T>> {
    match r {
        Err(MetadataError::NotFound | MetadataError::Status { status: 400, .. }) => Ok(Vec::new()),
        other => other,
    }
}

impl ListenBrainz {
    pub fn new(http: Http) -> Self {
        Self { http, labs: base_url(LABS_BASE).expect("valid"), api: base_url(API_BASE).expect("valid") }
    }

    /// Points the client at other servers (tests).
    pub fn with_base_urls(http: Http, labs: &str, api: &str) -> Result<Self> {
        Ok(Self { http, labs: base_url(labs)?, api: base_url(api)? })
    }

    /// The artists heard with the artist `mbid`, the most first.
    pub async fn similar_artists(&self, mbid: &str) -> Result<Vec<Similar>> {
        mbid_ok(mbid)?;
        let url = build_url(&self.labs, "similar-artists/json", &[("artist_mbids", mbid), ("algorithm", ARTISTS_ALGORITHM)])?;
        let raw: Result<Vec<RawArtist>> = self.http.get_json(Service::ListenBrainz, url).await;
        Ok(unknown_is_empty(raw)?
            .into_iter()
            .filter(|a| a.artist_mbid != mbid)
            .map(|a| Similar { mbid: a.artist_mbid, name: a.name, artist: String::new(), score: a.score })
            .collect())
    }

    /// The recordings heard with the recording `mbid`, the most first.
    pub async fn similar_recordings(&self, mbid: &str) -> Result<Vec<Similar>> {
        mbid_ok(mbid)?;
        let url = build_url(&self.labs, "similar-recordings/json", &[("recording_mbids", mbid), ("algorithm", RECORDINGS_ALGORITHM)])?;
        let raw: Result<Vec<RawRecording>> = self.http.get_json(Service::ListenBrainz, url).await;
        Ok(unknown_is_empty(raw)?.into_iter().filter(|r| r.recording_mbid != mbid).map(recording).collect())
    }

    /// The recordings of the artist `mbid` by how many listen to them.
    pub async fn top_recordings(&self, mbid: &str) -> Result<Vec<Similar>> {
        mbid_ok(mbid)?;
        let url = build_url(&self.api, &format!("popularity/top-recordings-for-artist/{mbid}"), &[])?;
        let raw: Result<Vec<RawRecording>> = self.http.get_json(Service::ListenBrainz, url).await;
        Ok(unknown_is_empty(raw)?.into_iter().map(recording).collect())
    }
}

fn recording(r: RawRecording) -> Similar {
    Similar { mbid: r.recording_mbid, name: r.recording_name, artist: r.artist_credit_name, score: r.score }
}
