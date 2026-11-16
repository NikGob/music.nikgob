//! The player, the queue and the outputs: each command goes into the same
//! loop as the window's buttons, and the answer waits until the loop got
//! past it (`Shared::run`), so it has the state the command made.

use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use nikgob_core::model::TrackId;
use nikgob_view::{Command, Status, ViewState};

use crate::Shared;
use crate::auth::Caller;
use crate::error::{ApiError, Json, body, query_value};
use crate::library::{Track, blocking};
use crate::outputs::{self, Output, Pick};
use crate::play::{self, Kind, Matched, Mode};
use crate::state::{DuckState, PlayerState, RepeatMode, StateAnswer, duck_of};

/// Nothing is playing and nothing waits in the queue.
pub fn nothing_to_play(st: &ViewState) -> bool {
    st.player.current.is_none() && st.queue.current.is_none() && st.queue.next.is_empty()
}

impl Shared {
    async fn answer(&self, commands: Vec<Command>) -> Result<Json<StateAnswer>, ApiError> {
        self.run(commands).await?;
        Ok(Json(StateAnswer { state: self.snapshot() }))
    }

    fn playing(&self) -> bool {
        self.rt.state().player.status == Status::Playing
    }

    fn empty(&self) -> bool {
        let st = self.rt.state();
        nothing_to_play(&st)
    }
}

/// «Что играет».
#[utoipa::path(
    get,
    path = "/v1/state",
    tag = "player",
    responses((status = 200, body = PlayerState), (status = 401, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn state(State(sh): State<Arc<Shared>>) -> Json<PlayerState> {
    Json(sh.snapshot())
}

/// `POST /v1/player/play`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct PlayBody {
    /// What was said, as it is: an artist, an album, a song, a playlist;
    /// «моя волна», «любимые».
    pub query: Option<String>,
    /// `any` by default.
    pub kind: Option<Kind>,
    /// A thing by its id instead of `query` (then `kind` is needed).
    pub id: Option<String>,
    /// `null`: as «Слушать» does (an artist and «Любимые» shuffled).
    pub shuffle: Option<bool>,
    /// `replace` by default.
    pub mode: Option<Mode>,
    /// Switch the output first (an id of `GET /v1/outputs` or a part of a
    /// name), in the same request: the music starts there.
    pub output: Option<String>,
}

impl Shared {
    /// The output command of `output` (an id, a name or a part of one).
    pub(crate) fn output_command(&self, output: &str) -> Result<Command, ApiError> {
        let devices = self.rt.state().prefs.output_devices.clone();
        match outputs::resolve(output, &devices, cfg!(target_os = "linux")) {
            Pick::Default => Ok(Command::SetOutputDevice(None)),
            Pick::Device(name) => Ok(Command::SetOutputDevice(Some(name))),
            Pick::NotFound => Err(ApiError::not_found("Такого устройства вывода нет")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PlayAnswer {
    /// What was found; `null` when the request only resumed.
    pub matched: Option<Matched>,
    pub state: PlayerState,
}

/// Plays what a query or an id names. No body (or an empty `query` and no
/// id) resumes.
#[utoipa::path(
    post,
    path = "/v1/player/play",
    tag = "player",
    request_body(content = PlayBody, description = "Что играть", content_type = "application/json"),
    responses(
        (status = 200, body = PlayAnswer),
        (status = 400, body = crate::error::ErrorBody),
        (status = 404, body = crate::error::ErrorBody),
        (status = 409, body = crate::error::ErrorBody),
        (status = 503, body = crate::error::ErrorBody),
        (status = 504, body = crate::error::ErrorBody),
    ),
    security(("bearer" = []))
)]
pub async fn play(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<PlayAnswer>, ApiError> {
    let b: PlayBody = body(&bytes)?;
    let kind = b.kind.unwrap_or_default();
    let mode = b.mode.unwrap_or_default();
    if kind == Kind::Wave && mode != Mode::Replace {
        return Err(play::wave_not_queued());
    }
    // The output first: a device that does not exist changes nothing.
    let output = match b.output.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
        Some(o) => Some(sh.output_command(o)?),
        None => None,
    };
    if output.is_none() {
        sh.audio()?;
    }
    let query = b.query.clone().unwrap_or_default();
    if b.id.is_none() && query.trim().is_empty() && !matches!(kind, Kind::Liked | Kind::Wave) {
        if sh.empty() {
            return Err(ApiError::nothing_to_play());
        }
        sh.run_by(Some(caller.actor), output.into_iter().chain([Command::Play]).collect()).await?;
        return Ok(Json(PlayAnswer { matched: None, state: sh.snapshot() }));
    }
    let lib = sh.lib.clone();
    let target = match b.id.clone() {
        Some(id) => blocking(move || play::exact(&lib, kind, &id)).await?,
        None => blocking(move || play::find(&lib, &query, kind)).await?,
    };
    // The shuffle asked for (or the kind's default) is set before the new
    // list is made: `play::commands` puts `ApiShuffle` first.
    let commands: Vec<Command> = output.into_iter().chain(play::commands(&target, play::shuffled(target.kind, b.shuffle), mode)?).collect();
    // Who plays: the loop keeps `caller.actor` while these run, for the
    // listening events they start once events have an actor.
    log::debug!("api: play {:?} by a token of {:?}", target.kind, caller.actor);
    sh.run_by(Some(caller.actor), commands).await?;
    Ok(Json(PlayAnswer { matched: Some(target.matched()), state: sh.snapshot() }))
}

#[utoipa::path(post, path = "/v1/player/pause", tag = "player", responses((status = 200, body = StateAnswer)), security(("bearer" = [])))]
pub async fn pause(State(sh): State<Arc<Shared>>) -> Result<Json<StateAnswer>, ApiError> {
    sh.answer(vec![Command::Pause]).await
}

#[utoipa::path(
    post,
    path = "/v1/player/resume",
    tag = "player",
    responses((status = 200, body = StateAnswer), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn resume(State(sh): State<Arc<Shared>>) -> Result<Json<StateAnswer>, ApiError> {
    sh.audio()?;
    if sh.empty() {
        return Err(ApiError::nothing_to_play());
    }
    sh.answer(vec![Command::Play]).await
}

#[utoipa::path(
    post,
    path = "/v1/player/toggle",
    tag = "player",
    responses((status = 200, body = StateAnswer), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn toggle(State(sh): State<Arc<Shared>>) -> Result<Json<StateAnswer>, ApiError> {
    // A pause needs no sound.
    if !sh.playing() {
        sh.audio()?;
    }
    if sh.empty() {
        return Err(ApiError::nothing_to_play());
    }
    sh.answer(vec![Command::TogglePlay]).await
}

#[utoipa::path(
    post,
    path = "/v1/player/next",
    tag = "player",
    responses((status = 200, body = StateAnswer), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn next(State(sh): State<Arc<Shared>>) -> Result<Json<StateAnswer>, ApiError> {
    sh.audio()?;
    if sh.empty() {
        return Err(ApiError::nothing_to_play());
    }
    sh.answer(vec![Command::Next]).await
}

/// Past 3 s of the song it starts again, as in the player.
#[utoipa::path(post, path = "/v1/player/previous", tag = "player", responses((status = 200, body = StateAnswer)), security(("bearer" = [])))]
pub async fn previous(State(sh): State<Arc<Shared>>) -> Result<Json<StateAnswer>, ApiError> {
    sh.audio()?;
    sh.answer(vec![Command::Previous]).await
}

/// `PUT /v1/player/volume`: one of the three.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct VolumeBody {
    /// 0..1.
    pub volume: Option<f32>,
    /// Added to the volume.
    pub delta: Option<f32>,
    pub muted: Option<bool>,
}

/// The commands that bring the volume (`now`, `muted`) to what `b` asks:
/// a volume above zero or a positive delta unmutes, anything else keeps
/// the mute as it is.
pub fn volume_commands(now: f32, muted: bool, b: &VolumeBody) -> Result<Vec<Command>, ApiError> {
    let given = usize::from(b.volume.is_some()) + usize::from(b.delta.is_some()) + usize::from(b.muted.is_some());
    if given != 1 {
        return Err(ApiError::bad_request("Нужно одно из: volume, delta, muted"));
    }
    if b.volume.is_some_and(|v| !v.is_finite()) || b.delta.is_some_and(|d| !d.is_finite()) {
        return Err(ApiError::bad_request("Громкость — число"));
    }
    let set = |v: f32, unmutes: bool| {
        let v = v.clamp(0.0, 1.0);
        // The player's slider unmutes; a mute that is to stay is put back.
        if muted && !unmutes { vec![Command::SetVolume(v), Command::ToggleMute] } else { vec![Command::SetVolume(v)] }
    };
    Ok(match (b.volume, b.delta, b.muted) {
        (Some(v), _, _) => set(v, v > 0.0),
        (_, Some(d), _) => set(now + d, d > 0.0),
        (_, _, Some(m)) if m != muted => vec![Command::ToggleMute],
        _ => Vec::new(),
    })
}

#[utoipa::path(
    put,
    path = "/v1/player/volume",
    tag = "player",
    request_body(content = VolumeBody, content_type = "application/json"),
    responses((status = 200, body = StateAnswer), (status = 400, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn volume(State(sh): State<Arc<Shared>>, bytes: Bytes) -> Result<Json<StateAnswer>, ApiError> {
    let b: VolumeBody = body(&bytes)?;
    let (now, muted) = {
        let st = sh.rt.state();
        (st.player.volume, st.player.muted)
    };
    let commands = volume_commands(now, muted, &b)?;
    sh.answer(commands).await
}

/// `PUT /v1/player/duck`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct DuckBody {
    /// 0..1, a factor on the volume.
    pub gain: Option<f32>,
    /// 0..5000, 120 by default.
    pub fade_ms: Option<u32>,
    /// 100..60000, 10000 by default.
    pub ttl_ms: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct DuckAnswer {
    pub duck: Option<DuckState>,
}

/// The duck asked for, inside its limits: (gain, fade_ms, ttl_ms).
pub fn duck_params(b: &DuckBody) -> Result<(f32, u32, u32), ApiError> {
    let gain = b.gain.filter(|g| g.is_finite()).ok_or_else(|| ApiError::bad_request("Нужен gain от 0 до 1"))?;
    Ok((gain.clamp(0.0, 1.0), b.fade_ms.unwrap_or(120).min(5000), b.ttl_ms.unwrap_or(10_000).clamp(100, 60_000)))
}

/// Lowers the sound for `ttl_ms`; a new one changes the gain and counts
/// the time again.
#[utoipa::path(
    put,
    path = "/v1/player/duck",
    tag = "player",
    request_body(content = DuckBody, content_type = "application/json"),
    responses((status = 200, body = DuckAnswer), (status = 400, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn duck(State(sh): State<Arc<Shared>>, bytes: Bytes) -> Result<Json<DuckAnswer>, ApiError> {
    let (gain, fade_ms, ttl_ms) = duck_params(&body(&bytes)?)?;
    sh.run(vec![Command::SetDuck { gain, fade_ms, ttl_ms }]).await?;
    // The duck as the player keeps it: its time counted from when it was set.
    let duck: Option<DuckState> = {
        let st = sh.rt.state();
        duck_of(&st, nikgob_library::now_ms())
    };
    Ok(Json(DuckAnswer { duck }))
}

/// Brings the sound back; harmless when it is not lowered.
#[utoipa::path(
    delete,
    path = "/v1/player/duck",
    tag = "player",
    params(("fade_ms" = Option<u32>, Query, description = "0–5000, по умолчанию 450")),
    responses((status = 200, body = DuckAnswer)),
    security(("bearer" = []))
)]
pub async fn unduck(State(sh): State<Arc<Shared>>, RawQuery(raw): RawQuery) -> Result<Json<DuckAnswer>, ApiError> {
    let fade_ms = match query_value(raw.as_deref(), "fade_ms") {
        Some(v) => v.trim().parse::<u32>().map_err(|_| ApiError::bad_request("fade_ms — целое число"))?.min(5000),
        None => 450,
    };
    sh.run(vec![Command::ClearDuck { fade_ms }]).await?;
    Ok(Json(DuckAnswer { duck: None }))
}

/// `POST /v1/player/seek`: one of the two.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct SeekBody {
    /// Inside the track.
    pub position_ms: Option<i64>,
    /// From where it is, as the listener counts (more of a faster track).
    pub delta_ms: Option<i64>,
}

#[utoipa::path(
    post,
    path = "/v1/player/seek",
    tag = "player",
    request_body(content = SeekBody, content_type = "application/json"),
    responses((status = 200, body = StateAnswer), (status = 400, body = crate::error::ErrorBody), (status = 409, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn seek(State(sh): State<Arc<Shared>>, bytes: Bytes) -> Result<Json<StateAnswer>, ApiError> {
    let b: SeekBody = body(&bytes)?;
    let c = match (b.position_ms, b.delta_ms) {
        (Some(p), None) => Command::Seek(Duration::from_millis(u64::try_from(p.max(0)).unwrap_or(0))),
        (None, Some(d)) => Command::SeekBy(d),
        _ => return Err(ApiError::bad_request("Нужно одно из: position_ms, delta_ms")),
    };
    if sh.empty() {
        return Err(ApiError::nothing_to_play());
    }
    sh.answer(vec![c]).await
}

/// `PUT /v1/player/mode`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct ModeBody {
    pub shuffle: Option<bool>,
    pub repeat: Option<RepeatMode>,
}

/// The button presses that bring shuffle and repeat to what `b` asks.
pub fn mode_commands(shuffle: bool, repeat: RepeatMode, b: &ModeBody) -> Vec<Command> {
    let mut out = Vec::new();
    if b.shuffle.is_some_and(|s| s != shuffle) {
        out.push(Command::ToggleShuffle);
    }
    if let Some(want) = b.repeat {
        let presses = (want.step() + 3 - repeat.step()) % 3;
        out.extend((0..presses).map(|_| Command::CycleRepeat));
    }
    out
}

#[utoipa::path(
    put,
    path = "/v1/player/mode",
    tag = "player",
    request_body(content = ModeBody, content_type = "application/json"),
    responses((status = 200, body = StateAnswer), (status = 400, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn mode(State(sh): State<Arc<Shared>>, bytes: Bytes) -> Result<Json<StateAnswer>, ApiError> {
    let b: ModeBody = body(&bytes)?;
    if b.shuffle.is_none() && b.repeat.is_none() {
        return Err(ApiError::bad_request("Нужно shuffle или repeat"));
    }
    let (shuffle, repeat) = {
        let st = sh.rt.state();
        (st.player.shuffle, RepeatMode::of(st.player.repeat))
    };
    sh.answer(mode_commands(shuffle, repeat, &b)).await
}

/// Where the sound goes.
#[utoipa::path(get, path = "/v1/outputs", tag = "outputs", responses((status = 200, body = Vec<Output>)), security(("bearer" = [])))]
pub async fn list_outputs(State(sh): State<Arc<Shared>>) -> Json<Vec<Output>> {
    let st = sh.rt.state();
    Json(outputs::list(&st.prefs.output_devices, st.prefs.output_device.as_deref()))
}

/// `PUT /v1/player/output`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct OutputBody {
    /// An id of `GET /v1/outputs`, a name or a part of one; on Linux also a
    /// PulseAudio sink name.
    pub output: Option<String>,
}

#[utoipa::path(
    put,
    path = "/v1/player/output",
    tag = "outputs",
    request_body(content = OutputBody, content_type = "application/json"),
    responses((status = 200, body = StateAnswer), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn set_output(State(sh): State<Arc<Shared>>, bytes: Bytes) -> Result<Json<StateAnswer>, ApiError> {
    let b: OutputBody = body(&bytes)?;
    let q = b.output.filter(|o| !o.trim().is_empty()).ok_or_else(|| ApiError::bad_request("Нужен output"))?;
    let c = sh.output_command(&q)?;
    sh.answer(vec![c]).await
}

/// One place of the queue.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct QueueItem {
    /// Its place's id (stable while it is in the queue).
    pub entry: u64,
    pub track: Track,
}

/// `GET /v1/queue`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct QueueAnswer {
    /// What played before (oldest first).
    pub prev: Vec<QueueItem>,
    pub current: Option<QueueItem>,
    /// In play order.
    pub next: Vec<QueueItem>,
}

pub fn queue_of(st: &ViewState) -> QueueAnswer {
    let q = &st.queue;
    let item = |e: &nikgob_view::QueueEntry| QueueItem { entry: e.entry, track: Track::of_view(&e.track) };
    QueueAnswer {
        prev: q.prev.iter().map(item).collect(),
        current: q.current.as_ref().zip(q.current_entry).map(|(t, entry)| QueueItem { entry, track: Track::of_view(t) }),
        next: q.next.iter().map(item).collect(),
    }
}

fn in_queue(st: &ViewState, entry: u64) -> bool {
    let q = &st.queue;
    q.current_entry == Some(entry) || q.prev.iter().chain(&q.next).any(|e| e.entry == entry)
}

impl Shared {
    fn queue_now(&self) -> QueueAnswer {
        let st = self.rt.state();
        queue_of(&st)
    }

    fn has_entry(&self, entry: u64) -> bool {
        let st = self.rt.state();
        in_queue(&st, entry)
    }
}

fn entry_of(s: &str) -> Result<u64, ApiError> {
    s.trim().parse().map_err(|_| ApiError::bad_request("Неверный номер места в очереди"))
}

#[utoipa::path(get, path = "/v1/queue", tag = "queue", responses((status = 200, body = QueueAnswer)), security(("bearer" = [])))]
pub async fn queue(State(sh): State<Arc<Shared>>) -> Json<QueueAnswer> {
    Json(sh.queue_now())
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum QueueMode {
    /// After the current track.
    Next,
    /// At the end.
    #[default]
    Enqueue,
}

/// `POST /v1/queue`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct QueueAdd {
    pub track_ids: Vec<String>,
    /// `enqueue` by default.
    pub mode: Option<QueueMode>,
}

#[utoipa::path(
    post,
    path = "/v1/queue",
    tag = "queue",
    request_body(content = QueueAdd, content_type = "application/json"),
    responses((status = 200, body = QueueAnswer), (status = 400, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn queue_add(State(sh): State<Arc<Shared>>, bytes: Bytes) -> Result<Json<QueueAnswer>, ApiError> {
    let b: QueueAdd = body(&bytes)?;
    if b.track_ids.is_empty() {
        return Err(ApiError::bad_request("Нужен track_ids"));
    }
    let ids: Vec<TrackId> =
        b.track_ids.iter().map(|s| s.trim().parse::<TrackId>()).collect::<Result<_, _>>().map_err(|_| ApiError::bad_request("Неверный id песни"))?;
    let c = match b.mode.unwrap_or_default() {
        QueueMode::Next => Command::PlayNext(ids),
        QueueMode::Enqueue => Command::Enqueue(ids),
    };
    sh.run(vec![c]).await?;
    Ok(Json(sh.queue_now()))
}

#[utoipa::path(
    post,
    path = "/v1/queue/{entry}/play",
    tag = "queue",
    params(("entry" = u64, Path, description = "Место в очереди")),
    responses((status = 200, body = StateAnswer), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn queue_play(State(sh): State<Arc<Shared>>, Path(entry): Path<String>) -> Result<Json<StateAnswer>, ApiError> {
    let entry = entry_of(&entry)?;
    if !sh.has_entry(entry) {
        return Err(ApiError::not_found("Такого места в очереди нет"));
    }
    sh.answer(vec![Command::PlayQueueEntry(entry)]).await
}

#[utoipa::path(
    delete,
    path = "/v1/queue/{entry}",
    tag = "queue",
    params(("entry" = u64, Path, description = "Место в очереди")),
    responses((status = 200, body = QueueAnswer), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn queue_remove(State(sh): State<Arc<Shared>>, Path(entry): Path<String>) -> Result<Json<QueueAnswer>, ApiError> {
    let entry = entry_of(&entry)?;
    if !sh.has_entry(entry) {
        return Err(ApiError::not_found("Такого места в очереди нет"));
    }
    sh.run(vec![Command::RemoveFromQueue(entry)]).await?;
    Ok(Json(sh.queue_now()))
}

/// `POST /v1/queue/move`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct QueueMove {
    pub entry: Option<u64>,
    pub anchor: Option<u64>,
    /// After the anchor (true by default) or before it.
    pub after: Option<bool>,
}

#[utoipa::path(
    post,
    path = "/v1/queue/move",
    tag = "queue",
    request_body(content = QueueMove, content_type = "application/json"),
    responses((status = 200, body = QueueAnswer), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn queue_move(State(sh): State<Arc<Shared>>, bytes: Bytes) -> Result<Json<QueueAnswer>, ApiError> {
    let b: QueueMove = body(&bytes)?;
    let (Some(entry), Some(anchor)) = (b.entry, b.anchor) else {
        return Err(ApiError::bad_request("Нужны entry и anchor"));
    };
    if !sh.has_entry(entry) || !sh.has_entry(anchor) {
        return Err(ApiError::not_found("Такого места в очереди нет"));
    }
    sh.run(vec![Command::MoveInQueue { entry, anchor, after: b.after.unwrap_or(true) }]).await?;
    Ok(Json(sh.queue_now()))
}

#[utoipa::path(delete, path = "/v1/queue", tag = "queue", responses((status = 200, body = QueueAnswer)), security(("bearer" = [])))]
pub async fn queue_clear(State(sh): State<Arc<Shared>>) -> Result<Json<QueueAnswer>, ApiError> {
    sh.run(vec![Command::ClearQueue]).await?;
    Ok(Json(sh.queue_now()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vol(volume: Option<f32>, delta: Option<f32>, muted: Option<bool>) -> VolumeBody {
        VolumeBody { volume, delta, muted }
    }

    #[test]
    fn volume_as_the_contract_says() {
        assert_eq!(volume_commands(0.5, false, &vol(Some(0.7), None, None)).unwrap(), vec![Command::SetVolume(0.7)]);
        assert_eq!(volume_commands(0.5, false, &vol(Some(1.7), None, None)).unwrap(), vec![Command::SetVolume(1.0)]);
        assert_eq!(volume_commands(0.5, true, &vol(Some(0.3), None, None)).unwrap(), vec![Command::SetVolume(0.3)], "above zero unmutes");
        assert_eq!(volume_commands(0.5, true, &vol(Some(0.0), None, None)).unwrap(), vec![Command::SetVolume(0.0), Command::ToggleMute]);
        assert_eq!(volume_commands(0.5, false, &vol(None, Some(-0.1), None)).unwrap(), vec![Command::SetVolume(0.4)]);
        assert_eq!(volume_commands(0.95, true, &vol(None, Some(0.1), None)).unwrap(), vec![Command::SetVolume(1.0)], "a positive delta unmutes");
        assert_eq!(volume_commands(0.5, true, &vol(None, Some(-0.1), None)).unwrap(), vec![Command::SetVolume(0.4), Command::ToggleMute]);
        assert_eq!(volume_commands(0.5, false, &vol(None, None, Some(true))).unwrap(), vec![Command::ToggleMute]);
        assert!(volume_commands(0.5, true, &vol(None, None, Some(true))).unwrap().is_empty());
        assert_eq!(volume_commands(0.5, false, &vol(None, None, None)).unwrap_err().code, "bad_request");
        assert_eq!(volume_commands(0.5, false, &vol(Some(0.1), Some(0.1), None)).unwrap_err().code, "bad_request");
        assert_eq!(volume_commands(0.5, false, &vol(Some(f32::NAN), None, None)).unwrap_err().code, "bad_request");
    }

    #[test]
    fn duck_limits() {
        let b = |gain, fade_ms, ttl_ms| DuckBody { gain, fade_ms, ttl_ms };
        assert_eq!(duck_params(&b(Some(0.18), Some(120), Some(8000))).unwrap(), (0.18, 120, 8000));
        assert_eq!(duck_params(&b(Some(0.03), None, None)).unwrap(), (0.03, 120, 10_000), "the defaults");
        assert_eq!(duck_params(&b(Some(2.0), Some(9000), Some(10))).unwrap(), (1.0, 5000, 100));
        assert_eq!(duck_params(&b(Some(-1.0), None, Some(999_999))).unwrap(), (0.0, 120, 60_000));
        assert!(duck_params(&b(None, None, None)).is_err());
    }

    #[test]
    fn shuffle_and_repeat_by_their_buttons() {
        let m = |shuffle, repeat| ModeBody { shuffle, repeat };
        assert_eq!(mode_commands(false, RepeatMode::Off, &m(Some(true), None)), vec![Command::ToggleShuffle]);
        assert!(mode_commands(true, RepeatMode::Off, &m(Some(true), None)).is_empty());
        assert_eq!(mode_commands(false, RepeatMode::Off, &m(None, Some(RepeatMode::One))), vec![Command::CycleRepeat, Command::CycleRepeat]);
        assert_eq!(mode_commands(false, RepeatMode::One, &m(None, Some(RepeatMode::Off))), vec![Command::CycleRepeat]);
        assert!(mode_commands(false, RepeatMode::All, &m(None, Some(RepeatMode::All))).is_empty());
    }

    #[test]
    fn the_queue_has_its_entries() {
        let mut st = ViewState::default();
        assert!(nothing_to_play(&st));
        let row = |t: &str| nikgob_view::TrackRow {
            id: TrackId::new(),
            title: t.into(),
            artist: "A".into(),
            album: None,
            release_id: None,
            artists: Default::default(),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: None,
            source: None,
            has_local: true,
            liked: false,
            cover: None,
            clip: false,
            in_library: true,
        };
        st.queue.current = Some(row("now"));
        st.queue.current_entry = Some(5);
        st.queue.next = vec![nikgob_view::QueueEntry { entry: 6, track: row("then"), why: None, guest: false }];
        assert!(!nothing_to_play(&st));
        let q = queue_of(&st);
        assert_eq!(q.current.as_ref().map(|c| c.entry), Some(5));
        assert_eq!(q.next[0].track.title, "then");
        assert_eq!(q.next[0].track.artists, vec!["A".to_owned()]);
        assert!(in_queue(&st, 6) && in_queue(&st, 5) && !in_queue(&st, 7));
    }
}
