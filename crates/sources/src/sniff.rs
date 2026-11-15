//! What an audio file is, from its first bytes, never from its name: FLAC
//! (bare or in Ogg), WAV and AIFF (PCM only), MP3 (with its bitrate, or
//! VBR), Ogg Vorbis and Opus, MP4 (AAC or ALAC, from its sample entry),
//! raw AAC, WavPack and APE. The sources whose files say nothing of their
//! format before they come (SoundCloud's originals, Dogmazic's pages) look
//! at the head of the file to offer it for what it is, and at the whole
//! file once it is here.
//!
//! What the head does not say is read on where the file says it is (past
//! a long ID3 tag, past a big chunk before a WAV's `fmt `, at an MP4's
//! `moov` after its `mdat`), a few reads at most ([`MAX_READS`]); a file
//! still not told is not offered. A container alone never makes a file
//! lossless: a WAV is PCM only when its format chunk says so.

use std::future::Future;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use nikgob_core::model::Codec;
use nikgob_core::source::{Format, SourceError};

/// Bytes read to tell a file.
pub const HEAD: u64 = 64 * 1024;
/// Bytes read past an ID3 tag longer than [`HEAD`].
pub const AFTER_TAG: u64 = 16 * 1024;
/// Bytes read at a WAV's or an AIFF's next chunk.
const CHUNK: u64 = 4096;
/// Bytes read at an MP4's top-level box (its `moov`, the sample tables).
const MOOV: u64 = 256 * 1024;
/// Reads of one file at most, the head included.
pub const MAX_READS: usize = 4;
/// A lossy bitrate made of a size above this is a size that lies.
const MAX_LOSSY_KBPS: u128 = 2000;

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
    /// The answer is further in: read `len` bytes at `at` and [`resume`].
    More(Resume),
    /// Not audio this knows (a page, an error answer, a video, a WAV of
    /// packed samples).
    Unknown,
}

/// Where to read on, and what for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resume {
    pub at: u64,
    pub len: u64,
    step: Step,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    /// The audio past an ID3 tag.
    AfterTag,
    /// A WAV's chunks, from a chunk's header.
    Riff,
    /// An AIFF's chunks, from a chunk's header.
    Aiff { compressed: bool },
    /// An MP4's top-level boxes, from a box's header.
    Mp4,
}

fn more(at: u64, len: u64, step: Step) -> Sniff {
    Sniff::More(Resume { at, len, step })
}

/// The head of a file (from its first byte).
pub fn sniff(head: &[u8]) -> Sniff {
    match id3_len(head) {
        Some(n) if n >= head.len() as u64 => more(n, AFTER_TAG, Step::AfterTag),
        Some(n) => from_tag(bare(&head[n as usize..], n), n),
        None => bare(head, 0),
    }
}

/// Goes on with `bytes` read at `r.at` (up to `r.len` of them).
pub fn resume(r: &Resume, bytes: &[u8]) -> Sniff {
    if bytes.is_empty() {
        return Sniff::Unknown;
    }
    match r.step {
        Step::AfterTag => from_tag(bare(bytes, r.at), r.at),
        Step::Riff => riff(bytes, r.at, r.at),
        Step::Aiff { compressed } => aiff(bytes, r.at, r.at, compressed),
        Step::Mp4 => mp4(bytes, r.at, r.at),
    }
}

/// The audio past an ID3 tag starts where the tag ends.
fn from_tag(s: Sniff, start: u64) -> Sniff {
    match s {
        Sniff::Audio(s) => Sniff::Audio(Sniffed { start, ..s }),
        other => other,
    }
}

/// A file read through `read(at, len)` (a range of a download): the head,
/// then where it says, [`MAX_READS`] reads at most. The format and the
/// whole size the first answer gave; `None` for a file not told. A server
/// that cannot give a part past the start, or a part past the end, is a
/// file not told.
pub async fn sniff_remote<F, Fut>(mut read: F) -> Result<Option<(Sniffed, Option<u64>)>, SourceError>
where
    F: FnMut(u64, u64) -> Fut,
    Fut: Future<Output = Result<(Vec<u8>, Option<u64>), SourceError>>,
{
    let (head, size) = read(0, HEAD).await?;
    let mut s = sniff(&head);
    for _ in 1..MAX_READS {
        let Sniff::More(r) = &s else { break };
        if size.is_some_and(|total| r.at >= total) {
            return Ok(None);
        }
        let bytes = match read(r.at, r.len).await {
            Ok((b, _)) => b,
            Err(SourceError::Parse(_) | SourceError::Http { status: 416 }) => return Ok(None),
            Err(e) => return Err(e),
        };
        s = resume(r, &bytes);
    }
    Ok(match s {
        Sniff::Audio(found) => Some((found, size)),
        _ => None,
    })
}

/// A file on disk.
pub fn sniff_file(path: &Path) -> Option<Sniffed> {
    let mut f = std::fs::File::open(path).ok()?;
    let total = f.metadata().ok()?.len();
    let mut head = Vec::new();
    (&mut f).take(HEAD).read_to_end(&mut head).ok()?;
    let mut s = sniff(&head);
    for _ in 1..MAX_READS {
        let Sniff::More(r) = &s else { break };
        if r.at >= total {
            return None;
        }
        f.seek(SeekFrom::Start(r.at)).ok()?;
        let mut bytes = Vec::new();
        (&mut f).take(r.len).read_to_end(&mut bytes).ok()?;
        s = resume(r, &bytes);
    }
    match s {
        Sniff::Audio(found) => Some(found),
        _ => None,
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

/// The bitrate a lossy file has on average: its size without the tag in
/// front over its length. `None` when either is unknown, or when the size
/// (a server's word) makes no sense.
fn average(s: &Sniffed, size: Option<u64>, secs: Option<u32>) -> Option<u32> {
    let (size, secs) = (size?, secs.filter(|s| *s > 0)?);
    let kbps = u128::from(size.saturating_sub(s.start)) * 8 / u128::from(secs) / 1000;
    (kbps > 0 && kbps <= MAX_LOSSY_KBPS).then(|| u32::try_from(kbps).unwrap_or(u32::MAX))
}

/// The format to offer, the length known: a lossy format without a
/// bitrate (a VBR MP3, Opus, AAC) gets its average; an MP3 whose average
/// is not its first frames' bitrate (a VBR file without a VBR header)
/// gets its average too. Lossless stays as it is.
pub fn with_average(s: &Sniffed, size: Option<u64>, secs: Option<u32>) -> Format {
    let mut f = s.format.clone();
    if f.lossless() {
        return f;
    }
    let avg = average(s, size, secs);
    match (f.bitrate_kbps, avg) {
        (None, avg) => f.bitrate_kbps = avg,
        // More than 5 % apart: the frames read are not the file.
        (Some(k), Some(a)) if f.codec == Codec::Mp3 && u64::from(a.abs_diff(k)) * 20 > u64::from(k) => f.bitrate_kbps = Some(a),
        _ => {}
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
    b.get(i..i.checked_add(N)?)?.try_into().ok()
}

fn u16_le(b: &[u8], i: usize) -> Option<u16> {
    at::<2>(b, i).map(u16::from_le_bytes)
}

fn u32_le(b: &[u8], i: usize) -> Option<u32> {
    at::<4>(b, i).map(u32::from_le_bytes)
}

fn u16_be(b: &[u8], i: usize) -> Option<u16> {
    at::<2>(b, i).map(u16::from_be_bytes)
}

fn u32_be(b: &[u8], i: usize) -> Option<u32> {
    at::<4>(b, i).map(u32::from_be_bytes)
}

fn u64_be(b: &[u8], i: usize) -> Option<u64> {
    at::<8>(b, i).map(u64::from_be_bytes)
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

fn audio(s: Option<Sniffed>) -> Sniff {
    s.map_or(Sniff::Unknown, Sniff::Audio)
}

/// The audio from its first byte (no ID3 tag in front); `b` starts at
/// `base` in the file.
fn bare(b: &[u8], base: u64) -> Sniff {
    let tag = |i: usize, t: &[u8]| b.get(i..i + t.len()) == Some(t);
    if tag(0, b"fLaC") {
        return Sniff::Audio(flac(b, "flac"));
    }
    if tag(0, b"RIFF") && tag(8, b"WAVE") {
        return riff(b, base, base + 12);
    }
    if tag(0, b"FORM") && (tag(8, b"AIFF") || tag(8, b"AIFC")) {
        return aiff(b, base, base + 12, tag(8, b"AIFC"));
    }
    if tag(0, b"OggS") {
        return audio(ogg(b));
    }
    if tag(4, b"ftyp") {
        return mp4(b, base, base);
    }
    if tag(0, b"wvpk") {
        return Sniff::Audio(lossless(Codec::Other("wavpack".into()), "wv", None, None));
    }
    if tag(0, b"MAC ") {
        return Sniff::Audio(lossless(Codec::Other("ape".into()), "ape", None, None));
    }
    audio(mpeg(b))
}

/// FLAC: the STREAMINFO block says the rate and the bits.
fn flac(b: &[u8], ext: &'static str) -> Sniffed {
    let info = (b.get(4).is_some_and(|t| t & 0x7f == 0)).then(|| b.get(8..42)).flatten();
    let Some(si) = info else { return lossless(Codec::Flac, ext, None, None) };
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let bits = ((u16::from(si[12] & 1) << 4) | u16::from(si[13] >> 4)) + 1;
    lossless(Codec::Flac, ext, Some(bits), Some(rate))
}

/// A WAV's chunks from the one at `pos` (`b` starts at `base`): PCM or
/// float samples are lossless; anything packed in it (an MP3, ADPCM) is
/// not told. A chunk past the bytes read is read on.
fn riff(b: &[u8], base: u64, mut pos: u64) -> Sniff {
    loop {
        let Some(rel) = pos.checked_sub(base).and_then(|r| usize::try_from(r).ok()) else { return Sniff::Unknown };
        let (Some(id), Some(size)) = (at::<4>(b, rel), u32_le(b, rel + 4)) else { return more(pos, CHUNK, Step::Riff) };
        if &id == b"fmt " {
            let f = rel + 8;
            let Some(mut kind) = u16_le(b, f) else { return more(pos, CHUNK, Step::Riff) };
            if kind == 0xFFFE {
                // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID's first two bytes.
                let Some(sub) = u16_le(b, f + 24) else { return more(pos, CHUNK, Step::Riff) };
                kind = sub;
            }
            let (Some(rate), Some(bits)) = (u32_le(b, f + 4), u16_le(b, f + 14)) else { return more(pos, CHUNK, Step::Riff) };
            return match kind {
                1 | 3 => Sniff::Audio(lossless(Codec::Wav, "wav", Some(bits), Some(rate))),
                _ => Sniff::Unknown,
            };
        }
        pos = match pos.checked_add(8 + u64::from(size) + u64::from(size & 1)) {
            Some(p) => p,
            None => return Sniff::Unknown,
        };
    }
}

/// AIFF-C compressions whose samples are PCM or float as they are.
const AIFC_PCM: [&[u8; 4]; 11] = [b"NONE", b"none", b"sowt", b"twos", b"raw ", b"in24", b"in32", b"fl32", b"FL32", b"fl64", b"FL64"];

/// An AIFF's chunks from the one at `pos`: PCM samples (big- or
/// little-endian, or float) are lossless, the same as a WAV; an AIFF-C of
/// any other compression is not told.
fn aiff(b: &[u8], base: u64, mut pos: u64, compressed: bool) -> Sniff {
    let step = Step::Aiff { compressed };
    loop {
        let Some(rel) = pos.checked_sub(base).and_then(|r| usize::try_from(r).ok()) else { return Sniff::Unknown };
        let (Some(id), Some(size)) = (at::<4>(b, rel), u32_be(b, rel + 4)) else { return more(pos, CHUNK, step) };
        if &id == b"COMM" {
            let c = rel + 8;
            let (Some(bits), Some(rate)) = (u16_be(b, c + 6), at::<10>(b, c + 8)) else { return more(pos, CHUNK, step) };
            if compressed {
                let Some(kind) = at::<4>(b, c + 18) else { return more(pos, CHUNK, step) };
                if !AIFC_PCM.contains(&&kind) {
                    return Sniff::Unknown;
                }
            }
            return Sniff::Audio(lossless(Codec::Wav, "aiff", Some(bits), extended(&rate)));
        }
        pos = match pos.checked_add(8 + u64::from(size) + u64::from(size & 1)) {
            Some(p) => p,
            None => return Sniff::Unknown,
        };
    }
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

/// The boxes of an MP4 box's content: type and content (cut at the end
/// of `b` when the box goes on past it).
fn boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let (Some(size), Some(ty)) = (u32_be(b, pos), at::<4>(b, pos + 4)) {
        let (head, size) = match size {
            1 => match u64_be(b, pos + 8).and_then(|s| usize::try_from(s).ok()) {
                Some(s) => (16, s),
                None => break,
            },
            0 => (8, b.len() - pos),
            s => (8, s as usize),
        };
        if size < head {
            break;
        }
        let end = pos.saturating_add(size).min(b.len());
        out.push((ty, b.get(pos + head..end).unwrap_or_default()));
        match pos.checked_add(size) {
            Some(next) if next < b.len() => pos = next,
            _ => break,
        }
    }
    out
}

fn child<'a>(b: &'a [u8], ty: &[u8; 4]) -> Vec<&'a [u8]> {
    boxes(b).into_iter().filter(|(t, _)| t == ty).map(|(_, c)| c).collect()
}

/// An MP4's top-level boxes from the one at `pos` (`b` starts at
/// `base`): the audio track's sample entry in `moov` says the codec. A
/// `moov` past the bytes read (after the `mdat`) is read on.
fn mp4(b: &[u8], base: u64, mut pos: u64) -> Sniff {
    loop {
        let Some(rel) = pos.checked_sub(base).and_then(|r| usize::try_from(r).ok()) else { return Sniff::Unknown };
        let (Some(size32), Some(ty)) = (u32_be(b, rel), at::<4>(b, rel + 4)) else { return more(pos, MOOV, Step::Mp4) };
        let size = match size32 {
            1 => match u64_be(b, rel + 8) {
                Some(s) => s,
                None => return more(pos, MOOV, Step::Mp4),
            },
            // To the end of the file: the last box.
            0 => (b.len() - rel) as u64,
            s => u64::from(s),
        };
        if size < 8 {
            return Sniff::Unknown;
        }
        if &ty == b"moov" {
            let end = usize::try_from(size).map_or(b.len(), |s| rel.saturating_add(s).min(b.len()));
            let moov = &b[rel..end];
            if let Some(s) = sample_entry(moov.get(8..).unwrap_or_default()) {
                return audio(s);
            }
            let whole = (end - rel) as u64 >= size.min(MOOV);
            // Cut short by the bytes read: read on from the box itself (once:
            // a read that starts at it has all it gives).
            return if whole || rel == 0 { Sniff::Unknown } else { more(pos, MOOV, Step::Mp4) };
        }
        if size32 == 0 {
            return Sniff::Unknown;
        }
        pos = match pos.checked_add(size) {
            Some(p) => p,
            None => return Sniff::Unknown,
        };
    }
}

/// The audio track's first sample entry in a `moov`'s content:
/// `Some(Some(_))` told, `Some(None)` a codec this does not take (FLAC in
/// MP4, AC-3), `None` no audio sample entry in what was read.
fn sample_entry(moov: &[u8]) -> Option<Option<Sniffed>> {
    for trak in child(moov, b"trak") {
        let stsd = child(trak, b"mdia")
            .into_iter()
            .flat_map(|m| child(m, b"minf"))
            .flat_map(|m| child(m, b"stbl"))
            .flat_map(|s| child(s, b"stsd"))
            .next();
        let Some(stsd) = stsd else { continue };
        // A full box: version and flags, the number of entries, then the entries.
        let Some((ty, entry)) = boxes(stsd.get(8..).unwrap_or_default()).into_iter().next() else { continue };
        // An audio sample entry: 8 reserved, version, revision, vendor,
        // channels (16), sample size (18), …, rate 16.16 (24); boxes from 28.
        let bits = u16_be(entry, 18);
        let rate = u32_be(entry, 24).map(|r| r >> 16);
        return Some(match &ty {
            b"mp4a" => Some(lossy(Codec::Aac, "m4a", None)),
            b"Opus" => Some(lossy(Codec::Opus, "m4a", None)),
            b"alac" => {
                // The ALAC cookie has the real depth and rate (a rate of
                // 96 kHz does not fit the entry's 16.16).
                let cookie = child(entry.get(28..).unwrap_or_default(), b"alac").first().copied();
                let bits = cookie.and_then(|c| c.get(9)).map(|b| u16::from(*b)).or(bits);
                let rate = cookie.and_then(|c| u32_be(c, 24)).or(rate);
                Some(lossless(Codec::Alac, "m4a", bits, rate))
            }
            b"avc1" | b"hvc1" | b"hev1" | b"mp4v" | b"text" | b"tx3g" => continue,
            _ => None,
        });
    }
    None
}

const MP3_V1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const MP3_V2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
/// Frames looked at for a bitrate that changes.
const FRAMES: usize = 64;

/// An MPEG-1/2/2.5 Layer III frame header at `i`: its bitrate and length.
fn frame(b: &[u8], i: usize) -> Option<(u32, usize)> {
    let h = b.get(i..i + 4)?;
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3;
    let layer = (h[1] >> 1) & 3;
    let bi = usize::from(h[2] >> 4);
    let si = usize::from((h[2] >> 2) & 3);
    if version == 1 || layer != 1 || bi == 0 || bi == 15 || si == 3 {
        return None;
    }
    let v1 = version == 3;
    let kbps = if v1 { MP3_V1[bi] } else { MP3_V2[bi] };
    let rate: u32 = [[44_100, 48_000, 32_000], [22_050, 24_000, 16_000], [11_025, 12_000, 8_000]][match version {
        3 => 0,
        2 => 1,
        _ => 2,
    }][si];
    let pad = u32::from((h[2] >> 1) & 1);
    let per: u32 = if v1 { 144 } else { 72 };
    Some((kbps, (per * kbps * 1000 / rate + pad) as usize))
}

/// An MPEG audio frame (or raw AAC) within the first bytes (padding may
/// stand before it), with a second frame where the first says it ends
/// when the bytes reach that far, so a stray `FF Ex` is not taken for
/// one. The bitrate is the frames' when they all have the same one and no
/// VBR header (Xing, after the CRC when there is one, or VBRI) says
/// otherwise; else none, for [`with_average`] to make of the size.
fn mpeg(b: &[u8]) -> Option<Sniffed> {
    let scan = b.len().saturating_sub(4).min(4096);
    for i in 0..scan {
        if b[i] != 0xFF || b[i + 1] & 0xE0 != 0xE0 {
            continue;
        }
        if (b[i + 1] >> 1) & 3 == 0 && b[i + 1] & 0xF6 == 0xF0 {
            return Some(lossy(Codec::Aac, "aac", None));
        }
        let Some((kbps, len)) = frame(b, i) else { continue };
        if b.get(i + len..i + len + 4).is_some() && frame(b, i + len).is_none() {
            continue;
        }
        let v1 = (b[i + 1] >> 3) & 3 == 3;
        let mono = b[i + 3] >> 6 == 3;
        let crc = if b[i + 1] & 1 == 0 { 2 } else { 0 };
        let side = match (v1, mono) {
            (true, true) => 17,
            (true, false) => 32,
            (false, true) => 9,
            (false, false) => 17,
        };
        let x = i + 4 + crc + side;
        let header = b.get(x..x + 4);
        let vbr_header = header == Some(&b"Xing"[..]) || b.get(i + 36..i + 40) == Some(&b"VBRI"[..]);
        // A Xing or Info frame is silence of its own bitrate: the next
        // frames say what the music is.
        let tagged = vbr_header || header == Some(&b"Info"[..]);
        let mut rates: Vec<u32> = Vec::new();
        let mut next = i;
        while rates.len() < FRAMES {
            let Some((k, l)) = frame(b, next) else { break };
            if !(tagged && next == i) {
                rates.push(k);
            }
            next += l.max(1);
        }
        let steady = rates.first().copied().filter(|r| rates.iter().all(|x| x == r));
        let kbps = if vbr_header { None } else { steady.or(rates.is_empty().then_some(kbps)) };
        return Some(lossy(Codec::Mp3, "mp3", kbps));
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

    fn fmt_chunk(kind: u16, bits: u16, rate: u32) -> Vec<u8> {
        let mut b = b"fmt ".to_vec();
        b.extend(16u32.to_le_bytes());
        b.extend(kind.to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(rate.to_le_bytes());
        b.extend((rate * 2 * u32::from(bits) / 8).to_le_bytes());
        b.extend((2 * bits / 8).to_le_bytes());
        b.extend(bits.to_le_bytes());
        b
    }

    fn wav_head(kind: u16, bits: u16, rate: u32) -> Vec<u8> {
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        b.extend(b"LIST");
        b.extend(4u32.to_le_bytes());
        b.extend(b"INFO");
        b.extend(fmt_chunk(kind, bits, rate));
        b.extend(b"data\0\0\0\0");
        b
    }

    /// A WAV whose `fmt ` stands past a `JUNK` chunk of `junk` bytes.
    fn wav_after_junk(kind: u16, junk: usize) -> Vec<u8> {
        let mut b = b"RIFF\0\0\0\0WAVEJUNK".to_vec();
        b.extend((junk as u32).to_le_bytes());
        b.resize(b.len() + junk, 0);
        b.extend(fmt_chunk(kind, 16, 44_100));
        b.extend(b"data\0\0\0\0");
        b
    }

    fn aiff_head(form: &[u8; 4], compression: Option<&[u8; 4]>) -> Vec<u8> {
        let mut b = b"FORM\0\0\0\0".to_vec();
        b.extend(form);
        b.extend(b"COMM");
        b.extend((if compression.is_some() { 22u32 } else { 18 }).to_be_bytes());
        b.extend(2u16.to_be_bytes());
        b.extend(1000u32.to_be_bytes());
        b.extend(16u16.to_be_bytes());
        b.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        if let Some(c) = compression {
            b.extend(c);
        }
        b
    }

    /// MPEG-1 Layer III frames at these bitrates, 44.1 kHz, stereo; the
    /// first one a Xing frame when asked.
    fn mp3_rates(rates: &[u32], xing: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for (n, kbps) in rates.iter().enumerate() {
            let bi = MP3_V1.iter().position(|k| k == kbps).unwrap() as u8;
            let len = (144 * kbps * 1000 / 44_100) as usize;
            let mut f = vec![0xFF, 0xFB, bi << 4, 0x00];
            f.resize(len, 0);
            if n == 0 && xing {
                f[36..40].copy_from_slice(b"Xing");
            }
            out.extend(f);
        }
        out
    }

    fn mp3_frames(kbps: u32, xing: bool) -> Vec<u8> {
        mp3_rates(&[kbps; 3], xing)
    }

    fn mp4_box(ty: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut b = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend(ty);
        b.extend(content);
        b
    }

    /// An `moov` with one audio track whose sample entry is `entry`
    /// (with `extra` boxes inside it).
    fn moov(entry: &[u8; 4], extra: &[u8]) -> Vec<u8> {
        let mut e = vec![0u8; 16];
        e.extend(2u16.to_be_bytes());
        e.extend(16u16.to_be_bytes());
        e.extend([0u8; 4]);
        e.extend((44_100u32 << 16).to_be_bytes());
        e.extend(extra);
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(mp4_box(entry, &e));
        let stbl = mp4_box(b"stbl", &mp4_box(b"stsd", &stsd));
        let trak = mp4_box(b"trak", &mp4_box(b"mdia", &mp4_box(b"minf", &stbl)));
        mp4_box(b"moov", &trak)
    }

    fn told(b: &[u8]) -> (String, &'static str) {
        match sniff(b) {
            Sniff::Audio(s) => (s.format.label(), s.ext),
            other => panic!("{other:?}"),
        }
    }

    /// Reads `file` the way a source reads a download.
    fn remote(file: &[u8]) -> Option<(Sniffed, Option<u64>)> {
        let file = file.to_vec();
        let total = file.len() as u64;
        let read = |from: u64, len: u64| {
            let file = file.clone();
            async move {
                let from = (from as usize).min(file.len());
                let to = from.saturating_add(len as usize).min(file.len());
                Ok::<_, SourceError>((file[from..to].to_vec(), Some(total)))
            }
        };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(sniff_remote(read)).unwrap()
    }

    #[test]
    fn lossless_files_say_their_rate_and_bits() {
        assert_eq!(told(&streaminfo(44_100, 16)), ("FLAC".into(), "flac"));
        assert_eq!(told(&streaminfo(96_000, 24)), ("FLAC 24/96".into(), "flac"));
        assert_eq!(told(&wav_head(1, 16, 44_100)), ("WAV".into(), "wav"));
        assert_eq!(told(&wav_head(1, 24, 48_000)), ("WAV 24/48".into(), "wav"));
        assert_eq!(told(&wav_head(3, 32, 96_000)), ("WAV 32/96".into(), "wav"));
        assert_eq!(told(&aiff_head(b"AIFF", None)), ("WAV".into(), "aiff"));
        let Sniff::Audio(s) = sniff(&aiff_head(b"AIFF", None)) else { panic!() };
        assert_eq!((s.format.sample_rate, s.format.bit_depth), (Some(44_100), Some(16)));
        assert_eq!(told(&aiff_head(b"AIFC", Some(b"sowt"))), ("WAV".into(), "aiff"));
    }

    /// The container is not the samples: an MP3 or ADPCM in a WAV, a
    /// compressed AIFF-C, a format chunk nobody read, are never lossless.
    #[test]
    fn packed_samples_are_not_lossless() {
        assert_eq!(sniff(&wav_head(0x55, 0, 44_100)), Sniff::Unknown, "MP3 in a WAV");
        assert_eq!(sniff(&wav_head(0x02, 4, 44_100)), Sniff::Unknown, "MS ADPCM");
        assert_eq!(sniff(&aiff_head(b"AIFC", Some(b"ima4"))), Sniff::Unknown);
        assert_eq!(sniff(&aiff_head(b"AIFC", Some(b"ulaw"))), Sniff::Unknown);
        // The format chunk past the head: read on there, never assumed.
        let far = wav_after_junk(0x55, 70_000);
        let Sniff::More(r) = sniff(&far[..HEAD as usize]) else { panic!("{:?}", sniff(&far[..HEAD as usize])) };
        assert_eq!(r.at, 12 + 8 + 70_000);
        assert_eq!(resume(&r, &far[r.at as usize..]), Sniff::Unknown, "an MP3 after the junk");
        assert!(remote(&far).is_none());
        let pcm = wav_after_junk(1, 70_000);
        assert_eq!(remote(&pcm).unwrap().0.format.label(), "WAV");
        // Only the head of a WAV and nothing more to read: not offered.
        assert!(remote(&far[..HEAD as usize]).is_none());
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
        assert_eq!(sniff(&big), Sniff::More(Resume { at: 10 + 65_536, len: AFTER_TAG, step: Step::AfterTag }));
    }

    /// A VBR file without a VBR header is not its first frame.
    #[test]
    fn a_vbr_mp3_without_a_header_is_its_average() {
        let Sniff::Audio(s) = sniff(&mp3_rates(&[320, 128, 160, 96, 128], false)) else { panic!() };
        assert_eq!(s.format.bitrate_kbps, None, "frames that differ say nothing");
        assert_eq!(with_average(&s, Some(4_000_000), Some(250)).bitrate_kbps, Some(128));
        // Steady frames, but the size says otherwise: the size wins.
        let Sniff::Audio(s) = sniff(&mp3_frames(320, false)) else { panic!() };
        assert_eq!(with_average(&s, Some(4_000_000), Some(250)).bitrate_kbps, Some(128));
        // Steady frames and a size that agrees (tags at the end): the frames.
        assert_eq!(with_average(&s, Some(9_900_000), Some(250)).bitrate_kbps, Some(320));
        // A Xing header after a CRC.
        let mut crc = mp3_rates(&[128, 128, 128], false);
        crc[1] = 0xFA;
        crc[38..42].copy_from_slice(b"Xing");
        let Sniff::Audio(s) = sniff(&crc) else { panic!() };
        assert_eq!(s.format.bitrate_kbps, None);
        // An Info frame of 320 before music at 192: the music.
        let mut info = mp3_rates(&[320, 192, 192, 192], false);
        info[36..40].copy_from_slice(b"Info");
        assert_eq!(told(&info).0, "MP3 192");
    }

    /// A size from a server that lies neither overflows nor ranks.
    #[test]
    fn a_huge_size_does_not_overflow() {
        let Sniff::Audio(s) = sniff(&mp3_frames(128, true)) else { panic!() };
        assert_eq!(with_average(&s, Some(u64::MAX), Some(1)).bitrate_kbps, None);
        assert_eq!(with_average(&s, Some(u64::MAX / 4), Some(200)).bitrate_kbps, None);
        assert_eq!(with_average(&s, Some(100), None).bitrate_kbps, None);
    }

    #[test]
    fn mp4_is_told_by_its_sample_entry() {
        let ftyp = mp4_box(b"ftyp", b"M4A \0\0\0\0M4A mp42");
        let aac = [ftyp.clone(), moov(b"mp4a", &[])].concat();
        assert_eq!(told(&aac), ("AAC".into(), "m4a"));
        // «Palace» in the tags is not ALAC.
        let tagged = [ftyp.clone(), mp4_box(b"udta", b"\xa9nam Palace alac"), moov(b"mp4a", &[])].concat();
        assert_eq!(told(&tagged), ("AAC".into(), "m4a"));
        let mut cookie = vec![0u8; 4];
        cookie.extend(4096u32.to_be_bytes());
        cookie.extend([0, 24, 40, 10, 14, 2, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0]);
        cookie.extend(96_000u32.to_be_bytes());
        let alac = [ftyp.clone(), moov(b"alac", &mp4_box(b"alac", &cookie))].concat();
        assert_eq!(told(&alac), ("ALAC 24/96".into(), "m4a"));
        // The moov after a big mdat: read on there.
        let mut late = ftyp.clone();
        let mdat_len = 300_000usize;
        late.extend(((mdat_len + 8) as u32).to_be_bytes());
        late.extend(b"mdat");
        late.resize(late.len() + mdat_len, 0x61);
        late.extend(moov(b"alac", &mp4_box(b"alac", &cookie)));
        assert!(matches!(sniff(&late[..HEAD as usize]), Sniff::More(_)));
        assert_eq!(remote(&late).unwrap().0.format.label(), "ALAC 24/96");
        // An mdat with «alac» in it and no moov read: not lossless.
        let mut cut = ftyp.clone();
        cut.extend(((mdat_len + 8) as u32).to_be_bytes());
        cut.extend(b"mdat");
        cut.extend(b"alacalacalac");
        cut.resize(HEAD as usize, 0);
        assert!(remote(&cut).is_none());
    }

    #[test]
    fn ogg_and_the_rest() {
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
        // A WAV whose fmt is an MP3's, past a long JUNK chunk.
        let w = dir.path().join("packed.wav");
        std::fs::write(&w, wav_after_junk(0x55, 90_000)).unwrap();
        assert_eq!(sniff_file(&w), None);
    }
}
