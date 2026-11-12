//! The services' own marks (assets/logos): Spotify, YouTube Music and
//! SoundCloud, in their brand colours, drawn from SVG at the size they are
//! shown. Only the mark carries the brand's colour; what stands around it
//! keeps the scheme's.

use egui::{Color32, Image, ImageSource, Rect, Ui};
use nikgob_view::Service;

const SPOTIFY: ImageSource<'static> = egui::include_image!("../../../../../assets/logos/spotify.svg");
const YOUTUBE_MUSIC: ImageSource<'static> = egui::include_image!("../../../../../assets/logos/youtube-music.svg");
const SOUNDCLOUD: ImageSource<'static> = egui::include_image!("../../../../../assets/logos/soundcloud.svg");

/// The mark of `service`, if it has one here (Deezer has none: it is a
/// catalogue the user never picks).
pub(crate) fn of(service: Service) -> Option<ImageSource<'static>> {
    match service {
        Service::Spotify => Some(SPOTIFY),
        Service::Youtube => Some(YOUTUBE_MUSIC),
        Service::Soundcloud => Some(SOUNDCLOUD),
        Service::Deezer => None,
    }
}

/// Paints `service`'s mark into `rect` (a square), faded by `alpha`.
pub(crate) fn paint(ui: &Ui, rect: Rect, service: Service, alpha: f32) {
    if let Some(src) = of(service) {
        Image::new(src).tint(Color32::WHITE.gamma_multiply(alpha)).paint_at(ui, rect);
    }
}
