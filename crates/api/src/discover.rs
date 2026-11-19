//! Music outside the library: the connected services searched (each
//! result marked when the library has it), an artist's releases the
//! library lacks, and a share page (song.link / MusicLink, as the app's
//! Share finds it). Every route here asks the network: a token may make a few a
//! minute ([`crate::auth::LOOKUPS_PER_S`]), each service asked one of them.
//! The tracks a search finds are handed to the loop (as the window's search
//! results are) and kept here too, so a like, a playlist or a download of
//! one acts on it later ([`stream_tracks`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::extract::{Path, RawQuery, State};
use axum::http::StatusCode;
use serde::Serialize;
use utoipa::ToSchema;

use nikgob_core::model::{ArtistId, ReleaseId, TrackId};
use nikgob_core::streaming::{ArtistRef, RemoteId, Service, StreamAlbum, StreamSearch, StreamTrack, TrackKind};
use nikgob_library::Library;
use nikgob_view::{Command, ShareTarget};

use crate::Shared;
use crate::auth::Caller;
use crate::error::{ApiError, Json, query_value};
use crate::library::{blocking, db};
use crate::mutate::remote_id;

/// How long one service may take to answer a search.
const SEARCH_WAIT: Duration = Duration::from_secs(15);
/// How long a discography lookup is waited for.
const DISCOGRAPHY_WAIT: Duration = Duration::from_secs(25);
/// How long a share link is waited for.
const SHARE_WAIT: Duration = Duration::from_secs(25);
/// Services one request may name at most.
const SERVICES_MAX: usize = 6;
/// Service tracks kept for later requests at most.
const SEEN_MAX: usize = 2000;
/// Service tracks one request may act on at most.
pub const REMOTE_TRACKS_MAX: usize = 50;

/// Service tracks the API found, the oldest dropped first.
#[derive(Default)]
pub struct Seen {
    map: HashMap<RemoteId, StreamTrack>,
    order: VecDeque<RemoteId>,
}

impl Seen {
    pub fn keep(&mut self, t: &StreamTrack) {
        if self.map.insert(t.id.clone(), t.clone()).is_none() {
            self.order.push_back(t.id.clone());
        }
        while self.order.len() > SEEN_MAX {
            if let Some(old) = self.order.pop_front() {
                self.map.remove(&old);
            }
        }
    }

    pub fn get(&self, r: &RemoteId) -> Option<StreamTrack> {
        self.map.get(r).cloned()
    }
}

/// The service tracks behind `remotes`, to hand to the loop
/// ([`Command::ApiKnow`]) before the commands that act on them: as a
/// search found them, else asked of their service (one lookup per
/// service). `404` naming the ones no service has; `400` over
/// [`REMOTE_TRACKS_MAX`].
pub async fn stream_tracks(sh: &Shared, caller: &Caller, remotes: &[RemoteId]) -> Result<Vec<StreamTrack>, ApiError> {
    if remotes.len() > REMOTE_TRACKS_MAX {
        return Err(ApiError::of(StatusCode::BAD_REQUEST, "too_many", format!("No more than {REMOTE_TRACKS_MAX} service tracks at a time")));
    }
    let mut found: Vec<StreamTrack> = Vec::new();
    let mut ask: HashMap<Service, Vec<String>> = HashMap::new();
    {
        let seen = sh.seen.lock();
        for r in remotes {
            match seen.get(r) {
                Some(t) => found.push(t),
                None => ask.entry(r.service).or_default().push(r.id.clone()),
            }
        }
    }
    for (service, ids) in ask {
        let Some(svc) = sh.rt.service(service) else {
            return Err(ApiError::of(StatusCode::SERVICE_UNAVAILABLE, "no_services", format!("{} is not connected", service.as_str())));
        };
        sh.lookup_allowed(caller)?;
        let got = match tokio::time::timeout(SEARCH_WAIT, svc.tracks(&ids)).await {
            Ok(Ok(got)) => got,
            Ok(Err(e)) => return Err(ApiError::of(StatusCode::BAD_GATEWAY, "lookup_failed", e.to_string())),
            Err(_) => return Err(ApiError::of(StatusCode::GATEWAY_TIMEOUT, "lookup_timeout", "The service did not answer in time")),
        };
        let mut seen = sh.seen.lock();
        for t in got.into_iter().flatten() {
            seen.keep(&t);
            found.push(t);
        }
    }
    let have: HashSet<&RemoteId> = found.iter().map(|t| &t.id).collect();
    let missing: Vec<String> = remotes.iter().filter(|r| !have.contains(r)).map(ToString::to_string).collect();
    if !missing.is_empty() {
        return Err(ApiError::not_found(format!("The service has none of these tracks: {}", missing.join(", "))));
    }
    Ok(found)
}

fn names(artists: &[ArtistRef]) -> Vec<String> {
    artists.iter().map(|a| a.name.clone()).filter(|n| !n.trim().is_empty()).collect()
}

fn kind_name(k: TrackKind) -> &'static str {
    match k {
        TrackKind::Song => "song",
        TrackKind::Video => "video",
        TrackKind::Upload => "upload",
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct FoundTrack {
    /// `service:id`.
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    /// `song`, `video` (a music video), `upload`.
    pub kind: String,
    pub in_library: bool,
    /// The library's track of it, when the library knows it.
    pub library_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct FoundAlbum {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub year: Option<i32>,
    pub release_date: Option<String>,
    /// `album`, `single`, `ep`, `compilation`, `other`.
    pub release_type: String,
    pub total_tracks: Option<u32>,
    pub in_library: bool,
    pub library_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct FoundArtist {
    pub id: String,
    pub name: String,
    pub in_library: bool,
    pub library_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct FoundPlaylist {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub total_tracks: Option<u32>,
}

/// One service's answer.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ServiceAnswer {
    /// `spotify`, `youtube`, `deezer`, `soundcloud`.
    pub service: String,
    /// Why it gave nothing (not connected, rate limited, no answer).
    pub error: Option<String>,
    pub tracks: Vec<FoundTrack>,
    pub albums: Vec<FoundAlbum>,
    pub artists: Vec<FoundArtist>,
    pub playlists: Vec<FoundPlaylist>,
}

/// `GET /v1/discover/search`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct DiscoverAnswer {
    pub query: String,
    pub services: Vec<ServiceAnswer>,
}

/// The services asked: those named (`spotify,deezer`; each once, at most
/// [`SERVICES_MAX`] names), else all.
pub fn services_of(raw: Option<&str>) -> Result<Vec<Service>, ApiError> {
    let Some(list) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Service::ALL.to_vec());
    };
    let names: Vec<&str> = list.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if names.len() > SERVICES_MAX {
        return Err(ApiError::bad_request(format!("No more than {SERVICES_MAX} services")));
    }
    let mut out = Vec::new();
    for s in names {
        let service = Service::parse(&s.to_ascii_lowercase()).ok_or_else(|| ApiError::bad_request(format!("No such service: {s}")))?;
        if !out.contains(&service) {
            out.push(service);
        }
    }
    Ok(out)
}

/// The library's track of a service's track, and whether it is in the
/// library (not only seen in a search or a mix).
fn library_track(lib: &Library, t: &StreamTrack) -> (bool, Option<TrackId>) {
    let id = lib.track_for_stream(t).ok().flatten();
    let inside = id.is_some_and(|id| lib.track_in_library(id).ok().flatten().unwrap_or(false));
    (inside, id)
}

/// A service's answer marked against the library (blocking).
pub fn mark(lib: &Library, service: Service, r: &StreamSearch, kind: &str, limit: usize) -> ServiceAnswer {
    let all = kind == "any" || kind.is_empty();
    let tracks = if all || kind == "track" {
        r.tracks
            .iter()
            .take(limit)
            .map(|t| {
                let (in_library, id) = library_track(lib, t);
                FoundTrack {
                    id: t.id.to_string(),
                    title: t.title.clone(),
                    artists: names(&t.artists),
                    album: t.album.as_ref().map(|a| a.title.clone()),
                    duration_ms: t.duration_ms,
                    kind: kind_name(t.kind).to_owned(),
                    in_library,
                    library_id: id.map(|i| i.to_string()),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let albums = if all || kind == "album" { r.albums.iter().take(limit).map(|a| album_of(lib, a)).collect() } else { Vec::new() };
    let artists = if all || kind == "artist" {
        r.artists
            .iter()
            .take(limit)
            .map(|a| {
                let id = lib.artist_by_remote(&a.id).ok().flatten();
                FoundArtist { id: a.id.to_string(), name: a.name.clone(), in_library: id.is_some(), library_id: id.map(|i| i.to_string()) }
            })
            .collect()
    } else {
        Vec::new()
    };
    let playlists = if all || kind == "playlist" {
        r.playlists
            .iter()
            .take(limit)
            .map(|p| FoundPlaylist { id: p.id.to_string(), name: p.name.clone(), owner: p.owner.clone(), total_tracks: p.total_tracks })
            .collect()
    } else {
        Vec::new()
    };
    ServiceAnswer { service: service.as_str().to_owned(), error: None, tracks, albums, artists, playlists }
}

/// A service's album, marked against the library.
pub fn album_of(lib: &Library, a: &StreamAlbum) -> FoundAlbum {
    let id: Option<ReleaseId> = lib.release_by_remote(&a.id).ok().flatten();
    FoundAlbum {
        id: a.id.to_string(),
        title: a.title.clone(),
        artists: names(&a.artists),
        year: a.year,
        release_date: a.release_date.clone(),
        release_type: a.release_type.to_string(),
        total_tracks: a.total_tracks,
        in_library: id.is_some(),
        library_id: id.map(|i| i.to_string()),
    }
}

/// Searches the connected services at once.
#[utoipa::path(
    get,
    path = "/v1/discover/search",
    operation_id = "discover_search",
    tag = "discover",
    params(
        ("q" = String, Query, description = "What to find"),
        ("kind" = Option<String>, Query, description = "any (default), track, album, artist, playlist"),
        ("services" = Option<String>, Query, description = "Comma-separated: spotify, youtube, deezer, soundcloud (all connected by default)"),
        ("limit" = Option<u32>, Query, description = "How many per group of each service, 1–25, 5 by default"),
    ),
    responses((status = 200, body = DiscoverAnswer), (status = 400, body = crate::error::ErrorBody), (status = 429, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn search(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery) -> Result<Json<DiscoverAnswer>, ApiError> {
    let raw = raw.as_deref();
    let q = query_value(raw, "q").map(|q| q.trim().to_owned()).filter(|q| !q.is_empty()).ok_or_else(|| ApiError::bad_request("The q parameter is required"))?;
    let kind = query_value(raw, "kind").unwrap_or_default().to_lowercase();
    if !["", "any", "track", "album", "artist", "playlist"].contains(&kind.as_str()) {
        return Err(ApiError::bad_request("kind: any, track, album, artist or playlist"));
    }
    let limit = query_value(raw, "limit").and_then(|l| l.trim().parse::<usize>().ok()).unwrap_or(5).clamp(1, 25);
    let asked = services_of(query_value(raw, "services").as_deref())?;
    let ready: Vec<_> = asked.iter().filter_map(|s| sh.rt.service(*s).map(|svc| (*s, svc))).collect();
    if ready.is_empty() {
        return Err(ApiError::of(StatusCode::SERVICE_UNAVAILABLE, "no_services", "None of these services is connected"));
    }
    // Each service asked is a lookup of its own.
    for _ in &ready {
        sh.lookup_allowed(&caller)?;
    }
    let asks = ready.into_iter().map(|(service, svc)| {
        let q = q.clone();
        async move {
            let r = match tokio::time::timeout(SEARCH_WAIT, svc.search(&q)).await {
                Ok(Ok(r)) => Ok(r),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("no answer in time".to_owned()),
            };
            (service, r)
        }
    });
    let answers = futures_util::future::join_all(asks).await;
    // The tracks found, for the requests that act on them: kept here and
    // handed to the loop as its own search results are.
    let tracks: Vec<StreamTrack> = answers.iter().filter_map(|(_, r)| r.as_ref().ok()).flat_map(|r| r.tracks.iter().take(limit).cloned()).collect();
    if !tracks.is_empty() {
        {
            let mut seen = sh.seen.lock();
            for t in &tracks {
                seen.keep(t);
            }
        }
        sh.run(vec![Command::ApiKnow(tracks)]).await?;
    }
    let lib = sh.lib.clone();
    let services = blocking(move || {
        Ok(answers
            .iter()
            .map(|(service, r)| match r {
                Ok(found) => mark(&lib, *service, found, &kind, limit),
                Err(e) => ServiceAnswer {
                    service: service.as_str().to_owned(),
                    error: Some(e.clone()),
                    tracks: Vec::new(),
                    albums: Vec::new(),
                    artists: Vec::new(),
                    playlists: Vec::new(),
                },
            })
            .collect())
    })
    .await?;
    Ok(Json(DiscoverAnswer { query: q, services }))
}

/// `GET /v1/discover/artists/{id}/discography`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct DiscographyAnswer {
    pub artist_id: String,
    pub artist: String,
    /// When the services were asked (unix ms).
    pub looked_up_ms: Option<i64>,
    /// Newer than a day (an hour when a service failed).
    pub fresh: bool,
    /// Still being looked up: what is shown may be older.
    pub pending: bool,
    pub error: Option<String>,
    /// Albums, then singles and EPs, compilations, the rest; newest first.
    pub missing: Vec<FoundAlbum>,
}

/// What a library artist released that the library lacks (as the
/// artist page's “Not in library” line shows it).
#[utoipa::path(
    get,
    path = "/v1/discover/artists/{id}/discography",
    tag = "discover",
    params(("id" = String, Path, description = "the library artist's id"), ("refresh" = Option<bool>, Query, description = "Ask the services again if the details are stale")),
    responses((status = 200, body = DiscographyAnswer), (status = 404, body = crate::error::ErrorBody), (status = 503, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn discography(
    State(sh): State<Arc<Shared>>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    RawQuery(raw): RawQuery,
) -> Result<Json<DiscographyAnswer>, ApiError> {
    let artist: ArtistId = id.trim().parse().map_err(|_| ApiError::bad_request("Invalid id"))?;
    let refresh = query_value(raw.as_deref(), "refresh").is_some_and(|v| v == "true" || v == "1");
    let lib = sh.lib.clone();
    let (name, kept) = blocking(move || {
        let a = lib.get_artist(artist).map_err(db)?.ok_or_else(|| ApiError::not_found("No such artist"))?;
        Ok((a.name, nikgob_runtime::kept_discography(&lib, artist)))
    })
    .await?;
    // Fresh: as kept. Kept but old: as kept unless asked to look again.
    let fresh = kept.as_ref().is_some_and(|k| k.fresh);
    if fresh || (kept.is_some() && !refresh) {
        let lib = sh.lib.clone();
        return blocking(move || Ok(discography_answer(artist, &name, kept.as_ref(), false, &lib))).await.map(Json);
    }
    let asked = [Service::Spotify, Service::Deezer, Service::Youtube].iter().filter(|s| sh.rt.service(**s).is_some()).count();
    if asked == 0 {
        return Err(ApiError::of(StatusCode::SERVICE_UNAVAILABLE, "no_services", "No connected services for the discography"));
    }
    // Each service the loop asks is a lookup.
    for _ in 0..asked {
        sh.lookup_allowed(&caller)?;
    }
    let before = kept.as_ref().map(|k| k.at);
    sh.run_by(Some(caller.actor), vec![Command::LoadDiscography(artist)]).await?;
    // The loop asks the services in the background and keeps the answer.
    let deadline = tokio::time::Instant::now() + DISCOGRAPHY_WAIT;
    loop {
        let lib = sh.lib.clone();
        let now = blocking(move || Ok(nikgob_runtime::kept_discography(&lib, artist))).await?;
        let new = now.as_ref().is_some_and(|k| Some(k.at) != before);
        if new || tokio::time::Instant::now() >= deadline {
            let lib = sh.lib.clone();
            let name = name.clone();
            return blocking(move || Ok(discography_answer(artist, &name, now.as_ref(), !new, &lib))).await.map(Json);
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

/// The answer of a kept discography (blocking: it marks the albums
/// against the library).
fn discography_answer(artist: ArtistId, name: &str, kept: Option<&nikgob_runtime::KeptDiscography>, pending: bool, lib: &Library) -> DiscographyAnswer {
    DiscographyAnswer {
        artist_id: artist.to_string(),
        artist: name.to_owned(),
        looked_up_ms: kept.map(|k| k.at),
        fresh: kept.is_some_and(|k| k.fresh),
        pending,
        error: kept.and_then(|k| k.error.clone()),
        missing: kept.map(|k| k.missing.iter().take(100).map(|a| album_of(lib, a)).collect()).unwrap_or_default(),
    }
}

/// `GET /v1/discover/share`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ShareAnswer {
    /// The page with every service (or the service's own page).
    pub link: Option<String>,
    /// “Artist — Title”.
    pub text: String,
    /// The link opens in any service.
    pub everywhere: bool,
}

/// A share page of a track or an album: a library id or `service:id`.
pub fn share_target(kind: &str, id: &str) -> Result<ShareTarget, ApiError> {
    if let Some(r) = remote_id(id) {
        return match kind {
            "album" => Ok(ShareTarget::StreamAlbum(r)),
            _ => Ok(ShareTarget::StreamTrack(r)),
        };
    }
    let bad = || ApiError::bad_request("Invalid id");
    match kind {
        "album" => id.trim().parse().map(ShareTarget::Release).map_err(|_| bad()),
        "track" | "" => id.trim().parse().map(ShareTarget::Track).map_err(|_| bad()),
        _ => Err(ApiError::bad_request("kind: track or album")),
    }
}

/// A song.link / album.link page (MusicLink first), as the app's Share finds
/// it; nothing is copied and no toast shows.
#[utoipa::path(
    get,
    path = "/v1/discover/share",
    tag = "discover",
    params(("kind" = Option<String>, Query, description = "track (default) or album"), ("id" = String, Query, description = "the library's id or service:id")),
    responses((status = 200, body = ShareAnswer), (status = 400, body = crate::error::ErrorBody), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn share(State(sh): State<Arc<Shared>>, Extension(caller): Extension<Caller>, RawQuery(raw): RawQuery) -> Result<Json<ShareAnswer>, ApiError> {
    let raw = raw.as_deref();
    let id = query_value(raw, "id").filter(|i| !i.trim().is_empty()).ok_or_else(|| ApiError::bad_request("An id is required"))?;
    let target = share_target(query_value(raw, "kind").unwrap_or_default().trim(), &id)?;
    let lib = sh.lib.clone();
    let t = target.clone();
    blocking(move || {
        let there = match &t {
            ShareTarget::Track(id) => lib.get_track_row(*id).map_err(db)?.is_some(),
            ShareTarget::Release(id) => lib.get_release(*id).map_err(db)?.is_some(),
            _ => true,
        };
        if there { Ok(()) } else { Err(ApiError::not_found("Not found")) }
    })
    .await?;
    sh.lookup_allowed(&caller)?;
    let v = sh.ask(Some(caller.actor), SHARE_WAIT, move |reply| Command::ApiShare { target, reply }).await?;
    Ok(Json(ShareAnswer {
        link: v.get("link").and_then(|l| l.as_str()).map(str::to_owned),
        text: v.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_owned(),
        everywhere: v.get("everywhere").and_then(serde_json::Value::as_bool).unwrap_or(false),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_by_name() {
        assert_eq!(services_of(None).unwrap(), Service::ALL.to_vec());
        assert_eq!(services_of(Some("Deezer, youtube")).unwrap(), vec![Service::Deezer, Service::Youtube]);
        assert!(services_of(Some("napster")).is_err());
        assert_eq!(services_of(Some("deezer,deezer,DEEZER")).unwrap(), vec![Service::Deezer], "each once");
        assert!(services_of(Some("deezer,deezer,deezer,deezer,deezer,deezer,deezer")).is_err(), "too many names");
    }

    #[test]
    fn the_seen_tracks_are_bounded() {
        let mut seen = Seen::default();
        for i in 0..SEEN_MAX + 10 {
            seen.keep(&StreamTrack::new(RemoteId::new(Service::Deezer, i.to_string()), "S"));
        }
        assert_eq!(seen.map.len(), SEEN_MAX);
        assert!(seen.get(&RemoteId::new(Service::Deezer, "0")).is_none(), "the oldest went");
        assert!(seen.get(&RemoteId::new(Service::Deezer, (SEEN_MAX + 9).to_string())).is_some());
    }

    #[test]
    fn share_targets() {
        let t = TrackId::new();
        assert_eq!(share_target("", &t.to_string()).unwrap(), ShareTarget::Track(t));
        assert_eq!(share_target("album", "deezer:42").unwrap(), ShareTarget::StreamAlbum(RemoteId::new(Service::Deezer, "42")));
        assert_eq!(share_target("track", "spotify:x").unwrap(), ShareTarget::StreamTrack(RemoteId::new(Service::Spotify, "x")));
        assert!(share_target("artist", &t.to_string()).is_err());
        assert!(share_target("track", "nope").is_err());
    }

    /// A service's rows are marked when the library has them.
    #[test]
    fn results_are_marked_against_the_library() {
        let lib = Library::open_in_memory().unwrap();
        let mut found = StreamSearch::default();
        let mut t = StreamTrack::new(RemoteId::new(Service::Deezer, "1"), "Song");
        t.artists = vec![ArtistRef::new(None, "A")];
        found.tracks = vec![t.clone(), t];
        let a = mark(&lib, Service::Deezer, &found, "any", 1);
        assert_eq!(a.tracks.len(), 1, "the limit");
        assert_eq!(a.tracks[0].id, "deezer:1");
        assert!(!a.tracks[0].in_library);
        assert_eq!(a.tracks[0].kind, "song");
        assert!(mark(&lib, Service::Deezer, &found, "album", 5).tracks.is_empty());
    }
}
