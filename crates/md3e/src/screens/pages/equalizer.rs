//! The equalizer's sheet: its switch, the ready curves as chips, and the
//! curve itself with one point per band to drag. A point's number shows
//! only while it is dragged; a double click puts it back on the line.
//!
//! The sheet edits [`Prefs::eq_on`] and [`Prefs::eq_gains`] and sends the
//! prefs whenever they change, so every move of a point is heard at once
//! (the engine glides between curves, so dragging does not crackle).

use egui::{Align2, Context, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use nikgob_view::{Command, EQ_BANDS, EQ_FREQS, EQ_MAX_DB, EQ_PRESETS, Prefs, ViewState, eq_curve_db, eq_preset_of};

use crate::{
    Theme, TypeRole,
    components::{BottomSheet, FilterChip, Switch},
};

/// Height of the curve's field.
const FIELD_H: f32 = 200.0;
/// Room left and right of the first and the last band.
const SIDE: f32 = 20.0;
/// Room above the highest and under the lowest point.
const EDGE: f32 = 16.0;
/// A point snaps to this many dB.
const STEP_DB: f32 = 0.5;
/// How far past the outer bands the curve is drawn, in bands.
const OVERHANG: f32 = 0.45;

const BAND_NAMES: [&str; EQ_BANDS] = ["32", "64", "125", "250", "500", "1к", "2к", "4к", "8к", "16к"];

fn open_key() -> egui::Id {
    egui::Id::new("md3e-equalizer-open")
}

/// Opens the sheet (the «Эквалайзер» row of the settings).
pub(crate) fn open(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(open_key(), true));
}

/// The sheet, while it is open (and while it leaves).
pub(crate) fn overlay(ctx: &Context, state: &ViewState, out: &mut Vec<Command>) {
    let mut open = ctx.data(|d| d.get_temp::<bool>(open_key())).unwrap_or(false);
    let was = (state.prefs.eq_on, state.prefs.eq_gains);
    let mut now = was;
    let res = BottomSheet::new("md3e-equalizer")
        .height(FIELD_H + 190.0)
        .max_width(640.0)
        .show(ctx, &mut open, |ui| content(ui, &mut now.0, &mut now.1));
    if res.dismissed {
        open = false;
    }
    ctx.data_mut(|d| d.insert_temp(open_key(), open));
    if now != was {
        out.push(Command::SetPrefs(Box::new(Prefs {
            eq_on: now.0,
            eq_gains: now.1,
            ..state.prefs.clone()
        })));
    }
}

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

fn content(ui: &mut Ui, on: &mut bool, gains: &mut [f32; EQ_BANDS]) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.horizontal(|ui| {
        ui.label(TypeRole::TitleLarge.style().rich("Эквалайзер", s.on_surface));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(Switch::new(on).id_salt("eq-on"));
        });
    });
    ui.add_space(12.0);
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

/// The curve and its points.
fn field(ui: &mut Ui, on: &mut bool, gains: &mut [f32; EQ_BANDS]) {
    let s = Theme::get(ui.ctx()).scheme;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), FIELD_H + 24.0), Sense::hover());
    let (left, right) = (rect.left() + SIDE, rect.right() - SIDE);
    let (top, bottom) = (rect.top() + EDGE, rect.top() + FIELD_H - EDGE);
    let column = ((right - left) / (EQ_BANDS - 1) as f32).min(48.0);

    // The points first: what is drawn follows this frame's drag.
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
    // Off: the curve is there, quieter (it is what the switch would turn on).
    let ink = if *on { s.primary } else { s.on_surface_variant };
    let zero = db_y(top, bottom, 0.0);
    p.line_segment([pos2(rect.left(), zero), pos2(rect.right(), zero)], Stroke::new(1.0, s.outline_variant));

    // The curve the listener hears, not straight lines between the points:
    // neighbouring bands add up.
    let steps = 120;
    let (from, to) = (-OVERHANG, (EQ_BANDS - 1) as f32 + OVERHANG);
    let line: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let band = from + (to - from) * i as f32 / steps as f32;
            let db = eq_curve_db(gains, band_freq(band)).clamp(-EQ_MAX_DB * 1.25, EQ_MAX_DB * 1.25);
            pos2(band_x(left, right, band), db_y(top, bottom, db))
        })
        .collect();
    p.add(Shape::line(line, Stroke::new(2.0, ink)));

    for band in 0..EQ_BANDS {
        let x = band_x(left, right, band as f32);
        let at = pos2(x, db_y(top, bottom, gains[band]));
        let radius = if held == Some(band) {
            8.0
        } else if hovered == Some(band) {
            7.0
        } else {
            5.0
        };
        p.circle_filled(at, radius, ink);
        p.text(
            pos2(x, rect.top() + FIELD_H + 4.0),
            Align2::CENTER_TOP,
            BAND_NAMES[band],
            TypeRole::LabelSmall.font_id(),
            s.on_surface_variant,
        );
        if held == Some(band) {
            // Above the point, or under it at the top of the field.
            let (anchor, y) = if at.y - 14.0 - 16.0 < rect.top() {
                (Align2::CENTER_TOP, at.y + 14.0)
            } else {
                (Align2::CENTER_BOTTOM, at.y - 14.0)
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

    /// Drawn headless: the sheet opens from its key, shows the title, a
    /// chip per preset and a name per band, and sends nothing untouched.
    #[test]
    fn the_sheet_draws_and_sends_nothing_until_touched() {
        let ctx = Context::default();
        let mut fonts = egui::FontDefinitions::default();
        crate::typography::install_fonts(&mut fonts);
        ctx.set_fonts(fonts);
        crate::icons::install(&ctx);
        Theme::default().install(&ctx);
        let state = ViewState::default();
        let mut out = Vec::new();
        open(&ctx);
        let mut texts: Vec<String> = Vec::new();
        // Long enough for the sheet to have come up.
        for i in 0..40 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 700.0))),
                time: Some(f64::from(i) * 0.05),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                overlay(ui.ctx(), &state, &mut out);
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }

        assert!(out.is_empty(), "{out:?}");
        assert!(texts.iter().any(|t| t == "Эквалайзер"), "{texts:?}");
        for name in BAND_NAMES {
            assert!(texts.iter().any(|t| t == name), "no «{name}» in {texts:?}");
        }
    }
}
