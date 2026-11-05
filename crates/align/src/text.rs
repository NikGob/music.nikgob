//! Lyrics text → pieces to time and the tokens of a CTC model.
//!
//! A line is cut into pieces that keep their trailing spaces (joined they
//! give the line back, as [`nikgob_core::lyrics::LyricWord`] wants): words
//! of space-separated scripts, single characters of Japanese and Chinese.
//! Each piece becomes the model's letters: as written for a model of the
//! script ([`Letters::Native`]), or in Latin letters for the multilingual
//! romanised one ([`Letters::Roman`]). Letters a model lacks become
//! wildcards; punctuation becomes nothing.

use crate::ctc::STAR;

/// How a model spells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Letters {
    /// Its vocabulary holds the script's own (lower-case) letters; words are
    /// separated by `|`.
    Native,
    /// Latin letters only (MMS): Cyrillic and kana are romanised, kanji are
    /// wildcards; no word separator.
    Roman,
}

/// The script a text is mostly written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Script {
    Latin,
    Cyrillic,
    /// Kana (and kanji alongside it).
    Japanese,
    /// Han characters without kana.
    Chinese,
    Korean,
    Other,
}

pub fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' | '\u{ff66}'..='\u{ff9f}')
}

pub fn is_han(c: char) -> bool {
    matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{f900}'..='\u{faff}' | '々' | '〆')
}

fn is_hangul(c: char) -> bool {
    matches!(c, '\u{ac00}'..='\u{d7af}' | '\u{1100}'..='\u{11ff}' | '\u{3130}'..='\u{318f}')
}

/// Characters that are a piece of their own (no spaces between words).
fn is_cjk(c: char) -> bool {
    is_kana(c) || is_han(c)
}

impl Script {
    /// The script most letters of `text` are in (kana anywhere make it
    /// Japanese).
    pub fn of(text: &str) -> Self {
        let (mut latin, mut cyr, mut kana, mut han, mut hangul, mut other) = (0, 0, 0, 0, 0, 0);
        for c in text.chars() {
            if c.is_ascii_alphabetic() || matches!(c, '\u{00c0}'..='\u{024f}') {
                latin += 1;
            } else if matches!(c, '\u{0400}'..='\u{04ff}') {
                cyr += 1;
            } else if is_kana(c) {
                kana += 1;
            } else if is_han(c) {
                han += 1;
            } else if is_hangul(c) {
                hangul += 1;
            } else if c.is_alphabetic() {
                other += 1;
            }
        }
        if kana > 0 && kana + han >= (latin + cyr) / 4 {
            return Self::Japanese;
        }
        let best = [(latin, Self::Latin), (cyr, Self::Cyrillic), (han, Self::Chinese), (hangul, Self::Korean), (other, Self::Other)]
            .into_iter()
            .max_by_key(|(n, _)| *n)
            .unwrap_or((0, Self::Other));
        if best.0 == 0 { Self::Other } else { best.1 }
    }
}

/// The language a text (or one of its lines) is sung in, as far as the
/// choice of an aligner goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Lang {
    English,
    Russian,
    Japanese,
    Chinese,
    Korean,
    /// Any other language (Latin, Cyrillic or another script).
    Other,
}

/// Words that tell a Latin-script language (the commonest of each).
const WORDS: [(&str, &[&str]); 7] = [
    ("en", &["the", "and", "you", "i", "to", "me", "my", "it", "is", "that", "your", "on", "for", "be", "we", "all", "with", "love", "so", "this", "but", "just", "when", "what", "like", "don't", "can", "know", "i'm", "it's", "of", "in", "a", "oh", "yeah", "now"]),
    ("es", &["que", "de", "la", "el", "y", "en", "me", "un", "una", "mi", "por", "te", "lo", "es", "con", "se", "tu", "para", "como", "yo", "pero", "amor", "más"]),
    ("fr", &["je", "de", "la", "le", "et", "les", "que", "tu", "pas", "un", "une", "est", "des", "dans", "mon", "moi", "pour", "qui", "ne", "il", "sur", "avec", "c'est"]),
    ("de", &["ich", "und", "die", "der", "du", "nicht", "das", "ist", "es", "ein", "mich", "mir", "zu", "wie", "sie", "wir", "auf", "mein", "dich"]),
    ("it", &["che", "di", "il", "la", "non", "e", "un", "mi", "ti", "per", "una", "sono", "io", "tu", "lo", "con", "se", "ma"]),
    ("pt", &["que", "de", "não", "eu", "e", "o", "um", "você", "me", "meu", "do", "da", "se", "em", "mais", "por", "pra"]),
    ("pl", &["nie", "się", "w", "to", "na", "że", "jest", "z", "mnie", "mi", "jak", "ja", "co", "tak", "ty", "do"]),
];

/// A word made of Japanese syllables written in Latin letters (romaji).
fn romaji(word: &str) -> bool {
    let b = word.as_bytes();
    let vowel = |c: u8| b"aiueo".contains(&c);
    let mut i = 0;
    while i < b.len() {
        if vowel(b[i]) {
            i += 1;
            continue;
        }
        if b[i] == b'n' && b.get(i + 1).is_none_or(|c| !vowel(*c) && *c != b'y') {
            i += 1;
            continue;
        }
        // A consonant (doubled, «sh», «ch», «ts», then maybe «y») and a vowel.
        let mut j = i + 1;
        if b.get(j) == Some(&b[i]) && b[i] != b'n' {
            j += 1;
        }
        if matches!((b[i], b.get(j)), (b's' | b'c', Some(b'h')) | (b't', Some(b's'))) {
            j += 1;
        }
        if !b"kgsztdnhbpmyrwfjc".contains(&b[i]) {
            return false;
        }
        if b.get(j) == Some(&b'y') {
            j += 1;
        }
        if !b.get(j).is_some_and(|c| vowel(*c)) {
            return false;
        }
        i = j + 1;
    }
    !b.is_empty()
}

/// Whether a Latin-script text is English (rather than another language
/// in Latin letters, romaji included). Short lines with nothing telling
/// count as English: that is what the Latin lines of a Russian or
/// Japanese song mostly are.
fn english(text: &str) -> bool {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !(c.is_alphabetic() || c == '\'' || c == '’'))
        .filter(|w| !w.is_empty())
        .collect();
    if words.is_empty() {
        return true;
    }
    let share = |list: &[&str]| words.iter().filter(|w| list.contains(w)).count() as f32 / words.len() as f32;
    let en = share(WORDS[0].1);
    let rest = WORDS[1..].iter().map(|(_, l)| share(l)).fold(0.0, f32::max);
    if en >= 0.08 && en >= rest {
        return true;
    }
    if rest >= 0.08 {
        return false;
    }
    let letters: Vec<char> = lower.chars().filter(|c| c.is_alphabetic()).collect();
    let marked = letters.iter().filter(|c| !c.is_ascii()).count();
    if marked as f32 > letters.len() as f32 * 0.02 {
        return false;
    }
    let roman = words.iter().filter(|w| w.len() > 1 && romaji(w)).count();
    roman as f32 <= words.len() as f32 * 0.6 || words.len() < 3
}

impl Lang {
    /// The language of a text (or a line).
    pub fn of(text: &str) -> Self {
        match Script::of(text) {
            Script::Japanese => Self::Japanese,
            Script::Chinese => Self::Chinese,
            Script::Korean => Self::Korean,
            Script::Latin if english(text) => Self::English,
            // Ukrainian and Belarusian letters the Russian aligner lacks.
            Script::Cyrillic if !text.chars().any(|c| matches!(c, 'і' | 'ї' | 'є' | 'ґ' | 'ў' | 'І' | 'Ї' | 'Є' | 'Ґ' | 'Ў')) => Self::Russian,
            _ => Self::Other,
        }
    }

    /// The language of each line of a text sung mostly in `main`: a line
    /// in another script than the text's is its own language (an English
    /// line in a Russian song), the rest are the text's (kanji alone in a
    /// Japanese song, a Spanish word in an English one).
    pub fn of_lines<'a>(main: Self, lines: impl IntoIterator<Item = &'a str>) -> Vec<Self> {
        let script = |l: Self| match l {
            Self::English => Some(Script::Latin),
            Self::Russian => Some(Script::Cyrillic),
            Self::Japanese => Some(Script::Japanese),
            _ => None,
        };
        lines
            .into_iter()
            .map(|line| {
                let own = Script::of(line);
                let letters = line.chars().filter(|c| c.is_alphabetic()).count();
                let differs = matches!(own, Script::Latin | Script::Cyrillic | Script::Japanese) && Some(own) != script(main);
                if letters >= 2 && differs && !(main == Self::Other && own == Script::Latin) {
                    Self::of(line)
                } else {
                    main
                }
            })
            .collect()
    }
}

/// Whether a piece is hummed or a held vowel rather than said: «м», «мм»,
/// «мгм», «угу», «хм», «mm», «hmm», «mhm», and one vowel held over a few
/// letters («а-а», «о-о-о», «ooo»). A speech model hears such a
/// piece as blank or as anything, so it is timed by the voice instead
/// ([`crate::ctc::Tok::Hum`]). A single vowel is a word («а», «о», «a»).
pub fn is_hum(piece: &str) -> bool {
    let letters: Vec<char> = piece.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect();
    // A kana is a syllable of a word («ん» too), never a hum of its own.
    let Some(&first) = letters.first().filter(|_| !piece.chars().any(is_cjk)) else { return false };
    let word: String = letters.iter().collect();
    if letters.iter().all(|c| matches!(c, 'м' | 'm')) {
        return true;
    }
    if matches!(word.as_str(), "мгм" | "угу" | "хм" | "хмм" | "гм" | "hm" | "hmm" | "mhm" | "mmh" | "mmm") {
        return true;
    }
    letters.len() >= 2 && "аеёиоуыэaeiou".contains(first) && letters.iter().all(|c| *c == first)
}

/// Cuts a line into the pieces timed one by one; they keep their trailing
/// whitespace, so joined they give `line` back.
pub fn pieces(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            cur.push(c);
            // A run of spaces ends the piece once something else follows.
            if chars.peek().is_some_and(|n| !n.is_whitespace()) {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if is_cjk(c) {
            // Small kana and the long mark are sung with the one before.
            let joins = matches!(c, 'ゃ' | 'ゅ' | 'ょ' | 'ャ' | 'ュ' | 'ョ' | 'ぁ' | 'ぃ' | 'ぅ' | 'ぇ' | 'ぉ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ' | 'ー' | 'っ' | 'ッ' | '々');
            if !cur.is_empty() && !(joins && cur.chars().last().is_some_and(is_cjk)) {
                out.push(std::mem::take(&mut cur));
            }
            cur.push(c);
            continue;
        }
        if cur.chars().last().is_some_and(is_cjk) && c.is_alphanumeric() {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        // Trailing spaces stay with the last piece.
        if cur.trim().is_empty() && !out.is_empty() {
            out.last_mut().unwrap().push_str(&cur);
        } else {
            out.push(cur);
        }
    }
    out
}

/// A model's letters and how it spells.
#[derive(Clone, Debug)]
pub struct Vocab {
    pub letters: Letters,
    /// Letter → id; multi-letter keys (`<pad>`) are not letters.
    ids: std::collections::HashMap<char, i32>,
    /// The word separator (`|`), for native models.
    pub sep: Option<i32>,
    pub blank: usize,
    /// Latin words are spelled by how they sound in the model's letters
    /// ([`english_as_russian`]) when it has Cyrillic letters and no Latin
    /// ones; off, they are wildcards.
    pub by_sound: bool,
}

impl Vocab {
    /// From a Hugging Face `vocab.json` (letter → id).
    pub fn from_json(json: &str, letters: Letters) -> Result<Self, serde_json::Error> {
        let map: std::collections::HashMap<String, i32> = serde_json::from_str(json)?;
        let blank = ["<blank>", "<pad>"].iter().find_map(|k| map.get(*k)).copied().unwrap_or(0) as usize;
        let sep = if letters == Letters::Native { map.get("|").copied() } else { None };
        let single = |k: &str| {
            let mut cs = k.chars();
            let c = cs.next()?;
            (cs.next().is_none() && c != '|' && !c.is_whitespace()).then_some(c)
        };
        // Some models spell in capitals only (wav2vec2 960h): the text is
        // matched in lower case.
        let upper = map.keys().filter_map(|k| single(k)).any(char::is_uppercase)
            && !map.keys().filter_map(|k| single(k)).any(char::is_lowercase);
        let ids = map
            .iter()
            .filter_map(|(k, v)| {
                let c = single(k)?;
                Some((if upper { c.to_lowercase().next().unwrap_or(c) } else { c }, *v))
            })
            .collect();
        Ok(Self { letters, ids, sep, blank, by_sound: false })
    }

    /// The model spells Russian and has no Latin letters.
    fn cyrillic_only(&self) -> bool {
        self.ids.contains_key(&'а') && !self.ids.contains_key(&'a')
    }

    /// Letters a singer's vowels are heard as for each other (ids, both
    /// ways): и/ы/е/э, а/о/я, у/ю ([`crate::ctc::Costs::vowels`]). Only for
    /// a Russian model.
    pub fn similar(&self) -> Vec<(i32, i32)> {
        if !self.cyrillic_only() {
            return Vec::new();
        }
        let pairs = [('и', 'ы'), ('и', 'е'), ('е', 'э'), ('ы', 'е'), ('а', 'о'), ('а', 'я'), ('у', 'ю'), ('о', 'у')];
        pairs.iter().filter_map(|(a, b)| Some((*self.ids.get(a)?, *self.ids.get(b)?))).collect()
    }

    /// A model of a script's characters (thousands of them: Japanese)
    /// rather than of an alphabet's letters: it is unsure of every sung
    /// character, so voice the text lacks cannot be told from the text.
    pub fn syllabic(&self) -> bool {
        self.ids.len() > 200
    }

    /// The tokens of one piece (empty for punctuation).
    pub fn tokens(&self, piece: &str) -> Vec<i32> {
        let mut out = Vec::new();
        let mut lower: String = piece.trim().chars().flat_map(char::to_lowercase).collect();
        if self.by_sound && self.letters == Letters::Native && self.cyrillic_only() && lower.chars().any(|c| c.is_ascii_alphabetic()) {
            lower = latin_by_sound(&lower);
        }
        match self.letters {
            Letters::Native => {
                for c in lower.chars().map(fold) {
                    if let Some(id) = self.ids.get(&c) {
                        out.push(*id);
                    } else if let Some(base) = base_letters(c).filter(|b| b.chars().all(|l| self.ids.contains_key(&l))) {
                        // «ё» of a model without it, «é» of an English one.
                        out.extend(base.chars().map(|l| self.ids[&l]));
                    } else if c.is_alphanumeric() {
                        out.push(STAR);
                    }
                }
            }
            Letters::Roman => {
                let mut prev: Option<char> = None;
                let chars: Vec<char> = lower.chars().collect();
                for (i, &c) in chars.iter().enumerate() {
                    let next = chars.get(i + 1).copied();
                    let roman = romanise(c, prev, next);
                    prev = Some(c);
                    match roman {
                        Some(r) => out.extend(r.chars().filter_map(|l| self.ids.get(&l).copied())),
                        None if c.is_alphanumeric() => out.push(STAR),
                        None => {}
                    }
                }
            }
        }
        out
    }
}

/// Every run of Latin letters in `text` read the Russian way
/// ([`english_as_russian`]); the rest stays.
fn latin_by_sound(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let mut word = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        // An apostrophe inside a Latin word («Levi's», «don't») is part of it.
        let inside = matches!(c, '\'' | '’') && !word.is_empty() && chars.get(i + 1).is_some_and(char::is_ascii_alphabetic);
        if c.is_ascii_alphabetic() || inside {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            out.push_str(&english_as_russian(&std::mem::take(&mut word)));
        }
        out.push(c);
    }
    if !word.is_empty() {
        out.push_str(&english_as_russian(&word));
    }
    out
}

/// Common words the rules below read wrong.
const SAID: &[(&str, &str)] = &[
    ("a", "э"), ("i", "ай"), ("i'm", "айм"), ("the", "зе"), ("to", "ту"), ("do", "ду"), ("who", "ху"), ("you", "ю"), ("your", "ёр"),
    ("you're", "юр"), ("my", "май"), ("yeah", "е"), ("yea", "е"), ("oh", "оу"), ("ah", "а"), ("uh", "а"), ("ok", "окей"), ("okay", "окей"),
    ("hey", "хей"), ("what", "уот"), ("want", "уонт"), ("was", "уоз"), ("of", "оф"), ("are", "ар"), ("were", "уер"), ("one", "уан"),
    ("come", "кам"), ("some", "сам"), ("love", "лав"), ("give", "гив"), ("live", "лив"), ("have", "хэв"), ("gone", "гон"), ("done", "дан"),
    ("none", "нан"), ("there", "зэр"), ("where", "уэр"), ("here", "хир"), ("more", "мор"), ("sure", "шур"), ("all", "ол"), ("call", "кол"),
    ("ball", "бол"), ("talk", "ток"), ("walk", "уок"), ("know", "ноу"), ("money", "мани"), ("honey", "хани"), ("heart", "харт"),
    ("good", "гуд"), ("look", "лук"), ("book", "бук"), ("put", "пут"), ("push", "пуш"), ("full", "фул"), ("people", "пипл"),
    ("beautiful", "бьютифул"), ("girl", "гёрл"), ("world", "уорлд"), ("work", "уорк"), ("word", "уорд"), ("night", "найт"),
    ("baby", "бейби"), ("lady", "леди"), ("hello", "хеллоу"), ("bye", "бай"), ("goodbye", "гудбай"), ("levi's", "ливайс"),
    ("oliver", "оливер"), ("never", "невер"), ("ever", "эвер"), ("every", "эври"), ("very", "вери"), ("any", "эни"), ("many", "мени"),
];

/// How a Russian listener reads an English word, in Cyrillic, for the
/// Russian aligner: «Miley» → «майли», «Cyrus» → «сайрус», «Spies» →
/// «спайс», «Totally» → «тотали». A handful of rules (digraphs, a silent
/// final «e», a vowel long before one consonant in a two-syllable word, a
/// final «y»), common words from [`SAID`]; it need not be exact, only
/// closer to what is sung than wildcards are.
pub fn english_as_russian(word: &str) -> String {
    let w: String = word.to_lowercase().replace('’', "'");
    if let Some((_, r)) = SAID.iter().find(|(k, _)| *k == w) {
        return (*r).to_owned();
    }
    let w: Vec<u8> = w.bytes().filter(|b| b.is_ascii_alphabetic()).collect();
    if w.is_empty() {
        return String::new();
    }
    let n = w.len();
    let vowel = |i: usize| w.get(i).is_some_and(|c| b"aeiou".contains(c) || (*c == b'y' && i > 0 && !w.get(i + 1).is_some_and(|n| b"aeiou".contains(n))));
    // A silent final «e» after a consonant, in a word with another vowel.
    // A silent «e» after one consonant at the end («lone», «Sykes»): left
    // out, and the vowel before that consonant is long.
    let silent = if n > 2 && w[n - 1] == b'e' && !vowel(n - 2) && (0..n - 2).any(vowel) {
        Some(n - 1)
    } else if n > 3 && w[n - 1] == b's' && w[n - 2] == b'e' && !vowel(n - 3) && !b"sxzhc".contains(&w[n - 3]) && (0..n - 3).any(vowel) {
        Some(n - 2)
    } else {
        None
    };
    let magic_at = silent.and_then(|e| e.checked_sub(2)).filter(|v| vowel(*v));
    let end = n;
    // Vowel groups, for how many syllables the word has.
    let mut groups = 0;
    let mut i = 0;
    while i < end {
        if vowel(i) && Some(i) != silent {
            groups += 1;
            while i < end && vowel(i) && Some(i) != silent {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    let mut out = String::new();
    let mut i = 0;
    let mut group = 0;
    let at = |i: usize, s: &[u8]| w[i..].starts_with(s);
    while i < end {
        let c = w[i];
        if Some(i) == silent {
            i += 1;
            continue;
        }
        // Groups of letters first.
        let groups_of: &[(&str, &str)] = &[
            ("tion", "шн"), ("sion", "жн"), ("tch", "ч"), ("sch", "ск"), ("igh", "ай"), ("sh", "ш"), ("ch", "ч"), ("th", "з"),
            ("ph", "ф"), ("ck", "к"), ("qu", "кв"), ("wh", "у"), ("ng", "нг"), ("ee", "и"), ("ea", "и"), ("oo", "у"), ("ou", "ау"),
            ("oy", "ой"), ("oi", "ой"), ("ay", "ей"), ("ai", "ей"), ("au", "о"), ("aw", "о"), ("ew", "ю"),
        ];
        if let Some((g, r)) = groups_of.iter().find(|(g, _)| at(i, g.as_bytes()) && i + g.len() <= end) {
            if b"aeiouy".contains(&g.as_bytes()[0]) || *g == "igh" {
                group += 1;
            }
            out.push_str(r);
            i += g.len();
            continue;
        }
        if at(i, b"ow") {
            group += 1;
            out.push_str(if i + 2 == end && i > 1 { "оу" } else { "ау" });
            i += 2;
            continue;
        }
        if at(i, b"ey") && i + 2 == end {
            group += 1;
            out.push_str("и");
            i += 2;
            continue;
        }
        if at(i, b"ie") && (i + 2 == end || (i + 3 == end && w[end - 1] == b's')) {
            group += 1;
            // «spies», «die»: one syllable; «movie»: the last of two.
            out.push_str(if group == 1 { "ай" } else { "и" });
            i += 2;
            continue;
        }
        if at(i, b"ye") && i > 0 && i + 2 == end {
            group += 1;
            out.push_str("ай");
            i += 2;
            continue;
        }
        if c == b'y' && i == 0 && w.get(1).is_some_and(|n| b"aeiou".contains(n)) {
            let r = match w[1] {
                b'a' => "я",
                b'o' => "йо",
                b'u' => "ю",
                b'e' => "е",
                _ => "й",
            };
            out.push_str(r);
            group += 1;
            i += 2;
            continue;
        }
        if vowel(i) {
            group += 1;
            // Long before one consonant and a vowel (or a silent «e») in a
            // word of two syllables, or open at the end of a short word.
            let next_vowel = i + 2 < n && !vowel(i + 1) && vowel(i + 2) && Some(i + 2) != silent && w[i + 1] != b'x';
            let magic = magic_at == Some(i);
            let open = i + 1 == end && groups == 1;
            let long = magic || open || (next_vowel && groups == 2 && group == 1);
            let r = match (c, long) {
                (b'y', _) if i + 1 == end && groups > 1 => "и",
                (b'y', true) | (b'i', true) => "ай",
                (b'y', false) => "и",
                (b'a', true) => "ей",
                (b'e', true) => "и",
                (b'o', true) => "оу",
                (b'u', true) => "ю",
                (b'a', false) if group == 1 && i + 1 < end => "э",
                (b'a', false) => "а",
                (b'e', false) if i == 0 => "э",
                (b'e', false) => "е",
                (b'i', false) => "и",
                (b'o', false) => "о",
                (b'u', false) if group == 1 => "а",
                _ => "у",
            };
            out.push_str(r);
            i += 1;
            continue;
        }
        // A doubled consonant is one.
        if i > 0 && w[i - 1] == c && !b"aeiou".contains(&c) {
            i += 1;
            continue;
        }
        let soft = w.get(i + 1).is_some_and(|n| b"eiy".contains(n));
        let r = match c {
            b'b' => "б",
            b'c' if soft => "с",
            b'c' => "к",
            b'd' => "д",
            b'f' => "ф",
            b'g' if soft && silent == Some(i + 1) => "дж",
            b'g' => "г",
            // Silent after a vowel at the end («oh», «yeah»).
            b'h' if i + 1 == end && i > 0 => "",
            b'h' => "х",
            b'j' => "дж",
            b'k' if i == 0 && w.get(1) == Some(&b'n') => "",
            b'k' => "к",
            b'l' => "л",
            b'm' => "м",
            b'n' => "н",
            b'p' => "п",
            b'q' => "к",
            b'r' => "р",
            b's' => "с",
            b't' => "т",
            b'v' => "в",
            b'w' if i == 0 && w.get(1) == Some(&b'r') => "",
            b'w' if i > 0 && b"st".contains(&w[i - 1]) => "в",
            b'w' => "у",
            b'x' => "кс",
            b'z' => "з",
            _ => "",
        };
        out.push_str(r);
        i += 1;
    }
    out
}

/// Letters some texts use for others the models know.
fn fold(c: char) -> char {
    match c {
        '’' | '‘' | '`' | 'ʼ' => '\'',
        _ => c,
    }
}

/// One letter in Latin letters for MMS (`None`: not a letter it can spell).
fn romanise(c: char, _prev: Option<char>, next: Option<char>) -> Option<String> {
    if c.is_ascii_alphabetic() || c == '\'' {
        return Some(c.to_string());
    }
    if let Some(r) = latin_base(c) {
        return Some(r.to_owned());
    }
    if let Some(r) = cyrillic(c) {
        return Some(r.to_owned());
    }
    if is_kana(c) {
        let hira = to_hiragana(c);
        // The sokuon doubles the next consonant.
        if hira == 'っ' {
            let n = next.map(to_hiragana).and_then(kana_roman)?;
            return n.chars().next().filter(|c| !"aiueo".contains(*c)).map(|c| c.to_string()).or(Some(String::new()));
        }
        if hira == 'ー' {
            return Some(String::new());
        }
        return kana_roman(hira).map(str::to_owned);
    }
    None
}

/// The plain letters of a letter with a mark, for a model that lacks it.
fn base_letters(c: char) -> Option<&'static str> {
    match c {
        'ё' => Some("е"),
        'й' => Some("и"),
        _ => latin_base(c),
    }
}

fn latin_base(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' => "a",
        'æ' => "ae",
        'ç' | 'č' | 'ć' => "c",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
        'ì' | 'í' | 'î' | 'ï' | 'ī' => "i",
        'ñ' | 'ń' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => "o",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' => "u",
        'ý' | 'ÿ' => "y",
        'ß' => "ss",
        'ś' | 'š' => "s",
        'ź' | 'ż' | 'ž' => "z",
        'ł' => "l",
        'ř' => "r",
        'đ' | 'ď' => "d",
        'ť' => "t",
        _ => return None,
    })
}

fn cyrillic(c: char) -> Option<&'static str> {
    Some(match c {
        'а' => "a", 'б' => "b", 'в' => "v", 'г' => "g", 'д' => "d", 'е' => "e", 'ё' => "yo",
        'ж' => "zh", 'з' => "z", 'и' => "i", 'й' => "y", 'к' => "k", 'л' => "l", 'м' => "m",
        'н' => "n", 'о' => "o", 'п' => "p", 'р' => "r", 'с' => "s", 'т' => "t", 'у' => "u",
        'ф' => "f", 'х' => "kh", 'ц' => "ts", 'ч' => "ch", 'ш' => "sh", 'щ' => "shch",
        'ъ' => "", 'ы' => "y", 'ь' => "", 'э' => "e", 'ю' => "yu", 'я' => "ya",
        'і' => "i", 'ї' => "yi", 'є' => "ye", 'ґ' => "g", 'ў' => "u",
        _ => return None,
    })
}

fn to_hiragana(c: char) -> char {
    match c {
        '\u{30a1}'..='\u{30f6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn kana_roman(c: char) -> Option<&'static str> {
    Some(match c {
        'あ' | 'ぁ' => "a", 'い' | 'ぃ' => "i", 'う' | 'ぅ' => "u", 'え' | 'ぇ' => "e", 'お' | 'ぉ' => "o",
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        'さ' => "sa", 'し' => "shi", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "ji", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        'た' => "ta", 'ち' => "chi", 'つ' => "tsu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "ji", 'づ' => "zu", 'で' => "de", 'ど' => "do",
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        'は' => "ha", 'ひ' => "hi", 'ふ' => "fu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        'や' | 'ゃ' => "ya", 'ゆ' | 'ゅ' => "yu", 'よ' | 'ょ' => "yo",
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        'わ' => "wa", 'ゐ' => "i", 'ゑ' => "e", 'を' => "o", 'ん' => "n", 'ゔ' => "vu",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_keep_spaces_and_split_cjk() {
        assert_eq!(pieces("Hello  big world "), ["Hello  ", "big ", "world "]);
        assert_eq!(pieces("君の名は"), ["君", "の", "名", "は"]);
        assert_eq!(pieces("きょうも Love 夢"), ["きょ", "う", "も ", "Love ", "夢"]);
        assert_eq!(pieces("Привет, мир!").concat(), "Привет, мир!");
        assert!(pieces("   ").len() <= 1);
    }

    #[test]
    fn scripts() {
        assert_eq!(Script::of("Группа крови"), Script::Cyrillic);
        assert_eq!(Script::of("君の名は what"), Script::Japanese);
        assert_eq!(Script::of("Hello"), Script::Latin);
        assert_eq!(Script::of("我爱你"), Script::Chinese);
        assert_eq!(Script::of("…"), Script::Other);
    }

    #[test]
    fn languages_of_texts() {
        assert_eq!(Lang::of("I don't wanna be alone tonight\nHold me close"), Lang::English);
        assert_eq!(Lang::of("Oh yeah"), Lang::English);
        assert_eq!(Lang::of("Que te quiero, mi amor, para siempre"), Lang::Other);
        assert_eq!(Lang::of("Je ne sais pas pourquoi tu es là"), Lang::Other);
        assert_eq!(Lang::of("Kimi no na wa sayonara watashi no kokoro"), Lang::Other);
        assert_eq!(Lang::of("Группа крови на рукаве"), Lang::Russian);
        assert_eq!(Lang::of("Привіт, як справи? Їжак"), Lang::Other);
        assert_eq!(Lang::of("君の名は"), Lang::Japanese);
        assert_eq!(Lang::of("我爱你"), Lang::Chinese);
        assert_eq!(Lang::of("사랑해"), Lang::Korean);
        assert!(romaji("sayonara") && romaji("kokoro") && romaji("shinjitsu") && romaji("kitto"));
        assert!(!romaji("love") && !romaji("tonight") && !romaji("the"));
    }

    #[test]
    fn a_line_in_another_script_is_its_own_language() {
        let ru = Lang::of_lines(Lang::Russian, ["Я иду домой", "Baby, I love you", "OK", "(х2)"]);
        assert_eq!(ru, [Lang::Russian, Lang::English, Lang::English, Lang::Russian]);
        // Kanji alone are Japanese in a Japanese song; Latin letters are not.
        let ja = Lang::of_lines(Lang::Japanese, ["君の名は", "夢", "I want you", "a"]);
        assert_eq!(ja, [Lang::Japanese, Lang::Japanese, Lang::English, Lang::Japanese]);
        let en = Lang::of_lines(Lang::English, ["Hello there", "Привет, мир", "さよなら"]);
        assert_eq!(en, [Lang::English, Lang::Russian, Lang::Japanese]);
        // A Latin line of a Spanish song stays with the Spanish.
        assert_eq!(Lang::of_lines(Lang::Other, ["Baby I love you"]), [Lang::Other]);
    }

    #[test]
    fn capitals_only_vocabularies_and_missing_marks() {
        let caps = Vocab::from_json(r#"{"<pad>":0,"|":4,"E":5,"T":6,"A":7,"'":27,"C":19,"F":20}"#, Letters::Native).unwrap();
        assert_eq!(caps.tokens("Eat"), [5, 7, 6]);
        assert_eq!(caps.tokens("café"), [19, 7, 20, 5]);
        let ru = Vocab::from_json(r#"{"<pad>":0,"|":4,"е":10,"ж":11}"#, Letters::Native).unwrap();
        assert_eq!(ru.tokens("Ёж"), [10, 11]);
    }

    #[test]
    fn english_words_read_the_russian_way() {
        for (en, ru) in [
            ("Miley", "майли"),
            ("Cyrus", "сайрус"),
            ("Spies", "спайс"),
            ("Totally", "тотали"),
            ("Sykes", "сайкс"),
            ("sunshine", "саншайн"),
            ("Sorry", "сори"),
            ("Swag", "свэг"),
            ("Sheesh", "шиш"),
            ("crack", "крэк"),
            ("Rockstar", "рокстар"),
            ("Monster", "монстер"),
            ("high", "хай"),
            ("Levi's", "ливайс"),
            ("goodbye", "гудбай"),
            ("night", "найт"),
            ("Come", "кам"),
        ] {
            assert_eq!(english_as_russian(en), ru, "{en}");
        }
        assert_eq!(latin_by_sound("с miley cyrus, йоу!"), "с майли сайрус, йоу!");
        assert_eq!(latin_by_sound("speed'ом"), "спид'ом");
    }

    /// A Russian model spells a Latin word by sound instead of wildcards.
    #[test]
    fn latin_words_of_a_russian_text_are_spelled_by_sound() {
        let mut ru = Vocab::from_json(r#"{"<pad>":0,"|":4,"а":5,"й":6,"м":7,"л":8,"и":9}"#, Letters::Native).unwrap();
        assert_eq!(ru.tokens("Miley"), [STAR, STAR, STAR, STAR, STAR]);
        ru.by_sound = true;
        assert_eq!(ru.tokens("Miley"), [7, 5, 6, 8, 9]);
        assert_eq!(ru.tokens("мама"), [7, 5, 7, 5]);
        // A model with Latin letters keeps them.
        let mut en = Vocab::from_json(r#"{"<pad>":0,"|":4,"m":5,"i":6}"#, Letters::Native).unwrap();
        en.by_sound = true;
        assert_eq!(en.tokens("mi"), [5, 6]);
        assert!(en.similar().is_empty());
        let vowels = Vocab::from_json(r#"{"<pad>":0,"|":4,"а":5,"о":6,"и":7,"е":8}"#, Letters::Native).unwrap();
        assert_eq!(vowels.similar(), [(7, 8), (5, 6)]);
    }

    #[test]
    fn tokens_native_and_roman() {
        let native = Vocab::from_json(r#"{"<pad>":0,"|":4,"а":7,"б":8,"в":9}"#, Letters::Native).unwrap();
        assert_eq!(native.blank, 0);
        assert_eq!(native.sep, Some(4));
        assert_eq!(native.tokens("Ба-в!"), [8, 7, 9]);
        assert_eq!(native.tokens("бX"), [8, STAR]);
        assert!(native.tokens("…").is_empty());

        let json = r#"{"<blank>":0,"a":1,"b":2,"k":3,"s":4,"h":5,"i":6,"y":7,"o":8,"n":9,"'":10}"#;
        let roman = Vocab::from_json(json, Letters::Roman).unwrap();
        assert_eq!(roman.sep, None);
        // «бах» → b a k h; «しん» → s h i n; a kanji is a wildcard.
        assert_eq!(roman.tokens("бах"), [2, 1, 3, 5]);
        assert_eq!(roman.tokens("しん"), [4, 5, 6, 9]);
        assert_eq!(roman.tokens("夢"), [STAR]);
        // Letters the model lacks are left out: «d» and «t» here.
        assert_eq!(roman.tokens("don't"), [8, 9, 10]);
    }
}
