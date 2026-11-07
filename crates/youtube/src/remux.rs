//! WebM (Matroska) Opus → Ogg Opus, packet for packet, no re-encoding.
//!
//! The Ogg stream (RFC 7845) gets the `OpusHead` from the track's
//! `CodecPrivate` (its pre-skip set from `CodecDelay` when present), an
//! `OpusTags` with what is known about the track, and the audio packets with
//! granule positions counted from each packet's TOC. The last page's granule
//! drops the samples the WebM marks as padding (`DiscardPadding`, or the
//! samples past the segment `Duration`), so the length stays exact.

use std::io::Write;

use ogg::writing::{PacketWriteEndInfo, PacketWriter};

/// Matroska element ids used here.
mod id {
    pub const EBML: u32 = 0x1A45_DFA3;
    pub const SEGMENT: u32 = 0x1853_8067;
    pub const INFO: u32 = 0x1549_A966;
    pub const TIMECODE_SCALE: u32 = 0x2A_D7B1;
    pub const DURATION: u32 = 0x4489;
    pub const TRACKS: u32 = 0x1654_AE6B;
    pub const TRACK_ENTRY: u32 = 0xAE;
    pub const TRACK_NUMBER: u32 = 0xD7;
    pub const CODEC_ID: u32 = 0x86;
    pub const CODEC_PRIVATE: u32 = 0x63A2;
    pub const CODEC_DELAY: u32 = 0x56AA;
    pub const CLUSTER: u32 = 0x1F43_B675;
    pub const TIMECODE: u32 = 0xE7;
    pub const POSITION: u32 = 0xA7;
    pub const PREV_SIZE: u32 = 0xAB;
    pub const SIMPLE_BLOCK: u32 = 0xA3;
    pub const BLOCK_GROUP: u32 = 0xA0;
    pub const BLOCK: u32 = 0xA1;
    pub const BLOCK_DURATION: u32 = 0x9B;
    pub const REFERENCE_BLOCK: u32 = 0xFB;
    pub const DISCARD_PADDING: u32 = 0x75A2;
    pub const VOID: u32 = 0xEC;
    pub const CRC32: u32 = 0xBF;
}

/// Opus always runs at 48 kHz on the granule timeline.
const OPUS_RATE: u64 = 48_000;
/// Force a page end after this many packets (about a second of 20 ms
/// packets), so pages stay small for seeking.
const PACKETS_PER_PAGE: usize = 50;

/// Text tags written into `OpusTags`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
}

/// The Opus track of a WebM file.
#[derive(Debug, Default)]
pub(crate) struct OpusTrack {
    pub head: Vec<u8>,
    pub codec_delay_ns: Option<u64>,
    pub packets: Vec<Vec<u8>>,
    /// Padding at the end, in nanoseconds (from the last `BlockGroup`).
    pub discard_padding_ns: i64,
    /// Segment duration in nanoseconds.
    pub duration_ns: Option<f64>,
}

struct Reader<'a> {
    data: &'a [u8],
}

/// An element header: id, body start, body end (`None` = unknown size).
struct Header {
    id: u32,
    body: usize,
    end: Option<usize>,
}

impl Reader<'_> {
    fn vint(&self, pos: usize, keep_marker: bool) -> Result<(u64, usize, bool), String> {
        let first = *self.data.get(pos).ok_or("unexpected end of data")?;
        if first == 0 {
            return Err(format!("bad EBML number at byte {pos}"));
        }
        let len = first.leading_zeros() as usize + 1;
        let bytes = self
            .data
            .get(pos..pos + len)
            .ok_or("unexpected end of data")?;
        let mut v = if keep_marker {
            u64::from(first)
        } else {
            u64::from(first) & (0xFF >> len)
        };
        for &b in &bytes[1..] {
            v = (v << 8) | u64::from(b);
        }
        let all_ones = !keep_marker && v == (1u64 << (7 * len)) - 1;
        Ok((v, len, all_ones))
    }

    fn header(&self, pos: usize) -> Result<Header, String> {
        let (id, id_len, _) = self.vint(pos, true)?;
        let (size, size_len, unknown) = self.vint(pos + id_len, false)?;
        let body = pos + id_len + size_len;
        let end = if unknown {
            None
        } else {
            let end = body
                .checked_add(usize::try_from(size).map_err(|_| "element too large")?)
                .ok_or("element too large")?;
            Some(end.min(self.data.len()))
        };
        Ok(Header {
            id: id as u32,
            body,
            end,
        })
    }

    fn uint(&self, h: &Header) -> u64 {
        let end = h.end.unwrap_or(h.body);
        self.data[h.body..end]
            .iter()
            .fold(0, |v, &b| (v << 8) | u64::from(b))
    }

    fn sint(&self, h: &Header) -> i64 {
        let bytes = &self.data[h.body..h.end.unwrap_or(h.body)];
        if bytes.is_empty() {
            return 0;
        }
        let v = bytes.iter().fold(0u64, |v, &b| (v << 8) | u64::from(b));
        let shift = 64 - 8 * bytes.len() as u32;
        ((v << shift) as i64) >> shift
    }

    fn float(&self, h: &Header) -> Option<f64> {
        let bytes = &self.data[h.body..h.end.unwrap_or(h.body)];
        match bytes.len() {
            4 => Some(f64::from(f32::from_be_bytes(bytes.try_into().ok()?))),
            8 => Some(f64::from_be_bytes(bytes.try_into().ok()?)),
            _ => None,
        }
    }

    fn bytes(&self, h: &Header) -> &[u8] {
        &self.data[h.body..h.end.unwrap_or(h.body)]
    }
}

fn is_cluster_child(id: u32) -> bool {
    matches!(
        id,
        id::TIMECODE
            | id::POSITION
            | id::PREV_SIZE
            | id::SIMPLE_BLOCK
            | id::BLOCK_GROUP
            | id::VOID
            | id::CRC32
    )
}

/// Pulls the first Opus track out of a WebM file.
pub(crate) fn demux_opus(data: &[u8]) -> Result<OpusTrack, String> {
    let r = Reader { data };
    let ebml = r.header(0)?;
    if ebml.id != id::EBML {
        return Err("not a WebM/Matroska file".into());
    }
    let mut pos = ebml.end.ok_or("bad EBML header")?;
    let seg = loop {
        if pos >= data.len() {
            return Err("no Segment".into());
        }
        let h = r.header(pos)?;
        if h.id == id::SEGMENT {
            break h;
        }
        pos = h.end.ok_or("bad top-level element")?;
    };
    let seg_end = seg.end.unwrap_or(data.len());
    let mut out = OpusTrack::default();
    let mut timecode_scale = 1_000_000u64;
    let mut duration = None;
    let mut track_no = None;
    pos = seg.body;
    while pos < seg_end {
        let h = r.header(pos)?;
        match h.id {
            id::INFO => {
                for c in children(&r, &h)? {
                    match c.id {
                        id::TIMECODE_SCALE => timecode_scale = r.uint(&c),
                        id::DURATION => duration = r.float(&c),
                        _ => {}
                    }
                }
            }
            id::TRACKS if track_no.is_none() => {
                for entry in children(&r, &h)?
                    .into_iter()
                    .filter(|c| c.id == id::TRACK_ENTRY)
                {
                    let mut number = None;
                    let mut opus = false;
                    let mut head = Vec::new();
                    let mut delay = None;
                    for c in children(&r, &entry)? {
                        match c.id {
                            id::TRACK_NUMBER => number = Some(r.uint(&c)),
                            id::CODEC_ID => opus = r.bytes(&c) == b"A_OPUS",
                            id::CODEC_PRIVATE => head = r.bytes(&c).to_vec(),
                            id::CODEC_DELAY => delay = Some(r.uint(&c)),
                            _ => {}
                        }
                    }
                    if opus {
                        track_no = number;
                        out.head = head;
                        out.codec_delay_ns = delay;
                        break;
                    }
                }
                if track_no.is_none() {
                    return Err("no Opus track".into());
                }
            }
            id::CLUSTER => {
                let track = track_no.ok_or("cluster before the track list")?;
                let end = cluster(&r, &h, track, &mut out)?;
                pos = end;
                continue;
            }
            _ => {}
        }
        pos = h.end.ok_or("unknown-size element in segment")?;
    }
    if track_no.is_none() {
        return Err("no Opus track".into());
    }
    out.duration_ns = duration.map(|d| d * timecode_scale as f64);
    Ok(out)
}

/// The children of a known-size master element.
fn children(r: &Reader<'_>, parent: &Header) -> Result<Vec<Header>, String> {
    let end = parent.end.ok_or("unknown-size element")?;
    let mut pos = parent.body;
    let mut out = Vec::new();
    while pos < end {
        let h = r.header(pos)?;
        pos = h.end.ok_or("unknown-size element")?;
        out.push(h);
    }
    Ok(out)
}

/// Collects the blocks of one cluster; returns where the cluster ends
/// (an unknown-size cluster ends at the first element that is not its child).
fn cluster(r: &Reader<'_>, h: &Header, track: u64, out: &mut OpusTrack) -> Result<usize, String> {
    let end = h.end.unwrap_or(r.data.len());
    let mut pos = h.body;
    while pos < end {
        let c = r.header(pos)?;
        if h.end.is_none() && !is_cluster_child(c.id) {
            return Ok(pos);
        }
        let c_end = c.end.ok_or("unknown-size element in cluster")?;
        match c.id {
            id::SIMPLE_BLOCK => block(r, &c, track, out)?,
            id::BLOCK_GROUP => {
                let mut padding = 0;
                let mut got = false;
                for g in children(r, &c)? {
                    match g.id {
                        id::BLOCK => got = block(r, &g, track, out).map(|()| true)?,
                        id::DISCARD_PADDING => padding = r.sint(&g),
                        id::BLOCK_DURATION | id::REFERENCE_BLOCK => {}
                        _ => {}
                    }
                }
                if got && padding > 0 {
                    out.discard_padding_ns = padding;
                }
            }
            _ => {}
        }
        pos = c_end;
    }
    Ok(end)
}

/// One (Simple)Block: track number, 16-bit timecode, flags, frame.
fn block(r: &Reader<'_>, h: &Header, track: u64, out: &mut OpusTrack) -> Result<(), String> {
    let body = r.bytes(h);
    let rd = Reader { data: body };
    let (number, len, _) = rd.vint(0, false)?;
    if number != track {
        return Ok(());
    }
    let flags = *body.get(len + 2).ok_or("short block")?;
    if flags & 0x06 != 0 {
        return Err("laced blocks are not supported".into());
    }
    out.packets.push(body[len + 3..].to_vec());
    Ok(())
}

/// Samples (at 48 kHz) in one Opus packet, from its TOC byte (RFC 6716 §3.1).
pub(crate) fn packet_samples(packet: &[u8]) -> u64 {
    let Some(&toc) = packet.first() else {
        return 0;
    };
    let config = toc >> 3;
    let frame = match config {
        0..=11 => [480, 960, 1920, 2880][usize::from(config % 4)],
        12..=15 => [480, 960][usize::from(config % 2)],
        _ => [120, 240, 480, 960][usize::from(config % 4)],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => packet.get(1).map_or(0, |b| u64::from(b & 0x3F)),
    };
    frame * frames
}

fn opus_tags(tags: &Tags) -> Vec<u8> {
    let vendor = concat!("nikgob ", env!("CARGO_PKG_VERSION"));
    let comments: Vec<String> = [
        ("TITLE", &tags.title),
        ("ARTIST", &tags.artist),
        ("ALBUM", &tags.album),
    ]
    .into_iter()
    .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={v}")))
    .collect();
    let mut out = b"OpusTags".to_vec();
    out.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    out.extend_from_slice(vendor.as_bytes());
    out.extend_from_slice(&(comments.len() as u32).to_le_bytes());
    for c in comments {
        out.extend_from_slice(&(c.len() as u32).to_le_bytes());
        out.extend_from_slice(c.as_bytes());
    }
    out
}

/// Remuxes a WebM Opus file into Ogg Opus; `serial` names the logical stream.
pub(crate) fn webm_to_ogg(
    webm: &[u8],
    tags: &Tags,
    serial: u32,
    out: impl Write,
) -> Result<(), String> {
    let track = demux_opus(webm)?;
    let mut head = track.head;
    if head.len() < 19 || &head[..8] != b"OpusHead" {
        return Err("the Opus track has no valid OpusHead".into());
    }
    if head[9] > 2 {
        return Err("multichannel Opus is not supported".into());
    }
    if let Some(ns) = track.codec_delay_ns {
        let pre_skip = (ns * OPUS_RATE).div_ceil(1_000_000_000);
        let pre_skip = u16::try_from(pre_skip).map_err(|_| "codec delay too large")?;
        head[10..12].copy_from_slice(&pre_skip.to_le_bytes());
    }
    let pre_skip = u64::from(u16::from_le_bytes([head[10], head[11]]));
    if track.packets.is_empty() {
        return Err("the Opus track has no audio".into());
    }

    let total: u64 = track.packets.iter().map(|p| packet_samples(p)).sum();
    let padding = if track.discard_padding_ns > 0 {
        (track.discard_padding_ns as u64 * OPUS_RATE).div_ceil(1_000_000_000)
    } else if let Some(d) = track.duration_ns {
        // Only a trim within the last packet is believable.
        let exact = pre_skip + (d * OPUS_RATE as f64 / 1e9).round() as u64;
        let last = track.packets.last().map_or(0, |p| packet_samples(p));
        total.checked_sub(exact).filter(|&p| p < last).unwrap_or(0)
    } else {
        0
    };

    let mut w = PacketWriter::new(out);
    let io = |e: std::io::Error| e.to_string();
    w.write_packet(head, serial, PacketWriteEndInfo::EndPage, 0)
        .map_err(io)?;
    w.write_packet(opus_tags(tags), serial, PacketWriteEndInfo::EndPage, 0)
        .map_err(io)?;
    let n = track.packets.len();
    let mut granule = 0u64;
    for (i, p) in track.packets.into_iter().enumerate() {
        granule += packet_samples(&p);
        let info = if i + 1 == n {
            granule = granule.saturating_sub(padding).max(pre_skip);
            PacketWriteEndInfo::EndStream
        } else if (i + 1) % PACKETS_PER_PAGE == 0 {
            PacketWriteEndInfo::EndPage
        } else {
            PacketWriteEndInfo::NormalPacket
        };
        w.write_packet(p, serial, info, granule).map_err(io)?;
    }
    w.into_inner().flush().map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toc_sample_counts() {
        // CELT 20 ms, one frame.
        assert_eq!(packet_samples(&[0xF8, 0]), 960);
        // SILK 60 ms, two frames.
        assert_eq!(packet_samples(&[(3 << 3) | 1]), 5760);
        // Hybrid 10 ms, code 3 with 3 frames.
        assert_eq!(packet_samples(&[(12 << 3) | 3, 3]), 1440);
        // CELT 2.5 ms.
        assert_eq!(packet_samples(&[16 << 3]), 120);
        assert_eq!(packet_samples(&[]), 0);
    }

    #[test]
    fn tags_packet_layout() {
        let t = opus_tags(&Tags {
            title: Some("Группа крови".into()),
            artist: Some("Кино".into()),
            album: None,
        });
        assert_eq!(&t[..8], b"OpusTags");
        let vlen = u32::from_le_bytes(t[8..12].try_into().unwrap()) as usize;
        let count = u32::from_le_bytes(t[12 + vlen..16 + vlen].try_into().unwrap());
        assert_eq!(count, 2);
        let first = 16 + vlen;
        let clen = u32::from_le_bytes(t[first..first + 4].try_into().unwrap()) as usize;
        assert_eq!(
            std::str::from_utf8(&t[first + 4..first + 4 + clen]).unwrap(),
            "TITLE=Группа крови"
        );
    }

    /// Frames nikgob-audio decodes from a file (48 kHz stereo).
    fn decoded_frames(path: &std::path::Path) -> usize {
        let (player, _events, mut device) =
            nikgob_audio::Player::offline(nikgob_audio::OfflineOptions::default()).unwrap();
        player.play(nikgob_audio::PlayItem::file(1, path));
        device.drain(48_000 * 20).len() / 2
    }

    #[test]
    fn remuxes_the_webm_fixture_into_ogg_opus() {
        let webm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../audio/tests/fixtures/tone.webm");
        let webm = std::fs::read(&webm_path).unwrap();
        let tags = Tags {
            title: Some("Tone".into()),
            artist: Some("Кино".into()),
            album: None,
        };
        let mut ogg_bytes = Vec::new();
        webm_to_ogg(&webm, &tags, 7, &mut ogg_bytes).unwrap();

        // Structure: OpusHead (BOS) on its own page, OpusTags, then audio
        // with growing granules, the last packet ending the stream.
        let demuxed = demux_opus(&webm).unwrap();
        let mut r = ogg::PacketReader::new(std::io::Cursor::new(&ogg_bytes));
        let head = r.read_packet().unwrap().unwrap();
        assert!(head.first_in_stream());
        assert_eq!(&head.data[..8], b"OpusHead");
        assert_eq!(head.absgp_page(), 0);
        assert_eq!(head.stream_serial(), 7);
        let tags_pkt = r.read_packet().unwrap().unwrap();
        assert_eq!(&tags_pkt.data[..8], b"OpusTags");
        assert!(tags_pkt.data.windows(9).any(|w| w == b"TITLE=Ton"));
        let mut audio = Vec::new();
        while let Some(p) = r.read_packet().unwrap() {
            audio.push(p);
        }
        assert_eq!(audio.len(), demuxed.packets.len());
        assert!(
            audio
                .iter()
                .zip(&demuxed.packets)
                .all(|(a, b)| &a.data == b)
        );
        let last = audio.last().unwrap();
        assert!(last.last_in_stream());
        let granules: Vec<u64> = audio.iter().map(|p| p.absgp_page()).collect();
        assert!(granules.windows(2).all(|w| w[0] <= w[1]));
        let pre_skip = u64::from(u16::from_le_bytes([head.data[10], head.data[11]]));
        if let Some(ns) = demuxed.codec_delay_ns {
            assert_eq!(pre_skip, (ns * 48_000).div_ceil(1_000_000_000));
        }
        let total: u64 = demuxed.packets.iter().map(|p| packet_samples(p)).sum();
        assert!(last.absgp_page() <= total && last.absgp_page() + 960 > total - 960);

        // nikgob-audio decodes both to the same length (within a frame).
        let dir = std::env::temp_dir().join(format!("nikgob-yt-remux-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ogg_path = dir.join("tone.opus");
        std::fs::write(&ogg_path, &ogg_bytes).unwrap();
        let from_webm = decoded_frames(&webm_path);
        let from_ogg = decoded_frames(&ogg_path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(from_webm > 40_000, "{from_webm} frames from the WebM");
        assert!(
            from_webm.abs_diff(from_ogg) <= 960,
            "WebM {from_webm} frames, Ogg {from_ogg} frames"
        );
        assert_eq!(
            last.absgp_page() - pre_skip,
            from_ogg as u64,
            "the Ogg length is what its last granule says"
        );
    }

    #[test]
    fn rejects_non_webm() {
        assert!(demux_opus(b"OggS\0\0\0\0").is_err());
        assert!(demux_opus(&[]).is_err());
    }
}
