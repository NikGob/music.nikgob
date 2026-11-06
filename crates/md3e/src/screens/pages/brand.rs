//! The app's mark: the NikGob sun over «Made by NikGob». It closes
//! Настройки (a click opens the author's GitHub) and stands in for Главная
//! while the page is built at the start of a run.

use egui::{Color32, CursorIcon, Image, ImageSource, Rect, Sense, Ui, pos2, vec2};

use crate::{Theme, TypeRole, components::anim};

const MARK: ImageSource<'static> = egui::include_image!("../../../../../assets/brand/nikgob-mark.svg");
const LABEL: &str = "Made by NikGob";
const GITHUB: &str = "https://github.com/NikGob";
/// The sun's side at the foot of Настройки, and on the loading page.
const SMALL: f32 = 40.0;
const LARGE: f32 = 96.0;
/// Between the sun and the label.
const GAP: f32 = 8.0;
/// One breath of the loading mark (seconds).
const BREATH: f64 = 1.8;

/// The sun over the label, centred in `rect`'s width from its top;
/// `alpha` fades both.
fn paint(ui: &Ui, rect: Rect, side: f32, color: Color32, alpha: f32) {
    let g = crate::typography::layout(ui.painter(), TypeRole::LabelLarge.style().job(LABEL, color));
    let sun = Rect::from_center_size(pos2(rect.center().x, rect.top() + side / 2.0), vec2(side, side));
    Image::new(MARK).tint(Color32::WHITE.gamma_multiply(alpha)).paint_at(ui, sun);
    let at = pos2(rect.center().x - g.size().x / 2.0, sun.bottom() + GAP);
    ui.painter().galley(at, g, color.gamma_multiply(alpha));
}

fn height(side: f32) -> f32 {
    side + GAP + TypeRole::LabelLarge.style().line_height
}

/// The foot of Настройки: the mark, centred; a click opens GitHub.
pub fn made_by(ui: &mut Ui) {
    let s = Theme::get(ui.ctx()).scheme;
    let width = ui.available_width();
    let (row, _) = ui.allocate_exact_size(vec2(width, height(SMALL) + 32.0), Sense::hover());
    let g = crate::typography::layout(ui.painter(), TypeRole::LabelLarge.style().job(LABEL, s.on_surface_variant));
    let hit = Rect::from_center_size(row.center(), vec2(g.size().x.max(SMALL) + 24.0, row.height() - 8.0));
    let resp = ui
        .interact(hit, ui.id().with("made-by-nikgob"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    if hover > 0.0 {
        ui.painter().rect_filled(hit, 12.0, s.on_surface.gamma_multiply(0.08 * hover));
    }
    let color = lerp(s.on_surface_variant, s.on_surface, hover);
    let top = row.center().y - height(SMALL) / 2.0;
    paint(ui, Rect::from_min_max(pos2(row.left(), top), row.max), SMALL, color, 0.85 + 0.15 * hover);
    if resp.clicked() {
        super::sources::open_url(GITHUB);
    }
}

/// Главная before its first build: the larger mark in the middle of the
/// view, breathing slowly (held still with reduced motion).
pub fn loading(ui: &mut Ui) {
    let s = Theme::get(ui.ctx()).scheme;
    let view = ui.clip_rect();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), (view.bottom() - ui.cursor().top()).max(LARGE * 2.0)), Sense::hover());
    let alpha = if Theme::get(ui.ctx()).reduced_motion {
        1.0
    } else {
        ui.ctx().request_repaint();
        let phase = (anim::now(ui.ctx()) / BREATH * std::f64::consts::TAU).cos();
        (0.75 - 0.25 * phase) as f32
    };
    let top = rect.center().y - height(LARGE) / 2.0;
    paint(ui, Rect::from_min_max(pos2(rect.left(), top), rect.max), LARGE, s.on_surface_variant, alpha);
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}
