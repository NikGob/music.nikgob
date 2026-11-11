//! LRCLIB: synchronized (LRC) and plain lyrics.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url};
use crate::lrc::parse_lrc;
use crate::text::{artist_key, search_artist, search_title, similarity, title_key};

const DEFAULT_BASE: &str = "https://lrclib.net/";
/// LRCLIB itself matches `/api/get` within +-2 s; the same window is used to
/// accept `/api/search` hits.
const DURATION_WINDOW_SECS: f64 = 3.0;
/// Minimum text similarity (0..=1, see `text::similarity`) of a search hit.
const MIN_TITLE_SIMILARITY: f64 = 0.85;
const MIN_ARTIST_SIMILARITY: f64 = 0.7;
/// LRCLIB rejects a `duration` outside this range (whole seconds) with
/// HTTP 400, so such a value is never sent.
const DURATION_RANGE_SECS: std::ops::RangeInclusive<f64> = 1.0..=3600.0;

/// The duration as LRCLIB takes it, or `None` when it is unknown or out of the
/// range the API accepts (a stream without a length, a long mix).
fn usable_duration(duration: Option<Duration>) -> Option<Duration> {
    duration.filter(|d| DURATION_RANGE_SECS.contains(&d.as_secs_f64().round()))
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Lyrics {
    pub id: i64,
    pub track_name: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    /// Seconds.
    pub duration: Option<f64>,
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    /// Raw LRC text; see [`Lyrics::synced_lines`].
    pub synced_lyrics: Option<String>,
}

impl Lyrics {
    pub fn has_synced(&self) -> bool {
        self.synced_lyrics
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    }

    /// Parsed synchronized lyrics (empty if there are none).
    pub fn synced_lines(&self) -> Vec<(Duration, String)> {
        self.synced_lyrics
            .as_deref()
            .map(parse_lrc)
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub struct Lrclib {
    http: Http,
    base: Url,
}

impl Lrclib {
    pub fn new(http: Http) -> Self {
        Self {
            http,
            base: base_url(DEFAULT_BASE).expect("default base URL is valid"),
        }
    }

    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self {
            http,
            base: base_url(base)?,
        })
    }

    /// `GET /api/get`; `None` when LRCLIB has no such track.
    pub async fn get(
        &self,
        artist: &str,
        track: &str,
        album: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<Lyrics>> {
        let secs = usable_duration(duration).map(|d| d.as_secs_f64().round().to_string());
        let mut params = vec![("artist_name", artist), ("track_name", track)];
        if let Some(a) = album.map(str::trim).filter(|a| !a.is_empty()) {
            params.push(("album_name", a));
        }
        if let Some(d) = secs.as_deref() {
            params.push(("duration", d));
        }
        let url = build_url(&self.base, "api/get", &params)?;
        match self.http.get_json::<Lyrics>(Service::Lrclib, url).await {
            Ok(l) => Ok(Some(l)),
            Err(MetadataError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `GET /api/search` by track (and optionally artist/album) name.
    pub async fn search(
        &self,
        artist: Option<&str>,
        track: &str,
        album: Option<&str>,
    ) -> Result<Vec<Lyrics>> {
        let mut params = vec![("track_name", track)];
        if let Some(a) = artist.map(str::trim).filter(|a| !a.is_empty()) {
            params.push(("artist_name", a));
        }
        if let Some(a) = album.map(str::trim).filter(|a| !a.is_empty()) {
            params.push(("album_name", a));
        }
        let url = build_url(&self.base, "api/search", &params)?;
        self.http.get_json(Service::Lrclib, url).await
    }

    /// Exact `get` first, then `search` as a fallback. Among search hits the
    /// closest duration (within a few seconds) wins, synced lyrics preferred.
    ///
    /// Tags are messy ("Song (feat. X)", "Song - Remastered 2011", "Song [Live]"),
    /// so when the raw names find nothing the lookup is repeated with the
    /// normalized title and artist, and finally by title alone.
    pub async fn find(
        &self,
        artist: &str,
        track: &str,
        album: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<Lyrics>> {
        // An unusable length would also filter out every search hit.
        let duration = usable_duration(duration);
        if let Some(found) = self.get(artist, track, album, duration).await? {
            return Ok(Some(found));
        }
        let clean_artist = search_artist(artist);
        let clean_track = search_title(track);
        if (clean_artist != artist.trim() || clean_track != track.trim())
            && let Some(found) = self
                .get(&clean_artist, &clean_track, album, duration)
                .await?
        {
            return Ok(Some(found));
        }
        let hits = self.search(Some(&clean_artist), &clean_track, None).await?;
        let empty = hits.is_empty();
        if let Some(best) = pick_best(hits, &clean_artist, &clean_track, duration) {
            return Ok(Some(best));
        }
        if empty {
            // The artist spelling may be what misses (transliteration, "A & B").
            let hits = self.search(None, &clean_track, None).await?;
            return Ok(pick_best(hits, &clean_artist, &clean_track, duration));
        }
        Ok(None)
    }
}

/// LRCLIB's search is fuzzy: a hit has to be the same song, or the lyrics of
/// some other song end up in the user's file.
fn same_song(hit: &Lyrics, artist: &str, track: &str) -> bool {
    let (want_title, got_title) = (title_key(track).base, title_key(&hit.track_name).base);
    if !want_title.is_empty()
        && !got_title.is_empty()
        && similarity(&want_title, &got_title) < MIN_TITLE_SIMILARITY
    {
        return false;
    }
    let (want_artist, got_artist) = (artist_key(artist), artist_key(&hit.artist_name));
    want_artist.is_empty()
        || got_artist.is_empty()
        || similarity(&want_artist, &got_artist) >= MIN_ARTIST_SIMILARITY
}

fn pick_best(
    hits: Vec<Lyrics>,
    artist: &str,
    track: &str,
    duration: Option<Duration>,
) -> Option<Lyrics> {
    let want = duration.map(|d| d.as_secs_f64());
    let mut scored: Vec<(f64, Lyrics)> = hits
        .into_iter()
        .filter(|l| same_song(l, artist, track))
        .filter_map(|l| {
            let diff = match (want, l.duration) {
                (Some(w), Some(d)) => (w - d).abs(),
                _ => 0.0,
            };
            (diff <= DURATION_WINDOW_SECS).then_some((diff, l))
        })
        .collect();
    scored.sort_by(|(da, a), (db, b)| b.has_synced().cmp(&a.has_synced()).then(da.total_cmp(db)));
    scored.into_iter().next().map(|(_, l)| l)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_outside_the_api_range_is_dropped() {
        assert_eq!(usable_duration(None), None);
        assert_eq!(usable_duration(Some(Duration::ZERO)), None);
        assert_eq!(usable_duration(Some(Duration::from_millis(400))), None);
        assert_eq!(usable_duration(Some(Duration::from_secs(4000))), None);
        let hour = Some(Duration::from_millis(3_600_400));
        assert_eq!(usable_duration(hour), hour);
        assert_eq!(usable_duration(Some(Duration::from_secs(1))), Some(Duration::from_secs(1)));
        assert_eq!(usable_duration(Some(Duration::from_secs(233))), Some(Duration::from_secs(233)));
    }
}
