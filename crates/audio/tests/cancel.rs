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
fn wav16(rate: u32, frames: usize) -> (Vec<u8>, Vec<f32>) {
    let ints = to_i16(&signal(rate, 0, frames));
    let mut out = Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: rate,
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


/// The last frames of `out` are the stream's at `pos`.
fn lands(out: &[f32], pos: Duration, original: &[f32]) -> bool {
    let end = (pos.as_secs_f64() * f64::from(RATE)).round() as usize;
    let n = 128;
    (end.saturating_sub(4)..=end + 4).any(|e| {
        e >= n / 2 && e * 2 <= original.len()
            && max_diff(&out[out.len() - n..], &original[(e - n / 2) * 2..e * 2]) < 1e-6
    })
}

fn errors(events: &crossbeam_channel::Receiver<Event>) -> Vec<Event> {
    events
        .try_iter()
        .filter(|e| matches!(e, Event::Error { .. }))
        .collect()
}

#[test]
fn stop_takes_a_stalled_stream_away_at_once() {
    let (bytes, _) = wav16(RATE, RATE as usize * 3);
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
    let (bytes, _) = wav16(RATE, RATE as usize * 3);
    let mut rig = rig(RATE);
    let (item, gate) = hanging(1, bytes, at(0.3));
    rig.player.play(item);
    gate.wait_until("the read stalls", |g| g.waiting.load(Ordering::SeqCst));
    let (short, short_samples) = wav16(RATE, RATE as usize / 2);
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
    let (a, a_samples) = wav16(RATE, RATE as usize / 4);
    let (b, b_samples) = wav16(RATE, RATE as usize / 4);
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
    let (bytes, original) = wav16(RATE, RATE as usize * 3);
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
    let (bytes, _) = wav16(RATE, RATE as usize * 3);
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
fn superseded_opens_never_hold_the_newest_up() {
    // Opens that hang where nothing can interrupt them (a service lookup
    // that ignores `open_wanted`): superseded, they no longer count, and the
    // newest item opens and plays at once while they still hang.
    let (bytes, _) = wav16(RATE, RATE as usize / 4);
    let release = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicUsize::new(0));
    let mut rig = rig(RATE);
    let slow = |id: u64| {
        let (release, busy, bytes) = (Arc::clone(&release), Arc::clone(&busy), bytes.clone());
        let open: StreamOpener = Box::new(move || {
            busy.fetch_add(1, Ordering::SeqCst);
            while !release.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            busy.fetch_sub(1, Ordering::SeqCst);
            Ok(Box::new(Cursorish(Cursor::new(bytes))) as Box<dyn ByteSource>)
        });
        PlayItem::stream(id, open)
    };
    for id in 1..=3 {
        rig.player.play(slow(id));
        let t = Instant::now();
        while busy.load(Ordering::SeqCst) < id as usize {
            assert!(t.elapsed() < PROMPT, "open {id} runs");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    // An opener that looks whether it is still wanted gives up by itself.
    let wanted_seen = Arc::new(AtomicBool::new(true));
    let seen = Arc::clone(&wanted_seen);
    let polite: StreamOpener = Box::new(move || {
        let wanted = nikgob_audio::open_wanted();
        let t = Instant::now();
        while wanted() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(1));
        }
        seen.store(wanted(), Ordering::SeqCst);
        Err("no longer wanted".into())
    });
    rig.player.play(PlayItem::stream(4, polite));
    std::thread::sleep(Duration::from_millis(50));
    let t = Instant::now();
    rig.player.play(whole(5, bytes.clone()));
    let out = rig.device.drain(RATE as usize * 2);
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());
    assert!(out.len() / 2 >= RATE as usize / 4, "the newest played: {}", out.len() / 2);
    assert_eq!(busy.load(Ordering::SeqCst), 3, "the superseded ones still hang");
    let t = Instant::now();
    while wanted_seen.load(Ordering::SeqCst) {
        assert!(t.elapsed() < PROMPT, "the polite opener saw it was superseded");
        std::thread::sleep(Duration::from_millis(1));
    }
    release.store(true, Ordering::SeqCst);

    // Opens whose probe stalls end as soon as they are superseded.
    let (wav, _) = wav16(RATE, RATE as usize);
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

#[test]
fn a_seek_wakes_a_read_stalled_in_the_other_item_of_a_crossfade() {
    // In a crossfade both items are read; here the outgoing one stalls
    // while the incoming one is what the listener hears and seeks in.
    let (a, _) = wav16(RATE, RATE as usize * 3);
    let (b, b_samples) = wav16(RATE, RATE as usize * 3);
    let mut rig = rig(RATE);
    rig.player.set_crossfade(Some(Duration::from_secs(1)));
    let (first, gate) = hanging(1, a, at(2.5));
    rig.player.play(first);
    rig.player.set_next(whole(2, b));
    let t = Instant::now();
    while rig.player.current_id() != Some(2) {
        assert!(t.elapsed() < PROMPT * 5, "the crossfade is heard");
        let _ = rig.device.render(256);
    }
    gate.wait_until("the outgoing item stalls", |g| g.waiting.load(Ordering::SeqCst));
    let target = Duration::from_secs(1);
    let t = Instant::now();
    rig.seek_to(target);
    let out = rig.device.render(RATE as usize / 10);
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());
    let pos = rig.player.position().unwrap();
    assert_eq!(rig.player.current_id(), Some(2));
    assert!(
        pos > target + Duration::from_millis(90) && pos < target + Duration::from_millis(200),
        "{pos:?}"
    );
    assert!(lands(&out, pos, &b_samples), "the incoming item at {pos:?}");
    gate.wait_until("the outgoing item is let go", |g| g.dropped.load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(100));
    assert!(errors(&rig.events).is_empty());
}

#[test]
fn a_second_seek_right_after_the_first_is_not_lost() {
    let (bytes, original) = wav16(RATE, RATE as usize * 3);
    let mut rig = rig(RATE);
    let (item, gate) = hanging(1, bytes, at(0.6));
    rig.player.play(item);
    gate.wait_until("the read stalls", |g| g.waiting.load(Ordering::SeqCst));
    // Two seeks back to back: the first is taken, plays on to the stall
    // (which must not wait now: the second is on its way), then the second.
    rig.player.seek(Duration::from_millis(200));
    let target = Duration::from_millis(300);
    let t = Instant::now();
    rig.seek_to(target);
    let out = rig.device.render(RATE as usize / 10);
    assert!(t.elapsed() < PROMPT, "{:?}", t.elapsed());
    let pos = rig.player.position().unwrap();
    assert!(
        pos > target + Duration::from_millis(90) && pos < target + Duration::from_millis(200),
        "{pos:?}"
    );
    assert!(lands(&out, pos, &original), "the stream at {pos:?}");
    gate.wait_until("the read stalls again", |g| g.waiting.load(Ordering::SeqCst));
    rig.player.stop();
    gate.wait_until("the stalled stream is let go", |g| g.dropped.load(Ordering::SeqCst));
    std::thread::sleep(Duration::from_millis(100));
    assert!(errors(&rig.events).is_empty());
}

/// Bytes whose read past `at` waits for `go`, then fails once (a real
/// failure of the stream, not a cancel), then serves the rest.
struct FailsOnce {
    bytes: Cursor<Vec<u8>>,
    at: u64,
    go: Arc<AtomicBool>,
    waiting: Arc<AtomicBool>,
    failed: bool,
}

impl Read for FailsOnce {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.failed && self.bytes.position() >= self.at {
            self.waiting.store(true, Ordering::SeqCst);
            while !self.go.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            self.failed = true;
            return Err(std::io::Error::other("connection reset"));
        }
        self.bytes.read(buf)
    }
}

impl Seek for FailsOnce {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.bytes.seek(pos)
    }
}

impl ByteSource for FailsOnce {
    fn byte_len(&self) -> Option<u64> {
        Some(self.bytes.get_ref().len() as u64)
    }

    fn format_hint(&self) -> &str {
        "wav"
    }
}

#[test]
fn a_failure_of_the_playing_item_is_reported_while_the_next_one_is_replaced() {
    let (a, _) = wav16(RATE, RATE as usize * 3);
    let (b, _) = wav16(RATE, RATE as usize / 4);
    let mut rig = rig(RATE);
    let go = Arc::new(AtomicBool::new(false));
    let waiting = Arc::new(AtomicBool::new(false));
    let (g, w) = (Arc::clone(&go), Arc::clone(&waiting));
    let open: StreamOpener = Box::new(move || {
        Ok(Box::new(FailsOnce {
            bytes: Cursor::new(a),
            at: at(2.5),
            go: g,
            waiting: w,
            failed: false,
        }) as Box<dyn ByteSource>)
    });
    rig.player.play(PlayItem::stream(1, open));
    rig.player.set_next(whole(2, b.clone()));
    // Heard up to where the next item is long open and the read waits.
    let _ = rig.device.render(RATE as usize * 2);
    let t = Instant::now();
    while !waiting.load(Ordering::SeqCst) {
        assert!(t.elapsed() < PROMPT, "the read of the playing item waits");
        std::thread::sleep(Duration::from_millis(1));
    }
    // The next item is replaced (and cancelled) just as the playing one
    // fails for real.
    rig.player.set_next(whole(3, b));
    go.store(true, Ordering::SeqCst);
    let seen = wait_event(&rig.events, PROMPT, |e| matches!(e, Event::Error { id: Some(1), .. }));
    assert!(
        seen.iter().any(|e| matches!(e, Event::Error { id: Some(1), .. })),
        "the failure is reported: {seen:?}"
    );
}

#[test]
fn plays_from_two_threads_leave_the_last_one_sent_playing() {
    let (bytes, _) = wav16(RATE, RATE as usize / 10);
    let mut rig = rig(RATE);
    for round in 0..20u64 {
        let (a, b) = (round * 2 + 10, round * 2 + 11);
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let (p, bar, bb) = (rig.player.clone(), Arc::clone(&barrier), bytes.clone());
        let other = std::thread::spawn(move || {
            bar.wait();
            p.play(whole(b, bb));
        });
        barrier.wait();
        rig.player.play(whole(a, bytes.clone()));
        other.join().unwrap();
        // Whichever came last plays: never neither.
        let t = Instant::now();
        let mut seen = Vec::new();
        loop {
            let _ = rig.device.render(256);
            seen.extend(rig.events.try_iter());
            let started = seen.iter().any(|e| match e {
                Event::Started { id, .. } | Event::TrackChanged { to: id, .. } => *id == a || *id == b,
                _ => false,
            });
            if started {
                break;
            }
            assert!(t.elapsed() < PROMPT, "round {round}: nothing plays: {seen:?}");
        }
        rig.player.stop();
        let _ = rig.device.render(256);
    }
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
