//! `ItemSource::Stream`: encoded bytes from an in-memory reader, opened off
//! the caller's thread, decoded like files (WAV, Ogg Vorbis, M4A/AAC,
//! WebM/Opus), with seeking, gapless hand-over, stream-supplied gain and
//! failures reported as `Event::Error`.

mod common;

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::*;
use nikgob_audio::{ByteSource, Event, PlayItem, StreamOpener};

const RATE: u32 = 48_000;
/// Frames of the de-click ramp at a start, left out of exact comparisons.
const SKIP: usize = 1000;

fn fixture(name: &str) -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read(p).unwrap()
}

/// Bytes in memory, as a streaming service would hand them out.
struct Memory {
    bytes: Cursor<Vec<u8>>,
    hint: &'static str,
    gain_db: Option<f32>,
    fail_reads: bool,
}

impl Memory {
    fn new(bytes: Vec<u8>, hint: &'static str) -> Self {
        Self {
            bytes: Cursor::new(bytes),
            hint,
            gain_db: None,
            fail_reads: false,
        }
    }
}

impl Read for Memory {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.fail_reads {
            return Err(std::io::Error::other("connection reset"));
        }
        self.bytes.read(buf)
    }
}

impl Seek for Memory {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.bytes.seek(pos)
    }
}

impl ByteSource for Memory {
    fn byte_len(&self) -> Option<u64> {
        Some(self.bytes.get_ref().len() as u64)
    }

    fn format_hint(&self) -> &str {
        self.hint
    }

    fn gain_db(&self) -> Option<f32> {
        self.gain_db
    }
}

fn opener(m: Memory) -> StreamOpener {
    Box::new(move || Ok(Box::new(m) as Box<dyn ByteSource>))
}

fn stream(id: u64, m: Memory) -> PlayItem {
    PlayItem::stream(id, opener(m))
}

/// 16-bit stereo WAV of [`signal`] and the exact samples it holds.
fn wav16(start: usize, frames: usize) -> (Vec<u8>, Vec<f32>) {
    let ints = to_i16(&signal(RATE, start, frames));
    let mut out = Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::new(&mut out, spec).unwrap();
        for &s in &ints {
            w.write_sample(s as i16).unwrap();
        }
        w.finalize().unwrap();
    }
    (out.into_inner(), from_i16(&ints))
}

fn tail_matches(tail: &[f32], end_frame: usize, reference: &[f32], tol: f32) -> bool {
    let n = tail.len() / 2;
    (end_frame.saturating_sub(4)..=end_frame + 4).any(|e| {
        e >= n && e * 2 <= reference.len() && max_diff(tail, &reference[(e - n) * 2..e * 2]) < tol
    })
}

/// The fixtures' left channel: 0.3 * sin(2 pi 440 t).
fn tone(t: f64) -> f32 {
    (0.3 * (std::f64::consts::TAU * 440.0 * t).sin()) as f32
}

/// Lag (device frames) and SNR of `left` against the tone, where `left[n]`
/// should be the tone at `t0 + n / RATE`.
fn lag_and_snr(left: &[f32], t0: f64) -> (i64, f32) {
    let window = 6000.min(left.len() / 2);
    let start = (left.len() - window) / 2;
    let err_at = |lag: i64| -> (f32, f32) {
        (start..start + window).fold((0.0, 0.0), |(e, s), n| {
            let r = tone((n as f64 - lag as f64) / f64::from(RATE) + t0);
            (e + (left[n] - r) * (left[n] - r), s + r * r)
        })
    };
    let best = (-54i64..=54)
        .min_by(|&a, &b| err_at(a).0.total_cmp(&err_at(b).0))
        .unwrap();
    let (err, sig) = err_at(best);
    (best, 10.0 * (sig / err.max(1e-20)).log10())
}

#[test]
fn wav_stream_decodes_exactly_and_seeks() {
    let total = RATE as usize * 3;
    let (bytes, original) = wav16(0, total);

    let mut rig = rig(RATE);
    rig.player
        .play(stream(1, Memory::new(bytes.clone(), "wav")));
    let full = rig.device.drain(RATE as usize * 10);
    assert_eq!(full.len(), original.len());
    assert!(max_diff(&full[SKIP * 2..], &original[SKIP * 2..]) < 1e-6);
    let seen = wait_event(&rig.events, Duration::from_secs(1), |e| {
        matches!(e, Event::Ended { id: 1 })
    });
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::Started { id: 1, .. })),
        "{seen:?}"
    );

    let mut rig = common::rig(RATE);
    rig.player.play(stream(1, Memory::new(bytes, "wav")));
    rig.device.render(4000);
    let target = Duration::from_millis(2_100);
    rig.player.seek(target);
    let out = rig.device.render(RATE as usize / 4);
    let pos = rig.player.position().unwrap();
    assert!(
        pos > target && pos < target + Duration::from_millis(400),
        "{pos:?}"
    );
    let end = (pos.as_secs_f64() * f64::from(RATE)).round() as usize;
    assert!(
        tail_matches(&out[out.len() - 128..], end, &original, 1e-6),
        "audio after the seek is not the stream's audio at the reported position"
    );
}

#[test]
fn hint_free_stream_is_probed() {
    let (bytes, original) = wav16(0, RATE as usize / 2);
    let mut rig = rig(RATE);
    rig.player.play(stream(1, Memory::new(bytes, "")));
    let out = rig.device.drain(RATE as usize * 5);
    assert_eq!(out.len(), original.len());
}

#[test]
fn ogg_vorbis_m4a_and_webm_opus_streams_play() {
    // (fixture, hint, length tolerance in frames, lag tolerance)
    for (name, hint, len_tol, lag_tol) in [
        ("tone.ogg", "ogg", 1, 1),
        ("tone.m4a", "m4a", 2_200, 2_200),
        // WebM: DiscardPadding is not applied (a few ms too long).
        ("tone.webm", "webm", 1_000, 1),
    ] {
        let mut rig = rig(RATE);
        rig.player.play(stream(7, Memory::new(fixture(name), hint)));
        let out = rig.device.drain(RATE as usize * 10);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        let frames = left.len() as i64;
        assert!(
            (frames - 48_000).abs() <= len_tol,
            "{name}: {frames} frames"
        );
        let (lag, snr) = lag_and_snr(&left, 0.0);
        assert!(lag.abs() <= lag_tol, "{name}: lag {lag}");
        assert!(snr > 15.0, "{name}: SNR {snr:.1} dB");
        eprintln!("{name} as a stream: {frames} frames, lag {lag}, SNR {snr:.1} dB");
    }
}

#[test]
fn webm_opus_stream_seeks() {
    let mut rig = rig(RATE);
    rig.player
        .play(stream(1, Memory::new(fixture("tone.webm"), "webm")));
    rig.device.render(2000);
    let target = Duration::from_millis(500);
    rig.player.seek(target);
    let out = rig.device.render(RATE as usize / 4);
    let pos = rig.player.position().unwrap();
    assert!(
        pos > target && pos < target + Duration::from_millis(400),
        "{pos:?}"
    );
    // Compare what was rendered right after the seek with the tone at the
    // target: Matroska timestamps are whole milliseconds, so allow 1 ms.
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let (lag, snr) = lag_and_snr(&left[..9000], target.as_secs_f64());
    eprintln!("webm after the seek: lag {lag}, SNR {snr:.1} dB");
    assert!(lag.abs() <= 48, "lag {lag} frames after the seek");
    assert!(snr > 15.0, "SNR {snr:.1} dB after the seek");
}

#[test]
fn stream_next_is_gapless() {
    let n = RATE as usize;
    let (a, wa) = wav16(0, n);
    let (b, wb) = wav16(n, n);
    let mut rig = rig(RATE);
    rig.player.play(stream(1, Memory::new(a, "wav")));
    rig.player.set_next(stream(2, Memory::new(b, "wav")));
    let out = rig.device.drain(RATE as usize * 10);
    let want: Vec<f32> = wa.into_iter().chain(wb).collect();
    assert_eq!(out.len(), want.len());
    assert!(
        max_diff(&out[SKIP * 2..], &want[SKIP * 2..]) < 1e-6,
        "join is not sample exact"
    );
    let seen = wait_event(&rig.events, Duration::from_secs(1), |e| {
        matches!(e, Event::TrackChanged { .. })
    });
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::TrackChanged { from: 1, to: 2, .. })),
        "{seen:?}"
    );
}

#[test]
fn slow_next_stream_is_waited_for() {
    let n = RATE as usize / 2;
    let (a, wa) = wav16(0, n);
    let (b, wb) = wav16(n, n);
    let release = Arc::new(AtomicBool::new(false));
    let gate = Arc::clone(&release);
    let slow: StreamOpener = Box::new(move || {
        while !gate.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(Box::new(Memory::new(b, "wav")) as Box<dyn ByteSource>)
    });
    let mut rig = rig(RATE);
    rig.player.play(stream(1, Memory::new(a, "wav")));
    rig.player.set_next(PlayItem::stream(2, slow));
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        release.store(true, Ordering::Release);
    });
    let out = rig.device.drain(RATE as usize * 10);
    let want: Vec<f32> = wa.into_iter().chain(wb).collect();
    assert_eq!(
        out.len(),
        want.len(),
        "the first item did not end into nothing"
    );
    assert!(max_diff(&out[SKIP * 2..], &want[SKIP * 2..]) < 1e-6);
}

#[test]
fn stream_gain_replaces_the_item_gain() {
    let (bytes, original) = wav16(0, RATE as usize / 2);
    let mut m = Memory::new(bytes, "wav");
    m.gain_db = Some(-6.0);
    let mut rig = rig(RATE);
    rig.player.set_normalization(true);
    rig.player.play(stream(1, m).with_gain_db(6.0));
    let out = rig.device.drain(RATE as usize * 5);
    let ratio = rms(&out) / rms(&original);
    assert!((ratio - 0.501).abs() < 0.01, "gain ratio {ratio}");
}

#[test]
fn failures_are_error_events() {
    let rig = rig(RATE);
    rig.player.play(PlayItem::stream(
        5,
        Box::new(|| Err("not logged in".into())),
    ));
    let seen = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Error { .. })
    });
    let msg = seen.iter().find_map(|e| match e {
        Event::Error {
            id: Some(5),
            message,
        } => Some(message.clone()),
        _ => None,
    });
    assert!(
        msg.as_deref().is_some_and(|m| m.contains("not logged in")),
        "{seen:?}"
    );
    assert_eq!(rig.player.position(), None);

    let mut m = Memory::new(fixture("tone.ogg"), "ogg");
    m.fail_reads = true;
    rig.player.play(stream(6, m));
    let seen = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Error { .. })
    });
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::Error { id: Some(6), .. })),
        "{seen:?}"
    );
}

#[test]
fn stream_play_cuts_the_current_file_at_once() {
    let dir = tmp();
    let p = path(&dir, "a.wav");
    write_wav_f32(&p, RATE, 2, &signal(RATE, 0, RATE as usize * 3));
    let (bytes, original) = wav16(0, RATE as usize / 2);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(4000);
    rig.player.play(stream(2, Memory::new(bytes, "wav")));
    let out = rig.device.drain(RATE as usize * 10);
    // A de-click fade of the old item, then exactly the stream.
    assert!(out.len() / 2 < RATE as usize, "{} frames", out.len() / 2);
    let tail = &out[out.len() - original.len() / 2..];
    assert!(max_diff(tail, &original[original.len() / 2..]) < 1e-6);
    let seen = wait_event(&rig.events, Duration::from_secs(1), |e| {
        matches!(e, Event::Ended { id: 2 })
    });
    assert!(
        !seen.iter().any(|e| matches!(e, Event::Ended { id: 1 })),
        "{seen:?}"
    );
}

#[test]
fn a_seek_while_the_stream_opens_applies_once_it_is_open() {
    let total = RATE as usize * 3;
    let (bytes, original) = wav16(0, total);
    let release = Arc::new(AtomicBool::new(false));
    let gate = Arc::clone(&release);
    let slow: StreamOpener = Box::new(move || {
        while !gate.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(Box::new(Memory::new(bytes, "wav")) as Box<dyn ByteSource>)
    });
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::stream(1, slow));
    assert_eq!(
        rig.player.current_id(),
        Some(1),
        "the requested item, at its start"
    );
    assert_eq!(rig.player.position(), Some(Duration::ZERO));
    let target = Duration::from_secs(2);
    rig.player.seek(target);
    assert_eq!(rig.player.position(), Some(target));
    release.store(true, Ordering::Release);
    let out = rig.device.render(RATE as usize / 4);
    let pos = rig.player.position().unwrap();
    assert!(
        pos > target && pos < target + Duration::from_millis(400),
        "{pos:?}"
    );
    let end = (pos.as_secs_f64() * f64::from(RATE)).round() as usize;
    assert!(tail_matches(&out[out.len() - 128..], end, &original, 1e-6));
}

/// A source that failed and is played again under the same id (the next
/// source of one queue entry: Spotify refused, YouTube Music plays), and an
/// item played again over itself (restart), are announced as started: the
/// runtime waits for that to leave Buffering.
#[test]
fn replaying_the_same_id_is_announced() {
    let mut rig = rig(RATE);
    let mut m = Memory::new(fixture("tone.ogg"), "ogg");
    m.fail_reads = true;
    rig.player.play(stream(7, m));
    let seen = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Error { .. })
    });
    assert!(seen.iter().any(|e| matches!(e, Event::Error { id: Some(7), .. })), "{seen:?}");
    let _ = rig.device.drain(RATE as usize / 10);

    let (bytes, _) = wav16(0, RATE as usize * 3);
    rig.player.play(stream(7, Memory::new(bytes.clone(), "wav")));
    let _ = rig.device.drain(RATE as usize / 2);
    let seen = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Started { id: 7, .. } | Event::TrackChanged { to: 7, .. })
    });
    assert!(
        seen.iter().any(|e| matches!(e, Event::Started { id: 7, .. } | Event::TrackChanged { to: 7, .. })),
        "after a failed source: {seen:?}"
    );

    rig.player.play(stream(7, Memory::new(bytes, "wav")));
    let _ = rig.device.drain(RATE as usize / 2);
    let seen = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Started { id: 7, .. } | Event::TrackChanged { to: 7, .. })
    });
    assert!(
        seen.iter().any(|e| matches!(e, Event::Started { id: 7, .. } | Event::TrackChanged { to: 7, .. })),
        "played over itself: {seen:?}"
    );
}
