//! A text against the recording: forced alignment of all of it over the
//! whole song, then
//!
//! * how well it fits ([`Fit::score`]): the share of its pieces the model
//!   clearly hears where the path put them. Another song's words, or
//!   lines this recording does not have, are squeezed in where the model
//!   hears something else, with a low peak;
//! * for a text with line times, how they sit against the voice: the
//!   median offset of the line starts and its spread. A small spread is
//!   one constant shift (a source early or late), a large one is another
//!   version of the song (another length, another arrangement);
//! * the word timing of the whole text, made from the alignment.

use std::time::Duration;

use nikgob_core::lyrics::{LyricLine, LyricWord};

use crate::ctc::{Emissions, force_align};
use crate::text::{Vocab, pieces};

/// A piece the model hears this clearly is there.
const HEARD: f32 = 0.3;
/// A text with at least this share of its pieces heard is this song's.
pub const FITS: f32 = 0.55;
/// Line starts spread less than this around their median offset are one
/// constant shift.
pub const STEADY: f32 = 0.4;
/// Offsets smaller than this are left alone.
pub const NOTICEABLE: f32 = 0.15;
/// The aligner puts a word's start this much after the singer starts it
/// (the median against Apple's word timing on 8 songs, +0.03…+0.11 s;
/// docs/lyrics-align.md): made times are moved back by it.
pub const LATE: f32 = 0.06;

/// How a text's lines sit against the recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Timing {
    /// The text has no line times (plain text).
    None,
    /// Its line times match the voice.
    Fits,
    /// One constant shift away ([`Fit::offset`]).
    Shifted,
    /// They belong to another version of the song.
    Elsewhere,
}

/// The verdict on one text.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    /// Share of pieces heard (0..1).
    pub score: f32,
    /// [`Self::score`] over what the same letters in another order get
    /// here (0 at chance, 1 all heard).
    pub heard: f32,
    /// How much of the text the model spells on its own (0..1, see
    /// [`crate::ctc::letters_heard`]).
    pub spelled: f32,
    /// Share of the song's sung time the text's words cover (0..1; 1 when
    /// unknown): a text missing a verse covers less.
    pub covered: f32,
    pub timing: Timing,
    /// Source line time − voice, seconds (the median), for line-timed texts.
    pub offset: f32,
    /// Median distance of the line offsets from [`Self::offset`].
    pub spread: f32,
    /// The text with word timing from the alignment.
    pub lines: Vec<LyricLine>,
}

impl Fit {
    pub fn fits(&self) -> bool {
        self.score >= FITS
    }
}

/// One piece of a line on the timeline.
#[derive(Clone, Debug)]
struct Placed {
    line: usize,
    text: String,
    span: Option<(f32, f32)>,
    peak: f32,
}

/// Aligns `lines` (the text of a source; `timed` when its line starts are
/// real times) to the song's emissions.
pub fn fit_text(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], timed: bool) -> Option<Fit> {
    // Pieces: a word-timed source keeps its own; others are cut here.
    let mut placed: Vec<Placed> = Vec::new();
    let mut tokens: Vec<i32> = Vec::new();
    let mut owner: Vec<Option<usize>> = Vec::new();
    for (li, l) in lines.iter().enumerate() {
        if l.text.trim().is_empty() {
            continue;
        }
        let ps: Vec<String> = if l.words.is_empty() { pieces(&l.text) } else { l.words.iter().map(|w| w.text.clone()).collect() };
        for p in ps {
            let toks = vocab.tokens(&p);
            let idx = placed.len();
            for t in &toks {
                tokens.push(*t);
                owner.push(Some(idx));
            }
            let ends_word = p.ends_with(char::is_whitespace);
            placed.push(Placed { line: li, text: p, span: None, peak: 0.0 });
            if let Some(sep) = vocab.sep
                && ends_word
                && !toks.is_empty()
            {
                tokens.push(sep);
                owner.push(None);
            }
        }
        if let Some(sep) = vocab.sep
            && tokens.last() != Some(&sep)
            && !tokens.is_empty()
        {
            tokens.push(sep);
            owner.push(None);
        }
    }
    if tokens.is_empty() {
        return None;
    }
    let spans = force_align(em, &tokens)?;
    let mut peaks: Vec<(f32, usize)> = vec![(0.0, 0); placed.len()];
    for (span, own) in spans.iter().zip(&owner) {
        let Some(i) = *own else { continue };
        let (a, b) = (em.secs(span.first), em.secs(span.last + 1));
        let p = &mut placed[i];
        p.span = Some(match p.span {
            None => (a, b),
            Some((s, e)) => (s.min(a), e.max(b)),
        });
        peaks[i].0 += span.peak;
        peaks[i].1 += 1;
    }
    for (p, (sum, n)) in placed.iter_mut().zip(&peaks) {
        if *n > 0 {
            p.peak = sum / *n as f32;
        }
    }
    let with_tokens: Vec<&Placed> = placed.iter().filter(|p| p.span.is_some()).collect();
    let score = with_tokens.iter().filter(|p| p.peak >= HEARD).count() as f32 / with_tokens.len().max(1) as f32;

    // Line starts against the voice, from lines the model hears well.
    let mut first: Vec<Option<(f32, f32)>> = vec![None; lines.len()];
    let mut heard: Vec<(f32, usize)> = vec![(0.0, 0); lines.len()];
    for p in &placed {
        if let Some((a, _)) = p.span {
            if first[p.line].is_none() {
                first[p.line] = Some((a, p.peak));
            }
            heard[p.line].0 += p.peak;
            heard[p.line].1 += 1;
        }
    }
    let (timing, offset, spread) = if timed {
        // The better heard half of the lines (how sure a model is of
        // singing differs a lot between languages and voices).
        let line_score = |i: usize| (heard[i].1 > 0).then(|| heard[i].0 / heard[i].1 as f32);
        let scores: Vec<f32> = (0..lines.len()).filter_map(line_score).collect();
        let bar = median(&scores);
        let diffs: Vec<f32> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                let (a, _) = first[i]?;
                (line_score(i)? >= bar).then(|| l.start.as_secs_f32() - a)
            })
            .collect();
        if diffs.len() < 3 {
            (Timing::Elsewhere, 0.0, f32::INFINITY)
        } else {
            let off = median(&diffs);
            let spread = median(&diffs.iter().map(|d| (d - off).abs()).collect::<Vec<_>>());
            let timing = if spread > STEADY {
                Timing::Elsewhere
            } else if off.abs() >= NOTICEABLE {
                Timing::Shifted
            } else {
                Timing::Fits
            };
            (timing, off, spread)
        }
    } else {
        (Timing::None, 0.0, 0.0)
    };

    let text: Vec<i32> = tokens.iter().copied().filter(|t| Some(*t) != vocab.sep).collect();
    let heard_letters = crate::ctc::heard_letters(em, vocab.sep);
    let spelled = crate::ctc::letters_heard(&heard_letters, &text);
    // What a text of the same letters in another order spells here: the
    // chance level of this song and this alphabet.
    let mut scrambled = text.clone();
    for chunk in scrambled.chunks_mut(4) {
        chunk.reverse();
    }
    scrambled.reverse();
    let chance = crate::ctc::letters_heard(&heard_letters, &scrambled);
    let spelled = ((spelled - chance) / (1.0 - chance).max(0.05)).clamp(0.0, 1.0);
    // The same for the share of letters heard where the path put them.
    let chance_score = force_align(em, &scrambled).map_or(0.0, |spans| {
        spans.iter().filter(|s| s.peak >= HEARD).count() as f32 / spans.len().max(1) as f32
    });
    let heard = ((score - chance_score) / (1.0 - chance_score).max(0.05)).clamp(0.0, 1.0);
    Some(Fit { score, heard, spelled, covered: 1.0, timing, offset, spread, lines: timed_lines(lines, &placed) })
}

/// The source's lines with the pieces' times as words. A piece without
/// letters (a dash) sticks to the one before it; a line without any keeps
/// its own start.
fn timed_lines(lines: &[LyricLine], placed: &[Placed]) -> Vec<LyricLine> {
    let mut out: Vec<LyricLine> = Vec::with_capacity(lines.len());
    let d = |s: f32| Duration::from_secs_f32((s - LATE).max(0.0));
    for (li, l) in lines.iter().enumerate() {
        let mine: Vec<&Placed> = placed.iter().filter(|p| p.line == li).collect();
        let mut line = LyricLine { words: Vec::new(), ..l.clone() };
        if mine.iter().all(|p| p.span.is_none()) {
            line.end = None;
            out.push(line);
            continue;
        }
        let mut words: Vec<LyricWord> = Vec::new();
        for p in &mine {
            match p.span {
                Some((a, b)) => words.push(LyricWord { start: d(a), end: d(b), text: p.text.clone() }),
                None => match words.last_mut() {
                    Some(w) => w.text.push_str(&p.text),
                    None => words.push(LyricWord { start: Duration::ZERO, end: Duration::ZERO, text: p.text.clone() }),
                },
            }
        }
        // A leading piece without letters takes the next piece's start.
        if words.len() > 1 && words[0].end.is_zero() {
            let w0 = words.remove(0);
            words[0].text = w0.text + &words[0].text;
        }
        // A word lasts until the next one starts when the gap is short
        // (the sung vowel goes on), so the fill does not stall.
        for i in 0..words.len().saturating_sub(1) {
            let next = words[i + 1].start;
            if next > words[i].end && next - words[i].end < Duration::from_millis(600) {
                words[i].end = next;
            }
        }
        line.start = words.first().map_or(l.start, |w| w.start);
        line.end = words.last().map(|w| w.end);
        line.words = words;
        out.push(line);
    }
    // The aligner can leave a line slightly before the one it follows
    // (repeated words): keep the order.
    for i in 1..out.len() {
        if out[i].start < out[i - 1].start && !out[i].background {
            let s = out[i - 1].start;
            let shift = s - out[i].start;
            out[i].start = s;
            for w in &mut out[i].words {
                w.start += shift;
                w.end += shift;
            }
            if let Some(e) = &mut out[i].end {
                *e += shift;
            }
        }
    }
    out
}

fn median(v: &[f32]) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    if s.is_empty() {
        return 0.0;
    }
    let m = s.len() / 2;
    if s.len() % 2 == 0 { (s[m - 1] + s[m]) / 2.0 } else { s[m] }
}

/// Lines of a plain text (no times): one a line, empty lines dropped,
/// section labels (`[Chorus]`, `(x2)`) left out.
pub fn plain_lines(text: &str) -> Vec<LyricLine> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| !(l.starts_with('[') && l.ends_with(']')))
        .map(|l| LyricLine::new(Duration::ZERO, l))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Letters;

    /// A vocabulary of «а», «б», «в» and `|`, and emissions that hear the
    /// letters of `heard` (one per frame pair, blanks between).
    fn setup(heard: &str) -> (Vocab, Emissions) {
        let vocab = Vocab::from_json(r#"{"<pad>":0,"|":1,"а":2,"б":3,"в":4}"#, Letters::Native).unwrap();
        let id = |c: char| match c {
            'а' => 2,
            'б' => 3,
            'в' => 4,
            ' ' => 1,
            _ => 0,
        };
        let mut logp = Vec::new();
        let mut push = |best: usize| {
            for i in 0..5 {
                logp.push(if i == best { 0.9f32.ln() } else { 0.025f32.ln() });
            }
        };
        for c in heard.chars() {
            push(0);
            push(id(c));
            push(id(c));
        }
        push(0);
        (vocab, Emissions { logp, vocab: 5, fps: 10.0, blank: 0 })
    }

    fn line(s: f32, t: &str) -> LyricLine {
        LyricLine::new(Duration::from_secs_f32(s), t)
    }

    #[test]
    fn the_right_text_fits_and_gets_word_times() {
        let (v, em) = setup("аб ва ббб");
        let fit = fit_text(&em, &v, &[line(0.0, "аб ва"), line(1.8, "ббб")], false).unwrap();
        assert!(fit.fits(), "{}", fit.score);
        assert_eq!(fit.timing, Timing::None);
        let l = &fit.lines[0];
        assert_eq!(l.words.len(), 2);
        assert_eq!(l.words[0].text, "аб ");
        assert!(l.words[0].start < l.words[1].start);
        assert!(fit.lines[1].start > l.words[1].start);
    }

    #[test]
    fn another_text_does_not_fit() {
        let (v, em) = setup("аааааааааааа");
        let fit = fit_text(&em, &v, &[line(0.0, "бв вб"), line(1.0, "вбв")], false).unwrap();
        assert!(!fit.fits(), "{}", fit.score);
    }

    #[test]
    fn line_times_shifted_by_a_constant_are_found() {
        // Lines heard at 0.1, 1.3 and 2.5 s; the source says 0.6, 1.8, 3.0 s.
        let (v, em) = setup("ааа ббб ввв");
        let lines = [line(0.6, "ааа"), line(1.8, "ббб"), line(3.0, "ввв")];
        let fit = fit_text(&em, &v, &lines, true).unwrap();
        assert_eq!(fit.timing, Timing::Shifted, "{fit:?}");
        assert!((fit.offset - 0.5).abs() < 0.15, "{}", fit.offset);
        // Times all over the place: another version.
        let lines = [line(0.0, "ааа"), line(3.0, "ббб"), line(1.0, "ввв")];
        assert_eq!(fit_text(&em, &v, &lines, true).unwrap().timing, Timing::Elsewhere);
    }

    #[test]
    fn plain_text_lines_drop_labels() {
        let l = plain_lines("[Куплет 1]\nРаз два\n\n  три  \n[Припев]");
        assert_eq!(l.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Раз два", "три"]);
    }
}
