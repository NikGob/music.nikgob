//! The whole job for one song: decode, separate the voice, hear it with the
//! aligner of the text's script, fit texts, recognise one from scratch.
//! Models load on first use and stay until [`Aligner::unload`].

use std::collections::HashMap;
use std::path::Path;

use nikgob_core::lyrics::LyricLine;

use crate::catalog::{Catalog, ModelId};
use crate::ctc::Emissions;
use crate::engine::{Device, Model};
use crate::fit::{Fit, fit_text};
use crate::separate::{self, KIM_VOCAL_2};
use crate::text::{Letters, Script, Vocab};
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

fn percentile(v: &[f32], q: f32) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s.get(((s.len() as f32 - 1.0) * q).round() as usize).copied().unwrap_or(0.0)
}

/// The aligner for a text's script.
pub fn aligner_for(script: Script) -> Option<ModelId> {
    match script {
        Script::Cyrillic => Some(ModelId::AlignerRu),
        Script::Japanese => Some(ModelId::AlignerJa),
        Script::Latin | Script::Other => Some(ModelId::AlignerMms),
        // Han without kana and Hangul: no aligner spells them yet.
        Script::Chinese | Script::Korean => None,
    }
}

fn letters_of(id: ModelId) -> Letters {
    if id == ModelId::AlignerMms { Letters::Roman } else { Letters::Native }
}

/// The loaded models.
pub struct Aligner {
    catalog: Catalog,
    device: Device,
    threads: usize,
    separator: Option<Model>,
    ctc: HashMap<ModelId, (Model, Vocab)>,
    whisper: Option<Whisper>,
}

impl Aligner {
    pub fn new(catalog: Catalog, device: Device, threads: usize) -> Self {
        Self { catalog, device, threads, separator: None, ctc: HashMap::new(), whisper: None }
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Where the models run now (the GPU falls back to the CPU).
    pub fn device(&self) -> Device {
        self.separator
            .as_ref()
            .map(|m| m.device)
            .or_else(|| self.ctc.values().next().map(|(m, _)| m.device))
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
        self.separator.is_some() || !self.ctc.is_empty() || self.whisper.is_some()
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
            if self.separator.is_none() {
                let file = self.catalog.file(ModelId::Separator, "Kim_Vocal_2.onnx");
                self.separator = Some(Model::load(&file, self.device, self.threads)?);
            }
            let mix = decoded.resampled(separate::RATE)?;
            let model = self.separator.as_mut().expect("loaded above");
            let voice = separate::vocals(model, KIM_VOCAL_2, &mix, &mut |f| progress(Stage::Separate(f)))?;
            voice.resampled(RATE)?.mono()
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

    fn ctc(&mut self, id: ModelId) -> Result<&mut (Model, Vocab), Error> {
        if !self.ctc.contains_key(&id) {
            if !self.catalog.ready(id) {
                return Err(Error::Missing("aligner"));
            }
            self.runtime()?;
            let model = Model::load(&self.catalog.file(id, "model_int8.onnx"), self.device, self.threads)?;
            let json = std::fs::read_to_string(self.catalog.file(id, "vocab.json"))?;
            let vocab = Vocab::from_json(&json, letters_of(id)).map_err(|e| Error::Model(format!("vocab: {e}")))?;
            self.ctc.insert(id, (model, vocab));
        }
        Ok(self.ctc.get_mut(&id).expect("inserted above"))
    }

    /// What aligner `id` hears in the song (computed once per song).
    fn heard(&mut self, prep: &mut Prepared, id: ModelId, progress: &mut dyn FnMut(Stage) -> bool) -> Result<(), Error> {
        if prep.heard.contains_key(&id) {
            return Ok(());
        }
        let (model, vocab) = self.ctc(id)?;
        let blank = vocab.blank;
        let em = emissions(model, &prep.voice, blank, &mut |f| progress(Stage::Listen(f)))?;
        prep.heard.insert(id, em);
        Ok(())
    }

    /// Fits a text to the song (`timed`: its line starts are real times).
    /// `None` when no aligner spells its script or it has nothing to align.
    pub fn fit(&mut self, prep: &mut Prepared, lines: &[LyricLine], timed: bool, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Option<Fit>, Error> {
        let Some(id) = Self::needs(lines) else { return Ok(None) };
        self.fit_with(prep, id, lines, timed, progress)
    }

    /// [`Self::fit`] with a given aligner.
    pub fn fit_with(&mut self, prep: &mut Prepared, id: ModelId, lines: &[LyricLine], timed: bool, progress: &mut dyn FnMut(Stage) -> bool) -> Result<Option<Fit>, Error> {
        self.heard(prep, id, progress)?;
        if !progress(Stage::Align) {
            return Err(Error::Stopped);
        }
        let vocab = self.ctc(id)?.1.clone();
        Ok(fit_text(&prep.heard[&id], &vocab, lines, timed).map(|mut f| {
            f.covered = prep.covered(&f.lines);
            f
        }))
    }

    /// The aligners a text needs (to download them first).
    pub fn needs(lines: &[LyricLine]) -> Option<ModelId> {
        let text: String = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        aligner_for(Script::of(&text))
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
        let segments = w.transcribe(&prep.voice, &lang, &spans, &mut |f| progress(Stage::Recognise(f)))?;
        let lines: Vec<LyricLine> = segments
            .iter()
            .map(|s| LyricLine::new(std::time::Duration::from_secs_f32(s.start.max(0.0)), s.text.clone()))
            .collect();
        if lines.is_empty() {
            return Ok(lines);
        }
        // Word timing from the aligner when one spells the language;
        // otherwise the segments as lines.
        match Self::needs(&lines).filter(|id| self.catalog.ready(*id)) {
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
