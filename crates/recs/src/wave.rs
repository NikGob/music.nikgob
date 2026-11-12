//! «Моя волна» (docs/recommendations.md §2, §3.3): one endless stream of
//! the library and of the open world, steered by the user's filters and by
//! what they do while it plays.
//!
//! * **The filters** ([`WaveSeed`]): novelty (the share of new music:
//!   [`NOVELTY_FAMILIAR`] … [`NOVELTY_ONLY_NEW`]), character (any, lively,
//!   calm, dark, the background) and scene (all, or one of the user's own
//!   scenes: a taste profile, a family of genres, a kind of song). A card
//!   of «Миксы для тебя» starts the same wave with its own filter, may keep
//!   it to a few profiles (a blend's, [`WaveSeed::only`]) or lead its prior
//!   by a part of the day ([`WaveSeed::part`]).
//! * **The clusters** ([`Bandit`]): the user's taste profiles holding at
//!   least [`MIN_ARM_TRACKS`] library tracks of the filter (with no
//!   profiles yet: the families of genres, else the kinds of song), at most
//!   [`MAX_ARMS`], the largest first. A library with fewer than two such
//!   clusters plays the filter as one.
//! * **Thompson sampling**: each cluster has Beta(α, β). The prior is how
//!   much the user plays the cluster in this part of the day (a card's part
//!   when it asks), relative to the cluster they play most then
//!   ([`PRIOR_STRENGTH`]); at each choice the clusters of the session's last
//!   songs heard out add to α ([`SESSION_WEIGHT`]), so a wave started after
//!   an evening of rap starts with rap. The cluster whose sample is highest
//!   plays a block of [`BLOCK_MIN`]–[`BLOCK_MAX`] songs (no jumps from an
//!   orchestra to autotuned rap to metal), then another sample is drawn.
//! * **What the user does**: a song heard out or liked grows its cluster's
//!   α; a rejection (a skip in its first half, a scrub-skip, a song taken
//!   out of the queue) grows β, and the block ends at once when a fresh
//!   sample prefers another cluster — always at the second rejection of a
//!   block — and the next block is of another cluster. «↻ Другой вайб»
//!   ([`Bandit::other_vibe`]) moves to another cluster at once and counts
//!   nothing against the song playing.
//! * **The songs** of a block come from [`crate::open_world::next`] with
//!   the cluster as the scene: the library and the open pool ranked
//!   together, new music by the novelty's share. A cluster that has
//!   nothing left this round is passed over; with every cluster dry the
//!   filter itself fills what is left, then its songs played again (those
//!   heard least lately first, none heard lately); only when even that is
//!   nothing the scene is let go, and the wave says so (its chip shows
//!   «Все»). The character and the novelty are never let go: a chip never
//!   shows a filter that does not hold.
//!
//! The bandit lives in the radio's state ([`crate::RadioState`]): a few
//! numbers a cluster, saved with the queue so a wave goes on after a
//! restart; nothing else is kept.

use std::collections::{HashMap, HashSet, VecDeque};

use nikgob_core::streaming::RemoteId;
use serde::{Deserialize, Serialize};

use crate::model::UserModel;
use crate::open_world::{Character, OpenCandidate, Scene, WaveContext, WaveFilter, WavePick, fits_filter, scene_of_new};
use crate::pick::Item;
use crate::profiles::LaneKey;
use crate::rng::Rng;
use crate::session::{Session, Verdict};
use crate::taste::DayPart;

/// «Знакомое»: the library alone.
pub const NOVELTY_FAMILIAR: f32 = 0.0;
/// «Баланс».
pub const NOVELTY_BALANCE: f32 = 0.3;
/// «Больше нового».
pub const NOVELTY_MORE: f32 = 0.6;
/// «Только новое»: new music alone while there is.
pub const NOVELTY_ONLY_NEW: f32 = 1.0;
/// A block of one cluster is this many songs at least…
pub const BLOCK_MIN: u8 = 4;
/// …and at most.
pub const BLOCK_MAX: u8 = 6;
/// A cluster needs this many library tracks of the filter.
pub const MIN_ARM_TRACKS: usize = 5;
/// The clusters a wave weighs at most.
pub const MAX_ARMS: usize = 12;
/// The prior's weight in songs: the cluster played most in this part of
/// the day starts as if 4 of 5 of its songs were heard out.
pub const PRIOR_STRENGTH: f32 = 4.0;
/// The session's last songs heard out add this much α to their clusters
/// at a choice, in all…
pub const SESSION_WEIGHT: f32 = 2.0;
/// …of the last this many.
const SESSION_LOOK: usize = 5;
/// The picks whose cluster is remembered (feedback comes for the queued).
const SERVED_KEEP: usize = 64;
/// A block ends at this many rejections of it.
const BLOCK_REJECTS: u8 = 2;
/// A library song is played again only past this many picks after it
/// (the queue's buffer and the song playing)…
pub const REPEAT_GAP: usize = 6;
/// …and a new song past this many (about an hour and a half).
pub const REPEAT_GAP_NEW: usize = 24;
/// Songs played again are looked for among the this many heard least
/// lately first, then twice as many, and so on.
const REPEAT_WINDOW: usize = 8;

/// What a wave plays (see the module docs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WaveSeed {
    pub filter: WaveFilter,
    /// The profiles (keys) it keeps to: a blend's; empty, any.
    #[serde(default)]
    pub only: Vec<u32>,
    /// The part of the day whose listening leads the prior; `None`, now.
    #[serde(default)]
    pub part: Option<DayPart>,
}

impl WaveSeed {
    /// The plain wave of `novelty`.
    pub fn plain(novelty: f32) -> Self {
        Self { filter: WaveFilter { novelty, character: Character::Any, scene: Scene::All }, only: Vec::new(), part: None }
    }
}

/// One cluster of the bandit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arm {
    pub scene: Scene,
    pub alpha: f32,
    pub beta: f32,
    /// Its library tracks of the filter.
    pub tracks: u32,
}

impl Arm {
    /// Its Beta's mean.
    pub fn mean(&self) -> f32 {
        self.alpha / (self.alpha + self.beta)
    }
}

/// The clusters of a wave and where it stands (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Bandit {
    pub arms: Vec<Arm>,
    /// What the arms were found for: the filter and the profiles kept to.
    #[serde(default)]
    built_for: Option<(WaveFilter, Vec<u32>)>,
    /// The cluster of the block being picked (ahead of the song playing
    /// by the queue's buffer).
    pub current: Option<usize>,
    /// The number of the block being picked (each choice a new one).
    #[serde(default)]
    block: u32,
    /// Songs of the block still to pick, and its length.
    block_left: u8,
    block_len: u8,
    /// The block rejected last (its number) and its rejections: a block's
    /// songs are rejected as they play, after the next block was picked.
    #[serde(default)]
    rejects: (u32, u8),
    /// The next block is not of this cluster.
    avoid: Option<usize>,
    /// The cluster and the block of each recent pick.
    served: VecDeque<Served>,
    /// Seeds the draws (the radio's), and how many were drawn.
    seed: u64,
    draws: u64,
}

/// A pick the bandit remembers: its cluster and its block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Served {
    item: Item,
    arm: usize,
    block: u32,
}

/// One draw of Gamma(`shape`, 1) (Marsaglia–Tsang; a shape below one is
/// lifted by a uniform's power).
fn gamma(rng: &mut Rng, shape: f32) -> f32 {
    let shape = f64::from(shape.max(1e-3));
    if shape < 1.0 {
        let u = rng.f64().max(f64::MIN_POSITIVE);
        return gamma(rng, (shape + 1.0) as f32) * u.powf(1.0 / shape) as f32;
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        // A normal draw by Box–Muller.
        let (u1, u2) = (rng.f64().max(f64::MIN_POSITIVE), rng.f64());
        let x = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
        let v = (1.0 + c * x).powi(3);
        if v <= 0.0 {
            continue;
        }
        let u = rng.f64().max(f64::MIN_POSITIVE);
        if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
            return (d * v) as f32;
        }
    }
}

/// One draw of Beta(`a`, `b`).
pub fn beta(rng: &mut Rng, a: f32, b: f32) -> f32 {
    let x = gamma(rng, a);
    let y = gamma(rng, b);
    if x + y <= 0.0 { 0.5 } else { x / (x + y) }
}

/// The cluster key of the library track at `i` (see the module docs).
fn cluster_of(ctx: &WaveContext<'_>, i: usize) -> Option<Scene> {
    let m = ctx.model;
    if !m.profiles.is_empty() {
        return m.profile_at(i).and_then(|j| m.profiles.list.get(j)).map(|p| Scene::Profile(p.key));
    }
    if let Some(f) = ctx.genre.and_then(|g| g(i)) {
        return Some(Scene::Genre(f));
    }
    match LaneKey::of(m.song_type_at(i)) {
        LaneKey::Unknown => None,
        l => Some(Scene::Kind(l)),
    }
}

/// Whether the library track at `i` is of `scene`.
fn of_scene(ctx: &WaveContext<'_>, scene: Scene, i: usize) -> bool {
    fits_filter(ctx, &WaveFilter { novelty: 0.0, character: Character::Any, scene }, i)
}

impl Bandit {
    /// A bandit over `arms` as they are (tests, the evaluation).
    pub fn with_arms(arms: Vec<Arm>, seed: u64) -> Self {
        Self { arms, seed, ..Self::default() }
    }

    /// Its draws are seeded by `seed` (the radio's), once.
    pub fn seed_once(&mut self, seed: u64) {
        if self.seed == 0 {
            self.seed = seed.max(1);
        }
    }

    fn rng(&mut self) -> Rng {
        self.draws += 1;
        Rng::derive(self.seed, self.draws)
    }

    /// The clusters of `seed` over `ctx` with their priors (see the module
    /// docs); one arm of the filter itself when fewer than two are found.
    pub fn arms_of(ctx: &WaveContext<'_>, seed: &WaveSeed) -> Vec<Arm> {
        let m = ctx.model;
        let part = seed.part.unwrap_or_else(|| m.taste.day_part()).index();
        let now = m.now();
        // Per cluster: tracks, plays in the part of the day, love.
        let mut by: HashMap<Scene, (u32, f32, f32)> = HashMap::new();
        for i in 0..m.len() {
            if !(ctx.ok)(i) || !fits_filter(ctx, &seed.filter, i) {
                continue;
            }
            let Some(scene) = cluster_of(ctx, i) else { continue };
            if !seed.only.is_empty() && !matches!(scene, Scene::Profile(k) if seed.only.contains(&k)) {
                continue;
            }
            let e = by.entry(scene).or_default();
            e.0 += 1;
            e.1 += f32::from(m.taste.daypart_plays_at(i)[part]);
            e.2 += m.tracks[i].aff_long(now).max(0.0);
        }
        let mut found: Vec<(Scene, u32, f32, f32)> = by.into_iter().filter(|(_, v)| v.0 as usize >= MIN_ARM_TRACKS).map(|(s, v)| (s, v.0, v.1, v.2)).collect();
        found.sort_by(|a, b| b.1.cmp(&a.1).then(scene_order(a.0).cmp(&scene_order(b.0))));
        found.truncate(MAX_ARMS);
        if found.len() < 2 {
            let tracks = found.first().map_or(0, |f| f.1);
            return vec![Arm { scene: seed.filter.scene, alpha: 1.0 + PRIOR_STRENGTH / 2.0, beta: 1.0 + PRIOR_STRENGTH / 2.0, tracks }];
        }
        // How much each is played now (with love for a part never heard),
        // relative to the one played most.
        let plays: f32 = found.iter().map(|f| f.2).sum();
        let love: f32 = found.iter().map(|f| f.3).sum::<f32>().max(1e-6);
        let weight: Vec<f32> = found
            .iter()
            .map(|f| {
                let now = if plays > 0.0 { f.2 / plays } else { 0.0 };
                let loved = f.3 / love;
                let tracks = f.1 as f32;
                // A little of the cluster's size, so a cluster of the
                // library never heard still has a chance.
                0.6 * now + 0.3 * loved + 0.1 * tracks / found.iter().map(|g| g.1 as f32).sum::<f32>()
            })
            .collect();
        let top = weight.iter().copied().fold(0.0f32, f32::max).max(1e-6);
        found
            .iter()
            .zip(weight)
            .map(|(f, w)| {
                let mean = (w / top).clamp(0.1, 0.9);
                Arm { scene: f.0, alpha: 1.0 + PRIOR_STRENGTH * mean, beta: 1.0 + PRIOR_STRENGTH * (1.0 - mean), tracks: f.1 }
            })
            .collect()
    }

    /// The arms are those of `seed` (found again when its filter or the
    /// profiles it keeps to changed; a cluster found again keeps what it
    /// learnt).
    pub fn ensure(&mut self, ctx: &WaveContext<'_>, seed: &WaveSeed) {
        let key = (seed.filter, seed.only.clone());
        if self.built_for.as_ref() == Some(&key) && !self.arms.is_empty() {
            return;
        }
        let mut arms = Self::arms_of(ctx, seed);
        for a in &mut arms {
            if let Some(old) = self.arms.iter().find(|o| o.scene == a.scene) {
                a.alpha = old.alpha;
                a.beta = old.beta;
            }
        }
        self.arms = arms;
        self.built_for = Some(key);
        self.current = None;
        self.block_left = 0;
        self.avoid = None;
        self.served.clear();
    }

    /// The arms of `seed` were found (not since changed).
    pub fn is_built(&self) -> bool {
        self.built_for.is_some()
    }

    /// Chooses the next block's cluster by Thompson sampling: `bonus` adds
    /// to each arm's α for this draw (the session's), `dry` arms and the
    /// one to avoid are passed over (the avoided one too when nothing else
    /// is left). False when no arm is left.
    pub fn choose(&mut self, bonus: &[f32], dry: &[usize]) -> bool {
        let avoid = self.avoid.take();
        let mut rng = self.rng();
        let mut best: Option<(f32, usize)> = None;
        for pass in 0..2 {
            for (k, a) in self.arms.iter().enumerate() {
                if dry.contains(&k) || (pass == 0 && avoid == Some(k)) {
                    continue;
                }
                let s = beta(&mut rng, a.alpha + bonus.get(k).copied().unwrap_or(0.0), a.beta);
                if best.is_none_or(|b| s > b.0) {
                    best = Some((s, k));
                }
            }
            if best.is_some() {
                break;
            }
        }
        let Some((_, k)) = best else {
            self.current = None;
            return false;
        };
        self.current = Some(k);
        self.block = self.block.wrapping_add(1).max(1);
        self.block_len = BLOCK_MIN + rng.below(usize::from(BLOCK_MAX - BLOCK_MIN) + 1) as u8;
        self.block_left = self.block_len;
        true
    }

    /// Songs of the block still to pick.
    pub fn block_left(&self) -> u8 {
        self.block_left
    }

    /// The cluster playing.
    pub fn current_scene(&self) -> Option<Scene> {
        self.current.and_then(|k| self.arms.get(k)).map(|a| a.scene)
    }

    /// `item` was picked from arm `k`.
    pub fn note_served(&mut self, item: Item, k: usize) {
        self.served.retain(|s| s.item != item);
        // Of the block being picked when it is of `k` (else of no block).
        let block = if self.current == Some(k) { self.block } else { 0 };
        self.served.push_back(Served { item, arm: k, block });
        while self.served.len() > SERVED_KEEP {
            self.served.pop_front();
        }
        if self.current == Some(k) {
            self.block_left = self.block_left.saturating_sub(1);
        }
    }

    fn served_of(&self, item: &Item) -> Option<&Served> {
        self.served.iter().rev().find(|s| s.item == *item)
    }

    fn arm_of(&self, item: &Item) -> Option<usize> {
        self.served_of(item).map(|s| s.arm)
    }

    /// How a served song ended (its verdict): heard out grows α, a
    /// rejection β, and may end the block (see the module docs).
    pub fn feedback(&mut self, item: &Item, verdict: Verdict) {
        let Some(s) = self.served_of(item) else { return };
        let (k, block) = (s.arm, s.block);
        match verdict {
            Verdict::Approved => {
                if let Some(a) = self.arms.get_mut(k) {
                    a.alpha += 1.0;
                }
            }
            v if v.rejects() => self.rejected(k, block, if v == Verdict::NotNow { 2 } else { 1 }),
            _ => {}
        }
    }

    /// A rejection of a song of arm `k` from its block `block`, counting
    /// `n`. The block is the song's own, not the one picked ahead of it
    /// meanwhile: when the wave leaves the cluster, the block being picked
    /// ends if it is of that cluster, and the next one is of another.
    fn rejected(&mut self, k: usize, block: u32, n: u8) {
        if let Some(a) = self.arms.get_mut(k) {
            a.beta += f32::from(n);
        }
        if block == 0 {
            return;
        }
        self.rejects = if self.rejects.0 == block { (block, self.rejects.1.saturating_add(n)) } else { (block, n) };
        let leave = self.rejects.1 >= BLOCK_REJECTS || {
            // One rejection: a fresh look at every cluster.
            let mut rng = self.rng();
            let draws: Vec<f32> = self.arms.iter().map(|a| beta(&mut rng, a.alpha, a.beta)).collect();
            let best = draws.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|x| x.0);
            self.arms.len() > 1 && best != Some(k)
        };
        if leave && self.arms.len() > 1 {
            if self.current == Some(k) {
                self.block_left = 0;
            }
            self.avoid = Some(k);
        }
    }

    /// A song liked while it played: its cluster's α grows.
    pub fn liked(&mut self, item: &Item) {
        if let Some(a) = self.arm_of(item).and_then(|k| self.arms.get_mut(k)) {
            a.alpha += 1.0;
        }
    }

    /// A pick taken out of the queue by hand: a rejection.
    pub fn removed(&mut self, item: &Item) {
        if let Some((k, block)) = self.served_of(item).map(|s| (s.arm, s.block)) {
            self.rejected(k, block, 1);
        }
    }

    /// Picks that left the queue unplayed: the block gets their places
    /// back.
    pub fn forget(&mut self, items: &[Item]) {
        for item in items {
            if let Some(block) = self.served_of(item).map(|s| s.block) {
                // Only to the block being picked, and not when the skips
                // ended it.
                let ended = self.avoid.is_some() && self.avoid == self.current;
                if block != 0 && block == self.block && !ended && self.block_left < self.block_len {
                    self.block_left += 1;
                }
                self.served.retain(|s| s.item != *item);
            }
        }
    }

    /// «↻ Другой вайб»: another cluster than that of the song `playing`
    /// at once (and, with three or more, than that of the block picked
    /// ahead of it), nothing counted against either. False when there is
    /// no other cluster.
    pub fn other_vibe(&mut self, playing: Option<&Item>) -> bool {
        if self.arms.len() < 2 {
            self.block_left = 0;
            return false;
        }
        let from = playing.and_then(|i| self.arm_of(i)).or(self.current);
        let ahead: Vec<usize> = self.current.filter(|c| Some(*c) != from && self.arms.len() > 2).into_iter().collect();
        self.avoid = from;
        self.choose(&[], &ahead)
    }

    /// The cluster the song `item` was picked from.
    pub fn scene_of(&self, item: &Item) -> Option<Scene> {
        self.arm_of(item).and_then(|k| self.arms.get(k)).map(|a| a.scene)
    }

    /// The α the session's last songs heard out add to each arm (see the
    /// module docs).
    pub fn session_bonus(&self, ctx: &WaveContext<'_>, session: &Session) -> Vec<f32> {
        let m = ctx.model;
        let heard: Vec<usize> = session
            .tracks()
            .rev()
            .filter(|p| Session::played_through(p, m))
            .filter_map(|p| m.position(p.track))
            .take(SESSION_LOOK)
            .collect();
        let mut out = vec![0.0f32; self.arms.len()];
        if heard.is_empty() || self.arms.len() < 2 {
            return out;
        }
        for &i in &heard {
            for (k, a) in self.arms.iter().enumerate() {
                if of_scene(ctx, a.scene, i) {
                    out[k] += SESSION_WEIGHT / heard.len() as f32;
                }
            }
        }
        out
    }
}

/// A stable order of scenes for ties.
fn scene_order(s: Scene) -> (u8, u32) {
    match s {
        Scene::All => (0, 0),
        Scene::Profile(k) => (1, k),
        Scene::Kind(l) => (2, u32::from(crate::mixes::lane_code(l))),
        Scene::Genre(f) => (3, u32::from(f)),
    }
}

/// What [`pick`] picked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Picked {
    pub picks: Vec<WavePick>,
    /// The user's scene had nothing left, even to play again: the wave
    /// went on over every scene (its seed is to say so).
    pub scene_let_go: bool,
}

/// What a wave may play again when its filter has nothing new left: the
/// keys of `served` (oldest first, as [`pick`] takes them) but the last
/// ones, the queue's: [`REPEAT_GAP`] of the library, [`REPEAT_GAP_NEW`] of
/// new music (whose plays the model does not know, so they rest by the
/// picks since).
pub fn repeatable(served: &[(String, bool)]) -> Vec<String> {
    let len = served.len();
    // Each song by its last pick (a song played again is served twice).
    let mut last: HashMap<&str, (usize, bool)> = HashMap::new();
    for (k, (key, new)) in served.iter().enumerate() {
        last.insert(key.as_str(), (k, *new));
    }
    let mut out: Vec<(usize, &str)> = last
        .into_iter()
        .filter(|(_, (k, new))| len - k > if *new { REPEAT_GAP_NEW } else { REPEAT_GAP })
        .map(|(key, (k, _))| (k, key))
        .collect();
    out.sort_unstable();
    out.into_iter().map(|(_, key)| key.to_owned()).collect()
}

/// The next `n` of the wave of `seed` (see the module docs): blocks of
/// the bandit's clusters, none of `served` (track ids and
/// [`crate::stream_song_key`]s); each pick noted with its cluster.
///
/// When every cluster is dry: the filter itself; then its songs played
/// again, from those heard least lately (`repeatable`, oldest first, see
/// [`repeatable`]), none heard within `cool_ms` (nor within the radio's
/// rest); then, only when even that is nothing, the scene is let go
/// ([`Picked::scene_let_go`]). The character and the novelty always hold:
/// with nothing of them left the wave picks nothing.
#[allow(clippy::too_many_arguments)]
pub fn pick(
    bandit: &mut Bandit,
    seed: &WaveSeed,
    ctx: &WaveContext<'_>,
    pool: &[OpenCandidate],
    session: &Session,
    n: usize,
    served: &HashSet<String>,
    repeatable: &[String],
    cool_ms: i64,
) -> Picked {
    bandit.ensure(ctx, seed);
    let m: &UserModel = ctx.model;
    let user = seed.filter;
    let keys: HashMap<&RemoteId, &str> = pool.iter().map(|c| (&c.track.id, c.key.as_str())).collect();
    // The user's scene holds whatever the cluster: the library and the
    // pool are cut to it when a cluster is another kind of scene.
    let scene_ok = |i: usize| of_scene(ctx, user.scene, i);
    let ok = |i: usize| (ctx.ok)(i) && scene_ok(i);
    let within = WaveContext { model: m, ok: &ok, genre: ctx.genre, background: ctx.background, artist_tracks: ctx.artist_tracks };
    let in_scene: Vec<OpenCandidate> = if user.scene == Scene::All {
        Vec::new()
    } else {
        pool.iter().filter(|c| scene_of_new(ctx, user.scene, c, &(ctx.artist_tracks)(&c.track))).cloned().collect()
    };
    let pool_in: &[OpenCandidate] = if user.scene == Scene::All { pool } else { &in_scene };
    let mut served = served.clone();
    let mut out: Vec<WavePick> = Vec::with_capacity(n);
    let mut dry: Vec<usize> = Vec::new();
    let bonus = bandit.session_bonus(ctx, session);
    while out.len() < n {
        if (bandit.current.is_none() || bandit.block_left == 0 || bandit.current.is_some_and(|k| dry.contains(&k))) && !bandit.choose(&bonus, &dry) {
            break;
        }
        let Some(k) = bandit.current else { break };
        let scene = bandit.arms[k].scene;
        let filter = WaveFilter { scene, ..user };
        let want = (n - out.len()).min(usize::from(bandit.block_left.max(1)));
        let (c, p) = if scene == user.scene { (ctx, pool) } else { (&within, pool_in) };
        let next = crate::open_world::next(c, p, &filter, session, want, &served);
        if next.picks.is_empty() {
            dry.push(k);
            bandit.block_left = 0;
            continue;
        }
        for p in next.picks {
            served.insert(served_key(&p.item, &keys));
            bandit.note_served(p.item.clone(), k);
            out.push(p);
        }
    }
    // Every cluster dry: the filter itself, then its songs again.
    let mut scene_let_go = false;
    let mut stages = vec![user];
    if user.scene != Scene::All {
        stages.push(WaveFilter { scene: Scene::All, ..user });
    }
    for (stage, f) in stages.iter().enumerate() {
        if out.len() >= n {
            break;
        }
        let before = out.len();
        let next = crate::open_world::next(ctx, pool, f, session, n - out.len(), &served);
        for p in next.picks {
            served.insert(served_key(&p.item, &keys));
            out.push(p);
        }
        if out.len() < n {
            let again = repeats(ctx, pool, f, session, n - out.len(), &served, repeatable, cool_ms);
            for p in again {
                served.insert(served_key(&p.item, &keys));
                out.push(p);
            }
        }
        scene_let_go |= stage > 0 && out.len() > before;
    }
    Picked { picks: out, scene_let_go }
}

/// Up to `n` songs of `filter` played again (see [`pick`]): `repeatable`
/// let back in from the oldest, a window doubling until enough are found;
/// none heard within `cool_ms` (the session's plays included).
#[allow(clippy::too_many_arguments)]
fn repeats(
    ctx: &WaveContext<'_>,
    pool: &[OpenCandidate],
    filter: &WaveFilter,
    session: &Session,
    n: usize,
    served: &HashSet<String>,
    repeatable: &[String],
    cool_ms: i64,
) -> Vec<WavePick> {
    let m = ctx.model;
    let now = session.tracks().map(|p| p.ts).max().unwrap_or(0).max(m.now());
    let lately: HashSet<usize> = session.tracks().filter(|p| now - p.ts < cool_ms).filter_map(|p| m.position(p.track)).collect();
    let ok = |i: usize| (ctx.ok)(i) && !lately.contains(&i) && m.taste.last_played_at(i).is_none_or(|t| now - t >= cool_ms);
    let cooled = WaveContext { model: m, ok: &ok, genre: ctx.genre, background: ctx.background, artist_tracks: ctx.artist_tracks };
    let keys: HashMap<&RemoteId, &str> = pool.iter().map(|c| (&c.track.id, c.key.as_str())).collect();
    let mut out: Vec<WavePick> = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut window = REPEAT_WINDOW.min(repeatable.len());
    while window > 0 && out.len() < n {
        let free: HashSet<&String> = repeatable[..window].iter().filter(|k| !taken.contains(*k)).collect();
        let allowed: HashSet<String> = served.iter().filter(|k| !free.contains(k)).cloned().chain(taken.iter().cloned()).collect();
        let next = crate::open_world::next(&cooled, pool, filter, session, n - out.len(), &allowed);
        for p in next.picks {
            taken.insert(served_key(&p.item, &keys));
            out.push(p);
        }
        if window == repeatable.len() {
            break;
        }
        window = (window * 2).min(repeatable.len());
    }
    out
}

/// What `served` holds of `item`.
fn served_key(item: &Item, keys: &HashMap<&RemoteId, &str>) -> String {
    match item {
        Item::Track(t) => t.to_string(),
        Item::Stream(r) => keys.get(r).map_or_else(|| r.to_string(), |k| (*k).to_owned()),
    }
}

/// What a card of «Миксы для тебя» is as a wave: its filter at `novelty`,
/// the profiles it keeps to, the part of the day leading its prior. `None`
/// for a card that is no wave (what played, a world's).
pub fn preset_of(key: crate::mixes::MixKey, novelty: f32) -> Option<WaveSeed> {
    use crate::mixes::MixKey as K;
    let mut s = WaveSeed::plain(novelty);
    match key {
        K::Profile(k) => s.filter.scene = Scene::Profile(k),
        K::Everything | K::Daily(_) | K::Liked => {}
        K::Calm => s.filter.character = Character::Calm,
        K::Lively => s.filter.character = Character::Lively,
        K::Background { profile, .. } => {
            s.filter.character = Character::Background;
            s.filter.scene = Scene::Profile(profile);
        }
        K::Blend(keys) => s.only = keys.into_iter().filter(|k| *k != 0).collect(),
        K::Kind(c) => s.filter.scene = Scene::Kind(crate::mixes::lane_of_code(c)),
        K::Part(p) => s.part = DayPart::ALL.get(usize::from(p)).copied(),
        K::Genre(f) => s.filter.scene = Scene::Genre(f),
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::tid;

    fn arm(k: u32, alpha: f32, beta: f32) -> Arm {
        Arm { scene: Scene::Profile(k), alpha, beta, tracks: 20 }
    }

    /// Three clusters, the first played most.
    fn bandit() -> Bandit {
        Bandit::with_arms(vec![arm(1, 4.6, 1.4), arm(2, 2.0, 4.0), arm(3, 1.4, 4.6)], 7)
    }

    #[test]
    fn beta_draws_stay_in_range_and_follow_the_mean() {
        let mut rng = Rng::new(3);
        let n = 4000;
        let mut sum = 0.0f32;
        for _ in 0..n {
            let x = beta(&mut rng, 4.0, 1.0);
            assert!((0.0..=1.0).contains(&x));
            sum += x;
        }
        let mean = sum / n as f32;
        assert!((mean - 0.8).abs() < 0.03, "{mean}");
        let small = (0..n).map(|_| beta(&mut rng, 0.5, 0.5)).sum::<f32>() / n as f32;
        assert!((small - 0.5).abs() < 0.04, "{small}");
    }

    /// The prior leads: the cluster played most at this time is chosen
    /// most often, but the others get their turns.
    #[test]
    fn the_prior_leads_the_choice() {
        let mut counts = [0usize; 3];
        for s in 0..400 {
            let mut b = bandit();
            b.seed = s;
            assert!(b.choose(&[], &[]));
            counts[b.current.unwrap()] += 1;
        }
        assert!(counts[0] > counts[1] && counts[1] > counts[2], "{counts:?}");
        assert!(counts[2] > 0, "every cluster has a chance: {counts:?}");
        // The session's cluster wins with its bonus.
        let mut with = 0;
        for s in 0..400 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[0.0, 0.0, 6.0], &[]);
            with += usize::from(b.current == Some(2));
        }
        assert!(with > counts[2] * 3, "{with} vs {}", counts[2]);
    }

    /// A block is 4–6 songs of one cluster; then another draw.
    #[test]
    fn blocks_are_four_to_six() {
        for s in 0..50 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let len = b.block_left();
            assert!((BLOCK_MIN..=BLOCK_MAX).contains(&len), "{len}");
            for t in 0..u128::from(len) {
                b.note_served(Item::Track(tid(t)), k);
            }
            assert_eq!(b.block_left(), 0);
        }
    }

    /// Heard out grows α of its cluster; a skip grows β; two skips of a
    /// block move the wave to another cluster.
    #[test]
    fn two_skips_switch_the_cluster() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let (a0, b0) = (b.arms[k].alpha, b.arms[k].beta);
            let items: Vec<Item> = (0..3).map(|t| Item::Track(tid(100 + t))).collect();
            for i in &items {
                b.note_served(i.clone(), k);
            }
            b.feedback(&items[0], Verdict::Approved);
            assert_eq!(b.arms[k].alpha, a0 + 1.0);
            b.feedback(&items[1], Verdict::Rejected);
            b.feedback(&items[2], Verdict::Rejected);
            assert_eq!(b.arms[k].beta, b0 + 2.0);
            assert_eq!(b.block_left(), 0, "the block ends");
            b.choose(&[], &[]);
            assert_ne!(b.current, Some(k), "the next block is of another cluster");
        }
    }

    /// One skip may end the block (when a fresh look prefers another
    /// cluster); it always grows β, and «не сейчас» counts as two.
    #[test]
    fn one_skip_looks_again() {
        let mut left = 0;
        for s in 0..100 {
            let mut b = Bandit::with_arms(vec![arm(1, 2.0, 2.0), arm(2, 2.0, 2.0)], s);
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            b.note_served(Item::Track(tid(1)), k);
            b.feedback(&Item::Track(tid(1)), Verdict::Rejected);
            assert_eq!(b.arms[k].beta, 3.0);
            left += usize::from(b.block_left() == 0);
        }
        assert!(left > 20 && left < 95, "about as often as the other cluster's draw wins: {left}");
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        b.note_served(Item::Track(tid(1)), k);
        b.feedback(&Item::Track(tid(1)), Verdict::NotNow);
        assert_eq!(b.block_left(), 0, "not now counts as two");
    }

    /// «↻ Другой вайб»: another cluster at once, and nothing counted
    /// against the cluster or the song playing.
    #[test]
    fn another_vibe_counts_no_skip() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            b.note_served(Item::Track(tid(1)), k);
            let before = b.arms.clone();
            assert!(b.other_vibe(Some(&Item::Track(tid(1)))));
            assert_ne!(b.current, Some(k));
            assert_eq!(b.arms, before, "no α or β moved");
            assert!((BLOCK_MIN..=BLOCK_MAX).contains(&b.block_left()));
        }
        let mut one = Bandit::with_arms(vec![arm(1, 2.0, 2.0)], 1);
        one.choose(&[], &[]);
        assert!(!one.other_vibe(None), "nowhere else to go");
    }

    /// A full block of `k` picked: the queue's buffer then picks ahead.
    fn played_block(b: &mut Bandit, k: usize, from: u128) -> Vec<Item> {
        b.avoid = None;
        let others: Vec<usize> = (0..b.arms.len()).filter(|&j| j != k).collect();
        assert!(b.choose(&[], &others));
        let items: Vec<Item> = (0..u128::from(b.block_left())).map(|t| Item::Track(tid(from + t))).collect();
        for i in &items {
            b.note_served(i.clone(), k);
        }
        items
    }

    /// Two skips of the songs playing leave their cluster though the next
    /// block was picked ahead already: of another cluster, it goes on and
    /// the one after avoids theirs; of the same, it ends.
    #[test]
    fn skips_leave_the_playing_cluster_past_a_block_boundary() {
        for s in 1..30 {
            // The next block of another cluster.
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            // One song of the next block (of the second cluster) queued.
            assert!(b.choose(&[], &[0, 2]));
            b.note_served(Item::Track(tid(200)), 1);
            let left = b.block_left();
            b.feedback(&a[a.len() - 2], Verdict::Rejected);
            b.feedback(&a[a.len() - 1], Verdict::Rejected);
            assert_eq!(b.block_left(), left, "the block ahead goes on");
            b.block_left = 0;
            b.choose(&[], &[]);
            assert_ne!(b.current, Some(0), "the next block leaves the skipped cluster");

            // The next block of the same cluster again.
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            played_block(&mut b, 0, 200);
            b.feedback(&a[a.len() - 2], Verdict::Rejected);
            b.feedback(&a[a.len() - 1], Verdict::Rejected);
            assert_eq!(b.block_left(), 0, "the block ahead of the skipped cluster ends");
            b.choose(&[], &[]);
            assert_ne!(b.current, Some(0));
        }
    }

    /// «↻ Другой вайб» leaves the cluster of the song playing, not that of
    /// the block picked ahead of it (nor that one, with three clusters).
    #[test]
    fn another_vibe_leaves_the_playing_cluster() {
        for s in 1..40 {
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            played_block(&mut b, 1, 200);
            let before = b.arms.clone();
            assert!(b.other_vibe(a.last()));
            assert_eq!(b.current, Some(2), "neither the playing nor the ahead");
            assert_eq!(b.arms, before);
            // Two clusters: the one ahead is the other.
            let mut two = Bandit::with_arms(vec![arm(1, 4.0, 1.0), arm(2, 1.0, 4.0)], s);
            let a = played_block(&mut two, 0, 100);
            played_block(&mut two, 1, 200);
            assert!(two.other_vibe(a.last()));
            assert_eq!(two.current, Some(1));
        }
    }

    /// What plays again: each song by its last pick, past the queue's gap
    /// (longer for new music), heard least lately first.
    #[test]
    fn repeatable_is_past_the_gap_oldest_first() {
        let mut served: Vec<(String, bool)> = (0..10).map(|k| (format!("t{k}"), false)).collect();
        served.push(("t0".into(), false));
        served.push(("n0".into(), true));
        let again = repeatable(&served);
        // 12 served: the last 6 (t6…t9, t0, n0) wait; t0 was picked again.
        assert_eq!(again, vec!["t1", "t2", "t3", "t4", "t5"]);
        let mut long: Vec<(String, bool)> = vec![("n0".into(), true)];
        long.extend((0..REPEAT_GAP_NEW).map(|k| (format!("t{k}"), false)));
        assert_eq!(repeatable(&long)[0], "n0");
    }

    /// Picks dropped unplayed give the block its places back; a removed
    /// pick is a rejection.
    #[test]
    fn forgotten_picks_return_to_the_block() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let left = b.block_left();
        b.note_served(Item::Track(tid(1)), k);
        b.note_served(Item::Track(tid(2)), k);
        b.forget(&[Item::Track(tid(2))]);
        assert_eq!(b.block_left(), left - 1);
        let beta0 = b.arms[k].beta;
        b.removed(&Item::Track(tid(1)));
        assert_eq!(b.arms[k].beta, beta0 + 1.0);
        // A song the bandit never served says nothing.
        b.feedback(&Item::Track(tid(9)), Verdict::Rejected);
        assert_eq!(b.arms[k].beta, beta0 + 1.0);
    }

    /// A card of the row is its wave.
    #[test]
    fn cards_are_presets() {
        use crate::mixes::MixKey as K;
        assert_eq!(preset_of(K::Calm, 0.3).unwrap().filter.character, Character::Calm);
        assert_eq!(preset_of(K::Genre(3), 0.3).unwrap().filter.scene, Scene::Genre(3));
        assert_eq!(preset_of(K::Profile(9), 0.6).unwrap().filter, WaveFilter { novelty: 0.6, character: Character::Any, scene: Scene::Profile(9) });
        let bg = preset_of(K::Background { profile: 4, card: 0, cards: 2 }, 0.0).unwrap();
        assert_eq!((bg.filter.character, bg.filter.scene), (Character::Background, Scene::Profile(4)));
        assert_eq!(preset_of(K::Blend([1, 2, 0]), 0.3).unwrap().only, vec![1, 2]);
        assert_eq!(preset_of(K::Part(3), 0.3).unwrap().part, Some(DayPart::Night));
        assert_eq!(preset_of(K::Daily(0), 0.3).unwrap(), WaveSeed::plain(0.3));
    }

    /// The bandit goes through serde with the queue.
    #[test]
    fn the_bandit_is_saved() {
        let mut b = bandit();
        b.choose(&[], &[]);
        b.note_served(Item::Track(tid(1)), 0);
        let json = serde_json::to_string(&b).unwrap();
        let back: Bandit = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }
}
