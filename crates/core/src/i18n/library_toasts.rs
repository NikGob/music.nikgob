//! Toasts of library edits (liked albums, tracks in the library, album choices).

/// Static strings of this domain.
pub struct LibraryToastStrings {}

pub static RU: LibraryToastStrings = LibraryToastStrings {};

pub static EN: LibraryToastStrings = LibraryToastStrings {};

locale_table! {
    /// Returns this domain's strings for `loc`.
    library_toasts_in -> LibraryToastStrings,
    /// Returns this domain's strings for the active process locale.
    library_toasts,
    { Ru => RU, En => EN }
}
