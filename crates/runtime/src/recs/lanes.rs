//! «Миксы для тебя» (home v2 §3, §8): the lanes the user listens in. Songs
//! by the language of their [`SongType`](nikgob_recs::SongType) (their
//! lyrics', else their artist's, else what the title says plainly; a song
//! of an unknown language goes in no lane by language, and a lane holds
//! only songs of its own: the diversity rules may make it shorter, never
//! fill it from another); soundtracks apart and split by energy into
//! «Тихие» and «Бодрые», and «На повторе». The set and
//! its order follow the lanes' share of starts over four weeks; the songs
//! are picked anew each day (at 06:00, `local_day`) or with «Собрать
//! заново».

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{EventType, TrackId};
use nikgob_core::streaming::StreamTrack;
use nikgob_recs::songtype::VoiceSource;
use nikgob_recs::tiles::franchise;
use nikgob_recs::{Axes, Lang, TilesInput};
use nikgob_view as v;

use super::{BuildInput, CoverRef, Snapshot, day_seed};

/// A song shorter than this is an interlude: no mix picks it (OMORI has
/// 67 of 179 under a minute).
const INTERLUDE_MS: u32 = 75_000;
/// About two hours a mix.
const MIX_MS: u64 = 2 * 60 * 60 * 1000;
/// A song of unknown length counts so.
const GUESS_MS: u32 = 210_000;
/// A lane needs this many songs to be a mix.
const MIN_SONGS: usize = 12;
/// At most this many mixes.
pub(crate) const MAX_LANES: usize = 6;
/// A song longer than this is left out (they are skipped early a third of
/// the time, recs-v2 §2).
pub(super) const LONG_MS: u32 = 9 * 60 * 1000;
/// One world takes at most this share of a lane.
const WORLD_SHARE: f32 = 0.25;
/// Japanese is a lane of its own with this many songs and this share of
/// the starts; else it joins English as «Песни не на русском».
const JAPANESE_SONGS: usize = 40;
const JAPANESE_SHARE: f32 = 0.05;
/// The window of the lanes' shares.
const SHARE_DAYS: i64 = 28;
/// «На повторе» looks this far back (30 once the history is longer).
const REPEAT_DAYS_SHORT: i64 = 14;
const REPEAT_DAYS_LONG: i64 = 30;
/// About every fifth song of a mix is new «С новым».
const NEW_EVERY: usize = 5;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// One mix of «Миксы для тебя».
pub(crate) struct LaneHome {
    pub lane: v::Lane,
    pub tracks: Vec<TrackId>,
    /// Its artists, most played first.
    pub artists: Vec<String>,
    pub covers: Vec<CoverRef>,
    /// «С новым»: new songs of the services in the lane's language.
    pub new: Vec<StreamTrack>,
}

impl LaneHome {
    /// What it plays: its songs, with «С новым» a new one after every
    /// fourth.
    pub fn picks(&self, with_new: bool) -> Vec<nikgob_recs::Pick> {
        use nikgob_recs::{Item, Pick, Reason};
        let mut out = Vec::with_capacity(self.tracks.len() + self.new.len());
        let mut new = self.new.iter();
        for (i, t) in self.tracks.iter().enumerate() {
            out.push(Pick { item: Item::Track(*t), reason: Reason::Favourite });
            if with_new
                && (i + 1) % (NEW_EVERY - 1) == 0
                && let Some(n) = new.next()
            {
                out.push(Pick { item: Item::Stream(n.id.clone()), reason: Reason::New });
            }
        }
        out
    }
}

/// A number in 0..1 of `seed` and a track (a stable shuffle).
fn jitter(seed: u64, t: TrackId) -> f32 {
    let mut h = seed ^ 0x9e37_79b9_7f4a_7c15;
    for b in t.to_string().bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    h ^= h >> 33;
    (h % 10_000) as f32 / 10_000.0
}

/// The songs of each lane by their type, and those of other languages
/// than the lanes' («Песни не на русском» may take them).
fn pools(snap: &Snapshot, tiles: &TilesInput) -> (HashMap<v::Lane, Vec<(usize, TrackId)>>, Vec<(usize, TrackId)>) {
    let norm = snap.index.normalizer();
    let mut by_lane: HashMap<v::Lane, Vec<(usize, TrackId)>> = HashMap::new();
    let mut soundtracks: Vec<(usize, TrackId, Option<f32>)> = Vec::new();
    let mut other: Vec<(usize, TrackId)> = Vec::new();
    for (i, t) in snap.catalog.tracks().iter().enumerate() {
        if snap.model.taste.blocks(t) || (snap.local_only && !t.local) {
            continue;
        }
        let Some(f) = tiles.facts.get(&t.id) else { continue };
        if f.duration_ms.is_some_and(|d| d < INTERLUDE_MS) {
            continue;
        }
        let ty = snap.types.get(t.id).copied().unwrap_or_default();
        if ty.ost || ty.voice_by == VoiceSource::LrclibFlag {
            let energy = snap.index.get(t.id).map(|v| Axes::of(&norm.z(v)).energy);
            soundtracks.push((i, t.id, energy));
            continue;
        }
        let lane = match ty.lang {
            Some(Lang::Russian) => v::Lane::Russian,
            Some(Lang::Japanese) => v::Lane::Japanese,
            Some(Lang::English) => v::Lane::English,
            // Not English: only «Песни не на русском» takes it.
            Some(_) => {
                other.push((i, t.id));
                continue;
            }
            // Unknown: no lane by language («На повторе» still may).
            None => continue,
        };
        by_lane.entry(lane).or_default().push((i, t.id));
    }
    // Soundtracks: below the median energy calm, above it loud; without
    // a vector, the side of their album's analysed songs, else calm.
    let mut energies: Vec<f32> = soundtracks.iter().filter_map(|s| s.2).collect();
    energies.sort_by(f32::total_cmp);
    let median = energies.get(energies.len() / 2).copied().unwrap_or(0.0);
    for (i, id, e) in soundtracks {
        let lane = if e.is_some_and(|e| e >= median) { v::Lane::DriveSoundtracks } else { v::Lane::CalmSoundtracks };
        by_lane.entry(lane).or_default().push((i, id));
    }
    (by_lane, other)
}

/// The lanes of today.
pub(super) fn build(snap: &Snapshot, input: &BuildInput, tiles: &TilesInput) -> Vec<LaneHome> {
    let (mut by_lane, other) = pools(snap, tiles);

    // The starts of four weeks by lane.
    let now = snap.model.taste.now();
    let lane_of: HashMap<TrackId, v::Lane> =
        by_lane.iter().flat_map(|(l, ts)| ts.iter().map(move |(_, t)| (*t, *l))).collect();
    let mut starts: HashMap<v::Lane, u32> = HashMap::new();
    let mut total_starts = 0u32;
    for p in tiles.events.iter().filter(|p| p.kind == EventType::Start && p.ts >= now - SHARE_DAYS * DAY_MS) {
        total_starts += 1;
        if let Some(l) = lane_of.get(&p.track) {
            *starts.entry(*l).or_default() += 1;
        }
    }
    let ja = by_lane.get(&v::Lane::Japanese).map_or(0, Vec::len);
    let ja_share = if total_starts == 0 {
        0.0
    } else {
        starts.get(&v::Lane::Japanese).copied().unwrap_or(0) as f32 / total_starts as f32
    };
    if ja < JAPANESE_SONGS || (snap.model.taste.has_history() && ja_share < JAPANESE_SHARE) {
        let mut merged = by_lane.remove(&v::Lane::English).unwrap_or_default();
        merged.extend(by_lane.remove(&v::Lane::Japanese).unwrap_or_default());
        merged.extend(other);
        let s = starts.remove(&v::Lane::English).unwrap_or(0) + starts.remove(&v::Lane::Japanese).unwrap_or(0);
        if !merged.is_empty() {
            by_lane.insert(v::Lane::NotRussian, merged);
            starts.insert(v::Lane::NotRussian, s);
        }
    }

    let seed = day_seed(&input.day) ^ input.lanes_salt;
    let mut lanes: Vec<(v::Lane, Vec<TrackId>)> = Vec::new();
    for (lane, pool) in &by_lane {
        let picked = pick(snap, tiles, pool, seed ^ (*lane as u64).wrapping_mul(0x2545_f491_4f6c_dd1d));
        if picked.len() >= MIN_SONGS {
            lanes.push((*lane, picked));
        }
    }
    // The order of the lanes: by their share of the starts (by size without
    // a history); stable among equals.
    lanes.sort_by(|a, b| {
        let key = |l: v::Lane| (starts.get(&l).copied().unwrap_or(0), by_lane.get(&l).map_or(0, Vec::len));
        let (ka, kb) = (key(a.0), key(b.0));
        if snap.model.taste.has_history() { kb.cmp(&ka) } else { kb.1.cmp(&ka.1) }.then(a.0.cmp(&b.0))
    });
    lanes.truncate(MAX_LANES - usize::from(snap.model.taste.has_history()));
    if snap.model.taste.has_history()
        && let Some(repeat) = on_repeat(snap, tiles)
    {
        lanes.push((v::Lane::OnRepeat, repeat));
    }
    lanes.truncate(MAX_LANES);

    let mut used_new: HashSet<String> = HashSet::new();
    lanes
        .into_iter()
        .map(|(lane, tracks)| {
            let artists = artists_of(snap, tiles, &tracks);
            let covers = covers(snap, &tracks);
            let new = if snap.local_only { Vec::new() } else { new_for(snap, input, lane, tracks.len(), &mut used_new) };
            LaneHome { lane, tracks, artists, covers, new }
        })
        .collect()
}

/// About two hours of a lane: the best liked first (with a little chance
/// mixed in by the day), nothing over [`LONG_MS`], under the diversity
/// rules of every mix — an artist once in three, an album twice in ten, a
/// world twice in three and [`WORLD_SHARE`] of the lane — in the smooth
/// order by sound.
fn pick(snap: &Snapshot, tiles: &TilesInput, pool: &[(usize, TrackId)], seed: u64) -> Vec<TrackId> {
    let duration = |t: &TrackId| tiles.facts.get(t).and_then(|f| f.duration_ms);
    let mut scored: Vec<(f32, usize, TrackId)> = pool
        .iter()
        .filter(|(_, t)| duration(t).is_none_or(|d| d <= LONG_MS))
        .map(|&(i, t)| {
            let a = snap.model.taste.affinity_at(i);
            // What played lately rests about a day and a half.
            // A dislike near in sound leans it a little lower.
            let near = snap.model.taste.dislike_near_at(i);
            (a.max(0.0).sqrt() + 0.6 * jitter(seed, t) - nikgob_recs::radio::MIX_REST * snap.model.taste.lately_at(i) - near, i, t)

        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.2.cmp(&b.2)));
    // How many songs make the two hours, the best first.
    let mut total: u64 = 0;
    let mut n = 0;
    let mut songs = HashSet::new();
    for (_, i, t) in &scored {
        if total >= MIX_MS {
            break;
        }
        let info = &snap.catalog.tracks()[*i];
        if songs.insert(info.song.unwrap_or(info.id)) {
            total += u64::from(duration(t).unwrap_or(GUESS_MS));
            n += 1;
        }
    }
    let cands: Vec<(TrackId, f32)> = scored.iter().map(|(s, _, t)| (*t, *s)).collect();
    snap.recs().mix_of(&cands, n, WORLD_SHARE)
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
    let mut count: HashMap<nikgob_core::model::ArtistId, (usize, usize)> = HashMap::new();
    for (k, t) in tracks.iter().enumerate() {
        if let Some(a) = snap.catalog.get(*t).and_then(|i| i.artists.first()) {
            let e = count.entry(*a).or_insert((0, k));
            e.0 += 1;
        }
    }
    let mut ranked: Vec<_> = count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.1.1.cmp(&b.1.1)));
    let mut seen = HashSet::new();
    ranked
        .into_iter()
        .filter_map(|(a, _)| tiles.artist_names.get(&a).cloned())
        .filter(|n| seen.insert(n.to_lowercase()))
        .take(12)
        .collect()
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

/// «С новым»: new songs the services found, in the lane's language (any
/// for the soundtracks and «На повторе»), each in one mix only.
fn new_for(snap: &Snapshot, input: &BuildInput, lane: v::Lane, songs: usize, used: &mut HashSet<String>) -> Vec<StreamTrack> {
    let want = (songs / (NEW_EVERY - 1)).max(1);
    let fits = |t: &StreamTrack| {
        let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
        let lang = snap.types.lang_of_new(&t.title, &artists);
        match lane {
            v::Lane::Russian => lang == Some(Lang::Russian),
            v::Lane::English => lang == Some(Lang::English),
            v::Lane::Japanese => lang == Some(Lang::Japanese),
            v::Lane::NotRussian => lang.is_some_and(|l| l != Lang::Russian),
            v::Lane::CalmSoundtracks | v::Lane::DriveSoundtracks => {
                t.album.as_ref().is_some_and(|a| franchise(&a.title).is_some())
            }
            v::Lane::OnRepeat => true,
        }
    };
    let mut out = Vec::new();
    for t in &input.new {
        if out.len() >= want {
            break;
        }
        if snap.is_new(t) && fits(t) && used.insert(t.id.to_string()) {
            out.push(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lane where one franchise is the most loved (as MADE IN ABYSS in
    /// «Тихие саундтреки»): never more than two of it running, a quarter of
    /// the lane at most, and no song over nine minutes.
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
                TrackInfo::new(*id, vec![ArtistId::new()], Some(album)).with_world(&world)
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
        };
        let mut tiles = TilesInput::default();
        for (k, id) in ids.iter().enumerate() {
            let ms = if k == 0 { 10 * 60_000 } else { 180_000 };
            tiles.facts.insert(*id, TrackFacts { duration_ms: Some(ms), ..TrackFacts::default() });
        }
        let pool: Vec<(usize, TrackId)> = ids.iter().enumerate().map(|(k, id)| (k, *id)).collect();
        let picked = pick(&snap, &tiles, &pool, 7);
        assert!(picked.len() >= 30, "two hours: {}", picked.len());
        assert!(!picked.contains(&ids[0]), "ten minutes is too long");
        let mia: Vec<bool> = picked.iter().map(|t| ids[..40].contains(t)).collect();
        assert!(mia.iter().filter(|m| **m).count() * 4 <= picked.len() + 3, "a quarter: {mia:?}");
        let worlds: Vec<Option<u64>> = picked.iter().map(|t| snap.catalog.get(*t).and_then(|i| i.world)).collect();
        for w in worlds.windows(3) {
            assert!(!(w[0] == w[1] && w[1] == w[2]), "a world thrice running: {worlds:?}");
        }
    }

    /// A short «Песни на русском» among many English songs, one of them
    /// with a Cyrillic «е» in its English lyrics (AJR's «World's Smallest
    /// Violin» on LRCLIB): the lane holds its Russian songs only, two
    /// artists' as they are, never filled from another language.
    #[test]
    fn a_short_russian_lane_stays_pure() {
        use nikgob_core::model::ArtistId;
        use nikgob_recs::{Catalog, DiscoveryFilter, ModelInput, SongInput, TasteInput, TrackFacts, TrackInfo, UserModel, VectorIndex, song_types};
        let ru = "[00:01.00]Город в огне, и мы идём домой\n[00:05.00]По ночным дворам до самого утра";
        let en = "[00:01.00]We left the lights on in the kitchen\n[00:05.00]And the radio was playing low";
        let homoglyph = "[00:01.00]We left the lights on in the kitchen\n[00:05.00]Is it over y\u{0435}t, the radio playing low";
        let ids: Vec<TrackId> = (0..60).map(|_| TrackId::new()).collect();
        // Fourteen Russian songs of two artists, 46 English of many.
        let ru_artists = [ArtistId::new(), ArtistId::new()];
        let artists: Vec<ArtistId> = (0..60).map(|k| if k < 14 { ru_artists[k % 2] } else { ArtistId::new() }).collect();
        let catalog = Catalog::new(ids.iter().zip(&artists).map(|(id, a)| TrackInfo::new(*id, vec![*a], None)));
        let mut index = VectorIndex::new();
        for (k, id) in ids.iter().enumerate() {
            let mut v = [0.0f32; nikgob_recs::DIM];
            v[0] = k as f32;
            index.insert(*id, v);
        }
        let inputs: Vec<SongInput> = ids
            .iter()
            .enumerate()
            .map(|(k, id)| SongInput {
                id: *id,
                main_artist: Some(artists[k]),
                title: format!("Song {k}"),
                lyrics: Some((false, (if k < 14 { ru } else if k == 14 { homoglyph } else { en }).to_owned())),
                ..SongInput::default()
            })
            .collect();
        let types = song_types(&inputs);
        let model = UserModel::build(&catalog, &TasteInput::default(), ModelInput { index: &index, types: Some(&types), weights: None }, 1_000 * DAY_MS);
        let snap = Snapshot {
            model,
            types,
            catalog,
            index,
            filter: DiscoveryFilter::default(),
            artist_keys: HashMap::new(),
            known_albums: HashSet::new(),
            played_songs: HashSet::new(),
            suggested_new: Vec::new(),
            local_only: false,
            worlds: HashMap::new(),
        };
        let mut tiles = TilesInput::default();
        for id in &ids {
            tiles.facts.insert(*id, TrackFacts { duration_ms: Some(180_000), ..TrackFacts::default() });
        }
        let (lanes, _) = pools(&snap, &tiles);
        let russian: Vec<TrackId> = lanes[&v::Lane::Russian].iter().map(|(_, t)| *t).collect();
        assert_eq!(russian, ids[..14].to_vec(), "the Russian songs only, not the English one with a Cyrillic letter");
        assert!(lanes[&v::Lane::English].iter().any(|(_, t)| *t == ids[14]));
        let picked = pick(&snap, &tiles, &lanes[&v::Lane::Russian], 7);
        assert!(!picked.is_empty() && picked.iter().all(|t| russian.contains(t)), "{picked:?}");
    }
}
