//! One decoding pass over a file that yields everything the pipeline wants to
//! know about the audio itself: a Chromaprint fingerprint (first 120 s),
//! integrated loudness (EBU R128) and, for lossless inputs, the spectrum verdict.

use std::ops::ControlFlow;
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ebur128::{EbuR128, Mode};
use rusty_chromaprint::{Configuration, FingerprintCompressor, Fingerprinter, match_fingerprints};

use crate::decode::{PcmSpec, decode_file};
use crate::error::Result;
use crate::spectrum::{SpectrumCollector, SpectrumReport};

/// The fingerprint covers at most this much audio (AcoustID needs no more).
pub const FINGERPRINT_SECS: u64 = 120;

#[derive(Clone, Debug, PartialEq)]
pub struct Fingerprint {
    /// Raw sub-fingerprints (Chromaprint algorithm 2, the AcoustID default).
    pub raw: Vec<u32>,
    /// Compressed and URL-safe base64, the form AcoustID and `ACOUSTID_FINGERPRINT` use.
    pub encoded: String,
    /// Length of the fingerprinted audio.
    pub duration: Duration,
}

impl Fingerprint {
    pub fn from_raw(raw: Vec<u32>, duration: Duration) -> Self {
        let config = Configuration::preset_test2();
        let bytes = FingerprintCompressor::from(&config).compress(&raw);
        Self {
            encoded: URL_SAFE_NO_PAD.encode(bytes),
            raw,
            duration,
        }
    }

    /// Decodes a fingerprint stored in a tag (`Fingerprint::encoded`).
    pub fn decode(encoded: &str) -> Option<Vec<u32>> {
        let bytes = URL_SAFE_NO_PAD.decode(encoded.trim()).ok()?;
        decompress(&bytes)
    }
}

/// Inverse of `FingerprintCompressor::compress` (Chromaprint's wire format).
fn decompress(data: &[u8]) -> Option<Vec<u32>> {
    if data.len() < 4 {
        return None;
    }
    let size = (usize::from(data[1]) << 16) | (usize::from(data[2]) << 8) | usize::from(data[3]);
    let body = &data[4..];
    let unpack = |bytes: &[u8], n: usize, count: usize| -> Vec<u8> {
        let mut out = Vec::with_capacity(count);
        let mut bit = 0usize;
        while out.len() < count {
            let byte = bit / 8;
            if byte >= bytes.len() {
                break;
            }
            let shift = bit % 8;
            let mut v = u16::from(bytes[byte]) >> shift;
            if shift + n > 8 && byte + 1 < bytes.len() {
                v |= u16::from(bytes[byte + 1]) << (8 - shift);
            }
            out.push((v & ((1 << n) - 1)) as u8);
            bit += n;
        }
        out
    };
    // The normal array holds, per sub-fingerprint, bit positions as deltas
    // terminated by a 0; we do not know its length up front, so read greedily
    // until `size` zero terminators were seen.
    let mut normal = Vec::new();
    let mut zeros = 0usize;
    let mut bit = 0usize;
    while zeros < size {
        let byte = bit / 8;
        if byte >= body.len() {
            return None;
        }
        let shift = bit % 8;
        let mut v = u16::from(body[byte]) >> shift;
        if shift + 3 > 8 && byte + 1 < body.len() {
            v |= u16::from(body[byte + 1]) << (8 - shift);
        }
        let v = (v & 7) as u8;
        if v == 0 {
            zeros += 1;
        }
        normal.push(v);
        bit += 3;
    }
    let normal_bytes = (normal.len() * 3).div_ceil(8);
    let exceptional_src = body.get(normal_bytes..)?;
    let exceptional_needed = normal.iter().filter(|v| **v == 7).count();
    let exceptional = unpack(exceptional_src, 5, exceptional_needed);
    if exceptional.len() < exceptional_needed {
        return None;
    }
    let mut out = Vec::with_capacity(size);
    let (mut last, mut cur, mut pos) = (0u32, 0u32, 0u32);
    let mut ex = exceptional.into_iter();
    for v in normal {
        if v == 0 {
            last ^= cur;
            out.push(last);
            cur = 0;
            pos = 0;
            continue;
        }
        let mut delta = u32::from(v);
        if v == 7 {
            delta += u32::from(ex.next()?);
        }
        pos += delta;
        if pos == 0 || pos > 32 {
            return None;
        }
        cur |= 1 << (pos - 1);
    }
    (out.len() == size).then_some(out)
}

/// How alike two fingerprints are: the share (0..=1) of the shorter one that
/// lies in matching segments. Same recording -> near 1, different music -> near 0.
pub fn fingerprint_similarity(a: &[u32], b: &[u32]) -> f64 {
    if a.len() < 8 || b.len() < 8 {
        return 0.0;
    }
    let config = Configuration::preset_test2();
    let Ok(segments) = match_fingerprints(a, b, &config) else {
        return 0.0;
    };
    // Segment scores are bit-error rates in bits (0 = identical, 32 = unrelated).
    let covered: usize = segments
        .iter()
        .filter(|s| s.score < 10.0)
        .map(|s| s.items_count)
        .sum();
    (covered as f64 / a.len().min(b.len()) as f64).min(1.0)
}

/// Two fingerprints are the same recording above this similarity.
pub const SAME_RECORDING_SIMILARITY: f64 = 0.6;

#[derive(Clone, Debug)]
pub struct AudioAnalysis {
    pub spec: PcmSpec,
    pub frames: u64,
    pub fingerprint: Option<Fingerprint>,
    /// Integrated loudness, LUFS.
    pub loudness_lufs: Option<f64>,
    pub spectrum: Option<SpectrumReport>,
    /// The averaged spectrum behind `spectrum`, in
    /// [`nikgob_core::quality::SPECTRUM_POINTS`] bands.
    pub spectrum_db: Option<Vec<f32>>,
}

impl AudioAnalysis {
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.frames as f64 / f64::from(self.spec.sample_rate.max(1)))
    }
}

/// Decodes `path` once and runs the requested analyses. `duration_hint` (from
/// the container) spreads the spectrum windows over the whole file.
pub fn analyze_file(
    path: &Path,
    want_spectrum: bool,
    duration_hint: Duration,
) -> Result<AudioAnalysis> {
    struct State {
        printer: Fingerprinter,
        printer_ok: bool,
        fp_frames_left: u64,
        meter: Option<EbuR128>,
        spectrum: Option<SpectrumCollector>,
        frames: u64,
        fp_frames: u64,
        i16_buf: Vec<i16>,
        started: Option<PcmSpec>,
    }
    let mut st = State {
        printer: Fingerprinter::new(&Configuration::preset_test2()),
        printer_ok: false,
        fp_frames_left: 0,
        meter: None,
        spectrum: None,
        frames: 0,
        fp_frames: 0,
        i16_buf: Vec::new(),
        started: None,
    };
    let mut sink = |chunk: &[f32], spec: PcmSpec| -> ControlFlow<()> {
        if st.started.is_none() {
            st.started = Some(spec);
            st.printer_ok = st
                .printer
                .start(spec.sample_rate, spec.channels as u32)
                .is_ok();
            st.fp_frames_left = u64::from(spec.sample_rate) * FINGERPRINT_SECS;
            st.meter = EbuR128::new(spec.channels as u32, spec.sample_rate, Mode::I).ok();
            if want_spectrum {
                st.spectrum = Some(SpectrumCollector::new(
                    spec.sample_rate,
                    spec.channels,
                    duration_hint.as_secs_f64(),
                ));
            }
        }
        let ch = spec.channels;
        let frames = (chunk.len() / ch) as u64;
        st.frames += frames;
        if st.printer_ok && st.fp_frames_left > 0 {
            let take = frames.min(st.fp_frames_left) as usize;
            st.i16_buf.clear();
            st.i16_buf.extend(
                chunk[..take * ch]
                    .iter()
                    .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16),
            );
            st.printer.consume(&st.i16_buf);
            st.fp_frames_left -= take as u64;
            st.fp_frames += take as u64;
        }
        if let Some(m) = &mut st.meter {
            let _ = m.add_frames_f32(chunk);
        }
        if let Some(s) = &mut st.spectrum {
            s.push(chunk);
        }
        ControlFlow::Continue(())
    };
    let spec = decode_file(path, &mut sink)?;
    drop(sink);

    let fingerprint = if st.printer_ok && st.fp_frames > 0 {
        st.printer.finish();
        let raw = st.printer.fingerprint().to_vec();
        (!raw.is_empty()).then(|| {
            Fingerprint::from_raw(
                raw,
                Duration::from_secs_f64(st.fp_frames as f64 / f64::from(spec.sample_rate)),
            )
        })
    } else {
        None
    };
    let loudness_lufs = st
        .meter
        .as_ref()
        .and_then(|m| m.loudness_global().ok())
        .filter(|l| l.is_finite());
    let (spectrum, spectrum_db) = match st.spectrum.map(SpectrumCollector::finish_with_levels) {
        Some((report, levels)) => (Some(report), levels),
        None => (None, None),
    };
    Ok(AudioAnalysis {
        spec,
        frames: st.frames,
        fingerprint,
        loudness_lufs,
        spectrum,
        spectrum_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::testsig::Rng;

    /// Music-like test signal: a few decaying notes with harmonics.
    pub(crate) fn melody(rate: u32, secs: f64, seed: u64) -> Vec<f32> {
        let mut rng = Rng(seed);
        let n = (f64::from(rate) * secs) as usize;
        let mut out = vec![0f32; n];
        let note_len = (rate as usize) / 4;
        let mut start = 0;
        while start < n {
            let base = 110.0 * 2f64.powf(f64::from((rng.next_f32().abs() * 24.0) as i32) / 12.0);
            for i in 0..note_len.min(n - start) {
                let t = i as f64 / f64::from(rate);
                let env = (-3.0 * t).exp() * (1.0 - (-200.0 * t).exp());
                let mut s = 0.0;
                for h in 1..=5 {
                    s += (2.0 * std::f64::consts::PI * base * f64::from(h) * t).sin() / f64::from(h);
                }
                out[start + i] += (s * env * 0.3) as f32;
            }
            start += note_len;
        }
        out
    }

    fn fp(samples: &[f32], rate: u32) -> Fingerprint {
        let mut p = Fingerprinter::new(&Configuration::preset_test2());
        p.start(rate, 1).unwrap();
        let i16s: Vec<i16> = samples.iter().map(|s| (s * 32767.0) as i16).collect();
        p.consume(&i16s);
        p.finish();
        Fingerprint::from_raw(p.fingerprint().to_vec(), Duration::from_secs(1))
    }

    #[test]
    fn fingerprint_roundtrips_through_its_tag_form() {
        let f = fp(&melody(44_100, 20.0, 1), 44_100);
        assert!(f.raw.len() > 20);
        let back = Fingerprint::decode(&f.encoded).expect("decodes");
        assert_eq!(back, f.raw);
        assert!(Fingerprint::decode("not base64 !!").is_none());
        assert!(Fingerprint::decode("AAAA").is_none());
    }

    #[test]
    fn same_audio_matches_and_different_audio_does_not() {
        let a = melody(44_100, 25.0, 11);
        let same = fp(&a, 44_100);
        // The same recording at another sample rate and a bit quieter.
        let resampled: Vec<f32> = crate::resample::resample_all(&a, 1, 44_100, 48_000)
            .iter()
            .map(|s| s * 0.7)
            .collect();
        let again = fp(&resampled, 48_000);
        let other = fp(&melody(44_100, 25.0, 99), 44_100);
        let sim_same = fingerprint_similarity(&same.raw, &again.raw);
        let sim_other = fingerprint_similarity(&same.raw, &other.raw);
        assert!(sim_same > SAME_RECORDING_SIMILARITY, "same: {sim_same}");
        assert!(sim_other < 0.3, "different: {sim_other}");
        assert!(fingerprint_similarity(&same.raw, &same.raw) > 0.95);
        assert_eq!(fingerprint_similarity(&[], &same.raw), 0.0);
    }
}
