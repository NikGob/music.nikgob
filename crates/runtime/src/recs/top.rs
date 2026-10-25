//! The mix tiles of the card at the top of Главная (recs-v2 §7): after the
//! world to continue come the mix of the moment (what the user plays at
//! this part of the day, by its sound) and «После <мир>» (the sound of the
//! first world's core, from the other worlds), then the other worlds.
//!
//! Each is 20 songs, no world over a quarter of them nor thrice running,
//! in the smooth order. The set is a function of the day, the mix's key
//! (the part of the day; the world) and its salt, kept in the library's
//! cache (`recs.top_mixes`): a build later in the day does not change it,
//! a dislike only takes its song out. «Другой» draws a new salt, twice a
//! day.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{EventType, ReleaseId, TrackId};
use nikgob_recs::{DayPart, RadioParams, TilesInput, style_words};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{BuildInput, Snapshot, cache_read, cache_write, day_seed, daylist_key};

pub(crate) const TOP_KEY: &str = "recs.top_mixes";
/// Songs of a mix (45–60 minutes: a night's session is 26 minutes at the
/// median, three quarters under 53).
const LEN: usize = 20;
/// Fewer than this and there is no tile.
const MIN_LEN: usize = 8;
/// One world takes at most this share of a mix.
const WORLD_SHARE: f32 = 0.25;
/// «Другой» picks another set this many times a day.
pub(crate) const PRESSES: u8 = 2;
/// Worlds named in the line of why, and covers of the picture.
const NAMED: usize = 4;

/// One mix tile as the build hands it over.
#[derive(Clone, Debug)]
pub(crate) struct TopMixHome {
    pub mix: v::TopMix,
    /// «Микс на вечер», «Похоже по звуку».
    pub kicker: String,
    /// «Спокойное и мрачное», «После MADE IN ABYSS».
    pub title: String,
    pub reason: String,
    /// «Микс · 20 треков · 52 мин · 6 миров · 3 ты ещё не слышал».
    pub facts: String,
    pub tracks: Vec<TrackId>,
    /// The first track of each of its first worlds: the picture.
    pub covers: Vec<ReleaseId>,
    /// Its worlds in the order they come, by key.
    pub worlds: Vec<String>,
    /// «После»: the key of the world it comes after.
    pub after: Option<String>,
    /// Of its tracks, the ones never started.
    pub unheard: usize,
    /// The titles of the first track and of the first one never started.
    pub first: String,
    pub first_new: Option<(usize, String)>,
    pub duration_ms: u64,
    /// «Другой» may pick again today.
    pub other: bool,
}

/// What a mix was picked for and its songs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Set {
    key: String,
    title: String,
    tracks: Vec<TrackId>,
}

/// The day's mixes as the cache keeps them (by [`v::TopMix`] order).
#[derive(Debug, Default, Serialize, Deserialize)]
struct TopCache {
    day: String,
    salts: [u64; 2],
    presses: [u8; 2],
    sets: [Option<Set>; 2],
}

const fn slot(m: v::TopMix) -> usize {
    match m {
        v::TopMix::Moment => 0,
        v::TopMix::After => 1,
    }
}

/// «по вечерам».
const fn when(part: DayPart) -> &'static str {
    match part {
        DayPart::Morning => "по утрам",
        DayPart::Day => "днём",
        DayPart::Evening => "по вечерам",
        DayPart::Night => "по ночам",
    }
}

/// «Микс на вечер».
const fn moment_kicker(part: DayPart) -> &'static str {
    match part {
        DayPart::Morning => "Микс на утро",
        DayPart::Day => "Микс на день",
        DayPart::Evening => "Микс на вечер",
        DayPart::Night => "Микс на ночь",
    }
}

/// «A, B, C и D» (more than `NAMED`: «A, B, C, D и другие»).
fn names(list: &[&str]) -> String {
    match list {
        [] => String::new(),
        [a] => (*a).to_owned(),
        _ if list.len() > NAMED => format!("{} и другие", list[..NAMED].join(", ")),
        [init @ .., last] => format!("{} и {last}", init.join(", ")),
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// The world of the last track started, by its token.
fn last_world(snap: &Snapshot, input: &TilesInput) -> Option<u64> {
    let last = input.events.iter().filter(|e| e.kind == EventType::Start).max_by_key(|e| e.ts)?;
    snap.catalog.get(last.track)?.world
}

/// The world a track `id` belongs to, by its token, if the catalog has it.
fn world_of(snap: &Snapshot, id: TrackId) -> Option<u64> {
    snap.catalog.get(id).and_then(|t| t.world)
}

/// The mix tiles of now, `refresh` one of them picked anew («Другой»).
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput, refresh: Option<v::TopMix>) -> Vec<TopMixHome> {
    if !snap.taste.has_history() {
        return Vec::new();
    }
    let lib = &input.lib;
    let mut cache = cache_read::<TopCache>(lib, TOP_KEY).filter(|c| c.day == input.day).unwrap_or_else(|| TopCache {
        day: input.day.clone(),
        ..TopCache::default()
    });
    let mut changed = false;
    if let Some(m) = refresh {
        let k = slot(m);
        if cache.presses[k] < PRESSES {
            cache.presses[k] += 1;
            cache.salts[k] = fastrand::u64(1..);
            changed = true;
        }
    }
    let recs = snap.recs();
    let fits = |id: &TrackId| {
        snap.catalog
            .get(*id)
            .is_some_and(|t| !snap.taste.blocks(t) && (t.local || !snap.local_only))
    };
    let long = |id: &TrackId| tiles.facts.get(id).and_then(|f| f.duration_ms).is_some_and(|d| d > super::lanes::LONG_MS);
    let (part, _, part_key) = daylist_key(input.now, input.utc_offset_min);
    let world1 = last_world(snap, tiles);

    let mut out = Vec::new();
    for m in v::TopMix::ALL {
        let k = slot(m);
        let salt = cache.salts[k];
        let key = match m {
            v::TopMix::Moment => format!("{part_key}|{}|{salt}", snap.local_only),
            v::TopMix::After => match world1 {
                Some(w) => format!("{}|{w}|{}|{salt}", input.day, snap.local_only),
                None => continue,
            },
        };
        let kept = cache.sets[k].as_ref().filter(|s| s.key == key).cloned();
        let set = match kept {
            Some(s) => s,
            None => {
                let params = RadioParams { new_share: 0.0, seed: day_seed(&key) };
                let (title, tracks) = match m {
                    v::TopMix::Moment => {
                        let (centre, _, picks) = recs.daylist(part, &[], 2 * LEN, &params);
                        let len = picks.len().max(1) as f32;
                        let cands: Vec<(TrackId, f32)> = picks
                            .iter()
                            .enumerate()
                            .filter_map(|(k, p)| match p.item {
                                nikgob_recs::Item::Track(id) => Some((id, 1.0 - k as f32 / len)),
                                nikgob_recs::Item::Stream(_) => None,
                            })
                            .filter(|(id, _)| !long(id))
                            .collect();
                        let title = style_words(centre.axes).map_or_else(|| format!("Как обычно {}", when(part)), |w| capitalized(&w));
                        (title, recs.mix_of(&cands, LEN, WORLD_SHARE))
                    }
                    v::TopMix::After => {
                        let w = world1.expect("keyed by it");
                        let title = snap.worlds.get(&w).map_or_else(String::new, |x| format!("После {}", x.1));
                        let tracks: Vec<TrackId> = recs.after_world(w, LEN + 4, WORLD_SHARE, &params).into_iter().filter(|id| !long(id)).take(LEN).collect();
                        (title, tracks)
                    }
                };
                changed = true;
                let s = Set { key, title, tracks };
                cache.sets[k] = Some(s.clone());
                s
            }
        };
        let tracks: Vec<TrackId> = set.tracks.into_iter().filter(|id| fits(id)).collect();
        if tracks.len() < MIN_LEN || set.title.is_empty() {
            continue;
        }
        out.push(describe(snap, tiles, m, part, set.title, tracks, world1, cache.presses[k] < PRESSES));
    }
    if changed {
        cache_write(lib, TOP_KEY, input.now, &cache);
    }
    out
}

/// A mix's tile from its songs: the line of why, the facts, the picture.
#[allow(clippy::too_many_arguments)]
fn describe(
    snap: &Snapshot,
    tiles: &TilesInput,
    m: v::TopMix,
    part: DayPart,
    title: String,
    tracks: Vec<TrackId>,
    world1: Option<u64>,
    other: bool,
) -> TopMixHome {
    let heard: HashSet<TrackId> = tiles.events.iter().filter(|e| e.kind == EventType::Start).map(|e| e.track).collect();
    // Its worlds by how many songs each has, the first to come first among equals.
    let mut worlds: Vec<(u64, usize, usize)> = Vec::new();
    let mut covers: Vec<ReleaseId> = Vec::new();
    let mut seen: HashSet<u64> = HashSet::new();
    for (k, id) in tracks.iter().enumerate() {
        let Some(w) = world_of(snap, *id) else { continue };
        match worlds.iter_mut().find(|x| x.0 == w) {
            Some(x) => x.1 += 1,
            None => worlds.push((w, 1, k)),
        }
        if seen.insert(w)
            && covers.len() < NAMED
            && let Some(r) = snap.catalog.get(*id).and_then(|t| t.release)
        {
            covers.push(r);
        }
    }
    let in_order: Vec<String> = worlds.iter().filter_map(|x| snap.worlds.get(&x.0)).map(|x| x.0.clone()).collect();
    worlds.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    let titles: Vec<&str> = worlds.iter().filter_map(|x| snap.worlds.get(&x.0)).map(|x| x.1.as_str()).filter(|t| !t.is_empty()).collect();
    let named = names(&titles);
    let (kicker, reason, after) = match m {
        v::TopMix::Moment => (moment_kicker(part).to_owned(), format!("Из того, что ты слушаешь {}: {named}", when(part)), None),
        v::TopMix::After => {
            let w = world1.and_then(|w| snap.worlds.get(&w));
            let name = w.map_or("", |x| x.1.as_str());
            (
                "Похоже по звуку".to_owned(),
                format!("Звучит как твой {name}, но из других миров: {named}"),
                w.map(|x| x.0.clone()),
            )
        }
    };
    let duration_ms: u64 = tracks.iter().map(|id| u64::from(tiles.facts.get(id).and_then(|f| f.duration_ms).unwrap_or(0))).sum();
    let unheard = tracks.iter().filter(|id| !heard.contains(id)).count();
    let title_of = |id: &TrackId| tiles.facts.get(id).map_or_else(String::new, |f| f.title.clone());
    let first_new = tracks.iter().position(|id| !heard.contains(id)).map(|k| (k, title_of(&tracks[k])));
    let plural = crate::streams::plural;
    let n = tracks.len();
    let w = worlds.len();
    let mut facts = vec![
        v::WorldKind::Mix.label().to_owned(),
        format!("{n} {}", plural(n, "трек", "трека", "треков")),
        minutes(duration_ms),
        format!("{w} {}", plural(w, "мир", "мира", "миров")),
    ];
    if unheard > 0 {
        facts.push(format!("{unheard} ты ещё не слышал"));
    }
    TopMixHome {
        mix: m,
        kicker,
        title,
        reason,
        facts: facts.join(" · "),
        first: title_of(&tracks[0]),
        first_new,
        tracks,
        covers,
        worlds: in_order,
        after,
        unheard,
        duration_ms,
        other,
    }
}

/// «52 мин», «1 ч 05 мин».
fn minutes(ms: u64) -> String {
    let m = (ms + 30_000) / 60_000;
    if m < 60 { format!("{m} мин") } else { format!("{} ч {:02} мин", m / 60, m % 60) }
}

/// The tile of `t` on the row, its picture resolved by the caller: what
/// it plays and the three cards («Из миров» opens the first of its worlds
/// that is on the row, `on_row`).
pub(crate) fn world(t: &TopMixHome, covers: Vec<v::Cover>, release: Option<ReleaseId>, on_row: &[(String, String)]) -> v::World {
    let mut cards = vec![v::WorldCard {
        icon: v::WorldIcon::Play,
        kicker: "Первым будет".into(),
        title: t.first.clone(),
        go: v::WorldGo::Play(t.tracks.clone()),
    }];
    if let Some((k, title)) = &t.first_new
        && *k > 0
    {
        cards.push(v::WorldCard {
            icon: v::WorldIcon::Next,
            kicker: "Ещё не слышал".into(),
            title: title.clone(),
            go: v::WorldGo::Play(t.tracks[*k..].to_vec()),
        });
    }
    let row: HashMap<&str, &str> = on_row.iter().map(|(k, title)| (k.as_str(), title.as_str())).collect();
    let present: Vec<(&str, &str)> = t.worlds.iter().filter_map(|k| row.get_key_value(k.as_str()).map(|(k, v)| (*k, *v))).collect();
    if let Some((key, _)) = present.first() {
        let more = t.worlds.len().saturating_sub(2);
        let first_two: Vec<&str> = present.iter().take(2).map(|x| x.1).collect();
        let title = if more > 0 { format!("{} +{more}", first_two.join(", ")) } else { first_two.join(", ") };
        cards.push(v::WorldCard { icon: v::WorldIcon::Album, kicker: "Из миров".into(), title, go: v::WorldGo::World((*key).to_owned()) });
    }
    let mut shuffled = t.tracks.clone();
    fastrand::shuffle(&mut shuffled);
    let n = t.tracks.len();
    v::World {
        key: t.mix.key().to_owned(),
        title: t.title.clone(),
        kind: v::WorldKind::Mix,
        release,
        cover: covers.first().cloned(),
        cover_hd: None,
        kicker: t.kicker.clone(),
        facts: t.facts.clone(),
        heard: (n - t.unheard) as u32,
        total: n as u32,
        start: v::WorldStart::Begin,
        playing: false,
        primary: t.tracks.clone(),
        primary_tip: format!(
            "{} {}, {}: по порядку микса",
            n,
            crate::streams::plural(n, "трек", "трека", "треков"),
            minutes(t.duration_ms)
        ),
        shuffled,
        albums: Vec::new(),
        cards,
        mix: Some(v::MixId::Top(t.mix)),
        covers,
        reason: t.reason.clone(),
        mix_other: t.other,
    }
}

/// Puts the mix tiles on the row after its first world (the one to
/// continue): «После» only after the world it is about; a tile taken off
/// the row stays off.
pub(crate) fn place(worlds: &mut Vec<v::World>, mixes: Vec<v::World>, after: &[Option<String>], hidden: &[String]) {
    worlds.retain(|w| w.kind != v::WorldKind::Mix);
    let Some(first) = worlds.first().map(|w| w.key.clone()) else { return };
    let mut at = 1;
    for (w, a) in mixes.into_iter().zip(after) {
        if hidden.contains(&w.key) || a.as_ref().is_some_and(|a| *a != first) {
            continue;
        }
        worlds.insert(at, w);
        at += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_as_a_list() {
        assert_eq!(names(&["A"]), "A");
        assert_eq!(names(&["A", "B"]), "A и B");
        assert_eq!(names(&["A", "B", "C", "D"]), "A, B, C и D");
        assert_eq!(names(&["A", "B", "C", "D", "E"]), "A, B, C, D и другие");
        assert_eq!(minutes(52 * 60_000), "52 мин");
        assert_eq!(minutes(65 * 60_000), "1 ч 05 мин");
    }
}
