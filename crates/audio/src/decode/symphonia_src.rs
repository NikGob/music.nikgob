//! File decoding through Symphonia, with Opus routed to `ropus`.
//!
//! Timeline model. `emitted` is the index, on the playable timeline, of the
//! next frame this source will hand out. For most codecs Symphonia's decoders
//! already remove encoder delay and padding (`Packet::trim_start/trim_end`),
//! so the timeline starts at the first packet's timestamp. Ogg Opus is the
//! exception: Symphonia's Ogg demuxer reports packet timestamps on the granule
//! timeline, which *includes* the pre-skip, and has no Opus decoder of its
//! own, so here `origin = pre_skip` is subtracted and the first `pre_skip`
//! frames fall at negative indices, which are dropped. The last page's
//! granule position already encodes the end trim; the demuxer turns that into
//! `trim_end` on the final packet, which is applied by the Opus wrapper.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{TimeBase, Timestamp};

use super::opus::{OpusHead, OpusPackets};
use super::{FrameSource, ReadStatus};
use crate::types::DecodeError;

/// How long before the seek target decoding restarts for codecs whose output
/// depends on earlier packets (80 ms is the Opus recommendation).
const OPUS_PREROLL_MS: u64 = 80;
const LOSSY_PREROLL_MS: u64 = 100;

/// Consecutive undecodable packets after which the stream is given up on.
const MAX_BAD_PACKETS: u32 = 64;

enum Dec {
    Sym(Box<dyn AudioDecoder>),
    Opus(Box<OpusPackets>),
}

pub(crate) struct SymphoniaSource {
    format: Box<dyn FormatReader>,
    track_id: u32,
    dec: Dec,
    rate: u32,
    channels: usize,
    time_base: TimeBase,
    /// Timeline origin in frames (Opus pre-skip, otherwise 0).
    origin: i64,
    total: Option<u64>,
    preroll: u64,
    /// Timeline index of the next frame to emit.
    emitted: i64,
    /// Next packet defines `emitted` (start of stream or just after a seek).
    resync: bool,
    /// Frames below this timeline index are dropped.
    skip_to: i64,
    eof: bool,
    bad_packets: u32,
    scratch: Vec<f32>,
    /// Decoded while learning the format of a stream whose parameters were
    /// incomplete; handed out by the first `read`.
    prefetch: Option<Vec<f32>>,
}

impl SymphoniaSource {
    pub(crate) fn open(path: &Path) -> Result<Self, DecodeError> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        Self::from_format(format)
    }

    fn from_format(format: Box<dyn FormatReader>) -> Result<Self, DecodeError> {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| DecodeError::Unsupported("no audio track".into()))?;
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            return Err(DecodeError::Unsupported(
                "track has no audio parameters".into(),
            ));
        };
        let track_id = track.id;
        let start_ts = track.start_ts;
        let num_frames = track.num_frames;
        let duration = track.duration;
        let is_opus = params.codec == CODEC_ID_OPUS;

        let mut rate = params.sample_rate.unwrap_or(0);
        let mut channels = params.channels.as_ref().map_or(0, |c| c.count());

        let (dec, origin) = if is_opus {
            let extra = params
                .extra_data
                .as_deref()
                .ok_or_else(|| DecodeError::Format("opus stream without OpusHead".into()))?;
            let head = OpusHead::parse(extra)?;
            rate = 48_000;
            channels = head.channels;
            (
                Dec::Opus(Box::new(OpusPackets::new(head)?)),
                i64::from(head.pre_skip),
            )
        } else {
            let decoder = symphonia::default::get_codecs()
                .make_audio_decoder(&params, &AudioDecoderOptions::default())
                .map_err(|e| match e {
                    SymError::Unsupported(m) => DecodeError::Unsupported(m.to_owned()),
                    other => DecodeError::Format(other.to_string()),
                })?;
            (Dec::Sym(decoder), 0)
        };

        let time_base = track
            .time_base
            .unwrap_or_else(|| TimeBase::try_from_recip(rate.max(1)).unwrap_or_default());

        let mut source = Self {
            format,
            track_id,
            dec,
            rate,
            channels,
            time_base,
            origin,
            total: None,
            preroll: preroll_frames(params.codec, rate.max(1)),
            emitted: 0,
            resync: true,
            skip_to: 0,
            eof: false,
            bad_packets: 0,
            scratch: Vec::new(),
            prefetch: None,
        };
        source.emitted = source.ts_to_frames(start_ts) - origin;

        // Parameters can be incomplete (some containers only know the format
        // from the first decoded buffer): decode ahead to learn it.
        if source.rate == 0 || source.channels == 0 {
            let mut first = Vec::new();
            loop {
                match source.read(&mut first)? {
                    ReadStatus::Data => break,
                    ReadStatus::Pending => {}
                    ReadStatus::Eof => {
                        return Err(DecodeError::Format("empty audio stream".into()));
                    }
                }
            }
            source.prefetch = Some(first);
        }
        if source.rate == 0 || !(1..=32).contains(&source.channels) {
            return Err(DecodeError::Unsupported(format!(
                "{} Hz, {} channels",
                source.rate, source.channels
            )));
        }

        source.total = num_frames
            .map(|n| n.saturating_sub(origin as u64))
            .or_else(|| {
                duration.map(|d| {
                    (source.dur_to_frames(d.get() as i64).max(0) as u64)
                        .saturating_sub(origin as u64)
                })
            });
        Ok(source)
    }

    /// Frames of the playable timeline for a timestamp in track time-base units.
    fn ts_to_frames(&self, ts: Timestamp) -> i64 {
        self.dur_to_frames(ts.get())
    }

    fn dur_to_frames(&self, ticks: i64) -> i64 {
        let numer = i128::from(self.time_base.numer.get());
        let denom = i128::from(self.time_base.denom.get());
        if numer == 1 && denom == i128::from(self.rate) {
            return ticks;
        }
        let num = i128::from(ticks) * numer * i128::from(self.rate);
        let half = denom / 2;
        let rounded = if num >= 0 { num + half } else { num - half };
        (rounded / denom) as i64
    }

    fn frames_to_ts(&self, frames: i64) -> Timestamp {
        let numer = i128::from(self.time_base.numer.get());
        let denom = i128::from(self.time_base.denom.get());
        if numer == 1 && denom == i128::from(self.rate) {
            return Timestamp::new(frames);
        }
        let num = i128::from(frames) * denom;
        let div = i128::from(self.rate) * numer;
        let half = div / 2;
        let rounded = if num >= 0 { num + half } else { num - half };
        Timestamp::new((rounded / div) as i64)
    }

    /// Decodes `packet` into `self.scratch`; returns the frame count.
    fn decode_packet(&mut self, packet: &Packet) -> Result<usize, DecodeError> {
        self.scratch.clear();
        let trim_start = self.dur_to_frames(packet.trim_start.get() as i64).max(0) as usize;
        let trim_end = self.dur_to_frames(packet.trim_end.get() as i64).max(0) as usize;
        match &mut self.dec {
            Dec::Opus(opus) => opus.decode(&packet.data, trim_start, trim_end, &mut self.scratch),
            Dec::Sym(decoder) => {
                let buf = decoder.decode(packet)?;
                let spec = buf.spec();
                let (rate, channels) = (spec.rate(), spec.channels().count());
                if self.rate == 0 {
                    self.rate = rate;
                }
                if self.channels == 0 {
                    self.channels = channels;
                }
                if rate != self.rate || channels != self.channels {
                    return Err(DecodeError::Unsupported(
                        "stream changes format in the middle".into(),
                    ));
                }
                let frames = buf.frames();
                buf.copy_to_vec_interleaved::<f32>(&mut self.scratch);
                Ok(frames)
            }
        }
    }

    fn reset_decoder(&mut self) -> Result<(), DecodeError> {
        match &mut self.dec {
            Dec::Opus(opus) => opus.reset(),
            Dec::Sym(decoder) => {
                decoder.reset();
                Ok(())
            }
        }
    }
}

fn preroll_frames(codec: AudioCodecId, rate: u32) -> u64 {
    let ms = if codec == CODEC_ID_OPUS {
        OPUS_PREROLL_MS
    } else if [
        CODEC_ID_MP1,
        CODEC_ID_MP2,
        CODEC_ID_MP3,
        CODEC_ID_AAC,
        CODEC_ID_VORBIS,
    ]
    .contains(&codec)
    {
        LOSSY_PREROLL_MS
    } else {
        0
    };
    u64::from(rate) * ms / 1000
}

impl FrameSource for SymphoniaSource {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn total_frames(&self) -> Option<u64> {
        self.total
    }

    fn read(&mut self, out: &mut Vec<f32>) -> Result<ReadStatus, DecodeError> {
        if let Some(first) = self.prefetch.take() {
            out.extend_from_slice(&first);
            return Ok(ReadStatus::Data);
        }
        loop {
            if self.eof {
                return Ok(ReadStatus::Eof);
            }
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(SymError::ResetRequired) => {
                    self.eof = true;
                    return Ok(ReadStatus::Eof);
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    log::warn!("audio file ends abruptly: {e}");
                    self.eof = true;
                    return Ok(ReadStatus::Eof);
                }
                Err(e) => return Err(e.into()),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let frames = match self.decode_packet(&packet) {
                Ok(frames) => {
                    self.bad_packets = 0;
                    frames
                }
                Err(e @ (DecodeError::Format(_) | DecodeError::Io(_))) => {
                    self.bad_packets += 1;
                    log::warn!("skipping undecodable packet: {e}");
                    if self.bad_packets > MAX_BAD_PACKETS {
                        return Err(e);
                    }
                    // The lost packet's frames are unknown; keep the timeline
                    // honest by letting the next packet resynchronize it.
                    self.resync = true;
                    continue;
                }
                Err(e) => return Err(e),
            };
            if self.resync {
                let first = self.ts_to_frames(packet.pts)
                    + self.dur_to_frames(packet.trim_start.get() as i64)
                    - self.origin;
                self.emitted = first;
                self.resync = false;
            }
            let ch = self.channels;
            let frames_i = frames as i64;
            let drop = (self.skip_to - self.emitted).clamp(0, frames_i) as usize;
            self.emitted += frames_i;
            if drop < frames {
                out.extend_from_slice(&self.scratch[drop * ch..frames * ch]);
                return Ok(ReadStatus::Data);
            }
        }
    }

    fn seek(&mut self, frame: u64) -> Result<(), DecodeError> {
        let target = i64::try_from(frame).unwrap_or(i64::MAX);
        let from = (target - self.preroll as i64).max(0);
        let ts = self.frames_to_ts(from + self.origin);
        match self.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts,
                track_id: self.track_id,
            },
        ) {
            Ok(_) => {}
            Err(SymError::SeekError(SeekErrorKind::OutOfRange)) => {
                // Past the end: behave as a seek to the very end.
                self.eof = true;
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
        self.reset_decoder()?;
        self.eof = false;
        self.resync = true;
        self.skip_to = target;
        self.bad_packets = 0;
        self.prefetch = None;
        Ok(())
    }
}
