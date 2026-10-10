//! AcoustID `/v2/lookup`: fingerprint -> scored MusicBrainz recordings.

use std::time::Duration;

use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Deserializer};
use url::Url;

use crate::config::MetadataConfig;
use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, ok_body, parse_json};

const DEFAULT_BASE: &str = "https://api.acoustid.org/";

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AcoustIdArtist {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AcoustIdReleaseGroup {
    pub id: String,
    pub title: String,
    /// `Album`, `Single`, ...
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AcoustIdRecording {
    /// MusicBrainz recording id.
    pub id: String,
    pub title: Option<String>,
    /// Seconds.
    pub duration: Option<f64>,
    #[serde(deserialize_with = "null_default")]
    pub artists: Vec<AcoustIdArtist>,
    #[serde(rename = "releasegroups", deserialize_with = "null_default")]
    pub release_groups: Vec<AcoustIdReleaseGroup>,
}

impl AcoustIdRecording {
    pub fn duration(&self) -> Option<Duration> {
        self.duration
            .filter(|d| d.is_finite() && *d >= 0.0)
            .map(Duration::from_secs_f64)
    }

    /// Artists joined with `", "` (AcoustID gives no join phrases).
    pub fn artist_names(&self) -> String {
        self.artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One fingerprint match: an AcoustID track with the recordings linked to it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AcoustIdResult {
    /// AcoustID track id.
    pub id: String,
    /// Match confidence 0..=1.
    pub score: f64,
    #[serde(deserialize_with = "null_default")]
    pub recordings: Vec<AcoustIdRecording>,
}

#[derive(Clone)]
pub struct AcoustId {
    http: Http,
    base: Url,
    client_key: String,
}

// The client key is a secret: never print it.
impl std::fmt::Debug for AcoustId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcoustId")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl AcoustId {
    /// Fails with [`MetadataError::NotConfigured`] when the config has no key.
    pub fn new(http: Http, config: &MetadataConfig) -> Result<Self> {
        let key = config
            .acoustid_key()
            .ok_or(MetadataError::NotConfigured("AcoustID client key"))?;
        Self::with_base_url(http, key, DEFAULT_BASE)
    }

    pub fn with_base_url(http: Http, client_key: &str, base: &str) -> Result<Self> {
        if client_key.trim().is_empty() {
            return Err(MetadataError::NotConfigured("AcoustID client key"));
        }
        Ok(Self {
            http,
            base: base_url(base)?,
            client_key: client_key.trim().to_owned(),
        })
    }

    /// Looks the fingerprint up (`meta=recordings+releasegroups+compress`).
    /// Results are sorted by score, best first. The key and fingerprint travel in
    /// the POST body, never in the URL.
    pub async fn lookup(
        &self,
        fingerprint: &str,
        duration: Duration,
    ) -> Result<Vec<AcoustIdResult>> {
        #[derive(Deserialize)]
        struct Raw {
            status: String,
            #[serde(default, deserialize_with = "null_default")]
            results: Vec<AcoustIdResult>,
            error: Option<RawError>,
        }
        #[derive(Deserialize)]
        struct RawError {
            #[serde(default)]
            message: String,
        }

        let fingerprint = fingerprint.trim();
        if fingerprint.is_empty() {
            return Err(MetadataError::InvalidArgument("empty fingerprint".into()));
        }
        let url = self
            .base
            .join("v2/lookup")
            .map_err(|e| MetadataError::InvalidArgument(e.to_string()))?;
        let secs = duration.as_secs().max(1).to_string();
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("client", &self.client_key)
            .append_pair("format", "json")
            .append_pair("duration", &secs)
            .append_pair("fingerprint", fingerprint)
            .append_pair("meta", "recordings+releasegroups+compress")
            .finish();
        let req = self
            .http
            .client()
            .post(url)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(body);
        let resp = self.http.send(Service::AcoustId, req).await?;
        let status = resp.status();
        // AcoustID reports API errors as a 4xx/5xx with a JSON error object.
        let bytes = match ok_body(resp).await {
            Err(MetadataError::Status { body, .. }) => {
                return Err(match parse_json::<Raw>(body.as_bytes()) {
                    Ok(Raw { error: Some(e), .. }) => MetadataError::Service(e.message),
                    _ => MetadataError::Status {
                        status: status.as_u16(),
                        body,
                    },
                });
            }
            other => other?,
        };
        let raw: Raw = parse_json(&bytes)?;
        if raw.status != "ok" {
            return Err(MetadataError::Service(
                raw.error.map_or_else(|| raw.status.clone(), |e| e.message),
            ));
        }
        let mut results = raw.results;
        results.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(results)
    }
}

fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
