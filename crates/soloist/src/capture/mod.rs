//! The sound Soloist plays, captured from its process so it can go through
//! our engine (see the crate docs for what the capture can and cannot do).

#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use self::windows::{CHANNELS, CaptureOptions, CaptureStats, ProcessCapture, RATE, set_sessions, set_sessions_read_back};
