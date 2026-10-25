//! The ambient background of Now Playing and the visualizer: a few large,
//! soft colour clouds in the scheme's accents drifting slowly over the whole
//! window and swelling with the bass. No hard edges, nothing anchored to the
//! cover.

use std::f32::consts::TAU;
use std::sync::atomic::{AtomicU8, Ordering};

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
    let clouds: [(egui::Pos2, f32, Color32); 4] = std::array::from_fn(|i| {
        let cloud = &CLOUDS[i];
        let x = cloud.home.0 + cloud.reach.0 * (phase * cloud.speed.0 * TAU + cloud.phase.0).sin();
        let y = cloud.home.1 + cloud.reach.1 * (phase * cloud.speed.1 * TAU + cloud.phase.1).cos();
        let center = pos2(rect.left() + rect.width() * x, rect.top() + rect.height() * y);
        let radius = size * cloud.radius * (1.0 + 0.10 * bass);
        (center, radius, colors[i].gamma_multiply(strength))
    });
    if GPU.load(Ordering::Relaxed) != GPU_NO {
        gpu::paint(&painter, rect, &clouds);
    } else {
        let mut mesh = Mesh::with_texture(texture(ctx).id());
        for (center, radius, color) in clouds {
            mesh.add_rect_with_uv(
                Rect::from_center_size(center, vec2(radius, radius) * 2.0),
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                color,
            );
        }
        painter.add(egui::Shape::mesh(mesh));
    }
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

/// Whether the clouds can be drawn by [`gpu`]: unknown until it first runs.
static GPU: AtomicU8 = AtomicU8::new(GPU_UNKNOWN);
const GPU_UNKNOWN: u8 = 0;
const GPU_YES: u8 = 1;
const GPU_NO: u8 = 2;

/// The clouds in one shader pass. Drawn as textured quads, every cloud was
/// blended into the 8-bit window on its own, each rounded there with the
/// same dither pattern as the others, so the rounding of each cloud showed
/// as faint steps along its own contours (wide on a dark, quiet cover).
/// Here all four are summed in floating point, exactly as the quads blend,
/// [`falloff`] is evaluated per pixel (no texels), and the sum is dithered
/// once at screen resolution before the window rounds it.
#[allow(unsafe_code)]
mod gpu {
    use std::sync::{Arc, Mutex, Weak};

    use egui::{Color32, Painter, Pos2, Rect};
    use egui_glow::glow::{self, HasContext as _};

    use super::{GPU, GPU_NO, GPU_YES};

    struct Program {
        program: glow::Program,
        cloud: Option<glow::UniformLocation>,
        color: Option<glow::UniformLocation>,
        viewport: Option<glow::UniformLocation>,
    }

    /// The program of each GL context it was built in (`None`: it cannot be).
    static PROGRAMS: Mutex<Vec<(Weak<glow::Context>, Option<Arc<Program>>)>> = Mutex::new(Vec::new());

    const VERTEX: &str = "
        void main() {
            // Two triangles over the callback's viewport.
            float x = float(gl_VertexID % 2) * 2.0 - 1.0;
            float y = float(gl_VertexID / 2) * 2.0 - 1.0;
            gl_Position = vec4(x, y, 0.0, 1.0);
        }
    ";

    const FRAGMENT: &str = "
        #ifdef GL_ES
            precision highp float;
        #endif
        // Per cloud: centre (fractions of the viewport, y up) and radius
        // (a fraction of its width); colour premultiplied, in gamma space.
        uniform vec4 u_cloud[4];
        uniform vec4 u_color[4];
        // The viewport in window pixels: left, bottom, width, height.
        uniform vec4 u_viewport;
        out vec4 f_color;

        float interleaved_gradient_noise(vec2 n) {
            float f = 0.06711056 * n.x + 0.00583715 * n.y;
            return fract(52.9829189 * fract(f));
        }

        void main() {
            vec2 px = gl_FragCoord.xy - u_viewport.xy;
            vec4 sum = vec4(0.0);
            for (int i = 0; i < 4; i++) {
                vec2 d = px - u_cloud[i].xy * u_viewport.zw;
                float k = length(d) / (u_cloud[i].z * u_viewport.z);
                float q = max(1.0 - k * k, 0.0);
                vec4 c = u_color[i] * (q * q);
                // Over the clouds before it, as the quads blended.
                sum = c + sum * (1.0 - c.a);
            }
            // Dithered once, at the window's pixels, into its 8 bits.
            float noise = (interleaved_gradient_noise(gl_FragCoord.xy) - 0.5) * 0.95;
            sum.rgb += noise / 255.0;
            f_color = sum;
        }
    ";

    fn compile(gl: &glow::Context) -> Option<Program> {
        let version = egui_glow::ShaderVersion::get(gl);
        // gl_VertexID and `out` need GLSL 1.40 / ES 3.00.
        if !version.is_new_shader_interface() {
            return None;
        }
        let head = version.version_declaration();
        unsafe {
            let program = gl.create_program().ok()?;
            let mut shaders = Vec::new();
            for (kind, src) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                let shader = gl.create_shader(kind).ok()?;
                gl.shader_source(shader, &format!("{head}{src}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    log::warn!("ambient shader: {}", gl.get_shader_info_log(shader));
                    gl.delete_shader(shader);
                    gl.delete_program(program);
                    return None;
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }
            gl.link_program(program);
            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }
            if !gl.get_program_link_status(program) {
                log::warn!("ambient shader: {}", gl.get_program_info_log(program));
                gl.delete_program(program);
                return None;
            }
            Some(Program {
                program,
                cloud: gl.get_uniform_location(program, "u_cloud"),
                color: gl.get_uniform_location(program, "u_color"),
                viewport: gl.get_uniform_location(program, "u_viewport"),
            })
        }
    }

    fn program(gl: &Arc<glow::Context>) -> Option<Arc<Program>> {
        let mut programs = PROGRAMS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        programs.retain(|(c, _)| c.strong_count() > 0);
        if let Some((_, p)) = programs.iter().find(|(c, _)| c.upgrade().is_some_and(|c| Arc::ptr_eq(&c, gl))) {
            return p.clone();
        }
        let p = compile(gl).map(Arc::new);
        programs.push((Arc::downgrade(gl), p.clone()));
        p
    }

    /// Adds the clouds (centre, radius, premultiplied colour) over `rect`.
    pub(super) fn paint(painter: &Painter, rect: Rect, clouds: &[(Pos2, f32, Color32); 4]) {
        let opacity = painter.opacity();
        let mut cloud = [0.0f32; 16];
        let mut color = [0.0f32; 16];
        for (i, (center, radius, c)) in clouds.iter().enumerate() {
            cloud[i * 4] = (center.x - rect.left()) / rect.width();
            cloud[i * 4 + 1] = (rect.bottom() - center.y) / rect.height();
            cloud[i * 4 + 2] = radius / rect.width();
            for (j, v) in c.to_array().into_iter().enumerate() {
                color[i * 4 + j] = f32::from(v) / 255.0 * opacity;
            }
        }
        let callback = egui_glow::CallbackFn::new(move |info, painter| {
            let gl = painter.gl();
            let Some(p) = program(gl) else {
                GPU.store(GPU_NO, std::sync::atomic::Ordering::Relaxed);
                return;
            };
            GPU.store(GPU_YES, std::sync::atomic::Ordering::Relaxed);
            let vp = info.viewport_in_pixels();
            unsafe {
                gl.use_program(Some(p.program));
                gl.uniform_4_f32_slice(p.cloud.as_ref(), &cloud);
                gl.uniform_4_f32_slice(p.color.as_ref(), &color);
                gl.uniform_4_f32(
                    p.viewport.as_ref(),
                    vp.left_px as f32,
                    vp.from_bottom_px as f32,
                    vp.width_px as f32,
                    vp.height_px as f32,
                );
                gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            }
        });
        painter.add(egui::PaintCallback { rect, callback: Arc::new(callback) });
    }
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
    // Made once: every new window (shown again from the tray) uploads the
    // same picture.
    static IMAGE: std::sync::OnceLock<ColorImage> = std::sync::OnceLock::new();
    let image = IMAGE.get_or_init(|| {
        let half = TEXTURE as f32 * 0.5;
        let pixels = (0..TEXTURE * TEXTURE)
            .map(|i| {
                let (x, y) = ((i % TEXTURE) as f32 + 0.5 - half, (i / TEXTURE) as f32 + 0.5 - half);
                let a = falloff((x * x + y * y).sqrt() / half);
                let v = (a * 255.0).round() as u8;
                Color32::from_rgba_premultiplied(v, v, v, v)
            })
            .collect();
        ColorImage::new([TEXTURE, TEXTURE], pixels)
    });
    let t = ctx.load_texture("md3e-ambient-cloud", image.clone(), TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}
