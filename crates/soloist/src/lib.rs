//! Spotify Soloist as the player's Spotify sound.
//!
//! Soloist is Spotify's official headless Spotify Connect player with a
//! local WebSocket control API (developer.spotify.com/documentation/soloist).
//! Spotify refuses librespot's audio keys to many Premium accounts lately
//! (librespot #1649); Soloist is Spotify's own answer, so when it is set up
//! and paired, Spotify tracks play through it and librespot (then YouTube
//! Music) stays the fallback.
//!
//! * [`install`]: the official builds, downloaded from Spotify's CDN only
//!   when the user asks, unpacked into versioned folders, renewed before
//!   they expire.
//! * [`daemon`]: the child process and its API port.
//! * [`client`] / [`protocol`]: the WebSocket API.
//! * [`Soloist`]: the setup steps (download → the user's key → choosing the
//!   device once in a Spotify app), restarts, the handle tracks play through.
//! * [`SoloistTrack`]: one track as a live source of our engine.
//! * [`capture`]: Soloist's sound captured from its process (Windows).
//!
//! # Where the sound goes
//!
//! Soloist's sound goes through our engine, so the engine's volume,
//! normalization, visualizer and output device act on it as on any track.
//! On Windows the capture is WASAPI process loopback of Soloist's process
//! tree; the copy Soloist itself plays has to be inaudible while the
//! capture still hears it. Muting its audio session or a level of 0
//! silences the capture too (`tests/loopback_mute.rs`), but any level above
//! 0 is a plain float factor before the capture: the session is set to
//! 1e-6 (-120 dB) and the captured sound is scaled back up, as clean as at
//! full level (`examples/route_bench.rs` measured it,
//! with no gaps over minutes, ~160 ms added delay and 2-3 % of one core).
//! Soloist's own volume stays at 100. Without a capture (other systems, an
//! older Windows, a failure) Soloist is heard itself: the user's volume
//! becomes its session level and the engine plays its track at volume 0.
//!
//! What cannot apply to a live stream: crossfade and gapless into or out of
//! a Soloist track (one player, no overlap: a clean cut), a choice of
//! quality.

pub mod capture;
pub mod client;
pub mod daemon;
pub mod install;
#[cfg(windows)]
mod job;
mod manager;
pub mod protocol;
pub mod track;

pub use manager::{BoxFuture, Config, KeyStore, LiveView, Soloist, default_device_name};
pub use track::SoloistTrack;
