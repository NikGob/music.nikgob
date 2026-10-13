//! LRC (synchronized lyrics) parser.

use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord};

/// Parses LRC text into `(timestamp, line)` pairs sorted by time.
///
/// Handles several timestamps per line (`[00:12.00][01:30.00]chorus`), `[offset:+500]`
/// (milliseconds; positive means the text appears earlier), 1-3 digit fractions
/// (`.5`, `.50`, `.500`), a `:` before the fraction (`[01:02:30]`), minutes over 59,
/// enhanced word tags (`<00:12.30>` are dropped) and ignores `[ar:]`-style metadata.
/// Empty text is kept: it marks an instrumental break.
pub fn parse_lrc(text: &str) -> Vec<(Duration, String)> {
    let (raw, offset_ms) = scan(text);
    raw.into_iter()
        .map(|r| (shift(r.ms, offset_ms), strip_word_tags(&r.rest)))
        .collect()
}

/// Like [`parse_lrc`], but keeps enhanced-LRC word tags as [`LyricWord`] timing.
///
/// Every `<mm:ss.xx>` starts a word that runs until the next tag; a word ends
/// where the next one starts. The last word ends at a trailing empty tag
/// (`<00:07.10>` after the text) or, without one, at the next line's start (one
/// second after its own start for the last line). `[offset:]` shifts the word
/// times as well. Lines without word tags have no `words` and `end: None`.
/// The line text is the same as [`parse_lrc`] gives.
pub fn parse_lrc_lines(text: &str) -> Vec<LyricLine> {
    let (raw, offset_ms) = scan(text);
    let starts: Vec<Duration> = raw.iter().map(|r| shift(r.ms, offset_ms)).collect();
    let mut out = Vec::with_capacity(raw.len());
    for (i, r) in raw.iter().enumerate() {
        let start = starts[i];
        let mut line = LyricLine::new(start, strip_word_tags(&r.rest));
        let fallback_end = starts
            .get(i + 1)
            .copied()
            .filter(|n| *n > start)
            .unwrap_or(start + Duration::from_secs(1));
        // Repeated stamps (`[t1][t2]text`) replay the first stamp's word times.
        let delta = i128::from(r.ms) - i128::from(r.base_ms);
        line.words = split_words(&r.rest, start, fallback_end, |ms| {
            let moved = (i128::from(ms) + delta).clamp(0, i128::from(u64::MAX)) as u64;
            shift(moved, offset_ms)
        });
        if let Some(last) = line.words.last() {
            line.end = Some(last.end);
        }
        out.push(line);
    }
    out
}

struct RawLine {
    /// Line time without the offset.
    ms: u64,
    /// The first timestamp written on the line (the word tags refer to it).
    base_ms: u64,
    /// Text after the `[..]` tags, word tags still in.
    rest: String,
}

/// Line stamps (expanded, sorted by time) and the `[offset:]` in milliseconds.
fn scan(text: &str) -> (Vec<RawLine>, i64) {
    let mut out: Vec<RawLine> = Vec::new();
    let mut offset_ms: i64 = 0;

    for raw in text.trim_start_matches('\u{feff}').lines() {
        let mut rest = raw.trim();
        let mut stamps: Vec<u64> = Vec::new();

        while let Some(inner) = rest.strip_prefix('[') {
            let Some(end) = inner.find(']') else { break };
            let tag = &inner[..end];
            if let Some(ms) = parse_timestamp(tag) {
                stamps.push(ms);
            } else if stamps.is_empty() && is_metadata_tag(tag) {
                if let Some(v) = tag
                    .strip_prefix("offset:")
                    .or_else(|| tag.strip_prefix("OFFSET:"))
                {
                    offset_ms = v
                        .trim()
                        .trim_start_matches('+')
                        .parse()
                        .unwrap_or(offset_ms);
                }
            } else {
                break;
            }
            rest = inner[end + 1..].trim_start();
        }

        let Some(&base_ms) = stamps.first() else {
            continue;
        };
        for ms in stamps {
            out.push(RawLine {
                ms,
                base_ms,
                rest: rest.to_owned(),
            });
        }
    }

    out.sort_by_key(|r| r.ms);
    (out, offset_ms)
}

fn shift(ms: u64, offset_ms: i64) -> Duration {
    let shifted = if offset_ms >= 0 {
        ms.saturating_sub(offset_ms.unsigned_abs())
    } else {
        ms.saturating_add(offset_ms.unsigned_abs())
    };
    Duration::from_millis(shifted)
}

/// Words of a line body; empty when it has no word tags.
fn split_words(
    body: &str,
    line_start: Duration,
    fallback_end: Duration,
    time: impl Fn(u64) -> Duration,
) -> Vec<LyricWord> {
    // (start, text) pieces: the text before the first tag belongs to the line start.
    let mut marks: Vec<(Duration, &str)> = Vec::new();
    let mut pending = line_start;
    let mut cursor = 0;
    let mut search = 0;
    let mut tagged = false;
    while let Some(rel) = body[search..].find('<') {
        let at = search + rel;
        let after = &body[at + 1..];
        match after
            .find('>')
            .and_then(|e| Some((e, parse_timestamp(&after[..e])?)))
        {
            Some((end, ms)) => {
                marks.push((pending, &body[cursor..at]));
                tagged = true;
                pending = time(ms);
                cursor = at + 1 + end + 1;
                search = cursor;
            }
            None => search = at + 1,
        }
    }
    if !tagged {
        return Vec::new();
    }
    marks.push((pending, &body[cursor..]));

    let mut words: Vec<LyricWord> = Vec::new();
    for (i, &(start, text)) in marks.iter().enumerate() {
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            // A bare space between tags belongs to the word before it.
            if !text.is_empty()
                && let Some(prev) = words.last_mut()
                && !prev.text.ends_with(' ')
            {
                prev.text.push(' ');
            }
            continue;
        }
        if text.starts_with(char::is_whitespace)
            && let Some(prev) = words.last_mut()
            && !prev.text.ends_with(' ')
        {
            prev.text.push(' ');
        }
        let mut word_text = collapsed;
        if text.ends_with(char::is_whitespace) {
            word_text.push(' ');
        }
        let end = marks.get(i + 1).map_or(fallback_end, |m| m.0).max(start);
        words.push(LyricWord {
            start,
            end,
            text: word_text,
        });
    }
    if let Some(last) = words.last_mut() {
        let trimmed = last.text.trim_end().len();
        last.text.truncate(trimmed);
    }
    words
}

/// `mm:ss`, `mm:ss.ff`, `mm:ss:ff` -> milliseconds.
fn parse_timestamp(tag: &str) -> Option<u64> {
    let mut parts = tag.split(':');
    let minutes = parts.next()?;
    let seconds = parts.next()?;
    let colon_fraction = parts.next();
    if parts.next().is_some() {
        return None;
    }
    let (secs, dot_fraction) = match seconds.split_once('.') {
        Some((s, f)) => (s, Some(f)),
        None => (seconds, None),
    };
    if colon_fraction.is_some() && dot_fraction.is_some() {
        return None;
    }
    let fraction = colon_fraction.or(dot_fraction);
    if !all_digits(minutes) || !all_digits(secs) || fraction.is_some_and(|f| !all_digits(f)) {
        return None;
    }
    let minutes: u64 = minutes.parse().ok()?;
    let secs: u64 = secs.parse().ok()?;
    let frac_ms: u64 = fraction.map_or(0, |f| {
        let mut digits: String = f.chars().take(3).collect();
        while digits.len() < 3 {
            digits.push('0');
        }
        digits.parse().unwrap_or(0)
    });
    // Garbage like `[99999999999999999999:00]` must not overflow.
    minutes
        .checked_mul(60_000)?
        .checked_add(secs.checked_mul(1000)?)?
        .checked_add(frac_ms)
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `[ar:..]`, `[ti:..]`, `[offset:..]`, `[by:..]`, ...
fn is_metadata_tag(tag: &str) -> bool {
    tag.split_once(':').is_some_and(|(key, _)| {
        !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphabetic() || b == b'#')
    })
}

/// Removes enhanced-LRC `<mm:ss.xx>` word timestamps and trims.
fn strip_word_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        match after.find('>') {
            Some(end) if parse_timestamp(&after[..end]).is_some() => {
                out.push_str(&rest[..start]);
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    // Word tags leave doubled spaces behind.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn basic_lines_and_precision_variants() {
        let lrc = "[00:12.34]hundredths\n[00:13.5]tenths\n[00:14.567]thousandths\n[00:15]whole\n[01:02:30]colon fraction\n[75:00.00]long track\n";
        assert_eq!(
            parse_lrc(lrc),
            vec![
                (ms(12_340), "hundredths".to_owned()),
                (ms(13_500), "tenths".to_owned()),
                (ms(14_567), "thousandths".to_owned()),
                (ms(15_000), "whole".to_owned()),
                (ms(62_300), "colon fraction".to_owned()),
                (ms(4_500_000), "long track".to_owned()),
            ]
        );
    }

    #[test]
    fn fraction_digits_are_a_decimal_fraction() {
        // .05 is 50 ms, not 5 ms and not 500 ms.
        assert_eq!(parse_lrc("[00:01.05]x")[0].0, ms(1_050));
        assert_eq!(parse_lrc("[00:01.005]x")[0].0, ms(1_005));
        assert_eq!(parse_lrc("[00:01.5]x")[0].0, ms(1_500));
        // More than three digits: truncated to milliseconds.
        assert_eq!(parse_lrc("[00:01.12345]x")[0].0, ms(1_123));
    }

    #[test]
    fn multiple_timestamps_per_line_are_expanded_and_sorted() {
        let lrc = "[00:30.00][00:10.00]chorus\n[00:20.00]verse";
        assert_eq!(
            parse_lrc(lrc),
            vec![
                (ms(10_000), "chorus".to_owned()),
                (ms(20_000), "verse".to_owned()),
                (ms(30_000), "chorus".to_owned()),
            ]
        );
    }

    #[test]
    fn offset_shifts_every_line() {
        // Positive offset: lyrics appear earlier.
        let lrc = "[offset:+500]\n[00:10.00]a\n[00:00.20]b";
        assert_eq!(
            parse_lrc(lrc),
            vec![(ms(0), "b".to_owned()), (ms(9_500), "a".to_owned())],
            "never underflows"
        );
        // Negative offset: later; the tag may come after the lines.
        let lrc = "[00:10.00]a\n[offset:-250]";
        assert_eq!(parse_lrc(lrc), vec![(ms(10_250), "a".to_owned())]);
    }

    #[test]
    fn metadata_tags_empty_lines_and_crlf() {
        let lrc = "\u{feff}[ar:Artist]\r\n[ti:Title]\r\n[length: 03:20]\r\n[00:01.00]first\r\n[00:02.00]\r\n\r\njust text\r\n[00:03.00]  padded  \r\n";
        assert_eq!(
            parse_lrc(lrc),
            vec![
                (ms(1_000), "first".to_owned()),
                (ms(2_000), String::new()),
                (ms(3_000), "padded".to_owned()),
            ]
        );
    }

    #[test]
    fn enhanced_word_tags_are_dropped_and_brackets_in_text_survive() {
        let lrc = "[00:05.00]<00:05.00>Hel<00:05.40>lo <00:06.00>world\n[00:08.00][Chorus] la la\n[00:09.00]a < b";
        let lines = parse_lrc(lrc);
        assert_eq!(lines[0].1, "Hello world");
        assert_eq!(lines[1].1, "[Chorus] la la");
        assert_eq!(lines[2].1, "a < b");
    }

    #[test]
    fn plain_text_and_garbage_give_nothing() {
        assert!(parse_lrc("").is_empty());
        assert!(parse_lrc("no timestamps\nat all").is_empty());
        assert!(parse_lrc("[xx:yy]nope\n[1:2:3:4]nope").is_empty());
        // Absurd numbers are dropped, not wrapped around or panicking.
        assert!(
            parse_lrc("[18446744073709551615:18446744073709551615]x\n[00:18446744073709551615]y")
                .is_empty()
        );
    }

    #[test]
    fn lines_with_word_tags_get_word_timing() {
        let lrc = "[00:05.00]<00:05.00>Hel<00:05.40>lo <00:06.00>world<00:07.10>\n[00:08.00]plain line\n[00:10.00]<00:10.00>last <00:10.50>one";
        let lines = parse_lrc_lines(lrc);
        assert_eq!(lines.len(), 3);
        let w = &lines[0].words;
        assert_eq!(lines[0].text, "Hello world");
        assert_eq!(
            w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
            ["Hel", "lo ", "world"]
        );
        assert_eq!((w[0].start, w[0].end), (ms(5_000), ms(5_400)));
        assert_eq!((w[1].start, w[1].end), (ms(5_400), ms(6_000)));
        assert_eq!((w[2].start, w[2].end), (ms(6_000), ms(7_100)));
        assert_eq!(lines[0].end, Some(ms(7_100)));
        assert!(lines[1].words.is_empty());
        assert_eq!(lines[1].end, None);
        assert_eq!(lines[1].text, "plain line");
        // Last line: no trailing tag, 1 s after its start.
        assert_eq!(lines[2].words[1].end, ms(11_000));
        assert_eq!(lines[2].text, "last one");
    }

    #[test]
    fn last_word_without_trailing_tag_ends_at_next_line() {
        let lrc = "[00:01.00]<00:01.00>a <00:01.50>b\n[00:03.00]<00:03.00>c";
        let lines = parse_lrc_lines(lrc);
        assert_eq!(lines[0].words[1].end, ms(3_000));
        assert_eq!(lines[0].words[0].text, "a ");
        assert_eq!(lines[0].words[1].text, "b");
    }

    #[test]
    fn word_times_follow_offset_and_bare_spaces_stick_to_the_previous_word() {
        let lrc = "[offset:+500]\n[00:05.00]<00:05.00>one<00:05.50> <00:05.60>two<00:06.00>";
        let line = &parse_lrc_lines(lrc)[0];
        assert_eq!(line.start, ms(4_500));
        assert_eq!(line.words[0].text, "one ");
        assert_eq!(line.words[0].start, ms(4_500));
        assert_eq!(line.words[0].end, ms(5_000));
        assert_eq!(line.words[1].text, "two");
        let joined: String = line.words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(joined, line.text);
    }

    #[test]
    fn parse_lrc_lines_matches_parse_lrc_on_plain_lrc() {
        let lrc = "[ar:x]\n[00:30.00][00:10.00]chorus\n[00:20.00]\n[00:25.00]a < b";
        let plain = parse_lrc(lrc);
        let lines = parse_lrc_lines(lrc);
        assert_eq!(plain.len(), lines.len());
        for ((t, text), l) in plain.iter().zip(&lines) {
            assert_eq!((*t, text.as_str()), (l.start, l.text.as_str()));
            assert!(l.words.is_empty() && l.end.is_none());
        }
    }
}
