//! Is another process' sound, captured by WASAPI process loopback and played
//! through our engine, smooth enough to be the copy that is heard?
//!
//! The parent starts this example again as a child in `tone` mode: the child
//! plays a quiet 300 Hz tone through our engine with a louder one-period
//! marker every second (its height tells four markers apart). The parent
//! sets the child's audio session the way Soloist's would be set, captures
//! the child by process id with the crate's [`ProcessCapture`] and plays the
//! capture through our engine as a live source, either [`SoloistTrack`]
//! itself (`--track`, behind a stand-in for the manager) or a copy of its
//! reading with the knobs this bench turns (prebuffer, cut-back threshold).
//!
//! Drift shows as the slope of the latency over the run. There is no drift
//! correction knob: the backlog a clock difference builds sits in the
//! engine's ring (the decoder drains the capture every few milliseconds),
//! which a source cannot see, so the capture's backlog stays near one packet
//! whatever the clocks do.
//!
//! Both processes watch their own engine's visualizer tap, which the output
//! callback writes: a marker's time is the first poll that saw the callback
//! write it, so `parent - child` is what the route adds between the two
//! callbacks. Both engines buffer the same 90 ms after their callback, so it
//! is also how much later the routed copy is heard than the original would
//! be. A 300 Hz tone repeats every 160 samples at 48 kHz and 147 at 44.1 kHz,
//! so every sample that differs from the one a period earlier (outside a
//! marker) is a gap, a drop or a jump.
//!
//! `cargo run --release -p nikgob-soloist --example route_bench -- [options]`
//!
//! * `--secs 180`: length of the measured run.
//! * `--session 1e-4`: the child's session level (`mute` mutes it instead).
//! * `--levels`: first measure the capture at several session levels.
//! * `--track`: play through [`SoloistTrack`] (its constants) instead of the
//!   bench's source.
//! * `--prebuffer 40 --cut 160`: the bench source's prebuffer and the backlog
//!   it cuts back to the prebuffer, in ms.
//! * `--load 6 --ui`: busy threads and a 144 Hz UI-like loop meanwhile.
//! * `--mmcss`: the capture thread in MMCSS "Pro Audio" (needs the crate's
//!   [`CaptureOptions`]).
//! * `--child-device NAME`: the child plays on another output (`devices`
//!   lists them), so the two clocks differ.
//! * `--vol 0`: our engine's volume (the tap reads before it, so 0 keeps the
//!   run silent).
//! * `--parent-device NAME`: our engine plays there.
//! * `--listen NAME --markers-only`: what is heard. Both engines play on a
//!   virtual cable (`--child-device` and `--parent-device` its input, the
//!   child's session at 1 and `--vol 1`), the child plays only the markers,
//!   and the bench records the cable's output: every marker comes twice,
//!   the original and our copy, and their distance is the latency as heard,
//!   whatever either engine's device buffer holds.
#![allow(unsafe_code)]

#[cfg(not(windows))]
fn main() {
    eprintln!("route_bench measures WASAPI process loopback: Windows only");
}

#[cfg(windows)]
fn main() {
    bench::main();
}

#[cfg(windows)]
mod bench {
    use std::collections::{HashMap, VecDeque};
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command as Proc, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use nikgob_audio::{
        Device, PcmError, PcmRead, PcmSource, PlayItem, Player, PlayerOptions, SNAPSHOT_LEN,
        VisualizerTap,
    };
    use nikgob_soloist::capture::{CaptureOptions, ProcessCapture, set_sessions};
    use nikgob_soloist::protocol::{Anchor, Command, Status};
    use nikgob_soloist::track::Link;
    use nikgob_soloist::{LiveView, SoloistTrack};
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes, SetThreadPriority,
        THREAD_PRIORITY_HIGHEST,
    };

    const TONE_HZ: u64 = 300;
    const SRC_RATE: u64 = 48_000;
    /// One marker a second.
    const MARK_EVERY: u64 = 48_000;
    const URI: &str = "spotify:track:bench";

    fn unix_ns() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64)
    }

    fn ft_ns(f: FILETIME) -> u64 {
        ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) * 100
    }

    fn process_cpu_ns() -> u64 {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = unsafe { GetProcessTimes(GetCurrentProcess(), &raw mut c, &raw mut e, &raw mut k, &raw mut u) };
        ft_ns(k) + ft_ns(u)
    }

    fn thread_cpu_ns() -> u64 {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = unsafe { GetThreadTimes(GetCurrentThread(), &raw mut c, &raw mut e, &raw mut k, &raw mut u) };
        ft_ns(k) + ft_ns(u)
    }

    struct Args {
        mode: String,
        values: HashMap<String, String>,
    }

    impl Args {
        fn parse() -> Self {
            let mut it = std::env::args().skip(1).peekable();
            let mode = match it.peek() {
                Some(m) if !m.starts_with("--") => it.next().unwrap_or_default(),
                _ => "bench".to_owned(),
            };
            let mut values = HashMap::new();
            while let Some(a) = it.next() {
                let Some(k) = a.strip_prefix("--") else { continue };
                let v = match it.peek() {
                    Some(v) if !v.starts_with("--") => it.next().unwrap_or_default(),
                    _ => "1".to_owned(),
                };
                values.insert(k.to_owned(), v);
            }
            Self { mode, values }
        }
        fn get<T: std::str::FromStr>(&self, k: &str, default: T) -> T {
            self.values.get(k).and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        fn has(&self, k: &str) -> bool {
            self.values.contains_key(k)
        }
        fn text(&self, k: &str) -> Option<String> {
            self.values.get(k).cloned()
        }
    }

    pub fn main() {
        let args = Args::parse();
        match args.mode.as_str() {
            "tone" => tone(&args),
            "devices" => {
                for d in nikgob_audio::output_device_names().unwrap_or_default() {
                    println!("{d}");
                }
            }
            _ => parent(&args),
        }
    }

    // ---------------------------------------------------------------- signal

    /// The child's signal at frame `n`: the tone, a period of it 3-6 times as
    /// high every second (the height is the marker's number mod 4).
    fn sample(n: u64, amp: f32, markers_only: bool) -> f32 {
        let p = SRC_RATE / TONE_HZ;
        let base = (std::f32::consts::TAU * (n % p) as f32 / p as f32).sin() * amp;
        if n % MARK_EVERY < p {
            base * (3 + (n / MARK_EVERY) % 4) as f32
        } else if markers_only {
            0.0
        } else {
            base
        }
    }

    struct Tone {
        n: u64,
        amp: Arc<AtomicU32>,
        markers_only: bool,
    }

    impl PcmSource for Tone {
        fn sample_rate(&self) -> u32 {
            SRC_RATE as u32
        }
        fn channels(&self) -> u16 {
            2
        }
        fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
            let a = f32::from_bits(self.amp.load(Ordering::Relaxed));
            for f in out.chunks_exact_mut(2) {
                let s = sample(self.n, a, self.markers_only);
                f[0] = s;
                f[1] = s;
                self.n += 1;
            }
            Ok(PcmRead::Frames(out.len() / 2))
        }
    }

    // --------------------------------------------------------------- watcher

    #[derive(Clone, Copy, Debug)]
    struct Mark {
        seq: u8,
        t_ns: u64,
    }

    #[derive(Clone, Copy, Debug, Default)]
    struct Health {
        /// Places where the tone broke (gap, drop, jump), 100 ms apart at least.
        events: u64,
        /// Runs of exact zeros (silence the engine filled in).
        zero_gaps: u64,
        zero_ms: f64,
        /// Samples the watcher itself missed (it was starved): not counted.
        lost: u64,
        /// Seconds of tone checked.
        checked_s: f64,
        /// Tone power over the power of what differs from a period earlier.
        snr_db: f64,
        amp: f32,
    }

    /// Follows an engine's visualizer tap: the callback times, the markers,
    /// the tone's breaks.
    struct Watcher {
        tap: Arc<VisualizerTap>,
        rate: u64,
        p: u64,
        /// Poll times at which `written` had grown, with the new value.
        cb: VecDeque<(u64, u64)>,
        last_w: u64,
        win: VecDeque<f32>,
        win_base: u64,
        ingested: u64,
        analysed: u64,
        started: bool,
        /// Calibration: block peaks until the tone's height is known.
        peaks: Vec<f32>,
        block_peak: f32,
        block_len: u64,
        amp: f32,
        marks_x: VecDeque<u64>,
        pending: VecDeque<u64>,
        quiet_until: u64,
        last_flag: Option<u64>,
        zero_run: u64,
        sig2: f64,
        res2: f64,
        health: Health,
        snap_a: Box<[f32; SNAPSHOT_LEN]>,
        snap_b: Box<[f32; SNAPSHOT_LEN]>,
    }

    impl Watcher {
        fn new(tap: Arc<VisualizerTap>) -> Self {
            Self {
                tap,
                rate: 48_000,
                p: 160,
                cb: VecDeque::new(),
                last_w: 0,
                win: VecDeque::new(),
                win_base: 0,
                ingested: 0,
                analysed: 0,
                started: false,
                peaks: Vec::new(),
                block_peak: 0.0,
                block_len: 0,
                amp: 0.0,
                marks_x: VecDeque::new(),
                pending: VecDeque::new(),
                quiet_until: 0,
                last_flag: None,
                zero_run: 0,
                sig2: 0.0,
                res2: 0.0,
                health: Health::default(),
                snap_a: Box::new([0.0; SNAPSHOT_LEN]),
                snap_b: Box::new([0.0; SNAPSHOT_LEN]),
            }
        }

        /// One poll; returns the markers resolved by it.
        fn tick(&mut self) -> Vec<Mark> {
            let now = unix_ns();
            let w = self.tap.written();
            if w != self.last_w {
                self.cb.push_back((now, w));
                self.last_w = w;
                while self.cb.len() > 4000 {
                    self.cb.pop_front();
                }
            }
            // The tap hands out what is heard (it lags by the callback's
            // delay); the newest samples are wanted, so the delay is set to
            // 0 just before each copy. The callback sets it again at its
            // start, so two copies must agree and `written` must not move.
            self.tap.set_delay(0);
            if !self.tap.snapshot(&mut self.snap_a) {
                return Vec::new();
            }
            self.tap.set_delay(0);
            let ok = self.tap.snapshot(&mut self.snap_b);
            if !ok || self.tap.written() != w || self.snap_a[..] != self.snap_b[..] {
                return Vec::new();
            }
            let rate = u64::from(self.tap.sample_rate());
            if rate != self.rate {
                self.rate = rate;
                self.p = rate / TONE_HZ;
            }
            let start = w - SNAPSHOT_LEN as u64;
            if !self.started {
                self.started = true;
                self.win_base = start;
                self.ingested = start;
                self.analysed = start + self.p;
            }
            if self.ingested < start {
                self.health.lost += start - self.ingested;
                self.win.clear();
                self.win_base = start;
                self.ingested = start;
                self.analysed = start + self.p;
                self.last_flag = None;
            }
            if self.ingested >= w {
                return Vec::new();
            }
            let from = (self.ingested - start) as usize;
            for i in from..SNAPSHOT_LEN {
                let s = self.snap_b[i];
                self.ingest(s);
            }
            self.analyse();
            let mut out = Vec::new();
            while let Some(&x) = self.pending.front() {
                if x + self.p > self.ingested || x < self.win_base {
                    break;
                }
                self.pending.pop_front();
                let lo = (x - self.win_base) as usize;
                let peak = self.win.range(lo..lo + self.p as usize).fold(0f32, |m, s| m.max(s.abs()));
                let seq = ((peak / self.amp).round() as i64 - 3).clamp(0, 3) as u8;
                // The callback wrote x at the first poll that saw `written`
                // pass it.
                let t = self.cb.iter().find(|(_, cw)| *cw > x).map_or(now, |(t, _)| *t);
                out.push(Mark { seq, t_ns: t });
            }
            let keep_from = self.analysed.saturating_sub(4 * self.p);
            while self.win_base < keep_from && !self.win.is_empty() {
                self.win.pop_front();
                self.win_base += 1;
            }
            out
        }

        fn ingest(&mut self, s: f32) {
            let m = self.ingested;
            self.win.push_back(s);
            self.ingested += 1;
            if self.amp == 0.0 {
                self.block_peak = self.block_peak.max(s.abs());
                self.block_len += 1;
                if self.block_len == self.p {
                    if self.block_peak > 0.0 {
                        self.peaks.push(self.block_peak);
                    }
                    self.block_peak = 0.0;
                    self.block_len = 0;
                    if self.peaks.len() >= 300 {
                        self.peaks.sort_by(f32::total_cmp);
                        self.amp = self.peaks[self.peaks.len() / 2];
                        self.health.amp = self.amp;
                    }
                }
                return;
            }
            if s.abs() > 2.0 * self.amp && m >= self.quiet_until {
                self.pending.push_back(m);
                self.marks_x.push_back(m);
                if self.marks_x.len() > 8 {
                    self.marks_x.pop_front();
                }
                self.quiet_until = m + 2 * self.p;
            }
        }

        fn analyse(&mut self) {
            if self.amp == 0.0 {
                self.analysed = self.ingested.saturating_sub(self.p);
                return;
            }
            let p = self.p;
            while self.analysed + p <= self.ingested {
                let n = self.analysed;
                self.analysed += 1;
                if n < self.win_base + p {
                    continue;
                }
                let s = self.win[(n - self.win_base) as usize];
                let q = self.win[(n - p - self.win_base) as usize];
                if s == 0.0 {
                    self.zero_run += 1;
                } else {
                    if self.zero_run >= 8 {
                        self.health.zero_gaps += 1;
                        self.health.zero_ms += self.zero_run as f64 * 1000.0 / self.rate as f64;
                    }
                    self.zero_run = 0;
                }
                let near_mark = self.marks_x.iter().any(|&x| n + p >= x && n <= x + 2 * p);
                if near_mark {
                    continue;
                }
                let d = (s - q).abs();
                self.sig2 += f64::from(s) * f64::from(s);
                self.health.checked_s += 1.0 / self.rate as f64;
                if d > 0.05 * self.amp {
                    if self.last_flag.is_none_or(|l| n > l + self.rate / 10) {
                        self.health.events += 1;
                    }
                    self.last_flag = Some(n);
                } else {
                    self.res2 += f64::from(d) * f64::from(d);
                }
            }
            if self.res2 > 0.0 {
                // The difference of two samples carries their noise twice.
                self.health.snr_db = 10.0 * (self.sig2 / (self.res2 / 2.0)).log10();
            } else if self.sig2 > 0.0 {
                self.health.snr_db = f64::INFINITY;
            }
        }
    }

    fn raise_priority() {
        let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST) };
    }

    fn output_options(device: Option<String>) -> PlayerOptions {
        let mut o = PlayerOptions::default();
        if let Some(d) = device {
            o.output.device = Device::Named(d);
        }
        o
    }

    // ----------------------------------------------------------------- child

    fn tone(args: &Args) {
        let amp: f32 = args.get("amp", 0.01);
        let (player, _events) = Player::spawn(output_options(args.text("device"))).expect("engine");
        player.set_volume(1.0);
        let level = Arc::new(AtomicU32::new(0f32.to_bits()));
        let tone = Tone { n: 0, amp: level.clone(), markers_only: args.has("markers-only") };
        player.play(PlayItem::pcm(1, Box::new(tone), None));
        let quit = Arc::new(AtomicBool::new(false));
        {
            let quit = quit.clone();
            std::thread::spawn(move || {
                for line in std::io::stdin().lock().lines() {
                    match line.as_deref().map(str::trim) {
                        Ok("go") => level.store(amp.to_bits(), Ordering::Relaxed),
                        Ok("quit") | Err(_) => break,
                        _ => {}
                    }
                }
                quit.store(true, Ordering::Release);
            });
        }
        raise_priority();
        let mut w = Watcher::new(player.visualizer_tap());
        let mut out = std::io::stdout().lock();
        let mut last_h = Instant::now();
        while !quit.load(Ordering::Acquire) {
            for m in w.tick() {
                let _ = writeln!(out, "M {} {}", m.seq, m.t_ns);
            }
            if last_h.elapsed() > Duration::from_secs(2) {
                last_h = Instant::now();
                let h = w.health;
                let _ = writeln!(
                    out,
                    "H {} {} {:.1} {} {} {} {:.0}",
                    h.events,
                    h.zero_gaps,
                    h.zero_ms,
                    h.lost,
                    player.underruns(),
                    player.late_callbacks(),
                    h.checked_s
                );
            }
            let _ = out.flush();
            std::thread::sleep(Duration::from_micros(500));
        }
        let h = w.health;
        let _ = writeln!(
            out,
            "H {} {} {:.1} {} {} {} {:.0}",
            h.events,
            h.zero_gaps,
            h.zero_ms,
            h.lost,
            player.underruns(),
            player.late_callbacks(),
            h.checked_s
        );
        let _ = out.flush();
    }

    // ---------------------------------------------------------------- parent

    /// The manager as [`SoloistTrack`] sees it: Soloist ready and playing our
    /// item, the capture behind it.
    struct BenchLink {
        cap: Arc<ProcessCapture>,
        gain: f32,
        changes: AtomicU64,
        start_ms: u64,
    }

    impl Link for BenchLink {
        fn snapshot(&self) -> LiveView {
            LiveView {
                ready: true,
                active: true,
                status: Status::Playing,
                uri: Some(URI.to_owned()),
                duration_ms: Some(36_000_000),
                anchor: Some(Anchor { position_ms: 0, timestamp_ms: self.start_ms, speed: 1.0 }),
                track_changes: self.changes.load(Ordering::Acquire),
                error: None,
                ..Default::default()
            }
        }
        fn send(&self, c: Command) -> bool {
            if matches!(c, Command::Play(_)) {
                self.changes.fetch_add(1, Ordering::AcqRel);
            }
            true
        }
        fn take_captured(&self, out: &mut [f32]) -> Option<usize> {
            Some(self.cap.take(out))
        }
        fn buffered_frames(&self) -> Option<usize> {
            Some(self.cap.buffered_frames())
        }
        fn clear_captured(&self) {
            self.cap.clear();
        }
        fn keep_captured(&self, frames: usize) {
            self.cap.keep_newest(frames);
        }
        fn capture_gain(&self) -> f32 {
            self.gain
        }
        fn paused_by_us(&self) -> bool {
            false
        }
        fn playing(&self, _delta: i32) {}
    }

    /// Notes when a marker comes out of the capture (read time on the
    /// decoder thread).
    struct ReadMarks {
        amp: f32,
        quiet: u64,
        peak: Option<(f32, u64, u64)>,
        n: u64,
        out: Arc<Mutex<Vec<Mark>>>,
    }

    impl ReadMarks {
        fn feed(&mut self, frames: &[f32]) {
            let p = SRC_RATE / TONE_HZ;
            let t = unix_ns();
            for f in frames.chunks_exact(2) {
                let s = f[0].abs();
                if let Some((peak, at, when)) = &mut self.peak {
                    *peak = peak.max(s);
                    if self.n >= *at + p {
                        let seq = ((*peak / self.amp).round() as i64 - 3).clamp(0, 3) as u8;
                        if let Ok(mut o) = self.out.lock() {
                            o.push(Mark { seq, t_ns: *when });
                        }
                        self.peak = None;
                    }
                } else if s > 2.0 * self.amp && self.n >= self.quiet {
                    self.peak = Some((s, self.n, t));
                    self.quiet = self.n + 2 * p;
                }
                self.n += 1;
            }
        }
    }

    /// [`SoloistTrack`]'s reading of the capture with the bench's knobs.
    struct BenchSource {
        cap: Arc<ProcessCapture>,
        gain: f32,
        pre: usize,
        cut: usize,
        prebuffered: bool,
        cuts: Arc<AtomicU64>,
        marks: ReadMarks,
    }

    impl PcmSource for BenchSource {
        fn sample_rate(&self) -> u32 {
            SRC_RATE as u32
        }
        fn channels(&self) -> u16 {
            2
        }
        fn read(&mut self, out: &mut [f32]) -> Result<PcmRead, PcmError> {
            let buffered = self.cap.buffered_frames();
            if !self.prebuffered {
                if buffered < self.pre {
                    return Ok(PcmRead::Pending);
                }
                self.prebuffered = true;
            }
            if buffered > self.cut {
                self.cap.keep_newest(self.pre);
                self.cuts.fetch_add(1, Ordering::Relaxed);
            }
            let want = out.len() / 2;
            let n = self.cap.take(&mut out[..want * 2]) / 2;
            if n == 0 {
                return Ok(PcmRead::Pending);
            }
            let g = self.gain;
            if g > 1e-9 && g < 0.999 {
                let k = 1.0 / g;
                for s in &mut out[..n * 2] {
                    *s = (*s * k).clamp(-1.0, 1.0);
                }
            }
            self.marks.feed(&out[..n * 2]);
            Ok(PcmRead::Frames(n))
        }
    }

    fn percentile(v: &mut [f64], q: f64) -> f64 {
        if v.is_empty() {
            return f64::NAN;
        }
        v.sort_by(f64::total_cmp);
        let i = ((v.len() - 1) as f64 * q).round() as usize;
        v[i]
    }

    /// Least-squares slope of y over x.
    fn slope(points: &[(f64, f64)]) -> f64 {
        let n = points.len() as f64;
        if n < 2.0 {
            return f64::NAN;
        }
        let (sx, sy) = points.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let (mx, my) = (sx / n, sy / n);
        let (mut num, mut den) = (0.0, 0.0);
        for (x, y) in points {
            num += (x - mx) * (y - my);
            den += (x - mx) * (x - mx);
        }
        num / den
    }

    /// Latencies (ms) of `later` behind the matching `earlier` markers, with
    /// the time (s since `t0`) of each.
    fn match_marks(earlier: &[Mark], later: &[Mark], t0: u64) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        for m in later {
            let best = earlier
                .iter()
                .filter(|e| e.seq == m.seq && e.t_ns <= m.t_ns && m.t_ns - e.t_ns < 2_500_000_000)
                .map(|e| e.t_ns)
                .max();
            if let Some(e) = best {
                out.push((m.t_ns.saturating_sub(t0) as f64 / 1e9, (m.t_ns - e) as f64 / 1e6));
            }
        }
        out
    }

    fn session(pid: u32, mode: &str) -> usize {
        let r = if mode == "mute" {
            set_sessions(&[pid], Some(true), None)
        } else {
            set_sessions(&[pid], Some(false), Some(mode.parse().unwrap_or(1.0)))
        };
        r.unwrap_or(0)
    }

    /// Captured level (dBFS, rescaled by `1/gain`) and the periodicity SNR.
    fn level(cap: &ProcessCapture, gain: f32, secs: f32) -> (f64, f64) {
        cap.clear();
        let end = Instant::now() + Duration::from_secs_f32(secs);
        let mut left = Vec::new();
        let mut buf = vec![0f32; 9600];
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
            loop {
                let got = cap.take(&mut buf);
                if got == 0 {
                    break;
                }
                left.extend(buf[..got].chunks_exact(2).map(|f| f[0] / gain));
            }
        }
        let p = (SRC_RATE / TONE_HZ) as usize;
        let rms = (left.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / left.len().max(1) as f64).sqrt();
        let amp = rms * std::f64::consts::SQRT_2;
        // How far the rescaled capture is from a pure sine fitted to it: a
        // sample that differs from the one a period earlier is a gap, but a
        // coarse level step would keep the periodicity, so the precision is
        // measured against the fit.
        let keep: Vec<usize> = (3 * p..left.len().saturating_sub(3 * p))
            .filter(|&n| !left[n - 3 * p..n + 3 * p].iter().any(|s| f64::from(s.abs()) > 1.5 * amp))
            .collect();
        let phase = |n: usize| std::f64::consts::TAU * (n % p) as f64 / p as f64;
        let (mut c, mut si) = (0.0, 0.0);
        for &n in &keep {
            c += f64::from(left[n]) * phase(n).cos();
            si += f64::from(left[n]) * phase(n).sin();
        }
        let k = 2.0 / keep.len().max(1) as f64;
        let (c, si) = (c * k, si * k);
        let (mut sig, mut res) = (0.0, 0.0);
        for &n in &keep {
            let fit = c * phase(n).cos() + si * phase(n).sin();
            sig += fit * fit;
            res += (f64::from(left[n]) - fit).powi(2);
        }
        let db = if rms > 0.0 { 20.0 * rms.log10() } else { f64::NEG_INFINITY };
        let snr = if res > 0.0 { 10.0 * (sig / res).log10() } else { f64::INFINITY };
        (db, snr)
    }

    /// Records the input device `name` and notes the time of every marker
    /// pulse in it (unix ns), until the stream is dropped.
    fn listen(name: &str, amp: f32, out: Arc<Mutex<Vec<u64>>>) -> cpal::Stream {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = host
            .input_devices()
            .expect("input devices")
            .find(|d| d.to_string() == name)
            .unwrap_or_else(|| panic!("no input device {name}"));
        let config = device.default_input_config().expect("input config").config();
        let rate = u64::from(config.sample_rate);
        let ch = usize::from(config.channels);
        let p = rate / TONE_HZ;
        let mut n = 0u64;
        let mut quiet_until = 0u64;
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let now = unix_ns();
                    let frames = (data.len() / ch) as u64;
                    for (i, f) in data.chunks_exact(ch).enumerate() {
                        if f[0].abs() > 1.5 * amp && n >= quiet_until {
                            // The newest frame of a packet arrived just now.
                            let t = now - (frames - i as u64) * 1_000_000_000 / rate;
                            out.lock().unwrap().push(t);
                            quiet_until = n + 4 * p;
                        }
                        n += 1;
                    }
                },
                |e| eprintln!("listen: {e}"),
                None,
            )
            .expect("input stream");
        stream.play().expect("listen");
        eprintln!("listening to {name} at {rate} Hz");
        stream
    }

    fn parent(args: &Args) {
        let secs: u64 = args.get("secs", 180);
        let amp: f32 = args.get("amp", 0.01);
        let sess = args.text("session").unwrap_or_else(|| "1e-4".to_owned());
        let gain: f32 = if sess == "mute" { 1.0 } else { sess.parse().unwrap_or(1.0) };
        let loads: usize = args.get("load", 0);

        let exe = std::env::current_exe().expect("exe");
        let mut cmd = Proc::new(exe);
        cmd.args(["tone", "--amp", &amp.to_string()]);
        if let Some(d) = args.text("child-device") {
            cmd.args(["--device", &d]);
        }
        if args.has("markers-only") {
            cmd.arg("--markers-only");
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("child");
        let pid = child.id();
        let mut child_in = child.stdin.take().expect("stdin");
        let child_marks: Arc<Mutex<Vec<Mark>>> = Arc::default();
        let child_health: Arc<Mutex<String>> = Arc::default();
        {
            let out = child.stdout.take().expect("stdout");
            let (marks, health) = (child_marks.clone(), child_health.clone());
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let f: Vec<&str> = line.split_whitespace().collect();
                    match f.first() {
                        Some(&"M") if f.len() == 3 => {
                            if let (Ok(seq), Ok(t)) = (f[1].parse(), f[2].parse()) {
                                marks.lock().unwrap().push(Mark { seq, t_ns: t });
                            }
                        }
                        Some(&"H") => *health.lock().unwrap() = line.clone(),
                        _ => {}
                    }
                }
            });
        }

        // The child's session appears once its engine opened the device.
        let mut n = 0;
        let until = Instant::now() + Duration::from_secs(8);
        while n == 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(250));
            n = session(pid, &sess);
        }
        eprintln!("child {pid}: {n} session(s) set to {sess}");
        let opts = CaptureOptions { mmcss: args.has("mmcss"), ..CaptureOptions::default() };
        let cap = Arc::new(ProcessCapture::start_with(pid, opts).expect("process loopback"));
        let _ = writeln!(child_in, "go");
        let _ = child_in.flush();
        // The child's ring is about a second ahead of its callback.
        std::thread::sleep(Duration::from_millis(1500));

        if args.has("levels") {
            println!("capture level by session state (tone {:.1} dBFS RMS):", 20.0 * f64::from(amp / 2f32.sqrt()).log10());
            for (label, mode, g) in [
                ("1", "1.0", 1.0f32),
                ("1e-2", "0.01", 0.01),
                ("1e-3", "0.001", 0.001),
                ("1e-4", "0.0001", 0.0001),
                ("1e-5", "0.00001", 0.00001),
                ("1e-6", "0.000001", 0.000001),
                ("0", "0", 1.0),
                ("mute", "mute", 1.0),
            ] {
                session(pid, mode);
                std::thread::sleep(Duration::from_millis(300));
                let (db, snr) = level(&cap, g, 1.5);
                println!("  session {label:>5}: captured x1/{g:<8} {db:7.1} dBFS, SNR against a fitted sine {snr:6.1} dB");
            }
            // Unmuted first: a mute set by the last step stays otherwise.
            session(pid, "1e-4");
            session(pid, &sess);
            std::thread::sleep(Duration::from_millis(300));
            if secs == 0 {
                session(pid, "1.0");
                let _ = writeln!(child_in, "quit");
                let _ = child.wait();
                return;
            }
        }

        let (player, _events) = Player::spawn(output_options(args.text("parent-device"))).expect("engine");
        player.set_volume(args.get("vol", 0.0));
        let heard: Arc<Mutex<Vec<u64>>> = Arc::default();
        let listener = args.text("listen").map(|name| listen(&name, amp, heard.clone()));
        let cuts = Arc::new(AtomicU64::new(0));
        let read_marks: Arc<Mutex<Vec<Mark>>> = Arc::default();
        let track = args.has("track");
        let source: Box<dyn PcmSource> = if track {
            let link = Arc::new(BenchLink { cap: cap.clone(), gain, changes: AtomicU64::new(0), start_ms: unix_ns() / 1_000_000 });
            Box::new(SoloistTrack::new(link, URI.to_owned(), Some(Duration::from_secs(36_000))))
        } else {
            let ms = |v: u64| (v * SRC_RATE / 1000) as usize;
            Box::new(BenchSource {
                cap: cap.clone(),
                gain,
                pre: ms(args.get("prebuffer", 40)),
                cut: ms(args.get("cut", 160)),
                prebuffered: false,
                cuts: cuts.clone(),
                marks: ReadMarks { amp, quiet: 0, peak: None, n: 0, out: read_marks.clone() },
            })
        };
        player.play(PlayItem::pcm(1, source, None));

        let stop = Arc::new(AtomicBool::new(false));
        let parent_marks: Arc<Mutex<Vec<Mark>>> = Arc::default();
        let watcher = {
            let (stop, marks, tap) = (stop.clone(), parent_marks.clone(), player.visualizer_tap());
            std::thread::spawn(move || {
                raise_priority();
                let mut w = Watcher::new(tap);
                while !stop.load(Ordering::Acquire) {
                    let new = w.tick();
                    if !new.is_empty() {
                        marks.lock().unwrap().extend(new);
                    }
                    std::thread::sleep(Duration::from_micros(500));
                }
                (w.health, thread_cpu_ns())
            })
        };
        let mut loaders = Vec::new();
        for i in 0..loads {
            let stop = stop.clone();
            loaders.push(std::thread::spawn(move || {
                let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ i as u64;
                let mut acc = 0f64;
                while !stop.load(Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        acc += (x as f64).sqrt();
                    }
                }
                std::hint::black_box(acc);
                thread_cpu_ns()
            }));
        }
        if args.has("ui") {
            let stop = stop.clone();
            loaders.push(std::thread::spawn(move || {
                // A 144 Hz frame loop busy 4 of every 6.9 ms, like the
                // interface drawing Now Playing.
                let frame = Duration::from_micros(6944);
                let mut next = Instant::now();
                let mut acc = 0f64;
                while !stop.load(Ordering::Relaxed) {
                    let busy = Instant::now() + Duration::from_millis(4);
                    while Instant::now() < busy {
                        for k in 0..1000 {
                            acc += f64::from(k).sin();
                        }
                    }
                    next += frame;
                    if let Some(d) = next.checked_duration_since(Instant::now()) {
                        std::thread::sleep(d);
                    } else {
                        next = Instant::now();
                    }
                }
                std::hint::black_box(acc);
                thread_cpu_ns()
            }));
        }

        let t0 = unix_ns();
        let wall0 = Instant::now();
        let cpu0 = process_cpu_ns();
        let main0 = thread_cpu_ns();
        let under0 = player.underruns();
        let late0 = player.late_callbacks();
        let frames0 = cap.frames_captured();
        let mut backlog: Vec<(f64, f64)> = Vec::new();
        while wall0.elapsed() < Duration::from_secs(secs) {
            std::thread::sleep(Duration::from_millis(100));
            backlog.push((wall0.elapsed().as_secs_f64(), cap.buffered_frames() as f64 * 1000.0 / SRC_RATE as f64));
            let el = wall0.elapsed().as_secs();
            if el > 0 && el.is_multiple_of(30) && backlog.len().is_multiple_of(10) {
                eprintln!(
                    "{el:>4} s: underruns {} late {} marks {}/{}",
                    player.underruns() - under0,
                    player.late_callbacks() - late0,
                    parent_marks.lock().unwrap().len(),
                    child_marks.lock().unwrap().len()
                );
            }
        }
        let wall = wall0.elapsed().as_secs_f64();
        let captured = cap.frames_captured() - frames0;
        let main_cpu = thread_cpu_ns() - main0;
        stop.store(true, Ordering::Release);
        let (health, watcher_cpu) = watcher.join().expect("watcher");
        let load_cpu: u64 = loaders.into_iter().map(|h| h.join().unwrap_or(0)).sum();
        let cpu = process_cpu_ns() - cpu0;
        let underruns = player.underruns() - under0;
        let late = player.late_callbacks() - late0;
        player.stop();

        // Back to a normal level before the child goes, so Windows does not
        // keep the bench's level for the next run.
        session(pid, "1.0");
        let _ = writeln!(child_in, "quit");
        let _ = child_in.flush();
        let _ = child.wait();
        let st = cap.stats();
        drop(cap);

        drop(listener);
        let pulses = heard.lock().unwrap().clone();
        if !pulses.is_empty() {
            // Pairs: the original, then our copy well within the second
            // before the next marker.
            let mut heard_lat = Vec::new();
            let mut i = 0;
            while i + 1 < pulses.len() {
                let d = pulses[i + 1] - pulses[i];
                if d < 600_000_000 {
                    heard_lat.push(d as f64 / 1e6);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let n = heard_lat.len();
            let first: Vec<f64> = heard_lat.iter().take(n / 3).copied().collect();
            let last: Vec<f64> = heard_lat.iter().skip(n - n / 3).copied().collect();
            println!(
                "heard (cable output): {} pulses, {n} pairs, copy behind the original median {:.1} ms, p95 {:.1}, min {:.1}, max {:.1}; first third {:.1}, last third {:.1}",
                pulses.len(),
                percentile(&mut heard_lat.clone(), 0.5),
                percentile(&mut heard_lat.clone(), 0.95),
                percentile(&mut heard_lat.clone(), 0.0),
                percentile(&mut heard_lat, 1.0),
                percentile(&mut first.clone(), 0.5),
                percentile(&mut last.clone(), 0.5),
            );
        }

        let pm = parent_marks.lock().unwrap().clone();
        let cm = child_marks.lock().unwrap().clone();
        let rm = read_marks.lock().unwrap().clone();
        let lat = match_marks(&cm, &pm, t0);
        let mut l: Vec<f64> = lat.iter().map(|p| p.1).collect();
        let route = l.clone();
        let (med, p95, p99) = (percentile(&mut l, 0.5), percentile(&mut l, 0.95), percentile(&mut l, 0.99));
        let (lo, hi) = (l.first().copied().unwrap_or(f64::NAN), l.last().copied().unwrap_or(f64::NAN));
        let first: Vec<f64> = lat.iter().filter(|p| p.0 < 60.0).map(|p| p.1).collect();
        let last: Vec<f64> = lat.iter().filter(|p| p.0 > wall - 60.0).map(|p| p.1).collect();
        let drift = slope(&lat) * 60.0;
        let mut read_lat: Vec<f64> = match_marks(&cm, &rm, t0).iter().map(|p| p.1).collect();
        let mut out_lat: Vec<f64> = match_marks(&rm, &pm, t0).iter().map(|p| p.1).collect();
        let mut b: Vec<f64> = backlog.iter().filter(|p| p.0 > 5.0).map(|p| p.1).collect();
        let bslope = slope(&backlog.iter().copied().filter(|p| p.0 > 5.0).collect::<Vec<_>>()) * 60.0;
        let other = watcher_cpu + load_cpu + main_cpu;
        let route_cpu = cpu.saturating_sub(other) as f64 / 1e9 / wall * 100.0;

        println!(
            "run: {wall:.0} s, session {sess}, source {}, load {loads}{}, mmcss {}",
            if track { "SoloistTrack".to_owned() } else {
                format!("bench (prebuffer {} ms, cut {} ms)", args.get("prebuffer", 40), args.get("cut", 160))
            },
            if args.has("ui") { " + 144 Hz UI" } else { "" },
            args.has("mmcss"),
        );
        println!(
            "latency child callback -> our callback: median {med:.1} ms, p95 {p95:.1}, p99 {p99:.1}, min {lo:.1}, max {hi:.1} ({} of {} markers)",
            route.len(),
            cm.len()
        );
        if !read_lat.is_empty() {
            println!(
                "  of it: child callback -> read from the capture median {:.1} ms (p95 {:.1}); read -> our callback median {:.1} ms (p95 {:.1})",
                percentile(&mut read_lat.clone(), 0.5),
                percentile(&mut read_lat, 0.95),
                percentile(&mut out_lat.clone(), 0.5),
                percentile(&mut out_lat, 0.95)
            );
        }
        println!(
            "drift: latency first minute median {:.1} ms, last minute {:.1} ms, slope {drift:+.2} ms/min; capture backlog mean {:.1} ms (min {:.1}, max {:.1}), slope {bslope:+.2} ms/min; capture clock {:.1} frames/s",
            percentile(&mut first.clone(), 0.5),
            percentile(&mut last.clone(), 0.5),
            b.iter().sum::<f64>() / b.len().max(1) as f64,
            percentile(&mut b, 0.0),
            percentile(&mut b, 1.0),
            captured as f64 / wall
        );
        println!(
            "gaps: tone breaks {} , zero runs {} ({:.1} ms), engine underruns {underruns}, late callbacks {late}, cut-backs {}, watcher lost {} samples, checked {:.0} s, periodicity SNR {:.1} dB (inf: every period identical)",
            health.events,
            health.zero_gaps,
            health.zero_ms,
            cuts.load(Ordering::Relaxed),
            health.lost,
            health.checked_s,
            health.snr_db
        );
        println!(
            "capture: buffer {} frames, engine period {:.1} ms, packets {} of {}..{} frames, longest wait between packets {:.1} ms, discontinuities {}, mmcss {}",
            st.buffer_frames,
            st.period_hns as f64 / 10_000.0,
            st.packets,
            st.min_packet,
            st.max_packet,
            st.max_gap_us as f64 / 1000.0,
            st.discontinuities,
            st.mmcss
        );
        println!("child (the original) H events zero_gaps zero_ms lost underruns late checked_s: {}", child_health.lock().unwrap());
        println!(
            "cpu: process {:.2} % of a core; capture + engine {route_cpu:.2} % (watcher {:.2} %, load {:.0} %)",
            cpu as f64 / 1e9 / wall * 100.0,
            watcher_cpu as f64 / 1e9 / wall * 100.0,
            load_cpu as f64 / 1e9 / wall * 100.0
        );
    }
}
