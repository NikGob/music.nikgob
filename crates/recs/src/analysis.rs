//! Sound analysis: bliss features (and the sound model's vector) from
//! decoded PCM.
//!
//! bliss wants mono `f32` at 22 050 Hz. The background analysis decodes
//! only a few windows of a track ([`windows`]: 3 × 15 s at 20, 45 and 70 %
//! of its length) into a [`WindowedAnalyzer`], which downmixes them,
//! resamples each with rubato's synchronous FFT resampler, gives bliss the
//! windows together and the sound model ([`crate::embedding`]) each window
//! on its own: the peak of an 8-minute track is a few MB instead of the
//! whole decode. [`PcmAnalyzer`] is the older whole-track pass (the first
//! [`MAX_SECONDS`]), kept for buffers in memory and as the reference the
//! windows are measured against.
//!
//! bliss computes its descriptors on five threads of its own, at normal
//! priority whatever the caller's is; one feature pass runs at a time in the
//! process, so analysis never takes more of the CPU than that from playback.

use std::ops::ControlFlow;
use std::sync::Mutex;

use bliss_audio::{NUMBER_FEATURES, Song};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::embedding::{EmbedSum, SoundModel};

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

/// Held while bliss computes the features of one track.
static BLISS: Mutex<()> = Mutex::new(());

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
    #[error("sound model: {0}")]
    Model(String),
}

fn resample_err(e: impl std::fmt::Display) -> AnalysisError {
    AnalysisError::Resample(e.to_string())
}

/// Mono FFT resampling to `rate_out` ([`SAMPLE_RATE`] for bliss), trimming
/// the filter delay at the start and flushing the tail at the end (as
/// `nikgob-audio`'s stream resampler does).
struct ToRate {
    rs: Fft<f32>,
    rate_in: u32,
    rate_out: u32,
    /// Mono input not yet resampled; `in_pos` is how much of it is consumed.
    inbuf: Vec<f32>,
    in_pos: usize,
    outbuf: Vec<f32>,
    delay_left: usize,
    in_total: u64,
    out_total: u64,
}

impl ToRate {
    fn new(rate_in: u32, rate_out: u32) -> Result<Self, AnalysisError> {
        let rs = Fft::<f32>::new(rate_in as usize, rate_out as usize, CHUNK_FRAMES, 1, FixedSync::Input)
            .map_err(resample_err)?;
        let outbuf = vec![0.0; rs.output_frames_max()];
        let delay_left = rs.output_delay();
        Ok(Self {
            rs,
            rate_in,
            rate_out,
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
        let expected = (u128::from(self.in_total) * u128::from(self.rate_out)).div_ceil(u128::from(self.rate_in)) as u64;
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
    resampler: Option<ToRate>,
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
        let resampler = (sample_rate != SAMPLE_RATE).then(|| ToRate::new(sample_rate, SAMPLE_RATE)).transpose()?;
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
        bliss_of(&self.mono)
    }
}

/// bliss's features of 22 050 Hz mono audio (one feature pass at a time in
/// the process).
fn bliss_of(mono: &[f32]) -> Result<Vector, AnalysisError> {
    if mono.len() < (MIN_SECONDS * SAMPLE_RATE) as usize {
        return Err(AnalysisError::TooShort);
    }
    let features = {
        let _one_at_a_time = BLISS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Song::analyze(mono).map_err(|e| AnalysisError::Bliss(e.to_string()))?.as_vec()
    };
    let vector: Vector = features
        .as_slice()
        .try_into()
        .map_err(|_| AnalysisError::Bliss(format!("{} features instead of {DIM}", features.len())))?;
    if vector.iter().any(|v| !v.is_finite()) {
        return Err(AnalysisError::Bliss("a feature is not a number".into()));
    }
    Ok(vector)
}

/// Seconds of one analysis window.
pub const WINDOW_SECONDS: f64 = 15.0;
/// Where the windows of a track sit, as shares of its length.
pub const WINDOW_AT: [f64; 3] = [0.2, 0.45, 0.7];
/// A window never takes more than this, whatever the decoder hands over.
const MAX_WINDOW_SECONDS: u64 = 60;
/// Where a preview's window starts (the head of a stream: past the intro)…
pub const PREVIEW_START: f64 = 15.0;
/// …and how long it is.
pub const PREVIEW_SECONDS: f64 = 30.0;

/// A stretch of a track to decode and analyse, in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub start: f64,
    pub len: f64,
}

/// The windows of a track `duration` seconds long (docs/recommendations.md
/// §5.1.2): three of [`WINDOW_SECONDS`] at [`WINDOW_AT`] of its length,
/// pushed apart so they never overlap. A track shorter than the three gets
/// one window of [`WINDOW_SECONDS`] around 45 % of it (or all of it when
/// shorter still). A track of unknown length (a container that does not
/// say) gets three back to back from its start: with no length to place
/// them by, the head is the only part known to be there, and it is the
/// same 45 s of audio and three vectors of the model as any other track.
pub fn windows(duration: Option<f64>) -> Vec<Window> {
    let n = WINDOW_AT.len();
    let total = WINDOW_SECONDS * n as f64;
    match duration {
        Some(d) if d.is_finite() && d >= total => {
            let mut out = Vec::with_capacity(n);
            let mut lo = 0.0f64;
            for (i, at) in WINDOW_AT.iter().enumerate() {
                let hi = d - WINDOW_SECONDS * (n - i) as f64;
                let start = (at * d).min(hi).max(lo);
                out.push(Window { start, len: WINDOW_SECONDS });
                lo = start + WINDOW_SECONDS;
            }
            out
        }
        Some(d) if d.is_finite() && d > 0.0 => {
            let start = (WINDOW_AT[1] * d - WINDOW_SECONDS / 2.0).min(d - WINDOW_SECONDS).max(0.0);
            vec![Window { start, len: WINDOW_SECONDS }]
        }
        _ => (0..n).map(|i| Window { start: WINDOW_SECONDS * i as f64, len: WINDOW_SECONDS }).collect(),
    }
}

/// The window of a preview of new music (the head of a stream, its first
/// minute or so): [`PREVIEW_SECONDS`] from [`PREVIEW_START`], or from the
/// start of a song too short for that.
pub fn preview_windows(duration: Option<f64>) -> Vec<Window> {
    let start = if duration.is_some_and(|d| d < PREVIEW_START + PREVIEW_SECONDS) { 0.0 } else { PREVIEW_START };
    vec![Window { start, len: PREVIEW_SECONDS }]
}

/// What a windowed pass gives.
#[derive(Debug)]
pub struct Analysed {
    /// bliss's vector of the windows together; `None` when not asked.
    pub bliss: Option<Result<Vector, AnalysisError>>,
    /// The sound model's raw vector, the mean of the windows'; `None` when
    /// there is no model.
    pub embedding: Option<Result<Vec<f32>, AnalysisError>>,
    /// Seconds of audio taken.
    pub seconds: f64,
}

/// The analysis of a few windows of a track (docs/recommendations.md
/// §5.1.2): feed it each window's decoded PCM ([`Self::push`] with the
/// window's number), then [`Self::finish`]. bliss gets the windows one
/// after another as 22 050 Hz mono (45 s at most, about 4 MB); the sound
/// model gets each window at its own rate as soon as the window is over,
/// and only the running mean of its vectors is kept. Each window is
/// resampled on its own (no filter smears across a seam).
///
/// bliss's statistics (means and deviations of the spectral features, the
/// chroma, the tempo of the onsets) are of the windows instead of the
/// track's first 8 minutes: the same quantities over a sample of the
/// track, so vectors of either kind stay comparable and the stored ones
/// are not computed again.
pub struct WindowedAnalyzer<'m> {
    channels: usize,
    rate_in: u32,
    want_bliss: bool,
    model: Option<&'m dyn SoundModel>,
    window: Option<usize>,
    bliss_rs: Option<ToRate>,
    model_rs: Option<ToRate>,
    /// The windows so far, 22 050 Hz mono.
    bliss: Vec<f32>,
    /// The current window at the model's rate.
    model_buf: Vec<f32>,
    sum: EmbedSum,
    mono: Vec<f32>,
    frames_in_window: u64,
    frames_total: u64,
    error: Option<AnalysisError>,
    model_error: Option<AnalysisError>,
}

impl<'m> WindowedAnalyzer<'m> {
    /// `bliss`: compute bliss's vector; `model`: the sound model's too.
    pub fn new(sample_rate: u32, channels: usize, bliss: bool, model: Option<&'m dyn SoundModel>) -> Result<Self, AnalysisError> {
        if sample_rate == 0 || channels == 0 {
            return Err(AnalysisError::Format(format!("{sample_rate} Hz, {channels} channels")));
        }
        Ok(Self {
            channels,
            rate_in: sample_rate,
            want_bliss: bliss,
            model,
            window: None,
            bliss_rs: None,
            model_rs: None,
            bliss: Vec::new(),
            model_buf: Vec::new(),
            sum: EmbedSum::default(),
            mono: Vec::new(),
            frames_in_window: 0,
            frames_total: 0,
            error: None,
            model_error: None,
        })
    }

    fn open(&mut self, window: usize) -> Result<(), AnalysisError> {
        self.window = Some(window);
        self.frames_in_window = 0;
        if self.want_bliss && self.rate_in != SAMPLE_RATE {
            self.bliss_rs = Some(ToRate::new(self.rate_in, SAMPLE_RATE)?);
        }
        if let Some(m) = self.model
            && self.rate_in != m.sample_rate()
        {
            self.model_rs = Some(ToRate::new(self.rate_in, m.sample_rate())?);
        }
        Ok(())
    }

    /// Ends the current window: the resamplers' tails, and the model's pass
    /// over it.
    fn close(&mut self) -> Result<(), AnalysisError> {
        if self.window.take().is_none() {
            return Ok(());
        }
        if let Some(mut rs) = self.bliss_rs.take() {
            rs.finish(&mut self.bliss)?;
        }
        if let Some(mut rs) = self.model_rs.take() {
            rs.finish(&mut self.model_buf)?;
        }
        if let Some(m) = self.model {
            let secs = self.model_buf.len() as f64 / f64::from(m.sample_rate());
            // Less than a second says nothing (a seam's leftover).
            if secs >= 1.0 && self.model_error.is_none() {
                match m.embed(&self.model_buf) {
                    Ok(v) => self.sum.add(&v, secs),
                    Err(e) => self.model_error = Some(e),
                }
            }
            self.model_buf.clear();
        }
        Ok(())
    }

    /// Takes interleaved frames of window `window` (numbered as the plan
    /// of [`windows`]). Breaks when the window is full or something failed.
    pub fn push(&mut self, window: usize, interleaved: &[f32]) -> ControlFlow<()> {
        if self.error.is_some() {
            return ControlFlow::Break(());
        }
        if self.window != Some(window)
            && let Err(e) = self.close().and_then(|()| self.open(window))
        {
            self.error = Some(e);
            return ControlFlow::Break(());
        }
        let ch = self.channels;
        let cap = MAX_WINDOW_SECONDS * u64::from(self.rate_in);
        let frames = (interleaved.len() / ch).min(cap.saturating_sub(self.frames_in_window) as usize);
        self.frames_in_window += frames as u64;
        self.frames_total += frames as u64;
        self.mono.clear();
        self.mono.extend(interleaved[..frames * ch].chunks_exact(ch).map(|f| if ch == 1 { f[0] } else { f.iter().sum::<f32>() / ch as f32 }));
        if let Err(e) = self.feed() {
            self.error = Some(e);
            return ControlFlow::Break(());
        }
        if self.frames_in_window >= cap { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    }

    fn feed(&mut self) -> Result<(), AnalysisError> {
        if self.want_bliss {
            match &mut self.bliss_rs {
                None => self.bliss.extend_from_slice(&self.mono),
                Some(rs) => {
                    rs.inbuf.extend_from_slice(&self.mono);
                    rs.process(&mut self.bliss)?;
                }
            }
        }
        if self.model.is_some() {
            match &mut self.model_rs {
                None => self.model_buf.extend_from_slice(&self.mono),
                Some(rs) => {
                    rs.inbuf.extend_from_slice(&self.mono);
                    rs.process(&mut self.model_buf)?;
                }
            }
        }
        Ok(())
    }

    /// Seconds of audio taken so far.
    pub fn seconds(&self) -> f64 {
        self.frames_total as f64 / f64::from(self.rate_in)
    }

    /// bliss over the windows and the mean of the model's vectors.
    pub fn finish(mut self) -> Result<Analysed, AnalysisError> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        self.close()?;
        let seconds = self.seconds();
        let bliss = self.want_bliss.then(|| bliss_of(&self.bliss));
        self.bliss = Vec::new();
        let embedding = self.model.map(|_| match (self.model_error.take(), self.sum.mean()) {
            (Some(e), _) => Err(e),
            (None, Some(v)) => Ok(v),
            (None, None) => Err(AnalysisError::TooShort),
        });
        Ok(Analysed { bliss, embedding, seconds })
    }
}

/// Analyses an interleaved buffer by the windows of its length (what the
/// decoder does for a file, done in memory): bliss and, given a model, its
/// vector.
pub fn analyze_windows(samples: &[f32], sample_rate: u32, channels: usize, model: Option<&dyn SoundModel>) -> Result<Analysed, AnalysisError> {
    let mut a = WindowedAnalyzer::new(sample_rate, channels, true, model)?;
    let frames = samples.len() / channels.max(1);
    let duration = frames as f64 / f64::from(sample_rate);
    for (i, w) in windows(Some(duration)).iter().enumerate() {
        let from = ((w.start * f64::from(sample_rate)) as usize).min(frames);
        let to = (((w.start + w.len) * f64::from(sample_rate)) as usize).min(frames);
        for chunk in samples[from * channels..to * channels].chunks(CHUNK_FRAMES * 8 * channels) {
            if a.push(i, chunk).is_break() {
                break;
            }
        }
    }
    a.finish()
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

    #[test]
    fn three_windows_spread_over_the_track_and_never_overlap() {
        let w = windows(Some(240.0));
        assert_eq!(w.iter().map(|w| w.start).collect::<Vec<_>>(), [48.0, 108.0, 168.0]);
        assert!(w.iter().all(|w| w.len == WINDOW_SECONDS));
        // Just long enough: back to back from the start.
        let w = windows(Some(45.0));
        assert_eq!(w.iter().map(|w| w.start).collect::<Vec<_>>(), [0.0, 15.0, 30.0]);
        for d in [45.0, 50.0, 59.0, 61.0, 100.0, 480.0, 3600.0] {
            let w = windows(Some(d));
            assert_eq!(w.len(), 3, "{d}");
            for pair in w.windows(2) {
                assert!(pair[1].start >= pair[0].start + pair[0].len - 1e-9, "{d}: {w:?}");
            }
            assert!(w[2].start + w[2].len <= d + 1e-9, "{d}: {w:?}");
        }
        // Short: one window of 15 s around 45 %, inside the track.
        for (d, start) in [(30.0, 6.0), (44.0, 12.3), (16.0, 0.0), (8.0, 0.0)] {
            let w = windows(Some(d));
            assert_eq!(w.len(), 1, "{d}");
            assert!((w[0].start - start).abs() < 1e-9 && w[0].len == WINDOW_SECONDS, "{d}: {w:?}");
            assert!(w[0].start + WINDOW_SECONDS <= d.max(WINDOW_SECONDS) + 1e-9, "{d}: {w:?}");
        }
        // Unknown: three back to back from the start.
        let starts = |w: Vec<Window>| w.iter().map(|w| w.start).collect::<Vec<_>>();
        assert_eq!(starts(windows(None)), [0.0, 15.0, 30.0]);
        assert_eq!(starts(windows(Some(f64::NAN))), [0.0, 15.0, 30.0]);
        assert!(windows(None).iter().all(|w| w.len == WINDOW_SECONDS));
        assert_eq!(preview_windows(Some(200.0)), [Window { start: PREVIEW_START, len: PREVIEW_SECONDS }]);
        assert_eq!(preview_windows(Some(30.0))[0].start, 0.0);
        assert_eq!(preview_windows(None)[0].start, PREVIEW_START);
    }

    /// A model that answers each window with its mean level and length.
    struct Fake;

    impl SoundModel for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        fn dim(&self) -> usize {
            2
        }
        fn sample_rate(&self) -> u32 {
            16_000
        }
        fn embed(&self, mono: &[f32]) -> Result<Vec<f32>, AnalysisError> {
            let mean = mono.iter().map(|x| x.abs()).sum::<f32>() / mono.len() as f32;
            Ok(vec![mean, mono.len() as f32 / 16_000.0])
        }
    }

    /// Each window reaches the model alone, at its rate, and the vectors
    /// are averaged by the audio they cover.
    #[test]
    fn the_model_hears_each_window_and_the_mean_is_kept() {
        let rate = 44_100;
        let mut a = WindowedAnalyzer::new(rate, 2, true, Some(&Fake)).unwrap();
        // Window 0: 10 s at level 0.2; window 1: 5 s at level 0.8.
        let w0 = vec![0.2f32; rate as usize * 2 * 10];
        let w1 = vec![0.8f32; rate as usize * 2 * 5];
        for c in w0.chunks(4096) {
            let _ = a.push(0, c);
        }
        for c in w1.chunks(4096) {
            let _ = a.push(1, c);
        }
        assert!((a.seconds() - 15.0).abs() < 1e-6);
        let out = a.finish().unwrap();
        let e = out.embedding.unwrap().unwrap();
        // The mean level, by length: (0.2·10 + 0.8·5) / 15 = 0.4.
        assert!((e[0] - 0.4).abs() < 0.01, "{e:?}");
        // The length each window had at 16 kHz, averaged the same way.
        assert!((e[1] - (10.0 * 10.0 + 5.0 * 5.0) / 15.0).abs() < 0.05, "{e:?}");
        assert!(out.bliss.is_some());

        // Without bliss only the model runs; without a model only bliss.
        let mut a = WindowedAnalyzer::new(16_000, 1, false, Some(&Fake)).unwrap();
        let _ = a.push(0, &vec![0.5; 16_000 * 3]);
        let out = a.finish().unwrap();
        assert!(out.bliss.is_none() && out.embedding.unwrap().is_ok());
        let mut a = WindowedAnalyzer::new(16_000, 1, true, None).unwrap();
        let _ = a.push(0, &music(16_000, 3.0, 120.0, 0.5, 3));
        let out = a.finish().unwrap();
        assert!(out.embedding.is_none() && out.bliss.unwrap().is_ok());
        // Too little audio for the model: an error of its own.
        let mut a = WindowedAnalyzer::new(16_000, 1, false, Some(&Fake)).unwrap();
        let _ = a.push(0, &[0.1; 1000]);
        assert!(matches!(a.finish().unwrap().embedding, Some(Err(AnalysisError::TooShort))));
    }

    /// A window stops taking audio past its cap.
    #[test]
    fn a_window_has_a_cap() {
        let mut a = WindowedAnalyzer::new(8_000, 1, true, None).unwrap();
        let second = vec![0.1f32; 8_000];
        let mut n = 0;
        while a.push(0, &second).is_continue() {
            n += 1;
            assert!(n < 100);
        }
        assert_eq!(n + 1, MAX_WINDOW_SECONDS as usize);
    }

    /// A song in a key: a progression of `pattern` (semitones over `root`
    /// Hz) repeating, `bpm` notes a minute, `bright` adds high harmonics
    /// and noise (the way songs repeat their chords, unlike [`music`]'s
    /// random notes).
    fn song(rate: u32, secs: f64, bpm: f64, bright: f32, root: f64, pattern: &[i32]) -> Vec<f32> {
        let mut rng = Rng(7);
        let n = (f64::from(rate) * secs) as usize;
        let mut out = vec![0f32; n];
        let note_len = (f64::from(rate) * 60.0 / bpm) as usize;
        let (mut start, mut k) = (0, 0);
        while start < n {
            let base = root * 2f64.powf(f64::from(pattern[k % pattern.len()]) / 12.0);
            k += 1;
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

    /// bliss of three windows lands near bliss of the whole track (the
    /// first 8 minutes): much nearer than another track, so the vectors
    /// already stored stay comparable with the new ones.
    #[test]
    fn windowed_bliss_stays_comparable_with_whole_tracks() {
        let rate = 22_050;
        let major = [0, 4, 7, 12, 5, 9, 7, 2];
        let minor = [0, 3, 7, 10, 5, 8, 7, 3];
        let songs: Vec<Vec<f32>> = [(96.0, 0.3, 220.0, &major), (128.0, 1.0, 246.9, &minor), (170.0, 2.0, 196.0, &major), (110.0, 0.6, 261.6, &minor)]
            .iter()
            .map(|&(bpm, bright, root, pattern)| song(rate, 150.0, bpm, bright, root, pattern))
            .collect();
        let whole: Vec<Vector> = songs.iter().map(|s| analyze_pcm(s, rate, 1).unwrap()).collect();
        let windowed: Vec<Vector> = songs.iter().map(|s| analyze_windows(s, rate, 1, None).unwrap().bliss.unwrap().unwrap()).collect();
        let mut idx = crate::index::VectorIndex::new();
        for (i, v) in whole.iter().chain(&windowed).enumerate() {
            idx.insert(i, *v);
        }
        for i in 0..songs.len() {
            let own = idx.distance(&whole[i], &windowed[i]);
            let other = (0..songs.len()).filter(|j| *j != i).map(|j| idx.distance(&whole[i], &whole[j])).fold(f32::MAX, f32::min);
            assert!(own < other * 0.5, "song {i}: windowed {own} vs nearest other {other}");
        }
    }
}
