//! Resources a client can attach for context: what plays now (with the
//! playing song's timed text), the queue, a summary of the library, the
//! playlists and one playlist. Every token may read them.

use serde_json::{Value, json};

use crate::backend::{ApiRequest, Backend, encode};
use crate::tools::{ToolError, compact, lyrics_brief};

pub const NOW_PLAYING: &str = "music://now-playing";
pub const QUEUE: &str = "music://queue";
pub const SUMMARY: &str = "music://library/summary";
pub const PLAYLISTS: &str = "music://playlists";
/// `music://playlist/{id}`.
pub const PLAYLIST_PREFIX: &str = "music://playlist/";

const JSON_TYPE: &str = "application/json";

/// The `resources/list` entries.
pub fn list() -> Vec<Value> {
    [
        (NOW_PLAYING, "now-playing", "What is playing now, its position, and the synced lyrics of the playing song."),
        (QUEUE, "queue", "The play queue: the current track and what follows."),
        (SUMMARY, "library-summary", "The library in numbers (tracks, formats, sizes, lyrics, fake lossless files) and the last 30 days of listening."),
        (PLAYLISTS, "playlists", "The user's playlists with their ids and track counts."),
    ]
    .iter()
    .map(|(uri, name, description)| json!({"uri": uri, "name": name, "title": name, "description": description, "mimeType": JSON_TYPE}))
    .collect()
}

/// The `resources/templates/list` entries.
pub fn templates() -> Vec<Value> {
    vec![json!({
        "uriTemplate": "music://playlist/{id}",
        "name": "playlist",
        "title": "playlist",
        "description": "One playlist with its tracks (`liked` is Liked Songs).",
        "mimeType": JSON_TYPE,
    })]
}

/// Why a resource could not be read.
#[derive(Debug)]
pub enum ReadError {
    Unknown(String),
    Tool(ToolError),
}

impl From<ToolError> for ReadError {
    fn from(e: ToolError) -> Self {
        Self::Tool(e)
    }
}

async fn get<B: Backend>(b: &B, path: String) -> Result<Value, ToolError> {
    let r = b.call(ApiRequest::get(path)).await.map_err(ToolError::Unreachable)?;
    if r.ok() { Ok(r.body) } else { Err(ToolError::Api { status: r.status, body: r.body }) }
}

/// The `resources/read` result of `uri`.
pub async fn read<B: Backend>(b: &B, uri: &str) -> Result<Value, ReadError> {
    let v = match uri {
        NOW_PLAYING => {
            let mut state = get(b, "/v1/state".to_owned()).await?;
            let id = state.get("track").and_then(|t| t.get("id")).and_then(Value::as_str).map(str::to_owned);
            if let Some(id) = id {
                let lyrics = match get(b, format!("/v1/tracks/{}/lyrics", encode(&id))).await {
                    Ok(l) => lyrics_brief(&l),
                    Err(ToolError::Api { status: 404 | 403, .. }) => Value::Null,
                    Err(e) => return Err(e.into()),
                };
                state["lyrics"] = lyrics;
            }
            state
        }
        QUEUE => get(b, "/v1/queue".to_owned()).await?,
        SUMMARY => {
            let health = get(b, "/v1/stats/health".to_owned()).await?;
            let listening = get(b, "/v1/stats/listening?days=30&limit=5".to_owned()).await?;
            json!({"library": health, "last_30_days": listening})
        }
        PLAYLISTS => get(b, "/v1/playlists".to_owned()).await?,
        _ => match uri.strip_prefix(PLAYLIST_PREFIX).filter(|id| !id.is_empty() && !id.contains('/')) {
            Some(id) => get(b, format!("/v1/playlists/{}", encode(id))).await?,
            None => return Err(ReadError::Unknown(uri.to_owned())),
        },
    };
    let text = serde_json::to_string(&compact(v)).unwrap_or_default();
    Ok(json!({"contents": [{"uri": uri, "mimeType": JSON_TYPE, "text": text}]}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_resources_and_the_template() {
        let uris: Vec<String> = list().iter().map(|r| r["uri"].as_str().unwrap().to_owned()).collect();
        assert_eq!(uris, [NOW_PLAYING, QUEUE, SUMMARY, PLAYLISTS]);
        assert_eq!(templates()[0]["uriTemplate"], "music://playlist/{id}");
        assert!(list().iter().all(|r| r["mimeType"] == JSON_TYPE));
    }
}
