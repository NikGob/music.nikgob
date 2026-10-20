//! Recommendations (docs/stage4.md). Pure logic: no database, no network,
//! no decoding; the runtime feeds it samples and library rows.
//!
//! * [`analysis`]: bliss features of a track from its decoded PCM (mono
//!   22 050 Hz, the first [`MAX_SECONDS`]), tagged [`ANALYZED_WITH`].
//! * [`index`]: every vector of the library in memory, z-normalised over the
//!   library itself, with brute-force nearest neighbours.

pub mod analysis;
pub mod index;

pub use analysis::{ANALYZED_WITH, AnalysisError, DIM, MAX_SECONDS, PcmAnalyzer, SAMPLE_RATE, Vector, analyze_pcm};
pub use index::VectorIndex;
