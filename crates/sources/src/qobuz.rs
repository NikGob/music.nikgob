//! Qobuz through a public Qobuz-DL instance (docs/stage4.5.md §4): the
//! free FLAC downloader the user found in the spirit of Lucida, with an
//! open API and no account of ours. `GET /api/get-music?q=` searches
//! (Qobuz's own catalogue answer: `data.tracks.items`, `data.albums.items`),
//! `GET /api/get-album?album_id=` lists an album, and
//! `GET /api/download-music?track_id=&quality=27` gives a signed URL of
//! the file (27 = the best there is, up to 24/192; 6 = CD).
//!
//! Lucida itself sits behind a bot check and its Qobuz side answered 403
//! in October 2026, so it is not used.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use serde::Deserialize;

use crate::http::{client, download, get_json, safe_name};
use crate::instances::Instances;
use crate::text::catalogue_queries;

pub const DEFAULT_INSTANCES: &[&str] = &["https://qobuz.squid.wtf"];
/// The best format there is.
const QUALITY: &str = "27";

pub struct Qobuz {
    instances: Instances,
    http: reqwest::Client,
}

impl Qobuz {
    pub fn new(urls: &[String]) -> Self {
        Self::with_pace(urls, Duration::from_millis(1500))
    }

    pub fn with_pace(urls: &[String], every: Duration) -> Self {
        Self {
            instances: Instances::new(urls, DEFAULT_INSTANCES, every),
            http: client(),
        }
    }
}

#[derive(Deserialize)]
struct Answer<T> {
    #[serde(default)]
    success: bool,
    data: Option<T>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

impl<T> Answer<T> {
    fn data(self) -> Result<T, SourceError> {
        match self.data {
            Some(d) if self.success => Ok(d),
            _ => Err(SourceError::Parse(format!(
                "the instance said no: {}",
                self.error.map(|e| e.to_string()).unwrap_or_default()
            ))),
        }
    }
}

#[derive(Deserialize, Clone)]
struct Page<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
}

impl<T> Default for Page<T> {
    fn default() -> Self {
        Self { items: Vec::new() }
    }
}

#[derive(Deserialize, Default)]
struct Found {
    #[serde(default)]
    tracks: Page<Track>,
    #[serde(default)]
    albums: Page<Album>,
}

#[derive(Deserialize, Clone, Default)]
struct Named {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Clone)]
struct Track {
    id: serde_json::Value,
    title: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    track_number: Option<u32>,
    #[serde(default)]
    media_number: Option<u32>,
    #[serde(default)]
    maximum_bit_depth: Option<u8>,
    /// kHz (44.1, 96).
    #[serde(default)]
    maximum_sampling_rate: Option<f64>,
    #[serde(default)]
    performer: Option<Named>,
    #[serde(default)]
    album: Option<AlbumRef>,
    #[serde(default = "yes")]
    streamable: bool,
}

#[derive(Deserialize, Clone)]
struct AlbumRef {
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: Option<Named>,
}

#[derive(Deserialize, Clone)]
struct Album {
    id: serde_json::Value,
    title: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    artist: Option<Named>,
    #[serde(default)]
    maximum_bit_depth: Option<u8>,
    #[serde(default)]
    maximum_sampling_rate: Option<f64>,
    #[serde(default = "yes")]
    streamable: bool,
    #[serde(default)]
    tracks: Option<Page<Track>>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct Link {
    url: String,
}

fn id_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn format(bits: Option<u8>, khz: Option<f64>) -> Format {
    Format {
        bit_depth: Some(bits.unwrap_or(16)),
        sample_rate: Some(khz.map_or(44_100, |k| (k * 1000.0).round() as u32)),
        ..Format::new(Codec::Flac)
    }
}

fn with_version(title: &str, version: Option<&str>) -> String {
    match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => format!("{title} ({v})"),
        None => title.to_owned(),
    }
}

fn track_offer(t: &Track) -> Offer {
    let title = with_version(&t.title, t.version.as_deref());
    let artist = t
        .performer
        .as_ref()
        .map(|p| p.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| t.album.as_ref().and_then(|a| a.artist.as_ref()).map(|a| a.name.clone()));
    Offer {
        source: SourceKey::Qobuz,
        provider: "Qobuz".into(),
        id: id_text(&t.id),
        title: format!("{} — {title}", artist.as_deref().unwrap_or("?")),
        artist,
        track_title: Some(title),
        album: t.album.as_ref().map(|a| a.title.clone()),
        duration_ms: t.duration.map(|d| d * 1000),
        isrc: t.isrc.clone(),
        scope: OfferScope::Track,
        format: Some(format(t.maximum_bit_depth, t.maximum_sampling_rate)),
        size_bytes: None,
        seeders: None,
        info_url: None,
        handle: id_text(&t.id),
    }
}

fn album_offer(a: &Album) -> Offer {
    let title = with_version(&a.title, a.version.as_deref());
    let artist = a.artist.as_ref().map(|n| n.name.clone());
    Offer {
        source: SourceKey::Qobuz,
        provider: "Qobuz".into(),
        id: id_text(&a.id),
        title: format!("{} — {title}", artist.as_deref().unwrap_or("?")),
        artist,
        track_title: None,
        album: Some(title),
        duration_ms: None,
        isrc: None,
        scope: OfferScope::Album,
        format: Some(format(a.maximum_bit_depth, a.maximum_sampling_rate)),
        size_bytes: None,
        seeders: None,
        info_url: None,
        handle: format!("album:{}", id_text(&a.id)),
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl Qobuz {
    async fn link(&self, track: &str) -> Result<String, SourceError> {
        let http = &self.http;
        self.instances
            .call(|base| async move {
                let a: Answer<Link> =
                    get_json(http, &format!("{base}/api/download-music?track_id={}&quality={QUALITY}", enc(track))).await?;
                a.data().map(|l| l.url)
            })
            .await
    }

    async fn album_tracks(&self, album: &str) -> Result<Vec<Track>, SourceError> {
        let http = &self.http;
        self.instances
            .call(|base| async move {
                let a: Answer<Album> = get_json(http, &format!("{base}/api/get-album?album_id={}", enc(album))).await?;
                Ok(a.data()?.tracks.map(|p| p.items).unwrap_or_default())
            })
            .await
    }
}

impl Source for Qobuz {
    fn key(&self) -> SourceKey {
        SourceKey::Qobuz
    }

    fn name(&self) -> &str {
        "Qobuz"
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            let mut out: Vec<Offer> = Vec::new();
            let http = &self.http;
            for q in catalogue_queries(want) {
                let found: Found = self
                    .instances
                    .call(|base| {
                        let q = q.clone();
                        async move {
                            let a: Answer<Found> = get_json(http, &format!("{base}/api/get-music?q={}&offset=0", enc(&q))).await?;
                            a.data()
                        }
                    })
                    .await?;
                match want.scope {
                    WantScope::Track => out.extend(found.tracks.items.iter().filter(|t| t.streamable).map(track_offer)),
                    WantScope::Album => out.extend(found.albums.items.iter().filter(|a| a.streamable).map(album_offer)),
                }
                if !out.is_empty() {
                    break;
                }
            }
            out.dedup_by(|a, b| a.id == b.id);
            Ok(out)
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, _: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let tracks: Vec<(String, String)> = match offer.handle.strip_prefix("album:") {
                Some(album) => self
                    .album_tracks(album)
                    .await?
                    .iter()
                    .filter(|t| t.streamable)
                    .map(|t| {
                        let disc = t.media_number.unwrap_or(1);
                        let no = t.track_number.unwrap_or(0);
                        let name = with_version(&t.title, t.version.as_deref());
                        (id_text(&t.id), crate::http::track_file_name(disc as u32, no as u32, &name))
                    })
                    .collect(),
                None => vec![(offer.handle.clone(), safe_name(offer.track_title.as_deref().unwrap_or(&offer.title)))],
            };
            if tracks.is_empty() {
                return Err(SourceError::NothingToFetch);
            }
            let mut files = Vec::new();
            let mut before = 0u64;
            for (id, name) in tracks {
                let url = self.link(&id).await?;
                let base = before;
                let p = progress.clone();
                let step: Progress = Arc::new(move |done, _| p(base + done, None));
                let path = download(&self.http, &url, &dest.join(format!("{name}.flac")), &step).await?;
                before += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                files.push(path);
            }
            Ok(Fetched::Files(files))
        })
    }
}
