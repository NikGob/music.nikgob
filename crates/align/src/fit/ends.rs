//! Line ends and spreading: where a word ends against the voice, a
//! squeezed word's fair share, and no line running into the next.

use super::*;
use crate::text::can_hold;

/// Aligns again a line placed more than [`ASTRAY`] from its moved start
/// (source time less `offset`), inside its window: from half a second
/// before that start to half a second after the next line's. The others
/// keep the whole-song pass, where every line's neighbours hold their own
/// words (a window alone lets a line take its neighbour's sounds: 97 % of
/// words within 0.3 s fell to 65 % when every line was aligned so).
pub(super) fn realign_lines(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], placed: &mut [Placed], offset: f32, skip: &[bool]) {
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

/// Which lines [`source_ends`] ends at the next line's start (no end of
/// their own).
pub(super) fn ends_from_next(lines: &[LyricLine]) -> Vec<bool> {
    lines.iter().map(|l| !l.background && l.end.is_none_or(|e| e <= l.start)).collect()
}

/// The source's own end of each line in this recording's time (less
/// `offset`): the line's end, else the next line's start (an empty end
/// stamp is a line too). `None` for a background line and for the last
/// line without an end.
pub(super) fn source_ends(lines: &[LyricLine], offset: f32) -> Vec<Option<f32>> {
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
pub(super) fn hold_to_source(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], placed: &mut [Placed], offset: f32, skip: &[bool]) {
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
    /// [`Tuning::stop_hold`], [`Tuning::far_end`] (a line without a source
    /// end is held only while the voice holds without a dip) and
    /// [`Tuning::squeezed`].
    pub tuning: Tuning,
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
    let VoiceEnds { voiced, step, tail, level, tuning } = v;
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
        let words = l.words.len();
        // With `end_level`, the voice counts for an end only at that share
        // of the line's level (the median over where its words are sung).
        let floor = match level.filter(|_| tuning.end_level > 0.0 && !l.background) {
            Some(level) => {
                let at = |t: f32| ((t / step).floor().max(0.0) as usize).min(level.len());
                let (a, b) = (at(l.words[0].start.as_secs_f32()), at(l.words[words - 1].end.as_secs_f32()));
                let sung = &level[a..b.max(a)];
                (!sung.is_empty()).then(|| median(sung) * tuning.end_level)
            }
            None => None,
        };
        let gap = next_starts[li].map_or(f32::INFINITY, |n| n - l.words[words - 1].end.as_secs_f32());
        let floor = floor.filter(|_| gap >= tuning.end_gap);
        for (i, w) in l.words.iter_mut().enumerate() {
            let (a, b) = (w.start.as_secs_f32(), w.end.as_secs_f32());
            let i0 = ((a / step).floor().max(0.0) as usize).min(voiced.len());
            let i1 = ((b / step).ceil().max(0.0) as usize).min(voiced.len());
            let floor = floor.filter(|_| i + 1 == words);
            let sounds = |i: usize| voiced[i] && floor.is_none_or(|f| level.and_then(|l| l.get(i)).is_some_and(|e| *e >= f));
            let Some(last) = (i0..i1).rev().find(|i| sounds(*i)) else { continue };
            let cut = (last + 1) as f32 * step + AFTER;
            // A word before a comma inside a line ends with its voice after
            // a shorter silence: the line takes its breath there
            // (`comma_ends`).
            let tail = if tuning.comma_ends && i + 1 < words && breathes(&w.text) { tail.min(BREATH) } else { tail };
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
        let bound = bounds.get(li).copied().flatten();
        for i in 0..n {
            let b = l.words[i].end.as_secs_f32();
            let mut limit = if i + 1 < n {
                l.words[i + 1].start.as_secs_f32()
            } else {
                let own = bound.unwrap_or(b + HOLD);
                next_line.map_or(own, |s| own.min(s))
            };
            if tuning.stop_hold > 0.0 && !can_hold(&l.words[i].text) {
                limit = limit.min(b + tuning.stop_hold);
            }
            if limit <= b {
                continue;
            }
            // Without a source end, a dip ends the hold.
            let bridge = tuning.far_end <= 0.0 || bound.is_some() || i + 1 < n;
            let mut j = (b / step).floor().max(0.0) as usize;
            let mut held = None;
            while j < level.len() && (j as f32) * step < limit {
                if held_at(j) {
                    held = Some(j);
                } else if !bridge || !held_at(j + 1) {
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
    if tuning.squeezed {
        spread_squeezed(lines);
    }
}

/// Letters left at one frame each («Miley»: «M» heard, «-iley» dumped at
/// the end of a long held stretch) after this many frames of voice inside
/// the word are spread over it ([`spread_collapsed`]).
const COLLAPSED_AFTER: usize = 10;

/// A word's letters (first and last frame each, in order) where two or
/// more at its end got a frame each right after a long stretch of voice
/// ([`COLLAPSED_AFTER`] frames, most of them `voiced`) inside the word:
/// they are spread evenly over that stretch, from the end of the last
/// letter before them to the word's end, a vowel (`vowel`) taking twice a
/// consonant's share. The word's start and end stay; what moves is where
/// its syllables fall inside it. Nothing changes otherwise.
pub fn spread_collapsed(letters: &mut [(usize, usize)], vowel: &[bool], voiced: &[bool]) {
    let n = letters.len();
    let mut k = 0;
    while k < n && letters[n - 1 - k].0 == letters[n - 1 - k].1 && (k == 0 || letters[n - k].0 <= letters[n - 1 - k].1 + 2) {
        k += 1;
    }
    if k < 2 || k == n {
        return;
    }
    let first = n - k;
    let from = letters[first - 1].1 + 1;
    let end = letters[n - 1].1;
    let gap = letters[first].0.saturating_sub(from);
    // Letters out of order (the last before the run ending past the word's
    // end) leave nothing to spread.
    if gap < COLLAPSED_AFTER || (end + 1).saturating_sub(from) < k {
        return;
    }
    let sung = (from..letters[first].0).filter(|t| voiced.get(*t).copied().unwrap_or(false)).count();
    if sung * 2 < gap {
        return;
    }
    let weight = |i: usize| if vowel.get(i).copied().unwrap_or(false) { 2.0 } else { 1.0 };
    let total: f32 = (first..n).map(weight).sum();
    let frames = (end + 1 - from) as f32;
    let mut at = from as f32;
    for i in first..n {
        let a = at.round() as usize;
        at += frames * weight(i) / total;
        let b = ((at.round() as usize).max(a + 1) - 1).min(end);
        letters[i] = (a.min(end), b.max(a.min(end)));
    }
    letters[n - 1].1 = end;
}

/// A word gets at least this long a letter (seconds), up to [`FAIR_MAX`].
const FAIR_LETTER: f32 = 0.05;
const FAIR_MIN: f32 = 0.12;
const FAIR_MAX: f32 = 0.3;

pub(super) fn fair(word: &str) -> f32 {
    (word.chars().filter(|c| c.is_alphabetic()).count() as f32 * FAIR_LETTER).clamp(FAIR_MIN, FAIR_MAX)
}

/// A word squeezed to almost nothing («Cyrus» 125.76–125.76: its letters
/// collapsed at the end of its stretch, or cut where the next line starts)
/// takes a fair share ([`fair`]) from the word before it, as long as that
/// one keeps its own; a line's first word takes it from the pause after
/// it. Nothing moves past the next line's start.
pub(super) fn spread_squeezed(lines: &mut [LyricLine]) {
    let starts: Vec<Option<Duration>> = (0..lines.len())
        .map(|li| lines[li + 1..].iter().filter(|n| !n.background).find_map(|n| n.words.first().map(|w| w.start)))
        .collect();
    for (li, l) in lines.iter_mut().enumerate() {
        if l.background || l.words.is_empty() {
            continue;
        }
        let limit = starts[li].map_or(f32::INFINITY, |d| d.as_secs_f32());
        let n = l.words.len();
        let secs = |d: Duration| d.as_secs_f32();
        for i in (0..n).rev() {
            let want = fair(&l.words[i].text);
            let end = secs(l.words[i].end).min(limit).max(secs(l.words[i].start).min(limit));
            let start = secs(l.words[i].start).min(end);
            if end - start >= want * 0.5 {
                continue;
            }
            if i > 0 {
                let prev_start = secs(l.words[i - 1].start);
                let floor = prev_start + fair(&l.words[i - 1].text);
                let new_start = (end - want).max(floor);
                if new_start < start {
                    l.words[i].start = Duration::from_secs_f32(new_start);
                    l.words[i].end = Duration::from_secs_f32(end);
                    let pe = secs(l.words[i - 1].end).min(new_start).max(prev_start);
                    l.words[i - 1].end = Duration::from_secs_f32(pe);
                    continue;
                }
            }
            let room = if i + 1 < n { secs(l.words[i + 1].start) } else { limit };
            let new_end = (start + want).min(room);
            if new_end > end {
                l.words[i].end = Duration::from_secs_f32(new_end);
            }
        }
        l.start = l.words[0].start;
        l.end = l.words.last().map(|w| w.end);
    }
}

/// A sung line ends by the time the next one starts (its last word cut
/// there, never before its own start); background lines go with theirs.
pub(super) fn no_overlap(lines: &mut [LyricLine]) {
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
