//! The separate developer profile (`--demo-profile`, the app's `profile.rs`):
//! a player with its own library, settings and accounts for videos and UI
//! tests. The app says once at the start whether this run is that profile;
//! the interface then offers what only the profile has («Включить с
//! YouTube», its key) and hides what makes no sense inside it (the row that
//! opens and resets it).

use std::sync::atomic::{AtomicBool, Ordering};

static PROFILE: AtomicBool = AtomicBool::new(false);

/// Marks this run as the developer profile (the app, at the start).
pub fn set_profile(on: bool) {
    PROFILE.store(on, Ordering::Relaxed);
}

/// This run is the developer profile.
pub fn profile() -> bool {
    PROFILE.load(Ordering::Relaxed)
}

/// What Настройки → «Для разработчика» asks of the profile; the app does it
/// itself (the runtime never sees it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevProfile {
    /// «Открыть»: the profile starts beside this player (or its window
    /// comes up, when it runs).
    Open,
    /// «Сбросить»: the running profile quits, its folder is deleted and
    /// seeded again.
    Reset,
}
