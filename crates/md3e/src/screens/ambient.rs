//! The ambient background of Now Playing and the visualizer: a few large,
//! soft colour clouds in the scheme's accents drifting slowly over the whole
//! window and swelling with the bass. No hard edges, nothing anchored to the
//! cover.

use std::f32::consts::TAU;

use egui::{Color32, ColorImage, Context, Id, Mesh, Painter, Rect, TextureHandle, TextureOptions, pos2, vec2};

use crate::Theme;

/// Side of the cloud texture: one texel per few pixels of the largest cloud,
/// which bilinear filtering blends into a continuous ramp.
const TEXTURE: usize = 512;

/// One cloud: where it wanders (fractions of the rect), how fast, how big.
struct Cloud {
    home: (f32, f32),
    reach: (f32, f32),
    speed: (f32, f32),
    phase: (f32, f32),
    radius: f32,
}

const CLOUDS: [Cloud; 4] = [
    Cloud { home: (0.25, 0.30), reach: (0.18, 0.14), speed: (0.031, 0.023), phase: (0.0, 1.3), radius: 0.62 },
    Cloud { home: (0.78, 0.68), reach: (0.16, 0.18), speed: (0.019, 0.029), phase: (2.1, 0.4), radius: 0.70 },
    Cloud { home: (0.70, 0.18), reach: (0.20, 0.10), speed: (0.027, 0.017), phase: (4.0, 2.7), radius: 0.48 },
    Cloud { home: (0.20, 0.85), reach: (0.14, 0.12), speed: (0.015, 0.033), phase: (1.1, 5.2), radius: 0.55 },
];

/// Paints the clouds into `rect`. `bass` is the smoothed level (0..1);
/// `moving` advances the drift (playing and motion allowed). The drift phase
/// is accumulated, so pausing freezes the picture and reopening the window
/// continues where it was. `strength` is the user's multiplier
/// (`Prefs::ambient_strength`).
pub fn paint(ctx: &Context, painter: &Painter, rect: Rect, bass: f32, moving: bool, strength: f32) {
    let theme = Theme::get(ctx);
    let s = theme.scheme;
    let phase = drift(ctx, moving && !theme.reduced_motion);
    let colors = [s.primary, s.tertiary, s.secondary, s.primary_container];
    let strength = (if theme.dark { 0.20 } else { 0.16 } * (1.0 + 0.6 * bass) * strength.clamp(0.25, 2.0)).min(1.0);
    let size = rect.width().max(rect.height());
    let painter = painter.with_clip_rect(painter.clip_rect().intersect(rect));
    let mut mesh = Mesh::with_texture(texture(ctx).id());
    for (cloud, color) in CLOUDS.iter().zip(colors) {
        let x = cloud.home.0 + cloud.reach.0 * (phase * cloud.speed.0 * TAU + cloud.phase.0).sin();
        let y = cloud.home.1 + cloud.reach.1 * (phase * cloud.speed.1 * TAU + cloud.phase.1).cos();
        let center = pos2(rect.left() + rect.width() * x, rect.top() + rect.height() * y);
        let radius = size * cloud.radius * (1.0 + 0.10 * bass);
        mesh.add_rect_with_uv(
            Rect::from_center_size(center, vec2(radius, radius) * 2.0),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            color.gamma_multiply(strength),
        );
    }
    painter.add(egui::Shape::mesh(mesh));
    if moving {
        ctx.request_repaint();
    }
}

/// Seconds of drift so far; only runs while `moving`.
fn drift(ctx: &Context, moving: bool) -> f32 {
    let now = ctx.input(|i| i.time);
    let id = Id::new("md3e-ambient-drift");
    ctx.data_mut(|d| {
        let (last, phase) = d.get_temp::<(f64, f32)>(id).unwrap_or((now, 0.0));
        // Long gaps (window hidden) are not replayed as a jump.
        let dt = (now - last).clamp(0.0, 0.1) as f32;
        let phase = if moving { phase + dt } else { phase };
        d.insert_temp(id, (now, phase));
        phase
    })
}

/// How a cloud fades from its centre (`k` 0) to its edge (`k` 1): a smooth
/// curve with no slope at either end.
fn falloff(k: f32) -> f32 {
    let q = (1.0 - k * k).max(0.0);
    q * q
}

/// The cloud's shape as a texture: white, its opacity [`falloff`] from the
/// centre. Drawn tinted, one quad per cloud. (Rings of vertices joined by
/// straight ramps left a visible edge at every ring, and the edges moved
/// outwards as the clouds swelled with the bass.)
fn texture(ctx: &Context) -> TextureHandle {
    let id = Id::new("md3e-ambient-cloud");
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let half = TEXTURE as f32 * 0.5;
    let pixels = (0..TEXTURE * TEXTURE)
        .map(|i| {
            let (x, y) = ((i % TEXTURE) as f32 + 0.5 - half, (i / TEXTURE) as f32 + 0.5 - half);
            let a = falloff((x * x + y * y).sqrt() / half);
            let v = (a * 255.0).round() as u8;
            Color32::from_rgba_premultiplied(v, v, v, v)
        })
        .collect();
    let image = ColorImage::new([TEXTURE, TEXTURE], pixels);
    let t = ctx.load_texture("md3e-ambient-cloud", image, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}
