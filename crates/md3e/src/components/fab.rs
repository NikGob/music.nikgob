//! Floating action buttons: `Fab` / extended FAB (primary-container,
//! level 3, level 4 on hover) and `FabMenu`, where the FAB morphs into a 56
//! dp round close button and up to six 56 dp pills that appear one after
//! another (fast spatial + fast effects). Click-away, Esc or the close
//! button closes it; there is no scrim.

use egui::{
    Area, Color32, Context, Id, Key, LayerId, Modifiers, Order, Rect, Response, Sense, Stroke, Ui,
    Vec2, Widget, WidgetInfo, WidgetType, pos2, vec2,
};

use super::{anim, button::hit_rect, common, paint, paint::Radii};
use crate::{
    ColorScheme, Icon, Theme, TypeRole,
    tokens::{Elevation, shape},
};

/// Container / content pairs a FAB may use.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum FabColor {
    #[default]
    Primary,
    Secondary,
    Tertiary,
}

impl FabColor {
    /// (container, on-container, solid, on-solid)
    fn roles(self, s: &ColorScheme) -> (Color32, Color32, Color32, Color32) {
        match self {
            Self::Primary => (
                s.primary_container,
                s.on_primary_container,
                s.primary,
                s.on_primary,
            ),
            Self::Secondary => (
                s.secondary_container,
                s.on_secondary_container,
                s.secondary,
                s.on_secondary,
            ),
            Self::Tertiary => (
                s.tertiary_container,
                s.on_tertiary_container,
                s.tertiary,
                s.on_tertiary,
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum FabSize {
    /// 56, icon 24, radius 16.
    #[default]
    Standard,
    /// 80, icon 28, radius 20.
    Medium,
    /// 96, icon 32, radius 28.
    Large,
}

impl FabSize {
    pub const fn height(self) -> f32 {
        match self {
            Self::Standard => 56.0,
            Self::Medium => 80.0,
            Self::Large => 96.0,
        }
    }

    pub const fn icon(self) -> f32 {
        match self {
            Self::Standard => 24.0,
            Self::Medium => 28.0,
            Self::Large => 32.0,
        }
    }

    pub const fn radius(self) -> f32 {
        match self {
            Self::Standard => shape::LARGE,
            Self::Medium => shape::LARGE_INCREASED,
            Self::Large => shape::EXTRA_LARGE,
        }
    }

    /// Side padding of the extended form.
    const fn padding(self) -> f32 {
        match self {
            Self::Standard => 16.0,
            Self::Medium => 26.0,
            Self::Large => 28.0,
        }
    }

    const fn gap(self) -> f32 {
        match self {
            Self::Standard => 8.0,
            Self::Medium => 16.0,
            Self::Large => 20.0,
        }
    }

    fn label(self) -> TypeRole {
        match self {
            Self::Standard => TypeRole::LabelLarge,
            Self::Medium => TypeRole::TitleMedium,
            Self::Large => TypeRole::HeadlineSmall,
        }
    }
}

/// A FAB; with a label it is an extended FAB.
#[must_use = "add it with ui.add(fab)"]
pub struct Fab {
    icon: Icon,
    label: Option<String>,
    size: FabSize,
    color: FabColor,
    id_salt: Option<Id>,
}

impl Fab {
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            label: None,
            size: FabSize::Standard,
            color: FabColor::Primary,
            id_salt: None,
        }
    }

    /// Makes it an extended FAB.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn size(mut self, size: FabSize) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: FabColor) -> Self {
        self.color = color;
        self
    }

    pub fn id_salt(mut self, salt: impl egui::AsId) -> Self {
        self.id_salt = Some(Id::new(salt));
        self
    }
}

fn extended_width(ui: &Ui, size: FabSize, label: &str) -> f32 {
    let g = common::plain(ui.painter(), label, size.label().style(), Color32::WHITE);
    2.0 * size.padding() + size.icon() + size.gap() + g.size().x
}

impl Widget for Fab {
    fn ui(self, ui: &mut Ui) -> Response {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let s = theme.scheme;
        let (container, on, _, _) = self.color.roles(&s);
        let h = self.size.height();
        let w = self
            .label
            .as_deref()
            .map_or(h, |l| extended_width(ui, self.size, l));
        let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |salt| ui.id().with(salt));
        let response = ui.interact(hit_rect(rect), id, Sense::click());
        if ui.is_rect_visible(rect) {
            paint_fab(
                ui,
                id,
                rect,
                &response,
                self.size,
                container,
                on,
                self.icon,
                self.label.as_deref(),
                0.0,
            );
        }
        let label = self
            .label
            .clone()
            .unwrap_or_else(|| self.icon.name().to_owned());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
        response
    }
}

/// Paints a FAB at `rect` (its current, possibly morphing rect).
/// `morph` 0..1 blends the extended label away.
#[allow(clippy::too_many_arguments)]
fn paint_fab(
    ui: &Ui,
    id: Id,
    rect: Rect,
    r: &Response,
    size: FabSize,
    container: Color32,
    on: Color32,
    icon: Icon,
    label: Option<&str>,
    morph: f32,
) {
    let ctx = ui.ctx();
    let radii = Radii::all(size.radius());
    let elevation = if r.hovered() {
        Elevation::Level4
    } else {
        Elevation::Level3
    };
    paint::shadow(ui.painter(), rect, radii, elevation);
    paint::container(ui.painter(), rect, radii, container, Stroke::NONE);
    let layer = common::layer_opacity(ctx, id, r, true);
    paint::state_layer(ui.painter(), rect, radii, on, layer);
    let icon_c = pos2(
        rect.left() + size.padding().min(rect.width() * 0.5) + size.icon() * 0.5,
        rect.center().y,
    );
    let icon_c = if label.is_none() {
        rect.center()
    } else {
        icon_c
    };
    icon.widget(false, on, size.icon())
        .paint_at(ui, Rect::from_center_size(icon_c, Vec2::splat(size.icon())));
    if let Some(label) = label {
        let a = (1.0 - morph).clamp(0.0, 1.0);
        if a > 0.01 {
            let g = common::plain(
                ui.painter(),
                label,
                size.label().style(),
                common::fade(on, a),
            );
            let painter = ui.painter().with_clip_rect(rect);
            painter.galley(
                pos2(
                    rect.left() + size.padding() + size.icon() + size.gap(),
                    rect.center().y - g.size().y * 0.5,
                ),
                g,
                on,
            );
        }
    }
    if common::focused(r) {
        paint::focus_ring(ui.painter(), rect, radii, Theme::get(ctx).scheme.secondary);
    }
}

/// One pill in a FAB menu.
#[derive(Clone, Debug)]
pub struct FabMenuItem {
    pub icon: Icon,
    pub label: String,
}

impl FabMenuItem {
    pub fn new(icon: Icon, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
        }
    }
}

/// What a FAB menu reported.
pub struct FabMenuOutput {
    /// The FAB / close button.
    pub response: Response,
    pub open: bool,
    /// The item that was picked (the menu closes).
    pub clicked: Option<usize>,
}

/// A FAB that opens into a menu of up to six pills.
#[must_use = "call .show(ui)"]
pub struct FabMenu<'a> {
    id: Id,
    icon: Icon,
    label: Option<String>,
    items: &'a [FabMenuItem],
    color: FabColor,
    upwards: bool,
    align_right: bool,
}

const ITEM_H: f32 = 56.0;
const ITEM_GAP: f32 = 4.0;
const STAGGER: f64 = 0.04;

impl<'a> FabMenu<'a> {
    pub fn new(id_salt: impl egui::AsId, icon: Icon, items: &'a [FabMenuItem]) -> Self {
        Self {
            id: Id::new(id_salt),
            icon,
            label: None,
            items: &items[..items.len().min(6)],
            color: FabColor::Primary,
            upwards: false,
            align_right: false,
        }
    }

    /// The closed form is an extended FAB with this label.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn color(mut self, color: FabColor) -> Self {
        self.color = color;
        self
    }

    /// Items stack above the FAB (a FAB at the bottom of the screen).
    /// Default: below it (a FAB at the top of the rail).
    pub fn upwards(mut self, up: bool) -> Self {
        self.upwards = up;
        self
    }

    /// Items align with the FAB's right edge.
    pub fn align_right(mut self, right: bool) -> Self {
        self.align_right = right;
        self
    }

    /// Open state can be driven from outside (for example a shortcut).
    pub fn set_open(ctx: &Context, id_salt: impl egui::AsId, open: bool) {
        let id = Id::new(id_salt);
        ctx.data_mut(|d| d.insert_temp(id.with("open"), open));
        ctx.request_repaint();
    }

    pub fn show(self, ui: &mut Ui) -> FabMenuOutput {
        let ctx = ui.ctx().clone();
        let theme = Theme::get(&ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let id = self.id;
        let (container, on, solid, on_solid) = self.color.roles(&s);
        let size = FabSize::Standard;
        let now = anim::now(&ctx);

        let closed_w = self
            .label
            .as_deref()
            .map_or(56.0, |l| extended_width(ui, size, l));
        let (slot, _) = ui.allocate_exact_size(vec2(closed_w, 56.0), Sense::hover());

        let open_key = id.with("open");
        let mut open = ctx.data(|d| d.get_temp::<bool>(open_key)).unwrap_or(false);

        // Morph of the main button.
        let k = anim::fraction(
            &ctx,
            id.with("k"),
            f32::from(u8::from(open)),
            motion.fast_spatial,
        );
        let w = closed_w + (56.0 - closed_w) * k;
        let vis = Rect::from_min_size(
            if self.align_right {
                pos2(slot.right() - w, slot.top())
            } else {
                slot.min
            },
            vec2(w, 56.0),
        );
        let fab_id = id.with("fab");
        let response = ui.interact(hit_rect(vis), fab_id, Sense::click());
        if response.clicked() {
            open = !open;
            ctx.data_mut(|d| d.insert_temp(id.with("t"), now));
        }
        if open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            open = false;
            ctx.data_mut(|d| d.insert_temp(id.with("t"), now));
            ctx.memory_mut(|m| m.request_focus(fab_id));
        }

        let bg = anim::color(
            &ctx,
            id.with("bg"),
            if open { solid } else { container },
            motion.fast_effects,
        );
        let fg = anim::color(
            &ctx,
            id.with("fg"),
            if open { on_solid } else { on },
            motion.fast_effects,
        );
        let radius = size.radius() + (28.0 - size.radius()) * k.clamp(0.0, 1.0);
        if ui.is_rect_visible(vis) {
            // The shared painter uses the size's radius; morph it here.
            let radii = Radii::all(radius.max(0.0));
            let elevation = if response.hovered() {
                Elevation::Level4
            } else {
                Elevation::Level3
            };
            paint::shadow(ui.painter(), vis, radii, elevation);
            paint::container(ui.painter(), vis, radii, bg, Stroke::NONE);
            let layer = common::layer_opacity(&ctx, fab_id, &response, true);
            paint::state_layer(ui.painter(), vis, radii, fg, layer);
            let kk = k.clamp(0.0, 1.0);
            let c = pos2(vis.left() + 28.0, vis.center().y);
            self.icon
                .widget(false, common::fade(fg, 1.0 - kk), 24.0)
                .paint_at(ui, Rect::from_center_size(c, Vec2::splat(24.0)));
            Icon::Close
                .widget(false, common::fade(fg, kk), 24.0)
                .paint_at(ui, Rect::from_center_size(c, Vec2::splat(24.0)));
            if let Some(label) = &self.label {
                let a = (1.0 - kk * 2.0).clamp(0.0, 1.0);
                if a > 0.01 {
                    let g = common::plain(
                        ui.painter(),
                        label,
                        size.label().style(),
                        common::fade(fg, a),
                    );
                    ui.painter().with_clip_rect(vis).galley(
                        pos2(
                            vis.left() + 16.0 + 24.0 + 8.0,
                            vis.center().y - g.size().y * 0.5,
                        ),
                        g,
                        fg,
                    );
                }
            }
            if common::focused(&response) {
                paint::focus_ring(ui.painter(), vis, radii, s.secondary);
            }
        }
        let label_text = self
            .label
            .clone()
            .unwrap_or_else(|| self.icon.name().to_owned());
        response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, open, &label_text));

        // Items.
        let n = self.items.len();
        let toggled_at: f64 = ctx
            .data(|d| d.get_temp(id.with("t")))
            .unwrap_or(f64::NEG_INFINITY);
        let elapsed = now - toggled_at;
        let layer = LayerId::new(Order::Foreground, id.with("items"));
        let painter = ctx.layer_painter(layer);
        let widths: Vec<f32> = self
            .items
            .iter()
            .map(|it| {
                48.0 + 24.0
                    + 8.0
                    + common::plain(&painter, &it.label, TypeRole::TitleMedium.style(), on)
                        .size()
                        .x
            })
            .collect();
        let col_w = widths.iter().copied().fold(0.0, f32::max);
        let mut pres = Vec::with_capacity(n);
        let mut any_visible = false;
        for i in 0..n {
            // Open: first item first. Close: last item first.
            let delay = if open {
                i as f64 * STAGGER
            } else {
                (n - 1 - i) as f64 * STAGGER * 0.5
            };
            let item_open = open && elapsed >= delay;
            if open && elapsed < delay {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    (delay - elapsed).max(0.001),
                ));
            }
            let close_open = !open && elapsed < delay;
            let p = common::presence(
                &ctx,
                id.with(("item", i)),
                item_open || close_open,
                motion.fast_spatial,
                motion.fast_effects,
            );
            any_visible |= p.visible();
            pres.push(p);
        }

        let mut clicked = None;
        if any_visible && n > 0 {
            let total_h = n as f32 * ITEM_H + (n as f32 - 1.0) * ITEM_GAP;
            let col_x = if self.align_right {
                vis.right() - col_w
            } else {
                vis.left()
            };
            let col_y = if self.upwards {
                vis.top() - 4.0 - total_h
            } else {
                vis.bottom() + 4.0
            };
            let col = Rect::from_min_size(pos2(col_x, col_y), vec2(col_w, total_h));
            let items = self.items;
            Area::new(id.with("items"))
                .fade_in(false)
                .order(Order::Foreground)
                .fixed_pos(col.min)
                .constrain(false)
                .interactable(true)
                .show(&ctx, |ui| {
                    ui.allocate_exact_size(col.size(), Sense::hover());
                    for (i, it) in items.iter().enumerate() {
                        let p = pres[i];
                        if !p.visible() {
                            continue;
                        }
                        let slide = (1.0 - p.spatial) * if self.upwards { 16.0 } else { -16.0 };
                        let ix = if self.align_right {
                            col.right() - widths[i]
                        } else {
                            col.left()
                        };
                        let iy = if self.upwards {
                            col.bottom() - (n - i) as f32 * ITEM_H - (n - 1 - i) as f32 * ITEM_GAP
                        } else {
                            col.top() + i as f32 * (ITEM_H + ITEM_GAP)
                        };
                        let rect =
                            Rect::from_min_size(pos2(ix, iy + slide), vec2(widths[i], ITEM_H));
                        let iid = id.with(("pill", i));
                        let active = open && p.alpha() > 0.5;
                        let r = ui.interact(
                            rect,
                            iid,
                            if active {
                                Sense::click()
                            } else {
                                Sense::hover()
                            },
                        );
                        let a = p.alpha();
                        let radii = Radii::all(shape::FULL);
                        let painter = ui.painter();
                        paint::shadow(painter, rect, radii, Elevation::Level3);
                        paint::container(
                            painter,
                            rect,
                            radii,
                            common::fade(container, a),
                            Stroke::NONE,
                        );
                        let layer = common::layer_opacity(ui.ctx(), iid, &r, active);
                        paint::state_layer(painter, rect, radii, on, layer);
                        it.icon.widget(false, common::fade(on, a), 24.0).paint_at(
                            ui,
                            Rect::from_center_size(
                                pos2(rect.left() + 24.0 + 12.0, rect.center().y),
                                Vec2::splat(24.0),
                            ),
                        );
                        let g = common::plain(
                            painter,
                            &it.label,
                            TypeRole::TitleMedium.style(),
                            common::fade(on, a),
                        );
                        painter.galley(
                            pos2(
                                rect.left() + 24.0 + 24.0 + 8.0,
                                rect.center().y - g.size().y * 0.5,
                            ),
                            g,
                            on,
                        );
                        if active && common::focused(&r) {
                            paint::focus_ring(painter, rect, radii, s.secondary);
                        }
                        if active && r.clicked() {
                            clicked = Some(i);
                        }
                        let label = it.label.clone();
                        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
                    }
                });
            if open {
                // Click-away: a press that is neither on the FAB nor an item.
                let away = ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer
                            .interact_pos()
                            .is_some_and(|p| !hit_rect(vis).contains(p) && !col.contains(p))
                });
                if away {
                    open = false;
                    ctx.data_mut(|d| d.insert_temp(id.with("t"), now));
                }
            }
        }
        if clicked.is_some() {
            open = false;
            ctx.data_mut(|d| d.insert_temp(id.with("t"), now));
        }
        ctx.data_mut(|d| d.insert_temp(open_key, open));
        FabMenuOutput {
            response,
            open,
            clicked,
        }
    }
}
