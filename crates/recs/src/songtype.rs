//! «Тип песни»: what a track is beyond its sound — sung or
//! instrumental, in which language, a soundtrack's song or not.
//!
//! * **Voice**, the first argument that holds: a title that names an
//!   instrumental, an off vocal or a karaoke version is instrumental
//!   (whatever lyrics of the song were fetched for it); lyrics of 12
//!   letters or more are sung; LRCLIB's instrumental mark is instrumental;
//!   else the audio says it — above [`SUNG_P`] sung, below
//!   [`INSTRUMENTAL_P`] instrumental — by the best it has: the neural sound
//!   model's vector ([`EmbedProbe`]: a logistic regression on it, trained
//!   on the library's own labels once it has [`MIN_PROBE_LABELS`] of each
//!   kind; AUC 0.98 on the owner's library in a held-out check, against
//!   0.94 of bliss's), else the model's styles read by their labels
//!   ([`crate::styles::Styles::voice`], no training at all), else bliss's
//!   23 z-scores ([`VoiceProbe`]); else a soundtrack's track is
//!   instrumental (confidence 0.6) and the rest unknown.
//! * **Dynamics** ([`Dynamics`]): the crest of the sketch — how loud the
//!   loud parts are and how far they stand above the quiet ones — so calm
//!   and the background can turn down a song that explodes
//!   ([`SongType::calm_dynamics`], [`SongType::background_dynamics`]).
//! * **Lyric valence** ([`crate::lyric_valence`]): the sign of the feeling
//!   of the cached lyrics, exactly 0 without them; a soft nudge of the dark
//!   and bright moods, never a filter.
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
use crate::embedding::{COMPACT_DIM, Compact};
use crate::mood::Axes;
use crate::sketch::SoundSketch;
use crate::tiles::{Script, script_of};

/// Bumped whenever a rule here changes: what was classified by older rules
/// (and the day's lanes picked by them) is classified again.
pub const SONGTYPE_RULES: u32 = 3;
/// The model's probe is trained from this many labels of each kind…
pub const MIN_PROBE_LABELS: usize = 20;
/// …on this many rows at most (an even sample of a larger library).
const MAX_PROBE_ROWS: usize = 4000;
/// A calm song's loud parts are this quiet at most (dB of full scale, the
/// crest's 95th percentile)…
pub const CALM_PEAK_DB: f32 = -13.0;
/// …and stand this far above its quiet ones at most: a song that whispers
/// and then explodes is no calm one, whatever its mean.
pub const CALM_SPREAD_DB: f32 = 18.0;
/// The background's: a little louder at most, as steady.
pub const BACKGROUND_PEAK_DB: f32 = -11.0;
pub const BACKGROUND_SPREAD_DB: f32 = 18.0;
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
    /// The title names an instrumental, off vocal or karaoke version.
    Title,
    /// The neural sound model's vector ([`EmbedProbe`]).
    Model,
    /// The neural sound model's styles, by their labels.
    Styles,
}

/// The crest of a song's loudness, whole dB ([`crate::sketch::Crest`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dynamics {
    /// The loud parts (the 95th percentile of its seconds).
    pub peak: i8,
    /// How far they stand above the quiet ones (the 10th percentile).
    pub spread: u8,
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
    /// The crest of its sketch, when it has one.
    #[serde(default)]
    pub dynamics: Option<Dynamics>,
    /// The feeling of its cached lyrics, ×100 (−100 sad … 100 happy); 0
    /// without them.
    #[serde(default)]
    pub valence: i8,
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
            dynamics: None,
            valence: 0,
        }
    }
}

/// The lane a song type falls in for the session's «same lane»:
/// instrumental (or a soundtrack's track of unknown voice), a sung
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

    /// Its loud parts are quiet enough for calm music and never explode
    /// out of quiet ones ([`CALM_PEAK_DB`], [`CALM_SPREAD_DB`]); a song
    /// without a sketch passes (its mean judges it, as before).
    pub fn calm_dynamics(&self) -> bool {
        self.dynamics.is_none_or(|d| f32::from(d.peak) <= CALM_PEAK_DB && f32::from(d.spread) <= CALM_SPREAD_DB)
    }

    /// The same for the background ([`BACKGROUND_PEAK_DB`]).
    pub fn background_dynamics(&self) -> bool {
        self.dynamics.is_none_or(|d| f32::from(d.peak) <= BACKGROUND_PEAK_DB && f32::from(d.spread) <= BACKGROUND_SPREAD_DB)
    }

    /// The feeling of its cached lyrics, −1…1 (0 without them).
    pub fn lyric_valence(&self) -> f32 {
        f32::from(self.valence) / 100.0
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

/// Sung or instrumental by the neural sound model's vector: a logistic
/// regression on its compact vector less the library's mean, made unit
/// length (the scale of the weights is [`EMBED_SCALE`]).
#[derive(Clone, Debug, PartialEq)]
pub struct EmbedProbe {
    /// The bias first.
    w: Vec<f32>,
    mean: [f32; COMPACT_DIM],
}

/// The centred unit vector is scaled by this before the regression (the
/// steps of [`VoiceProbe`]'s training suit features of about unit size).
const EMBED_SCALE: f32 = 16.0;

impl EmbedProbe {
    /// The features of `c` around `mean`; `None` for a vector at the mean.
    fn features(c: &Compact, mean: &[f32; COMPACT_DIM]) -> Option<[f32; COMPACT_DIM]> {
        let mut v: [f32; COMPACT_DIM] = std::array::from_fn(|k| f32::from(c.q[k]) * c.scale - mean[k]);
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !(n > 1e-9) {
            return None;
        }
        v.iter_mut().for_each(|x| *x *= EMBED_SCALE / n);
        Some(v)
    }

    /// Trained on `rows` (a vector, sung) around `mean` (the library's
    /// mean vector) as [`VoiceProbe::train`] trains; `None` with fewer
    /// than [`MIN_PROBE_LABELS`] of either kind.
    pub fn train(rows: &[(Compact, bool)], mean: [f32; COMPACT_DIM]) -> Option<Self> {
        let sung = rows.iter().filter(|r| r.1).count();
        if sung < MIN_PROBE_LABELS || rows.len() - sung < MIN_PROBE_LABELS {
            return None;
        }
        let step = rows.len().div_ceil(MAX_PROBE_ROWS).max(1);
        let x: Vec<([f32; COMPACT_DIM], f32)> =
            rows.iter().step_by(step).filter_map(|(c, y)| Some((Self::features(c, &mean)?, f32::from(u8::from(*y))))).collect();
        let n = x.len() as f32;
        let mut w = vec![0.0f32; COMPACT_DIM + 1];
        let mut g = vec![0.0f32; COMPACT_DIM + 1];
        for _ in 0..VoiceProbe::EPOCHS {
            g.iter_mut().for_each(|v| *v = 0.0);
            for (f, y) in &x {
                let p = sigmoid(w[0] + f.iter().zip(&w[1..]).map(|(a, b)| a * b).sum::<f32>());
                let d = y - p;
                g[0] += d;
                for (gi, fi) in g[1..].iter_mut().zip(f) {
                    *gi += d * fi;
                }
            }
            for (wi, gi) in w.iter_mut().zip(&g) {
                *wi += VoiceProbe::LR * (gi / n - VoiceProbe::L2 * *wi);
            }
        }
        Some(Self { w, mean })
    }

    /// The chance a track of this vector is sung.
    pub fn p(&self, c: &Compact) -> Option<f32> {
        let f = Self::features(c, &self.mean)?;
        Some(sigmoid(self.w[0] + f.iter().zip(&self.w[1..]).map(|(a, b)| a * b).sum::<f32>()))
    }
}

/// The mean of the vectors `of` gives for `ids`.
fn mean_vector(ids: impl Iterator<Item = TrackId>, of: &dyn Fn(TrackId) -> Option<Compact>) -> Option<[f32; COMPACT_DIM]> {
    let mut sum = [0f64; COMPACT_DIM];
    let mut n = 0usize;
    for c in ids.filter_map(of) {
        for (s, q) in sum.iter_mut().zip(c.q) {
            *s += f64::from(q) * f64::from(c.scale);
        }
        n += 1;
    }
    (n > 0).then(|| std::array::from_fn(|k| (sum[k] / n as f64) as f32))
}

/// A version that is the same work as the song for a session: without its
/// lead voice ([`wordless_version`]) or cut short (a TV size, a short
/// version, a game's or a film's size).
fn same_work_version(base: &str, version: &str) -> bool {
    let words: Vec<String> = version.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect();
    let has = |w: &str| words.iter().any(|x| x == w);
    wordless_version(&format!("{base} ({version})")) || has("short") || (has("size") && (has("tv") || has("game") || has("movie"))) || (has("tv") && (has("ver") || has("version") || has("edit")))
}

/// The work a title is of and whether the title names a version of it:
/// the title without its marks of an off vocal, an instrumental, a karaoke,
/// a TV size or a short version («Song (TV Size) [Off Vocal]» → «Song»,
/// true). A remix, a live or an acoustic version stays its own song.
pub fn work_title(title: &str) -> (String, bool) {
    let mut t = title.to_owned();
    let mut marked = false;
    while let Some((base, version)) = nikgob_core::clip_title::version_split(&t).or_else(|| last_part(&t)) {
        if !same_work_version(&base, &version) {
            break;
        }
        t = base;
        marked = true;
    }
    (t, marked)
}

/// The last bracket or « - » tail of a title and the title without it
/// (what [`nikgob_core::clip_title::version_split`] does not take as a
/// version: «Short Ver.»).
fn last_part(title: &str) -> Option<(String, String)> {
    let t = title.trim_end();
    let cut = |at: usize, version: &str| {
        let base = t[..at].trim_end().to_owned();
        (!base.is_empty()).then(|| (base, version.trim().to_owned()))
    };
    if let Some(close) = t.chars().last().filter(|c| matches!(c, ')' | ']' | '）' | '］')) {
        let open = match close {
            ')' => '(',
            ']' => '[',
            '）' => '（',
            _ => '［',
        };
        let at = t.rfind(open)?;
        return cut(at, &t[at + open.len_utf8()..t.len() - close.len_utf8()]);
    }
    let at = t.rfind(" - ")?;
    cut(at, &t[at + 3..])
}

/// A title that names a version without its lead voice: an instrumental,
/// an off vocal, a karaoke ([`nikgob_core::clip_title::version_mark`]).
pub fn wordless_version(title: &str) -> bool {
    use nikgob_core::clip_title::VersionKind;
    nikgob_core::clip_title::version_mark(title).is_some_and(|(_, k)| matches!(k, VersionKind::Instrumental | VersionKind::OffVocal | VersionKind::Karaoke))
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
    /// Its sketch ([`crate::sketch`]), when the analysis made one.
    pub sketch: Option<SoundSketch>,
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

/// The type of every track of `inputs` (see the module docs), without the
/// neural model's vectors.
pub fn song_types(inputs: &[SongInput]) -> SongTypes {
    song_types_with(inputs, &|_| None)
}

/// [`song_types`] with the neural model's vector of each track (`embedding`).
pub fn song_types_with(inputs: &[SongInput], embedding: &dyn Fn(TrackId) -> Option<Compact>) -> SongTypes {
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

    // The sound's probes, on the library's own labels: a version named
    // wordless by its title is instrumental whatever lyrics it was given.
    let has_text = |t: &SongInput| {
        matches!(&t.lyrics, Some((false, text)) if lyric_words(text).chars().filter(|c| c.is_alphabetic()).count() >= LYRICS_LETTERS)
    };
    let label = |t: &SongInput| -> Option<bool> {
        if wordless_version(&t.title) {
            return Some(false);
        }
        match &t.lyrics {
            Some((true, _)) => Some(false),
            _ if has_text(t) => Some(true),
            _ => None,
        }
    };
    let rows: Vec<(Vector, bool)> = inputs.iter().filter_map(|t| Some((t.z?, label(t)?))).collect();
    let probe = VoiceProbe::train(&rows);
    let embed_probe = mean_vector(inputs.iter().map(|t| t.id), embedding).and_then(|mean| {
        let rows: Vec<(Compact, bool)> = inputs.iter().filter_map(|t| Some((embedding(t.id)?, label(t)?))).collect();
        EmbedProbe::train(&rows, mean)
    });

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
        if let Some(c) = t.sketch.and_then(|s| s.crest()) {
            ty.dynamics = Some(Dynamics { peak: c.peak.round().clamp(-128.0, 127.0) as i8, spread: c.spread().round().clamp(0.0, 255.0) as u8 });
        }
        // The audio's word: the model's vector, its styles, bliss.
        let sound = embed_probe
            .as_ref()
            .zip(embedding(t.id))
            .and_then(|(p, c)| Some((p.p(&c)?, VoiceSource::Model)))
            .or_else(|| t.sketch.and_then(|s| s.styles()).map(|s| (s.voice, VoiceSource::Styles)))
            .or_else(|| probe.zip(t.z).map(|(p, z)| (p.p(&z), VoiceSource::Sound)));
        (ty.voice, ty.voice_conf, ty.voice_by) = if wordless_version(&t.title) {
            (Voice::Instrumental, 0.95, VoiceSource::Title)
        } else if has_text(t) {
            (Voice::Sung, 1.0, VoiceSource::Lyrics)
        } else if matches!(t.lyrics, Some((true, _))) {
            (Voice::Instrumental, 1.0, VoiceSource::LrclibFlag)
        } else if let Some((p, by)) = sound.filter(|(p, _)| *p > SUNG_P || *p < INSTRUMENTAL_P) {
            let voice = if p > SUNG_P { Voice::Sung } else { Voice::Instrumental };
            (voice, (p - 0.5).abs() * 2.0, by)
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
        if ty.voice == Voice::Sung
            && let Some((false, text)) = &t.lyrics
        {
            ty.valence = (crate::lyric_valence::lyric_valence(&lyric_words(text)) * 100.0).round() as i8;
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

    /// A compact vector near a direction (`voice` moves a few of its
    /// dimensions, as a voice moves the model's vector).
    fn compact(rng: &mut Rng, voice: bool) -> Compact {
        let v: [f32; COMPACT_DIM] = std::array::from_fn(|k| rng.f32() - 0.5 + if voice && k % 17 == 3 { 0.6 } else { 0.0 });
        Compact::of_unit(&v).unwrap()
    }

    /// The model's probe learns the voice from the library's own labels and
    /// tells the tracks nothing else tells; with too few labels of a kind
    /// it is not trained, and the styles speak instead.
    #[test]
    fn the_models_vector_tells_the_voice() {
        let mut rng = Rng::new(9);
        let lyrics = "[00:01]We walked along the river in the rain tonight";
        let mut inputs = Vec::new();
        let mut vectors: HashMap<TrackId, Compact> = HashMap::new();
        for i in 0..120u128 {
            let sung = i % 2 == 0;
            let labelled = i < 80;
            let lyr = labelled.then(|| if sung { (false, lyrics.to_owned()) } else { (true, String::new()) });
            inputs.push(SongInput { id: tid(i), title: format!("T{i}"), lyrics: lyr, ..SongInput::default() });
            vectors.insert(tid(i), compact(&mut rng, sung));
        }
        let types = song_types_with(&inputs, &|id| vectors.get(&id).copied());
        let mut right = 0;
        for i in 80..120u128 {
            let t = types.get(tid(i)).unwrap();
            assert!(matches!(t.voice_by, VoiceSource::Model | VoiceSource::Nothing), "{t:?}");
            if t.voice == if i % 2 == 0 { Voice::Sung } else { Voice::Instrumental } {
                right += 1;
            }
        }
        assert!(right >= 34, "{right} of 40 told right");
        // Few instrumentals labelled: no probe; a sketch's styles speak.
        let few: Vec<SongInput> = inputs.iter().filter(|t| t.id.as_uuid().as_u128() >= 80 || t.id.as_uuid().as_u128() % 2 == 0).cloned().collect();
        let mut st = crate::styles::Styles::default();
        st.voice = 0.05;
        let mut few = few;
        few.push(SongInput { id: tid(500), title: "Score".into(), sketch: Some(SoundSketch::default().with_styles(Some(&st))), ..SongInput::default() });
        let types = song_types_with(&few, &|id| vectors.get(&id).copied());
        assert!(types.by_track.values().all(|t| t.voice_by != VoiceSource::Model));
        let score = types.get(tid(500)).unwrap();
        assert_eq!((score.voice, score.voice_by), (Voice::Instrumental, VoiceSource::Styles));
    }

    /// A title naming an off vocal or a karaoke is instrumental, even with
    /// the song's lyrics fetched for it; a live version is not.
    #[test]
    fn a_wordless_version_is_told_by_its_title() {
        let lyrics = Some((false, "[00:01]君の名前を呼んでいた夜の向こうで ずっとここにいるよ".to_owned()));
        let types = song_types(&[
            SongInput { id: tid(0), title: "夏の歌 (off vocal ver.)".into(), lyrics: lyrics.clone(), ..SongInput::default() },
            SongInput { id: tid(1), title: "Song (Karaoke Version)".into(), ..SongInput::default() },
            SongInput { id: tid(2), title: "夏の歌 (Live)".into(), lyrics, ..SongInput::default() },
        ]);
        let t = |i| types.get(tid(i)).unwrap();
        assert_eq!((t(0).voice, t(0).voice_by, t(0).lane()), (Voice::Instrumental, VoiceSource::Title, SongLane::Instrumental));
        assert_eq!(t(1).voice, Voice::Instrumental);
        assert_eq!((t(2).voice, t(2).voice_by), (Voice::Sung, VoiceSource::Lyrics));
        assert!(wordless_version("Song (Instrumental)") && !wordless_version("Song (Remix)") && !wordless_version("Instinct"));
    }

    /// The crest of the sketch makes the dynamics: a song that explodes is
    /// neither calm nor background; without a sketch nothing changes. The
    /// lyrics' feeling is kept for sung songs only.
    #[test]
    fn dynamics_and_valence_come_with_the_type() {
        use crate::sketch::Crest;
        let sketch = |peak: f32, low: f32| Some(SoundSketch::new(Some(Crest { peak, low, max: peak + 1.0 }), None, None, None));
        let sad = "[00:01]I cry alone in the cold dark night, the pain and tears and sorrow never end";
        let types = song_types(&[
            SongInput { id: tid(0), sketch: sketch(-24.0, -30.0), ..SongInput::default() },
            SongInput { id: tid(1), sketch: sketch(-8.0, -32.0), ..SongInput::default() },
            SongInput { id: tid(2), ..SongInput::default() },
            SongInput { id: tid(3), lyrics: Some((false, sad.into())), ..SongInput::default() },
        ]);
        let t = |i| types.get(tid(i)).unwrap();
        assert_eq!(t(0).dynamics, Some(Dynamics { peak: -24, spread: 6 }));
        assert!(t(0).calm_dynamics() && t(0).background_dynamics());
        assert!(!t(1).calm_dynamics() && !t(1).background_dynamics(), "{:?}", t(1).dynamics);
        assert!(t(2).dynamics.is_none() && t(2).calm_dynamics());
        assert!(t(3).lyric_valence() < -0.3, "{}", t(3).lyric_valence());
        assert_eq!(t(0).valence, 0);
    }

    /// The versions of a song for a session are one work; a remix or a
    /// live one stays its own.
    #[test]
    fn versions_of_a_song_are_one_work() {
        assert_eq!(work_title("夏の歌 (off vocal ver.)"), ("夏の歌".to_owned(), true));
        assert_eq!(work_title("Song (TV Size)"), ("Song".to_owned(), true));
        assert_eq!(work_title("Song (TV Size) [Instrumental]"), ("Song".to_owned(), true));
        assert_eq!(work_title("Song - Short Ver."), ("Song".to_owned(), true));
        assert_eq!(work_title("Song (Game Size)"), ("Song".to_owned(), true));
        assert_eq!(work_title("Song (Remix)"), ("Song (Remix)".to_owned(), false));
        assert_eq!(work_title("Song (Live)"), ("Song (Live)".to_owned(), false));
        assert_eq!(work_title("Song"), ("Song".to_owned(), false));
        assert_eq!(work_title("Shortcut"), ("Shortcut".to_owned(), false));
    }

    #[test]
    fn soundtrack_songs_name_their_theme() {
        assert_eq!(theme_of(None, "Opening Theme", "LiSA"), Theme::Opening);
        assert_eq!(theme_of(None, "Special Edition", "LiSA"), Theme::Insert, "words, not letters");
        assert_eq!(theme_of(None, "Song", "ナナチ(CV:井澤詩織)"), Theme::CharacterSong);
    }
}
