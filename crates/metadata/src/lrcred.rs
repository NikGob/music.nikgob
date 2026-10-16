//! lrc.red TTML, addressed by a verified ISRC.

use std::time::Duration;

use nikgob_core::lyrics::LyricLine;
use url::Url;

use crate::http::{Service, base_url, build_url};
use crate::text::{artist_key, search_artist, search_title, title_key};
use crate::{Deezer, DeezerTrack, Http, MetadataError, Result, parse_ttml};

#[derive(Clone, Debug)]
pub struct LrcRed {
    http: Http,
    base: Url,
}

impl LrcRed {
    pub fn new(http: Http) -> Self {
        Self::with_base_url(http, "https://lrc.red/").expect("valid base URL")
    }

    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self {
            http,
            base: base_url(base)?,
        })
    }

    pub async fn get(&self, isrc: &str) -> Result<Option<Vec<LyricLine>>> {
        let isrc = isrc.trim();
        if isrc.len() != 12 || !isrc.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(MetadataError::InvalidArgument("invalid ISRC".into()));
        }
        let url = build_url(
            &self.base,
            &format!("s/{}.ttml", isrc.to_ascii_uppercase()),
            &[],
        )?;
        match self.http.get_body(Service::LrcRed, url).await {
            Ok(body) => {
                let xml =
                    std::str::from_utf8(&body).map_err(|e| MetadataError::Parse(e.to_string()))?;
                Ok(Some(parse_ttml(xml)?.lines).filter(|l| !l.is_empty()))
            }
            Err(MetadataError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn find(
        &self,
        deezer: &Deezer,
        isrc: Option<&str>,
        artist: &str,
        title: &str,
        duration: Option<Duration>,
    ) -> Result<Option<Vec<LyricLine>>> {
        if let Some(isrc) = isrc.filter(|s| !s.trim().is_empty()) {
            return self.get(isrc).await;
        }
        for hit in deezer
            .search_track(Some(&search_artist(artist)), &search_title(title), 10)
            .await?
        {
            if !matches_track(&hit, artist, title, duration) {
                continue;
            }
            // Deezer search normally omits ISRC; retrieve only a verified hit.
            let track = if hit.isrc.is_some() {
                hit
            } else {
                deezer.track(hit.id).await?
            };
            if matches_track(&track, artist, title, duration)
                && let Some(isrc) = track.isrc.as_deref()
            {
                return self.get(isrc).await;
            }
        }
        Ok(None)
    }
}

fn matches_track(hit: &DeezerTrack, artist: &str, title: &str, duration: Option<Duration>) -> bool {
    !artist_key(artist).is_empty()
        && !title_key(title).base.is_empty()
        && artist_key(hit.artist_name()) == artist_key(artist)
        && title_key(&hit.title) == title_key(title)
        && duration.is_none_or(|d| {
            hit.duration()
                .is_some_and(|h| h.abs_diff(d) <= Duration::from_secs(3))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isrc_hit_must_match_artist_title_variant_and_duration() {
        let hit: DeezerTrack = serde_json::from_str(
            r#"{"title":"Blinding Lights","artist":{"name":"The Weeknd"},"duration":200}"#,
        )
        .unwrap();
        assert!(matches_track(
            &hit,
            "Weeknd",
            "Blinding Lights (Remastered)",
            Some(Duration::from_secs(203))
        ));
        assert!(!matches_track(
            &hit,
            "Someone Else",
            "Blinding Lights",
            None
        ));
        assert!(!matches_track(&hit, "Weeknd", "Other Song", None));
        assert!(!matches_track(
            &hit,
            "Weeknd",
            "Blinding Lights (Live)",
            None
        ));
        assert!(!matches_track(
            &hit,
            "Weeknd",
            "Blinding Lights",
            Some(Duration::from_secs(204))
        ));
    }
}
