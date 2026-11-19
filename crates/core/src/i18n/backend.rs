//! Backend crates: core names and sources, soloist, streaming services, library, metadata, playback.

/// Static strings: backend crates: core names and sources, soloist, streaming services, library, metadata, playback.
pub struct BackendStrings {}

pub static RU: BackendStrings = BackendStrings {};

pub static EN: BackendStrings = BackendStrings {};

locale_table! {
    /// Returns these strings for `loc`.
    backend_in -> BackendStrings,
    /// Returns these strings for the active process locale.
    backend,
    { Ru => RU, En => EN }
}
