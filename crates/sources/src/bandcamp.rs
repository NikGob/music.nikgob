//! Bandcamp as a [`Source`]: only what the artist gives away (free or
//! name-your-price from 0), in FLAC. The site has no API for this; its
//! own search box asks a public JSON endpoint ([`DEFAULT_API`]), and a
//! release page carries its data in `data-tralbum`: a free release has a
//! `freeDownloadPage` there. That page's `data-blob` links each format;
//! the FLAC link is asked through `statdownload` until the file is ready,
//! and the file is a zip of the album (or the track's own file).
//!
//! A paid release is not a candidate, and neither is a free one that wants
//! an email address first (it has no `freeDownloadPage`). A track sold on
//! its own but free with its album comes from the album's zip.
//!
//! The search page itself sits behind a script check; the endpoint and the
//! release pages do not. One request at a time with a pause, and a pause
//! after a check turned us away ([`Breaker`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use nikgob_metadata::text::{similarity, title_key};
use serde::{Deserialize, Serialize};

use crate::http::{Breaker, PAGE_TIMEOUT, Pace, client, download, get_page, is_bot_check, safe_name};
use crate::matching::same_artist;
use crate::text::{album_keys, artist_keys, catalogue_queries, franchise_of, is_soundtrack, title_keys};

/// The search box's endpoint.
pub const DEFAULT_API: &str = "https://bandcamp.com/api/bcsearch_public_api/1/autocomplete_elastic";
/// Releases of a search looked into.
const RELEASES: usize = 3;
/// Names this alike are the wished track or album.
const TITLE: f64 = 0.85;
/// Asks of `statdownload` before giving up on a file being made.
const STAT_TRIES: u32 = 12;
/// Audio a release's zip may hold.
const AUDIO: &[&str] = &["flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aif", "aiff"];

pub struct Bandcamp {
    api: String,
    http: reqwest::Client,
    pace: Pace,
    breaker: Breaker,
    /// Between asks of a file still being made.
    poll: Duration,
}

impl Default for Bandcamp {
    fn default() -> Self {
        Self::new()
    }
}

impl Bandcamp {
    pub fn new() -> Self {
        Self::with(DEFAULT_API, Duration::from_millis(1500), Duration::from_secs(3))
    }

    /// Another endpoint and pace (tests).
    pub fn with(api: &str, every: Duration, poll: Duration) -> Self {
        Self {
            api: api.to_owned(),
            http: client(),
            pace: Pace::new(every),
            breaker: Breaker::default(),
            poll,
        }
    }

    async fn page(&self, url: &str) -> Result<String, SourceError> {
        self.breaker.check()?;
        self.pace.wait().await;
        let r = get_page(&self.http, url, None).await.and_then(|body| {
            if is_challenge(&body) { Err(SourceError::Blocked("a script check".into())) } else { Ok(body) }
        });
        self.breaker.note(&r);
        r
    }

    /// What the search box finds for `text` of a kind (`a` albums, `t`
    /// tracks).
    async fn find(&self, text: &str, kind: &str) -> Result<Vec<Hit>, SourceError> {
        self.breaker.check()?;
        self.pace.wait().await;
        let r = self.ask(text, kind).await;
        self.breaker.note(&r);
        r
    }

    async fn ask(&self, text: &str, kind: &str) -> Result<Vec<Hit>, SourceError> {
        let body = serde_json::json!({ "search_text": text, "search_filter": kind, "full_page": false, "fan_id": null });
        let resp = self
            .http
            .post(&self.api)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&body)
            .timeout(PAGE_TIMEOUT)
            .send()
            .await
            .map_err(|e| SourceError::Unreachable(e.without_url().to_string()))?;
        let status = resp.status();
        let wait = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let text = resp.text().await.map_err(|e| SourceError::Unreachable(e.without_url().to_string()))?;
        if is_bot_check(status, &text) || is_challenge(&text) {
            return Err(SourceError::Blocked("a script check".into()));
        }
        match status.as_u16() {
            200..=299 => search_hits(&text),
            429 => Err(SourceError::RateLimited { retry_after: wait }),
            s => Err(SourceError::Http { status: s }),
        }
    }

    /// The URL of the file behind a format's link, once Bandcamp has made
    /// it: `statdownload` says `ok` with it, or hands a fresh link for an
    /// old one, or (while it zips the album) nothing yet.
    async fn ready(&self, link: &str) -> Result<String, SourceError> {
        let mut link = link.to_owned();
        for _ in 0..STAT_TRIES {
            let Some(url) = stat_url(&link, rand()) else { return Err(SourceError::Parse("a download link of another shape".into())) };
            let body = self.page(&url).await?;
            let Some(stat) = stat_result(&body) else { return Err(SourceError::Parse("an unreadable download status".into())) };
            match (stat.result.as_str(), stat.download_url, stat.retry_url) {
                ("ok", Some(file), _) => return Ok(file),
                (_, _, Some(fresh)) if fresh != link => link = fresh,
                _ => tokio::time::sleep(self.poll).await,
            }
        }
        Err(SourceError::Unreachable("the download was not made ready".into()))
    }
}

/// The page of a script check instead of the answer (served with 200).
fn is_challenge(body: &str) -> bool {
    body.contains("<title>Client Challenge</title>") || body.contains("_fs-ch-")
}

/// A number for the `.rand` of a status ask.
fn rand() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() % 1_000_000_000)
}

/// One hit of the search box.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Hit {
    /// `a` an album, `t` a track, `b` an artist.
    #[serde(rename = "type", deserialize_with = "nullable")]
    pub kind: String,
    #[serde(deserialize_with = "nullable")]
    pub name: String,
    #[serde(deserialize_with = "nullable")]
    pub band_name: String,
    /// A track's album.
    pub album_name: Option<String>,
    /// The release's page.
    #[serde(deserialize_with = "nullable")]
    pub item_url_path: String,
}

/// A value that may come as `null`: its default then.
fn nullable<'de, D: serde::Deserializer<'de>, T: Default + Deserialize<'de>>(d: D) -> Result<T, D::Error> {
    Option::<T>::deserialize(d).map(Option::unwrap_or_default)
}

#[derive(Deserialize)]
struct Auto {
    auto: AutoResults,
}

#[derive(Deserialize)]
struct AutoResults {
    #[serde(default)]
    results: Vec<Hit>,
}

/// The hits of a search box answer, as listed.
pub fn search_hits(json: &str) -> Result<Vec<Hit>, SourceError> {
    let a: Auto = serde_json::from_str(json).map_err(|e| SourceError::Parse(e.to_string()))?;
    Ok(a.auto.results.into_iter().filter(|h| !h.item_url_path.is_empty()).collect())
}

/// A release page's data (`data-tralbum`), what of it is used.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Tralbum {
    /// The page a free release (without an email asked) is downloaded from.
    #[serde(rename = "freeDownloadPage")]
    pub free_download_page: Option<String>,
    /// `album` or `track`.
    #[serde(deserialize_with = "nullable")]
    pub item_type: String,
    pub artist: Option<String>,
    pub current: Current,
    #[serde(deserialize_with = "nullable")]
    pub trackinfo: Vec<TrackInfo>,
    /// A track's album, as a path on the artist's site.
    pub album_url: Option<String>,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Current {
    pub title: Option<String>,
    pub minimum_price: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct TrackInfo {
    pub title: Option<String>,
    pub track_num: Option<u32>,
    /// Seconds.
    pub duration: Option<f64>,
}

impl Tralbum {
    /// The free download page: `None` for a paid release, or a free one
    /// that asks for an email address.
    pub fn free_page(&self) -> Option<&str> {
        self.free_download_page.as_deref().map(str::trim).filter(|p| !p.is_empty())
    }

    /// The tracks as (number, title).
    pub fn tracks(&self) -> Vec<(u32, String)> {
        self.trackinfo
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.title.clone().map(|title| (t.track_num.unwrap_or(i as u32 + 1), title)))
            .collect()
    }
}

/// The JSON in a `data-<name>="…"` attribute, entities decoded.
fn data_attr<T: serde::de::DeserializeOwned>(html: &str, name: &str) -> Option<T> {
    let marker = format!("data-{name}=\"");
    let at = html.find(&marker)? + marker.len();
    let end = html[at..].find('"')? + at;
    serde_json::from_str(&crate::html::decode(&html[at..end])).ok()
}

/// A release page's data; `None` when the page has none.
pub fn tralbum(html: &str) -> Option<Tralbum> {
    data_attr(html, "tralbum")
}

#[derive(Deserialize)]
struct Blob {
    #[serde(default)]
    download_items: Vec<DownloadItem>,
}

#[derive(Deserialize)]
struct DownloadItem {
    #[serde(default)]
    downloads: HashMap<String, Encoding>,
}

#[derive(Deserialize)]
struct Encoding {
    url: String,
    #[serde(default)]
    size_mb: Option<String>,
}

/// The FLAC link of a download page and the file's size when it says.
pub fn flac_link(html: &str) -> Option<(String, Option<u64>)> {
    let blob: Blob = data_attr(html, "blob")?;
    blob.download_items.into_iter().find_map(|i| {
        i.downloads.get("flac").map(|e| (e.url.clone(), e.size_mb.as_deref().and_then(megabytes)))
    })
}

/// "418.6MB", "980KB", "1.2GB".
fn megabytes(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_uppercase();
    let (n, mult) = if let Some(n) = s.strip_suffix("GB") {
        (n, 1024.0 * 1024.0 * 1024.0)
    } else if let Some(n) = s.strip_suffix("MB") {
        (n, 1024.0 * 1024.0)
    } else if let Some(n) = s.strip_suffix("KB") {
        (n, 1024.0)
    } else {
        return None;
    };
    let n: f64 = n.trim().parse().ok()?;
    Some((n * mult) as u64)
}

/// The status ask of a format's link: `/download/` → `/statdownload/`.
pub fn stat_url(link: &str, rand: u128) -> Option<String> {
    let at = link.find("/download/")?;
    let sep = if link.contains('?') { '&' } else { '?' };
    Some(format!("{}/statdownload/{}{sep}.vrs=1&.rand={rand}", &link[..at], &link[at + "/download/".len()..]))
}

/// What `statdownload` said.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Stat {
    #[serde(deserialize_with = "nullable")]
    pub result: String,
    pub download_url: Option<String>,
    pub retry_url: Option<String>,
}

/// The status in a `statdownload` answer (JSON, or JSON in a script call).
pub fn stat_result(body: &str) -> Option<Stat> {
    let (a, b) = (body.find('{')?, body.rfind('}')?);
    (a < b).then(|| serde_json::from_str(&body[a..=b]).ok()).flatten()
}

/// `href` on the site of `page`.
fn join(page: &str, href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        return href.to_owned();
    }
    match url::Url::parse(page).and_then(|u| u.join(href)) {
        Ok(u) => u.to_string(),
        Err(_) => href.to_owned(),
    }
}

/// The track number and title in the name of a file of an album's zip
/// ("Artist - Album - 03 Title.flac"): of the « - NN » in it, the one
/// whose title is most like that track's on the album.
pub fn entry_track(name: &str, tracks: &[(u32, String)]) -> Option<(u32, String)> {
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    let mut best: Option<(f64, u32, String)> = None;
    for (at, _) in stem.match_indices(" - ") {
        let rest = &stem[at + 3..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 || digits > 3 {
            continue;
        }
        let Some(title) = rest[digits..].strip_prefix(' ') else { continue };
        let Ok(no) = rest[..digits].parse::<u32>() else { continue };
        let fit = tracks
            .iter()
            .find(|(n, _)| *n == no)
            .map_or(0.0, |(_, t)| similarity(&title_key(t).base, &title_key(title).base));
        if best.as_ref().is_none_or(|(f, ..)| fit > *f) {
            best = Some((fit, no, title.trim().to_owned()));
        }
    }
    best.map(|(_, n, t)| (n, t))
}

fn is_audio(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, e)| AUDIO.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

/// A file name Windows takes that keeps its extension.
fn file_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() && ext.len() <= 5 => format!("{}.{}", safe_name(stem), safe_name(ext)),
        _ => safe_name(name),
    }
}

/// Writes the audio files of the zip at `zip` that `keep` takes (by their
/// name) into `dest`, flat.
pub fn unzip_audio(zip: &Path, dest: &Path, keep: impl Fn(&str) -> bool) -> Result<Vec<PathBuf>, SourceError> {
    let io = |e: std::io::Error| SourceError::Io(e.to_string());
    let file = std::fs::File::open(zip).map_err(io)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| SourceError::Parse(format!("a broken zip: {e}")))?;
    let mut out: Vec<PathBuf> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| SourceError::Parse(format!("a broken zip: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        // Only the file's own name: nothing lands outside `dest`.
        let Some(name) = entry.enclosed_name().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())) else { continue };
        if !is_audio(&name) || !keep(&name) {
            continue;
        }
        let path = dest.join(file_name(&name));
        if out.contains(&path) {
            continue;
        }
        let written = std::fs::File::create(&path).and_then(|mut f| std::io::copy(&mut entry, &mut f));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&path);
            return Err(io(e));
        }
        out.push(path);
    }
    Ok(out)
}

/// What a fetch needs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Handle {
    /// The free download page.
    page: String,
    /// The release's own page (a fresh download page when that one is old).
    release: String,
    /// The track taken out of the album's zip (number, title), for a track
    /// that is free only with its album.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pick: Option<(u32, String)>,
    /// The album's tracks (number, title): which file of the zip is which.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tracks: Vec<(u32, String)>,
    /// A lone track's file name, without the extension.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
}

/// The wish is a soundtrack (its artist may be a label's or a game's name).
fn soundtrack(want: &Want) -> bool {
    want.album.as_deref().is_some_and(is_soundtrack) || want.title.as_deref().and_then(franchise_of).is_some()
}

/// A hit by the wished artist, or one that may be credited otherwise.
fn artist_fits(hit: &Hit, want: &Want) -> bool {
    same_artist(&hit.band_name, want) || soundtrack(want) || artist_keys(want).is_empty()
}

fn best(got: &[String], wanted: &[String]) -> f64 {
    got.iter().flat_map(|g| wanted.iter().map(move |w| similarity(g, w))).fold(0.0, f64::max)
}

/// The album hits worth a look for an album wish.
pub fn album_hits(hits: Vec<Hit>, want: &Want) -> Vec<Hit> {
    let Some(wanted) = want.album.as_deref().or(want.title.as_deref()).map(album_keys) else { return Vec::new() };
    hits.into_iter()
        .filter(|h| h.kind == "a" && artist_fits(h, want) && best(&album_keys(&h.name), &wanted) >= TITLE)
        .take(RELEASES)
        .collect()
}

/// The track hits worth a look for a track wish.
pub fn track_hits(hits: Vec<Hit>, want: &Want) -> Vec<Hit> {
    let Some(wanted) = want.title.as_deref().map(title_keys) else { return Vec::new() };
    hits.into_iter()
        .filter(|h| h.kind == "t" && artist_fits(h, want) && best(&title_keys(&h.name), &wanted) >= TITLE)
        .take(RELEASES)
        .collect()
}

/// The track of `tracks` that is the wished one: alike by title, or at the
/// wished number and not unlike.
fn wished_track<'a>(info: &'a [TrackInfo], want: &Want) -> Option<(u32, &'a TrackInfo)> {
    let wanted = title_keys(want.title.as_deref()?);
    info.iter()
        .enumerate()
        .map(|(i, t)| {
            let no = t.track_num.unwrap_or(i as u32 + 1);
            let sim = t.title.as_deref().map_or(0.0, |s| best(&title_keys(s), &wanted));
            (no, t, sim)
        })
        .filter(|(no, _, sim)| *sim >= TITLE || (want.track_no > 0 && *no == want.track_no && *sim >= 0.5))
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(no, t, _)| (no, t))
}

fn flac() -> Option<Format> {
    Some(Format::new(Codec::Flac))
}

fn seconds(d: Option<f64>) -> Option<u32> {
    d.filter(|s| *s > 0.0).map(|s| (s * 1000.0).round() as u32)
}

/// An album offer of a free release page.
fn album_offer(release: &str, t: &Tralbum, hit: &Hit) -> Option<Offer> {
    let page = join(release, t.free_page()?);
    let title = t.current.title.clone().unwrap_or_else(|| hit.name.clone());
    let artist = t.artist.clone().unwrap_or_else(|| hit.band_name.clone());
    let handle = Handle { page, release: release.to_owned(), tracks: t.tracks(), ..Handle::default() };
    Some(Offer {
        source: SourceKey::Bandcamp,
        provider: "Bandcamp".into(),
        id: release.to_owned(),
        title: format!("{artist} — {title}"),
        artist: Some(artist),
        track_title: None,
        album: Some(title),
        duration_ms: None,
        isrc: None,
        scope: OfferScope::Album,
        format: flac(),
        size_bytes: None,
        seeders: None,
        info_url: Some(release.to_owned()),
        handle: serde_json::to_string(&handle).unwrap_or_default(),
    })
}

/// A track offer: the track's own free page, or (`album`: its album's page
/// and data) the album's, the track picked out of its zip.
fn track_offer(track_page: &str, t: &Tralbum, hit: &Hit, album: Option<(&str, &Tralbum)>, want: &Want) -> Option<Offer> {
    let artist = t.artist.clone().unwrap_or_else(|| hit.band_name.clone());
    let (handle, title, duration, album_title) = match album {
        None => {
            let title = t.current.title.clone().unwrap_or_else(|| hit.name.clone());
            let handle = Handle {
                page: join(track_page, t.free_page()?),
                release: track_page.to_owned(),
                name: format!("{artist} - {title}"),
                ..Handle::default()
            };
            (handle, title, t.trackinfo.first().and_then(|i| seconds(i.duration)), hit.album_name.clone())
        }
        Some((album_page, a)) => {
            let (no, info) = wished_track(&a.trackinfo, want)?;
            let title = info.title.clone().unwrap_or_else(|| hit.name.clone());
            let handle = Handle {
                page: join(album_page, a.free_page()?),
                release: album_page.to_owned(),
                pick: Some((no, title.clone())),
                tracks: a.tracks(),
                name: String::new(),
            };
            (handle, title, seconds(info.duration), a.current.title.clone().or_else(|| hit.album_name.clone()))
        }
    };
    Some(Offer {
        source: SourceKey::Bandcamp,
        provider: "Bandcamp".into(),
        id: track_page.to_owned(),
        title: format!("{artist} — {title}"),
        artist: Some(artist),
        track_title: Some(title),
        album: album_title,
        duration_ms: duration,
        isrc: None,
        scope: OfferScope::Track,
        format: flac(),
        size_bytes: None,
        seeders: None,
        info_url: Some(track_page.to_owned()),
        handle: serde_json::to_string(&handle).unwrap_or_default(),
    })
}

impl Bandcamp {
    async fn search_albums(&self, want: &Want) -> Result<Vec<Offer>, SourceError> {
        let Some(q) = catalogue_queries(want).into_iter().next() else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for hit in album_hits(self.find(&q, "a").await?, want) {
            let Some(t) = self.release(&hit.item_url_path).await? else { continue };
            // A sampler of the same name is not the album.
            if want.album_tracks > 0 && (t.trackinfo.len() as u32) * 2 < want.album_tracks {
                continue;
            }
            out.extend(album_offer(&hit.item_url_path, &t, &hit));
        }
        Ok(out)
    }

    async fn search_tracks(&self, want: &Want) -> Result<Vec<Offer>, SourceError> {
        let mut hits = Vec::new();
        for q in catalogue_queries(want) {
            hits = track_hits(self.find(&q, "t").await?, want);
            if !hits.is_empty() {
                break;
            }
        }
        let mut out = Vec::new();
        let mut albums: Vec<String> = Vec::new();
        for hit in hits {
            let page = hit.item_url_path.clone();
            let Some(t) = self.release(&page).await? else { continue };
            if t.free_page().is_some() {
                out.extend(track_offer(&page, &t, &hit, None, want));
                continue;
            }
            // Sold on its own: free with its album, maybe.
            let Some(album) = t.album_url.as_deref().map(|a| join(&page, a)) else { continue };
            if albums.contains(&album) {
                continue;
            }
            albums.push(album.clone());
            let Some(a) = self.release(&album).await? else { continue };
            out.extend(track_offer(&page, &t, &hit, Some((&album, &a)), want));
        }
        Ok(out)
    }

    /// A release page's data; `None` when the page is gone or has none
    /// (the next candidate), an error when the site turned us away.
    async fn release(&self, url: &str) -> Result<Option<Tralbum>, SourceError> {
        match self.page(url).await {
            Ok(html) => Ok(tralbum(&html)),
            Err(e) if e.is_refusal() => Err(e),
            Err(e) => {
                log::info!("bandcamp: {url}: {e}");
                Ok(None)
            }
        }
    }

    /// The FLAC link of an offer's download page; an old page is asked
    /// for again through the release's own page.
    async fn link(&self, h: &Handle) -> Result<(String, Option<u64>), SourceError> {
        if let Some(l) = flac_link(&self.page(&h.page).await?) {
            return Ok(l);
        }
        let t = tralbum(&self.page(&h.release).await?).ok_or_else(|| SourceError::Parse("a release page without its data".into()))?;
        let fresh = join(&h.release, t.free_page().ok_or(SourceError::NothingToFetch)?);
        flac_link(&self.page(&fresh).await?).ok_or_else(|| SourceError::Parse("a download page without FLAC".into()))
    }
}

impl Source for Bandcamp {
    fn key(&self) -> SourceKey {
        SourceKey::Bandcamp
    }

    fn name(&self) -> &str {
        "Bandcamp"
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            match want.scope {
                WantScope::Album => self.search_albums(want).await,
                WantScope::Track => self.search_tracks(want).await,
            }
        })
    }

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let h: Handle = serde_json::from_str(&offer.handle).map_err(|e| SourceError::Parse(e.to_string()))?;
            let (link, size) = self.link(&h).await?;
            let file = self.ready(&link).await?;
            self.pace.wait().await;
            let step: Progress = Arc::new(move |done, total| progress(done, total.or(size)));
            let got = download(&self.http, &file, &dest.join("bandcamp.download"), &step).await?;
            let mut head = [0u8; 4];
            {
                use std::io::Read;
                let n = std::fs::File::open(&got).and_then(|mut f| f.read(&mut head)).map_err(|e| SourceError::Io(e.to_string()))?;
                if n < 4 {
                    let _ = std::fs::remove_file(&got);
                    return Err(SourceError::Parse("an empty file".into()));
                }
            }
            if &head == b"fLaC" {
                // A track's own file.
                let name = if h.name.is_empty() { offer.track_title.clone().unwrap_or_else(|| offer.title.clone()) } else { h.name.clone() };
                let path = dest.join(file_name(&format!("{name}.flac")));
                std::fs::rename(&got, &path).map_err(|e| SourceError::Io(e.to_string()))?;
                return Ok(Fetched::Files(vec![path]));
            }
            if &head != b"PK\x03\x04" {
                let _ = std::fs::remove_file(&got);
                return Err(SourceError::Parse("neither a FLAC nor a zip".into()));
            }
            let (zip, into) = (got.clone(), dest.to_owned());
            let have: Vec<String> = want.got.iter().flat_map(|g| title_keys(g)).collect();
            let (pick, tracks) = (h.pick.clone(), h.tracks.clone());
            let unzipped = tokio::task::spawn_blocking(move || {
                unzip_audio(&zip, &into, |name| {
                    let track = entry_track(name, &tracks);
                    match &pick {
                        Some((no, title)) => track.is_some_and(|(n, t)| {
                            let sim = similarity(&title_key(&t).base, &title_key(title).base);
                            sim >= TITLE || (n == *no && sim >= 0.5)
                        }),
                        // The tracks the album has a file of already stay out.
                        None => track.is_none_or(|(_, t)| !title_keys(&t).iter().any(|k| have.contains(k))),
                    }
                })
            })
            .await
            .map_err(|e| SourceError::Io(e.to_string()));
            let _ = std::fs::remove_file(&got);
            let files = unzipped??;
            if files.is_empty() {
                return Err(if h.pick.is_some() { SourceError::Parse("the track is not in the album's zip".into()) } else { SourceError::NothingToFetch });
            }
            Ok(Fetched::Files(files))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ALBUMS: &str = include_str!("../tests/pages/bandcamp_search_albums.json");
    const TRACKS: &str = include_str!("../tests/pages/bandcamp_search_tracks.json");
    const FREE: &str = include_str!("../tests/pages/bandcamp_album_free.html");
    const PAID: &str = include_str!("../tests/pages/bandcamp_album_paid.html");
    const TRACK_PAID: &str = include_str!("../tests/pages/bandcamp_track_paid.html");
    const DOWNLOAD: &str = include_str!("../tests/pages/bandcamp_download.html");

    fn want(artist: &str, title: Option<&str>, album: Option<&str>, scope: WantScope) -> Want {
        Want {
            artists: vec![artist.into()],
            title: title.map(Into::into),
            album: album.map(Into::into),
            scope,
            ..Want::default()
        }
    }

    fn radio_album() -> Want {
        want("DM DOKURO", None, Some("UNDERTALE: The Underground Radio"), WantScope::Album)
    }

    fn radio_track() -> Want {
        Want {
            duration_ms: Some(215_700),
            track_no: 1,
            ..want("DM DOKURO", Some("The Underground Radio"), Some("UNDERTALE: The Underground Radio"), WantScope::Track)
        }
    }

    #[test]
    fn search_answers_are_read() {
        let hits = search_hits(ALBUMS).unwrap();
        assert_eq!(hits.len(), 4);
        assert_eq!(hits[0].kind, "a");
        assert_eq!(hits[0].band_name, "toby fox");
        assert_eq!(hits[0].item_url_path, "http://BASE/album/undertale-soundtrack");
        let tracks = search_hits(TRACKS).unwrap();
        assert!(tracks.iter().all(|h| h.kind == "t" && h.album_name.as_deref() == Some("UNDERTALE: The Underground Radio")));
        assert!(search_hits("<html>").is_err());
    }

    #[test]
    fn only_the_wished_artist_and_name_are_looked_into() {
        let hits = album_hits(search_hits(ALBUMS).unwrap(), &radio_album());
        assert_eq!(hits.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["UNDERTALE: The Underground Radio"]);
        // A soundtrack may be credited to a label: any artist, by name.
        let demo = album_hits(search_hits(ALBUMS).unwrap(), &want("Toby Fox", None, Some("Undertale Demo OST"), WantScope::Album));
        assert!(demo.iter().any(|h| h.name == "Undertale Demo OST"));
        assert!(!demo.iter().any(|h| h.name.contains("Radio") || h.name.contains("LIVE")));
        let tracks = track_hits(search_hits(TRACKS).unwrap(), &radio_track());
        assert_eq!(tracks.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["The Underground Radio"]);
        assert!(track_hits(search_hits(TRACKS).unwrap(), &want("Someone Else", Some("SAVE"), None, WantScope::Track)).is_empty());
    }

    #[test]
    fn free_and_paid_releases_are_told_apart() {
        let free = tralbum(FREE).unwrap();
        assert_eq!(free.item_type, "album");
        assert_eq!(free.artist.as_deref(), Some("DM DOKURO"));
        assert_eq!(free.current.title.as_deref(), Some("UNDERTALE: The Underground Radio"));
        assert!(free.free_page().unwrap().starts_with("http://BASE/download?"));
        assert_eq!(free.tracks()[1], (2, "BONESCUFFLE".to_owned()));
        assert_eq!(free.current.minimum_price, Some(0.0));
        let paid = tralbum(PAID).unwrap();
        assert_eq!(paid.free_page(), None);
        assert_eq!(paid.current.minimum_price, Some(5.0));
        let track = tralbum(TRACK_PAID).unwrap();
        assert_eq!(track.free_page(), None, "the track costs on its own");
        assert_eq!(track.album_url.as_deref(), Some("/album/undertale-the-underground-radio"));
        assert!(tralbum("<html></html>").is_none());
    }

    #[test]
    fn the_download_page_and_its_status() {
        let (link, size) = flac_link(DOWNLOAD).unwrap();
        assert!(link.starts_with("http://BASE/download/album?enc=flac&"), "{link}");
        assert_eq!(size, Some((418.6 * 1024.0 * 1024.0) as u64));
        assert_eq!(
            stat_url("https://p5.bandcamp.com/download/album?enc=flac&id=1&ts=2", 7).as_deref(),
            Some("https://p5.bandcamp.com/statdownload/album?enc=flac&id=1&ts=2&.vrs=1&.rand=7")
        );
        let ok = stat_result(r#"if ( window.Downloads ) { Downloads.statResult ( {"result":"ok","url":"x","download_url":"https://p4.bcbits.com/download/album/1/flac/2?id=2"} ) };"#).unwrap();
        assert_eq!(ok.result, "ok");
        assert_eq!(ok.download_url.as_deref(), Some("https://p4.bcbits.com/download/album/1/flac/2?id=2"));
        let old = stat_result(r#"if ( window.Downloads ) { Downloads.statResult( {"result":"err","retry_url":"https://p5.bandcamp.com/download/album?enc=flac&id=1&ts=3","errortype":"ExpiredFreeDownloadError"} ) };"#).unwrap();
        assert_eq!(old.retry_url.as_deref(), Some("https://p5.bandcamp.com/download/album?enc=flac&id=1&ts=3"));
        assert!(stat_result("nothing").is_none());
    }

    #[test]
    fn zip_names_say_the_track() {
        let tracks = vec![(1, "The Underground Radio".to_owned()), (2, "Song - 2 Mix".to_owned())];
        assert_eq!(
            entry_track("DM DOKURO - UNDERTALE_ The Underground Radio - 01 The Underground Radio.flac", &tracks),
            Some((1, "The Underground Radio".to_owned()))
        );
        assert_eq!(entry_track("A - B - 02 Song - 2 Mix.flac", &tracks), Some((2, "Song - 2 Mix".to_owned())));
        assert_eq!(entry_track("cover.jpg", &tracks), None);
        assert_eq!(file_name(&format!("{}.flac", "x".repeat(200))), format!("{}.flac", "x".repeat(120)));
    }

    #[test]
    fn a_track_free_only_with_its_album_is_offered_from_the_album() {
        let hits = track_hits(search_hits(TRACKS).unwrap(), &radio_track());
        let hit = &hits[0];
        let o = track_offer(
            "http://h/track/the-underground-radio",
            &tralbum(TRACK_PAID).unwrap(),
            hit,
            Some(("http://h/album/undertale-the-underground-radio", &tralbum(FREE).unwrap())),
            &radio_track(),
        )
        .unwrap();
        assert_eq!(o.scope, OfferScope::Track);
        assert_eq!(o.duration_ms, Some(215_667));
        assert!(o.lossless());
        let h: Handle = serde_json::from_str(&o.handle).unwrap();
        assert_eq!(h.pick, Some((1, "The Underground Radio".to_owned())));
        assert_eq!(h.release, "http://h/album/undertale-the-underground-radio");
        let ranked = crate::select::rank(vec![o], &radio_track(), &[SourceKey::Bandcamp]);
        assert!(ranked.first().is_some_and(|r| r.verdict.sure), "the artist's track at its length");
        // Paid on its own and no album: nothing.
        assert!(track_offer("http://h/t", &tralbum(TRACK_PAID).unwrap(), hit, None, &radio_track()).is_none());
        assert!(track_offer("http://h/t", &tralbum(TRACK_PAID).unwrap(), hit, Some(("http://h/a", &tralbum(PAID).unwrap())), &radio_track()).is_none());
    }

    fn server_text(s: &str, base: &str) -> String {
        s.replace("http://BASE", base)
    }

    async fn site() -> MockServer {
        let server = MockServer::start().await;
        let base = server.uri();
        for (p, body) in [
            ("/album/undertale-the-underground-radio", FREE),
            ("/album/undertale-demo-ost", PAID),
            ("/track/the-underground-radio", TRACK_PAID),
            ("/download", DOWNLOAD),
        ] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200).set_body_raw(server_text(body, &base), "text/html"))
                .mount(&server)
                .await;
        }
        server
    }

    fn quick(server: &MockServer) -> Bandcamp {
        Bandcamp::with(&format!("{}/api", server.uri()), Duration::ZERO, Duration::from_millis(5))
    }

    async fn answer(server: &MockServer, json: &str) {
        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(server_text(json, &server.uri()), "application/json"))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_free_album_is_offered_and_a_paid_one_is_not() {
        let server = site().await;
        answer(&server, ALBUMS).await;
        let bc = quick(&server);
        let offers = bc.search(&radio_album()).await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].scope, OfferScope::Album);
        assert_eq!(offers[0].album.as_deref(), Some("UNDERTALE: The Underground Radio"));
        let ranked = crate::select::rank(offers, &radio_album(), &[SourceKey::Bandcamp]);
        assert!(ranked[0].verdict.sure);
        let paid = bc.search(&want("Toby Fox", None, Some("Undertale Demo OST"), WantScope::Album)).await.unwrap();
        assert!(paid.is_empty(), "{paid:?}");
    }

    #[tokio::test]
    async fn a_track_is_found_through_its_album() {
        let server = site().await;
        answer(&server, TRACKS).await;
        let offers = quick(&server).search(&radio_track()).await.unwrap();
        assert_eq!(offers.len(), 1, "{offers:?}");
        assert_eq!(offers[0].track_title.as_deref(), Some("The Underground Radio"));
    }

    #[tokio::test]
    async fn a_script_check_is_a_block() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_raw("<html><title>Client Challenge</title></html>", "text/html"))
            .mount(&server)
            .await;
        let got = quick(&server).search(&radio_album()).await;
        assert!(matches!(got, Err(SourceError::Blocked(_))), "{got:?}");
    }

    fn album_zip() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for name in [
            "DM DOKURO - UNDERTALE_ The Underground Radio - 01 The Underground Radio.flac",
            "DM DOKURO - UNDERTALE_ The Underground Radio - 02 BONESCUFFLE.flac",
            "DM DOKURO - UNDERTALE_ The Underground Radio - 03 The Dapper Mannequin Bakery.flac",
            "cover.jpg",
            "../outside.flac",
        ] {
            w.start_file(name, opts).unwrap();
            w.write_all(b"fLaC\0\0\0\x22data").unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    async fn files_site(server: &MockServer) {
        let base = server.uri();
        Mock::given(path("/statdownload/album"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(r#"if ( window.Downloads ) {{ Downloads.statResult ( {{"result":"ok","download_url":"{base}/files/album"}} ) }};"#),
                "text/javascript",
            ))
            .mount(server)
            .await;
        Mock::given(path("/files/album"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(album_zip(), "application/zip"))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn an_album_comes_as_its_zip_unpacked_without_what_is_there() {
        let server = site().await;
        answer(&server, ALBUMS).await;
        files_site(&server).await;
        let bc = quick(&server);
        let mut w = radio_album();
        let offer = bc.search(&w).await.unwrap().remove(0);
        w.got = vec!["BONESCUFFLE".into()];
        let dir = tempfile::tempdir().unwrap();
        let progress: Progress = Arc::new(|_, _| {});
        let Fetched::Files(mut files) = bc.fetch(&offer, &w, dir.path(), progress).await.unwrap() else { panic!() };
        files.sort();
        let names: Vec<String> = files.iter().map(|f| f.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(
            names,
            [
                "DM DOKURO - UNDERTALE_ The Underground Radio - 01 The Underground Radio.flac",
                "DM DOKURO - UNDERTALE_ The Underground Radio - 03 The Dapper Mannequin Bakery.flac",
            ]
        );
        assert!(files.iter().all(|f| f.parent() == Some(dir.path())));
        assert!(!dir.path().parent().unwrap().join("outside.flac").exists(), "nothing outside the folder");
        assert!(!dir.path().join("bandcamp.download").exists(), "the zip is gone");
        assert!(!dir.path().join("cover.jpg").exists());
    }

    #[tokio::test]
    async fn a_track_is_taken_out_of_its_album_zip() {
        let server = site().await;
        answer(&server, TRACKS).await;
        files_site(&server).await;
        let bc = quick(&server);
        let offer = bc.search(&radio_track()).await.unwrap().remove(0);
        let dir = tempfile::tempdir().unwrap();
        let progress: Progress = Arc::new(|_, _| {});
        let Fetched::Files(files) = bc.fetch(&offer, &radio_track(), dir.path(), progress).await.unwrap() else { panic!() };
        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().ends_with("01 The Underground Radio.flac"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn an_old_link_is_traded_for_a_fresh_one() {
        let server = MockServer::start().await;
        let base = server.uri();
        Mock::given(path("/statdownload/old"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                format!(r#"Downloads.statResult( {{"result":"err","retry_url":"{base}/download/new?enc=flac","errortype":"ExpiredFreeDownloadError"}} )"#),
                "text/javascript",
            ))
            .mount(&server)
            .await;
        Mock::given(path("/statdownload/new"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(format!(r#"{{"result":"ok","download_url":"{base}/f"}}"#), "application/json"))
            .mount(&server)
            .await;
        let got = quick(&server).ready(&format!("{base}/download/old?enc=flac")).await.unwrap();
        assert_eq!(got, format!("{base}/f"));
    }
}
