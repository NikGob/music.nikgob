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

use crate::catalog::ModelId;
use crate::ctc::{Emissions, force_align};
use crate::text::{Vocab, is_hum, pieces};

/// A piece the model hears this clearly is there.
const HEARD: f32 = 0.3;
/// A text with at least this share of its pieces heard is this song's.
pub const FITS: f32 = 0.55;
/// Line starts spread less than this around their median offset are one
/// constant shift.
pub const STEADY: f32 = 0.4;
/// A line placed further than this from where its own steady times put
/// it is aligned again in its window.
const ASTRAY: f32 = 1.0;
/// A block of lines heard by another aligner may reach this far into its
/// neighbours' times.
const MARGIN: f32 = 0.3;
/// Offsets smaller than this are left alone.
pub const NOTICEABLE: f32 = 0.15;
/// The aligner puts a word's start this much after the singer starts it
/// (the median against Apple's word timing on 8 songs, +0.03…+0.11 s;
/// docs/lyrics-align.md): made times are moved back by it.
pub const LATE: f32 = 0.06;
/// A word may run this far past where its source ends the line (the
/// line's own end, else the next line's start, an empty end stamp too).
const OVERRUN: f32 = 0.5;
/// A word whose last this-long stretch has no voice in it ends where the
/// voice does ([`end_with_voice`]); a shorter pause is the vowel going on.
pub const TAIL: f32 = 0.6;
/// … that long after the last voiced moment.
const AFTER: f32 = 0.1;
/// A held word with no source end of its line goes on at most this long
/// past where the aligner ended it.
const HOLD: f32 = 4.0;
/// A held word goes on while the voice stays at this share of its line's
/// level («осенний листопад»: «беда» held at 0.8–1.1 of the loud level,
/// the reverb after «посильней» at 0.3–0.5 of a line sung at 0.9).
const HELD: f32 = 0.6;

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
    /// The text with word timing from the alignment, and the lines the
    /// song sings once more than the text has them ([`repeats`]).
    pub lines: Vec<LyricLine>,
    /// Where the source ends each of [`Self::lines`] in this recording's
    /// time, when the text's own line times sit on the voice (else, and
    /// for a line sung once more, `None`; see [`source_ends`]).
    pub ends: Vec<Option<f32>>,
    /// The aligner the verdict is from (its measures have their own
    /// levels: docs/lyrics-align.md, «The check»).
    pub aligner: Option<ModelId>,
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

/// Lines of a text that another aligner hears better than the text's own
/// (an English line of a Russian song): that aligner's emissions of the
/// song, its letters, and the lines' indices.
#[derive(Clone, Copy, Debug)]
pub struct Foreign<'a> {
    pub em: &'a Emissions,
    pub vocab: &'a Vocab,
    pub lines: &'a [usize],
}

/// Aligns `lines` (the text of a source; `timed` when its line starts are
/// real times) to the song's emissions.
pub fn fit_text(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], timed: bool) -> Option<Fit> {
    fit_text_with(em, vocab, &[], lines, timed, None)
}

/// [`fit_text`] with lines in another language: the whole text is aligned
/// with the text's own aligner first (its foreign lines too, which keeps
/// their place in time), then each run of foreign lines again with its
/// own aligner, between the lines around it. The verdict is the text's
/// own aligner's. `voiced`: which of `em`'s frames have the voice in them
/// (the separated voice; `None` on the mix), for hummed words.
///
/// Two passes: the verdict comes from the plain alignment of the whole
/// text over the whole song (what the check's levels were measured on);
/// the word timing from a second one ([`place_on_path`]) that keeps each
/// line within its source times when they sit on the voice, lets voice
/// the text lacks go between lines and times hums by the voice.
pub fn fit_text_with(em: &Emissions, vocab: &Vocab, foreign: &[Foreign<'_>], lines: &[LyricLine], timed: bool, voiced: Option<&[bool]>) -> Option<Fit> {
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
                (line_score(i)? >= bar).then_some(l.start.as_secs_f32() - a)
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

    // The word timing: the timing pass, each line kept within its own
    // moved times when they sit on the voice (a spoken intro, an ad-lib
    // the text lacks can pull a line's first word away in the pass over
    // the whole song). Where the lines do not fit their times, the first
    // pass stays, with a line that went astray aligned again in its window.
    let alien: Vec<bool> = (0..lines.len()).map(|i| foreign.iter().any(|f| f.lines.contains(&i))).collect();
    let steady = matches!(timing, Timing::Fits | Timing::Shifted);
    let bands = steady.then(|| line_bands(lines, offset, em.secs(em.frames())));
    if !place_on_path(em, vocab, &mut placed, bands.as_deref(), voiced) && steady {
        realign_lines(em, vocab, lines, &mut placed, offset, &alien);
    }
    let total = em.secs(em.frames());
    for f in foreign {
        place_foreign(f, lines, &mut placed, total);
    }
    // No word outlasts its line as the source ends it (whole-song
    // alignment can drag a last word over the song's silent tail).
    if matches!(timing, Timing::Fits | Timing::Shifted) {
        hold_to_source(em, vocab, lines, &mut placed, offset, &alien);
    }

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
    // Lines sung once more than the text has them, in the voice no word
    // took, go in as lines of their own.
    let mut all = lines.to_vec();
    if let Some(v) = voiced {
        for (li, extra) in repeats(em, vocab, lines, &placed, v, &alien) {
            let at = all.len();
            all.push(LyricLine { start: Duration::ZERO, end: None, words: Vec::new(), ..lines[li].clone() });
            placed.extend(extra.into_iter().map(|p| Placed { line: at, ..p }));
        }
    }
    let made = timed_lines(&all, &placed);
    let own_ends = if steady { source_ends(lines, offset) } else { vec![None; lines.len()] };
    // The repeats where they are sung, after the line before them.
    let mut order: Vec<usize> = (0..lines.len()).collect();
    for j in lines.len()..all.len() {
        let at = order.iter().position(|&i| !made[i].background && made[i].start > made[j].start).unwrap_or(order.len());
        order.insert(at, j);
    }
    let ends: Vec<Option<f32>> = order.iter().map(|&i| own_ends.get(i).copied().flatten()).collect();
    let mut made: Vec<Option<LyricLine>> = made.into_iter().map(Some).collect();
    let mut out: Vec<LyricLine> = order.iter().filter_map(|&i| made[i].take()).collect();
    keep_order(&mut out);
    Some(Fit { score, heard, spelled, covered: 1.0, timing, offset, spread, lines: out, ends, aligner: None })
}

/// Where each line's words may be in this recording's time when the text's
/// line times sit on the voice: from [`ASTRAY`] before its moved start to
/// [`OVERRUN`] past where the source ends it ([`source_ends`]; the song's
/// end for the last line). `None` for background lines, which go with the
/// line they back.
fn line_bands(lines: &[LyricLine], offset: f32, total: f32) -> Vec<Option<Band>> {
    let ends = source_ends(lines, offset);
    lines
        .iter()
        .zip(ends)
        .map(|(l, end)| {
            if l.background {
                return None;
            }
            let start = l.start.as_secs_f32() - offset;
            Some(Band { from: (start - ASTRAY).max(0.0), to: end.map_or(total, |e| (e + OVERRUN).min(total)), start })
        })
        .collect()
}

/// Where a line's words may be (seconds), and where the source starts it.
#[derive(Clone, Copy, Debug)]
struct Band {
    from: f32,
    to: f32,
    start: f32,
}

/// The timing pass: the text's pieces again over the whole song, as one
/// path ([`crate::ctc::align_path`]), with
/// * each line within its band (`bands`, [`line_bands`]), so a repeated
///   line takes its own repeat and neighbours never overlap;
/// * a filler between lines and after the last that takes voice the text
///   lacks (a hum, an ad-lib, a line sung once more) instead of a word
///   stretched over it (not for a model of characters, [`Vocab::syllabic`]);
/// * hummed words ([`crate::text::is_hum`]) timed by where the voice is.
///
/// False (nothing changed) when the pieces do not fit the bands.
fn place_on_path(em: &Emissions, vocab: &Vocab, placed: &mut [Placed], bands: Option<&[Option<Band>]>, voiced: Option<&[bool]>) -> bool {
    use crate::ctc::{Path, Tok, align_path};
    let frame = |s: f32| ((s * em.fps).round().max(0.0) as usize).min(em.frames().saturating_sub(1));
    // A model of characters hears the right ones far under its best: a
    // filler would take the text's own voice (Japanese: 93 % of words
    // within 0.3 s without, 81–91 % with one).
    let fill = !vocab.syllabic();
    let mut path = Path::default();
    let mut owner: Vec<Option<usize>> = Vec::new();
    let mut line = None;
    for (i, p) in placed.iter().enumerate() {
        let band = bands.and_then(|b| b[p.line]);
        let hum = is_hum(&p.text);
        let toks = vocab.tokens(&p.text);
        if toks.is_empty() && !hum {
            continue;
        }
        if line.is_some_and(|l| l != p.line) && path.toks.last() != Some(&Tok::Filler) && fill {
            // A line ends a word, then whatever the text lacks may follow.
            if let Some(sep) = vocab.sep
                && path.toks.last() != Some(&Tok::Letter(sep))
            {
                path.push(Tok::Letter(sep), None);
                owner.push(None);
            }
            path.push(Tok::Filler, None);
            owner.push(None);
        }
        // The line's first token is pulled towards its source start.
        let mut at = (line != Some(p.line)).then(|| band.map(|b| frame(b.start))).flatten();
        line = Some(p.line);
        let band = band.map(|b| (frame(b.from), frame(b.to)));
        let toks = if hum { vec![Tok::Hum] } else { toks.into_iter().map(Tok::Letter).collect() };
        for t in toks {
            path.push_at(t, band, at.take());
            owner.push(Some(i));
        }
        if let Some(sep) = vocab.sep
            && p.text.ends_with(char::is_whitespace)
        {
            path.push(Tok::Letter(sep), None);
            owner.push(None);
        }
    }
    if path.toks.is_empty() {
        return false;
    }
    // Voice after the last line (a line sung once more) is none of its words.
    if fill {
        path.push(Tok::Filler, None);
        owner.push(None);
    }
    let Some(spans) = align_path(em, &path, voiced) else { return false };
    let mut fresh: Vec<Option<(f32, f32)>> = vec![None; placed.len()];
    for (span, own) in spans.iter().zip(&owner) {
        let (Some(span), Some(i)) = (span, own) else { continue };
        let (a, b) = (em.secs(span.first), em.secs(span.last + 1));
        fresh[*i] = Some(fresh[*i].map_or((a, b), |(s, e)| (s.min(a), e.max(b))));
    }
    for (p, f) in placed.iter_mut().zip(fresh) {
        if f.is_some() {
            p.span = f;
        }
    }
    true
}

/// Voice this long that no word covers (pauses under [`JOINED`] in it) is
/// looked at for a line sung once more than the text has it.
const UNCOVERED: f32 = 1.2;
const JOINED: f32 = 0.5;
/// … a line heard this well there (the share of its letters the model
/// hears where the path puts them) is that line again («космоцветок»: its
/// last line's two repeats are heard at 0.6 and over; no line of the
/// measured songs with a full text was taken for a repeat at 0.6),
const REPEAT: f32 = 0.6;
/// … when it has this many letters at least (a short line is heard almost
/// anywhere).
const REPEAT_LETTERS: usize = 8;

/// Lines the song sings once more than the text has them (a last line
/// sung three times, written once): in each stretch of voice that no
/// word covers ([`UNCOVERED`]), the nearest lines before it (four) and
/// after it (two) are tried, and the best heard ([`REPEAT`]) is that line
/// again; what is left of the stretch is tried in turn. Each comes with
/// its pieces, for a line of its own (their `line`: the line repeated).
fn repeats(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], placed: &[Placed], voiced: &[bool], skip: &[bool]) -> Vec<(usize, Vec<Placed>)> {
    use crate::ctc::{Path, Tok, align_path};
    let n = em.frames().min(voiced.len());
    let frame = |s: f32| ((s * em.fps).round().max(0.0) as usize).min(n);
    let mut covered = vec![false; n];
    for (a, b) in placed.iter().filter_map(|p| p.span) {
        covered[frame(a - 0.3)..frame(b + 0.3)].iter_mut().for_each(|c| *c = true);
    }
    // Each line's place, for the lines around a stretch.
    let mut place: Vec<Option<(f32, f32)>> = vec![None; lines.len()];
    for p in placed {
        if let Some((a, b)) = p.span {
            place[p.line] = Some(place[p.line].map_or((a, b), |(s, e)| (s.min(a), e.max(b))));
        }
    }
    let usable = |li: usize| !lines[li].background && !skip[li] && place[li].is_some();
    let joined = (JOINED * em.fps) as usize;
    let long = (UNCOVERED * em.fps) as usize;
    let mut out = Vec::new();
    let mut t = 0;
    while t < n {
        if covered[t] || !voiced[t] {
            t += 1;
            continue;
        }
        let a = t;
        let mut b = t;
        while t < n && !covered[t] && t - b <= joined {
            if voiced[t] {
                b = t;
            }
            t += 1;
        }
        let mut from = a;
        while b + 1 - from >= long {
            let (sa, sb) = (em.secs(from), em.secs(b + 1));
            let mut before: Vec<usize> = (0..lines.len()).filter(|&i| usable(i) && place[i].is_some_and(|(_, e)| e <= sa + 0.3)).collect();
            before.sort_by(|x, y| place[*y].unwrap().1.total_cmp(&place[*x].unwrap().1));
            let mut after: Vec<usize> = (0..lines.len()).filter(|&i| usable(i) && place[i].is_some_and(|(s, _)| s >= sb - 0.3)).collect();
            after.sort_by(|x, y| place[*x].unwrap().0.total_cmp(&place[*y].unwrap().0));
            let (slice, at) = em.slice((sa - 0.3).max(0.0), sb + 0.3);
            let mut best: Option<(f32, usize, Vec<Placed>, usize)> = None;
            for li in before.into_iter().take(4).chain(after.into_iter().take(2)) {
                let mine: Vec<&Placed> = placed.iter().filter(|p| p.line == li).collect();
                let mut path = Path::default();
                let mut owner = Vec::new();
                for (k, p) in mine.iter().enumerate() {
                    let toks = if is_hum(&p.text) { vec![Tok::Hum] } else { vocab.tokens(&p.text).into_iter().map(Tok::Letter).collect() };
                    for t in toks {
                        path.push(t, None);
                        owner.push(Some(k));
                    }
                    if let Some(sep) = vocab.sep
                        && p.text.ends_with(char::is_whitespace)
                        && path.toks.last().is_some_and(|t| *t != Tok::Letter(sep))
                    {
                        path.push(Tok::Letter(sep), None);
                        owner.push(None);
                    }
                }
                let letters = path.toks.iter().filter(|t| matches!(t, Tok::Letter(id) if Some(*id) != vocab.sep)).count();
                if letters < REPEAT_LETTERS {
                    continue;
                }
                path.push(Tok::Filler, None);
                owner.push(None);
                let Some(spans) = align_path(&slice, &path, Some(&voiced[at..(at + slice.frames()).min(voiced.len())])) else { continue };
                let heard = spans
                    .iter()
                    .zip(&path.toks)
                    .filter(|(s, t)| s.is_some_and(|s| s.peak >= HEARD) && matches!(t, Tok::Letter(id) if Some(*id) != vocab.sep))
                    .count();
                let score = heard as f32 / letters as f32;
                if score < REPEAT || best.as_ref().is_some_and(|(s, ..)| *s >= score) {
                    continue;
                }
                let mut pieces: Vec<Placed> = mine.iter().map(|p| Placed { line: li, text: p.text.clone(), span: None, peak: 0.0 }).collect();
                let mut end = from;
                for (span, own) in spans.iter().zip(&owner) {
                    let (Some(span), Some(k)) = (span, own) else { continue };
                    let (x, y) = (em.secs(at + span.first), em.secs(at + span.last + 1));
                    pieces[*k].span = Some(pieces[*k].span.map_or((x, y), |(s, e)| (s.min(x), e.max(y))));
                    end = end.max(at + span.last + 1);
                }
                // Sung again, a line takes about as long as it did (a line
                // smeared over a long stretch is not it).
                let (first, last) = pieces.iter().filter_map(|p| p.span).fold((f32::INFINITY, 0.0f32), |(s, e), (x, y)| (s.min(x), e.max(y)));
                let own = place[li].map_or(0.0, |(a, b)| b - a);
                if last - first > own * 2.0 + 0.5 {
                    continue;
                }
                best = Some((score, li, pieces, end));
            }
            let Some((_, li, pieces, end)) = best else { break };
            out.push((li, pieces));
            from = end.max(from + 1);
        }
    }
    out
}

/// Aligns again a line placed more than [`ASTRAY`] from its moved start
/// (source time less `offset`), inside its window: from half a second
/// before that start to half a second after the next line's. The others
/// keep the whole-song pass, where every line's neighbours hold their own
/// words (a window alone lets a line take its neighbour's sounds: 97 % of
/// words within 0.3 s fell to 65 % when every line was aligned so).
fn realign_lines(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], placed: &mut [Placed], offset: f32, skip: &[bool]) {
    let starts: Vec<f32> = lines.iter().map(|l| l.start.as_secs_f32() - offset).collect();
    let total = em.secs(em.frames());
    for (li, l) in lines.iter().enumerate() {
        if l.text.trim().is_empty() || l.background || skip[li] {
            continue;
        }
        let next = starts[li + 1..]
            .iter()
            .zip(&lines[li + 1..])
            .find(|(_, n)| !n.background && !n.text.trim().is_empty())
            .map_or(total, |(s, _)| *s);
        let first = placed.iter().filter(|p| p.line == li).find_map(|p| p.span).map(|(a, _)| a);
        if first.is_none_or(|a| (a - starts[li]).abs() <= ASTRAY) {
            continue;
        }
        let (from, to) = ((starts[li] - 0.5).max(0.0), (next + 0.5).min(total));
        if to - from < 0.3 {
            continue;
        }
        let idx: Vec<usize> = placed.iter().enumerate().filter(|(_, p)| p.line == li).map(|(i, _)| i).collect();
        align_pieces(em, vocab, placed, &idx, from, to);
    }
}

/// The source's own end of each line in this recording's time (less
/// `offset`): the line's end, else the next line's start (an empty end
/// stamp is a line too). `None` for a background line and for the last
/// line without an end.
fn source_ends(lines: &[LyricLine], offset: f32) -> Vec<Option<f32>> {
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if l.background {
                return None;
            }
            let start = l.start.as_secs_f32();
            let own = l.end.map(|e| e.as_secs_f32()).filter(|e| *e > start);
            let next = || lines[i + 1..].iter().filter(|n| !n.background).map(|n| n.start.as_secs_f32()).find(|s| *s > start);
            own.or_else(next).map(|e| e - offset)
        })
        .collect()
}

/// Keeps every word of a line within [`OVERRUN`] of the source's end of
/// it ([`source_ends`]): a line with a word that starts past that is
/// aligned again between its own start and that end; a word that only
/// runs over is cut there.
fn hold_to_source(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], placed: &mut [Placed], offset: f32, skip: &[bool]) {
    let ends = source_ends(lines, offset);
    let total = em.secs(em.frames());
    for (li, l) in lines.iter().enumerate() {
        let Some(end) = ends[li].filter(|_| !skip[li]) else { continue };
        let limit = (end + OVERRUN).min(total);
        let idx: Vec<usize> = placed.iter().enumerate().filter(|(_, p)| p.line == li).map(|(i, _)| i).collect();
        let spans = || idx.iter().filter_map(|i| placed[*i].span);
        if spans().all(|(_, b)| b <= limit) {
            continue;
        }
        if spans().any(|(a, _)| a >= limit - 0.1) {
            let first = spans().map(|(a, _)| a).fold(f32::INFINITY, f32::min);
            let from = ((l.start.as_secs_f32() - offset).min(first) - 0.5).max(0.0);
            if limit - from >= 0.3 {
                align_pieces(em, vocab, placed, &idx, from, limit);
            }
        }
        for &i in &idx {
            if let Some((a, b)) = placed[i].span {
                let b = b.min(limit);
                placed[i].span = Some((a.min(b), b));
            }
        }
    }
}

/// How the words end against the voice ([`end_with_voice`]).
#[derive(Clone, Copy, Debug)]
pub struct VoiceEnds<'a> {
    /// One flag every `step` seconds: the voice is there.
    pub voiced: &'a [bool],
    pub step: f32,
    /// A word whose last this-long stretch has no voice ends with it.
    pub tail: f32,
    /// The voice's loudness at every step, for holding words: a word goes
    /// on while the voice stays near its line's level (only where it is
    /// the voice alone, not the mix: `None`).
    pub level: Option<&'a [f32]>,
}

/// Ends words where the voice ends, both ways:
/// * a word whose last `tail` seconds or more have no voice ends a moment
///   after its last voiced step (never into a silent tail);
/// * a word the voice goes on holding after the aligner's end (a held
///   last vowel: the aligner hears its letters only at the start) lasts
///   while the voice stays at [`HELD`] of its line's level or more (a
///   reverb tail or a quiet backing voice falls under it), a step under
///   it bridged, up to the next word's
///   start, the next line's start and where the source ends its line
///   (`bounds`, see [`Fit::ends`]); without a source end, at most
///   [`HOLD`] longer.
///
/// A line ends with its last word, so no pause starts while a note is
/// held. A word with no voice under it at all is left where the aligner
/// put it.
pub fn end_with_voice(lines: &mut [LyricLine], v: VoiceEnds<'_>, bounds: &[Option<f32>]) {
    let VoiceEnds { voiced, step, tail, level } = v;
    if voiced.is_empty() || step <= 0.0 {
        return;
    }
    // Where the next sung line starts, after each line.
    let next_starts: Vec<Option<f32>> = (0..lines.len())
        .map(|li| lines[li + 1..].iter().filter(|n| !n.background).find_map(|n| n.words.first().map(|w| w.start.as_secs_f32())))
        .collect();
    for (li, l) in lines.iter_mut().enumerate() {
        if l.words.is_empty() {
            continue;
        }
        for w in &mut l.words {
            let (a, b) = (w.start.as_secs_f32(), w.end.as_secs_f32());
            let i0 = ((a / step).floor().max(0.0) as usize).min(voiced.len());
            let i1 = ((b / step).ceil().max(0.0) as usize).min(voiced.len());
            let Some(last) = (i0..i1).rev().find(|i| voiced[*i]) else { continue };
            let cut = (last + 1) as f32 * step + AFTER;
            if b - (cut - AFTER) >= tail && cut < b {
                w.end = Duration::from_secs_f32(cut.max(a));
            }
        }
        let Some(level) = level.filter(|_| !l.background) else {
            l.end = l.words.last().map(|w| w.end);
            continue;
        };
        // The line's level: the median over where its words are sung.
        let at = |t: f32| ((t / step).floor().max(0.0) as usize).min(level.len());
        let (a, b) = (l.words[0].start.as_secs_f32(), l.words[l.words.len() - 1].end.as_secs_f32());
        let sung = &level[at(a)..at(b).max(at(a))];
        if sung.is_empty() {
            l.end = l.words.last().map(|w| w.end);
            continue;
        }
        let bar = median(sung) * HELD;
        let held_at = |j: usize| level.get(j).is_some_and(|e| *e >= bar);
        let next_line = next_starts[li];
        let n = l.words.len();
        for i in 0..n {
            let b = l.words[i].end.as_secs_f32();
            let limit = if i + 1 < n {
                l.words[i + 1].start.as_secs_f32()
            } else {
                let own = bounds.get(li).copied().flatten().unwrap_or(b + HOLD);
                next_line.map_or(own, |s| own.min(s))
            };
            if limit <= b {
                continue;
            }
            let mut j = (b / step).floor().max(0.0) as usize;
            let mut held = None;
            while j < level.len() && (j as f32) * step < limit {
                if held_at(j) {
                    held = Some(j);
                } else if !held_at(j + 1) {
                    break;
                }
                j += 1;
            }
            if let Some(k) = held {
                let e = ((k + 1) as f32 * step).min(limit);
                if e > b {
                    l.words[i].end = Duration::from_secs_f32(e);
                }
            }
        }
        l.end = l.words.last().map(|w| w.end);
    }
    no_overlap(lines);
}

/// A sung line ends by the time the next one starts (its last word cut
/// there, never before its own start); background lines go with theirs.
fn no_overlap(lines: &mut [LyricLine]) {
    for li in 0..lines.len() {
        if lines[li].background || lines[li].words.is_empty() {
            continue;
        }
        let Some(next) = lines[li + 1..].iter().filter(|n| !n.background).find_map(|n| n.words.first().map(|w| w.start)) else { continue };
        let l = &mut lines[li];
        if let Some(w) = l.words.last_mut()
            && w.end > next
        {
            w.end = next.max(w.start);
            l.end = Some(w.end);
        }
    }
}

/// Aligns each run of `f`'s lines again with its own aligner, between the
/// end of the line before it and the start of the line after it (as the
/// whole-song pass placed them), a little into both.
fn place_foreign(f: &Foreign<'_>, lines: &[LyricLine], placed: &mut [Placed], total: f32) {
    let span_of = |placed: &[Placed], li: usize| {
        placed.iter().filter(|p| p.line == li).filter_map(|p| p.span).fold(None, |acc: Option<(f32, f32)>, (a, b)| {
            Some(acc.map_or((a, b), |(s, e)| (s.min(a), e.max(b))))
        })
    };
    let sung: Vec<usize> = (0..lines.len()).filter(|i| !lines[*i].text.trim().is_empty()).collect();
    let mut k = 0;
    while k < sung.len() {
        if !f.lines.contains(&sung[k]) {
            k += 1;
            continue;
        }
        let first = k;
        while k < sung.len() && f.lines.contains(&sung[k]) {
            k += 1;
        }
        let from = sung[..first].iter().rev().find_map(|li| span_of(placed, *li)).map_or(0.0, |(_, e)| e);
        let to = sung[k..].iter().find_map(|li| span_of(placed, *li)).map_or(total, |(a, _)| a);
        let (from, to) = ((from - MARGIN).max(0.0), (to + MARGIN).min(total));
        if to - from < 0.3 {
            continue;
        }
        let idx: Vec<usize> = placed.iter().enumerate().filter(|(_, p)| sung[first..k].contains(&p.line)).map(|(i, _)| i).collect();
        align_pieces(f.em, f.vocab, placed, &idx, from, to);
    }
}

/// Aligns the pieces `idx` of `placed` within seconds `from..to` of `em`
/// and takes their new places (all of them, or none when they do not fit).
fn align_pieces(em: &Emissions, vocab: &Vocab, placed: &mut [Placed], idx: &[usize], from: f32, to: f32) {
    let mut tokens = Vec::new();
    let mut owner = Vec::new();
    for &i in idx {
        let toks = vocab.tokens(&placed[i].text);
        for t in &toks {
            tokens.push(*t);
            owner.push(Some(i));
        }
        if let Some(sep) = vocab.sep
            && !toks.is_empty()
            && placed[i].text.ends_with(char::is_whitespace)
        {
            tokens.push(sep);
            owner.push(None);
        }
    }
    if tokens.is_empty() {
        return;
    }
    let (slice, at) = em.slice(from, to);
    let Some(spans) = force_align(&slice, &tokens) else { return };
    let mut fresh: Vec<Option<(f32, f32)>> = vec![None; placed.len()];
    let mut peaks: Vec<(f32, usize)> = vec![(0.0, 0); placed.len()];
    for (span, i) in spans.iter().zip(&owner) {
        let Some(i) = *i else { continue };
        let (a, b) = (em.secs(at + span.first), em.secs(at + span.last + 1));
        fresh[i] = Some(fresh[i].map_or((a, b), |(s, e)| (s.min(a), e.max(b))));
        peaks[i].0 += span.peak;
        peaks[i].1 += 1;
    }
    for &i in idx {
        if let Some(span) = fresh[i] {
            placed[i].span = Some(span);
            placed[i].peak = peaks[i].0 / peaks[i].1.max(1) as f32;
        }
    }
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
    out
}

/// The aligner can leave a line slightly before the one it follows
/// (repeated words): the order is kept.
fn keep_order(out: &mut [LyricLine]) {
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
}

fn median(v: &[f32]) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    if s.is_empty() {
        return 0.0;
    }
    let m = s.len() / 2;
    if s.len().is_multiple_of(2) { (s[m - 1] + s[m]) / 2.0 } else { s[m] }
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

    /// Emissions over a 5-letter vocabulary (0 blank) from the best letter
    /// of each frame, at 10 frames a second.
    fn frames(best: &[usize]) -> Emissions {
        let mut logp = Vec::new();
        for &b in best {
            for i in 0..5 {
                logp.push(if i == b { 0.9f32.ln() } else { 0.025f32.ln() });
            }
        }
        Emissions { logp, vocab: 5, fps: 10.0, blank: 0 }
    }

    #[test]
    fn a_foreign_line_is_placed_by_its_own_aligner() {
        // The text's aligner hears «а», an ad-lib it takes for letters
        // (frames 4–5), nothing in the foreign line, then «б»; the other
        // aligner hears «xyz» at frames 10–12.
        let (v, _) = setup("");
        let main = frames(&[0, 2, 0, 0, 4, 4, 0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 0]);
        let other_vocab = Vocab::from_json(r#"{"<pad>":0,"|":1,"x":2,"y":3,"z":4}"#, Letters::Native).unwrap();
        let other = frames(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 4, 0, 0, 0, 0]);
        let lines = [line(0.0, "а"), line(0.0, "xyz"), line(0.0, "б")];
        let alone = fit_text(&main, &v, &lines, false).unwrap();
        assert!(alone.lines[1].start.as_secs_f32() < 0.7, "{:?}", alone.lines[1]);
        let f = Foreign { em: &other, vocab: &other_vocab, lines: &[1] };
        let both = fit_text_with(&main, &v, &[f], &lines, false, None).unwrap();
        let start = both.lines[1].start.as_secs_f32();
        assert!((start - (1.0 - LATE)).abs() < 0.05, "{:?}", both.lines[1]);
        // The verdict stays the text's aligner's.
        assert_eq!(both.score, alone.score);
        assert!(both.lines[2].start > both.lines[1].start);
    }

    /// «Хитрый Юра» four times: a line the text has twice while the song
    /// sings it three times takes the repeats its source times point to,
    /// the one between goes to no word, and the lines never overlap.
    #[test]
    fn each_repeat_of_a_line_takes_its_own_occurrence() {
        let (v, _) = setup("");
        let mut best = vec![0; 40];
        for (t, l) in [(1, [4, 2]), (10, [2, 3]), (18, [2, 3]), (26, [2, 3]), (34, [4, 2])] {
            best[t..t + 2].copy_from_slice(&l);
        }
        let em = frames(&best);
        let lines = [line(0.1, "ва"), line(1.0, "аб"), line(2.6, "аб"), line(3.4, "ва")];
        let fit = fit_text(&em, &v, &lines, true).unwrap();
        assert_eq!(fit.timing, Timing::Fits, "{fit:?}");
        let starts: Vec<f32> = fit.lines.iter().map(|l| l.start.as_secs_f32() + LATE).collect();
        assert!((starts[1] - 1.0).abs() < 0.05 && (starts[2] - 2.6).abs() < 0.05, "{starts:?}");
        for w in fit.lines.windows(2) {
            assert!(w[0].end.unwrap() <= w[1].start, "{:?}", fit.lines);
        }
    }

    #[test]
    fn hums_and_held_vowels_are_told_from_words() {
        for hum in ["м", "М, ", "мм", "мгм", "угу, ", "а-а", "о-о-о", "Mmm", "hmm"] {
            assert!(is_hum(hum), "{hum}");
        }
        for word in ["а", "о ", "мама", "ура", "a", "oh", "ага", "ой", "ん", "ああ"] {
            assert!(!is_hum(word), "{word}");
        }
    }

    /// Lines end by the time the next one starts.
    #[test]
    fn a_line_never_runs_into_the_next() {
        let w = |a: f32, b: f32| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: "x".into() };
        let mut a = line(1.0, "x");
        a.words = vec![w(1.0, 1.5), w(1.5, 3.2)];
        let mut b = line(3.0, "x");
        b.words = vec![w(3.0, 3.5)];
        let mut lines = vec![a, b];
        no_overlap(&mut lines);
        assert_eq!(lines[0].words[1].end, Duration::from_secs(3));
        assert_eq!(lines[0].end, Some(Duration::from_secs(3)));
    }

    /// «В этой траве»: the last line's word ends in the song's silent
    /// tail when that tail is heard as letters; the source's empty end
    /// stamp holds it.
    #[test]
    fn a_last_word_ends_where_the_source_ends_its_line() {
        let (v, _) = setup("");
        // «ав» at 0.2 s, «ба» at 0.8 s, «в» at 1.4 s, silence, then a
        // phantom «а» at 4.1 s.
        let mut best = vec![0, 0, 2, 0, 4, 1, 0, 0, 3, 0, 2, 1, 0, 0, 4];
        best.extend([0; 26]);
        best.extend([2, 2, 0]);
        let em = frames(&best);
        let mut lines = vec![line(0.2, "ав"), line(0.8, "ба"), line(1.4, "ва")];
        let alone = fit_text(&em, &v, &lines, true).unwrap();
        assert!(matches!(alone.timing, Timing::Fits | Timing::Shifted), "{alone:?}");
        assert!(alone.lines[2].words[0].end.as_secs_f32() > 4.0, "without an end it runs to the tail: {:?}", alone.lines[2]);
        // The empty end stamp at 1.7 s.
        lines.push(line(1.7, ""));
        let held = fit_text(&em, &v, &lines, true).unwrap();
        let end = held.lines[2].words[0].end.as_secs_f32();
        assert!(end <= 1.7 + OVERRUN + 0.01, "{:?}", held.lines[2]);
        assert_eq!(held.lines[2].end, held.lines[2].words.last().map(|w| w.end));
        // A line's own end holds it just the same.
        let mut own = lines[..3].to_vec();
        own[2].end = Some(Duration::from_secs_f32(1.8));
        assert!(fit_text(&em, &v, &own, true).unwrap().lines[2].words[0].end.as_secs_f32() <= 1.8 + OVERRUN + 0.01);
        // A plain text has no times of its own: nothing holds it here.
        let plain: Vec<LyricLine> = lines[..3].iter().map(|l| line(0.0, &l.text)).collect();
        assert_eq!(fit_text(&em, &v, &plain, false).unwrap().timing, Timing::None);
    }

    #[test]
    fn source_ends_take_the_line_s_end_or_the_next_start() {
        let mut ls = vec![line(1.0, "а"), line(3.0, "б"), line(5.0, "в"), line(9.0, "")];
        ls[1].end = Some(Duration::from_secs(4));
        let mut bg = line(3.5, "о");
        bg.background = true;
        ls.insert(2, bg);
        let ends = source_ends(&ls, 0.5);
        assert_eq!(ends, [Some(2.5), Some(3.5), None, Some(8.5), None]);
    }

    /// Where the voice ends, a word ends: a sung held note stays long, a
    /// word that runs on into silence stops a moment after the voice.
    #[test]
    fn a_word_ends_with_the_voice_but_a_held_note_stays() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut held = line(1.0, "о-о-о");
        held.words = vec![w(1.0, 5.0, "о-о-о")];
        let mut tail = line(6.0, "ха");
        tail.words = vec![w(6.0, 6.2, "х"), w(6.2, 16.0, "а")];
        let mut short = line(17.0, "да");
        short.words = vec![w(17.0, 17.9, "да")];
        let mut lines = vec![held, tail, short];
        // Voice from 1 to 5 s, 6 to 6.8 s, 17 to 17.5 s (tenths).
        let voiced: Vec<bool> = (0..200).map(|i| (10..50).contains(&i) || (60..68).contains(&i) || (170..175).contains(&i)).collect();
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: None };
        end_with_voice(&mut lines, v, &[None; 3]);
        assert!((lines[0].words[0].end.as_secs_f32() - 5.0).abs() < 1e-3, "held: {:?}", lines[0].words);
        let end = lines[1].words[1].end.as_secs_f32();
        assert!((end - 6.9).abs() < 0.01, "{end}");
        assert_eq!(lines[1].end, Some(lines[1].words[1].end));
        // 0.4 s of no voice at its end is the vowel going on: kept.
        assert!((lines[2].words[0].end.as_secs_f32() - 17.9).abs() < 1e-3);
    }

    /// «осенний листопад»: «беда» is sung held for seconds, the aligner
    /// hears its letters only at the start (79.88 → 80.16) and the line's
    /// source end is 85.29. The word goes on while the voice holds it near
    /// the line's level, a short dip bridged, and stops where the voice
    /// stops or falls to a reverb tail; never past the source's end of the
    /// line or into the next line.
    #[test]
    fn a_held_last_word_lasts_while_the_voice_holds_it() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let line_of = |words: Vec<LyricWord>| {
            let mut l = line(words[0].start.as_secs_f32(), "x");
            l.end = words.last().map(|w| w.end);
            l.words = words;
            l
        };
        let lines = vec![
            line_of(vec![w(78.6, 79.4, "Знаешь, "), w(79.4, 79.88, "не "), w(79.88, 80.16, "беда")]),
            line_of(vec![w(85.3, 86.0, "Если")]),
        ];
        // Sung at 0.9 from 78.6 to 83.4 s with a dip at 81.0 s, a reverb
        // tail at 0.4 to 84.5 s, the next line from 85.3 s.
        let level: Vec<f32> = (0..900)
            .map(|i| match i {
                810 => 0.3,
                786..834 => 0.9,
                834..845 => 0.4,
                853.. => 0.9,
                _ => 0.0,
            })
            .collect();
        let voiced: Vec<bool> = level.iter().map(|e| *e > 0.05).collect();
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: Some(&level) };
        let mut held = lines.clone();
        end_with_voice(&mut held, v, &[Some(85.29), None]);
        let end = held[0].words[2].end.as_secs_f32();
        assert!((end - 83.4).abs() < 0.01, "{end}");
        assert_eq!(held[0].end, Some(held[0].words[2].end), "the line ends with the held word");
        assert!((held[0].words[0].end.as_secs_f32() - 79.4).abs() < 1e-3, "a word followed by the next keeps its end");
        // Sung on to the next line: the source's end of the line holds it.
        let on: Vec<f32> = (0..900).map(|i| if i >= 786 { 0.9 } else { 0.0 }).collect();
        let v_on = VoiceEnds { level: Some(&on), ..v };
        let mut long = lines.clone();
        end_with_voice(&mut long, v_on, &[Some(85.0), None]);
        assert!((long[0].words[2].end.as_secs_f32() - 85.0).abs() < 0.01);
        // … else the next line's start.
        let mut next = lines.clone();
        end_with_voice(&mut next, v_on, &[Some(90.0), None]);
        assert!((next[0].words[2].end.as_secs_f32() - 85.3).abs() < 0.01);
        // On the mix nothing is held.
        let mut mix = lines.clone();
        end_with_voice(&mut mix, VoiceEnds { level: None, ..v }, &[Some(85.29), None]);
        assert!((mix[0].words[2].end.as_secs_f32() - 80.16).abs() < 1e-3);
        // Without a source end, at most HOLD longer (and the last line too).
        let mut last = vec![lines[0].clone()];
        end_with_voice(&mut last, v_on, &[None]);
        assert!((last[0].words[2].end.as_secs_f32() - (80.16 + HOLD)).abs() < 0.01);
    }

    #[test]
    fn plain_text_lines_drop_labels() {
        let l = plain_lines("[Куплет 1]\nРаз два\n\n  три  \n[Припев]");
        assert_eq!(l.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Раз два", "три"]);
    }
}
