//! Helpers shared by the components of this crate: animated state layers,
//! enter/exit presence for overlays, layer scaling and truncated text.

use std::sync::Arc;

use egui::{
    Color32, Context, Galley, Id, LayerId, Painter, Pos2, Response,
    emath::TSTransform,
    text::TextWrapping,
};
use nikgob_motion::{Animated, Spring};

use super::anim;
use crate::{Theme, TypeStyle, tokens::state};

/// Whether the widget has keyboard focus (shows the focus ring).
pub(crate) fn focused(r: &Response) -> bool {
    r.has_focus()
}

/// The raw state-layer opacity for the current pointer/keyboard state.
pub(crate) fn state_target(r: &Response, enabled: bool, dragged: bool) -> f32 {
    if !enabled {
        0.0
    } else if dragged {
        state::DRAGGED
    } else if r.is_pointer_button_down_on() {
        state::PRESSED
    } else if focused(r) {
        state::FOCUS
    } else if r.hovered() {
        state::HOVER
    } else {
        0.0
    }
}

/// The state-layer opacity, animated with fast effects.
pub(crate) fn layer_opacity(ctx: &Context, id: Id, r: &Response, enabled: bool) -> f32 {
    let motion = Theme::get(ctx).motion;
    anim::fraction(
        ctx,
        id.with("layer"),
        state_target(r, enabled, false),
        motion.fast_effects,
    )
}

/// Enter/exit progress of an overlay. Both values run 0 -> 1 when `open`;
/// `spatial` may overshoot, `effects` never does.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Presence {
    pub spatial: f32,
    pub effects: f32,
}

impl Presence {
    /// Anything left to draw (opening, open or still fading out).
    pub fn visible(self) -> bool {
        self.effects > 0.002 || self.spatial.abs() > 0.002
    }

    pub fn alpha(self) -> f32 {
        self.effects.clamp(0.0, 1.0)
    }
}

fn presence_one(ctx: &Context, key: Id, open: bool, spring: Spring) -> f32 {
    let exists = ctx.data(|d| d.get_temp::<Animated<f32>>(key).is_some());
    if !exists {
        if !open {
            return 0.0;
        }
        // Seed at 0 so the first frame animates in.
        anim::fraction(ctx, key, 0.0, spring);
    }
    let v = anim::fraction(ctx, key, if open { 1.0 } else { 0.0 }, spring);
    if !open {
        let t = anim::now(ctx);
        let settled = ctx.data(|d| {
            d.get_temp::<Animated<f32>>(key)
                .is_none_or(|a| a.is_settled(t))
        });
        if settled {
            ctx.data_mut(|d| d.remove::<Animated<f32>>(key));
            return 0.0;
        }
    }
    v
}

/// Presence of an overlay identified by `id`. Calling it with `open = false`
/// once the exit finished costs nothing and leaves no state behind.
pub(crate) fn presence(
    ctx: &Context,
    id: Id,
    open: bool,
    spatial: Spring,
    effects: Spring,
) -> Presence {
    Presence {
        spatial: presence_one(ctx, id.with("p-spatial"), open, spatial),
        effects: presence_one(ctx, id.with("p-effects"), open, effects),
    }
}

/// Scales a whole layer around `pivot` (global coordinates); 1.0 clears it.
pub(crate) fn layer_scale(ctx: &Context, layer: LayerId, pivot: Pos2, scale: f32) {
    let t = if (scale - 1.0).abs() < 1e-3 {
        TSTransform::IDENTITY
    } else {
        TSTransform::new(pivot.to_vec2() * (1.0 - scale), scale)
    };
    ctx.set_transform_layer(layer, t);
}

/// A single-line galley, truncated with an ellipsis at `max_width`.
pub(crate) fn line(
    painter: &Painter,
    text: &str,
    style: TypeStyle,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    crate::typography::layout_sized(painter, style, |style, k| {
        let mut job = style.job(text, color);
        job.wrap = TextWrapping::truncate_at_width((max_width * k).max(1.0));
        job
    })
}

/// An unwrapped galley.
pub(crate) fn plain(
    painter: &Painter,
    text: &str,
    style: TypeStyle,
    color: Color32,
) -> Arc<Galley> {
    crate::typography::layout_sized(painter, style, |style, _| style.job(text, color))
}

/// `color` at `opacity` (for disabled content and fades).
pub(crate) fn fade(color: Color32, opacity: f32) -> Color32 {
    color.gamma_multiply(opacity.clamp(0.0, 1.0))
}

/// Draws the image at `uri` filling `rect` (centre-cropped) with `corner`
/// radii, or a tonal tile with `fallback` while there is no image yet.
/// An image cropped to a Material shape, or without one the initials on
/// `fill` in `on` (artist avatars).
pub(crate) fn paint_shape_avatar(
    ui: &egui::Ui,
    rect: egui::Rect,
    uri: Option<&str>,
    initials: &str,
    shape: nikgob_shapes::MaterialShape,
    fill: Color32,
    on: Color32,
) {
    use nikgob_shapes::paint as shape_paint;
    // The shape is flattened and filled anew each frame: not out of sight.
    if !ui.is_rect_visible(rect) {
        return;
    }
    let cubics = shape.polygon().cubics();
    let opts = shape_paint::FillOptions::for_ppp(ui.ctx().pixels_per_point());
    let tex = uri
        .and_then(|u| crate::covers::texture(ui.ctx(), u, rect.size(), 1.0))
        .map(|t| t.id);
    let mesh = match tex {
        Some(t) => shape_paint::texture_mesh(
            cubics,
            rect,
            0.0,
            t,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
            &opts,
        ),
        None => shape_paint::fill_mesh(cubics, rect, 0.0, fill, &opts),
    };
    ui.painter().add(egui::Shape::mesh(mesh));
    if tex.is_none() && !initials.is_empty() {
        // The avatar's size may be moving: laid out at a whole size.
        let size = (rect.height() * 0.32).max(12.0);
        let style = crate::TypeStyle::new(size, size, 500, 0.0);
        let g = crate::typography::layout_sized(ui.painter(), style, |style, _| {
            egui::text::LayoutJob::simple_singleline(initials.to_owned(), style.font_id(), on)
        });
        let at = egui::Align2::CENTER_CENTER.anchor_size(rect.center(), g.size());
        ui.painter().galley(at.min, g, on);
    }
}

pub(crate) fn paint_image(
    ui: &egui::Ui,
    rect: egui::Rect,
    uri: Option<&str>,
    corner: egui::CornerRadius,
    fallback: crate::Icon,
) {
    paint_image_zoomed(ui, rect, uri, corner, fallback, 1.0);
}

/// [`paint_image`] with the picture magnified `zoom` times inside its frame.
pub(crate) fn paint_image_zoomed(
    ui: &egui::Ui,
    rect: egui::Rect,
    uri: Option<&str>,
    corner: egui::CornerRadius,
    fallback: crate::Icon,
    zoom: f32,
) {
    use egui::{Image, Rect, pos2, vec2};

    use super::paint::{self, Radii};

    if let Some(uri) = uri
        && let Some(texture) = crate::covers::texture(ui.ctx(), uri, rect.size(), zoom)
    {
        let (tw, th) = (texture.size.x.max(1.0), texture.size.y.max(1.0));
        let target = rect.width() / rect.height().max(1.0);
        let aspect = tw / th;
        let uv = if aspect > target {
            let f = target / aspect;
            Rect::from_min_max(pos2((1.0 - f) * 0.5, 0.0), pos2((1.0 + f) * 0.5, 1.0))
        } else {
            let f = aspect / target;
            Rect::from_min_max(pos2(0.0, (1.0 - f) * 0.5), pos2(1.0, (1.0 + f) * 0.5))
        };
        let uv = Rect::from_center_size(uv.center(), uv.size() / zoom.max(1.0));
        Image::from_texture(texture)
            .texture_options(super::PHOTO_TEXTURE)
            .uv(uv)
            .corner_radius(corner)
            .paint_at(ui, rect);
        return;
    }
    let s = Theme::get(ui.ctx()).scheme;
    let radii = Radii([
        f32::from(corner.nw),
        f32::from(corner.ne),
        f32::from(corner.se),
        f32::from(corner.sw),
    ]);
    paint::container(
        ui.painter(),
        rect,
        radii,
        s.secondary_container,
        egui::Stroke::NONE,
    );
    let size = (rect.width().min(rect.height()) * 0.4).clamp(16.0, 56.0);
    fallback
        .widget(true, s.on_secondary_container, size)
        .paint_at(ui, Rect::from_center_size(rect.center(), vec2(size, size)));
}

/// [`paint_image_zoomed`] cut to the outline [`super::paint::container`]
/// draws for `rect` and `radii` (fractional radii, the same points): a cover
/// flush with a card's edge follows the card's own corners at any scale.
/// An edge `seam` says yes to is hard (it meets another picture).
pub(crate) fn paint_image_shaped(
    ui: &egui::Ui,
    rect: egui::Rect,
    uri: Option<&str>,
    radii: super::paint::Radii,
    fallback: crate::Icon,
    zoom: f32,
    seam: &dyn Fn(Pos2, Pos2) -> bool,
) {
    use egui::{Rect, pos2, vec2};

    use super::paint;

    if !ui.is_rect_visible(rect) {
        return;
    }
    if let Some(uri) = uri
        && let Some(texture) = crate::covers::texture(ui.ctx(), uri, rect.size(), zoom)
    {
        let (tw, th) = (texture.size.x.max(1.0), texture.size.y.max(1.0));
        let target = rect.width() / rect.height().max(1.0);
        let aspect = tw / th;
        let uv = if aspect > target {
            let f = target / aspect;
            Rect::from_min_max(pos2((1.0 - f) * 0.5, 0.0), pos2((1.0 + f) * 0.5, 1.0))
        } else {
            let f = aspect / target;
            Rect::from_min_max(pos2(0.0, (1.0 - f) * 0.5), pos2(1.0, (1.0 + f) * 0.5))
        };
        let uv = Rect::from_center_size(uv.center(), uv.size() / zoom.max(1.0));
        let points = paint::rounded_rect_points(rect, radii);
        let feather = 1.0 / ui.ctx().pixels_per_point().max(0.01);
        let mesh = paint::textured_polygon(&points, rect, uv, texture.id, feather, &|a, b| !seam(a, b));
        if !mesh.is_empty() {
            ui.painter().add(egui::Shape::mesh(mesh));
        }
        return;
    }
    let s = Theme::get(ui.ctx()).scheme;
    paint::container(ui.painter(), rect, radii, s.secondary_container, egui::Stroke::NONE);
    let size = (rect.width().min(rect.height()) * 0.4).clamp(16.0, 56.0);
    fallback
        .widget(true, s.on_secondary_container, size)
        .paint_at(ui, Rect::from_center_size(rect.center(), vec2(size, size)));
}

/// `c` fully transparent but keeping its hue, so colour springs between
/// "none" and `c` do not pass through black.
pub(crate) fn clear(c: Color32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, 0)
}

/// Paints a full-window scrim (32 % of `scrim`) at `alpha` in its own layer
/// and returns its response, which swallows input for everything below.
pub(crate) fn scrim(ctx: &Context, id: Id, alpha: f32) -> Response {
    let screen = ctx.content_rect();
    let color = Theme::get(ctx)
        .scheme
        .scrim_overlay()
        .gamma_multiply(alpha.clamp(0.0, 1.0));
    egui::Area::new(id)
        .fade_in(false)
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .constrain(false)
        .interactable(true)
        .show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
            ui.painter().rect_filled(rect, 0.0, color);
            resp
        })
        .inner
}
