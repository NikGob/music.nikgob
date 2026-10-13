//! QQ Music: line-synced LRC (and translation) from the public web endpoints.
//!
//! Search uses `smartbox_new` (it answers where `client_search_cp` returns 500),
//! which gives no duration; to make up for it a hit is only accepted when the
//! artist matches and the lyrics do not run past the end of the track.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

use crate::cn::{self, Hit};
use crate::error::{MetadataError, Result};
use crate::http::{Http, Service, base_url, build_url, parse_json};
use crate::lrc::parse_lrc;
use nikgob_core::lyrics::LyricLine;

const DEFAULT_BASE: &str = "https://c.y.qq.com/";
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const HEADERS: &[(&str, &str)] = &[("User-Agent", BROWSER_UA), ("Referer", "https://y.qq.com/")];
const INSTRUMENTAL_MARK: &str = "此歌曲为没有填词的纯音乐";
/// Lyrics may not outlast the track by more than this.
const LYRICS_SLACK: Duration = Duration::from_secs(8);

/// Lyrics of one QQ Music song; both parts are raw LRC text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QqLyrics {
    pub mid: String,
    pub lrc: Option<String>,
    pub translation: Option<String>,
}

#[derive(Clone, Debug)]
pub struct QqMusic {
    http: Http,
    base: Url,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SmartboxResponse {
    data: SmartboxData,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SmartboxData {
    song: SmartboxSongs,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SmartboxSongs {
    itemlist: Vec<SmartboxItem>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SmartboxItem {
    mid: String,
    name: String,
    singer: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LyricResponse {
    lyric: Option<String>,
    trans: Option<String>,
}

impl QqMusic {
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

    async fn search(&self, query: &str) -> Result<Vec<SmartboxItem>> {
        let url = build_url(
            &self.base,
            "splcloud/fcgi-bin/smartbox_new.fcg",
            &[("key", query), ("format", "json")],
        )?;
        let body = self.http.get_body_with(Service::Qq, url, HEADERS).await?;
        let r: SmartboxResponse = parse_json(strip_jsonp(&body))?;
        Ok(r.data.song.itemlist)
    }

    /// Lyrics of a known song mid; `None` when QQ Music has none.
    pub async fn lyrics(&self, mid: &str) -> Result<Option<QqLyrics>> {
        let url = build_url(
            &self.base,
            "lyric/fcgi-bin/fcg_query_lyric_new.fcg",
            &[("songmid", mid), ("format", "json"), ("nobase64", "1")],
        )?;
        let body = match self.http.get_body_with(Service::Qq, url, HEADERS).await {
            Ok(b) => b,
            Err(MetadataError::NotFound) => return Ok(None),
            Err(e) => return Err(e),
        };
        let r: LyricResponse = parse_json(strip_jsonp(&body))?;
        let clean = |s: Option<String>| s.map(|s| decode_text(&s)).filter(|s| !s.trim().is_empty());
        let out = QqLyrics {
            mid: mid.to_owned(),
            lrc: clean(r.lyric),
            translation: clean(r.trans),
        };
        Ok(out.lrc.is_some().then_some(out))
    }

    /// Searches "title artist" (then the title alone), takes the first hit with a
    /// matching title and artist whose lyrics fit the track length, and returns them.
    pub async fn find(
        &self,
        artist: &str,
        title: &str,
        _album: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<QqLyrics>> {
        let mut tried = Vec::new();
        for with_artist in [true, false] {
            let q = cn::query(artist, title, with_artist);
            if q.is_empty() || tried.contains(&q) {
                continue;
            }
            let items = self.search(&q).await?;
            tried.push(q);
            let mut hits: Vec<Hit<String>> = items
                .into_iter()
                .map(|s| Hit {
                    payload: s.mid,
                    title: s.name,
                    artists: cn::split_artists(&s.singer),
                    duration: None,
                })
                .collect();
            // Try every acceptable hit (there is no duration to rank them by).
            while let Some(hit) = cn::pick(hits.clone(), artist, title, None) {
                hits.retain(|h| h.payload != hit.payload);
                if let Some(found) = self.lyrics(&hit.payload).await?
                    && fits(&found, duration)
                {
                    return Ok(Some(found));
                }
            }
        }
        Ok(None)
    }
}

/// The lyrics must not run past the end of a track of the known length.
fn fits(l: &QqLyrics, duration: Option<Duration>) -> bool {
    let (Some(limit), Some(lrc)) = (duration, l.lrc.as_deref()) else {
        return true;
    };
    parse_lrc(lrc)
        .last()
        .is_none_or(|(t, _)| *t <= limit + LYRICS_SLACK)
}

/// The lines to show (empty for instrumentals): credit lines dropped, translation attached.
pub fn qq_lines(l: &QqLyrics) -> Vec<LyricLine> {
    let Some(lrc) = l.lrc.as_deref() else {
        return Vec::new();
    };
    if lrc.contains(INSTRUMENTAL_MARK) {
        return Vec::new();
    }
    let mut lines = cn::strip_credits(crate::lrc::parse_lrc_lines(lrc));
    if let Some(t) = l.translation.as_deref() {
        cn::attach(&mut lines, t, false);
    }
    lines
}

/// `MusicJsonCallback({...})` -> `{...}`.
fn strip_jsonp(body: &[u8]) -> &[u8] {
    let (Some(a), Some(b)) = (
        body.iter().position(|c| *c == b'{'),
        body.iter().rposition(|c| *c == b'}'),
    ) else {
        return body;
    };
    if a < b { &body[a..=b] } else { body }
}

/// LRC text from the lyric field: plain, or base64 when `nobase64` was ignored;
/// HTML entities decoded.
fn decode_text(raw: &str) -> String {
    let t = raw.trim();
    let text = if t.starts_with('[') {
        t.to_owned()
    } else {
        base64_decode(t)
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_else(|| t.to_owned())
    };
    decode_entities(&text)
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace()) {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Decodes `&#58;`, `&#x3A;`, `&apos;`, `&amp;`, `&quot;`, `&lt;`, `&gt;`, `&nbsp;` once.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|e| *e <= 10).and_then(|e| {
            let name = &rest[1..e];
            let ch = match name {
                "apos" => '\'',
                "amp" => '&',
                "quot" => '"',
                "lt" => '<',
                "gt" => '>',
                "nbsp" => ' ',
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(h) => u32::from_str_radix(h, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)?
                }
            };
            Some((ch, e + 1))
        });
        match decoded {
            Some((ch, len)) => {
                out.push(ch);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities_are_decoded() {
        assert_eq!(
            decode_entities("[00:01&#46;00]I&#32;don&apos;t&#58; &#x41; &amp; &bogus; &"),
            "[00:01.00]I don't: A & &bogus; &"
        );
    }

    #[test]
    fn base64_lyrics_and_jsonp_are_handled() {
        // "[00:01.00]hi"
        assert_eq!(decode_text("WzAwOjAxLjAwXWhp"), "[00:01.00]hi");
        assert_eq!(decode_text("[00:01.00]plain"), "[00:01.00]plain");
        let body = br#"MusicJsonCallback({"lyric":"x"});"#;
        assert_eq!(strip_jsonp(body), br#"{"lyric":"x"}"#);
    }

    #[test]
    fn lines_drop_header_credits_and_get_translation() {
        let l = QqLyrics {
            mid: "m".into(),
            lrc: Some(
                "[ti:Ghost]\n[ar:Eve]\n[00:00.00]ゴースト - Eve\n[00:00.26]词：Eve\n[00:00.29]曲：Eve\n[00:00.62]制作人：Eve\n[00:01.07]一行目\n[00:04.76]二行目\n"
                    .into(),
            ),
            translation: Some("[00:01.07]第一行\n[00:04.76]//\n".into()),
        };
        let lines = qq_lines(&l);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "一行目");
        assert_eq!(lines[0].translation.as_deref(), Some("第一行"));
        assert_eq!(lines[1].translation, None);
    }

    #[test]
    fn instrumental_has_no_lines() {
        let l = QqLyrics {
            mid: "m".into(),
            lrc: Some("[00:00.00]此歌曲为没有填词的纯音乐，请您欣赏\n".into()),
            translation: None,
        };
        assert!(qq_lines(&l).is_empty());
    }
}
