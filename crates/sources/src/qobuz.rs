//! Qobuz through a public Qobuz-DL instance: the
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use serde::Deserialize;

use crate::album::{Patience, fetch_each, left_out};
use crate::http::{Reply, client, download, is_bot_check, is_daily_limit, read, safe_name, until_utc_midnight};
use crate::instances::Instances;
use crate::text::catalogue_queries;

/// The one that searched and gave a file on 2026-10-31. It is a Cloudflare
/// Worker on the free plan: once the day's requests of everyone using it
/// are spent it answers 429 (error 1027) until midnight UTC, and that wait
/// is kept ([`crate::http::is_daily_limit`]), on that instance alone. On
/// 2026-11-10 the same host answered 400 with the upstream's 401: its
/// token is dead, and that instance sits out for [`TOKEN_PAUSE`] rather
/// than being asked on every wish. `qobuz.squid.wtf` is gone (no DNS since
/// 2026-10-31). Other instances go in `sources.toml`.
pub const DEFAULT_INSTANCES: &[&str] = &["https://qobuz-dll.foreyou.workers.dev"];
/// The best format there is.
const QUALITY: &str = "27";
/// A dead instance token (400 whose body says 401 or token) sits out this
/// long, then it is asked again.
const TOKEN_PAUSE: Duration = Duration::from_secs(6 * 3600);
/// How often each instance is probed before a wish uses it.
const HEALTH_EVERY: Duration = Duration::from_secs(30 * 60);

pub struct Qobuz {
    instances: Instances,
    http: reqwest::Client,
    /// When the instances were last probed.
    checked: tokio::sync::Mutex<Option<Instant>>,
}

impl Qobuz {
    pub fn new(urls: &[String]) -> Self {
        Self::with_pace(urls, Duration::from_millis(1500))
    }

    pub fn with_pace(urls: &[String], every: Duration) -> Self {
        Self::from_list(urls, urls.iter().any(|u| !u.trim().is_empty()), every)
    }

    /// `own`: the list is the user's (`sources.toml`), not one an uptime
    /// list may replace. Qobuz has no uptime list; the flag is kept so a
    /// reload can say which.
    pub fn from_list(urls: &[String], own: bool, every: Duration) -> Self {
        Self {
            instances: Instances::with_own(urls, DEFAULT_INSTANCES, own, every),
            http: client(),
            checked: tokio::sync::Mutex::new(None),
        }
    }

    /// The list `sources.toml` names now. An instance that stays keeps its pause.
    pub fn set_instances(&self, urls: &[String], own: bool) {
        self.instances.apply(urls, DEFAULT_INSTANCES, own);
    }

    /// The instances asked, and whether the list is the user's.
    pub fn instances_now(&self) -> (Vec<String>, bool) {
        (self.instances.urls(), self.instances.is_own())
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
        Some(v) if !title.to_lowercase().contains(&v.to_lowercase()) => format!("{title} ({v})"),
        Some(_) => title.to_owned(),
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

/// A 400 whose body is the upstream refusing the instance's token.
fn dead_token(body: &str) -> bool {
    let b = body.to_ascii_lowercase();
    b.contains("401") || b.contains("token")
}

/// Qobuz-DL's answer, with a dead token and a daily quota told as a wait
/// of that instance (the body of a 400 is not thrown away).
pub fn classify(reply: &Reply) -> Result<(), SourceError> {
    let status = reqwest::StatusCode::from_u16(reply.status).unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if status.is_success() {
        return Ok(());
    }
    if is_daily_limit(status, &reply.body) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        return Err(SourceError::RateLimited { retry_after: Some(until_utc_midnight(now)) });
    }
    if reply.status == 400 && dead_token(&reply.body) {
        return Err(SourceError::RateLimited { retry_after: Some(TOKEN_PAUSE) });
    }
    if is_bot_check(status, &reply.body) {
        return Err(SourceError::Blocked(crate::http::BOT_CHECK.into()));
    }
    Err(match status {
        reqwest::StatusCode::TOO_MANY_REQUESTS => SourceError::RateLimited { retry_after: reply.retry_after },
        s => SourceError::Http { status: s.as_u16() },
    })
}

async fn qobuz_json<T: serde::de::DeserializeOwned>(http: &reqwest::Client, url: &str) -> Result<T, SourceError> {
    let reply = read(http, url).await?;
    classify(&reply)?;
    serde_json::from_str(&reply.body).map_err(|e| SourceError::Parse(e.to_string()))
}

impl Qobuz {
    /// Probes each instance that is not already sitting out, at most every
    /// [`HEALTH_EVERY`], so a dead one is skipped before a wish asks it.
    /// A pause is that instance's alone.
    async fn health(&self) {
        {
            let mut checked = self.checked.lock().await;
            if checked.is_some_and(|t| t.elapsed() < HEALTH_EVERY) {
                return;
            }
            *checked = Some(Instant::now());
        }
        for base in self.instances.urls() {
            if self.instances.paused(&base) {
                continue;
            }
            self.instances.wait().await;
            let r = probe(&self.http, &base).await;
            if let Err(e) = &r {
                log::info!("qobuz: {base} is not healthy: {e}");
            }
            self.instances.note_at(&base, &r);
        }
    }

    async fn link(&self, track: &str) -> Result<String, SourceError> {
        let http = &self.http;
        self.instances
            .call(|base| async move {
                let a: Answer<Link> = qobuz_json(http, &format!("{base}/api/download-music?track_id={}&quality={QUALITY}", enc(track))).await?;
                a.data().map(|l| l.url)
            })
            .await
    }

    async fn album_tracks(&self, album: &str) -> Result<Vec<Track>, SourceError> {
        let http = &self.http;
        self.instances
            .call(|base| async move {
                let a: Answer<Album> = qobuz_json(http, &format!("{base}/api/get-album?album_id={}", enc(album))).await?;
                Ok(a.data()?.tracks.map(|p| p.items).unwrap_or_default())
            })
            .await
    }
}

async fn probe(http: &reqwest::Client, base: &str) -> Result<(), SourceError> {
    let reply = read(http, &format!("{base}/api/get-music?q=a&offset=0")).await?;
    classify(&reply)?;
    let v: serde_json::Value = serde_json::from_str(&reply.body).map_err(|e| SourceError::Parse(e.to_string()))?;
    if v.get("success") == Some(&serde_json::Value::Bool(true)) {
        Ok(())
    } else {
        Err(SourceError::Parse("the probe was not a catalogue".into()))
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
            self.health().await;
            let mut out: Vec<Offer> = Vec::new();
            let http = &self.http;
            for q in catalogue_queries(want) {
                let found: Found = self
                    .instances
                    .call(|base| {
                        let q = q.clone();
                        async move {
                            let a: Answer<Found> = qobuz_json(http, &format!("{base}/api/get-music?q={}&offset=0", enc(&q))).await?;
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

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            self.health().await;
            let tracks: Vec<(String, String)> = match offer.handle.strip_prefix("album:") {
                Some(album) => {
                    let all: Vec<(String, String, String)> = self
                        .album_tracks(album)
                        .await?
                        .iter()
                        .filter(|t| t.streamable)
                        .map(|t| {
                            let disc = t.media_number.unwrap_or(1);
                            let no = t.track_number.unwrap_or(0);
                            let name = with_version(&t.title, t.version.as_deref());
                            (id_text(&t.id), crate::http::track_file_name(disc, no, &name), name)
                        })
                        .collect();
                    left_out(all, &want.got)
                }
                None => vec![(offer.handle.clone(), safe_name(offer.track_title.as_deref().unwrap_or(&offer.title)))],
            };
            let before = Arc::new(AtomicU64::new(0));
            fetch_each(tracks, Patience::default(), |id, name| {
                let (before, progress) = (before.clone(), progress.clone());
                async move {
                    let url = self.link(&id).await?;
                    let base = before.load(Ordering::Relaxed);
                    let step: Progress = Arc::new(move |done, _| progress(base + done, None));
                    let path = download(&self.http, &url, &dest.join(format!("{name}.flac")), &step).await?;
                    before.fetch_add(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
                    Ok(path)
                }
            })
            .await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::Reply;

    #[test]
    fn a_dead_token_waits_six_hours_and_a_daily_quota_waits_until_midnight() {
        let dead = classify(&Reply {
            status: 400,
            body: r#"{"success":false,"error":"Request failed with status code 401"}"#.into(),
            retry_after: None,
        })
        .unwrap_err();
        assert_eq!(dead, SourceError::RateLimited { retry_after: Some(TOKEN_PAUSE) });
        let plain = classify(&Reply { status: 400, body: r#"{"success":false,"error":"bad request"}"#.into(), retry_after: None }).unwrap_err();
        assert_eq!(plain, SourceError::Http { status: 400 }, "a 400 that is not a dead token stays a 400");
        let quota = classify(&Reply {
            status: 429,
            body: "Error 1027: Worker exceeded daily quota".into(),
            retry_after: None,
        })
        .unwrap_err();
        let SourceError::RateLimited { retry_after: Some(left) } = quota else { panic!("{quota:?}") };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(crate::http::ends_at_utc_midnight(now, left), "a daily quota waits until midnight UTC");
    }
}
