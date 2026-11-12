//! Streaming PCM decoding for analysis and conversion.
//!
//! Symphonia handles FLAC, MP3, AAC/ALAC (MP4), Vorbis and WAV/AIFF. Opus has no
//! Symphonia decoder, so its packets are read by the Ogg demuxer and decoded by
//! `ropus` (the same split the playback engine uses). Output is interleaved
//! `f32`; the callback may stop the decode early.
//!
//! [`decode_file_windows`] decodes only some stretches of a track (the sound
//! analysis's windows), seeking to each where the container allows it and
//! reading past what lies before it where it does not.

use std::fs::File;
use std::ops::ControlFlow;
use std::path::Path;

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::error::{ImportError, Result};

/// Consecutive undecodable packets after which a file is given up on.
const MAX_BAD_PACKETS: u32 = 64;
const OPUS_MAX_FRAME: usize = 5760;

/// A stretch of a track to decode, in seconds from its start.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub start: f64,
    pub len: f64,
}

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
    let file = File::open(crate::sanitize::long_path(path)).map_err(|e| ImportError::io(path, e))?;
    let ext = path.extension().and_then(|e| e.to_str());
    decode_source(Box::new(file), ext, path, sink)
}

/// Decodes only the spans `plan` asks for, given the track's length in
/// seconds when the container knows it; `sink` gets each span's chunks with
/// the span's number and may stop a span early (the next one follows).
/// Spans are taken in order; one past the end gives nothing.
pub fn decode_file_windows(
    path: &Path,
    plan: &mut dyn FnMut(Option<f64>) -> Vec<Span>,
    sink: &mut dyn FnMut(usize, &[f32], PcmSpec) -> ControlFlow<()>,
) -> Result<PcmSpec> {
    let file = File::open(crate::sanitize::long_path(path)).map_err(|e| ImportError::io(path, e))?;
    let ext = path.extension().and_then(|e| e.to_str());
    let mut opened = Opened::new(Box::new(file), ext, path)?;
    opened.windows(plan, sink)
}

/// [`decode_file_windows`] for encoded bytes in memory (a stream's audio,
/// a preview's head).
pub fn decode_bytes_windows(
    bytes: Vec<u8>,
    ext: &str,
    name: &str,
    plan: &mut dyn FnMut(Option<f64>) -> Vec<Span>,
    sink: &mut dyn FnMut(usize, &[f32], PcmSpec) -> ControlFlow<()>,
) -> Result<PcmSpec> {
    let ext = ext.trim_start_matches('.');
    let mut opened = Opened::new(Box::new(std::io::Cursor::new(bytes)), (!ext.is_empty()).then_some(ext), Path::new(name))?;
    opened.windows(plan, sink)
}

/// [`decode_file`] for encoded bytes in memory (a stream's audio); `ext`
/// is the container as a file extension (`"webm"`, `"m4a"`, `"ogg"`…) and
/// `name` only names it in errors.
pub fn decode_bytes(
    bytes: Vec<u8>,
    ext: &str,
    name: &str,
    sink: &mut dyn FnMut(&[f32], PcmSpec) -> ControlFlow<()>,
) -> Result<PcmSpec> {
    let ext = ext.trim_start_matches('.');
    decode_source(
        Box::new(std::io::Cursor::new(bytes)),
        (!ext.is_empty()).then_some(ext),
        Path::new(name),
        sink,
    )
}

fn decode_source(
    media: Box<dyn symphonia::core::io::MediaSource>,
    ext: Option<&str>,
    path: &Path,
    sink: &mut dyn FnMut(&[f32], PcmSpec) -> ControlFlow<()>,
) -> Result<PcmSpec> {
    let mut opened = Opened::new(media, ext, path)?;
    let mut scratch: Vec<f32> = Vec::new();
    while opened.next(&mut scratch)? {
        if scratch.is_empty() {
            continue;
        }
        let spec = opened.spec();
        if sink(&scratch, spec).is_break() {
            return Ok(spec);
        }
    }
    opened.finished_spec()
}

/// An open track: its demuxer and decoder, and where the decode is.
struct Opened<'p> {
    path: &'p Path,
    format: Box<dyn FormatReader>,
    track_id: u32,
    dec: Dec,
    rate: u32,
    channels: usize,
    /// The track's length in seconds, when the container says.
    duration: Option<f64>,
    time_base: Option<symphonia::core::units::TimeBase>,
    /// Frames from the track's start to the end of what was decoded.
    pos: u64,
    bad_packets: u32,
}

/// A seek lands this much before a span's start: the decoder settles
/// (Opus asks for 80 ms) on audio that is read past.
const SEEK_LEAD: f64 = 0.2;

impl<'p> Opened<'p> {
    fn new(media: Box<dyn symphonia::core::io::MediaSource>, ext: Option<&str>, path: &'p Path) -> Result<Self> {
        let bad = |m: String| ImportError::decode(path, m);
        let mss = MediaSourceStream::new(media, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = ext {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(|e| bad(e.to_string()))?;
        let track = format.default_track(TrackType::Audio).ok_or_else(|| bad("no audio track".into()))?;
        let track_id = track.id;
        let time_base = track.time_base;
        let num_frames = track.num_frames;
        let track_duration = track.duration;
        let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
            return Err(bad("track has no audio parameters".into()));
        };

        let mut rate = params.sample_rate.unwrap_or(0);
        let mut channels = params.channels.as_ref().map_or(0, |c| c.count());
        let dec = if params.codec == CODEC_ID_OPUS {
            let extra = params.extra_data.as_deref().ok_or_else(|| bad("opus stream without OpusHead".into()))?;
            let (ch, pre_skip, gain) = parse_opus_head(path, extra)?;
            rate = 48_000;
            channels = ch;
            Dec::Opus { decoder: opus_decoder(path, ch)?, buf: vec![0.0; OPUS_MAX_FRAME * ch], gain, skip: pre_skip }
        } else {
            let decoder = symphonia::default::get_codecs()
                .make_audio_decoder(&params, &AudioDecoderOptions::default())
                .map_err(|e| ImportError::Unsupported(format!("{}: {e}", path.display())))?;
            Dec::Sym(decoder)
        };
        let duration = match (time_base, track_duration) {
            (Some(tb), Some(d)) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
            _ => None,
        }
        .or_else(|| num_frames.filter(|_| rate > 0).map(|n| n as f64 / f64::from(rate)))
        .filter(|d| d.is_finite() && *d > 0.0);
        Ok(Self { path, format, track_id, dec, rate, channels, duration, time_base, pos: 0, bad_packets: 0 })
    }

    fn spec(&self) -> PcmSpec {
        PcmSpec { sample_rate: self.rate, channels: self.channels }
    }

    fn finished_spec(&self) -> Result<PcmSpec> {
        if self.rate == 0 || self.channels == 0 {
            return Err(ImportError::decode(self.path, "empty audio stream"));
        }
        Ok(self.spec())
    }

    /// Decodes the next packet of the track into `scratch` (cleared; it
    /// may stay empty). False at the end of the stream.
    fn next(&mut self, scratch: &mut Vec<f32>) -> Result<bool> {
        let path = self.path;
        let bad = |m: String| ImportError::decode(path, m);
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) | Err(SymError::ResetRequired) => return Ok(false),
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(false),
                Err(e) => return Err(bad(e.to_string())),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            scratch.clear();
            let channels = self.channels;
            let decoded: std::result::Result<(), String> = match &mut self.dec {
                Dec::Opus { decoder, buf, gain, skip } => match decoder.decode_float(&packet.data, buf, ropus::DecodeMode::Normal) {
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
                        if self.rate == 0 {
                            self.rate = spec.rate();
                        }
                        if self.channels == 0 {
                            self.channels = spec.channels().count();
                        }
                        if spec.rate() != self.rate || spec.channels().count() != self.channels {
                            return Err(ImportError::Unsupported(format!("{}: stream changes format in the middle", path.display())));
                        }
                        buf.copy_to_vec_interleaved::<f32>(scratch);
                        Ok(())
                    }
                    Err(SymError::DecodeError(m)) => Err(m.to_string()),
                    Err(SymError::IoError(e)) => Err(e.to_string()),
                    Err(e) => return Err(bad(e.to_string())),
                },
            };
            match decoded {
                Ok(()) => self.bad_packets = 0,
                Err(m) => {
                    self.bad_packets += 1;
                    log::warn!("{}: skipping undecodable packet: {m}", path.display());
                    if self.bad_packets > MAX_BAD_PACKETS {
                        return Err(bad(m));
                    }
                    continue;
                }
            }
            if self.channels > 0 {
                self.pos += (scratch.len() / self.channels) as u64;
            }
            return Ok(true);
        }
    }

    /// Seeks to `secs` (sample-accurate: at or before it). False when the
    /// container cannot; the decode then goes on from where it was.
    fn seek(&mut self, secs: f64) -> bool {
        let Some(time) = Time::try_from_secs_f64(secs.max(0.0)) else { return false };
        match self.format.seek(SeekMode::Accurate, SeekTo::Time { time, track_id: Some(self.track_id) }) {
            Ok(to) => {
                let landed = self
                    .time_base
                    .and_then(|tb| tb.calc_time(to.actual_ts))
                    .map(|t| t.as_secs_f64())
                    .filter(|t| t.is_finite() && *t >= 0.0)
                    .unwrap_or(secs);
                self.pos = (landed * f64::from(self.rate)) as u64;
                match &mut self.dec {
                    Dec::Sym(d) => d.reset(),
                    Dec::Opus { decoder, skip, .. } => {
                        // A fresh decoder; no pre-skip in the middle.
                        match opus_decoder(self.path, self.channels) {
                            Ok(d) => *decoder = d,
                            Err(_) => return false,
                        }
                        *skip = 0;
                    }
                }
                true
            }
            Err(e) => {
                log::debug!("{}: cannot seek to {secs:.1} s: {e}", self.path.display());
                false
            }
        }
    }

    /// Decodes the spans of `plan` (see [`decode_file_windows`]).
    fn windows(
        &mut self,
        plan: &mut dyn FnMut(Option<f64>) -> Vec<Span>,
        sink: &mut dyn FnMut(usize, &[f32], PcmSpec) -> ControlFlow<()>,
    ) -> Result<PcmSpec> {
        let spans = plan(self.duration);
        let mut scratch: Vec<f32> = Vec::new();
        'spans: for (i, span) in spans.iter().enumerate() {
            // Past the end there is nothing (and some demuxers fail a seek there).
            if self.duration.is_some_and(|d| span.start >= d) {
                break;
            }
            if self.rate > 0 {
                let here = self.pos as f64 / f64::from(self.rate);
                if span.start - SEEK_LEAD > here + 1.0 {
                    self.seek(span.start - SEEK_LEAD);
                }
            }
            let mut need: Option<u64> = None;
            loop {
                if !self.next(&mut scratch)? {
                    break 'spans;
                }
                if scratch.is_empty() || self.channels == 0 || self.rate == 0 {
                    continue;
                }
                let ch = self.channels;
                let rate = f64::from(self.rate);
                let frames = (scratch.len() / ch) as u64;
                let chunk_start = self.pos - frames;
                let first = (span.start.max(0.0) * rate) as u64;
                if self.pos <= first {
                    continue;
                }
                let left = need.get_or_insert((span.len.max(0.0) * rate) as u64);
                let skip = first.saturating_sub(chunk_start) as usize;
                let take = ((frames as usize - skip) as u64).min(*left) as usize;
                if take > 0 {
                    *left -= take as u64;
                    if sink(i, &scratch[skip * ch..(skip + take) * ch], self.spec()).is_break() {
                        break;
                    }
                }
                if *left == 0 {
                    break;
                }
            }
        }
        self.finished_spec()
    }
}

fn opus_decoder(path: &Path, channels: usize) -> Result<ropus::Decoder> {
    ropus::Decoder::new(48_000, if channels == 1 { ropus::Channels::Mono } else { ropus::Channels::Stereo })
        .map_err(|e| ImportError::decode(path, format!("opus decoder: {e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::testwav::write_wav;

    /// A slow ramp from -0.9 to 0.9 over `secs`: a sample says where in
    /// the track it is.
    fn ramp(rate: u32, secs: f64, channels: usize) -> Vec<f32> {
        let n = (f64::from(rate) * secs) as usize;
        (0..n).flat_map(|i| std::iter::repeat_n(-0.9 + 1.8 * i as f32 / n as f32, channels)).collect()
    }

    fn time_of(sample: f32, secs: f64) -> f64 {
        f64::from(sample + 0.9) / 1.8 * secs
    }

    /// Each span starts where it was asked (the container seeks there or
    /// the decode reads past what lies before) and is as long as asked; a
    /// span past the end gives nothing; the plan sees the track's length.
    fn check_spans(decode: impl FnOnce(&mut dyn FnMut(Option<f64>) -> Vec<Span>, &mut dyn FnMut(usize, &[f32], PcmSpec) -> ControlFlow<()>) -> Result<PcmSpec>, secs: f64, rate: u32, channels: usize) {
        let mut seen_len = None;
        let mut plan = |d: Option<f64>| {
            seen_len = d;
            vec![Span { start: 20.0, len: 3.0 }, Span { start: 70.0, len: 2.0 }, Span { start: secs + 10.0, len: 5.0 }]
        };
        let mut got: Vec<Vec<f32>> = vec![Vec::new(); 3];
        let mut sink = |w: usize, chunk: &[f32], spec: PcmSpec| {
            assert_eq!(spec.channels, channels);
            got[w].extend(chunk.chunks_exact(channels).map(|f| f[0]));
            ControlFlow::Continue(())
        };
        let spec = decode(&mut plan, &mut sink).unwrap();
        assert_eq!(spec.sample_rate, rate);
        assert!((seen_len.unwrap() - secs).abs() < 0.1, "{seen_len:?}");
        assert_eq!(got[0].len(), 3 * rate as usize);
        assert_eq!(got[1].len(), 2 * rate as usize);
        assert!(got[2].is_empty());
        for (w, want) in [(0, 20.0), (1, 70.0)] {
            let at = time_of(got[w][0], secs);
            assert!((at - want).abs() < 0.05, "span {w} starts at {at}");
            let end = time_of(*got[w].last().unwrap(), secs);
            assert!(end > at, "span {w} runs forward");
        }
    }

    #[test]
    fn spans_of_a_wav_from_disk_and_memory() {
        let dir = tempfile::tempdir().unwrap();
        let (rate, secs) = (8_000, 100.0);
        let wav = dir.path().join("ramp.wav");
        write_wav(&wav, &ramp(rate, secs, 2), rate, 2);
        check_spans(|plan, sink| decode_file_windows(&wav, plan, sink), secs, rate, 2);
        let bytes = std::fs::read(&wav).unwrap();
        check_spans(|plan, sink| decode_bytes_windows(bytes, "wav", "ramp", plan, sink), secs, rate, 2);
    }

    /// A sink that stops a span early moves on to the next one.
    #[test]
    fn a_stopped_span_leaves_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("ramp.wav");
        write_wav(&wav, &ramp(8_000, 60.0, 1), 8_000, 1);
        let mut counts = [0usize; 2];
        let mut plan = |_: Option<f64>| vec![Span { start: 5.0, len: 10.0 }, Span { start: 30.0, len: 1.0 }];
        let mut sink = |w: usize, chunk: &[f32], _: PcmSpec| {
            counts[w] += chunk.len();
            if w == 0 { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
        };
        decode_file_windows(&wav, &mut plan, &mut sink).unwrap();
        assert!(counts[0] > 0 && counts[0] < 80_000);
        assert_eq!(counts[1], 8_000);
    }
}
