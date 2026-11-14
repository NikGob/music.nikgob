//! A stream read that waits for the network forever never holds a command
//! up: stop, another item, a new next item, a seek and the engine's end
//! each take effect at once, the stalled source is let go, nothing about it
//! is reported, and opens in flight stay capped with the newest winning.

mod common;

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use common::*;
use nikgob_audio::{ByteSource, Event, Interrupter, PlayItem, StreamOpener};

const RATE: u32 = 48_000;
/// What "at once" means here (generous for a busy test machine).
const PROMPT: Duration = Duration::from_secs(2);

/// What the test sees of a [`Hang`].
#[derive(Default)]
struct Gate {
    interrupted: Mutex<bool>,
    changed: Condvar,
    /// A read is waiting now.
    waiting: AtomicBool,
    dropped: AtomicBool,
}

impl Gate {
    fn wait_until(&self, what: &str, f: impl Fn(&Self) -> bool) {
        let t = Instant::now();
        while !f(self) {
            assert!(t.elapsed() < PROMPT, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Bytes up to `limit`; a read past it waits until the engine interrupts
/// it, like a download that stalled for good.
struct Hang {
    bytes: Cursor<Vec<u8>>,
    limit: u64,
    gate: Arc<Gate>,
}

impl Read for Hang {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let pos = self.bytes.position();
        if pos >= self.limit {
            let mut on = self.gate.interrupted.lock().unwrap();
            self.gate.waiting.store(true, Ordering::SeqCst);
            while !*on {
                on = self.gate.changed.wait(on).unwrap();
            }
            self.gate.waiting.store(false, Ordering::SeqCst);
            return Err(std::io::Error::other("interrupted"));
        }
        let n = buf.len().min((self.limit - pos) as usize);
        self.bytes.read(&mut buf[..n])
    }
}

impl Seek for Hang {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.bytes.seek(pos)
    }
}

impl ByteSource for Hang {
    fn byte_len(&self) -> Option<u64> {
        Some(self.bytes.get_ref().len() as u64)
    }

    fn format_hint(&self) -> &str {
        "wav"
    }

    fn interrupter(&self) -> Option<Interrupter> {
        let gate = Arc::clone(&self.gate);
        Some(Arc::new(move |on| {
            *gate.interrupted.lock().unwrap() = on;
            gate.changed.notify_all();
        }))
    }
}

impl Drop for Hang {
    fn drop(&mut self) {
        self.gate.dropped.store(true, Ordering::SeqCst);
    }
}

/// A WAV stream of `bytes` that stalls at byte `limit`.
fn hanging(id: u64, bytes: Vec<u8>, limit: u64) -> (PlayItem, Arc<Gate>) {
    let gate = Arc::new(Gate::default());
    let g = Arc::clone(&gate);
    let open: StreamOpener = Box::new(move || {
        Ok(Box::new(Hang {
            bytes: Cursor::new(bytes),
            limit,
            gate: g,
        }) as Box<dyn ByteSource>)
    });
    (PlayItem::stream(id, open), gate)
}

/// A stream that never stalls.
fn whole(id: u64, bytes: Vec<u8>) -> PlayItem {
    hanging(id, bytes, u64::MAX).0
}

/// 16-bit stereo WAV of [`signal`] and the exact samples it holds.
fn wav16(frames: usize) -> (Vec<u8>, Vec<f32>) {
    let ints = to_i16(&signal(RATE, 0, frames));
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

/// The byte where `secs` of the WAV end (44-byte header, 4 bytes a frame).
fn at(secs: f64) -> u64 {
    44 + (secs * f64::from(RATE)) as u64 * 4
}

fn errors(events: &crossbeam_channel::Receiver<Event>) -> Vec<Event> {
    events
        .try_iter()
        .filter(|e| matches!(e, Event::Error { .. }))
        .collect()
}

#[test]
fn stop_takes_a_stalled_stream_away_at_once() {
    let (bytes, _) = wav16(RATE as usize * 3);
    let rig = rig(RATE);
    let (item, gate) = hanging(1, bytes, at(0.3));
    rig.player.play(item);
    gate.wait_until("the read stalls", |g| g.waiting.load(Ordering::SeqCst));
    let t = Instant::now();
    rig.player.stop();
    gate.wait_until("the stalled stream is let go", |g| g.dropped.load(Ordering::SeqCst));
    assert!(t.elapsed() < PROMPT);
    let seen = wait_event(&rig.events, PROMPT, |e| matches!(e, Event::Stopped));
    assert!(seen.iter().any(|e| matches!(e, Event::Stopped)), "{seen:?}");
    std::thread::sleep(Duration::from_millis(100));
    assert!(errors(&rig.events).is_empty());
}

#[test]
fn another_item_plays_over_a_stalled_stream_at_once() {
    let (bytes, _) = wav16(RATE as usize * 3);
    let mut rig = rig(RATE);
    let (item, gate) = hanging(1, bytes, at(0.3));
    rig.player.play(item);
    gate.wait_until("the read stalls", |g| g.waiting.load(Ordering::SeqCst));
    let (short, short_samples) = wav16(RATE as usize / 2);
    let t = Instant::now();
    rig.player.play(whole(2, short));
    gate.wait_until("the stalled stream is let go", |g| g.dropped.load(Ordering::SeqCst));
    let out = rig.device.drain(RATE as usize * 5);
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());
    // The new item, whole, at the end of what was heard.
    let tail = &out[out.len() - short_samples.len() / 2..];
    assert!(max_diff(tail, &short_samples[short_samples.len() / 2..]) < 1e-6);
    let seen = wait_event(&rig.events, PROMPT, |e| matches!(e, Event::Ended { id: 2 }));
    assert!(seen.iter().any(|e| matches!(e, Event::Ended { id: 2 })), "{seen:?}");
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{seen:?}");
    assert!(errors(&rig.events).is_empty());
}

#[test]
fn a_new_next_item_replaces_one_stalled_while_opening() {
    let (a, a_samples) = wav16(RATE as usize / 4);
    let (b, b_samples) = wav16(RATE as usize / 4);
    let mut rig = rig(RATE);
    rig.player.play(whole(1, a));
    // Its probe stalls at the first byte.
    let (next, gate) = hanging(2, b.clone(), 0);
    rig.player.set_next(next);
    gate.wait_until("the open stalls", |g| g.waiting.load(Ordering::SeqCst));
    let t = Instant::now();
    rig.player.set_next(whole(3, b));
    gate.wait_until("the stalled open ends", |g| g.dropped.load(Ordering::SeqCst));
    assert!(t.elapsed() < PROMPT);
    let out = rig.device.drain(RATE as usize * 5);
    // Gapless: the first item, then the new next item, nothing between.
    let frames = (a_samples.len() + b_samples.len()) / 2;
    assert!(out.len() / 2 >= frames, "{} frames", out.len() / 2);
    let tail = &out[out.len() - b_samples.len()..];
    assert!(max_diff(tail, &b_samples) < 1e-6);
    let seen = wait_event(&rig.events, PROMPT, |e| matches!(e, Event::Ended { id: 3 }));
    assert!(
        seen.iter().any(|e| matches!(e, Event::TrackChanged { from: 1, to: 3, .. })),
        "{seen:?}"
    );
    assert!(!seen.iter().any(|e| matches!(e, Event::Error { .. })), "{seen:?}");
}

#[test]
fn a_seek_in_a_stalled_stream_is_taken_at_once() {
    let (bytes, original) = wav16(RATE as usize * 3);
    let mut rig = rig(RATE);
    let (item, gate) = hanging(1, bytes, at(0.6));
    rig.player.play(item);
    gate.wait_until("the read stalls", |g| g.waiting.load(Ordering::SeqCst));
    // Back into what is there: it plays from there.
    let target = Duration::from_millis(200);
    let t = Instant::now();
    rig.seek_to(target);
    let out = rig.device.render(RATE as usize / 10);
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());
    let pos = rig.player.position().unwrap();
    assert!(
        pos > target + Duration::from_millis(90) && pos < target + Duration::from_millis(200),
        "{pos:?}"
    );
    let end = (pos.as_secs_f64() * f64::from(RATE)).round() as usize;
    let n = 128;
    let matched = (end.saturating_sub(4)..=end + 4)
        .any(|e| max_diff(&out[out.len() - n..], &original[(e - n / 2) * 2..e * 2]) < 1e-6);
    assert!(matched, "the audio after the seek is the stream's at {pos:?}");
    // It stalls again further on, and a stop still takes it away.
    gate.wait_until("the read stalls again", |g| g.waiting.load(Ordering::SeqCst));
    rig.player.stop();
    gate.wait_until("the stalled stream is let go", |g| g.dropped.load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(100));
    assert!(errors(&rig.events).is_empty());
}

#[test]
fn the_engine_ends_while_a_stream_stalls() {
    let (bytes, _) = wav16(RATE as usize * 3);
    let Rig { player, events, device } = rig(RATE);
    let (item, gate) = hanging(1, bytes, at(0.3));
    player.play(item);
    gate.wait_until("the read stalls", |g| g.waiting.load(Ordering::SeqCst));
    let t = Instant::now();
    drop(player);
    gate.wait_until("the decoder ends and lets the stream go", |g| {
        g.dropped.load(Ordering::SeqCst)
    });
    assert!(t.elapsed() < PROMPT);
    drop((events, device));
}

#[test]
fn superseded_opens_end_and_opens_in_flight_are_capped() {
    // Opens that hang where nothing can interrupt them (a service's own
    // lookup): the first three run, the ones superseded while waiting in
    // line never run, and the newest plays once a thread is free.
    let (bytes, _) = wav16(RATE as usize / 4);
    let release = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let ran = Arc::new(Mutex::new(Vec::new()));
    let rig = rig(RATE);
    let slow = |id: u64| {
        let (release, busy, most, ran, bytes) = (
            Arc::clone(&release),
            Arc::clone(&busy),
            Arc::clone(&most),
            Arc::clone(&ran),
            bytes.clone(),
        );
        let open: StreamOpener = Box::new(move || {
            ran.lock().unwrap().push(id);
            let now = busy.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            while !release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            busy.fetch_sub(1, Ordering::SeqCst);
            Ok(Box::new(Cursorish(Cursor::new(bytes))) as Box<dyn ByteSource>)
        });
        PlayItem::stream(id, open)
    };
    // Each of the first three is let start before the next one comes
    // (opens superseded before a thread took them never run at all).
    for id in 1..=3 {
        rig.player.play(slow(id));
        let t = Instant::now();
        while busy.load(Ordering::SeqCst) < id as usize {
            assert!(t.elapsed() < PROMPT, "open {id} runs");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    for id in 4..=7 {
        rig.player.play(slow(id));
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(most.load(Ordering::SeqCst), 3, "capped");
    release.store(true, Ordering::SeqCst);
    let mut runs = ran.lock().unwrap().clone();
    let t = Instant::now();
    while !runs.contains(&7) {
        assert!(t.elapsed() < PROMPT, "the newest opens: {runs:?}");
        std::thread::sleep(Duration::from_millis(5));
        runs = ran.lock().unwrap().clone();
    }
    runs.sort_unstable();
    assert_eq!(runs, [1, 2, 3, 7], "the ones superseded in line never ran");
    assert_eq!(most.load(Ordering::SeqCst), 3);

    // Opens whose probe stalls end as soon as they are superseded.
    let (wav, _) = wav16(RATE as usize);
    let (first, a) = hanging(10, wav.clone(), 0);
    rig.player.play(first);
    a.wait_until("the first open stalls", |g| g.waiting.load(Ordering::SeqCst));
    let (second, b) = hanging(11, wav, 0);
    rig.player.play(second);
    a.wait_until("the superseded open ends", |g| g.dropped.load(Ordering::SeqCst));
    b.wait_until("the second open stalls", |g| g.waiting.load(Ordering::SeqCst));
    rig.player.stop();
    b.wait_until("the stopped open ends", |g| g.dropped.load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(100));
    assert!(errors(&rig.events).is_empty());
}

/// Plain bytes in memory.
struct Cursorish(Cursor<Vec<u8>>);

impl Read for Cursorish {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for Cursorish {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}

impl ByteSource for Cursorish {
    fn byte_len(&self) -> Option<u64> {
        Some(self.0.get_ref().len() as u64)
    }

    fn format_hint(&self) -> &str {
        "wav"
    }
}
