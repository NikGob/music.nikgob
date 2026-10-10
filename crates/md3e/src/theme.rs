//! The active theme, stored in the egui context, and its mapping onto egui's
//! stock `Visuals` so widgets we did not write still sit in the palette.

use egui::{
    Color32, Context, CornerRadius, Id, Stroke, Visuals,
    style::{Selection, WidgetVisuals},
};

use crate::{
    color::{ColorScheme, over, with_opacity},
    tokens::{Elevation, Motion, shape, state},
};

/// Everything a component needs to paint and animate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub scheme: ColorScheme,
    pub dark: bool,
    pub motion: Motion,
    /// Reduced motion removes spring overshoot; `motion` already reflects it.
    pub reduced_motion: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(ColorScheme::baseline(true), true)
    }
}

fn store_id() -> Id {
    Id::new("nikgob-md3e-theme")
}

impl Theme {
    /// A theme with Expressive motion.
    pub fn new(scheme: ColorScheme, dark: bool) -> Self {
        Self {
            scheme,
            dark,
            motion: Motion::expressive(),
            reduced_motion: false,
        }
    }

    /// The same theme with reduced motion on or off (choosing the motion set).
    #[must_use]
    pub fn with_reduced_motion(self, reduced: bool) -> Self {
        Self {
            reduced_motion: reduced,
            motion: if reduced {
                Motion::reduced()
            } else {
                Motion::expressive()
            },
            ..self
        }
    }

    /// The same theme with another (for example interpolated) scheme.
    #[must_use]
    pub fn with_scheme(self, scheme: ColorScheme) -> Self {
        Self { scheme, ..self }
    }

    /// The theme stored in `ctx`, or the default dark baseline if none was set.
    pub fn get(ctx: &Context) -> Self {
        ctx.data(|d| d.get_temp::<Self>(store_id()))
            .unwrap_or_default()
    }

    /// Stores `theme` for [`Self::get`]. It does not touch egui's visuals;
    /// see [`Self::install`].
    pub fn set(ctx: &Context, theme: Self) {
        ctx.data_mut(|d| d.insert_temp(store_id(), theme));
    }

    /// [`Self::set`] plus [`Self::apply_egui_visuals`].
    pub fn install(self, ctx: &Context) {
        Self::set(ctx, self);
        self.apply_egui_visuals(ctx);
    }

    /// Maps the roles onto egui's `Visuals` for the context's active theme
    /// (switching it to dark or light to match), so stock widgets inherit
    /// the palette: panels on `surface`, windows on `surface-container-high`,
    /// buttons tonal, text fields on `surface-container-highest`.
    pub fn apply_egui_visuals(&self, ctx: &Context) {
        let theme = if self.dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        ctx.set_theme(theme);
        ctx.set_visuals_of(theme, self.visuals());
    }

    /// The `Visuals` [`Self::apply_egui_visuals`] installs.
    pub fn visuals(&self) -> Visuals {
        let s = &self.scheme;
        let mut v = if self.dark {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        let radius = CornerRadius::same(shape::MEDIUM as u8);
        let widget = |bg: Color32, weak: Color32, stroke: Stroke, fg: Color32| WidgetVisuals {
            bg_fill: bg,
            weak_bg_fill: weak,
            bg_stroke: stroke,
            corner_radius: radius,
            fg_stroke: Stroke::new(1.5, fg),
            expansion: 0.0,
        };

        v.dark_mode = self.dark;
        v.override_text_color = None;
        v.weak_text_color = Some(s.on_surface_variant);
        v.hyperlink_color = s.primary;
        v.faint_bg_color = s.surface_container_low;
        v.extreme_bg_color = s.surface_container_lowest;
        v.text_edit_bg_color = Some(s.surface_container_highest);
        v.code_bg_color = s.surface_container_high;
        v.warn_fg_color = s.tertiary;
        v.error_fg_color = s.error;

        v.panel_fill = s.surface;
        v.window_fill = s.surface_container_high;
        v.window_stroke = Stroke::NONE;
        v.window_corner_radius = CornerRadius::same(shape::EXTRA_LARGE as u8);
        v.window_shadow = Elevation::Level3.shadow();
        v.menu_corner_radius = CornerRadius::same(shape::EXTRA_SMALL as u8);
        v.popup_shadow = Elevation::Level2.shadow();
        v.disabled_alpha = state::DISABLED_CONTENT;

        v.selection = Selection {
            bg_fill: s.primary_container,
            stroke: Stroke::new(1.0, s.on_primary_container),
        };
        v.text_cursor.stroke = Stroke::new(2.0, s.primary);

        let w = &mut v.widgets;
        w.noninteractive = widget(
            s.surface_container,
            s.surface,
            Stroke::new(1.0, s.outline_variant),
            s.on_surface,
        );
        // Buttons are tonal: secondary-container with a state layer on top.
        w.inactive = widget(
            s.surface_container_highest,
            s.secondary_container,
            Stroke::NONE,
            s.on_secondary_container,
        );
        w.hovered = widget(
            over(s.surface_container_highest, s.on_surface, state::HOVER),
            over(
                s.secondary_container,
                s.on_secondary_container,
                state::HOVER,
            ),
            Stroke::NONE,
            s.on_secondary_container,
        );
        w.active = widget(
            over(s.surface_container_highest, s.on_surface, state::PRESSED),
            over(
                s.secondary_container,
                s.on_secondary_container,
                state::PRESSED,
            ),
            Stroke::new(1.0, with_opacity(s.on_secondary_container, state::PRESSED)),
            s.on_secondary_container,
        );
        w.open = widget(
            over(s.surface_container_highest, s.on_surface, state::FOCUS),
            over(
                s.secondary_container,
                s.on_secondary_container,
                state::FOCUS,
            ),
            Stroke::NONE,
            s.on_secondary_container,
        );
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::contrast_ratio;

    #[test]
    fn get_returns_what_set_stored() {
        let ctx = Context::default();
        assert_eq!(Theme::get(&ctx), Theme::default());
        let theme = Theme::new(ColorScheme::baseline(false), false).with_reduced_motion(true);
        Theme::set(&ctx, theme);
        assert_eq!(Theme::get(&ctx), theme);
        assert!(Theme::get(&ctx).motion.fast_spatial.damping_ratio >= 1.0);
    }

    #[test]
    fn visuals_follow_the_roles() {
        for dark in [false, true] {
            let theme = Theme::new(ColorScheme::baseline(dark), dark);
            let v = theme.visuals();
            let s = &theme.scheme;
            assert_eq!(v.dark_mode, dark);
            assert_eq!(v.panel_fill, s.surface);
            assert_eq!(v.window_fill, s.surface_container_high);
            assert_eq!(v.hyperlink_color, s.primary);
            // Default button text is readable on the default button fill.
            let w = &v.widgets.inactive;
            assert!(contrast_ratio(w.fg_stroke.color, w.weak_bg_fill) >= 4.5);
            let n = &v.widgets.noninteractive;
            assert!(contrast_ratio(n.fg_stroke.color, v.panel_fill) >= 4.5);
        }
    }

    #[test]
    fn install_switches_the_context_theme() {
        let ctx = Context::default();
        Theme::new(ColorScheme::baseline(false), false).install(&ctx);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(
            ctx.global_style().visuals.panel_fill,
            ColorScheme::baseline(false).surface
        );
        Theme::default().install(&ctx);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
    }
}
