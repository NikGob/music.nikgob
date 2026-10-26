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
//! modes' business ([`crate::listening`]), not this one's; but a playlist
//! the user plays mostly in their own background sessions
//! ([`background_playlists`]) is a hint for its tracks without words,
//! whatever it is called.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::UnixMs;

use crate::listening::{EXPLICIT_STARTS, Listening};
use crate::songtype::{SongType, Voice};

/// A track suits the background from this score up.
pub const SUITS: f32 = 0.5;
/// A playlist is played in the background with this many plays from it at
/// least ([`EXPLICIT_STARTS`], a plainly background session's)…
pub const PLAYLIST_PLAYS: usize = EXPLICIT_STARTS;
/// …and this mean `p_bg` of their sessions.
pub const PLAYLIST_P_BG: f32 = 0.5;

/// What the rule reads of a track.
#[derive(Clone, Copy, Debug)]
pub struct BackgroundTrack<'a> {
    pub song: &'a SongType,
    pub title: &'a str,
    pub album: Option<&'a str>,
    pub artist: &'a str,
    /// It is in a playlist the user plays in the background
    /// ([`background_playlists`]).
    pub background_playlist: bool,
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
/// …of a film's or a series' (listened to as songs as often as not).
const SCREEN: &[&str] = &["anime", "tv animation", "the movie", "movie", "film", "series", "アニメ", "劇場版", "ドラマ"];

/// The basic rule (see the module docs): no words at all (instrumental,
/// or a soundtrack's track of unknown voice), then ambient or lo-fi words
/// or a playlist played in the background 0.9, a soundtrack of a game or
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
        if has(AMBIENT) || t.background_playlist {
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

/// The playlists the user plays in the background (by the caller's
/// numbers): of `plays` (a playlist and when a play from it started), one
/// with [`PLAYLIST_PLAYS`] plays whose sessions are background by
/// [`PLAYLIST_P_BG`] on the mean ([`Listening::p_at`]). None for a user who
/// never listens so; nothing about a playlist's name.
pub fn background_playlists(plays: &[(usize, UnixMs)], listening: &Listening) -> HashSet<usize> {
    if !listening.has_background {
        return HashSet::new();
    }
    let mut by: HashMap<usize, (usize, f32)> = HashMap::new();
    for &(list, ts) in plays {
        let e = by.entry(list).or_default();
        e.0 += 1;
        e.1 += listening.p_at(ts);
    }
    by.into_iter().filter(|(_, (n, p))| *n >= PLAYLIST_PLAYS && *p / *n as f32 >= PLAYLIST_P_BG).map(|(l, _)| l).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track<'a>(song: &'a SongType, title: &'a str, album: Option<&'a str>) -> BackgroundTrack<'a> {
        BackgroundTrack { song, title, album, artist: "Someone", background_playlist: false }
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
        // A playlist the user plays in the background: its pieces without
        // words, never its songs.
        let kept = |song| BackgroundTrack { background_playlist: true, ..track(song, "Etude", Some("Piano works")) };
        assert!(b.suits(&kept(&inst)));
        assert!(!b.suits(&kept(&sung)));
    }

    /// A playlist played in long untouched sessions is the background's,
    /// whatever it is called; one played among skips and picks is not; a
    /// user who never listens so has none.
    #[test]
    fn a_playlist_played_in_the_background() {
        use crate::listening::Heard;
        use crate::session::Ended;
        const T0: UnixMs = 1_000_000_000_000;
        let mut starts = Vec::new();
        let mut plays = Vec::new();
        let mut at = T0;
        for k in 0..30 {
            let bg = k % 3 == 0;
            let n = if bg { 25 } else { 8 };
            for j in 0..n {
                let ts = at + j as i64 * 200_000;
                let skip = !bg && j % 2 == 1;
                starts.push(Heard {
                    ts,
                    end: Some(ts + if skip { 10_000 } else { 200_000 }),
                    ended: if skip { Ended::Early } else { Ended::Complete },
                    manual: !bg && j % 3 == 1,
                    touches: 0,
                    attention: Some(if bg { 1.0 } else { 0.1 }),
                    duration_ms: Some(200_000),
                });
                // Playlist 0 plays in the long runs, playlist 1 in the
                // others, playlist 2 a little in each.
                plays.push((usize::from(!bg), ts));
                if j == 0 {
                    plays.push((2, ts));
                }
            }
            at += 20 * 3_600_000;
        }
        let (l, _) = Listening::of(&starts);
        assert!(l.has_background);
        assert_eq!(background_playlists(&plays, &l), HashSet::from([0]));
        assert!(background_playlists(&plays, &Listening::default()).is_empty());
    }
}
