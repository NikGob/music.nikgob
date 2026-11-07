//! A sound model stronger than bliss (docs/recs-graph.md §7): a trained
//! embedding (discogs-effnet, CLAP, MERT through ONNX) that hears the genre
//! and the style bliss does not. None is built in: the ones weighed cost two
//! to ten times bliss's time per track on the CPU and need a native runtime
//! and a model of 18 MB to 600 MB, too much for the background analysis of
//! a large library; the graph of similar music ([`crate::graph`]) gives the
//! likeness of genre for free. When one comes, it implements this trait,
//! its vectors are kept beside bliss's under [`SoundModel::name`], and they
//! join as a second space of sound in the taste profiles
//! ([`crate::profiles`]) and the `sound` feature of the model of the next
//! start; downloading it is a setting, off by default.

use crate::analysis::AnalysisError;

/// A model of sound: mono PCM at its own rate in, a vector out.
pub trait SoundModel: Send + Sync {
    /// The model and its version, kept with each vector (like
    /// [`crate::ANALYZED_WITH`]): another one is analysed again.
    fn name(&self) -> &str;
    /// The length of its vectors.
    fn dim(&self) -> usize;
    /// The rate of the PCM it reads (16 000 Hz for discogs-effnet).
    fn sample_rate(&self) -> u32;
    /// The vector of a track's mono PCM at [`Self::sample_rate`].
    fn embed(&self, mono: &[f32]) -> Result<Vec<f32>, AnalysisError>;
}
