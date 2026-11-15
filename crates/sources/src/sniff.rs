//! What an audio file is, from its first bytes, never from its name: FLAC
//! (bare or in Ogg), WAV and AIFF (PCM), MP3 (with its bitrate, or VBR),
//! Ogg Vorbis and Opus, MP4 (AAC or ALAC), raw AAC, WavPack and APE. The
//! sources whose files say nothing of their format before they come
//! (SoundCloud's originals, Dogmazic's pages) look at the head of the file
//! to offer it for what it is, and at the whole file once it is here.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use nikgob_core::model::Codec;
use nikgob_core::source::{Format, SourceError};

/// Bytes read to tell a file.
pub const HEAD: u64 = 64 * 1024;
/// Bytes read past an ID3 tag longer than [`HEAD`].
pub const AFTER_TAG: u64 = 16 * 1024;

/// An audio file told.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sniffed {
    pub format: Format,
    /// The extension the file gets.
    pub ext: &'static str,
    /// Where the audio starts (past an ID3 tag), for a bitrate made of
    /// the size.
    pub start: u64,
}

/// What the head of a file says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sniff {
    Audio(Sniffed),
    /// An ID3 tag (a cover in it) longer than the bytes read: the audio
    /// starts at this offset.
    After(u64),
    /// Not audio this knows (a page, an error answer, a video).
    Unknown,
}

/// The head of a file (from its first byte).
pub fn sniff(head: &[u8]) -> Sniff {
    match id3_len(head) {
        Some(n) if n as usize >= head.len() => Sniff::After(n),
        Some(n) => bare(&head[n as usize..]).map_or(Sniff::Unknown, |s| Sniff::Audio(Sniffed { start: n, ..s })),
        None => bare(head).map_or(Sniff::Unknown, Sniff::Audio),
    }
}

/// The audio past an ID3 tag of `start` bytes (the bytes read there).
pub fn sniff_at(bytes: &[u8], start: u64) -> Option<Sniffed> {
    bare(bytes).map(|s| Sniffed { start, ..s })
}

/// A file on disk.
pub fn sniff_file(path: &Path) -> Option<Sniffed> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = Vec::new();
    (&mut f).take(HEAD).read_to_end(&mut head).ok()?;
    match sniff(&head) {
        Sniff::Audio(s) => Some(s),
        Sniff::After(n) => {
            f.seek(SeekFrom::Start(n)).ok()?;
            let mut rest = Vec::new();
            f.take(AFTER_TAG).read_to_end(&mut rest).ok()?;
            sniff_at(&rest, n)
        }
        Sniff::Unknown => None,
    }
}

/// A downloaded file renamed to what its bytes are (a song uploaded as
/// «….WAV.mp3» is an MP3); one that is not audio is removed.
pub async fn named_by_bytes(path: &Path) -> Result<PathBuf, SourceError> {
    let Some(s) = sniff_file(path) else {
        let _ = tokio::fs::remove_file(path).await;
        return Err(SourceError::Parse("the file is not audio".into()));
    };
    let right = path.with_extension(s.ext);
    if right != path {
        tokio::fs::rename(path, &right).await.map_err(|e| SourceError::Io(e.to_string()))?;
    }
    Ok(right)
}

/// A lossy format without a bitrate (a VBR MP3, Opus) gets the average of
/// its size over its length; anything else stays as it is.
pub fn with_average(s: &Sniffed, size: Option<u64>, secs: Option<u32>) -> Format {
    let mut f = s.format.clone();
    if !f.lossless()
        && f.bitrate_kbps.is_none()
        && let (Some(size), Some(secs)) = (size, secs.filter(|s| *s > 0))
    {
        let kbps = size.saturating_sub(s.start) * 8 / u64::from(secs) / 1000;
        f.bitrate_kbps = (kbps > 0).then(|| u32::try_from(kbps).unwrap_or(u32::MAX));
    }
    f
}

/// The length of an ID3v2 tag at the start, header and footer included.
fn id3_len(b: &[u8]) -> Option<u64> {
    if b.len() < 10 || &b[..3] != b"ID3" {
        return None;
    }
    let size = b[6..10].iter().fold(0u64, |acc, x| (acc << 7) | u64::from(x & 0x7f));
    let footer = if b[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + size + footer)
}

fn at<const N: usize>(b: &[u8], i: usize) -> Option<[u8; N]> {
    b.get(i..i + N)?.try_into().ok()
}

fn u16_le(b: &[u8], i: usize) -> Option<u16> {
    at::<2>(b, i).map(u16::from_le_bytes)
}

fn u32_le(b: &[u8], i: usize) -> Option<u32> {
    at::<4>(b, i).map(u32::from_le_bytes)
}

fn u32_be(b: &[u8], i: usize) -> Option<u32> {
    at::<4>(b, i).map(u32::from_be_bytes)
}

fn lossless(codec: Codec, ext: &'static str, bits: Option<u16>, rate: Option<u32>) -> Sniffed {
    Sniffed {
        format: Format {
            bit_depth: bits.and_then(|b| u8::try_from(b).ok()).filter(|b| *b > 0),
            sample_rate: rate.filter(|r| *r > 0),
            ..Format::new(codec)
        },
        ext,
        start: 0,
    }
}

fn lossy(codec: Codec, ext: &'static str, kbps: Option<u32>) -> Sniffed {
    Sniffed { format: Format { bitrate_kbps: kbps.filter(|k| *k > 0), ..Format::new(codec) }, ext, start: 0 }
}

/// The audio from its first byte (no ID3 tag in front).
fn bare(b: &[u8]) -> Option<Sniffed> {
    let tag = |i: usize, t: &[u8]| b.get(i..i + t.len()) == Some(t);
    if tag(0, b"fLaC") {
        return Some(flac(b, "flac"));
    }
    if tag(0, b"RIFF") && tag(8, b"WAVE") {
        return wav(b);
    }
    if tag(0, b"FORM") && (tag(8, b"AIFF") || tag(8, b"AIFC")) {
        return aiff(b);
    }
    if tag(0, b"OggS") {
        return ogg(b);
    }
    if tag(4, b"ftyp") {
        return Some(mp4(b));
    }
    if tag(0, b"wvpk") {
        return Some(lossless(Codec::Other("wavpack".into()), "wv", None, None));
    }
    if tag(0, b"MAC ") {
        return Some(lossless(Codec::Other("ape".into()), "ape", None, None));
    }
    mpeg(b)
}

/// FLAC: the STREAMINFO block says the rate and the bits.
fn flac(b: &[u8], ext: &'static str) -> Sniffed {
    let info = (b.get(4).is_some_and(|t| t & 0x7f == 0)).then(|| b.get(8..42)).flatten();
    let Some(si) = info else { return lossless(Codec::Flac, ext, None, None) };
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let bits = ((u16::from(si[12] & 1) << 4) | u16::from(si[13] >> 4)) + 1;
    lossless(Codec::Flac, ext, Some(bits), Some(rate))
}

/// WAV: PCM or float samples are lossless; anything packed in it (an MP3,
/// ADPCM) is not told.
fn wav(b: &[u8]) -> Option<Sniffed> {
    let mut pos = 12usize;
    while let Some(size) = u32_le(b, pos + 4) {
        if b.get(pos..pos + 4) == Some(&b"fmt "[..]) {
            let f = pos + 8;
            let mut kind = u16_le(b, f)?;
            if kind == 0xFFFE {
                kind = u16_le(b, f + 24)?;
            }
            if !matches!(kind, 1 | 3) {
                return None;
            }
            return Some(lossless(Codec::Wav, "wav", u16_le(b, f + 14), u32_le(b, f + 4)));
        }
        pos = pos.checked_add(8 + size as usize + (size as usize & 1))?;
    }
    // The format chunk past the bytes read: still a WAV.
    Some(lossless(Codec::Wav, "wav", None, None))
}

/// AIFF / AIFF-C: PCM samples (big- or little-endian, or float) are
/// lossless, the same as a WAV.
fn aiff(b: &[u8]) -> Option<Sniffed> {
    let compressed = b.get(8..12) == Some(&b"AIFC"[..]);
    let mut pos = 12usize;
    while let Some(size) = u32_be(b, pos + 4) {
        if b.get(pos..pos + 4) == Some(&b"COMM"[..]) {
            let c = pos + 8;
            let bits = at::<2>(b, c + 6).map(u16::from_be_bytes);
            let rate = at::<10>(b, c + 8).and_then(|x| extended(&x));
            if compressed {
                let kind = b.get(c + 18..c + 22)?;
                if !matches!(kind, b"NONE" | b"none" | b"sowt" | b"twos" | b"fl32" | b"FL32" | b"fl64" | b"FL64") {
                    return None;
                }
            }
            return Some(lossless(Codec::Wav, "aiff", bits, rate));
        }
        pos = pos.checked_add(8 + size as usize + (size as usize & 1))?;
    }
    Some(lossless(Codec::Wav, "aiff", None, None))
}

/// An 80-bit extended float (AIFF's sample rate) as a whole number.
fn extended(b: &[u8; 10]) -> Option<u32> {
    let exp = ((i32::from(b[0] & 0x7f) << 8) | i32::from(b[1])) - 16383;
    let mant = u64::from_be_bytes(b[2..10].try_into().ok()?);
    if !(0..=31).contains(&exp) {
        return None;
    }
    u32::try_from(mant >> (63 - exp)).ok()
}

/// Ogg: the first packet names the codec.
fn ogg(b: &[u8]) -> Option<Sniffed> {
    let segments = usize::from(*b.get(26)?);
    let p = b.get(27 + segments..)?;
    if p.starts_with(b"OpusHead") {
        return Some(lossy(Codec::Opus, "opus", None));
    }
    if p.starts_with(b"\x01vorbis") {
        let nominal = at::<4>(p, 20).map(i32::from_le_bytes).filter(|n| *n > 0);
        return Some(lossy(Codec::Vorbis, "ogg", nominal.map(|n| n.unsigned_abs() / 1000)));
    }
    if p.starts_with(b"\x7fFLAC") {
        return Some(flac(p.get(9..)?, "oga"));
    }
    None
}

/// MP4 audio: ALAC when its sample entry says so, else AAC.
fn mp4(b: &[u8]) -> Sniffed {
    if b.windows(4).any(|w| w == b"alac") {
        lossless(Codec::Alac, "m4a", None, None)
    } else {
        lossy(Codec::Aac, "m4a", None)
    }
}

const MP3_V1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const MP3_V2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];

/// An MPEG audio frame (or raw AAC) within the first bytes (padding may
/// stand before it); a second frame where the first says it ends, when the
/// bytes reach that far, so a stray `FF Ex` is not taken for one.
fn mpeg(b: &[u8]) -> Option<Sniffed> {
    let scan = b.len().saturating_sub(4).min(4096);
    for i in 0..scan {
        if b[i] != 0xFF || b[i + 1] & 0xE0 != 0xE0 {
            continue;
        }
        let version = (b[i + 1] >> 3) & 3;
        let layer = (b[i + 1] >> 1) & 3;
        if layer == 0 && b[i + 1] & 0xF6 == 0xF0 {
            return Some(lossy(Codec::Aac, "aac", None));
        }
        let bi = usize::from(b[i + 2] >> 4);
        let si = usize::from((b[i + 2] >> 2) & 3);
        if version == 1 || layer != 1 || bi == 0 || bi == 15 || si == 3 {
            continue;
        }
        let v1 = version == 3;
        let kbps = if v1 { MP3_V1[bi] } else { MP3_V2[bi] };
        let rate = [[44_100, 48_000, 32_000], [22_050, 24_000, 16_000], [11_025, 12_000, 8_000]][match version {
            3 => 0,
            2 => 1,
            _ => 2,
        }][si];
        let pad = u32::from((b[i + 2] >> 1) & 1);
        let per: u32 = if v1 { 144 } else { 72 };
        let len = (per * kbps * 1000 / rate + pad) as usize;
        if let Some(next) = b.get(i + len..i + len + 2)
            && !(next[0] == 0xFF && next[1] & 0xE0 == 0xE0)
        {
            continue;
        }
        let mono = b[i + 3] >> 6 == 3;
        let side = match (v1, mono) {
            (true, true) => 17,
            (true, false) => 32,
            (false, true) => 9,
            (false, false) => 17,
        };
        let xing = b.get(i + 4 + side..i + 8 + side);
        let vbr = xing == Some(&b"Xing"[..]) || b.get(i + 36..i + 40) == Some(&b"VBRI"[..]);
        return Some(lossy(Codec::Mp3, "mp3", (!vbr).then_some(kbps)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streaminfo(rate: u32, bits: u16) -> Vec<u8> {
        let mut b = b"fLaC".to_vec();
        b.extend([0x80, 0, 0, 34]);
        let mut si = vec![0x10, 0, 0x10, 0, 0, 0, 0, 0, 0, 0];
        let ch = 1u32; // stereo: channels - 1
        let b1 = (rate >> 12) as u8;
        let b2 = (rate >> 4) as u8;
        let b3 = (((rate & 0xF) << 4) as u8) | ((ch as u8) << 1) | (((bits - 1) >> 4) as u8 & 1);
        let b4 = (((bits - 1) & 0xF) << 4) as u8;
        si.extend([b1, b2, b3, b4]);
        si.extend([0u8; 20]);
        b.extend(si);
        b
    }

    fn wav_head(kind: u16, bits: u16, rate: u32) -> Vec<u8> {
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        b.extend(b"LIST");
        b.extend(4u32.to_le_bytes());
        b.extend(b"INFO");
        b.extend(b"fmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(kind.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(rate.to_le_bytes());
        b.extend((rate * 2 * u32::from(bits) / 8).to_le_bytes());
        b.extend((2 * bits / 8).to_le_bytes());
        b.extend(bits.to_le_bytes());
        b.extend(b"data\0\0\0\0");
        b
    }

    /// MPEG-1 Layer III frames at `kbps`, 44.1 kHz, stereo.
    fn mp3_frames(kbps: u32, xing: bool) -> Vec<u8> {
        let bi = MP3_V1.iter().position(|k| *k == kbps).unwrap() as u8;
        let len = (144 * kbps * 1000 / 44_100) as usize;
        let mut out = Vec::new();
        for n in 0..3 {
            let mut f = vec![0xFF, 0xFB, bi << 4, 0x00];
            f.resize(len, 0);
            if n == 0 && xing {
                f[36..40].copy_from_slice(b"Xing");
            }
            out.extend(f);
        }
        out
    }

    fn told(b: &[u8]) -> (String, &'static str) {
        match sniff(b) {
            Sniff::Audio(s) => (s.format.label(), s.ext),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn lossless_files_say_their_rate_and_bits() {
        assert_eq!(told(&streaminfo(44_100, 16)), ("FLAC".into(), "flac"));
        assert_eq!(told(&streaminfo(96_000, 24)), ("FLAC 24/96".into(), "flac"));
        assert_eq!(told(&wav_head(1, 16, 44_100)), ("WAV".into(), "wav"));
        assert_eq!(told(&wav_head(1, 24, 48_000)), ("WAV 24/48".into(), "wav"));
        assert_eq!(told(&wav_head(3, 32, 96_000)), ("WAV 32/96".into(), "wav"));
        // An MP3 packed in a WAV is not a lossless file.
        assert_eq!(sniff(&wav_head(0x55, 0, 44_100)), Sniff::Unknown);
        let mut aiff = b"FORM\0\0\0\0AIFFCOMM".to_vec();
        aiff.extend(18u32.to_be_bytes());
        aiff.extend(2u16.to_be_bytes());
        aiff.extend(1000u32.to_be_bytes());
        aiff.extend(16u16.to_be_bytes());
        aiff.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(told(&aiff), ("WAV".into(), "aiff"));
        let s = sniff_at(&aiff, 0).unwrap();
        assert_eq!((s.format.sample_rate, s.format.bit_depth), (Some(44_100), Some(16)));
    }

    #[test]
    fn an_mp3_is_an_mp3_whatever_it_is_called() {
        assert_eq!(told(&mp3_frames(320, false)), ("MP3 320".into(), "mp3"));
        assert_eq!(told(&mp3_frames(192, false)), ("MP3 192".into(), "mp3"));
        // VBR: no bitrate of its own; the size over the length gives one.
        let Sniff::Audio(vbr) = sniff(&mp3_frames(128, true)) else { panic!() };
        assert_eq!(vbr.format.bitrate_kbps, None);
        assert_eq!(with_average(&vbr, Some(8_000_000), Some(250)).bitrate_kbps, Some(256));
        // Behind an ID3 tag (with padding before the first frame).
        let mut tagged = b"ID3\x03\0\0\0\0\0\x0a".to_vec();
        tagged.extend([0u8; 10]);
        tagged.extend([0u8; 3]);
        tagged.extend(mp3_frames(256, false));
        let Sniff::Audio(s) = sniff(&tagged) else { panic!() };
        assert_eq!((s.format.label(), s.start), ("MP3 256".into(), 20));
        // A tag longer than what was read: read on from where it ends.
        let mut big = b"ID3\x04\0\0\0\x04\0\0".to_vec();
        big.extend([0u8; 100]);
        assert_eq!(sniff(&big), Sniff::After(10 + 65_536));
        assert!(!told(&mp3_frames(320, false)).0.starts_with("FLAC"));
    }

    #[test]
    fn ogg_mp4_and_the_rest() {
        let page = |packet: &[u8]| {
            let mut b = b"OggS".to_vec();
            b.extend([0u8; 22]);
            b.push(1);
            b.push(packet.len() as u8);
            b.extend(packet);
            b
        };
        let mut vorbis = b"\x01vorbis".to_vec();
        vorbis.extend([0, 0, 0, 0, 2]);
        vorbis.extend(44_100u32.to_le_bytes());
        vorbis.extend(0i32.to_le_bytes());
        vorbis.extend(192_000i32.to_le_bytes());
        vorbis.extend(0i32.to_le_bytes());
        assert_eq!(told(&page(&vorbis)), ("VORBIS 192".into(), "ogg"));
        assert_eq!(told(&page(b"OpusHead\x01\x02")), ("OPUS".into(), "opus"));
        let mut oga = b"\x7fFLAC\x01\x00\x00\x01".to_vec();
        oga.extend(streaminfo(48_000, 24));
        assert_eq!(told(&page(&oga)), ("FLAC 24/48".into(), "oga"));
        let mut m4a = b"\0\0\0\x20ftypM4A \0\0\0\0".to_vec();
        m4a.extend(b"....mp4a....");
        assert_eq!(told(&m4a), ("AAC".into(), "m4a"));
        m4a.extend(b"alac");
        assert_eq!(told(&m4a), ("ALAC".into(), "m4a"));
        assert_eq!(told(&[0xFF, 0xF1, 0x50, 0x80, 0, 0]).1, "aac");
        assert_eq!(sniff(b"<!DOCTYPE html><html>"), Sniff::Unknown);
        assert_eq!(sniff(br#"{"subsonic-response":{}}"#), Sniff::Unknown);
        assert_eq!(sniff(b""), Sniff::Unknown);
    }

    #[test]
    fn a_file_on_disk_is_told_past_a_long_tag() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("original.wav");
        // Called .wav, an MP3 behind a 70 KB tag.
        let mut b = b"ID3\x03\0\0\x00\x04\x22\x30".to_vec();
        let tag = 0x04 * 128 * 128 + 0x22 * 128 + 0x30;
        b.resize(10 + tag, 0);
        b.extend(mp3_frames(320, false));
        std::fs::write(&p, &b).unwrap();
        let s = sniff_file(&p).unwrap();
        assert_eq!((s.format.label(), s.ext, s.start), ("MP3 320".into(), "mp3", 10 + tag as u64));
    }
}
