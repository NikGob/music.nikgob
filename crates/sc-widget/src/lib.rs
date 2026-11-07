//! SoundCloud's official player for the tracks SoundCloud streams only
//! with DRM (`StreamError::Drm`, major labels' `cbc-/ctr-encrypted-hls`).
//!
//! Nothing is decrypted here. SoundCloud's own embed widget
//! (w.soundcloud.com/player, in an iframe allowed `autoplay;
//! encrypted-media`, driven with its SC.Widget postMessage API) plays the
//! track in a webview that is never shown; the system's Widevine CDM in
//! WebView2 does what it does in any browser. Windows only: WebKitGTK has
//! no Widevine, so elsewhere such tracks keep their old fallback.
//!
//! * The widget host is a child run of the app, `nikgob --sc-widget
//!   <profile dir>` (the app's `scwidget` module): an off-screen window
//!   with the webview, commands on its stdin, the widget's events on its
//!   stdout ([`protocol`], the page in `page.html`). Its own process tree
//!   keeps its sound apart from the app's and from the sign-in windows'.
//! * [`ScWidget`] starts it for the first track, keeps it between tracks
//!   and closes it after a few idle minutes; it routes the host's sound
//!   into our engine exactly like Spotify Soloist's
//!   (`nikgob_soloist::route`: WASAPI process loopback of the host's tree,
//!   the audio sessions of the tree at 1e-6 and undone on the captured copy,
//!   a stalled capture dropped so the widget is heard itself).
//! * [`WidgetTrack`] is one track as a live source of the engine: start,
//!   seek, end and errors come from the widget's events. Crossfade and
//!   gapless cannot apply (one player, a clean cut, as for Soloist).

pub mod manager;
pub mod protocol;
pub mod track;

pub use manager::{Config, ScWidget};
pub use track::WidgetTrack;

/// The page the host shows (served from its own origin, a secure context).
pub const PAGE: &str = include_str!("page.html");
