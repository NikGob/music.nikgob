//! The user model (recs-v2 §4): what the history says about every track,
//! world and pair of tracks, a model of the next start and a model of the
//! early skip, both learning online.
//!
//! * [`TrackStats`]: per track two sums of outcome weights, halving in 45
//!   days and in 2 days (§4.2), counts of starts and outcomes, the last
//!   start and complete, starts by part of the day. An outcome weighs
//!   (§4.1): complete +1, late skip +0.15 × the share heard, early skip
//!   −0.8 (×0.6 after the user's own choice, ×1.3 after the app's, ×0.5
//!   from the third running in a playlist or the library), a start never
//!   ended +0.1 (0 when the app closed on it), a repeat within a day of a
//!   complete +0.7; positives ×1.2 when the user chose; a like +2.
//! * [`WorldStats`] and the transitions world → world inside sessions;
//!   [`CoOccur`]: track → track inside sessions, `0.5^(k−1)` for the k-th
//!   start after (k ≤ 3), the top [`CO_TOP`] of each track kept.
//! * [`Model`]: a conditional logit over [`FEATURES`] of every candidate
//!   (`scripts/replay.py`, P2): one step after each start the user chose
//!   (`w ← w + η(f(target) − E_P[f]) − ηλ(w − w_prior)`, the expectation
//!   over the 100 best candidates), from the hand weights [`HAND`], the
//!   step halved for the first [`FULL_STEP_AFTER`] starts. Starts the app
//!   chose never teach it (the model would learn its own picks).
//! * [`SkipModel`]: online logistic regression of an early skip on
//!   [`SKIP_FEATURES`], a step on every start that completed or was skipped
//!   early, whoever chose it.
//!
//! [`UserModel::build`] walks the history in order of time: the stats, and
//! the weights too unless [`Weights`] of the same [`FEATURE_VERSION`] are
//! given (the library's cache keeps them between builds, the runtime
//! teaches them in between). The same walk replays a history prequentially
//! ([`replay`]): every start is predicted before it is learnt.

use std::collections::{HashMap, HashSet};

use nikgob_core::model::{ArtistId, EndReason, EventContext, EventOrigin, EventType, TrackId, UnixMs};

use serde::{Deserialize, Serialize};

use crate::analysis::Vector;
use crate::index::{VectorIndex, euclid};
use crate::session::{Ended, Session, TrackLookup};
use crate::songtype::{SongLane, SongType, SongTypes, sigmoid};
use crate::taste::{
    Catalog, DAY_MS, DayPart, EARLY_SKIP_MS, HOUR_MS, Play, REPEAT_WITHIN_MS, SESSION_GAP_MS, SKIP_STREAK_FACTOR,
    SKIP_STREAK_FROM, Taste, TasteInput, W_COMPLETE, W_LIKE, W_REPEAT, W_SKIP_EARLY, W_SKIP_LATE_HEARD, W_START_ONLY,
};

/// Bumped when a feature changes meaning: cached weights of another
/// version are learnt again from the history.
pub const FEATURE_VERSION: u32 = 1;

pub const H_LONG_MS: i64 = 45 * DAY_MS;
pub const H_HOT_MS: i64 = 2 * DAY_MS;
const R_COMPLETE: f32 = 1.0;
const R_LATE: f32 = 0.15;
const R_EARLY: f32 = -0.8;
const R_OPEN: f32 = 0.1;
const R_REPEAT: f32 = 0.7;
const R_LIKE: f32 = 2.0;
const MANUAL_POSITIVE: f32 = 1.2;
const MANUAL_EARLY: f32 = 0.6;
const AUTO_EARLY: f32 = 1.3;
/// Track → track pairs kept per track.
pub const CO_TOP: usize = 50;
/// A start shorter than this is an interlude.
const INTERLUDE_MS: u32 = 75_000;
/// A track longer than this is long (the skip model).
const LONG_MS: u32 = 6 * 60_000;
/// Song lanes ([`lane_index`]).
const LANES: usize = 5;
/// A radio of one kind of song goes on to another only when the user's
/// own next choices go there this often (a generic prior: about one in
/// seven)…
pub const LANE_FOLLOWS: f32 = 0.15;
/// …over at least this many choices from the first kind.
pub const LANE_EVIDENCE: f32 = 10.0;

pub fn lane_index(l: SongLane) -> usize {
    match l {
        SongLane::Instrumental => 0,
        SongLane::Russian => 1,
        SongLane::Japanese => 2,
        SongLane::Western => 3,
        SongLane::Unknown => 4,
    }
}


/// The features of a candidate for the next start, in this order.
pub const FEATURES: [&str; 16] = [
    "aff_long", "aff_hot", "liked", "sound", "skip_push", "next_album", "same_album", "same_world", "world_trans",
    "cooc", "in_session", "recent3h", "daypart_world", "same_lane", "never", "interlude",
];
pub const F: usize = FEATURES.len();
pub const F_SOUND: usize = 3;
pub const F_SKIP_PUSH: usize = 4;
pub const F_NEXT_ALBUM: usize = 5;
pub const F_SAME_ALBUM: usize = 6;
pub const F_SAME_WORLD: usize = 7;
pub const F_COOC: usize = 9;
pub const F_SAME_LANE: usize = 13;
/// The hand weights (`scripts/replay.py`, `HAND`): the prior and the start.
pub const HAND: [f32; F] = [1.0, 1.2, 0.3, 0.8, 1.0, 3.0, 1.0, 0.8, 1.5, 1.5, -2.0, 0.3, 1.0, 0.8, -0.3, -0.5];
/// What the replay of 15 days learnt from them (`out/replay.json`), for
/// reference and tests.
pub const LEARNED: [f32; F] = [0.68, -0.1, 0.94, 1.15, 0.0, 3.62, 0.52, 0.65, 1.64, 1.77, -0.1, 0.7, 0.77, 0.66, -1.1, -0.03];
const LR: f32 = 0.05;
const L2: f32 = 0.01;
/// The step is halved until this many starts taught the model.
pub const FULL_STEP_AFTER: u32 = 300;
/// The expectation of a step is taken over this many best candidates.
pub const EXPECT_OVER: usize = 100;

/// The features of a start for its early skip, in this order.
pub const SKIP_FEATURES: [&str; 23] = [
    "bias", "aff_today", "aff_long", "aff_hot", "early_rate", "never", "liked", "pos0", "pos_log", "ctx_album",
    "ctx_playlist", "ctx_artist", "ctx_home", "same_album", "same_world", "sound_jump", "night", "ost", "vocal",
    "long", "interlude", "prev_early", "prev2_early",
];
pub const G: usize = SKIP_FEATURES.len();
const SKIP_LR: f32 = 0.08;
const SKIP_L2: f32 = 0.001;

/// What the history says about one track (by catalog position).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackStats {
    a_long: f32,
    a_hot: f32,
    /// The weights of [`crate::taste`] (the affinity of phase 1).
    a_today: f32,
    t_ref: UnixMs,
    pub starts: u32,
    pub completes: u32,
    pub early: u16,
    pub late: u16,
    pub last_start: Option<UnixMs>,
    pub last_complete: Option<UnixMs>,
    pub part: [u16; 4],
    /// A start not ended yet, and who chose it.
    open: Option<(UnixMs, Option<EventOrigin>)>,
    liked: Option<UnixMs>,
}

fn halve(x: f32, from: UnixMs, to: UnixMs, half: i64) -> f32 {
    if to <= from {
        return x;
    }
    x * (-((to - from) as f64) / half as f64).exp2() as f32
}

impl TrackStats {
    fn add(&mut self, ts: UnixMs, r: f32, today: f32) {
        let ts = ts.max(self.t_ref);
        self.a_long = halve(self.a_long, self.t_ref, ts, H_LONG_MS) + r;
        self.a_hot = halve(self.a_hot, self.t_ref, ts, H_HOT_MS) + r;
        self.a_today = halve(self.a_today, self.t_ref, ts, crate::taste::HALF_LIFE_MS) + today;
        self.t_ref = ts;
    }

    /// The long affinity at `now`, `tanh(A / 3)`.
    pub fn aff_long(&self, now: UnixMs) -> f32 {
        let mut a = halve(self.a_long, self.t_ref, now, H_LONG_MS);
        if let Some((t, _)) = self.open {
            a += halve(R_OPEN, t, now, H_LONG_MS);
        }
        if self.liked.is_some_and(|t| t <= now) {
            a += R_LIKE;
        }
        (a / 3.0).tanh()
    }

    /// The hot affinity at `now`, `tanh(A / 2)`.
    pub fn aff_hot(&self, now: UnixMs) -> f32 {
        (halve(self.a_hot, self.t_ref, now, H_HOT_MS) / 2.0).tanh()
    }

    /// The affinity of phase 1 at `now`, `tanh(A / 3)`.
    pub fn aff_today(&self, now: UnixMs) -> f32 {
        let mut a = halve(self.a_today, self.t_ref, now, crate::taste::HALF_LIFE_MS);
        if let Some((t, _)) = self.open {
            a += halve(W_START_ONLY, t, now, crate::taste::HALF_LIFE_MS);
        }
        if self.liked.is_some_and(|t| t <= now) {
            a += W_LIKE;
        }
        (a / 3.0).tanh()
    }

    pub fn liked(&self, now: UnixMs) -> bool {
        self.liked.is_some_and(|t| t <= now)
    }
}

/// A world's presence (§4.3).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldStats {
    hot: f32,
    t_ref: UnixMs,
    pub last: Option<UnixMs>,
    pub part: [u32; 4],
    pub n: u32,
}

impl WorldStats {
    /// Starts halving in a day, at `now`.
    pub fn hot(&self, now: UnixMs) -> f32 {
        halve(self.hot, self.t_ref, now, DAY_MS)
    }
}

/// Track → track inside sessions (by catalog position).
#[derive(Clone, Debug, Default)]
pub struct CoOccur {
    next: Vec<Vec<(u32, f32)>>,
}

impl CoOccur {
    fn new(n: usize) -> Self {
        Self { next: vec![Vec::new(); n] }
    }

    fn add(&mut self, a: usize, b: usize, w: f32) {
        let row = &mut self.next[a];
        match row.iter_mut().find(|(c, _)| *c as usize == b) {
            Some(e) => e.1 += w,
            None => row.push((b as u32, w)),
        }
        if row.len() > 2 * CO_TOP {
            row.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            row.truncate(CO_TOP);
        }
    }

    pub fn get(&self, a: usize, b: usize) -> f32 {
        self.next.get(a).and_then(|r| r.iter().find(|(c, _)| *c as usize == b)).map_or(0.0, |e| e.1)
    }

    /// What followed `a`, most first (the top [`CO_TOP`]).
    pub fn after(&self, a: usize) -> Vec<(usize, f32)> {
        let mut row: Vec<(usize, f32)> = self.next.get(a).map(|r| r.iter().map(|(c, w)| (*c as usize, *w)).collect()).unwrap_or_default();
        row.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        row.truncate(CO_TOP);
        row
    }
}

/// The model of the next start.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub w: [f32; F],
    pub prior: [f32; F],
    /// Starts it learnt from.
    pub seen: u32,
}

impl Default for Model {
    fn default() -> Self {
        Self { w: HAND, prior: HAND, seen: 0 }
    }
}

impl Model {
    pub fn score(&self, f: &[f32; F]) -> f32 {
        self.w.iter().zip(f).map(|(w, x)| w * x).sum()
    }

    /// One step towards `target` among `cands` (the target among them or
    /// not): the conditional logit's gradient, L2 towards the prior.
    pub fn observe(&mut self, target: &[f32; F], cands: &[[f32; F]]) {
        let scores: Vec<f32> = cands.iter().map(|f| self.score(f)).collect();
        let top = scores.iter().copied().fold(self.score(target), f32::max);
        let mut z: f32 = (self.score(target) - top).exp();
        let mut expect = target.map(|x| x * z);
        for (f, s) in cands.iter().zip(&scores) {
            let p = (s - top).exp();
            z += p;
            for (e, x) in expect.iter_mut().zip(f) {
                *e += p * x;
            }
        }
        let lr = if self.seen < FULL_STEP_AFTER { LR * 0.5 } else { LR };
        for k in 0..F {
            let grad = target[k] - expect[k] / z;
            self.w[k] += lr * grad - lr * L2 * (self.w[k] - self.prior[k]);
        }
        self.seen += 1;
    }
}

/// The model of an early skip.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkipModel {
    pub w: [f32; G],
    pub seen: u32,
}

impl Default for SkipModel {
    fn default() -> Self {
        Self { w: [0.0; G], seen: 0 }
    }
}

impl SkipModel {
    pub fn p(&self, x: &[f32; G]) -> f32 {
        sigmoid(self.w.iter().zip(x).map(|(w, x)| w * x).sum())
    }

    /// One step: `skipped` early or completed.
    pub fn observe(&mut self, x: &[f32; G], skipped: bool) {
        let p = self.p(x);
        let y = f32::from(u8::from(skipped));
        for (w, xi) in self.w.iter_mut().zip(x) {
            *w += SKIP_LR * ((y - p) * xi - SKIP_L2 * *w);
        }
        self.seen += 1;
    }
}

/// Both models' weights, as the library's cache keeps them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Weights {
    pub version: u32,
    pub next: Model,
    pub skip: SkipModel,
}

impl Default for Weights {
    fn default() -> Self {
        Self { version: FEATURE_VERSION, next: Model::default(), skip: SkipModel::default() }
    }
}

/// Whether a start teaches the model of the next start: the user chose
/// it (a history older than origins: anything but a radio's).
pub fn teaches(origin: Option<EventOrigin>, context: EventContext) -> bool {
    match origin {
        Some(o) => !o.is_auto(),
        None => context != EventContext::Radio,
    }
}

/// What [`UserModel::build`] reads besides the catalog and the taste's input.
#[derive(Clone, Copy)]
pub struct ModelInput<'a> {
    pub index: &'a VectorIndex<TrackId>,
    pub types: Option<&'a SongTypes>,
    /// The weights to go on with (else learnt again from the history).
    pub weights: Option<Weights>,
}

/// One moment of a session, as the features see it.
#[derive(Clone, Debug)]
pub struct Moment {
    pub now: UnixMs,
    /// The session's tracks by catalog position (unknown ones left out),
    /// oldest first, and how each ended.
    pub sess: Vec<(usize, Ended, EventContext)>,
    pub prev: Option<usize>,
    pub next_album: Option<usize>,
    pub anchor: Option<Vector>,
    pub pushes: Vec<Vector>,
    pub lane: Option<SongLane>,
    pub part: usize,
    pub skip_streak: u8,
    pub album_session: bool,
    pub world_session: Option<u64>,
    pub repeat_one: bool,
}

/// The user model (see the module docs).
#[derive(Clone, Debug)]
pub struct UserModel {
    pub taste: Taste,
    now: UnixMs,
    utc_offset_min: i32,
    ids: Vec<TrackId>,
    pos: HashMap<TrackId, usize>,
    pub tracks: Vec<TrackStats>,
    emb: Vec<Option<Vector>>,
    release: Vec<Option<u64>>,
    album_pos: Vec<u32>,
    world: Vec<Option<u64>>,
    artist: Vec<Option<ArtistId>>,
    duration: Vec<Option<u32>>,
    types: Vec<SongType>,
    next_in_album: Vec<Option<u32>>,
    pub worlds: HashMap<u64, WorldStats>,
    part_total: [u32; 4],
    trans: HashMap<u64, HashMap<u64, f32>>,
    /// Song lane → song lane of the starts the user chose, by
    /// [`lane_index`].
    lane_trans: [[f32; LANES]; LANES],
    pub co: CoOccur,
    pub weights: Weights,
    /// The median distance between tracks.
    scale: f32,
    /// The walk: its session, the skip features of open starts, the run of
    /// early skips for leafing.
    walk: Session,
    pending: HashMap<usize, [f32; G]>,
    streak: u32,
    last_event: Option<UnixMs>,
    learn: bool,
}

impl TrackLookup for UserModel {
    fn vector(&self, id: TrackId) -> Option<Vector> {
        self.pos.get(&id).and_then(|&i| self.emb[i])
    }
    fn album(&self, id: TrackId) -> Option<(u64, u32)> {
        let &i = self.pos.get(&id)?;
        Some((self.release[i]?, self.album_pos[i]))
    }
    fn world(&self, id: TrackId) -> Option<u64> {
        self.pos.get(&id).and_then(|&i| self.world[i])
    }
    fn lane(&self, id: TrackId) -> Option<SongLane> {
        self.pos.get(&id).map(|&i| self.types[i].lane())
    }
}

/// Events in the order the walk takes them: by time, a start after the
/// end it shares a moment with.
fn ordered(events: &[Play]) -> Vec<&Play> {
    let mut sorted: Vec<&Play> = events.iter().collect();
    sorted.sort_by_key(|p| (p.ts, p.kind == EventType::Start));
    sorted
}

impl UserModel {
    /// The model of the history of `input` over `catalog` at `now` (see
    /// the module docs); the taste of [`Taste::build`] with it.
    pub fn build(catalog: &Catalog, input: &TasteInput, extra: ModelInput<'_>, now: UnixMs) -> Self {
        let taste = Taste::build(catalog, input, now);
        let mut m = Self::empty(catalog, taste, extra, input.utc_offset_min, now);
        m.learn = extra.weights.is_none_or(|w| w.version != FEATURE_VERSION);
        if let Some(w) = extra.weights.filter(|w| w.version == FEATURE_VERSION) {
            m.weights = w;
        }
        // Likes as they are, from when they were set (else always).
        for id in &input.likes {
            if let Some(&i) = m.pos.get(id) {
                m.tracks[i].liked = Some(i64::MIN);
            }
        }
        for (id, at) in &input.like_times {
            if let Some(&i) = m.pos.get(id)
                && m.tracks[i].liked.is_some()
            {
                m.tracks[i].liked = Some(*at);
            }
        }
        for p in ordered(&input.events) {
            m.event(p, None);
        }
        m.now = now;
        m.learn = false;
        m
    }

    fn empty(catalog: &Catalog, taste: Taste, extra: ModelInput<'_>, utc_offset_min: i32, now: UnixMs) -> Self {
        let n = catalog.len();
        let norm = extra.index.normalizer();
        let ids: Vec<TrackId> = catalog.tracks().iter().map(|t| t.id).collect();
        let pos = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let emb: Vec<Option<Vector>> = ids.iter().map(|id| extra.index.get(*id).map(|v| norm.embed(v))).collect();
        let release: Vec<Option<u64>> = catalog.tracks().iter().map(|t| t.release.map(|r| crate::pick::token(r.as_uuid().as_bytes()))).collect();
        let album_pos: Vec<u32> = catalog.tracks().iter().map(|t| t.position).collect();
        let mut by_release: HashMap<u64, Vec<usize>> = HashMap::new();
        for (i, r) in release.iter().enumerate() {
            if let Some(r) = r {
                by_release.entry(*r).or_default().push(i);
            }
        }
        let mut next_in_album = vec![None; n];
        for list in by_release.values_mut() {
            list.sort_by_key(|&i| (album_pos[i], i));
            for w in list.windows(2) {
                if album_pos[w[1]] > album_pos[w[0]] {
                    next_in_album[w[0]] = Some(w[1] as u32);
                }
            }
            // Equal places: the next greater one.
            for k in 0..list.len() {
                if next_in_album[list[k]].is_none() {
                    next_in_album[list[k]] = list[k + 1..].iter().find(|&&j| album_pos[j] > album_pos[list[k]]).map(|&j| j as u32);
                }
            }
        }
        let types = ids.iter().map(|id| extra.types.and_then(|t| t.get(*id)).copied().unwrap_or_default()).collect();
        let vs: Vec<&Vector> = emb.iter().flatten().collect();
        let scale = {
            let step = (vs.len() / 64).max(1);
            let sample: Vec<&&Vector> = vs.iter().step_by(step).collect();
            let half = sample.len() / 2;
            let mut d: Vec<f32> = (0..half).map(|k| euclid(*sample[k], *sample[k + half])).collect();
            d.sort_by(f32::total_cmp);
            d.get(d.len() / 2).copied().filter(|x| *x > 1e-3).unwrap_or(2.0)
        };
        Self {
            taste,
            now,
            utc_offset_min,
            ids,
            pos,
            tracks: vec![TrackStats::default(); n],
            emb,
            release,
            album_pos,
            world: catalog.tracks().iter().map(|t| t.world).collect(),
            artist: catalog.tracks().iter().map(|t| t.main_artist()).collect(),
            duration: catalog.tracks().iter().map(|t| t.duration_ms).collect(),
            types,
            next_in_album,
            worlds: HashMap::new(),
            part_total: [0; 4],
            trans: HashMap::new(),
            lane_trans: [[0.0; LANES]; LANES],
            co: CoOccur::new(n),
            weights: Weights::default(),
            scale,
            walk: Session::default(),
            pending: HashMap::new(),
            streak: 0,
            last_event: None,
            learn: false,
        }
    }

    pub fn now(&self) -> UnixMs {
        self.now
    }

    pub fn position(&self, id: TrackId) -> Option<usize> {
        self.pos.get(&id).copied()
    }

    pub fn id(&self, i: usize) -> TrackId {
        self.ids[i]
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn embedded_at(&self, i: usize) -> Option<&Vector> {
        self.emb[i].as_ref()
    }

    pub fn song_type_at(&self, i: usize) -> &SongType {
        &self.types[i]
    }

    pub fn world_at(&self, i: usize) -> Option<u64> {
        self.world[i]
    }

    pub fn main_artist_at(&self, i: usize) -> Option<ArtistId> {
        self.artist[i]
    }

    /// Songs of lane `b` may follow songs of lane `a` in a radio of `a`:
    /// the same kind, anything unknown, or a kind this user goes on to
    /// from `a` in at least [`LANE_FOLLOWS`] of their own choices (once
    /// they made [`LANE_EVIDENCE`] of them).
    pub fn lanes_go(&self, a: SongLane, b: SongLane) -> bool {
        if a == b || a == SongLane::Unknown || b == SongLane::Unknown {
            return true;
        }
        let row = &self.lane_trans[lane_index(a)];
        let total: f32 = row.iter().sum();
        total >= LANE_EVIDENCE && row[lane_index(b)] / total >= LANE_FOLLOWS
    }

    pub fn release_at(&self, i: usize) -> Option<u64> {
        self.release[i]
    }

    pub fn next_in_album(&self, i: usize) -> Option<usize> {
        self.next_in_album[i].map(|j| j as usize)
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    fn part_of(&self, ts: UnixMs) -> usize {
        DayPart::at(ts, self.utc_offset_min).index()
    }

    /// The moment of `session` at `now` (at least the session's last start).
    pub fn moment(&self, session: &Session, now: UnixMs) -> Moment {
        let now = now.max(session.last_ts().unwrap_or(now));
        let sess: Vec<(usize, Ended, EventContext)> =
            session.tracks().filter_map(|p| Some((self.position(p.track)?, p.ended, p.context))).collect();
        let prev = session.last().and_then(|p| self.position(p.track));
        Moment {
            now,
            next_album: prev.and_then(|i| self.next_in_album(i)),
            prev,
            sess,
            anchor: session.anchor(self),
            pushes: session.pushes(self),
            lane: session.lane(self),
            part: self.part_of(now),
            skip_streak: session.skip_streak(),
            album_session: session.album_session(self),
            world_session: session.world_session(self),
            repeat_one: session.repeat_one(),
        }
    }

    /// The features of candidate `c` at moment `m` ([`FEATURES`]).
    pub fn features(&self, m: &Moment, c: usize) -> [f32; F] {
        let t = &self.tracks[c];
        let mut f = [0.0f32; F];
        f[0] = t.aff_long(m.now);
        f[1] = t.aff_hot(m.now);
        f[2] = f32::from(u8::from(t.liked(m.now)));
        let e = self.emb[c].as_ref();
        f[F_SOUND] = match (e, &m.anchor) {
            (Some(e), Some(a)) => -euclid(e, a) / self.scale,
            _ => -1.0,
        };
        if let Some(e) = e {
            f[F_SKIP_PUSH] = -m.pushes.iter().map(|p| (1.0 - euclid(e, p) / self.scale).max(0.0)).sum::<f32>();
        }
        f[F_NEXT_ALBUM] = f32::from(u8::from(m.next_album == Some(c)));
        if let Some(p) = m.prev {
            f[F_SAME_ALBUM] = f32::from(u8::from(self.release[c].is_some() && self.release[c] == self.release[p]));
            let (wc, wp) = (self.world[c], self.world[p]);
            f[F_SAME_WORLD] = f32::from(u8::from(wp.is_some() && wc == wp));
            if let (Some(wp), Some(wc)) = (wp, wc)
                && let Some(out) = self.trans.get(&wp)
            {
                let total: f32 = out.values().sum();
                f[8] = out.get(&wc).copied().unwrap_or(0.0) / (total + 2.0);
            }
        }
        let mut co = 0.0;
        for (k, (s, _, _)) in m.sess.iter().rev().take(3).enumerate() {
            co += self.co.get(*s, c) * 0.6f32.powi(k as i32);
        }
        f[F_COOC] = co.tanh();
        let in_s = m.sess.iter().filter(|(s, _, _)| *s == c).count();
        f[10] = in_s.min(2) as f32;
        f[11] = f32::from(u8::from(in_s == 0 && t.last_start.is_some_and(|l| m.now - l < 3 * HOUR_MS)));
        if let Some(w) = self.world[c].and_then(|w| self.worlds.get(&w)) {
            let n: u32 = w.part.iter().sum();
            let all: u32 = self.part_total.iter().sum();
            if n >= 3 && all > 0 {
                f[12] = w.part[m.part] as f32 / n as f32 - self.part_total[m.part] as f32 / all as f32;
            }
        }
        f[F_SAME_LANE] = f32::from(u8::from(m.lane.is_some_and(|l| l == self.types[c].lane())));
        f[14] = f32::from(u8::from(t.starts == 0));
        f[15] = f32::from(u8::from(self.duration[c].is_some_and(|d| d < INTERLUDE_MS)));
        f
    }

    /// The relevance of `c` by the model of the next start.
    pub fn score(&self, m: &Moment, c: usize) -> f32 {
        self.weights.next.score(&self.features(m, c))
    }

    /// The features of starting `c` now for its early skip
    /// ([`SKIP_FEATURES`]), started from `ctx`.
    pub fn skip_features(&self, m: &Moment, c: usize, ctx: EventContext) -> [f32; G] {
        let t = &self.tracks[c];
        let ty = &self.types[c];
        let mut x = [0.0f32; G];
        let b = |v: bool| f32::from(u8::from(v));
        x[0] = 1.0;
        x[1] = t.aff_today(m.now);
        x[2] = t.aff_long(m.now);
        x[3] = t.aff_hot(m.now);
        x[4] = (f32::from(t.early) + 0.2) / (f32::from(t.early) + t.completes as f32 + 1.0);
        x[5] = b(t.starts == 0);
        x[6] = b(t.liked(m.now));
        x[7] = b(m.sess.is_empty());
        x[8] = (m.sess.len() as f32).ln_1p();
        x[9] = b(ctx == EventContext::Album);
        x[10] = b(ctx == EventContext::Playlist);
        x[11] = b(ctx == EventContext::Artist);
        x[12] = b(matches!(ctx, EventContext::HomeSection | EventContext::Radio));
        if let Some(p) = m.prev {
            x[13] = b(self.release[c].is_some() && self.release[c] == self.release[p]);
            x[14] = b(self.world[p].is_some() && self.world[c] == self.world[p]);
        }
        x[15] = match (m.prev.and_then(|p| self.emb[p].as_ref()), self.emb[c].as_ref()) {
            (Some(a), Some(v)) => euclid(a, v) / self.scale,
            _ => 1.0,
        };
        let local = m.now + i64::from(self.utc_offset_min) * 60_000;
        x[16] = b(local.rem_euclid(DAY_MS) / HOUR_MS < 6);
        x[17] = b(ty.ost);
        x[18] = b(ty.vocal());
        x[19] = b(self.duration[c].is_some_and(|d| d > LONG_MS));
        x[20] = b(self.duration[c].is_some_and(|d| d < INTERLUDE_MS));
        let mut ends = m.sess.iter().rev().map(|(_, e, _)| *e);
        let (e1, e2) = (ends.next(), ends.next());
        x[21] = b(e1 == Some(Ended::Early));
        x[22] = b(e1 == Some(Ended::Early) && e2 == Some(Ended::Early));
        x
    }

    /// The chance `c` started now from `ctx` is skipped in its first 30 s.
    pub fn p_skip(&self, m: &Moment, c: usize, ctx: EventContext) -> f32 {
        self.weights.skip.p(&self.skip_features(m, c, ctx))
    }

    /// The candidates of the next start at `m` (§5.2): the nearest to the
    /// anchor, what followed the last three, the next on the album, the
    /// previous track's world, the hot and the long favourites; `ok`
    /// filters them.
    pub fn candidates(&self, m: &Moment, ok: &dyn Fn(usize) -> bool) -> Vec<usize> {
        let mut out: HashSet<usize> = HashSet::new();
        if let Some(a) = &m.anchor {
            let mut near: Vec<(f32, usize)> = self.emb.iter().enumerate().filter_map(|(i, e)| Some((euclid(e.as_ref()?, a), i))).filter(|(_, i)| ok(*i)).collect();
            near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            out.extend(near.into_iter().take(200).map(|(_, i)| i));
        }
        let mut co: HashMap<usize, f32> = HashMap::new();
        for (k, (s, _, _)) in m.sess.iter().rev().take(3).enumerate() {
            for (c, w) in self.co.after(*s) {
                *co.entry(c).or_default() += w * 0.6f32.powi(k as i32);
            }
        }
        let mut co: Vec<(usize, f32)> = co.into_iter().filter(|(c, _)| ok(*c)).collect();
        co.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        out.extend(co.into_iter().take(50).map(|(c, _)| c));
        out.extend(m.next_album.filter(|&c| ok(c)));
        if let Some(w) = m.prev.and_then(|p| self.world[p]) {
            out.extend((0..self.len()).filter(|&i| self.world[i] == Some(w) && ok(i)));
        }
        let mut top = |key: &dyn Fn(&TrackStats) -> f32, n: usize| {
            let mut v: Vec<(f32, usize)> = (0..self.len()).filter(|&i| ok(i)).map(|i| (key(&self.tracks[i]), i)).filter(|(k, _)| *k > 0.0).collect();
            v.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
            out.extend(v.into_iter().take(n).map(|(_, i)| i));
        };
        top(&|t| t.aff_hot(m.now), 100);
        top(&|t| t.aff_long(m.now), 200);
        let mut out: Vec<usize> = out.into_iter().collect();
        out.sort_unstable();
        out
    }

    /// Teaches `next` the start of `target` at `m`, the user's choice,
    /// among the candidates of the moment: the runtime's step between
    /// builds (its weights, this model's stats).
    pub fn teach_next(&self, next: &mut Model, m: &Moment, target: usize) {
        let cands = self.candidates(m, &|c| c != target);
        let scored = cands.iter().map(|&c| self.features(m, c)).collect();
        step(next, self.features(m, target), scored);
    }

    /// One event of the history, in order of time. `hook` sees every start
    /// that follows another of its session, before anything learns from it.
    fn event(&mut self, p: &Play, hook: Option<&mut dyn FnMut(&Self, &Moment, usize, &Play)>) {
        if self.last_event.is_none_or(|l| p.ts - l >= SESSION_GAP_MS) {
            self.streak = 0;
        }
        self.last_event = Some(p.ts);
        let Some(&i) = self.pos.get(&p.track) else {
            if p.kind == EventType::Start {
                self.walk.start(p.track, p.ts, p.context, p.origin);
            }
            return;
        };
        match p.kind {
            EventType::Like => self.tracks[i].liked = Some(p.ts),
            EventType::Unlike => self.tracks[i].liked = None,
            EventType::Complete => {
                self.streak = 0;
                let manual = self.tracks[i].open.take().is_some_and(|(_, o)| o == Some(EventOrigin::Manual));
                let r = R_COMPLETE * if manual { MANUAL_POSITIVE } else { 1.0 };
                let t = &mut self.tracks[i];
                t.add(p.ts, r, W_COMPLETE);
                t.completes += 1;
                t.last_complete = Some(p.ts);
                self.walk.end(p.track, Ended::Complete);
                if let Some(x) = self.pending.remove(&i).filter(|_| self.learn) {
                    self.weights.skip.observe(&x, false);
                }
            }
            EventType::Skip => {
                let opened = self.tracks[i].open.take();
                if p.end_reason == Some(EndReason::AppClosed) {
                    self.pending.remove(&i);
                    return;
                }
                let origin = opened.and_then(|(_, o)| o);
                match p.position_ms {
                    Some(pos) if pos < EARLY_SKIP_MS => {
                        self.streak += 1;
                        let leafing = self.streak >= SKIP_STREAK_FROM && matches!(p.context, EventContext::Playlist | EventContext::Library);
                        let lean = if leafing { SKIP_STREAK_FACTOR } else { 1.0 };
                        let who = match origin {
                            Some(EventOrigin::Manual) => MANUAL_EARLY,
                            Some(_) => AUTO_EARLY,
                            None => 1.0,
                        };
                        let t = &mut self.tracks[i];
                        t.add(p.ts, R_EARLY * who * lean, W_SKIP_EARLY * lean);
                        t.early = t.early.saturating_add(1);
                        self.walk.end(p.track, Ended::Early);
                        if let Some(x) = self.pending.remove(&i).filter(|_| self.learn) {
                            self.weights.skip.observe(&x, true);
                        }
                    }
                    pos => {
                        self.streak = 0;
                        let heard = match (pos, self.duration[i]) {
                            (Some(pos), Some(d)) if d > 0 => (pos as f32 / d as f32).clamp(0.0, 1.0),
                            _ => 0.0,
                        };
                        let manual = origin == Some(EventOrigin::Manual);
                        let t = &mut self.tracks[i];
                        t.add(p.ts, R_LATE * heard * if manual { MANUAL_POSITIVE } else { 1.0 }, W_SKIP_LATE_HEARD * heard);
                        t.late = t.late.saturating_add(1);
                        self.walk.end(p.track, Ended::Late);
                        self.pending.remove(&i);
                    }
                }
            }
            EventType::Start => self.start(i, p, hook),
            _ => {}
        }
    }

    fn start(&mut self, i: usize, p: &Play, hook: Option<&mut dyn FnMut(&Self, &Moment, usize, &Play)>) {
        let ts = p.ts;
        // A start after a pause begins a session of its own.
        if self.walk.last_ts().is_none_or(|l| ts - l >= SESSION_GAP_MS) {
            self.walk = Session::default();
        }
        let m = self.moment(&self.walk, ts);
        let x = self.skip_features(&m, i, p.context);
        self.pending.insert(i, x);
        if m.prev.is_some() {
            let learn = self.learn && teaches(p.origin, p.context);
            match hook {
                Some(h) => {
                    h(self, &m, i, p);
                    if learn {
                        let all = (0..self.len()).filter(|&c| c != i).map(|c| self.features(&m, c)).collect();
                        let target = self.features(&m, i);
                        step(&mut self.weights.next, target, all);
                    }
                }
                None if learn => {
                    let mut next = self.weights.next;
                    self.teach_next(&mut next, &m, i);
                    self.weights.next = next;
                }
                None => {}
            }
        }
        // What this start says.
        for (k, (s, _, _)) in m.sess.iter().rev().take(3).enumerate() {
            self.co.add(*s, i, 0.5f32.powi(k as i32));
        }
        if let Some(prev) = m.prev
            && let (Some(wp), Some(wc)) = (self.world[prev], self.world[i])
        {
            *self.trans.entry(wp).or_default().entry(wc).or_default() += 1.0;
        }
        if let Some(prev) = m.prev
            && teaches(p.origin, p.context)
        {
            self.lane_trans[lane_index(self.types[prev].lane())][lane_index(self.types[i].lane())] += 1.0;
        }
        let part = self.part_of(ts);
        let t = &mut self.tracks[i];
        if let Some((t0, o)) = t.open.take() {
            // The start before never ended.
            let r = R_OPEN * if o == Some(EventOrigin::Manual) { MANUAL_POSITIVE } else { 1.0 };
            t.add(t0, r, W_START_ONLY);
        }
        if t.last_complete.is_some_and(|c| ts - c <= REPEAT_WITHIN_MS) {
            t.add(ts, R_REPEAT, W_REPEAT);
        }
        t.starts += 1;
        t.last_start = Some(ts);
        t.part[part] = t.part[part].saturating_add(1);
        t.open = Some((ts, p.origin));
        if let Some(w) = self.world[i] {
            let ws = self.worlds.entry(w).or_default();
            ws.hot = halve(ws.hot, ws.t_ref, ts, DAY_MS) + 1.0;
            ws.t_ref = ts.max(ws.t_ref);
            ws.last = Some(ts);
            ws.part[part] += 1;
            ws.n += 1;
        }
        self.part_total[part] += 1;
        self.walk.start(p.track, ts, p.context, p.origin);
    }
}

/// One step of `next` towards `target` over the [`EXPECT_OVER`] best of
/// the other candidates' features.
fn step(next: &mut Model, target: [f32; F], cands: Vec<[f32; F]>) {
    let mut scored: Vec<(f32, [f32; F])> = cands.into_iter().map(|f| (next.score(&f), f)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.truncate(EXPECT_OVER);
    let cands: Vec<[f32; F]> = scored.into_iter().map(|(_, f)| f).collect();
    next.observe(&target, &cands);
}

/// The area under the ROC curve of `(score, positive)` (ties count half).
pub fn auc(rows: &[(f32, bool)]) -> Option<f32> {
    let pos: Vec<f32> = rows.iter().filter(|r| r.1).map(|r| r.0).collect();
    let neg: Vec<f32> = rows.iter().filter(|r| !r.1).map(|r| r.0).collect();
    if pos.is_empty() || neg.is_empty() {
        return None;
    }
    let mut win = 0.0f64;
    for a in &pos {
        for b in &neg {
            win += if a > b { 1.0 } else if a == b { 0.5 } else { 0.0 };
        }
    }
    Some((win / (pos.len() * neg.len()) as f64) as f32)
}

/// Hits of a replay: how many of the real next starts were first, in the
/// first ten, and the mean reciprocal rank.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hits {
    pub n: usize,
    pub hit1: f32,
    pub hit10: f32,
    pub mrr: f32,
}

impl Hits {
    fn of(ranks: &[f32]) -> Self {
        let n = ranks.len().max(1) as f32;
        Self {
            n: ranks.len(),
            hit1: ranks.iter().filter(|r| **r <= 1.0).count() as f32 / n,
            hit10: ranks.iter().filter(|r| **r <= 10.0).count() as f32 / n,
            mrr: ranks.iter().map(|r| 1.0 / r).sum::<f32>() / n,
        }
    }
}

/// What [`replay`] measured (recs-v2 §13.1): the next start among the whole
/// catalog by the affinity of phase 1, the hand weights and the learnt
/// ones, on every next start, the free ones (not the next on the album),
/// the next ones on the album and the completed ones; the early skip on
/// the second half of the history.
#[derive(Clone, Debug, Default)]
pub struct ReplayReport {
    pub splits: Vec<(&'static str, &'static str, Hits)>,
    pub skip_auc: Option<f32>,
    pub skip_auc_affinity: Option<f32>,
    pub skip_auc_history: Option<f32>,
    pub skip_test: usize,
    pub weights: Weights,
}

impl ReplayReport {
    pub fn get(&self, split: &str, model: &str) -> Hits {
        self.splits.iter().find(|(s, m, _)| *s == split && *m == model).map_or_else(Hits::default, |x| x.2)
    }
}

/// 1-based rank of `scores[target]`, ties counted half.
fn rank_of(scores: &[f32], target: usize) -> f32 {
    let s = scores[target];
    let above = scores.iter().filter(|v| **v > s).count();
    let ties = scores.iter().filter(|v| **v == s).count() - 1;
    above as f32 + 1.0 + ties as f32 / 2.0
}

/// Replays `input`'s history in order of time, prequentially, from the
/// hand weights (see [`ReplayReport`]).
pub fn replay(catalog: &Catalog, input: &TasteInput, extra: ModelInput<'_>) -> ReplayReport {
    let events = ordered(&input.events);
    let (Some(first), Some(last)) = (events.first().map(|p| p.ts), events.last().map(|p| p.ts)) else {
        return ReplayReport::default();
    };
    let mid = first + (last - first) / 2;
    // The outcome of every start (for the splits), by its place in `events`.
    let mut outcome: HashMap<usize, Ended> = HashMap::new();
    let mut open: HashMap<TrackId, usize> = HashMap::new();
    for (k, p) in events.iter().enumerate() {
        match p.kind {
            EventType::Start => {
                open.insert(p.track, k);
                outcome.insert(k, Ended::Open);
            }
            EventType::Complete | EventType::Skip => {
                if let Some(j) = open.remove(&p.track) {
                    outcome.insert(j, match p.kind {
                        EventType::Complete => Ended::Complete,
                        _ if p.end_reason == Some(EndReason::AppClosed) => Ended::Open,
                        _ if p.position_ms.is_some_and(|x| x < EARLY_SKIP_MS) => Ended::Early,
                        _ => Ended::Late,
                    });
                }
            }
            _ => {}
        }
    }
    let taste = Taste::build(catalog, &TasteInput::default(), first);
    let mut m = UserModel::empty(catalog, taste, extra, input.utc_offset_min, first);
    m.learn = true;
    let mut ranks: HashMap<(&'static str, &'static str), Vec<f32>> = HashMap::new();
    let mut skips: Vec<(f32, f32, f32, bool)> = Vec::new();
    let mut k_of: HashMap<*const Play, usize> = HashMap::new();
    for (k, p) in events.iter().enumerate() {
        k_of.insert(*p as *const Play, k);
    }
    for p in &events {
        let k = k_of[&(*p as *const Play)];
        let ended = outcome.get(&k).copied().unwrap_or(Ended::Open);
        // The skip model's prediction before it learns from this start.
        if p.kind == EventType::Start && p.ts >= mid && matches!(ended, Ended::Complete | Ended::Early)
            && let Some(i) = m.position(p.track)
        {
            let walk = if m.walk.last_ts().is_none_or(|l| p.ts - l >= SESSION_GAP_MS) { Session::default() } else { m.walk.clone() };
            let mo = m.moment(&walk, p.ts);
            let x = m.skip_features(&mo, i, p.context);
            skips.push((m.weights.skip.p(&x), -x[1], -x[2] + x[4], ended == Ended::Early));
        }
        let mut hook = |model: &UserModel, mo: &Moment, target: usize, _: &Play| {
            let free = mo.next_album != Some(target);
            let mut splits: Vec<&'static str> = vec!["all", if free { "free" } else { "album_next" }];
            if ended == Ended::Complete {
                splits.push("completed");
                if free {
                    splits.push("completed_free");
                }
            }
            if p.ts >= mid {
                splits.push(if free { "free:second_half" } else { "album_next:second_half" });
            }
            let feats: Vec<[f32; F]> = (0..model.len()).map(|c| model.features(mo, c)).collect();
            let hand = Model::default();
            let scores = |w: &Model| -> Vec<f32> { feats.iter().map(|f| w.score(f)).collect() };
            let today: Vec<f32> = (0..model.len()).map(|c| model.tracks[c].aff_today(mo.now) + 1e-6 * (c as f32 / model.len() as f32)).collect();
            for (name, s) in [("phase1_affinity", today), ("hand", scores(&hand)), ("learned", scores(&model.weights.next))] {
                let r = rank_of(&s, target);
                for split in &splits {
                    ranks.entry((split, name)).or_default().push(r);
                }
            }
        };
        m.event(p, Some(&mut hook));
    }
    let mut splits: Vec<(&'static str, &'static str, Hits)> = ranks.into_iter().map(|((s, n), r)| (s, n, Hits::of(&r))).collect();
    splits.sort_by_key(|(s, n, _)| (*s, *n));
    let rows = |pick: fn(&(f32, f32, f32, bool)) -> f32| -> Vec<(f32, bool)> { skips.iter().map(|r| (pick(r), r.3)).collect() };
    ReplayReport {
        splits,
        skip_auc: auc(&rows(|r| r.0)),
        skip_auc_affinity: auc(&rows(|r| r.1)),
        skip_auc_history: auc(&rows(|r| r.2)),
        skip_test: skips.len(),
        weights: m.weights,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use nikgob_core::model::EventContext;

    use super::*;
    use crate::rng::Rng;
    use crate::test_util::*;

    pub(crate) const T0: UnixMs = 20_000 * DAY_MS;
    const ALBUM: u128 = 10;

    /// A listener of albums: 30 albums of ten tracks in ten worlds of three
    /// (each world its own sound), sessions of one world's albums in order,
    /// a favourite pair (after track 5 of an album often its track 9), a
    /// skip now and then, `days` of it.
    pub(crate) fn album_listener(days: i64, seed: u64) -> (Catalog, VectorIndex<TrackId>, TasteInput) {
        let mut rng = Rng::new(seed);
        let cat = Catalog::new((0..300u128).map(|i| {
            let album = i / ALBUM;
            info(i, &[(album / 3) as u32], Some(album))
                .with_world(&format!("w{}", album / 3))
                .with_position((i % ALBUM) as u32 + 1)
                .with_duration(Some(200_000))
        }));
        let mut idx = VectorIndex::with_weights([1.0; crate::DIM]);
        for i in 0..300u128 {
            let w = (i / ALBUM / 3) as f32;
            idx.insert(tid(i), point(w * 3.0 + rng.f32(), (w * 7.0) % 5.0 + rng.f32()));
        }
        let mut events = Vec::new();
        let mut ts = T0 - days * DAY_MS;
        while ts < T0 {
            let world = if rng.f32() < 0.6 { rng.below(3) as u128 } else { rng.below(10) as u128 };
            let mut album = world * 3 + rng.below(3) as u128;
            let mut track = rng.below(4) as u128;
            let mut ctx = EventContext::Album;
            for _ in 0..(4 + rng.below(8)) {
                let id = tid(album * ALBUM + track);
                events.push(Play { context: ctx, ..Play::new(id, ts, EventType::Start, Some(0)) });
                let early = rng.f32() < 0.1;
                ts += if early { 10_000 } else { 200_000 };
                let (kind, pos) = if early { (EventType::Skip, 10_000) } else { (EventType::Complete, 200_000) };
                events.push(Play { context: ctx, ..Play::new(id, ts, kind, Some(pos)) });
                ctx = EventContext::Queue;
                track = if track == 4 && rng.f32() < 0.7 { 8 } else { track + 1 };
                if track >= ALBUM {
                    album = world * 3 + (album + 1) % 3;
                    track = 0;
                    ctx = EventContext::Album;
                }
            }
            ts += (2 + rng.below(10) as i64) * HOUR_MS;
        }
        (cat, idx, TasteInput { events, ..TasteInput::default() })
    }

    pub(crate) fn input(idx: &VectorIndex<TrackId>) -> ModelInput<'_> {
        ModelInput { index: idx, types: None, weights: None }
    }

    #[test]
    fn learns_next_album() {
        let (cat, idx, history) = album_listener(20, 1);
        let m = UserModel::build(&cat, &history, input(&idx), T0);
        assert!(m.weights.next.seen > 200, "{}", m.weights.next.seen);
        assert!(m.weights.next.w[F_NEXT_ALBUM] > HAND[F_NEXT_ALBUM], "{:?}", m.weights.next.w);
        // Cached weights are kept as they are, the stats rebuilt.
        let again = UserModel::build(&cat, &history, ModelInput { weights: Some(m.weights), ..input(&idx) }, T0);
        assert_eq!(again.weights, m.weights);
        assert_eq!(again.tracks, m.tracks);
        // Weights of another feature version are learnt again.
        let old = Weights { version: FEATURE_VERSION + 1, ..Weights::default() };
        assert_eq!(UserModel::build(&cat, &history, ModelInput { weights: Some(old), ..input(&idx) }, T0).weights, m.weights);
    }

    /// The replay on the listener of albums (the replay test of a fixture;
    /// the real library's is `runtime/tests/real_replay.rs`): the learnt
    /// model finds the next start in its first ten in half of the starts at
    /// least, far more often than the affinity of phase 1.
    #[test]
    fn prequential_beats_affinity() {
        let (cat, idx, history) = album_listener(15, 2);
        let r = replay(&cat, &history, input(&idx));
        let (learned, hand, today) = (r.get("all", "learned"), r.get("all", "hand"), r.get("all", "phase1_affinity"));
        assert!(learned.n > 300, "{learned:?}");
        assert!(learned.hit10 >= 0.5, "hit@10 {learned:?}");
        assert!(learned.hit10 > today.hit10 + 0.2 && learned.mrr > today.mrr, "{learned:?} against {today:?}");
        assert!(learned.mrr >= hand.mrr - 0.02, "{learned:?} against {hand:?}");
        // The favourite pair is learnt: what follows inside sessions.
        assert!(r.weights.next.w[F_COOC] > HAND[F_COOC], "{:?}", r.weights.next.w);
    }

    /// An early skip makes the next more likely to be skipped too.
    #[test]
    fn prev_early_predicts() {
        let mut rng = Rng::new(3);
        let cat = Catalog::new((0..200u128).map(|i| info(i, &[i as u32 % 20], Some(i / 10)).with_duration(Some(200_000))));
        let idx = VectorIndex::new();
        let mut events = Vec::new();
        let mut ts = T0 - 30 * DAY_MS;
        let mut prev_early = false;
        for k in 0..3_000u128 {
            if k % 12 == 0 {
                ts += 3 * HOUR_MS;
                prev_early = false;
            }
            let id = tid(rng.below(200) as u128);
            events.push(Play { context: EventContext::Playlist, ..Play::new(id, ts, EventType::Start, Some(0)) });
            let early = rng.f32() < if prev_early { 0.6 } else { 0.1 };
            ts += if early { 5_000 } else { 200_000 };
            let (kind, pos) = if early { (EventType::Skip, 5_000) } else { (EventType::Complete, 200_000) };
            events.push(Play::new(id, ts, kind, Some(pos)));
            prev_early = early;
        }
        let history = TasteInput { events, ..TasteInput::default() };
        let r = replay(&cat, &history, input(&idx));
        let w = r.weights.skip.w;
        assert!(w[21] > 0.5, "prev_early {w:?}");
        assert!(r.skip_auc.unwrap() > 0.65, "{:?}", r.skip_auc);
        let m = UserModel::build(&cat, &history, input(&idx), ts);
        assert!(m.weights.skip.seen > 2_000);
    }

    /// The outcome weights of §4.1: a late skip is a little plus, the app
    /// closing on a track adds nothing, an early skip of the app's pick
    /// weighs more than one of the user's own.
    #[test]
    fn outcomes_weigh_as_listened() {
        let cat = Catalog::new((0..4u128).map(|i| info(i, &[0], None).with_duration(Some(200_000))));
        let idx = VectorIndex::new();
        let start = |i: u128, o: EventOrigin, ts| Play { origin: Some(o), ..Play::new(tid(i), ts, EventType::Start, Some(0)) };
        let events = vec![
            start(0, EventOrigin::Manual, T0 - 100_000),
            Play::new(tid(0), T0 - 90_000, EventType::Skip, Some(5_000)),
            start(1, EventOrigin::Radio, T0 - 80_000),
            Play::new(tid(1), T0 - 70_000, EventType::Skip, Some(5_000)),
            start(2, EventOrigin::Manual, T0 - 60_000),
            Play { end_reason: Some(EndReason::AppClosed), ..Play::new(tid(2), T0 - 50_000, EventType::Skip, Some(5_000)) },
            start(3, EventOrigin::Queue, T0 - 40_000),
            Play::new(tid(3), T0 - 30_000, EventType::Skip, Some(150_000)),
        ];
        let m = UserModel::build(&cat, &TasteInput { events, ..TasteInput::default() }, input(&idx), T0);
        let a = |i: usize| m.tracks[i].aff_long(T0);
        assert!(a(0) < 0.0 && a(1) < a(0), "the app's pick skipped weighs more: {} {}", a(0), a(1));
        assert!(a(2).abs() < 1e-6, "closing the app says nothing: {}", a(2));
        assert!(a(3) > 0.0, "a late skip is a little plus: {}", a(3));
        assert!(m.tracks[2].open.is_none());
    }
}
