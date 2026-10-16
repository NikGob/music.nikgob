//! Better Lyrics TTML fallback, with a mirror for transport/server failures.

use nikgob_core::lyrics::LyricLine;
use serde::Deserialize;
use url::Url;

use crate::http::{Service, base_url, build_url, ok_body, parse_json};
use crate::{Http, MetadataError, Result, parse_ttml};

#[derive(Clone, Debug)]
pub struct BetterLyrics {
    http: Http,
    base: Url,
    mirror: Url,
}

impl BetterLyrics {
    pub fn new(http: Http) -> Self {
        Self::with_base_urls(
            http,
            "https://lyrics-api.boidu.dev/",
            "https://api.betterlyrics.org/",
        )
        .expect("valid base URLs")
    }

    pub fn with_base_urls(http: Http, base: &str, mirror: &str) -> Result<Self> {
        Ok(Self {
            http,
            base: base_url(base)?,
            mirror: base_url(mirror)?,
        })
    }

    async fn get(&self, base: &Url, artist: &str, title: &str) -> Result<Option<Vec<LyricLine>>> {
        #[derive(Deserialize)]
        struct Payload {
            ttml: String,
        }
        let url = build_url(base, "getLyrics", &[("s", title), ("a", artist)])?;
        // Leave room for the mirror inside the runtime's 20-second deadline.
        let request = self
            .http
            .client()
            .get(url)
            .timeout(std::time::Duration::from_secs(8));
        let payload = async {
            let response = self.http.send(Service::BetterLyrics, request).await?;
            parse_json::<Payload>(&ok_body(response).await?)
        }
        .await;
        match payload {
            Ok(payload) if payload.ttml.trim().is_empty() => Ok(None),
            Ok(payload) => Ok(Some(parse_ttml(&payload.ttml)?.lines).filter(|l| !l.is_empty())),
            Err(MetadataError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn find(&self, artist: &str, title: &str) -> Result<Option<Vec<LyricLine>>> {
        match self.get(&self.base, artist, title).await {
            Err(
                MetadataError::Network(_)
                | MetadataError::Status {
                    status: 500..=599, ..
                },
            ) => self.get(&self.mirror, artist, title).await,
            result => result,
        }
    }
}
