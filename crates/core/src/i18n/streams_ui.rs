//! Streaming pages and stream rows.

/// Static strings: streaming pages and stream rows.
pub struct StreamsUiStrings {}

pub static RU: StreamsUiStrings = StreamsUiStrings {};

pub static EN: StreamsUiStrings = StreamsUiStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    streams_ui_in -> StreamsUiStrings,
    /// Returns these strings for the active process locale.
    streams_ui,
    { Ru => RU, En => EN }
}
