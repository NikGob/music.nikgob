//! Deep mode («Глубокое создание субтитров», docs/lyrics-align.md, «Deep
//! mode»): what the player does beyond the normal timing when the user
//! turns it on: a stronger voice separator, the song heard twice over
//! shifted windows, and a second aligner for a language whose letters are
//! heard with the first one's or instead of them. Only the word timing
//! comes from it; the check's verdict stays the normal aligner's (its
//! levels were measured on it).

use std::collections::HashMap;

use crate::catalog::ModelId;
use crate::ctc::Emissions;

/// How a second aligner's hearing is used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Second {
    #[default]
    Off,
    /// Its letters instead of the first aligner's.
    Alone,
    /// Both aligners' letter probabilities averaged, frame by frame.
    Fused,
    /// Their log-probabilities averaged (a letter both hear stands out,
    /// one only one of them hears is damped).
    Product,
}

/// What deep mode changes (each part measured on its own: «Deep mode»).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deep {
    /// The voice separated by Mel-Band RoFormer instead of MDX-Net…
    pub separator: bool,
    /// … for Russian texts too (it heard Russian songs worse).
    pub separator_russian: bool,
    /// RoFormer's chunks overlap by half (each moment heard twice), else
    /// they follow each other (half the work).
    pub overlap: bool,
    /// The song heard a second time over windows cut half a window later,
    /// the two averaged (`fit::Tuning::windows`).
    pub windows: bool,
    /// Quiet sung passages raised before the aligners hear them
    /// ([`level`]), for every language…
    pub level: bool,
    /// … or for Russian only.
    pub level_russian: bool,
    /// GigaAM for Russian.
    pub russian: Second,
    /// The singing aligner (Tune Studio's) for English.
    pub english: Second,
    /// The singing aligner for the languages MMS hears.
    pub other: Second,
    /// The first aligner's share in [`Second::Fused`].
    pub weight: f32,
}

impl Deep {
    /// Nothing: the normal timing.
    pub const OFF: Self = Self {
        separator: false,
        separator_russian: false,
        overlap: true,
        windows: false,
        level: false,
        level_russian: false,
        russian: Second::Off,
        english: Second::Off,
        other: Second::Off,
        weight: 0.5,
    };

    /// What the setting turns on: what measured better (docs/lyrics-align.md,
    /// «Deep mode»).
    pub const CHOSEN: Self = Self { separator: true, level_russian: true, russian: Second::Fused, english: Second::Fused, ..Self::OFF };

    /// The second aligner of aligner `main`, and how it is used.
    pub fn second(&self, main: ModelId) -> Option<(ModelId, Second)> {
        let (id, how) = match main {
            ModelId::AlignerRu => (ModelId::AlignerRuDeep, self.russian),
            ModelId::AlignerEn => (ModelId::AlignerSung, self.english),
            ModelId::AlignerMms => (ModelId::AlignerSung, self.other),
            _ => return None,
        };
        (how != Second::Off).then_some((id, how))
    }

    /// Aligner `id` hears the voice with its quiet passages raised.
    pub fn levels(&self, id: ModelId) -> bool {
        self.level || (self.level_russian && matches!(id, ModelId::AlignerRu | ModelId::AlignerRuDeep))
    }

    /// Deep mode's separator takes the voice out for a text heard by
    /// `main` (`None`: not known yet, a text to recognise).
    pub fn separates(&self, main: Option<ModelId>) -> bool {
        self.separator && (self.separator_russian || main != Some(ModelId::AlignerRu))
    }

    /// The parts deep mode wants for a text heard by `main` (with a
    /// separation: `separate`).
    pub fn parts(&self, main: ModelId, separate: bool) -> Vec<ModelId> {
        let mut out = Vec::new();
        if separate && self.separates(Some(main)) {
            out.push(ModelId::SeparatorDeep);
        }
        if let Some((id, _)) = self.second(main) {
            out.push(id);
        }
        out
    }
}

/// A quiet passage is raised towards this share of the voice's loud level…
const LEVEL_TO: f32 = 0.5;
/// … by at most this much…
const LEVEL_MAX: f32 = 6.0;
/// … where the voice is at least this share of its loud level (under it,
/// silence and noise stay as they are).
const LEVEL_FLOOR: f32 = 0.04;

/// The voice (16 kHz) with its quiet sung passages raised: each tenth of a
/// second's loudness (over the half second around it) brought towards
/// [`LEVEL_TO`] of the voice's loud level, the gain eased between tenths.
/// An aligner hears a 30 s window brought to one loudness as a whole, so a
/// line chanted at a fifth of the song's level next to loud ones is nearly
/// silence to it.
pub fn level(voice: &[f32]) -> Vec<f32> {
    const HOP: usize = 1600;
    let rms: Vec<f32> = voice.chunks(HOP).map(|c| (c.iter().map(|s| s * s).sum::<f32>() / c.len().max(1) as f32).sqrt()).collect();
    if rms.is_empty() {
        return voice.to_vec();
    }
    let mut sorted = rms.clone();
    sorted.sort_by(f32::total_cmp);
    let loud = sorted[((sorted.len() - 1) as f32 * 0.95) as usize];
    if loud <= 0.0 {
        return voice.to_vec();
    }
    let gain: Vec<f32> = (0..rms.len())
        .map(|i| {
            let around = &rms[i.saturating_sub(2)..(i + 3).min(rms.len())];
            let r = (around.iter().map(|v| v * v).sum::<f32>() / around.len() as f32).sqrt();
            if r < loud * LEVEL_FLOOR { 1.0 } else { (loud * LEVEL_TO / r).clamp(1.0, LEVEL_MAX) }
        })
        .collect();
    voice
        .iter()
        .enumerate()
        .map(|(n, s)| {
            // Between the centres of the tenths around this sample.
            let x = (n as f32 / HOP as f32 - 0.5).max(0.0);
            let i = (x as usize).min(gain.len() - 1);
            let j = (i + 1).min(gain.len() - 1);
            let f = x - i as f32;
            s * (gain[i] * (1.0 - f) + gain[j] * f)
        })
        .collect()
}

/// A model's letters by name (`vocab.json`), its blank under `<blank>`.
pub fn letters(json: &str) -> HashMap<String, usize> {
    let map: HashMap<String, i64> = serde_json::from_str(json).unwrap_or_default();
    let blank = ["<blank>", "<pad>"].iter().find_map(|k| map.get(*k)).copied();
    let mut out: HashMap<String, usize> = map
        .iter()
        .filter(|(k, _)| k.chars().count() == 1)
        .map(|(k, v)| (k.to_lowercase(), *v as usize))
        .collect();
    if let Some(b) = blank {
        out.insert("<blank>".into(), b as usize);
    }
    out
}

/// Averages `other`'s letter probabilities into `main`'s, frame by frame
/// (both at one frame rate), `weight` of them `main`'s: each of `main`'s
/// letters takes `other`'s letter of the same name (a Russian «ё» its
/// «е» when `other` has none; the blanks and word separators each
/// other's); what `other` has no letter for keeps `main`'s alone, and each
/// frame is brought back to a sum of one. Frames `other` lacks stay.
/// `product`: the log-probabilities are averaged instead ([`Second::Product`]).
pub fn fuse(main: &Emissions, main_letters: &HashMap<String, usize>, other: &Emissions, other_letters: &HashMap<String, usize>, weight: f32, product: bool) -> Emissions {
    let mut map: Vec<Option<usize>> = vec![None; main.vocab];
    for (k, &id) in main_letters {
        let o = other_letters.get(k).or_else(|| if k == "ё" { other_letters.get("е") } else { None });
        if id < main.vocab
            && let Some(&o) = o
            && o < other.vocab
        {
            map[id] = Some(o);
        }
    }
    let mut out = main.clone();
    let n = main.frames().min(other.frames());
    let w = weight.clamp(0.0, 1.0);
    for t in 0..n {
        let (m, o) = (main.row(t), other.row(t));
        let row = &mut out.logp[t * main.vocab..(t + 1) * main.vocab];
        let mut sum = 0.0f32;
        for (i, v) in row.iter_mut().enumerate() {
            let p = match (map[i], product) {
                (Some(j), false) => w * m[i].exp() + (1.0 - w) * o[j].exp(),
                (None, false) => w * m[i].exp(),
                (Some(j), true) => (w * m[i] + (1.0 - w) * o[j]).exp(),
                (None, true) => m[i].exp(),
            };
            *v = p;
            sum += p;
        }
        for v in row.iter_mut() {
            *v = (*v / sum.max(1e-30)).max(1e-30).ln();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn em(rows: &[[f32; 3]]) -> Emissions {
        Emissions { logp: rows.iter().flatten().map(|p| p.ln()).collect(), vocab: 3, fps: 50.0, blank: 0 }
    }

    /// Letters of the same name are averaged whatever their ids; a letter
    /// the other model lacks keeps the first one's share.
    #[test]
    fn two_hearings_are_fused_by_letter() {
        let main = em(&[[0.1, 0.8, 0.1], [0.5, 0.25, 0.25]]);
        // The other model has «а» and «б» the other way round, no «ё».
        let other = em(&[[0.1, 0.1, 0.8], [0.9, 0.05, 0.05]]);
        let ml = letters(r#"{"<pad>":0,"а":1,"ё":2}"#);
        let ol = letters(r#"{"<blank>":0,"е":1,"а":2}"#);
        let f = fuse(&main, &ml, &other, &ol, 0.5, false);
        let p = |t: usize, i: usize| f.row(t)[i].exp();
        assert!((p(0, 1) - 0.8).abs() < 1e-5, "«а» agreed: {}", p(0, 1));
        // «ё» takes the other's «е».
        assert!((p(0, 2) - 0.1).abs() < 1e-5);
        assert!((p(1, 0) - 0.7).abs() < 1e-5, "blanks averaged: {}", p(1, 0));
        for t in 0..2 {
            assert!(((0..3).map(|i| p(t, i)).sum::<f32>() - 1.0).abs() < 1e-5);
        }
        // All of it the first model's: nothing changes.
        let same = fuse(&main, &ml, &other, &ol, 1.0, false);
        assert!(same.logp.iter().zip(&main.logp).all(|(a, b)| (a - b).abs() < 1e-5));
        // Geometric: a letter one of them hardly hears is damped more.
        let g = fuse(&main, &ml, &other, &ol, 0.5, true);
        assert!(g.row(1)[1].exp() < f.row(1)[1].exp() && (g.row(0)[1].exp() - 0.8).abs() < 1e-5);
        let same = fuse(&main, &ml, &other, &ol, 1.0, true);
        assert!(same.logp.iter().zip(&main.logp).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    /// A quiet sung line is raised, a loud one and silence are not.
    #[test]
    fn quiet_singing_is_raised() {
        let tone = |n: usize, a: f32| (0..n).map(move |i| (i as f32 * 0.07).sin() * a);
        let voice: Vec<f32> = tone(32_000, 0.5).chain(tone(32_000, 0.1)).chain(std::iter::repeat_n(0.0, 32_000)).chain(tone(32_000, 0.002)).collect();
        let out = level(&voice);
        let rms = |a: usize, b: usize| (out[a..b].iter().map(|s| s * s).sum::<f32>() / (b - a) as f32).sqrt();
        let loud = rms(8_000, 24_000);
        assert!((loud - 0.5 / 2f32.sqrt()).abs() < 0.02, "{loud}");
        let quiet = rms(40_000, 56_000);
        assert!(quiet > 0.15, "raised: {quiet}");
        assert!(rms(72_000, 88_000) < 1e-6);
        assert!(rms(104_000, 120_000) < 0.002, "noise stays");
    }

    #[test]
    fn each_language_has_its_second_aligner() {
        let d = Deep::CHOSEN;
        assert_eq!(d.second(ModelId::AlignerRu).map(|s| s.0), Some(ModelId::AlignerRuDeep));
        assert_eq!(d.second(ModelId::AlignerJa), None);
        assert_eq!(Deep::OFF.second(ModelId::AlignerRu), None);
        assert!(Deep::OFF.parts(ModelId::AlignerRu, true).is_empty());
        // RoFormer heard Russian songs worse: their voice stays MDX-Net's.
        assert!(!d.parts(ModelId::AlignerRu, true).contains(&ModelId::SeparatorDeep));
        assert!(d.parts(ModelId::AlignerJa, true).contains(&ModelId::SeparatorDeep));
        assert!(!d.parts(ModelId::AlignerEn, false).contains(&ModelId::SeparatorDeep));
        assert!(d.separates(None) && !d.separates(Some(ModelId::AlignerRu)));
        assert!(d.levels(ModelId::AlignerRuDeep) && !d.levels(ModelId::AlignerEn) && !Deep::OFF.levels(ModelId::AlignerRu));
    }
}
