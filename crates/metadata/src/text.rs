//! Text normalization and fuzzy comparison used by the matcher. Pure functions.
//!
//! Pipeline for titles: lowercase, `ё` -> `е`, drop "noise" (`(Remastered 2011)`,
//! `- Live`, `feat. X`, ...) while remembering variant flags (live/remix/...),
//! strip diacritics, transliterate Cyrillic to Latin (GOST-ish), drop punctuation,
//! fold spelling variants of the same Russian sound, collapse repeated letters.

use strsim::jaro_winkler;

/// Which kind of recording a title claims to be. A mismatch (studio vs live) is
/// penalized by the matcher even though the base titles are equal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Variant(u8);

impl Variant {
    pub const LIVE: Self = Self(1);
    pub const REMIX: Self = Self(2);
    pub const ACOUSTIC: Self = Self(4);
    pub const INSTRUMENTAL: Self = Self(8);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// A title reduced to what identifies the song.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleKey {
    pub base: String,
    pub variant: Variant,
}

/// Words that mark a bracket/suffix as release noise rather than part of the title.
const NOISE_WORDS: &[&str] = &[
    "remaster",
    "remastered",
    "remastering",
    "ремастер",
    "ремастеринг",
    "deluxe",
    "edition",
    "version",
    "versions",
    "edit",
    "bonus",
    "explicit",
    "clean",
    "anniversary",
    "expanded",
    "reissue",
    "radio",
    "версия",
    "издание",
    "бонус",
    "live",
    "лайв",
    "концертная",
    "живая",
    "remix",
    "ремикс",
    "acoustic",
    "акустика",
    "акустическая",
    "instrumental",
    "инструментал",
    "karaoke",
    "караоке",
];

fn word_variant(w: &str) -> Variant {
    match w {
        "live" | "лайв" | "концертная" | "живая" => Variant::LIVE,
        "remix" | "ремикс" => Variant::REMIX,
        "acoustic" | "акустика" | "акустическая" => Variant::ACOUSTIC,
        "instrumental" | "инструментал" | "karaoke" | "караоке" => {
            Variant::INSTRUMENTAL
        }
        _ => Variant::default(),
    }
}

fn words(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
}

fn is_year(w: &str) -> bool {
    w.len() == 4 && w.starts_with(['1', '2']) && w.bytes().all(|b| b.is_ascii_digit())
}

/// A bracket body counts as noise when it mentions a noise word, a lone year, or a feat clause.
fn bracket_is_noise(body: &str) -> bool {
    let trimmed = body.trim();
    if starts_with_feat(trimmed) || trimmed.starts_with("with ") {
        return true;
    }
    if matches!(
        trimmed,
        "mono" | "stereo" | "mono mix" | "stereo mix" | "single" | "album" | "original"
    ) {
        return true;
    }
    let mut count = 0;
    let mut noisy = false;
    for w in words(trimmed) {
        count += 1;
        noisy |= NOISE_WORDS.contains(&w);
    }
    noisy || (count == 1 && words(trimmed).all(is_year))
}

/// A " - xxx" tail counts as noise when it starts with a noise word/year or mentions remaster.
fn suffix_is_noise(tail: &str) -> bool {
    let t = tail.trim();
    if t.is_empty() {
        return false;
    }
    if matches!(t, "mono" | "stereo" | "single" | "album version") {
        return true;
    }
    let mut it = words(t);
    let Some(first) = it.next() else { return false };
    if NOISE_WORDS.contains(&first) || is_year(first) || first == "from" || first == "feat" {
        return true;
    }
    words(t).any(|w| w.starts_with("remaster") || w.starts_with("ремастер"))
}

fn starts_with_feat(s: &str) -> bool {
    [
        "feat.",
        "feat ",
        "ft.",
        "ft ",
        "featuring ",
        "при уч.",
        "при участии ",
    ]
    .iter()
    .any(|p| s.starts_with(p))
}

/// Removes noise brackets (recording their variant), turns other brackets into spaces.
fn strip_brackets(s: &str, variant: &mut Variant) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let close = match chars[i] {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close) = close
            && let Some(len) = chars[i + 1..].iter().position(|c| *c == close)
        {
            let body: String = chars[i + 1..i + 1 + len].iter().collect();
            let lowered = body.to_lowercase();
            if bracket_is_noise(&lowered) {
                for w in words(&lowered) {
                    variant.insert(word_variant(w));
                }
                out.push(' ');
            } else {
                out.push(' ');
                out.push_str(&body);
                out.push(' ');
            }
            i += len + 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Cuts a trailing " - Live" / " - 2011 Remaster" style segment.
fn strip_dash_suffix(s: &str, variant: &mut Variant) -> String {
    for sep in [" - ", " – ", " — "] {
        let mut from = 0;
        while let Some(pos) = s[from..].find(sep) {
            let at = from + pos;
            let tail = s[at + sep.len()..].to_lowercase();
            if suffix_is_noise(&tail) {
                for w in words(&tail) {
                    variant.insert(word_variant(w));
                }
                return s[..at].to_owned();
            }
            from = at + sep.len();
        }
    }
    s.to_owned()
}

/// Cuts a bare `feat. X` / `ft. X` tail (the bracketed form is handled by `strip_brackets`).
fn strip_feat(s: &str) -> String {
    // ASCII lowercasing keeps byte offsets valid.
    let probe = s.to_ascii_lowercase();
    for marker in [
        " feat.",
        " feat ",
        " ft.",
        " ft ",
        " featuring ",
        " при уч.",
        " при участии ",
    ] {
        if let Some(pos) = probe.find(marker) {
            return s[..pos].to_owned();
        }
    }
    s.to_owned()
}

fn lower(s: &str) -> String {
    s.to_lowercase().replace('ё', "е")
}

/// Title -> comparable key plus variant flags.
pub fn title_key(raw: &str) -> TitleKey {
    let lowered = lower(raw);
    let mut variant = Variant::default();
    let no_brackets = strip_brackets(&lowered, &mut variant);
    let no_dash = strip_dash_suffix(&no_brackets, &mut variant);
    let no_feat = strip_feat(&no_dash);
    let mut base = fold_text(&no_feat);
    if base.is_empty() {
        base = fold_text(&lowered);
    }
    TitleKey { base, variant }
}

/// Title with release noise removed but original casing/script kept; meant for
/// building search queries.
pub fn search_title(raw: &str) -> String {
    let mut ignored = Variant::default();
    let no_brackets = strip_brackets(raw, &mut ignored);
    let no_dash = strip_dash_suffix(&no_brackets, &mut ignored);
    let cleaned = strip_feat(&no_dash);
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        raw.trim().to_owned()
    } else {
        collapsed
    }
}

/// Artist without a trailing `feat. X`, original casing kept.
pub fn search_artist(raw: &str) -> String {
    let cleaned = strip_feat(raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        raw.trim().to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Artist -> comparable key (no feat. tail, no leading "the").
pub fn artist_key(raw: &str) -> String {
    let lowered = lower(raw);
    let mut ignored = Variant::default();
    let no_brackets = strip_brackets(&lowered, &mut ignored);
    let no_feat = strip_feat(&no_brackets);
    let folded = fold_text(&no_feat);
    let key = folded.strip_prefix("the ").unwrap_or(&folded).to_owned();
    if key.is_empty() { folded } else { key }
}

/// Album -> comparable key. Same noise rules as titles.
pub fn album_key(raw: &str) -> String {
    title_key(raw).base
}

/// Core normalization on already lowercased text: no brackets handling.
fn fold_text(s: &str) -> String {
    let mut latin = String::with_capacity(s.len() + 8);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Decomposed text (a base letter plus a combining mark, as macOS and
            // some taggers write it) must read like the precomposed letter, not
            // split the word in two.
            '\u{0300}'..='\u{036F}' => {}
            'и' if chars.peek() == Some(&'\u{0306}') => {
                chars.next();
                latin.push('y');
            }
            '\'' | '’' | 'ʼ' | '`' | '´' => {}
            '&' => latin.push_str(" and "),
            c if c.is_alphanumeric() => {
                if let Some(t) = translit_cyrillic(c) {
                    latin.push_str(t);
                } else if let Some(t) = deaccent(c) {
                    latin.push_str(t);
                } else {
                    latin.push(c);
                }
            }
            _ => latin.push(' '),
        }
    }
    latin
        .split_whitespace()
        .map(fold_word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// GOST-ish transliteration of lowercase Cyrillic (`ё` already folded to `е`).
fn translit_cyrillic(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' | 'ґ' => "g",
        'д' => "d",
        'е' | 'э' => "e",
        'ж' => "zh",
        'з' => "z",
        'и' | 'і' => "i",
        'й' => "y",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' | 'ў' => "u",
        'ф' => "f",
        'х' => "kh",
        'ц' => "ts",
        'ч' => "ch",
        'ш' => "sh",
        'щ' => "shch",
        'ъ' | 'ь' => "",
        'ы' => "y",
        'ю' => "yu",
        'я' => "ya",
        'ї' => "yi",
        'є' => "ye",
        _ => return None,
    })
}

/// Latin letters with diacritics -> ASCII.
fn deaccent(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'ç' | 'ć' | 'č' | 'ĉ' => "c",
        'ď' | 'đ' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ė' | 'ę' | 'ě' => "e",
        'ğ' | 'ĝ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ł' | 'ĺ' | 'ľ' => "l",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'ŕ' | 'ř' => "r",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'ť' | 'ţ' | 'ț' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'ß' => "ss",
        'æ' => "ae",
        'œ' => "oe",
        _ => return None,
    })
}

/// Makes common spelling variants of Russian sounds converge, then collapses doubles.
fn fold_word(word: &str) -> String {
    let mut w = word.to_owned();
    // Word-initial "ye" (Yelena / Елена).
    if let Some(rest) = w.strip_prefix("ye") {
        w = format!("e{rest}");
    }
    for (from, to) in [
        ("shch", "sh"),
        ("sch", "sh"),
        ("kh", "h"),
        ("ts", "c"),
        ("tz", "c"),
        ("ck", "k"),
        ("qu", "kv"),
        ("q", "k"),
        ("x", "ks"),
        ("w", "v"),
        ("ju", "yu"),
        ("ja", "ya"),
    ] {
        if w.contains(from) {
            w = w.replace(from, to);
        }
    }
    // Word-final -iy / -ii / -yy / -ij (Дмитрий -> Dmitry).
    for tail in ["iy", "ii", "yy", "ij", "yi"] {
        if w.len() > 2 && w.ends_with(tail) {
            w.truncate(w.len() - tail.len());
            w.push('y');
            break;
        }
    }
    let mut out = String::with_capacity(w.len());
    let mut prev = None;
    for c in w.chars() {
        if Some(c) != prev {
            out.push(c);
        }
        prev = Some(c);
    }
    out
}

/// Similarity 0..=1 of two already normalized keys. Equal strings give 1.0;
/// unrelated strings give about 0. Word order is ignored.
pub fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let direct = jaro_winkler(a, b);
    let sorted = jaro_winkler(&sort_words(a), &sort_words(b));
    let jw = direct.max(sorted);
    ((jw - 0.55) / 0.45).clamp(0.0, 1.0)
}

fn sort_words(s: &str) -> String {
    let mut v: Vec<&str> = s.split(' ').collect();
    v.sort_unstable();
    v.join(" ")
}

/// Track number, artist and title guessed from a file name such as
/// `01 - Кино - Группа крови.flac` or `Artist - Title.mp3`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFilename {
    pub track_no: Option<u32>,
    pub artist: Option<String>,
    pub title: Option<String>,
}

pub fn parse_filename(name: &str) -> ParsedFilename {
    // Drop directories and extension.
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = match file.rsplit_once('.') {
        Some((stem, ext))
            if (1..=5).contains(&ext.len()) && ext.chars().all(char::is_alphanumeric) =>
        {
            stem
        }
        _ => file,
    };
    let mut stem = stem.replace('_', " ");
    let mut track_no = None;

    // Leading "01 ", "01.", "01 - ", "1-05 " (disc-track).
    let digits_end = stem
        .char_indices()
        .find(|(_, c)| !c.is_ascii_digit())
        .map_or(stem.len(), |(i, _)| i);
    if (1..=3).contains(&digits_end) {
        let after = &stem[digits_end..];
        let trimmed = after.trim_start_matches(['.', ')', '-', ' ']);
        if trimmed.len() < after.len() && !trimmed.is_empty() {
            track_no = stem[..digits_end].parse().ok();
            stem = trimmed.to_owned();
        }
    }

    let parts: Vec<&str> = stem
        .split(" - ")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    let (artist, title) = match parts.as_slice() {
        [] => (None, None),
        [title] => (None, Some((*title).to_owned())),
        [artist, rest @ ..] => (Some((*artist).to_owned()), Some(rest.join(" - "))),
    };
    ParsedFilename {
        track_no,
        artist,
        title,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_noise_and_flags_variants() {
        assert_eq!(title_key("Song (Remastered 2011)").base, "song");
        assert_eq!(title_key("Song - 2011 Remaster").base, "song");
        assert_eq!(title_key("Song - Remastered 2011").base, "song");
        assert_eq!(title_key("Song [Deluxe Edition]").base, "song");
        assert_eq!(title_key("Song (feat. Someone)").base, "song");
        assert_eq!(title_key("Song feat. Someone").base, "song");
        let live = title_key("Song - Live");
        assert_eq!(live.base, "song");
        assert!(live.variant.contains(Variant::LIVE));
        let remix = title_key("Song (DJ X Remix)");
        assert_eq!(remix.base, "song");
        assert!(remix.variant.contains(Variant::REMIX));
        // Meaningful brackets survive.
        assert_eq!(title_key("Song (Part 2)").base, "song part 2");
        assert!(title_key("Plain").variant.is_empty());
    }

    #[test]
    fn cyrillic_and_latin_converge() {
        assert_eq!(artist_key("Кино"), artist_key("Kino"));
        assert_eq!(artist_key("Земфира"), artist_key("Zemfira"));
        assert_eq!(artist_key("Аквариум"), artist_key("Aquarium"));
        assert_eq!(
            artist_key("Наутилус Помпилиус"),
            artist_key("Nautilus Pompilius")
        );
        assert_eq!(artist_key("Мумий Тролль"), artist_key("Mumiy Troll"));
        assert_eq!(title_key("Ёлка").base, title_key("Елка").base);
        assert_eq!(artist_key("The Beatles"), artist_key("Beatles"));
        assert_eq!(artist_key("Björk"), artist_key("Bjork"));
    }

    #[test]
    fn decomposed_unicode_reads_like_precomposed() {
        // "й" and "ё" written as base letter + combining mark (NFD).
        assert_eq!(artist_key("Дмитри\u{0438}\u{0306}"), artist_key("Дмитрий"));
        assert_eq!(title_key("Е\u{0308}лка").base, title_key("Ёлка").base);
        assert_eq!(artist_key("Bjo\u{0308}rk"), artist_key("Björk"));
    }

    #[test]
    fn similarity_behaves() {
        assert!((similarity("kino", "kino") - 1.0).abs() < f64::EPSILON);
        assert!(similarity("abbey road", "abey road") > 0.9);
        assert!(similarity("kino", "metallica") < 0.3);
        assert!(similarity("blue red", "red blue") > 0.95);
    }

    #[test]
    fn filenames() {
        let p = parse_filename("C:\\Inbox\\01 - Кино - Группа крови.flac");
        assert_eq!(p.track_no, Some(1));
        assert_eq!(p.artist.as_deref(), Some("Кино"));
        assert_eq!(p.title.as_deref(), Some("Группа крови"));
        let p = parse_filename("Artist - Title.mp3");
        assert_eq!(p.track_no, None);
        assert_eq!(p.artist.as_deref(), Some("Artist"));
        let p = parse_filename("07. Just A Title.opus");
        assert_eq!(p.track_no, Some(7));
        assert_eq!(p.artist, None);
        assert_eq!(p.title.as_deref(), Some("Just A Title"));
        // A year-like number is not a track number.
        assert_eq!(parse_filename("1999.mp3").track_no, None);
    }
}
