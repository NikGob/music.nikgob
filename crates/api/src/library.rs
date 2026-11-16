//! Reading the library: the search, a track, an album, an artist, the
//! playlists and a song's text. Every read runs on the blocking pool (the
//! library's own read connections), never on the server's threads.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, RawQuery, State};
use serde::Serialize;
use utoipa::ToSchema;

use nikgob_core::model::{ArtistId, LikeTarget, PlaylistId, ReleaseId, TrackId};
use nikgob_library::{Library, LibraryError, ReleaseRow, SearchResults};
use nikgob_view::LyricLine;

use crate::Shared;
use crate::error::{ApiError, Json, query_value};

/// A library read on the blocking pool.
pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError::internal(e.to_string()))?
}

pub(crate) fn db(e: LibraryError) -> ApiError {
    ApiError::internal(e.to_string())
}

fn gone() -> ApiError {
    ApiError::not_found("Ничего не нашлось")
}

fn bad_id() -> ApiError {
    ApiError::bad_request("Неверный id")
}

/// The id of «Любимые» in the API.
pub const LIKED_ID: &str = "liked";

fn ms(v: u32) -> i64 {
    i64::from(v)
}

fn cover_url(release: Option<ReleaseId>, has_cover: bool) -> Option<String> {
    release.filter(|_| has_cover).map(|id| format!("/v1/media/albums/{id}/cover"))
}

fn names(credit: &nikgob_core::model::ArtistCredit, display: &str) -> Vec<String> {
    let names: Vec<String> = credit.0.iter().map(|p| p.name.clone()).filter(|n| !n.trim().is_empty()).collect();
    if names.is_empty() && !display.trim().is_empty() { vec![display.to_owned()] } else { names }
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub duration_ms: Option<i64>,
    pub liked: bool,
    /// It has a file here (`GET /v1/media/tracks/{id}/audio`).
    pub local: bool,
    /// The codec of what plays (`flac`, `opus`, `mp3`…).
    pub codec: Option<String>,
    /// `/v1/media/albums/{id}/cover` when the album has a cover.
    pub cover: Option<String>,
}

impl Track {
    pub fn of_row(r: &nikgob_library::TrackRow) -> Self {
        Self {
            id: r.id.to_string(),
            title: r.title.clone(),
            artists: names(&r.credit, &r.artist),
            album: r.album.clone(),
            album_id: r.release_id.map(|id| id.to_string()),
            duration_ms: r.duration_ms.map(ms),
            liked: r.liked,
            local: r.has_local,
            codec: r.codec.as_ref().map(|c| c.as_str().to_owned()),
            cover: cover_url(r.release_id, r.cover_ref.is_some()),
        }
    }

    pub fn of_view(r: &nikgob_view::TrackRow) -> Self {
        Self {
            id: r.id.to_string(),
            title: r.title.clone(),
            artists: crate::state::artists_of(r),
            album: r.album.clone(),
            album_id: r.release_id.map(|id| id.to_string()),
            duration_ms: r.duration.map(|d: Duration| i64::try_from(d.as_millis()).unwrap_or(i64::MAX)),
            liked: r.liked,
            local: r.has_local,
            codec: None,
            cover: cover_url(r.release_id, r.cover.as_ref().is_some_and(|c| c.file().is_some())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Album {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub year: Option<i32>,
    /// `album`, `single`, `ep`, `compilation`, `other`.
    pub release_type: String,
    /// Songs of it the library knows.
    pub tracks_known: u32,
    /// Of them, the ones with a file here.
    pub tracks_local: u32,
    pub liked: bool,
    pub cover: Option<String>,
}

impl Album {
    pub fn of_row(r: &ReleaseRow) -> Self {
        let rel = &r.release;
        let display = rel.artist_credit.display();
        Self {
            id: rel.id.to_string(),
            title: rel.title.clone(),
            artists: names(&rel.artist_credit, &display),
            year: rel.year,
            release_type: rel.release_type.to_string(),
            tracks_known: r.known_tracks,
            tracks_local: r.local_tracks,
            liked: r.liked,
            cover: cover_url(Some(rel.id), rel.cover_ref.is_some()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct AlbumDetail {
    pub album: Album,
    /// In album order.
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Artist {
    pub id: String,
    pub name: String,
    pub liked: bool,
    pub releases: u32,
    /// Songs of theirs with a file here.
    pub tracks_local: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ArtistDetail {
    pub artist: Artist,
    /// Newest first.
    pub albums: Vec<Album>,
    /// Their songs in the library, by album (the first 200).
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Playlist {
    /// `liked` for «Любимые».
    pub id: String,
    pub name: String,
    pub track_count: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    /// In its order (the first 5000).
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, ToSchema)]
pub struct SearchAnswer {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
}

fn playlist_of(p: &nikgob_library::PlaylistSummary) -> Playlist {
    Playlist { id: p.id.map_or_else(|| LIKED_ID.to_owned(), |id| id.to_string()), name: p.name.clone(), track_count: p.track_count }
}

/// The search's answer, of `kind` only (`any`: everything).
pub fn search_answer(r: &SearchResults, kind: &str) -> SearchAnswer {
    let all = kind == "any" || kind.is_empty();
    SearchAnswer {
        tracks: if all || kind == "track" { r.tracks.iter().map(Track::of_row).collect() } else { Vec::new() },
        albums: if all || kind == "album" { r.releases.iter().map(Album::of_row).collect() } else { Vec::new() },
        artists: if all || kind == "artist" {
            r.artists
                .iter()
                .map(|a| Artist {
                    id: a.artist.id.to_string(),
                    name: a.artist.name.clone(),
                    liked: a.liked,
                    releases: a.release_count,
                    tracks_local: a.local_track_count,
                })
                .collect()
        } else {
            Vec::new()
        },
        playlists: if all || kind == "playlist" { r.playlists.iter().map(playlist_of).collect() } else { Vec::new() },
    }
}

/// `GET /v1/search?q=&kind=&limit=`.
#[utoipa::path(
    get,
    path = "/v1/search",
    tag = "library",
    params(
        ("q" = String, Query, description = "Что искать"),
        ("kind" = Option<String>, Query, description = "any (по умолчанию), track, album, artist, playlist"),
        ("limit" = Option<u32>, Query, description = "Сколько в каждой группе, 1–50, по умолчанию 10"),
    ),
    responses((status = 200, body = SearchAnswer), (status = 400, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn search(State(sh): State<Arc<Shared>>, RawQuery(raw): RawQuery) -> Result<Json<SearchAnswer>, ApiError> {
    let q = query_value(raw.as_deref(), "q").unwrap_or_default();
    if q.trim().is_empty() {
        return Err(ApiError::bad_request("Нужен параметр q"));
    }
    let kind = query_value(raw.as_deref(), "kind").unwrap_or_default().to_lowercase();
    if !["", "any", "track", "album", "artist", "playlist"].contains(&kind.as_str()) {
        return Err(ApiError::bad_request("kind: any, track, album, artist или playlist"));
    }
    let limit = query_value(raw.as_deref(), "limit").and_then(|l| l.parse::<u32>().ok()).unwrap_or(10).clamp(1, 50);
    let lib = sh.lib.clone();
    let found = blocking(move || lib.search(&q, limit).map_err(db)).await?;
    Ok(Json(search_answer(&found, &kind)))
}

/// `GET /v1/tracks/{id}`.
#[utoipa::path(
    get,
    path = "/v1/tracks/{id}",
    tag = "library",
    params(("id" = String, Path, description = "id песни")),
    responses((status = 200, body = Track), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn track(State(sh): State<Arc<Shared>>, Path(id): Path<String>) -> Result<Json<Track>, ApiError> {
    let id: TrackId = id.parse().map_err(|_| bad_id())?;
    let lib = sh.lib.clone();
    let row = blocking(move || lib.get_track_row(id).map_err(db)).await?.ok_or_else(gone)?;
    Ok(Json(Track::of_row(&row)))
}

/// `GET /v1/albums/{id}`.
#[utoipa::path(
    get,
    path = "/v1/albums/{id}",
    tag = "library",
    params(("id" = String, Path, description = "id альбома")),
    responses((status = 200, body = AlbumDetail), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn album(State(sh): State<Arc<Shared>>, Path(id): Path<String>) -> Result<Json<AlbumDetail>, ApiError> {
    let id: ReleaseId = id.parse().map_err(|_| bad_id())?;
    let lib = sh.lib.clone();
    let (row, tracks) = blocking(move || {
        let row = lib.get_release_row(id).map_err(db)?.ok_or_else(gone)?;
        let tracks = lib.tracks_of_release(id).map_err(db)?;
        Ok((row, tracks))
    })
    .await?;
    Ok(Json(AlbumDetail { album: Album::of_row(&row), tracks: tracks.iter().map(Track::of_row).collect() }))
}

/// `GET /v1/artists/{id}`.
#[utoipa::path(
    get,
    path = "/v1/artists/{id}",
    tag = "library",
    params(("id" = String, Path, description = "id исполнителя")),
    responses((status = 200, body = ArtistDetail), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn artist(State(sh): State<Arc<Shared>>, Path(id): Path<String>) -> Result<Json<ArtistDetail>, ApiError> {
    let id: ArtistId = id.parse().map_err(|_| bad_id())?;
    let lib = sh.lib.clone();
    blocking(move || artist_detail(&lib, id)).await.map(Json)
}

fn artist_detail(lib: &Library, id: ArtistId) -> Result<ArtistDetail, ApiError> {
    let a = lib.get_artist(id).map_err(db)?.ok_or_else(gone)?;
    let albums = lib.releases_of_artist(id).map_err(db)?;
    let tracks = lib.tracks_of_artist(id, 0, 200).map_err(db)?;
    let liked = lib.is_liked(LikeTarget::Artist(id)).map_err(db)?;
    let local = u32::try_from(tracks.iter().filter(|t| t.has_local).count()).unwrap_or(u32::MAX);
    Ok(ArtistDetail {
        artist: Artist {
            id: a.id.to_string(),
            name: a.name,
            liked,
            releases: u32::try_from(albums.len()).unwrap_or(u32::MAX),
            tracks_local: local,
        },
        albums: albums.iter().map(Album::of_row).collect(),
        tracks: tracks.iter().map(Track::of_row).collect(),
    })
}

/// `GET /v1/playlists`.
#[utoipa::path(
    get,
    path = "/v1/playlists",
    tag = "library",
    responses((status = 200, body = Vec<Playlist>)),
    security(("bearer" = []))
)]
pub async fn playlists(State(sh): State<Arc<Shared>>) -> Result<Json<Vec<Playlist>>, ApiError> {
    let lib = sh.lib.clone();
    let all = blocking(move || lib.list_playlists().map_err(db)).await?;
    Ok(Json(all.iter().map(playlist_of).collect()))
}

/// `GET /v1/playlists/{id}`.
#[utoipa::path(
    get,
    path = "/v1/playlists/{id}",
    tag = "library",
    params(("id" = String, Path, description = "id плейлиста или liked")),
    responses((status = 200, body = PlaylistDetail), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn playlist(State(sh): State<Arc<Shared>>, Path(id): Path<String>) -> Result<Json<PlaylistDetail>, ApiError> {
    let lib = sh.lib.clone();
    if id == LIKED_ID {
        let (count, rows) = blocking(move || Ok((lib.liked_count().map_err(db)?, lib.liked_tracks(0, 5000).map_err(db)?))).await?;
        return Ok(Json(PlaylistDetail {
            playlist: Playlist { id: LIKED_ID.to_owned(), name: nikgob_library::LIKED_PLAYLIST_NAME.to_owned(), track_count: count },
            tracks: rows.iter().map(Track::of_row).collect(),
        }));
    }
    let id: PlaylistId = id.parse().map_err(|_| bad_id())?;
    let (p, entries) = blocking(move || {
        let p = lib.get_playlist(id).map_err(db)?.ok_or_else(gone)?;
        let entries = lib.playlist_entries(id, 0, 5000).map_err(db)?;
        Ok((p, entries))
    })
    .await?;
    Ok(Json(PlaylistDetail {
        playlist: Playlist { id: p.id.to_string(), name: p.name, track_count: u32::try_from(entries.len()).unwrap_or(u32::MAX) },
        tracks: entries.iter().map(|e| Track::of_row(&e.track)).collect(),
    }))
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct LyricsWord {
    pub start_ms: i64,
    pub end_ms: i64,
    /// With its spaces: the words of a line joined are the line.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct LyricsLine {
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    /// Empty: an instrumental break.
    pub text: String,
    /// Word (syllable) timing; empty for a line-timed text.
    pub words: Vec<LyricsWord>,
    /// Background vocals.
    pub background: bool,
}

/// `GET /v1/tracks/{id}/lyrics`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Lyrics {
    pub track_id: String,
    /// Where the text comes from (`file`, `lrclib`, `lyricsplus`…), when
    /// known.
    pub source: Option<String>,
    /// `words`, `lines` or `none`.
    pub timing: String,
    pub lines: Vec<LyricsLine>,
    /// The text without timing, when there is no timed one.
    pub plain: Option<String>,
    pub instrumental: bool,
}

fn dur_ms(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// Timed lines as the API gives them.
pub fn lines_of(lines: &[LyricLine]) -> (String, Vec<LyricsLine>) {
    let out: Vec<LyricsLine> = lines
        .iter()
        .map(|l| LyricsLine {
            start_ms: dur_ms(l.start),
            end_ms: l.end.map(dur_ms),
            text: l.text.clone(),
            words: l.words.iter().map(|w| LyricsWord { start_ms: dur_ms(w.start), end_ms: dur_ms(w.end), text: w.text.clone() }).collect(),
            background: l.background,
        })
        .collect();
    let timing = if out.iter().any(|l| !l.words.is_empty()) {
        "words"
    } else if out.is_empty() {
        "none"
    } else {
        "lines"
    };
    (timing.to_owned(), out)
}

#[utoipa::path(
    get,
    path = "/v1/tracks/{id}/lyrics",
    tag = "library",
    params(("id" = String, Path, description = "id песни")),
    responses((status = 200, body = Lyrics), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn lyrics(State(sh): State<Arc<Shared>>, Path(id): Path<String>) -> Result<Json<Lyrics>, ApiError> {
    let id: TrackId = id.parse().map_err(|_| bad_id())?;
    // The song playing: the text its screen shows, with the timing shown.
    let shown = {
        let st = sh.rt.state();
        let l = &st.lyrics;
        (l.track == Some(id) && (!l.lines.is_empty() || l.plain.is_some()))
            .then(|| (l.source.map(|s| s.key().to_owned()), lines_of(&l.lines), l.plain.clone()))
    };
    if let Some((source, (timing, lines), plain)) = shown {
        let plain = if lines.is_empty() { plain } else { None };
        return Ok(Json(Lyrics { track_id: id.to_string(), source, timing, lines, plain, instrumental: false }));
    }
    // Any other: what the lookup kept of it.
    let lib = sh.lib.clone();
    let (exists, stored) = blocking(move || Ok((lib.get_track_row(id).map_err(db)?.is_some(), lib.lyrics(id).map_err(db)?))).await?;
    if !exists {
        return Err(gone());
    }
    let Some(stored) = stored.filter(|s| !s.is_miss()) else {
        return Err(ApiError::not_found("Текста нет"));
    };
    let parsed = stored.synced.as_deref().map(nikgob_metadata::parse_lrc_lines).unwrap_or_default();
    let (timing, lines) = lines_of(&parsed);
    let plain = if lines.is_empty() { stored.plain.clone() } else { None };
    Ok(Json(Lyrics { track_id: id.to_string(), source: Some("lrclib".to_owned()), timing, lines, plain, instrumental: stored.instrumental }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_word_timing() {
        let mut word = LyricLine::new(Duration::from_millis(1_000), "Hel lo");
        word.words = vec![
            nikgob_view::LyricWord { start: Duration::from_millis(1_000), end: Duration::from_millis(1_400), text: "Hel".into() },
            nikgob_view::LyricWord { start: Duration::from_millis(1_400), end: Duration::from_millis(2_000), text: " lo".into() },
        ];
        let (timing, lines) = lines_of(&[word]);
        assert_eq!(timing, "words");
        assert_eq!(lines[0].words[1].start_ms, 1_400);
        let (timing, lines) = lines_of(&[LyricLine::new(Duration::from_millis(500), "a"), LyricLine::new(Duration::from_millis(900), "")]);
        assert_eq!((timing.as_str(), lines.len(), lines[0].end_ms), ("lines", 2, None));
        assert_eq!(lines_of(&[]).0, "none");
    }

    #[test]
    fn a_search_of_one_kind_has_only_it() {
        let r = SearchResults::default();
        let a = search_answer(&r, "track");
        assert!(a.tracks.is_empty() && a.albums.is_empty());
        let v = serde_json::to_value(search_answer(&r, "any")).unwrap();
        for k in ["tracks", "albums", "artists", "playlists"] {
            assert!(v[k].is_array(), "{k}");
        }
    }
}
