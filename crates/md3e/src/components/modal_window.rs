//! A modal window over the dimmed player: the scrim, the surface with its
//! corners and padding, the motion it comes and goes on, and the ways out
//! (a click on the scrim, Escape).
//!
//! With an origin it grows out of that rect and back (container transform:
//! bounds, corners and colour morph on the default spatial spring, the
//! content fades in once it has room); without one it scales and fades in
//! from the centre on the same spring.
//!
//! Whether it is open is the caller's to keep; [`ModalWindow::rewind`]
//! makes the next opening start from the beginning.

use egui::{Align, Area, Context, Id, Key, Layout, Modifiers, Order, Rect, Sense, Stroke, Ui, UiBuilder, Vec2, vec2};
use nikgob_motion::Animated;

use super::{anim, paint, paint::Radii};
use crate::Theme;
use crate::tokens::{Elevation, shape};

/// Between the window's edge and what it shows.
pub const PADDING: f32 = 24.0;
/// The same of a shell ([`ModalWindow::shell`]), and its corners.
pub const SHELL_PADDING: f32 = 12.0;
const SHELL_RADIUS: f32 = 36.0;
/// The least room left around the window, both sides together.
const MARGIN: f32 = 96.0;
/// The corners of the control a window grows out of.
const ORIGIN_RADIUS: f32 = 16.0;
/// The size a window without an origin starts from, of its own.
const CENTRE_SCALE: f32 = 0.85;

const DRAWN: &str = "md3e-modal-windows-drawn";

/// The windows drawn in the frame at `at`, each with its layer.
#[derive(Clone, Default)]
struct Drawn {
    at: f64,
    windows: Vec<(egui::LayerId, Rect)>,
}

/// A modal window (see the module).
#[must_use = "call .show(ctx, open, |ui, inner| ...)"]
pub struct ModalWindow {
    id: Id,
    max: Vec2,
    height: Option<f32>,
    escape: bool,
    origin: Option<Rect>,
    shell: bool,
}

impl ModalWindow {
    /// A window as large as `max` allows.
    pub fn new(id: Id, max: Vec2) -> Self {
        Self { id, max, height: None, escape: true, origin: None, shell: false }
    }

    /// A shell for cards: a quieter tone, rounder corners and less room at
    /// its edge ([`SHELL_PADDING`]); what it shows stands on cards of a
    /// brighter tone inside it.
    pub fn shell(mut self) -> Self {
        self.shell = true;
        self
    }

    /// As tall as this (its padding included) instead of as tall as it may be.
    pub fn height(mut self, height: f32) -> Self {
        self.height = Some(height);
        self
    }

    /// Whether Escape closes it (it does unless told otherwise).
    pub fn escape(mut self, escape: bool) -> Self {
        self.escape = escape;
        self
    }

    /// The control it grows out of and shrinks back into.
    pub fn origin(mut self, origin: Rect) -> Self {
        self.origin = Some(origin);
        self
    }

    /// Puts the motion of the window `id` at its start, so that it opens
    /// from there.
    pub fn rewind(ctx: &Context, id: Id) {
        let motion = Theme::get(ctx).motion;
        let spatial = anim::effective(ctx, motion.default_spatial);
        let effects = anim::effective(ctx, motion.default_effects);
        ctx.data_mut(|d| {
            d.insert_temp(id.with("k"), Animated::new(0.0_f32, spatial).with_threshold(0.01));
            d.insert_temp(id.with("a"), Animated::new(0.0_f32, effects).with_threshold(0.001));
        });
    }

    /// `at` on `layer` is the scrim of a window, not the window itself:
    /// the wheel there is not the window's (it stays the volume's).
    pub fn scrim_at(ctx: &Context, layer: egui::LayerId, at: egui::Pos2) -> bool {
        ctx.data(|d| d.get_temp::<Drawn>(Id::new(DRAWN)))
            .is_some_and(|drawn| drawn.windows.iter().any(|(l, rect)| *l == layer && !rect.contains(at)))
    }

    /// Where the window stands once it is open.
    pub fn target(&self, ctx: &Context) -> Rect {
        let screen = ctx.content_rect();
        let width = (screen.width() - MARGIN).clamp(320.0, self.max.x);
        let height = match self.height {
            Some(height) => height.min(screen.height() - MARGIN * 0.5),
            None => (screen.height() - MARGIN).clamp(320.0, self.max.y),
        };
        // On whole points, whatever its size: a window of another height
        // (another tab) must not stand half a pixel off the one before.
        let size = vec2(width.round(), height.round());
        let min = (screen.center() - size * 0.5).round();
        Rect::from_min_size(min, size)
    }

    /// Draws the window while it is `open` or still leaving; `body` fills
    /// its inner rect (usable once the window has nearly arrived). True
    /// when a click on the scrim or Escape closed it.
    pub fn show(self, ctx: &Context, open: bool, body: impl FnOnce(&mut Ui, Rect)) -> bool {
        let theme = Theme::get(ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let id = self.id;
        let k = anim::f32(ctx, id.with("k"), if open { 1.0 } else { 0.0 }, motion.default_spatial);
        let alpha = anim::fraction(ctx, id.with("a"), if open { 1.0 } else { 0.0 }, motion.default_effects).clamp(0.0, 1.0);
        if !open && alpha < 0.002 && k.abs() < 0.002 {
            return false;
        }
        let screen = ctx.content_rect();
        let target = self.target(ctx);
        let grows = self.origin.is_some();
        let from = self.origin.unwrap_or_else(|| Rect::from_center_size(target.center(), target.size() * CENTRE_SCALE));
        let kc = k.clamp(0.0, 1.03);
        let k01 = k.clamp(0.0, 1.0);
        let rect = Rect::from_min_max(from.min.lerp(target.min, kc), from.max.lerp(target.max, kc));
        // Out of a control: its corners and colour turn into the window's,
        // the content appears once the container has grown most of the
        // way. From the centre: the window fades in whole.
        let (corner, tone, padding) =
            if self.shell { (SHELL_RADIUS, s.surface_container, SHELL_PADDING) } else { (shape::EXTRA_LARGE, s.surface_container_high, PADDING) };
        let (radius, fill, content_a) = if grows {
            (
                ORIGIN_RADIUS + (corner - ORIGIN_RADIUS) * k01,
                lerp_color(s.secondary_container, tone, k01),
                ((k - 0.45) / 0.55).clamp(0.0, 1.0) * alpha,
            )
        } else {
            (corner, tone.gamma_multiply(alpha), alpha)
        };

        // Where the window is on its layer, for [`Self::scrim_at`].
        let layer = egui::LayerId::new(Order::Tooltip, id.with("dialog"));
        let time = ctx.input(|i| i.time);
        ctx.data_mut(|d| {
            let drawn = d.get_temp_mut_or_default::<Drawn>(Id::new(DRAWN));
            if drawn.at != time {
                drawn.at = time;
                drawn.windows.clear();
            }
            drawn.windows.push((layer, rect));
        });

        // One layer over everything (Now Playing too): the scrim that dims
        // the player and closes on a click, then the window.
        let mut scrim_clicked = false;
        Area::new(id.with("dialog"))
            .fade_in(false)
            .order(Order::Tooltip)
            .fixed_pos(screen.min)
            .constrain(false)
            .interactable(open)
            .show(ctx, |ui| {
                ui.set_clip_rect(screen);
                let (r, scrim) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(r, 0.0, s.scrim_overlay().gamma_multiply(alpha));
                scrim_clicked = scrim.clicked() && !rect.contains(scrim.interact_pointer_pos().unwrap_or_default());
                let radii = Radii::all(radius).clamped(rect);
                // No shadow under a surface that is still half seen through.
                if grows || alpha > 0.5 {
                    paint::shadow(ui.painter(), rect, radii, if alpha > 0.5 { Elevation::Level3 } else { Elevation::Level1 });
                }
                paint::container(ui.painter(), rect, radii, fill, Stroke::NONE);
                let inner = target.shrink(padding);
                let mut cui = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)).id_salt("modal-window-body"));
                cui.set_clip_rect(rect.shrink(2.0).intersect(screen));
                cui.multiply_opacity(content_a);
                // Usable once it has (nearly) arrived; a disabled ui fades
                // what it paints, so only the content is.
                if !(open && k > 0.9) {
                    cui.disable();
                }
                body(&mut cui, inner);
            });
        open && (scrim_clicked || (self.escape && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape))))
    }
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let (a, b) = (a.to_srgba_unmultiplied(), b.to_srgba_unmultiplied());
    let m = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8;
    egui::Color32::from_rgba_unmultiplied(m(0), m(1), m(2), m(3))
}
