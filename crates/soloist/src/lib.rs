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
//! The plan was to capture Soloist's sound and play only our copy, so the
//! engine's volume, normalization and visualizer act on it. On Windows that
//! needs Soloist's own output silenced while the capture still hears it, and
//! the test in `tests/loopback_mute.rs` shows it cannot be: muting Soloist's
//! audio session or turning it to 0 silences the process-loopback capture
//! too. So Soloist stays the audible copy. The capture feeds the engine an
//! inaudible copy (the engine plays a Soloist item at volume 0; its
//! visualizer reads before the volume), so the spectrum and visualizer still
//! move, and the position, the end of the track and errors reach the queue
//! like any other source's. The user's volume becomes the level of Soloist's
//! audio session through the engine's own curve (Soloist itself is kept at
//! 100), and the captured copy is scaled back up by the same factor.
//!
//! What cannot apply to a live stream played by another program: crossfade
//! and gapless into or out of a Soloist track (one player, no overlap: a
//! clean cut), normalization (Soloist does its own), a choice of quality.

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
