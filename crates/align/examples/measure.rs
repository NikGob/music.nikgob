//! Measures the aligners on real songs (docs/lyrics-align.md, «Measured»).
//!
//! `cargo run -p nikgob-align --release --example measure -- <models dir> <jobs.json> [cpu]`
//!
//! A job is `{"name", "path", "kind": "words"|"lines", "lines": [...],
//! "wrong": [...]}` with lines as `LyricLine` serialises: "words" jobs hold
//! a reference word timing (Apple's), "lines" jobs a source's line timing.

use std::time::Instant;

use nikgob_align::catalog::ModelId;
use nikgob_align::{Aligner, Catalog, Device, Stage};
use nikgob_core::lyrics::LyricLine;

#[derive(serde::Deserialize)]
struct Job {
    name: String,
    path: String,
    kind: String,
    lines: Vec<LyricLine>,
    #[serde(default)]
    wrong: Vec<LyricLine>,
    #[serde(default)]
    aligners: Vec<String>,
    #[serde(default)]
    mix: bool,
}

fn id_of(s: &str) -> ModelId {
    match s {
        "ru" => ModelId::AlignerRu,
        "ja" => ModelId::AlignerJa,
        _ => ModelId::AlignerMms,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog = Catalog::new(&args[1]);
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let device = if args.get(3).map(String::as_str) == Some("cpu") { Device::Cpu } else { Device::Gpu };
    let threads = 6;
    let mut aligner = Aligner::new(catalog, device, threads);
    for job in jobs {
        for separate in if job.mix { vec![true, false] } else { vec![true] } {
            let t0 = Instant::now();
            let mut sep_secs = 0.0;
            let mut t_sep = None;
            let mut prep = match aligner.prepare(std::path::Path::new(&job.path), separate, &mut |s| {
                if matches!(s, Stage::Separate(_)) && t_sep.is_none() {
                    t_sep = Some(Instant::now());
                }
                true
            }) {
                Ok(p) => p,
                Err(e) => {
                    println!("{}: {e}", job.name);
                    continue;
                }
            };
            if let Some(t) = t_sep {
                sep_secs = t.elapsed().as_secs_f32();
            }
            let prep_secs = t0.elapsed().as_secs_f32();
            let ids: Vec<ModelId> = if job.aligners.is_empty() {
                vec![Aligner::needs(&job.lines).unwrap_or(ModelId::AlignerMms)]
            } else {
                job.aligners.iter().map(|s| id_of(s)).collect()
            };
            for id in ids {
                let t1 = Instant::now();
                let fit = aligner.fit_with(&mut prep, id, &job.lines, job.kind != "plain", &mut |_| true).unwrap().unwrap();
                let listen = t1.elapsed().as_secs_f32();
                let (wrong, wrong_spelled) = if job.wrong.is_empty() {
                    (f32::NAN, f32::NAN)
                } else {
                    aligner.fit_with(&mut prep, id, &job.wrong, false, &mut |_| true).unwrap().map_or((f32::NAN, f32::NAN), |f| (f.heard, f.spelled))
                };
                let mut report = format!(
                    "{:40} {:5} {:?} {:?} fit {:.2} wrong {:.2} spelled {:.2} wrong {:.2} covered {:.2} timing {:?} offset {:+.2} spread {:.2}",
                    job.name.chars().take(40).collect::<String>(),
                    if separate { "voice" } else { "mix" },
                    id,
                    aligner.device(),
                    fit.heard,
                    wrong,
                    fit.spelled,
                    wrong_spelled,
                    fit.covered,
                    fit.timing,
                    fit.offset,
                    fit.spread
                );
                if job.kind == "words" {
                    let mut errs = Vec::new();
                    let mut line_errs = Vec::new();
                    for (r, g) in job.lines.iter().zip(&fit.lines) {
                        if r.text.trim().is_empty() || r.background {
                            continue;
                        }
                        if let (Some(rw), Some(gw)) = (r.words.first(), g.words.first()) {
                            line_errs.push(gw.start.as_secs_f32() - rw.start.as_secs_f32());
                        }
                        for (rw, gw) in r.words.iter().zip(&g.words) {
                            errs.push(gw.start.as_secs_f32() - rw.start.as_secs_f32());
                        }
                    }
                    let abs: Vec<f32> = errs.iter().map(|e| e.abs()).collect();
                    let mut sorted = errs.clone();
                    sorted.sort_by(f32::total_cmp);
                    let within = |v: &[f32], t: f32| v.iter().filter(|e| e.abs() < t).count() as f32 / v.len().max(1) as f32 * 100.0;
                    report += &format!(
                        " | words {} bias {:+.3} MAE {:.3} <0.3s {:.0}% <0.5s {:.0}% | lines <0.3s {:.0}%",
                        errs.len(),
                        sorted.get(sorted.len() / 2).copied().unwrap_or(0.0),
                        abs.iter().sum::<f32>() / abs.len().max(1) as f32,
                        within(&errs, 0.3),
                        within(&errs, 0.5),
                        within(&line_errs, 0.3)
                    );
                }
                report += &format!(" | {:.0}s audio, decode+separate {:.1}s (separate {:.1}s), listen+align {:.1}s", prep.secs, prep_secs, sep_secs, listen);
                println!("{report}");
            }
        }
    }
}
