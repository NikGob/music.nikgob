//! The mix tiles of the card at the top of Главная: after the
//! world to continue come the mix of the moment (what the user plays at
//! this part of the day, by its sound) and «После <мир>» (the sound of the
//! first world's core, from the other worlds), then the other worlds. The
//! card is out of the app for now ([`nikgob_view::HOME_CARD`]): the mixes are built
//! here and tested, and wait for its new design to be shown.
//!
//! Each is 20 songs, no world over a quarter of them nor thrice running,
//! in the smooth order. The set is a function of the day, the mix's key
//! (the part of the day; the world) and its salt, kept in the library's
//! cache (`recs.top_mixes`): a build later in the day does not change it,
//! a dislike only takes its song out. «Другой» draws a new salt, twice a
//! day.

use std::collections::HashSet;

use nikgob_core::i18n::{Locale, tracks_count_in};
use nikgob_core::model::{EventType, ReleaseId, TrackId};
use nikgob_recs::{DayPart, RadioParams, TilesInput, style_words_in};
use serde::{Deserialize, Serialize};

use super::{BuildInput, Snapshot, cache_read, cache_write, day_seed, daylist_key};

const TOP_KEY: &str = "recs.top_mixes";
/// Songs of a mix (45–60 minutes: a night's session is 26 minutes at the
/// median, three quarters under 53).
const LEN: usize = 20;
/// Fewer than this and there is no tile.
const MIN_LEN: usize = 8;
/// One world takes at most this share of a mix.
const WORLD_SHARE: f32 = 0.25;
/// «Другой» picks another set this many times a day.
const PRESSES: u8 = 2;
/// Worlds named in the line of why, and covers of the picture.
const NAMED: usize = 4;

/// The mix tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopMix {
    /// What the user plays at this part of the day, by its sound.
    Moment,
    /// «После <мир>».
    After,
}

impl TopMix {
    const ALL: [Self; 2] = [Self::Moment, Self::After];
}

/// One mix tile as the build hands it over.
// What the card would show; read again once it is back.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct TopMixHome {
    pub mix: TopMix,
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

/// The day's mixes as the cache keeps them (by [`TopMix`] order).
#[derive(Debug, Default, Serialize, Deserialize)]
struct TopCache {
    day: String,
    salts: [u64; 2],
    presses: [u8; 2],
    sets: [Option<Set>; 2],
}

const fn slot(m: TopMix) -> usize {
    match m {
        TopMix::Moment => 0,
        TopMix::After => 1,
    }
}

/// «по вечерам» / «in the evenings».
const fn when(part: DayPart, loc: Locale) -> &'static str {
    match loc {
        Locale::Ru => match part {
            DayPart::Morning => "по утрам",
            DayPart::Day => "днём",
            DayPart::Evening => "по вечерам",
            DayPart::Night => "по ночам",
        },
        Locale::En => match part {
            DayPart::Morning => "in the mornings",
            DayPart::Day => "during the day",
            DayPart::Evening => "in the evenings",
            DayPart::Night => "at night",
        },
    }
}

/// «Микс на вечер» / «Evening Mix».
const fn moment_kicker(part: DayPart, loc: Locale) -> &'static str {
    match loc {
        Locale::Ru => match part {
            DayPart::Morning => "Микс на утро",
            DayPart::Day => "Микс на день",
            DayPart::Evening => "Микс на вечер",
            DayPart::Night => "Микс на ночь",
        },
        Locale::En => match part {
            DayPart::Morning => "Morning Mix",
            DayPart::Day => "Daytime Mix",
            DayPart::Evening => "Evening Mix",
            DayPart::Night => "Night Mix",
        },
    }
}

/// «A, B, C и D» / «A, B, C & D» (more than `NAMED`: «A, B, C, D и другие» / «A, B, C, D & more»).
fn names(list: &[&str], loc: Locale) -> String {
    match list {
        [] => String::new(),
        [a] => (*a).to_owned(),
        _ if list.len() > NAMED => match loc {
            Locale::Ru => format!("{} и другие", list[..NAMED].join(", ")),
            Locale::En => format!("{} & more", list[..NAMED].join(", ")),
        },
        [init @ .., last] => match loc {
            Locale::Ru => format!("{} и {last}", init.join(", ")),
            Locale::En => format!("{} & {last}", init.join(", ")),
        },
    }
}

/// The name of a sound in two short words: «спокойное и мрачное» of
/// [`style_words_in`] reads «Тихо и темно».
fn sound_name(words: &str, loc: Locale) -> String {
    fn short(w: &str) -> &str {
        match w {
            "энергичное" => "бодро",
            "спокойное" => "тихо",
            "светлое" => "светло",
            "мрачное" => "темно",
            "ровное" => "ровно",
            "переменчивое" => "переменчиво",
            other => other,
        }
    }
    let s = match loc {
        Locale::Ru => words.split(" и ").map(short).collect::<Vec<_>>().join(" и "),
        Locale::En => words.to_owned(),
    };
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
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput, refresh: Option<TopMix>) -> Vec<TopMixHome> {
    if !snap.model.taste.has_history() {
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
            .is_some_and(|t| !snap.model.taste.blocks(t) && (t.local || !snap.local_only))
    };
    let long = |id: &TrackId| tiles.facts.get(id).and_then(|f| f.duration_ms).is_some_and(|d| d > super::lanes::LONG_MS);
    let (part, _, part_key) = daylist_key(input.now, input.utc_offset_min);
    let world1 = last_world(snap, tiles);

    let loc = input.locale;
    let mut out = Vec::new();
    for m in TopMix::ALL {
        let k = slot(m);
        let salt = cache.salts[k];
        let key = match m {
            TopMix::Moment => format!("{part_key}|{}|{salt}|{}", snap.local_only, loc.code()),
            TopMix::After => match world1 {
                Some(w) => format!("{}|{w}|{}|{salt}|{}", input.day, snap.local_only, loc.code()),
                None => continue,
            },
        };
        let kept = cache.sets[k].as_ref().filter(|s| s.key == key).cloned();
        let set = match kept {
            Some(s) => s,
            None => {
                let params = RadioParams { new_share: 0.0, seed: day_seed(&key) };
                let (title, tracks) = match m {
                    TopMix::Moment => {
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
                        let fallback = match loc {
                            Locale::Ru => format!("Как обычно {}", when(part, loc)),
                            Locale::En => format!("Usual {}", when(part, loc)),
                        };
                        let title = style_words_in(centre.axes, loc).map_or(fallback, |w| sound_name(&w, loc));
                        (title, recs.mix_of(&cands, LEN, WORLD_SHARE))
                    }
                    TopMix::After => {
                        let w = world1.expect("keyed by it");
                        let title = snap.worlds.get(&w).map_or_else(String::new, |x| match loc {
                            Locale::Ru => format!("После {}", x.1),
                            Locale::En => format!("After {}", x.1),
                        });
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
        out.push(describe(snap, tiles, m, part, loc, set.title, tracks, world1, cache.presses[k] < PRESSES));
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
    m: TopMix,
    part: DayPart,
    loc: Locale,
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
    let named = names(&titles, loc);
    let (kicker, reason, after) = match m {
        TopMix::Moment => (
            moment_kicker(part, loc).to_owned(),
            match loc {
                Locale::Ru => format!("Из того, что ты слушаешь {}: {named}", when(part, loc)),
                Locale::En => format!("From what you play {}: {named}", when(part, loc)),
            },
            None,
        ),
        TopMix::After => {
            let w = world1.and_then(|w| snap.worlds.get(&w));
            let name = w.map_or("", |x| x.1.as_str());
            (
                match loc {
                    Locale::Ru => "Похоже по звуку",
                    Locale::En => "Similar Sound",
                }
                .to_owned(),
                match loc {
                    Locale::Ru => format!("Звучит как твой {name}, но из других миров: {named}"),
                    Locale::En => format!("Sounds like your {name}, from other worlds: {named}"),
                },
                w.map(|x| x.0.clone()),
            )
        }
    };
    let duration_ms: u64 = tracks.iter().map(|id| u64::from(tiles.facts.get(id).and_then(|f| f.duration_ms).unwrap_or(0))).sum();
    let unheard = tracks.iter().filter(|id| !heard.contains(id)).count();
    let title_of = |id: &TrackId| tiles.facts.get(id).map_or_else(String::new, |f| f.title.clone());
    let first_new = tracks.iter().position(|id| !heard.contains(id)).map(|k| (k, title_of(&tracks[k])));
    let n = tracks.len();
    let w = worlds.len();
    let mut facts = vec![
        match loc {
            Locale::Ru => "Микс",
            Locale::En => "Mix",
        }
        .to_owned(),
        tracks_count_in(loc, n as u64),
        minutes(duration_ms, loc),
        match loc {
            Locale::Ru => format!("{w} {}", crate::streams::plural(w, "мир", "мира", "миров")),
            Locale::En => {
                if w == 1 {
                    "1 world".to_owned()
                } else {
                    format!("{w} worlds")
                }
            }
        },
    ];
    if unheard > 0 {
        facts.push(match loc {
            Locale::Ru => format!("{unheard} ты ещё не слышал"),
            Locale::En => format!("{unheard} unheard"),
        });
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
fn minutes(ms: u64, loc: Locale) -> String {
    let m = (ms + 30_000) / 60_000;
    match loc {
        Locale::Ru => {
            if m < 60 { format!("{m} мин") } else { format!("{} ч {:02} мин", m / 60, m % 60) }
        }
        Locale::En => {
            if m < 60 { format!("{m} min") } else { format!("{} hr {:02} min", m / 60, m % 60) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_as_a_list() {
        assert_eq!(names(&["A"], Locale::Ru), "A");
        assert_eq!(names(&["A", "B"], Locale::Ru), "A и B");
        assert_eq!(names(&["A", "B", "C", "D"], Locale::Ru), "A, B, C и D");
        assert_eq!(names(&["A", "B", "C", "D", "E"], Locale::Ru), "A, B, C, D и другие");
        assert_eq!(names(&["A", "B"], Locale::En), "A & B");
        assert_eq!(minutes(52 * 60_000, Locale::Ru), "52 мин");
        assert_eq!(minutes(65 * 60_000, Locale::Ru), "1 ч 05 мин");
        assert_eq!(sound_name("спокойное и мрачное", Locale::Ru), "Тихо и темно");
        assert_eq!(sound_name("энергичное", Locale::Ru), "Бодро");
    }

    /// Three soundtracks of 10 songs, MADE IN ABYSS played last: the mix
    /// of the moment and «После MADE IN ABYSS» (none of it), each kept for
    /// the day, «Другой» twice and no more.
    #[test]
    fn the_moment_and_after_the_world_are_mixes_of_the_day() {
        use nikgob_core::model::{Artist, ArtistCredit, Codec, Event, Release, Source, SourceKind, Track};
        use nikgob_library::{Library, now_ms};
        let lib = Library::open_in_memory().unwrap();
        let index: crate::analysis::SoundIndex = Default::default();
        let now = now_ms();
        let mut rng = fastrand::Rng::with_seed(5);
        let mut mia = HashSet::new();
        let mut last = None;
        for (w, (franchise, composer)) in [("MADE IN ABYSS", "Kevin Penkin"), ("OMORI", "pedro silva"), ("Hollow Knight", "Christopher Larkin")].iter().enumerate() {
            let a = lib.upsert_artist(&Artist::new(*composer)).unwrap();
            let credit = ArtistCredit::single(*composer, Some(a.id));
            for (k, title) in [format!("{franchise} ORIGINAL SOUNDTRACK"), format!("{franchise} ORIGINAL SOUNDTRACK 2")].iter().enumerate() {
                let r = lib.upsert_release(&Release::new(title.clone(), credit.clone())).unwrap();
                for n in 0..5 {
                    let mut t = Track::new(format!("{title} {n}"), credit.clone());
                    t.release_id = Some(r.id);
                    t.track_no = n + 1;
                    t.duration_ms = Some(180_000);
                    let t = lib.upsert_track(&t).unwrap();
                    lib.add_source(&Source::new(t.id, SourceKind::LocalFile, format!("x/{w}/{k}/{n}.flac"), Codec::Flac)).unwrap();
                    let mut v = [0.0f32; nikgob_recs::DIM];
                    for x in &mut v {
                        *x = rng.f32() + w as f32 * 0.3;
                    }
                    index.write().insert(t.id, v);
                    // Every world heard a little, days ago; MADE IN ABYSS last.
                    let ts = now - 86_400_000 * (4 - w as i64) + i64::from(n) * 1000 + k as i64 * 10_000;
                    if n < 3 {
                        lib.record_event_raw(&Event::new(t.id, EventType::Start, ts, lib.device_id())).unwrap();
                        lib.record_event_raw(&Event::new(t.id, EventType::Complete, ts + 500, lib.device_id())).unwrap();
                    }
                    if w == 0 {
                        mia.insert(t.id);
                        last = Some(t.id);
                    }
                }
            }
        }
        lib.record_event_raw(&Event::new(last.unwrap(), EventType::Start, now - 60_000, lib.device_id())).unwrap();
        let (snap, tiles, _) = super::super::read_snapshot(&lib, &index, now, 0, &[], false).unwrap();
        let input = BuildInput {
            lib: lib.clone(),
            index: index.clone(),
            day: "2026-10-25".into(),
            locale: Locale::Ru,
            epoch: crate::home_v2::Epoch::default(),
            utc_offset_min: 0,
            now,
            refresh_daily: false,
            refresh_daylist: false,
            new_share: 0.0,
            local_only: false,
            new: Vec::new(),
            new_at: 0,
            new_pool: Vec::new(),
            new_sources: Default::default(),
            suggested: Vec::new(),
            rerank: false,
            refresh_mix: None,
            row_more: 0,
            blends: Vec::new(),
            session: nikgob_recs::Session::default(),
            mix_prefs: super::super::lanes::MixPrefs::of(&nikgob_view::Prefs::default()),
            extra: Default::default(),
            new_log: Default::default(),
        };
        let mixes = build(&snap, &input, &tiles, None);
        let moment = mixes.iter().find(|m| m.mix == TopMix::Moment).expect("the moment");
        assert!(moment.tracks.len() >= MIN_LEN, "{}", moment.tracks.len());
        assert!(moment.facts.starts_with("Микс · "), "{}", moment.facts);
        assert!(moment.reason.starts_with("Из того, что ты слушаешь"), "{}", moment.reason);
        let after = mixes.iter().find(|m| m.mix == TopMix::After).expect("«После»");
        assert_eq!(after.title, "После MADE IN ABYSS");
        assert_eq!(after.after.as_deref(), Some("made in abyss"));
        assert!(after.tracks.iter().all(|t| !mia.contains(t)), "none of the world itself");
        assert!(after.tracks.len() >= MIN_LEN);
        // Kept for the day.
        let again = build(&snap, &input, &tiles, None);
        assert_eq!(again.iter().map(|m| m.tracks.clone()).collect::<Vec<_>>(), mixes.iter().map(|m| m.tracks.clone()).collect::<Vec<_>>());
        // «Другой»: twice, then no more today.
        let one = build(&snap, &input, &tiles, Some(TopMix::Moment));
        assert!(one[0].other);
        let two = build(&snap, &input, &tiles, Some(TopMix::Moment));
        assert!(!two[0].other);
        let three = build(&snap, &input, &tiles, Some(TopMix::Moment));
        assert_eq!(three[0].tracks, two[0].tracks, "the third press changes nothing");
    }
}
