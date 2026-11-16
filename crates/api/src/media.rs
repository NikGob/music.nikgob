//! The files: a song's best local file as it is (HTTP Range: a player can
//! seek and resume), and an album's cover. Conversion to other formats
//! comes later (profiles).
//!
//! Only files inside the library, the masters folder and the folders the
//! host names ([`crate::ApiContext::media_roots`]) are served, with links
//! followed first: a library entry that is a link to a file elsewhere is
//! "not found", like a file the library does not have.

use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};

use nikgob_core::model::{ReleaseId, SourceKind, TrackId};
use nikgob_runtime::Resolver;

use crate::Shared;
use crate::error::{ApiError, query_value};
use crate::library::{blocking, db};

/// The file's BLAKE3 (hex) as it is now, with `?hash=1`: a paired device
/// checks the copy it fetched by it.
pub const HASH_HEADER: &str = "x-nikgob-blake3";
/// The file's extension (`flac`, `opus`…), for the copy's name.
pub const EXT_HEADER: &str = "x-nikgob-ext";

/// What a `Range` header asks of a file `size` bytes long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// The whole file (no header, one not understood, or several ranges).
    Full,
    /// Bytes `start..=end`.
    Part(u64, u64),
    /// Nothing of it can be given (416).
    Unsatisfiable,
}

/// Reads `Range: bytes=a-b`, `bytes=a-` and `bytes=-n` (the last n).
pub fn parse_range(header: Option<&str>, size: u64) -> Span {
    let Some(spec) = header.map(str::trim).and_then(|h| h.strip_prefix("bytes=")) else {
        return Span::Full;
    };
    if spec.contains(',') {
        return Span::Full;
    }
    let Some((a, b)) = spec.trim().split_once('-') else {
        return Span::Full;
    };
    let (a, b) = (a.trim(), b.trim());
    if size == 0 {
        return Span::Unsatisfiable;
    }
    let last = size - 1;
    match (a.is_empty(), b.is_empty()) {
        // The last n bytes.
        (true, false) => match b.parse::<u64>() {
            Ok(0) => Span::Unsatisfiable,
            Ok(n) => Span::Part(size.saturating_sub(n), last),
            Err(_) => Span::Full,
        },
        (false, _) => {
            let Ok(start) = a.parse::<u64>() else { return Span::Full };
            if start > last {
                return Span::Unsatisfiable;
            }
            let end = if b.is_empty() {
                last
            } else {
                match b.parse::<u64>() {
                    Ok(e) if e >= start => e.min(last),
                    _ => return Span::Full,
                }
            };
            Span::Part(start, end)
        }
        (true, true) => Span::Full,
    }
}

/// The content type of a file by its extension.
pub fn content_type(path: &FsPath) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    match ext.as_str() {
        "flac" => "audio/flac",
        "opus" | "ogg" | "oga" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" | "aac" | "alac" => "audio/mp4",
        "wav" => "audio/wav",
        "aif" | "aiff" => "audio/aiff",
        "webm" => "audio/webm",
        "wv" => "audio/x-wavpack",
        "ape" => "audio/x-ape",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

/// `path` with its links followed, if that is inside one of `roots`
/// (themselves with links followed).
pub fn inside(path: &FsPath, roots: &[PathBuf]) -> Option<PathBuf> {
    let real = std::fs::canonicalize(path).ok()?;
    roots.iter().any(|r| real.starts_with(r)).then_some(real)
}

/// The folders files are served from, links followed (a folder that is
/// not there is left out).
pub(crate) fn roots(shared: &Shared) -> Vec<PathBuf> {
    let (library, masters) = {
        let st = shared.rt.state();
        (st.prefs.library_dir.clone(), st.prefs.masters_dir.clone())
    };
    [library, masters]
        .into_iter()
        .chain(shared.media_roots.iter().cloned())
        .filter(|p| !p.as_os_str().is_empty())
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect()
}

/// Opens `path` if it is a file inside `roots`: by its real path, which is
/// checked again once open (a link put in between is refused).
pub(crate) fn open_inside(path: &FsPath, roots: &[PathBuf]) -> Option<(std::fs::File, PathBuf)> {
    let real = inside(path, roots)?;
    let file = std::fs::File::open(&real).ok()?;
    let still = std::fs::canonicalize(&real).ok()? == real;
    (still && file.metadata().ok()?.is_file()).then_some((file, real))
}

/// The open file `file` (named `path`, for its type) as the answer to
/// `headers` (its Range).
pub(crate) async fn serve_file(file: std::fs::File, path: &FsPath, headers: &HeaderMap) -> Result<Response, ApiError> {
    let gone = || ApiError::not_found("Файла нет");
    let mut file = tokio::fs::File::from_std(file);
    let size = file.metadata().await.map_err(|_| gone())?.len();
    let kind = HeaderValue::from_static(content_type(path));
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let (status, start, len) = match parse_range(range, size) {
        Span::Full => (StatusCode::OK, 0, size),
        Span::Part(a, b) => (StatusCode::PARTIAL_CONTENT, a, b - a + 1),
        Span::Unsatisfiable => {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return Ok(r);
        }
    };
    if start > 0 {
        file.seek(std::io::SeekFrom::Start(start)).await.map_err(|e| ApiError::internal(e.to_string()))?;
    }
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(file.take(len), 64 * 1024));
    let mut r = Response::new(body);
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, kind);
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(v) = HeaderValue::from_str(&format!("bytes {start}-{}/{size}", start + len - 1))
    {
        h.insert(header::CONTENT_RANGE, v);
    }
    Ok(r)
}

/// The song's best local file inside the served folders, open: available,
/// the best quality first.
fn best_file(shared: &Shared, track: TrackId) -> Result<Option<(std::fs::File, PathBuf)>, ApiError> {
    let sources = shared.lib.sources_of_track(track).map_err(db)?;
    let root = shared.rt.state().prefs.library_dir.clone();
    let resolver = Resolver::new(shared.lib.clone(), root);
    let roots = roots(shared);
    let mut local: Vec<_> = sources.into_iter().filter(|s| s.kind == SourceKind::LocalFile && s.available && s.trashed_at.is_none()).collect();
    local.sort_by_key(|s| (std::cmp::Reverse(s.is_active), std::cmp::Reverse(s.quality_score)));
    Ok(local.into_iter().find_map(|s| open_inside(&resolver.path(&s.locator), &roots)))
}

/// The BLAKE3 (hex) of the file at `path`.
fn blake3_of(path: &FsPath) -> std::io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(std::fs::File::open(path)?)?;
    Ok(h.finalize().to_hex().to_string())
}

/// `GET /v1/media/tracks/{id}/audio`.
#[utoipa::path(
    get,
    path = "/v1/media/tracks/{id}/audio",
    tag = "media",
    params(
        ("id" = String, Path, description = "id песни"),
        ("hash" = Option<String>, Query, description = "1: BLAKE3 файла в заголовке x-nikgob-blake3"),
    ),
    responses(
        (status = 200, description = "Файл как есть"),
        (status = 206, description = "Часть файла по Range"),
        (status = 404, body = crate::error::ErrorBody),
        (status = 416, description = "Range за концом файла"),
    ),
    security(("bearer" = []), ("token" = []))
)]
pub async fn audio(State(sh): State<Arc<Shared>>, Path(id): Path<String>, RawQuery(raw): RawQuery, headers: HeaderMap) -> Result<Response, ApiError> {
    let id: TrackId = id.parse().map_err(|_| ApiError::bad_request("Неверный id"))?;
    let hash = query_value(raw.as_deref(), "hash").is_some_and(|v| v == "1");
    let shared = sh.clone();
    let found = blocking(move || {
        let Some((file, path)) = best_file(&shared, id)? else {
            return Ok(None);
        };
        let sum = if hash { Some(blake3_of(&path).map_err(|e| ApiError::internal(e.to_string()))?) } else { None };
        Ok(Some((file, path, sum)))
    })
    .await?;
    let (file, path, sum) = found.ok_or_else(|| ApiError::not_found("Файла этой песни здесь нет"))?;
    let mut r = serve_file(file, &path, &headers).await?;
    if let Some(ext) = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
        && let Ok(v) = HeaderValue::from_str(&ext)
    {
        r.headers_mut().insert(EXT_HEADER, v);
    }
    if let Some(sum) = sum
        && let Ok(v) = HeaderValue::from_str(&sum)
    {
        r.headers_mut().insert(HASH_HEADER, v);
    }
    Ok(r)
}

/// Where an album's cover is: a file (open), or a service's url.
enum CoverAt {
    File(std::fs::File, PathBuf),
    Web(String),
}

/// `GET /v1/media/albums/{id}/cover`.
#[utoipa::path(
    get,
    path = "/v1/media/albums/{id}/cover",
    tag = "media",
    params(("id" = String, Path, description = "id альбома")),
    responses(
        (status = 200, description = "Картинка"),
        (status = 307, description = "Обложка сервиса: её адрес"),
        (status = 404, body = crate::error::ErrorBody),
    ),
    security(("bearer" = []), ("token" = []))
)]
pub async fn cover(State(sh): State<Arc<Shared>>, Path(id): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    let id: ReleaseId = id.parse().map_err(|_| ApiError::bad_request("Неверный id"))?;
    let shared = sh.clone();
    let at = blocking(move || {
        let root = shared.rt.state().prefs.library_dir.clone();
        let mut resolver = Resolver::new(shared.lib.clone(), root);
        let roots = roots(&shared);
        Ok(resolver.release_cover(id).and_then(|c| match c.web_url() {
            Some(url) => Some(CoverAt::Web(url.to_owned())),
            None => c.file().and_then(|p| open_inside(p, &roots)).map(|(f, p)| CoverAt::File(f, p)),
        }))
    })
    .await?;
    match at {
        Some(CoverAt::File(f, p)) => serve_file(f, &p, &headers).await,
        Some(CoverAt::Web(url)) => Ok(Redirect::temporary(&url).into_response()),
        None => Err(ApiError::not_found("Обложки нет")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 1000), Span::Full);
        assert_eq!(parse_range(Some("bytes=0-499"), 1000), Span::Part(0, 499));
        assert_eq!(parse_range(Some("bytes=500-"), 1000), Span::Part(500, 999));
        assert_eq!(parse_range(Some("bytes=-100"), 1000), Span::Part(900, 999));
        assert_eq!(parse_range(Some("bytes=-5000"), 1000), Span::Part(0, 999));
        assert_eq!(parse_range(Some("bytes=900-5000"), 1000), Span::Part(900, 999));
        assert_eq!(parse_range(Some("bytes=999-999"), 1000), Span::Part(999, 999));
        assert_eq!(parse_range(Some("bytes=1000-"), 1000), Span::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=-0"), 1000), Span::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Span::Unsatisfiable);
        assert_eq!(parse_range(Some("bytes=5-1"), 1000), Span::Full, "backwards: ignored");
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 1000), Span::Full, "several: the whole file");
        assert_eq!(parse_range(Some("items=0-1"), 1000), Span::Full);
        assert_eq!(parse_range(Some("bytes=x-"), 1000), Span::Full);
        assert_eq!(parse_range(Some("bytes=-"), 1000), Span::Full);
        assert_eq!(parse_range(Some(" bytes= 10 - 20 "), 1000), Span::Part(10, 20));
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(FsPath::new("a/b.FLAC")), "audio/flac");
        assert_eq!(content_type(FsPath::new("x.opus")), "audio/ogg");
        assert_eq!(content_type(FsPath::new("x.mp3")), "audio/mpeg");
        assert_eq!(content_type(FsPath::new("x.m4a")), "audio/mp4");
        assert_eq!(content_type(FsPath::new("cover.jpg")), "image/jpeg");
        assert_eq!(content_type(FsPath::new("noext")), "application/octet-stream");
    }

    /// A file is served whole, or the part asked for with its headers.
    #[tokio::test]
    async fn a_file_is_served_by_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.flac");
        std::fs::write(&path, (0u8..100).collect::<Vec<u8>>()).unwrap();
        let mut h = HeaderMap::new();
        let open = || std::fs::File::open(&path).unwrap();
        let full = serve_file(open(), &path, &h).await.unwrap();
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_LENGTH], "100");
        assert_eq!(full.headers()[header::CONTENT_TYPE], "audio/flac");
        h.insert(header::RANGE, "bytes=10-19".parse().unwrap());
        let part = serve_file(open(), &path, &h).await.unwrap();
        assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(part.headers()[header::CONTENT_RANGE], "bytes 10-19/100");
        assert_eq!(part.headers()[header::CONTENT_LENGTH], "10");
        let bytes = axum::body::to_bytes(part.into_body(), 1024).await.unwrap();
        assert_eq!(bytes.as_ref(), &(10u8..20).collect::<Vec<u8>>()[..]);
        h.insert(header::RANGE, "bytes=200-".parse().unwrap());
        let none = serve_file(open(), &path, &h).await.unwrap();
        assert_eq!(none.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(none.headers()[header::CONTENT_RANGE], "bytes */100");
        // The hash a paired device checks its copy by is the whole file's.
        assert_eq!(blake3_of(&path).unwrap(), blake3::hash(&(0u8..100).collect::<Vec<u8>>()).to_hex().to_string());
    }

    /// Only files inside the served folders, links followed: a link in the
    /// library to a file outside is not served, `..` does not get out.
    #[test]
    fn only_files_inside_the_folders_are_served() {
        let lib = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let song = lib.path().join("a.flac");
        std::fs::write(&song, b"x").unwrap();
        let secret = outside.path().join("secret.txt");
        std::fs::write(&secret, b"s").unwrap();
        let roots = vec![std::fs::canonicalize(lib.path()).unwrap()];
        assert!(open_inside(&song, &roots).is_some());
        assert!(open_inside(&secret, &roots).is_none());
        assert!(open_inside(&lib.path().join("..").join(outside.path().file_name().unwrap()).join("secret.txt"), &roots).is_none());
        assert!(open_inside(&lib.path().join("missing.flac"), &roots).is_none());
        assert!(open_inside(lib.path(), &roots).is_none(), "a folder is not a file");
        #[cfg(unix)]
        {
            let link = lib.path().join("b.flac");
            std::os::unix::fs::symlink(&secret, &link).unwrap();
            assert!(open_inside(&link, &roots).is_none(), "a link to outside");
        }
    }
}
