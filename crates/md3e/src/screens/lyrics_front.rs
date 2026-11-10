//! The pace of the light along a sung line: where its front is at each
//! moment, worked out from the times of the line's pieces.
//!
//! A line's timing says when each piece (a syllable, a word) starts and
//! ends. A front that keeps to every one of those times exactly changes
//! speed at each piece: a short piece flies by, a long one drags, and the
//! times of a text timed by ear or by a model are a few hundredths off
//! anyway. So the front goes through *gates* instead: each place on the
//! line with a time has to be passed no sooner than that time (the light
//! never runs ahead of the voice) and no later than a tolerance after it.
//! Through the gates it takes the straightest way there is (a taut string),
//! which is one even pace wherever the times allow it and bends only where
//! they do not: at a note held long, at a pause. That way is then averaged
//! over the last moments, which rounds its bends: the front's speed has no
//! step anywhere. It is a function of the song's position alone, so it is
//! the same at any frame rate and after a seek either way.

/// Pieces sung with a pause up to this long (seconds) between them are one
/// run: the front goes on from one into the next. Over a longer pause it
/// finishes the run, waits with it lit whole and comes onto the next anew.
pub(super) const BRIDGE: f32 = 0.3;
/// How late (seconds) the front may pass a piece's start or end at full
/// «Плавность».
const LATE: f32 = 0.14;
/// A place inside a piece (the end of a letter) has no time of its own, only
/// a guess by the letters' weights: at full «Плавность» it may be passed
/// this share of the piece's length (of its first second) sooner or later
/// than the guess, never outside the piece's own times.
const GUESS: f32 = 0.3;
/// The seconds the front's way is averaged over, at no «Плавность» and at
/// full: a few frames, so its speed never steps, to a calm glide. Not
/// longer: the front is this much late at most, and a run's first letter
/// has to light with the voice.
const WINDOW: (f32, f32) = (0.05, 0.18);
/// The soft edge takes this long (seconds) to leave a run's last letter: at
/// the run's own last pace, within these bounds.
const OUT: (f32, f32) = (0.04, 0.3);
/// Gates are kept this far apart (px) along the line, so each is ahead of
/// the one before even where two share a place (a pause inside a word).
const APART: f64 = 1e-3;

/// One timed span of a line on the front's way: `from`..`to` px along the
/// line's advances, sung from `start` to `end` seconds.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Piece {
    pub start: f32,
    pub end: f32,
    pub from: f32,
    pub to: f32,
    /// Its marks (in the list given with the pieces): places inside it with
    /// a guessed time, in order.
    pub marks: std::ops::Range<usize>,
}

/// How the front keeps to the times: «Плавность» as the gates' tolerance
/// and the averaging window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Ease {
    /// How late a timed place may be passed, seconds.
    late: f32,
    /// The share of a piece's length a guessed place may be off its guess.
    guess: f32,
    /// The seconds the way is averaged over.
    window: f32,
}

impl Ease {
    /// `smooth`: 0 keeps to the times exactly .. 1 the calmest glide.
    pub(super) fn of(smooth: f32) -> Self {
        let s = smooth.clamp(0.0, 1.0);
        Ease { late: LATE * s, guess: GUESS * s, window: WINDOW.0 + (WINDOW.1 - WINDOW.0) * s }
    }

    /// The times themselves: no tolerance and no averaging.
    #[cfg(test)]
    pub(super) const EXACT: Ease = Ease { late: 0.0, guess: 0.0, window: 0.0 };

    /// The longest the front may be behind the exact one, seconds.
    #[cfg(test)]
    pub(super) fn behind(self) -> f32 {
        self.late + self.guess + self.window
    }
}

/// A place the front passes within a span of time.
#[derive(Clone, Copy, Debug)]
struct Gate {
    x: f64,
    /// No sooner than this, seconds.
    a: f64,
    /// No later than this.
    b: f64,
}

/// A bend of a run's way, with the way's integrals up to it.
#[derive(Clone, Copy, Debug)]
struct Knot {
    /// Seconds from the run's start.
    t: f64,
    x: f64,
    /// ∫ and ∬ of the way (less its first place) from the run's start.
    i1: f64,
    i2: f64,
}

/// The front over one run of pieces: its way (time to place, both rising)
/// and the window it is averaged over.
#[derive(Clone, Debug)]
pub(super) struct Run {
    /// When it starts, seconds.
    t0: f64,
    knots: Vec<Knot>,
    /// Half the averaging window, seconds.
    h: f64,
}

/// The runs of the front over `pieces` (in the order they are written and
/// sung; `marks` are the places inside them, as (px, seconds)) and the run
/// of each piece. `edge` is the width of the light's soft edge, whose
/// middle the front is: it starts half an edge before a run's first letter
/// (nothing lit) and ends half an edge past its last (all lit).
pub(super) fn runs(pieces: &[Piece], marks: &[(f32, f32)], edge: f32, ease: Ease) -> (Vec<Run>, Vec<usize>) {
    let mut out = Vec::new();
    let mut of = Vec::with_capacity(pieces.len());
    let mut gates: Vec<Gate> = Vec::new();
    let half = f64::from(edge.max(0.0)) * 0.5;
    let late = f64::from(ease.late);
    let mut first = 0;
    while first < pieces.len() {
        let mut last = first;
        while last + 1 < pieces.len() && pieces[last + 1].start - pieces[last].end <= BRIDGE {
            last += 1;
        }
        gates.clear();
        let p = &pieces[first];
        gates.push(Gate { x: f64::from(p.from) - half, a: f64::from(p.start), b: f64::from(p.start) });
        for i in first..=last {
            let p = &pieces[i];
            let (start, end) = (f64::from(p.start), f64::from(p.end.max(p.start)));
            // Where it starts, unless it goes on right from the one before.
            if i > first && (p.from > pieces[i - 1].to + 0.25 || p.start > pieces[i - 1].end + 1e-3) {
                gates.push(Gate { x: f64::from(p.from), a: start, b: start + late });
            }
            let guess = f64::from(ease.guess) * (end - start).min(1.0);
            for &(x, t) in marks.get(p.marks.clone()).unwrap_or(&[]) {
                let t = f64::from(t).clamp(start, end);
                gates.push(Gate { x: f64::from(x), a: (t - guess).max(start), b: t + guess + late });
            }
            gates.push(Gate { x: f64::from(p.to), a: end, b: end + late });
        }
        // The edge leaves the last letter at the pace it crossed it.
        let p = &pieces[last];
        if half > 0.0 {
            let (width, time) = (f64::from(p.to - p.from), f64::from(p.end - p.start));
            let out_s = if width > 0.0 && time > 0.0 { half * time / width } else { f64::from(OUT.1) };
            let at = f64::from(p.end.max(p.start)) + out_s.clamp(f64::from(OUT.0), f64::from(OUT.1));
            gates.push(Gate { x: f64::from(p.to) + half, a: at, b: at });
        } else if let Some(g) = gates.last_mut() {
            g.b = g.a;
        }
        out.push(Run::through(&mut gates, f64::from(ease.window)));
        of.extend(std::iter::repeat_n(out.len() - 1, last + 1 - first));
        first = last + 1;
    }
    (out, of)
}

impl Run {
    /// The run through `gates` (the first and the last are points),
    /// averaged over `window` seconds.
    fn through(gates: &mut [Gate], window: f64) -> Self {
        // Times and places both rise: a gate before the one ahead of it (a
        // timing slip) waits for it.
        for k in 1..gates.len() {
            let before = gates[k - 1];
            let g = &mut gates[k];
            g.x = g.x.max(before.x);
            g.a = g.a.max(before.a);
            g.b = g.b.max(g.a);
        }
        let t0 = gates.first().map_or(0.0, |g| g.a);
        let way = taut(gates);
        let x0 = way.first().map_or(0.0, |k| k.1);
        let mut knots: Vec<Knot> = Vec::with_capacity(way.len());
        for (t, x) in way {
            let t = t - t0;
            let knot = match knots.last() {
                Some(k) => {
                    let (t, x) = (t.max(k.t), x.max(k.x));
                    let d = t - k.t;
                    let m = if d > 0.0 { (x - k.x) / d } else { 0.0 };
                    Knot {
                        t,
                        x,
                        i1: k.i1 + (k.x - x0) * d + m * d * d / 2.0,
                        i2: k.i2 + k.i1 * d + (k.x - x0) * d * d / 2.0 + m * d * d * d / 6.0,
                    }
                }
                None => Knot { t, x, i1: 0.0, i2: 0.0 },
            };
            knots.push(knot);
        }
        if knots.is_empty() {
            knots.push(Knot { t: 0.0, x: 0.0, i1: 0.0, i2: 0.0 });
        }
        Run { t0, knots, h: (window * 0.5).max(0.0) }
    }

    fn first(&self) -> Knot {
        self.knots[0]
    }

    fn last(&self) -> Knot {
        self.knots[self.knots.len() - 1]
    }

    /// The knot the way is at or past `t` seconds from the start (`t` not
    /// before it) and the way's pace from there, px a second.
    fn span(&self, t: f64) -> (Knot, f64) {
        let j = self.knots.partition_point(|k| k.t <= t).max(1) - 1;
        let k = self.knots[j];
        let m = match self.knots.get(j + 1) {
            Some(n) if n.t > k.t => (n.x - k.x) / (n.t - k.t),
            _ => 0.0,
        };
        (k, m)
    }

    /// Where the way itself (not averaged) is at `p` seconds.
    pub(super) fn way(&self, p: f64) -> f32 {
        let t = p - self.t0;
        if t <= 0.0 {
            return self.first().x as f32;
        }
        let (k, m) = self.span(t);
        (k.x + m * (t - k.t)) as f32
    }

    /// ∬ of the way (less its first place) up to `t` seconds from the start.
    fn area(&self, t: f64) -> f64 {
        if t <= 0.0 {
            return 0.0;
        }
        let (k, m) = self.span(t);
        let d = t - k.t;
        k.i2 + k.i1 * d + (k.x - self.first().x) * d * d / 2.0 + m * d * d * d / 6.0
    }

    /// Where the front is at `p` seconds: the way averaged over the window
    /// just gone, the middle of it weighing most (twice over half the
    /// window, so its speed has no step and no kink). Never ahead of the
    /// way, never back, and at rest at the way's ends.
    pub(super) fn at(&self, p: f64) -> f32 {
        let t = p - self.t0;
        let (first, last) = (self.first(), self.last());
        if t <= 0.0 {
            return first.x as f32;
        }
        let h = self.h;
        if h < 1e-4 {
            return self.way(p);
        }
        if t >= last.t + 2.0 * h {
            return last.x as f32;
        }
        let mean = (self.area(t) - 2.0 * self.area(t - h) + self.area(t - 2.0 * h)) / (h * h);
        (first.x + mean).clamp(first.x, last.x) as f32
    }

    /// When the way goes on from `x` px, seconds: when it comes there,
    /// unless it waits there (a pause), then when the wait is over.
    pub(super) fn left(&self, x: f32) -> f32 {
        let x = f64::from(x);
        let j = self.knots.partition_point(|k| k.x <= x);
        let t = if j == 0 {
            0.0
        } else if let Some(b) = self.knots.get(j) {
            let a = self.knots[j - 1];
            a.t + (b.t - a.t) * (x - a.x) / (b.x - a.x)
        } else {
            self.last().t
        };
        (self.t0 + t) as f32
    }

    /// When the way first reaches `x` px, seconds.
    #[cfg(test)]
    pub(super) fn reached(&self, x: f32) -> f32 {
        let x = f64::from(x);
        let j = self.knots.partition_point(|k| k.x < x);
        let t = if j == 0 {
            0.0
        } else if let Some(b) = self.knots.get(j) {
            let a = self.knots[j - 1];
            a.t + (b.t - a.t) * (x - a.x) / (b.x - a.x)
        } else {
            self.last().t
        };
        (self.t0 + t) as f32
    }

    /// When the run starts, and when its front has come to rest at its end.
    pub(super) fn times(&self) -> (f64, f64) {
        (self.t0, self.t0 + self.last().t + 2.0 * self.h)
    }

    /// Where the front rests before and after the run.
    #[cfg(test)]
    pub(super) fn ends(&self) -> (f32, f32) {
        (self.first().x as f32, self.last().x as f32)
    }
}

/// The straightest way through `gates` in order, from the first (a point)
/// to the last (a point), as its bends (seconds, px). From each bend it
/// goes straight for as long as one line passes every gate ahead within its
/// span; where the next gate needs it later (or sooner) than the line
/// allows, it bends at the gate that held the line back, at that gate's
/// latest (or earliest) moment.
fn taut(gates: &[Gate]) -> Vec<(f64, f64)> {
    let n = gates.len();
    let mut out = Vec::with_capacity(n.min(8));
    let Some(first) = gates.first() else { return out };
    let place = |k: usize| gates[k].x + k as f64 * APART;
    out.push((first.a, first.x));
    let (mut at, mut from) = (first.a, 0);
    while from + 1 < n {
        let x0 = place(from);
        // The paces (seconds a px) a straight line from here may have.
        let (mut soonest, mut latest) = (f64::NEG_INFINITY, f64::INFINITY);
        let (mut soonest_at, mut latest_at) = (from, from);
        let mut bend = None;
        for k in from + 1..n {
            let dx = place(k) - x0;
            let (lo, hi) = ((gates[k].a - at) / dx, (gates[k].b - at) / dx);
            if lo > latest {
                bend = Some((latest_at, gates[latest_at].b));
                break;
            }
            if hi < soonest {
                bend = Some((soonest_at, gates[soonest_at].a));
                break;
            }
            if lo > soonest {
                (soonest, soonest_at) = (lo, k);
            }
            if hi < latest {
                (latest, latest_at) = (hi, k);
            }
        }
        let (k, t) = bend.unwrap_or((n - 1, gates[n - 1].a));
        out.push((t, gates[k].x));
        (at, from) = (t, k);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(start: f32, end: f32, from: f32, to: f32) -> Piece {
        Piece { start, end, from, to, marks: 0..0 }
    }

    /// Glued pieces `(seconds, px)` from 1 s and 0 px on.
    fn glued(parts: &[(f32, f32)]) -> Vec<Piece> {
        let (mut t, mut x) = (1.0, 0.0);
        parts
            .iter()
            .map(|&(secs, px)| {
                let p = piece(t, t + secs, x, x + px);
                (t, x) = (p.end, p.to);
                p
            })
            .collect()
    }

    fn one(pieces: &[Piece], edge: f32, ease: Ease) -> Run {
        let (mut runs, of) = runs(pieces, &[], edge, ease);
        assert_eq!(runs.len(), 1, "one run");
        assert!(of.iter().all(|r| *r == 0));
        runs.remove(0)
    }

    /// A line of quick and held syllables, as a text timed by ear has them.
    fn ragged() -> Vec<Piece> {
        glued(&[(0.12, 30.0), (0.31, 26.0), (0.09, 34.0), (0.22, 28.0), (1.6, 30.0), (0.08, 36.0), (0.14, 22.0), (0.4, 40.0)])
    }

    #[test]
    fn the_exact_way_is_on_every_time() {
        let pieces = ragged();
        let run = one(&pieces, 0.0, Ease::EXACT);
        for p in &pieces {
            assert!((run.way(f64::from(p.start)) - p.from).abs() < 1e-3);
            assert!((run.way(f64::from(p.end)) - p.to).abs() < 1e-3);
            let mid = f64::from(p.start + p.end) * 0.5;
            assert!((run.way(mid) - (p.from + p.to) * 0.5).abs() < 1e-2, "even across a piece");
            assert!((run.reached(p.to) - p.end).abs() < 1e-4);
        }
        // With no window the front is the way.
        assert_eq!(run.at(1.3), run.way(1.3));
        assert_eq!(run.ends(), (0.0, pieces.last().unwrap().to));
    }

    /// The report: a long syllable dragged and a short one flew by, every
    /// piece at its own speed. The front never runs back and never ahead of
    /// the times, is never later than its tolerance, and its speed has no
    /// step: from one frame to the next it changes by a small share only.
    #[test]
    fn the_front_glides_without_a_step_in_its_speed() {
        let pieces = ragged();
        let end = f64::from(pieces.last().unwrap().end);
        for smooth in [0.0, 0.25, 0.5, 0.85, 1.0] {
            for edge in [0.0, 14.0] {
                let ease = Ease::of(smooth);
                let exact = one(&pieces, edge, Ease::EXACT);
                let run = one(&pieces, edge, ease);
                let behind = f64::from(ease.behind());
                let dt = 1.0 / 240.0;
                let (mut was, mut speed_was) = (run.at(0.5), 0.0f32);
                // The fastest piece is sung at 450 px a second.
                let top = pieces.iter().map(|p| (p.to - p.from) / (p.end - p.start)).fold(0.0f32, f32::max);
                let mut frame = 0;
                loop {
                    let p = 0.5 + f64::from(frame) * dt;
                    if p > end + 1.0 {
                        break;
                    }
                    frame += 1;
                    let x = run.at(p);
                    let case = format!("smooth {smooth}, edge {edge}, at {p:.3}");
                    assert!(x >= was - 1e-3, "{case}: runs back");
                    assert!(x <= exact.way(p) + 1e-2, "{case}: ahead of the voice, {x} against {}", exact.way(p));
                    assert!(x >= exact.way(p - behind) - 1e-2, "{case}: too late, {x} against {}", exact.way(p - behind));
                    let speed = (x - was) / dt as f32;
                    // The window is at least 50 ms: 12 frames or more to
                    // take up any change of pace, the largest one too.
                    assert!((speed - speed_was).abs() <= 0.2 * top, "{case}: the speed steps from {speed_was} to {speed}");
                    (was, speed_was) = (x, speed);
                }
                assert_eq!(was, exact.ends().1, "smooth {smooth}: the run ends lit whole");
                assert_eq!(run.at(0.9), exact.ends().0);
            }
        }
    }

    /// Each piece's end is reached on time or within the tolerance after
    /// it, and no piece is entered before it starts.
    #[test]
    fn every_piece_is_reached_within_the_tolerance() {
        let pieces = ragged();
        for smooth in [0.0, 0.5, 1.0] {
            let ease = Ease::of(smooth);
            let run = one(&pieces, 0.0, ease);
            for p in &pieces {
                let at = run.reached(p.to);
                assert!(at >= p.end - 1e-4 && at <= p.end + ease.late + 1e-4, "smooth {smooth}: {at} for {}", p.end);
                assert!(run.at(f64::from(p.start)) <= p.from + 1e-2, "smooth {smooth}: in before {}", p.start);
                assert!(run.at(f64::from(p.end + ease.late + ease.window)) >= p.to - 1e-2, "smooth {smooth}: not through by {}", p.end);
            }
        }
    }

    /// Neighbours' speeds come closer with «Плавность»: a short piece
    /// between calmer ones no longer flies by, yet a held one still takes
    /// its own long time.
    #[test]
    fn neighbours_keep_closer_speeds_and_a_held_piece_stays_long() {
        let pieces = ragged();
        let fastest = |run: &Run| {
            let dt = 1.0 / 240.0;
            (0..1200).map(|f| (run.at(0.9 + f64::from(f + 1) * dt) - run.at(0.9 + f64::from(f) * dt)) / dt as f32).fold(0.0f32, f32::max)
        };
        let exact = fastest(&one(&pieces, 0.0, Ease::EXACT));
        let (calm, calmest) = (fastest(&one(&pieces, 0.0, Ease::of(0.5))), fastest(&one(&pieces, 0.0, Ease::of(1.0))));
        assert!((exact - 36.0 / 0.08).abs() < 5.0, "{exact}");
        assert!(calm < 0.75 * exact && calmest < calm, "{exact} {calm} {calmest}");
        // The straight way: the four quick pieces before the held one are
        // sung at paces from 84 to 378 px a second; the front crosses them
        // at paces within a third of each other.
        let run = one(&pieces, 0.0, Ease::of(1.0));
        let paces: Vec<f32> = pieces[..4].iter().map(|p| (p.to - p.from) / (run.reached(p.to) - run.reached(p.from))).collect();
        let (low, high) = paces.iter().fold((f32::MAX, 0.0f32), |a, v| (a.0.min(*v), a.1.max(*v)));
        assert!(high / low < 1.9, "{paces:?}");
        // The held piece takes its 1.6 s, less the little the gates give.
        let held = &pieces[4];
        let took = run.reached(held.to) - run.reached(held.from);
        assert!(took > 1.3 && took < 1.75, "{took}");
        assert!(run.at(f64::from(held.start) + 0.8) > held.from + 5.0 && run.at(f64::from(held.start) + 0.8) < held.to - 5.0);
    }

    /// A function of the position alone: the same at any frame rate, and
    /// after a seek back or on.
    #[test]
    fn the_front_is_the_same_whatever_the_frames_and_after_a_seek() {
        let run = one(&ragged(), 12.0, Ease::of(0.7));
        let at: Vec<f32> = [2.0, 1.2, 3.4, 0.2, 2.0, 9.0, 1.2].iter().map(|p| run.at(*p)).collect();
        assert_eq!((at[0], at[1]), (at[4], at[6]));
        assert!(at[1] < at[0] && at[0] < at[2]);
        assert_eq!((at[3], at[5]), run.ends());
        // 30 frames a second or 240: no frame moves it far, and both pass
        // the same places.
        for fps in [30.0, 240.0] {
            let mut was = run.at(0.9);
            for f in 0..(4.0 * fps) as i32 {
                let x = run.at(0.9 + f64::from(f) / fps);
                assert!(x >= was - 1e-3 && x - was < 520.0 / fps as f32 + 0.5, "{fps}: {was} → {x}");
                was = x;
            }
        }
    }

    /// A pause splits the runs; a short one is bridged, the front slowing
    /// through the space between the words and never stopping dead.
    #[test]
    fn a_pause_parts_the_runs_and_a_short_one_is_bridged() {
        let pieces = [piece(1.0, 1.4, 0.0, 40.0), piece(1.6, 2.0, 48.0, 90.0), piece(2.9, 3.3, 98.0, 140.0), piece(3.3, 3.3, 140.0, 150.0)];
        let (runs, of) = runs(&pieces, &[], 10.0, Ease::of(0.5));
        assert_eq!(of, [0, 0, 1, 1]);
        let (a, b) = (&runs[0], &runs[1]);
        // The first run rests with its edge past its last letter, the next
        // with its edge before its first.
        assert_eq!(a.ends(), (-5.0, 95.0));
        assert_eq!(b.ends(), (93.0, 155.0));
        assert!(a.times().1 < 2.9 && a.at(2.6) == 95.0 && b.at(2.9) == 93.0);
        // Through the bridged pause: slower, still moving.
        let step = |p: f64| a.at(p + 0.02) - a.at(p);
        assert!(step(1.5) > 0.05 && step(1.5) < step(1.2) && step(1.5) < step(1.8));
        // A piece with no length still lights, over the edge's own time.
        let zero = [piece(7.0, 7.0, 0.0, 10.0)];
        let run = one(&zero, 10.0, Ease::of(0.5));
        assert!(run.at(6.99) == -5.0 && run.at(7.1) > 0.0 && run.at(7.1) < 15.0 && run.at(8.0) == 15.0);
    }

    /// Guessed places inside a piece (its letters): kept at no «Плавность»,
    /// given way as it grows, so a quick piece is crossed evenly and only a
    /// held one lingers on its vowel.
    #[test]
    fn guessed_places_give_way_to_an_even_pace() {
        // Three letters of 10 px: the consonants quick, the vowel long.
        let marks = [(10.0, 1.1), (20.0, 1.5)];
        let pieces = [Piece { start: 1.0, end: 1.6, from: 0.0, to: 30.0, marks: 0..2 }];
        let exact = runs(&pieces, &marks, 0.0, Ease::EXACT).0.remove(0);
        assert!((exact.reached(10.0) - 1.1).abs() < 1e-4 && (exact.reached(20.0) - 1.5).abs() < 1e-4);
        let calm = runs(&pieces, &marks, 0.0, Ease::of(1.0)).0.remove(0);
        assert!((calm.reached(10.0) - 1.2).abs() < 1e-3, "an even pace fits the gates: {}", calm.reached(10.0));
        // Held four times as long, the vowel still takes the most: 1.6 s
        // of the 2.4 by the letters, never less than its even third.
        let marks = [(10.0, 1.4), (20.0, 3.0)];
        let pieces = [Piece { start: 1.0, end: 3.4, from: 0.0, to: 30.0, marks: 0..2 }];
        let vowel = |smooth: f32| {
            let run = runs(&pieces, &marks, 0.0, Ease::of(smooth)).0.remove(0);
            assert!((run.reached(0.0) - 1.0).abs() < 1e-4 && (run.reached(30.0) - 3.4).abs() < 1e-4);
            run.reached(20.0) - run.reached(10.0)
        };
        assert!((vowel(0.0) - 1.6).abs() < 1e-3);
        assert!(vowel(0.5) < 1.5 && vowel(0.5) > vowel(1.0) && vowel(1.0) > 0.8, "{} {}", vowel(0.5), vowel(1.0));
    }

    /// Times that slip back (a piece timed before the one written ahead of
    /// it) never turn the front round.
    #[test]
    fn a_timing_slip_never_turns_the_front_back() {
        let pieces = [piece(1.0, 1.5, 0.0, 30.0), piece(0.8, 1.2, 30.0, 60.0), piece(2.0, 1.9, 60.0, 90.0)];
        for ease in [Ease::EXACT, Ease::of(0.5)] {
            let (runs, _) = runs(&pieces, &[], 8.0, ease);
            for run in &runs {
                let mut was = f32::MIN;
                for ms in 500..3500 {
                    let x = run.at(f64::from(ms) / 1000.0);
                    assert!(x.is_finite() && x >= was - 1e-3, "{ms}");
                    was = x;
                }
            }
        }
    }

    #[test]
    fn the_taut_way_bends_only_where_a_gate_makes_it() {
        let g = |x: f64, a: f64, b: f64| Gate { x, a, b };
        // One line fits every gate: no bend.
        let straight = taut(&[g(0.0, 0.0, 0.0), g(10.0, 0.9, 1.2), g(20.0, 1.8, 2.3), g(30.0, 3.0, 3.0)]);
        assert_eq!(straight, [(0.0, 0.0), (3.0, 30.0)]);
        // The middle gate closes before the line gets there: a bend at its
        // latest moment.
        let bent = taut(&[g(0.0, 0.0, 0.0), g(10.0, 0.2, 0.5), g(30.0, 3.0, 3.0)]);
        assert_eq!(bent, [(0.0, 0.0), (0.5, 10.0), (3.0, 30.0)]);
        // It opens after the line has passed: a bend at its earliest.
        let waits = taut(&[g(0.0, 0.0, 0.0), g(10.0, 2.0, 2.4), g(30.0, 3.0, 3.0)]);
        assert_eq!(waits, [(0.0, 0.0), (2.0, 10.0), (3.0, 30.0)]);
        // Two gates at one place (a pause inside a word): it stays there.
        let pause = taut(&[g(0.0, 0.0, 0.0), g(10.0, 1.0, 1.0), g(10.0, 2.0, 2.0), g(20.0, 3.0, 3.0)]);
        assert_eq!(pause, [(0.0, 0.0), (1.0, 10.0), (2.0, 10.0), (3.0, 20.0)]);
    }
}
