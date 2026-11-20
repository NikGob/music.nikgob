//! Small helpers shared by MD3E screens.

use std::time::Duration;

use egui::{Color32, Rect, Stroke, Ui, vec2};
use nikgob_view::{AlbumDate, Cover, ReleaseDay};

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
    nikgob_core::i18n::fmt_total(d)
}

/// An album's release date as [`AlbumDate`] says: «2026», «Октябрь 2026»
/// or «23 октября 2026». What is not known falls back to what is (no day:
/// the month and year; no month: the year); `None` without even a year.
pub fn fmt_album_date(released: Option<ReleaseDay>, year: Option<i32>, how: AlbumDate) -> Option<String> {
    let of = nikgob_core::i18n::screens::months_gen();
    let month = nikgob_core::i18n::screens::months_nom();
    let Some(d) = released else { return year.map(|y| y.to_string()) };
    let m = usize::from(d.month.clamp(1, 12) - 1);
    Some(match (how, d.day) {
        (AlbumDate::Year, _) => d.year.to_string(),
        (AlbumDate::Full, Some(day)) => format!("{day} {} {}", of[m], d.year),
        (AlbumDate::MonthYear | AlbumDate::Full, _) => format!("{} {}", month[m], d.year),
    })
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

/// What plays, in one quiet line: «YouTube · Opus ~160 кбит/с» for a
/// stream (where its sound really comes from: a Spotify track may play
/// from YouTube Music), «YouTube · клип · Opus ~160 кбит/с» for a music
/// video without its song, the file's own format for a local track — a
/// temporary copy (saved from YouTube Music until a real file is found)
/// names where it came from, like a stream does.
pub fn sound_line(state: &nikgob_view::ViewState) -> Option<String> {
    let p = &state.player;
    let clip = p.current.as_ref().is_some_and(|t| t.clip);
    let quality = p.audio_quality.as_deref().or_else(|| p.current.as_ref().and_then(|t| t.quality.as_deref()));
    let service = p
        .audio_service
        // The file is local, but it is a stand-in: said where it came from.
        .or_else(|| p.current.as_ref().filter(|t| t.temporary).map(|_| nikgob_view::Service::Youtube))
        .map(|s| match s {
            nikgob_view::Service::Spotify => "Spotify",
            // A Deezer entry plays from YouTube Music.
            nikgob_view::Service::Youtube | nikgob_view::Service::Deezer => "YouTube",
            nikgob_view::Service::Soundcloud => "SoundCloud",
        });
    let service = service.map(|s| if clip { format!("{s} · {}", nikgob_core::i18n::screens().clip_word) } else { s.to_owned() });
    match (service, quality) {
        (Some(s), Some(q)) => Some(format!("{s} · {q}")),
        (Some(s), None) => Some(s),
        (None, Some(q)) => Some(q.to_owned()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use nikgob_view::{AlbumDate, ReleaseDay, TrackId};

    /// A playing track with a quality of its own.
    fn current(quality: Option<&str>, temporary: bool) -> nikgob_view::TrackRow {
        nikgob_view::TrackRow {
            id: TrackId::new(),
            title: "Песня".into(),
            artist: "Кино".into(),
            album: None,
            release_id: None,
            artists: Default::default(),
            disc_no: 1,
            track_no: 1,
            duration: None,
            quality: quality.map(str::to_owned),
            source: None,
            has_local: true,
            liked: false,
            cover: None,
            clip: false,
            in_library: true,
            temporary,
        }
    }

    #[test]
    fn a_temporary_copy_names_its_service_like_a_stream() {
        use nikgob_view::ViewState;
        // A real file: its own format, no service.
        let mut state = ViewState::default();
        state.player.current = Some(current(Some("Opus · 160"), false));
        assert_eq!(super::sound_line(&state).as_deref(), Some("Opus · 160"));
        // A temporary copy: where it came from, before the format.
        state.player.current = Some(current(Some("Opus · 160"), true));
        assert_eq!(super::sound_line(&state).as_deref(), Some("YouTube · Opus · 160"));
        // What plays is the stream itself: its service was there already.
        state.player.audio_service = Some(nikgob_view::Service::Youtube);
        state.player.audio_quality = Some("Opus ~160 кбит/с".into());
        assert_eq!(super::sound_line(&state).as_deref(), Some("YouTube · Opus ~160 кбит/с"));
    }

    #[test]
    fn album_dates_read_in_russian_as_chosen() {
        let day = Some(ReleaseDay { year: 2026, month: 10, day: Some(23) });
        let month = Some(ReleaseDay { year: 2003, month: 12, day: None });
        let f = super::fmt_album_date;
        assert_eq!(f(day, Some(2026), AlbumDate::Year).as_deref(), Some("2026"));
        assert_eq!(f(day, Some(2026), AlbumDate::MonthYear).as_deref(), Some("Октябрь 2026"));
        assert_eq!(f(day, Some(2026), AlbumDate::Full).as_deref(), Some("23 октября 2026"));
        // Only the month known: the full date is the month and year.
        assert_eq!(f(month, None, AlbumDate::Full).as_deref(), Some("Декабрь 2003"));
        assert_eq!(f(month, None, AlbumDate::Year).as_deref(), Some("2003"));
        // Only the year known: every option is the year.
        for how in [AlbumDate::Year, AlbumDate::MonthYear, AlbumDate::Full] {
            assert_eq!(f(None, Some(1999), how).as_deref(), Some("1999"));
            assert_eq!(f(None, None, how), None);
        }
    }
}
