//! The whole job for one song: decode, separate the voice, hear it with the
//! aligner of the text's script, fit texts, recognise one from scratch.
//! Models load on first use and stay until [`Aligner::unload`].

use std::collections::HashMap;
use std::path::Path;

use nikgob_core::lyrics::LyricLine;

use crate::catalog::{Catalog, ModelId};
use crate::ctc::{Emissions, Voice};
use crate::deep::{self, Deep, Second};
use crate::engine::{Device, Model};
use crate::fit::{Fit, Foreign, Tuning, fit_text_with};
use crate::separate::{self, KIM_VOCAL_2, ROFORMER_CHUNK};
use crate::text::{Lang, Letters, Vocab};
use crate::whisper::{self, Whisper};
use crate::{Error, audio};

/// The rate of the aligners and Whisper.
const RATE: u32 = 16_000;
/// Window of the aligners, with context on both sides.
const WINDOW_S: usize = 30;
/// The second hearing of [`Tuning::windows`] cuts its windows this much
/// later (seconds): half a window.
pub const SHIFT_S: usize = 15;
const CONTEXT_S: usize = 2;
/// Voice energy is measured over this many samples (0.1 s).
const ENERGY_HOP: usize = 1600;
/// A window is brought to unit loudness for the aligner, but never raised
/// by more than this share of the song's own level: a near-silent window
/// (a song's silent tail) raised to full loudness is noise the aligner
/// hears letters in, and a text's last word went on into it.
const QUIET: f32 = 0.1;
/// Under this share of the voice's loud level a tenth of a second has no
/// voice in it, for where a word ends.
const SILENT: f32 = 0.05;
/// On the mix (no separated voice), a tenth the aligner hears as blank at
/// least this surely has no voice, and only this long a stretch of them
/// ends a word (singing is mostly blank to a CTC model between letters).
const BLANK: f32 = 0.95;
const MIX_TAIL: f32 = 1.5;
/// The voice coming in: its loudness rising this much (dB) over the frames
/// before counts as half an onset, twice that as a whole one.
const ONSET_DB: f32 = 6.0;
/// … looked for this many frames back from where a word starts.
const ONSET_BACK: usize = 4;
/// The separated voice's sides at this share of its level or under: the
/// lead alone, in the middle; at [`SIDE_BACKING`] or over: backing vocals.
const SIDE_LEAD: f32 = 0.15;
const SIDE_BACKING: f32 = 0.6;

/// Where a job is, for the progress shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    Decode,
    /// Separating the voice (done share).
    Separate(f32),
    /// Hearing it with an aligner (done share).
    Listen(f32),
    Align,
    /// Recognising from scratch (done share).
    Recognise(f32),
}

/// A song ready for texts: its voice at 16 kHz and what the aligners heard.
pub struct Prepared {
    pub voice: Vec<f32>,
    pub secs: f32,
    /// The voice was separated from the mix (else it is the mix).
    pub separated: bool,
    /// RMS of the voice every 0.1 s.
    energy: Vec<f32>,
    /// RMS of the separated voice's sides (left less right, halved) every
    /// 0.1 s: a voice panned off the centre (backing vocals) is loud here,
    /// the lead sung in the middle is not. Empty when unknown.
    side: Vec<f32>,
    /// RMS of the mix every 0.1 s (empty when unknown): a sound the
    /// separator lets through (a lead synth on a drop) is loud in the
    /// voice but small against the mix.
    mix: Vec<f32>,
    heard: HashMap<ModelId, Emissions>,
    /// What deep mode heard for an aligner's text (its second aligner's
    /// letters, alone or fused with the first's), with those letters: the
    /// word timing comes from it, the verdict from [`Self::heard`].
    deep: HashMap<ModelId, (Emissions, Vocab)>,
    /// The voice was separated by deep mode's separator.
    pub deep_separated: bool,
}

impl Prepared {
    /// Whether seconds `a..b` have the voice in them.
    pub fn voiced(&self, a: f32, b: f32) -> bool {
        if !self.separated {
            return true;
        }
        let peak = percentile(&self.energy, 0.95);
        let i = (a * 10.0).max(0.0) as usize;
        let j = ((b * 10.0).ceil() as usize).min(self.energy.len());
        self.energy[i.min(j)..j].iter().any(|e| *e > peak * 0.12)
    }
}

impl Prepared {
    /// A song from its voice at 16 kHz (`separated`: the voice alone, not
    /// the mix), with nothing heard yet.
    pub fn from_voice(voice: Vec<f32>, separated: bool) -> Self {
        let secs = voice.len() as f32 / RATE as f32;
        let energy = voice
            .chunks(ENERGY_HOP)
            .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len().max(1) as f32).sqrt())
            .collect();
        Self { voice, secs, separated, energy, side: Vec::new(), mix: Vec::new(), heard: HashMap::new(), deep: HashMap::new(), deep_separated: false }
    }

    /// Takes the side and mix levels ([`Self::side`], [`Self::mix`]; kept
    /// from an earlier run).
    pub fn set_levels(&mut self, side: Vec<f32>, mix: Vec<f32>) {
        self.side = side;
        self.mix = mix;
    }

    /// RMS of the separated voice's sides every tenth of a second.
    pub fn side(&self) -> &[f32] {
        &self.side
    }

    /// RMS of the mix every tenth of a second.
    pub fn mix(&self) -> &[f32] {
        &self.mix
    }

    /// Takes what aligner `id` heard in this song (kept from an earlier run).
    pub fn set_emissions(&mut self, id: ModelId, em: Emissions) {
        self.heard.insert(id, em);
    }

    /// What aligner `id` heard in the song, once it has.
    pub fn emissions(&self, id: ModelId) -> Option<&Emissions> {
        self.heard.get(&id)
    }

    /// Takes what deep mode heard for aligner `id`'s texts (kept from an
    /// earlier run), with its letters.
    pub fn set_deep_emissions(&mut self, id: ModelId, em: Emissions, vocab: Vocab) {
        self.deep.insert(id, (em, vocab));
    }

    /// What deep mode heard for aligner `id`'s texts, once it has.
    pub fn deep_emissions(&self, id: ModelId) -> Option<&Emissions> {
        self.deep.get(&id).map(|(em, _)| em)
    }

    /// RMS of the voice every tenth of a second.
    pub fn energy(&self) -> &[f32] {
        &self.energy
    }

    /// Ends the words of `lines` where the voice ends, and holds them while
    /// it goes on ([`crate::fit::end_with_voice`]; `bounds`: where the
    /// source ends each line): by the separated voice's energy, or on the
    /// mix by long stretches aligner `id` hears as blank only (a held vowel
    /// is mostly blank to it there, so nothing is held on the mix).
    pub fn end_with_voice(&self, lines: &mut [LyricLine], id: ModelId, bounds: &[Option<f32>], tuning: &Tuning) {
        use crate::fit::VoiceEnds;
        if self.separated {
            let peak = percentile(&self.energy, 0.95);
            let voiced: Vec<bool> = (0..self.energy.len()).map(|i| self.energy[i] > peak * SILENT && self.over_mix(i, tuning.mix_ratio)).collect();
            // What leaks from the mix is no voice to hold a word on.
            let level: Vec<f32> = (0..self.energy.len()).map(|i| if self.over_mix(i, tuning.mix_ratio) { self.energy[i] } else { 0.0 }).collect();
            let step = ENERGY_HOP as f32 / RATE as f32;
            let v = VoiceEnds { voiced: &voiced, step, tail: crate::fit::TAIL, level: Some(&level), tuning: *tuning };
            crate::fit::end_with_voice(lines, v, bounds);
        } else if let Some(em) = self.heard.get(&id) {
            let per = (em.fps / 10.0).round().max(1.0) as usize;
            let voiced: Vec<bool> = (0..em.frames().div_ceil(per))
                .map(|i| {
                    let frames = i * per..((i + 1) * per).min(em.frames());
                    let n = frames.len().max(1) as f32;
                    frames.map(|t| em.row(t)[em.blank].exp()).sum::<f32>() / n < BLANK
                })
                .collect();
            let step = per as f32 / em.fps;
            crate::fit::end_with_voice(lines, VoiceEnds { voiced: &voiced, step, tail: MIX_TAIL, level: None, tuning: *tuning }, bounds);
        }
    }

    /// Tenth `i`'s voice is at least `ratio` of the mix there (always
    /// with no ratio or no mix level).
    fn over_mix(&self, i: usize, ratio: f32) -> bool {
        ratio <= 0.0 || self.mix.get(i).is_none_or(|m| self.energy.get(i).is_some_and(|e| *e >= m * ratio))
    }

    /// Which of `em`'s frames have the separated voice in them (`None` on
    /// the mix, where it cannot tell); `ratio`: [`Tuning::mix_ratio`].
    pub fn voiced_frames(&self, em: &Emissions, ratio: f32) -> Option<Vec<bool>> {
        if !self.separated {
            return None;
        }
        let tenths: Vec<bool> = self.voiced_tenths().into_iter().enumerate().map(|(i, v)| v && self.over_mix(i, ratio)).collect();
        let step = ENERGY_HOP as f32 / RATE as f32;
        Some((0..em.frames()).map(|t| tenths.get((em.secs(t) / step) as usize).copied().unwrap_or(false)).collect())
    }

    /// How much of the separated voice is off the centre at each of `em`'s
    /// frames (0..1: its sides' level against its whole level, from a
    /// share of [`SIDE_LEAD`] up to [`SIDE_BACKING`]). Empty when unknown.
    pub fn backing_frames(&self, em: &Emissions) -> Vec<f32> {
        if self.side.is_empty() {
            return Vec::new();
        }
        let step = ENERGY_HOP as f32 / RATE as f32;
        (0..em.frames())
            .map(|t| {
                let i = (em.secs(t) / step) as usize;
                let (s, e) = (self.side.get(i).copied().unwrap_or(0.0), self.energy.get(i).copied().unwrap_or(0.0));
                if e <= 1e-6 { 0.0 } else { ((s / e - SIDE_LEAD) / (SIDE_BACKING - SIDE_LEAD)).clamp(0.0, 1.0) }
            })
            .collect()
    }

    /// How sharply the separated voice comes in at each of `em`'s frames
    /// (0..1): its loudness rising by [`ONSET_DB`] or more against the
    /// frames just before, looked for a few frames back (the aligner puts a
    /// word's start a little after the singer starts it). Empty on the mix.
    pub fn onset_frames(&self, em: &Emissions, voiced: &[bool]) -> Vec<f32> {
        if !self.separated || em.fps <= 0.0 {
            return Vec::new();
        }
        let hop = ((RATE as f32 / em.fps).round() as usize).max(1);
        let db: Vec<f32> = (0..em.frames())
            .map(|t| {
                let a = (t * hop).min(self.voice.len());
                let b = ((t + 1) * hop).min(self.voice.len());
                let c = &self.voice[a..b];
                let rms = (c.iter().map(|s| s * s).sum::<f32>() / c.len().max(1) as f32).sqrt();
                20.0 * (rms + 1e-5).log10()
            })
            .collect();
        let rise: Vec<f32> = (0..db.len())
            .map(|t| {
                let before = db[t.saturating_sub(4)..t].iter().copied().fold(f32::INFINITY, f32::min);
                if !voiced.get(t).copied().unwrap_or(false) || !before.is_finite() {
                    return 0.0;
                }
                ((db[t] - before - ONSET_DB) / ONSET_DB).clamp(0.0, 1.0)
            })
            .collect();
        (0..rise.len()).map(|t| rise[t.saturating_sub(ONSET_BACK)..=t].iter().copied().fold(0.0, f32::max)).collect()
    }

    /// Which tenths of a second have the voice (all of them for a mix).
    pub fn voiced_tenths(&self) -> Vec<bool> {
        if !self.separated {
            return vec![true; self.energy.len()];
        }
        let peak = percentile(&self.energy, 0.95);
        self.energy.iter().map(|e| *e > peak * 0.12).collect()
    }

    /// Share of the voiced tenths of a second that `lines`' words cover
    /// (with a little room around each word).
    pub fn covered(&self, lines: &[LyricLine]) -> f32 {
        if !self.separated || self.energy.is_empty() {
            return 1.0;
        }
        let peak = percentile(&self.energy, 0.95);
        let voiced: Vec<bool> = self.energy.iter().map(|e| *e > peak * 0.12).collect();
        let mut covered = vec![false; voiced.len()];
        for l in lines {
            for w in &l.words {
                let a = ((w.start.as_secs_f32() - 0.3) * 10.0).max(0.0) as usize;
                let b = (((w.end.as_secs_f32() + 0.3) * 10.0).ceil() as usize).min(covered.len());
                for c in covered.iter_mut().take(b).skip(a) {
                    *c = true;
                }
            }
        }
        let total = voiced.iter().filter(|v| **v).count();
        if total == 0 {
            return 1.0;
        }
        voiced.iter().zip(&covered).filter(|(v, c)| **v && **c).count() as f32 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn text(lines: &[&str]) -> Vec<LyricLine> {
        lines.iter().map(|l| LyricLine::new(Duration::ZERO, *l)).collect()
    }

    #[test]
    fn each_language_goes_to_its_aligner() {
        let ru = Route::of(&text(&["Я иду домой", "Baby, I love you", "И снова дождь"])).unwrap();
        assert_eq!(ru.main, ModelId::AlignerRu);
        assert_eq!(ru.lines, [ModelId::AlignerRu, ModelId::AlignerEn, ModelId::AlignerRu]);
        assert_eq!(ru.models(), [ModelId::AlignerRu, ModelId::AlignerEn]);
        let ja = Route::of(&text(&["君の名は", "Sayonara watashi no kokoro", "夢を見た"])).unwrap();
        assert_eq!(ja.main, ModelId::AlignerJa);
        // Romaji is heard by the romanising aligner.
        assert_eq!(ja.lines[1], ModelId::AlignerMms);
        let en = Route::of(&text(&["Hold me close", "Look me dead in my eyes"])).unwrap();
        assert_eq!((en.main, en.models().len()), (ModelId::AlignerEn, 1));
        let es = Route::of(&text(&["Que te quiero, mi amor", "para siempre"])).unwrap();
        assert_eq!(es.main, ModelId::AlignerMms);
        assert!(Route::of(&text(&["我爱你"])).is_none());
        assert_eq!(Aligner::needs(&text(&["Hold me close"])), Some(ModelId::AlignerEn));
    }

    #[test]
    fn one_aligner_is_held_at_a_time() {
        use std::cell::Cell;
        use std::rc::Rc;
        /// A model that counts how many are alive.
        struct Counted(Rc<Cell<usize>>);
        impl Drop for Counted {
            fn drop(&mut self) {
                self.0.set(self.0.get() - 1);
            }
        }
        let alive = Rc::new(Cell::new(0));
        let mut slot: OneLoaded<Counted> = OneLoaded::default();
        let load = |alive: &Rc<Cell<usize>>| {
            // The one before is gone before the next one loads.
            assert_eq!(alive.get(), 0, "two aligners in memory at once");
            alive.set(alive.get() + 1);
            Ok::<_, ()>(Counted(alive.clone()))
        };
        slot.get(ModelId::AlignerRu, || load(&alive)).unwrap();
        // The same one again is not loaded again.
        slot.get(ModelId::AlignerRu, || -> Result<Counted, ()> { panic!("loaded twice") }).unwrap();
        slot.get(ModelId::AlignerEn, || load(&alive)).unwrap();
        slot.get(ModelId::AlignerJa, || load(&alive)).unwrap();
        assert_eq!(alive.get(), 1);
        assert_eq!(slot.held().map(|(id, _)| id), Some(ModelId::AlignerJa));
        // A failed load leaves nothing held.
        assert!(slot.get(ModelId::AlignerMms, || Err::<Counted, ()>(())).is_err());
        assert_eq!((alive.get(), slot.held().is_none()), (0, true));
        let a = Aligner::new(Catalog::new(""), Device::Cpu, 1);
        assert_eq!((a.aligner_loaded(), a.loaded()), (None, false));
    }

    /// Latin lines of a Russian text stay with the Russian aligner (spelled
    /// by sound), unless the text is sung in both languages.
    #[test]
    fn latin_lines_of_a_russian_text_stay_with_it_by_sound() {
        let ru = text(&["Я пойду прогуляюсь", "Miley Cyrus", "Miley Cyrus", "Где мои деньги", "Дома закончился крэк", "Мы рисовали в подъезде то ли пентаграмму"]);
        assert_eq!(Route::of_with(&ru, false).unwrap().lines[1], ModelId::AlignerEn);
        let by_sound = Route::of_with(&ru, true).unwrap();
        assert_eq!(by_sound.models(), [ModelId::AlignerRu]);
        let both = text(&["Я иду домой", "Baby, I love you so much tonight", "Hold me close and never let me go"]);
        assert_eq!(Route::of_with(&both, true).unwrap().lines[1], ModelId::AlignerEn);
    }

    /// The voice coming in sharply is an onset a few frames on; backing
    /// vocals are the voice off the centre.
    #[test]
    fn onsets_and_backing_vocals_from_the_voice() {
        // Silence for 0.5 s, then a loud tone.
        let voice: Vec<f32> = (0..16_000).map(|i| if i < 8_000 { 0.0 } else { (i as f32 * 0.05).sin() * 0.5 }).collect();
        let mut prep = Prepared::from_voice(voice, true);
        let em = Emissions { logp: vec![0.0; 50 * 2], vocab: 2, fps: 50.0, blank: 0 };
        let voiced = vec![true; 50];
        let onset = prep.onset_frames(&em, &voiced);
        assert_eq!(onset.len(), 50);
        assert!(onset[25] > 0.9 && onset[28] > 0.9, "{onset:?}");
        assert!(onset[10] == 0.0 && onset[40] == 0.0, "{onset:?}");
        assert!(prep.backing_frames(&em).is_empty());
        prep.set_levels(vec![0.0; 5].into_iter().chain(vec![0.3; 5]).collect(), Vec::new());
        let b = prep.backing_frames(&em);
        assert!(b[30] > 0.9 && b[10] == 0.0, "{b:?}");
    }

    /// Frames are cut from each window at whole strides of the model: a
    /// click is heard at its own frame in every window, the first and the
    /// shifted hearing alike.
    #[test]
    fn every_window_keeps_its_frames_in_place() {
        // A wav2vec2-like model: a frame every 320 samples over 400 of
        // them; letter 1 where the frame holds a click.
        let mut run = |x: Vec<f32>| -> Result<(Vec<i64>, Vec<f32>), Error> {
            let frames = (x.len() - 400) / 320 + 1;
            let logits: Vec<f32> = (0..frames).flat_map(|j| if x[j * 320..j * 320 + 400].iter().any(|v| *v > 5.0) { [0.0, 9.0] } else { [9.0, 0.0] }).collect();
            Ok((vec![1, frames as i64, 2], logits))
        };
        let secs = 100;
        let mut voice = vec![0.0f32; secs * RATE as usize];
        let clicks: Vec<usize> = (1..secs).map(|s| s * RATE as usize + 3200 * (s % 7)).collect();
        for &c in &clicks {
            voice[c] = 1.0;
        }
        for shift in [0, SHIFT_S * RATE as usize] {
            let em = emissions_with(&mut run, &voice, 0, shift, true, &mut |_| true).unwrap();
            assert_eq!(em.fps, 50.0);
            assert!(em.frames().abs_diff(secs * 50) <= 1, "{}", em.frames());
            for &c in &clicks {
                let t = c / 320;
                assert!(em.row(t)[1] > em.row(t)[0], "shift {shift}: the click at {c} is not at frame {t}");
            }
        }
    }

    #[test]
    fn english_falls_back_to_the_multilingual_aligner() {
        assert_eq!(fallback(ModelId::AlignerEn), Some(ModelId::AlignerMms));
        assert_eq!(fallback(ModelId::AlignerRu), None);
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::new(dir.path());
        let ready = |id: ModelId| {
            std::fs::create_dir_all(catalog.path(id)).unwrap();
            std::fs::write(catalog.path(id).join(".ready"), b"").unwrap();
        };
        let a = Aligner::new(catalog.clone(), Device::Cpu, 1);
        // Neither there: the English one is what to download.
        assert_eq!(a.usable(ModelId::AlignerEn), ModelId::AlignerEn);
        ready(ModelId::AlignerMms);
        assert_eq!(a.usable(ModelId::AlignerEn), ModelId::AlignerMms);
        ready(ModelId::AlignerEn);
        assert_eq!(a.usable(ModelId::AlignerEn), ModelId::AlignerEn);
    }
}

/// RMS of `x` (at `rate`) every tenth of a second.
fn tenths(x: &[f32], rate: u32) -> Vec<f32> {
    let hop = (rate as usize / 10).max(1);
    x.chunks(hop).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len().max(1) as f32).sqrt()).collect()
}

fn percentile(v: &[f32], q: f32) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s.get(((s.len() as f32 - 1.0) * q).round() as usize).copied().unwrap_or(0.0)
}

/// The aligner for a language (docs/lyrics-align.md, «Models chosen»).
pub fn aligner_for(lang: Lang) -> Option<ModelId> {
    match lang {
        Lang::Russian => Some(ModelId::AlignerRu),
        Lang::Japanese => Some(ModelId::AlignerJa),
        Lang::English => Some(ModelId::AlignerEn),
        Lang::Other => Some(ModelId::AlignerMms),
        // Han without kana and Hangul: no aligner spells them yet.
        Lang::Chinese | Lang::Korean => None,
    }
}

/// What hears a language when its own aligner is not there (and may not
/// be downloaded): the multilingual one, which it was measured against.
pub fn fallback(id: ModelId) -> Option<ModelId> {
    (id == ModelId::AlignerEn).then_some(ModelId::AlignerMms)
}

/// Which aligner hears each line of a text: the text's language's, and
/// for a line in another script its own (an English line of a Russian
/// song).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    pub main: ModelId,
    /// One per line.
    pub lines: Vec<ModelId>,
}

impl Route {
    /// `None` when no aligner spells the text's language.
    pub fn of(lines: &[LyricLine]) -> Option<Self> {
        Self::of_with(lines, Tuning::default().by_sound)
    }

    /// [`Self::of`]; `by_sound`: the Latin lines of a mostly Russian text
    /// stay with the Russian aligner, spelled by sound ([`Tuning::by_sound`]).
    pub fn of_with(lines: &[LyricLine], by_sound: bool) -> Option<Self> {
        let text: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        let lang = Lang::of(&text);
        let main = aligner_for(lang)?;
        let keep = by_sound && lang == Lang::Russian && latin_share(&text) < BILINGUAL;
        let lines = Lang::of_lines(lang, lines.iter().map(|l| l.text.as_str()))
            .into_iter()
            .map(|l| if keep && l == Lang::English { main } else { aligner_for(l).unwrap_or(main) })
            .collect();
        Some(Self { main, lines })
    }

    /// Every aligner it uses, the text's own first.
    pub fn models(&self) -> Vec<ModelId> {
        let mut out = vec![self.main];
        for id in &self.lines {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        out
    }
}

/// A Russian text with this share of its words in Latin letters or more is
/// sung in two languages: its English lines go to the English aligner.
const BILINGUAL: f32 = 0.34;

/// The share of a text's words written in Latin letters.
fn latin_share(text: &str) -> f32 {
    let words: Vec<&str> = text.split_whitespace().filter(|w| w.chars().any(char::is_alphabetic)).collect();
    let latin = words.iter().filter(|w| w.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_ascii_alphabetic())).count();
    latin as f32 / words.len().max(1) as f32
}

fn letters_of(id: ModelId) -> Letters {
    if matches!(id, ModelId::AlignerMms | ModelId::AlignerSung) { Letters::Roman } else { Letters::Native }
}

/// An aligner's `vocab.json` (GigaAM's list of letters read as one).
pub fn vocab_text(catalog: &Catalog, id: ModelId) -> Result<String, Error> {
    if id == ModelId::AlignerRuDeep {
        return Ok(crate::gigaam::vocab_json(&std::fs::read_to_string(catalog.file(id, "vocab.txt"))?));
    }
    Ok(std::fs::read_to_string(catalog.file(id, "vocab.json"))?)
}

/// One model of a kind held at a time: the one before is dropped before
/// another is loaded, so two aligners never sit in memory together.
pub struct OneLoaded<T> {
    item: Option<(ModelId, T)>,
}

impl<T> Default for OneLoaded<T> {
    fn default() -> Self {
        Self { item: None }
    }
}

impl<T> OneLoaded<T> {
    /// `id`'s model, loaded by `load` (after the one held is dropped)
    /// when it is not the one held.
    pub fn get<E>(&mut self, id: ModelId, load: impl FnOnce() -> Result<T, E>) -> Result<&mut T, E> {
        if self.item.as_ref().is_none_or(|(held, _)| *held != id) {
            self.item = None;
            self.item = Some((id, load()?));
        }
        Ok(&mut self.item.as_mut().expect("loaded above").1)
    }

    pub fn held(&self) -> Option<(ModelId, &T)> {
        self.item.as_ref().map(|(id, t)| (*id, t))
    }

    pub fn clear(&mut self) {
        self.item = None;
    }
}

/// The loaded models, kept modest (a whole-library pass runs for hours in
/// the background): one aligner at a time, the separator only while it
/// separates, Whisper only while it recognises.
pub struct Aligner {
    catalog: Catalog,
    device: Device,
    threads: usize,
    separator: Option<Model>,
    ctc: OneLoaded<Model>,
    /// The aligners' letters (small; kept once read).
    vocabs: HashMap<ModelId, Vocab>,
    whisper: Option<Whisper>,
    /// How the timing pass places words.
    tuning: Tuning,
    /// What deep mode adds ([`Deep::OFF`]: nothing).
    deep: Deep,
}

impl Aligner {
    pub fn new(catalog: Catalog, device: Device, threads: usize) -> Self {
        Self { catalog, device, threads, separator: None, ctc: OneLoaded::default(), vocabs: HashMap::new(), whisper: None, tuning: Tuning::default(), deep: Deep::OFF }
    }

    /// How the timing pass places words from now on.
    pub fn set_tuning(&mut self, tuning: Tuning) {
        self.tuning = tuning;
    }

    pub fn tuning(&self) -> Tuning {
        self.tuning
    }

    /// What deep mode adds from now on ([`Deep::OFF`] for the normal
    /// timing).
    pub fn set_deep(&mut self, deep: Deep) {
        self.deep = deep;
    }

    pub fn deep(&self) -> Deep {
        self.deep
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Where the models run now (the GPU falls back to the CPU).
    pub fn device(&self) -> Device {
        self.separator
            .as_ref()
            .map(|m| m.device)
            .or_else(|| self.ctc.held().map(|(_, m)| m.device))
            .unwrap_or(self.device)
    }

    /// Another device: the models load again there.
    pub fn set_device(&mut self, device: Device) {
        if device != self.device {
            self.device = device;
            self.unload();
        }
    }

    /// Frees the models' memory.
    pub fn unload(&mut self) {
        self.separator = None;
        self.ctc.clear();
        self.whisper = None;
    }

    pub fn loaded(&self) -> bool {
        self.separator.is_some() || self.ctc.held().is_some() || self.whisper.is_some()
    }

    /// The aligner held in memory, if any.
    pub fn aligner_loaded(&self) -> Option<ModelId> {
        self.ctc.held().map(|(id, _)| id)
    }

    fn runtime(&self) -> Result<(), Error> {
        if !self.catalog.ready(ModelId::Runtime) {
            return Err(Error::Missing("ONNX Runtime"));
        }
        crate::engine::load_runtime(&self.catalog.path(ModelId::Runtime))
    }

    /// Decodes `path` and, with `separate`, takes the voice out of the mix.
    pub fn prepare(&mut self, path: &Path, separate: bool, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Prepared, Error> {
        if !progress(Stage::Decode) {
            return Err(Error::Stopped);
        }
        let decoded = audio::decode(path)?;
        let mix_level = tenths(&decoded.mono(), decoded.rate);
        let mut side = Vec::new();
        let separated = separate && self.catalog.ready(ModelId::Separator);
        let mut deep_separated = false;
        let voice = if separated {
            self.runtime()?;
            // Separation takes the most memory of all: the aligner held
            // from the song before goes first (it loads again in a second).
            self.ctc.clear();
            let mix = decoded.resampled(separate::RATE)?;
            let mut voice = None;
            if self.deep.separator && self.catalog.ready(ModelId::SeparatorDeep) {
                match self.separate_deep(mix.clone(), progress) {
                    Ok(v) => {
                        voice = Some(v);
                        deep_separated = true;
                    }
                    Err(Error::Stopped) => return Err(Error::Stopped),
                    // A card short of memory or a runtime without its
                    // operators: the normal separator does it.
                    Err(e) => log::warn!("align: deep separation failed, separating as usual: {e}"),
                }
            }
            let voice = match voice {
                Some(v) => v,
                None => {
                    if self.separator.is_none() {
                        let file = self.catalog.file(ModelId::Separator, "Kim_Vocal_2.onnx");
                        self.separator = Some(Model::load(&file, self.device, self.threads)?);
                    }
                    let model = self.separator.as_mut().expect("loaded above");
                    let voice = separate::vocals(model, KIM_VOCAL_2, mix, &mut |f| progress(Stage::Separate(f)));
                    // The separator's working memory is the largest of all: it goes
                    // as soon as the voice is out (loading it again takes a second).
                    self.separator = None;
                    voice?
                }
            };
            let voice = voice.resampled(RATE)?;
            if let [l, r] = &voice.channels[..] {
                let s: Vec<f32> = l.iter().zip(r).map(|(a, b)| (a - b) / 2.0).collect();
                side = tenths(&s, RATE);
            }
            voice.mono()
        } else {
            decoded.resampled(RATE)?.mono()
        };
        let mut prep = Prepared::from_voice(voice, separated);
        prep.set_levels(side, mix_level);
        prep.deep_separated = deep_separated;
        Ok(prep)
    }

    /// The voice by deep mode's separator (Mel-Band RoFormer), loaded for
    /// this separation only.
    fn separate_deep(&mut self, mix: audio::Audio, progress: &mut dyn FnMut(Stage) -> bool) -> Result<audio::Audio, Error> {
        let file = self.catalog.file(ModelId::SeparatorDeep, ModelId::SeparatorDeep.model_file());
        let mut model = Model::load(&file, self.device, self.threads)?;
        let step = if self.deep.overlap { ROFORMER_CHUNK / 2 } else { ROFORMER_CHUNK };
        // This export gives the voice's spectrum, not a mask.
        separate::vocals_roformer(&mut model, mix, step, false, &mut |f| progress(Stage::Separate(f)))
    }

    /// Aligner `id`'s letters.
    fn vocab(&mut self, id: ModelId) -> Result<Vocab, Error> {
        if let Some(v) = self.vocabs.get(&id) {
            let mut v = v.clone();
            v.by_sound = self.tuning.by_sound;
            return Ok(v);
        }
        if !self.catalog.ready(id) {
            return Err(Error::Missing("aligner"));
        }
        let json = vocab_text(&self.catalog, id)?;
        let vocab = Vocab::from_json(&json, letters_of(id)).map_err(|e| Error::Model(format!("vocab: {e}")))?;
        self.vocabs.insert(id, vocab.clone());
        let mut vocab = vocab;
        vocab.by_sound = self.tuning.by_sound;
        Ok(vocab)
    }

    /// What aligner `id` hears in the song (computed once per song; the
    /// aligner held before is dropped first).
    pub fn hear(&mut self, prep: &mut Prepared, id: ModelId, progress: &mut dyn FnMut(Stage) -> bool) -> Result<(), Error> {
        if prep.heard.contains_key(&id) {
            return Ok(());
        }
        let blank = self.vocab(id)?.blank;
        self.runtime()?;
        let (file, device, threads) = (self.catalog.file(id, "model_int8.onnx"), self.device, self.threads);
        let model = self.ctc.get(id, || Model::load(&file, device, threads))?;
        let windows = self.tuning.windows || self.deep.windows;
        let second = self.deep.second(id).filter(|(s, _)| self.catalog.ready(*s));
        // The share of the hearing each pass is, for the progress.
        let passes = 1.0 + f32::from(u8::from(windows)) + f32::from(u8::from(second.is_some()));
        // Deep mode hears at whole strides ([`emissions_exact`]): its
        // hearings line up with each other.
        type Hear = fn(&mut Model, &[f32], usize, usize, &mut dyn FnMut(f32) -> bool) -> Result<Emissions, Error>;
        let hear: Hear = if self.deep == Deep::OFF { emissions } else { emissions_exact };
        let mut em = hear(model, &prep.voice, blank, 0, &mut |f| progress(Stage::Listen(f / passes)))?;
        if windows {
            let other = hear(model, &prep.voice, blank, SHIFT_S * RATE as usize, &mut |f| progress(Stage::Listen((1.0 + f) / passes)))?;
            em.merge(&other);
        }
        prep.heard.insert(id, em);
        if let Some((sid, how)) = second {
            let done = passes - 1.0;
            match self.hear_second(prep, id, sid, how, &mut |f| progress(Stage::Listen((done + f) / passes))) {
                Ok(()) => {}
                Err(Error::Stopped) => return Err(Error::Stopped),
                // Short of memory or a model that will not load: the
                // first aligner's timing stands.
                Err(e) => log::warn!("align: the second aligner {sid:?} failed, timing as usual: {e}"),
            }
        }
        Ok(())
    }

    /// What deep mode's second aligner `sid` hears, used for aligner
    /// `id`'s texts as `how` says ([`Prepared::deep`]).
    fn hear_second(&mut self, prep: &mut Prepared, id: ModelId, sid: ModelId, how: Second, progress: &mut dyn FnMut(f32) -> bool) -> Result<(), Error> {
        let em = self.hear_with(prep, sid, progress)?;
        let deep = self.second_heard(prep, id, sid, how, &em)?;
        prep.deep.insert(id, deep);
        Ok(())
    }

    /// What aligner `id`'s texts are timed by in deep mode, from what its
    /// second aligner `sid` heard (`em`, at its own frame rate).
    pub fn second_heard(&mut self, prep: &Prepared, id: ModelId, sid: ModelId, how: Second, em: &Emissions) -> Result<(Emissions, Vocab), Error> {
        let main = prep.heard.get(&id).ok_or(Error::Missing("aligner"))?;
        let em = deep::earlier(&em.at_rate(main.fps), self.deep.late(sid));
        Ok(match how {
            Second::Fused => {
                let (a, b) = (deep::letters(&vocab_text(&self.catalog, id)?), deep::letters(&vocab_text(&self.catalog, sid)?));
                (deep::fuse(main, &a, &em, &b, self.deep.weight), self.vocab(id)?)
            }
            _ => (em, self.vocab(sid)?),
        })
    }

    /// What aligner `id` (any, deep mode's second ones too) hears in the
    /// song, at its own frame rate; the aligner held before goes first.
    pub fn hear_with(&mut self, prep: &Prepared, id: ModelId, progress: &mut dyn FnMut(f32) -> bool) -> Result<Emissions, Error> {
        let blank = self.vocab(id)?.blank;
        self.runtime()?;
        let (file, device, threads) = (self.catalog.file(id, id.model_file()), self.device, self.threads);
        let model = self.ctc.get(id, || Model::load(&file, device, threads))?;
        if id == ModelId::AlignerRuDeep {
            crate::gigaam::emissions(model, &prep.voice, blank, progress)
        } else {
            emissions_exact(model, &prep.voice, blank, 0, progress)
        }
    }

    /// Where aligner `id` runs now, if it is held.
    pub fn held_device(&self) -> Option<Device> {
        self.ctc.held().map(|(_, m)| m.device)
    }

    /// What aligner `id` hears over windows cut [`SHIFT_S`] later than
    /// [`Self::hear`]'s (for [`Tuning::windows`], kept apart by the bench).
    pub fn hear_shifted(&mut self, prep: &Prepared, id: ModelId) -> Result<Emissions, Error> {
        let blank = self.vocab(id)?.blank;
        self.runtime()?;
        let (file, device, threads) = (self.catalog.file(id, "model_int8.onnx"), self.device, self.threads);
        let model = self.ctc.get(id, || Model::load(&file, device, threads))?;
        emissions(model, &prep.voice, blank, SHIFT_S * RATE as usize, &mut |_| true)
    }

    /// [`Self::hear_shifted`] at whole strides, as deep mode hears
    /// ([`emissions_exact`]).
    pub fn hear_shifted_exact(&mut self, prep: &Prepared, id: ModelId) -> Result<Emissions, Error> {
        let blank = self.vocab(id)?.blank;
        self.runtime()?;
        let (file, device, threads) = (self.catalog.file(id, "model_int8.onnx"), self.device, self.threads);
        let model = self.ctc.get(id, || Model::load(&file, device, threads))?;
        emissions_exact(model, &prep.voice, blank, SHIFT_S * RATE as usize, &mut |_| true)
    }

    /// Fits a text to the song (`timed`: its line starts are real times).
    /// `None` when no aligner spells its script or it has nothing to align.
    /// Each line goes to the aligner of its language ([`Route`]) when that
    /// one is there; the text's own is needed.
    pub fn fit(&mut self, prep: &mut Prepared, lines: &[LyricLine], timed: bool, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Option<Fit>, Error> {
        let Some(route) = Route::of_with(lines, self.tuning.by_sound) else { return Ok(None) };
        let main = self.usable(route.main);
        let mut others: Vec<(ModelId, Vec<usize>)> = Vec::new();
        for (i, id) in route.lines.iter().enumerate() {
            let id = self.usable(*id);
            if id == main || !self.catalog.ready(id) {
                continue;
            }
            match others.iter_mut().find(|(o, _)| *o == id) {
                Some((_, ls)) => ls.push(i),
                None => others.push((id, vec![i])),
            }
        }
        self.hear(prep, main, progress)?;
        for (id, _) in &others {
            self.hear(prep, *id, progress)?;
        }
        if !progress(Stage::Align) {
            return Err(Error::Stopped);
        }
        let vocab = self.vocab(main)?;
        let mut vocabs = Vec::new();
        for (id, _) in &others {
            vocabs.push(self.vocab(*id)?);
        }
        let foreign: Vec<Foreign<'_>> =
            others.iter().zip(&vocabs).map(|((id, ls), v)| Foreign { em: &prep.heard[id], vocab: v, lines: ls }).collect();
        let em = &prep.heard[&main];
        let tuning = self.tuning;
        let voiced = prep.voiced_frames(em, tuning.mix_ratio);
        let onset = voiced.as_ref().map(|v| prep.onset_frames(em, v)).unwrap_or_default();
        let backing = prep.backing_frames(em);
        let voice = voiced.as_deref().map(|v| Voice { voiced: v, onset: &onset, backing: &backing });
        let fit = fit_text_with(em, &vocab, &foreign, lines, timed, voice, &tuning);
        // Deep mode: the word timing from what it heard (the foreign lines
        // from theirs where it heard them too), the verdict as above.
        let deep_fit = if prep.deep.contains_key(&main) || others.iter().any(|(id, _)| prep.deep.contains_key(id)) {
            let pick = |id: &ModelId, v: &Vocab| -> (&Emissions, Vocab) {
                match prep.deep.get(id) {
                    Some((e, dv)) => (e, dv.clone()),
                    None => (&prep.heard[id], v.clone()),
                }
            };
            let (dem, dvocab) = pick(&main, &vocab);
            let dforeign: Vec<(&Emissions, Vocab, &[usize])> = others
                .iter()
                .zip(&vocabs)
                .map(|((id, ls), v)| {
                    let (e, v) = pick(id, v);
                    (e, v, ls.as_slice())
                })
                .collect();
            let dforeign: Vec<Foreign<'_>> = dforeign.iter().map(|(e, v, ls)| Foreign { em: e, vocab: v, lines: ls }).collect();
            let voiced = prep.voiced_frames(dem, tuning.mix_ratio);
            let onset = voiced.as_ref().map(|v| prep.onset_frames(dem, v)).unwrap_or_default();
            let backing = prep.backing_frames(dem);
            let voice = voiced.as_deref().map(|v| Voice { voiced: v, onset: &onset, backing: &backing });
            fit_text_with(dem, &dvocab, &dforeign, lines, timed, voice, &tuning)
        } else {
            None
        };
        let fit = match (fit, deep_fit) {
            (Some(mut f), Some(d)) => {
                f.lines = d.lines;
                f.ends = d.ends;
                Some(f)
            }
            (f, _) => f,
        };
        Ok(fit.map(|mut f| {
            let bounds = f.ends.clone();
            prep.end_with_voice(&mut f.lines, main, &bounds, &tuning);
            f.covered = prep.covered(&f.lines);
            f.aligner = Some(main);
            f
        }))
    }

    /// `id`, or what stands in for it when it is not there and that is.
    pub fn usable(&self, id: ModelId) -> ModelId {
        match fallback(id) {
            Some(f) if !self.catalog.ready(id) && self.catalog.ready(f) => f,
            _ => id,
        }
    }

    /// [`Self::fit`] with a given aligner for every line.
    pub fn fit_with(&mut self, prep: &mut Prepared, id: ModelId, lines: &[LyricLine], timed: bool, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Option<Fit>, Error> {
        self.hear(prep, id, progress)?;
        if !progress(Stage::Align) {
            return Err(Error::Stopped);
        }
        let vocab = self.vocab(id)?;
        let em = &prep.heard[&id];
        let tuning = self.tuning;
        let voiced = prep.voiced_frames(em, tuning.mix_ratio);
        let onset = voiced.as_ref().map(|v| prep.onset_frames(em, v)).unwrap_or_default();
        let backing = prep.backing_frames(em);
        let voice = voiced.as_deref().map(|v| Voice { voiced: v, onset: &onset, backing: &backing });
        Ok(fit_text_with(em, &vocab, &[], lines, timed, voice, &tuning).map(|mut f| {
            let bounds = f.ends.clone();
            prep.end_with_voice(&mut f.lines, id, &bounds, &tuning);
            f.covered = prep.covered(&f.lines);
            f.aligner = Some(id);
            f
        }))
    }

    /// The aligner a text needs (to download it first); its foreign lines
    /// may want others ([`Route::models`]).
    pub fn needs(lines: &[LyricLine]) -> Option<ModelId> {
        Route::of(lines).map(|r| r.main)
    }

    /// Recognises the song from scratch: Whisper's lines, then their words
    /// aligned like any text. `languages` limits the language detection
    /// (empty: any).
    pub fn recognise(&mut self, prep: &mut Prepared, languages: &[&str], progress: &mut dyn FnMut(Stage) -> bool) -> Result<Vec<LyricLine>, Error> {
        if !self.catalog.ready(ModelId::Whisper) {
            return Err(Error::Missing("Whisper"));
        }
        self.runtime()?;
        if self.whisper.is_none() {
            // On the CPU: DirectML's int8 kernels turn this export's
            // decoder into noise (docs/lyrics-align.md).
            self.whisper = Some(Whisper::load(&self.catalog.path(ModelId::Whisper), Device::Cpu, self.threads.max(4))?);
        }
        let spans = whisper::stretches(&prep.voiced_tenths());
        if spans.is_empty() {
            return Ok(Vec::new());
        }
        // The language from the longest stretch.
        let (a, b) = spans.iter().copied().max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0))).unwrap_or((0.0, 30.0));
        let at = |s: f32| ((s * whisper::RATE as f32) as usize).min(prep.voice.len());
        let w = self.whisper.as_mut().expect("loaded above");
        let lang = w.language(&prep.voice[at(a)..at(b)], languages)?;
        log::info!("align: recognising in {lang}");
        let segments = w.transcribe(&prep.voice, &lang, &spans, &mut |f| progress(Stage::Recognise(f)));
        // Whisper (1.1 GB) is held only while it recognises.
        self.whisper = None;
        let segments = segments?;
        let lines: Vec<LyricLine> = segments
            .iter()
            .map(|s| LyricLine::new(std::time::Duration::from_secs_f32(s.start.max(0.0)), s.text.clone()))
            .collect();
        if lines.is_empty() {
            return Ok(lines);
        }
        // Word timing from the aligner when one spells the language;
        // otherwise the segments as lines.
        match Self::needs(&lines).map(|id| self.usable(id)).filter(|id| self.catalog.ready(*id)) {
            Some(_) => Ok(self.fit(prep, &lines, false, progress)?.map_or(lines.clone(), |f| f.lines)),
            None => Ok(lines),
        }
    }
}

/// Per-frame log-probabilities of a CTC model over the whole voice, in
/// windows with context so words at the edges are heard whole.
/// `shift`: the windows are cut this many samples later (the song is heard
/// after as much silence, which is left out again).
///
/// Each window's frames are cut by its own frames a second (a little under
/// 50), so each window loses its last frame and the times of the next ones
/// are stretched by about as much: within a few milliseconds, but not the
/// same for windows cut elsewhere. The normal timing was measured so and
/// stays so; deep mode, which adds hearings over other windows, cuts at
/// whole strides ([`emissions_exact`]).
pub fn emissions(model: &mut Model, voice: &[f32], blank: usize, shift: usize, progress: &mut dyn FnMut(f32) -> bool) -> Result<Emissions, Error> {
    emissions_with(&mut |x| model.run_f32_shaped(vec![1, x.len()], x), voice, blank, shift, false, progress)
}

/// [`emissions`] with each window's frames cut at whole strides of the
/// model (320 samples for wav2vec2): every frame kept, in its place, 50 a
/// second exactly, whatever the windows.
pub fn emissions_exact(model: &mut Model, voice: &[f32], blank: usize, shift: usize, progress: &mut dyn FnMut(f32) -> bool) -> Result<Emissions, Error> {
    emissions_with(&mut |x| model.run_f32_shaped(vec![1, x.len()], x), voice, blank, shift, true, progress)
}

/// [`emissions`] (`exact`: [`emissions_exact`]) with the model as a
/// function from a window's samples to its logits and their shape.
fn emissions_with(
    run: &mut dyn FnMut(Vec<f32>) -> Result<(Vec<i64>, Vec<f32>), Error>,
    voice: &[f32],
    blank: usize,
    shift: usize,
    exact: bool,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Emissions, Error> {
    if shift > 0 {
        let mut padded = vec![0.0f32; shift];
        padded.extend_from_slice(voice);
        let mut em = emissions_with(run, &padded, blank, 0, exact, progress)?;
        let skip = (shift as f32 / RATE as f32 * em.fps).round() as usize;
        em.logp.drain(..(skip * em.vocab).min(em.logp.len()));
        return Ok(em);
    }
    let sr = RATE as usize;
    let (win, ctx) = (WINDOW_S * sr, CONTEXT_S * sr);
    let n = voice.len();
    let level = (voice.iter().map(|v| v * v).sum::<f32>() / n.max(1) as f32).sqrt();
    let mut logp: Vec<f32> = Vec::new();
    let mut vocab = 0;
    let mut fps = 50.0f32;
    let mut stride: Option<usize> = None;
    let mut start = 0;
    while start < n {
        let (a, b) = (start.saturating_sub(ctx), (start + win + ctx).min(n));
        let mut x = voice[a..b].to_vec();
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        let var = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.len() as f32;
        let sd = (var + 1e-7).sqrt().max(level * QUIET);
        for v in &mut x {
            *v = (*v - mean) / sd;
        }
        let len = x.len();
        let (shape, mut logits) = run(x)?;
        let (frames, v) = match shape[..] {
            [1, t, v] => (t as usize, v as usize),
            _ => return Err(Error::Model(format!("aligner output shape {shape:?}"))),
        };
        vocab = v;
        Emissions::log_softmax(&mut logits, v);
        let (s, e) = if exact {
            // A window gives a frame every stride from its first sample;
            // the stride is read off the first (longest) window: a short
            // last one rounds off.
            let hop = *stride.get_or_insert(((len as f32 / frames.max(1) as f32).round() as usize).max(1));
            fps = sr as f32 / hop as f32;
            let s = ((start - a) / hop).min(frames);
            (s, (s + win.min(n - start).div_ceil(hop)).min(frames))
        } else {
            fps = frames as f32 / (len as f32 / sr as f32);
            let s = (((start - a) as f32 / sr as f32) * fps).round() as usize;
            (s, (s + ((win.min(n - start)) as f32 / sr as f32 * fps).round() as usize).min(frames))
        };
        logp.extend_from_slice(&logits[s * v..e * v]);
        start += win;
        if !progress((start.min(n)) as f32 / n as f32) {
            return Err(Error::Stopped);
        }
    }
    Ok(Emissions { logp, vocab, fps, blank })
}
