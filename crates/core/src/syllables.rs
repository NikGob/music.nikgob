//! Words cut into the syllables they are sung in, as written.
//!
//! The pieces keep every character of the word, so joined they give it
//! back (a [`crate::lyrics::LyricWord`] of each keeps the line's text):
//! leading punctuation goes with the first syllable, trailing punctuation
//! and spaces with the last, a hyphen with the part before it («нон-»,
//! «стоп»). A word without a vowel, a number or a word of kana or Han
//! characters (sung a character at a time already) is one piece.
//!
//! * Cyrillic: a syllable a vowel. One consonant between two vowels goes
//!   to the next one (не-бо), «й» stays with the one before (май-ли); in a
//!   longer run the sonorants (й р л м н) it starts with stay with the
//!   syllable before and the rest goes on (кон-тракт, солн-це, ко-смо),
//!   at least one consonant always going on; «ь» and «ъ» stay with their
//!   consonant (силь-но, се-мья).
//! * Latin: a syllable a run of vowels (a e i o u, y after a consonant or
//!   at the end), except where two of a e o meet (or «ia», «io», «eo»,
//!   «ua», «uo») outside «qu»/«gu». One consonant goes to the next vowel
//!   (Mi-ley, Cy-rus), «x» and «ck» stay; of a longer run the next vowel
//!   gets the last consonant, or the last two when they open a syllable
//!   (bl, tr, st…), the rest stays (gol-den, to-tal-ly, a-pril). «ch»,
//!   «sh», «th», «ph», «wh», «gh» are one consonant. English also drops a
//!   silent «e»: at the end after a consonant (time, Imagine; «-le» after a
//!   consonant is sung: lit-tle), in «-es» and «-ed» unless they are heard
//!   (pla-ces, want-ed), and before «-ly», «-ful», «-less», «-ness»,
//!   «-ment» (lone-ly).

/// Which rules a word is cut by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rules {
    Cyrillic,
    /// Latin letters read as English (a silent «e»).
    English,
    /// Latin letters of another language (every vowel heard).
    Latin,
    /// Not cut: kana, Han, Hangul, numbers, other scripts.
    Whole,
}

impl Rules {
    /// The rules for `word` by its letters: Cyrillic, Latin (`english`
    /// when the text is English, or a Latin word inside a text of another
    /// script, which is mostly English), else whole.
    pub fn of(word: &str, english: bool) -> Self {
        let (mut cyr, mut latin) = (0, 0);
        for c in word.chars() {
            if is_cjk(c) {
                return Self::Whole;
            }
            if is_cyrillic(c) {
                cyr += 1;
            } else if c.is_alphabetic() && (c.is_ascii() || matches!(c, '\u{00c0}'..='\u{024f}')) {
                latin += 1;
            }
        }
        if cyr > 0 && cyr >= latin {
            Self::Cyrillic
        } else if latin > 0 {
            if english { Self::English } else { Self::Latin }
        } else {
            Self::Whole
        }
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{31f0}'..='\u{31ff}' | '\u{ff66}'..='\u{ff9f}' | '\u{4e00}'..='\u{9fff}' | '\u{3400}'..='\u{4dbf}' | '\u{f900}'..='\u{faff}' | '\u{ac00}'..='\u{d7af}' | '\u{1100}'..='\u{11ff}' | '\u{3130}'..='\u{318f}')
}

fn is_cyrillic(c: char) -> bool {
    matches!(c, '\u{0400}'..='\u{04ff}')
}

/// `word` cut into its syllables by `rules`; joined they give `word`.
pub fn split(word: &str, rules: Rules) -> Vec<&str> {
    if rules == Rules::Whole || word.trim().is_empty() {
        return vec![word];
    }
    // Byte offsets where a new syllable starts (after the first).
    let mut cuts: Vec<usize> = Vec::new();
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    // The written parts of a hyphenated word, each cut on its own; the
    // hyphen ends the part before it.
    let mut from = 0;
    for k in 0..chars.len() {
        let (_, c) = chars[k];
        let hyphen = matches!(c, '-' | '\u{2010}' | '\u{2011}')
            && k > 0
            && chars[k - 1].1.is_alphabetic()
            && chars.get(k + 1).is_some_and(|(_, n)| n.is_alphabetic());
        if hyphen {
            part_cuts(&chars[from..=k], rules, &mut cuts);
            cuts.push(chars[k + 1].0);
            from = k + 1;
        }
    }
    part_cuts(&chars[from..], rules, &mut cuts);
    let mut out = Vec::with_capacity(cuts.len() + 1);
    let mut at = 0;
    for c in cuts {
        if c > at && c < word.len() {
            out.push(&word[at..c]);
            at = c;
        }
    }
    out.push(&word[at..]);
    out
}

/// The cuts inside one written part (no hyphen in it but maybe at its end).
fn part_cuts(part: &[(usize, char)], rules: Rules, cuts: &mut Vec<usize>) {
    // The letters, lower case, and where each is in the part.
    let letters: Vec<(usize, char)> =
        part.iter().filter(|(_, c)| c.is_alphabetic()).map(|(i, c)| (*i, c.to_lowercase().next().unwrap_or(*c))).collect();
    if letters.len() < 2 {
        return;
    }
    let lower: Vec<char> = letters.iter().map(|(_, c)| *c).collect();
    let nuclei = match rules {
        Rules::Cyrillic => cyrillic_nuclei(&lower),
        Rules::English | Rules::Latin => latin_nuclei(&lower, rules == Rules::English),
        Rules::Whole => return,
    };
    for w in nuclei.windows(2) {
        let (a, b) = (w[0], w[1]);
        // The letters between the two vowels (a.1 is one past the first).
        let at = match rules {
            Rules::Cyrillic => cyrillic_cut(&lower, a.1, b.0),
            _ => latin_cut(&lower, a.1, b.0),
        };
        // A cut is placed before the letter it starts with; anything not a
        // letter right before it (an apostrophe) stays with the left.
        cuts.push(letters[at].0);
    }
}

const CYR_VOWELS: &str = "аеёиоуыэюяіїє";

/// Each vowel is a syllable's: (first letter, one past the last).
fn cyrillic_nuclei(l: &[char]) -> Vec<(usize, usize)> {
    (0..l.len()).filter(|i| CYR_VOWELS.contains(l[*i])).map(|i| (i, i + 1)).collect()
}

/// Where the syllable after the vowel ending at `from` starts, the next
/// vowel being at `to`.
fn cyrillic_cut(l: &[char], from: usize, to: usize) -> usize {
    let soft = |c: char| matches!(c, 'ь' | 'ъ');
    // The consonants between (the signs go with the one before them).
    let cons: Vec<usize> = (from..to).filter(|i| !soft(l[*i])).collect();
    // A cut before consonant `k` of the run, moved past the signs after
    // the one before it.
    let before = |k: usize| -> usize { if k == 0 { from } else { cons[k] } };
    match cons.len() {
        0 => to,
        1 => {
            if matches!(l[cons[0]], 'й' | 'ў') {
                to
            } else {
                cons[0]
            }
        }
        n => {
            let sonorant = |c: char| matches!(c, 'й' | 'ў' | 'р' | 'л' | 'м' | 'н');
            let lead = cons.iter().take_while(|i| sonorant(l[**i])).count();
            // «й» before the last consonant still stays: «айва» is ай-ва.
            before(lead.min(n - 1))
        }
    }
}

/// Runs of vowels that are one syllable's sound: (first, one past last).
fn latin_nuclei(l: &[char], english: bool) -> Vec<(usize, usize)> {
    let n = l.len();
    let plain = |c: char| "aeiouàáâãäåæèéêëìíîïòóôõöøùúûüýÿœāēīōū".contains(c);
    // «y» is a vowel after a consonant or at the end, a consonant at the
    // start and between vowels (you, beyond, player).
    let vowel: Vec<bool> = (0..n)
        .map(|i| {
            let c = l[i];
            if c == 'y' {
                let after_vowel = i > 0 && plain(l[i - 1]);
                let before_vowel = l.get(i + 1).is_some_and(|c| plain(*c));
                i > 0 && !(after_vowel && before_vowel) && !(before_vowel && i == 0)
            } else {
                plain(c)
            }
        })
        .collect();
    // «u» of «qu»/«gu» before a vowel is no vowel of its own.
    let vowel: Vec<bool> = (0..n)
        .map(|i| vowel[i] && !(l[i] == 'u' && i > 0 && matches!(l[i - 1], 'q' | 'g') && vowel.get(i + 1).copied().unwrap_or(false)))
        .collect();
    let strong = |c: char| matches!(c, 'a' | 'e' | 'o');
    let hiatus = |a: char, b: char| {
        (strong(a) && strong(b) && !(english && matches!((a, b), ('e', 'a') | ('o', 'a') | ('o', 'o') | ('e', 'e'))))
            || matches!((a, b), ('i', 'a') | ('i', 'o') | ('e', 'o') | ('u', 'a') | ('u', 'o'))
    };
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if !vowel[i] {
            i += 1;
            continue;
        }
        let a = i;
        i += 1;
        while i < n && vowel[i] && !hiatus(l[i - 1], l[i]) {
            i += 1;
        }
        out.push((a, i));
    }
    if english {
        drop_silent_e(l, &mut out);
    }
    out
}

/// English «e» that is written, not sung (see the module's notes).
fn drop_silent_e(l: &[char], nuclei: &mut Vec<(usize, usize)>) {
    let n = l.len();
    let consonant = |i: usize| l.get(i).is_some_and(|c| c.is_alphabetic() && !"aeiouy".contains(*c));
    let tail = |i: usize| -> String { l[i..].iter().collect() };
    let mut k = 1;
    while k < nuclei.len() {
        let (a, b) = nuclei[k];
        let single_e = b == a + 1 && l[a] == 'e' && consonant(a.wrapping_sub(1)) && a > 0;
        let silent = single_e && {
            let rest = tail(b);
            let prev = l[a - 1];
            let before = a.checked_sub(2).map(|i| l[i]);
            match rest.as_str() {
                // time, Imagine; little and table are sung.
                "" => !(prev == 'l' && before.is_some_and(|c| consonant(a - 2) && c != 'l')),
                "s" => !matches!(prev, 's' | 'z' | 'x' | 'c' | 'g') && !(prev == 'h' && matches!(before, Some('c' | 's'))),
                "d" => !matches!(prev, 't' | 'd'),
                "ly" | "ful" | "less" | "ness" | "ment" | "ments" => b + rest.len() == n,
                _ => false,
            }
        };
        if silent {
            nuclei.remove(k);
        } else {
            k += 1;
        }
    }
}

/// [`cyrillic_cut`] for Latin letters.
fn latin_cut(l: &[char], from: usize, to: usize) -> usize {
    // The consonant units between: a digraph counts as one.
    let mut units: Vec<(usize, usize)> = Vec::new();
    let mut i = from;
    while i < to {
        let pair = (l[i], l.get(i + 1).copied().unwrap_or(' '));
        let two = i + 1 < to && matches!(pair, ('c', 'h') | ('s', 'h') | ('t', 'h') | ('p', 'h') | ('w', 'h') | ('g', 'h') | ('c', 'k'));
        let len = if two { 2 } else { 1 };
        units.push((i, i + len));
        i += len;
    }
    let onset = |a: &(usize, usize), b: &(usize, usize)| {
        let (x, y) = (&l[a.0..a.1], &l[b.0..b.1]);
        let s: String = x.iter().chain(y).collect();
        matches!(
            s.as_str(),
            "bl" | "br" | "cl" | "cr" | "dr" | "fl" | "fr" | "gl" | "gr" | "pl" | "pr" | "tr" | "sc" | "sk" | "sl" | "sm" | "sn" | "sp" | "st" | "sw" | "tw" | "thr" | "shr" | "chr" | "phr" | "wr"
        )
    };
    // A sung «-le» takes the consonant before its «l» (lit-tle, ta-ble).
    if to + 1 == l.len() && l[to] == 'e' && l[to - 1] == 'l' && units.len() >= 2 {
        return units[units.len() - 2].0;
    }
    match units.len() {
        0 => to,
        1 => {
            let (a, b) = units[0];
            let stays = l[a] == 'x' || (b == a + 2 && l[a] == 'c' && l[a + 1] == 'k');
            if stays { to } else { a }
        }
        n => {
            let last = units[n - 1];
            if onset(&units[n - 2], &last) && !(n == 2 && l[units[0].0] == l[last.0]) {
                units[n - 2].0
            } else {
                last.0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cut(word: &str, rules: Rules) -> Vec<&str> {
        let parts = split(word, rules);
        assert_eq!(parts.concat(), word, "{word}");
        parts
    }

    /// Lines of the bench songs («Мой топор», «Юра, Юра», «Нон стоп»,
    /// «Ханнамонтана», «космоцветок»).
    #[test]
    fn russian_words_are_cut_by_their_vowels() {
        let r = Rules::Cyrillic;
        assert_eq!(cut("космоцветок", r), ["ко", "смо", "цве", "ток"]);
        assert_eq!(cut("топор ", r), ["то", "пор "]);
        assert_eq!(cut("Хитрый ", r), ["Хи", "трый "]);
        assert_eq!(cut("Юра, ", r), ["Ю", "ра, "]);
        assert_eq!(cut("нон-стоп ", r), ["нон-", "стоп "]);
        assert_eq!(cut("Джинсовка ", r), ["Джин", "со", "вка "]);
        assert_eq!(cut("солнце", r), ["солн", "це"]);
        assert_eq!(cut("сильно", r), ["силь", "но"]);
        assert_eq!(cut("семья", r), ["се", "мья"]);
        assert_eq!(cut("майли", r), ["май", "ли"]);
        assert_eq!(cut("контракт", r), ["кон", "тракт"]);
        assert_eq!(cut("сердце", r), ["сер", "дце"]);
        assert_eq!(cut("Анна", r), ["Ан", "на"]);
        assert_eq!(cut("деньги?! ", r), ["день", "ги?! "]);
        assert_eq!(cut("«увидели", r), ["«у", "ви", "де", "ли"]);
        assert_eq!(cut("поэт", r), ["по", "эт"]);
        // One vowel or none: whole.
        for w in ["в ", "мм", "Где ", "стоп"] {
            assert_eq!(cut(w, r), [w]);
        }
        // Two vowels are two syllables, as written.
        assert_eq!(cut("йоу! ", r), ["йо", "у! "]);
        // A held vowel written out is a syllable a letter.
        assert_eq!(cut("а-а-а", r), ["а-", "а-", "а"]);
    }

    #[test]
    fn english_words_drop_a_silent_e() {
        let e = Rules::English;
        assert_eq!(cut("Miley ", e), ["Mi", "ley "]);
        assert_eq!(cut("Cyrus", e), ["Cy", "rus"]);
        assert_eq!(cut("Totally ", e), ["To", "tal", "ly "]);
        assert_eq!(cut("Spies, ", e), ["Spies, "]);
        assert_eq!(cut("Levi's, ", e), ["Le", "vi's, "]);
        assert_eq!(cut("golden ", e), ["gol", "den "]);
        assert_eq!(cut("hour", e), ["hour"]);
        assert_eq!(cut("Natural", e), ["Na", "tu", "ral"]);
        assert_eq!(cut("Imagine ", e), ["I", "ma", "gine "]);
        assert_eq!(cut("time ", e), ["time "]);
        assert_eq!(cut("little", e), ["lit", "tle"]);
        assert_eq!(cut("lonely", e), ["lone", "ly"]);
        assert_eq!(cut("places", e), ["pla", "ces"]);
        assert_eq!(cut("wanted", e), ["wan", "ted"]);
        assert_eq!(cut("loved", e), ["loved"]);
        assert_eq!(cut("yeah", e), ["yeah"]);
        assert_eq!(cut("player", e), ["pla", "yer"]);
        assert_eq!(cut("April", e), ["A", "pril"]);
        assert_eq!(cut("nothing", e), ["no", "thing"]);
        assert_eq!(cut("picking", e), ["pick", "ing"]);
        assert_eq!(cut("quiet", e), ["quiet"]);
        assert_eq!(cut("do-dodge ", e), ["do-", "dodge "]);
        assert_eq!(cut("Swag", e), ["Swag"]);
    }

    #[test]
    fn other_latin_hears_every_vowel_and_kana_stays_whole() {
        let l = Rules::Latin;
        assert_eq!(cut("noche", l), ["no", "che"]);
        assert_eq!(cut("kokoro", l), ["ko", "ko", "ro"]);
        assert_eq!(cut("poeta", l), ["po", "e", "ta"]);
        assert_eq!(cut("ciudad", l), ["ciu", "dad"]);
        assert_eq!(cut("心", Rules::Whole), ["心"]);
        assert_eq!(Rules::of("ありがとう", true), Rules::Whole);
        assert_eq!(Rules::of("Привет,", true), Rules::Cyrillic);
        assert_eq!(Rules::of("Miley", true), Rules::English);
        assert_eq!(Rules::of("Miley", false), Rules::Latin);
        assert_eq!(Rules::of("2024", true), Rules::Whole);
        assert_eq!(cut("2024", Rules::Whole), ["2024"]);
    }
}
