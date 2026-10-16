//! LyricsPlus v2: millisecond line and syllable timing.

use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord};
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::http::{Service, base_url, build_url};
use crate::{Http, MetadataError, Result};

#[derive(Clone, Debug)]
pub struct LyricsPlus {
    http: Http,
    base: Url,
}

impl LyricsPlus {
    pub fn new(http: Http) -> Self {
        Self::with_base_url(http, "https://lyricsplus.prjktla.my.id/").expect("valid base URL")
    }

    pub fn with_base_url(http: Http, base: &str) -> Result<Self> {
        Ok(Self {
            http,
            base: base_url(base)?,
        })
    }

    pub async fn find(
        &self,
        artist: &str,
        title: &str,
        album: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<Vec<LyricLine>>> {
        if artist.trim().is_empty() || title.trim().is_empty() {
            return Err(MetadataError::InvalidArgument(
                "title and artist are required".into(),
            ));
        }
        let seconds = duration.map(|d| d.as_secs_f64().to_string());
        let mut params = vec![("title", title), ("artist", artist)];
        if let Some(album) = album.filter(|a| !a.trim().is_empty()) {
            params.push(("album", album));
        }
        if let Some(seconds) = seconds.as_deref() {
            params.push(("duration", seconds));
        }
        let url = build_url(&self.base, "v2/lyrics/get", &params)?;
        match self
            .http
            .get_json::<Payload>(Service::LyricsPlus, url)
            .await
        {
            Ok(payload) => Ok(Some(payload.lines()?).filter(|l| !l.is_empty())),
            Err(MetadataError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: String,
    lyrics: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    time: u64,
    duration: u64,
    text: String,
    #[serde(default)]
    syllabus: Vec<Syllable>,
    #[serde(default)]
    element: Element,
    #[serde(default, rename = "isBackground")]
    background: bool,
    #[serde(default)]
    translation: Option<Value>,
}

#[derive(Default, Deserialize)]
struct Element {
    #[serde(default)]
    singer: String,
    #[serde(default, rename = "isBackground")]
    background: bool,
}

#[derive(Deserialize)]
struct Syllable {
    time: u64,
    duration: u64,
    text: String,
    #[serde(default, rename = "isBackground")]
    background: bool,
}

impl Payload {
    fn lines(self) -> Result<Vec<LyricLine>> {
        let word = if self.kind.eq_ignore_ascii_case("word") {
            true
        } else if self.kind.eq_ignore_ascii_case("line") {
            false
        } else {
            return Err(MetadataError::Parse(
                "unknown LyricsPlus timing type".into(),
            ));
        };
        let mut entries = self.lyrics;
        entries.sort_by_key(|l| l.time);
        let mut out = Vec::new();
        for entry in entries {
            let mut line = LyricLine {
                start: Duration::from_millis(entry.time),
                end: Some(Duration::from_millis(
                    entry.time.saturating_add(entry.duration),
                )),
                text: entry.text,
                background: entry.background || entry.element.background,
                duet: entry
                    .element
                    .singer
                    .strip_prefix('v')
                    .and_then(|v| v.parse::<u32>().ok())
                    .is_some_and(|v| v > 1 && v < 1000),
                translation: entry
                    .translation
                    .as_ref()
                    .and_then(|t| t.as_str().or_else(|| t.get("text")?.as_str()))
                    .map(str::to_owned),
                ..Default::default()
            };
            let mut bg = LyricLine {
                background: true,
                duet: line.duet,
                ..Default::default()
            };
            if word {
                for syllable in entry.syllabus {
                    let target = if syllable.background && !line.background {
                        &mut bg
                    } else {
                        &mut line
                    };
                    target.words.push(LyricWord {
                        start: Duration::from_millis(syllable.time),
                        end: Duration::from_millis(syllable.time.saturating_add(syllable.duration)),
                        text: syllable.text,
                    });
                }
                if !line.words.is_empty() {
                    line.text = line.words.iter().map(|w| w.text.as_str()).collect();
                } else if !bg.words.is_empty() {
                    line.text.clear();
                }
            }
            if !line.text.trim().is_empty() {
                out.push(line);
            } else {
                bg.translation = line.translation;
            }
            if let (Some(first), Some(last)) = (bg.words.first(), bg.words.last()) {
                bg.start = first.start;
                bg.end = Some(last.end);
                bg.text = bg.words.iter().map(|w| w.text.as_str()).collect();
                out.push(bg);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_background_duet_and_translation() {
        let payload: Payload = serde_json::from_str(r#"{"type":"Word","lyrics":[{"time":1000,"duration":3000,"text":"Hello world (oh)","element":{"singer":"v2"},"translation":{"lang":"ru","text":"Привет"},"syllabus":[{"time":1000,"duration":200,"text":"Hel"},{"time":1200,"duration":500,"text":"lo "},{"time":2000,"duration":1000,"text":"world"},{"time":2500,"duration":500,"text":"oh","isBackground":true}]}]}"#).unwrap();
        let lines = payload.lines().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[0].words[1].text, "lo ");
        assert_eq!(lines[0].words[1].end, Duration::from_millis(1700));
        assert!(lines[0].duet && !lines[0].background);
        assert_eq!(lines[0].translation.as_deref(), Some("Привет"));
        assert!(lines[1].background && lines[1].duet);
        assert_eq!(lines[1].text, "oh");
        assert_eq!(lines[1].start, Duration::from_millis(2500));
    }

    #[test]
    fn line_timing_and_background() {
        let payload: Payload = serde_json::from_str(r#"{"type":"Line","lyrics":[{"time":1000,"duration":1200,"text":"Oh","isBackground":true,"element":{"singer":"v1"},"translation":"О"}]}"#).unwrap();
        let line = &payload.lines().unwrap()[0];
        assert!(line.words.is_empty() && line.background && !line.duet);
        assert_eq!(line.end, Some(Duration::from_millis(2200)));
        assert_eq!(line.translation.as_deref(), Some("О"));
    }
}
