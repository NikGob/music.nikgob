//! Where the last words of a text end against the voice.
//!
//! `cargo run -p nikgob-align --release --example ends -- <models dir> <audio> <lrc or json lines> [from secs]`
//!
//! Fits the LRC text (with its line times) to the song on the CPU and
//! prints the words from `from` on, and with `TABLE=1` every tenth of a
//! second there: the voice's energy against its loud level, the aligner's
//! blank posterior and its best letter.

use std::path::Path;

use nikgob_align::{Aligner, Catalog, Device, Stage};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let threads = std::env::var("THREADS").ok().and_then(|t| t.parse().ok()).unwrap_or(2);
    let mut aligner = Aligner::new(Catalog::new(&args[1]), Device::Cpu, threads);
    let text = std::fs::read_to_string(&args[3]).unwrap();
    // JSON lines as the library keeps them, or LRC.
    let lines: Vec<nikgob_core::lyrics::LyricLine> = match serde_json::from_str(&text) {
        Ok(lines) => lines,
        Err(_) => nikgob_metadata_free::parse(&text),
    };
    let from: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(100.0);
    let mut prep = aligner.prepare(Path::new(&args[2]), true, &mut |_: Stage| true).unwrap();
    let fit = aligner.fit(&mut prep, &lines, true, &mut |_| true).unwrap().unwrap();
    println!("timing {:?} offset {:.2} spread {:.2} covered {:.2}", fit.timing, fit.offset, fit.spread, fit.covered);
    for l in fit.lines.iter().filter(|l| l.start.as_secs_f32() >= from - 10.0) {
        println!("{:7.2} {:?} «{}»", l.start.as_secs_f32(), l.end.map(|e| e.as_secs_f32()), l.text);
        for w in &l.words {
            println!("        {:7.2} {:7.2} {:?}", w.start.as_secs_f32(), w.end.as_secs_f32(), w.text);
        }
    }
    // The same text without its times (what a Genius text gets).
    let plain: Vec<_> = lines.iter().filter(|l| !l.text.trim().is_empty()).map(|l| nikgob_core::lyrics::LyricLine::new(std::time::Duration::ZERO, l.text.clone())).collect();
    let blind = aligner.fit(&mut prep, &plain, false, &mut |_| true).unwrap().unwrap();
    println!("plain:");
    for l in blind.lines.iter().rev().take(2).rev() {
        println!("{:7.2} {:?} «{}»", l.start.as_secs_f32(), l.end.map(|e| e.as_secs_f32()), l.text);
        for w in &l.words {
            println!("        {:7.2} {:7.2} {:?}", w.start.as_secs_f32(), w.end.as_secs_f32(), w.text);
        }
    }
    // The tenths themselves only when asked (`TABLE=1`).
    if std::env::var("TABLE").is_err() {
        return;
    }
    let id = fit.aligner.unwrap();
    let em = prep.emissions(id).unwrap();
    let energy = prep.energy();
    let mut sorted = energy.to_vec();
    sorted.sort_by(f32::total_cmp);
    let peak = sorted[((sorted.len() - 1) as f32 * 0.95) as usize];
    let per = (em.fps / 10.0).round() as usize;
    for (i, e) in energy.iter().enumerate().skip((from * 10.0) as usize) {
        let frames = (i * per..((i + 1) * per).min(em.frames())).collect::<Vec<_>>();
        if frames.is_empty() {
            break;
        }
        let blank = frames.iter().map(|t| em.row(*t)[em.blank].exp()).sum::<f32>() / frames.len() as f32;
        let best = frames
            .iter()
            .map(|t| {
                let r = em.row(*t);
                (0..em.vocab).filter(|v| *v != em.blank).max_by(|a, b| r[*a].total_cmp(&r[*b])).map(|v| (v, r[v].exp())).unwrap()
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        println!("{:6.1}s energy {:5.3} blank {:4.2} best {:3} {:4.2}", i as f32 / 10.0, e / peak, blank, best.0, best.1);
    }
}

mod nikgob_metadata_free {
    use std::time::Duration;

    use nikgob_core::lyrics::LyricLine;

    /// `[mm:ss.xx] text` lines (enough for a measurement).
    pub fn parse(text: &str) -> Vec<LyricLine> {
        text.lines()
            .filter_map(|l| {
                let l = l.trim();
                let close = l.find(']')?;
                let stamp = &l[1..close];
                let (m, s) = stamp.split_once(':')?;
                let secs = m.parse::<f32>().ok()? * 60.0 + s.parse::<f32>().ok()?;
                Some(LyricLine::new(Duration::from_secs_f32(secs), l[close + 1..].trim()))
            })
            .collect()
    }
}
