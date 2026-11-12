//! «Фигуры»: a core of Material shapes that morphs on the beat and swells
//! with the bass, ringed by twelve satellites, one per part of the
//! spectrum, each growing and taking its accent colour with its part.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use egui::{Painter, Rect, pos2, vec2};
use nikgob_shapes::{MaterialShape, Morph, paint::fill_shape};

use super::signal::{REST, Signal};
use crate::{ColorScheme, lerp_color};

const RING: usize = 12;

/// The shapes the core walks through, one morph per neighbouring pair.
const CORE: [MaterialShape; 6] = [
    MaterialShape::Cookie12Sided,
    MaterialShape::SoftBurst,
    MaterialShape::Flower,
    MaterialShape::Clover8Leaf,
    MaterialShape::Sunny,
    MaterialShape::Puffy,
];

const SATELLITES: [MaterialShape; RING] = [
    MaterialShape::Circle,
    MaterialShape::Pill,
    MaterialShape::Cookie4Sided,
    MaterialShape::Pentagon,
    MaterialShape::Oval,
    MaterialShape::Cookie6Sided,
    MaterialShape::Diamond,
    MaterialShape::Clover4Leaf,
    MaterialShape::Triangle,
    MaterialShape::Cookie7Sided,
    MaterialShape::Gem,
    MaterialShape::SoftBoom,
];

fn morphs() -> &'static [Morph] {
    static M: OnceLock<Vec<Morph>> = OnceLock::new();
    M.get_or_init(|| {
        (0..CORE.len())
            .map(|i| Morph::new(CORE[i].polygon(), CORE[(i + 1) % CORE.len()].polygon()))
            .collect()
    })
}

pub struct Shapes {
    levels: [f32; RING],
    bass: f32,
    pair: usize,
    progress: f32,
    angle: f32,
}

impl Default for Shapes {
    fn default() -> Self {
        Self { levels: [0.0; RING], bass: 0.0, pair: 0, progress: 1.0, angle: 0.0 }
    }
}

impl Shapes {
    pub fn update(&mut self, sig: &Signal, reduced: bool) {
        let dt = sig.dt;
        let mut target = [0.0f32; RING];
        for (g, slot) in target.iter_mut().enumerate() {
            *slot = if reduced { 0.3 } else { Signal::at(&sig.bands, (g as f32 + 0.5) / RING as f32) };
        }
        let bass_target = if reduced { 0.3 } else { (sig.calm * 0.5 + sig.kick * 0.6).clamp(0.0, 1.0) };
        // Fast attack, slow release (slow all round for reduced motion).
        let follow = |cur: f32, to: f32| {
            let k = if reduced {
                1.5
            } else if to > cur {
                30.0
            } else {
                6.0
            };
            cur + (to - cur) * (1.0 - (-dt * k).exp())
        };
        for (l, to) in self.levels.iter_mut().zip(target) {
            *l = follow(*l, to);
        }
        self.bass = follow(self.bass, bass_target);
        if reduced {
            self.progress = 1.0;
            self.angle = 0.0;
            return;
        }
        if sig.beat && self.progress > 0.6 {
            self.pair = (self.pair + 1) % CORE.len();
            self.progress = 0.0;
        }
        self.progress += (1.0 - self.progress) * (1.0 - (-dt * 7.0).exp());
        self.angle += dt * sig.drive * 0.25 * (1.0 + self.bass);
    }

    #[cfg(test)]
    pub fn angle(&self) -> f32 {
        self.angle
    }

    /// Settled: nothing would change in the next frame of a pause.
    pub fn still(&self) -> bool {
        self.progress > 0.999 && self.bass < REST && self.levels.iter().all(|l| *l < REST)
    }

    pub fn paint(&self, painter: &Painter, rect: Rect, s: &ColorScheme) {
        let c = rect.center();
        let r = rect.width().min(rect.height()) * 0.5;

        // Satellites: one per spectrum group.
        let ring_r = r * 0.68;
        let accents = [
            (s.primary_container, s.primary),
            (s.secondary_container, s.secondary),
            (s.tertiary_container, s.tertiary),
        ];
        for (i, shape) in SATELLITES.iter().enumerate() {
            let level = self.levels[i];
            let a = i as f32 / RING as f32 * TAU + self.angle * 0.4;
            let pos = pos2(c.x + ring_r * a.cos(), c.y + ring_r * a.sin());
            let size = r * (0.12 + 0.2 * level);
            let (base, accent) = accents[i % 3];
            let color = lerp_color(base, accent, level);
            let spin = self.angle * (1.0 + (i % 3) as f32) * if i % 2 == 0 { 1.0 } else { -1.0 };
            fill_shape(painter, shape.polygon().cubics(), Rect::from_center_size(pos, vec2(size, size)), spin, color);
        }

        // Core: morphs on the beat, swells with the bass.
        let core = r * (0.62 + 0.2 * self.bass);
        let cubics = morphs()[self.pair].as_cubics(self.progress.clamp(0.0, 1.0));
        fill_shape(painter, &cubics, Rect::from_center_size(c, vec2(core, core)), self.angle, s.primary);
        let inner = core * (0.5 + 0.1 * self.bass);
        fill_shape(
            painter,
            CORE[(self.pair + 3) % CORE.len()].polygon().cubics(),
            Rect::from_center_size(c, vec2(inner, inner)),
            -self.angle * 1.5,
            s.primary_container,
        );
    }
}
