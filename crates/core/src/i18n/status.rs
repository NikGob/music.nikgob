//! Import, wishlist and torrent statuses rendered from runtime state.

/// Static strings: import, wishlist and torrent statuses rendered from runtime state.
pub struct StatusStrings {}

pub static RU: StatusStrings = StatusStrings {};

pub static EN: StatusStrings = StatusStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    status_in -> StatusStrings,
    /// Returns these strings for the active process locale.
    status,
    { Ru => RU, En => EN }
}
