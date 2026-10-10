//! Files written by real encoders (ffmpeg with libopus, libmp3lame, libvorbis,
//! aac, flac, alac; see `tests/fixtures/regenerate.sh`) decoded through the
//! whole engine and compared with the analytic signal they encode.

mod common;

use common::*;
use nikgob_audio::PlayItem;
use std::path::PathBuf;

const RATE: u32 = 48_000;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// The signal the fixtures encode, at the device rate, left channel only.
fn reference(offset_secs: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| {
            let t = n as f64 / f64::from(RATE) + offset_secs;
            (0.3 * (std::f64::consts::TAU * 440.0 * t).sin()) as f32
        })
        .collect()
}

struct Measured {
    frames: usize,
    lag: i64,
    snr: f32,
}

/// Plays a fixture and finds the lag (in device frames) at which the left
/// channel best matches the analytic signal.
fn measure(name: &str, offset_secs: f64) -> Measured {
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, fixture(name)));
    let out = rig.device.drain(RATE as usize * 10);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let frames = left.len();
    let window = 8000.min(frames / 3);
    let start = frames / 3;
    let mut best = (0i64, f32::MAX);
    // A pure tone only fixes the lag modulo its period (109 frames at 440 Hz),
    // so search within half a period.
    for lag in -54i64..=54 {
        let err: f32 = (start..start + window)
            .map(|n| {
                let t = (n as f64 - lag as f64) / f64::from(RATE) + offset_secs;
                let r = (0.3 * (std::f64::consts::TAU * 440.0 * t).sin()) as f32;
                (left[n] - r) * (left[n] - r)
            })
            .sum();
        if err < best.1 {
            best = (lag, err);
        }
    }
    let sig: f32 = (start..start + window)
        .map(|n| {
            let t = (n as f64 - best.0 as f64) / f64::from(RATE) + offset_secs;
            let r = (0.3 * (std::f64::consts::TAU * 440.0 * t).sin()) as f32;
            r * r
        })
        .sum();
    Measured {
        frames,
        lag: best.0,
        snr: 10.0 * (sig / best.1.max(1e-20)).log10(),
    }
}

#[test]
fn real_libopus_files_have_exact_length_and_alignment() {
    let a = measure("opus_a.opus", 0.0);
    assert_eq!(a.frames, 72_000, "1.5 s of opus A");
    assert!(a.lag.abs() <= 1, "pre-skip handled: lag {}", a.lag);
    assert!(a.snr > 20.0, "SNR {}", a.snr);
    let b = measure("opus_b.opus", 1.5);
    assert_eq!(b.frames, 48_000, "1.0 s of opus B");
    assert!(b.lag.abs() <= 1, "lag {}", b.lag);
    assert!(b.snr > 20.0, "SNR {}", b.snr);
}

#[test]
fn real_libopus_pair_is_gapless_and_phase_continuous() {
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, fixture("opus_a.opus")));
    rig.player
        .set_next(PlayItem::file(2, fixture("opus_b.opus")));
    let out = rig.device.drain(RATE as usize * 10);
    assert_eq!(out.len() / 2, 120_000);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    // One continuous 440 Hz tone across the join at 1.5 s: compare against
    // the analytic signal over a window straddling it.
    let want = reference(0.0, left.len());
    let join = 72_000;
    let err = (join - 2000..join + 2000)
        .map(|n| (left[n] - want[n]).abs())
        .fold(0.0, f32::max);
    assert!(err < 0.03, "deviation across the join {err}");
}

#[test]
fn real_lossless_files_decode_exactly() {
    for name in ["tone.flac", "tone_alac.m4a"] {
        let m = measure(name, 0.0);
        // 0.5 s at 44.1 kHz, resampled to 48 kHz.
        assert_eq!(m.frames, 24_000, "{name}");
        assert_eq!(m.lag, 0, "{name}");
        assert!(m.snr > 40.0, "{name}: SNR {}", m.snr);
    }
}

#[test]
fn real_lossy_files_play_with_correct_length_and_alignment() {
    for (name, tolerance_frames) in [("tone.mp3", 1), ("tone.ogg", 1), ("tone.m4a", 2_200)] {
        let m = measure(name, 0.0);
        assert!(
            (m.frames as i64 - 48_000).abs() <= 2_200,
            "{name}: {} frames",
            m.frames
        );
        assert!(
            m.lag.abs() <= tolerance_frames,
            "{name}: lag {} frames",
            m.lag
        );
        assert!(m.snr > 15.0, "{name}: SNR {}", m.snr);
        eprintln!(
            "{name}: {} frames, lag {}, SNR {:.1} dB",
            m.frames, m.lag, m.snr
        );
    }
}
