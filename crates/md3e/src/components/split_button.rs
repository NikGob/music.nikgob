//! Split button: a leading action button and
//! a trailing menu button, 2 dp apart. Outer corners are full, the inner
//! ones small (XS-M 4, L 8, XL 12); when the menu opens the trailing button
//! turns fully round and its chevron rotates.

use std::f32::consts::PI;

use egui::{Id, Rect, Response, Sense, Ui, Vec2, vec2};

use super::{
    anim,
    button::{Button, Size, Style, Visual, hit_rect, paint_container},
    menu::{Menu, MenuItem},
    paint::Radii,
};
use crate::{Icon, Theme, tokens::shape};

const GAP: f32 = 2.0;

/// Inner corner radius of the two halves.
const fn inner_radius(size: Size) -> f32 {
    match size {
        Size::Xs | Size::S | Size::M => 4.0,
        Size::L => 8.0,
        Size::Xl => 12.0,
    }
}

/// Horizontal padding around the chevron.
const fn trigger_padding(size: Size) -> f32 {
    match size {
        Size::Xs | Size::S => 12.0,
        Size::M => 13.0,
        Size::L => 26.0,
        Size::Xl => 37.0,
    }
}

/// The chevron sits slightly left of centre (optical alignment).
const fn chevron_shift(size: Size) -> f32 {
    match size {
        Size::Xs | Size::S => -1.0,
        Size::M => -2.0,
        Size::L => -3.0,
        Size::Xl => -6.0,
    }
}

/// What a split button reported.
pub struct SplitOutput {
    /// The action button.
    pub main: Response,
    /// The menu button.
    pub trigger: Response,
    /// Index of the menu item that was picked.
    pub picked: Option<usize>,
}

/// A split button with its menu.
#[must_use = "call .show(ui)"]
pub struct SplitButton {
    id: Id,
    label: String,
    icon: Option<Icon>,
    size: Size,
    style: Style,
    enabled: bool,
    items: Vec<MenuItem>,
    trigger_tooltip: String,
    width_delta: f32,
}

impl SplitButton {
    pub fn new(id_salt: impl egui::AsId, label: impl Into<String>) -> Self {
        Self {
            id: Id::new(id_salt),
            label: label.into(),
            icon: None,
            size: Size::M,
            style: Style::Tonal,
            enabled: true,
            items: Vec::new(),
            trigger_tooltip: "Ещё варианты".to_owned(),
            width_delta: 0.0,
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

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Adds a menu item.
    pub fn item(mut self, item: MenuItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn trigger_tooltip(mut self, text: impl Into<String>) -> Self {
        self.trigger_tooltip = text.into();
        self
    }

    /// Extra width on the action button (from a button group).
    pub(crate) fn width_delta(mut self, d: f32) -> Self {
        self.width_delta = d;
        self
    }

    fn trigger_width(&self) -> f32 {
        self.size.icon_button_icon() + 2.0 * trigger_padding(self.size)
    }

    fn main_button(&self) -> Button {
        let inner = inner_radius(self.size);
        let b = Button::new(&self.label)
            .size(self.size)
            .style(self.style)
            .enabled(self.enabled)
            .radii(
                Radii::sides(shape::FULL, inner),
                Radii::sides(self.size.pressed_radius(), inner),
            )
            .id_salt((self.id, "main"));
        match self.icon {
            Some(i) => b.icon(i),
            None => b,
        }
    }

    /// Width of both halves and the gap, without group adjustments.
    pub(crate) fn natural_width(&self, ui: &Ui) -> f32 {
        self.main_button().natural_width(ui) + GAP + self.trigger_width()
    }

    pub fn show(self, ui: &mut Ui) -> SplitOutput {
        let ctx = ui.ctx().clone();
        let motion = Theme::get(&ctx).motion;
        let id = ui.id().with(self.id);
        let open_key = id.with("open");
        let mut open: bool = ctx.data(|d| d.get_temp(open_key)).unwrap_or(false);
        let inner = inner_radius(self.size);
        let height = self.size.height();
        let tw = self.trigger_width();
        let main_w = self.main_button().natural_width(ui) + self.width_delta;

        let main_btn = self.main_button().width_delta(self.width_delta);
        let mut main = None;
        let mut trigger = None;
        let mut visual = Rect::NOTHING;
        ui.allocate_ui_with_layout(
            vec2(main_w + GAP + tw, height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                main = Some(ui.add(main_btn));

                let (v, _) = ui.allocate_exact_size(vec2(tw, height), Sense::hover());
                visual = v;
                let tid = id.with("trigger");
                let resp = ui.interact(hit_rect(v), tid, Sense::click());
                if ui.is_rect_visible(v) {
                    let rest = if open {
                        Radii::all(shape::FULL)
                    } else {
                        Radii::sides(inner, shape::FULL)
                    };
                    let pressed = Radii::sides(inner, self.size.pressed_radius());
                    let content = paint_container(
                        ui,
                        &Visual {
                            id: tid,
                            rect: v,
                            response: &resp,
                            style: self.style,
                            size: self.size,
                            enabled: self.enabled,
                            rest,
                            pressed,
                            toggle: None,
                            colors: None,
                        },
                    );
                    let turn = anim::fraction(
                        &ctx,
                        tid.with("turn"),
                        if open { 1.0 } else { 0.0 },
                        motion.fast_spatial,
                    );
                    let s = self.size.icon_button_icon();
                    let centre = v.center() + vec2(chevron_shift(self.size), 0.0);
                    Icon::ExpandMore
                        .widget(false, content, s)
                        .rotate(turn * PI, Vec2::splat(0.5))
                        .paint_at(ui, Rect::from_center_size(centre, Vec2::splat(s)));
                }
                trigger = Some(resp.on_hover_text(&self.trigger_tooltip));
            },
        );
        let (main, trigger) = (main.expect("added"), trigger.expect("added"));

        if self.enabled && trigger.clicked() {
            open = !open;
        }
        let mut menu = Menu::new((id, "menu")).align_right(true);
        for item in self.items {
            menu = menu.item(item);
        }
        let picked = menu.popup(&ctx, visual, &mut open);
        ctx.data_mut(|d| d.insert_temp(open_key, open));
        SplitOutput {
            main,
            trigger,
            picked,
        }
    }
}
