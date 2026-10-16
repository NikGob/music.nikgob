//! The window's title bar in the interface colours, so the window reads as
//! one surface with no system-coloured strip on top.
//!
//! The caption stays the system's own (drag, double-click to maximize,
//! resize edges, Snap Layouts on the maximize button, the system menu all
//! keep working); on Windows 11 it is painted through winit's safe DWM
//! wrappers: background in the theme's `surface`, title in `on-surface`,
//! light or dark caption buttons to match. It follows theme switches and
//! the cover-coloured scheme as it animates. Older Windows ignore the
//! colours and keep a dark or light caption.

use egui::{Color32, Context, SystemTheme, ViewportCommand};
use nikgob_view::ThemeMode;

#[derive(Default)]
pub struct TitleBar {
    /// What was last applied: background, text, the caption buttons' theme.
    applied: Option<(Color32, Color32, Option<bool>)>,
}

/// The caption buttons' theme for the app's theme setting: forced when the
/// app forces one, left to the system when the app follows it (forcing it
/// would stop the window from hearing the system switch).
fn caption_theme(mode: ThemeMode) -> Option<bool> {
    match mode {
        ThemeMode::Dark => Some(true),
        ThemeMode::Light => Some(false),
        ThemeMode::System => None,
    }
}

impl TitleBar {
    /// Call once per frame, after the interface installed this frame's theme.
    pub fn sync(&mut self, ctx: &Context, frame: &eframe::Frame, mode: ThemeMode) {
        let theme = nikgob_md3e::Theme::get(ctx);
        let want = (theme.scheme.surface, theme.scheme.on_surface, caption_theme(mode));
        if self.applied == Some(want) {
            return;
        }
        let first = self.applied.is_none();
        if first || self.applied.is_some_and(|a| a.2 != want.2) {
            ctx.send_viewport_cmd(ViewportCommand::SetTheme(match want.2 {
                Some(true) => SystemTheme::Dark,
                Some(false) => SystemTheme::Light,
                None => SystemTheme::SystemDefault,
            }));
        }
        if paint(frame, want.0, want.1) {
            self.applied = Some(want);
        }
    }
}

#[cfg(windows)]
fn paint(frame: &eframe::Frame, bg: Color32, fg: Color32) -> bool {
    use winit::platform::windows::{Color, WindowExtWindows};
    let Some(window) = frame.winit_window() else {
        return false;
    };
    let c = |c: Color32| Color::from_rgb(c.r(), c.g(), c.b());
    window.set_title_background_color(Some(c(bg)));
    window.set_title_text_color(c(fg));
    true
}

#[cfg(not(windows))]
fn paint(_frame: &eframe::Frame, _bg: Color32, _fg: Color32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_buttons_follow_the_system_only_when_the_app_does() {
        assert_eq!(caption_theme(ThemeMode::Dark), Some(true));
        assert_eq!(caption_theme(ThemeMode::Light), Some(false));
        assert_eq!(caption_theme(ThemeMode::System), None);
    }
}
