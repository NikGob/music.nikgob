//! One artist under several spellings: what the library compares artist
//! names by (docs/stage4.md §12.13).
//!
//! * [`name_key`]: the same name in another width or case, with other
//!   punctuation (`リコ（CV：富田美憂）` and `リコ(CV:富田美憂)`, `HITORIE` and
//!   `Hitorie`);
//! * [`latin_key`]: how a name sounds in Latin letters: its kana read in
//!   Hepburn romaji (`ヒトリエ` → `hitorie`), its Cyrillic transliterated
//!   (`Татьяна Буланова` → `tatiana bulanova`), Latin as written, with long
//!   vowels and the spellings romanizations disagree on folded
//!   (`Oohara` = `Ohara`, `Tatyana` = `Tatiana`); `None` for kanji, which
//!   have no reading without a dictionary;
//! * [`swapped`]: a two-word Latin name in the other order (`Fujisawa
//!   Yoshiaki` and `Yoshiaki Fujisawa`).

/// Fullwidth ASCII and the ideographic space as ASCII, halfwidth katakana
/// (with their sound marks) as the usual ones.
pub fn width_fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{FF01}'..='\u{FF5E}' => out.push(char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)),
            '\u{3000}' => out.push(' '),
            '\u{FF65}' => out.push('・'),
            '\u{FF70}' => out.push('ー'),
            '\u{FF9E}' | '\u{FF9F}' => {
                // A sound mark after a halfwidth letter: the voiced letter.
                let mark = if c == '\u{FF9E}' { 1 } else { 2 };
                match out.pop() {
                    Some(prev) if voiced(prev, mark).is_some() => out.push(voiced(prev, mark).unwrap_or(prev)),
                    Some(prev) => {
                        out.push(prev);
                        out.push(if mark == 1 { '゛' } else { '゜' });
                    }
                    None => {}
                }
            }
            '\u{FF66}'..='\u{FF9D}' => out.push(HALFWIDTH[(c as u32 - 0xFF66) as usize]),
            c => out.push(c),
        }
    }
    out
}

/// Halfwidth katakana U+FF66..=U+FF9D as the usual katakana.
const HALFWIDTH: [char; 56] = [
    'ヲ', 'ァ', 'ィ', 'ゥ', 'ェ', 'ォ', 'ャ', 'ュ', 'ョ', 'ッ', 'ー', 'ア', 'イ', 'ウ', 'エ', 'オ', 'カ', 'キ', 'ク', 'ケ',
    'コ', 'サ', 'シ', 'ス', 'セ', 'ソ', 'タ', 'チ', 'ツ', 'テ', 'ト', 'ナ', 'ニ', 'ヌ', 'ネ', 'ノ', 'ハ', 'ヒ', 'フ', 'ヘ',
    'ホ', 'マ', 'ミ', 'ム', 'メ', 'モ', 'ヤ', 'ユ', 'ヨ', 'ラ', 'リ', 'ル', 'レ', 'ロ', 'ワ', 'ン',
];

/// The kana `c` with a sound mark (`mark` 1: ゛, 2: ゜).
fn voiced(c: char, mark: u32) -> Option<char> {
    match (c, mark) {
        ('ウ', 1) => return Some('ヴ'),
        ('う', 1) => return Some('ゔ'),
        _ => {}
    }
    let marked = char::from_u32(c as u32 + mark)?;
    (unvoiced(marked) == c).then_some(marked)
}

/// The kana `c` without its sound mark (`が` → `か`, `パ` → `ハ`), else `c`.
pub fn unvoiced(c: char) -> char {
    let base = c as u32;
    let (k, katakana) = if (0x30A0..=0x30FF).contains(&base) { (base - 0x60, true) } else { (base, false) };
    let plain = match k {
        // が..ぢ: the voiced letter follows its plain one.
        0x304C..=0x3062 if (k - 0x304B) % 2 == 1 => k - 1,
        0x3065 | 0x3067 | 0x3069 => k - 1,
        // ば, ぱ: one and two after は.
        0x306F..=0x307D => k - (k - 0x306F) % 3,
        0x3094 => 0x3046,
        _ => return c,
    };
    char::from_u32(if katakana { plain + 0x60 } else { plain }).unwrap_or(c)
}

/// The disc a folder's name says: `CD2`, `Disc 2`, `disk 3`, `Диск 2`,
/// `Album [CD2]` → 2.
pub fn disc_of_folder(name: &str) -> Option<u32> {
    let lower = width_fold(name).to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for (i, w) in words.iter().enumerate() {
        for p in ["cd", "disc", "disk", "диск"] {
            let Some(rest) = w.strip_prefix(p) else { continue };
            let digits = if rest.is_empty() { words.get(i + 1).copied().unwrap_or_default() } else { rest };
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return digits.parse().ok().filter(|n| *n > 0);
            }
        }
    }
    None
}

/// A title without its sound marks, widths folded: `パウロ` and `バウロ`
/// are one (a typo of one mark).
pub fn voiceless(s: &str) -> String {
    width_fold(s).chars().filter(|c| !matches!(c, '\u{3099}' | '\u{309A}')).map(unvoiced).collect()
}

/// Lower case, `ё` → `е`, one width ([`width_fold`]), letters and digits
/// only: one name however it is cased, spaced and bracketed.
pub fn name_key(name: &str) -> String {
    width_fold(name)
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c == 'ё' { 'е' } else { c })
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// The artist line of a compilation, which names nobody: «Various
/// Artists», «Разные исполнители», «VA», «Сборник» and their spellings in
/// other languages.
pub fn is_various_artists(name: &str) -> bool {
    const VARIOUS: &[&str] = &[
        "variousartists",
        "variousartist",
        "various",
        "va",
        "разныеисполнители",
        "разныеартисты",
        "различныеисполнители",
        "сборник",
        "verschiedeneinterpreten",
        "verschiedenekünstler",
        "variosartistas",
        "variosinterpretes",
        "variosintérpretes",
        "váriosartistas",
        "artistesdivers",
        "artistivari",
        "różniwykonawcy",
        "різнівиконавці",
    ];
    VARIOUS.contains(&name_key(name).as_str())
}

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3041}'..='\u{3096}' | '\u{30A1}'..='\u{30FA}' | 'ー' | '・')
}

fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04FF}')
}

/// Has letters, all of them Latin (accents allowed).
pub fn is_latin(name: &str) -> bool {
    let s = width_fold(name);
    s.chars().any(char::is_alphabetic) && s.chars().filter(|c| c.is_alphabetic()).all(|c| (c as u32) < 0x250)
}

/// Written in a script of its own (kana, kanji, Cyrillic…), not in Latin
/// letters: the spelling to show of one artist's names.
pub fn is_native(name: &str) -> bool {
    width_fold(name).chars().any(|c| c.is_alphabetic() && (c as u32) >= 0x250)
}

/// Written in a script a reader of the Russian interface may not read:
/// letters that are neither Latin (accents and modifier letters included)
/// nor Cyrillic: kana, kanji, hangul, Greek, Arabic… Such an artist gets a
/// Latin name shown beside its own.
pub fn foreign_script(name: &str) -> bool {
    width_fold(name).chars().any(|c| {
        let k = c as u32;
        c.is_alphabetic() && k >= 0x250 && !(0x2B0..=0x36F).contains(&k) && !(0x400..=0x52F).contains(&k) && !(0x1E00..=0x1EFF).contains(&k)
    })
}

/// A name written in kana only as a Latin name to show: its romaji with
/// each word capitalised (`リコ` → `Riko`, `ナナホシ` → `Nanahoshi`). `None`
/// for anything else: kanji have no reading without a dictionary.
pub fn romaji_name(name: &str) -> Option<String> {
    let romaji = kana_romaji(name)?;
    let words: Vec<String> = romaji
        .split(' ')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// Hepburn romaji of one kana (katakana folded to hiragana).
fn romaji(c: char) -> Option<&'static str> {
    let k = c as u32;
    let k = if (0x30A1..=0x30F6).contains(&k) { k - 0x60 } else { k };
    const ROW: [&str; 84] = [
        "a", "a", "i", "i", "u", "u", "e", "e", "o", "o", // ぁ..お
        "ka", "ga", "ki", "gi", "ku", "gu", "ke", "ge", "ko", "go", // か..ご
        "sa", "za", "shi", "ji", "su", "zu", "se", "ze", "so", "zo", // さ..ぞ
        "ta", "da", "chi", "ji", "", "tsu", "zu", "te", "de", "to", "do", // た..ど (っ)
        "na", "ni", "nu", "ne", "no", // な..の
        "ha", "ba", "pa", "hi", "bi", "pi", "fu", "bu", "pu", "he", "be", "pe", "ho", "bo", "po", // は..ぽ
        "ma", "mi", "mu", "me", "mo", // ま..も
        "ya", "ya", "yu", "yu", "yo", "yo", // ゃ..よ
        "ra", "ri", "ru", "re", "ro", // ら..ろ
        "wa", "wa", "i", "e", "o", "n", "vu", // ゎ..ゔ
    ];
    match k {
        0x3041..=0x3094 => ROW.get((k - 0x3041) as usize).copied(),
        _ => None,
    }
}

fn small(c: char) -> bool {
    matches!(c, 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'ゃ' | 'ゅ' | 'ょ' | 'ゎ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ャ' | 'ュ' | 'ョ' | 'ヮ')
}

/// A name written in kana only, in Hepburn romaji (`ヒトリエ` → `hitorie`,
/// `ナナホシ` → `nanahoshi`); `None` when it has anything else but spaces
/// and `・`.
pub fn kana_romaji(name: &str) -> Option<String> {
    let s = width_fold(name);
    let s = s.trim();
    if s.is_empty() || !s.chars().all(|c| is_kana(c) || c == ' ') || !s.chars().any(|c| is_kana(c) && c != 'ー' && c != '・') {
        return None;
    }
    let mut out = String::new();
    let mut double = false;
    for c in s.chars() {
        match c {
            ' ' | '・' => out.push(' '),
            'ー' => {}
            'っ' | 'ッ' => double = true,
            c if small(c) => {
                let r = romaji(c)?;
                // きゃ → kya, しゃ → sha, ふぁ → fa, てぃ → ti.
                if r.starts_with('y') && out.ends_with('i') {
                    out.pop();
                    if !(out.ends_with("sh") || out.ends_with("ch") || out.ends_with('j')) {
                        out.push('y');
                    }
                    out.push_str(&r[1..]);
                } else {
                    if out.ends_with(['a', 'i', 'u', 'e', 'o']) {
                        out.pop();
                    }
                    out.push_str(r);
                }
            }
            c => {
                let r = romaji(c)?;
                if double {
                    double = false;
                    if let Some(first) = r.chars().next().filter(|f| !"aiueon".contains(*f)) {
                        out.push(if r.starts_with("ch") { 't' } else { first });
                    }
                }
                out.push_str(r);
            }
        }
    }
    Some(out.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Cyrillic letters in Latin (one scheme; [`fold_sounds`] evens out the rest).
fn cyrillic_latin(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a",
        'б' => "b",
        'в' => "v",
        'г' => "g",
        'д' => "d",
        'е' | 'ё' | 'э' => "e",
        'ж' => "zh",
        'з' => "z",
        'и' | 'й' | 'ы' | 'і' => "i",
        'к' => "k",
        'л' => "l",
        'м' => "m",
        'н' => "n",
        'о' => "o",
        'п' => "p",
        'р' => "r",
        'с' => "s",
        'т' => "t",
        'у' => "u",
        'ф' => "f",
        'х' => "h",
        'ц' => "c",
        'ч' => "ch",
        'ш' | 'щ' => "sh",
        'ъ' | 'ь' => "",
        'ю' => "iu",
        'я' => "ia",
        _ => return None,
    })
}

/// What romanizations disagree on, evened out: `y` and `j` as `i`, `kh` as
/// `h`, `ts` as `c`, `w` as `v`, long vowels (`ou`, `oo`, `uu`) and double
/// letters once.
fn fold_sounds(word: &str) -> String {
    let mut w = word.to_owned();
    for (from, to) in [("kh", "h"), ("ts", "c"), ("tz", "c"), ("ou", "o"), ("y", "i"), ("j", "i"), ("w", "v")] {
        w = w.replace(from, to);
    }
    let mut out = String::with_capacity(w.len());
    for c in w.chars() {
        if out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    out
}

fn deaccent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => 'a',
        'ç' | 'č' | 'ć' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'ē' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ī' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => 'o',
        'š' => 's',
        'ù' | 'ú' | 'û' | 'ü' | 'ū' => 'u',
        'ý' | 'ÿ' => 'y',
        'ž' => 'z',
        c => c,
    }
}

/// How a name sounds in Latin letters, words folded ([`fold_sounds`]):
/// Latin as written, kana in romaji, Cyrillic transliterated; `None` for a
/// name with anything else (kanji) or without letters.
pub fn latin_key(name: &str) -> Option<String> {
    let s = width_fold(name).to_lowercase();
    let latin = if let Some(r) = kana_romaji(&s) {
        r
    } else if s.chars().any(is_cyrillic) {
        let mut out = String::new();
        for c in s.chars() {
            match cyrillic_latin(c) {
                Some(l) => out.push_str(l),
                None if c.is_alphabetic() && !c.is_ascii() => return None,
                None => out.push(c),
            }
        }
        out
    } else if is_latin(&s) {
        s.chars().map(deaccent).collect()
    } else {
        return None;
    };
    let words: Vec<String> = latin
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(fold_sounds)
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// A name of two Latin words in the other order (`Fujisawa Yoshiaki` →
/// `yoshiaki fujisawa`, folded like [`latin_key`]).
pub fn swapped(name: &str) -> Option<String> {
    if !is_latin(name) {
        return None;
    }
    let key = latin_key(name)?;
    match key.split(' ').collect::<Vec<_>>()[..] {
        [a, b] => Some(format!("{b} {a}")),
        _ => None,
    }
}

/// Two spellings of one name by their sound ([`latin_key`], [`swapped`]):
/// `ヒトリエ` and `HITORIE`, `Татьяна Буланова` and `Tatiana Bulanova`,
/// `Fujisawa Yoshiaki` and `Yoshiaki Fujisawa`. Short names (under four
/// letters) only as written.
pub fn same_name(a: &str, b: &str) -> bool {
    let (ka, kb) = (name_key(a), name_key(b));
    if !ka.is_empty() && ka == kb {
        return true;
    }
    let (Some(la), Some(lb)) = (latin_key(a), latin_key(b)) else { return false };
    if la.replace(' ', "").len() < 4 {
        return false;
    }
    let squash = |s: &str| s.replace(' ', "");
    squash(&la) == squash(&lb) || swapped(a).is_some_and(|s| s == lb) || swapped(b).is_some_and(|s| s == la)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_compilation_artist_names_nobody() {
        for name in ["Various Artists", "VARIOUS ARTISTS", "V.A.", "Разные исполнители", "Сборник", "Various", "Varios Artistas"] {
            assert!(is_various_artists(name), "{name}");
        }
        for name in ["Various Cruelties", "Artists", "Кино", "Vanessa"] {
            assert!(!is_various_artists(name), "{name}");
        }
    }

    #[test]
    fn widths_and_marks_fold() {
        assert_eq!(width_fold("（CV：富田美憂）"), "(CV:富田美憂)");
        assert_eq!(width_fold("ﾉｰｹﾞｰﾑ･ﾉｰﾗｲﾌ"), "ノーゲーム・ノーライフ");
        assert_eq!(name_key("リコ（CV：富田美憂）"), name_key("リコ(CV:富田美憂)"));
        assert_eq!(name_key("HITORIE"), name_key("Hitorie"));
        assert_eq!(unvoiced('パ'), 'ハ');
        assert_eq!(unvoiced('バ'), 'ハ');
        assert_eq!(unvoiced('が'), 'か');
        assert_eq!(unvoiced('ヅ'), 'ツ');
        assert_eq!(unvoiced('ア'), 'ア');
        assert_eq!(unvoiced('ロ'), 'ロ');
    }

    #[test]
    fn folders_name_their_disc() {
        assert_eq!(disc_of_folder("CD2"), Some(2));
        assert_eq!(disc_of_folder("Disc 1"), Some(1));
        assert_eq!(disc_of_folder("Диск 3"), Some(3));
        assert_eq!(disc_of_folder("Album [CD2]"), Some(2));
        assert_eq!(disc_of_folder("ＣＤ２"), Some(2));
        assert_eq!(disc_of_folder("Discography"), None);
        assert_eq!(disc_of_folder("'Mushoku Tensei' Original Soundtrack 2"), None);
        assert_eq!(voiceless("パウロ"), voiceless("バウロ"));
        assert_ne!(voiceless("パウロ"), voiceless("パオロ"));
    }

    #[test]
    fn kana_reads_in_romaji() {
        assert_eq!(kana_romaji("ヒトリエ").as_deref(), Some("hitorie"));
        assert_eq!(kana_romaji("ナナホシ").as_deref(), Some("nanahoshi"));
        assert_eq!(kana_romaji("シルフィエット").as_deref(), Some("shirufietto"));
        assert_eq!(kana_romaji("ジブリール").as_deref(), Some("jiburiru"));
        assert_eq!(kana_romaji("きゃりーぱみゅぱみゅ").as_deref(), Some("kyaripamyupamyu"));
        assert_eq!(kana_romaji("大原ゆい子"), None, "kanji");
        assert_eq!(kana_romaji("ーー"), None);
    }

    #[test]
    fn a_romaji_name_only_for_kana() {
        assert_eq!(romaji_name("リコ").as_deref(), Some("Riko"));
        assert_eq!(romaji_name("ナナホシ").as_deref(), Some("Nanahoshi"));
        assert_eq!(romaji_name("ｼﾙﾌｨｴｯﾄ").as_deref(), Some("Shirufietto"));
        assert_eq!(romaji_name("ジブリール・ブラン").as_deref(), Some("Jiburiru Buran"));
        assert_eq!(romaji_name("佐宗綾子"), None, "kanji");
        assert_eq!(romaji_name("大原ゆい子"), None, "kanji with kana");
        assert_eq!(romaji_name("YOASOBI"), None);
        assert_eq!(romaji_name("Кино"), None);
    }

    #[test]
    fn foreign_scripts_are_neither_latin_nor_cyrillic() {
        for name in ["佐宗綾子", "リコ", "ヨルシカ", "아이유", "Βασίλης", "大原ゆい子 & Ohara"] {
            assert!(foreign_script(name), "{name}");
        }
        for name in ["Кино", "Beyoncé", "Sigur Rós", "Zhitʼ", "Phạm Quỳnh Anh", "Ｙ Ｏ Ａ", "", "123"] {
            assert!(!foreign_script(name), "{name}");
        }
    }

    #[test]
    fn one_name_in_two_scripts() {
        assert!(same_name("ヒトリエ", "HITORIE"));
        assert!(same_name("Татьяна Буланова", "Tatiana Bulanova"));
        assert!(same_name("Fujisawa Yoshiaki", "Yoshiaki Fujisawa"));
        assert!(same_name("Oohara Yuuko", "Ohara Yuko"));
        assert!(same_name("リコ（CV：富田美憂）", "リコ(CV:富田美憂)"));
        assert!(!same_name("大原ゆい子", "Ohara Yuiko"), "kanji need other evidence");
        assert!(!same_name("Кино", "Kina"));
        assert!(!same_name("Rambl", "Мэвл"));
        assert!(!same_name("ナナホシ", "Wakayama Shion"));
        assert!(!same_name("Ado", "Odo"), "short names only as written");
        assert!(is_native("大原ゆい子") && is_native("Татьяна") && !is_native("Ohara Yuiko"));
        assert!(is_latin("Beyoncé") && !is_latin("ヒトリエ"));
    }
}
