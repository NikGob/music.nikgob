//! Runtime messages: cold start, home, recs, replica, connect, sync, lyrics, search, streams.

/// Static strings: runtime messages: cold start, home, recs, replica, connect, sync, lyrics, search, streams.
pub struct ActivityStrings {}

pub static RU: ActivityStrings = ActivityStrings {};

pub static EN: ActivityStrings = ActivityStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    activity_in -> ActivityStrings,
    /// Returns these strings for the active process locale.
    activity,
    { Ru => RU, En => EN }
}
