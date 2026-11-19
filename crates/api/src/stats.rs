//! Analytics: the listening report of a period, the forgotten gems, the
//! health of the library and the tracks nearest in sound (the same sound
//! vectors the recommendations use: bliss, blended with the neural model's
//! where the library has enough of them).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{RawQuery, State};
use serde::Serialize;
use utoipa::ToSchema;

use nikgob_core::model::TrackId;
use nikgob_library::{Library, LibraryHealth, ListeningReport, StatTrack};
use nikgob_recs::{Compact, VectorIndex};

use crate::Shared;
use crate::error::{ApiError, Json, query_value};
use crate::library::{Track, blocking, db};

const DAY_MS: i64 = 24 * 3600 * 1000;
/// How long the sound vectors read from the library are used.
const SOUND_KEPT: Duration = Duration::from_secs(10 * 60);

fn number<T: std::str::FromStr>(raw: Option<&str>, key: &str) -> Result<Option<T>, ApiError> {
    match query_value(raw, key) {
        Some(v) if !v.trim().is_empty() => v.trim().parse().map(Some).map_err(|_| ApiError::bad_request(format!("{key} must be a number"))),
        _ => Ok(None),
    }
}

/// This computer's offset from UTC now, in seconds (the hours of the day
/// are local).
fn local_offset_s() -> i64 {
    i64::from(jiff::Zoned::now().offset().seconds())
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct StatTrackOut {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub plays: u32,
    pub completes: u32,
    pub skips: u32,
    pub last_played_ms: Option<i64>,
    pub liked: bool,
}

impl StatTrackOut {
    fn of(t: &StatTrack) -> Self {
        Self {
            id: t.id.to_string(),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            plays: t.plays,
            completes: t.completes,
            skips: t.skips,
            last_played_ms: t.last_played,
            liked: t.liked,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct NamedCount {
    pub id: Option<String>,
    pub name: String,
    /// The album's artist (albums only).
    pub artist: Option<String>,
    pub plays: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct KeyCount {
    pub key: String,
    pub plays: u32,
    /// This device (the one answering).
    pub this_device: Option<bool>,
}

/// `GET /v1/stats/listening`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ListeningAnswer {
    pub from_ms: i64,
    pub to_ms: i64,
    pub plays: u32,
    pub completes: u32,
    pub skips: u32,
    /// Skipped in the first 30 s.
    pub early_skips: u32,
    /// Songs played to the end out of those started, 0..1.
    pub completion_rate: f64,
    pub hours: f64,
    pub distinct_tracks: u32,
    pub top_tracks: Vec<StatTrackOut>,
    pub top_artists: Vec<NamedCount>,
    pub top_albums: Vec<NamedCount>,
    /// Songs started in each local hour, 0–23.
    pub by_hour: Vec<u32>,
    pub by_device: Vec<KeyCount>,
    /// `album`, `playlist`, `radio`, `search`…
    pub by_context: Vec<KeyCount>,
}

impl ListeningAnswer {
    pub fn of(r: &ListeningReport, this_device: &str) -> Self {
        let rate = if r.plays == 0 { 0.0 } else { (f64::from(r.completes) / f64::from(r.plays) * 1000.0).round() / 1000.0 };
        Self {
            from_ms: r.from_ms,
            to_ms: r.to_ms,
            plays: r.plays,
            completes: r.completes,
            skips: r.skips,
            early_skips: r.early_skips,
            completion_rate: rate.min(1.0),
            hours: (r.listened_ms as f64 / 3_600_000.0 * 10.0).round() / 10.0,
            distinct_tracks: r.distinct_tracks,
            top_tracks: r.top_tracks.iter().map(StatTrackOut::of).collect(),
            top_artists: r.top_artists.iter().map(|a| NamedCount { id: a.id.map(|i| i.to_string()), name: a.name.clone(), artist: None, plays: a.plays }).collect(),
            top_albums: r
                .top_albums
                .iter()
                .map(|a| NamedCount { id: Some(a.id.to_string()), name: a.title.clone(), artist: Some(a.artist.clone()), plays: a.plays })
                .collect(),
            by_hour: r.by_hour.to_vec(),
            by_device: r.by_device.iter().map(|(k, n)| KeyCount { key: k.clone(), plays: *n, this_device: Some(k == this_device) }).collect(),
            by_context: r.by_context.iter().map(|(k, n)| KeyCount { key: k.clone(), plays: *n, this_device: None }).collect(),
        }
    }
}

/// The period of `days` (or `from_ms`/`to_ms`) ending `now`.
pub fn period(days: Option<i64>, from: Option<i64>, to: Option<i64>, now: i64) -> Result<(i64, i64), ApiError> {
    let to = to.unwrap_or(now);
    let from = match (from, days) {
        (Some(f), _) => f,
        (None, Some(d)) => to - d.clamp(1, 3650) * DAY_MS,
        (None, None) => to - 30 * DAY_MS,
    };
    if from >= to {
        return Err(ApiError::bad_request("from_ms must be earlier than to_ms"));
    }
    Ok((from, to))
}

/// What was listened to: plays, hours, songs to the end and early skips,
/// the top songs, artists and albums, hours of the day, devices, places.
#[utoipa::path(
    get,
    path = "/v1/stats/listening",
    tag = "stats",
    params(
        ("days" = Option<i64>, Query, description = "The last N days (30 by default)"),
        ("from_ms" = Option<i64>, Query, description = "The period's start, unix ms"),
        ("to_ms" = Option<i64>, Query, description = "The period's end, unix ms (now by default)"),
        ("limit" = Option<u32>, Query, description = "How many in each top, 1–50, 10 by default"),
    ),
    responses((status = 200, body = ListeningAnswer), (status = 400, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn listening(State(sh): State<Arc<Shared>>, RawQuery(raw): RawQuery) -> Result<Json<ListeningAnswer>, ApiError> {
    let raw = raw.as_deref();
    let (from, to) = period(number(raw, "days")?, number(raw, "from_ms")?, number(raw, "to_ms")?, nikgob_library::now_ms())?;
    let limit = number::<u32>(raw, "limit")?.unwrap_or(10).clamp(1, 50);
    let lib = sh.lib.clone();
    let offset = local_offset_s();
    let (report, device) = blocking(move || Ok((lib.listening_report(from, to, limit, offset).map_err(db)?, lib.device_id().0))).await?;
    Ok(Json(ListeningAnswer::of(&report, &device)))
}

/// `GET /v1/stats/forgotten`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct ForgottenAnswer {
    /// Not played since (unix ms).
    pub before_ms: i64,
    pub min_plays: u32,
    pub tracks: Vec<StatTrackOut>,
}

/// Songs liked or played a lot that were not played for `days`.
#[utoipa::path(
    get,
    path = "/v1/stats/forgotten",
    tag = "stats",
    params(
        ("days" = Option<i64>, Query, description = "Not played for this many days (90 by default)"),
        ("min_plays" = Option<u32>, Query, description = "Played at least this many times (5 by default) — or liked"),
        ("limit" = Option<u32>, Query, description = "1–100, 20 by default"),
    ),
    responses((status = 200, body = ForgottenAnswer)),
    security(("bearer" = []))
)]
pub async fn forgotten(State(sh): State<Arc<Shared>>, RawQuery(raw): RawQuery) -> Result<Json<ForgottenAnswer>, ApiError> {
    let raw = raw.as_deref();
    let days = number::<i64>(raw, "days")?.unwrap_or(90).clamp(1, 3650);
    let min_plays = number::<u32>(raw, "min_plays")?.unwrap_or(5).max(1);
    let limit = number::<u32>(raw, "limit")?.unwrap_or(20).clamp(1, 100);
    let before = nikgob_library::now_ms() - days * DAY_MS;
    let lib = sh.lib.clone();
    let rows = blocking(move || lib.forgotten_gems(before, min_plays, limit).map_err(db)).await?;
    Ok(Json(ForgottenAnswer { before_ms: before, min_plays, tracks: rows.iter().map(StatTrackOut::of).collect() }))
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct FormatOut {
    /// `lossless`, `opus`, `mp3`, `aac`, `vorbis`, `other`.
    pub group: String,
    pub files: u32,
    pub bytes: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct FakeOut {
    pub track_id: String,
    pub name: String,
    pub verdict: String,
    pub origin: String,
}

/// `GET /v1/stats/health`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct HealthAnswer {
    pub tracks: u32,
    pub local_tracks: u32,
    pub albums: u32,
    pub artists: u32,
    pub playlists: u32,
    pub liked_tracks: u32,
    pub trashed_tracks: u32,
    pub local_bytes: i64,
    pub local_hours: f64,
    pub formats: Vec<FormatOut>,
    /// Songs with a file and no timed text kept.
    pub without_synced_lyrics: u32,
    pub fake_lossless: u32,
    pub fake_lossless_examples: Vec<FakeOut>,
    pub masters: u32,
    pub masters_bytes: i64,
    pub masters_without_replica: u32,
    pub missing_files: u32,
}

impl HealthAnswer {
    pub fn of(h: &LibraryHealth) -> Self {
        Self {
            tracks: h.tracks,
            local_tracks: h.local_tracks,
            albums: h.albums,
            artists: h.artists,
            playlists: h.playlists,
            liked_tracks: h.liked_tracks,
            trashed_tracks: h.trashed_tracks,
            local_bytes: h.local_bytes,
            local_hours: (h.local_ms as f64 / 3_600_000.0 * 10.0).round() / 10.0,
            formats: h.formats.iter().map(|f| FormatOut { group: f.group.clone(), files: f.files, bytes: f.bytes }).collect(),
            without_synced_lyrics: h.without_synced_lyrics,
            fake_lossless: h.fake_lossless,
            fake_lossless_examples: h
                .fake_lossless_examples
                .iter()
                .map(|f| FakeOut { track_id: f.track.to_string(), name: format!("{} — {}", f.artist, f.title), verdict: f.verdict.clone(), origin: f.origin.clone() })
                .collect(),
            masters: h.masters,
            masters_bytes: h.masters_bytes,
            masters_without_replica: h.masters_without_replica,
            missing_files: h.missing_files,
        }
    }
}

/// The library in numbers: sizes, formats, songs without a timed text,
/// fake lossless files, masters without a copy elsewhere, missing files.
#[utoipa::path(get, path = "/v1/stats/health", tag = "stats", responses((status = 200, body = HealthAnswer)), security(("bearer" = [])))]
pub async fn health(State(sh): State<Arc<Shared>>) -> Result<Json<HealthAnswer>, ApiError> {
    let lib = sh.lib.clone();
    let h = blocking(move || lib.library_health(20).map_err(db)).await?;
    Ok(Json(HealthAnswer::of(&h)))
}

/// The library's sound vectors, read at most every [`SOUND_KEPT`].
#[derive(Default)]
pub struct SoundCache {
    kept: parking_lot::Mutex<Option<(Instant, Arc<VectorIndex<TrackId>>)>>,
}

/// Every sound vector of the library: bliss, and the neural model's beside.
pub fn sound_index(lib: &Library) -> Result<VectorIndex<TrackId>, ApiError> {
    let mut idx = VectorIndex::new();
    for a in lib.analysis_vectors(nikgob_recs::ANALYZED_WITH).map_err(db)? {
        if let Ok(v) = nikgob_recs::Vector::try_from(a.bliss_vector.as_slice()) {
            idx.insert(a.track_id, v);
        }
    }
    for (t, bytes) in lib.embedding_vectors(nikgob_recs::EMBEDDED_WITH).map_err(db)? {
        if let Some(c) = Compact::from_bytes(&bytes) {
            idx.set_embedding(t, c);
        }
    }
    Ok(idx)
}

impl SoundCache {
    fn get(&self, lib: &Library) -> Result<Arc<VectorIndex<TrackId>>, ApiError> {
        if let Some((at, idx)) = self.kept.lock().as_ref()
            && at.elapsed() < SOUND_KEPT
        {
            return Ok(idx.clone());
        }
        let idx = Arc::new(sound_index(lib)?);
        *self.kept.lock() = Some((Instant::now(), idx.clone()));
        Ok(idx)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct Similar {
    pub track: Track,
    /// Smaller is nearer.
    pub distance: f32,
}

/// `GET /v1/stats/similar`.
#[derive(Clone, Debug, PartialEq, Serialize, ToSchema)]
pub struct SimilarAnswer {
    pub seed: Track,
    pub similar: Vec<Similar>,
}

/// The library's tracks nearest in sound to a track (the playing one by
/// default).
#[utoipa::path(
    get,
    path = "/v1/stats/similar",
    tag = "stats",
    params(
        ("track_id" = Option<String>, Query, description = "the track's id (the playing one by default)"),
        ("limit" = Option<u32>, Query, description = "1–50, 10 by default"),
    ),
    responses((status = 200, body = SimilarAnswer), (status = 404, body = crate::error::ErrorBody)),
    security(("bearer" = []))
)]
pub async fn similar(State(sh): State<Arc<Shared>>, RawQuery(raw): RawQuery) -> Result<Json<SimilarAnswer>, ApiError> {
    let raw = raw.as_deref();
    let limit = number::<usize>(raw, "limit")?.unwrap_or(10).clamp(1, 50);
    let seed: TrackId = match query_value(raw, "track_id").filter(|s| !s.trim().is_empty()) {
        Some(s) => s.trim().parse().map_err(|_| ApiError::bad_request("Invalid id"))?,
        None => sh.rt.state().player.current.as_ref().map(|t| t.id).ok_or_else(|| ApiError::bad_request("Nothing is playing: a track_id is required"))?,
    };
    let sh2 = sh.clone();
    let (seed_row, near) = blocking(move || {
        let lib = &sh2.lib;
        let row = lib.get_track_row(seed).map_err(db)?.ok_or_else(|| ApiError::not_found("Not found"))?;
        let idx = sh2.sound.get(lib)?;
        let v = *idx.get(seed).ok_or_else(|| ApiError::not_found("The song has no audio analysis yet"))?;
        let near = idx.nearest_to(Some(seed), &v, limit + 1, |k| k != seed);
        let ids: Vec<TrackId> = near.iter().map(|(k, _)| *k).collect();
        let rows = lib.get_track_rows(&ids).map_err(db)?;
        let out: Vec<Similar> = near.iter().filter_map(|(k, d)| rows.get(k).map(|r| Similar { track: Track::of_row(r), distance: (d * 1000.0).round() / 1000.0 })).take(limit).collect();
        Ok((row, out))
    })
    .await?;
    Ok(Json(SimilarAnswer { seed: Track::of_row(&seed_row), similar: near }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods() {
        let now = 1_000 * DAY_MS;
        assert_eq!(period(None, None, None, now).unwrap(), (now - 30 * DAY_MS, now));
        assert_eq!(period(Some(7), None, None, now).unwrap(), (now - 7 * DAY_MS, now));
        assert_eq!(period(Some(0), None, None, now).unwrap(), (now - DAY_MS, now));
        assert_eq!(period(None, Some(5), Some(10), now).unwrap(), (5, 10));
        assert!(period(None, Some(10), Some(5), now).is_err());
        assert_eq!(number::<u32>(Some("limit=7"), "limit").unwrap(), Some(7));
        assert!(number::<u32>(Some("limit=x"), "limit").is_err());
        assert_eq!(number::<u32>(None, "limit").unwrap(), None);
    }

    #[test]
    fn a_report_reads_as_the_api_says() {
        let mut r = ListeningReport { plays: 4, completes: 3, listened_ms: 5_400_000, ..Default::default() };
        r.by_device = vec![("pc".into(), 4)];
        let a = ListeningAnswer::of(&r, "pc");
        assert_eq!(a.completion_rate, 0.75);
        assert_eq!(a.hours, 1.5);
        assert_eq!(a.by_hour.len(), 24);
        assert_eq!(a.by_device[0].this_device, Some(true));
        let v = serde_json::to_value(&a).unwrap();
        for k in ["plays", "early_skips", "top_tracks", "top_artists", "top_albums", "by_hour", "by_device", "hours"] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert_eq!(ListeningAnswer::of(&ListeningReport::default(), "pc").completion_rate, 0.0);
    }

    /// The nearest by sound on a fixture library: the seed itself is left
    /// out, the nearer one first.
    #[test]
    fn similar_tracks_by_their_vectors() {
        use nikgob_core::model::{Analysis, AnalysisSource, Artist, ArtistCredit, Track as CoreTrack};
        let lib = Library::open_in_memory().unwrap();
        let a = lib.upsert_artist(&Artist::new("A")).unwrap();
        let mut ids = Vec::new();
        for (i, base) in [0.0f32, 0.1, 5.0].iter().enumerate() {
            let t = lib.upsert_track(&CoreTrack::new(format!("T{i}"), ArtistCredit::single("A", Some(a.id)))).unwrap();
            let v: Vec<f32> = (0..nikgob_recs::DIM).map(|k| base + k as f32 * 0.01).collect();
            lib.put_analysis(&Analysis { track_id: t.id, bliss_vector: v, analyzed_with: nikgob_recs::ANALYZED_WITH.to_owned(), source: AnalysisSource::File }).unwrap();
            ids.push(t.id);
        }
        let idx = sound_index(&lib).unwrap();
        assert_eq!(idx.len(), 3);
        let v = *idx.get(ids[0]).unwrap();
        let near = idx.nearest_to(Some(ids[0]), &v, 3, |k| k != ids[0]);
        assert_eq!(near.iter().map(|(k, _)| *k).collect::<Vec<_>>(), vec![ids[1], ids[2]]);
    }
}
