//! The user model: what the history says about every track,
//! world and pair of tracks, a model of the next start and a model of the
//! early skip, both learning online.
//!
//! * [`TrackStats`]: per track two sums of outcome weights, halving in 45
//!   days and in 2 days, counts of starts and outcomes, the last
//!   start and complete, starts by part of the day. An outcome weighs:
//!   complete +1, late skip +0.15 × the share heard, early skip
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
//!   chose never teach it (the model would learn its own picks). A build
//!   learning the weights anew steps on the latest [`TEACH_LAST`] starts
//!   only (a history of 135 000 events took over two minutes; the oldest
//!   steps fade under the later ones anyway). Beside the album, the world,
//!   the sound and the pairs, it sees what was listened together with the
//!   session's tracks in the sessions before ([`F_SESS_CO`]), the track
//!   playing now again ([`F_IS_PREV`]: some play one song on repeat) and
//!   another album's first track ([`F_ALBUM_FIRST`]: an album after an
//!   album), each weighed by what the user's own choices teach.
//! * [`SkipModel`]: online logistic regression of an early skip on
//!   [`SKIP_FEATURES`], a step on every start that completed or was skipped
//!   early, whoever chose it.
//!
//! * Once the history is walked ([`UserModel::settle`]): how the user
//!   listens ([`crate::listening`]: the background sessions, if any), the
//!   taste profiles ([`crate::profiles`]) by the plays' outcomes, a
//!   background play counting [`crate::listening::W_BG`], their shares of
//!   the recent plays, the regions of sound they make ([`crate::regions`])
//!   and how much lower each track stands for the dislikes near it
//!   ([`crate::dislike`]).
//! * Imported plays ([`EventOrigin::Import`], another service's history)
//!   teach the long taste alone: the long affinity of their tracks (and
//!   the profiles' mass). Never the session, the pairs, the transitions,
//!   the models or the hot affinity: those are this app's own listening.
//! * A song saved in a service's library and not liked here (an import's
//!   «music library songs») is long taste too: [`R_SAVED`] on its long
//!   affinity and [`MASS_SAVED`] on the profiles' mass, a state like a like.
//!   One the catalog lacks gives its [`MASS_SAVED`] to the catalog's tracks
//!   of its artist, shared among them: the profile of that artist's sound
//!   weighs more.
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
use crate::graph::{GraphInput, SimGraph};
use crate::index::{Normalizer, VectorIndex, euclid};
use crate::listening::{Heard, Listening, SessionSignals, W_BG};
use crate::profiles::{
    ALIVE_DAYS, Input as ProfilesInput, Params, Profiles, SHARE_HALF_DAYS, SharePlay, background_cards, background_shares, quota,
    shares,
};
use crate::regions::Regions;
use crate::session::{Ended, Familiarity, NOT_NOW_MS, Session, TrackLookup, in_stream, skip_ended};
use crate::songtype::{SongLane, SongType, SongTypes, sigmoid};
use crate::taste::{
    Catalog, DAY_MS, DayPart, EARLY_SKIP_MS, HOUR_MS, Play, REPEAT_WITHIN_MS, SESSION_GAP_MS, SKIP_STREAK_FACTOR,
    SKIP_STREAK_FROM, Taste, TasteInput, W_COMPLETE, W_LIKE, W_REPEAT, W_SKIP_EARLY, W_SKIP_LATE_HEARD, W_START_ONLY,
};

/// Bumped when a feature changes meaning: cached weights of another
/// version are learnt again from the history.
pub const FEATURE_VERSION: u32 = 6;

pub const H_LONG_MS: i64 = 45 * DAY_MS;
pub const H_HOT_MS: i64 = 2 * DAY_MS;
const R_COMPLETE: f32 = 1.0;
const R_LATE: f32 = 0.15;
const R_EARLY: f32 = -0.8;
const R_OPEN: f32 = 0.1;
const R_REPEAT: f32 = 0.7;
const R_LIKE: f32 = 2.0;
/// A track is loved for long with this much long affinity (about three
/// completes of late) over this many completes, or a like.
const LOVED_AFF: f32 = 0.75;
const LOVED_COMPLETES: u32 = 3;
/// A song saved in a service's library, not liked (an import's «music
/// library songs»): long taste without decay.
const R_SAVED: f32 = 1.0;
const MANUAL_POSITIVE: f32 = 1.2;
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
/// A song the app chose, left within this of its start (the app closed on
/// it, from [`ABANDON_FROM_MS`] on: what plays for a moment says nothing),
/// was skipped early.
pub const ABANDON_MS: u32 = 25_000;
pub const ABANDON_FROM_MS: u32 = 3_000;
const MANUAL_EARLY: f32 = 0.6;
const AUTO_EARLY: f32 = 1.3;
/// Track → track pairs kept per track.
pub const CO_TOP: usize = 50;
/// A play imported from another service counts this much in the taste
/// profiles' shares beside the user's own plays ([`UserModel::settle`]).
pub const IMPORT_SHARE: f32 = 0.25;
/// Tracks of one session this many steps apart at most are listened
/// together ([`F_SESS_CO`]), `1 / k` for k steps.
const BAG_WINDOW: usize = 10;
/// A start shorter than this is an interlude.
const INTERLUDE_MS: u32 = 75_000;
/// A track longer than this is long (the skip model).
const LONG_MS: u32 = 6 * 60_000;
/// Song lanes ([`lane_index`]).
const LANES: usize = 5;
/// The mass of the taste profiles: a play's outcome
/// weighs this, halving in [`H_MASS_MS`]…
const MASS_COMPLETE: f32 = 1.0;
const MASS_LATE: f32 = 0.5;
const MASS_OPEN: f32 = 0.3;
const MASS_EARLY: f32 = -0.5;
pub const H_MASS_MS: i64 = 90 * DAY_MS;
/// …a like and a playlist entry this.
const MASS_LIKE: f32 = 2.0;
const MASS_PLAYLIST: f32 = 1.5;
const MASS_SAVED: f32 = 1.0;
/// A like counts as a touch of the play it fell in within this.
const LIKE_IN_PLAY_MS: i64 = 20 * 60_000;
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
const GRAPH_ARTIST: f32 = 0.5;
/// Candidates by the graphs: the neighbours of the session…
const GRAPH_CANDIDATES: usize = 50;
/// …and the best known tracks of this many artists like it, so many each.
const GRAPH_ARTISTS: usize = 6;
const GRAPH_ARTIST_TRACKS: usize = 4;
/// The library's prior as the profiles' mass ([`crate::graph`]), while the
/// history is short: this much at most, fading as `1 / (1 + starts / COLD_STARTS)`.
const COLD_MASS: f32 = 0.3;
const COLD_STARTS: f32 = 30.0;
/// From this many starts the history alone is the taste (the prior would
/// only blur the profiles of the tracks it gathers).
const COLD_UNTIL: usize = 300;
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
    /// Its completes by the part of the day ([`DayPart::index`]).
    pub done: [u16; 4],
    /// A start not ended yet, and who chose it.
    open: Option<(UnixMs, Option<EventOrigin>)>,
    liked: Option<UnixMs>,
    /// Saved in a service's library (see [`R_SAVED`]).
    saved: bool,
    /// How much of a like's (or a save's) love no longer counts: `1 −` its
    /// freshness ([`UserModel::freshness_at`], set once the history is
    /// walked; 0 while it is).
    love_stale: f32,
    /// Its last [`RECENT_LOOK`] outcomes, the latest in the lowest bit (1:
    /// an early skip), and how many there are.
    recent: u8,
    recent_n: u8,
}

/// How much of a love last heard `age` before the user's latest listening
/// counts now ([`UserModel::freshness_at`]).
pub fn freshness(age: i64) -> f32 {
    if age <= FRESH_GRACE_MS {
        return 1.0;
    }
    let halves = (age - FRESH_GRACE_MS) as f64 / FRESH_HALF_MS as f64;
    STALE_FLOOR + (1.0 - STALE_FLOOR) * (-halves).exp2() as f32
}

fn halve(x: f32, from: UnixMs, to: UnixMs, half: i64) -> f32 {
    if to <= from {
        return x;
    }
    x * (-((to - from) as f64) / half as f64).exp2() as f32
}

impl TrackStats {
    /// `r` of an outcome at `ts` (`today` the weight of [`crate::taste`]),
    /// the hot affinity left out unless `hot` (an imported play is long
    /// taste only).
    fn add_as(&mut self, ts: UnixMs, r: f32, today: f32, hot: bool) {
        let ts = ts.max(self.t_ref);
        self.a_long = halve(self.a_long, self.t_ref, ts, H_LONG_MS) + r;
        self.a_hot = halve(self.a_hot, self.t_ref, ts, H_HOT_MS) + if hot { r } else { 0.0 };
        self.a_today = halve(self.a_today, self.t_ref, ts, crate::taste::HALF_LIFE_MS) + today;
        self.t_ref = ts;
    }

    /// `r` of an outcome at `ts` on the hot affinity alone: the session's
    /// mood, nothing in the long run («not now», [`crate::session::Verdict::NotNow`]).
    fn add_hot(&mut self, ts: UnixMs, r: f32) {
        let ts = ts.max(self.t_ref);
        self.a_long = halve(self.a_long, self.t_ref, ts, H_LONG_MS);
        self.a_hot = halve(self.a_hot, self.t_ref, ts, H_HOT_MS) + r;
        self.a_today = halve(self.a_today, self.t_ref, ts, crate::taste::HALF_LIFE_MS);
        self.t_ref = ts;
    }

    /// An outcome: skipped early or not ([`Self::burnt`]).
    fn note(&mut self, early: bool) {
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
            a += halve(R_OPEN, t, now, H_LONG_MS);
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
            a += halve(W_START_ONLY, t, now, crate::taste::HALF_LIFE_MS);
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
#[derive(Clone, Debug, Default, PartialEq)]
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
fn teach_from(events: &[Play]) -> UnixMs {
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

/// What [`UserModel::build`] reads besides the catalog and the taste's input.
#[derive(Clone, Copy)]
pub struct ModelInput<'a> {
    pub index: &'a VectorIndex<TrackId>,
    pub types: Option<&'a SongTypes>,
    /// The weights to go on with (else learnt again from the history).
    pub weights: Option<Weights>,
    /// The collective graph of similar music ([`crate::graph`]), resolved
    /// to the catalog; none: «Похожее по слушателям» is off or nothing is
    /// known yet.
    pub graph: Option<&'a GraphInput>,
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
    /// The sounds the session rejected and how hard each pushes.
    pub pushes: Vec<(Vector, f32)>,
    pub lane: Option<SongLane>,
    pub part: usize,
    pub skip_streak: u8,
    pub album_session: bool,
    pub world_session: Option<u64>,
    pub repeat_one: bool,
    /// How much the session so far is background ([`Listening::p_now`]).
    pub p_bg: f32,
    /// The collective graph's neighbours of the session's last tracks (not
    /// skipped early), by catalog position, and the artists like theirs
    /// (not theirs), each by `0.6^k` of the k-th from the end…
    pub graph_near: HashMap<usize, f32>,
    pub graph_artists: HashMap<ArtistId, f32>,
    /// …and the personal graph's.
    pub pgraph_near: HashMap<usize, f32>,
    /// What was listened in the same sessions as the session's tracks
    /// ([`F_SESS_CO`]), each of those by `0.8^k` of the k-th from the end.
    pub bag_near: HashMap<usize, f32>,
}

/// The rejections of a session measured by the sound index
/// ([`UserModel::push_probe`]).
pub struct PushProbe<'a> {
    index: &'a VectorIndex<TrackId>,
    probe: crate::index::SoundProbe,
    pushed: Vec<(TrackId, f32)>,
}

/// What a live session is ([`UserModel::live`]): one profile or many,
/// how much of it is background, whether it is focused, and the profile of
/// its last start.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Live {
    pub mode: Option<crate::listening::Mode>,
    pub p_bg: f32,
    pub focus: bool,
    pub profile: Option<usize>,
}

/// A start of the history as [`UserModel::settle`] reads it.
struct Started {
    ts: UnixMs,
    track: TrackId,
    pos: Option<usize>,
    ended: Ended,
    end: Option<UnixMs>,
    origin: Option<EventOrigin>,
    context: EventContext,
    touches: u32,
    attention: Option<f32>,
    import: bool,
}

/// The home's cards of the taste profiles ([`UserModel::home_profiles`]):
/// how many each one takes, and a background profile's cards as its tracks
/// (catalog positions), the calm first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HomeProfiles {
    pub cards: Vec<usize>,
    pub background: Vec<Vec<Vec<usize>>>,
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
    /// The catalog positions of each main artist.
    by_artist: HashMap<ArtistId, Vec<u32>>,
    duration: Vec<Option<u32>>,
    types: Vec<SongType>,
    next_in_album: Vec<Option<u32>>,
    /// The first track of its album.
    album_first: Vec<bool>,
    pub worlds: HashMap<u64, WorldStats>,
    part_total: [u32; 4],
    trans: HashMap<u64, HashMap<u64, f32>>,
    /// The transitions out of each world in all.
    trans_total: HashMap<u64, f32>,
    /// The catalog positions of each world.
    by_world: HashMap<u64, Vec<u32>>,
    /// Song lane → song lane of the starts the user chose, by
    /// [`lane_index`].
    lane_trans: [[f32; LANES]; LANES],
    pub co: CoOccur,
    /// Tracks listened in the same sessions, either order ([`BAG_WINDOW`]).
    pub bag: CoOccur,
    /// The collective graph and the personal one ([`crate::graph`]).
    pub graph: SimGraph,
    personal: Vec<Vec<(u32, f32)>>,
    /// How cold the history is: 1 with none, `1 / (1 + starts / COLD_STARTS)`,
    /// 0 from [`COLD_UNTIL`] starts.
    cold: f32,
    /// «Без истории — по самой библиотеке».
    library_prior: bool,
    /// The taste profiles ([`crate::profiles`]), the profile of each track,
    /// the profiles' shares of the recent plays (a background play counting
    /// little), the mean `p_bg` of each one's plays and the best long
    /// affinity in each.
    pub profiles: Profiles,
    pub profile_of: Vec<Option<u16>>,
    pub profile_share: Vec<f32>,
    pub profile_bg: Vec<f32>,
    profile_top: Vec<f32>,
    /// How the user listens: the background sessions, if any.
    pub listening: Listening,
    pub weights: Weights,
    /// The median distance between tracks.
    scale: f32,
    norm: Normalizer,
    /// The regions of sound of the taste profiles.
    pub regions: Regions,
    /// When each track was last heard (a play of any origin, a like set by
    /// hand: not one of [`BULK_LIKES`] within an hour) and the latest of
    /// them ([`Self::freshness_at`]).
    last_heard: Vec<Option<UnixMs>>,
    latest_heard: Option<UnixMs>,

    /// The embedded vectors of the disliked tracks (a new track near them
    /// stands lower).
    disliked: Vec<Vector>,
    /// The walk: its session, the skip features of open starts, the run of
    /// early skips for leafing.
    walk: Session,
    pending: HashMap<usize, [f32; G]>,
    streak: u32,
    last_event: Option<UnixMs>,
    learn: bool,
    /// The walk teaches the weights from this start on ([`TEACH_LAST`]).
    learn_from: UnixMs,
    /// Imported starts not ended yet, by catalog position.
    import_open: HashMap<usize, UnixMs>,
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
    fn duration(&self, id: TrackId) -> Option<u32> {
        self.pos.get(&id).and_then(|&i| self.duration[i])
    }
    /// New music the catalog lacks is unfamiliar.
    fn familiarity(&self, id: TrackId) -> Familiarity {
        let Some(&i) = self.pos.get(&id) else { return Familiarity::Unfamiliar };
        let t = &self.tracks[i];
        if t.loved(self.now) {
            Familiarity::Loved
        } else if t.unfamiliar(self.now) {
            Familiarity::Unfamiliar
        } else {
            Familiarity::Known
        }
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
        m.learn_from = teach_from(&input.events);
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
        for id in &input.saved {
            if let Some(&i) = m.pos.get(id) {
                m.tracks[i].saved = true;
            }
        }
        if input.personal_graph {
            m.personal = crate::graph::personal(catalog, &input.events, &input.playlists);
        }
        // How the user listens, before the walk: the skip model reads the
        // background of each moment.
        let (starts, p_bg) = m.listen(input);
        for p in ordered(&input.events) {
            m.event(p, None);
        }
        m.close_imported();
        m.now = now;
        m.learn = false;
        m.settle(catalog, input, &starts, &p_bg);
        m
    }

    /// What a live `session` is: pure or mixed by the
    /// profiles of its starts, its `p_bg` ranked among the user's own
    /// sessions, the profile of its last start.
    pub fn live(&self, session: &Session) -> Live {
        let profile = |t: TrackId| self.position(t).and_then(|i| self.profile_at(i));
        let signals = SessionSignals::of_session(session, |t| self.position(t).and_then(|i| self.duration[i]));
        Live {
            mode: session.mode(profile),
            p_bg: self.moment(session, session.last_ts().unwrap_or(self.now)).p_bg,
            focus: self.listening.focused(&signals),
            profile: session.last().and_then(|p| profile(p.track)),
        }
    }

    /// The profile of the track at `i`.
    pub fn profile_at(&self, i: usize) -> Option<usize> {
        self.profile_of.get(i).copied().flatten().map(usize::from)
    }

    /// The soft membership of the track at `i` in each profile
    /// ([`Profiles::membership`]); a track without a sound is wholly of
    /// its own profile.
    pub fn membership_at(&self, i: usize) -> Vec<f32> {
        match &self.emb[i] {
            Some(e) => self.profiles.membership(e, &self.types[i]),
            None => (0..self.profiles.len()).map(|j| if self.profile_at(i) == Some(j) { 1.0 } else { 0.0 }).collect(),
        }
    }

    /// The length of the track at `i`.
    pub fn duration_at(&self, i: usize) -> Option<u32> {
        self.duration[i]
    }

    /// The long affinity of the track at `i` within its profile: 1 for the
    /// best loved of it, so a small profile's favourites stand as high as a
    /// big one's.
    pub fn profile_affinity(&self, i: usize) -> f32 {
        let a = self.tracks[i].aff_long(self.now);
        match self.profile_at(i).and_then(|j| self.profile_top.get(j)) {
            Some(top) if *top > 1e-6 => a / top,
            _ => a,
        }
    }

    /// Whether profile `j` is a background profile: most of its plays
    /// background, of a user who has background sessions at all.
    pub fn is_background_profile(&self, j: usize) -> bool {
        self.listening.has_background && self.profile_bg.get(j).is_some_and(|b| *b >= 0.5)
    }

    /// The home's cards of the profiles out of `slots`. A background profile whose tracks suit the
    /// background (`suits`, see [`crate::background`]) takes up to
    /// `bg_cards` cards by its share, of those tracks only, split by
    /// energy; one with too few (its background is its ordinary songs)
    /// counts as any other. Every active profile keeps a card.
    pub fn home_profiles(&self, slots: usize, bg_cards: usize, suits: impl Fn(usize) -> bool) -> HomeProfiles {
        let k = self.profiles.len();
        let mut suited: Vec<Vec<(usize, f32)>> = vec![Vec::new(); k];
        for i in 0..self.len() {
            if let Some(j) = self.profile_at(i).filter(|&j| self.is_background_profile(j))
                && suits(i)
            {
                let energy = self.emb[i].map_or(0.0, |e| crate::mood::Axes::of(&self.norm.z_of_embedded(&e)).energy);
                suited[j].push((i, energy));
            }
        }
        let room: Vec<usize> = suited.iter().map(|s| background_cards(s, bg_cards).len()).collect();
        let background: Vec<bool> = room.iter().map(|r| *r > 0).collect();
        let cap = room.iter().copied().max().unwrap_or(0).max(1);
        let mut cards = quota(&self.profile_share, slots, &background, cap);
        // Each background profile's own cap.
        for j in 0..k {
            if background[j] && cards[j] > room[j] {
                let spare = cards[j] - room[j];
                cards[j] = room[j];
                if let Some(o) = (0..k).filter(|&o| !background[o] && cards[o] > 0).max_by(|a, b| self.profile_share[*a].total_cmp(&self.profile_share[*b])) {
                    cards[o] += spare;
                }
            }
        }
        let background = (0..k).map(|j| if background[j] { background_cards(&suited[j], cards[j]) } else { Vec::new() }).collect();
        HomeProfiles { cards, background }
    }

    /// Every start of the history and how it ended, its seeks and the likes
    /// set while it played as its touches, in order of time.
    fn starts_of(&self, input: &TasteInput) -> Vec<Started> {
        let mut out: Vec<Started> = Vec::new();
        let mut open: HashMap<TrackId, usize> = HashMap::new();
        for p in ordered(&input.events) {
            match p.kind {
                EventType::Start => {
                    open.insert(p.track, out.len());
                    out.push(Started {
                        ts: p.ts,
                        pos: self.pos.get(&p.track).copied(),
                        track: p.track,
                        ended: Ended::Open,
                        end: None,
                        origin: p.origin,
                        context: p.context,
                        touches: 0,
                        attention: None,
                        import: p.origin == Some(EventOrigin::Import),
                    });
                }
                EventType::Complete | EventType::Skip => {
                    let Some(k) = open.remove(&p.track) else { continue };
                    let s = &mut out[k];
                    s.end = Some(p.ts);
                    s.attention = p.attention;
                    s.ended = match p.kind {
                        EventType::Complete => Ended::Complete,
                        _ if p.end_reason == Some(EndReason::AppClosed) => Ended::Open,
                        _ if p.position_ms.is_some_and(|x| x < EARLY_SKIP_MS) => Ended::Early,
                        _ => Ended::Late,
                    };
                }
                EventType::Seek => {
                    if let Some(&k) = open.get(&p.track) {
                        out[k].touches += 1;
                    }
                }
                _ => {}
            }
        }
        let mut by_track: HashMap<TrackId, Vec<usize>> = HashMap::new();
        for (k, s) in out.iter().enumerate().filter(|(_, s)| !s.import) {
            by_track.entry(s.track).or_default().push(k);
        }
        for (t, at) in &input.like_times {
            let Some(list) = by_track.get(t) else { continue };
            let k = list.partition_point(|&k| out[k].ts <= *at);
            if let Some(&k) = k.checked_sub(1).map(|x| &list[x])
                && *at - out[k].ts <= LIKE_IN_PLAY_MS
            {
                out[k].touches += 1;
            }
        }
        out
    }

    /// How the user listens ([`Listening`]): of the user's own starts of
    /// `input`; the starts and the `p_bg` of each.
    fn listen(&mut self, input: &TasteInput) -> (Vec<Started>, Vec<f32>) {
        let starts = self.starts_of(input);
        let own: Vec<usize> = (0..starts.len()).filter(|&k| !starts[k].import).collect();
        let heard: Vec<Heard> = own
            .iter()
            .enumerate()
            .map(|(x, &k)| {
                let s = &starts[k];
                // A history older than origins: a start right after one not
                // heard out, not of a radio, is the user's pick.
                let manual = match s.origin {
                    Some(o) => o == EventOrigin::Manual,
                    None => x > 0 && {
                        let prev = &starts[own[x - 1]];
                        s.ts - prev.ts < SESSION_GAP_MS && prev.ended != Ended::Complete && s.context != EventContext::Radio
                    },
                };
                Heard {
                    ts: s.ts,
                    end: s.end,
                    ended: s.ended,
                    manual,
                    touches: s.touches,
                    attention: s.attention,
                    duration_ms: s.pos.and_then(|i| self.duration[i]),
                }
            })
            .collect();
        let (listening, p_own) = Listening::of(&heard);
        let mut p_bg = vec![0.0f32; starts.len()];
        for (x, &k) in own.iter().enumerate() {
            p_bg[k] = p_own[x];
        }
        self.listening = listening;
        (starts, p_bg)
    }

    /// The taste profiles, their shares and the dislikes' neighbours, once
    /// the history is walked (`starts` and their `p_bg` of [`Self::listen`]).
    fn settle(&mut self, catalog: &Catalog, input: &TasteInput, starts: &[Started], p_bg: &[f32]) {
        let now = self.now;
        let n = self.len();
        self.settle_freshness();
        let set = &input.profiles;
        let w_bg = if set.background_less { W_BG } else { 1.0 };
        let own: Vec<usize> = (0..starts.len()).filter(|&k| !starts[k].import).collect();
        // The profiles' mass: the outcomes, a background play counting
        // little; likes and playlists. And their life: the active plays of
        // the last two months.
        let mut mass = vec![0.0f32; n];
        let mut alive = vec![0.0f32; n];
        let mut plays: Vec<(UnixMs, usize)> = Vec::new();
        for (k, s) in starts.iter().enumerate() {
            let Some(i) = s.pos else { continue };
            let r = match s.ended {
                Ended::Complete => MASS_COMPLETE,
                Ended::Late => MASS_LATE,
                Ended::Open => MASS_OPEN,
                Ended::Early => MASS_EARLY,
            };
            mass[i] += r * halve(1.0, s.ts, now, H_MASS_MS) * (1.0 - (1.0 - w_bg) * p_bg[k]);
            if p_bg[k] < 0.5 && s.ended != Ended::Early && now - s.ts <= ALIVE_DAYS * DAY_MS {
                alive[i] += 1.0;
            }
            plays.push((s.ts, i));
        }
        for (i, m) in mass.iter_mut().enumerate() {
            if self.tracks[i].liked(now) {
                *m += MASS_LIKE;
            } else if self.tracks[i].saved {
                *m += MASS_SAVED;
            }
        }
        for (t, added) in &input.playlist_entries {
            if let Some(&i) = self.pos.get(t) {
                mass[i] += MASS_PLAYLIST * halve(1.0, *added, now, H_MASS_MS);
            }
        }
        for (_, artists) in self.taste.saved_elsewhere() {
            let Some(of) = artists.iter().map(|a| catalog.of_artist(*a)).find(|of| !of.is_empty()) else { continue };
            for &i in of {
                mass[i as usize] += MASS_SAVED / of.len() as f32;
            }
        }
        // The library is a taste too while the history is short: every
        // track a little, the ones the graph gathers around more.
        self.cold = if starts.len() >= COLD_UNTIL { 0.0 } else { 1.0 / (1.0 + starts.len() as f32 / COLD_STARTS) };
        self.library_prior = input.library_prior;
        for (i, m) in mass.iter_mut().enumerate() {
            *m += self.prior_mass(i);
        }
        for (i, m) in mass.iter_mut().enumerate() {
            if self.taste.blocks(&catalog.tracks()[i]) {
                *m = 0.0;
            }
        }
        let playlists: Vec<Vec<usize>> = input.playlists.iter().map(|l| l.iter().filter_map(|t| self.pos.get(t).copied()).collect()).collect();
        let day = (now + i64::from(self.utc_offset_min) * 60_000).div_euclid(DAY_MS);
        self.profiles = Profiles::build(&ProfilesInput {
            ids: &self.ids,
            sound: &self.emb,
            types: &self.types,
            artists: &self.artist,
            mass: &mass,
            alive: &alive,
            plays: &plays,
            playlists: &playlists,
            scale: self.scale,
            params: Params { split: set.split_by_sound, ..Params::default() },
            history: set.history.as_ref(),
            day,
        });
        self.regions = self.profiles.regions(&self.emb);
        self.profile_of = self.profiles.all_of().to_vec();
        let k = self.profiles.len();
        // Their shares of the recent plays (an early skip is no choice of it)
        // and how much of each one's plays is background.
        // A play imported from another service is the long taste: beside
        // the user's own plays here it counts [`IMPORT_SHARE`] (a fresh
        // import of a whole history would take the shares over as if it
        // were today's listening); alone, it is all there is.
        let own_listening = own.iter().any(|&k| starts[k].pos.is_some() && starts[k].ended != Ended::Early);
        let share_play = |x: usize| {
            let s = &starts[x];
            let i = s.pos?;
            let weight = if s.ended == Ended::Early {
                0.0
            } else if s.import && own_listening {
                IMPORT_SHARE
            } else {
                1.0
            };
            Some(SharePlay { ts: s.ts, profile: self.profile_at(i), p_bg: p_bg[x], weight })
        };
        let all: Vec<SharePlay> = (0..starts.len()).filter_map(share_play).collect();
        let own_plays: Vec<SharePlay> = own.iter().filter_map(|&x| share_play(x)).collect();
        let sh = shares(&all, k, w_bg, Some(SHARE_HALF_DAYS), now);
        // No plays yet: the profiles' shares of the mass (the likes, the
        // playlists, the library's prior).
        let mut by_mass = vec![0.0f32; k];
        for (i, m) in mass.iter().enumerate() {
            if let Some(j) = self.profile_at(i) {
                by_mass[j] += m.max(0.0);
            }
        }
        let total_mass: f32 = by_mass.iter().sum();
        self.profile_share = if sh.iter().sum::<f32>() > 0.0 {
            sh
        } else if total_mass > 0.0 {
            by_mass.iter().map(|m| m / total_mass).collect()
        } else {
            vec![1.0 / k.max(1) as f32; k]
        };
        self.profile_bg = background_shares(&own_plays, k);
        let mut top = vec![0.0f32; k];
        for i in 0..n {
            if let Some(j) = self.profile_at(i) {
                top[j] = top[j].max(self.tracks[i].aff_long(now));
            }
        }
        self.profile_top = top;
        let disliked: Vec<usize> = input.disliked_tracks.iter().filter_map(|t| self.pos.get(t).copied()).collect();
        if input.dislike_neighbours {
            self.disliked = disliked.iter().filter_map(|&i| self.emb[i]).collect();
            let near = crate::dislike::near_dislikes(&self.emb, &disliked, self.scale);
            self.taste.set_near_dislikes(near);
        }
    }

    /// When each track was last heard and how much of its like counts now
    /// (see [`Self::freshness_at`]): every surface weighs an old love and a
    /// bulk of likes so.
    fn settle_freshness(&mut self) {
        let n = self.len();
        let mut likes: Vec<UnixMs> = (0..n).filter_map(|i| self.taste.liked_time_at(i)).collect();
        likes.sort_unstable();
        let by_hand = |t: UnixMs| {
            let from = likes.partition_point(|x| *x < t - BULK_WINDOW_MS);
            let to = likes.partition_point(|x| *x <= t + BULK_WINDOW_MS);
            to - from < BULK_LIKES
        };
        self.last_heard = (0..n)
            .map(|i| {
                let s = &self.tracks[i];
                let liked = self.taste.liked_time_at(i).filter(|t| by_hand(*t));
                [self.taste.last_played_at(i), s.last_start, s.last_complete, liked].into_iter().flatten().max()
            })
            .collect();
        self.latest_heard = self.last_heard.iter().flatten().copied().max();
        for i in 0..n {
            let stale = 1.0 - self.freshness_at(i);
            self.tracks[i].love_stale = stale;
        }
    }

    /// When the track at `i` was last heard (see [`Self::last_heard`]).
    pub fn last_heard_at(&self, i: usize) -> Option<UnixMs> {
        self.last_heard.get(i).copied().flatten()
    }

    /// The user's latest listening (a play, a like set by hand).
    pub fn latest_heard(&self) -> Option<UnixMs> {
        self.latest_heard
    }

    /// How much of a love of the track at `i` counts now: whole while it
    /// was heard within [`FRESH_GRACE_MS`] of the user's latest listening
    /// (a user back after a year has last year's weeks, not none), then by
    /// half every [`FRESH_HALF_MS`] down to [`STALE_FLOOR`]; never heard
    /// once anything was, [`UNHEARD_LOVE`].
    pub fn freshness_at(&self, i: usize) -> f32 {
        match (self.latest_heard, self.last_heard_at(i)) {
            (Some(l), Some(h)) => freshness(l - h),
            (Some(_), None) => UNHEARD_LOVE,
            _ => 1.0,
        }
    }

    /// The chance of an early skip of the track at `i`, `p` by the model:
    /// at least what its last outcomes say when it is worn out now
    /// ([`TrackStats::burnt`]).
    pub fn skip_risk(&self, i: usize, p: f32) -> f32 {
        self.tracks.get(i).and_then(|t| t.burnt(p)).map_or(p, |b| b.max(p))
    }

    /// The graph's prior of the track at `i` (how central it is, how
    /// popular) as a cold start: whole for a track never started, fading
    /// with its own starts — what the user plays of it says more.
    pub fn prior_at(&self, i: usize) -> f32 {
        self.graph.prior(i) / (1.0 + self.tracks.get(i).map_or(0, |t| t.starts) as f32)
    }

    /// A raw bliss vector in the model's space.
    pub fn embed(&self, v: &Vector) -> Vector {
        self.norm.embed(v)
    }

    /// How much lower a new track sounding `v` (embedded) stands for the
    /// dislikes near it.
    pub fn dislike_near_of(&self, v: &Vector) -> f32 {
        crate::dislike::near_dislikes_of(v, &self.disliked, self.scale)
    }

    fn empty(catalog: &Catalog, taste: Taste, extra: ModelInput<'_>, utc_offset_min: i32, now: UnixMs) -> Self {
        let n = catalog.len();
        let norm = extra.index.normalizer();
        let ids: Vec<TrackId> = catalog.tracks().iter().map(|t| t.id).collect();
        let mut pos: HashMap<TrackId, usize> = ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        // A copy of a catalog track (one recording on two albums) is it.
        for (id, i) in catalog.aliases() {
            pos.entry(id).or_insert(i);
        }
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
        let mut album_first = vec![false; n];
        for list in by_release.values() {
            if let Some(&i) = list.first() {
                album_first[i] = true;
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
            by_artist: catalog.tracks().iter().enumerate().fold(HashMap::new(), |mut m: HashMap<ArtistId, Vec<u32>>, (i, t)| {
                if let Some(a) = t.main_artist() {
                    m.entry(a).or_default().push(i as u32);
                }
                m
            }),
            duration: catalog.tracks().iter().map(|t| t.duration_ms).collect(),
            types,
            next_in_album,
            album_first,
            worlds: HashMap::new(),
            part_total: [0; 4],
            trans: HashMap::new(),
            trans_total: HashMap::new(),
            by_world: catalog.tracks().iter().enumerate().fold(HashMap::new(), |mut m: HashMap<u64, Vec<u32>>, (i, t)| {
                if let Some(w) = t.world {
                    m.entry(w).or_default().push(i as u32);
                }
                m
            }),
            lane_trans: [[0.0; LANES]; LANES],
            co: CoOccur::new(n),
            bag: CoOccur::new(n),
            graph: extra.graph.filter(|g| !g.is_empty()).map(|g| SimGraph::build(catalog, g)).unwrap_or_default(),
            personal: Vec::new(),
            cold: 1.0,
            library_prior: false,
            profiles: Profiles::default(),
            profile_of: vec![None; n],
            profile_share: Vec::new(),
            profile_bg: Vec::new(),
            profile_top: Vec::new(),
            listening: Listening::default(),
            weights: Weights::default(),
            scale,
            norm,
            regions: Regions::default(),
            last_heard: Vec::new(),
            latest_heard: None,
            disliked: Vec::new(),
            walk: Session::default(),
            pending: HashMap::new(),
            streak: 0,
            last_event: None,
            learn: false,
            learn_from: UnixMs::MIN,
            import_open: HashMap::new(),
        }
    }

    pub fn now(&self) -> UnixMs {
        self.now
    }

    /// How cold the history is: 1 with none, towards 0 as it grows.
    pub fn cold(&self) -> f32 {
        self.cold
    }

    /// What the library's prior gives the track at `i` while the history
    /// is short (the taste profiles' mass, a mix's love of it): up to
    /// [`COLD_MASS`], fading with the history; 0 when it is off.
    pub fn prior_mass(&self, i: usize) -> f32 {
        if self.library_prior { COLD_MASS * self.cold * (0.3 + 0.7 * self.graph.prior(i)) } else { 0.0 }
    }

    /// The personal graph's neighbours of the track at `i`.
    pub fn personal_near(&self, i: usize) -> &[(u32, f32)] {
        self.personal.get(i).map_or(&[], Vec::as_slice)
    }

    /// The tracks of artist `a` (main artist), by catalog position.
    pub fn of_artist(&self, a: ArtistId) -> &[u32] {
        self.by_artist.get(&a).map_or(&[], Vec::as_slice)
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

    /// The energy of the track at `i` on the mood axes ([`crate::mood::Axes`]),
    /// when it has a sound.
    pub fn energy_at(&self, i: usize) -> Option<f32> {
        self.emb[i].map(|e| self.energy_of(&e))
    }

    /// The energy of an embedded sound `v` on the mood axes.
    pub fn energy_of(&self, v: &Vector) -> f32 {
        crate::mood::Axes::of(&self.norm.z_of_embedded(v)).energy
    }

    /// The loudness of the track at `i` as bliss gives it (absolute, −1
    /// silence … 1 full scale), when it has a sound.
    pub fn loudness_at(&self, i: usize) -> Option<f32> {
        self.emb[i].and_then(|e| self.norm.raw_of_embedded(&e, crate::mood::LOUDNESS))
    }

    /// The track at `i` on the mood axes, when it has a sound.
    pub fn axes_at(&self, i: usize) -> Option<crate::mood::Axes> {
        self.emb[i].map(|e| self.axes_of(&e))
    }

    /// An embedded sound `v` on the mood axes.
    pub fn axes_of(&self, v: &Vector) -> crate::mood::Axes {
        crate::mood::Axes::of(&self.norm.z_of_embedded(v))
    }

    /// The share of the user's starts in `part` of the day ([`DayPart::index`]);
    /// a quarter without a history.
    pub fn part_share(&self, part: usize) -> f32 {
        let all: u32 = self.part_total.iter().sum();
        if all == 0 { 0.25 } else { self.part_total[part] as f32 / all as f32 }
    }

    fn part_of(&self, ts: UnixMs) -> usize {
        DayPart::at(ts, self.utc_offset_min).index()
    }

    /// The moment of `session` at `now` (at least the session's last start).
    pub fn moment(&self, session: &Session, now: UnixMs) -> Moment {
        self.moment_with(session, now, true)
    }

    /// [`Self::moment`]; the neighbours of the session in the graphs and in
    /// the sessions before only when it will `rank` candidates (the walk of
    /// the history needs them for a step alone).
    fn moment_with(&self, session: &Session, now: UnixMs, rank: bool) -> Moment {
        let now = now.max(session.last_ts().unwrap_or(now));
        let sess: Vec<(usize, Ended, EventContext)> =
            session.tracks().filter_map(|p| Some((self.position(p.track)?, p.ended, p.context))).collect();
        let prev = session.last().and_then(|p| self.position(p.track));
        // The graphs around the last three not skipped early.
        let mut graph_near: HashMap<usize, f32> = HashMap::new();
        let mut graph_artists: HashMap<ArtistId, f32> = HashMap::new();
        let mut pgraph_near: HashMap<usize, f32> = HashMap::new();
        // What was not skipped early: a skip past its first 30 s (even
        // before its half) still tells what the session goes on from — only
        // what played through seeded worse (autoplay's and the wave's
        // replays of the history).
        let through: Vec<usize> = if rank { sess.iter().rev().filter(|(_, e, _)| *e != Ended::Early).map(|x| x.0).collect() } else { Vec::new() };
        let recent: Vec<usize> = through.iter().take(3).copied().collect();
        let mut bag_near: HashMap<usize, f32> = HashMap::new();
        for (k, s) in through.iter().take(BAG_WINDOW).enumerate() {
            let f = 0.8f32.powi(k as i32);
            for &(c, w) in self.bag.row(*s) {
                *bag_near.entry(c as usize).or_default() += f * w;
            }
        }
        let own: HashSet<ArtistId> = recent.iter().filter_map(|&s| self.artist[s]).collect();
        // The walk of the collective graph from them (its first step the
        // neighbours of each, as one hop always counted).
        let seeds: Vec<(usize, f32)> = recent.iter().enumerate().map(|(k, &s)| (s, 0.6f32.powi(k as i32))).collect();
        if !seeds.is_empty() {
            graph_near = self.graph.walk(&seeds);
        }
        for (k, &s) in recent.iter().enumerate() {
            let f = 0.6f32.powi(k as i32);
            if let Some(a) = self.artist[s] {
                for &(b, w) in self.graph.similar_artists(a) {
                    if !own.contains(&b) {
                        *graph_artists.entry(b).or_default() += f * w;
                    }
                }
            }
            for &(c, w) in self.personal.get(s).map_or(&[][..], Vec::as_slice) {
                *pgraph_near.entry(c as usize).or_default() += f * w;
            }
        }
        Moment {
            bag_near,
            graph_near,
            graph_artists,
            pgraph_near,
            now,
            next_album: prev.and_then(|i| self.next_in_album(i)),
            prev,
            sess,
            anchor: session.anchor(self),
            pushes: session.pushes(self),
            lane: session.lane(self),
            part: self.part_of(now),
            skip_streak: session.skip_streak(self),
            album_session: session.album_session(self),
            world_session: session.world_session(self),
            repeat_one: session.repeat_one(),
            p_bg: self.listening.p_now(&SessionSignals::of_session(session, |t| self.position(t).and_then(|i| self.duration[i]))),
        }
    }

    /// The features of candidate `c` at moment `m` ([`FEATURES`]).
    pub fn features(&self, m: &Moment, c: usize) -> [f32; F] {
        let t = &self.tracks[c];
        let mut f = [0.0f32; F];
        f[0] = t.aff_long(m.now);
        f[1] = t.aff_hot(m.now);
        f[2] = if t.liked(m.now) { t.love_fresh() } else { 0.0 };
        let e = self.emb[c].as_ref();
        f[F_SOUND] = match (e, &m.anchor) {
            (Some(e), Some(a)) => -euclid(e, a) / self.scale,
            _ => -1.0,
        };
        if e.is_some() {
            f[F_SKIP_PUSH] = self.skip_push_of(&m.pushes, c);
        }
        f[F_NEXT_ALBUM] = f32::from(u8::from(m.next_album == Some(c)));
        if let Some(p) = m.prev {
            f[F_SAME_ALBUM] = f32::from(u8::from(self.release[c].is_some() && self.release[c] == self.release[p]));
            let (wc, wp) = (self.world[c], self.world[p]);
            f[F_SAME_WORLD] = f32::from(u8::from(wp.is_some() && wc == wp));
            if let (Some(wp), Some(wc)) = (wp, wc)
                && let Some(out) = self.trans.get(&wp)
            {
                let total = self.trans_total.get(&wp).copied().unwrap_or(0.0);
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
        let like_artist = self.artist[c].and_then(|a| m.graph_artists.get(&a)).copied().unwrap_or(0.0);
        f[F_GRAPH] = (m.graph_near.get(&c).copied().unwrap_or(0.0) + GRAPH_ARTIST * like_artist).tanh();
        f[F_PGRAPH] = m.pgraph_near.get(&c).copied().unwrap_or(0.0).tanh();
        f[F_PRIOR] = self.prior_at(c);
        f[F_SESS_CO] = (m.bag_near.get(&c).copied().unwrap_or(0.0) / 2.0).tanh();
        f[F_IS_PREV] = f32::from(u8::from(m.prev == Some(c)));
        f[F_ALBUM_FIRST] = f32::from(u8::from(self.album_first[c] && m.prev.is_none_or(|p| self.release[p] != self.release[c])));
        f
    }

    /// [`F_SKIP_PUSH`] of `c` by the sounds `pushes` (embedded, each with
    /// how hard it pushes): 0 for a track without a sound.
    pub fn skip_push_of(&self, pushes: &[(Vector, f32)], c: usize) -> f32 {
        let Some(e) = self.emb[c].as_ref() else { return 0.0 };
        -pushes.iter().map(|(p, w)| w * (1.0 - euclid(e, p) / self.scale).max(0.0)).sum::<f32>()
    }

    /// The tracks `session` rejected, to measure candidates against by
    /// both sounds where the neural model heard them ([`Self::skip_push_by`]);
    /// none without rejections or before the index blends.
    pub fn push_probe<'a>(&self, session: &Session, index: Option<&'a VectorIndex<TrackId>>) -> Option<PushProbe<'a>> {
        let index = index?;
        let pushed = session.pushed(self);
        if pushed.is_empty() {
            return None;
        }
        let ids: Vec<TrackId> = pushed.iter().map(|p| p.0).collect();
        let probe = index.sound_probe(&ids)?;
        Some(PushProbe { index, probe, pushed })
    }

    /// [`F_SKIP_PUSH`] of `c` by the session's rejections as `probe` holds
    /// them: by the blend of bliss and the neural model where both tracks
    /// have the model's vector (an instrumental beat and a sung track of one
    /// tempo stand apart), by bliss else. 0 for a track without a sound.
    pub fn skip_push_by(&self, probe: &PushProbe<'_>, c: usize) -> f32 {
        let Some(e) = self.emb[c].as_ref() else { return 0.0 };
        let mut sum = 0.0f32;
        for (k, (t, w)) in probe.pushed.iter().enumerate() {
            let d = probe.index.probe_distance(&probe.probe, k, self.ids[c]).or_else(|| self.pos.get(t).and_then(|&j| self.emb[j].as_ref()).map(|p| euclid(e, p)));
            if let Some(d) = d {
                sum += w * (1.0 - d / self.scale).max(0.0);
            }
        }
        -sum
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
        x[G_BACKGROUND] = m.p_bg;
        x
    }

    /// The chance `c` started now from `ctx` is skipped in its first 30 s.
    pub fn p_skip(&self, m: &Moment, c: usize, ctx: EventContext) -> f32 {
        self.skip_risk(c, self.weights.skip.p(&self.skip_features(m, c, ctx)))
    }

    /// The features of starting a new track of a service now from `ctx`
    /// ([`SKIP_FEATURES`]): never played, no affinity, its sound `vector`
    /// (embedded) and length when known.
    pub fn skip_features_new(&self, m: &Moment, vector: Option<&Vector>, duration_ms: Option<u32>, ctx: EventContext) -> [f32; G] {
        let mut x = [0.0f32; G];
        let b = |v: bool| f32::from(u8::from(v));
        x[0] = 1.0;
        x[4] = 0.2;
        x[5] = 1.0;
        x[7] = b(m.sess.is_empty());
        x[8] = (m.sess.len() as f32).ln_1p();
        x[9] = b(ctx == EventContext::Album);
        x[10] = b(ctx == EventContext::Playlist);
        x[11] = b(ctx == EventContext::Artist);
        x[12] = b(matches!(ctx, EventContext::HomeSection | EventContext::Radio));
        x[15] = match (m.prev.and_then(|p| self.emb[p].as_ref()), vector) {
            (Some(a), Some(v)) => euclid(a, v) / self.scale,
            _ => 1.0,
        };
        let local = m.now + i64::from(self.utc_offset_min) * 60_000;
        x[16] = b(local.rem_euclid(DAY_MS) / HOUR_MS < 6);
        x[19] = b(duration_ms.is_some_and(|d| d > LONG_MS));
        x[20] = b(duration_ms.is_some_and(|d| d < INTERLUDE_MS));
        let mut ends = m.sess.iter().rev().map(|(_, e, _)| *e);
        let (e1, e2) = (ends.next(), ends.next());
        x[21] = b(e1 == Some(Ended::Early));
        x[22] = b(e1 == Some(Ended::Early) && e2 == Some(Ended::Early));
        x[G_NEW_SERVICE] = 1.0;
        x[G_BACKGROUND] = m.p_bg;
        x
    }

    /// The chance a new track of a service started now is skipped in its
    /// first 30 s.
    pub fn p_skip_new(&self, m: &Moment, vector: Option<&Vector>, duration_ms: Option<u32>, ctx: EventContext) -> f32 {
        self.weights.skip.p(&self.skip_features_new(m, vector, duration_ms, ctx))
    }


    /// The candidates of the next start at `m`: the nearest to the
    /// anchor, what followed the last three, the next on the album, the
    /// previous track's world, the hot and the long favourites; `ok`
    /// filters them.
    pub fn candidates(&self, m: &Moment, ok: &dyn Fn(usize) -> bool) -> Vec<usize> {
        let mut out: HashSet<usize> = HashSet::new();
        if let Some(a) = &m.anchor {
            let near: Vec<(f32, usize)> = self.emb.iter().enumerate().filter_map(|(i, e)| Some((euclid(e.as_ref()?, a), i))).filter(|(_, i)| ok(*i)).collect();
            out.extend(best_n(near, 200, |x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1))).into_iter().map(|(_, i)| i));
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
        // The graphs: the neighbours of the session, the best known tracks
        // of the artists like its.
        let strongest = |map: &HashMap<usize, f32>| {
            let mut v: Vec<(usize, f32)> = map.iter().map(|(c, w)| (*c, *w)).filter(|(c, _)| ok(*c)).collect();
            v.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
            v.into_iter().take(GRAPH_CANDIDATES).map(|(c, _)| c).collect::<Vec<usize>>()
        };
        out.extend(strongest(&m.graph_near));
        out.extend(strongest(&m.bag_near));
        out.extend(strongest(&m.pgraph_near));
        let mut artists: Vec<(ArtistId, f32)> = m.graph_artists.iter().map(|(a, w)| (*a, *w)).collect();
        artists.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
        for (a, _) in artists.into_iter().take(GRAPH_ARTISTS) {
            let mut theirs: Vec<usize> = self.by_artist.get(&a).map_or(&[][..], Vec::as_slice).iter().map(|&i| i as usize).filter(|&i| ok(i)).collect();
            let key = |i: usize| self.tracks[i].aff_long(m.now) + self.prior_at(i);
            theirs.sort_by(|x, y| key(*y).total_cmp(&key(*x)).then(x.cmp(y)));
            out.extend(theirs.into_iter().take(GRAPH_ARTIST_TRACKS));
        }
        out.extend(m.next_album.filter(|&c| ok(c)));
        if let Some(w) = m.prev.and_then(|p| self.world[p]) {
            out.extend(self.by_world.get(&w).map_or(&[][..], Vec::as_slice).iter().map(|&i| i as usize).filter(|&i| ok(i)));
        }
        let mut top = |key: &dyn Fn(&TrackStats) -> f32, n: usize| {
            let v: Vec<(f32, usize)> = (0..self.len()).filter(|&i| ok(i)).map(|i| (key(&self.tracks[i]), i)).filter(|(k, _)| *k > 0.0).collect();
            out.extend(best_n(v, n, |x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1))).into_iter().map(|(_, i)| i));
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

    /// One imported play's event (another service's history): its outcome
    /// teaches the long affinity of its track, nothing else.
    fn imported(&mut self, p: &Play) {
        let Some(&i) = self.pos.get(&p.track) else { return };
        match p.kind {
            EventType::Start => {
                if let Some(t0) = self.import_open.insert(i, p.ts) {
                    self.tracks[i].add_as(t0, R_OPEN, W_START_ONLY, false);
                }
            }
            EventType::Complete => {
                if self.import_open.remove(&i).is_some() {
                    self.tracks[i].add_as(p.ts, R_COMPLETE, W_COMPLETE, false);
                }
            }
            EventType::Skip => {
                if self.import_open.remove(&i).is_none() {
                    return;
                }
                let (r, today) = match p.position_ms {
                    Some(pos) if pos < EARLY_SKIP_MS => (R_EARLY, W_SKIP_EARLY),
                    pos => {
                        let heard = match (pos, self.duration[i]) {
                            (Some(pos), Some(d)) if d > 0 => (pos as f32 / d as f32).clamp(0.0, 1.0),
                            _ => 0.0,
                        };
                        (R_LATE * heard, W_SKIP_LATE_HEARD * heard)
                    }
                };
                self.tracks[i].add_as(p.ts, r, today, false);
            }
            _ => {}
        }
    }

    /// Imported starts that never ended count as such.
    fn close_imported(&mut self) {
        let mut open: Vec<(usize, UnixMs)> = self.import_open.drain().collect();
        open.sort_unstable();
        for (i, t0) in open {
            self.tracks[i].add_as(t0, R_OPEN, W_START_ONLY, false);
        }
    }

    /// One event of the history, in order of time. `hook` sees every start
    /// that follows another of its session, before anything learns from it.
    fn event(&mut self, p: &Play, hook: Option<&mut dyn FnMut(&Self, &Moment, usize, &Play)>) {
        if p.origin == Some(EventOrigin::Import) {
            self.imported(p);
            return;
        }
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
            EventType::Like => {
                self.tracks[i].liked = Some(p.ts);
                self.walk.like(p.track);
            }
            EventType::Unlike => self.tracks[i].liked = None,
            EventType::Complete => {
                let part = self.part_of(p.ts);
                self.streak = 0;
                let manual = self.tracks[i].open.take().is_some_and(|(_, o)| o == Some(EventOrigin::Manual));
                let r = R_COMPLETE * if manual { MANUAL_POSITIVE } else { 1.0 };
                self.tracks[i].add_as(p.ts, r, W_COMPLETE, true);
                let t = &mut self.tracks[i];
                t.note(false);
                t.completes += 1;
                t.done[part] = t.done[part].saturating_add(1);
                t.last_complete = Some(p.ts);
                self.walk.end(p.track, Ended::Complete);
                if let Some(x) = self.pending.remove(&i).filter(|_| self.learn) {
                    self.weights.skip.observe(&x, false);
                }
            }
            EventType::Skip => {
                let opened = self.tracks[i].open.take();
                let origin = opened.and_then(|(_, o)| o);
                // The app closed on it: no outcome, unless the app chose it
                // and it was left within its first seconds (abandoned).
                let abandoned = p.position_ms.is_some_and(|x| (ABANDON_FROM_MS..ABANDON_MS).contains(&x)) && in_stream(p.context, origin);
                if p.end_reason == Some(EndReason::AppClosed) && !abandoned {
                    self.pending.remove(&i);
                    return;
                }
                match p.position_ms {
                    // A short track's outro is no early skip.
                    Some(pos) if skip_ended(pos, self.duration[i]) == Ended::Early => {
                        self.streak += 1;
                        let leafing = self.streak >= SKIP_STREAK_FROM && matches!(p.context, EventContext::Playlist | EventContext::Library);
                        let lean = if leafing { SKIP_STREAK_FACTOR } else { 1.0 };
                        let who = match origin {
                            Some(EventOrigin::Manual) => MANUAL_EARLY,
                            Some(_) => AUTO_EARLY,
                            None => 1.0,
                        };
                        // A fast skip of a long-loved track in a stream is
                        // «not now»: the session's mood, nothing against the
                        // track in the long run.
                        let not_now = pos < NOT_NOW_MS && in_stream(p.context, origin) && self.tracks[i].loved(p.ts);
                        self.tracks[i].note(true);
                        if not_now {
                            self.tracks[i].add_hot(p.ts, R_EARLY * who * lean);
                        } else {
                            self.tracks[i].add_as(p.ts, R_EARLY * who * lean, W_SKIP_EARLY * lean, true);
                            let t = &mut self.tracks[i];
                            t.early = t.early.saturating_add(1);
                        }
                        self.walk.end_at(p.track, Ended::Early, pos, self.duration[i], false);
                        // «Not now» teaches the skip model nothing either.
                        if let Some(x) = self.pending.remove(&i).filter(|_| self.learn && !not_now) {
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
                        self.tracks[i].add_as(p.ts, R_LATE * heard * if manual { MANUAL_POSITIVE } else { 1.0 }, W_SKIP_LATE_HEARD * heard, true);
                        let t = &mut self.tracks[i];
                        t.note(false);
                        t.late = t.late.saturating_add(1);
                        match pos {
                            Some(pos) => self.walk.end_at(p.track, Ended::Late, pos, self.duration[i], false),
                            None => self.walk.end(p.track, Ended::Late),
                        }
                        self.pending.remove(&i);
                    }
                }
            }
            EventType::Start => self.start(i, p, hook),
            EventType::Seek => {
                if let Some(to) = p.position_ms {
                    self.walk.seek(p.track, to);
                }
            }
            EventType::AddToPlaylist => {}
        }
    }

    fn start(&mut self, i: usize, p: &Play, hook: Option<&mut dyn FnMut(&Self, &Moment, usize, &Play)>) {
        let ts = p.ts;
        // A start after a pause begins a session of its own.
        if self.walk.last_ts().is_none_or(|l| ts - l >= SESSION_GAP_MS) {
            self.walk = Session::default();
        }
        let learn = self.learn && teaches(p.origin, p.context) && ts >= self.learn_from;
        let m = self.moment_with(&self.walk, ts, hook.is_some() || learn);
        let x = self.skip_features(&m, i, p.context);
        self.pending.insert(i, x);
        if m.prev.is_some() {
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
            *self.trans_total.entry(wp).or_default() += 1.0;
        }
        if let Some(prev) = m.prev
            && teaches(p.origin, p.context)
        {
            self.lane_trans[lane_index(self.types[prev].lane())][lane_index(self.types[i].lane())] += 1.0;
        }
        // Listened together within the session, either order.
        for (k, (s, e, _)) in m.sess.iter().rev().take(BAG_WINDOW).enumerate() {
            if *s != i && *e != Ended::Early {
                let w = 1.0 / (k + 1) as f32;
                self.bag.add(*s, i, w);
                self.bag.add(i, *s, w);
            }
        }
        let part = self.part_of(ts);
        if let Some((t0, o)) = self.tracks[i].open.take() {
            // The start before never ended.
            let r = R_OPEN * if o == Some(EventOrigin::Manual) { MANUAL_POSITIVE } else { 1.0 };
            self.tracks[i].add_as(t0, r, W_START_ONLY, true);
        }
        if self.tracks[i].last_complete.is_some_and(|c| ts - c <= REPEAT_WITHIN_MS) {
            self.tracks[i].add_as(ts, R_REPEAT, W_REPEAT, true);
        }
        let t = &mut self.tracks[i];
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

/// The best `n` of `v` by `cmp` (a total order, the best first), in no
/// order: what sorting and taking `n` keeps, in linear time.
fn best_n<T>(mut v: Vec<T>, n: usize, cmp: impl Fn(&T, &T) -> std::cmp::Ordering) -> Vec<T> {
    if n == 0 {
        v.clear();
    } else if v.len() > n {
        v.select_nth_unstable_by(n - 1, &cmp);
        v.truncate(n);
    }
    v
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
/// first ten, in the first fifty, the mean reciprocal rank and the NDCG of
/// the first ten (one relevant track: `1 / log2(rank + 1)` within ten).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hits {
    pub n: usize,
    pub hit1: f32,
    pub hit10: f32,
    pub hit50: f32,
    pub mrr: f32,
    pub ndcg10: f32,
}

impl Hits {
    fn of(ranks: &[f32]) -> Self {
        let n = ranks.len().max(1) as f32;
        Self {
            n: ranks.len(),
            hit1: ranks.iter().filter(|r| **r <= 1.0).count() as f32 / n,
            hit10: ranks.iter().filter(|r| **r <= 10.0).count() as f32 / n,
            hit50: ranks.iter().filter(|r| **r <= 50.0).count() as f32 / n,
            mrr: ranks.iter().map(|r| 1.0 / r).sum::<f32>() / n,
            ndcg10: ranks.iter().filter(|r| **r <= 10.0).map(|r| 1.0 / (r + 1.0).log2()).sum::<f32>() / n,
        }
    }
}

/// What [`replay`] measured: the next start among the whole
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
    /// The share of the catalog the learnt model's first ten ever held.
    pub coverage: f32,
    /// The share of the scored starts whose track was among the candidates
    /// of the moment ([`UserModel::candidates`]: what a radio ranks).
    pub cand_recall: f32,
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
    replay_with(catalog, input, extra, &[], &HashSet::new())
}

/// [`replay`] with `dislikes` as the runtime has them (out of every
/// ranking, their sound neighbours a little lower, [`crate::dislike`]),
/// and the starts of `unscored` left out of the hits (to compare runs with
/// and without dislikes on the same starts).
pub fn replay_with(
    catalog: &Catalog,
    input: &TasteInput,
    extra: ModelInput<'_>,
    dislikes: &[TrackId],
    unscored: &HashSet<TrackId>,
) -> ReplayReport {
    let events = ordered(&input.events);
    // The user's own history: imported plays are walked, never scored.
    let own: Vec<UnixMs> = events.iter().filter(|p| p.origin != Some(EventOrigin::Import)).map(|p| p.ts).collect();
    let (Some(&first), Some(&last)) = (own.first(), own.last()) else {
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
    if input.personal_graph {
        m.personal = crate::graph::personal(catalog, &input.events, &input.playlists);
    }
    m.learn = true;
    m.listen(input);
    let disliked: Vec<usize> = dislikes.iter().filter_map(|t| m.position(*t)).collect();
    let near = crate::dislike::near_dislikes(&m.emb, &disliked, m.scale);
    let mut shown: HashSet<usize> = HashSet::new();
    let mut ranks: HashMap<(&'static str, &'static str), Vec<f32>> = HashMap::new();
    let mut skips: Vec<(f32, f32, f32, bool)> = Vec::new();
    let mut held: Vec<bool> = Vec::new();
    let mut k_of: HashMap<*const Play, usize> = HashMap::new();
    for (k, p) in events.iter().enumerate() {
        k_of.insert(*p as *const Play, k);
    }
    for p in &events {
        let k = k_of[&(*p as *const Play)];
        let ended = outcome.get(&k).copied().unwrap_or(Ended::Open);
        // The skip model's prediction before it learns from this start.
        if p.kind == EventType::Start
            && p.origin != Some(EventOrigin::Import)
            && p.ts >= mid
            && matches!(ended, Ended::Complete | Ended::Early)
            && let Some(i) = m.position(p.track)
        {
            let walk = if m.walk.last_ts().is_none_or(|l| p.ts - l >= SESSION_GAP_MS) { Session::default() } else { m.walk.clone() };
            let mo = m.moment(&walk, p.ts);
            let x = m.skip_features(&mo, i, p.context);
            skips.push((m.weights.skip.p(&x), -x[1], -x[2] + x[4], ended == Ended::Early));
        }
        let mut hook = |model: &UserModel, mo: &Moment, target: usize, _: &Play| {
            if unscored.contains(&model.id(target)) || disliked.contains(&target) {
                return;
            }
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
            // Listened: not skipped early (what the user went on with).
            if ended != Ended::Early {
                splits.push("listened");
                if free {
                    splits.push("listened_free");
                }
                if p.ts >= mid {
                    splits.push("listened:second_half");
                }
            }
            held.push(model.candidates(mo, &|_| true).binary_search(&target).is_ok());
            let feats: Vec<[f32; F]> = (0..model.len()).map(|c| model.features(mo, c)).collect();
            let hand = Model::default();
            // A disliked track is out, its neighbours a little lower.
            let calibrated = |mut s: Vec<f32>| -> Vec<f32> {
                for (c, x) in s.iter_mut().enumerate() {
                    *x -= near[c];
                }
                for &d in &disliked {
                    s[d] = f32::NEG_INFINITY;
                }
                s
            };
            let scores = |w: &Model| -> Vec<f32> { calibrated(feats.iter().map(|f| w.score(f)).collect()) };
            let today: Vec<f32> =
                calibrated((0..model.len()).map(|c| model.tracks[c].aff_today(mo.now) + 1e-6 * (c as f32 / model.len() as f32)).collect());
            let learned = scores(&model.weights.next);
            let mut top: Vec<usize> = (0..learned.len()).collect();
            let k = 10.min(top.len());
            if k > 0 {
                top.select_nth_unstable_by(k - 1, |a, b| learned[*b].total_cmp(&learned[*a]));
                shown.extend(top[..k].iter().copied());
            }
            for (name, s) in [("phase1_affinity", today), ("hand", scores(&hand)), ("learned", learned)] {
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
        coverage: shown.len() as f32 / m.len().max(1) as f32,
        cand_recall: held.iter().filter(|h| **h).count() as f32 / held.len().max(1) as f32,
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

    /// A fast skip of a loved track in a radio is «not now»: nothing
    /// against it in the long run; the same skip of the user's own pick
    /// counts.
    #[test]
    fn not_now_costs_a_loved_track_nothing_in_the_long_run() {
        let cat = Catalog::new((0..2u128).map(|i| info(i, &[1], Some(i)).with_duration(Some(200_000))));
        let idx = VectorIndex::new();
        let mut events = Vec::new();
        let mut ts = T0 - 10 * DAY_MS;
        for _ in 0..5 {
            for t in 0..2u128 {
                events.push(Play::new(tid(t), ts, EventType::Start, Some(0)));
                ts += 200_000;
                events.push(Play::new(tid(t), ts, EventType::Complete, Some(200_000)));
            }
            ts += HOUR_MS;
        }
        let skip = |t: u128, origin: EventOrigin, context: EventContext, ts: UnixMs| {
            [
                Play { context, origin: Some(origin), ..Play::new(tid(t), ts, EventType::Start, Some(0)) },
                Play { context, ..Play::new(tid(t), ts + 4_000, EventType::Skip, Some(4_000)) },
            ]
        };
        events.extend(skip(1, EventOrigin::Manual, EventContext::Playlist, T0 - DAY_MS));
        events.extend(skip(0, EventOrigin::Radio, EventContext::Radio, T0 - DAY_MS / 2));
        let build = |events: Vec<Play>| {
            UserModel::build(&cat, &TasteInput { events, ..TasteInput::default() }, ModelInput { index: &idx, types: None, weights: None, graph: None }, T0)
        };
        let m = build(events.clone());
        assert!(m.tracks[0].aff_long(T0) > m.tracks[1].aff_long(T0) + 0.005, "{} {}", m.tracks[0].aff_long(T0), m.tracks[1].aff_long(T0));
        assert_eq!((m.tracks[0].early, m.tracks[1].early), (0, 1));
        // The skip model learns from the user's own skip, not from «not now»:
        // as if that skip never was.
        let without: Vec<Play> = events.iter().filter(|p| !(p.track == tid(0) && p.kind == EventType::Skip)).cloned().collect();
        let w = build(without);
        assert_eq!(m.weights.skip.seen, w.weights.skip.seen);
        assert_eq!(m.weights.skip.w, w.weights.skip.w);
    }

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
        ModelInput { index: idx, types: None, weights: None, graph: None }
    }

    /// The best of a list in linear time keeps what sorting it does.
    #[test]
    fn best_n_keeps_what_sorting_keeps() {
        let mut rng = Rng::new(5);
        for n in [0usize, 1, 7, 200, 500] {
            let v: Vec<(f32, usize)> = (0..300).map(|i| ((rng.below(50) as f32) / 7.0, i)).collect();
            let cmp = |x: &(f32, usize), y: &(f32, usize)| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1));
            let mut sorted = v.clone();
            sorted.sort_by(cmp);
            let mut want: Vec<usize> = sorted.into_iter().take(n).map(|x| x.1).collect();
            let mut got: Vec<usize> = best_n(v, n, cmp).into_iter().map(|x| x.1).collect();
            want.sort_unstable();
            got.sort_unstable();
            assert_eq!(got, want, "{n}");
        }
    }

    /// A build learning the weights anew steps on the latest
    /// [`TEACH_LAST`] starts only: a long history does not take minutes;
    /// a short one teaches whole.
    #[test]
    fn a_rebuild_teaches_the_latest_starts() {
        let (cat, idx, history) = album_listener(150, 6);
        let starts = history.events.iter().filter(|p| p.kind == EventType::Start).count();
        assert!(starts > 2 * TEACH_LAST, "{starts}");
        let m = UserModel::build(&cat, &history, input(&idx), T0);
        assert!(m.weights.next.seen as usize <= TEACH_LAST && m.weights.next.seen as usize > TEACH_LAST / 2, "{}", m.weights.next.seen);
        assert!(m.weights.next.w[F_NEXT_ALBUM] > HAND[F_NEXT_ALBUM], "{:?}", m.weights.next.w);
        // Every start is still in the stats and the pairs.
        assert_eq!(m.tracks.iter().map(|t| t.starts as usize).sum::<usize>(), starts);
        let (cat, idx, short) = album_listener(20, 6);
        let teaching = short.events.iter().filter(|p| p.kind == EventType::Start).count();
        assert!(teaching < TEACH_LAST);
        assert_eq!(teach_from(&short.events), UnixMs::MIN);
        assert!(UserModel::build(&cat, &short, input(&idx), T0).weights.next.seen > 200);
    }

    /// The features of the session: the track playing now again, the first
    /// track of another album, what was listened together with the
    /// session's tracks before.
    #[test]
    fn repeats_album_starts_and_listened_together() {
        let (cat, idx, history) = album_listener(20, 7);
        let m = UserModel::build(&cat, &history, input(&idx), T0);
        // Track 5 of album 3 playing.
        let now = T0 + DAY_MS;
        let playing = tid(3 * ALBUM + 4);
        let mut session = Session::default();
        session.start(playing, now - 100_000, EventContext::Album, Some(EventOrigin::Manual));
        let mo = m.moment(&session, now);
        let at = |i: u128| m.position(tid(i)).unwrap();
        let f = |i: u128| m.features(&mo, at(i));
        assert_eq!(f(3 * ALBUM + 4)[F_IS_PREV], 1.0);
        assert_eq!(f(3 * ALBUM + 5)[F_IS_PREV], 0.0);
        assert_eq!(f(4 * ALBUM)[F_ALBUM_FIRST], 1.0, "another album's first");
        assert_eq!(f(3 * ALBUM)[F_ALBUM_FIRST], 0.0, "the playing album's own first");
        assert_eq!(f(4 * ALBUM + 1)[F_ALBUM_FIRST], 0.0);
        // Listened together, either order: the tracks of its sessions, its
        // own album's and the favourite pair's; never one of another world
        // it never shared a session with.
        let most = (0..m.len()).max_by_key(|&i| (m.bag.row(i).len(), i)).unwrap();
        let mut session = Session::default();
        session.start(m.id(most), now - 100_000, EventContext::Album, Some(EventOrigin::Manual));
        let mo = m.moment(&session, now);
        let row = m.bag.row(most);
        assert!(row.len() >= 5, "{row:?}");
        assert!(row.iter().all(|&(c, w)| w > 0.0 && m.features(&mo, c as usize)[F_SESS_CO] > 0.0));
        assert!(row.iter().any(|&(c, _)| m.world_at(c as usize) == m.world_at(most)));
        let never_with = (0..m.len()).find(|&i| i != most && m.bag.get(most, i) == 0.0 && m.world_at(i) != m.world_at(most)).unwrap();
        assert_eq!(m.features(&mo, never_with)[F_SESS_CO], 0.0);
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

    /// A listener of much rap and a few other kinds: a profile of each
    /// kind, balanced by the user's own choices; hours of soundtracks the
    /// app played on in the background raise them inside their profile but
    /// hardly in the balance; a shelf of favourites shows the small kinds
    /// at about their shares.
    #[test]
    fn profiles_balance_by_active_choices() {
        use crate::regions::spread;
        let (cat, idx, mut input, types) = crate::radio::tests::listener(true);
        let build = |input: &TasteInput| UserModel::build(&cat, input, ModelInput { index: &idx, types: Some(&types), weights: None, graph: None }, T0);
        let m = build(&input);
        assert!(m.regions.len() >= 4, "{:?}", m.regions.lanes);
        let kind = |i: usize| m.id(i).0.as_u128() / 1_000;
        let share_of = |m: &UserModel, k: u128| -> f32 {
            (0..m.regions.len())
                .filter(|&j| (0..m.len()).any(|i| m.profile_at(i) == Some(j) && kind(i) == k && m.tracks[i].starts > 0))
                .map(|j| m.profile_share[j])
                .sum()
        };
        let (rap, ost) = (share_of(&m, 0), share_of(&m, 1));
        assert!(rap > 0.45 && ost > 0.05, "rap {rap}, soundtracks {ost}");
        assert!(!m.listening.has_background, "sessions all alike: no background");
        // Three runs of a hundred soundtracks the app played on, on three
        // days, no action between.
        let ost_ids: Vec<TrackId> = (0..m.len()).filter(|&i| kind(i) == 1).map(|i| m.id(i)).collect();
        let runs = |origin: EventOrigin| {
            let mut out = Vec::new();
            for day in 1..=3 {
                let mut ts = T0 - day * DAY_MS;
                for k in 0..100 {
                    let id = ost_ids[k % ost_ids.len()];
                    out.push(Play { origin: Some(origin), ..Play::new(id, ts, EventType::Start, Some(0)) });
                    ts += 200_000;
                    out.push(Play::new(id, ts, EventType::Complete, Some(200_000)));
                }
            }
            out
        };
        let mut bg_input = input.clone();
        bg_input.events.extend(runs(EventOrigin::Autoplay));
        let bg = build(&bg_input);
        assert!(bg.listening.has_background && bg.listening.background_sessions() == 3, "{:?}", bg.listening.sessions.len());
        let (rap2, ost2) = (share_of(&bg, 0), share_of(&bg, 1));
        assert!(rap2 > ost2, "background: rap {rap2} soundtracks {ost2} (before {rap} {ost})");
        let j = bg.profile_at((0..bg.len()).find(|&i| kind(i) == 1).unwrap()).unwrap();
        assert!(bg.profile_bg[j] > 0.5, "a background profile: {:?}", bg.profile_bg);
        let i = (0..bg.len()).find(|&i| kind(i) == 1).unwrap();
        assert!(bg.tracks[i].aff_long(T0) > m.tracks[i].aff_long(T0), "counted inside its profile");
        // «Фон меньше влияет» off: the same runs count fully.
        bg_input.profiles.background_less = false;
        let full = share_of(&build(&bg_input), 1);
        assert!(full > ost2 + 0.15, "counted fully: {full} (down-weighted {ost2})");
        // The same soundtracks chosen one by one are listened to, not
        // background: an instrumental kind is never down-weighted as such.
        input.events.extend(runs(EventOrigin::Manual));
        let fg = build(&input);
        assert!(!fg.listening.has_background);
        let ost3 = share_of(&fg, 1);
        assert!(ost3 > ost2 + 0.15, "chosen soundtracks weigh: {ost3} (in the background {ost2})");

        // A shelf of ten favourites: every kind of the omnivore in it.
        let mut fav: Vec<usize> = (0..m.len()).filter(|&i| m.tracks[i].aff_long(T0) > 0.0).collect();
        fav.sort_by(|a, b| m.tracks[*b].aff_long(T0).total_cmp(&m.tracks[*a].aff_long(T0)));
        let items: Vec<(usize, Option<usize>)> = fav.iter().map(|&i| (i, m.profile_at(i))).collect();
        let shelf = spread(&items, &m.profile_share, 10);
        for k in 0..4 {
            assert!(shelf.iter().any(|&i| kind(i) == k), "kind {k} on the shelf: {:?}", shelf.iter().map(|&i| kind(i)).collect::<Vec<_>>());
        }
        // Within a profile its best loved stands at 1.
        let best = (0..m.len()).filter(|&i| kind(i) == 1).map(|i| m.profile_affinity(i)).fold(0.0f32, f32::max);
        assert!((best - 1.0).abs() < 1e-4, "{best}");
    }

    /// A thousand imported plays (another service's history) teach the
    /// long affinity of their tracks and nothing else: no step of either
    /// model, no pair, no world's presence, no transition, no hot
    /// affinity, whether they are years old or amid the user's own; and a
    /// replay scores the same starts. Years old, the weights come out about
    /// the same (the affinity they gave has faded; their tracks only join
    /// the candidates of a step).
    #[test]
    fn import_does_not_teach() {
        let (cat, idx, history) = album_listener(20, 4);
        let own = UserModel::build(&cat, &history, input(&idx), T0);
        let first = history.events.iter().map(|p| p.ts).min().unwrap();
        let with_imports = |from: UnixMs, step: i64| {
            let mut rng = Rng::new(9);
            let mut with = history.clone();
            for k in 0..1_000i64 {
                let id = tid(rng.below(300) as u128);
                let ts = from + k * step + rng.below(1000) as i64;
                let import = |kind, at, pos| Play { origin: Some(EventOrigin::Import), ..Play::new(id, at, kind, pos) };
                with.events.push(import(EventType::Start, ts, Some(0)));
                with.events.push(if k % 5 == 0 {
                    import(EventType::Skip, ts + 10_000, Some(10_000))
                } else {
                    import(EventType::Complete, ts + 200_000, Some(200_000))
                });
            }
            with
        };
        for (what, with) in [
            ("years old", with_imports(first - 3_000 * DAY_MS, 9 * HOUR_MS)),
            ("amid", with_imports(first, (T0 - first) / 1_000)),
        ] {
            let m = UserModel::build(&cat, &with, input(&idx), T0);
            assert_eq!((m.weights.next.seen, m.weights.skip.seen), (own.weights.next.seen, own.weights.skip.seen), "{what}");
            assert_eq!(m.co, own.co, "{what}: no pair");
            assert_eq!(m.worlds, own.worlds, "{what}: no world's presence");
            assert_eq!((m.lane_trans, &m.trans), (own.lane_trans, &own.trans), "{what}");
            let longer = (0..m.len()).filter(|&i| m.tracks[i].a_long > own.tracks[i].a_long + 1e-6).count();
            assert!(longer > 50, "{what}: the long affinity learnt ({longer})");
            for i in 0..m.len() {
                assert!((m.tracks[i].aff_hot(T0) - own.tracks[i].aff_hot(T0)).abs() < 1e-5, "{what}: hot {i}");
                assert_eq!((m.tracks[i].starts, m.tracks[i].early), (own.tracks[i].starts, own.tracks[i].early), "{what}");
            }
            let (a, b) = (replay(&cat, &history, input(&idx)), replay(&cat, &with, input(&idx)));
            assert_eq!((a.get("all", "learned").n, a.skip_test), (b.get("all", "learned").n, b.skip_test), "{what}");
            if what == "years old" {
                let close = |x: &[f32], y: &[f32]| x.iter().zip(y).all(|(a, b)| (a - b).abs() < 0.01);
                assert!(close(&m.weights.next.w, &own.weights.next.w) && close(&m.weights.skip.w, &own.weights.skip.w), "{:?}", m.weights);
            }
        }
    }

    /// The outcome weights: a late skip is a little plus, the app

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

    /// The library is a taste while the history is short: all of it
    /// without one, fading with the starts, none from [`COLD_UNTIL`] on or
    /// with «Без истории — по самой библиотеке» off.
    #[test]
    fn the_library_prior_fades_with_the_history() {
        let cat = Catalog::new((0..20u128).map(|i| info(i, &[i as u32 % 4], None).with_duration(Some(200_000))));
        let idx = VectorIndex::new();
        let built = |n: usize, prior: bool| {
            let mut events = Vec::new();
            for k in 0..n {
                let ts = T0 - (n - k) as i64 * 300_000;
                events.push(Play::new(tid(k as u128 % 20), ts, EventType::Start, Some(0)));
                events.push(Play::new(tid(k as u128 % 20), ts + 200_000, EventType::Complete, Some(200_000)));
            }
            UserModel::build(&cat, &TasteInput { events, library_prior: prior, ..TasteInput::default() }, input(&idx), T0)
        };
        let none = built(0, true);
        assert_eq!(none.cold(), 1.0);
        assert!(none.prior_mass(0) > 0.0);
        let some = built(30, true);
        assert!((some.cold() - 0.5).abs() < 1e-6, "{}", some.cold());
        assert!(some.prior_mass(0) < none.prior_mass(0) && some.prior_mass(0) > 0.0);
        let last = built(COLD_UNTIL - 1, true);
        assert!(last.cold() > 0.0);
        let warm = built(COLD_UNTIL, true);
        assert_eq!(warm.cold(), 0.0);
        assert_eq!(warm.prior_mass(0), 0.0);
        assert_eq!(built(0, false).prior_mass(0), 0.0);
    }

    fn build_of(cat: &Catalog, input: TasteInput) -> UserModel {
        let idx = VectorIndex::new();
        UserModel::build(cat, &input, ModelInput { index: &idx, types: None, weights: None, graph: None }, T0)
    }

    fn played(t: u128, ts: UnixMs, origin: EventOrigin, context: EventContext, end: Option<u32>, reason: Option<EndReason>) -> [Play; 2] {
        let start = Play { context, origin: Some(origin), ..Play::new(tid(t), ts, EventType::Start, Some(0)) };
        let end = match end {
            None => Play { context, ..Play::new(tid(t), ts + 200_000, EventType::Complete, Some(200_000)) },
            Some(at) => Play { context, end_reason: reason, ..Play::new(tid(t), ts + i64::from(at), EventType::Skip, Some(at)) },
        };
        [start, end]
    }

    /// A like is a state, not a moment: it counts whole while its song is
    /// heard, less as it was last heard long before the latest listening,
    /// and a bulk of likes never heard (a service's library brought at
    /// once) counts half; the mixes still read the whole like.
    #[test]
    fn an_old_love_and_a_bulk_of_likes_count_less() {
        let cat = Catalog::new((0..30u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
        let mut events = Vec::new();
        // Song 0 heard lately, song 1 last heard half a year ago.
        events.extend(played(0, T0 - DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
        events.extend(played(1, T0 - 180 * DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
        // Liked by hand: 0 and 1; 2…26 in a bulk a year ago, never heard.
        let mut like_times = vec![(tid(0), T0 - DAY_MS), (tid(1), T0 - 181 * DAY_MS)];
        like_times.extend((2..27u128).map(|t| (tid(t), T0 - 365 * DAY_MS + t as i64 * 1_000)));
        let likes = like_times.iter().map(|l| l.0).collect();
        let m = build_of(&cat, TasteInput { events, likes, like_times, ..TasteInput::default() });
        assert_eq!(m.freshness_at(0), 1.0);
        let old = m.freshness_at(1);
        assert!(old < 0.6 && old >= STALE_FLOOR, "{old}");
        assert_eq!(m.freshness_at(5), UNHEARD_LOVE, "a bulk like says nothing of when");
        let (t0, t1, t5) = (&m.tracks[0], &m.tracks[1], &m.tracks[5]);
        assert!(t1.aff_long(T0) < t1.aff_long_whole(T0));
        assert!(t5.aff_long(T0) < t5.aff_long_whole(T0));
        assert!(t0.aff_long(T0) > t1.aff_long(T0));
        assert_eq!(t5.aff_long_whole(T0), m.tracks[6].aff_long_whole(T0));
        // The model's «liked» fades too.
        let mo = m.moment(&Session::default(), T0);
        assert_eq!(m.features(&mo, 0)[2], 1.0);
        assert!(m.features(&mo, 5)[2] < 1.0 && m.features(&mo, 5)[2] > 0.0);
    }

    /// Skipped early in most of its last plays, a liked song is worn out
    /// now: its chance of an early skip is at least theirs whatever the
    /// model says; one skip in three is no wear.
    #[test]
    fn a_liked_song_skipped_lately_is_worn_out() {
        let mut t = TrackStats::default();
        for early in [false, false, false, false, true, true, true] {
            t.note(early);
        }
        assert_eq!(t.burnt(0.1), None, "three of the last six is not most");
        t.note(true);
        let p = t.burnt(0.1).expect("four of the last six");
        assert!((p - (4.0 + 0.2) / 8.0).abs() < 1e-6, "{p}");
        let mut t = TrackStats::default();
        for early in [false, true, false, false] {
            t.note(early);
        }
        assert_eq!(t.burnt(0.1), None);
        let mut t = TrackStats::default();
        for early in [true, true] {
            t.note(early);
        }
        assert_eq!(t.burnt(0.1), None, "too few outcomes");

        let cat = Catalog::new((0..3u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
        let mut events = Vec::new();
        let mut ts = T0 - 20 * DAY_MS;
        for _ in 0..6 {
            events.extend(played(0, ts, EventOrigin::Manual, EventContext::Library, None, None));
            events.extend(played(1, ts + HOUR_MS, EventOrigin::Manual, EventContext::Library, None, None));
            ts += DAY_MS;
        }
        for _ in 0..4 {
            events.extend(played(0, ts, EventOrigin::Radio, EventContext::Radio, Some(15_000), Some(EndReason::UserNext)));
            ts += DAY_MS;
        }
        let likes = vec![tid(0), tid(1)];
        let m = build_of(&cat, TasteInput { events, likes, ..TasteInput::default() });
        assert!(m.tracks[0].liked(T0));
        let mo = m.moment(&Session::default(), T0);
        let (worn, kept) = (m.p_skip(&mo, 0, EventContext::Radio), m.p_skip(&mo, 1, EventContext::Radio));
        assert!(worn >= 0.55 && worn > kept + 0.1, "{worn} {kept}");
        assert_eq!(m.skip_risk(1, 0.2), 0.2);
    }

    /// A song the app chose and the app closed on in its first seconds was
    /// skipped early; closed on at once, or the user's own pick, it says
    /// nothing.
    #[test]
    fn a_stream_song_left_at_once_is_an_early_skip() {
        let cat = Catalog::new((0..4u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
        let mut events = Vec::new();
        events.extend(played(0, T0 - 4 * HOUR_MS, EventOrigin::Radio, EventContext::Radio, Some(12_000), Some(EndReason::AppClosed)));
        events.extend(played(1, T0 - 3 * HOUR_MS, EventOrigin::Radio, EventContext::Radio, Some(500), Some(EndReason::AppClosed)));
        events.extend(played(2, T0 - 2 * HOUR_MS, EventOrigin::Manual, EventContext::Album, Some(12_000), Some(EndReason::AppClosed)));
        events.extend(played(3, T0 - HOUR_MS, EventOrigin::Mix, EventContext::HomeSection, Some(60_000), Some(EndReason::AppClosed)));
        let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
        assert_eq!(m.tracks[0].early, 1, "left in its first seconds");
        assert_eq!((m.tracks[1].early, m.tracks[2].early, m.tracks[3].early), (0, 0, 0));
        assert_eq!((m.tracks[3].late, m.tracks[1].late), (0, 0), "the app closed: no outcome");
    }

    /// The graph's prior is a cold start: whole for a song never started,
    /// fading with its own starts.
    #[test]
    fn the_graphs_prior_fades_with_a_songs_plays() {
        let cat = Catalog::new((0..2u128).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
        let mut events = Vec::new();
        for k in 0..3 {
            events.extend(played(1, T0 - (k + 1) * DAY_MS, EventOrigin::Manual, EventContext::Library, None, None));
        }
        let m = build_of(&cat, TasteInput { events, ..TasteInput::default() });
        assert_eq!(m.tracks[1].starts, 3);
        // No graph here: both priors are 0, and the fade keeps them so.
        assert_eq!(m.prior_at(0), m.graph.prior(0));
        assert_eq!(m.prior_at(1), m.graph.prior(1) / 4.0);
    }

    /// A session's rejections push the candidates by both sounds where the
    /// neural model heard them: two songs bliss cannot tell apart from the
    /// skipped one, one of its genre and one of another, are pushed apart
    /// by the model's vectors; without a rejection there is nothing to
    /// measure.
    #[test]
    fn a_rejection_pushes_by_both_sounds() {
        use crate::embedding::{COMPACT_DIM, Compact};
        let n = 40u128;
        let cat = Catalog::new((0..n).map(|i| info(i, &[i as u32], Some(i)).with_duration(Some(200_000))));
        let mut idx = VectorIndex::new();
        let mut rng = Rng::new(5);
        for i in 0..n {
            let x = match i {
                0 => 0.0,
                1 | 2 => 0.05,
                _ => i as f32 * 0.3,
            };
            idx.insert(tid(i), line(x));
            // Two genres: the even dimensions and the odd ones.
            let genre = match i {
                1 => 1,
                0 | 2 => 0,
                _ => (i % 2) as usize,
            };
            let v: [f32; COMPACT_DIM] = std::array::from_fn(|j| if j % 2 == genre { 1.0 } else { 0.0 } + 0.3 * rng.f32());
            assert!(idx.set_embedding(tid(i), Compact::of_unit(&v).unwrap()));
        }
        let m = UserModel::build(&cat, &TasteInput::default(), ModelInput { index: &idx, types: None, weights: None, graph: None }, T0);
        let mut s = Session::default();
        s.start(tid(0), T0 - 60_000, EventContext::Radio, Some(EventOrigin::Radio));
        s.end_at(tid(0), Ended::Early, 5_000, Some(200_000), false);
        assert_eq!(s.pushed(&m).len(), s.pushes(&m).len());
        let probe = m.push_probe(&s, Some(&idx)).expect("a rejection, an index that blends");
        let (one, two) = (m.position(tid(1)).unwrap(), m.position(tid(2)).unwrap());
        let mo = m.moment(&s, T0);
        let bliss = (m.skip_push_of(&mo.pushes, one), m.skip_push_of(&mo.pushes, two));
        assert!(bliss.1 < 0.0 && (bliss.0 - bliss.1).abs() < 1e-6, "{bliss:?}");
        let (other, same) = (m.skip_push_by(&probe, one), m.skip_push_by(&probe, two));
        assert!(same < other - 0.05, "the skipped genre is pushed harder: {same} {other}");
        assert!(m.push_probe(&Session::default(), Some(&idx)).is_none());
        assert!(m.push_probe(&s, None).is_none());
    }
}
