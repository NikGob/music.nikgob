//! Helpers shared by the NetEase and QQ Music clients: matching a search hit
//! against the wanted track, dropping credit lines, attaching translations.

use std::time::Duration;

use nikgob_core::lyrics::LyricLine;

use crate::lrc::parse_lrc_lines;
use crate::text::{artist_key, search_artist, search_title, similarity, title_key};

const DURATION_WINDOW_SECS: f64 = 3.0;
/// With a duration this close the artist spelling no longer has to match
/// (romanized vs. kanji names).
const TIGHT_DURATION_SECS: f64 = 1.5;
const MIN_TITLE_SIMILARITY: f64 = 0.85;
const MIN_ARTIST_SIMILARITY: f64 = 0.7;
/// Translation lines are matched to the original by start time within this window.
const TRANSLATION_WINDOW_MS: i128 = 300;

/// One search result of a service.
#[derive(Debug, Clone)]
pub(crate) struct Hit<T> {
    pub payload: T,
    pub title: String,
    pub artists: Vec<String>,
    pub duration: Option<Duration>,
}

/// The search text for a track: `"title artist"` or just the title.
pub(crate) fn query(artist: &str, title: &str, with_artist: bool) -> String {
    let t = search_title(title);
    let a = search_artist(artist);
    if with_artist && !a.is_empty() {
        format!("{t} {a}")
    } else {
        t
    }
}

/// Splits a "A/B、C" credit into single artists (the whole string stays first).
pub(crate) fn split_artists(raw: &str) -> Vec<String> {
    let mut out = vec![raw.trim().to_owned()];
    out.extend(
        raw.split(['/', '、', '&', ',', ';'])
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != raw.trim())
            .map(str::to_owned),
    );
    out
}

/// Best hit that is the same song. A known hit duration must be within 3 s (and
/// the artist must look alike, unless it is within 1.5 s); without one the
/// artist has to match. Closest duration wins, then the better title.
pub(crate) fn pick<T>(
    hits: Vec<Hit<T>>,
    artist: &str,
    title: &str,
    duration: Option<Duration>,
) -> Option<Hit<T>> {
    let want_title = title_key(title).base;
    let want_artist = artist_key(artist);
    let want = duration.map(|d| d.as_secs_f64());
    let mut best: Option<(f64, f64, Hit<T>)> = None;
    for hit in hits {
        let got_title = title_key(&hit.title).base;
        if want_title.is_empty() || got_title.is_empty() {
            continue;
        }
        let title_sim = similarity(&want_title, &got_title);
        if title_sim < MIN_TITLE_SIMILARITY {
            continue;
        }
        let artist_ok = want_artist.is_empty()
            || hit.artists.iter().any(|a| {
                let k = artist_key(a);
                !k.is_empty() && similarity(&want_artist, &k) >= MIN_ARTIST_SIMILARITY
            });
        let diff = match (want, hit.duration) {
            (Some(w), Some(d)) => {
                let diff = (w - d.as_secs_f64()).abs();
                if diff > DURATION_WINDOW_SECS || !(artist_ok || diff <= TIGHT_DURATION_SECS) {
                    continue;
                }
                diff
            }
            _ => {
                if !artist_ok {
                    continue;
                }
                0.0
            }
        };
        let better = best
            .as_ref()
            .is_none_or(|(d, t, _)| diff < *d || (diff == *d && title_sim > *t));
        if better {
            best = Some((diff, title_sim, hit));
        }
    }
    best.map(|(_, _, h)| h)
}

/// `作词 : X`, `词：X`, `Composer: X` ... : the credits both services put in front of the lyrics.
fn is_credit(text: &str) -> bool {
    const KEYS: &[&str] = &[
        "作词",
        "作詞",
        "作曲",
        "编曲",
        "編曲",
        "制作人",
        "製作人",
        "词",
        "曲",
        "lyricist",
        "composer",
        "arranger",
        "lyrics",
        "music",
        "producer",
    ];
    let t = text.trim();
    let Some(pos) = t.find([':', '：']) else {
        return false;
    };
    let key = t[..pos].trim().to_lowercase();
    KEYS.contains(&key.as_str())
}

/// Drops the leading credit lines, and the `title - artist` header line that
/// sits in front of (or between) blocks of them (NetEase and QQ both do this).
pub(crate) fn strip_credits(mut lines: Vec<LyricLine>) -> Vec<LyricLine> {
    let header = |i: usize| {
        lines.get(i).is_some_and(|l| l.text.contains(" - "))
            && lines.get(i + 1).is_some_and(|n| is_credit(&n.text))
    };
    let mut skip = 0;
    while lines.get(skip).is_some_and(|l| is_credit(&l.text)) || header(skip) {
        skip += 1;
    }
    lines.drain(..skip);
    lines
}

/// Fills `translation` (or `romanization`) of each line from the LRC `extra` by start time.
pub(crate) fn attach(lines: &mut [LyricLine], extra: &str, romanization: bool) {
    let extra: Vec<LyricLine> = parse_lrc_lines(extra)
        .into_iter()
        .filter(|l| !l.text.trim().is_empty() && l.text.trim() != "//")
        .collect();
    if extra.is_empty() {
        return;
    }
    for line in lines.iter_mut() {
        let start = line.start.as_millis() as i128;
        let nearest = extra
            .iter()
            .map(|e| ((e.start.as_millis() as i128 - start).abs(), e))
            .filter(|(d, _)| *d <= TRANSLATION_WINDOW_MS)
            .min_by_key(|(d, _)| *d);
        if let Some((_, e)) = nearest {
            let text = Some(e.text.trim().to_owned());
            if romanization {
                line.romanization = text;
            } else {
                line.translation = text;
            }
        }
    }
}

#[cfg(test)]
mod credit_tests {
    use super::*;

    #[test]
    fn header_between_credit_blocks_goes_too() {
        let lrc = "[00:00.000] 作词 : 岡本優星
[00:00.476] 作曲 : 岡本優星
[00:00.953]High Ace!!! - ammo
[00:02.690]作词：岡本優星
[00:05.534]编曲：ammo
[00:18.801]常勝 気流になって
[00:20.000]a - b";
        let lines = strip_credits(parse_lrc_lines(lrc));
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "常勝 気流になって");
    }
}
