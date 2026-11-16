//! Touch gestures without hover: a value on a spring a finger holds and
//! lets go with its speed, the decision where a fling lands, and the
//! gestures of a row (tap, long press, swipe to the right) and of the left
//! edge (back). Rows sense clicks only, so a vertical drag that starts on
//! one still scrolls the list under it.

use egui::{Context, Id, Pos2, Rect, Response, Sense, Ui, Vec2};
use nikgob_motion::Spring;

/// A press held this long without moving is a long press.
pub const LONG_PRESS: f64 = 0.38;
/// A row swiped this far to the right does its swipe action.
pub const SWIPE_TRIGGER: f32 = 72.0;
/// Movement under this is still a tap (or a long press).
const SLOP: f32 = 8.0;
/// A swipe starts once the finger went this far sideways (and mostly so).
const SWIPE_START: f32 = 14.0;
/// A fling faster than this (dp/s) decides by its direction alone.
pub const FLING: f32 = 650.0;
/// The left edge a back swipe starts from.
pub const EDGE: f32 = 24.0;
/// A back swipe this long goes back.
pub const BACK_TRIGGER: f32 = 96.0;

/// A value on a spring that a finger can hold and let go of with its
/// speed: the spring starts from that speed, so a fling carries on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sprung {
    from: f32,
    v0: f32,
    target: f32,
    start: f64,
    spring: Spring,
}

impl Sprung {
    /// At rest on `value`.
    pub const fn at(value: f32) -> Self {
        Self { from: value, v0: 0.0, target: value, start: 0.0, spring: Spring::new(1.0, 400.0) }
    }

    fn solved(&self, now: f64) -> (f32, f32) {
        let (dx, v) = self.spring.solve(self.from - self.target, self.v0, (now - self.start) as f32);
        (self.target + dx, v)
    }

    pub fn value(&self, now: f64) -> f32 {
        if self.from == self.target && self.v0 == 0.0 {
            return self.target;
        }
        self.solved(now).0
    }

    /// Units per second.
    pub fn velocity(&self, now: f64) -> f32 {
        if self.from == self.target && self.v0 == 0.0 {
            return 0.0;
        }
        self.solved(now).1
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    /// Close enough to its target and slow enough to stop drawing frames.
    pub fn settled(&self, now: f64) -> bool {
        let (x, v) = self.solved(now);
        (self.from == self.target && self.v0 == 0.0) || ((x - self.target).abs() < 0.0005 && v.abs() < 0.05)
    }

    /// Held by a finger at `value`: no motion of its own.
    pub fn hold(&mut self, value: f32, now: f64) {
        *self = Self { from: value, v0: 0.0, target: value, start: now, spring: self.spring };
    }

    /// Let go at `value` moving at `velocity` (units/s), toward `target`.
    pub fn release(&mut self, value: f32, velocity: f32, target: f32, spring: Spring, now: f64) {
        *self = Self { from: value, v0: velocity, target, start: now, spring };
    }

    /// Toward `target` from wherever it is, keeping its speed.
    pub fn go(&mut self, target: f32, spring: Spring, now: f64) {
        if target == self.target && spring == self.spring {
            return;
        }
        let (x, v) = if self.settled(now) { (self.target, 0.0) } else { self.solved(now) };
        self.release(x, v, target, spring, now);
    }

    /// There at once.
    pub fn snap(&mut self, value: f32) {
        self.from = value;
        self.target = value;
        self.v0 = 0.0;
    }
}

/// Whether something dragged to `fraction` (0 closed … 1 open) of a span
/// `span` long, let go at `velocity` (dp/s, toward open positive), opens:
/// a fling decides by its direction, else the nearer end.
pub fn settles_open(fraction: f32, velocity: f32, span: f32) -> bool {
    let _ = span;
    if velocity > FLING {
        true
    } else if velocity < -FLING {
        false
    } else {
        fraction > 0.5
    }
}

/// A drag past where it acts goes on with resistance.
pub fn rubber(x: f32, limit: f32) -> f32 {
    if x <= limit { x } else { limit + (x - limit) * 0.35 }
}

/// What happened on a row this frame.
pub struct RowGesture {
    pub response: Response,
    pub tap: bool,
    /// Held without moving (or a right click on the PC): the row's menu.
    pub long: bool,
    /// How far the finger has the row to the right now (0: not swiping).
    pub offset: f32,
    /// Let go past [`SWIPE_TRIGGER`].
    pub swiped: bool,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    origin: Pos2,
    t0: f64,
    long: bool,
    swipe: bool,
    moved: bool,
}

/// The gestures of a row over `rect`; `swipe`: it has a swipe action.
pub fn row(ui: &Ui, rect: Rect, id: Id, swipe: bool) -> RowGesture {
    let response = ui.interact(rect, id, Sense::click());
    let ctx = ui.ctx().clone();
    let (pressed, down, origin, pos, now) = ctx.input(|i| {
        (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.interact_pos(), i.time)
    });
    let key = id.with("press");
    let mut st: Option<Press> = ctx.data(|d| d.get_temp(key));
    if pressed
        && let Some(o) = origin
        && ui.clip_rect().intersect(rect).contains(o)
    {
        st = Some(Press { origin: o, t0: now, long: false, swipe: false, moved: false });
    }
    let mut g = RowGesture { tap: false, long: response.secondary_clicked(), offset: 0.0, swiped: false, response };
    match st {
        Some(mut p) if down => {
            let d = pos.map_or(Vec2::ZERO, |q| q - p.origin);
            if !p.swipe && !p.long && !p.moved {
                if swipe && d.x > SWIPE_START && d.x.abs() > d.y.abs() * 1.4 {
                    p.swipe = true;
                } else if d.length() > SLOP {
                    p.moved = true;
                } else if now - p.t0 >= LONG_PRESS {
                    p.long = true;
                    g.long = true;
                }
            }
            if p.swipe {
                g.offset = rubber(d.x.max(0.0), SWIPE_TRIGGER);
            }
            // Frames only while a finger is down on it and it is undecided
            // or moving.
            if !p.moved {
                ctx.request_repaint();
            }
            ctx.data_mut(|d| d.insert_temp(key, p));
        }
        Some(p) => {
            let d = pos.map_or(Vec2::ZERO, |q| q - p.origin);
            if p.swipe {
                g.swiped = d.x >= SWIPE_TRIGGER;
            } else if !p.long {
                g.tap = g.response.clicked();
            }
            ctx.data_mut(|d| d.remove::<Press>(key));
        }
        None => g.tap = g.response.clicked(),
    }
    g
}

/// A value a finger sets while `live` is some, and that springs back to 0
/// on `spring` once it lets go.
pub fn follow(ctx: &Context, id: Id, live: Option<f32>, spring: Spring) -> f32 {
    match live {
        Some(v) => {
            nikgob_md3e::components::anim::f32(ctx, id, v, spring);
            nikgob_md3e::components::anim::snap_f32(ctx, id, v);
            v
        }
        None => nikgob_md3e::components::anim::f32(ctx, id, 0.0, spring),
    }
}

/// A back swipe from the left edge of `area`: how far it is now, and
/// whether it was let go far or fast enough to go back.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeSwipe {
    pub offset: f32,
    pub back: bool,
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    origin: Pos2,
    swipe: bool,
    dead: bool,
}

/// The back swipe over `area` (only while there is somewhere to go back).
pub fn edge_back(ctx: &Context, area: Rect, id: Id, enabled: bool) -> EdgeSwipe {
    let key = id.with("edge");
    if !enabled {
        ctx.data_mut(|d| d.remove::<Edge>(key));
        return EdgeSwipe::default();
    }
    let (pressed, down, origin, pos, vx) = ctx.input(|i| {
        (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.press_origin(), i.pointer.interact_pos(), i.pointer.velocity().x)
    });
    let mut st: Option<Edge> = ctx.data(|d| d.get_temp(key));
    if pressed
        && let Some(o) = origin
        && area.contains(o)
        && o.x < area.left() + EDGE
    {
        st = Some(Edge { origin: o, swipe: false, dead: false });
    }
    let Some(mut e) = st else { return EdgeSwipe::default() };
    let d = pos.map_or(Vec2::ZERO, |q| q - e.origin);
    if down {
        if !e.swipe && !e.dead {
            if d.x > SWIPE_START && d.x > d.y.abs() * 1.4 {
                e.swipe = true;
            } else if d.length() > SWIPE_START {
                e.dead = true;
            }
        }
        ctx.data_mut(|data| data.insert_temp(key, e));
        if e.swipe {
            ctx.request_repaint();
            return EdgeSwipe { offset: d.x.max(0.0), back: false };
        }
        return EdgeSwipe::default();
    }
    ctx.data_mut(|data| data.remove::<Edge>(key));
    EdgeSwipe { offset: 0.0, back: e.swipe && (d.x > BACK_TRIGGER || vx > FLING) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fling_decides_by_its_direction_else_the_nearer_end() {
        assert!(settles_open(0.2, FLING + 1.0, 800.0));
        assert!(!settles_open(0.9, -FLING - 1.0, 800.0));
        assert!(settles_open(0.6, 0.0, 800.0));
        assert!(!settles_open(0.4, 100.0, 800.0));
    }

    #[test]
    fn a_spring_let_go_moving_keeps_its_speed_and_lands_on_its_target() {
        let spring = Spring::new(0.8, 380.0);
        let mut s = Sprung::at(0.0);
        s.hold(0.3, 1.0);
        assert_eq!(s.value(1.5), 0.3);
        s.release(0.3, 2.0, 1.0, spring, 1.0);
        assert!((s.velocity(1.0) - 2.0).abs() < 1e-4);
        assert!((s.value(1.0) - 0.3).abs() < 1e-6);
        assert!(s.value(1.02) > 0.3);
        assert!((s.value(6.0) - 1.0).abs() < 1e-3);
        assert!(s.settled(6.0));
        // Turned around mid-way, from where it is.
        let mid = s.value(1.05);
        s.go(0.0, spring, 1.05);
        assert!((s.value(1.05) - mid).abs() < 1e-5);
        assert_eq!(s.target(), 0.0);
    }

    #[test]
    fn a_drag_past_its_limit_goes_on_slower() {
        assert_eq!(rubber(40.0, 72.0), 40.0);
        assert!(rubber(172.0, 72.0) < 172.0);
        assert!(rubber(172.0, 72.0) > 72.0);
    }
}
