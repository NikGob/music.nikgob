//! The app's mark: the NikGob sun over «Made by NikGob». It closes
//! Настройки (a click opens the author's GitHub), with the version at the
//! page's right, and stands in for Главная while the page is built at the
//! start of a run.

use egui::{Align2, Color32, CursorIcon, Image, ImageSource, Rangef, Rect, Sense, Ui, pos2, vec2};

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

/// What the build calls itself: the version for a handout build (made by
/// scripts/release.ps1, which sets `NIKGOB_RELEASE`), «admin» for the
/// owner's own builds.
pub fn version() -> &'static str {
    match option_env!("NIKGOB_RELEASE") {
        Some(_) => env!("CARGO_PKG_VERSION"),
        None => "admin",
    }
}

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

/// The foot of Настройки across the page's width `page`: the mark in its
/// middle (a click opens GitHub), the version at its right on the label's
/// line.
pub fn made_by(ui: &mut Ui, page: Rangef) {
    let s = Theme::get(ui.ctx()).scheme;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), height(SMALL) + 24.0), Sense::hover());
    let row = Rect::from_x_y_ranges(page, row.y_range());
    let g = crate::typography::layout(ui.painter(), TypeRole::LabelLarge.style().job(LABEL, s.on_surface_variant));
    let top = row.center().y - height(SMALL) / 2.0;
    let hit = Rect::from_center_size(pos2(row.center().x, top + height(SMALL) / 2.0), vec2(g.size().x.max(SMALL), height(SMALL)));
    let resp = ui
        .interact(hit, ui.id().with("made-by-nikgob"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    let color = lerp(s.on_surface_variant, s.on_surface, hover);
    paint(ui, Rect::from_min_max(pos2(row.left(), top), row.max), SMALL, color, 0.85 + 0.15 * hover);
    let line = top + SMALL + GAP + TypeRole::LabelLarge.style().line_height / 2.0;
    ui.painter().text(
        pos2(row.right() - 16.0, line),
        Align2::RIGHT_CENTER,
        version(),
        TypeRole::LabelSmall.style().font_id(),
        s.on_surface_variant,
    );
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
