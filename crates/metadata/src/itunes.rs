//! The iTunes Search API (no key): album art of a song, for covers the
//! other sources do not have (a single only on YouTube, a soundtrack Deezer
//! lacks).
//!
//! `GET https://itunes.apple.com/search?term=<artist title>&entity=song`
//! answers with songs and their `artworkUrl100`; the same url with another
//! size (`…/1200x1200bb.jpg`) gives the art larger. `entity=album` answers
//! with albums, their songs counted. Apple asks for about 20
//! requests a minute: requests are spaced by the shared rate limiter
//! ([`crate::RateLimits::itunes`]).

use serde::Deserialize;
use url::Url;

use crate::error::Result;
use crate::http::{Http, Service, base_url, build_url, parse_json};

const DEFAULT_BASE: &str = "https://itunes.apple.com/";
/// The side the art is asked at.
pub const ART_SIDE: u32 = 1200;

/// A song of the store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ItunesSong {
    pub artist_name: String,
    pub track_name: String,
    pub collection_name: Option<String>,
    pub artwork_url100: Option<String>,
}

impl ItunesSong {
    /// The art at [`ART_SIDE`] pixels.
    pub fn artwork(&self) -> Option<String> {
        self.artwork_url100.as_deref().filter(|u| !u.is_empty()).map(|u| sized_artwork(u, ART_SIDE))
    }
}

/// An album of the store. Its name ends in « - Single» or « - EP» for
/// those.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ItunesAlbum {
    pub artist_name: String,
    pub collection_name: String,
    pub track_count: Option<u32>,
    pub artwork_url100: Option<String>,
}

/// `…/100x100bb.jpg` → `…/{side}x{side}bb.jpg`; any other url as it is.
pub fn sized_artwork(url: &str, side: u32) -> String {
    match url.rsplit_once('/') {
        Some((base, file)) if file.contains("x") && file.contains("bb.") => {
            let ext = file.rsplit_once('.').map_or("jpg", |(_, e)| e);
            format!("{base}/{side}x{side}bb.{ext}")
        }
        _ => url.to_owned(),
    }
}

/// Parses a `/search` answer.
pub fn parse_search(body: &[u8]) -> Result<Vec<ItunesSong>> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        results: Vec<ItunesSong>,
    }
    let raw: Raw = parse_json(body)?;
    Ok(raw.results)
}

/// Parses a `/search` answer of albums.
pub fn parse_albums(body: &[u8]) -> Result<Vec<ItunesAlbum>> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        results: Vec<ItunesAlbum>,
    }
    let raw: Raw = parse_json(body)?;
    Ok(raw.results)
}

#[derive(Clone, Debug)]
pub struct Itunes {
    http: Http,
    base: Url,
}

impl Itunes {
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

    /// Songs for a free-form `term` («artist title»), at most `limit`.
    pub async fn search_songs(&self, term: &str, limit: u32) -> Result<Vec<ItunesSong>> {
        let limit = limit.clamp(1, 50).to_string();
        let url = build_url(&self.base, "search", &[("term", term), ("entity", "song"), ("limit", limit.as_str())])?;
        let body = self.http.get_body(Service::Itunes, url).await?;
        parse_search(&body)
    }

    /// Albums for a free-form `term` («artist title»), at most `limit`.
    pub async fn search_albums(&self, term: &str, limit: u32) -> Result<Vec<ItunesAlbum>> {
        let limit = limit.clamp(1, 50).to_string();
        let url = build_url(&self.base, "search", &[("term", term), ("entity", "album"), ("limit", limit.as_str())])?;
        let body = self.http.get_body(Service::Itunes, url).await?;
        parse_albums(&body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_search_answer_gives_songs_and_their_large_art() {
        let body = br#"{"resultCount":2,"results":[
            {"wrapperType":"track","kind":"song","artistName":"Yorushika","trackName":"Martian","collectionName":"Martian - Single",
             "artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music/v4/ab/cd/x.jpg/100x100bb.jpg"},
            {"wrapperType":"track","kind":"song","artistName":"Other","trackName":"Song"}]}"#;
        let songs = parse_search(body).unwrap();
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].collection_name.as_deref(), Some("Martian - Single"));
        assert_eq!(
            songs[0].artwork().as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music/v4/ab/cd/x.jpg/1200x1200bb.jpg")
        );
        assert_eq!(songs[1].artwork(), None);
        assert_eq!(sized_artwork("https://x/y.png", 600), "https://x/y.png");
    }

    #[test]
    fn an_album_search_answer_gives_albums_and_their_songs_counted() {
        let body = br#"{"resultCount":1,"results":[
            {"wrapperType":"collection","collectionType":"Album","artistName":"nyan.mp3","collectionName":"Drawings - Single",
             "trackCount":3,"artworkUrl100":"https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/eb/cover.jpg/100x100bb.jpg"}]}"#;
        let albums = parse_albums(body).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].collection_name, "Drawings - Single");
        assert_eq!(albums[0].track_count, Some(3));
        assert_eq!(
            sized_artwork(albums[0].artwork_url100.as_deref().unwrap(), 3000),
            "https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/eb/cover.jpg/3000x3000bb.jpg"
        );
    }
}
