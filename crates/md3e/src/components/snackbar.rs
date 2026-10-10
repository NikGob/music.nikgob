//! Snackbar host: a queue that shows one snackbar at a time above the
//! floating player. `inverse-surface`, radius 4, level 3, body-medium text, a
//! label-large action in `inverse-primary` and an optional close icon.
//! It slides in on a default spatial spring, fades on fast effects and
//! dismisses itself after 4 s (time-based; the timer holds while hovered).

use egui::{
    Align2, Area, Context, Id, LayerId, Order, Rect, Sense, Stroke, Vec2, WidgetInfo, WidgetType,
    pos2,
    text::{LayoutJob, TextWrapping},
    vec2,
};

use super::{anim, common, paint, paint::Radii};
use crate::{
    Icon, Theme, TypeRole,
    tokens::{Elevation, shape},
};

/// Seconds a snackbar stays up by default.
pub const AUTO_DISMISS: f64 = 4.0;
/// Seconds the exit animation is given before the next snackbar appears.
const EXIT: f64 = 0.15;

/// One queue entry.
#[derive(Clone, Copy, Debug)]
pub struct SnackbarItem<'a> {
    pub id: u64,
    pub text: &'a str,
    pub action: Option<&'a str>,
}

impl<'a> SnackbarItem<'a> {
    pub fn new(id: u64, text: &'a str) -> Self {
        Self {
            id,
            text,
            action: None,
        }
    }

    pub fn action(mut self, label: &'a str) -> Self {
        self.action = Some(label);
        self
    }
}

impl<'a> From<(u64, &'a str, Option<&'a str>)> for SnackbarItem<'a> {
    fn from((id, text, action): (u64, &'a str, Option<&'a str>)) -> Self {
        Self { id, text, action }
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
    shown_at: f64,
    leaving_at: Option<f64>,
}

/// The queue logic, separated from painting so it can be tested.
#[derive(Clone, Debug, Default)]
struct Queue {
    current: Option<Current>,
    /// Ids already reported dismissed but possibly still in the caller's list.
    done: Vec<u64>,
}

impl Queue {
    /// Advances the queue; returns the ids that timed out this step.
    /// `held` is the time the timer was paused (pointer over the snackbar).
    fn step(&mut self, items: &[SnackbarItem<'_>], now: f64, held: f64, duration: f64) -> Vec<u64> {
        let mut dismissed = Vec::new();
        self.done.retain(|d| items.iter().any(|i| i.id == *d));

        if self
            .current
            .as_ref()
            .and_then(|c| c.leaving_at)
            .is_some_and(|t| now - t >= EXIT)
        {
            self.current = None;
        }
        if self.current.is_none()
            && let Some(item) = items.iter().find(|i| !self.done.contains(&i.id))
        {
            self.current = Some(Current {
                id: item.id,
                text: item.text.to_owned(),
                action: item.action.map(str::to_owned),
                shown_at: now,
                leaving_at: None,
            });
        }
        if let Some(cur) = &mut self.current
            && cur.leaving_at.is_none()
        {
            cur.shown_at += held;
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

    /// The user dismissed the current snackbar. Returns its id.
    fn dismiss(&mut self, now: f64) -> Option<u64> {
        let cur = self.current.as_mut()?;
        if cur.leaving_at.is_some() {
            return None;
        }
        cur.leaving_at = Some(now);
        self.done.push(cur.id);
        Some(cur.id)
    }

    fn remaining(&self, now: f64, duration: f64) -> Option<f64> {
        let cur = self.current.as_ref()?;
        cur.leaving_at
            .is_none()
            .then(|| (cur.shown_at + duration - now).max(0.0))
    }
}

/// Shows queued snackbars.
pub struct SnackbarHost {
    id: Id,
    duration: f64,
    show_close: bool,
}

impl SnackbarHost {
    pub fn new(id_salt: impl egui::AsId) -> Self {
        Self {
            id: Id::new(id_salt),
            duration: AUTO_DISMISS,
            show_close: false,
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

    /// Call once per frame. The snackbar sits centred in `area`, its bottom
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
            dismissed: queue.step(items, now, held, self.duration),
            ..SnackbarEvents::default()
        };
        let mut hovering = false;

        if let Some(cur) = queue.current.clone() {
            let leaving = cur.leaving_at.is_some();
            let pres = common::presence(
                ctx,
                self.id.with(("p", cur.id)),
                !leaving,
                motion.default_spatial,
                motion.fast_effects,
            );
            if pres.visible() {
                let layer = LayerId::new(Order::Foreground, self.id.with("area"));
                let painter = ctx.layer_painter(layer);
                let max_w = (area.width() - 32.0).clamp(200.0, 672.0);
                let lay = layout(
                    &painter,
                    &cur.text,
                    cur.action.as_deref(),
                    self.show_close,
                    max_w,
                );
                let slide = (1.0 - pres.spatial) * 24.0;
                let bottom_center = pos2(area.center().x, area.bottom() - bottom_inset + slide);
                let mut action_hit = false;
                let mut close_hit = false;
                let alpha = pres.alpha();
                Area::new(self.id.with("area"))
                    .order(Order::Foreground)
                    .fixed_pos(bottom_center)
                    .pivot(Align2::CENTER_BOTTOM)
                    .constrain(false)
                    .interactable(true)
                    .show(ctx, |ui| {
                        ui.set_opacity(alpha);
                        let (rect, _) = ui.allocate_exact_size(lay.size, Sense::hover());
                        let id = self.id.with(("bar", cur.id));
                        let bar = ui.interact(rect, id, Sense::hover());
                        hovering = bar.contains_pointer();
                        let radii = Radii::all(shape::EXTRA_SMALL);
                        paint::shadow(ui.painter(), rect, radii, Elevation::Level3);
                        paint::container(
                            ui.painter(),
                            rect,
                            radii,
                            s.inverse_surface,
                            Stroke::NONE,
                        );
                        ui.painter().galley(
                            pos2(
                                rect.left() + 16.0,
                                rect.center().y - lay.text.size().y * 0.5,
                            ),
                            lay.text.clone(),
                            s.inverse_on_surface,
                        );
                        let mut right = rect.right();
                        if self.show_close {
                            let c = pos2(right - 24.0, rect.center().y);
                            let hit = Rect::from_center_size(c, Vec2::splat(48.0));
                            let r = ui.interact(hit, id.with("close"), Sense::click());
                            close_hit = r.clicked();
                            let l = common::layer_opacity(ctx, id.with("close"), &r, true);
                            paint::state_layer(
                                ui.painter(),
                                Rect::from_center_size(c, Vec2::splat(40.0)),
                                Radii::all(shape::FULL),
                                s.inverse_on_surface,
                                l,
                            );
                            Icon::Close
                                .widget(false, s.inverse_on_surface, 24.0)
                                .paint_at(ui, Rect::from_center_size(c, Vec2::splat(24.0)));
                            r.widget_info(|| {
                                WidgetInfo::labeled(WidgetType::Button, true, "Close")
                            });
                            right -= 48.0;
                        }
                        if let Some(g) = &lay.action {
                            let w = g.size().x + 24.0;
                            let btn = Rect::from_min_size(
                                pos2(right - 8.0 - w, rect.center().y - 20.0),
                                vec2(w, 40.0),
                            );
                            let hit = Rect::from_center_size(btn.center(), vec2(w, 48.0));
                            let r = ui.interact(hit, id.with("action"), Sense::click());
                            action_hit = r.clicked();
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
                                paint::focus_ring(
                                    ui.painter(),
                                    btn,
                                    Radii::all(shape::FULL),
                                    s.secondary,
                                );
                            }
                            let label = cur.action.clone().unwrap_or_default();
                            r.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, &label));
                        }
                        bar.widget_info(|| WidgetInfo::labeled(WidgetType::Label, true, &cur.text));
                    });
                if !leaving
                    && (action_hit || close_hit)
                    && let Some(id) = queue.dismiss(now)
                {
                    events.dismissed.push(id);
                    if action_hit {
                        events.action_clicked = Some(id);
                    }
                }
            }
            if let Some(rem) = queue.remaining(now, self.duration) {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(rem.max(0.016)));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(30));
            }
        }
        ctx.data_mut(|d| {
            d.insert_temp(state_id, queue);
            d.insert_temp(hover_id, hovering);
        });
        events
    }
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
        + if close { 48.0 } else { 0.0 }
        + if action_g.is_none() && !close {
            16.0
        } else {
            0.0
        };
    let avail_text = (max_w - 16.0 - right).max(80.0);
    // Natural width first, then wrap at the available width (max 2 lines).
    let natural = common::plain(
        painter,
        text,
        TypeRole::BodyMedium.style(),
        s.inverse_on_surface,
    );
    let width = (16.0 + natural.size().x + right).clamp(344.0_f32.min(max_w), max_w);
    let text_w = (width - 16.0 - right).min(avail_text).max(80.0);
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
    let lines = if galley.size().y > 24.0 { 2.0 } else { 1.0 };
    Layout {
        size: vec2(width, 28.0 + 20.0 * lines),
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

    #[test]
    fn shows_one_at_a_time_and_times_out_after_four_seconds() {
        let mut q = Queue::default();
        let list = items(&[1, 2]);
        assert!(q.step(&list, 10.0, 0.0, AUTO_DISMISS).is_empty());
        assert_eq!(q.current.as_ref().map(|c| c.id), Some(1));
        assert!(q.step(&list, 13.9, 0.0, AUTO_DISMISS).is_empty());
        assert_eq!(q.step(&list, 14.0, 0.0, AUTO_DISMISS), vec![1]);
        // Still leaving: the second one waits for the exit.
        assert_eq!(q.current.as_ref().map(|c| c.id), Some(1));
        assert!(q.step(&list, 14.05, 0.0, AUTO_DISMISS).is_empty());
        assert_eq!(q.current.as_ref().map(|c| c.id), Some(1));
        // Caller did not remove 1 yet: it is still skipped.
        assert!(q.step(&list, 14.2, 0.0, AUTO_DISMISS).is_empty());
        assert_eq!(q.current.as_ref().map(|c| c.id), Some(2));
        assert_eq!(q.step(&list, 18.2, 0.0, AUTO_DISMISS), vec![2]);
    }

    #[test]
    fn holding_pauses_the_timer() {
        let mut q = Queue::default();
        let list = items(&[7]);
        q.step(&list, 0.0, 0.0, AUTO_DISMISS);
        q.step(&list, 3.0, 2.0, AUTO_DISMISS); // 2 s held
        assert!(q.step(&list, 5.5, 0.0, AUTO_DISMISS).is_empty());
        assert_eq!(q.step(&list, 6.0, 0.0, AUTO_DISMISS), vec![7]);
    }

    #[test]
    fn user_dismissal_and_withdrawal() {
        let mut q = Queue::default();
        let list = items(&[1, 2]);
        q.step(&list, 0.0, 0.0, AUTO_DISMISS);
        assert_eq!(q.dismiss(1.0), Some(1));
        assert_eq!(q.dismiss(1.0), None);
        // Caller dropped 1; 2 follows after the exit.
        let rest = items(&[2]);
        q.step(&rest, 1.2, 0.0, AUTO_DISMISS);
        assert_eq!(q.current.as_ref().map(|c| c.id), Some(2));
        // Withdrawn from outside: fades out without a dismissal event.
        assert!(q.step(&[], 2.0, 0.0, AUTO_DISMISS).is_empty());
        assert!(q.current.as_ref().is_some_and(|c| c.leaving_at.is_some()));
        q.step(&[], 2.2, 0.0, AUTO_DISMISS);
        assert!(q.current.is_none());
    }
}
