//! Small helpers shared by MD3E screens.

use std::time::Duration;

use egui::{Color32, Rect, Stroke, Ui, vec2};
use nikgob_view::{Cover, SourceKind};

use crate::{
    Icon, Theme,
    components::{Radii, paint},
};

/// `m:ss` or `h:mm:ss`.
pub fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// "42 сек" / "47 мин" / "1 ч 12 мин".
pub fn fmt_total(d: Duration) -> String {
    let m = d.as_secs() / 60;
    if m == 0 {
        format!("{} сек", d.as_secs())
    } else if m >= 60 {
        format!("{} ч {} мин", m / 60, m % 60)
    } else {
        format!("{m} мин")
    }
}

/// Russian plural: 1 трек, 2 трека, 5 треков.
pub fn plural<'a>(n: u64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let (n10, n100) = (n % 10, n % 100);
    if n10 == 1 && n100 != 11 {
        one
    } else if (2..=4).contains(&n10) && !(12..=14).contains(&n100) {
        few
    } else {
        many
    }
}

/// A cover image in a rounded rect, or a tonal placeholder with a note icon.
pub fn cover(ui: &Ui, cover: Option<&Cover>, rect: Rect, radius: f32) {
    let s = Theme::get(ui.ctx()).scheme;
    let radii = Radii::all(radius);
    match cover {
        Some(c) => {
            paint::container(ui.painter(), rect, radii, s.surface_container_highest, Stroke::NONE);
            if let Some(texture) = crate::covers::texture(ui.ctx(), &c.uri(), rect.size(), 1.0) {
                egui::Image::from_texture(texture)
                    .texture_options(crate::components::PHOTO_TEXTURE)
                    .corner_radius(radius.min(255.0) as u8)
                    .paint_at(ui, rect);
            }
        }
        None => {
            paint::container(ui.painter(), rect, radii, s.secondary_container, Stroke::NONE);
            let icon = (rect.width() * 0.4).clamp(16.0, 96.0);
            Icon::MusicNote
                .widget(false, s.on_secondary_container, icon)
                .paint_at(ui, Rect::from_center_size(rect.center(), vec2(icon, icon)));
        }
    }
}

/// The source badge on a cover corner: nothing for local files.
pub fn source_badge(ui: &Ui, rect: Rect, source: Option<SourceKind>) {
    let s = Theme::get(ui.ctx()).scheme;
    // The scheme's colours, not the services' brand green and red.
    let icon = match source {
        Some(SourceKind::Spotify) => Icon::GraphicEq,
        Some(SourceKind::Youtube) => Icon::PlayArrow,
        _ => return,
    };
    let color = s.on_surface_variant;
    let size = (rect.width() * 0.4).clamp(14.0, 22.0);
    let b = Rect::from_min_size(rect.right_bottom() - vec2(size, size), vec2(size, size));
    ui.painter().circle_filled(b.center(), size * 0.5, s.surface_container_highest);
    icon.widget(true, color, size * 0.7)
        .paint_at(ui, Rect::from_center_size(b.center(), vec2(size * 0.7, size * 0.7)));
}

/// Ellipsized single-line text.
pub fn label(ui: &mut Ui, text: &str, role: crate::TypeRole, color: Color32) -> egui::Response {
    ui.add(
        egui::Label::new(role.style().rich(text, color))
            .truncate()
            .selectable(false),
    )
}

/// A stream still opening: the play button's icon gives way to a loading
/// indicator in the button's own colours (`bg` hides the icon).
pub fn buffering_overlay(ui: &mut Ui, button: Rect, bg: Color32, fg: Color32) {
    let size = (button.height().min(button.width()) * 0.62).max(20.0);
    let c = button.center();
    ui.painter().circle_filled(c, size * 0.5, bg);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_center_size(c, vec2(size, size))).id_salt(("buffering", button.min.x as i32, button.min.y as i32)));
    child.add(crate::components::LoadingIndicator::new().size(size).color(fg));
}

/// "звук: YouTube" for a stream (None for a local file).
pub fn audio_label(state: &nikgob_view::ViewState) -> Option<String> {
    let service = state.player.audio_service?;
    let name = match service {
        nikgob_view::Service::Spotify => "Spotify",
        nikgob_view::Service::Youtube => "YouTube",
    };
    Some(format!("звук: {name}"))
}
