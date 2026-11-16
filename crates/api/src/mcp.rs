//! `/mcp`: the MCP server over Streamable HTTP, behind the same token as
//! `/v1`. Each message is one POST answered with JSON (`202` for a
//! notification); no session and no server-sent stream is kept (`GET` and
//! `DELETE` are `405`). The tools call the `/v1` routes in-process with
//! the caller's own `Authorization`, so the rights, the rate limit and the
//! actor are exactly those of a REST request.

use axum::Extension;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;

use nikgob_mcp::{ApiRequest, ApiResponse, Backend, McpServer, Unreachable};

use crate::auth::Caller;
use crate::error::JSON;

/// The `/v1` routes, for the tools.
#[derive(Clone)]
pub struct Inner(pub Router);

/// Answers larger than this are cut off (a library dump would be).
const ANSWER_LIMIT: usize = 16 * 1024 * 1024;

/// The API called in-process with one caller's `Authorization`.
pub struct InProcess {
    router: Router,
    auth: HeaderValue,
}

impl InProcess {
    pub fn new(router: Router, auth: HeaderValue) -> Self {
        Self { router, auth }
    }
}

impl Backend for InProcess {
    async fn call(&self, r: ApiRequest) -> Result<ApiResponse, Unreachable> {
        let body = match &r.body {
            Some(v) => Body::from(serde_json::to_vec(v).unwrap_or_default()),
            None => Body::empty(),
        };
        let req = Request::builder()
            .method(r.method.as_str())
            .uri(r.path.as_str())
            .header(header::AUTHORIZATION, self.auth.clone())
            .header(header::CONTENT_TYPE, JSON)
            .body(body)
            .map_err(|e| Unreachable::Other(e.to_string()))?;
        let resp = match self.router.clone().oneshot(req).await {
            Ok(resp) => resp,
            Err(never) => match never {},
        };
        let status = resp.status().as_u16();
        let bytes = axum::body::to_bytes(resp.into_body(), ANSWER_LIMIT).await.map_err(|e| Unreachable::Other(e.to_string()))?;
        let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        Ok(ApiResponse { status, body })
    }
}

/// `POST /mcp`: one JSON-RPC message (or a batch) in, its answer out.
pub async fn post(Extension(caller): Extension<Caller>, Extension(inner): Extension<Inner>, headers: HeaderMap, bytes: Bytes) -> Response {
    // The guard let the request in by this header (`/mcp` takes no
    // `?token=`).
    let Some(auth) = headers.get(header::AUTHORIZATION).cloned() else {
        return crate::error::ApiError::unauthorized().into_response();
    };
    let server = McpServer::with_preset(InProcess::new(inner.0, auth), caller.preset);
    let text = String::from_utf8_lossy(&bytes);
    match server.handle_text(&text).await {
        Some(answer) => {
            let mut r = Response::new(Body::from(answer));
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(JSON));
            r
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// `GET` and `DELETE /mcp`: no stream and no session to end.
pub async fn other() -> Response {
    let mut r = StatusCode::METHOD_NOT_ALLOWED.into_response();
    r.headers_mut().insert(header::ALLOW, HeaderValue::from_static("POST"));
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use serde_json::json;

    /// The in-process call sends the caller's header and reads the answer.
    #[tokio::test]
    async fn the_routes_are_called_with_the_callers_token() {
        let app = Router::new().route(
            "/v1/echo",
            get(|h: HeaderMap| async move {
                let auth = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
                crate::error::Json(json!({"auth": auth}))
            }),
        );
        let b = InProcess::new(app, HeaderValue::from_static("Bearer nk_test"));
        let r = b.call(ApiRequest::get("/v1/echo")).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body["auth"], "Bearer nk_test");
        let r = b.call(ApiRequest::get("/v1/none")).await.unwrap();
        assert_eq!(r.status, 404);
    }

    #[tokio::test]
    async fn get_is_not_allowed() {
        let r = other().await;
        assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
    }
}
