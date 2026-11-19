//! Shared track/album/artist context menu items.

/// Static strings: shared track/album/artist context menu items.
pub struct MenusStrings {}

pub static RU: MenusStrings = MenusStrings {};

pub static EN: MenusStrings = MenusStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    menus_in -> MenusStrings,
    /// Returns these strings for the active process locale.
    menus,
    { Ru => RU, En => EN }
}
