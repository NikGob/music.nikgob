//! Streaming PCM decoding for analysis and conversion.
//!
//! Symphonia handles FLAC, MP3, AAC/ALAC (MP4), Vorbis and WAV/AIFF. Opus has no
//! Symphonia decoder, so its packets are read by the Ogg demuxer and decoded by
//! `ropus` (the same split the playback engine uses). Output is interleaved
//! `f32`; the callback may stop the decode early.

use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use crate::error::{ImportError, Result};

/// Consecutive undecodable packets after which a file is given up on.
const MAX_BAD_PACKETS: u32 = 64;
const OPUS_MAX_FRAME: usize = 5760;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmSpec {
    pub sample_rate: u32,
    pub channels: usize,
}

enum Dec {
    Sym(Box<dyn AudioDecoder>),
    Opus {
        decoder: ropus::Decoder,
        buf: Vec<f32>,
        gain: f32,
        /// Frames still to drop from the start (pre-skip).
        skip: usize,
    },
}

/// Parses the fields of `OpusHead` we need: (channels, pre-skip, linear gain).
fn parse_opus_head(path: &Path, data: &[u8]) -> Result<(usize, usize, f32)> {
    if data.len() < 19 || &data[..8] != b"OpusHead" {
        return Err(ImportError::decode(path, "bad OpusHead"));
    }
    let channels = usize::from(data[9]);
    let pre_skip = usize::from(u16::from_le_bytes([data[10], data[11]]));
    let gain_q8 = i16::from_le_bytes([data[16], data[17]]);
    if data[18] != 0 || !(1..=2).contains(&channels) {
        return Err(ImportError::Unsupported(format!(
            "{}: Opus mapping family {} with {channels} channels",
            path.display(),
            data[18]
        )));
    }
    let gain = 10f32.powf(f32::from(gain_q8) / 256.0 / 20.0);
    Ok((channels, pre_skip, gain))
}

/// Decodes `path`, handing interleaved `f32` chunks to `sink` until the end of
/// the file or until the sink breaks. Returns the stream format.
pub fn decode_file(
    path: &Path,
    sink: &mut dyn FnMut(&[f32], PcmSpec) -> ControlFlow<()>,
) -> Result<PcmSpec> {
    let bad = |m: String| ImportError::decode(path, m);
    let file = File::open(crate::sanitize::long_path(path)).map_err(|e| ImportError::io(path, e))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| bad(e.to_string()))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| bad("no audio track".into()))?;
    let track_id = track.id;
    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
        return Err(bad("track has no audio parameters".into()));
    };

    let mut rate = params.sample_rate.unwrap_or(0);
    let mut channels = params.channels.as_ref().map_or(0, |c| c.count());
    let mut dec = if params.codec == CODEC_ID_OPUS {
        let extra = params
            .extra_data
            .as_deref()
            .ok_or_else(|| bad("opus stream without OpusHead".into()))?;
        let (ch, pre_skip, gain) = parse_opus_head(path, extra)?;
        rate = 48_000;
        channels = ch;
        let decoder = ropus::Decoder::new(
            48_000,
            if ch == 1 {
                ropus::Channels::Mono
            } else {
                ropus::Channels::Stereo
            },
        )
        .map_err(|e| bad(format!("opus decoder: {e:?}")))?;
        Dec::Opus {
            decoder,
            buf: vec![0.0; OPUS_MAX_FRAME * ch],
            gain,
            skip: pre_skip,
        }
    } else {
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| ImportError::Unsupported(format!("{}: {e}", path.display())))?;
        Dec::Sym(decoder)
    };

    let mut scratch: Vec<f32> = Vec::new();
    let mut bad_packets = 0u32;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) | Err(SymError::ResetRequired) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(bad(e.to_string())),
        };
        if packet.track_id != track_id {
            continue;
        }
        scratch.clear();
        let decoded: std::result::Result<(), String> = match &mut dec {
            Dec::Opus {
                decoder,
                buf,
                gain,
                skip,
            } => match decoder.decode_float(&packet.data, buf, ropus::DecodeMode::Normal) {
                Ok(frames) => {
                    let drop = (*skip).min(frames);
                    *skip -= drop;
                    // The Ogg demuxer turns the final granule position into
                    // `trim_end` of the last packet (end padding).
                    let end = frames.saturating_sub(packet.trim_end.get().max(0) as usize).max(drop);
                    let slice = &buf[drop * channels..end * channels];
                    if (*gain - 1.0).abs() > f32::EPSILON {
                        scratch.extend(slice.iter().map(|s| s * *gain));
                    } else {
                        scratch.extend_from_slice(slice);
                    }
                    Ok(())
                }
                Err(e) => Err(format!("opus packet: {e:?}")),
            },
            Dec::Sym(decoder) => match decoder.decode(&packet) {
                Ok(buf) => {
                    let spec = buf.spec();
                    if rate == 0 {
                        rate = spec.rate();
                    }
                    if channels == 0 {
                        channels = spec.channels().count();
                    }
                    if spec.rate() != rate || spec.channels().count() != channels {
                        return Err(ImportError::Unsupported(format!(
                            "{}: stream changes format in the middle",
                            path.display()
                        )));
                    }
                    buf.copy_to_vec_interleaved::<f32>(&mut scratch);
                    Ok(())
                }
                Err(SymError::DecodeError(m)) => Err(m.to_string()),
                Err(SymError::IoError(e)) => Err(e.to_string()),
                Err(e) => return Err(bad(e.to_string())),
            },
        };
        match decoded {
            Ok(()) => bad_packets = 0,
            Err(m) => {
                bad_packets += 1;
                log::warn!("{}: skipping undecodable packet: {m}", path.display());
                if bad_packets > MAX_BAD_PACKETS {
                    return Err(bad(m));
                }
                continue;
            }
        }
        if scratch.is_empty() {
            continue;
        }
        let spec = PcmSpec {
            sample_rate: rate,
            channels,
        };
        if sink(&scratch, spec).is_break() {
            return Ok(spec);
        }
    }
    if rate == 0 || channels == 0 {
        return Err(bad("empty audio stream".into()));
    }
    Ok(PcmSpec {
        sample_rate: rate,
        channels,
    })
}
