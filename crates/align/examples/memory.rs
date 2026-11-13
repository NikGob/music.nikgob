//! Memory of a small background pass.
//!
//! `cargo run -p nikgob-align --release --example memory -- <models dir> <jobs.json> <keep secs> [cpu]`
//!
//! Each job (`{"name", "path", "lines"}`, as for `measure`) is checked as
//! the background pass does: decoded, separated, its text fitted with its
//! language's aligner. Then the process idles a minute and frees the
//! models after `keep secs` without work, as the aligner's thread does.
//! The working set and private bytes are printed at each step, and the
//! peak at the end (Windows, through PowerShell).

use std::time::{Duration, Instant};

use nikgob_align::catalog::ModelId;
use nikgob_align::{Aligner, Catalog, Device, Stage};
use nikgob_core::lyrics::LyricLine;

#[derive(serde::Deserialize)]
struct Job {
    name: String,
    path: String,
    lines: Vec<LyricLine>,
}

mod mem {
    /// Working set, private bytes and their peaks, in MB (asked of the
    /// system, as Task Manager shows them).
    pub fn now() -> (f64, f64, f64, f64) {
        let script = format!(
            "$p = Get-Process -Id {}; '{{0}} {{1}} {{2}} {{3}}' -f $p.WorkingSet64, $p.PrivateMemorySize64, $p.PeakWorkingSet64, $p.PeakPagedMemorySize64",
            std::process::id()
        );
        let mut cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window of its own.
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let v: Vec<f64> = text.split_whitespace().filter_map(|n| n.parse().ok()).map(|b: f64| b / 1_048_576.0).collect();
        match v[..] {
            [a, b, c, d] => (a, b, c, d),
            _ => (0.0, 0.0, 0.0, 0.0),
        }
    }
}

fn report(t0: Instant, what: &str) {
    let (ws, private, peak_ws, peak_private) = mem::now();
    println!("{:6.1}s {what:44} working set {ws:7.0} MB  private {private:7.0} MB  (peak {peak_ws:7.0} / {peak_private:7.0} MB)", t0.elapsed().as_secs_f32());
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog = Catalog::new(&args[1]);
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let keep = Duration::from_secs(args[3].parse().unwrap());
    let device = if args.get(4).map(String::as_str) == Some("cpu") { Device::Cpu } else { Device::Gpu };
    let threads = std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(3);
    let t0 = Instant::now();
    report(t0, "start");
    nikgob_align::engine::load_runtime(&catalog.path(ModelId::Runtime)).unwrap();
    report(t0, "runtime loaded (the base)");
    let mut aligner = Aligner::new(catalog, device, threads);
    for job in &jobs {
        let mut prep = aligner.prepare(std::path::Path::new(&job.path), true, &mut |_: Stage| true).unwrap();
        report(t0, &format!("{}: separated", job.name.chars().take(30).collect::<String>()));
        let fit = aligner.fit(&mut prep, &job.lines, true, &mut |_| true).unwrap();
        report(t0, &format!("{}: fitted ({} lines)", job.name.chars().take(30).collect::<String>(), fit.map_or(0, |f| f.lines.len())));
    }
    drop(jobs);
    let done = Instant::now();
    let mut freed = false;
    while done.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_secs(5));
        if !freed && done.elapsed() >= keep {
            aligner.unload();
            freed = true;
            report(t0, "models freed");
        } else {
            report(t0, "idle");
        }
    }
}
