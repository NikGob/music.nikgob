//! Spotify sign-in: the browser page and the login failures.

/// Static strings of this domain.
pub struct SpotifyAuthStrings {}

pub static RU: SpotifyAuthStrings = SpotifyAuthStrings {};

pub static EN: SpotifyAuthStrings = SpotifyAuthStrings {};

locale_table! {
    /// Returns this domain's strings for `loc`.
    spotify_auth_in -> SpotifyAuthStrings,
    /// Returns this domain's strings for the active process locale.
    spotify_auth,
    { Ru => RU, En => EN }
}
