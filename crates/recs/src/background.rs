//! Whether a track suits listening in the background: under it one can
//! think, read, work.
//!
//! Everything that needs to know — the background cards of the home and
//! their mixes, the radio of a background session, what new music «does
//! not get in the way» — asks [`BackgroundScore`] and nothing else, so the
//! full design (`background-v1`: the evenness of the sound and the user's
//! own behaviour, per user) replaces the [`Basic`] rule here without
//! touching them.
//!
//! The basic rule (the user's decision of 2026-10-26): ambient, lo-fi or a
//! game's soundtrack, without lyrics. A song with words is not background
//! however calm, nor is any track without words as such: a soundtrack of a
//! film or a series is often listened to as a song, so it stands below the
//! line. What the user really plays in the background is the listening
//! modes' business ([`crate::listening`]), not this one's; a playlist the
//! user named for it («Background music», «Для работы») is a hint all the
//! same, for its tracks without words.

use crate::songtype::{SongType, Voice};

/// A track suits the background from this score up.
pub const SUITS: f32 = 0.5;

/// What the rule reads of a track.
#[derive(Clone, Copy, Debug)]
pub struct BackgroundTrack<'a> {
    pub song: &'a SongType,
    pub title: &'a str,
    pub album: Option<&'a str>,
    pub artist: &'a str,
    /// The names of the user's playlists it is in.
    pub playlists: &'a [&'a str],
}

/// How well a track suits the background, 0…1 (see the module docs).
pub trait BackgroundScore {
    fn score(&self, t: &BackgroundTrack<'_>) -> f32;

    fn suits(&self, t: &BackgroundTrack<'_>) -> bool {
        self.score(t) >= SUITS
    }
}

/// Words of a title, an album or an artist of music made to stay in the
/// background.
const AMBIENT: &[&str] = &[
    "ambient", "lo-fi", "lofi", "lo fi", "chillhop", "chill", "study", "sleep", "relax", "relaxing", "meditation",
    "calm", "drone", "focus",
];
/// …of a game's soundtrack.
const GAME: &[&str] = &["game", "video game", "gamerip", "bgm", "ゲーム"];
/// …of a playlist the user keeps for the background.
const PLAYLIST: &[&str] = &[
    "background", "bgm", "work", "working", "study", "focus", "concentration", "sleep", "chill", "relax", "ambient",
    "lofi", "lo-fi", "фон", "фоновая", "фоновое", "фоновые", "работа", "работы", "учёба", "учёбы", "учеба", "учебы",
    "сон", "сна", "фокус", "спокойное",
];
/// …of a film's or a series' (listened to as songs as often as not).
const SCREEN: &[&str] = &["anime", "tv animation", "the movie", "movie", "film", "series", "アニメ", "劇場版", "ドラマ"];

/// The basic rule (see the module docs): no words at all (instrumental,
/// or a soundtrack's track of unknown voice), then ambient or lo-fi words
/// or a playlist kept for the background 0.9, a soundtrack of a game or
/// of no screen named 0.6, of a film or a series 0.3, any other
/// instrumental 0.2; anything with words 0.
#[derive(Clone, Copy, Debug, Default)]
pub struct Basic;

impl BackgroundScore for Basic {
    fn score(&self, t: &BackgroundTrack<'_>) -> f32 {
        let s = t.song;
        let wordless = s.voice == Voice::Instrumental || (s.ost && s.voice == Voice::Unknown);
        if !wordless || s.theme.is_some() {
            return 0.0;
        }
        let text = format!(" {} {} {} ", t.title, t.album.unwrap_or_default(), t.artist).to_lowercase();
        let words: String = text.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { ' ' }).collect();
        let words = format!(" {} ", words.split_whitespace().collect::<Vec<_>>().join(" "));
        let has = |list: &[&str]| list.iter().any(|w| if w.is_ascii() { words.contains(&format!(" {w} ")) } else { text.contains(w) });
        let kept = t.playlists.iter().any(|name| {
            let name = name.to_lowercase();
            let words: Vec<&str> = name.split(|c: char| !(c.is_alphanumeric() || c == '-')).filter(|w| !w.is_empty()).collect();
            PLAYLIST.iter().any(|w| words.contains(w))
        });
        if has(AMBIENT) || kept {
            0.9
        } else if s.ost && has(SCREEN) && !has(GAME) {
            0.3
        } else if s.ost || has(GAME) {
            0.6
        } else {
            0.2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track<'a>(song: &'a SongType, title: &'a str, album: Option<&'a str>) -> BackgroundTrack<'a> {
        BackgroundTrack { song, title, album, artist: "Someone", playlists: &[] }
    }

    /// Lo-fi and a game's soundtrack without words suit the background; a
    /// calm song with words, a theme song, a plain instrumental and an
    /// anime's score do not.
    #[test]
    fn the_basic_rule() {
        let inst = SongType { voice: Voice::Instrumental, ..SongType::default() };
        let ost = SongType { voice: Voice::Unknown, ost: true, ..SongType::default() };
        let sung = SongType { voice: Voice::Sung, ..SongType::default() };
        let b = Basic;
        assert!(b.suits(&track(&inst, "sleepless at midnight", Some("Lofi Girl - chill beats to study to"))));
        assert!(b.suits(&track(&ost, "Greenpath", Some("Hollow Knight (Original Soundtrack)"))));
        assert!(b.suits(&track(&inst, "Silent Gauntlet", Some("Big Walk Game Soundtrack"))));
        assert!(!b.suits(&track(&ost, "Hanezeve Caradhina", Some("TVアニメ「メイドインアビス」オリジナルサウンドトラック"))));
        assert!(!b.suits(&track(&ost, "Theme", Some("Made in Abyss Anime OST"))));
        assert!(!b.suits(&track(&sung, "a calm song", Some("Lofi songs to relax"))));
        assert!(!b.suits(&track(&inst, "Etude", Some("Piano works"))));
        let theme = SongType { theme: Some(crate::songtype::Theme::Opening), ..ost };
        assert!(!b.suits(&track(&theme, "Opening", Some("Hollow Knight OST"))));
        assert!(!b.suits(&track(&SongType::default(), "Ambient piece", None)), "the voice unknown: words may be in it");
        // A playlist the user keeps for the background: its pieces without
        // words, never its songs.
        let kept = |song| BackgroundTrack { playlists: &["Background music", "Любимое"], ..track(song, "Etude", Some("Piano works")) };
        assert!(b.suits(&kept(&inst)));
        assert!(b.suits(&BackgroundTrack { playlists: &["Для работы"], ..track(&inst, "Etude", None) }));
        assert!(!b.suits(&kept(&sung)));
        assert!(!b.suits(&BackgroundTrack { playlists: &["Workout", "Фонтан"], ..track(&inst, "Etude", None) }), "words, not parts of words");
    }
}
