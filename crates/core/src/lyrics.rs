//! Timed lyrics shared by the parsers, the runtime and the UI.
//!
//! A line always has a start; word-synced sources (TTML, LyricsPlus, enhanced LRC)
//! also carry the timing of every word (syllable), which lets the view light
//! the line up exactly as it is sung instead of guessing a pace.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// One sung piece of a line: a word or a syllable. `text` keeps its spaces
/// (`"Hel"`, `"lo "`, `"world"`), so joining the words gives the line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LyricWord {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricLine {
    pub start: Duration,
    /// When the line is done; `None` for line-synced LRC (the next line's
    /// start is the best guess there).
    pub end: Option<Duration>,
    /// The whole line. Empty text marks an instrumental break.
    pub text: String,
    /// Word timing; empty for line-synced sources.
    pub words: Vec<LyricWord>,
    /// Background vocals (shown smaller, under the main line).
    pub background: bool,
    /// Sung by the second voice of a duet (aligned to the other side).
    pub duet: bool,
    pub translation: Option<String>,
    pub romanization: Option<String>,
}

impl LyricLine {
    /// A line-synced line.
    pub fn new(start: Duration, text: impl Into<String>) -> Self {
        Self {
            start,
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn is_word_synced(&self) -> bool {
        !self.words.is_empty()
    }
}

/// Where a lyrics text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LyricsSource {
    /// A `.ttml` / `.lrc` next to the file, or the file's own lyrics tag.
    File,
    LyricsPlus,
    LrcRed,
    BetterLyrics,
    /// lrclib.net.
    Lrclib,
    /// genius.com (plain text).
    Genius,
    /// Heard in the recording itself (docs/lyrics-align.md): the last resort.
    Recognized,
}

impl LyricsSource {
    pub const ALL: [Self; 7] =
        [Self::File, Self::LyricsPlus, Self::LrcRed, Self::BetterLyrics, Self::Lrclib, Self::Genius, Self::Recognized];

    /// Stable key for storage.
    pub fn key(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Lrclib => "lrclib",
            Self::LyricsPlus => "lyricsplus",
            Self::LrcRed => "lrcred",
            Self::BetterLyrics => "betterlyrics",
            Self::Genius => "genius",
            Self::Recognized => "recognized",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }

    /// Short name for the UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::File => "Файл",
            Self::Lrclib => "LRCLIB",
            Self::LyricsPlus => "LyricsPlus",
            Self::LrcRed => "lrc.red",
            Self::BetterLyrics => "Better Lyrics",
            Self::Genius => "Genius",
            Self::Recognized => "Распознан",
        }
    }
}

/// How precise a text is; better ones win when nothing was picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LyricsSync {
    Plain,
    Line,
    Word,
    Syllable,
}

impl LyricsSync {
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "без синхронизации",
            Self::Line => "по строкам",
            Self::Word => "по словам",
            Self::Syllable => "по слогам",
        }
    }

    pub fn of(lines: &[LyricLine]) -> Option<Self> {
        if lines.iter().any(|l| l.words.windows(2).any(|w| !w[0].text.ends_with(char::is_whitespace))) {
            Some(Self::Syllable)
        } else if lines.iter().any(LyricLine::is_word_synced) {
            Some(Self::Word)
        } else if lines.is_empty() {
            None
        } else {
            Some(Self::Line)
        }
    }
}

/// Labels a credits line starts with («Written by: …», «Слова: …»,
/// «作词 : …»); the ones marked `true` need a colon after them (the word
/// alone may start a sung line).
const CREDIT_LABELS: &[(&str, bool)] = &[
    ("written by", false),
    ("lyrics by", false),
    ("words by", false),
    ("music by", false),
    ("composed by", false),
    ("produced by", false),
    ("arranged by", false),
    ("mixed by", false),
    ("mastered by", false),
    ("songwriters", true),
    ("songwriter", true),
    ("writers", true),
    ("writer", true),
    ("composers", true),
    ("composer", true),
    ("lyricist", true),
    ("lyrics", true),
    ("producers", true),
    ("producer", true),
    ("слова и музыка", true),
    ("слова", true),
    ("музыка", true),
    ("авторы", true),
    ("автор", true),
    ("текст", true),
    ("композитор", true),
    ("аранжировка", true),
    ("продюсер", true),
    ("作词", true),
    ("作詞", true),
    ("作曲", true),
    ("编曲", true),
    ("編曲", true),
    ("制作人", true),
    ("词", true),
    ("曲", true),
];

/// A line that names who wrote the song rather than being sung: a label
/// («Written by …», «Слова: …», «作曲 : …»), names joined by «/» («Ben
/// McKee/Dan Reynolds/…», each capitalised, one of them a full name), or
/// two or more of `names` (the track's artists and composers) and nothing
/// else.
pub fn is_credit_line(text: &str, names: &[String]) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if labelled_credit(t) {
        return true;
    }
    let lower = t.to_lowercase();
    let parts: Vec<&str> = t.split(['/', '／']).map(str::trim).collect();
    let name_like = |p: &str| {
        let words: Vec<&str> = p.split_whitespace().collect();
        (1..=4).contains(&words.len())
            && words.iter().all(|w| w.chars().next().is_some_and(char::is_uppercase))
            && !p.chars().any(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '!' | '?' | ':' | ';' | '(' | ')'))
    };
    if parts.len() >= 2 && parts.iter().all(|p| name_like(p)) && parts.iter().any(|p| p.split_whitespace().count() >= 2) {
        return true;
    }
    if names.is_empty() {
        return false;
    }
    let key = |s: &str| -> String { s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect() };
    let known: Vec<String> = names.iter().map(|n| key(n)).filter(|n| !n.is_empty()).collect();
    let mut spaced = format!(" {lower} ");
    for sep in [" and ", " и ", " x ", " feat. ", " feat ", " ft. "] {
        spaced = spaced.replace(sep, ",");
    }
    let parts: Vec<String> = spaced.split([',', '&', '/', ';', '、', '，']).map(key).filter(|p| !p.is_empty()).collect();
    parts.len() >= 2 && parts.iter().all(|p| known.contains(p))
}

/// A line that starts with a credits label ([`CREDIT_LABELS`]): a colon
/// after it, or a name after a «… by».
fn labelled_credit(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    CREDIT_LABELS.iter().any(|(label, colon)| {
        lower.strip_prefix(label).is_some_and(|rest| {
            let rest = rest.trim_start();
            rest.starts_with([':', '：']) || (!colon && rest.chars().next().is_some_and(char::is_alphabetic) && rest.len() < lower.len())
        })
    })
}

/// Drops the credits lines of a fetched text ([`is_credit_line`]) before
/// its first sung line (and labelled ones after its last), when sung
/// lines remain. Returns how many went.
pub fn drop_credits(lines: &mut Vec<LyricLine>, names: &[String]) -> usize {
    let before = lines.len();
    let sung = |l: &LyricLine| !l.text.trim().is_empty();
    let lead = lines.iter().take_while(|l| !sung(l) || is_credit_line(&l.text, names)).count();
    let tail = lines
        .iter()
        .rev()
        .take_while(|l| !sung(l) || labelled_credit(&l.text))
        .count();
    let credits = |r: &[LyricLine]| r.iter().any(sung);
    if lead < lines.len() && credits(&lines[..lead]) {
        lines.drain(..lead);
    }
    let tail = tail.min(lines.len());
    let from = lines.len() - tail;
    if from > 0 && lines[from..].iter().any(|l| sung(l) && labelled_credit(&l.text)) {
        lines.truncate(from);
    }
    before - lines.len()
}

/// [`drop_credits`] for a plain text, a line each.
pub fn drop_credit_text(text: &str, names: &[String]) -> String {
    let mut lines: Vec<LyricLine> = text.lines().map(|l| LyricLine::new(Duration::ZERO, l)).collect();
    if drop_credits(&mut lines, names) == 0 {
        return text.to_owned();
    }
    lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("
").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LyricsPlus' «Take Me to the Beach» opens with its songwriters.
    #[test]
    fn songwriter_lines_are_not_lyrics() {
        let names = vec!["Imagine Dragons".to_owned(), "Ado".to_owned()];
        let mut lines = vec![
            LyricLine::new(Duration::from_millis(2246), "Ben McKee/Dan Reynolds/Mattias Larsson/Robin Lennart Fredriksson/Wayne Sermon/Matteo Professione"),
            LyricLine::new(Duration::from_millis(9435), "People pleasing planet"),
            LyricLine::new(Duration::from_millis(11436), "Got a million people saying how to plan it"),
        ];
        assert_eq!(drop_credits(&mut lines, &names), 1);
        assert_eq!(lines[0].text, "People pleasing planet");
        for credit in ["Written by Dan Reynolds", "Lyrics by: Мария", "Слова: Иван Петров", "作词 : 米津玄師", "Музыка: А. Б.", "Imagine Dragons & Ado"] {
            assert!(is_credit_line(credit, &names), "{credit}");
        }
        for sung in ["Lyrics of my life", "Слова не нужны", "Yes/No", "AC/DC on the radio, oh/yeah", "Imagine Dragons", "Музыка громче", "I'm writing words by the sea"] {
            assert!(!is_credit_line(sung, &names), "{sung}");
        }
        // A text of credits only keeps them; a credit after the last line
        // goes when it is labelled.
        let mut only = vec![LyricLine::new(Duration::ZERO, "Written by Somebody")];
        assert_eq!(drop_credits(&mut only, &[]), 0);
        let mut tail = vec![LyricLine::new(Duration::ZERO, "La la la"), LyricLine::new(Duration::ZERO, ""), LyricLine::new(Duration::ZERO, "作曲 : 某人")];
        assert_eq!(drop_credits(&mut tail, &[]), 2);
        assert_eq!(drop_credit_text("Written by: X
Первая строка
Вторая", &[]), "Первая строка
Вторая");
    }

    #[test]
    fn sync_detects_glued_pieces_and_orders_precision() {
        assert_eq!(LyricsSync::of(&[]), None);
        let mut l = LyricLine::new(Duration::ZERO, "Hello world");
        assert_eq!(LyricsSync::of(&[l.clone()]), Some(LyricsSync::Line));
        l.words = ["Hello ", "world"].map(|text| LyricWord { text: text.into(), ..Default::default() }).to_vec();
        assert_eq!(LyricsSync::of(&[l.clone()]), Some(LyricsSync::Word));
        l.words = ["Hel", "lo ", "world"].map(|text| LyricWord { text: text.into(), ..Default::default() }).to_vec();
        assert_eq!(LyricsSync::of(&[l]), Some(LyricsSync::Syllable));
        assert!(LyricsSync::Plain < LyricsSync::Line);
        assert!(LyricsSync::Line < LyricsSync::Word);
        assert!(LyricsSync::Word < LyricsSync::Syllable);
        assert_eq!(LyricsSync::Syllable.label(), "по слогам");
    }
}
