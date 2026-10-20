//! Sound analysis: bliss features from decoded PCM.
//!
//! bliss wants mono `f32` at 22 050 Hz. [`PcmAnalyzer`] takes interleaved
//! chunks at any rate the way a decoder hands them out, downmixes them,
//! resamples them with rubato's synchronous FFT resampler and keeps at most
//! the first [`MAX_SECONDS`] (bliss's statistics need no more, and long OST
//! mixes and disc images do not eat the CPU); [`PcmAnalyzer::finish`] runs
//! bliss. Only the 22 050 Hz mono copy is held, never the decoded original.

use std::ops::ControlFlow;

use bliss_audio::{NUMBER_FEATURES, Song};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

/// The rate bliss analyses at.
pub const SAMPLE_RATE: u32 = 22_050;
/// Only this much of a track is analysed.
pub const MAX_SECONDS: u32 = 8 * 60;
/// Shorter audio is not analysed (bliss's windows need more, and a jingle
/// says nothing about a sound).
const MIN_SECONDS: u32 = 1;
/// Features per vector (bliss features version 2).
pub const DIM: usize = NUMBER_FEATURES;

/// A track's bliss features, in bliss's order (tempo, zero-crossing rate,
/// spectral centroid / rolloff / flatness and loudness as mean and deviation,
/// then 13 chroma features).
pub type Vector = [f32; DIM];

/// `analysis.analyzed_with` of what this build computes: the bliss release
/// and its features version. A stored vector with another tag is computed
/// again in the background.
pub const ANALYZED_WITH: &str = "bliss-0.13:2";

const CHUNK_FRAMES: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("unsupported pcm format: {0}")]
    Format(String),
    #[error("too short to analyse")]
    TooShort,
    #[error("resampler: {0}")]
    Resample(String),
    #[error("bliss: {0}")]
    Bliss(String),
}

fn resample_err(e: impl std::fmt::Display) -> AnalysisError {
    AnalysisError::Resample(e.to_string())
}

/// Mono FFT resampling to [`SAMPLE_RATE`], trimming the filter delay at the
/// start and flushing the tail at the end (as `nikgob-audio`'s stream
/// resampler does).
struct ToBliss {
    rs: Fft<f32>,
    rate_in: u32,
    /// Mono input not yet resampled; `in_pos` is how much of it is consumed.
    inbuf: Vec<f32>,
    in_pos: usize,
    outbuf: Vec<f32>,
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl ToBliss {
    fn new(rate_in: u32) -> Result<Self, AnalysisError> {
        let rs = Fft::<f32>::new(rate_in as usize, SAMPLE_RATE as usize, CHUNK_FRAMES, 1, FixedSync::Input)
            .map_err(resample_err)?;
        let outbuf = vec![0.0; rs.output_frames_max()];
        let delay_left = rs.output_delay();
        Ok(Self {
            rs,
            rate_in,
            inbuf: Vec::with_capacity(CHUNK_FRAMES * 2),
            in_pos: 0,
            outbuf,
            delay_left,
            in_total: 0,
            out_total: 0,
        })
    }

    fn process(&mut self, out: &mut Vec<f32>) -> Result<(), AnalysisError> {
        loop {
            let need = self.rs.input_frames_next();
            if self.inbuf.len() - self.in_pos < need {
                break;
            }
            let out_frames = self.rs.output_frames_next();
            let (used, produced) = {
                let src = InterleavedSlice::new(&self.inbuf[self.in_pos..], 1, need).map_err(resample_err)?;
                let mut dst = InterleavedSlice::new_mut(&mut self.outbuf, 1, out_frames).map_err(resample_err)?;
                self.rs.process_into_buffer(&src, &mut dst, None).map_err(resample_err)?
            };
            self.in_pos += used;
            self.in_total += used as u64;
            self.emit(produced, u64::MAX, out);
        }
        if self.in_pos > 0 && self.in_pos >= self.inbuf.len() / 2 {
            self.inbuf.drain(..self.in_pos);
            self.in_pos = 0;
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<f32>) -> Result<(), AnalysisError> {
        let remaining = self.inbuf.len() - self.in_pos;
        self.in_total += remaining as u64;
        let expected = (u128::from(self.in_total) * u128::from(SAMPLE_RATE)).div_ceil(u128::from(self.rate_in)) as u64;
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
                self.inbuf.resize(self.in_pos + need, 0.0);
                let src = InterleavedSlice::new(&self.inbuf[self.in_pos..], 1, need).map_err(resample_err)?;
                let mut dst = InterleavedSlice::new_mut(&mut self.outbuf, 1, out_frames).map_err(resample_err)?;
                self.rs.process_into_buffer(&src, &mut dst, Some(&idx)).map_err(resample_err)?.1
            };
            partial = Some(0);
            self.emit(produced, expected, out);
        }
        Ok(())
    }

    fn emit(&mut self, frames: usize, limit: u64, out: &mut Vec<f32>) {
        let skip = self.delay_left.min(frames);
        self.delay_left -= skip;
        let take = (frames - skip).min(limit.saturating_sub(self.out_total) as usize);
        out.extend_from_slice(&self.outbuf[skip..skip + take]);
        self.out_total += take as u64;
    }
}

/// Streaming front end of the analysis: feed it what the decoder produces,
/// then [`Self::finish`].
pub struct PcmAnalyzer {
    channels: usize,
    /// Input frames still taken before the [`MAX_SECONDS`] cap.
    frames_left: u64,
    /// `None` when the input already is at [`SAMPLE_RATE`].
    resampler: Option<ToBliss>,
    /// The 22 050 Hz mono signal bliss gets.
    mono: Vec<f32>,
    /// The first resampler failure (reported by `finish`).
    error: Option<AnalysisError>,
}

impl PcmAnalyzer {
    pub fn new(sample_rate: u32, channels: usize) -> Result<Self, AnalysisError> {
        if sample_rate == 0 || channels == 0 {
            return Err(AnalysisError::Format(format!("{sample_rate} Hz, {channels} channels")));
        }
        let frames_left = u64::from(MAX_SECONDS) * u64::from(sample_rate);
        let resampler = (sample_rate != SAMPLE_RATE).then(|| ToBliss::new(sample_rate)).transpose()?;
        let capacity = (u64::from(MAX_SECONDS) * u64::from(SAMPLE_RATE)).min(1 << 22) as usize;
        Ok(Self {
            channels,
            frames_left,
            resampler,
            mono: Vec::with_capacity(capacity),
            error: None,
        })
    }

    /// Takes interleaved frames. Breaks once the cap is reached (or the
    /// resampler failed): the rest of the track is not needed.
    pub fn push(&mut self, interleaved: &[f32]) -> ControlFlow<()> {
        if self.is_done() {
            return ControlFlow::Break(());
        }
        let ch = self.channels;
        let frames = (interleaved.len() / ch).min(self.frames_left as usize);
        self.frames_left -= frames as u64;
        let mono = interleaved[..frames * ch].chunks_exact(ch).map(|f| {
            if ch == 1 {
                f[0]
            } else {
                f.iter().sum::<f32>() / ch as f32
            }
        });
        match &mut self.resampler {
            None => self.mono.extend(mono),
            Some(rs) => {
                rs.inbuf.extend(mono);
                if let Err(e) = rs.process(&mut self.mono) {
                    self.error = Some(e);
                }
            }
        }
        if self.is_done() { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }

    fn is_done(&self) -> bool {
        self.frames_left == 0 || self.error.is_some()
    }

    /// Seconds of audio taken so far.
    pub fn seconds(&self) -> f64 {
        let pending = self.resampler.as_ref().map_or(0, |r| r.inbuf.len() - r.in_pos);
        self.mono.len() as f64 / f64::from(SAMPLE_RATE)
            + self.resampler.as_ref().map_or(0.0, |r| pending as f64 / f64::from(r.rate_in))
    }

    /// Runs bliss over what was pushed.
    pub fn finish(mut self) -> Result<Vector, AnalysisError> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        if let Some(rs) = &mut self.resampler {
            rs.finish(&mut self.mono)?;
        }
        if self.mono.len() < (MIN_SECONDS * SAMPLE_RATE) as usize {
            return Err(AnalysisError::TooShort);
        }
        let features = Song::analyze(&self.mono).map_err(|e| AnalysisError::Bliss(e.to_string()))?.as_vec();
        let vector: Vector = features
            .as_slice()
            .try_into()
            .map_err(|_| AnalysisError::Bliss(format!("{} features instead of {DIM}", features.len())))?;
        if vector.iter().any(|v| !v.is_finite()) {
            return Err(AnalysisError::Bliss("a feature is not a number".into()));
        }
        Ok(vector)
    }
}

/// Analyses a whole interleaved buffer: downmix, resample to 22 050 Hz,
/// the first 8 minutes, bliss.
pub fn analyze_pcm(samples: &[f32], sample_rate: u32, channels: usize) -> Result<Vector, AnalysisError> {
    let mut a = PcmAnalyzer::new(sample_rate, channels)?;
    for chunk in samples.chunks(CHUNK_FRAMES * 8 * channels) {
        if a.push(chunk).is_break() {
            break;
        }
    }
    a.finish()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small deterministic generator for test signals.
    pub(crate) struct Rng(pub u64);

    impl Rng {
        pub(crate) fn next_f32(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
        }
    }

    /// Music-like mono signal: decaying notes with harmonics, `bpm` notes a
    /// minute, `bright` adds high harmonics and noise.
    pub(crate) fn music(rate: u32, secs: f64, bpm: f64, bright: f32, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed.max(1));
        let n = (f64::from(rate) * secs) as usize;
        let mut out = vec![0f32; n];
        let note_len = (f64::from(rate) * 60.0 / bpm) as usize;
        let mut start = 0;
        while start < n {
            let base = 110.0 * 2f64.powf(f64::from((rng.next_f32().abs() * 24.0) as i32) / 12.0);
            for i in 0..note_len.min(n - start) {
                let t = i as f64 / f64::from(rate);
                let env = (-4.0 * t).exp() * (1.0 - (-300.0 * t).exp());
                let mut s = 0.0;
                for h in 1..=8 {
                    let weight = if h > 3 { f64::from(bright) } else { 1.0 };
                    s += weight * (std::f64::consts::TAU * base * f64::from(h) * t).sin() / f64::from(h);
                }
                out[start + i] += (s * env * 0.25) as f32 + rng.next_f32() * 0.02 * bright;
            }
            start += note_len;
        }
        out
    }

    fn stereo(mono: &[f32]) -> Vec<f32> {
        mono.iter().flat_map(|s| [*s, *s * 0.8]).collect()
    }

    #[test]
    fn analyzed_with_names_bliss_features_version() {
        assert_eq!(
            ANALYZED_WITH,
            format!("bliss-0.13:{}", bliss_audio::FeaturesVersion::LATEST as u16)
        );
        assert_eq!(DIM, 23);
    }

    #[test]
    fn stereo_analyses_like_its_mono_and_other_music_is_further() {
        let mono = music(44_100, 20.0, 120.0, 0.5, 7);
        let a = analyze_pcm(&stereo(&mono), 44_100, 2).unwrap();
        let mono_only = {
            let mut p = PcmAnalyzer::new(44_100, 1).unwrap();
            let _ = p.push(&mono);
            p.finish().unwrap()
        };
        let dist: f32 = a.iter().zip(&mono_only).map(|(x, y)| (x - y).powi(2)).sum::<f32>().sqrt();
        assert!(dist < 0.3, "downmix changes little: {dist}");
        assert!(a.iter().all(|v| v.is_finite()));
        // Something else sounds different.
        let other = analyze_pcm(&music(44_100, 20.0, 200.0, 2.0, 99), 44_100, 1).unwrap();
        let far: f32 = a.iter().zip(&other).map(|(x, y)| (x - y).powi(2)).sum::<f32>().sqrt();
        assert!(far > dist * 2.0, "different music is further: {far} vs {dist}");
    }

    #[test]
    fn chunked_pushes_match_one_buffer() {
        let mono = music(48_000, 12.0, 100.0, 1.0, 3);
        let whole = analyze_pcm(&mono, 48_000, 1).unwrap();
        let mut p = PcmAnalyzer::new(48_000, 1).unwrap();
        for c in mono.chunks(777) {
            let _ = p.push(c);
        }
        let chunked = p.finish().unwrap();
        for (a, b) in whole.iter().zip(&chunked) {
            assert!((a - b).abs() < 1e-3, "{whole:?} vs {chunked:?}");
        }
    }

    #[test]
    fn only_the_first_minutes_are_taken() {
        let rate = 8_000;
        let mut p = PcmAnalyzer::new(rate, 2).unwrap();
        let second = vec![0.1f32; rate as usize * 2];
        let mut pushed = 0;
        while p.push(&second).is_continue() {
            pushed += 1;
            assert!(pushed <= MAX_SECONDS, "the cap stops the decode");
        }
        assert_eq!(pushed + 1, MAX_SECONDS);
        assert!((p.seconds() - f64::from(MAX_SECONDS)).abs() < 0.5, "{}", p.seconds());
    }

    #[test]
    fn rejects_short_and_malformed_input() {
        assert!(matches!(analyze_pcm(&[0.0; 4000], 44_100, 1), Err(AnalysisError::TooShort)));
        assert!(matches!(PcmAnalyzer::new(0, 2), Err(AnalysisError::Format(_))));
        assert!(matches!(PcmAnalyzer::new(44_100, 0), Err(AnalysisError::Format(_))));
    }
}
