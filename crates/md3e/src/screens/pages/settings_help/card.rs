use std::collections::HashMap;
use std::sync::Arc;

use egui::{
    Align, Area, Color32, CursorIcon, Galley, Id, Key, Modifiers, Order, Painter, Pos2, Rect, Sense, Shape, Stroke,
    Ui, Vec2, pos2, text::LayoutJob, vec2,
};
use nikgob_core::i18n::Locale;
use nikgob_view::{self as v, Prefs};
use nikgob_core::i18n::settings::{
    fmt_days_in, fmt_gb_in, fmt_one_decimal_in, settings_choices_in, settings_misc, settings_misc_in, settings_row_help_in,
    settings_row_labels_in,
};

use super::super::home::Section;
use crate::screens::stage::{Part, Scene, Script};
use crate::screens::{bg_shapes, lyrics, now_playing};
use crate::{
    ColorScheme, Contrast, Elevation, Icon, SchemeVariant, Theme, TypeRole,
    components::{anim, paint, paint::Radii, slider::wave_line},
    tokens::shape,
};

use super::*;

use super::preview::{Preview, sample};
use super::stage::{field_label_in, field_text_in};

/// How long the glow on a linked row lasts.
pub const FOCUS_S: f64 = 1.5;
/// How long the tint on a linked group lasts.
pub const GROUP_S: f64 = 2.4;
/// The opacity of the primary layer tinting a linked group at its fullest.
pub const GROUP_TINT: f32 = 0.12;
/// Frames the scroll to a linked row is repeated, while the page settles.
const SCROLL_FRAMES: u8 = 3;

/// The row a link opened, kept for its glow.
#[derive(Clone, Copy, Debug)]
struct Focus {
    id: SettingId,
    start: f64,
    scrolls: u8,
    rect: Option<Rect>,
    /// The whole group of the row is shown (tinted), not the row.
    group: bool,
}

/// The group being drawn while a link asks for a whole group: its tint is
/// reserved behind its title and painted once its end is known.
#[derive(Clone, Copy, Debug)]
struct Span {
    pass: u64,
    layer: egui::LayerId,
    bg: egui::layers::ShapeIdx,
    clip: Rect,
    x: egui::Rangef,
    top: f32,
    /// It holds the linked row.
    linked: bool,
}

/// What a row reports about its insides while it is drawn.
#[derive(Clone, Debug, Default)]
struct Parts {
    button: Option<Rect>,
    control: Option<Rect>,
    choices: Vec<Rect>,
    menu: Vec<Rect>,
}

/// One row drawn this pass, for the cards.
#[derive(Clone, Debug)]
struct Row {
    id: SettingId,
    label: String,
    rect: Rect,
    button: Rect,
    control: Option<Rect>,
    /// Its segmented buttons, and the items of its open menu.
    choices: Vec<Rect>,
    menu: Vec<Rect>,
    clip: Rect,
    hovered: bool,
    /// The widget with the keyboard focus is in it.
    focused: bool,
    /// How it was drawn: faded by a row it depends on, or switched off.
    opacity: f32,
    enabled: bool,
    /// The title of the group it is in ([`note_group`]).
    group: String,
}

#[derive(Clone, Debug, Default)]
struct Rows {
    pass: u64,
    list: Vec<Row>,
}

/// The card pinned open by a row's ⓘ.
#[derive(Clone, Copy, Debug)]
struct Pinned {
    id: SettingId,
    /// The pass it was opened in (its click is not a click outside).
    opened: u64,
    /// When it opened, for its fade-in.
    at: f64,
    /// What it shows (an option while the pointer is on its button) and
    /// since when, for the cross-fade.
    body: Body,
    changed: f64,
    /// Where the card was last drawn.
    card: Option<Rect>,
}

/// What a card shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Body {
    /// The row's explanation and its current preview.
    Row,
    /// One option of a choice, previewed.
    Option(usize),
    /// The explanation and the options as lines (nothing to preview).
    All,
    /// A preset's: the explanation, then its fields × its presets, the
    /// current preset's column marked.
    Table,
}

fn focus_key() -> Id {
    Id::new("settings-focus")
}

fn span_key() -> Id {
    Id::new("settings-focus-span")
}

fn parts_key() -> Id {
    Id::new("settings-help-parts")
}

fn rows_key() -> Id {
    Id::new("settings-help-rows")
}

fn group_key() -> Id {
    Id::new("settings-help-group")
}

fn pinned_key() -> Id {
    Id::new("settings-help-pinned")
}

fn pinned(ctx: &egui::Context) -> Option<Pinned> {
    ctx.data(|d| d.get_temp::<Option<Pinned>>(pinned_key())).flatten()
}

fn set_pinned(ctx: &egui::Context, p: Option<Pinned>) {
    ctx.data_mut(|d| d.insert_temp(pinned_key(), p));
}

/// Starts the scroll and glow for `id` (the page then switches its tab);
/// with `group`, for the whole group that holds it.
pub fn begin_focus(ctx: &egui::Context, id: SettingId, group: bool) {
    let start = anim::now(ctx);
    ctx.data_mut(|d| d.insert_temp(focus_key(), Focus { id, start, scrolls: SCROLL_FRAMES, rect: None, group }));
}

/// The tint (0..1) of a linked group `t` seconds after the link: it comes
/// up on the M3 slow-effects spring once the page has landed, holds while
/// the eye finds it, and eases back out by [`GROUP_S`]. Reduced motion
/// shows it at once and only fades it out.
pub fn group_tint_at(t: f32, reduced: bool) -> f32 {
    const DELAY: f32 = 0.15;
    const HOLD: f32 = 1.2;
    let total = GROUP_S as f32;
    if !(0.0..total).contains(&t) {
        return 0.0;
    }
    let rise = if reduced {
        1.0
    } else {
        let t = (t - DELAY).max(0.0);
        if t <= 0.0 { 0.0 } else { 1.0 - nikgob_motion::tokens::SLOW_EFFECTS.solve(1.0, 0.0, t).0 }
    };
    let out = ((t - HOLD) / (total - HOLD)).clamp(0.0, 1.0);
    rise.clamp(0.0, 1.0) * (1.0 - out * out * (3.0 - 2.0 * out))
}

/// Called as each settings group starts (before its title): ends the
/// group before it and, while a link asks for a whole group, reserves the
/// new group's tint behind its title.
pub fn group_start(ui: &mut Ui) {
    let ctx = ui.ctx().clone();
    end_group(ui);
    if ctx.data(|d| d.get_temp::<Focus>(focus_key())).is_some_and(|f| f.group) {
        let span = Span {
            pass: ctx.cumulative_pass_nr(),
            layer: ui.layer_id(),
            bg: ui.painter().add(Shape::Noop),
            clip: ui.clip_rect(),
            x: ui.max_rect().x_range(),
            // The title comes after a 24 px space.
            top: ui.cursor().top() + 12.0,
            linked: false,
        };
        ctx.data_mut(|d| d.insert_temp(span_key(), span));
    }
}

/// Ends the group being drawn here: the linked one gets its tint and is
/// where the page scrolls.
fn end_group(ui: &Ui) {
    let ctx = ui.ctx();
    let Some(span) = ctx.data_mut(|d| {
        let span = d.get_temp::<Span>(span_key());
        d.remove::<Span>(span_key());
        span
    }) else {
        return;
    };
    if span.pass != ctx.cumulative_pass_nr() || !span.linked {
        return;
    }
    let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key())) else { return };
    let rect = Rect::from_x_y_ranges(span.x, span.top..=ui.cursor().top() + 8.0).expand2(vec2(12.0, 0.0));
    let theme = Theme::get(ctx);
    let a = group_tint_at((anim::now(ctx) - f.start) as f32, theme.reduced_motion);
    if a > 0.0 {
        // A primary tonal layer over the page, as a selected surface has:
        // not secondary_container, which is the sliders' own track colour
        // and would hide it.
        let painter = Painter::new(ctx.clone(), span.layer, Rect::from_x_y_ranges(rect.x_range(), span.clip.y_range()));
        painter.set(span.bg, Shape::rect_filled(rect, shape::LARGE, theme.scheme.primary.gamma_multiply(GROUP_TINT * a)));
    }
    if (anim::now(ctx) - f.start) < GROUP_S {
        ctx.request_repaint();
    }
    f.rect = Some(rect);
    ctx.data_mut(|d| d.insert_temp(focus_key(), f));
}

/// The glow (0..1) `t` seconds after a link opened the row: two soft
/// pulses fading out by [`FOCUS_S`].
pub fn glow_at(t: f32) -> f32 {
    let total = FOCUS_S as f32;
    if !(0.0..total).contains(&t) {
        return 0.0;
    }
    let rise = (t / 0.15).clamp(0.0, 1.0);
    let fall = 1.0 - (t / total).powi(2);
    let pulse = 0.7 + 0.3 * (t * std::f32::consts::TAU / 0.75).cos();
    rise * fall * pulse
}

/// Wraps one settings item: the glow behind it when a link opened it, its
/// ⓘ button (unless the item placed it itself with [`help_button`]) and
/// the cards about it.
pub fn item<R>(ui: &mut Ui, id: SettingId, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = ui.ctx().clone();
    ctx.data_mut(|d| d.insert_temp(parts_key(), Parts::default()));
    let bg = ui.painter().add(Shape::Noop);
    let inner = ui.scope(add);
    let rect = inner.response.rect;
    if let Some(f) = ctx.data(|d| d.get_temp::<Focus>(focus_key()))
        && f.id == id
        && f.group
    {
        ctx.data_mut(|d| {
            if let Some(mut span) = d.get_temp::<Span>(span_key()) {
                span.linked = true;
                d.insert_temp(span_key(), span);
            }
        });
    } else if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key()))
        && f.id == id
    {
        let t = (anim::now(&ctx) - f.start) as f32;
        let a = glow_at(t);
        if a > 0.0 {
            let s = Theme::get(&ctx).scheme;
            // The whole row of the column, with room around the text. It
            // may reach past the page's left edge, so only the vertical clip
            // of the scroll area applies.
            let row = Rect::from_x_y_ranges(ui.max_rect().x_range(), rect.y_range());
            let r = row.expand2(vec2(12.0, 7.0));
            let clip = ui.clip_rect();
            let mut painter = ui.painter().clone();
            painter.set_clip_rect(Rect::from_x_y_ranges(r.x_range(), clip.y_range()));
            painter.set(bg, Shape::rect_filled(r, shape::LARGE, s.primary.gamma_multiply(0.18 * a)));
            painter.rect_stroke(r, shape::LARGE, Stroke::new(2.0, s.primary.gamma_multiply(0.7 * a)), egui::StrokeKind::Inside);
            ctx.request_repaint();
        }
        f.rect = Some(rect);
        ctx.data_mut(|d| d.insert_temp(focus_key(), f));
    }
    let parts = ctx.data_mut(|d| d.remove_temp::<Parts>(parts_key())).unwrap_or_default();
    let button = parts.button.unwrap_or_else(|| {
        // Blocks of fields: beside the first field, on its centre line.
        let r = Rect::from_min_size(pos2(rect.right() + 8.0, rect.top() + 17.0), Vec2::splat(BTN));
        help_button_at(ui, id, r);
        r
    });
    let row = Row {
        id,
        label: label.to_owned(),
        rect: rect.union(button),
        button,
        control: parts.control,
        choices: parts.choices,
        menu: parts.menu,
        clip: ui.clip_rect(),
        hovered: ui.rect_contains_pointer(rect.union(button)),
        focused: ctx
            .memory(|m| m.focused())
            .and_then(|f| ctx.read_response(f))
            .is_some_and(|r| rect.union(button).contains(r.rect.center())),
        opacity: ui.opacity(),
        enabled: ui.is_enabled(),
        group: ctx.data(|d| d.get_temp::<String>(group_key())).unwrap_or_default(),
    };
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| {
        let rows = d.get_temp_mut_or_default::<Rows>(rows_key());
        if rows.pass != pass {
            rows.pass = pass;
            rows.list.clear();
        }
        rows.list.push(row);
    });
    inner.inner
}

/// Size of the ⓘ button.
pub const BTN: f32 = 22.0;

/// The ⓘ button of the item being drawn, placed here in the layout (next
/// to the row's label).
pub fn help_button(ui: &mut Ui, id: SettingId) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(BTN), Sense::hover());
    help_button_at(ui, id, rect);
    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).button = Some(rect));
}

/// The ⓘ button of the item being drawn, just after `beside` (a control
/// that fills its row, such as a connected group) and on its centre line.
pub fn help_button_beside(ui: &mut Ui, id: SettingId, beside: Rect) {
    let rect = Rect::from_min_size(pos2(beside.right() + 8.0, beside.center().y - BTN * 0.5), Vec2::splat(BTN));
    help_button_at(ui, id, rect);
    ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).button = Some(rect));
}

/// The rects of the item's open menu, in [`SettingId::choices`] order.
pub fn report_menu(ctx: &egui::Context, rects: Vec<Rect>) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).menu = rects);
}

/// The rect of the item's control (the sweep checks it does not cover the ⓘ).
pub fn report_control(ctx: &egui::Context, rect: Rect) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).control = Some(rect));
}

/// The rects of the item's options, in [`SettingId::choices`] order, so
/// hovering one previews it.
pub fn report_choices(ctx: &egui::Context, rects: Vec<Rect>) {
    ctx.data_mut(|d| d.get_temp_mut_or_default::<Parts>(parts_key()).choices = rects);
}

fn help_button_at(ui: &Ui, id: SettingId, rect: Rect) {
    let ctx = ui.ctx();
    let s = Theme::get(ctx).scheme;
    let resp = ui
        .interact(rect, Id::new(("settings-help-btn", id)), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let was = pinned(ctx).is_some_and(|p| p.id == id);
    if resp.clicked() {
        let at = anim::now(ctx);
        set_pinned(ctx, (!was).then(|| Pinned { id, opened: ctx.cumulative_pass_nr(), at, body: Body::Row, changed: at, card: None }));
    }
    let on = was != resp.clicked();
    let painter = ui.painter();
    if on {
        painter.circle_filled(rect.center(), BTN * 0.5, s.secondary_container);
    } else if resp.hovered() {
        painter.circle_filled(rect.center(), BTN * 0.5, s.on_surface.gamma_multiply(0.08));
    }
    let tint = if on {
        s.on_secondary_container
    } else if resp.hovered() {
        s.on_surface
    } else {
        s.on_surface_variant.gamma_multiply(0.75)
    };
    Icon::Info.widget(on, tint, 16.0).paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(16.0)));
}

/// After the page: scrolls to a linked row and shows the pinned card. A
/// pick in the card lands in `prefs`. Beside the live stage (`samples`
/// false) the card has no sample of its own: the stage shows it.
pub fn finish(ui: &mut Ui, prefs: &mut Prefs, samples: bool) {
    let ctx = ui.ctx().clone();
    end_group(ui);
    if let Some(mut f) = ctx.data(|d| d.get_temp::<Focus>(focus_key())) {
        if let Some(rect) = f.rect.take()
            && f.scrolls > 0
        {
            // After the controls, so their nested scroll areas cannot take
            // it. A group taller than the view shows from its title.
            if f.group && rect.height() > ui.clip_rect().height() * 0.8 {
                ui.scroll_to_rect(Rect::from_min_size(rect.min - vec2(0.0, 16.0), vec2(rect.width(), 1.0)), Some(Align::Min));
            } else {
                ui.scroll_to_rect(rect.expand(24.0), Some(Align::Center));
            }
            f.scrolls -= 1;
            ctx.request_repaint();
        }
        let lasts = if f.group { GROUP_S } else { FOCUS_S };
        // A row that never shows (another level, a hidden list) lets go too.
        let gone = anim::now(&ctx) - f.start > lasts + 1.0;
        if (f.scrolls == 0 && anim::now(&ctx) - f.start > lasts) || gone {
            ctx.data_mut(|d| d.remove::<Focus>(focus_key()));
        } else {
            ctx.data_mut(|d| d.insert_temp(focus_key(), f));
        }
    }

    let pass = ctx.cumulative_pass_nr();
    let rows = ctx.data(|d| d.get_temp::<Rows>(rows_key())).filter(|r| r.pass == pass).map(|r| r.list).unwrap_or_default();

    if let Some(mut pin) = pinned(&ctx) {
        // Gone with its ⓘ (another tab, hidden, scrolled away): the card
        // opens next to that button, so it has nothing to hold on to.
        let Some(row) = rows.iter().find(|r| r.id == pin.id && r.clip.intersects(r.button)) else {
            set_pinned(&ctx, None);
            return;
        };
        let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
        let outside = pin.opened != pass
            && ctx.input(|i| {
                i.pointer.any_pressed()
                    && i.pointer
                        .interact_pos()
                        .is_some_and(|p| !row.rect.contains(p) && !pin.card.is_some_and(|c| c.contains(p)))
            });
        if esc || outside {
            set_pinned(&ctx, None);
            return;
        }
        // The row's current option, or the option whose button (or item
        // of its open menu) the pointer is on.
        let now = anim::now(&ctx);
        let pointer = ctx.pointer_hover_pos();
        let n = row.id.choices().len();
        let on = |p: Pos2| row.choices.iter().position(|r| r.contains(p)).or_else(|| row.menu.iter().position(|r| r.contains(p)));
        let body = match pointer.and_then(on).filter(|i| *i < n) {
            Some(i) => Body::Option(i),
            None => base_body(row.id),
        };
        if body != pin.body {
            pin.body = body;
            pin.changed = now;
        }
        let content = ((now - pin.changed) / 0.14).clamp(0.0, 1.0) as f32;
        if content < 1.0 {
            ctx.request_repaint();
        }
        let loc = prefs.resolved_locale();
        let layout = Layout::new(&ctx, row, body, samples, loc);
        // Next to this row's own ⓘ, not under the row: a group title's row
        // is the whole list, and anchoring there dropped the card in the corner.
        let anchor = row.button;
        let target = Rect::from_min_size(place(anchor, layout.size, ctx.content_rect()), layout.size);
        // A short fade and rise from the row (effects timing, no bounce).
        let t = ((anim::now(&ctx) - pin.at) / 0.18).clamp(0.0, 1.0) as f32;
        let k = 1.0 - (1.0 - t).powi(3);
        if t < 1.0 {
            ctx.request_repaint();
        }
        let rise = if target.top() >= anchor.bottom() { -1.0 } else { 1.0 } * 8.0 * (1.0 - k);
        let rect = target.translate(vec2(0.0, -rise));
        let out = show_card(&ctx, Id::new("settings-help-pinned-card"), rect, k, content, &layout, row, prefs, true);
        if let Some(i) = out.pick {
            row.id.choose(prefs, i);
        }
        pin.card = Some(rect);
        if !out.close {
            // A press outside the row and the card puts the card away and
            // reaches no slider under it.
            crate::components::slider::note_dismissible(&ctx, row.rect);
            crate::components::slider::note_dismissible(&ctx, rect);
        }
        set_pinned(&ctx, (!out.close).then_some(pin));
    }
}

/// What the ⓘ card of `id` shows: the explanation and the current
/// option's preview, or the options as lines when nothing is previewed.
pub(super) fn base_body(id: SettingId) -> Body {
    if matches!(id, SettingId::Preset(_)) {
        Body::Table
    } else if !id.choices().is_empty() && id.preview().is_none() {
        Body::All
    } else {
        Body::Row
    }
}

/// Text width of a one-thing card.
const CARD_W: f32 = 280.0;
/// Text width of a preset's card (its table).
const TABLE_W: f32 = 360.0;
/// A line of a preset's table.
const TABLE_ROW: f32 = 20.0;
const PREVIEW_H: f32 = 120.0;
const PAD: f32 = 12.0;
/// Between a row's ⓘ and its card.
pub(super) const GAP: f32 = 6.0;

/// A paragraph in `role`, wrapped at `width`.
fn para(painter: &Painter, role: TypeRole, text: &str, color: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), role.style().text_format(color));
    job.wrap.max_width = width;
    painter.layout_job(job)
}

/// Where a card of `size` opens next to `button`: just under it, from the
/// button's left, or just above when there is no room below; grown left
/// from the button's right edge when it would leave the screen on the right.
/// On screen, and over the button only when it fits neither way.
pub(super) fn place(button: Rect, size: Vec2, screen: Rect) -> Pos2 {
    // Keep off the window's edges: more at the sides than at the bottom,
    // where a few px decide whether the card fits under its button.
    let screen = screen.shrink2(vec2(8.0, 4.0));
    let below = button.bottom() + GAP;
    let above = button.top() - GAP - size.y;
    let y = if below + size.y <= screen.bottom() {
        below
    } else if above >= screen.top() {
        above
    } else if screen.bottom() - button.bottom() >= button.top() - screen.top() {
        (screen.bottom() - size.y).max(screen.top())
    } else {
        screen.top()
    };
    // Next to the ⓘ: grow right from its left edge. If that leaves the
    // screen, grow left from its right edge. If neither fits, stay on screen.
    let grow_right = button.left();
    let grow_left = button.right() - size.x;
    let fits = |x: f32| x >= screen.left() && x + size.x <= screen.right();
    let x = if fits(grow_right) {
        grow_right
    } else if fits(grow_left) {
        grow_left
    } else {
        grow_right.clamp(screen.left(), (screen.right() - size.x).max(screen.left()))
    };
    pos2(x, y)
}

fn card_frame(painter: &Painter, rect: Rect, s: &ColorScheme) {
    let radii = Radii::all(shape::MEDIUM);
    paint::shadow(painter, rect, radii, Elevation::Level2);
    paint::container(painter, rect, radii, s.surface_container, Stroke::NONE);
}

/// Prefs with option `i` of `id` picked.
fn with_choice(id: SettingId, prefs: &Prefs, i: usize) -> Prefs {
    let mut p = prefs.clone();
    id.choose(&mut p, i);
    p
}

/// A card laid out: its size and its texts.
struct Layout {
    size: Vec2,
    body: Body,
    kind: Option<Preview>,
    title: Arc<Galley>,
    help: Arc<Galley>,
    /// [`Body::All`]: each option's name and what it does.
    texts: Vec<(Arc<Galley>, Arc<Galley>)>,
    /// [`Body::Table`].
    table: Option<Table>,
    /// The text's width.
    width: f32,
    /// A close button in the header (the pinned card).
    close: bool,
}

/// A preset's table: the presets across, the fields down (texts without a
/// colour of their own, painted in the colour of their column).
struct Table {
    heads: Vec<Arc<Galley>>,
    rows: Vec<(Arc<Galley>, Vec<Arc<Galley>>)>,
    label_w: f32,
}

impl Table {
    fn new(painter: &Painter, loc: Locale, area: v::presets::Area, width: f32) -> Self {
        let text = |role: TypeRole, t: &str| para(painter, role, t, Color32::PLACEHOLDER, width);
        let heads: Vec<Arc<Galley>> = area.names().iter().map(|n| text(TypeRole::LabelMedium, *n)).collect();
        let n = heads.len();
        let rows: Vec<(Arc<Galley>, Vec<Arc<Galley>>)> = area
            .fields()
            .iter()
            .filter(|f| **f != v::presets::Field::RadioNewShareBy)
            .map(|&f| {
                let values = (0..n)
                    .map(|i| {
                        let t = area.values(i).iter().find(|(g, _)| *g == f).map_or_else(|| "—".to_owned(), |(_, v)| field_text_in(loc, f, v));
                        text(TypeRole::BodySmall, &t)
                    })
                    .collect();
                (text(TypeRole::BodySmall, field_label_in(loc, f)), values)
            })
            .collect();
        let label_w = rows.iter().map(|(l, _)| l.size().x).fold(0.0, f32::max).min(width * 0.42) + 8.0;
        Self { heads, rows, label_w }
    }

    fn height(&self) -> f32 {
        TABLE_ROW * (self.rows.len() + 1) as f32
    }

    /// Paints it from `(x, y)`, `width` wide, the column of preset
    /// `current` marked.
    fn paint(&self, painter: &Painter, x: f32, y: f32, width: f32, current: Option<usize>, s: &ColorScheme) {
        let n = self.heads.len().max(1);
        let col = (width - self.label_w) / n as f32;
        let at = |i: usize| x + self.label_w + i as f32 * col;
        if let Some(c) = current.filter(|c| *c < n) {
            let r = Rect::from_min_size(pos2(at(c) - 4.0, y - 2.0), vec2(col, self.height() + 4.0));
            painter.rect_filled(r, shape::SMALL, s.secondary_container);
        }
        let mid = |g: &Arc<Galley>, top: f32| top + (TABLE_ROW - g.size().y) * 0.5;
        for (i, h) in self.heads.iter().enumerate() {
            let color = if current == Some(i) { s.on_secondary_container } else { s.on_surface_variant };
            painter.galley(pos2(at(i), mid(h, y)), h.clone(), color);
        }
        for (k, (label, values)) in self.rows.iter().enumerate() {
            let top = y + TABLE_ROW * (k + 1) as f32;
            painter.galley(pos2(x, mid(label, top)), label.clone(), s.on_surface);
            for (i, v) in values.iter().enumerate() {
                let color = if current == Some(i) { s.on_secondary_container } else { s.on_surface_variant };
                painter.galley(pos2(at(i), mid(v, top)), v.clone(), color);
            }
        }
    }
}

/// Room for the close button beside the title.
const CLOSE_W: f32 = 32.0;

impl Layout {
    fn new(ctx: &egui::Context, row: &Row, body: Body, samples: bool, loc: Locale) -> Self {
        let pinned = true;
        let s = Theme::get(ctx).scheme;
        let painter = ctx.layer_painter(egui::LayerId::new(Order::Tooltip, Id::new("settings-help-measure")));
        let choices = row.id.choices_in(loc);
        let kind = if matches!(body, Body::All | Body::Table) || !samples { None } else { row.id.preview() };
        let width = if body == Body::Table { TABLE_W } else { CARD_W };
        let (title, help) = match body {
            Body::Option(i) => (format!("{} · {}", row.label, choices[i].label), choices[i].help.to_owned()),
            _ if row.id.beta() => (row.label.clone(), settings_misc_in(loc).beta_card.to_owned() + row.id.help_in(loc)),
            _ => (row.label.clone(), row.id.help_in(loc).to_owned()),
        };
        let close_w = if pinned { CLOSE_W } else { 0.0 };
        let title = para(&painter, TypeRole::TitleSmall, &title, s.on_surface, width - close_w);
        let help = para(&painter, TypeRole::BodyMedium, &help, s.on_surface_variant, width);
        let table = match (body, row.id) {
            (Body::Table, SettingId::Preset(area)) => Some(Table::new(&painter, loc, area, width)),
            _ => None,
        };
        let texts: Vec<(Arc<Galley>, Arc<Galley>)> = if body == Body::All {
            choices
                .iter()
                .map(|c| {
                    (
                        para(&painter, TypeRole::LabelLarge, c.label, s.on_surface, CARD_W - 28.0),
                        para(&painter, TypeRole::BodySmall, c.help, s.on_surface_variant, CARD_W - 28.0),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let header_h = title.size().y.max(if pinned { 24.0 } else { 0.0 });
        let mut h = PAD + header_h + 2.0 + help.size().y;
        if kind.is_some() {
            h += 10.0 + PREVIEW_H;
        }
        for (lg, hg) in &texts {
            h += 8.0 + lg.size().y + hg.size().y;
        }
        if let Some(t) = &table {
            h += 10.0 + t.height();
        }
        h += PAD;
        Self { size: vec2(width + 2.0 * PAD, h), body, kind, title, help, texts, table, width, close: pinned }
    }
}

/// What a card's clicks did.
struct CardOut {
    pick: Option<usize>,
    close: bool,
}

/// Paints `layout` into `rect` (which may still be gliding to its size) at
/// `alpha`, the content at `content`; `interactive` cards take clicks.
/// Title (with a close button when pinned), explanation, then the preview
/// of the option it is about (the current one for the row), or the
/// options as lines when there is nothing to preview.
#[allow(clippy::too_many_arguments)]
fn show_card(
    ctx: &egui::Context,
    area: Id,
    rect: Rect,
    alpha: f32,
    content: f32,
    layout: &Layout,
    row: &Row,
    prefs: &Prefs,
    interactive: bool,
) -> CardOut {
    let s = Theme::get(ctx).scheme;
    let mut out = CardOut { pick: None, close: false };
    Area::new(area)
        .order(Order::Tooltip)
        .fixed_pos(rect.min)
        .constrain(false)
        .interactable(interactive)
        .fade_in(false)
        .show(ctx, |ui| {
            ui.set_opacity(alpha);
            let sense = if interactive { Sense::click() } else { Sense::hover() };
            let (_, _) = ui.allocate_exact_size(rect.size(), sense);
            card_frame(ui.painter(), rect, &s);
            let x0 = rect.left() + PAD;
            let mut y = rect.top() + PAD;
            let header_h = layout.title.size().y.max(if layout.close { 24.0 } else { 0.0 });
            if layout.close {
                let close_r = Rect::from_center_size(pos2(rect.right() - PAD - 10.0, y + header_h * 0.5), Vec2::splat(28.0));
                let resp = ui.interact(close_r, area.with("close"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                if resp.hovered() {
                    ui.painter().circle_filled(close_r.center(), 14.0, s.on_surface.gamma_multiply(0.08));
                }
                Icon::Close.widget(false, s.on_surface_variant, 18.0).paint_at(ui, Rect::from_center_size(close_r.center(), Vec2::splat(18.0)));
                out.close = resp.clicked();
            }
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.multiply_opacity(content);
            let ui = &mut ui;
            let th = layout.title.size().y;
            ui.painter().galley(pos2(x0, y + (header_h - th) * 0.5), layout.title.clone(), s.on_surface);
            y += header_h + 2.0;
            ui.painter().galley(pos2(x0, y), layout.help.clone(), s.on_surface_variant);
            y += layout.help.size().y;
            if let Some(kind) = layout.kind {
                y += 10.0;
                let r = Rect::from_min_size(pos2(x0, y), vec2(CARD_W, PREVIEW_H));
                match layout.body {
                    Body::Option(i) => {
                        sample(ui, r, kind, &with_choice(row.id, prefs, i), Some(Id::new(("card", row.id, i))));
                    }
                    // A choice shows its current option exactly; a slider or
                    // switch may show a stand-in while its effect is off.
                    _ if !row.id.choices().is_empty() => {
                        let i = row.id.chosen(prefs);
                        sample(ui, r, kind, &with_choice(row.id, prefs, i), Some(Id::new(("card", row.id, i))));
                    }
                    _ => sample(ui, r, kind, prefs, None),
                }
            }
            if let (Some(t), SettingId::Preset(area)) = (&layout.table, row.id) {
                y += 10.0;
                t.paint(ui.painter(), x0, y, layout.width, area.current(prefs), &s);
            }
            if !layout.texts.is_empty() {
                // The options as lines: a radio mark, the name, what it does.
                let chosen = row.id.chosen(prefs);
                for (i, (lg, hg)) in layout.texts.iter().enumerate() {
                    y += 8.0;
                    let rh = lg.size().y + hg.size().y;
                    let r = Rect::from_min_size(pos2(x0 - 4.0, y - 4.0), vec2(CARD_W + 8.0, rh + 8.0));
                    let sel = i == chosen;
                    if interactive {
                        let resp = ui.interact(r, area.with(("opt", i)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
                        if resp.hovered() && !sel {
                            ui.painter().rect_filled(r, shape::SMALL, s.on_surface.gamma_multiply(0.06));
                        }
                        if resp.clicked() && !sel {
                            out.pick = Some(i);
                        }
                    }
                    let dot = pos2(x0 + 8.0, y + lg.size().y * 0.5);
                    ui.painter().circle_stroke(dot, 6.0, Stroke::new(2.0, if sel { s.primary } else { s.on_surface_variant }));
                    if sel {
                        ui.painter().circle_filled(dot, 3.0, s.primary);
                    }
                    let lh = lg.size().y;
                    ui.painter().galley(pos2(x0 + 24.0, y), lg.clone(), if sel { s.primary } else { s.on_surface });
                    ui.painter().galley(pos2(x0 + 24.0, y + lh), hg.clone(), s.on_surface_variant);
                    y += rh;
                }
            }
        });
    out
}

/// The settings group being drawn is titled `title` (the rows after it
/// carry it, for the settings sweep).
pub fn note_group(ctx: &egui::Context, title: &str) {
    ctx.data_mut(|d| d.insert_temp(group_key(), title.to_owned()));
}

/// One settings row as the page drew it (the settings sweep reads them).
#[derive(Clone, Debug)]
pub struct DrawnRow {
    pub id: SettingId,
    pub label: String,
    pub group: String,
    /// The row with its ⓘ, its ⓘ, its control and the control's options.
    pub rect: Rect,
    pub button: Rect,
    pub control: Option<Rect>,
    pub choices: Vec<Rect>,
    /// The items of its open menu, in [`SettingId::choices`] order.
    pub menu: Vec<Rect>,
    /// What of the page is on screen around it.
    pub clip: Rect,
    pub opacity: f32,
    pub enabled: bool,
    /// The pointer is on it (or its ⓘ).
    pub hovered: bool,
    /// The widget with the keyboard focus is in it.
    pub focused: bool,
}

/// The rows of the last pass that drew any, with that pass's number.
pub fn drawn_rows(ctx: &egui::Context) -> (u64, Vec<DrawnRow>) {
    let rows = ctx.data(|d| d.get_temp::<Rows>(rows_key())).unwrap_or_default();
    let list = rows
        .list
        .into_iter()
        .map(|r| DrawnRow {
            id: r.id,
            label: r.label,
            group: r.group,
            rect: r.rect,
            button: r.button,
            control: r.control,
            choices: r.choices,
            menu: r.menu,
            clip: r.clip,
            opacity: r.opacity,
            enabled: r.enabled,
            hovered: r.hovered,
            focused: r.focused,
        })
        .collect();
    (rows.pass, list)
}

