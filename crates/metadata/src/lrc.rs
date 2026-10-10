//! LRC (synchronized lyrics) parser.

use std::time::Duration;

/// Parses LRC text into `(timestamp, line)` pairs sorted by time.
///
/// Handles several timestamps per line (`[00:12.00][01:30.00]chorus`), `[offset:+500]`
/// (milliseconds; positive means the text appears earlier), 1-3 digit fractions
/// (`.5`, `.50`, `.500`), a `:` before the fraction (`[01:02:30]`), minutes over 59,
/// enhanced word tags (`<00:12.30>` are dropped) and ignores `[ar:]`-style metadata.
/// Empty text is kept: it marks an instrumental break.
pub fn parse_lrc(text: &str) -> Vec<(Duration, String)> {
    let mut out: Vec<(u64, String)> = Vec::new();
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

        if stamps.is_empty() {
            continue;
        }
        let line = strip_word_tags(rest);
        for ms in stamps {
            out.push((ms, line.clone()));
        }
    }

    out.sort_by_key(|(ms, _)| *ms);
    out.into_iter()
        .map(|(ms, line)| {
            let shifted = if offset_ms >= 0 {
                ms.saturating_sub(offset_ms.unsigned_abs())
            } else {
                ms.saturating_add(offset_ms.unsigned_abs())
            };
            (Duration::from_millis(shifted), line)
        })
        .collect()
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
}
