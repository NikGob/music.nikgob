//! The API reached over HTTP with a token (the stdio server talks to the
//! running player this way).

use std::time::Duration;

use serde_json::Value;

use crate::backend::{ApiRequest, ApiResponse, Backend, Method, Unreachable};

/// The longest a request may take: the dry search of the download sources
/// and the lookups on the services are the slow ones.
const TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// The API at `base` (`http://127.0.0.1:47210`) with `token`.
pub struct HttpBackend {
    client: reqwest::Client,
    base: String,
    token: String,
}

impl HttpBackend {
    pub fn new(base: &str, token: &str) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            // The player is on this computer or the home network: never
            // through a proxy the system may name.
            .no_proxy()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { client, base: base.trim_end_matches('/').to_owned(), token: token.to_owned() })
    }
}

fn method(m: Method) -> reqwest::Method {
    match m {
        Method::Get => reqwest::Method::GET,
        Method::Post => reqwest::Method::POST,
        Method::Put => reqwest::Method::PUT,
        Method::Delete => reqwest::Method::DELETE,
    }
}

impl Backend for HttpBackend {
    async fn call(&self, request: ApiRequest) -> Result<ApiResponse, Unreachable> {
        let url = format!("{}{}", self.base, request.path);
        let mut rb = self.client.request(method(request.method), url).header("Authorization", format!("Bearer {}", self.token));
        if let Some(body) = &request.body {
            rb = rb.header("Content-Type", "application/json").body(serde_json::to_vec(body).unwrap_or_default());
        }
        let resp = rb.send().await.map_err(|e| {
            if e.is_connect() {
                Unreachable::NotRunning
            } else if e.is_timeout() {
                Unreachable::Other("нет ответа вовремя".to_owned())
            } else {
                Unreachable::Other(e.to_string())
            }
        })?;
        let status = resp.status().as_u16();
        let bytes = resp.bytes().await.map_err(|e| Unreachable::Other(e.to_string()))?;
        let body = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        Ok(ApiResponse { status, body })
    }
}
