//! «Дейлист» (docs/stage4.md §12.2): a selection of its own for each part
//! of the day, named by its sound and time («Тягучий вечер четверга»).
//!
//! * **Centre**: the mean point on the mood axes ([`Axes`]) of what was
//!   played in this part of the day, each analysed track weighted by its
//!   plays then ([`Recs::daylist_centre`]); with fewer than
//!   [`CENTRE_MIN_PLAYS`] such plays, the middle of the first two moods of
//!   the part's card order ([`Mood::order_for`]).
//! * **Pool**: the [`DAYLIST_SHARE`] of the analysed library nearest to the
//!   centre on the axes, plus tracks without a vector that were played in
//!   this part of the day. It plays as an endless radio
//!   ([`RadioSeed::Daylist`]): scored by closeness, affinity and the part's
//!   bonus, «Новое в радио» as everywhere, the diversity rules and the
//!   smooth order.
//! * **Name**: «<прилагательное> <часть дня> <день недели>» in the genitive
//!   of the day, the adjective agreeing with the part (утро neuter, день and
//!   вечер masculine, ночь feminine); a night belongs to the day it started
//!   ([`DayPart::day_of`]). The adjective comes from the centre's cell
//!   (energy × brightness × steadiness, 2–3 per cell), picked by the day
//!   and the part, so a name holds for its part of the day.

use nikgob_core::model::TrackId;

use crate::analysis::{DIM, Vector};
use crate::mood::{Axes, Mood};
use crate::pick::{NewTrack, Pick, Recs};
use crate::radio::{RadioParams, RadioSeed, RadioState};
use crate::rng::Rng;
use crate::taste::DayPart;

pub const DAYLIST_LEN: usize = 30;
/// Plays of analysed tracks a part of the day needs for its own centre.
pub const CENTRE_MIN_PLAYS: u32 = 5;
/// The daylist draws from this share of the library nearest its centre.
pub const DAYLIST_SHARE: f32 = 1.0 / 3.0;
/// The tracks nearest the centre whose mean is its point in the bliss
/// space (the radio's pull).
const CORE: usize = 24;

/// Where a daylist is centred.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Centre {
    pub axes: Axes,
    /// From the plays of this part of the day (not the moods' fallback).
    pub from_history: bool,
}

impl Recs<'_> {
    /// The daylist's centre for `part` (see the module docs).
    pub fn daylist_centre(&self, part: DayPart) -> Centre {
        let mut sum = [0.0f32; 3];
        let mut plays = 0u32;
        for (id, v) in self.index.iter() {
            let Some(i) = self.catalog.position(id) else { continue };
            let n = u32::from(self.taste.daypart_plays_at(i)[part.index()]);
            if n == 0 {
                continue;
            }
            let a = Axes::of(&self.norm.z(v)).to_array();
            for (s, x) in sum.iter_mut().zip(a) {
                *s += n as f32 * x;
            }
            plays += n;
        }
        if plays >= CENTRE_MIN_PLAYS {
            return Centre {
                axes: Axes::from_array(sum.map(|s| s / plays as f32)),
                from_history: true,
            };
        }
        let [a, b, ..] = Mood::order_for(part);
        let (a, b) = (a.direction().to_array(), b.direction().to_array());
        Centre {
            axes: Axes::from_array(std::array::from_fn(|k| (a[k] + b[k]) / 2.0)),
            from_history: false,
        }
    }

    /// The analysed tracks nearest `centre` on the axes (disliked ones left
    /// out), [`DAYLIST_SHARE`] of them, nearest first, with a closeness from
    /// 1 (the nearest) down to 0.
    pub fn daylist_pool(&self, centre: Axes) -> Vec<(TrackId, f32)> {
        let mut all: Vec<(TrackId, f32)> = self
            .index
            .iter()
            .filter(|(id, _)| self.catalog.position(*id).is_some_and(|i| !self.blocked(i)))
            .map(|(id, v)| (id, Axes::of(&self.norm.z(v)).distance(centre)))
            .collect();
        all.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let keep = ((all.len() as f32) * DAYLIST_SHARE).ceil() as usize;
        all.truncate(keep);
        let n = all.len().max(2) - 1;
        all.into_iter().enumerate().map(|(k, (id, _))| (id, 1.0 - k as f32 / n as f32)).collect()
    }

    /// The centre's point in the weighted bliss space: the mean of the
    /// tracks nearest to it on the axes. `None` without analysis.
    pub(crate) fn daylist_point(&self, centre: Axes) -> Option<Vector> {
        let core: Vec<Vector> = self
            .daylist_pool(centre)
            .into_iter()
            .take(CORE)
            .filter_map(|(id, _)| self.embedded(id))
            .collect();
        if core.is_empty() {
            return None;
        }
        let mut sum = [0.0f32; DIM];
        for v in &core {
            for (s, x) in sum.iter_mut().zip(v) {
                *s += x;
            }
        }
        Some(sum.map(|s| s / core.len() as f32))
    }

    /// The daylist of `part`: its first `len` picks and the radio that
    /// plays on from them. `new` are new-music candidates, best first.
    pub fn daylist(&self, part: DayPart, new: &[NewTrack], len: usize, params: &RadioParams) -> (Centre, RadioState, Vec<Pick>) {
        let centre = self.daylist_centre(part);
        let mut radio = RadioState::new(RadioSeed::Daylist {
            part,
            centre: centre.axes.to_array(),
        });
        radio.add_new(new.iter().cloned());
        let picks = radio.extend(self, len, params);
        (centre, radio, picks)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gender {
    Masculine,
    Neuter,
    Feminine,
}

/// The noun of a part of the day and its gender.
const fn noun(part: DayPart) -> (&'static str, Gender) {
    match part {
        DayPart::Morning => ("утро", Gender::Neuter),
        DayPart::Day => ("день", Gender::Masculine),
        DayPart::Evening => ("вечер", Gender::Masculine),
        DayPart::Night => ("ночь", Gender::Feminine),
    }
}

/// The weekday of a local day number (1970-01-01 was a Thursday), in the
/// genitive: «понедельника» … «воскресенья».
pub fn weekday_genitive(day: i64) -> &'static str {
    const NAMES: [&str; 7] = ["понедельника", "вторника", "среды", "четверга", "пятницы", "субботы", "воскресенья"];
    NAMES[(day + 3).rem_euclid(7) as usize]
}

/// Adjectives (masculine) by the cell of the axes: index
/// `energy ≥ 0` × 4 + `brightness ≥ 0` × 2 + `steadiness ≥ 0`.
const ADJECTIVES: [&[&str]; 8] = [
    // Quiet, dark, restless.
    &["Мрачный", "Туманный", "Сумрачный"],
    // Quiet, dark, steady.
    &["Тягучий", "Тихий", "Глубокий"],
    // Quiet, bright, restless.
    &["Мечтательный", "Воздушный", "Лёгкий"],
    // Quiet, bright, steady.
    &["Ясный", "Нежный", "Тёплый"],
    // Loud, dark, restless.
    &["Бурный", "Мятежный", "Колючий"],
    // Loud, dark, steady.
    &["Напористый", "Хлёсткий", "Упрямый"],
    // Loud, bright, restless.
    &["Яркий", "Озорной", "Пёстрый"],
    // Loud, bright, steady.
    &["Бодрый", "Солнечный", "Звонкий"],
];

fn cell(a: Axes) -> usize {
    usize::from(a.energy >= 0.0) * 4 + usize::from(a.brightness >= 0.0) * 2 + usize::from(a.steadiness >= 0.0)
}

/// A masculine adjective («-ый», «-ой», «-ий») in `gender`.
fn agree(masculine: &str, gender: Gender) -> String {
    if gender == Gender::Masculine {
        return masculine.to_owned();
    }
    let mut chars: Vec<char> = masculine.chars().collect();
    let ending: String = chars.split_off(chars.len().saturating_sub(2)).into_iter().collect();
    let stem: String = chars.iter().collect();
    let last = chars.last().copied().unwrap_or(' ');
    let (neuter, feminine) = match ending.as_str() {
        "ий" if "жчшщ".contains(last) => ("ее", "ая"),
        "ий" if "гкх".contains(last) => ("ое", "ая"),
        // A soft stem («синий»).
        "ий" => ("ее", "яя"),
        _ => ("ое", "ая"),
    };
    let ending = if gender == Gender::Neuter { neuter } else { feminine };
    format!("{stem}{ending}")
}

/// The daylist's name: «Тягучий вечер четверга». `day` is the local day the
/// part belongs to ([`DayPart::day_of`]); the adjective is the centre's,
/// picked by the day and the part.
pub fn daylist_title(centre: Axes, part: DayPart, day: i64) -> String {
    let words = ADJECTIVES[cell(centre)];
    let pick = Rng::derive(day as u64, 0xDA7 + part.index() as u64).below(words.len());
    let (noun, gender) = noun(part);
    format!("{} {noun} {}", agree(words[pick], gender), weekday_genitive(day))
}

/// «до 23:00»: when the part of the day ends.
pub fn daylist_until(part: DayPart) -> String {
    format!("до {:02}:00", part.end_hour())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::VectorIndex;
    use crate::pick::{Item, Reason, assert_diverse};
    use crate::taste::{Catalog, DAY_MS, HOUR_MS, Play, Taste, TasteInput};
    use crate::test_util::*;
    use nikgob_core::model::EventType;

    const NOW: i64 = 2_000 * DAY_MS;

    #[test]
    fn every_adjective_agrees_with_every_part() {
        let forms: [(&str, &str, &str); 24] = [
            ("Мрачный", "Мрачное", "Мрачная"),
            ("Туманный", "Туманное", "Туманная"),
            ("Сумрачный", "Сумрачное", "Сумрачная"),
            ("Тягучий", "Тягучее", "Тягучая"),
            ("Тихий", "Тихое", "Тихая"),
            ("Глубокий", "Глубокое", "Глубокая"),
            ("Мечтательный", "Мечтательное", "Мечтательная"),
            ("Воздушный", "Воздушное", "Воздушная"),
            ("Лёгкий", "Лёгкое", "Лёгкая"),
            ("Ясный", "Ясное", "Ясная"),
            ("Нежный", "Нежное", "Нежная"),
            ("Тёплый", "Тёплое", "Тёплая"),
            ("Бурный", "Бурное", "Бурная"),
            ("Мятежный", "Мятежное", "Мятежная"),
            ("Колючий", "Колючее", "Колючая"),
            ("Напористый", "Напористое", "Напористая"),
            ("Хлёсткий", "Хлёсткое", "Хлёсткая"),
            ("Упрямый", "Упрямое", "Упрямая"),
            ("Яркий", "Яркое", "Яркая"),
            ("Озорной", "Озорное", "Озорная"),
            ("Пёстрый", "Пёстрое", "Пёстрая"),
            ("Бодрый", "Бодрое", "Бодрая"),
            ("Солнечный", "Солнечное", "Солнечная"),
            ("Звонкий", "Звонкое", "Звонкая"),
        ];
        let all: Vec<&str> = ADJECTIVES.iter().flat_map(|c| c.iter().copied()).collect();
        assert_eq!(all.len(), forms.len());
        for (m, n, f) in forms {
            assert!(all.contains(&m), "{m} is not in the table");
            assert_eq!(agree(m, Gender::Masculine), m);
            assert_eq!(agree(m, Gender::Neuter), n);
            assert_eq!(agree(m, Gender::Feminine), f);
        }
        for c in ADJECTIVES {
            assert!((2..=3).contains(&c.len()));
        }
        assert_eq!(agree("Синий", Gender::Feminine), "Синяя");
        assert_eq!(agree("Синий", Gender::Neuter), "Синее");
    }

    #[test]
    fn names_agree_and_nights_belong_to_their_day() {
        // Day 0 was a Thursday.
        assert_eq!(weekday_genitive(0), "четверга");
        assert_eq!(weekday_genitive(4), "понедельника");
        assert_eq!(weekday_genitive(-1), "среды");
        let words = |a: Axes| ADJECTIVES[cell(a)];
        let quiet_dark_steady = Axes::from_array([-0.4, -0.3, 0.2]);
        let title = daylist_title(quiet_dark_steady, DayPart::Evening, 0);
        assert!(title.ends_with(" вечер четверга"), "{title}");
        assert!(words(quiet_dark_steady).iter().any(|w| title.starts_with(w)), "{title}");
        let loud_bright_steady = Axes::from_array([0.4, 0.3, 0.2]);
        let title = daylist_title(loud_bright_steady, DayPart::Morning, 4);
        assert!(title.ends_with("ое утро понедельника"), "{title}");
        let quiet_dark_restless = Axes::from_array([-0.4, -0.3, -0.2]);
        let title = daylist_title(quiet_dark_restless, DayPart::Night, 1);
        assert!(title.ends_with("ая ночь пятницы"), "{title}");
        // 02:00 of a Saturday is still Friday's night.
        let ts = 2 * DAY_MS + 2 * HOUR_MS;
        let day = DayPart::day_of(ts, 0);
        assert_eq!(DayPart::at(ts, 0), DayPart::Night);
        assert!(daylist_title(quiet_dark_restless, DayPart::Night, day).ends_with("пятницы"));
        // The same day and part, the same name; the days vary it.
        assert_eq!(daylist_title(quiet_dark_steady, DayPart::Evening, 9), daylist_title(quiet_dark_steady, DayPart::Evening, 9));
        let names: std::collections::HashSet<String> =
            (0..21).map(|d| daylist_title(quiet_dark_steady, DayPart::Evening, d * 7)).collect();
        assert!(names.len() >= 2, "{names:?}");
        assert_eq!(daylist_until(DayPart::Evening), "до 23:00");
        assert_eq!(daylist_until(DayPart::Night), "до 06:00");
    }

    /// 90 tracks by 45 artists, energy growing with the number; tracks 60..90
    /// are played in the evening, 0..30 in the morning; track 75 disliked;
    /// 10 tracks without a vector (90..100), 95 played in the evening.
    fn library() -> (Catalog, VectorIndex<TrackId>, TasteInput) {
        let cat = Catalog::new((0..100u128).map(|i| info(i, &[(i % 45) as u32], Some(5_000 + i))));
        let mut idx = VectorIndex::new();
        for i in 0..90u128 {
            let e = i as f32 / 90.0;
            let mut v = [0.0f32; DIM];
            for f in [0, 1, 2, 8] {
                v[f] += e;
            }
            idx.insert(tid(i), v);
        }
        let mut events = Vec::new();
        let at = |day: i64, hour: i64| NOW - day * DAY_MS - NOW.rem_euclid(DAY_MS) + hour * HOUR_MS;
        for d in 1..4 {
            for i in (60..90u128).chain([95]) {
                events.push(Play { track: tid(i), ts: at(d, 20), kind: EventType::Complete, position_ms: None, context: Default::default(), origin: None, end_reason: None, attention: None });
            }
            for i in 0..30u128 {
                events.push(Play { track: tid(i), ts: at(d, 8), kind: EventType::Complete, position_ms: None, context: Default::default(), origin: None, end_reason: None, attention: None });
            }
        }
        let input = TasteInput {
            events,
            disliked_tracks: vec![tid(75)],
            ..Default::default()
        };
        (cat, idx, input)
    }

    fn num(p: &Pick) -> u128 {
        match &p.item {
            Item::Track(id) => id.0.as_u128(),
            Item::Stream(_) => u128::MAX,
        }
    }

    #[test]
    fn the_daylist_follows_what_the_part_of_the_day_plays() {
        let (cat, idx, input) = library();
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let params = RadioParams { new_share: 0.0, seed: 3 };

        let evening = recs.daylist_centre(DayPart::Evening);
        let morning = recs.daylist_centre(DayPart::Morning);
        assert!(evening.from_history && morning.from_history);
        assert!(evening.axes.energy > 0.5 && morning.axes.energy < -0.5, "{evening:?} {morning:?}");
        let (_, mut radio, picks) = recs.daylist(DayPart::Evening, &[], 20, &params);
        assert_eq!(picks.len(), 20);
        let loud = picks.iter().filter(|p| (55..90).contains(&num(p)) || num(p) == 95).count();
        assert!(loud >= 18, "the evening is loud: {loud} of {:?}", picks.iter().map(num).collect::<Vec<_>>());
        assert!(picks.iter().all(|p| num(p) != 75), "disliked");
        assert!(picks.iter().any(|p| num(p) == 95), "a track without a vector played in the evening");
        assert!(picks.iter().any(|p| p.reason == Reason::DayPart(DayPart::Evening)));
        assert_diverse(&recs, &picks);
        // It plays on as a radio, without repeats, and survives serde.
        let json = serde_json::to_string(&radio).unwrap();
        radio = serde_json::from_str(&json).unwrap();
        let more = radio.extend(&recs, 10, &params);
        assert_eq!(more.len(), 10);
        assert!(more.iter().all(|p| !picks.contains(p)));
        // Deterministic for a seed, different for another («Обновить»).
        assert_eq!(picks, recs.daylist(DayPart::Evening, &[], 20, &params).2);
        let other = |seed| recs.daylist(DayPart::Evening, &[], 10, &RadioParams { new_share: 0.0, seed }).2;
        assert!((4..10).any(|seed| other(seed) != other(3)));

        let (_, _, picks) = recs.daylist(DayPart::Morning, &[], 20, &params);
        let quiet = picks.iter().filter(|p| num(p) < 35).count();
        assert!(quiet >= 18, "the morning is quiet: {quiet}");
    }

    #[test]
    fn a_part_without_history_takes_its_moods() {
        let (cat, idx, input) = library();
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        // Nothing played in the day: Энергичное and Для фокуса.
        let day = recs.daylist_centre(DayPart::Day);
        assert!(!day.from_history);
        let want = Axes::from_array([0.125, 0.0, 0.25]);
        assert!(day.axes.distance(want) < 1e-6, "{day:?}");
        // Without any analysis the daylist is what this part plays.
        let empty = VectorIndex::new();
        let recs = Recs::new(&cat, &taste, &empty);
        let (centre, _, picks) = recs.daylist(DayPart::Evening, &[], 15, &RadioParams { new_share: 0.0, seed: 1 });
        assert!(!centre.from_history);
        assert_eq!(picks.len(), 15);
        let evening = picks.iter().filter(|p| (60..90).contains(&num(p)) || num(p) == 95).count();
        assert!(evening >= 13, "{evening}");
    }

    #[test]
    fn new_music_takes_its_share() {
        let (cat, idx, input) = library();
        let taste = Taste::build(&cat, &input, NOW);
        let recs = Recs::new(&cat, &taste, &idx);
        let new: Vec<NewTrack> = (0..10)
            .map(|k| NewTrack {
                id: nikgob_core::streaming::RemoteId::new(nikgob_core::streaming::Service::Deezer, format!("{k}")),
                artist: format!("someone {k}"),
                album: None,
                vector: None,
            })
            .collect();
        let (_, _, picks) = recs.daylist(DayPart::Evening, &new, 30, &RadioParams { new_share: 0.2, seed: 1 });
        assert_eq!(picks.iter().filter(|p| matches!(p.item, Item::Stream(_))).count(), 6);
    }
}
