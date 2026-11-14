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
use crate::text::{album_key, artist_key, search_artist, search_title, similarity, title_key};

const DEFAULT_BASE: &str = "https://api.deezer.com/";
const ALBUM_TRACKS_PAGE: usize = 100;
const ALBUM_TRACKS_MAX_PAGES: usize = 10;
const MAX_IMAGE_BYTES: usize = 12 << 20;
/// A 30-second preview MP3 is about half a megabyte.
const MAX_PREVIEW_BYTES: usize = 2 << 20;
/// Minimum name similarity (0..=1, see `text::similarity`) of an artist hit.
const MIN_ARTIST_SIMILARITY: f64 = 0.85;
/// Minimum title similarity of a track / album match.
const MIN_TITLE_SIMILARITY: f64 = 0.9;
/// Track durations further apart than this (seconds) are different recordings.
const MAX_DURATION_DIFF_SECS: u64 = 5;
/// How many same-named artists are checked against the local titles.
const MAX_VERIFIED_CANDIDATES: usize = 5;

/// What [`Deezer::verified_artist_lookup`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtistPicture {
    /// The photo of an artist proved to be this one.
    Found(String),
    /// Deezer has no artist of this name with a photo.
    Missing,
    /// Same-named artists with photos, none proved to be this one.
    Unverified,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DeezerArtist {
    pub id: u64,
    pub name: String,
}

/// An artist from `search/artist`, with photos.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct DeezerArtistHit {
    pub id: u64,
    pub name: String,
    pub picture_small: Option<String>,
    pub picture_medium: Option<String>,
    pub picture_big: Option<String>,
    /// 1000x1000.
    pub picture_xl: Option<String>,
    pub nb_fan: Option<u64>,
}

impl DeezerArtistHit {
    /// Largest real photo; Deezer's grey "no photo" placeholder (an image URL
    /// with an empty hash, `/images/artist//...`) does not count.
    pub fn best_picture(&self) -> Option<&str> {
        [
            &self.picture_xl,
            &self.picture_big,
            &self.picture_medium,
            &self.picture_small,
        ]
        .into_iter()
        .filter_map(|c| c.as_deref())
        .find(|c| !c.is_empty() && !c.contains("/artist//"))
    }
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
    /// The URL of its free 30-second preview (an MP3 on Deezer's CDN), when
    /// Deezer has one.
    pub preview: Option<String>,
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

    /// `GET /search/album?q=…` (free-form: Deezer's `artist:"…"` and
    /// `album:"…"` fields find nothing on this endpoint and on tracks).
    pub async fn search_album(&self, q: &str, limit: u32) -> Result<Vec<DeezerAlbum>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            data: Vec<DeezerAlbum>,
        }
        let limit = limit.clamp(1, 100).to_string();
        let raw: Raw = self
            .get("search/album", &[("q", q), ("limit", limit.as_str())])
            .await?;
        Ok(raw.data)
    }

    /// `GET /search/artist?q=name`.
    pub async fn search_artist(&self, name: &str, limit: u32) -> Result<Vec<DeezerArtistHit>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            data: Vec<DeezerArtistHit>,
        }
        let limit = limit.clamp(1, 50).to_string();
        let q = strip_quotes(name);
        let raw: Raw = self
            .get(
                "search/artist",
                &[("q", q.as_str()), ("limit", limit.as_str())],
            )
            .await?;
        Ok(raw.data)
    }

    /// `GET /artist/{id}`.
    pub async fn artist(&self, id: u64) -> Result<DeezerArtistHit> {
        self.get(&format!("artist/{id}"), &[]).await
    }

    /// `GET /artist/{id}/albums`.
    pub async fn artist_albums(&self, id: u64, limit: u32) -> Result<Vec<DeezerAlbum>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            data: Vec<DeezerAlbum>,
        }
        let limit = limit.clamp(1, 100).to_string();
        let raw: Raw = self
            .get(&format!("artist/{id}/albums"), &[("limit", limit.as_str())])
            .await?;
        Ok(raw.data)
    }

    /// `GET /artist/{id}/top`.
    pub async fn artist_top(&self, id: u64, limit: u32) -> Result<Vec<DeezerTrack>> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default, deserialize_with = "null_default")]
            data: Vec<DeezerTrack>,
        }
        let limit = limit.clamp(1, 100).to_string();
        let raw: Raw = self
            .get(&format!("artist/{id}/top"), &[("limit", limit.as_str())])
            .await?;
        Ok(raw.data)
    }

    /// Photo URL of the artist who recorded the given local track. The track is
    /// searched on Deezer and only a confident match (artist, title and, when
    /// both are known, duration) lends its `artist.id`: immune to name clashes.
    /// When the structured search (`artist:"…" track:"…"`) finds nothing,
    /// the plain one is asked with the same checks: Deezer's field search
    /// misses songs its plain search has.
    pub async fn artist_picture_by_track(
        &self,
        artist: &str,
        title: &str,
        duration: Option<Duration>,
    ) -> Result<Option<String>> {
        let want_artist = artist_key(artist);
        let want_title = title_key(title).base;
        if want_artist.is_empty() || want_title.is_empty() {
            return Ok(None);
        }
        let mut hits = self
            .search_track(Some(&search_artist(artist)), &search_title(title), 10)
            .await?;
        if hits.is_empty() {
            let plain = format!("{} {}", search_artist(artist), search_title(title));
            hits = self.search_raw(&plain, 10).await?;
        }
        let found = hits.into_iter().find(|t| {
            let Some(a) = t.artist.as_ref().filter(|a| a.id != 0) else {
                return false;
            };
            let duration_ok = match (duration, t.duration) {
                (Some(local), Some(remote)) => {
                    local.as_secs().abs_diff(u64::from(remote)) <= MAX_DURATION_DIFF_SECS
                }
                _ => true,
            };
            duration_ok
                && similarity(&want_artist, &artist_key(&a.name)) >= MIN_ARTIST_SIMILARITY
                && similarity(&want_title, &title_key(&t.title).base) >= MIN_TITLE_SIMILARITY
        });
        let Some(id) = found.and_then(|t| t.artist).map(|a| a.id) else {
            return Ok(None);
        };
        Ok(self.artist(id).await?.best_picture().map(str::to_owned))
    }

    /// Photo URL of the artist called `name`, accepted only when one of that
    /// Deezer artist's albums or top tracks matches one of `known_titles` (titles
    /// of the user's own albums and tracks by this artist). Among same-named
    /// candidates the most popular verified one wins; none verified -> `None`.
    pub async fn verified_artist_picture(
        &self,
        name: &str,
        known_titles: &[String],
    ) -> Result<Option<String>> {
        Ok(match self.verified_artist_lookup(name, known_titles).await? {
            ArtistPicture::Found(url) => Some(url),
            ArtistPicture::Missing | ArtistPicture::Unverified => None,
        })
    }

    /// [`Self::verified_artist_picture`] telling «Deezer has no such artist
    /// with a photo» ([`ArtistPicture::Missing`]) from «same-named artists
    /// with photos, none proved this one» ([`ArtistPicture::Unverified`]).
    pub async fn verified_artist_lookup(
        &self,
        name: &str,
        known_titles: &[String],
    ) -> Result<ArtistPicture> {
        let want = artist_key(name);
        let known: Vec<String> = known_titles
            .iter()
            .map(|t| album_key(t))
            .filter(|k| !k.is_empty())
            .collect();
        if want.is_empty() || known.is_empty() {
            return Ok(ArtistPicture::Missing);
        }
        let mut candidates: Vec<DeezerArtistHit> = self
            .search_artist(name, 10)
            .await?
            .into_iter()
            .filter(|h| h.best_picture().is_some())
            .filter(|h| similarity(&want, &artist_key(&h.name)) >= MIN_ARTIST_SIMILARITY)
            .collect();
        candidates.sort_by_key(|h| std::cmp::Reverse(h.nb_fan.unwrap_or(0)));
        if candidates.is_empty() {
            return Ok(ArtistPicture::Missing);
        }
        for hit in candidates.into_iter().take(MAX_VERIFIED_CANDIDATES) {
            let mut theirs: Vec<String> = Vec::new();
            match self.artist_albums(hit.id, 100).await {
                Ok(a) => theirs.extend(a.into_iter().map(|a| a.title)),
                Err(MetadataError::NotFound) => {}
                Err(e) => return Err(e),
            }
            match self.artist_top(hit.id, 50).await {
                Ok(t) => theirs.extend(t.into_iter().map(|t| t.title)),
                Err(MetadataError::NotFound) => {}
                Err(e) => return Err(e),
            }
            let overlaps = theirs.iter().map(|t| album_key(t)).any(|t| {
                !t.is_empty()
                    && known
                        .iter()
                        .any(|k| similarity(k, &t) >= MIN_TITLE_SIMILARITY)
            });
            if overlaps && let Some(url) = hit.best_picture() {
                return Ok(ArtistPicture::Found(url.to_owned()));
            }
        }
        Ok(ArtistPicture::Unverified)
    }

    /// The free 30-second preview of a song: of Deezer's track `id` when it
    /// is known, else of the hit of a search by `artist` and `title` whose
    /// artist and title match (the same checks as the artist's photo; the
    /// variant of the title too: an instrumental is not the song). `None`
    /// when Deezer has neither the song nor a preview of it.
    pub async fn preview_url(&self, id: Option<u64>, artist: &str, title: &str, duration: Option<Duration>) -> Result<Option<String>> {
        if let Some(id) = id {
            return Ok(self.track(id).await?.preview.filter(|p| !p.is_empty()));
        }
        let want_artist = artist_key(artist);
        let want = title_key(title);
        if want_artist.is_empty() || want.base.is_empty() {
            return Ok(None);
        }
        let hits = self.search_track(Some(&search_artist(artist)), &search_title(title), 10).await?;
        let found = hits.into_iter().find(|t| {
            let duration_ok = match (duration, t.duration) {
                (Some(local), Some(remote)) => local.as_secs().abs_diff(u64::from(remote)) <= MAX_DURATION_DIFF_SECS,
                _ => true,
            };
            let theirs = title_key(&t.title);
            duration_ok
                && t.preview.as_deref().is_some_and(|p| !p.is_empty())
                && similarity(&want_artist, &artist_key(t.artist_name())) >= MIN_ARTIST_SIMILARITY
                && similarity(&want.base, &theirs.base) >= MIN_TITLE_SIMILARITY
                && theirs.variant == want.variant
        });
        Ok(found.and_then(|t| t.preview))
    }

    /// The free preview of the song of `isrc` (Deezer's public lookup by
    /// ISRC, no login): the very recording, whatever its credited artist
    /// and title. `None` when Deezer has no such track or no preview of it;
    /// an ISRC not of twelve letters and digits is not asked.
    pub async fn preview_by_isrc(&self, isrc: &str) -> Result<Option<String>> {
        let isrc: String = isrc.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect();
        if isrc.len() != 12 {
            return Ok(None);
        }
        // Deezer answers an unknown ISRC with «no data»: no track.
        let t = match self.get::<DeezerTrack>(&format!("track/isrc:{isrc}"), &[]).await {
            Ok(t) => t,
            Err(MetadataError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok((t.id > 0).then_some(t.preview).flatten().filter(|p| !p.is_empty()))
    }

    /// Downloads a preview `url` (an MP3 of Deezer's CDN).
    pub async fn download_preview(&self, url: &str) -> Result<Vec<u8>> {
        let url = Url::parse(url).map_err(|e| MetadataError::InvalidArgument(format!("bad preview URL: {e}")))?;
        self.http.get_bytes(Service::Deezer, url, MAX_PREVIEW_BYTES).await
    }

    /// Downloads an image URL from Deezer's CDN (photos, covers).
    pub async fn download_image(&self, url: &str) -> Result<Vec<u8>> {
        let url = Url::parse(url)
            .map_err(|e| MetadataError::InvalidArgument(format!("bad image URL: {e}")))?;
        self.http
            .get_bytes(Service::Deezer, url, MAX_IMAGE_BYTES)
            .await
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
