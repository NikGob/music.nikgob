//! Cover Art Archive: front cover of a release or release group.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url};

const DEFAULT_BASE: &str = "https://coverartarchive.org/";
/// Plan section 5: covers are kept at up to ~1200 px.
pub const DEFAULT_MAX_EDGE: u32 = 1200;
const MAX_COVER_BYTES: usize = 24 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoverTarget {
    Release(String),
    ReleaseGroup(String),
}

impl CoverTarget {
    fn path(&self) -> Result<String> {
        let (kind, id) = match self {
            Self::Release(id) => ("release", id),
            Self::ReleaseGroup(id) => ("release-group", id),
        };
        let ok = id.len() == 36 && id.chars().all(|c| c == '-' || c.is_ascii_hexdigit());
        if ok {
            Ok(format!("{kind}/{id}"))
        } else {
            Err(MetadataError::InvalidArgument(format!(
                "not a MusicBrainz id: {id:?}"
            )))
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct CoverImage {
    /// URL of the original upload (may be huge).
    pub image: String,
    /// Keys: `"250"`, `"500"`, `"1200"`, plus legacy `"small"` (=250) / `"large"` (=500).
    #[serde(deserialize_with = "null_default")]
    pub thumbnails: BTreeMap<String, String>,
    pub front: bool,
    pub back: bool,
    pub approved: bool,
    #[serde(deserialize_with = "null_default")]
    pub types: Vec<String>,
}

impl CoverImage {
    /// Largest rendition with edge <= `max_edge` (1200 -> 500 -> 250); if none fits,
    /// the smallest available thumbnail; the original only when there are no thumbnails.
    pub fn url_for(&self, max_edge: u32) -> &str {
        let mut sized: Vec<(u32, &str)> = self
            .thumbnails
            .iter()
            .filter_map(|(k, v)| {
                let edge = match k.as_str() {
                    "small" => 250,
                    "large" => 500,
                    n => n.parse().ok()?,
                };
                Some((edge, v.as_str()))
            })
            .collect();
        sized.sort_by_key(|(edge, _)| *edge);
        sized
            .iter()
            .rev()
            .find(|(edge, _)| *edge <= max_edge)
            .or_else(|| sized.first())
            .map_or(self.image.as_str(), |(_, url)| url)
    }
}

/// A downloaded cover.
#[derive(Debug, Clone)]
pub struct FetchedCover {
    pub url: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct CoverArtArchive {
    http: Http,
    base: Url,
}

impl CoverArtArchive {
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

    /// The front image listing, preferring approved images. `None` when the
    /// archive has no art (404) or no front image.
    pub async fn front_cover(&self, target: &CoverTarget) -> Result<Option<CoverImage>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            images: Vec<CoverImage>,
        }
        let url = build_url(&self.base, &target.path()?, &[])?;
        let raw: Raw = match self.http.get_json(Service::CoverArt, url).await {
            Ok(raw) => raw,
            Err(MetadataError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut fronts: Vec<CoverImage> = raw
            .images
            .into_iter()
            .filter(|i| i.front || i.types.iter().any(|t| t.eq_ignore_ascii_case("front")))
            .collect();
        fronts.sort_by_key(|i| !i.approved);
        Ok(fronts.into_iter().next())
    }

    /// Downloads an image URL as returned by [`CoverImage::url_for`].
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let url = Url::parse(&upgrade_https(url))
            .map_err(|e| MetadataError::InvalidArgument(format!("bad cover URL: {e}")))?;
        self.http
            .get_bytes(Service::CoverArt, url, MAX_COVER_BYTES)
            .await
    }

    /// `front_cover` + `download` of the rendition for `max_edge`.
    pub async fn fetch_front(
        &self,
        target: &CoverTarget,
        max_edge: u32,
    ) -> Result<Option<FetchedCover>> {
        let Some(image) = self.front_cover(target).await? else {
            return Ok(None);
        };
        let url = image.url_for(max_edge).to_owned();
        if url.is_empty() {
            return Ok(None);
        }
        let bytes = match self.download(&url).await {
            Ok(b) => b,
            Err(MetadataError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(Some(FetchedCover { url, bytes }))
    }

    /// Tries the release first, then falls back to its release group.
    pub async fn fetch_front_with_fallback(
        &self,
        release_mbid: &str,
        release_group_mbid: Option<&str>,
        max_edge: u32,
    ) -> Result<Option<FetchedCover>> {
        if let Some(c) = self
            .fetch_front(&CoverTarget::Release(release_mbid.to_owned()), max_edge)
            .await?
        {
            return Ok(Some(c));
        }
        match release_group_mbid {
            Some(rg) => {
                self.fetch_front(&CoverTarget::ReleaseGroup(rg.to_owned()), max_edge)
                    .await
            }
            None => Ok(None),
        }
    }
}

/// The archive's JSON still contains `http://` links; its servers speak https.
fn upgrade_https(url: &str) -> String {
    for host in ["coverartarchive.org", "archive.org"] {
        let prefix = format!("http://{host}");
        if let Some(rest) = url.strip_prefix(&prefix) {
            return format!("https://{host}{rest}");
        }
    }
    // Subdomains such as `ia800000.us.archive.org`.
    if let Some(rest) = url.strip_prefix("http://")
        && rest
            .split('/')
            .next()
            .is_some_and(|h| h.ends_with(".archive.org"))
    {
        return format!("https://{rest}");
    }
    url.to_owned()
}

fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}
