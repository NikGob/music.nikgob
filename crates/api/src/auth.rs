//! Who may do what: the token of a request (`Authorization: Bearer`, or
//! `?token=` for the event stream and the media only), the right each
//! route needs and the rights of each preset, a per-token rate limit, and
//! the uses reported to the runtime (at most once a second a token).
//!
//! Tokens are never logged; nothing here logs a request at all.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use nikgob_core::api_token::{self, Hashed};
use nikgob_view::{ApiActor, ApiPreset, ApiTokenView, Command};
use parking_lot::{Mutex, RwLock};

use crate::Shared;
use crate::error::{ApiError, query_value};

/// What a route needs of a token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Right {
    /// The state, the queue, the outputs, the library, the events.
    Read,
    /// Play, pause, next, seek, shuffle and repeat.
    Player,
    Queue,
    Volume,
    Duck,
    Output,
    /// The files themselves (audio, covers).
    Media,
    /// Changes of the library: likes, playlists, downloads, and (with a
    /// confirmation) tags and the Trash.
    Library,
}

impl Right {
    pub const ALL: [Self; 8] =
        [Self::Read, Self::Player, Self::Queue, Self::Volume, Self::Duck, Self::Output, Self::Media, Self::Library];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Player => "player",
            Self::Queue => "queue",
            Self::Volume => "volume",
            Self::Duck => "duck",
            Self::Output => "output",
            Self::Media => "media",
            Self::Library => "library",
        }
    }
}

/// Whether a token of `preset` has `right`: «Управление» drives the player
/// and reads (the state, the queue, the library), but not the files
/// themselves; «Полный» does everything (the tokens and the devices have no
/// route in this wave); «Только чтение» reads.
pub fn allows(preset: ApiPreset, right: Right) -> bool {
    match preset {
        ApiPreset::Full => true,
        ApiPreset::Control => !matches!(right, Right::Media | Right::Library),
        ApiPreset::Read => right == Right::Read,
    }
}

/// The right a `/v1` request needs, by its method and path.
pub fn needed(method: &Method, path: &str) -> Right {
    let p = path.trim_end_matches('/');
    if p.starts_with("/v1/media/") {
        return Right::Media;
    }
    // Any token may talk MCP: each tool's own request to the API needs its
    // right.
    if p == "/mcp" {
        return Right::Read;
    }
    if method == Method::GET || method == Method::HEAD {
        return Right::Read;
    }
    match p {
        "/v1/player/volume" => Right::Volume,
        "/v1/player/duck" => Right::Duck,
        "/v1/player/output" => Right::Output,
        _ if p == "/v1/player" || p.starts_with("/v1/player/") => Right::Player,
        _ if p == "/v1/queue" || p.starts_with("/v1/queue/") => Right::Queue,
        _ => Right::Library,
    }
}

/// Where `?token=` is taken: the event stream (a browser's `EventSource`
/// sends no headers) and the media (an `<audio src>`).
pub fn query_token_allowed(path: &str) -> bool {
    path == "/v1/events" || path.starts_with("/v1/media/")
}

/// The token of `Authorization: Bearer …`.
pub fn bearer(headers: &HeaderMap) -> Option<String> {
    let v = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = v.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_owned()).filter(|t| !t.is_empty())
}

/// Who called: the token's id and name, its preset, and who plays with it
/// (the listening history will tag what it starts with that).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub token_id: String,
    pub name: String,
    pub preset: ApiPreset,
    pub actor: ApiActor,
}

struct Entry {
    caller: Caller,
    hash: Hashed,
    revoked: bool,
}

/// The tokens as the runtime shows them, their hashes parsed once.
#[derive(Default)]
pub struct Tokens {
    seen: RwLock<Vec<ApiTokenView>>,
    entries: RwLock<Arc<Vec<Entry>>>,
}

impl Tokens {
    /// Takes the runtime's list when it changed.
    pub fn refresh(&self, tokens: &[ApiTokenView]) {
        if self.seen.read().as_slice() == tokens {
            return;
        }
        let entries: Vec<Entry> = tokens
            .iter()
            .filter_map(|t| {
                let hash = Hashed::parse(&t.hash)?;
                Some(Entry {
                    caller: Caller { token_id: t.id.clone(), name: t.name.clone(), preset: t.preset, actor: t.actor },
                    hash,
                    revoked: t.revoked,
                })
            })
            .collect();
        *self.entries.write() = Arc::new(entries);
        *self.seen.write() = tokens.to_vec();
    }

    /// The caller of `secret`: a token that is kept and not revoked. Every
    /// kept hash is compared (in constant time each), whichever matches.
    pub fn find(&self, secret: &str) -> Option<Caller> {
        if !api_token::well_formed(secret) {
            return None;
        }
        let presented = Hashed::of(secret);
        let entries = self.entries.read().clone();
        let mut found = None;
        for e in entries.iter() {
            if e.hash.matches(&presented) && !e.revoked {
                found = Some(e.caller.clone());
            }
        }
        found
    }
}

/// Requests a second a token may make on average…
const RATE_PER_S: f64 = 20.0;
/// …and at once.
const BURST: f64 = 40.0;

/// Lookups on the services (and the dry search of the download sources) a
/// second a token may make on average…
pub const LOOKUPS_PER_S: f64 = 0.5;
/// …and at once.
pub const LOOKUP_BURST: f64 = 5.0;

/// A token bucket per token.
pub struct Limiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
    rate: f64,
    burst: f64,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new(RATE_PER_S, BURST)
    }
}

impl Limiter {
    /// `rate` requests a second on average, `burst` at once.
    pub fn new(rate: f64, burst: f64) -> Self {
        Self { buckets: Mutex::default(), rate, burst }
    }

    /// Whether the token `id` may make a request at `now`.
    pub fn allow(&self, id: &str, now: Instant) -> bool {
        let mut b = self.buckets.lock();
        // Only kept tokens get here: the map stays as small as their list.
        let (level, at) = b.entry(id.to_owned()).or_insert((self.burst, now));
        let refill = now.saturating_duration_since(*at).as_secs_f64() * self.rate;
        *level = (*level + refill).min(self.burst);
        *at = now;
        if *level >= 1.0 {
            *level -= 1.0;
            true
        } else {
            false
        }
    }
}

/// When each token's use was last reported to the runtime.
#[derive(Default)]
pub struct Uses {
    last: Mutex<HashMap<String, Instant>>,
}

impl Uses {
    /// Whether a use of `id` at `now` is to be reported (once a second at
    /// most).
    pub fn note(&self, id: &str, now: Instant) -> bool {
        let mut m = self.last.lock();
        match m.get(id) {
            Some(at) if now.saturating_duration_since(*at).as_millis() < 1000 => false,
            _ => {
                m.insert(id.to_owned(), now);
                true
            }
        }
    }
}

/// The guard of every `/v1` route: a token, the right, the rate.
pub async fn guard(State(sh): State<Arc<Shared>>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    // A paired device through the mesh (in this process only): its
    // certificate stands for the token; the rights and the rate are the
    // same.
    if let Some(crate::Trusted(caller)) = req.extensions_mut().remove::<crate::Trusted>() {
        let need = needed(req.method(), &path);
        if !allows(caller.preset, need) {
            return ApiError::forbidden(need.key()).into_response();
        }
        if !sh.limiter.allow(&caller.token_id, Instant::now()) {
            return ApiError::rate_limited().into_response();
        }
        req.extensions_mut().insert(caller);
        return next.run(req).await;
    }
    let secret = bearer(req.headers()).or_else(|| {
        if query_token_allowed(&path) { query_value(req.uri().query(), "token") } else { None }
    });
    // Against the tokens as they are now: a revoke counts from the next request.
    let Some(caller) = secret.as_deref().and_then(|s| sh.caller(s)) else {
        return ApiError::unauthorized().into_response();
    };
    let need = needed(req.method(), &path);
    if !allows(caller.preset, need) {
        return ApiError::forbidden(need.key()).into_response();
    }
    let now = Instant::now();
    if !sh.limiter.allow(&caller.token_id, now) {
        return ApiError::rate_limited().into_response();
    }
    if sh.uses.note(&caller.token_id, now) {
        sh.rt.send(Command::ApiTokensUsed(vec![(caller.token_id.clone(), nikgob_library::now_ms())]));
    }
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// `GET /v1/token`: the token of the request.
#[derive(Clone, Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    /// `control`, `full` or `read`.
    pub preset: String,
    /// `owner`, `assistant`, `agent` or `guest`.
    pub actor: String,
    /// What it may do: `read`, `player`, `queue`, `volume`, `duck`,
    /// `output`, `media`, `library`.
    pub rights: Vec<String>,
}

/// A preset or an actor as its serde name.
fn name_of<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

pub fn token_info(c: &Caller) -> TokenInfo {
    TokenInfo {
        id: c.token_id.clone(),
        name: c.name.clone(),
        preset: name_of(&c.preset),
        actor: name_of(&c.actor),
        rights: Right::ALL.iter().filter(|r| allows(c.preset, **r)).map(|r| r.key().to_owned()).collect(),
    }
}

/// Who the token is and what it may do (the MCP server shows its tools by
/// it).
#[utoipa::path(get, path = "/v1/token", tag = "player", responses((status = 200, body = TokenInfo)), security(("bearer" = [])))]
pub async fn whoami(axum::Extension(caller): axum::Extension<Caller>) -> crate::error::Json<TokenInfo> {
    crate::error::Json(token_info(&caller))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(id: &str, secret: &str, preset: ApiPreset, revoked: bool) -> ApiTokenView {
        ApiTokenView {
            id: id.into(),
            name: id.into(),
            preset,
            actor: ApiActor::Assistant,
            created_ms: 0,
            last_used_ms: None,
            revoked,
            hash: api_token::hash(secret),
        }
    }

    #[test]
    fn a_token_is_found_by_its_secret_and_a_revoked_one_is_refused() {
        let (a, b, c) = (api_token::generate().unwrap(), api_token::generate().unwrap(), api_token::generate().unwrap());
        let tokens = Tokens::default();
        tokens.refresh(&[view("a", &a, ApiPreset::Control, false), view("b", &b, ApiPreset::Read, true)]);
        assert_eq!(tokens.find(&a).map(|c| c.token_id), Some("a".to_owned()));
        assert_eq!(tokens.find(&a).map(|c| c.actor), Some(ApiActor::Assistant));
        assert!(tokens.find(&b).is_none(), "revoked");
        assert!(tokens.find(&c).is_none(), "unknown");
        assert!(tokens.find("nk_short").is_none());
        assert!(tokens.find("").is_none());
        // Revoked in the runtime: refused at the next refresh.
        tokens.refresh(&[view("a", &a, ApiPreset::Control, true)]);
        assert!(tokens.find(&a).is_none());
    }

    #[test]
    fn each_preset_has_its_rights() {
        use Right::*;
        for r in Right::ALL {
            assert!(allows(ApiPreset::Full, r), "{r:?}");
        }
        for r in [Read, Player, Queue, Volume, Duck, Output] {
            assert!(allows(ApiPreset::Control, r), "{r:?}");
        }
        assert!(!allows(ApiPreset::Control, Media));
        assert!(!allows(ApiPreset::Control, Library));
        for r in Right::ALL {
            assert_eq!(allows(ApiPreset::Read, r), r == Read, "{r:?}");
        }
    }

    #[test]
    fn each_route_needs_its_right() {
        let (get, post, put, del) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
        let cases = [
            (&get, "/v1/state", Right::Read),
            (&get, "/v1/events", Right::Read),
            (&get, "/v1/queue", Right::Read),
            (&get, "/v1/outputs", Right::Read),
            (&get, "/v1/search", Right::Read),
            (&get, "/v1/tracks/x/lyrics", Right::Read),
            (&get, "/v1/media/tracks/x/audio", Right::Media),
            (&Method::HEAD, "/v1/media/albums/x/cover", Right::Media),
            (&post, "/v1/player/play", Right::Player),
            (&post, "/v1/player/pause", Right::Player),
            (&post, "/v1/player/seek", Right::Player),
            (&put, "/v1/player/mode", Right::Player),
            (&put, "/v1/player/volume", Right::Volume),
            (&put, "/v1/player/duck", Right::Duck),
            (&del, "/v1/player/duck", Right::Duck),
            (&put, "/v1/player/output", Right::Output),
            (&post, "/v1/queue", Right::Queue),
            (&post, "/v1/queue/12/play", Right::Queue),
            (&del, "/v1/queue/12", Right::Queue),
            (&post, "/v1/queue/move", Right::Queue),
            (&del, "/v1/queue", Right::Queue),
            (&post, "/v1/playlists", Right::Library),
        ];
        for (m, p, r) in cases {
            assert_eq!(needed(m, p), r, "{m} {p}");
        }
        // The assistant's token: everything the voice contract uses, the
        // library's reads too; the files need «Полный».
        assert!(allows(ApiPreset::Control, needed(&get, "/v1/search")));
        assert!(allows(ApiPreset::Control, needed(&get, "/v1/albums/x")));
        assert!(!allows(ApiPreset::Control, needed(&get, "/v1/media/tracks/x/audio")));
        for (m, p, _) in cases.iter().filter(|c| !c.1.starts_with("/v1/media") && c.1 != "/v1/playlists") {
            assert!(allows(ApiPreset::Control, needed(m, p)), "{m} {p}");
        }
    }

    #[test]
    fn the_token_comes_from_the_header_or_for_some_paths_the_query() {
        let mut h = HeaderMap::new();
        assert_eq!(bearer(&h), None);
        h.insert(header::AUTHORIZATION, "Bearer nk_abc".parse().unwrap());
        assert_eq!(bearer(&h).as_deref(), Some("nk_abc"));
        h.insert(header::AUTHORIZATION, "bearer   nk_abc ".parse().unwrap());
        assert_eq!(bearer(&h).as_deref(), Some("nk_abc"));
        h.insert(header::AUTHORIZATION, "Basic xyz".parse().unwrap());
        assert_eq!(bearer(&h), None);
        assert!(query_token_allowed("/v1/events"));
        assert!(query_token_allowed("/v1/media/tracks/x/audio"));
        assert!(!query_token_allowed("/v1/state"));
        assert!(!query_token_allowed("/v1/player/play"));
    }

    #[test]
    fn a_token_is_limited_to_its_rate() {
        let l = Limiter::default();
        let t0 = Instant::now();
        let allowed = (0..100).filter(|_| l.allow("a", t0)).count();
        assert_eq!(allowed, BURST as usize);
        assert!(l.allow("b", t0), "each token has its own bucket");
        let later = t0 + std::time::Duration::from_millis(500);
        let refilled = (0..100).filter(|_| l.allow("a", later)).count();
        assert_eq!(refilled, (RATE_PER_S * 0.5) as usize);
    }

    #[test]
    fn mcp_and_the_new_routes_need_their_rights() {
        let (get, post, put, del) = (Method::GET, Method::POST, Method::PUT, Method::DELETE);
        assert_eq!(needed(&post, "/mcp"), Right::Read, "any token talks MCP");
        assert_eq!(needed(&get, "/mcp"), Right::Read);
        for p in ["/v1/library/feedback", "/v1/library/retag", "/v1/library/trash", "/v1/playlists/x/tracks", "/v1/playlists/x/remove", "/v1/downloads"] {
            assert_eq!(needed(&post, p), Right::Library, "{p}");
        }
        assert_eq!(needed(&put, "/v1/playlists/x"), Right::Library);
        assert_eq!(needed(&del, "/v1/playlists/x"), Right::Library);
        for p in ["/v1/stats/listening", "/v1/discover/search", "/v1/downloads", "/v1/downloads/sources", "/v1/token"] {
            assert_eq!(needed(&get, p), Right::Read, "{p}");
        }
        assert!(!allows(ApiPreset::Control, needed(&post, "/v1/library/trash")));
        assert!(allows(ApiPreset::Read, needed(&get, "/v1/stats/health")));
    }

    #[test]
    fn the_token_says_what_it_may_do() {
        let c = Caller { token_id: "a".into(), name: "Claude".into(), preset: ApiPreset::Control, actor: ApiActor::Agent };
        let info = token_info(&c);
        assert_eq!((info.preset.as_str(), info.actor.as_str()), ("control", "agent"));
        assert!(info.rights.contains(&"player".to_owned()) && !info.rights.contains(&"library".to_owned()));
        let lookups = Limiter::new(LOOKUPS_PER_S, LOOKUP_BURST);
        let t0 = Instant::now();
        assert_eq!((0..20).filter(|_| lookups.allow("a", t0)).count(), LOOKUP_BURST as usize);
        assert!(lookups.allow("a", t0 + std::time::Duration::from_secs(2)));
    }

    #[test]
    fn uses_are_reported_once_a_second() {
        let u = Uses::default();
        let t0 = Instant::now();
        assert!(u.note("a", t0));
        assert!(!u.note("a", t0 + std::time::Duration::from_millis(300)));
        assert!(u.note("b", t0));
        assert!(u.note("a", t0 + std::time::Duration::from_millis(1100)));
    }
}
