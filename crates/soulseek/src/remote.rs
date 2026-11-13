//! The player's side of a headless `nikgob-slsk`: the same calls as the
//! embedded client, over the service's HTTP API. A download runs on the
//! service; once it is done the file is fetched here (resuming a partial
//! fetch) and removed there.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt as _;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::http::{FileInfo, FolderBody, SearchBody};
use crate::transfer::{MAX_FILE_BYTES, finish_into, sanitize};
use crate::{Download, DownloadInfo, DownloadState, Error, SearchQuery, SearchResult, ShareSettings, Status};

const POLL: Duration = Duration::from_secs(1);
/// A poll of a download (or its start) answers within this.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
/// No byte of a fetch for this long: the service stalled.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct RemoteClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

impl std::fmt::Debug for RemoteClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteClient").field("base", &self.base).finish_non_exhaustive()
    }
}

fn remote(e: impl std::fmt::Display) -> Error {
    Error::Remote(e.to_string())
}

/// `a/b c.flac` with each part escaped for a URL path.
fn url_path(rel: &str) -> String {
    rel.split('/')
        .map(|part| {
            part.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

impl RemoteClient {
    /// `base` like `http://pi.local:5031`; `token` as in the service's
    /// settings.
    pub fn new(base: &str, token: &str) -> Result<Self, Error> {
        let base = base.trim().trim_end_matches('/').to_owned();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(Error::Config(format!("not an http(s) address: {base}")));
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(remote)?;
        Ok(Self { base, token: token.to_owned(), http })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let r = self.http.request(method, format!("{}{path}", self.base));
        if self.token.is_empty() { r } else { r.bearer_auth(&self.token) }
    }

    async fn send<T: DeserializeOwned>(&self, r: reqwest::RequestBuilder) -> Result<T, Error> {
        let resp = r.send().await.map_err(remote)?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(match status.as_u16() {
                503 => Error::NotLoggedIn,
                504 => Error::Timeout,
                _ => Error::Remote(format!("{status}: {text}")),
            });
        }
        resp.json().await.map_err(remote)
    }

    async fn send_empty(&self, r: reqwest::RequestBuilder) -> Result<(), Error> {
        let resp = r.send().await.map_err(remote)?;
        if resp.status().is_success() { Ok(()) } else { Err(Error::Remote(resp.status().to_string())) }
    }

    pub async fn status(&self) -> Result<Status, Error> {
        self.send(self.req(reqwest::Method::GET, "/api/status")).await
    }

    pub async fn search(&self, q: &SearchQuery, wait: Duration) -> Result<Vec<SearchResult>, Error> {
        let body = SearchBody { text: q.text.clone(), kind: q.kind, wait_secs: Some(wait.as_secs().max(1)) };
        let r = self.req(reqwest::Method::POST, "/api/search").json(&body).timeout(wait + Duration::from_secs(30));
        self.send(r).await
    }

    pub async fn folder(&self, user: &str, folder: &str) -> Result<Vec<SearchResult>, Error> {
        let body = FolderBody { user: user.to_owned(), folder: folder.to_owned() };
        self.send(self.req(reqwest::Method::POST, "/api/folder").json(&body)).await
    }

    pub async fn downloads(&self) -> Result<Vec<DownloadInfo>, Error> {
        self.send(self.req(reqwest::Method::GET, "/api/downloads")).await
    }

    pub async fn uploads(&self) -> Result<Vec<crate::UploadInfo>, Error> {
        self.send(self.req(reqwest::Method::GET, "/api/uploads")).await
    }

    pub async fn files(&self) -> Result<Vec<FileInfo>, Error> {
        self.send(self.req(reqwest::Method::GET, "/api/files")).await
    }

    pub async fn share(&self) -> Result<ShareSettings, Error> {
        self.send(self.req(reqwest::Method::GET, "/api/share")).await
    }

    pub async fn set_share(&self, s: ShareSettings) -> Result<(), Error> {
        self.send_empty(self.req(reqwest::Method::PUT, "/api/share").json(&s)).await
    }

    /// Fetches a finished file (`rel` as [`Self::files`] lists it) to
    /// `local`, resuming from a `.part` there; a file already at `local`
    /// stays and the fetched one takes a free name next to it, which is
    /// returned.
    pub async fn fetch(&self, rel: &str, local: &Path, progress: Option<&watch::Sender<DownloadState>>) -> Result<PathBuf, Error> {
        if let Some(d) = local.parent() {
            tokio::fs::create_dir_all(d).await?;
        }
        let part = PathBuf::from(format!("{}.part", local.display()));
        let have = tokio::fs::metadata(&part).await.map_or(0, |m| m.len());
        let mut r = self.req(reqwest::Method::GET, &format!("/api/files/{}", url_path(rel)));
        if have > 0 {
            r = r.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut resp = tokio::time::timeout(READ_TIMEOUT, r.send()).await.map_err(|_| Error::Timeout)?.map_err(remote)?;
        if !resp.status().is_success() {
            return Err(Error::Remote(resp.status().to_string()));
        }
        let resumed = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        let total = resp.content_length().unwrap_or(0) + if resumed { have } else { 0 };
        let too_large = || Error::Remote("the file is too large".into());
        if total > MAX_FILE_BYTES {
            return Err(too_large());
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&part)
            .await?;
        let mut got = if resumed { have } else { 0 };
        while let Some(chunk) = tokio::time::timeout(READ_TIMEOUT, resp.chunk()).await.map_err(|_| Error::Timeout)?.map_err(remote)? {
            got += chunk.len() as u64;
            if got > MAX_FILE_BYTES {
                drop(file);
                let _ = tokio::fs::remove_file(&part).await;
                return Err(too_large());
            }
            file.write_all(&chunk).await?;
            if let Some(p) = progress {
                p.send_replace(DownloadState::Transferring { received: got, size: total, bytes_per_sec: 0 });
            }
        }
        file.flush().await?;
        drop(file);
        if total > 0 && got != total {
            return Err(Error::Remote(format!("fetched {got} of {total} bytes")));
        }
        let dir = local.parent().unwrap_or(Path::new("."));
        let name = local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(finish_into(&part, dir, &name).await?)
    }

    /// Removes a fetched file from the service.
    pub async fn remove(&self, rel: &str) -> Result<(), Error> {
        self.send_empty(self.req(reqwest::Method::DELETE, &format!("/api/files/{}", url_path(rel))).timeout(POLL_TIMEOUT)).await
    }

    /// Has the service download `r`, then brings the file into `dir` here.
    pub async fn download(&self, r: &SearchResult, dir: &Path) -> Result<Download, Error> {
        let info: DownloadInfo = self.send(self.req(reqwest::Method::POST, "/api/downloads").json(r).timeout(POLL_TIMEOUT)).await?;
        let (tx, rx) = watch::channel(info.state.clone());
        let cancel = CancellationToken::new();
        let me = self.clone();
        let dir = dir.to_path_buf();
        let stop = cancel.clone();
        let id = info.id;
        tokio::spawn(async move {
            let end = tokio::select! {
                end = me.follow(id, &dir, &tx) => end,
                // Whatever it was doing (a poll, the fetch) stops here; a
                // partial fetch stays for a resume.
                _ = stop.cancelled() => {
                    let del = me.req(reqwest::Method::DELETE, &format!("/api/downloads/{id}")).timeout(POLL_TIMEOUT);
                    let _ = me.send_empty(del).await;
                    Err(Error::Cancelled)
                }
            };
            if let Err(e) = end {
                let reason = if stop.is_cancelled() { "Cancelled".to_owned() } else { e.to_string() };
                tx.send_replace(DownloadState::Failed { reason });
            }
        });
        Ok(Download::new(id, rx, cancel))
    }

    async fn follow(&self, id: u64, dir: &Path, tx: &watch::Sender<DownloadState>) -> Result<(), Error> {
        let mut misses = 0;
        let rel = loop {
            tokio::time::sleep(POLL).await;
            let poll = self.req(reqwest::Method::GET, &format!("/api/downloads/{id}")).timeout(POLL_TIMEOUT);
            let info: DownloadInfo = match self.send(poll).await {
                Ok(i) => i,
                // The link to the service can drop for a while.
                Err(e) if misses < 60 => {
                    log::debug!("soulseek remote: download {id}: {e}");
                    misses += 1;
                    continue;
                }
                Err(e) => return Err(e),
            };
            misses = 0;
            match info.state {
                DownloadState::Done { path } => break path.to_string_lossy().replace('\\', "/"),
                DownloadState::Failed { reason } => {
                    tx.send_replace(DownloadState::Failed { reason });
                    return Ok(());
                }
                s => {
                    tx.send_replace(s);
                }
            }
        };
        let mut local = dir.to_path_buf();
        for part in rel.split('/') {
            local.push(sanitize(part));
        }
        let local = self.fetch(&rel, &local, Some(tx)).await?;
        if let Err(e) = self.remove(&rel).await {
            log::debug!("soulseek remote: cannot remove {rel} on the service: {e}");
        }
        tx.send_replace(DownloadState::Done { path: local });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_paths_are_escaped() {
        assert_eq!(url_path("Album 1/01 a#b.flac"), "Album%201/01%20a%23b.flac");
        assert_eq!(url_path("勘"), "%E5%8B%98");
    }

    #[test]
    fn only_http_addresses() {
        assert!(RemoteClient::new("pi.local:5031", "").is_err());
        assert!(RemoteClient::new("http://pi.local:5031/", "t").is_ok());
    }
}
