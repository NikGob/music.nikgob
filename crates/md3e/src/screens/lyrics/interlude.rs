use super::*;

/// The room a wait opens between lines, at the pane's text size: taller for
/// the thick form, a larger ring and higher waves.
pub(super) fn wait_height(prefs: &Prefs) -> f32 {
    let base = if prefs.lyrics_gap_style >= 2 { 42.0 } else { 34.0 };
    let waves = if prefs.lyrics_gap_style % 2 == 1 {
        1.0 + 0.15 * (prefs.lyrics_gap_height.clamp(0.5, 2.0) - 1.0).max(0.0)
    } else {
        1.0
    };
    base * prefs.lyrics_gap_size.clamp(0.8, 1.5) * waves
}

/// The spring a wait's room opens and closes on (the lines part with it).
pub(super) const WAIT_ROOM: Spring = nikgob_motion::tokens::EXPRESSIVE_SLOW_SPATIAL;
/// The spring its ring grows in on, once the room has begun to open.
pub(super) const WAIT_RING: Spring = nikgob_motion::tokens::EXPRESSIVE_DEFAULT_SPATIAL;
/// How long the ring takes to leave: the first half it closes into one
/// ring, the second half that ring shrinks and fades.
pub(super) const WAIT_LEAVE: f32 = 0.5;
/// With the countdown the ring is gone this long before the next line, as
/// the 3 comes in.
pub(super) const WAIT_COUNT_GONE: f32 = 2.9;

/// How far (0..1) a ring has grown in `t` seconds after it starts to come,
/// for the settings' samples (the lyrics drive it by [`WAIT_RING`] itself).
pub(super) fn ring_in(t: f32) -> f32 {
    if t <= 0.0 { 0.0 } else { 1.0 - WAIT_RING.solve(1.0, 0.0, t).0 }
}

/// The wait between lines: a determinate circular wavy progress indicator
/// (Material 3 Expressive). The wavy arc grows clockwise from the top as the
/// gap passes, so it reads as "how much is left"; the rest of the circle is
/// the flat track. While playing the wave travels and swells with the bass.
/// It grows in with `appear` (0..1, on its spring); before the next line
/// (or the countdown) it closes into one ring, which shrinks and fades as
/// one shape, so no end or joint ever shows. Song time drives it, so
/// pausing freezes it. `k` scales it with the text (bigger in mega mode).
#[allow(clippy::too_many_arguments)]
pub(super) fn waiting(painter: &egui::Painter, line: Rect, k: f32, start: Duration, end: Duration, pos: Duration, appear: f32, playing: bool, bass: f32, edge: f32, prefs: &Prefs) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let theme = Theme::get(painter.ctx());
    let s = theme.scheme;
    let motion = !theme.reduced_motion;
    let span = end.saturating_sub(start).as_secs_f32().max(0.001);
    let progress = (pos.saturating_sub(start).as_secs_f32() / span).clamp(0.0, 1.0);
    let remaining = end.saturating_sub(pos).as_secs_f32();
    let fade = (remaining / 0.4).clamp(0.0, 1.0);
    let fade = fade * fade * (3.0 - 2.0 * fade);
    let shown = appear.clamp(0.0, 1.0);
    if edge * shown <= 0.01 {
        return;
    }
    let countdown = prefs.lyrics_countdown && remaining <= 3.0;
    let digit = remaining.ceil().clamp(1.0, 3.0);
    let age = digit - remaining;
    let reveal = if countdown { ((3.0 - remaining) / 0.25).clamp(0.0, 1.0) } else { 0.0 };
    let reveal = 1.0 - (1.0 - reveal).powi(3);
    let thick_form = prefs.lyrics_gap_style >= 2;
    let base_radius = if thick_form { 16.0 } else { 13.0 } * k;
    let c = pos2(line.left() + 2.0 * k + base_radius, line.center().y);
    if countdown {
        let ease = 1.0 - (1.0 - (age / 0.22).clamp(0.0, 1.0)).powi(3);
        let pop = if theme.reduced_motion { 1.0 } else { 1.0 + 0.22 * (1.0 - ease) };
        let alpha = edge * shown * fade * reveal * (age / 0.12).clamp(0.0, 1.0);
        let color = s.primary.gamma_multiply(alpha);
        let g = crate::typography::layout_sized(painter, style_at(28.0 * k * pop), |style, _| {
            LayoutJob::simple_singleline(format!("{digit:.0}"), style.font_id(), color)
        });
        painter.galley(Align2::CENTER_CENTER.anchor_size(c, g.size()).min, g, color);
        // The digit before fades out (shrinking a little) while this one
        // fades in, so the count never blinks between them.
        if digit < 3.0 && age < 0.12 {
            let out = 1.0 - age / 0.12;
            let prev = edge * shown * fade * reveal * out * out;
            let shrink = if theme.reduced_motion { 1.0 } else { 1.0 - 0.12 * (1.0 - out) };
            let color = s.primary.gamma_multiply(prev);
            let g = crate::typography::layout_sized(painter, style_at(28.0 * k * shrink), |style, _| {
                LayoutJob::simple_singleline(format!("{:.0}", digit + 1.0), style.font_id(), color)
            });
            painter.galley(Align2::CENTER_CENTER.anchor_size(c, g.size()).min, g, color);
        }
    }
    // Leaving: the gaps shut, the wave smooths out and the track takes the
    // indicator's colour (one closed ring), then that ring goes.
    let gone_at = if prefs.lyrics_countdown { WAIT_COUNT_GONE } else { 0.0 };
    let out = ((gone_at + WAIT_LEAVE - remaining) / WAIT_LEAVE).clamp(0.0, 1.0);
    let close = (out * 2.0).min(1.0);
    let close = close * close * (3.0 - 2.0 * close);
    let vanish = (out * 2.0 - 1.0).clamp(0.0, 1.0);
    let vanish = vanish * vanish;
    let alpha = edge * shown * (1.0 - vanish);
    if alpha <= 0.01 {
        return;
    }
    // From 60 % as it comes (the spring's small overshoot a soft pop), to
    // 60 % as it goes; reduced motion only fades it.
    let shrink = if motion { (0.6 + 0.4 * appear) * (1.0 - 0.4 * vanish) } else { 1.0 };
    let radius = base_radius * shrink;
    let thick = if thick_form { 6.0 } else { 3.0 } * k * shrink;
    // As in Compose's CircularWavyProgressIndicator: flat near 0 % and
    // 100 %, wavy in between; the crests travel along the arc (one
    // wavelengths a second) and the ring itself stays put. Song time, so
    // it is still when paused and goes on from there.
    let ramp = (progress / 0.1).min((1.0 - progress) / 0.05).clamp(0.0, 1.0);
    let height = prefs.lyrics_gap_height.clamp(0.5, 2.0);
    // About half the stroke: anything less reads as a lumpy polygon, and
    // the tip (where the wave comes out) looks straight half the time.
    // Fewer crests stand taller, more lie lower: «Количество волн» shows as
    // a few big waves or a fine ripple, not the same ring.
    let count = (8.0 / prefs.lyrics_gap_waves.clamp(3.0, 16.0)).sqrt().clamp(0.7, 1.6);
    let requested_amp = if prefs.lyrics_gap_style % 2 == 1 {
        ramp * k * shrink * height * count * (1.4 + 0.5 * if motion && playing { bass.clamp(0.0, 1.0) } else { 0.0 })
    } else { 0.0 };
    let (waves, amp) = wave_geometry(prefs.lyrics_gap_waves, radius, thick, requested_amp, height);
    let amp = amp * (1.0 - close);
    // The song's own time: it stops where it is on a pause and goes on from
    // there (the wall clock while playing and 0 when paused made the wave
    // jump at both).
    let clock = if motion { pos.as_secs_f32() } else { 0.0 };
    // One wavelength a second: a calm ripple. The ring itself stays put.
    let phase = clock * TAU;
    // Chord clearance includes both round caps and radial wave excursions.
    let clearance = thick + 2.0 * amp + 3.0 * k * shrink;
    let gap = 2.0 * (clearance / (2.0 * radius)).clamp(0.0, 1.0).asin() * (1.0 - close);
    let sweep = progress * TAU;
    let feather = 1.0 / super::super::stage::ppp(painter.ctx());
    let points = |from: f32, to: f32, amp: f32| -> Vec<egui::Pos2> {
        let n = ((to - from) * radius / (k * shrink * 0.6)).ceil().max(2.0) as usize;
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f32 / n as f32;
                // Running back from the head, like the linear wavy
                // progress: the wave comes out of the tip. The crests keep
                // their own pace on the ring; tied to the growing head they
                // stood still in a gap as many seconds long as there are
                // crests, and crawled or ran backwards in others.
                let r = radius + amp * (-waves * a - phase).sin();
                let a = a - FRAC_PI_2;
                pos2(c.x + r * a.cos(), c.y + r * a.sin())
            })
            .collect()
    };
    // Opaque scheme colours (as in Material: secondary-container track,
    // primary indicator); transparency only while it fades, and each piece
    // is one mesh, so nothing in it shows darker.
    let indicator = s.primary.gamma_multiply(alpha);
    if close >= 1.0 {
        let mut ring = points(0.0, TAU, 0.0);
        ring.pop();
        painter.add(ribbon(&ring, thick, true, indicator, feather));
        return;
    }
    let arc = |from: f32, to: f32, amp: f32, color: Color32| {
        if to - from <= 0.0 {
            return;
        }
        // An arc shorter than its stroke thins away instead of ending as a
        // dot that pops (unless it is closing into the ring).
        let t = ((to - from) * radius / thick.max(0.01)).clamp(0.0, 1.0);
        let taper = t * t * (3.0 - 2.0 * t);
        let width = thick * (taper + (1.0 - taper) * close);
        painter.add(ribbon(&points(from, to, amp), width, false, color, feather));
    };
    // The flat track: the part still to wait.
    let track = crate::lerp_color(s.secondary_container, s.primary, close).gamma_multiply(alpha);
    arc(sweep + gap, TAU - gap, 0.0, track);
    // The wavy arc: the part already waited.
    arc(0.0, sweep, amp, indicator);
}

/// A stroke `width` wide along `pts`, with round ends (a ring when
/// `closed`), as one mesh with a soft rim `feather` wide. Nothing in it is
/// drawn twice, so in a see-through colour no cap, join or seam shows
/// darker (cap discs laid over a stroke did, as two dots where the ring
/// closes).
pub(super) fn ribbon(pts: &[egui::Pos2], width: f32, closed: bool, color: Color32, feather: f32) -> egui::Shape {
    use std::f32::consts::PI;
    let n = pts.len();
    if n < 2 || width <= 0.0 || color.a() == 0 {
        return egui::Shape::Noop;
    }
    // Thinner than the rim: fainter instead.
    let (core, color) = if width < feather {
        (0.0, color.gamma_multiply(width / feather))
    } else {
        (width * 0.5 - feather * 0.5, color)
    };
    let rim = core + feather;
    let left = |i: usize| {
        let (a, b) = if closed {
            (pts[(i + n - 1) % n], pts[(i + 1) % n])
        } else {
            (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)])
        };
        let d = (b - a).normalized();
        vec2(-d.y, d.x)
    };
    let mut mesh = egui::epaint::Mesh::default();
    // Per point: outer left, core left, core right, outer right.
    for (i, p) in pts.iter().enumerate() {
        let l = left(i);
        mesh.colored_vertex(*p + l * rim, Color32::TRANSPARENT);
        mesh.colored_vertex(*p + l * core, color);
        mesh.colored_vertex(*p - l * core, color);
        mesh.colored_vertex(*p - l * rim, Color32::TRANSPARENT);
    }
    let segments = if closed { n } else { n - 1 };
    for i in 0..segments {
        let (a, b) = (4 * i as u32, 4 * ((i + 1) % n) as u32);
        for (x, y) in [(0, 1), (1, 2), (2, 3)] {
            mesh.add_triangle(a + x, a + y, b + x);
            mesh.add_triangle(a + y, b + y, b + x);
        }
    }
    if !closed {
        // Half discs from the left edge round the back to the right one,
        // sharing the end's own vertices.
        for (i, back) in [(0, -(pts[1] - pts[0]).normalized()), (n - 1, (pts[n - 1] - pts[n - 2]).normalized())] {
            let (p, l, at) = (pts[i], left(i), 4 * i as u32);
            let centre = mesh.vertices.len() as u32;
            mesh.colored_vertex(p, color);
            let steps = ((rim * PI * 0.5).ceil() as usize).clamp(4, 24);
            let (mut core_was, mut rim_was) = (at + 1, at);
            for j in 1..=steps {
                let (c, r) = if j == steps {
                    (at + 2, at + 3)
                } else {
                    let a = PI * j as f32 / steps as f32;
                    let d = l * a.cos() + back * a.sin();
                    let c = mesh.vertices.len() as u32;
                    mesh.colored_vertex(p + d * core, color);
                    mesh.colored_vertex(p + d * rim, Color32::TRANSPARENT);
                    (c, c + 1)
                };
                mesh.add_triangle(centre, core_was, c);
                mesh.add_triangle(core_was, c, r);
                mesh.add_triangle(core_was, r, rim_was);
                (core_was, rim_was) = (c, r);
            }
        }
    }
    egui::Shape::mesh(mesh)
}

/// The crests and their height for a ring of `radius` and `stroke`: at
/// most `height` × 12 % of a wavelength, and never so high that the thick
/// stroke folds over itself.
pub(super) fn wave_geometry(waves: f32, radius: f32, stroke: f32, requested_amp: f32, height: f32) -> (f32, f32) {
    let waves = waves.clamp(3.0, 16.0).round();
    let wavelength = std::f32::consts::TAU * radius / waves;
    // Also bound the curvature so a thick stroke never folds over itself:
    // for r = R + A·sin(nθ) the crest/trough curvature is ≈ (R ± A·n²)/R²,
    // which must stay under 2/stroke.
    let fold = (2.0 * radius * radius / stroke - radius).max(0.0) / (waves * waves + 1.0);
    let amp = requested_amp.max(0.0).min(wavelength * 0.12 * height.clamp(0.5, 2.0)).min(fold);
    (waves, amp)
}
