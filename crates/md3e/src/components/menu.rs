//! Expressive standard menu: a popup anchored to a widget (or opened at the
//! pointer as a context menu) on `surface-container-low`, radius 16, level 2.
//! Items have an optional leading icon, a label, a trailing shortcut and
//! dividers; the selected item is `tertiary-container`. Keyboard: Up/Down
//! (wrapping), Home/End, Enter/Space to pick, Esc to close.
//!
//! An item may hold a submenu ([`MenuItem::submenu`]): a trailing chevron,
//! and a second surface of the same look beside the menu, level with the
//! item. It opens when the pointer rests on the item, at once on a click,
//! Right or Enter; Left or Esc closes it alone, another item of the menu
//! closes it too, and an item picked in it closes the whole menu.

use egui::{
    Area, Color32, Context, Id, Key, LayerId, Modifiers, Order, Painter, Pos2, Rect, Response, Sense,
    Stroke, Ui, Vec2, WidgetInfo, WidgetType, pos2, vec2,
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
/// Wide enough for every label the app has: a menu grows to its longest
/// item, so no label is cut.
pub const MAX_WIDTH: f32 = 400.0;
const PAD: f32 = 4.0;
const DIVIDER_HEIGHT: f32 = 9.0;
/// A group's label above its items.
const LABEL_HEIGHT: f32 = 36.0;
/// Between a menu and its submenu.
const SUBMENU_GAP: f32 = 4.0;
/// How long the pointer rests on an item before its submenu opens (or the
/// open one of another item closes), in seconds.
const HOVER_DELAY: f64 = 0.25;

/// One selectable row.
#[derive(Clone, Debug)]
pub struct MenuItem {
    pub label: String,
    pub icon: Option<Icon>,
    pub shortcut: Option<String>,
    pub enabled: bool,
    pub selected: bool,
    submenu: Option<Vec<Entry>>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            shortcut: None,
            enabled: true,
            selected: false,
            submenu: None,
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

    /// Opens `child` beside the menu instead of being picked: its items and
    /// dividers (its id and alignment are not used). The row itself takes
    /// no index, the child's items take theirs in its place. One level: an
    /// item of `child` holds no submenu of its own.
    pub fn submenu(mut self, child: Menu) -> Self {
        debug_assert!(items_of(&child.entries).all(|i| i.submenu.is_none()));
        self.submenu = Some(child.entries);
        self
    }

    /// The indices the row takes: one, or one for each item of its submenu.
    fn leaves(&self) -> usize {
        self.submenu.as_ref().map_or(1, |c| items_of(c).count())
    }
}

fn items_of(entries: &[Entry]) -> impl Iterator<Item = &MenuItem> {
    entries.iter().filter_map(|e| match e {
        Entry::Item(i) => Some(i),
        Entry::Divider | Entry::Label(_) => None,
    })
}

/// The open submenu of a menu, kept between frames.
#[derive(Clone, Copy, Debug, Default)]
struct SubState {
    /// The row whose submenu is open.
    open: Option<usize>,
    /// The row last opened (its submenu is drawn while it fades out).
    shown: usize,
    /// The keys go to the submenu.
    focus: bool,
    /// The highlight in the submenu.
    hl: Option<usize>,
    /// The row under the pointer, since when, and whether the rest on it
    /// has been acted on.
    hover: Option<(usize, f64, bool)>,
}

/// What one surface of a menu saw this frame, by the place of its rows.
struct PanelOut {
    hovered: Option<usize>,
    clicked: Option<usize>,
    rects: Vec<Rect>,
}

#[derive(Clone, Debug)]
enum Entry {
    Item(MenuItem),
    Divider,
    /// A label over the items that follow (not an item: nothing to pick).
    Label(String),
}

/// Where the popup goes.
#[derive(Clone, Copy, Debug)]
enum Placement {
    /// Below (or above, if there is no room) a widget; `right` aligns the
    /// menu's right edge with the widget's.
    Anchored { rect: Rect, right: bool, center: bool },
    /// Top-left corner at a point (context menu).
    At(Pos2),
}

/// A menu. Item indices count only items, not dividers; a row with a
/// submenu is not counted, the items of its submenu are, in its place.
#[derive(Clone, Debug)]
#[must_use = "call .popup(..), .attach(..) or .context_menu(..)"]
pub struct Menu {
    id: Id,
    entries: Vec<Entry>,
    align_right: bool,
    align_center: bool,
}

impl Menu {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            entries: Vec::new(),
            align_right: false,
            align_center: false,
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

    /// A group label (Label Large, `on-surface-variant`) over the items
    /// after it; it is not an item and takes no index.
    pub fn group_label(mut self, text: impl Into<String>) -> Self {
        self.entries.push(Entry::Label(text.into()));
        self
    }

    /// Aligns the right edge with the anchor (for ⋮ buttons at the end).
    pub fn align_right(mut self, right: bool) -> Self {
        self.align_right = right;
        self
    }

    /// Centres the menu over the anchor (a control in the middle of a row).
    pub fn align_center(mut self, center: bool) -> Self {
        self.align_center = center;
        self
    }

    /// Where the items of the menu `id_salt` are on screen, while it is
    /// shown this pass (for help cards about the item under the pointer).
    /// An item of a submenu that is not open is nowhere (`Rect::NOTHING`).
    pub fn item_rects(ctx: &Context, id_salt: impl egui::AsId) -> Option<Vec<Rect>> {
        let key = Id::new(id_salt).with("item-rects");
        let pass = ctx.cumulative_pass_nr();
        ctx.data(|d| d.get_temp::<(u64, Vec<Rect>)>(key)).filter(|(p, _)| *p == pass).map(|(_, r)| r)
    }

    /// Where the rows of the menu `id_salt` itself are (its items and the
    /// rows that open a submenu, in order), while it is shown this pass.
    pub fn row_rects(ctx: &Context, id_salt: impl egui::AsId) -> Option<Vec<Rect>> {
        let key = Id::new(id_salt).with("row-rects");
        let pass = ctx.cumulative_pass_nr();
        ctx.data(|d| d.get_temp::<(u64, Vec<Rect>)>(key)).filter(|(p, _)| *p == pass).map(|(_, r)| r)
    }

    fn items(&self) -> impl Iterator<Item = &MenuItem> {
        items_of(&self.entries)
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
        let (right, center) = (self.align_right, self.align_center);
        let picked = self.show_at(
            &ctx,
            Placement::Anchored {
                rect: anchor.rect,
                right,
                center,
            },
            &mut open,
        );
        ctx.data_mut(|d| d.insert_temp(key, open));
        picked
    }

    /// A popup below `anchor`, open while `*open`. Clicking the anchor does
    /// not close it (the caller toggles `open` on anchor clicks).
    pub fn popup(self, ctx: &Context, anchor: Rect, open: &mut bool) -> Option<usize> {
        let (right, center) = (self.align_right, self.align_center);
        self.show_at(
            ctx,
            Placement::Anchored {
                rect: anchor,
                right,
                center,
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
        let hl_key = self.id.with("hl");
        let sub_key = self.id.with("sub-state");
        let sub_id = self.id.with("sub");
        let sub_area = self.id.with("sub-area");
        if !pres.visible() {
            common::layer_scale(ctx, layer, Pos2::ZERO, 1.0);
            common::layer_scale(ctx, LayerId::new(Order::Tooltip, sub_area), Pos2::ZERO, 1.0);
            common::presence(ctx, sub_id, false, motion.fast_spatial, motion.fast_effects);
            ctx.data_mut(|d| {
                d.remove::<Option<usize>>(hl_key);
                d.remove::<SubState>(sub_key);
            });
            return None;
        }
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| d.insert_temp(Id::new(SHOWN_KEY), pass));

        // Measure.
        let painter = ctx.layer_painter(layer);
        let size = measure(&painter, &self.entries, s.on_surface);
        let (menu_w, height) = (size.x, size.y);

        // Position.
        let screen = ctx.content_rect().shrink(4.0);
        let (mut top_left, pivot) = match placement {
            Placement::Anchored { rect, right, center } => {
                let x = if center {
                    rect.center().x - menu_w * 0.5
                } else if right {
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

        // The rows of the menu itself: the first index each takes, and the
        // submenu it opens.
        let mut next = 0;
        let rows: Vec<(usize, Option<&[Entry]>)> = self
            .items()
            .map(|item| {
                let row = (next, item.submenu.as_deref());
                next += item.leaves();
                row
            })
            .collect();
        let has_sub = |p: usize| rows.get(p).is_some_and(|r| r.1.is_some());
        let child_flags = |p: usize| -> Vec<bool> {
            rows.get(p).and_then(|r| r.1).map(|c| items_of(c).map(|i| i.enabled).collect()).unwrap_or_default()
        };
        let mut st: SubState = ctx.data(|d| d.get_temp(sub_key)).unwrap_or_default();
        if !was_open || st.open.is_some_and(|p| !has_sub(p)) {
            st.open = None;
            st.focus = false;
        }

        // Keyboard.
        let now = ctx.input(|i| i.time);
        let mut highlight: Option<usize> = ctx.data(|d| d.get_temp(hl_key)).flatten();
        let enabled_flags: Vec<bool> = self.items().map(|i| i.enabled).collect();
        let mut picked = None;
        let mut close = false;
        if was_open {
            ctx.input_mut(|i| {
                let down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
                let up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
                let home = i.consume_key(Modifiers::NONE, Key::Home);
                let end = i.consume_key(Modifiers::NONE, Key::End);
                let enter = i.consume_key(Modifiers::NONE, Key::Enter)
                    | i.consume_key(Modifiers::NONE, Key::Space);
                let esc = i.consume_key(Modifiers::NONE, Key::Escape);
                let step = |from: Option<usize>, flags: &[bool]| {
                    let mut h = from;
                    if down {
                        h = step_index(h, flags, 1);
                    }
                    if up {
                        h = step_index(h, flags, -1);
                    }
                    if home {
                        h = step_index(None, flags, 1);
                    }
                    if end {
                        h = step_index(None, flags, -1);
                    }
                    h
                };
                if let Some(p) = st.open.filter(|_| st.focus) {
                    // The keys are the submenu's; Left and Esc leave it.
                    let flags = child_flags(p);
                    st.hl = step(st.hl, &flags);
                    if enter && let Some(c) = st.hl.filter(|&c| flags.get(c) == Some(&true)) {
                        picked = Some(rows[p].0 + c);
                    }
                    if i.consume_key(Modifiers::NONE, Key::ArrowLeft) | esc {
                        st.open = None;
                        st.focus = false;
                    }
                } else {
                    if st.open.is_some() {
                        if i.consume_key(Modifiers::NONE, Key::ArrowLeft) | esc {
                            st.open = None;
                        }
                    } else if esc {
                        close = true;
                    }
                    let before = highlight;
                    highlight = step(highlight, &enabled_flags);
                    if highlight != before {
                        st.open = None;
                    }
                    let on = highlight.filter(|&h| enabled_flags.get(h) == Some(&true));
                    if let Some(h) = on {
                        if has_sub(h) {
                            if i.consume_key(Modifiers::NONE, Key::ArrowRight) | enter {
                                st.open = Some(h);
                                st.focus = true;
                                st.hl = step_index(None, &child_flags(h), 1);
                            }
                        } else if enter {
                            picked = Some(rows[h].0);
                        }
                    }
                }
            });
        }

        let alpha = pres.alpha();
        let id = self.id;
        let entries = &self.entries;
        let expanded = st.open;
        // While it is open a clear scrim lies under it over the window: a
        // press outside closes the menu and goes no further. It used to
        // land on what was under it too: a slider took the value at the
        // press, a switch flipped, a button acted.
        if was_open {
            let screen = ctx.content_rect();
            let scrim = Area::new(self.id.with("scrim"))
                .fade_in(false)
                .order(Order::Tooltip)
                .fixed_pos(screen.min)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| {
                    ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
                });
            let under = scrim.response.layer_id;
            ctx.move_to_top(under);
            ctx.set_sublayer(under, layer);
            ctx.set_sublayer(under, LayerId::new(Order::Tooltip, sub_area));
        }
        let area = Area::new(area_id)
            .fade_in(false)
            .order(Order::Tooltip)
            .fixed_pos(top_left)
            .constrain(false)
            .interactable(true)
            .show(ctx, |ui| {
                ui.set_opacity(alpha);
                panel(ui, size, entries, id, highlight, expanded)
            });
        common::layer_scale(ctx, area.response.layer_id, pivot, 0.9 + 0.1 * pres.spatial);
        let out = area.inner;

        // Pointer moves the highlight; resting on a row opens its submenu
        // and closes another row's; a click opens it at once.
        if let Some(h) = out.hovered {
            highlight = Some(h);
            if !st.hover.is_some_and(|(at, ..)| at == h) {
                st.hover = Some((h, now, false));
            }
            if was_open && let Some((_, since, false)) = st.hover {
                let left = HOVER_DELAY - (now - since);
                if left <= 0.0 {
                    st.hover = Some((h, since, true));
                    let want = has_sub(h).then_some(h);
                    if st.open != want {
                        st.open = want;
                        st.focus = false;
                        st.hl = None;
                    }
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
                }
            }
        } else {
            st.hover = None;
        }
        if was_open && let Some(p) = out.clicked {
            if has_sub(p) {
                st.open = Some(p);
                st.focus = true;
                st.hl = None;
                st.hover = st.hover.map(|(at, since, _)| (at, since, true));
            } else {
                picked = Some(rows[p].0);
            }
        }

        // The submenu.
        if let Some(p) = st.open {
            st.shown = p;
        }
        let sub_pres = common::presence(ctx, sub_id, st.open.is_some(), motion.fast_spatial, motion.fast_effects);
        let child = rows.get(st.shown).and_then(|r| r.1.map(|c| (r.0, c)));
        let mut child_rect = None;
        let mut child_rects = Vec::new();
        if let (true, Some((first, child)), Some(&row)) = (sub_pres.visible(), child, out.rects.get(st.shown)) {
            let child_size = measure(&painter, child, s.on_surface);
            let (pos, left) = submenu_pos(menu_rect, row, child_size, screen);
            let child_hl = st.hl.filter(|_| st.focus && st.open.is_some());
            let child_alpha = sub_pres.alpha().min(alpha);
            let area = Area::new(sub_area)
                .fade_in(false)
                .order(Order::Tooltip)
                .fixed_pos(pos)
                .constrain(false)
                .interactable(true)
                .show(ctx, |ui| {
                    ui.set_opacity(child_alpha);
                    panel(ui, child_size, child, sub_id, child_hl, None)
                });
            let edge = if left { pos.x + child_size.x } else { pos.x };
            common::layer_scale(ctx, area.response.layer_id, pos2(edge, row.center().y), 0.9 + 0.1 * sub_pres.spatial);
            if st.open.is_some() {
                if let Some(c) = area.inner.hovered {
                    st.hl = Some(c);
                    st.focus = true;
                }
                if let Some(c) = area.inner.clicked {
                    picked = Some(first + c);
                }
                child_rect = Some(Rect::from_min_size(pos, child_size));
                child_rects = area.inner.rects;
            }
        } else {
            common::layer_scale(ctx, LayerId::new(Order::Tooltip, sub_area), Pos2::ZERO, 1.0);
        }

        // Every item by its index: a row of the menu, or an item of the
        // open submenu.
        let mut item_rects = Vec::new();
        for (p, (_, sub)) in rows.iter().enumerate() {
            match sub {
                None => item_rects.push(out.rects.get(p).copied().unwrap_or(Rect::NOTHING)),
                Some(c) => {
                    let shown = st.open == Some(p);
                    for k in 0..items_of(c).count() {
                        item_rects.push(child_rects.get(k).copied().filter(|_| shown).unwrap_or(Rect::NOTHING));
                    }
                }
            }
        }
        let pass = ctx.cumulative_pass_nr();
        ctx.data_mut(|d| {
            d.insert_temp(id.with("item-rects"), (pass, item_rects));
            d.insert_temp(id.with("row-rects"), (pass, out.rects));
        });

        // A press outside closes (on its anchor too: the scrim takes the
        // press, the anchor never sees it to close the menu itself).
        if was_open {
            let outside_press = ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !menu_rect.contains(p) && !child_rect.is_some_and(|c| c.contains(p)))
            });
            if outside_press {
                close = true;
            }
        }
        if was_open && (picked.is_some() || close) {
            *open = false;
            st.open = None;
            st.focus = false;
        }
        ctx.data_mut(|d| {
            d.insert_temp(hl_key, highlight);
            d.insert_temp(sub_key, st);
        });
        picked
    }
}

/// The size of a surface that holds `entries`: as wide as its longest row.
fn measure(painter: &Painter, entries: &[Entry], ink: Color32) -> Vec2 {
    let label_style = TypeRole::BodyLarge.style();
    let short_style = TypeRole::LabelLarge.style();
    let mut content_w: f32 = 0.0;
    let mut height = 2.0 * PAD;
    for entry in entries {
        match entry {
            Entry::Item(item) => {
                let mut w = 12.0 + 12.0;
                if item.icon.is_some() {
                    w += 24.0 + 12.0;
                }
                w += common::plain(painter, &item.label, label_style, ink).size().x;
                if let Some(sc) = &item.shortcut {
                    w += 24.0 + common::plain(painter, sc, short_style, ink).size().x;
                }
                if item.submenu.is_some() {
                    w += 12.0 + 24.0;
                }
                content_w = content_w.max(w);
                height += ITEM_HEIGHT;
            }
            Entry::Divider => height += DIVIDER_HEIGHT,
            Entry::Label(text) => {
                content_w = content_w.max(24.0 + common::plain(painter, text, short_style, ink).size().x);
                height += LABEL_HEIGHT;
            }
        }
    }
    vec2(menu_width(content_w), height)
}

/// Where a submenu of `size` goes, and whether it is on the left: beside
/// `menu`, to the right or, when only the left has room, to the left; its
/// first row level with `row`; inside `screen`.
fn submenu_pos(menu: Rect, row: Rect, size: Vec2, screen: Rect) -> (Pos2, bool) {
    let right = menu.right() + SUBMENU_GAP;
    let left = menu.left() - SUBMENU_GAP - size.x;
    let flip = right + size.x > screen.right() && left >= screen.left();
    let x = if flip { left } else { right.min((screen.right() - size.x).max(screen.left())) };
    let y = (row.top() - PAD).clamp(screen.top(), (screen.bottom() - size.y).max(screen.top()));
    (pos2(x, y), flip)
}

/// Paints one surface (a menu or a submenu) of `size` and its rows. The
/// row `highlight` has the keyboard's layer, the row `expanded` its open
/// submenu.
fn panel(ui: &mut Ui, size: Vec2, entries: &[Entry], id: Id, highlight: Option<usize>, expanded: Option<usize>) -> PanelOut {
    let s = Theme::get(ui.ctx()).scheme;
    let mut out = PanelOut { hovered: None, clicked: None, rects: Vec::new() };
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let radii = Radii::all(shape::LARGE);
    paint::shadow(ui.painter(), rect, radii, Elevation::Level2);
    paint::container(ui.painter(), rect, radii, s.surface_container_low, Stroke::NONE);
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
            Entry::Label(text) => {
                let galley = common::plain(ui.painter(), text, TypeRole::LabelLarge.style(), s.on_surface_variant);
                let at = pos2(rect.left() + PAD + 12.0, y + (LABEL_HEIGHT - galley.size().y) * 0.5);
                ui.painter().galley(at, galley, s.on_surface_variant);
                y += LABEL_HEIGHT;
            }
            Entry::Item(item) => {
                let row = Rect::from_min_size(pos2(rect.left() + PAD, y), vec2(rect.width() - 2.0 * PAD, ITEM_HEIGHT));
                let sense = if item.enabled { Sense::click() } else { Sense::hover() };
                out.rects.push(row);
                let r = ui.interact(row, id.with(("item", index)), sense);
                if item.enabled && r.hovered() {
                    out.hovered = Some(index);
                }
                if item.enabled && r.clicked() {
                    out.clicked = Some(index);
                }
                let lit = highlight == Some(index) || expanded == Some(index);
                paint_item(ui, row, item, &r, lit, id.with(("item", index)));
                y += ITEM_HEIGHT;
                index += 1;
            }
        }
    }
    out
}

fn paint_item(ui: &Ui, row: Rect, item: &MenuItem, r: &Response, highlighted: bool, id: Id) {
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
    if item.submenu.is_some() {
        right -= 24.0;
        Icon::ChevronRight
            .widget(false, variant, 24.0)
            .paint_at(ui, Rect::from_center_size(pos2(right + 12.0, row.center().y), Vec2::splat(24.0)));
        right -= 12.0;
    }
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

/// The width of a menu whose longest item is `content_w` wide: a point to
/// spare, so the label measured is never cut when it is laid out again.
pub fn menu_width(content_w: f32) -> f32 {
    (content_w.ceil() + 1.0 + 2.0 * PAD).clamp(MIN_WIDTH, MAX_WIDTH)
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
    fn a_menu_fits_its_longest_item() {
        // «Не рекомендовать исполнителя» with its icon is about 330 points.
        assert!(menu_width(330.4) - 2.0 * PAD >= 330.4 + 0.5);
        assert_eq!(menu_width(10.0), MIN_WIDTH);
        assert_eq!(menu_width(900.0), MAX_WIDTH);
    }

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

    const KEY: &str = "submenu-test";

    /// «One», «More ▸» («Two» │ «Three»), «Four»: the indices 0, (1, 2), 3.
    fn cascade() -> Menu {
        let child = Menu::new("child").label("Two").divider().label("Three");
        Menu::new(KEY).label("One").item(MenuItem::new("More").submenu(child)).label("Four")
    }

    /// A menu shown frame after frame, and where it was last frame.
    struct Shown {
        ctx: Context,
        time: f64,
        open: bool,
        rows: Vec<Rect>,
        items: Vec<Rect>,
    }

    impl Shown {
        fn new() -> Self {
            let ctx = Context::default();
            let mut fonts = egui::FontDefinitions::default();
            crate::typography::install_fonts(&mut fonts);
            ctx.set_fonts(fonts);
            crate::icons::install(&ctx);
            Theme::default().install(&ctx);
            let mut shown = Self { ctx, time: 0.0, open: true, rows: Vec::new(), items: Vec::new() };
            shown.settle();
            shown
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> Option<usize> {
            self.time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let ctx = self.ctx.clone();
            let anchor = Rect::from_min_size(pos2(100.0, 100.0), vec2(40.0, 40.0));
            let (open, rows, items) = (&mut self.open, &mut self.rows, &mut self.items);
            let mut picked = None;
            let mut out = self.ctx.run_ui(input, |_| {
                picked = cascade().popup(&ctx, anchor, open);
                *rows = Menu::row_rects(&ctx, KEY).unwrap_or_default();
                *items = Menu::item_rects(&ctx, KEY).unwrap_or_default();
            });
            out.textures_delta.clear();
            picked
        }

        /// Long enough for a surface to open or close.
        fn settle(&mut self) {
            for _ in 0..24 {
                assert_eq!(self.frame(Vec::new()), None);
            }
        }

        fn click(&mut self, at: Pos2) -> Option<usize> {
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            let mut picked = self.frame(vec![egui::Event::PointerMoved(at)]);
            picked = picked.or(self.frame(Vec::new()));
            picked = picked.or(self.frame(vec![button(true)]));
            picked = picked.or(self.frame(vec![button(false)]));
            picked.or(self.frame(Vec::new()))
        }

        fn key(&mut self, key: Key) -> Option<usize> {
            let event = egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
            let mut picked = self.frame(vec![event]);
            for _ in 0..3 {
                picked = picked.or(self.frame(Vec::new()));
            }
            picked
        }

        /// The submenu is on screen.
        fn child(&self) -> bool {
            self.items.get(1).is_some_and(|r| r.is_positive())
        }
    }

    #[test]
    fn a_submenu_opens_on_a_click_and_its_item_closes_the_menu() {
        let mut m = Shown::new();
        assert_eq!((m.rows.len(), m.items.len()), (3, 4));
        assert!(!m.child());
        // The row that opens it is not an item: nothing is picked.
        assert_eq!(m.click(m.rows[1].center()), None);
        m.settle();
        assert!(m.open && m.child());
        // Beside the menu, level with its row.
        assert!(m.items[1].left() > m.rows[1].right());
        assert!((m.items[1].top() - m.rows[1].top()).abs() < 0.5);
        assert_eq!(m.click(m.items[2].center()), Some(2));
        assert!(!m.open);
    }

    /// A press outside an open menu closes it and goes no further: the
    /// widget under it is not clicked (a slider under a menu took the value
    /// at the press that closed it). With the menu closed it is.
    #[test]
    fn a_press_that_closes_a_menu_does_not_reach_what_is_under_it() {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        let target = Rect::from_min_size(pos2(600.0, 500.0), vec2(120.0, 40.0));
        let anchor = Rect::from_min_size(pos2(100.0, 100.0), vec2(40.0, 40.0));
        let mut open = true;
        let mut time = 0.0;
        let mut frame = |events: Vec<egui::Event>, open: &mut bool| -> bool {
            time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut clicked = false;
            let ctx2 = ctx.clone();
            let mut out = ctx.run_ui(input, |ui| {
                clicked = ui.interact(target, Id::new("under"), Sense::click()).clicked();
                cascade().popup(&ctx2, anchor, open);
            });
            out.textures_delta.clear();
            clicked
        };
        for _ in 0..24 {
            frame(Vec::new(), &mut open);
        }
        let at = target.center();
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        let click = |frame: &mut dyn FnMut(Vec<egui::Event>, &mut bool) -> bool, open: &mut bool| {
            let mut hit = frame(vec![egui::Event::PointerMoved(at)], open);
            hit |= frame(Vec::new(), open);
            hit |= frame(vec![button(true)], open);
            hit |= frame(vec![button(false)], open);
            hit | frame(Vec::new(), open)
        };
        assert!(open);
        assert!(!click(&mut frame, &mut open), "the press that closed the menu clicked what was under it");
        assert!(!open, "it closed");
        for _ in 0..24 {
            frame(Vec::new(), &mut open);
        }
        assert!(click(&mut frame, &mut open), "closed, a click reaches it");
    }

    #[test]
    fn the_rows_around_a_submenu_keep_their_indices() {
        let mut m = Shown::new();
        assert_eq!(m.click(m.rows[2].center()), Some(3));
        assert!(!m.open);
    }

    #[test]
    fn esc_closes_the_submenu_alone() {
        let mut m = Shown::new();
        m.click(m.rows[1].center());
        m.settle();
        assert!(m.child());
        assert_eq!(m.key(Key::Escape), None);
        assert!(m.open && !m.child());
        m.key(Key::Escape);
        assert!(!m.open);
    }

    #[test]
    fn the_keys_walk_into_a_submenu_and_back() {
        let mut m = Shown::new();
        m.key(Key::ArrowDown);
        m.key(Key::ArrowDown);
        m.key(Key::ArrowRight);
        assert!(m.open && m.child());
        m.key(Key::ArrowLeft);
        assert!(m.open && !m.child());
        // Enter opens it too; Down, Enter picks its second item.
        assert_eq!(m.key(Key::Enter), None);
        assert!(m.child());
        m.key(Key::ArrowDown);
        assert_eq!(m.key(Key::Enter), Some(2));
        assert!(!m.open);
    }

    #[test]
    fn a_submenu_goes_left_when_the_right_has_no_room() {
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0));
        let size = vec2(200.0, 300.0);
        let row = |menu: Rect, y: f32| Rect::from_min_size(pos2(menu.left() + PAD, y), vec2(menu.width() - 2.0 * PAD, ITEM_HEIGHT));
        let menu = Rect::from_min_size(pos2(100.0, 100.0), vec2(200.0, 400.0));
        let (at, left) = submenu_pos(menu, row(menu, 200.0), size, screen);
        assert_eq!((at, left), (pos2(menu.right() + SUBMENU_GAP, 200.0 - PAD), false));
        let menu = Rect::from_min_size(pos2(780.0, 100.0), vec2(200.0, 400.0));
        let (at, left) = submenu_pos(menu, row(menu, 200.0), size, screen);
        assert_eq!((at.x, left), (menu.left() - SUBMENU_GAP - size.x, true));
        // Near the bottom it stays on screen.
        let (at, _) = submenu_pos(menu, row(menu, 450.0), size, screen);
        assert_eq!(at.y, screen.bottom() - size.y);
    }
}
