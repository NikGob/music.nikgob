//! Library page: tabs, sorting, empty states.

/// Static strings: library page: tabs, sorting, empty states.
pub struct LibraryStrings {}

pub static RU: LibraryStrings = LibraryStrings {};

pub static EN: LibraryStrings = LibraryStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    library_in -> LibraryStrings,
    /// Returns these strings for the active process locale.
    library,
    { Ru => RU, En => EN }
}
