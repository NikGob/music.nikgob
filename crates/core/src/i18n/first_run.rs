//! First-run wizard.

/// Static strings: first-run wizard.
pub struct FirstRunStrings {}

pub static RU: FirstRunStrings = FirstRunStrings {};

pub static EN: FirstRunStrings = FirstRunStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    first_run_in -> FirstRunStrings,
    /// Returns these strings for the active process locale.
    first_run,
    { Ru => RU, En => EN }
}
