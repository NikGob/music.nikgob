//! Odesli (song.link / album.link): one share link for a track or album that
//! opens in whatever service the friend uses (docs/stage3.md §0, "Поделиться").
//!
//! `GET https://api.song.link/v1-alpha.1/links?url=<service url>` answers with
//! `pageUrl` (`https://song.link/…` for a song, `https://album.link/…` for an
//! album). Without an API key the limit is 10 requests a minute: requests are
//! spaced by the shared rate limiter ([`crate::RateLimits::odesli`]) and every
//! answer, found or not, is cached for the session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Deserialize;
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url, parse_json};

const DEFAULT_BASE: &str = "https://api.song.link/v1-alpha.1/";

/// What Odesli knows about a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OdesliPage {
    /// The aggregator page to share.
    pub page_url: String,
    /// Title and artist of the entity the link was resolved to, when given.
    pub title: Option<String>,
    pub artist: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinksResponse {
    page_url: Option<String>,
    entity_unique_id: Option<String>,
    #[serde(default)]
    entities_by_unique_id: HashMap<String, Entity>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entity {
    title: Option<String>,
    artist_name: Option<String>,
}

/// Parses a `/links` answer; `None` when it has no page url.
pub fn parse_links(body: &[u8]) -> Result<Option<OdesliPage>> {
    let r: LinksResponse = parse_json(body)?;
    let Some(page_url) = r.page_url.filter(|u| !u.trim().is_empty()) else {
        return Ok(None);
    };
    let entity = r
        .entity_unique_id
        .as_ref()
        .and_then(|id| r.entities_by_unique_id.get(id));
    Ok(Some(OdesliPage {
        page_url,
        title: entity.and_then(|e| e.title.clone()),
        artist: entity.and_then(|e| e.artist_name.clone()),
    }))
}

#[derive(Clone, Debug)]
pub struct Odesli {
    http: Http,
    base: Url,
    cache: Arc<Mutex<HashMap<String, Option<OdesliPage>>>>,
}

impl Odesli {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            base: base_url(DEFAULT_BASE).expect("default base URL is valid"),
            cache: Arc::default(),
        }
    }

    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self {
            base: base_url(base)?,
            ..Self::new(http)
        })
    }

    /// The share page of a service url (`https://open.spotify.com/track/…`,
    /// `https://music.youtube.com/watch?v=…`); `None` when Odesli does not
    /// know it. Cached per url for the session.
    pub async fn page(&self, service_url: &str) -> Result<Option<OdesliPage>> {
        if let Some(hit) = self.cached(service_url) {
            return Ok(hit);
        }
        let url = build_url(&self.base, "links", &[("url", service_url)])?;
        let found = match self.http.get_body(Service::Odesli, url).await {
            Ok(body) => parse_links(&body)?,
            // An unknown entity is a 400 or a 404 ("could_not_resolve_entity").
            Err(MetadataError::NotFound | MetadataError::Status { status: 400, .. }) => None,
            Err(e) => return Err(e),
        };
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(service_url.to_owned(), found.clone());
        Ok(found)
    }

    fn cached(&self, service_url: &str) -> Option<Option<OdesliPage>> {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(service_url)
            .cloned()
    }
}
