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
//! memory (minute): live 212 MB, peak 480 MB (most 512), private 543 MB (allocator 48, outside Rust 283), working set 552 MB, mimalloc 260 MB (peak 900; pages 317, abandoned 32, threads 26), allocations 12.3 M (+45 k); covers 41/64 MB in 37 sizes (decoding 0 MB), lyrics 12 tracks ~2.1 MB, stream buffers 2 / 14 MB
//! ```
//!
//! `mimalloc` is what the allocator itself has committed now and at most
//! since the start, with its pages in use, the pages of threads that ended
//! (`abandoned`) and the threads it serves. Of the private bytes,
//! `allocator` is what mimalloc keeps beyond `live` (freed, not given back)
//! and `outside Rust` what is not the Rust heap's at all: thread stacks,
//! the GPU driver's and DirectML's memory charged to the process, the C
//! heap of ONNX Runtime and the driver. Without mimalloc's numbers the two
//! are one `held` (private minus `live`). A large `allocator` that stays
//! large says memory is not given back; a large `live` says something holds
//! it; a large `outside Rust` is native code's. On Linux the process's
//! numbers are the anonymous resident memory and the resident set.
//!
//! [`tune`] sets mimalloc to commit pages as they are written and to give
//! freed memory back at once (its defaults commit whole pages up front and
//! keep freed memory a second or more); the workers that do bursts collect
//! their own heap after them (`nikgob_runtime::release_thread_memory`).

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

/// mimalloc 3's options by their place in its `mi_option_t` (the Rust
/// bindings name only the first few), each with its default. [`tune`] sets
/// none of them unless the version is 3.x and every one still reads its
/// default, so another mimalloc (v2, or a 3.x that moved one) is left as
/// it is.
mod option {
    use std::ffi::c_long;

    /// Freed arena memory is decommitted after this many ms (1000).
    pub const PURGE_DELAY: (i32, c_long) = (15, 1000);
    /// Every N generic allocations a thread collects its heap (10000).
    pub const GENERIC_COLLECT: (i32, c_long) = (34, 10_000);
    /// Full small pages a heap keeps per size class (2).
    pub const PAGE_FULL_RETAIN: (i32, c_long) = (36, 2);
    /// 0: a page is committed whole when taken; 1: as it is written; 2: as
    /// it is written, except where the system overcommits (Linux) (0).
    pub const PAGE_COMMIT_ON_DEMAND: (i32, c_long) = (40, 0);
    /// The major version these places are of (`MI_MALLOC_VERSION` / 10000).
    pub const MAJOR: i32 = 3;
}

/// What [`tune`] did, for the log once there is one.
static TUNED: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The options [`tune`] sets, with their values.
const TUNING: [((i32, std::ffi::c_long), std::ffi::c_long); 4] = [
    (option::PAGE_COMMIT_ON_DEMAND, 2),
    (option::PURGE_DELAY, 0),
    (option::PAGE_FULL_RETAIN, 0),
    (option::GENERIC_COLLECT, 1_000),
];

/// mimalloc's options, set first thing in `main` (before the threads):
///
/// * a page is committed as it is written (on Windows; Linux commits
///   lazily anyway): a burst over many sizes and threads no longer commits
///   gigabytes it never touches;
/// * freed arena memory is decommitted at once, not a second later and
///   only when the arena is used again;
/// * no full pages kept per size class, and each thread collects its heap
///   every 1 000 generic allocations instead of 10 000.
///
/// [`start`] logs what was done. Returns whether the options were set.
pub fn tune() -> bool {
    use libmimalloc_sys as mi;
    // SAFETY: plain calls into the linked mimalloc with option places
    // inside its range (`_mi_option_last` is 47 in 3.x, checked by the
    // version first); reading and setting an option is thread safe.
    let (set, said) = unsafe {
        let version = mi::mi_version();
        if version / 10_000 != option::MAJOR {
            (false, format!("mimalloc {version}: options left as they are (these are {}.x's)", option::MAJOR))
        } else if !TUNING.iter().all(|((at, default), _)| mi::mi_option_get(*at) == *default) {
            (false, format!("mimalloc {version}: options left as they are (one is not at its default)"))
        } else {
            for ((at, _), value) in TUNING {
                mi::mi_option_set(at, value);
            }
            (true, format!("mimalloc {version}: pages committed on demand, purged at once, no full pages kept, collected every 1000"))
        }
    };
    let _ = TUNED.set(said);
    set
}

/// What mimalloc's statistics say of itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Mimalloc {
    /// Bytes it has committed now and at most since the start.
    committed: u64,
    peak: u64,
    /// Pages in use now.
    pages: u64,
    /// Pages of threads that ended, not yet taken over by another.
    abandoned: u64,
    /// Threads with a heap now.
    threads: u64,
}

/// mimalloc's statistics. (Its `mi_process_info` is the whole process's on
/// Windows: the same numbers [`process`] reads.)
fn mimalloc_stats() -> Option<Mimalloc> {
    use libmimalloc_sys as mi;
    // SAFETY: with no buffer mimalloc allocates the text itself (null on
    // failure), NUL-terminated; it is read once and handed back to
    // mimalloc's own free.
    unsafe {
        let text = mi::mi_stats_get_json(0, std::ptr::null_mut());
        if text.is_null() {
            return None;
        }
        let got = stats_of(&std::ffi::CStr::from_ptr(text).to_string_lossy());
        mi::mi_free(text.cast());
        got
    }
}

/// `"<key>": { "total": T, "peak": P, "current": C }` of mimalloc's
/// statistics as (C, P).
fn entry_of(json: &str, key: &str) -> Option<(u64, u64)> {
    let rest = &json[json.find(&format!("\"{key}\":"))?..];
    let entry = &rest[..rest.find('}')?];
    let field = |key: &str| -> Option<u64> {
        let label = format!("\"{key}\":");
        let from = entry.find(&label)? + label.len();
        let digits: String = entry[from..].trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
        u64::try_from(digits.parse::<i64>().ok()?).ok()
    };
    Some((field("current")?, field("peak")?))
}

/// [`Mimalloc`] from the statistics' text: the committed bytes must be
/// there, a count reads 0 when a version leaves it out.
fn stats_of(json: &str) -> Option<Mimalloc> {
    let (committed, peak) = entry_of(json, "committed")?;
    let now = |key: &str| entry_of(json, key).map_or(0, |(c, _)| c);
    Some(Mimalloc { committed, peak, pages: now("pages"), abandoned: now("pages_abandoned"), threads: now("threads") })
}

/// Starts the line once a minute (on a thread of its own that sleeps in
/// between).
pub fn start() {
    if let Some(t) = TUNED.get() {
        log::info!("memory: {t}");
    }
    let started = std::thread::Builder::new().name("memory-log".into()).spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(60));
            log_now("minute");
            // Freed arena memory not given back yet goes now (the collect
            // of this thread's own heap visits every arena).
            // SAFETY: a plain call; mimalloc's collect is thread safe.
            unsafe { libmimalloc_sys::mi_collect(true) };
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
    /// What mimalloc says of itself.
    mimalloc: Option<Mimalloc>,
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
            mimalloc: mimalloc_stats(),
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
        // Whose the private bytes beyond `live` are: the allocator's own
        // keeping, or native code's (one `held` when that is not known).
        let parts = match s.mimalloc {
            Some(m) => format!(
                "allocator {}, outside Rust {}",
                mb(m.committed.saturating_sub(s.live as u64)),
                mb(p.private.saturating_sub(m.committed))
            ),
            None => format!("held {}", mb(p.private.saturating_sub(s.live as u64))),
        };
        out += &format!(", {private} {} MB ({parts}), {resident} {} MB", mb(p.private), mb(p.resident));
    }
    if let Some(m) = s.mimalloc {
        out += &format!(
            ", mimalloc {} MB (peak {}; pages {}, abandoned {}, threads {})",
            mb(m.committed),
            mb(m.peak),
            m.pages,
            m.abandoned,
            m.threads
        );
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
            mimalloc: Some(Mimalloc { committed: 260 << 20, peak: 900 << 20, pages: 317, abandoned: 32, threads: 26 }),
            covers: nikgob_md3e::covers::Memory { textures: 41 << 20, sizes: 37, budget: 64 << 20, decoding: 0 },
            lyrics: (12, 2_200_000),
            streams: (2, 14 << 20),
        };
        let (private, resident) = if cfg!(windows) { ("private", "working set") } else { ("anon", "resident") };
        assert_eq!(
            line("minute", &s),
            format!(
                "memory (minute): live 212 MB, peak 480 MB (most 512), {private} 543 MB (allocator 48, outside Rust 283), {resident} 552 MB, \
                 mimalloc 260 MB (peak 900; pages 317, abandoned 32, threads 26), allocations 12.3 M (+45 k); covers 41/64 MB in 37 sizes (decoding 0.0 MB), lyrics 12 tracks ~2.1 MB, \
                 stream buffers 2 / 14 MB"
            )
        );
        let none = Sample { process: None, mimalloc: None, ..s };
        assert!(!line("exit", &none).contains("held"));
        assert!(!line("exit", &none).contains("mimalloc"));
        // Without mimalloc's numbers the private bytes beyond `live` are one.
        let unknown = Sample { mimalloc: None, ..s };
        assert!(line("exit", &unknown).contains(&format!("{private} 543 MB (held 331)")), "{}", line("exit", &unknown));
    }

    /// mimalloc's own statistics give its committed bytes, read from the
    /// text as mimalloc writes it.
    #[test]
    fn mimalloc_says_what_it_committed() {
        let json = r#"{
  "reserved": { "total": 9, "peak": 9, "current": 9 },
  "committed": { "total": 5000, "peak": 4000, "current": 1234 },
  "reset": 0,
}"#;
        assert_eq!(stats_of(json), Some(Mimalloc { committed: 1234, peak: 4000, ..Default::default() }));
        assert_eq!(stats_of("{}"), None);
        let full = r#"{
  "pages": { "total": 7884, "peak": 352, "current": 317 },
  "committed": { "total": 1549991936, "peak": 159645696, "current": 37552128 },
  "page_committed": { "total": 0, "peak": 0, "current": 0 },
  "pages_abandoned": { "total": 1147, "peak": 63, "current": 32 },
  "threads": { "total": 94, "peak": 39, "current": 26 },
}"#;
        assert_eq!(
            stats_of(full),
            Some(Mimalloc { committed: 37_552_128, peak: 159_645_696, pages: 317, abandoned: 32, threads: 26 })
        );
        let m = mimalloc_stats().expect("mimalloc's statistics");
        assert!(m.committed > 0 && m.peak >= m.committed && m.threads > 0, "{m:?}");
    }

    /// The option places are mimalloc 3's: before the options are set each
    /// reads its default, after it the value set. (One test, so that no
    /// other sets them meanwhile.)
    #[test]
    fn the_options_are_set_where_they_are() {
        use libmimalloc_sys as mi;
        // SAFETY: plain option reads and a collect of this thread's heap.
        unsafe {
            assert_eq!(mi::mi_version() / 10_000, option::MAJOR, "the linked mimalloc is 3.x");
            for ((at, default), _) in TUNING {
                assert_eq!(mi::mi_option_get(at), default, "option {at} at its default");
            }
        }
        assert!(tune(), "{:?}", TUNED.get());
        // SAFETY: as above.
        unsafe {
            for ((at, _), value) in TUNING {
                assert_eq!(libmimalloc_sys::mi_option_get(at), value, "option {at} set");
            }
        }
        // Allocation goes on as before, and a collect after it is fine.
        let v: Vec<Vec<u8>> = (0..1000).map(|i| vec![1u8; 64 + i * 37]).collect();
        assert_eq!(v.iter().map(Vec::len).sum::<usize>(), 64 * 1000 + 37 * 999 * 1000 / 2);
        drop(v);
        // SAFETY: a plain call.
        unsafe { libmimalloc_sys::mi_collect(true) };
        assert!(!tune(), "a second time finds them set and leaves them");
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
