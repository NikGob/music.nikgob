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
//!   cluster, none heard out or past its half between, leaves it
//!   ([`LEAVE_UNSETTLED`]; [`LEAVE_SETTLED`] of two artists, the unknown
//!   ones counted as one, or [`LEAVE_UNKNOWN`] of unknown ones alone): each
//!   weighed by what it says (an unfamiliar song [`MISSED_WEIGHT`], a whole
//!   one where the wave plays new music alone; «not now» of a loved one
//!   [`NOT_NOW_WEIGHT`], or nothing once the vibe is settled: the song is
//!   worn, not the vibe, and the next song stays in the cluster, once);
//!   then the block ends at once and the next is of another cluster. A
//!   like approves at once. «↻ Другой вайб»
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

use nikgob_core::streaming::RemoteId;
use serde::{Deserialize, Serialize};

use crate::model::UserModel;
use crate::open_world::{Ahead, Character, OpenCandidate, Scene, WaveContext, WaveFilter, WavePick, WaveTurn, fits_filter, scene_of_new};
use crate::pick::Item;
use crate::profiles::LaneKey;
use crate::rng::Rng;
use crate::session::{Session, Verdict};
use crate::taste::DayPart;

mod sequence;

#[cfg(test)]
mod tests;

pub use sequence::sequence;
#[cfg(test)]
use sequence::{edge_cost, kept_steady};
#[cfg(test)]
use sequence::*;

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
/// …and at this weight, of two artists at least, once it is (the songs of
/// unknown artists one artist, «unknown»)…
pub const LEAVE_SETTLED: f32 = 4.0;
/// …or at this weight when every song of the run is of an unknown artist.
pub const LEAVE_UNKNOWN: f32 = 5.0;
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
    /// The next song is of this cluster, once: «not now» of a worn song in
    /// a settled vibe turns from the song, not the vibe, and holds nothing
    /// longer.
    #[serde(default)]
    stay: Option<usize>,
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

/// Rejections running of one cluster, none heard out or past its half
/// between: their weight, how many, their artists (the unknown as one), and whether it
/// holds the cluster no more (the wave left it, «↻ Другой вайб» did, or a
/// loved song was skipped as worn since).
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
        let slot = match seed.part {
            Some(p) => m.slot().with_part(p),
            None => m.slot(),
        };
        let part = slot.part.index();
        let now = m.now();
        // Per cluster: tracks, how much it is played in the part of the
        // day, love.
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
            e.1 += m.taste.part_mass_at(i)[part];
            e.2 += m.tracks[i].aff_long(now).max(0.0);
        }
        // A cluster that is a taste profile: its share of this part of this
        // weekday ([`crate::context::TimeModel::profile_share`]), so a
        // Saturday morning unlike the other mornings leads its own way.
        for (scene, e) in by.iter_mut() {
            if let Scene::Profile(key) = scene
                && let Some(j) = m.profiles.list.iter().position(|p| p.key == *key)
                && m.time.learned(slot.part) > 0.0
            {
                e.1 = m.profile_slot_share(j, slot);
            }
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
        self.stay = None;
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

    /// The cluster the next pick keeps to: the one held, else the one
    /// «not now» of a worn song stays in for a song.
    fn kept(&self) -> Option<usize> {
        self.held().or(self.stay.filter(|k| *k < self.arms.len() && self.arms.len() > 1))
    }

    /// [`Self::choose`]; `hold`: as [`Self::kept`] says.
    fn draw(&mut self, bonus: &[f32], dry: &[usize], hold: bool) -> bool {
        // While the mood is lost the cluster left stays avoided (short
        // blocks would come back to it at once).
        let avoid = if self.lost { self.avoid } else { self.avoid.take() };
        let mut rng = self.rng();
        let mut best: Option<(f32, usize)> = self.kept().filter(|k| hold && !dry.contains(k)).map(|k| (1.0, k));
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
        // The song that stays in the cluster is picked.
        if self.stay == Some(k) {
            self.stay = None;
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
        // A song of the radio ended: what «not now» kept is over.
        self.stay = None;
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
                // vibe; the cluster is not told, a run before it holds the
                // cluster no more, and the next song stays in it, once.
                if weight <= 0.0 {
                    if let Some(r) = self.run.as_mut() {
                        r.released = true;
                    }
                    self.stay = Some(k);
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
            // Heard past its half (or on after a drag): the rejections
            // running end there (nothing else is told).
            Verdict::Neutral => self.run = None,
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
        self.stay = None;
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
    /// artists (settled; the songs of unknown artists are one artist,
    /// «unknown», and a run of theirs alone is left at [`LEAVE_UNKNOWN`])
    /// the wave leaves the cluster: the block being picked ends if it is of
    /// `k` (the songs rejected are of a block before it, picked ahead
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
        // Unknown artists are one artist (empty).
        if !run.artists.contains(&artist) {
            run.artists.push(artist);
        }
        let enough = if run.artists.iter().all(String::is_empty) { run.weight >= LEAVE_UNKNOWN } else { run.weight >= LEAVE_SETTLED && run.artists.len() >= 2 };
        let leave = self.arms.len() > 1 && if settled { enough } else { run.weight >= LEAVE_UNSETTLED };
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
    /// song heard out or past its half; «not now» in a settled vibe not
    /// counted).
    pub fn skips_running(&self) -> u8 {
        self.run.as_ref().map_or(0, |r| r.count)
    }

    /// Whether «not now» just held the cluster for one recovery song.
    pub fn is_stay(&self) -> bool {
        self.stay.is_some()
    }

    /// The last `max` known artists served by this wave (oldest first).
    pub fn recent_artists(&self, max: usize) -> Vec<String> {
        let from = self.served.len().saturating_sub(max);
        self.served.iter().skip(from).map(|s| s.artist.clone()).filter(|a| !a.is_empty()).collect()
    }

    /// After a skip, whether the queue keeps its `next` pick of the radio
    /// (`None`: the next is none, or the user's own): one skip running keeps
    /// it, unless a cluster is held ([`Self::held`]) and it is of another;
    /// two running replace the whole tail.
    pub fn keep_next(&self, next: Option<&Item>) -> bool {
        self.skips_running() < crate::radio::SKIP_SHIFT && next.is_none_or(|i| self.kept().is_none_or(|h| self.arm_of(i) == Some(h)))
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
        self.stay = None;
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
    let mut recent_artists = bandit.recent_artists(crate::open_world::ARTIST_FATIGUE_WINDOW);
    let after_skip = bandit.skips_running() > 0 || bandit.is_stay() || session.last().is_some_and(|p| Session::verdict(p, m).rejects());
    let push_recent = |recent: &mut Vec<String>, artist: String| {
        if !artist.is_empty() {
            recent.push(artist);
            if recent.len() > crate::open_world::ARTIST_FATIGUE_WINDOW {
                recent.remove(0);
            }
        }
    };
    while out.len() < n {
        // A cluster held by rejections running takes the picks back from a
        // block picked ahead of another.
        let held = bandit.kept().filter(|h| bandit.current != Some(*h) && !dry.contains(h)).is_some();
        if (bandit.current.is_none() || bandit.block_left == 0 || bandit.current.is_some_and(|k| dry.contains(&k)) || held) && !bandit.choose(&bonus, &dry) {
            break;
        }
        let Some(k) = bandit.current else { break };
        let scene = bandit.arms[k].scene;
        let filter = WaveFilter { scene, novelty: bandit.novelty_of(user.novelty), ..user };
        let want = (n - out.len()).min(usize::from(bandit.block_left.max(1)));
        let (c, p) = if scene == user.scene { (ctx, pool) } else { (&within, pool_in) };
        let next = crate::open_world::next_turn(
            c,
            p,
            &filter,
            session,
            want,
            &served,
            WaveTurn {
                queued: &ahead,
                seed: run_seed,
                skipped_new,
                again: &[],
                works,
                recent_artists: &recent_artists,
                after_skip: after_skip && out.is_empty(),
            },
        );
        asks.add(m, p, &filter, &next);
        if next.picks.is_empty() {
            dry.push(k);
            bandit.block_left = 0;
            continue;
        }
        for p in next.picks {
            served.insert(served_key(&p.item, &keys));
            let a = ahead_of(&p.item);
            let label = artist_of(a);
            bandit.note_served(p.item.clone(), k, &label);
            push_recent(&mut recent_artists, label);
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
        let next = crate::open_world::next_turn(
            ctx,
            pool,
            f,
            session,
            n - out.len(),
            &served,
            WaveTurn {
                queued: &ahead,
                seed: run_seed,
                skipped_new,
                again: &[],
                works,
                recent_artists: &recent_artists,
                after_skip: after_skip && out.is_empty(),
            },
        );
        asks.add(m, pool, f, &next);
        for p in next.picks {
            served.insert(served_key(&p.item, &keys));
            let a = ahead_of(&p.item);
            push_recent(&mut recent_artists, artist_of(a));
            ahead.push(a);
            out.push(p);
        }
        if out.len() < n {
            let again = repeats(
                ctx,
                pool,
                f,
                session,
                n - out.len(),
                &served,
                repeatable,
                cool_ms,
                WaveTurn {
                    queued: &ahead,
                    seed: run_seed,
                    skipped_new,
                    again: &[],
                    works,
                    recent_artists: &recent_artists,
                    after_skip: after_skip && out.is_empty(),
                },
            );
            for p in again {
                served.insert(served_key(&p.item, &keys));
                let a = ahead_of(&p.item);
                push_recent(&mut recent_artists, artist_of(a));
                ahead.push(a);
                out.push(p);
            }
        }
        scene_let_go |= stage > 0 && out.len() > before;
    }
    sequence(ctx, user.character, session, queued, &mut out);
    Picked { picks: out, scene_let_go, asks }
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
