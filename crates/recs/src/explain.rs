//! Why a mix or a pick: one line, only what can be checked,
//! by what the pick came from and the facts behind it.
//!
//! * **A moment** of «Миксы для тебя» ([`Recs::why_moment`]): «Из того,
//!   что ты дослушиваешь вечером, — без слов» — the part of the day when
//!   its songs are heard out [`WHEN_RATIO`] times as often as the user's
//!   others are (and [`WHEN_SHARE`] of them then), the kind when
//!   [`KIND_SHARE`] of it is without words and the name does not say so;
//!   no line with neither.
//! * **A world's mix** ([`Recs::why_world_mix`]): «Что ты дослушивал чаще
//!   всего в MADE IN ABYSS» for «Лучшее» of a history, «Ты не слышал; по
//!   звуку ближе всего к Chain of the Abyss» for «Ещё не слышал» (the loved
//!   track nearest its first, only within the median distance), «Звучит как
//!   твой MADE IN ABYSS, но из других миров» for «В дорогу».
//! * **A radio's pick** ([`Recs::why_pick`]): its [`Reason`], which the radio
//!   chose by the feature of the largest share of its score — «После
//!   Рассел кроу: часто идёт следом», «Похоже по звуку» (only within the
//!   median distance of what it was picked after), «Следующий на альбоме».
//! * **New music** ([`new_found`], [`suggested`]): «Новое: нашлось по Ghost
//!   Avenue в YouTube Music», «Новое: советует Deezer».
//! * **«На повторе»** ([`on_repeat`]): «Ты включал это 12 раз за неделю».
//!
//! Nothing promises «понравится»; a name is never said again; only the
//! first pick of a radio and the new ones are explained (the runtime's
//! choice).

use nikgob_core::i18n::{Locale, PerLocale, PluralForms, plural_forms};
use nikgob_core::model::TrackId;

use crate::index::euclid;
use crate::pick::{Pick, Reason, Recs};
use crate::profiles::Kind;
use crate::taste::DayPart;
use crate::world_mixes::{WorldMix, WorldMixKind};

/// A part of the day is a moment's when its songs are heard out then this
/// many times as often as the user's songs are…
pub const WHEN_RATIO: f32 = 1.3;
/// …and this share of their completes is then.
pub const WHEN_SHARE: f32 = 0.4;
/// A mix is «без слов» with this share of it without words.
pub const KIND_SHARE: f32 = 0.8;
/// Completes a moment needs for its part of the day.
const WHEN_MIN: u32 = 8;

/// «вечером», «после полуночи» / «in the evening», «at night»…
pub const fn when_in(part: DayPart, loc: Locale) -> &'static str {
    match (loc, part) {
        (Locale::Ru, DayPart::Morning) => "утром",
        (Locale::Ru, DayPart::Day) => "днём",
        (Locale::Ru, DayPart::Evening) => "вечером",
        (Locale::Ru, DayPart::Night) => "ночью",
        (Locale::En, DayPart::Morning) => "in the morning",
        (Locale::En, DayPart::Day) => "during the day",
        (Locale::En, DayPart::Evening) => "in the evening",
        (Locale::En, DayPart::Night) => "at night",
    }
}

/// «Новое: нашлось по Ghost Avenue в YouTube Music» / «New: found via Ghost Avenue on YouTube Music».
pub fn new_found(seed: &str, service: &str, loc: Locale) -> String {
    match loc {
        Locale::Ru => format!("Новое: нашлось по {seed} в {service}"),
        Locale::En => format!("New: found via {seed} on {service}"),
    }
}

/// A new song the service itself suggests: «Новое: советует Deezer» / «New: suggested by Deezer».
pub fn suggested(service: &str, loc: Locale) -> String {
    match loc {
        Locale::Ru => format!("Новое: советует {service}"),
        Locale::En => format!("New: suggested by {service}"),
    }
}

/// «Ты включал это 12 раз за неделю» / «You played this 12 times this week»; none under two.
pub fn on_repeat(starts: u32, loc: Locale) -> Option<String> {
    if starts < 2 {
        return None;
    }
    Some(match loc {
        Locale::Ru => {
            let unit = plural_forms(loc, u64::from(starts), &PluralForms { one: "раз", few: "раза", many: "раз" });
            format!("Ты включал это {starts} {unit} за неделю")
        }
        Locale::En => {
            let unit = plural_forms(loc, u64::from(starts), &PluralForms { one: "time", few: "times", many: "times" });
            format!("You played this {starts} {unit} this week")
        }
    })
}

impl Recs<'_> {
    /// Why a moment with `tracks` (catalog positions) and profile/mix `kind`
    /// (see the module docs); none before anything of it was heard out, or
    /// with neither a part of the day nor a wordless note of its own.
    pub fn why_moment(&self, tracks: &[usize], kind: Option<Kind>, loc: Locale) -> Option<String> {
        let m = self.model?;
        if tracks.is_empty() {
            return None;
        }
        let sum = |done: &[u16; 4], into: &mut [u32; 4]| {
            for (k, d) in done.iter().enumerate() {
                into[k] += u32::from(*d);
            }
        };
        let mut here = [0u32; 4];
        for &i in tracks {
            sum(&m.tracks[i].done, &mut here);
        }
        let mine: u32 = here.iter().sum();
        if mine == 0 {
            return None;
        }
        let mut all = [0u32; 4];
        for t in &m.tracks {
            sum(&t.done, &mut all);
        }
        let total: u32 = all.iter().sum::<u32>().max(1);
        let part = DayPart::ALL
            .into_iter()
            .filter(|_| mine >= WHEN_MIN)
            .map(|p| {
                let k = p.index();
                let share = here[k] as f32 / mine as f32;
                let usual = all[k] as f32 / total as f32;
                (share, share / usual.max(1e-3), p)
            })
            .filter(|(share, ratio, _)| *share >= WHEN_SHARE && *ratio >= WHEN_RATIO)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|x| x.2);
        let wordless = tracks.iter().filter(|&&i| m.song_type_at(i).voice == crate::songtype::Voice::Instrumental).count();
        let instrumental =
            wordless as f32 >= KIND_SHARE * tracks.len() as f32 && !matches!(kind, Some(Kind::Instrumental | Kind::InstrumentalSoundtracks));
        // Nothing of its own to say: no line (the same on every card says
        // nothing).
        if part.is_none() && !instrumental {
            return None;
        }
        let mut line = match loc {
            Locale::Ru => "Из того, что ты дослушиваешь".to_owned(),
            Locale::En => "Tracks you finish listening to".to_owned(),
        };
        if let Some(p) = part {
            line.push(' ');
            line.push_str(when_in(p, loc));
        }
        if instrumental {
            match loc {
                Locale::Ru => line.push_str(", — без слов"),
                Locale::En => line.push_str(" — instrumental"),
            }
        }
        Some(line)
    }

    /// Why a mix of the world `world` (its name); `title` names a track
    /// (catalog position).
    pub fn why_world_mix(&self, mix: &WorldMix, world: &str, title: &dyn Fn(usize) -> Option<String>, loc: Locale) -> Option<String> {
        let m = self.model?;
        match mix.kind {
            WorldMixKind::Best => {
                // Of a history, not the albums' first tracks standing in.
                let heard = mix.tracks.iter().filter(|&&i| m.tracks[i].completes > 0).count();
                (heard * 2 >= mix.tracks.len() && heard > 0).then(|| match loc {
                    Locale::Ru => format!("Что ты дослушивал чаще всего в {world}"),
                    Locale::En => format!("What you finished most in {world}"),
                })
            }
            WorldMixKind::Unheard => {
                let near = mix.near.and_then(|n| {
                    let a = m.embedded_at(*mix.tracks.first()?)?;
                    let b = m.embedded_at(n)?;
                    (euclid(a, b) <= m.scale()).then_some(n)
                });
                Some(match (loc, near.and_then(title)) {
                    (Locale::Ru, Some(t)) => format!("Ты не слышал; по звуку ближе всего к {t}"),
                    (Locale::Ru, None) => "Ты не слышал ничего из этого".to_owned(),
                    (Locale::En, Some(t)) => format!("Unheard; closest in sound to {t}"),
                    (Locale::En, None) => "None of these heard yet".to_owned(),
                })
            }
            WorldMixKind::Road => Some(match loc {
                Locale::Ru => format!("Звучит как твой {world}, но из других миров"),
                Locale::En => format!("Sounds like your {world}, from other worlds"),
            }),
            WorldMixKind::Calm | WorldMixKind::Moving => None,
        }
    }

    /// Bilingual explanation pair for a radio's pick `p` after `before` (see the module docs);
    /// `title` names a track. None for what the module docs leave out.
    pub fn why_pick_pair(&self, p: &Pick, before: Option<TrackId>, title: &dyn Fn(TrackId) -> Option<String>) -> Option<PerLocale<String>> {
        let crate::Item::Track(id) = &p.item else { return None };
        Some(match p.reason {
            Reason::NextOnAlbum => PerLocale {
                ru: "Следующий на альбоме".to_owned(),
                en: "Next on album".to_owned(),
            },
            Reason::FollowsOften(t) => {
                let name = title(t)?;
                PerLocale {
                    ru: format!("После {name}: часто идёт следом"),
                    en: format!("Often follows {name}"),
                }
            }
            Reason::ListenedTogether(t) => {
                let name = title(t)?;
                PerLocale {
                    ru: format!("Часто слушают вместе с {name}"),
                    en: format!("Often played with {name}"),
                }
            }
            Reason::SimilarTo(t) => self.near(*id, Some(t)).then(|| PerLocale {
                ru: "Похоже по звуку".to_owned(),
                en: "Similar sound".to_owned(),
            })?,
            Reason::SoundsLikeSeed => self.near(*id, before).then(|| PerLocale {
                ru: "Похоже по звуку".to_owned(),
                en: "Similar sound".to_owned(),
            })?,
            _ => return None,
        })
    }

    /// Why a radio's pick `p`, played after `before` (see the module docs);
    /// `title` names a track. None for what the module docs leave out.
    pub fn why_pick(&self, p: &Pick, before: Option<TrackId>, title: &dyn Fn(TrackId) -> Option<String>, loc: Locale) -> Option<String> {
        self.why_pick_pair(p, before, title).map(|pair| pair.get(loc).clone())
    }

    /// `a` sounds within the median distance of `b`.
    fn near(&self, a: TrackId, b: Option<TrackId>) -> bool {
        let Some(m) = self.model else { return false };
        let at = |t: TrackId| m.position(t).and_then(|i| m.embedded_at(i));
        match (at(a), b.and_then(at)) {
            (Some(x), Some(y)) => euclid(x, y) <= m.scale(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::DIM;
    use crate::index::VectorIndex;
    use crate::model::{ModelInput, UserModel};
    use crate::taste::{Catalog, Play, TasteInput};
    use crate::test_util::*;
    use crate::world_mixes::UNHEARD_LEN;
    use nikgob_core::model::EventType;

    const DAY: i64 = crate::taste::DAY_MS;
    const T0: i64 = 20_000 * DAY;

    /// Forty tracks of one world along a line of sound: the first ten
    /// heard out in the evenings, the next twenty by day, the rest never.
    fn library() -> (Catalog, VectorIndex<TrackId>, TasteInput) {
        let mut tracks = Vec::new();
        let mut idx = VectorIndex::with_weights([1.0; DIM]);
        for i in 0..40u128 {
            tracks.push(info(i, &[(i % 7) as u32], Some(i % 5)).with_world("the world").with_duration(Some(200_000)));
            idx.insert(tid(i), point(i as f32, 0.0));
        }
        let mut events = Vec::new();
        for i in 0..30u128 {
            let hour = if i < 10 { 20 } else { 13 };
            for day in 1..4i64 {
                let ts = T0 - day * DAY - (i as i64) * 60_000 + hour * 3_600_000 - DAY;
                events.push(Play::new(tid(i), ts, EventType::Start, Some(0)));
                events.push(Play::new(tid(i), ts + 200_000, EventType::Complete, Some(200_000)));
            }
        }
        let likes = (0..10).map(tid).collect();
        (Catalog::new(tracks), idx, TasteInput { events, likes, ..TasteInput::default() })
    }

    /// A moment says the part of the day its songs are heard out in when
    /// it stands out, and nothing of the day when it does not.
    #[test]
    fn a_moment_says_when_it_is_heard_out() {
        let (cat, idx, input) = library();
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, T0);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let at = |r: std::ops::Range<u128>| -> Vec<usize> { r.filter_map(|i| model.position(tid(i))).collect() };
        assert_eq!(recs.why_moment(&at(0..10), Some(Kind::Songs), Locale::Ru).as_deref(), Some("Из того, что ты дослушиваешь вечером"));
        assert_eq!(recs.why_moment(&at(0..10), Some(Kind::Songs), Locale::En).as_deref(), Some("Tracks you finish listening to in the evening"));
        assert_eq!(recs.why_moment(&at(0..30), None, Locale::Ru), None, "no part of the day of its own");
        assert_eq!(recs.why_moment(&at(30..40), Some(Kind::Songs), Locale::En), None, "nothing of it heard out");
    }

    /// A moment of mostly instrumental tracks says so only when its typed
    /// [`Kind`] is not already instrumental (even in English).
    #[test]
    fn a_moment_checks_instrumental_by_kind_not_title() {
        use crate::songtype::{SongType, SongTypes, Voice};
        let (cat, idx, input) = library();
        let mut types = SongTypes::default();
        for i in 0..40u128 {
            types.by_track.insert(tid(i), SongType { voice: Voice::Instrumental, ..SongType::default() });
        }
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, T0);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let at = |r: std::ops::Range<u128>| -> Vec<usize> { r.filter_map(|i| model.position(tid(i))).collect() };
        assert_eq!(
            recs.why_moment(&at(0..10), Some(Kind::Calm), Locale::Ru).as_deref(),
            Some("Из того, что ты дослушиваешь вечером, — без слов")
        );
        assert_eq!(
            recs.why_moment(&at(0..10), Some(Kind::Calm), Locale::En).as_deref(),
            Some("Tracks you finish listening to in the evening — instrumental")
        );
        assert_eq!(
            recs.why_moment(&at(0..10), Some(Kind::Instrumental), Locale::Ru).as_deref(),
            Some("Из того, что ты дослушиваешь вечером")
        );
        assert_eq!(
            recs.why_moment(&at(0..10), Some(Kind::InstrumentalSoundtracks), Locale::En).as_deref(),
            Some("Tracks you finish listening to in the evening")
        );
        assert_eq!(
            recs.why_moment(&at(0..30), Some(Kind::Calm), Locale::En).as_deref(),
            Some("Tracks you finish listening to — instrumental")
        );
        assert_eq!(recs.why_moment(&at(0..30), Some(Kind::Instrumental), Locale::En), None);
    }

    /// «Ещё не слышал» names the loved track nearest its first, «Лучшее»
    /// is of what was heard out; a pick far from its seed is not «похоже».
    #[test]
    fn a_world_mix_says_its_anchor() {
        let (cat, idx, input) = library();
        let model = UserModel::build(&cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, T0);
        let recs = Recs::new(&cat, &model.taste, &idx).with_model(&model);
        let mixes = recs.world_mixes(crate::world_token("the world"), DayPart::Day);
        let name = |i: usize| Some(format!("T{}", model.id(i).0.as_u128()));
        let of = |k: WorldMixKind| mixes.iter().find(|m| m.kind == k).unwrap_or_else(|| panic!("{k:?}"));
        let unheard = of(WorldMixKind::Unheard);
        assert_eq!(unheard.tracks.len(), UNHEARD_LEN.min(10));
        let why = recs.why_world_mix(unheard, "Мир", &name, Locale::Ru).unwrap();
        assert!(why.starts_with("Ты не слышал; по звуку ближе всего к T"), "{why}");
        let why_en = recs.why_world_mix(unheard, "World", &name, Locale::En).unwrap();
        assert!(why_en.starts_with("Unheard; closest in sound to T"), "{why_en}");
        assert_eq!(recs.why_world_mix(of(WorldMixKind::Best), "Мир", &name, Locale::Ru).as_deref(), Some("Что ты дослушивал чаще всего в Мир"));
        assert_eq!(recs.why_world_mix(of(WorldMixKind::Best), "World", &name, Locale::En).as_deref(), Some("What you finished most in World"));
        let road = crate::WorldMix { kind: WorldMixKind::Road, tracks: vec![0], near: None };
        assert_eq!(recs.why_world_mix(&road, "Мир", &name, Locale::Ru).as_deref(), Some("Звучит как твой Мир, но из других миров"));
        assert_eq!(recs.why_world_mix(&road, "World", &name, Locale::En).as_deref(), Some("Sounds like your World, from other worlds"));
        let pick = |i: u128, reason| Pick { item: crate::Item::Track(tid(i)), reason };
        let title = |_: TrackId| Some("Рассел кроу".to_owned());
        assert_eq!(recs.why_pick(&pick(1, Reason::NextOnAlbum), None, &title, Locale::Ru).as_deref(), Some("Следующий на альбоме"));
        assert_eq!(recs.why_pick(&pick(1, Reason::NextOnAlbum), None, &title, Locale::En).as_deref(), Some("Next on album"));
        assert_eq!(
            recs.why_pick(&pick(1, Reason::FollowsOften(tid(0))), None, &title, Locale::Ru).as_deref(),
            Some("После Рассел кроу: часто идёт следом")
        );
        assert_eq!(recs.why_pick(&pick(1, Reason::FollowsOften(tid(0))), None, &title, Locale::En).as_deref(), Some("Often follows Рассел кроу"));
        assert_eq!(recs.why_pick(&pick(1, Reason::SimilarTo(tid(0))), None, &title, Locale::Ru).as_deref(), Some("Похоже по звуку"));
        assert_eq!(recs.why_pick(&pick(1, Reason::SimilarTo(tid(0))), None, &title, Locale::En).as_deref(), Some("Similar sound"));
        assert_eq!(
            recs.why_pick_pair(&pick(1, Reason::ListenedTogether(tid(0))), None, &title),
            Some(PerLocale {
                ru: "Часто слушают вместе с Рассел кроу".to_owned(),
                en: "Often played with Рассел кроу".to_owned(),
            })
        );
        assert_eq!(recs.why_pick(&pick(39, Reason::SimilarTo(tid(0))), None, &title, Locale::Ru), None, "beyond the median distance");
    }

    #[test]
    fn says_the_count_in_russian_and_english() {
        assert_eq!(on_repeat(1, Locale::Ru), None);
        assert_eq!(on_repeat(1, Locale::En), None);
        assert_eq!(on_repeat(3, Locale::Ru).as_deref(), Some("Ты включал это 3 раза за неделю"));
        assert_eq!(on_repeat(12, Locale::Ru).as_deref(), Some("Ты включал это 12 раз за неделю"));
        assert_eq!(on_repeat(22, Locale::Ru).as_deref(), Some("Ты включал это 22 раза за неделю"));
        assert_eq!(on_repeat(2, Locale::En).as_deref(), Some("You played this 2 times this week"));
        assert_eq!(on_repeat(12, Locale::En).as_deref(), Some("You played this 12 times this week"));
        assert_eq!(new_found("Ghost Avenue", "YouTube Music", Locale::Ru), "Новое: нашлось по Ghost Avenue в YouTube Music");
        assert_eq!(new_found("Ghost Avenue", "YouTube Music", Locale::En), "New: found via Ghost Avenue on YouTube Music");
        assert_eq!(suggested("Deezer", Locale::Ru), "Новое: советует Deezer");
        assert_eq!(suggested("Deezer", Locale::En), "New: suggested by Deezer");
    }
}
