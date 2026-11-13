//! GigaAM-v3 CTC, a second Russian aligner for deep mode: a Conformer that hears 64 log-mel
//! bands of 16 kHz audio (torchaudio's `MelSpectrogram` as GigaAM's
//! `FeatureExtractor` sets it: n_fft 320, hop 160, no centring, HTK mel
//! scale, no band normalisation) and gives the 33 Russian letters without
//! «ё», a word separator and the blank, one frame per 40 ms.

use std::sync::Arc;

use ort::value::Tensor;
use realfft::RealFftPlanner;

use crate::Error;
use crate::ctc::Emissions;
use crate::engine::Model;

const RATE: usize = 16_000;
const N_FFT: usize = 320;
const HOP: usize = 160;
pub const MELS: usize = 64;
/// Frames a second of the output (four mel frames each).
pub const FPS: f32 = 25.0;
/// Windows of audio heard at once, with context on both sides (seconds),
/// as the wav2vec2 aligners are.
const WINDOW_S: usize = 30;
const CONTEXT_S: usize = 2;
/// Each window is brought to this RMS (speech at about −26 dBFS), never
/// raised by more than the song's own level allows (as
/// `pipeline::emissions` does: a silent tail raised is noise).
const TARGET_RMS: f32 = 0.05;
const QUIET: f32 = 0.1;

/// The model's letters as the aligners' `vocab.json` has them: its word
/// separator `▁` as `|`, its blank `<blk>` as `<blank>`.
pub fn vocab_json(txt: &str) -> String {
    let mut map = serde_json::Map::new();
    for line in txt.lines() {
        let mut parts = line.rsplitn(2, ' ');
        let (Some(id), Some(tok)) = (parts.next(), parts.next()) else { continue };
        let Ok(id) = id.trim().parse::<i64>() else { continue };
        let tok = match tok {
            "▁" | " " => "|",
            "<blk>" => "<blank>",
            t => t,
        };
        map.insert(tok.to_owned(), id.into());
    }
    serde_json::Value::Object(map).to_string()
}

/// HTK mel filters, `bins × MELS` (torchaudio's `melscale_fbanks`).
fn mel_filters() -> Vec<f32> {
    let bins = N_FFT / 2 + 1;
    let mel = |f: f64| 2595.0 * (1.0 + f / 700.0).log10();
    let hz = |m: f64| 700.0 * (10f64.powf(m / 2595.0) - 1.0);
    let top = mel(RATE as f64 / 2.0);
    let pts: Vec<f64> = (0..MELS + 2).map(|k| hz(top * k as f64 / (MELS + 1) as f64)).collect();
    let mut fb = vec![0.0f32; bins * MELS];
    for i in 0..bins {
        let f = i as f64 * (RATE as f64 / 2.0) / (bins - 1) as f64;
        for j in 0..MELS {
            let down = (f - pts[j]) / (pts[j + 1] - pts[j]);
            let up = (pts[j + 2] - f) / (pts[j + 2] - pts[j + 1]);
            fb[i * MELS + j] = down.min(up).max(0.0) as f32;
        }
    }
    fb
}

/// Log-mel features of `x` (16 kHz): `MELS × frames`, band-major, and the
/// number of frames.
pub fn features(x: &[f32]) -> (Vec<f32>, usize) {
    if x.len() < N_FFT {
        return (Vec::new(), 0);
    }
    let frames = (x.len() - N_FFT) / HOP + 1;
    let bins = N_FFT / 2 + 1;
    let fb = mel_filters();
    // A periodic Hann window, as `torch.hann_window`.
    let window: Vec<f32> = (0..N_FFT).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N_FFT as f32).cos()).collect();
    let fft: Arc<dyn realfft::RealToComplex<f32>> = RealFftPlanner::<f32>::new().plan_fft_forward(N_FFT);
    let mut frame = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut power = vec![0.0f32; bins];
    let mut out = vec![0.0f32; MELS * frames];
    for t in 0..frames {
        let at = t * HOP;
        for (i, v) in frame.iter_mut().enumerate() {
            *v = x[at + i] * window[i];
        }
        let _ = fft.process(&mut frame, &mut spectrum);
        for (p, z) in power.iter_mut().zip(&spectrum) {
            *p = z.norm_sqr();
        }
        for j in 0..MELS {
            let e: f32 = (0..bins).map(|i| power[i] * fb[i * MELS + j]).sum();
            out[j * frames + t] = e.clamp(1e-9, 1e9).ln();
        }
    }
    (out, frames)
}

/// What the model hears in `voice` (16 kHz) over the whole song, 25 frames
/// a second, log-probabilities; `blank`: its blank's id.
pub fn emissions(model: &mut Model, voice: &[f32], blank: usize, progress: &mut dyn FnMut(f32) -> bool) -> Result<Emissions, Error> {
    let (win, ctx) = (WINDOW_S * RATE, CONTEXT_S * RATE);
    let n = voice.len();
    let level = (voice.iter().map(|v| v * v).sum::<f32>() / n.max(1) as f32).sqrt();
    let mut logp: Vec<f32> = Vec::new();
    let mut vocab = 0;
    let mut start = 0;
    while start < n {
        let (a, b) = (start.saturating_sub(ctx), (start + win + ctx).min(n));
        let mut x = voice[a..b].to_vec();
        if x.len() < N_FFT {
            x.resize(N_FFT, 0.0);
        }
        let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
        let gain = TARGET_RMS / rms.max(level * QUIET).max(1e-6);
        for v in &mut x {
            *v *= gain;
        }
        let (feats, frames) = features(&x);
        let input = Tensor::from_array((vec![1usize, MELS, frames], feats)).map_err(|e| Error::Model(e.to_string()))?;
        let lengths = Tensor::from_array((vec![1usize], vec![frames as i64])).map_err(|e| Error::Model(e.to_string()))?;
        let out = model.session().run(ort::inputs![input, lengths]).map_err(|e| Error::Model(e.to_string()))?;
        let (shape, data) = out[0].try_extract_tensor::<f32>().map_err(|e| Error::Model(e.to_string()))?;
        // `[1, frames, letters]`, or letters first.
        let (t_len, v, letters_first) = match shape[..] {
            [1, t, v] if v <= 64 => (t as usize, v as usize, false),
            [1, v, t] if v <= 64 => (t as usize, v as usize, true),
            _ => return Err(Error::Model(format!("GigaAM output shape {shape:?}"))),
        };
        let mut rows: Vec<f32> = if letters_first { (0..t_len).flat_map(|t| (0..v).map(move |k| data[k * t_len + t])).collect() } else { data.to_vec() };
        drop(out);
        vocab = v;
        Emissions::log_softmax(&mut rows, v);
        let s = ((((start - a) as f32) / RATE as f32) * FPS).round() as usize;
        let e = (s + ((win.min(n - start)) as f32 / RATE as f32 * FPS).round() as usize).min(t_len);
        logp.extend_from_slice(&rows[s.min(e) * v..e * v]);
        start += win;
        if !progress(start.min(n) as f32 / n as f32) {
            return Err(Error::Stopped);
        }
    }
    Ok(Emissions { logp, vocab, fps: FPS, blank })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Letters, Vocab};

    #[test]
    fn its_letters_read_as_an_aligners() {
        let mut txt = String::from("▁ 0\n");
        for (i, c) in ('а'..='я').enumerate() {
            txt.push_str(&format!("{c} {}\n", i + 1));
        }
        txt.push_str("<blk> 33\n");
        let v = Vocab::from_json(&vocab_json(&txt), Letters::Native).unwrap();
        assert_eq!((v.blank, v.sep), (33, Some(0)));
        // «ё», which the model lacks, is heard as «е».
        assert_eq!(v.tokens("ёж"), [6, 7]);
        assert_eq!(v.tokens("мир"), [13, 9, 17]);
    }

    /// The mel filters cover the band evenly on the mel scale, and a tone
    /// lights the band of its pitch.
    #[test]
    fn a_tone_is_heard_in_its_band() {
        let fb = mel_filters();
        let bins = N_FFT / 2 + 1;
        // Every band has some bin; no bin past the top is lost.
        for j in 0..MELS {
            assert!((0..bins).any(|i| fb[i * MELS + j] > 0.0), "band {j} is empty");
        }
        let tone: Vec<f32> = (0..RATE).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / RATE as f32).sin() * 0.1).collect();
        let (f, frames) = features(&tone);
        assert_eq!(frames, (RATE - N_FFT) / HOP + 1);
        let t = frames / 2;
        let loudest = (0..MELS).max_by(|a, b| f[a * frames + t].total_cmp(&f[b * frames + t])).unwrap();
        // 1 kHz sits about a third of the way up the mel scale to 8 kHz.
        let mel = |hz: f32| 2595.0 * (1.0 + hz / 700.0).log10();
        let expect = (mel(1000.0) / mel(8000.0) * (MELS + 1) as f32).round() as usize - 1;
        assert!(loudest.abs_diff(expect) <= 1, "{loudest} vs {expect}");
        // Silence is clamped, not minus infinity.
        let (s, _) = features(&vec![0.0; 1000]);
        assert!(s.iter().all(|v| (*v - 1e-9f32.ln()).abs() < 1e-3));
    }
}
