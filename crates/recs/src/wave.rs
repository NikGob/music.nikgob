//! «Моя волна»: one endless stream of
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
//!   α and lets what the wave's rejections added to every β fade
//!   ([`BETA_KEEP`]); a rejection (a skip in its first half, a scrub-skip, a
//!   song taken out of the queue) grows β. A skip or two stay in the vibe:
//!   the picks turn from the skipped song's artist and sound within the
//!   cluster, and after two running the block plays fewer new songs; the
//!   next block stays in the cluster too. Only a run of rejections of one
//!   cluster, none heard out between, leaves it ([`LEAVE_UNSETTLED`],
//!   [`LEAVE_SETTLED`], the latter of two known artists): each weighed by
//!   what it says (an unfamiliar song [`MISSED_WEIGHT`], a whole one where
//!   the wave plays new music alone; «not now» of a loved one
//!   [`NOT_NOW_WEIGHT`] or nothing once the vibe is settled: the song is
//!   worn, not the vibe); then the block ends at once and the next is of
//!   another cluster. A like approves at once. «↻ Другой вайб»
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
//! What the picks ask of the runtime ([`WaveAsks`]): the previews of the
//! new songs about to play that have none, and more new songs of a filter
//! whose new songs ran short.
//!
//! The bandit lives in the radio's state ([`crate::RadioState`]): a few
//! numbers a cluster, saved with the queue so a wave goes on after a
//! restart; nothing else is kept.

use std::collections::{HashMap, HashSet, VecDeque};

use nikgob_core::model::TrackId;
use nikgob_core::streaming::RemoteId;
use serde::{Deserialize, Serialize};

use crate::model::UserModel;
use crate::open_world::{Ahead, Character, OpenCandidate, Scene, WaveContext, WaveFilter, WavePick, WaveTurn, fits_filter, scene_of_new};
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
/// A run of rejections of one cluster, none heard out between, leaves it
/// at this weight while its vibe is not settled (the last song heard out
/// was of another, or none was)…
pub const LEAVE_UNSETTLED: f32 = 3.0;
/// …and at this weight, of two artists at least, once it is.
pub const LEAVE_SETTLED: f32 = 4.0;
/// A rejection of a known song weighs 1 in the run; of an unfamiliar one
/// («did not land») this (1 where the wave plays new music alone)…
pub const MISSED_WEIGHT: f32 = 0.5;
/// …and «not now» of a loved one this while the vibe is not settled
/// (settled, nothing).
pub const NOT_NOW_WEIGHT: f32 = 1.5;
/// At this many rejections running of the cluster being picked its block
/// plays half the share of new music.
pub const RUN_HALVE: u8 = 2;
/// A wave writes this many rejections at most into the memory of a cluster
/// ([`BanditMemory`]): one mood of an evening does not weigh on the days
/// after.
pub const MEMORY_NO_CAP: f32 = 2.0;
/// Each song heard out keeps this share of what the wave's rejections
/// added to every arm's β.
pub const BETA_KEEP: f32 = 0.85;
/// While the wave looks for the mood (its start, a block left after
/// rejections) a block is this many songs at most: a wrong cluster costs
/// a song or two, not a block…
pub const PROBE_MAX: u8 = 2;
/// …until this many songs running were heard out; then blocks are
/// [`BLOCK_MIN`]–[`BLOCK_MAX`] again.
pub const SETTLED_AFTER: u8 = 2;
/// New songs rejected in one block this often halve the share of new
/// music of the next block (not of «Только новое»).
pub const NEW_REJECTS_HALVE: u8 = 2;
/// What a wave remembers of a cluster in a part of the day
/// ([`BanditMemory`]) fades by this much a day…
pub const MEMORY_DECAY: f32 = 0.85;
/// …and weighs this many songs at most beside the prior.
pub const MEMORY_WEIGHT: f32 = 6.0;
/// The clusters remembered at most (the oldest go).
const MEMORY_CAP: usize = 96;
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

/// What the waves heard of each cluster in each part of the day, kept
/// across waves and restarts (the library's cache keeps it): songs heard
/// out and rejected, faded by [`MEMORY_DECAY`] a day. A wave started
/// adds it to its clusters' priors ([`MEMORY_WEIGHT`] songs at most), so
/// it remembers what worked yesterday evening.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BanditMemory {
    entries: Vec<Remembered>,
}

/// One cluster of one character of wave in one part of the day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Remembered {
    /// [`WaveFilter::token`] of its character and scene.
    key: String,
    part: u8,
    ok: f32,
    no: f32,
    /// The day (since the epoch) its counts are of.
    day: i64,
}

impl BanditMemory {
    fn faded(e: &Remembered, day: i64) -> (f32, f32) {
        let f = MEMORY_DECAY.powi((day - e.day).clamp(0, 365) as i32);
        (e.ok * f, e.no * f)
    }

    /// The songs heard out and rejected of cluster `key` in `part` as of
    /// `day`, [`MEMORY_WEIGHT`] in all at most.
    pub fn prior(&self, key: &str, part: u8, day: i64) -> (f32, f32) {
        let Some(e) = self.entries.iter().find(|e| e.key == key && e.part == part) else { return (0.0, 0.0) };
        let (ok, no) = Self::faded(e, day);
        let total = ok + no;
        if total > MEMORY_WEIGHT { (ok * MEMORY_WEIGHT / total, no * MEMORY_WEIGHT / total) } else { (ok, no) }
    }

    /// A song of cluster `key` was heard out (`ok`) or rejected in `part`
    /// on `day`.
    pub fn learn(&mut self, key: &str, part: u8, day: i64, ok: bool) {
        self.learn_as(key, part, day, ok, 1.0);
    }

    /// [`Self::learn`] counting `by` songs.
    pub fn learn_as(&mut self, key: &str, part: u8, day: i64, ok: bool, by: f32) {
        let at = match self.entries.iter().position(|e| e.key == key && e.part == part) {
            Some(k) => k,
            None => {
                self.entries.push(Remembered { key: key.to_owned(), part, ok: 0.0, no: 0.0, day });
                self.entries.len() - 1
            }
        };
        let e = &mut self.entries[at];
        (e.ok, e.no) = Self::faded(e, day);
        e.day = day;
        if ok {
            e.ok += by;
        } else {
            e.no += by;
        }
        if self.entries.len() > MEMORY_CAP {
            let oldest = self.entries.iter().enumerate().min_by_key(|(_, e)| e.day).map(|(k, _)| k);
            if let Some(k) = oldest {
                self.entries.remove(k);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
    /// The rejections running of the cluster rejected last ([`Run`]): a
    /// block's songs are rejected as they play, after the next block was
    /// picked.
    #[serde(default)]
    run: Option<Run>,
    /// The cluster of the last song heard out: its vibe is settled.
    #[serde(default)]
    heard_in: Option<usize>,
    /// Each arm's β as the wave found it (or as it was restored, for a
    /// wave saved before; [`BETA_KEEP`]).
    #[serde(default)]
    base_beta: Vec<f32>,
    /// What the wave's rejections wrote into the memory of each cluster,
    /// by the memory's key ([`MEMORY_NO_CAP`]), whatever its arms now.
    #[serde(default)]
    memory_no: Vec<(String, f32)>,
    /// The wave plays new music alone («Только новое»): a song that did not
    /// land weighs a whole rejection there.
    #[serde(default)]
    only_new: bool,
    /// The next block is not of this cluster.
    avoid: Option<usize>,
    /// The cluster and the block of each recent pick.
    served: VecDeque<Served>,
    /// Seeds the draws (the radio's), and how many were drawn.
    seed: u64,
    draws: u64,
    /// Songs heard out running (of any cluster), and whether the mood is
    /// lost: a block was left after its rejections and fewer than
    /// [`SETTLED_AFTER`] songs were heard out running since ([`PROBE_MAX`]).
    /// A wave starts with the mood its prior and its session say.
    #[serde(default)]
    approved_run: u8,
    #[serde(default)]
    lost: bool,
    /// The block of the new songs rejected last and how many of it.
    #[serde(default)]
    new_rejects: (u32, u8),
    /// The next block plays half the share of new music, and whether the
    /// block being picked does ([`NEW_REJECTS_HALVE`]).
    #[serde(default)]
    halve_next: bool,
    #[serde(default)]
    halved: bool,
    /// What it learns across waves ([`BanditMemory`]): the runtime hands it
    /// over and keeps it.
    #[serde(skip)]
    pub memory: BanditMemory,
    /// The day and the part of the day of its last picks, and the
    /// character of its filter: what the memory is kept by.
    #[serde(default)]
    day: i64,
    #[serde(default)]
    part: u8,
    #[serde(default)]
    character: Character,
}

/// A pick the bandit remembers: its cluster, its block, its artist (empty:
/// unknown) and whether it was liked (its approval told then).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Served {
    item: Item,
    arm: usize,
    block: u32,
    #[serde(default)]
    artist: String,
    #[serde(default)]
    liked: bool,
}

/// Rejections running of one cluster, none heard out between: their
/// weight, how many, their known artists, and whether it holds the
/// cluster no more (the wave left it, «↻ Другой вайб» did, or a song was
/// heard past its half since).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Run {
    arm: usize,
    weight: f32,
    count: u8,
    artists: Vec<String>,
    released: bool,
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
        let base_beta = arms.iter().map(|a| a.beta).collect();
        Self { arms, seed, base_beta, ..Self::default() }
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
        self.restored();
        let key = (seed.filter, seed.only.clone());
        if self.built_for.as_ref() == Some(&key) && !self.arms.is_empty() {
            return;
        }
        let mut arms = Self::arms_of(ctx, seed);
        self.stamp(ctx, seed);
        let mut base_beta = Vec::with_capacity(arms.len());
        for a in &mut arms {
            if let Some((j, old)) = self.arms.iter().enumerate().find(|(_, o)| o.scene == a.scene) {
                a.alpha = old.alpha;
                a.beta = old.beta;
                base_beta.push(self.base_beta.get(j).copied().unwrap_or(old.beta));
            } else {
                // What worked in this part of the day before.
                let (ok, no) = self.memory.prior(&self.memory_key(a.scene), self.part, self.day);
                a.alpha += ok;
                a.beta += no;
                base_beta.push(a.beta);
            }
        }
        self.arms = arms;
        self.base_beta = base_beta;
        self.built_for = Some(key);
        self.current = None;
        self.block_left = 0;
        self.avoid = None;
        self.run = None;
        self.heard_in = None;
        self.served.clear();
    }

    /// The day, the part of the day and the character the memory is kept
    /// by, as of `ctx`'s model.
    fn stamp(&mut self, ctx: &WaveContext<'_>, seed: &WaveSeed) {
        let m = ctx.model;
        self.day = m.now().div_euclid(crate::taste::DAY_MS);
        self.part = m.taste.day_part().index() as u8;
        self.character = seed.filter.character;
        self.only_new = seed.filter.novelty >= NOVELTY_ONLY_NEW;
    }

    /// A wave saved before its arms kept their β at the start: its β as
    /// restored is where what its rejections add fades to.
    fn restored(&mut self) {
        if self.base_beta.len() < self.arms.len() {
            let from = self.base_beta.len();
            self.base_beta.extend(self.arms[from..].iter().map(|a| a.beta));
        }
    }

    /// The memory's key of a cluster's `scene`.
    fn memory_key(&self, scene: Scene) -> String {
        WaveFilter { novelty: 0.0, character: self.character, scene }.token()
    }

    /// Learns a verdict of arm `k` into the memory, counting `by` songs (the
    /// rejections of a wave [`MEMORY_NO_CAP`] in all at most).
    fn remember(&mut self, k: usize, ok: bool, by: f32) {
        let Some(scene) = self.arms.get(k).map(|a| a.scene) else { return };
        let key = self.memory_key(scene);
        let by = if ok {
            by
        } else {
            let at = match self.memory_no.iter().position(|(c, _)| *c == key) {
                Some(at) => at,
                None => {
                    self.memory_no.push((key.clone(), 0.0));
                    self.memory_no.len() - 1
                }
            };
            let written = &mut self.memory_no[at].1;
            let by = by.min(MEMORY_NO_CAP - *written);
            *written += by.max(0.0);
            by
        };
        if by > 0.0 {
            self.memory.learn_as(&key, self.part, self.day, ok, by);
        }
    }

    /// The share of new music of the block being picked: `novelty`, half
    /// of it after new songs were rejected in the block before
    /// ([`NEW_REJECTS_HALVE`]; «Только новое» stays whole).
    pub fn novelty_of(&self, novelty: f32) -> f32 {
        if self.halved && novelty < NOVELTY_ONLY_NEW && crate::open_world::part_on(crate::open_world::Part::Rejected) { novelty * 0.5 } else { novelty }
    }

    /// The mood is not lost ([`SETTLED_AFTER`]): blocks are of full length.
    pub fn settled(&self) -> bool {
        !self.lost
    }

    /// The arms of `seed` were found (not since changed).
    pub fn is_built(&self) -> bool {
        self.built_for.is_some()
    }

    /// Chooses the next block's cluster by Thompson sampling: `bonus` adds
    /// to each arm's α for this draw (the session's), `dry` arms and the
    /// one to avoid are passed over (the avoided one too when nothing else
    /// is left). A cluster with rejections running short of leaving it
    /// ([`Self::held`]) is chosen again, while it has songs. False when no
    /// arm is left.
    pub fn choose(&mut self, bonus: &[f32], dry: &[usize]) -> bool {
        self.draw(bonus, dry, true)
    }

    /// The cluster a run of rejections short of leaving holds: a skip or
    /// two never move the wave, not at a block's end either.
    fn held(&self) -> Option<usize> {
        self.run.as_ref().filter(|r| !r.released && r.arm < self.arms.len() && self.arms.len() > 1).map(|r| r.arm)
    }

    /// [`Self::choose`]; `hold`: as [`Self::held`] says.
    fn draw(&mut self, bonus: &[f32], dry: &[usize], hold: bool) -> bool {
        // While the mood is lost the cluster left stays avoided (short
        // blocks would come back to it at once).
        let avoid = if self.lost { self.avoid } else { self.avoid.take() };
        let mut rng = self.rng();
        let mut best: Option<(f32, usize)> = self.held().filter(|k| hold && !dry.contains(k)).map(|k| (1.0, k));
        for pass in 0..2 {
            if best.is_some() {
                break;
            }
            for (k, a) in self.arms.iter().enumerate() {
                if dry.contains(&k) || (pass == 0 && avoid == Some(k)) {
                    continue;
                }
                let s = beta(&mut rng, a.alpha + bonus.get(k).copied().unwrap_or(0.0), a.beta);
                if best.is_none_or(|b| s > b.0) {
                    best = Some((s, k));
                }
            }
        }
        let Some((_, k)) = best else {
            self.current = None;
            return false;
        };
        self.current = Some(k);
        self.block = self.block.wrapping_add(1).max(1);
        self.halved = std::mem::take(&mut self.halve_next);
        // Short blocks while the mood is looked for.
        let (lo, hi) = if !self.lost || !crate::open_world::part_on(crate::open_world::Part::Probe) { (BLOCK_MIN, BLOCK_MAX) } else { (1, PROBE_MAX) };
        self.block_len = lo + rng.below(usize::from(hi - lo) + 1) as u8;
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

    /// `item` by `artist` (a known artist's identity; empty, unknown) was
    /// picked from arm `k`.
    pub fn note_served(&mut self, item: Item, k: usize, artist: &str) {
        self.served.retain(|s| s.item != item);
        // Of the block being picked when it is of `k` (else of no block).
        let block = if self.current == Some(k) { self.block } else { 0 };
        self.served.push_back(Served { item, arm: k, block, artist: artist.to_owned(), liked: false });
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

    /// How a served song ended (its verdict, the session's): heard out
    /// grows α, a rejection β, and a run of them leaves the cluster (see
    /// the module docs). A song liked said it all when it was liked.
    pub fn feedback(&mut self, item: &Item, verdict: Verdict) {
        self.restored();
        let Some(s) = self.served_of(item).filter(|s| !s.liked) else { return };
        let (k, block, artist) = (s.arm, s.block, s.artist.clone());
        match verdict {
            Verdict::Approved => self.approved(k),
            v if v.rejects() => {
                let weight = match v {
                    // Of new music alone every song is new: one that did not
                    // land is a rejection like any.
                    Verdict::Missed if self.only_new => 1.0,
                    Verdict::Missed => MISSED_WEIGHT,
                    Verdict::NotNow if self.heard_in == Some(k) => 0.0,
                    Verdict::NotNow => NOT_NOW_WEIGHT,
                    _ => 1.0,
                };
                // «Not now» in a settled vibe: the song is worn, not the
                // vibe; the cluster is not told.
                if weight <= 0.0 {
                    return;
                }
                self.approved_run = 0;
                self.remember(k, false, weight);
                // New songs rejected in a block: the next one plays fewer.
                if matches!(item, Item::Stream(_)) {
                    let at = if block == 0 { self.block } else { block };
                    self.new_rejects = if self.new_rejects.0 == at { (at, self.new_rejects.1.saturating_add(1)) } else { (at, 1) };
                    if self.new_rejects.1 >= NEW_REJECTS_HALVE {
                        self.halve_next = true;
                    }
                }
                self.rejected(k, weight, artist);
            }
            // Heard past its half (or on after a drag): the vibe will do,
            // the rejections running hold the cluster no more (they still
            // count).
            Verdict::Neutral => {
                if let Some(r) = self.run.as_mut() {
                    r.released = true;
                }
            }
            _ => {}
        }
    }

    /// A song of arm `k` heard out or liked: α grows, the rejections
    /// running end, its vibe is settled, and what rejections added to β
    /// fades.
    fn approved(&mut self, k: usize) {
        if let Some(a) = self.arms.get_mut(k) {
            a.alpha += 1.0;
        }
        self.remember(k, true, 1.0);
        self.approved_run = self.approved_run.saturating_add(1);
        self.run = None;
        self.heard_in = Some(k);
        for (a, base) in self.arms.iter_mut().zip(&self.base_beta) {
            if a.beta > *base {
                a.beta = base + (a.beta - base) * BETA_KEEP;
            }
        }
        // While the mood is looked for, a cluster heard out keeps its block
        // going; found, the block grows to a full one.
        let probing = self.lost && self.current == Some(k) && crate::open_world::part_on(crate::open_world::Part::Probe);
        if self.approved_run >= SETTLED_AFTER && self.lost {
            self.lost = false;
            self.avoid = None;
        }
        if probing {
            let at_least = if self.lost { 1 } else { BLOCK_MIN };
            if self.block_left < at_least {
                self.block_len = self.block_len.saturating_add(at_least - self.block_left);
                self.block_left = at_least;
            }
        }
    }

    /// A rejection of a song of arm `k` by `artist` (empty: unknown),
    /// weighing `weight`: β grows by it and it joins the run of `k` (a run
    /// of another cluster ends). At [`RUN_HALVE`] running the block being
    /// picked, of `k`, plays half the share of new music; at
    /// [`LEAVE_UNSETTLED`] (the vibe not settled) or [`LEAVE_SETTLED`] of two
    /// known artists (settled; songs of unknown artists prove no second
    /// one) the wave leaves the cluster: the block being picked ends if it
    /// is of `k` (the songs rejected are of a block before it, picked ahead
    /// meanwhile), and the next one is of another. Short of that the run
    /// holds the cluster ([`Self::held`]).
    fn rejected(&mut self, k: usize, weight: f32, artist: String) {
        if let Some(a) = self.arms.get_mut(k) {
            a.beta += weight;
        }
        if self.run.as_ref().is_none_or(|r| r.arm != k) {
            self.run = Some(Run { arm: k, ..Run::default() });
        }
        let settled = self.heard_in == Some(k);
        let Some(run) = self.run.as_mut() else { return };
        run.weight += weight;
        run.count = run.count.saturating_add(1);
        if !artist.is_empty() && !run.artists.contains(&artist) {
            run.artists.push(artist);
        }
        let leave = self.arms.len() > 1 && if settled { run.weight >= LEAVE_SETTLED && run.artists.len() >= 2 } else { run.weight >= LEAVE_UNSETTLED };
        run.released = leave;
        if run.count >= RUN_HALVE && self.current == Some(k) && crate::open_world::part_on(crate::open_world::Part::Rejected) {
            self.halved = true;
        }
        if leave {
            if self.current == Some(k) {
                self.block_left = 0;
            }
            self.avoid = Some(k);
            // The mood is lost: short blocks until it is found again.
            self.lost = true;
        }
    }

    /// The rejections running of the cluster rejected last (none since a
    /// song heard out; «not now» in a settled vibe not counted).
    pub fn skips_running(&self) -> u8 {
        self.run.as_ref().map_or(0, |r| r.count)
    }

    /// A song liked while it played: approved at once, once (its end tells
    /// nothing more).
    pub fn liked(&mut self, item: &Item) {
        self.restored();
        let Some(s) = self.served.iter_mut().rev().find(|s| s.item == *item && !s.liked) else { return };
        s.liked = true;
        let k = s.arm;
        self.approved(k);
    }

    /// A pick taken out of the queue by hand: its cluster's β grows (the
    /// queue trimmed, not the vibe heard: no run of rejections).
    pub fn removed(&mut self, item: &Item) {
        if let Some(a) = self.arm_of(item).and_then(|k| self.arms.get_mut(k)) {
            a.beta += 1.0;
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
    /// ahead of it), nothing counted against either; the rejections running
    /// hold their cluster no more. False when there is no other cluster.
    pub fn other_vibe(&mut self, playing: Option<&Item>) -> bool {
        if self.arms.len() < 2 {
            self.block_left = 0;
            return false;
        }
        if let Some(r) = self.run.as_mut() {
            r.released = true;
        }
        let from = playing.and_then(|i| self.arm_of(i)).or(self.current);
        let ahead: Vec<usize> = self.current.filter(|c| Some(*c) != from && self.arms.len() > 2).into_iter().collect();
        self.avoid = from;
        self.draw(&[], &ahead, false)
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
    pub asks: WaveAsks,
}

/// The new songs whose previews a wave asks for at most at once.
const PREVIEWS_ASKED: usize = 6;

/// What a wave's picks ask of the runtime.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaveAsks {
    /// New songs of the pool without a preview it picked or is about to
    /// pick ([`crate::open_world::WaveNext::unheard`]), [`PREVIEWS_ASKED`]
    /// at most: their sound places them before they play.
    pub previews: Vec<RemoteId>,
    /// A filter whose new songs ran short
    /// ([`crate::open_world::WaveNext::short`]): its token
    /// ([`WaveFilter::token`]) and its library songs to look around, the
    /// best loved first.
    pub short: Option<(String, Vec<nikgob_core::model::TrackId>)>,
}

impl WaveAsks {
    /// Adds what a call of [`crate::open_world::next`] for `filter` over
    /// `pool` told.
    fn add(&mut self, m: &UserModel, pool: &[OpenCandidate], filter: &WaveFilter, next: &crate::open_world::WaveNext) {
        for &k in &next.unheard {
            if self.previews.len() < PREVIEWS_ASKED && !self.previews.contains(&pool[k].track.id) {
                self.previews.push(pool[k].track.id.clone());
            }
        }
        if next.short && self.short.is_none() && !next.seeds.is_empty() {
            self.short = Some((filter.token(), next.seeds.iter().map(|&i| m.id(i)).collect()));
        }
    }
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
/// with nothing of them left the wave picks nothing. `queued`: the songs
/// picked and queued after the one playing (they play first).
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
    queued: &[Ahead],
    skipped_new: &[RemoteId],
    works: &[String],
) -> Picked {
    bandit.ensure(ctx, seed);
    bandit.stamp(ctx, seed);
    let m: &UserModel = ctx.model;
    let user = seed.filter;
    let keys: HashMap<&RemoteId, &str> = pool.iter().map(|c| (&c.track.id, c.key.as_str())).collect();
    // The user's scene holds whatever the cluster: the library and the
    // pool are cut to it when a cluster is another kind of scene.
    let scene_ok = |i: usize| of_scene(ctx, user.scene, i);
    let ok = |i: usize| (ctx.ok)(i) && scene_ok(i);
    let within = WaveContext { ok: &ok, ..*ctx };
    let in_scene: Vec<OpenCandidate> = if user.scene == Scene::All {
        Vec::new()
    } else {
        pool.iter().filter(|c| scene_of_new(ctx, user.scene, c, &(ctx.artist_tracks)(&c.track))).cloned().collect()
    };
    let pool_in: &[OpenCandidate] = if user.scene == Scene::All { pool } else { &in_scene };
    let mut served = served.clone();
    let mut out: Vec<WavePick> = Vec::with_capacity(n);
    let mut dry: Vec<usize> = Vec::new();
    let mut asks = WaveAsks::default();
    let bonus = bandit.session_bonus(ctx, session);
    // What plays before the next pick: the queue, then the picks so far.
    let mut ahead: Vec<Ahead> = queued.to_vec();
    let ahead_of = |item: &Item| match item {
        Item::Track(t) => Ahead::Track(*t),
        Item::Stream(r) => pool.iter().find(|c| c.track.id == *r).map_or(Ahead::New(crate::open_world::ArtistKey::Unknown), |c| Ahead::of_new(ctx, c)),
    };
    // Who a pick is by for the bandit: a known artist, else nobody (empty).
    let artist_of = |a: Ahead| {
        let key = match a {
            Ahead::Track(t) => m.position(t).map_or(crate::open_world::ArtistKey::Unknown, |i| crate::open_world::ArtistKey::of_track(m, i)),
            Ahead::New(key) => key,
        };
        key.label(String::new)
    };
    let run_seed = bandit.seed;
    while out.len() < n {
        // A cluster held by rejections running takes the picks back from a
        // block picked ahead of another.
        let held = bandit.held().filter(|h| bandit.current != Some(*h) && !dry.contains(h)).is_some();
        if (bandit.current.is_none() || bandit.block_left == 0 || bandit.current.is_some_and(|k| dry.contains(&k)) || held) && !bandit.choose(&bonus, &dry) {
            break;
        }
        let Some(k) = bandit.current else { break };
        let scene = bandit.arms[k].scene;
        let filter = WaveFilter { scene, novelty: bandit.novelty_of(user.novelty), ..user };
        let want = (n - out.len()).min(usize::from(bandit.block_left.max(1)));
        let (c, p) = if scene == user.scene { (ctx, pool) } else { (&within, pool_in) };
        let next = crate::open_world::next_turn(c, p, &filter, session, want, &served, WaveTurn { queued: &ahead, seed: run_seed, skipped_new, again: &[], works });
        asks.add(m, p, &filter, &next);
        if next.picks.is_empty() {
            dry.push(k);
            bandit.block_left = 0;
            continue;
        }
        for p in next.picks {
            served.insert(served_key(&p.item, &keys));
            let a = ahead_of(&p.item);
            bandit.note_served(p.item.clone(), k, &artist_of(a));
            ahead.push(a);
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
        let next = crate::open_world::next_turn(ctx, pool, f, session, n - out.len(), &served, WaveTurn { queued: &ahead, seed: run_seed, skipped_new, again: &[], works });
        asks.add(m, pool, f, &next);
        for p in next.picks {
            served.insert(served_key(&p.item, &keys));
            ahead.push(ahead_of(&p.item));
            out.push(p);
        }
        if out.len() < n {
            let again = repeats(ctx, pool, f, session, n - out.len(), &served, repeatable, cool_ms, WaveTurn { queued: &ahead, seed: run_seed, skipped_new, again: &[], works });
            for p in again {
                served.insert(served_key(&p.item, &keys));
                ahead.push(ahead_of(&p.item));
                out.push(p);
            }
        }
        scene_let_go |= stage > 0 && out.len() > before;
    }
    sequence(ctx, user.character, session, queued, &mut out);
    Picked { picks: out, scene_let_go, asks }
}

/// The library picks of a batch at most this many: orders of more are not
/// all weighed ([`sequence`]).
const SEQUENCE_MAX: usize = 6;
/// Two songs of one artist next to each other cost this much in an order.
const SAME_ARTIST_COST: f32 = 10.0;
/// A song put a place later than the pull put it costs this much: the
/// order of the pull stands unless a step saves more.
const DELAY_COST: f32 = 0.15;
/// A key change of the circle of fifths' far side costs this much, a step
/// to a neighbour a sixth of it…
const KEY_COST: f32 = 0.5;
/// …told only when both keys are this sure (the correlation of their
/// pitch classes with the key's profile).
const KEY_SURE: f32 = 0.5;
/// A head this much louder than the tail before it is free; louder still,
/// a sudden attack, and it costs up to [`ATTACK_COST`] at [`ATTACK_FULL_DB`]
/// more.
const ATTACK_FREE_DB: f32 = 6.0;
const ATTACK_FULL_DB: f32 = 12.0;
const ATTACK_COST: f32 = 0.5;

/// Orders the library's songs of a calm or background wave's batch of
/// `picks` among their own places (new songs keep theirs: their sound is
/// mostly unknown) by the cost of the steps from the song before (the last
/// queued, else the session's): the edges' when both songs have them
/// ([`crate::sketch::Edge`]: the keys on the circle of fifths, a loud head
/// straight after a quiet tail), else a DJ's step of tempo and harmony
/// ([`crate::pick::dj_cost`]), and the energy kept steady. No artist twice
/// running; each place a song is put later than the pull put it costs
/// ([`DELAY_COST`]). The order given wins a tie. A wave of any other
/// character keeps the pull's order: reordered, the owner's replay heard
/// fewer of the songs it played next and more skipped sounds back.
pub fn sequence(ctx: &WaveContext<'_>, character: Character, session: &Session, queued: &[Ahead], picks: &mut [WavePick]) {
    if !crate::open_world::part_on(crate::open_world::Part::Sequence) {
        return;
    }
    let steady = matches!(character, Character::Calm | Character::Background);
    if !steady {
        return;
    }
    let m = ctx.model;
    let Some(ix) = ctx.index else { return };
    let slots: Vec<usize> = picks.iter().enumerate().filter(|(_, p)| !p.new && matches!(p.item, Item::Track(_))).map(|(k, _)| k).collect();
    if slots.len() < 2 || slots.len() > SEQUENCE_MAX {
        return;
    }
    let prev: Option<TrackId> = queued.iter().rev().find_map(|q| if let Ahead::Track(t) = q { Some(*t) } else { None }).or_else(|| session.last().map(|p| p.track));
    let prev_artist = prev.and_then(|t| m.position(t)).map(|i| crate::open_world::ArtistKey::of_track(m, i).label(|| format!("track:{i}")));
    let id_of = |p: &WavePick| if let Item::Track(t) = p.item { Some(t) } else { None };
    let step = |a: Option<TrackId>, b: Option<TrackId>| match (a, b) {
        (Some(a), Some(b)) => transition(m, ix, a, b, steady),
        _ => 0.0,
    };
    let cost = |order: &[WavePick]| {
        let mut c = 0.0;
        let (mut before, mut before_artist) = (prev, prev_artist.clone());
        for p in order {
            c += step(before, id_of(p));
            if before_artist.as_deref() == Some(p.artist.as_str()) {
                c += SAME_ARTIST_COST;
            }
            before = id_of(p);
            before_artist = Some(p.artist.clone());
        }
        c
    };
    let lib: Vec<WavePick> = slots.iter().map(|&k| picks[k].clone()).collect();
    let mut best: (f32, Vec<usize>) = (f32::INFINITY, Vec::new());
    let mut perm: Vec<usize> = (0..lib.len()).collect();
    loop {
        let mut order = picks.to_vec();
        for (s, &j) in slots.iter().zip(&perm) {
            order[*s] = lib[j].clone();
        }
        let delay: usize = perm.iter().enumerate().map(|(at, &j)| at.saturating_sub(j)).sum();
        let c = cost(&order) + DELAY_COST * delay as f32;
        if c < best.0 - 1e-6 {
            best = (c, perm.clone());
        }
        if !next_permutation(&mut perm) {
            break;
        }
    }
    for (s, &j) in slots.iter().zip(&best.1) {
        picks[*s] = lib[j].clone();
    }
}

/// The next permutation in lexicographic order; false after the last.
fn next_permutation(p: &mut [usize]) -> bool {
    let Some(i) = (1..p.len()).rev().find(|&i| p[i - 1] < p[i]) else { return false };
    let j = (i..p.len()).rev().find(|&j| p[j] > p[i - 1]).unwrap_or(i);
    p.swap(i - 1, j);
    p[i..].reverse();
    true
}

/// The place of a key on the circle of fifths (a minor key at its relative
/// major's).
fn fifths(k: crate::sketch::Key) -> i32 {
    let root = i32::from(k.root) + if k.minor { 3 } else { 0 };
    (root * 7).rem_euclid(12)
}

/// What the step from the library's song `a` to `b` costs (see
/// [`sequence`]): 0 a seamless one.
pub(crate) fn transition(m: &UserModel, ix: &crate::index::VectorIndex<TrackId>, a: TrackId, b: TrackId, steady: bool) -> f32 {
    let edges = ix.sketch(a).and_then(|s| s.tail()).zip(ix.sketch(b).and_then(|s| s.head()));
    let mut c = match edges {
        Some((tail, head)) => edge_cost(&tail, &head),
        None => match (ix.get(a), ix.get(b)) {
            (Some(x), Some(y)) => crate::pick::dj_cost(&crate::pick::Dj::of(x), &crate::pick::Dj::of(y)),
            _ => 0.0,
        },
    };
    if steady
        && let (Some(x), Some(y)) = (m.position(a).and_then(|i| m.energy_at(i)), m.position(b).and_then(|i| m.energy_at(i)))
    {
        c += (x - y).abs();
    }
    c
}

/// The step from a song's `tail` to the next one's `head`: their keys'
/// distance on the circle of fifths (both sure), and a head far louder
/// than the tail it follows coming in at full level.
pub(crate) fn edge_cost(tail: &crate::sketch::Edge, head: &crate::sketch::Edge) -> f32 {
    let key = match (tail.key(), head.key()) {
        (Some((a, ra)), Some((b, rb))) if ra >= KEY_SURE && rb >= KEY_SURE => {
            let d = (fifths(a) - fifths(b)).abs();
            KEY_COST * d.min(12 - d) as f32 / 6.0
        }
        _ => 0.0,
    };
    let attack = if head.shape == crate::sketch::EdgeShape::Cold {
        ATTACK_COST * ((head.loudness - tail.loudness - ATTACK_FREE_DB) / (ATTACK_FULL_DB - ATTACK_FREE_DB)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    key + attack
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
    turn: WaveTurn<'_>,
) -> Vec<WavePick> {
    let m = ctx.model;
    let now = session.tracks().map(|p| p.ts).max().unwrap_or(0).max(m.now());
    let lately: HashSet<usize> = session.tracks().filter(|p| now - p.ts < cool_ms).filter_map(|p| m.position(p.track)).collect();
    let ok = |i: usize| (ctx.ok)(i) && !lately.contains(&i) && m.taste.last_played_at(i).is_none_or(|t| now - t >= cool_ms);
    let cooled = WaveContext { ok: &ok, ..*ctx };
    let keys: HashMap<&RemoteId, &str> = pool.iter().map(|c| (&c.track.id, c.key.as_str())).collect();
    let mut out: Vec<WavePick> = Vec::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut window = REPEAT_WINDOW.min(repeatable.len());
    while window > 0 && out.len() < n {
        let free: HashSet<&String> = repeatable[..window].iter().filter(|k| !taken.contains(*k)).collect();
        let allowed: HashSet<String> = served.iter().filter(|k| !free.contains(k)).cloned().chain(taken.iter().cloned()).collect();
        let back: Vec<String> = free.iter().map(|k| (*k).clone()).collect();
        let next = crate::open_world::next_turn(&cooled, pool, filter, session, n - out.len(), &allowed, WaveTurn { again: &back, ..turn });
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

    /// Once the mood is found a block is 4–6 songs of one cluster; then
    /// another draw.
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
                b.note_served(Item::Track(tid(t)), k, "");
            }
            assert_eq!(b.block_left(), 0);
        }
    }

    /// Songs `from`… of arm `k`, one by each of `artists`, served.
    fn serve(b: &mut Bandit, k: usize, from: u128, artists: &[&str]) -> Vec<Item> {
        artists
            .iter()
            .enumerate()
            .map(|(t, a)| {
                let i = Item::Track(tid(from + t as u128));
                b.note_served(i.clone(), k, a);
                i
            })
            .collect()
    }

    /// The wave's next block, drawn now, is of another cluster than `k`.
    fn has_left(b: &mut Bandit, k: usize) -> bool {
        b.block_left = 0;
        b.choose(&[], &[]);
        b.current != Some(k)
    }

    /// Heard out grows α of its cluster; a skip grows β; two skips stay
    /// in the vibe (its block goes on, playing fewer new songs), whether
    /// the vibe was settled or not.
    #[test]
    fn two_skips_stay_in_the_vibe() {
        for s in 0..40 {
            for settled in [false, true] {
                let mut b = bandit();
                b.seed = s;
                b.choose(&[], &[]);
                let k = b.current.unwrap();
                let (a0, b0) = (b.arms[k].alpha, b.arms[k].beta);
                let items = serve(&mut b, k, 100, &["a", "b", "c"]);
                if settled {
                    b.feedback(&items[0], Verdict::Approved);
                    assert_eq!(b.arms[k].alpha, a0 + 1.0);
                }
                let left_before = b.block_left();
                b.feedback(&items[1], Verdict::Rejected);
                b.feedback(&items[2], Verdict::Rejected);
                assert!((b.arms[k].beta - (b0 + 2.0)).abs() < 1e-4);
                assert_eq!(b.skips_running(), 2);
                assert_eq!(b.block_left(), left_before, "the block goes on");
                assert!(b.settled());
                assert_eq!(b.novelty_of(0.6), 0.3, "with fewer new songs");
                assert!(!has_left(&mut b, k));
            }
        }
    }

    /// One skip never leaves the cluster, however close the others are.
    #[test]
    fn one_skip_never_leaves() {
        for s in 0..100 {
            let mut b = Bandit::with_arms(vec![arm(1, 2.0, 2.0), arm(2, 2.0, 2.0)], s);
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let left_before = b.block_left();
            b.note_served(Item::Track(tid(1)), k, "");
            b.feedback(&Item::Track(tid(1)), Verdict::Rejected);
            assert_eq!(b.arms[k].beta, 3.0);
            assert_eq!(b.block_left(), left_before - 1);
            assert!(b.settled());
            assert!(!has_left(&mut b, k), "the next block is of it too");
        }
    }

    /// A vibe not settled yet (nothing heard out in it) is left at the third
    /// skip running.
    #[test]
    fn three_skips_leave_an_unsettled_vibe() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["a", "a", "a"]);
            b.feedback(&items[0], Verdict::Rejected);
            b.feedback(&items[1], Verdict::Rejected);
            assert!(b.settled());
            b.feedback(&items[2], Verdict::Rejected);
            assert_eq!(b.block_left(), 0, "the block ends");
            assert!(has_left(&mut b, k), "the next block is of another cluster");
        }
    }

    /// A settled vibe (a song of it heard out last) is left at the fourth
    /// skip running, of two artists at least: four of one artist turn from
    /// the artist, a fifth of another leaves.
    #[test]
    fn a_settled_vibe_takes_four_skips_of_two_artists() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["x", "a", "b", "a", "b"]);
            b.feedback(&items[0], Verdict::Approved);
            for i in &items[1..4] {
                b.feedback(i, Verdict::Rejected);
            }
            assert!(b.settled(), "three stay");
            b.feedback(&items[4], Verdict::Rejected);
            assert!(has_left(&mut b, k), "the fourth leaves");

            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["x", "a", "a", "a", "a", "b"]);
            b.feedback(&items[0], Verdict::Approved);
            for i in &items[1..5] {
                b.feedback(i, Verdict::Rejected);
            }
            assert!(b.settled(), "four of one artist stay");
            b.feedback(&items[5], Verdict::Rejected);
            assert!(has_left(&mut b, k), "another artist's leaves");
        }
    }

    /// A song heard out, of any cluster, ends the run of skips.
    #[test]
    fn a_song_heard_out_ends_the_run() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["a", "b", "c", "d"]);
            let other = serve(&mut b, (k + 1) % 3, 200, &["e"]);
            b.feedback(&items[0], Verdict::Rejected);
            b.feedback(&items[1], Verdict::Rejected);
            b.feedback(&other[0], Verdict::Approved);
            assert_eq!(b.skips_running(), 0);
            b.feedback(&items[2], Verdict::Rejected);
            b.feedback(&items[3], Verdict::Rejected);
            assert_eq!(b.skips_running(), 2);
            assert!(!has_left(&mut b, k), "two running, not four");
        }
    }

    /// «Не сейчас» of a loved song: in a settled vibe the song is worn, not
    /// the vibe (nothing counted: no β, no run, no memory); in a vibe not
    /// settled it weighs one and a half, two leave.
    #[test]
    fn not_now_counts_by_the_vibe() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["x", "a", "b", "c", "d", "e"]);
            b.feedback(&items[0], Verdict::Approved);
            let before = b.arms.clone();
            let memory = b.memory.clone();
            for i in &items[1..] {
                b.feedback(i, Verdict::NotNow);
            }
            assert_eq!(b.arms, before);
            assert_eq!(b.memory, memory);
            assert_eq!(b.skips_running(), 0);
            assert!(b.settled());

            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let b0 = b.arms[k].beta;
            let items = serve(&mut b, k, 100, &["a", "b"]);
            b.feedback(&items[0], Verdict::NotNow);
            assert!((b.arms[k].beta - (b0 + NOT_NOW_WEIGHT)).abs() < 1e-4);
            assert!(b.settled());
            b.feedback(&items[1], Verdict::NotNow);
            assert!(has_left(&mut b, k));
        }
    }

    /// A song that did not land (unfamiliar) weighs half a skip: five stay,
    /// the sixth leaves a vibe not settled.
    #[test]
    fn a_song_that_did_not_land_weighs_half() {
        for s in 0..40 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let b0 = b.arms[k].beta;
            let items = serve(&mut b, k, 100, &["a", "b", "c", "d", "e", "f"]);
            for i in &items[..5] {
                b.feedback(i, Verdict::Missed);
            }
            assert!((b.arms[k].beta - (b0 + 5.0 * MISSED_WEIGHT)).abs() < 1e-4);
            assert!(b.settled(), "five stay");
            b.feedback(&items[5], Verdict::Missed);
            assert!(has_left(&mut b, k), "the sixth leaves");
        }
    }

    /// One mood of an evening does not weigh on the days after: a wave
    /// writes two rejections at most into the memory of a cluster; and what
    /// its rejections added to β fades with each song heard out.
    #[test]
    fn rejections_weigh_little_past_the_mood() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let base = b.arms[k].beta;
        let items = serve(&mut b, k, 100, &["a", "b", "c", "d", "e", "f"]);
        for i in &items {
            b.feedback(i, Verdict::Rejected);
        }
        assert!((b.arms[k].beta - (base + 6.0)).abs() < 1e-4);
        let key = b.memory_key(b.arms[k].scene);
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP));
        let other = serve(&mut b, (k + 1) % 3, 200, &["g", "h"]);
        b.feedback(&other[0], Verdict::Approved);
        assert!((b.arms[k].beta - (base + 6.0 * BETA_KEEP)).abs() < 1e-4);
        b.feedback(&other[1], Verdict::Approved);
        assert!((b.arms[k].beta - (base + 6.0 * BETA_KEEP * BETA_KEEP)).abs() < 1e-4);
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP), "heard out elsewhere");
    }

    /// The memory's cap holds for a cluster through the wave whatever its
    /// arms: left out by a filter and back, it is the same cluster.
    #[test]
    fn the_memory_cap_holds_through_a_filter_and_back() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let scene = b.arms[0].scene;
        let key = b.memory_key(scene);
        for i in serve(&mut b, 0, 100, &["a", "b", "c"]) {
            b.feedback(&i, Verdict::Rejected);
        }
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP));
        // A filter without it, then one with it again (at another place).
        let a = b.arms.remove(0);
        b.arms.push(a);
        let k = b.arms.iter().position(|a| a.scene == scene).unwrap();
        b.run = None;
        for i in serve(&mut b, k, 200, &["d", "e"]) {
            b.feedback(&i, Verdict::Rejected);
        }
        assert_eq!(b.memory.prior(&key, b.part, b.day), (0.0, MEMORY_NO_CAP));
    }

    /// Songs of unknown artists prove no second artist: a settled vibe is
    /// left by four skips of two known artists, not by unknown ones.
    #[test]
    fn unknown_artists_are_no_second_artist() {
        for s in 0..20 {
            let mut b = bandit();
            b.seed = s;
            b.choose(&[], &[]);
            let k = b.current.unwrap();
            let items = serve(&mut b, k, 100, &["x", "", "", "", "", "a", "b"]);
            b.feedback(&items[0], Verdict::Approved);
            for i in &items[1..6] {
                b.feedback(i, Verdict::Rejected);
            }
            assert!(b.settled(), "four unknown and one known artist stay");
            assert!(!has_left(&mut b, k));
            b.feedback(&items[6], Verdict::Rejected);
            assert!(has_left(&mut b, k), "a second known artist leaves");
        }
    }

    /// Where the wave plays new music alone a song that did not land is a
    /// whole rejection: three leave a vibe not settled.
    #[test]
    fn of_new_music_alone_a_miss_is_a_rejection() {
        let mut b = bandit();
        b.only_new = true;
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let items = serve(&mut b, k, 100, &["a", "b", "c"]);
        for i in &items {
            b.feedback(i, Verdict::Missed);
        }
        assert!(has_left(&mut b, k));
    }

    /// A like approves at once and once: it ends the run and settles the
    /// vibe, and the song's end, skipped or heard out, adds nothing.
    #[test]
    fn a_like_approves_at_once_and_once() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let items = serve(&mut b, k, 100, &["a", "b", "c", "d"]);
        b.feedback(&items[0], Verdict::Rejected);
        b.feedback(&items[1], Verdict::Rejected);
        let a0 = b.arms[k].alpha;
        b.liked(&items[2]);
        assert_eq!(b.arms[k].alpha, a0 + 1.0);
        assert_eq!(b.skips_running(), 0);
        assert_eq!(b.heard_in, Some(k));
        let after = b.clone();
        b.feedback(&items[2], Verdict::Rejected);
        assert_eq!(b, after, "liked, then skipped: nothing more");
        b.feedback(&items[2], Verdict::Approved);
        b.liked(&items[2]);
        assert_eq!(b, after, "nor heard out, nor liked again");
        // Settled: «not now» counts nothing, a skip one.
        b.feedback(&items[3], Verdict::NotNow);
        assert_eq!(b.skips_running(), 0);
    }

    /// What came while picks were on their way is told to their state in
    /// the order it came: a like after two skips ends their run there too.
    #[test]
    fn a_like_during_an_extension_keeps_its_order() {
        let mut live = bandit();
        live.choose(&[], &[]);
        let k = live.current.unwrap();
        let items = serve(&mut live, k, 100, &["a", "b", "c"]);
        let mut picked = live.clone();
        let mut late = live.clone();
        live.feedback(&items[0], Verdict::Rejected);
        live.feedback(&items[1], Verdict::Rejected);
        live.liked(&items[2]);
        live.feedback(&items[2], Verdict::Approved);
        // The marks as they came, on the state the picks came with.
        picked.feedback(&items[0], Verdict::Rejected);
        picked.feedback(&items[1], Verdict::Rejected);
        picked.liked(&items[2]);
        picked.feedback(&items[2], Verdict::Approved);
        assert_eq!(picked, live);
        assert_eq!(picked.skips_running(), 0);
        assert!(picked.held().is_none());
        // The ends first and the likes after: the like would count twice.
        late.feedback(&items[0], Verdict::Rejected);
        late.feedback(&items[1], Verdict::Rejected);
        late.feedback(&items[2], Verdict::Approved);
        late.liked(&items[2]);
        assert!(late.arms[k].alpha > live.arms[k].alpha);
    }

    /// A song heard past its half after a skip lets the cluster go at the
    /// block's end (the skip still counts).
    #[test]
    fn a_song_heard_past_its_half_lets_the_cluster_go() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let items = serve(&mut b, k, 100, &["a", "b"]);
        b.feedback(&items[0], Verdict::Rejected);
        assert_eq!(b.held(), Some(k));
        b.feedback(&items[1], Verdict::Neutral);
        assert_eq!(b.held(), None);
        assert_eq!(b.skips_running(), 1);
    }

    /// «↻ Другой вайб»: another cluster at once, and nothing counted
    /// against the cluster or the song playing; so after a skip or two too.
    #[test]
    fn another_vibe_counts_no_skip() {
        for s in 0..40 {
            for skips in 0..3 {
                let mut b = bandit();
                b.seed = s;
                b.choose(&[], &[]);
                let k = b.current.unwrap();
                let items = serve(&mut b, k, 100, &["a", "b", "c"]);
                for i in &items[..skips] {
                    b.feedback(i, Verdict::Rejected);
                }
                let (before, memory) = (b.arms.clone(), b.memory.clone());
                assert!(b.other_vibe(items.last()));
                assert_ne!(b.current, Some(k));
                assert_eq!(b.arms, before, "no α or β moved");
                assert_eq!(b.memory, memory, "nothing remembered");
                assert!(b.settled());
                assert!((BLOCK_MIN..=BLOCK_MAX).contains(&b.block_left()));
            }
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
            b.note_served(i.clone(), k, "");
        }
        items
    }

    /// Three skips of the songs playing leave their cluster though the
    /// next block was picked ahead already: of another cluster, it goes on
    /// and the one after avoids theirs; of the same, it ends.
    #[test]
    fn skips_leave_the_playing_cluster_past_a_block_boundary() {
        for s in 1..30 {
            // The next block of another cluster.
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            // One song of the next block (of the second cluster) queued.
            assert!(b.choose(&[], &[0, 2]));
            b.note_served(Item::Track(tid(200)), 1, "");
            let left = b.block_left();
            for i in &a[a.len() - 3..] {
                b.feedback(i, Verdict::Rejected);
            }
            assert_eq!(b.block_left(), left, "the block ahead goes on");
            b.block_left = 0;
            b.choose(&[], &[]);
            assert_ne!(b.current, Some(0), "the next block leaves the skipped cluster");

            // The next block of the same cluster again.
            let mut b = bandit();
            b.seed = s;
            let a = played_block(&mut b, 0, 100);
            played_block(&mut b, 0, 200);
            for i in &a[a.len() - 3..] {
                b.feedback(i, Verdict::Rejected);
            }
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
    /// pick grows its cluster's β, and runs no skips.
    #[test]
    fn forgotten_picks_return_to_the_block() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let left = b.block_left();
        b.note_served(Item::Track(tid(1)), k, "");
        b.note_served(Item::Track(tid(2)), k, "");
        b.forget(&[Item::Track(tid(2))]);
        assert_eq!(b.block_left(), left - 1);
        let beta0 = b.arms[k].beta;
        b.removed(&Item::Track(tid(1)));
        assert_eq!(b.arms[k].beta, beta0 + 1.0);
        assert_eq!(b.skips_running(), 0);
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
        b.note_served(Item::Track(tid(1)), 0, "");
        let json = serde_json::to_string(&b).unwrap();
        let back: Bandit = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    /// A wave starts with full blocks (its prior and its session say the
    /// mood); a block left after its rejections loses the mood: blocks of
    /// one or two songs, going on while heard out, until two songs running
    /// are heard out.
    #[test]
    fn short_blocks_until_the_mood_is_found_again() {
        let mut b = bandit();
        assert!(b.choose(&[], &[]));
        assert!((BLOCK_MIN..=BLOCK_MAX).contains(&b.block_left()), "{}", b.block_left());
        let k = b.current.unwrap();
        let bad: Vec<Item> = (0..3).map(|t| Item::Track(tid(400 + t))).collect();
        for i in &bad {
            b.note_served(i.clone(), k, "");
        }
        for i in &bad {
            b.feedback(i, Verdict::Rejected);
        }
        assert!(!b.settled());
        b.choose(&[], &[]);
        assert!((1..=PROBE_MAX).contains(&b.block_left()));
        let k = b.current.unwrap();
        let items: Vec<Item> = (0..2).map(|t| Item::Track(tid(300 + t))).collect();
        for i in &items {
            b.note_served(i.clone(), k, "");
        }
        b.feedback(&items[0], Verdict::Approved);
        assert!(!b.settled());
        assert!(b.block_left() >= 1, "a cluster heard out keeps its block going");
        b.feedback(&items[1], Verdict::Approved);
        assert!(b.settled());
        assert!(b.block_left() >= BLOCK_MIN, "found, the block grows to a full one");
        b.block_left = 0;
        b.choose(&[], &[]);
        assert!((BLOCK_MIN..=BLOCK_MAX).contains(&b.block_left()));
    }

    /// Two new songs rejected in a block halve the next block's share of
    /// new music (not «Только новое»); the block after is whole again; a
    /// library song rejected counts nothing towards it.
    #[test]
    fn rejected_new_songs_halve_the_next_block() {
        let mut b = bandit();
        b.choose(&[], &[]);
        let k = b.current.unwrap();
        let new = |n: &str| Item::Stream(RemoteId::new(nikgob_core::streaming::Service::Youtube, n));
        let items = [new("a"), new("b"), Item::Track(tid(9))];
        for i in &items {
            b.note_served(i.clone(), k, "");
        }
        b.feedback(&items[2], Verdict::Rejected);
        b.feedback(&items[0], Verdict::Rejected);
        b.block_left = 0;
        b.choose(&[], &[]);
        assert_eq!(b.novelty_of(0.6), 0.6, "one new song rejected: whole");
        let k = b.current.unwrap();
        let more = [new("c"), new("d")];
        for i in &more {
            b.note_served(i.clone(), k, "");
        }
        for i in &more {
            b.feedback(i, Verdict::Rejected);
        }
        b.block_left = 0;
        b.choose(&[], &[]);
        assert_eq!(b.novelty_of(0.6), 0.3);
        assert_eq!(b.novelty_of(NOVELTY_ONLY_NEW), NOVELTY_ONLY_NEW);
        b.block_left = 0;
        b.choose(&[], &[]);
        assert_eq!(b.novelty_of(0.6), 0.6, "the block after is whole again");
    }

    /// The memory keeps what each cluster did in each part of the day,
    /// fades by the day, weighs a few songs at most, and keeps its size.
    #[test]
    fn the_memory_fades_and_stays_small() {
        let mut m = BanditMemory::default();
        for _ in 0..4 {
            m.learn("any.p1", 2, 100, true);
        }
        m.learn("any.p1", 2, 100, false);
        assert_eq!(m.prior("any.p1", 2, 100), (4.0, 1.0));
        assert_eq!(m.prior("any.p1", 1, 100), (0.0, 0.0), "another part of the day");
        let (ok, no) = m.prior("any.p1", 2, 102);
        assert!((ok - 4.0 * MEMORY_DECAY * MEMORY_DECAY).abs() < 1e-4 && (no - MEMORY_DECAY * MEMORY_DECAY).abs() < 1e-4);
        for _ in 0..40 {
            m.learn("any.p2", 0, 100, true);
        }
        let (ok, no) = m.prior("any.p2", 0, 100);
        assert!((ok + no - MEMORY_WEIGHT).abs() < 1e-4);
        for k in 0..(MEMORY_CAP + 20) {
            m.learn(&format!("any.p{k}"), 3, 200 + k as i64, true);
        }
        assert_eq!(m.entries.len(), MEMORY_CAP);
        let back: BanditMemory = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    /// Edges: a key on the far side of the circle of fifths costs more
    /// than its neighbour or its relative minor; a loud cold head right
    /// after a quiet tail costs, a fade-in does not.
    #[test]
    fn steps_by_their_edges() {
        use crate::sketch::{Edge, EdgeShape};
        // C major's pitch classes, and rotated to other keys.
        let c: [u8; 12] = [15, 1, 8, 1, 10, 9, 1, 13, 1, 8, 1, 3];
        let key = |shift: usize, loudness: f32, shape: EdgeShape| Edge { loudness, shape, silence: 0.0, chroma: std::array::from_fn(|i| c[(i + 12 - shift) % 12]) };
        let tail = key(0, -12.0, EdgeShape::Fade);
        let same = edge_cost(&tail, &key(0, -12.0, EdgeShape::Cold));
        let fifth = edge_cost(&tail, &key(7, -12.0, EdgeShape::Cold));
        let tritone = edge_cost(&tail, &key(6, -12.0, EdgeShape::Cold));
        assert!(same < 1e-6, "{same}");
        assert!(fifth < tritone, "{fifth} {tritone}");
        let attack = edge_cost(&key(0, -24.0, EdgeShape::Fade), &key(0, -8.0, EdgeShape::Cold));
        let eased = edge_cost(&key(0, -24.0, EdgeShape::Fade), &key(0, -8.0, EdgeShape::Fade));
        assert!(attack > eased + 0.3, "{attack} {eased}");
        assert_eq!(fifths(crate::sketch::Key { root: 9, minor: true }), fifths(crate::sketch::Key { root: 0, minor: false }), "A minor is C major's relative");
    }

    /// The permutations of a batch are all weighed, the first one first.
    #[test]
    fn every_order_of_a_batch() {
        let mut p = vec![0, 1, 2];
        let mut seen = vec![p.clone()];
        while next_permutation(&mut p) {
            seen.push(p.clone());
        }
        assert_eq!(seen.len(), 6);
        assert_eq!(seen[5], vec![2, 1, 0]);
    }

    /// The bandit at a cold start (a report): six clusters, one the mood
    /// of the moment (heard out 4 in 5), the rest 1 in 4, the prior saying
    /// nothing; how many songs until three are heard out running: blocks of
    /// 4 to 6 throughout; short blocks once the mood is lost (as it plays);
    /// short blocks from the start too; as it plays with the memory of the
    /// mood from the day before.
    #[test]
    #[ignore = "a report"]
    fn cold_start_report() {
        let runs = 400u64;
        let mut totals = [0.0f32; 4];
        for (policy, total) in totals.iter_mut().enumerate() {
            for run in 0..runs {
                let arms: Vec<Arm> = (0..6).map(|k| arm(k, 2.0, 2.0)).collect();
                let mut b = Bandit::with_arms(arms, 1 + run);
                b.lost = policy == 2;
                if policy == 3 {
                    // The day before heard the mood out (the memory's songs
                    // on its prior, at most MEMORY_WEIGHT).
                    b.arms[3].alpha += 4.0;
                }
                let mut rng = Rng::new(9_000 + run);
                let (mut steps, mut run_ok, mut t) = (0u32, 0u32, 0u128);
                while run_ok < 3 && steps < 200 {
                    if policy == 0 {
                        b.lost = false;
                    }
                    if b.block_left() == 0 || b.current.is_none() {
                        b.choose(&[], &[]);
                    }
                    let k = b.current.unwrap();
                    let item = Item::Track(tid(10_000 + t));
                    t += 1;
                    b.note_served(item.clone(), k, "");
                    let p = if k == 3 { 0.8 } else { 0.25 };
                    let ok = rng.f32() < p;
                    b.feedback(&item, if ok { Verdict::Approved } else { Verdict::Rejected });
                    run_ok = if ok { run_ok + 1 } else { 0 };
                    steps += 1;
                }
                *total += steps as f32;
            }
        }
        let mean = |x: f32| x / runs as f32;
        eprintln!(
            "songs to three heard out running at a cold start: full blocks {:.1}, short once lost {:.1}, short from the start {:.1}, short once lost with memory {:.1}",
            mean(totals[0]),
            mean(totals[1]),
            mean(totals[2]),
            mean(totals[3])
        );
        assert!(totals[1] <= totals[0]);
    }
}
