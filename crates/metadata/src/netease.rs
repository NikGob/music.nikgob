//! NetEase Cloud Music: lyrics from the public web endpoints (no signed or
//! encrypted API). The public `/api/song/lyric` returns line-synced LRC with
//! translation and romanization; word-synced YRC is parsed when it shows up.

use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord};
use serde::Deserialize;
use url::Url;

use crate::cn::{self, Hit};
use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url, parse_json};
use crate::lrc::parse_lrc_lines;

const DEFAULT_BASE: &str = "https://music.163.com/";
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const HEADERS: &[(&str, &str)] = &[
    ("User-Agent", BROWSER_UA),
    ("Referer", "https://music.163.com"),
];

/// Lyrics of one NetEase song; every part is raw text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NeteaseLyrics {
    pub id: u64,
    /// Line-synced LRC.
    pub lrc: Option<String>,
    /// Word-synced YRC (see [`parse_yrc`]); the public endpoint often has none.
    pub yrc: Option<String>,
    /// LRC with the translation.
    pub translation: Option<String>,
    /// LRC with the romanization.
    pub romanization: Option<String>,
    pub instrumental: bool,
}

#[derive(Clone, Debug)]
pub struct Netease {
    http: Http,
    base: Url,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResponse {
    result: Option<SearchResult>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SearchResult {
    songs: Option<Vec<Song>>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Song {
    id: u64,
    name: String,
    artists: Vec<Named>,
    /// Milliseconds.
    duration: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Named {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricResponse {
    lrc: Option<LyricPart>,
    tlyric: Option<LyricPart>,
    romalrc: Option<LyricPart>,
    yrc: Option<LyricPart>,
    #[serde(rename = "pureMusic")]
    pure_music: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricPart {
    lyric: Option<String>,
}

fn part(p: Option<LyricPart>) -> Option<String> {
    p.and_then(|p| p.lyric).filter(|s| !s.trim().is_empty())
}

impl Netease {
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

    async fn search(&self, query: &str) -> Result<Vec<Song>> {
        let url = build_url(
            &self.base,
            "api/search/get",
            &[("s", query), ("type", "1"), ("limit", "10")],
        )?;
        let body = self
            .http
            .get_body_with(Service::Netease, url, HEADERS)
            .await?;
        let r: SearchResponse = parse_json(&body)?;
        Ok(r.result.and_then(|r| r.songs).unwrap_or_default())
    }

    /// Lyrics of a known song id; `None` when NetEase has no lyrics for it.
    pub async fn lyrics(&self, id: u64) -> Result<Option<NeteaseLyrics>> {
        let id_s = id.to_string();
        let url = build_url(
            &self.base,
            "api/song/lyric",
            &[
                ("id", id_s.as_str()),
                ("lv", "1"),
                ("tv", "1"),
                ("rv", "1"),
                ("yv", "1"),
            ],
        )?;
        let body = match self
            .http
            .get_body_with(Service::Netease, url, HEADERS)
            .await
        {
            Ok(b) => b,
            Err(MetadataError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let r: LyricResponse = parse_json(&body)?;
        let out = NeteaseLyrics {
            id,
            lrc: part(r.lrc),
            yrc: part(r.yrc),
            translation: part(r.tlyric),
            romanization: part(r.romalrc),
            instrumental: r.pure_music,
        };
        let usable = out.instrumental || out.lrc.is_some() || out.yrc.is_some();
        Ok(usable.then_some(out))
    }

    /// Searches "title artist" (then the title alone) for the same song (duration
    /// within 3 s, similar title and artist) and fetches its lyrics.
    pub async fn find(
        &self,
        artist: &str,
        title: &str,
        _album: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<NeteaseLyrics>> {
        let mut tried = Vec::new();
        for with_artist in [true, false] {
            let q = cn::query(artist, title, with_artist);
            if q.is_empty() || tried.contains(&q) {
                continue;
            }
            let songs = self.search(&q).await?;
            tried.push(q);
            let hits = songs
                .into_iter()
                .map(|s| Hit {
                    payload: s.id,
                    title: s.name,
                    artists: s
                        .artists
                        .into_iter()
                        .flat_map(|a| cn::split_artists(&a.name))
                        .collect(),
                    duration: s.duration.map(Duration::from_millis),
                })
                .collect();
            if let Some(hit) = cn::pick(hits, artist, title, duration) {
                return self.lyrics(hit.payload).await;
            }
        }
        Ok(None)
    }
}

/// Parses YRC (`[lineStartMs,lineDurMs](wordStartMs,wordDurMs,0)word...`).
/// JSON credit lines (`{"t":0,"c":[...]}`) are skipped.
pub fn parse_yrc(text: &str) -> Vec<LyricLine> {
    let mut out = Vec::new();
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let raw = raw.trim_start();
        let Some(rest) = raw.strip_prefix('[') else {
            continue;
        };
        let Some((head, body)) = rest.split_once(']') else {
            continue;
        };
        let Some((start, dur)) = parse_pair(head) else {
            continue;
        };
        let words = parse_words(body);
        let text: String = words.iter().map(|w| w.text.as_str()).collect();
        out.push(LyricLine {
            start: Duration::from_millis(start),
            end: Some(Duration::from_millis(start + dur)),
            text: text.trim().to_owned(),
            words,
            ..Default::default()
        });
    }
    out
}

fn parse_pair(s: &str) -> Option<(u64, u64)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// `(start,dur,0)` at the start of `s` -> (start, dur, length of the tag).
fn word_tag(s: &str) -> Option<(u64, u64, usize)> {
    let inner = s.strip_prefix('(')?;
    let end = inner.find(')')?;
    let mut parts = inner[..end].split(',');
    let start = parts.next()?.trim().parse().ok()?;
    let dur = parts.next()?.trim().parse().ok()?;
    let flag = parts.next()?;
    if parts.next().is_some() || flag.trim().parse::<i64>().is_err() {
        return None;
    }
    Some((start, dur, end + 2))
}

fn parse_words(body: &str) -> Vec<LyricWord> {
    fn flush(cur: Option<(u64, u64, usize)>, body: &str, end: usize, words: &mut Vec<LyricWord>) {
        let Some((s, d, from)) = cur else { return };
        let text = &body[from..end];
        if text.trim().is_empty() {
            if let Some(prev) = words.last_mut() {
                prev.text.push_str(text);
            }
        } else {
            words.push(LyricWord {
                start: Duration::from_millis(s),
                end: Duration::from_millis(s + d),
                text: text.to_owned(),
            });
        }
    }
    let mut words: Vec<LyricWord> = Vec::new();
    let mut current: Option<(u64, u64, usize)> = None; // start, dur, text start
    let mut i = 0;
    while i < body.len() {
        if body.as_bytes()[i] == b'('
            && let Some((s, d, len)) = word_tag(&body[i..])
        {
            flush(current, body, i, &mut words);
            current = Some((s, d, i + len));
            i += len;
            continue;
        }
        i += body[i..].chars().next().map_or(1, char::len_utf8);
    }
    flush(current, body, body.len(), &mut words);
    words
}

/// The lines to show: YRC when there is any, otherwise the LRC, without the leading
/// credit lines and with translation/romanization attached by start time.
pub fn netease_lines(l: &NeteaseLyrics) -> Vec<LyricLine> {
    if l.instrumental {
        return Vec::new();
    }
    let mut lines = l
        .yrc
        .as_deref()
        .map(parse_yrc)
        .filter(|v| !v.is_empty())
        .or_else(|| l.lrc.as_deref().map(parse_lrc_lines))
        .unwrap_or_default();
    lines = cn::strip_credits(lines);
    if let Some(t) = l.translation.as_deref() {
        cn::attach(&mut lines, t, false);
    }
    if let Some(r) = l.romanization.as_deref() {
        cn::attach(&mut lines, r, true);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn yrc_words_lines_and_json_credits() {
        let yrc = concat!(
            "{\"t\":0,\"c\":[{\"tx\":\"作词: \"},{\"tx\":\"DECO*27\"}]}\n",
            "[1000,2000](1000,500,0)Hel(1500,500,0)lo (2000,1000,0)world \n",
            "[4000,1000](4000,1000,0)(oh)\n",
            "[9000,500](9000,250,0)A(9250,250,0) \n",
        );
        let lines = parse_yrc(yrc);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].start, ms(1000));
        assert_eq!(lines[0].end, Some(ms(3000)));
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(lines[0].words.len(), 3);
        assert_eq!(lines[0].words[1].text, "lo ");
        assert_eq!(lines[0].words[1].start, ms(1500));
        assert_eq!(lines[0].words[1].end, ms(2000));
        assert_eq!(lines[1].text, "(oh)");
        // A whitespace-only word is folded into the previous one.
        assert_eq!(lines[2].words.len(), 1);
        assert_eq!(lines[2].words[0].text, "A ");
    }

    #[test]
    fn lrc_credits_are_dropped_and_translation_attached() {
        let l = NeteaseLyrics {
            id: 1,
            lrc: Some(
                "[00:00.000] 作词 : DECO*27\n[00:01.000] 作曲 : DECO*27\n[00:02.000] 编曲 : X\n[00:03.275] 嫉妬秒読み入ります\n[00:05.329] 誰かさん\n[00:07.000]\n"
                    .into(),
            ),
            translation: Some("[by:x]\n[00:03.275] 嫉妒的倒计时\n[00:05.400] 某人\n".into()),
            romanization: Some("[00:03.275]shi tto\n".into()),
            ..Default::default()
        };
        let lines = netease_lines(&l);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "嫉妬秒読み入ります");
        assert_eq!(lines[0].translation.as_deref(), Some("嫉妒的倒计时"));
        assert_eq!(lines[0].romanization.as_deref(), Some("shi tto"));
        // 71 ms apart: still the same line.
        assert_eq!(lines[1].translation.as_deref(), Some("某人"));
        assert_eq!(lines[1].romanization, None);
        assert!(lines[2].text.is_empty());
    }

    #[test]
    fn yrc_wins_over_lrc_and_instrumental_is_empty() {
        let mut l = NeteaseLyrics {
            id: 1,
            lrc: Some("[00:01.000]a\n".into()),
            yrc: Some("[1000,500](1000,500,0)b\n".into()),
            ..Default::default()
        };
        assert_eq!(netease_lines(&l)[0].text, "b");
        l.instrumental = true;
        assert!(netease_lines(&l).is_empty());
    }
}
