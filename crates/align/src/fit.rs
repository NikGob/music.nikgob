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
use crate::ctc::{Costs, Emissions, Voice, force_align};
use crate::text::{Vocab, can_hold, is_hum, latin_by_sound, pieces, sung_by_sound};

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
/// [`TAIL`] for a word before a comma inside a line ([`Tuning::comma_ends`]).
const BREATH: f32 = 0.3;
/// A held word with no source end of its line goes on at most this long
/// past where the aligner ended it.
const HOLD: f32 = 4.0;
/// A held word goes on while the voice stays at this share of its line's
/// level («осенний листопад»: «беда» held at 0.8–1.1 of the loud level,
/// the reverb after «посильней» at 0.3–0.5 of a line sung at 0.9).
const HELD: f32 = 0.6;

/// How the timing pass places words beyond `align-4`'s rules
/// (docs/lyrics-align.md, «align-5»). [`Tuning::default`] is what the
/// player uses; [`Tuning::align4`] turns every addition off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// What frames of the timing pass cost ([`Costs`]).
    pub costs: Costs,
    /// Latin words of a Cyrillic text are spelled for the Russian aligner
    /// by how they sound («Miley Cyrus» → «майли сайрус») instead of as
    /// wildcards, and Latin lines of a mostly Russian text stay with it
    /// instead of going to the English aligner.
    pub by_sound: bool,
    /// Identical lines in a row (three or more) that came out of very
    /// different lengths are pulled this hard (a second off) towards an
    /// even share of their stretch, and aligned again (0: off).
    pub even: f32,
    /// A word squeezed to almost nothing (zero length, collapsed letters)
    /// takes a fair share of time from the word before it.
    pub squeezed: bool,
    /// A line's end taken from the next line's start this far (seconds)
    /// after its last word is no end (a long instrumental after it): the
    /// word is held as in a text without times (0: off).
    pub far_end: f32,
    /// A word ending in a stop («п», «т», «к», «б», «д», «г», «ц», «ч»)
    /// is held at most this long past where the aligner ended it: a held
    /// note is a vowel (0: off).
    pub stop_hold: f32,
    /// A tenth of a second is voice only when the separated voice is at
    /// least this share of the mix there (a synth on a drop leaks into
    /// the voice but is small against the mix; 0: off).
    pub mix_ratio: f32,
    /// The aligner hears the song twice, the second time over windows cut
    /// half a window later, and the two are averaged (a word on a window's
    /// edge is heard whole once; twice the hearing time).
    pub windows: bool,
    /// Lines heard by another aligner (a run of English lines) keep their
    /// own line bands when the text's times sit on the voice.
    pub foreign_bands: bool,
    /// Lines of a Russian text written mostly in Latin letters («Miley
    /// Cyrus», «Джинсовка Levi's, йоу! Totally Spies, йоу!») stay with the
    /// Russian aligner, their Latin words spelled by sound; Latin words of
    /// a mostly Cyrillic line («Я будто тролль warlord») stay wildcards
    /// ([`Self::by_sound`] spells those too).
    pub sound_lines: bool,
    /// A line whose end comes from the next line's start more than
    /// [`FAR_GAP`] after its own start (an instrumental before the next
    /// line) is held at most until its first word plus how long its words
    /// take to sing ([`speech_secs`]) and [`FAR_SLACK`].
    pub far_cap: bool,
    /// For holding a word, the voice counts only where it is at least
    /// this share of the mix (0: off)
    pub hold_vmr: f32,
    /// … and at least this prominent (the lead, not a backing voice or a
    /// reverb; 0: off).
    pub hold_prominence: f32,
    /// A hum («м», «угу») ends where its voice does instead of lasting
    /// until the next word.
    pub hum_ends: bool,
    /// Lines heard by another aligner are aligned again with the timing
    /// pass's costs (a blank inside a word or a pause inside a line where
    /// there is no voice), not by a plain forced alignment: «Мяу,
    /// nyan.mp3» no longer starts 15 s before the tag is said.
    pub foreign_costs: bool,
    /// Numbers and abbreviations in capitals are spelled as they are said
    /// ([`crate::text::said`]).
    pub said: bool,
    /// A pause inside a line after a comma, a dash or other punctuation
    /// costs [`Costs::comma_pause`] instead of [`Costs::line_pause`]: that is
    /// where a line breathes.
    pub comma_pause: bool,
    /// A lone Latin word between Cyrillic words of a Cyrillic line is
    /// spelled by sound for the Russian aligner (not one at a line's end:
    /// «Я будто тролль warlord» lost a line so).
    pub sound_inner: bool,
    /// Two or more Latin words in a row in a line of a Russian text that
    /// is not mostly Latin, anywhere in it, are spelled by sound for the
    /// Russian aligner («Грёбаный сукин сын Oliver Sykes?!»); a single
    /// one at the line's end is not.
    pub sound_runs: bool,
    /// Voice the text lacks may also go after punctuation inside a line
    /// (the filler there too, not only between lines): an echo or a
    /// backing repeat of the words before the comma does not pull the
    /// line's next words onto it («Он такой скучный, (он такой скучный,
    /// он такой…) просто зануда»).
    pub comma_fill: bool,
    /// A word before punctuation inside a line ends with its voice once
    /// [`BREATH`] of what follows has none (not [`TAIL`]).
    pub comma_ends: bool,
    /// Without line times, [`Self::sound_lines`] leaves out lines sung
    /// again in the next or the line before (a hook: the English aligner
    /// keeps its repeats better there).
    pub sound_plain: bool,
}

/// See [`Tuning::far_cap`].
pub const FAR_GAP: f32 = 5.0;
pub const FAR_SLACK: f32 = 2.5;
/// A sung syllable takes about this long (seconds), for [`speech_secs`].
const SYLLABLE: f32 = 0.3;

/// About how long a line's words take to sing: its vowels (syllables).
pub fn speech_secs(text: &str) -> f32 {
    text.chars().flat_map(char::to_lowercase).filter(|c| "аеёиоуыэюяaeiouy".contains(*c)).count().max(1) as f32 * SYLLABLE
}

impl Tuning {
    /// The timing pass of `align-4`.
    pub const fn align4() -> Self {
        Self {
            costs: Costs {
                in_word: 0.0,
                in_word_silent: 0.0,
                line_pause: 0.0,
                onset: 0.0,
                vowels: 0.0,
                lead: 0.0,
                quiet_vmr: 0.0,
                gap_backing: 0.0,
                gap_lead: 0.0,
                pause_after: 0,
                comma_pause: 0.0,
                fill_base: 0.0,
                fill_prominence: 0.0,
            },
            by_sound: false,
            even: 0.0,
            squeezed: false,
            far_end: 0.0,
            stop_hold: 0.0,
            mix_ratio: 0.0,
            windows: false,
            foreign_bands: false,
            sound_lines: false,
            far_cap: false,
            hold_vmr: 0.0,
            hold_prominence: 0.0,
            hum_ends: false,
            foreign_costs: false,
            sound_plain: false,
            said: false,
            comma_pause: false,
            sound_inner: false,
            sound_runs: false,
            comma_fill: false,
            comma_ends: false,
        }
    }

}

/// `align-7` (docs/lyrics-align.md, «align-6», «align-7»):
/// `align-5`'s blank inside a word where there is no voice, and
/// * Latin lines of a Russian text by sound ([`Tuning::sound_lines`]; a hook
///   sung over and over only with line times, [`Tuning::sound_plain`]);
/// * a pause without voice longer than 8 frames (0.16 s) between two words
///   of a line costs 0.3 a frame, 0.2 after punctuation
///   ([`Tuning::comma_pause`]: the pause goes between lines, or where the
///   line takes its breath), and a word before punctuation ends with its
///   voice after a shorter silence ([`Tuning::comma_ends`]);
/// * a word that cannot be held is held at most 0.25 s ([`Tuning::stop_hold`]);
/// * hums end with their voice ([`Tuning::hum_ends`]);
/// * lines heard by another aligner keep these costs
///   ([`Tuning::foreign_costs`]);
/// * numbers and abbreviations in capitals are spelled as said
///   ([`Tuning::said`]: «25 ТГК» → «двадцать пять тэгэка»).
///
/// Measured on the bench with the hearing cut at whole strides (50 frames a
/// second), alone and together; none is worse on any song or case of it.
/// The rest of the knobs measured worse or no better, or better only on
/// the user's case songs and on none of the reference-timed ones
/// ([`Tuning::sound_inner`], [`Tuning::sound_runs`]: off).
impl Default for Tuning {
    fn default() -> Self {
        let mut t = Self::align4();
        t.costs.in_word_silent = 0.3;
        t.costs.line_pause = 0.3;
        t.costs.pause_after = 8;
        t.costs.comma_pause = 0.2;
        t.comma_pause = true;
        t.sound_lines = true;
        t.sound_plain = true;
        t.stop_hold = 0.25;
        t.hum_ends = true;
        t.foreign_costs = true;
        t.said = true;
        t.comma_ends = true;
        t
    }
}

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
    /// What the aligner spells for it: the text, or a Latin line of a
    /// Russian text by sound ([`Tuning::sound_lines`]).
    say: String,
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
    fit_text_with(em, vocab, &[], lines, timed, None, &Tuning::default())
}

/// [`fit_text`] with lines in another language: the whole text is aligned
/// with the text's own aligner first (its foreign lines too, which keeps
/// their place in time), then each run of foreign lines again with its
/// own aligner, between the lines around it. The verdict is the text's
/// own aligner's. `voice`: which of `em`'s frames have the voice in them
/// and where it comes in (the separated voice; `None` on the mix).
///
/// Two passes: the verdict comes from the plain alignment of the whole
/// text over the whole song (what the check's levels were measured on);
/// the word timing from a second one ([`place_on_path`]) that keeps each
/// line within its source times when they sit on the voice, lets voice
/// the text lacks go between lines and times hums by the voice.
pub fn fit_text_with(em: &Emissions, vocab: &Vocab, foreign: &[Foreign<'_>], lines: &[LyricLine], timed: bool, voice: Option<Voice<'_>>, tuning: &Tuning) -> Option<Fit> {
    // Pieces: a word-timed source keeps its own; others are cut here.
    let mut placed: Vec<Placed> = Vec::new();
    let mut tokens: Vec<i32> = Vec::new();
    let mut owner: Vec<Option<usize>> = Vec::new();
    for (li, l) in lines.iter().enumerate() {
        if l.text.trim().is_empty() {
            continue;
        }
        // A line of a Russian text written mostly in Latin letters, heard
        // by the Russian aligner: its Latin words by sound.
        let by_sound = tuning.sound_lines && !foreign.iter().any(|f| f.lines.contains(&li)) && vocab.cyrillic_only() && sung_by_sound(&l.text);
        let ps: Vec<String> = if l.words.is_empty() { pieces(&l.text) } else { l.words.iter().map(|w| w.text.clone()).collect() };
        // A lone Latin word with Cyrillic words on both sides («Кто
        // раздаёт вам swag? (Ла-ла-ла-ла)») by sound too (`sound_inner`).
        // So is a run of two or more Latin words (`sound_runs`: «сукин сын
        // Oliver Sykes?!»), unless another aligner takes the line.
        let runs = tuning.sound_runs && !foreign.iter().any(|f| f.lines.contains(&li));
        let inner = if (tuning.sound_inner || runs) && vocab.cyrillic_only() { latin_inside(&ps, tuning.sound_inner, runs) } else { vec![false; ps.len()] };
        for (p, inner) in ps.into_iter().zip(inner) {
            let say = if by_sound || inner { latin_by_sound(&p.to_lowercase()) } else { p.clone() };
            let toks = vocab.tokens(&say);
            let idx = placed.len();
            for t in &toks {
                tokens.push(*t);
                owner.push(Some(idx));
            }
            let ends_word = p.ends_with(char::is_whitespace);
            placed.push(Placed { line: li, text: p, say, span: None, peak: 0.0 });
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
    if !place_on_path(em, vocab, lines, &mut placed, bands.as_deref(), voice, tuning) && steady {
        realign_lines(em, vocab, lines, &mut placed, offset, &alien);
    }
    let total = em.secs(em.frames());
    for f in foreign {
        let costs = tuning.foreign_costs.then_some((tuning.costs, voice.map(|v| v.voiced).filter(|v| v.len() == f.em.frames())));
        place_foreign(f, lines, &mut placed, total, bands.as_deref().filter(|_| tuning.foreign_bands), costs);
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
    if let Some(v) = voice {
        for (li, extra) in repeats(em, vocab, lines, &placed, v.voiced, &alien) {
            let at = all.len();
            all.push(LyricLine { start: Duration::ZERO, end: None, words: Vec::new(), ..lines[li].clone() });
            placed.extend(extra.into_iter().map(|p| Placed { line: at, ..p }));
        }
    }
    let made = timed_lines(&all, &placed, tuning);
    let own_ends = if steady { source_ends(lines, offset) } else { vec![None; lines.len()] };
    // The repeats where they are sung, after the line before them.
    let mut order: Vec<usize> = (0..lines.len()).collect();
    for j in lines.len()..all.len() {
        let at = order.iter().position(|&i| !made[i].background && made[i].start > made[j].start).unwrap_or(order.len());
        order.insert(at, j);
    }
    // An end taken from a next line that starts long after this line's
    // last word (an instrumental between) holds nothing.
    let next_end = if steady { ends_from_next(lines) } else { Vec::new() };
    let ends: Vec<Option<f32>> = order
        .iter()
        .map(|&i| {
            let e = own_ends.get(i).copied().flatten()?;
            let last = made[i].words.last().map(|w| w.end.as_secs_f32() + LATE);
            let far = tuning.far_end > 0.0 && next_end.get(i) == Some(&true) && last.is_some_and(|b| e - b > tuning.far_end);
            let first = made[i].words.first().map(|w| w.start.as_secs_f32() + LATE);
            let e = match first {
                Some(a) if tuning.far_cap && next_end.get(i) == Some(&true) && e - a > FAR_GAP => e.min(a + speech_secs(&made[i].text) + FAR_SLACK),
                _ => e,
            };
            (!far).then_some(e)
        })
        .collect();
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
fn place_on_path(
    em: &Emissions,
    vocab: &Vocab,
    lines: &[LyricLine],
    placed: &mut [Placed],
    bands: Option<&[Option<Band>]>,
    voice: Option<Voice<'_>>,
    tuning: &Tuning,
) -> bool {
    use crate::ctc::{Gap, Path, Tok, align_path};
    let frame = |s: f32| ((s * em.fps).round().max(0.0) as usize).min(em.frames().saturating_sub(1));
    // A model of characters hears the right ones far under its best: a
    // filler would take the text's own voice (Japanese: 93 % of words
    // within 0.3 s without, 81–91 % with one).
    let fill = !vocab.syllabic();
    let mut path = Path { costs: tuning.costs, similar: vocab.similar(), ..Path::default() };
    let mut owner: Vec<Option<usize>> = Vec::new();
    // Each line's first token, for lines sung alike.
    let mut heads: Vec<(usize, usize)> = Vec::new();
    let mut line = None;
    let spelled: Vec<bool> = placed.iter().map(|p| is_hum(&p.text) || !vocab.tokens(&p.say).is_empty()).collect();
    for (i, p) in placed.iter().enumerate() {
        let band = bands.and_then(|b| b[p.line]);
        let hum = is_hum(&p.text);
        let toks = vocab.tokens(&p.say);
        if !spelled[i] {
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
        if line != Some(p.line) {
            heads.push((p.line, path.toks.len()));
        }
        line = Some(p.line);
        // The last piece of its line: the blank after it is between lines.
        let next = placed[i + 1..].iter().zip(&spelled[i + 1..]).find(|(_, s)| **s).map(|(n, _)| n);
        let ends_line = next.is_none_or(|n| n.line != p.line);
        // Hums are breaths apart («м, м, м, угу»): a pause next to one is
        // no pause inside a line.
        // A comma or a dash is where a line takes its breath («Дави на газ,
        // [pause] дави на газ»): a pause there is free (`comma_pause`).
        let comma = tuning.comma_pause && breathes(&p.text);
        let gap = if ends_line || hum || next.is_some_and(|n| is_hum(&n.text)) {
            Gap::Lines
        } else if comma {
            Gap::Comma
        } else {
            Gap::Words
        };
        let band = band.map(|b| (frame(b.from), frame(b.to)));
        let toks = if hum { vec![Tok::Hum] } else { toks.into_iter().map(Tok::Letter).collect() };
        let first = path.toks.len();
        for t in toks {
            path.push_at(t, band, at.take());
            owner.push(Some(i));
            let k = path.toks.len() - 1;
            path.gaps[k] = Gap::InWord;
        }
        path.starts[first] = true;
        let k = path.toks.len() - 1;
        path.gaps[k] = gap;
        if let Some(sep) = vocab.sep
            && p.text.ends_with(char::is_whitespace)
        {
            path.push(Tok::Letter(sep), None);
            owner.push(None);
            let k = path.toks.len() - 1;
            path.gaps[k] = gap;
            path.pauses[k] = matches!(gap, Gap::Words | Gap::Comma);
        }
        // After a comma the voice may hold what the text does not write
        // (`comma_fill`): an echo of the words before it.
        if gap == Gap::Comma && tuning.comma_fill && fill && path.toks.last() != Some(&Tok::Filler) {
            path.push(Tok::Filler, band);
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
    let Some(mut spans) = align_path(em, &path, voice) else { return false };
    if tuning.even > 0.0 && even_repeats(lines, &heads, &owner, placed, &spans, &mut path, tuning.even, bands.is_some())
        && let Some(again) = align_path(em, &path, voice)
    {
        spans = again;
    }
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

/// Identical lines in a row (three or more, «Miley Cyrus» eight times) are
/// sung about equally long. Where the timing pass made one of them far
/// longer or shorter than the others (a line took two sung repeats, the
/// next ones moved on by one), each is pulled (`pull` a second) towards
/// an even share of the stretch they all took. True when the path was
/// changed (to be aligned again). Lines with source times that sit on
/// the voice keep those (`banded`).
#[allow(clippy::too_many_arguments)]
fn even_repeats(
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
        let share = (to - from) as f32 / n as f32;
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
        // A repeat found here may end past `b` (its slice runs 0.3 s on).
        while (b + 1).saturating_sub(from) >= long {
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
                let v = crate::ctc::Voice { voiced: &voiced[at..(at + slice.frames()).min(voiced.len())], onset: &[], ..Default::default() };
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

/// Which lines [`source_ends`] ends at the next line's start (no end of
/// their own).
fn ends_from_next(lines: &[LyricLine]) -> Vec<bool> {
    lines.iter().map(|l| !l.background && l.end.is_none_or(|e| e <= l.start)).collect()
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
        for (i, w) in l.words.iter_mut().enumerate() {
            let (a, b) = (w.start.as_secs_f32(), w.end.as_secs_f32());
            let i0 = ((a / step).floor().max(0.0) as usize).min(voiced.len());
            let i1 = ((b / step).ceil().max(0.0) as usize).min(voiced.len());
            let Some(last) = (i0..i1).rev().find(|i| voiced[*i]) else { continue };
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
    if gap < COLLAPSED_AFTER || end + 1 - from < k {
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

fn fair(word: &str) -> f32 {
    (word.chars().filter(|c| c.is_alphabetic()).count() as f32 * FAIR_LETTER).clamp(FAIR_MIN, FAIR_MAX)
}

/// A word squeezed to almost nothing («Cyrus» 125.76–125.76: its letters
/// collapsed at the end of its stretch, or cut where the next line starts)
/// takes a fair share ([`fair`]) from the word before it, as long as that
/// one keeps its own; a line's first word takes it from the pause after
/// it. Nothing moves past the next line's start.
fn spread_squeezed(lines: &mut [LyricLine]) {
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
/// whole-song pass placed them), a little into both; with `bands` (the
/// text's line times sit on the voice) each line of the run within its
/// own band there, as the timing pass keeps them.
///
/// With `costs` ([`Tuning::foreign_costs`]) the run is aligned as the
/// timing pass aligns the text: what its blanks cost inside a word and
/// between words, over which frames have the voice (when known at that
/// aligner's frames), so a word is not stretched over seconds of silence
/// and a line not split around one.
fn place_foreign(
    f: &Foreign<'_>,
    lines: &[LyricLine],
    placed: &mut [Placed],
    total: f32,
    bands: Option<&[Option<Band>]>,
    costs: Option<(Costs, Option<&[bool]>)>,
) {
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
        let fits = match costs {
            Some((c, voiced)) => align_banded(f.em, f.vocab, placed, &idx, from, to, bands, Some((c, voiced))),
            None => bands.is_some_and(|b| align_banded(f.em, f.vocab, placed, &idx, from, to, Some(b), None)),
        };
        if !fits {
            align_pieces(f.em, f.vocab, placed, &idx, from, to);
        }
    }
}

/// [`align_pieces`] with each piece's line kept within its band (as far as
/// it overlaps `from..to`; `bands`), a filler between lines, and with
/// `costs` what blanks inside a word and between words cost over which
/// frames of `em` have the voice. False (nothing changed) when they do
/// not fit.
#[allow(clippy::too_many_arguments)]
fn align_banded(
    em: &Emissions,
    vocab: &Vocab,
    placed: &mut [Placed],
    idx: &[usize],
    from: f32,
    to: f32,
    bands: Option<&[Option<Band>]>,
    costs: Option<(Costs, Option<&[bool]>)>,
) -> bool {
    use crate::ctc::{Gap, Path, Tok, align_path};
    let (slice, at) = em.slice(from, to);
    let n = slice.frames();
    if n == 0 {
        return false;
    }
    let frame = |s: f32| (((s * em.fps).round().max(0.0) as usize).saturating_sub(at)).min(n - 1);
    let mut path = Path { costs: costs.map(|c| c.0).unwrap_or_default(), ..Path::default() };
    let mut owner = Vec::new();
    let mut line = None;
    let spelled: Vec<usize> = idx.iter().copied().filter(|i| !vocab.tokens(&placed[*i].say).is_empty()).collect();
    for (k, &i) in spelled.iter().enumerate() {
        let p = &placed[i];
        let toks = vocab.tokens(&p.say);
        if line.is_some_and(|l| l != p.line) && !vocab.syllabic() {
            path.push(Tok::Filler, None);
            owner.push(None);
        }
        line = Some(p.line);
        let band = bands.and_then(|b| b.get(p.line).copied().flatten()).map(|b| (frame(b.from), frame(b.to)));
        let in_line = spelled.get(k + 1).is_some_and(|n| placed[*n].line == p.line);
        let gap = if costs.is_some() && in_line { Gap::Words } else { Gap::Lines };
        for t in toks {
            path.push(Tok::Letter(t), band);
            owner.push(Some(i));
            let j = path.toks.len() - 1;
            path.gaps[j] = if costs.is_some() { Gap::InWord } else { Gap::Lines };
        }
        let j = path.toks.len() - 1;
        path.gaps[j] = gap;
        if let Some(sep) = vocab.sep
            && p.text.ends_with(char::is_whitespace)
        {
            path.push(Tok::Letter(sep), None);
            owner.push(None);
            let j = path.toks.len() - 1;
            path.gaps[j] = gap;
            path.pauses[j] = gap == Gap::Words;
        }
    }
    if path.toks.is_empty() {
        return false;
    }
    let voiced = costs.and_then(|c| c.1).map(|v| &v[at.min(v.len())..(at + n).min(v.len())]);
    let voice = voiced.map(|v| crate::ctc::Voice { voiced: v, ..Default::default() });
    let Some(spans) = align_path(&slice, &path, voice) else { return false };
    let mut fresh: Vec<Option<(f32, f32)>> = vec![None; placed.len()];
    for (span, own) in spans.iter().zip(&owner) {
        let (Some(span), Some(i)) = (span, own) else { continue };
        let (a, b) = (em.secs(at + span.first), em.secs(at + span.last + 1));
        fresh[*i] = Some(fresh[*i].map_or((a, b), |(s, e)| (s.min(a), e.max(b))));
    }
    for &i in idx {
        if let Some(span) = fresh[i] {
            placed[i].span = Some(span);
        }
    }
    true
}

/// Aligns the pieces `idx` of `placed` within seconds `from..to` of `em`
/// and takes their new places (all of them, or none when they do not fit).
fn align_pieces(em: &Emissions, vocab: &Vocab, placed: &mut [Placed], idx: &[usize], from: f32, to: f32) {
    let mut tokens = Vec::new();
    let mut owner = Vec::new();
    for &i in idx {
        let toks = vocab.tokens(&placed[i].say);
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

/// Which of a line's pieces are a lone Latin word with a Cyrillic word
/// on each side (`lone`, [`Tuning::sound_inner`]), or one of two or more
/// Latin words in a row (`runs`, [`Tuning::sound_runs`]); a single one at
/// either end of the line is neither («Я будто тролль warlord»).
///
/// Pieces glued without a space between them («war|lord», a word in
/// syllables) are one word here.
fn latin_inside(pieces: &[String], lone: bool, runs: bool) -> Vec<bool> {
    // The written words: each piece's word, and each word's text.
    let mut word_of = Vec::with_capacity(pieces.len());
    let mut words: Vec<String> = Vec::new();
    let mut open = false;
    for p in pieces {
        if !open {
            words.push(String::new());
        }
        word_of.push(words.len() - 1);
        words.last_mut().unwrap().push_str(p);
        open = !p.ends_with(char::is_whitespace);
    }
    let cyrillic = |w: &String| w.chars().any(|c| matches!(c, '\u{0400}'..='\u{04ff}'));
    let latin: Vec<bool> = words.iter().map(|w| w.chars().any(|c| c.is_ascii_alphabetic()) && !cyrillic(w)).collect();
    let chosen: Vec<bool> = (0..words.len())
        .map(|i| {
            let alone = lone && i > 0 && cyrillic(&words[i - 1]) && words.get(i + 1).is_some_and(cyrillic);
            let in_run = runs && (i > 0 && latin[i - 1] || latin.get(i + 1).copied().unwrap_or(false));
            latin[i] && (alone || in_run)
        })
        .collect();
    word_of.into_iter().map(|w| chosen[w]).collect()
}

/// A word written with punctuation after it (a comma, a dash, a full
/// stop): where a line may take its breath.
fn breathes(text: &str) -> bool {
    text.trim_end().ends_with([',', ';', ':', '—', '–', '-', '…', '.', '!', '?'])
}

/// The source's lines with the pieces' times as words. A piece without
/// letters (a dash) sticks to the one before it; a line without any keeps
/// its own start.
fn timed_lines(lines: &[LyricLine], placed: &[Placed], tuning: &Tuning) -> Vec<LyricLine> {
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
        // (the sung vowel goes on), so the fill does not stall; a hum ends
        // with its voice (`hum_ends`: «м, м, м, угу» with breaths between).
        for i in 0..words.len().saturating_sub(1) {
            let next = words[i + 1].start;
            if tuning.hum_ends && is_hum(&words[i].text) {
                continue;
            }
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
        let both = fit_text_with(&main, &v, &[f], &lines, false, None, &Tuning::default()).unwrap();
        let start = both.lines[1].start.as_secs_f32();
        assert!((start - (1.0 - LATE)).abs() < 0.05, "{:?}", both.lines[1]);
        // The verdict stays the text's aligner's.
        assert_eq!(both.score, alone.score);
        assert!(both.lines[2].start > both.lines[1].start);
    }

    /// «M» heard at once, «-iley» dumped at the end of the held stretch:
    /// the dumped letters are spread over it, vowels longer.
    #[test]
    fn collapsed_letters_are_spread_over_the_voiced_stretch() {
        let mut letters = vec![(0, 3), (40, 40), (41, 41), (42, 42), (43, 43)];
        let vowel = [false, true, false, true, true];
        let voiced = vec![true; 50];
        spread_collapsed(&mut letters, &vowel, &voiced);
        assert_eq!(letters[0], (0, 3));
        assert_eq!(letters[1].0, 4);
        assert_eq!(letters[4].1, 43);
        for w in letters.windows(2) {
            assert!(w[1].0 > w[0].1 && w[1].0 == w[0].1 + 1, "{letters:?}");
        }
        // The vowel «i» takes about twice the consonant «l».
        let len = |s: (usize, usize)| s.1 + 1 - s.0;
        assert!(len(letters[1]) >= 2 * len(letters[2]) - 1, "{letters:?}");
        // Over silence nothing moves (the word's end is the aligner's).
        let mut quiet = vec![(0, 3), (40, 40), (41, 41)];
        spread_collapsed(&mut quiet, &[false, true, false], &[false; 50]);
        assert_eq!(quiet, [(0, 3), (40, 40), (41, 41)]);
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
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: None, tuning: Tuning::align4() };
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
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: Some(&level), tuning: Tuning::align4() };
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

    /// «Miley Cyrus»: the last word of a line squeezed to nothing where the
    /// next line starts takes a fair share from the word before it.
    #[test]
    fn a_squeezed_word_gets_a_fair_share() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut a = line(123.71, "Miley Cyrus");
        a.words = vec![w(123.71, 124.50, "Miley "), w(125.76, 125.76, "Cyrus")];
        let mut b = line(125.74, "Слышишь");
        b.words = vec![w(125.74, 126.44, "Слышишь")];
        let mut lines = vec![a, b];
        spread_squeezed(&mut lines);
        let cyrus = &lines[0].words[1];
        assert!((cyrus.end.as_secs_f32() - 125.74).abs() < 0.01, "{cyrus:?}");
        assert!((cyrus.end - cyrus.start).as_secs_f32() >= fair("Cyrus") - 0.01, "{cyrus:?}");
        assert!(lines[0].words[0].end <= cyrus.start);
        // A word of fair length stays.
        let mut c = line(1.0, "да нет");
        c.words = vec![w(1.0, 1.5, "да "), w(1.5, 1.9, "нет")];
        c.end = Some(Duration::from_secs_f32(1.9));
        let mut keep = vec![c.clone()];
        spread_squeezed(&mut keep);
        assert_eq!(keep[0], c);
    }

    /// A held note is a vowel: a word that ends in a stop is not held on.
    #[test]
    fn a_word_ending_in_a_stop_is_not_held() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut l = line(1.0, "хватит");
        l.words = vec![w(1.0, 1.6, "хватит")];
        l.end = Some(Duration::from_secs_f32(1.6));
        let level: Vec<f32> = (0..60).map(|i| if (10..50).contains(&i) { 0.9 } else { 0.0 }).collect();
        let voiced: Vec<bool> = level.iter().map(|e| *e > 0.05).collect();
        let v = VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: Some(&level), tuning: Tuning::align4() };
        let mut held = vec![l.clone()];
        end_with_voice(&mut held, v, &[Some(5.0)]);
        assert!((held[0].words[0].end.as_secs_f32() - 5.0).abs() < 0.01);
        let mut stopped = vec![l];
        end_with_voice(&mut stopped, VoiceEnds { tuning: Tuning { stop_hold: 0.2, ..Tuning::align4() }, ..v }, &[Some(5.0)]);
        assert!((stopped[0].words[0].end.as_secs_f32() - 1.8).abs() < 0.01, "{:?}", stopped[0].words);
        assert!(can_hold("Sykes?!") && !can_hold("хватит") && !can_hold("crack,"));
    }

    /// «Дави на газ, [pause] дави на газ»: a word before a comma ends with
    /// its voice after a shorter silence than elsewhere; a word without one,
    /// and the last word of a line, keep the usual tail.
    #[test]
    fn a_word_before_a_comma_ends_with_its_breath() {
        let w = |a: f32, b: f32, t: &str| LyricWord { start: Duration::from_secs_f32(a), end: Duration::from_secs_f32(b), text: t.into() };
        let mut l = line(1.0, "газ, дави газ газ,");
        l.words = vec![w(1.0, 1.9, "газ, "), w(1.9, 2.3, "дави "), w(3.0, 3.9, "газ "), w(4.0, 4.9, "газ,")];
        // Voice at 1.0–1.4, 1.9–2.3, 3.0–3.4 and 4.0–4.4 s (tenths).
        let voiced: Vec<bool> = (0..60).map(|i| [10, 19, 30, 40].iter().any(|a| (*a..*a + 4).contains(&i))).collect();
        let tuning = Tuning { comma_ends: true, ..Tuning::align4() };
        let mut lines = vec![l.clone()];
        end_with_voice(&mut lines, VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: None, tuning }, &[None]);
        let ends: Vec<f32> = lines[0].words.iter().map(|w| w.end.as_secs_f32()).collect();
        assert!((ends[0] - 1.5).abs() < 0.01, "{ends:?}");
        assert!((ends[2] - 3.9).abs() < 0.01, "{ends:?}");
        assert!((ends[3] - 4.9).abs() < 0.01, "{ends:?}");
        let mut lines = vec![l];
        end_with_voice(&mut lines, VoiceEnds { voiced: &voiced, step: 0.1, tail: TAIL, level: None, tuning: Tuning::align4() }, &[None]);
        assert!((lines[0].words[0].end.as_secs_f32() - 1.9).abs() < 0.01);
    }

    #[test]
    fn a_lone_latin_word_inside_a_cyrillic_line_is_found() {
        let inside = |t: &str| latin_inside(&pieces(t), true, false);
        assert_eq!(inside("Кто раздаёт вам swag? (Ла-ла-ла-ла)"), [false, false, false, true, false]);
        assert_eq!(inside("Я будто тролль warlord"), [false; 4]);
        assert_eq!(inside("Джинсовка Levi's, йоу!"), [false, true, false]);
        assert_eq!(inside("Totally Spies, йоу!"), [false; 3]);
        let runs = |t: &str| latin_inside(&pieces(t), false, true);
        assert_eq!(runs("Грёбаный сукин сын Oliver Sykes?!"), [false, false, false, true, true]);
        assert_eq!(runs("Я будто тролль warlord"), [false; 4]);
        assert_eq!(runs("Кто раздаёт вам swag? (Ла-ла-ла-ла)"), [false; 5]);
        // A word in syllables is one word.
        let glued: Vec<String> = ["Я ", "будто ", "тролль ", "war", "lord"].iter().map(|s| s.to_string()).collect();
        assert_eq!(latin_inside(&glued, true, true), [false; 5]);
    }

    /// «Это нон-стоп» then twelve seconds of instrumental: the next line's
    /// start is no end of this one.
    #[test]
    fn a_line_ends_at_the_next_start_only_without_its_own_end() {
        let mut ls = vec![line(1.0, "а"), line(3.0, "б"), line(15.0, "в")];
        ls[1].end = Some(Duration::from_secs(4));
        assert_eq!(ends_from_next(&ls), [true, false, true]);
    }

    /// Eight «Miley Cyrus» in a row: one taking two sung repeats pulls each
    /// towards an even share of the stretch they took.
    #[test]
    fn lines_sung_alike_are_pulled_to_even_shares() {
        use crate::ctc::{Path, Tok, TokenSpan};
        let lines: Vec<LyricLine> = (0..4).map(|i| line(i as f32, "Miley Cyrus")).collect();
        // One token a line: lines at frames 0, 10, 30 (two repeats), 35.
        let placed: Vec<Placed> = (0..4).map(|li| Placed { line: li, text: "x".into(), say: "x".into(), span: None, peak: 0.0 }).collect();
        let mut path = Path::default();
        for _ in 0..4 {
            path.push(Tok::Letter(1), None);
        }
        let heads: Vec<(usize, usize)> = (0..4).map(|i| (i, i)).collect();
        let owner: Vec<Option<usize>> = (0..4).map(Some).collect();
        let span = |a: usize, b: usize| Some(TokenSpan { first: a, last: b, peak: 1.0 });
        let spans = vec![span(0, 8), span(10, 28), span(30, 33), span(35, 39)];
        assert!(even_repeats(&lines, &heads, &owner, &placed, &spans, &mut path, 3.0, false));
        assert_eq!(path.at, [Some(0), Some(10), Some(20), Some(30)]);
        assert_eq!(path.pull[2], 3.0);
        // Even already, or kept by source times: left alone.
        let even = vec![span(0, 8), span(10, 18), span(20, 28), span(30, 39)];
        let mut p2 = path.clone();
        p2.at = vec![None; 4];
        assert!(!even_repeats(&lines, &heads, &owner, &placed, &even, &mut p2, 3.0, false));
        assert!(!even_repeats(&lines, &heads, &owner, &placed, &spans, &mut p2, 3.0, true));
    }

    #[test]
    fn plain_text_lines_drop_labels() {
        let l = plain_lines("[Куплет 1]\nРаз два\n\n  три  \n[Припев]");
        assert_eq!(l.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["Раз два", "три"]);
    }
}
