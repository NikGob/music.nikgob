//! The voice out of the mix: an MDX-Net vocal model (UVR's Kim Vocal 2)
//! on the short-time spectrum of 44.1 kHz stereo, chunk by chunk, as UVR
//! runs it (the model takes the real and imaginary parts of both channels
//! up to `dim_f` bins and gives the voice's back).

use std::sync::Arc;

use realfft::{RealFftPlanner, num_complex::Complex32};

use crate::Error;
use crate::audio::Audio;
use crate::engine::Model;

/// The rate the model works at.
pub const RATE: u32 = 44_100;

#[derive(Clone, Copy, Debug)]
pub struct MdxSpec {
    pub n_fft: usize,
    pub hop: usize,
    /// Frequency bins the model sees.
    pub dim_f: usize,
    /// Frames of one chunk.
    pub dim_t: usize,
}

/// Kim Vocal 2.
pub const KIM_VOCAL_2: MdxSpec = MdxSpec { n_fft: 7680, hop: 1024, dim_f: 3072, dim_t: 256 };

/// The voice of `mix` (44.1 kHz; mono is doubled). `progress` gets the done
/// share after each chunk and stops the work when it returns false.
pub fn vocals(model: &mut Model, spec: MdxSpec, mix: Audio, progress: &mut dyn FnMut(f32) -> bool) -> Result<Audio, Error> {
    let mix = mix.stereo();
    let n = mix.frames();
    let chunk = spec.hop * (spec.dim_t - 1);
    let trim = spec.n_fft / 2;
    let step = chunk - 2 * trim;
    let pad = step - n % step;
    let rate = mix.rate;
    // The mix goes as it is padded: one copy of the song at a time.
    let padded: Vec<Vec<f32>> = mix
        .channels
        .into_iter()
        .map(|c| {
            let mut v = vec![0.0; trim];
            v.extend_from_slice(&c);
            v.resize(trim + n + pad + trim, 0.0);
            v
        })
        .collect();
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(spec.n_fft);
    let inv = planner.plan_fft_inverse(spec.n_fft);
    let window = hann(spec.n_fft);
    let bins = spec.n_fft / 2 + 1;
    let mut out: Vec<Vec<f32>> = vec![vec![0.0; n + pad]; 2];
    let chunks = (n + pad) / step;
    for k in 0..chunks {
        let at = k * step;
        let mut input = vec![0.0f32; 4 * spec.dim_f * spec.dim_t];
        for c in 0..2 {
            let wav = &padded[c][at..at + chunk];
            let spec_c = stft(wav, spec, &window, &fwd);
            for t in 0..spec.dim_t {
                for f in 0..spec.dim_f {
                    let z = spec_c[t * bins + f];
                    input[((2 * c) * spec.dim_f + f) * spec.dim_t + t] = z.re;
                    input[((2 * c + 1) * spec.dim_f + f) * spec.dim_t + t] = z.im;
                }
            }
        }
        let shape = [1usize, 4, spec.dim_f, spec.dim_t];
        let y = model.run_f32(shape.to_vec(), input)?;
        if y.len() != 4 * spec.dim_f * spec.dim_t {
            return Err(Error::Model(format!("separation gave {} values", y.len())));
        }
        for c in 0..2 {
            let mut s = vec![Complex32::new(0.0, 0.0); spec.dim_t * bins];
            for t in 0..spec.dim_t {
                for f in 0..spec.dim_f {
                    s[t * bins + f] = Complex32::new(
                        y[((2 * c) * spec.dim_f + f) * spec.dim_t + t],
                        y[((2 * c + 1) * spec.dim_f + f) * spec.dim_t + t],
                    );
                }
            }
            let wav = istft(&mut s, spec, &window, &inv, chunk);
            out[c][at..at + step].copy_from_slice(&wav[trim..trim + step]);
        }
        if !progress((k + 1) as f32 / chunks as f32) {
            return Err(Error::Stopped);
        }
    }
    for c in &mut out {
        c.truncate(n);
    }
    Ok(Audio { rate, channels: out })
}

/// Mel-Band RoFormer (Kim's vocal model, deep mode): the model gives a
/// complex mask over the short-time spectrum of an 8 s stereo chunk
/// (n_fft 2048, hop 441: 1025 bins × 801 frames, the planes L.re, L.im,
/// R.re, R.im), the voice is the mix's spectrum times it.
pub const ROFORMER: MdxSpec = MdxSpec { n_fft: 2048, hop: 441, dim_f: 1025, dim_t: 801 };
/// Samples of one chunk (8 s).
pub const ROFORMER_CHUNK: usize = 352_800;

/// The voice of `mix` (44.1 kHz; mono is doubled) by Mel-Band RoFormer, as
/// its training code runs it (`demix`): chunks every `step` samples (half
/// a chunk: each moment heard twice; a whole chunk: once, half the work),
/// faded in and out over a tenth of a chunk where they overlap and added
/// up, weighed by the fades. `progress` as for [`vocals`].
pub fn vocals_roformer(model: &mut Model, mix: Audio, step: usize, progress: &mut dyn FnMut(f32) -> bool) -> Result<Audio, Error> {
    let shape = vec![1, 4, ROFORMER.dim_f, ROFORMER.dim_t];
    roformer_with(&mut |x| model.run_f32(shape.clone(), x), mix, step, progress)
}

/// [`vocals_roformer`] with the model as a function from a chunk's
/// spectrum to its mask.
fn roformer_with(run: &mut dyn FnMut(Vec<f32>) -> Result<Vec<f32>, Error>, mix: Audio, step: usize, progress: &mut dyn FnMut(f32) -> bool) -> Result<Audio, Error> {
    let spec = ROFORMER;
    let chunk = ROFORMER_CHUNK;
    let step = step.clamp(1, chunk);
    let border = chunk - step;
    // Fades only where chunks overlap: chunks one after another meet
    // whole (faded, both would weigh nothing where they meet).
    let fade = (chunk / 10).min(border);
    let mix = mix.stereo();
    let rate = mix.rate;
    let n = mix.frames();
    // Overlapping chunks: the song is reflected at both ends so its first
    // and last moments are heard as often as the rest.
    let padded = border > 0 && n > 2 * border;
    let src: Vec<Vec<f32>> = mix.channels.into_iter().map(|c| if padded { reflect_pad(&c, border) } else { c }).collect();
    let len = src[0].len();
    let mut ramp = vec![1.0f32; chunk];
    for i in 0..fade {
        let f = i as f32 / (fade - 1).max(1) as f32;
        ramp[i] = f;
        ramp[chunk - 1 - i] = f;
    }
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(spec.n_fft);
    let inv = planner.plan_fft_inverse(spec.n_fft);
    let window = hann(spec.n_fft);
    let bins = spec.n_fft / 2 + 1;
    let plane = spec.dim_f * spec.dim_t;
    let mut out: Vec<Vec<f32>> = vec![vec![0.0; len]; 2];
    let mut weight = vec![0.0f32; len];
    let chunks = if len <= chunk { 1 } else { (len - chunk).div_ceil(step) + 1 };
    let mut at = 0;
    let mut k = 0;
    while at < len {
        let part = (len - at).min(chunk);
        let mut input = vec![0.0f32; 4 * plane];
        for (c, ch) in src.iter().enumerate() {
            let mut wav = ch[at..at + part].to_vec();
            if part < chunk {
                // The last chunk: reflected on when it is over half a chunk
                // long, else silence.
                wav = if part > chunk / 2 + 1 { reflect_pad_end(&wav, chunk) } else { [wav, vec![0.0; chunk - part]].concat() };
            }
            let s = stft(&wav, spec, &window, &fwd);
            for t in 0..spec.dim_t {
                for f in 0..spec.dim_f {
                    let z = s[t * bins + f];
                    input[(2 * c) * plane + f * spec.dim_t + t] = z.re;
                    input[(2 * c + 1) * plane + f * spec.dim_t + t] = z.im;
                }
            }
        }
        let mask = run(input.clone())?;
        if mask.iter().any(|v| !v.is_finite()) {
            return Err(Error::Model("separation gave values that are not numbers".into()));
        }
        if mask.len() != 4 * plane {
            return Err(Error::Model(format!("separation gave {} values", mask.len())));
        }
        let mut w = ramp.clone();
        if at == 0 {
            w[..fade].fill(1.0);
        }
        if at + chunk >= len {
            w[chunk - fade..].fill(1.0);
        }
        for c in 0..2 {
            let mut s = vec![Complex32::new(0.0, 0.0); spec.dim_t * bins];
            for t in 0..spec.dim_t {
                // The DC bin is left out of the voice (`zero_dc`).
                for f in 1..spec.dim_f {
                    let re = (2 * c) * plane + f * spec.dim_t + t;
                    let im = re + plane;
                    let (a, b) = (input[re], input[im]);
                    let (m, q) = (mask[re], mask[im]);
                    s[t * bins + f] = Complex32::new(a * m - b * q, a * q + b * m);
                }
            }
            let wav = istft(&mut s, spec, &window, &inv, chunk);
            for i in 0..part {
                out[c][at + i] += wav[i] * w[i];
            }
        }
        for i in 0..part {
            weight[at + i] += w[i];
        }
        k += 1;
        if !progress(k as f32 / chunks as f32) {
            return Err(Error::Stopped);
        }
        if at + chunk >= len {
            break;
        }
        at += step;
    }
    for c in &mut out {
        for (v, w) in c.iter_mut().zip(&weight) {
            *v = if *w > 1e-8 { *v / w } else { 0.0 };
        }
        if padded {
            c.drain(..border);
            c.truncate(n);
        }
    }
    Ok(Audio { rate, channels: out })
}

/// `x` with `pad` samples reflected onto both ends (as `torch`'s reflect
/// padding, never repeating the edge sample).
fn reflect_pad(x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len() as isize;
    let at = |i: isize| -> f32 {
        let mut i = i;
        if i < 0 {
            i = -i;
        }
        if i >= n {
            i = 2 * (n - 1) - i;
        }
        x[i.clamp(0, n - 1) as usize]
    };
    (-(pad as isize)..n + pad as isize).map(at).collect()
}

/// `x` reflected on at its end up to `len` samples.
fn reflect_pad_end(x: &[f32], len: usize) -> Vec<f32> {
    let n = x.len();
    let mut out = x.to_vec();
    // Called with less to add than `x` has (over half a chunk long).
    out.extend((0..len.saturating_sub(n)).map(|i| x[n.saturating_sub(2 + i).min(n - 1)]));
    out
}

fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect()
}

/// Centred STFT (reflect padding, as `torch.stft(center=True)`): `dim_t`
/// frames of `n_fft / 2 + 1` bins, frame-major.
fn stft(wav: &[f32], spec: MdxSpec, window: &[f32], fft: &Arc<dyn realfft::RealToComplex<f32>>) -> Vec<Complex32> {
    let half = spec.n_fft / 2;
    let len = wav.len() as isize;
    let at = |i: isize| -> f32 {
        // Reflect without repeating the edge sample.
        let mut i = i;
        if i < 0 {
            i = -i;
        }
        if i >= len {
            i = 2 * (len - 1) - i;
        }
        wav[i.clamp(0, len - 1) as usize]
    };
    let bins = half + 1;
    let mut out = vec![Complex32::new(0.0, 0.0); spec.dim_t * bins];
    let mut frame = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    for t in 0..spec.dim_t {
        let start = (t * spec.hop) as isize - half as isize;
        for (i, x) in frame.iter_mut().enumerate() {
            *x = at(start + i as isize) * window[i];
        }
        let _ = fft.process(&mut frame, &mut spectrum);
        out[t * bins..(t + 1) * bins].copy_from_slice(&spectrum);
    }
    out
}

/// Overlap-add back to `len` samples (the inverse of [`stft`]).
fn istft(s: &mut [Complex32], spec: MdxSpec, window: &[f32], ifft: &Arc<dyn realfft::ComplexToReal<f32>>, len: usize) -> Vec<f32> {
    let bins = spec.n_fft / 2 + 1;
    let half = spec.n_fft / 2;
    let total = len + spec.n_fft;
    let mut sig = vec![0.0f32; total];
    let mut norm = vec![0.0f32; total];
    let mut frame = ifft.make_output_vec();
    let scale = 1.0 / spec.n_fft as f32;
    for t in 0..spec.dim_t {
        let bin = &mut s[t * bins..(t + 1) * bins];
        // A real signal's spectrum has real DC and Nyquist bins.
        bin[0].im = 0.0;
        bin[bins - 1].im = 0.0;
        let _ = ifft.process(bin, &mut frame);
        let at = t * spec.hop;
        for i in 0..spec.n_fft {
            sig[at + i] += frame[i] * scale * window[i];
            norm[at + i] += window[i] * window[i];
        }
    }
    (0..len).map(|i| sig[i + half] / norm[i + half].max(1e-8)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stft_and_back_is_the_identity() {
        let spec = MdxSpec { n_fft: 64, hop: 16, dim_f: 32, dim_t: 17 };
        let len = spec.hop * (spec.dim_t - 1);
        let wav: Vec<f32> = (0..len).map(|i| (i as f32 * 0.37).sin() + 0.3 * (i as f32 * 0.05).cos()).collect();
        let mut planner = RealFftPlanner::<f32>::new();
        let (fwd, inv) = (planner.plan_fft_forward(64), planner.plan_fft_inverse(64));
        let w = hann(64);
        let mut s = stft(&wav, spec, &w, &fwd);
        let back = istft(&mut s, spec, &w, &inv, len);
        let err = wav.iter().zip(&back).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "{err}");
    }

    /// A mask that keeps everything gives the mix back, chunk seams and
    /// both ends included, whatever the step.
    #[test]
    fn a_whole_mask_gives_the_mix_back() {
        let n = ROFORMER_CHUNK * 2 + 12_345;
        let wav: Vec<f32> = (0..n).map(|i| (i as f32 * 0.31).sin() * 0.4 + (i as f32 * 0.21).sin() * 0.2).collect();
        let plane = ROFORMER.dim_f * ROFORMER.dim_t;
        for step in [ROFORMER_CHUNK / 2, ROFORMER_CHUNK] {
            let mix = Audio { rate: RATE, channels: vec![wav.clone(), wav.iter().map(|v| -v).collect()] };
            let mut calls = 0;
            let mut run = |x: Vec<f32>| {
                assert_eq!(x.len(), 4 * plane);
                calls += 1;
                Ok([vec![1.0; plane], vec![0.0; plane], vec![1.0; plane], vec![0.0; plane]].concat())
            };
            let out = roformer_with(&mut run, mix, step, &mut |_| true).unwrap();
            assert_eq!(out.channels[0].len(), n);
            let err = out.channels[0].iter().zip(&wav).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            // The DC bin left out costs a little where a frame does not hold whole cycles.
            assert!(err < 5e-3, "step {step}: {err}");
            assert!((out.channels[1][1000] + wav[1000]).abs() < 5e-3);
            assert!(calls >= n / step, "step {step}: {calls} chunks");
        }
    }

    #[test]
    fn reflecting_never_repeats_the_edge() {
        assert_eq!(reflect_pad(&[1.0, 2.0, 3.0, 4.0], 2), [3.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 2.0]);
        assert_eq!(reflect_pad_end(&[1.0, 2.0, 3.0, 4.0], 6), [1.0, 2.0, 3.0, 4.0, 3.0, 2.0]);
    }
}
