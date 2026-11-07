//! Last.fm: similar tracks and artists from its scrobbles
//! (`track.getSimilar`, `artist.getSimilar`), only with the user's own free
//! API key (Settings, settings.toml; empty by default). The key is a query
//! parameter: it never reaches a log (errors carry no URL).

use serde::Deserialize;
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url, parse_json};

const DEFAULT_BASE: &str = "https://ws.audioscrobbler.com/2.0/";
/// Entries asked of a list.
const LIMIT: &str = "50";
/// Last.fm's error for what it does not know.
const NOT_FOUND: i64 = 6;

/// One similar track or artist.
#[derive(Clone, Debug, PartialEq)]
pub struct LastFmSimilar {
    pub name: String,
    /// A track's artist; empty for an artist.
    pub artist: String,
    pub mbid: Option<String>,
    /// Last.fm's `match`, 0…1.
    pub score: f32,
}

#[derive(Clone)]
pub struct LastFm {
    http: Http,
    base: Url,
    key: String,
}

impl std::fmt::Debug for LastFm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LastFm").field("base", &self.base.as_str()).field("key", &"<redacted>").finish()
    }
}

#[derive(Deserialize)]
struct RawArtistRef {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct RawEntry {
    name: String,
    #[serde(default)]
    mbid: Option<String>,
    #[serde(default, rename = "match", deserialize_with = "number")]
    score: f32,
    #[serde(default)]
    artist: Option<RawArtistRef>,
}

#[derive(Deserialize)]
struct RawTracks {
    #[serde(default)]
    track: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawArtists {
    #[serde(default)]
    artist: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawAnswer {
    #[serde(default)]
    error: Option<i64>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    similartracks: Option<RawTracks>,
    #[serde(default)]
    similarartists: Option<RawArtists>,
}

/// Last.fm writes numbers as numbers or as strings.
fn number<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<f32, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        F(f32),
        S(String),
    }
    Ok(match N::deserialize(d)? {
        N::F(x) => x,
        N::S(s) => s.trim().parse().unwrap_or(0.0),
    })
}

impl LastFm {
    /// A client with the user's key; none for an empty key.
    pub fn new(http: Http, key: &str) -> Option<Self> {
        Self::with_base_url(http, DEFAULT_BASE, key).ok().flatten()
    }

    /// Points the client at another server (tests).
    pub fn with_base_url(http: Http, base: &str, key: &str) -> Result<Option<Self>> {
        let key = key.trim();
        if key.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self { http, base: base_url(base)?, key: key.to_owned() }))
    }

    async fn ask(&self, params: &[(&str, &str)]) -> Result<RawAnswer> {
        let mut all: Vec<(&str, &str)> = vec![("api_key", self.key.as_str()), ("format", "json"), ("autocorrect", "1"), ("limit", LIMIT)];
        all.extend_from_slice(params);
        let url = build_url(&self.base, "", &all)?;
        let body = match self.http.get_body(Service::LastFm, url).await {
            Ok(b) => b,
            // Last.fm answers its errors with a status and a body too.
            Err(MetadataError::Status { body, .. }) => body.into_bytes(),
            Err(e) => return Err(e),
        };
        let answer: RawAnswer = parse_json(&body)?;
        match answer.error {
            None | Some(NOT_FOUND) => Ok(answer),
            Some(code) => Err(MetadataError::Service(format!("Last.fm error {code}: {}", answer.message.unwrap_or_default()))),
        }
    }

    /// The tracks like `title` by `artist`, the most first.
    pub async fn similar_tracks(&self, artist: &str, title: &str) -> Result<Vec<LastFmSimilar>> {
        let a = self.ask(&[("method", "track.getsimilar"), ("artist", artist), ("track", title)]).await?;
        Ok(a.similartracks
            .map(|l| l.track)
            .unwrap_or_default()
            .into_iter()
            .map(|e| LastFmSimilar {
                artist: e.artist.map(|a| a.name).unwrap_or_default(),
                name: e.name,
                mbid: e.mbid.filter(|m| !m.is_empty()),
                score: e.score,
            })
            .collect())
    }

    /// The artists like `artist`, the most first.
    pub async fn similar_artists(&self, artist: &str) -> Result<Vec<LastFmSimilar>> {
        let a = self.ask(&[("method", "artist.getsimilar"), ("artist", artist)]).await?;
        Ok(a.similarartists
            .map(|l| l.artist)
            .unwrap_or_default()
            .into_iter()
            .map(|e| LastFmSimilar { name: e.name, artist: String::new(), mbid: e.mbid.filter(|m| !m.is_empty()), score: e.score })
            .collect())
    }
}
