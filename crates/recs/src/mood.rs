//! Moods (docs/stage4.md §4 «Под настроение»): six cards always, each a
//! tail of a sound axis.
//!
//! The axes come from bliss's features (bliss 0.13, features version 2,
//! `AnalysisIndex` order) as z-scores over this library, so "energetic" means
//! energetic for this library:
//!
//! | index | feature | used by |
//! |---|---|---|
//! | 0 | tempo | energy |
//! | 1 | zero-crossing rate | energy |
//! | 2 | spectral centroid, mean | energy, brightness |
//! | 3 | spectral centroid, deviation | steadiness (−) |
//! | 4 | spectral rolloff, mean | brightness |
//! | 8 | loudness, mean | energy |
//! | 9 | loudness, deviation | steadiness (−) |
//! | 16 | chroma 7: share of major triads | brightness |
//! | 17 | chroma 8: share of minor triads | brightness (−) |
//!
//! * **energy** = mean of tempo, ZCR, centroid and loudness;
//! * **brightness** = half timbre (centroid, rolloff), half harmony (major
//!   minus minor triads);
//! * **steadiness** = minus the mean of the loudness and centroid
//!   deviations.
//!
//! A mood's score per track ([`Mood::score`]): Энергичное = energy,
//! Спокойное = −energy, Светлое = brightness, Мрачное = −brightness, Драйв =
//! energy and steadiness together, Для фокуса = steadiness minus energy above
//! the middle (steady, not loud: low to middle energy). A mood draws from the
//! top third of the library by its score ([`Recs::mood_pool`]); the radio
//! machinery then weighs affinity and the day part
//! ([`crate::radio::RadioSeed::Mood`]).

use nikgob_core::model::TrackId;
use serde::{Deserialize, Serialize};

use crate::analysis::Vector;
use crate::pick::Recs;
use crate::taste::DayPart;

const TEMPO: usize = 0;
const ZCR: usize = 1;
const CENTROID: usize = 2;
const CENTROID_STD: usize = 3;
const ROLLOFF: usize = 4;
const LOUDNESS: usize = 8;
const LOUDNESS_STD: usize = 9;
/// The features the energy is the mean of (tests).
#[cfg(test)]
pub(crate) const ENERGY_FEATURES: [usize; 4] = [TEMPO, ZCR, CENTROID, LOUDNESS];
const MAJOR: usize = 16;
const MINOR: usize = 17;

/// The six moods.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Mood {
    /// «Спокойное».
    Calm,
    /// «Энергичное».
    Energetic,
    /// «Мрачное».
    Dark,
    /// «Светлое».
    Bright,
    /// «Для фокуса».
    Focus,
    /// «Драйв».
    Drive,
}

/// A track's position on the three sound axes (z-score units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axes {
    pub energy: f32,
    pub brightness: f32,
    pub steadiness: f32,
}

impl Axes {
    /// The axes of a vector of z-scores ([`crate::index::Normalizer::z`]).
    pub fn of(z: &Vector) -> Self {
        Self {
            energy: (z[TEMPO] + z[ZCR] + z[CENTROID] + z[LOUDNESS]) / 4.0,
            brightness: (z[CENTROID] + z[ROLLOFF]) / 4.0 + (z[MAJOR] - z[MINOR]) / 4.0,
            steadiness: -(z[LOUDNESS_STD] + z[CENTROID_STD]) / 2.0,
        }
    }

    pub const fn to_array(self) -> [f32; 3] {
        [self.energy, self.brightness, self.steadiness]
    }

    pub const fn from_array(a: [f32; 3]) -> Self {
        Self {
            energy: a[0],
            brightness: a[1],
            steadiness: a[2],
        }
    }

    pub fn distance(self, other: Self) -> f32 {
        let (a, b) = (self.to_array(), other.to_array());
        crate::index::euclid(&a, &b)
    }
}

impl Mood {
    pub const ALL: [Self; 6] = [Self::Calm, Self::Energetic, Self::Dark, Self::Bright, Self::Focus, Self::Drive];

    /// The card's title.
    pub const fn title(self) -> &'static str {
        match self {
            Self::Calm => "Спокойное",
            Self::Energetic => "Энергичное",
            Self::Dark => "Мрачное",
            Self::Bright => "Светлое",
            Self::Focus => "Для фокуса",
            Self::Drive => "Драйв",
        }
    }

    /// How much a track with these axes has this mood (higher = more).
    pub fn score(self, a: Axes) -> f32 {
        match self {
            Self::Calm => -a.energy,
            Self::Energetic => a.energy,
            Self::Dark => -a.brightness,
            Self::Bright => a.brightness,
            Self::Focus => a.steadiness - a.energy.max(0.0),
            Self::Drive => (a.energy + a.steadiness) / 2.0,
        }
    }

    /// Where the mood lies on the axes, half a standard deviation out
    /// (the daylist of a part of the day without its own history).
    pub fn direction(self) -> Axes {
        let (energy, brightness, steadiness) = match self {
            Self::Calm => (-0.5, 0.0, 0.0),
            Self::Energetic => (0.5, 0.0, 0.0),
            Self::Dark => (0.0, -0.5, 0.0),
            Self::Bright => (0.0, 0.5, 0.0),
            Self::Focus => (-0.25, 0.0, 0.5),
            Self::Drive => (0.35, 0.0, 0.35),
        };
        Axes {
            energy,
            brightness,
            steadiness,
        }
    }

    /// The six cards in the order for a part of the day: morning Светлое,
    /// Энергичное; day Энергичное, Для фокуса; evening Спокойное, Светлое;
    /// night Спокойное, Мрачное; the others after them in [`Self::ALL`]
    /// order. Nothing is hidden.
    pub fn order_for(part: DayPart) -> [Self; 6] {
        let first: [Self; 2] = match part {
            DayPart::Morning => [Self::Bright, Self::Energetic],
            DayPart::Day => [Self::Energetic, Self::Focus],
            DayPart::Evening => [Self::Calm, Self::Bright],
            DayPart::Night => [Self::Calm, Self::Dark],
        };
        let mut out = [first[0]; 6];
        out[1] = first[1];
        let mut k = 2;
        for m in Self::ALL {
            if !first.contains(&m) {
                out[k] = m;
                k += 1;
            }
        }
        out
    }
}

/// A mood's share of the library.
pub const MOOD_SHARE: f32 = 1.0 / 3.0;

impl Recs<'_> {
    /// The top third of the library's analysed tracks by `mood`'s score
    /// (disliked ones left out), best first, with the scores. Empty without
    /// analysis.
    pub fn mood_pool(&self, mood: Mood) -> Vec<(TrackId, f32)> {
        let mut all: Vec<(TrackId, f32)> = self
            .index
            .iter()
            .filter(|(id, _)| self.catalog.position(*id).is_some_and(|i| !self.blocked(i)))
            .map(|(id, v)| (id, mood.score(Axes::of(&self.norm.z(v)))))
            .collect();
        all.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let keep = ((all.len() as f32) * MOOD_SHARE).ceil() as usize;
        all.truncate(keep);
        all
    }

    /// The six mood cards in the order for now's part of the day.
    pub fn mood_order(&self) -> [Mood; 6] {
        Mood::order_for(self.taste.day_part())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_part_orders_cards_and_hides_nothing() {
        for part in DayPart::ALL {
            let order = Mood::order_for(part);
            let mut sorted = order;
            sorted.sort();
            let mut all = Mood::ALL;
            all.sort();
            assert_eq!(sorted, all);
        }
        assert_eq!(Mood::order_for(DayPart::Night)[..2], [Mood::Calm, Mood::Dark]);
        assert_eq!(Mood::order_for(DayPart::Morning)[..2], [Mood::Bright, Mood::Energetic]);
        assert_eq!(Mood::order_for(DayPart::Day)[..2], [Mood::Energetic, Mood::Focus]);
        assert_eq!(Mood::order_for(DayPart::Evening)[..2], [Mood::Calm, Mood::Bright]);
    }

    #[test]
    fn feature_indices_match_bliss() {
        use bliss_audio::AnalysisIndex as A;
        assert_eq!(A::Tempo as usize, TEMPO);
        assert_eq!(A::Zcr as usize, ZCR);
        assert_eq!(A::MeanSpectralCentroid as usize, CENTROID);
        assert_eq!(A::StdDeviationSpectralCentroid as usize, CENTROID_STD);
        assert_eq!(A::MeanSpectralRolloff as usize, ROLLOFF);
        assert_eq!(A::MeanLoudness as usize, LOUDNESS);
        assert_eq!(A::StdDeviationLoudness as usize, LOUDNESS_STD);
        assert_eq!(A::Chroma7 as usize, MAJOR);
        assert_eq!(A::Chroma8 as usize, MINOR);
    }
}
