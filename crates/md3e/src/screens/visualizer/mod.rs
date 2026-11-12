//! The visualizer (md3e-player.md §3.5): four pictures of the music drawn
//! from the spectrum, the beat and the loudness, in the scheme's colours
//! and Material shapes, over the whole window or the whole screen. A
//! floating toolbar at the bottom picks the picture and the screen, and
//! steps aside while the pointer rests. With reduced motion everything
//! holds still.

mod calm;
mod cover;
mod shapes;
mod signal;
mod spectrum;
pub mod synth;

use std::f32::consts::TAU;

use egui::{Area, Color32, Context, CursorIcon, Id, Order, Painter, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2};
use nikgob_motion::Spring;
use nikgob_shapes::{MaterialShape, paint::fill_shape};
use nikgob_view::{Ambient, Command, PrefPatch, Status, ViewState, VizConcept};

use super::common;
use crate::{
    Icon, Theme, TypeRole,
    components::{ConnectedGroup, Glyph, IconButton, Size, ToolbarColor, anim, floating_toolbar},
};

/// Seconds the pointer may rest before the toolbar steps aside.
const IDLE_HIDE: f64 = 2.5;
/// Seconds of the cross-fade between two pictures.
const SWITCH: f64 = 0.35;

/// One frame of a spring toward `target`: position `x`, velocity `v`.
pub(crate) fn spring_step(spring: Spring, x: &mut f32, v: &mut f32, target: f32, dt: f32) {
    let (dx, dv) = spring.solve(*x - target, *v, dt);
    *x = target + dx;
    *v = dv;
}

/// What the visualizer asks of the shell this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VizAction {
    None,
    Close,
    /// Its own full-screen button (as F11).
    ToggleFullscreen,
}

/// Animation state of the visualizer.
#[derive(Default)]
pub struct Viz {
    last: f64,
    opened: f64,
    signal: signal::Signal,
    /// The picture chosen here, ahead of the prefs coming back.
    concept: Option<VizConcept>,
    /// The picture fading out and when the switch began.
    previous: Option<(VizConcept, f64)>,
    cover: cover::Cover,
    spectrum: spectrum::Spectrum,
    shapes: shapes::Shapes,
    calm: calm::Calm,
    /// When the pointer last moved, and where it was.
    moved_at: f64,
    pointer: Option<egui::Pos2>,
}

const NAMES: [&str; 4] = ["Обложка", "Спектр", "Фигуры", "Спокойно"];
const GLYPHS: [Glyph; 4] = [glyph_cover, glyph_spectrum, glyph_shapes, glyph_calm];

fn index(c: VizConcept) -> usize {
    VizConcept::ALL.iter().position(|x| *x == c).unwrap_or(0)
}

impl Viz {
    fn update(&mut self, concept: VizConcept, width: f32, live: bool, reduced: bool) {
        let sig = &self.signal;
        match concept {
            VizConcept::Cover => self.cover.update(sig, reduced),
            VizConcept::Spectrum => self.spectrum.update(sig, width, reduced),
            VizConcept::Shapes => self.shapes.update(sig, reduced),
            VizConcept::Calm => self.calm.update(sig, live, reduced),
        }
    }

    fn paint(&self, concept: VizConcept, ui: &Ui, rect: Rect, s: &crate::ColorScheme, uri: Option<&str>) {
        let painter = ui.painter();
        match concept {
            VizConcept::Cover => self.cover.paint(ui, rect, s, uri),
            VizConcept::Spectrum => self.spectrum.paint(painter, rect, s),
            VizConcept::Shapes => self.shapes.paint(painter, rect, s),
            VizConcept::Calm => self.calm.paint(painter, rect, s),
        }
    }

    /// Draws the visualizer over the whole window (the shell makes the
    /// window full screen when asked). `bands`/`bass` are the analyzer's,
    /// `fullscreen` whether the window is full screen now.
    pub fn show(
        &mut self,
        ctx: &Context,
        state: &ViewState,
        bands: &[f32],
        bass: f32,
        fullscreen: bool,
        out: &mut Vec<Command>,
    ) -> VizAction {
        let theme = Theme::get(ctx);
        let s = theme.scheme;
        let reduced = theme.reduced_motion;
        let now = anim::now(ctx);
        if now - self.last > 0.5 {
            // Opened again: the prefs' picture, the toolbar shown.
            self.opened = now;
            self.moved_at = now;
            self.concept = None;
            self.previous = None;
        }
        self.last = now;
        let concept = *self.concept.get_or_insert(state.prefs.viz_concept);
        let playing = state.player.status == Status::Playing;
        let screen = ctx.content_rect();
        self.signal.update(now, bands, bass, playing);
        self.update(concept, screen.width(), playing, reduced);
        let switch = self.previous.map(|(c, at)| (c, ((now - at) / SWITCH) as f32));
        if let Some((prev, k)) = switch {
            if k >= 1.0 {
                self.previous = None;
            } else {
                self.update(prev, screen.width(), playing, reduced);
            }
        }
        let fade = ((now - self.opened) as f32 / 0.3).clamp(0.0, 1.0);

        // The pointer resting hides the toolbar (and the pointer).
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        if pointer != self.pointer {
            self.pointer = pointer;
            self.moved_at = now;
        }
        let mut action = VizAction::None;

        Area::new(Id::new("md3e-visualizer"))
            .fade_in(false)
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                ui.multiply_opacity(fade);
                // Swallows input meant for what is underneath.
                let (rect, bg) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                if bg.double_clicked() {
                    action = VizAction::ToggleFullscreen;
                }
                ui.painter().rect_filled(rect, 0.0, s.surface);
                let current = state.player.current.as_ref();
                // Soft clouds behind everything (not tied to the cover).
                if state.prefs.ambient != Ambient::Off {
                    super::ambient::paint(ctx, ui.painter(), rect, self.signal.kick * 0.5 + self.signal.calm * 0.5, playing, state.prefs.ambient_strength);
                }
                let uri = current.and_then(|t| t.cover.as_ref()).map(nikgob_view::Cover::uri);
                let ease = |k: f32| 1.0 - (1.0 - k.clamp(0.0, 1.0)).powi(3);
                let k_new = switch.filter(|(_, k)| *k < 1.0).map_or(1.0, |(_, k)| ease(k));
                if let Some((prev, k)) = switch.filter(|(_, k)| *k < 1.0) {
                    let mut old = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("viz-old"));
                    old.multiply_opacity(1.0 - ease(k));
                    self.paint(prev, &old, rect, &s, uri.as_deref());
                }
                let mut pic = ui.new_child(UiBuilder::new().max_rect(rect).id_salt("viz-pic"));
                pic.multiply_opacity(k_new);
                self.paint(concept, &pic, rect, &s, uri.as_deref());

                // Corner: small cover (not over the cover picture) and title.
                let thumb = match (concept, switch.filter(|(_, k)| *k < 1.0)) {
                    (VizConcept::Cover, Some((p, _))) if p != VizConcept::Cover => 1.0 - k_new,
                    (VizConcept::Cover, _) => 0.0,
                    (_, Some((VizConcept::Cover, _))) => k_new,
                    _ => 1.0,
                };
                let cover = Rect::from_min_size(rect.min + vec2(24.0, 24.0), vec2(72.0, 72.0));
                if thumb > 0.0 {
                    let mut cui = ui.new_child(UiBuilder::new().max_rect(cover).id_salt("viz-thumb"));
                    cui.multiply_opacity(thumb);
                    common::cover(&cui, current.and_then(|t| t.cover.as_ref()), cover, 20.0);
                }
                if let Some(t) = current {
                    let left = 24.0 + 88.0 * thumb;
                    let text = Rect::from_min_size(
                        pos2(rect.left() + left, cover.top() + 14.0),
                        vec2((rect.width() * 0.4).clamp(120.0, 360.0), 48.0),
                    );
                    let mut tui = ui.new_child(UiBuilder::new().max_rect(text));
                    tui.spacing_mut().item_spacing.y = 0.0;
                    common::label(&mut tui, &t.title, TypeRole::TitleMedium, s.on_surface);
                    common::label(&mut tui, &t.artist, TypeRole::BodyMedium, s.on_surface_variant);
                }

                // The toolbar: the picture, the screen, close.
                let bar = Rect::from_center_size(
                    pos2(rect.center().x, rect.bottom() - 24.0 - 32.0),
                    vec2(8.0 + GROUP_W + 16.0 + 40.0 + 4.0 + 40.0 + 8.0, 64.0),
                );
                let hovered = pointer.is_some_and(|p| bar.expand(16.0).contains(p));
                let shown = hovered || now - self.moved_at < IDLE_HIDE;
                let k = anim::fraction(ctx, Id::new("viz-toolbar"), if shown { 1.0 } else { 0.0 }, theme.motion.default_effects);
                if !shown && pointer.is_some_and(|p| rect.contains(p)) {
                    ctx.set_cursor_icon(CursorIcon::None);
                }
                if k > 0.01 {
                    let bar = bar.translate(vec2(0.0, 24.0 * (1.0 - k)));
                    let mut tui = ui.new_child(UiBuilder::new().max_rect(bar).id_salt("viz-toolbar"));
                    tui.multiply_opacity(k);
                    if k < 0.5 {
                        tui.disable();
                    }
                    floating_toolbar(&mut tui, bar, ToolbarColor::Standard, |ui| {
                        let picked = ui
                            .allocate_ui(vec2(GROUP_W, 40.0), |ui| {
                                ConnectedGroup::new("viz-concept", &NAMES).glyphs(&GLYPHS).show(ui, index(concept))
                            })
                            .inner;
                        if let Some(i) = picked {
                            let next = VizConcept::ALL[i];
                            self.previous = Some((concept, now));
                            self.concept = Some(next);
                            out.push(Command::SetPref(PrefPatch::VizConcept(next)));
                        }
                        ui.add_space(8.0);
                        if ui
                            .add(
                                IconButton::new(Icon::Fullscreen)
                                    .size(Size::S)
                                    .selected(fullscreen)
                                    .tooltip("Во весь экран (F11)")
                                    .id_salt("viz-fullscreen"),
                            )
                            .clicked()
                        {
                            action = VizAction::ToggleFullscreen;
                        }
                        if ui
                            .add(IconButton::new(Icon::Close).size(Size::S).tooltip("Закрыть (Esc, F12)").id_salt("viz-close"))
                            .clicked()
                        {
                            action = VizAction::Close;
                        }
                    });
                }
            });
        if reduced {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else {
            ctx.request_repaint();
        }
        action
    }
}

/// The picture switcher's width: four 40-point members, 2 apart.
const GROUP_W: f32 = 4.0 * 40.0 + 3.0 * 2.0;

// The switcher's glyphs: each a still of its picture.

fn glyph_cover(p: &Painter, r: Rect, c: Color32) {
    let inner = r.shrink(r.width() * 0.2);
    fill_shape(p, MaterialShape::Cookie9Sided.polygon().cubics(), inner, 0.0, c);
    nikgob_shapes::paint::stroke_shape(p, MaterialShape::Cookie9Sided.polygon().cubics(), r.shrink(0.5), 0.0, Stroke::new(1.5, c));
}

fn glyph_spectrum(p: &Painter, r: Rect, c: Color32) {
    let heights = [0.35, 0.7, 1.0, 0.7, 0.35];
    let w = r.width() / 5.0;
    for (i, h) in heights.iter().enumerate() {
        let x = r.left() + (i as f32 + 0.5) * w;
        let bar = Rect::from_center_size(pos2(x, r.center().y), vec2(w * 0.6, (r.height() * 0.85 * h).max(w * 0.6)));
        p.rect_filled(bar, (w * 0.3) as u8, c);
    }
}

fn glyph_shapes(p: &Painter, r: Rect, c: Color32) {
    let core = r.shrink(r.width() * 0.3);
    fill_shape(p, MaterialShape::SoftBurst.polygon().cubics(), core, 0.0, c);
    for i in 0..6 {
        let a = i as f32 / 6.0 * TAU;
        let at = r.center() + vec2(a.cos(), a.sin()) * r.width() * 0.4;
        p.circle_filled(at, r.width() * 0.08, c);
    }
}

fn glyph_calm(p: &Painter, r: Rect, c: Color32) {
    p.circle_filled(r.center(), r.width() * 0.14, c);
    p.circle_stroke(r.center(), r.width() * 0.3, Stroke::new(1.5, c));
    p.circle_stroke(r.center(), r.width() * 0.46, Stroke::new(1.0, c.gamma_multiply(0.6)));
}
