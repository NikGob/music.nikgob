//! Recognition from scratch with Whisper large-v3-turbo (ONNX export of
//! the encoder and the merged decoder with its key/value cache), on the
//! separated voice: stretch by sung stretch (at most 30 s, cut at pauses),
//! greedy decoding without timestamps (Whisper's timestamp mode made up
//! subtitle credits on intros), the text cut into lines at its sentences.
//!
//! The timing comes afterwards from the CTC aligner (the text is aligned
//! like any other plain text).

use std::collections::HashMap;
use std::path::Path;

use ort::value::{DynValue, Tensor};
use realfft::RealFftPlanner;

use crate::Error;
use crate::engine::{Device, GraphOptimizationLevel, Model};

pub const RATE: u32 = 16_000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 128;
/// Samples of one window (30 s).
const WINDOW: usize = 30 * RATE as usize;
const FRAMES: usize = WINDOW / HOP;
/// Tokens decoded per stretch at most.
const MAX_TOKENS: usize = 200;
/// Seconds a timestamp token stands for.

/// One recognised line.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub start: f32,
    pub end: f32,
    pub text: String,
}

/// The special tokens and the byte-level vocabulary.
pub struct Tokenizer {
    /// Token id → its bytes.
    bytes: Vec<Vec<u8>>,
    special: HashMap<String, u32>,
    pub eot: u32,
    pub sot: u32,
    pub transcribe: u32,
    pub no_timestamps: u32,
    pub timestamp_begin: u32,
    /// `<|xx|>` language tokens (code, id).
    pub languages: Vec<(String, u32)>,
}

/// GPT-2's map of bytes to printable characters, reversed.
fn byte_decoder() -> HashMap<char, u8> {
    let mut bs: Vec<u32> = (u32::from(b'!')..=u32::from(b'~')).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    bs.into_iter()
        .zip(cs)
        .filter_map(|(b, c)| Some((char::from_u32(c)?, b as u8)))
        .collect()
}

impl Tokenizer {
    /// From a Hugging Face `tokenizer.json`.
    pub fn from_json(json: &str) -> Result<Self, Error> {
        #[derive(serde::Deserialize)]
        struct Added {
            id: u32,
            content: String,
        }
        #[derive(serde::Deserialize)]
        struct BpeModel {
            vocab: HashMap<String, u32>,
        }
        #[derive(serde::Deserialize)]
        struct File {
            model: BpeModel,
            added_tokens: Vec<Added>,
        }
        let f: File = serde_json::from_str(json).map_err(|e| Error::Model(format!("tokenizer: {e}")))?;
        let dec = byte_decoder();
        let size = f.model.vocab.values().chain(f.added_tokens.iter().map(|a| &a.id)).max().map_or(0, |m| *m as usize + 1);
        let mut bytes = vec![Vec::new(); size];
        for (tok, id) in &f.model.vocab {
            bytes[*id as usize] = tok.chars().filter_map(|c| dec.get(&c).copied()).collect();
        }
        let special: HashMap<String, u32> = f.added_tokens.iter().map(|a| (a.content.clone(), a.id)).collect();
        let get = |k: &str| special.get(k).copied().ok_or_else(|| Error::Model(format!("tokenizer has no {k}")));
        let mut languages: Vec<(String, u32)> = special
            .iter()
            .filter_map(|(k, id)| {
                let code = k.strip_prefix("<|")?.strip_suffix("|>")?;
                (code.len() >= 2 && code.len() <= 3 && code.chars().all(|c| c.is_ascii_lowercase())).then(|| (code.to_owned(), *id))
            })
            .collect();
        languages.sort_by_key(|(_, id)| *id);
        Ok(Self {
            eot: get("<|endoftext|>")?,
            sot: get("<|startoftranscript|>")?,
            transcribe: get("<|transcribe|>")?,
            no_timestamps: get("<|notimestamps|>")?,
            timestamp_begin: get("<|0.00|>")?,
            bytes,
            special,
            languages,
        })
    }

    pub fn language(&self, code: &str) -> Option<u32> {
        self.special.get(&format!("<|{code}|>")).copied()
    }

    /// The text of ordinary tokens (special ones are skipped).
    pub fn decode(&self, tokens: &[u32]) -> String {
        let bytes: Vec<u8> = tokens
            .iter()
            .filter(|t| **t < self.eot)
            .flat_map(|t| self.bytes.get(*t as usize).cloned().unwrap_or_default())
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

/// Slaney-style mel filters (librosa's, as Whisper's feature extractor).
fn mel_filters() -> Vec<f32> {
    let bins = N_FFT / 2 + 1;
    let hz_to_mel = |f: f32| if f < 1000.0 { 3.0 * f / 200.0 } else { 15.0 + (f / 1000.0).ln() / (6.4f32.ln() / 27.0) };
    let mel_to_hz = |m: f32| if m < 15.0 { 200.0 * m / 3.0 } else { 1000.0 * ((m - 15.0) * 6.4f32.ln() / 27.0).exp() };
    let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(8000.0));
    let pts: Vec<f32> = (0..N_MELS + 2).map(|i| mel_to_hz(lo + (hi - lo) * i as f32 / (N_MELS + 1) as f32)).collect();
    let mut w = vec![0.0f32; N_MELS * bins];
    for m in 0..N_MELS {
        let norm = 2.0 / (pts[m + 2] - pts[m]);
        for b in 0..bins {
            let f = b as f32 * RATE as f32 / N_FFT as f32;
            let up = (f - pts[m]) / (pts[m + 1] - pts[m]);
            let down = (pts[m + 2] - f) / (pts[m + 2] - pts[m + 1]);
            w[m * bins + b] = up.min(down).max(0.0) * norm;
        }
    }
    w
}

/// Whisper's log-mel features of one window (padded to 30 s): `128 × 3000`.
pub fn log_mel(samples: &[f32], filters: &[f32]) -> Vec<f32> {
    let bins = N_FFT / 2 + 1;
    let mut padded = samples[..samples.len().min(WINDOW)].to_vec();
    padded.resize(WINDOW, 0.0);
    let half = N_FFT / 2;
    let n = padded.len() as isize;
    let at = |i: isize| -> f32 {
        let i = if i < 0 { -i } else if i >= n { 2 * (n - 1) - i } else { i };
        padded[i.clamp(0, n - 1) as usize]
    };
    let window: Vec<f32> = (0..N_FFT).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N_FFT as f32).cos()).collect();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let mut frame = fft.make_input_vec();
    let mut spec = fft.make_output_vec();
    let mut mel = vec![0.0f32; N_MELS * FRAMES];
    let mut power = vec![0.0f32; bins];
    for t in 0..FRAMES {
        let start = (t * HOP) as isize - half as isize;
        for (i, x) in frame.iter_mut().enumerate() {
            *x = at(start + i as isize) * window[i];
        }
        let _ = fft.process(&mut frame, &mut spec);
        for (p, z) in power.iter_mut().zip(&spec) {
            *p = z.norm_sqr();
        }
        for m in 0..N_MELS {
            let row = &filters[m * bins..(m + 1) * bins];
            let v: f32 = row.iter().zip(&power).map(|(a, b)| a * b).sum();
            mel[m * FRAMES + t] = v.max(1e-10).log10();
        }
    }
    let max = mel.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    for v in &mut mel {
        *v = ((*v).max(max - 8.0) + 4.0) / 4.0;
    }
    mel
}

/// The two sessions and the tokenizer.
pub struct Whisper {
    encoder: Model,
    decoder: Model,
    pub tokenizer: Tokenizer,
    filters: Vec<f32>,
}

/// The decoder's cache between steps.
struct Cache {
    values: HashMap<String, DynValue>,
}

impl Whisper {
    /// From a folder with `encoder_model_int8.onnx`,
    /// `decoder_model_merged_int8.onnx` and `tokenizer.json`.
    pub fn load(dir: &Path, device: Device, threads: usize) -> Result<Self, Error> {
        let tokenizer = Tokenizer::from_json(&std::fs::read_to_string(dir.join("tokenizer.json"))?)?;
        Ok(Self {
            encoder: Model::load(&dir.join("encoder_model_int8.onnx"), device, threads)?,
            decoder: Model::load_with(&dir.join("decoder_model_merged_int8.onnx"), device, threads, GraphOptimizationLevel::Level1)?,
            tokenizer,
            filters: mel_filters(),
        })
    }

    pub fn device(&self) -> Device {
        self.encoder.device
    }

    fn encode(&mut self, samples: &[f32]) -> Result<DynValue, Error> {
        let mel = log_mel(samples, &self.filters);
        let input = Tensor::from_array((vec![1usize, N_MELS, FRAMES], mel)).map_err(model_err)?;
        let mut out = self.encoder.session().run(ort::inputs![input]).map_err(model_err)?;
        let key = out.keys().next().map(str::to_owned).ok_or_else(|| Error::Model("encoder gave nothing".into()))?;
        out.remove(&key).ok_or_else(|| Error::Model("encoder gave nothing".into()))
    }

    /// One decoder step: the logits of the last position.
    fn step(&mut self, tokens: &[u32], encoded: &DynValue, cache: &mut Option<Cache>) -> Result<Vec<f32>, Error> {
        let session = self.decoder.session();
        let names: Vec<(String, Vec<i64>)> = session
            .inputs()
            .iter()
            .map(|i| {
                let dims = match i.dtype() {
                    ort::value::ValueType::Tensor { shape, .. } => shape.iter().copied().collect(),
                    _ => Vec::new(),
                };
                (i.name().to_owned(), dims)
            })
            .collect();
        let ids: Vec<i64> = tokens.iter().map(|t| i64::from(*t)).collect();
        let mut inputs: Vec<(String, ort::session::SessionInputValue<'_>)> = Vec::new();
        let first = cache.is_none();
        for (name, dims) in &names {
            let value: ort::session::SessionInputValue<'_> = if name == "input_ids" {
                Tensor::from_array((vec![1usize, ids.len()], ids.clone())).map_err(model_err)?.into()
            } else if name == "encoder_hidden_states" {
                encoded.into()
            } else if name == "use_cache_branch" {
                Tensor::from_array((vec![1usize], vec![!first])).map_err(model_err)?.into()
            } else if let Some(key) = name.strip_prefix("past_key_values.") {
                match cache.as_ref().and_then(|c| c.values.get(&format!("present.{key}"))) {
                    Some(v) => v.into(),
                    None => {
                        // The first step has no past: empty along the sequence.
                        let shape: Vec<usize> = dims.iter().enumerate().map(|(i, d)| if i == 2 { 0 } else if *d > 0 { *d as usize } else { 1 }).collect();
                        let shape: Vec<usize> = if dims.len() == 4 { vec![1, shape[1], 0, shape[3]] } else { shape };
                        Tensor::<f32>::from_array((shape, Vec::new())).map_err(model_err)?.into()
                    }
                }
            } else {
                return Err(Error::Model(format!("unknown decoder input {name}")));
            };
            inputs.push((name.clone(), value));
        }
        let mut out = session.run(inputs).map_err(model_err)?;
        let (shape, logits) = out["logits"].try_extract_tensor::<f32>().map_err(model_err)?;
        let vocab = *shape.last().unwrap_or(&0) as usize;
        let last = logits[logits.len() - vocab..].to_vec();
        let mut values = cache.take().map(|c| c.values).unwrap_or_default();
        let keys: Vec<String> = out.keys().filter(|k| k.starts_with("present.")).map(str::to_owned).collect();
        for k in keys {
            // The encoder's cache is made on the first step only; later
            // steps give it back empty.
            if !first && k.contains(".encoder.") {
                continue;
            }
            if let Some(v) = out.remove(&k) {
                values.insert(k, v);
            }
        }
        *cache = Some(Cache { values });
        Ok(last)
    }

    /// The language of a window (its code), among `allowed` when given.
    pub fn language(&mut self, samples: &[f32], allowed: &[&str]) -> Result<String, Error> {
        let encoded = self.encode(samples)?;
        let mut cache = None;
        let logits = self.step(&[self.tokenizer.sot], &encoded, &mut cache)?;
        let best = self
            .tokenizer
            .languages
            .iter()
            .filter(|(code, _)| allowed.is_empty() || allowed.contains(&code.as_str()))
            .max_by(|a, b| logits[a.1 as usize].total_cmp(&logits[b.1 as usize]))
            .map(|(c, _)| c.clone());
        best.ok_or_else(|| Error::Model("no language".into()))
    }

    /// Decodes one stretch of voice (at most 30 s) greedily, without
    /// timestamps: its text.
    fn text_of(&mut self, samples: &[f32], lang: u32) -> Result<String, Error> {
        let tk = &self.tokenizer;
        let prompt = vec![tk.sot, lang, tk.transcribe, tk.no_timestamps];
        let encoded = self.encode(samples)?;
        let mut cache = None;
        let mut tokens: Vec<u32> = Vec::new();
        let mut feed = prompt;
        for _ in 0..MAX_TOKENS {
            let logits = self.step(&feed, &encoded, &mut cache)?;
            let next = pick(&logits, self.tokenizer.eot);
            if next == self.tokenizer.eot {
                break;
            }
            tokens.push(next);
            feed = vec![next];
            if repeating(&tokens) {
                break;
            }
        }
        Ok(self.tokenizer.decode(&tokens).trim().to_owned())
    }

    /// Recognises a voice track (16 kHz mono) stretch by stretch: `spans`
    /// are the sung stretches (seconds, at most 30 s each, see
    /// [`stretches`]). `progress` gets the done share and stops the work
    /// when it returns false. Each recognised sentence is a line starting
    /// at its stretch (the aligner times it).
    pub fn transcribe(
        &mut self,
        samples: &[f32],
        language: &str,
        spans: &[(f32, f32)],
        progress: &mut dyn FnMut(f32) -> bool,
    ) -> Result<Vec<Segment>, Error> {
        let lang = self.tokenizer.language(language).ok_or_else(|| Error::Model(format!("no language {language}")))?;
        let mut out: Vec<Segment> = Vec::new();
        for (i, &(a, b)) in spans.iter().enumerate() {
            let (x, y) = ((a * RATE as f32) as usize, ((b * RATE as f32) as usize).min(samples.len()));
            if y <= x {
                continue;
            }
            let text = self.text_of(&samples[x..y], lang)?;
            for line in lines_of(&text) {
                // Whisper's habit on music: subtitle credits and sign-offs.
                if hallucinated(&line) {
                    continue;
                }
                if out.last().is_some_and(|p| p.text == line) && out.iter().rev().take(3).filter(|p| p.text == line).count() >= 3 {
                    continue;
                }
                out.push(Segment { start: a, end: b, text: line });
            }
            if !progress((i + 1) as f32 / spans.len() as f32) {
                return Err(Error::Stopped);
            }
        }
        Ok(out)
    }
}

/// The sung stretches of a track from its voice's energy (one value a
/// tenth of a second, `voiced` above the floor): runs of voice joined
/// across pauses shorter than 0.8 s, cut to at most 28 s, with a little
/// room on both sides.
pub fn stretches(voiced: &[bool]) -> Vec<(f32, f32)> {
    const MAX: usize = 280;
    const GAP: usize = 8;
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < voiced.len() {
        if !voiced[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < voiced.len() && voiced[i] {
            i += 1;
        }
        match runs.last_mut() {
            Some(last) if start - last.1 < GAP && i - last.0 <= MAX => last.1 = i,
            _ => runs.push((start, i)),
        }
    }
    let mut out = Vec::new();
    for (a, b) in runs {
        let mut s = a;
        while s < b {
            let e = (s + MAX).min(b);
            // Too short to say anything (a breath).
            if e - s >= 3 {
                out.push(((s.saturating_sub(3)) as f32 / 10.0, (e + 3).min(voiced.len()) as f32 / 10.0));
            }
            s = e;
        }
    }
    out
}

/// A recognised text in lines: sentences, and a long one at its commas.
fn lines_of(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        cur.push(c);
        if matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？') {
            out.push(std::mem::take(&mut cur));
        }
    }
    out.push(cur);
    let mut lines = Vec::new();
    for s in out {
        let s = s.trim().trim_end_matches(['.', '。']).trim().to_owned();
        if s.is_empty() {
            continue;
        }
        if s.split_whitespace().count() > 8 && s.contains(',') {
            lines.extend(s.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()));
        } else {
            lines.push(s);
        }
    }
    lines
}

/// Lines Whisper makes up over music (credits of subtitles it learnt from).
fn hallucinated(line: &str) -> bool {
    let l = line.to_lowercase();
    ["субтитры", "редактор субтитров", "спасибо за просмотр", "продолжение следует", "подписывайтесь", "thanks for watching", "thank you for watching", "ご視聴ありがとう", "字幕"]
        .iter()
        .any(|p| l.contains(p))
}

/// The likeliest ordinary token, or the end.
fn pick(logits: &[f32], eot: u32) -> u32 {
    logits
        .iter()
        .enumerate()
        .take(eot as usize + 1)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map_or(eot, |(i, _)| i as u32)
}

/// The last tokens loop (a short run said again and again).
fn repeating(tokens: &[u32]) -> bool {
    for n in 1..=8 {
        let reps = 4;
        if tokens.len() < n * reps {
            continue;
        }
        let tail = &tokens[tokens.len() - n * reps..];
        if (1..reps).all(|r| tail[..n] == tail[r * n..(r + 1) * n]) {
            return true;
        }
    }
    false
}

fn model_err(e: impl std::fmt::Display) -> Error {
    Error::Model(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_filters_are_triangles_summing_like_librosa() {
        let f = mel_filters();
        let bins = N_FFT / 2 + 1;
        assert_eq!(f.len(), N_MELS * bins);
        assert!(f.iter().all(|v| *v >= 0.0));
        // Every filter has some weight, and nothing above 8 kHz is used.
        for m in 0..N_MELS {
            assert!(f[m * bins..(m + 1) * bins].iter().any(|v| *v > 0.0), "{m}");
        }
        assert!(f[(N_MELS - 1) * bins + bins - 1] < 1e-6);
    }

    #[test]
    fn stretches_join_short_pauses_and_split_long_singing() {
        let mut v = vec![false; 1000];
        v[100..150].iter_mut().for_each(|x| *x = true);
        v[155..200].iter_mut().for_each(|x| *x = true);
        v[400..401].iter_mut().for_each(|x| *x = true);
        v[500..900].iter_mut().for_each(|x| *x = true);
        let s = stretches(&v);
        assert_eq!(s[0], (9.7, 20.3));
        assert_eq!(s.len(), 3, "{s:?}");
        assert!(s[1..].iter().all(|(a, b)| b - a <= 28.7));
        assert!((s[1].0 - 49.7).abs() < 1e-4);
    }

    #[test]
    fn lines_from_sentences_and_credits_dropped() {
        assert_eq!(lines_of("Мой топор, я будто тролль. Я разрублю! Что?"), ["Мой топор, я будто тролль", "Я разрублю!", "Что?"]);
        assert_eq!(lines_of("раз два три четыре пять шесть семь восемь, девять десять"), ["раз два три четыре пять шесть семь восемь", "девять десять"]);
        assert!(hallucinated("Субтитры сделал DimaTorzok"));
        assert!(!hallucinated("Мой топор"));
    }

    #[test]
    fn log_mel_has_whisper_shape_and_range() {
        let tone: Vec<f32> = (0..RATE as usize).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin() * 0.5).collect();
        let m = log_mel(&tone, &mel_filters());
        assert_eq!(m.len(), N_MELS * FRAMES);
        let max = m.iter().copied().fold(f32::MIN, f32::max);
        let min = m.iter().copied().fold(f32::MAX, f32::min);
        assert!((max - min - 2.0).abs() < 1e-3, "{min} {max}");
    }

    #[test]
    fn byte_level_decoding_and_text_only_tokens() {
        let json = r#"{"model":{"vocab":{"Ġпр":0,"и":1,"вет":2,"!":3}},"added_tokens":[
            {"id":10,"content":"<|endoftext|>"},{"id":11,"content":"<|startoftranscript|>"},
            {"id":12,"content":"<|ru|>"},{"id":13,"content":"<|transcribe|>"},
            {"id":14,"content":"<|notimestamps|>"},{"id":15,"content":"<|0.00|>"}]}"#;
        // «Ġпр» in the vocabulary is GPT-2's spelling of the bytes; build
        // the right one from the real bytes instead.
        let enc: HashMap<u8, char> = byte_decoder().into_iter().map(|(c, b)| (b, c)).collect();
        let spell = |s: &str| s.bytes().map(|b| enc[&b]).collect::<String>();
        let json = json.replace("Ġпр", &spell(" пр")).replace("\"и\"", &format!("\"{}\"", spell("и"))).replace("вет", &spell("вет"));
        let tk = Tokenizer::from_json(&json).unwrap();
        assert_eq!(tk.decode(&[0, 1, 2, 3, 10]), " привет!");
        assert_eq!(tk.language("ru"), Some(12));
        assert_eq!(tk.languages, vec![("ru".to_owned(), 12)]);

        // Vocabulary: 0..4 text, 10 eot, 14 no-ts, 15.. timestamps: only
        // text or the end is picked.
        let mut logits = vec![0.0f32; 40];
        logits[2] = 5.0;
        logits[20] = 9.0;
        assert_eq!(pick(&logits, 10), 2);
        logits[10] = 6.0;
        assert_eq!(pick(&logits, 10), 10);
        assert!(repeating(&[1, 2, 1, 2, 1, 2, 1, 2]));
        assert!(!repeating(&[1, 2, 3, 4, 5, 6, 7, 8]));
    }
}

#[cfg(test)]
mod against_reference {
    /// `NIKGOB_MEL_IN=raw.f32 NIKGOB_MEL_OUT=out.f32 cargo test -p nikgob-align mel_dump -- --ignored`:
    /// the features of raw 16 kHz samples, to set against Hugging Face's.
    #[test]
    #[ignore = "a manual comparison"]
    fn mel_dump() {
        let raw = std::fs::read(std::env::var("NIKGOB_MEL_IN").unwrap()).unwrap();
        let samples: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        let mel = super::log_mel(&samples, &super::mel_filters());
        let out: Vec<u8> = mel.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(std::env::var("NIKGOB_MEL_OUT").unwrap(), out).unwrap();
    }
}
