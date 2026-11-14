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
//!
//! Opus in Matroska / WebM (YouTube itag 251) goes through the same `ropus`
//! wrapper. There the demuxer has already subtracted the `CodecDelay` from
//! the block timestamps, so the timeline origin is 0; as the delay is
//! rounded to the millisecond, the first packet is snapped to exactly
//! `-pre_skip`. `DiscardPadding` (end trim) is not applied by Symphonia's
//! Matroska demuxer, so a WebM stream may end a few milliseconds late.
//!
//! Symphonia 0.6's Matroska seek lands (and labels packets) up to a cluster
//! off when it uses the cues, so Matroska is seeked without them: the
//! container is opened again and packets are skipped, without decoding, up
//! to the target. A target ahead of what was demuxed skips on from there
//! without the reopen, so seeking forward costs the distance, not the
//! position.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_MP1, CODEC_ID_MP2, CODEC_ID_MP3, CODEC_ID_OPUS, CODEC_ID_VORBIS,
};
use symphonia::core::codecs::audio::{AudioCodecId, AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::core::units::{TimeBase, Timestamp};

use super::opus::{OpusHead, OpusPackets};
use super::{FrameSource, ReadStatus};
use crate::cancel::Slot;
use crate::types::{ByteSource, DecodeError};

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
    /// Matroska Opus: the first packet's exact timeline index (`-pre_skip`),
    /// used once at the start of the stream (see the module docs).
    exact_start: Option<i64>,
    /// Matroska Opus: frames added to every packet position, the rounding of
    /// the `CodecDelay` learnt from the first packet.
    pts_fix: i64,
    /// Matroska: seek by reopening and skipping packets (see the module docs).
    reopen: Option<Reopen>,
    /// A packet read ahead by such a seek, handed to the next `read`.
    held: Option<Packet>,
    /// Matroska: timeline end of the last packet of the track the demuxer
    /// gave, so a seek past it skips on from there instead of reopening.
    demuxed_end: i64,
}

/// Opens the container again from its first byte (Matroska seeking).
type Reopen = Box<dyn Fn() -> Result<Box<dyn FormatReader>, DecodeError> + Send>;

/// A [`ByteSource`] as Symphonia's `MediaSource` (which must be `Sync`). The
/// source is shared so its gain and duration can be asked after the probe.
/// The first read error is kept: the probe reads past it as the end of
/// the bytes, and the stream's own reason (a refused key, an age gate)
/// says more than «no suitable format reader». A read that gave up is told
/// to the item's slot, which knows whether the engine made it (see
/// [`crate::cancel`]).
struct SharedBytes(
    Arc<Mutex<Box<dyn ByteSource>>>,
    Arc<Mutex<Option<String>>>,
    Option<Arc<Slot>>,
);

impl SharedBytes {
    fn with<T>(&self, f: impl FnOnce(&mut Box<dyn ByteSource>) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl Read for SharedBytes {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.with(|s| s.read(buf)).inspect_err(|e| {
            // Only a read that gave up (see `Interrupter`) may have given
            // way: any other error is the stream's own failure.
            if e.kind() == std::io::ErrorKind::WouldBlock
                && let Some(slot) = &self.2
            {
                slot.note_given_way();
            }
            self.1.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert_with(|| e.to_string());
        })
    }
}

impl Seek for SharedBytes {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.with(|s| s.seek(pos))
    }
}

impl MediaSource for SharedBytes {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        self.with(|s| s.byte_len())
    }
}

/// What a stream says about itself once probed.
pub(crate) struct StreamInfo {
    pub gain_db: Option<f32>,
    pub duration: Option<std::time::Duration>,
}

impl SymphoniaSource {
    pub(crate) fn open(path: &Path) -> Result<Self, DecodeError> {
        let file = File::open(path)?;
        let ext = path.extension().and_then(|e| e.to_str());
        let mut source = Self::open_media(Box::new(file), ext)?;
        if source.is_mkv() {
            let (path, ext) = (path.to_owned(), ext.map(str::to_owned));
            source.reopen = Some(Box::new(move || {
                let file = File::open(&path)?;
                probe(Box::new(file), ext.as_deref())
            }));
        }
        Ok(source)
    }

    fn is_mkv(&self) -> bool {
        self.format.format_info().short_name == "matroska"
    }

    /// Probes and opens encoded bytes from a stream.
    pub(crate) fn open_stream(
        bytes: Box<dyn ByteSource>,
        slot: Option<Arc<Slot>>,
    ) -> Result<(Self, StreamInfo), DecodeError> {
        let hint = bytes
            .format_hint()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        let shared = Arc::new(Mutex::new(bytes));
        let failure = Arc::new(Mutex::new(None));
        let ext = (!hint.is_empty()).then_some(hint);
        let opened = Self::open_media(
            Box::new(SharedBytes(Arc::clone(&shared), Arc::clone(&failure), slot.clone())),
            ext.as_deref(),
        );
        let mut source = match opened {
            Ok(s) => s,
            Err(e) => {
                let why = failure.lock().unwrap_or_else(PoisonError::into_inner).take();
                return Err(why.map_or(e, DecodeError::Stream));
            }
        };
        if source.is_mkv() {
            let shared = Arc::clone(&shared);
            source.reopen = Some(Box::new(move || {
                shared
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .seek(SeekFrom::Start(0))?;
                probe(
                    Box::new(SharedBytes(
                        Arc::clone(&shared),
                        Arc::new(Mutex::new(None)),
                        slot.clone(),
                    )),
                    ext.as_deref(),
                )
            }));
        }
        let info = {
            let s = shared.lock().unwrap_or_else(PoisonError::into_inner);
            StreamInfo {
                gain_db: s.gain_db().filter(|g| g.is_finite()),
                duration: s.duration(),
            }
        };
        Ok((source, info))
    }

    fn open_media(media: Box<dyn MediaSource>, ext: Option<&str>) -> Result<Self, DecodeError> {
        Self::from_format(probe(media, ext)?)
    }

    /// Timeline end of a packet.
    fn packet_end(&self, packet: &Packet) -> i64 {
        self.ts_to_frames(packet.pts) + self.dur_to_frames(packet.dur.get() as i64) - self.origin
            + self.pts_fix
    }

    /// Matroska seek: skip whole packets up to the one holding timeline
    /// frame `from`, which is kept for the next read. From where the demuxer
    /// is when `from` lies ahead of it (`ahead`: it is not at its end), else
    /// from a reopen at byte 0.
    fn seek_by_skipping(&mut self, from: i64, ahead: bool) -> Result<(), DecodeError> {
        let Some(reopen) = &self.reopen else {
            return Err(DecodeError::Seek("cannot reopen".into()));
        };
        // A millisecond of margin: the first packet's end may be noted
        // before `pts_fix` is learnt.
        let margin = i64::from(self.rate / 1000);
        if !(ahead && from >= self.demuxed_end.saturating_add(margin)) {
            self.format = reopen()?;
            self.demuxed_end = i64::MIN;
        }
        self.held = None;
        loop {
            match self.format.next_packet() {
                Ok(Some(packet)) => {
                    if packet.track_id != self.track_id {
                        continue;
                    }
                    let end = self.packet_end(&packet);
                    self.demuxed_end = end;
                    if end > from {
                        self.held = Some(packet);
                        return Ok(());
                    }
                }
                Ok(None) | Err(SymError::ResetRequired) => {
                    self.eof = true;
                    return Ok(());
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.eof = true;
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        }
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
        let is_mkv = format.format_info().short_name == "matroska";
        // Matroska tracks carry no length of their own; the segment does.
        let duration = track.duration.or_else(|| {
            let media = format.media_info();
            media
                .duration
                .filter(|_| is_mkv && media.time_base == track.time_base)
        });
        let is_opus = params.codec == CODEC_ID_OPUS;

        let mut rate = params.sample_rate.unwrap_or(0);
        let mut channels = params.channels.as_ref().map_or(0, |c| c.count());
        let mut exact_start = None;

        let (dec, origin) = if is_opus {
            let extra = params
                .extra_data
                .as_deref()
                .ok_or_else(|| DecodeError::Format("opus stream without OpusHead".into()))?;
            let head = OpusHead::parse(extra)?;
            rate = 48_000;
            channels = head.channels;
            // Matroska timestamps already exclude the pre-skip (CodecDelay).
            let origin = if is_mkv { 0 } else { i64::from(head.pre_skip) };
            if is_mkv {
                exact_start = Some(-i64::from(head.pre_skip));
            }
            (Dec::Opus(Box::new(OpusPackets::new(head)?)), origin)
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
            exact_start,
            pts_fix: 0,
            reopen: None,
            held: None,
            demuxed_end: i64::MIN,
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

fn probe(
    media: Box<dyn MediaSource>,
    ext: Option<&str>,
) -> Result<Box<dyn FormatReader>, DecodeError> {
    let mss = MediaSourceStream::new(media, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = ext {
        hint.with_extension(ext);
    }
    Ok(symphonia::default::get_probe().probe(
        &hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?)
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
            let next = match self.held.take() {
                Some(packet) => Ok(Some(packet)),
                None => self.format.next_packet(),
            };
            let packet = match next {
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
            // Every packet, the ones read while learning the format too.
            self.demuxed_end = self.demuxed_end.max(self.packet_end(&packet));
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
                // Within a millisecond of the exact start: it is the start,
                // and the difference is the delay's rounding.
                if let Some(exact) = self.exact_start.take()
                    && (first - exact).abs() <= i64::from(self.rate / 1000)
                {
                    self.pts_fix = exact - first;
                }
                self.emitted = first + self.pts_fix;
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
        if self.reopen.is_some() {
            let ahead = !self.eof;
            self.eof = false;
            self.seek_by_skipping(from, ahead)?;
            self.reset_decoder()?;
            self.resync = true;
            self.skip_to = target;
            self.bad_packets = 0;
            self.prefetch = None;
            self.exact_start = None;
            return Ok(());
        }
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
        self.exact_start = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that fails before its first byte, the way a refused
    /// Spotify key or a YouTube age gate does.
    struct Refused;

    impl Read for Refused {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("youtube: age-restricted"))
        }
    }

    impl Seek for Refused {
        fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
            Ok(0)
        }
    }

    impl ByteSource for Refused {
        fn byte_len(&self) -> Option<u64> {
            None
        }
        fn format_hint(&self) -> &str {
            "ogg"
        }
    }

    /// WebM bytes counting the seeks back to byte 0 (the reopens).
    struct Counted(std::io::Cursor<&'static [u8]>, Arc<std::sync::atomic::AtomicUsize>);

    impl Read for Counted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Seek for Counted {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            if pos == SeekFrom::Start(0) {
                self.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            self.0.seek(pos)
        }
    }

    impl ByteSource for Counted {
        fn byte_len(&self) -> Option<u64> {
            Some(self.0.get_ref().len() as u64)
        }
        fn format_hint(&self) -> &str {
            "webm"
        }
    }

    /// The first timeline frame `s` gives after a seek.
    fn first_after(s: &mut SymphoniaSource) -> i64 {
        let mut out = Vec::new();
        assert!(matches!(s.read(&mut out).unwrap(), ReadStatus::Data));
        s.emitted - (out.len() / s.channels) as i64
    }

    #[test]
    fn matroska_seeks_ahead_without_reopening() {
        let bytes: &'static [u8] = include_bytes!("../../tests/fixtures/tone.webm");
        let reopens = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let src = Counted(std::io::Cursor::new(bytes), Arc::clone(&reopens));
        let (mut s, _) = SymphoniaSource::open_stream(Box::new(src), None).unwrap();
        assert!(s.is_mkv());
        let mut out = Vec::new();
        s.read(&mut out).unwrap();
        let rate = i64::from(s.rate);
        let base = reopens.load(std::sync::atomic::Ordering::Relaxed);
        // Ahead of what was demuxed: no reopen, and it lands where asked.
        s.seek((rate / 2) as u64).unwrap();
        assert_eq!(reopens.load(std::sync::atomic::Ordering::Relaxed), base, "reopened to seek ahead");
        assert_eq!(first_after(&mut s), rate / 2);
        // Back: a reopen, and it lands as exactly.
        s.seek((rate / 10) as u64).unwrap();
        let back = reopens.load(std::sync::atomic::Ordering::Relaxed);
        assert!(back > base, "a seek back reopens");
        assert_eq!(first_after(&mut s), rate / 10);
        // Ahead again from there, still without one.
        s.seek((rate * 8 / 10) as u64).unwrap();
        assert_eq!(reopens.load(std::sync::atomic::Ordering::Relaxed), back);
        assert_eq!(first_after(&mut s), rate * 8 / 10);
    }

    #[test]
    fn a_stream_that_fails_says_its_own_reason() {
        let Err(e) = SymphoniaSource::open_stream(Box::new(Refused), None) else { panic!("opened") };
        assert!(e.to_string().contains("age-restricted"), "{e}");
    }
}
