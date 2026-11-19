//! `/v1/replica/…`: what the owner's other devices ask of this one to keep
//! copies of the library (through the mesh's tunnel; the songs' files come
//! from `/v1/media/tracks/{id}/audio?profile=orig`). Only an owner's paired
//! device may ask: a token and a guest's device are refused.
//!
//! * `GET /v1/replica/masters?after=&limit=` — the lossless masters this
//!   device has the file of, by hash
//!   ([`nikgob_library::replica::MastersPage`]);
//! * `GET /v1/replica/masters/{hash}` — one master's file (Range), its hash
//!   and extension in the headers;
//! * `POST /v1/replica/check` — which of these files this device holds
//!   ([`nikgob_library::replica::CheckAnswer`]): the copies it verified, and
//!   (`rehash`, or a copy never verified) read and hashed again now.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;

use nikgob_library::replica::{CheckAnswer, CheckAsk, Held, MastersPage};

use crate::Shared;
use crate::auth::Caller;
use crate::error::{ApiError, Json, query_value};
use crate::library::{blocking, db};
use crate::media::{EXT_HEADER, HASH_HEADER, inside, open_inside, roots, serve_file};
use crate::sync::{is_hash, peer_of};

/// Masters a page lists at most.
const PAGE_MAX: u32 = 500;
/// Files one check asks about at most.
const CHECK_MAX: usize = 500;
/// Bytes one check reads and hashes at most (the rest are told deferred;
/// one file larger than this is read when it is the first to be read).
const HASH_BUDGET: u64 = 4 << 30;

/// The files right in the masters folder by their stem (`{hash}.flac`; not
/// the `.json` sidecars, not the cue sheets).
fn flat_files(dir: &FsPath) -> HashMap<String, PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else { return HashMap::new() };
    rd.flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json") || e.eq_ignore_ascii_case("cue")))
        .filter_map(|p| Some((p.file_stem()?.to_string_lossy().into_owned(), p)))
        .collect()
}

/// A locator or a path as an absolute path (relative ones are the
/// library folder's).
fn absolute(path: &str, library: &FsPath) -> PathBuf {
    let p = FsPath::new(path);
    if p.is_absolute() { p.to_owned() } else { library.join(p) }
}

/// The first of `paths` that is a file inside `roots`, else the flat one.
fn first_file(paths: &[String], flat: Option<&PathBuf>, library: &FsPath, roots: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().map(|p| absolute(p, library)).chain(flat.cloned()).find(|p| p.is_file() && inside(p, roots).is_some())
}

/// The BLAKE3 (hex) of the file at `path`.
fn blake3_of(path: &FsPath) -> std::io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(std::fs::File::open(path)?)?;
    Ok(h.finalize().to_hex().to_string())
}

/// `GET /v1/replica/masters`.
pub async fn masters(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery) -> Result<Json<MastersPage>, ApiError> {
    peer_of(&caller)?;
    let raw = raw.as_deref();
    let after = query_value(raw, "after").unwrap_or_default();
    if !after.is_empty() && !is_hash(&after) {
        return Err(ApiError::bad_request("Invalid after"));
    }
    let limit = match query_value(raw, "limit") {
        Some(v) => v.parse::<u32>().map_err(|_| ApiError::bad_request("Invalid limit"))?.clamp(1, PAGE_MAX),
        None => PAGE_MAX,
    };
    let shared = sh.clone();
    let page = blocking(move || {
        let (library, masters) = {
            let st = shared.rt.state();
            (st.prefs.library_dir.clone(), st.prefs.masters_dir.clone())
        };
        let roots = roots(&shared);
        let files = shared.lib.masters_after(&after, limit).map_err(db)?;
        let next = (files.len() as u32 >= limit).then(|| files.last().map(|f| f.item.hash.clone())).flatten();
        let flat = if files.iter().any(|f| f.flat) { flat_files(&masters) } else { HashMap::new() };
        // Only those whose file is here: the others would be asked for in
        // vain.
        let listed = files
            .into_iter()
            .filter(|f| first_file(&f.paths, if f.flat { flat.get(&f.item.hash) } else { None }, &library, &roots).is_some())
            .map(|f| f.item)
            .collect();
        Ok(MastersPage { masters: listed, next })
    })
    .await?;
    Ok(Json(page))
}

/// `GET /v1/replica/masters/{hash}`.
pub async fn master_file(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, Path(hash): Path<String>, headers: HeaderMap) -> Result<Response, ApiError> {
    peer_of(&caller)?;
    if !is_hash(&hash) {
        return Err(ApiError::bad_request("Invalid hash"));
    }
    let shared = sh.clone();
    let h = hash.clone();
    let found = blocking(move || {
        let (library, masters) = {
            let st = shared.rt.state();
            (st.prefs.library_dir.clone(), st.prefs.masters_dir.clone())
        };
        let roots = roots(&shared);
        let named: Vec<PathBuf> = shared.lib.master_files_of_hash(&h).map_err(db)?.iter().map(|p| absolute(p, &library)).collect();
        let flat = flat_files(&masters).remove(&h);
        Ok(named.into_iter().chain(flat).find_map(|p| open_inside(&p, &roots)))
    })
    .await?;
    let (file, path) = found.ok_or_else(|| ApiError::not_found("This original is not here"))?;
    let mut r = serve_file(file, &path, &headers).await?;
    if let Some(ext) = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase)
        && let Ok(v) = HeaderValue::from_str(&ext)
    {
        r.headers_mut().insert(EXT_HEADER, v);
    }
    // The hash the library knows it by: the device checks what it got
    // against it (a file changed here is refused there).
    if let Ok(v) = HeaderValue::from_str(&hash) {
        r.headers_mut().insert(HASH_HEADER, v);
    }
    Ok(r)
}

/// `POST /v1/replica/check`.
pub async fn check(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<CheckAnswer>, ApiError> {
    peer_of(&caller)?;
    let ask: CheckAsk = serde_json::from_slice(&bytes).map_err(|e| ApiError::bad_request(format!("Invalid request body: {e}")))?;
    if ask.hashes.len() > CHECK_MAX {
        return Err(ApiError::bad_request(format!("No more than {CHECK_MAX} files at a time")));
    }
    let shared = sh.clone();
    let answer = blocking(move || {
        let (library, masters) = {
            let st = shared.rt.state();
            (st.prefs.library_dir.clone(), st.prefs.masters_dir.clone())
        };
        let roots = roots(&shared);
        let me = shared.lib.device_id().0;
        let mut flat: Option<HashMap<String, PathBuf>> = None;
        let mut budget = HASH_BUDGET;
        let mut read_any = false;
        let mut held = Vec::new();
        let mut deferred = Vec::new();
        for hash in ask.hashes.iter().filter(|h| is_hash(h)) {
            let (paths, is_flat) = shared.lib.copies_of_hash(hash).map_err(db)?;
            let flat_file = if is_flat { flat.get_or_insert_with(|| flat_files(&masters)).get(hash).cloned() } else { None };
            let Some(file) = first_file(&paths, flat_file.as_ref(), &library, &roots) else { continue };
            let known = shared.lib.replica_verified(hash, &me).map_err(db)?;
            if let Some(at) = known.filter(|_| !ask.rehash) {
                held.push(Held { hash: hash.clone(), verified_ms: at });
                continue;
            }
            let size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(u64::MAX);
            // Not read this time is not absent: told apart, its row kept.
            if size > budget && read_any {
                deferred.push(hash.clone());
                continue;
            }
            budget = budget.saturating_sub(size);
            read_any = true;
            match blake3_of(&file) {
                Ok(sum) if sum == *hash => {
                    let now = nikgob_library::now_ms();
                    shared.lib.note_replica(hash, &me, Some(now)).map_err(db)?;
                    held.push(Held { hash: hash.clone(), verified_ms: now });
                }
                Ok(_) => {
                    log::warn!("replica: {} is not the file {hash} any more", file.display());
                    shared.lib.note_replica(hash, &me, None).map_err(db)?;
                }
                Err(e) => log::warn!("replica: {} not read: {e}", file.display()),
            }
        }
        Ok(CheckAnswer { held, deferred })
    })
    .await?;
    Ok(Json(answer))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::StatusCode;
    use nikgob_core::model::{Artist, ArtistCredit, Codec, Master, Track};
    use nikgob_library::Library;
    use nikgob_library::replica::MasterItem;
    use nikgob_view::{ApiActor, ApiPreset};

    use super::*;

    const PI: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

    fn caller(token: &str, actor: ApiActor) -> Caller {
        Caller { token_id: token.into(), name: "Pi".into(), preset: ApiPreset::Full, actor }
    }

    async fn body(r: Response) -> Bytes {
        axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap()
    }

    /// Through the router: a token is refused; a paired owner device sees
    /// the masters whose file is here, gets one by its hash (Range works),
    /// and hears which files are held — hashed now when never verified or
    /// when asked, a changed one not.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_owners_devices_get_the_masters_and_their_copies() {
        let dir = tempfile::tempdir().unwrap();
        let masters = dir.path().join(".musiclib").join("masters");
        let named = masters.join("Кино").join("1988 - Группа крови").join("01 - Группа крови.flac");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        let song: Vec<u8> = (0..=255u8).cycle().take(5_000).collect();
        std::fs::write(&named, &song).unwrap();
        let hash = blake3::hash(&song).to_hex().to_string();
        let missing = blake3::hash(b"gone").to_hex().to_string();

        let library = Library::open_in_memory().unwrap();
        let artist = library.upsert_artist(&Artist::new("Кино")).unwrap();
        let credit = ArtistCredit::single(artist.name.clone(), Some(artist.id));
        let t = library.upsert_track(&Track::new("Группа крови", credit.clone())).unwrap();
        let u = library.upsert_track(&Track::new("Кукушка", credit)).unwrap();
        let master = |track, h: &str, path: Option<String>| Master {
            track_id: track,
            codec: Codec::Flac,
            file_hash: h.to_owned(),
            size_bytes: 5_000,
            origin: None,
            file_path: path,
            replicas: Vec::new(),
        };
        library.put_master(&master(t.id, &hash, Some(named.to_string_lossy().into_owned()))).unwrap();
        library.put_master(&master(u.id, &missing, None)).unwrap();

        let rt = nikgob_runtime::Runtime::start(
            nikgob_runtime::RuntimeConfig {
                library: library.clone(),
                streams: Vec::new(),
                library_dir: dir.path().to_owned(),
                prefs: nikgob_view::Prefs { library_dir: dir.path().to_owned(), ..nikgob_view::Prefs::default() },
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
        let r = local.call(caller("k3J9aQ0b1c", ApiActor::Owner), get("/v1/replica/masters")).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "a token");
        let r = local.call(caller(&format!("mesh:{PI}"), ApiActor::Guest), get("/v1/replica/masters")).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "a guest's device");

        let pi = caller(&format!("mesh:{PI}"), ApiActor::Owner);
        let r = local.call(pi.clone(), get("/v1/replica/masters")).await;
        assert_eq!(r.status(), StatusCode::OK);
        let page: MastersPage = serde_json::from_slice(&body(r).await).unwrap();
        assert_eq!(page.masters, vec![MasterItem { hash: hash.clone(), codec: "flac".into(), size: 5_000, tracks: vec![t.id] }], "only one whose file is here");
        assert_eq!(page.next, None);

        let mut ranged = get(&format!("/v1/replica/masters/{hash}"));
        ranged.headers_mut().insert(axum::http::header::RANGE, HeaderValue::from_static("bytes=1000-"));
        let r = local.call(pi.clone(), ranged).await;
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.headers().get(HASH_HEADER).unwrap(), hash.as_str());
        assert_eq!(r.headers().get(EXT_HEADER).unwrap(), "flac");
        assert_eq!(body(r).await.as_ref(), &song[1000..]);
        let r = local.call(pi.clone(), get(&format!("/v1/replica/masters/{missing}"))).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        // A song's file: the original only, for now.
        let r = local.call(pi.clone(), get(&format!("/v1/media/tracks/{}/audio?profile=mp3", t.id))).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        let r = local.call(pi.clone(), get(&format!("/v1/media/tracks/{}/audio?profile=orig", t.id))).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "no song file here, only its master");

        // Exactly one file of a song, by its hash: the owner's devices only;
        // the plain route stays a token's.
        let opus = dir.path().join("Кино").join("1988 - Группа крови").join("02 - Кукушка.opus");
        std::fs::create_dir_all(opus.parent().unwrap()).unwrap();
        let bytes: Vec<u8> = (0..=255u8).rev().cycle().take(3_000).collect();
        std::fs::write(&opus, &bytes).unwrap();
        let ohash = blake3::hash(&bytes).to_hex().to_string();
        let mut s = nikgob_core::model::Source::new(u.id, nikgob_core::model::SourceKind::LocalFile, opus.to_string_lossy().into_owned(), Codec::Opus);
        s.file_hash = Some(ohash.clone());
        s.size_bytes = Some(bytes.len() as u64);
        library.add_source(&s).unwrap();
        let by_hash = format!("/v1/media/tracks/{}/audio?profile=orig&hash={ohash}", u.id);
        let r = local.call(caller("k3J9aQ0b1c", ApiActor::Owner), get(&by_hash)).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "a token asks no copy's file");
        let r = local.call(caller("k3J9aQ0b1c", ApiActor::Owner), get(&format!("/v1/media/tracks/{}/audio", u.id))).await;
        assert_eq!(r.status(), StatusCode::OK, "the plain route as it was");
        let r = local.call(pi.clone(), get(&by_hash)).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers().get(HASH_HEADER).unwrap(), ohash.as_str());
        assert_eq!(body(r).await.as_ref(), bytes.as_slice());
        let real = std::fs::canonicalize(&opus).unwrap();
        assert!(library.file_stamp(&real.to_string_lossy()).unwrap().is_some_and(|(size, _, h)| size == 3_000 && h == ohash), "noted: a resumed request is not read again");
        let r = local.call(pi.clone(), get(&format!("/v1/media/tracks/{}/audio?hash={hash}", u.id))).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "not a file of this song");

        // Its tags rewritten in place: the old hash finds nothing, and the
        // library notes the file as it is, so the next ask by its new hash
        // gets it.
        let retagged: Vec<u8> = bytes.iter().copied().chain(*b"TAGS").collect();
        std::fs::write(&opus, &retagged).unwrap();
        let nhash = blake3::hash(&retagged).to_hex().to_string();
        let r = local.call(pi.clone(), get(&by_hash)).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "the old file is gone");
        let src = library.sources_of_track(u.id).unwrap().into_iter().find(|x| x.id == s.id).unwrap();
        assert_eq!((src.file_hash.as_deref(), src.size_bytes), (Some(nhash.as_str()), Some(retagged.len() as u64)), "noted anew");
        let r = local.call(pi.clone(), get(&format!("/v1/media/tracks/{}/audio?profile=orig&hash={nhash}", u.id))).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(body(r).await.as_ref(), retagged.as_slice());

        let check = |rehash: bool| {
            let ask = CheckAsk { hashes: vec![hash.clone(), missing.clone()], rehash };
            axum::http::Request::post("/v1/replica/check").body(Body::from(serde_json::to_vec(&ask).unwrap())).unwrap()
        };
        let r = local.call(pi.clone(), check(false)).await;
        assert_eq!(r.status(), StatusCode::OK);
        let a: CheckAnswer = serde_json::from_slice(&body(r).await).unwrap();
        assert_eq!(a.held.iter().map(|h| h.hash.as_str()).collect::<Vec<_>>(), [hash.as_str()], "hashed now: never verified before");
        let me = library.device_id().0;
        assert_eq!(library.replica_verified(&hash, &me).unwrap(), Some(a.held[0].verified_ms));

        std::fs::write(&named, b"changed").unwrap();
        let r = local.call(pi.clone(), check(false)).await;
        let a: CheckAnswer = serde_json::from_slice(&body(r).await).unwrap();
        assert_eq!(a.held.len(), 1, "its verified copy, as noted");
        let r = local.call(pi.clone(), check(true)).await;
        let a: CheckAnswer = serde_json::from_slice(&body(r).await).unwrap();
        assert!(a.held.is_empty(), "read again: not that file any more");
        assert_eq!(library.replica_verified(&hash, &me).unwrap(), None);
        drop(rt);
    }
}
