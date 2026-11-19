//! Smaller pages and shared page widgets (cards, shelves, marks, sound, equalizer, home).

/// Static strings: smaller pages and shared page widgets (cards, shelves, marks, sound, equalizer, home).
pub struct PagesStrings {}

pub static RU: PagesStrings = PagesStrings {};

pub static EN: PagesStrings = PagesStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    pages_in -> PagesStrings,
    /// Returns these strings for the active process locale.
    pages,
    { Ru => RU, En => EN }
}
