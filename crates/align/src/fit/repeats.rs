//! Lines the song sings once more than the text has them, and identical
//! lines in a row pulled towards even shares of their stretch.

use std::time::{Duration, Instant};

use super::*;

/// Identical lines in a row (three or more, «Miley Cyrus» eight times) are
/// sung about equally long. Where the timing pass made one of them far
/// longer or shorter than the others (a line took two sung repeats, the
/// next ones moved on by one), each is pulled (`pull` a second) towards
/// an even share of the stretch they all took. True when the path was
/// changed (to be aligned again). Lines with source times that sit on
/// the voice keep those (`banded`).
#[allow(clippy::too_many_arguments)]
pub(super) fn even_repeats(
    lines: &[LyricLine],
    heads: &[(usize, usize)],
    owner: &[Option<usize>],
    placed: &[Placed],
    spans: &[Option<crate::ctc::TokenSpan>],
    path: &mut crate::ctc::Path,
    pull: f32,
    banded: bool,
) -> bool {
    if banded {
        return false;
    }
    let key = |li: usize| -> String { lines[li].text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect() };
    // Where each line was placed (its first and last frame).
    let mut place: Vec<Option<(usize, usize)>> = vec![None; lines.len()];
    for (span, own) in spans.iter().zip(owner) {
        let (Some(span), Some(i)) = (span, own) else { continue };
        let li = placed[*i].line;
        place[li] = Some(place[li].map_or((span.first, span.last), |(a, b)| (a.min(span.first), b.max(span.last))));
    }
    let mut changed = false;
    let mut k = 0;
    while k < heads.len() {
        let mut j = k + 1;
        while j < heads.len() && !lines[heads[j].0].background && key(heads[j].0) == key(heads[k].0) && !key(heads[k].0).is_empty() {
            j += 1;
        }
        let run = &heads[k..j];
        k = j;
        if run.len() < 3 || lines[run[0].0].background {
            continue;
        }
        let Some(spots) = run.iter().map(|(li, _)| place[*li]).collect::<Option<Vec<_>>>() else { continue };
        let (from, to) = (spots[0].0, spots[spots.len() - 1].1 + 1);
        let n = run.len();
        let share = to.saturating_sub(from) as f32 / n as f32;
        // Each line's length, start to the next line's start.
        let lens: Vec<f32> = (0..n).map(|i| (if i + 1 < n { spots[i + 1].0 } else { to }) as f32 - spots[i].0 as f32).collect();
        if !lens.iter().any(|l| *l > share * 1.5 || *l < share * 0.5) {
            continue;
        }
        for (i, (_, tok)) in run.iter().enumerate() {
            path.at[*tok] = Some(from + (share * i as f32).round() as usize);
            path.pull[*tok] = pull;
        }
        changed = true;
    }
    changed
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
/// The search for lines sung once more gives up after this long and keeps
/// what it found by then: over a text of another song (or another
/// recording) a long stretch of voice no word covers can take minutes
/// («Контракт» against another song's text sat for more than 5 minutes).
/// The verdict does not depend on it, so a good text is never thrown out
/// for a search cut short.
pub(super) const REPEATS_BUDGET: Duration = Duration::from_secs(3);

/// Lines the song sings once more than the text has them (a last line
/// sung three times, written once): in each stretch of voice that no
/// word covers ([`UNCOVERED`]), the nearest lines before it (four) and
/// after it (two) are tried, and the best heard ([`REPEAT`]) is that line
/// again; what is left of the stretch is tried in turn. Each comes with
/// its pieces, for a line of its own (their `line`: the line repeated).
pub(super) fn repeats(em: &Emissions, vocab: &Vocab, lines: &[LyricLine], placed: &[Placed], voiced: &[bool], skip: &[bool], budget: Duration) -> Vec<(usize, Vec<Placed>)> {
    use crate::ctc::{Path, Tok, align_path};
    let started = Instant::now();
    let n = em.frames().min(voiced.len());
    let frame = |s: f32| ((s * em.fps).round().max(0.0) as usize).min(n);
    let mut covered = vec![false; n];
    for (a, b) in placed.iter().filter_map(|p| p.span) {
        let (from, to) = (frame(a - 0.3), frame(b + 0.3));
        covered[from.min(to)..to].iter_mut().for_each(|c| *c = true);
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
        // A repeat found here may end past `b` (its slice runs 0.3 s on).
        while (b + 1).saturating_sub(from) >= long {
            if started.elapsed() >= budget {
                log::warn!("align: search for repeated lines stopped after {:.1} s at {:.1} s of {:.1} s, {} found", started.elapsed().as_secs_f32(), em.secs(from), em.secs(n), out.len());
                return out;
            }
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
                    let toks = if is_hum(&p.text) { vec![Tok::Hum] } else { vocab.tokens(&p.say).into_iter().map(Tok::Letter).collect() };
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
                let till = (at + slice.frames()).min(voiced.len());
                let v = crate::ctc::Voice { voiced: &voiced[at.min(till)..till], onset: &[], ..Default::default() };
                let Some(spans) = align_path(&slice, &path, Some(v)) else { continue };
                let heard = spans
                    .iter()
                    .zip(&path.toks)
                    .filter(|(s, t)| s.is_some_and(|s| s.peak >= HEARD) && matches!(t, Tok::Letter(id) if Some(*id) != vocab.sep))
                    .count();
                let score = heard as f32 / letters as f32;
                if score < REPEAT || best.as_ref().is_some_and(|(s, ..)| *s >= score) {
                    continue;
                }
                let mut pieces: Vec<Placed> = mine.iter().map(|p| Placed { line: li, text: p.text.clone(), say: p.say.clone(), span: None, peak: 0.0 }).collect();
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
