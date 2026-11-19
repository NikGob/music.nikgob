//! Search page.

/// Static strings: search page.
pub struct SearchStrings {}

pub static RU: SearchStrings = SearchStrings {};

pub static EN: SearchStrings = SearchStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    search_in -> SearchStrings,
    /// Returns these strings for the active process locale.
    search,
    { Ru => RU, En => EN }
}
