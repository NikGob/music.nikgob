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
use crate::text::{Vocab, is_hum, latin_by_sound, pieces, sung_by_sound};

mod ends;
mod repeats;
#[cfg(test)]
mod tests;

pub use ends::{VoiceEnds, end_with_voice, spread_collapsed};
use ends::{ends_from_next, hold_to_source, realign_lines, source_ends};
use repeats::{REPEATS_BUDGET, even_repeats, repeats};

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
/// (the median against Apple's word timing on 8 songs, +0.03…+0.11 s):
/// made times are moved back by it.
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

/// How the timing pass places words beyond `align-4`'s rules.
/// [`Tuning::default`] is what the player uses; [`Tuning::align4`] turns every addition off.
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
    /// The last word of a line ends where the separated voice falls under
    /// this share of its line's level ([`end_with_voice`]) and stays under
    /// it for the usual tail, not only where it falls silent: a reverb or
    /// the instrumental's bleed after a held last word is no more of it
    /// (0: off). Only before a gap of at least [`Tuning::end_gap`] seconds
    /// to the next line (0: before any).
    pub end_level: f32,
    pub end_gap: f32,
    /// A word before punctuation inside a line ends with its voice once
    /// [`BREATH`] of what follows has none (not [`TAIL`]).
    pub comma_ends: bool,
    /// [`Costs::vowels`] for every pair of vowels a singer's are heard as
    /// for each other, not only «о» said as «а» ([`crate::text::Vocab::similar`]).
    pub vowels_all: bool,
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
                start_silent: 0.0,
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
            end_level: 0.0,
            end_gap: 0.0,
            comma_ends: false,
            vowels_all: false,
        }
    }

}

/// `align-8`:
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
///   ([`Tuning::said`]: «25 ТГК» → «двадцать пять тэгэка»);
/// * a sung «а» may be heard for a written unstressed «о»
///   ([`Costs::vowels`] 1.5, Russian only: «Мой топор» 2 word starts, 1 end
///   and 1 line better, nothing worse, words under 80 ms included);
/// * a word squeezed to almost nothing takes a fair share from the word
///   before it ([`Tuning::squeezed`]; with it on the bench has 40 % fewer
///   words lit for under 80 ms and the same word and line starts and ends
///   within 0.3 s on every song).
///
/// Measured on the bench with the hearing cut at whole strides (50 frames a
/// second), alone and together; none is worse on any song or case of it.
/// Also on, though they change only the user's case songs and none of
/// the reference-timed ones (no worse there, alone or together): a lone
/// Latin word between Cyrillic ones and a run of two or more Latin words
/// in a Cyrillic line by sound ([`Tuning::sound_inner`],
/// [`Tuning::sound_runs`]: «swag?», «Oliver Sykes»), and a line's last
/// word ending where its voice falls under half the line's level
/// ([`Tuning::end_level`]: «fine» held over a reverb tail). The rest of
/// the knobs measured worse or no better.
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
        t.sound_inner = true;
        t.sound_runs = true;
        t.end_level = 0.5;
        t.squeezed = true;
        t.costs.vowels = 1.5;
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
    /// levels).
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
        for (li, extra) in repeats(em, vocab, lines, &placed, v.voiced, &alien, REPEATS_BUDGET) {
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
    let mut path = Path { costs: tuning.costs, similar: vocab.similar(tuning.vowels_all), ..Path::default() };
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
