//! The graph of similar music on synthetic listeners (docs/recs-graph.md
//! §8): a world of scenes whose sounds and kinds of song collide in pairs
//! (Russian rap and Russian rock, two kinds of soundtracks), a collective
//! graph with noise, partial coverage and songs beyond the library, and
//! five listeners — files only and no history, files and 20 plays, streams
//! only, a heavy history, one who never listens in the background. Each is
//! measured with the graph and without it: the next start's hit@10, how
//! much of a track's radio stays in the seed's scene and would be heard
//! out, the diversity of the radio and of «Миксы для тебя», the
//! separation of the taste profiles (ARI against the scenes), and autoplay
//! from a seed of a scene the library has little of after a session of
//! another language: how much of it keeps to the seed's kind of song, and
//! that nothing of the session comes back.
//!
//! `NIKGOB_EVAL_OUT=dir cargo test -p nikgob-recs --test graph_eval -- --nocapture`
//! writes each listener's numbers to `dir/<listener>.json`.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EventContext, EventOrigin, EventType, ReleaseId, TrackId, UnixMs};
use nikgob_recs::graph::{Edge, GraphInput, GraphSource};
use nikgob_recs::mixes::{Builder, Kept, MixKey, Settings};
use nikgob_recs::{
    Catalog, DIM, DayPart, Ended, Item, Lang, Live, ModelInput, Play, RadioParams, RadioSeed, RadioState, Recs, Session, SongType,
    SongTypes, TasteInput, TrackInfo, UserModel, VectorIndex, Voice,
};
use serde_json::{Value, json};
use uuid::Uuid;

const DAY: i64 = 86_400_000;
const T0: UnixMs = 1_790_000_000_000;
const SCENES: usize = 10;
const ARTISTS: usize = 8;
const PER_ARTIST: usize = 12;
const OUTSIDE: u128 = 1_000_000;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    fn u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn f32(&mut self) -> f32 {
        (self.u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn below(&mut self, n: usize) -> usize {
        (self.u64() % n.max(1) as u64) as usize
    }
    fn gauss(&mut self, mean: f32, sd: f32) -> f32 {
        mean + sd * ((0..12).map(|_| self.f32()).sum::<f32>() - 6.0)
    }
    /// An index by `weights`.
    fn pick(&mut self, weights: &[f32]) -> usize {
        let total: f32 = weights.iter().sum();
        let mut x = self.f32() * total;
        for (k, w) in weights.iter().enumerate() {
            x -= w;
            if x <= 0.0 {
                return k;
            }
        }
        weights.len() - 1
    }
}

fn tid(i: usize) -> TrackId {
    TrackId(Uuid::from_u128(i as u128 + 1))
}

fn aid(a: usize) -> ArtistId {
    ArtistId(Uuid::from_u128((1 << 64) | a as u128))
}

/// The world: every track's scene, artist, album, popularity in its
/// artist (1 the best known), sound and kind.
struct World {
    scene: Vec<usize>,
    artist: Vec<usize>,
    album: Vec<usize>,
    pop: Vec<f32>,
    sound: Vec<[f32; DIM]>,
    kind: Vec<SongType>,
    world: Vec<String>,
}

/// Scenes 2k and 2k+1 sound alike and are songs of one kind.
fn scene_kind(s: usize) -> SongType {
    let sung = |lang| SongType { voice: Voice::Sung, lang: Some(lang), ..SongType::default() };
    match s / 2 {
        0 => sung(Lang::Russian),
        1 => SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() },
        2 => sung(Lang::English),
        3 => sung(Lang::Japanese),
        _ => SongType::default(),
    }
}

/// The scenes heard next to each other by everyone (no likeness of sound).
fn scene_neighbour(s: usize) -> usize {
    (s + 4) % SCENES
}

fn world(seed: u64) -> World {
    let mut rng = Rng::new(seed);
    let pairs: Vec<[f32; DIM]> = (0..SCENES / 2).map(|_| std::array::from_fn(|_| rng.gauss(0.0, 1.0))).collect();
    let centres: Vec<[f32; DIM]> = (0..SCENES).map(|s| std::array::from_fn(|d| rng.gauss(pairs[s / 2][d], 0.25))).collect();
    let mut w = World { scene: Vec::new(), artist: Vec::new(), album: Vec::new(), pop: Vec::new(), sound: Vec::new(), kind: Vec::new(), world: Vec::new() };
    for (s, centre) in centres.iter().enumerate() {
        for a in 0..ARTISTS {
            let artist = s * ARTISTS + a;
            let offset: [f32; DIM] = std::array::from_fn(|_| rng.gauss(0.0, 0.35));
            let mut ranks: Vec<usize> = (0..PER_ARTIST).collect();
            for k in (1..ranks.len()).rev() {
                ranks.swap(k, rng.below(k + 1));
            }
            for (k, rank) in ranks.iter().enumerate() {
                w.scene.push(s);
                w.artist.push(artist);
                w.album.push(artist * 2 + k / 6);
                w.pop.push(1.0 - *rank as f32 / PER_ARTIST as f32);
                w.sound.push(std::array::from_fn(|d| rng.gauss(centre[d] + offset[d], 0.75)));
                w.kind.push(scene_kind(s));
                // A soundtrack's franchise is two of its composers.
                w.world.push(if s / 2 == 1 { format!("franchise {s} {}", a / 2) } else { format!("artist {artist}") });
            }
        }
    }
    w
}

/// The collective graph: YouTube Music's radio of `yt` of the tracks,
/// similar artists of most artists, tags of half, the popularity of some.
fn collective(w: &World, yt: f32, seed: u64) -> GraphInput {
    let mut rng = Rng::new(seed ^ 0x67_7261_7068);
    let n = w.scene.len();
    let by_scene: Vec<Vec<usize>> = (0..SCENES).map(|s| (0..n).filter(|&i| w.scene[i] == s).collect()).collect();
    let mut g = GraphInput::default();
    for i in 0..n {
        if rng.f32() >= yt {
            continue;
        }
        let mut seen = HashSet::from([i]);
        let mut list: Vec<u128> = Vec::new();
        while list.len() < 25 {
            let r = rng.f32();
            let to = if r < 0.3 {
                // Beyond the library.
                list.push(OUTSIDE + rng.below(50_000) as u128);
                continue;
            } else if r < 0.75 {
                let pool = &by_scene[w.scene[i]];
                pool[rng.pick(&pool.iter().map(|&j| w.pop[j] + 0.2).collect::<Vec<_>>())]
            } else if r < 0.9 {
                let pool = &by_scene[scene_neighbour(w.scene[i])];
                pool[rng.below(pool.len())]
            } else {
                rng.below(n)
            };
            if seen.insert(to) {
                list.push(to as u128 + 1);
            }
        }
        for (rank, to) in list.into_iter().enumerate() {
            g.tracks.push(Edge { from: tid(i), to: TrackId(Uuid::from_u128(to)), rank: rank as u32, of: 25, source: GraphSource::YoutubeRadio });
        }
    }
    let artists = SCENES * ARTISTS;
    for a in 0..artists {
        let s = a / ARTISTS;
        if rng.f32() < 0.85 {
            let mut seen = HashSet::from([a]);
            let mut list = Vec::new();
            while list.len() < 12 {
                let r = rng.f32();
                let b = if r < 0.75 {
                    s * ARTISTS + rng.below(ARTISTS)
                } else if r < 0.9 {
                    scene_neighbour(s) * ARTISTS + rng.below(ARTISTS)
                } else {
                    rng.below(artists)
                };
                if seen.insert(b) {
                    list.push(b);
                }
                if seen.len() >= ARTISTS * 2 {
                    break;
                }
            }
            for (rank, b) in list.iter().enumerate() {
                g.artists.push(Edge { from: aid(a), to: aid(*b), rank: rank as u32, of: list.len() as u32, source: GraphSource::DeezerArtists });
            }
        }
        if rng.f32() < 0.5 {
            g.tags.push((aid(a), format!("scene {s}"), 10.0));
            g.tags.push((aid(a), format!("family {}", s / 2), 5.0));
            g.tags.push((aid(a), format!("noise {}", rng.below(30)), 2.0));
        }
        if rng.f32() < 0.6 {
            let mut own: Vec<usize> = (0..n).filter(|&i| w.artist[i] == a).collect();
            own.sort_by(|x, y| (w.pop[*y] + 0.15 * rng.f32()).total_cmp(&(w.pop[*x] + 0.15 * rng.f32())));
            for (rank, &i) in own.iter().take(10).enumerate() {
                g.popular.push((tid(i), rank as u32, 10));
            }
        }
    }
    g
}

/// How the listener likes each scene (0: never chooses it).
const TASTE: [f32; SCENES] = [1.0, 0.08, 0.8, 0.08, 0.7, 0.05, 0.6, 0.0, 0.0, 0.0];

/// A listener's library and history.
struct Listener {
    name: &'static str,
    catalog: Catalog,
    index: VectorIndex<TrackId>,
    types: SongTypes,
    /// The world's track of each catalog position.
    of: Vec<usize>,
    train: TasteInput,
    /// Sessions after the history: (track, ts, context) of each start.
    test: Vec<Vec<(TrackId, UnixMs, EventContext)>>,
    now: UnixMs,
}

struct Habits {
    plays: usize,
    days: i64,
    local: bool,
    /// Which of the world's tracks the library holds.
    holds: fn(&World, usize) -> bool,
    likes: usize,
    background: bool,
}

/// One session of the listener: a scene by taste, its tracks by how well
/// liked and known, the next on the album now and then, another scene
/// sometimes.
fn session(w: &World, held: &[usize], rng: &mut Rng, pref: &[f32]) -> Vec<(usize, EventContext)> {
    let liked: Vec<f32> = (0..SCENES).map(|s| if held.iter().any(|&i| w.scene[i] == s) { TASTE[s] } else { 0.0 }).collect();
    let mut scene = rng.pick(&liked);
    let len = 4 + rng.below(9);
    let mut out: Vec<(usize, EventContext)> = Vec::new();
    let mut used: HashSet<usize> = HashSet::new();
    while out.len() < len {
        let r = rng.f32();
        if let Some(&(last, _)) = out.last()
            && r < 0.15
            && let Some(&next) = held.iter().find(|&&j| w.album[j] == w.album[last] && j == last + 1)
            && used.insert(next)
        {
            out.push((next, EventContext::Album));
            continue;
        }
        if !out.is_empty() && r > 0.87 {
            scene = rng.pick(&liked);
        }
        let pool: Vec<usize> = held.iter().copied().filter(|&j| w.scene[j] == scene && !used.contains(&j)).collect();
        if pool.is_empty() {
            break;
        }
        let t = pool[rng.pick(&pool.iter().map(|&j| pref[j] * pref[j] + 0.01).collect::<Vec<_>>())];
        used.insert(t);
        out.push((t, if out.is_empty() { EventContext::Library } else { EventContext::Queue }));
    }
    out
}

fn listener(name: &'static str, w: &World, h: &Habits, seed: u64) -> Listener {
    let mut rng = Rng::new(seed);
    let n = w.scene.len();
    let held: Vec<usize> = (0..n).filter(|&i| (h.holds)(w, i)).collect();
    let pref: Vec<f32> = (0..n).map(|i| 0.5 * w.pop[i] + 0.5 * rng.f32()).collect();
    let catalog = Catalog::new(held.iter().map(|&i| {
        TrackInfo::new(tid(i), vec![aid(w.artist[i])], Some(ReleaseId(Uuid::from_u128((2 << 64) | w.album[i] as u128))))
            .with_local(h.local)
            .with_duration(Some(200_000))
            .with_position((i % 6) as u32 + 1)
            .with_world(&w.world[i])
    }));
    let mut index = VectorIndex::new();
    if h.local {
        for &i in &held {
            index.insert(tid(i), w.sound[i]);
        }
    }
    let mut types = SongTypes::default();
    for &i in &held {
        types.by_track.insert(tid(i), w.kind[i]);
    }
    let of: Vec<usize> = catalog.tracks().iter().map(|t| t.id.0.as_u128() as usize - 1).collect();
    // The history, then the sessions it is tested on.
    let mut events = Vec::new();
    let mut test = Vec::new();
    let start = T0 - h.days * DAY;
    let gap = (h.days * DAY) / (h.plays / 7 + 1).max(1) as i64;
    let mut ts = start;
    let mut plays = 0;
    while plays < h.plays + 300 {
        let s = session(w, &held, &mut rng, &pref);
        let testing = plays >= h.plays;
        let mut kept = Vec::new();
        for (t, ctx) in s {
            let id = tid(t);
            let bg = h.background && rng.f32() < 0.3;
            if testing {
                kept.push((id, ts, ctx));
            } else {
                events.push(Play { context: ctx, origin: Some(EventOrigin::Manual), ..Play::new(id, ts, EventType::Start, Some(0)) });
                let early = !bg && rng.f32() > 0.8 + 0.18 * pref[t];
                let (kind, pos, len) = if early { (EventType::Skip, 9_000, 9_000) } else { (EventType::Complete, 200_000, 200_000) };
                events.push(Play { context: ctx, ..Play::new(id, ts + len, kind, Some(pos)) });
            }
            ts += 205_000;
            plays += 1;
        }
        if testing {
            test.push(kept);
        }
        ts += gap.max(40 * 60_000);
    }
    let now = if h.plays == 0 { start } else { events.iter().map(|p| p.ts).max().unwrap_or(start) + DAY / 2 };
    // Test sessions after `now`.
    let shift = now + DAY - test.first().and_then(|s| s.first()).map_or(now, |x| x.1);
    for s in &mut test {
        for x in s.iter_mut() {
            x.1 += shift;
        }
    }
    let mut likes: Vec<TrackId> = Vec::new();
    let mut by_pref: Vec<usize> = held.iter().copied().filter(|&i| TASTE[w.scene[i]] >= 0.5).collect();
    by_pref.sort_by(|a, b| pref[*b].total_cmp(&pref[*a]));
    likes.extend(by_pref.iter().take(h.likes).map(|&i| tid(i)));
    let train = TasteInput { events, likes, personal_graph: true, library_prior: true, ..TasteInput::default() };
    Listener { name, catalog, index, types, of, train, test, now }
}

/// What a variant of the engine has: the graph, and the library's prior
/// and the personal graph (both off is today's engine).
#[derive(Clone, Copy)]
struct Variant {
    graph: bool,
    prior: bool,
}

const TODAY: Variant = Variant { graph: false, prior: false };
const PRIOR: Variant = Variant { graph: false, prior: true };
const GRAPH: Variant = Variant { graph: true, prior: true };

fn model(l: &Listener, g: &GraphInput, v: Variant) -> UserModel {
    let input = TasteInput { personal_graph: v.prior, library_prior: v.prior, ..l.train.clone() };
    UserModel::build(
        &l.catalog,
        &input,
        ModelInput { index: &l.index, types: Some(&l.types), weights: None, graph: v.graph.then_some(g) },
        l.now,
    )
}

/// hit@10 and the mean reciprocal rank of the test sessions' next starts
/// by the model as built (it learns nothing from them).
fn held_out(l: &Listener, m: &UserModel) -> (f32, f32, usize) {
    let (mut hit, mut rr, mut n) = (0usize, 0.0f32, 0usize);
    for s in &l.test {
        let mut sess = Session::default();
        for (k, &(t, ts, ctx)) in s.iter().enumerate() {
            if k > 0
                && let Some(target) = m.position(t)
            {
                let mo = m.moment(&sess, ts);
                let scores: Vec<f32> = (0..m.len()).map(|c| m.score(&mo, c)).collect();
                let above = scores.iter().filter(|x| **x > scores[target]).count();
                let ties = scores.iter().filter(|x| **x == scores[target]).count() - 1;
                let rank = above as f32 + 1.0 + ties as f32 / 2.0;
                hit += usize::from(rank <= 10.0);
                rr += 1.0 / rank;
                n += 1;
            }
            sess.start(t, ts, ctx, Some(EventOrigin::Manual));
            sess.end(t, Ended::Complete);
        }
    }
    (hit as f32 / n.max(1) as f32, rr / n.max(1) as f32, n)
}

/// Would the listener hear `pick` out after a radio of `seed`: the seed's
/// scene yes, a liked scene half as much as they like it, else no.
fn accept(w: &World, seed: usize, pick: usize) -> f32 {
    if w.scene[pick] == w.scene[seed] { 1.0 } else { 0.5 * TASTE[w.scene[pick]] }
}

/// A track's radio from seeds of the liked scenes: how much of it is the
/// seed's scene, would be heard out, its artists and its worlds.
fn radio(w: &World, l: &Listener, m: &UserModel) -> Value {
    let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(m);
    let mut rng = Rng::new(77);
    let seeds: Vec<usize> = (0..l.catalog.len()).filter(|&i| TASTE[w.scene[l.of[i]]] >= 0.5).collect();
    let (mut same, mut acc, mut picks, mut artists, mut world_max, mut runs) = (0.0f32, 0.0f32, 0usize, 0.0f32, 0.0f32, 0usize);
    for k in 0..16 {
        let seed = seeds[rng.below(seeds.len())];
        let id = l.catalog.tracks()[seed].id;
        let mut s = Session::default();
        s.start(id, l.now, EventContext::Radio, Some(EventOrigin::Manual));
        s.end(id, Ended::Complete);
        let mut r = RadioState::new(RadioSeed::Track(id));
        r.attune(s, None);
        let out = r.extend(&recs, 15, &RadioParams { new_share: 0.0, seed: k });
        let got: Vec<usize> = out.iter().filter_map(|p| match p.item {
            Item::Track(t) => Some(t.0.as_u128() as usize - 1),
            Item::Stream(_) => None,
        }).collect();
        let ws = l.of[seed];
        same += got.iter().filter(|&&g| w.scene[g] == w.scene[ws]).count() as f32;
        acc += got.iter().map(|&g| accept(w, ws, g)).sum::<f32>();
        picks += got.len();
        artists += got.iter().map(|&g| w.artist[g]).collect::<HashSet<_>>().len() as f32 / got.len().max(1) as f32;
        let mut per: HashMap<&str, usize> = HashMap::new();
        for &g in &got {
            *per.entry(w.world[g].as_str()).or_default() += 1;
        }
        world_max += per.values().copied().max().unwrap_or(0) as f32 / got.len().max(1) as f32;
        runs += 1;
    }
    let p = picks.max(1) as f32;
    json!({
        "in_seed_scene": round(same / p),
        "heard_out": round(acc / p),
        "distinct_artists": round(artists / runs as f32),
        "top_world_share": round(world_max / runs as f32),
        "picks": picks,
    })
}

/// Autoplay after a session of six songs of a liked Japanese scene and a
/// seed of a little-held Russian or English scene: the share of the picks
/// of the seed's kind of song (language) and of its scene, and how many
/// played in the session.
fn autoplay(w: &World, l: &Listener, m: &UserModel) -> Value {
    let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(m);
    let mut rng = Rng::new(91);
    let of_scene = |s: usize| (0..l.catalog.len()).filter(|&i| w.scene[l.of[i]] == s).collect::<Vec<usize>>();
    let session_pool = of_scene(6);
    let seeds: Vec<usize> = [1usize, 5].iter().flat_map(|&s| of_scene(s)).collect();
    let (mut kind, mut scene, mut picks, mut again) = (0usize, 0usize, 0usize, 0usize);
    if seeds.is_empty() || session_pool.is_empty() {
        return json!({ "same_kind": 1.0, "in_seed_scene": 1.0, "repeats": 0, "picks": 0 });
    }
    for k in 0..12 {
        let mut s = Session::default();
        let mut ts = l.now - 40 * 60_000;
        let mut played = Vec::new();
        for _ in 0..6 {
            let id = l.catalog.tracks()[session_pool[rng.below(session_pool.len())]].id;
            s.start(id, ts, EventContext::HomeSection, None);
            ts += 200_000;
            s.end(id, Ended::Complete);
            played.push(id);
        }
        let seed = seeds[rng.below(seeds.len())];
        let id = l.catalog.tracks()[seed].id;
        s.start(id, ts, EventContext::Album, Some(EventOrigin::Manual));
        s.end(id, Ended::Complete);
        let mut r = RadioState::new(RadioSeed::Track(id)).continuing();
        r.attune(s, None);
        let out = r.extend(&recs, 15, &RadioParams { new_share: 0.0, seed: k });
        let ws = l.of[seed];
        for p in &out {
            let Item::Track(t) = p.item else { continue };
            let g = t.0.as_u128() as usize - 1;
            kind += usize::from(w.kind[g].lang == w.kind[ws].lang && w.kind[g].voice == w.kind[ws].voice);
            scene += usize::from(w.scene[g] == w.scene[ws]);
            again += usize::from(played.contains(&t));
            picks += 1;
        }
    }
    let p = picks.max(1) as f32;
    json!({ "same_kind": round(kind as f32 / p), "in_seed_scene": round(scene as f32 / p), "repeats": again, "picks": picks })
}

/// The adjusted Rand index of two labelings.
fn ari(a: &[usize], b: &[usize]) -> f32 {
    let mut table: HashMap<(usize, usize), f64> = HashMap::new();
    let (mut ra, mut rb): (HashMap<usize, f64>, HashMap<usize, f64>) = (HashMap::new(), HashMap::new());
    for (x, y) in a.iter().zip(b) {
        *table.entry((*x, *y)).or_default() += 1.0;
        *ra.entry(*x).or_default() += 1.0;
        *rb.entry(*y).or_default() += 1.0;
    }
    let c2 = |n: f64| n * (n - 1.0) / 2.0;
    let index: f64 = table.values().map(|n| c2(*n)).sum();
    let (sa, sb): (f64, f64) = (ra.values().map(|n| c2(*n)).sum(), rb.values().map(|n| c2(*n)).sum());
    let expected = sa * sb / c2(a.len() as f64);
    let max = (sa + sb) / 2.0;
    if (max - expected).abs() < 1e-9 { 1.0 } else { ((index - expected) / (max - expected)) as f32 }
}

/// «Миксы для тебя» and the taste profiles: how many profiles, their ARI
/// against the scenes (over the liked scenes' tracks), how pure each
/// profile's mix is in its scene, its artists and its largest world.
fn mixes(w: &World, l: &Listener, m: &UserModel) -> Value {
    let recs = Recs::new(&l.catalog, &m.taste, &l.index).with_model(m);
    let ok = |_: usize| true;
    let suits = |_: usize| true;
    let b = Builder::new(&recs, m, &ok, &suits, 7, DayPart::Evening, Live::default(), Settings::default());
    let (lineup, _) = b.lineup(&Kept::default());
    let liked: Vec<usize> = (0..m.len()).filter(|&i| TASTE[w.scene[l.of[i]]] > 0.0).collect();
    let profiles: Vec<usize> = liked.iter().map(|&i| m.profile_at(i).unwrap_or(usize::MAX)).collect();
    let scenes: Vec<usize> = liked.iter().map(|&i| w.scene[l.of[i]]).collect();
    let (mut purity, mut artists, mut world, mut own) = (0.0f32, 0.0f32, 0.0f32, 0usize);
    let mut worst_world = 0.0f32;
    for mix in &lineup {
        let got: Vec<usize> = mix.tracks.iter().map(|&i| l.of[i]).collect();
        let mut per: HashMap<&str, usize> = HashMap::new();
        for &g in &got {
            *per.entry(w.world[g].as_str()).or_default() += 1;
        }
        let top = per.values().copied().max().unwrap_or(0) as f32 / got.len().max(1) as f32;
        world += top;
        worst_world = worst_world.max(top);
        artists += got.iter().map(|&g| w.artist[g]).collect::<HashSet<_>>().len() as f32 / got.len().max(1) as f32;
        if matches!(mix.key, MixKey::Profile(_) | MixKey::Kind(_)) {
            let mut by: HashMap<usize, usize> = HashMap::new();
            for &g in &got {
                *by.entry(w.scene[g]).or_default() += 1;
            }
            purity += by.values().copied().max().unwrap_or(0) as f32 / got.len().max(1) as f32;
            own += 1;
        }
    }
    let k = lineup.len().max(1) as f32;
    json!({
        "profiles": m.profiles.len(),
        "profile_ari": round(if m.profiles.is_empty() { 0.0 } else { ari(&profiles, &scenes) }),
        "mixes": lineup.iter().map(|x| x.title.clone()).collect::<Vec<_>>(),
        "mix_scene_purity": round(purity / own.max(1) as f32),
        "mix_distinct_artists": round(artists / k),
        "mix_top_world_share": round(world / k),
        "mix_worst_world_share": round(worst_world),
        "has_background": m.listening.has_background,
    })
}

fn round(x: f32) -> f32 {
    (x * 1000.0).round() / 1000.0
}

fn hits(l: &Listener, m: &UserModel) -> Value {
    let (h, rr, n) = held_out(l, m);
    json!({ "hit10": round(h), "mrr": round(rr), "n": n })
}

/// Every variant of `l`'s engine measured; written out when asked.
fn evaluate(w: &World, l: &Listener, g: &GraphInput, replay: bool) -> Value {
    let mut out = serde_json::Map::new();
    for (name, v) in [("today", TODAY), ("prior", PRIOR), ("graph", GRAPH)] {
        let m = model(l, g, v);
        let mut row = json!({ "radio": radio(w, l, &m), "mixes": mixes(w, l, &m), "autoplay": autoplay(w, l, &m) });
        if !l.test.is_empty() && !l.train.events.is_empty() {
            row["held_out"] = hits(l, &m);
        }
        if replay {
            let input = TasteInput { personal_graph: v.prior, library_prior: v.prior, ..l.train.clone() };
            let r = nikgob_recs::model::replay(
                &l.catalog,
                &input,
                ModelInput { index: &l.index, types: Some(&l.types), weights: None, graph: v.graph.then_some(g) },
            );
            let h = |s: &str, k: &str| {
                let x = r.get(s, k);
                json!({ "n": x.n, "hit10": round(x.hit10), "mrr": round(x.mrr) })
            };
            row["replay"] = json!({ "all_learned": h("all", "learned"), "all_hand": h("all", "hand"), "free_learned": h("free", "learned"), "coverage": round(r.coverage) });
        }
        out.insert(name.to_owned(), row);
    }
    let v = json!({ "listener": l.name, "tracks": l.catalog.len(), "plays": l.train.events.iter().filter(|p| p.kind == EventType::Start).count(), "variants": out });
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    if let Some(dir) = std::env::var_os("NIKGOB_EVAL_OUT") {
        let path = std::path::Path::new(&dir).join(format!("{}.json", l.name));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
    }
    v
}

/// Autoplay from a little-held scene after a session of another language
/// keeps to the seed's kind of song and brings nothing of the session back.
fn autoplay_holds(v: &Value) {
    for k in ["today", "prior", "graph"] {
        assert!(f(v, &["variants", k, "autoplay", "same_kind"]) >= 0.9, "{k}: {v}");
        assert_eq!(f(v, &["variants", k, "autoplay", "repeats"]), 0.0, "{k}: {v}");
    }
}

fn f(v: &Value, path: &[&str]) -> f32 {
    let mut x = v;
    for p in path {
        x = &x[*p];
    }
    x.as_f64().unwrap_or_else(|| panic!("{path:?} in {v}")) as f32
}

/// The library of a collector: every scene, some of the scenes not liked
/// only in part.
fn collector(w: &World, i: usize) -> bool {
    TASTE[w.scene[i]] > 0.0 || i.is_multiple_of(3)
}

#[test]
fn files_only_and_no_history() {
    let w = world(1);
    let g = collective(&w, 0.7, 1);
    let l = listener("local_no_history", &w, &Habits { plays: 0, days: 1, local: true, holds: collector, likes: 0, background: false }, 11);
    let v = evaluate(&w, &l, &g, false);
    autoplay_holds(&v);
    let r = |k: &str, m: &str| f(&v, &["variants", k, "radio", m]);
    let x = |k: &str, m: &str| f(&v, &["variants", k, "mixes", m]);
    // The graph keeps a radio in its scene where the sound cannot.
    assert!(r("graph", "in_seed_scene") > r("today", "in_seed_scene") + 0.1, "{v}");
    assert!(r("graph", "heard_out") > r("today", "heard_out"), "{v}");
    // Profiles and their mixes from the first launch, a world a quarter at
    // most, artists apart.
    assert!(x("today", "profiles") == 0.0 && x("graph", "profiles") >= 3.0, "{v}");
    assert!(x("graph", "mix_worst_world_share") <= 0.3, "{v}");
    assert!(x("graph", "mix_distinct_artists") >= x("today", "mix_distinct_artists") - 0.1, "{v}");
}

#[test]
fn files_only_and_twenty_plays() {
    let w = world(2);
    let g = collective(&w, 0.7, 2);
    let l = listener("local_20_plays", &w, &Habits { plays: 20, days: 3, local: true, holds: collector, likes: 0, background: false }, 12);
    let v = evaluate(&w, &l, &g, false);
    autoplay_holds(&v);
    let h = |k: &str| f(&v, &["variants", k, "held_out", "hit10"]);
    let r = |k: &str, m: &str| f(&v, &["variants", k, "radio", m]);
    assert!(h("graph") > h("today"), "{v}");
    assert!(r("graph", "in_seed_scene") > r("today", "in_seed_scene"), "{v}");
    assert!(f(&v, &["variants", "graph", "mixes", "mix_worst_world_share"]) <= 0.3, "{v}");
}

#[test]
fn streams_only() {
    let w = world(3);
    let g = collective(&w, 0.95, 3);
    // Liked and saved streams of the liked scenes, no sound at all.
    let holds = |w: &World, i: usize| (TASTE[w.scene[i]] >= 0.5 && i.is_multiple_of(2)) || (TASTE[w.scene[i]] > 0.0 && i.is_multiple_of(7));
    let l = listener("streams_only", &w, &Habits { plays: 150, days: 20, local: false, holds, likes: 40, background: false }, 13);
    let v = evaluate(&w, &l, &g, false);
    autoplay_holds(&v);
    let h = |k: &str| f(&v, &["variants", k, "held_out", "hit10"]);
    assert!(h("graph") > h("today") + 0.03, "{v}");
    assert!(f(&v, &["variants", "graph", "radio", "in_seed_scene"]) >= f(&v, &["variants", "today", "radio", "in_seed_scene"]), "{v}");
}

#[test]
fn a_heavy_history() {
    let w = world(4);
    let g = collective(&w, 0.7, 4);
    let l = listener("heavy_history", &w, &Habits { plays: 3_000, days: 120, local: true, holds: collector, likes: 60, background: true }, 14);
    let v = evaluate(&w, &l, &g, true);
    autoplay_holds(&v);
    let rep = |k: &str| f(&v, &["variants", k, "replay", "all_learned", "hit10"]);
    let x = |k: &str, m: &str| f(&v, &["variants", k, "mixes", m]);
    // The graph helps a little or stays out of the way; profiles and the
    // world cap hold.
    assert!(rep("graph") >= rep("today") - 0.01, "{v}");
    assert!(f(&v, &["variants", "graph", "held_out", "hit10"]) >= f(&v, &["variants", "today", "held_out", "hit10"]) - 0.01, "{v}");
    assert!(x("graph", "profile_ari") >= x("today", "profile_ari") - 0.02, "{v}");
    assert!(x("graph", "mix_worst_world_share") <= 0.3, "{v}");
}

#[test]
fn never_in_the_background() {
    let w = world(5);
    let g = collective(&w, 0.7, 5);
    let l = listener("never_background", &w, &Habits { plays: 1_200, days: 60, local: true, holds: collector, likes: 20, background: false }, 15);
    let v = evaluate(&w, &l, &g, false);
    autoplay_holds(&v);
    let x = |k: &str, m: &str| f(&v, &["variants", k, "mixes", m]);
    assert_eq!(v["variants"]["graph"]["mixes"]["has_background"], json!(false), "{v}");
    assert_eq!(x("graph", "profiles"), x("today", "profiles"), "{v}");
    assert!(x("graph", "profile_ari") >= x("today", "profile_ari") - 0.02, "{v}");
    assert!(f(&v, &["variants", "graph", "held_out", "hit10"]) >= f(&v, &["variants", "today", "held_out", "hit10"]) - 0.01, "{v}");
    assert!(x("graph", "mix_worst_world_share") <= 0.3, "{v}");
}
