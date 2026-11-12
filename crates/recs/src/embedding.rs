//! A trained model of sound beside bliss (docs/recommendations.md §5): a
//! Discogs-EffNet embedding hears the genre and the style bliss does not.
//!
//! * [`SoundModel`]: mono PCM of one window at the model's rate in, the
//!   model's raw vector out (the runtime implements it over ONNX Runtime;
//!   this crate stays without one).
//! * [`Mel`]: the model's input, MusiCNN-style log-mel patches as Essentia
//!   makes them for Discogs-EffNet (16 kHz, 512-point frames every 256
//!   samples, 96 Slaney mel bands of the power spectrum, `log10(1 + 10⁴·x)`,
//!   patches of 128 frames).
//! * [`EmbedSum`]: the windows' vectors averaged on the fly, so no window's
//!   audio outlives its own pass.
//! * [`Compact`]: what is kept of a track, 256 × `i8` with one scale: a
//!   fixed random projection ([`project`]) of the averaged vector,
//!   L2-normalised and quantised. 260 bytes a track in SQLite and in
//!   memory; its cosines are within about 0.01 of the float ones.
//!
//! Vectors are compared in [`crate::index::VectorIndex`] (a track without
//! one falls back to bliss there) and against a niche by
//! [`crate::index::VectorIndex::niche_fit`].

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

use crate::analysis::AnalysisError;

/// A model of sound: mono PCM at its own rate in, a vector out.
pub trait SoundModel: Send + Sync {
    /// The model and its version, kept with each vector (like
    /// [`crate::ANALYZED_WITH`]): another one is computed again.
    fn name(&self) -> &str;
    /// The length of its raw vectors.
    fn dim(&self) -> usize;
    /// The rate of the PCM it reads (16 000 Hz for Discogs-EffNet).
    fn sample_rate(&self) -> u32;
    /// The vector of one window of mono PCM at [`Self::sample_rate`] (the
    /// mean over the window's patches).
    fn embed(&self, mono: &[f32]) -> Result<Vec<f32>, AnalysisError>;
}

/// The rate Discogs-EffNet reads.
pub const EMBED_RATE: u32 = 16_000;
/// Length of the raw Discogs-EffNet embedding (its `PartitionedCall:1`).
pub const EFFNET_DIM: usize = 1280;
/// Bytes of the kept vector.
pub const COMPACT_DIM: usize = 256;
/// The projection's seed: part of every stored vector's tag.
pub const PROJECTION_SEED: u64 = 0x5EED_0256_EFF0_0001;
/// The tag of the compact vectors this build computes: the model file and
/// the projection. A stored vector with another tag is computed again.
pub const EMBEDDED_WITH: &str = "discogs-effnet-bsdynamic-1:rp256-1";

/// FFT frame of the mel front end.
pub const FRAME: usize = 512;
/// Hop between frames.
pub const HOP: usize = 256;
/// Mel bands.
pub const MEL_BANDS: usize = 96;
/// Frames of one patch (the model's input is `[n, PATCH, MEL_BANDS]`).
pub const PATCH: usize = 128;
/// Frames between patches: no overlap (Essentia's default is 62, about one
/// a second; a window of 15 s gives 7 patches instead of 14 for half the
/// time, and the average over them barely moves).
pub const PATCH_HOP: usize = 128;

/// The MusiCNN-style log-mel front end of Discogs-EffNet.
pub struct Mel {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    /// Per band: its first FFT bin and the weights from there.
    filters: Vec<(usize, Vec<f32>)>,
}

impl Default for Mel {
    fn default() -> Self {
        Self::new()
    }
}

fn hz_to_mel(f: f64) -> f64 {
    // Slaney's: linear below 1 kHz, logarithmic above.
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    let logstep = 6.4f64.ln() / 27.0;
    if f >= MIN_LOG_HZ { min_log_mel + (f / MIN_LOG_HZ).ln() / logstep } else { f / F_SP }
}

fn mel_to_hz(m: f64) -> f64 {
    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    let min_log_mel = MIN_LOG_HZ / F_SP;
    let logstep = 6.4f64.ln() / 27.0;
    if m >= min_log_mel { MIN_LOG_HZ * (logstep * (m - min_log_mel)).exp() } else { F_SP * m }
}

impl Mel {
    pub fn new() -> Self {
        let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
        // Essentia's Hann (not normalised).
        let window = (0..FRAME)
            .map(|i| (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / (FRAME - 1) as f64).cos()) as f32)
            .collect();
        let bins = FRAME / 2 + 1;
        let rate = f64::from(EMBED_RATE);
        let (lo, hi) = (hz_to_mel(0.0), hz_to_mel(rate / 2.0));
        let edges: Vec<f64> = (0..MEL_BANDS + 2)
            .map(|i| mel_to_hz(lo + (hi - lo) * i as f64 / (MEL_BANDS + 1) as f64))
            .collect();
        let bin_hz = |j: usize| j as f64 * rate / FRAME as f64;
        let filters = (0..MEL_BANDS)
            .map(|b| {
                let (f0, f1, f2) = (edges[b], edges[b + 1], edges[b + 2]);
                // Triangles in Hz, each of unit area ("unit_tri").
                let norm = 2.0 / (f2 - f0);
                let mut first = None;
                let mut w = Vec::new();
                for j in 0..bins {
                    let f = bin_hz(j);
                    let v = if f > f0 && f < f2 {
                        if f <= f1 { (f - f0) / (f1 - f0) } else { (f2 - f) / (f2 - f1) }
                    } else {
                        0.0
                    };
                    if v > 0.0 {
                        first.get_or_insert(j);
                        w.push((v * norm) as f32);
                    } else if first.is_some() {
                        break;
                    }
                }
                (first.unwrap_or(0), w)
            })
            .collect();
        Self { fft, window, filters }
    }

    /// The log-mel frames of `mono` (16 kHz), `MEL_BANDS` values each, the
    /// first frame centred on the first sample (Essentia's frame cutter).
    pub fn frames(&self, mono: &[f32]) -> Vec<f32> {
        if mono.is_empty() {
            return Vec::new();
        }
        let n = 1 + mono.len().div_ceil(HOP);
        let mut out = Vec::with_capacity(n * MEL_BANDS);
        let mut input = self.fft.make_input_vec();
        let mut spectrum = self.fft.make_output_vec();
        let mut scratch = self.fft.make_scratch_vec();
        let mut power = vec![0f32; FRAME / 2 + 1];
        for f in 0..n {
            let centre = (f * HOP) as isize;
            let start = centre - (FRAME / 2) as isize;
            if start >= mono.len() as isize {
                break;
            }
            for (i, x) in input.iter_mut().enumerate() {
                let at = start + i as isize;
                let s = if at >= 0 && (at as usize) < mono.len() { mono[at as usize] } else { 0.0 };
                *x = s * self.window[i];
            }
            if self.fft.process_with_scratch(&mut input, &mut spectrum, &mut scratch).is_err() {
                break;
            }
            for (p, c) in power.iter_mut().zip(&spectrum) {
                *p = c.norm_sqr();
            }
            for (first, w) in &self.filters {
                let e: f32 = w.iter().zip(&power[*first..]).map(|(a, b)| a * b).sum();
                out.push((1.0 + 10_000.0 * e).log10());
            }
        }
        out
    }

    /// The starts (in frames) of the patches of `frames` frames; a window
    /// shorter than one patch gives one patch, its frames repeated.
    pub fn patch_starts(frames: usize) -> impl Iterator<Item = usize> {
        let n = if frames == 0 { 0 } else if frames <= PATCH { 1 } else { 1 + (frames - PATCH) / PATCH_HOP };
        (0..n).map(|i| i * PATCH_HOP)
    }

    /// One patch (`PATCH × MEL_BANDS`) of `frames` from frame `start`,
    /// repeating the frames when there are not enough.
    pub fn patch(frames: &[f32], start: usize) -> Vec<f32> {
        let total = frames.len() / MEL_BANDS;
        let mut out = Vec::with_capacity(PATCH * MEL_BANDS);
        for i in 0..PATCH {
            let f = if start + i < total { start + i } else { (start + i) % total.max(1) };
            out.extend_from_slice(&frames[f * MEL_BANDS..(f + 1) * MEL_BANDS]);
        }
        out
    }
}

/// The windows' raw vectors, averaged as they come (weighted by the audio
/// each covers).
#[derive(Clone, Debug, Default)]
pub struct EmbedSum {
    sum: Vec<f64>,
    weight: f64,
}

impl EmbedSum {
    pub fn add(&mut self, v: &[f32], weight: f64) {
        if weight <= 0.0 || v.iter().any(|x| !x.is_finite()) {
            return;
        }
        if self.sum.is_empty() {
            self.sum = vec![0.0; v.len()];
        }
        if self.sum.len() != v.len() {
            return;
        }
        for (s, x) in self.sum.iter_mut().zip(v) {
            *s += f64::from(*x) * weight;
        }
        self.weight += weight;
    }

    /// The mean, `None` before the first vector.
    pub fn mean(&self) -> Option<Vec<f32>> {
        (self.weight > 0.0).then(|| self.sum.iter().map(|s| (s / self.weight) as f32).collect())
    }
}

/// A fixed random projection of a raw vector of any length to
/// [`COMPACT_DIM`] dimensions (±1 entries from [`PROJECTION_SEED`], made on
/// the fly: no matrix is kept). Distances survive it (Johnson–Lindenstrauss).
pub fn project(v: &[f32]) -> [f32; COMPACT_DIM] {
    let mut out = [0f32; COMPACT_DIM];
    for (r, o) in out.iter_mut().enumerate() {
        let mut rng = crate::rng::Rng::derive(PROJECTION_SEED, r as u64);
        let mut acc = 0f64;
        let mut bits = 0u64;
        for (i, x) in v.iter().enumerate() {
            if i % 64 == 0 {
                bits = rng.next_u64();
            }
            let sign = if bits >> (i % 64) & 1 == 1 { 1.0 } else { -1.0 };
            acc += sign * f64::from(*x);
        }
        *o = (acc / (COMPACT_DIM as f64).sqrt()) as f32;
    }
    out
}

/// A kept vector: unit length, 256 × `i8` times `scale`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Compact {
    pub scale: f32,
    pub q: [i8; COMPACT_DIM],
}

impl Compact {
    /// Bytes of [`Self::to_bytes`].
    pub const BYTES: usize = 4 + COMPACT_DIM;

    /// The kept vector of a raw embedding: projected, L2-normalised,
    /// quantised. `None` for a vector of zeros or not a number.
    pub fn of_raw(raw: &[f32]) -> Option<Self> {
        if raw.is_empty() || raw.iter().any(|x| !x.is_finite()) {
            return None;
        }
        Self::of_unit(&project(raw))
    }

    /// Quantises `v` after normalising it to unit length.
    pub fn of_unit(v: &[f32; COMPACT_DIM]) -> Option<Self> {
        let norm = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
        if !(norm > 1e-12) || !norm.is_finite() {
            return None;
        }
        let max = v.iter().fold(0f64, |m, x| m.max((f64::from(*x) / norm).abs()));
        let scale = max / 127.0;
        if !(scale > 0.0) {
            return None;
        }
        let q = std::array::from_fn(|i| (f64::from(v[i]) / norm / scale).round().clamp(-127.0, 127.0) as i8);
        Some(Self { scale: scale as f32, q })
    }

    /// The (about unit) vector back.
    pub fn values(&self) -> [f32; COMPACT_DIM] {
        std::array::from_fn(|i| f32::from(self.q[i]) * self.scale)
    }

    /// Cosine of two kept vectors.
    pub fn cosine(&self, other: &Self) -> f32 {
        let (mut ab, mut aa, mut bb) = (0i64, 0i64, 0i64);
        for (a, b) in self.q.iter().zip(&other.q) {
            let (a, b) = (i64::from(*a), i64::from(*b));
            ab += a * b;
            aa += a * a;
            bb += b * b;
        }
        if aa == 0 || bb == 0 {
            return 0.0;
        }
        (ab as f64 / ((aa as f64).sqrt() * (bb as f64).sqrt())) as f32
    }

    /// `scale` (f32, little endian), then the 256 bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::BYTES);
        out.extend_from_slice(&self.scale.to_le_bytes());
        out.extend(self.q.iter().map(|x| x.to_le_bytes()[0]));
        out
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        if b.len() != Self::BYTES {
            return None;
        }
        let scale = f32::from_le_bytes(b[..4].try_into().ok()?);
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let q = std::array::from_fn(|i| i8::from_le_bytes([b[4 + i]]));
        Some(Self { scale, q })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::tests::Rng;

    fn unit(rng: &mut Rng) -> [f32; COMPACT_DIM] {
        let v: [f32; COMPACT_DIM] = std::array::from_fn(|_| rng.next_f32());
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.map(|x| x / n)
    }

    fn cos(a: &[f32], b: &[f32]) -> f32 {
        let d: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        d / (a.iter().map(|x| x * x).sum::<f32>().sqrt() * b.iter().map(|x| x * x).sum::<f32>().sqrt())
    }

    /// The quantised cosine is within 0.01 of the float one, for pairs
    /// near and far, and the bytes round-trip.
    #[test]
    fn quantisation_keeps_the_cosine() {
        let mut rng = Rng(17);
        let mut worst = 0f32;
        for i in 0..400 {
            let a = unit(&mut rng);
            // Every other pair is a close one (a small change of `a`).
            let b = if i % 2 == 0 {
                let noise = unit(&mut rng);
                std::array::from_fn(|k| a[k] + noise[k] * 0.3)
            } else {
                unit(&mut rng)
            };
            let (qa, qb) = (Compact::of_unit(&a).unwrap(), Compact::of_unit(&b).unwrap());
            let err = (qa.cosine(&qb) - cos(&a, &b)).abs();
            worst = worst.max(err);
            assert_eq!(Compact::from_bytes(&qa.to_bytes()), Some(qa));
        }
        assert!(worst < 0.01, "worst cosine error {worst}");
        assert_eq!(Compact::of_unit(&[0.0; COMPACT_DIM]), None);
        assert_eq!(Compact::from_bytes(&[0; 10]), None);
    }

    /// The projection keeps the angles of raw vectors roughly (the
    /// ordering of near and far), and it is the same every time.
    #[test]
    fn projection_keeps_angles() {
        let mut rng = Rng(5);
        let base: Vec<f32> = (0..EFFNET_DIM).map(|_| rng.next_f32()).collect();
        let near: Vec<f32> = base.iter().map(|x| x + rng.next_f32() * 0.2).collect();
        let far: Vec<f32> = (0..EFFNET_DIM).map(|_| rng.next_f32()).collect();
        let (pb, pn, pf) = (project(&base), project(&near), project(&far));
        assert!((cos(&pb, &pn) - cos(&base, &near)).abs() < 0.1);
        assert!((cos(&pb, &pf) - cos(&base, &far)).abs() < 0.15);
        assert!(cos(&pb, &pn) > cos(&pb, &pf) + 0.5);
        assert_eq!(project(&base), pb, "deterministic");
        let (cb, cn) = (Compact::of_raw(&base).unwrap(), Compact::of_raw(&near).unwrap());
        assert!((cb.cosine(&cn) - cos(&pb, &pn)).abs() < 0.01);
    }

    /// A tone lights the band of its frequency; silence is all zeros
    /// (`log10(1)`); patches cover the frames and pad a short window.
    #[test]
    fn mel_frames_and_patches() {
        let mel = Mel::new();
        let rate = EMBED_RATE as f32;
        let tone: Vec<f32> = (0..EMBED_RATE as usize).map(|i| (std::f32::consts::TAU * 1000.0 * i as f32 / rate).sin() * 0.5).collect();
        let frames = mel.frames(&tone);
        let n = frames.len() / MEL_BANDS;
        assert_eq!(n, 1 + tone.len().div_ceil(HOP));
        let mid = &frames[(n / 2) * MEL_BANDS..(n / 2 + 1) * MEL_BANDS];
        let loudest = (0..MEL_BANDS).max_by(|a, b| mid[*a].total_cmp(&mid[*b])).unwrap();
        // 1 kHz lies at mel 15 of the 0–8 kHz scale's ~45: about a third.
        let expect = hz_to_mel(1000.0) / hz_to_mel(8000.0) * (MEL_BANDS + 1) as f64;
        assert!((loudest as f64 - expect).abs() <= 2.0, "{loudest} vs {expect}");
        assert!(mel.frames(&vec![0.0; 4000]).iter().all(|x| *x == 0.0));

        assert_eq!(Mel::patch_starts(0).count(), 0);
        assert_eq!(Mel::patch_starts(50).collect::<Vec<_>>(), [0]);
        assert_eq!(Mel::patch_starts(PATCH * 3 + 5).collect::<Vec<_>>(), [0, PATCH_HOP, 2 * PATCH_HOP]);
        let short = vec![1.0; 10 * MEL_BANDS];
        assert_eq!(Mel::patch(&short, 0).len(), PATCH * MEL_BANDS);
    }

    #[test]
    fn the_sum_averages_by_weight() {
        let mut s = EmbedSum::default();
        assert_eq!(s.mean(), None);
        s.add(&[1.0, 0.0], 1.0);
        s.add(&[0.0, 1.0], 3.0);
        s.add(&[f32::NAN, 1.0], 1.0);
        s.add(&[5.0], 1.0);
        assert_eq!(s.mean(), Some(vec![0.25, 0.75]));
    }
}
