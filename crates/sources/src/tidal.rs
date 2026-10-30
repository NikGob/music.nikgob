//! Tidal through a public hifi-api instance (docs/stage4.5.md §4;
//! github.com/uimaxbai/hifi-api, the API behind Monochrome and its kin):
//! `GET /search/?s=` (tracks) and `?al=` (albums), `GET /album/?id=` lists
//! an album, `GET /track/?id=&quality=LOSSLESS` gives a manifest
//! (`application/vnd.tidal.bts`: base64 JSON with the file's `urls`). The
//! CD-quality tier is asked: hi-res comes as a DASH manifest of segments.
//!
//! Instances come and go (Tidal bans their accounts): several are asked in
//! turn, and the answers are read loosely (older instances wrap them in a
//! list).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use serde_json::Value;

use crate::http::{client, download, get_json, safe_name};
use crate::instances::Instances;
use crate::text::catalogue_queries;

pub const DEFAULT_INSTANCES: &[&str] = &[
    "https://api.monochrome.tf",
    "https://eu-central.monochrome.tf",
    "https://us-west.monochrome.tf",
    "https://monochrome-api.samidy.com",
];

pub struct Tidal {
    instances: Instances,
    http: reqwest::Client,
}

impl Tidal {
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

/// The first array under `key` anywhere in `v` (breadth first, so the
/// outer `items` wins over a nested one).
fn find_array<'a>(v: &'a Value, key: &str) -> Option<&'a Vec<Value>> {
    let mut queue = std::collections::VecDeque::from([v]);
    while let Some(v) = queue.pop_front() {
        match v {
            Value::Object(m) => {
                if let Some(Value::Array(a)) = m.get(key) {
                    return Some(a);
                }
                queue.extend(m.values());
            }
            Value::Array(a) => queue.extend(a.iter()),
            _ => {}
        }
    }
    None
}

/// The first object having `key` anywhere in `v`.
fn find_with<'a>(v: &'a Value, key: &str) -> Option<&'a serde_json::Map<String, Value>> {
    let mut queue = std::collections::VecDeque::from([v]);
    while let Some(v) = queue.pop_front() {
        match v {
            Value::Object(m) => {
                if m.contains_key(key) {
                    return Some(m);
                }
                queue.extend(m.values());
            }
            Value::Array(a) => queue.extend(a.iter()),
            _ => {}
        }
    }
    None
}

fn s(v: &Value, k: &str) -> Option<String> {
    match v.get(k)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn artist_of(v: &Value) -> Option<String> {
    v.get("artist")
        .and_then(|a| s(a, "name"))
        .or_else(|| v.get("artists").and_then(Value::as_array).and_then(|a| a.first()).and_then(|a| s(a, "name")))
}

fn title_of(v: &Value) -> Option<String> {
    let t = s(v, "title")?;
    Some(match s(v, "version").filter(|x| !x.trim().is_empty()) {
        Some(ver) => format!("{t} ({ver})"),
        None => t,
    })
}

/// What the LOSSLESS tier gives: CD FLAC (an item that is only HIGH, AAC).
fn format_of(v: &Value) -> Format {
    match s(v, "audioQuality").as_deref() {
        Some("HIGH" | "LOW") => Format { bitrate_kbps: Some(320), ..Format::new(Codec::Aac) },
        _ => Format { bit_depth: Some(16), sample_rate: Some(44_100), ..Format::new(Codec::Flac) },
    }
}

fn playable(v: &Value) -> bool {
    !matches!(v.get("allowStreaming"), Some(Value::Bool(false))) && !matches!(v.get("streamReady"), Some(Value::Bool(false)))
}

fn track_offer(v: &Value) -> Option<Offer> {
    let id = s(v, "id")?;
    let title = title_of(v)?;
    let artist = artist_of(v);
    Some(Offer {
        source: SourceKey::Tidal,
        provider: "Tidal".into(),
        id: id.clone(),
        title: format!("{} — {title}", artist.as_deref().unwrap_or("?")),
        artist,
        track_title: Some(title),
        album: v.get("album").and_then(|a| s(a, "title")),
        duration_ms: v.get("duration").and_then(Value::as_u64).map(|d| d as u32 * 1000),
        isrc: s(v, "isrc"),
        scope: OfferScope::Track,
        format: Some(format_of(v)),
        size_bytes: None,
        seeders: None,
        info_url: None,
        handle: id,
    })
}

fn album_offer(v: &Value) -> Option<Offer> {
    let id = s(v, "id")?;
    let title = title_of(v)?;
    let artist = artist_of(v);
    Some(Offer {
        source: SourceKey::Tidal,
        provider: "Tidal".into(),
        id: id.clone(),
        title: format!("{} — {title}", artist.as_deref().unwrap_or("?")),
        artist,
        track_title: None,
        album: Some(title),
        duration_ms: None,
        isrc: None,
        scope: OfferScope::Album,
        format: Some(format_of(v)),
        size_bytes: None,
        seeders: None,
        info_url: None,
        handle: format!("album:{id}"),
    })
}

/// The file's URL in a `/track/` answer: a BTS manifest's first URL, or
/// the `OriginalTrackUrl` older instances add. DRM and DASH are refused.
pub fn file_url(answer: &Value) -> Result<String, SourceError> {
    if let Some(m) = find_with(answer, "OriginalTrackUrl")
        && let Some(Value::String(u)) = m.get("OriginalTrackUrl")
    {
        return Ok(u.clone());
    }
    let m = find_with(answer, "manifest").ok_or_else(|| SourceError::Parse("no manifest".into()))?;
    let mime = m.get("manifestMimeType").and_then(Value::as_str).unwrap_or("application/vnd.tidal.bts");
    if mime != "application/vnd.tidal.bts" {
        return Err(SourceError::Parse(format!("a {mime} manifest")));
    }
    let raw = m.get("manifest").and_then(Value::as_str).unwrap_or_default();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| SourceError::Parse(format!("manifest: {e}")))?;
    let bts: Value = serde_json::from_slice(&bytes).map_err(|e| SourceError::Parse(format!("manifest: {e}")))?;
    if bts.get("encryptionType").and_then(Value::as_str).is_some_and(|e| e != "NONE") {
        return Err(SourceError::Parse("an encrypted stream".into()));
    }
    bts.get("urls")
        .and_then(Value::as_array)
        .and_then(|u| u.first())
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| SourceError::Parse("a manifest without URLs".into()))
}

/// The extension of the manifest's codec.
fn ext_of(answer: &Value) -> &'static str {
    let mime = find_with(answer, "manifest")
        .and_then(|m| m.get("manifest"))
        .and_then(Value::as_str)
        .and_then(|raw| base64::engine::general_purpose::STANDARD.decode(raw.trim()).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| s(&v, "mimeType"));
    match mime.as_deref() {
        Some(m) if m.contains("mp4") || m.contains("aac") => "m4a",
        _ => "flac",
    }
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl Tidal {
    async fn ask(&self, path: String) -> Result<Value, SourceError> {
        let http = &self.http;
        self.instances
            .call(|base| {
                let path = path.clone();
                async move { get_json::<Value>(http, &format!("{base}{path}")).await }
            })
            .await
    }
}

impl Source for Tidal {
    fn key(&self) -> SourceKey {
        SourceKey::Tidal
    }

    fn name(&self) -> &str {
        "Tidal"
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            let mut out: Vec<Offer> = Vec::new();
            for q in catalogue_queries(want) {
                let (param, kind) = match want.scope {
                    WantScope::Track => ("s", None),
                    WantScope::Album => ("al", Some("albums")),
                };
                let v = self.ask(format!("/search/?{param}={}", enc(&q))).await?;
                // Albums sit under `albums.items`; tracks are the items.
                let scope = kind.and_then(|k| v.get("data").unwrap_or(&v).get(k)).unwrap_or(&v);
                let items = find_array(scope, "items").cloned().unwrap_or_default();
                out.extend(items.iter().filter(|i| playable(i)).filter_map(|i| match want.scope {
                    WantScope::Track => track_offer(i),
                    WantScope::Album => album_offer(i),
                }));
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
                Some(album) => {
                    let v = self.ask(format!("/album/?id={}", enc(album))).await?;
                    find_array(&v, "items")
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|i| i.get("item").unwrap_or(i))
                        .filter(|t| playable(t))
                        .filter_map(|t| {
                            let id = s(t, "id")?;
                            let disc = t.get("volumeNumber").and_then(Value::as_u64).unwrap_or(1);
                            let no = t.get("trackNumber").and_then(Value::as_u64).unwrap_or(0);
                            Some((id, format!("{disc}-{no:02} - {}", safe_name(&title_of(t)?))))
                        })
                        .collect()
                }
                None => vec![(offer.handle.clone(), safe_name(offer.track_title.as_deref().unwrap_or(&offer.title)))],
            };
            if tracks.is_empty() {
                return Err(SourceError::NothingToFetch);
            }
            let mut files = Vec::new();
            let mut before = 0u64;
            for (id, name) in tracks {
                let answer = self.ask(format!("/track/?id={}&quality=LOSSLESS", enc(&id))).await?;
                let url = file_url(&answer)?;
                let base = before;
                let p = progress.clone();
                let step: Progress = Arc::new(move |done, _| p(base + done, None));
                let path = download(&self.http, &url, &dest.join(format!("{name}.{}", ext_of(&answer))), &step).await?;
                before += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                files.push(path);
            }
            Ok(Fetched::Files(files))
        })
    }
}
