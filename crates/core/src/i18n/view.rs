//! Labels of view-model enums (crates/view).

/// Static strings: labels of view-model enums (crates/view).
pub struct ViewStrings {}

pub static RU: ViewStrings = ViewStrings {};

pub static EN: ViewStrings = ViewStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    view_in -> ViewStrings,
    /// Returns these strings for the active process locale.
    view,
    { Ru => RU, En => EN }
}
