//! Downloads: what runs, what waits in «Хочу скачать» and in «Подтверди»,
//! a dry search of the download sources, and a request to download. A
//! small request (a few releases of a sure match, not too big) goes to
//! «Хочу скачать» at once, as the window's «Скачать» does; anything larger
//! waits as a card in the app's own «Подтверди» for the owner's click.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use nikgob_core::model::{ReleaseId, TrackId, WishlistStatus};
use nikgob_core::streaming::RemoteId;
use nikgob_view::{Command, ViewState};

use crate::Shared;
use crate::auth::Caller;
use crate::error::{ApiError, Json, body, query_value};
use crate::library::{blocking, db};
use crate::mutate::remote_id;

/// A request of at most this many releases may start by itself…
pub const SMALL_RELEASES: usize = 3;
/// …of at most this many bytes (as estimated)…
pub const SMALL_BYTES: u64 = 1_500 * 1024 * 1024;
/// …estimated as lossless files: bytes a second of music.
const BYTES_PER_S: u64 = 110_000;
/// A song of unknown length counts as this long.
const SONG_S: u64 = 240;
/// The dry search of the sources is waited for this long.
const SOURCES_WAIT: Duration = Duration::from_secs(75);
/// A service's album is asked for at most this long.
const ALBUM_WAIT: Duration = Duration::from_secs(10);
/// Items of «Хочу скачать» listed at most.
const WISHLIST_SHOWN: usize = 50;

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct JobOut {
    pub id: u64,
    pub title: String,
    pub subtitle: String,
    /// 0..1; none while it cannot be told.
    pub progress: Option<f32>,
    pub rate: Option<String>,
    pub eta: Option<String>,
    pub paused: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ConfirmOut {
    pub id: u64,
    pub title: String,
    pub origin: String,
    pub question: String,
    pub options: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct WishOut {
    pub id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    /// `waiting`, `new`, `searching`, `confirm`, `downloading`,
    /// `no_torrent`, `failed`…
    pub status: String,
    /// `track` or `album`.
    pub scope: String,
    pub note: Option<String>,
    pub track_id: Option<String>,
    pub album_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StatusCount {
    pub status: String,
    pub count: u32,
}

/// `GET /v1/downloads`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct DownloadsAnswer {
    pub active: Vec<JobOut>,
    /// «Подтверди»: cards waiting for the owner in the app.
    pub confirm: Vec<ConfirmOut>,
    /// «Хочу скачать» by state.
    pub wishlist_counts: Vec<StatusCount>,
    /// Its items not done yet, newest first (the first 50).
    pub wishlist: Vec<WishOut>,
}

fn downloads_of(st: &ViewState) -> (Vec<JobOut>, Vec<ConfirmOut>) {
    let d = &st.downloads;
    let active = d
        .active
        .iter()
        .map(|j| JobOut {
            id: j.id,
            title: j.title.clone(),
            subtitle: j.subtitle.clone(),
            progress: j.progress,
            rate: j.rate.clone(),
            eta: j.eta.clone(),
            paused: j.paused,
            error: j.error.clone(),
        })
        .collect();
    let confirm = d
        .confirm
        .iter()
        .map(|c| ConfirmOut {
            id: c.id,
            title: c.title.clone(),
            origin: c.origin.clone(),
            question: c.question.clone(),
            options: c.options.iter().map(|o| if o.detail.is_empty() { o.label.clone() } else { format!("{} — {}", o.label, o.detail) }).collect(),
        })
        .collect();
    (active, confirm)
}

/// What downloads, what waits for a file, what waits for the owner.
#[utoipa::path(get, path = "/v1/downloads", tag = "downloads", responses((status = 200, body = DownloadsAnswer)), security(("bearer" = [])))]
pub async fn inspect(State(sh): State<Arc<Shared>>) -> Result<Json<DownloadsAnswer>, ApiError> {
    let (active, confirm) = {
        let st = sh.rt.state();
        downloads_of(&st)
    };
    let lib = sh.lib.clone();
    let mut items = blocking(move || lib.list_wishlist().map_err(db)).await?;
    let mut counts: Vec<StatusCount> = Vec::new();
    for i in &items {
        let s = i.status.as_str();
        match counts.iter_mut().find(|c| c.status == s) {
            Some(c) => c.count += 1,
            None => counts.push(StatusCount { status: s.to_owned(), count: 1 }),
        }
    }
    items.retain(|i| i.status != WishlistStatus::Done);
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let wishlist = items
        .iter()
        .take(WISHLIST_SHOWN)
        .map(|i| WishOut {
            id: i.id.to_string(),
            title: i.title.clone(),
            artist: i.artist.clone(),
            status: i.status.as_str().to_owned(),
            scope: i.scope.as_str().to_owned(),
            note: i.note.clone(),
            track_id: i.track_id.map(|t| t.to_string()),
            album_id: i.release_id.map(|r| r.to_string()),
        })
        .collect();
    Ok(Json(DownloadsAnswer { active, confirm, wishlist_counts: counts, wishlist }))
}

/// `GET /v1/downloads/sources`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct SourcesAnswer {
    /// What was searched for, the sources asked and those that failed,
    /// how many offers came and match, the best of them.
    #[schema(value_type = Object)]
    pub found: Value,
}

/// Searches the download sources for a library track or album without
/// downloading anything (slow: the torrent indexer can take a minute).
#[utoipa::path(
    get,
    path = "/v1/downloads/sources",
    tag = "downloads",
    params(("track_id" = Option<String>, Query, description = "id песни"), ("album_id" = Option<String>, Query, description = "id альбома")),
    responses((status = 200, body = SourcesAnswer), (status = 400, body = crate::error::ErrorBody), (status = 429, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn sources(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery) -> Result<Json<SourcesAnswer>, ApiError> {
    let raw = raw.as_deref();
    let bad = || ApiError::bad_request("Неверный id");
    let track: Option<TrackId> = query_value(raw, "track_id").filter(|s| !s.trim().is_empty()).map(|s| s.trim().parse().map_err(|_| bad())).transpose()?;
    let release: Option<ReleaseId> =
        query_value(raw, "album_id").filter(|s| !s.trim().is_empty()).map(|s| s.trim().parse().map_err(|_| bad())).transpose()?;
    if track.is_none() == release.is_none() {
        return Err(ApiError::bad_request("Нужен один из: track_id, album_id"));
    }
    let lib = sh.lib.clone();
    blocking(move || {
        let there = match (track, release) {
            (Some(t), _) => lib.get_track_row(t).map_err(db)?.is_some(),
            (None, Some(r)) => lib.get_release(r).map_err(db)?.is_some(),
            _ => false,
        };
        if there { Ok(()) } else { Err(ApiError::not_found("Ничего не нашлось")) }
    })
    .await?;
    sh.lookup_allowed(&caller)?;
    let found = sh.ask(Some(caller.actor), SOURCES_WAIT, move |reply| Command::ApiSourceSearch { track, release: if track.is_some() { None } else { release }, reply }).await?;
    Ok(Json(SourcesAnswer { found }))
}

/// `POST /v1/downloads`.
#[derive(Clone, Debug, Default, Deserialize, ToSchema)]
pub struct EnqueueBody {
    /// Library tracks (those with a file are left out).
    #[serde(default)]
    pub track_ids: Vec<String>,
    /// Library albums: their songs without a file.
    #[serde(default)]
    pub album_ids: Vec<String>,
    /// Service tracks, `service:id`.
    #[serde(default)]
    pub remote_track_ids: Vec<String>,
    /// Service albums, `service:id`.
    #[serde(default)]
    pub remote_album_ids: Vec<String>,
}

/// `POST /v1/downloads`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct EnqueueAnswer {
    /// `queued` (in «Хочу скачать»), `queued_in_confirm` (waits for the
    /// owner in «Подтверди») or `nothing_to_do`.
    pub status: String,
    /// The «Подтверди» card, when it waits there.
    pub confirm_id: Option<u64>,
    /// What it asks for, one line each.
    pub items: Vec<String>,
    pub releases: usize,
    pub tracks: usize,
    pub estimated_bytes: u64,
    /// Library tracks left out: they have a file already.
    pub already_here: Vec<String>,
}

/// One thing a download request asks for.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub line: String,
    /// Songs it brings, and their length (ms) where known.
    pub tracks: usize,
    pub known_ms: u64,
    pub unknown: usize,
    /// It is a release of its own (an album, or a song of an album the
    /// request has not counted yet).
    pub release: Option<String>,
    /// What starts it (one command may stand for several wants).
    pub command: Option<Command>,
}

/// The estimate and whether it may start by itself.
pub fn estimate(wants: &[Want]) -> (usize, usize, u64, bool) {
    let releases: HashSet<&String> = wants.iter().filter_map(|w| w.release.as_ref()).collect();
    let releases = releases.len();
    let tracks: usize = wants.iter().map(|w| w.tracks).sum();
    let secs: u64 = wants.iter().map(|w| w.known_ms / 1000 + w.unknown as u64 * SONG_S).sum();
    let bytes = secs * BYTES_PER_S;
    (releases, tracks, bytes, releases <= SMALL_RELEASES && bytes <= SMALL_BYTES)
}

/// Asks the player to download songs and albums: at once when the request
/// is small, else as a card in «Подтверди» for the owner.
#[utoipa::path(
    post,
    path = "/v1/downloads",
    tag = "downloads",
    request_body(content = EnqueueBody, content_type = "application/json"),
    responses((status = 200, body = EnqueueAnswer), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn enqueue(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, bytes: Bytes) -> Result<Json<EnqueueAnswer>, ApiError> {
    let b: EnqueueBody = body(&bytes)?;
    let bad = |what: &str, s: &str| ApiError::bad_request(format!("Неверный id {what}: {s}"));
    let tracks: Vec<TrackId> = b.track_ids.iter().map(|s| s.trim().parse().map_err(|_| bad("песни", s))).collect::<Result<_, _>>()?;
    let albums: Vec<ReleaseId> = b.album_ids.iter().map(|s| s.trim().parse().map_err(|_| bad("альбома", s))).collect::<Result<_, _>>()?;
    let remote_tracks: Vec<RemoteId> = b.remote_track_ids.iter().map(|s| remote_id(s).ok_or_else(|| bad("сервиса", s))).collect::<Result<_, _>>()?;
    let remote_albums: Vec<RemoteId> = b.remote_album_ids.iter().map(|s| remote_id(s).ok_or_else(|| bad("сервиса", s))).collect::<Result<_, _>>()?;
    if tracks.is_empty() && albums.is_empty() && remote_tracks.is_empty() && remote_albums.is_empty() {
        return Err(ApiError::bad_request("Нечего скачивать"));
    }
    let lib = sh.lib.clone();
    let (mut wants, already) = blocking(move || {
        let mut wants = Vec::new();
        let mut already = Vec::new();
        let rows = lib.get_track_rows(&tracks).map_err(db)?;
        let mut missing = Vec::new();
        for t in &tracks {
            let r = rows.get(t).ok_or_else(|| ApiError::not_found(format!("Нет такой песни: {t}")))?;
            if r.has_local {
                already.push(format!("{} — {}", r.artist, r.title));
                continue;
            }
            missing.push(*t);
            wants.push(Want {
                line: format!("{} — {}", r.artist, r.title),
                tracks: 1,
                known_ms: r.duration_ms.map_or(0, u64::from),
                unknown: usize::from(r.duration_ms.is_none()),
                release: Some(r.release_id.map_or_else(|| t.to_string(), |r| r.to_string())),
                command: None,
            });
        }
        // One command for the songs: the window's «Скачать» of a selection.
        if let Some(first) = wants.first_mut() {
            first.command = Some(Command::DownloadTracks(missing));
        }
        for a in &albums {
            let row = lib.get_release_row(*a).map_err(db)?.ok_or_else(|| ApiError::not_found(format!("Нет такого альбома: {a}")))?;
            let songs = lib.tracks_of_release(*a).map_err(db)?;
            let lacking: Vec<_> = songs.iter().filter(|t| !t.has_local).collect();
            if lacking.is_empty() {
                already.push(format!("{} — {}", row.release.artist_credit.display(), row.release.title));
                continue;
            }
            wants.push(Want {
                line: format!("{} — {} ({})", row.release.artist_credit.display(), row.release.title, crate::mutate::tracks_word(lacking.len())),
                tracks: lacking.len(),
                known_ms: lacking.iter().filter_map(|t| t.duration_ms).map(u64::from).sum(),
                unknown: lacking.iter().filter(|t| t.duration_ms.is_none()).count(),
                release: Some(a.to_string()),
                command: Some(Command::DownloadRelease(*a)),
            });
        }
        Ok((wants, already))
    })
    .await?;
    if !remote_tracks.is_empty() {
        let n = remote_tracks.len();
        wants.push(Want {
            line: format!("{} из сервисов", crate::mutate::tracks_word(n)),
            tracks: n,
            known_ms: 0,
            unknown: n,
            release: None,
            command: Some(Command::DownloadStream { tracks: remote_tracks.clone() }),
        });
        // Each song of a service counts as a release of its own.
        for r in &remote_tracks {
            wants.push(Want { line: String::new(), tracks: 0, known_ms: 0, unknown: 0, release: Some(r.to_string()), command: None });
        }
    }
    if !remote_albums.is_empty() {
        sh.lookup_allowed(&caller)?;
    }
    for r in remote_albums {
        // Its size from the service, when it answers in time.
        let album = match sh.rt.service(r.service) {
            Some(svc) => tokio::time::timeout(ALBUM_WAIT, svc.album(&r.id)).await.ok().and_then(Result::ok),
            None => None,
        };
        let (line, n, known, unknown) = match &album {
            Some(a) => {
                let n = if a.tracks.is_empty() { a.total_tracks.unwrap_or(10) as usize } else { a.tracks.len() };
                let known: u64 = a.tracks.iter().filter_map(|t| t.duration_ms).sum();
                let unknown = if a.tracks.is_empty() { n } else { a.tracks.iter().filter(|t| t.duration_ms.is_none()).count() };
                (format!("{} — {} ({})", album_artists(a), a.title, crate::mutate::tracks_word(n)), n, known, unknown)
            }
            None => (format!("Альбом {r}"), 12, 0, 12),
        };
        wants.push(Want { line, tracks: n, known_ms: known, unknown, release: Some(r.to_string()), command: Some(Command::DownloadStreamAlbum(r)) });
    }
    let (releases, track_count, estimated, small) = estimate(&wants);
    let items: Vec<String> = wants.iter().filter(|w| !w.line.is_empty()).map(|w| w.line.clone()).collect();
    let commands: Vec<Command> = wants.into_iter().filter_map(|w| w.command).collect();
    let answer = |status: &str, confirm_id| EnqueueAnswer {
        status: status.to_owned(),
        confirm_id,
        items: items.clone(),
        releases,
        tracks: track_count,
        estimated_bytes: estimated,
        already_here: already.clone(),
    };
    if commands.is_empty() {
        return Ok(Json(answer("nothing_to_do", None)));
    }
    if small {
        sh.run_by(Some(caller.actor), commands).await?;
        return Ok(Json(answer("queued", None)));
    }
    let title = match items.as_slice() {
        [one] => one.clone(),
        [first, rest @ ..] => format!("{first} и ещё {}", rest.len()),
        [] => "Загрузка".to_owned(),
    };
    let origin = format!("Запрос «{}»", caller.name);
    sh.run_by(Some(caller.actor), vec![Command::ApiConfirmDownload { title, origin, lines: items.clone(), commands }]).await?;
    // The card just made: the newest of the API's.
    let confirm_id = sh.rt.state().downloads.confirm.iter().map(|c| c.id).filter(|id| id & (1 << 50) != 0).max();
    Ok(Json(answer("queued_in_confirm", confirm_id)))
}

/// A service album's artists as one line.
fn album_artists(a: &nikgob_core::streaming::StreamAlbum) -> String {
    {
        let names: Vec<&str> = a.artists.iter().map(|a| a.name.as_str()).filter(|n| !n.trim().is_empty()).collect();
        if names.is_empty() { "?".to_owned() } else { names.join(", ") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(tracks: usize, ms: u64, release: Option<&str>) -> Want {
        Want { line: "x".into(), tracks, known_ms: ms, unknown: 0, release: release.map(str::to_owned), command: None }
    }

    #[test]
    fn a_small_request_starts_and_a_large_one_waits() {
        // Two albums of 40 minutes: small.
        let (r, t, bytes, small) = estimate(&[want(10, 2_400_000, Some("a")), want(10, 2_400_000, Some("b"))]);
        assert_eq!((r, t), (2, 20));
        assert_eq!(bytes, 4800 * BYTES_PER_S);
        assert!(small);
        // Four releases: waits.
        let four: Vec<Want> = ["a", "b", "c", "d"].iter().map(|id| want(1, 200_000, Some(*id))).collect();
        assert!(!estimate(&four).3);
        // Songs of one album count once.
        let same = [want(1, 200_000, Some("a")), want(1, 200_000, Some("a")), want(1, 200_000, Some("a")), want(1, 200_000, Some("a"))];
        assert_eq!(estimate(&same).0, 1);
        // One huge box set: waits.
        assert!(!estimate(&[want(300, 300 * 300_000, Some("box"))]).3);
        // Unknown lengths count as four minutes each.
        let w = Want { unknown: 3, ..want(3, 0, Some("a")) };
        assert_eq!(estimate(&[w]).2, 3 * SONG_S * BYTES_PER_S);
    }
}
