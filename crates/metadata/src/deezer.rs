//! Deezer public API (no key). Fallback source, good for Russian-language releases.
//!
//! Deezer answers errors with HTTP 200 and `{"error": {...}}`; those are mapped to
//! [`MetadataError`] here.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url};

const DEFAULT_BASE: &str = "https://api.deezer.com/";
const ALBUM_TRACKS_PAGE: usize = 100;
const ALBUM_TRACKS_MAX_PAGES: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DeezerArtist {
    pub id: u64,
    pub name: String,
}

/// Album as embedded in a track, or the full album from [`Deezer::album`].
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DeezerAlbum {
    pub id: u64,
    pub title: String,
    pub upc: Option<String>,
    pub cover_small: Option<String>,
    pub cover_medium: Option<String>,
    pub cover_big: Option<String>,
    /// 1000x1000.
    pub cover_xl: Option<String>,
    pub release_date: Option<String>,
    pub nb_tracks: Option<u32>,
    /// `album`, `single`, `ep`, `compile`.
    pub record_type: Option<String>,
    pub label: Option<String>,
    pub artist: Option<DeezerArtist>,
    /// Only filled by [`Deezer::album`].
    #[serde(rename = "tracks", deserialize_with = "tracks_data")]
    pub tracks: Vec<DeezerTrack>,
}

impl DeezerAlbum {
    /// Largest cover available.
    pub fn best_cover(&self) -> Option<&str> {
        [
            &self.cover_xl,
            &self.cover_big,
            &self.cover_medium,
            &self.cover_small,
        ]
        .into_iter()
        .filter_map(|c| c.as_deref())
        .find(|c| !c.is_empty())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DeezerTrack {
    pub id: u64,
    pub title: String,
    pub title_short: Option<String>,
    /// Seconds.
    pub duration: Option<u32>,
    /// Present on track lookups, absent from search results and album listings.
    pub isrc: Option<String>,
    pub track_position: Option<u32>,
    pub disk_number: Option<u32>,
    pub release_date: Option<String>,
    pub explicit_lyrics: Option<bool>,
    pub artist: Option<DeezerArtist>,
    pub album: Option<DeezerAlbum>,
}

impl DeezerTrack {
    pub fn duration(&self) -> Option<Duration> {
        self.duration.map(|s| Duration::from_secs(u64::from(s)))
    }

    pub fn artist_name(&self) -> &str {
        self.artist.as_ref().map_or("", |a| a.name.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct Deezer {
    http: Http,
    base: Url,
}

impl Deezer {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            base: base_url(DEFAULT_BASE).expect("default base URL is valid"),
        }
    }

    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self {
            http,
            base: base_url(base)?,
        })
    }

    async fn get<T: DeserializeOwned>(&self, path: &str, params: &[(&str, &str)]) -> Result<T> {
        let url = build_url(&self.base, path, params)?;
        let value: serde_json::Value = self.http.get_json(Service::Deezer, url).await?;
        if let Some(err) = value.get("error") {
            let code = err.get("code").and_then(serde_json::Value::as_i64);
            let message = err
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown Deezer error");
            return Err(match code {
                Some(800) => MetadataError::NotFound,
                Some(4 | 100) => MetadataError::RateLimited(None),
                _ => MetadataError::Service(message.to_owned()),
            });
        }
        serde_json::from_value(value).map_err(|e| MetadataError::Parse(e.to_string()))
    }

    /// `GET /search/track?q=artist:"x" track:"y"`.
    pub async fn search_track(
        &self,
        artist: Option<&str>,
        title: &str,
        limit: u32,
    ) -> Result<Vec<DeezerTrack>> {
        let mut q = String::new();
        if let Some(a) = artist.map(str::trim).filter(|a| !a.is_empty()) {
            q.push_str(&format!("artist:\"{}\" ", strip_quotes(a)));
        }
        q.push_str(&format!("track:\"{}\"", strip_quotes(title)));
        self.search_raw(&q, limit).await
    }

    /// Free-form query (`q=`), for when structured fields are unavailable.
    pub async fn search_raw(&self, q: &str, limit: u32) -> Result<Vec<DeezerTrack>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            data: Vec<DeezerTrack>,
        }
        let limit = limit.clamp(1, 100).to_string();
        let raw: Raw = self
            .get("search/track", &[("q", q), ("limit", limit.as_str())])
            .await?;
        Ok(raw.data)
    }

    /// Track lookup; unlike search results it carries the ISRC and release date.
    pub async fn track(&self, id: u64) -> Result<DeezerTrack> {
        self.get(&format!("track/{id}"), &[]).await
    }

    /// Album with its full track list.
    pub async fn album(&self, id: u64) -> Result<DeezerAlbum> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default, deserialize_with = "null_default")]
            data: Vec<DeezerTrack>,
        }
        let mut album: DeezerAlbum = self.get(&format!("album/{id}"), &[]).await?;
        // The embedded list is capped; page through the rest when it is short.
        let want = album.nb_tracks.unwrap_or(0) as usize;
        let mut pages = 0;
        while album.tracks.len() < want && pages < ALBUM_TRACKS_MAX_PAGES {
            pages += 1;
            let index = album.tracks.len().to_string();
            let limit = ALBUM_TRACKS_PAGE.to_string();
            let page: Page = self
                .get(
                    &format!("album/{id}/tracks"),
                    &[("index", index.as_str()), ("limit", limit.as_str())],
                )
                .await?;
            if page.data.is_empty() {
                break;
            }
            album.tracks.extend(page.data);
        }
        Ok(album)
    }
}

fn strip_quotes(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '"')
        .collect::<String>()
        .trim()
        .to_owned()
}

fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// `"tracks": {"data": [...]}` -> the inner vector.
fn tracks_data<'de, D>(d: D) -> std::result::Result<Vec<DeezerTrack>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default, deserialize_with = "null_default")]
        data: Vec<DeezerTrack>,
    }
    Ok(Option::<Wrapper>::deserialize(d)?
        .map(|w| w.data)
        .unwrap_or_default())
}
