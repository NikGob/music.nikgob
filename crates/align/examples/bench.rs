//! The aligner's accuracy on a set of songs, quickly enough to try changes.
//!
//! `cargo run -p nikgob-align --release --example bench -- <models dir> <jobs.json> <cache dir> [name filter]`
//!
//! A job is `{"name", "path", "kind": "words"|"lines"|"case", "lines": [...],
//! "show": [[from, to], ...]}` with lines as `LyricLine` serialises:
//! "words" jobs hold a reference word timing (Apple's or lrc.red's), "lines"
//! jobs a reference line timing, "case" jobs only a text (its made words
//! in the `show` stretches are printed). The separated voice and what each
//! aligner hears are kept in the cache dir after the first run (the
//! separation is what takes minutes on the CPU), so a later run only
//! aligns. `PREP=1` fills the cache and stops; `CACHED=1` leaves out the
//! songs not in it yet. `DEVICE=gpu` runs the models on the GPU (else the
//! CPU, `THREADS` threads, 2 by default); `DUTY=0.5` keeps them busy only
//! that share of the time (after each chunk a model runs, the bench sleeps
//! as long again), so a run leaves the machine usable. Each text is aligned with
//! its line times (as the player does for a timed text) and as a plain
//! text (no times: what a Genius text gets), and another song's text of
//! the same language as a plain text, for the check. `EMS=x.` reads and
//! keeps the hearings as `<stem>.x.<aligner>.em` (another hearing of the
//! same voice: the fixed 50-a-second slicing). `SHOW=150-176` prints a
//! case's made words in those seconds instead of its `show` stretches.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_align::catalog::ModelId;
use nikgob_align::ctc::Emissions;
use nikgob_align::fit::{Fit, Tuning};
use nikgob_align::pipeline::Route;
use nikgob_align::{Aligner, Catalog, Device, Prepared, Stage};
use nikgob_core::lyrics::LyricLine;

#[derive(serde::Deserialize)]
struct Job {
    name: String,
    /// Left out of the runs (kept so the cache's numbering stays).
    #[serde(default)]
    off: bool,
    /// Prepared in this order (lowest first; else the jobs' order).
    #[serde(default)]
    rank: Option<usize>,
    path: String,
    kind: String,
    lines: Vec<LyricLine>,
    #[serde(default)]
    show: Vec<(f32, f32)>,
    /// What is sung where in a "case" job: `[line, word, start, end]`,
    /// the line counted among the text's non-empty lines, the word among
    /// its made words, seconds (an end below zero is not checked).
    #[serde(default)]
    marks: Vec<(usize, usize, f32, f32)>,
}

fn key(id: ModelId) -> &'static str {
    match id {
        ModelId::AlignerRu => "ru",
        ModelId::AlignerJa => "ja",
        ModelId::AlignerEn => "en",
        _ => "mms",
    }
}

fn read_f32(path: &Path) -> Option<Vec<f32>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path).ok()?.read_to_end(&mut bytes).ok()?;
    Some(bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
}

fn write_f32(path: &Path, head: &[f32], v: &[f32]) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for x in head.iter().chain(v) {
        f.write_all(&x.to_le_bytes()).unwrap();
    }
}

/// Sleeps after a stretch of model work so the model runs only `DUTY` of
/// the time (`DUTY=0.5`: as long again as the work took). Called from the
/// models' progress, so a long separation is spread out chunk by chunk.
struct Duty {
    share: f32,
    since: std::time::Instant,
}

impl Duty {
    fn from_env() -> Self {
        let share = std::env::var("DUTY").ok().and_then(|d| d.parse().ok()).unwrap_or(1.0f32).clamp(0.05, 1.0);
        Self { share, since: std::time::Instant::now() }
    }

    /// A model call starts now.
    fn start(&mut self) {
        self.since = std::time::Instant::now();
    }

    /// A chunk of model work ended: rest, then count from here.
    fn rest(&mut self) {
        if self.share < 1.0 {
            let took = self.since.elapsed().as_secs_f32();
            std::thread::sleep(Duration::from_secs_f32(took * (1.0 - self.share) / self.share));
        }
        self.since = std::time::Instant::now();
    }
}

/// The song's voice and what its aligners hear, from the cache or made
/// (and kept) now.
fn prepared(aligner: &mut Aligner, job: &Job, cache: &Path, duty: &mut Duty) -> Option<Prepared> {
    // Kept by the audio file's name: jobs on one song share it.
    let stem = Path::new(&job.path).file_stem()?.to_string_lossy().into_owned();
    let voice_file = cache.join(format!("{stem}.voice"));
    if std::env::var("CACHED").is_ok() && !voice_file.exists() {
        return None;
    }
    let levels = cache.join(format!("{stem}.levels"));
    let mut prep = match read_f32(&voice_file) {
        Some(v) => {
            let mut p = Prepared::from_voice(v, true);
            if let Some(l) = read_f32(&levels) {
                let n = l[0] as usize;
                p.set_levels(l[1..1 + n].to_vec(), l[1 + n..].to_vec());
            }
            p
        }
        None => {
            duty.start();
            let t0 = std::time::Instant::now();
            let p = match aligner.prepare(Path::new(&job.path), true, &mut |_: Stage| {
                duty.rest();
                true
            }) {
                Ok(p) => p,
                Err(e) => {
                    println!("{}: {e}", job.name);
                    return None;
                }
            };
            println!("{}: decoded and separated in {:.1} s (with rests)", job.name, t0.elapsed().as_secs_f32());
            write_f32(&voice_file, &[], &p.voice);
            write_f32(&levels, &[p.side().len() as f32], &[p.side(), p.mix()].concat());
            p
        }
    };
    // Every aligner any tuning may route a line to (Latin lines by sound
    // or not), so measuring never runs a model.
    let route = Route::of_with(&job.lines, false, false, true)?;
    for id in route.models() {
        // `EMS=x.`: another set of hearings kept beside these (`<stem>.x.ru.em`).
        let ems = std::env::var("EMS").unwrap_or_default();
        let file = cache.join(format!("{stem}.{ems}{}.em", key(id)));
        match read_f32(&file) {
            Some(v) => {
                let (vocab, blank, fps) = (v[0] as usize, v[1] as usize, v[2]);
                prep.set_emissions(id, Emissions { logp: v[3..].to_vec(), vocab, fps, blank });
            }
            // `CACHED=1` never runs a model.
            None if std::env::var("CACHED").is_ok() => return None,
            None => {
                // Heard once here; the second hearing is kept apart below.
                let tuning = aligner.tuning();
                aligner.set_tuning(Tuning { windows: false, ..tuning });
                duty.start();
                let t0 = std::time::Instant::now();
                aligner
                    .hear(&mut prep, id, &mut |_| {
                        duty.rest();
                        true
                    })
                    .unwrap();
                aligner.set_tuning(tuning);
                println!("{}: heard by {} in {:.1} s (with rests)", job.name, key(id), t0.elapsed().as_secs_f32());
                let em = prep.emissions(id).unwrap();
                write_f32(&file, &[em.vocab as f32, em.blank as f32, em.fps], &em.logp);
            }
        }
        // Over windows cut half a window later (`Tuning::windows`).
        if aligner.tuning().windows {
            let file = cache.join(format!("{stem}.{ems}{}.w.em", key(id)));
            let other = match read_f32(&file) {
                Some(v) => Emissions { logp: v[3..].to_vec(), vocab: v[0] as usize, fps: v[2], blank: v[1] as usize },
                None => {
                    duty.start();
                    let em = aligner.hear_shifted(&prep, id).unwrap();
                    duty.rest();
                    write_f32(&file, &[em.vocab as f32, em.blank as f32, em.fps], &em.logp);
                    em
                }
            };
            let mut em = prep.emissions(id).unwrap().clone();
            em.merge(&other);
            prep.set_emissions(id, em);
        }
    }
    Some(prep)
}

/// Each reference word against the made piece it starts in (its start) and
/// the made piece its last letter is in (its end, only where that piece
/// ends with it): made − reference, seconds.
fn word_errors(reference: &[LyricLine], made: &[LyricLine]) -> (Vec<f32>, Vec<f32>) {
    let (mut starts, mut ends) = (Vec::new(), Vec::new());
    for (r, g) in reference.iter().zip(made) {
        if r.text.trim().is_empty() || r.background || g.words.is_empty() {
            continue;
        }
        // Made pieces by the letters they cover, spaces left out.
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

/// Neighbouring sung lines that overlap (one ends after the next starts).
fn overlaps(made: &[LyricLine]) -> usize {
    let sung: Vec<&LyricLine> = made.iter().filter(|l| !l.background && !l.words.is_empty()).collect();
    sung.windows(2).filter(|w| w[0].end.is_some_and(|e| e.as_secs_f32() > w[1].start.as_secs_f32() + 0.01)).count()
}

#[derive(Default, Clone)]
struct Tally {
    starts: Vec<f32>,
    ends: Vec<f32>,
    lines: Vec<f32>,
    overlaps: usize,
    /// Lines sung once more than the text has them, added.
    extra: usize,
}

impl Tally {
    fn add(&mut self, o: &Tally) {
        self.starts.extend(&o.starts);
        self.ends.extend(&o.ends);
        self.lines.extend(&o.lines);
        self.overlaps += o.overlaps;
        self.extra += o.extra;
    }
}

fn within(v: &[f32], t: f32) -> f32 {
    v.iter().filter(|e| e.abs() < t).count() as f32 / v.len().max(1) as f32 * 100.0
}

fn mae(v: &[f32]) -> f32 {
    v.iter().map(|e| e.abs()).sum::<f32>() / v.len().max(1) as f32
}

fn median(v: &[f32]) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s.get(s.len() / 2).copied().unwrap_or(0.0)
}

fn plain(lines: &[LyricLine]) -> Vec<LyricLine> {
    lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| LyricLine::new(Duration::ZERO, l.text.clone())).collect()
}

fn summary(label: &str, t: &Tally) -> String {
    format!(
        "{label:28} words {:5} start<0.3 {:5.1}% med {:+.3} end<0.3 {:5.1}% med {:+.3} (n {:4}) | lines {:4} start<0.3 {:5.1}% mae {:5.2}s med {:+5.2} | overlaps {} extra {}",
        t.starts.len(),
        within(&t.starts, 0.3),
        median(&t.starts),
        within(&t.ends, 0.3),
        median(&t.ends),
        t.ends.len(),
        t.lines.len(),
        within(&t.lines, 0.3),
        mae(&t.lines),
        median(&t.lines),
        t.overlaps,
        t.extra
    )
}

fn print_lines(lines: &[LyricLine], show: &[(f32, f32)]) {
    for l in lines {
        let s = l.start.as_secs_f32();
        if !show.iter().any(|(a, b)| (*a..=*b).contains(&s)) {
            continue;
        }
        println!("    {:7.2} {:>7} «{}»", s, l.end.map_or("-".into(), |e| format!("{:.2}", e.as_secs_f32())), l.text);
        for w in &l.words {
            println!("            {:7.2} {:7.2} {:?}", w.start.as_secs_f32(), w.end.as_secs_f32(), w.text);
        }
    }
}

/// Every tenth of a second in `show`: the voice's energy against its loud
/// level, the aligner's blank and its best letter.
fn table(aligner: &Aligner, prep: &Prepared, id: ModelId, show: &[(f32, f32)]) {
    let json = std::fs::read_to_string(aligner.catalog().file(id, "vocab.json")).unwrap();
    let vocab: std::collections::HashMap<String, usize> = serde_json::from_str(&json).unwrap();
    let letter = |i: usize| vocab.iter().find(|(_, v)| **v == i).map_or("?".into(), |(k, _)| k.clone());
    let em = prep.emissions(id).unwrap();
    let energy = prep.energy();
    let mut sorted = energy.to_vec();
    sorted.sort_by(f32::total_cmp);
    let peak = sorted[((sorted.len() - 1) as f32 * 0.95) as usize];
    let per = (em.fps / 10.0).round() as usize;
    for &(a, b) in show {
        for (i, e) in energy.iter().enumerate().take((b * 10.0) as usize).skip((a * 10.0) as usize) {
            let frames: Vec<usize> = (i * per..((i + 1) * per).min(em.frames())).collect();
            if frames.is_empty() {
                break;
            }
            let blank = frames.iter().map(|t| em.row(*t)[em.blank].exp()).sum::<f32>() / frames.len() as f32;
            let (best, p) = frames
                .iter()
                .map(|t| {
                    let r = em.row(*t);
                    (0..em.vocab).filter(|v| *v != em.blank).max_by(|a, b| r[*a].total_cmp(&r[*b])).map(|v| (v, r[v].exp())).unwrap()
                })
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .unwrap();
            let bar = "#".repeat(((e / peak) * 20.0).min(30.0) as usize);
            println!("      {:6.1}s blank {:4.2} {:>3} {:4.2} {}", i as f32 / 10.0, blank, letter(best), p, bar);
        }
    }
}

/// The made words against a case's marks: start errors and end errors.
fn mark_errors(marks: &[(usize, usize, f32, f32)], lines: &[LyricLine], made: &[LyricLine]) -> (Vec<f32>, Vec<f32>) {
    // The made lines as the text has them (lines added as repeats left out).
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

fn verdict(f: &Fit) -> String {
    format!(
        "score {:.2} heard {:.2} spelled {:.2} covered {:.2} {:?} {:+.2}/{:.2}",
        f.score, f.heard, f.spelled, f.covered, f.timing, f.offset, f.spread
    )
}

/// The timing pass to measure: the player's ([`Tuning::default`]), or
/// `align-4`'s with `BASE=4`, `align-5`'s with `BASE=5`, each knob
/// overridden by its variable.
fn tuning_from_env() -> Tuning {
    let mut t = match std::env::var("BASE").as_deref() {
        Ok("4") => Tuning::align4(),
        // align-5: align-4 and a blank inside a word without voice at 0.3.
        Ok("5") => {
            let mut t = Tuning::align4();
            t.costs.in_word_silent = 0.3;
            t
        }
        _ => Tuning::default(),
    };
    let num = |k: &str, v: &mut f32| {
        if let Some(x) = std::env::var(k).ok().and_then(|x| x.parse().ok()) {
            *v = x;
        }
    };
    let flag = |k: &str, v: &mut bool| {
        if let Ok(x) = std::env::var(k) {
            *v = x == "1";
        }
    };
    num("T_IN_WORD", &mut t.costs.in_word);
    num("T_IN_WORD_SILENT", &mut t.costs.in_word_silent);
    num("T_LINE_PAUSE", &mut t.costs.line_pause);
    num("T_ONSET", &mut t.costs.onset);
    num("T_VOWELS", &mut t.costs.vowels);
    num("T_EVEN", &mut t.even);
    num("T_FAR_END", &mut t.far_end);
    num("T_STOP", &mut t.stop_hold);
    num("T_MIX", &mut t.mix_ratio);
    num("T_LEAD", &mut t.costs.lead);
    num("T_QUIET_VMR", &mut t.costs.quiet_vmr);
    num("T_GAP_BACKING", &mut t.costs.gap_backing);
    num("T_GAP_LEAD", &mut t.costs.gap_lead);
    num("T_FILL_BASE", &mut t.costs.fill_base);
    num("T_FILL_PROM", &mut t.costs.fill_prominence);
    num("T_COMMA_COST", &mut t.costs.comma_pause);
    if let Some(x) = std::env::var("T_PAUSE_AFTER").ok().and_then(|x| x.parse().ok()) {
        t.costs.pause_after = x;
    }
    flag("T_WINDOWS", &mut t.windows);
    flag("T_FOREIGN", &mut t.foreign_bands);
    flag("T_SOUND", &mut t.by_sound);
    flag("T_SQUEEZED", &mut t.squeezed);
    flag("T_SOUND_LINES", &mut t.sound_lines);
    flag("T_FAR_CAP", &mut t.far_cap);
    flag("T_HUM_ENDS", &mut t.hum_ends);
    flag("T_FOREIGN_COSTS", &mut t.foreign_costs);
    flag("T_SOUND_PLAIN", &mut t.sound_plain);
    flag("T_SAID", &mut t.said);
    flag("T_COMMA", &mut t.comma_pause);
    flag("T_INNER", &mut t.sound_inner);
    flag("T_RUNS", &mut t.sound_runs);
    flag("T_COMMA_FILL", &mut t.comma_fill);
    num("T_END_LEVEL", &mut t.end_level);
    num("T_END_GAP", &mut t.end_gap);
    flag("T_COMMA_ENDS", &mut t.comma_ends);
    num("T_HOLD_VMR", &mut t.hold_vmr);
    num("T_HOLD_PROM", &mut t.hold_prominence);
    t
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog = Catalog::new(&args[1]);
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let cache = PathBuf::from(&args[3]);
    std::fs::create_dir_all(&cache).unwrap();
    let filter = args.get(4).cloned().unwrap_or_default();
    let threads = std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(2);
    let prep_only = std::env::var("PREP").is_ok();
    let device = if std::env::var("DEVICE").as_deref() == Ok("gpu") { Device::Gpu } else { Device::Cpu };
    let mut aligner = Aligner::new(catalog, device, threads);
    let mut duty = Duty::from_env();
    aligner.set_tuning(tuning_from_env());
    println!("tuning {:?}", aligner.tuning());
    let mut groups: Vec<(String, Tally)> = Vec::new();
    let mut pool = |name: String, t: &Tally| match groups.iter_mut().find(|(n, _)| *n == name) {
        Some((_, g)) => g.add(t),
        None => groups.push((name, t.clone())),
    };
    let mut order: Vec<usize> = (0..jobs.len()).collect();
    // Seconds spent aligning (the hearing is cached).
    let mut secs = 0.0f32;
    if prep_only {
        order.sort_by_key(|i| jobs[*i].rank.unwrap_or(*i));
    }
    for idx in order {
        let job = &jobs[idx];
        if job.off || !job.name.contains(&filter) {
            continue;
        }
        // A `stop` file in the cache ends a long run between songs.
        if cache.join("stop").exists() {
            println!("stopped");
            break;
        }
        let Some(mut prep) = prepared(&mut aligner, job, &cache, &mut duty) else { continue };
        if prep_only {
            println!("prepared {}", job.name);
            aligner.unload();
            continue;
        }
        let main = Route::of(&job.lines).map(|r| r.main).unwrap();
        // Another song's text of the same language, for the check.
        let wrong = jobs
            .iter()
            .enumerate()
            .filter(|(i, j)| *i != idx && !j.off && j.kind != "case" && Route::of(&j.lines).map(|r| r.main) == Some(main))
            .min_by_key(|(i, _)| (*i + jobs.len() - idx) % jobs.len())
            .map(|(_, j)| plain(&j.lines));
        println!("== {} ({}, {})", job.name, job.kind, key(main));
        for timed in [true, false] {
            let lines = if timed { job.lines.clone() } else { plain(&job.lines) };
            let t0 = std::time::Instant::now();
            let fit = aligner.fit(&mut prep, &lines, timed, &mut |_| true);
            secs += t0.elapsed().as_secs_f32();
            let fit = match fit {
                Ok(Some(f)) => f,
                other => {
                    println!("  {}: {:?}", if timed { "timed" } else { "plain" }, other.err());
                    continue;
                }
            };
            let mode = if timed { "timed" } else { "plain" };
            let reference: Vec<LyricLine> = if timed { job.lines.clone() } else { job.lines.iter().filter(|l| !l.text.trim().is_empty()).cloned().collect() };
            let mut t = Tally { overlaps: overlaps(&fit.lines), extra: fit.lines.len().saturating_sub(lines.len()), ..Tally::default() };
            // The lines as the reference has them (an added repeat left out).
            let mut fit = fit;
            if t.extra > 0 && job.kind != "case" {
                let mut kept = Vec::new();
                let mut k = 0;
                for l in &fit.lines {
                    if k < lines.len() && l.text == lines[k].text {
                        kept.push(l.clone());
                        k += 1;
                    }
                }
                if kept.len() == lines.len() {
                    fit.lines = kept;
                }
            }
            if job.kind == "words" {
                (t.starts, t.ends) = word_errors(&reference, &fit.lines);
            }
            if job.kind != "case" {
                t.lines = line_errors(&reference, &fit.lines);
                let far: Vec<String> = t.lines.iter().enumerate().filter(|(_, e)| e.abs() >= 0.3).map(|(i, e)| format!("{i} {e:+.2}")).collect();
                println!("  {mode:28} lines off: {}", far.join(", "));
            }
            println!("  {}", summary(mode, &t));
            println!("  {mode:28} {}", verdict(&fit));
            if job.kind == "case" {
                if !job.marks.is_empty() {
                    let (starts, ends) = mark_errors(&job.marks, &lines, &fit.lines);
                    let far: Vec<String> = job
                        .marks
                        .iter()
                        .zip(&starts)
                        .filter(|(_, e)| e.abs() >= 0.3)
                        .map(|(m, e)| format!("{}/{} {:+.2}", m.0, m.1, e))
                        .collect();
                    println!(
                        "  {mode:28} marks {:3} start<0.3 {:5.1}% mae {:5.2}s | ends {:3} <0.3 {:5.1}% mae {:5.2}s | off: {}",
                        starts.len(),
                        within(&starts, 0.3),
                        mae(&starts.iter().map(|e| e.clamp(-30.0, 30.0)).collect::<Vec<_>>()),
                        ends.len(),
                        within(&ends, 0.3),
                        mae(&ends),
                        far.join(", ")
                    );
                    let mut t = Tally { starts, ends, ..Tally::default() };
                    t.overlaps = overlaps(&fit.lines);
                    pool(format!("case marks {mode}"), &t);
                }
                // `SHOW=150-176`: these seconds instead of the job's own.
                let show: Vec<(f32, f32)> = match std::env::var("SHOW").ok().and_then(|s| s.split_once('-').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))) {
                    Some(r) => vec![r],
                    None => job.show.clone(),
                };
                if std::env::var("QUIET").is_err() {
                    print_lines(&fit.lines, &show);
                }
                if timed && std::env::var("TABLE").is_ok() {
                    table(&aligner, &prep, main, &show);
                }
            } else {
                pool(format!("{} {} {}", job.kind, key(main), mode), &t);
                pool(format!("{} all {}", job.kind, mode), &t);
            }
        }
        // `NOWRONG=1`: the check of another song's text left out (it does
        // not depend on the timing pass).
        if let Some(w) = wrong.filter(|_| std::env::var("NOWRONG").is_err())
            && let Ok(Some(f)) = aligner.fit(&mut prep, &w, false, &mut |_| true)
        {
            println!("  {:28} {}", "another song's text", verdict(&f));
        }
    }
    println!("aligning took {secs:.1} s");
    if !groups.is_empty() {
        println!("== pooled");
        groups.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, t) in &groups {
            println!("  {}", summary(name, t));
        }
    }
}
