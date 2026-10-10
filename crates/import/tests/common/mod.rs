#![allow(dead_code)]

//! Fixture builders shared by the integration tests: synthetic audio, covers,
//! and a library root in a temp dir.

use std::path::{Path, PathBuf};

use nikgob_import::ImportConfig;
use realfft::RealFftPlanner;

pub struct Rng(pub u64);

impl Rng {
    /// Uniform in [-1, 1).
    pub fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }
}

/// A few decaying notes with harmonics (what the fingerprint latches on to).
pub fn melody(rate: u32, secs: f64, seed: u64) -> Vec<f32> {
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

/// Music-like and full band: bright sawtooth notes (a 1/h harmonic series that
/// runs to Nyquist and folds back, smooth like a real mix) plus a little noise.
/// The fake-lossless check must see this as a genuine lossless file.
pub fn broadband(rate: u32, secs: f64, seed: u64) -> Vec<f32> {
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
            let saw = 2.0 * (base * t).fract() - 1.0;
            out[start + i] = (0.3 * env * saw) as f32;
        }
        start += note_len;
    }
    let mut noise = Rng(seed ^ 0x9E37_79B9);
    for s in &mut out {
        *s += noise.next_f32() * 0.002;
    }
    out
}

/// White noise cut by a brick wall at `cutoff_hz`: what an MP3 decode looks like.
pub fn lowpassed_noise(rate: u32, secs: f64, cutoff_hz: f32, seed: u64) -> Vec<f32> {
    let n = (f64::from(rate) * secs) as usize & !1;
    let mut rng = Rng(seed);
    let mut sig: Vec<f32> = (0..n).map(|_| rng.next_f32() * 0.2).collect();
    let mut planner = RealFftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let mut spec = fwd.make_output_vec();
    fwd.process(&mut sig, &mut spec).unwrap();
    let hz_per_bin = rate as f32 / n as f32;
    for (k, c) in spec.iter_mut().enumerate() {
        if k as f32 * hz_per_bin > cutoff_hz {
            *c = Default::default();
        }
    }
    inv.process(&mut spec, &mut sig).unwrap();
    for s in &mut sig {
        *s = *s / n as f32 + rng.next_f32() * (0.5 / 32_768.0);
    }
    sig
}

pub fn write_wav(path: &Path, mono: &[f32], rate: u32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in mono {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

/// FLAC written by ffmpeg (libFLAC-compatible; the pure-Rust `flacenc` output is
/// not readable by Symphonia, so it is not used for end-to-end fixtures).
pub fn ffmpeg_flac(wav: &Path, flac: &Path) {
    let status = std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(wav)
        .args(["-c:a", "flac"])
        .arg(flac)
        .status()
        .expect("ffmpeg on PATH");
    assert!(status.success());
}

pub fn solid_png(rgb: [u8; 3], w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// A library root with Music/, Inbox/, Downloads/, Trash/ under a temp dir.
pub struct Root {
    pub dir: tempfile::TempDir,
    pub cfg: ImportConfig,
}

impl Root {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ImportConfig::with_root(dir.path());
        cfg.fetch_lyrics = false;
        cfg.online_timeout = std::time::Duration::from_secs(5);
        for d in [&cfg.library_dir, &cfg.inbox_dir, &cfg.downloads_dir, &cfg.trash_dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        Self { dir, cfg }
    }

    pub fn inbox(&self, name: &str) -> PathBuf {
        self.cfg.inbox_dir.join(name)
    }
}

pub fn count_files(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| e.path().is_file()).count())
        .unwrap_or(0)
}
