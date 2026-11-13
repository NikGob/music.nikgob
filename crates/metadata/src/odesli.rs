//! song.link / album.link (Odesli) pages without its API, the share fallback
//! when MusicLink cannot build a page (an album new to it).
//!
//! The API (`api.song.link/v1-alpha.1`) was retired on 2026-07-31 and
//! answers 401 without a key; the public pages stay. A page's short address
//! is built from the service id (`song.link/s/<spotify id>`,
//! `album.link/d/<deezer id>`, `song.link/y/<youtube id>`,
//! `song.link/i/<apple id>`); YouTube Music albums have none. The page of
//! an item Odesli does not know answers 200 without an `og:title`, so a page
//! is checked once before it is shared. Checks are cached for the session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use url::Url;

use crate::error::Result;
use crate::http::{Http, Service, base_url};

/// Checks kept for the session, at most.
const CACHE_MAX: usize = 256;
/// A song.link page is about 80 KB; anything far bigger is not one.
const PAGE_MAX_BYTES: usize = 1024 * 1024;

/// The song.link / album.link page of a service page, when Odesli has a
/// short address for that service and kind.
pub fn page_url(service_url: &str) -> Option<String> {
    let url = Url::parse(service_url.trim()).ok()?;
    let host = url.host_str()?.trim_start_matches("www.");
    let segs: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    let id_ok = |id: &str| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let (album, code, id): (bool, &str, String) = match host {
        "open.spotify.com" => {
            // `/intl-xx/track/<id>` as well as `/track/<id>`.
            let at = segs.iter().position(|s| *s == "track" || *s == "album")?;
            (segs[at] == "album", "s", (*segs.get(at + 1)?).to_owned())
        }
        "deezer.com" => {
            let at = segs.iter().position(|s| *s == "track" || *s == "album")?;
            (segs[at] == "album", "d", (*segs.get(at + 1)?).to_owned())
        }
        "music.youtube.com" | "youtube.com" if segs.first() == Some(&"watch") => {
            let v = url.query_pairs().find(|(k, _)| k == "v")?.1.into_owned();
            (false, "y", v)
        }
        "youtu.be" => (false, "y", (*segs.first()?).to_owned()),
        "music.apple.com" => {
            // `/us/album/<name>/<album id>?i=<track id>`.
            let at = segs.iter().position(|s| *s == "album" || *s == "song")?;
            let last = (*segs.last()?).to_owned();
            match url.query_pairs().find(|(k, _)| k == "i") {
                Some((_, track)) => (false, "i", track.into_owned()),
                None if segs[at] == "song" => (false, "i", last),
                None if segs.len() > at + 1 => (true, "i", last),
                None => return None,
            }
        }
        _ => return None,
    };
    if !id_ok(&id) {
        return None;
    }
    let site = if album { "album.link" } else { "song.link" };
    Some(format!("https://{site}/{code}/{id}"))
}

/// Whether a song.link page shows an item: its `og:title` has text.
pub fn page_has_entity(html: &[u8]) -> bool {
    let html = String::from_utf8_lossy(html);
    let Some(at) = html.find("property=\"og:title\"") else {
        return false;
    };
    // The tag around it: `<meta property="og:title" content="…"/>`.
    let start = html[..at].rfind('<').unwrap_or(at);
    let end = html[at..].find('>').map_or(html.len(), |e| at + e);
    let tag = &html[start..end];
    tag.split("content=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .is_some_and(|c| !c.trim().is_empty())
}

#[derive(Clone, Debug)]
pub struct Odesli {
    http: Http,
    /// Where pages are fetched instead of their own host (tests).
    base: Option<Url>,
    cache: Arc<Mutex<HashMap<String, bool>>>,
}

impl Odesli {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            base: None,
            cache: Arc::default(),
        }
    }

    /// Fetches pages from `base` (their path kept) instead (tests).
    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self {
            base: Some(base_url(base)?),
            ..Self::new(http)
        })
    }

    /// Whether the page (from [`page_url`]) shows the item.
    pub async fn has_page(&self, page_url: &str) -> Result<bool> {
        if let Some(&hit) = self.cache.lock().unwrap_or_else(PoisonError::into_inner).get(page_url) {
            return Ok(hit);
        }
        let mut url = Url::parse(page_url)
            .map_err(|e| crate::error::MetadataError::InvalidArgument(format!("bad page url: {e}")))?;
        if let Some(base) = &self.base {
            url = base
                .join(url.path().trim_start_matches('/'))
                .map_err(|e| crate::error::MetadataError::InvalidArgument(format!("bad path: {e}")))?;
        }
        let found = match self.http.get_body_limited(Service::Odesli, url, None, PAGE_MAX_BYTES).await {
            Ok(body) => page_has_entity(&body),
            Err(crate::error::MetadataError::NotFound) => false,
            Err(e) => return Err(e),
        };
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.len() >= CACHE_MAX {
            cache.clear();
        }
        cache.insert(page_url.to_owned(), found);
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_pages_of_each_service() {
        let cases = [
            ("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT", Some("https://song.link/s/4cOdK2wGLETKBW3PvgPWqT")),
            ("https://open.spotify.com/intl-de/track/4cOdK2wGLETKBW3PvgPWqT?si=x", Some("https://song.link/s/4cOdK2wGLETKBW3PvgPWqT")),
            ("https://open.spotify.com/album/2noRn2Aes5aoNVsU6iWThc", Some("https://album.link/s/2noRn2Aes5aoNVsU6iWThc")),
            ("https://www.deezer.com/track/3966082521", Some("https://song.link/d/3966082521")),
            ("https://www.deezer.com/en/album/544629332", Some("https://album.link/d/544629332")),
            ("https://music.youtube.com/watch?v=dQw4w9WgXcQ", Some("https://song.link/y/dQw4w9WgXcQ")),
            ("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=3", Some("https://song.link/y/dQw4w9WgXcQ")),
            ("https://music.apple.com/us/album/discovery/697194953", Some("https://album.link/i/697194953")),
            ("https://music.apple.com/us/album/one-more-time/697194953?i=697195462", Some("https://song.link/i/697195462")),
            // YouTube Music albums have no short page; nor do other services.
            ("https://music.youtube.com/browse/MPREb_OLmD8O5IYNS", None),
            ("https://music.youtube.com/playlist?list=OLAK5uy_x", None),
            ("https://soundcloud.com/forss/flickermood", None),
            ("https://open.spotify.com/artist/x", None),
            ("https://open.spotify.com/track/", None),
            ("https://open.spotify.com/track/a%2Fb", None),
            ("not a url", None),
        ];
        for (from, want) in cases {
            assert_eq!(page_url(from).as_deref(), want, "{from}");
        }
    }

    #[test]
    fn a_page_without_a_title_shows_nothing() {
        assert!(page_has_entity(
            br#"<head><meta property="og:url" content="x"/><meta property="og:title" content="Discovery by Daft Punk"/></head>"#
        ));
        assert!(page_has_entity(r#"<meta property="og:title" content="оставь себе by Mzlff"/>"#.as_bytes()));
        assert!(!page_has_entity(br#"<meta property="og:title" content=" "/>"#));
        assert!(!page_has_entity(br#"<head><title></title></head>"#));
        assert!(!page_has_entity(b""));
    }
}
