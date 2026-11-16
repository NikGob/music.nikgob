//! Playing on another device: a zone's transferable state and its load
//! (what a transfer is made of), and the player commands a remote forwards
//! to the zone it drives (`POST /v1/player/remote`, between the devices
//! only: not in the document). Paired devices call these through the
//! mesh's tunnel; a token with the rights can call the first two.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use utoipa::ToSchema;

use nikgob_core::model::TrackId;
use nikgob_view::{Command, RemoteCommand, ZoneLoaded, ZoneSnapshot};

use crate::Shared;
use crate::auth::Caller;
use crate::error::{ApiError, Json, body};
use crate::library::{blocking, db};
use crate::state::StateAnswer;

/// A snapshot is answered within this.
const SNAPSHOT_WAIT: Duration = Duration::from_secs(5);
/// A load is done within this.
const LOAD_WAIT: Duration = Duration::from_secs(10);
/// At most this many tracks of a queue are loaded (around its current one).
pub const LOAD_MAX: usize = 2000;
/// A body with more tracks than this is refused.
const BODY_MAX_TRACKS: usize = 20_000;
/// At most this many commands in one forward.
const REMOTE_MAX: usize = 64;

/// What a zone plays, to be played by another: `GET /v1/player/snapshot`,
/// the body of `POST /v1/player/load` (the document's form of the view's
/// `ZoneSnapshot`).
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct PlayerSnapshot {
    /// The queue's track ids in play order.
    pub tracks: Vec<String>,
    /// The order to go back to when shuffle goes off, as places in `tracks`.
    pub original: Vec<u64>,
    /// The current track's place in `tracks`.
    pub index: Option<u64>,
    /// Where in the current track.
    pub position_ms: u64,
    pub paused: bool,
    /// The current track's speed (shown; each track keeps its own).
    pub speed: f32,
    pub shuffle: bool,
    /// `off`, `all` or `one`.
    pub repeat: String,
    /// What the queue was started from («альбома «…»»).
    pub context: Option<String>,
    /// `release:<id>`, `playlist:<id>`, `artist:<id>`, a mix, a radio's seed.
    pub page: Option<String>,
    /// `Mix`, `OwnPlaylist`, `Album` or `ReadyPlaylist`.
    pub kind: Option<String>,
    /// The radio, wave or endless mix the queue plays: the receiving
    /// device goes on generating it.
    #[schema(value_type = Option<Object>)]
    pub radio: Option<serde_json::Value>,
    /// `[[place, track id], …]`: what «Автовоспроизведение» added.
    #[schema(value_type = Object)]
    pub autoplay_at: serde_json::Value,
    /// Tracks the sending device plays from a file (it serves those of its
    /// library at `/v1/media/tracks/{id}/audio`).
    pub files: Vec<String>,
}

/// What `POST /v1/player/load` answers.
#[derive(ToSchema)]
#[allow(dead_code)]
pub struct LoadAnswer {
    /// What the zone held before (as `GET /v1/player/snapshot` says it),
    /// for an undo; `null` when its queue was empty.
    #[schema(value_type = Option<Object>)]
    pub previous: Option<serde_json::Value>,
    /// Ids not loaded: unknown here, or with no file here nor at the device
    /// that sent them (a service's song).
    pub skipped: Vec<String>,
    /// Tracks the queue has now.
    pub loaded: u64,
}

/// `POST /v1/player/remote`.
#[derive(Debug, Default, Deserialize)]
pub struct RemoteBody {
    #[serde(default)]
    pub commands: Vec<RemoteCommand>,
}

impl Shared {
    /// The paired device a request comes from: one through the mesh's
    /// tunnel (its certificate's id begins its caller's), or the one the
    /// connect service names for a load here. Both callers exist only in
    /// this process: a token's id has no `:`.
    pub(crate) fn mesh_peer(&self, caller: &Caller) -> Option<String> {
        if let Some(id) = caller.token_id.strip_prefix("connect:") {
            return (!id.is_empty() && id != "-").then(|| id.to_owned());
        }
        let short = caller.token_id.strip_prefix("mesh:").filter(|s| !s.is_empty())?;
        let st = self.rt.state();
        st.mesh.nodes.iter().find(|n| n.id.starts_with(short)).map(|n| n.id.clone())
    }

    /// `snap.files` with the tracks this library has a file of.
    async fn with_files(&self, snap: &mut ZoneSnapshot) -> Result<(), ApiError> {
        let lib = self.lib.clone();
        let ids = snap.tracks.clone();
        let rows = blocking(move || lib.get_track_rows(&ids).map_err(db)).await?;
        let mut have: HashSet<TrackId> = snap.files.iter().copied().collect();
        for t in &snap.tracks {
            if rows.get(t).is_some_and(|r| r.has_local) && have.insert(*t) {
                snap.files.push(*t);
            }
        }
        Ok(())
    }
}

/// This zone as a snapshot to play elsewhere: the queue, the place, the
/// modes, the radio, and which tracks this device has a file of.
#[utoipa::path(
    get,
    path = "/v1/player/snapshot",
    tag = "player",
    responses((status = 200, body = PlayerSnapshot), (status = 504, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn snapshot(State(sh): State<Arc<Shared>>) -> Result<Json<ZoneSnapshot>, ApiError> {
    let v = sh.ask(None, SNAPSHOT_WAIT, |reply| Command::ApiSnapshot { reply }).await?;
    let mut snap: ZoneSnapshot = serde_json::from_value(v).map_err(|e| ApiError::internal(e.to_string()))?;
    sh.with_files(&mut snap).await?;
    Ok(Json(snap))
}

/// Replaces this zone's queue with a snapshot and plays it from its place
/// (or stays paused if it says so). Tracks this library does not know, or
/// has no file of while the sender has none either, are left out and
/// listed, never invented; files only the sender has are fetched from it
/// as they come. Answers what the zone held before, for an undo.
#[utoipa::path(
    post,
    path = "/v1/player/load",
    tag = "player",
    request_body(content = PlayerSnapshot, content_type = "application/json"),
    responses(
        (status = 200, body = LoadAnswer),
        (status = 400, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 504, body = crate::error::ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn load(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<ZoneLoaded>, ApiError> {
    let snap: ZoneSnapshot = body(&bytes)?;
    if snap.tracks.len() > BODY_MAX_TRACKS {
        return Err(ApiError::bad_request("Слишком длинная очередь"));
    }
    if snap.index.is_some_and(|i| i >= snap.tracks.len()) {
        return Err(ApiError::bad_request("index за концом очереди"));
    }
    let asked = !snap.tracks.is_empty();
    let snap = snap.window(LOAD_MAX);
    let lib = sh.lib.clone();
    let ids = snap.tracks.clone();
    let rows = blocking(move || lib.get_track_rows(&ids).map_err(db)).await?;
    let files: HashSet<TrackId> = snap.files.iter().copied().collect();
    let (snap, skipped) = snap.keep(|t| rows.get(&t).is_some_and(|r| r.has_local || files.contains(&t)));
    if asked && snap.tracks.is_empty() {
        return Err(ApiError::of(StatusCode::CONFLICT, "nothing_playable", "Здесь нет ни одной из этих песен"));
    }
    let loaded = snap.tracks.len();
    let from = sh.mesh_peer(&caller);
    let v = sh.ask(Some(caller.actor), LOAD_WAIT, move |reply| Command::ApiLoad { snapshot: Box::new(snap), from, reply }).await?;
    let mut previous: Option<ZoneSnapshot> = serde_json::from_value(v.get("previous").cloned().unwrap_or(serde_json::Value::Null)).unwrap_or(None);
    if let Some(p) = previous.as_mut() {
        sh.with_files(p).await?;
    }
    Ok(Json(ZoneLoaded { previous, skipped, loaded }))
}

/// `POST /v1/player/remote`: the commands a remote forwards, run here in
/// order as one request (the files of tracks this device lacks come from
/// the device that sent them).
pub async fn remote(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<StateAnswer>, ApiError> {
    let b: RemoteBody = body(&bytes)?;
    if b.commands.is_empty() {
        return Err(ApiError::bad_request("Нужен commands"));
    }
    if b.commands.len() > REMOTE_MAX {
        return Err(ApiError::bad_request("Слишком много команд сразу"));
    }
    let from = sh.mesh_peer(&caller);
    let commands: Vec<Command> = b.commands.into_iter().map(RemoteCommand::command).collect();
    sh.run_by(Some(caller.actor), vec![Command::ConnectRun { from, commands }]).await?;
    Ok(Json(StateAnswer { state: sh.snapshot() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track};
    use nikgob_library::Library;
    use nikgob_view::{ApiActor, ApiPreset, Status};

    struct Fixture {
        rt: nikgob_runtime::Runtime,
        sh: Arc<Shared>,
        local: crate::LocalApi,
        tracks: Vec<TrackId>,
    }

    /// A player whose library has `with` tracks with a file and `without`
    /// tracks without one, its API in this process.
    fn fixture(with: usize, without: usize) -> Fixture {
        let lib = Library::open_in_memory().unwrap();
        let mut tracks = Vec::new();
        for i in 0..with + without {
            let t = lib.upsert_track(&Track::new(format!("T{i}"), ArtistCredit::single("A", None))).unwrap();
            if i < with {
                lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("music/{i}.flac"), Codec::Flac)).unwrap();
            }
            tracks.push(t.id);
        }
        let wake = crate::Wake::new();
        let w = wake.clone();
        let rt = nikgob_runtime::Runtime::start(
            nikgob_runtime::RuntimeConfig {
                library: lib.clone(),
                streams: Vec::new(),
                library_dir: std::env::temp_dir(),
                prefs: nikgob_view::Prefs::default(),
                prefs_path: None,
                settings_path: None,
                import: None,
            },
            Box::new(nikgob_runtime::NullPlayback),
            Arc::new(move || w.wake()),
        );
        let ctx = crate::ApiContext { runtime: rt.handle(), library: lib, wake, media_roots: Vec::new() };
        let sh = Arc::new(Shared::new(ctx));
        let local = crate::LocalApi::new(sh.clone());
        Fixture { rt, sh, local, tracks }
    }

    fn caller() -> Caller {
        Caller { token_id: "mesh:abcdef0123".into(), name: "PC".into(), preset: ApiPreset::Full, actor: ApiActor::Owner }
    }

    async fn call(local: &crate::LocalApi, req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let r = local.call(caller(), req).await;
        let status = r.status();
        let bytes = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    fn post(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::post(path).header("content-type", "application/json").body(Body::from(body.to_string())).unwrap()
    }

    async fn until(what: &str, f: impl Fn() -> bool) {
        for _ in 0..2000 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn ids(v: &serde_json::Value) -> Vec<String> {
        v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
    }

    /// Unknown tracks and tracks with no file here nor at the sender are
    /// left out and listed; a paused snapshot stays paused; the queue
    /// before comes back in the answer, its files said.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_load_leaves_out_what_cannot_play_and_answers_what_was_there() {
        let f = fixture(2, 1);
        let [t0, t1, t2] = [f.tracks[0], f.tracks[1], f.tracks[2]];
        let unknown = TrackId::new();
        let a = serde_json::json!({"tracks": [t0, unknown, t2, t1], "index": 0, "position_ms": 12_000, "paused": true});
        let (status, v) = call(&f.local, post("/v1/player/load", a)).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert!(v["previous"].is_null(), "the queue was empty");
        assert_eq!(ids(&v["skipped"]), vec![unknown.to_string(), t2.to_string()]);
        assert_eq!(v["loaded"], 2);
        until("paused on the first", || {
            let st = f.rt.state();
            st.player.status == Status::Paused && st.player.current.as_ref().map(|c| c.id) == Some(t0)
        })
        .await;
        {
            let st = f.rt.state();
            assert_eq!(st.player.status, Status::Paused);
            assert_eq!(st.player.current.as_ref().map(|c| c.id), Some(t0));
            assert_eq!(st.player.position_now(), Duration::from_millis(12_000));
        }

        // A track the sender has a file of is kept; the queue before answers.
        let b = serde_json::json!({"tracks": [t1, t2], "index": 0, "paused": true, "files": [t2]});
        let (status, v) = call(&f.local, post("/v1/player/load", b)).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert!(ids(&v["skipped"]).is_empty());
        assert_eq!(v["loaded"], 2);
        let previous: ZoneSnapshot = serde_json::from_value(v["previous"].clone()).unwrap();
        assert_eq!((previous.tracks.clone(), previous.index, previous.paused), (vec![t0, t1], Some(0), true));
        assert_eq!(previous.files, vec![t0, t1], "with their files here");

        // This zone's own snapshot: its files are only the library's.
        let (status, v) = call(&f.local, Request::get("/v1/player/snapshot").body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        let s: ZoneSnapshot = serde_json::from_value(v).unwrap();
        assert_eq!((s.tracks.clone(), s.index, s.paused), (vec![t1, t2], Some(0), true));
        assert_eq!(s.files, vec![t1]);

        // Nothing of it here: refused, the queue as it was.
        let (status, v) = call(&f.local, post("/v1/player/load", serde_json::json!({"tracks": [unknown], "index": 0}))).await;
        assert_eq!(status, StatusCode::CONFLICT, "{v}");
        assert_eq!(v["error"], "nothing_playable");
        assert_eq!(f.rt.state().player.current.as_ref().map(|c| c.id), Some(t1));
        let (status, _) = call(&f.local, post("/v1/player/load", serde_json::json!({"tracks": [t0], "index": 3}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // A remote's commands run here.
        let (status, v) = call(&f.local, post("/v1/player/remote", serde_json::json!({"commands": [{"do": "toggle_play"}]}))).await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert!(v["state"]["status"] == "playing" || v["state"]["status"] == "buffering", "{v}");
        let (status, _) = call(&f.local, post("/v1/player/remote", serde_json::json!({"commands": []}))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// A device through the tunnel is known by the start of its id; the
    /// connect service names the device itself; a token is no device.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_device_of_a_request_is_found() {
        let f = fixture(0, 0);
        let id = "abcdef0123456789abcdef0123456789";
        let node = nikgob_view::MeshNodeView { id: id.into(), name: "PC".into(), online: true, ..Default::default() };
        f.rt.send(Command::MeshUpdate(Box::new(nikgob_view::MeshSnapshot {
            running: true,
            nodes: vec![nikgob_view::MeshSnapshotNode { view: node, cert: String::new() }],
            ..Default::default()
        })));
        for _ in 0..400 {
            if f.rt.state().mesh.node(id).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(f.sh.mesh_peer(&caller()).as_deref(), Some(id));
        let named = Caller { token_id: format!("connect:{id}"), ..caller() };
        assert_eq!(f.sh.mesh_peer(&named).as_deref(), Some(id));
        let none = Caller { token_id: "connect:-".into(), ..caller() };
        assert_eq!(f.sh.mesh_peer(&none), None);
        let token = Caller { token_id: "Ab12Cd34Ef".into(), ..caller() };
        assert_eq!(f.sh.mesh_peer(&token), None);
    }
}
