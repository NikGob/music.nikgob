use super::*;

/// One timed piece spans its own glyph advances, even across frozen rows.
pub(super) struct Syllable {
    pub(super) start: f32,
    pub(super) end: f32,
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) letters: usize,
    /// Its whitespace-delimited word ([`Timing::words`]).
    pub(super) word: usize,
    /// Its first and one past its last glyph.
    pub(super) glyphs: (usize, usize),
}

/// A written unit for «По словам»: a word, or a part of a hyphenated word
/// as it is written («do-», «dodge»), from its first letter's time to its
/// last one's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Unit {
    pub(super) start: f32,
    pub(super) end: f32,
}

pub(super) struct Timing {
    pub(super) syllables: Vec<Syllable>,
    /// Syllable index for each glyph (spaces have none).
    pub(super) glyphs: Vec<Option<usize>>,
    pub(super) units: Vec<Unit>,
    /// Unit index for each glyph (spaces have none).
    pub(super) glyph_units: Vec<Option<usize>>,
    /// Each letter's own part of its syllable's time, weighted by
    /// [`letter_weight`] (spaces have none).
    #[cfg(test)]
    pub(super) letter_times: Vec<Option<(f32, f32)>>,
    /// When each letter starts to light (and to jump) under the look the
    /// timing is for: as the front's way comes to it in the wave, or to its
    /// syllable or written word where those light whole; never before a
    /// letter written ahead of it. None for spaces, and for every letter
    /// where nothing lights by itself: karaoke follows the front, Focus by
    /// letters lights the whole line.
    pub(super) lit: Vec<Option<f32>>,
    /// Each glyph's span on the line's run of advances.
    #[cfg(test)]
    pub(super) xs: Vec<(f32, f32)>,
    /// First syllable of each whitespace-delimited word.
    pub(super) words: Vec<usize>,
    /// The way the light goes along the line.
    pub(super) front: Front,
}

/// The light's way along a line under a look: a [`Run`] for the pieces sung
/// one after another without a pause (a whole line, when it is sung in one
/// breath), each with a front of its own that crosses it in one motion: no
/// stop and no step in its speed at a letter, a syllable or a word, and
/// never back. It waits only where the singing does, at the end of a run,
/// with the run lit whole and the next one not begun.
///
/// What it keeps time by is the look's unit: by syllables the timed
/// pieces, each crossed evenly; by letters those with the ends of their
/// letters as the letters' weights guess them (a held vowel draws out); by
/// words each written word, crossed evenly from its first sound to its
/// last.
pub(super) struct Front {
    pub(super) runs: Vec<Run>,
    /// Run index for each glyph (spaces have none).
    pub(super) of: Vec<Option<usize>>,
    /// The width of the light's soft edge, px: the front is its middle.
    pub(super) edge: f32,
}

impl Front {
    /// The front for `look` over a line's pieces, and when each letter
    /// starts to light under it ([`Timing::lit`]).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn over(
        look: Look,
        syllables: &[Syllable],
        glyphs: &[Option<usize>],
        units: &[Unit],
        glyph_units: &[Option<usize>],
        times: &[Option<(f32, f32)>],
        xs: &[(f32, f32)],
        edge: f32,
        ease: Ease,
    ) -> (Self, Vec<Option<f32>>) {
        let n = glyphs.len();
        // Focus by letters lights the line whole: nothing goes along it.
        if look.style == Style::Focus && look.by == By::Letters {
            return (Front { runs: Vec::new(), of: vec![None; n], edge: 0.0 }, vec![None; n]);
        }
        let sweeping = look.style == Style::Karaoke;
        // Only karaoke draws the front's edge; the other looks take its
        // times.
        let edge = if sweeping { edge.max(0.0) } else { 0.0 };
        let owners = if look.words() { glyph_units } else { glyphs };
        let mut pieces: Vec<Piece> = Vec::new();
        let mut marks: Vec<(f32, f32)> = Vec::new();
        let mut piece_of: Vec<Option<usize>> = vec![None; n];
        let mut filling = None;
        for k in 0..n {
            let Some(o) = owners[k] else { continue };
            if filling != Some(o) {
                let (start, end) = if look.words() {
                    units.get(o).map_or((0.0, 0.0), |u| (u.start, u.end))
                } else {
                    (syllables[o].start, syllables[o].end)
                };
                pieces.push(Piece { start, end, from: xs[k].0, to: xs[k].1, marks: marks.len()..marks.len() });
                filling = Some(o);
            } else if let Some(piece) = pieces.last_mut() {
                // By letters: where each letter ends inside its piece.
                if look.by == By::Letters
                    && let Some((at, _)) = times[k]
                {
                    marks.push((xs[k].0, at));
                    piece.marks.end = marks.len();
                }
                piece.to = xs[k].1;
            }
            piece_of[k] = Some(pieces.len() - 1);
        }
        let (runs, run_of) = lyrics_front::runs(&pieces, &marks, edge, ease);
        let of = piece_of.iter().map(|p| p.map(|i| run_of[i])).collect();
        let lit = if sweeping {
            vec![None; n]
        } else {
            // The wave lights letter by letter (by words the word whole).
            let by_letter = look.style == Style::Wave && !look.words();
            (0..n)
                .map(|k| piece_of[k].map(|i| runs[run_of[i]].left(if by_letter { xs[k].0 } else { pieces[i].from })))
                .collect()
        };
        (Front { runs, of, edge }, lit)
    }

    /// The unlit end of run `run`'s soft edge at `p` seconds, px along the
    /// line: the light is whole an edge's width behind it. −∞ before the run
    /// is sung (nothing of it lit), +∞ once its front has come to rest (all
    /// of it lit); the edge is past either end of the run by then, so
    /// neither is a step.
    pub(super) fn head(&self, run: usize, p: f64) -> f32 {
        let Some(run) = self.runs.get(run) else { return f32::INFINITY };
        let (start, rest) = run.times();
        if p <= start {
            f32::NEG_INFINITY
        } else if p >= rest {
            f32::INFINITY
        } else {
            run.at(p) + 0.5 * self.edge
        }
    }
}

/// The timings of the lines being sung, kept from frame to frame: a line's
/// timing and its front's way are worked out once for its layout, look and
/// settings, not every frame.
#[derive(Clone, Default)]
pub(super) struct Timings(pub(super) Vec<(u64, Arc<Timing>)>);

/// How many timings are kept: the lines sung or fading at once, twice over
/// for a change of look.
pub(super) const TIMINGS: usize = 12;

/// [`Timing::of`], kept ([`Timings`]).
pub(super) fn timing_of(ctx: &egui::Context, l: &LyricLine, g: &Galley, look: Look, pace: Pace, size: f32) -> Option<Arc<Timing>> {
    use std::hash::{Hash, Hasher};
    if l.words.is_empty() {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (look.style as u8, look.by as u8, pace.edge(size).to_bits(), pace.smooth.to_bits()).hash(&mut h);
    for w in &l.words {
        (w.start, w.end, &w.text).hash(&mut h);
    }
    for gl in g.rows.iter().flat_map(|r| &r.row.glyphs) {
        (gl.chr, gl.advance_width.to_bits()).hash(&mut h);
    }
    let key = h.finish();
    let id = egui::Id::new("lyrics-timings");
    let kept = ctx.data_mut(|d| d.get_temp_mut_or_default::<Timings>(id).0.iter().find(|(k, _)| *k == key).map(|(_, t)| t.clone()));
    if kept.is_some() {
        return kept;
    }
    let timing = Arc::new(Timing::of(l, g, look, pace, size)?);
    ctx.data_mut(|d| {
        let kept = &mut d.get_temp_mut_or_default::<Timings>(id).0;
        if kept.len() >= TIMINGS {
            kept.remove(0);
        }
        kept.push((key, timing.clone()));
    });
    Some(timing)
}

impl Timing {
    /// The timing of `l` laid out as `g`, by letters and on its times
    /// exactly.
    #[cfg(test)]
    pub(super) fn new(l: &LyricLine, g: &Galley) -> Option<Self> {
        Self::build(l, g, Look { style: Style::Wave, by: By::Letters }, 0.0, Ease::EXACT)
    }

    /// The timing of `l` laid out as `g` at text `size`, for `look` at
    /// `pace`: «Плавность» is how closely the front keeps to the times,
    /// «Скорость подсветки» how wide the karaoke edge is.
    pub(super) fn of(l: &LyricLine, g: &Galley, look: Look, pace: Pace, size: f32) -> Option<Self> {
        Self::build(l, g, look, pace.edge(size), Ease::of(pace.smooth))
    }

    pub(super) fn build(l: &LyricLine, g: &Galley, look: Look, edge: f32, ease: Ease) -> Option<Self> {
        // By letters a letter's part of its syllable's time goes by its
        // weight; else evenly by its advance.
        let even = look.by != By::Letters;
        if l.words.is_empty() {
            return None;
        }
        let mut syllables = Vec::new();
        let mut letters = Vec::new();
        let mut words = Vec::new();
        for (i, w) in l.words.iter().enumerate() {
            if i == 0 || l.words[i - 1].text.ends_with(char::is_whitespace)
                || w.text.starts_with(char::is_whitespace)
            {
                words.push(i);
            }
            let n = w.text.chars().filter(|c| !c.is_whitespace()).count();
            syllables.push(Syllable {
                start: w.start.as_secs_f32(), end: w.end.as_secs_f32().max(w.start.as_secs_f32()),
                from: 0.0, to: 0.0, letters: n, word: words.len() - 1, glyphs: (0, 0),
            });
            letters.extend(std::iter::repeat_n(i, n));
        }
        let mut letters = letters.into_iter();
        let mut x = 0.0;
        let mut glyphs = Vec::new();
        let mut xs = Vec::new();
        let mut seen = vec![false; syllables.len()];
        let mut weights = vec![0.0f32; syllables.len()];
        for (k, gl) in g.rows.iter().flat_map(|r| &r.row.glyphs).enumerate() {
            let syllable = if gl.chr.is_whitespace() { None } else { letters.next() };
            if let Some(i) = syllable {
                if !seen[i] {
                    syllables[i].from = x;
                    syllables[i].glyphs.0 = k;
                    seen[i] = true;
                }
                syllables[i].to = x + gl.advance_width;
                syllables[i].glyphs.1 = k + 1;
                weights[i] += letter_weight(gl.chr);
            }
            glyphs.push(syllable);
            xs.push((x, x + gl.advance_width));
            x += gl.advance_width;
        }
        // Each letter's own part of its syllable's time: vowels, which
        // carry a held sound, take more of it than consonants (evenly by
        // its advance, `even`).
        let mut done = vec![0.0f32; syllables.len()];
        let times: Vec<Option<(f32, f32)>> = g.rows.iter().flat_map(|r| &r.row.glyphs).zip(&glyphs).zip(&xs)
            .map(|((gl, syllable), &(x0, x1))| {
                let i = (*syllable)?;
                let s = &syllables[i];
                if even {
                    let at = |x: f32| s.start + (s.end - s.start) * ((x - s.from) / (s.to - s.from).max(f32::EPSILON)).clamp(0.0, 1.0);
                    // The last letter ends exactly with its syllable.
                    return Some((at(x0), if x1 >= s.to - 1e-3 { s.end } else { at(x1) }));
                }
                let per = (s.end - s.start) / weights[i].max(f32::EPSILON);
                let w = letter_weight(gl.chr);
                let at = s.start + per * done[i];
                done[i] += w;
                // The last letter ends exactly with its syllable.
                let to = if done[i] >= weights[i] - 1e-4 { s.end } else { (s.start + per * done[i]).min(s.end) };
                Some((at.min(s.end), to))
            })
            .collect();
        let glyph_units = written_units(g.rows.iter().flat_map(|r| &r.row.glyphs).map(|gl| gl.chr));
        let mut spans: Vec<Option<Unit>> = vec![None; glyph_units.iter().flatten().max().map_or(0, |n| n + 1)];
        for (u, t) in glyph_units.iter().zip(&times) {
            if let (Some(u), Some((a, b))) = (*u, *t) {
                let span = spans[u].get_or_insert(Unit { start: a, end: b });
                span.start = span.start.min(a);
                span.end = span.end.max(b);
            }
        }
        // A unit none of whose letters is timed (a text that differs from
        // its timing) lights with the one before it.
        let first = syllables.first().map_or(0.0, |s| s.start);
        let mut last = Unit { start: first, end: first };
        // Letters and units light in the order they are written: a piece
        // timed to start before one written ahead of it (a timing slip)
        // waits for it, so the light never runs back along the line.
        let mut latest = f32::NEG_INFINITY;
        let units: Vec<Unit> = spans
            .into_iter()
            .map(|u| {
                last = u.unwrap_or(last);
                latest = latest.max(last.start);
                Unit { start: latest, end: last.end.max(latest) }
            })
            .collect();
        let (front, lit) = Front::over(look, &syllables, &glyphs, &units, &glyph_units, &times, &xs, edge, ease);
        Some(Self {
            syllables,
            glyphs,
            units,
            glyph_units,
            #[cfg(test)]
            letter_times: times,
            lit,
            #[cfg(test)]
            xs,
            words,
            front,
        })
    }

    /// How high (in rest heights) the written word of syllable `s` stands
    /// at `p` by syllables: it goes up with each of its syllables, a share
    /// at a time and each with its own jump, so it is up in full once the
    /// last is sung. With the seconds since that last one's onset
    /// (negative before it), for its settling onto whole pixels.
    pub(super) fn word_jump(&self, s: usize, p: f32, speed: f32, window: f32) -> (f32, f32) {
        let w = self.syllables[s].word;
        let first = self.words[w];
        let end = self.words.get(w + 1).copied().unwrap_or(self.syllables.len()).max(first + 1);
        let onset = |i: usize| {
            let sy = &self.syllables[i];
            if sy.letters == 0 { sy.start } else { self.lit.get(sy.glyphs.0).copied().flatten().unwrap_or(sy.start) }
        };
        let height = (first..end).map(|i| jump_over(p - onset(i), speed, window)).sum::<f32>() / (end - first) as f32;
        (height, p - onset(end - 1))
    }
}

/// How much of its syllable's (or word's) time a letter takes: vowels,
/// which carry a held sound, more than consonants; signs and marks little.
/// Letters that are syllables already (kana, hangul, hanzi) share evenly.
pub(super) fn letter_weight(c: char) -> f32 {
    const VOWELS: &str = "aeiouyàáâãäåæèéêëìíîïòóôõöøùúûüýÿœāăąēĕėęěīĭįıōŏőūŭůűųаеёиоуыэюяіїєўαεηιουωάέήίόύώϊϋ";
    if !c.is_alphanumeric() {
        return 0.25;
    }
    let lower = c.to_lowercase().next().unwrap_or(c);
    if VOWELS.contains(lower) {
        2.5
    } else if matches!(lower, 'ь' | 'ъ') {
        0.25
    } else {
        1.0
    }
}

/// The written unit of each char (whitespace has none): words, and the
/// parts of a hyphenated word as written, the hyphen ending its part
/// («do-dodge» is «do-» and «dodge»). A hyphen with no letter before it
/// («-то») does not split.
pub(super) fn written_units(chars: impl IntoIterator<Item = char>) -> Vec<Option<usize>> {
    let mut out = Vec::new();
    let mut count = 0;
    // The last char of the unit being read, and whether it has a letter.
    let mut prev: Option<char> = None;
    let mut lettered = false;
    for c in chars {
        if c.is_whitespace() {
            out.push(None);
            prev = None;
            continue;
        }
        let split = prev.is_some_and(|p| matches!(p, '-' | '\u{2010}' | '\u{2011}') && lettered && c.is_alphanumeric());
        if prev.is_none() || split {
            count += 1;
            lettered = false;
        }
        lettered |= c.is_alphanumeric();
        out.push(Some(count - 1));
        prev = Some(c);
    }
    out
}

/// How a letter or word moves and lights up as it is sung: «Скорость
/// прыжка» and «Скорость подсветки».
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pace {
    /// How fast a jump goes up and settles (1: Material's expressive fast
    /// spatial spring).
    pub(super) jump: f32,
    /// 0 lights up smoothly .. 1 at once.
    pub(super) light: f32,
    /// «Плавность»: 0 by the times exactly .. 1 smooth.
    pub(super) smooth: f32,
}

impl Pace {
    pub(super) fn of(prefs: &Prefs) -> Self {
        Pace {
            jump: prefs.lyrics_jump_speed.clamp(0.5, 2.0),
            light: prefs.lyrics_light_speed.clamp(0.0, 1.0),
            smooth: prefs.lyrics_smooth.clamp(0.0, 1.0),
        }
    }

    /// The seconds every motion is averaged over («Плавность»): a letter's
    /// pop or step spreads over it, so neighbouring letters move together.
    pub(super) fn window(self) -> f32 {
        self.smooth * SMOOTH_WINDOW
    }

    /// Seconds a letter or word takes to light up (90 % of the way), never
    /// less than [`LIGHT_FLOOR`]; focus brightens [`FOCUS_CALM`] times slower.
    pub(super) fn rise(self, style: Style) -> f32 {
        let rise = (0.5 * (1.0 - self.light) * (1.0 - self.light)).max(LIGHT_FLOOR);
        if style == Style::Focus { rise * FOCUS_CALM } else { rise }
    }

    /// The soft edge of the karaoke front at text `size`, which is how long
    /// a letter it crosses takes to light up: about a letter wide at the
    /// default «Скорость подсветки» (a letter is some 0.55 of the size), a
    /// fifth of one at once (still no hard cut), two and a half at the
    /// smoothest.
    pub(super) fn edge(self, size: f32) -> f32 {
        size * (0.12 + 1.3 * (1.0 - self.light))
    }
}

/// The shortest a letter or word takes to light up, at the fastest
/// «Скорость подсветки»: three frames or so, a quick ease where lighting at
/// once was a switch.
pub(super) const LIGHT_FLOOR: f32 = 0.05;
/// Focus brightens words this many times slower than the others light them.
pub(super) const FOCUS_CALM: f32 = 2.2;
/// How lit (0..1 from the unsung tone to the lit one) the whole current
/// line is under Focus by syllables or words: readable, and plainly dimmer
/// than the piece being sung, which alone is lit in full.
pub(super) const FOCUS_LINE: f32 = 0.3;
/// Seconds a piece takes to go back to the line's tone once it is sung.
pub(super) const FOCUS_FALL: f32 = 0.25;
/// A piece stands out for at least this long, however short it is sung.
pub(super) const FOCUS_HOLD: f32 = 0.25;

/// How much (0..1) a piece sung from `start` to `end` stands out at `p`
/// under Focus: up as it starts (see [`light_over`]), back down over
/// [`FOCUS_FALL`] once it is sung. Nothing stays lit behind it: Focus has
/// no fill.
pub(super) fn focus_pulse(p: f32, start: f32, end: f32, rise: f32, window: f32) -> f32 {
    let end = end.max(start + FOCUS_HOLD) + 0.5 * window;
    let down = ((p - end) / FOCUS_FALL).clamp(0.0, 1.0);
    light_over(p - start, rise, window) * (1.0 - down * down * (3.0 - 2.0 * down))
}

/// The averaging window at full «Плавность», in seconds: two or three sung
/// letters.
pub(super) const SMOOTH_WINDOW: f32 = 0.4;

/// «Плавность» for the lights and jumps of the wave and of Focus: the
/// window each is spread over. Karaoke has none: its front takes
/// «Плавность» as how closely it keeps to the times ([`Ease`]).
pub(super) fn smooth_window(look: Look, pace: Pace) -> f32 {
    if look.style == Style::Karaoke { 0.0 } else { pace.window() }
}

/// A letter or word starts lighting up (and jumping) this long before its
/// time, so the motion lands on the voice.
pub(super) const ONSET_LEAD: f32 = 0.03;

/// How lit (0..1) a letter or word is `t` seconds after its onset: from just
/// before it, up on a critically damped spring that is 90 % there after
/// `rise` seconds (at once for 0). Only ever rises.
pub(super) fn light_up(t: f32, rise: f32) -> f32 {
    let t = t + ONSET_LEAD;
    if t <= 0.0 {
        return 0.0;
    }
    if rise < 0.005 {
        return 1.0;
    }
    // (1 + ωt)·e^(−ωt) falls to 0.1 at ωt ≈ 3.89.
    let w = 3.89 / rise;
    (1.0 - Spring::new(1.0, w * w).solve(1.0, 0.0, t).0).clamp(0.0, 1.0)
}

/// [`light_up`] averaged over `window` seconds around `t` («Плавность»),
/// exactly (by its integral): it starts half a window early and takes a
/// window longer, still rising only and continuous even when lighting at once.
pub(super) fn light_over(t: f32, rise: f32, window: f32) -> f32 {
    if window <= 0.0 {
        return light_up(t, rise);
    }
    // ∫ light_up from its start: s + (2 + ωs)·e^(−ωs)/ω − 2/ω, or s at once.
    let area = |t: f32| {
        let s = t + ONSET_LEAD;
        if s <= 0.0 {
            0.0
        } else if rise < 0.005 {
            s
        } else {
            let w = 3.89 / rise;
            s + ((2.0 + w * s) * (-w * s).exp() - 2.0) / w
        }
    };
    ((area(t + window * 0.5) - area(t - window * 0.5)) / window).clamp(0.0, 1.0)
}

/// [`jump_height`] averaged over `window` seconds around `t`: the pop
/// spreads into a gentle rise, so the letters around the one being sung
/// rise and settle with it, a bump travelling along the line.
pub(super) fn jump_over(t: f32, speed: f32, window: f32) -> f32 {
    const SAMPLES: usize = 9;
    if window <= 0.0 {
        return jump_height(t, speed);
    }
    (0..SAMPLES).map(|i| jump_height(t - window * 0.5 + window * i as f32 / (SAMPLES - 1) as f32, speed)).sum::<f32>()
        / SAMPLES as f32
}

/// How high a jump is `t` seconds after its onset, in rest heights: off the
/// line at once with a kick, over the top (about 1.6) some 70 ms later, then
/// down onto its rest height (1) on Material's expressive fast spatial
/// spring; `speed` times faster or slower.
pub(super) fn jump_height(t: f32, speed: f32) -> f32 {
    let t = t + ONSET_LEAD;
    if t <= 0.0 {
        return 0.0;
    }
    let k = speed.clamp(0.25, 4.0);
    let base = nikgob_motion::tokens::EXPRESSIVE_FAST_SPATIAL;
    let spring = Spring::new(base.damping_ratio, base.stiffness * k * k);
    1.0 + spring.solve(-1.0, JUMP_KICK * k, t).0
}

/// The jump's upward speed at its onset, in rest heights a second.
pub(super) const JUMP_KICK: f32 = 60.0;
/// A jump is averaged over at least this many seconds, whatever «Плавность»
/// is: its kick comes in over a few frames instead of in one.
pub(super) const JUMP_EASE: f32 = 0.05;
/// A jump at speed 1 is at rest (within a hundredth of a pixel) after this.
pub(super) const JUMP_SETTLE: f32 = 0.4;
/// A sung wave letter (or word) rests this high, in text sizes, at jump 1.
pub(super) const RISE: f32 = 0.15;

/// How far (0..1) a wave lift is drawn on whole pixels: from none while it
/// moves to all within [`CRISP`] seconds of coming to rest (`rested`
/// seconds ago, negative while it still moves), and back to none as its
/// line's `hold` leaves 1 (it moves again then, going down).
pub(super) fn crisp_lift(rested: f32, hold: f32) -> f32 {
    let smooth = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        x * x * (3.0 - 2.0 * x)
    };
    smooth(rested / CRISP) * smooth((hold - 0.9) / 0.1)
}

/// Seconds a lift at rest takes to settle onto whole pixels.
pub(super) const CRISP: f32 = 0.15;

/// How long a held note's extra glow takes to settle after it ends.
pub(super) const SETTLE: f32 = 0.3;
/// Seconds a held note takes to come up to what sets it apart.
pub(super) const HELD_IN: f32 = 0.35;
/// How far (0..1 from the unsung tone to the lit one) the letters of a held
/// note brighten ahead of the karaoke front while the note is held: the
/// front goes through it slowly, and the note reads as held, not as stuck.
pub(super) const HELD_WARM: f32 = 0.25;

pub(super) fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// How much of a held note (0..1) a piece sung over `secs` seconds with
/// `letters` letters is: none at a speaking pace, whole from 1.2 s on with
/// a quarter of a second or more a letter (a long word sung at its usual
/// pace is none). No threshold: a note a little shorter is set apart a
/// little less.
pub(super) fn held_note(secs: f32, letters: usize) -> f32 {
    smoothstep((secs - 0.6) / 0.6) * smoothstep((secs / letters.max(1) as f32 - 0.12) / 0.13)
}

/// How far (0..1) a note held from `start` has come up at `pos`.
pub(super) fn held_in(start: f32, pos: f32) -> f32 {
    smoothstep((pos - start) / HELD_IN)
}

/// 1 while the note is held, then down to 0 over [`SETTLE`].
pub(super) fn held_envelope(end: f32, pos: f32) -> f32 {
    smoothstep(1.0 - (pos - end) / SETTLE)
}

#[allow(clippy::too_many_arguments)]
/// `pos` is where the line is sung up to: the position while it is
/// current, and after that on as it fades ([`sung_pos`]), so nothing in it
/// jumps when it stops being current; it only goes back with `hold`: how
/// far (0..1) its sung pieces are lit (and its wave pieces up), 1 while it
/// is current and down with its focus after that. `timing` is the line's
/// for `look` ([`Timing::of`]).
pub(super) fn timed_line(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, timing: &Timing, pos: Duration,
    focus: f32, hold: f32, size: f32, look: Look, base: Color32, lit: Color32, prefs: &Prefs, fade: Option<Fade>)
{
    let style = look.style;
    let pace = Pace::of(prefs);
    // A piece under Focus lights as fast as any other (the calm rise is the
    // whole line's, by letters).
    let rise = pace.rise(if style == Style::Focus { Style::Wave } else { style });
    let window = smooth_window(look, pace);
    let p = pos.as_secs_f32();
    // Karaoke: each run of pieces is lit up to its own soft front, whatever
    // the step ([`Front`]); a piece never lights at once.
    let sweeping = style == Style::Karaoke;
    let front = &timing.front;
    let at = pos.as_secs_f64();
    let theme = Theme::get(painter.ctx());
    let glow_color = crate::lerp_color(lit, theme.scheme.primary, 0.3);
    let ppp = super::super::stage::ppp(painter.ctx());
    let hold = hold.clamp(0.0, 1.0);
    // Karaoke: the sung words float up as the light comes to them
    // («Подъём спетого»), a whole number of pixels once lit, and go down
    // with the line's hold. Reduced motion keeps them on the line.
    let float_up = if sweeping && !theme.reduced_motion {
        (KARAOKE_RISE * size * prefs.lyrics_lift.clamp(0.0, 1.0) * ppp).round() / ppp * hold
    } else {
        0.0
    };
    let n = timing.glyphs.len();
    let mut inks = Vec::with_capacity(n);
    let mut glows = Vec::with_capacity(n);
    // The run of the last glyph that had one, and where its front is: a
    // glyph with no run of its own (a space, a letter past the timed text)
    // goes with the run before it.
    let mut run = (usize::MAX, f32::NEG_INFINITY);
    for (k, glyph) in timing.glyphs.iter().enumerate() {
        let syllable = glyph.map(|s| &timing.syllables[s]);
        // A note held long stands out while it is held: under karaoke, and
        // in every look by syllables. It goes with the line's hold.
        let held = syllable
            .filter(|s| (sweeping || look.by == By::Syllables) && p > s.start)
            .map_or(0.0, |s| held_note(s.end - s.start, s.letters) * held_in(s.start, p) * hold);
        let swell = syllable.map_or(0.0, |s| held * held_envelope(s.end, p));
        // When the piece this glyph goes with starts to light.
        let onset = timing.lit.get(k).copied().flatten();
        // Focus has no fill: the line is bright as a whole and the piece
        // being sung a little brighter, only while it is sung.
        let on = onset.map(|at| match piece_end(timing, k, look).filter(|_| style == Style::Focus) {
            Some(end) => FOCUS_LINE + (1.0 - FOCUS_LINE) * focus_pulse(p, at, end, rise, window),
            None => light_over(p - at, rise, window),
        });
        // Karaoke and Focus keep one baseline: sung and unsung words never
        // drift apart (that read as broken text), a word with a held note
        // too (it glows instead). Reduced motion keeps the wave down as
        // well (it only lights up).
        // How much (0..1) of the wave's crest this glyph is on.
        let mut crest = 0.0;
        // How much bigger the glyph is drawn: a wave piece swells a little
        // on its pop.
        let mut grow = 0.0;
        let lift = match onset {
            Some(at) if style == Style::Wave && !theme.reduced_motion => {
                // The jump's speed scales its smoothing window too; with
                // no window it still eases off the line ([`JUMP_EASE`]).
                let spread = (window / pace.jump).max(JUMP_EASE);
                // By syllables the whole written word goes up with them.
                let (height, t) = match glyph.filter(|_| look.by == By::Syllables) {
                    Some(s) => timing.word_jump(s, p, pace.jump, spread),
                    None => (jump_over(p - at, pace.jump, spread), p - at),
                };
                // Up while it is sung, then down to where sung text rests:
                // the crest goes along the line with the voice, and a held
                // note stays up for as long as it is held.
                // It comes down on a spring that lands a touch below where
                // sung text rests and bounces back ([`land`]).
                let sinks = wave_end(timing, k, look).map_or(f32::INFINITY, |end| end + WAVE_STAY);
                let sunk = land(p - sinks, pace.jump);
                crest = (1.0 - sunk).clamp(0.0, 1.0);
                let lift = height * (1.0 - (1.0 - WAVE_REST) * sunk) * RISE * size * prefs.lyrics_wave_jump.max(0.0) * hold;
                // The piece's own pop over the top swells it a little.
                let pop = (jump_over(p - at, pace.jump, spread) - 1.0).max(0.0);
                grow = (JUMP_SWELL * pop).min(SWELL_MAX) * crest * hold * prefs.lyrics_wave_jump.clamp(0.0, 1.0);
                // At rest on whole pixels, so a raised word is as crisp as
                // the rest; in motion to the fraction, so it never steps.
                // It eases between the two, so it never steps onto or off
                // the pixel either: off them before it sinks, onto them
                // again once it is down.
                let up = crisp_lift(t + ONSET_LEAD - (JUMP_SETTLE / pace.jump + spread), hold) * smoothstep((sinks - p) / WAVE_STAY);
                let crisp = up.max(crisp_lift(p - (sinks + WAVE_FALL / pace.jump), hold));
                lift + ((lift * ppp).round() / ppp - lift) * crisp
            }
            _ => 0.0,
        };
        if sweeping
            && let Some(r) = front.of.get(k).copied().flatten()
            && r != run.0
        {
            run = (r, front.head(r, at));
        }
        // A glyph of the line the front goes along: in `ahead` until its
        // run's front comes to it and in `behind` once the front is past.
        let head = run.1;
        // Lit, it has floated up by the whole `float_up`; under the front's
        // soft edge by as much as the light at its middle.
        let swept = |ahead: Color32, behind: Color32| {
            if head == f32::INFINITY {
                Ink::solid(behind, lift + float_up)
            } else if head == f32::NEG_INFINITY {
                Ink::solid(ahead, lift)
            } else {
                Ink { lift, base: ahead, lit: behind, head, feather: front.edge, rise: float_up, grow: 0.0 }
            }
        };
        let ink = match on {
            Some(on) => Ink::solid(crate::lerp_color(base, lit, on * hold), lift),
            // A held note warms up ahead of the front, and only ever gets
            // brighter: the front lights it whole as it goes through.
            None if sweeping => swept(crate::lerp_color(base, lit, HELD_WARM * held), lit),
            None => Ink::solid(lit, lift),
        };
        inks.push(Ink { grow, ..ink });
        let strength = focus * (0.1 + 0.06 * swell);
        let glow_ink = |strength: f32| glow_color.gamma_multiply(strength.clamp(0.0, 1.0));
        let glow_of = match (on, style) {
            // The wave's light is strongest on its crest.
            (Some(on), Style::Wave) => Ink::solid(glow_ink(strength * on * (WAVE_GLOW_REST + (WAVE_GLOW_CREST - WAVE_GLOW_REST) * crest)), lift),
            (Some(on), _) => Ink::solid(glow_ink(strength * on), lift),
            (None, Style::Karaoke) => swept(Color32::TRANSPARENT, glow_ink(strength)),
            (None, Style::Focus) if !look.words() => Ink::solid(glow_ink(strength), lift),
            _ => Ink::solid(Color32::TRANSPARENT, lift),
        };
        glows.push(Ink { grow, ..glow_of });
    }
    // Every glyph is drawn once, whole, in its own place and colour: no clip
    // edge between pieces, so no seam, cut overhang or second copy shows.
    // In a soft band at the view's edge the colours thin by height too.
    let put = |pos: egui::Pos2, g: Arc<Galley>, fallback: Color32| {
        let (pos, g) = afloat(painter, pos, &g);
        let g = through(&g, pos, None, fade).unwrap_or(g);
        painter.galley(pos, g, fallback);
    };
    if prefs.lyrics_glow && glows.iter().any(|i| i.lit.a() > 0) {
        let g = inked(g, |k| glows[k]);
        for offset in GLOW {
            put(line.min + offset, g.clone(), glow_color);
        }
    }
    put(line.min, inked(g, |k| inks[k]), lit);
}

/// When the piece glyph `k` lights with under `look` ([`Timing::lit`]) is
/// sung: its written unit by words, else its syllable.
pub(super) fn piece_end(timing: &Timing, k: usize, look: Look) -> Option<f32> {
    if look.words() {
        timing.glyph_units.get(k).copied().flatten().map(|u| timing.units[u].end)
    } else {
        timing.glyphs.get(k).copied().flatten().map(|s| timing.syllables[s].end)
    }
}

/// When the piece glyph `k` rises with in the wave is sung: its letter's
/// syllable by letters, its whole written word by syllables (the word goes
/// up with them) and by words.
pub(super) fn wave_end(timing: &Timing, k: usize, look: Look) -> Option<f32> {
    let s = timing.glyphs.get(k).copied().flatten();
    match look.by {
        By::Letters => s.map(|s| timing.syllables[s].end),
        By::Words => piece_end(timing, k, look).or_else(|| s.map(|s| timing.syllables[s].end)),
        By::Syllables => s.map(|s| {
            let w = timing.syllables[s].word;
            let first = timing.words[w];
            let end = timing.words.get(w + 1).copied().unwrap_or(timing.syllables.len()).max(first + 1);
            timing.syllables[end - 1].end
        }),
    }
}

/// A sung wave piece stays on the crest this long after it is sung.
pub(super) const WAVE_STAY: f32 = 0.08;
/// Seconds it takes to come down from the crest to where sung text rests.
pub(super) const WAVE_FALL: f32 = 0.45;
/// Where sung text rests, as a share of the crest's height.
pub(super) const WAVE_REST: f32 = 0.35;
/// The glow of sung text at rest and on the crest, against the other looks'.
pub(super) const WAVE_GLOW_REST: f32 = 0.6;
pub(super) const WAVE_GLOW_CREST: f32 = 1.8;
/// How much a wave piece swells for each rest height its pop goes over
/// the top (the pop goes some 0.6 over), and at most.
pub(super) const JUMP_SWELL: f32 = 0.12;
pub(super) const SWELL_MAX: f32 = 0.08;
/// How high sung karaoke text floats at «Подъём спетого» 100 %, in text
/// sizes: two or three pixels.
pub(super) const KARAOKE_RISE: f32 = 0.06;

/// How far (0..1, a little past 1 on its bounce) a wave piece has come
/// down from its crest `t` seconds after it starts down: on a spring with
/// the expressive spatial springs' bounce (fast spatial damping, default
/// spatial stiffness), which lands a touch below where sung text rests and
/// settles back within [`WAVE_FALL`]; `speed` times faster or slower.
pub(super) fn land(t: f32, speed: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    let k = speed.clamp(0.25, 4.0);
    let spring = Spring::new(
        nikgob_motion::tokens::EXPRESSIVE_FAST_SPATIAL.damping_ratio,
        nikgob_motion::tokens::EXPRESSIVE_DEFAULT_SPATIAL.stiffness * k * k,
    );
    1.0 + spring.solve(-1.0, 0.0, t).0
}

/// Where the glow's copies of a line go.
pub(super) const GLOW: [egui::Vec2; 3] = [vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(0.0, -1.0)];

/// A soft light around the whole line `g`: copies of it in `color` at
/// `strength`, a pixel to either side and above.
pub(super) fn glow(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, color: Color32, strength: f32, fade: Option<Fade>) {
    if strength <= 0.001 {
        return;
    }
    for offset in GLOW {
        fade_galley(painter, line.min + offset, g, color.gamma_multiply(strength), fade);
    }
}

/// The small line under a lyric line: its translation, else romanization.
pub(super) fn sub(l: &LyricLine) -> Option<&str> {
    l.translation.as_deref().or(l.romanization.as_deref())
}

#[cfg(test)]
pub(super) fn glyph_count(g: &Galley) -> usize {
    g.rows.iter().map(|r| r.row.glyphs.len()).sum::<usize>().max(1)
}

/// How one glyph is painted: raised by `lift` px and lit along the line: in
/// `lit` up to `head - feather` px of its advances (rows flattened), fading
/// to `base` at `head` (a hard edge when `feather` is 0). It floats up
/// `rise` px more as lit as its middle is, and is drawn `grow` bigger
/// about the middle of its foot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Ink {
    pub(super) lift: f32,
    pub(super) base: Color32,
    pub(super) lit: Color32,
    pub(super) head: f32,
    pub(super) feather: f32,
    pub(super) rise: f32,
    pub(super) grow: f32,
}

impl Ink {
    pub(super) fn solid(color: Color32, lift: f32) -> Self {
        Ink { lift, base: color, lit: color, head: f32::INFINITY, feather: 0.0, rise: 0.0, grow: 0.0 }
    }

    pub(super) fn uniform(self) -> bool {
        self.base == self.lit || self.head == f32::INFINITY
    }

    /// How lit (0..1) the line is `x` px along its advances.
    pub(super) fn light(self, x: f32) -> f32 {
        if self.head == f32::INFINITY {
            1.0
        } else if self.feather <= 0.0 {
            f32::from(x < self.head)
        } else {
            ((self.head - x) / self.feather).clamp(0.0, 1.0)
        }
    }

    pub(super) fn at(self, x: f32) -> Color32 {
        if self.uniform() { self.lit } else { crate::lerp_color(self.base, self.lit, self.light(x)) }
    }
}

/// `g` with each glyph (numbered over all rows) painted as `ink` says, as
/// one galley for one draw: vertex colours instead of clip rects, which the
/// renderer rounds to whole pixels (a front that stepped, slivers of a
/// neighbour's ink in the wrong colour or height) and which cut glyphs that
/// reach past their advance.
pub(super) fn inked(g: &Galley, ink: impl Fn(usize) -> Ink) -> Arc<Galley> {
    let mut out = g.clone();
    let (mut k, mut x) = (0, 0.0);
    let mut bounds = Rect::NOTHING;
    let (mut vertices, mut indices) = (0, 0);
    for placed in &mut out.rows {
        let row = &placed.row;
        let src = &row.visuals.mesh;
        let mut mesh = egui::epaint::Mesh::with_texture(src.texture_id);
        let mut firsts = Vec::with_capacity(row.glyphs.len());
        for gl in &row.glyphs {
            firsts.push(mesh.vertices.len() as u32);
            // The line's run of advances at this glyph's x is `along` + x.
            let along = x - gl.pos.x;
            x += gl.advance_width;
            let i = k;
            k += 1;
            if gl.uv_rect.is_nothing() {
                continue;
            }
            let first = gl.first_vertex as usize;
            if let Some(q) = src.vertices.get(first..first + 4) {
                ink_quad(&mut mesh, [q[0], q[1], q[2], q[3]], along, ink(i));
            }
        }
        let mut new = (**row).clone();
        // Each glyph's first vertex is that of its first piece now.
        for (gl, first) in new.glyphs.iter_mut().zip(firsts) {
            gl.first_vertex = first;
        }
        new.visuals.mesh_bounds = mesh.calc_bounds();
        new.visuals.glyph_index_start = 0;
        new.visuals.glyph_vertex_range = 0..mesh.vertices.len();
        bounds = bounds.union(new.visuals.mesh_bounds.translate(placed.pos.to_vec2()));
        vertices += mesh.vertices.len();
        indices += mesh.indices.len();
        new.visuals.mesh = mesh;
        placed.row = Arc::new(new);
    }
    out.mesh_bounds = bounds;
    out.num_vertices = vertices;
    out.num_indices = indices;
    Arc::new(out)
}

/// One glyph's quad (left top, right top, left bottom, right bottom, as
/// egui lays them out) into `mesh`, raised by `ink.lift`; where the light's
/// gradient bends inside it, it is split there, so the linearly blended
/// vertex colours draw the gradient exactly.
pub(super) fn ink_quad(mesh: &mut egui::epaint::Mesh, q: [egui::epaint::Vertex; 4], along: f32, ink: Ink) {
    let (left, right) = (q[2].pos.x, q[3].pos.x);
    // The whole glyph goes up together: its line's lift and its rise by the
    // light at its middle.
    let lift = if ink.rise == 0.0 { ink.lift } else { ink.lift + ink.rise * ink.light(along + 0.5 * (left + right)) };
    let foot = q[2].pos.lerp(q[3].pos, 0.5);
    let mut cuts = [0.0f32; 4];
    let mut n = 0;
    if !ink.uniform() && right > left {
        for edge in [ink.head - ink.feather, ink.head] {
            let t = (edge - along - left) / (right - left);
            if t > cuts[n] && t < 1.0 {
                n += 1;
                cuts[n] = t;
            }
        }
    }
    n += 1;
    cuts[n] = 1.0;
    // Each piece's colour is taken just inside it, so a hard edge stays hard.
    let point = |t: f32, top: bool, inward: f32| {
        let (a, b) = if top { (q[0], q[1]) } else { (q[2], q[3]) };
        let at = a.pos.lerp(b.pos, t);
        let at = if ink.grow == 0.0 { at } else { foot + (at - foot) * (1.0 + ink.grow) };
        egui::epaint::Vertex {
            pos: at - vec2(0.0, lift),
            uv: a.uv.lerp(b.uv, t),
            color: ink.at(along + left + (right - left) * t + inward),
        }
    };
    for w in cuts[..=n].windows(2) {
        let idx = mesh.vertices.len() as u32;
        mesh.vertices.extend([point(w[0], true, 1e-3), point(w[1], true, -1e-3), point(w[0], false, 1e-3), point(w[1], false, -1e-3)]);
        mesh.indices.extend([idx, idx + 1, idx + 2, idx + 2, idx + 1, idx + 3]);
    }
}

/// The line lit left to right (row by row when it wraps) up to `head`
/// pixels of its glyphs (rows flattened), the edge of the light a soft
/// gradient.
#[cfg(test)]
pub(super) fn karaoke(painter: &egui::Painter, g: &Arc<Galley>, line: Rect, head: f32, feather: f32, base: Color32, lit: Color32) {
    if head.is_infinite() {
        painter.galley_with_override_text_color(line.min, g.clone(), lit);
        return;
    }
    painter.galley(line.min, inked(g, |_| Ink { lift: 0.0, base, lit, head, feather, rise: 0.0, grow: 0.0 }), lit);
}
