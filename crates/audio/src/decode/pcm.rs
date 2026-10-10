//! Adapter from the public [`PcmSource`] trait to the internal frame source.

use std::time::Duration;

use super::{FrameSource, ReadStatus};
use crate::types::{DecodeError, PcmRead, PcmSource};

const CHUNK_FRAMES: usize = 1024;

pub(crate) struct PcmFrames {
    source: Box<dyn PcmSource>,
    rate: u32,
    channels: usize,
    scratch: Vec<f32>,
}

impl PcmFrames {
    pub(crate) fn new(source: Box<dyn PcmSource>) -> Result<Self, DecodeError> {
        let rate = source.sample_rate();
        let channels = usize::from(source.channels());
        if rate == 0 || channels == 0 {
            return Err(DecodeError::Unsupported(
                "pcm source with zero rate or channels".into(),
            ));
        }
        Ok(Self {
            source,
            rate,
            channels,
            scratch: vec![0.0; CHUNK_FRAMES * channels],
        })
    }
}

impl FrameSource for PcmFrames {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn total_frames(&self) -> Option<u64> {
        self.source
            .duration()
            .map(|d| (d.as_secs_f64() * f64::from(self.rate)).round() as u64)
    }

    fn read(&mut self, out: &mut Vec<f32>) -> Result<ReadStatus, DecodeError> {
        match self
            .source
            .read(&mut self.scratch)
            .map_err(|e| DecodeError::Pcm(e.to_string()))?
        {
            PcmRead::Frames(0) | PcmRead::Pending => Ok(ReadStatus::Pending),
            PcmRead::Frames(n) => {
                let n = n.min(CHUNK_FRAMES);
                out.extend_from_slice(&self.scratch[..n * self.channels]);
                Ok(ReadStatus::Data)
            }
            PcmRead::End => Ok(ReadStatus::Eof),
        }
    }

    fn seek(&mut self, frame: u64) -> Result<(), DecodeError> {
        let pos = Duration::from_secs_f64(frame as f64 / f64::from(self.rate));
        if self.source.seek(pos) {
            Ok(())
        } else {
            Err(DecodeError::Seek("this source cannot seek".into()))
        }
    }
}
