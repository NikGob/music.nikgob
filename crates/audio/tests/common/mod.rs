//! Fixture generation and helpers shared by the integration tests. Nothing
//! here needs a sound device.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use nikgob_audio::{Event, OfflineDevice, OfflineOptions, Player};

/// A deterministic stereo test signal: two tones per channel plus a little
/// pseudo-noise so any misalignment shows up. `start` is the index of the
/// first frame, so consecutive pieces of one recording can be generated
/// separately.
pub fn signal(rate: u32, start: usize, frames: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * 2);
    for i in start..start + frames {
        let t = i as f64 / f64::from(rate);
        let noise = |seed: u64| {
            let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed;
            x ^= x >> 29;
            x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x ^= x >> 32;
            ((x & 0xFFFF) as f64 / 65536.0 - 0.5) * 0.02
        };
        let l = 0.30 * (std::f64::consts::TAU * 440.0 * t).sin()
            + 0.20 * (std::f64::consts::TAU * 1234.5 * t + 0.3).sin()
            + noise(1);
        let r = 0.25 * (std::f64::consts::TAU * 660.0 * t).sin()
            + 0.15 * (std::f64::consts::TAU * 3000.0 * t + 1.1).sin()
            + noise(2);
        out.push(l as f32);
        out.push(r as f32);
    }
    out
}

/// Quantizes to 16 bit exactly the way a lossless 16-bit file stores it.
pub fn to_i16(samples: &[f32]) -> Vec<i32> {
    samples
        .iter()
        .map(|s| (s * 32768.0).round().clamp(-32768.0, 32767.0) as i32)
        .collect()
}

pub fn from_i16(samples: &[i32]) -> Vec<f32> {
    samples.iter().map(|&s| s as f32 / 32768.0).collect()
}

pub fn write_wav_f32(path: &Path, rate: u32, channels: u16, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

/// Writes a valid FLAC file using VERBATIM subframes (no compression): a
/// minimal dependency-free encoder, good enough to exercise the demuxer and
/// seeking with exact, lossless content.
pub fn write_flac16(path: &Path, rate: u32, channels: usize, samples: &[i32]) {
    const BLOCK: usize = 4096;
    fn crc8(data: &[u8]) -> u8 {
        let mut crc = 0u8;
        for &b in data {
            crc ^= b;
            for _ in 0..8 {
                crc = if crc & 0x80 != 0 {
                    (crc << 1) ^ 0x07
                } else {
                    crc << 1
                };
            }
        }
        crc
    }
    fn crc16(data: &[u8]) -> u16 {
        let mut crc = 0u16;
        for &b in data {
            crc ^= u16::from(b) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 {
                    (crc << 1) ^ 0x8005
                } else {
                    crc << 1
                };
            }
        }
        crc
    }
    let total = samples.len() / channels;
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&[0x80, 0, 0, 34]);
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // frame sizes unknown
    let packed: u64 =
        (u64::from(rate) << 44) | (((channels - 1) as u64) << 41) | (15u64 << 36) | total as u64;
    out.extend_from_slice(&packed.to_be_bytes());
    out.extend_from_slice(&[0u8; 16]); // md5 unknown
    for (n, block) in samples.chunks(BLOCK * channels).enumerate() {
        let frames = block.len() / channels;
        let mut frame = vec![0xFF, 0xF8, 0x70, ((channels as u8 - 1) << 4) | 0x08];
        // UTF-8 coded frame number.
        let num = n as u32;
        if num < 0x80 {
            frame.push(num as u8);
        } else if num < 0x800 {
            frame.extend_from_slice(&[0xC0 | (num >> 6) as u8, 0x80 | (num & 0x3F) as u8]);
        } else {
            frame.extend_from_slice(&[
                0xE0 | (num >> 12) as u8,
                0x80 | ((num >> 6) & 0x3F) as u8,
                0x80 | (num & 0x3F) as u8,
            ]);
        }
        frame.extend_from_slice(&((frames - 1) as u16).to_be_bytes());
        frame.push(crc8(&frame));
        for c in 0..channels {
            frame.push(0x02); // verbatim subframe, no wasted bits
            for f in 0..frames {
                frame.extend_from_slice(&(block[f * channels + c] as i16).to_be_bytes());
            }
        }
        let crc = crc16(&frame);
        frame.extend_from_slice(&crc.to_be_bytes());
        out.extend_from_slice(&frame);
    }
    std::fs::write(path, out).unwrap();
}

/// Encodes stereo 48 kHz `samples` as Ogg Opus with the ropus encoder.
/// Returns the pre-skip written to the header.
pub fn write_opus(path: &Path, samples: &[f32], gain_q8: i16) -> u32 {
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};
    use ropus::{Application, Bitrate, Channels, Encoder};
    const FRAME: usize = 960;
    let mut enc = Encoder::builder(48_000, Channels::Stereo, Application::Audio)
        .bitrate(Bitrate::try_bits(160_000).unwrap())
        .build()
        .expect("opus encoder");
    let pre_skip = enc.lookahead();
    let total = samples.len() / 2;

    let mut file = std::fs::File::create(path).unwrap();
    let mut w = PacketWriter::new(&mut file);
    let serial = 0x1234_5678;
    let mut head = b"OpusHead".to_vec();
    head.push(1);
    head.push(2);
    head.extend_from_slice(&(pre_skip as u16).to_le_bytes());
    head.extend_from_slice(&48_000u32.to_le_bytes());
    head.extend_from_slice(&gain_q8.to_le_bytes());
    head.push(0);
    w.write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)
        .unwrap();
    let mut tags = b"OpusTags".to_vec();
    let vendor = b"nikgob-test";
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    w.write_packet(tags, serial, PacketWriteEndInfo::EndPage, 0)
        .unwrap();

    let blocks = total.div_ceil(FRAME);
    // The decoder delivers `pre_skip` frames of delay, so one more frame
    // block than the input needs may be required to flush the encoder.
    let total_blocks = blocks + usize::from(blocks * FRAME < total + pre_skip as usize);
    let mut packet = vec![0u8; 4000];
    for b in 0..total_blocks {
        let mut pcm = vec![0.0f32; FRAME * 2];
        let from = b * FRAME;
        if from < total {
            let to = (from + FRAME).min(total);
            pcm[..(to - from) * 2].copy_from_slice(&samples[from * 2..to * 2]);
        }
        let n = enc.encode_float(&pcm, &mut packet).expect("encode");
        let last = b + 1 == total_blocks;
        let granule = ((b + 1) * FRAME).min(total + pre_skip as usize) as u64;
        let info = if last {
            PacketWriteEndInfo::EndStream
        } else if b % 25 == 24 {
            PacketWriteEndInfo::EndPage
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        w.write_packet(packet[..n].to_vec(), serial, info, granule)
            .unwrap();
    }
    pre_skip
}

pub struct Rig {
    pub player: Player,
    pub events: Receiver<Event>,
    pub device: OfflineDevice,
}

pub fn rig(rate: u32) -> Rig {
    let (player, events, device) = Player::offline(OfflineOptions {
        sample_rate: rate,
        channels: 2,
        block_frames: 256,
    })
    .unwrap();
    Rig {
        player,
        events,
        device,
    }
}

/// Collects events until `pred` matches one or the timeout passes.
pub fn wait_event(
    events: &Receiver<Event>,
    timeout: Duration,
    pred: impl Fn(&Event) -> bool,
) -> Vec<Event> {
    let end = Instant::now() + timeout;
    let mut seen = Vec::new();
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        match events.recv_timeout(left) {
            Ok(e) => {
                let hit = pred(&e);
                seen.push(e);
                if hit {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    seen
}

pub fn max_diff(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "length mismatch");
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

pub fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

pub fn path(dir: &tempfile::TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

/// RMS of an interleaved buffer.
pub fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

/// Signal-to-error ratio in dB of `got` against `want`.
pub fn snr_db(want: &[f32], got: &[f32]) -> f32 {
    let sig: f32 = want.iter().map(|s| s * s).sum();
    let err: f32 = want.iter().zip(got).map(|(a, b)| (a - b) * (a - b)).sum();
    10.0 * (sig / err.max(1e-20)).log10()
}
