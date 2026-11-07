//! Frame sources: everything the engine can pull interleaved `f32` from.

mod opus;
mod pcm;
mod symphonia_src;

use std::path::Path;

pub(crate) use pcm::PcmFrames;
pub(crate) use symphonia_src::{StreamInfo, SymphoniaSource};

use crate::types::{ByteSource, DecodeError};

/// Outcome of one [`FrameSource::read`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadStatus {
    /// Some frames were appended to the output.
    Data,
    /// Nothing right now; ask again shortly (live sources).
    Pending,
    /// The stream is over.
    Eof,
}

/// A decoded stream of interleaved `f32` frames at the source's own rate.
///
/// Positions are frame indices on the *playable* timeline: encoder delay,
/// Opus pre-skip and end padding are already excluded, so two files that are
/// consecutive pieces of one recording concatenate sample-exactly.
pub(crate) trait FrameSource: Send {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// Playable length in frames, when the container says.
    fn total_frames(&self) -> Option<u64>;
    /// Appends decoded frames (whole frames only) to `out`.
    fn read(&mut self, out: &mut Vec<f32>) -> Result<ReadStatus, DecodeError>;
    /// Repositions so the next frame read is frame `frame` (sample accurate).
    fn seek(&mut self, frame: u64) -> Result<(), DecodeError>;
}

/// Opens a file as a frame source.
pub(crate) fn open_file(path: &Path) -> Result<SymphoniaSource, DecodeError> {
    SymphoniaSource::open(path)
}

/// Opens the encoded bytes of a stream as a frame source.
pub(crate) fn open_stream(
    bytes: Box<dyn ByteSource>,
) -> Result<(SymphoniaSource, StreamInfo), DecodeError> {
    SymphoniaSource::open_stream(bytes)
}
