//! The equalizer's editor: the ready curves as chips and the ten bands.
//!
//! A band is a rail with a handle on it, the handle at the band's own gain;
//! from the middle line to the handle the rail is filled. Behind the rails
//! lies what the listener hears: the response of all ten filters together,
//! which near a moved band is not that band's gain (neighbours add up), so
//! it is drawn as the ground the handles stand on and not as a line through
//! them. A handle's number shows only while it is dragged; a double click
//! puts it back on the middle line.
//!
//! The editor changes the switch and the gains it is given; the sheet that
//! shows it ([`super::sound`]) sends them, so every move of a handle is
//! heard at once (the engine glides between curves: dragging does not
//! crackle).

use egui::{Align2, Color32, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use nikgob_view::{EQ_BANDS, EQ_FREQS, EQ_MAX_DB, EQ_PRESETS, eq_curve_db, eq_preset_of};

use crate::{
    Theme, TypeRole,
    components::{FilterChip, Radii, anim, paint},
    tokens::{shape, state},
};

/// Height of the bands' field.
const FIELD_H: f32 = 200.0;
/// Room left and right of the first and the last band.
const SIDE: f32 = 20.0;
/// Room above the highest and under the lowest handle.
const EDGE: f32 = 16.0;
/// A handle snaps to this many dB.
const STEP_DB: f32 = 0.5;
/// How far past the outer bands the response is drawn, in bands.
const OVERHANG: f32 = 0.45;
/// Width of a band's rail.
const RAIL_W: f32 = 4.0;
/// Radius of a handle, at rest (every band's is the same) and held.
const HANDLE_R: f32 = 7.0;
const HANDLE_HELD_R: f32 = 9.0;
/// Radius of a handle's state layer.
const HALO_R: f32 = 16.0;
/// The editor's height: the chips' row, the gap, the field and the bands'
/// names (one row of chips; a narrow window wraps them and takes more).
pub(super) const EDITOR_H: f32 = 32.0 + 12.0 + FIELD_H + 24.0;

pub(super) const BAND_NAMES: [&str; EQ_BANDS] = ["32", "64", "125", "250", "500", "1к", "2к", "4к", "8к", "16к"];

/// Where band `band` (fractional: between two bands, or past the ends) is
/// across a field from `left` to `right`.
fn band_x(left: f32, right: f32, band: f32) -> f32 {
    left + (right - left) * band / (EQ_BANDS - 1) as f32
}

/// The frequency at a fractional band: the bands sit evenly, so between two
/// of them the frequency goes by their ratio.
fn band_freq(band: f32) -> f32 {
    let last = (EQ_BANDS - 1) as f32;
    if band <= 0.0 {
        return EQ_FREQS[0] * 2f32.powf(band);
    }
    if band >= last {
        return EQ_FREQS[EQ_BANDS - 1] * 2f32.powf(band - last);
    }
    let i = band.floor() as usize;
    let (a, b) = (EQ_FREQS[i], EQ_FREQS[i + 1]);
    a * (b / a).powf(band - i as f32)
}

/// Where `db` is in a field from `top` to `bottom`.
fn db_y(top: f32, bottom: f32, db: f32) -> f32 {
    let mid = (top + bottom) * 0.5;
    mid - db / EQ_MAX_DB * (bottom - top) * 0.5
}

/// The gain a pointer at `y` asks for, on the steps.
fn y_db(top: f32, bottom: f32, y: f32) -> f32 {
    let mid = (top + bottom) * 0.5;
    let db = (mid - y) / ((bottom - top) * 0.5) * EQ_MAX_DB;
    ((db / STEP_DB).round() * STEP_DB).clamp(-EQ_MAX_DB, EQ_MAX_DB)
}

fn db_text(db: f32) -> String {
    if db == 0.0 {
        "0 дБ".to_owned()
    } else {
        let sign = if db > 0.0 { "+" } else { "−" };
        let abs = db.abs();
        if abs.fract() == 0.0 { format!("{sign}{abs:.0} дБ") } else { format!("{sign}{abs:.1} дБ") }
    }
}

/// The ready curves and the bands. A curve picked or a handle moved
/// switches the equalizer on.
pub(super) fn editor(ui: &mut Ui, on: &mut bool, gains: &mut [f32; EQ_BANDS]) {
    let preset = eq_preset_of(gains);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (i, (name, curve)) in EQ_PRESETS.iter().enumerate() {
            if ui.add(FilterChip::new(*name, preset == Some(i)).id_salt(("eq-preset", i))).clicked() {
                *gains = *curve;
                // A curve picked is a curve wanted; «Ровно» leaves the switch alone.
                if i > 0 {
                    *on = true;
                }
            }
        }
    });
    ui.add_space(12.0);
    field(ui, on, gains);
}

/// The response of the whole equalizer across a field, left to right.
fn response_line(gains: &[f32; EQ_BANDS], left: f32, right: f32, top: f32, bottom: f32) -> Vec<Pos2> {
    let steps = 120;
    let (from, to) = (-OVERHANG, (EQ_BANDS - 1) as f32 + OVERHANG);
    (0..=steps)
        .map(|i| {
            let band = from + (to - from) * i as f32 / steps as f32;
            let db = eq_curve_db(gains, band_freq(band)).clamp(-EQ_MAX_DB * 1.25, EQ_MAX_DB * 1.25);
            pos2(band_x(left, right, band), db_y(top, bottom, db))
        })
        .collect()
}

/// The ground between `line` and the middle line at `zero`, filled.
fn response_fill(line: &[Pos2], zero: f32, color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    for p in line {
        mesh.colored_vertex(*p, color);
        mesh.colored_vertex(pos2(p.x, zero), color);
    }
    for i in 0..line.len().saturating_sub(1) as u32 {
        let at = i * 2;
        mesh.add_triangle(at, at + 1, at + 2);
        mesh.add_triangle(at + 1, at + 3, at + 2);
    }
    mesh
}

/// The bands: their rails and handles over the response.
fn field(ui: &mut Ui, on: &mut bool, gains: &mut [f32; EQ_BANDS]) {
    let theme = Theme::get(ui.ctx());
    let s = theme.scheme;
    let ctx = ui.ctx().clone();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_H + 24.0), Sense::hover());
    let (left, right) = (rect.left() + SIDE, rect.right() - SIDE);
    let (top, bottom) = (rect.top() + EDGE, rect.top() + FIELD_H - EDGE);
    let column = ((right - left) / (EQ_BANDS - 1) as f32).min(48.0);
    let id = ui.id().with("eq-field");

    // The handles first: what is drawn follows this frame's drag.
    let mut held: Option<usize> = None;
    let mut hovered: Option<usize> = None;
    for band in 0..EQ_BANDS {
        let x = band_x(left, right, band as f32);
        let grip = Rect::from_min_max(pos2(x - column * 0.5, rect.top()), pos2(x + column * 0.5, rect.top() + FIELD_H));
        let r = ui.interact(grip, ui.id().with(("eq-band", band)), Sense::click_and_drag());
        if r.double_clicked() {
            gains[band] = 0.0;
        } else if (r.dragged() || r.is_pointer_button_down_on())
            && let Some(p) = r.interact_pointer_pos()
        {
            gains[band] = y_db(top, bottom, p.y);
            *on = true;
            held = Some(band);
        }
        if r.hovered() {
            hovered = Some(band);
        }
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, format!("{} Гц", BAND_NAMES[band])));
    }

    let p = ui.painter_at(rect.expand(8.0));
    // Off: everything is there, quieter (it is what the switch would turn on).
    let ink = if *on { s.primary } else { s.on_surface_variant };
    let zero = db_y(top, bottom, 0.0);

    // What is heard, behind the bands.
    let line = response_line(gains, left, right, top, bottom);
    p.add(Shape::mesh(response_fill(&line, zero, ink.gamma_multiply(0.12))));
    p.add(Shape::line(line, Stroke::new(1.5, ink.gamma_multiply(0.5))));
    p.line_segment([pos2(rect.left(), zero), pos2(rect.right(), zero)], Stroke::new(1.0, s.outline_variant));

    for band in 0..EQ_BANDS {
        let x = band_x(left, right, band as f32);
        let at = pos2(x, db_y(top, bottom, gains[band]));
        let (is_held, is_hovered) = (held == Some(band), hovered == Some(band));

        // The rail, and its part from the middle line to the handle.
        let rail = Rect::from_min_max(pos2(x - RAIL_W * 0.5, top), pos2(x + RAIL_W * 0.5, bottom));
        paint::container(&p, rail, Radii::all(shape::FULL), s.surface_container_highest, Stroke::NONE);
        let active = Rect::from_min_max(pos2(x - RAIL_W * 0.5, at.y.min(zero)), pos2(x + RAIL_W * 0.5, at.y.max(zero)));
        if active.height() > 0.5 {
            paint::container(&p, active, Radii::all(shape::FULL), ink, Stroke::NONE);
        }

        // The handle: the same for every band; its state is a layer around
        // it, and only the one in the hand grows.
        let layer = if is_held {
            state::DRAGGED
        } else if is_hovered {
            state::HOVER
        } else {
            0.0
        };
        let layer = anim::fraction(&ctx, id.with(("layer", band)), layer, theme.motion.fast_effects);
        if layer > 0.001 {
            p.circle_filled(at, HALO_R, ink.gamma_multiply(layer));
        }
        let radius = anim::f32(
            &ctx,
            id.with(("radius", band)),
            if is_held { HANDLE_HELD_R } else { HANDLE_R },
            theme.motion.fast_spatial,
        )
        .max(HANDLE_R - 1.0);
        // A rim of the sheet's own colour keeps the handle apart from its rail.
        p.circle_filled(at, radius + 2.0, s.surface_container_low);
        p.circle_filled(at, radius, ink);

        p.text(
            pos2(x, rect.top() + FIELD_H + 4.0),
            Align2::CENTER_TOP,
            BAND_NAMES[band],
            TypeRole::LabelSmall.font_id(),
            s.on_surface_variant,
        );
        if is_held {
            // Above the handle, or under it at the top of the field.
            let (anchor, y) = if at.y - 16.0 - 16.0 < rect.top() {
                (Align2::CENTER_TOP, at.y + 16.0)
            } else {
                (Align2::CENTER_BOTTOM, at.y - 16.0)
            };
            p.text(pos2(x, y), anchor, db_text(gains[band]), TypeRole::LabelMedium.font_id(), s.on_surface);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_sits_where_its_gain_is_and_reads_back() {
        let (top, bottom) = (100.0, 300.0);
        assert_eq!(db_y(top, bottom, 0.0), 200.0);
        assert_eq!(db_y(top, bottom, EQ_MAX_DB), top);
        assert_eq!(db_y(top, bottom, -EQ_MAX_DB), bottom);
        for db in [-12.0, -3.5, 0.0, 0.5, 6.0, 12.0] {
            assert_eq!(y_db(top, bottom, db_y(top, bottom, db)), db);
        }
        // Past the field's edge the point stays on it.
        assert_eq!(y_db(top, bottom, 0.0), EQ_MAX_DB);
        assert_eq!(y_db(top, bottom, 1000.0), -EQ_MAX_DB);
    }

    #[test]
    fn the_curve_passes_through_the_band_centres() {
        for (band, freq) in EQ_FREQS.iter().enumerate() {
            assert!((band_freq(band as f32) - freq).abs() < freq * 1e-4, "band {band}");
        }
        assert!((band_freq(4.5) - (500.0f32 * 1000.0).sqrt()).abs() < 0.5);
        assert!(band_freq(-OVERHANG) < 32.0 && band_freq(9.0 + OVERHANG) > 16_000.0);
        assert_eq!(band_x(10.0, 100.0, 0.0), 10.0);
        assert_eq!(band_x(10.0, 100.0, 9.0), 100.0);
    }

    #[test]
    fn the_number_of_a_dragged_point_reads_as_decibels() {
        assert_eq!(db_text(0.0), "0 дБ");
        assert_eq!(db_text(3.0), "+3 дБ");
        assert_eq!(db_text(-4.5), "−4.5 дБ");
    }

    /// The response is drawn where it is heard: flat on the middle line
    /// with nothing moved, through a lone band's handle, and filled down
    /// (or up) to the middle line with two triangles a step.
    #[test]
    fn the_response_lies_on_the_middle_line_until_a_band_moves() {
        let (left, right, top, bottom) = (0.0, 900.0, 100.0, 300.0);
        let zero = db_y(top, bottom, 0.0);
        let mut gains = [0.0; EQ_BANDS];
        let flat = response_line(&gains, left, right, top, bottom);
        assert!(flat.iter().all(|p| (p.y - zero).abs() < 1e-3));
        assert!(flat.first().unwrap().x < left && flat.last().unwrap().x > right);

        gains[5] = 6.0;
        let line = response_line(&gains, left, right, top, bottom);
        let handle = pos2(band_x(left, right, 5.0), db_y(top, bottom, 6.0));
        let nearest = line.iter().min_by(|a, b| (a.x - handle.x).abs().total_cmp(&(b.x - handle.x).abs())).unwrap();
        assert!((nearest.y - handle.y).abs() < 2.0, "{nearest:?} against {handle:?}");

        let fill = response_fill(&line, zero, Color32::WHITE);
        assert_eq!(fill.vertices.len(), line.len() * 2);
        assert_eq!(fill.indices.len(), (line.len() - 1) * 6);
        assert!(fill.indices.iter().all(|i| (*i as usize) < fill.vertices.len()));
    }
}
