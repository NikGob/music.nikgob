//! «Спокойно», for listening in the background: one shape in the middle
//! breathing with the track's loudness over about a second and turning
//! slowly, and on the beat (at most one every ~1.5 s, or every few
//! seconds in music without one) its outline sets off outwards like a ring
//! on water and thins out. Louder moments leave bolder rings.

use std::f32::consts::TAU;

use egui::{Painter, Rect, Stroke, vec2};
use nikgob_motion::tokens;
use nikgob_shapes::{MaterialShape, paint::stroke_shape};

use super::{
    signal::{REST, Signal},
    spring_step,
};
use crate::ColorScheme;

const SHAPE: MaterialShape = MaterialShape::Cookie9Sided;
const RINGS: usize = 8;
/// Seconds a ring travels.
const LIFE: f32 = 7.0;
/// Fewest seconds between two rings.
const GAP: f32 = 1.5;
/// A ring every this many seconds when no beat comes.
const IDLE: f32 = 4.0;

#[derive(Clone, Copy, Default)]
struct Ring {
    age: f32,
    strength: f32,
    angle: f32,
    alive: bool,
}

#[derive(Default)]
pub struct Calm {
    rings: [Ring; RINGS],
    since: f32,
    angle: f32,
    breath: f32,
    breath_v: f32,
}

impl Calm {
    pub fn update(&mut self, sig: &Signal, live: bool, reduced: bool) {
        let dt = sig.dt;
        if reduced {
            self.breath = 0.3;
            self.rings = [Ring::default(); RINGS];
            return;
        }
        spring_step(tokens::EXPRESSIVE_SLOW_SPATIAL, &mut self.breath, &mut self.breath_v, sig.calm, dt);
        // A pause stops the rings where they are, as it stops the music.
        let run = dt * sig.drive;
        self.angle = (self.angle + run * TAU / 60.0 * (0.5 + sig.calm)).rem_euclid(TAU);
        self.since += run;
        for r in &mut self.rings {
            if r.alive {
                r.age += run;
                r.alive = r.age < LIFE;
                r.angle += run * TAU / 90.0;
            }
        }
        let due = (sig.beat && self.since >= GAP) || (live && sig.calm > 0.08 && self.since >= IDLE);
        if due {
            self.since = 0.0;
            // The oldest place (or a free one).
            let slot = (0..RINGS).max_by(|&a, &b| {
                let age = |r: &Ring| if r.alive { r.age } else { f32::MAX };
                age(&self.rings[a]).total_cmp(&age(&self.rings[b]))
            });
            if let Some(i) = slot {
                self.rings[i] = Ring { age: 0.0, strength: (0.35 + 0.65 * sig.level).clamp(0.0, 1.0), angle: self.angle, alive: true };
            }
        }
    }

    #[cfg(test)]
    pub fn angle(&self) -> f32 {
        self.angle
    }

    /// Settled: nothing would change in the next frame of a pause (with
    /// the signal still: no drive, no loudness).
    pub fn still(&self) -> bool {
        self.breath.abs() < REST && self.breath_v.abs() < REST
    }

    pub fn paint(&self, painter: &Painter, rect: Rect, s: &ColorScheme) {
        let c = rect.center();
        let side = rect.width().min(rect.height());
        let core = side * 0.3 * (0.9 + 0.18 * self.breath.clamp(-0.2, 1.3));
        let reach = rect.size().length() * 0.5;
        let cubics = SHAPE.polygon().cubics();
        for r in self.rings.iter().filter(|r| r.alive) {
            let k = r.age / LIFE;
            // Quick off the shape, slowing as it spreads.
            let out = 1.0 - (1.0 - k).powf(1.6);
            let size = core + (reach * 2.0 - core) * out * 0.75;
            let fade = (1.0 - k).powf(1.6) * r.strength;
            // See-through, so it reads the same over the ambient clouds.
            let color = s.primary.gamma_multiply(fade);
            let width = 1.0 + 2.5 * r.strength * (1.0 - k);
            stroke_shape(painter, cubics, Rect::from_center_size(c, vec2(size, size)), r.angle, Stroke::new(width, color));
        }
        nikgob_shapes::paint::fill_shape(painter, cubics, Rect::from_center_size(c, vec2(core, core)), self.angle, s.primary_container);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(beat: bool, calm: f32) -> Signal {
        let mut s = Signal::default();
        (s.dt, s.beat, s.calm, s.level, s.drive) = (0.1, beat, calm, calm, 1.0);
        s
    }

    #[test]
    fn rings_follow_beats_no_closer_than_the_gap() {
        let mut calm = Calm::default();
        // A beat every 0.5 s for 6 s: a ring every 1.5 s at most.
        for frame in 0..60 {
            calm.update(&sig(frame % 5 == 0, 0.5), true, false);
        }
        let alive = calm.rings.iter().filter(|r| r.alive).count();
        assert!((3..=4).contains(&alive), "{alive}");
    }

    #[test]
    fn music_without_a_beat_still_sends_rings_and_silence_none() {
        let mut calm = Calm::default();
        for _ in 0..100 {
            calm.update(&sig(false, 0.5), true, false);
        }
        assert!(calm.rings.iter().any(|r| r.alive));
        let mut calm = Calm::default();
        for _ in 0..100 {
            calm.update(&sig(false, 0.0), false, false);
        }
        assert!(calm.rings.iter().all(|r| !r.alive));
    }
}
