//! How Soloist's captured sound fares on its way through our engine: a child
//! (this binary again) renders a quiet tone, the parent captures it by
//! process id and plays it through a real engine on the default output via
//! [`SoloistTrack`], with the engine at volume 0 (so nothing is heard).
//! It reports the output's underruns, the added delay (capture prebuffer +
//! the engine's ring + the device) and the process' CPU, then the same for
//! the tone played straight by the engine as the baseline. By hand:
//! `cargo test -p nikgob-soloist --test capture_soak -- --ignored --nocapture`
//! (`SOAK_SECS`, default 180).
#![cfg(windows)]
#![allow(unsafe_code)]

use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nikgob_audio::{PcmRead, PcmSource, PlayItem, Player, PlayerOptions};
use nikgob_soloist::LiveView;
use nikgob_soloist::capture::ProcessCapture;
use nikgob_soloist::protocol::{Anchor, Command, Status};
use nikgob_soloist::track::{Link, SoloistTrack};

const CHILD: &str = "NIKGOB_SOAK_CHILD";
const URI: &str = "spotify:track:soak";

struct Tone {
    phase: f32,
}

impl PcmSource for Tone {
    fn sample_rate(&self) -> u32 {
        48_000
    }
    fn channels(&self) -> u16 {
        2
    }
    fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, nikgob_audio::PcmError> {
        for f in out.chunks_exact_mut(2) {
            let s = (self.phase * std::f32::consts::TAU).sin() * 0.001;
            self.phase = (self.phase + 1000.0 / 48_000.0).fract();
            f[0] = s;
            f[1] = s;
        }
        Ok(PcmRead::Frames(out.len() / 2))
    }
}

#[test]
#[ignore = "only as the child of the soak"]
fn soak_child() {
    let Some(secs) = std::env::var(CHILD).ok().and_then(|s| s.parse::<u64>().ok()) else {
        return;
    };
    let (player, _events) = Player::spawn(PlayerOptions::default()).unwrap();
    player.set_volume(1.0);
    player.play(PlayItem::pcm(1, Box::new(Tone { phase: 0.0 }), None));
    std::thread::sleep(Duration::from_secs(secs));
}

/// A Soloist that is always playing our track, over a real capture.
struct CaptureLink {
    capture: ProcessCapture,
    taken: AtomicU64,
}

impl Link for CaptureLink {
    fn snapshot(&self) -> LiveView {
        LiveView {
            ready: true,
            active: true,
            status: Status::Playing,
            uri: Some(URI.into()),
            duration_ms: Some(3_600_000),
            anchor: Some(Anchor { position_ms: 0, timestamp_ms: 1, speed: 1.0 }),
            track_changes: 1,
            error: None,
        }
    }
    fn send(&self, _c: Command) -> bool {
        true
    }
    fn take_captured(&self, out: &mut [f32]) -> Option<usize> {
        let n = self.capture.take(out);
        self.taken.fetch_add(n as u64 / 2, Ordering::Relaxed);
        Some(n)
    }
    fn buffered_frames(&self) -> Option<usize> {
        Some(self.capture.buffered_frames())
    }
    fn clear_captured(&self) {
        self.capture.clear();
    }
    fn keep_captured(&self, frames: usize) {
        self.capture.keep_newest(frames);
    }
    fn capture_gain(&self) -> f32 {
        1.0
    }
    fn paused_by_us(&self) -> bool {
        false
    }
    fn playing(&self, _delta: i32) {}
}

/// CPU time of this process so far.
fn cpu_time() -> Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(GetCurrentProcess(), &raw mut a, &raw mut b, &raw mut k, &raw mut u) }.unwrap();
    let t = |f: FILETIME| ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) * 100;
    Duration::from_nanos(t(k) + t(u))
}

struct Report {
    underruns: u64,
    cpu_percent: f64,
    delays_ms: Vec<f64>,
}

fn watch(player: &Player, secs: u64, delay: impl Fn(&Player) -> Option<f64>) -> Report {
    let start_underruns = player.underruns();
    let cpu0 = cpu_time();
    let t0 = Instant::now();
    let mut delays = Vec::new();
    let mut last_report = Instant::now();
    while t0.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_millis(250));
        if let Some(d) = delay(player) {
            delays.push(d);
        }
        if last_report.elapsed() >= Duration::from_secs(30) {
            last_report = Instant::now();
            println!("  {:>4} s: underruns {}", t0.elapsed().as_secs(), player.underruns() - start_underruns);
        }
    }
    let cpu = cpu_time() - cpu0;
    Report {
        underruns: player.underruns() - start_underruns,
        cpu_percent: cpu.as_secs_f64() / t0.elapsed().as_secs_f64() * 100.0,
        delays_ms: delays,
    }
}

fn stats(v: &[f64]) -> String {
    if v.is_empty() {
        return "-".into();
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let p = |q: f64| s[((s.len() - 1) as f64 * q) as usize];
    format!("median {:.0} ms, p95 {:.0} ms, max {:.0} ms", p(0.5), p(0.95), p(1.0))
}

#[test]
#[ignore = "plays a quiet tone and an inaudible engine on the default output for minutes; by hand"]
fn capture_through_the_engine() {
    let secs: u64 = std::env::var("SOAK_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(180);
    let exe = std::env::current_exe().unwrap();
    let mut child = Process::new(exe)
        .args(["--exact", "soak_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, (secs + 60).to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    let link = Arc::new(CaptureLink { capture: ProcessCapture::start(child.id()).unwrap(), taken: AtomicU64::new(0) });

    // Baseline: the engine playing a tone of its own.
    let (player, _ev) = Player::spawn(PlayerOptions::default()).unwrap();
    player.set_volume(0.0);
    player.play(PlayItem::pcm(1, Box::new(Tone { phase: 0.0 }), None));
    std::thread::sleep(Duration::from_secs(2));
    let base = watch(&player, 30, |_| None);
    player.stop();
    drop(player);

    // The capture path.
    let (player, _ev) = Player::spawn(PlayerOptions::default()).unwrap();
    player.set_volume(0.0);
    let track = SoloistTrack::new(link.clone(), URI.into(), None);
    player.play(PlayItem::pcm(2, Box::new(track), None));
    std::thread::sleep(Duration::from_secs(2));
    let l = link.clone();
    let routed = watch(&player, secs, move |p| {
        // Frames given to the engine minus frames it has played = its ring
        // and the device; plus what waits in the capture.
        let pos = p.position()?;
        let given = l.taken.load(Ordering::Relaxed) as f64 / 48.0;
        let heard = pos.as_secs_f64() * 1000.0;
        let waiting = l.capture.buffered_frames() as f64 / 48.0;
        Some(given - heard + waiting)
    });
    let captured_sound = link.capture.frames_sounding();
    player.stop();
    let _ = child.kill();
    let _ = child.wait();
    println!("baseline (engine tone, 30 s): underruns {}, CPU {:.2} % of one core", base.underruns, base.cpu_percent);
    println!(
        "capture path ({secs} s): underruns {}, CPU {:.2} % of one core, delay {}",
        routed.underruns,
        routed.cpu_percent,
        stats(&routed.delays_ms)
    );
    println!("captured sounding frames: {captured_sound}");
    assert!(captured_sound > 48_000, "the capture heard the tone");
}
