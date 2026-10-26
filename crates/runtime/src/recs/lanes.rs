//! «Миксы для тебя» (recs-v2 §6, profiles-v1 §6): the five moments of
//! [`nikgob_recs::mixes`] — the profiles' own mixes, «Всё вперемешку», the
//! calm and the lively side of everything loved, «Фон» for whoever
//! listens so, the blends the user made today first — and «На повторе».
//! What a day picked stays the day's (`recs.mixes` in the library's cache:
//! the lineup, each mix's songs, yesterday's, the salts of «Другой
//! состав» and the blends), from 06:00 (`local_day`) to 06:00; only the
//! order of the cards and of the songs follows the part of the day.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EventType, TrackId};
use nikgob_core::streaming::StreamTrack;
use nikgob_recs::mixes::{Builder, Kept, MIN_SONGS, Mix, MixKey, Settings};
use nikgob_recs::profiles::{LaneKey, NameStyle};
use nikgob_recs::tiles::franchise;
use nikgob_recs::{Lang, Live, Session, TilesInput};
use nikgob_view as v;
use serde::{Deserialize, Serialize};

use super::{BuildInput, CoverRef, Snapshot, cache_read, cache_write};

const MIXES_KEY: &str = "recs.mixes";
/// A song shorter than this is an interlude: no mix picks it.
const INTERLUDE_MS: u32 = 75_000;
/// About two hours of «На повторе».
const MIX_MS: u64 = 2 * 60 * 60 * 1000;
/// A song of unknown length counts so.
const GUESS_MS: u32 = 210_000;
/// A song longer than this is left out (they are skipped early a third of
/// the time, recs-v2 §2).
pub(super) const LONG_MS: u32 = nikgob_recs::mixes::LONG_MS;
/// «На повторе» looks this far back (30 once the history is longer).
const REPEAT_DAYS_SHORT: i64 = 14;
const REPEAT_DAYS_LONG: i64 = 30;
/// About every fifth song of a mix is new «С новым» before anything was
/// heard; then the share its taste profile's appetite draws
/// ([`nikgob_recs::NewLog::share`]).
const NEW_EVERY: usize = 5;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const HOUR_MS: i64 = 60 * 60 * 1000;

/// One mix of «Миксы для тебя».
pub(crate) struct LaneHome {
    pub key: v::MixKey,
    pub title: String,
    pub tracks: Vec<TrackId>,
    /// Familiar and loved: a new song of «С новым» stands between two.
    pub anchors: Vec<bool>,
    /// Its artists, most played first.
    pub artists: Vec<String>,
    pub covers: Vec<CoverRef>,
    /// «С новым»: new songs of the services that fit it.
    pub new: Vec<StreamTrack>,
    /// The profile «Смешать с…» starts from.
    pub blend_base: Option<u32>,
}

impl LaneHome {
    /// What it plays: its songs, with «С новым» its new ones each between
    /// two loved familiar songs (recs-v2 §5.4), never in the first two
    /// places nor two running, spread as evenly as such places allow.
    pub fn picks(&self, with_new: bool) -> Vec<nikgob_recs::Pick> {
        use nikgob_recs::{Item, Pick, Reason};
        let n = if with_new { self.new.len() } else { 0 };
        let len = self.tracks.len();
        // The gaps after place k (k ≥ 1): between two anchors.
        let gaps: Vec<usize> = (1..len.saturating_sub(1)).filter(|&k| self.anchors.get(k) == Some(&true) && self.anchors.get(k + 1) == Some(&true)).collect();
        let mut after: HashSet<usize> = HashSet::new();
        let mut placed = 0;
        for j in 0..n.min(gaps.len()) {
            // The gap nearest the even spot of the j-th new song, free.
            let spot = (j + 1) * len / (n + 1);
            if let Some(&g) = gaps.iter().filter(|g| !after.contains(g) && !after.contains(&(*g - 1)) && !after.contains(&(*g + 1))).min_by_key(|g| g.abs_diff(spot)) {
                after.insert(g);
                placed += 1;
            }
        }
        let mut out = Vec::with_capacity(len + placed);
        let mut new = self.new.iter();
        for (k, t) in self.tracks.iter().enumerate() {
            out.push(Pick { item: Item::Track(*t), reason: Reason::Favourite });
            if after.contains(&k)
                && let Some(s) = new.next()
            {
                out.push(Pick { item: Item::Stream(s.id.clone()), reason: Reason::New });
            }
        }
        out
    }
}

/// What the cache keeps: the day it is for, the settings it was chosen
/// under and the day's mixes.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MixesCache {
    day: String,
    settings: u64,
    kept: Kept,
}

/// The engine's key of a view's key («На повторе» is the runtime's).
pub(crate) fn engine_key(k: v::MixKey) -> Option<MixKey> {
    Some(match k {
        v::MixKey::Profile(p) => MixKey::Profile(p),
        v::MixKey::Everything => MixKey::Everything,
        v::MixKey::Calm => MixKey::Calm,
        v::MixKey::Lively => MixKey::Lively,
        v::MixKey::Background { profile, card, cards } => MixKey::Background { profile, card, cards },
        v::MixKey::Blend(p) => MixKey::Blend(p),
        v::MixKey::Kind(c) => MixKey::Kind(c),
        v::MixKey::OnRepeat => return None,
    })
}

fn view_key(k: MixKey) -> v::MixKey {
    match k {
        MixKey::Profile(p) => v::MixKey::Profile(p),
        MixKey::Everything => v::MixKey::Everything,
        MixKey::Calm => v::MixKey::Calm,
        MixKey::Lively => v::MixKey::Lively,
        MixKey::Background { profile, card, cards } => v::MixKey::Background { profile, card, cards },
        MixKey::Blend(p) => v::MixKey::Blend(p),
        MixKey::Kind(c) => v::MixKey::Kind(c),
    }
}

/// The moments the prefs ask for.
pub(crate) fn settings(prefs: &MixPrefs) -> Settings {
    Settings {
        profiles: prefs.profiles,
        everything: prefs.everything,
        moods: prefs.moods,
        background_cards: usize::from(prefs.background_cards),
        names: match prefs.names {
            v::ProfileNames::Kind => NameStyle::Kind,
            v::ProfileNames::Artists => NameStyle::Artists,
        },
    }
}

/// The prefs of «Миксы для тебя» a build reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MixPrefs {
    pub profiles: bool,
    pub everything: bool,
    pub moods: bool,
    pub on_repeat: bool,
    pub background_cards: u8,
    pub names: v::ProfileNames,
}

impl MixPrefs {
    pub fn of(p: &v::Prefs) -> Self {
        Self {
            profiles: p.mixes_profiles,
            everything: p.mixes_everything,
            moods: p.mixes_moods,
            on_repeat: p.mixes_on_repeat,
            background_cards: p.background_cards,
            names: p.profile_names,
        }
    }

    /// What tells a lineup chosen under other settings.
    fn fingerprint(&self) -> u64 {
        u64::from(self.profiles)
            | u64::from(self.everything) << 1
            | u64::from(self.moods) << 2
            | u64::from(self.background_cards) << 3
            | u64::from(self.names == v::ProfileNames::Artists) << 6
    }
}

/// The local day number the mixes are drawn by: days start at 06:00.
pub(crate) fn day_number(now: nikgob_core::model::UnixMs, utc_offset_min: i32) -> i64 {
    (now + i64::from(utc_offset_min) * 60_000 - super::DAY_STARTS_HOUR * HOUR_MS).div_euclid(DAY_MS)
}

/// The engine over `snap` for `session` at `now`.
#[allow(clippy::too_many_arguments)]
fn with_builder<T>(snap: &Snapshot, prefs: &MixPrefs, fit: &[bool], ok: &dyn Fn(usize) -> bool, session: &Session, now: nikgob_core::model::UnixMs, utc_offset_min: i32, f: impl FnOnce(&Builder<'_>) -> T) -> T {
    let recs = snap.recs();
    let live: Live = snap.model.live(session);
    let suits = |i: usize| fit.get(i).copied().unwrap_or(false);
    let (part, _, _) = super::daylist_key(now, utc_offset_min);
    let b = Builder::new(&recs, &snap.model, ok, &suits, day_number(now, utc_offset_min), part, live, settings(prefs));
    f(&b)
}

/// Today's «Миксы для тебя» and the profiles «Смешать с…» offers.
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput) -> (Vec<LaneHome>, Vec<v::BlendProfile>) {
    let lib = &input.lib;
    let prefs = &input.mix_prefs;
    let mut cache: MixesCache = cache_read(lib, MIXES_KEY).unwrap_or_default();
    if cache.day != input.day {
        cache = MixesCache { day: input.day.clone(), settings: prefs.fingerprint(), kept: std::mem::take(&mut cache.kept).next_day() };
    }
    if cache.settings != prefs.fingerprint() {
        cache.settings = prefs.fingerprint();
        cache.kept.lineup.clear();
    }
    if let Some(key) = input.refresh_mix.and_then(engine_key) {
        let salt = fastrand::u64(1..);
        match cache.kept.salts.iter_mut().find(|s| s.0 == key) {
            Some(s) => s.1 = salt,
            None => cache.kept.salts.push((key, salt)),
        }
    }
    for b in input.blends.iter().filter_map(|b| engine_key(*b)) {
        if !cache.kept.blends.contains(&b) {
            cache.kept.blends.push(b);
        }
    }
    // A mix takes what the tiles know (their length), what is not junk.
    let ok = |i: usize| {
        let t = &snap.catalog.tracks()[i];
        tiles.facts.contains_key(&t.id) && (t.local || !snap.local_only)
    };
    let names = |a: ArtistId| tiles.artist_names.get(&a).cloned();
    let (mut mixes, lineup, blendable) = with_builder(snap, prefs, &snap.background, &ok, &input.session, input.now, input.utc_offset_min, |b| {
        let (mut mixes, lineup) = b.lineup(&cache.kept);
        b.named(&mut mixes, &names);
        (mixes, lineup, blend_profiles(snap, b, &names))
    });
    cache.kept.lineup = lineup;
    cache.kept.keep(&mixes, &snap.model);
    cache_write(lib, MIXES_KEY, input.now, &cache);
    log_lineup(snap, tiles, &mixes);

    let seed = super::day_seed(&input.day);
    let mut used_new: HashSet<String> = HashSet::new();
    let mut out: Vec<LaneHome> = mixes
        .drain(..)
        .enumerate()
        .map(|(k, m)| {
            let mut lane = lane_of(snap, tiles, &m);
            if !snap.local_only && !matches!(m.key, MixKey::Background { .. }) {
                lane.new = new_for(snap, input, &m, seed.wrapping_add(k as u64), &mut used_new);
            }
            lane
        })
        .collect();
    if prefs.on_repeat
        && snap.model.taste.has_history()
        && let Some(tracks) = on_repeat(snap, tiles)
    {
        let anchors = vec![true; tracks.len()];
        out.push(LaneHome {
            key: v::MixKey::OnRepeat,
            title: "На повторе".into(),
            artists: artists_of(snap, tiles, &tracks),
            covers: covers(snap, &tracks),
            anchors,
            tracks,
            new: Vec::new(),
            blend_base: None,
        });
    }
    (out, blendable)
}

/// A blend of `profiles` (the card's first), now, from the snapshot: it
/// plays at once and stands first until the day's end.
pub(crate) fn blend_now(snap: &Snapshot, prefs: &MixPrefs, profiles: &[u32], names: &dyn Fn(ArtistId) -> Option<String>, session: &Session) -> Option<LaneHome> {
    let key = MixKey::blend(profiles)?;
    let (now, utc_offset_min) = (super::recs_now(), super::local_day().1);
    let ok = |i: usize| snap.catalog.tracks()[i].local || !snap.local_only;
    let mix = with_builder(snap, prefs, &[], &ok, session, now, utc_offset_min, |b| {
        let mut m = vec![b.mix(key, &Kept::default())?];
        b.named(&mut m, names);
        m.pop()
    })?;
    let artists: Vec<String> = mix.artists.iter().filter_map(|a| names(*a)).collect();
    let tracks: Vec<TrackId> = mix.tracks.iter().map(|&i| snap.model.id(i)).collect();
    Some(LaneHome {
        key: view_key(mix.key),
        title: mix.title,
        covers: covers(snap, &tracks),
        anchors: mix.anchors,
        tracks,
        artists: dedupe(artists),
        new: Vec::new(),
        blend_base: None,
    })
}

/// Keeps the blend of `key` among the day's mixes at once (a restart
/// before the next build keeps it too).
pub(crate) fn remember_blend(lib: &nikgob_library::Library, prefs: &MixPrefs, day: &str, now: nikgob_core::model::UnixMs, key: v::MixKey) {
    let Some(key) = engine_key(key) else { return };
    let mut cache: MixesCache = cache_read(lib, MIXES_KEY).unwrap_or_default();
    if cache.day != day {
        cache = MixesCache { day: day.to_owned(), settings: prefs.fingerprint(), kept: std::mem::take(&mut cache.kept).next_day() };
    }
    cache.kept.blends.retain(|b| *b != key);
    cache.kept.blends.push(key);
    cache_write(lib, MIXES_KEY, now, &cache);
}

/// The visible profiles with songs enough, by their names (the largest
/// share first).
fn blend_profiles(snap: &Snapshot, b: &Builder<'_>, names: &dyn Fn(ArtistId) -> Option<String>) -> Vec<v::BlendProfile> {
    let model = &snap.model;
    let mut order: Vec<usize> = (0..model.profiles.len()).filter(|&j| model.profiles.list[j].visible).collect();
    order.sort_by(|a, b| model.profile_share[*b].total_cmp(&model.profile_share[*a]).then(a.cmp(b)));
    let mut out = Vec::new();
    for j in order {
        let key = model.profiles.list[j].key;
        let Some(mut m) = b.mix(MixKey::Profile(key), &Kept::default()) else { continue };
        if m.tracks.len() < MIN_SONGS {
            continue;
        }
        b.named(std::slice::from_mut(&mut m), names);
        out.push(v::BlendProfile { key, title: m.title });
    }
    out
}

fn dedupe(names: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    names.into_iter().filter(|n| seen.insert(n.to_lowercase())).take(12).collect()
}

/// The home's card of a mix.
fn lane_of(snap: &Snapshot, tiles: &TilesInput, m: &Mix) -> LaneHome {
    let tracks: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
    let artists = dedupe(m.artists.iter().filter_map(|a| tiles.artist_names.get(a).cloned()).collect());
    LaneHome {
        key: view_key(m.key),
        title: m.title.clone(),
        covers: covers(snap, &tracks),
        anchors: m.anchors.clone(),
        tracks,
        artists,
        new: Vec::new(),
        blend_base: m.key.blend_base(),
    }
}

/// The lineup in the log: each card, its first songs.
fn log_lineup(snap: &Snapshot, tiles: &TilesInput, mixes: &[Mix]) {
    let line: Vec<String> = mixes.iter().map(|m| format!("«{}» {}", m.title, m.tracks.len())).collect();
    log::info!("recs: mixes for you: {}", line.join(", "));
    for m in mixes {
        let first: Vec<String> = m
            .tracks
            .iter()
            .take(5)
            .filter_map(|&i| tiles.facts.get(&snap.model.id(i)).map(|f| format!("{} — {}", f.artist, f.title)))
            .collect();
        log::debug!("recs: mix «{}»: {}", m.title, first.join(" · "));
    }
}

/// The taste profile most of `tracks` are of.
fn profile_of(snap: &Snapshot, tracks: &[TrackId]) -> Option<usize> {
    let mut n: HashMap<usize, usize> = HashMap::new();
    for t in tracks {
        if let Some(&j) = snap.profiles.of.get(t) {
            *n.entry(j).or_default() += 1;
        }
    }
    n.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|x| x.0)
}

/// How many new songs a mix of `songs` gets at `share` (one at least).
fn new_count(songs: usize, share: f32) -> usize {
    ((songs as f32 * share / (1.0 - share)).round() as usize).max(1)
}

/// «На повторе»: the songs started at least twice over the last two weeks
/// (a month once the history is that long), what plays on repeat now first
/// (the hot affinity, then the count).
fn on_repeat(snap: &Snapshot, tiles: &TilesInput) -> Option<Vec<TrackId>> {
    let now = snap.model.taste.now();
    let first = tiles.events.iter().map(|p| p.ts).min()?;
    let days = if now - first >= 30 * DAY_MS { REPEAT_DAYS_LONG } else { REPEAT_DAYS_SHORT };
    let mut n: HashMap<TrackId, u32> = HashMap::new();
    for p in tiles.events.iter().filter(|p| p.ts >= now - days * DAY_MS) {
        match p.kind {
            EventType::Start => *n.entry(p.track).or_default() += 1,
            EventType::Complete => *n.entry(p.track).or_default() += 1,
            _ => {}
        }
    }
    let mut top: Vec<(f32, u32, TrackId)> = n
        .into_iter()
        .filter(|(t, c)| {
            *c >= 2
                && snap.catalog.get(*t).is_some_and(|i| !snap.model.taste.blocks(i) && (i.local || !snap.local_only))
                && tiles.facts.get(t).is_none_or(|f| f.duration_ms.is_none_or(|d| d >= INTERLUDE_MS))
        })
        .map(|(t, c)| (snap.catalog.position(t).map_or(0.0, |i| snap.model.taste.hot_at(i)), c, t))
        .collect();
    top.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    let mut total = 0u64;
    let mut out = Vec::new();
    for (_, _, t) in top {
        if total >= MIX_MS {
            break;
        }
        total += u64::from(tiles.facts.get(&t).and_then(|f| f.duration_ms).unwrap_or(GUESS_MS));
        out.push(t);
    }
    (out.len() >= MIN_SONGS).then_some(out)
}

/// The main artists of `tracks`, by how many of them they have (then by
/// their first place), as the library writes them.
fn artists_of(snap: &Snapshot, tiles: &TilesInput, tracks: &[TrackId]) -> Vec<String> {
    let mut count: HashMap<ArtistId, (usize, usize)> = HashMap::new();
    for (k, t) in tracks.iter().enumerate() {
        if let Some(a) = snap.catalog.get(*t).and_then(|i| i.artists.first()) {
            let e = count.entry(*a).or_insert((0, k));
            e.0 += 1;
        }
    }
    let mut ranked: Vec<_> = count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    dedupe(ranked.into_iter().filter_map(|(a, _)| tiles.artist_names.get(&a).cloned()).collect())
}

/// Covers of the mix's first albums, each once.
fn covers(snap: &Snapshot, tracks: &[TrackId]) -> Vec<CoverRef> {
    let mut out: Vec<CoverRef> = Vec::new();
    for r in tracks.iter().filter_map(|t| snap.catalog.get(*t).and_then(|i| i.release)) {
        let c = CoverRef::Release(r);
        if !out.contains(&c) {
            out.push(c);
            if out.len() == super::COVER_CANDIDATES {
                break;
            }
        }
    }
    out
}

/// The kinds of song of a mix: the sung languages of its profiles and
/// whether soundtracks without words are among them.
fn kinds_of(snap: &Snapshot, m: &Mix) -> (HashSet<Lang>, bool) {
    let mut langs = HashSet::new();
    let mut inst = false;
    let lanes: Vec<LaneKey> = match m.key {
        MixKey::Kind(_) => m.tracks.iter().map(|&i| LaneKey::of(snap.model.song_type_at(i))).collect(),
        _ => m.profiles.iter().filter_map(|&j| snap.model.profiles.list.get(j)).map(|p| p.lane).collect(),
    };
    for l in lanes {
        match l {
            LaneKey::Sung(x) => {
                langs.insert(x);
            }
            LaneKey::Instrumental => inst = true,
            LaneKey::Unknown => {}
        }
    }
    (langs, inst)
}

/// «С новым»: new songs the services found that fit the mix — of the
/// languages of its profiles, a soundtrack's where it has soundtracks —
/// each in one mix only; as many as the share of the mix's taste profile
/// asks, those whose preview sounds of the profile first. «Фон» takes
/// none.
fn new_for(snap: &Snapshot, input: &BuildInput, m: &Mix, seed: u64, used: &mut HashSet<String>) -> Vec<StreamTrack> {
    let regions = &snap.model.regions;
    let tracks: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
    let profile = profile_of(snap, &tracks);
    let want = if input.new_log.events.is_empty() || regions.is_empty() {
        (tracks.len() / (NEW_EVERY - 1)).max(1)
    } else {
        let share = input.new_log.share(regions, profile, &mut nikgob_recs::rng::Rng::new(seed));
        new_count(tracks.len(), share)
    };
    let in_profile = |t: &StreamTrack| {
        let v = snap.new_track(t).vector.map(|v| snap.model.embed(&v));
        let of = v.and_then(|v| regions.of(&v, None));
        of.is_some_and(|j| m.profiles.contains(&j))
    };
    let (langs, inst) = kinds_of(snap, m);
    let fits = |t: &StreamTrack| {
        let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
        match snap.types.lang_of_new(&t.title, &artists) {
            Some(l) => langs.contains(&l),
            None => inst && t.album.as_ref().is_some_and(|a| franchise(&a.title).is_some()),
        }
    };
    let mut candidates: Vec<&StreamTrack> = input.new.iter().filter(|t| snap.is_new(t) && fits(t)).collect();
    candidates.sort_by_key(|t| !in_profile(t));
    let mut out = Vec::new();
    for t in candidates {
        if out.len() >= want {
            break;
        }
        if used.insert(t.id.to_string()) {
            out.push(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library where one franchise is the most loved (as MADE IN ABYSS
    /// among soundtracks): no mix plays it more than twice running nor
    /// over a quarter, and none takes a song over nine minutes.
    #[test]
    fn a_lane_has_no_run_of_one_world() {
        use nikgob_core::model::{ArtistId, ReleaseId};
        use nikgob_recs::{Catalog, DiscoveryFilter, ModelInput, SongTypes, TasteInput, TrackFacts, TrackInfo, UserModel, VectorIndex};
        let ids: Vec<TrackId> = (0..80).map(|_| TrackId::new()).collect();
        let albums: Vec<ReleaseId> = (0..12).map(|_| ReleaseId::new()).collect();
        let tracks: Vec<TrackInfo> = ids
            .iter()
            .enumerate()
            .map(|(k, id)| {
                let (world, album) = if k < 40 { ("made in abyss".to_owned(), albums[k % 4]) } else { (format!("world {}", k % 8), albums[4 + k % 8]) };
                let ms = if k == 0 { 10 * 60_000 } else { 180_000 };
                TrackInfo::new(*id, vec![ArtistId::new()], Some(album)).with_world(&world).with_duration(Some(ms))
            })
            .collect();
        let catalog = Catalog::new(tracks);
        let mut index = VectorIndex::new();
        for (k, id) in ids.iter().enumerate() {
            let mut v = [0.0f32; nikgob_recs::DIM];
            v[0] = (k % 40) as f32;
            v[1] = (k / 40) as f32;
            index.insert(*id, v);
        }
        let input = TasteInput { likes: ids[..40].to_vec(), ..TasteInput::default() };
        let model = UserModel::build(&catalog, &input, ModelInput { index: &index, types: None, weights: None }, 1_000 * DAY_MS);
        let snap = Snapshot {
            model,
            types: SongTypes::default(),
            catalog,
            index,
            filter: DiscoveryFilter::default(),
            artist_keys: HashMap::new(),
            known_albums: HashSet::new(),
            played_songs: HashSet::new(),
            suggested_new: Vec::new(),
            local_only: false,
            worlds: HashMap::new(),
            new_vectors: HashMap::new(),
            artist_affinity: HashMap::new(),
            profiles: Default::default(),
            background: Vec::new(),
        };
        let mut tiles = TilesInput::default();
        for (k, id) in ids.iter().enumerate() {
            let ms = if k == 0 { 10 * 60_000 } else { 180_000 };
            tiles.facts.insert(*id, TrackFacts { duration_ms: Some(ms), ..TrackFacts::default() });
        }
        let prefs = MixPrefs::of(&v::Prefs::default());
        let ok = |i: usize| tiles.facts.contains_key(&snap.catalog.tracks()[i].id);
        let mixes = with_builder(&snap, &prefs, &[], &ok, &Session::default(), 1_000 * DAY_MS, 0, |b| b.lineup(&Kept::default()).0);
        assert!(!mixes.is_empty());
        for m in &mixes {
            let picked: Vec<TrackId> = m.tracks.iter().map(|&i| snap.model.id(i)).collect();
            assert!(picked.len() >= MIN_SONGS, "«{}»: {}", m.title, picked.len());
            assert!(!picked.contains(&ids[0]), "ten minutes is too long");
            let mia: Vec<bool> = picked.iter().map(|t| ids[..40].contains(t)).collect();
            assert!(mia.iter().filter(|m| **m).count() * 4 <= picked.len() + 3, "«{}», a quarter: {mia:?}", m.title);
            let worlds: Vec<Option<u64>> = picked.iter().map(|t| snap.catalog.get(*t).and_then(|i| i.world)).collect();
            for w in worlds.windows(3) {
                assert!(!(w[0] == w[1] && w[1] == w[2]), "«{}», a world thrice running: {worlds:?}", m.title);
            }
        }
    }

    /// «Миксы для тебя» of a real library (`NIKGOB_REAL_LIBRARY`, a copy of
    /// it is read): the lineup of each part of the day with its first
    /// songs, the rules of diversity each mix keeps, its energy by quarters,
    /// and what of it the next day keeps.
    #[test]
    #[ignore = "needs NIKGOB_REAL_LIBRARY"]
    fn the_mixes_of_a_real_library() {
        use nikgob_recs::DayPart;
        let Some(src) = std::env::var_os("NIKGOB_REAL_LIBRARY") else { return };
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("library.sqlite");
        std::fs::copy(&src, &db).unwrap();
        for ext in ["-wal", "-shm"] {
            let mut side = src.clone();
            side.push(ext);
            if std::path::Path::new(&side).exists() {
                let mut to = db.clone().into_os_string();
                to.push(ext);
                std::fs::copy(&side, to).unwrap();
            }
        }
        let lib = nikgob_library::Library::open(&db).unwrap();
        let index = crate::analysis::SoundIndex::default();
        crate::analysis::load_index(&lib, &index);
        let now = super::super::recs_now();
        let (snap, tiles, _) = super::super::read_snapshot(&lib, &index, now, 180, &[], false).unwrap();
        let recs = snap.recs();
        let suits = |i: usize| snap.background.get(i).copied().unwrap_or(false);
        let ok = |i: usize| tiles.facts.contains_key(&snap.catalog.tracks()[i].id);
        let names = |a: ArtistId| tiles.artist_names.get(&a).cloned();
        let day = day_number(now, 180);
        let song = |i: usize| tiles.facts.get(&snap.model.id(i)).map_or_else(String::new, |f| format!("{} — {}", f.artist, f.title));
        let shares: Vec<String> = snap.model.profiles.list.iter().zip(&snap.model.profile_share).map(|(p, s)| format!("{:?} {:.2}", p.lane, s)).collect();
        println!("background listener: {}; profiles: {}", snap.model.listening.has_background, shares.join(", "));
        let mut broken = 0;
        let mut kept_day = Kept::default();
        for part in DayPart::ALL {
            let b = Builder::new(&recs, &snap.model, &ok, &suits, day, part, snap.model.live(&Session::default()), Settings::default());
            let (mut mixes, lineup) = b.lineup(&Kept::default());
            b.named(&mut mixes, &names);
            println!("{part:?}: {}", mixes.iter().map(|m| format!("«{}»", m.title)).collect::<Vec<_>>().join(", "));
            for m in &mixes {
                let info = |i: usize| &snap.catalog.tracks()[i];
                let world_runs = m.tracks.windows(3).filter(|w| info(w[0]).world.is_some() && info(w[0]).world == info(w[1]).world && info(w[1]).world == info(w[2]).world).count();
                let artist_near = (0..m.tracks.len())
                    .filter(|&k| {
                        let a = info(m.tracks[k]).artists.first();
                        (k.saturating_sub(2)..k).any(|j| info(m.tracks[j]).artists.first() == a)
                    })
                    .count();
                for k in 0..m.tracks.len() {
                    let a = info(m.tracks[k]).artists.first();
                    if let Some(j) = (k.saturating_sub(2)..k).find(|&j| info(m.tracks[j]).artists.first() == a) {
                        println!("    «{}» {j}: {} / {k}: {}", m.title, song(m.tracks[j]), song(m.tracks[k]));
                    }
                }
                let mut worlds: HashMap<u64, usize> = HashMap::new();
                for &i in &m.tracks {
                    if let Some(w) = info(i).world {
                        *worlds.entry(w).or_default() += 1;
                    }
                }
                let top = worlds.values().copied().max().unwrap_or(0) as f32 / m.tracks.len() as f32;
                let e: Vec<f32> = m.tracks.iter().filter_map(|&i| snap.model.energy_at(i)).collect();
                let mut sorted = e.clone();
                sorted.sort_by(f32::total_cmp);
                let pct = |x: f32| sorted.iter().filter(|y| **y < x).count() as f32 / sorted.len().max(1) as f32;
                let quarters: Vec<String> = e.chunks(e.len().div_ceil(4).max(1)).map(|c| format!("{:.2}", c.iter().map(|x| pct(*x)).sum::<f32>() / c.len() as f32)).collect();
                broken += world_runs + artist_near + usize::from(top > 0.25 + 1.0 / m.tracks.len() as f32);
                if part == DayPart::Day {
                    let artists: Vec<String> = m.artists.iter().take(4).filter_map(|a| names(*a)).collect();
                    println!("  «{}» · {} · {} songs · worlds {} (top {:.0}%) · world thrice {world_runs} · artist within 3 {artist_near} · energy {}", m.title, artists.join(", "), m.tracks.len(), worlds.len(), top * 100.0, quarters.join(" "));
                    for &i in m.tracks.iter().take(10) {
                        println!("      {}", song(i));
                    }
                } else {
                    println!("  «{}» energy {} · world thrice {world_runs} · artist within 3 {artist_near}", m.title, quarters.join(" "));
                }
            }
            if part == DayPart::Day {
                kept_day.lineup = lineup;
                kept_day.keep(&mixes, &snap.model);
            }
        }
        // The next day: what of each mix stays.
        let next = kept_day.clone().next_day();
        let b = Builder::new(&recs, &snap.model, &ok, &suits, day + 1, DayPart::Day, Live::default(), Settings::default());
        let (mut mixes, _) = b.lineup(&next);
        b.named(&mut mixes, &names);
        for m in &mixes {
            let before: HashSet<TrackId> = kept_day.today.iter().find(|t| t.0 == m.key).map(|t| t.2.iter().copied().collect()).unwrap_or_default();
            let same = m.tracks.iter().filter(|&&i| before.contains(&snap.model.id(i))).count();
            println!("next day «{}»: {same} of {} kept", m.title, m.tracks.len());
        }
        assert_eq!(broken, 0, "rules broken");
    }

    /// «С новым» takes as many new songs as its share asks, each between
    /// two loved familiar songs, never in the first two places nor two
    /// running.
    #[test]
    fn new_songs_stand_between_loved_ones() {
        use nikgob_core::streaming::{RemoteId, Service};
        use nikgob_recs::Item;
        assert_eq!(new_count(36, 0.1), 4);
        assert_eq!(new_count(36, 0.2), 9);
        assert_eq!(new_count(3, 0.05), 1);
        let new: Vec<StreamTrack> = (0..4)
            .map(|k| StreamTrack::new(RemoteId { service: Service::Youtube, id: format!("v{k}") }, format!("New {k}")))
            .collect();
        let anchors: Vec<bool> = (0..28).map(|k| k % 5 != 3).collect();
        let lane = LaneHome {
            key: v::MixKey::Calm,
            title: "Спокойное".into(),
            tracks: (0..28).map(|_| TrackId::new()).collect(),
            anchors: anchors.clone(),
            artists: Vec::new(),
            covers: Vec::new(),
            new,
            blend_base: None,
        };
        let picks = lane.picks(true);
        let at: Vec<usize> = picks.iter().enumerate().filter(|(_, p)| matches!(p.item, Item::Stream(_))).map(|(i, _)| i).collect();
        assert_eq!(at.len(), 4, "{at:?}");
        for (k, &i) in at.iter().enumerate() {
            assert!(i >= 2, "{at:?}");
            // The songs around it: before it k new ones stand.
            let before = i - 1 - k;
            assert!(anchors[before] && anchors[before + 1], "{at:?}");
        }
        for w in at.windows(2) {
            assert!(w[1] - w[0] >= 4, "spread: {at:?}");
        }
        assert_eq!(lane.picks(false).len(), 28);
    }
}
