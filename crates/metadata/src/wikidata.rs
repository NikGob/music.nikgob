//! Wikidata (what an item says of an artist: its English label, its
//! picture, its MusicBrainz ids) and Wikimedia Commons (the picture itself,
//! under a free license). The item is reached from a MusicBrainz artist's
//! Wikidata link, so it is that artist's own.

use serde_json::Value;
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url};

const API_BASE: &str = "https://www.wikidata.org/w/";
const COMMONS_BASE: &str = "https://commons.wikimedia.org/wiki/";
const MAX_IMAGE_BYTES: usize = 12 << 20;
/// Width of the Commons thumbnail asked for (drawn as an avatar).
const THUMB_WIDTH: &str = "1000";

#[derive(Clone, Debug)]
pub struct Wikidata {
    http: Http,
    api: Url,
    commons: Url,
}

/// What a Wikidata item says of an artist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WikidataArtist {
    pub id: String,
    /// Its English label: a Latin name.
    pub label_en: Option<String>,
    /// The Commons file of its picture (P18).
    pub image: Option<String>,
    /// Its MusicBrainz artist ids (P434).
    pub musicbrainz: Vec<String>,
}

impl Wikidata {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            api: base_url(API_BASE).expect("default base URL is valid"),
            commons: base_url(COMMONS_BASE).expect("default base URL is valid"),
        }
    }

    /// Points the client at other servers (tests).
    pub fn with_base_urls(http: Http, api: &str, commons: &str) -> Result<Self> {
        Ok(Self {
            http,
            api: base_url(api)?,
            commons: base_url(commons)?,
        })
    }

    /// The item `id` (`Q…`) as an artist.
    pub async fn artist(&self, id: &str) -> Result<WikidataArtist> {
        let digits = id.strip_prefix('Q').unwrap_or_default();
        if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
            return Err(MetadataError::InvalidArgument(format!("not a Wikidata item: {id:?}")));
        }
        let url = build_url(
            &self.api,
            "api.php",
            &[
                ("action", "wbgetentities"),
                ("ids", id),
                ("props", "labels|claims"),
                ("languages", "en"),
                ("format", "json"),
            ],
        )?;
        let v: Value = self.http.get_json(Service::Wikimedia, url).await?;
        let item = &v["entities"][id];
        if !item.is_object() || item.get("missing").is_some() {
            return Err(MetadataError::NotFound);
        }
        let strings = |property: &str| -> Vec<String> {
            item["claims"][property]
                .as_array()
                .map(|claims| {
                    claims
                        .iter()
                        .filter_map(|c| c["mainsnak"]["datavalue"]["value"].as_str())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok(WikidataArtist {
            id: id.to_owned(),
            label_en: item["labels"]["en"]["value"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
            image: strings("P18").into_iter().next(),
            musicbrainz: strings("P434"),
        })
    }

    /// The Commons picture `file` (a file name, as P18 gives it), about
    /// 1000 px wide.
    pub async fn download_image(&self, file: &str) -> Result<Vec<u8>> {
        let mut url = self.commons.clone();
        url.path_segments_mut()
            .map_err(|()| MetadataError::InvalidArgument("bad Commons URL".into()))?
            .pop_if_empty()
            .push("Special:FilePath")
            .push(file.trim());
        url.query_pairs_mut().append_pair("width", THUMB_WIDTH);
        self.http.get_bytes(Service::Wikimedia, url, MAX_IMAGE_BYTES).await
    }
}

/// Pictures on the services' CDNs (an artist's image in YouTube Music or
/// Spotify), with the shared limits.
#[derive(Clone, Debug)]
pub struct WebImages {
    http: Http,
}

impl WebImages {
    pub fn new(http: Http) -> Self {
        Self { http }
    }

    /// Downloads the picture at `url` (http or https only).
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let url = Url::parse(url.trim()).map_err(|e| MetadataError::InvalidArgument(format!("bad image URL: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(MetadataError::InvalidArgument("not a web image".into()));
        }
        self.http.get_bytes(Service::WebImages, url, MAX_IMAGE_BYTES).await
    }

    /// The cover of the Spotify album `id` by its public oEmbed (no
    /// account): its title and its thumbnail's url. `None`: it has none.
    /// It tells no songs.
    pub async fn spotify_album_art(&self, id: &str) -> Result<Option<SpotifyAlbumArt>> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(MetadataError::InvalidArgument(format!("not a Spotify id: {id:?}")));
        }
        let page = format!("https://open.spotify.com/album/{id}");
        let base = base_url(SPOTIFY_BASE)?;
        let url = build_url(&base, "oembed", &[("url", page.as_str())])?;
        let body: Value = self.http.get_json(Service::WebImages, url).await?;
        Ok(oembed_art(&body))
    }
}

/// A Spotify album as its oEmbed tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpotifyAlbumArt {
    pub title: String,
    pub thumbnail: String,
}

const SPOTIFY_BASE: &str = "https://open.spotify.com/";

/// The title and the thumbnail of an oEmbed answer.
fn oembed_art(body: &Value) -> Option<SpotifyAlbumArt> {
    let thumbnail = body.get("thumbnail_url").and_then(Value::as_str).filter(|u| u.starts_with("https://"))?;
    let title = body.get("title").and_then(Value::as_str).unwrap_or_default();
    Some(SpotifyAlbumArt { title: title.to_owned(), thumbnail: thumbnail.to_owned() })
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::{MetadataConfig, RateLimits};

    fn http() -> Http {
        Http::new(&MetadataConfig::new("tests@example.invalid")).unwrap().with_rate_limits(RateLimits::unlimited())
    }

    #[tokio::test]
    async fn an_item_gives_its_label_picture_and_musicbrainz_id() {
        let server = MockServer::start().await;
        let body = r#"{"entities": {"Q11": {"id": "Q11",
            "labels": {"en": {"language": "en", "value": "Ayako Saso"}},
            "claims": {"P18": [{"mainsnak": {"datavalue": {"value": "Ayako_Saso_2019.jpg", "type": "string"}}}],
                       "P434": [{"mainsnak": {"datavalue": {"value": "0b2f0b5a-0000-4000-8000-000000000001"}}}]}}}}"#;
        Mock::given(method("GET"))
            .and(path("/w/api.php"))
            .and(query_param("ids", "Q11"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(body, "application/json"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/wiki/Special:FilePath/Ayako_Saso_2019.jpg"))
            .and(query_param("width", "1000"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![0xFF, 0xD8, 0xFF, 1], "image/jpeg"))
            .mount(&server)
            .await;
        let wd = Wikidata::with_base_urls(http(), &format!("{}/w/", server.uri()), &format!("{}/wiki/", server.uri())).unwrap();
        let a = wd.artist("Q11").await.unwrap();
        assert_eq!(a.label_en.as_deref(), Some("Ayako Saso"));
        assert_eq!(a.image.as_deref(), Some("Ayako_Saso_2019.jpg"));
        assert_eq!(a.musicbrainz, vec!["0b2f0b5a-0000-4000-8000-000000000001".to_owned()]);
        assert_eq!(wd.download_image("Ayako_Saso_2019.jpg").await.unwrap(), vec![0xFF, 0xD8, 0xFF, 1]);
        assert!(wd.artist("P18").await.is_err());
    }

    #[tokio::test]
    async fn a_spotify_oembed_gives_its_thumbnail() {
        let body: Value = serde_json::from_str(
            r#"{"title":"Global Warming","thumbnail_url":"https://image-cdn-fa.spotifycdn.com/image/ab67616d00001e02aa","thumbnail_width":300}"#,
        )
        .unwrap();
        assert_eq!(
            oembed_art(&body),
            Some(SpotifyAlbumArt {
                title: "Global Warming".into(),
                thumbnail: "https://image-cdn-fa.spotifycdn.com/image/ab67616d00001e02aa".into()
            })
        );
        assert_eq!(oembed_art(&serde_json::json!({"title": "x"})), None);
        assert!(WebImages::new(http()).spotify_album_art("../x").await.is_err());
    }
}
