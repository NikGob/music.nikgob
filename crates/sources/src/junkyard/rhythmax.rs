//! «Rhythmax» (odskyler): a hifi API with its own Tidal accounts, the one
//! site of this class that answered without a check when the source was
//! written. It is not the hifi-api protocol of [`crate::tidal`]: the
//! catalogue is Tidal's own v1 API, asked with a token a Cloudflare Worker
//! hands out, and the file comes as an MPEG-DASH manifest of FLAC frames
//! that is remuxed here into a plain `.flac` without decoding anything.
//!
//! Two tokens, both free and both cached until shortly before they expire:
//! the catalogue one ([`CATALOGUE_TOKEN`]) and the file one (`{base}/token`).
//! A 401 drops the cached token and asks once more.
//!
//! The manifest's `uri` is a static MPD: an initialization segment (an
//! `ftyp` + `moov` whose `stsd` carries the FLAC `dfLa` metadata blocks)
//! and numbered media segments whose `mdat` payloads are the FLAC frames in
//! order. The `.flac` is `fLaC`, the `dfLa` blocks, then every payload.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, SourceError, SourceKey, Want, WantScope};
use serde::Deserialize;
use tokio::io::AsyncWriteExt as _;

use crate::http::{
    BOT_CHECK, PAGE_TIMEOUT, Pace, Reply, client, is_bot_check, is_daily_limit, safe_name, unreachable, until_utc_midnight,
};
use crate::instances::Instances;
use crate::text::catalogue_queries;

use super::{Backend, DEFAULT_RHYTHMAX};

/// Where the catalogue token comes from: a Cloudflare Worker, free, no
/// account of ours.
const CATALOGUE_TOKEN: &str = "https://auth.odskyler.workers.dev/token";
/// Tidal's own catalogue, as the web app asks it.
const CATALOGUE: &str = "https://api.tidal.com/v1";
/// The quality asked of the manifest: CD FLAC. A hi-res item is asked at
/// this tier too, so the answer is always a FLAC manifest.
const QUALITY: &str = "LOSSLESS";
/// A token is fetched again this long before it expires.
const TOKEN_SLACK: Duration = Duration::from_secs(30);
/// The base is probed at most this often.
const HEALTH_EVERY: Duration = Duration::from_secs(30 * 60);
/// One request to one site per this.
const PACE: Duration = Duration::from_secs(1);

/// A token and when it stops being good.
#[derive(Clone)]
struct Token {
    value: String,
    until: Instant,
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    #[serde(default)]
    expires_in: u64,
}

impl Token {
    fn of(body: &str) -> Result<Self, SourceError> {
        let answer: TokenAnswer = serde_json::from_str(body).map_err(|e| SourceError::Parse(format!("a token: {e}")))?;
        let life = Duration::from_secs(answer.expires_in).saturating_sub(TOKEN_SLACK);
        Ok(Self { value: answer.access_token, until: Instant::now() + life })
    }

    fn good(&self) -> bool {
        self.until > Instant::now()
    }
}

#[derive(Deserialize)]
struct Found {
    #[serde(default)]
    items: Vec<Item>,
}

/// One track of Tidal's catalogue answer.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: serde_json::Value,
    title: String,
    #[serde(default)]
    version: Option<String>,
    /// Seconds.
    #[serde(default)]
    duration: Option<u32>,
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    audio_quality: Option<String>,
    #[serde(default)]
    artist: Option<Named>,
    #[serde(default)]
    album: Option<Album>,
}

#[derive(Deserialize)]
struct Named {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct Album {
    #[serde(default)]
    title: String,
}

pub struct Rhythmax {
    /// The bases asked, each with its own pause.
    instances: Instances,
    /// One request to Tidal's catalogue per this.
    catalogue: Pace,
    http: reqwest::Client,
    /// The catalogue's address and its token's, the built-in ones or a
    /// test's.
    api: String,
    token_url: String,
    catalogue_token: tokio::sync::Mutex<Option<Token>>,
    /// The file token and the base it came from.
    file_token: tokio::sync::Mutex<Option<(String, Token)>>,
    /// When the bases were last probed.
    probed: tokio::sync::Mutex<Option<Instant>>,
}

impl Rhythmax {
    /// The backend on `urls`, asking Tidal's catalogue with the token
    /// `auth.odskyler.workers.dev` hands out.
    pub fn new(urls: &[String]) -> Self {
        Self::with(urls, CATALOGUE, CATALOGUE_TOKEN, PACE)
    }

    /// `api` and `token` are the catalogue's addresses and `every` the pace
    /// between requests: the built-in ones, or a test's.
    fn with(urls: &[String], api: &str, token: &str, every: Duration) -> Self {
        Self {
            instances: Instances::with_own(urls, DEFAULT_RHYTHMAX, true, every),
            catalogue: Pace::new(every),
            http: client(),
            api: api.trim_end_matches('/').to_owned(),
            token_url: token.to_owned(),
            catalogue_token: tokio::sync::Mutex::new(None),
            file_token: tokio::sync::Mutex::new(None),
            probed: tokio::sync::Mutex::new(None),
        }
    }

    /// The bases are probed at most every [`HEALTH_EVERY`], so one whose
    /// accounts are all gone is skipped before a wish asks it. A base that
    /// is sitting out is skipped without a request, as everywhere else.
    async fn health(&self) -> Result<(), SourceError> {
        let fresh = {
            let mut probed = self.probed.lock().await;
            let fresh = probed.is_none_or(|t| t.elapsed() >= HEALTH_EVERY);
            if fresh {
                *probed = Some(Instant::now());
            }
            fresh
        };
        let http = &self.http;
        self.instances
            .call(|base| async move {
                if fresh {
                    probe(http, &base).await?;
                }
                Ok(base)
            })
            .await
            .map(|_| ())
    }

    /// The catalogue token, fetched again when it is spent.
    async fn catalogue_token(&self) -> Result<String, SourceError> {
        let mut slot = self.catalogue_token.lock().await;
        if let Some(token) = slot.as_ref().filter(|t| t.good()) {
            return Ok(token.value.clone());
        }
        let reply = read_with(&self.http, &self.token_url, None).await?;
        let token = Token::of(&checked(reply)?)?;
        *slot = Some(token.clone());
        Ok(token.value)
    }

    /// The file token of `base`, fetched again when it is spent.
    async fn file_token(&self, base: &str) -> Result<String, SourceError> {
        let mut slot = self.file_token.lock().await;
        if let Some((from, token)) = slot.as_ref()
            && from == base
            && token.good()
        {
            return Ok(token.value.clone());
        }
        let reply = read_with(&self.http, &format!("{base}/token"), None).await?;
        let token = Token::of(&checked(reply)?)?;
        *slot = Some((base.to_owned(), token.clone()));
        Ok(token.value)
    }

    /// Tidal's catalogue answer for `query`, with the catalogue token; a
    /// 401 drops the token and asks once more.
    async fn tracks(&self, query: &str) -> Result<Found, SourceError> {
        let url = format!("{}/search/tracks?query={}&limit=25&offset=0&countryCode=US", self.api, enc(query));
        let mut again = true;
        loop {
            let token = self.catalogue_token().await?;
            self.catalogue.wait().await;
            let reply = read_with(&self.http, &url, Some(&token)).await?;
            if reply.status == 401 && again {
                again = false;
                *self.catalogue_token.lock().await = None;
                continue;
            }
            let body = checked(reply)?;
            return serde_json::from_str(&body).map_err(|e| SourceError::Parse(e.to_string()));
        }
    }

    /// The manifest's `uri` of `id` on `base`, with the file token; a 401
    /// drops the token and asks once more.
    async fn manifest(&self, base: &str, id: &str) -> Result<String, SourceError> {
        let url = format!("{base}/manifests?id={}&quality={QUALITY}", enc(id));
        let mut again = true;
        loop {
            let token = self.file_token(base).await?;
            self.instances.wait().await;
            let reply = read_with(&self.http, &url, Some(&token)).await?;
            if reply.status == 401 && again {
                again = false;
                *self.file_token.lock().await = None;
                continue;
            }
            return manifest_uri(&checked(reply)?);
        }
    }

    /// The `.flac` of `id` from `base` into `dest`.
    async fn fetch_one(&self, base: &str, id: &str, dest: &Path, progress: &Progress) -> Result<PathBuf, SourceError> {
        let uri = self.manifest(base, id).await?;
        let mpd = read_bytes(&self.http, &uri, None).await?;
        let plan = Plan::of(&mpd)?;
        write_flac(&self.http, &plan, dest, progress).await
    }
}

impl Backend for Rhythmax {
    fn name(&self) -> &'static str {
        "Rhythmax"
    }

    fn key(&self) -> &'static str {
        "rhythmax"
    }

    fn search<'a>(&'a self, want: &'a Want) -> BoxFuture<'a, Result<Vec<Offer>, SourceError>> {
        Box::pin(async move {
            // Tracks only: an album is asked of the backends that list one.
            if want.scope != WantScope::Track {
                return Ok(Vec::new());
            }
            self.health().await?;
            let mut out: Vec<Offer> = Vec::new();
            for q in catalogue_queries(want) {
                out.extend(self.tracks(&q).await?.items.iter().filter_map(offer_of));
                if !out.is_empty() {
                    break;
                }
            }
            out.dedup_by(|a, b| a.id == b.id);
            Ok(out)
        })
    }

    fn fetch<'a>(
        &'a self,
        offer: &'a Offer,
        _want: &'a Want,
        dest: &'a Path,
        progress: Progress,
    ) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            let id = offer
                .handle
                .strip_prefix("rhythmax:")
                .ok_or_else(|| SourceError::Parse(format!("not a Rhythmax handle: {}", offer.handle)))?;
            let name = safe_name(offer.track_title.as_deref().unwrap_or(&offer.title));
            let progress = &progress;
            let path = self
                .instances
                .call(|base| {
                    let name = name.clone();
                    async move { self.fetch_one(&base, id, &dest.join(format!("{name}.flac")), progress).await }
                })
                .await?;
            Ok(Fetched::Files(vec![path]))
        })
    }
}

/// The offer of one catalogue item, `None` for one that has no lossless
/// version (the manifest would give an AAC file, not a FLAC).
fn offer_of(item: &Item) -> Option<Offer> {
    if !matches!(item.audio_quality.as_deref(), Some("LOSSLESS" | "HI_RES_LOSSLESS")) {
        return None;
    }
    let id = id_text(&item.id)?;
    let title = with_version(&item.title, item.version.as_deref());
    let artist = item.artist.as_ref().map(|a| a.name.clone()).filter(|n| !n.is_empty());
    Some(Offer {
        source: SourceKey::Junkyard,
        provider: "Rhythmax".into(),
        id: format!("rhythmax:{id}"),
        title: format!("{} — {title}", artist.as_deref().unwrap_or("?")),
        artist,
        track_title: Some(title),
        album: item.album.as_ref().map(|a| a.title.clone()).filter(|t| !t.is_empty()),
        duration_ms: item.duration.map(|d| d * 1000),
        isrc: item.isrc.clone(),
        scope: OfferScope::Track,
        format: Some(Format { bit_depth: Some(16), sample_rate: Some(44_100), ..Format::new(Codec::Flac) }),
        size_bytes: None,
        seeders: None,
        info_url: None,
        handle: format!("rhythmax:{id}"),
    })
}

fn id_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A version the title does not already say.
fn with_version(title: &str, version: Option<&str>) -> String {
    match version.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if !title.to_lowercase().contains(&v.to_lowercase()) => format!("{title} ({v})"),
        _ => title.to_owned(),
    }
}

/// The manifest's `uri` in a `/manifests` answer: `data.data.attributes`
/// (the web app's shape) or `data.attributes` (one level up). A
/// presentation other than `FULL` is a preview, and there is no file.
fn manifest_uri(body: &str) -> Result<String, SourceError> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| SourceError::Parse(format!("a manifest: {e}")))?;
    let data = v.get("data").ok_or_else(|| SourceError::Parse("a manifest without data".into()))?;
    let attributes = data
        .get("data")
        .and_then(|d| d.get("attributes"))
        .or_else(|| data.get("attributes"))
        .ok_or_else(|| SourceError::Parse("a manifest without attributes".into()))?;
    let presentation = attributes.get("trackPresentation").and_then(serde_json::Value::as_str).unwrap_or("FULL");
    if presentation != "FULL" {
        return Err(SourceError::NothingToFetch);
    }
    attributes
        .get("uri")
        .and_then(serde_json::Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
        .ok_or(SourceError::NothingToFetch)
}

/// What the MPD says to fetch.
struct Plan {
    init: String,
    /// The media template, with `$Number$` in it.
    media: String,
    start: u64,
    count: u64,
    /// The file's size as the MPD estimates it (bandwidth × duration).
    total: Option<u64>,
}

impl Plan {
    fn of(mpd: &[u8]) -> Result<Self, SourceError> {
        let text = std::str::from_utf8(mpd).map_err(|e| SourceError::Parse(format!("the MPD: {e}")))?;
        let init = attr(text, "initialization").ok_or_else(|| SourceError::Parse("an MPD without an initialization".into()))?;
        let media = attr(text, "media").ok_or_else(|| SourceError::Parse("an MPD without media segments".into()))?;
        let start = attr(text, "startNumber").and_then(|v| v.parse().ok()).unwrap_or(1);
        let timeline = between(text, "<SegmentTimeline>", "</SegmentTimeline>")
            .ok_or_else(|| SourceError::Parse("an MPD without a segment timeline".into()))?;
        let count = segments(timeline);
        if count == 0 {
            return Err(SourceError::Parse("an MPD with no segment".into()));
        }
        let total = attr(text, "bandwidth")
            .and_then(|v| v.parse::<u64>().ok())
            .zip(duration_secs(text))
            .map(|(bandwidth, secs)| (bandwidth as f64 * secs / 8.0) as u64);
        Ok(Self { init: unescape(init), media: unescape(media), start, count, total })
    }

    /// The URL of the `n`th segment (from 0).
    fn segment(&self, n: u64) -> String {
        self.media.replace("$Number$", &(self.start + n).to_string())
    }
}

/// The value of `name="…"` in `text`.
fn attr<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let at = text.find(&format!("{name}=\""))? + name.len() + 2;
    let end = text[at..].find('"')?;
    Some(&text[at..at + end])
}

/// What sits between `open` and the next `close`.
fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let at = text.find(open)? + open.len();
    let end = text[at..].find(close)?;
    Some(&text[at..at + end])
}

/// How many segments a `SegmentTimeline` names: each `<S>` is one, plus its
/// `r` repeats.
fn segments(timeline: &str) -> u64 {
    let mut count = 0u64;
    let mut rest = timeline;
    while let Some(at) = rest.find("<S ") {
        let Some(end) = rest[at..].find('>') else { break };
        let tag = &rest[at..at + end];
        count += attr(tag, "r").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + 1;
        rest = &rest[at + end..];
    }
    count
}

/// `PT2M41.25S` → 161.25 seconds.
fn duration_secs(text: &str) -> Option<f64> {
    let rest = attr(text, "mediaPresentationDuration")?.strip_prefix("PT")?;
    let mut total = 0.0;
    let mut number = String::new();
    for c in rest.chars() {
        match c {
            '0'..='9' | '.' => number.push(c),
            'H' => {
                total += number.parse::<f64>().ok()? * 3600.0;
                number.clear();
            }
            'M' => {
                total += number.parse::<f64>().ok()? * 60.0;
                number.clear();
            }
            'S' => {
                total += number.parse::<f64>().ok()?;
                number.clear();
            }
            _ => return None,
        }
    }
    Some(total)
}

/// The XML escapes an attribute value carries.
fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

/// The payload of the first box of `kind` in `data` (a list of boxes).
fn box_of<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let size = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        let name = &data[at + 4..at + 8];
        let (header, size) = match size {
            0 => (8, data.len() - at),
            1 => {
                let large = u64::from_be_bytes(data.get(at + 8..at + 16)?.try_into().ok()?);
                (16, usize::try_from(large).ok()?)
            }
            n => (8, n),
        };
        if size < header || at + size > data.len() {
            return None;
        }
        if name == kind.as_slice() {
            return Some(&data[at + header..at + size]);
        }
        at += size;
    }
    None
}

/// The head of the `.flac`: `fLaC` and the metadata blocks the init
/// segment's `dfLa` box carries (STREAMINFO first).
fn flac_header(init: &[u8]) -> Result<Vec<u8>, SourceError> {
    let missing = |what: &str| SourceError::Parse(format!("the init segment has no {what}"));
    let moov = box_of(init, b"moov").ok_or_else(|| missing("moov"))?;
    let trak = box_of(moov, b"trak").ok_or_else(|| missing("trak"))?;
    let mdia = box_of(trak, b"mdia").ok_or_else(|| missing("mdia"))?;
    let minf = box_of(mdia, b"minf").ok_or_else(|| missing("minf"))?;
    let stbl = box_of(minf, b"stbl").ok_or_else(|| missing("stbl"))?;
    let stsd = box_of(stbl, b"stsd").ok_or_else(|| missing("stsd"))?;
    // A FullBox: version and flags, then the entry count.
    let entries = stsd.get(8..).ok_or_else(|| missing("stsd entries"))?;
    let entry = box_of(entries, b"fLaC").ok_or_else(|| missing("fLaC sample entry"))?;
    // The audio sample entry's own fields, before its child boxes.
    let children = entry.get(28..).ok_or_else(|| missing("fLaC sample entry fields"))?;
    let dfla = box_of(children, b"dfLa").ok_or_else(|| missing("dfLa"))?;
    // A FullBox: version and flags, then the FLAC metadata blocks.
    let blocks = dfla.get(4..).ok_or_else(|| missing("dfLa blocks"))?;
    let mut out = Vec::with_capacity(4 + blocks.len());
    out.extend_from_slice(b"fLaC");
    out.extend_from_slice(blocks);
    Ok(out)
}

/// The FLAC frames of one media segment: its `mdat` payload.
fn frames(segment: &[u8]) -> Result<&[u8], SourceError> {
    box_of(segment, b"mdat").ok_or_else(|| SourceError::Parse("a segment without mdat".into()))
}

/// The `.flac` of `plan` into `dest`, through a `.part` file renamed when
/// whole.
async fn write_flac(http: &reqwest::Client, plan: &Plan, dest: &Path, progress: &Progress) -> Result<PathBuf, SourceError> {
    let io = |e: std::io::Error| SourceError::Io(e.to_string());
    let init = read_bytes(http, &plan.init, None).await?;
    let header = flac_header(&init)?;
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(io)?;
    }
    let part = dest.with_extension("flac.part");
    let mut file = tokio::fs::File::create(&part).await.map_err(io)?;
    let mut done = header.len() as u64;
    let result: Result<(), SourceError> = async {
        file.write_all(&header).await.map_err(io)?;
        progress(done, plan.total);
        for n in 0..plan.count {
            let segment = read_bytes(http, &plan.segment(n), None).await?;
            let payload = frames(&segment)?;
            file.write_all(payload).await.map_err(io)?;
            done += payload.len() as u64;
            progress(done, plan.total);
        }
        file.flush().await.map_err(io)?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    tokio::fs::rename(&part, dest).await.map_err(io)?;
    Ok(dest.to_owned())
}

/// The health answer of a base: no active account is a refusal, so the
/// base sits out.
async fn probe(http: &reqwest::Client, base: &str) -> Result<(), SourceError> {
    let body = checked(read_with(http, &format!("{base}/"), None).await?)?;
    let v: serde_json::Value = serde_json::from_str(&body).map_err(|e| SourceError::Parse(format!("the health: {e}")))?;
    match v.get("active").and_then(serde_json::Value::as_u64) {
        Some(0) => Err(SourceError::Blocked("no active account".into())),
        _ => Ok(()),
    }
}

/// The body of a reply that came, or the error it is.
fn checked(reply: Reply) -> Result<String, SourceError> {
    let status = reqwest::StatusCode::from_u16(reply.status).unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if status.is_success() {
        return Ok(reply.body);
    }
    Err(classify(status, &reply.body, reply.retry_after))
}

/// A bot check, a daily quota, a wait asked for, or the status.
fn classify(status: reqwest::StatusCode, body: &str, retry_after: Option<Duration>) -> SourceError {
    if is_bot_check(status, body) {
        return SourceError::Blocked(BOT_CHECK.into());
    }
    if is_daily_limit(status, body) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        return SourceError::RateLimited { retry_after: Some(until_utc_midnight(now)) };
    }
    match status {
        reqwest::StatusCode::TOO_MANY_REQUESTS => SourceError::RateLimited { retry_after },
        s => SourceError::Http { status: s.as_u16() },
    }
}

/// `url` with a bearer token, as [`crate::http::read`] reads it.
async fn read_with(http: &reqwest::Client, url: &str, token: Option<&str>) -> Result<Reply, SourceError> {
    let mut req = http.get(url).timeout(PAGE_TIMEOUT);
    if let Some(token) = token {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = req.send().await.map_err(unreachable)?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = resp.text().await.map_err(unreachable)?;
    Ok(Reply { status, body, retry_after })
}

/// The bytes of `url` (a segment, an MPD), with a bearer token when given.
async fn read_bytes(http: &reqwest::Client, url: &str, token: Option<&str>) -> Result<Vec<u8>, SourceError> {
    let mut req = http.get(url).timeout(PAGE_TIMEOUT);
    if let Some(token) = token {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let resp = req.send().await.map_err(unreachable)?;
    let status = resp.status();
    if !status.is_success() {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = resp.text().await.unwrap_or_default();
        return Err(classify(status, &body, retry_after));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(unreachable)
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use nikgob_core::source::Source;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The MPD of a real answer, trimmed: two `<S>` (40 + 1 segments), the
    /// URLs XML-escaped.
    const MPD: &str = r#"<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="static" mediaPresentationDuration="PT2M41.25S"><Period id="0"><AdaptationSet id="0" contentType="audio" mimeType="audio/mp4"><Representation id="FLAC,44100,16" codecs="flac" bandwidth="838204" audioSamplingRate="44100"><AudioChannelConfiguration schemeIdUri="urn:mpeg:dash:23003:3:audio_channel_configuration:2011" value="2"/><SegmentTemplate timescale="44100" initialization="https://sp-ad-fa.audio.tidal.com/mediatracks/GisI/0.mp4?token=1795206429~L21l&amp;info=UExB" media="https://sp-ad-fa.audio.tidal.com/mediatracks/GisI/$Number$.mp4?token=1795206429~L21l&amp;info=UExB" startNumber="1"><SegmentTimeline><S d="176128" r="39"/><S d="66005"/></SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#;

    /// One real item of `/search/tracks`, trimmed.
    const ITEM: &str = r#"{"id":172802834,"title":"Warum warum (Supraland Credits Song)","version":null,"duration":161,"trackNumber":1,"volumeNumber":1,"isrc":"DEHY11900690","audioQuality":"LOSSLESS","mediaMetadata":{"tags":["LOSSLESS"]},"artist":{"id":8563652,"name":"Con Fetti Pesto","type":"MAIN"},"artists":[{"id":8563652,"name":"Con Fetti Pesto","type":"MAIN"}],"album":{"id":172802830,"title":"Warum Warum (Supraland Credits Song)","cover":"f287eadb-b7d0-40a8-875a-d3fbe4bec7e6"}}"#;

    fn item(json: &str) -> Item {
        serde_json::from_str(json).unwrap()
    }

    fn want() -> Want {
        Want {
            artists: vec!["Con Fetti Pesto".into()],
            title: Some("Warum warum (Supraland Credits Song)".into()),
            album: Some("Warum Warum (Supraland Credits Song)".into()),
            duration_ms: Some(161_000),
            ..Want::default()
        }
    }

    /// An MPD of `segments` segments, pointing at `init` and `media`.
    fn mpd(init: &str, media: &str, segments: &str) -> String {
        format!(
            r#"<MPD type="static" mediaPresentationDuration="PT0M2S"><Period><AdaptationSet><Representation id="FLAC,44100,16" codecs="flac" bandwidth="1000"><SegmentTemplate timescale="44100" initialization="{init}" media="{media}" startNumber="1"><SegmentTimeline>{segments}</SegmentTimeline></SegmentTemplate></Representation></AdaptationSet></Period></MPD>"#
        )
    }

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    /// An init segment shaped like the real one: `ftyp` + `moov` whose
    /// `stsd` carries a `fLaC` sample entry with a `dfLa` box holding a
    /// 34-byte STREAMINFO block.
    fn init_segment() -> Vec<u8> {
        let mut streaminfo = vec![0x80, 0x00, 0x00, 0x22];
        streaminfo.extend_from_slice(&[0x11; 34]);
        let mut dfla = vec![0u8; 4];
        dfla.extend_from_slice(&streaminfo);
        let mut entry = vec![0u8; 28];
        entry.extend_from_slice(&boxed(b"dfLa", &dfla));
        let mut stsd = vec![0u8; 8];
        stsd.extend_from_slice(&boxed(b"fLaC", &entry));
        let stbl = boxed(b"stbl", &boxed(b"stsd", &stsd));
        let minf = boxed(b"minf", &stbl);
        let mdia = boxed(b"mdia", &minf);
        let trak = boxed(b"trak", &mdia);
        let mut out = boxed(b"ftyp", b"isom");
        out.extend_from_slice(&boxed(b"moov", &trak));
        out
    }

    fn media_segment(frames: &[u8]) -> Vec<u8> {
        let mut out = boxed(b"moof", b"mfhd");
        out.extend_from_slice(&boxed(b"mdat", frames));
        out
    }

    fn progress() -> Progress {
        Arc::new(|_, _| {})
    }

    /// The MPD's init URL and its 41 segment URLs, the escapes undone.
    #[test]
    fn the_mpd_names_the_init_and_every_segment() {
        let plan = Plan::of(MPD.as_bytes()).unwrap();
        assert_eq!(plan.init, "https://sp-ad-fa.audio.tidal.com/mediatracks/GisI/0.mp4?token=1795206429~L21l&info=UExB");
        assert_eq!(plan.count, 41, "40 repeats and one");
        assert_eq!(plan.start, 1);
        assert_eq!(
            plan.segment(0),
            "https://sp-ad-fa.audio.tidal.com/mediatracks/GisI/1.mp4?token=1795206429~L21l&info=UExB"
        );
        assert_eq!(
            plan.segment(40),
            "https://sp-ad-fa.audio.tidal.com/mediatracks/GisI/41.mp4?token=1795206429~L21l&info=UExB"
        );
        assert_eq!(plan.total, Some((838_204.0 * 161.25 / 8.0) as u64), "the estimate of the size");
        assert_eq!(duration_secs(MPD), Some(161.25));
        assert!(Plan::of(b"<MPD></MPD>").is_err(), "an MPD without segments is not a plan");
    }

    /// The remux: `fLaC`, the STREAMINFO block, then the two segments'
    /// frames in order.
    #[tokio::test]
    async fn the_remux_writes_a_flac() {
        let server = MockServer::start().await;
        let init = init_segment();
        let (first, second) = (media_segment(b"frame-one"), media_segment(b"frame-two"));
        for (at, body) in [("/0.mp4", init.clone()), ("/1.mp4", first.clone()), ("/2.mp4", second.clone())] {
            Mock::given(path(at))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
                .mount(&server)
                .await;
        }
        let plan = Plan {
            init: format!("{}/0.mp4", server.uri()),
            media: format!("{}/$Number$.mp4", server.uri()),
            start: 1,
            count: 2,
            total: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("01 - Warum warum.flac");
        let path = write_flac(&client(), &plan, &dest, &progress()).await.unwrap();
        let got = std::fs::read(&path).unwrap();
        let mut expected = b"fLaC".to_vec();
        expected.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]);
        expected.extend_from_slice(&[0x11; 34]);
        expected.extend_from_slice(b"frame-one");
        expected.extend_from_slice(b"frame-two");
        assert_eq!(got, expected);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "nothing left of the .part file");
    }

    /// A catalogue item becomes an offer of this source: FLAC, the length
    /// in ms, the artist, the title with its version.
    #[test]
    fn a_catalogue_item_becomes_an_offer() {
        let offer = offer_of(&item(ITEM)).unwrap();
        assert_eq!(offer.source, SourceKey::Junkyard);
        assert_eq!(offer.provider, "Rhythmax");
        assert_eq!(offer.handle, "rhythmax:172802834");
        assert_eq!(offer.title, "Con Fetti Pesto — Warum warum (Supraland Credits Song)");
        assert_eq!(offer.artist.as_deref(), Some("Con Fetti Pesto"));
        assert_eq!(offer.album.as_deref(), Some("Warum Warum (Supraland Credits Song)"));
        assert_eq!(offer.duration_ms, Some(161_000));
        assert_eq!(offer.isrc.as_deref(), Some("DEHY11900690"));
        assert!(offer.lossless());
        assert_eq!(offer.format.unwrap().label(), "FLAC");
        let versioned = item(r#"{"id":7,"title":"Avant","version":"Remastered","audioQuality":"HI_RES_LOSSLESS"}"#);
        assert_eq!(offer_of(&versioned).unwrap().track_title.as_deref(), Some("Avant (Remastered)"));
        let lossy = item(r#"{"id":8,"title":"Avant","audioQuality":"HIGH"}"#);
        assert!(offer_of(&lossy).is_none(), "no lossless version: no offer");
    }

    /// A preview is no file; the two shapes of the answer both give the
    /// `uri`.
    #[test]
    fn a_preview_is_no_file() {
        let full = r#"{"data":{"data":{"attributes":{"trackPresentation":"FULL","uri":"https://im-fa.manifest.tidal.com/1/manifests/x.mpd?token=1"}}}}"#;
        assert_eq!(manifest_uri(full).unwrap(), "https://im-fa.manifest.tidal.com/1/manifests/x.mpd?token=1");
        let one_up = r#"{"data":{"attributes":{"trackPresentation":"FULL","uri":"https://im-fa.manifest.tidal.com/1/manifests/y.mpd"}}}"#;
        assert_eq!(manifest_uri(one_up).unwrap(), "https://im-fa.manifest.tidal.com/1/manifests/y.mpd");
        let preview = r#"{"data":{"data":{"attributes":{"trackPresentation":"PREVIEW","uri":"https://x/y.mpd"}}}}"#;
        assert_eq!(manifest_uri(preview).unwrap_err(), SourceError::NothingToFetch);
        let no_uri = r#"{"data":{"data":{"attributes":{"trackPresentation":"FULL"}}}}"#;
        assert_eq!(manifest_uri(no_uri).unwrap_err(), SourceError::NothingToFetch);
    }

    /// The whole chain against a mock: token, search, manifest, MPD, init
    /// and two segments, and the `.flac` on disk.
    #[tokio::test]
    async fn the_whole_chain_gives_a_flac() {
        let server = MockServer::start().await;
        let json = |body: String| ResponseTemplate::new(200).set_body_raw(body, "application/json");
        let init = init_segment();
        let (first, second) = (media_segment(b"frame-one"), media_segment(b"frame-two"));
        let mpd = mpd(
            &format!("{}/0.mp4", server.uri()),
            &format!("{}/$Number$.mp4", server.uri()),
            r#"<S d="1"/><S d="1"/>"#,
        );
        let mocks: Vec<(&str, ResponseTemplate)> = vec![
            ("/", json(r#"{"accounts":22,"active":17}"#.into())),
            ("/token", json(r#"{"access_token":"file-token","client_name":"Rhythmax by ODSkyler","expires_in":600}"#.into())),
            (
                "/cat-token",
                json(r#"{"access_token":"cat-token","clientName":"x","expires_in":14400,"scope":"","token_type":"Bearer"}"#.into()),
            ),
            ("/v1/search/tracks", json(format!(r#"{{"items":[{ITEM}],"totalNumberOfItems":1}}"#))),
            (
                "/manifests",
                json(format!(
                    r#"{{"data":{{"data":{{"attributes":{{"trackPresentation":"FULL","uri":"{}/track.mpd"}}}}}}}}"#,
                    server.uri()
                )),
            ),
            ("/track.mpd", ResponseTemplate::new(200).set_body_raw(mpd, "application/dash+xml")),
            ("/0.mp4", ResponseTemplate::new(200).set_body_bytes(init)),
            ("/1.mp4", ResponseTemplate::new(200).set_body_bytes(first)),
            ("/2.mp4", ResponseTemplate::new(200).set_body_bytes(second)),
        ];
        for (at, body) in mocks {
            Mock::given(path(at)).respond_with(body).mount(&server).await;
        }
        let backend = Rhythmax::with(
            &[server.uri()],
            &format!("{}/v1", server.uri()),
            &format!("{}/cat-token", server.uri()),
            Duration::ZERO,
        );
        let junkyard = super::super::Junkyard { backends: vec![Box::new(backend)] };
        let found = junkyard.search(&want()).await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].handle, "rhythmax:172802834");
        let dir = tempfile::tempdir().unwrap();
        let got = junkyard.fetch(&found[0], &want(), dir.path(), progress()).await.unwrap();
        let Fetched::Files(files) = got else { panic!("{got:?}") };
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_name().unwrap().to_string_lossy(), "Warum warum (Supraland Credits Song).flac");
        let bytes = std::fs::read(&files[0]).unwrap();
        assert!(bytes.starts_with(b"fLaC"), "a FLAC file");
        assert!(bytes.ends_with(b"frame-two"), "the last segment's frames");
        assert!(bytes.windows(9).any(|w| w == b"frame-one".as_slice()), "the first segment's frames");
    }
}
