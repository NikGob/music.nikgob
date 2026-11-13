//! LyricsPlus v2: millisecond line and syllable timing.
//!
//! LyricsPlus answers from several providers (`metadata.source`: Apple,
//! Musixmatch, QQ Music …). QQ Music's word timing of a song that is not
//! Chinese, Japanese or Korean is generated (words back to back, lengths in
//! proportion to the letters), so such a text keeps only its line times
//! ([`LyricsPlusText::words_dropped`]). Credit lines («Composed by：…»,
//! «作词 : …») are left out whatever the provider. A timing type it does not
//! know («Static», «None», …) is read by what the entries carry: syllable
//! times as word timing, line times as line timing, else a plain text.

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

    /// The lines alone (see [`Self::find_text`]).
    pub async fn find(
        &self,
        artist: &str,
        title: &str,
        album: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<Vec<LyricLine>>> {
        Ok(self.find_text(artist, title, album, duration).await?.map(|t| t.lines))
    }

    /// The text for `artist` (sent as given: «A, B» finds what «A» may
    /// not) and `title`; `None` when LyricsPlus has none.
    pub async fn find_text(
        &self,
        artist: &str,
        title: &str,
        album: Option<&str>,
        duration: Option<Duration>,
    ) -> Result<Option<LyricsPlusText>> {
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
            Ok(payload) => Ok(Some(payload.text()?).filter(|t| !t.lines.is_empty() || t.plain.is_some())),
            Err(MetadataError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// One LyricsPlus answer.
#[derive(Clone, Debug, PartialEq)]
pub struct LyricsPlusText {
    pub lines: Vec<LyricLine>,
    /// Who LyricsPlus took it from (`metadata.source`), when it says.
    pub provider: Option<String>,
    /// The provider's word timing was generated (QQ Music, a song that is
    /// not CJK) and was dropped: the lines keep their own times, which are
    /// less trustworthy than another source's line sync.
    pub words_dropped: bool,
    /// A text without any timing (a timing type LyricsPlus has no times
    /// for): the lines in order, one per line; `lines` is empty then.
    pub plain: Option<String>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: String,
    lyrics: Vec<Entry>,
    #[serde(default)]
    metadata: Option<Meta>,
}

#[derive(Default, Deserialize)]
struct Meta {
    #[serde(default)]
    source: Option<String>,
}

/// QQ Music (`"QQ Music"`, `"qq"`).
fn is_qq(provider: &str) -> bool {
    provider.to_ascii_lowercase().contains("qq")
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}'   // Hiragana, Katakana
        | '\u{3400}'..='\u{4DBF}' // CJK Extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{AC00}'..='\u{D7AF}' // Hangul syllables
        | '\u{1100}'..='\u{11FF}' // Hangul Jamo
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
    )
}

/// The lines are Chinese, Japanese or Korean: at least a tenth of their
/// letters are (an ideograph is a word, a Latin word many letters).
fn mostly_cjk(lines: &[LyricLine]) -> bool {
    let (mut cjk, mut letters) = (0usize, 0usize);
    for c in lines.iter().flat_map(|l| l.text.chars()).filter(|c| c.is_alphabetic()) {
        letters += 1;
        cjk += usize::from(is_cjk(c));
    }
    cjk > 0 && cjk * 10 >= letters
}

/// Credit labels a line may start with (before a `:` or `：`).
const CREDITS: &[&str] = &[
    "composed by", "composer", "lyrics by", "lyricist", "written by", "writer", "words by",
    "music by", "arranged by", "arranger", "producer", "produced by", "mixed by", "mastered by",
    "recorded by", "vocals by",
    "作词", "作曲", "编曲", "作詞", "編曲", "词", "曲", "制作人", "监制", "混音", "母带", "录音",
    "和声", "出品", "发行", "企划", "统筹", "吉他", "贝斯", "鼓",
];

/// «Composed by：Еремин Василий Сергеевич», «作词 : 周杰伦»: a credit, not
/// a sung line.
pub fn is_credit_line(text: &str) -> bool {
    let lower = text.trim_start().to_lowercase();
    CREDITS.iter().any(|label| {
        lower
            .strip_prefix(label)
            .is_some_and(|rest| rest.trim_start().starts_with([':', '：']))
    })
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    time: u64,
    #[serde(default)]
    duration: u64,
    #[serde(default)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Timing {
    Word,
    Line,
    Plain,
}

impl Payload {
    fn text(mut self) -> Result<LyricsPlusText> {
        let provider = self
            .metadata
            .take()
            .and_then(|m| m.source)
            .filter(|p| !p.trim().is_empty());
        let timing = self.timing();
        if timing == Timing::Plain {
            let text: Vec<String> = self
                .lyrics
                .into_iter()
                .map(|e| e.text.trim().to_owned())
                .filter(|t| !t.is_empty() && !is_credit_line(t))
                .collect();
            let plain = (!text.is_empty()).then(|| text.join("\n"));
            return Ok(LyricsPlusText { lines: Vec::new(), provider, words_dropped: false, plain });
        }
        let mut lines = self.lines()?;
        lines.retain(|l| !is_credit_line(&l.text));
        let generated = provider.as_deref().is_some_and(is_qq) && !mostly_cjk(&lines);
        let words_dropped = generated && lines.iter().any(LyricLine::is_word_synced);
        if generated {
            for l in &mut lines {
                l.words.clear();
            }
        }
        Ok(LyricsPlusText { lines, provider, words_dropped, plain: None })
    }

    /// How the entries are timed: by the type, or for a type not known by
    /// what they carry.
    fn timing(&self) -> Timing {
        if self.kind.eq_ignore_ascii_case("word") || self.kind.eq_ignore_ascii_case("syllable") {
            return Timing::Word;
        }
        if self.kind.eq_ignore_ascii_case("line") {
            return Timing::Line;
        }
        if self.lyrics.iter().any(|e| e.syllabus.iter().any(|s| s.time > 0 || s.duration > 0)) {
            Timing::Word
        } else if self.lyrics.iter().any(|e| e.time > 0 || e.duration > 0) {
            Timing::Line
        } else {
            Timing::Plain
        }
    }

    fn lines(self) -> Result<Vec<LyricLine>> {
        let word = match self.timing() {
            Timing::Word => true,
            Timing::Line => false,
            Timing::Plain => return Err(MetadataError::Parse("LyricsPlus text has no timing".into())),
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

    /// A QQ Music answer of `type: Word` with `lines` (text, time) whose
    /// words are cut at spaces, back to back.
    fn qq(lines: &[(&str, u64)]) -> String {
        let entries: Vec<String> = lines
            .iter()
            .map(|(text, time)| {
                let mut t = *time;
                let words: Vec<String> = text
                    .split_inclusive(' ')
                    .map(|w| {
                        let d = 100 * w.chars().count() as u64;
                        let s = format!(r#"{{"time":{t},"duration":{d},"text":{}}}"#, serde_json::to_string(w).unwrap());
                        t += d;
                        s
                    })
                    .collect();
                format!(
                    r#"{{"time":{time},"duration":{},"text":{},"syllabus":[{}]}}"#,
                    t - time,
                    serde_json::to_string(text).unwrap(),
                    words.join(",")
                )
            })
            .collect();
        format!(r#"{{"type":"Word","metadata":{{"source":"QQ Music"}},"lyrics":[{}]}}"#, entries.join(","))
    }

    #[test]
    fn qq_word_timing_of_a_russian_song_is_dropped_with_the_credits() {
        let json = qq(&[
            ("Lyrics by：Еремин Василий Сергеевич", 0),
            ("Composed by：Еремин Василий Сергеевич", 500),
            ("Ты мой лёд ", 12_000),
            ("И я таю ", 15_000),
        ]);
        let text = serde_json::from_str::<Payload>(&json).unwrap().text().unwrap();
        assert_eq!(text.provider.as_deref(), Some("QQ Music"));
        assert!(text.words_dropped);
        let got: Vec<(&str, u64)> = text.lines.iter().map(|l| (l.text.as_str(), l.start.as_millis() as u64)).collect();
        assert_eq!(got, [("Ты мой лёд ", 12_000), ("И я таю ", 15_000)]);
        assert!(text.lines.iter().all(|l| l.words.is_empty()));
        assert_eq!(nikgob_core::lyrics::LyricsSync::of(&text.lines), Some(nikgob_core::lyrics::LyricsSync::Line));
    }

    #[test]
    fn qq_word_timing_of_a_chinese_song_stays() {
        let json = qq(&[("作词 : 周杰伦", 0), ("作曲：周杰伦", 300), ("窗外的 麻雀 ", 20_000), ("在电线杆上 多嘴 ", 24_000)]);
        let text = serde_json::from_str::<Payload>(&json).unwrap().text().unwrap();
        assert!(!text.words_dropped);
        assert_eq!(text.lines.len(), 2, "no credits: {:?}", text.lines);
        assert!(text.lines.iter().all(LyricLine::is_word_synced));
    }

    #[test]
    fn other_providers_keep_their_words_and_lose_credits() {
        let json = r#"{"type":"Word","metadata":{"source":"Apple"},"lyrics":[{"time":0,"duration":500,"text":"Written by: Somebody"},{"time":1000,"duration":1000,"text":"Hello ","syllabus":[{"time":1000,"duration":400,"text":"Hel"},{"time":1400,"duration":600,"text":"lo "}]}]}"#;
        let text = serde_json::from_str::<Payload>(json).unwrap().text().unwrap();
        assert!(!text.words_dropped);
        assert_eq!(text.lines.len(), 1);
        assert_eq!(text.lines[0].words.len(), 2);
        // Not a credit: a sung line that mentions one.
        assert!(!is_credit_line("Producer of my dreams"));
        assert!(is_credit_line("  PRODUCER : X"));
        assert!(is_credit_line("编曲：X"));
    }

    #[test]
    fn an_unknown_timing_type_with_line_times_reads_as_line_timing() {
        let payload: Payload = serde_json::from_str(r#"{"type":"Static","lyrics":[{"time":2000,"duration":1500,"text":"Второй"},{"time":500,"duration":1500,"text":"Первый"}],"metadata":{"source":"Apple"}}"#).unwrap();
        let t = payload.text().unwrap();
        assert_eq!(t.plain, None);
        assert_eq!(t.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Первый", "Второй"]);
        assert_eq!(t.lines[0].start, Duration::from_millis(500));
        assert!(t.lines.iter().all(|l| l.words.is_empty()));
    }

    #[test]
    fn an_unknown_timing_type_without_times_is_a_plain_text() {
        let payload: Payload = serde_json::from_str(r#"{"type":"None","lyrics":[{"text":"Раз"},{"time":0,"duration":0,"text":"Два"},{"text":"Composed by：X"},{"text":" "}]}"#).unwrap();
        let t = payload.text().unwrap();
        assert!(t.lines.is_empty());
        assert_eq!(t.plain.as_deref(), Some("Раз\nДва"));
    }

    #[test]
    fn an_unknown_timing_type_with_syllables_keeps_the_words() {
        let payload: Payload = serde_json::from_str(r#"{"type":"Karaoke","lyrics":[{"time":1000,"duration":1000,"text":"ab","syllabus":[{"time":1000,"duration":400,"text":"a"},{"time":1400,"duration":600,"text":"b"}]}]}"#).unwrap();
        let t = payload.text().unwrap();
        assert_eq!(t.lines[0].words.len(), 2);
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
