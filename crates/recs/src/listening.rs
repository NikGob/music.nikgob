//! How the user listens (profiles-v1 §4): in the background or not, one
//! profile or many. Nothing here is a fixed cut of minutes or touches:
//! each signal of a session is ranked among the user's own sessions.
//!
//! * **Signals** ([`SessionSignals`]): touches per track (early and late
//!   skips, seeks, likes, the user's own picks inside the session), the
//!   session's length, the share of starts that followed a track heard
//!   out by themselves, and how far the window was while it played (0
//!   focused, 0.5 in view behind another window, 1 hidden; where known).
//!   The hour and the kind of song never enter: the background-ness of a
//!   profile is what comes out, not what goes in.
//! * **Score** ([`background_scores`]): the mean of the touches' rank
//!   (fewer higher), the length's and the starts' by themselves among the
//!   user's sessions of three starts or more, and the window as it is.
//! * **Threshold**: Otsu's over the user's scores, trusted when the two
//!   classes separate (η ≥ [`ETA_MIN`]) and the upper one is passive in
//!   fact (under [`PASSIVE_IA`] touches per track and at most
//!   [`PASSIVE_REL`] of the lower's); else a fixed band of the ranked
//!   score that only the most passive sessions reach. `p_bg` is soft, 0…1,
//!   around it ([`p_background`]).
//! * **Only who listens in the background has it** ([`Listening::of`]): a
//!   user without [`EXPLICIT_SESSIONS`] plainly background sessions (long,
//!   hardly a touch) gets `p_bg = 0` everywhere, so nobody who listens
//!   attentively is down-weighted by chance.
//! * **Pure or mixed** ([`infer_mode`]): a session of one profile at
//!   [`PURE_SHARE`] of its starts is pure; the first three starts guess it.
//!
//! A background play teaches its own profile at [`W_BG`] and takes no share
//! from the others ([`crate::profiles`]).

use std::collections::HashMap;

use nikgob_core::model::UnixMs;
use serde::{Deserialize, Serialize};

use crate::session::{Ended, Session};
use crate::taste::SESSION_GAP_MS;

/// A session ranks among the user's from this many starts.
pub const MIN_STARTS: usize = 3;
/// Otsu's threshold needs this many sessions.
pub const MIN_SESSIONS: usize = 6;
/// Otsu's separation of the user's sessions needed to trust its threshold.
pub const ETA_MIN: f32 = 0.55;
/// The upper class is background in fact: fewer touches per track than
/// this…
pub const PASSIVE_IA: f32 = 0.2;
/// …and at most this share of the other sessions' (the definition of
/// background, not a setting of the person: without it Otsu always finds
/// something).
pub const PASSIVE_REL: f32 = 0.5;
/// The soft band around the user's threshold.
const BAND: f32 = 0.08;
/// Without a threshold of the user's: `p_bg` grows from 0 to 1 over this
/// band of the ranked score.
const FALLBACK: (f32, f32) = (0.6, 0.85);
/// A background play counts this much for the balance of the profiles
/// («Фон меньше влияет на рекомендации»; off, 1).
pub const W_BG: f32 = 0.25;
/// A session is plainly background with `p_bg` ≥ ½, under [`PASSIVE_IA`]
/// touches per track and this many starts at least…
pub const EXPLICIT_STARTS: usize = 8;
/// …and a user listens in the background with this many such sessions.
pub const EXPLICIT_SESSIONS: usize = 2;
/// A focused session's `p_bg` is under this.
pub const FOCUS_P_BG: f32 = 0.2;
/// A session is pure with this share of its starts in one profile.
pub const PURE_SHARE: f32 = 0.8;
/// The last track of a session is taken this long when its end is unknown.
const LAST_TRACK_MS: f32 = 210_000.0;

/// One start of the user's own, as the modes read it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    pub ts: UnixMs,
    /// When it ended, if it did.
    pub end: Option<UnixMs>,
    pub ended: Ended,
    /// The user picked it (not the queue or a recommendation going on).
    pub manual: bool,
    /// Seeks and likes while it played.
    pub touches: u32,
    /// How far the window was while it played ([`crate::taste::Play::attention`]).
    pub attention: Option<f32>,
    pub duration_ms: Option<u32>,
}

impl Heard {
    /// How long it played (its length when its end is unknown).
    fn played_ms(&self) -> f32 {
        match self.end {
            Some(e) if e > self.ts => (e - self.ts) as f32,
            _ => self.duration_ms.map_or(LAST_TRACK_MS, |d| d as f32),
        }
    }
}

/// What a session says of how it was listened (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionSignals {
    pub starts: u32,
    /// Touches per track.
    pub ia: f32,
    pub minutes: f32,
    /// The share of starts after a track heard out, not the user's pick.
    pub auto: f32,
    /// How far the window was, where known for half of the time at least.
    pub hidden: Option<f32>,
}

impl SessionSignals {
    /// The signals of one session's starts, oldest first.
    pub fn of(run: &[Heard]) -> Self {
        let Some((first, last)) = run.first().zip(run.last()) else { return Self::default() };
        let n = run.len();
        let (mut touches, mut auto) = (0u32, 0u32);
        for (k, h) in run.iter().enumerate() {
            touches += u32::from(matches!(h.ended, Ended::Early | Ended::Late)) + h.touches;
            if k > 0 {
                if h.manual {
                    touches += 1;
                } else if run[k - 1].ended == Ended::Complete {
                    auto += 1;
                }
            }
        }
        let minutes = ((last.ts - first.ts) as f32 + last.played_ms()) / 60_000.0;
        let (mut known, mut away, mut all) = (0.0f32, 0.0f32, 0.0f32);
        for h in run {
            let ms = h.played_ms();
            all += ms;
            if let Some(a) = h.attention {
                known += ms;
                away += ms * a;
            }
        }
        Self {
            starts: n as u32,
            ia: touches as f32 / n as f32,
            minutes,
            auto: auto as f32 / (n.max(2) - 1) as f32,
            hidden: (all > 0.0 && known >= 0.5 * all).then(|| away / known),
        }
    }

    /// The signals of a live session ([`Session`]): its ends and who chose
    /// each start (seeks, likes and the window are not in it); `duration`
    /// tells a track's length.
    pub fn of_session(session: &Session, duration: impl Fn(nikgob_core::model::TrackId) -> Option<u32>) -> Self {
        let played: Vec<_> = session.tracks().collect();
        let run: Vec<Heard> = played
            .iter()
            .enumerate()
            .map(|(k, p)| Heard {
                ts: p.ts,
                end: played.get(k + 1).map(|n| n.ts),
                ended: p.ended,
                manual: p.origin == Some(nikgob_core::model::EventOrigin::Manual),
                touches: 0,
                attention: None,
                duration_ms: duration(p.track),
            })
            .collect();
        Self::of(&run)
    }
}

/// The sessions of `starts` (by time): runs without a pause of
/// [`SESSION_GAP_MS`], as ranges of `starts`.
pub fn sessions(starts: &[Heard]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    for k in 1..=starts.len() {
        if k == starts.len() || starts[k].ts - starts[k - 1].ts >= SESSION_GAP_MS {
            if k > from {
                out.push(from..k);
            }
            from = k;
        }
    }
    out
}

/// The percentile (0…1) of each value among `values`, ties averaged.
pub fn pct_rank(values: &[f32]) -> Vec<f32> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f32::total_cmp);
    values.iter().map(|v| rank_in(&sorted, *v)).collect()
}

/// The percentile of `v` among `sorted`, ties counted half.
fn rank_in(sorted: &[f32], v: f32) -> f32 {
    if sorted.is_empty() {
        return 0.5;
    }
    let below = sorted.partition_point(|x| *x < v);
    let upto = sorted.partition_point(|x| *x <= v);
    (below as f32 + 0.5 * (upto - below) as f32) / sorted.len() as f32
}

/// The background score of each session (see the module docs): the mean
/// of its signals, each ranked among `rows`, the window as it is.
pub fn background_scores(rows: &[SessionSignals]) -> Vec<f32> {
    let ia = pct_rank(&rows.iter().map(|r| r.ia).collect::<Vec<_>>());
    let len = pct_rank(&rows.iter().map(|r| r.minutes).collect::<Vec<_>>());
    let auto = pct_rank(&rows.iter().map(|r| r.auto).collect::<Vec<_>>());
    rows.iter().enumerate().map(|(k, r)| score_of(1.0 - ia[k], len[k], auto[k], r.hidden)).collect()
}

fn score_of(calm: f32, len: f32, auto: f32, hidden: Option<f32>) -> f32 {
    match hidden {
        Some(h) => (calm + len + auto + h) / 4.0,
        None => (calm + len + auto) / 3.0,
    }
}

/// Otsu's threshold of `scores` and its separation η (the variance
/// between the two classes over the whole, 0…1); `None` under
/// [`MIN_SESSIONS`] scores or with no spread.
pub fn otsu(scores: &[f32]) -> Option<(f32, f32)> {
    let mut xs = scores.to_vec();
    xs.sort_by(f32::total_cmp);
    let n = xs.len();
    if n < MIN_SESSIONS {
        return None;
    }
    let mean = xs.iter().sum::<f32>() / n as f32;
    let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n as f32;
    if var <= 1e-9 {
        return None;
    }
    let mut best = (0.0f32, None);
    let mut low = 0.0f32;
    for i in 1..n {
        low += xs[i - 1];
        let (w0, w1) = (i as f32 / n as f32, (n - i) as f32 / n as f32);
        let (m0, m1) = (low / i as f32, (mean * n as f32 - low) / (n - i) as f32);
        let between = w0 * w1 * (m0 - m1).powi(2);
        if between > best.0 {
            best = (between, Some((xs[i - 1] + xs[i]) / 2.0));
        }
    }
    best.1.map(|t| (t, best.0 / var))
}

/// The soft background weights of sessions with `scores` and touches per
/// track `ia`: around the user's own threshold when it is trusted (see the
/// module docs), else over the fixed band. Returns the weights, the
/// threshold (when it is the user's) and the separation.
pub fn p_background(scores: &[f32], ia: &[f32]) -> (Vec<f32>, Option<f32>, f32) {
    let split = otsu(scores);
    let eta = split.map_or(0.0, |s| s.1);
    let own = split.filter(|(thr, eta)| {
        if *eta < ETA_MIN {
            return false;
        }
        let mean = |up: bool| {
            let v: Vec<f32> = scores.iter().zip(ia).filter(|(s, _)| (**s > *thr) == up).map(|(_, a)| *a).collect();
            (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32)
        };
        matches!((mean(true), mean(false)), (Some(up), Some(down)) if up <= PASSIVE_IA && up <= PASSIVE_REL * down)
    });
    let thr = own.map(|s| s.0);
    (scores.iter().map(|s| p_of(*s, thr)).collect(), thr, eta)
}

fn p_of(score: f32, thr: Option<f32>) -> f32 {
    match thr {
        Some(t) => (0.5 + (score - t) / (2.0 * BAND)).clamp(0.0, 1.0),
        None => ((score - FALLBACK.0) / (FALLBACK.1 - FALLBACK.0)).clamp(0.0, 1.0),
    }
}

/// One session of the user's, as [`Listening`] keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionMode {
    pub start: UnixMs,
    pub end: UnixMs,
    pub signals: SessionSignals,
    pub score: f32,
    pub p_bg: f32,
}

/// The user's listening modes (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Listening {
    /// The sessions of [`MIN_STARTS`] or more, oldest first.
    pub sessions: Vec<SessionMode>,
    /// The user's own threshold, when trusted.
    pub threshold: Option<f32>,
    pub eta: f32,
    /// The user has plainly background sessions ([`EXPLICIT_SESSIONS`]):
    /// background exists as a mode for them at all.
    pub has_background: bool,
    /// The signals of those sessions, sorted, to rank a live one.
    ia: Vec<f32>,
    minutes: Vec<f32>,
    auto: Vec<f32>,
}

impl Listening {
    /// The modes of the user's own `starts` (by time) and `p_bg` of each
    /// start (its session's).
    pub fn of(starts: &[Heard]) -> (Self, Vec<f32>) {
        let mut p = vec![0.0f32; starts.len()];
        let runs: Vec<_> = sessions(starts).into_iter().filter(|r| r.len() >= MIN_STARTS).collect();
        let rows: Vec<SessionSignals> = runs.iter().map(|r| SessionSignals::of(&starts[r.clone()])).collect();
        if rows.is_empty() {
            return (Self::default(), p);
        }
        let scores = background_scores(&rows);
        let ia: Vec<f32> = rows.iter().map(|r| r.ia).collect();
        let (mut pbg, threshold, eta) = p_background(&scores, &ia);
        let explicit = rows
            .iter()
            .zip(&pbg)
            .filter(|(r, p)| **p >= 0.5 && r.ia < PASSIVE_IA && r.starts as usize >= EXPLICIT_STARTS)
            .count();
        let has_background = explicit >= EXPLICIT_SESSIONS;
        if !has_background {
            pbg.iter_mut().for_each(|x| *x = 0.0);
        }
        let mut sessions = Vec::with_capacity(runs.len());
        for (k, r) in runs.iter().enumerate() {
            for x in &mut p[r.clone()] {
                *x = pbg[k];
            }
            let last = &starts[r.end - 1];
            sessions.push(SessionMode {
                start: starts[r.start].ts,
                end: last.ts + last.played_ms() as i64,
                signals: rows[k],
                score: scores[k],
                p_bg: pbg[k],
            });
        }
        let sorted = |f: fn(&SessionSignals) -> f32| {
            let mut v: Vec<f32> = rows.iter().map(f).collect();
            v.sort_by(f32::total_cmp);
            v
        };
        let me = Self {
            sessions,
            threshold: threshold.filter(|_| has_background),
            eta,
            has_background,
            ia: sorted(|r| r.ia),
            minutes: sorted(|r| r.minutes),
            auto: sorted(|r| r.auto),
        };
        (me, p)
    }

    /// `p_bg` of a live session with `s`, ranked among the user's sessions;
    /// every touch lowers it at once, down to none for a run touched twice
    /// [`PASSIVE_IA`] per track (the cancel of a background mix is an
    /// action, no switch). 0 for a user without background, and before
    /// [`MIN_STARTS`].
    pub fn p_now(&self, s: &SessionSignals) -> f32 {
        if !self.has_background || (s.starts as usize) < MIN_STARTS {
            return 0.0;
        }
        let score = score_of(1.0 - rank_in(&self.ia, s.ia), rank_in(&self.minutes, s.minutes), rank_in(&self.auto, s.auto), s.hidden);
        let passive = (1.0 - s.ia / (2.0 * PASSIVE_IA)).clamp(0.0, 1.0);
        p_of(score, self.threshold) * passive
    }

    /// `p_bg` of the session that played at `ts` (0 outside the sessions of
    /// [`MIN_STARTS`]).
    pub fn p_at(&self, ts: UnixMs) -> f32 {
        let k = self.sessions.partition_point(|s| s.start <= ts);
        k.checked_sub(1).map(|k| &self.sessions[k]).filter(|s| ts <= s.end).map_or(0.0, |s| s.p_bg)
    }

    /// A live session with `s` is focused (profiles-v1 §4.1): hardly
    /// background (`p_bg` under [`FOCUS_P_BG`]) and touched more than the
    /// user's median session. Never before [`MIN_STARTS`].
    pub fn focused(&self, s: &SessionSignals) -> bool {
        (s.starts as usize) >= MIN_STARTS && !self.ia.is_empty() && rank_in(&self.ia, s.ia) > 0.5 && self.p_now(s) < FOCUS_P_BG
    }

    /// How many of the sessions are background (`p_bg` ≥ ½).
    pub fn background_sessions(&self) -> usize {
        self.sessions.iter().filter(|s| s.p_bg >= 0.5).count()
    }
}

/// What a session is: one profile, or many.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// [`PURE_SHARE`] of it in one profile.
    Pure(usize),
    Mixed,
}

/// The mode of a session whose starts were of `profiles` (oldest first):
/// pure when [`PURE_SHARE`] of them are of one profile; with three or
/// fewer, pure only when all of them are (the first three guess it).
/// `None` before a start of a known profile.
pub fn infer_mode(profiles: &[Option<usize>]) -> Option<Mode> {
    let known: Vec<usize> = profiles.iter().flatten().copied().collect();
    if known.is_empty() {
        return None;
    }
    let mut count: HashMap<usize, usize> = HashMap::new();
    for j in &known {
        *count.entry(*j).or_default() += 1;
    }
    let (&top, &n) = count.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))?;
    let pure = if known.len() <= 3 { n == known.len() } else { n as f32 >= PURE_SHARE * known.len() as f32 };
    Some(if pure { Mode::Pure(top) } else { Mode::Mixed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    const T0: UnixMs = 1_000_000_000_000;

    /// A session of `n` tracks of 200 s from `at`: each touched with `p`,
    /// the window `hidden`.
    fn session(rng: &mut Rng, at: UnixMs, n: usize, p: f32, hidden: Option<f32>) -> Vec<Heard> {
        (0..n)
            .map(|k| {
                let skip = rng.f32() < p;
                let ts = at + k as i64 * 200_000;
                Heard {
                    ts,
                    end: Some(ts + if skip { 15_000 } else { 200_000 }),
                    ended: if skip { Ended::Early } else { Ended::Complete },
                    manual: k > 0 && rng.f32() < p,
                    touches: u32::from(rng.f32() < p / 2.0),
                    attention: hidden,
                    duration_ms: Some(200_000),
                }
            })
            .collect()
    }

    /// Two classes of sessions (short and touched, long and untouched):
    /// Otsu's threshold of the user's own sits between them, and every long
    /// one is background.
    #[test]
    fn otsu_on_two_classes() {
        let mut rng = Rng::new(1);
        let mut starts = Vec::new();
        let mut at = T0;
        for k in 0..40 {
            let bg = k % 4 == 0;
            let n = if bg { 25 + rng.below(20) } else { 4 + rng.below(10) };
            starts.extend(session(&mut rng, at, n, if bg { 0.02 } else { 0.5 }, Some(if bg { 0.9 } else { 0.2 })));
            at += 20 * 3_600_000;
        }
        let (l, p) = Listening::of(&starts);
        assert!(l.has_background);
        let thr = l.threshold.expect("the user's own threshold");
        assert!(l.eta >= ETA_MIN, "{}", l.eta);
        for (k, s) in l.sessions.iter().enumerate() {
            assert_eq!(s.p_bg >= 0.5, k % 4 == 0, "session {k}: score {} threshold {thr}", s.score);
        }
        assert!(p.iter().filter(|x| **x >= 0.5).count() > starts.len() / 2, "most plays are in the long runs");
    }

    /// Everyone touches about every other track: whatever the lengths and
    /// the window, nothing is background and nothing is down-weighted.
    #[test]
    fn no_background_user_gets_none() {
        let mut rng = Rng::new(2);
        let mut starts = Vec::new();
        let mut at = T0;
        for _ in 0..80 {
            let n = 3 + rng.below(30);
            let hidden = (rng.f32() < 0.3).then(|| rng.f32());
            starts.extend(session(&mut rng, at, n, 0.5, hidden));
            at += 9 * 3_600_000;
        }
        let (l, p) = Listening::of(&starts);
        assert!(!l.has_background, "{:?}", l.sessions.iter().map(|s| (s.signals.ia, s.p_bg)).collect::<Vec<_>>());
        assert!(p.iter().all(|x| *x == 0.0));
        let s = SessionSignals { starts: 40, ia: 0.0, minutes: 300.0, auto: 1.0, hidden: Some(1.0) };
        assert_eq!(l.p_now(&s), 0.0, "not even a live run");
    }

    /// A user who never skips or seeks: the touches say nothing, the window
    /// and the length still tell the background runs.
    #[test]
    fn never_skips_uses_window() {
        let mut rng = Rng::new(3);
        let mut starts = Vec::new();
        let mut at = T0;
        let mut truth = Vec::new();
        for k in 0..60 {
            let bg = k % 3 == 0;
            let n = if bg { 15 + rng.below(25) } else { 4 + rng.below(16) };
            let hidden = if bg { 0.85 + rng.f32() * 0.15 } else { rng.f32() * 0.3 };
            let mut s = session(&mut rng, at, n, 0.0, Some(hidden));
            if !bg {
                // The user picks now and then, never skips.
                for h in s.iter_mut().skip(1) {
                    h.manual = rng.f32() < 0.3;
                }
            }
            starts.extend(s);
            truth.push(bg);
            at += 20 * 3_600_000;
        }
        let (l, _) = Listening::of(&starts);
        assert!(l.has_background);
        let rows: Vec<(f32, bool)> = l.sessions.iter().zip(&truth).map(|(s, t)| (s.score, *t)).collect();
        let auc = crate::model::auc(&rows).unwrap();
        assert!(auc >= 0.95, "{auc}");
        let right = l.sessions.iter().zip(&truth).filter(|(s, t)| (s.p_bg >= 0.5) == **t).count();
        assert!(right as f32 >= 0.9 * truth.len() as f32, "{right} of {}", truth.len());
    }

    /// A background run goes on with nothing touched; every touch of the
    /// user's lowers its `p_bg` at once, and a few in a short run cancel it.
    #[test]
    fn touch_cancels_background() {
        let mut rng = Rng::new(4);
        let mut starts = Vec::new();
        let mut at = T0;
        for k in 0..30 {
            let bg = k % 3 == 0;
            starts.extend(session(&mut rng, at, if bg { 30 } else { 8 }, if bg { 0.0 } else { 0.5 }, Some(if bg { 1.0 } else { 0.2 })));
            at += 20 * 3_600_000;
        }
        let (l, _) = Listening::of(&starts);
        let run = |starts: u32, touches: u32| {
            let ia = touches as f32 / starts as f32;
            let auto = (starts - 1 - touches.min(starts - 1)) as f32 / (starts - 1) as f32;
            l.p_now(&SessionSignals { starts, ia, minutes: starts as f32 * 200.0 / 60.0, auto, hidden: Some(1.0) })
        };
        let calm = run(12, 0);
        assert!(calm >= 0.5, "{calm}");
        let mut last = calm;
        for touches in 1..=4 {
            let p = run(12, touches);
            assert!(p < last, "{touches} touches: {last} → {p}");
            last = p;
        }
        assert!(run(6, 3) < 0.2, "a skip and two picks in six: {}", run(6, 3));
    }

    #[test]
    fn pure_and_mixed() {
        assert_eq!(infer_mode(&[Some(2), Some(2), Some(2)]), Some(Mode::Pure(2)));
        assert_eq!(infer_mode(&[Some(2), None, Some(1)]), Some(Mode::Mixed));
        assert_eq!(infer_mode(&[Some(1); 9].iter().copied().chain([Some(0)]).collect::<Vec<_>>()), Some(Mode::Pure(1)));
        assert_eq!(infer_mode(&[Some(1), Some(1), Some(1), Some(0), Some(0)]), Some(Mode::Mixed));
        assert_eq!(infer_mode(&[None, None]), None);
    }
}
