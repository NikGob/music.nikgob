//! Changes of the library, each the command the window's buttons send:
//! likes and the rest of the feedback, playlists, tags and the Trash. The
//! additive and reversible ones act at once; the destructive ones (tags,
//! the Trash, removing from, renaming or deleting a playlist) answer the
//! exact plan and a one-time `confirm_token` first ([`crate::confirm`]),
//! every item it would touch listed (so at most [`PLAN_MAX`] per request).
//! A file outside the library is never touched: such tracks are listed
//! apart and left alone.
//! Nothing here deletes a file for good: the Trash only, and nothing at
//! all while the Trash is off.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::ToSchema;

use nikgob_core::model::{ArtistId, LikeTarget, PlaylistId, ReleaseId, TrackId};
use nikgob_core::streaming::{RemoteId, Service};
use nikgob_library::Library;
use nikgob_view::{Command, LIKED_PLAYLIST, RecTarget, TagPatch};

use crate::Shared;
use crate::auth::Caller;
use crate::confirm::{PlanAnswer, args_hash, plan_hash};
use crate::error::{ApiError, Json, body, query_value};
use crate::library::{LIKED_ID, Playlist, blocking, db};

/// Items one destructive request may touch at most: its plan lists every
/// one of them.
pub const PLAN_MAX: usize = 50;

/// `400 too_many`: split the request.
fn too_many(what: &str) -> ApiError {
    ApiError::of(StatusCode::BAD_REQUEST, "too_many", format!("No more than {PLAN_MAX} {what} at a time: split the request"))
}

/// `service:id` of a service's track or album (`spotify:4uLU…`).
pub fn remote_id(s: &str) -> Option<RemoteId> {
    let (service, id) = s.trim().split_once(':')?;
    let service = Service::parse(service)?;
    (!id.is_empty()).then(|| RemoteId::new(service, id))
}

fn bad_id() -> ApiError {
    ApiError::bad_request("Invalid id")
}

fn gone() -> ApiError {
    ApiError::not_found("Not found")
}

fn track_ids(ids: &[String]) -> Result<Vec<TrackId>, ApiError> {
    ids.iter().map(|s| s.trim().parse::<TrackId>().map_err(|_| ApiError::bad_request(format!("Invalid track id: {s}")))).collect()
}

fn release_ids(ids: &[String]) -> Result<Vec<ReleaseId>, ApiError> {
    ids.iter().map(|s| s.trim().parse::<ReleaseId>().map_err(|_| ApiError::bad_request(format!("Invalid album id: {s}")))).collect()
}

fn remote_ids(ids: &[String]) -> Result<Vec<RemoteId>, ApiError> {
    ids.iter().map(|s| remote_id(s).ok_or_else(|| ApiError::bad_request(format!("Invalid service id (need service:id): {s}")))).collect()
}

/// The playlist of an API id (`liked` is Liked Songs).
fn playlist_id(s: &str) -> Result<PlaylistId, ApiError> {
    if s == LIKED_ID { Ok(LIKED_PLAYLIST) } else { s.parse().map_err(|_| bad_id()) }
}

/// "1 track", "3 tracks", "12 tracks" (English: singular only for 1).
pub fn tracks_word(n: usize) -> String {
    let word = if n == 1 { "track" } else { "tracks" };
    format!("{n} {word}")
}

// ---------------------------------------------------------------- feedback

/// `POST /v1/library/feedback`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct FeedbackBody {
    /// `like`, `unlike`, `dislike`, `undislike`, `dont_recommend`,
    /// `recommend`, `add_to_library`, `remove_from_library`.
    pub action: Option<String>,
    /// `track`, `album`, `artist`.
    pub kind: Option<String>,
    /// A library id, or `service:id` of a service's track or album.
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct DoneAnswer {
    /// `done`.
    pub status: String,
    #[schema(value_type = Object)]
    pub result: Value,
}

fn done(result: Value) -> Json<DoneAnswer> {
    Json(DoneAnswer { status: "done".to_owned(), result })
}

/// The thing feedback is about.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Track(TrackId),
    Album(ReleaseId),
    Artist(ArtistId),
    StreamTrack(RemoteId),
    StreamAlbum(RemoteId),
}

pub fn target_of(kind: &str, id: &str) -> Result<Target, ApiError> {
    if let Some(r) = remote_id(id) {
        return match kind {
            "track" => Ok(Target::StreamTrack(r)),
            "album" => Ok(Target::StreamAlbum(r)),
            _ => Err(ApiError::bad_request("A service artist can only be marked by its library id")),
        };
    }
    match kind {
        "track" => id.trim().parse().map(Target::Track).map_err(|_| bad_id()),
        "album" => id.trim().parse().map(Target::Album).map_err(|_| bad_id()),
        "artist" => id.trim().parse().map(Target::Artist).map_err(|_| bad_id()),
        _ => Err(ApiError::bad_request("kind: track, album or artist")),
    }
}

/// The command of feedback `action` on `t`: the one the window's button
/// sends (each with its Undo).
pub fn feedback_command(action: &str, t: &Target) -> Result<Command, ApiError> {
    let no = || ApiError::bad_request(format!("{action} is not allowed for this object"));
    Ok(match (action, t) {
        ("like" | "unlike", Target::Track(id)) => Command::Like(LikeTarget::Track(*id), action == "like"),
        ("like" | "unlike", Target::Album(id)) => Command::Like(LikeTarget::Release(*id), action == "like"),
        ("like" | "unlike", Target::Artist(id)) => Command::Like(LikeTarget::Artist(*id), action == "like"),
        ("like" | "unlike", Target::StreamTrack(r)) => Command::LikeStream { track: r.clone(), on: action == "like" },
        ("like" | "unlike", Target::StreamAlbum(r)) => Command::LikeStreamAlbum { album: r.clone(), on: action == "like" },
        ("dislike" | "undislike", Target::Track(id)) => Command::Dislike { track: *id, dislike: action == "dislike", sure: true, relike: false },
        ("dont_recommend" | "recommend", Target::Track(id)) => Command::RecFeedback { target: RecTarget::Track(*id), dislike: action == "dont_recommend" },
        ("dont_recommend" | "recommend", Target::Artist(id)) => Command::RecFeedback { target: RecTarget::Artist(*id), dislike: action == "dont_recommend" },
        ("dont_recommend" | "recommend", Target::StreamTrack(r)) => {
            Command::RecFeedback { target: RecTarget::StreamTrack(r.clone()), dislike: action == "dont_recommend" }
        }
        ("add_to_library" | "remove_from_library", Target::Track(id)) => Command::TrackInLibrary { track: *id, on: action == "add_to_library" },
        ("add_to_library" | "remove_from_library", Target::Album(id)) => Command::AddWholeAlbum { id: *id, on: action == "add_to_library" },
        ("add_to_library" | "remove_from_library", Target::StreamTrack(r)) => Command::StreamInLibrary { track: r.clone(), on: action == "add_to_library" },
        (
            "dislike" | "undislike" | "dont_recommend" | "recommend" | "add_to_library" | "remove_from_library",
            _,
        ) => return Err(no()),
        _ => return Err(ApiError::bad_request("action: like, unlike, dislike, undislike, dont_recommend, recommend, add_to_library or remove_from_library")),
    })
}

/// The name of a library thing; `not_found` when it is not there.
fn title_of(lib: &Library, t: &Target) -> Result<Option<String>, ApiError> {
    Ok(match t {
        Target::Track(id) => Some(lib.get_track_row(*id).map_err(db)?.ok_or_else(gone)?.title),
        Target::Album(id) => Some(lib.get_release(*id).map_err(db)?.ok_or_else(gone)?.title),
        Target::Artist(id) => Some(lib.get_artist(*id).map_err(db)?.ok_or_else(gone)?.name),
        Target::StreamTrack(_) | Target::StreamAlbum(_) => None,
    })
}

/// Likes, dislikes, don't-recommend, add-to-library: reversible,
/// each with the app's Undo.
#[utoipa::path(
    post,
    path = "/v1/library/feedback",
    tag = "library-edit",
    request_body(content = FeedbackBody, content_type = "application/json"),
    responses((status = 200, body = DoneAnswer), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn feedback(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<DoneAnswer>, ApiError> {
    let b: FeedbackBody = body(&bytes)?;
    let (Some(action), Some(kind), Some(id)) = (b.action, b.kind, b.id) else {
        return Err(ApiError::bad_request("action, kind and id are required"));
    };
    let target = target_of(kind.trim(), &id)?;
    let command = feedback_command(action.trim(), &target)?;
    let lib = sh.lib.clone();
    let t = target.clone();
    let title = blocking(move || title_of(&lib, &t)).await?;
    let mut commands = Vec::new();
    if let Target::StreamTrack(r) = &target {
        let tracks = crate::discover::stream_tracks(&sh, &caller, std::slice::from_ref(r)).await?;
        commands.push(Command::ApiKnow(tracks));
    }
    commands.push(command);
    sh.run_by(Some(caller.actor), commands).await?;
    Ok(done(json!({"action": action.trim(), "kind": kind.trim(), "id": id.trim(), "title": title})))
}

// ---------------------------------------------------------------- playlists

/// `POST /v1/playlists` and `POST /v1/playlists/{id}/tracks`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct PlaylistAddBody {
    /// A new playlist's name (`POST /v1/playlists`).
    pub name: Option<String>,
    #[serde(default)]
    pub track_ids: Vec<String>,
    /// Its tracks in album order.
    pub album_id: Option<String>,
    /// Service tracks, `service:id` (they join the library).
    #[serde(default)]
    pub remote_track_ids: Vec<String>,
}

/// A playlist as it is now.
fn playlist_now(lib: &Library, id: PlaylistId) -> Result<Playlist, ApiError> {
    if id == LIKED_PLAYLIST {
        return Ok(Playlist { id: LIKED_ID.to_owned(), name: nikgob_library::LIKED_PLAYLIST_NAME.to_owned(), track_count: lib.liked_count().map_err(db)? });
    }
    let p = lib.get_playlist(id).map_err(db)?.ok_or_else(|| ApiError::not_found("No such playlist"))?;
    let count = lib.list_playlists().map_err(db)?.into_iter().find(|s| s.id == Some(id)).map_or(0, |s| s.track_count);
    Ok(Playlist { id: p.id.to_string(), name: p.name, track_count: count })
}

/// Checks the songs and the album exist; the album's songs in order.
fn album_tracks(lib: &Library, album: Option<&str>) -> Result<Vec<TrackId>, ApiError> {
    let Some(a) = album else { return Ok(Vec::new()) };
    let id: ReleaseId = a.trim().parse().map_err(|_| bad_id())?;
    lib.get_release(id).map_err(db)?.ok_or_else(|| ApiError::not_found("No such album"))?;
    Ok(lib.tracks_of_release(id).map_err(db)?.into_iter().map(|t| t.id).collect())
}

fn known_tracks(lib: &Library, ids: &[TrackId]) -> Result<(), ApiError> {
    let found = lib.get_track_rows(ids).map_err(db)?;
    match ids.iter().find(|t| !found.contains_key(t)) {
        Some(t) => Err(ApiError::not_found(format!("No such track: {t}"))),
        None => Ok(()),
    }
}

/// Creates a playlist, with songs, an album's songs or songs of a service.
#[utoipa::path(
    post,
    path = "/v1/playlists",
    tag = "library-edit",
    request_body(content = PlaylistAddBody, content_type = "application/json"),
    responses((status = 200, body = DoneAnswer), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn create_playlist(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<DoneAnswer>, ApiError> {
    let b: PlaylistAddBody = body(&bytes)?;
    let name = b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::bad_request("name is required"))?.to_owned();
    let mut tracks = track_ids(&b.track_ids)?;
    let remotes = remote_ids(&b.remote_track_ids)?;
    let lib = sh.lib.clone();
    let album = b.album_id.clone();
    let tracks = blocking(move || {
        known_tracks(&lib, &tracks)?;
        tracks.extend(album_tracks(&lib, album.as_deref())?);
        Ok(tracks)
    })
    .await?;
    let mut commands = Vec::new();
    if !remotes.is_empty() {
        commands.push(Command::ApiKnow(crate::discover::stream_tracks(&sh, &caller, &remotes).await?));
    }
    // The id is chosen here: the answer names this playlist, whatever
    // other playlists of that name are made meanwhile.
    let id = PlaylistId::new();
    commands.push(Command::ApiCreatePlaylist { id, name, tracks });
    if !remotes.is_empty() {
        commands.push(Command::AddStreamToPlaylist(id, remotes));
    }
    sh.run_by(Some(caller.actor), commands).await?;
    let lib = sh.lib.clone();
    let p = blocking(move || playlist_now(&lib, id)).await?;
    Ok(done(json!({"playlist": p})))
}

/// Adds songs, an album's songs or songs of a service to a playlist
/// (`liked`: likes them). Only adds.
#[utoipa::path(
    post,
    path = "/v1/playlists/{id}/tracks",
    tag = "library-edit",
    params(("id" = String, Path, description = "playlist id or liked")),
    request_body(content = PlaylistAddBody, content_type = "application/json"),
    responses((status = 200, body = DoneAnswer), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn add_to_playlist(
    State(sh): State<Arc<Shared>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> Result<Json<DoneAnswer>, ApiError> {
    let b: PlaylistAddBody = body(&bytes)?;
    let pid = playlist_id(&id)?;
    let tracks = track_ids(&b.track_ids)?;
    let remotes = remote_ids(&b.remote_track_ids)?;
    let album: Option<ReleaseId> = b.album_id.as_deref().map(|a| a.trim().parse().map_err(|_| bad_id())).transpose()?;
    if tracks.is_empty() && remotes.is_empty() && album.is_none() {
        return Err(ApiError::bad_request("Nothing to add: track_ids, album_id or remote_track_ids"));
    }
    let lib = sh.lib.clone();
    let t = tracks.clone();
    blocking(move || {
        playlist_now(&lib, pid)?;
        known_tracks(&lib, &t)?;
        if let Some(a) = album {
            lib.get_release(a).map_err(db)?.ok_or_else(|| ApiError::not_found("No such album"))?;
        }
        Ok(())
    })
    .await?;
    let mut commands = Vec::new();
    if !remotes.is_empty() {
        commands.push(Command::ApiKnow(crate::discover::stream_tracks(&sh, &caller, &remotes).await?));
    }
    if !tracks.is_empty() {
        commands.push(Command::AddToPlaylist(pid, tracks));
    }
    if let Some(a) = album {
        commands.push(Command::AddAlbumToPlaylist(pid, a));
    }
    if !remotes.is_empty() {
        commands.push(Command::AddStreamToPlaylist(pid, remotes));
    }
    sh.run_by(Some(caller.actor), commands).await?;
    let lib = sh.lib.clone();
    let p = blocking(move || playlist_now(&lib, pid)).await?;
    Ok(done(json!({"playlist": p})))
}

/// What a destructive request would do, resolved now.
pub struct Plan {
    pub action: &'static str,
    pub affected: usize,
    pub plan: Value,
    pub reversible: bool,
}

/// The gate of a destructive request. Without a token: the plan, with a
/// new token bound to these arguments and to this plan. With one: `None`
/// (go on) when it fits the arguments and the plan resolved now is the
/// one it was shown with; else `409 plan_changed` with the plan as it is
/// now and a new token (nothing done).
fn gate(sh: &Shared, caller: &Caller, route: &str, args: &Value, token: Option<&str>, plan: Plan) -> Result<Option<PlanAnswer>, ApiError> {
    let args = args_hash(route, args);
    let shown = plan_hash(&plan.plan);
    let answer = |token: String| PlanAnswer::new(plan.action, plan.affected, plan.plan.clone(), plan.reversible, token);
    let Some(t) = token.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(Some(answer(sh.confirms.issue(&caller.token_id, &args, &shown, Instant::now())?)));
    };
    let was = sh.confirms.take(t, &caller.token_id, &args, Instant::now())?;
    if was == shown {
        return Ok(None);
    }
    let mut fresh = answer(sh.confirms.issue(&caller.token_id, &args, &shown, Instant::now())?);
    fresh.status = "plan_changed".to_owned();
    Err(ApiError::plan_changed(serde_json::to_value(fresh).unwrap_or_default()))
}

/// Either the plan (with a token) or, confirmed, what was done.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
#[serde(untagged)]
pub enum Destructive {
    Plan(PlanAnswer),
    Done(DoneAnswer),
}

/// `PUT /v1/playlists/{id}`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RenameBody {
    pub name: Option<String>,
    pub confirm_token: Option<String>,
}

/// Renames a playlist (with a confirmation).
#[utoipa::path(
    put,
    path = "/v1/playlists/{id}",
    tag = "library-edit",
    params(("id" = String, Path, description = "playlist id")),
    request_body(content = RenameBody, content_type = "application/json"),
    responses((status = 200, body = Destructive), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn rename_playlist(
    State(sh): State<Arc<Shared>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> Result<Json<Destructive>, ApiError> {
    let b: RenameBody = body(&bytes)?;
    let pid = playlist_id(&id)?;
    if pid == LIKED_PLAYLIST {
        return Err(ApiError::bad_request("The Liked Songs playlist cannot be renamed"));
    }
    let name = b.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| ApiError::bad_request("name is required"))?.to_owned();
    let lib = sh.lib.clone();
    let p = blocking(move || playlist_now(&lib, pid)).await?;
    let args = json!({"playlist": id, "name": name});
    let plan = Plan { action: "playlist_rename", affected: 1, plan: json!({"playlist": p.id, "before": p.name, "after": name}), reversible: true };
    if let Some(answer) = gate(&sh, &caller, "PUT /v1/playlists/{id}", &args, b.confirm_token.as_deref(), plan)? {
        return Ok(Json(Destructive::Plan(answer)));
    }
    sh.run_by(Some(caller.actor), vec![Command::RenamePlaylist(pid, name)]).await?;
    let lib = sh.lib.clone();
    let p = blocking(move || playlist_now(&lib, pid)).await?;
    Ok(Json(Destructive::Done(DoneAnswer { status: "done".to_owned(), result: json!({"playlist": p}) })))
}

/// `POST /v1/playlists/{id}/remove`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RemoveBody {
    #[serde(default)]
    pub track_ids: Vec<String>,
    pub confirm_token: Option<String>,
}

/// Takes songs out of a playlist (with a confirmation; each comes back
/// with the app's Undo). Out of Liked is an unlike.
#[utoipa::path(
    post,
    path = "/v1/playlists/{id}/remove",
    tag = "library-edit",
    params(("id" = String, Path, description = "playlist id or liked")),
    request_body(content = RemoveBody, content_type = "application/json"),
    responses((status = 200, body = Destructive), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn remove_from_playlist(
    State(sh): State<Arc<Shared>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> Result<Json<Destructive>, ApiError> {
    let b: RemoveBody = body(&bytes)?;
    let pid = playlist_id(&id)?;
    let tracks = track_ids(&b.track_ids)?;
    if tracks.is_empty() {
        return Err(ApiError::bad_request("track_ids is required"));
    }
    if tracks.len() > PLAN_MAX {
        return Err(too_many("tracks"));
    }
    let lib = sh.lib.clone();
    let t = tracks.clone();
    let (p, inside, outside) = blocking(move || {
        let p = playlist_now(&lib, pid)?;
        let rows = if pid == LIKED_PLAYLIST { lib.liked_tracks(0, u32::MAX).map_err(db)? } else { lib.playlist_entries(pid, 0, u32::MAX).map_err(db)?.into_iter().map(|e| e.track).collect() };
        let mut inside = Vec::new();
        let mut outside = Vec::new();
        for id in t {
            match rows.iter().find(|r| r.id == id) {
                Some(r) => inside.push((id, format!("{} — {}", r.artist, r.title))),
                None => outside.push(id.to_string()),
            }
        }
        Ok((p, inside, outside))
    })
    .await?;
    let args = json!({"playlist": id, "track_ids": tracks.iter().map(ToString::to_string).collect::<Vec<_>>()});
    let plan = json!({
        "playlist": {"id": p.id, "name": p.name, "track_count": p.track_count},
        "remove": inside.iter().map(|(id, name)| json!({"id": id.to_string(), "name": name})).collect::<Vec<_>>(),
        "not_in_playlist": outside,
    });
    let plan = Plan { action: "playlist_remove", affected: inside.len(), plan, reversible: true };
    if let Some(answer) = gate(&sh, &caller, "POST /v1/playlists/{id}/remove", &args, b.confirm_token.as_deref(), plan)? {
        return Ok(Json(Destructive::Plan(answer)));
    }
    let commands: Vec<Command> = inside.iter().map(|(t, _)| Command::RemoveFromPlaylist(pid, *t)).collect();
    let removed = commands.len();
    if !commands.is_empty() {
        sh.run_by(Some(caller.actor), commands).await?;
    }
    let lib = sh.lib.clone();
    let p = blocking(move || playlist_now(&lib, pid)).await?;
    Ok(Json(Destructive::Done(DoneAnswer { status: "done".to_owned(), result: json!({"removed": removed, "playlist": p}) })))
}

/// Deletes a playlist (with a confirmation; the app has no Undo
/// for it). Its songs stay in the library.
#[utoipa::path(
    delete,
    path = "/v1/playlists/{id}",
    tag = "library-edit",
    params(("id" = String, Path, description = "playlist id"), ("confirm_token" = Option<String>, Query, description = "From the response without one")),
    responses((status = 200, body = Destructive), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn delete_playlist(
    State(sh): State<Arc<Shared>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Json<Destructive>, ApiError> {
    let pid = playlist_id(&id)?;
    if pid == LIKED_PLAYLIST {
        return Err(ApiError::bad_request("The Liked Songs playlist cannot be deleted"));
    }
    let lib = sh.lib.clone();
    let p = blocking(move || playlist_now(&lib, pid)).await?;
    let token = query_value(raw.as_deref(), "confirm_token");
    let plan = json!({"delete": {"id": p.id, "name": p.name, "track_count": p.track_count}, "songs_stay_in_library": true});
    let plan = Plan { action: "playlist_delete", affected: 1, plan, reversible: false };
    if let Some(answer) = gate(&sh, &caller, "DELETE /v1/playlists/{id}", &json!({"playlist": id}), token.as_deref(), plan)? {
        return Ok(Json(Destructive::Plan(answer)));
    }
    sh.run_by(Some(caller.actor), vec![Command::DeletePlaylist(pid)]).await?;
    Ok(Json(Destructive::Done(DoneAnswer { status: "done".to_owned(), result: json!({"deleted": p}) })))
}

// ---------------------------------------------------------------- tags

/// `POST /v1/library/retag`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RetagBody {
    #[serde(default)]
    pub track_ids: Vec<String>,
    /// The fields to write; the ones not given stay as the file has them.
    #[schema(value_type = Object)]
    #[serde(default)]
    pub changes: TagPatch,
    pub confirm_token: Option<String>,
}

/// Whether the numbers of `p` are numbers.
pub fn check_patch(p: &TagPatch) -> Result<(), ApiError> {
    if p.is_empty() {
        return Err(ApiError::bad_request("changes: nothing to change"));
    }
    let digits = |v: &Option<String>| v.as_deref().map(str::trim).is_none_or(|s| s.is_empty() || s.chars().all(|c| c.is_ascii_digit()));
    if !digits(&p.track_no) || !digits(&p.disc_no) {
        return Err(ApiError::bad_request("track_no and disc_no must be numbers"));
    }
    if p.year.as_deref().map(str::trim).is_some_and(|y| !y.is_empty() && (y.len() != 4 || !y.chars().all(|c| c.is_ascii_digit()))) {
        return Err(ApiError::bad_request("year must be four digits"));
    }
    Ok(())
}

/// The plan of one track: each field the patch changes, before → after;
/// `None` when nothing would change.
pub fn retag_plan(now: &Value, p: &TagPatch) -> Option<Value> {
    let mut out = serde_json::Map::new();
    for (k, new) in p.fields() {
        let Some(new) = new.map(str::trim) else { continue };
        let old = now.get(k).and_then(Value::as_str).unwrap_or_default();
        if old != new {
            out.insert(k.to_owned(), json!({"before": old, "after": new}));
        }
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// The values a track's plan showed before the change, as a patch.
pub fn shown_before(fields: &Value) -> TagPatch {
    let mut m = serde_json::Map::new();
    if let Some(f) = fields.as_object() {
        for (k, v) in f {
            if let Some(before) = v.get("before") {
                m.insert(k.clone(), before.clone());
            }
        }
    }
    serde_json::from_value(Value::Object(m)).unwrap_or_default()
}

/// Writes tags into the files of songs and the library (with a
/// confirmation; the plan has each song's fields before and after — the
/// app keeps no other copy of them).
#[utoipa::path(
    post,
    path = "/v1/library/retag",
    tag = "library-edit",
    request_body(content = RetagBody, content_type = "application/json"),
    responses((status = 200, body = Destructive), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn retag(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<Destructive>, ApiError> {
    let b: RetagBody = body(&bytes)?;
    let tracks = track_ids(&b.track_ids)?;
    if tracks.is_empty() {
        return Err(ApiError::bad_request("track_ids is required"));
    }
    if tracks.len() > PLAN_MAX {
        return Err(too_many("tracks"));
    }
    check_patch(&b.changes)?;
    let lib = sh.lib.clone();
    let t = tracks.clone();
    blocking(move || known_tracks(&lib, &t)).await?;
    // What the files have now.
    let t = tracks.clone();
    let read = sh.ask(Some(caller.actor), Duration::from_secs(30), move |reply| Command::ApiReadTags { tracks: t, reply }).await?;
    let now = read.get("tags").cloned().unwrap_or_default();
    let outside: Vec<String> = read.get("outside").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    let mut changes = Vec::new();
    let mut without_file = Vec::new();
    let mut same = Vec::new();
    for t in &tracks {
        if outside.contains(&t.to_string()) {
            continue;
        }
        match now.get(t.to_string()) {
            None => without_file.push(t.to_string()),
            Some(tags) => match retag_plan(tags, &b.changes) {
                Some(plan) => {
                    let name = format!("{} — {}", tags.get("artist").and_then(Value::as_str).unwrap_or_default(), tags.get("title").and_then(Value::as_str).unwrap_or_default());
                    changes.push((*t, name, plan));
                }
                None => same.push(t.to_string()),
            },
        }
    }
    let args = serde_json::to_value(RetagBody { confirm_token: None, ..b.clone() }).unwrap_or_default();
    let plan = json!({
        "tracks": changes.iter().map(|(id, name, fields)| json!({"id": id.to_string(), "name": name, "fields": fields})).collect::<Vec<_>>(),
        "without_file": without_file,
        "already_so": same,
        "outside_library": outside,
        "files_renamed": "the files move to the place their new tags name",
    });
    let plan = Plan { action: "retag", affected: changes.len(), plan, reversible: false };
    if let Some(answer) = gate(&sh, &caller, "POST /v1/library/retag", &args, b.confirm_token.as_deref(), plan)? {
        return Ok(Json(Destructive::Plan(answer)));
    }
    if changes.is_empty() {
        return Ok(Json(Destructive::Done(DoneAnswer {
            status: "done".to_owned(),
            result: json!({"done": [], "failed": [], "without_file": without_file, "already_so": same, "outside_library": outside}),
        })));
    }
    let patch = b.changes.clone();
    // Each track with the values the plan showed: the loop writes nothing
    // over a field that changed since.
    let list: Vec<(TrackId, TagPatch, TagPatch)> = changes.iter().map(|(t, _, fields)| (*t, patch.clone(), shown_before(fields))).collect();
    let wait = Duration::from_secs((10 + list.len() as u64).min(120));
    let mut result = sh.ask(Some(caller.actor), wait, move |reply| Command::ApiRetag { tracks: list, reply }).await?;
    if let Some(m) = result.as_object_mut() {
        m.insert("without_file".to_owned(), json!(without_file));
        m.insert("already_so".to_owned(), json!(same));
        m.insert("outside_library".to_owned(), json!(outside));
    }
    Ok(Json(Destructive::Done(DoneAnswer { status: "done".to_owned(), result })))
}

// ---------------------------------------------------------------- the Trash

/// `POST /v1/library/trash`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct TrashBody {
    #[serde(default)]
    pub track_ids: Vec<String>,
    #[serde(default)]
    pub album_ids: Vec<String>,
    pub confirm_token: Option<String>,
}

/// Whether the Trash would move a file of `track` (a live local file or
/// the lyrics next to it) that lies outside `root` (links followed). No
/// root (the library folder is not there): every file is outside.
pub fn touches_outside(lib: &Library, track: TrackId, root: &[PathBuf]) -> Result<bool, ApiError> {
    for s in lib.sources_of_track(track).map_err(db)? {
        if !s.kind.is_local() {
            continue;
        }
        let file = PathBuf::from(&s.locator);
        if !file.exists() {
            continue;
        }
        for f in [file.with_extension("lrc"), file] {
            if f.exists() && crate::media::inside(&f, root).is_none() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// The toast's word for what goes, in `loc`: «Трек», «Альбом», «12 треков» / "Track", "Album", "12 tracks".
pub fn trash_what_in(loc: nikgob_core::i18n::Locale, tracks: usize, only_one_album: bool) -> String {
    nikgob_core::i18n::activity::fmt_trash_what_in(loc, tracks, only_one_album)
}

/// The toast's word for what goes, in English: "Track", "Album", "12 tracks".
#[cfg(test)]
pub fn trash_what(tracks: usize, only_one_album: bool) -> String {
    trash_what_in(nikgob_core::i18n::Locale::En, tracks, only_one_album)
}

/// Moves songs and albums to the library's Trash (with a confirmation):
/// kept 30 days, the app's Undo and Trash bring them back. Never
/// deletes for good; refused while the Trash is off in the settings.
#[utoipa::path(
    post,
    path = "/v1/library/trash",
    tag = "library-edit",
    request_body(content = TrashBody, content_type = "application/json"),
    responses((status = 200, body = Destructive), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn trash(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<Destructive>, ApiError> {
    let b: TrashBody = body(&bytes)?;
    if !sh.rt.state().prefs.use_trash {
        return Err(ApiError::of(StatusCode::CONFLICT, "trash_off", "The Trash is off in the settings: nothing is deleted through the API"));
    }
    let tracks = track_ids(&b.track_ids)?;
    let albums = release_ids(&b.album_ids)?;
    if tracks.is_empty() && albums.is_empty() {
        return Err(ApiError::bad_request("track_ids or album_ids is required"));
    }
    let library_dir = sh.rt.state().prefs.library_dir.clone();
    let lib = sh.lib.clone();
    let (all, names, album_names, outside) = blocking(move || {
        let root: Vec<PathBuf> = std::fs::canonicalize(&library_dir).ok().into_iter().collect();
        let mut all: Vec<TrackId> = Vec::new();
        let mut album_names = Vec::new();
        for a in &albums {
            let r = lib.get_release(*a).map_err(db)?.ok_or_else(|| ApiError::not_found(format!("No such album: {a}")))?;
            album_names.push(json!({"id": a.to_string(), "name": format!("{} — {}", r.artist_credit.display(), r.title)}));
            for t in lib.tracks_of_release(*a).map_err(db)? {
                if !all.contains(&t.id) {
                    all.push(t.id);
                }
            }
        }
        known_tracks(&lib, &tracks)?;
        for t in tracks {
            if !all.contains(&t) {
                all.push(t);
            }
        }
        if all.len() > PLAN_MAX {
            return Err(too_many("tracks (albums count by their tracks)"));
        }
        // A track with a file (or its lyrics) outside the library stays.
        let mut outside = Vec::new();
        let mut kept = Vec::new();
        for t in all {
            if touches_outside(&lib, t, &root)? {
                outside.push(t);
            } else {
                kept.push(t);
            }
        }
        let rows = lib.get_track_rows(&kept).map_err(db)?;
        let names: Vec<Value> = kept
            .iter()
            .filter_map(|t| rows.get(t).map(|r| json!({"id": t.to_string(), "name": format!("{} — {}", r.artist, r.title), "has_file": r.has_local})))
            .collect();
        Ok((kept, names, album_names, outside))
    })
    .await?;
    if all.is_empty() && !outside.is_empty() {
        return Err(ApiError::of(StatusCode::CONFLICT, "outside_library", "These songs' files are outside the library: the API does not delete them"));
    }
    let args = serde_json::to_value(TrashBody { confirm_token: None, ..b.clone() }).unwrap_or_default();
    let outside: Vec<String> = outside.iter().map(ToString::to_string).collect();
    let plan = json!({"albums": album_names, "tracks": names, "outside_library": outside, "to": "the library's Trash, 30 days"});
    let plan = Plan { action: "trash", affected: all.len(), plan, reversible: true };
    if let Some(answer) = gate(&sh, &caller, "POST /v1/library/trash", &args, b.confirm_token.as_deref(), plan)? {
        return Ok(Json(Destructive::Plan(answer)));
    }
    // The app's UI locale: the undo toast is drawn in it.
    let what = trash_what_in(nikgob_core::i18n::locale(), all.len(), b.album_ids.len() == 1 && b.track_ids.is_empty() && outside.is_empty());
    let n = all.len();
    sh.run_by(Some(caller.actor), vec![Command::ApiTrash { tracks: all, what }]).await?;
    Ok(Json(Destructive::Done(DoneAnswer { status: "done".to_owned(), result: json!({"trashed": n, "undo": "the app's Undo or its Trash"}) })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_ids_and_words() {
        assert_eq!(remote_id("spotify:4uLU"), Some(RemoteId::new(Service::Spotify, "4uLU")));
        assert_eq!(remote_id("deezer:"), None);
        assert_eq!(remote_id("nope:1"), None);
        assert_eq!(remote_id("0190a1b2-0000-7000-8000-000000000000"), None);
        assert_eq!(tracks_word(1), "1 track");
        assert_eq!(tracks_word(3), "3 tracks");
        assert_eq!(tracks_word(12), "12 tracks");
        assert_eq!(tracks_word(21), "21 tracks");
        assert_eq!(tracks_word(101), "101 tracks");
        assert_eq!(tracks_word(111), "111 tracks");
        assert_eq!(trash_what(1, false), "Track");
        assert_eq!(trash_what(10, true), "Album");
        assert_eq!(trash_what(5, false), "5 tracks");
        assert_eq!(trash_what_in(nikgob_core::i18n::Locale::Ru, 1, false), "Трек");
        assert_eq!(trash_what_in(nikgob_core::i18n::Locale::Ru, 5, false), "5 треков");
        assert_eq!(trash_what_in(nikgob_core::i18n::Locale::Ru, 10, true), "Альбом");
        assert_eq!(trash_what_in(nikgob_core::i18n::Locale::En, 21, false), "21 tracks");
    }

    #[test]
    fn each_feedback_is_the_buttons_command() {
        let t = TrackId::new();
        let a = ArtistId::new();
        let r = RemoteId::new(Service::Deezer, "1");
        assert_eq!(feedback_command("like", &Target::Track(t)).unwrap(), Command::Like(LikeTarget::Track(t), true));
        assert_eq!(feedback_command("unlike", &Target::Artist(a)).unwrap(), Command::Like(LikeTarget::Artist(a), false));
        assert_eq!(feedback_command("like", &Target::StreamTrack(r.clone())).unwrap(), Command::LikeStream { track: r.clone(), on: true });
        assert_eq!(
            feedback_command("dislike", &Target::Track(t)).unwrap(),
            Command::Dislike { track: t, dislike: true, sure: true, relike: false }
        );
        assert_eq!(
            feedback_command("dont_recommend", &Target::Artist(a)).unwrap(),
            Command::RecFeedback { target: RecTarget::Artist(a), dislike: true }
        );
        assert_eq!(feedback_command("add_to_library", &Target::Track(t)).unwrap(), Command::TrackInLibrary { track: t, on: true });
        assert_eq!(feedback_command("dislike", &Target::Artist(a)).unwrap_err().code, "bad_request");
        assert_eq!(feedback_command("explode", &Target::Track(t)).unwrap_err().code, "bad_request");
        assert!(matches!(target_of("track", "youtube:abc"), Ok(Target::StreamTrack(_))));
        assert!(target_of("artist", "youtube:abc").is_err());
        assert!(target_of("track", "nope").is_err());
        assert!(target_of("song", &t.to_string()).is_err());
    }

    #[test]
    fn a_retag_plan_has_only_what_changes() {
        let now = json!({"title": "Old", "artist": "A", "year": "1988", "genre": ""});
        let p = TagPatch { title: Some("New".into()), year: Some("1988".into()), ..Default::default() };
        let plan = retag_plan(&now, &p).unwrap();
        assert_eq!(plan, json!({"title": {"before": "Old", "after": "New"}}));
        let same = TagPatch { artist: Some(" A ".into()), ..Default::default() };
        assert!(retag_plan(&now, &same).is_none());
        assert!(check_patch(&TagPatch::default()).is_err());
        assert!(check_patch(&TagPatch { year: Some("88".into()), ..Default::default() }).is_err());
        assert!(check_patch(&TagPatch { track_no: Some("x".into()), ..Default::default() }).is_err());
        assert!(check_patch(&TagPatch { year: Some("1988".into()), track_no: Some("3".into()), ..Default::default() }).is_ok());
    }

    #[test]
    fn the_shown_values_become_the_expected_patch() {
        let fields = json!({"title": {"before": "Old", "after": "New"}, "year": {"before": "", "after": "1988"}});
        let p = shown_before(&fields);
        assert_eq!(p.title.as_deref(), Some("Old"));
        assert_eq!(p.year.as_deref(), Some(""));
        assert_eq!(p.artist, None, "a field the plan did not show is not checked");
    }

    #[test]
    fn a_track_with_a_file_outside_the_library_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let music = dir.path().join("Music");
        let other = dir.path().join("Other");
        std::fs::create_dir_all(&music).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let root = vec![std::fs::canonicalize(&music).unwrap()];
        let lib = Library::open_in_memory().unwrap();
        let track = |file: &std::path::Path| {
            use nikgob_core::model::{ArtistCredit, Codec, Source, SourceKind, Track as CoreTrack};
            let t = lib.upsert_track(&CoreTrack::new("T", ArtistCredit::single("A", None))).unwrap();
            lib.add_source(&Source::new(t.id, SourceKind::LocalFile, file.to_string_lossy(), Codec::Opus)).unwrap();
            t.id
        };
        let inside = track(&music.join("a.opus"));
        let outside = track(&other.join("b.opus"));
        std::fs::write(music.join("a.opus"), b"x").unwrap();
        std::fs::write(other.join("b.opus"), b"x").unwrap();
        assert!(!touches_outside(&lib, inside, &root).unwrap());
        assert!(touches_outside(&lib, outside, &root).unwrap());
        std::fs::write(other.join("a.lrc"), b"x").unwrap();
        assert!(!touches_outside(&lib, inside, &root).unwrap(), "only the lyrics next to the file count");
    }

    #[test]
    fn the_plan_answer_reads_as_the_contract_says() {
        let p = PlanAnswer::new("trash", 2, json!({"tracks": []}), true, "ct_x".into());
        let v = serde_json::to_value(Destructive::Plan(p)).unwrap();
        assert_eq!(v["status"], "needs_confirmation");
        assert_eq!(v["confirm_token"], "ct_x");
        assert_eq!(v["expires_in_s"], 300);
        assert_eq!(v["affected"], 2);
        let d = serde_json::to_value(Destructive::Done(DoneAnswer { status: "done".into(), result: json!({"trashed": 2}) })).unwrap();
        assert_eq!(d, json!({"status": "done", "result": {"trashed": 2}}));
    }
}
