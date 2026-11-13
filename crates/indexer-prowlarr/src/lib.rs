//! [`Indexer`] implementation on top of Prowlarr's native API
//! (`GET /api/v1/search`, the audio categories, `X-Api-Key` header).
//!
//! A release is fetched from its `downloadUrl`; when that fails (the
//! tracker answers 5xx through Prowlarr, an HTML page instead of a
//! .torrent) its magnet is used: `magnetUrl`, which some indexers (nyaa
//! and other public ones) give as a Prowlarr link that redirects to the
//! magnet, else one made from the info hash.

use std::sync::Arc;
use std::time::Duration;

use nikgob_core::indexer::{
    BoxFuture, Candidate, CandidateHandle, Indexer, IndexerError, SearchKind, SearchQuery,
    TorrentPayload,
};
use nikgob_core::settings::{ProwlarrSettings, Settings};
use reqwest::header::LOCATION;
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use url::Url;

const AUDIO: u32 = 3000;
const AUDIO_LOSSLESS: u32 = 3040;
/// Every audio category of the Newznab standard but audiobooks and video:
/// indexers map their own categories onto these (nyaa's "Audio - Lossy"
/// onto 3010, RuTracker's soundtracks onto 3000 or 3050), and an indexer
/// that maps only onto a child is not found by the parent alone.
const AUDIO_ANY: [u32; 5] = [AUDIO, 3010, AUDIO_LOSSLESS, 3050, 3060];

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Prowlarr fans out to every tracker and answers only when the slowest is done.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_REDIRECTS: usize = 5;
const MAX_TORRENT_BYTES: usize = 16 << 20;

#[derive(Clone)]
pub struct Prowlarr {
    inner: Arc<Inner>,
}

struct Inner {
    client: Client,
    /// Always ends with `/`, so `join` keeps a reverse-proxy path prefix.
    base: Url,
    api_key: String,
}

impl std::fmt::Debug for Prowlarr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prowlarr")
            .field("base", &self.inner.base.as_str())
            .finish_non_exhaustive()
    }
}

impl Prowlarr {
    pub fn new(settings: &ProwlarrSettings) -> Result<Self, IndexerError> {
        let raw = settings.url.trim();
        if raw.is_empty() || settings.api_key.trim().is_empty() {
            return Err(IndexerError::NotConfigured);
        }
        let mut base = Url::parse(raw)
            .map_err(|e| IndexerError::Parse(format!("invalid Prowlarr URL: {e}")))?;
        if !matches!(base.scheme(), "http" | "https") || base.host().is_none() {
            return Err(IndexerError::Parse(
                "Prowlarr URL must be http(s) with a host".into(),
            ));
        }
        base.set_query(None);
        base.set_fragment(None);
        if !base.path().ends_with('/') {
            let path = format!("{}/", base.path());
            base.set_path(&path);
        }
        // Redirects are handled by hand: a download may answer with a `magnet:` Location.
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("nikgob-music/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| IndexerError::Unreachable(e.without_url().to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                client,
                base,
                api_key: settings.api_key.trim().to_owned(),
            }),
        })
    }

    pub fn from_settings(settings: &Settings) -> Result<Self, IndexerError> {
        let prowlarr = settings
            .indexer
            .prowlarr
            .as_ref()
            .ok_or(IndexerError::NotConfigured)?;
        Self::new(prowlarr)
    }
}

impl Indexer for Prowlarr {
    fn name(&self) -> &str {
        "Prowlarr"
    }

    fn search(&self, query: &SearchQuery) -> BoxFuture<'_, Result<Vec<Candidate>, IndexerError>> {
        let inner = self.inner.clone();
        let query = query.clone();
        Box::pin(async move { inner.search(&query).await })
    }

    fn fetch_torrent(
        &self,
        candidate: &Candidate,
    ) -> BoxFuture<'_, Result<TorrentPayload, IndexerError>> {
        let inner = self.inner.clone();
        let handle = candidate.handle.clone();
        Box::pin(async move { inner.fetch(&handle).await })
    }

    fn check(&self) -> BoxFuture<'_, Result<usize, IndexerError>> {
        let inner = self.inner.clone();
        Box::pin(async move { inner.check().await })
    }
}

/// Subset of Prowlarr's `IndexerResource`: whether it is switched on.
#[derive(Deserialize)]
struct IndexerInfo {
    #[serde(default)]
    enable: bool,
}

/// Subset of Prowlarr's `ReleaseResource` (camelCase JSON) we care about.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Release {
    guid: Option<String>,
    title: Option<String>,
    indexer: Option<String>,
    indexer_id: Option<i64>,
    size: Option<i64>,
    seeders: Option<i64>,
    leechers: Option<i64>,
    publish_date: Option<String>,
    #[serde(default)]
    categories: Vec<ReleaseCategory>,
    info_url: Option<String>,
    info_hash: Option<String>,
    download_url: Option<String>,
    magnet_url: Option<String>,
    protocol: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseCategory {
    id: u32,
}

/// What `fetch_torrent` needs; lives inside [`CandidateHandle`] as JSON.
#[derive(Serialize, Deserialize)]
struct Handle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    download_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    magnet_url: Option<String>,
    /// For a magnet when neither link works.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    info_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn count(n: Option<i64>) -> Option<u32> {
    n.and_then(|n| u32::try_from(n).ok())
}

impl Release {
    fn into_candidate(self) -> Option<Candidate> {
        if !self.protocol.as_deref()?.eq_ignore_ascii_case("torrent") {
            return None;
        }
        let download_url = non_empty(self.download_url);
        let magnet_url = non_empty(self.magnet_url);
        let guid = non_empty(self.guid).or_else(|| non_empty(self.info_hash.clone()))?;
        let id = match self.indexer_id {
            Some(indexer_id) => format!("{indexer_id}:{guid}"),
            None => guid,
        };
        let handle = serde_json::to_string(&Handle {
            download_url,
            magnet_url,
            info_hash: non_empty(self.info_hash.clone()),
            title: non_empty(self.title.clone()),
        })
        .ok()?;
        Some(Candidate {
            id,
            title: self.title.unwrap_or_default(),
            indexer_name: self.indexer.unwrap_or_default(),
            size_bytes: self.size.and_then(|n| u64::try_from(n).ok()),
            seeders: count(self.seeders),
            leechers: count(self.leechers),
            published: non_empty(self.publish_date),
            categories: self.categories.into_iter().map(|c| c.id).collect(),
            info_url: non_empty(self.info_url),
            info_hash: non_empty(self.info_hash),
            handle: CandidateHandle(handle),
        })
    }
}

impl Inner {
    fn endpoint(&self, path: &str) -> Url {
        self.base
            .join(path)
            .expect("static path joins onto a valid base")
    }

    fn same_origin(&self, url: &Url) -> bool {
        url.origin() == self.base.origin()
    }

    /// The API key goes only to our own Prowlarr, never to hosts we were redirected to.
    fn get(&self, url: &Url) -> reqwest::RequestBuilder {
        let req = self.client.get(url.clone());
        if self.same_origin(url) {
            req.header("X-Api-Key", &self.api_key)
        } else {
            req
        }
    }

    /// `GET api/v1/indexer`: the key works and this many trackers are on.
    async fn check(&self) -> Result<usize, IndexerError> {
        let url = self.endpoint("api/v1/indexer");
        let resp = self
            .get(&url)
            .timeout(CONNECT_TIMEOUT)
            .send()
            .await
            .map_err(unreachable)?;
        let resp = check_status(resp)?;
        let list: Vec<IndexerInfo> = resp
            .json()
            .await
            .map_err(|e| IndexerError::Parse(e.without_url().to_string()))?;
        Ok(list.iter().filter(|i| i.enable).count())
    }

    async fn search(&self, query: &SearchQuery) -> Result<Vec<Candidate>, IndexerError> {
        let mut url = self.endpoint("api/v1/search");
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("query", &query.text);
            q.append_pair("type", "search");
            match query.kind {
                SearchKind::Lossless => {
                    q.append_pair("categories", &AUDIO_LOSSLESS.to_string());
                }
                SearchKind::Any => {
                    for c in AUDIO_ANY {
                        q.append_pair("categories", &c.to_string());
                    }
                }
            }
            if let Some(limit) = query.limit {
                q.append_pair("limit", &limit.to_string());
            }
        }
        log::debug!("prowlarr search: {}", redact(&url));
        let resp = self
            .get(&url)
            .timeout(SEARCH_TIMEOUT)
            .send()
            .await
            .map_err(unreachable)?;
        let resp = check_status(resp)?;
        let releases: Vec<Release> = resp
            .json()
            .await
            .map_err(|e| IndexerError::Parse(e.without_url().to_string()))?;
        Ok(releases
            .into_iter()
            .filter_map(Release::into_candidate)
            .collect())
    }

    async fn fetch(&self, handle: &CandidateHandle) -> Result<TorrentPayload, IndexerError> {
        let handle: Handle = serde_json::from_str(&handle.0)
            .map_err(|e| IndexerError::Parse(format!("bad candidate handle: {e}")))?;
        let mut last = None;
        if let Some(download_url) = non_empty(handle.download_url) {
            let url = Url::parse(&download_url)
                .map_err(|e| IndexerError::Parse(format!("bad download URL: {e}")))?;
            match self.download(url).await {
                Ok(p) => return Ok(p),
                // A wrong key is wrong for the magnet link too.
                Err(e @ IndexerError::Unauthorized) => return Err(e),
                Err(e) => {
                    log::info!("prowlarr: the .torrent did not come ({e}), trying the magnet");
                    last = Some(e);
                }
            }
        }
        match non_empty(handle.magnet_url) {
            Some(magnet) if is_magnet(&magnet) => return Ok(TorrentPayload::Magnet(magnet)),
            // A Prowlarr link that answers with the magnet (or the file).
            Some(link) if link.starts_with("http://") || link.starts_with("https://") => {
                let url = Url::parse(&link).map_err(|e| IndexerError::Parse(format!("bad magnet link: {e}")))?;
                match self.download(url).await {
                    Ok(p) => return Ok(p),
                    Err(e @ IndexerError::Unauthorized) => return Err(e),
                    Err(e) => last = Some(e),
                }
            }
            Some(_) => last = Some(IndexerError::Parse("magnetUrl is not a magnet link".into())),
            None => {}
        }
        if let Some(magnet) = handle.info_hash.as_deref().and_then(|h| magnet_of(h, handle.title.as_deref())) {
            return Ok(TorrentPayload::Magnet(magnet));
        }
        Err(last.unwrap_or(IndexerError::NothingToFetch))
    }

    async fn download(&self, mut url: Url) -> Result<TorrentPayload, IndexerError> {
        for _ in 0..=MAX_REDIRECTS {
            log::debug!("prowlarr fetch: {}", redact(&url));
            let resp = self
                .get(&url)
                .timeout(FETCH_TIMEOUT)
                .send()
                .await
                .map_err(unreachable)?;
            let status = resp.status();
            if status.is_redirection() {
                let location = resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .map(str::trim)
                    .ok_or_else(|| IndexerError::Parse("redirect without Location".into()))?;
                if is_magnet(location) {
                    return Ok(TorrentPayload::Magnet(location.to_owned()));
                }
                url = url
                    .join(location)
                    .map_err(|e| IndexerError::Parse(format!("bad redirect target: {e}")))?;
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(IndexerError::Parse("redirect to unsupported scheme".into()));
                }
                continue;
            }
            let bytes = read_capped(check_status(resp)?, MAX_TORRENT_BYTES).await?;
            // A .torrent is a bencoded dictionary; anything else is an HTML error page.
            if bytes.first() != Some(&b'd') {
                return Err(IndexerError::Parse(
                    "response is not a .torrent file".into(),
                ));
            }
            return Ok(TorrentPayload::TorrentFile(bytes));
        }
        Err(IndexerError::Parse("too many redirects".into()))
    }
}

/// A magnet link of a v1 info hash (40 hex digits or 32 base32 letters).
fn magnet_of(hash: &str, title: Option<&str>) -> Option<String> {
    let hash = hash.trim();
    let ok = (hash.len() == 40 && hash.chars().all(|c| c.is_ascii_hexdigit()))
        || (hash.len() == 32 && hash.chars().all(|c| c.is_ascii_alphanumeric()));
    if !ok {
        return None;
    }
    let mut out = format!("magnet:?xt=urn:btih:{}", hash.to_lowercase());
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        out.push_str("&dn=");
        out.extend(url::form_urlencoded::byte_serialize(t.as_bytes()));
    }
    Some(out)
}

fn is_magnet(s: &str) -> bool {
    s.as_bytes()
        .get(..7)
        .is_some_and(|p| p.eq_ignore_ascii_case(b"magnet:"))
}

/// The body read chunk by chunk, refused as soon as it passes `max` bytes
/// (a declared length over it is refused before reading anything).
async fn read_capped(mut resp: Response, max: usize) -> Result<Vec<u8>, IndexerError> {
    let too_large = || IndexerError::Parse("torrent file is too large".into());
    if resp.content_length().is_some_and(|n| n > max as u64) {
        return Err(too_large());
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(unreachable)? {
        if out.len() + chunk.len() > max {
            return Err(too_large());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

fn unreachable(e: reqwest::Error) -> IndexerError {
    // `without_url`: request URLs may carry `apikey=`.
    IndexerError::Unreachable(e.without_url().to_string())
}

fn check_status(resp: Response) -> Result<Response, IndexerError> {
    match resp.status() {
        s if s.is_success() => Ok(resp),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(IndexerError::Unauthorized),
        s => Err(IndexerError::Http { status: s.as_u16() }),
    }
}

/// URL for logs with credential-like query values masked.
fn redact(url: &Url) -> String {
    if url.query().is_none() {
        return url.to_string();
    }
    let mut out = url.clone();
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let secret = k.eq_ignore_ascii_case("apikey") || k.eq_ignore_ascii_case("api_key");
            (
                k.into_owned(),
                if secret { "***".into() } else { v.into_owned() },
            )
        })
        .collect();
    out.query_pairs_mut().clear().extend_pairs(pairs);
    out.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnets_are_made_of_hashes_only() {
        let h = "0123456789ABCDEF0123456789abcdef01234567";
        assert_eq!(
            magnet_of(h, Some("A & B")).unwrap(),
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=A+%26+B"
        );
        assert!(magnet_of("not-a-hash", None).is_none());
    }

    #[test]
    fn is_magnet_takes_any_text() {
        assert!(is_magnet("MAGNET:?xt=urn:btih:abc"));
        assert!(!is_magnet("magnet"));
        // Byte 7 falls inside the two-byte "é": slicing the str would panic.
        assert!(!is_magnet("aaaaaa\u{e9}"));
        assert!(!is_magnet("\u{430}\u{431}\u{432}\u{433}"));
    }

    #[test]
    fn redact_masks_apikey_only() {
        let url = Url::parse("http://h/1/download?apikey=SECRET&file=a%20b").unwrap();
        let s = redact(&url);
        assert!(!s.contains("SECRET"));
        assert!(s.contains("apikey=***"));
        assert!(s.contains("file=a+b"));
    }

    #[test]
    fn base_path_prefix_is_kept() {
        let p = Prowlarr::new(&ProwlarrSettings {
            url: "https://example.com/prowlarr".into(),
            api_key: "k".into(),
        })
        .unwrap();
        assert_eq!(
            p.inner.endpoint("api/v1/search").as_str(),
            "https://example.com/prowlarr/api/v1/search"
        );
    }

    #[test]
    fn debug_has_no_key() {
        let p = Prowlarr::new(&ProwlarrSettings {
            url: "http://h:9696".into(),
            api_key: "topsecret".into(),
        })
        .unwrap();
        assert!(!format!("{p:?}").contains("topsecret"));
    }
}
