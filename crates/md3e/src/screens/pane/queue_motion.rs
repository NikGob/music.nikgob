//! How the queue changes when shuffle turns on or off under the same
//! track: the playing track keeps its place on screen and the list changes
//! around it. Rows still in sight slide to their new place, the ones that
//! go fade out fast, the new ones fade in a moment later, the farther from
//! the playing track the later. Where the list cannot scroll far enough to
//! keep the playing track still (its top or its end), the rest of the way
//! is a glide; a playing track out of sight is reached by a short glide.
//!
//! Every value is a function of the time since the change, so the list
//! keeps nothing per row and a settled list costs nothing.

use nikgob_motion::Spring;

use crate::Motion;

/// The «Ранее» label, kept with the rows.
pub(super) const PREV_LABEL: u64 = u64::MAX;
/// What follows the queue (the radio looking for more, autoplay).
pub(super) const TAIL: u64 = u64::MAX - 1;
/// The gap under the history, above «Сейчас играет».
pub(super) const PREV_GAP: f32 = 12.0;

/// New rows wait this long, so the ones they replace are nearly gone.
const LEAD: f32 = 0.040;
/// And this much more per row away from the playing track…
const STAGGER: f32 = 0.016;
/// …up to this many rows.
const STAGGER_ROWS: usize = 6;
/// New rows come this far toward the playing track.
const DRIFT: f32 = 12.0;
/// The glide to a playing track out of sight covers at most this.
const GLIDE_CAP: f32 = 280.0;
/// The last row's delay with its springs settled.
const RUNS: f32 = 0.6;
/// How much of the playing row must show for it to stay where it is.
const IN_SIGHT: f32 = 24.0;

/// A row (or label) as it was painted: screen y from the top of the list's
/// view, or from the playing track's block once a change keeps it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Seen {
    pub entry: u64,
    pub y: f32,
    pub played: bool,
}

/// What a row does during a change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Role {
    Still,
    /// In sight before and after: slides from `from` above its place.
    Stays { from: f32 },
    /// New in sight: fades in after `delay`, coming from `drift`.
    Arrives { delay: f32, drift: f32 },
}

impl Role {
    /// (Vertical offset, opacity) `t` seconds into the change.
    pub fn at(self, t: f32, m: &Motion) -> (f32, f32) {
        match self {
            Self::Still => (0.0, 1.0),
            Self::Stays { from } => (m.default_spatial.solve(from, 0.0, t).0, 1.0),
            Self::Arrives { delay, drift } => {
                let t = t - delay;
                if t <= 0.0 {
                    return (drift, 0.0);
                }
                let alpha = 1.0 - m.default_effects.solve(1.0, 0.0, t).0;
                (m.default_spatial.solve(drift, 0.0, t).0, alpha.clamp(0.0, 1.0))
            }
        }
    }
}

/// How long the `rank`-th row from the playing track (1 = next to it)
/// waits before it fades in.
pub(super) fn stagger(rank: usize) -> f32 {
    LEAD + STAGGER * rank.saturating_sub(1).min(STAGGER_ROWS - 1) as f32
}

/// Where the playing track's block starts in the list: under the history
/// (its label `head`, `prev` rows of `step`, the gap) when there is one.
pub(super) fn current_top(prev: usize, head: f32, step: f32) -> f32 {
    if prev == 0 { 0.0 } else { head + prev as f32 * step + PREV_GAP }
}

/// Whether enough of the playing row (`head` under its block's top at
/// `anchor`, `row_h` tall) shows in a view `view_h` tall to keep it there.
pub(super) fn in_sight(anchor: f32, head: f32, row_h: f32, view_h: f32) -> bool {
    let top = anchor + head;
    top + row_h >= IN_SIGHT && top <= view_h - IN_SIGHT
}

/// The glide's start when the playing track was out of sight: the way from
/// the old place toward it, capped.
pub(super) fn glide_seed(current_top: f32, scroll: f32) -> f32 {
    (current_top - scroll).clamp(-GLIDE_CAP, GLIDE_CAP)
}

/// What the first frame leaves to glide: the list showed at `wanted` and
/// then clamped it to `clamped`; the playing track was meant at `desired`
/// and painted at `painted`.
pub(super) fn residual(wanted: f32, clamped: f32, desired: f32, painted: f32) -> f32 {
    clamped - wanted + desired - painted
}

/// A shuffle turned on or off, from its first frame until it settles.
#[derive(Clone, Debug)]
pub(super) struct Change {
    pub started: f64,
    /// The playing track was in sight and keeps its place; otherwise the
    /// list goes to it.
    pub anchored: bool,
    /// What was in sight: y from the playing track's block when anchored,
    /// from the view's top otherwise.
    before: Vec<Seen>,
    /// Of those, what is still in sight after the change.
    stays: Vec<u64>,
    /// And what is not: painted where it was, fading out.
    pub ghosts: Vec<Seen>,
    /// Where each of `ghosts` is in the queue now (history or not, index),
    /// to paint it; `None`: no longer in what the queue shows.
    pub ghost_at: Vec<Option<(bool, usize)>>,
    /// Still the first frame (what stays is being found out).
    pub first: bool,
    /// The scroll offset asked for, and where the playing track was meant.
    pub wanted: f32,
    pub desired: f32,
    /// The whole list's offset at the start, gliding to nothing.
    pub shift: f32,
    spring: Spring,
}

impl Change {
    /// What the row `entry` does: `rel` is its place from the playing
    /// track's block, `rank` how many rows away it is, `below` whether it
    /// comes after it, `in_sight` whether its place shows this frame.
    pub fn role(&mut self, entry: u64, rel: f32, rank: usize, below: bool, in_sight: bool) -> Role {
        // A row that goes to the other side of the playing track would
        // slide across it: it is swapped instead.
        if self.anchored
            && let Some(b) = self.before.iter().find(|b| b.entry == entry)
            && (b.y < 0.0) == (rel < 0.0)
        {
            if self.first && in_sight {
                self.stays.push(entry);
                return Role::Stays { from: b.y - rel };
            }
            if !self.first && self.stays.contains(&entry) {
                return Role::Stays { from: b.y - rel };
            }
        }
        let drift = if below { DRIFT } else { -DRIFT };
        Role::Arrives { delay: stagger(rank), drift }
    }

    /// After the first frame's rows: whatever was in sight and stays out
    /// of it fades out where it was.
    pub fn end_first(&mut self) {
        let stays = &self.stays;
        self.ghosts = self.before.iter().filter(|b| !stays.contains(&b.entry)).copied().collect();
    }

    /// The whole list's offset `t` seconds in.
    pub fn shift_at(&self, t: f32) -> f32 {
        self.spring.solve(self.shift, 0.0, t).0
    }

    /// How much a row that goes still shows `t` seconds in.
    pub fn ghost_alpha(t: f32, m: &Motion) -> f32 {
        m.fast_effects.solve(1.0, 0.0, t).0.clamp(0.0, 1.0)
    }

    /// Everything has settled `t` seconds in.
    pub fn done(&self, t: f32) -> bool {
        !self.first && t >= RUNS && self.shift_at(t).abs() < 0.25
    }
}

/// The queue's memory between frames.
#[derive(Clone, Debug, Default)]
pub(super) struct QueueMotion {
    /// Drawn on frame `frame` with this track and shuffle.
    pub known: bool,
    pub frame: u64,
    pub current: Option<u64>,
    pub shuffle: bool,
    /// The scroll offset and the view's height last frame.
    pub scroll: f32,
    pub view_h: f32,
    /// From the playing track's block to its row (the label above it).
    pub head: f32,
    /// Where the playing track's block was painted, from the view's top.
    pub anchor: Option<f32>,
    /// The rows painted in sight last frame (filled again every frame).
    pub seen: Vec<Seen>,
    pub change: Option<Change>,
}

impl QueueMotion {
    /// Shuffle turned on or off under the same track, whose block now
    /// starts at `current_top` in the list: starts the change and returns
    /// the scroll offset to show the list at. Reduced motion only keeps the
    /// playing track in place (or goes to it), with nothing moving.
    pub fn begin(&mut self, now: f64, current_top: f32, row_h: f32, m: &Motion, reduced: bool) -> f32 {
        let anchor = self.anchor.filter(|&a| in_sight(a, self.head, row_h, self.view_h));
        let (wanted, desired, seed) = match anchor {
            Some(d) => (current_top - d, d, 0.0),
            None => (current_top, 0.0, glide_seed(current_top, self.scroll)),
        };
        if reduced {
            self.change = None;
            return wanted;
        }
        let mut before = std::mem::take(&mut self.seen);
        if let Some(d) = anchor {
            for b in &mut before {
                b.y -= d;
            }
        }
        let spring = if anchor.is_some() { m.default_spatial } else { m.slow_spatial };
        self.change = Some(Change {
            started: now,
            anchored: anchor.is_some(),
            before,
            stays: Vec::new(),
            ghosts: Vec::new(),
            ghost_at: Vec::new(),
            first: true,
            wanted,
            desired,
            shift: seed,
            spring: spring.without_bounce(),
        });
        wanted
    }

    /// After the first frame was shown: the list clamped the offset to
    /// `clamped` and painted the playing track's block at `self.anchor`.
    pub fn end_first_frame(&mut self, clamped: f32) {
        let painted = self.anchor;
        if let Some(c) = self.change.as_mut().filter(|c| c.first) {
            c.shift += match (c.anchored, painted) {
                (true, Some(a)) => residual(c.wanted, clamped, c.desired, a),
                _ => clamped - c.wanted,
            };
            c.first = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: f32 = 80.0;

    fn seen(entry: u64, y: f32) -> Seen {
        Seen { entry, y, played: false }
    }

    fn motion() -> QueueMotion {
        QueueMotion { known: true, head: 28.0, view_h: 600.0, ..Default::default() }
    }

    #[test]
    fn the_playing_block_sits_under_the_history() {
        assert_eq!(current_top(0, 28.0, STEP), 0.0);
        assert_eq!(current_top(3, 28.0, STEP), 28.0 + 240.0 + PREV_GAP);
    }

    #[test]
    fn the_playing_row_is_in_sight_while_enough_of_it_shows() {
        // The row is 72 tall, 28 under the block's top, the view 600.
        assert!(in_sight(0.0, 28.0, 72.0, 600.0));
        assert!(in_sight(-70.0, 28.0, 72.0, 600.0), "30 of it shows at the top");
        assert!(!in_sight(-80.0, 28.0, 72.0, 600.0));
        assert!(in_sight(540.0, 28.0, 72.0, 600.0), "32 of it shows at the bottom");
        assert!(!in_sight(560.0, 28.0, 72.0, 600.0));
    }

    #[test]
    fn new_rows_start_later_the_farther_they_are() {
        assert!((stagger(1) - 0.040).abs() < 1e-6);
        assert!((stagger(2) - 0.056).abs() < 1e-6);
        assert!((stagger(6) - 0.120).abs() < 1e-6);
        assert_eq!(stagger(6), stagger(40), "capped");
        assert_eq!(stagger(0), stagger(1));
    }

    #[test]
    fn a_playing_track_in_sight_keeps_its_place() {
        let mut m = motion();
        m.anchor = Some(200.0);
        m.seen = vec![seen(7, 120.0), seen(8, 300.0)];
        // Shuffle off brings 10 played rows back above it.
        let top = current_top(10, m.head, STEP);
        let offset = m.begin(1.0, top, 72.0, &Motion::default(), false);
        assert_eq!(top - offset, 200.0, "its block shows where it was");
        let c = m.change.as_ref().unwrap();
        assert!(c.anchored);
        assert_eq!(c.before, vec![seen(7, -80.0), seen(8, 100.0)], "rows kept from the playing track");
        assert_eq!(c.shift_at(0.0), 0.0);
        assert!(m.seen.is_empty());
    }

    #[test]
    fn what_the_list_cannot_scroll_glides() {
        let mut m = motion();
        // Three played rows showed above it; shuffle on removes them.
        m.anchor = Some(268.0);
        let offset = m.begin(1.0, 0.0, 72.0, &Motion::default(), false);
        assert_eq!(offset, -268.0);
        // The list clamps to 0 and painted the block where it was meant.
        m.end_first_frame(0.0);
        let c = m.change.as_ref().unwrap();
        assert!(!c.first);
        assert_eq!(c.shift_at(0.0), 268.0, "it starts where it was");
        let half = c.shift_at(0.1);
        assert!(half > 0.0 && half < 268.0);
        assert!(c.shift_at(0.6).abs() < 1.0, "and arrives without overshoot");
        assert!((0..60).all(|i| c.shift_at(i as f32 * 0.01) >= 0.0));
    }

    #[test]
    fn a_wrong_guess_of_its_place_is_glided_away() {
        assert_eq!(residual(100.0, 100.0, 200.0, 200.0), 0.0);
        assert_eq!(residual(100.0, 60.0, 200.0, 200.0), -40.0, "clamped at the end");
        assert_eq!(residual(100.0, 100.0, 200.0, 197.0), 3.0, "painted a little higher");
    }

    #[test]
    fn a_playing_track_out_of_sight_is_reached_by_a_short_glide() {
        let mut m = motion();
        m.anchor = Some(-2000.0);
        m.scroll = 2400.0;
        let offset = m.begin(1.0, 0.0, 72.0, &Motion::default(), false);
        assert_eq!(offset, 0.0, "the list goes to it");
        let c = m.change.as_ref().unwrap();
        assert!(!c.anchored);
        assert_eq!(c.shift_at(0.0), -GLIDE_CAP, "coming from above, capped");
        assert_eq!(glide_seed(500.0, 400.0), 100.0);
    }

    #[test]
    fn reduced_motion_keeps_the_place_and_moves_nothing() {
        let mut m = motion();
        m.anchor = Some(200.0);
        m.seen = vec![seen(7, 120.0)];
        let offset = m.begin(1.0, 500.0, 72.0, &Motion::reduced(), true);
        assert_eq!(offset, 300.0);
        assert!(m.change.is_none());
    }

    #[test]
    fn rows_in_sight_before_and_after_slide_the_others_swap() {
        let mut m = motion();
        m.anchor = Some(0.0);
        m.seen = vec![seen(PREV_LABEL, -400.0), seen(1, 100.0), seen(2, 180.0), seen(3, 260.0)];
        m.begin(1.0, 0.0, 72.0, &Motion::default(), false);
        let c = m.change.as_mut().unwrap();
        // First frame: 2 lands at 340 in sight, 3 lands out of sight, 9 is new.
        assert_eq!(c.role(2, 340.0, 4, true, true), Role::Stays { from: -160.0 });
        assert_eq!(c.role(3, 4000.0, 50, true, false), Role::Arrives { delay: stagger(50), drift: DRIFT });
        assert_eq!(c.role(9, 100.0, 1, true, true), Role::Arrives { delay: stagger(1), drift: DRIFT });
        assert_eq!(c.role(5, -100.0, 1, false, true), Role::Arrives { delay: stagger(1), drift: -DRIFT });
        c.end_first();
        let gone: Vec<u64> = c.ghosts.iter().map(|g| g.entry).collect();
        assert_eq!(gone, vec![PREV_LABEL, 1, 3]);
        c.first = false;
        // Later frames remember who stays, even out of sight.
        assert_eq!(c.role(2, 340.0, 4, true, false), Role::Stays { from: -160.0 });
        assert!(matches!(c.role(3, 500.0, 5, true, true), Role::Arrives { .. }), "it faded out, it fades in");
    }

    #[test]
    fn a_row_never_slides_across_the_playing_track() {
        let mut m = motion();
        m.anchor = Some(300.0);
        // A played row above it lands among the next ones (shuffle on).
        m.seen = vec![seen(4, 220.0), seen(5, 400.0)];
        m.begin(1.0, 0.0, 72.0, &Motion::default(), false);
        let c = m.change.as_mut().unwrap();
        assert!(matches!(c.role(4, 180.0, 2, true, true), Role::Arrives { .. }));
        assert_eq!(c.role(5, 260.0, 3, true, true), Role::Stays { from: -160.0 });
        c.end_first();
        assert_eq!(c.ghosts, vec![seen(4, -80.0)]);
    }

    #[test]
    fn nothing_stays_when_the_list_goes_to_the_playing_track() {
        let mut m = motion();
        m.anchor = None;
        m.seen = vec![seen(1, 100.0)];
        m.begin(1.0, 0.0, 72.0, &Motion::default(), false);
        let c = m.change.as_mut().unwrap();
        assert!(matches!(c.role(1, 100.0, 1, true, true), Role::Arrives { .. }));
        c.end_first();
        assert_eq!(c.ghosts, vec![seen(1, 100.0)], "painted where it was in the view");
    }

    #[test]
    fn rows_move_and_fade_on_their_springs() {
        let m = Motion::default();
        assert_eq!(Role::Still.at(0.3, &m), (0.0, 1.0));
        let (dy, a) = Role::Stays { from: -160.0 }.at(0.0, &m);
        assert_eq!((dy, a), (-160.0, 1.0));
        let (dy, _) = Role::Stays { from: -160.0 }.at(RUNS, &m);
        assert!(dy.abs() < 1.0);
        let arrives = Role::Arrives { delay: 0.05, drift: DRIFT };
        assert_eq!(arrives.at(0.03, &m), (DRIFT, 0.0), "waits for its turn");
        let (dy, a) = arrives.at(0.12, &m);
        assert!(a > 0.3 && a < 1.0 && dy < DRIFT);
        let (dy, a) = arrives.at(RUNS, &m);
        assert!(a > 0.99 && dy.abs() < 0.5);
        assert!(Change::ghost_alpha(0.0, &m) > 0.99);
        assert!(Change::ghost_alpha(0.1, &m) < 0.05, "what goes is gone fast");
    }

    #[test]
    fn a_change_ends_once_everything_settled() {
        let mut m = motion();
        m.anchor = Some(268.0);
        m.begin(1.0, 0.0, 72.0, &Motion::default(), false);
        assert!(!m.change.as_ref().unwrap().done(5.0), "not before its first frame");
        m.end_first_frame(0.0);
        let c = m.change.as_ref().unwrap();
        assert!(!c.done(0.2));
        assert!(c.done(1.0));
    }
}
