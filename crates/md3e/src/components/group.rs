//! Button groups: the standard group (pressing a button widens it and
//! squeezes its neighbours) and the connected group (2 dp gaps, inner
//! corners small, the selected item fully round).

use egui::{Id, Response, Ui};

use super::{
    anim,
    button::{Button, IconButton, Size},
    paint::Radii,
};
use crate::{Theme, tokens::shape};

/// One member of a group.
pub enum Item {
    Button(Button),
    Icon(IconButton),
}

impl From<Button> for Item {
    fn from(b: Button) -> Self {
        Self::Button(b)
    }
}

impl From<IconButton> for Item {
    fn from(b: IconButton) -> Self {
        Self::Icon(b)
    }
}

impl Item {
    fn natural_width(&self, ui: &Ui) -> f32 {
        match self {
            Self::Button(b) => b.natural_width(ui),
            Self::Icon(b) => b.natural_width(),
        }
    }

    fn height(&self) -> f32 {
        match self {
            Self::Button(b) => b.height(),
            Self::Icon(b) => b.height(),
        }
    }
}

/// How much a pressed member grows, as a fraction of its width (Compose
/// `ButtonGroupDefaults.ExpandedRatio`).
const EXPAND_RATIO: f32 = 0.15;

/// A standard button group. Members should share size and shape.
pub struct ButtonGroup {
    id: Id,
    size: Size,
}

impl ButtonGroup {
    pub fn new(id_salt: impl egui::AsId, size: Size) -> Self {
        Self {
            id: Id::new(id_salt),
            size,
        }
    }

    pub fn show(self, ui: &mut Ui, items: Vec<Item>) -> Vec<Response> {
        let id = ui.id().with(self.id);
        let motion = Theme::get(ui.ctx()).motion;
        let pressed: Option<usize> = ui.ctx().data(|d| d.get_temp(id.with("pressed")));
        let widths: Vec<f32> = items.iter().map(|i| i.natural_width(ui)).collect();
        let n = items.len();

        let mut deltas = vec![0.0; n];
        if let Some(p) = pressed.filter(|&p| p < n) {
            let grow = widths[p] * EXPAND_RATIO;
            deltas[p] += grow;
            let neighbours: Vec<usize> = [p.checked_sub(1), (p + 1 < n).then_some(p + 1)]
                .into_iter()
                .flatten()
                .collect();
            for &j in &neighbours {
                deltas[j] -= grow / neighbours.len() as f32;
            }
        }

        let mut responses = Vec::with_capacity(n);
        let mut now_pressed = None;
        // A fixed-height row so members of different sizes (the hero play
        // button among M buttons) share one centre line.
        let row_h = items.iter().map(Item::height).fold(0.0, f32::max);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), row_h),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
            ui.spacing_mut().item_spacing.x = self.size.group_gap();
            for (i, item) in items.into_iter().enumerate() {
                let d = anim::f32(ui.ctx(), id.with(("w", i)), deltas[i], motion.fast_spatial);
                let r = match item {
                    Item::Button(b) => ui.add(b.width_delta(d).id_salt((self.id, i))),
                    Item::Icon(b) => ui.add(b.width_delta(d).id_salt((self.id, i))),
                };
                if r.is_pointer_button_down_on() {
                    now_pressed = Some(i);
                }
                responses.push(r);
            }
            },
        );
        ui.ctx().data_mut(|d| match now_pressed {
            Some(p) => {
                d.insert_temp(id.with("pressed"), p);
            }
            None => d.remove::<usize>(id.with("pressed")),
        });
        responses
    }
}

/// A connected button group used as a single-select switcher (replaces
/// segmented buttons). Returns the newly selected index when it changes.
pub struct ConnectedGroup<'a> {
    id: Id,
    size: Size,
    labels: &'a [&'a str],
    icons: Option<&'a [crate::Icon]>,
    equal_width: Option<f32>,
}

impl<'a> ConnectedGroup<'a> {
    pub fn new(id_salt: impl egui::AsId, labels: &'a [&'a str]) -> Self {
        Self {
            id: Id::new(id_salt),
            size: Size::S,
            labels,
            icons: None,
            equal_width: None,
        }
    }

    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Icon-only members (labels then become tooltips).
    pub fn icons(mut self, icons: &'a [crate::Icon]) -> Self {
        self.icons = Some(icons);
        self
    }

    /// Give every member the same width.
    pub fn equal_width(mut self, width: f32) -> Self {
        self.equal_width = Some(width);
        self
    }

    pub fn show(self, ui: &mut Ui, selected: usize) -> Option<usize> {
        let n = self.labels.len();
        let inner = self.size.connected_inner();
        let pressed_inner = inner * 0.5;
        let mut changed = None;
        // Exactly one button tall: `horizontal` starts from a shorter row
        // and pushes the buttons off the parent's centre line.
        let row = egui::vec2(ui.available_width(), self.size.height());
        ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for i in 0..n {
                let sel = i == selected;
                let l = if i == 0 || sel { shape::FULL } else { inner };
                let r = if i + 1 == n || sel { shape::FULL } else { inner };
                let rest = Radii::sides(l, r);
                let pressed = Radii::sides(
                    if i == 0 { shape::FULL } else { pressed_inner },
                    if i + 1 == n { shape::FULL } else { pressed_inner },
                );
                let response = if let Some(icons) = self.icons {
                    let mut b = IconButton::new(icons[i])
                        .size(self.size)
                        .style(super::button::Style::Tonal)
                        .selected(sel)
                        .tooltip(self.labels[i])
                        .radii(rest, pressed)
                        .id_salt((self.id, i));
                    if let Some(w) = self.equal_width {
                        let natural = b.natural_width();
                        b = b.width_delta(w - natural);
                    }
                    ui.add(b)
                } else {
                    let mut b = Button::new(self.labels[i])
                        .size(self.size)
                        .style(super::button::Style::Tonal)
                        .selected(sel)
                        .radii(rest, pressed)
                        .id_salt((self.id, i));
                    if let Some(w) = self.equal_width {
                        b = b.min_width(w);
                    }
                    ui.add(b)
                };
                if response.clicked() && !sel {
                    changed = Some(i);
                }
            }
        });
        changed
    }
}
