//! Lyrics against a song's own audio (docs/lyrics-align.md).
//!
//! * [`separate`]: the voice out of the mix (MDX-Net).
//! * [`ctc`] and [`text`]: forced alignment of a known text to the voice,
//!   piece by piece (words; characters of Japanese).
//! * [`syllables`]: the made words cut into syllables timed by the voice.
//! * [`fit`]: whether a fetched text is this recording's, how far its line
//!   times are off, and its word timing made from the alignment.
//! * [`whisper`]: a text from scratch when there is none anywhere.
//! * [`catalog`]: the runtime and models, downloaded on first use.
//!
//! Everything here is blocking and CPU or GPU heavy: the caller runs it on
//! a background thread of low priority.

pub mod audio;
pub mod catalog;
pub mod ctc;
pub mod engine;
pub mod fit;
pub mod pipeline;
pub mod separate;
pub mod syllables;
pub mod text;
pub mod whisper;

pub use catalog::{Catalog, ModelId, ModelState};
pub use engine::Device;
pub use pipeline::{Aligner, Prepared, Stage};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file cannot be read now (missing, a drive not there).
    #[error("unavailable: {0}")]
    Unavailable(String),
    #[error("audio: {0}")]
    Audio(String),
    #[error("runtime: {0}")]
    Runtime(String),
    #[error("model: {0}")]
    Model(String),
    #[error("download: {0}")]
    Download(String),
    #[error("{0} is not downloaded")]
    Missing(&'static str),
    #[error("stopped")]
    Stopped,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
