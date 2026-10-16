//! Common buttons and icon buttons (M3 Expressive): five sizes, round or
//! square, corners that morph on press, toggles that change shape and colour.

use egui::{
    Color32, Id, Rect, Response, Sense, Stroke, Ui, Vec2, Widget, text::LayoutJob, vec2,
};

use super::{anim, paint, paint::Radii};
use crate::{
    ColorScheme, Icon, Theme, TypeRole,
    tokens::{Elevation, shape, state},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Size {
    Xs,
    #[default]
    S,
    M,
    L,
    Xl,
}

impl Size {
    pub const fn height(self) -> f32 {
        match self {
            Self::Xs => 32.0,
            Self::S => 40.0,
            Self::M => 56.0,
            Self::L => 96.0,
            Self::Xl => 136.0,
        }
    }

    /// Icon size inside a labelled button.
    pub const fn button_icon(self) -> f32 {
        match self {
            Self::Xs | Self::S => 20.0,
            Self::M => 24.0,
            Self::L => 32.0,
            Self::Xl => 40.0,
        }
    }

    /// Icon size inside an icon button.
    pub const fn icon_button_icon(self) -> f32 {
        match self {
            Self::Xs => 20.0,
            Self::S | Self::M => 24.0,
            Self::L => 32.0,
            Self::Xl => 40.0,
        }
    }

    pub const fn side_padding(self) -> f32 {
        match self {
            Self::Xs | Self::S => 16.0,
            Self::M => 24.0,
            Self::L => 48.0,
            Self::Xl => 64.0,
        }
    }

    pub const fn gap(self) -> f32 {
        match self {
            Self::Xs | Self::S | Self::M => 8.0,
            Self::L => 12.0,
            Self::Xl => 16.0,
        }
    }

    pub const fn label(self) -> TypeRole {
        match self {
            Self::Xs | Self::S => TypeRole::LabelLarge,
            Self::M => TypeRole::TitleMedium,
            Self::L => TypeRole::HeadlineSmall,
            Self::Xl => TypeRole::HeadlineLarge,
        }
    }

    pub const fn square_radius(self) -> f32 {
        match self {
            Self::Xs | Self::S => shape::MEDIUM,
            Self::M => shape::LARGE,
            Self::L | Self::Xl => shape::EXTRA_LARGE,
        }
    }

    pub const fn pressed_radius(self) -> f32 {
        match self {
            Self::Xs | Self::S => shape::SMALL,
            Self::M => shape::MEDIUM,
            Self::L | Self::Xl => shape::LARGE,
        }
    }

    pub const fn outline(self) -> f32 {
        match self {
            Self::Xs | Self::S | Self::M => 1.0,
            Self::L => 2.0,
            Self::Xl => 3.0,
        }
    }

    /// Padding between buttons of a standard button group.
    pub const fn group_gap(self) -> f32 {
        match self {
            Self::Xs => 18.0,
            Self::S => 12.0,
            _ => 8.0,
        }
    }

    /// Inner corner radius in a connected button group.
    pub const fn connected_inner(self) -> f32 {
        match self {
            Self::Xs => 4.0,
            Self::S | Self::M => 8.0,
            Self::L => 16.0,
            Self::Xl => 20.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Shape {
    #[default]
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Style {
    Elevated,
    #[default]
    Filled,
    Tonal,
    Outlined,
    Text,
    /// Icon buttons only: no container.
    Standard,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum IconWidth {
    Narrow,
    #[default]
    Default,
    Wide,
}

/// Container, content and border colours of a style in a toggle state.
#[derive(Clone, Copy, Debug)]
pub struct Colors {
    pub container: Color32,
    pub content: Color32,
    pub border: Color32,
}

impl Style {
    /// `toggle`: None for a plain button, Some(selected) for a toggle.
    pub fn colors(self, s: &ColorScheme, toggle: Option<bool>) -> Colors {
        let c = |container, content| Colors {
            container,
            content,
            border: Color32::TRANSPARENT,
        };
        match (self, toggle) {
            (Self::Elevated, Some(true)) => c(s.primary, s.on_primary),
            (Self::Elevated, _) => c(s.surface_container_low, s.primary),
            (Self::Filled, Some(false)) => c(s.surface_container, s.on_surface_variant),
            (Self::Filled, _) => c(s.primary, s.on_primary),
            (Self::Tonal, Some(true)) => c(s.secondary, s.on_secondary),
            (Self::Tonal, _) => c(s.secondary_container, s.on_secondary_container),
            (Self::Outlined, Some(true)) => c(s.inverse_surface, s.inverse_on_surface),
            (Self::Outlined, _) => Colors {
                container: Color32::TRANSPARENT,
                content: s.on_surface_variant,
                border: s.outline_variant,
            },
            (Self::Text, _) => c(Color32::TRANSPARENT, s.primary),
            (Self::Standard, Some(true)) => c(Color32::TRANSPARENT, s.primary),
            (Self::Standard, _) => c(Color32::TRANSPARENT, s.on_surface_variant),
        }
    }

    fn rest_elevation(self) -> Elevation {
        if self == Self::Elevated {
            Elevation::Level1
        } else {
            Elevation::Level0
        }
    }
}

/// Everything the shared painter needs for one frame of a button.
pub(crate) struct Visual<'a> {
    pub id: Id,
    pub rect: Rect,
    pub response: &'a Response,
    pub style: Style,
    pub size: Size,
    pub enabled: bool,
    /// Corner radii at rest (already resolved from shape/toggle/group).
    pub rest: Radii,
    /// Radii while pressed.
    pub pressed: Radii,
    pub toggle: Option<bool>,
    /// Overrides the style's container colour (e.g. vibrant toolbar items).
    pub colors: Option<Colors>,
}

/// Paints the container, state layer and focus ring; returns the content colour.
pub(crate) fn paint_container(ui: &Ui, v: &Visual<'_>) -> Color32 {
    let theme = Theme::get(ui.ctx());
    let motion = theme.motion;
    let s = &theme.scheme;
    let ctx = ui.ctx();
    let r = v.response;
    let pressed = v.enabled && r.is_pointer_button_down_on();
    let hovered = v.enabled && r.hovered();
    let focused = v.enabled && r.has_focus() && ctx.memory(|m| m.focused() == Some(r.id));

    let mut colors = v.colors.unwrap_or_else(|| v.style.colors(s, v.toggle));
    if !v.enabled {
        let has_container = colors.container.a() > 0;
        colors.container = if has_container {
            s.on_surface.gamma_multiply(state::DISABLED_CONTAINER)
        } else {
            Color32::TRANSPARENT
        };
        colors.content = s.on_surface.gamma_multiply(state::DISABLED_CONTENT);
        if colors.border.a() > 0 {
            colors.border = s.on_surface.gamma_multiply(state::DISABLED_CONTAINER);
        }
    }

    let container = anim::color(ctx, v.id.with("bg"), colors.container, motion.fast_effects);
    let content = anim::color(ctx, v.id.with("fg"), colors.content, motion.fast_effects);
    let border = anim::color(ctx, v.id.with("bd"), colors.border, motion.fast_effects);

    let morph = anim::fraction(
        ctx,
        v.id.with("press"),
        if pressed { 1.0 } else { 0.0 },
        motion.fast_spatial,
    );
    // Toggle shape changes ride the same spring through `rest`.
    // Resolve "full" (infinite) radii to the rect first: springs need
    // finite targets.
    let rest_target = v.rest.clamped(v.rect);
    let pressed_target = v.pressed.clamped(v.rect);
    let rest = Radii(
        [0, 1, 2, 3]
            .map(|i| anim::f32(ctx, v.id.with(("r", i)), rest_target.0[i], motion.fast_spatial)),
    );
    let radii = rest.lerp(pressed_target, morph.clamp(0.0, 1.2)).clamped(v.rect);

    let layer = if pressed {
        state::PRESSED
    } else if focused {
        state::FOCUS
    } else if hovered {
        state::HOVER
    } else {
        0.0
    };
    let layer = anim::fraction(ctx, v.id.with("layer"), layer, motion.fast_effects);

    let painter = ui.painter();
    let mut elevation = if v.enabled {
        v.style.rest_elevation()
    } else {
        Elevation::Level0
    };
    if hovered && elevation == Elevation::Level1 {
        elevation = Elevation::Level2;
    }
    paint::shadow(painter, v.rect, radii, elevation);
    let stroke = if border.a() > 0 {
        Stroke::new(v.size.outline(), border)
    } else {
        Stroke::NONE
    };
    paint::container(painter, v.rect, radii, container, stroke);
    paint::state_layer(painter, v.rect, radii, content, layer);
    if focused {
        paint::focus_ring(painter, v.rect, radii, s.secondary);
    }
    content
}

/// Rest radii for a shape, flipped when a toggle is selected.
pub(crate) fn rest_radius(size: Size, shape: Shape, toggle: Option<bool>) -> f32 {
    let round = matches!(
        (shape, toggle == Some(true)),
        (Shape::Round, false) | (Shape::Square, true)
    );
    if round {
        shape::FULL
    } else {
        size.square_radius()
    }
}

/// Grows the interactive area to the 48 dp minimum around a smaller visual.
pub(crate) fn hit_rect(visual: Rect) -> Rect {
    let min = crate::tokens::spacing::TARGET;
    let grow = vec2(
        ((min - visual.width()) * 0.5).max(0.0),
        ((min - visual.height()) * 0.5).max(0.0),
    );
    visual.expand2(grow)
}

/// A labelled button with an optional leading icon.
#[must_use = "call .ui(ui) or ui.add(button)"]
pub struct Button {
    label: String,
    icon: Option<Icon>,
    size: Size,
    style: Style,
    shape: Shape,
    toggle: Option<bool>,
    enabled: bool,
    min_width: f32,
    /// Extra width (or less, if negative) from a button group.
    width_delta: f32,
    radii: Option<Radii>,
    pressed_radii: Option<Radii>,
    id_salt: Option<Id>,
    emphasized: bool,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            size: Size::S,
            style: Style::Filled,
            shape: Shape::Round,
            toggle: None,
            enabled: true,
            min_width: 0.0,
            width_delta: 0.0,
            radii: None,
            pressed_radii: None,
            id_salt: None,
            emphasized: false,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    /// Makes this a toggle button in the given state (text buttons cannot toggle).
    pub fn selected(mut self, selected: bool) -> Self {
        self.toggle = Some(selected);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn min_width(mut self, w: f32) -> Self {
        self.min_width = w;
        self
    }

    pub fn emphasized(mut self, emphasized: bool) -> Self {
        self.emphasized = emphasized;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }

    pub(crate) fn width_delta(mut self, d: f32) -> Self {
        self.width_delta = d;
        self
    }

    pub(crate) fn radii(mut self, rest: Radii, pressed: Radii) -> Self {
        self.radii = Some(rest);
        self.pressed_radii = Some(pressed);
        self
    }

    fn job(&self, color: Color32) -> LayoutJob {
        let role = self.size.label();
        let style = if self.emphasized {
            role.emphasized()
        } else {
            role.style()
        };
        style.job(self.label.clone(), color)
    }

    /// Natural width without group adjustments.
    pub(crate) fn natural_width(&self, ui: &Ui) -> f32 {
        let galley = ui.painter().layout_job(self.job(Color32::WHITE));
        let mut w = galley.size().x + 2.0 * self.size.side_padding();
        if self.icon.is_some() {
            w += self.size.button_icon() + self.size.gap();
        }
        w.max(self.min_width).max(self.size.height())
    }
}

impl Widget for Button {
    fn ui(self, ui: &mut Ui) -> Response {
        let natural = self.natural_width(ui);
        let size = vec2(natural + self.width_delta, self.size.height());
        // The id before the allocation: afterwards `next_auto_id` is the
        // next widget's, whose rect would replace this button's hit area.
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |s| ui.id().with(s));
        let (visual, _) = ui.allocate_exact_size(size, Sense::hover());
        let response = ui.interact(hit_rect(visual), id, Sense::click());

        if ui.is_rect_visible(visual) {
            let rest = self.radii.unwrap_or_else(|| {
                Radii::all(rest_radius(self.size, self.shape, self.toggle))
            });
            let pressed = self
                .pressed_radii
                .unwrap_or_else(|| Radii::all(self.size.pressed_radius()));
            let content = paint_container(
                ui,
                &Visual {
                    id,
                    rect: visual,
                    response: &response,
                    style: self.style,
                    size: self.size,
                    enabled: self.enabled,
                    rest,
                    pressed,
                    toggle: self.toggle,
                    colors: None,
                },
            );
            let galley = ui.painter().layout_job(self.job(content));
            let icon_w = if self.icon.is_some() {
                self.size.button_icon() + self.size.gap()
            } else {
                0.0
            };
            let total = icon_w + galley.size().x;
            let mut x = visual.center().x - total * 0.5;
            if let Some(icon) = self.icon {
                let s = self.size.button_icon();
                let r = Rect::from_min_size(egui::pos2(x, visual.center().y - s * 0.5), Vec2::splat(s));
                icon.widget(self.toggle == Some(true), content, s)
                    .paint_at(ui, r);
                x += icon_w;
            }
            let pos = egui::pos2(x, visual.center().y - galley.size().y * 0.5);
            ui.painter().galley(pos, galley, content);
        }
        let label = self.label;
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                self.enabled,
                self.toggle == Some(true),
                &label,
            )
        });
        response
    }
}

/// An icon-only button.
#[must_use = "call .ui(ui) or ui.add(button)"]
pub struct IconButton {
    icon: Icon,
    /// A different icon for the selected state (e.g. play → pause).
    selected_icon: Option<Icon>,
    size: Size,
    style: Style,
    shape: Shape,
    width: IconWidth,
    toggle: Option<bool>,
    enabled: bool,
    tooltip: Option<String>,
    width_delta: f32,
    radii: Option<Radii>,
    pressed_radii: Option<Radii>,
    colors: Option<Colors>,
    id_salt: Option<Id>,
    filled_icon: bool,
}

impl IconButton {
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            selected_icon: None,
            size: Size::S,
            style: Style::Standard,
            shape: Shape::Round,
            width: IconWidth::Default,
            toggle: None,
            enabled: true,
            tooltip: None,
            width_delta: 0.0,
            radii: None,
            pressed_radii: None,
            colors: None,
            id_salt: None,
            filled_icon: false,
        }
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn shape(mut self, shape: Shape) -> Self {
        self.shape = shape;
        self
    }

    pub fn width(mut self, width: IconWidth) -> Self {
        self.width = width;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.toggle = Some(selected);
        self
    }

    pub fn selected_icon(mut self, icon: Icon) -> Self {
        self.selected_icon = Some(icon);
        self
    }

    /// Draw the filled form of the icon even when not selected.
    pub fn filled_icon(mut self, filled: bool) -> Self {
        self.filled_icon = filled;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn colors(mut self, colors: Colors) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }

    pub(crate) fn width_delta(mut self, d: f32) -> Self {
        self.width_delta = d;
        self
    }

    pub(crate) fn radii(mut self, rest: Radii, pressed: Radii) -> Self {
        self.radii = Some(rest);
        self.pressed_radii = Some(pressed);
        self
    }

    pub(crate) fn natural_width(&self) -> f32 {
        let pad = match (self.size, self.width) {
            (Size::Xs, IconWidth::Narrow) => 4.0,
            (Size::Xs, IconWidth::Default) => 6.0,
            (Size::Xs, IconWidth::Wide) => 10.0,
            (Size::S, IconWidth::Narrow) => 4.0,
            (Size::S, IconWidth::Default) => 8.0,
            (Size::S, IconWidth::Wide) => 14.0,
            (Size::M, IconWidth::Narrow) => 12.0,
            (Size::M, IconWidth::Default) => 16.0,
            (Size::M, IconWidth::Wide) => 24.0,
            (Size::L, IconWidth::Narrow) => 16.0,
            (Size::L, IconWidth::Default) => 32.0,
            (Size::L, IconWidth::Wide) => 48.0,
            (Size::Xl, IconWidth::Narrow) => 32.0,
            (Size::Xl, IconWidth::Default) => 48.0,
            (Size::Xl, IconWidth::Wide) => 72.0,
        };
        let w = self.size.icon_button_icon() + 2.0 * pad;
        if self.width == IconWidth::Default {
            w.max(self.size.height())
        } else {
            w
        }
    }
}

impl Widget for IconButton {
    fn ui(self, ui: &mut Ui) -> Response {
        let size = vec2(self.natural_width() + self.width_delta, self.size.height());
        // The id before the allocation: afterwards `next_auto_id` is the
        // next widget's, whose rect would replace this button's hit area.
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |s| ui.id().with(s));
        let (visual, _) = ui.allocate_exact_size(size, Sense::hover());
        let mut response = ui.interact(hit_rect(visual), id, Sense::click());

        if ui.is_rect_visible(visual) {
            let rest = self.radii.unwrap_or_else(|| {
                Radii::all(rest_radius(self.size, self.shape, self.toggle))
            });
            let pressed = self
                .pressed_radii
                .unwrap_or_else(|| Radii::all(self.size.pressed_radius()));
            let content = paint_container(
                ui,
                &Visual {
                    id,
                    rect: visual,
                    response: &response,
                    style: self.style,
                    size: self.size,
                    enabled: self.enabled,
                    rest,
                    pressed,
                    toggle: self.toggle,
                    colors: self.colors,
                },
            );
            let selected = self.toggle == Some(true);
            let icon = match (selected, self.selected_icon) {
                (true, Some(i)) => i,
                _ => self.icon,
            };
            let s = self.size.icon_button_icon();
            let r = Rect::from_center_size(visual.center(), Vec2::splat(s));
            icon.widget(selected || self.filled_icon, content, s)
                .paint_at(ui, r);
        }
        if let Some(tip) = &self.tooltip {
            response = response.on_hover_text(tip);
        }
        let label = self.tooltip.clone().unwrap_or_else(|| self.icon.name().to_owned());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                self.enabled,
                self.toggle == Some(true),
                &label,
            )
        });
        response
    }
}

impl Button {
    pub(crate) fn height(&self) -> f32 {
        self.size.height()
    }
}

impl IconButton {
    pub(crate) fn height(&self) -> f32 {
        self.size.height()
    }
}
