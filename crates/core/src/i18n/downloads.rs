//! Downloads and wishlist pages.

/// Static strings: downloads and wishlist pages.
pub struct DownloadsStrings {}

pub static RU: DownloadsStrings = DownloadsStrings {};

pub static EN: DownloadsStrings = DownloadsStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    downloads_in -> DownloadsStrings,
    /// Returns these strings for the active process locale.
    downloads,
    { Ru => RU, En => EN }
}
