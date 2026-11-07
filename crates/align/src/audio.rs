//! A song's audio for the models: decoded with the importer's decoder and
//! resampled whole (rubato's FFT resampler), at most [`MAX_SECONDS`].

use std::ops::ControlFlow;
use std::path::Path;

use rubato::audioadapter_buffers::direct::{SequentialSliceOfSlices, SequentialSliceOfVecs};
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::Error;

/// Longer files (mixes, disc images) are not aligned.
pub const MAX_SECONDS: u32 = 20 * 60;
const CHUNK: usize = 4096;

/// Planar audio at one rate.
#[derive(Clone, Debug, Default)]
pub struct Audio {
    pub rate: u32,
    /// One `Vec` a channel.
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    pub fn secs(&self) -> f32 {
        self.channels.first().map_or(0.0, |c| c.len() as f32 / self.rate.max(1) as f32)
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    /// The channels averaged into one.
    pub fn mono(&self) -> Vec<f32> {
        let n = self.channels.len().max(1) as f32;
        (0..self.frames()).map(|i| self.channels.iter().map(|c| c[i]).sum::<f32>() / n).collect()
    }

    /// Two channels (mono is doubled, more are averaged in pairs).
    pub fn stereo(self) -> Self {
        match self.channels.len() {
            2 => self,
            1 => Self { rate: self.rate, channels: vec![self.channels[0].clone(), self.channels[0].clone()] },
            _ => {
                let m = self.mono();
                Self { rate: self.rate, channels: vec![m.clone(), m] }
            }
        }
    }

    /// Resampled to `rate` (the same audio when it already is).
    pub fn resampled(self, rate: u32) -> Result<Self, Error> {
        if rate == self.rate || self.frames() == 0 {
            return Ok(Self { rate, ..self });
        }
        let ch = self.channels.len();
        let mut rs = Fft::<f32>::new(self.rate as usize, rate as usize, CHUNK, ch, FixedSync::Input)
            .map_err(|e| Error::Audio(e.to_string()))?;
        let total = self.frames();
        let expected = (total as u64 * u64::from(rate)).div_ceil(u64::from(self.rate)) as usize;
        let delay = rs.output_delay();
        let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(expected + CHUNK); ch];
        let mut outbuf: Vec<Vec<f32>> = vec![vec![0.0; rs.output_frames_max()]; ch];
        let mut pos = 0;
        let mut padded: Vec<Vec<f32>> = vec![Vec::new(); ch];
        let mut guard = 0;
        while out[0].len() < expected + delay && guard < total / CHUNK + 64 {
            guard += 1;
            let need = rs.input_frames_next();
            let avail = total.saturating_sub(pos);
            let (src, partial): (Vec<&[f32]>, Option<usize>) = if avail >= need {
                (self.channels.iter().map(|c| &c[pos..pos + need]).collect(), None)
            } else {
                for (p, c) in padded.iter_mut().zip(&self.channels) {
                    p.clear();
                    p.extend_from_slice(&c[pos.min(total)..]);
                    p.resize(need, 0.0);
                }
                (padded.iter().map(Vec::as_slice).collect(), Some(avail))
            };
            let frames = rs.output_frames_next();
            let input = SequentialSliceOfSlices::new(&src, ch, need).map_err(|e| Error::Audio(e.to_string()))?;
            let mut output =
                SequentialSliceOfVecs::new_mut(&mut outbuf, ch, frames).map_err(|e| Error::Audio(e.to_string()))?;
            let idx = partial.map(|p| Indexing { input_offset: 0, output_offset: 0, partial_len: Some(p), active_channels_mask: None });
            let (used, produced) =
                rs.process_into_buffer(&input, &mut output, idx.as_ref()).map_err(|e| Error::Audio(e.to_string()))?;
            pos += used;
            for (o, b) in out.iter_mut().zip(&outbuf) {
                o.extend_from_slice(&b[..produced]);
            }
        }
        for o in &mut out {
            o.drain(..delay.min(o.len()));
            o.truncate(expected);
        }
        Ok(Self { rate, channels: out })
    }
}

/// Decodes a file (at most [`MAX_SECONDS`]).
pub fn decode(path: &Path) -> Result<Audio, Error> {
    let mut audio = Audio::default();
    let mut limit = 0usize;
    let mut sink = |chunk: &[f32], spec: nikgob_import::decode::PcmSpec| -> ControlFlow<()> {
        if audio.channels.is_empty() {
            audio.rate = spec.sample_rate;
            audio.channels = vec![Vec::new(); spec.channels.max(1)];
            limit = MAX_SECONDS as usize * spec.sample_rate as usize;
        }
        let ch = audio.channels.len();
        for frame in chunk.chunks_exact(ch) {
            for (c, s) in audio.channels.iter_mut().zip(frame) {
                c.push(*s);
            }
        }
        if audio.frames() >= limit { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
    };
    nikgob_import::decode::decode_file(path, &mut sink).map_err(|e| match e {
        nikgob_import::ImportError::Io { .. } => Error::Unavailable(e.to_string()),
        e => Error::Audio(e.to_string()),
    })?;
    if audio.frames() == 0 || audio.rate == 0 {
        return Err(Error::Audio("no audio".into()));
    }
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampling_keeps_length_and_pitch() {
        let rate = 44_100;
        let tone: Vec<f32> = (0..rate * 2).map(|i| (i as f32 / rate as f32 * 440.0 * std::f32::consts::TAU).sin()).collect();
        let a = Audio { rate: rate as u32, channels: vec![tone.clone(), tone] };
        let b = a.resampled(16_000).unwrap();
        assert_eq!(b.rate, 16_000);
        assert_eq!(b.frames(), 32_000);
        // Zero crossings of a 440 Hz tone over two seconds: about 1760.
        let m = b.mono();
        let crossings = m.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((crossings as i32 - 880).abs() <= 3, "{crossings}");
        // No delay: the tone starts at once.
        assert!(m[..200].iter().any(|s| s.abs() > 0.3));
    }

    #[test]
    fn channel_shapes() {
        let a = Audio { rate: 10, channels: vec![vec![1.0, 3.0]] };
        assert_eq!(a.clone().stereo().channels.len(), 2);
        let b = Audio { rate: 10, channels: vec![vec![1.0, 3.0], vec![3.0, 5.0]] };
        assert_eq!(b.mono(), [2.0, 4.0]);
        assert!((b.secs() - 0.2).abs() < 1e-6);
    }
}
