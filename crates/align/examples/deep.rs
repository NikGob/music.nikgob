//! Deep mode against the normal timing on the shared bench
//! (docs/lyrics-align.md, «Deep mode»).
//!
//! `cargo run -p nikgob-align --release --example deep -- <models dir> <jobs.json> <cache dir> [name filter]`
//!
//! Jobs and the cache are the bench's (`examples/bench.rs`): the normal
//! separated voice and what the normal aligners hear are read from there
//! (a song without them is left out); what deep mode adds is kept there
//! too under names of its own: `<song>.rof.voice` / `.rof.levels` (the
//! voice by Mel-Band RoFormer), `<song>.x.<aligner>.em` for what an aligner
//! (`giga`, `sung` the second ones) hears in the bench's voice,
//! `<song>.rof.<aligner>.em` in the RoFormer voice; `.w.em` for a second
//! hearing over shifted windows. `OLD=1` takes the bench's own hearing
//! instead (made before the windows were cut at whole strides). Nothing
//! there is overwritten.
//!
//! What is measured is set by variables: `SEP=rof` (RoFormer's voice),
//! `WIN=1` (heard twice), `LEVEL=1` (quiet passages raised first; its
//! hearing kept as `<song>.lv.<aligner>.em`), `RU`, `EN`, `OTHER` = `alone` | `fused` | `product` (the
//! second aligner of Russian, English, the MMS languages), `W=0.5` (the first aligner's share
//! when fused), `OVERLAP=0` (RoFormer's chunks one after another).
//! `PREP=1` only fills the cache; `CACHED=1` runs no model (a song
//! missing something is left out). Every model run takes the round's GPU
//! lock (`LOCK=<dir>`) and, with `DUTY=0.5`, sleeps as long as it ran.
//! `DEVICE=cpu` and `THREADS` (2) say where models run. `TIME=1`: no
//! cache, each job made from its file as the player does (normal, then
//! deep as set), with time and peak memory printed.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nikgob_align::catalog::ModelId;
use nikgob_align::ctc::Emissions;
use nikgob_align::deep::{Deep, Second};
use nikgob_align::fit::Fit;
use nikgob_align::pipeline::Route;
use nikgob_align::{Aligner, Catalog, Device, Prepared, Stage};
use nikgob_core::lyrics::LyricLine;

#[derive(serde::Deserialize)]
struct Job {
    name: String,
    #[serde(default)]
    off: bool,
    path: String,
    kind: String,
    lines: Vec<LyricLine>,
    #[serde(default)]
    marks: Vec<(usize, usize, f32, f32)>,
}

fn key(id: ModelId) -> &'static str {
    match id {
        ModelId::AlignerRu => "ru",
        ModelId::AlignerJa => "ja",
        ModelId::AlignerEn => "en",
        ModelId::AlignerRuDeep => "giga",
        ModelId::AlignerSung => "sung",
        _ => "mms",
    }
}

fn read_f32(path: &Path) -> Option<Vec<f32>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    Some(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

/// Writes a cache file once (never over another's).
fn write_f32(path: &Path, head: &[f32], v: &[f32]) {
    if path.exists() {
        return;
    }
    let tmp = path.with_extension("tmp-deep");
    {
        let mut f = std::io::BufWriter::new(std::fs::File::create(&tmp).unwrap());
        for x in head.iter().chain(v) {
            f.write_all(&x.to_le_bytes()).unwrap();
        }
    }
    let _ = std::fs::rename(&tmp, path);
}

fn read_em(path: &Path) -> Option<Emissions> {
    let v = read_f32(path)?;
    Some(Emissions { logp: v[3..].to_vec(), vocab: v[0] as usize, fps: v[2], blank: v[1] as usize })
}

fn write_em(path: &Path, em: &Emissions) {
    write_f32(path, &[em.vocab as f32, em.blank as f32, em.fps], &em.logp);
}

/// The round's GPU lock and duty cycle around one model run.
fn model_run<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let lock = std::env::var("LOCK").ok().map(PathBuf::from);
    if let Some(l) = &lock {
        while std::fs::create_dir(l).is_err() {
            std::thread::sleep(Duration::from_secs(30));
        }
        let _ = std::fs::write(l.join("owner"), format!("r09-deep pid {} {what}", std::process::id()));
    }
    let t0 = Instant::now();
    let out = f();
    let took = t0.elapsed();
    if let Some(l) = &lock {
        let _ = std::fs::remove_dir_all(l);
    }
    println!("    {what}: {:.1} s", took.as_secs_f32());
    let duty: f32 = std::env::var("DUTY").ok().and_then(|d| d.parse().ok()).unwrap_or(0.0);
    if duty > 0.0 && duty < 1.0 {
        std::thread::sleep(took.mul_f32((1.0 - duty) / duty));
    }
    out
}

fn second_of(s: &str) -> Second {
    match s {
        "alone" => Second::Alone,
        "fused" => Second::Fused,
        "product" => Second::Product,
        _ => Second::Off,
    }
}

fn deep_from_env() -> Deep {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CHOSEN") == "1" {
        return Deep::CHOSEN;
    }
    Deep {
        separator: var("SEP") == "rof",
        separator_russian: var("SEP") == "rof",
        overlap: var("OVERLAP") != "0",
        windows: var("WIN") == "1",
        level: var("LEVEL") == "1",
        level_russian: var("LEVEL") == "ru",
        russian: second_of(&var("RU")),
        english: second_of(&var("EN")),
        other: second_of(&var("OTHER")),
        weight: var("W").parse().unwrap_or(0.5),
    }
}

/// The song from the cache, with what `deep` adds heard (and kept) now.
fn prepared(aligner: &mut Aligner, job: &Job, cache: &Path, deep: &Deep) -> Option<Prepared> {
    let stem = Path::new(&job.path).file_stem()?.to_string_lossy().into_owned();
    let rof = deep.separates(Route::of(&job.lines).map(|r| r.main));
    // What the aligners heard: the bench's own (`OLD=1`, made before the
    // windows were cut at whole strides) or this example's.
    let old = std::env::var("OLD").is_ok();
    // `CACHED=1`: no model runs, only what the cache has.
    let cached = old || std::env::var("CACHED").is_ok();
    // What an aligner heard, by its name in the cache: raised first
    // (`Deep::levels`) or not.
    let tag = |id: ModelId| match (rof, old, deep.levels(id)) {
        (true, _, false) => "rof.",
        (true, _, true) => "rof.lv.",
        (false, true, _) => "",
        (false, false, false) => "x.",
        (false, false, true) => "lv.",
    };
    let vtag = if rof { "rof." } else { "" };
    let voice_file = cache.join(format!("{stem}.{vtag}voice"));
    let levels_file = cache.join(format!("{stem}.{vtag}levels"));
    let voice = match read_f32(&voice_file) {
        Some(v) => v,
        None if rof && !cached => {
            aligner.set_deep(Deep { separator: true, ..Deep::OFF });
            let p = model_run(&format!("{} RoFormer separation", job.name), || aligner.prepare(Path::new(&job.path), true, &mut |_: Stage| true));
            aligner.set_deep(*deep);
            let p = match p {
                Ok(p) if p.deep_separated => p,
                Ok(_) => {
                    println!("{}: RoFormer failed", job.name);
                    return None;
                }
                Err(e) => {
                    println!("{}: {e}", job.name);
                    return None;
                }
            };
            write_f32(&voice_file, &[], &p.voice);
            write_f32(&levels_file, &[p.side().len() as f32], &[p.side(), p.mix()].concat());
            p.voice
        }
        None => return None,
    };
    let mut prep = Prepared::from_voice(voice, true);
    if let Some(l) = read_f32(&levels_file) {
        let n = l[0] as usize;
        prep.set_levels(l[1..1 + n].to_vec(), l[1 + n..].to_vec());
    }
    let route = Route::of(&job.lines)?;
    for id in route.models() {
        let file = cache.join(format!("{stem}.{}{}.em", tag(id), key(id)));
        let mut em = match read_em(&file) {
            Some(e) => e,
            None if !old && !cached => {
                let em = model_run(&format!("{} {}", job.name, key(id)), || aligner.hear_with(&prep, id, &mut |_| true)).ok()?;
                write_em(&file, &em);
                em
            }
            None => return None,
        };
        if deep.windows {
            let file = cache.join(format!("{stem}.{}{}.w.em", tag(id), key(id)));
            let other = match read_em(&file) {
                Some(e) => e,
                None if cached => return None,
                None => {
                    let mut p = Prepared::from_voice(prep.voice.clone(), true);
                    p.set_emissions(id, em.clone());
                    let e = model_run(&format!("{} {} shifted", job.name, key(id)), || aligner.hear_shifted(&p, id)).ok()?;
                    write_em(&file, &e);
                    e
                }
            };
            em.merge(&other);
        }
        prep.set_emissions(id, em);
        if let Some((sid, how)) = deep.second(id) {
            let file = cache.join(format!("{stem}.{}{}.em", tag(sid), key(sid)));
            let sem = match read_em(&file) {
                Some(e) => e,
                None if cached => return None,
                None => {
                    let e = model_run(&format!("{} {}", job.name, key(sid)), || aligner.hear_with(&prep, sid, &mut |_| true));
                    match e {
                        Ok(e) => {
                            write_em(&file, &e);
                            e
                        }
                        Err(e) => {
                            println!("{}: {}: {e}", job.name, key(sid));
                            continue;
                        }
                    }
                }
            };
            let (dem, vocab) = aligner.second_heard(&prep, id, sid, how, &sem).ok()?;
            prep.set_deep_emissions(id, dem, vocab);
        }
    }
    aligner.unload();
    Some(prep)
}

fn word_errors(reference: &[LyricLine], made: &[LyricLine]) -> (Vec<f32>, Vec<f32>) {
    let (mut starts, mut ends) = (Vec::new(), Vec::new());
    for (r, g) in reference.iter().zip(made) {
        if r.text.trim().is_empty() || r.background || g.words.is_empty() {
            continue;
        }
        let mut at = 0;
        let pieces: Vec<(usize, usize, f32, f32)> = g
            .words
            .iter()
            .map(|w| {
                let n = w.text.chars().filter(|c| !c.is_whitespace()).count();
                let p = (at, at + n, w.start.as_secs_f32(), w.end.as_secs_f32());
                at += n;
                p
            })
            .collect();
        let mut at = 0;
        for rw in &r.words {
            let n = rw.text.chars().filter(|c| !c.is_whitespace()).count();
            if n == 0 {
                continue;
            }
            let (a, b) = (at, at + n);
            at = b;
            if let Some(p) = pieces.iter().find(|p| p.0 <= a && a < p.1.max(p.0 + 1)) {
                starts.push(p.2 - rw.start.as_secs_f32());
            }
            if let Some(p) = pieces.iter().find(|p| p.0 < b && b <= p.1)
                && p.1 == b
            {
                ends.push(p.3 - rw.end.as_secs_f32());
            }
        }
    }
    (starts, ends)
}

fn line_errors(reference: &[LyricLine], made: &[LyricLine]) -> Vec<f32> {
    reference
        .iter()
        .zip(made)
        .filter(|(r, _)| !r.text.trim().is_empty() && !r.background)
        .map(|(r, g)| g.words.first().map_or(g.start, |w| w.start).as_secs_f32() - r.start.as_secs_f32())
        .collect()
}

fn mark_errors(marks: &[(usize, usize, f32, f32)], lines: &[LyricLine], made: &[LyricLine]) -> (Vec<f32>, Vec<f32>) {
    let sung: Vec<&LyricLine> = lines.iter().filter(|l| !l.text.trim().is_empty()).collect();
    let mut kept: Vec<&LyricLine> = Vec::new();
    for l in made {
        if kept.len() < sung.len() && l.text == sung[kept.len()].text {
            kept.push(l);
        }
    }
    let (mut starts, mut ends) = (Vec::new(), Vec::new());
    for &(li, wi, start, end) in marks {
        let Some(w) = kept.get(li).and_then(|l| l.words.get(wi)) else {
            starts.push(f32::INFINITY);
            continue;
        };
        starts.push(w.start.as_secs_f32() - start);
        if end >= 0.0 {
            ends.push(w.end.as_secs_f32() - end);
        }
    }
    (starts, ends)
}

fn within(v: &[f32], t: f32) -> f32 {
    v.iter().filter(|e| e.abs() < t).count() as f32 / v.len().max(1) as f32 * 100.0
}

fn plain(lines: &[LyricLine]) -> Vec<LyricLine> {
    lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| LyricLine::new(Duration::ZERO, l.text.clone())).collect()
}

#[derive(Default, Clone)]
struct Tally {
    starts: Vec<f32>,
    ends: Vec<f32>,
    lines: Vec<f32>,
}

impl Tally {
    fn add(&mut self, o: &Tally) {
        self.starts.extend(&o.starts);
        self.ends.extend(&o.ends);
        self.lines.extend(&o.lines);
    }

    fn show(&self) -> String {
        format!(
            "words {:4} start<0.3 {:5.1}% end<0.3 {:5.1}% | lines {:3} <0.3 {:5.1}%",
            self.starts.len(),
            within(&self.starts, 0.3),
            within(&self.ends, 0.3),
            self.lines.len(),
            within(&self.lines, 0.3)
        )
    }
}

fn verdict(f: &Fit) -> String {
    format!("heard {:.2} spelled {:.2} {:?} {:+.2}", f.heard, f.spelled, f.timing, f.offset)
}

/// One job against its references.
fn score(job: &Job, lines: &[LyricLine], timed: bool, fit: &Fit) -> (Tally, String) {
    let mut t = Tally::default();
    let reference: Vec<LyricLine> = if timed { job.lines.clone() } else { job.lines.iter().filter(|l| !l.text.trim().is_empty()).cloned().collect() };
    let mut made = fit.lines.clone();
    if made.len() > lines.len() && job.kind != "case" {
        let mut kept = Vec::new();
        for l in &made {
            if kept.len() < lines.len() && l.text == lines[kept.len()].text {
                kept.push(l.clone());
            }
        }
        if kept.len() == lines.len() {
            made = kept;
        }
    }
    let mut far = String::new();
    match job.kind.as_str() {
        "words" => {
            (t.starts, t.ends) = word_errors(&reference, &made);
            t.lines = line_errors(&reference, &made);
        }
        "lines" => t.lines = line_errors(&reference, &made),
        _ => {
            let (s, e) = mark_errors(&job.marks, lines, &fit.lines);
            far = job.marks.iter().zip(&s).filter(|(_, e)| e.abs() >= 0.3).map(|(m, e)| format!("{}/{} {:+.2}", m.0, m.1, e)).collect::<Vec<_>>().join(", ");
            t.starts = s;
            t.ends = e;
        }
    }
    (t, far)
}

mod mem {
    /// Peak working set and peak private bytes so far, MB.
    pub fn peak() -> (f64, f64) {
        let script = format!("$p = Get-Process -Id {}; '{{0}} {{1}}' -f $p.PeakWorkingSet64, $p.PeakPagedMemorySize64", std::process::id());
        let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output();
        let text = out.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
        let v: Vec<f64> = text.split_whitespace().filter_map(|n| n.parse().ok()).map(|b: f64| b / 1_048_576.0).collect();
        match v[..] {
            [a, b] => (a, b),
            _ => (0.0, 0.0),
        }
    }

    /// The card's used memory (MB), as `nvidia-smi` says.
    pub fn vram() -> f64 {
        let out = std::process::Command::new("nvidia-smi").args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"]).output();
        out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().lines().next().and_then(|l| l.trim().parse().ok())).unwrap_or(0.0)
    }
}

/// `TIME=1`: one song made from its file as the player makes it, with
/// time, the process's peak memory and the card's (its most used memory
/// over what it used before).
fn timed_run(aligner: &mut Aligner, job: &Job, deep: Deep) {
    aligner.set_deep(deep);
    let mut line = String::new();
    model_run(&format!("{} whole", job.name), || {
        // Measured from here, under the lock: the card's memory over what
        // it used just before (another program's use may still move it).
        let base = mem::vram();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let peak = std::sync::Arc::new(std::sync::Mutex::new(base));
        let watcher = {
            let (stop, peak) = (stop.clone(), peak.clone());
            std::thread::spawn(move || {
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let v = mem::vram();
                    let mut p = peak.lock().unwrap();
                    *p = p.max(v);
                    drop(p);
                    std::thread::sleep(Duration::from_millis(300));
                }
            })
        };
        let t0 = Instant::now();
        let mut sep = 0.0;
        let main = Route::of(&job.lines).map(|r| aligner.usable(r.main));
        let prep = aligner.prepare_for(Path::new(&job.path), true, main, &mut |_| true);
        sep += t0.elapsed().as_secs_f32();
        let (ok, used) = match prep {
            Ok(mut prep) => {
                let fit = aligner.fit(&mut prep, &job.lines, true, &mut |_| true);
                // What deep mode really used (a part that failed falls back).
                let used = format!("RoFormer {}, second aligner {}", prep.deep_separated, main.is_some_and(|m| prep.deep_emissions(m).is_some()));
                (fit.as_ref().is_ok_and(Option::is_some), used)
            }
            Err(e) => (false, e.to_string()),
        };
        let total = t0.elapsed().as_secs_f32();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = watcher.join();
        let vram = *peak.lock().unwrap() - base;
        let (ws, private) = mem::peak();
        line = format!(
            "  TIME {} {:?} ok {ok} ({used}) | decode and separation {sep:.1} s, all {total:.1} s | peak working set {ws:.0} MB private {private:.0} MB | card +{vram:.0} MB",
            job.name,
            aligner.device(),
        );
    });
    println!("{line}");
    aligner.unload();
}

/// `REF=<dir>`: RoFormer's port against the export's own reference (one
/// 8 s chunk, `ref-input.f32` and the vocal estimates of both exports).
fn parity(aligner: &mut Aligner, dir: &Path) {
    use nikgob_align::separate::{ROFORMER_CHUNK, vocals_roformer};
    let input = read_f32(&dir.join("ref-input.f32")).unwrap();
    let chans: Vec<Vec<f32>> = (0..2).map(|c| input.iter().skip(c).step_by(2).copied().collect()).collect();
    let mix = nikgob_align::audio::Audio { rate: 44_100, channels: chans };
    let catalog = aligner.catalog().clone();
    nikgob_align::engine::load_runtime(&catalog.path(ModelId::Runtime)).unwrap();
    let file = std::env::var("REF_MODEL").map(PathBuf::from).unwrap_or_else(|_| catalog.file(ModelId::SeparatorDeep, ModelId::SeparatorDeep.model_file()));
    let device = aligner.device();
    use nikgob_align::engine::GraphOptimizationLevel as O;
    let level = match std::env::var("OPT").as_deref() {
        Ok("0") => O::Disable,
        Ok("1") => O::Level1,
        Ok("2") => O::Level2,
        _ => O::Level3,
    };
    let mut model = nikgob_align::engine::Model::load_with(&file, device, 2, level).unwrap();
    let t0 = Instant::now();
    let out = model_run("RoFormer chunk", || vocals_roformer(&mut model, mix, ROFORMER_CHUNK, std::env::var("SPEC").is_err(), &mut |_| true)).unwrap();
    println!("one chunk on {:?}: {:.1} s", model.device, t0.elapsed().as_secs_f32());
    let got: Vec<f32> = (0..out.channels[0].len()).flat_map(|i| [out.channels[0][i], out.channels[1][i]]).collect();
    for name in ["ref-stems-all-int8.f32", "ref-stems-all-fp16.f32"] {
        let r = read_f32(&dir.join(name)).unwrap();
        let sig: f64 = r.iter().map(|v| (*v as f64).powi(2)).sum();
        let err: f64 = r.iter().zip(&got).map(|(a, b)| ((*a - *b) as f64).powi(2)).sum();
        println!("against {name}: SNR {:.1} dB", 10.0 * (sig / err.max(1e-20)).log10());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog = Catalog::new(&args[1]);
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let cache = PathBuf::from(&args[3]);
    let filter = args.get(4).map(|f| f.trim_matches('"').to_owned()).unwrap_or_default();
    let threads = std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(2);
    let device = if std::env::var("DEVICE").as_deref() == Ok("cpu") { Device::Cpu } else { Device::Gpu };
    let mut aligner = Aligner::new(catalog, device, threads);
    let deep = deep_from_env();
    aligner.set_deep(deep);
    println!("deep {deep:?} on {device:?}, jobs matching {filter:?}");
    if let Ok(dir) = std::env::var("REF") {
        parity(&mut aligner, Path::new(&dir));
        return;
    }
    if std::env::var("TIME").is_ok() {
        for job in jobs.iter().filter(|j| !j.off && j.name.contains(&filter)) {
            timed_run(&mut aligner, job, deep);
        }
        return;
    }
    let mut pooled: Vec<(String, Tally)> = Vec::new();
    for job in &jobs {
        if job.off || !job.name.contains(&filter) {
            continue;
        }
        if cache.join("stop").exists() {
            break;
        }
        let Some(mut prep) = prepared(&mut aligner, job, &cache, &deep) else {
            println!("-- {}: not in the cache", job.name);
            continue;
        };
        if std::env::var("PREP").is_ok() {
            println!("prepared {}", job.name);
            continue;
        }
        let main = Route::of(&job.lines).map(|r| r.main).unwrap();
        println!("== {} ({}, {})", job.name, job.kind, key(main));
        for timed in [true, false] {
            let lines = if timed { job.lines.clone() } else { plain(&job.lines) };
            let Ok(Some(fit)) = aligner.fit(&mut prep, &lines, timed, &mut |_| true) else {
                println!("  {}: no fit", if timed { "timed" } else { "plain" });
                continue;
            };
            let mode = if timed { "timed" } else { "plain" };
            let (t, far) = score(job, &lines, timed, &fit);
            println!("  {mode} {} | {}{}", t.show(), verdict(&fit), if far.is_empty() { String::new() } else { format!(" | off: {far}") });
            let group = if job.kind == "case" { format!("case {mode}") } else { format!("{} {} {mode}", job.kind, key(main)) };
            for g in [group, format!("all {mode}")] {
                match pooled.iter_mut().find(|(n, _)| *n == g) {
                    Some((_, p)) => p.add(&t),
                    None => pooled.push((g, t.clone())),
                }
            }
        }
    }
    println!("== pooled");
    pooled.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, t) in &pooled {
        println!("  {name:20} {}", t.show());
    }
}
