//! Spotify sign-in with the Authorization Code + PKCE flow.
//!
//! Ported from Spotifast `src/auth.rs` (https://github.com/crmne/spotifast).
//! Copyright (c) 2026 Carmine Paolino, MIT License.
//!
//! Two independent grants exist because Spotify treats them differently:
//!
//! - the **Web API grant** of the user's own Development Mode app (its own
//!   Client ID and quota; Spotify lets such an app work only while its owner
//!   has Premium). The public app several open-source clients share is not
//!   used any more (2026-10-17): its quota, shared by all their users,
//!   answered 429 even to a first request;
//! - the **playback grant** under Spotify's own desktop client identity, the
//!   one librespot streams with. Its access token is exchanged once for a
//!   reusable librespot credential.
//!
//! The browser does the password entry; this process only sees the one-time
//! authorization code Spotify sends back to a loopback listener.

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;

/// Spotify's own desktop client identity, the one librespot streams with.
pub const PLAYBACK_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";
pub const PLAYBACK_REDIRECT_PORT: u16 = 8898;

/// The ignored live tests are being run on purpose (`NIKGOB_SPOTIFY_LIVE`).
#[cfg(test)]
pub fn live_tests() -> bool {
    std::env::var_os("NIKGOB_SPOTIFY_LIVE").is_some()
}
/// Where the personal app's sign-in comes back (its registered Redirect URI).
pub const WEB_REDIRECT_PORT: u16 = 8989;

pub const REDIRECT_PATH: &str = "/login";

/// Playback: what librespot needs to stream.
pub const PLAYBACK_SCOPES: &[&str] = &["streaming"];

/// Web API: library, playlists, history, and `user-read-private` for the plan.
pub const WEB_SCOPES: &[&str] = &[
    "playlist-modify-private",
    "playlist-modify-public",
    "playlist-read-collaborative",
    "playlist-read-private",
    "user-library-modify",
    "user-library-read",
    "user-read-private",
    "user-read-recently-played",
    "user-top-read",
];

pub(crate) const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
pub(crate) const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// A browser sends its request line at once; anything slower or longer is
/// not the redirect and must not hold the listener while the user waits.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_LINE_LIMIT: u64 = 8 * 1024;
/// Refresh this long before the access token expires.
const REFRESH_MARGIN: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// The callback port is taken by another program.
    #[error("Port {0} is in use by another app. Close it and try again.")]
    PortBusy(u16),
    #[error("Sign-in cancelled")]
    Cancelled,
    #[error("Sign-in timed out, try again")]
    TimedOut,
    /// Spotify refused the grant itself; only the browser can mint a new one.
    #[error("Spotify rejected sign-in ({status}): {detail}")]
    Rejected { status: u16, detail: Rejection },
    /// The endpoint could not answer; the grant may still be valid.
    #[error("Spotify is unavailable: {0}")]
    Unreachable(String),
    #[error("{0}")]
    Callback(&'static str),
}

impl AuthError {
    /// Formats the failure only when a display locale is available.
    pub fn text_in(&self, loc: nikgob_core::i18n::Locale) -> String {
        use nikgob_core::i18n::{spotify_auth, spotify_auth_in};

        match self {
            Self::PortBusy(port) => spotify_auth::fmt_port_busy_in(loc, *port),
            Self::Cancelled => spotify_auth_in(loc).cancelled.to_owned(),
            Self::TimedOut => spotify_auth_in(loc).timed_out.to_owned(),
            Self::Rejected { status, detail } => {
                spotify_auth::fmt_rejected_in(loc, *status, detail.text_in(loc))
            }
            Self::Unreachable(detail) => spotify_auth::fmt_unreachable_in(loc, detail),
            Self::Callback(detail) => (*detail).to_owned(),
        }
    }
}

/// A safe OAuth classification, without endpoint response text or secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    BrowserDenied,
    InvalidGrant,
    InvalidClient,
    InvalidScope,
    AccessDenied,
    Other,
}

impl Rejection {
    fn text_in(self, loc: nikgob_core::i18n::Locale) -> &'static str {
        let s = nikgob_core::i18n::spotify_auth_in(loc);
        match self {
            Self::BrowserDenied => s.browser_denied,
            Self::InvalidGrant => s.invalid_grant,
            Self::InvalidClient => s.invalid_client,
            Self::InvalidScope => s.invalid_scope,
            Self::AccessDenied => s.access_denied,
            Self::Other => s.retry_sign_in,
        }
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.text_in(nikgob_core::i18n::Locale::En))
    }
}

/// One OAuth application identity and what it is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub client_id: String,
    pub redirect_port: u16,
    pub scopes: &'static [&'static str],
}

impl Grant {
    pub fn playback() -> Self {
        Self {
            client_id: PLAYBACK_CLIENT_ID.into(),
            redirect_port: PLAYBACK_REDIRECT_PORT,
            scopes: PLAYBACK_SCOPES,
        }
    }

    /// `None` for a blank client id.
    pub fn personal(client_id: &str) -> Option<Self> {
        let client_id = client_id.trim();
        (!client_id.is_empty()).then(|| Self {
            client_id: client_id.into(),
            redirect_port: WEB_REDIRECT_PORT,
            scopes: WEB_SCOPES,
        })
    }

    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{REDIRECT_PATH}", self.redirect_port)
    }

    pub fn is_playback(&self) -> bool {
        self.client_id == PLAYBACK_CLIENT_ID
    }
}

/// A started sign-in: the URL to open and the secrets needed to finish it.
#[derive(Clone)]
pub struct Flow {
    pub verifier: String,
    pub state: String,
    pub url: String,
}

fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

/// The authorize URL with a fresh PKCE verifier and state. The playback
/// grant always shows Spotify's consent dialog, so the user sees which
/// account streams.
pub fn begin(grant: &Grant) -> Flow {
    let verifier = random_token(48);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token(18);
    let mut url = url::Url::parse(AUTHORIZE_URL).expect("static url");
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", &grant.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &grant.redirect_uri())
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge)
            .append_pair("state", &state)
            .append_pair("scope", &grant.scopes.join(" "));
        if grant.is_playback() {
            q.append_pair("show_dialog", "true");
        }
    }
    Flow {
        verifier,
        state,
        url: url.into(),
    }
}

/// Reserves the callback port before the browser is opened.
pub fn listen_for_redirect(port: u16) -> Result<TcpListener, AuthError> {
    let address: SocketAddr = ([127, 0, 0, 1], port).into();
    let listener = std::net::TcpListener::bind(address).map_err(|_| AuthError::PortBusy(port))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| AuthError::Unreachable(e.to_string()))?;
    TcpListener::from_std(listener).map_err(|e| AuthError::Unreachable(e.to_string()))
}

/// Waits on a reserved listener for Spotify's redirect and returns the
/// authorization code. Ends early when `cancel` flips to true, or after ten
/// minutes. Requests that are not the redirect (a favicon, a stale tab) are
/// answered and ignored.
pub async fn wait_for_code(
    listener: TcpListener,
    expected_state: &str,
    mut cancel: watch::Receiver<bool>,
) -> Result<String, AuthError> {
    let deadline = tokio::time::sleep(LOGIN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        let (mut stream, _) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(a) => a,
                Err(e) => return Err(AuthError::Unreachable(e.to_string())),
            },
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return Err(AuthError::Cancelled);
                }
                continue;
            }
            _ = &mut deadline => return Err(AuthError::TimedOut),
        };
        let mut reader = BufReader::new(&mut stream).take(REQUEST_LINE_LIMIT);
        let mut line = String::new();
        match tokio::time::timeout(REQUEST_TIMEOUT, reader.read_line(&mut line)).await {
            Ok(Ok(_)) if line.ends_with('\n') => {}
            _ => continue,
        }
        let outcome = parse_request_line(&line, expected_state);
        // This listener has no UI preferences; show both supported languages.
        use nikgob_core::i18n::{Locale, common_in, spotify_auth_in};
        let ru = spotify_auth_in(Locale::Ru);
        let en = spotify_auth_in(Locale::En);
        let (status, body) = match &outcome {
            Ok(_) => (
                "200 OK",
                page(
                    common_in(Locale::Ru).done,
                    common_in(Locale::En).done,
                    ru.signed_in,
                    en.signed_in,
                ),
            ),
            Err(e) => (
                "400 Bad Request",
                page(
                    ru.sign_in_failed,
                    en.sign_in_failed,
                    &e.text_in(Locale::Ru),
                    &e.text_in(Locale::En),
                ),
            ),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        match outcome {
            Ok(code) => return Ok(code),
            // Spotify said no: waiting longer would not help.
            Err(e @ AuthError::Rejected { .. }) => return Err(e),
            Err(_) => continue,
        }
    }
}

/// The code from the redirect's request line. Never echoes the request into
/// the error: it may carry a code.
pub(crate) fn parse_request_line(line: &str, expected_state: &str) -> Result<String, AuthError> {
    let target = line
        .split_whitespace()
        .nth(1)
        .ok_or(AuthError::Callback("malformed request"))?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != REDIRECT_PATH {
        return Err(AuthError::Callback("unexpected path"));
    }
    let (mut code, mut state, mut error) = (None, None, None);
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*key {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Err(AuthError::Callback("state mismatch"));
    }
    if error.is_some() {
        return Err(AuthError::Rejected {
            status: 400,
            detail: Rejection::BrowserDenied,
        });
    }
    code.ok_or(AuthError::Callback("no authorization code"))
}

#[derive(Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

pub async fn exchange_code(
    http: &reqwest::Client,
    token_url: &str,
    grant: &Grant,
    code: &str,
    verifier: &str,
) -> Result<TokenResponse, AuthError> {
    token_request(
        http,
        token_url,
        &[
            ("client_id", grant.client_id.as_str()),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", grant.redirect_uri().as_str()),
            ("code_verifier", verifier),
        ],
    )
    .await
}

pub async fn refresh(
    http: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse, AuthError> {
    token_request(
        http,
        token_url,
        &[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ],
    )
    .await
}

async fn token_request(
    http: &reqwest::Client,
    token_url: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, AuthError> {
    let response = http
        .post(token_url)
        .form(form)
        .send()
        .await
        .map_err(|e| AuthError::Unreachable(e.without_url().to_string()))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    decode_token_response(status.as_u16(), &text)
}

/// Token endpoint errors may contain usable grants in their description:
/// only known classifications are reported, never the response text.
pub(crate) fn decode_token_response(status: u16, text: &str) -> Result<TokenResponse, AuthError> {
    if (400..500).contains(&status) {
        let value = serde_json::from_str::<serde_json::Value>(text).ok();
        let detail = match value.as_ref().and_then(|v| v["error"].as_str()) {
            Some("invalid_grant") => Rejection::InvalidGrant,
            Some("invalid_client") => Rejection::InvalidClient,
            Some("invalid_scope") => Rejection::InvalidScope,
            Some("access_denied") => Rejection::AccessDenied,
            _ => Rejection::Other,
        };
        return Err(AuthError::Rejected { status, detail });
    }
    if !(200..300).contains(&status) {
        return Err(AuthError::Unreachable(format!("HTTP {status}")));
    }
    serde_json::from_str(text)
        .map_err(|_| AuthError::Unreachable("unreadable token response".into()))
}

/// A Web API grant, serialized only into protected storage. Deliberately has
/// no Debug implementation: it contains usable secrets.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredToken {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: u64,
    #[serde(default)]
    pub scope: String,
}

impl StoredToken {
    /// `previous_refresh` is kept when a refresh answer carries no new one.
    pub fn from_response(
        client_id: &str,
        response: TokenResponse,
        previous_refresh: Option<&str>,
    ) -> Option<Self> {
        let refresh_token = response
            .refresh_token
            .or_else(|| previous_refresh.map(str::to_string))?;
        Some(Self {
            client_id: client_id.into(),
            access_token: response.access_token,
            refresh_token,
            expires_at: now_secs() + response.expires_in.unwrap_or(3600),
            scope: response.scope.unwrap_or_default(),
        })
    }

    pub fn needs_refresh(&self) -> bool {
        now_secs() + REFRESH_MARGIN.as_secs() >= self.expires_at
    }

    pub fn expired(&self) -> bool {
        now_secs() >= self.expires_at
    }
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn page(heading_ru: &str, heading_en: &str, body_ru: &str, body_en: &str) -> String {
    format!(
        "<!doctype html><html lang=\"ru\"><meta charset=\"utf-8\"><title>{heading_ru} / {heading_en}</title>\
<style>body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#111;color:#eee;font-family:system-ui,sans-serif}}\
main{{max-width:28rem;padding:2rem;text-align:center}}</style>\
<main><h1>{heading_ru}<br><span lang=\"en\">{heading_en}</span></h1><p>{body_ru}<br><span lang=\"en\">{body_en}</span></p></main><script>setTimeout(function(){{window.close()}},1500)</script></html>"
    )
}

/// How the service opens a sign-in page (the system browser; tests put a
/// stub in, see [`open_browser`]).
pub type Opener = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

/// Opens `url` in the system browser. Failing is not fatal: the UI shows the
/// link too. A test must never get here (it would open the user's real
/// browser on a real sign-in page): it panics, unless the live tests run.
pub fn open_browser(url: &str) {
    #[cfg(test)]
    assert!(live_tests(), "a test tried to open the system browser at {url}");
    #[cfg(windows)]
    let result = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    match result {
        // Reaped in the background, or the finished opener stays a zombie.
        #[cfg(unix)]
        Ok(mut child) => {
            let _ = std::thread::Builder::new().name("opener".into()).spawn(move || {
                let _ = child.wait();
            });
        }
        #[cfg(not(unix))]
        Ok(_) => {}
        Err(e) => log::warn!("spotify: cannot open the browser: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_urls_carry_pkce_and_the_right_identity() {
        let flow = begin(&Grant::personal("test-client-id").unwrap());
        assert!(flow.verifier.len() >= 43);
        let url = url::Url::parse(&flow.url).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(url.host_str(), Some("accounts.spotify.com"));
        assert_eq!(q["client_id"], "test-client-id");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:8989/login");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["state"], flow.state);
        assert_eq!(
            q["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(flow.verifier.as_bytes()))
        );
        assert!(q["scope"].contains("user-library-read"));
        assert!(!q.contains_key("show_dialog"));

        let playback = url::Url::parse(&begin(&Grant::playback()).url).unwrap();
        let q: std::collections::HashMap<_, _> = playback.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], PLAYBACK_CLIENT_ID);
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:8898/login");
        assert_eq!(q["scope"], "streaming");
        assert_eq!(q["show_dialog"], "true");

        let personal = Grant::personal("  abc ").unwrap();
        assert_eq!(personal.client_id, "abc");
        assert_eq!(personal.redirect_port, 8989);
        assert!(Grant::personal(" ").is_none());
    }

    #[test]
    fn callback_lines_are_parsed_and_never_echoed() {
        let code = parse_request_line("GET /login?code=abc%20d&state=s1 HTTP/1.1\r\n", "s1");
        assert_eq!(code.unwrap(), "abc d");
        assert!(parse_request_line("GET /login?code=abc&state=other HTTP/1.1", "s1").is_err());
        assert!(parse_request_line("GET /favicon.ico HTTP/1.1", "s1").is_err());
        assert!(matches!(
            parse_request_line("GET /login?error=access_denied&state=s1 HTTP/1.1", "s1"),
            Err(AuthError::Rejected { status: 400, detail: Rejection::BrowserDenied })
        ));
        for line in [
            "GET /login?error=secret-code&state=s1 HTTP/1.1",
            "GET /secret-code HTTP/1.1",
        ] {
            let e = parse_request_line(line, "s1").unwrap_err();
            assert!(!e.to_string().contains("secret"), "{e}");
            for loc in nikgob_core::i18n::Locale::ALL {
                assert!(!e.text_in(loc).contains("secret"));
            }
        }
    }

    #[test]
    fn token_rejections_keep_only_safe_classifications() {
        for (code, expected) in [
            ("invalid_grant", Rejection::InvalidGrant),
            ("invalid_client", Rejection::InvalidClient),
            ("invalid_scope", Rejection::InvalidScope),
            ("access_denied", Rejection::AccessDenied),
            ("secret-refresh", Rejection::Other),
        ] {
            let body = format!(r#"{{"error":"{code}","error_description":"secret-refresh"}}"#);
            let error = decode_token_response(400, &body).err().unwrap();
            assert_eq!(error, AuthError::Rejected { status: 400, detail: expected });
            for loc in nikgob_core::i18n::Locale::ALL {
                assert!(!error.text_in(loc).contains("secret"));
            }
        }
    }

    #[test]
    fn token_errors_never_include_the_response() {
        for (status, body) in [
            (
                400,
                r#"{"error":"invalid_grant","error_description":"secret-refresh"}"#,
            ),
            (400, r#"{"error":"secret-refresh"}"#),
            (503, "secret-refresh"),
            (200, r#"{"access_token":"secret-refresh","expires_in":"x"}"#),
        ] {
            let Err(e) = decode_token_response(status, body) else {
                panic!("{status} decoded");
            };
            assert!(!e.to_string().contains("secret"), "{e}");
            for loc in nikgob_core::i18n::Locale::ALL {
                assert!(!e.text_in(loc).contains("secret"));
            }
        }
        let ok = decode_token_response(
            200,
            r#"{"access_token":"a","refresh_token":"r","expires_in":3600,"scope":"x y"}"#,
        )
        .unwrap();
        let token = StoredToken::from_response("id", ok, None).unwrap();
        assert!(!token.needs_refresh() && !token.expired());
        assert_eq!(token.refresh_token, "r");
        let refreshed = TokenResponse {
            access_token: "b".into(),
            refresh_token: None,
            expires_in: Some(10),
            scope: None,
        };
        let token = StoredToken::from_response("id", refreshed, Some("r")).unwrap();
        assert_eq!(token.refresh_token, "r");
        assert!(token.needs_refresh());
    }

    #[test]
    fn a_busy_port_is_reported_by_number() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();
        let e = listen_for_redirect(port).unwrap_err();
        assert!(matches!(e, AuthError::PortBusy(p) if p == port));
        assert!(e.to_string().contains(&port.to_string()));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_requests_do_not_hold_the_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut stalled = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        stalled.write_all(b"GET /login?code=").await.unwrap();
        let mut favicon = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        favicon
            .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        browser
            .write_all(b"GET /login?code=the-code&state=st HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let (_cancel, rx) = watch::channel(false);
        assert_eq!(wait_for_code(listener, "st", rx).await.unwrap(), "the-code");
    }

    #[tokio::test]
    async fn a_cancelled_login_stops_waiting() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (tx, rx) = watch::channel(false);
        let waiting = tokio::spawn(async move { wait_for_code(listener, "st", rx).await });
        tx.send(true).unwrap();
        assert!(matches!(waiting.await.unwrap(), Err(AuthError::Cancelled)));
    }
}
