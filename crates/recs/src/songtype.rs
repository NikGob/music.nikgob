//! «Тип песни» (recs-v2 §10): what a track is beyond its sound — sung or
//! instrumental, in which language, a soundtrack's song or not.
//!
//! * **Voice**, the first argument that holds: lyrics of 12 letters or more
//!   are sung; LRCLIB's instrumental mark is instrumental; else the sound
//!   says it ([`VoiceProbe`]: a logistic regression on the 23 z-scores of
//!   bliss, trained on the library's own labels, AUC 0.94 offline): above
//!   [`SUNG_P`] sung, below [`INSTRUMENTAL_P`] instrumental; else a
//!   soundtrack's track is instrumental (confidence 0.6) and the rest
//!   unknown.
//! * **Language**, unless instrumental: the lyrics' own, by the script most
//!   of their letters are in (an odd Cyrillic «е» in English lyrics is no
//!   Russian), Latin letters by their words (romanised Japanese by its
//!   syllables and particles, the rest by `whatlang`); else the language
//!   most of the artist's lyrics are in; else what the title says plainly
//!   («RUS COVER», kana, Cyrillic). A Latin title alone says nothing: the
//!   language stays unknown, which is no English.
//! * **Soundtrack** and its **theme songs**: the album is a franchise's;
//!   a sung track of it is an opening, an ending, an insert or a character
//!   song by its album and title words.
//!
//! `whatlang` stays the detector of Latin lyrics: `lingua` is surer on short
//! texts, but its models of even the seven languages wanted add megabytes
//! to the binary, and lyrics are long enough for `whatlang`.

use std::collections::HashMap;

use nikgob_core::model::{ArtistId, TrackId};
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::mood::Axes;
use crate::tiles::{Script, script_of};

/// Bumped whenever a rule here changes: what was classified by older rules
/// (and the day's lanes picked by them) is classified again.
pub const SONGTYPE_RULES: u32 = 2;
/// The sound says sung above this…
pub const SUNG_P: f32 = 0.85;
/// …and instrumental below this.
pub const INSTRUMENTAL_P: f32 = 0.15;
/// Lyrics need this many letters to count.
const LYRICS_LETTERS: usize = 12;
/// Lyrics in Latin letters need this many words to tell their language …
const LATIN_WORDS: usize = 6;
/// … and the detector this much confidence.
const LATIN_CONFIDENCE: f64 = 0.5;
/// Japanese lyrics: at least this share of the letters kana or kanji.
const JAPANESE_SHARE: f32 = 0.25;

/// Uploaders whose name stands where the artist's should (a channel of
/// anime openings): they cast no vote for a language.
const UPLOADERS: &[&str] = &["crunchyroll", "funimation", "aniplex", "muse asia", "ani-one asia", "various artists"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Voice {
    Sung,
    Instrumental,
    Unknown,
}

/// What told the voice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoiceSource {
    Lyrics,
    LrclibFlag,
    Sound,
    /// A soundtrack's track without lyrics.
    Soundtrack,
    Nothing,
}

/// What a song is sung in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Lang {
    Russian,
    Japanese,
    Korean,
    English,
    Spanish,
    French,
    German,
    /// Any other.
    Other,
}

/// What told the language.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LangSource {
    Lyrics,
    ArtistMajority,
    Title,
}

/// A soundtrack's sung track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    Opening,
    Ending,
    /// Sung in the story, or a theme song collection's track.
    Insert,
    CharacterSong,
}

/// The type of one track.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SongType {
    pub voice: Voice,
    /// How sure the voice is (0…1).
    pub voice_conf: f32,
    pub voice_by: VoiceSource,
    pub lang: Option<Lang>,
    pub lang_by: Option<LangSource>,
    /// On a franchise's album.
    pub ost: bool,
    pub theme: Option<Theme>,
    /// The percentile of its energy in the library (0.5 without sound).
    pub energy_q: f32,
    /// The cell of energy × brightness × steadiness (each above or below
    /// the library's middle), as the daylist's moods see it.
    pub mood_cell: u8,
    pub synced_lyrics: bool,
}

impl Default for SongType {
    fn default() -> Self {
        Self {
            voice: Voice::Unknown,
            voice_conf: 0.0,
            voice_by: VoiceSource::Nothing,
            lang: None,
            lang_by: None,
            ost: false,
            theme: None,
            energy_q: 0.5,
            mood_cell: 0,
            synced_lyrics: false,
        }
    }
}

/// The lane a song type falls in for the session's «same lane» (recs-v2
/// §5.2): instrumental (or a soundtrack's track of unknown voice), a sung
/// language, or unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SongLane {
    Instrumental,
    Russian,
    Japanese,
    Western,
    Unknown,
}

impl SongType {
    pub fn lane(&self) -> SongLane {
        if self.voice == Voice::Instrumental || (self.ost && self.voice == Voice::Unknown) {
            return SongLane::Instrumental;
        }
        match self.lang {
            Some(Lang::Russian) => SongLane::Russian,
            Some(Lang::Japanese) => SongLane::Japanese,
            Some(Lang::English | Lang::Spanish | Lang::French | Lang::German) => SongLane::Western,
            _ => SongLane::Unknown,
        }
    }

    /// Sung, as far as anything tells.
    pub fn vocal(&self) -> bool {
        self.voice == Voice::Sung
    }
}

// ---------------------------------------------------------------- voice

/// Sung or instrumental by the sound: a logistic regression on the
/// z-scores of the bliss features ([`crate::index::Normalizer::z`]).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceProbe {
    /// The bias first.
    w: [f32; DIM + 1],
}

impl VoiceProbe {
    const EPOCHS: usize = 300;
    const LR: f32 = 0.1;
    const L2: f32 = 0.01;

    /// Trained on `rows` (z-scores, sung) as `scripts/vocal_probe.py` does:
    /// full-batch gradient ascent, 300 epochs. `None` without both kinds.
    pub fn train(rows: &[(Vector, bool)]) -> Option<Self> {
        if !rows.iter().any(|r| r.1) || !rows.iter().any(|r| !r.1) {
            return None;
        }
        let mut w = [0.0f32; DIM + 1];
        let n = rows.len() as f32;
        for _ in 0..Self::EPOCHS {
            let mut g = [0.0f32; DIM + 1];
            for (x, y) in rows {
                let p = sigmoid(w[0] + x.iter().zip(&w[1..]).map(|(a, b)| a * b).sum::<f32>());
                let d = f32::from(u8::from(*y)) - p;
                g[0] += d;
                for (gi, xi) in g[1..].iter_mut().zip(x) {
                    *gi += d * xi;
                }
            }
            for (wi, gi) in w.iter_mut().zip(g) {
                *wi += Self::LR * (gi / n - Self::L2 * *wi);
            }
        }
        Some(Self { w })
    }

    /// The chance a track of these z-scores is sung.
    pub fn p(&self, z: &Vector) -> f32 {
        sigmoid(self.w[0] + z.iter().zip(&self.w[1..]).map(|(a, b)| a * b).sum::<f32>())
    }
}

pub(crate) fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-30.0, 30.0)).exp())
}

// ---------------------------------------------------------------- language

/// Whether a word is spelt like romanised Japanese: syllables of an
/// optional onset and a vowel, «n», or a doubled consonant.
pub fn romaji_word(w: &str) -> bool {
    const ONSETS: [&str; 30] = [
        "ky", "gy", "sh", "ch", "ts", "ny", "hy", "by", "py", "my", "ry", "j", "k", "g", "s", "z", "t", "d", "n", "h",
        "b", "p", "m", "y", "r", "w", "f", "", "", "",
    ];
    let b = w.as_bytes();
    let mut i = 0;
    if b.is_empty() {
        return false;
    }
    'next: while i < b.len() {
        // A doubled consonant («kk», «tt»): the first is a pause.
        if i + 1 < b.len() && b[i] == b[i + 1] && b"kstpgdbcmz".contains(&b[i]) {
            i += 1;
            continue;
        }
        for o in ONSETS {
            let o = o.as_bytes();
            if b[i..].starts_with(o) && b.get(i + o.len()).is_some_and(|c| b"aiueo".contains(c)) {
                i += o.len() + 1;
                continue 'next;
            }
        }
        if b[i] == b'n' {
            i += 1;
            continue;
        }
        return false;
    }
    true
}

/// Whether Latin lyrics are romanised Japanese: nearly every word spelt
/// so, and its particles among them.
fn romaji(words: &[String]) -> bool {
    const PARTICLES: [&str; 11] = ["wa", "no", "ga", "wo", "ni", "de", "to", "mo", "yo", "ne", "ka"];
    let shaped = words.iter().filter(|w| romaji_word(w)).count();
    let particles = words.iter().filter(|w| PARTICLES.contains(&w.as_str())).count();
    shaped * 100 >= words.len() * 85 && particles >= 2
}

/// The language of lyrics in Latin letters, when they say it clearly.
pub fn latin_language(text: &str) -> Option<Lang> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic() && c != '\'')
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.len() < LATIN_WORDS {
        return None;
    }
    if romaji(&words) {
        return Some(Lang::Japanese);
    }
    match whatlang::detect(text) {
        Some(info) if info.is_reliable() || info.confidence() >= LATIN_CONFIDENCE => Some(match info.lang() {
            whatlang::Lang::Eng => Lang::English,
            whatlang::Lang::Rus => Lang::Russian,
            whatlang::Lang::Jpn => Lang::Japanese,
            whatlang::Lang::Spa => Lang::Spanish,
            whatlang::Lang::Fra => Lang::French,
            whatlang::Lang::Deu => Lang::German,
            _ => Lang::Other,
        }),
        // A line or two the detector is unsure of: English by its little
        // words («my», «the», «in»).
        _ => english_words(&words).then_some(Lang::English),
    }
}

/// Whether a quarter of the words, two at least, are English little words.
fn english_words(words: &[String]) -> bool {
    const LITTLE: [&str; 40] = [
        "the", "i", "you", "my", "in", "and", "to", "of", "is", "it", "me", "we", "on", "for", "that", "your", "be",
        "all", "with", "what", "this", "but", "are", "was", "can", "just", "don't", "i'm", "it's", "know", "when",
        "never", "they", "our", "his", "her", "from", "at", "will", "have",
    ];
    let hits = words.iter().filter(|w| LITTLE.contains(&w.as_str())).count();
    hits >= 2 && hits * 4 >= words.len()
}

/// What a title says of its song's language plainly: a marker («RUS
/// COVER», «Japanese ver.»), else its script — Cyrillic, kana, kanji or
/// Japanese punctuation, hangul. A Latin title says nothing.
pub fn title_language(text: &str) -> Option<Lang> {
    const RUSSIAN: [&str; 12] = [
        "rus cover", "ru cover", "russian cover", "(russian)", "[russian]", "на русском", "rus ver", "rus.ver",
        "russian ver", "(rus)", "[rus]", "кавер на русском",
    ];
    const JAPANESE: [&str; 7] = ["japanese ver", "jp ver", "jpn ver", "jp.ver", "japanese version", "(japanese)", "[japanese]"];
    let lower = text.to_lowercase();
    if RUSSIAN.iter().any(|m| lower.contains(m)) {
        return Some(Lang::Russian);
    }
    if JAPANESE.iter().any(|m| lower.contains(m)) {
        return Some(Lang::Japanese);
    }
    match script_of(text)? {
        Script::Cyrillic => Some(Lang::Russian),
        Script::Japanese => Some(Lang::Japanese),
        Script::Korean => Some(Lang::Korean),
        Script::Latin => None,
    }
}

/// The words of a lyrics text as kept: LRC without its time tags, an
/// online source's JSON lines by their `"text"` values, TTML without tags.
pub fn lyric_words(raw: &str) -> String {
    let raw = raw.trim_start();
    let mut out = String::new();
    if raw.starts_with('[') && raw.contains("\"text\":\"") {
        for part in raw.split("\"text\":\"").skip(1) {
            if let Some(end) = part.find('"') {
                out.push_str(&part[..end]);
                out.push(' ');
            }
        }
        return out;
    }
    let mut depth_sq = 0;
    let mut depth_angle = 0;
    for c in raw.chars() {
        match c {
            '[' => depth_sq += 1,
            ']' => depth_sq = (depth_sq - 1).max(0),
            '<' => depth_angle += 1,
            '>' => depth_angle = (depth_angle - 1).max(0),
            _ if depth_sq == 0 && depth_angle == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The script most of the letters of lyrics are in: Japanese once a
/// quarter of them is kana or kanji (Japanese lyrics mix in English words),
/// else the commonest. One letter of another script (a Cyrillic «е» for a
/// Latin «e») changes nothing.
fn lyrics_script(words: &str) -> Option<Script> {
    let mut count: HashMap<Script, usize> = HashMap::new();
    let mut buf = [0u8; 4];
    for c in words.chars().filter(|c| c.is_alphabetic() || matches!(*c as u32, 0x3040..=0x30FF | 0x4E00..=0x9FFF)) {
        if let Some(s) = script_of(c.encode_utf8(&mut buf)) {
            *count.entry(s).or_default() += 1;
        }
    }
    let total: usize = count.values().sum();
    if total == 0 {
        return None;
    }
    if count.get(&Script::Japanese).is_some_and(|&n| n as f32 >= JAPANESE_SHARE * total as f32) {
        return Some(Script::Japanese);
    }
    count.into_iter().max_by_key(|(s, n)| (*n, *s as u8)).map(|(s, _)| s)
}

/// The language of a lyrics text, if it has enough letters: by the script
/// most of them are in, and Latin letters by the words themselves.
pub fn lyrics_language(raw: &str) -> Option<Lang> {
    let words = lyric_words(raw);
    if words.chars().filter(|c| c.is_alphabetic()).count() < LYRICS_LETTERS {
        return None;
    }
    match lyrics_script(&words)? {
        Script::Cyrillic => Some(Lang::Russian),
        Script::Japanese => Some(Lang::Japanese),
        Script::Korean => Some(Lang::Korean),
        Script::Latin => latin_language(&words),
    }
}

// ---------------------------------------------------------------- the library

/// What [`song_types`] reads of one track.
#[derive(Clone, Debug, Default)]
pub struct SongInput {
    pub id: TrackId,
    pub main_artist: Option<ArtistId>,
    /// The main artist's name (an uploader's channel votes for no language).
    pub artist_name: String,
    pub title: String,
    /// The credit as shown.
    pub credit: String,
    pub album: Option<String>,
    /// On a franchise's album.
    pub ost: bool,
    /// The lyrics row: LRCLIB's instrumental mark and the start of a text.
    pub lyrics: Option<(bool, String)>,
    pub synced_lyrics: bool,
    /// Its bliss vector as z-scores.
    pub z: Option<Vector>,
}

/// The types of a library's tracks, and what tells the language of a new
/// song of a service.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SongTypes {
    pub by_track: HashMap<TrackId, SongType>,
    /// The language most lyrics of an artist are in, by the artist's name
    /// in lower case.
    pub artist_langs: HashMap<String, Lang>,
}

impl SongTypes {
    pub fn get(&self, id: TrackId) -> Option<&SongType> {
        self.by_track.get(&id)
    }

    /// A new song of a service: its artist's language, else its title's.
    pub fn lang_of_new(&self, title: &str, artists: &[&str]) -> Option<Lang> {
        artists
            .iter()
            .find_map(|a| self.artist_langs.get(&a.to_lowercase()).copied())
            .or_else(|| title_language(&format!("{title} {}", artists.join(" "))))
    }
}

/// A soundtrack's sung track: what kind of theme its album and title say.
fn theme_of(album: Option<&str>, title: &str, credit: &str) -> Theme {
    let has_word = |text: &str, word: &str| {
        text.split(|c: char| !c.is_alphanumeric()).any(|w| w.eq_ignore_ascii_case(word))
    };
    if credit.contains("CV:") || credit.contains("CV：") || credit.contains("(CV") {
        return Theme::CharacterSong;
    }
    if has_word(title, "opening") || title.contains("オープニング") {
        return Theme::Opening;
    }
    if has_word(title, "ending") || title.contains("エンディング") {
        return Theme::Ending;
    }
    let album = album.unwrap_or_default().to_lowercase();
    if album.contains("character song") {
        return Theme::CharacterSong;
    }
    Theme::Insert
}

/// The type of every track of `inputs` (see the module docs).
pub fn song_types(inputs: &[SongInput]) -> SongTypes {
    // The lyrics' own languages and the artists' majorities.
    let sung: HashMap<TrackId, Lang> = inputs
        .iter()
        .filter_map(|t| match &t.lyrics {
            Some((false, text)) => Some((t.id, lyrics_language(text)?)),
            _ => None,
        })
        .collect();
    let mut votes: HashMap<ArtistId, HashMap<Lang, u32>> = HashMap::new();
    let mut names: HashMap<ArtistId, String> = HashMap::new();
    for t in inputs {
        let (Some(a), Some(l)) = (t.main_artist, sung.get(&t.id)) else { continue };
        let name = t.artist_name.to_lowercase();
        if UPLOADERS.contains(&name.as_str()) {
            continue;
        }
        *votes.entry(a).or_default().entry(*l).or_default() += 1;
        names.entry(a).or_insert(name);
    }
    let artist: HashMap<ArtistId, Lang> = votes
        .into_iter()
        .filter_map(|(a, v)| v.into_iter().max_by_key(|(l, n)| (*n, std::cmp::Reverse(*l))).map(|(l, _)| (a, l)))
        .collect();
    let artist_langs: HashMap<String, Lang> = artist.iter().filter_map(|(a, l)| Some((names.get(a)?.clone(), *l))).collect();

    // The sound's probe, on the library's own labels.
    let has_text = |t: &SongInput| {
        matches!(&t.lyrics, Some((false, text)) if lyric_words(text).chars().filter(|c| c.is_alphabetic()).count() >= LYRICS_LETTERS)
    };
    let rows: Vec<(Vector, bool)> = inputs
        .iter()
        .filter_map(|t| {
            let z = t.z?;
            match &t.lyrics {
                Some((true, _)) => Some((z, false)),
                _ if has_text(t) => Some((z, true)),
                _ => None,
            }
        })
        .collect();
    let probe = VoiceProbe::train(&rows);

    // Energy percentiles and the middle of the axes.
    let axes: Vec<(TrackId, Axes)> = inputs.iter().filter_map(|t| Some((t.id, Axes::of(&t.z?)))).collect();
    let mut energies: Vec<f32> = axes.iter().map(|(_, a)| a.energy).collect();
    energies.sort_by(f32::total_cmp);
    let median = |f: fn(&Axes) -> f32| {
        let mut v: Vec<f32> = axes.iter().map(|(_, a)| f(a)).collect();
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let mid = [median(|a| a.energy), median(|a| a.brightness), median(|a| a.steadiness)];
    let axes: HashMap<TrackId, Axes> = axes.into_iter().collect();

    let mut by_track = HashMap::with_capacity(inputs.len());
    for t in inputs {
        let mut ty = SongType { ost: t.ost, synced_lyrics: t.synced_lyrics, ..SongType::default() };
        if let Some(a) = axes.get(&t.id) {
            let below = energies.partition_point(|e| *e < a.energy);
            ty.energy_q = if energies.len() > 1 { below as f32 / (energies.len() - 1) as f32 } else { 0.5 };
            ty.mood_cell = u8::from(a.energy >= mid[0]) | u8::from(a.brightness >= mid[1]) << 1 | u8::from(a.steadiness >= mid[2]) << 2;
        }
        let sound = probe.zip(t.z).map(|(p, z)| p.p(&z));
        (ty.voice, ty.voice_conf, ty.voice_by) = if has_text(t) {
            (Voice::Sung, 1.0, VoiceSource::Lyrics)
        } else if matches!(t.lyrics, Some((true, _))) {
            (Voice::Instrumental, 1.0, VoiceSource::LrclibFlag)
        } else if let Some(p) = sound.filter(|p| *p > SUNG_P || *p < INSTRUMENTAL_P) {
            let voice = if p > SUNG_P { Voice::Sung } else { Voice::Instrumental };
            (voice, (p - 0.5).abs() * 2.0, VoiceSource::Sound)
        } else if t.ost {
            (Voice::Instrumental, 0.6, VoiceSource::Soundtrack)
        } else {
            (Voice::Unknown, 0.0, VoiceSource::Nothing)
        };
        if ty.voice != Voice::Instrumental {
            let found = sung
                .get(&t.id)
                .map(|l| (*l, LangSource::Lyrics))
                .or_else(|| t.main_artist.and_then(|a| artist.get(&a)).map(|l| (*l, LangSource::ArtistMajority)))
                .or_else(|| title_language(&format!("{} {}", t.title, t.credit)).map(|l| (l, LangSource::Title)));
            if let Some((l, by)) = found {
                ty.lang = Some(l);
                ty.lang_by = Some(by);
            }
        }
        if t.ost && ty.voice == Voice::Sung {
            ty.theme = Some(theme_of(t.album.as_deref(), &t.title, &t.credit));
        }
        by_track.insert(t.id, ty);
    }
    SongTypes { by_track, artist_langs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;
    use crate::test_util::*;

    #[test]
    fn lyrics_tell_the_language_whatever_the_format() {
        let lrc = "[00:12.34]Город в огне, и мы идём домой\n[00:15.00]По ночным дворам";
        assert_eq!(lyrics_language(lrc), Some(Lang::Russian));
        let json = r#"[{"start":{"secs":0,"nanos":5},"text":"We walked along the river in the rain","words":[]}]"#;
        assert_eq!(lyrics_language(json), Some(Lang::English));
        let ja = "[00:01.00]君の名前を呼んでいた 夜の向こうで";
        assert_eq!(lyrics_language(ja), Some(Lang::Japanese));
        assert_eq!(lyric_words(r#"[{"text":"Да","words":[]}]"#).trim(), "Да");
        assert_eq!(lyrics_language("[00:01.00]"), None);
    }

    /// AJR's «World's Smallest Violin» on LRCLIB spells one «yet» with a
    /// Cyrillic «е»: the lyrics are English all the same.
    #[test]
    fn one_cyrillic_letter_makes_no_russian_song() {
        let lrc = "[00:04.35] We left the lights on in the kitchen\n[00:07.51] And the radio was playing low\n\
                   [00:09.45] Nobody wanted to go home\n[00:11.20] Is it over y\u{0435}t?\n\
                   [01:28.35] (Oh) we stayed until the morning";
        assert_eq!(lyrics_language(lrc), Some(Lang::English));
        // Japanese lyrics with English lines are Japanese.
        let mixed = "[00:01]君の名前を呼んでいた夜の向こうで\n[00:05]I will be there for you\n[00:09]ずっとここにいるよ";
        assert_eq!(lyrics_language(mixed), Some(Lang::Japanese));
    }

    #[test]
    fn romaji_is_japanese() {
        let romaji = "[00:01.00]kimi no na wo yonde ita yoru no mukou de\n[00:05.00]boku wa mada koko ni iru yo kimi ga inakute mo\n[00:09.00]sora ga naite iru kara namida wo misenai";
        assert_eq!(lyrics_language(romaji), Some(Lang::Japanese));
        let de = "[00:01.00]Ich gehe durch die Straßen und denke nur an dich\n[00:05.00]Die Nacht ist kalt und meine Freundin ist nicht hier";
        assert_eq!(lyrics_language(de), Some(Lang::German));
        let en = "[00:01.00]I walk along the streets and only think of you\n[00:05.00]The night is cold and my heart is not here with you";
        assert_eq!(lyrics_language(en), Some(Lang::English));
        assert_eq!(latin_language("hello there my friend"), None);
        assert!(romaji_word("kimi") && romaji_word("shinjitsu") && romaji_word("kitto") && romaji_word("n"));
        assert!(!romaji_word("the") && !romaji_word("love") && !romaji_word("street"));
    }

    /// A Latin title alone tells nothing: without lyrics or an artist's
    /// language a song stays unknown, never English.
    #[test]
    fn unknown_is_not_english() {
        let t = |i: u128, title: &str, credit: &str| SongInput {
            id: tid(i),
            title: title.into(),
            credit: credit.into(),
            ..SongInput::default()
        };
        let types = song_types(&[
            t(0, "Meine Freundin ist kaputt", "Con Fetti Pesto"),
            t(1, "There is a reason (RUS COVER)", "j.am"),
            t(2, "A Horny Money World 〜legendary night〜", "Houshou Marine"),
            SongInput { lyrics: Some((false, "[00:10]I keep on walking in the circles that I made\n[00:14]And every time I turn around the road is gone".into())), ..t(3, "うずまき", "Machine Girl") },
        ]);
        let lang = |i| types.get(tid(i)).unwrap().lang;
        assert_eq!(lang(0), None);
        assert_eq!(types.get(tid(0)).unwrap().lane(), SongLane::Unknown);
        assert_eq!(lang(1), Some(Lang::Russian));
        assert_eq!(lang(2), Some(Lang::Japanese));
        assert_eq!(lang(3), Some(Lang::English), "the lyrics win over a Japanese title");
    }

    /// The artist's language covers their songs without lyrics; a
    /// soundtrack's track without lyrics or a clear sound is instrumental.
    #[test]
    fn artists_and_soundtracks_fill_the_gaps() {
        let ru = "[00:01]Город в огне, и мы идём домой по ночным дворам";
        let inputs = vec![
            SongInput { id: tid(0), main_artist: Some(aid(1)), artist_name: "Кино".into(), lyrics: Some((false, ru.into())), ..SongInput::default() },
            SongInput { id: tid(1), main_artist: Some(aid(1)), artist_name: "Кино".into(), title: "Song".into(), ..SongInput::default() },
            SongInput { id: tid(2), ost: true, title: "Hail Hex".into(), ..SongInput::default() },
            SongInput { id: tid(3), lyrics: Some((true, String::new())), ..SongInput::default() },
        ];
        let types = song_types(&inputs);
        let t1 = types.get(tid(1)).unwrap();
        assert_eq!((t1.lang, t1.lang_by), (Some(Lang::Russian), Some(LangSource::ArtistMajority)));
        assert_eq!(types.get(tid(2)).unwrap().lane(), SongLane::Instrumental);
        assert_eq!(types.get(tid(3)).unwrap().voice_by, VoiceSource::LrclibFlag);
        assert_eq!(types.lang_of_new("Новая", &["кино"]), Some(Lang::Russian));
    }

    /// The probe tells sung from instrumental on a fixture of features
    /// where the voice moves a few of them, as the bliss features do.
    #[test]
    fn voice_probe_auc() {
        let mut rng = Rng::new(5);
        let mut row = |sung: bool| -> (Vector, bool) {
            let mut z: Vector = std::array::from_fn(|_| rng.f32() * 2.0 - 1.0);
            if sung {
                for f in [1usize, 3, 9, 16] {
                    z[f] += 0.9;
                }
            }
            (z, sung)
        };
        let train: Vec<(Vector, bool)> = (0..400).map(|i| row(i % 3 == 0)).collect();
        let test: Vec<(Vector, bool)> = (0..300).map(|i| row(i % 3 == 0)).collect();
        let probe = VoiceProbe::train(&train).unwrap();
        let scored: Vec<(f32, bool)> = test.iter().map(|(z, y)| (probe.p(z), *y)).collect();
        let auc = crate::model::auc(&scored).unwrap();
        assert!(auc > 0.9, "AUC {auc}");
        assert!(VoiceProbe::train(&train[..1]).is_none(), "one kind only");
    }

    #[test]
    fn soundtrack_songs_name_their_theme() {
        assert_eq!(theme_of(None, "Opening Theme", "LiSA"), Theme::Opening);
        assert_eq!(theme_of(None, "Special Edition", "LiSA"), Theme::Insert, "words, not letters");
        assert_eq!(theme_of(None, "Song", "ナナチ(CV:井澤詩織)"), Theme::CharacterSong);
    }
}
