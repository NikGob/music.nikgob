//! A standard button group that also takes split buttons and wraps onto
//! further lines when the buttons do not fit: pressing a member widens it and
//! squeezes its neighbours on the same line (fast spatial), like
//! [`super::ButtonGroup`].

use egui::{Id, Response, Ui};

use super::{
    anim,
    button::{Button, IconButton, Size},
    split_button::SplitButton,
};
use crate::Theme;

/// How much a pressed member grows, as a fraction of its width.
const EXPAND_RATIO: f32 = 0.15;

/// One member of the row.
pub enum Member {
    Button(Button),
    Icon(IconButton),
    Split(SplitButton),
}

impl From<Button> for Member {
    fn from(b: Button) -> Self {
        Self::Button(b)
    }
}

impl From<IconButton> for Member {
    fn from(b: IconButton) -> Self {
        Self::Icon(b)
    }
}

impl From<SplitButton> for Member {
    fn from(b: SplitButton) -> Self {
        Self::Split(b)
    }
}

impl Member {
    fn natural_width(&self, ui: &Ui) -> f32 {
        match self {
            Self::Button(b) => b.natural_width(ui),
            Self::Icon(b) => b.natural_width(),
            Self::Split(b) => b.natural_width(ui),
        }
    }
}

/// What the row reported.
pub struct RowOutput {
    /// One per member, in order (a split button reports its action button).
    pub responses: Vec<Response>,
    /// (member index, menu item index) of a picked split-button item.
    pub picked: Option<(usize, usize)>,
}

/// The row.
pub struct ButtonRow {
    id: Id,
    size: Size,
}

impl ButtonRow {
    pub fn new(id_salt: impl egui::AsId, size: Size) -> Self {
        Self {
            id: Id::new(id_salt),
            size,
        }
    }

    pub fn show(self, ui: &mut Ui, members: Vec<Member>) -> RowOutput {
        let id = ui.id().with(self.id);
        let motion = Theme::get(ui.ctx()).motion;
        let gap = self.size.group_gap();
        let pressed: Option<usize> = ui.ctx().data(|d| d.get_temp(id.with("pressed")));
        let widths: Vec<f32> = members.iter().map(|m| m.natural_width(ui)).collect();
        let avail = ui.available_width();

        // Greedy line breaks on the natural widths.
        let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
        let mut used = 0.0;
        for (i, w) in widths.iter().enumerate() {
            let line = lines.last_mut().expect("one line");
            if !line.is_empty() && used + gap + w > avail {
                lines.push(vec![i]);
                used = *w;
            } else {
                used += if line.is_empty() { 0.0 } else { gap } + w;
                line.push(i);
            }
        }

        let mut deltas = vec![0.0; members.len()];
        if let Some(p) = pressed.filter(|&p| p < members.len()) {
            let line = lines.iter().find(|l| l.contains(&p)).expect("member on a line");
            let grow = widths[p] * EXPAND_RATIO;
            deltas[p] += grow;
            let neighbours: Vec<usize> = line.iter().copied().filter(|&j| j + 1 == p || j == p + 1).collect();
            for &j in &neighbours {
                deltas[j] -= grow / neighbours.len() as f32;
            }
        }

        let mut slots: Vec<Option<Member>> = members.into_iter().map(Some).collect();
        let mut responses: Vec<Option<Response>> = (0..slots.len()).map(|_| None).collect();
        let mut picked = None;
        let mut now_pressed = None;
        let old = ui.spacing().item_spacing;
        for line in &lines {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &i in line {
                    let d = anim::f32(ui.ctx(), id.with(("w", i)), deltas[i], motion.fast_spatial);
                    let r = match slots[i].take().expect("member") {
                        Member::Button(b) => ui.add(b.width_delta(d).id_salt((self.id, i))),
                        Member::Icon(b) => ui.add(b.width_delta(d).id_salt((self.id, i))),
                        Member::Split(b) => {
                            let o = b.width_delta(d).show(ui);
                            if let Some(p) = o.picked {
                                picked = Some((i, p));
                            }
                            if o.trigger.is_pointer_button_down_on() {
                                now_pressed = Some(i);
                            }
                            o.main
                        }
                    };
                    if r.is_pointer_button_down_on() {
                        now_pressed = Some(i);
                    }
                    responses[i] = Some(r);
                }
            });
            ui.spacing_mut().item_spacing = old;
        }
        ui.ctx().data_mut(|d| match now_pressed {
            Some(p) => {
                d.insert_temp(id.with("pressed"), p);
            }
            None => d.remove::<usize>(id.with("pressed")),
        });
        RowOutput {
            responses: responses.into_iter().map(|r| r.expect("shown")).collect(),
            picked,
        }
    }
}
