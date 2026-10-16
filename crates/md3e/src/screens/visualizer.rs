//! The fullscreen visualizer (md3e-player.md §3.5): shapes from the Material
//! library pulse with the spectrum and the core morphs on the beat. Colours
//! come from the scheme. With reduced motion everything holds still.

use std::f32::consts::TAU;
use std::sync::OnceLock;

use egui::{Area, Context, Id, Order, Rect, Sense, UiBuilder, pos2, vec2};
use nikgob_shapes::{MaterialShape, Morph, paint::fill_shape};
use nikgob_view::{Ambient, Status, ViewState};

use super::common;
use crate::{
    Icon, Theme, TypeRole, lerp_color,
    components::{IconButton, Size, Style, anim},
};

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

/// Animation state of the visualizer.
#[derive(Default)]
pub struct Viz {
    last: f64,
    opened: f64,
    levels: [f32; RING],
    bass: f32,
    slow: f32,
    beat_at: f64,
    pair: usize,
    progress: f32,
    angle: f32,
}

impl Viz {
    /// Advances the levels and the beat from the spectrum (or an idle pulse
    /// when there is no audio data).
    fn update(&mut self, ctx: &Context, bands: &[f32], playing: bool, reduced: bool) {
        let now = anim::now(ctx);
        if now - self.last > 0.5 {
            self.opened = now;
            self.progress = 1.0;
        }
        let dt = ((now - self.last) as f32).clamp(0.0, 0.1);
        self.last = now;
        let t = now as f32;

        let energy: f32 = bands.iter().sum();
        let real = playing && bands.len() >= RING && energy > 0.02;
        let mut target = [0.0f32; RING];
        let bass_target;
        if reduced {
            target.fill(0.3);
            bass_target = 0.3;
        } else if real {
            for (g, slot) in target.iter_mut().enumerate() {
                let (a, b) = (g * bands.len() / RING, (g + 1) * bands.len() / RING);
                let avg = bands[a..b.max(a + 1)].iter().sum::<f32>() / (b.max(a + 1) - a) as f32;
                *slot = (avg * 1.5).clamp(0.0, 1.0);
            }
            let n = (bands.len() / 10).max(1);
            bass_target = (bands[..n].iter().sum::<f32>() / n as f32 * 1.4).clamp(0.0, 1.0);
        } else {
            let play = if playing { 1.0 } else { 0.0 };
            let pulse = (t * TAU * 1.9).sin().max(0.0).powi(6);
            for (i, slot) in target.iter_mut().enumerate() {
                let wave = 0.18 + 0.12 * (t * 1.3 + i as f32 * 0.7).sin();
                *slot = (wave * (0.4 + 0.6 * play)
                    + play * 0.3 * pulse * (1.0 - i as f32 / RING as f32))
                    .clamp(0.0, 1.0);
            }
            bass_target = 0.15 * (1.0 - play) + play * (0.25 + 0.6 * pulse);
        }

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
        self.slow += (self.bass - self.slow) * (1.0 - (-dt * 1.5).exp());

        if reduced {
            self.progress = 1.0;
            self.angle = 0.0;
            return;
        }
        if self.bass - self.slow > 0.12 && self.bass > 0.2 && now - self.beat_at > 0.35 {
            self.beat_at = now;
            if self.progress > 0.6 {
                self.pair = (self.pair + 1) % CORE.len();
                self.progress = 0.0;
            }
        }
        self.progress += (1.0 - self.progress) * (1.0 - (-dt * 7.0).exp());
        self.angle += dt * 0.25 * (1.0 + self.bass);
    }

    /// Draws the visualizer over the whole window. Returns true when closed.
    pub fn show(&mut self, ctx: &Context, state: &ViewState, bands: &[f32]) -> bool {
        let theme = Theme::get(ctx);
        let s = theme.scheme;
        let reduced = theme.reduced_motion;
        let playing = state.player.status == Status::Playing;
        self.update(ctx, bands, playing, reduced);
        let fade = ((anim::now(ctx) - self.opened) as f32 / 0.3).clamp(0.0, 1.0);
        let screen = ctx.content_rect();
        let mut closed = false;

        Area::new(Id::new("md3e-visualizer"))
            .fade_in(false)
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                ui.multiply_opacity(fade);
                // Swallows input meant for what is underneath.
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                ui.painter().rect_filled(rect, 0.0, s.surface);
                let current = state.player.current.as_ref();
                // Soft clouds behind everything (not tied to the cover).
                if state.prefs.ambient != Ambient::Off {
                    let playing = state.player.status == nikgob_view::Status::Playing;
                    super::ambient::paint(ctx, ui.painter(), rect, self.bass, playing, state.prefs.ambient_strength);
                }
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
                    fill_shape(
                        ui.painter(),
                        shape.polygon().cubics(),
                        Rect::from_center_size(pos, vec2(size, size)),
                        spin,
                        color,
                    );
                }

                // Core: morphs on the beat, swells with the bass.
                let core = r * (0.62 + 0.2 * self.bass);
                let cubics = morphs()[self.pair].as_cubics(self.progress.clamp(0.0, 1.0));
                fill_shape(
                    ui.painter(),
                    &cubics,
                    Rect::from_center_size(c, vec2(core, core)),
                    self.angle,
                    s.primary,
                );
                let inner = core * (0.5 + 0.1 * self.bass);
                fill_shape(
                    ui.painter(),
                    CORE[(self.pair + 3) % CORE.len()].polygon().cubics(),
                    Rect::from_center_size(c, vec2(inner, inner)),
                    -self.angle * 1.5,
                    s.primary_container,
                );

                // Corner: small cover and title.
                let cover = Rect::from_min_size(rect.min + vec2(24.0, 24.0), vec2(72.0, 72.0));
                common::cover(ui, current.and_then(|t| t.cover.as_ref()), cover, 20.0);
                if let Some(t) = current {
                    let text = Rect::from_min_size(
                        pos2(cover.right() + 16.0, cover.top() + 14.0),
                        vec2((rect.width() * 0.4).clamp(120.0, 360.0), 48.0),
                    );
                    let mut tui = ui.new_child(UiBuilder::new().max_rect(text));
                    tui.spacing_mut().item_spacing.y = 0.0;
                    common::label(&mut tui, &t.title, TypeRole::TitleMedium, s.on_surface);
                    common::label(&mut tui, &t.artist, TypeRole::BodyMedium, s.on_surface_variant);
                }
                let close = Rect::from_min_size(pos2(rect.right() - 24.0 - 40.0, rect.top() + 24.0), vec2(40.0, 40.0));
                let mut cui = ui.new_child(UiBuilder::new().max_rect(close));
                if cui
                    .add(
                        IconButton::new(Icon::Close)
                            .size(Size::S)
                            .style(Style::Tonal)
                            .tooltip("Закрыть (Esc, F12)")
                            .id_salt("viz-close"),
                    )
                    .clicked()
                {
                    closed = true;
                }
            });
        if reduced {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else {
            ctx.request_repaint();
        }
        closed
    }
}
