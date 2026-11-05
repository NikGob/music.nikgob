//! Syllable timing from the voice (docs/lyrics-align.md, «Syllables»).
//!
//! The timing pass gives each word its start and end; here a word of
//! several syllables ([`nikgob_core::syllables`]) is cut into them, each a
//! [`LyricWord`] of its own (the last keeps the word's space, so readers of
//! words see one written word as glued pieces). Where each syllable starts
//! is heard: the word's letters are aligned again over the word's own
//! stretch of the same emissions (the timing pass's path restricted to the
//! word, with its final start and end, holds included), and a syllable
//! starts where its first letter is heard. So the word keeps its start and
//! end exactly, a held note goes to the syllable sung through it (the last
//! one for a held last vowel: it lasts to the word's end), and fast
//! syllables get what the voice gave them.
//!
//! Letters the path squeezed into a frame or two (a word's end the model
//! did not hear) would make a syllable flash: a syllable shorter than half
//! its fair length ([`fair`]) takes that length from the syllable before
//! it, else from the one after, as long as that one keeps its own. Where
//! the word is too short to align its letters, or its letters are mostly
//! wildcards, the syllables share its time by their letters (vowels
//! twice).

use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord};
use nikgob_core::syllables::{Rules, split};

use crate::ctc::{Emissions, STAR, force_align};
use crate::fit::LATE;
use crate::text::{Vocab, is_hum};

/// What heard a line: its aligner's emissions of the song and letters.
#[derive(Clone, Copy, Debug)]
pub struct Heard<'a> {
    pub em: &'a Emissions,
    pub vocab: &'a Vocab,
    /// Latin words are cut as English ([`Rules::English`]); false for the
    /// multilingual aligner's languages.
    pub english: bool,
}

/// A syllable gets at least half of this long (seconds a letter, vowels
/// twice) before it takes time from its neighbour …
const FAIR_LETTER: f32 = 0.04;
/// … within these bounds.
const FAIR_MIN: f32 = 0.06;
const FAIR_MAX: f32 = 0.2;

/// Cuts the words of each line into syllables timed by `heard` (one per
/// line; `None` leaves the line as it is). A line that already comes in
/// syllables (glued pieces: Apple's, lrc.red's) is kept.
pub fn time_lines(lines: &mut [LyricLine], heard: &[Option<Heard<'_>>]) {
    for (l, h) in lines.iter_mut().zip(heard) {
        let Some(h) = h else { continue };
        if l.words.windows(2).any(|w| !w[0].text.ends_with(char::is_whitespace)) {
            continue;
        }
        l.words = l.words.iter().flat_map(|w| syllables_of(w, *h)).collect();
    }
}

/// One word as its syllables (itself when it has one).
pub fn syllables_of(w: &LyricWord, h: Heard<'_>) -> Vec<LyricWord> {
    let parts = split(&w.text, Rules::of(&w.text, h.english));
    if parts.len() < 2 || is_hum(&w.text) {
        return vec![w.clone()];
    }
    let (a, b) = (w.start.as_secs_f32(), w.end.as_secs_f32().max(w.start.as_secs_f32()));
    let weights: Vec<f32> = parts.iter().map(|p| weight(p)).collect();
    let mut starts = heard_starts(&parts, a, b, h).unwrap_or_else(|| by_weight(&weights, a, b));
    spread_squeezed(&mut starts, &weights, a, b);
    pieces(w, &parts, &starts)
}

/// The word cut at `starts` (the first is the word's own start).
fn pieces(w: &LyricWord, parts: &[&str], starts: &[f32]) -> Vec<LyricWord> {
    let n = parts.len();
    let at = |i: usize| if i == 0 { w.start } else { Duration::from_secs_f32(starts[i]).clamp(w.start, w.end.max(w.start)) };
    (0..n)
        .map(|i| LyricWord { start: at(i), end: if i + 1 == n { w.end.max(w.start) } else { at(i + 1) }, text: parts[i].to_owned() })
        .collect()
}

/// How much of a word's time a syllable's letters ask for: a vowel twice
/// a consonant, signs nothing.
fn weight(part: &str) -> f32 {
    const VOWELS: &str = "aeiouyàáâãäåæèéêëìíîïòóôõöøùúûüýÿœаеёиоуыэюяіїє";
    part.chars()
        .filter(|c| c.is_alphabetic() && !matches!(c, 'ь' | 'ъ' | 'Ь' | 'Ъ'))
        .map(|c| if VOWELS.contains(c.to_lowercase().next().unwrap_or(c)) { 2.0 } else { 1.0 })
        .sum::<f32>()
        .max(0.5)
}

/// Each syllable's start where its first letter is heard in the word's
/// stretch (`a`..`b`, made time); `None` when it cannot be aligned there.
fn heard_starts(parts: &[&str], a: f32, b: f32, h: Heard<'_>) -> Option<Vec<f32>> {
    let mut tokens = Vec::new();
    let mut first = Vec::with_capacity(parts.len());
    for p in parts {
        let toks = h.vocab.tokens(p);
        if toks.is_empty() {
            return None;
        }
        first.push(tokens.len());
        tokens.extend(toks);
    }
    // Wildcards (Latin for a Russian model) are placed anywhere voiced:
    // no evidence of where a syllable starts.
    if tokens.iter().filter(|t| **t == STAR).count() * 2 > tokens.len() {
        return None;
    }
    let (slice, at) = h.em.slice(a + LATE, b + LATE);
    let spans = force_align(&slice, &tokens)?;
    if spans.len() != tokens.len() {
        return None;
    }
    Some(
        first
            .iter()
            .enumerate()
            .map(|(i, &k)| if i == 0 { a } else { (h.em.secs(at + spans[k].first) - LATE).clamp(a, b) })
            .collect(),
    )
}

/// Starts that share `a`..`b` by `weights`.
fn by_weight(weights: &[f32], a: f32, b: f32) -> Vec<f32> {
    let total: f32 = weights.iter().sum::<f32>().max(f32::EPSILON);
    let mut done = 0.0;
    weights
        .iter()
        .map(|w| {
            let s = a + (b - a) * done / total;
            done += w;
            s
        })
        .collect()
}

/// How long a syllable of `weight` should last at least, with the word's
/// `room` shared by all `weights` when they ask for more.
fn fair(weights: &[f32], room: f32) -> Vec<f32> {
    let want: Vec<f32> = weights.iter().map(|w| (w * FAIR_LETTER).clamp(FAIR_MIN, FAIR_MAX)).collect();
    let total: f32 = want.iter().sum();
    let k = if total > room { room / total.max(f32::EPSILON) } else { 1.0 };
    want.into_iter().map(|f| f * k).collect()
}

/// A syllable squeezed under half its fair length takes that length from
/// the one before it (then from the one after), each giver keeping its
/// own; starts stay in order within `a`..`b`.
fn spread_squeezed(starts: &mut [f32], weights: &[f32], a: f32, b: f32) {
    let n = starts.len();
    if n < 2 {
        return;
    }
    starts[0] = a;
    for i in 1..n {
        starts[i] = starts[i].clamp(starts[i - 1], b);
    }
    let fair = fair(weights, b - a);
    let end = |s: &[f32], i: usize| if i + 1 < n { s[i + 1] } else { b };
    for i in (1..n).rev() {
        let e = end(starts, i);
        if e - starts[i] >= fair[i] * 0.5 {
            continue;
        }
        let floor = starts[i - 1] + fair[i - 1];
        let new = (e - fair[i]).max(floor);
        if new < starts[i] {
            starts[i] = new;
        }
    }
    for i in 0..n - 1 {
        let e = starts[i + 1];
        if e - starts[i] >= fair[i] * 0.5 {
            continue;
        }
        let ceil = end(starts, i + 1) - fair[i + 1];
        let new = (starts[i] + fair[i]).min(ceil);
        if new > starts[i + 1] {
            starts[i + 1] = new;
        }
    }
    for i in 1..n {
        starts[i] = starts[i].clamp(starts[i - 1], b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Letters;

    fn word(start: f32, end: f32, text: &str) -> LyricWord {
        LyricWord { start: Duration::from_secs_f32(start), end: Duration::from_secs_f32(end), text: text.into() }
    }

    /// A vocabulary of a few Russian letters and emissions (50 frames a
    /// second) that hear each `(frame, letter)` and blank elsewhere.
    fn heard(frames: usize, at: &[(usize, char)]) -> (Vocab, Emissions) {
        let letters = "кмосцветпрдаб";
        let mut json = String::from(r#"{"<pad>":0,"|":1"#);
        for (i, c) in letters.chars().enumerate() {
            json.push_str(&format!(r#","{c}":{}"#, i + 2));
        }
        json.push('}');
        let vocab = Vocab::from_json(&json, Letters::Native).unwrap();
        let v = letters.chars().count() + 2;
        let mut logp = Vec::with_capacity(frames * v);
        for t in 0..frames {
            let hit = at.iter().find(|(f, _)| *f == t).map(|(_, c)| letters.chars().position(|l| l == *c).unwrap() + 2);
            for k in 0..v {
                let p: f32 = match hit {
                    Some(h) if k == h => 0.9,
                    Some(_) => 0.1 / (v - 1) as f32,
                    None if k == 0 => 0.9,
                    None => 0.1 / (v - 1) as f32,
                };
                logp.push(p.ln());
            }
        }
        (vocab, Emissions { logp, vocab: v, fps: 50.0, blank: 0 })
    }

    fn texts(ws: &[LyricWord]) -> Vec<&str> {
        ws.iter().map(|w| w.text.as_str()).collect()
    }

    /// «ко-смо-цве-ток»: fast over «ко-смо-», a held «-цве-», closed by
    /// «-ток»; the word keeps its start and end.
    #[test]
    fn syllables_start_where_their_first_letter_is_heard() {
        // Frames (made time + LATE): к 50, о 52, с 55, м 57, о 59, ц 63,
        // в 65, е 67 … held … т 105, о 107, к 110.
        let marks = [(50, 'к'), (52, 'о'), (55, 'с'), (57, 'м'), (59, 'о'), (63, 'ц'), (65, 'в'), (67, 'е'), (105, 'т'), (107, 'о'), (110, 'к')];
        let (vocab, em) = heard(200, &marks);
        let h = Heard { em: &em, vocab: &vocab, english: false };
        let w = word(1.0 - LATE, 2.3 - LATE, "космоцветок ");
        let s = syllables_of(&w, h);
        assert_eq!(texts(&s), ["ко", "смо", "цве", "ток "]);
        assert_eq!(s[0].start, w.start);
        assert_eq!(s[3].end, w.end);
        let starts: Vec<f32> = s.iter().map(|x| x.start.as_secs_f32() + LATE).collect();
        for (got, want) in starts.iter().zip([1.0, 1.1, 1.26, 2.1]) {
            assert!((got - want).abs() < 0.021, "{starts:?}");
        }
        // Each ends where the next starts: the held «цве» lasts 0.84 s.
        assert!(s.windows(2).all(|p| p[0].end == p[1].start));
        assert!((s[2].end.as_secs_f32() - s[2].start.as_secs_f32() - 0.84).abs() < 0.03);
    }

    /// The last syllable's letters squeezed into the stretch's last frames
    /// take a fair share from the syllable before.
    #[test]
    fn squeezed_letters_are_spread_and_never_flash() {
        // «то» heard at 50/52, «пор» squeezed into the last three frames.
        let (vocab, em) = heard(100, &[(50, 'т'), (52, 'о'), (87, 'п'), (88, 'о'), (89, 'р')]);
        let h = Heard { em: &em, vocab: &vocab, english: false };
        let w = word(1.0 - LATE, 1.8 - LATE, "топор");
        let s = syllables_of(&w, h);
        assert_eq!(texts(&s), ["то", "пор"]);
        let last = s[1].end.as_secs_f32() - s[1].start.as_secs_f32();
        assert!(last >= 0.15, "{last}");
        assert_eq!((s[0].start, s[1].end), (w.start, w.end));
    }

    #[test]
    fn short_or_unheard_words_share_their_time_by_letters() {
        let (vocab, em) = heard(10, &[]);
        let h = Heard { em: &em, vocab: &vocab, english: true };
        // Three frames cannot hold five letters: by weight.
        let s = syllables_of(&word(0.0, 0.06, "мама "), h);
        assert_eq!(texts(&s), ["ма", "ма "]);
        assert!((s[1].start.as_secs_f32() - 0.03).abs() < 1e-3);
        // Latin for a Russian model is wildcards only: by weight too.
        let s = syllables_of(&word(0.0, 0.1, "Miley"), h);
        assert_eq!(texts(&s), ["Mi", "ley"]);
        // One syllable, a hum and zero length stay whole or in order.
        assert_eq!(syllables_of(&word(0.0, 0.1, "стоп "), h).len(), 1);
        assert_eq!(syllables_of(&word(0.0, 0.1, "а-а-а"), h).len(), 1);
        let z = syllables_of(&word(1.0, 1.0, "мама"), h);
        assert!(z.iter().all(|x| x.start == x.end && x.start == Duration::from_secs(1)));
    }

    #[test]
    fn lines_in_syllables_already_are_kept() {
        let (vocab, em) = heard(10, &[]);
        let h = Heard { em: &em, vocab: &vocab, english: true };
        let mut lines = vec![LyricLine { words: vec![word(0.0, 0.1, "ма"), word(0.1, 0.2, "ма ")], ..Default::default() }];
        let before = lines.clone();
        time_lines(&mut lines, &[Some(h)]);
        assert_eq!(lines, before);
        let mut lines = vec![LyricLine { words: vec![word(0.0, 0.2, "мама "), word(0.2, 0.3, "да")], ..Default::default() }];
        time_lines(&mut lines, &[Some(h)]);
        assert_eq!(texts(&lines[0].words), ["ма", "ма ", "да"]);
        let joined: String = lines[0].words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(joined, "мама да");
    }
}
