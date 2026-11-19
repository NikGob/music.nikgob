//! Screens, panes and dialogs: now playing, queue pane, sweep, find song, tag editor, output, sleep timer.

/// Static strings: screens, panes and dialogs: now playing, queue pane, sweep, find song, tag editor, output, sleep timer.
pub struct ScreensStrings {}

pub static RU: ScreensStrings = ScreensStrings {};

pub static EN: ScreensStrings = ScreensStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    screens_in -> ScreensStrings,
    /// Returns these strings for the active process locale.
    screens,
    { Ru => RU, En => EN }
}
