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

use nikgob_core::model::TrackId;

use crate::index::euclid;
use crate::pick::{Pick, Reason, Recs};
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

/// «вечером», «после полуночи»…
pub const fn when(part: DayPart) -> &'static str {
    match part {
        DayPart::Morning => "утром",
        DayPart::Day => "днём",
        DayPart::Evening => "вечером",
        DayPart::Night => "ночью",
    }
}

/// «Новое: нашлось по Ghost Avenue в YouTube Music».
pub fn new_found(seed: &str, service: &str) -> String {
    format!("Новое: нашлось по {seed} в {service}")
}

/// A new song the service itself suggests: «Новое: советует Deezer».
pub fn suggested(service: &str) -> String {
    format!("Новое: советует {service}")
}

/// «Ты включал это 12 раз за неделю»; none under two.
pub fn on_repeat(starts: u32) -> Option<String> {
    (starts >= 2).then(|| format!("Ты включал это {starts} {} за неделю", times(starts)))
}

fn times(n: u32) -> &'static str {
    match (n % 10, n % 100) {
        (2..=4, x) if !(12..=14).contains(&x) => "раза",
        _ => "раз",
    }
}

impl Recs<'_> {
    /// Why a moment with `tracks` (catalog positions) and the name `title`
    /// (see the module docs); none before anything of it was heard out, or
    /// with neither a part of the day nor a kind of its own.
    pub fn why_moment(&self, tracks: &[usize], title: &str) -> Option<String> {
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
        let kind = (wordless as f32 >= KIND_SHARE * tracks.len() as f32 && !title.to_lowercase().contains("без слов")).then_some("без слов");
        // Nothing of its own to say: no line (the same on every card says
        // nothing).
        if part.is_none() && kind.is_none() {
            return None;
        }
        let mut line = "Из того, что ты дослушиваешь".to_owned();
        if let Some(p) = part {
            line.push(' ');
            line.push_str(when(p));
        }
        if let Some(k) = kind {
            line.push_str(", — ");
            line.push_str(k);
        }
        Some(line)
    }

    /// Why a mix of the world `world` (its name); `title` names a track
    /// (catalog position).
    pub fn why_world_mix(&self, mix: &WorldMix, world: &str, title: &dyn Fn(usize) -> Option<String>) -> Option<String> {
        let m = self.model?;
        match mix.kind {
            WorldMixKind::Best => {
                // Of a history, not the albums' first tracks standing in.
                let heard = mix.tracks.iter().filter(|&&i| m.tracks[i].completes > 0).count();
                (heard * 2 >= mix.tracks.len() && heard > 0).then(|| format!("Что ты дослушивал чаще всего в {world}"))
            }
            WorldMixKind::Unheard => {
                let near = mix.near.and_then(|n| {
                    let a = m.embedded_at(*mix.tracks.first()?)?;
                    let b = m.embedded_at(n)?;
                    (euclid(a, b) <= m.scale()).then_some(n)
                });
                Some(match near.and_then(title) {
                    Some(t) => format!("Ты не слышал; по звуку ближе всего к {t}"),
                    None => "Ты не слышал ничего из этого".to_owned(),
                })
            }
            WorldMixKind::Road => Some(format!("Звучит как твой {world}, но из других миров")),
            WorldMixKind::Calm | WorldMixKind::Moving => None,
        }
    }

    /// Why a radio's pick `p`, played after `before` (see the module docs);
    /// `title` names a track. None for what the module docs leave out.
    pub fn why_pick(&self, p: &Pick, before: Option<TrackId>, title: &dyn Fn(TrackId) -> Option<String>) -> Option<String> {
        let crate::Item::Track(id) = &p.item else { return None };
        match p.reason {
            Reason::NextOnAlbum => Some("Следующий на альбоме".to_owned()),
            Reason::FollowsOften(t) => title(t).map(|t| format!("После {t}: часто идёт следом")),
            Reason::ListenedTogether(t) => title(t).map(|t| format!("Часто слушают вместе с {t}")),
            Reason::SimilarTo(t) => self.near(*id, Some(t)).then(|| "Похоже по звуку".to_owned()),
            Reason::SoundsLikeSeed => self.near(*id, before).then(|| "Похоже по звуку".to_owned()),
            _ => None,
        }
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
        assert_eq!(recs.why_moment(&at(0..10), "Песни").as_deref(), Some("Из того, что ты дослушиваешь вечером"));
        assert_eq!(recs.why_moment(&at(0..30), "Всё вперемешку"), None, "no part of the day of its own");
        assert_eq!(recs.why_moment(&at(30..40), "Песни"), None, "nothing of it heard out");
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
        let why = recs.why_world_mix(unheard, "Мир", &name).unwrap();
        assert!(why.starts_with("Ты не слышал; по звуку ближе всего к T"), "{why}");
        assert_eq!(recs.why_world_mix(of(WorldMixKind::Best), "Мир", &name).as_deref(), Some("Что ты дослушивал чаще всего в Мир"));
        let pick = |i: u128, reason| Pick { item: crate::Item::Track(tid(i)), reason };
        let title = |_: TrackId| Some("Рассел кроу".to_owned());
        assert_eq!(recs.why_pick(&pick(1, Reason::NextOnAlbum), None, &title).as_deref(), Some("Следующий на альбоме"));
        assert_eq!(recs.why_pick(&pick(1, Reason::FollowsOften(tid(0))), None, &title).as_deref(), Some("После Рассел кроу: часто идёт следом"));
        assert_eq!(recs.why_pick(&pick(1, Reason::SimilarTo(tid(0))), None, &title).as_deref(), Some("Похоже по звуку"));
        assert_eq!(recs.why_pick(&pick(39, Reason::SimilarTo(tid(0))), None, &title), None, "beyond the median distance");
    }

    #[test]
    fn says_the_count_in_russian() {
        assert_eq!(on_repeat(1), None);
        assert_eq!(on_repeat(3).as_deref(), Some("Ты включал это 3 раза за неделю"));
        assert_eq!(on_repeat(12).as_deref(), Some("Ты включал это 12 раз за неделю"));
        assert_eq!(on_repeat(22).as_deref(), Some("Ты включал это 22 раза за неделю"));
        assert_eq!(new_found("Ghost Avenue", "YouTube Music"), "Новое: нашлось по Ghost Avenue в YouTube Music");
    }
}
