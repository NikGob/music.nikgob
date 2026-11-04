//! The whole job for one song: decode, separate the voice, hear it with the
//! aligner of the text's script, fit texts, recognise one from scratch.
//! Models load on first use and stay until [`Aligner::unload`].

use std::collections::HashMap;
use std::path::Path;

use nikgob_core::lyrics::LyricLine;

use crate::catalog::{Catalog, ModelId};
use crate::ctc::Emissions;
use crate::engine::{Device, Model};
use crate::fit::{Fit, Foreign, fit_text_with};
use crate::separate::{self, KIM_VOCAL_2};
use crate::text::{Lang, Letters, Vocab};
use crate::whisper::{self, Whisper};
use crate::{Error, audio};

/// The rate of the aligners and Whisper.
const RATE: u32 = 16_000;
/// Window of the aligners, with context on both sides.
const WINDOW_S: usize = 30;
const CONTEXT_S: usize = 2;
/// Voice energy is measured over this many samples (0.1 s).
const ENERGY_HOP: usize = 1600;

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
    heard: HashMap<ModelId, Emissions>,
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
        let text: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        let lang = Lang::of(&text);
        let main = aligner_for(lang)?;
        let lines = Lang::of_lines(lang, lines.iter().map(|l| l.text.as_str()))
            .into_iter()
            .map(|l| aligner_for(l).unwrap_or(main))
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

fn letters_of(id: ModelId) -> Letters {
    if id == ModelId::AlignerMms { Letters::Roman } else { Letters::Native }
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
}

impl Aligner {
    pub fn new(catalog: Catalog, device: Device, threads: usize) -> Self {
        Self { catalog, device, threads, separator: None, ctc: OneLoaded::default(), vocabs: HashMap::new(), whisper: None }
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
        let separated = separate && self.catalog.ready(ModelId::Separator);
        let voice = if separated {
            self.runtime()?;
            // Separation takes the most memory of all: the aligner held
            // from the song before goes first (it loads again in a second).
            self.ctc.clear();
            if self.separator.is_none() {
                let file = self.catalog.file(ModelId::Separator, "Kim_Vocal_2.onnx");
                self.separator = Some(Model::load(&file, self.device, self.threads)?);
            }
            let mix = decoded.resampled(separate::RATE)?;
            let model = self.separator.as_mut().expect("loaded above");
            let voice = separate::vocals(model, KIM_VOCAL_2, &mix, &mut |f| progress(Stage::Separate(f)));
            // The separator's working memory is the largest of all: it goes
            // as soon as the voice is out (loading it again takes a second).
            self.separator = None;
            drop(mix);
            voice?.resampled(RATE)?.mono()
        } else {
            decoded.resampled(RATE)?.mono()
        };
        let secs = voice.len() as f32 / RATE as f32;
        let energy = voice
            .chunks(ENERGY_HOP)
            .map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len().max(1) as f32).sqrt())
            .collect();
        Ok(Prepared { voice, secs, separated, energy, heard: HashMap::new() })
    }

    /// Aligner `id`'s letters.
    fn vocab(&mut self, id: ModelId) -> Result<Vocab, Error> {
        if let Some(v) = self.vocabs.get(&id) {
            return Ok(v.clone());
        }
        if !self.catalog.ready(id) {
            return Err(Error::Missing("aligner"));
        }
        let json = std::fs::read_to_string(self.catalog.file(id, "vocab.json"))?;
        let vocab = Vocab::from_json(&json, letters_of(id)).map_err(|e| Error::Model(format!("vocab: {e}")))?;
        self.vocabs.insert(id, vocab.clone());
        Ok(vocab)
    }

    /// What aligner `id` hears in the song (computed once per song; the
    /// aligner held before is dropped first).
    fn heard(&mut self, prep: &mut Prepared, id: ModelId, progress: &mut dyn FnMut(Stage) -> bool) -> Result<(), Error> {
        if prep.heard.contains_key(&id) {
            return Ok(());
        }
        let blank = self.vocab(id)?.blank;
        self.runtime()?;
        let (file, device, threads) = (self.catalog.file(id, "model_int8.onnx"), self.device, self.threads);
        let model = self.ctc.get(id, || Model::load(&file, device, threads))?;
        let em = emissions(model, &prep.voice, blank, &mut |f| progress(Stage::Listen(f)))?;
        prep.heard.insert(id, em);
        Ok(())
    }

    /// Fits a text to the song (`timed`: its line starts are real times).
    /// `None` when no aligner spells its script or it has nothing to align.
    /// Each line goes to the aligner of its language ([`Route`]) when that
    /// one is there; the text's own is needed.
    pub fn fit(&mut self, prep: &mut Prepared, lines: &[LyricLine], timed: bool, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Option<Fit>, Error> {
        let Some(route) = Route::of(lines) else { return Ok(None) };
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
        self.heard(prep, main, progress)?;
        for (id, _) in &others {
            self.heard(prep, *id, progress)?;
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
        Ok(fit_text_with(&prep.heard[&main], &vocab, &foreign, lines, timed).map(|mut f| {
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
        self.heard(prep, id, progress)?;
        if !progress(Stage::Align) {
            return Err(Error::Stopped);
        }
        let vocab = self.vocab(id)?;
        Ok(fit_text_with(&prep.heard[&id], &vocab, &[], lines, timed).map(|mut f| {
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
pub fn emissions(model: &mut Model, voice: &[f32], blank: usize, progress: &mut dyn FnMut(f32) -> bool) -> Result<Emissions, Error> {
    let sr = RATE as usize;
    let (win, ctx) = (WINDOW_S * sr, CONTEXT_S * sr);
    let n = voice.len();
    let mut logp: Vec<f32> = Vec::new();
    let mut vocab = 0;
    let mut fps = 50.0f32;
    let mut start = 0;
    while start < n {
        let (a, b) = (start.saturating_sub(ctx), (start + win + ctx).min(n));
        let mut x = voice[a..b].to_vec();
        let mean = x.iter().sum::<f32>() / x.len() as f32;
        let var = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / x.len() as f32;
        let sd = (var + 1e-7).sqrt();
        for v in &mut x {
            *v = (*v - mean) / sd;
        }
        let len = x.len();
        let (shape, mut logits) = model.run_f32_shaped(vec![1, len], x)?;
        let (frames, v) = match shape[..] {
            [1, t, v] => (t as usize, v as usize),
            _ => return Err(Error::Model(format!("aligner output shape {shape:?}"))),
        };
        vocab = v;
        Emissions::log_softmax(&mut logits, v);
        fps = frames as f32 / (len as f32 / sr as f32);
        let s = (((start - a) as f32 / sr as f32) * fps).round() as usize;
        let e = (s + ((win.min(n - start)) as f32 / sr as f32 * fps).round() as usize).min(frames);
        logp.extend_from_slice(&logits[s * v..e * v]);
        start += win;
        if !progress((start.min(n)) as f32 / n as f32) {
            return Err(Error::Stopped);
        }
    }
    Ok(Emissions { logp, vocab, fps, blank })
}
