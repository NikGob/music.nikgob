//! The song title inside a music video's title.
//!
//! A clip's title carries what clips add: «Official Video», «MV», «Music
//! Video», «(Lyrics)», «【MV】», a leading «Artist - », a trailing «/ Artist».
//! [`song_titles`] gives the titles its song may have (the YouTube Music
//! counterpart search, docs/stage3.md §7 «Клипы в лайках»); [`display_title`]
//! is the one a clip without its song is shown with («AIYOU - Eve Music
//! Video» → «AIYOU»).
//!
//! * Brackets of noise go («(Official Music Video)», «【MV】», «[4K]»), and
//!   so does a bracket that is only the artist («【Eve】»); a bracket with a
//!   version («(Live)», «(Acoustic Version)») stays part of the title; any
//!   other bracket is a half of the title: the quoted title of «Eve「Ghost
//!   Avenue」MV», the translation of «ファイトソング (Fight Song)».
//! * The rest is split at « - », « – », « — », « | », « / »; a part that is
//!   only noise or the artist goes («Title - Eve Music Video», «Title /
//!   Eve»).
//! * Noise at the end of a part goes when it says it is a clip («Title MV»,
//!   «Title Official Video», «Title Lyric Video», «Title Visualizer»), with
//!   the artist's name right before it («Title Eve Music Video»).
//!
//! A track title may be dual the same way («夏子 - Natsuko»): one song with
//! or without its romanization or translation is still the song
//! ([`same_dual_title`], docs/stage3.md §7 «Одна песня — один TrackId»).

/// Words a bracket or a tail may consist of and still be what clips add.
const CLIP_NOISE: &[&str] = &[
    "official", "video", "music", "mv", "m/v", "pv", "clip", "lyric", "lyrics", "audio", "visualizer",
    "visualiser", "hd", "hq", "4k", "full", "ver", "version", "short", "teaser", "premiere", "remastered",
    "remaster", "клип", "официальный", "official video", "anime", "animation", "the", "movie", "with",
    "subtitles", "english", "sub", "subs", "romaji", "eng", "jp", "kr", "mv版", "公式", "オフィシャル",
    "ミュージックビデオ", "歌詞", "歌詞付き", "リリックビデオ", "映像", "4k60", "60fps", "uhd",
];

/// Words that make another version of a song (they stay in the title).
const VERSION_WORDS: &[&str] = &[
    "remix", "mix", "live", "acoustic", "instrumental", "inst", "karaoke", "cover", "edit", "tv",
    "size", "off", "vocal", "unplugged", "demo", "piano", "orchestra", "slowed", "sped", "nightcore",
    "reprise", "extended", "ライブ", "カバー", "弾き語り",
];

/// Words a clip's tail may consist of («Title Official Music Video»).
const TAIL_NOISE: &[&str] = &[
    "official", "music", "video", "mv", "m/v", "pv", "lyric", "lyrics", "visualizer", "visualiser",
    "audio", "clip", "hd", "4k", "клип", "официальный", "ミュージックビデオ", "リリックビデオ",
];

/// A tail of [`TAIL_NOISE`] goes only with one of these in it (a song may
/// well end in «Video» or «Audio»).
const TAIL_MARKS: &[&str] = &[
    "official", "mv", "m/v", "pv", "visualizer", "visualiser", "клип", "официальный", "ミュージックビデオ",
    "リリックビデオ",
];

/// Lower case, full-width forms folded, `ё` → `е`, only letters and digits.
pub fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            _ => c,
        })
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '/'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Only what clips add (every word noise), or the artist's own name.
fn is_noise(body: &str, artist: &str) -> bool {
    let a = fold(artist);
    let ws = words(body);
    !ws.is_empty()
        && ws.iter().any(|w| CLIP_NOISE.contains(&w.as_str()) || fold(w) == a)
        && ws.iter().all(|w| {
            CLIP_NOISE.contains(&w.as_str())
                || fold(w) == a
                || a.contains(&fold(w)) && !fold(w).is_empty()
                || w.chars().all(|c| c.is_ascii_digit())
        })
        && !ws.iter().any(|w| VERSION_WORDS.contains(&w.as_str()))
}

/// «feat. X», «ft. X», «with X»: a guest, not a title.
fn is_feat(body: &str) -> bool {
    let b = body.trim_start().to_lowercase();
    ["feat.", "feat ", "ft.", "ft ", "featuring ", "with "].iter().any(|p| b.starts_with(p))
}

fn is_version(body: &str) -> bool {
    words(body).iter().any(|w| VERSION_WORDS.contains(&w.as_str()))
}

const OPEN: &[char] = &['(', '[', '【', '「', '『', '〔', '（', '［', '〈', '《'];

fn closing(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '【' => '】',
        '「' => '」',
        '『' => '』',
        '〔' => '〕',
        '（' => '）',
        '［' => '］',
        '〈' => '〉',
        _ => '》',
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A token of a tail as compared: lower case, without the punctuation
/// around it («Video!» → «video»).
fn tail_word(w: &str) -> String {
    w.trim_matches(|c: char| !(c.is_alphanumeric() || c == '/')).to_lowercase()
}

/// `part` without a clip tail («AIYOU Eve Music Video» → «AIYOU»); at least
/// one word stays.
fn strip_tail(part: &str, artist: &str) -> String {
    let tokens: Vec<&str> = part.split_whitespace().collect();
    let mut keep = tokens.len();
    while keep > 1 && TAIL_NOISE.contains(&tail_word(tokens[keep - 1]).as_str()) {
        keep -= 1;
    }
    let tail: Vec<String> = tokens[keep..].iter().map(|w| tail_word(w)).collect();
    let pair = tail.windows(2).any(|p| matches!(p[0].as_str(), "music" | "lyric") && p[1] == "video");
    if keep == tokens.len() || !(pair || tail.iter().any(|w| TAIL_MARKS.contains(&w.as_str()))) {
        return part.to_owned();
    }
    // The artist's name right before the noise («… Eve Music Video»).
    let a = fold(artist);
    if !a.is_empty() {
        for n in (1..keep).rev() {
            if fold(&tokens[n..keep].join(" ")) == a {
                keep = n;
                break;
            }
        }
    }
    tokens[..keep].join(" ")
}

/// The halves of a clip's title (outside the brackets, in order) and the
/// titles found inside brackets; both empty for a title that is only noise.
fn parts(title: &str, artist: &str) -> (Vec<String>, Vec<String>) {
    let mut outside = String::new();
    let mut inner: Vec<String> = Vec::new();
    let mut rest = title;
    while let Some(open) = rest.find(OPEN) {
        let ch = rest[open..].chars().next().unwrap_or('(');
        outside.push_str(&rest[..open]);
        let after = &rest[open + ch.len_utf8()..];
        let close = closing(ch);
        let (body, next) = match after.find(close) {
            Some(c) => (&after[..c], &after[c + close.len_utf8()..]),
            None => (after, ""),
        };
        if is_feat(body) {
            outside.push(' ');
        } else if is_version(body) && !is_noise(body, artist) {
            outside.push_str(&format!(" ({body}) "));
        } else if !is_noise(body, artist) && !body.trim().is_empty() {
            inner.push(squash(body));
            outside.push(' ');
        } else {
            outside.push(' ');
        }
        rest = next;
    }
    outside.push_str(rest);
    let mut halves: Vec<String> = vec![squash(&outside)];
    for sep in [" | ", " / ", " - ", " – ", " — ", " ‐ "] {
        halves = halves
            .into_iter()
            .flat_map(|p| p.split(sep).map(squash).collect::<Vec<_>>())
            .collect();
    }
    let a = fold(artist);
    let mut halves: Vec<String> = halves
        .into_iter()
        .filter(|p| !p.is_empty() && !is_noise(p, artist) && fold(p) != a)
        .map(|p| strip_tail(&p, artist))
        .filter(|p| !p.is_empty() && !is_noise(p, artist) && fold(p) != a)
        .collect();
    // «Artist「Title」», «Eve『Title』MV»: the bracket was the title.
    if halves.iter().all(|h| fold(h).is_empty() || fold(h) == a) {
        halves.clear();
    }
    (halves, inner)
}

/// The titles a clip's song may have: the title without what clips add,
/// and for a dual title each half (see the module docs). Never empty for
/// a title with letters; the whole cleaned title comes first.
pub fn song_titles(title: &str, artist: &str) -> Vec<String> {
    let (mut halves, inner) = parts(title, artist);
    halves.extend(inner);
    // A title that was only noise keeps itself.
    if halves.is_empty() {
        let t = squash(title);
        return if fold(&t).is_empty() { Vec::new() } else { vec![t] };
    }
    let mut out = Vec::new();
    if halves.len() > 1 {
        out.push(halves.join(" "));
    }
    for h in halves {
        if !out.contains(&h) {
            out.push(h);
        }
    }
    out
}

/// How a clip without its song is titled: its halves as they read
/// («インソムニア - Insomnia»), a title from a bracket in brackets
/// («ファイトソング (Fight Song)»), nothing clips add. The title itself
/// when nothing is left.
pub fn display_title(title: &str, artist: &str) -> String {
    let (halves, inner) = parts(title, artist);
    let shown = if halves.is_empty() {
        inner.join(" - ")
    } else {
        let mut s = halves.join(" - ");
        for i in &inner {
            s.push_str(&format!(" ({i})"));
        }
        s
    };
    if fold(&shown).is_empty() { squash(title) } else { shown }
}

/// Letters outside the Latin script (CJK, kana, Cyrillic, Hangul…).
fn has_non_latin(s: &str) -> bool {
    s.chars().any(|c| c.is_alphabetic() && !is_latin(c))
}

/// Letters, all of them Latin.
fn all_latin(s: &str) -> bool {
    s.chars().any(char::is_alphabetic) && s.chars().all(|c| !c.is_alphabetic() || is_latin(c))
}

fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{C0}'..='\u{24F}' | '\u{1E00}'..='\u{1EFF}')
}

/// The « - » parts of a title, without empty ones.
fn dash_parts(title: &str) -> Vec<String> {
    let mut parts = vec![squash(title)];
    for sep in [" - ", " – ", " — "] {
        parts = parts.into_iter().flat_map(|p| p.split(sep).map(squash).collect::<Vec<_>>()).collect();
    }
    parts.retain(|p| !p.is_empty());
    parts
}

/// One song under a single and a dual title: the « - » parts of one are the
/// leading or the trailing parts of the other (compared by `key`), and what
/// the other adds is the title in another script — non-Latin letters on one
/// side, only Latin ones on the other («夏子 - Natsuko» and «夏子»,
/// «勘冴えて悔しいわ - Kan Saete Kuyashiiwa» and «Kan Saete Kuyashiiwa»).
/// A version («夏子 - Live») or two Latin halves («Song - Remix», «A - B»)
/// are other titles. Equal titles are for the caller to compare.
pub fn same_dual_title<K: PartialEq>(a: &str, b: &str, key: impl Fn(&str) -> K) -> bool {
    let (pa, pb) = (dash_parts(a), dash_parts(b));
    let (long, short) = if pa.len() > pb.len() { (pa, pb) } else { (pb, pa) };
    if short.is_empty() || long.len() == short.len() {
        return false;
    }
    let short_keys: Vec<K> = short.iter().map(|p| key(p)).collect();
    let n = long.len() - short.len();
    [(&long[..short.len()], &long[short.len()..]), (&long[n..], &long[..n])]
        .into_iter()
        .any(|(kept, dropped)| {
            let same = kept.iter().zip(&short_keys).all(|(p, k)| key(p) == *k);
            let (kept, dropped) = (kept.join(" "), dropped.join(" "));
            same && !is_version(&dropped)
                && (has_non_latin(&kept) && all_latin(&dropped) || all_latin(&kept) && has_non_latin(&dropped))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_titles_lose_what_clips_add() {
        assert_eq!(song_titles("インソムニア - Insomnia", "Eve"), ["インソムニア Insomnia", "インソムニア", "Insomnia"]);
        assert_eq!(song_titles("アヴァン - Avant", "Eve"), ["アヴァン Avant", "アヴァン", "Avant"]);
        assert_eq!(song_titles("夏子 - Natsuko", "Funky Monkey Bλby's"), ["夏子 Natsuko", "夏子", "Natsuko"]);
        assert_eq!(song_titles("Ghost Avenue", "Eve"), ["Ghost Avenue"]);
        assert_eq!(song_titles("Eve - Ghost Avenue (Official Video)", "Eve"), ["Ghost Avenue"]);
        assert_eq!(song_titles("Eve「Ghost Avenue」MV", "Eve"), ["Ghost Avenue"]);
        assert_eq!(song_titles("【MV】ドラマツルギー / Eve", "Eve"), ["ドラマツルギー"]);
        assert_eq!(song_titles("Группа крови (Official Music Video)", "Кино"), ["Группа крови"]);
        assert_eq!(song_titles("Кино - Группа крови (клип)", "Кино"), ["Группа крови"]);
        assert_eq!(song_titles("Song (Lyrics)", "X"), ["Song"]);
        assert_eq!(song_titles("Song [Official Audio] | X", "X"), ["Song"]);
        assert_eq!(
            song_titles("ファイトソング (Fight Song) - Eve Music Video", "Eve"),
            ["ファイトソング Fight Song", "ファイトソング", "Fight Song"]
        );
        // A version stays part of the title.
        assert_eq!(song_titles("Song (Live at Budokan)", "X"), ["Song (Live at Budokan)"]);
        assert_eq!(song_titles("Song (Acoustic Version) - Official Video", "X"), ["Song (Acoustic Version)"]);
        // A title that is only noise keeps itself.
        assert_eq!(song_titles("MV", "X"), ["MV"]);
    }

    #[test]
    fn the_usual_clip_patterns_give_the_song_title() {
        for (title, artist, song) in [
            ("AIYOU - Eve Music Video", "Eve", "AIYOU"),
            ("Kaikai Kitan - Eve Music Video", "Eve", "Kaikai Kitan"),
            ("AIYOU Eve Music Video", "Eve", "AIYOU"),
            ("Eve『AIYOU』MV", "Eve", "AIYOU"),
            ("Eve「AIYOU」MV", "Eve", "AIYOU"),
            ("AIYOU (Official Music Video)", "Eve", "AIYOU"),
            ("AIYOU / Eve", "Eve", "AIYOU"),
            ("【Eve】AIYOU", "Eve", "AIYOU"),
            ("AIYOU MV", "Eve", "AIYOU"),
            ("AIYOU M/V", "Eve", "AIYOU"),
            ("AIYOU Official Video", "Eve", "AIYOU"),
            ("AIYOU - Lyric Video", "Eve", "AIYOU"),
            ("AIYOU (Lyric Video)", "Eve", "AIYOU"),
            ("AIYOU Lyric Video", "Eve", "AIYOU"),
            ("AIYOU [Visualizer]", "Eve", "AIYOU"),
            ("AIYOU - Visualizer", "Eve", "AIYOU"),
            ("AIYOU Visualizer", "Eve", "AIYOU"),
            ("Eve - AIYOU [Official Visualizer]", "Eve", "AIYOU"),
            ("Группа крови Официальный клип", "Кино", "Группа крови"),
        ] {
            assert_eq!(song_titles(title, artist)[0], song, "{title}");
            assert_eq!(display_title(title, artist), song, "{title}");
        }
    }

    #[test]
    fn a_song_that_ends_in_a_noise_word_keeps_it() {
        // No clip marker in the tail: «Video», «Audio» belong to the song.
        assert_eq!(song_titles("Video Killed the Radio Star", "The Buggles"), ["Video Killed the Radio Star"]);
        assert_eq!(song_titles("Home Video", "Lorde"), ["Home Video"]);
        assert_eq!(song_titles("Music", "Madonna"), ["Music"]);
        assert_eq!(display_title("Official", "X"), "Official");
    }

    #[test]
    fn a_clip_is_shown_by_its_song_title() {
        assert_eq!(display_title("インソムニア - Insomnia", "Eve"), "インソムニア - Insomnia");
        assert_eq!(display_title("ファイトソング (Fight Song) - Eve Music Video", "Eve"), "ファイトソング (Fight Song)");
        assert_eq!(display_title("Song (Live at Budokan) [Official Video]", "X"), "Song (Live at Budokan)");
        assert_eq!(display_title("Ghost Avenue", "Eve"), "Ghost Avenue");
        assert_eq!(display_title("MV", "X"), "MV");
    }

    #[test]
    fn a_title_with_its_romanization_is_the_title() {
        let same = |a, b| same_dual_title(a, b, fold);
        assert!(same("夏子 - Natsuko", "夏子"));
        assert!(same("夏子", "夏子 - Natsuko"));
        assert!(same("勘冴えて悔しいわ - Kan Saete Kuyashiiwa", "Kan Saete Kuyashiiwa"));
        assert!(same(
            "けっかおーらい - Kekka Orai - Kekka Orai - My Hero Academia: Vigilantes Opening Theme Song",
            "Kekka Orai - My Hero Academia: Vigilantes Opening Theme Song"
        ));
        assert!(same("Группа крови - Gruppa krovi", "группа крови"));
        assert!(same("Gruppa krovi - Группа крови", "Gruppa Krovi"));
        // Versions and two Latin halves are other titles.
        assert!(!same("Song - Live", "Song"));
        assert!(!same("Song - Remix", "Song"));
        assert!(!same("A - B", "A"));
        assert!(!same("A - B", "B"));
        assert!(!same("夏子 - Live", "夏子"));
        assert!(!same("夏子 - Natsuko (Remix)", "夏子"));
        assert!(!same("夏子 - 冬子", "夏子"));
        // Not the leading or trailing parts, or nothing added.
        assert!(!same("夏子 - Natsuko", "Fuyuko"));
        assert!(!same("夏子 - Natsuko", "夏子 - Natsuko"));
        assert!(!same("Natsuko - 夏子 - Natsuko", "夏子"));
    }

    #[test]
    fn folding_ignores_case_width_and_punctuation() {
        assert_eq!(fold("Ghost  Avenue!"), "ghostavenue");
        assert_eq!(fold("ＩＮＳＯＭＮＩＡ"), "insomnia");
        assert_eq!(fold("Ёлка"), "елка");
        assert_eq!(fold("Funky Monkey Bλby's"), "funkymonkeybλbys");
    }
}
