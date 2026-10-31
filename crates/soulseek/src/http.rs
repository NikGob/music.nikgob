//! The headless service's HTTP API (`nikgob-slsk` on a server or a
//! Raspberry Pi), which [`crate::RemoteClient`] speaks.
//!
//! Every request needs `Authorization: Bearer <token>`; without a token the
//! service only listens on the loopback address. Finished downloads stay in
//! the service's downloads folder until the player fetches and removes them.
//!
//! - `GET /api/status` → [`Status`]
//! - `POST /api/search` `{text, kind, wait_secs}` → `[SearchResult]`
//! - `POST /api/folder` `{user, folder}` → `[SearchResult]`
//! - `POST /api/downloads` [`SearchResult`] → [`DownloadInfo`];
//!   `GET /api/downloads`, `GET /api/downloads/{id}`, `DELETE /api/downloads/{id}`
//!   (a finished download's path is relative to the downloads folder, with `/`)
//! - `GET /api/files` → `[FileInfo]`; `GET /api/files/{path}` (with `Range:
//!   bytes=N-` to resume); `DELETE /api/files/{path}`
//! - `GET /api/share`, `PUT /api/share` [`ShareSettings`]

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncSeekExt as _;

use crate::{Client, Config, DownloadInfo, DownloadState, Error, QueryKind, SearchQuery, SearchResult, ShareSettings};

/// The settings file of the headless service (TOML).
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceConfig {
    pub server: String,
    pub username: String,
    pub password: String,
    pub listen_port: u16,
    /// Folders shared, read-only.
    pub share_folders: Vec<PathBuf>,
    pub downloads_dir: PathBuf,
    pub state_dir: Option<PathBuf>,
    pub share: ShareSettings,
    pub http: HttpConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpConfig {
    /// `address:port` of the API.
    pub listen: String,
    /// The bearer token the player sends; empty = loopback only.
    pub token: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self { listen: "127.0.0.1:5031".into(), token: String::new() }
    }
}

impl Default for ServiceConfig {
    fn default() -> Self {
        let c = Config::default();
        Self {
            server: c.server,
            username: String::new(),
            password: String::new(),
            listen_port: c.listen_port,
            share_folders: Vec::new(),
            downloads_dir: PathBuf::from("downloads"),
            state_dir: None,
            share: ShareSettings::default(),
            http: HttpConfig::default(),
        }
    }
}

impl std::fmt::Debug for ServiceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceConfig")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("listen_port", &self.listen_port)
            .field("share_folders", &self.share_folders)
            .field("downloads_dir", &self.downloads_dir)
            .field("http_listen", &self.http.listen)
            .finish_non_exhaustive()
    }
}

impl ServiceConfig {
    pub fn client_config(&self) -> Config {
        Config {
            server: self.server.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            listen_port: self.listen_port,
            share_folders: self.share_folders.clone(),
            share: self.share,
            state_dir: self.state_dir.clone(),
            ..Config::default()
        }
    }
}

/// A finished file in the service's downloads folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInfo {
    /// Relative to the downloads folder, with `/`.
    pub path: String,
    pub size: u64,
    /// Seconds since the epoch.
    pub modified: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SearchBody {
    pub text: String,
    #[serde(default)]
    pub kind: QueryKind,
    #[serde(default)]
    pub wait_secs: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FolderBody {
    pub user: String,
    pub folder: String,
}

struct AppState {
    client: Client,
    downloads: PathBuf,
    token: String,
}

type Shared = State<Arc<AppState>>;

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
        let code = match e {
            Error::NotLoggedIn => StatusCode::SERVICE_UNAVAILABLE,
            Error::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Error::Offline(_) | Error::Unreachable(_) => StatusCode::BAD_GATEWAY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(code, e.to_string())
    }
}

fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}

/// Constant-time comparison of the token.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn auth(State(st): Shared, req: Request, next: Next) -> Response {
    if !st.token.is_empty() {
        let given = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !same(given.as_bytes(), st.token.as_bytes()) {
            return (StatusCode::UNAUTHORIZED, "bad token").into_response();
        }
    }
    next.run(req).await
}

/// A path inside the downloads folder from its `/`-separated relative form;
/// None for anything that would leave it.
fn inside(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut p = root.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':']) {
            return None;
        }
        p.push(part);
    }
    // Belt and braces: nothing in it may be a root, prefix or parent.
    let tail = p.strip_prefix(root).ok()?;
    tail.components().all(|c| matches!(c, Component::Normal(_))).then_some(p)
}

fn relative(root: &Path, p: &Path) -> Option<String> {
    let tail = p.strip_prefix(root).ok()?;
    let parts: Vec<String> = tail.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A download as the API shows it: a finished file's path relative to the
/// downloads folder.
fn api_info(root: &Path, mut d: DownloadInfo) -> DownloadInfo {
    if let DownloadState::Done { path } = &d.state {
        d.state = DownloadState::Done { path: PathBuf::from(relative(root, path).unwrap_or_default()) };
    }
    d
}

async fn status(State(st): Shared) -> impl IntoResponse {
    axum::Json(st.client.status())
}

async fn search(State(st): Shared, axum::Json(b): axum::Json<SearchBody>) -> Result<impl IntoResponse, ApiError> {
    let wait = Duration::from_secs(b.wait_secs.unwrap_or(8).clamp(1, 60));
    let q = SearchQuery { text: b.text, kind: b.kind };
    Ok(axum::Json(st.client.search(&q, wait).await?))
}

async fn folder(State(st): Shared, axum::Json(b): axum::Json<FolderBody>) -> Result<impl IntoResponse, ApiError> {
    Ok(axum::Json(st.client.folder(&b.user, &b.folder).await?))
}

async fn start_download(State(st): Shared, axum::Json(r): axum::Json<SearchResult>) -> Result<impl IntoResponse, ApiError> {
    let d = st.client.download(&r, &st.downloads);
    let info = st.client.downloads().into_iter().find(|i| i.id == d.id()).ok_or_else(not_found)?;
    Ok(axum::Json(api_info(&st.downloads, info)))
}

async fn list_downloads(State(st): Shared) -> impl IntoResponse {
    let v: Vec<DownloadInfo> = st.client.downloads().into_iter().map(|d| api_info(&st.downloads, d)).collect();
    axum::Json(v)
}

async fn get_download(State(st): Shared, UrlPath(id): UrlPath<u64>) -> Result<impl IntoResponse, ApiError> {
    let info = st.client.downloads().into_iter().find(|i| i.id == id).ok_or_else(not_found)?;
    Ok(axum::Json(api_info(&st.downloads, info)))
}

async fn cancel_download(State(st): Shared, UrlPath(id): UrlPath<u64>) -> Result<impl IntoResponse, ApiError> {
    st.client.download_by_id(id).ok_or_else(not_found)?.cancel();
    Ok(StatusCode::NO_CONTENT)
}

async fn list_files(State(st): Shared) -> Result<impl IntoResponse, ApiError> {
    let root = st.downloads.clone();
    let files = tokio::task::spawn_blocking(move || {
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let Ok(ft) = e.file_type() else { continue };
                let p = e.path();
                if ft.is_dir() {
                    stack.push(p);
                } else if ft.is_file() && p.extension().is_none_or(|x| x != "part") {
                    let Ok(md) = e.metadata() else { continue };
                    let Some(path) = relative(&root, &p) else { continue };
                    let modified =
                        md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
                    out.push(FileInfo { path, size: md.len(), modified });
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    })
    .await
    .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(axum::Json(files))
}

fn range_start(headers: &HeaderMap) -> Option<u64> {
    let v = headers.get(header::RANGE)?.to_str().ok()?;
    let (start, end) = v.strip_prefix("bytes=")?.split_once('-')?;
    end.is_empty().then(|| start.trim().parse().ok()).flatten()
}

async fn get_file(State(st): Shared, UrlPath(rel): UrlPath<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let p = inside(&st.downloads, &rel).ok_or_else(not_found)?;
    let mut f = tokio::fs::File::open(&p).await.map_err(|_| not_found())?;
    let size = f.metadata().await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?.len();
    let start = range_start(&headers).filter(|s| *s <= size);
    if let Some(s) = start {
        f.seek(std::io::SeekFrom::Start(s)).await.map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(f, 64 * 1024));
    let mut r = Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CONTENT_LENGTH, size - start.unwrap_or(0));
    if let Some(s) = start {
        r = r
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {s}-{}/{size}", size.saturating_sub(1)));
    }
    r.body(body).map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn delete_file(State(st): Shared, UrlPath(rel): UrlPath<String>) -> Result<impl IntoResponse, ApiError> {
    let p = inside(&st.downloads, &rel).ok_or_else(not_found)?;
    tokio::fs::remove_file(&p).await.map_err(|_| not_found())?;
    // Empty folders it leaves go too (never the downloads folder itself).
    let mut dir = p.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == st.downloads || tokio::fs::remove_dir(&d).await.is_err() {
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn get_share(State(st): Shared) -> impl IntoResponse {
    axum::Json(st.client.share_settings())
}

async fn put_share(State(st): Shared, axum::Json(s): axum::Json<ShareSettings>) -> impl IntoResponse {
    st.client.set_share(s);
    StatusCode::NO_CONTENT
}

/// The API's routes for `client`, with finished files under `downloads`.
pub fn router(client: Client, downloads: PathBuf, token: String) -> Router {
    let st = Arc::new(AppState { client, downloads, token });
    Router::new()
        .route("/api/status", get(status))
        .route("/api/search", post(search))
        .route("/api/folder", post(folder))
        .route("/api/downloads", get(list_downloads).post(start_download))
        .route("/api/downloads/{id}", get(get_download).delete(cancel_download))
        .route("/api/files", get(list_files))
        .route("/api/files/{*path}", get(get_file).delete(delete_file))
        .route("/api/share", get(get_share).put(put_share))
        .layer(middleware::from_fn_with_state(st.clone(), auth))
        .with_state(st)
}

/// Serves the API on `listen` until `stop` resolves. Refuses to listen on
/// anything but loopback without a token.
pub async fn serve(
    client: Client,
    downloads: PathBuf,
    listen: &str,
    token: String,
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<SocketAddr, Error> {
    let addr: SocketAddr = listen.parse().map_err(|_| Error::Config(format!("bad HTTP address {listen}")))?;
    if token.is_empty() && !addr.ip().is_loopback() {
        return Err(Error::Config("an HTTP token is needed to listen beyond this machine".into()));
    }
    std::fs::create_dir_all(&downloads)?;
    let downloads = std::fs::canonicalize(&downloads)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let bound = listener.local_addr()?;
    let app = router(client, downloads, token);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(stop).await {
            log::warn!("soulseek http: {e}");
        }
    });
    Ok(bound)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside() {
        let root = Path::new("/srv/dl");
        assert_eq!(inside(root, "Album/01.flac"), Some(root.join("Album").join("01.flac")));
        assert!(inside(root, "../etc/passwd").is_none());
        assert!(inside(root, "Album/../../x").is_none());
        assert!(inside(root, "/abs").is_none());
        assert!(inside(root, "C:/x").is_none());
        assert!(inside(root, "a\\..\\b").is_none());
        assert_eq!(relative(root, &root.join("A").join("b.flac")).as_deref(), Some("A/b.flac"));
    }

    #[test]
    fn range_header() {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=100-".parse().unwrap());
        assert_eq!(range_start(&h), Some(100));
        h.insert(header::RANGE, "bytes=1-5".parse().unwrap());
        assert_eq!(range_start(&h), None);
    }

    #[test]
    fn service_config_reads_toml() {
        let c: ServiceConfig = toml::from_str(
            "username = \"me\"\npassword = \"pw\"\nshare_folders = [\"/srv/music\"]\n[share]\nenabled = true\nupload_limit_kbps = 500\nupload_slots = 3\n[http]\nlisten = \"0.0.0.0:5031\"\ntoken = \"t\"\n",
        )
        .unwrap();
        assert_eq!(c.share.upload_limit_kbps, 500);
        assert_eq!(c.client_config().share_folders, vec![PathBuf::from("/srv/music")]);
        assert!(!format!("{c:?}").contains("pw"));
    }
}
