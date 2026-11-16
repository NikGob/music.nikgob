//! «Добавить устройство → Установить»: a Linux machine (a Raspberry Pi, a
//! server) installs the headless player from this computer with the line
//! the window shows ([`nikgob_core::releases::install_command`]):
//!
//! ```text
//! curl -fsSL "http://<this computer>:<port>/install/nikgobd.sh?k=<key>" -o /tmp/nikgobd-install.sh \
//!   && echo "<sha256>  /tmp/nikgobd-install.sh" | sha256sum -c --quiet - \
//!   && sh /tmp/nikgobd-install.sh <ticket>
//! ```
//!
//! `GET /install/nikgobd.sh` is the POSIX sh script
//! ([`nikgob_core::releases::install_script`]) with this computer's address,
//! the key and the checksum of each build there in it: it downloads the
//! build for the machine (`GET /install/nikgobd-<version>-linux-<arch>.tar.gz`),
//! checks it, installs it in `~/.local/bin`, writes the example config if
//! there is none, installs the systemd user unit, pairs with the ticket it
//! was given and starts the service. No sudo, no audio settings touched,
//! no linger enabled (it says how).
//!
//! Every file here wants the download key of the invite the mesh holds now
//! (`?k=`, derived one way from it, compared in constant time by the mesh
//! itself: [`set_install_check`]); without it, and for any other file than
//! this version's builds in the releases folder, the answer is 404. The
//! ticket never travels: the line hands it to the script on the device. The
//! Windows program of this version is served the same way.

use std::sync::{Arc, PoisonError, RwLock};

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nikgob_core::releases;
use serde::Deserialize;

use crate::Shared;
use crate::error::ApiError;

/// Answers whether a download key opens `/install/` now.
pub type InstallCheck = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The mesh's check, while it runs (one mesh per process).
static CHECK: RwLock<Option<InstallCheck>> = RwLock::new(None);

/// The mesh registers how a download key is checked against the invite it
/// holds (under its own lock: an invite spent or revoked there opens
/// nothing from that moment), and takes it back (`None`) when it stops.
/// Without a check `/install/` answers nothing.
pub fn set_install_check(check: Option<InstallCheck>) {
    *CHECK.write().unwrap_or_else(PoisonError::into_inner) = check;
}

/// `key` opens the files by `check`.
fn key_ok(check: Option<&InstallCheck>, key: &str) -> bool {
    releases::plain_key(key) && check.is_some_and(|c| c(key))
}

#[derive(Deserialize)]
pub(crate) struct KeyQuery {
    #[serde(default)]
    k: String,
}

/// The address the request was sent to (its `Host`), if it is a plain
/// host[:port] that may stand in a shell script.
pub(crate) fn host_of(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?.trim();
    releases::plain_host(host).then(|| host.to_owned())
}

fn not_found() -> Response {
    ApiError::not_found("Нет такого адреса").into_response()
}

/// The content type of a build by its name.
fn content_type(file: &str) -> &'static str {
    if file.ends_with(".tar.gz") {
        "application/gzip"
    } else if file.ends_with(".exe") {
        "application/vnd.microsoft.portable-executable"
    } else {
        "application/octet-stream"
    }
}

/// `GET /install/{file}?k=<key>`.
pub(crate) async fn serve(State(_sh): State<Arc<Shared>>, Path(file): Path<String>, Query(q): Query<KeyQuery>, headers: HeaderMap) -> Response {
    let check = CHECK.read().unwrap_or_else(PoisonError::into_inner).clone();
    if !key_ok(check.as_ref(), &q.k) {
        return not_found();
    }
    let version = env!("CARGO_PKG_VERSION");
    if file == releases::INSTALL_SCRIPT {
        let Some(host) = host_of(&headers) else { return not_found() };
        // The builds' checksums: read once per file, off the async threads.
        let builds = tokio::task::spawn_blocking(move || releases::releases_dir().map(|d| releases::linux_builds(&d, version)).unwrap_or_default())
            .await
            .unwrap_or_default();
        let mut r = releases::install_script(&host, &q.k, version, &builds).into_response();
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/x-shellscript; charset=utf-8"));
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return r;
    }
    if !releases::servable(&file, version) {
        return not_found();
    }
    let Some(path) = releases::releases_dir().and_then(|d| releases::find_in(&d, &file)) else {
        return not_found();
    };
    let f = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!("api: a build could not be opened: {e}");
            return not_found();
        }
    };
    let len = f.metadata().await.ok().map(|m| m.len());
    let body = Body::from_stream(tokio_util::io::ReaderStream::with_capacity(f, 64 * 1024));
    let mut r = Response::new(body);
    *r.status_mut() = StatusCode::OK;
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(&file)));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(len) = len
        && let Ok(v) = HeaderValue::from_str(&len.to_string())
    {
        h.insert(header::CONTENT_LENGTH, v);
    }
    if file.ends_with(".exe")
        && let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{file}\""))
    {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_key_the_mesh_takes_opens_the_files() {
        let key = releases::download_key("nkm1abcdefgh234567");
        let want = key.clone();
        let check: InstallCheck = Arc::new(move |k: &str| releases::same_secret(k.as_bytes(), want.as_bytes()));
        assert!(key_ok(Some(&check), &key));
        assert!(!key_ok(Some(&check), &releases::download_key("nkm1abcdefgh234568")), "another invite's key");
        assert!(!key_ok(Some(&check), "nkm1abcdefgh234567"), "the ticket itself is no key");
        assert!(!key_ok(Some(&check), ""));
        assert!(!key_ok(None, &key), "no mesh, nothing opens");
    }

    #[test]
    fn the_host_must_be_plain() {
        let mut h = HeaderMap::new();
        assert_eq!(host_of(&h), None);
        h.insert(header::HOST, HeaderValue::from_static("192.168.0.10:47210"));
        assert_eq!(host_of(&h).as_deref(), Some("192.168.0.10:47210"));
        h.insert(header::HOST, HeaderValue::from_static("[fe80::1]:47210"));
        assert!(host_of(&h).is_some());
        h.insert(header::HOST, HeaderValue::from_static("x';id;'"));
        assert_eq!(host_of(&h), None);
    }

    #[test]
    fn builds_have_their_types() {
        assert_eq!(content_type("nikgobd-0.0.5-linux-aarch64.tar.gz"), "application/gzip");
        assert_eq!(content_type("music.nikgob-0.0.5.exe"), "application/vnd.microsoft.portable-executable");
    }
}
