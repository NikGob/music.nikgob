//! The tunnel to a device's API (ALPN `nikgob/http/1`): one HTTP/1.1
//! request per QUIC bi-stream, served by that device's API router in its
//! own process — no TCP port needed, and it works with the API switched
//! off. Ring membership is the authentication: an owner's certificate is
//! a caller with every right, a guest's one with control and reading.

use std::convert::Infallible;

use axum::body::Body;
use hyper_util::rt::{TokioIo, TokioTimer};
use nikgob_api::{Caller, LocalApi};
use nikgob_view::{ApiActor, ApiPreset, MeshScope};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::ring::Cert;

/// A tunnelled request's head must come within this (its body then streams
/// as long as it takes).
const HEADER_READ: std::time::Duration = std::time::Duration::from_secs(10);

/// The caller a member's certificate makes.
pub fn caller_of(cert: &Cert) -> Caller {
    let (preset, actor) = match cert.scope {
        MeshScope::Owner => (ApiPreset::Full, ApiActor::Owner),
        MeshScope::Guest => (ApiPreset::Control, ApiActor::Guest),
    };
    // The whole node id: what the device may (syncing turned off with it)
    // is looked up by it.
    Caller { token_id: format!("mesh:{}", cert.node), name: cert.name.clone(), preset, actor }
}

/// Serves one request that comes on `recv`/`send` with the local API.
pub async fn serve<R, W>(recv: R, send: W, api: LocalApi, caller: Caller)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(tokio::io::join(recv, send));
    let svc = hyper::service::service_fn(move |req: http::Request<hyper::body::Incoming>| {
        let api = api.clone();
        let caller = caller.clone();
        async move { Ok::<_, Infallible>(api.call(caller, req.map(Body::new)).await) }
    });
    let mut b = hyper::server::conn::http1::Builder::new();
    b.keep_alive(false).timer(TokioTimer::new()).header_read_timeout(HEADER_READ);
    if let Err(e) = b.serve_connection(io, svc).await {
        log::debug!("mesh: a tunnelled request ended: {e}");
    }
}

/// Sends `req` on `recv`/`send` and gives the answer (its body streams on
/// while it is read).
pub async fn request<R, W>(recv: R, send: W, req: http::Request<Body>) -> Result<http::Response<Body>, String>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(tokio::io::join(recv, send));
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Body>(io).await.map_err(|e| e.to_string())?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            log::debug!("mesh: a tunnel stream ended: {e}");
        }
    });
    let resp = sender.send_request(origin_form(req)).await.map_err(|e| e.to_string())?;
    Ok(resp.map(Body::new))
}

/// The request with its path and query only (a full URL's scheme and
/// host mean nothing in the tunnel).
pub fn origin_form(mut req: http::Request<Body>) -> http::Request<Body> {
    let pq = req.uri().path_and_query().map_or_else(|| "/".to_owned(), |p| p.as_str().to_owned());
    if let Ok(uri) = pq.parse::<http::Uri>() {
        *req.uri_mut() = uri;
    }
    if !req.headers().contains_key(http::header::HOST) {
        req.headers_mut().insert(http::header::HOST, http::HeaderValue::from_static("nikgob.mesh"));
    }
    req
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_url_becomes_its_path() {
        let req = http::Request::get("http://pi:47210/v1/state?x=1").body(Body::empty()).unwrap();
        let r = origin_form(req);
        assert_eq!(r.uri().to_string(), "/v1/state?x=1");
        assert_eq!(r.headers()[http::header::HOST], "nikgob.mesh");
    }

    #[test]
    fn a_guest_gets_control_and_reading() {
        let ring = iroh::SecretKey::from_bytes(&[1; 32]);
        let node = iroh::SecretKey::from_bytes(&[2; 32]).public();
        let owner = Cert::issue(&ring, &node, "Pi", nikgob_view::MeshKind::Pi, MeshScope::Owner, 1);
        let c = caller_of(&owner);
        assert_eq!((c.preset, c.actor, c.name.as_str()), (ApiPreset::Full, ApiActor::Owner, "Pi"));
        assert_eq!(c.token_id, format!("mesh:{node}"), "its whole node id");
        let guest = Cert::issue(&ring, &node, "TV", nikgob_view::MeshKind::Tv, MeshScope::Guest, 1);
        let c = caller_of(&guest);
        assert_eq!((c.preset, c.actor), (ApiPreset::Control, ApiActor::Guest));
    }

    /// A request through the tunnel's two ends (an in-memory pipe) reaches
    /// a router and its answer, Range and all, comes back.
    #[tokio::test]
    async fn a_request_goes_through_the_tunnel() {
        use axum::routing::get;
        let router = axum::Router::new().route(
            "/v1/media/x",
            get(|headers: http::HeaderMap| async move {
                let range = headers.get(http::header::RANGE).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
                (http::StatusCode::PARTIAL_CONTENT, [(http::header::CONTENT_RANGE, "bytes 0-3/10")], format!("got {range}"))
            }),
        );
        let api = LocalApi::for_router(router);
        let (client, server) = tokio::io::duplex(64 << 10);
        let (sr, sw) = tokio::io::split(server);
        let caller = Caller { token_id: "mesh:x".into(), name: "PC".into(), preset: ApiPreset::Full, actor: ApiActor::Owner };
        let served = tokio::spawn(serve(sr, sw, api, caller));
        let (cr, cw) = tokio::io::split(client);
        let req = http::Request::get("/v1/media/x").header(http::header::RANGE, "bytes=0-3").body(Body::empty()).unwrap();
        let resp = request(cr, cw, req).await.unwrap();
        assert_eq!(resp.status(), http::StatusCode::PARTIAL_CONTENT);
        assert_eq!(resp.headers()[http::header::CONTENT_RANGE], "bytes 0-3/10");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
        assert_eq!(&body[..], b"got bytes=0-3");
        served.await.unwrap();
    }
}
