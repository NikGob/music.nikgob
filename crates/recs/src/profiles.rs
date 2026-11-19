//! Taste profiles: a profile is the tracks of one
//! kind of song that sound alike, with a mass of taste. Nothing here names
//! a language or a genre of anyone's: the kinds are whatever the library
//! holds, the number of profiles comes from the data.
//!
//! * **Point** of a track ([`Space`]): its embedded sound, plus its kind as
//!   more dimensions in units of the library's median distance `s` — the
//!   voice (sung `+0.6·s`, instrumental `−0.6·s`), the sung language
//!   one-hot (`s/√2` each, any language the detector names) and a
//!   soundtrack's flag (`0.3·s`). A track without its own sound takes its
//!   artist's mean sound (else the library's).
//! * **Mass** (the caller's, [`crate::model`]): its plays' outcomes with a
//!   90-day half-life, a background play at [`crate::listening::W_BG`],
//!   likes and playlists ([`Input::mass`]).
//! * **Kind first** ([`discover_typed`]): the tracks with mass fall in lanes
//!   by their [`LaneKey`] (instrumental, each sung language, unknown). A
//!   lane stands on its own with [`MIN_SHARE`] of the mass and
//!   [`MIN_TRACKS`] tracks, or while it is alive ([`ALIVE_PLAYS`] active
//!   plays in [`ALIVE_DAYS`]: what the user still plays by hand is never
//!   swallowed by what grew), or while it was a profile [`DIES_AFTER_DAYS`]
//!   ago. Smaller lanes join the nearest profile.
//! * **Then sound** ([`discover`]): within a lane a weighted k-means with
//!   room to spare, then merges — a cluster under the share or the tracks
//!   into its nearest, and two neighbours whose split is no better than a
//!   single Gaussian cloud's ([`unimodal_ratio`] under [`SPLIT_RATIO`], a
//!   test of shape, both sides weighed alike). Rap and rock of one language
//!   are two profiles only when their sounds part.
//! * **Every track** gets its profile: by its own lane's nearest centre
//!   (else any), a track of unknown kind without a sound by the majority
//!   of its playlists, else of the plays around it.
//! * **Stable keys** ([`ProfileHistory`]): a profile keeps its key while it
//!   stays the same (within half its radius, or holding most of what it
//!   held: the larger part of a split, the heavier of a merge); a new one
//!   is shown after [`BORN_AFTER`] days of builds in a row.
//! * **Names** ([`Profiles::name`]): the kind at 60 % of the profile (with
//!   a hysteresis of 5 %), a word of loudness against the user's other
//!   profiles of that kind, the main artists in the subtitle; or by the
//!   artists alone ([`NameStyle`]).
//! * **Balance** ([`shares`], [`quota`]): the profiles' shares of the
//!   recent plays, a background play counting [`crate::listening::W_BG`];
//!   the home's cards by share with a floor of one for each active profile,
//!   a background profile at most [`BG_CARDS`] of them, split by energy
//!   ([`background_cards`]).

use std::collections::{HashMap, HashSet};

use nikgob_core::i18n::home_recs::{
    ProfileKind, SungLang, fmt_artists_more_in, fmt_artists_pair_in, fmt_lang_songs_in, loudness_word_in,
};
use nikgob_core::i18n::Locale;
use nikgob_core::model::{ArtistId, TrackId, UnixMs};
use serde::{Deserialize, Serialize};

use crate::analysis::{DIM, Vector};
use crate::songtype::{Lang, SongLane, SongType, Voice};
use crate::taste::DAY_MS;

/// A split must beat one Gaussian cloud's best by this much.
pub const SPLIT_RATIO: f32 = 1.35;
/// A profile carries this share of the mass at least…
pub const MIN_SHARE: f32 = 0.04;
/// …and this many tracks.
pub const MIN_TRACKS: usize = 8;
/// Profiles at most.
pub const K_MAX: usize = 10;
/// A cluster is tested against this many of its nearest.
const NEIGHBOURS: usize = 3;
/// A lane is alive with this many active plays…
pub const ALIVE_PLAYS: f32 = 30.0;
/// …in this many days.
pub const ALIVE_DAYS: i64 = 60;
/// The type's dimensions in units of the median distance.
const VOICE: f32 = 0.6;
const LANG: f32 = 1.0;
const OST: f32 = 0.3;
/// A new profile is shown after builds on this many days in a row…
pub const BORN_AFTER: u32 = 3;
/// …and a lane that was a profile stands this long after it stopped
/// earning it.
pub const DIES_AFTER_DAYS: i64 = 14;
/// A profile keeps its key within this share of its radius…
const KEEP_RADII: f32 = 0.5;
/// …or holding this share of what it held.
const KEEP_OVERLAP: f32 = 0.5;
/// Members a profile's history remembers.
const KEPT_MEMBERS: usize = 200;
/// A kind names a profile at this share of it…
const NAME_SHARE: f32 = 0.6;
/// …and stays its name down to this one.
const NAME_KEEP: f32 = 0.55;
/// A profile is active on the home with this share.
pub const ACTIVE_SHARE: f32 = 0.02;
/// Cards of a background profile on the home at most (the user's choice).
pub const BG_CARDS: usize = 3;
/// The shares' half-life.
pub const SHARE_HALF_DAYS: i64 = 30;
/// The version of [`ProfileHistory`]: another one starts over.
const HISTORY_VERSION: u32 = 1;

/// The kind of song a lane holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LaneKey {
    /// No voice: instrumental, or a soundtrack's track of unknown voice.
    Instrumental,
    Sung(Lang),
    Unknown,
}

impl LaneKey {
    pub fn of(t: &SongType) -> Self {
        if t.lane() == SongLane::Instrumental {
            return Self::Instrumental;
        }
        t.lang.map_or(Self::Unknown, Self::Sung)
    }

    /// The coarse lane of the session's «same lane».
    pub fn song_lane(self) -> SongLane {
        match self {
            Self::Instrumental => SongLane::Instrumental,
            Self::Sung(Lang::Russian) => SongLane::Russian,
            Self::Sung(Lang::Japanese) => SongLane::Japanese,
            Self::Sung(Lang::English | Lang::Spanish | Lang::French | Lang::German) => SongLane::Western,
            _ => SongLane::Unknown,
        }
    }
}

/// The space of the tracks' points (see the module docs).
#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    scale: f32,
    langs: Vec<Lang>,
}

impl Space {
    /// The space of a library whose median distance is `scale` and whose
    /// tracks are of `types`.
    pub fn new(scale: f32, types: &[SongType]) -> Self {
        let mut langs: Vec<Lang> = types.iter().filter(|t| LaneKey::of(t) != LaneKey::Instrumental).filter_map(|t| t.lang).collect();
        langs.sort_unstable();
        langs.dedup();
        Self { scale: scale.max(1e-3), langs }
    }

    pub fn dim(&self) -> usize {
        DIM + 2 + self.langs.len()
    }

    /// The point of a track sounding `sound` of type `t`.
    pub fn point(&self, sound: &Vector, t: &SongType) -> Vec<f32> {
        let lane = LaneKey::of(t);
        let voice = match (lane, t.voice) {
            (LaneKey::Instrumental, _) => -1.0,
            (_, Voice::Sung) => 1.0,
            _ => 0.0,
        };
        let mut p = Vec::with_capacity(self.dim());
        p.extend_from_slice(sound);
        p.push(VOICE * self.scale * voice);
        let lang = if lane == LaneKey::Instrumental { None } else { t.lang };
        p.extend(self.langs.iter().map(|l| if Some(*l) == lang { LANG * self.scale / std::f32::consts::SQRT_2 } else { 0.0 }));
        p.push(if t.ost { OST * self.scale } else { 0.0 });
        p
    }
}

// ------------------------------------------------------------ geometry

fn d2(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// Points of one dimension in a row.
#[derive(Clone, Debug)]
pub struct Points {
    dim: usize,
    data: Vec<f32>,
}

impl Points {
    pub fn new(dim: usize) -> Self {
        Self { dim, data: Vec::new() }
    }

    pub fn push(&mut self, p: &[f32]) {
        debug_assert_eq!(p.len(), self.dim);
        self.data.extend_from_slice(p);
    }

    pub fn row(&self, i: usize) -> &[f32] {
        &self.data[i * self.dim..(i + 1) * self.dim]
    }

    pub fn len(&self) -> usize {
        self.data.len() / self.dim.max(1)
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

fn wmean(pts: &Points, w: &[f32], ids: &[usize]) -> Vec<f32> {
    let mut c = vec![0.0f32; pts.dim];
    let mut tot = 0.0f32;
    for &i in ids {
        tot += w[i];
        for (a, b) in c.iter_mut().zip(pts.row(i)) {
            *a += w[i] * b;
        }
    }
    if tot > 0.0 {
        c.iter_mut().for_each(|x| *x /= tot);
    }
    c
}

fn wsse(pts: &Points, w: &[f32], ids: &[usize], c: &[f32]) -> f32 {
    ids.iter().map(|&i| w[i] * d2(pts.row(i), c)).sum()
}

/// The largest eigenvalue of the weighted covariance of `ids` around `c`
/// over its trace, by power iteration from the farthest point.
fn lambda1_share(pts: &Points, w: &[f32], ids: &[usize], c: &[f32]) -> f32 {
    let dim = pts.dim;
    let tot: f32 = ids.iter().map(|&i| w[i]).sum();
    if tot <= 0.0 {
        return 1.0;
    }
    let xs: Vec<Vec<f32>> = ids.iter().map(|&i| pts.row(i).iter().zip(c).map(|(a, b)| a - b).collect()).collect();
    let tr: f32 = xs.iter().zip(ids).map(|(x, &i)| w[i] * x.iter().map(|v| v * v).sum::<f32>()).sum::<f32>() / tot;
    if tr <= 1e-12 {
        return 1.0;
    }
    let far = (0..xs.len()).max_by(|&a, &b| {
        let (na, nb) = (xs[a].iter().map(|v| v * v).sum::<f32>(), xs[b].iter().map(|v| v * v).sum::<f32>());
        na.total_cmp(&nb).then(b.cmp(&a))
    });
    let Some(far) = far else { return 1.0 };
    let n = xs[far].iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    let mut v: Vec<f32> = xs[far].iter().map(|x| x / n).collect();
    let mut lam = 0.0f32;
    for _ in 0..40 {
        let mut nv = vec![0.0f32; dim];
        for (x, &i) in xs.iter().zip(ids) {
            let proj: f32 = x.iter().zip(&v).map(|(a, b)| a * b).sum();
            for (a, b) in nv.iter_mut().zip(x) {
                *a += w[i] * proj * b;
            }
        }
        nv.iter_mut().for_each(|x| *x /= tot);
        lam = nv.iter().map(|x| x * x).sum::<f32>().sqrt();
        if lam <= 1e-12 {
            break;
        }
        v = nv.into_iter().map(|x| x / lam).collect();
    }
    lam / tr
}

/// How much better the split of `a ∪ b` into `a` and `b` is than the best
/// split of one Gaussian cloud of the same shape: the drop of the squared
/// error over `(2/π)·λ₁/tr`. Scale-free; both sides weigh the same (the
/// test is about shape, the mass has its own rule). Under [`SPLIT_RATIO`]
/// the two are one profile.
pub fn unimodal_ratio(pts: &Points, w: &[f32], a: &[usize], b: &[usize]) -> f32 {
    let (ma, mb): (f32, f32) = (a.iter().map(|&i| w[i]).sum(), b.iter().map(|&i| w[i]).sum());
    if ma <= 0.0 || mb <= 0.0 {
        return 0.0;
    }
    let mut nw = w.to_vec();
    for &i in a {
        nw[i] = w[i] / ma;
    }
    for &i in b {
        nw[i] = w[i] / mb;
    }
    let u: Vec<usize> = a.iter().chain(b).copied().collect();
    let c = wmean(pts, &nw, &u);
    let sse = wsse(pts, &nw, &u, &c);
    if sse <= 0.0 {
        return 0.0;
    }
    let sse2 = wsse(pts, &nw, a, &wmean(pts, &nw, a)) + wsse(pts, &nw, b, &wmean(pts, &nw, b));
    let share = lambda1_share(pts, &nw, &u, &c);
    if share <= 0.0 {
        return 0.0;
    }
    (1.0 - sse2 / sse) / (2.0 / std::f32::consts::PI * share)
}

/// Deterministic farthest-first seeds: the point nearest the weighted
/// mean, then each next the farthest from those taken (distance × √weight,
/// so stray light tracks do not seed).
fn seeds(pts: &Points, w: &[f32], ids: &[usize], k: usize) -> Vec<Vec<f32>> {
    let c = wmean(pts, w, ids);
    let Some(&first) = ids.iter().min_by(|&&a, &&b| d2(pts.row(a), &c).total_cmp(&d2(pts.row(b), &c)).then(a.cmp(&b))) else {
        return Vec::new();
    };
    let mut out = vec![pts.row(first).to_vec()];
    let mut dmin: Vec<f32> = ids.iter().map(|&i| d2(pts.row(i), &out[0])).collect();
    while out.len() < k {
        let far = (0..ids.len()).max_by(|&a, &b| (dmin[a] * w[ids[a]].sqrt()).total_cmp(&(dmin[b] * w[ids[b]].sqrt())).then(b.cmp(&a)));
        let Some(far) = far.filter(|&f| dmin[f] > 1e-12) else { break };
        out.push(pts.row(ids[far]).to_vec());
        let s = out.last().expect("a seed").clone();
        for (d, &i) in dmin.iter_mut().zip(ids) {
            *d = d.min(d2(pts.row(i), &s));
        }
    }
    out
}

fn nearest(p: &[f32], cents: &[Vec<f32>]) -> usize {
    let mut best = (f32::INFINITY, 0);
    for (j, c) in cents.iter().enumerate() {
        let d = d2(p, c);
        if d < best.0 {
            best = (d, j);
        }
    }
    best.1
}

/// A weighted k-means of `ids` from `cents`: the non-empty groups.
fn kmeans(pts: &Points, w: &[f32], ids: &[usize], mut cents: Vec<Vec<f32>>, iters: usize) -> Vec<Vec<usize>> {
    let mut lab = vec![usize::MAX; ids.len()];
    for _ in 0..iters {
        let mut moved = false;
        for (k, &i) in ids.iter().enumerate() {
            let j = nearest(pts.row(i), &cents);
            moved |= lab[k] != j;
            lab[k] = j;
        }
        for (j, c) in cents.iter_mut().enumerate() {
            let m: Vec<usize> = ids.iter().zip(&lab).filter(|(_, l)| **l == j).map(|(i, _)| *i).collect();
            if !m.is_empty() {
                *c = wmean(pts, w, &m);
            }
        }
        if !moved {
            break;
        }
    }
    (0..cents.len())
        .map(|j| ids.iter().zip(&lab).filter(|(_, l)| **l == j).map(|(i, _)| *i).collect::<Vec<usize>>())
        .filter(|g| !g.is_empty())
        .collect()
}

/// How [`discover`] splits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub split_ratio: f32,
    pub min_share: f32,
    pub min_tracks: usize,
    pub k_max: usize,
    /// Split by sound at all («Делить профили по звуку»); off, a lane is
    /// one profile.
    pub split: bool,
}

impl Default for Params {
    fn default() -> Self {
        Self { split_ratio: SPLIT_RATIO, min_share: MIN_SHARE, min_tracks: MIN_TRACKS, k_max: K_MAX, split: true }
    }
}

/// The profiles of `ids` (points `pts`, masses `w > 0`) by sound: the
/// groups (see the module docs).
pub fn discover(pts: &Points, w: &[f32], ids: &[usize], p: &Params) -> Vec<Vec<usize>> {
    if ids.is_empty() {
        return Vec::new();
    }
    let total: f32 = ids.iter().map(|&i| w[i]).sum();
    let k0 = if p.split { (ids.len() / p.min_tracks).clamp(1, p.k_max + 4) } else { 1 };
    let mut groups = kmeans(pts, w, ids, seeds(pts, w, ids, k0), 25);
    // Each group has an id, for the tests already made.
    let mut gid: Vec<u32> = (0..groups.len() as u32).collect();
    let mut next_id = groups.len() as u32;
    let mut ratios: HashMap<(u32, u32), f32> = HashMap::new();
    let mass = |g: &[usize]| g.iter().map(|&i| w[i]).sum::<f32>();
    while groups.len() > 1 {
        let cs: Vec<Vec<f32>> = groups.iter().map(|g| wmean(pts, w, g)).collect();
        let nearest_other = |i: usize| (0..groups.len()).filter(|&j| j != i).min_by(|&a, &b| d2(&cs[i], &cs[a]).total_cmp(&d2(&cs[i], &cs[b])).then(a.cmp(&b)));
        let small = (0..groups.len())
            .filter(|&i| mass(&groups[i]) < p.min_share * total || groups[i].len() < p.min_tracks)
            .min_by(|&a, &b| mass(&groups[a]).total_cmp(&mass(&groups[b])).then(a.cmp(&b)));
        let (i, j) = if let Some(i) = small {
            (i, nearest_other(i).expect("two groups"))
        } else {
            let mut near: Vec<(usize, usize)> = Vec::new();
            for i in 0..groups.len() {
                let mut by: Vec<usize> = (0..groups.len()).filter(|&j| j != i).collect();
                by.sort_by(|&a, &b| d2(&cs[i], &cs[a]).total_cmp(&d2(&cs[i], &cs[b])).then(a.cmp(&b)));
                for &j in by.iter().take(NEIGHBOURS) {
                    let pair = (i.min(j), i.max(j));
                    if !near.contains(&pair) {
                        near.push(pair);
                    }
                }
            }
            let mut best: Option<(f32, usize, usize)> = None;
            for (i, j) in near {
                let key = (gid[i].min(gid[j]), gid[i].max(gid[j]));
                let r = *ratios.entry(key).or_insert_with(|| unimodal_ratio(pts, w, &groups[i], &groups[j]));
                if best.is_none_or(|b| r < b.0) {
                    best = Some((r, i, j));
                }
            }
            match best {
                Some((r, i, j)) if r < p.split_ratio => (i, j),
                _ if groups.len() <= p.k_max => break,
                _ => {
                    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
                    for i in 0..groups.len() {
                        for j in i + 1..groups.len() {
                            pairs.push((d2(&cs[i], &cs[j]), i, j));
                        }
                    }
                    let (_, i, j) = pairs.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).expect("pairs");
                    (i, j)
                }
            }
        };
        let (i, j) = (i.min(j), i.max(j));
        let moved = groups.remove(j);
        groups[i].extend(moved);
        gid.remove(j);
        gid[i] = next_id;
        next_id += 1;
    }
    let cents: Vec<Vec<f32>> = groups.iter().map(|g| wmean(pts, w, g)).collect();
    kmeans(pts, w, ids, cents, 10)
}

/// What [`discover_typed`] found: the groups of tracks and each one's lane.
#[derive(Clone, Debug, Default)]
pub struct Found {
    pub groups: Vec<Vec<usize>>,
    pub lanes: Vec<LaneKey>,
    /// The lanes that earned a profile of their own (by mass or by being
    /// alive), not only by having been one.
    pub earned: HashSet<LaneKey>,
}

/// Kind first, then sound (see the module docs): the profiles of `ids`
/// (points `pts`, masses `w > 0`, lanes `lane`, active plays `alive`);
/// `kept` the lanes that were profiles lately and stand while they have
/// the tracks.
pub fn discover_typed(
    pts: &Points,
    w: &[f32],
    ids: &[usize],
    lane: &dyn Fn(usize) -> LaneKey,
    alive: &dyn Fn(usize) -> f32,
    kept: &HashSet<LaneKey>,
    p: &Params,
) -> Found {
    let total: f32 = ids.iter().map(|&i| w[i]).sum::<f32>().max(1e-9);
    let mut by: Vec<(LaneKey, Vec<usize>)> = Vec::new();
    for &i in ids {
        let l = lane(i);
        match by.iter_mut().find(|g| g.0 == l) {
            Some(g) => g.1.push(i),
            None => by.push((l, vec![i])),
        }
    }
    let mass = |g: &[usize]| g.iter().map(|&i| w[i]).sum::<f32>();
    by.sort_by(|a, b| mass(&b.1).total_cmp(&mass(&a.1)).then(a.0.cmp(&b.0)));
    let mut out = Found::default();
    let mut rest: Vec<usize> = Vec::new();
    for (l, mem) in by {
        let m = mass(&mem);
        let lively = mem.len() >= 2 * p.min_tracks && mem.iter().map(|&i| alive(i)).sum::<f32>() >= ALIVE_PLAYS;
        let earned = m >= p.min_share * total || lively;
        if mem.len() < p.min_tracks || !(earned || kept.contains(&l)) {
            rest.extend(mem);
            continue;
        }
        if earned {
            out.earned.insert(l);
        }
        let inner = Params { min_share: p.min_share * total / m, ..*p };
        for g in discover(pts, w, &mem, &inner) {
            out.groups.push(g);
            out.lanes.push(l);
        }
    }
    if out.groups.is_empty() {
        // Nothing stands alone: one profile of everything (by sound).
        for g in discover(pts, w, ids, p) {
            let mut n: HashMap<LaneKey, f32> = HashMap::new();
            for &i in &g {
                *n.entry(lane(i)).or_default() += w[i];
            }
            let l = n.into_iter().max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0))).map_or(LaneKey::Unknown, |x| x.0);
            out.groups.push(g);
            out.lanes.push(l);
        }
        return out;
    }
    let cents: Vec<Vec<f32>> = out.groups.iter().map(|g| wmean(pts, w, g)).collect();
    for i in rest {
        out.groups[nearest(pts.row(i), &cents)].push(i);
    }
    // At most k_max: the two nearest centres merge (the lane of the heavier).
    while out.groups.len() > p.k_max {
        let cs: Vec<Vec<f32>> = out.groups.iter().map(|g| wmean(pts, w, g)).collect();
        let mut best = (f32::INFINITY, 0, 1);
        for i in 0..cs.len() {
            for j in i + 1..cs.len() {
                let d = d2(&cs[i], &cs[j]);
                if d < best.0 {
                    best = (d, i, j);
                }
            }
        }
        let (_, i, j) = best;
        if mass(&out.groups[j]) > mass(&out.groups[i]) {
            out.lanes[i] = out.lanes[j];
        }
        let moved = out.groups.remove(j);
        out.lanes.remove(j);
        out.groups[i].extend(moved);
    }
    out
}

// ------------------------------------------------------------ names

/// What a profile is called by: its kind of song, or its artists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NameStyle {
    /// «Японские песни», the artists below (the default).
    #[default]
    Kind,
    /// «Eve, Radwimps и другие».
    Artists,
}

/// The kind a profile is named by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Instrumental,
    InstrumentalSoundtracks,
    Sung(Lang),
    /// Sung in a language the app has no word for (or none it told).
    Songs,
    Soundtracks,
    Calm,
    Lively,
}

/// Loudness disambiguator when two or more taste profiles share a [`Kind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Loudness {
    Quiet,
    Loud,
}

impl Loudness {
    /// Localized loudness word appended to a profile kind (`"потише"` / `"quieter"`).
    pub const fn word_in(self, loc: Locale) -> &'static str {
        loudness_word_in(loc, self == Self::Quiet)
    }
}

impl Kind {
    /// Localized name of `self` in `loc`.
    pub fn title_in(self, loc: Locale) -> String {
        let kind = match self {
            Self::Instrumental => ProfileKind::Instrumental,
            Self::InstrumentalSoundtracks => ProfileKind::InstrumentalSoundtracks,
            Self::Songs => ProfileKind::Songs,
            Self::Soundtracks => ProfileKind::Soundtracks,
            Self::Calm => ProfileKind::Calm,
            Self::Lively => ProfileKind::Lively,
            Self::Sung(l) => match lang_adjective_in(l, loc) {
                Some(adjective) => return fmt_lang_songs_in(loc, adjective),
                None => ProfileKind::Songs,
            },
        };
        kind.title_in(loc).to_owned()
    }
}

/// The adjective of a language's songs in `loc`, when the language has one.
pub const fn lang_adjective_in(l: Lang, loc: Locale) -> Option<&'static str> {
    let lang = match l {
        Lang::Russian => Some(SungLang::Russian),
        Lang::Japanese => Some(SungLang::Japanese),
        Lang::Korean => Some(SungLang::Korean),
        Lang::English => Some(SungLang::English),
        Lang::Spanish => Some(SungLang::Spanish),
        Lang::French => Some(SungLang::French),
        Lang::German => Some(SungLang::German),
        Lang::Other => None,
    };
    match lang {
        Some(lang) => Some(lang.adjective_in(loc)),
        None => None,
    }
}

/// What a profile is made of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Shares of its tracks: without words, of a soundtrack.
    pub instrumental: f32,
    pub soundtrack: f32,
    /// The share of its tracks with words that is sung (not unknown).
    pub sung: f32,
    /// Its tracks with words by language, the largest first.
    pub langs: Vec<(Lang, f32)>,
    /// The mean percentile of its tracks' energy in the library.
    pub energy: f32,
    /// Its artists by mass, the heaviest first (five at most).
    pub artists: Vec<ArtistId>,
}

impl Stats {
    fn holds(&self, k: Kind, at: f32) -> bool {
        let lang = |l: Lang| self.langs.iter().find(|x| x.0 == l).map_or(0.0, |x| x.1);
        match k {
            Kind::InstrumentalSoundtracks => self.instrumental >= at && self.soundtrack >= at,
            Kind::Instrumental => self.instrumental >= at,
            Kind::Sung(l) => self.instrumental < NAME_SHARE && lang(l) >= at,
            Kind::Songs => self.instrumental < NAME_SHARE && self.sung >= at,
            Kind::Soundtracks => self.soundtrack >= at,
            Kind::Calm => self.energy < 0.5,
            Kind::Lively => self.energy >= 0.5,
        }
    }

    /// The kind that names it: the one before while it still holds at
    /// [`NAME_KEEP`], else the first that holds at [`NAME_SHARE`].
    pub fn kind(&self, before: Option<Kind>) -> Kind {
        if let Some(k) = before.filter(|k| self.holds(*k, NAME_KEEP)) {
            return k;
        }
        let at = NAME_SHARE;
        if self.instrumental >= at {
            return if self.soundtrack >= at { Kind::InstrumentalSoundtracks } else { Kind::Instrumental };
        }
        if let Some(&(l, s)) = self.langs.first()
            && s >= at
            && lang_adjective_in(l, Locale::Ru).is_some()
        {
            return Kind::Sung(l);
        }
        if self.sung >= at || self.langs.first().is_some_and(|x| x.1 >= at) {
            return Kind::Songs;
        }
        if self.soundtrack >= at {
            return Kind::Soundtracks;
        }
        if self.energy < 0.5 { Kind::Calm } else { Kind::Lively }
    }
}

// ------------------------------------------------------------ history

/// One profile as the history keeps it between builds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KeptProfile {
    pub key: u32,
    pub lane: LaneKey,
    /// Its centre in the space of its build, and the typical distance of
    /// a member to it.
    pub centre: Vec<f32>,
    pub radius: f32,
    pub mass: f32,
    /// Its heaviest members.
    pub members: Vec<TrackId>,
    /// The day (local) it was last built, and how many days in a row.
    pub day: i64,
    pub days: u32,
    pub visible: bool,
    /// The last day its lane earned it (see [`Found::earned`]).
    pub earned_day: i64,
    pub kind: Option<Kind>,
}

/// The profiles of the builds before, for stable keys and names.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileHistory {
    pub version: u32,
    pub next_key: u32,
    /// The profiles of the last build, and those gone within
    /// [`DIES_AFTER_DAYS`] (they may come back with their key).
    pub kept: Vec<KeptProfile>,
}

impl ProfileHistory {
    /// The lanes that were a shown profile and earned it within
    /// [`DIES_AFTER_DAYS`] of `day`.
    fn kept_lanes(&self, day: i64) -> HashSet<LaneKey> {
        self.kept.iter().filter(|k| k.visible && day - k.earned_day < DIES_AFTER_DAYS).map(|k| k.lane).collect()
    }
}

// ------------------------------------------------------------ profiles

/// The user's settings of the profiles and the history of the builds
/// before.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// «Фон меньше влияет на рекомендации»: a background play counts
    /// [`crate::listening::W_BG`] (off: fully).
    pub background_less: bool,
    /// «Делить профили по звуку» ([`Params::split`]).
    pub split_by_sound: bool,
    pub history: Option<ProfileHistory>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { background_less: true, split_by_sound: true, history: None }
    }
}

/// What [`Profiles::build`] reads, by catalog position.
pub struct Input<'a> {
    pub ids: &'a [TrackId],
    /// The embedded sound of each track (none: not analysed).
    pub sound: &'a [Option<Vector>],
    pub types: &'a [SongType],
    pub artists: &'a [Option<ArtistId>],
    /// The taste mass of each track (≤ 0: not a member).
    pub mass: &'a [f32],
    /// The active plays of each track in the last [`ALIVE_DAYS`].
    pub alive: &'a [f32],
    /// The plays in order of time, by catalog position (the neighbours of
    /// a track of unknown kind).
    pub plays: &'a [(UnixMs, usize)],
    /// The playlists' tracks, by catalog position.
    pub playlists: &'a [Vec<usize>],
    /// The library's median distance.
    pub scale: f32,
    pub params: Params,
    pub history: Option<&'a ProfileHistory>,
    /// The local day of the build.
    pub day: i64,
}

/// One taste profile.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    /// Stable between builds while it is the same ([`ProfileHistory`]).
    pub key: u32,
    pub lane: LaneKey,
    /// The mean sound of its members (embedded).
    pub sound: Vector,
    /// Its tracks with mass, and their mass.
    pub members: usize,
    pub mass: f32,
    /// Shown in the app (a new one after [`BORN_AFTER`] days).
    pub visible: bool,
    pub stats: Stats,
    pub kind: Kind,
}

/// The user's taste profiles (see the module docs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profiles {
    pub list: Vec<Profile>,
    /// The profile of each catalog track.
    of: Vec<Option<u16>>,
    space: Option<Space>,
    centres: Vec<Vec<f32>>,
    /// The membership's temperature: the median squared distance of a
    /// member to its centre.
    tau: f32,
    /// What the next build goes on from.
    pub history: ProfileHistory,
}

impl Profiles {
    pub fn build(input: &Input<'_>) -> Self {
        let n = input.ids.len();
        let space = Space::new(input.scale, input.types);
        // The sound of a track without its own: its artist's mean, else
        // the library's.
        let mut by_artist: HashMap<ArtistId, (Vector, f32)> = HashMap::new();
        let mut all = ([0.0f32; DIM], 0.0f32);
        for i in 0..n {
            let Some(v) = &input.sound[i] else { continue };
            for k in 0..DIM {
                all.0[k] += v[k];
            }
            all.1 += 1.0;
            if let Some(a) = input.artists[i] {
                let e = by_artist.entry(a).or_insert(([0.0; DIM], 0.0));
                for k in 0..DIM {
                    e.0[k] += v[k];
                }
                e.1 += 1.0;
            }
        }
        let mean_of = |(s, c): &(Vector, f32)| -> Vector { if *c > 0.0 { s.map(|x| x / c) } else { [0.0; DIM] } };
        let library_mean = mean_of(&all);
        let sound_of = |i: usize| -> Vector {
            input.sound[i].unwrap_or_else(|| input.artists[i].and_then(|a| by_artist.get(&a)).map_or(library_mean, mean_of))
        };
        let mut pts = Points::new(space.dim());
        for i in 0..n {
            pts.push(&space.point(&sound_of(i), &input.types[i]));
        }
        let w: Vec<f32> = input.mass.iter().map(|m| m.max(0.0)).collect();
        // Members: the tracks with mass, those with their own sound when
        // there are enough of them (else a streamer's: by artist and kind).
        let with_mass: Vec<usize> = (0..n).filter(|&i| w[i] > 0.0).collect();
        let own: Vec<usize> = with_mass.iter().copied().filter(|&i| input.sound[i].is_some()).collect();
        let ids = if own.len() >= 3 * input.params.min_tracks { own } else { with_mass };
        let lane = |i: usize| LaneKey::of(&input.types[i]);
        let kept = input.history.map(|h| h.kept_lanes(input.day)).unwrap_or_default();
        let found = discover_typed(&pts, &w, &ids, &lane, &|i| input.alive[i], &kept, &input.params);
        let k = found.groups.len();
        let centres: Vec<Vec<f32>> = found.groups.iter().map(|g| wmean(&pts, &w, g)).collect();
        let mut of: Vec<Option<u16>> = vec![None; n];
        for (j, g) in found.groups.iter().enumerate() {
            for &i in g {
                of[i] = Some(j as u16);
            }
        }
        let mut d: Vec<f32> = found.groups.iter().enumerate().flat_map(|(j, g)| g.iter().map(|&i| d2(pts.row(i), &centres[j])).collect::<Vec<_>>()).collect();
        d.sort_by(f32::total_cmp);
        let tau = d.get(d.len() / 2).copied().filter(|t| *t > 1e-6).unwrap_or(1.0);
        if k > 0 {
            // Every other track: the nearest of its lane's profiles, else of all.
            let by_lane: HashMap<LaneKey, Vec<usize>> = found.lanes.iter().enumerate().fold(HashMap::new(), |mut m, (j, l)| {
                m.entry(*l).or_insert_with(Vec::new).push(j);
                m
            });
            for i in 0..n {
                if of[i].is_some() {
                    continue;
                }
                let p = pts.row(i);
                let pool: Vec<usize> = by_lane.get(&lane(i)).cloned().unwrap_or_else(|| (0..k).collect());
                let j = pool.into_iter().min_by(|&a, &b| d2(p, &centres[a]).total_cmp(&d2(p, &centres[b])).then(a.cmp(&b)));
                of[i] = j.map(|j| j as u16);
            }
            // A track of unknown kind without a sound: the majority of its
            // playlists, else of the plays around it.
            let blind: HashSet<usize> =
                (0..n).filter(|&i| input.sound[i].is_none() && lane(i) == LaneKey::Unknown && input.types[i].voice == Voice::Unknown).collect();
            if !blind.is_empty() {
                let mut votes: HashMap<usize, HashMap<u16, f32>> = HashMap::new();
                for list in input.playlists {
                    let mut tally: HashMap<u16, f32> = HashMap::new();
                    for &t in list.iter().filter(|t| !blind.contains(t)) {
                        if let Some(j) = of[t] {
                            *tally.entry(j).or_default() += 1.0;
                        }
                    }
                    let Some((&top, _)) = tally.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))) else { continue };
                    for &t in list.iter().filter(|t| blind.contains(t)) {
                        *votes.entry(t).or_default().entry(top).or_default() += 1.0;
                    }
                }
                let decided: HashSet<usize> = votes.keys().copied().collect();
                let plays = input.plays;
                for (k, &(ts, t)) in plays.iter().enumerate() {
                    if !blind.contains(&t) || decided.contains(&t) {
                        continue;
                    }
                    let near = plays[k.saturating_sub(3)..(k + 4).min(plays.len())]
                        .iter()
                        .filter(|(at, u)| (at - ts).abs() < crate::taste::SESSION_GAP_MS * 3 && !blind.contains(u));
                    for (_, u) in near {
                        if let Some(j) = of[*u] {
                            *votes.entry(t).or_default().entry(j).or_default() += 0.01;
                        }
                    }
                }
                for (t, v) in votes {
                    if let Some((&j, _)) = v.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))) {
                        of[t] = Some(j);
                    }
                }
            }
        }
        // What each profile is made of.
        let mut list: Vec<Profile> = Vec::with_capacity(k);
        for (j, g) in found.groups.iter().enumerate() {
            let mass: f32 = g.iter().map(|&i| w[i]).sum();
            let cnt = g.len().max(1) as f32;
            let inst = g.iter().filter(|&&i| lane(i) == LaneKey::Instrumental).count() as f32;
            let ost = g.iter().filter(|&&i| input.types[i].ost).count() as f32;
            let worded: Vec<usize> = g.iter().copied().filter(|&i| lane(i) != LaneKey::Instrumental).collect();
            let mut langs: HashMap<Lang, f32> = HashMap::new();
            for &i in &worded {
                if let Some(l) = input.types[i].lang {
                    *langs.entry(l).or_default() += 1.0;
                }
            }
            let nw = worded.len().max(1) as f32;
            let mut langs: Vec<(Lang, f32)> = langs.into_iter().map(|(l, c)| (l, c / nw)).collect();
            langs.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let sung = worded.iter().filter(|&&i| input.types[i].voice == Voice::Sung || input.types[i].lang.is_some()).count() as f32 / nw;
            let mut am: HashMap<ArtistId, f32> = HashMap::new();
            for &i in g {
                if let Some(a) = input.artists[i] {
                    *am.entry(a).or_default() += w[i];
                }
            }
            let mut artists: Vec<(ArtistId, f32)> = am.into_iter().collect();
            artists.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let with_sound: Vec<usize> = g.iter().copied().filter(|&i| input.sound[i].is_some()).collect();
            let sound: Vector = if with_sound.is_empty() { std::array::from_fn(|k| centres[j][k]) } else {
                let mut s = [0.0f32; DIM];
                let mut tot = 0.0;
                for &i in &with_sound {
                    let v = input.sound[i].expect("a sound");
                    for k in 0..DIM {
                        s[k] += w[i] * v[k];
                    }
                    tot += w[i];
                }
                s.map(|x| x / tot.max(1e-9))
            };
            let stats = Stats {
                instrumental: inst / cnt,
                soundtrack: ost / cnt,
                sung,
                langs,
                energy: g.iter().map(|&i| input.types[i].energy_q).sum::<f32>() / cnt,
                artists: artists.into_iter().take(5).map(|a| a.0).collect(),
            };
            list.push(Profile { key: 0, lane: found.lanes[j], sound, members: g.len(), mass, visible: true, kind: stats.kind(None), stats });
        }
        let mut out = Self { list, of, space: Some(space), centres, tau, history: ProfileHistory::default() };
        out.evolve(input, &found, &pts, &w);
        out
    }

    /// Keys, birth and death against the history (see the module docs),
    /// and the history the next build goes on from.
    fn evolve(&mut self, input: &Input<'_>, found: &Found, pts: &Points, w: &[f32]) {
        let day = input.day;
        let fresh = input.history.filter(|h| h.version == HISTORY_VERSION);
        let first_build = fresh.is_none_or(|h| h.kept.is_empty());
        let mut next_key = fresh.map_or(1, |h| h.next_key.max(1));
        let old: Vec<KeptProfile> = fresh.map(|h| h.kept.clone()).unwrap_or_default();
        let members: Vec<HashSet<TrackId>> = found.groups.iter().map(|g| g.iter().map(|&i| input.ids[i]).collect()).collect();
        let radius: Vec<f32> = found
            .groups
            .iter()
            .enumerate()
            .map(|(j, g)| {
                let mut d: Vec<f32> = g.iter().map(|&i| d2(pts.row(i), &self.centres[j])).collect();
                d.sort_by(f32::total_cmp);
                d.get(d.len() / 2).copied().unwrap_or(0.0).sqrt()
            })
            .collect();
        // Every pair of a new profile and an old one of its lane that may
        // be the same, the surest first.
        let mut pairs: Vec<(f32, f32, f32, usize, usize)> = Vec::new();
        for (j, p) in self.list.iter().enumerate() {
            for (o, k) in old.iter().enumerate() {
                if k.lane != p.lane {
                    continue;
                }
                let overlap = if k.members.is_empty() { 0.0 } else { k.members.iter().filter(|t| members[j].contains(t)).count() as f32 / k.members.len() as f32 };
                let dist = if k.centre.len() == self.centres[j].len() { d2(&k.centre, &self.centres[j]).sqrt() } else { f32::INFINITY };
                if overlap >= KEEP_OVERLAP || dist < KEEP_RADII * k.radius {
                    pairs.push((overlap, k.mass, dist, j, o));
                }
            }
        }
        pairs.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.total_cmp(&a.1)).then(a.2.total_cmp(&b.2)).then(a.3.cmp(&b.3)).then(a.4.cmp(&b.4)));
        let mut taken_new = vec![None; self.list.len()];
        let mut taken_old = vec![false; old.len()];
        for (_, _, _, j, o) in pairs {
            if taken_new[j].is_none() && !taken_old[o] {
                taken_new[j] = Some(o);
                taken_old[o] = true;
            }
        }
        let mut kept = Vec::new();
        for (j, p) in self.list.iter_mut().enumerate() {
            let before = taken_new[j].map(|o| &old[o]);
            p.key = before.map_or_else(
                || {
                    next_key += 1;
                    next_key - 1
                },
                |k| k.key,
            );
            let days = match before {
                Some(k) if k.day == day => k.days,
                Some(k) if k.day == day - 1 => k.days + 1,
                _ => 1,
            };
            p.visible = first_build || before.is_some_and(|k| k.visible) || days >= BORN_AFTER;
            p.kind = p.stats.kind(before.and_then(|k| k.kind));
            let mut heavy: Vec<usize> = found.groups[j].clone();
            heavy.sort_by(|a, b| w[*b].total_cmp(&w[*a]).then(a.cmp(b)));
            let earned = found.earned.contains(&p.lane);
            kept.push(KeptProfile {
                key: p.key,
                lane: p.lane,
                centre: self.centres[j].clone(),
                radius: radius[j],
                mass: p.mass,
                members: heavy.iter().take(KEPT_MEMBERS).map(|&i| input.ids[i]).collect(),
                day,
                days,
                visible: p.visible,
                earned_day: if earned { day } else { before.map_or(day, |k| k.earned_day) },
                kind: Some(p.kind),
            });
        }
        // The ones gone stay a while: back within it, they keep their key.
        for (o, k) in old.into_iter().enumerate() {
            if !taken_old[o] && day - k.day < DIES_AFTER_DAYS {
                kept.push(k);
            }
        }
        self.history = ProfileHistory { version: HISTORY_VERSION, next_key, kept };
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// The profile of the catalog track at `i`.
    pub fn of(&self, i: usize) -> Option<usize> {
        self.of.get(i).copied().flatten().map(usize::from)
    }

    /// Every catalog track's profile.
    pub fn all_of(&self) -> &[Option<u16>] {
        &self.of
    }

    /// The soft membership of a track sounding `sound` of type `t` in each
    /// profile (summing to 1).
    pub fn membership(&self, sound: &Vector, t: &SongType) -> Vec<f32> {
        let Some(space) = &self.space else { return Vec::new() };
        let p = space.point(sound, t);
        let logits: Vec<f32> = self.centres.iter().map(|c| -d2(&p, c) / self.tau).collect();
        let top = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let e: Vec<f32> = logits.iter().map(|l| (l - top).exp()).collect();
        let z: f32 = e.iter().sum();
        e.into_iter().map(|x| x / z).collect()
    }

    /// How far apart the centres of profiles `a` and `b` are.
    pub fn distance(&self, a: usize, b: usize) -> f32 {
        match (self.centres.get(a), self.centres.get(b)) {
            (Some(x), Some(y)) => d2(x, y).sqrt(),
            _ => f32::INFINITY,
        }
    }

    /// The regions of sound of the profiles ([`crate::regions`]): their
    /// sounds, the coarse lanes and the temperature of the sound alone.
    pub fn regions(&self, sound: &[Option<Vector>]) -> crate::regions::Regions {
        let centres: Vec<Vector> = self.list.iter().map(|p| p.sound).collect();
        let lanes = self.list.iter().map(|p| Some(p.lane.song_lane())).collect();
        let mut d: Vec<f32> = (0..self.of.len())
            .filter_map(|i| Some(crate::index::euclid(sound.get(i)?.as_ref()?, &centres[self.of(i)?]).powi(2)))
            .collect();
        d.sort_by(f32::total_cmp);
        let tau = d.get(d.len() / 2).copied().filter(|t| *t > 1e-4).unwrap_or(1.0);
        crate::regions::Regions::from_parts(centres, lanes, tau)
    }

    /// Loudness disambiguator for profile `j` among all profiles of the same [`Kind`].
    pub fn kind_loudness(&self, j: usize) -> Option<Loudness> {
        let p = &self.list[j];
        let same: Vec<usize> = (0..self.list.len()).filter(|&o| self.list[o].kind == p.kind).collect();
        if same.len() < 2 {
            return None;
        }
        let mut by = same;
        by.sort_by(|a, b| self.list[*a].stats.energy.total_cmp(&self.list[*b].stats.energy).then(a.cmp(b)));
        let r = by.iter().position(|o| *o == j).unwrap_or(0);
        if r == 0 {
            Some(Loudness::Quiet)
        } else if r == by.len() - 1 {
            Some(Loudness::Loud)
        } else {
            None
        }
    }

    /// The name of profile `j` and its subtitle in `loc`, by `style`; `artist`
    /// names an artist. A word of loudness tells apart profiles of one
    /// kind, the main artist what is still alike.
    pub fn name_in(&self, j: usize, style: NameStyle, loc: Locale, artist: &dyn Fn(ArtistId) -> Option<String>) -> (String, String) {
        let names: Vec<String> = self.list[j].stats.artists.iter().filter_map(|a| artist(*a)).collect();
        if style == NameStyle::Artists && !names.is_empty() {
            let title = match names.len() {
                1 => names[0].clone(),
                2 => fmt_artists_pair_in(loc, &names[0], &names[1]),
                _ => fmt_artists_more_in(loc, &names[0], &names[1]),
            };
            return (title, names.iter().skip(2).take(3).cloned().collect::<Vec<_>>().join(", "));
        }
        let base = |idx: usize| -> String {
            let p = &self.list[idx];
            match self.kind_loudness(idx) {
                Some(loud) => format!("{}, {}", p.kind.title_in(loc), loud.word_in(loc)),
                None => p.kind.title_in(loc),
            }
        };
        let mut title = base(j);
        let twin = (0..self.list.len()).any(|o| o != j && base(o) == title);
        if twin && let Some(a) = names.first() {
            title = format!("{title} · {a}");
        }
        (title, names.iter().take(3).cloned().collect::<Vec<_>>().join(", "))
    }
}

// ------------------------------------------------------------ balance

/// One play for the shares: when, its profile, its session's `p_bg` and
/// its weight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharePlay {
    pub ts: UnixMs,
    pub profile: Option<usize>,
    pub p_bg: f32,
    pub weight: f32,
}

/// The shares of `k` profiles in `plays` at `now`: a
/// background play counts `1 − (1 − w_bg)·p_bg`, each play halving in
/// `half_days` (none: all time).
pub fn shares(plays: &[SharePlay], k: usize, w_bg: f32, half_days: Option<i64>, now: UnixMs) -> Vec<f32> {
    let mut acc = vec![0.0f32; k];
    for p in plays {
        let Some(j) = p.profile.filter(|j| *j < k) else { continue };
        let rec = half_days.map_or(1.0, |h| (-((now - p.ts).max(0) as f64) / (h * DAY_MS) as f64).exp2() as f32);
        acc[j] += p.weight * rec * (1.0 - (1.0 - w_bg) * p.p_bg);
    }
    let tot: f32 = acc.iter().sum();
    if tot > 0.0 { acc.iter().map(|a| a / tot).collect() } else { vec![0.0; k] }
}

/// The mean `p_bg` of each profile's plays (none: 0).
pub fn background_shares(plays: &[SharePlay], k: usize) -> Vec<f32> {
    let mut sum = vec![(0.0f32, 0.0f32); k];
    for p in plays {
        if let Some(j) = p.profile.filter(|j| *j < k) {
            sum[j].0 += p.p_bg;
            sum[j].1 += 1.0;
        }
    }
    sum.into_iter().map(|(s, n)| if n > 0.0 { s / n } else { 0.0 }).collect()
}

/// The home's cards of each profile, a calibrated allocation with a floor: each active profile
/// (`shares ≥` [`ACTIVE_SHARE`]) one card while the `slots` allow, the
/// largest shares first; the rest by share, largest remainders, a
/// background profile (`background`) at most `bg_slots`.
pub fn quota(shares: &[f32], slots: usize, background: &[bool], bg_slots: usize) -> Vec<usize> {
    let mut out = vec![0usize; shares.len()];
    let mut active: Vec<usize> = (0..shares.len()).filter(|&j| shares[j] >= ACTIVE_SHARE).collect();
    active.sort_by(|a, b| shares[*b].total_cmp(&shares[*a]).then(a.cmp(b)));
    if active.is_empty() {
        return out;
    }
    for &j in active.iter().take(slots) {
        out[j] = 1;
    }
    let cap = |j: usize| if background.get(j).copied().unwrap_or(false) { bg_slots.max(1) } else { usize::MAX };
    let mut left = slots.saturating_sub(out.iter().sum());
    while left > 0 {
        // The cards of the capped are theirs; the others share the rest.
        let open: Vec<usize> = active.iter().copied().filter(|&j| out[j] < cap(j)).collect();
        let tot: f32 = open.iter().map(|&j| shares[j]).sum();
        let room = slots - active.iter().filter(|&&j| out[j] >= cap(j)).map(|&j| out[j]).sum::<usize>();
        let want = |j: usize| shares[j] / tot.max(1e-9) * room as f32 - out[j] as f32;
        let Some(j) = open.into_iter().max_by(|&a, &b| want(a).total_cmp(&want(b)).then(b.cmp(&a))) else { break };
        out[j] += 1;
        left -= 1;
    }
    out
}

/// A background profile's suitable tracks (`tracks` with their energy, see
/// [`crate::background`]) as up to `cards` cards of [`MIN_TRACKS`] at
/// least, split by energy: the calm ones, the middle, the livelier.
pub fn background_cards(tracks: &[(usize, f32)], cards: usize) -> Vec<Vec<usize>> {
    let k = cards.min(tracks.len() / MIN_TRACKS);
    if k == 0 {
        return Vec::new();
    }
    let mut by: Vec<(usize, f32)> = tracks.to_vec();
    by.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let n = by.len();
    (0..k).map(|c| by[c * n / k..(c + 1) * n / k].iter().map(|x| x.0).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;
    use crate::test_util::*;

    /// A gaussian cloud of `n` points around `c` (spread `s`) in the sound.
    fn cloud(rng: &mut Rng, c: [f32; 2], n: usize, s: f32) -> Vec<Vector> {
        (0..n)
            .map(|_| {
                let g = |rng: &mut Rng| (0..6).map(|_| rng.f32()).sum::<f32>() - 3.0;
                let mut v = [0.0f32; DIM];
                v[0] = c[0] + s * g(rng);
                v[1] = c[1] + s * g(rng);
                v[2] = s * g(rng) * 0.5;
                v
            })
            .collect()
    }

    fn sung(l: Lang) -> SongType {
        SongType { voice: Voice::Sung, lang: Some(l), ..SongType::default() }
    }

    struct Lib {
        ids: Vec<TrackId>,
        sound: Vec<Option<Vector>>,
        types: Vec<SongType>,
        artists: Vec<Option<ArtistId>>,
        mass: Vec<f32>,
        alive: Vec<f32>,
    }

    impl Lib {
        fn new() -> Self {
            Self { ids: Vec::new(), sound: Vec::new(), types: Vec::new(), artists: Vec::new(), mass: Vec::new(), alive: Vec::new() }
        }

        fn add(&mut self, vs: Vec<Vector>, ty: SongType, artist: u32, mass: f32) {
            for v in vs {
                let i = self.ids.len() as u128;
                self.ids.push(tid(i));
                self.sound.push(Some(v));
                self.types.push(ty);
                self.artists.push(Some(aid(artist + (i % 3) as u32)));
                self.mass.push(mass);
                self.alive.push(0.0);
            }
        }

        fn build(&self, history: Option<&ProfileHistory>, day: i64) -> Profiles {
            self.build_with(history, day, Params::default())
        }

        fn build_with(&self, history: Option<&ProfileHistory>, day: i64, params: Params) -> Profiles {
            Profiles::build(&Input {
                ids: &self.ids,
                sound: &self.sound,
                types: &self.types,
                artists: &self.artists,
                mass: &self.mass,
                alive: &self.alive,
                plays: &[],
                playlists: &[],
                scale: 1.0,
                params,
                history,
                day,
            })
        }
    }

    /// Three languages of one sound: three profiles, each its own.
    #[test]
    fn lanes_split_by_language() {
        let mut rng = Rng::new(1);
        let mut l = Lib::new();
        for (k, lang) in [Lang::Russian, Lang::Japanese, Lang::Spanish].into_iter().enumerate() {
            l.add(cloud(&mut rng, [0.0, 0.0], 40, 0.5), sung(lang), k as u32 * 10, 1.0);
        }
        let p = l.build(None, 0);
        assert_eq!(p.len(), 3, "{:?}", p.list.iter().map(|x| x.lane).collect::<Vec<_>>());
        for j in 0..3 {
            let lanes: HashSet<LaneKey> = (0..l.ids.len()).filter(|&i| p.of(i) == Some(j)).map(|i| LaneKey::of(&l.types[i])).collect();
            assert_eq!(lanes.len(), 1);
        }
    }

    /// One Gaussian cloud stays one profile however many points.
    #[test]
    fn one_cloud_stays_one() {
        let mut rng = Rng::new(2);
        let mut l = Lib::new();
        l.add(cloud(&mut rng, [0.0, 0.0], 200, 1.0), sung(Lang::Russian), 0, 1.0);
        assert_eq!(l.build(None, 0).len(), 1);
    }

    /// Two clouds 5σ apart in one language: two profiles.
    #[test]
    fn two_clouds_split() {
        let mut rng = Rng::new(3);
        let mut l = Lib::new();
        l.add(cloud(&mut rng, [0.0, 0.0], 100, 1.0), sung(Lang::Russian), 0, 1.0);
        l.add(cloud(&mut rng, [5.0, 0.0], 100, 1.0), sung(Lang::Russian), 10, 1.0);
        let p = l.build(None, 0);
        assert_eq!(p.len(), 2);
        assert_ne!(p.of(0), p.of(150));
        // Not split by sound when the setting says so.
        assert_eq!(l.build_with(None, 0, Params { split: false, ..Params::default() }).len(), 1);
    }

    /// A light profile cleanly apart from a heavy one (masses 1:10) is not
    /// swallowed: the test of shape weighs both sides alike.
    #[test]
    fn light_profile_not_swallowed() {
        let mut rng = Rng::new(4);
        let mut l = Lib::new();
        l.add(cloud(&mut rng, [0.0, 0.0], 100, 1.0), sung(Lang::Russian), 0, 1.0);
        l.add(cloud(&mut rng, [6.0, 0.0], 40, 1.0), sung(Lang::Russian), 10, 0.25);
        let p = l.build(None, 0);
        assert_eq!(p.len(), 2, "{:?}", p.list.iter().map(|x| x.members).collect::<Vec<_>>());
    }

    /// A kind grown to 90 % of the mass does not swallow one the user still
    /// plays by hand (30 active plays in 60 days).
    #[test]
    fn alive_survives_growth() {
        let mut rng = Rng::new(5);
        let mut l = Lib::new();
        l.add(cloud(&mut rng, [0.0, 0.0], 150, 1.0), SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() }, 0, 3.0);
        l.add(cloud(&mut rng, [1.0, 0.0], 20, 1.0), sung(Lang::Japanese), 10, 0.5);
        assert_eq!(l.build(None, 0).len(), 1, "2 % of the mass, nothing alive: joins");
        for a in l.alive.iter_mut().skip(150) {
            *a = 2.0;
        }
        let p = l.build(None, 0);
        assert_eq!(p.len(), 2, "alive");
        assert!(p.list.iter().any(|x| x.lane == LaneKey::Sung(Lang::Japanese)));
    }

    /// The key stays while a profile drifts a little; a new one shows
    /// after three days of builds; a lane gone keeps standing two weeks.
    #[test]
    fn key_kept_under_drift() {
        let mut rng = Rng::new(6);
        let mut l = Lib::new();
        l.add(cloud(&mut rng, [0.0, 0.0], 80, 1.0), sung(Lang::Russian), 0, 1.0);
        l.add(cloud(&mut rng, [0.0, 3.0], 80, 1.0), sung(Lang::English), 10, 1.0);
        let a = l.build(None, 10);
        // The sound drifts a little, the masses change.
        for v in l.sound.iter_mut().flatten() {
            v[0] += 0.2;
        }
        for (k, m) in l.mass.iter_mut().enumerate() {
            *m = 1.0 + (k % 5) as f32 * 0.3;
        }
        let b = l.build(Some(&a.history), 11);
        let keys = |p: &Profiles| p.list.iter().map(|x| (x.lane, x.key)).collect::<HashMap<_, _>>();
        assert_eq!(keys(&a), keys(&b));
    }

    #[test]
    fn born_after_three_builds() {
        let mut rng = Rng::new(7);
        let mut l = Lib::new();
        l.add(cloud(&mut rng, [0.0, 0.0], 80, 1.0), sung(Lang::Russian), 0, 1.0);
        let mut h = l.build(None, 100).history;
        // Japanese songs come.
        l.add(cloud(&mut rng, [0.0, 3.0], 40, 1.0), sung(Lang::Japanese), 10, 1.0);
        let ja = |p: &Profiles| p.list.iter().find(|x| x.lane == LaneKey::Sung(Lang::Japanese)).cloned().unwrap();
        for (day, shown) in [(101, false), (101, false), (102, false), (103, true), (104, true)] {
            let p = l.build(Some(&h), day);
            assert_eq!(ja(&p).visible, shown, "day {day}");
            assert!(p.list.iter().find(|x| x.lane == LaneKey::Sung(Lang::Russian)).unwrap().visible);
            h = p.history;
        }
        // The Japanese songs fade to a trace: the lane stands for two weeks.
        let key = ja(&l.build(Some(&h), 104)).key;
        for m in l.mass.iter_mut().skip(80) {
            *m = 0.01;
        }
        let p = l.build(Some(&h), 110);
        assert_eq!(ja(&p).key, key, "still there");
        let p = l.build(Some(&p.history), 104 + DIES_AFTER_DAYS + 1);
        assert!(p.list.iter().all(|x| x.lane != LaneKey::Sung(Lang::Japanese)), "gone");
    }

    /// Names come from what is in the profile: the kind at 60 %, a word of
    /// loudness between two of a kind, the artists below; or the artists.
    #[test]
    fn names_by_content() {
        let mut rng = Rng::new(8);
        let mut l = Lib::new();
        let calm = |mut t: SongType, e: f32| {
            t.energy_q = e;
            t
        };
        l.add(cloud(&mut rng, [0.0, 0.0], 60, 0.8), calm(sung(Lang::Russian), 0.3), 0, 1.0);
        l.add(cloud(&mut rng, [6.0, 0.0], 60, 0.8), calm(sung(Lang::Russian), 0.8), 10, 1.0);
        l.add(cloud(&mut rng, [0.0, 6.0], 50, 0.8), SongType { voice: Voice::Instrumental, ost: true, ..SongType::default() }, 20, 1.0);
        l.add(cloud(&mut rng, [6.0, 6.0], 40, 0.8), sung(Lang::Other), 30, 1.0);
        let p = l.build(None, 0);
        let artist = |a: ArtistId| Some(format!("A{}", a.0.as_u128()));
        let mut names: Vec<String> = (0..p.len()).map(|j| p.name_in(j, NameStyle::Kind, Locale::Ru, &artist).0).collect();
        names.sort();
        assert_eq!(names, ["Песни", "Русские песни, погромче", "Русские песни, потише", "Саундтреки без слов"]);
        let mut en_names: Vec<String> = (0..p.len()).map(|j| p.name_in(j, NameStyle::Kind, Locale::En, &artist).0).collect();
        en_names.sort();
        assert_eq!(en_names, ["Instrumental Soundtracks", "Russian Songs, louder", "Russian Songs, quieter", "Songs"]);
        let (title, sub) = p.name_in(0, NameStyle::Artists, Locale::Ru, &artist);
        assert!(title.ends_with(" и другие") && title.starts_with('A'), "{title} / {sub}");
        let (en_title, en_sub) = p.name_in(0, NameStyle::Artists, Locale::En, &artist);
        assert!(en_title.ends_with(" & more") && en_title.starts_with('A'), "{en_title} / {en_sub}");
        let (_, sub) = p.name_in(0, NameStyle::Kind, Locale::Ru, &artist);
        assert_eq!(sub.split(", ").count(), 3);
        assert_eq!(Kind::Instrumental.title_in(Locale::En), "Instrumental");
        assert_eq!(Kind::Soundtracks.title_in(Locale::En), "Soundtracks");
        assert_eq!(Kind::Calm.title_in(Locale::En), "Calm");
        assert_eq!(Kind::Lively.title_in(Locale::En), "Upbeat");
        assert_eq!(Kind::Sung(Lang::Japanese).title_in(Locale::En), "Japanese Songs");
    }

    #[test]
    fn shares_and_background() {
        let plays: Vec<SharePlay> = (0..100)
            .map(|k| SharePlay { ts: 1_000, profile: Some(usize::from(k >= 70)), p_bg: if k < 70 { 1.0 } else { 0.0 }, weight: 1.0 })
            .collect();
        let naive = shares(&plays, 2, 1.0, None, 1_000);
        let balanced = shares(&plays, 2, crate::listening::W_BG, None, 1_000);
        assert!((naive[0] - 0.7).abs() < 1e-4);
        assert!(balanced[0] < 0.45 && balanced[1] > 0.55, "{balanced:?}");
        assert_eq!(background_shares(&plays, 2), vec![1.0, 0.0]);
    }

    /// Ambient played in the background up to 70 %: at most three cards,
    /// every other active profile keeps one.
    #[test]
    fn background_profile_capped() {
        let s = [0.70, 0.20, 0.05, 0.05];
        assert_eq!(quota(&s, 6, &[false; 4], BG_CARDS), vec![3, 1, 1, 1], "by share");
        let s = [0.85, 0.09, 0.03, 0.03];
        assert_eq!(quota(&s, 6, &[true, false, false, false], BG_CARDS), vec![3, 1, 1, 1]);
        assert_eq!(quota(&s, 6, &[true, false, false, false], 1), vec![1, 3, 1, 1], "one card of background");
        assert_eq!(quota(&s, 6, &[false; 4], BG_CARDS), vec![3, 1, 1, 1]);
        let cards = background_cards(&(0..30).map(|i| (i, i as f32 / 30.0)).collect::<Vec<_>>(), BG_CARDS);
        assert_eq!(cards.len(), 3);
        assert!(cards[0].iter().all(|i| *i < 10) && cards[2].iter().all(|i| *i >= 20));
        assert_eq!(background_cards(&[(0, 0.1); 12], 3).len(), 1);
    }

    /// Each active profile gets a card while they last, the largest first;
    /// one under the active share gets none.
    #[test]
    fn floor_for_each_active() {
        let s = [0.9, 0.04, 0.03, 0.02, 0.01];
        assert_eq!(quota(&s, 6, &[false; 5], BG_CARDS), vec![3, 1, 1, 1, 0]);
        let many = [0.3, 0.2, 0.1, 0.1, 0.1, 0.1, 0.05, 0.05];
        let q = quota(&many, 6, &[false; 8], BG_CARDS);
        assert_eq!(q.iter().sum::<usize>(), 6);
        assert_eq!(q, vec![1, 1, 1, 1, 1, 1, 0, 0], "the largest first, the rest by days");
        assert_eq!(quota(&[1.0], 6, &[false], BG_CARDS), vec![6]);
        assert!(quota(&[], 6, &[], BG_CARDS).is_empty());
    }
}
