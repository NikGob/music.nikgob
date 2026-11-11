//! What the platform does with the window: whether it can hide and come
//! back, and whether it is out of sight while neither minimized nor hidden.
//!
//! eframe already runs only the app's logic (no drawing) for a window that
//! is minimized, hidden or reported occluded: X11 reports a fully covered
//! window, and on Wayland a window the compositor stops sending frame
//! callbacks to (another workspace, under a fullscreen window) gets only
//! its logic run after a quarter of a second. Windows reports neither for
//! a window on another virtual desktop: DWM cloaks it, and that is asked
//! here.

use std::time::{Duration, Instant};

/// Whether `ViewportCommand::Visible(false)` really hides this window.
///
/// Wayland has no request to unmap a toplevel: winit's `set_visible` does
/// nothing there (and its `is_visible` does not know), so a window "hidden"
/// to the tray stayed on screen with nothing drawn in it (the compositor's
/// blur through an empty surface), and coming back depended on the
/// compositor still sending it frame callbacks. There the window is closed
/// and made again instead, as with «Быстрое открытие из трея» off. X11 and
/// Windows hide and show it.
pub fn can_hide(frame: &eframe::Frame) -> bool {
    let wayland = wayland(frame);
    if wayland {
        log::info!("window: Wayland cannot hide a window; closing it to the tray makes it again on show");
    }
    !wayland
}

#[cfg(target_os = "linux")]
fn wayland(frame: &eframe::Frame) -> bool {
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    frame
        .winit_window()
        .and_then(|w| w.window_handle().ok().map(|h| matches!(h.as_raw(), RawWindowHandle::Wayland(_))))
        .unwrap_or_else(|| std::env::var_os("WAYLAND_DISPLAY").is_some())
}

#[cfg(not(target_os = "linux"))]
fn wayland(_frame: &eframe::Frame) -> bool {
    false
}

/// How long a frame of a cloaked window waits: a few frames a second keep
/// its logic going, and nothing of it is seen.
pub const CLOAKED_FRAME: Duration = Duration::from_millis(250);

/// How often a window that is not cloaked is asked again.
const RECHECK: Duration = Duration::from_millis(250);

/// Whether DWM keeps the window out of sight (cloaked: on another virtual
/// desktop, say), asked a few times a second, and every frame while it is,
/// so the first frame back is not held.
#[derive(Default)]
pub struct Cloak {
    checked: Option<Instant>,
    cloaked: bool,
}

impl Cloak {
    pub fn cloaked(&mut self, frame: &eframe::Frame) -> bool {
        if self.cloaked || self.checked.is_none_or(|t| t.elapsed() >= RECHECK) {
            self.checked = Some(Instant::now());
            let now = cloaked(frame);
            if now != self.cloaked {
                log::info!("window: {}", if now { "cloaked (out of sight), drawn a few times a second" } else { "in sight again" });
            }
            self.cloaked = now;
        }
        self.cloaked
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn cloaked(frame: &eframe::Frame) -> bool {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use winit::raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    let Some(window) = frame.winit_window() else {
        return false;
    };
    let Ok(handle) = window.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return false;
    };
    let hwnd = h.hwnd.get() as *mut core::ffi::c_void;
    let mut cloaked: u32 = 0;
    // SAFETY: the window is ours and alive while the frame is; DWMWA_CLOAKED
    // writes one DWORD into `cloaked`, which lives for the whole call.
    let hr = unsafe {
        DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&raw mut cloaked).cast(), std::mem::size_of::<u32>() as u32)
    };
    hr == 0 && cloaked != 0
}

#[cfg(not(windows))]
fn cloaked(_frame: &eframe::Frame) -> bool {
    false
}
