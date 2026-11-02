//! CTC forced alignment: the most likely path of a known token sequence
//! through a model's per-frame log-probabilities (Viterbi over the tokens
//! with blanks between them, as torchaudio's `forced_align`).
//!
//! A token the model has no letter for is a wildcard ([`STAR`]): it takes
//! whatever non-blank letter the frame likes best, a little discounted, so
//! a kanji outside the vocabulary or a digit still gets its place in time
//! without pulling the rest off.

/// A token that matches any non-blank letter.
pub const STAR: i32 = -1;
/// What a wildcard costs against the best letter of its frame (natural log).
const STAR_PENALTY: f32 = 1.0;
/// Backpointers kept at most (frames × states): about a 20-minute song with
/// a long text. Longer ones are aligned in pieces by the caller.
pub const MAX_CELLS: usize = 600_000_000;

/// Per-frame log-probabilities of a CTC model over a whole song.
#[derive(Clone, Debug, Default)]
pub struct Emissions {
    /// `frames × vocab`, log-softmax.
    pub logp: Vec<f32>,
    pub vocab: usize,
    /// Frames per second of audio.
    pub fps: f32,
    pub blank: usize,
}

impl Emissions {
    pub fn frames(&self) -> usize {
        self.logp.len().checked_div(self.vocab).unwrap_or(0)
    }

    pub fn row(&self, t: usize) -> &[f32] {
        &self.logp[t * self.vocab..(t + 1) * self.vocab]
    }

    /// A frame as seconds.
    pub fn secs(&self, frame: usize) -> f32 {
        frame as f32 / self.fps
    }

    /// The frames from `from` to `to` (seconds) as their own emissions.
    pub fn slice(&self, from: f32, to: f32) -> (Self, usize) {
        let a = ((from * self.fps).floor().max(0.0) as usize).min(self.frames());
        let b = ((to * self.fps).ceil().max(0.0) as usize).clamp(a, self.frames());
        (
            Self { logp: self.logp[a * self.vocab..b * self.vocab].to_vec(), vocab: self.vocab, fps: self.fps, blank: self.blank },
            a,
        )
    }

    /// Turns raw logits (`frames × vocab`) into log-probabilities in place.
    pub fn log_softmax(logits: &mut [f32], vocab: usize) {
        for row in logits.chunks_exact_mut(vocab) {
            let max = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let sum: f32 = row.iter().map(|x| (x - max).exp()).sum();
            let log = max + sum.ln();
            for x in row {
                *x -= log;
            }
        }
    }
}

/// Where one token of the sequence was sung.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenSpan {
    /// First and last frame (inclusive).
    pub first: usize,
    pub last: usize,
    /// How the token compares with the letter the model likes best, at the
    /// token's best frame (its probability over that letter's, 0..1): 1
    /// where the token is what is heard, low where the path only squeezed
    /// it in. Relative, because a speech model is unsure of every letter
    /// of singing, the right ones too.
    pub peak: f32,
}

/// Aligns `tokens` (vocabulary ids, or [`STAR`]) to `em`. `None` when the
/// sequence cannot fit (more tokens than frames) or is too long to keep.
pub fn force_align(em: &Emissions, tokens: &[i32]) -> Option<Vec<TokenSpan>> {
    let t_len = em.frames();
    let l = tokens.len();
    if l == 0 || t_len == 0 {
        return Some(Vec::new());
    }
    // Repeats need a blank between them, so a frame each plus one.
    let repeats = tokens.windows(2).filter(|w| w[0] == w[1] && w[0] != STAR).count();
    if l + repeats > t_len {
        return None;
    }
    let s_len = 2 * l + 1;
    if t_len.saturating_mul(s_len) > MAX_CELLS {
        return None;
    }
    let blank = em.blank;
    let best: Vec<f32> = (0..t_len)
        .map(|t| {
            em.row(t)
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != blank)
                .map(|(_, v)| *v)
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .collect();
    let star: Vec<f32> = best.iter().map(|b| b - STAR_PENALTY).collect();
    let emit = |t: usize, s: usize| -> f32 {
        if s.is_multiple_of(2) {
            em.row(t)[blank]
        } else {
            match tokens[s / 2] {
                STAR => star[t],
                tok => em.row(t)[tok as usize],
            }
        }
    };
    // A token may follow the one two states back directly (skipping the
    // blank between them) unless both are the same letter.
    let skip: Vec<bool> = (0..s_len)
        .map(|s| s % 2 == 1 && s >= 3 && (tokens[s / 2] != tokens[s / 2 - 1] || tokens[s / 2] == STAR))
        .collect();
    const NEG: f32 = -1e30;
    let mut dp = vec![NEG; s_len];
    let mut next = vec![NEG; s_len];
    let mut back = vec![0u8; t_len * s_len];
    dp[0] = emit(0, 0);
    dp[1] = emit(0, 1);
    for t in 1..t_len {
        // States that can still reach the end in the frames left, and
        // that the frames so far can have reached.
        let lo = s_len.saturating_sub(2 * (t_len - t) + 1);
        let hi = (2 * t + 2).min(s_len);
        let row = &mut back[t * s_len..(t + 1) * s_len];
        for s in 0..s_len {
            if s < lo || s >= hi {
                next[s] = NEG;
                continue;
            }
            let mut best = dp[s];
            let mut k = 0u8;
            if s >= 1 && dp[s - 1] > best {
                best = dp[s - 1];
                k = 1;
            }
            if skip[s] && dp[s - 2] > best {
                best = dp[s - 2];
                k = 2;
            }
            next[s] = best + emit(t, s);
            row[s] = k;
        }
        std::mem::swap(&mut dp, &mut next);
    }
    let mut s = if s_len >= 2 && dp[s_len - 2] > dp[s_len - 1] { s_len - 2 } else { s_len - 1 };
    if dp[s] <= NEG / 2.0 {
        return None;
    }
    let mut spans: Vec<Option<TokenSpan>> = vec![None; l];
    for t in (0..t_len).rev() {
        if s % 2 == 1 {
            let i = s / 2;
            let p = (emit(t, s) - best[t]).exp().min(1.0);
            let span = spans[i].get_or_insert(TokenSpan { first: t, last: t, peak: 0.0 });
            span.first = t;
            span.peak = span.peak.max(p);
        }
        let k = back[t * s_len + s] as usize;
        s -= k;
    }
    spans.into_iter().collect()
}

/// The letters the model hears on its own (best letter of each frame,
/// repeats collapsed, blanks and `skip` left out).
pub fn heard_letters(em: &Emissions, skip: Option<i32>) -> Vec<i32> {
    let mut out = Vec::new();
    let mut prev = usize::MAX;
    for t in 0..em.frames() {
        let row = em.row(t);
        let best = (0..em.vocab).max_by(|a, b| row[*a].total_cmp(&row[*b])).unwrap_or(em.blank);
        if best != prev && best != em.blank && Some(best as i32) != skip {
            out.push(best as i32);
        }
        prev = best;
    }
    out
}

/// How much of `text` (tokens, [`STAR`] matching anything) the heard
/// letters spell, in order: 1 − edits / letters, where a letter of the
/// text missing or heard as another costs 1 and a heard letter the text
/// does not have costs a half (backing vocals, ad-libs). 0..1.
pub fn letters_heard(heard: &[i32], text: &[i32]) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let ins = 0.5f32;
    // Heard letters before and after the text are free (an intro, an
    // outro): it may start and end anywhere.
    let mut prev: Vec<f32> = vec![0.0; heard.len() + 1];
    let mut cur = vec![0.0f32; heard.len() + 1];
    for (i, &t) in text.iter().enumerate() {
        cur[0] = (i + 1) as f32;
        for (j, &h) in heard.iter().enumerate() {
            let sub = prev[j] + if t == STAR || t == h { 0.0 } else { 1.0 };
            cur[j + 1] = sub.min(prev[j + 1] + 1.0).min(cur[j] + ins);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    let cost = prev.iter().copied().fold(f32::INFINITY, f32::min);
    (1.0 - cost / text.len() as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Emissions over a 4-letter vocabulary (0 blank) from the best letter of
    /// each frame, which gets `p`, the rest sharing what is left.
    fn em(best: &[usize], p: f32) -> Emissions {
        let vocab = 4;
        let mut logp = Vec::new();
        for &b in best {
            for i in 0..vocab {
                logp.push(if i == b { p.ln() } else { ((1.0 - p) / 3.0).ln() });
            }
        }
        Emissions { logp, vocab, fps: 50.0, blank: 0 }
    }

    #[test]
    fn letters_land_on_their_frames() {
        let e = em(&[0, 0, 1, 1, 0, 2, 0, 0, 3, 3, 3, 0], 0.9);
        let spans = force_align(&e, &[1, 2, 3]).unwrap();
        assert_eq!(spans.iter().map(|s| (s.first, s.last)).collect::<Vec<_>>(), [(2, 3), (5, 5), (8, 10)]);
        assert!(spans.iter().all(|s| (s.peak - 1.0).abs() < 1e-4));
    }

    #[test]
    fn a_repeated_letter_needs_a_blank_between() {
        let e = em(&[1, 1, 0, 1, 1], 0.9);
        let spans = force_align(&e, &[1, 1]).unwrap();
        assert_eq!((spans[0].first, spans[0].last, spans[1].first, spans[1].last), (0, 1, 3, 4));
        // Two frames cannot hold the same letter twice.
        assert!(force_align(&em(&[1, 1], 0.9), &[1, 1]).is_none());
        assert!(force_align(&em(&[1], 0.9), &[1, 2]).is_none());
    }

    #[test]
    fn a_wildcard_takes_the_best_letter_and_a_wrong_text_scores_low() {
        let e = em(&[0, 1, 0, 2, 2, 0, 3, 0], 0.95);
        let spans = force_align(&e, &[1, STAR, 3]).unwrap();
        assert_eq!((spans[1].first, spans[1].last), (3, 4));
        assert!(spans[1].peak < 0.5, "a wildcard is discounted: {}", spans[1].peak);
        // The letters the audio does not have get squeezed in, with a low peak.
        let wrong = force_align(&e, &[3, 3, 1]).unwrap();
        assert!(wrong.iter().filter(|s| s.peak < 0.1).count() >= 2, "{wrong:?}");
    }

    #[test]
    fn heard_letters_spell_the_right_text_better() {
        let e = em(&[0, 1, 1, 0, 2, 0, 3, 3, 0, 1], 0.9);
        let heard = heard_letters(&e, None);
        assert_eq!(heard, [1, 2, 3, 1]);
        assert!((letters_heard(&heard, &[1, 2, 3, 1]) - 1.0).abs() < 1e-6);
        assert!(letters_heard(&heard, &[3, 3, 2, 2]) < 0.5);
        assert!((letters_heard(&heard, &[STAR, 2]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn log_softmax_normalises_rows() {
        let mut x = vec![1.0, 2.0, 3.0, 0.0, 0.0, 0.0];
        Emissions::log_softmax(&mut x, 3);
        for row in x.chunks(3) {
            let sum: f32 = row.iter().map(|v| v.exp()).sum();
            assert!((sum - 1.0).abs() < 1e-5);
        }
    }
}
