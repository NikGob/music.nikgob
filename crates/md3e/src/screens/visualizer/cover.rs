//! «Обложка»: the cover in a Material shape. It swells on each beat with
//! a bouncy spring, turns a full circle every sixteen beats and takes the
//! next shape every four, morphing there. The spectrum stands around its
//! outline, lows at the top, mirrored left and right, with each
//! direction's peak falling back slowly.

use std::f32::consts::TAU;

use egui::{Mesh, Pos2, Rect, Shape, Stroke, Ui, epaint::PathShape, vec2};
use nikgob_motion::tokens;
use nikgob_shapes::MaterialShape;

use super::{
    signal::{REST, Signal},
    spring_step,
};
use crate::ColorScheme;
use crate::screens::now_playing::Outline;

/// The shapes the cover walks through.
const WALK: [MaterialShape; 8] = [
    MaterialShape::Cookie12Sided,
    MaterialShape::Sunny,
    MaterialShape::Flower,
    MaterialShape::Cookie9Sided,
    MaterialShape::SoftBurst,
    MaterialShape::Clover8Leaf,
    MaterialShape::Cookie7Sided,
    MaterialShape::Puffy,
];

/// Directions of the spectrum around the outline (every 2°).
const DIRS: usize = 180;

pub struct Cover {
    from: usize,
    to: usize,
    /// Morph `from` → `to`, 0..1 with the spring's overshoot.
    t: f32,
    t_v: f32,
    scale: f32,
    scale_v: f32,
    angle: f32,
    /// The band shown per direction and its slowly falling peak.
    shown: [f32; DIRS],
    peak: [f32; DIRS],
    peak_fall: [f32; DIRS],
}

impl Default for Cover {
    fn default() -> Self {
        Self {
            from: 0,
            to: 0,
            t: 1.0,
            t_v: 0.0,
            scale: 1.0,
            scale_v: 0.0,
            angle: 0.0,
            shown: [0.0; DIRS],
            peak: [0.0; DIRS],
            peak_fall: [0.0; DIRS],
        }
    }
}

impl Cover {
    pub fn update(&mut self, sig: &Signal, reduced: bool) {
        let dt = sig.dt;
        if reduced {
            self.t = 1.0;
            self.scale = 1.0;
            for (i, v) in self.shown.iter_mut().enumerate() {
                *v = 0.15 + 0.1 * (i as f32 / DIRS as f32 * TAU * 3.0).cos().abs();
            }
            self.peak = self.shown;
            return;
        }
        if sig.beat && sig.beats.is_multiple_of(4) && self.t > 0.8 {
            self.from = self.to;
            self.to = (self.to + 1) % WALK.len();
            self.t = 0.0;
            self.t_v = 0.0;
        }
        spring_step(tokens::EXPRESSIVE_DEFAULT_SPATIAL, &mut self.t, &mut self.t_v, 1.0, dt);
        spring_step(tokens::EXPRESSIVE_FAST_SPATIAL, &mut self.scale, &mut self.scale_v, 1.0 + 0.07 * sig.kick, dt);
        self.angle = (self.angle + dt * sig.drive * TAU / (sig.period * 16.0)).rem_euclid(TAU);
        let mut raw = [0.0f32; DIRS];
        for (i, r) in raw.iter_mut().enumerate() {
            *r = Signal::around(&sig.bands, i as f32 / DIRS as f32 * TAU).powf(1.4);
        }
        for i in 0..DIRS {
            // Neighbours blended in, so the edge is a line, not teeth.
            let at = |k: isize| raw[(i as isize + k).rem_euclid(DIRS as isize) as usize];
            let level = (at(-2) + 2.0 * at(-1) + 3.0 * at(0) + 2.0 * at(1) + at(2)) / 9.0;
            self.shown[i] = level;
            if level >= self.peak[i] {
                self.peak[i] = level;
                self.peak_fall[i] = 0.0;
            } else {
                self.peak_fall[i] += 0.9 * dt;
                self.peak[i] = (self.peak[i] - self.peak_fall[i] * dt).max(level);
            }
        }
    }

    #[cfg(test)]
    pub fn angle(&self) -> f32 {
        self.angle
    }

    /// Settled: nothing would change in the next frame of a pause.
    pub fn still(&self) -> bool {
        (self.t - 1.0).abs() < 1e-3
            && self.t_v.abs() < 1e-3
            && (self.scale - 1.0).abs() < 1e-3
            && self.scale_v.abs() < 1e-3
            && self.shown.iter().chain(&self.peak).all(|v| *v < REST)
    }

    pub fn paint(&self, ui: &Ui, rect: Rect, s: &ColorScheme, uri: Option<&str>) {
        let c = rect.center();
        let side = rect.width().min(rect.height());
        let size = side * 0.46 * self.scale;
        let place = Rect::from_center_size(c, vec2(size, size));
        let outline = if self.from == self.to {
            Outline::swap(WALK[self.to], WALK[self.to], 1.0, place, self.angle)
        } else {
            Outline::swap(WALK[self.from], WALK[self.to], self.t, place, self.angle)
        };
        let radii = outline.radii();
        let gap = side * 0.018;
        let amp = side * 0.16;
        let base = 2.0;
        let dir = |i: usize| {
            let th = i as f32 / DIRS as f32 * TAU;
            vec2(th.sin(), -th.cos())
        };
        let r_at = |i: usize| radii[(i * radii.len() / DIRS) % radii.len()];

        // The band: a strip from just off the outline out to the level.
        let mut mesh = Mesh::default();
        let mut edge: Vec<Pos2> = Vec::with_capacity(DIRS);
        for i in 0..DIRS {
            let d = dir(i);
            let inner = r_at(i) + gap;
            let outer = inner + base + amp * self.shown[i];
            mesh.colored_vertex(c + d * inner, s.primary_container);
            mesh.colored_vertex(c + d * outer, s.primary_container);
            edge.push(c + d * outer);
        }
        for i in 0..DIRS as u32 {
            let j = (i + 1) % DIRS as u32;
            mesh.add_triangle(2 * i, 2 * j, 2 * j + 1);
            mesh.add_triangle(2 * i, 2 * j + 1, 2 * i + 1);
        }
        let painter = ui.painter();
        painter.add(Shape::mesh(mesh));
        // Soft edges for the strip (the mesh has no anti-aliasing).
        let inner_edge: Vec<Pos2> = (0..DIRS).map(|i| c + dir(i) * (r_at(i) + gap)).collect();
        painter.add(PathShape::closed_line(inner_edge, Stroke::new(1.0, s.primary_container)));
        painter.add(PathShape::closed_line(edge, Stroke::new(2.5, s.primary)));
        // The peaks: a ring of dots (as around the cover in Now Playing)
        // that the band pushes out and that settles back onto it.
        let dot = (side * 0.0045).max(2.0);
        for i in (0..DIRS).step_by(3) {
            let r = r_at(i) + gap + base + amp * self.peak[i] + 3.0 * dot;
            painter.circle_filled(c + dir(i) * r, dot, s.tertiary);
        }

        outline.paint(ui, uri);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_changes_every_fourth_beat_and_settles() {
        let mut cover = Cover::default();
        let mut sig = Signal::default();
        (sig.dt, sig.period) = (1.0 / 60.0, 0.5);
        let mut changes = 0;
        for frame in 0..(60 * 8) {
            sig.beat = frame % 30 == 0;
            if sig.beat {
                sig.beats += 1;
            }
            let before = cover.to;
            cover.update(&sig, false);
            changes += usize::from(cover.to != before);
        }
        // 16 beats, a change on each fourth.
        assert_eq!(changes, 4);
        for _ in 0..120 {
            sig.beat = false;
            cover.update(&sig, false);
        }
        assert!((cover.t - 1.0).abs() < 0.01);
        assert!((cover.scale - 1.0).abs() < 0.01);
    }

    #[test]
    fn peaks_hold_above_the_level_and_fall_back() {
        let mut cover = Cover::default();
        let mut sig = Signal::default();
        (sig.dt, sig.period, sig.bands) = (1.0 / 60.0, 0.5, vec![0.9; 16]);
        cover.update(&sig, false);
        sig.bands.fill(0.1);
        cover.update(&sig, false);
        assert!(cover.peak[0] > 0.8 && cover.shown[0] < 0.2);
        for _ in 0..180 {
            cover.update(&sig, false);
        }
        assert!((cover.peak[0] - cover.shown[0]).abs() < 1e-3);
    }
}
