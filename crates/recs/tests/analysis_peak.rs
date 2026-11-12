//! The heap peak of the sound analysis of an 8-minute track
//! (docs/recommendations.md §5.1.2): the whole-track pass (the first 8
//! minutes into bliss) against the three windows of 15 s, with and without
//! the neural model's front end (its log-mel patches; the ONNX session's
//! own memory is the runtime's and is not counted here).
//!
//! The audio is made chunk by chunk as a decoder hands it out, so only what
//! the analysis itself keeps is measured. Run with
//! `cargo test -p nikgob-recs --test analysis_peak -- --ignored --nocapture`.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use nikgob_recs::embedding::{EFFNET_DIM, EMBED_RATE, MEL_BANDS, Mel};
use nikgob_recs::{AnalysisError, PcmAnalyzer, SoundModel, WindowedAnalyzer, windows};

/// Counts the bytes held and the most held since the last reset.
struct Counting;

static HELD: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to the system allocator unchanged; only the
// counters are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's contract is passed on.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let now = HELD.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(now, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) };
        HELD.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as above.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            if new_size >= layout.size() {
                let now = HELD.fetch_add(new_size - layout.size(), Ordering::Relaxed) + new_size - layout.size();
                PEAK.fetch_max(now, Ordering::Relaxed);
            } else {
                HELD.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        p
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const RATE: u32 = 44_100;
const SECONDS: u32 = 8 * 60;
const CHUNK: usize = 4096;

/// Stereo music-like audio from `from` (frames), `n` frames.
fn chunk(from: usize, n: usize, out: &mut Vec<f32>) {
    out.clear();
    for i in from..from + n {
        let t = i as f32 / RATE as f32;
        let note = (t * 2.0) as usize;
        let f = 110.0 * 2f32.powf((note % 12) as f32 / 12.0);
        let env = (-4.0 * (t % 0.5)).exp();
        let s = (std::f32::consts::TAU * f * t).sin() * 0.3 * env + (std::f32::consts::TAU * f * 3.0 * t).sin() * 0.1 * env;
        out.extend_from_slice(&[s, s * 0.9]);
    }
}

/// Bytes held at the peak of `f`, over what was held before it.
fn peak_of(f: impl FnOnce()) -> usize {
    let base = HELD.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    f();
    PEAK.load(Ordering::Relaxed).saturating_sub(base)
}

fn mib(b: usize) -> f64 {
    b as f64 / 1_048_576.0
}

/// The model's front end without the network: the log-mel frames and one
/// patch at a time, as the runtime's Discogs-EffNet makes them.
struct FrontEnd(Mel);

impl SoundModel for FrontEnd {
    fn name(&self) -> &str {
        "front-end"
    }
    fn dim(&self) -> usize {
        EFFNET_DIM
    }
    fn sample_rate(&self) -> u32 {
        EMBED_RATE
    }
    fn embed(&self, mono: &[f32]) -> Result<Vec<f32>, AnalysisError> {
        let frames = self.0.frames(mono);
        let n = frames.len() / MEL_BANDS;
        let mut v = vec![0f32; EFFNET_DIM];
        for start in Mel::patch_starts(n) {
            let patch = Mel::patch(&frames, start);
            v[0] += patch[0];
        }
        Ok(v)
    }
}

#[test]
#[ignore = "a measurement: run it with --ignored --nocapture"]
fn the_windows_keep_the_analysis_peak_small() {
    let frames = (RATE * SECONDS) as usize;
    let mut buf = Vec::with_capacity(CHUNK * 2);

    let whole = peak_of(|| {
        let mut a = PcmAnalyzer::new(RATE, 2).unwrap();
        let mut at = 0;
        while at < frames {
            chunk(at, CHUNK.min(frames - at), &mut buf);
            at += CHUNK;
            if a.push(&buf).is_break() {
                break;
            }
        }
        a.finish().unwrap();
    });

    let run_windows = |model: Option<&dyn SoundModel>, buf: &mut Vec<f32>| {
        let mut a = WindowedAnalyzer::new(RATE, 2, true, model).unwrap();
        for (i, w) in windows(Some(f64::from(SECONDS))).iter().enumerate() {
            let (from, to) = ((w.start * f64::from(RATE)) as usize, ((w.start + w.len) * f64::from(RATE)) as usize);
            let mut at = from;
            while at < to {
                chunk(at, CHUNK.min(to - at), buf);
                at += CHUNK;
                let _ = a.push(i, buf);
            }
        }
        let out = a.finish().unwrap();
        assert!(out.bliss.unwrap().is_ok());
    };
    let windowed = peak_of(|| run_windows(None, &mut buf));
    let model = FrontEnd(Mel::new());
    let with_model = peak_of(|| run_windows(Some(&model), &mut buf));

    println!("analysis peak of an 8-minute track: whole {:.1} MiB, three windows {:.1} MiB, with the model's front end {:.1} MiB", mib(whole), mib(windowed), mib(with_model));
    assert!(mib(windowed) < 25.0, "{:.1} MiB", mib(windowed));
    assert!(mib(with_model) < 25.0, "{:.1} MiB", mib(with_model));
    assert!(windowed * 4 < whole, "the windows take a fraction of the whole pass");
}
