//! Recognises songs from scratch and compares the words with a known text
//! (docs/lyrics-align.md, «Measured»).
//!
//! `cargo run -p nikgob-align --example recognise -- <models dir> <jobs.json> [cpu]`
//! (the jobs of `measure`).

use std::time::Instant;

use nikgob_align::{Aligner, Catalog, Device};
use nikgob_core::lyrics::LyricLine;

#[derive(serde::Deserialize)]
struct Job {
    name: String,
    path: String,
    lines: Vec<LyricLine>,
}

fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for w in text.split_whitespace() {
        let w: String = w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
        if w.is_empty() {
            continue;
        }
        // Japanese has no spaces: characters are compared.
        if w.chars().any(|c| ('\u{3040}'..='\u{9fff}').contains(&c)) {
            out.extend(w.chars().map(String::from));
        } else {
            out.push(w);
        }
    }
    out
}

/// Word error rate of `hyp` against `reference` (edit distance / reference words).
fn wer(reference: &[String], hyp: &[String]) -> f32 {
    let mut prev: Vec<usize> = (0..=hyp.len()).collect();
    for (i, r) in reference.iter().enumerate() {
        let mut cur = vec![i + 1; hyp.len() + 1];
        for (j, h) in hyp.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(r != h)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[hyp.len()] as f32 / reference.len().max(1) as f32
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let device = if args.get(3).map(String::as_str) == Some("cpu") { Device::Cpu } else { Device::Gpu };
    let mut aligner = Aligner::new(Catalog::new(&args[1]), device, 6);
    for job in jobs {
        let t0 = Instant::now();
        let mut prep = aligner.prepare(std::path::Path::new(&job.path), true, &mut |_| true).unwrap();
        let t1 = Instant::now();
        let lines = match aligner.recognise(&mut prep, &[], &mut |_| true) {
            Ok(l) => l,
            Err(e) => {
                println!("{}: {e}", job.name);
                continue;
            }
        };
        let rec = t1.elapsed().as_secs_f32();
        let reference: Vec<String> = job.lines.iter().flat_map(|l| words(&l.text)).collect();
        let hyp: Vec<String> = lines.iter().flat_map(|l| words(&l.text)).collect();
        println!(
            "{:40} WER {:.0}% ({} ref words, {} recognised) | {:?} prepare {:.1}s recognise+align {:.1}s for {:.0}s",
            job.name.chars().take(40).collect::<String>(),
            wer(&reference, &hyp) * 100.0,
            reference.len(),
            hyp.len(),
            aligner.device(),
            (t1 - t0).as_secs_f32(),
            rec,
            prep.secs
        );
        for l in lines.iter().take(6) {
            println!("    [{:6.2}] {}", l.start.as_secs_f32(), l.text);
        }
    }
}
