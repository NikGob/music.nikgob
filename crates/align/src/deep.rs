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
}

/// What deep mode changes (each part measured on its own: «Deep mode»).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deep {
    /// The voice separated by Mel-Band RoFormer instead of MDX-Net.
    pub separator: bool,
    /// RoFormer's chunks overlap by half (each moment heard twice), else
    /// they follow each other (half the work).
    pub overlap: bool,
    /// The song heard a second time over windows cut half a window later,
    /// the two averaged (`fit::Tuning::windows`).
    pub windows: bool,
    /// GigaAM for Russian.
    pub russian: Second,
    /// A second aligner for English: [`Self::english_model`].
    pub english: Second,
    pub english_model: ModelId,
    /// The singing aligner for the languages MMS hears.
    pub other: Second,
    /// The first aligner's share in [`Second::Fused`].
    pub weight: f32,
}

impl Deep {
    /// Nothing: the normal timing.
    pub const OFF: Self = Self {
        separator: false,
        overlap: true,
        windows: false,
        russian: Second::Off,
        english: Second::Off,
        english_model: ModelId::AlignerEnDeep,
        other: Second::Off,
        weight: 0.5,
    };

    /// What the setting turns on: what measured better (docs/lyrics-align.md,
    /// «Deep mode»).
    pub const CHOSEN: Self = Self { separator: true, windows: true, russian: Second::Fused, english: Second::Fused, ..Self::OFF };

    /// The second aligner of aligner `main`, and how it is used.
    pub fn second(&self, main: ModelId) -> Option<(ModelId, Second)> {
        let (id, how) = match main {
            ModelId::AlignerRu => (ModelId::AlignerRuDeep, self.russian),
            ModelId::AlignerEn => (self.english_model, self.english),
            ModelId::AlignerMms => (ModelId::AlignerSung, self.other),
            _ => return None,
        };
        (how != Second::Off).then_some((id, how))
    }

    /// The parts deep mode wants for a text heard by `main` (with a
    /// separation: `separate`).
    pub fn parts(&self, main: ModelId, separate: bool) -> Vec<ModelId> {
        let mut out = Vec::new();
        if separate && self.separator {
            out.push(ModelId::SeparatorDeep);
        }
        if let Some((id, _)) = self.second(main) {
            out.push(id);
        }
        out
    }
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
pub fn fuse(main: &Emissions, main_letters: &HashMap<String, usize>, other: &Emissions, other_letters: &HashMap<String, usize>, weight: f32) -> Emissions {
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
            let p = match map[i] {
                Some(j) => w * m[i].exp() + (1.0 - w) * o[j].exp(),
                None => w * m[i].exp(),
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
        let f = fuse(&main, &ml, &other, &ol, 0.5);
        let p = |t: usize, i: usize| f.row(t)[i].exp();
        assert!((p(0, 1) - 0.8).abs() < 1e-5, "«а» agreed: {}", p(0, 1));
        // «ё» takes the other's «е».
        assert!((p(0, 2) - 0.1).abs() < 1e-5);
        assert!((p(1, 0) - 0.7).abs() < 1e-5, "blanks averaged: {}", p(1, 0));
        for t in 0..2 {
            assert!(((0..3).map(|i| p(t, i)).sum::<f32>() - 1.0).abs() < 1e-5);
        }
        // All of it the first model's: nothing changes.
        let same = fuse(&main, &ml, &other, &ol, 1.0);
        assert!(same.logp.iter().zip(&main.logp).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn each_language_has_its_second_aligner() {
        let d = Deep::CHOSEN;
        assert_eq!(d.second(ModelId::AlignerRu).map(|s| s.0), Some(ModelId::AlignerRuDeep));
        assert_eq!(d.second(ModelId::AlignerJa), None);
        assert_eq!(Deep::OFF.second(ModelId::AlignerRu), None);
        assert!(Deep::OFF.parts(ModelId::AlignerRu, true).is_empty());
        assert!(d.parts(ModelId::AlignerRu, true).contains(&ModelId::SeparatorDeep));
        assert!(!d.parts(ModelId::AlignerRu, false).contains(&ModelId::SeparatorDeep));
    }
}
