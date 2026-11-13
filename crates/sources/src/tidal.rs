//! Tidal through a public hifi-api instance
//! (github.com/uimaxbai/hifi-api, the API behind Monochrome and its kin):
//! `GET /search/?s=` (tracks) and `?al=` (albums), `GET /album/?id=` lists
//! an album, `GET /track/?id=&quality=LOSSLESS` gives a manifest
//! (`application/vnd.tidal.bts`: base64 JSON with the file's `urls`). The
//! CD-quality tier is asked: hi-res comes as a DASH manifest of segments.
//!
//! Instances come and go (Tidal bans their accounts): several are asked in
//! turn, and the answers are read loosely (older instances wrap them in a
//! list). Which ones work is read from Monochrome's uptime list
//! ([`UPTIME`]): it tries every instance it knows every few minutes and
//! tells those that search from those that also give files. An instance
//! can search and still refuse every file (`403 {"detail": "Upstream API
//! error"}`: Tidal no longer plays for its account); the file is asked of
//! the ones that give files. When the list says none gives files, Tidal
//! offers nothing (its search says so, [`SourceError::NothingToFetch`])
//! until one does.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use nikgob_core::indexer::BoxFuture;
use nikgob_core::model::Codec;
use nikgob_core::source::{Fetched, Format, Offer, OfferScope, Progress, Source, SourceError, SourceKey, Want, WantScope};
use serde_json::Value;

use crate::album::{Patience, fetch_each, left_out};
use crate::http::{Pace, client, download, get_json, is_bot_check, is_daily_limit, read, safe_name, until_utc_midnight};
use crate::instances::Instances;
use crate::text::catalogue_queries;

/// Asked when the uptime list cannot be read: the one that searched on
/// 2026-11-04 (its files answered 403 that day). The monochrome.tf names
/// (api, eu-central, us-west) had no DNS records any more, and
/// `tracks.monochrome.st` is no hifi-api now (404 «Route not found» on
/// every path, 2026-11-09).
pub const DEFAULT_INSTANCES: &[&str] = &["https://monochrome-api.samidy.com"];

/// Monochrome's uptime worker: `{"api": [{"url"}], "streaming": [{"url"}],
/// "down": [{"url", "error"}]}`. The mirror
/// `tidal-uptime.jiffy-puffs-1j.workers.dev` answered 404 (error 1042) on
/// 2026-11-10; a dead second URL made a failed first look like no list.
pub const UPTIME: &[&str] = &["https://tidal-uptime.props-76styles.workers.dev/"];
/// The uptime list is read again after this.
const UPTIME_EVERY: Duration = Duration::from_secs(30 * 60);

pub struct Tidal {
    /// The instances that search (and list albums).
    instances: Instances,
    /// The instances asked for files.
    files: Instances,
    /// The uptime list says no instance gives files now.
    no_files: AtomicBool,
    /// One request to the instances per this.
    pace: Pace,
    /// Where the lists of the instances that work are read.
    uptime: Vec<String>,
    /// The user's list is in use: the uptime list does not replace it.
    follow_uptime: AtomicBool,
    /// `set_instances` asked for a fresh uptime read.
    stale: AtomicBool,
    /// When the uptime list was last read.
    checked: tokio::sync::Mutex<Option<Instant>>,
    /// When the user's own instances were last probed.
    probed: tokio::sync::Mutex<Option<Instant>>,
    http: reqwest::Client,
}

impl Tidal {
    pub fn new(urls: &[String]) -> Self {
        Self::with_pace(urls, Duration::from_millis(1500))
    }

    pub fn with_pace(urls: &[String], every: Duration) -> Self {
        let own = urls.iter().any(|u| !u.trim().is_empty());
        Self::from_list(urls, own, every)
    }

    /// `own`: `urls` is the user's list (`sources.toml`). The uptime list
    /// is not asked, and a reload can turn that back on.
    pub fn from_list(urls: &[String], own: bool, every: Duration) -> Self {
        Self {
            instances: Instances::with_own(urls, DEFAULT_INSTANCES, own, Duration::ZERO),
            files: Instances::with_own(urls, DEFAULT_INSTANCES, own, Duration::ZERO),
            no_files: AtomicBool::new(false),
            pace: Pace::new(every),
            uptime: UPTIME.iter().map(|u| (*u).to_owned()).collect(),
            follow_uptime: AtomicBool::new(!own),
            stale: AtomicBool::new(false),
            checked: tokio::sync::Mutex::new(None),
            probed: tokio::sync::Mutex::new(None),
            http: client(),
        }
    }

    /// The list `sources.toml` names now. An instance that stays keeps its pause.
    pub fn set_instances(&self, urls: &[String], own: bool) {
        self.instances.apply(urls, DEFAULT_INSTANCES, own);
        self.files.apply(urls, DEFAULT_INSTANCES, own);
        self.follow_uptime.store(!own, Ordering::Relaxed);
        if own {
            self.no_files.store(false, Ordering::Relaxed);
        } else {
            self.stale.store(true, Ordering::Relaxed);
        }
    }

    /// The search instances, and whether the list is the user's.
    pub fn instances_now(&self) -> (Vec<String>, bool) {
        (self.instances.urls(), self.instances.is_own())
    }

    /// The instances of the uptime list, read at most every
    /// [`UPTIME_EVERY`]; the lists stay as they were when it cannot be read.
    /// The user's own list is not replaced.
    async fn refresh(&self) {
        if !self.follow_uptime.load(Ordering::Relaxed) {
            return;
        }
        let mut checked = self.checked.lock().await;
        if self.stale.swap(false, Ordering::Relaxed) {
            *checked = None;
        }
        if checked.is_some_and(|t| t.elapsed() < UPTIME_EVERY) {
            return;
        }
        *checked = Some(Instant::now());
        for url in &self.uptime {
            match get_json::<Value>(&self.http, url).await {
                Ok(v) => {
                    let (search, files) = uptime_lists(&v, DEFAULT_INSTANCES);
                    let before = (self.instances.urls(), self.files.urls(), self.no_files.load(Ordering::Relaxed));
                    self.instances.replace(&search);
                    if let Some(files) = &files {
                        self.files.replace(files);
                    }
                    self.no_files.store(files.is_none(), Ordering::Relaxed);
                    let after = (self.instances.urls(), self.files.urls(), files.is_none());
                    if before != after {
                        match files {
                            Some(_) => log::info!("tidal: searching in {:?}, files from {:?}", after.0, after.1),
                            None => log::info!("tidal: searching in {:?}, no instance gives files", after.0),
                        }
                    }
                    return;
                }
                Err(e) => log::info!("tidal: the uptime list {url}: {e}"),
            }
        }
    }
}

/// The instances an uptime answer says work: those that search, and those
/// that give files, each followed by `defaults` it does not say are down.
/// An answer without `streaming` (an older list) names none apart: the
/// searching ones are asked for files. One with `streaming` empty says no
/// instance gives files: `None`, nobody is asked for a file only to answer
/// 403.
pub fn uptime_lists(v: &Value, defaults: &[&str]) -> (Vec<String>, Option<Vec<String>>) {
    let urls = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned).or_else(|| s(x, "url"))).collect())
            .unwrap_or_default()
    };
    let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
    let down = urls("down");
    let with_defaults = |mut list: Vec<String>| {
        for d in defaults {
            if !list.iter().chain(&down).any(|u| same(u, d)) {
                list.push((*d).to_owned());
            }
        }
        list
    };
    let api = urls("api");
    let files = match v.get("streaming") {
        None | Some(Value::Null) => Some(api.clone()),
        Some(_) => Some(urls("streaming")).filter(|s| !s.is_empty()),
    };
    (with_defaults(api), files.map(with_defaults))
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
        Some(ver) if !t.to_lowercase().contains(&ver.to_lowercase()) => format!("{t} ({ver})"),
        Some(_) => t,
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

/// Tries on one instance before the next one is asked.
const RETRIES: u32 = 3;
/// HTTP attempts for one item, across every instance. Two instances get
/// the three tries; a longer list is not asked. Never a flood.
const ITEM_CAP: u32 = 6;
const UPSTREAM: &str = "Upstream API error";

/// What to do after one attempt of an item.
enum Again {
    /// Wait, then try the same instance.
    Wait(Duration),
    /// This instance is done; the next one may be asked.
    Next,
    /// The item's cap is spent; no further request.
    Stop,
}

/// `status` 0 is a timeout. A 403 is retried only when the body is the
/// upstream error the owner sees clear on a later press; a ban is not.
fn retryable(status: u16, body: &str) -> bool {
    if status == 0 {
        return true;
    }
    if status == 403 && body.contains(UPSTREAM) {
        return true;
    }
    matches!(status, 202 | 408 | 425 | 500 | 502 | 503 | 504 | 521 | 522 | 524)
}

fn again(attempt: u32, status: u16, body: &str, spent: u32) -> Again {
    if spent >= ITEM_CAP {
        return Again::Stop;
    }
    if attempt + 1 >= RETRIES || !retryable(status, body) {
        return Again::Next;
    }
    let wait = if attempt == 0 { Duration::from_secs(1) } else { Duration::from_secs(4) };
    Again::Wait(wait)
}

struct Budget {
    spent: AtomicU32,
    last: Mutex<SourceError>,
}

impl Budget {
    fn new() -> Self {
        Self { spent: AtomicU32::new(0), last: Mutex::new(SourceError::NotConfigured) }
    }

    fn exhausted(&self) -> bool {
        self.spent.load(Ordering::Relaxed) >= ITEM_CAP
    }

    fn last(&self) -> SourceError {
        self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn note(&self, e: &SourceError) {
        *self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = e.clone();
    }

    /// Reserves one attempt. False when the cap was already spent.
    fn take(&self) -> bool {
        self.spent.fetch_add(1, Ordering::Relaxed) < ITEM_CAP
    }
}

enum Fail {
    Maybe { status: u16, body: String, error: SourceError },
    Stop(SourceError),
}

fn transport(e: SourceError) -> Fail {
    match &e {
        SourceError::Unreachable(s) if s.contains("timed out") => Fail::Maybe { status: 0, body: String::new(), error: e },
        _ => Fail::Stop(e),
    }
}

async fn once(http: &reqwest::Client, url: &str) -> Result<Value, Fail> {
    let reply = match read(http, url).await {
        Ok(r) => r,
        Err(e) => return Err(transport(e)),
    };
    let status = reqwest::StatusCode::from_u16(reply.status).unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if status.is_success() {
        return serde_json::from_str(&reply.body).map_err(|e| Fail::Stop(SourceError::Parse(e.to_string())));
    }
    if is_daily_limit(status, &reply.body) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        return Err(Fail::Stop(SourceError::RateLimited { retry_after: Some(until_utc_midnight(now)) }));
    }
    if is_bot_check(status, &reply.body) {
        return Err(Fail::Stop(SourceError::Blocked(crate::http::BOT_CHECK.into())));
    }
    let error = match status {
        reqwest::StatusCode::TOO_MANY_REQUESTS => SourceError::RateLimited { retry_after: reply.retry_after },
        s => SourceError::Http { status: s.as_u16() },
    };
    if retryable(reply.status, &reply.body) {
        Err(Fail::Maybe { status: reply.status, body: reply.body, error })
    } else {
        Err(Fail::Stop(error))
    }
}

/// Up to [`RETRIES`] on this URL, 1 s then 4 s apart, stopping at
/// [`ITEM_CAP`] for the item. A non-transient error is not retried.
async fn polite(http: &reqwest::Client, url: &str, budget: &Budget) -> Result<Value, SourceError> {
    let mut attempt = 0u32;
    loop {
        if !budget.take() {
            return Err(budget.last());
        }
        let spent = budget.spent.load(Ordering::Relaxed);
        match once(http, url).await {
            Ok(v) => return Ok(v),
            Err(Fail::Stop(e)) => {
                budget.note(&e);
                return Err(e);
            }
            Err(Fail::Maybe { status, body, error }) => {
                budget.note(&error);
                match again(attempt, status, &body, spent) {
                    Again::Wait(d) => {
                        attempt += 1;
                        tokio::time::sleep(d).await;
                    }
                    Again::Next | Again::Stop => return Err(error),
                }
            }
        }
    }
}

fn hard_outage(e: &SourceError) -> bool {
    match e {
        SourceError::Unreachable(_) | SourceError::RateLimited { .. } | SourceError::Blocked(_) => true,
        SourceError::Http { status } if *status >= 500 => true,
        _ => false,
    }
}

impl Tidal {
    /// `path` of the searching instances.
    async fn ask(&self, path: String) -> Result<Value, SourceError> {
        self.ask_of(&self.instances, path).await
    }

    /// One item: a few polite retries on a transient answer, then the next
    /// instance, and no more than [`ITEM_CAP`] requests in all.
    async fn ask_of(&self, instances: &Instances, path: String) -> Result<Value, SourceError> {
        self.refresh().await;
        let budget = Arc::new(Budget::new());
        let (http, pace) = (&self.http, &self.pace);
        instances
            .call(|base| {
                let path = path.clone();
                let budget = Arc::clone(&budget);
                async move {
                    if budget.exhausted() {
                        return Err(budget.last());
                    }
                    pace.wait().await;
                    polite(http, &format!("{base}{path}"), &budget).await
                }
            })
            .await
    }

    /// The user's own instances, probed at most every [`UPTIME_EVERY`], so
    /// a dead one is skipped. The uptime list is that probe when it is followed.
    async fn health(&self) {
        if self.follow_uptime.load(Ordering::Relaxed) {
            return;
        }
        {
            let mut probed = self.probed.lock().await;
            if probed.is_some_and(|t| t.elapsed() < UPTIME_EVERY) {
                return;
            }
            *probed = Some(Instant::now());
        }
        for base in self.instances.urls() {
            if self.instances.paused(&base) {
                continue;
            }
            self.pace.wait().await;
            let r = probe_search(&self.http, &base).await;
            match &r {
                Ok(()) => self.instances.note_at(&base, &r),
                Err(e) if hard_outage(e) => {
                    log::info!("tidal: {base} is not healthy: {e}");
                    self.instances.note_at(&base, &r);
                    self.files.note_at(&base, &r);
                }
                Err(_) => {}
            }
        }
    }
}

async fn probe_search(http: &reqwest::Client, base: &str) -> Result<(), SourceError> {
    let reply = read(http, &format!("{base}/search/?s=a")).await?;
    let status = reqwest::StatusCode::from_u16(reply.status).unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if status.is_success() {
        return Ok(());
    }
    if is_daily_limit(status, &reply.body) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        return Err(SourceError::RateLimited { retry_after: Some(until_utc_midnight(now)) });
    }
    Err(SourceError::Http { status: reply.status })
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
            // Nothing it finds could be downloaded: no offers, and said so.
            self.refresh().await;
            self.health().await;
            if self.no_files.load(Ordering::Relaxed) {
                return Err(SourceError::NothingToFetch);
            }
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

    fn fetch<'a>(&'a self, offer: &'a Offer, want: &'a Want, dest: &'a Path, progress: Progress) -> BoxFuture<'a, Result<Fetched, SourceError>> {
        Box::pin(async move {
            self.refresh().await;
            self.health().await;
            if self.no_files.load(Ordering::Relaxed) {
                return Err(SourceError::NothingToFetch);
            }
            let tracks: Vec<(String, String)> = match offer.handle.strip_prefix("album:") {
                Some(album) => {
                    let v = self.ask(format!("/album/?id={}", enc(album))).await?;
                    let all: Vec<(String, String, String)> = find_array(&v, "items")
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|i| i.get("item").unwrap_or(i))
                        .filter(|t| playable(t))
                        .filter_map(|t| {
                            let id = s(t, "id")?;
                            let disc = t.get("volumeNumber").and_then(Value::as_u64).unwrap_or(1);
                            let no = t.get("trackNumber").and_then(Value::as_u64).unwrap_or(0);
                            let title = title_of(t)?;
                            Some((id, crate::http::track_file_name(disc as u32, no as u32, &title), title))
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
                    let answer = self.ask_of(&self.files, format!("/track/?id={}&quality=LOSSLESS", enc(&id))).await?;
                    let url = file_url(&answer)?;
                    let base = before.load(Ordering::Relaxed);
                    let step: Progress = Arc::new(move |done, _| progress(base + done, None));
                    let path = download(&self.http, &url, &dest.join(format!("{name}.{}", ext_of(&answer))), &step).await?;
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

    #[test]
    fn a_version_the_title_has_is_not_said_twice() {
        let v = serde_json::json!({"title": "Spider-Man (Deluxe Edition)", "version": "Deluxe Edition"});
        assert_eq!(title_of(&v).as_deref(), Some("Spider-Man (Deluxe Edition)"));
        let v = serde_json::json!({"title": "Avant", "version": "Remastered"});
        assert_eq!(title_of(&v).as_deref(), Some("Avant (Remastered)"));
    }

    /// The uptime list as it was on 2026-11-04 and 11-09: one instance
    /// searches, `streaming` is empty — no instance gives files, so none is
    /// asked for one; the monochrome.tf names are down; a default it does
    /// not name stays last.
    #[test]
    fn the_uptime_list_says_which_instances_to_ask() {
        let v = serde_json::json!({
            "lastUpdated": "2026-11-04T18:10:08.444Z",
            "api": [{"url": "https://monochrome-api.samidy.com", "version": "2.3"}],
            "streaming": [],
            "down": [{"url": "https://api.monochrome.tf", "status": 504, "error": "Track unreachable"}]
        });
        let defaults = ["https://monochrome-api.samidy.com/", "https://api.monochrome.tf", "https://other.example"];
        let (search, files) = uptime_lists(&v, &defaults);
        assert_eq!(search, ["https://monochrome-api.samidy.com", "https://other.example"]);
        assert_eq!(files, None, "no instance gives files: none is asked");
        let v = serde_json::json!({"api": ["https://a", "https://b"], "streaming": [{"url": "https://b"}]});
        let (search, files) = uptime_lists(&v, &[]);
        assert_eq!((search.len(), files), (2, Some(vec!["https://b".to_owned()])));
        // An older list without `streaming`: the searching ones give files.
        let v = serde_json::json!({"api": ["https://a"]});
        assert_eq!(uptime_lists(&v, &[]).1, Some(vec!["https://a".to_owned()]));
    }

    #[test]
    fn the_dead_fallback_is_gone() {
        assert!(!DEFAULT_INSTANCES.iter().any(|u| u.contains("tracks.monochrome.st")));
        assert!(!UPTIME.iter().any(|u| u.contains("jiffy-puffs")), "the dead uptime mirror is not asked");
    }

    /// 403 «Upstream API error» and a transient status are tried three
    /// times (waits of 1 s and 4 s), then the next instance. Six attempts
    /// for one item is the cap: a third instance is not asked.
    #[test]
    fn an_upstream_error_is_retried_a_few_times_then_capped() {
        let body = r#"{"detail":"Upstream API error"}"#;
        assert!(matches!(again(0, 403, body, 1), Again::Wait(d) if d == Duration::from_secs(1)));
        assert!(matches!(again(1, 403, body, 2), Again::Wait(d) if d == Duration::from_secs(4)));
        assert!(matches!(again(2, 403, body, 3), Again::Next), "the third try is the last on this instance");
        assert!(matches!(again(0, 504, "", 1), Again::Wait(_)));
        assert!(matches!(again(0, 521, "", 1), Again::Wait(_)));
        assert!(matches!(again(0, 202, "", 1), Again::Wait(_)));
        assert!(matches!(again(0, 0, "", 1), Again::Wait(_)), "a timeout is retried");
        assert!(matches!(again(0, 403, "forbidden", 1), Again::Next), "a 403 that is not the upstream error is not retried");
        assert!(matches!(again(0, 404, body, 1), Again::Next));
        assert!(matches!(again(0, 403, body, ITEM_CAP), Again::Stop));
        let mut spent = 0u32;
        let mut instances = 0u32;
        while spent < ITEM_CAP + 2 && instances < 5 {
            instances += 1;
            let mut attempt = 0u32;
            loop {
                spent += 1;
                match again(attempt, 403, body, spent) {
                    Again::Wait(_) => attempt += 1,
                    Again::Next => break,
                    Again::Stop => {
                        assert!(instances <= 2, "a third instance is not given the retries");
                        assert_eq!(spent, ITEM_CAP);
                        return;
                    }
                }
            }
            assert!(attempt + 1 <= RETRIES, "one instance is not tried more than {RETRIES} times");
        }
        panic!("the cap did not stop the item: spent {spent}, instances {instances}");
    }
}
