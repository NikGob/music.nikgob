//! What one decode of a track tells beyond its vectors, kept in a few
//! dozen bytes ([`SoundSketch`], [`SKETCH_BYTES`]):
//!
//! * **The crest** ([`Crest`]): the loudness of each second of the analysis
//!   windows ([`Loudness`]), as its 95th and 10th percentiles and its
//!   loudest second. A track that whispers for minutes and then explodes
//!   has a calm mean but a high peak and a wide spread.
//! * **The edges** ([`Edge`], [`EdgeProbe`]): the first and the last
//!   non-silent seconds of the file — their loudness, how they start or
//!   end (a cold cut or a fade), the silence before or after them, and a
//!   12-bin profile of their pitch classes ([`Edge::key`]), so a sequence
//!   can match one track's end to the next one's start.
//! * **The styles** ([`crate::styles::Styles`]): the leading Discogs
//!   styles and the coarse shares, when the neural sound model heard it.
//!
//! Loudness is the RMS of the mono mix in dB of full scale (a full-scale
//! sine is −3 dB): plain, the same for every file whatever its rate.

use realfft::RealFftPlanner;

use crate::styles::{STYLE_COUNT, Styles, TOP};

/// Bytes of a sketch, in SQLite and in memory.
pub const SKETCH_BYTES: usize = 40;
/// The tag of the sketches' loudness and edges this build computes:
/// another one is computed again.
pub const SKETCHED_WITH: &str = "sketch-1";
/// The tag of the styles part: the model file and this summary of it.
pub const STYLED_WITH: &str = "discogs-effnet-bsdynamic-1:styles-1";

/// The crest's blocks are this long…
pub const BLOCK_SECONDS: f64 = 1.0;
/// …and one quieter than this is silence, out of the crest.
pub const SILENT_DB: f32 = -55.0;
/// The crest needs this many sounding blocks.
const MIN_BLOCKS: usize = 3;

/// An edge's window: the first or last seconds of a file decoded for it…
pub const EDGE_SECONDS: f64 = 10.0;
/// …of which this much from the first (or up to the last) sounding block
/// is measured.
pub const EDGE_TAKE: f64 = 5.0;
/// The edges are measured on files this long at least (shorter ones have
/// their windows over the whole of them).
pub const EDGE_MIN_TRACK: f64 = 90.0;
/// An edge's blocks are this long.
const EDGE_BLOCK: f64 = 0.1;
/// Below the loudest block of the edge by this much is silence too.
const EDGE_RANGE_DB: f32 = 45.0;
/// The first (last) second within this much of the edge's level: a cold
/// start (end)…
const COLD_DB: f32 = 3.0;
/// …this much below it: a fade.
const FADE_DB: f32 = 8.0;
/// The pitch-class profile hears this band.
const CHROMA_LO_HZ: f32 = 55.0;
const CHROMA_HI_HZ: f32 = 2000.0;

/// dB of a mean square.
fn db(ms: f64) -> f32 {
    (10.0 * (ms + 1e-12).log10()) as f32
}

/// dB as a byte: a quarter of a dB from −60 to +3.75.
fn db_q(x: f32) -> u8 {
    ((x + 60.0) * 4.0).round().clamp(0.0, 255.0) as u8
}

fn q_db(b: u8) -> f32 {
    f32::from(b) / 4.0 - 60.0
}

fn unit_q(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn q_unit(b: u8) -> f32 {
    f32::from(b) / 255.0
}

/// The value at share `q` of sorted `v`.
fn quantile(v: &[f32], q: f32) -> f32 {
    v[((v.len() - 1) as f32 * q).round() as usize]
}

/// The loudness of a track over its analysis windows (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crest {
    /// The 95th percentile of its seconds' loudness, dB.
    pub peak: f32,
    /// The 10th percentile.
    pub low: f32,
    /// The loudest second.
    pub max: f32,
}

impl Crest {
    /// How far its loud parts stand above its quiet ones, dB.
    pub fn spread(&self) -> f32 {
        (self.peak - self.low).max(0.0)
    }
}

/// The seconds' loudness of the analysis windows as they are decoded
/// (mono at the input's rate); each window ends with [`Self::end_window`].
#[derive(Clone, Debug)]
pub struct Loudness {
    block: usize,
    acc: f64,
    n: usize,
    blocks: Vec<f32>,
}

impl Loudness {
    pub fn new(rate: u32) -> Self {
        Self { block: ((f64::from(rate) * BLOCK_SECONDS) as usize).max(1), acc: 0.0, n: 0, blocks: Vec::new() }
    }

    pub fn push(&mut self, mono: &[f32]) {
        for &x in mono {
            self.acc += f64::from(x) * f64::from(x);
            self.n += 1;
            if self.n == self.block {
                self.blocks.push(db(self.acc / self.n as f64));
                self.acc = 0.0;
                self.n = 0;
            }
        }
    }

    /// The window is over: a last block of half a block or more counts, no
    /// block spans two windows.
    pub fn end_window(&mut self) {
        if self.n * 2 >= self.block {
            self.blocks.push(db(self.acc / self.n as f64));
        }
        self.acc = 0.0;
        self.n = 0;
    }

    /// The crest of the sounding blocks; `None` with too few of them.
    pub fn crest(&self) -> Option<Crest> {
        let mut v: Vec<f32> = self.blocks.iter().copied().filter(|b| *b > SILENT_DB).collect();
        if v.len() < MIN_BLOCKS {
            return None;
        }
        v.sort_by(f32::total_cmp);
        Some(Crest { peak: quantile(&v, 0.95), low: quantile(&v, 0.10), max: v[v.len() - 1] })
    }
}

/// How an edge starts (a head) or ends (a tail).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EdgeShape {
    #[default]
    Unknown,
    /// At full level from (or to) its first (last) second.
    Cold,
    /// Rising from (or falling to) silence.
    Fade,
    /// Neither plainly.
    Gradual,
}

impl EdgeShape {
    fn to_byte(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Cold => 1,
            Self::Fade => 2,
            Self::Gradual => 3,
        }
    }

    fn from_byte(b: u8) -> Self {
        match b {
            1 => Self::Cold,
            2 => Self::Fade,
            3 => Self::Gradual,
            _ => Self::Unknown,
        }
    }
}

/// The first or the last sounding seconds of a file (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// Loudness of its [`EDGE_TAKE`] seconds, dB.
    pub loudness: f32,
    pub shape: EdgeShape,
    /// Silence before the first (after the last) sounding block, seconds
    /// (within the edge's window).
    pub silence: f32,
    /// The weight of each pitch class (C, C♯ … B), 0…15, the strongest 15.
    pub chroma: [u8; 12],
}

/// A key: the root's pitch class (0 = C) and whether it is minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub root: u8,
    pub minor: bool,
}

/// Krumhansl–Kessler's profiles of the major and the minor key.
const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

fn correlation(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let (ma, mb) = (a.iter().sum::<f32>() / 12.0, b.iter().sum::<f32>() / 12.0);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let (x, y) = (a[i] - ma, b[i] - mb);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa <= 0.0 || bb <= 0.0 { 0.0 } else { ab / (aa * bb).sqrt() }
}

impl Edge {
    /// The key its pitch classes fit best (Krumhansl–Schmuckler) and how
    /// well (the correlation, −1…1); `None` for an empty profile.
    pub fn key(&self) -> Option<(Key, f32)> {
        if self.chroma.iter().all(|c| *c == self.chroma[0]) {
            return None;
        }
        let c: [f32; 12] = std::array::from_fn(|i| f32::from(self.chroma[i]));
        let mut best: Option<(Key, f32)> = None;
        for root in 0..12u8 {
            let rotated: [f32; 12] = std::array::from_fn(|i| c[(i + usize::from(root)) % 12]);
            for (minor, profile) in [(false, &MAJOR), (true, &MINOR)] {
                let r = correlation(&rotated, profile);
                if best.is_none_or(|(_, b)| r > b) {
                    best = Some((Key { root, minor }, r));
                }
            }
        }
        best
    }
}

/// The audio of one edge window (mono at the input's rate), measured once
/// it is over ([`Self::finish`]).
#[derive(Clone, Debug)]
pub struct EdgeProbe {
    rate: u32,
    head: bool,
    samples: Vec<f32>,
}

impl EdgeProbe {
    /// `head`: the start of the file; else its end.
    pub fn new(rate: u32, head: bool) -> Self {
        Self { rate, head, samples: Vec::new() }
    }

    pub fn push(&mut self, mono: &[f32]) {
        let cap = (f64::from(self.rate) * EDGE_SECONDS * 1.5) as usize;
        let room = cap.saturating_sub(self.samples.len());
        self.samples.extend_from_slice(&mono[..mono.len().min(room)]);
    }

    /// The edge; `None` for silence or too little audio.
    pub fn finish(self) -> Option<Edge> {
        let block = ((f64::from(self.rate) * EDGE_BLOCK) as usize).max(1);
        let blocks: Vec<f32> =
            self.samples.chunks_exact(block).map(|c| db(c.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>() / c.len() as f64)).collect();
        let take = (EDGE_TAKE / EDGE_BLOCK) as usize;
        let second = (1.0 / EDGE_BLOCK) as usize;
        if blocks.len() < second * 2 {
            return None;
        }
        let loudest = blocks.iter().copied().fold(f32::MIN, f32::max);
        let floor = SILENT_DB.max(loudest - EDGE_RANGE_DB);
        let sounding = |b: &f32| *b > floor;
        let (from, to, silence) = if self.head {
            let first = blocks.iter().position(sounding)?;
            (first, (first + take).min(blocks.len()), first)
        } else {
            let last = blocks.iter().rposition(sounding)?;
            (last.saturating_sub(take - 1), last + 1, blocks.len() - 1 - last)
        };
        if to - from < second {
            return None;
        }
        let mean_db = |bs: &[f32]| db(bs.iter().map(|b| 10f64.powf(f64::from(*b) / 10.0)).sum::<f64>() / bs.len() as f64);
        let level = mean_db(&blocks[from..to]);
        let edge_second = if self.head { mean_db(&blocks[from..from + second]) } else { mean_db(&blocks[to - second..to]) };
        let shape = if to - from < 2 * second {
            EdgeShape::Unknown
        } else if edge_second >= level - COLD_DB {
            EdgeShape::Cold
        } else if edge_second <= level - FADE_DB {
            EdgeShape::Fade
        } else {
            EdgeShape::Gradual
        };
        let audio = &self.samples[from * block..to * block];
        Some(Edge { loudness: level, shape, silence: silence as f32 * EDGE_BLOCK as f32, chroma: chroma(audio, self.rate) })
    }
}

/// The 12-bin pitch-class profile of `mono` (see the module docs), the
/// strongest bin 15; all zeros for silence.
pub fn chroma(mono: &[f32], rate: u32) -> [u8; 12] {
    let frame = ((f64::from(rate) * 0.18) as usize).next_power_of_two().max(256);
    if mono.len() < frame || rate == 0 {
        return [0; 12];
    }
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(frame);
    let window: Vec<f32> = (0..frame).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / frame as f32).cos()).collect();
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let bin_hz = rate as f32 / frame as f32;
    // Each bin's pitch class, or none outside the band.
    let classes: Vec<Option<usize>> = (0..spectrum.len())
        .map(|k| {
            let f = k as f32 * bin_hz;
            (CHROMA_LO_HZ..=CHROMA_HI_HZ).contains(&f).then(|| (((12.0 * (f / 440.0).log2()).round() as i32 + 69).rem_euclid(12)) as usize)
        })
        .collect();
    let mut acc = [0f64; 12];
    let mut start = 0;
    while start + frame <= mono.len() {
        for (i, x) in input.iter_mut().enumerate() {
            *x = mono[start + i] * window[i];
        }
        if fft.process(&mut input, &mut spectrum).is_err() {
            break;
        }
        for (c, s) in classes.iter().zip(&spectrum) {
            if let Some(c) = c {
                acc[*c] += f64::from(s.norm()).ln_1p();
            }
        }
        start += frame / 2;
    }
    let top = acc.iter().copied().fold(0.0, f64::max);
    if !(top > 0.0) {
        return [0; 12];
    }
    std::array::from_fn(|i| (acc[i] / top * 15.0).round().clamp(0.0, 15.0) as u8)
}

/// Where the sketch's parts lie in its bytes (see [`SoundSketch`]).
const FLAGS: usize = 0;
const CREST: usize = 1;
const HEAD: usize = 4;
const TAIL: usize = 13;
const EDGE_BYTES: usize = 9;
const TOP_AT: usize = 22;
const COARSE: usize = TOP_AT + 3 * TOP;
const HAS_CREST: u8 = 1;
const HAS_HEAD: u8 = 2;
const HAS_TAIL: u8 = 4;
const HAS_STYLES: u8 = 8;

/// A track's sketch (see the module docs), [`SKETCH_BYTES`] as stored:
/// a byte of which parts there are; the crest (peak, low, max as
/// quarter-dB bytes); the head and the tail (loudness, shape, silence in
/// tenths of a second, the 12 pitch classes as nibbles); the leading
/// styles (index u16 LE and probability ×255 each) and the coarse shares
/// (words, electronic, acoustic, speech, the leading styles' mass, ×255).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SoundSketch([u8; SKETCH_BYTES]);

impl Default for SoundSketch {
    fn default() -> Self {
        Self([0; SKETCH_BYTES])
    }
}

fn put_edge(out: &mut [u8], e: &Edge) {
    out[0] = db_q(e.loudness);
    out[1] = e.shape.to_byte();
    out[2] = (e.silence * 10.0).round().clamp(0.0, 255.0) as u8;
    for k in 0..6 {
        out[3 + k] = (e.chroma[2 * k].min(15) << 4) | e.chroma[2 * k + 1].min(15);
    }
}

fn get_edge(b: &[u8]) -> Edge {
    Edge {
        loudness: q_db(b[0]),
        shape: EdgeShape::from_byte(b[1]),
        silence: f32::from(b[2]) / 10.0,
        chroma: std::array::from_fn(|i| if i % 2 == 0 { b[3 + i / 2] >> 4 } else { b[3 + i / 2] & 0x0f }),
    }
}

impl SoundSketch {
    pub fn new(crest: Option<Crest>, head: Option<Edge>, tail: Option<Edge>, styles: Option<&Styles>) -> Self {
        let mut s = Self::default();
        if let Some(c) = crest {
            s.0[FLAGS] |= HAS_CREST;
            s.0[CREST..CREST + 3].copy_from_slice(&[db_q(c.peak), db_q(c.low), db_q(c.max)]);
        }
        if let Some(e) = head {
            s.0[FLAGS] |= HAS_HEAD;
            put_edge(&mut s.0[HEAD..HEAD + EDGE_BYTES], &e);
        }
        if let Some(e) = tail {
            s.0[FLAGS] |= HAS_TAIL;
            put_edge(&mut s.0[TAIL..TAIL + EDGE_BYTES], &e);
        }
        s.with_styles(styles)
    }

    /// The same sketch with these styles (none: without them).
    pub fn with_styles(mut self, styles: Option<&Styles>) -> Self {
        self.0[TOP_AT..].fill(0);
        self.0[FLAGS] &= !HAS_STYLES;
        if let Some(st) = styles {
            self.0[FLAGS] |= HAS_STYLES;
            for (k, (i, p)) in st.top.iter().enumerate() {
                let at = TOP_AT + 3 * k;
                let i = if usize::from(*i) < STYLE_COUNT { *i } else { u16::MAX };
                self.0[at..at + 2].copy_from_slice(&i.to_le_bytes());
                self.0[at + 2] = unit_q(*p);
            }
            for (k, x) in [st.voice, st.electronic, st.acoustic, st.speech, st.sure].into_iter().enumerate() {
                self.0[COARSE + k] = unit_q(x);
            }
        }
        self
    }

    pub fn crest(&self) -> Option<Crest> {
        (self.0[FLAGS] & HAS_CREST != 0).then(|| Crest { peak: q_db(self.0[CREST]), low: q_db(self.0[CREST + 1]), max: q_db(self.0[CREST + 2]) })
    }

    pub fn head(&self) -> Option<Edge> {
        (self.0[FLAGS] & HAS_HEAD != 0).then(|| get_edge(&self.0[HEAD..HEAD + EDGE_BYTES]))
    }

    pub fn tail(&self) -> Option<Edge> {
        (self.0[FLAGS] & HAS_TAIL != 0).then(|| get_edge(&self.0[TAIL..TAIL + EDGE_BYTES]))
    }

    pub fn styles(&self) -> Option<Styles> {
        if self.0[FLAGS] & HAS_STYLES == 0 {
            return None;
        }
        let top = std::array::from_fn(|k| {
            let at = TOP_AT + 3 * k;
            (u16::from_le_bytes([self.0[at], self.0[at + 1]]), q_unit(self.0[at + 2]))
        });
        let c = |k: usize| q_unit(self.0[COARSE + k]);
        Some(Styles { top, voice: c(0), electronic: c(1), acoustic: c(2), speech: c(3), sure: c(4) })
    }

    /// Nothing measured.
    pub fn is_empty(&self) -> bool {
        self.0[FLAGS] == 0
    }

    pub fn to_bytes(&self) -> [u8; SKETCH_BYTES] {
        self.0
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Some(Self(b.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, secs: f64, freqs: &[f32], level: f32) -> Vec<f32> {
        (0..(f64::from(rate) * secs) as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                freqs.iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum::<f32>() * level / freqs.len() as f32
            })
            .collect()
    }

    /// A quiet track that explodes in its last window has the calm one's
    /// low but a high peak and a wide spread; silence is left out.
    #[test]
    fn the_crest_hears_the_loud_part() {
        let rate = 8_000;
        let mut calm = Loudness::new(rate);
        let mut burst = Loudness::new(rate);
        for w in 0..3 {
            calm.push(&tone(rate, 15.0, &[220.0], 0.05));
            calm.end_window();
            burst.push(&tone(rate, 15.0, &[220.0], if w == 2 { 0.9 } else { 0.05 }));
            burst.end_window();
        }
        let (c, b) = (calm.crest().unwrap(), burst.crest().unwrap());
        // A sine at 0.05 is −29 dB, at 0.9 −4 dB.
        assert!((c.peak - -29.0).abs() < 0.5 && c.spread() < 0.5, "{c:?}");
        assert!((b.peak - -3.9).abs() < 0.5 && b.spread() > 20.0 && (b.low - c.low).abs() < 0.5, "{b:?}");
        let mut silent = Loudness::new(rate);
        silent.push(&vec![0.0; rate as usize * 20]);
        silent.push(&tone(rate, 2.0, &[220.0], 0.5));
        silent.end_window();
        assert!(silent.crest().is_none(), "two sounding seconds are too few");
        // A last half block counts, a shorter one does not.
        let mut l = Loudness::new(rate);
        l.push(&tone(rate, 2.6, &[220.0], 0.5));
        l.end_window();
        l.push(&tone(rate, 1.4, &[220.0], 0.5));
        l.end_window();
        assert_eq!(l.blocks.len(), 4);
    }

    /// A head after silence that starts at full level is a cold start; one
    /// rising from nothing a fade; a tail that dies away is a fade.
    #[test]
    fn edges_tell_cold_cuts_from_fades() {
        let rate = 16_000;
        let mut head = EdgeProbe::new(rate, true);
        head.push(&vec![0.0; rate as usize * 2]);
        head.push(&tone(rate, 8.0, &[261.6, 329.6, 392.0], 0.6));
        let h = head.finish().unwrap();
        assert_eq!(h.shape, EdgeShape::Cold);
        assert!((h.silence - 2.0).abs() < 0.15, "{h:?}");
        let ramp: Vec<f32> = tone(rate, 10.0, &[220.0], 0.6).iter().enumerate().map(|(i, x)| x * (i as f32 / (rate as f32 * 4.0)).min(1.0).powi(3)).collect();
        let mut fade_in = EdgeProbe::new(rate, true);
        fade_in.push(&ramp);
        assert_eq!(fade_in.finish().unwrap().shape, EdgeShape::Fade);
        let falling: Vec<f32> = ramp.iter().rev().copied().collect();
        let mut tail = EdgeProbe::new(rate, false);
        tail.push(&falling);
        tail.push(&vec![0.0; rate as usize]);
        let t = tail.finish().unwrap();
        assert_eq!(t.shape, EdgeShape::Fade);
        // The second of silence and the end of the fade below the floor.
        assert!(t.silence > 1.0 && t.silence < 2.0, "{t:?}");
        let mut cut = EdgeProbe::new(rate, false);
        cut.push(&tone(rate, 10.0, &[220.0], 0.6));
        assert_eq!(cut.finish().unwrap().shape, EdgeShape::Cold);
        assert!(EdgeProbe::new(rate, true).finish().is_none());
        let mut quiet = EdgeProbe::new(rate, true);
        quiet.push(&vec![0.0; rate as usize * 10]);
        assert!(quiet.finish().is_none(), "silence has no edge");
    }

    /// A C major chord reads as C major, an A minor one as A minor.
    #[test]
    fn the_pitch_classes_tell_the_key() {
        let rate = 22_050;
        let edge = |freqs: &[f32]| {
            let mut p = EdgeProbe::new(rate, true);
            p.push(&tone(rate, 6.0, freqs, 0.5));
            p.finish().unwrap()
        };
        let c = edge(&[130.8, 164.8, 196.0, 261.6, 329.6, 392.0, 523.3]);
        assert!(c.chroma[0] == 15 && c.chroma[4] >= 7 && c.chroma[7] >= 7, "{:?}", c.chroma);
        assert!(c.chroma[1] < 8 && c.chroma[6] < 8 && c.chroma[10] < 8, "{:?}", c.chroma);
        assert_eq!(c.key().unwrap().0, Key { root: 0, minor: false });
        let a = edge(&[110.0, 130.8, 164.8, 220.0, 261.6, 329.6, 440.0]);
        assert_eq!(a.key().unwrap().0, Key { root: 9, minor: true });
        let flat = Edge { loudness: -20.0, shape: EdgeShape::Unknown, silence: 0.0, chroma: [0; 12] };
        assert!(flat.key().is_none());
    }

    /// Every part round-trips its bytes to the precision it is kept at,
    /// and a sketch of none is empty.
    #[test]
    fn a_sketch_round_trips() {
        let crest = Crest { peak: -9.25, low: -31.0, max: -6.5 };
        let head = Edge { loudness: -18.0, shape: EdgeShape::Fade, silence: 1.3, chroma: [15, 0, 7, 1, 9, 3, 0, 12, 2, 5, 1, 4] };
        let tail = Edge { loudness: -12.5, shape: EdgeShape::Cold, silence: 0.0, chroma: [3; 12] };
        let mut styles = Styles { top: [(42, 0.6), (7, 0.25), (399, 0.1), (u16::MAX, 0.0)], voice: 0.9, electronic: 0.2, acoustic: 0.4, speech: 0.0, sure: 0.95 };
        let s = SoundSketch::new(Some(crest), Some(head), Some(tail), Some(&styles));
        let back = SoundSketch::from_bytes(&s.to_bytes()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.crest(), Some(crest));
        assert_eq!(back.head(), Some(head));
        assert_eq!(back.tail(), Some(tail));
        let st = back.styles().unwrap();
        assert_eq!(st.top.map(|t| t.0), styles.top.map(|t| t.0));
        assert!((st.voice - 0.9).abs() < 0.003 && (st.top[0].1 - 0.6).abs() < 0.003);
        // Without the styles, the rest stays.
        let bare = s.with_styles(None);
        assert!(bare.styles().is_none() && bare.crest() == Some(crest) && bare.tail() == Some(tail));
        styles.top[1].0 = 4000;
        assert_eq!(SoundSketch::default().with_styles(Some(&styles)).styles().unwrap().top[1].0, u16::MAX, "no such style");
        assert!(SoundSketch::default().is_empty());
        assert!(SoundSketch::from_bytes(&[0; 3]).is_none());
        assert_eq!(std::mem::size_of::<SoundSketch>(), SKETCH_BYTES);
    }
}
