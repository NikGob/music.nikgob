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
}
