//! «Качество» under a track's info: the technical facts of what plays and
//! what the spectrum check found, with the averaged spectrum as a plain
//! line chart and its cutoff marked. Quieter than the info above it.

use egui::{Pos2, Rect, Stroke, Ui, pos2, vec2};
use nikgob_view::{Command, SoundDetails, SpectrumPlot, TrackId};

use crate::{
    Icon, Theme, TypeRole,
    components::{Button, Size, Style},
};

/// Height of the chart, without its axis labels.
const CHART_H: f32 = 112.0;
/// The chart shows this many dB below its loudest band.
const RANGE_DB: f32 = 90.0;

/// The section of `track`'s file, with what can be done about it.
pub(super) fn section(ui: &mut Ui, d: &SoundDetails, track: Option<TrackId>, out: &mut Vec<Command>) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.add_space(20.0);
    ui.label(TypeRole::TitleSmall.style().rich("Качество", s.on_surface_variant));
    ui.add_space(8.0);
    if let Some(v) = &d.verdict {
        let color = if d.warning { s.error } else { s.on_surface };
        ui.label(TypeRole::BodyMedium.style().rich_para(v, color));
        ui.add_space(4.0);
    } else if d.pending {
        ui.label(TypeRole::BodySmall.style().rich("Проверка звука ещё не дошла до этого файла", s.on_surface_variant));
        ui.add_space(4.0);
    }
    if let Some(b) = &d.band {
        fact(ui, "Реальная полоса", b);
    }
    // Under the verdict: the MP3 a sure fake holds, and a better copy (a
    // doubt gets only the search).
    if let Some(track) = track.filter(|_| d.to_mp3 || d.better) {
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            let mp3 = Button::new(super::pages::TO_MP3).icon(Icon::Sync).style(Style::Tonal).size(Size::S);
            if d.to_mp3 && ui.add(mp3.id_salt("quality-to-mp3")).clicked() {
                out.push(Command::FakeToMp3(track));
            }
            let better = Button::new("Найти качество лучше").icon(Icon::Search).style(Style::Outlined).size(Size::S);
            if d.better && ui.add(better.id_salt("quality-better")).clicked() {
                out.push(Command::FindBetter(track));
            }
        });
        ui.add_space(4.0);
    }
    if let Some(p) = &d.spectrum {
        ui.add_space(8.0);
        chart(ui, p);
        ui.add_space(8.0);
    }
    for (k, v) in &d.facts {
        fact(ui, k, v);
    }
}

/// One quiet line: the name, then the value.
fn fact(ui: &mut Ui, k: &str, v: &str) {
    let s = Theme::get(ui.ctx()).scheme;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(TypeRole::LabelMedium.style().rich(k, s.on_surface_variant));
        ui.label(TypeRole::BodySmall.style().rich(v, s.on_surface));
    });
    ui.add_space(2.0);
}

/// The dB span the chart shows: from its loudest band (rounded up to 10)
/// down [`RANGE_DB`].
pub(crate) fn db_range(db: &[f32]) -> (f32, f32) {
    let top = db.iter().copied().filter(|d| d.is_finite()).fold(f32::NEG_INFINITY, f32::max);
    let top = if top.is_finite() { (top / 10.0).ceil() * 10.0 } else { 0.0 };
    (top - RANGE_DB, top)
}

/// The chart's line in `rect`: band `i` at its centre frequency, levels
/// below the span on the bottom edge.
pub(crate) fn line_points(p: &SpectrumPlot, rect: Rect) -> Vec<Pos2> {
    let n = p.db.len();
    if n == 0 {
        return Vec::new();
    }
    let (lo, hi) = db_range(&p.db);
    p.db
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let x = rect.left() + rect.width() * (i as f32 + 0.5) / n as f32;
            let t = ((d - lo) / (hi - lo)).clamp(0.0, 1.0);
            pos2(x, rect.bottom() - rect.height() * t)
        })
        .collect()
}

/// Tick frequencies, kHz: every 5 up to the top of the band.
pub(crate) fn ticks_khz(nyquist_hz: f32) -> Vec<u32> {
    (0..)
        .map(|k| k * 5)
        .take_while(|k| (*k as f32) * 1000.0 <= nyquist_hz)
        .collect()
}

fn chart(ui: &mut Ui, p: &SpectrumPlot) {
    let s = Theme::get(ui.ctx()).scheme;
    let width = ui.available_width().min(560.0);
    let label_h = TypeRole::LabelSmall.font_id().size + 6.0;
    let (outer, _) = ui.allocate_exact_size(vec2(width, CHART_H + label_h), egui::Sense::hover());
    let rect = Rect::from_min_size(outer.min, vec2(width, CHART_H));
    let painter = ui.painter_at(outer);
    let points = line_points(p, rect);
    if points.len() < 2 || p.nyquist_hz <= 0.0 {
        return;
    }
    // The area under the line, then the line.
    let fill = s.primary.gamma_multiply(0.14);
    let mut mesh = egui::Mesh::default();
    for w in points.windows(2) {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], fill);
        mesh.colored_vertex(w[1], fill);
        mesh.colored_vertex(pos2(w[1].x, rect.bottom()), fill);
        mesh.colored_vertex(pos2(w[0].x, rect.bottom()), fill);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base, base + 2, base + 3);
    }
    painter.add(egui::Shape::mesh(mesh));
    painter.add(egui::Shape::line(points, Stroke::new(1.5, s.primary)));
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, s.outline_variant));

    let x_of = |hz: f32| rect.left() + rect.width() * (hz / p.nyquist_hz).clamp(0.0, 1.0);
    let font = TypeRole::LabelSmall.font_id();
    for k in ticks_khz(p.nyquist_hz) {
        let x = x_of(k as f32 * 1000.0);
        painter.line_segment([pos2(x, rect.bottom()), pos2(x, rect.bottom() + 3.0)], Stroke::new(1.0, s.outline_variant));
        let align = if k == 0 { egui::Align2::LEFT_TOP } else { egui::Align2::CENTER_TOP };
        painter.text(pos2(x, rect.bottom() + 4.0), align, format!("{k}"), font.clone(), s.on_surface_variant);
    }
    painter.text(
        pos2(rect.right(), rect.bottom() + 4.0),
        egui::Align2::RIGHT_TOP,
        "кГц",
        font.clone(),
        s.on_surface_variant,
    );
    if let Some(cut) = p.cutoff_hz.filter(|c| *c > 0.0 && *c < p.nyquist_hz * 0.98) {
        let x = x_of(cut);
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], Stroke::new(1.0, s.outline));
        let text = nikgob_core::quality::khz(cut);
        // The mark's label sits left of the line, inside the chart.
        painter.text(pos2(x - 4.0, rect.top()), egui::Align2::RIGHT_TOP, text, font, s.on_surface);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chart_spans_ninety_db_under_its_loudest_band() {
        assert_eq!(db_range(&[-43.0, -80.0, -120.0]), (-130.0, -40.0));
        assert_eq!(db_range(&[]), (-90.0, 0.0));
        let p = SpectrumPlot { db: vec![-40.0, -130.0, -200.0, f32::NEG_INFINITY], nyquist_hz: 22_050.0, cutoff_hz: None };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 100.0));
        let pts = line_points(&p, rect);
        assert_eq!(pts.len(), 4);
        assert_eq!(pts[0], pos2(50.0, 0.0));
        // Below the span: on the bottom edge, never outside.
        assert!(pts[1..].iter().all(|q| q.y == 100.0));
        assert!(pts.iter().all(|q| rect.contains(*q)));
    }

    #[test]
    fn ticks_stop_at_the_top_of_the_band() {
        assert_eq!(ticks_khz(22_050.0), vec![0, 5, 10, 15, 20]);
        assert_eq!(ticks_khz(48_000.0), vec![0, 5, 10, 15, 20, 25, 30, 35, 40, 45]);
        assert_eq!(ticks_khz(0.0), vec![0]);
    }
}
