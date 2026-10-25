//! Expressive standard menu: a popup anchored to a widget (or opened at the
//! pointer as a context menu) on `surface-container-low`, radius 16, level 2.
//! Items have an optional leading icon, a label, a trailing shortcut and
//! dividers; the selected item is `tertiary-container`. Keyboard: Up/Down
//! (wrapping), Home/End, Enter/Space to pick, Esc to close.

use egui::{
    Area, Context, Id, Key, LayerId, Modifiers, Order, Pos2, Rect, Response, Sense, Stroke, Vec2,
    WidgetInfo, WidgetType, pos2, vec2,
};

use super::{anim, common, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape, state},
};

pub const ITEM_HEIGHT: f32 = 48.0;
/// The pass a menu was last on screen in (see [`Menu::shown_lately`]).
const SHOWN_KEY: &str = "md3e-menu-shown";
pub const MIN_WIDTH: f32 = 112.0;
pub const MAX_WIDTH: f32 = 280.0;
const PAD: f32 = 4.0;
const DIVIDER_HEIGHT: f32 = 9.0;

/// One selectable row.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub icon: Option<Icon>,
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub selected: bool,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            shortcut: None,
            enabled: true,
            selected: false,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The selected item (`tertiary-container`).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

#[derive(Clone, Debug)]
enum Entry {
    Item(MenuItem),
    Divider,
}

/// Where the popup goes.
#[derive(Clone, Copy, Debug)]
enum Placement {
    /// Below (or above, if there is no room) a widget; `right` aligns the
    /// menu's right edge with the widget's.
    Anchored { rect: Rect, right: bool },
    /// Top-left corner at a point (context menu).
    At(Pos2),
}

/// A menu. Item indices count only items, not dividers.
#[must_use = "call .popup(..), .attach(..) or .context_menu(..)"]
pub struct Menu {
    id: Id,
    entries: Vec<Entry>,
    align_right: bool,
}

impl Menu {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            entries: Vec::new(),
            align_right: false,
        }
    }

    pub fn item(mut self, item: MenuItem) -> Self {
        self.entries.push(Entry::Item(item));
        self
    }

    /// Shorthand for a plain item with a label.
    pub fn label(self, label: impl Into<String>) -> Self {
        self.item(MenuItem::new(label))
    }

    pub fn divider(mut self) -> Self {
        self.entries.push(Entry::Divider);
        self
    }

    /// Aligns the right edge with the anchor (for ⋮ buttons at the end).
    pub fn align_right(mut self, right: bool) -> Self {
        self.align_right = right;
        self
    }

    /// Where the items of the menu `id_salt` are on screen, while it is
    /// shown this pass (for help cards about the item under the pointer).
    pub fn item_rects(ctx: &Context, id_salt: impl egui::AsId) -> Option<Vec<Rect>> {
        let key = Id::new(id_salt).with("item-rects");
        let pass = ctx.cumulative_pass_nr();
        ctx.data(|d| d.get_temp::<(u64, Vec<Rect>)>(key)).filter(|(p, _)| *p == pass).map(|(_, r)| r)
    }

    fn items(&self) -> impl Iterator<Item = &MenuItem> {
        self.entries.iter().filter_map(|e| match e {
            Entry::Item(i) => Some(i),
            Entry::Divider => None,
        })
    }

    /// Toggled by clicks on `anchor` (right-aligned with `align_right`).
    /// Returns the index of the item that was picked.
    pub fn attach(self, anchor: &Response) -> Option<usize> {
        let ctx = anchor.ctx.clone();
        let key = anchor.id.with("md3e-menu-open");
        let mut open = ctx.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
        if anchor.clicked() {
            open = !open;
        }
        let right = self.align_right;
        let picked = self.show_at(
            &ctx,
            Placement::Anchored {
                rect: anchor.rect,
                right,
            },
            &mut open,
        );
        ctx.data_mut(|d| d.insert_temp(key, open));
        picked
    }

    /// A popup below `anchor`, open while `*open`. Clicking the anchor does
    /// not close it (the caller toggles `open` on anchor clicks).
    pub fn popup(self, ctx: &Context, anchor: Rect, open: &mut bool) -> Option<usize> {
        let right = self.align_right;
        self.show_at(
            ctx,
            Placement::Anchored {
                rect: anchor,
                right,
            },
            open,
        )
    }

    /// A menu was on screen this pass or the one before (long lists then
    /// lay out every row, so the row a menu belongs to never goes away
    /// under it).
    pub fn shown_lately(ctx: &Context) -> bool {
        let pass = ctx.cumulative_pass_nr();
        ctx.data(|d| d.get_temp::<u64>(Id::new(SHOWN_KEY))).is_some_and(|at| at + 1 >= pass)
    }

    /// Whether the context menu `id_salt` of `response` has nothing to show
    /// this frame: closed, not opened by this click, its exit done. Its
    /// items need not be built then (lists build one menu per row).
    pub fn context_idle(response: &Response, id_salt: impl egui::AsId) -> bool {
        if response.secondary_clicked() {
            return false;
        }
        let id = Id::new(id_salt);
        response.ctx.data(|d| {
            let open = d
                .get_temp::<(bool, Pos2)>(response.id.with("md3e-context-menu"))
                .is_some_and(|(open, _)| open);
            !open
                && d.get_temp::<nikgob_motion::Animated<f32>>(id.with("p-spatial")).is_none()
                && d.get_temp::<nikgob_motion::Animated<f32>>(id.with("p-effects")).is_none()
        })
    }

    /// Opens at the pointer on a right click on `response`.
    pub fn context_menu(self, response: &Response) -> Option<usize> {
        let ctx = response.ctx.clone();
        let key = response.id.with("md3e-context-menu");
        let (mut open, mut pos): (bool, Pos2) =
            ctx.data(|d| d.get_temp(key)).unwrap_or((false, Pos2::ZERO));
        if response.secondary_clicked() {
            open = true;
            pos = response
                .interact_pointer_pos()
                .or_else(|| ctx.pointer_latest_pos())
                .unwrap_or(response.rect.center());
        }
        let picked = self.show_at(&ctx, Placement::At(pos), &mut open);
        ctx.data_mut(|d| d.insert_temp(key, (open, pos)));
        picked
    }

    fn show_at(self, ctx: &Context, placement: Placement, open: &mut bool) -> Option<usize> {
        let theme = Theme::get(ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let was_open = *open;
        let area_id = self.id.with("area");
        let layer = LayerId::new(Order::Tooltip, area_id);
        let pres = common::presence(
            ctx,
            self.id,
            was_open,
            motion.fast_spatial,
            motion.fast_effects,
        );
        if !pres.visible() {
            common::layer_scale(ctx, layer, Pos2::ZERO, 1.0);
            ctx.data_mut(|d| d.remove::<Option<usize>>(self.id.with("hl")));
            return None;
        }
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(Id::new(SHOWN_KEY), pass));

        // Measure.
        let painter = ctx.layer_painter(layer);
        let label_style = TypeRole::BodyLarge.style();
        let short_style = TypeRole::LabelLarge.style();
        let mut content_w: f32 = 0.0;
        for item in self.items() {
            let mut w = 12.0 + 12.0;
            if item.icon.is_some() {
                w += 24.0 + 12.0;
            }
            w += common::plain(&painter, &item.label, label_style, s.on_surface)
                .size()
                .x;
            if let Some(sc) = &item.shortcut {
                w += 24.0
                    + common::plain(&painter, sc, short_style, s.on_surface)
                        .size()
                        .x;
            }
            content_w = content_w.max(w);
        }
        let menu_w = (content_w + 2.0 * PAD).clamp(MIN_WIDTH, MAX_WIDTH);
        let height: f32 = 2.0 * PAD
            + self
                .entries
                .iter()
                .map(|e| match e {
                    Entry::Item(_) => ITEM_HEIGHT,
                    Entry::Divider => DIVIDER_HEIGHT,
                })
                .sum::<f32>();
        let size = vec2(menu_w, height);

        // Position.
        let screen = ctx.content_rect().shrink(4.0);
        let (mut top_left, pivot) = match placement {
            Placement::Anchored { rect, right } => {
                let x = if right {
                    rect.right() - menu_w
                } else {
                    rect.left()
                };
                let below = pos2(x, rect.bottom() + 4.0);
                if below.y + height > screen.bottom() && rect.top() - 4.0 - height >= screen.top() {
                    (pos2(x, rect.top() - 4.0 - height), pos2(x, rect.top()))
                } else {
                    (below, pos2(x, rect.bottom()))
                }
            }
            Placement::At(p) => (p, p),
        };
        top_left.x = top_left
            .x
            .clamp(screen.left(), (screen.right() - menu_w).max(screen.left()));
        top_left.y = top_left
            .y
            .clamp(screen.top(), (screen.bottom() - height).max(screen.top()));
        let menu_rect = Rect::from_min_size(top_left, size);

        // Keyboard.
        let hl_key = self.id.with("hl");
        let mut highlight: Option<usize> = ctx.data(|d| d.get_temp(hl_key)).flatten();
        let enabled_flags: Vec<bool> = self.items().map(|i| i.enabled).collect();
        let mut picked = None;
        let mut close = false;
        if was_open {
            ctx.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    highlight = step_index(highlight, &enabled_flags, 1);
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    highlight = step_index(highlight, &enabled_flags, -1);
                }
                if i.consume_key(Modifiers::NONE, Key::Home) {
                    highlight = step_index(None, &enabled_flags, 1);
                }
                if i.consume_key(Modifiers::NONE, Key::End) {
                    highlight = step_index(None, &enabled_flags, -1);
                }
                let enter = i.consume_key(Modifiers::NONE, Key::Enter)
                    | i.consume_key(Modifiers::NONE, Key::Space);
                if enter && let Some(h) = highlight.filter(|&h| enabled_flags.get(h) == Some(&true))
                {
                    picked = Some(h);
                }
                if i.consume_key(Modifiers::NONE, Key::Escape) {
                    close = true;
                }
            });
        }

        let alpha = pres.alpha();
        let id = self.id;
        let entries = &self.entries;
        let mut hovered_item = None;
        let mut item_rects = Vec::new();
        let area = Area::new(area_id)
            .fade_in(false)
            .order(Order::Tooltip)
            .fixed_pos(top_left)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let radii = Radii::all(shape::LARGE);
                paint::shadow(ui.painter(), rect, radii, Elevation::Level2);
                paint::container(
                    ui.painter(),
                    rect,
                    radii,
                    s.surface_container_low,
                    Stroke::NONE,
                );
                let mut y = rect.top() + PAD;
                let mut index = 0;
                for entry in entries {
                    match entry {
                        Entry::Divider => {
                            let cy = y + DIVIDER_HEIGHT * 0.5;
                            ui.painter().line_segment(
                                [pos2(rect.left() + PAD, cy), pos2(rect.right() - PAD, cy)],
                                Stroke::new(1.0, s.outline_variant),
                            );
                            y += DIVIDER_HEIGHT;
                        }
                        Entry::Item(item) => {
                            let row = Rect::from_min_size(
                                pos2(rect.left() + PAD, y),
                                vec2(rect.width() - 2.0 * PAD, ITEM_HEIGHT),
                            );
                            let sense = if item.enabled {
                                Sense::click()
                            } else {
                                Sense::hover()
                            };
                            item_rects.push(row);
                            let r = ui.interact(row, id.with(("item", index)), sense);
                            if item.enabled && r.hovered() {
                                hovered_item = Some(index);
                            }
                            if item.enabled && r.clicked() {
                                picked = Some(index);
                            }
                            paint_item(
                                ui,
                                row,
                                item,
                                &r,
                                highlight == Some(index),
                                id.with(("item", index)),
                            );
                            y += ITEM_HEIGHT;
                            index += 1;
                        }
                    }
                }
            });
        common::layer_scale(ctx, area.response.layer_id, pivot, 0.9 + 0.1 * pres.spatial);

        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(id.with("item-rects"), (pass, item_rects)));

        // Pointer moves the highlight; a press outside closes.
        if let Some(h) = hovered_item {
            highlight = Some(h);
        }
        if was_open {
            let anchor_rect = match placement {
                Placement::Anchored { rect, .. } => Some(rect),
                Placement::At(_) => None,
            };
            let outside_press = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer.interact_pos().is_some_and(|p| {
                        !menu_rect.contains(p) && !anchor_rect.is_some_and(|a| a.contains(p))
                    })
            });
            if outside_press {
                close = true;
            }
        }
        ctx.data_mut(|d| d.insert_temp(hl_key, highlight));
        if was_open && (picked.is_some() || close) {
            *open = false;
        }
        picked
    }
}

fn paint_item(ui: &egui::Ui, row: Rect, item: &MenuItem, r: &Response, highlighted: bool, id: Id) {
    let ctx = ui.ctx();
    let theme = Theme::get(ctx);
    let (s, motion) = (theme.scheme, theme.motion);
    let radii = Radii::all(shape::MEDIUM);
    let (content, variant) = if !item.enabled {
        let c = common::fade(s.on_surface, state::DISABLED_CONTENT);
        (c, c)
    } else if item.selected {
        (s.on_tertiary_container, s.on_tertiary_container)
    } else {
        (s.on_surface, s.on_surface_variant)
    };
    let bg = anim::color(
        ctx,
        id.with("bg"),
        if item.selected {
            s.tertiary_container
        } else {
            common::clear(s.tertiary_container)
        },
        motion.fast_effects,
    );
    paint::container(ui.painter(), row, radii, bg, Stroke::NONE);
    let target = if !item.enabled {
        0.0
    } else if r.is_pointer_button_down_on() {
        state::PRESSED
    } else if highlighted {
        state::FOCUS
    } else {
        0.0
    };
    let layer = anim::fraction(ctx, id.with("hl-layer"), target, motion.fast_effects);
    paint::state_layer(ui.painter(), row, radii, content, layer);

    let mut x = row.left() + 12.0;
    if let Some(icon) = item.icon {
        icon.widget(item.selected, variant, 24.0).paint_at(
            ui,
            Rect::from_center_size(pos2(x + 12.0, row.center().y), Vec2::splat(24.0)),
        );
        x += 24.0 + 12.0;
    }
    let mut right = row.right() - 12.0;
    if let Some(sc) = &item.shortcut {
        let g = common::plain(ui.painter(), sc, TypeRole::LabelLarge.style(), variant);
        right -= g.size().x;
        ui.painter()
            .galley(pos2(right, row.center().y - g.size().y * 0.5), g, variant);
        right -= 24.0;
    }
    let g = common::line(
        ui.painter(),
        &item.label,
        TypeRole::BodyLarge.style(),
        content,
        right - x,
    );
    ui.painter()
        .galley(pos2(x, row.center().y - g.size().y * 0.5), g, content);
    let (label, enabled, sel) = (item.label.clone(), item.enabled, item.selected);
    r.widget_info(|| WidgetInfo::selected(WidgetType::Button, enabled, sel, &label));
}

/// Moves a menu highlight over the enabled items, wrapping around. `None`
/// starts from the top (`dir > 0`) or the bottom (`dir < 0`). Returns `None`
/// when nothing is enabled.
pub fn step_index(current: Option<usize>, enabled: &[bool], dir: i32) -> Option<usize> {
    let n = enabled.len();
    if n == 0 || !enabled.iter().any(|&e| e) {
        return None;
    }
    let forward = dir >= 0;
    let mut i = match (current, forward) {
        (Some(c), _) if c < n => c,
        (_, true) => n - 1,
        (_, false) => 0,
    };
    for _ in 0..n {
        i = if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        };
        if enabled[i] {
            return Some(i);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_wraps_and_skips_disabled() {
        let en = [true, false, true, true];
        assert_eq!(step_index(None, &en, 1), Some(0));
        assert_eq!(step_index(None, &en, -1), Some(3));
        assert_eq!(step_index(Some(0), &en, 1), Some(2));
        assert_eq!(step_index(Some(3), &en, 1), Some(0));
        assert_eq!(step_index(Some(0), &en, -1), Some(3));
        assert_eq!(step_index(Some(2), &en, -1), Some(0));
        assert_eq!(step_index(Some(9), &en, 1), Some(0));
        assert_eq!(step_index(Some(0), &[false, false], 1), None);
        assert_eq!(step_index(None, &[], 1), None);
        assert_eq!(step_index(Some(0), &[true], 1), Some(0));
    }
}
