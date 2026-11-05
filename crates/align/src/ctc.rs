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

    /// Averages `other`'s probabilities into these, frame by frame (the
    /// same model heard over windows cut elsewhere); frames `other` lacks
    /// stay as they are.
    pub fn merge(&mut self, other: &Self) {
        if other.vocab != self.vocab {
            return;
        }
        let n = self.frames().min(other.frames()) * self.vocab;
        for (a, b) in self.logp[..n].iter_mut().zip(&other.logp[..n]) {
            let m = a.max(*b);
            *a = m + (((*a - m).exp() + (*b - m).exp()) / 2.0).ln();
        }
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

/// What a token of a [`Path`] is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tok {
    /// A letter of the model (its id), or [`STAR`].
    Letter(i32),
    /// Voice the text does not have (a hum, an ad-lib, a line sung once
    /// more): any letter, discounted more than a wildcard, and skipped
    /// when there is none. Never first, never two in a row.
    Filler,
    /// A hummed interjection («м», «угу»): where the voice is it takes the
    /// frames the model hears as blank or as any letter, elsewhere it
    /// costs; it lasts at least [`HUM_FRAMES`].
    Hum,
}

/// What a frame a [`Tok::Filler`] takes costs against its best letter.
const FILL_PENALTY: f32 = 0.7;
/// … and going into one at all, so short gaps stay blank.
const FILL_ENTER: f32 = 1.0;
/// A blank frame inside a filler costs this against the blank (one filler
/// may take two repeats with a pause between them).
const FILL_BLANK: f32 = 0.05;
/// A hum takes at least this many frames (0.16 s at 50 a second).
pub const HUM_FRAMES: usize = 8;
/// Where the voice is, a hum's frame is this much likelier than the
/// blank's, so it covers the hummed voice rather than leaving it blank.
const HUM_BONUS: f32 = 0.2;
/// A hum's frame without voice costs this against the blank.
const HUM_SILENT: f32 = 3.0;
/// A hum starts where the voice does: starting more than [`ONSET`] frames
/// after the voice came in costs this (two hums never share one breath).
const HUM_LATE: f32 = 2.0;
const ONSET: usize = 6;

/// A line's first token starting further than this from where its source
/// times put it (seconds) costs ([`PULL`] a second beyond it).
const SLACK: f32 = 0.5;
pub const PULL: f32 = 1.0;

/// What the blank after a token of a [`Path`] is, for what a frame of it
/// costs ([`Costs`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gap {
    /// Between lines (or unknown): free.
    #[default]
    Lines,
    /// Between two words of a line.
    Words,
    /// Between two letters of one word.
    InWord,
}

/// What frames of some states cost beyond what the model hears, and what
/// starting a word where the voice comes in gains (natural log, a frame).
/// All zero: the plain Viterbi of `align-4`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Costs {
    /// A blank frame between two letters of a word, with voice.
    pub in_word: f32,
    /// … without voice: a word is not split over a pause or a drop.
    pub in_word_silent: f32,
    /// A frame without voice between two words of a line (its blank or
    /// its word separator): a line does not stretch over two sung
    /// repeats with a pause between, the pause goes between lines.
    pub line_pause: f32,
    /// Starting a word where the voice comes in gains this (times how
    /// sharply it comes in, 0..1): short shouted syllables the model
    /// hardly hears still start where they are sung.
    pub onset: f32,
    /// A letter may take a frame where the model hears a similar vowel
    /// ([`Path::similar`]) at this under that vowel (0: off).
    pub vowels: f32,
    /// A filler frame costs this much more where the voice is the lead
    /// (sung in the middle, [`Voice::backing`] low): voice the text lacks
    /// is mostly backing vocals off the centre.
    pub lead: f32,
}

/// A token sequence to align with what the plain sequence has not: frames
/// each token must stay within (a line between its source times), optional
/// fillers and hums ([`align_path`]).
#[derive(Clone, Debug, Default)]
pub struct Path {
    pub toks: Vec<Tok>,
    /// First and last frame each token may take (inclusive), if limited.
    pub bands: Vec<Option<(usize, usize)>>,
    /// The frame where a token that starts a line is expected to start
    /// (its source time): starting far from it costs,
    pub at: Vec<Option<usize>>,
    /// … that much a second beyond [`SLACK`].
    pub pull: Vec<f32>,
    /// What the blank after each token is.
    pub gaps: Vec<Gap>,
    /// Tokens that start a word.
    pub starts: Vec<bool>,
    /// Tokens whose own frames are a pause between words (a word
    /// separator inside a line).
    pub pauses: Vec<bool>,
    /// Letters heard for each other ([`Costs::vowels`]), both ways.
    pub similar: Vec<(i32, i32)>,
    pub costs: Costs,
}

impl Path {
    pub fn push(&mut self, tok: Tok, band: Option<(usize, usize)>) {
        self.push_at(tok, band, None);
    }

    pub fn push_at(&mut self, tok: Tok, band: Option<(usize, usize)>, at: Option<usize>) {
        self.toks.push(tok);
        self.bands.push(band);
        self.at.push(at);
        self.pull.push(PULL);
        self.gaps.push(Gap::Lines);
        self.starts.push(false);
        self.pauses.push(false);
    }
}

/// The voice under the frames, where it is known (the separated voice).
#[derive(Clone, Copy, Debug)]
pub struct Voice<'a> {
    /// Which frames have the voice in them.
    pub voiced: &'a [bool],
    /// How sharply the voice comes in at or just before each frame (0..1;
    /// empty: unknown).
    pub onset: &'a [f32],
    /// How much of the voice at each frame is off the centre (0 the lead
    /// in the middle, 1 backing vocals at the sides; empty: unknown).
    pub backing: &'a [f32],
}

/// One state of the alignment graph.
struct State {
    emit: Emit,
    tok: Option<usize>,
    band: (usize, usize),
    /// Where the path may come from besides the state itself: the state
    /// and what the step costs.
    from: Vec<(usize, f32)>,
    /// Coming in at another frame than this one costs ([`SLACK`], [`PULL`]).
    at: Option<usize>,
    /// Coming in away from where the voice comes in costs ([`HUM_LATE`]).
    onset: bool,
    /// What staying a frame here costs ([`Costs`]).
    stay: Stay,
    /// Coming in here starts a word ([`Costs::onset`]).
    starts: bool,
    /// Similar letters this one may take frames of ([`Costs::vowels`]).
    alts: Vec<usize>,
    pull: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Stay {
    Free,
    InWord,
    Words,
}

#[derive(Clone, Copy)]
enum Emit {
    Blank,
    Letter(usize),
    Star,
    Filler,
    Hum,
}

/// Aligns `path` to `em` like [`force_align`], with its bands, fillers and
/// hums; `voice` says which frames have the voice in them (for hums and
/// [`Costs`]; `None`: unknown, every frame may). A token's span is `None`
/// where it was skipped (a filler). `None` when it cannot fit in its bands.
pub fn align_path(em: &Emissions, path: &Path, voice: Option<Voice<'_>>) -> Option<Vec<Option<TokenSpan>>> {
    let voiced = voice.map(|v| v.voiced);
    let costs = path.costs;
    let t_len = em.frames();
    let n = path.toks.len();
    if n == 0 || t_len == 0 {
        return Some(vec![None; n]);
    }
    let all = (0, t_len - 1);
    // The graph: a blank, then each token (a hum as a chain of states)
    // and a blank after it.
    let blank_state = |from: Vec<(usize, f32)>, stay: Stay| State { emit: Emit::Blank, tok: None, band: all, from, at: None, onset: false, stay, starts: false, alts: Vec::new(), pull: 0.0 };
    let mut states: Vec<State> = vec![blank_state(Vec::new(), Stay::Free)];
    // Each token's first and last state, and the blank after it.
    let mut first = vec![0usize; n];
    let mut last = vec![0usize; n];
    let mut after = vec![0usize; n];
    let differ = |a: Tok, b: Tok| !matches!((a, b), (Tok::Letter(x), Tok::Letter(y)) if x == y && x != STAR);
    for (k, &tok) in path.toks.iter().enumerate() {
        let band = path.bands[k].unwrap_or(all);
        let blank_before = if k == 0 { 0 } else { after[k - 1] };
        // Where this token may start from: the blank before it, the token
        // before when the two differ, and past a skipped filler.
        let mut from = vec![(blank_before, 0.0)];
        if k > 0 && differ(path.toks[k - 1], tok) {
            from.push((last[k - 1], 0.0));
        }
        if k > 1 && path.toks[k - 1] == Tok::Filler {
            from.push((after[k - 2], 0.0));
            if differ(path.toks[k - 2], tok) {
                from.push((last[k - 2], 0.0));
            }
        }
        let enter = if tok == Tok::Filler { -FILL_ENTER } else { 0.0 };
        for (_, c) in &mut from {
            *c += enter;
        }
        let emit = match tok {
            Tok::Letter(STAR) => Emit::Star,
            Tok::Letter(id) => Emit::Letter(id as usize),
            Tok::Filler => Emit::Filler,
            Tok::Hum => Emit::Hum,
        };
        let alts: Vec<usize> = match tok {
            Tok::Letter(id) if id != STAR && costs.vowels > 0.0 => path
                .similar
                .iter()
                .filter_map(|&(a, b)| if a == id { Some(b) } else if b == id { Some(a) } else { None })
                .filter(|a| *a >= 0)
                .map(|a| a as usize)
                .collect(),
            _ => Vec::new(),
        };
        let stay = if path.pauses.get(k).copied().unwrap_or(false) { Stay::Words } else { Stay::Free };
        first[k] = states.len();
        let chain = if tok == Tok::Hum { HUM_FRAMES.min(band.1.saturating_sub(band.0) + 1).max(1) } else { 1 };
        for j in 0..chain {
            let (from, at) = if j == 0 { (std::mem::take(&mut from), path.at[k]) } else { (vec![(states.len() - 1, 0.0)], None) };
            let starts = j == 0 && path.starts.get(k).copied().unwrap_or(false);
            let pull = path.pull.get(k).copied().unwrap_or(PULL);
            states.push(State { emit, tok: Some(k), band, from, at, onset: j == 0 && tok == Tok::Hum, stay, starts, alts: alts.clone(), pull });
        }
        last[k] = states.len() - 1;
        let mut from = vec![(last[k], 0.0)];
        if tok == Tok::Filler && k > 0 {
            // Skipped: from the blank before it or the token before it.
            from.push((blank_before, 0.0));
            from.push((last[k - 1], 0.0));
        }
        after[k] = states.len();
        let stay = match path.gaps.get(k).copied().unwrap_or_default() {
            Gap::Lines => Stay::Free,
            Gap::Words => Stay::Words,
            Gap::InWord => Stay::InWord,
        };
        states.push(blank_state(from, stay));
    }
    let s_len = states.len();
    if t_len.saturating_mul(s_len) > MAX_CELLS || states.iter().any(|s| s.from.len() > 254) {
        return None;
    }
    // How long the voice has been in at each frame (frames; 0 without it).
    let mut since = vec![usize::MAX; t_len];
    if let Some(v) = voiced {
        let mut run = 0;
        for (t, s) in since.iter_mut().enumerate() {
            run = if v.get(t).copied().unwrap_or(false) { run + 1 } else { 0 };
            *s = if run == 0 { usize::MAX } else { run - 1 };
        }
    }
    let blank = em.blank;
    let best: Vec<f32> = (0..t_len)
        .map(|t| em.row(t).iter().enumerate().filter(|(i, _)| *i != blank).map(|(_, v)| *v).fold(f32::NEG_INFINITY, f32::max))
        .collect();
    let emit = |t: usize, e: Emit| -> f32 {
        let row = em.row(t);
        match e {
            Emit::Blank => row[blank],
            Emit::Letter(id) => row[id],
            Emit::Star => best[t] - STAR_PENALTY,
            Emit::Filler => {
                let lead = match voice.and_then(|v| v.backing.get(t)) {
                    Some(b) if costs.lead > 0.0 => costs.lead * (1.0 - b),
                    _ => 0.0,
                };
                (best[t] - FILL_PENALTY - lead).max(row[blank] - FILL_BLANK)
            }
            Emit::Hum => {
                let heard = row[blank].max(best[t]);
                match voiced.map(|v| v.get(t).copied().unwrap_or(false)) {
                    Some(true) => heard + HUM_BONUS,
                    Some(false) => row[blank] - HUM_SILENT,
                    None => heard,
                }
            }
        }
    };
    // What a state takes from a frame: what the model hears there (a
    // similar vowel's frame discounted), less what staying there costs.
    let heard = |t: usize, st: &State| -> f32 {
        let mut v = emit(t, st.emit);
        if !st.alts.is_empty() {
            let row = em.row(t);
            for &a in &st.alts {
                v = v.max(row[a] - costs.vowels);
            }
        }
        let quiet = voiced.is_some_and(|v| !v.get(t).copied().unwrap_or(false));
        v - match st.stay {
            Stay::Free => 0.0,
            Stay::InWord if quiet => costs.in_word_silent,
            Stay::InWord => costs.in_word,
            Stay::Words if quiet => costs.line_pause,
            Stay::Words => 0.0,
        }
    };
    const NEG: f32 = -1e30;
    let mut dp = vec![NEG; s_len];
    let mut next = vec![NEG; s_len];
    let mut back = vec![0u8; t_len * s_len];
    // The path starts in the first blank or the first token.
    let in_band = |s: &State, t: usize| s.band.0 <= t && t <= s.band.1;
    for s in [0, first[0]] {
        if in_band(&states[s], 0) {
            dp[s] = heard(0, &states[s]);
        }
    }
    for t in 1..t_len {
        let row = &mut back[t * s_len..(t + 1) * s_len];
        for (s, st) in states.iter().enumerate() {
            if !in_band(st, t) {
                next[s] = NEG;
                continue;
            }
            let mut best = dp[s];
            let mut k = 0u8;
            let mut pull = st.at.map_or(0.0, |a| st.pull * ((t.abs_diff(a) as f32 / em.fps) - SLACK).max(0.0));
            if st.onset && voiced.is_some() && since[t] > ONSET {
                pull += HUM_LATE;
            }
            if st.starts && costs.onset > 0.0 {
                pull -= costs.onset * voice.and_then(|v| v.onset.get(t)).copied().unwrap_or(0.0);
            }
            for (i, (p, c)) in st.from.iter().enumerate() {
                let v = dp[*p] + c - pull;
                if v > best {
                    best = v;
                    k = i as u8 + 1;
                }
            }
            next[s] = if best <= NEG / 2.0 { NEG } else { best + heard(t, st) };
            row[s] = k;
        }
        std::mem::swap(&mut dp, &mut next);
    }
    // It ends in the last blank or the last token (before a filler at the
    // end that took nothing too).
    let mut ends = vec![s_len - 1, last[n - 1]];
    if n > 1 && path.toks[n - 1] == Tok::Filler {
        ends.push(last[n - 2]);
    }
    let mut s = ends.into_iter().fold(s_len - 1, |a, b| if dp[b] > dp[a] { b } else { a });
    if dp[s] <= NEG / 2.0 {
        return None;
    }
    let mut spans: Vec<Option<TokenSpan>> = vec![None; n];
    for t in (0..t_len).rev() {
        let st = &states[s];
        if let Some(i) = st.tok {
            let p = match st.emit {
                Emit::Letter(_) | Emit::Star => (emit(t, st.emit) - best[t]).exp().min(1.0),
                _ => 0.0,
            };
            let span = spans[i].get_or_insert(TokenSpan { first: t, last: t, peak: 0.0 });
            span.first = t;
            span.peak = span.peak.max(p);
        }
        match back[t * s_len + s] {
            0 => {}
            k => s = st.from[k as usize - 1].0,
        }
    }
    Some(spans)
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

    fn path(toks: &[Tok]) -> Path {
        let mut p = Path::default();
        for t in toks {
            p.push(*t, None);
        }
        p
    }

    fn frames(spans: &[Option<TokenSpan>]) -> Vec<Option<(usize, usize)>> {
        spans.iter().map(|s| s.map(|s| (s.first, s.last))).collect()
    }

    #[test]
    fn a_plain_path_is_the_forced_alignment() {
        let e = em(&[0, 0, 1, 1, 0, 2, 0, 0, 3, 3, 3, 0], 0.9);
        let plain = force_align(&e, &[1, 2, 3]).unwrap();
        let spans = align_path(&e, &path(&[Tok::Letter(1), Tok::Letter(2), Tok::Letter(3)]), None).unwrap();
        assert_eq!(spans, plain.into_iter().map(Some).collect::<Vec<_>>());
    }

    /// Letters the text does not have between two lines go to the filler,
    /// not to the words around them; with nothing to take it is skipped.
    #[test]
    fn a_filler_takes_what_the_text_lacks() {
        let e = em(&[0, 1, 0, 2, 0, 3, 3, 3, 3, 3, 3, 0, 1, 0, 2, 0], 0.9);
        let toks = [Tok::Letter(1), Tok::Letter(2), Tok::Filler, Tok::Letter(1), Tok::Letter(2)];
        let spans = frames(&align_path(&e, &path(&toks), None).unwrap());
        assert_eq!(spans[1], Some((3, 3)), "{spans:?}");
        assert_eq!(spans[2], Some((5, 10)), "{spans:?}");
        assert_eq!(spans[3], Some((12, 12)), "{spans:?}");
        // Nothing between the lines: no filler.
        let e = em(&[0, 1, 0, 2, 0, 0, 1, 0, 2, 0], 0.9);
        let spans = frames(&align_path(&e, &path(&toks), None).unwrap());
        assert_eq!(spans[2], None, "{spans:?}");
        assert_eq!((spans[1], spans[3]), (Some((3, 3)), Some((6, 6))));
    }

    /// A hum heard as blank lasts while the voice does, and at least
    /// [`HUM_FRAMES`] where nothing tells.
    #[test]
    fn a_hum_lasts_while_the_voice_does() {
        let mut best = vec![0, 1, 0];
        best.extend([0; 20]);
        best.extend([2, 0]);
        let e = em(&best, 0.9);
        let toks = [Tok::Letter(1), Tok::Hum, Tok::Letter(2)];
        let voiced: Vec<bool> = (0..best.len()).map(|t| (5..17).contains(&t)).collect();
        let spans = frames(&align_path(&e, &path(&toks), Some(Voice { voiced: &voiced, onset: &[], backing: &[] })).unwrap());
        assert_eq!(spans[1], Some((5, 16)), "{spans:?}");
        let spans = frames(&align_path(&e, &path(&toks), None).unwrap());
        let (a, b) = spans[1].unwrap();
        assert!(b + 1 - a >= HUM_FRAMES, "{spans:?}");
    }

    /// A token kept within its band takes the occurrence there.
    #[test]
    fn a_band_picks_the_occurrence() {
        let e = em(&[0, 1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 0], 0.9);
        let mut p = Path::default();
        p.push(Tok::Letter(1), Some((5, 9)));
        assert_eq!(frames(&align_path(&e, &p, None).unwrap()), [Some((6, 7))]);
        // A band too narrow for its tokens does not fit.
        let mut p = Path::default();
        p.push(Tok::Letter(1), Some((5, 5)));
        p.push(Tok::Letter(2), Some((5, 5)));
        assert!(align_path(&e, &p, None).is_none());
    }

    /// «Это н… он-стоп»: a word's letters are not split over seconds
    /// without voice when a blank inside a word costs there.
    #[test]
    fn a_word_is_not_split_over_silence() {
        // «1» at frame 1, «2» faintly at frame 2, clearly at frame 15;
        // the voice is there for the first five frames only.
        let mut best = vec![0, 1, 0];
        best.extend([0; 12]);
        best.extend([2, 0]);
        let e = em(&best, 0.9);
        let mut p = path(&[Tok::Letter(1), Tok::Letter(2)]);
        p.gaps[0] = Gap::InWord;
        let voiced: Vec<bool> = (0..best.len()).map(|t| t < 5).collect();
        let v = Voice { voiced: &voiced, onset: &[], backing: &[] };
        assert_eq!(frames(&align_path(&e, &p, Some(v)).unwrap())[1], Some((15, 15)));
        p.costs.in_word_silent = 1.0;
        let (a, _) = frames(&align_path(&e, &p, Some(v)).unwrap())[1].unwrap();
        assert!(a <= 4, "{a}");
    }

    /// A word starts where the voice comes in when the model cannot tell.
    #[test]
    fn a_word_starts_at_a_voice_onset() {
        // Nothing heard clearly: blanks all along, the voice coming in
        // sharply at frame 6.
        let e = em(&[0; 12], 0.5);
        let mut p = path(&[Tok::Letter(1)]);
        p.starts[0] = true;
        let voiced = vec![true; 12];
        let onset: Vec<f32> = (0..12).map(|t| if t == 6 { 1.0 } else { 0.0 }).collect();
        let v = Voice { voiced: &voiced, onset: &onset, backing: &[] };
        p.costs.onset = 2.0;
        assert_eq!(frames(&align_path(&e, &p, Some(v)).unwrap())[0].map(|s| s.0), Some(6));
    }

    /// A letter may take a frame of a similar vowel, discounted.
    #[test]
    fn a_similar_vowel_is_heard_for_the_written_one() {
        let e = em(&[0, 0, 3, 3, 0, 0], 0.97);
        let mut p = path(&[Tok::Letter(1)]);
        p.similar = vec![(1, 3)];
        let plain = frames(&align_path(&e, &p, None).unwrap())[0];
        p.costs.vowels = 0.5;
        assert_eq!(frames(&align_path(&e, &p, None).unwrap())[0], Some((2, 3)), "plain: {plain:?}");
    }

    #[test]
    fn two_hearings_are_averaged() {
        let mut a = em(&[1, 2], 0.9);
        let b = em(&[1, 3], 0.9);
        a.merge(&b);
        assert!((a.row(0)[1].exp() - 0.9).abs() < 1e-4);
        assert!((a.row(1)[2].exp() - (0.9 + 0.1 / 3.0) / 2.0).abs() < 1e-4);
        let sum: f32 = a.row(1).iter().map(|v| v.exp()).sum();
        assert!((sum - 1.0).abs() < 1e-4);
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
