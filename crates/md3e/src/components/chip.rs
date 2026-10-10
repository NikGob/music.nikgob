//! Chips (32 dp): filter chips (radius 8 -> full when selected, a check
//! appears), assist chips with an optional leading icon, and input chips with
//! a remove button.

use egui::{
    Color32, Id, Rect, Response, Sense, Stroke, Ui, Vec2, Widget, WidgetInfo, WidgetType, pos2,
    vec2,
};

use super::{anim, button::hit_rect, common, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape, state},
};

pub const HEIGHT: f32 = 32.0;
const ICON: f32 = 18.0;
const ICON_SLOT: f32 = ICON + 8.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Filter,
    Assist,
    Input,
}

struct Spec {
    kind: Kind,
    label: String,
    selected: bool,
    icon: Option<Icon>,
    trailing: Option<Icon>,
    elevated: bool,
    enabled: bool,
    id_salt: Option<Id>,
}

struct Painted {
    response: Response,
    trailing_clicked: bool,
}

fn chip(ui: &mut Ui, spec: Spec) -> Painted {
    let ctx = ui.ctx().clone();
    let theme = Theme::get(&ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let id = spec
        .id_salt
        .map_or_else(|| ui.next_auto_id(), |salt| ui.id().with(salt));

    // Filter chips show a check while selected (unless they have an icon).
    let leading = match (spec.kind, spec.selected, spec.icon) {
        (Kind::Filter, true, _) => Some(Icon::Check),
        (_, _, icon) => icon,
    };
    let slot = anim::f32(
        &ctx,
        id.with("slot"),
        if leading.is_some() { ICON_SLOT } else { 0.0 },
        motion.fast_spatial,
    )
    .max(0.0);
    let slot_k = (slot / ICON_SLOT).clamp(0.0, 1.0);

    let content_color = if !spec.enabled {
        common::fade(s.on_surface, state::DISABLED_CONTENT)
    } else if spec.selected && spec.kind != Kind::Assist {
        s.on_secondary_container
    } else {
        s.on_surface_variant
    };
    let galley = common::plain(
        ui.painter(),
        &spec.label,
        TypeRole::LabelLarge.style(),
        content_color,
    );
    let left = 16.0 - 8.0 * slot_k;
    let right = if spec.trailing.is_some() {
        8.0 + ICON_SLOT
    } else {
        16.0
    };
    let width = left + slot + galley.size().x + right;
    let (visual, _) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::hover());
    let sense = if spec.enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(hit_rect(visual), id, sense);

    let mut trailing_clicked = false;
    if ui.is_rect_visible(visual) {
        let selected_look = spec.selected && spec.kind != Kind::Assist;
        // Colours.
        let container = if !spec.enabled {
            if selected_look {
                common::fade(s.on_surface, state::DISABLED_CONTAINER)
            } else {
                Color32::TRANSPARENT
            }
        } else if selected_look {
            s.secondary_container
        } else if spec.elevated {
            s.surface_container_low
        } else {
            common::clear(s.secondary_container)
        };
        let border = if selected_look || spec.elevated {
            common::clear(s.outline_variant)
        } else if spec.enabled {
            s.outline_variant
        } else {
            common::fade(s.on_surface, state::DISABLED_CONTAINER)
        };
        let container = anim::color(&ctx, id.with("bg"), container, motion.fast_effects);
        let border = anim::color(&ctx, id.with("bd"), border, motion.fast_effects);
        let content = anim::color(&ctx, id.with("fg"), content_color, motion.fast_effects);
        let icon_color = if !spec.enabled {
            content
        } else if spec.kind == Kind::Assist {
            s.primary
        } else if selected_look {
            s.on_secondary_container
        } else {
            s.on_surface_variant
        };
        let icon_color = anim::color(&ctx, id.with("ic"), icon_color, motion.fast_effects);

        let radius = anim::f32(
            &ctx,
            id.with("radius"),
            if selected_look && spec.kind == Kind::Filter {
                shape::FULL.min(HEIGHT * 0.5)
            } else {
                shape::SMALL
            },
            motion.fast_spatial,
        );
        let radii = Radii::all(radius).clamped(visual);

        let painter = ui.painter();
        if spec.enabled && spec.elevated {
            let e = if response.hovered() {
                Elevation::Level2
            } else {
                Elevation::Level1
            };
            paint::shadow(painter, visual, radii, e);
        }
        let stroke = if border.a() > 0 {
            Stroke::new(1.0, border)
        } else {
            Stroke::NONE
        };
        paint::container(painter, visual, radii, container, stroke);
        let layer = common::layer_opacity(&ctx, id, &response, spec.enabled);
        paint::state_layer(painter, visual, radii, content, layer);

        // Content.
        let mut x = visual.left() + left;
        if let Some(icon) = leading
            && slot_k > 0.02
        {
            let r = Rect::from_min_size(pos2(x, visual.center().y - ICON * 0.5), Vec2::splat(ICON));
            icon.widget(true, common::fade(icon_color, slot_k), ICON)
                .paint_at(ui, r);
        }
        x += slot;
        ui.painter().galley(
            pos2(x, visual.center().y - galley.size().y * 0.5),
            galley.clone(),
            content,
        );
        if let Some(icon) = spec.trailing {
            let c = pos2(visual.right() - 8.0 - ICON * 0.5, visual.center().y);
            let r = Rect::from_center_size(c, Vec2::splat(ICON));
            let hit = Rect::from_center_size(c, Vec2::splat(32.0));
            let resp = ui.interact(
                hit,
                id.with("trail"),
                if spec.enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                },
            );
            trailing_clicked = resp.clicked();
            let layer = anim::fraction(
                &ctx,
                id.with("trail-layer"),
                common::state_target(&resp, spec.enabled, false),
                motion.fast_effects,
            );
            paint::state_layer(ui.painter(), hit, Radii::all(shape::FULL), content, layer);
            icon.widget(false, icon_color, ICON).paint_at(ui, r);
        }
        if spec.enabled && common::focused(&response) {
            paint::focus_ring(ui.painter(), visual, radii, s.secondary);
        }
    }
    let label = spec.label;
    let (kind, selected, enabled) = (spec.kind, spec.selected, spec.enabled);
    response.widget_info(|| match kind {
        Kind::Assist => WidgetInfo::labeled(WidgetType::Button, enabled, &label),
        _ => WidgetInfo::selected(WidgetType::Button, enabled, selected, &label),
    });
    Painted {
        response,
        trailing_clicked,
    }
}

/// A filter chip: toggles on click (the caller owns the state).
#[must_use = "add it with ui.add(chip)"]
pub struct FilterChip {
    spec: Spec,
}

impl FilterChip {
    pub fn new(label: impl Into<String>, selected: bool) -> Self {
        Self {
            spec: Spec {
                kind: Kind::Filter,
                label: label.into(),
                selected,
                icon: None,
                trailing: None,
                elevated: false,
                enabled: true,
                id_salt: None,
            },
        }
    }

    /// A leading icon shown while unselected (selected shows a check).
    pub fn icon(mut self, icon: Icon) -> Self {
        self.spec.icon = Some(icon);
        self
    }

    /// A trailing icon (for example a dropdown arrow).
    pub fn trailing_icon(mut self, icon: Icon) -> Self {
        self.spec.trailing = Some(icon);
        self
    }

    pub fn elevated(mut self, elevated: bool) -> Self {
        self.spec.elevated = elevated;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.spec.enabled = enabled;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.spec.id_salt = Some(Id::new(salt));
        self
    }
}

impl Widget for FilterChip {
    fn ui(self, ui: &mut Ui) -> Response {
        chip(ui, self.spec).response
    }
}

/// An assist chip with an optional leading icon.
#[must_use = "add it with ui.add(chip)"]
pub struct AssistChip {
    spec: Spec,
}

impl AssistChip {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            spec: Spec {
                kind: Kind::Assist,
                label: label.into(),
                selected: false,
                icon: None,
                trailing: None,
                elevated: false,
                enabled: true,
                id_salt: None,
            },
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.spec.icon = Some(icon);
        self
    }

    pub fn elevated(mut self, elevated: bool) -> Self {
        self.spec.elevated = elevated;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.spec.enabled = enabled;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.spec.id_salt = Some(Id::new(salt));
        self
    }
}

impl Widget for AssistChip {
    fn ui(self, ui: &mut Ui) -> Response {
        chip(ui, self.spec).response
    }
}

/// What an input chip reported.
pub struct InputChipOutput {
    pub response: Response,
    /// The remove (x) button was clicked.
    pub removed: bool,
}

/// An input chip (a tag or filter the user entered) with a remove button.
#[must_use = "call .show(ui)"]
pub struct InputChip {
    spec: Spec,
}

impl InputChip {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            spec: Spec {
                kind: Kind::Input,
                label: label.into(),
                selected: false,
                icon: None,
                trailing: Some(Icon::Close),
                elevated: false,
                enabled: true,
                id_salt: None,
            },
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.spec.selected = selected;
        self
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.spec.icon = Some(icon);
        self
    }

    /// Hides the remove button.
    pub fn removable(mut self, removable: bool) -> Self {
        self.spec.trailing = removable.then_some(Icon::Close);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.spec.enabled = enabled;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.spec.id_salt = Some(Id::new(salt));
        self
    }

    pub fn show(self, ui: &mut Ui) -> InputChipOutput {
        let p = chip(ui, self.spec);
        InputChipOutput {
            response: p.response,
            removed: p.trailing_clicked,
        }
    }
}
