//! Engine behaviour tests on the offline backend: position, seek, pause,
//! volume, normalization, crossfade, PCM sources, errors, resampling.

mod common;

use common::*;
use nikgob_audio::{Event, PcmError, PcmRead, PcmSource, PlayItem};
use std::time::Duration;

const RATE: u32 = 48_000;

fn wav(
    dir: &tempfile::TempDir,
    name: &str,
    rate: u32,
    start: usize,
    frames: usize,
) -> (std::path::PathBuf, Vec<f32>) {
    let data = signal(rate, start, frames);
    let p = path(dir, name);
    write_wav_f32(&p, rate, 2, &data);
    (p, data)
}

/// Is `tail` (a stretch of rendered stereo audio ending at `end_frame`
/// within a few frames) a verbatim piece of `reference`?
fn tail_matches(tail: &[f32], end_frame: usize, reference: &[f32], tol: f32) -> bool {
    let n = tail.len() / 2;
    (end_frame.saturating_sub(4)..=end_frame + 4)
        .any(|e| e >= n && max_diff(tail, &reference[(e - n) * 2..e * 2]) < tol)
}

#[test]
fn no_item_renders_silence() {
    let mut rig = rig(RATE);
    let out = rig.device.render(RATE as usize / 2);
    assert_eq!(out.len(), RATE as usize);
    assert!(out.iter().all(|&s| s == 0.0));
    assert_eq!(rig.player.position(), None);
    assert_eq!(rig.player.current_id(), None);
}

#[test]
fn position_follows_the_played_clock() {
    let dir = tmp();
    let (p, _) = wav(&dir, "a.wav", RATE, 0, RATE as usize * 5);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(9, &p));
    // Reported immediately, before the audio thread has run.
    assert_eq!(rig.player.position(), Some(Duration::ZERO));
    assert_eq!(rig.player.current_id(), Some(9));
    rig.device.render(RATE as usize);
    let pos = rig.player.position().unwrap().as_secs_f64();
    assert!((pos - 1.0).abs() < 0.002, "position {pos}");
    rig.device.render(RATE as usize / 2);
    let pos = rig.player.position().unwrap().as_secs_f64();
    assert!((pos - 1.5).abs() < 0.002, "position {pos}");
}

#[test]
fn started_event_carries_the_duration() {
    let dir = tmp();
    let (p, _) = wav(&dir, "a.wav", RATE, 0, RATE as usize * 2);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(5, &p));
    rig.device.render(2048);
    let ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Started { .. })
    });
    match ev.last().unwrap() {
        Event::Started {
            id: 5,
            duration: Some(d),
        } => assert!((d.as_secs_f64() - 2.0).abs() < 1e-6, "{d:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn seek_is_sample_accurate_and_position_reports_the_target() {
    let dir = tmp();
    let total = RATE as usize * 6;
    let ints = to_i16(&signal(RATE, 0, total));
    let original = from_i16(&ints);
    let p = path(&dir, "a.flac");
    write_flac16(&p, RATE, 2, &ints);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(5000);
    let target = Duration::from_millis(3_250);
    rig.player.seek(target);
    assert_eq!(rig.player.position(), Some(target));
    let out = rig.device.render(RATE as usize / 2);
    let pos = rig.player.position().unwrap();
    assert!(
        pos > target && pos < target + Duration::from_millis(600),
        "{pos:?}"
    );
    let end = (pos.as_secs_f64() * f64::from(RATE)).round() as usize;
    let tail = &out[out.len() - 2 * 64..];
    assert!(
        tail_matches(tail, end, &original, 1e-6),
        "audio after the seek is not the file's audio at the reported position"
    );
    // Back to a second in.
    rig.player.seek(Duration::from_secs(1));
    rig.device.render(RATE as usize / 4);
    let pos = rig.player.position().unwrap();
    assert!(
        pos >= Duration::from_secs(1) && pos < Duration::from_millis(1400),
        "{pos:?}"
    );
}

#[test]
fn opus_seek_matches_linear_decoding() {
    let dir = tmp();
    let total = RATE as usize * 6;
    let original = signal(RATE, 0, total);
    let p = path(&dir, "a.opus");
    write_opus(&p, &original, 0);

    let mut lin = rig(RATE);
    lin.player.play(PlayItem::file(1, &p));
    let full = lin.device.drain(RATE as usize * 30);
    assert_eq!(full.len() / 2, total);

    let mut seeker = rig(RATE);
    seeker.player.play(PlayItem::file(1, &p));
    seeker.device.render(1000);
    seeker.player.seek(Duration::from_millis(4_000));
    let out = seeker.device.render(RATE as usize / 2);
    let pos = seeker.player.position().unwrap();
    let end = (pos.as_secs_f64() * f64::from(RATE)).round() as usize;
    let tail = &out[out.len() - 2 * 256..];
    assert!(
        tail_matches(tail, end, &full, 2e-3),
        "opus seek with pre-roll diverges from linear decoding (pos {pos:?})"
    );
}

#[test]
fn seek_to_the_start_and_past_the_end() {
    let dir = tmp();
    let (p, data) = wav(&dir, "a.wav", RATE, 0, RATE as usize * 2);
    let (q, _) = wav(&dir, "b.wav", RATE, 99, RATE as usize);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.player.set_next(PlayItem::file(2, &q));
    rig.device.render(RATE as usize / 2);
    rig.player.seek(Duration::ZERO);
    let out = rig.device.render(RATE as usize / 10);
    // After the ramps the start of the file is back.
    let k = 2000;
    let window = &out[k * 2..(k + 100) * 2];
    assert!(
        (0..1000).any(|s| max_diff(window, &data[(k - s) * 2..(k - s + 100) * 2]) < 1e-6),
        "the file restarts from its first frame"
    );
    // Past the end: continues with the next item.
    rig.player.seek(Duration::from_secs(60));
    rig.device.render(RATE as usize / 5);
    let ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::TrackChanged { .. })
    });
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::TrackChanged { from: 1, to: 2, .. })),
        "{ev:?}"
    );
}

#[test]
fn pause_stops_callbacks_and_resume_continues_from_the_same_sample() {
    let dir = tmp();
    let (p, data) = wav(&dir, "a.wav", RATE, 0, RATE as usize * 3);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(RATE as usize / 2);
    rig.player.pause();
    for _ in 0..200 {
        if rig.player.is_paused() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(rig.player.is_paused());
    // The pause fade runs in the callback, then the device is paused.
    let mut faded = rig.device.render(2048);
    for _ in 0..200 {
        if rig.device.is_device_paused() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
        faded.extend(rig.device.render(256));
    }
    assert!(
        rig.device.is_device_paused(),
        "the device must really pause"
    );
    let tail = &faded[faded.len() - 64..];
    assert!(
        tail.iter().all(|&s| s == 0.0),
        "the fade must end in silence"
    );
    assert!(
        rig.device.render(1000).is_empty(),
        "a paused device asks for nothing"
    );
    let frozen = rig.player.position().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(rig.player.position().unwrap(), frozen);

    rig.player.resume();
    for _ in 0..200 {
        if !rig.player.is_paused() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!rig.player.is_paused());
    let after = rig.device.render(RATE as usize / 2);
    assert!(!after.is_empty());
    assert!(rig.player.position().unwrap() > frozen);
    // Past the fade-in, the audio is a verbatim continuation of the file.
    let probe = 2000;
    let window = &after[probe * 2..(probe + 64) * 2];
    let near = (frozen.as_secs_f64() * 48_000.0) as usize;
    let found = (near.saturating_sub(500)..near + 4000)
        .any(|i| max_diff(window, &data[i * 2..(i + 64) * 2]) < 1e-6);
    assert!(found, "audio after resume is not a contiguous continuation");
    let ev: Vec<_> = rig.events.try_iter().collect();
    assert!(
        ev.contains(&Event::Paused) && ev.contains(&Event::Resumed),
        "{ev:?}"
    );
}

#[test]
fn volume_uses_a_cubic_curve_without_steps() {
    let dir = tmp();
    let data = vec![0.5f32; RATE as usize * 4];
    let p = path(&dir, "c.wav");
    write_wav_f32(&p, RATE, 2, &data);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(RATE as usize / 4);
    rig.player.set_volume(0.5);
    let out = rig.device.render(RATE as usize / 2);
    let max_step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.02, "zipper step {max_step}");
    let settled = out[out.len() - 100];
    assert!((settled - 0.5 * 0.125).abs() < 1e-3, "{settled}");
    assert_eq!(rig.player.volume(), 0.5);
}

#[test]
fn a_fade_glides_to_its_level_and_leaves_the_volume_alone() {
    let dir = tmp();
    let data = vec![0.5f32; RATE as usize * 8];
    let p = path(&dir, "f.wav");
    write_wav_f32(&p, RATE, 2, &data);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(RATE as usize / 4);
    // To silence over a second: halfway down after half of it, no steps.
    rig.player.set_fade(0.0, Duration::from_secs(1));
    let half = rig.device.render(RATE as usize / 2);
    let mid = half[half.len() - 2];
    assert!((mid - 0.5 * 0.125).abs() < 0.01, "halfway {mid}");
    let rest = rig.device.render(RATE as usize / 2 + 480);
    let max_step = half
        .iter()
        .chain(&rest)
        .collect::<Vec<_>>()
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max);
    assert!(max_step < 0.01, "zipper step {max_step}");
    assert!(rest[rest.len() - 2].abs() < 1e-4, "{}", rest[rest.len() - 2]);
    assert_eq!(rig.player.volume(), 1.0);
    // Back at once: the volume as it was.
    rig.player.set_fade(1.0, Duration::ZERO);
    let out = rig.device.render(RATE as usize / 4);
    assert!((out[out.len() - 2] - 0.5).abs() < 1e-3, "{}", out[out.len() - 2]);
}

#[test]
fn normalization_applies_item_gain_only_when_enabled() {
    let dir = tmp();
    // 4 s of stereo.
    let data = vec![0.4f32; RATE as usize * 8];
    let p = path(&dir, "c.wav");
    write_wav_f32(&p, RATE, 2, &data);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p).with_gain_db(-6.0206));
    let out = rig.device.render(RATE as usize / 2);
    assert!(
        (out[out.len() - 10] - 0.2).abs() < 1e-3,
        "{}",
        out[out.len() - 10]
    );
    rig.player.set_normalization(false);
    // Audible after the already-buffered ~1 s.
    rig.device.render(RATE as usize * 3 / 2);
    let out = rig.device.render(RATE as usize / 2);
    assert!(
        (out[out.len() - 10] - 0.4).abs() < 1e-3,
        "{}",
        out[out.len() - 10]
    );
}

#[test]
fn soft_limiter_keeps_hot_signals_in_range() {
    let dir = tmp();
    let data = vec![1.6f32; RATE as usize * 2];
    let p = path(&dir, "hot.wav");
    write_wav_f32(&p, RATE, 2, &data);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    let out = rig.device.render(RATE as usize / 2);
    assert!(out.iter().all(|s| s.abs() <= 1.0));
    assert!(out[out.len() - 1] > 0.95);
}

#[test]
fn crossfade_overlaps_the_tail_and_the_head() {
    let dir = tmp();
    let a = vec![0.5f32; RATE as usize * 3 * 2];
    let b = vec![-0.5f32; RATE as usize * 3 * 2];
    let (pa, pb) = (path(&dir, "a.wav"), path(&dir, "b.wav"));
    write_wav_f32(&pa, RATE, 2, &a);
    write_wav_f32(&pb, RATE, 2, &b);
    let mut rig = rig(RATE);
    rig.player.set_crossfade(Some(Duration::from_secs(1)));
    rig.player.play(PlayItem::file(1, &pa));
    rig.player.set_next(PlayItem::file(2, &pb));
    let out = rig.device.drain(RATE as usize * 30);
    // 3 s + 3 s - 1 s overlap.
    assert_eq!(out.len() / 2, RATE as usize * 5);
    let at = |s: f64| out[(s * f64::from(RATE)) as usize * 2];
    assert!((at(1.0) - 0.5).abs() < 1e-3, "before the fade: {}", at(1.0));
    assert!((at(4.5) + 0.5).abs() < 1e-3, "after the fade: {}", at(4.5));
    // Equal power at the midpoint of the fade (2.5 s).
    let mid = at(2.5);
    let want = 0.5 * (std::f32::consts::FRAC_PI_4.cos() - std::f32::consts::FRAC_PI_4.sin());
    assert!((mid - want).abs() < 2e-3, "{mid} vs {want}");
    let ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        *e == Event::QueueEmpty
    });
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::TrackChanged { from: 1, to: 2, .. })),
        "{ev:?}"
    );
}

#[test]
fn equal_power_crossfade_of_uncorrelated_signals_keeps_level() {
    let dir = tmp();
    let a = signal(RATE, 0, RATE as usize * 4);
    let b: Vec<f32> = (0..RATE as usize * 4)
        .flat_map(|i| {
            let t = i as f64 / 48_000.0;
            [
                (0.3 * (std::f64::consts::TAU * 523.25 * t).sin()
                    + 0.2 * (std::f64::consts::TAU * 2201.0 * t).sin()) as f32,
                (0.25 * (std::f64::consts::TAU * 777.0 * t).sin()
                    + 0.15 * (std::f64::consts::TAU * 5100.0 * t).sin()) as f32,
            ]
        })
        .collect();
    let (pa, pb) = (path(&dir, "a.wav"), path(&dir, "b.wav"));
    write_wav_f32(&pa, RATE, 2, &a);
    write_wav_f32(&pb, RATE, 2, &b);
    let mut rig = rig(RATE);
    rig.player.set_crossfade(Some(Duration::from_secs(2)));
    rig.player.play(PlayItem::file(1, &pa));
    rig.player.set_next(PlayItem::file(2, &pb));
    let out = rig.device.drain(RATE as usize * 30);
    let level = |from: f64, to: f64| {
        rms(&out[(from * 48_000.0) as usize * 2..(to * 48_000.0) as usize * 2])
    };
    let before = level(0.5, 1.5);
    let during = level(2.5, 3.5);
    assert!((during / before - 1.0).abs() < 0.25, "{before} vs {during}");
}

struct Sine {
    phase: f64,
    left: usize,
}

impl PcmSource for Sine {
    fn sample_rate(&self) -> u32 {
        44_100
    }
    fn channels(&self) -> u16 {
        1
    }
    fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
        if self.left == 0 {
            return Ok(PcmRead::End);
        }
        let n = out.len().min(self.left);
        for s in &mut out[..n] {
            *s = (0.5 * self.phase.sin()) as f32;
            self.phase += std::f64::consts::TAU * 1000.0 / 44_100.0;
        }
        self.left -= n;
        Ok(PcmRead::Frames(n))
    }
}

/// A 440 Hz stereo sine at the device rate whose reads hang once, for
/// `stall`, after `stall_at` frames: a disk or CPU holding the decoder up.
struct StallingSine {
    frame: usize,
    stall_at: usize,
    stall: Option<Duration>,
}

impl PcmSource for StallingSine {
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn channels(&self) -> u16 {
        2
    }
    fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
        if self.frame >= self.stall_at
            && let Some(stall) = self.stall.take()
        {
            std::thread::sleep(stall);
        }
        let frames = out.len() / 2;
        for f in out.as_chunks_mut::<2>().0 {
            let s = (0.5
                * (std::f64::consts::TAU * 440.0 * self.frame as f64 / f64::from(RATE)).sin())
                as f32;
            f[0] = s;
            f[1] = s;
            self.frame += 1;
        }
        Ok(PcmRead::Frames(frames))
    }
}

#[test]
fn a_stalled_decoder_is_a_counted_dropout_without_clicks() {
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::pcm(
        4,
        Box::new(StallingSine {
            frame: 0,
            stall_at: RATE as usize,
            stall: Some(Duration::from_millis(1500)),
        }),
        None,
    ));
    // The decoder fills its second ahead and hangs; half of it is pulled.
    let mut heard = rig.device.render(RATE as usize / 2);
    assert_eq!(rig.player.underruns(), 0);
    // Then in real time, like a sound card: the ring runs dry half a second
    // later and plays again once the source answers.
    let block = 256;
    let began = std::time::Instant::now();
    let mut pulled = 0usize;
    while pulled < RATE as usize * 5 / 2 {
        heard.extend(rig.device.render_now(block));
        pulled += block;
        let due = Duration::from_secs_f64(pulled as f64 / f64::from(RATE));
        if let Some(wait) = due.checked_sub(began.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    let dropouts = rig.player.underruns();
    assert!(
        dropouts > 10,
        "{dropouts} underruns for a second of silence"
    );
    let left: Vec<f32> = heard.iter().step_by(2).copied().collect();
    let silent = left.iter().filter(|s| **s == 0.0).count();
    assert!(silent > RATE as usize / 2, "{silent} silent frames");
    assert!(
        left[left.len() - 1000..].iter().any(|s| s.abs() > 0.3),
        "it plays again"
    );
    // A 440 Hz sine at 0.5 moves at most 0.03 a sample; a hard cut to or
    // from silence jumps up to 0.5.
    let step = left
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(step < 0.1, "a click of {step}");
}

#[test]
fn pcm_source_is_resampled_and_upmixed() {
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::pcm(
        3,
        Box::new(Sine {
            phase: 0.0,
            left: 44_100 * 2,
        }),
        Some(Duration::from_secs(2)),
    ));
    let out = rig.device.drain(RATE as usize * 10);
    assert_eq!(out.len() / 2, RATE as usize * 2);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let r: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
    assert!(
        max_diff(&l, &r) < 1e-6,
        "mono must be upmixed to both channels"
    );
    let mid = &l[RATE as usize / 2..RATE as usize / 2 + RATE as usize];
    let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    assert!(
        (crossings as i64 - 1000).abs() <= 1,
        "{crossings} cycles in one second"
    );
}

#[test]
fn file_at_another_rate_keeps_its_frequency() {
    let dir = tmp();
    let n = 44_100 * 3;
    let tone: Vec<f32> = (0..n)
        .flat_map(|i| {
            let s = (0.5 * (std::f64::consts::TAU * 1000.0 * i as f64 / 44_100.0).sin()) as f32;
            [s, s]
        })
        .collect();
    let p = path(&dir, "t.wav");
    write_wav_f32(&p, 44_100, 2, &tone);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    let out = rig.device.drain(RATE as usize * 10);
    assert_eq!(out.len() / 2, RATE as usize * 3);
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let mid = &l[RATE as usize..RATE as usize * 2];
    let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    assert!((crossings as i64 - 1000).abs() <= 1, "{crossings}");
}

#[test]
fn missing_file_is_an_error_event_and_playback_continues() {
    let dir = tmp();
    let (p, _) = wav(&dir, "a.wav", RATE, 0, RATE as usize * 2);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(1000);
    rig.player
        .play(PlayItem::file(2, dir.path().join("nope.wav")));
    let ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Error { .. })
    });
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::Error { id: Some(2), .. })),
        "{ev:?}"
    );
    rig.device.render(1000);
    assert_eq!(rig.player.current_id(), Some(1));
}

#[test]
fn undecodable_next_is_reported_and_current_item_ends_normally() {
    let dir = tmp();
    let (p, _) = wav(&dir, "a.wav", RATE, 0, RATE as usize);
    let junk = path(&dir, "junk.flac");
    std::fs::write(&junk, vec![0x42u8; 4096]).unwrap();
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.player.set_next(PlayItem::file(2, &junk));
    let out = rig.device.drain(RATE as usize * 10);
    assert_eq!(out.len() / 2, RATE as usize);
    let ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        *e == Event::QueueEmpty
    });
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::Error { id: Some(2), .. })),
        "{ev:?}"
    );
}

#[test]
fn stop_silences_and_clears() {
    let dir = tmp();
    let (p, _) = wav(&dir, "a.wav", RATE, 0, RATE as usize * 3);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(5000);
    rig.player.stop();
    rig.device.render(2000);
    let out = rig.device.render(2000);
    assert!(out.iter().all(|&s| s == 0.0));
    assert_eq!(rig.player.position(), None);
    let _ = wait_event(&rig.events, Duration::from_secs(1), |e| {
        *e == Event::Stopped
    });
}

#[test]
fn play_over_a_playing_item_reports_a_track_change() {
    let dir = tmp();
    let (p, _) = wav(&dir, "a.wav", RATE, 0, RATE as usize * 3);
    let (q, data) = wav(&dir, "b.wav", RATE, 5, RATE as usize * 3);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(5000);
    rig.player.play(PlayItem::file(2, &q));
    let out = rig.device.render(5000);
    let hit = (0..2000)
        .any(|k| max_diff(&out[(k + 600) * 2..(k + 664) * 2], &data[600 * 2..664 * 2]) < 1e-6);
    assert!(hit, "the new item must start from its first frame");
    let ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::TrackChanged { .. })
    });
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::TrackChanged { from: 1, to: 2, .. })),
        "{ev:?}"
    );
}

#[test]
fn opus_output_gain_header_is_applied() {
    let dir = tmp();
    let data: Vec<f32> = (0..RATE as usize * 2)
        .flat_map(|i| {
            let s = (0.2 * (std::f64::consts::TAU * 440.0 * i as f64 / 48_000.0).sin()) as f32;
            [s, s]
        })
        .collect();
    let (a, b) = (path(&dir, "g0.opus"), path(&dir, "g6.opus"));
    write_opus(&a, &data, 0);
    write_opus(&b, &data, 6 * 256);
    let mut r0 = rig(RATE);
    r0.player.play(PlayItem::file(1, &a));
    let o0 = r0.device.drain(RATE as usize * 10);
    let mut r6 = rig(RATE);
    r6.player.play(PlayItem::file(1, &b));
    let o6 = r6.device.drain(RATE as usize * 10);
    let ratio = rms(&o6[RATE as usize..]) / rms(&o0[RATE as usize..]);
    assert!((ratio - 1.995).abs() < 0.03, "ratio {ratio}");
}

#[test]
fn visualizer_tap_sees_the_music_before_the_volume() {
    let dir = tmp();
    let n = RATE as usize * 2;
    let tone: Vec<f32> = (0..n)
        .flat_map(|i| {
            let s = (0.5 * (std::f64::consts::TAU * 60.0 * i as f64 / 48_000.0).sin()) as f32;
            [s, s]
        })
        .collect();
    let p = path(&dir, "bass.wav");
    write_wav_f32(&p, RATE, 2, &tone);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    let mut analyzer = rig.player.analyzer();
    for _ in 0..40 {
        rig.device.render(768);
        analyzer.update_with_dt(Duration::from_millis(16));
    }
    assert!(analyzer.bass_energy() > 0.7, "{}", analyzer.bass_energy());
    rig.player.set_volume(0.0);
    for _ in 0..20 {
        rig.device.render(768);
        analyzer.update_with_dt(Duration::from_millis(16));
    }
    assert!(analyzer.bass_energy() > 0.7);
}

#[test]
fn device_change_mid_playback_carries_on_from_the_same_position() {
    let dir = tmp();
    let n = RATE as usize * 8;
    let tone: Vec<f32> = (0..n)
        .flat_map(|i| {
            let s = (0.5 * (std::f64::consts::TAU * 1000.0 * i as f64 / 48_000.0).sin()) as f32;
            [s, s]
        })
        .collect();
    let p = path(&dir, "t.wav");
    write_wav_f32(&p, RATE, 2, &tone);
    let mut rig = rig(RATE);
    rig.player.play(PlayItem::file(1, &p));
    rig.device.render(RATE as usize); // 1 s at 48 kHz stereo
    // Headphones: 44.1 kHz.
    rig.device.reconfigure(44_100, 2);
    let out = rig.device.render(44_100); // 1 s more
    let pos = rig.player.position().unwrap().as_secs_f64();
    assert!((pos - 2.0).abs() < 0.03, "position after the switch {pos}");
    let l: Vec<f32> = out.iter().step_by(2).copied().collect();
    let settled = &l[l.len() / 2..];
    let crossings = settled
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    let want = 1000.0 * settled.len() as f64 / 44_100.0;
    assert!(
        (crossings as f64 - want).abs() <= 1.5,
        "{crossings} vs {want}"
    );
    // A mono 96 kHz device next.
    rig.device.reconfigure(96_000, 1);
    let out = rig.device.render(96_000);
    let pos = rig.player.position().unwrap().as_secs_f64();
    assert!(
        (pos - 3.0).abs() < 0.03,
        "position after the second switch {pos}"
    );
    let settled = &out[out.len() / 2..];
    let crossings = settled
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    let want = 1000.0 * settled.len() as f64 / 96_000.0;
    assert!(
        (crossings as f64 - want).abs() <= 1.5,
        "{crossings} vs {want}"
    );
    // No spurious start/track events from the reconfiguration. The engine
    // can render all of this before the control thread has passed on even
    // the `Started`, so wait for that, then give stray ones a moment.
    let mut ev = wait_event(&rig.events, Duration::from_secs(2), |e| {
        matches!(e, Event::Started { .. })
    });
    std::thread::sleep(Duration::from_millis(100));
    ev.extend(rig.events.try_iter());
    ev.retain(|e| !matches!(e, Event::Position { .. }));
    assert_eq!(ev.len(), 1, "{ev:?}");
}
