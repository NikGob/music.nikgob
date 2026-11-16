//! `/v1/sync/…`: the owner's other devices take this library's changes
//! (and give theirs) through the mesh's tunnel. Only a paired device of the
//! owner may: a token (an assistant, a script) is refused, so is a guest's
//! device and a device syncing is turned off with. The device is the one
//! of its certificate (its full node id, from the tunnel's caller), never
//! one it names.
//!
//! * `GET /v1/sync/heads` — the database's epoch and how far each table's
//!   changes go ([`nikgob_library::SyncHeads`]);
//! * `GET /v1/sync/changes?table=&since=&limit=` — a page of a table's
//!   changes after `since` ([`nikgob_library::SyncPage`]), none of those
//!   that came from the device asking;
//! * `POST /v1/sync/apply` — a page of the device's changes merged in
//!   (idempotent) → [`nikgob_library::SyncApplied`]; a page of a clock far
//!   ahead is refused (409 `clock_ahead`);
//! * `GET /v1/sync/blob?hash=` — a picture the synced rows name (an album's
//!   cover, an artist's photo) by its BLAKE3 hash, when this device has it.

use std::path::Path as FsPath;
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
use crate::media::serve_file;

/// What a tunnelled caller's token id starts with ([`Caller::token_id`]
/// of a paired device: `mesh:` and its node id).
const MESH_PREFIX: &str = "mesh:";
/// The largest picture served.
const MAX_PICTURE: u64 = 20 << 20;

fn refused(message: &str) -> ApiError {
    ApiError { status: StatusCode::FORBIDDEN, code: "forbidden", message: message.to_owned(), needs: None }
}

/// The node id of the device asking: an owner's paired device, through the
/// tunnel (its certificate's id, as the tunnel's caller carries it).
pub(crate) fn peer_of(caller: &Caller) -> Result<String, ApiError> {
    let Some(node) = caller.token_id.strip_prefix(MESH_PREFIX) else {
        return Err(refused("Только для своих устройств"));
    };
    if caller.actor != ApiActor::Owner {
        return Err(refused("Гостевому устройству библиотека не передаётся"));
    }
    let fine = !node.is_empty() && node.len() <= 128 && node.chars().all(|c| c.is_ascii_alphanumeric());
    if !fine {
        return Err(refused("Неверный id устройства"));
    }
    Ok(node.to_owned())
}

/// [`peer_of`], and syncing with it is on (and on this database).
async fn allowed(sh: &Arc<Shared>, caller: &Caller) -> Result<String, ApiError> {
    let peer = peer_of(caller)?;
    if let Some(why) = sh.lib.sync_blocked() {
        return Err(ApiError { status: StatusCode::SERVICE_UNAVAILABLE, code: "sync_blocked", message: why, needs: None });
    }
    let lib = sh.lib.clone();
    let p = peer.clone();
    let off = blocking(move || lib.sync_cursors(&p).map(|c| c.off).map_err(db)).await?;
    if off {
        return Err(ApiError { status: StatusCode::FORBIDDEN, code: "sync_off", message: "Синхронизация с этим устройством выключена".into(), needs: None });
    }
    Ok(peer)
}

/// `GET /v1/sync/heads`.
pub async fn heads(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>) -> Result<Json<SyncHeads>, ApiError> {
    allowed(&sh, &caller).await?;
    let lib = sh.lib.clone();
    Ok(Json(blocking(move || lib.sync_heads().map_err(db)).await?))
}

/// `GET /v1/sync/changes`.
pub async fn changes(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery) -> Result<Json<SyncPage>, ApiError> {
    let peer = allowed(&sh, &caller).await?;
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
pub async fn apply(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<SyncApplied>, ApiError> {
    let peer = allowed(&sh, &caller).await?;
    let page: SyncPage = serde_json::from_slice(&bytes).map_err(|e| ApiError::bad_request(format!("Неверное тело запроса: {e}")))?;
    if nikgob_library::sync_table(&page.table).is_none() {
        return Err(ApiError::bad_request("Нет такой таблицы"));
    }
    if nikgob_library::page_ahead(&page, nikgob_library::now_ms()) {
        return Err(ApiError { status: StatusCode::CONFLICT, code: "clock_ahead", message: "Часы устройства спешат".into(), needs: None });
    }
    let lib = sh.lib.clone();
    Ok(Json(blocking(move || lib.sync_apply(&page, &peer, false).map_err(db)).await?))
}

/// Whether `hash` looks like a BLAKE3 hash (64 lowercase hex digits).
pub(crate) fn is_hash(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `path` names a picture file (by its extension).
pub(crate) fn is_picture(path: &FsPath) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    matches!(ext.as_deref(), Some("jpg" | "jpeg" | "png" | "webp" | "gif"))
}

/// `GET /v1/sync/blob?hash=`: the file this device keeps for picture
/// `hash`, only while its content is still that picture.
pub async fn blob(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery, headers: HeaderMap) -> Result<Response, ApiError> {
    allowed(&sh, &caller).await?;
    let hash = query_value(raw.as_deref(), "hash").unwrap_or_default();
    if !is_hash(&hash) {
        return Err(ApiError::bad_request("Неверный hash"));
    }
    let lib = sh.lib.clone();
    let found = blocking(move || {
        let Some(path) = lib.picture_file(&hash).map_err(db)? else { return Ok(None) };
        if !is_picture(&path) {
            return Ok(None);
        }
        let same = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= MAX_PICTURE)
            && std::fs::read(&path).is_ok_and(|bytes| blake3::hash(&bytes).to_hex().as_str() == hash);
        if !same {
            return Ok(None);
        }
        Ok(std::fs::File::open(&path).ok().map(|f| (f, path)))
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

    const PI: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

    #[test]
    fn only_the_owners_devices_may_sync() {
        let pi = caller(&format!("mesh:{PI}"), ApiActor::Owner);
        assert_eq!(peer_of(&pi).unwrap(), PI, "its whole node id");
        assert_eq!(peer_of(&caller("mesh:", ApiActor::Owner)).unwrap_err().status, StatusCode::FORBIDDEN);
        assert_eq!(peer_of(&caller("mesh:ab/../x", ApiActor::Owner)).unwrap_err().status, StatusCode::FORBIDDEN);
        let token = caller("k3J9aQ0b1c", ApiActor::Owner);
        assert_eq!(peer_of(&token).unwrap_err().status, StatusCode::FORBIDDEN, "a token");
        let agent = caller(&format!("mesh:{PI}"), ApiActor::Agent);
        assert_eq!(peer_of(&agent).unwrap_err().status, StatusCode::FORBIDDEN);
        let guest = caller(&format!("mesh:{PI}"), ApiActor::Guest);
        assert_eq!(peer_of(&guest).unwrap_err().status, StatusCode::FORBIDDEN, "a guest's device");
    }

    #[test]
    fn blobs_are_pictures_by_hash() {
        assert!(is_hash(&blake3::hash(b"x").to_hex()));
        assert!(!is_hash("ABCDEF"));
        assert!(!is_hash(&"g".repeat(64)));
        assert!(is_picture(FsPath::new("/m/Кино/cover.JPG")));
        assert!(!is_picture(FsPath::new("/m/Кино/01.flac")));
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
        let r = local.call(caller("k3J9aQ0b1c", ApiActor::Owner), get("/v1/sync/heads")).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let pi = caller(&format!("mesh:{PI}"), ApiActor::Owner);
        let r = local.call(pi.clone(), get("/v1/sync/heads")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let heads: SyncHeads = serde_json::from_slice(&bytes).unwrap();
        assert!(heads.tables["artist"] > 0);
        let r = local.call(pi.clone(), get("/v1/sync/changes?table=artist&since=0")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        let page: SyncPage = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(page.rows.len(), 1);
        let r = local.call(pi.clone(), get("/v1/sync/changes?table=meta")).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "only synced tables");
        // A picture by its hash, only while the file is still that picture.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cover.jpg");
        std::fs::write(&file, b"\xFF\xD8\xFFcover").unwrap();
        let hash = blake3::hash(b"\xFF\xD8\xFFcover").to_hex().to_string();
        library.put_picture_file(&hash, &file).unwrap();
        let r = local.call(pi.clone(), get(&format!("/v1/sync/blob?hash={hash}"))).await;
        assert_eq!(r.status(), StatusCode::OK);
        std::fs::write(&file, b"another").unwrap();
        let r = local.call(pi.clone(), get(&format!("/v1/sync/blob?hash={hash}"))).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "changed since");
        // Turned off for its whole id, whatever it says of itself.
        library.set_sync_off(PI, true).unwrap();
        let r = local.call(pi.clone(), get("/v1/sync/heads?peer=abcdef0123")).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "turned off");
        drop(rt);
    }
}
