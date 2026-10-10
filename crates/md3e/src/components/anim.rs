//! Springs kept in egui's memory, keyed by widget id.
//!
//! Every value is evaluated from `input.time`, so a frame that comes late
//! (or a window shown after being hidden) lands where the spring would be,
//! and a widget that is not drawn costs nothing.

use egui::{Color32, Context, Id};
use nikgob_motion::{Animatable, Animated, Spring};

use crate::Theme;

/// The clock every animation reads.
pub fn now(ctx: &Context) -> f64 {
    ctx.input(|i| i.time)
}

/// The spring to use, honouring reduced motion.
pub fn effective(ctx: &Context, spring: Spring) -> Spring {
    if Theme::get(ctx).reduced_motion {
        spring.without_bounce()
    } else {
        spring
    }
}

fn drive<T>(ctx: &Context, id: Id, target: T, spring: Spring, threshold: f32) -> T
where
    T: Animatable + Send + Sync + 'static,
{
    let spring = effective(ctx, spring);
    let t = now(ctx);
    let (value, settled) = ctx.data_mut(|d| {
        let a = d.get_temp_mut_or_insert_with(id, || {
            Animated::new(target, spring).with_threshold(threshold)
        });
        if a.spring() != spring {
            a.animate_to(target, spring, t);
        } else {
            a.set_target(target, t);
        }
        (a.value(t), a.is_settled(t))
    });
    if !settled {
        ctx.request_repaint();
    }
    value
}

/// A pixel quantity (size, radius, offset) following `target`.
pub fn f32(ctx: &Context, id: Id, target: f32, spring: Spring) -> f32 {
    // "Full" radii are infinite; a spring cannot travel there.
    if !target.is_finite() {
        return target;
    }
    drive(ctx, id, target, spring, 0.01)
}

/// A 0..1 fraction (opacity, progress of a morph) following `target`.
pub fn fraction(ctx: &Context, id: Id, target: f32, spring: Spring) -> f32 {
    drive(ctx, id, target, spring, 0.001)
}

/// A colour following `target`. Interpolates unmultiplied sRGB channels;
/// colour springs are effects springs, so there is no overshoot to clamp.
pub fn color(ctx: &Context, id: Id, target: Color32, spring: Spring) -> Color32 {
    let [r, g, b, a] = target.to_srgba_unmultiplied();
    let c = drive(
        ctx,
        id,
        [f32::from(r), f32::from(g), f32::from(b), f32::from(a)],
        spring,
        0.5,
    );
    let ch = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(ch(c[0]), ch(c[1]), ch(c[2]), ch(c[3]))
}

/// Jump an animation to `value` (e.g. on first show of a screen).
pub fn snap_f32(ctx: &Context, id: Id, value: f32) {
    ctx.data_mut(|d| {
        if let Some(mut a) = d.get_temp::<Animated<f32>>(id) {
            a.snap(value);
            d.insert_temp(id, a);
        }
    });
}
