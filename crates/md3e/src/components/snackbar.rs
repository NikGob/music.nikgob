//! Snackbar host: a stack of up to three snackbars above the floating
//! player, newest at the bottom. An Expressive take on the M3 snackbar:
//! `inverse-surface` with large corners and a soft level-3 shadow, a
//! leading shape chip whose shape and colour say what the message is about
//! (a morphing loading shape while something works), body-medium text, a
//! label-large action in `inverse-primary`, an optional close icon and a
//! thin line running down with the time left.
//!
//! Each snackbar springs up from below with a little bounce (default
//! spatial) while its corners settle, the stack glides to make room, and an
//! ending one shrinks and fades in place. Timers are time-based and hold
//! while the pointer is over any snackbar of the stack.

use egui::{
    Align2, Area, Context, Id, Order, Rect, Sense, Stroke, Vec2, WidgetInfo, WidgetType,
    pos2,
    text::{LayoutJob, TextWrapping},
    vec2,
};
use nikgob_shapes::{MaterialShape, paint::fill_shape};

use super::{LoadingIndicator, anim, common, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape},
};

/// Seconds a snackbar stays up by default.
pub const AUTO_DISMISS: f64 = 4.0;
/// Snackbars on screen at once; more wait their turn.
pub const MAX_VISIBLE: usize = 3;
/// Seconds an ending snackbar is still drawn (its exit animation).
const EXIT: f64 = 0.35;
/// Space between stacked snackbars.
const GAP: f32 = 8.0;
/// The leading chip.
const CHIP: f32 = 36.0;

/// What a snackbar reports; picks the chip's shape, colour and icon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnackbarKind {
    #[default]
    Info,
    Success,
    /// Work going on in the background: the chip is a loading shape.
    Working,
    Error,
}

/// One queue entry.
#[derive(Clone, Copy, Debug)]
pub struct SnackbarItem<'a> {
    pub id: u64,
    pub text: &'a str,
    pub action: Option<&'a str>,
    pub kind: SnackbarKind,
}

impl<'a> SnackbarItem<'a> {
    pub fn new(id: u64, text: &'a str) -> Self {
        Self {
            id,
            text,
            action: None,
            kind: SnackbarKind::Info,
        }
    }

    pub fn action(mut self, label: &'a str) -> Self {
        self.action = Some(label);
        self
    }

    pub fn kind(mut self, kind: SnackbarKind) -> Self {
        self.kind = kind;
        self
    }
}

impl<'a> From<(u64, &'a str, Option<&'a str>)> for SnackbarItem<'a> {
    fn from((id, text, action): (u64, &'a str, Option<&'a str>)) -> Self {
        Self {
            id,
            text,
            action,
            kind: SnackbarKind::Info,
        }
    }
}

/// What happened to the queue this frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnackbarEvents {
    /// The action button of this snackbar was pressed (it is also in
    /// `dismissed`).
    pub action_clicked: Option<u64>,
    /// Snackbars that ended (timeout, close icon or action); drop them from
    /// your list.
    pub dismissed: Vec<u64>,
}

#[derive(Clone, Debug)]
struct Current {
    id: u64,
    text: String,
    action: Option<String>,
    kind: SnackbarKind,
    shown_at: f64,
    leaving_at: Option<f64>,
}

impl Current {
    /// The share of its time still left, 1 -> 0.
    fn left(&self, now: f64, duration: f64) -> f32 {
        if self.leaving_at.is_some() || duration <= 0.0 {
            return 0.0;
        }
        (1.0 - (now - self.shown_at) / duration).clamp(0.0, 1.0) as f32
    }
}

/// The queue logic, separated from painting so it can be tested.
#[derive(Clone, Debug, Default)]
struct Queue {
    /// On screen, oldest first (ending ones too, until their exit is over).
    shown: Vec<Current>,
    /// Ids already reported dismissed but possibly still in the caller's list.
    done: Vec<u64>,
}

impl Queue {
    /// Advances the queue; returns the ids that timed out this step.
    /// `held` is the time the timers were paused (pointer over the stack).
    fn step(
        &mut self,
        items: &[SnackbarItem<'_>],
        now: f64,
        held: f64,
        duration: f64,
        max: usize,
    ) -> Vec<u64> {
        let mut dismissed = Vec::new();
        self.done.retain(|d| items.iter().any(|i| i.id == *d));
        self.shown
            .retain(|c| c.leaving_at.is_none_or(|t| now - t < EXIT));

        for item in items {
            let active = self.shown.iter().filter(|c| c.leaving_at.is_none()).count();
            if active >= max {
                break;
            }
            if self.done.contains(&item.id) || self.shown.iter().any(|c| c.id == item.id) {
                continue;
            }
            self.shown.push(Current {
                id: item.id,
                text: item.text.to_owned(),
                action: item.action.map(str::to_owned),
                kind: item.kind,
                shown_at: now,
                leaving_at: None,
            });
        }
        for cur in &mut self.shown {
            if cur.leaving_at.is_some() {
                continue;
            }
            if cur.shown_at < now {
                cur.shown_at += held;
            }
            if !items.iter().any(|i| i.id == cur.id) {
                // The caller withdrew it.
                cur.leaving_at = Some(now);
            } else if now - cur.shown_at >= duration {
                cur.leaving_at = Some(now);
                self.done.push(cur.id);
                dismissed.push(cur.id);
            }
        }
        dismissed
    }

    /// The user dismissed snackbar `id`. Returns it if it was still up.
    fn dismiss(&mut self, id: u64, now: f64) -> Option<u64> {
        let cur = self
            .shown
            .iter_mut()
            .find(|c| c.id == id && c.leaving_at.is_none())?;
        cur.leaving_at = Some(now);
        self.done.push(id);
        Some(id)
    }

    /// Seconds until the next snackbar times out.
    #[cfg(test)]
    fn remaining(&self, now: f64, duration: f64) -> Option<f64> {
        self.shown
            .iter()
            .filter(|c| c.leaving_at.is_none())
            .map(|c| (c.shown_at + duration - now).max(0.0))
            .reduce(f64::min)
    }
}

/// Shows queued snackbars.
pub struct SnackbarHost {
    id: Id,
    duration: f64,
    show_close: bool,
    max_visible: usize,
}

impl SnackbarHost {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            duration: AUTO_DISMISS,
            show_close: false,
            max_visible: MAX_VISIBLE,
        }
    }

    /// Seconds before a snackbar dismisses itself.
    pub fn duration(mut self, seconds: f64) -> Self {
        self.duration = seconds;
        self
    }

    /// Shows a close icon.
    pub fn show_close(mut self, show: bool) -> Self {
        self.show_close = show;
        self
    }

    /// How many snackbars may be up at once (1 = the classic one at a time).
    pub fn max_visible(mut self, n: usize) -> Self {
        self.max_visible = n.max(1);
        self
    }

    /// Call once per frame. The stack sits centred in `area`, its bottom
    /// edge `bottom_inset` above the area's bottom (the floating player).
    pub fn show(
        &self,
        ctx: &Context,
        area: Rect,
        bottom_inset: f32,
        items: &[SnackbarItem<'_>],
    ) -> SnackbarEvents {
        let theme = Theme::get(ctx);
        let (s, motion) = (theme.scheme, theme.motion);
        let now = anim::now(ctx);
        let state_id = self.id.with("queue");
        let hover_id = self.id.with("hover");
        let mut queue: Queue = ctx.data(|d| d.get_temp(state_id)).unwrap_or_default();
        let hold = ctx.data(|d| d.get_temp::<bool>(hover_id)).unwrap_or(false);
        let held = if hold {
            f64::from(ctx.input(|i| i.stable_dt))
        } else {
            0.0
        };

        let mut events = SnackbarEvents {
            dismissed: queue.step(items, now, held, self.duration, self.max_visible),
            ..SnackbarEvents::default()
        };
        let mut hovering = false;
        let mut user_dismissed: Option<(u64, bool)> = None;

        // Newest at the bottom: lay the stack out upwards from there.
        let max_w = (area.width() - 32.0).clamp(200.0, 600.0);
        let mut bottom = area.bottom() - bottom_inset;
        for cur in queue.shown.iter().rev() {
            let leaving = cur.leaving_at.is_some();
            let pres = common::presence(
                ctx,
                self.id.with(("p", cur.id)),
                !leaving,
                motion.default_spatial,
                motion.fast_effects,
            );
            let k = pres.spatial.clamp(0.0, 1.2);
            let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, self.id.with("measure")));
            let lay = layout(&painter, &cur.text, cur.action.as_deref(), self.show_close, max_w);
            // The slot collapses as a snackbar leaves, so the rest glide down.
            let slot = (lay.size.y + GAP) * if leaving { k.min(1.0) } else { 1.0 };
            let target = bottom;
            bottom -= slot;
            if !pres.visible() {
                continue;
            }
            // Others glide to their new place; a new one starts at its own.
            let y = anim::f32(
                ctx,
                self.id.with(("y", cur.id)),
                target,
                motion.default_spatial.without_bounce(),
            );
            // Rises from below on entry; shrinks in place on exit.
            let rise = if leaving { 0.0 } else { (1.0 - k) * (lay.size.y * 0.5 + 24.0) };
            let scale = 0.86 + 0.14 * k.min(1.0);
            let bottom_center = pos2(area.center().x, y + rise);
            let alpha = pres.alpha();
            let area_id = self.id.with(("area", cur.id));
            let resp = Area::new(area_id)
                .fade_in(false)
                .order(Order::Foreground)
                .fixed_pos(bottom_center)
                .pivot(Align2::CENTER_BOTTOM)
                .constrain(false)
                .interactable(!leaving)
                .show(ctx, |ui| {
                    ui.set_opacity(alpha);
                    let (rect, _) = ui.allocate_exact_size(lay.size, Sense::hover());
                    let id = self.id.with(("bar", cur.id));
                    let bar = ui.interact(rect, id, Sense::hover());
                    hovering |= bar.contains_pointer();
                    // Corners settle from extra-large to large as it lands.
                    let radius = shape::LARGE + (shape::EXTRA_LARGE - shape::LARGE) * (1.0 - k.min(1.0));
                    let radii = Radii::all(radius);
                    let elevation = if bar.contains_pointer() { Elevation::Level4 } else { Elevation::Level3 };
                    paint::shadow(ui.painter(), rect, radii, elevation);
                    paint::container(ui.painter(), rect, radii, s.inverse_surface, Stroke::NONE);

                    // Leading chip: the shape turns into place.
                    let chip = Rect::from_center_size(
                        pos2(rect.left() + 12.0 + CHIP * 0.5, rect.center().y),
                        Vec2::splat(CHIP),
                    );
                    paint_chip(ui, chip, cur.kind, (1.0 - k) * 1.2);

                    ui.painter().galley(
                        pos2(
                            rect.left() + TEXT_LEFT,
                            rect.center().y - lay.text.size().y * 0.5,
                        ),
                        lay.text.clone(),
                        s.inverse_on_surface,
                    );
                    let mut right = rect.right();
                    if self.show_close {
                        let c = pos2(right - 28.0, rect.center().y);
                        let hit = Rect::from_center_size(c, Vec2::splat(48.0));
                        let r = ui.interact(hit, id.with("close"), Sense::click());
                        if r.clicked() {
                            user_dismissed = Some((cur.id, false));
                        }
                        let l = common::layer_opacity(ctx, id.with("close"), &r, true);
                        paint::state_layer(
                            ui.painter(),
                            Rect::from_center_size(c, Vec2::splat(40.0)),
                            Radii::all(shape::FULL),
                            s.inverse_on_surface,
                            l,
                        );
                        Icon::Close
                            .widget(false, s.inverse_on_surface.gamma_multiply(0.8), 20.0)
                            .paint_at(ui, Rect::from_center_size(c, Vec2::splat(20.0)));
                        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Close"));
                        right -= 52.0;
                    }
                    if let Some(g) = &lay.action {
                        let w = g.size().x + 24.0;
                        let btn = Rect::from_min_size(
                            pos2(right - 8.0 - w, rect.center().y - 20.0),
                            vec2(w, 40.0),
                        );
                        let hit = Rect::from_center_size(btn.center(), vec2(w, 48.0));
                        let r = ui.interact(hit, id.with("action"), Sense::click());
                        if r.clicked() {
                            user_dismissed = Some((cur.id, true));
                        }
                        let l = common::layer_opacity(ctx, id.with("action"), &r, true);
                        paint::state_layer(
                            ui.painter(),
                            btn,
                            Radii::all(shape::FULL),
                            s.inverse_primary,
                            l,
                        );
                        ui.painter().galley(
                            pos2(btn.left() + 12.0, btn.center().y - g.size().y * 0.5),
                            g.clone(),
                            s.inverse_primary,
                        );
                        if common::focused(&r) {
                            paint::focus_ring(ui.painter(), btn, Radii::all(shape::FULL), s.secondary);
                        }
                        let label = cur.action.clone().unwrap_or_default();
                        r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
                    }

                    // Time left: a thin line along the bottom, inside the corners.
                    let left = cur.left(now, self.duration);
                    if left > 0.0 && cur.kind != SnackbarKind::Working {
                        let track = Rect::from_min_max(
                            pos2(rect.left() + radius, rect.bottom() - 5.0),
                            pos2(rect.right() - radius, rect.bottom() - 3.0),
                        );
                        paint::container(
                            ui.painter(),
                            track,
                            Radii::all(shape::FULL),
                            s.inverse_on_surface.gamma_multiply(0.12),
                            Stroke::NONE,
                        );
                        let mut fill = track;
                        fill.set_width(track.width() * left);
                        paint::container(
                            ui.painter(),
                            fill,
                            Radii::all(shape::FULL),
                            s.inverse_primary.gamma_multiply(0.85),
                            Stroke::NONE,
                        );
                    }
                    bar.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &cur.text));
                });
            common::layer_scale(ctx, resp.response.layer_id, bottom_center, scale);
        }

        if let Some((id, action)) = user_dismissed
            && let Some(id) = queue.dismiss(id, now)
        {
            events.dismissed.push(id);
            if action {
                events.action_clicked = Some(id);
            }
        }
        if !queue.shown.is_empty() {
            // The time-left line moves continuously.
            ctx.request_repaint_after(std::time::Duration::from_millis(30));
        }
        ctx.data_mut(|d| {
            d.insert_temp(state_id, queue);
            d.insert_temp(hover_id, hovering);
        });
        events
    }
}

/// Where the text starts: after the chip.
const TEXT_LEFT: f32 = 12.0 + CHIP + 14.0;

/// The leading chip: a Material shape in the kind's container colour with
/// its icon, or a morphing loading shape for work in progress. `turn` is the
/// extra rotation (radians) it still has to turn into place.
fn paint_chip(ui: &mut egui::Ui, rect: Rect, kind: SnackbarKind, turn: f32) {
    let s = Theme::get(ui.ctx()).scheme;
    // Container tones are the opposite of the inverse surface's in either
    // theme, so they stand out on it.
    let (shape, fill, on, icon) = match kind {
        SnackbarKind::Info => (MaterialShape::Circle, s.secondary_container, s.on_secondary_container, Icon::Info),
        SnackbarKind::Success => (MaterialShape::Cookie9Sided, s.inverse_primary, s.inverse_surface, Icon::Check),
        SnackbarKind::Error => (MaterialShape::Cookie4Sided, s.error_container, s.on_error_container, Icon::Error),
        SnackbarKind::Working => {
            ui.put(rect, LoadingIndicator::new().size(rect.width()).color(s.inverse_primary));
            return;
        }
    };
    let cubics = shape.polygon().cubics().to_vec();
    fill_shape(ui.painter(), &cubics, rect, turn, fill);
    icon.widget(false, on, 20.0)
        .paint_at(ui, Rect::from_center_size(rect.center(), Vec2::splat(20.0)));
}

struct Layout {
    size: Vec2,
    text: std::sync::Arc<egui::Galley>,
    action: Option<std::sync::Arc<egui::Galley>>,
}

fn layout(
    painter: &egui::Painter,
    text: &str,
    action: Option<&str>,
    close: bool,
    max_w: f32,
) -> Layout {
    let s = Theme::get(painter.ctx()).scheme;
    let action_g =
        action.map(|a| common::plain(painter, a, TypeRole::LabelLarge.style(), s.inverse_primary));
    let right = action_g
        .as_ref()
        .map_or(0.0, |g| g.size().x + 24.0 + 8.0 + 8.0)
        + if close { 52.0 } else { 0.0 }
        + if action_g.is_none() && !close {
            20.0
        } else {
            0.0
        };
    let avail_text = (max_w - TEXT_LEFT - right).max(80.0);
    // Natural width first, then wrap at the available width (max 2 lines).
    let natural = common::plain(
        painter,
        text,
        TypeRole::BodyMedium.style(),
        s.inverse_on_surface,
    );
    let width = (TEXT_LEFT + natural.size().x + right).clamp(320.0_f32.min(max_w), max_w);
    let text_w = (width - TEXT_LEFT - right).min(avail_text).max(80.0);
    let mut job = LayoutJob::single_section(
        text.to_owned(),
        TypeRole::BodyMedium
            .style()
            .text_format(s.inverse_on_surface),
    );
    job.wrap = TextWrapping {
        max_width: text_w,
        max_rows: 2,
        ..TextWrapping::default()
    };
    let galley = painter.layout_job(job);
    let lines: f32 = if galley.size().y > 24.0 { 2.0 } else { 1.0 };
    Layout {
        size: vec2(width, (32.0 + 20.0 * lines).max(60.0)),
        text: galley,
        action: action_g,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(ids: &[u64]) -> Vec<SnackbarItem<'static>> {
        ids.iter().map(|&i| SnackbarItem::new(i, "text")).collect()
    }

    fn ids(q: &Queue) -> Vec<u64> {
        q.shown.iter().filter(|c| c.leaving_at.is_none()).map(|c| c.id).collect()
    }

    #[test]
    fn stacks_up_to_the_limit_and_each_times_out_on_its_own() {
        let mut q = Queue::default();
        let list = items(&[1, 2, 3, 4]);
        assert!(q.step(&list, 10.0, 0.0, AUTO_DISMISS, 3).is_empty());
        assert_eq!(ids(&q), vec![1, 2, 3]);
        assert!(q.step(&list, 13.9, 0.0, AUTO_DISMISS, 3).is_empty());
        assert_eq!(q.step(&list, 14.0, 0.0, AUTO_DISMISS, 3), vec![1, 2, 3]);
        // The fourth comes up as soon as the others stop counting.
        q.step(&list, 14.01, 0.0, AUTO_DISMISS, 3);
        assert_eq!(ids(&q), vec![4]);
        // The ending ones are kept for their exit, then dropped.
        assert_eq!(q.shown.len(), 4);
        q.step(&list, 14.5, 0.0, AUTO_DISMISS, 3);
        assert_eq!(q.shown.len(), 1);
        assert_eq!(q.step(&list, 18.01, 0.0, AUTO_DISMISS, 3), vec![4]);
    }

    #[test]
    fn one_at_a_time_when_limited_to_one() {
        let mut q = Queue::default();
        let list = items(&[1, 2]);
        q.step(&list, 0.0, 0.0, AUTO_DISMISS, 1);
        assert_eq!(ids(&q), vec![1]);
        assert_eq!(q.step(&list, 4.0, 0.0, AUTO_DISMISS, 1), vec![1]);
        q.step(&list, 4.01, 0.0, AUTO_DISMISS, 1);
        assert_eq!(ids(&q), vec![2]);
    }

    #[test]
    fn holding_pauses_the_timers() {
        let mut q = Queue::default();
        let list = items(&[7]);
        q.step(&list, 0.0, 0.0, AUTO_DISMISS, 3);
        q.step(&list, 3.0, 2.0, AUTO_DISMISS, 3); // 2 s held
        assert!(q.step(&list, 5.5, 0.0, AUTO_DISMISS, 3).is_empty());
        assert!((q.shown[0].left(5.5, AUTO_DISMISS) - 0.125).abs() < 1e-3);
        assert_eq!(q.step(&list, 6.0, 0.0, AUTO_DISMISS, 3), vec![7]);
        assert_eq!(q.shown[0].left(6.0, AUTO_DISMISS), 0.0);
    }

    #[test]
    fn user_dismissal_and_withdrawal() {
        let mut q = Queue::default();
        let list = items(&[1, 2]);
        q.step(&list, 0.0, 0.0, AUTO_DISMISS, 3);
        assert_eq!(q.dismiss(1, 1.0), Some(1));
        assert_eq!(q.dismiss(1, 1.0), None);
        assert_eq!(ids(&q), vec![2]);
        assert_eq!(q.remaining(1.0, AUTO_DISMISS), Some(3.0));
        // Withdrawn from outside: fades out without a dismissal event.
        assert!(q.step(&[], 2.0, 0.0, AUTO_DISMISS, 3).is_empty());
        assert!(ids(&q).is_empty());
        q.step(&[], 2.5, 0.0, AUTO_DISMISS, 3);
        assert!(q.shown.is_empty());
        assert_eq!(q.remaining(2.5, AUTO_DISMISS), None);
    }
}
