//! Smaller runtime messages: track speed, the stream cache, export files.

/// Static strings of this domain.
pub struct MiscToastStrings {}

pub static RU: MiscToastStrings = MiscToastStrings {};

pub static EN: MiscToastStrings = MiscToastStrings {};

locale_table! {
    /// Returns this domain's strings for `loc`.
    misc_toasts_in -> MiscToastStrings,
    /// Returns this domain's strings for the active process locale.
    misc_toasts,
    { Ru => RU, En => EN }
}
