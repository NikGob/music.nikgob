//! Measures the aligners on real songs (docs/lyrics-align.md, «Measured»).
//!
//! `cargo run -p nikgob-align --release --example measure -- <models dir> <jobs.json> [cpu]`
//!
//! A job is `{"name", "path", "kind": "words"|"lines", "lines": [...],
//! "wrong": [...], "aligners": [...]}` with lines as `LyricLine`
//! serialises: "words" jobs hold a reference word timing (Apple's), "lines"
//! jobs a reference line timing. Each aligner is run on the text with its
//! line times (as the player does for a timed text) and as a plain text
//! (no times: what a Genius text gets), and on another song's text as a
//! plain text, for the check. `"auto"` is the player's own routing (the
//! text's language, line by line). Every result is also printed as one
//! `JSON {...}` line.

use std::time::{Duration, Instant};

use nikgob_align::catalog::ModelId;
use nikgob_align::fit::Fit;
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

fn id_of(s: &str) -> Option<ModelId> {
    Some(match s {
        "ru" => ModelId::AlignerRu,
        "rugolos" => ModelId::AlignerRuGolos,
        "ja" => ModelId::AlignerJa,
        "en" => ModelId::AlignerEn,
        "en960" => ModelId::AlignerEn960,
        "mms" => ModelId::AlignerMms,
        _ => return None,
    })
}

/// Starts of the made words (or lines) against the reference's.
fn errors(reference: &[LyricLine], made: &[LyricLine], words: bool) -> Vec<f32> {
    let mut errs = Vec::new();
    for (r, g) in reference.iter().zip(made) {
        if r.text.trim().is_empty() || r.background {
            continue;
        }
        if words {
            // By where each reference word starts in the line: the made
            // pieces need not be the reference's (a plain text is cut here).
            let mut at = 0;
            let made: Vec<(usize, f32)> = g
                .words
                .iter()
                .map(|w| {
                    let a = at;
                    at += w.text.chars().count();
                    (a, w.start.as_secs_f32())
                })
                .collect();
            let mut at = 0;
            for rw in &r.words {
                let lead = rw.text.chars().take_while(|c| c.is_whitespace()).count();
                let a = at + lead;
                at += rw.text.chars().count();
                if let Some((_, s)) = made.iter().rev().find(|(m, _)| *m <= a) {
                    errs.push(s - rw.start.as_secs_f32());
                }
            }
        } else {
            let start = g.words.first().map_or(g.start, |w| w.start);
            errs.push(start.as_secs_f32() - r.start.as_secs_f32());
        }
    }
    errs
}

fn stats(errs: &[f32]) -> serde_json::Value {
    let mut sorted = errs.to_vec();
    sorted.sort_by(f32::total_cmp);
    let within = |t: f32| errs.iter().filter(|e| e.abs() < t).count() as f32 / errs.len().max(1) as f32 * 100.0;
    serde_json::json!({
        "n": errs.len(),
        "median": sorted.get(sorted.len() / 2).copied().unwrap_or(0.0),
        "mae": errs.iter().map(|e| e.abs()).sum::<f32>() / errs.len().max(1) as f32,
        "w03": within(0.3),
        "w05": within(0.5),
    })
}

fn plain(lines: &[LyricLine]) -> Vec<LyricLine> {
    lines
        .iter()
        .filter(|l| !l.text.trim().is_empty())
        .map(|l| LyricLine::new(Duration::ZERO, l.text.clone()))
        .collect()
}

fn verdict(f: &Option<Fit>) -> serde_json::Value {
    match f {
        Some(f) => serde_json::json!({
            "score": f.score, "heard": f.heard, "spelled": f.spelled, "covered": f.covered,
            "timing": format!("{:?}", f.timing), "offset": f.offset, "spread": f.spread,
        }),
        None => serde_json::Value::Null,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let catalog = Catalog::new(&args[1]);
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let device = if args.get(3).map(String::as_str) == Some("cpu") { Device::Cpu } else { Device::Gpu };
    let threads = std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(4);
    let mut aligner = Aligner::new(catalog, device, threads);
    for job in jobs {
        for separate in if job.mix { vec![true, false] } else { vec![true] } {
            let t0 = Instant::now();
            let mut prep = match aligner.prepare(std::path::Path::new(&job.path), separate, &mut |_: Stage| true) {
                Ok(p) => p,
                Err(e) => {
                    println!("{}: {e}", job.name);
                    continue;
                }
            };
            let prep_secs = t0.elapsed().as_secs_f32();
            let names: Vec<String> = if job.aligners.is_empty() { vec!["auto".into()] } else { job.aligners.clone() };
            let reference_plain = plain(&job.lines);
            for name in names {
                let t1 = Instant::now();
                let run = |aligner: &mut Aligner, prep: &mut nikgob_align::Prepared, lines: &[LyricLine], timed: bool| match id_of(&name) {
                    Some(id) => aligner.fit_with(prep, id, lines, timed, &mut |_| true).unwrap(),
                    None => aligner.fit(prep, lines, timed, &mut |_| true).unwrap(),
                };
                let timed = run(&mut aligner, &mut prep, &job.lines, true);
                let listen = t1.elapsed().as_secs_f32();
                let blind = run(&mut aligner, &mut prep, &reference_plain, false);
                let wrong = if job.wrong.is_empty() { None } else { run(&mut aligner, &mut prep, &plain(&job.wrong), false) };
                let words = job.kind == "words";
                // Plain lines are the reference's lines less the empty ones.
                let kept: Vec<LyricLine> = job.lines.iter().filter(|l| !l.text.trim().is_empty()).cloned().collect();
                let mut out = serde_json::json!({
                    "song": job.name, "kind": job.kind, "aligner": name, "voice": separate,
                    "device": format!("{:?}", aligner.device()), "secs": prep.secs,
                    "prepare_s": prep_secs, "listen_s": listen,
                    "timed": verdict(&timed), "plain": verdict(&blind), "wrong": verdict(&wrong),
                });
                if let Some(f) = &timed {
                    out["timed"]["words"] = stats(&errors(&job.lines, &f.lines, words));
                    out["timed"]["lines"] = stats(&errors(&job.lines, &f.lines, false));
                }
                if let Some(f) = &blind {
                    out["plain"]["words"] = stats(&errors(&kept, &f.lines, words));
                    out["plain"]["lines"] = stats(&errors(&kept, &f.lines, false));
                }
                let pick = |v: &serde_json::Value, k: &str, m: &str| v[k][m]["w03"].as_f64().unwrap_or(f64::NAN);
                println!(
                    "{:38} {:7} {:5} words<0.3 timed {:3.0}% plain {:3.0}% | lines<0.3 timed {:3.0}% plain {:3.0}% | right heard {:.2} spelled {:.2} / wrong {:.2} {:.2} | {:.0}s audio, {:.1}s + {:.1}s",
                    job.name.chars().take(38).collect::<String>(),
                    name,
                    job.kind,
                    pick(&out, "timed", "words"),
                    pick(&out, "plain", "words"),
                    pick(&out, "timed", "lines"),
                    pick(&out, "plain", "lines"),
                    blind.as_ref().map_or(f32::NAN, |f| f.heard),
                    blind.as_ref().map_or(f32::NAN, |f| f.spelled),
                    wrong.as_ref().map_or(f32::NAN, |f| f.heard),
                    wrong.as_ref().map_or(f32::NAN, |f| f.spelled),
                    prep.secs,
                    prep_secs,
                    listen,
                );
                println!("JSON {out}");
            }
        }
    }
}
