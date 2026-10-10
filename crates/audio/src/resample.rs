//! Streaming sample-rate conversion on top of rubato's synchronous FFT
//! resampler (fixed ratio, highest quality), trimming the filter delay at the
//! start and flushing the tail at the end so the output length is exactly
//! `ceil(input * ratio)` and nothing is shifted in time.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::types::DecodeError;

const CHUNK_FRAMES: usize = 1024;

pub(crate) struct StreamResampler {
    rs: Fft<f32>,
    channels: usize,
    rate_in: u32,
    rate_out: u32,
    /// Pending interleaved input; `in_pos` is how much of it is consumed.
    inbuf: Vec<f32>,
    in_pos: usize,
    outbuf: Vec<f32>,
    delay_left: usize,
    in_total: u64,
    out_total: u64,
    finished: bool,
}

fn err(e: impl std::fmt::Display) -> DecodeError {
    DecodeError::Format(format!("resampler: {e}"))
}

impl StreamResampler {
    pub(crate) fn new(rate_in: u32, rate_out: u32, channels: usize) -> Result<Self, DecodeError> {
        let rs = Fft::<f32>::new(
            rate_in as usize,
            rate_out as usize,
            CHUNK_FRAMES,
            channels,
            FixedSync::Input,
        )
        .map_err(err)?;
        let outbuf = vec![0.0; rs.output_frames_max() * channels];
        let delay_left = rs.output_delay();
        Ok(Self {
            rate_in,
            rate_out,
            rs,
            channels,
            inbuf: Vec::with_capacity(CHUNK_FRAMES * channels * 2),
            in_pos: 0,
            outbuf,
            delay_left,
            in_total: 0,
            out_total: 0,
            finished: false,
        })
    }

    /// Feeds interleaved frames; appends whatever output is ready to `out`.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), DecodeError> {
        debug_assert!(!self.finished);
        self.inbuf.extend_from_slice(input);
        let ch = self.channels;
        loop {
            let need = self.rs.input_frames_next();
            if (self.inbuf.len() - self.in_pos) / ch < need {
                break;
            }
            let out_frames = self.rs.output_frames_next();
            let (used, produced) = {
                let src =
                    InterleavedSlice::new(&self.inbuf[self.in_pos..], ch, need).map_err(err)?;
                let mut dst =
                    InterleavedSlice::new_mut(&mut self.outbuf, ch, out_frames).map_err(err)?;
                self.rs
                    .process_into_buffer(&src, &mut dst, None)
                    .map_err(err)?
            };
            self.in_pos += used * ch;
            self.in_total += used as u64;
            self.emit(produced, u64::MAX, out);
        }
        if self.in_pos > 0 && self.in_pos >= self.inbuf.len() / 2 {
            self.inbuf.drain(..self.in_pos);
            self.in_pos = 0;
        }
        Ok(())
    }

    /// Flushes the filter tail. Call once, after the last `process`.
    pub(crate) fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), DecodeError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let ch = self.channels;
        let remaining = (self.inbuf.len() - self.in_pos) / ch;
        self.in_total += remaining as u64;
        let expected = (u128::from(self.in_total) * u128::from(self.rate_out))
            .div_ceil(u128::from(self.rate_in)) as u64;
        let mut partial = Some(remaining);
        let mut guard = 0;
        while self.out_total < expected && guard < 64 {
            guard += 1;
            let need = self.rs.input_frames_next();
            let out_frames = self.rs.output_frames_next();
            let idx = Indexing {
                input_offset: 0,
                output_offset: 0,
                partial_len: partial,
                active_channels_mask: None,
            };
            let produced = {
                // Always hand rubato a buffer at least `need` frames long.
                self.inbuf.resize(self.in_pos + need * ch, 0.0);
                let src =
                    InterleavedSlice::new(&self.inbuf[self.in_pos..], ch, need).map_err(err)?;
                let mut dst =
                    InterleavedSlice::new_mut(&mut self.outbuf, ch, out_frames).map_err(err)?;
                self.rs
                    .process_into_buffer(&src, &mut dst, Some(&idx))
                    .map_err(err)?
                    .1
            };
            partial = Some(0);
            self.emit(produced, expected, out);
        }
        Ok(())
    }

    /// Appends `frames` frames from `outbuf`, after dropping the start delay
    /// and without exceeding `limit` total output frames.
    fn emit(&mut self, frames: usize, limit: u64, out: &mut Vec<f32>) {
        let ch = self.channels;
        let skip = self.delay_left.min(frames);
        self.delay_left -= skip;
        let mut take = frames - skip;
        take = take.min(limit.saturating_sub(self.out_total) as usize);
        out.extend_from_slice(&self.outbuf[skip * ch..(skip + take) * ch]);
        self.out_total += take as u64;
    }

    /// Output frames produced so far.
    #[cfg(test)]
    pub(crate) fn out_total(&self) -> u64 {
        self.out_total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f64, rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| (std::f64::consts::TAU * freq * i as f64 / f64::from(rate)).sin() as f32 * 0.5)
            .collect()
    }

    /// Frequency by counting rising zero crossings, with sub-sample interpolation.
    fn measure_freq(samples: &[f32], rate: u32) -> f64 {
        let mut first = None;
        let mut last = 0.0;
        let mut count = 0usize;
        for i in 1..samples.len() {
            if samples[i - 1] < 0.0 && samples[i] >= 0.0 {
                let frac = f64::from(-samples[i - 1]) / f64::from(samples[i] - samples[i - 1]);
                let t = (i - 1) as f64 + frac;
                if first.is_none() {
                    first = Some(t);
                } else {
                    count += 1;
                }
                last = t;
            }
        }
        let first = first.expect("no zero crossings");
        count as f64 * f64::from(rate) / (last - first)
    }

    fn run(rate_in: u32, rate_out: u32, frames: usize, block: usize) -> Vec<f32> {
        let input = sine(1000.0, rate_in, frames);
        let mut rs = StreamResampler::new(rate_in, rate_out, 1).unwrap();
        let mut out = Vec::new();
        for chunk in input.chunks(block) {
            rs.process(chunk, &mut out).unwrap();
        }
        rs.finish(&mut out).unwrap();
        assert_eq!(rs.out_total() as usize, out.len());
        out
    }

    #[test]
    fn one_khz_stays_one_khz_and_length_is_exact() {
        for (a, b) in [
            (44_100, 48_000),
            (48_000, 44_100),
            (96_000, 48_000),
            (22_050, 48_000),
        ] {
            let out = run(a, b, a as usize * 2, 777);
            let expected = (a as usize * 2 * b as usize).div_ceil(a as usize);
            assert_eq!(out.len(), expected, "{a}->{b}");
            let f = measure_freq(&out[2000..out.len() - 2000], b);
            assert!((f - 1000.0).abs() < 0.5, "{a}->{b}: {f}");
        }
    }

    #[test]
    fn no_time_shift_start_and_tail_are_aligned() {
        // A sine starting at phase zero must still start at (almost) zero and
        // carry the same phase after resampling.
        let out = run(44_100, 48_000, 44_100, 1024);
        assert!(out[0].abs() < 0.02, "{}", out[0]);
        // Compare against an ideal 48 kHz sine.
        let ideal = sine(1000.0, 48_000, out.len());
        let n = out.len();
        let err: f32 = (4000..n - 4000)
            .map(|i| (out[i] - ideal[i]).abs())
            .fold(0.0, f32::max);
        assert!(err < 0.005, "max deviation {err}");
        // The tail is not truncated early: last samples still carry signal.
        let tail_energy: f32 = out[n - 200..].iter().map(|s| s * s).sum();
        assert!(tail_energy > 1.0, "{tail_energy}");
    }
}
