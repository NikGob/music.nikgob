//! The app's mark: the NikGob sun with «Made by NikGob». It closes
//! Настройки (beside the label, the build's version in a pill under it; a
//! click opens the author's GitHub), and stands in for Главная, the sun over
//! the label, while the page is built at the start of a run.

use egui::{Color32, CursorIcon, Image, ImageSource, Rangef, Rect, Sense, Ui, pos2, vec2};

use crate::{Theme, TypeRole, components::anim};

const MARK: ImageSource<'static> = egui::include_image!("../../../../../assets/brand/nikgob-mark.svg");
const LABEL: &str = "Made by NikGob";
const GITHUB: &str = "https://github.com/NikGob";
/// The sun's side on the loading page.
const LARGE: f32 = 96.0;
/// Between the sun and the label.
const GAP: f32 = 8.0;
/// Between the label and the version under it.
const VERSION_GAP: f32 = 4.0;
/// One breath of the loading mark (seconds).
const BREATH: f64 = 1.8;

/// What the build calls itself: the package version in normal/release
/// builds, «admin» in developer builds (`--features dev`).
pub fn version() -> &'static str {
    if own_build() { "admin" } else { env!("CARGO_PKG_VERSION") }
}

/// The version as the foot of Настройки names it: «Версия 0.1.0»,
/// «Версия admin».
pub fn version_label() -> String {
    format!("Версия {}", version())
}

/// A developer build (`--features dev`): developer rows show.
pub fn own_build() -> bool {
    cfg!(feature = "dev")
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

/// The sun alone, `side` wide, centred in a row of its own (over the
/// first run's first question).
pub(super) fn mark(ui: &mut Ui, side: f32) {
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), side), Sense::hover());
    let sun = Rect::from_center_size(row.center(), vec2(side, side));
    Image::new(MARK).paint_at(ui, sun);
}

fn height(side: f32) -> f32 {
    side + GAP + TypeRole::LabelLarge.style().line_height
}

/// The sun's side at the foot of Настройки, beside the label.
const FOOT: f32 = 76.0;
/// Between the sun and the label beside it.
const FOOT_GAP: f32 = 20.0;
/// The foot of Настройки, centred on `page`: the sun with «Made by NikGob»
/// beside it and the version as plain text under the label, one row (a
/// click opens GitHub).
pub fn made_by(ui: &mut Ui, page: Rangef) {
    let s = Theme::get(ui.ctx()).scheme;
    let name = crate::typography::layout(ui.painter(), TypeRole::TitleMedium.style().job(LABEL, s.on_surface));
    let ver = crate::typography::layout(ui.painter(), TypeRole::BodySmall.style().job(&version_label(), s.on_surface_variant));
    let text_w = name.size().x.max(ver.size().x);
    let text_h = name.size().y + VERSION_GAP + ver.size().y;
    let group = vec2(FOOT + FOOT_GAP + text_w, FOOT.max(text_h));
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), group.y + 24.0), Sense::hover());
    let row = Rect::from_x_y_ranges(page, row.y_range());
    let rect = Rect::from_center_size(row.center(), group);
    let resp = ui
        .interact(rect, ui.id().with("made-by-nikgob"), Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand);
    let hover = ui.ctx().animate_bool(resp.id, resp.hovered());
    let sun = Rect::from_min_size(pos2(rect.left(), rect.center().y - FOOT / 2.0), vec2(FOOT, FOOT));
    Image::new(MARK).tint(Color32::WHITE.gamma_multiply(0.85 + 0.15 * hover)).paint_at(ui, sun);
    let x = sun.right() + FOOT_GAP;
    let top = rect.center().y - text_h / 2.0;
    let color = lerp(s.on_surface_variant, s.on_surface, hover);
    ui.painter().galley(pos2(x, top), name, color);
    let at = pos2(x, top + text_h - ver.size().y);
    ui.painter().galley(at, ver, s.on_surface_variant);
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
