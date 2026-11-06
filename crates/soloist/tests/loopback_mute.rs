//! Does muting a process' audio session also silence its process-loopback
//! capture? A child (this test binary again) renders a quiet tone through the
//! audio engine; the parent captures it by process id, then mutes the
//! child's session and turns its volume to 0, measuring the captured level
//! each time. Run by hand (it needs an output device):
//! `cargo test -p nikgob-soloist --test loopback_mute -- --ignored --nocapture`.
//!
//! Result on Windows 11 26200 (2026-11-06): open -63 dBFS, muted -inf,
//! volume 0 -inf, restored -63. The session's mute and volume act before
//! process loopback, so Soloist's output is never muted or set to 0; a
//! small level above 0 is a plain factor the capture undoes (see the crate
//! docs and `ROUTED_LEVEL`).
#![cfg(windows)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nikgob_audio::{PcmRead, PcmSource, PlayItem, Player, PlayerOptions};
use nikgob_soloist::capture::{ProcessCapture, set_sessions};

/// -60 dBFS: measurable in the float capture, practically inaudible.
const AMPLITUDE: f32 = 0.001;
const CHILD: &str = "NIKGOB_TONE_CHILD";

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
            let s = (self.phase * std::f32::consts::TAU).sin() * AMPLITUDE;
            self.phase = (self.phase + 1000.0 / 48_000.0).fract();
            f[0] = s;
            f[1] = s;
        }
        Ok(PcmRead::Frames(out.len() / 2))
    }
}

/// The child: plays the tone for a while, then exits.
#[test]
#[ignore = "only as the child of loopback_mute"]
fn tone_child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let (player, _events) = Player::spawn(PlayerOptions::default()).unwrap();
    player.set_volume(1.0);
    player.play(PlayItem::pcm(1, Box::new(Tone { phase: 0.0 }), None));
    std::thread::sleep(Duration::from_secs(12));
}

/// RMS in dBFS of what the capture gets over `secs`, after dropping what
/// it had.
fn level(capture: &ProcessCapture, secs: f32) -> (f32, u64) {
    capture.clear();
    let before = capture.frames_captured();
    let end = Instant::now() + Duration::from_secs_f32(secs);
    let (mut sum, mut n) = (0f64, 0u64);
    let mut buf = vec![0f32; 4800];
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        loop {
            let got = capture.take(&mut buf);
            if got == 0 {
                break;
            }
            for s in &buf[..got] {
                sum += f64::from(*s) * f64::from(*s);
            }
            n += got as u64;
        }
    }
    let rms = if n == 0 { 0.0 } else { (sum / n as f64).sqrt() };
    let db = if rms > 0.0 { 20.0 * rms.log10() as f32 } else { f32::NEG_INFINITY };
    (db, capture.frames_captured() - before)
}

#[test]
#[ignore = "plays a quiet tone on the default output; run by hand"]
fn session_mute_and_process_loopback() {
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args(["--exact", "tone_child", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    std::thread::sleep(Duration::from_millis(2500));
    let capture = ProcessCapture::start(pid).expect("process loopback");
    let (open, f0) = level(&capture, 1.5);
    let muted_n = set_sessions(&[pid], Some(true), None).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let (muted, f1) = level(&capture, 1.5);
    set_sessions(&[pid], Some(false), Some(0.0)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let (zero, f2) = level(&capture, 1.5);
    set_sessions(&[pid], Some(false), Some(1.0)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let (back, f3) = level(&capture, 1.5);
    let _ = child.kill();
    let _ = child.wait();
    println!("sessions of the child: {muted_n}");
    println!("open:      {open:7.1} dBFS ({f0} frames)");
    println!("muted:     {muted:7.1} dBFS ({f1} frames)");
    println!("volume 0:  {zero:7.1} dBFS ({f2} frames)");
    println!("restored:  {back:7.1} dBFS ({f3} frames)");
    // The tone is -63 dBFS RMS; capture must hear it while open.
    assert!(open > -70.0, "the capture hears the open session");
    assert!(muted_n > 0, "the child has a session to mute");
}
