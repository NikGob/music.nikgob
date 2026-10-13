//! Timed lyrics shared by the parsers, the runtime and the UI.
//!
//! A line always has a start; word-synced sources (TTML, NetEase YRC, enhanced LRC)
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
    /// lrclib.net.
    Lrclib,
    /// NetEase Cloud Music (word-synced where it has YRC).
    Netease,
    /// QQ Music.
    Qq,
}

impl LyricsSource {
    pub const ALL: [Self; 4] = [Self::File, Self::Netease, Self::Lrclib, Self::Qq];

    /// Stable key for storage.
    pub fn key(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Lrclib => "lrclib",
            Self::Netease => "netease",
            Self::Qq => "qq",
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
            Self::Netease => "NetEase",
            Self::Qq => "QQ Музыка",
        }
    }
}

/// How precise a text is; better ones win when nothing was picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LyricsSync {
    Plain,
    Line,
    Word,
}

impl LyricsSync {
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "без синхронизации",
            Self::Line => "по строкам",
            Self::Word => "по словам",
        }
    }

    pub fn of(lines: &[LyricLine]) -> Option<Self> {
        if lines.iter().any(LyricLine::is_word_synced) {
            Some(Self::Word)
        } else if lines.is_empty() {
            None
        } else {
            Some(Self::Line)
        }
    }
}
