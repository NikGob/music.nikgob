//! The stats of the history and the two models that learn online: the per-track, per-world and pair counts with their constants, the model of the next start and the model of the early skip, and their weights.

use nikgob_core::model::{EventContext, EventOrigin, EventType, UnixMs};
use serde::{Deserialize, Serialize};

use crate::songtype::{SongLane, sigmoid};
use crate::taste::{DAY_MS, HOUR_MS, Play, W_LIKE, W_START_ONLY};

/// Bumped when a feature changes meaning: cached weights of another
/// version are learnt again from the history.
pub const FEATURE_VERSION: u32 = 6;

pub const H_LONG_MS: i64 = 45 * DAY_MS;
pub const H_HOT_MS: i64 = 2 * DAY_MS;
pub(super) const R_COMPLETE: f32 = 1.0;
pub(super) const R_LATE: f32 = 0.15;
pub(super) const R_EARLY: f32 = -0.8;
pub(super) const R_OPEN: f32 = 0.1;
pub(super) const R_REPEAT: f32 = 0.7;
const R_LIKE: f32 = 2.0;
/// A track is loved for long with this much long affinity (about three
/// completes of late) over this many completes, or a like.
const LOVED_AFF: f32 = 0.75;
const LOVED_COMPLETES: u32 = 3;
/// A song saved in a service's library, not liked (an import's «music
/// library songs»): long taste without decay.
const R_SAVED: f32 = 1.0;
pub(super) const MANUAL_POSITIVE: f32 = 1.2;
/// A like or a save is a state, not a moment: it counts whole while its
/// song was heard within this of the user's latest listening (a play of
/// any origin, a like set by hand)…
pub const FRESH_GRACE_MS: i64 = 30 * DAY_MS;
/// …then fades by half every this long…
pub const FRESH_HALF_MS: i64 = 60 * DAY_MS;
/// …down to this.
pub const STALE_FLOOR: f32 = 0.35;
/// What is loved by a state alone (a like, a save) and never heard counts
/// this much, once anything was heard.
pub const UNHEARD_LOVE: f32 = 0.5;
/// A loved song not heard for this long is an old one.
pub const STALE_MS: i64 = 90 * DAY_MS;
/// This many likes within [`BULK_WINDOW_MS`] of one another came from a
/// service's library at once: they say nothing of when the songs were
/// loved.
pub const BULK_LIKES: usize = 20;
pub const BULK_WINDOW_MS: i64 = HOUR_MS;
/// The last this many outcomes of a track are kept ([`TrackStats::burnt`])…
const RECENT_LOOK: u8 = 6;
/// …and from this many on, early skips in most of them say it is worn out
/// now, whatever its love: its chance of an early skip is at least theirs,
/// shrunk to the model's by this many outcomes' weight.
const BURNT_FROM: u8 = 3;
const BURNT_PRIOR: f32 = 2.0;
pub(super) const MANUAL_EARLY: f32 = 0.6;
pub(super) const AUTO_EARLY: f32 = 1.3;
/// Track → track pairs kept per track.
pub const CO_TOP: usize = 50;
/// A play imported from another service counts this much in the taste
/// profiles' shares beside the user's own plays ([`UserModel::settle`]).
pub const IMPORT_SHARE: f32 = 0.25;
/// Tracks of one session this many steps apart at most are listened
/// together ([`F_SESS_CO`]), `1 / k` for k steps.
pub(super) const BAG_WINDOW: usize = 10;
/// A start shorter than this is an interlude.
pub(super) const INTERLUDE_MS: u32 = 75_000;
/// A track longer than this is long (the skip model).
pub(super) const LONG_MS: u32 = 6 * 60_000;
/// Song lanes ([`lane_index`]).
pub(super) const LANES: usize = 5;
/// The mass of the taste profiles: a play's outcome
/// weighs this, halving in [`H_MASS_MS`]…
pub(super) const MASS_COMPLETE: f32 = 1.0;
pub(super) const MASS_LATE: f32 = 0.5;
pub(super) const MASS_OPEN: f32 = 0.3;
pub(super) const MASS_EARLY: f32 = -0.5;
pub const H_MASS_MS: i64 = 90 * DAY_MS;
/// …a like and a playlist entry this.
pub(super) const MASS_LIKE: f32 = 2.0;
pub(super) const MASS_PLAYLIST: f32 = 1.5;
pub(super) const MASS_SAVED: f32 = 1.0;
/// A like counts as a touch of the play it fell in within this.
pub(super) const LIKE_IN_PLAY_MS: i64 = 20 * 60_000;
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
pub const FEATURES: [&str; 22] = [
    "aff_long", "aff_hot", "liked", "sound", "skip_push", "next_album", "same_album", "same_world", "world_trans",
    "cooc", "in_session", "recent3h", "daypart_world", "same_lane", "never", "interlude", "graph", "pgraph", "prior",
    "sess_co", "is_prev", "album_first",
];
pub const F: usize = FEATURES.len();
pub const F_SOUND: usize = 3;
pub const F_SKIP_PUSH: usize = 4;
pub const F_NEXT_ALBUM: usize = 5;
pub const F_SAME_ALBUM: usize = 6;
pub const F_SAME_WORLD: usize = 7;
pub const F_COOC: usize = 9;
/// The track played in the session already.
pub const F_IN_SESSION: usize = 10;
pub const F_SAME_LANE: usize = 13;
/// The collective graph from the session ([`crate::graph`]), the personal
/// graph, the prior of the library.
pub const F_GRAPH: usize = 16;
pub const F_PGRAPH: usize = 17;
pub const F_PRIOR: usize = 18;
/// Listened in the same sessions as the session's tracks (either order,
/// [`BAG_WINDOW`] steps apart at most); the very track playing now again;
/// the first track of an album other than the one playing.
pub const F_SESS_CO: usize = 19;
pub const F_IS_PREV: usize = 20;
pub const F_ALBUM_FIRST: usize = 21;
/// The hand weights (`scripts/replay.py`, `HAND`; the graph's from the
/// synthetic listeners): the prior and the start.
pub const HAND: [f32; F] = [1.0, 1.2, 0.3, 0.8, 1.0, 3.0, 1.0, 0.8, 1.5, 1.5, -2.0, 0.3, 1.0, 0.8, -0.3, -0.5, 1.5, 1.0, 0.5, 0.5, 0.0, 0.3];
/// What the replay of 15 days learnt from them (`out/replay.json`), for
/// reference and tests (the graph's were not there then: the hand ones).
pub const LEARNED: [f32; F] = [0.68, -0.1, 0.94, 1.15, 0.0, 3.62, 0.52, 0.65, 1.64, 1.77, -0.1, 0.7, 0.77, 0.66, -1.1, -0.03, 1.5, 1.0, 0.5, 0.5, 0.0, 0.3];
/// An artist like the session's counts this much of a track like it.
pub(super) const GRAPH_ARTIST: f32 = 0.5;
/// Candidates by the graphs: the neighbours of the session…
pub(super) const GRAPH_CANDIDATES: usize = 50;
/// …and the best known tracks of this many artists like it, so many each.
pub(super) const GRAPH_ARTISTS: usize = 6;
pub(super) const GRAPH_ARTIST_TRACKS: usize = 4;
/// The library's prior as the profiles' mass ([`crate::graph`]), while the
/// history is short: this much at most, fading as `1 / (1 + starts / COLD_STARTS)`.
pub(super) const COLD_MASS: f32 = 0.3;
pub(super) const COLD_STARTS: f32 = 30.0;
/// From this many starts the history alone is the taste (the prior would
/// only blur the profiles of the tracks it gathers).
pub(super) const COLD_UNTIL: usize = 300;
const LR: f32 = 0.05;
const L2: f32 = 0.01;
/// The step is halved until this many starts taught the model.
pub const FULL_STEP_AFTER: u32 = 300;
/// The expectation of a step is taken over this many best candidates.
pub const EXPECT_OVER: usize = 100;
/// A build learning the weights anew (none cached, or of another
/// [`FEATURE_VERSION`]) steps on this many latest starts the user chose.
pub const TEACH_LAST: usize = 1_500;

/// The features of a start for its early skip, in this order
/// (`new_service`: new music from a service; `background`: the session's
/// `p_bg` so far — not skipped because not
/// listened to).
pub const SKIP_FEATURES: [&str; 25] = [
    "bias", "aff_today", "aff_long", "aff_hot", "early_rate", "never", "liked", "pos0", "pos_log", "ctx_album",
    "ctx_playlist", "ctx_artist", "ctx_home", "same_album", "same_world", "sound_jump", "night", "ost", "vocal",
    "long", "interlude", "prev_early", "prev2_early", "new_service", "background",
];
pub const G_NEW_SERVICE: usize = 23;
pub const G_BACKGROUND: usize = 24;
pub const G: usize = SKIP_FEATURES.len();
const SKIP_LR: f32 = 0.08;
const SKIP_L2: f32 = 0.001;

/// What the history says about one track (by catalog position).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackStats {
    pub(super) a_long: f32,
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
    /// Its completes by the part of the day ([`DayPart::index`]).
    pub done: [u16; 4],
    /// A start not ended yet, and who chose it…
    pub(super) open: Option<(UnixMs, Option<EventOrigin>)>,
    /// …and the weight of its context ([`crate::context`]).
    pub(super) open_w: f32,
    pub(super) liked: Option<UnixMs>,
    /// Saved in a service's library (see [`R_SAVED`]).
    pub(super) saved: bool,
    /// How much of a like's (or a save's) love no longer counts: `1 −` its
    /// freshness ([`UserModel::freshness_at`], set once the history is
    /// walked; 0 while it is).
    pub(super) love_stale: f32,
    /// Its last [`RECENT_LOOK`] outcomes, the latest in the lowest bit (1:
    /// an early skip), and how many there are.
    recent: u8,
    recent_n: u8,
}

/// The bins of the time between two completes of one song
/// ([`UserModel::repeat_rest`]): 2 h, 6 h, a day, three days, a week.
pub const REPEAT_BINS: [i64; 5] = [2 * HOUR_MS, 6 * HOUR_MS, 24 * HOUR_MS, 72 * HOUR_MS, 168 * HOUR_MS];
/// The time between repeats of a user whose repeats are not known yet
/// (fewer than [`REPEAT_MIN`]).
pub const REPEAT_DEFAULT_MS: i64 = 24 * HOUR_MS;
pub const REPEAT_MIN: u32 = 20;
/// A file come to the library this long ago is a new arrival…
pub const ARRIVAL_WINDOW_MS: i64 = 30 * DAY_MS;
/// …half as much of one after this; a day that brought more files than
/// this is an import of the library, none of them a new arrival.
pub const ARRIVAL_HALF_MS: i64 = 7 * DAY_MS;
pub const ARRIVAL_BATCH: usize = 100;

/// How much of a love last heard `age` before the user's latest listening
/// counts now ([`UserModel::freshness_at`]).
pub fn freshness(age: i64) -> f32 {
    if age <= FRESH_GRACE_MS {
        return 1.0;
    }
    let halves = (age - FRESH_GRACE_MS) as f64 / FRESH_HALF_MS as f64;
    STALE_FLOOR + (1.0 - STALE_FLOOR) * (-halves).exp2() as f32
}

pub(super) fn halve(x: f32, from: UnixMs, to: UnixMs, half: i64) -> f32 {
    if to <= from {
        return x;
    }
    x * (-((to - from) as f64) / half as f64).exp2() as f32
}

impl TrackStats {
    /// `r` of an outcome at `ts` (`today` the weight of [`crate::taste`]),
    /// the hot affinity left out unless `hot` (an imported play is long
    /// taste only).
    pub(super) fn add_as(&mut self, ts: UnixMs, r: f32, today: f32, hot: bool) {
        let ts = ts.max(self.t_ref);
        self.a_long = halve(self.a_long, self.t_ref, ts, H_LONG_MS) + r;
        self.a_hot = halve(self.a_hot, self.t_ref, ts, H_HOT_MS) + if hot { r } else { 0.0 };
        self.a_today = halve(self.a_today, self.t_ref, ts, crate::taste::HALF_LIFE_MS) + today;
        self.t_ref = ts;
    }

    /// `r` of an outcome at `ts` on the hot affinity alone: the session's
    /// mood, nothing in the long run («not now», [`crate::session::Verdict::NotNow`]).
    pub(super) fn add_hot(&mut self, ts: UnixMs, r: f32) {
        let ts = ts.max(self.t_ref);
        self.a_long = halve(self.a_long, self.t_ref, ts, H_LONG_MS);
        self.a_hot = halve(self.a_hot, self.t_ref, ts, H_HOT_MS) + r;
        self.a_today = halve(self.a_today, self.t_ref, ts, crate::taste::HALF_LIFE_MS);
        self.t_ref = ts;
    }

    /// An outcome: skipped early or not ([`Self::burnt`]).
    pub(super) fn note(&mut self, early: bool) {
        self.recent = (self.recent << 1) | u8::from(early);
        self.recent_n = (self.recent_n + 1).min(RECENT_LOOK);
    }

    /// Worn out now: skipped early in most of its last outcomes (at least
    /// [`BURNT_FROM`] of them), whatever its love — their share of early
    /// skips shrunk towards `p` (the model's chance) by [`BURNT_PRIOR`]
    /// outcomes. `None` when it is not.
    pub fn burnt(&self, p: f32) -> Option<f32> {
        let n = self.recent_n.min(RECENT_LOOK);
        if n < BURNT_FROM {
            return None;
        }
        let early = (u32::from(self.recent) & ((1u32 << n) - 1)).count_ones() as f32;
        (2.0 * early > f32::from(n)).then(|| (early + BURNT_PRIOR * p) / (f32::from(n) + BURNT_PRIOR))
    }

    /// How much of a like's love counts now (see [`Self::love_stale`]).
    pub fn love_fresh(&self) -> f32 {
        1.0 - self.love_stale
    }

    /// Loved for long at `now`: liked, or [`LOVED_AFF`] of long affinity
    /// over [`LOVED_COMPLETES`] completes at least.
    pub fn loved(&self, now: UnixMs) -> bool {
        self.liked(now) || (self.completes >= LOVED_COMPLETES && self.aff_long(now) >= LOVED_AFF)
    }

    /// Never heard before, or this once only.
    pub fn unfamiliar(&self, now: UnixMs) -> bool {
        !self.liked(now) && !self.saved && self.completes == 0 && self.starts <= 1
    }

    /// The long affinity at `now`, `tanh(A / 3)`: a like (or a save) as
    /// much as it counts now ([`Self::love_fresh`]: an old love and a like
    /// of a bulk never heard count less).
    pub fn aff_long(&self, now: UnixMs) -> f32 {
        self.aff_long_by(now, self.love_fresh())
    }

    /// [`Self::aff_long`] with a like counting whole whenever it was (the
    /// mixes weigh its freshness themselves, with the rest of a love).
    pub fn aff_long_whole(&self, now: UnixMs) -> f32 {
        self.aff_long_by(now, 1.0)
    }

    fn aff_long_by(&self, now: UnixMs, fresh: f32) -> f32 {
        let mut a = halve(self.a_long, self.t_ref, now, H_LONG_MS);
        if let Some((t, _)) = self.open {
            a += halve(R_OPEN, t, now, H_LONG_MS) * self.open_w;
        }
        if self.liked.is_some_and(|t| t <= now) {
            a += R_LIKE * fresh;
        } else if self.saved {
            a += R_SAVED * fresh;
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
            a += halve(W_START_ONLY, t, now, crate::taste::HALF_LIFE_MS) * self.open_w;
        }
        if self.liked.is_some_and(|t| t <= now) {
            a += W_LIKE;
        } else if self.saved {
            a += crate::taste::W_SAVED;
        }
        (a / 3.0).tanh()
    }

    pub fn liked(&self, now: UnixMs) -> bool {
        self.liked.is_some_and(|t| t <= now)
    }
}

/// A world's presence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldStats {
    pub(super) hot: f32,
    pub(super) t_ref: UnixMs,
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
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoOccur {
    next: Vec<Vec<(u32, f32)>>,
}

impl CoOccur {
    pub(super) fn new(n: usize) -> Self {
        Self { next: vec![Vec::new(); n] }
    }

    pub(super) fn add(&mut self, a: usize, b: usize, w: f32) {
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

    /// What was kept of `a`'s pairs, in no order.
    pub fn row(&self, a: usize) -> &[(u32, f32)] {
        self.next.get(a).map_or(&[], Vec::as_slice)
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

/// The time of the [`TEACH_LAST`]-th last start of `events` that teaches
/// the model of the next start: a build learning the weights anew steps on
/// those only (a long history would take minutes, and the oldest steps
/// would fade under the later ones anyway).
pub(super) fn teach_from(events: &[Play]) -> UnixMs {
    let mut ts: Vec<UnixMs> = events
        .iter()
        .filter(|p| p.kind == EventType::Start && p.origin != Some(EventOrigin::Import) && teaches(p.origin, p.context))
        .map(|p| p.ts)
        .collect();
    if ts.len() <= TEACH_LAST {
        return UnixMs::MIN;
    }
    let k = ts.len() - TEACH_LAST;
    *ts.select_nth_unstable(k).1
}

/// Whether a start teaches the model of the next start: the user chose
/// it (a history older than origins: anything but a radio's). An imported
/// play never does.
pub fn teaches(origin: Option<EventOrigin>, context: EventContext) -> bool {
    match origin {
        Some(o) => !o.is_auto(),
        None => context != EventContext::Radio,
    }
}
