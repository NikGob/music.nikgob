//! MusicLink (musiclink.one): one page for a song or an album with every
//! service on it, the page «Поделиться» copies.
//!
//! Without a key: `GET https://musiclink.one/resolve?q=<service url>` is the
//! site's own paste box. For a song (and an album it already knows) it
//! answers `307 Location: /song/…` (`/album/…`): the page is ready and that
//! address is what we share. `200` means it cannot build the page on its
//! server (an unknown item, or an album new to it, which it builds in the
//! visitor's browser behind a bot check).
//!
//! With the user's own key (free plan: 300 a month, one a second):
//! `GET https://api.musiclink.one/v2/resolve?q=<service url>` with
//! `Authorization: Bearer <key>` builds any page and answers
//! `{"success":true,"data":[{"url":"https://musiclink.one/…"}]}`; 404 is
//! "nothing matched".
//!
//! Answers are cached for the session (a bounded map).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use serde::Deserialize;
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url, parse_json};

/// Where the shared pages live.
pub const ORIGIN: &str = "https://musiclink.one";
const API_BASE: &str = "https://api.musiclink.one/";
/// Answers kept for the session, at most.
const CACHE_MAX: usize = 256;

/// The keyless share link of a service page: MusicLink resolves it when
/// opened (the page itself when [`MusicLink::page`] found one is better).
pub fn resolve_url(service_url: &str) -> String {
    let mut url = Url::parse(ORIGIN).expect("origin is valid");
    url.set_path("/resolve");
    url.query_pairs_mut().append_pair("q", service_url);
    url.into()
}

/// The share page a resolve redirect points at: `/song/…` or `/album/…` on
/// musiclink.one (relative or absolute), with its query kept. Anything else
/// (the home page, a sign-in, another host) is not a page.
pub fn page_from_location(location: &str) -> Option<String> {
    let origin = Url::parse(ORIGIN).expect("origin is valid");
    let url = origin.join(location.trim()).ok()?;
    if url.host_str() != origin.host_str() || url.scheme() != "https" {
        return None;
    }
    let path = url.path();
    let page = ["/song/", "/album/"]
        .iter()
        .any(|p| path.starts_with(p) && path.len() > p.len());
    if !page {
        return None;
    }
    let mut out = format!("{ORIGIN}{path}");
    if let Some(q) = url.query().filter(|q| !q.is_empty()) {
        out.push('?');
        out.push_str(q);
    }
    Some(out)
}

#[derive(Deserialize)]
struct ResolveResponse {
    #[serde(default)]
    data: Vec<ResolveItem>,
}

#[derive(Deserialize)]
struct ResolveItem {
    url: Option<String>,
    #[serde(default)]
    platforms: HashMap<String, serde_json::Value>,
}

/// The page url in an API `/v2/resolve` answer; `None` when it has none.
pub fn parse_resolve(body: &[u8]) -> Result<Option<String>> {
    let r: ResolveResponse = parse_json(body)?;
    Ok(r.data.into_iter().find_map(|item| {
        item.url
            .or_else(|| item.platforms.get("musiclink").and_then(|v| v.as_str()).map(str::to_owned))
            .and_then(|u| page_from_location(&u))
    }))
}

#[derive(Clone, Debug)]
pub struct MusicLink {
    http: Http,
    /// The site (`/resolve`).
    site: Url,
    /// The API (`/v2/resolve`).
    api: Url,
    cache: Arc<Mutex<HashMap<String, Option<String>>>>,
}

impl MusicLink {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            site: base_url(ORIGIN).expect("origin is valid"),
            api: base_url(API_BASE).expect("API base is valid"),
            cache: Arc::default(),
        }
    }

    /// Both the site and the API at `base` (tests).
    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        let base = base_url(base)?;
        Ok(Self {
            site: base.clone(),
            api: base,
            ..Self::new(http)
        })
    }

    /// The share page of a service page, without a key; `None` when
    /// MusicLink cannot build it on its server.
    pub async fn page(&self, service_url: &str) -> Result<Option<String>> {
        let key = format!("site {service_url}");
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        let url = build_url(&self.site, "resolve", &[("q", service_url)])?;
        let (status, location) = self.http.get_location(Service::MusicLink, url).await?;
        let found = if status.is_redirection() {
            location.as_deref().and_then(page_from_location)
        } else if status.is_success() || status.as_u16() == 404 {
            None
        } else {
            return Err(MetadataError::Status {
                status: status.as_u16(),
                body: String::new(),
            });
        };
        self.remember(key, found.clone());
        Ok(found)
    }

    /// The share page through the API with the user's key; builds pages the
    /// keyless way cannot. `None`: nothing matched.
    pub async fn api_page(&self, api_key: &str, service_url: &str) -> Result<Option<String>> {
        let key = format!("api {service_url}");
        if let Some(hit) = self.cached(&key) {
            return Ok(hit);
        }
        let url = build_url(&self.api, "v2/resolve", &[("q", service_url)])?;
        let auth = format!("Bearer {}", api_key.trim());
        let found = match self.http.get_body_auth(Service::MusicLink, url, &auth).await {
            Ok(body) => parse_resolve(&body)?,
            Err(MetadataError::NotFound) => None,
            Err(e) => return Err(e),
        };
        self.remember(key, found.clone());
        Ok(found)
    }

    fn cached(&self, key: &str) -> Option<Option<String>> {
        self.cache.lock().unwrap_or_else(PoisonError::into_inner).get(key).cloned()
    }

    fn remember(&self, key: String, found: Option<String>) {
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= CACHE_MAX {
            cache.clear();
        }
        cache.insert(key, found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resolve_link_carries_the_service_page() {
        assert_eq!(
            resolve_url("https://music.youtube.com/watch?v=dQw4w9WgXcQ"),
            "https://musiclink.one/resolve?q=https%3A%2F%2Fmusic.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ"
        );
        assert_eq!(
            resolve_url("https://open.spotify.com/album/2noRn2Aes5aoNVsU6iWThc"),
            "https://musiclink.one/resolve?q=https%3A%2F%2Fopen.spotify.com%2Falbum%2F2noRn2Aes5aoNVsU6iWThc"
        );
    }

    #[test]
    fn a_redirect_to_a_song_or_an_album_is_the_page() {
        assert_eq!(
            page_from_location("/song/mzlff/odnopolyarnosti").as_deref(),
            Some("https://musiclink.one/song/mzlff/odnopolyarnosti")
        );
        assert_eq!(
            page_from_location("/song/rick-astley/never-gonna-give-you-up?i=2").as_deref(),
            Some("https://musiclink.one/song/rick-astley/never-gonna-give-you-up?i=2")
        );
        assert_eq!(
            page_from_location("https://musiclink.one/album/daft-punk/discovery").as_deref(),
            Some("https://musiclink.one/album/daft-punk/discovery")
        );
        assert_eq!(page_from_location("/"), None);
        assert_eq!(page_from_location("/song/"), None);
        assert_eq!(page_from_location("/sign-in?next=/song/a/b"), None);
        assert_eq!(page_from_location("https://evil.example/song/a/b"), None);
        assert_eq!(page_from_location("http://musiclink.one/song/a/b"), None);
    }

    #[test]
    fn the_api_answer_gives_the_page() {
        let body = br#"{"success":true,"data":[{"type":"album","id":"QrDNOgP5KXH",
            "url":"https://musiclink.one/album/daft-punk/discovery","title":"Discovery",
            "platforms":{"spotify":"https://open.spotify.com/album/x"}}]}"#;
        assert_eq!(
            parse_resolve(body).unwrap().as_deref(),
            Some("https://musiclink.one/album/daft-punk/discovery")
        );
        let only_platforms = br#"{"success":true,"data":[{"type":"track",
            "platforms":{"musiclink":"https://musiclink.one/song/a/b"}}]}"#;
        assert_eq!(parse_resolve(only_platforms).unwrap().as_deref(), Some("https://musiclink.one/song/a/b"));
        assert_eq!(parse_resolve(br#"{"success":true,"data":[]}"#).unwrap(), None);
        assert_eq!(parse_resolve(br#"{"success":false,"error":"x"}"#).unwrap(), None);
        assert!(parse_resolve(b"<html>").is_err());
    }
}
