//! Scroll bars that keep out of the way, for every `ScrollArea`.
//!
//! The bar floats in a thin gutter of its own, so it never covers content
//! (no more bars over the buttons at the right edge of a list). It stays
//! invisible while nothing scrolls, shows on the hovered area while the
//! wheel or touchpad moves it, and fades out [`IDLE`] seconds after the
//! scrolling stops. Pointing at the bar itself always shows it, wider, so it
//! can still be grabbed.

use egui::{Context, Id, Vec2, style::ScrollStyle};

/// Width of the bar under the pointer.
pub const BAR: f32 = 8.0;
/// Width of the bar elsewhere.
pub const THIN: f32 = 4.0;
/// The gutter the bar floats in (content stops this far from the edge).
pub const GUTTER: f32 = BAR + 4.0;
/// Seconds after the last scroll the bar is still fully shown.
pub const IDLE: f64 = 1.0;
/// Seconds the fade-out takes.
pub const FADE: f64 = 0.3;

/// How visible a bar is `since` seconds after the last scroll (1 -> 0).
pub fn visibility(since: f64) -> f32 {
    if since <= IDLE {
        1.0
    } else {
        (1.0 - (since - IDLE) / FADE).clamp(0.0, 1.0) as f32
    }
}

/// The scroll style at a visibility (see [`visibility`]).
pub fn style(visible: f32) -> ScrollStyle {
    ScrollStyle {
        floating: true,
        bar_width: BAR,
        floating_width: THIN,
        floating_allocated_width: GUTTER,
        bar_inner_margin: 4.0,
        bar_outer_margin: 0.0,
        handle_min_length: 32.0,
        foreground_color: true,
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.35,
        dormant_handle_opacity: 0.0,
        active_handle_opacity: 0.55 * visible,
        interact_handle_opacity: 0.9,
        ..ScrollStyle::solid()
    }
}

fn last_scroll_id() -> Id {
    Id::new("md3e-scrollbars-last")
}

/// Call once per frame, before anything scrolls: notes scrolling and
/// installs the style for the current visibility (for the next frame's
/// `Ui`s; set the returned one on an already made root `Ui`).
pub fn apply(ctx: &Context) -> ScrollStyle {
    let (now, scrolled) = ctx.input(|i| {
        (
            i.time,
            i.smooth_scroll_delta != Vec2::ZERO
                || i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. })),
        )
    });
    let last = ctx.data_mut(|d| {
        let last = d.get_temp_mut_or_insert_with(last_scroll_id(), || f64::NEG_INFINITY);
        if scrolled {
            *last = now;
        }
        *last
    });
    let since = now - last;
    let visible = visibility(since);
    let want = style(visible);
    if ctx.global_style().spacing.scroll != want {
        ctx.all_styles_mut(|s| s.spacing.scroll = want);
    }
    if visible > 0.0 {
        // Wake up for the fade.
        let wait = if since < IDLE { IDLE - since } else { 0.016 };
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait.max(0.016)));
    }
    want
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_while_scrolling_then_fades_after_a_second() {
        assert_eq!(visibility(0.0), 1.0);
        assert_eq!(visibility(IDLE), 1.0);
        assert!((visibility(IDLE + FADE * 0.5) - 0.5).abs() < 1e-4);
        assert_eq!(visibility(IDLE + FADE), 0.0);
        assert_eq!(visibility(f64::INFINITY), 0.0);
    }

    #[test]
    fn the_bar_has_its_own_gutter_and_hides_when_idle() {
        let s = style(0.0);
        assert!(s.floating);
        assert!(s.allocated_width() >= s.bar_width, "the bar must not cover content");
        assert_eq!(s.dormant_handle_opacity, 0.0);
        assert_eq!(s.active_handle_opacity, 0.0);
        assert!(s.interact_handle_opacity > 0.5, "pointing at the bar shows it");
        assert!(style(1.0).active_handle_opacity > 0.0);
    }

    #[test]
    fn apply_installs_the_style_and_tracks_scrolling() {
        let ctx = Context::default();
        assert_eq!(apply(&ctx), style(0.0));
        assert_eq!(ctx.global_style().spacing.scroll, style(0.0));
    }
}
