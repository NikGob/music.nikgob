//! Parser for the TTML lyrics of the AMLL TTML DB (the Apple Music flavour of TTML).
//!
//! A `<p>` is a line, its timed `<span>`s are the words (syllables). Background
//! vocals (`ttm:role="x-bg"`) become their own lines right after the line they
//! belong to, translations / romanizations come from inline `x-translation` /
//! `x-roman` spans or from the `iTunesMetadata` block of the head.

use std::collections::HashMap;
use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord};
use roxmltree::{Document, Node};

use crate::error::{MetadataError, Result};

/// Parsed TTML lyrics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TtmlLyrics {
    /// Sorted by start; a background line follows the line it belongs to.
    pub lines: Vec<LyricLine>,
    /// `<amll:meta key=".." value=".."/>` pairs from the head, in file order
    /// (`musicName`, `artists`, `isrc`, `ncmMusicId`, ...; keys repeat).
    pub meta: Vec<(String, String)>,
}

impl TtmlLyrics {
    /// Every value of a meta key.
    pub fn meta_values<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.meta
            .iter()
            .filter(move |(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Parses AMLL / Apple Music TTML.
///
/// Fails only on malformed XML or a document whose root is not `<tt>`; unknown
/// elements are ignored.
pub fn parse_ttml(xml: &str) -> Result<TtmlLyrics> {
    let xml = xml.trim_start_matches('\u{feff}');
    let doc = Document::parse(xml).map_err(|e| MetadataError::Parse(format!("TTML: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "tt" {
        return Err(MetadataError::Parse("TTML: no <tt> root".into()));
    }

    let head = child_named(root, "head");
    let meta = head.map(read_meta).unwrap_or_default();
    let agents = read_agents(head);
    let (translations, romanizations) = head.map(read_head_texts).unwrap_or_default();

    let paragraphs: Vec<Node<'_, '_>> = child_named(root, "body")
        .map(|b| b.descendants().filter(|n| is_named(*n, "p")).collect())
        .unwrap_or_default();

    // Without declared agents the first one used by a line is the main side.
    let main_agent = agents.main.clone().or_else(|| {
        paragraphs
            .iter()
            .find_map(|p| attr(*p, "agent"))
            .map(str::to_owned)
    });

    let mut groups: Vec<Vec<LyricLine>> = Vec::new();
    for p in paragraphs {
        let duet = attr(p, "agent")
            .is_some_and(|id| main_agent.as_deref() != Some(id) && agents.is_person(id));
        let mut part = Part::default();
        collect(p, &mut part);

        let mut group = Vec::new();
        let begin = attr(p, "begin").and_then(parse_time);
        let end = attr(p, "end").and_then(parse_time);
        let key = attr(p, "key");

        let mut main = build_line(&mut part, begin, end, duet, false);
        if main.translation.is_none() {
            main.translation = key.and_then(|k| translations.get(k)).cloned();
        }
        if main.romanization.is_none() {
            main.romanization = key.and_then(|k| romanizations.get(k)).cloned();
        }
        // A paragraph holding only background vocals has no main line of its own.
        let only_bg = main.text.is_empty() && !part.bgs.is_empty();
        if !only_bg && (begin.is_some() || !main.text.is_empty() || main.is_word_synced()) {
            group.push(main);
        }
        for mut bg in part.bgs {
            let mut line = build_line(&mut bg.part, bg.begin, bg.end, duet, true);
            line.duet = duet;
            if !line.text.is_empty() {
                group.push(line);
            }
        }
        if !group.is_empty() {
            groups.push(group);
        }
    }
    groups.sort_by_key(|g| g[0].start);

    Ok(TtmlLyrics {
        lines: groups.into_iter().flatten().collect(),
        meta,
    })
}

/// What one `<p>` (or one background span) holds.
#[derive(Default)]
struct Part {
    words: Vec<RawWord>,
    /// Untimed text (line-synced TTML).
    plain: String,
    translation: Option<String>,
    romanization: Option<String>,
    bgs: Vec<Bg>,
}

struct RawWord {
    start: Duration,
    end: Option<Duration>,
    text: String,
}

struct Bg {
    begin: Option<Duration>,
    end: Option<Duration>,
    part: Part,
}

fn collect(node: Node<'_, '_>, part: &mut Part) {
    for child in node.children() {
        if child.is_text() {
            let t = child.text().unwrap_or_default();
            if t.trim().is_empty() {
                // Spacing between spans belongs to the word before it.
                if !t.is_empty() {
                    push_space(part);
                }
            } else {
                part.plain.push_str(t);
            }
            continue;
        }
        if !child.is_element() {
            continue;
        }
        match attr(child, "role") {
            Some("x-translation") => {
                let t = squash(&all_text(child));
                if !t.is_empty() && part.translation.is_none() {
                    part.translation = Some(t);
                }
            }
            Some("x-roman") => {
                let t = squash(&all_text(child));
                if !t.is_empty() && part.romanization.is_none() {
                    part.romanization = Some(t);
                }
            }
            Some("x-bg") => {
                let mut inner = Part::default();
                collect(child, &mut inner);
                part.bgs.push(Bg {
                    begin: attr(child, "begin").and_then(parse_time),
                    end: attr(child, "end").and_then(parse_time),
                    part: inner,
                });
            }
            _ => {
                let start = attr(child, "begin").and_then(parse_time);
                let has_timed_children = child
                    .descendants()
                    .skip(1)
                    .any(|d| d.is_element() && attr(d, "begin").is_some());
                match start {
                    Some(start) if !has_timed_children => {
                        let text = all_text(child);
                        if text.trim().is_empty() {
                            push_space(part);
                        } else {
                            part.words.push(RawWord {
                                start,
                                end: attr(child, "end").and_then(parse_time),
                                text,
                            });
                        }
                    }
                    _ => collect(child, part),
                }
            }
        }
    }
}

fn push_space(part: &mut Part) {
    if let Some(w) = part.words.last_mut()
        && !w.text.ends_with(char::is_whitespace)
    {
        w.text.push(' ');
    }
}

fn build_line(
    part: &mut Part,
    begin: Option<Duration>,
    end: Option<Duration>,
    duet: bool,
    background: bool,
) -> LyricLine {
    let raw = std::mem::take(&mut part.words);
    let mut words: Vec<LyricWord> = Vec::with_capacity(raw.len());
    for (i, w) in raw.iter().enumerate() {
        let wend = w
            .end
            .or_else(|| raw.get(i + 1).map(|n| n.start))
            .or(end)
            .unwrap_or(w.start)
            .max(w.start);
        words.push(LyricWord {
            start: w.start,
            end: wend,
            text: w.text.clone(),
        });
    }

    if let Some(first) = words.first_mut() {
        first.text = first.text.trim_start().to_owned();
    }
    if let Some(last) = words.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }

    let mut plain = squash(&part.plain);
    if background {
        strip_parens(&mut words, &mut plain);
    }
    let text = if words.is_empty() {
        plain
    } else {
        words.iter().map(|w| w.text.as_str()).collect::<String>()
    };

    let start = begin
        .or_else(|| words.first().map(|w| w.start))
        .unwrap_or_default();
    let end = end.or_else(|| words.last().map(|w| w.end));
    LyricLine {
        start,
        end,
        text,
        words,
        background,
        duet,
        translation: part.translation.take(),
        romanization: part.romanization.take(),
    }
}

/// Background vocals come as `(oh yeah)`; the view styles them itself.
fn strip_parens(words: &mut Vec<LyricWord>, plain: &mut String) {
    const OPEN: [char; 2] = ['(', '（'];
    const CLOSE: [char; 2] = [')', '）'];
    if words.is_empty() {
        let t = plain.trim();
        if t.starts_with(OPEN) && t.ends_with(CLOSE) {
            let inner = &t[t.chars().next().map_or(0, char::len_utf8)..];
            let inner = &inner[..inner.len() - inner.chars().last().map_or(0, char::len_utf8)];
            *plain = inner.trim().to_owned();
        }
        return;
    }
    let joined: String = words.iter().map(|w| w.text.as_str()).collect();
    let t = joined.trim();
    if !(t.starts_with(OPEN) && t.ends_with(CLOSE)) {
        return;
    }
    if let Some(first) = words.first_mut() {
        first.text = first.text.trim_start().trim_start_matches(OPEN).to_owned();
    }
    if let Some(last) = words.last_mut() {
        last.text = last.text.trim_end().trim_end_matches(CLOSE).to_owned();
    }
    // A bare "(" / ")" span leaves its time to the neighbour.
    if let [.., prev, last] = words.as_mut_slice()
        && last.text.is_empty()
    {
        prev.end = prev.end.max(last.end);
    }
    words.retain(|w| !w.text.is_empty());
    if let Some(last) = words.last_mut() {
        last.text = last.text.trim_end().to_owned();
    }
}

struct Agents {
    /// `(xml:id, is_person)` in file order.
    list: Vec<(String, bool)>,
    main: Option<String>,
}

impl Agents {
    fn is_person(&self, id: &str) -> bool {
        // A group / "other" agent (everyone together) never flips the side.
        self.list
            .iter()
            .find(|(i, _)| i == id)
            .is_none_or(|(_, person)| *person)
    }
}

fn read_agents(head: Option<Node<'_, '_>>) -> Agents {
    let mut list = Vec::new();
    if let Some(head) = head {
        for n in head.descendants().filter(|n| is_named(*n, "agent")) {
            if let Some(id) = attr(n, "id") {
                let person = attr(n, "type").is_none_or(|t| t.eq_ignore_ascii_case("person"));
                list.push((id.to_owned(), person));
            }
        }
    }
    let main = list
        .iter()
        .find(|(_, person)| *person)
        .map(|(id, _)| id.clone());
    Agents { list, main }
}

fn read_meta(head: Node<'_, '_>) -> Vec<(String, String)> {
    head.descendants()
        .filter(|n| is_named(*n, "meta"))
        .filter_map(|n| Some((attr(n, "key")?.to_owned(), attr(n, "value")?.to_owned())))
        .collect()
}

type TextMap = HashMap<String, String>;

/// `iTunesMetadata` translations and transliterations keyed by the line key.
fn read_head_texts(head: Node<'_, '_>) -> (TextMap, TextMap) {
    let mut translations = TextMap::new();
    let mut romanizations = TextMap::new();
    for section in head.descendants().filter(|n| n.is_element()) {
        let target = match section.tag_name().name() {
            "translation" => &mut translations,
            "transliteration" => &mut romanizations,
            _ => continue,
        };
        for t in section.children().filter(|n| is_named(*n, "text")) {
            let (Some(key), text) = (attr(t, "for"), squash(&all_text(t))) else {
                continue;
            };
            if !text.is_empty() {
                target.entry(key.to_owned()).or_insert(text);
            }
        }
    }
    (translations, romanizations)
}

fn child_named<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| is_named(*n, name))
}

fn is_named(node: Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}

/// Attribute by local name (the namespace prefix does not matter).
fn attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name() == name)
        .map(|a| a.value())
}

fn all_text(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

/// Collapses whitespace runs and trims.
fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// TTML time expression: `ss.mmm`, `m:ss.mmm`, `h:mm:ss.mmm`, or an offset time
/// with a `h`/`m`/`s`/`ms` suffix (`12.5s`, `1234ms`). `None` for anything else
/// (including values too large to represent).
fn parse_time(raw: &str) -> Option<Duration> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains(':') {
        let parts: Vec<&str> = s.split(':').collect();
        let (h, m, sec) = match parts.as_slice() {
            [m, s] => ("0", *m, *s),
            [h, m, s] => (*h, *m, *s),
            _ => return None,
        };
        let h = Duration::from_secs(digits(h)?).checked_mul(3600)?;
        let m = Duration::from_secs(digits(m)?).checked_mul(60)?;
        return h.checked_add(m)?.checked_add(decimal(sec)?);
    }
    if let Some(v) = s.strip_suffix("ms") {
        return Some(decimal(v)? / 1000);
    }
    if let Some(v) = s.strip_suffix('h') {
        return decimal(v)?.checked_mul(3600);
    }
    if let Some(v) = s.strip_suffix('m') {
        return decimal(v)?.checked_mul(60);
    }
    decimal(s.strip_suffix('s').unwrap_or(s))
}

fn digits(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Seconds as `12`, `12.5`, `.5`: the fraction is a decimal fraction.
fn decimal(s: &str) -> Option<Duration> {
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    let secs = if int.is_empty() { 0 } else { digits(int)? };
    let mut nanos = 0u32;
    if !frac.is_empty() {
        if !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut f: String = frac.chars().take(9).collect();
        while f.len() < 9 {
            f.push('0');
        }
        nanos = f.parse().ok()?;
    }
    Some(Duration::new(secs, nanos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    const HEAD: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:amll="http://www.example.com/ns/amll" itunes:timing="Word">"#;

    fn doc(head_inner: &str, body_inner: &str) -> String {
        format!(
            "{HEAD}<head><metadata><ttm:agent type=\"person\" xml:id=\"v1\"/><ttm:agent type=\"person\" xml:id=\"v2\"/><ttm:agent type=\"group\" xml:id=\"v1000\"/>{head_inner}</metadata></head><body dur=\"4:21.819\"><div begin=\"1\" end=\"60\">{body_inner}</div></body></tt>"
        )
    }

    fn words(line: &LyricLine) -> Vec<&str> {
        line.words.iter().map(|w| w.text.as_str()).collect()
    }

    #[test]
    fn time_formats() {
        assert_eq!(parse_time("31.037"), Some(ms(31_037)));
        assert_eq!(parse_time("4:21.819"), Some(ms(261_819)));
        assert_eq!(parse_time("1:02:03.5"), Some(ms(3_723_500)));
        assert_eq!(parse_time("12.5s"), Some(ms(12_500)));
        assert_eq!(parse_time("1234ms"), Some(ms(1_234)));
        assert_eq!(parse_time("1.5ms"), Some(Duration::from_micros(1_500)));
        assert_eq!(parse_time("2m"), Some(ms(120_000)));
        assert_eq!(parse_time("1h"), Some(ms(3_600_000)));
        assert_eq!(parse_time("7"), Some(ms(7_000)));
        assert_eq!(parse_time(".5"), Some(ms(500)));
        assert_eq!(parse_time("0:00.05"), Some(ms(50)));
        assert_eq!(parse_time(" 3.25 "), Some(ms(3_250)));
        assert_eq!(parse_time("90:00.000"), Some(ms(5_400_000)));
    }

    #[test]
    fn bad_and_huge_times_do_not_panic() {
        for bad in [
            "",
            "abc",
            "1:2:3:4",
            "-1",
            "1..2",
            ":",
            "1:",
            ":1",
            "s",
            "ms",
            "1.x",
            "+5",
            "99999999999999999999",
            "18446744073709551615h",
            "18446744073709551615:00:00",
            "18446744073709551615:18446744073709551615",
            "1e5",
        ] {
            assert_eq!(parse_time(bad), None, "{bad:?}");
        }
        // Largest representable seconds is still fine.
        assert!(parse_time("18446744073709551615").is_some());
    }

    #[test]
    fn words_keep_the_spaces_between_spans_cjk() {
        let xml = doc(
            r#"<amll:meta key="musicName" value="自由的你"/><amll:meta key="artists" value="A"/><amll:meta key="artists" value="B"/>"#,
            r#"<p begin="31.037" end="34.238" itunes:key="L1" ttm:agent="v1"><span begin="31.037" end="31.423">当</span><span begin="31.423" end="31.942">眼</span> <span begin="31.942" end="32.5">睛</span></p>"#,
        );
        let t = parse_ttml(&xml).unwrap();
        assert_eq!(t.meta[0], ("musicName".into(), "自由的你".into()));
        assert_eq!(t.meta_values("artists").collect::<Vec<_>>(), ["A", "B"]);
        let l = &t.lines[0];
        assert_eq!(words(l), ["当", "眼 ", "睛"]);
        assert_eq!(l.text, "当眼 睛");
        assert_eq!((l.start, l.end), (ms(31_037), Some(ms(34_238))));
        assert_eq!((l.words[0].start, l.words[0].end), (ms(31_037), ms(31_423)));
        assert!(!l.duet && !l.background);
    }

    #[test]
    fn spaces_inside_spans_and_trailing_whitespace() {
        let xml = doc(
            "",
            r#"<p begin="1" end="3"><span begin="1" end="1.5">Hel</span><span begin="1.5" end="2">lo </span><span begin="2" end="3">world </span>
</p>"#,
        );
        let l = &parse_ttml(&xml).unwrap().lines[0];
        assert_eq!(words(l), ["Hel", "lo ", "world"]);
        assert_eq!(l.text, "Hello world");
        let joined: String = l.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(joined, l.text);
    }

    #[test]
    fn line_timing_falls_back_to_words() {
        let xml = doc(
            "",
            r#"<p><span begin="5" end="6">a</span> <span begin="6" end="7.5">b</span></p>"#,
        );
        let l = &parse_ttml(&xml).unwrap().lines[0];
        assert_eq!((l.start, l.end), (ms(5_000), Some(ms(7_500))));
        assert_eq!(l.text, "a b");
    }

    #[test]
    fn line_synced_ttml_has_no_words() {
        let xml = doc(
            "",
            r#"<p begin="3.5" end="6">Just a line</p><p begin="7" end="9">  Second
   line </p>"#,
        );
        let t = parse_ttml(&xml).unwrap();
        assert_eq!(t.lines.len(), 2);
        assert!(t.lines[0].words.is_empty());
        assert_eq!(t.lines[0].text, "Just a line");
        assert_eq!(t.lines[0].end, Some(ms(6_000)));
        assert_eq!(t.lines[1].text, "Second line");
    }

    #[test]
    fn inline_translation_and_romanization() {
        let xml = doc(
            "",
            r#"<p begin="1" end="3"><span begin="1" end="2">你</span><span begin="2" end="3">好</span><span ttm:role="x-translation" xml:lang="en">Hello</span><span ttm:role="x-roman">ni hao</span></p>"#,
        );
        let l = &parse_ttml(&xml).unwrap().lines[0];
        assert_eq!(l.text, "你好");
        assert_eq!(l.translation.as_deref(), Some("Hello"));
        assert_eq!(l.romanization.as_deref(), Some("ni hao"));
        assert_eq!(l.words.len(), 2);
    }

    #[test]
    fn translations_from_the_head_by_key() {
        let head = r#"<iTunesMetadata xmlns="http://music.apple.com/lyric-ttml-internal"><translations><translation xml:lang="ru"><text for="L1">Привет <span>мир</span></text><text for="L2">Второй</text></translation></translations><transliterations><transliteration xml:lang="zh-Latn"><text for="L1"><span>ni</span> <span>hao</span></text></transliteration></transliterations></iTunesMetadata>"#;
        let xml = doc(
            head,
            r#"<p begin="1" end="2" itunes:key="L1"><span begin="1" end="2">你好</span></p><p begin="3" end="4" itunes:key="L2"><span begin="3" end="4">x</span><span ttm:role="x-translation">inline wins</span></p><p begin="5" end="6" itunes:key="L3"><span begin="5" end="6">y</span></p>"#,
        );
        let t = parse_ttml(&xml).unwrap();
        assert_eq!(t.lines[0].translation.as_deref(), Some("Привет мир"));
        assert_eq!(t.lines[0].romanization.as_deref(), Some("ni hao"));
        assert_eq!(t.lines[1].translation.as_deref(), Some("inline wins"));
        assert_eq!(t.lines[2].translation, None);
    }

    #[test]
    fn background_vocals_become_a_line_after_their_parent() {
        let xml = doc(
            "",
            r#"<p begin="10" end="14" ttm:agent="v1"><span begin="10" end="11">Main</span> <span begin="11" end="12">line</span><span ttm:role="x-bg" begin="12" end="14"><span begin="12" end="13">(oh</span> <span begin="13" end="14">yeah)</span><span ttm:role="x-translation">ох да</span></span></p><p begin="11.5" end="12"><span begin="11.5" end="12">next</span></p>"#,
        );
        let t = parse_ttml(&xml).unwrap();
        assert_eq!(t.lines.len(), 3);
        assert_eq!(t.lines[0].text, "Main line");
        assert!(!t.lines[0].background);
        let bg = &t.lines[1];
        assert!(bg.background);
        assert_eq!(bg.text, "oh yeah");
        assert_eq!(words(bg), ["oh ", "yeah"]);
        assert_eq!((bg.start, bg.end), (ms(12_000), Some(ms(14_000))));
        assert_eq!(bg.translation.as_deref(), Some("ох да"));
        // Parent's own text does not leak the background text.
        assert_eq!(t.lines[0].words.len(), 2);
        // Sorted by the parent's start; the bg line stays glued to it.
        assert_eq!(t.lines[2].text, "next");
    }

    #[test]
    fn background_without_own_times_uses_its_words_and_unwrapped_parens_stay() {
        let xml = doc(
            "",
            r#"<p begin="1" end="5"><span begin="1" end="2">a</span><span ttm:role="x-bg"><span begin="3" end="4">(hey</span><span begin="4" end="5">)</span></span><span ttm:role="x-bg"><span begin="3" end="4">no (parens)</span></span></p>"#,
        );
        let t = parse_ttml(&xml).unwrap();
        assert_eq!(t.lines[1].text, "hey");
        assert_eq!(t.lines[1].start, ms(3_000));
        assert_eq!(t.lines[1].end, Some(ms(5_000)));
        assert_eq!(t.lines[2].text, "no (parens)");
    }

    #[test]
    fn duet_agents() {
        let xml = doc(
            "",
            r#"<p begin="1" end="2" ttm:agent="v1"><span begin="1" end="2">a</span></p><p begin="3" end="4" ttm:agent="v2"><span begin="3" end="4">b</span><span ttm:role="x-bg" begin="3" end="4"><span begin="3" end="4">c</span></span></p><p begin="5" end="6" ttm:agent="v1000"><span begin="5" end="6">together</span></p><p begin="7" end="8"><span begin="7" end="8">none</span></p>"#,
        );
        let t = parse_ttml(&xml).unwrap();
        let flags: Vec<(&str, bool)> = t.lines.iter().map(|l| (l.text.as_str(), l.duet)).collect();
        assert_eq!(
            flags,
            [
                ("a", false),
                ("b", true),
                ("c", true),
                ("together", false),
                ("none", false)
            ]
        );
    }

    #[test]
    fn duet_without_declared_agents_uses_first_seen_as_main() {
        let xml = format!(
            "{HEAD}<head/><body><div><p begin=\"1\" end=\"2\" ttm:agent=\"a\">x</p><p begin=\"3\" end=\"4\" ttm:agent=\"b\">y</p></div></body></tt>"
        );
        let t = parse_ttml(&xml).unwrap();
        assert!(!t.lines[0].duet);
        assert!(t.lines[1].duet);
    }

    #[test]
    fn lines_are_sorted_by_start() {
        let xml = doc(
            "",
            r#"<p begin="9" end="10">late</p><p begin="2" end="3">early</p>"#,
        );
        let t = parse_ttml(&xml).unwrap();
        assert_eq!(t.lines[0].text, "early");
        assert_eq!(t.lines[1].text, "late");
    }

    #[test]
    fn unknown_elements_are_ignored_and_empty_body_is_ok() {
        let xml = format!(
            "{HEAD}<head><styling><style xml:id=\"s\"/></styling></head><body><div><foo/></div></body></tt>"
        );
        let t = parse_ttml(&xml).unwrap();
        assert!(t.lines.is_empty() && t.meta.is_empty());
        assert!(
            parse_ttml("\u{feff}<tt xmlns=\"http://www.w3.org/ns/ttml\"/>")
                .unwrap()
                .lines
                .is_empty()
        );
    }

    #[test]
    fn errors_on_malformed_xml_and_wrong_root() {
        assert!(matches!(
            parse_ttml("<tt><p>"),
            Err(MetadataError::Parse(_))
        ));
        assert!(matches!(parse_ttml("hello"), Err(MetadataError::Parse(_))));
        assert!(matches!(
            parse_ttml("<html><body/></html>"),
            Err(MetadataError::Parse(_))
        ));
    }

    #[test]
    fn missing_span_end_uses_the_next_word_or_line_end() {
        let xml = doc(
            "",
            r#"<p begin="1" end="4"><span begin="1">a</span><span begin="2">b</span></p>"#,
        );
        let l = &parse_ttml(&xml).unwrap().lines[0];
        assert_eq!(l.words[0].end, ms(2_000));
        assert_eq!(l.words[1].end, ms(4_000));
    }
}
