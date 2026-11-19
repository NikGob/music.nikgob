//! Album, artist and playlist detail pages.

/// Static strings: album, artist and playlist detail pages.
pub struct DetailStrings {}

pub static RU: DetailStrings = DetailStrings {};

pub static EN: DetailStrings = DetailStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    detail_in -> DetailStrings,
    /// Returns these strings for the active process locale.
    detail,
    { Ru => RU, En => EN }
}
