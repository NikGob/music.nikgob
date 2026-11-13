//! Lossless -> Ogg Opus conversion.
//!
//! [`RopusEncoder`] is the default: an in-process encoder built on `ropus`, the
//! pure-Rust port of libopus, with the Ogg container written here (RFC 7845):
//! `OpusHead` with the encoder's real pre-skip, an empty `OpusTags` that the tag
//! step fills in later, 20 ms packets, and a final granule position that trims
//! the padding exactly. It needs no C toolchain and no external program.
//!
//! [`FfmpegEncoder`] shells out to `ffmpeg -c:a libopus` and exists as a
//! fallback behind the same [`Encoder`] trait.

use std::fs::File;
use std::io::BufWriter;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ogg::writing::{PacketWriteEndInfo, PacketWriter};

use crate::decode::{PcmSpec, decode_file};
use crate::error::{ImportError, Result};
use crate::resample::Resampler;

/// Opus frame length used for encoding: 20 ms at 48 kHz.
const FRAME: usize = 960;
const MAX_PACKET: usize = 4000;
const COMPLEXITY: u8 = 10;
const VENDOR: &str = "nikgob-import (ropus)";

#[derive(Clone, Copy, Debug)]
pub struct EncodeOptions {
    /// VBR target for stereo (mono gets 60 % of it).
    pub bitrate_kbps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EncodedInfo {
    /// Audio length in 48 kHz frames (without pre-skip).
    pub frames_48k: u64,
    pub bytes: u64,
    /// Average bitrate of the produced file, kbps.
    pub bitrate_kbps: u32,
}

pub trait Encoder: Send + Sync {
    fn name(&self) -> &'static str;

    /// Reads any decodable audio file `src` and writes an Ogg Opus file at `dst`.
    fn encode_to_opus(&self, src: &Path, dst: &Path, opts: &EncodeOptions) -> Result<EncodedInfo>;
}

/// In-process libopus port. See the module docs.
#[derive(Clone, Copy, Debug, Default)]
pub struct RopusEncoder;

struct Session {
    enc: ropus::Encoder,
    channels: usize,
    resampler: Resampler,
    writer: PacketWriter<'static, BufWriter<File>>,
    serial: u32,
    pending: Vec<f32>,
    tmp: Vec<f32>,
    mix: Vec<f32>,
    packet: Vec<u8>,
    /// Last encoded packet, held back so the final one can end the stream.
    prev: Option<Vec<u8>>,
    packets: u64,
    pre_skip: u64,
    bytes: u64,
}

fn opus_head(channels: usize, pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity(19);
    v.extend_from_slice(b"OpusHead");
    v.push(1);
    v.push(channels as u8);
    v.extend_from_slice(&pre_skip.to_le_bytes());
    v.extend_from_slice(&input_rate.to_le_bytes());
    v.extend_from_slice(&0i16.to_le_bytes());
    v.push(0);
    v
}

fn opus_tags_empty() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"OpusTags");
    v.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    v.extend_from_slice(VENDOR.as_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v
}

impl Session {
    fn new(dst: &Path, spec: PcmSpec, opts: &EncodeOptions) -> Result<Self> {
        let channels = if spec.channels == 1 { 1 } else { 2 };
        let kbps = if channels == 1 {
            opts.bitrate_kbps * 6 / 10
        } else {
            opts.bitrate_kbps
        };
        let enc = ropus::Encoder::builder(
            48_000,
            if channels == 1 {
                ropus::Channels::Mono
            } else {
                ropus::Channels::Stereo
            },
            ropus::Application::Audio,
        )
        .bitrate(ropus::Bitrate::Bits(kbps.max(6) * 1000))
        .complexity(COMPLEXITY)
        .vbr(true)
        .build()
        .map_err(|e| ImportError::Encode(format!("{e:?}")))?;
        let pre_skip = u64::from(enc.lookahead());
        let file = File::create(crate::sanitize::long_path(dst))
            .map_err(|e| ImportError::io(dst, e))?;
        let mut writer = PacketWriter::new(BufWriter::new(file));
        let serial = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0xC0DE_C0DE, |d| (d.subsec_nanos() ^ d.as_secs() as u32) | 1);
        let io = |e: std::io::Error| ImportError::io(dst, e);
        writer
            .write_packet(
                opus_head(channels, pre_skip as u16, spec.sample_rate),
                serial,
                PacketWriteEndInfo::EndPage,
                0,
            )
            .map_err(io)?;
        writer
            .write_packet(opus_tags_empty(), serial, PacketWriteEndInfo::EndPage, 0)
            .map_err(io)?;
        Ok(Self {
            enc,
            channels,
            resampler: Resampler::new(spec.sample_rate, 48_000, channels),
            writer,
            serial,
            pending: Vec::new(),
            tmp: Vec::new(),
            mix: Vec::new(),
            packet: vec![0u8; MAX_PACKET],
            prev: None,
            packets: 0,
            pre_skip,
            bytes: 0,
        })
    }

    /// Converts `chunk` (interleaved, `in_channels` wide) to the encoder layout.
    fn mixdown(&mut self, chunk: &[f32], in_channels: usize) {
        self.mix.clear();
        if in_channels == self.channels {
            self.mix.extend_from_slice(chunk);
            return;
        }
        for frame in chunk.chunks_exact(in_channels) {
            if self.channels == 1 {
                self.mix.push(frame.iter().sum::<f32>() / in_channels as f32);
            } else if in_channels == 6 {
                // 5.1 (L R C LFE Ls Rs) -> stereo, ITU-style, normalised.
                let (l, r, c, ls, rs) = (frame[0], frame[1], frame[2], frame[4], frame[5]);
                let norm = 1.0 / (1.0 + 2.0 * std::f32::consts::FRAC_1_SQRT_2);
                self.mix
                    .push((l + c * std::f32::consts::FRAC_1_SQRT_2 + ls * 0.707) * norm);
                self.mix
                    .push((r + c * std::f32::consts::FRAC_1_SQRT_2 + rs * 0.707) * norm);
            } else {
                self.mix.push(frame[0]);
                self.mix.push(frame[1]);
            }
        }
    }

    fn push(&mut self, chunk: &[f32], in_channels: usize, dst: &Path) -> Result<()> {
        self.mixdown(chunk, in_channels);
        let mix = std::mem::take(&mut self.mix);
        self.tmp.clear();
        self.resampler.process(&mix, &mut self.tmp);
        self.mix = mix;
        self.pending.extend_from_slice(&self.tmp);
        self.drain(dst, false)
    }

    /// Encodes every full 20 ms frame in `pending`.
    fn drain(&mut self, dst: &Path, _final: bool) -> Result<()> {
        let frame_len = FRAME * self.channels;
        let mut offset = 0;
        while self.pending.len() - offset >= frame_len {
            self.encode_frame(offset, dst)?;
            offset += frame_len;
        }
        self.pending.drain(..offset);
        Ok(())
    }

    fn encode_frame(&mut self, offset: usize, dst: &Path) -> Result<()> {
        let frame_len = FRAME * self.channels;
        let n = self
            .enc
            .encode_float(&self.pending[offset..offset + frame_len], &mut self.packet)
            .map_err(|e| ImportError::Encode(format!("{e:?}")))?;
        let data = self.packet[..n].to_vec();
        self.bytes += n as u64;
        self.packets += 1;
        if let Some(prev) = self.prev.replace(data) {
            let granule = (self.packets - 1) * FRAME as u64;
            self.writer
                .write_packet(prev, self.serial, PacketWriteEndInfo::NormalPacket, granule)
                .map_err(|e| ImportError::io(dst, e))?;
        }
        Ok(())
    }

    fn finish(mut self, dst: &Path, spec: PcmSpec) -> Result<EncodedInfo> {
        let _ = spec;
        self.tmp.clear();
        self.resampler.finish(&mut self.tmp);
        self.pending.extend_from_slice(&self.tmp);
        let total = self.resampler.frames_out();
        if total == 0 {
            return Err(ImportError::decode(dst, "no audio to encode"));
        }
        // Zero-pad the last partial frame and keep encoding silence until the
        // decoder can output everything after dropping the pre-skip.
        let frame_len = FRAME * self.channels;
        let rem = self.pending.len() % frame_len;
        if rem != 0 {
            self.pending.resize(self.pending.len() + frame_len - rem, 0.0);
        }
        self.drain(dst, true)?;
        while self.packets * (FRAME as u64) < total + self.pre_skip {
            self.pending.resize(frame_len, 0.0);
            self.encode_frame(0, dst)?;
            self.pending.clear();
        }
        let final_granule = total + self.pre_skip;
        let last = self
            .prev
            .take()
            .ok_or_else(|| ImportError::Encode("no packets produced".into()))?;
        self.writer
            .write_packet(
                last,
                self.serial,
                PacketWriteEndInfo::EndStream,
                final_granule,
            )
            .map_err(|e| ImportError::io(dst, e))?;
        drop(self.writer);
        let bytes = std::fs::metadata(crate::sanitize::long_path(dst))
            .map(|m| m.len())
            .unwrap_or(self.bytes);
        let secs = total as f64 / 48_000.0;
        Ok(EncodedInfo {
            frames_48k: total,
            bytes,
            bitrate_kbps: (bytes as f64 * 8.0 / secs / 1000.0).round() as u32,
        })
    }
}

impl Encoder for RopusEncoder {
    fn name(&self) -> &'static str {
        "ropus (in-process libopus port)"
    }

    fn encode_to_opus(&self, src: &Path, dst: &Path, opts: &EncodeOptions) -> Result<EncodedInfo> {
        let mut session: Option<Session> = None;
        let mut failure: Option<ImportError> = None;
        let mut spec_seen: Option<PcmSpec> = None;
        let mut sink = |chunk: &[f32], spec: PcmSpec| -> ControlFlow<()> {
            spec_seen = Some(spec);
            if session.is_none() {
                match Session::new(dst, spec, opts) {
                    Ok(s) => session = Some(s),
                    Err(e) => {
                        failure = Some(e);
                        return ControlFlow::Break(());
                    }
                }
            }
            if let Some(s) = &mut session
                && let Err(e) = s.push(chunk, spec.channels, dst)
            {
                failure = Some(e);
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        };
        let decoded = decode_file(src, &mut sink);
        drop(sink);
        let result = (|| {
            if let Some(e) = failure.take() {
                return Err(e);
            }
            decoded?;
            let spec = spec_seen.ok_or_else(|| ImportError::decode(src, "no audio decoded"))?;
            session
                .take()
                .ok_or_else(|| ImportError::decode(src, "no audio decoded"))?
                .finish(dst, spec)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(crate::sanitize::long_path(dst));
        }
        result
    }
}

/// `ffmpeg -c:a libopus`, for machines where it is preferred or as a fallback.
#[derive(Clone, Debug)]
pub struct FfmpegEncoder {
    exe: PathBuf,
}

impl FfmpegEncoder {
    /// Looks for `ffmpeg` on `PATH`.
    pub fn find() -> Option<Self> {
        Self::find_exe().map(|exe| Self { exe })
    }

    /// The `ffmpeg` program on `PATH`.
    pub fn find_exe() -> Option<PathBuf> {
        let names: &[&str] = if cfg!(windows) {
            &["ffmpeg.exe"]
        } else {
            &["ffmpeg"]
        };
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
            .find(|p| p.is_file())
    }

    pub fn with_exe(exe: impl Into<PathBuf>) -> Self {
        Self { exe: exe.into() }
    }
}

impl Encoder for FfmpegEncoder {
    fn name(&self) -> &'static str {
        "ffmpeg (libopus)"
    }

    fn encode_to_opus(&self, src: &Path, dst: &Path, opts: &EncodeOptions) -> Result<EncodedInfo> {
        let output = Command::new(&self.exe)
            .args(["-nostdin", "-v", "error", "-y", "-i"])
            .arg(src)
            .args(["-map", "0:a:0", "-vn", "-map_metadata", "-1", "-c:a", "libopus"])
            .args(["-b:a", &format!("{}k", opts.bitrate_kbps)])
            .args(["-vbr", "on", "-application", "audio", "-f", "ogg"])
            .arg(dst)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| ImportError::Encode(format!("cannot run ffmpeg: {e}")))?;
        if !output.status.success() {
            let _ = std::fs::remove_file(dst);
            return Err(ImportError::Encode(format!(
                "ffmpeg failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let info = crate::tags::probe_info(dst)?;
        let bytes = std::fs::metadata(dst).map(|m| m.len()).unwrap_or(0);
        Ok(EncodedInfo {
            frames_48k: (info.duration.as_secs_f64() * 48_000.0) as u64,
            bytes,
            bitrate_kbps: info.bitrate_kbps.unwrap_or(opts.bitrate_kbps),
        })
    }
}

/// Test helpers shared with the other modules' tests.
#[cfg(test)]
pub(crate) mod testwav {
    use std::path::Path;

    /// Writes a 16-bit PCM WAV (`samples` interleaved, already scaled to +-1).
    pub fn write_wav(path: &Path, samples: &[f32], rate: u32, channels: u16) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for s in samples {
            w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    /// Writes a 16-bit FLAC with the pure-Rust `flacenc`.
    pub fn write_flac(path: &Path, samples: &[f32], rate: u32, channels: usize) {
        use flacenc::component::BitRepr;
        use flacenc::error::Verify;
        let pcm: Vec<i32> = samples
            .iter()
            .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i32)
            .collect();
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .expect("flac config");
        let source = flacenc::source::MemSource::from_samples(&pcm, channels, 16, rate as usize);
        let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
            .expect("flac encode");
        let mut sink = flacenc::bitsink::ByteSink::new();
        stream.write(&mut sink).expect("flac write");
        std::fs::write(path, sink.as_slice()).unwrap();
    }

    pub fn sine(freq: f64, rate: u32, secs: f64, amp: f32, channels: usize) -> Vec<f32> {
        let n = (f64::from(rate) * secs) as usize;
        let mut out = Vec::with_capacity(n * channels);
        for i in 0..n {
            let s = amp * (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin() as f32;
            for _ in 0..channels {
                out.push(s);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testwav::*;
    use super::*;
    use crate::tags;

    fn decode_all(path: &Path) -> (Vec<f32>, PcmSpec) {
        let mut all = Vec::new();
        let spec = decode_file(path, &mut |c, _| {
            all.extend_from_slice(c);
            ControlFlow::Continue(())
        })
        .unwrap();
        (all, spec)
    }

    #[test]
    fn wav_to_opus_roundtrip_has_right_length_pitch_and_header() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("in.wav");
        let opus = dir.path().join("out.opus");
        write_wav(&wav, &sine(1000.0, 44_100, 3.0, 0.5, 2), 44_100, 2);

        let info = RopusEncoder
            .encode_to_opus(&wav, &opus, &EncodeOptions { bitrate_kbps: 160 })
            .unwrap();
        assert_eq!(info.frames_48k, 144_000);
        assert!(info.bytes > 5_000, "{info:?}");
        assert!((40..260).contains(&info.bitrate_kbps), "{info:?}");

        // The container is a valid Ogg Opus file for lofty.
        let (_, finfo) = tags::read_tags(&opus).unwrap();
        assert_eq!(finfo.codec, nikgob_core::model::Codec::Opus);
        assert!(
            (finfo.duration.as_secs_f64() - 3.0).abs() < 0.03,
            "{:?}",
            finfo.duration
        );

        // Decoding gives back exactly the input length (pre-skip and padding trimmed).
        let (pcm, spec) = decode_all(&opus);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.channels, 2);
        let frames = pcm.len() / 2;
        assert!(
            frames.abs_diff(144_000) <= 2,
            "decoded {frames} frames, expected 144000"
        );
        // Level and pitch survive: look at the middle second of the left channel.
        let left: Vec<f32> = pcm.iter().step_by(2).copied().collect();
        let mid = &left[48_000..96_000];
        let rms = (mid.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / mid.len() as f64).sqrt();
        assert!((rms - 0.5 / 2f64.sqrt()).abs() < 0.02, "rms {rms}");
        let crossings = mid.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((crossings as f64 - 1000.0).abs() < 8.0, "crossings {crossings}");
    }

    #[test]
    fn mono_and_odd_rates_work() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("m.wav");
        let opus = dir.path().join("m.opus");
        write_wav(&wav, &sine(440.0, 22_050, 1.0, 0.4, 1), 22_050, 1);
        let info = RopusEncoder
            .encode_to_opus(&wav, &opus, &EncodeOptions { bitrate_kbps: 160 })
            .unwrap();
        assert_eq!(info.frames_48k, 48_000);
        let (pcm, spec) = decode_all(&opus);
        assert_eq!(spec.channels, 1);
        assert!(pcm.len().abs_diff(48_000) <= 2, "{}", pcm.len());
    }

    #[test]
    fn missing_input_fails_cleanly_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("x.opus");
        let err = RopusEncoder
            .encode_to_opus(&dir.path().join("none.wav"), &out, &EncodeOptions { bitrate_kbps: 160 });
        assert!(err.is_err());
        assert!(!out.exists());
    }

    #[test]
    #[ignore = "needs ffmpeg on PATH"]
    fn ffmpeg_encoder_produces_opus() {
        let Some(ff) = FfmpegEncoder::find() else {
            panic!("ffmpeg not found");
        };
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("in.wav");
        let opus = dir.path().join("out.opus");
        write_wav(&wav, &sine(1000.0, 44_100, 2.0, 0.5, 2), 44_100, 2);
        let info = ff
            .encode_to_opus(&wav, &opus, &EncodeOptions { bitrate_kbps: 160 })
            .unwrap();
        assert!(info.bytes > 3_000);
        let (_, finfo) = tags::read_tags(&opus).unwrap();
        assert_eq!(finfo.codec, nikgob_core::model::Codec::Opus);
    }
}
