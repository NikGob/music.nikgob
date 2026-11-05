//! Syllable timing on the bench set (docs/lyrics-align.md, «Syllables»).
//!
//! `cargo run -p nikgob-align --release --example syllables -- <models dir> <jobs.json> <cache dir> [name filter]`
//!
//! The jobs and caches of `examples/bench.rs` (only songs already in the
//! cache; no model runs). Each text is aligned with its line times as the
//! player does, then its words cut into syllables ([`Aligner::syllables`]),
//! and:
//! * the words must keep their starts and ends to the nanosecond;
//! * where the reference has a word in syllables (Apple's glued pieces:
//!   «ba|by», «vio|lin», «war|lord») the text is aligned with those words
//!   whole and each boundary the player cuts at the same letter is compared
//!   with the reference's, against where the letters' weighted share of the
//!   word (how the view lit a word before) puts it, both from the word's
//!   own start (so a word that is off as a whole does not count);
//! * every boundary inside a word against the nearest rise of the
//!   separated voice's loudness (a syllable's vowel coming in) and the
//!   nearest dip (the consonant between two vowels), the same for the
//!   weighted share.
//!
//! `SHOW=<from>-<to>` prints the syllables of the lines starting there and
//! the voice's loudness every 20 ms, to read them off by hand; `FIND=<text>`
//! the same for the lines with that text; `HELD=1` lists the words whose
//! syllables the voice holds very unevenly (one 0.5 s or more, another a
//! quarter of it or less), with where the weighted share would cut them.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use nikgob_align::catalog::ModelId;
use nikgob_align::ctc::Emissions;
use nikgob_align::pipeline::Route;
use nikgob_align::{Aligner, Catalog, Device, Prepared};
use nikgob_core::lyrics::{LyricLine, LyricWord};

#[derive(serde::Deserialize)]
struct Job {
    name: String,
    #[serde(default)]
    off: bool,
    path: String,
    kind: String,
    lines: Vec<LyricLine>,
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

/// The song from the bench's cache (`None` when it is not there).
fn cached(job: &Job, lines: &[LyricLine], cache: &Path) -> Option<Prepared> {
    let stem = Path::new(&job.path).file_stem()?.to_string_lossy().into_owned();
    let mut prep = Prepared::from_voice(read_f32(&cache.join(format!("{stem}.voice")))?, true);
    if let Some(l) = read_f32(&cache.join(format!("{stem}.levels"))) {
        let n = l[0] as usize;
        prep.set_levels(l[1..1 + n].to_vec(), l[1 + n..].to_vec());
    }
    for id in Route::of(lines)?.models() {
        // `EMS=x.`: the hearings kept as `<stem>.x.<aligner>.em`.
        let ems = std::env::var("EMS").unwrap_or_default();
        let v = read_f32(&cache.join(format!("{stem}.{ems}{}.em", key(id))))?;
        prep.set_emissions(id, Emissions { logp: v[3..].to_vec(), vocab: v[0] as usize, fps: v[2], blank: v[1] as usize });
    }
    Some(prep)
}

/// A reference's lines with its glued pieces joined into whole words.
fn whole_words(lines: &[LyricLine]) -> Vec<LyricLine> {
    lines
        .iter()
        .map(|l| {
            let mut words: Vec<LyricWord> = Vec::new();
            for w in &l.words {
                match words.last_mut() {
                    Some(last) if !last.text.ends_with(char::is_whitespace) && !w.text.starts_with(char::is_whitespace) => {
                        last.text.push_str(&w.text);
                        last.end = w.end;
                    }
                    _ => words.push(w.clone()),
                }
            }
            LyricLine { words, ..l.clone() }
        })
        .collect()
}

/// The boundaries inside written words of a line's pieces: (letters
/// before it in the line, its time, the start of its word).
fn inner_cuts(l: &LyricLine) -> Vec<(usize, f32, f32)> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut word_start = 0.0;
    for (i, w) in l.words.iter().enumerate() {
        let glued = i > 0 && !l.words[i - 1].text.ends_with(char::is_whitespace) && !w.text.starts_with(char::is_whitespace);
        if glued {
            out.push((at, w.start.as_secs_f32(), word_start));
        } else {
            word_start = w.start.as_secs_f32();
        }
        at += w.text.chars().filter(|c| !c.is_whitespace()).count();
    }
    out
}

/// How the view lit a word before syllables: each letter its weighted
/// share of the word's time (a vowel 2.5, a consonant 1, signs 0.25).
fn weight(c: char) -> f32 {
    const VOWELS: &str = "aeiouyàáâãäåæèéêëìíîïòóôõöøùúûüýÿœаеёиоуыэюяіїє";
    if !c.is_alphanumeric() {
        return 0.25;
    }
    let lower = c.to_lowercase().next().unwrap_or(c);
    if VOWELS.contains(lower) {
        2.5
    } else if matches!(lower, 'ь' | 'ъ') {
        0.25
    } else {
        1.0
    }
}

/// The time the weighted share puts letter `k` of line `l` (counted as in
/// [`inner_cuts`]) at, within its word of `words` (the made words).
fn weighted_at(words: &[LyricWord], k: usize) -> Option<(f32, f32)> {
    let mut at = 0;
    for w in words {
        let letters: Vec<char> = w.text.chars().filter(|c| !c.is_whitespace()).collect();
        if k < at + letters.len() {
            let total: f32 = letters.iter().map(|c| weight(*c)).sum();
            let before: f32 = letters[..k - at].iter().map(|c| weight(*c)).sum();
            let (a, b) = (w.start.as_secs_f32(), w.end.as_secs_f32());
            return Some((a + (b - a) * before / total.max(f32::EPSILON), a));
        }
        at += letters.len();
    }
    None
}

/// Rises of the voice's loudness: every 10 ms, a rise of 4 dB or more
/// over 30 ms that is the largest within 40 ms, where the voice is there.
fn onsets(voice: &[f32]) -> Vec<f32> {
    const HOP: usize = 160;
    let db: Vec<f32> = voice
        .chunks(HOP)
        .map(|c| 10.0 * (c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32 + 1e-9).log10())
        .collect();
    let mut sorted = db.clone();
    sorted.sort_by(f32::total_cmp);
    let loud = sorted[((sorted.len() - 1) as f32 * 0.95) as usize];
    let rise: Vec<f32> = (0..db.len()).map(|t| if t >= 3 { (db[t] - db[t - 3]).max(0.0) } else { 0.0 }).collect();
    (0..rise.len())
        .filter(|&t| {
            rise[t] >= 4.0 && db[t] > loud - 30.0 && (t.saturating_sub(4)..(t + 5).min(rise.len())).all(|u| rise[u] <= rise[t])
        })
        .map(|t| (t as f32 - 1.5) * HOP as f32 / 16_000.0)
        .collect()
}

/// Dips of the voice's loudness (a consonant between two vowels): every
/// 10 ms, the lowest within 30 ms, with 3 dB more on both sides within
/// 80 ms, where the voice is there.
fn dips(voice: &[f32]) -> Vec<f32> {
    const HOP: usize = 160;
    let db: Vec<f32> = voice
        .chunks(HOP)
        .map(|c| 10.0 * (c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32 + 1e-9).log10())
        .collect();
    let mut sorted = db.clone();
    sorted.sort_by(f32::total_cmp);
    let loud = sorted[((sorted.len() - 1) as f32 * 0.95) as usize];
    let n = db.len();
    let top = |r: std::ops::Range<usize>| db[r].iter().copied().fold(f32::NEG_INFINITY, f32::max);
    (8..n.saturating_sub(8))
        .filter(|&t| {
            (t - 3..t + 4).all(|u| db[u] >= db[t])
                && top(t - 8..t) - db[t] >= 3.0
                && top(t + 1..t + 9) - db[t] >= 3.0
                && top(t - 8..t + 9) > loud - 25.0
        })
        .map(|t| (t as f32 + 0.5) * HOP as f32 / 16_000.0)
        .collect()
}

fn nearest(onsets: &[f32], t: f32) -> f32 {
    let i = onsets.partition_point(|o| *o < t);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|j| onsets.get(j)).map(|o| t - o).min_by(|a, b| a.abs().total_cmp(&b.abs())).unwrap_or(f32::INFINITY)
}

fn within(v: &[f32], t: f32) -> f32 {
    v.iter().filter(|e| e.abs() < t).count() as f32 / v.len().max(1) as f32 * 100.0
}

fn median(v: &[f32]) -> f32 {
    let mut s: Vec<f32> = v.iter().map(|x| x.abs()).collect();
    s.sort_by(f32::total_cmp);
    s.get(s.len() / 2).copied().unwrap_or(f32::NAN)
}

fn signed_median(v: &[f32]) -> f32 {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s.get(s.len() / 2).copied().unwrap_or(f32::NAN)
}

#[derive(Default)]
struct Pool {
    words: usize,
    moved: usize,
    pieces: usize,
    /// Syllables of a cut word that last nothing, and under 40 ms.
    zero: usize,
    brief: usize,
    ref_mine: Vec<f32>,
    ref_weighted: Vec<f32>,
    on_mine: Vec<f32>,
    on_weighted: Vec<f32>,
    dip_mine: Vec<f32>,
    dip_weighted: Vec<f32>,
}

fn report(name: &str, p: &Pool) {
    println!(
        "  {name:24} words {:4} moved {} pieces {:4} zero {} <40ms {} | vs reference ({:3}): mine <0.05 {:5.1}% <0.1 {:5.1}% med {:.3} ({:+.3}) / weighted <0.05 {:5.1}% <0.1 {:5.1}% med {:.3} ({:+.3}) | vs voice rises ({:4}): mine <0.04 {:5.1}% <0.08 {:5.1}% med {:.3} ({:+.3}) / weighted <0.04 {:5.1}% <0.08 {:5.1}% med {:.3} ({:+.3})",
        p.words,
        p.moved,
        p.pieces,
        p.zero,
        p.brief,
        p.ref_mine.len(),
        within(&p.ref_mine, 0.05),
        within(&p.ref_mine, 0.1),
        median(&p.ref_mine),
        signed_median(&p.ref_mine),
        within(&p.ref_weighted, 0.05),
        within(&p.ref_weighted, 0.1),
        median(&p.ref_weighted),
        signed_median(&p.ref_weighted),
        p.on_mine.len(),
        within(&p.on_mine, 0.04),
        within(&p.on_mine, 0.08),
        median(&p.on_mine),
        signed_median(&p.on_mine),
        within(&p.on_weighted, 0.04),
        within(&p.on_weighted, 0.08),
        median(&p.on_weighted),
        signed_median(&p.on_weighted),
    );
    println!(
        "  {:24} vs voice dips ({:4}): mine <0.04 {:5.1}% <0.08 {:5.1}% med {:.3} ({:+.3}) / weighted <0.04 {:5.1}% <0.08 {:5.1}% med {:.3} ({:+.3})",
        "",
        p.dip_mine.len(),
        within(&p.dip_mine, 0.04),
        within(&p.dip_mine, 0.08),
        median(&p.dip_mine),
        signed_median(&p.dip_mine),
        within(&p.dip_weighted, 0.04),
        within(&p.dip_weighted, 0.08),
        median(&p.dip_weighted),
        signed_median(&p.dip_weighted),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let jobs: Vec<Job> = serde_json::from_str(&std::fs::read_to_string(&args[2]).unwrap()).unwrap();
    let cache = PathBuf::from(&args[3]);
    let filter = args.get(4).cloned().unwrap_or_default();
    let show: Option<(f32, f32)> = std::env::var("SHOW").ok().and_then(|s| {
        let (a, b) = s.split_once('-')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    });
    let mut aligner = Aligner::new(Catalog::new(&args[1]), Device::Cpu, 2);
    let mut pools: Vec<(String, Pool)> = Vec::new();
    for job in &jobs {
        if job.off || !job.name.contains(&filter) {
            continue;
        }
        let lines = if job.kind == "words" { whole_words(&job.lines) } else { job.lines.clone() };
        let Some(mut prep) = cached(job, &lines, &cache) else {
            println!("{}: not in the cache", job.name);
            continue;
        };
        let main = Route::of(&lines).map(|r| r.main).unwrap();
        let t0 = std::time::Instant::now();
        let Ok(Some(mut fit)) = aligner.fit(&mut prep, &lines, true, &mut |_| true) else {
            println!("{}: not aligned", job.name);
            continue;
        };
        let words = fit.lines.clone();
        let t1 = std::time::Instant::now();
        aligner.syllables(&prep, &lines, true, &mut fit);
        let took = t1.elapsed().as_secs_f32();
        let mut p = Pool::default();
        // The words as they were, glued back from the pieces.
        for (before, after) in words.iter().zip(&fit.lines) {
            let glued = whole_words(std::slice::from_ref(after));
            p.words += before.words.len();
            p.pieces += after.words.len();
            p.zero += after.words.iter().filter(|w| w.end <= w.start).count();
            p.brief += after.words.iter().filter(|w| w.end > w.start && w.end - w.start < Duration::from_millis(40)).count();
            p.moved += before.words.iter().zip(&glued[0].words).filter(|(a, b)| a != b).count() + before.words.len().abs_diff(glued[0].words.len());
        }
        // The reference's syllables against these, at the same letters.
        let reference: Vec<&LyricLine> = job.lines.iter().filter(|l| !l.text.trim().is_empty()).collect();
        let mut made: Vec<(&LyricLine, &LyricLine)> = Vec::new();
        for (l, w) in fit.lines.iter().zip(&words) {
            if made.len() < reference.len() && l.text == reference[made.len()].text {
                made.push((l, w));
            }
        }
        if job.kind == "words" && made.len() == reference.len() {
            for (r, (l, w)) in reference.iter().zip(&made) {
                let mine = inner_cuts(l);
                for (k, t, start) in inner_cuts(r) {
                    let Some((_, mt, ms)) = mine.iter().find(|c| c.0 == k) else { continue };
                    let Some((wt, ws)) = weighted_at(&w.words, k) else { continue };
                    p.ref_mine.push((mt - ms) - (t - start));
                    p.ref_weighted.push((wt - ws) - (t - start));
                }
            }
        }
        // Every boundary against the voice's rises.
        let (rises, falls) = (onsets(&prep.voice), dips(&prep.voice));
        for (l, w) in fit.lines.iter().zip(&words) {
            for (k, t, _) in inner_cuts(l) {
                if let Some((wt, _)) = weighted_at(&w.words, k) {
                    p.on_mine.push(nearest(&rises, t));
                    p.on_weighted.push(nearest(&rises, wt));
                    p.dip_mine.push(nearest(&falls, t));
                    p.dip_weighted.push(nearest(&falls, wt));
                }
            }
        }
        println!("== {} ({}, {}): aligned in {:.1} s, syllables in {:.3} s", job.name, job.kind, key(main), (t1 - t0).as_secs_f32(), took);
        report(&job.name, &p);
        if let Some((a, b)) = show {
            print_show(&fit.lines, &prep, |l| (a..=b).contains(&l.start.as_secs_f32()));
        }
        if let Ok(find) = std::env::var("FIND") {
            print_show(&fit.lines, &prep, |l| l.text.contains(&find));
        }
        if std::env::var("HELD").is_ok() {
            print_held(&fit.lines, &words);
        }
        let lang = format!("pooled {}", key(main));
        match pools.iter_mut().find(|(n, _)| *n == lang) {
            Some((_, q)) => merge(q, p),
            None => pools.push((lang, p)),
        }
    }
    println!("== pooled");
    for (name, p) in &pools {
        report(name, p);
    }
}

fn merge(into: &mut Pool, p: Pool) {
    into.words += p.words;
    into.moved += p.moved;
    into.pieces += p.pieces;
    into.zero += p.zero;
    into.brief += p.brief;
    into.ref_mine.extend(p.ref_mine);
    into.ref_weighted.extend(p.ref_weighted);
    into.on_mine.extend(p.on_mine);
    into.on_weighted.extend(p.on_weighted);
    into.dip_mine.extend(p.dip_mine);
    into.dip_weighted.extend(p.dip_weighted);
}

/// Words cut into syllables of very uneven lengths, against the weighted
/// share's cuts.
fn print_held(lines: &[LyricLine], words: &[LyricLine]) {
    for (l, w) in lines.iter().zip(words) {
        let glued = whole_words(std::slice::from_ref(l));
        let mut k = 0;
        for (word, made) in glued[0].words.iter().zip(&w.words) {
            let n = l.words[k..].iter().position(|p| p.text.ends_with(char::is_whitespace)).map_or(l.words.len() - k, |i| i + 1);
            let pieces = &l.words[k..k + n];
            let at = l.words[..k].iter().map(|p| p.text.chars().filter(|c| !c.is_whitespace()).count()).sum::<usize>();
            k += n;
            let lens: Vec<f32> = pieces.iter().map(|p| p.end.as_secs_f32() - p.start.as_secs_f32()).collect();
            let (long, short) = (lens.iter().copied().fold(0.0, f32::max), lens.iter().copied().fold(f32::INFINITY, f32::min));
            if pieces.len() < 2 || long < 0.5 || short > long * 0.25 {
                continue;
            }
            let mut cut = at;
            let mut weighted = Vec::new();
            for p in &pieces[..pieces.len() - 1] {
                cut += p.text.chars().filter(|c| !c.is_whitespace()).count();
                weighted.push(weighted_at(std::slice::from_ref(made), cut - at).map_or(f32::NAN, |x| x.0));
            }
            let mine: Vec<String> = pieces.iter().map(|p| format!("{}@{:.2}", p.text.trim_end(), p.start.as_secs_f32())).collect();
            let theirs: Vec<String> = weighted.iter().map(|t| format!("{t:.2}")).collect();
            println!("    {:?} {:.2}–{:.2}: {} | weighted cuts {}", word.text.trim_end(), word.start.as_secs_f32(), word.end.as_secs_f32(), mine.join(" "), theirs.join(" "));
        }
    }
}

/// The lines `pick` takes with their pieces, and the voice's loudness
/// every 20 ms over them, the pieces' starts marked.
fn print_show(lines: &[LyricLine], prep: &Prepared, pick: impl Fn(&LyricLine) -> bool) {
    let rises = onsets(&prep.voice);
    for l in lines.iter().filter(|l| !l.text.trim().is_empty() && pick(l)) {
        println!("    «{}»", l.text);
        for w in &l.words {
            println!("      {:7.3} {:7.3} {:?}", w.start.as_secs_f32(), w.end.as_secs_f32(), w.text);
        }
        let (from, to) = (l.start.as_secs_f32() - 0.1, l.end.map_or(l.start.as_secs_f32() + 3.0, |e| e.as_secs_f32()) + 0.1);
        let hop = 320;
        let mut t = from;
        while t < to {
            let i = (t * 16_000.0) as usize;
            let c = &prep.voice[i.min(prep.voice.len())..(i + hop).min(prep.voice.len())];
            let db = 10.0 * (c.iter().map(|x| x * x).sum::<f32>() / c.len().max(1) as f32 + 1e-9).log10();
            let at: Vec<&str> = l.words.iter().filter(|w| (t..t + 0.02).contains(&w.start.as_secs_f32())).map(|w| w.text.as_str()).collect();
            let rise = if rises.iter().any(|o| (t..t + 0.02).contains(o)) { "^" } else { " " };
            println!("      {t:7.2} {rise} {:<40} {}", "#".repeat(((db + 60.0) * 0.7).max(0.0) as usize), at.join(" "));
            t += 0.02;
        }
    }
}
