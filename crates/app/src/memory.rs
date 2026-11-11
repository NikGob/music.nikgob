//! Memory accounting: whose the process's memory is.
//!
//! The global allocator is mimalloc, wrapped to count the bytes the
//! program holds (`live`), the most it held since the last line (`peak`)
//! and every allocation made: three relaxed atomic operations per
//! allocation, nothing else. Once a minute (and when the window goes to the
//! tray and on exit, see [`log_now`]) one line goes to the log with those
//! counts, what the system says the process takes, and the sizes of the
//! known caches:
//!
//! ```text
//! memory (minute): live 212 MB, peak 480 MB (most 512), private 543 MB (held 331), working set 552 MB, allocations 12.3 M (+45 k); covers 41/64 MB in 37 sizes (decoding 0 MB), lyrics 12 tracks ~2.1 MB, stream buffers 2 / 14 MB
//! ```
//!
//! `held` is private bytes minus `live`: memory the allocator keeps after
//! it was freed (and the few private allocations that are not the heap's:
//! thread stacks, the GPU driver's). mimalloc gives unused pages back to the
//! system after a short delay, where the system heaps (Windows' and glibc's)
//! kept them for good after a peak. A large `held` that stays large says
//! memory is not given back; a large `live` says something holds
//! it. On Linux the process's numbers are the anonymous resident memory and
//! the resident set.

// The allocator and the system's process counters are unsafe calls.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

/// mimalloc, counted.
pub struct Counting;

/// What [`Counting`] hands every call to.
static INNER: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Bytes held now.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// The most bytes held since the last line.
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// The most bytes held since the start, as the lines saw it.
static MOST: AtomicUsize = AtomicUsize::new(0);
/// Allocations made (reallocations count too).
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
/// [`ALLOCATIONS`] at the last line.
static LOGGED_ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn grew(by: usize) {
    let live = LIVE.fetch_add(by, Relaxed).wrapping_add(by);
    // A plain read first: a new peak is rare, a write to the shared line is not.
    if live > PEAK.load(Relaxed) {
        PEAK.fetch_max(live, Relaxed);
    }
}

#[inline]
fn shrank(by: usize) {
    LIVE.fetch_sub(by, Relaxed);
}

// SAFETY: every call goes to mimalloc with the caller's own
// arguments; the counting only reads the layout and the result.
unsafe impl GlobalAlloc for Counting {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as the caller promised for this call.
        let ptr = unsafe { INNER.alloc(layout) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Relaxed);
            grew(layout.size());
        }
        ptr
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as the caller promised for this call.
        let ptr = unsafe { INNER.alloc_zeroed(layout) };
        if !ptr.is_null() {
            ALLOCATIONS.fetch_add(1, Relaxed);
            grew(layout.size());
        }
        ptr
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as the caller promised for this call.
        unsafe { INNER.dealloc(ptr, layout) };
        shrank(layout.size());
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as the caller promised for this call.
        let moved = unsafe { INNER.realloc(ptr, layout, new_size) };
        if !moved.is_null() {
            ALLOCATIONS.fetch_add(1, Relaxed);
            if new_size >= layout.size() {
                grew(new_size - layout.size());
            } else {
                shrank(layout.size() - new_size);
            }
        }
        moved
    }
}

/// Starts the line once a minute (on a thread of its own that sleeps in
/// between).
pub fn start() {
    let started = std::thread::Builder::new().name("memory-log".into()).spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            log_now("minute");
        }
    });
    if let Err(e) = started {
        log::warn!("memory: no thread for the memory line: {e}");
    }
}

/// Logs the line now; `at` says why ("minute", "tray", "exit").
pub fn log_now(at: &str) {
    log::info!("{}", line(at, &Sample::take()));
}

/// What the system says the process takes, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Process {
    /// Private bytes (Windows) or anonymous resident memory (Linux): what
    /// the heap's `live` is compared with.
    private: u64,
    /// The working set (Windows) or the resident set (Linux).
    resident: u64,
}

/// Everything one line says.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sample {
    live: usize,
    peak: usize,
    most: usize,
    allocations: u64,
    allocations_since: u64,
    process: Option<Process>,
    covers: nikgob_md3e::covers::Memory,
    /// Tracks and estimated bytes.
    lyrics: (usize, usize),
    /// Stream buffers and their bytes.
    streams: (usize, u64),
}

impl Sample {
    /// Reads every count; the peak and the allocations start again from here.
    fn take() -> Self {
        let live = LIVE.load(Relaxed);
        let peak = PEAK.swap(live, Relaxed).max(live);
        let most = MOST.fetch_max(peak, Relaxed).max(peak);
        let allocations = ALLOCATIONS.load(Relaxed);
        let before = LOGGED_ALLOCATIONS.swap(allocations, Relaxed);
        Self {
            live,
            peak,
            most,
            allocations,
            allocations_since: allocations.saturating_sub(before),
            process: process(),
            covers: nikgob_md3e::covers::memory(),
            lyrics: nikgob_runtime::lyrics_memory(),
            streams: nikgob_youtube::stream_buffers(),
        }
    }
}

/// Bytes as whole megabytes (one decimal below ten).
fn mb(bytes: u64) -> String {
    let m = bytes as f64 / (1024.0 * 1024.0);
    if m < 10.0 { format!("{m:.1}") } else { format!("{m:.0}") }
}

/// A count as it reads best: 950, 45 k, 12.3 M.
fn count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{} k", n / 1_000),
        _ => format!("{:.1} M", n as f64 / 1e6),
    }
}

fn line(at: &str, s: &Sample) -> String {
    let b = |n: usize| mb(n as u64);
    let mut out = format!("memory ({at}): live {} MB, peak {} MB (most {})", b(s.live), b(s.peak), b(s.most));
    if let Some(p) = s.process {
        let (private, resident) = if cfg!(windows) { ("private", "working set") } else { ("anon", "resident") };
        let held = p.private.saturating_sub(s.live as u64);
        out += &format!(", {private} {} MB (held {}), {resident} {} MB", mb(p.private), mb(held), mb(p.resident));
    }
    out += &format!(", allocations {} (+{})", count(s.allocations), count(s.allocations_since));
    let c = s.covers;
    out += &format!(
        "; covers {}/{} MB in {} sizes (decoding {} MB), lyrics {} tracks ~{} MB, stream buffers {} / {} MB",
        b(c.textures),
        b(c.budget),
        c.sizes,
        b(c.decoding),
        s.lyrics.0,
        b(s.lyrics.1),
        s.streams.0,
        mb(s.streams.1),
    );
    out
}

/// Private bytes and the working set, from `K32GetProcessMemoryInfo`.
#[cfg(windows)]
fn process() -> Option<Process> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the call fills at most `c.cb` bytes of `c`, its own size; the
    // pseudo handle of the current process needs no closing.
    let ok = unsafe {
        K32GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(), c.cb)
    };
    (ok != 0).then_some(Process { private: c.PrivateUsage as u64, resident: c.WorkingSetSize as u64 })
}

/// Anonymous resident memory and the resident set, from
/// `/proc/self/status` (in kB there).
#[cfg(target_os = "linux")]
fn process() -> Option<Process> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = |key: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|k| k * 1024)
    };
    Some(Process { private: kb("RssAnon:")?, resident: kb("VmRSS:")? })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process() -> Option<Process> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_says_whose_the_memory_is() {
        let s = Sample {
            live: 212 << 20,
            peak: 480 << 20,
            most: 512 << 20,
            allocations: 12_345_678,
            allocations_since: 45_200,
            process: Some(Process { private: 543 << 20, resident: 552 << 20 }),
            covers: nikgob_md3e::covers::Memory { textures: 41 << 20, sizes: 37, budget: 64 << 20, decoding: 0 },
            lyrics: (12, 2_200_000),
            streams: (2, 14 << 20),
        };
        let (private, resident) = if cfg!(windows) { ("private", "working set") } else { ("anon", "resident") };
        assert_eq!(
            line("minute", &s),
            format!(
                "memory (minute): live 212 MB, peak 480 MB (most 512), {private} 543 MB (held 331), {resident} 552 MB, \
                 allocations 12.3 M (+45 k); covers 41/64 MB in 37 sizes (decoding 0.0 MB), lyrics 12 tracks ~2.1 MB, \
                 stream buffers 2 / 14 MB"
            )
        );
        let none = Sample { process: None, ..s };
        assert!(!line("exit", &none).contains("held"));
    }

    #[test]
    fn counts_follow_what_is_held() {
        // Other tests allocate and free meanwhile: only what only grows is
        // looked at.
        let block = std::hint::black_box(vec![0u8; 8 << 20]);
        assert!(PEAK.load(Relaxed) >= 8 << 20, "an 8 MB block is counted");
        drop(block);
        assert!(ALLOCATIONS.load(Relaxed) > 0);
    }

    #[test]
    fn the_process_counters_are_read() {
        if cfg!(any(windows, target_os = "linux")) {
            let p = process().expect("the process's memory");
            assert!(p.private > 0 && p.resident > 0, "{p:?}");
        }
    }
}
