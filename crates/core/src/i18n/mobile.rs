//! Phone UI (crates/mobile).

/// Static strings: phone UI (crates/mobile).
pub struct MobileStrings {}

pub static RU: MobileStrings = MobileStrings {};

pub static EN: MobileStrings = MobileStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    mobile_in -> MobileStrings,
    /// Returns these strings for the active process locale.
    mobile,
    { Ru => RU, En => EN }
}
