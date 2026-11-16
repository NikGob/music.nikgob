//! «Добавить устройство → Установить»: a Linux machine (a Raspberry Pi, a
//! server) installs the headless player from this computer with one line,
//!
//! ```text
//! curl -fsSL "http://<this computer>:<port>/install/nikgobd.sh?t=<ticket>" | sh
//! ```
//!
//! `GET /install/nikgobd.sh` is a POSIX sh script with the ticket and this
//! computer's address in it: it downloads the build for the machine
//! (`GET /install/nikgobd-<version>-linux-<arch>.tar.gz`), installs it in
//! `~/.local/bin`, writes the example config if there is none, installs
//! the systemd user unit, pairs with the ticket and starts the service. No
//! sudo, no audio settings touched, no linger enabled (it says how).
//!
//! Every file here wants the invite being shown now (`?t=`, compared in
//! constant time, alive): without it, and for any other file than this
//! version's builds in the releases folder, the answer is 404. The Windows
//! program of this version is served the same way.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use nikgob_view::MeshInvite;
use serde::Deserialize;

use crate::Shared;
use crate::error::ApiError;

/// The script's name under `/install/`.
pub const SCRIPT: &str = "nikgobd.sh";

#[derive(Deserialize)]
pub(crate) struct TicketQuery {
    #[serde(default)]
    t: String,
}

/// `a == b` without stopping at the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `t` is the invite shown now and it is alive at `now_ms`.
pub(crate) fn ticket_ok(invite: Option<&MeshInvite>, t: &str, now_ms: i64) -> bool {
    match invite {
        Some(i) => !t.is_empty() && i.expires_ms > now_ms && same(i.ticket.as_bytes(), t.as_bytes()),
        None => false,
    }
}

/// The address the request was sent to (its `Host`), if it is a plain
/// host[:port] that may stand in a shell script.
pub(crate) fn host_of(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?.trim();
    let fine = !host.is_empty()
        && host.len() <= 255
        && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'-' | b'[' | b']'));
    fine.then(|| host.to_owned())
}

/// A ticket as it may stand in the script: letters and digits only (a
/// ticket is `nkm1` and base32).
fn plain(t: &str) -> bool {
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// The install script for `ticket`, downloading from `host`, of `version`.
pub(crate) fn script(host: &str, ticket: &str, version: &str) -> String {
    format!(
        r#"#!/bin/sh
# nikgobd {version}: the music.nikgob player without a window, from {host}.
set -eu
base='http://{host}'
ticket='{ticket}'
version='{version}'
case "$(uname -m)" in
  aarch64|arm64) arch=aarch64 ;;
  x86_64|amd64) arch=x86_64 ;;
  *) echo "nikgobd: нет сборки для $(uname -m)" >&2; exit 1 ;;
esac
name="nikgobd-$version-linux-$arch"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if command -v curl >/dev/null 2>&1; then
  curl -fsSL -o "$tmp/$name.tar.gz" "$base/install/$name.tar.gz?t=$ticket" || {{ echo "nikgobd: на компьютере нет сборки $version для $arch" >&2; exit 1; }}
elif command -v wget >/dev/null 2>&1; then
  wget -qO "$tmp/$name.tar.gz" "$base/install/$name.tar.gz?t=$ticket" || {{ echo "nikgobd: на компьютере нет сборки $version для $arch" >&2; exit 1; }}
else
  echo "nikgobd: нужен curl или wget" >&2; exit 1
fi
tar -xzf "$tmp/$name.tar.gz" -C "$tmp"
bin="$HOME/.local/bin"
conf="${{XDG_CONFIG_HOME:-$HOME/.config}}"
mkdir -p "$bin" "$conf/nikgob" "$conf/systemd/user"
if command -v systemctl >/dev/null 2>&1; then
  systemctl --user stop nikgobd 2>/dev/null || true
fi
install -m 755 "$tmp/$name/nikgobd" "$bin/nikgobd"
if [ ! -f "$conf/nikgob/nikgobd.toml" ]; then
  sed -e 's/^api_address/#api_address/' -e 's/^name =/#name =/' -e 's/^kind =/#kind =/' \
    "$tmp/$name/nikgobd.toml" > "$conf/nikgob/nikgobd.toml"
fi
install -m 644 "$tmp/$name/nikgobd.service" "$conf/systemd/user/nikgobd.service"
"$bin/nikgobd" pair "$ticket"
systemctl --user daemon-reload
systemctl --user enable --now nikgobd >/dev/null
echo "nikgobd $version установлен и привязан"
if [ "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || true)" != yes ]; then
  echo "чтобы он работал без входа в систему: sudo loginctl enable-linger $(id -un)"
fi
"#
    )
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

/// `GET /install/{file}?t=<ticket>`.
pub(crate) async fn serve(State(sh): State<Arc<Shared>>, Path(file): Path<String>, Query(q): Query<TicketQuery>, headers: HeaderMap) -> Response {
    let invite = sh.rt.state().mesh.invite.clone();
    if !ticket_ok(invite.as_ref(), &q.t, nikgob_library::now_ms()) || !plain(&q.t) {
        return not_found();
    }
    let version = env!("CARGO_PKG_VERSION");
    if file == SCRIPT {
        let Some(host) = host_of(&headers) else { return not_found() };
        let mut r = script(&host, &q.t, version).into_response();
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/x-shellscript; charset=utf-8"));
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        return r;
    }
    if !nikgob_core::releases::servable(&file, version) {
        return not_found();
    }
    let Some(path) = nikgob_core::releases::releases_dir().and_then(|d| nikgob_core::releases::find_in(&d, &file)) else {
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

    fn invite(expires_ms: i64) -> MeshInvite {
        MeshInvite { ticket: "nkm1abcdefgh234567".into(), expires_ms }
    }

    #[test]
    fn only_the_live_invite_opens_the_files() {
        let i = invite(10_000);
        assert!(ticket_ok(Some(&i), "nkm1abcdefgh234567", 9_000));
        assert!(!ticket_ok(Some(&i), "nkm1abcdefgh234567", 10_000), "expired");
        assert!(!ticket_ok(Some(&i), "nkm1abcdefgh234568", 9_000), "another ticket");
        assert!(!ticket_ok(Some(&i), "NKM1ABCDEFGH234567", 9_000), "exactly as shown");
        assert!(!ticket_ok(Some(&i), "", 9_000));
        assert!(!ticket_ok(None, "nkm1abcdefgh234567", 9_000), "no invite shown");
        assert!(plain("nkm1abc") && !plain("nkm1'; rm -rf ~") && !plain(""));
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

    /// The script downloads this version for the machine's architecture
    /// from where it was fetched, pairs with the ticket, starts the user
    /// service, and never asks for root or touches linger and sound.
    #[test]
    fn the_script_installs_pairs_and_starts() {
        let s = script("192.168.0.10:47210", "nkm1abc", "0.0.5");
        assert!(s.starts_with("#!/bin/sh\n"));
        assert!(s.contains("base='http://192.168.0.10:47210'"));
        assert!(s.contains("ticket='nkm1abc'"));
        assert!(s.contains("version='0.0.5'"));
        assert!(s.contains("name=\"nikgobd-$version-linux-$arch\""));
        assert!(s.contains("\"$base/install/$name.tar.gz?t=$ticket\""));
        assert!(s.contains("XDG_CONFIG_HOME:-$HOME/.config}"), "the braces of the shell survive the format");
        assert!(s.contains("\"$bin/nikgobd\" pair \"$ticket\""));
        assert!(s.contains("systemctl --user enable --now nikgobd"));
        assert!(s.contains("if [ ! -f \"$conf/nikgob/nikgobd.toml\" ]"), "a config there is kept");
        for line in s.lines().filter(|l| !l.trim_start().starts_with('#') && !l.contains("echo")) {
            assert!(!line.contains("sudo"), "{line}");
            assert!(!line.contains("enable-linger"), "{line}");
            assert!(!line.contains("pactl") && !line.contains("asound"), "{line}");
        }
    }

    #[test]
    fn builds_have_their_types() {
        assert_eq!(content_type("nikgobd-0.0.5-linux-aarch64.tar.gz"), "application/gzip");
        assert_eq!(content_type("music.nikgob-0.0.5.exe"), "application/vnd.microsoft.portable-executable");
    }
}
