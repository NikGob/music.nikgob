//! `/v1/sync/…`: the owner's other devices take this library's changes
//! (and give theirs) through the mesh's tunnel. Only a paired device of the
//! owner may: a token (an assistant, a script) is refused, so is a guest's
//! device and a device syncing is turned off with. Each asks with its own
//! node id (`peer`), which has to be the one of its certificate.
//!
//! * `GET /v1/sync/heads?peer=` — the database's epoch and how far each
//!   table's changes go ([`nikgob_library::SyncHeads`]);
//! * `GET /v1/sync/changes?table=&since=&limit=&peer=` — a page of a
//!   table's changes after `since` ([`nikgob_library::SyncPage`]), none of
//!   those that came from `peer`;
//! * `POST /v1/sync/apply?peer=` — a page of `peer`'s changes merged in
//!   (idempotent) → [`nikgob_library::SyncApplied`];
//! * `GET /v1/sync/blob?path=&peer=` — a picture the synced rows name (an
//!   album's cover, an artist's photo) by its path as the rows have it, if
//!   it is inside the served folders.

use std::path::{Component, Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;

use nikgob_library::{SyncApplied, SyncHeads, SyncPage};
use nikgob_view::ApiActor;

use crate::Shared;
use crate::auth::Caller;
use crate::error::{ApiError, Json, query_value};
use crate::library::{blocking, db};
use crate::media::{open_inside, roots, serve_file};

/// What a tunnelled caller's token id starts with ([`Caller::token_id`]
/// of a paired device: `mesh:` and the first characters of its node id).
const MESH_PREFIX: &str = "mesh:";

fn refused(message: &str) -> ApiError {
    ApiError { status: StatusCode::FORBIDDEN, code: "forbidden", message: message.to_owned(), needs: None }
}

/// The node id of the device asking: an owner's paired device, through the
/// tunnel, with `peer` its own id.
pub(crate) fn peer_of(caller: &Caller, peer: Option<&str>) -> Result<String, ApiError> {
    let Some(short) = caller.token_id.strip_prefix(MESH_PREFIX) else {
        return Err(refused("Только для своих устройств"));
    };
    if caller.actor != ApiActor::Owner {
        return Err(refused("Гостевому устройству библиотека не передаётся"));
    }
    let peer = peer.map(str::trim).unwrap_or_default();
    let fine = !peer.is_empty() && peer.len() <= 128 && peer.chars().all(|c| c.is_ascii_alphanumeric()) && peer.starts_with(short);
    if !fine {
        return Err(ApiError::bad_request("Неверный id устройства"));
    }
    Ok(peer.to_owned())
}

/// [`peer_of`], and syncing with it is on.
async fn allowed(sh: &Arc<Shared>, caller: &Caller, raw: Option<&str>) -> Result<String, ApiError> {
    let peer = peer_of(caller, query_value(raw, "peer").as_deref())?;
    let lib = sh.lib.clone();
    let p = peer.clone();
    let off = blocking(move || lib.sync_cursors(&p).map(|c| c.off).map_err(db)).await?;
    if off {
        return Err(ApiError { status: StatusCode::FORBIDDEN, code: "sync_off", message: "Синхронизация с этим устройством выключена".into(), needs: None });
    }
    Ok(peer)
}

/// `GET /v1/sync/heads`.
pub async fn heads(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery) -> Result<Json<SyncHeads>, ApiError> {
    allowed(&sh, &caller, raw.as_deref()).await?;
    let lib = sh.lib.clone();
    Ok(Json(blocking(move || lib.sync_heads().map_err(db)).await?))
}

/// `GET /v1/sync/changes`.
pub async fn changes(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery) -> Result<Json<SyncPage>, ApiError> {
    let peer = allowed(&sh, &caller, raw.as_deref()).await?;
    let raw = raw.as_deref();
    let table = query_value(raw, "table").unwrap_or_default();
    if nikgob_library::sync_table(&table).is_none() {
        return Err(ApiError::bad_request("Нет такой таблицы"));
    }
    let number = |key: &str| -> Result<Option<i64>, ApiError> {
        query_value(raw, key).map(|v| v.parse::<i64>().map_err(|_| ApiError::bad_request(format!("Неверное {key}")))).transpose()
    };
    let since = number("since")?.unwrap_or(0).max(0);
    let limit = number("limit")?.map_or(nikgob_library::sync::PAGE_ROWS, |n| n.clamp(1, i64::from(u32::MAX)) as u32);
    let lib = sh.lib.clone();
    Ok(Json(blocking(move || lib.sync_changes(&table, since, limit, Some(&peer)).map_err(db)).await?))
}

/// `POST /v1/sync/apply`.
pub async fn apply(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery, bytes: Bytes) -> Result<Json<SyncApplied>, ApiError> {
    let peer = allowed(&sh, &caller, raw.as_deref()).await?;
    let page: SyncPage = serde_json::from_slice(&bytes).map_err(|e| ApiError::bad_request(format!("Неверное тело запроса: {e}")))?;
    if nikgob_library::sync_table(&page.table).is_none() {
        return Err(ApiError::bad_request("Нет такой таблицы"));
    }
    let lib = sh.lib.clone();
    Ok(Json(blocking(move || lib.sync_apply(&page, &peer, false).map_err(db)).await?))
}

/// The picture of `path` as the synced rows name it: relative to the
/// library folder (or its parent, where the artists' photos are), or
/// absolute. Pictures only, nothing outside the library's own parts.
pub(crate) fn blob_path(path: &str, library: &FsPath) -> Option<Vec<PathBuf>> {
    let p = FsPath::new(path);
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)?;
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif") {
        return None;
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    if p.is_absolute() {
        return Some(vec![p.to_path_buf()]);
    }
    let mut out = vec![library.join(p)];
    if let Some(parent) = library.parent() {
        out.push(parent.join(p));
    }
    Some(out)
}

/// `GET /v1/sync/blob`.
pub async fn blob(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery, headers: HeaderMap) -> Result<Response, ApiError> {
    allowed(&sh, &caller, raw.as_deref()).await?;
    let path = query_value(raw.as_deref(), "path").unwrap_or_default();
    let shared = sh.clone();
    let found = blocking(move || {
        let library = shared.rt.state().prefs.library_dir.clone();
        let roots = roots(&shared);
        Ok(blob_path(&path, &library).and_then(|tries| tries.iter().find_map(|p| open_inside(p, &roots))))
    })
    .await?;
    let (file, real) = found.ok_or_else(|| ApiError::not_found("Файла нет"))?;
    serve_file(file, &real, &headers).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use nikgob_view::ApiPreset;

    fn caller(token: &str, actor: ApiActor) -> Caller {
        Caller { token_id: token.into(), name: "Pi".into(), preset: ApiPreset::Full, actor }
    }

    #[test]
    fn only_the_owners_devices_may_sync() {
        let pi = caller("mesh:abcdef0123", ApiActor::Owner);
        assert_eq!(peer_of(&pi, Some("abcdef0123456789")).unwrap(), "abcdef0123456789");
        assert_eq!(peer_of(&pi, Some("ffff")).unwrap_err().code, "bad_request", "another device's id");
        assert_eq!(peer_of(&pi, None).unwrap_err().code, "bad_request");
        assert_eq!(peer_of(&pi, Some("abcdef0123/../x")).unwrap_err().code, "bad_request");
        let token = caller("k3J9aQ0b1c", ApiActor::Owner);
        assert_eq!(peer_of(&token, Some("k3J9aQ0b1c")).unwrap_err().status, StatusCode::FORBIDDEN, "a token");
        let agent = caller("mesh:abcdef0123", ApiActor::Agent);
        assert_eq!(peer_of(&agent, Some("abcdef0123")).unwrap_err().status, StatusCode::FORBIDDEN);
        let guest = caller("mesh:abcdef0123", ApiActor::Guest);
        assert_eq!(peer_of(&guest, Some("abcdef0123")).unwrap_err().status, StatusCode::FORBIDDEN, "a guest's device");
    }

    #[test]
    fn blobs_are_pictures_inside_the_library() {
        let lib = FsPath::new("/music/Music");
        assert_eq!(blob_path("Кино/1988 - Группа крови/cover.jpg", lib).unwrap()[0], lib.join("Кино/1988 - Группа крови/cover.jpg"));
        assert_eq!(blob_path("Artists/a.JPG", lib).unwrap()[1], FsPath::new("/music").join("Artists/a.JPG"));
        assert!(blob_path("Кино/01.flac", lib).is_none(), "not a picture");
        assert!(blob_path("../secret.png", lib).is_none());
        assert!(blob_path("a/../../b.png", lib).is_none());
    }

    /// Through the router: a token is refused, a paired owner device gets
    /// the heads and the pages.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_routes_refuse_tokens_and_serve_devices() {
        use axum::body::Body;
        use nikgob_library::Library;
        let library = Library::open_in_memory().unwrap();
        library.upsert_artist(&nikgob_core::model::Artist::new("Кино")).unwrap();
        let rt = nikgob_runtime::Runtime::start(
            nikgob_runtime::RuntimeConfig {
                library: library.clone(),
                streams: Vec::new(),
                library_dir: std::env::temp_dir(),
                prefs: nikgob_view::Prefs::default(),
                prefs_path: None,
                settings_path: None,
                import: None,
            },
            Box::new(nikgob_runtime::NullPlayback),
            Arc::new(|| {}),
        );
        let ctx = crate::ApiContext { runtime: rt.handle(), library: library.clone(), wake: crate::Wake::new(), media_roots: Vec::new() };
        let local = crate::LocalApi::new(Arc::new(Shared::new(ctx)));
        let get = |uri: &str| axum::http::Request::get(uri).body(Body::empty()).unwrap();
        let r = local.call(caller("k3J9aQ0b1c", ApiActor::Owner), get("/v1/sync/heads?peer=k3J9aQ0b1c")).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let pi = caller("mesh:abcdef0123", ApiActor::Owner);
        let r = local.call(pi.clone(), get("/v1/sync/heads?peer=abcdef0123456789")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let heads: SyncHeads = serde_json::from_slice(&bytes).unwrap();
        assert!(heads.tables["artist"] > 0);
        let r = local.call(pi.clone(), get("/v1/sync/changes?table=artist&since=0&peer=abcdef0123456789")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let page: SyncPage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(page.rows.len(), 1);
        let r = local.call(pi.clone(), get("/v1/sync/changes?table=meta&peer=abcdef0123456789")).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "only synced tables");
        library.set_sync_off("abcdef0123456789", true).unwrap();
        let r = local.call(pi, get("/v1/sync/heads?peer=abcdef0123456789")).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "turned off");
        drop(rt);
    }
}
