//! Opus packet decoding with the pure-Rust `ropus` port of libopus.
//!
//! Container handling (Ogg pages, granule positions, pre-skip, end trimming)
//! lives in Symphonia's Ogg demuxer and in [`super::symphonia_src`]; this file
//! only turns packets into PCM and applies the `OpusHead` output gain.

use crate::types::DecodeError;

/// Largest frame an Opus packet can decode to: 120 ms at 48 kHz.
const MAX_FRAME: usize = 5760;

/// The fields of `OpusHead` (RFC 7845 section 5.1) the engine uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OpusHead {
    pub channels: usize,
    /// Samples at 48 kHz to discard from the start of the decoded stream.
    pub pre_skip: u32,
    /// Linear factor for the header's Q7.8 dB output gain.
    pub gain: f32,
}

impl OpusHead {
    pub(crate) fn parse(data: &[u8]) -> Result<Self, DecodeError> {
        if data.len() < 19 || &data[..8] != b"OpusHead" {
            return Err(DecodeError::Format("bad OpusHead".into()));
        }
        let channels = usize::from(data[9]);
        let pre_skip = u32::from(u16::from_le_bytes([data[10], data[11]]));
        let gain_q8 = i16::from_le_bytes([data[16], data[17]]);
        let mapping_family = data[18];
        if mapping_family != 0 || !(1..=2).contains(&channels) {
            return Err(DecodeError::Unsupported(format!(
                "opus mapping family {mapping_family} with {channels} channels"
            )));
        }
        let gain = 10f32.powf(f32::from(gain_q8) / 256.0 / 20.0);
        Ok(Self {
            channels,
            pre_skip,
            gain,
        })
    }
}

pub(crate) struct OpusPackets {
    decoder: ropus::Decoder,
    head: OpusHead,
    buf: Vec<f32>,
}

impl OpusPackets {
    pub(crate) fn new(head: OpusHead) -> Result<Self, DecodeError> {
        Ok(Self {
            decoder: Self::make(head)?,
            head,
            buf: vec![0.0; MAX_FRAME * head.channels],
        })
    }

    fn make(head: OpusHead) -> Result<ropus::Decoder, DecodeError> {
        let channels = if head.channels == 1 {
            ropus::Channels::Mono
        } else {
            ropus::Channels::Stereo
        };
        ropus::Decoder::new(48_000, channels)
            .map_err(|e| DecodeError::Format(format!("opus decoder: {e:?}")))
    }

    /// Clears the decoder's state (after a seek).
    pub(crate) fn reset(&mut self) -> Result<(), DecodeError> {
        self.decoder = Self::make(self.head)?;
        Ok(())
    }

    /// Decodes one packet, drops `trim_start` frames from its front and
    /// `trim_end` from its back, applies the header gain, and appends the
    /// interleaved result to `out`. Returns the number of frames appended.
    pub(crate) fn decode(
        &mut self,
        packet: &[u8],
        trim_start: usize,
        trim_end: usize,
        out: &mut Vec<f32>,
    ) -> Result<usize, DecodeError> {
        let ch = self.head.channels;
        let frames = self
            .decoder
            .decode_float(packet, &mut self.buf, ropus::DecodeMode::Normal)
            .map_err(|e| DecodeError::Format(format!("opus packet: {e:?}")))?;
        let start = trim_start.min(frames);
        let end = frames.saturating_sub(trim_end).max(start);
        let slice = &self.buf[start * ch..end * ch];
        if (self.head.gain - 1.0).abs() > f32::EPSILON {
            out.extend(slice.iter().map(|s| s * self.head.gain));
        } else {
            out.extend_from_slice(slice);
        }
        Ok(end - start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head_bytes(channels: u8, pre_skip: u16, gain_q8: i16, family: u8) -> Vec<u8> {
        let mut v = b"OpusHead".to_vec();
        v.push(1);
        v.push(channels);
        v.extend_from_slice(&pre_skip.to_le_bytes());
        v.extend_from_slice(&44_100u32.to_le_bytes());
        v.extend_from_slice(&gain_q8.to_le_bytes());
        v.push(family);
        v
    }

    #[test]
    fn parses_pre_skip_and_gain() {
        let head = OpusHead::parse(&head_bytes(2, 312, 256 * 6, 0)).unwrap();
        assert_eq!(head.channels, 2);
        assert_eq!(head.pre_skip, 312);
        // +6 dB.
        assert!((head.gain - 1.995_262_3).abs() < 1e-4, "{}", head.gain);
        let head = OpusHead::parse(&head_bytes(1, 0, -256 * 6, 0)).unwrap();
        assert!((head.gain - 0.501_187).abs() < 1e-4);
    }

    #[test]
    fn rejects_multistream_and_garbage() {
        assert!(OpusHead::parse(&head_bytes(6, 312, 0, 1)).is_err());
        assert!(OpusHead::parse(b"OpusTags....").is_err());
    }
}
