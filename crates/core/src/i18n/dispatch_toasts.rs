//! Toasts of the runtime's command dispatch (playlists, likes, queue).

/// Static strings of this domain.
pub struct DispatchToastStrings {}

pub static RU: DispatchToastStrings = DispatchToastStrings {};

pub static EN: DispatchToastStrings = DispatchToastStrings {};

locale_table! {
    /// Returns this domain's strings for `loc`.
    dispatch_toasts_in -> DispatchToastStrings,
    /// Returns this domain's strings for the active process locale.
    dispatch_toasts,
    { Ru => RU, En => EN }
}
