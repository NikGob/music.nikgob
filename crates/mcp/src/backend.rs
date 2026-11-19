//! What the MCP server asks: the player's REST API, one request at a time.
//! On stdio it is reached over HTTP ([`crate::http::HttpBackend`]); inside
//! the API's own server (`/mcp`) the router is called in-process with the
//! caller's token. Either way the token, its rights, the rate limit and
//! its actor are the API's.

use std::future::Future;

use serde_json::Value;

/// The HTTP method of a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// One request to the API: the path with its query (`/v1/search?q=…`)
/// and a JSON body.
#[derive(Clone, Debug, PartialEq)]
pub struct ApiRequest {
    pub method: Method,
    pub path: String,
    pub body: Option<Value>,
}

impl ApiRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self { method: Method::Get, path: path.into(), body: None }
    }

    pub fn post(path: impl Into<String>, body: Value) -> Self {
        Self { method: Method::Post, path: path.into(), body: Some(body) }
    }

    pub fn put(path: impl Into<String>, body: Value) -> Self {
        Self { method: Method::Put, path: path.into(), body: Some(body) }
    }

    pub fn delete(path: impl Into<String>) -> Self {
        Self { method: Method::Delete, path: path.into(), body: None }
    }
}

/// The API's answer: its status and its JSON (`null` for an empty or
/// non-JSON body).
#[derive(Clone, Debug, PartialEq)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Value,
}

impl ApiResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Why a request did not reach the API at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unreachable {
    /// Nothing listens: the player is not running or its API is off.
    NotRunning,
    /// Anything else, in words.
    Other(String),
}

/// What the player says when it cannot be reached.
pub const NOT_RUNNING: &str = "The player is not running or the API is off: Settings → Mesh → API and MCP";

impl Unreachable {
    pub fn message(&self) -> String {
        match self {
            Self::NotRunning => NOT_RUNNING.to_owned(),
            Self::Other(why) => format!("The player did not answer: {why}"),
        }
    }
}

/// The API, as the MCP server reaches it.
pub trait Backend: Send + Sync {
    fn call(&self, request: ApiRequest) -> impl Future<Output = Result<ApiResponse, Unreachable>> + Send;
}

/// `s` for a query string: everything but the unreserved characters
/// percent-encoded (UTF-8).
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A path with these query pairs (pairs without a value left out).
pub fn with_query(path: &str, pairs: &[(&str, Option<String>)]) -> String {
    let q: Vec<String> = pairs.iter().filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", encode(v)))).collect();
    if q.is_empty() { path.to_owned() } else { format!("{path}?{}", q.join("&")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_encoded() {
        assert_eq!(encode("кино x&y"), "%D0%BA%D0%B8%D0%BD%D0%BE%20x%26y");
        assert_eq!(encode("a-b_c.d~"), "a-b_c.d~");
        assert_eq!(with_query("/v1/search", &[("q", Some("a b".into())), ("kind", None), ("limit", Some("5".into()))]), "/v1/search?q=a%20b&limit=5");
        assert_eq!(with_query("/v1/state", &[("x", None)]), "/v1/state");
    }
}
